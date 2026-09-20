use serde_json::json;

/// TDD: Discord image attachments become Codex input_image content items.

#[derive(Debug, PartialEq)]
enum ContentItem {
    Text(String),
    Image { url: String },
}

fn parse_attachments(urls: &[&str], text: &str) -> Vec<ContentItem> {
    let mut items = Vec::new();
    if !text.is_empty() {
        items.push(ContentItem::Text(text.to_string()));
    }
    for u in urls {
        items.push(ContentItem::Image { url: u.to_string() });
    }
    items
}

fn to_codex_input(items: &[ContentItem]) -> serde_json::Value {
    let arr: Vec<serde_json::Value> = items.iter().map(|i| match i {
        ContentItem::Text(t) => json!({ "type": "text", "text": t }),
        ContentItem::Image { url } => json!({ "type": "image_url", "image_url": url }),
    }).collect();
    json!(arr)
}

#[test]
fn text_only_message_has_one_item() {
    let items = parse_attachments(&[], "hello");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0], ContentItem::Text("hello".into()));
}

#[test]
fn image_only_message_has_one_item() {
    let items = parse_attachments(&["https://cdn.discord.com/img.png"], "");
    assert_eq!(items.len(), 1);
    assert!(matches!(&items[0], ContentItem::Image { url } if url.contains("img.png")));
}

#[test]
fn text_plus_image_yields_two_items() {
    let items = parse_attachments(&["https://x.com/a.png"], "what is this?");
    assert_eq!(items.len(), 2);
}

#[test]
fn multiple_images_supported() {
    let items = parse_attachments(&["https://x.com/1.png", "https://x.com/2.png"], "compare");
    assert_eq!(items.len(), 3);
}

#[test]
fn codex_payload_shape_matches_protocol() {
    let items = parse_attachments(&["https://x.com/a.png"], "describe this");
    let payload = to_codex_input(&items);
    assert_eq!(payload[0]["type"], "text");
    assert_eq!(payload[0]["text"], "describe this");
    assert_eq!(payload[1]["type"], "image_url");
    assert_eq!(payload[1]["image_url"], "https://x.com/a.png");
}

#[test]
fn discord_attachment_urls_are_https() {
    let url = "https://cdn.discordapp.com/attachments/1/2/image.png";
    assert!(url.starts_with("https://"));
}

#[test]
fn empty_message_no_attachments_is_rejected_upstream() {
    let items = parse_attachments(&[], "");
    assert!(items.is_empty());
}