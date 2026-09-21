use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{Value, json};
use tokio::sync::mpsc;

use codex_discord_bridge::codex::{CodexEvent, CodexTransport};
use codex_discord_bridge::state::{BridgeState, FreeFormRoute};

type Calls = Arc<tokio::sync::Mutex<Vec<TestRequest>>>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TestRequest {
    StartThread {
        cwd: String,
        model: Option<String>,
    },
    ResumeThread {
        thread_id: String,
    },
    StartTurn {
        thread_id: String,
        text: String,
        image_urls: Vec<String>,
    },
}

#[derive(Debug, Clone)]
struct TestCodex {
    calls: Calls,
}

#[async_trait::async_trait]
impl CodexTransport for TestCodex {
    async fn start_thread(
        &self,
        cwd: &str,
        _approval_policy: &str,
        _sandbox: &str,
        model: Option<&str>,
    ) -> Result<String, String> {
        self.record(TestRequest::StartThread {
            cwd: cwd.to_string(),
            model: model.map(str::to_string),
        })
        .await;
        Ok("test-thread".to_string())
    }

    async fn resume_thread(&self, thread_id: &str) -> Result<Value, String> {
        self.record(TestRequest::ResumeThread {
            thread_id: thread_id.to_string(),
        })
        .await;
        Ok(json!({}))
    }

    async fn start_turn(&self, thread_id: &str, text: &str) -> Result<Value, String> {
        self.start_turn_with_content(thread_id, text, &[]).await
    }

    async fn start_turn_with_content(
        &self,
        thread_id: &str,
        text: &str,
        image_urls: &[String],
    ) -> Result<Value, String> {
        self.record(TestRequest::StartTurn {
            thread_id: thread_id.to_string(),
            text: text.to_string(),
            image_urls: image_urls.to_vec(),
        })
        .await;
        Ok(json!({}))
    }

    async fn steer_turn(
        &self,
        thread_id: &str,
        _expected_turn_id: &str,
        text: &str,
    ) -> Result<Value, String> {
        self.start_turn(thread_id, text).await
    }

    async fn respond_to_server_request(
        &self,
        _request_id: &Value,
        _result: Value,
    ) -> Result<(), String> {
        Ok(())
    }
}

impl TestCodex {
    fn new(calls: Calls) -> Arc<Self> {
        Arc::new(Self { calls })
    }

    async fn record(&self, request: TestRequest) {
        self.calls.lock().await.push(request);
    }

    async fn calls(&self) -> Vec<TestRequest> {
        self.calls.lock().await.clone()
    }
}

type TestState = Arc<BridgeState<TestCodex>>;

async fn new_state(calls: Calls) -> (TestState, PathBuf) {
    let (event_tx, _event_rx) = mpsc::unbounded_channel::<CodexEvent>();
    let temp_dir = tempfile::TempDir::new().unwrap();
    let state_path = temp_dir.path().join("state.json");
    let state = BridgeState::with_path(event_tx, state_path.clone());
    *state.codex.write().await = Some(TestCodex::new(calls));
    (state, state_path)
}

#[tokio::test]
async fn second_mapped_thread_message_does_not_start_new_thread() {
    let calls: Calls = Arc::default();
    let (state, _state_path) = new_state(calls).await;

    let discord_thread_id = 155_141_373_043_933_190;
    let codex_thread_id = "01a0c1b4-ef00-7120-ada1-da5012c6840f";
    state.map_thread(codex_thread_id, discord_thread_id);

    let route =
        state.route_free_form_message(discord_thread_id, true, Some(155_125_016_621_928_040));
    assert_eq!(
        route,
        FreeFormRoute::MappedCodexThread(codex_thread_id.to_string()),
        "a mapped Discord thread must continue its mapped Codex thread"
    );

    state
        .send_to_codex(&discord_thread_id, "first message")
        .await
        .expect("first mapped message must send");
    state
        .send_to_codex(&discord_thread_id, "second message")
        .await
        .expect("second mapped message must send");

    assert_eq!(
        state.codex.read().await.as_ref().unwrap().calls().await,
        vec![
            TestRequest::StartTurn {
                thread_id: codex_thread_id.to_string(),
                text: "first message".to_string(),
                image_urls: Vec::new(),
            },
            TestRequest::StartTurn {
                thread_id: codex_thread_id.to_string(),
                text: "second message".to_string(),
                image_urls: Vec::new(),
            },
        ],
        "both messages must be turns on the one mapped Codex thread"
    );
}

#[tokio::test]
async fn mapped_thread_message_with_images_continues_same_thread() {
    let calls: Calls = Arc::default();
    let (state, _state_path) = new_state(calls).await;

    // The later mapping wins, proving the route uses the real reverse map.
    state.map_thread("codex-with-images", 200_100);
    state.map_thread("codex-first", 200_100);

    state
        .send_to_codex_with_images(
            &200_100,
            "look at this",
            &["https://example.test/image.png".to_string()],
        )
        .await
        .expect("mapped image message must send");

    assert_eq!(
        state.codex.read().await.as_ref().unwrap().calls().await,
        vec![TestRequest::StartTurn {
            thread_id: "codex-first".to_string(),
            text: "look at this".to_string(),
            image_urls: vec!["https://example.test/image.png".to_string()],
        }]
    );
}

#[test]
fn unmapped_message_with_autothread_starts_unmapped_thread() {
    let (event_tx, _event_rx) = mpsc::unbounded_channel();
    let temp_dir = tempfile::TempDir::new().unwrap();
    let state: Arc<BridgeState> =
        BridgeState::with_path(event_tx, temp_dir.path().join("state.json"));

    let route = state.route_free_form_message(300_100, true, Some(300_000));
    assert_eq!(route, FreeFormRoute::StartUnmappedThread);
}

#[test]
fn unmapped_message_without_autothread_starts_in_channel() {
    let (event_tx, _event_rx) = mpsc::unbounded_channel();
    let temp_dir = tempfile::TempDir::new().unwrap();
    let state: Arc<BridgeState> =
        BridgeState::with_path(event_tx, temp_dir.path().join("state.json"));

    let route = state.route_free_form_message(300_200, false, None);
    assert_eq!(route, FreeFormRoute::StartThreadInChannel);
}
