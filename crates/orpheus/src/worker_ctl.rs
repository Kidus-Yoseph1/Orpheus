//! Supervises the `book-tts` worker process.
//!
//! Why Rust owns it: one worker serves one model, and on a 4 GB card two
//! models cannot stay resident (kokoro ≈ 1.4 GB + chatterbox-turbo ≈ 2.8 GB
//! > VRAM). So switching models means: kill the old process, spawn one for
//! the new model, wait for it to load, keep playing. Spawn is non-blocking;
//! health is polled from the UI tick.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use crate::app::{App, PlaybackState};

/// User-facing paths: set them in `[tts]` when autodetection can't find the
/// worker (e.g. binary installed, source tree elsewhere).
#[derive(Debug, Clone)]
pub struct WorkerPaths {
    pub script: Option<PathBuf>,
    pub python: Option<PathBuf>,
}

impl Default for WorkerPaths {
    fn default() -> Self {
        Self {
            script: None,
            python: None,
        }
    }
}

#[derive(Debug)]
pub struct WorkerHandle {
    child: Option<Child>,
    /// Model id the running process serves (None = nothing running).
    pub running_model: Option<String>,
    pub last_error: Option<String>,
}

impl Default for WorkerHandle {
    fn default() -> Self {
        Self {
            child: None,
            running_model: None,
            last_error: None,
        }
    }
}

/// Walk up from `start` looking for `rel`. Returns the first hit.
fn walk_up(start: &Path, rel: &str) -> Option<PathBuf> {
    let mut dir = Some(start.to_path_buf());
    while let Some(d) = dir {
        let p = d.join(rel);
        if p.is_file() {
            return Some(p);
        }
        dir = d.parent().map(Path::to_path_buf);
    }
    None
}

fn script_anchors() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        v.push(cwd);
    }
    // `cargo run` / `target/debug/orpheus` sit inside the repo, so this finds
    // the worker even when the reader was launched from another directory.
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            v.push(dir.to_path_buf());
        }
    }
    v
}

fn resolve_script(paths: &WorkerPaths) -> Option<PathBuf> {
    if let Some(p) = &paths.script {
        if p.is_file() {
            return Some(p.clone());
        }
    }
    if let Ok(env) = std::env::var("ORPHEUS_WORKER_SCRIPT") {
        let p = PathBuf::from(env);
        if p.is_file() {
            return Some(p);
        }
    }
    script_anchors()
        .into_iter()
        .find_map(|a| walk_up(&a, "tts-worker/server.py"))
}

fn resolve_python(paths: &WorkerPaths) -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(p) = &paths.python {
        if p.is_file() {
            v.push(p.clone());
        }
    }
    if let Ok(env) = std::env::var("ORPHEUS_WORKER_PYTHON") {
        let p = PathBuf::from(env);
        if p.is_file() {
            v.push(p);
        }
    }
    let venv_rel = "tts-worker/.venv312/bin/python";
    for a in script_anchors() {
        if let Some(p) = walk_up(&a, venv_rel) {
            if !v.contains(&p) {
                v.push(p);
            }
        }
    }
    v.push(PathBuf::from("python3"));
    v
}

/// Worker output goes to a file: with stdout/stderr nulled a crashing worker
/// looked identical to a slow one.
fn log_file(model: &str) -> Option<File> {
    let dir = dirs::data_local_dir()?.join("orpheus").join("logs");
    std::fs::create_dir_all(&dir).ok()?;
    File::create(dir.join(format!("worker-{model}.log"))).ok()
}

impl WorkerHandle {
    pub fn is_running(&mut self) -> bool {
        match self.child.as_mut() {
            Some(c) => match c.try_wait() {
                Ok(None) => true,
                _ => {
                    self.child = None;
                    self.running_model = None;
                    false
                }
            },
            None => false,
        }
    }

