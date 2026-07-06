use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Gauge, List, ListItem, Paragraph},
    Frame,
};

use crate::app::{ActivePanel, App};
use crate::audio::visualizer::VisualizerMode;
use crate::data::library::LibraryEntry;
use crate::data::metadata::LyricsKind;
use crate::data::playlist::QueueVisualItem;
use crate::ui::artwork;
use crate::ui::branding;
use crate::ui::lyrics_widget;
use crate::ui::visualizer_widget;

// ─── Native Terminal theme palette (follows terminal settings) ───
const C_FG: Color = Color::Reset;
const C_ACCENT: Color = Color::Magenta;
const C_ACCENT2: Color = Color::LightMagenta;
const C_CYAN: Color = Color::Cyan;
const C_GREEN: Color = Color::Green;
const C_ORANGE: Color = Color::Yellow;
const C_DIM: Color = Color::DarkGray;
const C_SURFACE: Color = Color::DarkGray;

/// Main render function — dispatches to the appropriate view inside a global two-pane layout.
fn render_instructions(f: &mut Frame, area: Rect, panel: ActivePanel) {
    let text = match panel {
        ActivePanel::Queue => "Move: ↑/↓/k/j  •  Play: Enter  •  Remove: d",
        ActivePanel::Library => "Navigate: ↑/↓/k/j  •  Expand/Enqueue: Enter  •  Search: /",
        ActivePanel::Search => "Type to search  •  Select: ↑/↓  •  Enqueue: Enter",
        ActivePanel::NowPlaying => "Lyrics Mode: m  •  Visualizer Mode: v",
        ActivePanel::Help => "Switch Views: F2-F6 / Tab  •  Quit: q / Esc",
    };
    let widget = Paragraph::new(Line::from(Span::styled(
        text,
        Style::default().fg(C_DIM).add_modifier(Modifier::ITALIC),
    )))
    .alignment(ratatui::layout::Alignment::Center);
    f.render_widget(widget, area);
}

