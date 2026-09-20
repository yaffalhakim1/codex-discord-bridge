use dashmap::DashMap;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};
use tracing::{debug, error, info};

type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ThreadInfo {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub preview: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub status: Option<Value>,
    #[serde(default)]
    pub updated_at: Option<i64>,
    #[serde(flatten)]
    pub extra: Value,
}

#[derive(Debug, Clone)]
pub struct ApprovalRequest {
    pub request_id: Value,
    pub method: String,
    pub thread_id: String,
    pub turn_id: String,
    pub item_id: String,
    pub command: Option<String>,
    pub cwd: Option<String>,
    pub available_decisions: Option<Value>,
}

#[derive(Debug, Clone)]
pub enum CodexEvent {
    ThreadStarted(ThreadInfo),
    ThreadStatusChanged { thread_id: String, status: Value },
    TurnStarted { thread_id: String, turn_id: String },
    TurnCompleted { thread_id: String, turn_id: String },
    ItemStarted { thread_id: String, item: Value },
    ItemCompleted { thread_id: String, item: Value },
    AgentMessageDelta { thread_id: String, delta: String },
    ApprovalRequest(ApprovalRequest),
    ApprovalResolved { request_id: Value, decision: Value },
    Other { method: String, params: Value },
}

pub struct CodexClient {
    ws_tx: Mutex<Option<futures_util::stream::SplitSink<WsStream, tokio_tungstenite::tungstenite::Message>>>,
    next_id: AtomicU64,
    pending: Arc<DashMap<Value, oneshot::Sender<Result<Value, String>>>>,
    event_tx: mpsc::UnboundedSender<CodexEvent>,
}

