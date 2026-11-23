//! Editor theme (Rust port of `editor/src/theme.ts`).
//!
//! Theme only describes **colors**: each token corresponds to a named color, UI always reads via
//! `Theme::color`, colors are no longer hardcoded. Presets match the Web editor exactly (same preset id looks the same on both sides).

use std::collections::HashMap;

/// sRGB color (8-bit components).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Rgba {
    pub const fn new(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    /// `#rrggbb` (optionally `#rrggbbaa`). Invalid input falls back to opaque black.
    pub fn from_hex(hex: &str) -> Self {
        let hex = hex.trim().trim_start_matches('#');
        let parse = |range: std::ops::Range<usize>| {
            u8::from_str_radix(hex.get(range).unwrap_or("00"), 16).unwrap_or(0)
        };
        match hex.len() {
            6 => Self::new(parse(0..2), parse(2..4), parse(4..6), 255),
            8 => Self::new(parse(0..2), parse(2..4), parse(4..6), parse(6..8)),
            _ => Self::new(0, 0, 0, 255),
        }
    }

    pub fn opacify(self, alpha: f32) -> Self {
        Self {
            a: (alpha.clamp(0.0, 1.0) * 255.0).round() as u8,
            ..self
        }
    }

    /// Linear interpolation (for hover / selected-state derived colors).
    pub fn mix(self, other: Self, t: f32) -> Self {
        let t = t.clamp(0.0, 1.0);
        let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round() as u8;
        Self::new(
            mix(self.r, other.r),
            mix(self.g, other.g),
            mix(self.b, other.b),
            mix(self.a, other.a),
        )
    }

    pub fn to_hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }
}

/// All themeable color tokens (order matches `THEME_TOKEN_GROUPS`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(usize)]
pub enum ThemeToken {
    Bg,
    BgSunken,
    Panel,
    PanelAlt,
    PanelRaised,
    Chrome,
    Border,
    BorderStrong,
    BorderSubtle,
    Text,
    TextStrong,
    TextSoft,
    Muted,
    MutedDim,
    RowHover,
    MenuHover,
    MenuActive,
    Selection,
    Accent,
    AccentSolid,
    OnAccent,
    AccentInk,
    AccentAlt,
    Success,
    SuccessSoft,
    SuccessBg,
    SuccessBorder,
    Danger,
    DangerSoft,
    DangerBg,
    DangerBgSubtle,
    DangerSolid,
    DangerBorder,
    OnDanger,
    Warning,
    WarningStrong,
    WarningBorder,
    Panic,
}

impl ThemeToken {
    pub const COUNT: usize = 38;

    pub const ALL: [ThemeToken; Self::COUNT] = [
        ThemeToken::Bg,
        ThemeToken::BgSunken,
        ThemeToken::Panel,
        ThemeToken::PanelAlt,
        ThemeToken::PanelRaised,
        ThemeToken::Chrome,
        ThemeToken::Border,
        ThemeToken::BorderStrong,
        ThemeToken::BorderSubtle,
        ThemeToken::Text,
        ThemeToken::TextStrong,
        ThemeToken::TextSoft,
        ThemeToken::Muted,
        ThemeToken::MutedDim,
        ThemeToken::RowHover,
        ThemeToken::MenuHover,
        ThemeToken::MenuActive,
        ThemeToken::Selection,
        ThemeToken::Accent,
        ThemeToken::AccentSolid,
        ThemeToken::OnAccent,
        ThemeToken::AccentInk,
        ThemeToken::AccentAlt,
        ThemeToken::Success,
        ThemeToken::SuccessSoft,
        ThemeToken::SuccessBg,
        ThemeToken::SuccessBorder,
        ThemeToken::Danger,
        ThemeToken::DangerSoft,
        ThemeToken::DangerBg,
        ThemeToken::DangerBgSubtle,
        ThemeToken::DangerSolid,
        ThemeToken::DangerBorder,
        ThemeToken::OnDanger,
        ThemeToken::Warning,
        ThemeToken::WarningStrong,
        ThemeToken::WarningBorder,
        ThemeToken::Panic,
    ];

