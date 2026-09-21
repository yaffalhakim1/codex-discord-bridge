use codex_discord_bridge::options::thread_title_from_message;

/// Regression: the auto-created Discord thread must be named from the first
/// user message, not always "Codex thread".
#[test]
fn thread_title_collapse_whitespace_runs() {
    assert_eq!(
        thread_title_from_message("  Fix \t the\n  login \r redirect   loop  "),
        "Fix the login redirect loop"
    );
}