impl CodexClient {
    pub async fn connect(
        url: &str,
        event_tx: mpsc::UnboundedSender<CodexEvent>,
    ) -> Result<Arc<Self>, String> {
        let (ws, _resp) = connect_async(url)
            .await
            .map_err(|e| format!("Failed to connect to Codex app-server at {url}: {e}"))?;
        let (sink, stream) = ws.split();
        let pending: Arc<DashMap<Value, oneshot::Sender<Result<Value, String>>>> =
            Arc::new(DashMap::new());
        let client = Arc::new(Self {
            ws_tx: Mutex::new(Some(sink)),
            next_id: AtomicU64::new(1),
            pending: pending.clone(),
            event_tx: event_tx.clone(),
        });

        // Read loop
        let pending_read = pending.clone();
        let event_tx_read = event_tx.clone();
        let mut reader = stream;
        tokio::spawn(async move {
            while let Some(msg) = reader.next().await {
                let msg = match msg {
                    Ok(m) => m,
                    Err(e) => {
                        error!("Codex WebSocket error: {e}");
                        break;
                    }
                };
                let text = match msg {
                    tokio_tungstenite::tungstenite::Message::Text(t) => t,
                    tokio_tungstenite::tungstenite::Message::Close(c) => {
                        info!("Codex WebSocket closed: {c:?}");
                        break;
                    }
                    _ => continue,
                };
                let parsed: Value = match serde_json::from_str(&text) {
                    Ok(v) => v,
                    Err(e) => {
                        debug!("Non-JSON frame: {e}");
                        continue;
                    }
                };

                // Server request (approval etc.)
                if let (Some(id), Some(method)) = (parsed.get("id"), parsed.get("method")) {
                    let method = method.as_str().unwrap_or("");
                    let params = parsed.get("params").cloned().unwrap_or_default();
                    let req_id = id.clone();
                    debug!("[codex] server request: {method}");

                    let event = if method.contains("requestApproval") || method == "execCommandApproval" || method == "applyPatchApproval" {
                        CodexEvent::ApprovalRequest(ApprovalRequest {
                            request_id: req_id.clone(),
                            method: method.to_string(),
                            thread_id: params["threadId"]
                                .as_str()
                                .or_else(|| params["conversationId"].as_str())
                                .unwrap_or("")
                                .to_string(),
                            turn_id: params["turnId"].as_str().unwrap_or("").to_string(),
                            item_id: params["itemId"]
                                .as_str()
                                .or_else(|| params["callId"].as_str())
                                .unwrap_or("")
                                .to_string(),
                            command: params["command"]
                                .as_str()
                                .map(String::from)
                                .or_else(|| {
                                    params["command"].as_array().map(|a| {
                                        a.iter()
                                            .filter_map(|v| v.as_str())
                                            .collect::<Vec<_>>()
                                            .join(" ")
                                    })
                                }),
                            cwd: params["cwd"].as_str().map(String::from),
                            available_decisions: params.get("availableDecisions").cloned(),
                        })
                    } else if method == "serverRequest/resolved" {
                        CodexEvent::ApprovalResolved {
                            request_id: req_id.clone(),
                            decision: params.clone(),
                        }
                    } else {
                        CodexEvent::Other {
                            method: method.to_string(),
                            params,
                        }
                    };
                    let _ = event_tx_read.send(event);
                    continue;
                }

                // Response to our request
                if let Some(id) = parsed.get("id") {
                    if let Some(entry) = pending_read.remove(id) {
                        let result = if let Some(err) = parsed.get("error") {
                            Err(err["message"].as_str().unwrap_or("Unknown error").to_string())
                        } else {
                            Ok(parsed.get("result").cloned().unwrap_or_default())
                        };
                        let _ = entry.1.send(result);
                    }
                    continue;
                }

                // Notification
                if let Some(method) = parsed.get("method").and_then(|m| m.as_str()) {
                    let params = parsed.get("params").cloned().unwrap_or_default();
                    let event = match method {
                        "thread/started" => {
                            let thread: Option<ThreadInfo> =
                                params["thread"].as_object().map(|_| {
                                    serde_json::from_value(params["thread"].clone()).unwrap_or(
                                        ThreadInfo {
                                            id: params["thread"]["id"]
                                                .as_str()
                                                .unwrap_or("")
                                                .to_string(),
                                            name: None,
                                            preview: None,
                                            cwd: None,
                                            source: None,
                                            status: None,
                                            updated_at: None,
                                            extra: json!({}),
                                        },
                                    )
                                });
                            if let Some(t) = thread {
                                CodexEvent::ThreadStarted(t)
                            } else {
                                CodexEvent::Other {
                                    method: method.to_string(),
                                    params,
                                }
                            }
                        }
                        "thread/status/changed" => CodexEvent::ThreadStatusChanged {
                            thread_id: params["threadId"].as_str().unwrap_or("").to_string(),
                            status: params["status"].clone(),
                        },
                        "turn/started" => CodexEvent::TurnStarted {
                            thread_id: params["threadId"].as_str().unwrap_or("").to_string(),
                            turn_id: params["turn"]["id"].as_str().unwrap_or("").to_string(),
                        },
                        "turn/completed" => CodexEvent::TurnCompleted {
                            thread_id: params["threadId"].as_str().unwrap_or("").to_string(),
                            turn_id: params["turn"]["id"].as_str().unwrap_or("").to_string(),
                        },
                        "item/started" => CodexEvent::ItemStarted {
                            thread_id: params["threadId"].as_str().unwrap_or("").to_string(),
                            item: params["item"].clone(),
                        },
                        "item/completed" => CodexEvent::ItemCompleted {
                            thread_id: params["threadId"].as_str().unwrap_or("").to_string(),
                            item: params["item"].clone(),
                        },
                        "item/agentMessage/delta" => Self::extract_agent_delta(&params),
                        _ => CodexEvent::Other {
                            method: method.to_string(),
                            params,
                        },
                    };
                    let _ = event_tx_read.send(event);
                }
            }
            info!("Codex read loop ended.");
        });

        // Initialize handshake
        let init = json!({
            "jsonrpc": "2.0",
            "id": client.next_id.fetch_add(1, Ordering::Relaxed),
            "method": "initialize",
            "params": {
                "clientInfo": { "name": "codex-discord-bridge", "version": "0.1.0" },
                "capabilities": { "experimentalApi": true }
            }
        });
        client.send_raw(init).await?;
        client
            .notify("initialized", json!({}))
            .await
            .map_err(|e| format!("initialized notify failed: {e}"))?;
        debug!("[codex] initialized notification sent");
        info!("Codex app-server client connected and initialized.");

        Ok(client)
    }

    fn extract_agent_delta(params: &Value) -> CodexEvent {
        let thread_id = params["threadId"].as_str().unwrap_or("").to_string();
        let delta = params["delta"].as_str().unwrap_or("").to_string();
        CodexEvent::AgentMessageDelta { thread_id, delta }
    }

    async fn send_raw(&self, payload: Value) -> Result<(), String> {
        let mut tx = self.ws_tx.lock().await;
        if let Some(sink) = tx.as_mut() {
            sink.send(tokio_tungstenite::tungstenite::Message::Text(
                serde_json::to_string(&payload).map_err(|e| e.to_string())?.into(),
            ))
            .await
            .map_err(|e| format!("WebSocket send failed: {e}"))?;
            Ok(())
        } else {
            Err("WebSocket is closed.".into())
        }
    }

