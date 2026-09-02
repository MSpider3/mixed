use crossterm::event::{self, KeyCode, KeyEventKind, MouseButton, MouseEventKind};

use crate::app::{ActivePanel, App};

/// Handle a crossterm key event. Returns true if the app should quit.
pub fn handle_key(app: &mut App, key: event::KeyEvent) -> bool {
    // Ignore release events to prevent double processing
    if key.kind == KeyEventKind::Release {
        return false;
    }

    // Loading guard: while the audio engine is initializing on a background thread,
    // only allow safe keys (quit, navigation, search panel). All playback-related
    // keys are silently swallowed — no panic, no queuing.
    if app.player_loading && !app.awaiting_dir_input && !app.searching {
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => return true,
            KeyCode::Tab => app.next_panel(),
            KeyCode::BackTab => app.prev_panel(),
            KeyCode::F(2) => app.active_panel = ActivePanel::Queue,
            KeyCode::F(3) => app.active_panel = ActivePanel::Library,
            KeyCode::F(5) => {
                app.active_panel = ActivePanel::Search;
                app.searching = true;
                app.search_query.clear();
            }
            KeyCode::F(6) => app.active_panel = ActivePanel::Help,
            _ => {} // swallow all other keys silently
        }
        app.refresh_needed = true;
        return false;
    }

    // If the user presses F2-F6 or Tab/BackTab, turn off search immediately and clean search query
    match key.code {
        KeyCode::F(2)
        | KeyCode::F(3)
        | KeyCode::F(4)
        | KeyCode::F(6)
        | KeyCode::Tab
        | KeyCode::BackTab => {
            if app.searching || app.active_panel == ActivePanel::Search {
                app.searching = false;
                app.search_query.clear();
            }
        }
        KeyCode::F(5) => {
            app.active_panel = ActivePanel::Search;
            app.searching = true;
            app.search_query.clear();
        }
        _ => {}
    }

    // If we're in search mode, handle text input
    if app.searching {
        return handle_search_input(app, key);
    }

    // If in directory input mode (first-run config)
    if app.awaiting_dir_input {
        return handle_dir_input(app, key);
    }

    match key.code {
        // Global quit
        KeyCode::Char('q') | KeyCode::Esc => return true,

        // View switching
        KeyCode::F(2) => app.active_panel = ActivePanel::Queue,
        KeyCode::F(3) => app.active_panel = ActivePanel::Library,
        KeyCode::F(4) => app.active_panel = ActivePanel::NowPlaying,
        KeyCode::F(5) => {
            app.active_panel = ActivePanel::Search;
            app.searching = true;
            app.search_query.clear();
        }
        KeyCode::F(6) => app.active_panel = ActivePanel::Help,

        // Tab to cycle views
        KeyCode::Tab => app.next_panel(),
        KeyCode::BackTab => app.prev_panel(),

        // Playback controls
        KeyCode::Char(' ') | KeyCode::Char('p') => app.toggle_pause(),
        KeyCode::Char('l') | KeyCode::Right => {
            if app.active_panel == ActivePanel::Library && app.library_cursor > 0 {
                let idx = app.library_cursor - 1;
                if idx < app.flat_library.len() {
                    let entry = &app.flat_library[idx].entry;
                    if entry.is_dir() {
                        app.expand_dir(entry.path().to_path_buf());
                        app.refresh_needed = true;
                        return false;
                    }
                }
            }
            app.next_track();
        }
        KeyCode::Char('n') => app.next_track(),
        KeyCode::Char('h') | KeyCode::Left => {
            if app.active_panel == ActivePanel::Library && app.library_cursor > 0 {
                let idx = app.library_cursor - 1;
                if idx < app.flat_library.len() {
                    let entry = &app.flat_library[idx].entry;
                    if entry.is_dir() {
                        app.collapse_dir(entry.path().to_path_buf());
                        app.refresh_needed = true;
                        return false;
                    }
                }
            }
            app.prev_track();
        }
        KeyCode::Char('S') => {
            app.stop();
        }
        KeyCode::Char('a') => {
            let starting_position = app
                .pending_seek
                .map(|d| d.as_millis() as u64)
                .unwrap_or_else(|| app.player().map(|p| p.elapsed_ms()).unwrap_or(0));
            let target = starting_position.saturating_sub(5000);
            app.pending_seek = Some(std::time::Duration::from_millis(target));
            app.last_seek_input = Some(std::time::Instant::now());
            app.refresh_needed = true;
        }
        KeyCode::Char('d') => {
            let starting_position = app
                .pending_seek
                .map(|d| d.as_millis() as u64)
                .unwrap_or_else(|| app.player().map(|p| p.elapsed_ms()).unwrap_or(0));
            let target =
                (starting_position + 5000).min(app.player().map(|p| p.duration_ms()).unwrap_or(0));
            app.pending_seek = Some(std::time::Duration::from_millis(target));
            app.last_seek_input = Some(std::time::Instant::now());
            app.refresh_needed = true;
        }
        KeyCode::Char('+') | KeyCode::Char('=') => app.volume_up(),
        KeyCode::Char('-') | KeyCode::Char('[') => app.volume_down(),
        KeyCode::Char('s') => app.toggle_shuffle(),
        KeyCode::Char('r') => {
            app.cycle_repeat();
        }
        KeyCode::Char('v') => app.toggle_visualizer(),
        KeyCode::Char('b') | KeyCode::Char('B') => app.search_artist_web(),
        KeyCode::Char('m') => {
            if app.active_panel == ActivePanel::NowPlaying {
                app.show_full_lyrics = !app.show_full_lyrics;
                app.lyrics_scroll = 0;
            }
        }

        // Navigation
        KeyCode::Up | KeyCode::Char('k') => scroll_up(app),
        KeyCode::Down | KeyCode::Char('j') => scroll_down(app),
        KeyCode::Enter => {
            if key.modifiers.contains(event::KeyModifiers::ALT) {
                handle_enqueue_and_play(app);
            } else {
                handle_enter(app);
            }
        }
        KeyCode::Char('G') => {
            handle_enter(app);
        }
        KeyCode::Char('o') | KeyCode::Char('O') => {
            if app.active_panel == ActivePanel::Library && app.library_cursor > 0 {
                let idx = app.library_cursor - 1;
                if idx < app.flat_library.len() {
                    let entry = &app.flat_library[idx].entry;
                    if entry.is_dir() {
                        let path = entry.path().to_path_buf();
                        if app.collapsed_dirs.contains(&path) {
                            app.expand_dir(path);
                        } else {
                            app.collapse_dir(path);
                        }
                        app.refresh_needed = true;
                        return false;
                    }
                }
            }
        }
        KeyCode::Delete => handle_delete(app),
        KeyCode::Char('f') => {
            if app.active_panel == ActivePanel::Queue {
                let idx = app.queue_cursor;
                app.playlist.move_up(idx);
                if app.queue_cursor > 0 {
                    app.queue_cursor -= 1;
                }
                app.refresh_needed = true;
            }
        }
        KeyCode::Char('g') => {
            if app.active_panel == ActivePanel::Queue {
                let idx = app.queue_cursor;
                app.playlist.move_down(idx);
                if app.queue_cursor + 1 < app.playlist.len() {
                    app.queue_cursor += 1;
                }
                app.refresh_needed = true;
            }
        }
        KeyCode::Backspace => {
            app.clear_playlist();
        }

        // Search shortcut
        KeyCode::Char('/') => {
            app.active_panel = ActivePanel::Search;
            app.searching = true;
            app.search_query.clear();
        }

        _ => {}
    }
    false
}

