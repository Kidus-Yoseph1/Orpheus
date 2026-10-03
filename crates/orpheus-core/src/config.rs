//! GUIDE §26 — TOML configuration.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_reader")]
    pub reader: ReaderConfig,
    #[serde(default)]
    pub playback: PlaybackConfig,
    #[serde(default)]
    pub tts: TtsConfig,
    #[serde(default)]
    pub library: LibraryConfig,
    #[serde(default)]
    pub cache: CacheConfig,
    #[serde(default)]
    pub pronunciation: std::collections::HashMap<String, String>,
}

fn default_reader() -> ReaderConfig {
    ReaderConfig::default()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReaderConfig {
    #[serde(default = "default_theme")]
    pub theme: String,
    #[serde(default = "default_style")]
    pub style: String,
    #[serde(default = "default_margin")]
    pub margin: u16,
    #[serde(default = "default_line_spacing")]
    pub line_spacing: u16,
}

fn default_theme() -> String {
    "midnight".into()
}
fn default_style() -> String {
    "paper".into()
}
fn default_margin() -> u16 {
    6
}
fn default_line_spacing() -> u16 {
    1
}

impl Default for ReaderConfig {
    fn default() -> Self {
        Self {
            theme: default_theme(),
            style: default_style(),
            margin: default_margin(),
            line_spacing: default_line_spacing(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaybackConfig {
    #[serde(default = "default_speed")]
    pub speed: f32,
    #[serde(default = "default_seek")]
    pub seek_seconds: u64,
    #[serde(default = "default_buffer")]
    pub buffer_seconds: u64,
}

fn default_speed() -> f32 {
    1.0
}
fn default_seek() -> u64 {
    10
}
fn default_buffer() -> u64 {
    90
}

impl Default for PlaybackConfig {
    fn default() -> Self {
        Self {
            speed: 1.0,
            seek_seconds: 10,
            buffer_seconds: 90,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TtsConfig {
    #[serde(default = "default_model")]
    pub model: String,
    #[serde(default = "default_device")]
    pub device: String,
    #[serde(default = "default_voice")]
    pub voice: String,
    #[serde(default = "default_worker_url")]
    pub worker_url: String,
    #[serde(default)]
    pub gpu: GpuConfig,
}

fn default_model() -> String {
    "kokoro".into()
}
fn default_device() -> String {
    "auto".into()
}
fn default_voice() -> String {
    "kokoro-default".into()
}
fn default_worker_url() -> String {
    "http://127.0.0.1:8765".into()
}

impl Default for TtsConfig {
    fn default() -> Self {
        Self {
            model: default_model(),
            device: default_device(),
            voice: default_voice(),
            worker_url: default_worker_url(),
            gpu: GpuConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_vram")]
    pub max_vram_mb: u64,
}

fn default_true() -> bool {
    true
}
fn default_vram() -> u64 {
    2500
}

impl Default for GpuConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_vram_mb: 2500,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LibraryConfig {
    #[serde(default)]
    pub directories: Vec<String>,
    #[serde(default = "default_true")]
    pub recursive: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_cache_gb")]
    pub max_size_gb: u64,
}

fn default_cache_gb() -> u64 {
    10
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_size_gb: 10,
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            reader: ReaderConfig::default(),
            playback: PlaybackConfig::default(),
            tts: TtsConfig::default(),
            library: LibraryConfig::default(),
            cache: CacheConfig::default(),
            pronunciation: Default::default(),
        }
    }
}

impl Config {
    pub fn path() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("orpheus").join("config.toml"))
    }

    pub fn load() -> Self {
        Self::path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| toml::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> Result<()> {
        if let Some(p) = Self::path() {
            if let Some(parent) = p.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(p, toml::to_string_pretty(self)?)?;
        }
        Ok(())
    }

    pub fn data_dir() -> PathBuf {
        dirs::data_dir()
            .map(|d| d.join("orpheus"))
            .unwrap_or_else(|| PathBuf::from(".orpheus-data"))
    }

    pub fn db_path() -> PathBuf {
        Self::data_dir().join("library.db")
    }

    pub fn audio_cache_dir() -> PathBuf {
        Self::data_dir().join("cache").join("audio")
    }

    /// Normalized user voice references: `<data>/voices/<id>.wav`.
    /// The worker is started with the same dir (`--voices-dir`).
    pub fn voices_dir() -> PathBuf {
        Self::data_dir().join("voices")
    }
}
