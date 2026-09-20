use dashmap::DashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::codex::{ApprovalRequest, CodexClient, CodexEvent};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ThreadMapping {
    pub codex_thread_id: String,
    pub discord_channel_id: u64,
    pub created_at: i64,
    pub last_activity_at: Option<i64>,
}

#[derive(Debug)]
pub struct StreamState {
    pub text: String,
    pub discord_message_id: Option<u64>,
    pub last_flush: Option<std::time::Instant>,
    pub dirty: bool,
}

#[derive(Debug, Clone)]
pub struct PendingApproval {
    pub token: String,
    pub request_id: serde_json::Value,
    pub thread_id: String,
    pub turn_id: String,
    pub item_id: String,
    pub discord_channel_id: u64,
    pub discord_message_id: Option<u64>,
    pub created_at: std::time::Instant,
}

#[derive(Debug, Clone)]
pub struct PendingWriteBack {
    pub id: String,
    pub thread_id: String,
    pub text: String,
    pub created_at: std::time::Instant,
}

pub struct BridgeState {
    pub codex: RwLock<Option<Arc<CodexClient>>>,
    /// codex_thread_id → discord_channel_id
    pub thread_map: DashMap<String, ThreadMapping>,
    /// discord_channel_id → codex_thread_id (reverse lookup)
    pub reverse_map: DashMap<u64, String>,
    /// approval_token → PendingApproval
    pub approvals: DashMap<String, PendingApproval>,
    /// thread_id → Vec<PendingWriteBack>
    pub write_queue: DashMap<String, Vec<PendingWriteBack>>,
    /// channel_id → latest codex turn_id (for steering)
    pub last_turn: DashMap<u64, String>,
    pub default_model: std::sync::Mutex<Option<String>>,
    pub auto_thread: std::sync::Mutex<crate::config_ext::AutoThreadConfig>,
    /// thread_id -> (accumulated_text, discord_message_id, last_flush)
    pub streams: DashMap<String, StreamState>,
    pub event_tx: mpsc::UnboundedSender<CodexEvent>,
}

pub type SharedState = Arc<BridgeState>;

impl BridgeState {
    pub fn new(event_tx: mpsc::UnboundedSender<CodexEvent>) -> SharedState {
        Arc::new(Self {
            codex: RwLock::new(None),
            thread_map: DashMap::new(),
            reverse_map: DashMap::new(),
            approvals: DashMap::new(),
            write_queue: DashMap::new(),
            last_turn: DashMap::new(),
            default_model: std::sync::Mutex::new(None),
            auto_thread: std::sync::Mutex::new(crate::config_ext::AutoThreadConfig::default()),
            streams: DashMap::new(),
            event_tx,
        })
    }

    const STATE_PATH: &'static str = "data/state.json";

    pub fn save_state(&self) {
        let entries: Vec<ThreadMapping> = self.thread_map.iter().map(|e| e.value().clone()).collect();
        let default_model = self.default_model.lock().ok().and_then(|g| g.clone());
        let json = serde_json::json!({
            "thread_map": entries,
            "default_model": default_model,
        });
        if let Err(e) = std::fs::create_dir_all("data") {
            warn!("Could not create data dir: {e}");
            return;
        }
        if let Err(e) = std::fs::write(Self::STATE_PATH, serde_json::to_string_pretty(&json).unwrap_or_default()) {
            warn!("Could not save state: {e}");
        } else {
            debug!("[state] saved {} mappings", entries.len());
        }
    }

    pub fn load_state(&self) {
        let text = match std::fs::read_to_string(Self::STATE_PATH) {
            Ok(t) => t,
            Err(_) => return,
        };
        let json: serde_json::Value = match serde_json::from_str(&text) {
            Ok(j) => j,
            Err(e) => { warn!("Corrupt state file: {e}"); return; }
        };
        if let Some(entries) = json["thread_map"].as_array() {
            for e in entries {
                let codex_id = e["codex_thread_id"].as_str().unwrap_or("").to_string();
                let channel = e["discord_channel_id"].as_u64().unwrap_or(0);
                if !codex_id.is_empty() && channel != 0 {
                    self.thread_map.insert(codex_id.clone(), ThreadMapping {
                        codex_thread_id: codex_id.clone(),
                        discord_channel_id: channel,
                        created_at: e["created_at"].as_i64().unwrap_or(0),
                        last_activity_at: e["last_activity_at"].as_i64(),
                    });
                    self.reverse_map.insert(channel, codex_id);
                }
            }
        }
        if let Some(m) = json["default_model"].as_str() {
            if let Ok(mut g) = self.default_model.lock() { *g = Some(m.to_string()); }
        }
        info!("[state] loaded {} mappings from disk", self.thread_map.len());
    }
    pub fn map_thread(&self, codex_id: &str, discord_channel_id: u64) {
        // If this channel was mapped to a different thread, drop the stale mapping.
        if let Some(prev) = self.reverse_map.get(&discord_channel_id) {
            let prev_id = prev.value().clone();
            if prev_id != codex_id {
                self.thread_map.remove(&prev_id);
            }
        }
        let entry = ThreadMapping {
            codex_thread_id: codex_id.to_string(),
            discord_channel_id,
            created_at: chrono::Utc::now().timestamp(),
            last_activity_at: None,
        };
        self.thread_map.insert(codex_id.to_string(), entry.clone());
        self.reverse_map.insert(discord_channel_id, codex_id.to_string());
        debug!("[state] mapped {codex_id} → channel {discord_channel_id}");
        self.save_state();
    }