    /// Kill the worker (frees VRAM). Safe when nothing runs.
    pub fn stop(&mut self) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
        self.running_model = None;
    }

    /// Ensure a worker serves `model`. Returns true when the process for
    /// this model is (already) running.
    pub fn ensure(&mut self, model: &str, port: u16, paths: &WorkerPaths) -> bool {
        if !self.is_running() {
            self.running_model = None;
        }
        if self.running_model.as_deref() == Some(model) && self.is_running() {
            return true;
        }
        self.stop(); // wrong model or dead process: release VRAM first
        let Some(script) = resolve_script(paths) else {
            self.last_error = Some(
                "tts-worker/server.py not found — set worker_script in [tts] of config.toml".into(),
            );
            return false;
        };
        let mut last_err = None;
        for py in resolve_python(paths) {
            let mut cmd = Command::new(&py);
            cmd.arg(&script)
                .arg("--model")
                .arg(model)
                .arg("--port")
                .arg(port.to_string())
                .arg("--preload")
                .stdin(Stdio::null())
                .stdout(Stdio::null());
            cmd.stderr(match log_file(model) {
                Some(f) => Stdio::from(f),
                None => Stdio::null(),
            });
            match cmd.spawn() {
                Ok(child) => {
                    self.child = Some(child);
                    self.running_model = Some(model.to_string());
                    self.last_error = None;
                    return true;
                }
                Err(e) => last_err = Some(e.to_string()),
            }
        }
        self.last_error = Some(format!(
            "could not start book-tts ({})",
            last_err.unwrap_or_default()
        ));
        false
    }
}

/// Called every tick: keeps a worker alive for the selected model and
/// surfaces load progress. Never blocks.
pub fn supervise(app: &mut App) {
    if !app.config.tts.manage_worker {
        return;
    }
    let model = app.tts.current_model_id.clone();
    if !app.worker_supported() {
        app.worker.stop();
        return;
    }
    let url = app.config.tts.worker_url.clone();
    let port = url
        .rsplit(':')
        .next()
        .and_then(|p| p.parse::<u16>().ok())
        .unwrap_or(8765);
    let paths = app.worker_paths.clone();
    let already =
        app.worker.running_model.as_deref() == Some(model.as_str()) && app.worker.is_running();

    if !already {
        if app.worker.ensure(&model, port, &paths) {
            app.worker_waiting = true;
            app.worker_load_started = std::time::Instant::now();
            app.status_msg = Some(format!("⏳ starting {model}… (first load takes a moment)"));
        } else if let Some(e) = app.worker.last_error.clone() {
            app.worker_waiting = false;
            app.status_msg = Some(format!("tts: {e}"));
        }
        return;
    }
    if !app.worker_waiting {
        return;
    }

    // Time-box the wait: a worker that never answers has failed, and the user
    // deserves an error instead of a permanent "⏳".
    let waited = app.worker_load_started.elapsed().as_secs();
    let timeout = if model.starts_with("chatterbox") {
        180
    } else {
        90
    };
    if waited > timeout {
        app.worker.stop();
        app.worker_waiting = false;
        app.playback = PlaybackState::Paused;
        app.buffering = false;
        app.status_msg = Some(format!(
            "tts: {model} failed to load — see logs/worker-{model}.log"
        ));
        return;
    }

    use orpheus_tts::TTSBackend;
    let Ok(worker) = orpheus_tts::WorkerBackend::new(url, model.clone()) else {
        app.worker_waiting = false;
        return;
    };
    let up = app.rt.block_on(worker.health()).unwrap_or(false);
    if up {
        app.worker_waiting = false;
        if let Some(msg) = app.status_msg.clone() {
            if msg.starts_with('⏳') {
                app.status_msg = None;
            }
        }
    } else if !app.worker.is_running() {
        // Process vanished (crash on load): report, stop looping.
        app.worker_waiting = false;
        app.playback = PlaybackState::Paused;
        app.buffering = false;
        app.status_msg = Some(format!(
            "tts: {model} crashed — see logs/worker-{model}.log"
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_found_from_repo_root() {
        let here = std::env::current_dir().unwrap();
        assert!(walk_up(&here, "tts-worker/server.py").is_some());
    }

    #[test]
    fn explicit_config_paths_win() {
        let paths = WorkerPaths {
            script: Some(PathBuf::from("tts-worker/server.py")),
            python: Some(PathBuf::from("tts-worker/.venv312/bin/python")),
        };
        let s = resolve_script(&paths).expect("config path resolves");
        assert!(s.ends_with("tts-worker/server.py"));
        assert!(!resolve_python(&paths).is_empty());
    }

    #[test]
    fn missing_script_reports_config_hint() {
        let paths = WorkerPaths {
            script: Some(PathBuf::from("/definitely/not/here.py")),
            python: None,
        };
        // Fails only when no autodetect anchor can see the tree either.
        if resolve_script(&paths).is_none() {
            let mut h = WorkerHandle::default();
            assert!(!h.ensure("kokoro", 9, &paths));
            let err = h.last_error.clone().unwrap_or_default();
            assert!(err.contains("worker_script"), "got: {err}");
        }
    }
}
