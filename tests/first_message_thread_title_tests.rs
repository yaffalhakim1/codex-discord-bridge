use codex_discord_bridge::options::thread_title_from_message;

#[test]
fn first_message_becomes_thread_name() {
    assert_eq!(
        thread_title_from_message("Fix the login redirect loop"),
        "Fix the login redirect loop"
    );
}

#[test]
fn long_first_message_is_truncated_on_word_boundary_with_ellipsis() {
    let message = "lorem ipsum dolor sit amet consectetur adipiscing elit ".repeat(5);
    let title = thread_title_from_message(&message);
    assert!(title.ends_with('…'));
    assert!(title.chars().count() <= 81);
    assert!(title.contains(' '));
    // cut must land on a word boundary of the original message
    let stem = title.trim_end_matches('…');
    assert!(message.starts_with(stem));
    assert!(message[stem.len()..].starts_with(' '));
}

#[test]
fn empty_first_message_uses_default_thread_name() {
    assert_eq!(thread_title_from_message(""), "Codex thread");
}
