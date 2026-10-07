mod app;
mod audio;
mod cli;
mod ui;
mod worker_ctl;

use std::io::stdout;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use clap::Parser;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use tracing_subscriber::EnvFilter;

use app::{App, PlaybackState, Screen};
use cli::{Cli, Commands};

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let cli = Cli::parse();
    let mut config = orpheus_core::Config::load();
    if let Some(t) = &cli.theme {
        config.reader.theme = t.clone();
    }
    if let Some(s) = &cli.style {
        config.reader.style = s.clone();
    }
    if let Some(m) = &cli.model {
        config.tts.model = m.clone();
    }
    if let Some(v) = &cli.voice {
        config.tts.voice = v.clone();
    }

    let db_path = orpheus_core::Config::db_path();
    let db = orpheus_core::LibraryDb::open(&db_path)?;
    let mut app = App::new(config, db);
    // Approach B store: installed models are discovered from
    // `<data>/models/<id>/orpheus-manifest.json`, never from HF cache.
    let model_store = app.model_store.clone();
    app.sync_models_from_store(&model_store);
    app.tts
        .select_model(&app.config.tts.model.clone())
        .unwrap_or(());
    app.tts.current_voice_id = app.config.tts.voice.clone();
    app.speed = app.config.playback.speed;
    app.refresh_recent();

    if cli.worker_check {
        return worker_check(&mut app);
    }

    // CLI routing (GUIDE §29).
    if let Some(cmd) = &cli.command {
        match cmd {
            Commands::Voices => app.goto(Screen::Voices),
            Commands::Models => app.goto(Screen::Models),
            Commands::Config => {
                let p = orpheus_core::Config::path()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "(no config dir)".into());
                println!("config: {p}\n{}", toml::to_string_pretty(&app.config)?);
                return Ok(());
            }
        }
    }
    if let Some(path) = &cli.path {
        if path.is_dir() {
            app.open_directory(path.clone(), true);
        } else if path.is_file() {
            if let Err(e) = app.open_book_file(path) {
                eprintln!("could not open {}: {e:#}", path.display());
            }
        } else {
            eprintln!("path not found: {}", path.display());
        }
    }

    run_tui(&mut app)?;
    // Shut the worker down with the reader (it holds VRAM).
    app.worker.stop();
    // Persist on exit.
    app.persist_position();
    app.config.reader.theme = app.theme.name.clone();
    app.config.reader.style = app.style.name().to_string();
    app.config.playback.speed = app.speed;
    app.config.tts.model = app.tts.current_model_id.clone();
    app.config.tts.voice = app.tts.current_voice_id.clone();
    let _ = app.config.save();
    Ok(())
}

/// Non-interactive verification: spawn the worker for the selected model and
/// report readiness. Mirrors exactly what the reader does on first play.
fn worker_check(app: &mut App) -> Result<()> {
    let model = app.tts.current_model_id.clone();
    if !app.worker_supported() {
        println!("✗ {model}: no worker backend yet (kokoro / chatterbox-turbo supported)");
        return Ok(());
    }
    println!("→ starting book-tts for {model} …");
    worker_ctl::supervise(app);
    if app.worker.running_model.is_none() {
        println!(
            "✗ could not spawn worker: {}",
            app.worker.last_error.clone().unwrap_or_default()
        );
        return Ok(());
    }
    // Poll health like the UI tick does.
    use orpheus_tts::TTSBackend;
    let worker = orpheus_tts::WorkerBackend::new(app.config.tts.worker_url.clone(), model.clone())?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(180);
    let mut ok = false;
    while std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(1500));
        let up = app.rt.block_on(worker.health()).unwrap_or(false);
        if app.worker.is_running() && up {
            ok = true;
            break;
        }
    }
    if ok {
        println!("✓ worker ready on {}", app.config.tts.worker_url);
        match app.rt.block_on(worker.list_voices()) {
            Ok(v) => println!("  voices: {}", v.len()),
            Err(e) => println!("  (voice list unavailable: {e})"),
        }
        app.worker.stop();
        println!("  worker stopped (freeing VRAM)");
    } else {
        println!("✗ worker did not become ready (missing model or env?)");
        app.worker.stop();
    }
    Ok(())
}

fn run_tui(app: &mut App) -> Result<()> {
    enable_raw_mode()?;
    let mut out = stdout();
    execute!(out, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(out);
    let mut term = Terminal::new(backend)?;

    let mut last_tick = std::time::Instant::now();
    let tick_rate = Duration::from_millis(250);

    loop {
        term.draw(|f| ui::render(f, app))?;

        let timeout = tick_rate
            .checked_sub(last_tick.elapsed())
            .unwrap_or_else(|| Duration::from_secs(0));
        if event::poll(timeout)? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                // Ctrl+C always quits.
                if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
                    app.should_quit = true;
                } else {
                    handle_key(app, key.code, key.modifiers);
                }
            }
        }
        if last_tick.elapsed() >= tick_rate {
            on_tick(app);
            last_tick = std::time::Instant::now();
        }
        if app.should_quit {
            break;
        }
    }

    disable_raw_mode()?;
    execute!(term.backend_mut(), LeaveAlternateScreen)?;
    term.show_cursor()?;
    Ok(())
}

