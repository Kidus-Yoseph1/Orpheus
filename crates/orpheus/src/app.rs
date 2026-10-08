//! GUIDE §31 — central AppState + screen navigation.
//! No app state lives inside render functions.

use std::path::{Path, PathBuf};

use anyhow::Result;
use orpheus_core::{Config, Document, LibraryDb, ReaderStyle, Theme};
use orpheus_tts::{worker_cache_candidates, BackendKind, ModelStore, TtsManager, WorkerBackend};

use crate::audio::FfplayPlayer;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Home,
    Directory,
    Reader,
    Models,
    Voices,
    VoiceEditor,
    Appearance,
    Help,
    Search,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackState {
    Stopped,
    Playing,
    Paused,
}

pub struct App {
    pub config: Config,
    pub theme: Theme,
    pub style: ReaderStyle,
    pub screen: Screen,
    pub prev_screen: Screen,
    pub should_quit: bool,
    pub status_msg: Option<String>,

    pub db: LibraryDb,
    pub tts: TtsManager,

    // Home
    pub recent: Vec<orpheus_core::db::BookRow>,
    pub home_selected: usize,

    // Directory mode
    pub dir_path: Option<PathBuf>,
    pub dir_entries: Vec<orpheus_core::library::FoundBook>,
    pub dir_selected: usize,

    // Reader
    pub book: Option<Document>,
    /// Global sentence index across the whole book (resume unit).
    pub sentence_cursor: usize,
    pub chapter_idx: usize,
    pub scroll: u16,
    pub playback: PlaybackState,
    pub speed: f32,

    // Reader rendering cache (kept for future TTS queue/word-timing; body now
    // renders directly from book blocks for flowing paragraphs).
    #[allow(dead_code)]
    pub chapter_lines: Vec<Line>,
    pub search_query: String,
    pub searching: bool,

    // Models screen
    pub model_selected: usize,
    pub model_input: String,
    pub model_typing: bool,

    // Appearance screen: 0 = themes section, 1 = styles section
    pub appearance_section: usize,
    pub theme_selected: usize,
    pub style_selected: usize,

    // Narration (real audio via worker + ffplay; mock timer otherwise)
    pub player: FfplayPlayer,
    pub rt: tokio::runtime::Runtime,
    pub audio_cache_dir: PathBuf,
    pub buffering: bool,
    pub mock_tick: u8,
    /// book-tts process supervision (spawn/kill per model).
    pub worker: crate::worker_ctl::WorkerHandle,
    /// Worker spawned but not yet answering /health (model still loading).
    pub worker_waiting: bool,
    /// When the current worker spawn happened (load timeout budget).
    pub worker_load_started: std::time::Instant,
    /// Ticks spent asking a foreign worker on our port to leave.
    pub worker_evict_attempts: u8,
    pub worker_paths: crate::worker_ctl::WorkerPaths,
    /// Whole-book export job (render every sentence, then join).
    pub export: Option<crate::export::ExportJob>,
    /// Sample length per reference path, keyed by (path, mtime): ffprobe is
    /// ~30ms and the picker lists every clone, so probe once per file.
    voice_dur_cache: std::collections::HashMap<String, (std::time::SystemTime, f64)>,

    // Voices screen
    pub model_store: ModelStore,
    pub voice_list: Vec<VoiceEntry>,
    pub voice_selected: usize,

    // Voice editor (add-voice form)
    pub ve_name: String,
    pub ve_path: String,
    pub ve_field: usize, // 0 = name, 1 = sample file
    /// Validated save waiting for a tick: set on Enter (after instant
    /// checks), consumed by tick_voice_save so the "normalizing…" message
    /// paints BEFORE ffmpeg blocks the loop. Keys never freeze; Esc while
    /// busy cancels back to the list on completion.
    pub pending_voice_save: Option<PendingVoiceSave>,
}

/// Everything save_voice needs after the fast checks passed.
#[derive(Debug, Clone)]
pub struct PendingVoiceSave {
    pub id: String,
    pub name: String,
    pub src: PathBuf,
    pub dest: PathBuf,
}

/// One row in the Voices screen. `note` explains provenance.
#[derive(Debug, Clone)]
pub struct VoiceEntry {
    pub id: String,
    pub note: String,
    pub custom: bool,
}

/// Build the voice list for a model from the on-disk store (no network):
/// `<models>/<id>/voices/*.pt` stems, default first. Falls back to the
/// current voice id alone when nothing is pulled yet.
pub fn voices_for_model(
    store: &ModelStore,
    model_id: &str,
    current_voice: &str,
) -> (Vec<VoiceEntry>, usize) {
    let mut ids: Vec<String> = Vec::new();
    if let Ok(dir) = store.model_dir(model_id) {
        if let Ok(entries) = std::fs::read_dir(dir.join("voices")) {
            let mut stems: Vec<String> = entries
                .flatten()
                .filter_map(|e| {
                    let p = e.path();
                    if p.extension().and_then(|x| x.to_str()) != Some("pt") {
                        return None;
                    }
                    p.file_stem().and_then(|s| s.to_str()).map(str::to_string)
                })
                .collect();
            stems.sort();
            let def = orpheus_tts::voice_stem_default(model_id);
            ids.push(def.clone());
            for s in stems {
                if s != def && !ids.contains(&s) {
                    ids.push(s);
                }
            }
        }
    }
    if ids.is_empty() && current_voice.is_empty() {
        // No store voices and nothing selected (e.g. mock): show a placeholder
        // so the list isn't mysteriously empty.
        ids.push(orpheus_tts::voice_stem_default(model_id));
    } else if !current_voice.is_empty() && !ids.contains(&current_voice.to_string()) {
        // Always keep the selected voice visible, even if it is a clone and
        // the model ships no built-in stems (chatterbox).
        ids.push(current_voice.to_string());
    }
    let def = orpheus_tts::voice_stem_default(model_id);
    let entries = ids
        .into_iter()
        .map(|id| VoiceEntry {
            note: if id == def {
                "default".into()
            } else {
                "built-in".into()
            },
            custom: false,
            id,
        })
        .collect::<Vec<_>>();
    let sel = entries
        .iter()
        .position(|v| v.id == current_voice)
        .unwrap_or(0);
    (entries, sel)
}