    pub fn index(self) -> usize {
        self as usize
    }

    pub fn key(self) -> &'static str {
        match self {
            ThemeToken::Bg => "bg",
            ThemeToken::BgSunken => "bgSunken",
            ThemeToken::Panel => "panel",
            ThemeToken::PanelAlt => "panelAlt",
            ThemeToken::PanelRaised => "panelRaised",
            ThemeToken::Chrome => "chrome",
            ThemeToken::Border => "border",
            ThemeToken::BorderStrong => "borderStrong",
            ThemeToken::BorderSubtle => "borderSubtle",
            ThemeToken::Text => "text",
            ThemeToken::TextStrong => "textStrong",
            ThemeToken::TextSoft => "textSoft",
            ThemeToken::Muted => "muted",
            ThemeToken::MutedDim => "mutedDim",
            ThemeToken::RowHover => "rowHover",
            ThemeToken::MenuHover => "menuHover",
            ThemeToken::MenuActive => "menuActive",
            ThemeToken::Selection => "selection",
            ThemeToken::Accent => "accent",
            ThemeToken::AccentSolid => "accentSolid",
            ThemeToken::OnAccent => "onAccent",
            ThemeToken::AccentInk => "accentInk",
            ThemeToken::AccentAlt => "accentAlt",
            ThemeToken::Success => "success",
            ThemeToken::SuccessSoft => "successSoft",
            ThemeToken::SuccessBg => "successBg",
            ThemeToken::SuccessBorder => "successBorder",
            ThemeToken::Danger => "danger",
            ThemeToken::DangerSoft => "dangerSoft",
            ThemeToken::DangerBg => "dangerBg",
            ThemeToken::DangerBgSubtle => "dangerBgSubtle",
            ThemeToken::DangerSolid => "dangerSolid",
            ThemeToken::DangerBorder => "dangerBorder",
            ThemeToken::OnDanger => "onDanger",
            ThemeToken::Warning => "warning",
            ThemeToken::WarningStrong => "warningStrong",
            ThemeToken::WarningBorder => "warningBorder",
            ThemeToken::Panic => "panic",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ThemeToken::Bg => "app background",
            ThemeToken::BgSunken => "sunken area (log / input)",
            ThemeToken::Panel => "panel",
            ThemeToken::PanelAlt => "secondary panel (tab header / tab)",
            ThemeToken::PanelRaised => "float layer (menu / popup)",
            ThemeToken::Chrome => "menu bar / status bar",
            ThemeToken::Border => "border",
            ThemeToken::BorderStrong => "strong border / divider",
            ThemeToken::BorderSubtle => "weak border",
            ThemeToken::Text => "text",
            ThemeToken::TextStrong => "highlighted text / title",
            ThemeToken::TextSoft => "secondary text",
            ThemeToken::Muted => "muted text",
            ThemeToken::MutedDim => "weakest text (tooltip)",
            ThemeToken::RowHover => "row hover background",
            ThemeToken::MenuHover => "menu hover background",
            ThemeToken::MenuActive => "menu open / pressed background",
            ThemeToken::Selection => "selected row background",
            ThemeToken::Accent => "accent color",
            ThemeToken::AccentSolid => "solid accent block",
            ThemeToken::OnAccent => "text on solid accent block",
            ThemeToken::AccentInk => "gradient button text",
            ThemeToken::AccentAlt => "secondary accent color",
            ThemeToken::Success => "success / running indication",
            ThemeToken::SuccessSoft => "success text",
            ThemeToken::SuccessBg => "success background (Play)",
            ThemeToken::SuccessBorder => "success border",
            ThemeToken::Danger => "danger / error",
            ThemeToken::DangerSoft => "danger text",
            ThemeToken::DangerBg => "danger background (Stop)",
            ThemeToken::DangerBgSubtle => "danger hover background",
            ThemeToken::DangerSolid => "danger solid block",
            ThemeToken::DangerBorder => "danger border",
            ThemeToken::OnDanger => "text on danger block",
            ThemeToken::Warning => "warning",
            ThemeToken::WarningStrong => "warning strong",
            ThemeToken::WarningBorder => "warning border",
            ThemeToken::Panic => "panic log",
        }
    }
}

