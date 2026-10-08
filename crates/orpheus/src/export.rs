//! Whole-book audio export: render every sentence once (reusing the Opus
//! cache), then join the clips into a single file with ffmpeg.
//!
//! Rendering runs on a background thread so the reader stays responsive; the
//! UI polls a channel and shows progress. Cancel is cooperative — cached
//! sentences survive, so restarting a cancelled export is cheap.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use orpheus_core::Config;
use orpheus_tts::WorkerBackend;

use crate::app::synthesize_one;

#[derive(Debug)]
pub enum Event {
    Ready,
    Progress { done: usize, total: usize },
    Joining,
    Finished(PathBuf),
    Failed(String),
    Cancelled,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Phase {
    Starting,
    Rendering,
    Joining,
    Done(PathBuf),
    Failed(String),
    Cancelled,
}

impl Phase {
    pub fn is_active(&self) -> bool {
        matches!(self, Phase::Starting | Phase::Rendering | Phase::Joining)
    }
}

pub struct ExportJob {
    pub total: usize,
    pub done: usize,
    pub phase: Phase,
    cancel: Arc<AtomicBool>,
    rx: Receiver<Event>,
}

impl ExportJob {
    pub fn start(
        texts: Vec<String>,
        url: String,
        model: String,
        voice: String,
        out: PathBuf,
    ) -> Result<Self, String> {
        if texts.is_empty() {
            return Err("nothing to export".into());
        }
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let job = ExportJob {
            total: texts.len(),
            done: 0,
            phase: Phase::Starting,
            cancel: cancel.clone(),
            rx,
        };
        std::thread::spawn(move || run(texts, url, model, voice, out, tx, cancel));
        Ok(job)
    }

    /// Drain everything the worker thread reported since the last tick.
    pub fn poll(&mut self) {
        while let Ok(ev) = self.rx.try_recv() {
            match ev {
                Event::Ready => self.phase = Phase::Rendering,
                Event::Progress { done, total } => {
                    self.done = done;
                    self.total = total;
                    self.phase = Phase::Rendering;
                }
                Event::Joining => self.phase = Phase::Joining,
                Event::Finished(path) => self.phase = Phase::Done(path),
                Event::Failed(e) => self.phase = Phase::Failed(e),
                Event::Cancelled => self.phase = Phase::Cancelled,
            }
        }
    }

    pub fn request_cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// `~/.local/share/orpheus/exports/<book>-<voice>-<model>.m4a`
pub fn export_path(title: &str, voice: &str, model: &str) -> PathBuf {
    let slug = |s: &str| {
        let v = crate::app::slugify(s);
        if v.is_empty() {
            "book".to_string()
        } else {
            v
        }
    };
    let name = format!("{}-{}-{}.m4a", slug(title), slug(voice), slug(model));
    Config::exports_dir().join(name)
}

/// Join clips with ffmpeg's concat demuxer into one `m4a`.
///
/// All clips come from the same worker (Opus, mono, 48 kHz), which is what
/// the concat demuxer requires.
pub fn concat(paths: &[PathBuf], out: &Path) -> std::result::Result<(), String> {
    if paths.is_empty() {
        return Err("no audio to join".into());
    }
    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    if paths.len() == 1 {
        std::fs::copy(&paths[0], out).map_err(|e| e.to_string())?;
        return Ok(());
    }
    let list_path = out.with_extension("concat.txt");
    let mut list = String::new();
    for p in paths {
        let escaped = p.to_string_lossy().replace('\'', "'\\''");
        list.push_str(&format!("file '{escaped}'\n"));
    }
    std::fs::write(&list_path, list).map_err(|e| e.to_string())?;

    let out_str = out.to_string_lossy().to_string();
    let res = std::process::Command::new("ffmpeg")
        .args([
            "-y",
            "-loglevel",
            "error",
            "-f",
            "concat",
            "-safe",
            "0",
            "-i",
            &list_path.to_string_lossy(),
            "-c:a",
            "aac",
            "-b:a",
            "96k",
            "-movflags",
            "+faststart",
            &out_str,
        ])
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|_| "ffmpeg not found — install ffmpeg to export".to_string())?;
    let _ = std::fs::remove_file(&list_path);