/// Main render function — dispatches to the appropriate view inside a global two-pane layout.
pub fn draw(f: &mut Frame, app: &mut App) {
    let size = f.area();

    // Screen-size guard: the layout requires at least 90 columns to render correctly.
    // On narrow terminals (e.g., portrait Android Termux), show a friendly prompt
    // instead of squashing the two-pane layout into an unusable sliver.
    if size.width < 90 {
        let msg = format!(
            "This application is optimized for landscape mode.\n\n\
             Current terminal: {} \u{00d7} {}\n\
             Recommended: at least 90 \u{00d7} 30\n\n\
             Please rotate your phone or increase the terminal width.",
            size.width, size.height
        );
        let widget = ratatui::widgets::Paragraph::new(msg)
            .alignment(ratatui::layout::Alignment::Center)
            .style(ratatui::style::Style::default().fg(C_ACCENT2));
        f.render_widget(widget, size);
        return;
    }

    // If awaiting directory input, show the welcome screen
    if app.awaiting_dir_input {
        draw_dir_input(f, app, size);
        return;
    }

    // If player is initializing, show loading screen
    if app.player_loading {
        let msg = "\n\nInitializing Audio Engine...\n\nPlease wait.";
        let widget = ratatui::widgets::Paragraph::new(msg)
            .alignment(ratatui::layout::Alignment::Center)
            .style(ratatui::style::Style::default().fg(C_ACCENT2));
        f.render_widget(widget, size);
        return;
    }

    // Global Two-Pane Layout Split (Left Pane gets full terminal height):
    // Left pane: Sixel artwork
    // Right pane: Content and footer
    let art_width = artwork::art_pane_width(size.width);
    let h_layout = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(art_width), Constraint::Min(0)])
        .split(size);

    let art_area = h_layout[0];
    let right_area = h_layout[1];

    // Draw floating Sixel album art in the left pane (if a song is playing/loaded and pane is visible)
    if art_width > 0 && !app.playlist.is_empty() && art_area.height >= 8 && art_area.width >= 8 {
        let v_split = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(2), // top margin
                Constraint::Min(4),    // middle art area
                Constraint::Length(2), // bottom margin
            ])
            .split(art_area);

        let h_split = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(2), // left margin
                Constraint::Min(4),    // middle art area
                Constraint::Length(2), // right margin
            ])
            .split(v_split[1]);

        let centered_art_area = h_split[1];
        artwork::render_artwork(f, centered_art_area, &mut app.current_cover_protocol);
    }

    // Split the right pane vertically: Content area + Footer at the absolute bottom of the right pane
    let right_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(3),    // Content area
            Constraint::Length(1), // Footer / tab bar
        ])
        .split(right_area);

    let right_content_area = right_layout[0];
    let footer_area = right_layout[1];

    // Wrap the right content pane in a layout with left and right margins (10%)
    let right_padded_layout = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(10), // left margin
            Constraint::Min(0),         // centered middle column
            Constraint::Percentage(10), // right margin
        ])
        .split(right_content_area);
    let right_content_padded = right_padded_layout[1];

    // Draw the appropriate active panel inside the right content padded area.
    // For NowPlaying, it implements its own exact vertical layout array (including the logo).
    // For other panels, we render the persistent logo at the top and the active panel below it.
    if app.active_panel == ActivePanel::NowPlaying {
        draw_now_playing(f, app, right_content_padded);
    } else {
        let show_logo = right_content_padded.height > 10;
        let (logo_area, instructions_area, tab_area) = if show_logo {
            let r_layout = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(6), // Logo height
                    Constraint::Length(1), // Shortcut instructions
                    Constraint::Min(0),
                ])
                .split(right_content_padded);
            (Some(r_layout[0]), Some(r_layout[1]), r_layout[2])
        } else {
            (None, None, right_content_padded)
        };

        if let Some(logo_rect) = logo_area {
            let song_name = app
                .playlist
                .current_entry()
                .map(|e| e.metadata.display_title(app.config.strip_track_numbers));
            branding::render_logo_with_song(f, logo_rect, song_name.as_deref());
        }

        if let Some(inst_rect) = instructions_area {
            render_instructions(f, inst_rect, app.active_panel);
        }

        match app.active_panel {
            ActivePanel::Queue => draw_queue(f, app, tab_area),
            ActivePanel::Library => draw_library(f, app, tab_area),
            ActivePanel::Search => draw_search(f, app, tab_area),
            ActivePanel::Help => draw_help(f, tab_area),
            ActivePanel::NowPlaying => unreachable!(),
        }
    }

    // Footer tab bar at the absolute bottom of the Right Pane
    draw_footer(f, app, footer_area);
}

// ─── Now Playing View ──────────────────────────────────────────────────────
fn draw_now_playing(f: &mut Frame, app: &mut App, area: Rect) {
    if app.playlist.is_empty() {
        let hint = Paragraph::new(Line::from(Span::styled(
            "No track loaded. Enqueue music (F3) to begin.",
            Style::default().fg(C_DIM),
        )))
        .alignment(ratatui::layout::Alignment::Center);
        f.render_widget(hint, area);
        return;
    }

    draw_info_pane(f, app, area);
}

fn draw_info_pane(f: &mut Frame, app: &mut App, area: Rect) {
    if area.height < 10 {
        return;
    }

    let song_name = app
        .playlist
        .current_entry()
        .map(|e| e.metadata.display_title(app.config.strip_track_numbers));

    if app.show_full_lyrics {
        // Layout: Logo + Instructions + Metadata + Spacer + Scrollable Lyrics + Spacer + Progress
        let constraints = vec![
            Constraint::Length(6), // Top Logo
            Constraint::Length(1), // Shortcut instructions
            Constraint::Length(3), // Metadata
            Constraint::Length(1), // Spacer
            Constraint::Fill(1),   // Scrollable Lyrics (THE CRITICAL EXPANSION)
            Constraint::Length(1), // Spacer
            Constraint::Length(2), // Progress bar + statistics
        ];
        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints(constraints)
            .split(area);

        branding::render_logo_with_song(f, layout[0], song_name.as_deref());
        render_instructions(f, layout[1], ActivePanel::NowPlaying);
        draw_metadata(f, app, layout[2]);
        draw_lyrics(f, app, layout[4]);
        draw_progress(f, app, layout[6]);
        return;
    }

    // Normal mode: 9 steps EXACT recipe
    let constraints = vec![
        Constraint::Length(6), // 1. Top Logo
        Constraint::Length(1), // 2. Shortcut instructions
        Constraint::Length(3), // 3. Metadata (centered)
        Constraint::Length(1), // 4. Spacer
        Constraint::Length(3), // 5. Lyrics Engine (max 3 lines, toggleable with 'm')
        Constraint::Length(1), // 6. Spacer
        Constraint::Fill(1),   // 7. Visualizer (THE CRITICAL EXPANSION - absorbs remaining space)
        Constraint::Length(1), // 8. Spacer
        Constraint::Length(2), // 9. Bottom Progress Bar
    ];

    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);

    branding::render_logo_with_song(f, layout[0], song_name.as_deref());
    render_instructions(f, layout[1], ActivePanel::NowPlaying);
    draw_metadata(f, app, layout[2]);
    draw_lyrics(f, app, layout[4]);
    draw_visualizer(f, app, layout[6]);
    draw_progress(f, app, layout[8]);
}

