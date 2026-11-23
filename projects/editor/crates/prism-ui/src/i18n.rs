//! Generic locale & dictionary table for `prism-ui`.
//!
//! Applications supply their own JSON/config dictionaries into [`I18nTable`];
//! `prism-ui` itself stores no hardcoded application strings.

use std::collections::HashMap;

/// Supported UI locales.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Locale {
    EnUs,
    #[default]
    ZhCn,
}

impl Locale {
    pub const ALL: [Self; 2] = [Self::EnUs, Self::ZhCn];

    pub fn id(self) -> &'static str {
        match self {
            Self::EnUs => "en-US",
            Self::ZhCn => "zh-CN",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::EnUs => "English",
            Self::ZhCn => "Simplified Chinese",
        }
    }

    pub fn parse(code: &str) -> Self {
        if code.starts_with("zh") {
            Self::ZhCn
        } else {
            Self::EnUs
        }
    }
}

/// Config-driven localization table: maps `(Locale, key) -> String`,
/// falling back to [`Locale::EnUs`] and finally to `key` itself.
#[derive(Clone, Debug, Default)]
pub struct I18nTable {
    dictionaries: HashMap<Locale, HashMap<String, String>>,
}

impl I18nTable {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register or replace all key-value pairs for `locale`.
    pub fn insert_dictionary(&mut self, locale: Locale, dict: HashMap<String, String>) {
        self.dictionaries.insert(locale, dict);
    }

    /// Look up `key` in `locale`, falling back to `EnUs`, then to `key`.
    pub fn get<'a>(&'a self, locale: Locale, key: &'a str) -> &'a str {
        self.dictionaries
            .get(&locale)
            .and_then(|d| d.get(key))
            .or_else(|| {
                self.dictionaries
                    .get(&Locale::EnUs)
                    .and_then(|d| d.get(key))
            })
            .map(String::as_str)
            .unwrap_or(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_resolves_locale_and_falls_back_to_en_then_key() {
        let mut table = I18nTable::new();
        table.insert_dictionary(
            Locale::EnUs,
            HashMap::from([("menu.window".into(), "Window".into())]),
        );
        table.insert_dictionary(
            Locale::ZhCn,
            HashMap::from([("menu.window".into(), "Window".into())]),
        );

        assert_eq!(table.get(Locale::EnUs, "menu.window"), "Window");
        assert_eq!(table.get(Locale::ZhCn, "menu.window"), "Window");
        assert_eq!(table.get(Locale::ZhCn, "missing.key"), "missing.key");
    }
}
