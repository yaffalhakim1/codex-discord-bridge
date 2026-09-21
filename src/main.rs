mod codex;
mod config;
mod config_ext;
mod discord;
mod options;
mod poster;
mod state;

use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::process::Command;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

use codex::{CodexClient, CodexEvent};
use config::Config;
use config_ext::BridgeConfig;
use poster::{
    SerenityPoster, finish_stream, handle_stream_delta, reset_stream, run_outbound_poster,
    should_mirror_agent_message,
};
use state::BridgeState;

static BRIDGE_CFG: std::sync::OnceLock<BridgeConfig> = std::sync::OnceLock::new();

pub use poster::DiscordOutbound;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let config = match Config::from_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Configuration error: {e}");
            eprintln!("\nCopy .env.example to .env and fill in your Discord credentials.");
            std::process::exit(1);
        }
    };

    let bridge_cfg = BridgeConfig::load();
    let _ = BRIDGE_CFG.set(bridge_cfg.clone());

    info!(
        "Starting codex-discord-bridge v{}",
        env!("CARGO_PKG_VERSION")
    );

    let (event_tx, mut event_rx) = mpsc::unbounded_channel::<CodexEvent>();
    let (outbound_tx, outbound_rx) = mpsc::unbounded_channel::<DiscordOutbound>();
    let state = BridgeState::new(event_tx.clone());
    *state.auto_thread.lock().unwrap() = bridge_cfg.auto_thread.clone();
    let codex_child: tokio::sync::Mutex<Option<tokio::process::Child>> =
        tokio::sync::Mutex::new(None);
    state.load_state();

    // Start the app-server only if nothing is already listening on the port.
    // Never attach the supervisor to a process this bridge does not own; it can
    // reconnect to an adopted server, but must only restart its own child.
    let listen_port = config.codex_port;
    let codex_cmd = config.codex_command.clone();
    let listen_url = format!("ws://127.0.0.1:{listen_port}");
    if is_codex_ready(listen_port, Duration::ZERO).await {
        info!("Reusing existing codex app-server on {listen_url}");
    } else {
        let child = spawn_codex(&codex_cmd, &listen_url);
        match child {
            Ok(c) => {
                info!("Spawned codex app-server: {codex_cmd} app-server --listen {listen_url}");
                *codex_child.lock().await = Some(c);
            }
            Err(e) => {
                error!("Failed to spawn codex: {e}");
                std::process::exit(1);
            }
        }
    }

    if !is_codex_ready(listen_port, Duration::from_secs(15)).await {
        error!("Codex app-server did not become ready on port {listen_port} in 15s.");
        std::process::exit(1);
    }
    info!("Codex app-server is listening on {listen_url}");

    // Connect to codex WebSocket
    let codex_client = match CodexClient::connect(&listen_url, event_tx.clone()).await {
        Ok(c) => c,
        Err(e) => {
            error!("Failed to connect to Codex: {e}");
            std::process::exit(1);
        }
    };
    *state.codex.write().await = Some(codex_client.clone());
    info!("Codex client ready.");

    // Spawn Discord bot
    let discord_config = config.clone();
    let discord_state = state.clone();
    let discord_token = discord_config.discord_bot_token.clone();
    let mut discord_client = serenity::Client::builder(
        &discord_token,
        serenity::model::gateway::GatewayIntents::GUILD_MESSAGES
            | serenity::model::gateway::GatewayIntents::MESSAGE_CONTENT,
    )
    .event_handler(discord::DiscordHandler::new(discord_config, discord_state))
    .await
    .expect("Failed to create Discord client");

    // Discord HTTP shard for outbound posts from the codex event loop
    let http = discord_client.http.clone();

    // Outbound poster task: receives DiscordOutbound and sends via serenity http
    let state_for_poster = state.clone();
    let poster = tokio::spawn(run_outbound_poster(
        outbound_rx,
        state_for_poster,
        SerenityPoster { http },
    ));

    let discord_handle = tokio::spawn(async move {
        if let Err(e) = discord_client.start().await {
            error!("Discord client error: {e}");
        }
    });

    // Codex event loop
    while let Some(event) = event_rx.recv().await {
        handle_codex_event(&event, &state, &outbound_tx, &config, &codex_child).await;
    }

    // Cleanup
    poster.abort();
    discord_handle.abort();
}

fn backoff_delay(attempt: u32) -> Duration {
    let backoff_ms = 500u64.saturating_mul(2u64.saturating_pow(attempt.min(3)));
    Duration::from_millis(backoff_ms.min(8_000))
}

fn spawn_codex(codex_cmd: &str, listen_url: &str) -> Result<tokio::process::Child, String> {
    Command::new(codex_cmd)
        .args(["app-server", "--listen", listen_url])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("spawn failed: {e}"))
}