fn draw_metadata(f: &mut Frame, app: &App, area: Rect) {
    if let Some(entry) = app.playlist.current_entry() {
        let meta = &entry.metadata;
        let lines = vec![
            Line::from(Span::styled(
                meta.display_title(app.config.strip_track_numbers),
                Style::default().fg(C_FG).add_modifier(Modifier::BOLD),
            )),
            Line::from(Span::styled(
                meta.display_artist(),
                Style::default().fg(C_ACCENT),
            )),
            Line::from(Span::styled(
                meta.display_album(),
                Style::default().fg(C_DIM),
            )),
        ];
        let widget = Paragraph::new(lines).alignment(ratatui::layout::Alignment::Center);
        f.render_widget(widget, area);
    }
}

fn draw_lyrics(f: &mut Frame, app: &App, area: Rect) {
    if let Some(entry) = app.playlist.current_entry() {
        if app.show_full_lyrics {
            if let Some(ref lyrics_data) = app.current_lyrics {
                lyrics_widget::render_full_timed_lyrics(
                    f,
                    area,
                    lyrics_data,
                    app.display_elapsed_secs(),
                    app.lyrics_scroll,
                );
                return;
            }
            match &entry.metadata.lyrics {
                LyricsKind::Timed(lines) => {
                    let data = crate::data::lyrics::LyricsData {
                        lines: lines.clone(),
                        is_timed: true,
                        word_timestamps: Vec::new(),
                    };
                    lyrics_widget::render_full_timed_lyrics(
                        f,
                        area,
                        &data,
                        app.display_elapsed_secs(),
                        app.lyrics_scroll,
                    );
                }
                LyricsKind::Untimed(lines) => {
                    lyrics_widget::render_untimed_lyrics(f, area, lines, app.lyrics_scroll);
                }
                LyricsKind::None => {
                    let hint = Paragraph::new(Line::from(Span::styled(
                        "No Lyrics Available. Press 'm' to go back",
                        Style::default().fg(C_DIM),
                    )))
                    .alignment(ratatui::layout::Alignment::Center);
                    f.render_widget(hint, area);
                }
            }
        } else {
            if let Some(ref lyrics_data) = app.current_lyrics {
                lyrics_widget::render_constrained_lyrics(
                    f,
                    area,
                    lyrics_data,
                    app.display_elapsed_secs(),
                );
                return;
            }
            match &entry.metadata.lyrics {
                LyricsKind::Timed(lines) => {
                    let data = crate::data::lyrics::LyricsData {
                        lines: lines.clone(),
                        is_timed: true,
                        word_timestamps: Vec::new(),
                    };
                    lyrics_widget::render_constrained_lyrics(
                        f,
                        area,
                        &data,
                        app.display_elapsed_secs(),
                    );
                }
                LyricsKind::Untimed(_) => {
                    let lines = vec![
                        Line::from(Span::styled("(Untimed Lyrics)", Style::default().fg(C_DIM))),
                        Line::from(Span::styled(
                            "[Press 'm' to view full lyrics]",
                            Style::default().fg(C_ACCENT2).add_modifier(Modifier::BOLD),
                        )),
                        Line::from(Span::styled("...", Style::default().fg(C_DIM))),
                    ];
                    let widget =
                        Paragraph::new(lines).alignment(ratatui::layout::Alignment::Center);
                    f.render_widget(widget, area);
                }
                LyricsKind::None => {}
            }
        }
    }
}