/// Narration advance each frame; App owns mock vs real-audio behavior.
fn on_tick(app: &mut App) {
    // Voice-save transcode runs here (not on the key press) so its
    // "normalizing…" message paints first and keys never freeze.
    app.tick_voice_save();
    // Keep exactly one worker alive, serving the selected model.
    worker_ctl::supervise(app);
    // Don't try to play audio while the worker is still loading.
    if !app.worker_waiting {
        app.tick_playback();
    }
}

fn handle_key(app: &mut App, code: KeyCode, mods: KeyModifiers) {
    app.status_msg = None;
    match app.screen {
        Screen::Home => home_key(app, code, mods),
        Screen::Directory => dir_key(app, code, mods),
        Screen::Reader => reader_key(app, code, mods),
        Screen::Models => models_key(app, code, mods),
        Screen::Appearance => appearance_key(app, code, mods),
        Screen::Voices => voices_key(app, code, mods),
        Screen::VoiceEditor => voice_editor_key(app, code, mods),
        Screen::Help => {
            if matches!(code, KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('?')) {
                app.go_back();
            }
        }
        Screen::Search => search_key(app, code, mods),
    }
}

fn home_key(app: &mut App, code: KeyCode, _mods: KeyModifiers) {
    match code {
        KeyCode::Char('q') => app.should_quit = true,
        KeyCode::Char('?') => app.goto(Screen::Help),
        KeyCode::Char('m') => app.goto(Screen::Models),
        KeyCode::Char('v') => app.open_voices(),
        KeyCode::Char('t') => app.open_appearance(),
        KeyCode::Char('o') => {
            let dir = default_books_dir();
            app.open_directory(dir, true);
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if app.home_selected > 0 {
                app.home_selected -= 1;
            }
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if app.home_selected + 1 < app.recent.len() {
                app.home_selected += 1;
            }
        }
        KeyCode::Char('d') => {
            if let Some(b) = app.recent.get(app.home_selected) {
                let id = b.id.clone();
                let _ = app.db.remove_book(&id);
                app.refresh_recent();
            }
        }
        KeyCode::Enter => {
            if let Some(b) = app.recent.get(app.home_selected).cloned() {
                let p = PathBuf::from(&b.path);
                if let Err(e) = app.open_book_file(&p) {
                    app.status_msg = Some(format!("open failed: {e:#}"));
                }
            }
        }
        _ => {}
    }
}

fn dir_key(app: &mut App, code: KeyCode, _mods: KeyModifiers) {
    match code {
        KeyCode::Char('q') => app.should_quit = true,
        KeyCode::Esc => app.goto(Screen::Home),
        KeyCode::Char('?') => app.goto(Screen::Help),
        KeyCode::Up | KeyCode::Char('k') => {
            if app.dir_selected > 0 {
                app.dir_selected -= 1;
            }
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if app.dir_selected + 1 < app.dir_entries.len() {
                app.dir_selected += 1;
            }
        }
        KeyCode::PageDown => {
            app.dir_selected = (app.dir_selected + 10).min(app.dir_entries.len().saturating_sub(1));
        }
        KeyCode::PageUp => {
            app.dir_selected = app.dir_selected.saturating_sub(10);
        }
        KeyCode::Char('r') => {
            if let Some(d) = app.dir_path.clone() {
                app.open_directory(d, true);
            }
        }
        KeyCode::Enter => {
            if let Some(e) = app.dir_entries.get(app.dir_selected).cloned() {
                if let Err(err) = app.open_book_file(&e.path) {
                    app.status_msg = Some(format!("open failed: {err:#}"));
                }
            }
        }
        _ => {}
    }
}

