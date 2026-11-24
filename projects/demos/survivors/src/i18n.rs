//! Localization support (`zh-CN` default and `en-US`) for menus, HUD, and diagnostics.

/// Supported UI languages; Simplified Chinese (`ZhCn`) is the default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum Language {
    #[default]
    ZhCn,
    EnUs,
}

impl Language {
    pub fn toggle(self) -> Self {
        match self {
            Self::ZhCn => Self::EnUs,
            Self::EnUs => Self::ZhCn,
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::ZhCn => "简体中文",
            Self::EnUs => "English",
        }
    }

    pub fn is_zh(self) -> bool {
        matches!(self, Self::ZhCn)
    }
}

/// Select either Simplified Chinese or English text for the active language.
pub fn tr(lang: Language, zh: &'static str, en: &'static str) -> &'static str {
    match lang {
        Language::ZhCn => zh,
        Language::EnUs => en,
    }
}
