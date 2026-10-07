//! Supervises the `book-tts` worker process.
//!
//! Why Rust owns it: one worker serves one model, and on a 4 GB card two
//! models cannot stay resident (kokoro ≈ 1.4 GB + chatterbox-turbo ≈ 2.8 GB
//! > VRAM). So switching models means: kill the old process, spawn one for
//! the new model, wait for it to load, keep playing. Spawn is non-blocking;
//! health is polled from the UI tick.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use crate::app::{App, PlaybackState};

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

/// Candidate python interpreters for the worker, best first.
fn python_candidates() -> Vec<PathBuf> {
    let mut v = Vec::new();
    // Repo-local venv created by tts-worker/install.sh (walks up from cwd
    // so it works from anywhere, incl. when launched via `orpheus`).
    let mut dir = std::env::current_dir().ok();
    while let Some(d) = dir {
        let p = d
            .join("tts-worker")
            .join(".venv312")
            .join("bin")
            .join("python");
        if p.is_file() {
            v.push(p);
            break;
        }
        dir = d.parent().map(Path::to_path_buf);
    }
    if let Some(home) = dirs::home_dir() {
        let p = home.join("Kidus/Development/Rust/Orpheus/tts-worker/.venv312/bin/python");
        if p.is_file() {
            v.push(p);
        }
    }
    v.push(PathBuf::from("python3"));
    v
}

/// Locate `tts-worker/server.py`.
fn server_script() -> Option<PathBuf> {
    let mut dir = std::env::current_dir().ok();
    while let Some(d) = dir {
        let p = d.join("tts-worker").join("server.py");
        if p.is_file() {
            return Some(p);
        }
        dir = d.parent().map(Path::to_path_buf);
    }
    None
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
    pub fn ensure(&mut self, model: &str, port: u16, preload: bool) -> bool {
        if !self.is_running() {
            self.running_model = None;
        }
        if self.running_model.as_deref() == Some(model) && self.is_running() {
            return true;
        }
        self.stop(); // wrong model or dead process: release VRAM first
        let Some(script) = server_script() else {
            self.last_error = Some("tts-worker/server.py not found".into());
            return false;
        };
        let mut last_err = None;
        for py in python_candidates() {
            let mut cmd = Command::new(&py);
            cmd.arg(&script)
                .arg("--model")
                .arg(model)
                .arg("--port")
                .arg(port.to_string())
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            if preload {
                cmd.arg("--preload");
            }
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
    let supported = app.worker_supported();
    if !supported {
        app.worker.stop();
        return;
    }
    let url = app.config.tts.worker_url.clone();
    let port = url
        .rsplit(':')
        .next()
        .and_then(|p| p.parse::<u16>().ok())
        .unwrap_or(8765);
    let already =
        app.worker.running_model.as_deref() == Some(model.as_str()) && app.worker.is_running();
    if !already {
        // First play on a given model: spawn, tell the user it's loading.
        if app.worker.ensure(&model, port, true) {
            app.worker_waiting = true;
            app.status_msg = Some(format!("⏳ starting {model}… (first load takes a moment)"));
        } else if let Some(e) = app.worker.last_error.clone() {
            app.status_msg = Some(format!("tts: {e}"));
        }
        return;
    }
    if !app.worker_waiting {
        return;
    }
    // Waiting for /health to answer = model loaded.
    use orpheus_tts::TTSBackend;
    let worker = orpheus_tts::WorkerBackend::new(url, model.clone());
    match worker {
        Ok(w) => {
            let rt = &app.rt;
            let up = rt.block_on(w.health()).unwrap_or(false);
            if up {
                app.worker_waiting = false;
                if let Some(msg) = app.status_msg.clone() {
                    if msg.starts_with('⏳') {
                        app.status_msg = None;
                    }
                }
            }
        }
        Err(e) => {
            app.worker_waiting = false;
            app.playback = PlaybackState::Paused;
            app.buffering = false;
            app.status_msg = Some(format!("tts: {e}"));
        }
    }
}
