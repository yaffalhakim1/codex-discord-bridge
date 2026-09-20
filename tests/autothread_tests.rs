use serde_json::json;

/// TDD: when a Codex thread starts (from any client) and it's not mapped,
/// the bridge creates a Discord thread under a configured category and maps it.

#[derive(Debug, PartialEq)]
enum Decision {
    /// Create a new Discord thread under this category
    CreateThread { category_id: u64, name: String },
    /// Thread already mapped — do nothing
    AlreadyMapped,
    /// Auto-creation disabled in config
    Disabled,
}

fn decide(
    enabled: bool,
    category_id: Option<u64>,
    already_mapped: bool,
    thread_name: Option<&str>,
    thread_preview: Option<&str>,
) -> Decision {
    if !enabled {
        return Decision::Disabled;
    }
    if already_mapped {
        return Decision::AlreadyMapped;
    }
    let category = match category_id {
        Some(c) => c,
        None => return Decision::Disabled, // no category configured = opt-out
    };
    // Name: explicit thread name, else first 40 chars of preview, else "Codex thread"
    let name = thread_name
        .map(|s| s.to_string())
        .or_else(|| thread_preview.map(|p| {
            let truncated: String = p.chars().take(40).collect();
            if p.chars().count() > 40 { format!("{truncated}…") } else { truncated }
        }))
        .unwrap_or_else(|| "Codex thread".to_string());
    Decision::CreateThread { category_id: category, name }
}

#[test]
fn disabled_config_never_creates() {
    let d = decide(false, Some(123), false, Some("x"), None);
    assert_eq!(d, Decision::Disabled);
}

#[test]
fn no_category_never_creates() {
    let d = decide(true, None, false, Some("x"), None);
    assert_eq!(d, Decision::Disabled);
}

#[test]
fn mapped_thread_is_skipped() {
    let d = decide(true, Some(123), true, Some("x"), None);
    assert_eq!(d, Decision::AlreadyMapped);
}

#[test]
fn uses_thread_name_when_present() {
    let d = decide(true, Some(123), false, Some("Fix login bug"), None);
    assert_eq!(d, Decision::CreateThread { category_id: 123, name: "Fix login bug".into() });
}

#[test]
fn falls_back_to_preview_truncated() {
    let preview = "This is a very long preview message that goes on and on and should be cut";
    let d = decide(true, Some(123), false, None, Some(preview));
    match d {
        Decision::CreateThread { name, .. } => {
            assert!(name.ends_with('…'));
            assert!(name.chars().count() <= 41);
        }
        _ => panic!("expected CreateThread"),
    }
}

#[test]
fn short_preview_is_used_whole() {
    let d = decide(true, Some(123), false, None, Some("short"));
    assert_eq!(d, Decision::CreateThread { category_id: 123, name: "short".into() });
}

#[test]
fn no_name_no_preview_uses_fallback() {
    let d = decide(true, Some(123), false, None, None);
    assert_eq!(d, Decision::CreateThread { category_id: 123, name: "Codex thread".into() });
}

#[test]
fn thread_start_payload_has_thread_object() {
    let params = json!({ "thread": { "id": "01a0be34", "name": null, "preview": "hello world", "cwd": "/tmp" } });
    assert!(params["thread"]["id"].is_string());
    assert!(params["thread"]["name"].is_null());
    assert_eq!(params["thread"]["preview"], "hello world");
}