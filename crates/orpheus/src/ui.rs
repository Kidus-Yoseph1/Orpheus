//! Screen rendering. Pure functions of `App`; no state mutation here.
//!
//! Design goal: feel like Foliate — a calm book page, not a dashboard.
//! - Narrow centered column (≈76 chars), generous side margins.
//! - Paragraphs flow; sentences share a line, current one highlighted inline.
//! - No heavy boxes; one hairline rule under the header, one above footer.
//! - Theme fills the whole screen background.

use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line as RLine, Span, Text},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
    Frame,
};

use crate::app::{App, Screen};

pub fn render(f: &mut Frame, app: &App) {
    // Paint the full screen with the theme background first so light
    // themes (paper/sepia) actually look like paper edge-to-edge.
    let bg = Block::default().style(Style::default().bg(app.theme.bg()));
    f.render_widget(bg, f.area());

    match app.screen {
        Screen::Home => render_home(f, app),
        Screen::Directory => render_directory(f, app),
        Screen::Reader => render_reader(f, app),
        Screen::Models => render_models(f, app),
        Screen::Voices => render_voices(f, app),
        Screen::VoiceEditor => render_voice_editor(f, app),
        Screen::Appearance => render_appearance(f, app),
        Screen::Help => render_help(f, app),
        Screen::Search => render_search(f, app),
    }
}

/// Narrow centered column like a book page. Max ~78 chars wide.
fn book_column(area: Rect, max_width: u16) -> Rect {
    let width = area.width.min(max_width);
    let margin = area.width.saturating_sub(width) / 2;
    Rect {
        x: area.x + margin,
        y: area.y,
        width,
        height: area.height,
    }
}

fn muted(app: &App) -> Style {
    Style::default().fg(app.theme.muted_c()).bg(app.theme.bg())
}

// --- Home (library, Foliate-style shelf list) ----------------------------------

fn render_home(f: &mut Frame, app: &App) {
    let col = book_column(f.area(), 72);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Length(6),
            Constraint::Length(2),
            Constraint::Min(4),
            Constraint::Length(2),
        ])
        .split(col);

    // Title block, centered like a book app.
    let title = Paragraph::new(Text::from(vec![
        RLine::from(vec![Span::styled(
            "ORPHEUS",
            Style::default()
                .fg(app.theme.heading_c())
                .bg(app.theme.bg())
                .add_modifier(Modifier::BOLD),
        )])
        .alignment(Alignment::Center),
        RLine::from(vec![Span::styled(
            "a quiet library · read & listen",
            Style::default().fg(app.theme.muted_c()).bg(app.theme.bg()),
        )])
        .alignment(Alignment::Center),
    ]));
    f.render_widget(title, chunks[0]);

    // Continue reading — single subtle card, no double boxes.
    if let Some(first) = app.recent.first() {
        let pct = (first.progress * 100.0).clamp(0.0, 100.0);
        let bar = text_progress_bar(pct / 100.0, 28);
        let card = Paragraph::new(Text::from(vec![
            RLine::from(vec![
                Span::styled(
                    "  ▶  ",
                    Style::default().fg(app.theme.accent_c()).bg(app.theme.bg()),
                ),
                Span::styled(
                    first.title.clone(),
                    Style::default()
                        .fg(app.theme.fg())
                        .bg(app.theme.bg())
                        .add_modifier(Modifier::BOLD),
                ),
            ]),
            RLine::from(vec![Span::styled(
                format!(
                    "      {} · chapter {} · {}%",
                    first.author,
                    first.chapter_idx + 1,
                    pct as usize
                ),
                muted(app),
            )]),
            RLine::from(vec![Span::styled(format!("      {bar}"), muted(app))]),
        ]))
        .block(
            Block::default()
                .title(" Continue reading ")
                .title_style(Style::default().fg(app.theme.muted_c()).bg(app.theme.bg()))
                .borders(Borders::ALL)
                .border_style(Style::default().fg(app.theme.border_c()).bg(app.theme.bg()))
                .style(Style::default().bg(app.theme.bg())),
        );
        f.render_widget(card, chunks[1]);
    } else {
        let card = Paragraph::new(Text::from(RLine::from(vec![Span::styled(
            "  No books yet — press  o  to open a folder, or run  orpheus ~/Books",
            muted(app),
        )])))
        .block(
            Block::default()
                .title(" Continue reading ")
                .title_style(Style::default().fg(app.theme.muted_c()).bg(app.theme.bg()))
                .borders(Borders::ALL)
                .border_style(Style::default().fg(app.theme.border_c()).bg(app.theme.bg()))
                .style(Style::default().bg(app.theme.bg())),
        );
        f.render_widget(card, chunks[1]);
    }

    f.render_widget(
        Paragraph::new(RLine::from(vec![Span::styled(
            "  Recent",
            Style::default()
                .fg(app.theme.heading_c())
                .bg(app.theme.bg())
                .add_modifier(Modifier::BOLD),
        )]))
        .style(Style::default().bg(app.theme.bg())),
        chunks[2],
    );

    let items: Vec<ListItem> = if app.recent.is_empty() {
        vec![ListItem::new(RLine::from(vec![Span::styled(
            "  (empty)",
            muted(app),
        )]))]
    } else {
        app.recent
            .iter()
            .enumerate()
            .map(|(i, b)| {
                let pct = (b.progress * 100.0).clamp(0.0, 100.0) as usize;
                if i == app.home_selected {
                    ListItem::new(RLine::from(vec![
                        Span::styled(
                            " › ",
                            Style::default()
                                .fg(app.theme.accent_c())
                                .bg(app.theme.selection_c())
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(
                            format!("{}  ", b.title),
                            Style::default()
                                .fg(app.theme.current_c())
                                .bg(app.theme.selection_c())
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(
                            format!("{} · {}%", b.author, pct),
                            Style::default()
                                .fg(app.theme.muted_c())
                                .bg(app.theme.selection_c()),
                        ),
                    ]))
                } else {
                    ListItem::new(RLine::from(vec![
                        Span::styled("   ", Style::default().bg(app.theme.bg())),
                        Span::styled(
                            format!("{}  ", b.title),
                            Style::default().fg(app.theme.fg()).bg(app.theme.bg()),
                        ),
                        Span::styled(
                            format!("{} · {}%", b.author, pct),
                            Style::default().fg(app.theme.muted_c()).bg(app.theme.bg()),
                        ),
                    ]))
                }
            })
            .collect()
    };
    scroll_list(f, chunks[3], items, app.home_selected, app);
    f.render_widget(
        footer_msg(
            app,
            "enter open · o folder · t appearance · m models · v voices · ? help · q quit",
        ),
        chunks[4],
    );
}

// --- Directory ------------------------------------------------------------------

fn render_directory(f: &mut Frame, app: &App) {
    let col = book_column(f.area(), 76);
    let dir = app
        .dir_path
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "(no directory)".into());
    let short_dir = truncate_middle(&dir, 52);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(4),
            Constraint::Length(2),
        ])
        .split(col);

    f.render_widget(
        Paragraph::new(Text::from(vec![
            RLine::from(vec![Span::styled(
                short_dir,
                Style::default().fg(app.theme.muted_c()).bg(app.theme.bg()),
            )])
            .alignment(Alignment::Center),
            RLine::from(vec![Span::styled(hairline(col.width), muted(app))]),
        ]))
        .style(Style::default().bg(app.theme.bg())),
        chunks[0],
    );

    let items: Vec<ListItem> = app
        .dir_entries
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let name = e
                .path
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            if i == app.dir_selected {
                ListItem::new(RLine::from(vec![
                    Span::styled(
                        " › ",
                        Style::default()
                            .fg(app.theme.accent_c())
                            .bg(app.theme.selection_c())
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        name,
                        Style::default()
                            .fg(app.theme.current_c())
                            .bg(app.theme.selection_c())
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!("  {}", e.format),
                        Style::default()
                            .fg(app.theme.muted_c())
                            .bg(app.theme.selection_c()),
                    ),
                ]))
            } else {
                ListItem::new(RLine::from(vec![
                    Span::styled("   ", Style::default().bg(app.theme.bg())),
                    Span::styled(name, Style::default().fg(app.theme.fg()).bg(app.theme.bg())),
                    Span::styled(
                        format!("  {}", e.format),
                        Style::default().fg(app.theme.muted_c()).bg(app.theme.bg()),
                    ),
                ]))
            }
        })
        .collect();
    let items = if items.is_empty() {
        vec![ListItem::new(RLine::from(vec![Span::styled(
            "  No .epub / .pdf here — press r to rescan",
            muted(app),
        )]))]
    } else {
        items
    };
    scroll_list(f, chunks[1], items, app.dir_selected, app);
    f.render_widget(
        footer_msg(app, "enter open · r rescan · esc back"),
        chunks[2],
    );
}