#[allow(clippy::too_many_lines)]
fn reader_key(app: &mut App, code: KeyCode, mods: KeyModifiers) {
    let shift = mods.contains(KeyModifiers::SHIFT);
    match code {
        KeyCode::Char('q') => {
            app.persist_position();
            app.should_quit = true;
        }
        KeyCode::Esc => {
            app.persist_position();
            app.go_back();
        }
        KeyCode::Char('?') => app.goto(Screen::Help),
        KeyCode::Char(' ') => app.toggle_play(),
        KeyCode::Left => {
            if shift {
                app.prev_sentence();
            } else {
                for _ in 0..app.config.playback.seek_seconds.max(1).min(5) {
                    app.prev_sentence();
                }
            }
        }
        KeyCode::Right => {
            if shift {
                app.next_sentence();
            } else {
                for _ in 0..app.config.playback.seek_seconds.max(1).min(5) {
                    app.next_sentence();
                }
            }
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if mods.contains(KeyModifiers::CONTROL) {
                app.scroll = app.scroll.saturating_sub(10);
            } else {
                app.scroll = app.scroll.saturating_sub(1);
            }
        }
        KeyCode::Down | KeyCode::Char('j') => {
            app.scroll = app.scroll.saturating_add(1);
        }
        KeyCode::Char('n') => app.next_chapter(),
        KeyCode::Char('p') => app.prev_chapter(),
        KeyCode::Char('+') | KeyCode::Char('=') => {
            app.set_speed(app.speed + 0.1);
            app.status_msg = Some(format!("speed {:.2}x", app.speed));
            app.restart_audio_if_playing();
        }
        KeyCode::Char('-') | KeyCode::Char('_') => {
            app.set_speed(app.speed - 0.1);
            app.status_msg = Some(format!("speed {:.2}x", app.speed));
            app.restart_audio_if_playing();
        }
        KeyCode::Char('0') => {
            app.set_speed(1.0);
            app.status_msg = Some("speed 1.00x".into());
            app.restart_audio_if_playing();
        }
        KeyCode::Char('/') => {
            app.search_query.clear();
            app.searching = true;
            app.goto(Screen::Search);
        }
        KeyCode::Char('b') => {
            if let Some(book) = &app.book {
                let _ = app.db.add_bookmark(
                    &book.id,
                    app.chapter_idx as i64,
                    app.sentence_cursor as i64,
                    "",
                );
                app.status_msg = Some("★ bookmarked".into());
            }
        }
        KeyCode::Char('v') => app.open_voices(),
        KeyCode::Char('m') => app.goto(Screen::Models),
        KeyCode::Char('t') => app.open_appearance(),
        KeyCode::Char('g') => {
            app.scroll = 0;
            app.chapter_idx = 0;
            app.rebuild_chapter_lines();
            app.snap_cursor_to_chapter();
            app.sentence_cursor = app.chapter_start_global();
            app.restart_audio_if_playing();
        }
        KeyCode::Char('G') => {
            if let Some(b) = &app.book {
                app.chapter_idx = b.chapters.len().saturating_sub(1);
                app.rebuild_chapter_lines();
                app.snap_cursor_to_chapter();
                app.restart_audio_if_playing();
            }
        }
        _ => {}
    }
}

fn models_key(app: &mut App, code: KeyCode, _mods: KeyModifiers) {
    if app.model_typing {
        match code {
            KeyCode::Esc => app.model_typing = false,
            KeyCode::Enter => {
                let id = app.model_input.trim().to_string();
                if !id.is_empty() {
                    // "Type a model name and pull it on the fly" (user req):
                    // register immediately, select it, mark for worker pull.
                    let _ = app.tts.select_model(&id);
                    app.player.stop();
                    app.buffering = false;
                    if app.playback == PlaybackState::Playing {
                        app.playback = PlaybackState::Paused;
                    }
                    app.status_msg = Some(format!(
                        "model '{id}' queued — worker will pull on first synthesis (M5/6)"
                    ));
                    app.model_input.clear();
                    app.model_typing = false;
                    app.goto(Screen::Reader);
                }
            }
            KeyCode::Backspace => {
                app.model_input.pop();
            }
            KeyCode::Char(c) => app.model_input.push(c),
            _ => {}
        }
        return;
    }
    let n = app.tts.list_models().len();
    match code {
        KeyCode::Esc | KeyCode::Char('q') => app.go_back(),
        KeyCode::Up | KeyCode::Char('k') => {
            if app.model_selected > 0 {
                app.model_selected -= 1;
            }
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if app.model_selected + 1 < n {
                app.model_selected += 1;
            }
        }
        KeyCode::Enter => {
            let models = app.tts.list_models();
            if let Some(m) = models.get(app.model_selected) {
                let id = m.id.clone();
                let _ = app.tts.select_model(&id);
                app.tts.current_voice_id = format!("{id}-default");
                // Different engine, different audio: stop, press Space to start.
                app.player.stop();
                app.buffering = false;
                // Model switch frees/reloads VRAM: the supervisor handles it
                // on the next tick (kill old worker, spawn for the new model).
                app.worker_waiting = false;
                if app.playback == PlaybackState::Playing {
                    app.playback = PlaybackState::Paused;
                }
                app.status_msg = Some(format!("model → {id} (position kept)"));
                app.go_back();
            }
        }
        KeyCode::Char('/') | KeyCode::Char('i') => {
            app.model_typing = true;
            app.model_input.clear();
        }
        _ => {}
    }
}

