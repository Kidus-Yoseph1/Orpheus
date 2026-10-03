//! Approach B model store: Orpheus owns `models/<id>/` directories.
//!
//! Layout (under e.g. `~/.local/share/orpheus/models/`):
//!
//! ```text
//! models/
//!   kokoro/
//!     orpheus-manifest.json
//!     kokoro-v1_0.pth / config.json / voices/*.bin ...
//!   hexgrad/Kokoro-82M/        # custom ids may nest like HF repos
//!     orpheus-manifest.json
//!     ...
//! ```
//!
//! The manifest is the source of truth for "installed". Downloads are
//! performed by `tts-worker/` straight into the model dir (via
//! `huggingface_hub`), then the manifest is written. The reader only ever
//! reads manifests — it never touches weight files.

use std::path::{Component, Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const MANIFEST_FILE: &str = "orpheus-manifest.json";
pub const MANIFEST_VERSION: u32 = 1;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("invalid model id: {0}")]
    InvalidId(String),
    #[error("io: {0}")]
    Io(String),
    #[error("manifest: {0}")]
    Manifest(String),
}

pub type Result<T> = std::result::Result<T, StoreError>;

/// Written alongside downloaded weights after a pull completes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelManifest {
    pub version: u32,
    pub id: String,
    /// HF repo the files came from, e.g. "hexgrad/Kokoro-82M".
    pub hf_repo: String,
    /// Resolved commit hash (pins `main` at pull time).
    pub revision: String,
    /// Backend family: "kokoro" | "chatterbox" | "chatterbox-turbo" | custom.
    pub backend: String,
    pub files: Vec<String>,
    pub total_bytes: u64,
    pub installed_at: DateTime<Utc>,
}