// --- Reader (Foliate-style page) -------------------------------------------------

fn render_reader(f: &mut Frame, app: &App) {
    let Some(book) = &app.book else {
        let area = centered(f.area(), 60, 30);
        f.render_widget(
            Paragraph::new("No book open.")
                .style(Style::default().bg(app.theme.bg()).fg(app.theme.fg())),
            area,
        );
        return;
    };
    let ch = &book.chapters[app.chapter_idx.min(book.chapters.len() - 1)];
    let pct = (app.progress() * 100.0).clamp(0.0, 100.0);
    let minimal = app.style == orpheus_core::theme::ReaderStyle::Minimal;

    // Page column: slightly wider than home for comfortable measure.
    let col = book_column(f.area(), 78);

    let mut constraints = vec![];
    if !minimal {
        constraints.push(Constraint::Length(3)); // header: title + rule
    }
    constraints.push(Constraint::Min(5)); // body
    if !minimal {
        constraints.push(Constraint::Length(3)); // footer: bar + info
    } else {
        constraints.push(Constraint::Length(1)); // slim progress only
    }
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(col);

    let mut idx = 0;
    if !minimal {
        render_reader_header(
            f,
            app,
            book.title.clone(),
            ch.title.clone(),
            pct,
            chunks[idx],
        );
        idx += 1;
    }

    render_reader_body(f, app, chunks[idx]);
    idx += 1;
    render_reader_footer(f, app, pct, chunks[idx], minimal);
}