fn draw_visualizer(f: &mut Frame, app: &mut App, area: Rect) {
    // try_read() is non-blocking: if the FFT thread currently holds the write
    // lock, we skip this frame rather than stalling the render loop (and
    // indirectly blocking the audio decode path via mutex back-pressure).
    if let Ok(bars) = app.visualizer_bars.try_read() {
        match app.visualizer_mode {
            VisualizerMode::Spectrum => {
                let scaled: Vec<u16> = bars
                    .iter()
                    .map(|&val| (val * (area.height as f32 * 8.0)) as u16)
                    .collect();
                visualizer_widget::render_spectrum(f, area, &scaled, area.height);
            }
            VisualizerMode::Braille => {
                visualizer_widget::render_braille(f, area, &bars);
            }
        }
    }
}

fn draw_progress(f: &mut Frame, app: &App, area: Rect) {
    if area.height < 2 {
        return;
    }

    // Apply left/right margins to the progress bar and stats (15% margin on each side)
    let padded_layout = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(15),
            Constraint::Min(0),
            Constraint::Percentage(15),
        ])
        .split(area);
    let active_area = padded_layout[1];

    let elapsed_ms = app.display_elapsed_ms();
    let total_ms = app.player.as_ref().map(|p| p.duration_ms()).unwrap_or(0);
    let ratio = if total_ms > 0 {
        (elapsed_ms as f64 / total_ms as f64).min(1.0)
    } else {
        0.0
    };

    // Progress bar (matches width of the centered content above it)
    let bar_area = Rect::new(active_area.x, active_area.y, active_area.width, 1);
    let gauge = Gauge::default()
        .gauge_style(Style::default().fg(C_ACCENT2).bg(C_SURFACE))
        .ratio(ratio)
        .label("");
    f.render_widget(gauge, bar_area);

    // Info line
    if active_area.height >= 2 {
        let info_area = Rect::new(active_area.x, active_area.y + 1, active_area.width, 1);

        let elapsed_str = format_time(elapsed_ms);
        let total_str = format_time(total_ms);
        let pct = (ratio * 100.0) as u32;
        let vol = app.player.as_ref().map(|p| p.volume()).unwrap_or(100);
        let repeat = app.playlist.repeat.symbol();
        let shuffle = if app.playlist.shuffle { "⤨" } else { "" };

        let status = if app.player.as_ref().map(|p| p.is_paused()).unwrap_or(false) { "⏸" } else { "▶" };
        let bitrate_str = app
            .now_playing_meta
            .as_ref()
            .and_then(|m| m.bitrate)
            .map(|b| format!("{}kbps", b))
            .unwrap_or_default();

        let info = format!(
            "{} {}/{} ({}%)  Vol:{}% {} {} {}",
            status, elapsed_str, total_str, pct, vol, repeat, shuffle, bitrate_str
        );

        let widget = Paragraph::new(Line::from(Span::styled(info, Style::default().fg(C_DIM))))
            .alignment(ratatui::layout::Alignment::Center);
        f.render_widget(widget, info_area);
    }
}