    if res.status.success() && out.is_file() {
        Ok(())
    } else {
        let err = String::from_utf8_lossy(&res.stderr).trim().to_string();
        Err(if err.is_empty() {
            "ffmpeg could not join the clips".into()
        } else {
            format!("ffmpeg: {err}")
        })
    }
}

fn run(
    texts: Vec<String>,
    url: String,
    model: String,
    voice: String,
    out: PathBuf,
    tx: Sender<Event>,
    cancel: Arc<AtomicBool>,
) {
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            let _ = tx.send(Event::Failed(e.to_string()));
            return;
        }
    };

    // The worker loads on its own tick (32 s for chatterbox), and a foreign
    // one may sit on the port until supervision evicts it: wait until
    // something answers *and* claims our model, never just any /health.
    let worker = match WorkerBackend::new(url.clone(), model.clone()) {
        Ok(w) => w,
        Err(e) => {
            let _ = tx.send(Event::Failed(e.to_string()));
            return;
        }
    };
    let deadline = Instant::now() + Duration::from_secs(180);
    loop {
        if cancel.load(Ordering::Relaxed) {
            let _ = tx.send(Event::Cancelled);
            return;
        }
        let ours = rt
            .block_on(worker.serving_model())
            .ok()
            .flatten()
            .map(|m| m == model)
            .unwrap_or(false);
        if ours {
            break;
        }
        if Instant::now() > deadline {
            let _ = tx.send(Event::Failed(format!(
                "{model} never became ready — see logs/worker-{model}.log"
            )));
            return;
        }
        std::thread::sleep(Duration::from_millis(800));
    }
    let _ = tx.send(Event::Ready);

    let cache = Config::audio_cache_dir();
    let total = texts.len();
    let mut clips: Vec<PathBuf> = Vec::with_capacity(total);
    for (i, text) in texts.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            let _ = tx.send(Event::Cancelled);
            return;
        }
        match synthesize_with_retry(&rt, &cache, &url, &model, &voice, text, &cancel) {
            Ok(Some(path)) => clips.push(path),
            Ok(None) => {
                let _ = tx.send(Event::Cancelled);
                return;
            }
            Err(e) => {
                let _ = tx.send(Event::Failed(format!("sentence {}: {e}", i + 1)));
                return;
            }
        }
        let _ = tx.send(Event::Progress { done: i + 1, total });
    }

    if cancel.load(Ordering::Relaxed) {
        let _ = tx.send(Event::Cancelled);
        return;
    }
    let _ = tx.send(Event::Joining);
    match concat(&clips, &out) {
        Ok(()) => {
            let _ = tx.send(Event::Finished(out));
        }
        Err(e) => {
            let _ = tx.send(Event::Failed(e));
        }
    }
}

/// A worker restarting mid-export (model switch, crash, eviction) drops the
/// connection for a couple of seconds — retry those instead of failing the
/// whole book. `Ok(None)` means the caller cancelled while waiting.
fn synthesize_with_retry(
    rt: &tokio::runtime::Runtime,
    cache: &Path,
    url: &str,
    model: &str,
    voice: &str,
    text: &str,
    cancel: &Arc<AtomicBool>,
) -> std::result::Result<Option<PathBuf>, String> {
    let mut attempt = 0;
    loop {
        match synthesize_one(rt, cache, url, model, voice, text) {
            Ok(path) => return Ok(Some(path)),
            Err(e) if attempt < 4 && transient(&e) => {
                attempt += 1;
                for _ in 0..8 {
                    if cancel.load(Ordering::Relaxed) {
                        return Ok(None);
                    }
                    std::thread::sleep(Duration::from_millis(250));
                }
            }
            Err(e) => return Err(e),
        }
    }
}

