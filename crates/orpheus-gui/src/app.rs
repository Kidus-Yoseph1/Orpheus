//! Orpheus GUI: an egui/eframe reader that reuses orpheus-core (document,
//! EPUB, config, DB, themes) and orpheus-tts (worker client, cache keys).
//!
//! It mirrors the TUI's look: centered column, per-sentence highlight, the
//! same theme palette, and the same worker+cache+ffplay narration pipeline.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use eframe::egui::{
    self, Align, Color32, CursorIcon, FontData, FontDefinitions, FontFamily, FontId, RichText,
    Stroke, TextFormat,
};
use eframe::egui::text::LayoutJob;
use orpheus_core::config::Config;
use orpheus_core::db::BookRow;
use orpheus_core::document::{BlockKind, Document};
use orpheus_core::library::FoundBook;
use orpheus_core::{LibraryDb, Theme};
use orpheus_tts::{worker_cache_candidates, ModelStore, WorkerBackend, TTSBackend};

use crate::audio::Player;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Library,
    Reader,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Playback {
    Stopped,
    Playing,
    Paused,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FontChoice {
    Serif,
    Mono,
}

struct Job {
    url: String,
    model: String,
    voice: String,
    text: String,
    out: PathBuf,
}

/// One flattened sentence across the whole book, for narration indexing.
#[derive(Clone)]
struct Flat {
    chapter: usize,
    speak: Option<String>,
}

struct BlockView {
    kind: BlockKind,
    /// (text, global sentence index)
    sentences: Vec<(String, usize)>,
}

struct ChapterView {
    title: String,
    blocks: Vec<BlockView>,
}

pub struct OrpheusGui {
    config: Config,
    theme: Theme,
    theme_name: String,
    db: LibraryDb,
    model_store: ModelStore,
    model: String,
    voice: String,
    voices: Vec<String>,

    screen: Screen,
    recent: Vec<BookRow>,
    books_dir: Vec<FoundBook>,
    path_input: String,

    book: Option<Document>,
    flat: Vec<Flat>,
    chapter_idx: usize,
    cursor: usize,
    last_scrolled: Option<usize>,
    view: Option<ChapterView>,
    view_key: (String, usize),

    speed: f32,
    playback: Playback,
    buffering: bool,
    player: Player,

    synth_tx: mpsc::Sender<Job>,
    results: mpsc::Receiver<(PathBuf, f32)>,
    durations: std::collections::HashMap<PathBuf, f32>,
    play_started: Option<Instant>,
    play_dur: f32,
    pending: HashSet<PathBuf>,

    font: FontChoice,
    serif_loaded: bool,
    visuals_key: String,
    status: Option<String>,
    status_until: Option<Instant>,
}

impl OrpheusGui {
    pub fn new(cc: &eframe::CreationContext<'_>, initial: Option<String>) -> Self {
        let config = Config::load();
        let theme = Theme::builtin(&config.reader.theme);
        let theme_name = theme.name.clone();
        let db = LibraryDb::open(&Config::db_path()).expect("open library db");
        let model_store = ModelStore::new(Config::data_dir().join("models"));
        let model = config.tts.model.clone();
        let voice = config.tts.voice.clone();

        let (tx, rx) = mpsc::channel::<Job>();
        let (res_tx, res_rx) = mpsc::channel::<(PathBuf, f32)>();
        std::thread::spawn(move || {
            let Ok(rt) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            else {
                return;
            };
            while let Ok(job) = rx.recv() {
                let Ok(worker) = WorkerBackend::new(job.url, job.model.clone()) else {
                    continue;
                };
                let req = orpheus_tts::SynthesisRequest {
                    request_id: uuid::Uuid::new_v4().to_string(),
                    model: job.model,
                    voice: job.voice,
                    text: job.text,
                    speed: 1.0,
                    out_path: job.out,
                };
                if let Ok(res) = rt.block_on(worker.synthesize(&req)) {
                    let _ = res_tx.send((res.audio_path, res.duration_secs));
                }
            }
        });

        let font = FontChoice::Serif;
        let serif_loaded = install_fonts(&cc.egui_ctx, font);

        let mut app = Self {
            config,
            theme,
            theme_name,
            db,
            model_store,
            model,
            voice,
            voices: Vec::new(),
            screen: Screen::Library,
            recent: Vec::new(),
            books_dir: Vec::new(),
            path_input: String::new(),
            book: None,
            flat: Vec::new(),
            chapter_idx: 0,
            cursor: 0,
            last_scrolled: None,
            view: None,
            view_key: (String::new(), usize::MAX),
            speed: 1.0,
            playback: Playback::Stopped,
            buffering: false,
            player: Player::new(),
            synth_tx: tx,
            results: res_rx,
            durations: std::collections::HashMap::new(),
            play_started: None,
            play_dur: 0.0,
            pending: HashSet::new(),
            font,
            serif_loaded,
            visuals_key: String::new(),
            status: None,
            status_until: None,
        };
        app.speed = app.config.playback.speed;
        app.refresh_recent();
        app.scan_books();
        app.voices = app.scan_voices();
        if let Some(p) = initial {
            let pb = PathBuf::from(&p);
            if pb.is_file() {
                app.open_book(&pb);
            }
        }
        app
    }

    fn set_status(&mut self, msg: impl Into<String>) {
        self.status = Some(msg.into());
        self.status_until = Some(Instant::now() + Duration::from_secs(6));
    }

    fn refresh_recent(&mut self) {
        if let Ok(rows) = self.db.recent_books(40) {
            self.recent = rows;
        }
    }

    fn scan_books(&mut self) {
        if let Some(home) = dirs::home_dir() {
            let dir = home.join("Books");
            if dir.is_dir() {
                self.books_dir = orpheus_core::library::scan_directory(&dir, false);
            }
        }
    }

    fn scan_voices(&self) -> Vec<String> {
        let mut v = vec![orpheus_tts::voice_stem_default(&self.model)];
        if let Ok(dir) = self.model_store.model_dir(&self.model) {
            if let Ok(rd) = std::fs::read_dir(dir.join("voices")) {
                let mut stems: Vec<String> = rd
                    .flatten()
                    .filter_map(|e| {
                        let p = e.path();
                        if p.extension().and_then(|x| x.to_str()) == Some("pt") {
                            p.file_stem().and_then(|s| s.to_str()).map(str::to_string)
                        } else {
                            None
                        }
                    })
                    .collect();
                stems.sort();
                for s in stems {
                    if !v.contains(&s) {
                        v.push(s);
                    }
                }
            }
        }
        if !v.iter().any(|x| x == &self.voice) {
            v.push(self.voice.clone());
        }
        v
    }

    // --- book lifecycle -----------------------------------------------------

    fn open_book(&mut self, path: &Path) {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();
        let doc = match ext.as_str() {
            "epub" => orpheus_core::epub::load_epub(path),
            "pdf" => orpheus_core::pdf::load_pdf(path),
            _ => {
                self.set_status(format!("unsupported file: {}", path.display()));
                return;
            }
        };
        let doc = match doc {
            Ok(d) => d,
            Err(e) => {
                self.set_status(format!("could not open: {e}"));
                return;
            }
        };
        let _ = self.db.upsert_book(
            &doc.id,
            &doc.source_path,
            &doc.title,
            &doc.author,
            doc.format.as_str(),
        );
        self.flat = build_flat(&doc);
        if let Ok(Some((ch, sent, _))) = self.db.get_position(&doc.id) {
            self.chapter_idx = (ch as usize).min(doc.chapters.len().saturating_sub(1));
            self.cursor = (sent as usize).min(self.flat.len().saturating_sub(1));
        } else {
            self.chapter_idx = 0;
            self.cursor = 0;
        }
        // Keep chapter and cursor consistent.
        if let Some(f) = self.flat.get(self.cursor) {
            self.chapter_idx = f.chapter;
        }
        self.player.stop();
        self.playback = Playback::Stopped;
        self.buffering = false;
        self.book = Some(doc);
        self.screen = Screen::Reader;
        self.last_scrolled = None;
        self.view = None;
        self.refresh_recent();
        self.set_status("opened · press Space to narrate");
    }

    fn chapter_start_global(&self) -> usize {
        self.flat
            .iter()
            .position(|f| f.chapter == self.chapter_idx)
            .unwrap_or(0)
    }

    fn total(&self) -> usize {
        self.flat.len()
    }

    fn progress(&self) -> f32 {
        if self.flat.is_empty() {
            0.0
        } else {
            self.cursor as f32 / self.flat.len() as f32
        }
    }

    fn persist(&self) {
        if let Some(b) = &self.book {
            let _ = self.db.save_position(
                &b.id,
                self.chapter_idx as i64,
                self.cursor as i64,
                self.progress() as f64,
            );
        }
    }

    fn set_cursor(&mut self, n: usize) {
        if n >= self.flat.len() {
            return;
        }
        self.cursor = n;
        if let Some(f) = self.flat.get(n) {
            self.chapter_idx = f.chapter;
        }
        self.persist();
    }

    fn speak_at(&self, idx: usize) -> Option<String> {
        self.flat.get(idx).and_then(|f| f.speak.clone())
    }

    fn advance(&mut self) -> bool {
        let mut n = self.cursor + 1;
        while n < self.flat.len() && self.flat[n].speak.is_none() {
            n += 1;
        }
        if n < self.flat.len() {
            self.set_cursor(n);
            true
        } else {
            false
        }
    }

    fn next_sentence(&mut self) {
        if self.advance() {
            self.restart_if_playing();
        }
    }

    fn prev_sentence(&mut self) {
        let mut n = self.cursor;
        while n > 0 {
            n -= 1;
            if self.flat[n].speak.is_some() {
                self.set_cursor(n);
                self.restart_if_playing();
                return;
            }
        }
    }

    fn next_chapter(&mut self) {
        if let Some(b) = &self.book {
            if self.chapter_idx + 1 < b.chapters.len() {
                self.chapter_idx += 1;
                self.cursor = self.chapter_start_global();
                self.persist();
                self.restart_if_playing();
            }
        }
    }

    fn prev_chapter(&mut self) {
        if self.chapter_idx > 0 {
            self.chapter_idx -= 1;
            self.cursor = self.chapter_start_global();
            self.persist();
            self.restart_if_playing();
        }
    }

    fn restart_if_playing(&mut self) {
        self.player.stop();
        self.play_started = None;
        if self.playback == Playback::Playing {
            self.buffering = true;
        }
    }

    fn toggle_play(&mut self) {
        match self.playback {
            Playback::Playing => {
                self.player.pause();
                self.playback = Playback::Paused;
                self.set_status("paused");
            }
            Playback::Paused => {
                if self.player.resume() {
                    self.playback = Playback::Playing;
                    self.set_status("resumed");
                } else {
                    self.playback = Playback::Playing;
                    self.buffering = true;
                }
            }
            Playback::Stopped => {
                if self.book.is_none() {
                    return;
                }
                if !self.player.available() {
                    self.set_status("ffplay not found · install ffmpeg");
                    return;
                }
                self.playback = Playback::Playing;
                self.buffering = true;
                self.set_status("buffering…");
            }
        }
    }

    // --- narration ----------------------------------------------------------

    fn cached_audio(&self, text: &str) -> Option<PathBuf> {
        let (_, cands) = worker_cache_candidates(
            &Config::audio_cache_dir(),
            &self.model,
            &self.voice,
            text,
        );
        cands.into_iter().find(|p| p.is_file())
    }

    fn ensure_synth(&mut self, text: &str) {
        let (_, cands) = worker_cache_candidates(
            &Config::audio_cache_dir(),
            &self.model,
            &self.voice,
            text,
        );
        if cands.iter().any(|p| p.is_file()) {
            return;
        }
        let out = cands[0].clone();
        if self.pending.contains(&out) {
            return;
        }
        self.pending.insert(out.clone());
        let _ = self.synth_tx.send(Job {
            url: self.config.tts.worker_url.clone(),
            model: self.model.clone(),
            voice: self.voice.clone(),
            text: text.to_string(),
            out,
        });
    }

    fn prefetch(&mut self) {
        let mut idx = self.cursor + 1;
        let mut queued = 0;
        while idx < self.flat.len() && queued < 3 {
            if let Some(t) = self.flat[idx].speak.clone() {
                self.ensure_synth(&t);
                queued += 1;
            }
            idx += 1;
        }
    }

    fn tick_narration(&mut self, ctx: &egui::Context) {
        while let Ok((p, d)) = self.results.try_recv() {
            self.durations.insert(p, d);
        }
        if self.playback != Playback::Playing || self.book.is_none() {
            return;
        }
        if self.buffering {
            let Some(text) = self.speak_at(self.cursor) else {
                if self.advance() {
                    ctx.request_repaint();
                } else {
                    self.playback = Playback::Stopped;
                    self.buffering = false;
                    self.set_status("finished");
                }
                return;
            };
            if let Some(path) = self.cached_audio(&text) {
                match self.player.play(&path, self.speed) {
                    Ok(()) => {
                        self.buffering = false;
                        self.status = Some(format!("▶ {}", short_voice(&self.voice)));
                        self.status_until = None;
                        self.play_started = Some(Instant::now());
                        self.play_dur = self.duration_of(&path).unwrap_or(0.0);
                        self.prefetch();
                    }
                    Err(e) => {
                        self.playback = Playback::Stopped;
                        self.buffering = false;
                        self.set_status(format!("audio: {e}"));
                    }
                }
            } else {
                self.ensure_synth(&text);
                ctx.request_repaint_after(Duration::from_millis(150));
            }
            return;
        }
        if self.player.is_playing() {
            // Sleep until the sentence should be over, then re-check. This
            // keeps the gap between sentences small at any speed without
            // polling every frame.
            ctx.request_repaint_after(self.time_until_sentence_end());
            return;
        }
        if self.advance() {
            self.buffering = true;
            self.play_started = None;
            self.prefetch();
            ctx.request_repaint();
        } else {
            self.playback = Playback::Stopped;
            self.set_status("finished");
        }
    }

    /// Time until the current sentence should have finished, using the known
    /// audio duration and the playback speed. Falls back to a short poll.
    fn time_until_sentence_end(&self) -> Duration {
        if self.play_dur > 0.0 {
            if let Some(start) = self.play_started {
                let expected = (self.play_dur / self.speed.max(0.1)) + 0.20;
                let elapsed = start.elapsed().as_secs_f32();
                if elapsed < expected {
                    return Duration::from_secs_f32((expected - elapsed).max(0.02));
                }
            }
        }
        Duration::from_millis(60)
    }

    fn duration_of(&mut self, path: &Path) -> Option<f32> {
        if let Some(d) = self.durations.get(path) {
            return Some(*d);
        }
        let d = probe_duration(path)?;
        self.durations.insert(path.to_path_buf(), d);
        Some(d)
    }

    // --- views --------------------------------------------------------------

    fn ensure_view(&mut self) {
        let Some(book) = self.book.as_ref() else {
            self.view = None;
            return;
        };
        if self.view.is_some()
            && self.view_key.0 == book.id
            && self.view_key.1 == self.chapter_idx
        {
            return;
        }
        let start = self.chapter_start_global();
        let Some(ch) = book.chapters.get(self.chapter_idx) else {
            self.view = None;
            return;
        };
        let mut g = start;
        let mut blocks = Vec::new();
        for b in &ch.blocks {
            let sentences: Vec<(String, usize)> = b
                .sentences
                .iter()
                .map(|s| {
                    let idx = g;
                    g += 1;
                    (s.text.clone(), idx)
                })
                .collect();
            blocks.push(BlockView {
                kind: b.kind,
                sentences,
            });
        }
        let title = ch.title.clone();
        let id = book.id.clone();
        self.view = Some(ChapterView { title, blocks });
        self.view_key = (id, self.chapter_idx);
    }

    fn apply_visuals(&mut self, ctx: &egui::Context) {
        let key = format!("{}|{:?}", self.theme.name, self.font);
        if self.visuals_key == key {
            return;
        }
        self.visuals_key = key;
        let bg = col(&self.theme.background);
        let fg = col(&self.theme.foreground);
        let accent = col(&self.theme.accent);
        let sel = col(&self.theme.selection);
        let border = col(&self.theme.border);
        let muted = col(&self.theme.muted);
        let dark = luminance(bg) < 0.5;
        let mut v = if dark {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        };
        v.panel_fill = bg;
        v.window_fill = bg;
        v.extreme_bg_color = if dark { Color32::from_gray(8) } else { Color32::from_gray(250) };
        v.faint_bg_color = sel;
        v.override_text_color = Some(fg);
        v.hyperlink_color = accent;
        v.selection.bg_fill = accent;
        v.selection.stroke = Stroke::new(1.0, fg);
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, border);
        v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, muted);
        v.widgets.inactive.bg_fill = sel;
        v.widgets.inactive.weak_bg_fill = sel;
        v.widgets.hovered.bg_fill = sel;
        v.widgets.hovered.bg_stroke = Stroke::new(1.0, accent);
        v.widgets.active.bg_fill = accent;
        ctx.set_visuals(v);
    }

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Orpheus").strong().size(15.0));
            if self.book.is_some() {
                ui.separator();
                if ui.button("◀⏮").on_hover_text("previous sentence").clicked() {
                    self.prev_sentence();
                }
                let play = if self.playback == Playback::Playing {
                    "⏸"
                } else {
                    "▶"
                };
                if ui
                    .add(egui::Button::new(RichText::new(play).size(18.0)))
                    .on_hover_text("play / pause")
                    .clicked()
                {
                    self.toggle_play();
                }
                if ui.button("⏭▶").on_hover_text("next sentence").clicked() {
                    self.next_sentence();
                }
                ui.separator();
                if ui.button("ch ‹").on_hover_text("previous chapter").clicked() {
                    self.prev_chapter();
                }
                if ui.button("ch ›").on_hover_text("next chapter").clicked() {
                    self.next_chapter();
                }
                ui.separator();
                let before = self.speed;
                ui.add(
                    egui::Slider::new(&mut self.speed, 0.5..=2.5)
                        .fixed_decimals(2)
                        .suffix("×"),
                );
                if (self.speed - before).abs() > 1e-4 {
                    self.restart_if_playing();
                }
                ui.separator();
                let names = Theme::builtin_names();
                egui::ComboBox::from_id_salt("theme")
                    .selected_text(self.theme_name.clone())
                    .show_ui(ui, |ui| {
                        for n in names {
                            if ui
                                .selectable_label(self.theme_name == n, n)
                                .clicked()
                            {
                                self.theme = Theme::builtin(n);
                                self.theme_name = n.to_string();
                            }
                        }
                    });
                let vsel = self.voice.clone();
                egui::ComboBox::from_id_salt("voice")
                    .selected_text(short_voice(&vsel))
                    .show_ui(ui, |ui| {
                        for v in self.voices.clone() {
                            if ui.selectable_label(self.voice == v, short_voice(&v)).clicked() {
                                self.voice = v;
                                self.player.stop();
                                if self.playback == Playback::Playing {
                                    self.buffering = true;
                                }
                            }
                        }
                    });
                if ui
                    .selectable_label(self.font == FontChoice::Serif, "serif")
                    .clicked()
                {
                    self.font = FontChoice::Serif;
                }
                if ui
                    .selectable_label(self.font == FontChoice::Mono, "mono")
                    .clicked()
                {
                    self.font = FontChoice::Mono;
                }
            }
        });
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if let Some(s) = self.status.clone() {
                ui.label(RichText::new(s).color(col(&self.theme.accent)).size(12.5));
            } else {
                ui.label(
                    RichText::new(
                        "Space play · Left/Right sentence · n/p chapter · drag an .epub here to open",
                    )
                    .color(col(&self.theme.status))
                    .size(12.5),
                );
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    RichText::new(format!("{} · {}", self.model, short_voice(&self.voice)))
                        .color(col(&self.theme.status))
                        .size(12.5),
                );
            });
        });
    }

    fn library_ui(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        ui.label(RichText::new("Library").strong().size(18.0));
        ui.add_space(4.0);

        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.path_input)
                    .hint_text("path to an .epub")
                    .desired_width(420.0),
            );
            if ui.button("Open").clicked() {
                let p = PathBuf::from(self.path_input.trim());
                if p.is_file() {
                    self.open_book(&p);
                } else {
                    self.set_status(format!("not a file: {}", p.display()));
                }
            }
            if ui.button("Rescan ~/Books").clicked() {
                self.scan_books();
            }
        });

        ui.add_space(12.0);
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if !self.recent.is_empty() {
                    ui.label(RichText::new("Continue reading").strong().size(14.0));
                    ui.add_space(4.0);
                    let recent = self.recent.clone();
                    for b in recent {
                        if self.book_row(ui, &b.title, &b.author, b.progress, &b.path) {
                            self.open_book(&PathBuf::from(&b.path));
                        }
                    }
                    ui.add_space(14.0);
                }
                if !self.books_dir.is_empty() {
                    ui.label(RichText::new("In ~/Books").strong().size(14.0));
                    ui.add_space(4.0);
                    let books = self.books_dir.clone();
                    for f in books {
                        let name = f
                            .path
                            .file_stem()
                            .and_then(|s| s.to_str())
                            .unwrap_or("book")
                            .replace(['-', '_'], " ");
                        if self.book_row(ui, &name, "", 0.0, &f.path.to_string_lossy()) {
                            self.open_book(&f.path);
                        }
                    }
                }
                if self.recent.is_empty() && self.books_dir.is_empty() {
                    ui.label(
                        RichText::new("No books yet. Drop an .epub here or type a path above.")
                            .color(col(&self.theme.muted)),
                    );
                }
            });
    }

    /// One clickable library row. Returns true when clicked.
    fn book_row(
        &self,
        ui: &mut egui::Ui,
        title: &str,
        author: &str,
        progress: f64,
        _path: &str,
    ) -> bool {
        let bg = col(&self.theme.background);
        let sel = col(&self.theme.selection);
        let fg = col(&self.theme.foreground);
        let muted = col(&self.theme.muted);
        let accent = col(&self.theme.accent);
        let mut clicked = false;
        egui::Frame::NONE
            .fill(bg)
            .inner_margin(egui::Margin::symmetric(10, 7))
            .show(ui, |ui| {
                let resp = ui
                    .horizontal(|ui| {
                        ui.label(RichText::new(title).color(fg).size(15.0));
                        ui.with_layout(
                            egui::Layout::right_to_left(egui::Align::Center),
                            |ui| {
                                if progress > 0.001 {
                                    ui.label(
                                        RichText::new(format!("{}%", (progress * 100.0) as i32))
                                            .color(accent)
                                            .size(12.5),
                                    );
                                }
                                if !author.is_empty() {
                                    ui.label(
                                        RichText::new(author).color(muted).size(12.5),
                                    );
                                }
                            },
                        );
                    })
                    .response;
                if resp.interact(egui::Sense::click()).hovered() {
                    ui.painter().rect_filled(resp.rect, 4.0, sel);
                }
                let hit = ui.interact(resp.rect, resp.id.with("hit"), egui::Sense::click());
                clicked = hit.clicked();
                if hit.hovered() {
                    ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
                }
            });
        clicked
    }

    fn toc_ui(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        ui.label(RichText::new("Chapters").strong().size(15.0));
        ui.add_space(4.0);
        let titles: Vec<String> = self
            .book
            .as_ref()
            .map(|b| b.chapters.iter().map(|c| c.title.clone()).collect())
            .unwrap_or_default();
        let current = self.chapter_idx;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for (i, t) in titles.iter().enumerate() {
                    let selected = i == current;
                    let label = RichText::new(format!("{}. {}", i + 1, t)).size(13.0);
                    if ui.selectable_label(selected, label).clicked() {
                        self.chapter_idx = i;
                        self.cursor = self.chapter_start_global();
                        self.persist();
                        self.restart_if_playing();
                    }
                }
            });
    }

    fn reader_ui(&mut self, ui: &mut egui::Ui) {
        self.ensure_view();
        if self.view.is_none() {
            ui.centered_and_justified(|ui| {
                ui.label(RichText::new("No book open").color(col(&self.theme.muted)));
            });
            return;
        }
        let theme = self.theme.clone();
        let cursor = self.cursor;
        let do_scroll = self.last_scrolled != Some(cursor);
        let body_px = match self.font {
            FontChoice::Serif => 20.0,
            FontChoice::Mono => 18.0,
        };
        let family = self.body_family();
        let fg = col(&theme.foreground);
        let bg = col(&theme.background);
        let heading = col(&theme.heading);
        let muted = col(&theme.muted);
        let accent = col(&theme.accent);
        let current = col(&theme.current_sentence);
        let selection = col(&theme.selection);
        let border = col(&theme.border);

        let total = self.total().max(1);
        let pct = (cursor as f32 / total as f32 * 100.0) as usize;
        let book_title = self
            .book
            .as_ref()
            .map(|b| b.title.clone())
            .unwrap_or_default();

        let outer_w = ui.available_width();
        let col_w = outer_w.min(760.0);
        let pad = ((outer_w - col_w) / 2.0).max(0.0);
        let Some(view) = self.view.as_ref() else {
            return;
        };

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.add_space(pad);
                    ui.vertical(|ui| {
                        ui.set_width(col_w);
                        // Header: title left, chapter · pct right
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(&book_title).color(muted).size(13.0));
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.label(
                                        RichText::new(format!("{} · {}%", view.title, pct))
                                            .color(muted)
                                            .size(13.0),
                                    );
                                },
                            );
                        });
                        let r = ui.available_rect_before_wrap();
                        ui.painter().hline(
                            r.x_range(),
                            r.top(),
                            Stroke::new(1.0, border),
                        );
                        ui.add_space(14.0);

                        let mut focus_rect: Option<egui::Rect> = None;
                        // Chapter title, centered.
                        ui.vertical_centered(|ui| {
                            ui.label(
                                RichText::new(&view.title)
                                    .color(heading)
                                    .size(body_px + 7.0)
                                    .strong(),
                            );
                        });
                        ui.add_space(16.0);

                        for block in &view.blocks {
                            match block.kind {
                                BlockKind::Heading => {
                                    let t: String = block
                                        .sentences
                                        .iter()
                                        .map(|s| s.0.clone())
                                        .collect::<Vec<_>>()
                                        .join(" ");
                                    if t.trim() == view.title.trim() {
                                        continue;
                                    }
                                    ui.label(
                                        RichText::new(t).color(heading).size(body_px + 3.0).strong(),
                                    );
                                    ui.add_space(8.0);
                                }
                                _ => {
                                    if block.sentences.is_empty() {
                                        continue;
                                    }
                                    let prefix = match block.kind {
                                        BlockKind::Quote => "▍ ",
                                        BlockKind::List => "  • ",
                                        _ => "",
                                    };
                                    let mut job = LayoutJob::default();
                                    job.wrap.max_width = col_w;
                                    if !prefix.is_empty() {
                                        job.append(
                                            prefix,
                                            0.0,
                                            TextFormat {
                                                font_id: FontId::new(body_px, family.clone()),
                                                color: accent,
                                                background: bg,
                                                ..Default::default()
                                            },
                                        );
                                    }
                                    let contains = block
                                        .sentences
                                        .iter()
                                        .any(|(_, idx)| *idx == cursor);
                                    for (i, (text, idx)) in block.sentences.iter().enumerate() {
                                        let is_cur = *idx == cursor;
                                        let f = TextFormat {
                                            font_id: FontId::new(body_px, family.clone()),
                                            color: if is_cur { current } else { fg },
                                            background: if is_cur { selection } else { bg },
                                            ..Default::default()
                                        };
                                        let s = if i == 0 {
                                            text.clone()
                                        } else {
                                            format!(" {text}")
                                        };
                                        job.append(&s, 0.0, f);
                                    }
                                    let galley = ui.ctx().fonts_mut(|fo| fo.layout_job(job));
                                    let resp = ui.add(egui::Label::new(galley));
                                    if contains {
                                        focus_rect = Some(resp.rect);
                                    }
                                    ui.add_space(11.0);
                                }
                            }
                        }
                        if do_scroll {
                            if let Some(r) = focus_rect {
                                ui.scroll_to_rect(r, Some(Align::Center));
                            }
                        }
                        ui.add_space(60.0);
                    });
                });
            });

        if do_scroll {
            self.last_scrolled = Some(cursor);
        }
    }

    fn body_family(&self) -> FontFamily {
        match self.font {
            FontChoice::Serif if self.serif_loaded => FontFamily::Name("book".into()),
            FontChoice::Mono => FontFamily::Monospace,
            _ => FontFamily::Proportional,
        }
    }

    // --- input --------------------------------------------------------------

    fn handle_keys(&mut self, ctx: &egui::Context) {
        let (space, left, right, n_key, p_key) = ctx.input(|i| {
            (
                i.key_pressed(egui::Key::Space),
                i.key_pressed(egui::Key::ArrowLeft),
                i.key_pressed(egui::Key::ArrowRight),
                i.key_pressed(egui::Key::N),
                i.key_pressed(egui::Key::P),
            )
        });
        if space {
            self.toggle_play();
        }
        if left {
            self.prev_sentence();
        }
        if right {
            self.next_sentence();
        }
        if n_key {
            self.next_chapter();
        }
        if p_key {
            self.prev_chapter();
        }
    }

    fn handle_dropped(&mut self, ctx: &egui::Context) {
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        if let Some(f) = dropped.first() {
            let p = f.path().to_path_buf();
            self.open_book(&p);
        }
    }
}

