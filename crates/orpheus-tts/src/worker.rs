//! HTTP client for `book-tts` (`tts-worker/server.py`).
//!
//! Contract mirrors the server exactly:
//! - cache key `sha256("orpheus-v1\x00" + model + "\x00" + voice_stem +
//!   "\x00" + normalized_text)[:32]`; worker stores `<key>.opus|wav`.
//! - speed is a *playback* concern (ffplay atempo); synthesis is always 1x.

use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{Result, TTSBackend, TtsCapabilities, TtsError, Voice};

pub const WORKER_DEFAULT_URL: &str = "http://127.0.0.1:8765";
pub const DEFAULT_VOICE_STEM: &str = "af_heart";

/// Map a UI voice id to the on-disk voice stem, identical to the server rule.
pub fn voice_stem(voice: &str) -> String {
    match voice {
        "kokoro-default" | "default" | "" => DEFAULT_VOICE_STEM.to_string(),
        v => v.to_string(),
    }
}

/// Default UI voice id for a model (what the Voices screen lists first).
pub fn voice_stem_default(model_id: &str) -> String {
    match model_id {
        "kokoro" => "kokoro-default".into(),
        _ => format!("{model_id}-default"),
    }
}

fn normalize_text(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Local cache path for a synthesis, without touching the network.
/// Checks `<key>.opus` first, then `<key>.wav`.
pub fn worker_cache_candidates(
    cache_dir: &std::path::Path,
    model: &str,
    voice: &str,
    text: &str,
) -> (PathBuf, Vec<PathBuf>) {
    let norm = normalize_text(text);
    let mut h = Sha256::new();
    h.update(b"orpheus-v1\x00");
    h.update(model.as_bytes());
    h.update(b"\x00");
    h.update(voice_stem(voice).as_bytes());
    h.update(b"\x00");
    h.update(norm.as_bytes());
    let key = hex_encode(&h.finalize()[..16]);
    let opus = cache_dir.join(format!("{key}.opus"));
    let wav = cache_dir.join(format!("{key}.wav"));
    (opus.clone(), vec![opus, wav])
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0xf) as usize] as char);
    }
    s
}

#[derive(Debug, Clone, Serialize)]
struct SynthBody {
    request_id: String,
    model: String,
    voice: String,
    text: String,
    speed: f32,
}

#[derive(Debug, Deserialize)]
struct SynthReply {
    request_id: String,
    audio_path: PathBuf,
    duration_secs: f32,
    #[allow(dead_code)]
    cached: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct HealthReply {
    ok: bool,
    model: Option<String>,
    #[allow(dead_code)]
    loaded: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct VoicesReply {
    voices: Vec<WorkerVoice>,
}

#[derive(Debug, Deserialize)]
struct WorkerVoice {
    id: String,
    name: String,
}

#[derive(Debug, Deserialize)]
struct ErrorReply {
    error: Option<String>,
}

/// `TTSBackend` over HTTP to a running `book-tts` worker.
#[derive(Debug, Clone)]
pub struct WorkerBackend {
    pub base_url: String,
    pub model_id: String,
    client: reqwest::Client,
}

impl WorkerBackend {
    pub fn new(base_url: impl Into<String>, model_id: impl Into<String>) -> Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(120))
            .build()
            .map_err(|e| TtsError::Backend(e.to_string()))?;
        Ok(Self {
            base_url: base_url.into(),
            model_id: model_id.into(),
            client,
        })
    }

    /// Model id the worker in front of us is serving, when it answers.
    ///
    /// Used to reconcile whatever is already listening on the port — a
    /// manually started worker, or an orphan left by a hard kill.
    pub async fn serving_model(&self) -> Result<Option<String>> {
        let reply: HealthReply = self.get_json("/health").await?;
        Ok(reply.model)
    }

    /// Ask a worker to exit. Its process is not ours to kill (it may have
    /// been started by hand), so this is the polite way to reclaim the port.
    pub async fn shutdown(&self) -> Result<()> {
        let url = format!("{}/shutdown", self.base_url);
        let _ = self
            .client
            .post(&url)
            .send()
            .await
            .map_err(|e| TtsError::Backend(e.to_string()))?;
        Ok(())
    }

    async fn get_json<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T> {
        let url = format!("{}{path}", self.base_url);
        let res = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| TtsError::Backend(format!("worker unreachable at {url}: {e}")))?;
        let status = res.status();
        if !status.is_success() {
            let msg = res
                .json::<ErrorReply>()
                .await
                .ok()
                .and_then(|e| e.error)
                .unwrap_or_else(|| format!("http {status}"));
            return Err(TtsError::Backend(msg));
        }
        res.json::<T>()
            .await
            .map_err(|e| TtsError::Backend(e.to_string()))
    }
}

#[async_trait::async_trait]
impl TTSBackend for WorkerBackend {
    fn name(&self) -> &str {
        "worker"
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
        // Loading is lazy server-side (first synthesize) or via --preload.
        self.health().await.map(|_| ())
    }

