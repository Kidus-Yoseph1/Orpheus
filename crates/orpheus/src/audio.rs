//! Local audio playback via ffplay (ships with ffmpeg, no mpv needed).
//!
//! Speed is applied at playback time (`-af atempo=`), so synthesis always
//! renders 1x and the opus cache stays speed-independent.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use anyhow::Result;

pub struct FfplayPlayer {
    bin: PathBuf,
    child: Option<Child>,
    paused: bool,
}

impl FfplayPlayer {
    pub fn new() -> Self {
        Self {
            bin: PathBuf::from("ffplay"),
            child: None,
            paused: false,
        }
    }

    #[allow(dead_code)]
    pub fn with_bin(bin: impl Into<PathBuf>) -> Self {
        Self {
            bin: bin.into(),
            child: None,
            paused: false,
        }
    }

    pub fn available(&self) -> bool {
        self.bin.is_absolute() && self.bin.is_file()
            || std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
                .any(|d| d.join(&self.bin).is_file())
    }

    pub fn atempo_arg(speed: f32) -> String {
        // atempo accepts 0.5..=100 in a single filter on modern ffmpeg.
        format!("atempo={:.2}", speed.clamp(0.5, 100.0))
    }

    /// Play a file, stopping whatever is current. Non-blocking.
    pub fn play(&mut self, path: &Path, speed: f32) -> Result<()> {
        self.stop();
        if !self.available() {
            anyhow::bail!("ffplay not found — install ffmpeg for narration audio");
        }
        let child = Command::new(&self.bin)
            .args([
                "-nodisp",
                "-autoexit",
                "-loglevel",
                "quiet",
                "-af",
                &Self::atempo_arg(speed),
                &path.to_string_lossy(),
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        self.child = Some(child);
        self.paused = false;
        Ok(())
    }

    pub fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.paused = false;
    }

    /// True pause: freeze ffplay mid-sentence, resume continues exactly
    /// where it stopped (no re-synthesis, no restart from the top).
    pub fn pause(&mut self) {
        if self.signal("STOP") {
            self.paused = true;
        } else {
            self.stop();
        }
    }

    /// Resume a frozen child in place. Returns false when there is nothing
    /// resumable (caller should re-buffer from the cursor instead).
    pub fn resume(&mut self) -> bool {
        if self.paused && self.child.is_some() && self.signal("CONT") {
            self.paused = false;
            // Child may have exited between STOP and CONT; verify liveness.
            if self.is_playing() {
                return true;
            }
        }
        self.stop();
        false
    }

    #[allow(dead_code)]
    pub fn has_child(&self) -> bool {
        self.child.is_some()
    }

    fn signal(&mut self, sig: &str) -> bool {
        let Some(child) = self.child.as_mut() else {
            return false;
        };
        // Reap first: no point signalling a dead child.
        if let Ok(Some(_)) = child.try_wait() {
            self.child = None;
            self.paused = false;
            return false;
        }
        Command::new("kill")
            .arg(format!("-{sig}"))
            .arg(child.id().to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    /// True while audio is still playing. Reaps finished children.
    pub fn is_playing(&mut self) -> bool {
        match self.child.as_mut() {
            Some(child) => match child.try_wait() {
                Ok(None) => true,
                _ => {
                    self.child = None;
                    false
                }
            },
            None => false,
        }
    }
}

impl Default for FfplayPlayer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atempo_formats_and_clamps() {
        assert_eq!(FfplayPlayer::atempo_arg(1.0), "atempo=1.00");
        assert_eq!(FfplayPlayer::atempo_arg(1.256), "atempo=1.26");
        assert_eq!(FfplayPlayer::atempo_arg(0.1), "atempo=0.50");
    }

    #[test]
    fn missing_binary_errors_instead_of_spawning() {
        let mut p = FfplayPlayer::with_bin("/nonexistent-ffplay-binary");
        assert!(!p.available());
        assert!(p.play(Path::new("/tmp/x.opus"), 1.0).is_err());
        assert!(!p.is_playing());
        p.pause(); // fallback stop, must not panic
        assert!(!p.resume());
        assert!(!p.has_child());
        p.stop(); // no-op, must not panic
    }

    /// True pause round-trip against real ffplay (skipped without ffmpeg).
    #[test]
    fn pause_freezes_and_resume_continues() {
        let probe = FfplayPlayer::new();
        if !probe.available() || which("ffmpeg").is_none() {
            return;
        }
        let wav = std::env::temp_dir().join("orpheus-pause-test.wav");
        let mk = Command::new("ffmpeg")
            .args([
                "-y",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=4",
                "-ar",
                "24000",
                "-ac",
                "1",
            ])
            .arg(&wav)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        if mk.map(|s| s.success()).unwrap_or(false) && wav.is_file() {
            let mut p = FfplayPlayer::new();
            p.play(&wav, 1.0).expect("ffplay starts");
            std::thread::sleep(std::time::Duration::from_millis(400));
            assert!(p.is_playing());
            p.pause();
            assert!(p.has_child(), "frozen child is kept for resume");
            std::thread::sleep(std::time::Duration::from_millis(200));
            assert!(p.resume(), "resume continues the same child");
            assert!(p.is_playing());
            p.stop();
            assert!(!p.has_child());
            let _ = std::fs::remove_file(&wav);
        }
    }

    fn which(bin: &str) -> Option<PathBuf> {
        std::env::split_paths(&std::env::var_os("PATH")?)
            .map(|d| d.join(bin))
            .find(|p| p.is_file())
    }
}
