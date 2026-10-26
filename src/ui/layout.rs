use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Gauge, List, ListItem, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState},
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
use unicode_width::UnicodeWidthStr;

pub use crate::ui::theme::*;
pub use crate::ui::widgets::*;

/// One short row of key + action pairs for the current view; pairs that do not fit
/// are left out from the end.
fn render_instructions(f: &mut Frame, area: Rect, app: &App) {
    let pairs: &[(&str, &str)] = match app.active_panel {
        ActivePanel::Queue => &[
            ("↑↓", "move"),
            ("Enter", "play"),
            ("Del", "remove"),
            ("f g", "reorder"),
            ("Ctrl+1-4", "source"),
            ("F6", "keys"),
        ],
        ActivePanel::Library => &[
            ("↑↓", "move"),
            ("Enter", "queue"),
            ("→", "open"),
            ("←", "up"),
            ("/", "search"),
            ("F6", "keys"),
        ],
        ActivePanel::Search => &[
            ("type", "search"),
            ("Tab", "next group"),
            ("↑↓", "move"),
            ("Enter", "queue"),
            ("Esc", "done"),
        ],
        ActivePanel::NowPlaying => &[
            ("Space", "pause"),
            ("a d", "seek"),
            ("m", "lyrics"),
            ("y", "other lyrics"),
            ("v", "visualizer"),
            ("F6", "keys"),
        ],
        ActivePanel::Help if app.help_filtering => {
            &[("type", "filter"), ("Enter", "keep"), ("Esc", "clear")]
        }
        ActivePanel::Help => &[("/", "filter"), ("Esc", "back")],
    };
    let mut spans = Vec::new();
    let mut used = 0;
    for (key, action) in pairs {
        let gap = if spans.is_empty() { 0 } else { 3 };
        let w = gap + key.width() + 1 + action.width();
        if used + w > area.width as usize {
            break;
        }
        if gap > 0 {
            spans.push(Span::raw("   "));
        }
        spans.push(Span::styled(
            *key,
            Style::default().fg(C_DIM).add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(
            format!(" {}", action),
            Style::default().fg(C_INACTIVE),
        ));
        used += w;
    }
    let widget = Paragraph::new(Line::from(spans)).alignment(ratatui::layout::Alignment::Center);
    f.render_widget(widget, area);
}

/// Three rows under the cover: what plays, how far it is, and shuffle, repeat
/// and volume. Clicking the progress line seeks.
fn draw_mini_now_playing(f: &mut Frame, app: &mut App, area: Rect) {
    let Some(entry) = app.playlist.current_entry() else {
        return;
    };
    let w = area.width as usize;
    let title = entry
        .metadata
        .display_title(app.config.strip_track_numbers)
        .to_string();
    let artist = entry.metadata.display_artist().to_string();
    let paused = app.player.as_ref().map(|p| p.is_paused()).unwrap_or(true);
    let elapsed = app.display_elapsed_ms();
    let total = app.player.as_ref().map(|p| p.duration_ms()).unwrap_or(0);
    let volume = app.player.as_ref().map(|p| p.volume()).unwrap_or(100);

    let title_w = title.width().min(w.saturating_sub(2));
    let line1 = Line::from(vec![
        Span::styled(
            if paused { "⏸ " } else { "▶ " },
            Style::default().fg(C_ACCENT2),
        ),
        Span::styled(
            fit(&title, title_w),
            Style::default().fg(C_FG).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            fit(&format!(" · {}", artist), w.saturating_sub(2 + title_w)),
            Style::default().fg(C_DIM),
        ),
    ]);

    let line2 = if app.buffering {
        Line::from(Span::styled("buffering…", Style::default().fg(C_CYAN)))
    } else {
        let (left, right) = (format_time(elapsed), format_time(total));
        let bar_w = w.saturating_sub(left.width() + right.width() + 2);
        let ratio = if total > 0 {
            (elapsed as f64 / total as f64).min(1.0)
        } else {
            0.0
        };
        let filled = ((ratio * bar_w as f64).round() as usize).min(bar_w);
        app.ui_bounds.progress_bar_rect = Some(Rect::new(
            area.x + left.width() as u16 + 1,
            area.y + 1,
            bar_w as u16,
            1,
        ));
        Line::from(vec![
            Span::styled(left, Style::default().fg(C_DIM)),
            Span::raw(" "),
            Span::styled("━".repeat(filled), Style::default().fg(C_ACCENT2)),
            Span::styled("─".repeat(bar_w - filled), Style::default().fg(C_INACTIVE)),
            Span::raw(" "),
            Span::styled(right, Style::default().fg(C_DIM)),
        ])
    };

    let shuffle = if app.playlist.shuffle {
        "⤨ on"
    } else {
        "⤨ off"
    };
    let repeat = match app.playlist.repeat {
        crate::data::playlist::RepeatMode::Off => "↻ off",
        crate::data::playlist::RepeatMode::Track => "↻ track",
        crate::data::playlist::RepeatMode::Queue => "↻ queue",
    };
    let modes = format!("{}   {}", shuffle, repeat);
    let line3 = Line::from(vec![
        Span::styled(modes.clone(), Style::default().fg(C_INACTIVE)),
        Span::styled(
            fit_right(&format!("vol {}%", volume), w.saturating_sub(modes.width())),
            Style::default().fg(C_DIM),
        ),
    ]);

    f.render_widget(Paragraph::new(vec![line1, line2, line3]), area);
}

/// Main render function — dispatches to the appropriate view inside a global two-pane layout.
pub fn draw(f: &mut Frame, app: &mut App) {
    draw_screen(f, app);
    crate::ui::theme::adapt(f.buffer_mut());
}

fn draw_screen(f: &mut Frame, app: &mut App) {
    app.ui_bounds.reset();
    let size = f.area();

    // Screen-size guard: the layout requires at least 90 columns and 10 rows to render correctly.
    // On narrow or short terminals, show a friendly prompt instead of squashing the two-pane layout.
    if size.width < 90 || size.height < 10 {
        if size.width > 0 && size.height > 0 {
            let msg = format!(
                "Terminal window is too small for two-pane layout.\n\n\
                 Current terminal: {} \u{00d7} {}\n\
                 Recommended: at least 90 \u{00d7} 30\n\n\
                 Please increase the terminal window size.",
                size.width, size.height
            );
            let widget = ratatui::widgets::Paragraph::new(msg)
                .alignment(ratatui::layout::Alignment::Center)
                .style(ratatui::style::Style::default().fg(C_ACCENT2));
            f.render_widget(widget, size);
        }
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
    // Left pane: Sixel artwork + Mini-Controls (in Queue, Library, Search, Help tabs)
    // Right pane: Content and footer
    let art_width = artwork::art_pane_width(size.width);
    let h_layout = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(art_width), Constraint::Min(0)])
        .split(size);

    let art_area = h_layout[0];
    let right_area = h_layout[1];

    // Left pane: Sixel album art and mini-controls
    if art_width > 0 && art_area.height >= 8 && art_area.width >= 8 {
        let is_now_playing = app.active_panel == ActivePanel::NowPlaying;

        if !is_now_playing {
            // Queue, Library, Search, Help tabs: Both album art and mini-controls are vertically centered together in the middle of the left pane
            let left_margin = 2;
            let right_margin = 2;
            let max_w = art_area.width.saturating_sub(left_margin + right_margin);

            let aspect_ratio = artwork::get_cell_aspect_ratio();
            let h_if_full_w = (max_w as f32 / aspect_ratio) as u16;
            // Below the art: spacer and mini-controls, then spacer and 3 rows of now playing
            let below = if app.playlist.is_empty() { 2 } else { 6 };
            let max_art_h = art_area.height.saturating_sub(below + 2);
            let art_h = h_if_full_w.min(max_art_h).max(4);
            let art_w = (((art_h as f32) * aspect_ratio) as u16).min(max_w).max(4);

            let total_block_h = art_h + below;
            let v_space = art_area.height.saturating_sub(total_block_h);
            let top_offset = art_area.y + v_space / 2;

            let art_x = art_area.x + (art_area.width.saturating_sub(art_w)) / 2;
            let art_rect = Rect::new(art_x, top_offset, art_w, art_h);

            if !app.playlist.is_empty() {
                artwork::render_artwork(f, art_rect, &mut app.current_cover_protocol);
            }

            // Mini-controls placed directly below the album art, centered as part of the middle block
            let ctrl_y = art_rect.y + art_rect.height + 1;
            if ctrl_y < art_area.y + art_area.height {
                let ctrl_area = Rect::new(art_area.x, ctrl_y, art_area.width, 1);
                draw_mini_controls(f, app, ctrl_area);
            }

            // What plays, as wide as the art (at least 24 columns), under the controls
            let bar_y = ctrl_y + 2;
            if !app.playlist.is_empty() && bar_y + 3 <= art_area.y + art_area.height {
                let bar_w = art_w.max(24).min(art_area.width.saturating_sub(2));
                let bar_x = art_area.x + (art_area.width - bar_w) / 2;
                draw_mini_now_playing(f, app, Rect::new(bar_x, bar_y, bar_w, 3));
            }
        } else {
            // Track tab (NowPlaying): Vertically centered artwork, NO mini-controls
            if !app.playlist.is_empty() {
                let art_box = Rect::new(
                    art_area.x + 2,
                    art_area.y + 2,
                    art_area.width.saturating_sub(4),
                    art_area.height.saturating_sub(4),
                );
                let art_rect = artwork::compute_art_rect(art_box, false);
                artwork::render_artwork(f, art_rect, &mut app.current_cover_protocol);
            }
        }
    }

    // Split the right pane vertically: Content area + Footer at the absolute bottom of the right pane
    let right_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(3),    // Content area
            Constraint::Length(2), // Source bar + Footer / tab bar
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
            branding::render_logo_with_song(f, logo_rect, song_name);
        }

        if let Some(inst_rect) = instructions_area {
            render_instructions(f, inst_rect, app);
        }

        app.ui_bounds.left_panel_rect = Some(tab_area);
        if (app.source == crate::app::SourceTab::Spotify
            || app.source == crate::app::SourceTab::YouTube)
            && app.active_panel != ActivePanel::Help
            && app.active_panel != ActivePanel::Queue
        {
            match app.active_panel {
                ActivePanel::Library => draw_browse(f, app, tab_area),
                ActivePanel::Search => draw_remote_search(f, app, tab_area),
                _ => render_empty(f, tab_area, "View not available for this source."),
            }
        } else {
            match app.active_panel {
                ActivePanel::Queue => draw_queue(f, app, tab_area),
                ActivePanel::Library => draw_library(f, app, tab_area),
                ActivePanel::Search => draw_search(f, app, tab_area),
                ActivePanel::Help => draw_help(f, app, tab_area),
                ActivePanel::NowPlaying => unreachable!(),
            }
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
            Constraint::Length(6), // 0. Top Logo
            Constraint::Length(1), // 1. Shortcut instructions
            Constraint::Length(4), // 2. Metadata
            Constraint::Length(1), // 3. Spacer
            Constraint::Fill(1),   // 4. Scrollable Lyrics
            Constraint::Length(1), // 5. Spacer
            Constraint::Length(2), // 6. Progress bar + statistics
        ];
        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints(constraints)
            .split(area);

        branding::render_logo_with_song(f, layout[0], song_name);
        render_instructions(f, layout[1], app);
        draw_metadata(f, app, layout[2]);
        draw_lyrics(f, app, layout[4]);
        draw_progress(f, app, layout[6]);
        return;
    }

    // Normal mode: Logo + Instructions + Metadata + Spacer + Lyrics + Spacer + Visualizer + Spacer + Progress
    let constraints = vec![
        Constraint::Length(6), // 0. Top Logo
        Constraint::Length(1), // 1. Shortcut instructions
        Constraint::Length(4), // 2. Metadata (centered)
        Constraint::Length(1), // 3. Spacer
        Constraint::Length(3), // 4. Lyrics Engine (max 3 lines, toggleable with 'm')
        Constraint::Length(1), // 5. Spacer
        Constraint::Fill(1),   // 6. Visualizer
        Constraint::Length(1), // 7. Spacer
        Constraint::Length(2), // 8. Bottom Progress Bar
    ];

    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);

    branding::render_logo_with_song(f, layout[0], song_name);
    render_instructions(f, layout[1], app);
    draw_metadata(f, app, layout[2]);
    draw_lyrics(f, app, layout[4]);
    draw_visualizer(f, app, layout[6]);
    draw_progress(f, app, layout[8]);
}

