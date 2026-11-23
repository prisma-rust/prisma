//! Config-driven JSON localization for `editor-app`, mirroring the old
//! editor's `locales/*.json` flattening and fallback rules.

use std::collections::HashMap;
use std::path::Path;

use prism_ui::i18n::{I18nTable, Locale};
use serde_json::Value;

const EN_US_JSON: &str = include_str!("../locales/en-US.json");
const ZH_CN_JSON: &str = include_str!("../locales/zh-CN.json");

/// Loaded localization state for the editor.
#[derive(Clone, Debug)]
pub struct EditorI18n {
    pub locale: Locale,
    table: I18nTable,
}

impl Default for EditorI18n {
    fn default() -> Self {
        let mut table = I18nTable::new();
        table.insert_dictionary(Locale::EnUs, load_locale_or_embedded("en-US", EN_US_JSON));
        table.insert_dictionary(Locale::ZhCn, load_locale_or_embedded("zh-CN", ZH_CN_JSON));
        Self {
            locale: Locale::ZhCn,
            table,
        }
    }
}

impl EditorI18n {
    pub fn t<'a>(&'a self, key: &'a str) -> &'a str {
        self.table.get(self.locale, key)
    }
}

fn load_locale_or_embedded(code: &str, embedded: &str) -> HashMap<String, String> {
    let disk_path = Path::new("locales").join(format!("{code}.json"));
    if let Ok(raw) = std::fs::read_to_string(&disk_path) {
        let parsed = flatten_json(&raw);
        if !parsed.is_empty() {
            return parsed;
        }
    }
    flatten_json(embedded)
}

fn flatten_json(raw: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let Ok(Value::Object(root)) = serde_json::from_str::<Value>(raw) else {
        return out;
    };
    flatten_object("", &root, &mut out);
    out
}

fn flatten_object(
    prefix: &str,
    obj: &serde_json::Map<String, Value>,
    out: &mut HashMap<String, String>,
) {
    for (k, v) in obj {
        let path = if prefix.is_empty() {
            k.clone()
        } else {
            format!("{prefix}.{k}")
        };
        match v {
            Value::String(s) => {
                out.insert(path, s.clone());
            }
            Value::Object(nested) => flatten_object(&path, nested, out),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_embedded_json_locales() {
        let mut i18n = EditorI18n::default();
        assert_eq!(i18n.t("menu.window"), "Window");
        i18n.locale = Locale::EnUs;
        assert_eq!(i18n.t("menu.window"), "Window");
        assert_eq!(i18n.t("missing.key"), "missing.key");
    }
}
