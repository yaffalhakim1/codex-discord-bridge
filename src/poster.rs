use async_trait::async_trait;
use serenity::all::{ChannelId, CreateMessage, MessageId};
use serenity::http::Http;
use tokio::sync::mpsc;
use tracing::error;

use crate::codex::CodexTransport;
use crate::discord;
use crate::state::{BridgeState, StreamState};

/// A message to post to Discord from the Codex event loop.
#[derive(Debug)]
pub enum DiscordOutbound {
    Plain {
        channel_id: u64,
        content: String,
    },
    EditStream {
        channel_id: u64,
        message_id: u64,
        content: String,
    },
    StartStream {
        channel_id: u64,
        content: String,
        thread_id: String,
    },
    CreateThread {
        category_id: u64,
        name: String,
        codex_thread_id: String,
    },
    ApprovalCard {
        channel_id: u64,
        token: String,
        title: String,
        detail: String,
    },
}

#[async_trait]
pub trait DiscordPoster: Send + Sync + 'static {
    async fn send_plain(&self, channel_id: u64, content: String) -> Result<(), serenity::Error>;
    async fn send_start_stream(
        &self,
        channel_id: u64,
        content: String,
    ) -> Result<MessageId, serenity::Error>;
    async fn edit_stream(
        &self,
        channel_id: u64,
        message_id: u64,
        content: String,
    ) -> Result<(), serenity::Error>;
    async fn create_thread(&self, category_id: u64, name: String) -> Result<u64, serenity::Error>;
    async fn send_approval_card(
        &self,
        channel_id: u64,
        title: String,
        detail: String,
        token: String,
    ) -> Result<(), serenity::Error>;
}

pub struct SerenityPoster {
    pub http: std::sync::Arc<Http>,
}

#[async_trait]
impl DiscordPoster for SerenityPoster {
    async fn send_plain(&self, channel_id: u64, content: String) -> Result<(), serenity::Error> {
        ChannelId::new(channel_id)
            .send_message(&self.http, CreateMessage::new().content(content))
            .await
            .map(|_| ())
    }

    async fn send_start_stream(
        &self,
        channel_id: u64,
        content: String,
    ) -> Result<MessageId, serenity::Error> {
        ChannelId::new(channel_id)
            .send_message(&self.http, CreateMessage::new().content(content))
            .await
            .map(|message| message.id)
    }

    async fn edit_stream(
        &self,
        channel_id: u64,
        message_id: u64,
        content: String,
    ) -> Result<(), serenity::Error> {
        ChannelId::new(channel_id)
            .edit_message(&self.http, MessageId::new(message_id), {
                serenity::builder::EditMessage::new().content(content)
            })
            .await
            .map(|_| ())
    }

    async fn create_thread(&self, category_id: u64, name: String) -> Result<u64, serenity::Error> {
        let payload = serde_json::json!({
            "name": name,
            "type": 11,
            "auto_archive_duration": 1440
        });
        self.http
            .create_thread(ChannelId::new(category_id), &payload, None)
            .await
            .map(|channel| channel.id.get())
    }

    async fn send_approval_card(
        &self,
        channel_id: u64,
        title: String,
        detail: String,
        token: String,
    ) -> Result<(), serenity::Error> {
        discord::post_approval_card(&self.http, channel_id, &title, &detail, &token)
            .await
            .map(|_| ())
    }
}

pub fn truncate_discord_message(content: String) -> String {
    if content.chars().count() > 1900 {
        let content: String = content.chars().take(1900).collect();
        format!("{content}\n… (truncated)")
    } else {
        content
    }
}

pub const STREAM_DEBOUNCE_MS: u64 = 1500;
pub const STREAM_MAX_LEN: usize = 1900;

