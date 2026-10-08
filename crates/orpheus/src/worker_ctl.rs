//! Supervises the `book-tts` worker process.
//!
//! Why Rust owns it: one worker serves one model, and on a 4 GB card two
//! models cannot stay resident (kokoro ≈ 1.4 GB + chatterbox-turbo ≈ 2.8 GB
//! > VRAM). So switching models means: stop the old worker, spawn one for
//! the new model, wait for it to load, keep playing.
//!
//! The port is shared state: a worker started by hand — or orphaned by a
//! hard kill — is already listening. Before spawning, we ask what it serves
//! and either adopt it (same model: skip a 32 s reload) or tell it to exit
//! so the right one can take the port.

use std::fs::File;
use std::net::TcpStream;
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
    /// Model id the running process serves (None = nothing of ours runs).
    pub running_model: Option<String>,
    /// True when we did not spawn it — a pre-existing worker we took over.
    adopted: bool,
    adopted_url: Option<String>,
    /// Foreign worker we asked to leave but that hasn't (no /shutdown, etc).
    evict_attempts: u8,
    pub last_error: Option<String>,
}

impl Default for WorkerHandle {
    fn default() -> Self {
        Self {
            child: None,
            running_model: None,
            adopted: false,
            adopted_url: None,
            evict_attempts: 0,
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

fn port_busy(port: u16) -> bool {
    TcpStream::connect(("127.0.0.1", port)).is_ok()
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

    pub fn owns_model(&mut self, model: &str) -> bool {
        if self.running_model.as_deref() != Some(model) {
            return false;
        }
        if self.adopted {
            return true;
        }
        self.is_running()
    }

    /// Take over a worker we didn't start (it already serves our model).
    pub fn adopt(&mut self, model: String, url: String) {
        self.child = None;
        self.running_model = Some(model);
        self.adopted = true;
        self.adopted_url = Some(url);
        self.evict_attempts = 0;
        self.last_error = None;
    }

    /// Forget a worker that went away (or that we asked to leave).
    pub fn disown(&mut self) {
        self.child = None;
        self.running_model = None;
        self.adopted = false;
        self.adopted_url = None;
    }

    /// Kill our own child (frees VRAM). Safe when nothing runs.
    pub fn stop(&mut self) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
        self.disown();
    }

    /// Stop whatever we are responsible for: adopted workers are asked over
    /// HTTP (not ours to kill), our own child is killed outright.
    pub fn shutdown(&mut self, rt: &tokio::runtime::Runtime) {
        if let Some(url) = self.adopted_url.clone() {
            if let Ok(w) = orpheus_tts::WorkerBackend::new(url, String::new()) {
                let _ = rt.block_on(w.shutdown());
            }
            self.disown();
            return;
        }
        self.stop();
    }

    /// Ensure a worker serves `model`. Returns true when the process for
    /// this model is (already) running.
    pub fn ensure(&mut self, model: &str, port: u16, paths: &WorkerPaths) -> bool {
        if !self.is_running() && !self.adopted {
            self.running_model = None;
        }
        if self.owns_model(model) {
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
                    self.adopted = false;
                    self.adopted_url = None;
                    self.evict_attempts = 0;
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

/// Model id answering on `url`, or `Err` when nothing replies.
pub fn serving_model(
    rt: &tokio::runtime::Runtime,
    url: &str,
) -> std::result::Result<Option<String>, ()> {
    let w = orpheus_tts::WorkerBackend::new(url, String::new()).map_err(|_| ())?;
    rt.block_on(w.serving_model()).map_err(|_| ())
}

fn evict(rt: &tokio::runtime::Runtime, url: &str) {
    if let Ok(w) = orpheus_tts::WorkerBackend::new(url, String::new()) {
        let _ = rt.block_on(w.shutdown());
    }
}

/// Called every tick: keeps one worker alive for the selected model and
/// surfaces load progress. Never blocks for long.
pub fn supervise(app: &mut App) {
    if !app.config.tts.manage_worker {
        return;
    }
    let model = app.tts.current_model_id.clone();
    if !app.worker_supported() {
        app.worker.shutdown(&app.rt);
        return;
    }
    let url = app.config.tts.worker_url.clone();
    let port = url
        .rsplit(':')
        .next()
        .and_then(|p| p.parse::<u16>().ok())
        .unwrap_or(8765);
    let paths = app.worker_paths.clone();

    if app.worker.owns_model(&model) {
        if !app.worker_waiting {
            return;
        }
        wait_until_ready(app, &model, &url);
        return;
    }

    // Reconcile whatever is already on the port before spawning.
    match serving_model(&app.rt, &url) {
        Ok(Some(serving)) if serving == model => {
            app.worker.adopt(model.clone(), url);
            app.worker_waiting = false;
            app.worker_evict_attempts = 0;
            app.status_msg = Some(format!("{model} worker already running — adopted"));
            return;
        }
        Ok(Some(serving)) => {
            if app.worker_evict_attempts >= 6 {
                // No /shutdown route (older worker) or it refuses to die.
                app.status_msg = Some(format!(
                    "tts: a {serving} worker holds the port and won't stop — kill it manually"
                ));
                return;
            }
            app.worker_evict_attempts += 1;
            evict(&app.rt, &url);
            app.status_msg = Some(format!("releasing the port from {serving}…"));
            return; // next tick spawns ours
        }
        Ok(None) if port_busy(port) => {
            app.status_msg = Some(format!("waiting for book-tts on port {port}…"));
            return;
        }
        Err(()) if port_busy(port) => {
            app.status_msg = Some(format!("waiting for book-tts on port {port}…"));
            return;
        }
        _ => {}
    }

    if app.worker.ensure(&model, port, &paths) {
        app.worker_waiting = true;
        app.worker_load_started = std::time::Instant::now();
        app.status_msg = Some(format!("⏳ starting {model}… (first load takes a moment)"));
    } else if let Some(e) = app.worker.last_error.clone() {
        app.worker_waiting = false;
        app.status_msg = Some(format!("tts: {e}"));
    }
}

/// Poll `/health` while our own worker loads; fail loudly instead of showing
/// `⏳` forever.
fn wait_until_ready(app: &mut App, model: &str, url: &str) {
    if !app.worker_waiting {
        return;
    }
    let waited = app.worker_load_started.elapsed().as_secs();
    let timeout = if model.starts_with("chatterbox") {
        180
    } else {
        90
    };
    if waited > timeout {
        app.worker.shutdown(&app.rt);
        app.worker_waiting = false;
        app.playback = PlaybackState::Paused;
        app.buffering = false;
        app.status_msg = Some(format!(
            "tts: {model} failed to load — see logs/worker-{model}.log"
        ));
        return;
    }

    use orpheus_tts::TTSBackend;
    let Ok(worker) = orpheus_tts::WorkerBackend::new(url.to_string(), model.to_string()) else {
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
        return;
    }
    if app.worker.adopted {
        // It answered before and went away: drop ownership, respawn.
        app.worker.disown();
        app.status_msg = Some(format!("tts: {model} worker disappeared"));
        return;
    }
    if !app.worker.is_running() {
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

    #[test]
    fn adopted_worker_counts_as_owned() {
        let mut h = WorkerHandle::default();
        h.adopt("chatterbox-turbo".into(), "http://127.0.0.1:8765".into());
        assert!(h.owns_model("chatterbox-turbo"));
        assert!(!h.owns_model("kokoro"));
        h.disown();
        assert!(!h.owns_model("chatterbox-turbo"));
    }

    #[test]
    fn port_probe_is_safe_when_nothing_listens() {
        // Port 1 is never open; this only asserts it doesn't panic.
        assert!(!port_busy(1));
    }
}
