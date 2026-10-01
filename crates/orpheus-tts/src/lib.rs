//! orpheus-tts: model-agnostic TTS abstraction.
//!
//! The reader (orpheus binary) must NEVER match on concrete model names.
//! It only talks to `TtsManager` / `TTSBackend` via this interface.
//!
//! Future backends (Kokoro, Chatterbox, Chatterbox-Turbo, custom HF ids)
//! implement `TTSBackend` and are registered without touching reader code.
//!
//! V1 Milestone 1+2 ships with `MockBackend` only. Real Python worker
//! integration (Unix socket / HTTP per GUIDE §3) lands in Milestone 5+6.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum TtsError {
    #[error("backend error: {0}")]
    Backend(String),
    #[error("model not found: {0}")]
    ModelNotFound(String),
    #[error("voice not found: {0}")]
    VoiceNotFound(String),
    #[error("model not loaded")]
    NotLoaded,
    #[error("synthesis failed: {0}")]
    Synthesis(String),
    #[error("io: {0}")]
    Io(String),
}

pub type Result<T> = std::result::Result<T, TtsError>;

/// Which family a model belongs to. `Custom` covers "type a HF id and pull it".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum BackendKind {
    Mock,
    Kokoro,
    Chatterbox,
    ChatterboxTurbo,
    /// User-typed model id, e.g. "hexgrad/Kokoro-82M" or any future backend.
    /// Resolved dynamically by the worker; the reader stays agnostic.
    Custom(String),
}