/// Handle text input in search mode. Returns true if app should quit.
fn handle_search_input(app: &mut App, key: event::KeyEvent) -> bool {
    match key.code {
        KeyCode::Esc => {
            app.searching = false;
        }
        KeyCode::Enter => {
            app.searching = false;
            if key.modifiers.contains(event::KeyModifiers::ALT) {
                handle_enqueue_and_play(app);
            } else {
                handle_enter(app);
            }
        }
        KeyCode::Backspace => {
            app.search_query.pop();
            app.run_search();
        }
        KeyCode::Up => {
            if app.search_cursor > 0 {
                app.search_cursor -= 1;
            }
        }
        KeyCode::Down => {
            if !app.search_results.is_empty() && app.search_cursor + 1 < app.search_results.len() {
                app.search_cursor += 1;
            }
        }
        KeyCode::Char(c) => {
            app.search_query.push(c);
            app.run_search();
        }
        _ => {}
    }
    false
}

/// Handle directory input during first-run setup.
fn handle_dir_input(app: &mut App, key: event::KeyEvent) -> bool {
    match key.code {
        KeyCode::Esc => return true,
        KeyCode::Enter => {
            app.finalize_dir_input();
        }
        KeyCode::Backspace => {
            app.dir_input.pop();
        }
        KeyCode::Char(c) => {
            app.dir_input.push(c);
        }
        _ => {}
    }
    false
}