pub fn handle_stream_delta<T>(
    thread_id: &str,
    delta: &str,
    state: &std::sync::Arc<BridgeState<T>>,
    outbound_tx: &mpsc::UnboundedSender<DiscordOutbound>,
    stream_live: bool,
    debounce: std::time::Duration,
) where
    T: CodexTransport,
{
    if !stream_live {
        return;
    }
    let channel_id = {
        let mapping = state.thread_map.get(thread_id);
        mapping.as_ref().map(|mapping| mapping.discord_channel_id)
    };
    let channel_id = match channel_id {
        Some(channel_id) => channel_id,
        None => return,
    };

    let now = std::time::Instant::now();
    let mut streams = state.streams.try_write().expect("stream map lock");
    let entry = streams.entry(thread_id.to_string()).or_insert(StreamState {
        text: String::new(),
        full_text: String::new(),
        discord_message_id: None,
        last_flush: None,
        dirty: false,
        last_flushed_text: None,
    });
    entry.text.push_str(delta);
    entry.dirty = true;

    let elapsed_ok = entry
        .last_flush
        .map(|last| now.duration_since(last) >= debounce)
        .unwrap_or(true);
    let near_limit = entry.text.len() >= STREAM_MAX_LEN;
    if !elapsed_ok && !near_limit {
        return;
    }

    let flushed = std::mem::take(&mut entry.text);
    entry.full_text.push_str(&flushed);
    entry.dirty = false;
    entry.last_flush = Some(now);
    let text = flushed;
    let full_text = entry.full_text.clone();
    let message_id = entry.discord_message_id;
    let last_flushed = entry.last_flushed_text.clone();
    drop(streams);

    // Skip redundant edits when Discord already has this exact text.
    if message_id.is_some() && last_flushed.as_deref() == Some(full_text.as_str()) {
        return;
    }

    if message_id.is_none() && state.pending_stream_starts.contains_key(thread_id) {
        return;
    }

    match message_id {
        Some(message_id) => {
            let _ = outbound_tx.send(DiscordOutbound::EditStream {
                channel_id,
                message_id,
                content: full_text,
            });
        }
        None => {
            state
                .pending_stream_starts
                .insert(thread_id.to_string(), ());
            let _ = outbound_tx.send(DiscordOutbound::StartStream {
                channel_id,
                content: text,
                thread_id: thread_id.to_string(),
            });
        }
    }
}
pub fn finish_stream<T>(
    thread_id: &str,
    state: &std::sync::Arc<BridgeState<T>>,
    outbound_tx: &mpsc::UnboundedSender<DiscordOutbound>,
    stream_live: bool,
) where
    T: CodexTransport,
{
    if !stream_live {
        return;
    }
    let full_text;
    let message_id;
    let had_new_text;
    {
        let mut streams = state.streams.try_write().expect("stream map lock");
        let Some(entry) = streams.get_mut(thread_id) else {
            return;
        };
        let pending = std::mem::take(&mut entry.text);
        had_new_text = !pending.is_empty();
        entry.full_text.push_str(&pending);
        entry.dirty = false;
        entry.last_flush = Some(std::time::Instant::now());
        full_text = entry.full_text.clone();
        message_id = entry.discord_message_id;
    }

    if full_text.is_empty() {
        state
            .streams
            .try_write()
            .expect("stream map lock")
            .remove(thread_id);
        state.pending_stream_starts.remove(thread_id);
        return;
    }

    let pending_start = message_id.is_none() && state.pending_stream_starts.contains_key(thread_id);
    match message_id {
        Some(message_id) if had_new_text => {
            let channel_id = {
                let mapping = state.thread_map.get(thread_id);
                mapping.as_ref().map(|mapping| mapping.discord_channel_id)
            };
            let channel_id = match channel_id {
                Some(channel_id) => channel_id,
                None => return,
            };
            let _ = outbound_tx.send(DiscordOutbound::EditStream {
                channel_id,
                message_id,
                content: full_text,
            });
        }
        None if !pending_start => {
            let channel_id = {
                let mapping = state.thread_map.get(thread_id);
                mapping.as_ref().map(|mapping| mapping.discord_channel_id)
            };
            let channel_id = match channel_id {
                Some(channel_id) => channel_id,
                None => return,
            };
            state
                .pending_stream_starts
                .insert(thread_id.to_string(), ());
            let _ = outbound_tx.send(DiscordOutbound::StartStream {
                channel_id,
                content: full_text,
                thread_id: thread_id.to_string(),
            });
        }
        Some(_) => {}
        // A start is already queued; its id is known only after the POST, so
        // dispatch_outbound sends the newer accumulated full_text immediately
        // after assigning the id.
        None => {}
    }
}
pub fn reset_stream<T>(thread_id: &str, state: &std::sync::Arc<BridgeState<T>>)
where
    T: CodexTransport,
{
    state
        .streams
        .try_write()
        .expect("stream map lock")
        .remove(thread_id);
    state.pending_stream_starts.remove(thread_id);
}

