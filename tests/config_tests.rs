use serde_json::Value;

/// TDD: bridge config — visibility toggles with sane defaults.

#[derive(Debug)]
struct BridgeConfig {
    mirror_agent_messages: bool,
    mirror_user_messages: bool,
    mirror_commands: bool,
    mirror_file_changes: bool,
    stream_live: bool,
    approval_ttl_minutes: u64,
}

impl Default for BridgeConfig {
    fn default() -> Self {
        Self {
            mirror_agent_messages: true,
            mirror_user_messages: false, // user echoes are noise
            mirror_commands: false,      // command spam is noise
            mirror_file_changes: false,
            stream_live: true,
            approval_ttl_minutes: 30,
        }
    }
}

fn load(json: &str) -> BridgeConfig {
    let mut cfg = BridgeConfig::default();
    let v: Value = serde_json::from_str(json).unwrap();
    if let Some(b) = v["mirror"]["agentMessages"].as_bool() {
        cfg.mirror_agent_messages = b;
    }
    if let Some(b) = v["mirror"]["userMessages"].as_bool() {
        cfg.mirror_user_messages = b;
    }
    if let Some(b) = v["mirror"]["commands"].as_bool() {
        cfg.mirror_commands = b;
    }
    if let Some(b) = v["mirror"]["fileChanges"].as_bool() {
        cfg.mirror_file_changes = b;
    }
    if let Some(b) = v["stream"]["live"].as_bool() {
        cfg.stream_live = b;
    }
    if let Some(n) = v["approvals"]["ttlMinutes"].as_u64() {
        cfg.approval_ttl_minutes = n;
    }
    cfg
}

#[test]
fn empty_config_uses_defaults() {
    let cfg = load("{}");
    assert!(cfg.mirror_agent_messages);
    assert!(!cfg.mirror_user_messages);
    assert!(cfg.stream_live);
    assert_eq!(cfg.approval_ttl_minutes, 30);
}

#[test]
fn can_enable_command_mirroring() {
    let cfg = load(r#"{"mirror":{"commands":true}}"#);
    assert!(cfg.mirror_commands);
}

#[test]
fn can_disable_streaming() {
    let cfg = load(r#"{"stream":{"live":false}}"#);
    assert!(!cfg.stream_live);
}

#[test]
fn can_set_custom_ttl() {
    let cfg = load(r#"{"approvals":{"ttlMinutes":5}}"#);
    assert_eq!(cfg.approval_ttl_minutes, 5);
}

#[test]
fn unknown_keys_are_ignored() {
    let cfg = load(r#"{"futureFeature":{"whatever":true}}"#);
    assert!(cfg.mirror_agent_messages); // defaults intact
}

#[test]
fn malformed_json_is_rejected_not_panic() {
    let result: Result<Value, _> = serde_json::from_str("{ bad");
    assert!(result.is_err());
}

#[test]
fn agent_message_off_disables_mirror() {
    let cfg = load(r#"{"mirror":{"agentMessages":false}}"#);
    assert!(!cfg.mirror_agent_messages);
}
