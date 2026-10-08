//! Local audio playback via ffplay, with true pause (SIGSTOP/SIGCONT).
//! Mirrors the TUI's player so speed stays a playback concern.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

pub struct Player {
    bin: PathBuf,
    child: Option<Child>,
    paused: bool,
}

impl Player {
    pub fn new() -> Self {
        Self {
            bin: PathBuf::from("ffplay"),
            child: None,
            paused: false,
        }
    }

    pub fn available(&self) -> bool {
        if self.bin.is_absolute() && self.bin.is_file() {
            return true;
        }
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
            .any(|d| d.join(&self.bin).is_file())
    }

    pub fn play(&mut self, path: &Path, speed: f32) -> anyhow::Result<()> {
        self.stop();
        let af = format!("atempo={:.2}", speed.clamp(0.5, 100.0));
        let child = Command::new(&self.bin)
            .args([
                "-nodisp",
                "-autoexit",
                "-loglevel",
                "quiet",
                "-af",
                &af,
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
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
        self.paused = false;
    }

    pub fn pause(&mut self) {
        let Some(c) = self.child.as_ref() else { return };
        let pid = c.id() as i32;
        unsafe {
            libc::kill(pid, libc::SIGSTOP);
        }
        self.paused = true;
    }

    /// Resume a frozen child in place. False if there is nothing resumable.
    pub fn resume(&mut self) -> bool {
        if self.paused {
            if let Some(c) = self.child.as_ref() {
                let pid = c.id() as i32;
                unsafe {
                    libc::kill(pid, libc::SIGCONT);
                }
                self.paused = false;
                if self.is_playing() {
                    return true;
                }
            }
        }
        self.stop();
        false
    }

    pub fn is_playing(&mut self) -> bool {
        match self.child.as_mut() {
            Some(c) => match c.try_wait() {
                Ok(None) => true,
                _ => {
                    self.child = None;
                    self.paused = false;
                    false
                }
            },
            None => false,
        }
    }
}

impl Default for Player {
    fn default() -> Self {
        Self::new()
    }
}