fn draw_metadata(f: &mut Frame, app: &mut App, area: Rect) {
    if let Some(entry) = app.playlist.current_entry() {
        let meta = &entry.metadata;
        let title_str = meta.display_title(app.config.strip_track_numbers);
        let title_len = unicode_width::UnicodeWidthStr::width(title_str) as u16;
        let title_w = title_len.min(area.width);
        let title_x = area.x + area.width.saturating_sub(title_w) / 2;
        app.ui_bounds.title_rect = Some(Rect::new(title_x, area.y, title_w, 1));

        let artist_str = meta.display_artist();
        let artist_len = unicode_width::UnicodeWidthStr::width(artist_str) as u16;
        let artist_w = artist_len.min(area.width);
        let artist_x = area.x + area.width.saturating_sub(artist_w) / 2;
        app.ui_bounds.artist_rect = Some(Rect::new(artist_x, area.y + 1, artist_w, 1));

        let album = match meta.year {
            Some(year) => format!("{}  ·  {}", meta.display_album(), year),
            None => meta.display_album().to_string(),
        };
        let mut lines = vec![
            Line::from(Span::styled(
                title_str,
                Style::default().fg(C_ACCENT2).add_modifier(Modifier::BOLD),
            )),
            Line::from(Span::styled(
                artist_str,
                Style::default().fg(C_FG).add_modifier(Modifier::UNDERLINED),
            )),
            Line::from(Span::styled(album, Style::default().fg(C_DIM))),
        ];
        // What the file is and its quality
        if let Some(summary) = &app.audio_summary {
            lines.push(Line::from(Span::styled(
                format!(" {} ", summary),
                Style::default().fg(C_DIM).bg(C_ROW_BG),
            )));
        }
        let widget = Paragraph::new(lines).alignment(ratatui::layout::Alignment::Center);
        f.render_widget(widget, area);
    }
}

fn draw_mini_controls(f: &mut Frame, app: &mut App, area: Rect) {
    if area.height == 0 || area.width < 10 {
        app.ui_bounds.mini_controls_rect = None;
        return;
    }
    app.ui_bounds.mini_controls_rect = Some(area);

    let is_paused = app.player.as_ref().map(|p| p.is_paused()).unwrap_or(true);
    let play_symbol = if is_paused { "▶" } else { "⏸" };

    let spans = vec![
        Span::styled("⏮", Style::default().fg(C_FG).add_modifier(Modifier::BOLD)),
        Span::styled("   ", Style::default().fg(C_DIM)),
        Span::styled(
            play_symbol,
            Style::default().fg(C_ACCENT2).add_modifier(Modifier::BOLD),
        ),
        Span::styled("   ", Style::default().fg(C_DIM)),
        Span::styled("⏭", Style::default().fg(C_FG).add_modifier(Modifier::BOLD)),
        Span::styled("   ", Style::default().fg(C_DIM)),
        Span::styled(
            "+",
            Style::default().fg(C_CYAN).add_modifier(Modifier::BOLD),
        ),
        Span::styled("   ", Style::default().fg(C_DIM)),
        Span::styled(
            "-",
            Style::default().fg(C_CYAN).add_modifier(Modifier::BOLD),
        ),
        Span::styled("   ", Style::default().fg(C_DIM)),
        Span::styled("∅", Style::default().fg(C_DIM).add_modifier(Modifier::BOLD)),
    ];

    let widget = Paragraph::new(Line::from(spans)).alignment(ratatui::layout::Alignment::Center);
    f.render_widget(widget, area);
}