fn scroll_up(app: &mut App) {
    match app.active_panel {
        ActivePanel::Queue => {
            if app.queue_cursor > 0 {
                app.queue_cursor -= 1;
            }
        }
        ActivePanel::Library => {
            if app.library_cursor > 0 {
                app.library_cursor -= 1;
            }
        }
        ActivePanel::Search => {
            if app.search_cursor > 0 {
                app.search_cursor -= 1;
            }
        }
        ActivePanel::NowPlaying if app.show_full_lyrics && app.lyrics_scroll > 0 => {
            app.lyrics_scroll -= 1;
        }
        _ => {}
    }
}

fn scroll_down(app: &mut App) {
    match app.active_panel {
        ActivePanel::Queue => {
            let max = app.playlist.len().saturating_sub(1);
            if app.queue_cursor < max {
                app.queue_cursor += 1;
            }
        }
        ActivePanel::Library => {
            let max = app.flat_library.len();
            if app.library_cursor < max {
                app.library_cursor += 1;
            }
        }
        ActivePanel::Search => {
            if !app.search_results.is_empty() && app.search_cursor + 1 < app.search_results.len() {
                app.search_cursor += 1;
            }
        }
        ActivePanel::NowPlaying if app.show_full_lyrics => {
            app.lyrics_scroll += 1;
        }
        _ => {}
    }
}

fn handle_enter(app: &mut App) {
    match app.active_panel {
        ActivePanel::Queue => {
            if app.queue_cursor < app.playlist.len() {
                if let Some(pos) = app
                    .playlist
                    .play_order
                    .iter()
                    .position(|&o| o == app.queue_cursor)
                {
                    app.playlist.current = pos;
                } else {
                    app.playlist.current = app.queue_cursor;
                }
                app.play_current();
            }
        }
        ActivePanel::Library => {
            app.library_enqueue_selected(false);
        }
        ActivePanel::Search => {
            app.search_enqueue_selected(false);
        }
        _ => {}
    }
}

fn handle_enqueue_and_play(app: &mut App) {
    match app.active_panel {
        ActivePanel::Library => {
            app.library_enqueue_selected(true);
        }
        ActivePanel::Search => {
            app.search_enqueue_selected(true);
        }
        _ => {}
    }
}

fn handle_delete(app: &mut App) {
    if app.active_panel == ActivePanel::Queue && app.queue_cursor < app.playlist.len() {
        let is_current = Some(app.queue_cursor) == app.playlist.current_real_index();
        app.playlist.remove(app.queue_cursor);
        if app.playlist.is_empty() {
            app.clear_playlist();
            return;
        }
        if app.queue_cursor >= app.playlist.len() && app.queue_cursor > 0 {
            app.queue_cursor -= 1;
        }
        if is_current {
            app.play_current();
        }
        app.rebuild_flat_library_view();
    }
}