    async fn request_inner(
        &self,
        method: &str,
        params: Value,
        timeout: Option<std::time::Duration>,
    ) -> Result<Value, String> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let payload = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        });
        let (tx, rx) = oneshot::channel();
        self.pending.insert(Value::from(id), tx);
        self.send_raw(payload).await?;

        let fut = async {
            match rx.await {
                Ok(result) => result,
                Err(_) => Err("request dropped (connection closed)".into()),
            }
        };
        match timeout {
            Some(d) => tokio::time::timeout(d, fut)
                .await
                .map_err(|_| format!("{method} timed out"))?,
            None => fut.await,
        }
    }

    pub async fn request(&self, method: &str, params: Value) -> Result<Value, String> {
        self.request_inner(method, params, Some(std::time::Duration::from_secs(30))).await
    }

    pub async fn request_timeout(&self, method: &str, params: Value, timeout: std::time::Duration) -> Result<Value, String> {
        self.request_inner(method, params, Some(timeout)).await
    }

    pub async fn notify(&self, method: &str, params: Value) -> Result<(), String> {
        self.send_raw(json!({ "jsonrpc": "2.0", "method": method, "params": params })).await
    }

    pub async fn respond_to_server_request(&self, request_id: &Value, result: Value) -> Result<(), String> {
        self.send_raw(json!({
            "jsonrpc": "2.0",
            "id": request_id,
            "result": result
        }))
        .await
    }

    pub async fn list_threads(&self, limit: i64) -> Result<Vec<ThreadInfo>, String> {
        let result = self
            .request("thread/list", json!({ "limit": limit, "sortKey": "updated_at" }))
            .await?;
        let data = result["data"].as_array().cloned().unwrap_or_default();
        Ok(data
            .into_iter()
            .filter_map(|v| serde_json::from_value(v).ok())
            .collect())
    }

    pub async fn start_thread(&self, cwd: &str, approval_policy: &str, sandbox: &str, model: Option<&str>) -> Result<String, String> {
        let mut params = json!({ "cwd": cwd, "approvalPolicy": approval_policy, "sandbox": sandbox });
        if let Some(m) = model {
            params["model"] = json!(m);
        }
        let result = self.request("thread/start", params).await?;
        result["thread"]["id"]
            .as_str()
            .map(String::from)
            .ok_or_else(|| "No thread id in response".into())
    }

    pub async fn list_models(&self) -> Result<Vec<Value>, String> {
        let result = self.request("model/list", json!({})).await?;
        let data = result["data"].as_array().cloned().unwrap_or_default();
        Ok(data)
    }

    pub async fn start_turn_with_model(&self, thread_id: &str, text: &str, model: Option<&str>) -> Result<Value, String> {
        let params = if let Some(m) = model {
            json!({ "threadId": thread_id, "input": [{ "type": "text", "text": text }], "model": m })
        } else {
            json!({ "threadId": thread_id, "input": [{ "type": "text", "text": text }] })
        };
        self.request("turn/start", params).await
    }
    pub async fn start_turn(&self, thread_id: &str, text: &str) -> Result<Value, String> {
        self.request(
            "turn/start",
            json!({ "threadId": thread_id, "input": [{ "type": "text", "text": text }] }),
        )
        .await
    }

    pub async fn resume_thread(&self, thread_id: &str) -> Result<Value, String> {
        self.request("thread/resume", json!({ "threadId": thread_id })).await
    }

    pub async fn steer_turn(&self, thread_id: &str, expected_turn_id: &str, text: &str) -> Result<Value, String> {
        self.request(
            "turn/steer",
            json!({ "threadId": thread_id, "expectedTurnId": expected_turn_id, "input": [{ "type": "text", "text": text }] }),
        )
        .await
    }
    pub async fn interrupt_turn(&self, thread_id: &str, turn_id: &str) -> Result<(), String> {
        self.request(
            "turn/interrupt",
            json!({ "threadId": thread_id, "turnId": turn_id }),
        )
        .await
        .map(|_| ())
    }

    pub async fn approve(&self, request_id: &Value) -> Result<(), String> {
        self.respond_to_server_request(request_id, json!({ "decision": "accept" })).await
    }

    pub async fn decline(&self, request_id: &Value) -> Result<(), String> {
        self.respond_to_server_request(request_id, json!({ "decision": "decline" })).await
    }

    pub async fn cancel(&self, request_id: &Value) -> Result<(), String> {
        self.respond_to_server_request(request_id, json!({ "decision": "cancel" })).await
    }
}