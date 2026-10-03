use crossterm::event::{self, KeyCode, KeyEventKind, MouseButton, MouseEventKind};

use crate::app::{ActivePanel, App};

/// Handle a crossterm key event. Returns true if the app should quit.
pub fn handle_key(app: &mut App, key: event::KeyEvent) -> bool {
    // Ignore release events to prevent double processing
    if key.kind == KeyEventKind::Release {
        return false;
    }

    // Explicit Ctrl+C global quit
    if key.modifiers.contains(event::KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        return true;
    }

    // First stage: Source switching (Ctrl+1..4 or Alt+1..4).
    // Checked before loading guard and search input so switching works anywhere
    // and digits are never typed into search boxes or directory input.
    let is_ctrl = key.modifiers.contains(event::KeyModifiers::CONTROL);
    let is_alt = key.modifiers.contains(event::KeyModifiers::ALT);
    if is_ctrl || is_alt {
        let target_tab = match key.code {
            KeyCode::Char('1') => Some(crate::app::SourceTab::Local),
            KeyCode::Char('2') => Some(crate::app::SourceTab::Spotify),
            KeyCode::Char('3') => Some(crate::app::SourceTab::YouTube),
            KeyCode::Char('4') => Some(crate::app::SourceTab::Unified),
            _ => None,
        };
        if let Some(tab) = target_tab {
            app.switch_source(tab);
            return false;
        }
    }

    // Loading guard: while the audio engine is initializing on a background thread,
    // only allow safe keys (quit, navigation, search panel). All playback-related
    // keys are silently swallowed — no panic, no queuing.
    if app.player_loading && !app.awaiting_dir_input && !app.searching {
        match key.code {
            KeyCode::Char('q') => return true,
            KeyCode::Tab => app.next_panel(),
            KeyCode::BackTab => app.prev_panel(),
            KeyCode::F(2) => app.active_panel = ActivePanel::Queue,
            KeyCode::F(3) if app.source != crate::app::SourceTab::Unified => {
                app.active_panel = ActivePanel::Library;
            }
            KeyCode::F(5) if app.source != crate::app::SourceTab::Unified => {
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
        KeyCode::F(5) if app.source != crate::app::SourceTab::Unified => {
            app.active_panel = ActivePanel::Search;
            app.searching = true;
            app.search_query.clear();
        }
        _ => {}
    }

    // If in remote credentials input mode
    // (the prompt is only drawn in the Library panel, so only capture keys there)
    let login_panel = app.active_panel == ActivePanel::Library;
    if let Some(view) = app.active_source_view_mut() {
        if view.awaiting_login_input && login_panel {
            match key.code {
                KeyCode::Esc => {
                    view.awaiting_login_input = false;
                    app.refresh_needed = true;
                    return false;
                }
                KeyCode::Enter => {
                    app.remote_enqueue_selected(false, false);
                    return false;
                }
                KeyCode::Backspace => {
                    view.login_input.pop();
                    app.refresh_needed = true;
                    return false;
                }
                KeyCode::Char(c) if !key.modifiers.contains(event::KeyModifiers::CONTROL) => {
                    view.login_input.push(c);
                    app.refresh_needed = true;
                    return false;
                }
                KeyCode::Char('u') if key.modifiers.contains(event::KeyModifiers::CONTROL) => {
                    view.login_input.clear();
                    app.refresh_needed = true;
                    return false;
                }
                _ => return false,
            }
        }
    }

    // If we're in search mode, handle text input
    if app.searching {
        return handle_search_input(app, key);
    }

    // If in directory input mode (first-run config)
    if app.awaiting_dir_input {
        return handle_dir_input(app, key);
    }

    // Guard against unhandled Control key combinations firing plain character actions (e.g. Ctrl+S firing shuffle)
    if is_ctrl {
        return false;
    }

    match key.code {
        // Global quit
        KeyCode::Char('q') => return true,

        // Esc returns to Queue if in another panel
        KeyCode::Esc => {
            if app.active_panel != ActivePanel::Queue {
                app.active_panel = ActivePanel::Queue;
                app.refresh_needed = true;
            }
        }

        // View switching
        KeyCode::F(2) => app.active_panel = ActivePanel::Queue,
        KeyCode::F(3) if app.source != crate::app::SourceTab::Unified => {
            app.active_panel = ActivePanel::Library;
        }
        KeyCode::F(4) => app.active_panel = ActivePanel::NowPlaying,
        KeyCode::F(5) if app.source != crate::app::SourceTab::Unified => {
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
            if key.modifiers.contains(event::KeyModifiers::SHIFT) {
                handle_play_next(app);
            } else if key.modifiers.contains(event::KeyModifiers::ALT) {
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
            if key.modifiers.contains(event::KeyModifiers::SHIFT) {
                handle_play_next(app);
            } else if key.modifiers.contains(event::KeyModifiers::ALT) {
                handle_enqueue_and_play(app);
            } else {
                handle_enter(app);
            }
        }
        KeyCode::Backspace => {
            if app.source == crate::app::SourceTab::Spotify
                || app.source == crate::app::SourceTab::YouTube
            {
                if let Some(view) = app.active_source_view_mut() {
                    view.search_query.pop();
                    app.remote_search_pending =
                        Some((view.search_query.clone(), std::time::Instant::now()));
                }
            } else {
                app.search_query.pop();
                app.run_search();
            }
        }
        KeyCode::Up => {
            if app.source == crate::app::SourceTab::Spotify
                || app.source == crate::app::SourceTab::YouTube
            {
                if let Some(view) = app.active_source_view_mut() {
                    if view.search_cursor > 0 {
                        view.search_cursor -= 1;
                    }
                }
            } else if app.search_cursor > 0 {
                app.search_cursor -= 1;
            }
        }
        KeyCode::Down => {
            if app.source == crate::app::SourceTab::Spotify
                || app.source == crate::app::SourceTab::YouTube
            {
                if let Some(view) = app.active_source_view_mut() {
                    if !view.search_results.is_empty()
                        && view.search_cursor + 1 < view.search_results.len()
                    {
                        view.search_cursor += 1;
                    }
                }
            } else if !app.search_results.is_empty()
                && app.search_cursor + 1 < app.search_results.len()
            {
                app.search_cursor += 1;
            }
        }
        KeyCode::Char(c) if !key.modifiers.contains(event::KeyModifiers::CONTROL) => {
            if app.source == crate::app::SourceTab::Spotify
                || app.source == crate::app::SourceTab::YouTube
            {
                if let Some(view) = app.active_source_view_mut() {
                    view.search_query.push(c);
                    app.remote_search_pending =
                        Some((view.search_query.clone(), std::time::Instant::now()));
                }
            } else {
                app.search_query.push(c);
                app.run_search();
            }
        }
        KeyCode::Char('u') if key.modifiers.contains(event::KeyModifiers::CONTROL) => {
            if app.source == crate::app::SourceTab::Spotify
                || app.source == crate::app::SourceTab::YouTube
            {
                if let Some(view) = app.active_source_view_mut() {
                    view.search_query.clear();
                    app.remote_search_pending =
                        Some((view.search_query.clone(), std::time::Instant::now()));
                }
            } else {
                app.search_query.clear();
                app.run_search();
            }
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
        KeyCode::Char(c) if !key.modifiers.contains(event::KeyModifiers::CONTROL) => {
            app.dir_input.push(c);
        }
        KeyCode::Char('u') if key.modifiers.contains(event::KeyModifiers::CONTROL) => {
            app.dir_input.clear();
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
            if app.source == crate::app::SourceTab::Spotify
                || app.source == crate::app::SourceTab::YouTube
            {
                if let Some(view) = app.active_source_view_mut() {
                    if view.cursor > 0 {
                        view.cursor -= 1;
                    }
                }
            } else if app.library_cursor > 0 {
                app.library_cursor -= 1;
            }
        }
        ActivePanel::Search => {
            if app.source == crate::app::SourceTab::Spotify
                || app.source == crate::app::SourceTab::YouTube
            {
                if let Some(view) = app.active_source_view_mut() {
                    if view.search_cursor > 0 {
                        view.search_cursor -= 1;
                    }
                }
            } else if app.search_cursor > 0 {
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
            if app.source == crate::app::SourceTab::Spotify
                || app.source == crate::app::SourceTab::YouTube
            {
                if let Some(view) = app.active_source_view_mut() {
                    let max = view.flat.len().saturating_sub(1);
                    if view.cursor < max {
                        view.cursor += 1;
                    }
                }
            } else {
                let max = app.flat_library.len();
                if app.library_cursor < max {
                    app.library_cursor += 1;
                }
            }
        }
        ActivePanel::Search => {
            if app.source == crate::app::SourceTab::Spotify
                || app.source == crate::app::SourceTab::YouTube
            {
                if let Some(view) = app.active_source_view_mut() {
                    let max = view.search_results.len().saturating_sub(1);
                    if view.search_cursor < max {
                        view.search_cursor += 1;
                    }
                }
            } else if !app.search_results.is_empty()
                && app.search_cursor + 1 < app.search_results.len()
            {
                app.search_cursor += 1;
            }
        }
        ActivePanel::NowPlaying if app.show_full_lyrics => {
            let total_lines = app
                .current_lyrics
                .as_ref()
                .map(|l| l.lines.len())
                .or_else(|| {
                    app.playlist
                        .current_entry()
                        .and_then(|e| match &e.metadata.lyrics {
                            crate::data::metadata::LyricsKind::Timed(lines) => Some(lines.len()),
                            crate::data::metadata::LyricsKind::Untimed(lines) => Some(lines.len()),
                            crate::data::metadata::LyricsKind::None => None,
                        })
                })
                .unwrap_or(0);
            if (app.lyrics_scroll as usize) + 1 < total_lines {
                app.lyrics_scroll += 1;
            }
        }
        _ => {}
    }
}

fn handle_enter(app: &mut App) {
    if (app.source == crate::app::SourceTab::Spotify
        || app.source == crate::app::SourceTab::YouTube)
        && (app.active_panel == ActivePanel::Library || app.active_panel == ActivePanel::Search)
    {
        app.remote_enqueue_selected(false, false);
        return;
    }
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
    if (app.source == crate::app::SourceTab::Spotify
        || app.source == crate::app::SourceTab::YouTube)
        && (app.active_panel == ActivePanel::Library || app.active_panel == ActivePanel::Search)
    {
        app.remote_enqueue_selected(true, false);
        return;
    }
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

fn handle_play_next(app: &mut App) {
    if (app.source == crate::app::SourceTab::Spotify
        || app.source == crate::app::SourceTab::YouTube)
        && (app.active_panel == ActivePanel::Library || app.active_panel == ActivePanel::Search)
    {
        app.remote_enqueue_selected(false, true);
        return;
    }
    let entry = match app.active_panel {
        // Row 0 is the library header; entries start at cursor 1
        ActivePanel::Library => app
            .library_cursor
            .checked_sub(1)
            .and_then(|idx| app.flat_library.get(idx))
            .map(|item| item.entry.clone()),
        ActivePanel::Search => app.search_results.get(app.search_cursor).cloned(),
        _ => None,
    };
    let Some(entry) = entry else {
        return;
    };
    let is_dir = entry.is_dir();
    let tracks: Vec<_> = entry
        .get_all_tracks()
        .into_iter()
        .filter(|(id, _)| !app.playlist.entry_ids.contains(id))
        .collect();
    if tracks.is_empty() {
        return;
    }

    if app.playlist.is_empty() {
        // Nothing is playing: queue in listed order and start from the first track
        for (id, meta) in tracks {
            app.playlist.add(id, meta);
        }
        app.playlist.current = 0;
        app.play_current();
    } else {
        // Insert in reverse so the first listed track ends up directly after the current one
        for (id, meta) in tracks.into_iter().rev() {
            app.playlist.play_next(id, meta);
        }
    }
    app.set_status(if is_dir {
        "Folder queued next"
    } else {
        "Track queued next"
    });
    app.rebuild_flat_library_view();
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

            // 3a. Footer source tab click
            if app.status_msg.is_none() {
                if let Some(source_rect) = app.ui_bounds.footer_source_rect {
                    if y == source_rect.y
                        && x >= source_rect.x
                        && x < source_rect.x + source_rect.width
                    {
                        const TOTAL_LEN: u16 = 39;
                        if source_rect.width >= TOTAL_LEN {
                            let start_x = source_rect.x + (source_rect.width - TOTAL_LEN) / 2;
                            if x >= start_x && x < start_x + TOTAL_LEN {
                                let offset = x - start_x;
                                let tab = if offset < 8 {
                                    Some(crate::app::SourceTab::Local)
                                } else if offset < 20 {
                                    Some(crate::app::SourceTab::Spotify)
                                } else if offset < 32 {
                                    Some(crate::app::SourceTab::YouTube)
                                } else {
                                    Some(crate::app::SourceTab::Unified)
                                };
                                if let Some(tab) = tab {
                                    app.switch_source(tab);
                                    return;
                                }
                            }
                        } else {
                            let tab_width = source_rect.width / 4;
                            let rel_x = x - source_rect.x;
                            if let Some(div) = rel_x.checked_div(tab_width) {
                                let selected = div.min(3) as usize;
                                if let Some(tab) = crate::app::SourceTab::from_index(selected) {
                                    app.switch_source(tab);
                                    return;
                                }
                            }
                        }
                    }
                }
            }

            // 3b. Footer tab click
            if let Some(footer_rect) = app.ui_bounds.footer_tabs_rect {
                if y == footer_rect.y && x >= footer_rect.x && x < footer_rect.x + footer_rect.width
                {
                    let rel_x = x - footer_rect.x;
                    let tabs = crate::app::tabs_for(app.source);
                    let tab_width = footer_rect.width / (tabs.len() as u16);
                    if let Some(div) = rel_x.checked_div(tab_width) {
                        let selected_tab = div.min(tabs.len() as u16 - 1) as usize;
                        let prev_panel = app.active_panel;
                        let (_, _, panel) = tabs[selected_tab];
                        if panel == ActivePanel::Search {
                            app.active_panel = ActivePanel::Search;
                            app.searching = true;
                            app.search_query.clear();
                        } else {
                            app.active_panel = panel;
                        }

                        if prev_panel == ActivePanel::Search
                            && app.active_panel != ActivePanel::Search
                        {
                            app.searching = false;
                            app.search_query.clear();
                        }
                        app.refresh_needed = true;
                        return;
                    }
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
                    // col 20: ∅  (cols 18..20)
                    if x < indent || x >= indent + total_w {
                        return;
                    }

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
                    match app.active_panel {
                        ActivePanel::Queue => {
                            if panel_rect.height >= 2 && y == panel_rect.y {
                                return;
                            }
                            let clicked_row = if panel_rect.height >= 2 {
                                (y - panel_rect.y - 1) as usize
                            } else {
                                (y - panel_rect.y) as usize
                            };
                            let visible_height = if panel_rect.height >= 2 {
                                (panel_rect.height - 1) as usize
                            } else {
                                panel_rect.height as usize
                            };
                            let visual_items = app
                                .playlist
                                .get_visual_items(app.show_folders, app.config.strip_track_numbers);
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
                            let clicked_row = (y - panel_rect.y) as usize;
                            if app.source == crate::app::SourceTab::Spotify
                                || app.source == crate::app::SourceTab::YouTube
                            {
                                if let Some(view) = app.active_source_view_mut() {
                                    let visible_height = panel_rect.height as usize;
                                    let (start, end) = crate::ui::widgets::scroll_offset(
                                        view.cursor,
                                        view.flat.len(),
                                        visible_height,
                                    );
                                    let target_cursor = start + clicked_row;
                                    if target_cursor < end {
                                        if view.cursor == target_cursor {
                                            handle_enter(app);
                                        } else {
                                            view.cursor = target_cursor;
                                        }
                                    }
                                }
                            } else {
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
                        }
                        ActivePanel::Search if (y - panel_rect.y) >= 2 => {
                            let clicked_row = (y - panel_rect.y) as usize;
                            let list_row = clicked_row - 2;
                            let results_height = panel_rect.height.saturating_sub(2) as usize;
                            if app.source == crate::app::SourceTab::Spotify
                                || app.source == crate::app::SourceTab::YouTube
                            {
                                if let Some(view) = app.active_source_view_mut() {
                                    let (start, end) = crate::ui::widgets::scroll_offset(
                                        view.search_cursor,
                                        view.search_results.len(),
                                        results_height,
                                    );
                                    let target_idx = start + list_row;
                                    if target_idx < end {
                                        if view.search_cursor == target_idx {
                                            handle_enter(app);
                                        } else {
                                            view.search_cursor = target_idx;
                                        }
                                    }
                                }
                            } else {
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