/// Theme panel groups (display order).
pub struct TokenGroup {
    pub id: &'static str,
    pub label: &'static str,
    pub tokens: &'static [ThemeToken],
}

pub const TOKEN_GROUPS: &[TokenGroup] = &[
    TokenGroup {
        id: "surface",
        label: "backgrounds and surfaces",
        tokens: &[
            ThemeToken::Bg,
            ThemeToken::BgSunken,
            ThemeToken::Panel,
            ThemeToken::PanelAlt,
            ThemeToken::PanelRaised,
            ThemeToken::Chrome,
        ],
    },
    TokenGroup {
        id: "border",
        label: "borders and dividers",
        tokens: &[
            ThemeToken::Border,
            ThemeToken::BorderStrong,
            ThemeToken::BorderSubtle,
        ],
    },
    TokenGroup {
        id: "text",
        label: "text",
        tokens: &[
            ThemeToken::Text,
            ThemeToken::TextStrong,
            ThemeToken::TextSoft,
            ThemeToken::Muted,
            ThemeToken::MutedDim,
        ],
    },
    TokenGroup {
        id: "interaction",
        label: "interaction states",
        tokens: &[
            ThemeToken::RowHover,
            ThemeToken::MenuHover,
            ThemeToken::MenuActive,
            ThemeToken::Selection,
        ],
    },
    TokenGroup {
        id: "accent",
        label: "accent colors",
        tokens: &[
            ThemeToken::Accent,
            ThemeToken::AccentSolid,
            ThemeToken::OnAccent,
            ThemeToken::AccentInk,
            ThemeToken::AccentAlt,
        ],
    },
    TokenGroup {
        id: "status",
        label: "status colors",
        tokens: &[
            ThemeToken::Success,
            ThemeToken::SuccessSoft,
            ThemeToken::SuccessBg,
            ThemeToken::SuccessBorder,
            ThemeToken::Danger,
            ThemeToken::DangerSoft,
            ThemeToken::DangerBg,
            ThemeToken::DangerBgSubtle,
            ThemeToken::DangerSolid,
            ThemeToken::DangerBorder,
            ThemeToken::OnDanger,
            ThemeToken::Warning,
            ThemeToken::WarningStrong,
            ThemeToken::WarningBorder,
            ThemeToken::Panic,
        ],
    },
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeMode {
    Dark,
    Light,
}

impl ThemeMode {
    pub fn is_dark(self) -> bool {
        matches!(self, ThemeMode::Dark)
    }
}

/// A complete theme = one color per token.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ThemeColors {
    values: [Rgba; ThemeToken::COUNT],
}

impl ThemeColors {
    pub fn color(&self, token: ThemeToken) -> Rgba {
        self.values[token.index()]
    }

    pub fn set(&mut self, token: ThemeToken, color: Rgba) {
        self.values[token.index()] = color;
    }