fn draw_lyrics(f: &mut Frame, app: &App, area: Rect) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    f.render_widget(ratatui::widgets::Clear, area);
    if let Some(ref picker) = app.lyrics_picker {
        draw_lyrics_picker(f, app, picker, area);
        return;
    }
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
            if let Some(ref lines) = app.current_plain_lyrics {
                lyrics_widget::render_untimed_lyrics(f, area, lines, app.lyrics_scroll);
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
                    let msg = if app.lyrics_searching {
                        "Searching for lyrics..."
                    } else {
                        "No lyrics found. Press 'y' to search, 'm' to go back"
                    };
                    let hint =
                        Paragraph::new(Line::from(Span::styled(msg, Style::default().fg(C_DIM))))
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
            let lyrics = match &app.current_plain_lyrics {
                Some(_) => &LyricsKind::Untimed(Vec::new()),
                None => &entry.metadata.lyrics,
            };
            match lyrics {
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
                LyricsKind::None if app.lyrics_searching => {
                    let hint = Paragraph::new(Line::from(Span::styled(
                        "Searching for lyrics...",
                        Style::default().fg(C_DIM),
                    )))
                    .alignment(ratatui::layout::Alignment::Center);
                    f.render_widget(hint, area);
                }
                LyricsKind::None => {}
            }
        }
    }
}

