use prism_ini::{format_value, parse_value, Ini};
use proptest::prelude::*;

fn section_name_strategy() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9_.-]{1,24}"
}

fn key_name_strategy() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9_.-]{1,24}"
}

fn value_strategy() -> impl Strategy<Value = String> {
    prop::string::string_regex("[a-zA-Z0-9_ .;:#/\\\\\"\\n-]{0,40}").unwrap()
}

proptest! {
    #[test]
    fn value_format_parse_roundtrip(raw in value_strategy()) {
        let formatted = format_value(&raw);
        let parsed = parse_value(&formatted);
        prop_assert_eq!(parsed, raw);
    }

    #[test]
    fn ini_render_parse_roundtrip(
        sections in prop::collection::vec(
            (
                section_name_strategy(),
                prop::collection::btree_map(key_name_strategy(), value_strategy(), 0..8),
            ),
            0..6,
        )
    ) {
        let mut ini = Ini::default();
        for (section, entries) in sections {
            for (key, value) in entries {
                ini.set(&section, &key, &value);
            }
        }
        let rendered = ini.render();
        let reparsed = Ini::parse(&rendered);
        prop_assert_eq!(reparsed, ini);
    }

    #[test]
    fn ini_parse_never_panics_on_arbitrary_text(text in ".*") {
        let parsed = Ini::parse(&text);
        let _ = parsed.render();
    }
}
