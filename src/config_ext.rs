use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct MirrorConfig {
    pub agent_messages: bool,
    pub user_messages: bool,
    pub commands: bool,
    pub file_changes: bool,
}

impl Default for MirrorConfig {
    fn default() -> Self {
        Self {
            agent_messages: true,
            user_messages: false,
            commands: false,
            file_changes: false,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct StreamConfig {
    pub live: bool,
}

impl Default for StreamConfig {
    fn default() -> Self {
        Self { live: true }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ApprovalsConfig {
    pub ttl_minutes: u64,
}

impl Default for ApprovalsConfig {
    fn default() -> Self {
        Self { ttl_minutes: 30 }
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct BridgeConfig {
    pub mirror: MirrorConfig,
    pub stream: StreamConfig,
    pub approvals: ApprovalsConfig,
    pub auto_thread: AutoThreadConfig,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, rename_all = "camelCase")]
#[derive(Default)]
pub struct AutoThreadConfig {
    pub enabled: bool,
    pub category_id: Option<u64>,
}


impl BridgeConfig {
    pub fn load() -> Self {
        match std::fs::read_to_string("bridge.json") {
            Ok(text) => match serde_json::from_str(&text) {
                Ok(c) => {
                    tracing::info!("Loaded bridge.json config");
                    c
                }
                Err(e) => {
                    tracing::warn!("bridge.json invalid ({e}), using defaults");
                    Self::default()
                }
            },
            Err(_) => Self::default(),
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_autothread_camelcase_from_bridge_json() {
        let text = r#"{
            "mirror": {"agentMessages": true, "userMessages": false},
            "autoThread": {"enabled": true, "categoryId": 1551250166219280404}
        }"#;
        let cfg: BridgeConfig = serde_json::from_str(text).expect("valid bridge.json");
        assert!(cfg.auto_thread.enabled, "autoThread.enabled must parse");
        assert_eq!(cfg.auto_thread.category_id, Some(1551250166219280404));
    }

    #[test]
    fn missing_autothread_defaults_to_disabled() {
        let cfg: BridgeConfig = serde_json::from_str("{}").unwrap();
        assert!(!cfg.auto_thread.enabled);
        assert!(cfg.auto_thread.category_id.is_none());
    }
}