/// "Sarah the Narrator" -> "sarah-the-narrator".
pub fn slugify(name: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for c in name.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
            dash = false;
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

/// Resolve what the user typed: bare filenames look in `~/voices/` first,
/// otherwise absolute/~/expanded paths. Must be an existing audio file.
pub fn resolve_sample_path(input: &str) -> std::result::Result<PathBuf, String> {
    let t = input.trim();
    if t.is_empty() {
        return Err("point at a sample file (wav/mp3/flac/m4a)".into());
    }
    let expanded = if let Some(rest) = t.strip_prefix("~/") {
        dirs::home_dir()
            .map(|h| h.join(rest))
            .unwrap_or_else(|| PathBuf::from(t))
    } else {
        PathBuf::from(t)
    };
    let candidates = if expanded.is_absolute() {
        vec![expanded]
    } else {
        let mut v = Vec::new();
        if let Some(home) = dirs::home_dir() {
            v.push(home.join("voices").join(&expanded));
            v.push(home.join(&expanded));
        }
        v.push(PathBuf::from(&expanded));
        v
    };
    for p in candidates {
        if p.is_file() {
            match p
                .extension()
                .and_then(|e| e.to_str())
                .map(str::to_lowercase)
            {
                Some(e) if ["wav", "mp3", "flac", "m4a", "ogg", "opus"].contains(&e.as_str()) => {
                    return Ok(p)
                }
                _ => return Err(format!("{} is not audio (wav/mp3/flac/m4a)", p.display())),
            }
        }
    }
    Err(format!("file not found: {t} (try ~/voices/<file>)"))
}

/// GUIDE §13: normalize a reference clip without touching the original.
/// Mono, 24kHz, leading silence trimmed, auto-leveled. Two fast passes
/// (measure, then fixed gain + limiter) — deliberately NOT loudnorm, which
/// hangs for minutes on some ffmpeg builds and froze the UI.
pub fn normalize_reference(src: &PathBuf, dest: &PathBuf) -> std::result::Result<(), String> {
    let gain = mean_volume_db(src).map(gain_for_mean).unwrap_or(0.0);
    let filter = format!(
        "silenceremove=start_periods=1:start_silence=0.3:start_threshold=-50dB,volume={gain:.1}dB,alimiter=limit=0.95"
    );
    let out = std::process::Command::new("ffmpeg")
        .args([
            "-y",
            "-loglevel",
            "error",
            "-i",
            &src.to_string_lossy(),
            "-ac",
            "1",
            "-ar",
            "24000",
            "-af",
            &filter,
            &dest.to_string_lossy(),
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map_err(|_| "ffmpeg not found — install ffmpeg to add voices".to_string())?;
    if out.success() && dest.is_file() {
        Ok(())
    } else {
        Err("ffmpeg could not read that file".into())
    }
}

/// Mean volume in dB via volumedetect (realtime or faster). None when
/// unmeasurable (silence / unreadable).
pub fn mean_volume_db(path: &PathBuf) -> Option<f64> {
    let out = std::process::Command::new("ffmpeg")
        .args([
            "-i",
            &path.to_string_lossy(),
            "-af",
            "volumedetect",
            "-vn",
            "-sn",
            "-dn",
            "-f",
            "null",
            "/dev/null",
        ])
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    parse_mean_volume(&String::from_utf8_lossy(&out.stderr))
}

pub fn parse_mean_volume(stderr: &str) -> Option<f64> {
    let i = stderr.find("mean_volume:")?;
    let rest = stderr[i + "mean_volume:".len()..].trim_start();
    let num: String = rest
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.' || *c == '-' || *c == '+')
        .collect();
    num.parse::<f64>().ok().filter(|v| v.is_finite())
}

/// Gain to land speech near -20 dB mean: bounded so quiet clips don't
/// explode into noise and loud ones aren't crushed.
pub fn gain_for_mean(mean_db: f64) -> f64 {
    (-20.0 - mean_db).clamp(-10.0, 20.0)
}

pub fn probe_secs(path: &PathBuf) -> std::result::Result<f64, String> {
    let out = std::process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
            &path.to_string_lossy(),
        ])
        .output()
        .map_err(|_| "ffprobe not found".to_string())?;
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse::<f64>()
        .map_err(|_| "could not read audio length".into())
}

/// One sentence's audio: the shared entry point for playback, previews and
/// export — cache hit returns instantly, otherwise ask the worker.
pub fn synthesize_one(
    rt: &tokio::runtime::Runtime,
    cache_dir: &Path,
    url: &str,
    model: &str,
    voice: &str,
    text: &str,
) -> std::result::Result<PathBuf, String> {
    let (_, cands) = worker_cache_candidates(cache_dir, model, voice, text);
    if let Some(hit) = cands.iter().find(|p| p.is_file()) {
        return Ok(hit.clone());
    }
    let worker =
        WorkerBackend::new(url.to_string(), model.to_string()).map_err(|e| e.to_string())?;
    let req = orpheus_tts::SynthesisRequest {
        request_id: uuid::Uuid::new_v4().to_string(),
        model: model.to_string(),
        voice: voice.to_string(),
        text: text.to_string(),
        speed: 1.0,
        out_path: cands[0].clone(),
    };
    use orpheus_tts::TTSBackend;
    rt.block_on(worker.synthesize(&req))
        .map(|r| r.audio_path)
        .map_err(|e| e.to_string())
}

/// Suggest the first sample found in ~/voices/ to cut typing.
pub fn default_sample_hint() -> Option<String> {
    let dir = dirs::home_dir()?.join("voices");
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            matches!(
                p.extension().and_then(|x| x.to_str()).map(str::to_lowercase),
                Some(e) if ["wav", "mp3", "flac", "m4a", "ogg", "opus"].contains(&e.as_str())
            )
            .then(|| p.file_name()?.to_str().map(str::to_string))
            .flatten()
        })
        .collect();
    names.sort();
    names.into_iter().next()
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct Line {
    pub sentence_idx: Option<usize>,
    pub is_heading: bool,
    pub text: String,
}