fn render_reader_header(
    f: &mut Frame,
    app: &App,
    book_title: String,
    chapter_title: String,
    pct: f32,
    area: Rect,
) {
    let left = truncate_middle(&format!("{book_title}"), 34);
    let right = format!(
        "{} · {}%",
        truncate_middle(&chapter_title, 30),
        pct as usize
    );
    // Two-column header with hairline rule beneath.
    let top = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(10), Constraint::Length(40)])
        .split(Rect { height: 1, ..area });
    f.render_widget(
        Paragraph::new(RLine::from(vec![Span::styled(
            left,
            Style::default().fg(app.theme.muted_c()).bg(app.theme.bg()),
        )]))
        .style(Style::default().bg(app.theme.bg())),
        top[0],
    );
    f.render_widget(
        Paragraph::new(
            RLine::from(vec![Span::styled(
                right,
                Style::default().fg(app.theme.muted_c()).bg(app.theme.bg()),
            )])
            .alignment(Alignment::Right),
        )
        .style(Style::default().bg(app.theme.bg())),
        top[1],
    );
    let rule_area = Rect {
        y: area.y + 1,
        height: 1,
        ..area
    };
    f.render_widget(
        Paragraph::new(
            RLine::from(vec![Span::styled(hairline(area.width), muted(app))])
                .alignment(Alignment::Center),
        )
        .style(Style::default().bg(app.theme.bg())),
        rule_area,
    );
    // Small breathing room is the third row (empty, bg-filled).
    let gap = Rect {
        y: area.y + 2,
        height: 1,
        ..area
    };
    f.render_widget(
        Block::default().style(Style::default().bg(app.theme.bg())),
        gap,
    );
}

fn render_reader_body(f: &mut Frame, app: &App, area: Rect) {
    let Some(book) = &app.book else { return };
    let Some(ch) = book.chapters.get(app.chapter_idx) else {
        return;
    };

    let focus = app.style == orpheus_core::theme::ReaderStyle::Focus;
    let cursor = app.sentence_cursor;

    // Global sentence offset of this chapter for cursor comparison.
    let chapter_start = app.chapter_start_global();

    let mut rlines: Vec<RLine> = Vec::new();
    let mut global = chapter_start;

    // Chapter title as centered display heading (Foliate-style).
    rlines.push(RLine::from(""));
    rlines.push(
        RLine::from(vec![Span::styled(
            ch.title.clone(),
            Style::default()
                .fg(app.theme.heading_c())
                .bg(app.theme.bg())
                .add_modifier(Modifier::BOLD),
        )])
        .alignment(Alignment::Center),
    );
    rlines.push(RLine::from(""));

    for block in &ch.blocks {
        use orpheus_core::document::BlockKind;
        match block.kind {
            BlockKind::Heading => {
                // Skip duplicate: chapter title already shown if identical.
                if block.text.trim() == ch.title.trim() {
                    global += block.sentences.len();
                    continue;
                }
                rlines.push(RLine::from(vec![Span::styled(
                    block.text.clone(),
                    Style::default()
                        .fg(app.theme.heading_c())
                        .bg(app.theme.bg())
                        .add_modifier(Modifier::BOLD),
                )]));
                rlines.push(RLine::from(""));
                global += block.sentences.len();
            }
            BlockKind::Quote => {
                let spans = sentence_spans(app, &block.sentences, global, true);
                global += block.sentences.len();
                let mut line = RLine::from(vec![Span::styled(
                    "▍ ",
                    Style::default().fg(app.theme.accent_c()).bg(app.theme.bg()),
                )]);
                line.spans.extend(spans);
                rlines.push(line);
                rlines.push(RLine::from(""));
            }
            BlockKind::List => {
                let spans = sentence_spans(app, &block.sentences, global, false);
                global += block.sentences.len();
                let mut line = RLine::from(vec![Span::styled(
                    "  • ",
                    Style::default().fg(app.theme.accent_c()).bg(app.theme.bg()),
                )]);
                line.spans.extend(spans);
                rlines.push(line);
                rlines.push(RLine::from(""));
            }
            BlockKind::Paragraph => {
                if block.sentences.is_empty() {
                    continue;
                }
                // In focus mode, dim paragraphs far from the cursor.
                let contains_cursor = (global..global + block.sentences.len()).contains(&cursor);
                if focus && !contains_cursor {
                    // Show only paragraphs near cursor; others collapse.
                    let cursor_block = cursor_block_index(ch, cursor, chapter_start);
                    let this_block = rlines.len();
                    let _ = (cursor_block, this_block);
                    // Approximate: keep heading + neighbours by distance in sentences.
                    let dist = global.abs_diff(cursor);
                    if dist > 6 {
                        global += block.sentences.len();
                        continue;
                    }
                }
                let dimmed = focus && !contains_cursor;
                let spans = sentence_spans_dim(app, &block.sentences, global, dimmed);
                global += block.sentences.len();
                rlines.push(RLine::from(spans));
                rlines.push(RLine::from(""));
            }
        }
    }

    let para = Paragraph::new(Text::from(rlines))
        .wrap(Wrap { trim: false })
        .scroll((app.scroll, 0))
        .style(Style::default().bg(app.theme.bg()).fg(app.theme.fg()));
    f.render_widget(para, area);
}

/// One span per sentence inside a paragraph; only the spoken sentence glows.
fn sentence_spans(
    app: &App,
    sentences: &[orpheus_core::Sentence],
    global_start: usize,
    _is_quote: bool,
) -> Vec<Span<'static>> {
    sentence_spans_dim(app, sentences, global_start, false)
}

fn sentence_spans_dim(
    app: &App,
    sentences: &[orpheus_core::Sentence],
    global_start: usize,
    dimmed: bool,
) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    for (i, s) in sentences.iter().enumerate() {
        let g = global_start + i;
        let is_current = g == app.sentence_cursor;
        let style = if is_current {
            Style::default()
                .fg(app.theme.current_c())
                .bg(app.theme.selection_c())
                .add_modifier(Modifier::BOLD)
        } else if dimmed {
            Style::default().fg(app.theme.muted_c()).bg(app.theme.bg())
        } else {
            Style::default().fg(app.theme.fg()).bg(app.theme.bg())
        };
        // Preserve a space between sentences (but not before first).
        let prefix = if i == 0 { "" } else { " " };
        spans.push(Span::styled(format!("{prefix}{}", s.text), style));
    }
    spans
}