pub fn should_mirror_agent_message(
    mirror_agent_messages: bool,
    stream_live: bool,
    stream_in_progress: bool,
) -> bool {
    mirror_agent_messages && !(stream_live && stream_in_progress)
}

async fn dispatch_start_stream<T, P>(
    thread_id: &str,
    channel_id: u64,
    content: String,
    state: &std::sync::Arc<BridgeState<T>>,
    poster: &P,
) where
    T: CodexTransport,
    P: DiscordPoster,
{
    match poster.send_start_stream(channel_id, content.clone()).await {
        Ok(message_id) => {
            let catch_up;
            {
                let mut streams = state.streams.try_write().expect("stream map lock");
                if let Some(stream) = streams.get_mut(thread_id) {
                    stream.discord_message_id = Some(message_id.get());
                    stream.last_flushed_text = Some(content.clone());
                    catch_up = if !stream.full_text.is_empty() && stream.full_text != content {
                        Some(stream.full_text.clone())
                    } else {
                        None
                    };
                } else {
                    catch_up = None;
                }
            }
            state.pending_stream_starts.remove(thread_id);

            if let Some(content) = catch_up
                && let Err(e) = poster
                    .edit_stream(channel_id, message_id.get(), content)
                    .await
            {
                error!("Failed to catch up stream after first message: {e}");
            }
        }
        Err(e) => {
            error!("Failed to start stream: {e}");
            state.pending_stream_starts.remove(thread_id);
        }
    }
}

pub async fn dispatch_outbound<T, P>(
    message: DiscordOutbound,
    state: &std::sync::Arc<BridgeState<T>>,
    poster: &P,
) where
    T: CodexTransport,
    P: DiscordPoster,
{
    match message {
        DiscordOutbound::Plain {
            channel_id,
            content,
        } => {
            let content = truncate_discord_message(content);
            if let Err(e) = poster.send_plain(channel_id, content).await {
                error!("Failed to send message: {e}");
            }
        }
        DiscordOutbound::StartStream {
            channel_id,
            content,
            thread_id,
        } => {
            let content = truncate_discord_message(content);
            dispatch_start_stream(&thread_id, channel_id, content, state, poster).await;
        }
        DiscordOutbound::EditStream {
            channel_id,
            message_id,
            content,
        } => {
            let content = truncate_discord_message(content);
            if let Err(e) = poster.edit_stream(channel_id, message_id, content).await {
                error!("Stream edit failed (may be rate-limited): {e}");
            }
        }
        DiscordOutbound::CreateThread {
            category_id,
            name,
            codex_thread_id,
        } => match poster.create_thread(category_id, name).await {
            Ok(new_id) => {
                state.map_thread(&codex_thread_id, new_id);
            }
            Err(e) => error!("[autothread] failed to create thread: {e}"),
        },
        DiscordOutbound::ApprovalCard {
            channel_id,
            token,
            title,
            detail,
        } => {
            if let Err(e) = poster
                .send_approval_card(channel_id, title, detail, token)
                .await
            {
                error!("Failed to post approval card: {e}");
            }
        }
    }
}

pub async fn run_outbound_poster<T, P>(
    mut outbound_rx: mpsc::UnboundedReceiver<DiscordOutbound>,
    state: std::sync::Arc<BridgeState<T>>,
    poster: P,
) where
    T: CodexTransport,
    P: DiscordPoster,
{
    while let Some(message) = outbound_rx.recv().await {
        dispatch_outbound(message, &state, &poster).await;
    }
}