impl BackendKind {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Mock => "mock",
            Self::Kokoro => "kokoro",
            Self::Chatterbox => "chatterbox",
            Self::ChatterboxTurbo => "chatterbox-turbo",
            Self::Custom(s) => s.as_str(),
        }
    }

    /// Heavier models need GPU; lightweight ones run fine on CPU.
    /// Used by UI to hint "good while multitasking" vs "best quality".
    pub fn weight_class(&self) -> WeightClass {
        match self {
            Self::Mock | Self::Kokoro => WeightClass::Light,
            Self::ChatterboxTurbo => WeightClass::Medium,
            Self::Chatterbox => WeightClass::Heavy,
            // Unknown custom models default to medium; worker reports real requirements.
            Self::Custom(_) => WeightClass::Medium,
        }
    }

    pub fn from_id(id: &str) -> Self {
        match id.trim().to_lowercase().as_str() {
            "mock" => Self::Mock,
            "kokoro" => Self::Kokoro,
            "chatterbox" => Self::Chatterbox,
            "chatterbox-turbo" | "chatterbox_turbo" | "turbo" => Self::ChatterboxTurbo,
            other => Self::Custom(other.to_string()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WeightClass {
    Light,
    Medium,
    Heavy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TtsCapabilities {
    pub voice_cloning: bool,
    pub streaming: bool,
    pub word_timing: bool,
    pub languages: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TtsModel {
    pub id: String,
    pub name: String,
    pub backend: BackendKind,
    pub language: String,
    /// Whether model files are present locally.
    pub installed: bool,
    pub capabilities: TtsCapabilities,
    /// Local path once pulled (None until installed).
    pub local_path: Option<PathBuf>,
}

impl TtsModel {
    pub fn mock() -> Self {
        Self {
            id: "mock".into(),
            name: "Mock (no audio)".into(),
            backend: BackendKind::Mock,
            language: "en".into(),
            installed: true,
            capabilities: TtsCapabilities {
                voice_cloning: false,
                streaming: false,
                word_timing: false,
                languages: vec!["en".into()],
            },
            local_path: None,
        }
    }

    pub fn builtin_catalog() -> Vec<Self> {
        vec![
            Self {
                id: "kokoro".into(),
                name: "Kokoro (lightweight)".into(),
                backend: BackendKind::Kokoro,
                language: "en".into(),
                installed: false,
                capabilities: TtsCapabilities {
                    voice_cloning: false,
                    streaming: true,
                    word_timing: true,
                    languages: vec!["en".into()],
                },
                local_path: None,
            },
            Self {
                id: "chatterbox-turbo".into(),
                name: "Chatterbox Turbo (cloning, efficient)".into(),
                backend: BackendKind::ChatterboxTurbo,
                language: "en".into(),
                installed: false,
                capabilities: TtsCapabilities {
                    voice_cloning: true,
                    streaming: false,
                    word_timing: false,
                    languages: vec!["en".into()],
                },
                local_path: None,
            },
            Self {
                id: "chatterbox".into(),
                name: "Chatterbox (cloning, best quality)".into(),
                backend: BackendKind::Chatterbox,
                language: "en".into(),
                installed: false,
                capabilities: TtsCapabilities {
                    voice_cloning: true,
                    streaming: false,
                    word_timing: false,
                    languages: vec!["en".into()],
                },
                local_path: None,
            },
            Self::mock(),
        ]
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Voice {
    pub id: String,
    pub name: String,
    pub model_id: String,
    pub reference_audio: Option<PathBuf>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl Voice {
    pub fn default_voice(model_id: &str) -> Self {
        Self {
            id: format!("{model_id}-default"),
            name: "Default Narrator".into(),
            model_id: model_id.into(),
            reference_audio: None,
            created_at: chrono::Utc::now(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SynthesisRequest {
    pub request_id: String,
    pub model: String,
    pub voice: String,
    pub text: String,
    pub speed: f32,
    /// Absolute path the worker should write audio to (opus preferred).
    pub out_path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SynthesisResult {
    pub request_id: String,
    pub audio_path: PathBuf,
    pub duration_secs: f32,
}

/// The single interface the reader uses. No model-specific branches allowed
/// outside implementations of this trait.
#[async_trait::async_trait]
pub trait TTSBackend: Send + Sync {
    fn name(&self) -> &str;
    fn capabilities(&self) -> TtsCapabilities;
    async fn load_model(&mut self) -> Result<()>;
    async fn unload_model(&mut self) -> Result<()>;
    async fn list_voices(&self) -> Result<Vec<Voice>>;
    async fn synthesize(&self, req: &SynthesisRequest) -> Result<SynthesisResult>;
    async fn health(&self) -> Result<bool>;
}

/// Stand-in backend until the Python worker lands. Always healthy,
/// returns a tiny placeholder result so queue/playback code can be
/// exercised without audio.
pub struct MockBackend {
    model_id: String,
}

impl MockBackend {
    pub fn new(model_id: impl Into<String>) -> Self {
        Self {
            model_id: model_id.into(),
        }
    }
}

#[async_trait::async_trait]
impl TTSBackend for MockBackend {
    fn name(&self) -> &str {
        "mock"
    }
    fn capabilities(&self) -> TtsCapabilities {
        TtsCapabilities {
            voice_cloning: false,
            streaming: false,
            word_timing: false,
            languages: vec!["en".into()],
        }
    }
    async fn load_model(&mut self) -> Result<()> {
        Ok(())
    }
    async fn unload_model(&mut self) -> Result<()> {
        Ok(())
    }
    async fn list_voices(&self) -> Result<Vec<Voice>> {
        Ok(vec![Voice::default_voice(&self.model_id)])
    }
    async fn synthesize(&self, req: &SynthesisRequest) -> Result<SynthesisResult> {
        // Estimate ~15 chars/sec for placeholder duration so UI progress works.
        let dur = (req.text.len() as f32 / 15.0).clamp(0.5, 30.0);
        Ok(SynthesisResult {
            request_id: req.request_id.clone(),
            audio_path: req.out_path.clone(),
            duration_secs: dur,
        })
    }
    async fn health(&self) -> Result<bool> {
        Ok(true)
    }
}

/// Owns the active model + voice selection. Reader calls only this.
pub struct TtsManager {
    pub models: HashMap<String, TtsModel>,
    pub current_model_id: String,
    pub current_voice_id: String,
}

impl TtsManager {
    pub fn new() -> Self {
        let mut models = HashMap::new();
        for m in TtsModel::builtin_catalog() {
            models.insert(m.id.clone(), m);
        }
        Self {
            current_model_id: "kokoro".into(),
            current_voice_id: "kokoro-default".into(),
            models,
        }
    }

    pub fn current_model(&self) -> Option<&TtsModel> {
        self.models.get(&self.current_model_id)
    }

    pub fn list_models(&self) -> Vec<&TtsModel> {
        let mut v: Vec<_> = self.models.values().collect();
        v.sort_by(|a, b| a.name.cmp(&b.name));
        v
    }

    /// Switch model without touching book/position (GUIDE §11).
    /// Lazy-load contract: caller unloads old + loads new via backend.
    pub fn select_model(&mut self, id: &str) -> Result<()> {
        if self.models.contains_key(id) {
            self.current_model_id = id.to_string();
            self.current_voice_id = format!("{id}-default");
            Ok(())
        } else {
            // Support "type a model name and pull it on the fly":
            // register unknown ids as Custom so UI/worker can install them.
            let kind = BackendKind::from_id(id);
            let model = TtsModel {
                id: id.to_string(),
                name: id.to_string(),
                backend: kind,
                language: "en".into(),
                installed: false,
                capabilities: TtsCapabilities {
                    voice_cloning: false,
                    streaming: false,
                    word_timing: false,
                    languages: vec!["en".into()],
                },
                local_path: None,
            };
            self.models.insert(id.to_string(), model);
            self.current_model_id = id.to_string();
            self.current_voice_id = format!("{id}-default");
            Ok(())
        }
    }

    pub fn mark_installed(&mut self, id: &str, path: PathBuf) {
        if let Some(m) = self.models.get_mut(id) {
            m.installed = true;
            m.local_path = Some(path);
        }
    }
}

impl Default for TtsManager {
    fn default() -> Self {
        Self::new()
    }
}

// Re-export async_trait macro without adding the dep to consumers.
mod async_trait {
    pub use async_trait::async_trait;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_catalog_has_light_and_heavy() {
        let m = TtsManager::new();
        assert!(m.models.contains_key("kokoro"));
        assert!(m.models.contains_key("chatterbox-turbo"));
        assert!(m.models.contains_key("chatterbox"));
        assert_eq!(
            m.models["kokoro"].backend.weight_class(),
            WeightClass::Light
        );
        assert_eq!(
            m.models["chatterbox"].backend.weight_class(),
            WeightClass::Heavy
        );
    }

    #[test]
    fn select_custom_model_registers_on_the_fly() {
        let mut m = TtsManager::new();
        m.select_model("hexgrad/Kokoro-82M").unwrap();
        assert_eq!(m.current_model_id, "hexgrad/Kokoro-82M");
        assert!(m.models.contains_key("hexgrad/Kokoro-82M"));
    }

    #[tokio::test]
    async fn mock_synthesize_returns_duration() {
        let b = MockBackend::new("mock");
        assert!(b.health().await.unwrap());
        let req = SynthesisRequest {
            request_id: "r1".into(),
            model: "mock".into(),
            voice: "mock-default".into(),
            text: "The desert was vast and silent.".into(),
            speed: 1.0,
            out_path: std::path::PathBuf::from("/tmp/r1.opus"),
        };
        let res = b.synthesize(&req).await.unwrap();
        assert!(res.duration_secs > 0.0);
    }
}