// ─── Queue View ────────────────────────────────────────────────────────────
fn draw_queue(f: &mut Frame, app: &mut App, area: Rect) {
    if area.width < 5 || area.height < 1 {
        return;
    }

    if app.playlist.is_empty() {
        let hint = Paragraph::new(Line::from(Span::styled(
            "  Queue is empty. Browse the library (F3) to add tracks.",
            Style::default().fg(C_DIM),
        )))
        .alignment(ratatui::layout::Alignment::Left);
        f.render_widget(hint, area);
        return;
    }

    let visual_items = app
        .playlist
        .get_visual_items(app.show_folders, app.config.strip_track_numbers);
    let playing_real = app.playlist.current_real_index();

    let visible_height = area.height as usize;
    let half = visible_height / 2;

    // Find the visual index corresponding to app.queue_cursor
    let mut highlighted_visual_idx = 0;
    for (idx, item) in visual_items.iter().enumerate() {
        if let QueueVisualItem::Track { entry_idx, .. } = item {
            if *entry_idx == app.queue_cursor {
                highlighted_visual_idx = idx;
                break;
            }
        }
    }

    let scroll = if highlighted_visual_idx >= half {
        highlighted_visual_idx - half
    } else {
        0
    };
    let scroll = scroll.min(visual_items.len().saturating_sub(1));
    let start = scroll;
    let end = (scroll + visible_height).min(visual_items.len());
    let end = end.max(start);

    let mut display_lines = Vec::new();

    for i in start..end {
        let item = &visual_items[i];
        match item {
            QueueVisualItem::Header { name } => {
                display_lines.push(Line::from(vec![
                    Span::styled(
                        "  📁 ",
                        Style::default().fg(C_ORANGE).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        name.as_str(),
                        Style::default().fg(C_ORANGE).add_modifier(Modifier::BOLD),
                    ),
                ]));
            }
            QueueVisualItem::Separator => {
                display_lines.push(Line::from("  "));
            }
            QueueVisualItem::Track {
                entry_idx, title, ..
            } => {
                let is_playing = Some(*entry_idx) == playing_real;
                let is_cursor = *entry_idx == app.queue_cursor;
                let prefix = if is_playing { "▶ " } else { "  " };

                let style = if is_playing && is_cursor {
                    Style::default().fg(C_GREEN).add_modifier(Modifier::BOLD)
                } else if is_playing {
                    Style::default().fg(C_GREEN)
                } else if is_cursor {
                    Style::default().fg(C_FG).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(C_DIM)
                };

                display_lines.push(Line::from(vec![
                    Span::styled("  ", style),
                    Span::styled(prefix, style),
                    Span::styled(title.as_str(), style),
                ]));
            }
        }
    }

    let widget = Paragraph::new(display_lines).alignment(ratatui::layout::Alignment::Left);
    f.render_widget(widget, area);
}

// ─── Library View ──────────────────────────────────────────────────────────
fn draw_library(f: &mut Frame, app: &mut App, area: Rect) {
    if area.width < 5 || area.height < 1 {
        return;
    }

    if app.library_loading {
        let lines = vec![
            Line::from(""),
            Line::from(Span::styled(
                "  Loading Library...",
                Style::default().fg(C_ACCENT2).add_modifier(Modifier::BOLD),
            )),
        ];
        let widget = Paragraph::new(lines).alignment(ratatui::layout::Alignment::Left);
        f.render_widget(widget, area);
        return;
    }

    if app.flat_library.is_empty() {
        let lines = vec![
            Line::from(""),
            Line::from(Span::styled(
                "  No music found in library path.",
                Style::default().fg(C_DIM),
            )),
            Line::from(Span::styled(
                "  Check your config and rescan.",
                Style::default().fg(C_DIM),
            )),
        ];
        let widget = Paragraph::new(lines).alignment(ratatui::layout::Alignment::Left);
        f.render_widget(widget, area);
        return;
    }

    // Determine enqueued status of the entire library using pre-calculated set
    let all_enqueued = app.flat_library.iter().all(|item| {
        if let LibraryEntry::Track { path, .. } = &item.entry {
            app.playlist.entry_paths.contains(path)
        } else {
            true
        }
    });

    let header_text = if all_enqueued {
        "*- MUSIC LIBRARY -"
    } else {
        "  - MUSIC LIBRARY -"
    };
    let is_header_cursor = app.library_cursor == 0;
    let header_style = if is_header_cursor {
        Style::default().fg(C_FG).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(C_CYAN)
    };

    let visible_height = area.height as usize;
    let total_items = 1 + app.flat_library.len();
    let scroll = if visible_height > 0 && app.library_cursor >= visible_height {
        app.library_cursor - visible_height + 1
    } else {
        0
    };
    let scroll = scroll.min(total_items.saturating_sub(1));
    let start = scroll;
    let end = (scroll + visible_height).min(total_items);
    let end = end.max(start);

    let mut list_items = Vec::new();

    for i in start..end {
        if i == 0 {
            list_items.push(ListItem::new(Line::from(Span::styled(
                header_text,
                header_style,
            ))));
        } else {
            let idx = i - 1;
            let item = &app.flat_library[idx];
            let entry = &item.entry;
            let entry_cursor_idx = idx + 1;
            let is_cursor = entry_cursor_idx == app.library_cursor;

            let enqueued = item.enqueued;

            let (icon, style) = if entry.is_dir() {
                let is_collapsed = app.collapsed_dirs.contains(entry.path());
                let dir_icon = if is_collapsed {
                    "▶ 📁 "
                } else {
                    "▼ 📁 "
                };
                let s = if is_cursor {
                    Style::default().fg(C_CYAN).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(C_CYAN)
                };
                (dir_icon, s)
            } else {
                let s = if is_cursor {
                    Style::default().fg(C_FG).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(C_DIM)
                };
                ("♪  ", s)
            };

            let mut line_spans = Vec::with_capacity(4 + item.ancestor_last.len());
            line_spans.push(Span::styled(if enqueued { "  * " } else { "    " }, style));

            for &ancestor_is_last in &item.ancestor_last {
                if ancestor_is_last {
                    line_spans.push(Span::styled("    ", style));
                } else {
                    line_spans.push(Span::styled("│   ", style));
                }
            }
            if item.is_last {
                line_spans.push(Span::styled("└── ", style));
            } else {
                line_spans.push(Span::styled("├── ", style));
            }

            line_spans.push(Span::styled(icon, style));
            line_spans.push(Span::styled(entry.name(), style));

            list_items.push(ListItem::new(Line::from(line_spans)));
        }
    }

    let list = List::new(list_items);
    f.render_widget(list, area);
}

