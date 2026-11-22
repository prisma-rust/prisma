//! UI strings (Rust port of `editor/src/i18n.ts` + `locales/*`).
//!
//! Both dictionaries come directly from the web editor's locale source files (JSON
//! mechanically converted from TS objects, keeping key paths and copy consistent), so the
//! same key shows the same sentence on both sides. Missing keys fall back to English, then
//! to the key itself — matching the frontend `t()` semantics.

use serde_json::Value;
use std::collections::HashMap;

pub const ZH_CN: &str = include_str!("locales/zh-CN.json");
pub const EN_US: &str = include_str!("locales/en-US.json");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Locale {
    #[default]
    ZhCn,
    EnUs,
}

impl Locale {
    pub fn code(self) -> &'static str {
        match self {
            Locale::ZhCn => "zh-CN",
            Locale::EnUs => "en-US",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Locale::ZhCn => "Simplified Chinese",
            Locale::EnUs => "English",
        }
    }

    pub fn parse(code: &str) -> Locale {
        if code.starts_with("en") {
            Locale::EnUs
        } else {
            Locale::ZhCn
        }
    }

    /// Consistent with the frontend: use English if the system language starts with `en`.
    pub fn detect(system_language: Option<&str>) -> Locale {
        match system_language {
            Some(language) if language.starts_with("en") => Locale::EnUs,
            _ => Locale::ZhCn,
        }
    }
}

#[derive(Debug, Clone)]
pub struct I18n {
    locale: Locale,
    dictionaries: [HashMap<String, String>; 2],
}

impl Default for I18n {
    fn default() -> Self {
        Self::new(Locale::default())
    }
}

impl I18n {
    pub fn new(locale: Locale) -> Self {
        Self {
            locale,
            dictionaries: [flatten(ZH_CN), flatten(EN_US)],
        }
    }

    pub fn locale(&self) -> Locale {
        self.locale
    }

    pub fn set_locale(&mut self, locale: Locale) {
        self.locale = locale;
    }

    fn dictionary(&self, locale: Locale) -> &HashMap<String, String> {
        match locale {
            Locale::ZhCn => &self.dictionaries[0],
            Locale::EnUs => &self.dictionaries[1],
        }
    }

    pub fn get(&self, path: &str) -> Option<&str> {
        self.dictionary(self.locale)
            .get(path)
            .or_else(|| self.dictionary(Locale::EnUs).get(path))
            .map(String::as_str)
    }

    /// `t("inspector.selectedCount", &[("count", "3")])`.
    pub fn t(&self, path: &str, params: &[(&str, &str)]) -> String {
        let template = self
            .get(path)
            .map(str::to_string)
            .unwrap_or_else(|| path.to_string());
        let mut out = template;
        for (key, value) in params {
            out = out.replace(&format!("{{{key}}}"), value);
        }
        out
    }

    pub fn t1(&self, path: &str) -> String {
        self.t(path, &[])
    }

    /// All keys in the dictionary (for debugging / integrity tests).
    pub fn keys(&self) -> Vec<&str> {
        let mut keys: Vec<&str> = self
            .dictionary(self.locale)
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        keys
    }

    /// The key sets of the two languages must match; a missing key means the port is incomplete.
    pub fn missing_keys(&self) -> Vec<String> {
        let zh = self.dictionary(Locale::ZhCn);
        let en = self.dictionary(Locale::EnUs);
        let mut missing: Vec<String> = Vec::new();
        for key in zh.keys() {
            if !en.contains_key(key) {
                missing.push(key.clone());
            }
        }
        for key in en.keys() {
            if !zh.contains_key(key) {
                missing.push(key.clone());
            }
        }
        missing.sort_unstable();
        missing.dedup();
        missing
    }
}

fn flatten(json: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    if let Ok(value) = serde_json::from_str::<Value>(json) {
        walk(&value, "", &mut out);
    }
    out
}

fn walk(value: &Value, prefix: &str, out: &mut HashMap<String, String>) {
    match value {
        Value::Object(map) => {
            for (key, item) in map {
                let path = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{prefix}.{key}")
                };
                walk(item, &path, out);
            }
        }
        Value::String(text) => {
            out.insert(prefix.to_string(), text.clone());
        }
        other => {
            out.insert(prefix.to_string(), other.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_dictionaries_have_identical_keys() {
        let i18n = I18n::default();
        assert!(
            i18n.missing_keys().is_empty(),
            "locale keys inconsistent: {:?}",
            i18n.missing_keys()
        );
        assert!(
            i18n.keys().len() > 300,
            "dictionary entry count {}",
            i18n.keys().len()
        );
    }

    #[test]
    fn translates_and_switches_locale() {
        let mut i18n = I18n::new(Locale::ZhCn);
        assert_eq!(i18n.t1("common.save"), "保存");
        i18n.set_locale(Locale::EnUs);
        assert_eq!(i18n.t1("common.save"), "Save");
    }

    #[test]
    fn substitutes_parameters() {
        let i18n = I18n::new(Locale::EnUs);
        let text = i18n.t("inspector.selectedCount", &[("count", "3")]);
        assert!(text.contains('3'), "{text}");
        assert!(!text.contains("{count}"));
    }

    #[test]
    fn unknown_key_falls_back_to_path() {
        let i18n = I18n::default();
        assert_eq!(i18n.t1("nope.nothing"), "nope.nothing");
    }

    #[test]
    fn detects_locale_like_frontend() {
        assert_eq!(Locale::detect(Some("en-GB")), Locale::EnUs);
        assert_eq!(Locale::detect(Some("zh-CN")), Locale::ZhCn);
        assert_eq!(Locale::detect(None), Locale::ZhCn);
        assert_eq!(Locale::parse("en-US"), Locale::EnUs);
    }
}