/// The list of other lyrics for the playing track (opened with `y`).
fn draw_lyrics_picker(f: &mut Frame, app: &App, picker: &crate::app::LyricsPicker, area: Rect) {
    use crate::sources::LyricsSync;

    let mut rows = vec![
        "Automatic: search again".to_string(),
        "No lyrics for this track".to_string(),
    ];
    rows.extend(picker.matches.iter().map(|m| {
        let mut row = format!("{} - {}", m.title, m.artist);
        if let Some(album) = m.album.as_deref().filter(|a| !a.is_empty()) {
            row.push_str(&format!(" | {}", album));
        }
        if let Some(secs) = m.duration_secs {
            row.push_str(&format!(" | {}:{:02}", secs / 60, secs % 60));
        }
        let sync = match m.sync {
            LyricsSync::Word => "word-synced",
            LyricsSync::Line => "synced",
            LyricsSync::Plain => "plain text",
        };
        row.push_str(&format!(" | {} | {}", sync, m.provider));
        row
    }));

    let title = app
        .playlist
        .current_entry()
        .map(|e| e.metadata.display_title(app.config.strip_track_numbers))
        .unwrap_or_default();
    let state = if picker.loading {
        " (searching...)".to_string()
    } else if let Some(ref error) = picker.error {
        format!(" ({})", error)
    } else {
        format!(" ({} found)", picker.matches.len())
    };
    let mut lines = vec![Line::from(vec![
        Span::styled(
            format!("  Lyrics for {}", title),
            Style::default().fg(C_ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::styled(state, Style::default().fg(C_DIM)),
    ])];

    // Header and footer take a line each; the list scrolls to keep the cursor in view
    let visible = (area.height as usize).saturating_sub(2).max(1);
    let first = (picker.cursor + 1).saturating_sub(visible);
    for (idx, row) in rows.iter().enumerate().skip(first).take(visible) {
        let (prefix, style) = if idx == picker.cursor {
            (
                "  > ",
                Style::default().fg(C_ACCENT2).add_modifier(Modifier::BOLD),
            )
        } else {
            ("    ", Style::default().fg(C_FG))
        };
        lines.push(Line::from(Span::styled(
            format!("{}{}", prefix, row),
            style,
        )));
    }

    let footer = if let Some((_, ref path)) = picker.confirm_replace {
        Span::styled(
            format!(
                "  {} is your own file. Replace it? y: yes, any other key: no",
                path.file_name().unwrap_or_default().to_string_lossy()
            ),
            Style::default().fg(C_ORANGE).add_modifier(Modifier::BOLD),
        )
    } else if let Some(ref input) = picker.input {
        Span::styled(
            format!("  Search: {}█  (Enter: search, Esc: cancel)", input),
            Style::default().fg(C_ACCENT2),
        )
    } else {
        Span::styled(
            "  Enter: use  |  /: search with your own words  |  Esc: close",
            Style::default().fg(C_DIM),
        )
    };
    lines.push(Line::from(footer));

    f.render_widget(Paragraph::new(lines), area);
}

fn draw_visualizer(f: &mut Frame, app: &mut App, area: Rect) {
    if area.height < 2 || area.width == 0 {
        return;
    }
    f.render_widget(ratatui::widgets::Clear, area);
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

/// Renders a mini volume bar: "▮▮▮▮▮▯▯▯" (8 cells) (Q4)
fn volume_bar(vol: u8) -> String {
    const TOTAL: usize = 8;
    let filled = (((vol as usize) * TOTAL + 50) / 100).min(TOTAL);
    let empty = TOTAL - filled;
    format!("{}{}", "▮".repeat(filled), "▯".repeat(empty))
}

fn draw_progress(f: &mut Frame, app: &mut App, area: Rect) {
    if area.height < 2 {
        app.ui_bounds.progress_bar_rect = None;
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

    let elapsed_str = format_time(elapsed_ms);
    let total_str = format_time(total_ms);

    if app.buffering {
        let is_yt = matches!(
            app.playlist.current_entry().map(|e| &e.id),
            Some(crate::data::track::TrackRef::YouTube(_))
        );
        let label = if is_yt {
            "downloading…"
        } else {
            "buffering…"
        };
        let label_w = (label.len() as u16).min(active_area.width);
        let left_rect = Rect::new(active_area.x, active_area.y, label_w, 1);
        f.render_widget(
            Paragraph::new(label).style(Style::default().fg(C_CYAN)),
            left_rect,
        );
        let bar_w = active_area.width.saturating_sub(label_w + 1);
        let bar_area = Rect::new(active_area.x + label_w + 1, active_area.y, bar_w, 1);
        app.ui_bounds.progress_bar_rect = Some(bar_area);
        let gauge = Gauge::default()
            .gauge_style(Style::default().fg(C_ACCENT2).bg(C_SURFACE))
            .ratio(0.0)
            .label("");
        f.render_widget(gauge, bar_area);
    } else {
        // Flank seek bar with timestamps (Q5)
        let label_w: u16 = 6;
        let bar_w = active_area.width.saturating_sub(label_w * 2);

        // Left timestamp
        let left_rect = Rect::new(active_area.x, active_area.y, label_w, 1);
        f.render_widget(
            Paragraph::new(format!("{:>5} ", elapsed_str)).style(Style::default().fg(C_DIM)),
            left_rect,
        );

        // Progress bar (narrower, between timestamps)
        let bar_area = Rect::new(active_area.x + label_w, active_area.y, bar_w, 1);
        app.ui_bounds.progress_bar_rect = Some(bar_area);
        let gauge = Gauge::default()
            .gauge_style(Style::default().fg(C_ACCENT2).bg(C_SURFACE))
            .ratio(ratio)
            .label("");
        f.render_widget(gauge, bar_area);

        // Right timestamp
        let right_rect = Rect::new(active_area.x + label_w + bar_w, active_area.y, label_w, 1);
        f.render_widget(
            Paragraph::new(format!(" {}", total_str)).style(Style::default().fg(C_DIM)),
            right_rect,
        );
    }

    // Info line
    if active_area.height >= 2 {
        let info_area = Rect::new(active_area.x, active_area.y + 1, active_area.width, 1);

        let pct = (ratio * 100.0) as u32;
        let vol = app.player.as_ref().map(|p| p.volume()).unwrap_or(100);
        let repeat = app.playlist.repeat.symbol();
        let shuffle = if app.playlist.shuffle { "⤨" } else { "" };

        let status = if app.player.as_ref().map(|p| p.is_paused()).unwrap_or(false) {
            "⏸"
        } else {
            "▶"
        };
        let bitrate_str = app
            .now_playing_meta
            .as_ref()
            .and_then(|m| m.bitrate)
            .map(|b| format!("{}kbps", b))
            .unwrap_or_default();

        let vol_bar = volume_bar(vol);
        let info = format!(
            "{} {}/{} ({}%)  {} {}%  {} {} {}",
            status, elapsed_str, total_str, pct, vol_bar, vol, repeat, shuffle, bitrate_str
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

    // Where each track stands in the play order: before the playing one means played
    let mut play_pos = vec![usize::MAX; app.playlist.len()];
    for (pos, &entry_idx) in app.playlist.play_order.iter().enumerate() {
        if let Some(slot) = play_pos.get_mut(entry_idx) {
            *slot = pos;
        }
    }
    let playing_real = app.playlist.current_real_index();
    let current_pos = app.playlist.current;

    // Header: size of the queue, and what is still to come
    let n = app.playlist.len();
    let mut header = vec![Span::styled(
        format!(
            "  Queue  ·  {} {}  ·  {}",
            n,
            if n == 1 { "track" } else { "tracks" },
            format_total(app.playlist.total_duration_secs())
        ),
        Style::default().fg(C_DIM),
    )];
    if playing_real.is_some() {
        let up_next = app
            .playlist
            .play_order
            .get(current_pos + 1..)
            .unwrap_or(&[]);
        let left_secs: u64 = up_next
            .iter()
            .filter_map(|&i| app.playlist.entries.get(i)?.metadata.duration)
            .map(|d| d.as_secs())
            .sum();
        header.push(Span::styled(
            format!(
                "     up next {}  ·  {}",
                up_next.len(),
                format_total(left_secs)
            ),
            Style::default().fg(C_INACTIVE),
        ));
    }
    f.render_widget(
        Paragraph::new(Line::from(header)),
        Rect::new(area.x, area.y, area.width, 1),
    );
    if area.height < 3 {
        return;
    }

    // Columns: bar, number, title, artist, album (when there is room), source, time
    let width = area.width.saturating_sub(1) as usize; // the scrollbar column
                                                       // Fixed cells: bar 1, number 5 + 1, gaps 2 + 2 (+ 2 before the album), source 1 + 1, time 5
    let with_album = width >= 20 + 50;
    let room = width.saturating_sub(if with_album { 20 } else { 18 });
    let (title_w, artist_w, album_w) = if with_album {
        let t = room * 45 / 100;
        let a = room * 30 / 100;
        (t, a, room - t - a)
    } else {
        let t = room * 60 / 100;
        (t, room - t, 0)
    };
    let column_line = |num: String,
                       title: String,
                       artist: String,
                       album: String,
                       dot: String,
                       time: String,
                       styles: [Style; 6]| {
        let mut spans = vec![
            Span::styled(fit_right(&num, 5), styles[0]),
            Span::styled(" ", styles[0]),
            Span::styled(fit(&title, title_w), styles[1]),
            Span::styled("  ", styles[1]),
            Span::styled(fit(&artist, artist_w), styles[2]),
            Span::styled("  ", styles[2]),
        ];
        if with_album {
            spans.push(Span::styled(fit(&album, album_w), styles[3]));
            spans.push(Span::styled("  ", styles[3]));
        }
        spans.push(Span::styled(fit(&dot, 1), styles[4]));
        spans.push(Span::styled(" ", styles[4]));
        spans.push(Span::styled(fit_right(&time, 5), styles[5]));
        spans
    };
    let faint = Style::default().fg(C_INACTIVE);
    let mut column_titles = vec![Span::raw(" ")];
    column_titles.extend(column_line(
        "#".into(),
        "Title".into(),
        "Artist".into(),
        "Album".into(),
        String::new(),
        "Time".into(),
        [faint; 6],
    ));
    f.render_widget(
        Paragraph::new(Line::from(column_titles)),
        Rect::new(area.x, area.y + 1, area.width, 1),
    );

    let list_area = Rect::new(area.x, area.y + 2, area.width, area.height - 2);
    let visual_items = app
        .playlist
        .get_visual_items(app.show_folders, app.config.strip_track_numbers);
    let visible_height = list_area.height as usize;
    let half = visible_height / 2;

    // Keep the cursor's row in the middle of the list
    let highlighted_visual_idx = visual_items
        .iter()
        .position(|item| {
            matches!(item, QueueVisualItem::Track { entry_idx, .. } if *entry_idx == app.queue_cursor)
        })
        .unwrap_or(0);
    let start = highlighted_visual_idx
        .saturating_sub(half)
        .min(visual_items.len().saturating_sub(1));
    let end = (start + visible_height).min(visual_items.len()).max(start);

    let mut display_lines = Vec::new();
    let mut rows = Vec::new();
    for (row, item) in visual_items[start..end].iter().enumerate() {
        match item {
            QueueVisualItem::Header { name } => {
                let style = Style::default().fg(C_ORANGE).add_modifier(Modifier::BOLD);
                display_lines.push(Line::from(vec![
                    Span::styled("  📁 ", style),
                    Span::styled(name.clone(), style),
                ]));
            }
            QueueVisualItem::Separator => display_lines.push(Line::from("  ")),
            QueueVisualItem::Track { entry_idx, .. } => {
                let Some(entry) = app.playlist.entries.get(*entry_idx) else {
                    display_lines.push(Line::from(""));
                    continue;
                };
                rows.push((list_area.y + row as u16, *entry_idx));
                let meta = &entry.metadata;
                let is_playing = Some(*entry_idx) == playing_real;
                let is_cursor = *entry_idx == app.queue_cursor;
                let played = !is_playing
                    && playing_real.is_some()
                    && play_pos.get(*entry_idx).is_some_and(|&p| p < current_pos);
                let source = match entry.id {
                    crate::data::track::TrackRef::Local(_) => crate::app::SourceTab::Local,
                    crate::data::track::TrackRef::Spotify(_) => crate::app::SourceTab::Spotify,
                    crate::data::track::TrackRef::YouTube(_) => crate::app::SourceTab::YouTube,
                };

                // Playing: accent and ▶; played: dimmed; the cursor: a row background
                let mut styles = if is_playing {
                    [Style::default().fg(C_ACCENT2).add_modifier(Modifier::BOLD); 6]
                } else if played {
                    [Style::default().fg(C_INACTIVE); 6]
                } else {
                    [
                        Style::default().fg(C_INACTIVE),
                        Style::default().fg(C_FG),
                        Style::default().fg(C_DIM),
                        Style::default().fg(C_INACTIVE),
                        Style::default(),
                        Style::default().fg(C_DIM),
                    ]
                };
                styles[4] = styles[4].fg(accent_for(source));
                if is_cursor {
                    for style in &mut styles {
                        *style = style.bg(C_ROW_BG);
                    }
                    styles[1] = styles[1].add_modifier(Modifier::BOLD);
                }
                let num = if is_playing {
                    "▶".to_string()
                } else {
                    (entry_idx + 1).to_string()
                };
                let mut spans = vec![Span::styled(
                    if is_playing { "▎" } else { " " },
                    styles[0].fg(C_ACCENT2),
                )];
                spans.extend(column_line(
                    num,
                    meta.display_title(app.config.strip_track_numbers)
                        .to_string(),
                    meta.display_artist().to_string(),
                    meta.album.clone().unwrap_or_default(),
                    "●".to_string(),
                    meta.duration
                        .map(|d| format_secs(d.as_secs()))
                        .unwrap_or_default(),
                    styles,
                ));
                display_lines.push(Line::from(spans));
            }
        }
    }
    let total_items = visual_items.len();
    drop(visual_items);
    app.ui_bounds.rows = rows;

    f.render_widget(Paragraph::new(display_lines), list_area);

    if total_items > visible_height {
        let scrollbar_col = list_area.x + list_area.width.saturating_sub(1);
        app.ui_bounds.scrollbar_rect =
            Some(Rect::new(scrollbar_col, list_area.y, 1, list_area.height));
        render_scrollbar(f, list_area, total_items, visible_height, start);
    }
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

    // Row 0, the breadcrumb: where you are and what this folder holds. Selecting it
    // queues the whole folder, as before.
    let top = app.flat_library.iter().filter(|item| item.depth == 0);
    let (folders, tracks, secs) = top.fold((0, 0, 0), |(d, t, s), item| {
        (
            d + usize::from(item.entry.is_dir()),
            t + item.tracks,
            s + item.duration_secs,
        )
    });
    let all_enqueued = app.flat_library.iter().all(|item| item.enqueued);
    let trail = std::iter::once("Local Library")
        .chain(app.local_nav_stack.iter().map(|(_, name, _)| name.as_str()))
        .collect::<Vec<_>>()
        .join(" › ");
    let mut counts = Vec::new();
    if folders > 0 {
        counts.push(format!(
            "{} {}",
            folders,
            if folders == 1 { "folder" } else { "folders" }
        ));
    }
    counts.push(format!(
        "{} {}",
        tracks,
        if tracks == 1 { "track" } else { "tracks" }
    ));
    counts.push(format_total(secs));
    let is_header_cursor = app.library_cursor == 0;
    let header_style = if is_header_cursor {
        Style::default().fg(C_CYAN).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(C_CYAN)
    };
    let header = Line::from(vec![
        Span::styled(
            if all_enqueued { "✓ " } else { "  " },
            Style::default().fg(C_GREEN),
        ),
        Span::styled(trail, header_style),
        Span::styled(
            format!("   {}", counts.join("  ·  ")),
            Style::default().fg(C_INACTIVE),
        ),
    ]);

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

    // Room for the name; the rest of the row holds counts or the track length
    let width = area.width.saturating_sub(1) as usize;
    const INFO_W: usize = 22;
    let mut list_items = Vec::new();
    let mut rows = Vec::new();

    for i in start..end {
        rows.push((area.y + (i - start) as u16, i));
        if i == 0 {
            list_items.push(ListItem::new(header.clone()));
            continue;
        }
        let item = &app.flat_library[i - 1];
        let entry = &item.entry;
        let is_cursor = i == app.library_cursor;
        let is_playing = if let LibraryEntry::Track { path, .. } = entry {
            app.playlist.current_entry().map(|e| &e.id)
                == Some(&crate::data::track::TrackRef::Local(path.clone()))
        } else {
            false
        };

        let (icon, style) = if entry.is_dir() {
            let s = if is_cursor {
                Style::default().fg(C_CYAN).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(C_FG)
            };
            ("📁 ", s)
        } else {
            let s = row_style(is_cursor, is_playing);
            (if is_playing { "▶  " } else { "♪  " }, s)
        };

        let mut tree = String::new();
        if !item.ancestor_last.is_empty() || item.is_last {
            for &ancestor_is_last in &item.ancestor_last {
                tree.push_str(if ancestor_is_last { "    " } else { "│   " });
            }
            tree.push_str(if item.is_last {
                "└── "
            } else {
                "├── "
            });
        }
        let info = if entry.is_dir() {
            format!(
                "{} {}  ·  {}",
                item.tracks,
                if item.tracks == 1 { "track" } else { "tracks" },
                format_total(item.duration_secs)
            )
        } else {
            match entry {
                LibraryEntry::Track { metadata, .. } => metadata
                    .duration
                    .map(|d| format_secs(d.as_secs()))
                    .unwrap_or_default(),
                _ => String::new(),
            }
        };
        let name_w = width.saturating_sub(2 + tree.width() + 3 + INFO_W).max(4);

        list_items.push(ListItem::new(Line::from(vec![
            // Queued: a check mark
            Span::styled(
                if item.enqueued { "✓ " } else { "  " },
                Style::default().fg(C_GREEN),
            ),
            Span::styled(tree, style),
            Span::styled(icon, style),
            Span::styled(fit(entry.name(), name_w), style),
            Span::styled(fit_right(&info, INFO_W), Style::default().fg(C_INACTIVE)),
        ])));
    }
    app.ui_bounds.rows = rows;

    let list = List::new(list_items);
    f.render_widget(list, area);

    // Render vertical scrollbar if library exceeds visible height
    if total_items > visible_height {
        let scrollbar_col = area.x + area.width.saturating_sub(1);
        app.ui_bounds.scrollbar_rect = Some(Rect::new(scrollbar_col, area.y, 1, area.height));

        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .thumb_symbol("█")
            .track_symbol(None)
            .begin_symbol(None)
            .end_symbol(None)
            .thumb_style(Style::default().fg(C_ACCENT2));
        let mut scrollbar_state =
            ScrollbarState::new(total_items.saturating_sub(visible_height)).position(start);
        f.render_stateful_widget(scrollbar, area, &mut scrollbar_state);
    }
}

/// One row of a remote library tree or search result list, drawn like the local
/// library: queued check mark, indentation by depth, folder/track icon, title,
/// subtitle, and the length of a track at the end.
fn browse_row<'a>(
    item: &'a crate::sources::BrowseItem,
    is_cursor: bool,
    _expanded: &std::collections::HashSet<String>,
    playlist: &crate::data::playlist::Playlist,
    width: usize,
) -> Line<'a> {
    let is_playing = item
        .track_ref
        .as_ref()
        .is_some_and(|tr| playlist.current_entry().map(|e| &e.id) == Some(tr));
    let enqueued = item
        .track_ref
        .as_ref()
        .is_some_and(|tr| playlist.entry_ids.contains(tr));

    let style = if item.is_container {
        let s = Style::default().fg(if is_cursor { C_CYAN } else { C_FG });
        if is_cursor {
            s.add_modifier(Modifier::BOLD)
        } else {
            s
        }
    } else {
        row_style(is_cursor, is_playing)
    };
    let icon = if item.is_container {
        "📁 "
    } else if item.id.starts_with(crate::sources::ADD_ALL_PREFIX) {
        "+  "
    } else if is_playing {
        "▶  "
    } else {
        "♪  "
    };
    let indent = if item.depth > 0 {
        "    ".repeat(item.depth)
    } else {
        String::new()
    };
    let length = item.duration_secs.map(format_secs).unwrap_or_default();

    // The title has priority; the subtitle only uses what is left of the row.
    // marker (2) + indent + icon (3) + length (6) + scrollbar column
    let title_room = width.saturating_sub(2 + indent.len() + 3 + 6 + 1);
    let title = truncate_to_width(&item.title, title_room);
    let sub_room = title_room
        .saturating_sub(unicode_width::UnicodeWidthStr::width(title) + 3)
        .min(28);
    let sub = if sub_room >= 4 {
        truncate_to_width(item.subtitle.as_deref().unwrap_or(""), sub_room)
    } else {
        ""
    };
    let used = title.width() + if sub.is_empty() { 0 } else { 3 + sub.width() };

    Line::from(vec![
        Span::styled(
            if enqueued { "✓ " } else { "  " },
            Style::default().fg(C_GREEN),
        ),
        Span::styled(indent, style),
        Span::styled(icon, style),
        Span::styled(title, style),
        Span::styled(
            if sub.is_empty() { "" } else { " • " },
            Style::default().fg(C_DIM),
        ),
        Span::styled(sub, Style::default().fg(C_DIM)),
        Span::styled(
            fit_right(&length, title_room.saturating_sub(used) + 6),
            Style::default().fg(C_INACTIVE),
        ),
    ])
}

/// The breadcrumb of a remote library: where you are and what the open level holds.
fn browse_breadcrumb(view: &crate::sources::SourceView, source_name: &str) -> Line<'static> {
    let trail = std::iter::once(source_name)
        .chain(view.nav_stack.iter().map(|s| s.title.as_str()))
        .collect::<Vec<_>>()
        .join(" › ");
    let top = view.flat.iter().filter(|item| item.depth == 0);
    let (folders, tracks, secs) = top.fold((0, 0, 0), |(d, t, s), item| {
        (
            d + usize::from(item.is_container),
            t + usize::from(item.track_ref.is_some()),
            s + item.duration_secs.unwrap_or(0),
        )
    });
    let mut counts = Vec::new();
    if folders > 0 {
        counts.push(format!(
            "{} {}",
            folders,
            if folders == 1 { "folder" } else { "folders" }
        ));
    }
    if tracks > 0 {
        counts.push(format!(
            "{} {}",
            tracks,
            if tracks == 1 { "track" } else { "tracks" }
        ));
        counts.push(format_total(secs));
    }
    Line::from(vec![
        Span::styled(format!("  {}", trail), Style::default().fg(C_CYAN)),
        Span::styled(
            format!("   {}", counts.join("  ·  ")),
            Style::default().fg(C_INACTIVE),
        ),
    ])
}

// ─── Remote Browse View (Spotify / YouTube) ─────────────────────────────────
fn draw_browse(f: &mut Frame, app: &mut App, area: Rect) {
    if area.width < 5 || area.height < 1 {
        return;
    }

    let source_name = match app.source {
        crate::app::SourceTab::Spotify => "Spotify",
        crate::app::SourceTab::YouTube => "YouTube Music",
        _ => "Remote",
    };

    let view = match app.active_source_view() {
        Some(v) => v.clone(),
        None => return,
    };

    if !view.connected || view.awaiting_login_input {
        let (status, status_color) =
            if view.connected && app.source == crate::app::SourceTab::YouTube {
                ("Connected. Paste a cookie header to sign in with it.", C_FG)
            } else if view.connected {
                ("Connected. Enter a Client ID to sign in with it.", C_FG)
            } else {
                ("Not connected. Press Enter to sign in.", C_RED)
            };
        let mut lines = vec![
            Line::from(""),
            Line::from(vec![
                Span::styled("  ", Style::default()),
                Span::styled(
                    format!("{} Source", source_name),
                    Style::default().fg(C_ACCENT).add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::from(vec![
                Span::styled("  Status: ", Style::default().fg(C_DIM)),
                Span::styled(status, Style::default().fg(status_color)),
            ]),
            Line::from(""),
        ];

        match app.source {
            crate::app::SourceTab::Spotify => {
                lines.push(Line::from(Span::styled(
                    "  • Requires Spotify Premium account.",
                    Style::default().fg(C_FG),
                )));
                lines.push(Line::from(Span::styled(
                    "  • Press Enter to sign in via Spotify in your browser.",
                    Style::default().fg(C_FG),
                )));
                lines.push(Line::from(Span::styled(
                    "  • (Optional) Press c to sign in with your own Client ID.",
                    Style::default().fg(C_DIM),
                )));
            }
            crate::app::SourceTab::YouTube => {
                lines.push(Line::from(Span::styled(
                    "  • Press Enter: a browser window opens just for mixed.",
                    Style::default().fg(C_FG),
                )));
                lines.push(Line::from(Span::styled(
                    "  • Sign in to YouTube Music there, then close the window.",
                    Style::default().fg(C_FG),
                )));
                lines.push(Line::from(Span::styled(
                    "  • Search and play work without signing in (F5).",
                    Style::default().fg(C_DIM),
                )));
                lines.push(Line::from(Span::styled(
                    "  • (Optional) Press c to paste a cookie header instead.",
                    Style::default().fg(C_DIM),
                )));
            }
            _ => {}
        }

        if let Some(ref err) = view.error {
            lines.push(Line::from(""));
            lines.push(Line::from(vec![
                Span::styled("  Error: ", Style::default().fg(C_RED)),
                Span::styled(err.as_str(), Style::default().fg(C_RED)),
            ]));
        }

        if let Some(ref status) = view.login_status {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                format!("  {}", status),
                Style::default().fg(C_ACCENT2).add_modifier(Modifier::BOLD),
            )));
        }

        if view.awaiting_login_input {
            lines.push(Line::from(""));
            lines.push(Line::from(vec![
                Span::styled(
                    "  Credentials: ",
                    Style::default().fg(C_ACCENT2).add_modifier(Modifier::BOLD),
                ),
                Span::styled(view.login_input.as_str(), Style::default().fg(C_FG)),
                Span::styled("█", Style::default().fg(C_ACCENT2)),
            ]));
        }

        let widget = Paragraph::new(lines).alignment(ratatui::layout::Alignment::Left);
        f.render_widget(widget, area);
        return;
    }

    if view.loading {
        let lines = vec![
            Line::from(""),
            Line::from(Span::styled(
                format!("  Loading {}...", source_name),
                Style::default().fg(C_ACCENT2).add_modifier(Modifier::BOLD),
            )),
        ];
        let widget = Paragraph::new(lines).alignment(ratatui::layout::Alignment::Left);
        f.render_widget(widget, area);
        return;
    }

    if view.flat.is_empty() {
        let lines = vec![
            Line::from(Span::styled(
                if view.nav_stack.is_empty() {
                    format!("  - {} -", view.current_path_display(source_name))
                } else {
                    format!(
                        "  - {} (Backspace: up) -",
                        view.current_path_display(source_name)
                    )
                },
                Style::default().fg(C_CYAN),
            )),
            Line::from(""),
            Line::from(Span::styled(
                format!("  No items found in {} library.", source_name),
                Style::default().fg(C_DIM),
            )),
            Line::from(Span::styled(
                if view.nav_stack.is_empty() {
                    "  Press F5 to switch to Search."
                } else {
                    "  Press Backspace to go back to parent."
                },
                Style::default().fg(C_DIM),
            )),
        ];
        let widget = Paragraph::new(lines).alignment(ratatui::layout::Alignment::Left);
        f.render_widget(widget, area);
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // Breadcrumb path header
            Constraint::Min(0),    // Item list
        ])
        .split(area);

    f.render_widget(
        Paragraph::new(browse_breadcrumb(&view, source_name)),
        chunks[0],
    );

    let list_area = chunks[1];
    let visible_height = list_area.height as usize;
    let (start, end) = scroll_offset(view.cursor, view.flat.len(), visible_height);
    let mut display_lines = Vec::new();

    app.ui_bounds.rows = (start..end)
        .map(|i| (list_area.y + (i - start) as u16, i))
        .collect();
    for (idx, item) in view.flat[start..end].iter().enumerate() {
        display_lines.push(browse_row(
            item,
            start + idx == view.cursor,
            &view.expanded,
            &app.playlist,
            list_area.width as usize,
        ));
    }

    let widget = Paragraph::new(display_lines).alignment(ratatui::layout::Alignment::Left);
    f.render_widget(widget, list_area);

    if view.flat.len() > visible_height {
        render_scrollbar(f, list_area, view.flat.len(), visible_height, start);
    }
}

/// The row under a search box: one chip per group of results, the shown one marked.
/// Tab moves to the next chip.
fn scope_chips(
    groups: &[(crate::sources::ResultGroup, usize)],
    scope: Option<crate::sources::ResultGroup>,
) -> Line<'static> {
    let total: usize = groups.iter().map(|&(_, n)| n).sum();
    let chips = std::iter::once((None, "All", total))
        .chain(groups.iter().map(|&(g, n)| (Some(g), g.label(), n)));
    let mut spans = vec![Span::raw("  ")];
    for (chip_scope, label, n) in chips {
        let style = if chip_scope == scope {
            Style::default()
                .fg(C_ACCENT2)
                .bg(C_ROW_BG)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(C_DIM)
        };
        spans.push(Span::styled(format!(" {} {} ", label, n), style));
        spans.push(Span::raw("  "));
    }
    spans.push(Span::styled("Tab: next", Style::default().fg(C_INACTIVE)));
    Line::from(spans)
}

/// Rows of a grouped result list: a heading before each group, then its results.
/// `None` is a heading; `Some(i)` is result `i`.
fn grouped_rows<T>(
    results: &[T],
    group: impl Fn(&T) -> crate::sources::ResultGroup,
) -> Vec<Option<usize>> {
    let mut rows = Vec::with_capacity(results.len() + 5);
    let mut last = None;
    for (i, item) in results.iter().enumerate() {
        let g = group(item);
        if last != Some(g) {
            rows.push(None);
            last = Some(g);
        }
        rows.push(Some(i));
    }
    rows
}

/// Draw `rows` (from `grouped_rows`) around the cursor, recording each result row for
/// mouse clicks. `heading` names the group of the result after a heading row.
fn draw_grouped_list(
    f: &mut Frame,
    app: &mut App,
    area: Rect,
    rows: &[Option<usize>],
    cursor: usize,
    heading: impl Fn(usize) -> String,
    mut line_for: impl FnMut(usize, &App) -> Line<'static>,
) {
    let visible = area.height as usize;
    let cursor_row = rows.iter().position(|&r| r == Some(cursor)).unwrap_or(0);
    let (start, end) = scroll_offset(cursor_row, rows.len(), visible);
    let mut lines = Vec::new();
    let mut hits = Vec::new();
    for (offset, row) in rows[start..end].iter().enumerate() {
        match *row {
            Some(i) => {
                hits.push((area.y + offset as u16, i));
                lines.push(line_for(i, app));
            }
            None => {
                // The heading names the group of the next result
                let next = rows[start + offset + 1..].iter().flatten().next().copied();
                lines.push(Line::from(Span::styled(
                    format!("  {}", next.map(&heading).unwrap_or_default()),
                    Style::default().fg(C_INACTIVE).add_modifier(Modifier::BOLD),
                )));
            }
        }
    }
    app.ui_bounds.rows = hits;
    f.render_widget(Paragraph::new(lines), area);
    if rows.len() > visible {
        app.ui_bounds.scrollbar_rect = Some(Rect::new(
            area.x + area.width.saturating_sub(1),
            area.y,
            1,
            area.height,
        ));
        render_scrollbar(f, area, rows.len(), visible, start);
    }
}

// ─── Search View ───────────────────────────────────────────────────────────
fn draw_search(f: &mut Frame, app: &mut App, area: Rect) {
    use crate::sources::{result_groups, ResultGroup};
    if area.width < 5 || area.height < 3 {
        return;
    }

    let input_line = Line::from(vec![
        Span::styled("  Search library: ", Style::default().fg(C_ACCENT)),
        Span::styled(app.search_query.clone(), Style::default().fg(C_FG)),
        Span::styled(
            if app.searching { "█" } else { "" },
            Style::default().fg(C_ACCENT2),
        ),
    ]);
    f.render_widget(
        Paragraph::new(input_line),
        Rect::new(area.x, area.y, area.width, 1),
    );

    let chips_area = Rect::new(area.x, area.y + 1, area.width, 1);
    let list_area = Rect::new(
        area.x,
        area.y + 2,
        area.width,
        area.height.saturating_sub(2),
    );
    if app.search_query.is_empty() {
        return;
    }
    if app.search_all.is_empty() {
        f.render_widget(
            Paragraph::new(Span::styled("  No results", Style::default().fg(C_RED))),
            chips_area,
        );
        return;
    }
    let groups = result_groups(&app.search_all, ResultGroup::of_library);
    f.render_widget(
        Paragraph::new(scope_chips(&groups, app.search_scope)),
        chips_area,
    );

    let rows = grouped_rows(&app.search_results, ResultGroup::of_library);
    let width = list_area.width.saturating_sub(1) as usize;
    let group_of: Vec<ResultGroup> = app
        .search_results
        .iter()
        .map(ResultGroup::of_library)
        .collect();
    let cursor = app.search_cursor;
    draw_grouped_list(
        f,
        app,
        list_area,
        &rows,
        cursor,
        |i| {
            let g = group_of[i];
            let n = groups
                .iter()
                .find(|&&(gg, _)| gg == g)
                .map_or(0, |&(_, n)| n);
            format!("{}  {}", g.label(), n)
        },
        |i, app| {
            let entry = &app.search_results[i];
            let style = if i == app.search_cursor {
                Style::default().fg(C_CYAN).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(C_FG)
            };
            let (icon, info) = match entry {
                LibraryEntry::Directory { .. } => {
                    let (tracks, secs) = entry.totals();
                    (
                        "📁 ",
                        format!(
                            "{} {}  ·  {}",
                            tracks,
                            if tracks == 1 { "track" } else { "tracks" },
                            format_total(secs)
                        ),
                    )
                }
                LibraryEntry::Track { metadata, .. } => (
                    "♪  ",
                    metadata
                        .duration
                        .map(|d| format_secs(d.as_secs()))
                        .unwrap_or_default(),
                ),
            };
            let queued = match entry {
                LibraryEntry::Track { path, .. } => app
                    .playlist
                    .entry_ids
                    .contains(&crate::data::track::TrackRef::Local(path.clone())),
                _ => false,
            };
            Line::from(vec![
                Span::styled(
                    if queued { "✓ " } else { "  " },
                    Style::default().fg(C_GREEN),
                ),
                Span::styled(icon, style),
                Span::styled(
                    fit(entry.name(), width.saturating_sub(5 + 22).max(4)),
                    style,
                ),
                Span::styled(fit_right(&info, 22), Style::default().fg(C_INACTIVE)),
            ])
        },
    );
}

// ─── Remote Search View (Spotify / YouTube) ─────────────────────────────────
fn draw_remote_search(f: &mut Frame, app: &mut App, area: Rect) {
    use crate::sources::{result_groups, ResultGroup};
    if area.width < 5 || area.height < 2 {
        return;
    }

    let source_name = match app.source {
        crate::app::SourceTab::Spotify => "Spotify",
        crate::app::SourceTab::YouTube => "YouTube Music",
        _ => "Remote",
    };

    let view = match app.active_source_view() {
        Some(v) => v.clone(),
        None => return,
    };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .split(area);

    let prompt = format!("  Search {}: ", source_name);
    render_input_line(
        f,
        chunks[0],
        &prompt,
        &view.search_query,
        app.searching,
        ratatui::layout::Alignment::Left,
    );

    let groups = result_groups(&view.search_all, ResultGroup::of_browse);
    if view.loading {
        f.render_widget(
            Paragraph::new(Span::styled(
                format!("  Searching {}...", source_name),
                Style::default().fg(C_DIM),
            )),
            chunks[1],
        );
    } else if !view.search_all.is_empty() {
        f.render_widget(
            Paragraph::new(scope_chips(&groups, view.search_scope)),
            chunks[1],
        );
    }

    let list_area = chunks[2];
    if list_area.height < 1 {
        return;
    }

    if view.search_results.is_empty() {
        if !view.loading && !view.search_query.is_empty() {
            let empty_hint = Paragraph::new(Line::from(Span::styled(
                "  No results found.",
                Style::default().fg(C_DIM),
            )))
            .alignment(ratatui::layout::Alignment::Left);
            f.render_widget(empty_hint, list_area);
        }
        return;
    }

    let rows = grouped_rows(&view.search_results, ResultGroup::of_browse);
    let width = list_area.width as usize;
    draw_grouped_list(
        f,
        app,
        list_area,
        &rows,
        view.search_cursor,
        |i| {
            let g = ResultGroup::of_browse(&view.search_results[i]);
            let n = groups
                .iter()
                .find(|&&(gg, _)| gg == g)
                .map_or(0, |&(_, n)| n);
            format!("{}  {}", g.label(), n)
        },
        |i, app| {
            let item = &view.search_results[i];
            let line = browse_row(
                item,
                i == view.search_cursor,
                &view.expanded,
                &app.playlist,
                width,
            );
            // Owned, so the line outlives this closure's borrow of the view
            Line::from(
                line.spans
                    .into_iter()
                    .map(|span| Span::styled(span.content.into_owned(), span.style))
                    .collect::<Vec<_>>(),
            )
        },
    );
}

/// Every key, by section, as listed in the help view.
const HELP: &[(&str, &[(&str, &str)])] = &[
    (
        "Navigation & Views",
        &[
            ("F2 - F6", "Switch views (Queue/Library/Now Playing/Search/Help)"),
            ("Tab / B-Tab", "Cycle active panel / view"),
            ("Tab (Search)", "Show the next group of results: Songs, Albums, ..."),
            ("k j ↑ ↓", "Scroll / Navigate list items"),
            ("q / Ctrl+C", "Quit application"),
            ("Esc", "Back to the queue view"),
            ("/ (Help)", "Filter this list of keys"),
        ],
    ),
    (
        "Music Sources (Local / Spotify / YouTube)",
        &[
            ("Ctrl+1 / Alt+1", "Switch to Local Library source"),
            ("Ctrl+2 / Alt+2", "Switch to Spotify source (requires Premium)"),
            ("c", "Spotify: own Client ID · YouTube: paste a cookie"),
            ("Ctrl+3 / Alt+3", "Switch to YouTube Music source (stream via yt-dlp)"),
            ("Ctrl+4 / Alt+4", "Switch to Unified Queue (mix tracks from any source)"),
            ("Credentials", "Config stored in ~/.config/mixed/credentials.json"),
            ("Lyrics", "Shown for all sources, found online"),
        ],
    ),
    (
        "Playback Controls",
        &[
            ("Space / p", "Play / Pause (restarts after a stop)"),
            ("S", "Stop playback"),
            ("n / l / →", "Next track (stays on the last one)"),
            ("h / ←", "Previous track / restart after 3 s"),
            ("a / d", "Seek backward / forward 5 seconds"),
            ("+ / =", "Volume up"),
            ("- / [", "Volume down"),
            ("s", "Toggle shuffle mode"),
            ("r", "Cycle repeat mode (off → track → queue)"),
            ("R", "Radio of the selected or playing track, queued next"),
        ],
    ),
    (
        "Queue & Library Management",
        &[
            ("Enter", "Enqueue selected item / Play selected queue item"),
            ("Alt + Enter", "Enqueue selected item and play immediately"),
            ("Shift+Enter", "Enqueue selected track to play next"),
            ("o ← →", "Toggle / Collapse/Expand directory tree in Library"),
            ("Delete", "Remove selected track from the queue"),
            ("Backspace", "Folder up (Library) / Clear queue"),
            ("f / g", "Move selected queue item up / down"),
            ("/", "Open search prompt to filter library"),
        ],
    ),
    (
        "Display & Lyrics",
        &[
            ("v", "Toggle spectrum/braille visualizer mode"),
            ("m", "Toggle full lyrics / 3-line timed lyrics view"),
            ("y", "Choose other lyrics (Now Playing)"),
            ("Enter / Esc", "Lyrics list: use / close (/ to search)"),
            ("b / B", "Search playing artist on Google in browser"),
        ],
    ),
    (
        "Mouse Interaction",
        &[
            ("Left Click", "Select item / Play (double-click) / Seek progress / Mini-controls / Search artist & song"),
            ("Scroll Wheel", "Scroll Library, Queue, Search, or Lyrics smoothly"),
        ],
    ),
];

// ─── Help View ─────────────────────────────────────────────────────────────
fn draw_help(f: &mut Frame, app: &App, area: Rect) {
    let filter = app.help_filter.to_lowercase();
    let mut help_text = vec![
        Line::from(vec![
            Span::styled(
                "  mixed ",
                Style::default().fg(C_ACCENT2).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("v{}", env!("CARGO_PKG_VERSION")),
                Style::default().fg(C_GREEN).add_modifier(Modifier::BOLD),
            ),
            Span::styled("  —  Keyboard Shortcuts & Help", Style::default().fg(C_DIM)),
        ]),
        // The filter: typed after "/", kept after Enter
        if app.help_filtering || !filter.is_empty() {
            Line::from(vec![
                Span::styled("  Filter keys: ", Style::default().fg(C_ACCENT)),
                Span::styled(app.help_filter.clone(), Style::default().fg(C_FG)),
                Span::styled(
                    if app.help_filtering { "█" } else { "" },
                    Style::default().fg(C_ACCENT2),
                ),
            ])
        } else {
            Line::from(Span::styled(
                "  Press / to filter the keys",
                Style::default().fg(C_INACTIVE),
            ))
        },
    ];

    let mut shown = 0;
    for (section, keys) in HELP {
        let matching: Vec<_> = keys
            .iter()
            .filter(|(key, what)| {
                filter.is_empty()
                    || key.to_lowercase().contains(&filter)
                    || what.to_lowercase().contains(&filter)
                    || section.to_lowercase().contains(&filter)
            })
            .collect();
        if matching.is_empty() {
            continue;
        }
        shown += matching.len();
        help_text.push(Line::from(""));
        help_text.push(Line::from(Span::styled(
            format!("  ── {} ──", section),
            Style::default().fg(C_CYAN).add_modifier(Modifier::BOLD),
        )));
        for (key, what) in matching {
            help_text.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(fit(key, 14), Style::default().fg(C_CYAN)),
                Span::styled(" •  ", Style::default().fg(C_DIM)),
                Span::styled(*what, Style::default().fg(C_FG)),
            ]));
        }
    }
    if shown == 0 {
        help_text.push(Line::from(""));
        help_text.push(Line::from(Span::styled(
            "  No key matches the filter.",
            Style::default().fg(C_DIM),
        )));
    }

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

    let text_height = 6.min(
        inner_area
            .height
            .saturating_sub(content_y.saturating_sub(inner_area.y)),
    );
    let text_area = Rect::new(inner_area.x, content_y, inner_area.width, text_height);
    let mut lines = vec![
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
    if let Some(ref msg) = app.status_msg {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            msg.as_str(),
            Style::default().fg(C_RED),
        )));
    }
    let widget = Paragraph::new(lines).alignment(ratatui::layout::Alignment::Center);
    f.render_widget(widget, text_area);
}