// ─── Search View ───────────────────────────────────────────────────────────
fn draw_search(f: &mut Frame, app: &mut App, area: Rect) {
    if area.width < 5 || area.height < 3 {
        return;
    }

    // Search input line (Centered)
    let input_area = Rect::new(area.x, area.y, area.width, 1);
    let cursor = if app.searching { "█" } else { "" };
    let input_line = Line::from(vec![
        Span::styled("Search: ", Style::default().fg(C_ACCENT)),
        Span::styled(&app.search_query, Style::default().fg(C_FG)),
        Span::styled(cursor, Style::default().fg(C_ACCENT2)),
    ]);
    let input_widget = Paragraph::new(input_line).alignment(ratatui::layout::Alignment::Center);
    f.render_widget(input_widget, input_area);

    // Results below it (Left-Aligned, with 2-space padding)
    let results_area = Rect::new(
        area.x,
        area.y + 2,
        area.width,
        area.height.saturating_sub(2),
    );

    let results_height = results_area.height as usize;
    let scroll = if results_height > 0 && app.search_cursor >= results_height {
        app.search_cursor - results_height + 1
    } else {
        0
    };
    let scroll = scroll.min(app.search_results.len().saturating_sub(1));
    let start = scroll;
    let end = (scroll + results_height).min(app.search_results.len());
    let end = end.max(start);

    let mut display_items = Vec::new();
    for idx in start..end {
        let entry = &app.search_results[idx];
        let is_cursor = idx == app.search_cursor;
        let style = if is_cursor {
            Style::default().fg(C_FG).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(C_DIM)
        };
        let prefix = if entry.is_dir() { "📁 " } else { "♪  " };
        display_items.push(Line::from(vec![
            Span::styled("  ", style),
            Span::styled(prefix, style),
            Span::styled(entry.name(), style),
        ]));
    }

    let list_widget = Paragraph::new(display_items).alignment(ratatui::layout::Alignment::Left);
    f.render_widget(list_widget, results_area);
}