impl App {
    pub fn new(config: Config, db: LibraryDb) -> Self {
        let theme = Theme::builtin(&config.reader.theme);
        let style = ReaderStyle::from_name(&config.reader.style);
        Self {
            theme,
            style,
            screen: Screen::Home,
            prev_screen: Screen::Home,
            should_quit: false,
            status_msg: None,
            recent: Vec::new(),
            home_selected: 0,
            dir_path: None,
            dir_entries: Vec::new(),
            dir_selected: 0,
            book: None,
            sentence_cursor: 0,
            chapter_idx: 0,
            scroll: 0,
            playback: PlaybackState::Stopped,
            speed: config.playback.speed,
            chapter_lines: Vec::new(),
            search_query: String::new(),
            searching: false,
            model_selected: 0,
            model_input: String::new(),
            model_typing: false,
            appearance_section: 0,
            theme_selected: 0,
            style_selected: 1,
            player: FfplayPlayer::new(),
            rt: tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("tokio runtime"),
            audio_cache_dir: Config::audio_cache_dir(),
            buffering: false,
            mock_tick: 0,
            worker: Default::default(),
            worker_waiting: false,
            worker_load_started: std::time::Instant::now(),
            worker_evict_attempts: 0,
            voice_dur_cache: std::collections::HashMap::new(),
            export: None,
            worker_paths: crate::worker_ctl::WorkerPaths {
                script: (!config.tts.worker_script.is_empty())
                    .then(|| std::path::PathBuf::from(config.tts.worker_script.clone())),
                python: (!config.tts.worker_python.is_empty())
                    .then(|| std::path::PathBuf::from(config.tts.worker_python.clone())),
            },
            model_store: ModelStore::new(Config::data_dir().join("models")),
            voice_list: Vec::new(),
            voice_selected: 0,
            ve_name: String::new(),
            ve_path: String::new(),
            ve_field: 0,
            pending_voice_save: None,
            tts: TtsManager::new(),
            config,
            db,
        }
    }

    /// Open appearance picker, syncing selection cursors to current values.
    pub fn open_appearance(&mut self) {
        let names = orpheus_core::Theme::builtin_names();
        self.theme_selected = names
            .iter()
            .position(|n| *n == self.theme.name)
            .unwrap_or(0);
        let styles = orpheus_core::ReaderStyle::all();
        self.style_selected = styles.iter().position(|s| *s == self.style).unwrap_or(1);
        self.appearance_section = 0;
        self.goto(Screen::Appearance);
    }

    pub fn apply_appearance_selection(&mut self) {
        if self.appearance_section == 0 {
            let names = orpheus_core::Theme::builtin_names();
            if let Some(name) = names.get(self.theme_selected) {
                self.theme = orpheus_core::Theme::builtin(name);
                self.status_msg = Some(format!("theme: {name}"));
            }
        } else {
            let styles = orpheus_core::ReaderStyle::all();
            if let Some(style) = styles.get(self.style_selected) {
                self.style = *style;
                self.status_msg = Some(format!("style: {}", style.name()));
            }
        }
    }

    pub fn refresh_recent(&mut self) {
        if let Ok(rows) = self.db.recent_books(20) {
            self.recent = rows;
        }
    }

    /// Approach B reconciliation: on-disk `models/<id>/` manifests plus the
    /// `tts_models` table become the manager's installed set. Builtins like
    /// `kokoro` flip to installed without any reader code changing.
    pub fn sync_models_from_store(&mut self, store: &orpheus_tts::ModelStore) {
        for (id, manifest) in store.scan_installed() {
            let path = store
                .model_dir(&id)
                .unwrap_or_else(|_| PathBuf::from(&manifest.id));
            self.tts.register_installed(&manifest, path.clone());
            let _ = self.db.upsert_tts_model(
                &manifest.id,
                &manifest.backend,
                &manifest.hf_repo,
                &manifest.revision,
                &path.to_string_lossy(),
                manifest.total_bytes as i64,
            );
        }
    }

    // --- Voices ---------------------------------------------------------------

    /// Open the picker: built-ins from the store + cloned voices from SQLite.
    pub fn open_voices(&mut self) {
        let model = self.tts.current_model_id.clone();
        let clones = self.usable_clones(&model);
        // Clone-only models have no presets: listing the fake `<id>-default`
        // would offer a voice the worker rejects.
        let mut list = if self.model_clones_natively(&model) {
            Vec::new()
        } else {
            voices_for_model(&self.model_store, &model, &self.tts.current_voice_id).0
        };
        for r in clones {
            if !list.iter().any(|v| v.id == r.id) {
                list.push(VoiceEntry {
                    id: r.id,
                    note: "clone".into(),
                    custom: true,
                });
            }
        }
        if list.is_empty() {
            list.push(VoiceEntry {
                id: String::new(),
                note: "no sample yet — press a to add one".into(),
                custom: false,
            });
        }
        let sel = list
            .iter()
            .position(|v| v.id == self.tts.current_voice_id)
            .unwrap_or(0);
        self.voice_list = list;
        self.voice_selected = sel;
        self.goto(Screen::Voices);
    }

    /// Select the highlighted voice. Stops playback (different audio).
    pub fn select_voice(&mut self) {
        let Some(entry) = self.voice_list.get(self.voice_selected).cloned() else {
            return;
        };
        if entry.id.is_empty() {
            // Placeholder row on a clone-only model: nothing to select.
            self.status_msg = Some("add a 6s+ sample first: press a".into());
            return;
        }
        self.tts.current_voice_id = entry.id.clone();
        self.config.tts.voice = entry.id.clone();
        self.player.stop();
        self.buffering = false;
        if self.playback == PlaybackState::Playing {
            self.playback = PlaybackState::Paused;
        }
        self.status_msg = Some(format!("voice → {} (press Space to listen)", entry.id));
        self.go_back();
    }

    /// Delete the highlighted voice if it is a user clone.
    pub fn delete_selected_voice(&mut self) {
        let Some(entry) = self.voice_list.get(self.voice_selected).cloned() else {
            return;
        };
        if !entry.custom {
            self.status_msg = Some("built-in voices can't be deleted".into());
            return;
        }
        let _ = self.db.remove_voice(&entry.id);
        self.voice_dur_cache.clear();
        let wav = Config::voices_dir().join(format!("{}.wav", entry.id));
        let _ = std::fs::remove_file(&wav);
        let se = Config::voices_dir().join(format!("{}.se.pt", entry.id));
        let _ = std::fs::remove_file(&se);
        if self.tts.current_voice_id == entry.id {
            let def = orpheus_tts::voice_stem_default(&self.tts.current_model_id);
            self.tts.current_voice_id = def.clone();
            self.config.tts.voice = def;
        }
        self.status_msg = Some(format!("voice '{}' deleted", entry.id));
        self.open_voices(); // refresh list, stay on screen
    }