/// Which block (by order) holds the cursor — for focus windowing.
fn cursor_block_index(ch: &orpheus_core::Chapter, cursor: usize, chapter_start: usize) -> usize {
    let mut g = chapter_start;
    for (bi, b) in ch.blocks.iter().enumerate() {
        let n = b.sentences.len();
        if cursor >= g && cursor < g + n.max(1) {
            return bi;
        }
        g += n;
    }
    0
}

fn render_reader_footer(f: &mut Frame, app: &App, pct: f32, area: Rect, minimal: bool) {
    if minimal {
        // Single slim bar, nothing else.
        let bar = text_progress_bar(pct as f64 / 100.0, area.width.saturating_sub(2));
        f.render_widget(
            Paragraph::new(
                RLine::from(vec![Span::styled(bar, muted(app))]).alignment(Alignment::Center),
            )
            .style(Style::default().bg(app.theme.bg())),
            area,
        );
        return;
    }
    let Some(book) = &app.book else { return };
    let bar_width = area.width.saturating_sub(10).max(20);
    let bar = text_progress_bar(pct as f64 / 100.0, bar_width);

    let play_icon = match app.playback {
        crate::app::PlaybackState::Playing => "▶",
        crate::app::PlaybackState::Paused => "❚❚",
        crate::app::PlaybackState::Stopped => "○",
    };
    let left = format!(
        "{play_icon} {:.2}× · {} · {}",
        app.speed,
        short_voice(&app.tts.current_voice_id),
        app.tts.current_model_id
    );
    let right = format!(
        "ch {}/{} · {}%",
        app.chapter_idx + 1,
        book.chapters.len(),
        pct as usize
    );

    let top = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(area);

    f.render_widget(
        Paragraph::new(
            RLine::from(vec![Span::styled(
                format!("  {bar}"),
                Style::default()
                    .fg(app.theme.progress_c())
                    .bg(app.theme.bg()),
            )])
            .alignment(Alignment::Center),
        )
        .style(Style::default().bg(app.theme.bg())),
        top[0],
    );

    let info = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(10), Constraint::Length(22)])
        .split(top[1]);
    f.render_widget(
        Paragraph::new(RLine::from(vec![Span::styled(
            format!("  {left}"),
            muted(app),
        )]))
        .style(Style::default().bg(app.theme.bg())),
        info[0],
    );
    f.render_widget(
        Paragraph::new(
            RLine::from(vec![Span::styled(right, muted(app))]).alignment(Alignment::Right),
        )
        .style(Style::default().bg(app.theme.bg())),
        info[1],
    );
    if let Some(msg) = &app.status_msg {
        // Transient feedback (buffering…, audio errors, speed, bookmarks…)
        // takes over the hint line so it is never invisible.
        f.render_widget(
            Paragraph::new(
                RLine::from(vec![Span::styled(
                    format!("  {msg}"),
                    Style::default()
                        .fg(app.theme.accent_c())
                        .bg(app.theme.bg())
                        .add_modifier(Modifier::BOLD),
                )])
                .alignment(Alignment::Center),
            )
            .style(Style::default().bg(app.theme.bg())),
            top[2],
        );
    } else {
        f.render_widget(
            hint_bar(
                app,
                "space play · ←/→ sentence · n/p chapter · j/k scroll · / find · t looks · ? help",
            ),
            top[2],
        );
    }
}

fn short_voice(id: &str) -> String {
    id.strip_suffix("-default").unwrap_or(id).to_string()
}

// --- Appearance (themes + reader styles) ----------------------------------------