fn appearance_key(app: &mut App, code: KeyCode, _mods: KeyModifiers) {
    let theme_count = orpheus_core::Theme::builtin_names().len();
    let style_count = orpheus_core::ReaderStyle::all().len();
    match code {
        KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('t') => app.go_back(),
        KeyCode::Tab | KeyCode::BackTab => {
            app.appearance_section = (app.appearance_section + 1) % 2;
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if app.appearance_section == 0 {
                if app.theme_selected > 0 {
                    app.theme_selected -= 1;
                }
                // Live preview: apply immediately.
                app.apply_appearance_selection();
            } else if app.style_selected > 0 {
                app.style_selected -= 1;
                app.apply_appearance_selection();
            }
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if app.appearance_section == 0 {
                if app.theme_selected + 1 < theme_count {
                    app.theme_selected += 1;
                }
                app.apply_appearance_selection();
            } else if app.style_selected + 1 < style_count {
                app.style_selected += 1;
                app.apply_appearance_selection();
            }
        }
        KeyCode::Enter | KeyCode::Char(' ') => {
            app.apply_appearance_selection();
            app.go_back();
        }
        _ => {}
    }
}

fn voices_key(app: &mut App, code: KeyCode, _mods: KeyModifiers) {
    let n = app.voice_list.len();
    match code {
        KeyCode::Esc | KeyCode::Char('q') => app.go_back(),
        KeyCode::Up | KeyCode::Char('k') => {
            if app.voice_selected > 0 {
                app.voice_selected -= 1;
            }
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if app.voice_selected + 1 < n {
                app.voice_selected += 1;
            }
        }
        KeyCode::PageDown => {
            app.voice_selected = (app.voice_selected + 10).min(n.saturating_sub(1));
        }
        KeyCode::PageUp => {
            app.voice_selected = app.voice_selected.saturating_sub(10);
        }
        KeyCode::Enter => app.select_voice(),
        KeyCode::Char('a') => app.open_voice_editor(),
        KeyCode::Char('p') => app.preview_selected_voice(),
        KeyCode::Char('d') => app.delete_selected_voice(),
        _ => {}
    }
}

fn voice_editor_key(app: &mut App, code: KeyCode, _mods: KeyModifiers) {
    match code {
        KeyCode::Esc => {
            app.pending_voice_save = None; // drop a staged save, if any
            app.open_voices(); // cancel back to the list
        }
        KeyCode::Tab | KeyCode::Down | KeyCode::Up => {
            app.ve_field = (app.ve_field + 1) % 2;
        }
        KeyCode::Enter => {
            // Enter on the name field moves to the file field; Enter on the
            // file field runs instant checks and stages the transcode, which
            // tick_voice_save performs after the UI repaints (no freeze).
            if app.ve_field == 0 {
                app.ve_field = 1;
            } else {
                match app.prepare_voice_save() {
                    Ok(pending) => {
                        app.pending_voice_save = Some(pending);
                        app.status_msg = Some("normalizing… (a few seconds, Esc backs out)".into());
                    }
                    Err(e) => app.status_msg = Some(e),
                }
            }
        }
        KeyCode::Backspace => {
            if app.ve_field == 0 {
                app.ve_name.pop();
            } else {
                app.ve_path.pop();
            }
        }
        KeyCode::Char(c) => {
            if app.ve_field == 0 {
                app.ve_name.push(c);
            } else {
                app.ve_path.push(c);
            }
        }
        _ => {}
    }
}

fn search_key(app: &mut App, code: KeyCode, _mods: KeyModifiers) {
    match code {
        KeyCode::Esc => {
            app.searching = false;
            app.go_back();
        }
        KeyCode::Enter => {
            // Jump to first hit.
            let q = app.search_query.to_lowercase();
            if let Some(book) = &app.book {
                // Walk flat sentences.
                let flat = book.flat_sentences();
                for (idx, (ci, _, _, s)) in flat.iter().enumerate() {
                    if s.text.to_lowercase().contains(&q) {
                        app.chapter_idx = *ci;
                        app.rebuild_chapter_lines();
                        app.sentence_cursor = idx;
                        app.persist_position();
                        app.restart_audio_if_playing();
                        break;
                    }
                }
            }
            app.searching = false;
            app.goto(Screen::Reader);
        }
        KeyCode::Backspace => {
            app.search_query.pop();
        }
        KeyCode::Char(c) => app.search_query.push(c),
        _ => {}
    }
}

fn default_books_dir() -> PathBuf {
    if let Some(home) = dirs::home_dir() {
        let books = home.join("Books");
        if books.is_dir() {
            return books;
        }
    }
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}