    pub fn hexes(&self) -> Vec<String> {
        self.values.iter().map(|color| color.to_hex()).collect()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ThemePreset {
    pub id: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub mode: ThemeMode,
    pub colors: ThemeColors,
}

pub const THEME_PRESETS: &[ThemePreset] = &[
    ThemePreset {
        id: "studio",
        label: "industrial dark",
        description: "default UE5 / Unity 6 dark studio palette",
        mode: ThemeMode::Dark,
        colors: ThemeColors {
            values: [
                Rgba::new(20, 22, 27, 255),    
                Rgba::new(17, 19, 23, 255),    
                Rgba::new(26, 29, 35, 255),    
                Rgba::new(34, 38, 46, 255),    
                Rgba::new(40, 45, 55, 255),    
                Rgba::new(22, 25, 31, 255),    
                Rgba::new(43, 48, 59, 255),    
                Rgba::new(58, 65, 80, 255),    
                Rgba::new(33, 37, 45, 255),    
                Rgba::new(208, 213, 223, 255), 
                Rgba::new(243, 245, 248, 255), 
                Rgba::new(176, 183, 197, 255), 
                Rgba::new(125, 133, 148, 255), 
                Rgba::new(89, 97, 112, 255),   
                Rgba::new(37, 42, 52, 255),    
                Rgba::new(44, 50, 62, 255),    
                Rgba::new(38, 77, 126, 255),   
                Rgba::new(38, 75, 122, 255),   
                Rgba::new(59, 130, 246, 255),  
                Rgba::new(37, 99, 235, 255),   
                Rgba::new(255, 255, 255, 255), 
                Rgba::new(11, 19, 32, 255),    
                Rgba::new(96, 165, 250, 255),  
                Rgba::new(52, 211, 153, 255),  
                Rgba::new(110, 231, 183, 255), 
                Rgba::new(30, 77, 59, 255),    
                Rgba::new(42, 107, 82, 255),   
                Rgba::new(248, 113, 113, 255), 
                Rgba::new(252, 165, 165, 255), 
                Rgba::new(91, 35, 39, 255),    
                Rgba::new(48, 26, 29, 255),    
                Rgba::new(185, 28, 28, 255),   
                Rgba::new(153, 27, 27, 255),   
                Rgba::new(255, 255, 255, 255), 
                Rgba::new(251, 191, 36, 255),  
                Rgba::new(245, 158, 11, 255),  
                Rgba::new(120, 83, 26, 255),   
                Rgba::new(244, 114, 182, 255), 
            ],
        },
    },
    ThemePreset {
        id: "midnight",
        label: "deep space blue",
        description: "default · cold blue accent",
        mode: ThemeMode::Dark,
        colors: ThemeColors {
            values: [
                Rgba::new(11, 14, 20, 255),    
                Rgba::new(13, 17, 25, 255),    
                Rgba::new(18, 23, 34, 255),    
                Rgba::new(23, 29, 43, 255),    
                Rgba::new(29, 32, 39, 255),    
                Rgba::new(24, 26, 31, 255),    
                Rgba::new(35, 43, 61, 255),    
                Rgba::new(51, 56, 67, 255),    
                Rgba::new(42, 45, 53, 255),    
                Rgba::new(230, 235, 245, 255), 
                Rgba::new(255, 255, 255, 255), 
                Rgba::new(207, 216, 234, 255), 
                Rgba::new(139, 150, 173, 255), 
                Rgba::new(127, 138, 158, 255), 
                Rgba::new(23, 29, 43, 255),    
                Rgba::new(36, 40, 50, 255),    
                Rgba::new(42, 50, 66, 255),    
                Rgba::new(27, 35, 51, 255),    
                Rgba::new(110, 168, 255, 255), 
                Rgba::new(45, 104, 196, 255),  
                Rgba::new(255, 255, 255, 255), 
                Rgba::new(6, 18, 31, 255),     
                Rgba::new(95, 240, 216, 255),  
                Rgba::new(79, 191, 135, 255),  
                Rgba::new(143, 224, 176, 255), 
                Rgba::new(37, 95, 59, 255),    
                Rgba::new(47, 122, 75, 255),   
                Rgba::new(255, 107, 107, 255), 
                Rgba::new(255, 180, 180, 255), 
                Rgba::new(106, 42, 47, 255),   
                Rgba::new(42, 27, 31, 255),    
                Rgba::new(141, 48, 56, 255),   
                Rgba::new(147, 64, 73, 255),   
                Rgba::new(255, 228, 230, 255), 
                Rgba::new(255, 207, 107, 255), 
                Rgba::new(224, 168, 76, 255),  
                Rgba::new(122, 101, 47, 255),  
                Rgba::new(255, 122, 217, 255), 
            ],
        },
    },
    ThemePreset {
        id: "violet",
        label: "midnight purple",
        description: "violet accent · pink-purple log",
        mode: ThemeMode::Dark,
        colors: ThemeColors {
            values: [
                Rgba::new(11, 9, 18, 255),     
                Rgba::new(13, 11, 22, 255),    
                Rgba::new(19, 16, 32, 255),    
                Rgba::new(25, 21, 39, 255),    
                Rgba::new(32, 27, 48, 255),    
                Rgba::new(23, 19, 32, 255),    
                Rgba::new(42, 36, 64, 255),    
                Rgba::new(59, 51, 85, 255),    
                Rgba::new(36, 31, 54, 255),    
                Rgba::new(238, 234, 255, 255), 
                Rgba::new(255, 255, 255, 255), 
                Rgba::new(212, 205, 242, 255), 
                Rgba::new(154, 146, 189, 255), 
                Rgba::new(125, 118, 160, 255), 
                Rgba::new(25, 21, 39, 255),    
                Rgba::new(36, 31, 56, 255),    
                Rgba::new(45, 39, 72, 255),    
                Rgba::new(33, 26, 58, 255),    
                Rgba::new(180, 140, 255, 255), 
                Rgba::new(107, 70, 201, 255),  
                Rgba::new(255, 255, 255, 255), 
                Rgba::new(11, 9, 18, 255),     
                Rgba::new(255, 122, 217, 255), 
                Rgba::new(95, 211, 154, 255),  
                Rgba::new(168, 240, 205, 255), 
                Rgba::new(31, 92, 64, 255),    
                Rgba::new(47, 122, 85, 255),   
                Rgba::new(255, 122, 138, 255), 
                Rgba::new(255, 194, 201, 255), 
                Rgba::new(106, 36, 50, 255),   
                Rgba::new(42, 24, 32, 255),    
                Rgba::new(141, 47, 68, 255),   
                Rgba::new(160, 58, 82, 255),   
                Rgba::new(255, 230, 234, 255), 
                Rgba::new(255, 210, 122, 255), 
                Rgba::new(224, 168, 76, 255),  
                Rgba::new(122, 95, 47, 255),   
                Rgba::new(255, 154, 232, 255), 
            ],
        },
    },
    ThemePreset {
        id: "graphite",
        label: "graphite",
        description: "neutral gray base · amber accent",
        mode: ThemeMode::Dark,
        colors: ThemeColors {
            values: [
                Rgba::new(14, 15, 17, 255),    
                Rgba::new(10, 11, 13, 255),    
                Rgba::new(21, 23, 26, 255),    
                Rgba::new(26, 28, 32, 255),    
                Rgba::new(33, 36, 41, 255),    
                Rgba::new(23, 25, 28, 255),    
                Rgba::new(42, 45, 51, 255),    
                Rgba::new(58, 62, 70, 255),    
                Rgba::new(36, 38, 43, 255),    
                Rgba::new(231, 233, 236, 255), 
                Rgba::new(255, 255, 255, 255), 
                Rgba::new(204, 208, 214, 255), 
                Rgba::new(142, 148, 158, 255), 
                Rgba::new(115, 121, 130, 255), 
                Rgba::new(26, 28, 32, 255),    
                Rgba::new(38, 41, 47, 255),    
                Rgba::new(47, 51, 59, 255),    
                Rgba::new(34, 38, 44, 255),    
                Rgba::new(226, 180, 85, 255),  
                Rgba::new(138, 106, 24, 255),  
                Rgba::new(255, 255, 255, 255), 
                Rgba::new(20, 18, 10, 255),    
                Rgba::new(127, 215, 195, 255), 
                Rgba::new(88, 189, 134, 255),  
                Rgba::new(158, 230, 188, 255), 
                Rgba::new(35, 85, 58, 255),    
                Rgba::new(49, 117, 79, 255),   
                Rgba::new(255, 115, 115, 255), 
                Rgba::new(255, 188, 188, 255), 
                Rgba::new(95, 42, 42, 255),    
                Rgba::new(42, 27, 27, 255),    
                Rgba::new(141, 48, 48, 255),   
                Rgba::new(160, 61, 61, 255),   
                Rgba::new(255, 230, 230, 255), 
                Rgba::new(255, 208, 107, 255), 
                Rgba::new(224, 168, 76, 255),  
                Rgba::new(122, 101, 47, 255),  
                Rgba::new(255, 122, 217, 255), 
            ],
        },
    },
    ThemePreset {
        id: "forest",
        label: "forest green",
        description: "ink green base · mint accent",
        mode: ThemeMode::Dark,
        colors: ThemeColors {
            values: [
                Rgba::new(8, 16, 11, 255),     
                Rgba::new(7, 13, 9, 255),      
                Rgba::new(15, 26, 19, 255),    
                Rgba::new(19, 33, 23, 255),    
                Rgba::new(24, 40, 29, 255),    
                Rgba::new(14, 23, 18, 255),    
                Rgba::new(32, 56, 40, 255),    
                Rgba::new(45, 77, 55, 255),    
                Rgba::new(27, 47, 34, 255),    
                Rgba::new(228, 242, 232, 255), 
                Rgba::new(255, 255, 255, 255), 
                Rgba::new(198, 221, 205, 255), 
                Rgba::new(134, 162, 145, 255), 
                Rgba::new(107, 133, 119, 255), 
                Rgba::new(19, 33, 23, 255),    
                Rgba::new(26, 43, 32, 255),    
                Rgba::new(33, 56, 42, 255),    
                Rgba::new(23, 48, 31, 255),    
                Rgba::new(110, 231, 160, 255), 
                Rgba::new(35, 122, 74, 255),   
                Rgba::new(255, 255, 255, 255), 
                Rgba::new(6, 18, 12, 255),     
                Rgba::new(127, 215, 195, 255), 
                Rgba::new(79, 191, 135, 255),  
                Rgba::new(169, 240, 198, 255), 
                Rgba::new(31, 92, 59, 255),    
                Rgba::new(47, 122, 82, 255),   
                Rgba::new(255, 125, 125, 255), 
                Rgba::new(255, 192, 192, 255), 
                Rgba::new(94, 42, 44, 255),    
                Rgba::new(38, 26, 27, 255),    
                Rgba::new(141, 50, 56, 255),   
                Rgba::new(160, 63, 69, 255),   
                Rgba::new(255, 232, 232, 255), 
                Rgba::new(255, 210, 122, 255), 
                Rgba::new(224, 168, 76, 255),  
                Rgba::new(122, 101, 47, 255),  
                Rgba::new(255, 140, 224, 255), 
            ],
        },
    },
    ThemePreset {
        id: "paper",
        label: "paper white",
        description: "light base · high readability",
        mode: ThemeMode::Light,
        colors: ThemeColors {
            values: [
                Rgba::new(242, 244, 247, 255), 
                Rgba::new(232, 235, 240, 255), 
                Rgba::new(255, 255, 255, 255), 
                Rgba::new(247, 248, 250, 255), 
                Rgba::new(255, 255, 255, 255), 
                Rgba::new(238, 241, 245, 255), 
                Rgba::new(214, 218, 225, 255), 
                Rgba::new(185, 191, 201, 255), 
                Rgba::new(227, 230, 235, 255), 
                Rgba::new(32, 36, 44, 255),    
                Rgba::new(0, 0, 0, 255),       
                Rgba::new(74, 82, 97, 255),    
                Rgba::new(107, 115, 131, 255), 
                Rgba::new(139, 147, 161, 255), 
                Rgba::new(238, 241, 246, 255), 
                Rgba::new(233, 237, 244, 255), 
                Rgba::new(221, 229, 242, 255), 
                Rgba::new(226, 235, 250, 255), 
                Rgba::new(31, 111, 235, 255),  
                Rgba::new(31, 111, 235, 255),  
                Rgba::new(255, 255, 255, 255), 
                Rgba::new(255, 255, 255, 255), 
                Rgba::new(15, 157, 138, 255),  
                Rgba::new(31, 143, 85, 255),   
                Rgba::new(20, 107, 63, 255),   
                Rgba::new(217, 242, 228, 255), 
                Rgba::new(134, 207, 168, 255), 
                Rgba::new(208, 52, 63, 255),   
                Rgba::new(163, 36, 46, 255),   
                Rgba::new(251, 218, 221, 255), 
                Rgba::new(253, 236, 237, 255), 
                Rgba::new(208, 52, 63, 255),   
                Rgba::new(231, 154, 161, 255), 
                Rgba::new(255, 255, 255, 255), 
                Rgba::new(176, 122, 18, 255),  
                Rgba::new(138, 95, 10, 255),   
                Rgba::new(227, 201, 138, 255), 
                Rgba::new(177, 63, 176, 255),  
            ],
        },
    },
];

pub const DEFAULT_THEME_PRESET: &str = THEME_PRESETS[0].id;

impl ThemeColors {
    pub fn preset(id: &str) -> ThemeColors {
        THEME_PRESETS
            .iter()
            .find(|preset| preset.id == id)
            .unwrap_or(&THEME_PRESETS[0])
            .colors
    }

    /// Overlay per-token overrides on top of a preset (only storing changed tokens).
    pub fn with_overrides(id: &str, overrides: &HashMap<ThemeToken, Rgba>) -> ThemeColors {
        let mut colors = Self::preset(id);
        for (token, color) in overrides {
            colors.set(*token, *color);
        }
        colors
    }
}

pub fn preset(id: &str) -> &'static ThemePreset {
    THEME_PRESETS
        .iter()
        .find(|preset| preset.id == id)
        .unwrap_or(&THEME_PRESETS[0])
}

/// UI theme state (corresponds to `prisma-editor.theme.v1`).
#[derive(Debug, Clone, PartialEq)]
pub struct ThemeState {
    pub preset: String,
    pub overrides: HashMap<ThemeToken, Rgba>,
}

impl Default for ThemeState {
    fn default() -> Self {
        Self {
            preset: DEFAULT_THEME_PRESET.to_string(),
            overrides: HashMap::new(),
        }
    }
}

impl ThemeState {
    pub fn colors(&self) -> ThemeColors {
        ThemeColors::with_overrides(&self.preset, &self.overrides)
    }

