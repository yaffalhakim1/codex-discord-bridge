use serde_json::json;

#[derive(Debug, PartialEq)]
enum Event {
    Response { id: i64 },
    ServerRequest { method: String },
    Notification { method: String },
}

fn classify(msg: &serde_json::Value) -> Event {
    let has_id = msg.get("id").is_some();
    let method = msg.get("method").and_then(|m| m.as_str());
    match (has_id, method) {
        (true, Some(_)) => Event::ServerRequest { method: method.unwrap().to_string() },
        (true, None) => Event::Response { id: msg["id"].as_i64().unwrap() },
        (false, Some(m)) => Event::Notification { method: m.to_string() },
        (false, None) => panic!("malformed"),
    }
}

#[test]
fn response_message_classifies_as_response() {
    let msg = json!({"jsonrpc":"2.0","id":42,"result":{"ok":true}});
    assert_eq!(classify(&msg), Event::Response { id: 42 });
}

#[test]
fn server_request_classifies_as_server_request() {
    let msg = json!({"jsonrpc":"2.0","id":"req-1","method":"item/commandExecution/requestApproval","params":{}});
    assert_eq!(classify(&msg), Event::ServerRequest { method: "item/commandExecution/requestApproval".into() });
}

#[test]
fn notification_classifies_as_notification() {
    let msg = json!({"jsonrpc":"2.0","method":"item/completed","params":{"threadId":"t1"}});
    assert_eq!(classify(&msg), Event::Notification { method: "item/completed".into() });
}

#[test]
fn extracts_approval_command_from_string() {
    let params = json!({"threadId":"t","turnId":"u","itemId":"i","command":"echo hi"});
    let cmd = params["command"].as_str().map(String::from).or_else(|| {
        params["command"].as_array().map(|a| a.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>().join(" "))
    });
    assert_eq!(cmd.as_deref(), Some("echo hi"));
}

#[test]
fn extracts_approval_command_from_array() {
    let params = json!({"threadId":"t","turnId":"u","itemId":"i","command":["pwsh","-Command","Get-Location"]});
    let cmd = params["command"].as_str().map(String::from).or_else(|| {
        params["command"].as_array().map(|a| a.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>().join(" "))
    });
    assert_eq!(cmd.as_deref(), Some("pwsh -Command Get-Location"));
}

#[test]
fn falls_back_to_conversation_id_when_no_thread_id() {
    let params = json!({"conversationId":"conv-1","turnId":"u","callId":"c1"});
    let thread_id = params["threadId"].as_str().or_else(|| params["conversationId"].as_str()).unwrap_or("");
    assert_eq!(thread_id, "conv-1");
}

#[test]
fn falls_back_to_call_id_when_no_item_id() {
    let params = json!({"callId":"call-9"});
    let item_id = params["itemId"].as_str().or_else(|| params["callId"].as_str()).unwrap_or("");
    assert_eq!(item_id, "call-9");
}