fn render_appearance(f: &mut Frame, app: &App) {
    let col = book_column(f.area(), 68);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(6),
            Constraint::Length(3),
            Constraint::Min(5),
            Constraint::Length(2),
        ])
        .split(col);

    f.render_widget(
        Paragraph::new(Text::from(vec![
            RLine::from(vec![Span::styled(
                "Appearance",
                Style::default()
                    .fg(app.theme.heading_c())
                    .bg(app.theme.bg())
                    .add_modifier(Modifier::BOLD),
            )])
            .alignment(Alignment::Center),
            RLine::from(vec![Span::styled(
                "themes change color · styles change layout",
                muted(app),
            )])
            .alignment(Alignment::Center),
        ]))
        .style(Style::default().bg(app.theme.bg())),
        chunks[0],
    );

    // Themes section.
    let theme_names = orpheus_core::Theme::builtin_names();
    let theme_items: Vec<ListItem> = theme_names
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let t = orpheus_core::Theme::builtin(name);
            let focused = app.appearance_section == 0 && i == app.theme_selected;
            let current = *name == app.theme.name;
            let marker = if current {
                "● "
            } else if focused {
                "› "
            } else {
                "  "
            };
            // Preview swatch: "Aa" painted in that theme's own colors on its bg.
            let swatch = format!(" Aa ");
            if focused {
                ListItem::new(RLine::from(vec![
                    Span::styled(
                        marker,
                        Style::default()
                            .fg(app.theme.accent_c())
                            .bg(app.theme.selection_c())
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!("{name}"),
                        Style::default()
                            .fg(app.theme.current_c())
                            .bg(app.theme.selection_c())
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled("  ", Style::default().bg(app.theme.selection_c())),
                    Span::styled(
                        swatch,
                        Style::default()
                            .fg(hex_color(&t.foreground))
                            .bg(hex_color(&t.background)),
                    ),
                    Span::styled(
                        format!("  {}", t.background),
                        Style::default()
                            .fg(app.theme.muted_c())
                            .bg(app.theme.selection_c()),
                    ),
                ]))
            } else {
                ListItem::new(RLine::from(vec![
                    Span::styled(
                        marker,
                        Style::default().fg(app.theme.accent_c()).bg(app.theme.bg()),
                    ),
                    Span::styled(
                        format!("{name}"),
                        Style::default().fg(app.theme.fg()).bg(app.theme.bg()),
                    ),
                    Span::styled("  ", Style::default().bg(app.theme.bg())),
                    Span::styled(
                        swatch,
                        Style::default()
                            .fg(hex_color(&t.foreground))
                            .bg(hex_color(&t.background)),
                    ),
                    Span::styled(format!("  {}", t.background), muted(app)),
                ]))
            }
        })
        .collect();
    let theme_title = if app.appearance_section == 0 {
        "▶ Themes (enter applies)"
    } else {
        "  Themes"
    };
    f.render_widget(
        Paragraph::new("").style(Style::default().bg(app.theme.bg())),
        chunks[1],
    );
    // Render section header + list manually stacked.
    let theme_area = chunks[1];
    let theme_inner = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(3)])
        .split(theme_area);
    f.render_widget(
        Paragraph::new(RLine::from(vec![Span::styled(
            theme_title,
            Style::default()
                .fg(app.theme.heading_c())
                .bg(app.theme.bg())
                .add_modifier(Modifier::BOLD),
        )]))
        .style(Style::default().bg(app.theme.bg())),
        theme_inner[0],
    );
    scroll_list(f, theme_inner[1], theme_items, app.theme_selected, app);

    // Live preview line in the *currently selected-for-cursor* theme.
    let preview_theme = theme_names
        .get(app.theme_selected)
        .map(|n| orpheus_core::Theme::builtin(n))
        .unwrap_or_else(|| app.theme.clone());
    f.render_widget(
        Paragraph::new(RLine::from(vec![
            Span::styled("  Preview: ", muted(app)),
            Span::styled(
                "The desert was vast and silent. ",
                Style::default()
                    .fg(hex_color(&preview_theme.foreground))
                    .bg(hex_color(&preview_theme.background)),
            ),
            Span::styled(
                "Paul looked up.",
                Style::default()
                    .fg(hex_color(&preview_theme.current_sentence))
                    .bg(hex_color(&preview_theme.selection))
                    .add_modifier(Modifier::BOLD),
            ),
        ]))
        .style(Style::default().bg(app.theme.bg()))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(app.theme.border_c()).bg(app.theme.bg())),
        ),
        chunks[2],
    );

    // Styles section.
    let styles = orpheus_core::ReaderStyle::all();
    let style_items: Vec<ListItem> = styles
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let focused = app.appearance_section == 1 && i == app.style_selected;
            let current = *s == app.style;
            let marker = if current {
                "● "
            } else if focused {
                "› "
            } else {
                "  "
            };
            let desc = match s {
                orpheus_core::theme::ReaderStyle::Classic => "traditional margins",
                orpheus_core::theme::ReaderStyle::Paper => "warm & spacious (default)",
                orpheus_core::theme::ReaderStyle::Sepia => "old paperback",
                orpheus_core::theme::ReaderStyle::Midnight => "dark & elegant",
                orpheus_core::theme::ReaderStyle::Focus => "only sentences near narration",
                orpheus_core::theme::ReaderStyle::Minimal => "text only, no chrome",
            };
            let style = if focused {
                Style::default()
                    .fg(app.theme.current_c())
                    .bg(app.theme.selection_c())
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(app.theme.fg()).bg(app.theme.bg())
            };
            let sub = if focused {
                Style::default()
                    .fg(app.theme.muted_c())
                    .bg(app.theme.selection_c())
            } else {
                muted(app)
            };
            ListItem::new(RLine::from(vec![
                Span::styled(marker, style),
                Span::styled(s.name(), style),
                Span::styled(format!("  · {desc}"), sub),
            ]))
        })
        .collect();
    let style_title = if app.appearance_section == 1 {
        "▶ Reader styles (enter applies)"
    } else {
        "  Reader styles"
    };
    let style_area = chunks[3];
    let style_inner = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(3)])
        .split(style_area);
    f.render_widget(
        Paragraph::new(RLine::from(vec![Span::styled(
            style_title,
            Style::default()
                .fg(app.theme.heading_c())
                .bg(app.theme.bg())
                .add_modifier(Modifier::BOLD),
        )]))
        .style(Style::default().bg(app.theme.bg())),
        style_inner[0],
    );
    scroll_list(f, style_inner[1], style_items, app.style_selected, app);

    f.render_widget(
        footer_msg(app, "tab section · ↑/↓ move · enter apply · esc back"),
        chunks[4],
    );
}

fn hex_color(raw: &str) -> ratatui::style::Color {
    let s = raw.trim().trim_start_matches('#');
    if s.len() != 6 {
        return ratatui::style::Color::Reset;
    }
    let r = u8::from_str_radix(&s[0..2], 16).unwrap_or(200);
    let g = u8::from_str_radix(&s[2..4], 16).unwrap_or(200);
    let b = u8::from_str_radix(&s[4..6], 16).unwrap_or(200);
    ratatui::style::Color::Rgb(r, g, b)
}

