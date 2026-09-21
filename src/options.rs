use serenity::model::application::{CommandDataOption, CommandDataOptionValue};

/// For a subcommand interaction, the user-supplied options live inside the
/// `SubCommand` variant of the first top-level option. This is a common
/// serenity pitfall: reading `cmd.data.options.first().value` as a String
/// always returns None for subcommands.
pub fn sub_options(options: &[CommandDataOption]) -> &[CommandDataOption] {
    match options.first() {
        Some(o) => match &o.value {
            CommandDataOptionValue::SubCommand(opts) => opts.as_slice(),
            _ => &[],
        },
        None => &[],
    }
}

pub fn string_at(options: &[CommandDataOption], index: usize) -> Option<String> {
    sub_options(options).get(index).and_then(|o| {
        if let CommandDataOptionValue::String(s) = &o.value {
            Some(s.clone())
        } else {
            None
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn opts(v: serde_json::Value) -> Vec<CommandDataOption> {
        serde_json::from_value(v).expect("valid CommandDataOption list")
    }

    #[test]
    fn reads_prompt_from_nested_subcommand() {
        let raw = json!([{"name":"new","type":1,"options":[{"name":"prompt","type":3,"value":"hello world"}]}]);
        let o = opts(raw);
        assert_eq!(string_at(&o, 0).as_deref(), Some("hello world"));
    }

    #[test]
    fn top_level_read_returns_none_for_subcommand() {
        let raw = json!([{"name":"new","type":1,"options":[]}]);
        let o = opts(raw);
        assert!(string_at(&o, 0).is_none());
    }

    #[test]
    fn empty_options_return_none() {
        let o = opts(json!([]));
        assert!(string_at(&o, 0).is_none());
    }

    #[test]
    fn second_option_reads_cwd() {
        let raw = json!([{"name":"new","type":1,"options":[
            {"name":"prompt","type":3,"value":"do the thing"},
            {"name":"cwd","type":3,"value":"C:/work"}
        ]}]);
        let o = opts(raw);
        assert_eq!(string_at(&o, 1).as_deref(), Some("C:/work"));
    }

    #[test]
    fn non_string_values_are_skipped_safely() {
        let raw = json!([{"name":"send","type":1,"options":[{"name":"limit","type":4,"value":5}]}]);
        let o = opts(raw);
        assert!(string_at(&o, 0).is_none());
    }
}
/// Derive a Discord thread title from the user's first message.
/// Whitespace collapsed, truncated to 80 chars on a word boundary with
/// an ellipsis when cut, "Codex thread" when empty.
pub fn thread_title_from_message(message: &str) -> String {
    let collapsed = message.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        return "Codex thread".to_string();
    }
    if collapsed.chars().count() <= 80 {
        return collapsed;
    }
    let truncated: String = collapsed.chars().take(77).collect();
    match truncated.rfind(' ') {
        Some(space) if space > 0 => truncated[..space].to_string() + "…",
        _ => truncated + "…",
    }
}

#[cfg(test)]
mod thread_title_tests {
    use super::thread_title_from_message;

    #[test]
    fn collapses_whitespace() {
        assert_eq!(
            thread_title_from_message("  fix\n\tthe   login  "),
            "fix the login"
        );
    }

    #[test]
    fn exactly_80_chars_is_kept_intact() {
        let m = "x".repeat(80);
        assert_eq!(thread_title_from_message(&m), m);
    }

    #[test]
    fn no_space_in_first_77_hard_cuts() {
        let m = "y".repeat(200);
        let t = thread_title_from_message(&m);
        assert_eq!(t.chars().count(), 78);
        assert!(t.ends_with('…'));
    }

    #[test]
    fn cut_lands_on_word_boundary() {
        let m = "lorem ipsum dolor sit amet consectetur adipiscing elit ".repeat(5);
        let t = thread_title_from_message(&m);
        assert!(t.ends_with('…'));
        assert!(t.chars().count() <= 81);
        let stem = t.trim_end_matches('…');
        assert!(m[stem.len()..].starts_with(' '));
    }
}