    /// Preview the highlighted voice: synthesize one line and play it now.
    /// Blocking for a few seconds on first synthesis (worker may load).
    pub fn preview_selected_voice(&mut self) {
        let Some(entry) = self.voice_list.get(self.voice_selected).cloned() else {
            return;
        };
        if entry.id.is_empty() {
            self.status_msg = Some("add a 6s+ sample first: press a".into());
            return;
        }
        if !self.worker_supported() {
            self.status_msg = Some(format!(
                "{} has no voice yet — pick kokoro to preview",
                self.tts.current_model_id
            ));
            return;
        }
        if !self.player.available() {
            self.status_msg = Some("ffplay not found — install ffmpeg".into());
            return;
        }
        const PREVIEW: &str = "The desert was vast and silent. Paul looked toward the horizon.";
        self.status_msg = Some(format!("previewing '{}'…", entry.id));
        match self.ensure_audio_for(&self.tts.current_model_id.clone(), &entry.id, PREVIEW) {
            Ok(path) => {
                if self.player.play(&path, 1.0).is_ok() {
                    self.status_msg = Some(format!("previewing '{}'", entry.id));
                } else {
                    self.status_msg = Some("preview playback failed".into());
                }
            }
            Err(e) => self.status_msg = Some(format!("preview failed: {e}")),
        }
    }

    /// Synthesize arbitrary text with an explicit model+voice (preview path).
    fn ensure_audio_for(
        &self,
        model: &str,
        voice: &str,
        text: &str,
    ) -> std::result::Result<PathBuf, String> {
        synthesize_one(
            &self.rt,
            &self.audio_cache_dir,
            &self.config.tts.worker_url,
            model,
            voice,
            text,
        )
    }

    // --- Add-voice editor ---------------------------------------------------------

    pub fn open_voice_editor(&mut self) {
        self.ve_name.clear();
        // Suggest the first sample found in ~/voices to cut typing.
        self.ve_path = default_sample_hint().unwrap_or_default();
        self.ve_field = 0;
        self.goto(Screen::VoiceEditor);
    }

    /// Fast checks only (name, file exists, source duration): runs on the key
    /// press and returns instantly. On success the caller stages the save and
    /// the heavy transcode happens in tick_voice_save, after the UI repaints.
    pub fn prepare_voice_save(&self) -> std::result::Result<PendingVoiceSave, String> {
        let name = self.ve_name.trim().to_string();
        if name.is_empty() {
            return Err("give the voice a name, then Enter".into());
        }
        let src = resolve_sample_path(&self.ve_path)?;
        let id = slugify(&name);
        if id.is_empty() {
            return Err("name must contain letters or digits".into());
        }
        // Probe BEFORE transcoding: a 20-minute chapter used to freeze the UI
        // through a doomed transcode before being rejected.
        let secs = probe_secs(&src)?;
        let min = self.min_sample_secs();
        if secs < min {
            return Err(format!(
                "sample is {secs:.1}s — {} needs {min:.0}s+ of speech",
                self.tts.current_model_id
            ));
        }
        if secs > 300.0 {
            return Err(format!(
                "sample is {secs:.0}s — cut 15–30s first, e.g. ffmpeg -ss 90 -t 25 -i input.mp3 out.wav"
            ));
        }
        let voices_dir = Config::voices_dir();
        std::fs::create_dir_all(&voices_dir).map_err(|e| e.to_string())?;
        Ok(PendingVoiceSave {
            id,
            name,
            src,
            dest: voices_dir.join(format!("{}.wav", slugify(&self.ve_name))),
        })
    }

    /// Heavy half of saving: normalize + register. Called from the tick, so
    /// the "normalizing…" status is already on screen and keys stay alive
    /// everywhere else. Esc meanwhile just leaves the editor; the result
    /// lands (or its error) right after.
    pub fn tick_voice_save(&mut self) {
        let Some(pending) = self.pending_voice_save.take() else {
            return;
        };
        if self.screen != Screen::VoiceEditor {
            return; // user backed out meanwhile; drop it
        }
        if let Err(e) = normalize_reference(&pending.src, &pending.dest) {
            self.status_msg = Some(e);
            return;
        }
        match probe_secs(&pending.dest) {
            Ok(secs) if secs >= 3.0 => {
                let model = self.tts.current_model_id.clone();
                match self.db.add_voice(
                    &pending.id,
                    &pending.name,
                    &model,
                    &pending.dest.to_string_lossy(),
                ) {
                    Ok(()) => {
                        self.voice_dur_cache.clear();
                        self.tts.current_voice_id = pending.id.clone();
                        self.config.tts.voice = pending.id.clone();
                        self.status_msg = Some(format!(
                            "voice '{}' added ({secs:.0}s) — p previews",
                            pending.name
                        ));
                        self.open_voices();
                    }
                    Err(e) => {
                        let _ = std::fs::remove_file(&pending.dest);
                        self.status_msg = Some(format!("could not save voice: {e}"));
                    }
                }
            }
            _ => {
                let _ = std::fs::remove_file(&pending.dest);
                self.status_msg = Some("transcode produced no usable audio".into());
            }
        }
    }

    // --- Whole-book export ---------------------------------------------------------

    pub fn export_active(&self) -> bool {
        self.export
            .as_ref()
            .map(|e| e.phase.is_active())
            .unwrap_or(false)
    }

    /// `x`: start an export, or cancel the running one.
    pub fn toggle_export(&mut self) {
        if self.export_active() {
            if let Some(job) = &self.export {
                job.request_cancel();
            }
            self.status_msg = Some("cancelling export\u{2913}\u{2026}".into());
            return;
        }
        self.export = None;
        self.start_export();
    }