/// Check whether the configured app-server port is accepting TCP connections.
/// This is intentionally bounded so a half-open listener cannot hang startup or
/// the reconnect supervisor.
async fn is_codex_ready(listen_port: u16, timeout: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if tokio::net::TcpStream::connect(("127.0.0.1", listen_port))
            .await
            .is_ok()
        {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

async fn handle_codex_event(
    event: &CodexEvent,
    state: &Arc<BridgeState>,
    outbound_tx: &mpsc::UnboundedSender<DiscordOutbound>,
    config: &Config,
    codex_child: &tokio::sync::Mutex<Option<tokio::process::Child>>,
) {
    match event {
        CodexEvent::ThreadStarted(thread) => {
            info!("[bridge] thread started: {}", thread.id);
            handle_auto_thread(thread, state, outbound_tx);
        }
        CodexEvent::ThreadStatusChanged { thread_id, status } => {
            let status_type = status["type"].as_str().unwrap_or("");
            debug!("[bridge] thread {thread_id} status: {status_type}");
        }
        CodexEvent::TurnStarted { thread_id, turn_id } => {
            debug!("[bridge] turn started on {thread_id}: {turn_id}");
            reset_stream(thread_id, state);
            if let Some(mapping) = state.thread_map.get(thread_id) {
                let channel_id = mapping.discord_channel_id;
                state.last_turn.insert(channel_id, turn_id.clone());
            }
        }
        CodexEvent::TurnCompleted { thread_id, .. } => {
            debug!("[bridge] turn completed on {thread_id}");
            finish_stream(
                thread_id,
                state,
                outbound_tx,
                BRIDGE_CFG.get().map(|c| c.stream.live).unwrap_or(true),
            );
            if let Some(mapping) = state.thread_map.get(thread_id) {
                let channel_id = mapping.discord_channel_id;
                state.last_turn.remove(&channel_id);
                let st = state.clone();
                let tid = thread_id.clone();
                tokio::spawn(async move {
                    st.flush_queue(&tid).await;
                });
            }
        }
        CodexEvent::ItemStarted { thread_id, item } => {
            let item_type = item["type"].as_str().unwrap_or("");
            debug!("[bridge] item started ({item_type}) on {thread_id}");
        }
        CodexEvent::ItemCompleted { thread_id, item } => {
            let item_type = item["type"].as_str().unwrap_or("");
            let mirror_allowed = should_mirror_agent_message(
                BRIDGE_CFG
                    .get()
                    .map(|c| c.mirror.agent_messages)
                    .unwrap_or(true),
                BRIDGE_CFG.get().map(|c| c.stream.live).unwrap_or(true),
                state
                    .streams
                    .try_read()
                    .map(|s| s.contains_key(thread_id))
                    .unwrap_or(false),
            );
            if item_type == "agentMessage" && !mirror_allowed {
                // The live stream owns the corresponding Discord message.
                return;
            }
            if let Some(mapping) = state.thread_map.get(thread_id) {
                let channel_id = mapping.discord_channel_id;
                mirror_item(channel_id, item_type, item, outbound_tx);
            }
        }
        CodexEvent::AgentMessageDelta { thread_id, delta } => {
            handle_stream_delta(
                thread_id,
                delta,
                state,
                outbound_tx,
                BRIDGE_CFG.get().map(|c| c.stream.live).unwrap_or(true),
                std::time::Duration::from_millis(poster::STREAM_DEBOUNCE_MS),
            );
        }
        CodexEvent::ApprovalRequest(req) => {
            info!("[bridge] approval request for thread {}", req.thread_id);
            handle_approval(req, state, outbound_tx);
        }
        CodexEvent::ApprovalResolved { .. } => {}
        CodexEvent::Disconnected => {
            error!("[bridge] Codex connection lost - restarting app-server");
            let active = state.mark_disconnected();
            for (channel_id, codex_thread_id) in active {
                if let Some(queue) = state.write_queue.get(&codex_thread_id)
                    && !queue.is_empty()
                {
                    let _ = outbound_tx.send(DiscordOutbound::Plain {
                        channel_id,
                        content:
                            "⚠️ Codex connection lost. The message will retry after reconnect."
                                .to_string(),
                    });
                }
            }

            // Kill only the child this bridge owns. Old clients are closed by
            // their read loop, so state.codex cannot retain a zombie sender.
            if let Some(mut child) = codex_child.lock().await.take()
                && let Err(e) = child.kill().await
            {
                warn!("Failed to terminate old codex app-server: {e}");
            }

            match restart_codex_supervisor(config, state).await {
                Ok(new_child) => {
                    *codex_child.lock().await = new_child;
                    state.clear_active_turns();
                    state.recover_after_reconnect().await;
                    info!("[bridge] Codex reconnected.");
                }
                Err(e) => error!("[bridge] reconnect failed: {e}"),
            }
        }

        CodexEvent::Other { method, .. } => {
            debug!("[bridge] codex event: {method}");
        }
    }
}

/// Restart only the app-server child. Discord state and mappings stay in this
/// process, so an existing Discord thread can continue on the new connection.
async fn restart_codex_supervisor(
    config: &Config,
    state: &Arc<BridgeState>,
) -> Result<Option<tokio::process::Child>, String> {
    let listen_port = config.codex_port;
    let listen_url = format!("ws://127.0.0.1:{listen_port}");
    let mut attempt = 0u32;
    loop {
        if is_codex_ready(listen_port, Duration::from_millis(100)).await {
            info!("Reusing existing codex app-server on {listen_url}");
            match CodexClient::connect(&listen_url, state.event_tx.clone()).await {
                Ok(client) => {
                    *state.codex.write().await = Some(client);
                    return Ok(None);
                }
                Err(e) => {
                    let delay = backoff_delay(attempt);
                    error!("[bridge] codex reconnect failed: {e}; retrying in {delay:?}");
                    tokio::time::sleep(delay).await;
                    attempt += 1;
                    continue;
                }
            }
        }

        let child = match spawn_codex(&config.codex_command, &listen_url) {
            Ok(child) => child,
            Err(e) => {
                let delay = backoff_delay(attempt);
                error!("[bridge] codex restart failed: {e}; retrying in {delay:?}");
                tokio::time::sleep(delay).await;
                attempt += 1;
                continue;
            }
        };
        info!(
            "Spawned codex app-server: {} app-server --listen {listen_url}",
            config.codex_command
        );
        *state.codex.write().await = None;
        if !is_codex_ready(listen_port, Duration::from_secs(15)).await {
            let mut child = child;
            let _ = child.kill().await;
            let delay = backoff_delay(attempt);
            error!("[bridge] codex not ready on {listen_url}; retrying in {delay:?}");
            tokio::time::sleep(delay).await;
            attempt += 1;
            continue;
        }
        match CodexClient::connect(&listen_url, state.event_tx.clone()).await {
            Ok(client) => {
                *state.codex.write().await = Some(client);
                return Ok(Some(child));
            }
            Err(e) => {
                let mut child = child;
                let _ = child.kill().await;
                let delay = backoff_delay(attempt);
                error!("[bridge] codex connect failed: {e}; retrying in {delay:?}");
                tokio::time::sleep(delay).await;
                attempt += 1;
            }
        }
    }
}
fn mirror_item(
    channel_id: u64,
    item_type: &str,
    item: &serde_json::Value,
    outbound_tx: &mpsc::UnboundedSender<DiscordOutbound>,
) {
    let cfg = BRIDGE_CFG.get();
    let allowed = match item_type {
        "agentMessage" => true,
        "userMessage" => cfg.map(|c| c.mirror.user_messages).unwrap_or(false),
        "commandExecution" => cfg.map(|c| c.mirror.commands).unwrap_or(false),
        "fileChange" => cfg.map(|c| c.mirror.file_changes).unwrap_or(false),
        _ => false,
    };
    if !allowed {
        return;
    }
    let content = match item_type {
        "agentMessage" => {
            let text = item["text"].as_str().unwrap_or("");
            if text.is_empty() {
                return;
            }
            text.to_string()
        }
        "reasoning" => return,
        _ => return,
    };
    let _ = outbound_tx.send(DiscordOutbound::Plain {
        channel_id,
        content,
    });
}

fn handle_auto_thread(
    thread: &codex::ThreadInfo,
    state: &Arc<BridgeState>,
    outbound_tx: &mpsc::UnboundedSender<DiscordOutbound>,
) {
    let cfg = BRIDGE_CFG.get();
    let (enabled, category_id) = match cfg {
        Some(c) => (c.auto_thread.enabled, c.auto_thread.category_id),
        None => (false, None),
    };
    info!(
        "[autothread] gate: enabled={} category={:?}",
        enabled, category_id
    );
    if !enabled {
        return;
    }
    if state.thread_map.contains_key(&thread.id) {
        return;
    }
    let category = match category_id {
        Some(c) => c,
        None => return,
    };
    let name = crate::options::thread_title_from_message(
        thread.preview.as_deref().unwrap_or(thread.name.as_deref().unwrap_or("")),
    );
    let _ = outbound_tx.send(DiscordOutbound::CreateThread {
        category_id: category,
        name,
        codex_thread_id: thread.id.clone(),
    });
}
fn handle_approval(
    req: &codex::ApprovalRequest,
    state: &Arc<BridgeState>,
    outbound_tx: &mpsc::UnboundedSender<DiscordOutbound>,
) {
    let channel_id = match state.thread_map.get(&req.thread_id) {
        Some(m) => m.discord_channel_id,
        None => {
            warn!("Approval for unmapped thread {} — ignoring", req.thread_id);
            return;
        }
    };

    let token = uuid::Uuid::new_v4().to_string();
    let command = req.command.as_deref().unwrap_or("(unknown)");
    let title = match req.method.as_str() {
        "item/commandExecution/requestApproval" => "🔐 Command Approval",
        "item/fileChange/requestApproval" => "🔐 File Change Approval",
        "item/permissions/requestApproval" => "🔐 Permissions Request",
        _ => "🔐 Approval Request",
    };
    let detail = format!(
        "**Command:** `{command}`\n**CWD:** `{}`",
        req.cwd.as_deref().unwrap_or("unknown")
    );

    state.register_approval(token.clone(), req, channel_id, None);
    let _ = outbound_tx.send(DiscordOutbound::ApprovalCard {
        channel_id,
        token,
        title: title.to_string(),
        detail,
    });
}