    pub fn unmap_thread(&self, codex_id: &str) {
        if let Some(entry) = self.thread_map.remove(codex_id) {
            self.reverse_map.remove(&entry.1.discord_channel_id);
        }
        self.save_state();
    }

    pub async fn list_mapped_threads(&self) -> Vec<ThreadMapping> {
        self.thread_map.iter().map(|e| e.value().clone()).collect()
    }

    pub async fn attach_thread(&self, codex_thread_id: &str, discord_channel_id: u64) -> Result<(), String> {
        let codex = self.codex.read().await;
        let codex = codex
            .as_ref()
            .ok_or_else(|| "Codex client not connected yet.".to_string())?;
        // Verify thread exists
        let result = codex.resume_thread(codex_thread_id).await;
        if let Err(e) = result {
            // If already owned by another client, we still map for read-only
            warn!("Could not resume thread {codex_thread_id}: {e} — mapping read-only");
        }
        self.map_thread(codex_thread_id, discord_channel_id);
        Ok(())
    }

    pub async fn detach_thread(&self, codex_thread_id: &str) -> Result<(), String> {
        if !self.thread_map.contains_key(codex_thread_id) {
            return Err("Thread is not mapped.".into());
        }
        self.unmap_thread(codex_thread_id);
        Ok(())
    }

    /// Send a turn to a mapped thread. If Codex reports the thread is unknown
    /// (e.g. bridge restarted with a fresh app-server), resume it from disk and retry once.
    async fn start_turn_resilient(
        &self,
        codex: &CodexClient,
        thread_id: &str,
        text: &str,
    ) -> Result<serde_json::Value, String> {
        match codex.start_turn(thread_id, text).await {
            Ok(v) => Ok(v),
            Err(e) if e.contains("not found") || e.contains("no rollout") => {
                debug!("[state] thread {thread_id} not loaded — resuming from disk");
                codex.resume_thread(thread_id).await?;
                codex.start_turn(thread_id, text).await
            }
            Err(e) => Err(e),
        }
    }
    pub async fn send_to_codex(&self, discord_channel_id: &u64, text: &str) -> Result<Option<String>, String> {
        let codex_id = self
            .reverse_map
            .get(discord_channel_id)
            .map(|v| v.value().clone())
            .ok_or_else(|| "No thread mapped to this channel.".to_string())?;
        let codex = self.codex.read().await;
        let codex = codex
            .as_ref()
            .ok_or_else(|| "Codex client not connected.".to_string())?;
        // If a turn is active, queue. Otherwise send immediately.
        let last_turn = self.last_turn.get(discord_channel_id).map(|v| v.value().clone());
        if last_turn.is_some() {
            let wb = PendingWriteBack {
                id: Uuid::new_v4().to_string(),
                thread_id: codex_id.clone(),
                text: text.to_string(),
                created_at: std::time::Instant::now(),
            };
            self.write_queue.entry(codex_id.clone()).or_default().push(wb);
            Ok(Some("queued".to_string()))
        } else {
            self.start_turn_resilient(codex, &codex_id, text).await?;
            Ok(Some("sent".to_string()))
        }
    }

    pub async fn send_to_codex_with_images(
        &self,
        discord_channel_id: &u64,
        text: &str,
        image_urls: &[String],
    ) -> Result<Option<String>, String> {
        let codex_id = self
            .reverse_map
            .get(discord_channel_id)
            .map(|v| v.value().clone())
            .ok_or_else(|| "No thread mapped to this channel.".to_string())?;
        let codex = self.codex.read().await;
        let codex = codex
            .as_ref()
            .ok_or_else(|| "Codex client not connected.".to_string())?;
        if let Err(e) = codex.start_turn_with_content(&codex_id, text, image_urls).await {
            if e.contains("not found") || e.contains("no rollout") {
                codex.resume_thread(&codex_id).await?;
                codex.start_turn_with_content(&codex_id, text, image_urls).await?;
            } else { return Err(e); }
        }
        Ok(Some("sent".to_string()))
    }
    pub async fn steer_to_codex(&self, discord_channel_id: &u64, text: &str) -> Result<Option<String>, String> {
        let codex_id = self
            .reverse_map
            .get(discord_channel_id)
            .map(|v| v.value().clone())
            .ok_or_else(|| "No thread mapped to this channel.".to_string())?;
        let active_turn = self.last_turn.get(discord_channel_id).map(|v| v.value().clone());
        let codex = self.codex.read().await;
        let codex = codex
            .as_ref()
            .ok_or_else(|| "Codex client not connected.".to_string())?;
        if let Some(turn_id) = active_turn {
            codex.steer_turn(&codex_id, &turn_id, text).await?;
            Ok(Some("steered".to_string()))
        } else {
            self.start_turn_resilient(codex, &codex_id, text).await?;
            Ok(Some("sent".to_string()))
        }
    }
    pub async fn retract_pending(&self, discord_channel_id: &u64) -> Result<Option<String>, String> {
        let codex_id = self
            .reverse_map
            .get(discord_channel_id)
            .map(|v| v.value().clone())
            .ok_or_else(|| "No thread mapped.".to_string())?;
        if let Some(mut queue) = self.write_queue.get_mut(&codex_id) {
            if let Some(last) = queue.pop() {
                return Ok(Some(last.text));
            }
        }
        Ok(None)
    }

