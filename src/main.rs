mod codex;
mod config;
mod discord;
mod state;

use serenity::all::{ChannelId, CreateMessage};
use std::process::Stdio;
use std::sync::Arc;
use tokio::process::Command;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

use codex::{CodexClient, CodexEvent};
use config::Config;
use state::BridgeState;

/// A message to post to a Discord channel from the Codex event loop.
#[derive(Debug)]
pub enum DiscordOutbound {
    Plain { channel_id: u64, content: String },
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

    info!("Starting codex-discord-bridge v{}", env!("CARGO_PKG_VERSION"));

    let (event_tx, mut event_rx) = mpsc::unbounded_channel::<CodexEvent>();
    let (outbound_tx, mut outbound_rx) = mpsc::unbounded_channel::<DiscordOutbound>();
    let state = BridgeState::new(event_tx.clone());

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
    let outbound_state = state.clone();

    // Outbound poster task: receives DiscordOutbound and sends via serenity http
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
        handle_codex_event(&event, &outbound_state, &outbound_tx).await;
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
        CodexEvent::AgentMessageDelta { .. } => {}
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
    let content = match item_type {
        "agentMessage" => {
            let text = item["text"].as_str().unwrap_or("");
            if text.is_empty() { return; }
            format!("🤖 **Codex:**\n{text}")
        }
        "userMessage" => {
            let text = item["content"][0]["text"].as_str().unwrap_or("");
            if text.is_empty() { return; }
            format!("👤 **You:**\n{text}")
        }
        "commandExecution" => {
            let cmd = item["command"].as_str().unwrap_or("");
            let status = item["status"].as_str().unwrap_or("");
            format!("🖥️ **Command** ({status}): `{cmd}`")
        }
        "reasoning" => return,
        "fileChange" => {
            format!("📝 **File change**: {}", item["id"].as_str().unwrap_or(""))
        }
        _ => return,
    };
    let _ = outbound_tx.send(DiscordOutbound::Plain { channel_id, content });
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