//! GUIDE §31 — central AppState + screen navigation.
//! No app state lives inside render functions.

use std::path::PathBuf;

use anyhow::Result;
use orpheus_core::{Config, Document, LibraryDb, ReaderStyle, Theme};
use orpheus_tts::TtsManager;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Home,
    Directory,
    Reader,
    Models,
    Voices,
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
        }
    }

    pub fn prev_sentence(&mut self) {
        if self.sentence_cursor > 0 {
            self.sentence_cursor -= 1;
            self.ensure_cursor_visible_chapter();
            self.persist_position();
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
        }
    }

    pub fn toggle_play(&mut self) {
        self.playback = match self.playback {
            PlaybackState::Playing => PlaybackState::Paused,
            _ => PlaybackState::Playing,
        };
        // Real audio queue lands in Milestone 7/8; mock advances highlight.
        self.status_msg = Some(match self.playback {
            PlaybackState::Playing => format!(
                "▶ narrating with {} @ {:.2}x (mock audio until TTS worker lands)",
                self.tts.current_model_id, self.speed
            ),
            PlaybackState::Paused => "⏸ paused".into(),
            PlaybackState::Stopped => "⏹ stopped".into(),
        });
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
            Screen::Home => "Enter open · o directory · t looks · m models · v voices · ? help · q quit".into(),
            Screen::Directory => {
                "Enter open · r rescan · Esc back · q quit".into()
            }
            Screen::Reader => {
                "Space play · ←/→ seek · n/p chapter · j/k scroll · / search · b bookmark · t looks · ? help".into()
            }
            _ => "Esc back · q quit".into(),
        }
    }
}