    /// Start a fresh Codex thread without mapping it to any channel.
    /// Used when autoThread is on: the Discord thread is created by the
    /// ThreadStarted handler and mapping happens there.
    pub async fn start_thread_unmapped(&self, prompt: &str) -> Result<String, String> {
        let codex = self.codex.read().await;
        let codex = codex
            .as_ref()
            .ok_or_else(|| "Codex client not connected.".to_string())?;
        let cwd = std::env::var("USERPROFILE")
            .or_else(|_| std::env::var("HOME"))
            .unwrap_or_default();
        let model = self.default_model.lock().ok().and_then(|g| g.clone());
        let thread_id = codex.start_thread(&cwd, "on-request", "read-only", model.as_deref()).await?;
        drop(codex);
        let codex = self.codex.read().await;
        if let Some(codex) = codex.as_ref() {
            self.start_turn_resilient(codex, &thread_id, prompt).await?;
        }
        Ok(thread_id)
    }
    pub async fn start_new_thread_in_channel(
        &self,
        discord_channel_id: &u64,
        prompt: &str,
        model: Option<&str>,
    ) -> Result<Option<String>, String> {
        let codex = self.codex.read().await;
        let codex = codex
            .as_ref()
            .ok_or_else(|| "Codex client not connected.".to_string())?;
        let cwd = std::env::var("USERPROFILE")
            .or_else(|_| std::env::var("HOME"))
            .unwrap_or_default();
        let model = match model { Some(m) => Some(m.to_string()), None => self.default_model.lock().ok().and_then(|g| g.clone()) };
        let thread_id = codex.start_thread(&cwd, "on-request", "read-only", model.as_deref()).await?;
        drop(codex);
        self.map_thread(&thread_id, *discord_channel_id);
        let codex = self.codex.read().await;
        if let Some(codex) = codex.as_ref() {
            self.start_turn_resilient(codex, &thread_id, prompt).await?;
        }
        self.last_turn.insert(*discord_channel_id, thread_id.clone());
        Ok(None)
    }
    pub async fn handle_approval_decision(&self, token: &str, decision: &str) -> Result<(), String> {
        let approval = self
            .approvals
            .get(token)
            .map(|a| a.value().clone())
            .ok_or_else(|| "Approval not found or already resolved.".to_string())?;
        // Expire after 30 minutes
        if approval.created_at.elapsed() > std::time::Duration::from_secs(1800) {
            self.approvals.remove(token);
            return Err("This approval request has expired.".to_string());
        }
        let codex = self.codex.read().await;
        let codex = codex
            .as_ref()
            .ok_or_else(|| "Codex client not connected.".to_string())?;
        let decision_json = match decision {
            "approve" => serde_json::json!({ "decision": "accept" }),
            "decline" => serde_json::json!({ "decision": "decline" }),
            "cancel" => serde_json::json!({ "decision": "cancel" }),
            _ => return Err("Unknown decision".into()),
        };
        codex
            .respond_to_server_request(&approval.request_id, decision_json)
            .await?;
        self.approvals.remove(token);
        info!("[state] approval {token} resolved as {decision}");
        Ok(())
    }

    pub fn register_approval(
        &self,
        token: String,
        req: &ApprovalRequest,
        discord_channel_id: u64,
        discord_message_id: Option<u64>,
    ) {
        let pa = PendingApproval {
            token,
            request_id: req.request_id.clone(),
            thread_id: req.thread_id.clone(),
            turn_id: req.turn_id.clone(),
            item_id: req.item_id.clone(),
            discord_channel_id,
            discord_message_id,
            created_at: std::time::Instant::now(),
        };
        self.approvals.insert(pa.token.clone(), pa);
    }

    /// Flush queued messages to codex when a turn completes.
    pub async fn flush_queue(&self, thread_id: &str) {
        let queue = self.write_queue.get_mut(thread_id);
        if let Some(mut q) = queue {
            if q.is_empty() {
                return;
            }
            let codex = self.codex.read().await;
            if let Some(codex) = codex.as_ref() {
                let msgs: Vec<String> = q.drain(..).map(|w| w.text).collect();
                for msg in msgs {
                    info!("[state] flushing queued message to {thread_id}");
                    if let Err(e) = codex.start_turn(thread_id, &msg).await {
                        error!("Failed to send queued message: {e}");
                    }
                }
            }
        }
    }
}
