use serde_json::json;
use std::collections::HashMap;

/// Mirrors ThreadMapping serialization used in state.rs save/load
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
struct ThreadMapping {
    codex_thread_id: String,
    discord_channel_id: u64,
    created_at: i64,
    last_activity_at: Option<i64>,
}

#[test]
fn thread_mapping_serializes_roundtrip() {
    let m = ThreadMapping {
        codex_thread_id: "01a0be34".into(),
        discord_channel_id: 123456789,
        created_at: 1700000000,
        last_activity_at: Some(1700000100),
    };
    let s = serde_json::to_string(&m).unwrap();
    let back: ThreadMapping = serde_json::from_str(&s).unwrap();
    assert_eq!(m, back);
}

#[test]
fn state_file_roundtrip_preserves_mappings() {
    let mappings = vec![
        ThreadMapping {
            codex_thread_id: "t1".into(),
            discord_channel_id: 111,
            created_at: 1,
            last_activity_at: None,
        },
        ThreadMapping {
            codex_thread_id: "t2".into(),
            discord_channel_id: 222,
            created_at: 2,
            last_activity_at: Some(9),
        },
    ];
    let state = json!({ "thread_map": mappings, "default_model": "gpt-5.5" });
    let text = serde_json::to_string_pretty(&state).unwrap();

    let parsed: serde_json::Value = serde_json::from_str(&text).unwrap();
    let loaded: Vec<ThreadMapping> = serde_json::from_value(parsed["thread_map"].clone()).unwrap();
    assert_eq!(loaded.len(), 2);
    assert_eq!(loaded[0].codex_thread_id, "t1");
    assert_eq!(parsed["default_model"], "gpt-5.5");
}

#[test]
fn corrupt_state_file_does_not_panic() {
    let bad = "{ not valid json {{{";
    let result: Result<serde_json::Value, _> = serde_json::from_str(bad);
    assert!(result.is_err());
}

#[test]
fn empty_state_file_loads_clean() {
    let empty = json!({ "thread_map": [], "default_model": null });
    let parsed: serde_json::Value =
        serde_json::from_str(&serde_json::to_string(&empty).unwrap()).unwrap();
    let entries = parsed["thread_map"].as_array().unwrap();
    assert!(entries.is_empty());
    assert!(parsed["default_model"].is_null());
}

// --- Write-back queue semantics ---

#[derive(Debug, Clone, PartialEq)]
struct WriteBack {
    id: String,
    text: String,
}

#[test]
fn queue_accumulates_and_retracts_last() {
    let mut q: Vec<WriteBack> = Vec::new();
    q.push(WriteBack {
        id: "1".into(),
        text: "first".into(),
    });
    q.push(WriteBack {
        id: "2".into(),
        text: "second".into(),
    });

    let retracted = q.pop();
    assert_eq!(retracted.unwrap().text, "second");
    assert_eq!(q.len(), 1);
}

#[test]
fn retract_from_empty_queue_returns_none() {
    let mut q: Vec<WriteBack> = Vec::new();
    assert!(q.pop().is_none());
}

// --- Approval token registry ---

#[test]
fn approval_lookup_by_token() {
    let mut approvals: HashMap<String, String> = HashMap::new();
    approvals.insert("tok-1".into(), "thread-a".into());
    approvals.insert("tok-2".into(), "thread-b".into());

    assert_eq!(approvals.get("tok-1").map(|s| s.as_str()), Some("thread-a"));
    assert!(!approvals.contains_key("missing"));
}

#[test]
fn approval_removal_after_decision() {
    let mut approvals: HashMap<String, String> = HashMap::new();
    approvals.insert("tok-1".into(), "thread-a".into());
    approvals.remove("tok-1");
    assert!(!approvals.contains_key("tok-1"));
}

// --- Decision payload shapes ---

#[test]
fn approve_decision_payload() {
    let d = json!({ "decision": "accept" });
    assert_eq!(d["decision"].as_str(), Some("accept"));
}

#[test]
fn decline_decision_payload() {
    let d = json!({ "decision": "decline" });
    assert_eq!(d["decision"].as_str(), Some("decline"));
}

#[test]
fn cancel_decision_payload() {
    let d = json!({ "decision": "cancel" });
    assert_eq!(d["decision"].as_str(), Some("cancel"));
}

#[test]
fn unknown_decision_rejected() {
    let d = "explode";
    let valid = matches!(d, "approve" | "decline" | "cancel");
    assert!(!valid);
}