    async fn unload_model(&mut self) -> Result<()> {
        let url = format!("{}/unload", self.base_url);
        self.client
            .post(&url)
            .send()
            .await
            .map_err(|e| TtsError::Backend(e.to_string()))?;
        Ok(())
    }

    async fn list_voices(&self) -> Result<Vec<Voice>> {
        let reply: VoicesReply = self.get_json("/voices").await?;
        Ok(reply
            .voices
            .into_iter()
            .map(|v| Voice {
                id: v.id.clone(),
                name: v.name,
                model_id: self.model_id.clone(),
                reference_audio: None,
                created_at: chrono::Utc::now(),
            })
            .collect())
    }

    async fn synthesize(&self, req: &crate::SynthesisRequest) -> Result<crate::SynthesisResult> {
        let url = format!("{}/synthesize", self.base_url);
        let body = SynthBody {
            request_id: req.request_id.clone(),
            model: self.model_id.clone(),
            voice: req.voice.clone(),
            text: req.text.clone(),
            speed: req.speed,
        };
        let res = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| TtsError::Backend(format!("worker unreachable: {e}")))?;
        if !res.status().is_success() {
            let msg = res
                .json::<ErrorReply>()
                .await
                .ok()
                .and_then(|e| e.error)
                .unwrap_or_else(|| "synthesis failed".into());
            return Err(TtsError::Synthesis(msg));
        }
        let reply: SynthReply = res
            .json()
            .await
            .map_err(|e| TtsError::Backend(e.to_string()))?;
        Ok(crate::SynthesisResult {
            request_id: reply.request_id,
            audio_path: reply.audio_path,
            duration_secs: reply.duration_secs,
        })
    }

    async fn health(&self) -> Result<bool> {
        let reply: HealthReply = self.get_json("/health").await?;
        Ok(reply.ok)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::path::Path;

    #[test]
    fn voice_stem_matches_server_rule() {
        assert_eq!(voice_stem("kokoro-default"), "af_heart");
        assert_eq!(voice_stem("default"), "af_heart");
        assert_eq!(voice_stem(""), "af_heart");
        assert_eq!(voice_stem("af_bella"), "af_bella");
    }

    #[test]
    fn cache_key_matches_python() {
        // Expected value produced by tts-worker/server.py::cache_key for
        // ("kokoro", "af_heart", "The desert was vast and silent.").
        let (_opus, cands) = worker_cache_candidates(
            Path::new("/tmp"),
            "kokoro",
            "kokoro-default",
            "The  desert   was vast and silent.",
        );
        assert_eq!(
            cands[0].file_name().unwrap().to_str().unwrap(),
            "b35e089db3d5a250031103d89470ac1d.opus"
        );
        // Whitespace normalization must not change the key.
        let (_, c2) = worker_cache_candidates(
            Path::new("/tmp"),
            "kokoro",
            "kokoro-default",
            "The desert was vast and silent.",
        );
        assert_eq!(cands, c2);
        // Both opus + wav candidates share the key.
        assert_eq!(
            cands[1].file_name().unwrap().to_str().unwrap(),
            "b35e089db3d5a250031103d89470ac1d.wav"
        );
    }

    /// Minimal stub worker speaking the real HTTP contract.
    fn stub_server() -> (String, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let handle = std::thread::spawn(move || {
            for stream in listener.incoming().take(3) {
                let mut s = stream.unwrap();
                let mut buf = vec![0u8; 4096];
                let n = s.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);
                let body = if req.contains("POST /synthesize") {
                    r#"{"request_id":"r1","audio_path":"/tmp/r1.opus","duration_secs":2.5,"cached":false}"#
                } else if req.contains("GET /voices") {
                    r#"{"voices":[{"id":"kokoro-default","name":"Default (af_heart)"}]}"#
                } else {
                    r#"{"ok":true,"model":"kokoro","loaded":true}"#
                };
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = s.write_all(resp.as_bytes());
            }
        });
        (url, handle)
    }

    #[tokio::test]
    async fn worker_backend_against_stub() {
        let (url, _h) = stub_server();
        // Give the thread a moment to accept.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let b = WorkerBackend::new(url, "kokoro").unwrap();
        assert!(b.health().await.unwrap());
        let voices = b.list_voices().await.unwrap();
        assert_eq!(voices[0].id, "kokoro-default");
        let res = b
            .synthesize(&crate::SynthesisRequest {
                request_id: "r1".into(),
                model: "kokoro".into(),
                voice: "kokoro-default".into(),
                text: "Hello.".into(),
                speed: 1.0,
                out_path: PathBuf::from("/tmp/r1.opus"),
            })
            .await
            .unwrap();
        assert_eq!(res.audio_path, PathBuf::from("/tmp/r1.opus"));
        assert!((res.duration_secs - 2.5).abs() < 1e-6);
    }
}