// --- Models ------------------------------------------------------------------

fn render_models(f: &mut Frame, app: &App) {
    let col = book_column(f.area(), 68);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(5),
            Constraint::Length(3),
            Constraint::Length(3),
        ])
        .split(col);

    f.render_widget(
        Paragraph::new(
            RLine::from(vec![Span::styled(
                "Voices & models",
                Style::default()
                    .fg(app.theme.heading_c())
                    .bg(app.theme.bg())
                    .add_modifier(Modifier::BOLD),
            )])
            .alignment(Alignment::Center),
        )
        .style(Style::default().bg(app.theme.bg())),
        chunks[0],
    );

    let models = app.tts.list_models();
    let items: Vec<ListItem> = models
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let current = m.id == app.tts.current_model_id;
            let marker = if current {
                "● "
            } else if i == app.model_selected {
                "› "
            } else {
                "  "
            };
            let weight = match m.backend.weight_class() {
                orpheus_tts::WeightClass::Light => "light · multitasking",
                orpheus_tts::WeightClass::Medium => "medium",
                orpheus_tts::WeightClass::Heavy => "heavy · focused listening",
            };
            let installed = if m.installed {
                "installed"
            } else {
                "pull on use"
            };
            if i == app.model_selected {
                ListItem::new(RLine::from(vec![
                    Span::styled(
                        marker,
                        Style::default()
                            .fg(app.theme.accent_c())
                            .bg(app.theme.selection_c())
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!("{}  ", m.name),
                        Style::default()
                            .fg(app.theme.current_c())
                            .bg(app.theme.selection_c())
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!("{weight} · {installed}"),
                        Style::default()
                            .fg(app.theme.muted_c())
                            .bg(app.theme.selection_c()),
                    ),
                ]))
            } else {
                ListItem::new(RLine::from(vec![
                    Span::styled(
                        marker,
                        Style::default().fg(app.theme.accent_c()).bg(app.theme.bg()),
                    ),
                    Span::styled(
                        format!("{}  ", m.name),
                        Style::default().fg(app.theme.fg()).bg(app.theme.bg()),
                    ),
                    Span::styled(format!("{weight} · {installed}"), muted(app)),
                ]))
            }
        })
        .collect();
    scroll_list(f, chunks[1], items, app.model_selected, app);

    let input = Paragraph::new(RLine::from(vec![
        Span::styled("  pull: ", muted(app)),
        Span::styled(
            format!("{}▌", app.model_input),
            Style::default().fg(app.theme.fg()).bg(app.theme.bg()),
        ),
    ]))
    .block(
        Block::default()
            .title(if app.model_typing {
                " typing — enter pulls "
            } else {
                " / or i to type a model id "
            })
            .title_style(Style::default().fg(app.theme.muted_c()).bg(app.theme.bg()))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(app.theme.border_c()).bg(app.theme.bg()))
            .style(Style::default().bg(app.theme.bg())),
    );
    f.render_widget(input, chunks[2]);
    f.render_widget(
        footer_msg(app, "switching never moves your place in the book"),
        chunks[3],
    );
}

// --- Voices ------------------------------------------------------------------

fn render_voices(f: &mut Frame, app: &App) {
    let col = book_column(f.area(), 64);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(4),
            Constraint::Length(4),
        ])
        .split(col);

    f.render_widget(
        Paragraph::new(
            RLine::from(vec![Span::styled(
                format!("Voices — {}", app.tts.current_model_id),
                Style::default()
                    .fg(app.theme.heading_c())
                    .bg(app.theme.bg())
                    .add_modifier(Modifier::BOLD),
            )])
            .alignment(Alignment::Center),
        )
        .style(Style::default().bg(app.theme.bg())),
        chunks[0],
    );
    let items: Vec<ListItem> = app
        .voice_list
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let current = v.id == app.tts.current_voice_id;
            let marker = if current {
                "● "
            } else if i == app.voice_selected {
                "› "
            } else {
                "  "
            };
            let (fg, bg, mods) = if i == app.voice_selected {
                (
                    app.theme.current_c(),
                    app.theme.selection_c(),
                    Modifier::BOLD,
                )
            } else {
                (app.theme.fg(), app.theme.bg(), Modifier::empty())
            };
            ListItem::new(RLine::from(vec![
                Span::styled(
                    marker,
                    Style::default()
                        .fg(app.theme.accent_c())
                        .bg(bg)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    v.id.clone(),
                    Style::default().fg(fg).bg(bg).add_modifier(mods),
                ),
                Span::styled(
                    format!("  · {}", v.note),
                    Style::default().fg(app.theme.muted_c()).bg(bg),
                ),
            ]))
        })
        .collect();
    let items = if items.is_empty() {
        vec![ListItem::new(RLine::from(vec![Span::styled(
            "  (no voices — pull a model first)",
            muted(app),
        )]))]
    } else {
        items
    };
    scroll_list(f, chunks[1], items, app.voice_selected, app);
    f.render_widget(
        footer_msg(
            app,
            "enter select · a add sample · p preview · d delete clone · esc back",
        ),
        chunks[2],
    );
}

// --- Voice editor (add-voice form) -----------------------------------------------