// ─── Footer Tab Bar ────────────────────────────────────────────────────────
fn draw_footer(f: &mut Frame, app: &mut App, area: Rect) {
    if area.height == 0 || area.width == 0 {
        return;
    }

    if area.height >= 2 {
        let footer_layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // Source bar or status message
                Constraint::Length(1), // F-key tabs bar
            ])
            .split(area);

        let source_area = footer_layout[0];
        let tabs_area = footer_layout[1];
        app.ui_bounds.footer_source_rect = Some(source_area);
        app.ui_bounds.footer_tabs_rect = Some(tabs_area);

        // Row 0: Status message if present, otherwise Source switcher tabs
        if let Some(ref msg) = app.status_msg {
            let colour = if msg.starts_with("Error")
                || msg.starts_with("Failed")
                || msg.starts_with("Invalid")
            {
                C_RED
            } else {
                C_ACCENT
            };
            let line = Line::from(vec![
                Span::styled("  ● ", Style::default().fg(colour)),
                Span::styled(msg.as_str(), Style::default().fg(C_FG)),
            ]);
            let widget = Paragraph::new(line).alignment(ratatui::layout::Alignment::Left);
            f.render_widget(widget, source_area);
        } else {
            let mut spans = Vec::new();
            for (i, tab) in crate::app::SourceTab::ALL.iter().enumerate() {
                if i > 0 {
                    spans.push(Span::styled("  ·  ", Style::default().fg(C_DIM)));
                }
                let style = if *tab == app.source {
                    Style::default()
                        .fg(accent_for(*tab))
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(C_DIM)
                };
                spans.push(Span::styled(tab.label(), style));
            }
            let line = Line::from(spans);
            let widget = Paragraph::new(line).alignment(ratatui::layout::Alignment::Center);
            f.render_widget(widget, source_area);
        }

        // Row 1: F-key tabs bar
        let tabs = crate::app::tabs_for(app.source);
        let mut spans = Vec::new();
        for (i, (key, label, panel)) in tabs.iter().enumerate() {
            if i > 0 {
                spans.push(Span::styled(" | ", Style::default().fg(C_DIM)));
            }
            let style = if *panel == app.active_panel {
                Style::default()
                    .fg(accent_for(app.source))
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(C_DIM)
            };
            spans.push(Span::styled(format!("{} {}", key, label), style));
        }

        // Play/Pause icon
        let status_icon = if app.player.as_ref().map(|p| p.is_paused()).unwrap_or(false) {
            "⏸"
        } else {
            "▶"
        };
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            status_icon,
            Style::default().fg(accent_for(app.source)),
        ));

        // Shuffle icon
        if app.playlist.shuffle {
            spans.push(Span::raw(" "));
            spans.push(Span::styled("⤨", Style::default().fg(C_ACCENT2)));
        }

        let line = Line::from(spans);
        let widget = Paragraph::new(line).alignment(ratatui::layout::Alignment::Center);
        f.render_widget(widget, tabs_area);
    } else {
        // Fallback for single-row footer (extremely constrained terminals)
        app.ui_bounds.footer_tabs_rect = Some(area);
        if let Some(ref msg) = app.status_msg {
            let colour = if msg.starts_with("Error")
                || msg.starts_with("Failed")
                || msg.starts_with("Invalid")
            {
                C_RED
            } else {
                C_ACCENT
            };
            let line = Line::from(vec![
                Span::styled("  ● ", Style::default().fg(colour)),
                Span::styled(msg.as_str(), Style::default().fg(C_FG)),
            ]);
            let widget = Paragraph::new(line).alignment(ratatui::layout::Alignment::Left);
            f.render_widget(widget, area);
            return;
        }

        let tabs = crate::app::tabs_for(app.source);
        let mut spans = Vec::new();
        for (i, (key, label, panel)) in tabs.iter().enumerate() {
            if i > 0 {
                spans.push(Span::styled(" | ", Style::default().fg(C_DIM)));
            }
            let style = if *panel == app.active_panel {
                Style::default()
                    .fg(accent_for(app.source))
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(C_DIM)
            };
            spans.push(Span::styled(format!("{} {}", key, label), style));
        }
        let line = Line::from(spans);
        let widget = Paragraph::new(line).alignment(ratatui::layout::Alignment::Center);
        f.render_widget(widget, area);
    }
}

// ─── Helpers ───────────────────────────────────────────────────────────────
fn format_time(ms: u64) -> String {
    let total_secs = ms / 1000;
    let mins = total_secs / 60;
    let secs = total_secs % 60;
    format!("{}:{:02}", mins, secs)
}