/// Handle mouse events.
pub fn handle_mouse(app: &mut App, mouse: event::MouseEvent) {
    let x = mouse.column;
    let y = mouse.row;

    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) | MouseEventKind::Drag(MouseButton::Left) => {
            // 1. Scrollbar click & drag (Queue / Library / Search)
            if let Some(sb_rect) = app.ui_bounds.scrollbar_rect {
                if x >= sb_rect.x
                    && x < sb_rect.x + sb_rect.width
                    && y >= sb_rect.y
                    && y < sb_rect.y + sb_rect.height
                {
                    let rel_y = (y - sb_rect.y) as f64;
                    let ratio = (rel_y / (sb_rect.height.max(1) as f64)).clamp(0.0, 1.0);
                    match app.active_panel {
                        ActivePanel::Queue => {
                            let total = app.playlist.len();
                            if total > 0 {
                                let target = (ratio * total as f64).round() as usize;
                                app.queue_cursor = target.min(total.saturating_sub(1));
                            }
                        }
                        ActivePanel::Library => {
                            let total = app.flat_library.len() + 1;
                            if total > 0 {
                                let target = (ratio * total as f64).round() as usize;
                                app.library_cursor = target.min(total.saturating_sub(1));
                            }
                        }
                        ActivePanel::Search => {
                            let total = app.search_results.len();
                            if total > 0 {
                                let target = (ratio * total as f64).round() as usize;
                                app.search_cursor = target.min(total.saturating_sub(1));
                            }
                        }
                        _ => {}
                    }
                    app.refresh_needed = true;
                    return;
                }
            }

            // 2. Progress bar click & drag (direct seek)
            if let Some(bar_rect) = app.ui_bounds.progress_bar_rect {
                if y == bar_rect.y && x >= bar_rect.x && x < bar_rect.x + bar_rect.width {
                    let denom = bar_rect.width.saturating_sub(1).max(1);
                    let ratio = ((x.saturating_sub(bar_rect.x) as f64) / (denom as f64)).min(1.0);
                    app.seek_to_ratio(ratio);
                    app.refresh_needed = true;
                    return;
                }
            }

            // For discrete click actions (tabs, mini-controls, web search, item play):
            // Only process on Down, not on Drag
            if let MouseEventKind::Drag(_) = mouse.kind {
                return;
            }

            // 3. Footer tab click
            if let Some(footer_rect) = app.ui_bounds.footer_tabs_rect {
                if y == footer_rect.y && x >= footer_rect.x && x < footer_rect.x + footer_rect.width
                {
                    let rel_x = x - footer_rect.x;
                    let tab_width = footer_rect.width / 5;
                    let selected_tab = if tab_width > 0 {
                        (rel_x / tab_width).min(4)
                    } else {
                        0
                    };

                    let prev_panel = app.active_panel;
                    match selected_tab {
                        0 => app.active_panel = ActivePanel::Queue,
                        1 => app.active_panel = ActivePanel::Library,
                        2 => app.active_panel = ActivePanel::NowPlaying,
                        3 => {
                            app.active_panel = ActivePanel::Search;
                            app.searching = true;
                        }
                        _ => app.active_panel = ActivePanel::Help,
                    }

                    if prev_panel == ActivePanel::Search && app.active_panel != ActivePanel::Search
                    {
                        app.searching = false;
                        app.search_query.clear();
                    }
                    app.refresh_needed = true;
                    return;
                }
            }

            // 4. Mini-controls click (rendered in left pane directly below album art in Queue/Library/Search/Help)
            if let Some(mini_rect) = app.ui_bounds.mini_controls_rect {
                if y == mini_rect.y && x >= mini_rect.x && x < mini_rect.x + mini_rect.width {
                    // Total width of "⏮   ▶   ⏭   +   -   ∅" is 21 cells
                    let total_w: u16 = 21;
                    let indent = mini_rect.x + (mini_rect.width.saturating_sub(total_w)) / 2;

                    // Button layout with 4-cell centers:
                    // col 0: ⏮  (cols 0..1)
                    // col 4: ▶/⏸ (cols 2..5)
                    // col 8: ⏭  (cols 6..9)
                    // col 12: +  (cols 10..13)
                    // col 16: -  (cols 14..17)
                    // col 20: ∅  (cols 18+)
                    if x < indent + 2 {
                        app.prev_track();
                    } else if x < indent + 6 {
                        app.toggle_pause();
                    } else if x < indent + 10 {
                        app.next_track();
                    } else if x < indent + 14 {
                        app.volume_up();
                    } else if x < indent + 18 {
                        app.volume_down();
                    } else {
                        app.clear_playlist();
                    }
                    app.refresh_needed = true;
                    return;
                }
            }

            // 5. Song Title & Artist click -> Google search ONLY in Track tab (NowPlaying)
            if app.active_panel == ActivePanel::NowPlaying {
                if let Some(title_rect) = app.ui_bounds.title_rect {
                    if y == title_rect.y && x >= title_rect.x && x < title_rect.x + title_rect.width
                    {
                        app.search_song_web();
                        return;
                    }
                }

                if let Some(artist_rect) = app.ui_bounds.artist_rect {
                    if y == artist_rect.y
                        && x >= artist_rect.x
                        && x < artist_rect.x + artist_rect.width
                    {
                        app.search_artist_web();
                        return;
                    }
                }
            }

            // 6. Left panel list item click (Queue / Library / Search)
            if let Some(panel_rect) = app.ui_bounds.left_panel_rect {
                let is_on_scrollbar = app
                    .ui_bounds
                    .scrollbar_rect
                    .map(|sb| x >= sb.x)
                    .unwrap_or(false)
                    || x >= panel_rect.x + panel_rect.width.saturating_sub(1);

                if !is_on_scrollbar
                    && x >= panel_rect.x
                    && x < panel_rect.x + panel_rect.width
                    && y >= panel_rect.y
                    && y < panel_rect.y + panel_rect.height
                {
                    let clicked_row = (y - panel_rect.y) as usize;
                    match app.active_panel {
                        ActivePanel::Queue => {
                            let visual_items = app
                                .playlist
                                .get_visual_items(app.show_folders, app.config.strip_track_numbers);
                            let visible_height = panel_rect.height as usize;
                            let half = visible_height / 2;
                            let mut highlighted_idx = 0;
                            for (idx, item) in visual_items.iter().enumerate() {
                                if let crate::data::playlist::QueueVisualItem::Track {
                                    entry_idx,
                                    ..
                                } = item
                                {
                                    if *entry_idx == app.queue_cursor {
                                        highlighted_idx = idx;
                                        break;
                                    }
                                }
                            }
                            let scroll = highlighted_idx
                                .saturating_sub(half)
                                .min(visual_items.len().saturating_sub(1));
                            let item_idx = scroll + clicked_row;
                            let clicked_entry = if item_idx < visual_items.len() {
                                if let crate::data::playlist::QueueVisualItem::Track {
                                    entry_idx,
                                    ..
                                } = &visual_items[item_idx]
                                {
                                    Some(*entry_idx)
                                } else {
                                    None
                                }
                            } else {
                                None
                            };
                            drop(visual_items);

                            if let Some(entry_idx) = clicked_entry {
                                if app.queue_cursor == entry_idx {
                                    handle_enter(app);
                                } else {
                                    app.queue_cursor = entry_idx;
                                }
                            }
                        }
                        ActivePanel::Library => {
                            let visible_height = panel_rect.height as usize;
                            let total_items = app.flat_library.len() + 1;
                            let scroll =
                                if visible_height > 0 && app.library_cursor >= visible_height {
                                    app.library_cursor - visible_height + 1
                                } else {
                                    0
                                }
                                .min(total_items.saturating_sub(1));
                            let target_cursor = scroll + clicked_row;
                            if target_cursor < total_items {
                                if app.library_cursor == target_cursor {
                                    handle_enter(app);
                                } else {
                                    app.library_cursor = target_cursor;
                                }
                            }
                        }
                        ActivePanel::Search => {
                            if clicked_row >= 2 {
                                let list_row = clicked_row - 2;
                                let results_height = panel_rect.height.saturating_sub(2) as usize;
                                let scroll =
                                    if results_height > 0 && app.search_cursor >= results_height {
                                        app.search_cursor - results_height + 1
                                    } else {
                                        0
                                    }
                                    .min(app.search_results.len().saturating_sub(1));
                                let target_idx = scroll + list_row;
                                if target_idx < app.search_results.len() {
                                    if app.search_cursor == target_idx {
                                        handle_enter(app);
                                    } else {
                                        app.search_cursor = target_idx;
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                    app.refresh_needed = true;
                }
            }
        }
        MouseEventKind::ScrollUp => scroll_up(app),
        MouseEventKind::ScrollDown => scroll_down(app),
        _ => {}
    }
}