fn render_voice_editor(f: &mut Frame, app: &App) {
    let col = book_column(f.area(), 62);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Min(6),
            Constraint::Length(2),
        ])
        .split(col);

    f.render_widget(
        Paragraph::new(
            RLine::from(vec![Span::styled(
                "Add voice",
                Style::default()
                    .fg(app.theme.heading_c())
                    .bg(app.theme.bg())
                    .add_modifier(Modifier::BOLD),
            )])
            .alignment(Alignment::Center),
        )
        .style(Style::default().bg(app.theme.bg())),
        chunks[0],
    );

    let name_field = Paragraph::new(RLine::from(vec![
        Span::styled(
            app.ve_name.clone(),
            Style::default().fg(app.theme.fg()).bg(app.theme.bg()),
        ),
        Span::styled(
            if app.ve_field == 0 { "▌" } else { "" },
            Style::default().fg(app.theme.muted_c()).bg(app.theme.bg()),
        ),
    ]))
    .block(
        Block::default()
            .title(if app.ve_field == 0 {
                "▶ name "
            } else {
                "  name "
            })
            .title_style(Style::default().fg(app.theme.muted_c()).bg(app.theme.bg()))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(app.theme.border_c()).bg(app.theme.bg()))
            .style(Style::default().bg(app.theme.bg())),
    );
    f.render_widget(name_field, chunks[1]);

    let path_field = Paragraph::new(RLine::from(vec![
        Span::styled(
            app.ve_path.clone(),
            Style::default().fg(app.theme.fg()).bg(app.theme.bg()),
        ),
        Span::styled(
            if app.ve_field == 1 { "▌" } else { "" },
            Style::default().fg(app.theme.muted_c()).bg(app.theme.bg()),
        ),
    ]))
    .block(
        Block::default()
            .title(if app.ve_field == 1 {
                "▶ sample file (~/voices/<file> or full path) "
            } else {
                "  sample file "
            })
            .title_style(Style::default().fg(app.theme.muted_c()).bg(app.theme.bg()))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(app.theme.border_c()).bg(app.theme.bg()))
            .style(Style::default().bg(app.theme.bg())),
    );
    f.render_widget(path_field, chunks[2]);

    // Samples found in ~/voices for reference.
    let mut lines = vec![RLine::from(vec![Span::styled(
        "  samples in ~/voices/:",
        muted(app),
    )])];
    match home_voices() {
        v if v.is_empty() => lines.push(RLine::from(vec![Span::styled(
            "  (none yet — put a wav/mp3/flac there)",
            muted(app),
        )])),
        v => {
            for name in v.into_iter().take(12) {
                lines.push(RLine::from(vec![Span::styled(
                    format!("  {name}"),
                    Style::default().fg(app.theme.fg()).bg(app.theme.bg()),
                )]));
            }
        }
    }
    lines.push(RLine::from(""));
    lines.push(RLine::from(vec![Span::styled(
        "  3s+ clean single-speaker clip; the original is never modified.",
        muted(app),
    )]));
    f.render_widget(
        Paragraph::new(Text::from(lines)).style(Style::default().bg(app.theme.bg())),
        chunks[3],
    );
    f.render_widget(
        footer_msg(app, "tab field · type · enter next/save · esc cancel"),
        chunks[4],
    );
}

fn home_voices() -> Vec<String> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(home.join("voices")) else {
        return Vec::new();
    };
    let mut out: Vec<String> = entries
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
    out.sort();
    out
}

// --- Help --------------------------------------------------------------------

fn render_help(f: &mut Frame, app: &App) {
    let col = book_column(f.area(), 62);
    let rows = vec![
        ("j / k · ↑ / ↓", "scroll"),
        ("space", "play / pause narration"),
        ("← / →", "previous / next sentence"),
        ("n / p", "next / previous chapter"),
        ("- / + · 0", "speed down / up / reset"),
        ("/", "search in book"),
        ("b", "bookmark"),
        ("t", "appearance — themes & styles"),
        ("v / m", "voices / TTS models"),
        ("?", "this help"),
        ("q · ctrl+c", "quit (position auto-saves)"),
        ("", ""),
        ("home", "enter open · o folder · d forget book"),
        ("shelf", "enter open · r rescan · esc back"),
        ("models", "enter select · / type id to pull"),
        (
            "voices",
            "enter select · a add · p preview · d delete · pgup/pgdn",
        ),
    ];
    let mut lines = vec![
        RLine::from(vec![Span::styled(
            "Help",
            Style::default()
                .fg(app.theme.heading_c())
                .bg(app.theme.bg())
                .add_modifier(Modifier::BOLD),
        )])
        .alignment(Alignment::Center),
        RLine::from(""),
    ];
    for (k, v) in rows {
        if k.is_empty() {
            lines.push(RLine::from(vec![Span::styled(v, muted(app))]));
        } else {
            lines.push(RLine::from(vec![
                Span::styled(
                    format!("  {k:<18}"),
                    Style::default().fg(app.theme.accent_c()).bg(app.theme.bg()),
                ),
                Span::styled(v, Style::default().fg(app.theme.fg()).bg(app.theme.bg())),
            ]));
        }
    }
    f.render_widget(
        Paragraph::new(Text::from(lines)).style(Style::default().bg(app.theme.bg())),
        col,
    );
}

// --- Search ------------------------------------------------------------------

