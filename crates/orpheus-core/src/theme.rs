//! GUIDE §27–28 — theme system. Config-driven palettes + reader styles.
//!
//! Themes live in `<config_dir>/orpheus/themes/*.toml` and ship with
//! built-ins. Terminal falls back gracefully without truecolor.

use anyhow::Result;
use ratatui::style::Color;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Theme {
    pub name: String,
    #[serde(default = "default_bg")]
    pub background: String,
    #[serde(default = "default_fg")]
    pub foreground: String,
    #[serde(default)]
    pub muted: String,
    #[serde(default)]
    pub heading: String,
    #[serde(default)]
    pub accent: String,
    #[serde(default)]
    pub current_sentence: String,
    #[serde(default)]
    pub selection: String,
    #[serde(default)]
    pub progress: String,
    #[serde(default)]
    pub border: String,
    #[serde(default)]
    pub status: String,
}

fn default_bg() -> String {
    "#1e1e2e".into()
}
fn default_fg() -> String {
    "#cdd6f4".into()
}

impl Default for Theme {
    fn default() -> Self {
        Self::midnight()
    }
}

impl Theme {
    pub fn builtin_names() -> Vec<&'static str> {
        vec!["midnight", "paper", "sepia", "dark", "forest", "minimal"]
    }

    pub fn builtin(name: &str) -> Self {
        match name {
            "paper" => Self {
                name: "paper".into(),
                background: "#F4EED8".into(),
                foreground: "#302B24".into(),
                muted: "#847A68".into(),
                heading: "#5E4028".into(),
                accent: "#8B5E34".into(),
                current_sentence: "#A34F24".into(),
                selection: "#E4D9BE".into(),
                progress: "#8B5E34".into(),
                border: "#C9BFA8".into(),
                status: "#6B5E4E".into(),
            },
            "sepia" => Self {
                name: "sepia".into(),
                background: "#E8D5B5".into(),
                foreground: "#4A3728".into(),
                muted: "#8C7A65".into(),
                heading: "#5C3D2E".into(),
                accent: "#9C5E2E".into(),
                current_sentence: "#B34A1F".into(),
                selection: "#D9C39A".into(),
                progress: "#9C5E2E".into(),
                border: "#BFA87E".into(),
                status: "#7A6A55".into(),
            },
            "dark" => Self {
                name: "dark".into(),
                background: "#1a1a1a".into(),
                foreground: "#d4d4d4".into(),
                muted: "#808080".into(),
                heading: "#e0e0e0".into(),
                accent: "#4e9a51".into(),
                current_sentence: "#ffd479".into(),
                selection: "#3a3a3a".into(),
                progress: "#4e9a51".into(),
                border: "#404040".into(),
                status: "#808080".into(),
            },
            "forest" => Self {
                name: "forest".into(),
                background: "#1a2421".into(),
                foreground: "#dce8d4".into(),
                muted: "#7a8a7a".into(),
                heading: "#b8d4a8".into(),
                accent: "#6aaa64".into(),
                current_sentence: "#f0d060".into(),
                selection: "#2a3a2a".into(),
                progress: "#6aaa64".into(),
                border: "#3a4a3a".into(),
                status: "#7a8a7a".into(),
            },
            "minimal" => Self {
                name: "minimal".into(),
                background: "#000000".into(),
                foreground: "#e8e8e8".into(),
                muted: "#666666".into(),
                heading: "#ffffff".into(),
                accent: "#888888".into(),
                current_sentence: "#ffffff".into(),
                selection: "#222222".into(),
                progress: "#888888".into(),
                border: "#333333".into(),
                status: "#666666".into(),
            },
            _ => Self::midnight(),
        }
    }

    pub fn midnight() -> Self {
        Self {
            name: "midnight".into(),
            background: "#1e1e2e".into(),
            foreground: "#cdd6f4".into(),
            muted: "#6c7086".into(),
            heading: "#cba6f7".into(),
            accent: "#89b4fa".into(),
            current_sentence: "#f9e2af".into(),
            selection: "#313244".into(),
            progress: "#89b4fa".into(),
            border: "#45475a".into(),
            status: "#6c7086".into(),
        }
    }

    pub fn from_toml_str(s: &str) -> Result<Self> {
        Ok(toml::from_str(s)?)
    }

    fn hex(&self, raw: &str) -> Color {
        parse_hex(raw).unwrap_or(Color::Reset)
    }

    pub fn bg(&self) -> Color {
        self.hex(&self.background)
    }
    pub fn fg(&self) -> Color {
        self.hex(&self.foreground)
    }
    pub fn muted_c(&self) -> Color {
        self.hex(&self.muted)
    }
    pub fn heading_c(&self) -> Color {
        self.hex(&self.heading)
    }
    pub fn accent_c(&self) -> Color {
        self.hex(&self.accent)
    }
    pub fn current_c(&self) -> Color {
        self.hex(&self.current_sentence)
    }
    pub fn selection_c(&self) -> Color {
        self.hex(&self.selection)
    }
    pub fn progress_c(&self) -> Color {
        self.hex(&self.progress)
    }
    pub fn border_c(&self) -> Color {
        self.hex(&self.border)
    }
    pub fn status_c(&self) -> Color {
        self.hex(&self.status)
    }
}

fn parse_hex(s: &str) -> Option<Color> {
    let s = s.trim().trim_start_matches('#');
    if s.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&s[0..2], 16).ok()?;
    let g = u8::from_str_radix(&s[2..4], 16).ok()?;
    let b = u8::from_str_radix(&s[4..6], 16).ok()?;
    Some(Color::Rgb(r, g, b))
}

/// Reader visual styles (GUIDE §28) — layout, not just palette.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReaderStyle {
    Classic,
    #[default]
    Paper,
    Sepia,
    Midnight,
    Focus,
    Minimal,
}

impl ReaderStyle {
    pub fn all() -> Vec<Self> {
        vec![
            Self::Classic,
            Self::Paper,
            Self::Sepia,
            Self::Midnight,
            Self::Focus,
            Self::Minimal,
        ]
    }
    pub fn name(&self) -> &'static str {
        match self {
            Self::Classic => "classic",
            Self::Paper => "paper",
            Self::Sepia => "sepia",
            Self::Midnight => "midnight",
            Self::Focus => "focus",
            Self::Minimal => "minimal",
        }
    }
    pub fn from_name(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "classic" => Self::Classic,
            "sepia" => Self::Sepia,
            "midnight" => Self::Midnight,
            "focus" => Self::Focus,
            "minimal" => Self::Minimal,
            _ => Self::Paper,
        }
    }
    /// How many surrounding paragraphs to show in focus mode etc.
    pub fn context_paragraphs(&self) -> usize {
        match self {
            Self::Focus => 1,
            Self::Minimal => usize::MAX,
            _ => 6,
        }
    }
}

/// Load all themes from a directory (user overrides + built-ins).
pub fn load_themes(dir: &std::path::Path) -> HashMap<String, Theme> {
    let mut map = HashMap::new();
    for name in Theme::builtin_names() {
        map.insert(name.to_string(), Theme::builtin(name));
    }
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.extension().and_then(|e| e.to_str()) != Some("toml") {
                continue;
            }
            if let Ok(s) = std::fs::read_to_string(&p) {
                if let Ok(t) = Theme::from_toml_str(&s) {
                    map.insert(t.name.clone(), t);
                }
            }
        }
    }
    map
}