impl ModelManifest {
    pub fn new(
        id: &str,
        hf_repo: &str,
        revision: &str,
        backend: &str,
        files: Vec<String>,
        total_bytes: u64,
    ) -> Self {
        Self {
            version: MANIFEST_VERSION,
            id: id.to_string(),
            hf_repo: hf_repo.to_string(),
            revision: revision.to_string(),
            backend: backend.to_string(),
            files,
            total_bytes,
            installed_at: Utc::now(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ModelStore {
    models_dir: PathBuf,
}

impl ModelStore {
    pub fn new(models_dir: PathBuf) -> Self {
        Self { models_dir }
    }

    pub fn models_dir(&self) -> &Path {
        &self.models_dir
    }

    /// Resolve `<models>/<id>/`, rejecting traversal and absolute ids.
    /// `org/name` ids nest one level, mirroring HF repos.
    pub fn model_dir(&self, id: &str) -> Result<PathBuf> {
        if id.is_empty() || id.len() > 200 {
            return Err(StoreError::InvalidId(id.to_string()));
        }
        let mut dir = self.models_dir.clone();
        let mut depth = 0;
        for comp in Path::new(id).components() {
            match comp {
                Component::Normal(s) => {
                    depth += 1;
                    if depth > 2 {
                        return Err(StoreError::InvalidId(id.to_string()));
                    }
                    dir.push(s);
                }
                _ => return Err(StoreError::InvalidId(id.to_string())),
            }
        }
        if depth == 0 {
            return Err(StoreError::InvalidId(id.to_string()));
        }
        Ok(dir)
    }

    pub fn manifest_path(&self, id: &str) -> Result<PathBuf> {
        Ok(self.model_dir(id)?.join(MANIFEST_FILE))
    }

    pub fn is_installed(&self, id: &str) -> bool {
        self.manifest_path(id).map(|p| p.is_file()).unwrap_or(false)
    }

    pub fn read_manifest(&self, id: &str) -> Result<ModelManifest> {
        let path = self.manifest_path(id)?;
        let raw = std::fs::read_to_string(&path).map_err(|e| StoreError::Io(e.to_string()))?;
        serde_json::from_str(&raw).map_err(|e| StoreError::Manifest(e.to_string()))
    }

    pub fn write_manifest(&self, manifest: &ModelManifest) -> Result<PathBuf> {
        let dir = self.model_dir(&manifest.id)?;
        std::fs::create_dir_all(&dir).map_err(|e| StoreError::Io(e.to_string()))?;
        let path = dir.join(MANIFEST_FILE);
        let raw = serde_json::to_string_pretty(manifest)
            .map_err(|e| StoreError::Manifest(e.to_string()))?;
        std::fs::write(&path, raw).map_err(|e| StoreError::Io(e.to_string()))?;
        Ok(path)
    }

    /// All installed models: `(id, manifest)`. `id` uses `/` separators
    /// even on Windows (matches HF repo style).
    pub fn scan_installed(&self) -> Vec<(String, ModelManifest)> {
        let mut out = Vec::new();
        scan_level(&self.models_dir, &[], &mut out);
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    pub fn remove(&self, id: &str) -> Result<()> {
        let dir = self.model_dir(id)?;
        if dir.is_dir() {
            std::fs::remove_dir_all(&dir).map_err(|e| StoreError::Io(e.to_string()))?;
        }
        // Prune now-empty parent (for nested org/name ids).
        if let Some(parent) = dir.parent() {
            if parent != self.models_dir.as_path() {
                let _ = std::fs::remove_dir(parent);
            }
        }
        Ok(())
    }

    pub fn dir_size_bytes(&self, id: &str) -> u64 {
        self.model_dir(id).map(|d| dir_size(&d)).unwrap_or(0)
    }
}

fn scan_level(base: &Path, prefix: &[String], out: &mut Vec<(String, ModelManifest)>) {
    // Depth cap: <models>/<id> or <models>/<org>/<name>.
    if prefix.len() > 2 {
        return;
    }
    let manifest = base.join(MANIFEST_FILE);
    if manifest.is_file() {
        if let Ok(raw) = std::fs::read_to_string(&manifest) {
            if let Ok(m) = serde_json::from_str::<ModelManifest>(&raw) {
                out.push((prefix.join("/"), m));
            }
        }
        return;
    }
    if prefix.len() == 2 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(base) else {
        return;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            if let Some(name) = p.file_name().and_then(|s| s.to_str()) {
                if name.starts_with('.') {
                    continue;
                }
                let mut next = prefix.to_vec();
                next.push(name.to_string());
                scan_level(&p, &next, out);
            }
        }
    }
}

fn dir_size(dir: &Path) -> u64 {
    let mut total = 0;
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_file() {
            total += entry.metadata().map(|m| m.len()).unwrap_or(0);
        } else if p.is_dir() {
            total += dir_size(&p);
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_store(tag: &str) -> (PathBuf, ModelStore) {
        let dir = std::env::temp_dir().join(format!("orpheus-store-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        (dir.clone(), ModelStore::new(dir))
    }

    #[test]
    fn manifest_roundtrip_and_scan() {
        let (dir, store) = tmp_store("roundtrip");
        assert!(!store.is_installed("kokoro"));

        let m = ModelManifest::new(
            "kokoro",
            "hexgrad/Kokoro-82M",
            "abc123",
            "kokoro",
            vec!["kokoro-v1_0.pth".into()],
            320_000_000,
        );
        store.write_manifest(&m).unwrap();
        assert!(store.is_installed("kokoro"));

        let back = store.read_manifest("kokoro").unwrap();
        assert_eq!(back.hf_repo, "hexgrad/Kokoro-82M");
        assert_eq!(back.revision, "abc123");

        // Nested custom id.
        let custom = ModelManifest::new(
            "hexgrad/Kokoro-82M",
            "hexgrad/Kokoro-82M",
            "def456",
            "custom",
            vec![],
            0,
        );
        store.write_manifest(&custom).unwrap();

        let ids: Vec<String> = store
            .scan_installed()
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        assert_eq!(ids, vec!["hexgrad/Kokoro-82M", "kokoro"]);

        store.remove("kokoro").unwrap();
        assert!(!store.is_installed("kokoro"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_traversal_ids() {
        let (_dir, store) = tmp_store("traversal");
        assert!(store.model_dir("../evil").is_err());
        assert!(store.model_dir("/abs").is_err());
        assert!(store.model_dir("a/b/c").is_err());
        assert!(store.model_dir("").is_err());
        assert!(store.model_dir("kokoro").is_ok());
        assert!(store.model_dir("org/name").is_ok());
    }

    #[test]
    fn parses_python_writer_manifest() {
        // Contract with tts-worker/download.py: same field names/types.
        let raw = r#"{
            "version": 1,
            "id": "kokoro",
            "hf_repo": "hexgrad/Kokoro-82M",
            "revision": "2e0a3e91a2ff3e0a3d2b8d7a1c9d4e5f6a7b8c9d",
            "backend": "kokoro",
            "files": ["kokoro-v1_0.pth", "config.json"],
            "total_bytes": 322174680,
            "installed_at": "2026-10-01T20:00:00+00:00"
        }"#;
        let m: ModelManifest = serde_json::from_str(raw).unwrap();
        assert_eq!(m.id, "kokoro");
        assert_eq!(m.hf_repo, "hexgrad/Kokoro-82M");
        assert_eq!(m.files.len(), 2);
    }
}