    fn start_export(&mut self) {
        if self.book.is_none() {
            self.status_msg = Some("no book open".into());
            return;
        }
        if !self.worker_supported() {
            self.status_msg = Some(format!(
                "{} has no voice yet — press m to pick one",
                self.tts.current_model_id
            ));
            return;
        }
        if !self.player.available() {
            self.status_msg = Some("ffplay not found — install ffmpeg".into());
            return;
        }
        let (title, texts) = {
            let Some(book) = &self.book else { return };
            let texts: Vec<String> = book
                .flat_sentences()
                .into_iter()
                .filter_map(|(_, _, _, s)| {
                    let t = s.speak_text.trim();
                    if t.is_empty() {
                        None
                    } else {
                        Some(t.to_string())
                    }
                })
                .collect();
            (book.title.clone(), texts)
        };
        // Narration and export both drive the worker; one at a time.
        self.player.stop();
        self.buffering = false;
        self.playback = PlaybackState::Stopped;

        let model = self.tts.current_model_id.clone();
        let voice = self.tts.current_voice_id.clone();
        let out = crate::export::export_path(&title, &voice, &model);
        match crate::export::ExportJob::start(
            texts,
            self.config.tts.worker_url.clone(),
            model,
            voice,
            out,
        ) {
            Ok(job) => {
                self.export = Some(job);
                self.status_msg = Some("⤓ export started…".into());
            }
            Err(e) => self.status_msg = Some(format!("export: {e}")),
        }
    }

    /// Drain export events each tick and report progress in the status line.
    pub fn tick_export(&mut self) {
        let Some(job) = self.export.as_mut() else {
            return;
        };
        job.poll();
        let done = job.done;
        let total = job.total;
        let phase = job.phase.clone();

        let (msg, finished) = match phase {
            crate::export::Phase::Starting => {
                ("⤓ export: waiting for the worker…".to_string(), false)
            }
            crate::export::Phase::Rendering => {
                let pct = if total > 0 { done * 100 / total } else { 0 };
                (
                    format!("rendering {pct}% ({done}/{total}) - x cancels"),
                    false,
                )
            }
            crate::export::Phase::Joining => (format!("⤓ joining {total} clips…"), false),
            crate::export::Phase::Done(path) => (format!("✓ exported → {}", path.display()), true),
            crate::export::Phase::Failed(e) => (format!("export failed: {e}"), true),
            crate::export::Phase::Cancelled => (
                "export cancelled — cached sentences are free on restart".to_string(),
                true,
            ),
        };
        if finished {
            self.export = None;
        }
        self.status_msg = Some(msg);
    }

    pub fn goto(&mut self, s: Screen) {
        if self.screen != s {
            self.prev_screen = self.screen;
        }
        self.screen = s;
        self.scroll = 0;
    }

    pub fn go_back(&mut self) {
        let cur = self.screen;
        self.screen = match cur {
            Screen::Reader => {
                if self.dir_path.is_some() {
                    Screen::Directory
                } else {
                    Screen::Home
                }
            }
            Screen::Directory
            | Screen::Models
            | Screen::Voices
            | Screen::VoiceEditor
            | Screen::Appearance
            | Screen::Help
            | Screen::Search => {
                if self.book.is_some() {
                    Screen::Reader
                } else {
                    self.prev_screen
                }
            }
            Screen::Home => Screen::Home,
        };
    }

    // --- Book lifecycle --------------------------------------------------------

    pub fn open_book_file(&mut self, path: &std::path::Path) -> Result<()> {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();
        let doc = match ext.as_str() {
            "epub" => orpheus_core::epub::load_epub(path)?,
            "pdf" => orpheus_core::pdf::load_pdf(path)?,
            _ => anyhow::bail!("unsupported file (want .epub/.pdf): {}", path.display()),
        };
        self.db.upsert_book(
            &doc.id,
            &doc.source_path,
            &doc.title,
            &doc.author,
            doc.format.as_str(),
        )?;
        // Restore position if known.
        if let Ok(Some((ch, sent, _))) = self.db.get_position(&doc.id) {
            self.chapter_idx = (ch as usize).min(doc.chapters.len().saturating_sub(1));
            self.sentence_cursor = sent as usize;
        } else {
            self.chapter_idx = 0;
            self.sentence_cursor = 0;
        }
        // Clamp cursor into chapter if mismatch.
        self.book = Some(doc);
        self.rebuild_chapter_lines();
        self.snap_cursor_to_chapter();
        self.goto(Screen::Reader);
        self.refresh_recent();
        Ok(())
    }

    pub fn open_directory(&mut self, dir: PathBuf, recursive: bool) {
        self.dir_entries = orpheus_core::library::scan_directory(&dir, recursive);
        self.dir_path = Some(dir);
        self.dir_selected = 0;
        self.goto(Screen::Directory);
    }

    pub fn total_sentences(&self) -> usize {
        self.book.as_ref().map(|b| b.total_sentences()).unwrap_or(0)
    }

    pub fn progress(&self) -> f32 {
        match &self.book {
            Some(b) => b.progress_for(self.chapter_idx, self.sentence_cursor),
            None => 0.0,
        }
    }

    pub fn persist_position(&self) {
        if let Some(b) = &self.book {
            let _ = self.db.save_position(
                &b.id,
                self.chapter_idx as i64,
                self.sentence_cursor as usize as i64,
                self.progress() as f64,
            );
        }
    }

    // --- Chapter lines ----------------------------------------------------------

    /// Flatten current chapter into render lines, tracking sentence ownership.
    pub fn rebuild_chapter_lines(&mut self) {
        self.chapter_lines.clear();
        let Some(book) = &self.book else { return };
        let Some(ch) = book.chapters.get(self.chapter_idx) else {
            return;
        };
        // Map global sentence index: count sentences in earlier chapters.
        let mut global = 0usize;
        for (ci, c) in book.chapters.iter().enumerate() {
            if ci < self.chapter_idx {
                global += c.blocks.iter().map(|b| b.sentences.len()).sum::<usize>();
            }
        }
        for block in &ch.blocks {
            if block.sentences.is_empty() {
                self.chapter_lines.push(Line {
                    sentence_idx: None,
                    is_heading: block.kind == orpheus_core::document::BlockKind::Heading,
                    text: block.text.clone(),
                });
                continue;
            }
            // One visual line per sentence (wrapping handled by ratatui).
            // Blank line between blocks is added as empty line.
            for s in &block.sentences {
                self.chapter_lines.push(Line {
                    sentence_idx: Some(global),
                    is_heading: block.kind == orpheus_core::document::BlockKind::Heading,
                    text: s.text.clone(),
                });
                global += 1;
            }
            self.chapter_lines.push(Line {
                sentence_idx: None,
                is_heading: false,
                text: String::new(),
            });
        }
    }

