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

    // If the user presses F2-F6 or Tab/BackTab, turn off search immediately
    match key.code {
        KeyCode::F(2)
        | KeyCode::F(3)
        | KeyCode::F(4)
        | KeyCode::F(5)
        | KeyCode::F(6)
        | KeyCode::Tab
        | KeyCode::BackTab => {
            app.searching = false;
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
            let starting_position = app.pending_seek
                .map(|d| d.as_millis() as u64)
                .unwrap_or_else(|| app.player().map(|p| p.elapsed_ms()).unwrap_or(0));
            let target = starting_position.saturating_sub(5000);
            app.pending_seek = Some(std::time::Duration::from_millis(target));
            app.last_seek_input = Some(std::time::Instant::now());
            app.refresh_needed = true;
        }
        KeyCode::Char('d') => {
            let starting_position = app.pending_seek
                .map(|d| d.as_millis() as u64)
                .unwrap_or_else(|| app.player().map(|p| p.elapsed_ms()).unwrap_or(0));
            let target = (starting_position + 5000).min(app.player().map(|p| p.duration_ms()).unwrap_or(0));
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
            app.search_cursor += 1;
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
        ActivePanel::NowPlaying
            if app.show_full_lyrics && app.lyrics_scroll > 0 => {
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
            app.search_cursor += 1;
        }
        ActivePanel::NowPlaying
            if app.show_full_lyrics => {
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
    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            let _x = mouse.column;
            let _y = mouse.row;
            // Mouse click handling for progress bar, tabs, etc.
            // Will be implemented with area tracking
        }
        MouseEventKind::ScrollUp => scroll_up(app),
        MouseEventKind::ScrollDown => scroll_down(app),
        _ => {}
    }
}