fn render_search(f: &mut Frame, app: &App) {
    let area = centered(f.area(), 70, 70);
    f.render_widget(Clear, area);
    // Search floats above the page; keep it themed.
    f.render_widget(
        Block::default()
            .style(Style::default().bg(app.theme.bg()).fg(app.theme.fg()))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(app.theme.border_c())),
        area,
    );
    let inner = Rect {
        x: area.x + 2,
        y: area.y + 1,
        width: area.width.saturating_sub(4),
        height: area.height.saturating_sub(2),
    };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(4)])
        .split(inner);

    f.render_widget(
        Paragraph::new(RLine::from(vec![
            Span::styled(
                "/",
                Style::default().fg(app.theme.accent_c()).bg(app.theme.bg()),
            ),
            Span::styled(
                app.search_query.clone(),
                Style::default().fg(app.theme.fg()).bg(app.theme.bg()),
            ),
            Span::styled(
                "▌",
                Style::default().fg(app.theme.muted_c()).bg(app.theme.bg()),
            ),
        ]))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(app.theme.border_c()).bg(app.theme.bg()))
                .title(" find in book ")
                .title_style(muted(app))
                .style(Style::default().bg(app.theme.bg())),
        ),
        chunks[0],
    );

    let q = app.search_query.to_lowercase();
    let mut hits: Vec<ListItem> = Vec::new();
    if !q.is_empty() {
        if let Some(book) = &app.book {
            for (ci, ch) in book.chapters.iter().enumerate() {
                for b in &ch.blocks {
                    for s in &b.sentences {
                        if s.text.to_lowercase().contains(&q) {
                            let preview: String = s.text.chars().take(88).collect();
                            hits.push(ListItem::new(RLine::from(vec![
                                Span::styled(
                                    format!(
                                        "  ch {} · {}  ",
                                        ci + 1,
                                        truncate_middle(&ch.title, 24)
                                    ),
                                    muted(app),
                                ),
                                Span::styled(
                                    preview,
                                    Style::default().fg(app.theme.fg()).bg(app.theme.bg()),
                                ),
                            ])));
                            if hits.len() >= 30 {
                                break;
                            }
                        }
                    }
                    if hits.len() >= 30 {
                        break;
                    }
                }
                if hits.len() >= 30 {
                    break;
                }
            }
        }
    }
    if hits.is_empty() {
        hits.push(ListItem::new(RLine::from(vec![Span::styled(
            "  type to search — enter jumps, esc cancels",
            muted(app),
        )])));
    }
    f.render_widget(
        List::new(hits).style(Style::default().bg(app.theme.bg())),
        chunks[1],
    );
}

// --- shared ------------------------------------------------------------------

/// A list that follows the cursor: keeps `selected` visible by offsetting
/// the viewport. Plain `List` renders from the top and strands everything
/// below the fold (voices, shelves, search hits).
fn scroll_list(f: &mut Frame, area: Rect, items: Vec<ListItem<'_>>, selected: usize, app: &App) {
    let vis = area.height.max(1) as usize;
    let offset = selected.saturating_sub(vis.saturating_sub(1));
    let mut state = ListState::default()
        .with_selected(if items.is_empty() {
            None
        } else {
            Some(selected.min(items.len().saturating_sub(1)))
        })
        .with_offset(offset);
    // Selection is painted manually per-row; neutralize the default
    // reversed highlight so it doesn't double up.
    let list = List::new(items)
        .highlight_style(Style::default())
        .style(Style::default().bg(app.theme.bg()));
    f.render_stateful_widget(list, area, &mut state);
}

fn hint_bar(app: &App, text: &str) -> Paragraph<'static> {
    Paragraph::new(
        RLine::from(vec![Span::styled(
            format!("  {text}"),
            Style::default().fg(app.theme.status_c()).bg(app.theme.bg()),
        )])
        .alignment(Alignment::Center),
    )
    .style(Style::default().bg(app.theme.bg()))
}

/// Footer line with a voice: transient status (errors, confirmations,
/// progress) replaces the hints while present, so no screen ever fails
/// silently. This was the actual "can't save audio" bug — the editor ran
/// errors nobody could see.
fn footer_msg(app: &App, hints: &str) -> Paragraph<'static> {
    match &app.status_msg {
        Some(msg) => Paragraph::new(
            RLine::from(vec![Span::styled(
                format!("  {msg}"),
                Style::default()
                    .fg(app.theme.accent_c())
                    .bg(app.theme.bg())
                    .add_modifier(Modifier::BOLD),
            )])
            .alignment(Alignment::Center),
        )
        .style(Style::default().bg(app.theme.bg())),
        None => hint_bar(app, hints),
    }
}

/// Thin Foliate-like rule.
fn hairline(width: u16) -> String {
    "─".repeat(width.saturating_sub(4) as usize)
}

/// Slim text progress bar: ━━━●───
fn text_progress_bar(ratio: f64, width: u16) -> String {
    let w = width.max(8) as usize;
    let filled = ((ratio.clamp(0.0, 1.0) * w as f64).round() as usize).min(w);
    let mut s = String::with_capacity(w + 2);
    for i in 0..w {
        if i < filled {
            s.push('━');
        } else if i == filled && filled < w {
            s.push('●');
        } else {
            s.push('─');
        }
    }
    s
}

fn truncate_middle(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    if max < 8 {
        return s.chars().take(max).collect();
    }
    let keep = (max - 1) / 2;
    let left: String = s.chars().take(keep).collect();
    let right: String = s
        .chars()
        .rev()
        .take(keep)
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    format!("{left}…{right}")
}

fn centered(area: Rect, pct_x: u16, pct_y: u16) -> Rect {
    let v = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - pct_y) / 2),
            Constraint::Percentage(pct_y),
            Constraint::Percentage((100 - pct_y) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - pct_x) / 2),
            Constraint::Percentage(pct_x),
            Constraint::Percentage((100 - pct_x) / 2),
        ])
        .split(v[1])[1]
}
