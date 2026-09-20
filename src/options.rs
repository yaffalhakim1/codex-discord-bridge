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
/// Pick a Discord thread name from a Codex thread's metadata.
/// Empty strings are treated as absent. Preview is truncated to 40 chars with an ellipsis.
pub fn thread_display_name(name: Option<&str>, preview: Option<&str>) -> String {
    let from_name = name.map(str::trim).filter(|s| !s.is_empty());
    let from_preview = preview
        .map(|p| {
            let t: String = p.chars().take(40).collect();
            if p.chars().count() > 40 { format!("{t}…") } else { t }
        })
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    from_name
        .map(|s| s.to_string())
        .or(from_preview)
        .unwrap_or_else(|| "Codex thread".to_string())
}

#[cfg(test)]
mod thread_name_tests {
    use super::thread_display_name;

    #[test]
    fn prefers_name_when_present() {
        assert_eq!(thread_display_name(Some("Fix login"), Some("preview")), "Fix login");
    }

    #[test]
    fn falls_back_to_preview_when_name_missing() {
        assert_eq!(thread_display_name(None, Some("build the thing")), "build the thing");
    }

    #[test]
    fn empty_string_name_is_treated_as_missing() {
        // This is the bug that shipped: Codex sends name=Some("") for new threads
        assert_eq!(thread_display_name(Some(""), Some("hello")), "hello");
    }

    #[test]
    fn empty_preview_falls_back_to_default() {
        // And this: preview is Some("") too
        assert_eq!(thread_display_name(Some(""), Some("")), "Codex thread");
        assert_eq!(thread_display_name(None, Some("")), "Codex thread");
    }

    #[test]
    fn whitespace_only_is_empty() {
        assert_eq!(thread_display_name(Some("   "), Some("  ")), "Codex thread");
    }

    #[test]
    fn long_preview_is_truncated_with_ellipsis() {
        let long = "a".repeat(60);
        let got = thread_display_name(None, Some(&long));
        assert!(got.ends_with('…'));
        assert_eq!(got.chars().count(), 41);
    }

    #[test]
    fn everything_missing_uses_default() {
        assert_eq!(thread_display_name(None, None), "Codex thread");
    }
}