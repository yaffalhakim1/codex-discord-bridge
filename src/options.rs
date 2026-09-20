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