    /// Ensure cursor points inside current chapter; used after chapter jumps.
    pub fn snap_cursor_to_chapter(&mut self) {
        let Some(book) = &self.book else { return };
        let mut start = 0usize;
        for (ci, c) in book.chapters.iter().enumerate() {
            let n: usize = c.blocks.iter().map(|b| b.sentences.len()).sum();
            if ci == self.chapter_idx {
                // Clamp cursor into [start, start+n).
                if n == 0
                    || self.sentence_cursor < start
                    || self.sentence_cursor >= start + n.max(1)
                {
                    self.sentence_cursor = start;
                }
                return;
            }
            start += n;
        }
    }

    pub fn chapter_start_global(&self) -> usize {
        let Some(book) = &self.book else { return 0 };
        let mut start = 0;
        for (ci, c) in book.chapters.iter().enumerate() {
            if ci == self.chapter_idx {
                return start;
            }
            start += c.blocks.iter().map(|b| b.sentences.len()).sum::<usize>();
        }
        start
    }

    #[allow(dead_code)]
    pub fn chapter_sentence_count(&self) -> usize {
        self.book
            .as_ref()
            .and_then(|b| b.chapters.get(self.chapter_idx))
            .map(|c| c.blocks.iter().map(|b| b.sentences.len()).sum())
            .unwrap_or(0)
    }

    pub fn next_sentence(&mut self) {
        if self.sentence_cursor + 1 < self.total_sentences() {
            self.sentence_cursor += 1;
            self.ensure_cursor_visible_chapter();
            self.persist_position();
            self.restart_audio_if_playing();
        }
    }

    pub fn prev_sentence(&mut self) {
        if self.sentence_cursor > 0 {
            self.sentence_cursor -= 1;
            self.ensure_cursor_visible_chapter();
            self.persist_position();
            self.restart_audio_if_playing();
        }
    }

    /// If cursor left the current chapter, follow it.
    fn ensure_cursor_visible_chapter(&mut self) {
        let Some(book) = &self.book else { return };
        let mut start = 0;
        for (ci, c) in book.chapters.iter().enumerate() {
            let n: usize = c.blocks.iter().map(|b| b.sentences.len()).sum();
            if self.sentence_cursor >= start && self.sentence_cursor < start + n.max(1) {
                if ci != self.chapter_idx {
                    self.chapter_idx = ci;
                    self.rebuild_chapter_lines();
                }
                return;
            }
            start += n;
        }
    }

    pub fn next_chapter(&mut self) {
        if let Some(b) = &self.book {
            if self.chapter_idx + 1 < b.chapters.len() {
                self.chapter_idx += 1;
                self.rebuild_chapter_lines();
                self.snap_cursor_to_chapter();
                // Move to chapter start.
                self.sentence_cursor = self.chapter_start_global();
                self.persist_position();
                self.restart_audio_if_playing();
            }
        }
    }

    pub fn prev_chapter(&mut self) {
        if self.chapter_idx > 0 {
            self.chapter_idx -= 1;
            self.rebuild_chapter_lines();
            self.snap_cursor_to_chapter();
            self.sentence_cursor = self.chapter_start_global();
            self.persist_position();
            self.restart_audio_if_playing();
        }
    }

    pub fn toggle_play(&mut self) {
        match self.playback {
            PlaybackState::Playing => {
                // True pause: freeze mid-sentence, resume continues in place.
                self.player.pause();
                self.buffering = false;
                self.playback = PlaybackState::Paused;
                self.status_msg = Some("⏸ paused".into());
            }
            _ => {
                if self.book.is_none() {
                    self.status_msg = Some("no book open".into());
                    return;
                }
                if !self.uses_worker_audio() {
                    // Mock model: advance the highlight on a timer (no audio).
                    self.playback = PlaybackState::Playing;
                    self.status_msg = Some(format!(
                        "▶ narrating with {} @ {:.2}x (mock audio until worker serves it)",
                        self.tts.current_model_id, self.speed
                    ));
                    return;
                }
                if !self.worker_supported() {
                    // Selected family has no voice worker yet (e.g. chatterbox).
                    // Say so plainly instead of a misleading restart hint.
                    self.status_msg = Some(format!(
                        "{} has no voice yet — press m and pick kokoro to listen",
                        self.tts.current_model_id
                    ));
                    return;
                }
                if !self.player.available() {
                    self.status_msg =
                        Some("ffplay not found — install ffmpeg for narration audio".into());
                    return;
                }
                // Resume a frozen sentence in place when possible; otherwise
                // re-buffer from the cursor (non-blocking, tick does the work).
                if self.player.resume() {
                    self.playback = PlaybackState::Playing;
                    self.status_msg = Some("▶ resumed".into());
                    return;
                }
                self.playback = PlaybackState::Playing;
                self.buffering = true;
                self.status_msg = Some("▶ buffering…".into());
            }
        }
    }

    // --- Narration (worker audio) --------------------------------------------------

    /// True when the current model should produce real audio via book-tts.
    pub fn uses_worker_audio(&self) -> bool {
        match self.tts.current_model() {
            Some(m) => !matches!(m.backend, BackendKind::Mock),
            None => false,
        }
    }

    /// True when a worker implementation exists for the current family.
    pub fn worker_supported(&self) -> bool {
        match self.tts.current_model() {
            Some(m) => matches!(
                m.backend,
                BackendKind::Kokoro | BackendKind::ChatterboxTurbo
            ),
            None => false,
        }
    }

    /// Minimum reference-clip length this model's cloning accepts.
    /// Kokoro/OpenVoice tolerate short clips; Chatterbox asserts >=5s.
    pub fn min_sample_secs(&self) -> f64 {
        self.min_sample_secs_for(&self.tts.current_model_id.clone())
    }

    pub fn min_sample_secs_for(&self, model_id: &str) -> f64 {
        match self
            .tts
            .list_models()
            .into_iter()
            .find(|m| m.id == model_id)
        {
            Some(m) if matches!(m.backend, BackendKind::ChatterboxTurbo) => 6.0,
            _ => 3.0,
        }
    }

    /// Clones this model can actually use: any registered sample (they are
    /// model-agnostic files) that exists and meets the model's minimum
    /// length. Too-short or missing samples are dropped instead of being
    /// offered and failing on every sentence.
    pub fn usable_clones(&mut self, model_id: &str) -> Vec<orpheus_core::VoiceRow> {
        let min = self.min_sample_secs_for(model_id);
        let mut rows = self.db.list_all_voices().unwrap_or_default();
        rows.retain(|r| {
            self.sample_secs(&r.reference_path)
                .map(|d| d + 0.01 >= min)
                .unwrap_or(false)
        });
        rows
    }

