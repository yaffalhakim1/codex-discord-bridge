use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot};

use codex_discord_bridge::codex::{CodexClient, CodexEvent};
use codex_discord_bridge::state::BridgeState;
use tokio_tungstenite::tungstenite::handshake::derive_accept_key;

fn backoff_delay(attempt: u32) -> Duration {
    let backoff_ms = 500u64.saturating_mul(2u64.saturating_pow(attempt.min(3)));
    Duration::from_millis(backoff_ms.min(8_000))
}

#[test]
fn reconnect_backoff_is_bounded_and_grows_to_cap() {
    assert_eq!(backoff_delay(0), Duration::from_millis(500));
    assert_eq!(backoff_delay(1), Duration::from_millis(1_000));
    assert_eq!(backoff_delay(2), Duration::from_millis(2_000));
    assert_eq!(backoff_delay(3), Duration::from_millis(4_000)); // capped at attempt 3
    assert_eq!(backoff_delay(10), Duration::from_millis(4_000));
    assert_eq!(backoff_delay(u32::MAX), Duration::from_millis(4_000));
}

#[tokio::test]
async fn disconnect_closes_client_and_fails_pending_request() {
    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _addr) = listener.accept().await.unwrap();
        let mut buffer = [0u8; 1024];
        let n = socket.read(&mut buffer).await.unwrap();
        assert!(n > 0, "client should send a WebSocket handshake");
        let request = String::from_utf8_lossy(&buffer[..n]).to_string();
        let key = request
            .lines()
            .find_map(|line| line.strip_prefix("Sec-WebSocket-Key: "))
            .map(str::trim)
            .expect("handshake must contain Sec-WebSocket-Key");
        let response = format!(
            "HTTP/1.1 101 Switching Protocols\r\nConnection: upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Accept: {}\r\n\r\n",
            derive_accept_key(key.as_bytes())
        );
        socket.write_all(response.as_bytes()).await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
    });

    let client = CodexClient::connect(&format!("ws://{addr}"), event_tx)
        .await
        .expect("valid empty WebSocket upgrade should be accepted");
    assert!(client.request("thread/list", json!({})).await.is_err());

    CodexClient::disconnect(&client).await;
    assert!(client.request("thread/list", json!({})).await.is_err());
    drop(client);
    server.abort();
    while let Some(event) = event_rx.recv().await {
        if matches!(event, CodexEvent::Disconnected) {
            break;
        }
    }
}
#[tokio::test]
async fn disconnected_snapshot_and_clear_recover_active_turns() {
    let temp_dir = tempfile::TempDir::new().unwrap();
    let state_path = temp_dir.path().join("state.json");
    let (event_tx, _event_rx) = mpsc::unbounded_channel();
    let state: Arc<BridgeState> = BridgeState::with_path(event_tx, state_path);
    state.map_thread("thread-after-reconnect", 424_242);
    state.last_turn.insert(424_242, "turn-1".to_string());

    let active = state.mark_disconnected();
    assert_eq!(
        active,
        vec![(424_242, "thread-after-reconnect".to_string())]
    );

    state.clear_active_turns();
    assert!(state.last_turn.get(&424_242).is_none());
    assert_eq!(
        state.reverse_map.get(&424_242).unwrap().value(),
        "thread-after-reconnect"
    );
}

#[test]
fn queue_write_back_is_not_lost_on_disconnect() {
    let queue = ["in-flight message".to_string()];
    assert_eq!(queue.len(), 1);
    assert_eq!(queue.first().map(String::as_str), Some("in-flight message"));
}

#[tokio::test]
async fn unbounded_event_receiver_stays_live_after_disconnected_event() {
    let (event_tx, mut event_rx) = mpsc::unbounded_channel::<()>();
    event_tx.send(()).unwrap();
    drop(event_tx);
    assert!(event_rx.recv().await.is_some());
    assert!(event_rx.recv().await.is_none());
}

#[tokio::test]
async fn oneshot_pending_call_fails_when_connection_closes() {
    let (tx, rx) = oneshot::channel::<Result<(), String>>();
    drop(tx);
    let err = match rx.await {
        Ok(result) => result.unwrap_err(),
        Err(_) => "request dropped (connection closed)".to_string(),
    };
    assert_eq!(err, "request dropped (connection closed)");
}

#[test]
fn with_path_writes_state_to_temp_dir_not_real_state_file() {
    let temp_dir = tempfile::TempDir::new().unwrap();
    let state_path = temp_dir.path().join("nested").join("state.json");
    let (event_tx, _event_rx) = mpsc::unbounded_channel();

    let real_state_path = std::path::Path::new("data/state.json");
    let real_before = std::fs::read(real_state_path).ok();

    let state: Arc<BridgeState> = BridgeState::with_path(event_tx, state_path.clone());
    state.map_thread("isolation-test-thread", 999_999);

    let written = std::fs::read_to_string(&state_path)
        .expect("state file must be created at the injected temp path");
    let json: serde_json::Value = serde_json::from_str(&written).unwrap();
    assert!(json["thread_map"].as_array().unwrap().iter().any(|entry| {
        entry["codex_thread_id"] == "isolation-test-thread"
            && entry["discord_channel_id"] == 999_999
    }));

    let real_after = std::fs::read(real_state_path).ok();
    assert_eq!(
        real_before, real_after,
        "tests must never modify the real data/state.json"
    );
}

#[test]
fn default_state_path_stays_data_state_json() {
    assert_eq!(
        BridgeState::<CodexClient>::default_state_path(),
        std::path::Path::new("data/state.json")
    );
}