// ─── Help View ─────────────────────────────────────────────────────────────
fn draw_help(f: &mut Frame, area: Rect) {
    let help_text = vec![
        Line::from(Span::styled(
            "  Keyboard Shortcuts",
            Style::default().fg(C_ACCENT2).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        // Navigation & Views
        Line::from(Span::styled(
            "  ── Navigation & Views ──",
            Style::default().fg(C_CYAN).add_modifier(Modifier::BOLD),
        )),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("F2 - F6", Style::default().fg(C_CYAN)),
            Span::styled("      •  ", Style::default().fg(C_DIM)),
            Span::styled(
                "Switch views (Queue/Library/Now Playing/Search/Help)",
                Style::default().fg(C_FG),
            ),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("Tab / B-Tab", Style::default().fg(C_CYAN)),
            Span::styled("  •  ", Style::default().fg(C_DIM)),
            Span::styled("Cycle active panel / view", Style::default().fg(C_FG)),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("k / j / ↑ / ↓", Style::default().fg(C_CYAN)),
            Span::styled("•  ", Style::default().fg(C_DIM)),
            Span::styled("Scroll / Navigate list items", Style::default().fg(C_FG)),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("q / Esc", Style::default().fg(C_CYAN)),
            Span::styled("      •  ", Style::default().fg(C_DIM)),
            Span::styled("Quit application", Style::default().fg(C_FG)),
        ]),
        Line::from(""),
        // Playback Controls
        Line::from(Span::styled(
            "  ── Playback Controls ──",
            Style::default().fg(C_CYAN).add_modifier(Modifier::BOLD),
        )),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("Space / p", Style::default().fg(C_CYAN)),
            Span::styled("    •  ", Style::default().fg(C_DIM)),
            Span::styled("Play / Pause toggle", Style::default().fg(C_FG)),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("S", Style::default().fg(C_CYAN)),
            Span::styled("            •  ", Style::default().fg(C_DIM)),
            Span::styled("Stop playback", Style::default().fg(C_FG)),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("n / l / →", Style::default().fg(C_CYAN)),
            Span::styled("    •  ", Style::default().fg(C_DIM)),
            Span::styled("Next track", Style::default().fg(C_FG)),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("p / h / ←", Style::default().fg(C_CYAN)),
            Span::styled("    •  ", Style::default().fg(C_DIM)),
            Span::styled("Previous track", Style::default().fg(C_FG)),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("a / d", Style::default().fg(C_CYAN)),
            Span::styled("        •  ", Style::default().fg(C_DIM)),
            Span::styled(
                "Seek backward / forward 5 seconds",
                Style::default().fg(C_FG),
            ),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("+ / =", Style::default().fg(C_CYAN)),
            Span::styled("        •  ", Style::default().fg(C_DIM)),
            Span::styled("Volume up", Style::default().fg(C_FG)),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("- / [", Style::default().fg(C_CYAN)),
            Span::styled("        •  ", Style::default().fg(C_DIM)),
            Span::styled("Volume down", Style::default().fg(C_FG)),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("s", Style::default().fg(C_CYAN)),
            Span::styled("            •  ", Style::default().fg(C_DIM)),
            Span::styled("Toggle shuffle mode", Style::default().fg(C_FG)),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("r", Style::default().fg(C_CYAN)),
            Span::styled("            •  ", Style::default().fg(C_DIM)),
            Span::styled(
                "Cycle repeat mode (off → track → queue)",
                Style::default().fg(C_FG),
            ),
        ]),
        Line::from(""),
        // Queue & Library Management
        Line::from(Span::styled(
            "  ── Queue & Library Management ──",
            Style::default().fg(C_CYAN).add_modifier(Modifier::BOLD),
        )),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("Enter", Style::default().fg(C_CYAN)),
            Span::styled("        •  ", Style::default().fg(C_DIM)),
            Span::styled(
                "Enqueue selected item / Play selected queue item",
                Style::default().fg(C_FG),
            ),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("Alt + Enter", Style::default().fg(C_CYAN)),
            Span::styled("  •  ", Style::default().fg(C_DIM)),
            Span::styled(
                "Enqueue selected item and play immediately",
                Style::default().fg(C_FG),
            ),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("o / Left/Right", Style::default().fg(C_CYAN)),
            Span::styled("•  ", Style::default().fg(C_DIM)),
            Span::styled(
                "Toggle / Collapse/Expand directory tree in Library",
                Style::default().fg(C_FG),
            ),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("Delete", Style::default().fg(C_CYAN)),
            Span::styled("       •  ", Style::default().fg(C_DIM)),
            Span::styled(
                "Remove selected track from the queue",
                Style::default().fg(C_FG),
            ),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("Backspace", Style::default().fg(C_CYAN)),
            Span::styled("    •  ", Style::default().fg(C_DIM)),
            Span::styled(
                "Clear the entire queue (stops playback)",
                Style::default().fg(C_FG),
            ),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("f / g", Style::default().fg(C_CYAN)),
            Span::styled("        •  ", Style::default().fg(C_DIM)),
            Span::styled(
                "Move selected queue item up / down",
                Style::default().fg(C_FG),
            ),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("/", Style::default().fg(C_CYAN)),
            Span::styled("            •  ", Style::default().fg(C_DIM)),
            Span::styled(
                "Open search prompt to filter library",
                Style::default().fg(C_FG),
            ),
        ]),
        Line::from(""),
        // Display & Lyrics
        Line::from(Span::styled(
            "  ── Display & Lyrics ──",
            Style::default().fg(C_CYAN).add_modifier(Modifier::BOLD),
        )),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("v", Style::default().fg(C_CYAN)),
            Span::styled("            •  ", Style::default().fg(C_DIM)),
            Span::styled(
                "Toggle spectrum/braille visualizer mode",
                Style::default().fg(C_FG),
            ),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("m", Style::default().fg(C_CYAN)),
            Span::styled("            •  ", Style::default().fg(C_DIM)),
            Span::styled(
                "Toggle full lyrics / 3-line timed lyrics view",
                Style::default().fg(C_FG),
            ),
        ]),
    ];

    let widget = Paragraph::new(help_text).alignment(ratatui::layout::Alignment::Left);
    f.render_widget(widget, area);
}