    pub fn set_preset(&mut self, id: &str) {
        self.preset = id.to_string();
        self.overrides.clear();
    }

    pub fn set_override(&mut self, token: ThemeToken, color: Rgba) {
        self.overrides.insert(token, color);
    }

    pub fn reset_override(&mut self, token: ThemeToken) {
        self.overrides.remove(&token);
    }
}

/// RGB (0..1) -> HSV (hue 0..360, saturation/value 0..1).
pub fn rgb_to_hsv(rgb: [f32; 3]) -> [f32; 3] {
    let [r, g, b] = rgb.map(|value| value.clamp(0.0, 1.0));
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;
    let hue = if delta < 1e-6 {
        0.0
    } else if max == r {
        60.0 * (((g - b) / delta) % 6.0)
    } else if max == g {
        60.0 * ((b - r) / delta + 2.0)
    } else {
        60.0 * ((r - g) / delta + 4.0)
    };
    let hue = if hue < 0.0 { hue + 360.0 } else { hue };
    let saturation = if max < 1e-6 { 0.0 } else { delta / max };
    [hue, saturation, max]
}

/// HSV (hue 0..360, saturation/value 0..1) -> RGB (0..1).
pub fn hsv_to_rgb(hsv: [f32; 3]) -> [f32; 3] {
    let hue = hsv[0].rem_euclid(360.0);
    let saturation = hsv[1].clamp(0.0, 1.0);
    let value = hsv[2].clamp(0.0, 1.0);
    let chroma = value * saturation;
    let section = hue / 60.0;
    let x = chroma * (1.0 - (section % 2.0 - 1.0).abs());
    let (r, g, b) = match section as u32 {
        0 => (chroma, x, 0.0),
        1 => (x, chroma, 0.0),
        2 => (0.0, chroma, x),
        3 => (0.0, x, chroma),
        4 => (x, 0.0, chroma),
        _ => (chroma, 0.0, x),
    };
    let m = value - chroma;
    [r + m, g + m, b + m]
}

/// `[r, g, b, a]` (0..1) -> `#rrggbb` (`#rrggbbaa` when `alpha` is true).
pub fn rgba_to_hex(rgba: [f32; 4], alpha: bool) -> String {
    let byte = |value: f32| (value.clamp(0.0, 1.0) * 255.0).round() as u8;
    if alpha {
        format!(
            "#{:02x}{:02x}{:02x}{:02x}",
            byte(rgba[0]),
            byte(rgba[1]),
            byte(rgba[2]),
            byte(rgba[3])
        )
    } else {
        format!(
            "#{:02x}{:02x}{:02x}",
            byte(rgba[0]),
            byte(rgba[1]),
            byte(rgba[2])
        )
    }
}

/// `#rgb` / `#rrggbb` / `#rrggbbaa` -> `[r, g, b, a]` (0..1).
pub fn hex_to_rgba(hex: &str) -> Option<[f32; 4]> {
    let hex = hex.trim().trim_start_matches('#');
    let parse = |range: std::ops::Range<usize>| {
        u8::from_str_radix(hex.get(range)?, 16)
            .ok()
            .map(|value| f32::from(value) / 255.0)
    };
    match hex.len() {
        6 => Some([parse(0..2)?, parse(2..4)?, parse(4..6)?, 1.0]),
        8 => Some([parse(0..2)?, parse(2..4)?, parse(4..6)?, parse(6..8)?]),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hsv_and_rgb_round_trip() {
        for rgb in [
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
            [0.2, 0.6, 0.9],
            [0.5, 0.5, 0.5],
        ] {
            let hsv = rgb_to_hsv(rgb);
            let back = hsv_to_rgb(hsv);
            for axis in 0..3 {
                assert!(
                    (back[axis] - rgb[axis]).abs() < 1e-3,
                    "{rgb:?} → {hsv:?} → {back:?}"
                );
            }
        }
        assert_eq!(rgb_to_hsv([0.0, 0.0, 0.0])[2], 0.0);
    }

    #[test]
    fn hex_parses_and_formats_rgba() {
        assert_eq!(rgba_to_hex([1.0, 0.5, 0.0, 1.0], false), "#ff8000");
        assert_eq!(rgba_to_hex([1.0, 0.5, 0.0, 0.5], true), "#ff800080");
        let color = hex_to_rgba("#ff8000").unwrap();
        assert!((color[0] - 1.0).abs() < 1e-3);
        assert!((color[3] - 1.0).abs() < 1e-3);
        assert!((hex_to_rgba("ff800080").unwrap()[3] - 128.0 / 255.0).abs() < 1e-3);
        assert_eq!(hex_to_rgba("#xyz"), None);
        assert_eq!(hex_to_rgba("#fff"), None);
    }
}
