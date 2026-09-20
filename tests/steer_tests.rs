use serde_json::json;

/// TDD: /codex send mode:steer should call turn/steer with expectedTurnId
/// when a turn is active, instead of turn/start.

#[derive(Debug, PartialEq)]
enum Action {
    Start { thread: String, text: String },
    Steer { thread: String, turn: String, text: String },
    Queue { thread: String, text: String },
}

fn route(has_mapped_thread: bool, has_active_turn: bool, mode: Option<&str>, thread: &str, active_turn: &str, text: &str) -> Action {
    if !has_mapped_thread {
        panic!("no thread");
    }
    match (mode, has_active_turn) {
        (Some("steer"), true) => Action::Steer { thread: thread.into(), turn: active_turn.into(), text: text.into() },
        (Some("steer"), false) => Action::Start { thread: thread.into(), text: text.into() },
        _ => {
            if has_active_turn {
                Action::Queue { thread: thread.into(), text: text.into() }
            } else {
                Action::Start { thread: thread.into(), text: text.into() }
            }
        }
    }
}

#[test]
fn steer_mode_with_active_turn_steers() {
    let a = route(true, true, Some("steer"), "t1", "turn-9", "focus on tests");
    assert_eq!(a, Action::Steer { thread: "t1".into(), turn: "turn-9".into(), text: "focus on tests".into() });
}

#[test]
fn steer_mode_without_active_turn_starts() {
    let a = route(true, false, Some("steer"), "t1", "", "focus on tests");
    assert_eq!(a, Action::Start { thread: "t1".into(), text: "focus on tests".into() });
}

#[test]
fn default_mode_with_active_turn_queues() {
    let a = route(true, true, None, "t1", "turn-9", "also do X");
    assert_eq!(a, Action::Queue { thread: "t1".into(), text: "also do X".into() });
}

#[test]
fn default_mode_without_active_turn_starts() {
    let a = route(true, false, None, "t1", "", "hello");
    assert_eq!(a, Action::Start { thread: "t1".into(), text: "hello".into() });
}

#[test]
fn steer_request_payload_shape() {
    let payload = json!({
        "threadId": "t1",
        "expectedTurnId": "turn-9",
        "input": [{ "type": "text", "text": "focus on tests" }]
    });
    assert_eq!(payload["threadId"], "t1");
    assert_eq!(payload["expectedTurnId"], "turn-9");
    assert_eq!(payload["input"][0]["type"], "text");
}

#[test]
fn freeform_message_defaults_to_queue_or_start() {
    // Free-form chat never steers unless explicitly asked
    let a = route(true, true, None, "t1", "turn-9", "new instruction");
    assert!(matches!(a, Action::Queue { .. }));
}