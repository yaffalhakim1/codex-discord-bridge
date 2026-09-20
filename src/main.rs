mod codex;
mod options;
mod config_ext;
mod config;
mod discord;
mod state;

use serenity::all::{ChannelId, CreateMessage, MessageId};
use std::process::Stdio;
use std::sync::Arc;
use tokio::process::Command;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

use codex::{CodexClient, CodexEvent};
use config::Config;
use config_ext::BridgeConfig;
use state::{BridgeState, StreamState};

/// A message to post to a Discord channel from the Codex event loop.
static BRIDGE_CFG: std::sync::OnceLock<BridgeConfig> = std::sync::OnceLock::new();

pub enum DiscordOutbound {
    Plain { channel_id: u64, content: String },
    EditStream { channel_id: u64, message_id: u64, content: String },
    StartStream { channel_id: u64, content: String, thread_id: String },
    CreateThread { category_id: u64, name: String, codex_thread_id: String },
    ApprovalCard { channel_id: u64, token: String, title: String, detail: String },
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
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
    info!("Starting codex-discord-bridge v{}", env!("CARGO_PKG_VERSION"));

    let (event_tx, mut event_rx) = mpsc::unbounded_channel::<CodexEvent>();
    let (outbound_tx, mut outbound_rx) = mpsc::unbounded_channel::<DiscordOutbound>();
    let state = BridgeState::new(event_tx.clone());
    state.load_state();

    // Spawn codex app-server process
    let listen_port = config.codex_port;
    let codex_cmd = config.codex_command.clone();
    let listen_url = format!("ws://127.0.0.1:{listen_port}");
    let child = Command::new(&codex_cmd)
        .args(["app-server", "--listen", &listen_url])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn();
    let mut child = match child {
        Ok(c) => {
            info!("Spawned codex app-server: {codex_cmd} app-server --listen {listen_url}");
            c
        }
        Err(e) => {
            error!("Failed to spawn codex: {e}");
            std::process::exit(1);
        }
    };

    // Wait for port ready
    let mut port_ready = false;
    for _ in 0..30 {
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
        if tokio::net::TcpStream::connect(("127.0.0.1", listen_port)).await.is_ok() {
            port_ready = true;
            break;
        }
    }
    if !port_ready {
        error!("Codex app-server did not become ready on port {listen_port} in 15s.");
        std::process::exit(1);
    }
    info!("Codex app-server is listening on {listen_url}");