    /// Duration of a sample, cached until the file changes.
    fn sample_secs(&mut self, path: &str) -> Option<f64> {
        let meta = std::fs::metadata(path).ok()?;
        let mtime = meta.modified().ok()?;
        if let Some((t, secs)) = self.voice_dur_cache.get(path) {
            if *t == mtime {
                return Some(*secs);
            }
        }
        let secs = probe_secs(&std::path::PathBuf::from(path)).ok()?;
        self.voice_dur_cache.insert(path.to_string(), (mtime, secs));
        Some(secs)
    }

    /// Does this model clone natively (no OpenVoice conversion stage)?
    /// Such models ship **no** built-in voices: `<id>-default` is meaningless
    /// and the worker rejects it, so the UI must always offer a real clone.
    pub fn model_clones_natively(&self, model_id: &str) -> bool {
        self.tts
            .list_models()
            .into_iter()
            .find(|m| m.id == model_id)
            .map(|m| matches!(m.backend, BackendKind::ChatterboxTurbo))
            .unwrap_or(false)
    }

    /// Voice to select for `model_id`: a preset when the model has one,
    /// otherwise the first registered clone (clone-only models), otherwise a
    /// placeholder the worker will explain how to fix.
    pub fn first_voice_for(&mut self, model_id: &str) -> String {
        if !self.model_clones_natively(model_id) {
            return orpheus_tts::voice_stem_default(model_id);
        }
        self.usable_clones(model_id)
            .into_iter()
            .map(|r| r.id)
            .next()
            .unwrap_or_else(|| orpheus_tts::voice_stem_default(model_id))
    }

    /// After a model switch (and at startup): guarantee the selected voice is
    /// one this model can actually render. Clone-only models reject
    /// `<id>-default`, which is exactly "model loads but never speaks".
    pub fn normalize_voice(&mut self) {
        let model = self.tts.current_model_id.clone();
        let voice = self.tts.current_voice_id.clone();
        if voice.is_empty() {
            let v = self.first_voice_for(&model);
            self.tts.current_voice_id = v;
            return;
        }
        if !self.model_clones_natively(&model) {
            return;
        }
        let known = self
            .usable_clones(&model)
            .into_iter()
            .any(|r| r.id == voice);
        if !known {
            let fallback = self.first_voice_for(&model);
            if fallback == voice {
                // No clone registered yet: keep it, the worker's error text
                // tells the user how to add one.
                self.status_msg =
                    Some("chatterbox needs a clone: press v then a to add a 6s+ sample".into());
            } else {
                self.tts.current_voice_id = fallback.clone();
                self.status_msg = Some(format!("voice → {fallback} (model has no built-ins)"));
            }
        }
    }

    /// Speakable (TTS-preprocessed) text at a global sentence index.
    /// None = artifact (page number etc.), skip it in narration.
    pub fn narration_text(&self, idx: usize) -> Option<String> {
        let book = self.book.as_ref()?;
        let flat = book.flat_sentences();
        flat.get(idx).and_then(|(_, _, _, s)| {
            let t = s.speak_text.trim();
            if t.is_empty() {
                None
            } else {
                Some(t.to_string())
            }
        })
    }

    fn cached_audio(&self, text: &str) -> Option<PathBuf> {
        let (_, cands) = worker_cache_candidates(
            &self.audio_cache_dir,
            &self.tts.current_model_id,
            &self.tts.current_voice_id,
            text,
        );
        cands.into_iter().find(|p| p.is_file())
    }

    /// Local file for this text: cache hit or worker synthesis (blocking).
    fn ensure_audio(&self, text: &str) -> std::result::Result<PathBuf, String> {
        self.ensure_audio_for(
            &self.tts.current_model_id.clone(),
            &self.tts.current_voice_id.clone(),
            text,
        )
    }

    fn play_sentence(&mut self, idx: usize) -> std::result::Result<(), String> {
        let text = self.narration_text(idx).ok_or_else(|| "skip".to_string())?;
        let path = self.ensure_audio(&text)?;
        self.player
            .play(&path, self.speed)
            .map_err(|e| e.to_string())
    }

    /// After any cursor move: drop stale audio so a later resume can never
    /// replay the old sentence. Restart is non-blocking (tick replays when
    /// still playing); seeks stay responsive even mid-synthesis.
    pub fn restart_audio_if_playing(&mut self) {
        if !self.uses_worker_audio() {
            return;
        }
        self.player.stop();
        if self.playback == PlaybackState::Playing {
            self.buffering = true;
        }
    }

    /// Prefetch the next few sentences in a background thread so continuous
    /// playback rarely waits on synthesis.
    pub fn prefetch_ahead(&self) {
        if !self.uses_worker_audio() || !self.worker_supported() {
            return;
        }
        let Some(book) = &self.book else { return };
        let total = book.total_sentences();
        let flat = book.flat_sentences();
        let mut jobs: Vec<(String, String, String, String)> = Vec::new();
        for idx in (self.sentence_cursor + 1)..=(self.sentence_cursor + 3).min(total) {
            let Some((_, _, _, s)) = flat.get(idx) else {
                continue;
            };
            let t = s.speak_text.trim();
            if t.is_empty() || self.cached_audio(t).is_some() {
                continue;
            }
            jobs.push((
                self.config.tts.worker_url.clone(),
                self.tts.current_model_id.clone(),
                self.tts.current_voice_id.clone(),
                t.to_string(),
            ));
        }
        if jobs.is_empty() {
            return;
        }
        let cache_dir = self.audio_cache_dir.clone();
        std::thread::spawn(move || {
            let Ok(rt) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            else {
                return;
            };
            use orpheus_tts::TTSBackend;
            for (url, model, voice, text) in jobs {
                let Ok(worker) = WorkerBackend::new(url, model.clone()) else {
                    return;
                };
                let (_, cands) = worker_cache_candidates(&cache_dir, &model, &voice, &text);
                let req = orpheus_tts::SynthesisRequest {
                    request_id: uuid::Uuid::new_v4().to_string(),
                    model,
                    voice,
                    text,
                    speed: 1.0,
                    out_path: cands[0].clone(),
                };
                // Best effort: failures surface when the sentence comes up.
                let _ = rt.block_on(worker.synthesize(&req));
            }
        });
    }