impl eframe::App for OrpheusGui {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.apply_visuals(&ctx);
        self.handle_dropped(&ctx);
        self.handle_keys(&ctx);
        self.tick_narration(&ctx);

        if let Some(t) = self.status_until {
            if Instant::now() >= t {
                self.status = None;
                self.status_until = None;
            }
        }

        let bg = col(&self.theme.background);
        egui::Panel::top("top")
            .frame(egui::Frame::NONE.fill(bg).inner_margin(egui::Margin::symmetric(12, 8)))
            .show(ui, |ui| self.top_bar(ui));
        egui::Panel::bottom("status")
            .frame(egui::Frame::NONE.fill(bg).inner_margin(egui::Margin::symmetric(12, 6)))
            .show(ui, |ui| self.status_bar(ui));

        match self.screen {
            Screen::Library => {
                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE.fill(bg).inner_margin(egui::Margin::same(16)))
                    .show(ui, |ui| self.library_ui(ui));
            }
            Screen::Reader => {
                egui::Panel::left("toc")
                    .resizable(true)
                    .default_size(230.0)
                    .frame(egui::Frame::NONE.fill(bg).inner_margin(egui::Margin::symmetric(12, 8)))
                    .show(ui, |ui| self.toc_ui(ui));
                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE.fill(bg))
                    .show(ui, |ui| self.reader_ui(ui));
            }
        }
    }
}