// ─── Directory Input (First Run) ───────────────────────────────────────────
fn draw_dir_input(f: &mut Frame, app: &App, area: Rect) {
    if area.width < 10 || area.height < 5 {
        return;
    }
    let padded_layout = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(15),
            Constraint::Min(0),
            Constraint::Percentage(15),
        ])
        .split(area);
    let inner_area = padded_layout[1];

    let content_y = inner_area.y + inner_area.height / 3;

    // Logo
    if content_y > inner_area.y + 2 && inner_area.width >= 40 {
        let logo_height = branding::logo_height().min(inner_area.height.saturating_sub(2));
        let logo_area = Rect::new(
            inner_area.x,
            inner_area.y + 1,
            inner_area.width,
            logo_height,
        );
        branding::render_logo(f, logo_area);
    }

    let text_height = 4.min(
        inner_area
            .height
            .saturating_sub(content_y.saturating_sub(inner_area.y)),
    );
    let text_area = Rect::new(inner_area.x, content_y, inner_area.width, text_height);
    let lines = vec![
        Line::from(Span::styled(
            "Welcome to mixed!",
            Style::default().fg(C_FG).add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            "Please enter the absolute path to your music directory:",
            Style::default().fg(C_DIM),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("> ", Style::default().fg(C_ACCENT)),
            Span::styled(&app.dir_input, Style::default().fg(C_FG)),
            Span::styled("█", Style::default().fg(C_ACCENT2)),
        ]),
    ];
    let widget = Paragraph::new(lines).alignment(ratatui::layout::Alignment::Center);
    f.render_widget(widget, text_area);
}

// ─── Footer Tab Bar ────────────────────────────────────────────────────────
fn draw_footer(f: &mut Frame, app: &App, area: Rect) {
    let tabs = [
        ("F2", "Playlist", ActivePanel::Queue),
        ("F3", "Library", ActivePanel::Library),
        ("F4", "Track", ActivePanel::NowPlaying),
        ("F5", "Search", ActivePanel::Search),
        ("F6", "Help", ActivePanel::Help),
    ];

    let mut spans = Vec::new();
    for (i, (key, label, panel)) in tabs.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(" | ", Style::default().fg(C_DIM)));
        }
        let style = if *panel == app.active_panel {
            Style::default().fg(C_ACCENT2).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(C_DIM)
        };
        spans.push(Span::styled(format!("{} {}", key, label), style));
    }

    // Play/Pause icon
    let status_icon = if app.player.as_ref().map(|p| p.is_paused()).unwrap_or(false) { "⏸" } else { "▶" };
    spans.push(Span::raw("  "));
    spans.push(Span::styled(status_icon, Style::default().fg(C_ACCENT)));

    // Shuffle icon
    if app.playlist.shuffle {
        spans.push(Span::raw(" "));
        spans.push(Span::styled("⤨", Style::default().fg(C_ACCENT2)));
    }

    let line = Line::from(spans);
    let widget = Paragraph::new(line).alignment(ratatui::layout::Alignment::Center);
    f.render_widget(widget, area);
}

// ─── Helpers ───────────────────────────────────────────────────────────────
fn format_time(ms: u64) -> String {
    let total_secs = ms / 1000;
    let mins = total_secs / 60;
    let secs = total_secs % 60;
    format!("{}:{:02}", mins, secs)
}