    /// Called every UI tick. Advances narration when the current file
    /// finishes; synthesizes when buffering. Runs on any screen so listening
    /// continues while browsing (highlight follows on return).
    pub fn tick_playback(&mut self) {
        if self.playback != PlaybackState::Playing || self.book.is_none() {
            return;
        }
        if !self.uses_worker_audio() {
            self.mock_tick += 1;
            let every = ((8.0 / self.speed).round() as u8).clamp(2, 16);
            if self.mock_tick >= every {
                self.mock_tick = 0;
                if self.sentence_cursor + 1 < self.total_sentences() {
                    self.next_sentence();
                } else {
                    self.playback = PlaybackState::Stopped;
                    self.status_msg = Some("■ finished".into());
                }
            }
            return;
        }
        if self.buffering {
            match self.play_sentence(self.sentence_cursor) {
                Ok(()) => {
                    self.buffering = false;
                    self.status_msg = Some(format!(
                        "▶ {} @ {:.2}x",
                        self.tts.current_voice_id, self.speed
                    ));
                    self.prefetch_ahead();
                }
                Err(e) if e == "skip" => {
                    // Junk sentence under cursor: step forward, stay buffering.
                    if self.sentence_cursor + 1 < self.total_sentences() {
                        self.sentence_cursor += 1;
                        self.ensure_cursor_visible_chapter();
                        self.persist_position();
                    } else {
                        self.playback = PlaybackState::Stopped;
                        self.buffering = false;
                    }
                }
                Err(e) => {
                    self.playback = PlaybackState::Paused;
                    self.buffering = false;
                    self.status_msg = Some(format!("audio: {e}"));
                }
            }
            return;
        }
        if self.player.is_playing() {
            return;
        }
        // Current file finished → advance to next speakable sentence.
        let total = self.total_sentences();
        let mut next = self.sentence_cursor + 1;
        while next < total && self.narration_text(next).is_none() {
            next += 1;
        }
        if next < total {
            self.sentence_cursor = next;
            self.ensure_cursor_visible_chapter();
            self.persist_position();
            self.buffering = true; // tick replays (usually instant cache hit)
        } else {
            self.playback = PlaybackState::Stopped;
            self.status_msg = Some("■ finished".into());
        }
    }

    pub fn set_speed(&mut self, s: f32) {
        self.speed = s.clamp(0.5, 2.5);
    }

    #[allow(dead_code)]
    pub fn status(&self) -> String {
        if let Some(m) = &self.status_msg {
            return m.clone();
        }
        match self.screen {
            Screen::Home => {
                "Enter open · o directory · t looks · m models · v voices · ? help · q quit".into()
            }
            Screen::Directory => "Enter open · r rescan · Esc back · q quit".into(),
            Screen::Reader => {
                "Space play · ←/→ sentence · shift+←/→ seek · n/p chapter · j/k scroll · / search · x export · ? help"
                    .into()
            }
            _ => "Esc back · q quit".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_store(tag: &str) -> (PathBuf, ModelStore) {
        let dir = std::env::temp_dir().join(format!("orpheus-voices-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        (dir.clone(), ModelStore::new(dir))
    }

    #[test]
    fn voice_list_from_store_stems_default_first() {
        let (_dir, store) = tmp_store("stems");
        // Fake a pulled kokoro: manifest + voices/*.pt.
        let manifest = orpheus_tts::ModelManifest::new(
            "kokoro",
            "hexgrad/Kokoro-82M",
            "abc",
            "kokoro",
            vec![],
            0,
        );
        store.write_manifest(&manifest).unwrap();
        let vdir = store.model_dir("kokoro").unwrap().join("voices");
        std::fs::create_dir_all(&vdir).unwrap();
        for stem in ["af_heart", "af_bella", "am_adam"] {
            std::fs::write(vdir.join(format!("{stem}.pt")), b"fake").unwrap();
        }
        let (list, sel) = voices_for_model(&store, "kokoro", "af_bella");
        let ids: Vec<_> = list.iter().map(|v| v.id.as_str()).collect();
        assert_eq!(ids[0], "kokoro-default");
        assert!(ids.contains(&"af_heart"));
        assert!(ids.contains(&"am_adam"));
        assert_eq!(sel, ids.iter().position(|i| *i == "af_bella").unwrap());
        assert!(list.iter().all(|v| !v.custom));
    }

    #[test]
    fn voice_list_falls_back_to_current() {
        let (_dir, store) = tmp_store("empty");
        let (list, sel) = voices_for_model(&store, "mock", "mock-default");
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "mock-default");
        assert_eq!(sel, 0);
    }

    #[test]
    fn gain_targets_minus20_bounded() {
        assert!((gain_for_mean(-25.7) - 5.7).abs() < 1e-9);
        assert_eq!(gain_for_mean(-20.0), 0.0);
        assert_eq!(gain_for_mean(-60.0), 20.0); // floor: don't amplify noise
        assert_eq!(gain_for_mean(0.0), -10.0); // ceiling on cuts
    }

    #[test]
    fn parses_volumedetect_output() {
        let sample = "[Parsed_volumedetect_0 @ 0x123] mean_volume: -25.7 dB\n\
                      [Parsed_volumedetect_0 @ 0x123] max_volume: -3.1 dB\n";
        assert_eq!(parse_mean_volume(sample), Some(-25.7));
        assert_eq!(parse_mean_volume("mean_volume: -inf dB"), None);
        assert_eq!(parse_mean_volume("mean_volume: n/a"), None);
        assert_eq!(parse_mean_volume("no stats here"), None);
    }

    #[test]
    fn slugify_names() {
        assert_eq!(slugify("Sarah the Narrator"), "sarah-the-narrator");
        assert_eq!(slugify("  Dune!! Voice 2 "), "dune-voice-2");
        assert_eq!(slugify("!!!"), "");
    }

    #[test]
    fn resolve_rejects_missing_and_non_audio() {
        assert!(resolve_sample_path("").is_err());
        assert!(resolve_sample_path("definitely-not-here-12345.wav").is_err());
        let dir = std::env::temp_dir().join(format!("orpheus-voices-txt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let txt = dir.join("note.txt");
        std::fs::write(&txt, b"hi").unwrap();
        assert!(resolve_sample_path(txt.to_str().unwrap()).is_err());
        let wav = dir.join("sample.wav");
        std::fs::write(&wav, b"RIFF....").unwrap();
        assert_eq!(resolve_sample_path(wav.to_str().unwrap()).unwrap(), wav);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