// --- helpers ----------------------------------------------------------------

fn build_flat(doc: &Document) -> Vec<Flat> {
    let mut v = Vec::new();
    for (ci, ch) in doc.chapters.iter().enumerate() {
        for b in &ch.blocks {
            for s in &b.sentences {
                let sp = s.speak_text.trim();
                v.push(Flat {
                    chapter: ci,
                    speak: if sp.is_empty() {
                        None
                    } else {
                        Some(sp.to_string())
                    },
                });
            }
        }
    }
    v
}

fn short_voice(id: &str) -> String {
    id.strip_suffix("-default").unwrap_or(id).to_string()
}

fn probe_duration(path: &Path) -> Option<f32> {
    let out = std::process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
        ])
        .arg(path)
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout).trim().parse::<f32>().ok()
}

fn col(hex: &str) -> Color32 {
    let s = hex.trim().trim_start_matches('#');
    if s.len() != 6 {
        return Color32::GRAY;
    }
    let p = |i: usize| u8::from_str_radix(&s[i..i + 2], 16).unwrap_or(128);
    Color32::from_rgb(p(0), p(2), p(4))
}

fn luminance(c: Color32) -> f32 {
    (0.299 * c.r() as f32 + 0.587 * c.g() as f32 + 0.114 * c.b() as f32) / 255.0
}

fn install_fonts(ctx: &egui::Context, _choice: FontChoice) -> bool {
    let mut fonts = FontDefinitions::default();
    let candidates = [
        "/usr/share/fonts/noto/NotoSerif-Regular.ttf",
        "/usr/share/fonts/liberation/LiberationSerif-Regular.ttf",
        "/usr/share/fonts/TTF/DejaVuSerif.ttf",
    ];
    let Some(path) = candidates.iter().find(|p| Path::new(p).is_file()) else {
        return false;
    };
    let Ok(bytes) = std::fs::read(path) else {
        return false;
    };
    fonts
        .font_data
        .insert("book".to_owned(), FontData::from_owned(bytes).into());
    fonts.families.insert(
        FontFamily::Name("book".into()),
        vec!["book".to_owned()],
    );
    ctx.set_fonts(fonts);
    true
}
