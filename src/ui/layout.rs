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

pub use crate::ui::theme::*;
pub use crate::ui::widgets::*;

/// Main render function — dispatches to the appropriate view inside a global two-pane layout.
fn render_instructions(
    f: &mut Frame,
    area: Rect,
    panel: ActivePanel,
    source: crate::sources::SourceTab,
) {
    let text = match panel {
        ActivePanel::Queue => match source {
            crate::sources::SourceTab::Unified => {
                "Unified Queue  •  Sources: Ctrl+1..4  •  Move: ↑/↓/k/j  •  Play: Enter  •  Del: Remove"
            }
            _ => "Sources: Ctrl+1..4 (or Alt+1..4)  •  Move: ↑/↓/k/j  •  Play: Enter  •  Del: Remove",
        },
        ActivePanel::Library => match source {
            crate::sources::SourceTab::Local => {
                "Local Library  •  Navigate: ↑/↓/k/j  •  Expand/Enqueue: Enter  •  Search: /"
            }
            crate::sources::SourceTab::Spotify => {
                "Spotify Library  •  Navigate: ↑/↓/k/j  •  Open/Close/Queue: Enter  •  Search: /"
            }
            crate::sources::SourceTab::YouTube => {
                "YouTube Music  •  Navigate: ↑/↓/k/j  •  Open/Close/Queue: Enter  •  Search: /"
            }
            crate::sources::SourceTab::Unified => {
                "Unified Library  •  Navigate: ↑/↓/k/j  •  Search: /"
            }
        },
        ActivePanel::Search => {
            "Search Active Source  •  Type query  •  Select: ↑/↓  •  Enqueue: Enter"
        }
        ActivePanel::NowPlaying => {
            "Now Playing  •  Lyrics: m  •  Visualizer: v  •  Sources: Ctrl+1..4"
        }
        ActivePanel::Help => "Switch Views: F2-F6 / Tab  •  Sources: Ctrl+1..4  •  Quit: q",
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
            // Reserve 2 vertical lines for spacer (1) + mini-controls (1)
            let max_art_h = art_area.height.saturating_sub(4);
            let art_h = h_if_full_w.min(max_art_h).max(4);
            let art_w = (((art_h as f32) * aspect_ratio) as u16).min(max_w).max(4);

            // Combined block height = Album Art (art_h) + Spacer (1) + Mini-controls (1)
            let total_block_h = art_h + 2;
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
            render_instructions(f, inst_rect, app.active_panel, app.source);
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
                ActivePanel::Help => draw_help(f, tab_area),
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
            Constraint::Length(3), // 2. Metadata
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
        render_instructions(f, layout[1], ActivePanel::NowPlaying, app.source);
        draw_metadata(f, app, layout[2]);
        draw_lyrics(f, app, layout[4]);
        draw_progress(f, app, layout[6]);
        return;
    }

    // Normal mode: Logo + Instructions + Metadata + Spacer + Lyrics + Spacer + Visualizer + Spacer + Progress
    let constraints = vec![
        Constraint::Length(6), // 0. Top Logo
        Constraint::Length(1), // 1. Shortcut instructions
        Constraint::Length(3), // 2. Metadata (centered)
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
    render_instructions(f, layout[1], ActivePanel::NowPlaying, app.source);
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

        let lines = vec![
            Line::from(Span::styled(
                title_str,
                Style::default().fg(C_FG).add_modifier(Modifier::BOLD),
            )),
            Line::from(Span::styled(artist_str, Style::default().fg(C_ACCENT))),
            Line::from(Span::styled(
                meta.display_album(),
                Style::default().fg(C_DIM),
            )),
        ];
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
                    let msg = if matches!(
                        entry.id,
                        crate::data::track::TrackRef::Spotify(_)
                            | crate::data::track::TrackRef::YouTube(_)
                    ) {
                        "Lyrics not available (local files only). Press 'm' to go back"
                    } else {
                        "No Lyrics Available. Press 'm' to go back"
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

    // Queue summary header (Q2)
    let mut list_area = area;
    if area.height >= 2 {
        let total_secs = app.playlist.total_duration_secs();
        let hours = total_secs / 3600;
        let mins = (total_secs % 3600) / 60;
        let duration_str = if hours > 0 {
            format!("{}h {}m", hours, mins)
        } else {
            format!("{} min", mins)
        };
        let n = app.playlist.len();
        let summary = format!(
            "  {} {}  ·  {}",
            n,
            if n == 1 { "track" } else { "tracks" },
            duration_str
        );
        let summary_area = Rect::new(area.x, area.y, area.width, 1);
        let header_line = Line::from(Span::styled(summary, Style::default().fg(C_DIM)));
        f.render_widget(Paragraph::new(header_line), summary_area);
        list_area = Rect::new(
            area.x,
            area.y + 1,
            area.width,
            area.height.saturating_sub(1),
        );
    }

    let visual_items = app
        .playlist
        .get_visual_items(app.show_folders, app.config.strip_track_numbers);
    let playing_real = app.playlist.current_real_index();

    let visible_height = list_area.height as usize;
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

    let scroll = highlighted_visual_idx.saturating_sub(half);
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

                let style = row_style(is_cursor, is_playing);

                display_lines.push(Line::from(vec![
                    Span::styled("  ", style),
                    Span::styled(prefix, style),
                    Span::styled(title.as_str(), style),
                ]));
            }
        }
    }

    let widget = Paragraph::new(display_lines).alignment(ratatui::layout::Alignment::Left);
    f.render_widget(widget, list_area);

    // Render vertical scrollbar if list exceeds visible height
    if visual_items.len() > visible_height {
        let scrollbar_col = list_area.x + list_area.width.saturating_sub(1);
        app.ui_bounds.scrollbar_rect =
            Some(Rect::new(scrollbar_col, list_area.y, 1, list_area.height));

        render_scrollbar(f, list_area, visual_items.len(), visible_height, start);
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

    // Determine enqueued status of the entire library using pre-calculated set
    let all_enqueued = app.flat_library.iter().all(|item| {
        if let LibraryEntry::Track { path, .. } = &item.entry {
            app.playlist
                .entry_ids
                .contains(&crate::data::track::TrackRef::Local(path.clone()))
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

// ─── Search View ───────────────────────────────────────────────────────────
fn draw_search(f: &mut Frame, app: &mut App, area: Rect) {
    if area.width < 5 || area.height < 3 {
        return;
    }

    // Search input line with match counter (Q3) (Centered)
    let input_area = Rect::new(area.x, area.y, area.width, 1);
    let cursor = if app.searching { "█" } else { "" };
    let count_span = if app.search_query.is_empty() {
        Span::raw("")
    } else if app.search_results.is_empty() {
        Span::styled("  no results", Style::default().fg(C_RED))
    } else {
        Span::styled(
            format!("  {} results", app.search_results.len()),
            Style::default().fg(C_DIM),
        )
    };
    let input_line = Line::from(vec![
        Span::styled("Search: ", Style::default().fg(C_ACCENT)),
        Span::styled(&app.search_query, Style::default().fg(C_FG)),
        Span::styled(cursor, Style::default().fg(C_ACCENT2)),
        count_span,
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

    // Render vertical scrollbar if search results exceed visible height
    if app.search_results.len() > results_height {
        let scrollbar_col = results_area.x + results_area.width.saturating_sub(1);
        app.ui_bounds.scrollbar_rect = Some(Rect::new(
            scrollbar_col,
            results_area.y,
            1,
            results_area.height,
        ));

        render_scrollbar(
            f,
            results_area,
            app.search_results.len(),
            results_height,
            start,
        );
    }
}

/// One row of a remote library tree or search result list, drawn like the local
/// library: enqueued marker, indentation by depth, folder/track icon, title, subtitle.
fn browse_row<'a>(
    item: &'a crate::sources::BrowseItem,
    is_cursor: bool,
    expanded: &std::collections::HashSet<String>,
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
        let s = Style::default().fg(C_CYAN);
        if is_cursor {
            s.add_modifier(Modifier::BOLD)
        } else {
            s
        }
    } else {
        row_style(is_cursor, is_playing)
    };
    let icon = if item.is_container {
        if expanded.contains(&item.id) {
            "▼ 📁 "
        } else {
            "▶ 📁 "
        }
    } else if is_playing {
        "▶  "
    } else {
        "♪  "
    };
    let marker = if enqueued { "  * " } else { "    " };
    let indent = "    ".repeat(item.depth);

    // The title has priority; the subtitle only uses what is left of the row.
    // marker (4) + indent + icon (5 cells) + scrollbar column
    let title_room = width.saturating_sub(4 + indent.len() + 5 + 1);
    let title = truncate_to_width(&item.title, title_room);
    let sub_room = title_room
        .saturating_sub(unicode_width::UnicodeWidthStr::width(title) + 3)
        .min(20);
    let sub = if sub_room >= 4 {
        truncate_to_width(item.subtitle.as_deref().unwrap_or(""), sub_room)
    } else {
        ""
    };

    Line::from(vec![
        Span::styled(marker, style),
        Span::styled(indent, style),
        Span::styled(icon, style),
        Span::styled(title, style),
        Span::styled(
            if sub.is_empty() { "" } else { " • " },
            Style::default().fg(C_DIM),
        ),
        Span::styled(sub, Style::default().fg(C_DIM)),
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

    if !view.connected {
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
                Span::styled(
                    "Not connected. Press Enter to sign in.",
                    Style::default().fg(C_RED),
                ),
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
                    "  • Requires personal Spotify Client ID.",
                    Style::default().fg(C_FG),
                )));
                lines.push(Line::from(Span::styled(
                    "  • Press Enter to configure Client ID / authorize via PKCE.",
                    Style::default().fg(C_DIM),
                )));
            }
            crate::app::SourceTab::YouTube => {
                lines.push(Line::from(Span::styled(
                    "  • Requires Cookie header from music.youtube.com.",
                    Style::default().fg(C_FG),
                )));
                lines.push(Line::from(Span::styled(
                    "  • Paste cookie or credentials to connect.",
                    Style::default().fg(C_FG),
                )));
                lines.push(Line::from(Span::styled(
                    "  • Press Enter to paste cookie header.",
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
            Line::from(""),
            Line::from(Span::styled(
                format!("  No items found in {} library.", source_name),
                Style::default().fg(C_DIM),
            )),
            Line::from(Span::styled(
                "  Press F5 to switch to Search.",
                Style::default().fg(C_DIM),
            )),
        ];
        let widget = Paragraph::new(lines).alignment(ratatui::layout::Alignment::Left);
        f.render_widget(widget, area);
        return;
    }

    let visible_height = area.height as usize;
    let (start, end) = scroll_offset(view.cursor, view.flat.len(), visible_height);
    let mut display_lines = Vec::new();

    for (idx, item) in view.flat[start..end].iter().enumerate() {
        display_lines.push(browse_row(
            item,
            start + idx == view.cursor,
            &view.expanded,
            &app.playlist,
            area.width as usize,
        ));
    }

    let widget = Paragraph::new(display_lines).alignment(ratatui::layout::Alignment::Left);
    f.render_widget(widget, area);

    if view.flat.len() > visible_height {
        render_scrollbar(f, area, view.flat.len(), visible_height, start);
    }
}

// ─── Remote Search View (Spotify / YouTube) ─────────────────────────────────
fn draw_remote_search(f: &mut Frame, app: &mut App, area: Rect) {
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

    let count_line = if view.loading {
        format!("  Searching {}...", source_name)
    } else {
        format!("  {} results", view.search_results.len())
    };
    let count_widget = Paragraph::new(Line::from(Span::styled(
        count_line,
        Style::default().fg(C_DIM),
    )))
    .alignment(ratatui::layout::Alignment::Left);
    f.render_widget(count_widget, chunks[1]);

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

    let visible_height = list_area.height as usize;
    let (start, end) = scroll_offset(
        view.search_cursor,
        view.search_results.len(),
        visible_height,
    );
    let mut display_lines = Vec::new();

    for (idx, item) in view.search_results[start..end].iter().enumerate() {
        display_lines.push(browse_row(
            item,
            start + idx == view.search_cursor,
            &view.expanded,
            &app.playlist,
            list_area.width as usize,
        ));
    }

    let widget = Paragraph::new(display_lines).alignment(ratatui::layout::Alignment::Left);
    f.render_widget(widget, list_area);

    if view.search_results.len() > visible_height {
        render_scrollbar(
            f,
            list_area,
            view.search_results.len(),
            visible_height,
            start,
        );
    }
}

// ─── Help View ─────────────────────────────────────────────────────────────
fn draw_help(f: &mut Frame, area: Rect) {
    let help_text = vec![
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
            Span::styled("q / Ctrl+C", Style::default().fg(C_CYAN)),
            Span::styled("   •  ", Style::default().fg(C_DIM)),
            Span::styled("Quit application", Style::default().fg(C_FG)),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("Esc", Style::default().fg(C_CYAN)),
            Span::styled("          •  ", Style::default().fg(C_DIM)),
            Span::styled("Back to the queue view", Style::default().fg(C_FG)),
        ]),
        Line::from(""),
        // Music Sources (Streaming & Local)
        Line::from(Span::styled(
            "  ── Music Sources (Local / Spotify / YouTube) ──",
            Style::default().fg(C_CYAN).add_modifier(Modifier::BOLD),
        )),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("Ctrl+1 / Alt+1", Style::default().fg(C_CYAN)),
            Span::styled(" •  ", Style::default().fg(C_DIM)),
            Span::styled("Switch to Local Library source", Style::default().fg(C_FG)),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("Ctrl+2 / Alt+2", Style::default().fg(C_CYAN)),
            Span::styled(" •  ", Style::default().fg(C_DIM)),
            Span::styled(
                "Switch to Spotify source (requires Premium & Client ID)",
                Style::default().fg(C_FG),
            ),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("Ctrl+3 / Alt+3", Style::default().fg(C_CYAN)),
            Span::styled(" •  ", Style::default().fg(C_DIM)),
            Span::styled(
                "Switch to YouTube Music source (stream via yt-dlp)",
                Style::default().fg(C_FG),
            ),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("Ctrl+4 / Alt+4", Style::default().fg(C_CYAN)),
            Span::styled(" •  ", Style::default().fg(C_DIM)),
            Span::styled(
                "Switch to Unified Queue (mix tracks from any source)",
                Style::default().fg(C_FG),
            ),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("Credentials", Style::default().fg(C_CYAN)),
            Span::styled("    •  ", Style::default().fg(C_DIM)),
            Span::styled(
                "Config stored in ~/.config/mixed/credentials.json",
                Style::default().fg(C_FG),
            ),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("Remote Lyrics", Style::default().fg(C_CYAN)),
            Span::styled("  •  ", Style::default().fg(C_DIM)),
            Span::styled(
                "Lyrics view is local-only; remote tracks display notice",
                Style::default().fg(C_FG),
            ),
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
            Span::styled("Shift+Enter", Style::default().fg(C_CYAN)),
            Span::styled("•  ", Style::default().fg(C_DIM)),
            Span::styled(
                "Enqueue selected track to play next",
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
        Line::from(vec![
            Span::raw("  "),
            Span::styled("b / B", Style::default().fg(C_CYAN)),
            Span::styled("        •  ", Style::default().fg(C_DIM)),
            Span::styled(
                "Search playing artist on Google in browser",
                Style::default().fg(C_FG),
            ),
        ]),
        Line::from(""),
        // Mouse Controls
        Line::from(Span::styled(
            "  ── Mouse Interaction ──",
            Style::default().fg(C_CYAN).add_modifier(Modifier::BOLD),
        )),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("Left Click", Style::default().fg(C_CYAN)),
            Span::styled("   •  ", Style::default().fg(C_DIM)),
            Span::styled(
                "Select item / Play (double-click) / Seek progress / Mini-controls / Search artist & song",
                Style::default().fg(C_FG),
            ),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("Scroll Wheel", Style::default().fg(C_CYAN)),
            Span::styled(" •  ", Style::default().fg(C_DIM)),
            Span::styled(
                "Scroll Library, Queue, Search, or Lyrics smoothly",
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