fn transient(err: &str) -> bool {
    err.contains("unreachable")
        || err.contains("error sending request")
        || err.contains("connection refused")
        || err.contains("worker never")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("orpheus-export-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn export_path_is_stable_and_filesystem_safe() {
        let p = export_path(
            "The Beginning of Infinity (David Deutsch)",
            "clarke",
            "chatterbox-turbo",
        );
        let name = p.file_name().unwrap().to_string_lossy().to_string();
        assert_eq!(
            name,
            "the-beginning-of-infinity-david-deutsch-clarke-chatterbox-turbo.m4a"
        );
        assert!(p.starts_with(Config::exports_dir()));
        // Same inputs, same path: a rerun overwrites instead of accumulating.
        assert_eq!(
            p,
            export_path(
                "The Beginning of Infinity (David Deutsch)",
                "clarke",
                "chatterbox-turbo"
            )
        );
    }

    #[test]
    fn export_path_falls_back_when_title_is_all_punctuation() {
        let p = export_path("!!!", "", "");
        assert!(p.file_name().unwrap().to_string_lossy().contains("book"));
    }

    #[test]
    fn concat_rejects_empty_input() {
        let dir = tmp("empty");
        let err = concat(&[], &dir.join("out.m4a")).unwrap_err();
        assert!(err.contains("no audio"), "got: {err}");
    }

    #[test]
    fn concat_joins_clips_into_one_m4a() {
        let dir = tmp("join");
        // Three tiny Opus clips, exactly what the worker produces.
        let mut clips = Vec::new();
        for (i, hz) in [440.0, 550.0, 660.0].into_iter().enumerate() {
            let clip = dir.join(format!("s{i}.opus"));
            let st = std::process::Command::new("ffmpeg")
                .args([
                    "-v",
                    "error",
                    "-f",
                    "lavfi",
                    "-i",
                    &format!("sine=frequency={hz}:duration=0.4"),
                    "-c:a",
                    "libopus",
                    "-ar",
                    "48000",
                    "-ac",
                    "1",
                    &clip.to_string_lossy(),
                    "-y",
                ])
                .status()
                .expect("ffmpeg available for this test");
            assert!(st.success(), "could not synthesise a test clip");
            clips.push(clip);
        }
        let out = dir.join("book.m4a");
        concat(&clips, &out).expect("join succeeds");
        assert!(out.is_file());
        assert!(out.metadata().unwrap().len() > 1000, "output is not empty");
        // Exactly one file written: the helper cleans up its list file.
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter(|e| e.path().extension().map(|x| x == "txt").unwrap_or(false))
            .collect();
        assert!(leftovers.is_empty(), "concat list left behind");
    }

    #[test]
    fn phase_is_active_only_while_running() {
        assert!(Phase::Rendering.is_active());
        assert!(Phase::Joining.is_active());
        assert!(!Phase::Cancelled.is_active());
        assert!(!Phase::Failed("x".into()).is_active());
        assert!(!Phase::Done(PathBuf::from("/x")).is_active());
    }

    #[test]
    fn transient_errors_are_the_retriable_ones() {
        assert!(transient(
            "backend error: worker unreachable: error sending request"
        ));
        assert!(transient("worker unreachable: connection refused"));
        assert!(!transient(
            "chatterbox has no built-in voices: add a sample"
        ));
        assert!(!transient("worker serves 'kokoro', got 'chatterbox-turbo'"));
        assert!(!transient("empty text"));
    }

    #[test]
    fn cancelled_job_reports_cancelled() {
        let (tx, rx) = channel();
        tx.send(Event::Cancelled).unwrap();
        let mut job = ExportJob {
            total: 3,
            done: 1,
            phase: Phase::Rendering,
            cancel: Arc::new(AtomicBool::new(false)),
            rx,
        };
        job.poll();
        assert_eq!(job.phase, Phase::Cancelled);
        assert!(!job.phase.is_active());
    }
}