    // Connect to codex WebSocket
    let codex_client = match CodexClient::connect(&listen_url, event_tx).await {
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
    let poster = tokio::spawn(async move {
        while let Some(msg) = outbound_rx.recv().await {
            match msg {
                DiscordOutbound::Plain { channel_id, content } => {
                    let content = if content.len() > 1900 {
                        format!("{}\n… (truncated)", &content[..1900])
                    } else {
                        content
                    };
                    let _ = ChannelId::new(channel_id)
                        .send_message(&http, CreateMessage::new().content(content))
                        .await;
                }
                DiscordOutbound::StartStream { channel_id, content, thread_id } => {
                    match ChannelId::new(channel_id)
                        .send_message(&http, CreateMessage::new().content(content))
                        .await
                    {
                        Ok(msg) => {
                            if let Some(mut st) = state_for_poster.streams.get_mut(&thread_id) {
                                st.discord_message_id = Some(msg.id.get());
                            }
                        }
                        Err(e) => error!("Failed to start stream: {e}"),
                    }
                }
                DiscordOutbound::EditStream { channel_id, message_id, content } => {
                    let content = if content.len() > 1900 {
                        format!("{}\n… (truncated)", &content[..1900])
                    } else {
                        content
                    };
                    if let Err(e) = ChannelId::new(channel_id)
                        .edit_message(&http, MessageId::new(message_id), serenity::builder::EditMessage::new().content(content))
                        .await
                    {
                        debug!("Stream edit failed (may be rate-limited): {e}");
                    }
                }
                DiscordOutbound::CreateThread { category_id, name, codex_thread_id } => {
                    // Create a public thread; Discord needs a parent message or channel.
                    // We create a new forum-style thread via the guild API: start from a channel.
                    // Simplest supported path: create thread with a starter message in the category's channel.
                    // Serenity 0.12: GuildChannel::create_thread requires a parent channel.
                    // We use REST directly: POST /channels/{parent}/threads with name + type.
                    // category_id here is treated as a TEXT CHANNEL ID to hang the thread off.
                    let body = serde_json::json!({
                        "name": name,
                        "type": 11, // PUBLIC_THREAD
                        "auto_archive_duration": 1440
                    });
                    let payload = serde_json::json!({
                        "name": name,
                        "type": 11, // PUBLIC_THREAD
                        "auto_archive_duration": 1440
                    });
                    match http.create_thread(ChannelId::new(category_id), &payload, None).await {
                        Ok(ch) => {
                            let new_id = ch.id.get();
                            state_for_poster.map_thread(&codex_thread_id, new_id);
                            info!("[autothread] created Discord thread {new_id} for Codex {codex_thread_id}");
                        }
                        Err(e) => error!("[autothread] failed to create thread: {e}"),
                    }
                }
                DiscordOutbound::ApprovalCard { channel_id, token, title, detail } => {
                    if let Err(e) = discord::post_approval_card(&http, channel_id, &title, &detail, &token).await {
                        error!("Failed to post approval card: {e}");
                    }
                }
            }
        }
    });

    let discord_handle = tokio::spawn(async move {
        if let Err(e) = discord_client.start().await {
            error!("Discord client error: {e}");
        }
    });

    // Codex event loop
    while let Some(event) = event_rx.recv().await {
        handle_codex_event(&event, &state, &outbound_tx).await;
    }

    // Cleanup
    poster.abort();
    discord_handle.abort();
    let _ = child.kill().await;
}

async fn handle_codex_event(
    event: &CodexEvent,
    state: &Arc<BridgeState>,
    outbound_tx: &mpsc::UnboundedSender<DiscordOutbound>,
) {
    match event {
        CodexEvent::ThreadStarted(thread) => {
            debug!("[bridge] thread started: {}", thread.id);
            handle_auto_thread(thread, state, outbound_tx);
        }
        CodexEvent::ThreadStatusChanged { thread_id, status } => {
            let status_type = status["type"].as_str().unwrap_or("");
            debug!("[bridge] thread {thread_id} status: {status_type}");
        }
        CodexEvent::TurnStarted { thread_id, turn_id } => {
            debug!("[bridge] turn started on {thread_id}: {turn_id}");
            if let Some(mapping) = state.thread_map.get(thread_id) {
                let channel_id = mapping.discord_channel_id;
                state.last_turn.insert(channel_id, turn_id.clone());
            }
        }
        CodexEvent::TurnCompleted { thread_id, .. } => {
            debug!("[bridge] turn completed on {thread_id}");
            if let Some(mapping) = state.thread_map.get(thread_id) {
                let channel_id = mapping.discord_channel_id;
                state.last_turn.remove(&channel_id);
                let st = state.clone();
                let tid = thread_id.clone();
                tokio::spawn(async move { st.flush_queue(&tid).await; });
            }
        }
        CodexEvent::ItemStarted { thread_id, item } => {
            let item_type = item["type"].as_str().unwrap_or("");
            debug!("[bridge] item started ({item_type}) on {thread_id}");
        }
        CodexEvent::ItemCompleted { thread_id, item } => {
            let item_type = item["type"].as_str().unwrap_or("");
            if let Some(mapping) = state.thread_map.get(thread_id) {
                let channel_id = mapping.discord_channel_id;
                mirror_item(channel_id, item_type, item, outbound_tx);
            }
        }
        CodexEvent::AgentMessageDelta { thread_id, delta } => {
            handle_stream_delta(thread_id, delta, state, outbound_tx);
        }
        CodexEvent::ApprovalRequest(req) => {
            info!("[bridge] approval request for thread {}", req.thread_id);
            handle_approval(req, state, outbound_tx);
        }
        CodexEvent::ApprovalResolved { .. } => {}
        CodexEvent::Other { method, .. } => {
            debug!("[bridge] codex event: {method}");
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
        "agentMessage" => cfg.map(|c| c.mirror.agent_messages).unwrap_or(true),
        "userMessage" => cfg.map(|c| c.mirror.user_messages).unwrap_or(false),
        "commandExecution" => cfg.map(|c| c.mirror.commands).unwrap_or(false),
        "fileChange" => cfg.map(|c| c.mirror.file_changes).unwrap_or(false),
        _ => false,
    };
    if !allowed { return; }
    let content = match item_type {
        "agentMessage" => {
            let text = item["text"].as_str().unwrap_or("");
            if text.is_empty() { return; }
            text.to_string()
        }
        "reasoning" => return,
        _ => return,
    };
    let _ = outbound_tx.send(DiscordOutbound::Plain { channel_id, content });
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
    if !enabled { return; }
    if state.thread_map.contains_key(&thread.id) { return; }
    let category = match category_id {
        Some(c) => c,
        None => return,
    };
    let name = thread
        .name
        .clone()
        .or_else(|| thread.preview.as_ref().map(|p| {
            let t: String = p.chars().take(40).collect();
            if p.chars().count() > 40 { format!("{t}…") } else { t }
        }))
        .unwrap_or_else(|| "Codex thread".to_string());
    let _ = outbound_tx.send(DiscordOutbound::CreateThread {
        category_id: category,
        name,
        codex_thread_id: thread.id.clone(),
    });
}
fn handle_stream_delta(
    thread_id: &str,
    delta: &str,
    state: &Arc<BridgeState>,
    outbound_tx: &mpsc::UnboundedSender<DiscordOutbound>,
) {
    if BRIDGE_CFG.get().map(|c| !c.stream.live).unwrap_or(false) { return; }
    const DEBOUNCE_MS: u128 = 1500;
    const MAX_LEN: usize = 1900;
    let channel_id = match state.thread_map.get(thread_id) {
        Some(m) => m.discord_channel_id,
        None => return,
    };
    let mut entry = state.streams.entry(thread_id.to_string()).or_insert(StreamState {
        text: String::new(),
        discord_message_id: None,
        last_flush: None,
        dirty: false,
    });
    entry.text.push_str(delta);
    entry.dirty = true;
    let now = std::time::Instant::now();
    let elapsed_ok = entry.last_flush.map(|t| now.duration_since(t).as_millis() >= DEBOUNCE_MS).unwrap_or(true);
    let near_limit = entry.text.len() >= MAX_LEN;
    if elapsed_ok || near_limit {
        let text = std::mem::take(&mut entry.text);
        entry.dirty = false;
        entry.last_flush = Some(now);
        let mid = entry.discord_message_id;
        drop(entry);
        match mid {
            Some(id) => {
                let _ = outbound_tx.send(DiscordOutbound::EditStream { channel_id, message_id: id, content: text });
            }
            None => {
                let _ = outbound_tx.send(DiscordOutbound::StartStream { channel_id, content: text, thread_id: thread_id.to_string() });
            }
        }
    }
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
    let _ = outbound_tx.send(DiscordOutbound::ApprovalCard { channel_id, token, title: title.to_string(), detail });
}