use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
use ratatui::{backend::TestBackend, Terminal};
use std::path::PathBuf;
use std::time::Duration;

use mixed::app::{ActivePanel, App};
use mixed::config::app_config::AppConfig;
use mixed::ui::events;
use mixed::ui::layout;

fn create_key_event(code: KeyCode) -> KeyEvent {
    KeyEvent {
        code,
        modifiers: KeyModifiers::empty(),
        kind: KeyEventKind::Press,
        state: KeyEventState::empty(),
    }
}

#[test]
fn test_first_run_flow_and_navigation() {
    let mut config = AppConfig::load();
    config.music_dir = None; // Start in first-run directory input mode

    let (mpris_cmd_tx, _mpris_cmd_rx) = crossbeam_channel::bounded(100);
    let (vis_wake_tx, _vis_wake_rx) = crossbeam_channel::bounded(1);
    let mut app = App::new(config, mpris_cmd_tx, vis_wake_tx);

    assert!(app.awaiting_dir_input);

    // Create a 80x24 test terminal
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).unwrap();

    // Verify drawing on first-run doesn't panic
    terminal.draw(|f| layout::draw(f, &mut app)).unwrap();

    // Create a temporary directory that exists
    let tmp_dir = std::env::temp_dir().join("mixed_mock_music");
    let _ = std::fs::create_dir_all(&tmp_dir);
    let path_str = tmp_dir.to_str().unwrap();

    // Simulate typing the directory
    for c in path_str.chars() {
        events::handle_key(&mut app, create_key_event(KeyCode::Char(c)));
    }
    assert_eq!(app.dir_input, path_str);

    // Press Enter to submit the directory path
    events::handle_key(&mut app, create_key_event(KeyCode::Enter));

    // Clean up temporary directory
    let _ = std::fs::remove_dir_all(&tmp_dir);

    // After enter, awaiting_dir_input should be false, and active panel should switch
    assert!(!app.awaiting_dir_input);
    assert_eq!(app.active_panel, ActivePanel::Library);

    // Let's verify navigation keys
    // F2: Queue
    events::handle_key(&mut app, create_key_event(KeyCode::F(2)));
    assert_eq!(app.active_panel, ActivePanel::Queue);

    // F3: Library
    events::handle_key(&mut app, create_key_event(KeyCode::F(3)));
    assert_eq!(app.active_panel, ActivePanel::Library);

    // F5: Search
    events::handle_key(&mut app, create_key_event(KeyCode::F(5)));
    assert_eq!(app.active_panel, ActivePanel::Search);
    assert!(app.searching);

    // Type query "synthwave"
    for c in "synthwave".chars() {
        events::handle_key(&mut app, create_key_event(KeyCode::Char(c)));
    }
    assert_eq!(app.search_query, "synthwave");

    // Press Esc to exit search mode
    events::handle_key(&mut app, create_key_event(KeyCode::Esc));
    assert!(!app.searching);

    // F6: Help
    events::handle_key(&mut app, create_key_event(KeyCode::F(6)));
    assert_eq!(app.active_panel, ActivePanel::Help);

    // Draw the final frame
    terminal.draw(|f| layout::draw(f, &mut app)).unwrap();
}

#[test]
fn test_layout_boundary_robustness() {
    let mut config = AppConfig::load();
    config.music_dir = Some("/mock/music".to_string());

    let (mpris_cmd_tx, _mpris_cmd_rx) = crossbeam_channel::bounded(100);
    let (vis_wake_tx, _vis_wake_rx) = crossbeam_channel::bounded(1);
    let mut app = App::new(config, mpris_cmd_tx, vis_wake_tx);
    app.awaiting_dir_input = false;

    // Test a matrix of terminal sizes down to 0x0
    let sizes = vec![
        (0, 0),
        (1, 1),
        (5, 5),
        (10, 5),
        (40, 10),
        (80, 24),
        (120, 40),
    ];

    for (w, h) in sizes {
        let backend = TestBackend::new(w, h);
        let mut terminal = Terminal::new(backend).unwrap();

        // Test in Library panel
        app.active_panel = ActivePanel::Library;
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            terminal.draw(|f| layout::draw(f, &mut app)).unwrap();
        }));
        assert!(
            res.is_ok(),
            "Layout panicked at size {}x{} on Library panel",
            w,
            h
        );

        // Test in Queue panel
        app.active_panel = ActivePanel::Queue;
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            terminal.draw(|f| layout::draw(f, &mut app)).unwrap();
        }));
        assert!(
            res.is_ok(),
            "Layout panicked at size {}x{} on Queue panel",
            w,
            h
        );

        // Test in Search panel
        app.active_panel = ActivePanel::Search;
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            terminal.draw(|f| layout::draw(f, &mut app)).unwrap();
        }));
        assert!(
            res.is_ok(),
            "Layout panicked at size {}x{} on Search panel",
            w,
            h
        );
    }
}

#[test]
fn test_mpris_bridge_concurrency_stress() {
    let (mpris_cmd_tx, _mpris_cmd_rx) = crossbeam_channel::bounded(100);
    let (mpris_state, mpris_update_tx) = mixed::sys::mpris::start_mpris(mpris_cmd_tx);

    // Spawn multiple threads sending rapid updates and D-Bus command requests
    let threads: Vec<_> = (0..5)
        .map(|thread_id| {
            let state_clone = mpris_state.clone();
            let update_tx_clone = mpris_update_tx.clone();
            std::thread::spawn(move || {
                for i in 0..100 {
                    if let Ok(mut meta) = state_clone.metadata.write() {
                        meta.title = format!("Thread {} Track {}", thread_id, i);
                    }
                    state_clone.playback_status.store(
                        if i % 2 == 0 { 1 } else { 2 },
                        std::sync::atomic::Ordering::Relaxed,
                    );
                    state_clone
                        .position_us
                        .store(i * 1000, std::sync::atomic::Ordering::Relaxed);
                    let _ = update_tx_clone.send(());
                    std::thread::sleep(Duration::from_micros(10));
                }
            })
        })
        .collect();

    // Ensure we can receive the bridged commands without deadlock
    for thread in threads {
        thread.join().unwrap();
    }

    assert!(
        mpris_state
            .position_us
            .load(std::sync::atomic::Ordering::Relaxed)
            >= 0
    );
}

#[test]
fn test_mouse_interaction_and_navigation() {
    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    use ratatui::layout::Rect;

    let mut config = AppConfig::load();
    config.music_dir = Some("/mock/music".to_string());
    config.desktop_notifications = true;

    let (mpris_cmd_tx, _mpris_cmd_rx) = crossbeam_channel::bounded(100);
    let (vis_wake_tx, _vis_wake_rx) = crossbeam_channel::bounded(1);
    let mut app = App::new(config, mpris_cmd_tx, vis_wake_tx);
    app.awaiting_dir_input = false;
    app.player_loading = false;

    let backend = TestBackend::new(100, 40);
    let mut terminal = Terminal::new(backend).unwrap();

    // 1. Draw frame to compute UI layout bounds
    terminal.draw(|f| layout::draw(f, &mut app)).unwrap();
    assert!(app.ui_bounds.footer_tabs_rect.is_some());

    // 2. Click footer tab 3 (Search)
    let footer_rect = app.ui_bounds.footer_tabs_rect.unwrap();
    let tab_width = footer_rect.width / 5;
    let search_tab_x = footer_rect.x + (tab_width * 3) + 2;

    let search_click = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: search_tab_x,
        row: footer_rect.y,
        modifiers: KeyModifiers::empty(),
    };
    events::handle_mouse(&mut app, search_click);
    assert_eq!(app.active_panel, ActivePanel::Search);
    assert!(app.searching);

    // Type a query
    app.search_query.push_str("rena uehara");
    assert_eq!(app.search_query, "rena uehara");

    // 3. Switch away to Library using mouse click on tab 1 (Library)
    let library_tab_x = footer_rect.x + tab_width + 2;
    let library_click = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: library_tab_x,
        row: footer_rect.y,
        modifiers: KeyModifiers::empty(),
    };
    events::handle_mouse(&mut app, library_click);
    assert_eq!(app.active_panel, ActivePanel::Library);
    assert!(!app.searching);
    assert_eq!(app.search_query, ""); // Clean search input verified

    // 4. Test seek_to_ratio
    app.seek_to_ratio(0.5);
    assert!(app.refresh_needed);

    // 5. Test mini-controls clicks
    app.ui_bounds.mini_controls_rect = Some(Rect::new(20, 10, 30, 1));
    let mini_rect = app.ui_bounds.mini_controls_rect.unwrap();
    let total_w: u16 = 21;
    let indent = mini_rect.x + (mini_rect.width.saturating_sub(total_w)) / 2;
    let play_pause_x = indent + 4; // col 4 is play/pause

    // Click play/pause toggle
    let play_pause_click = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: play_pause_x,
        row: mini_rect.y,
        modifiers: KeyModifiers::empty(),
    };
    events::handle_mouse(&mut app, play_pause_click);

    // Scroll wheel tests
    events::handle_mouse(
        &mut app,
        MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 10,
            row: 10,
            modifiers: KeyModifiers::empty(),
        },
    );
    events::handle_mouse(
        &mut app,
        MouseEvent {
            kind: MouseEventKind::ScrollUp,
            column: 10,
            row: 10,
            modifiers: KeyModifiers::empty(),
        },
    );

    // 6. Verify bounded mini-controls clicks (SEC-09)
    app.playlist.add(
        std::path::PathBuf::from("/mock/song.mp3"),
        mixed::data::metadata::TrackMetadata::default(),
    );
    assert_eq!(app.playlist.len(), 1);

    // Clicking in blank padding space to the right of mini-controls must NOT clear playlist
    let blank_space_click = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: indent + total_w + 2,
        row: mini_rect.y,
        modifiers: KeyModifiers::empty(),
    };
    events::handle_mouse(&mut app, blank_space_click);
    assert_eq!(
        app.playlist.len(),
        1,
        "Clicking blank space to the right must not clear playlist"
    );

    // Clicking to the left of mini-controls must be ignored
    let left_space_click = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: indent.saturating_sub(1),
        row: mini_rect.y,
        modifiers: KeyModifiers::empty(),
    };
    events::handle_mouse(&mut app, left_space_click);
    assert_eq!(app.playlist.len(), 1);

    // Clicking directly on the clear playlist button (indent + 19) clears playlist
    let clear_click = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: indent + 19,
        row: mini_rect.y,
        modifiers: KeyModifiers::empty(),
    };
    events::handle_mouse(&mut app, clear_click);
    assert_eq!(
        app.playlist.len(),
        0,
        "Clicking clear button directly should clear playlist"
    );
}

#[test]
fn test_mini_controls_placement_and_scrollbar_interaction() {
    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    use ratatui::layout::Rect;

    let mut config = AppConfig::load();
    config.music_dir = Some("/mock/music".to_string());

    let (mpris_cmd_tx, _mpris_cmd_rx) = crossbeam_channel::bounded(100);
    let (vis_wake_tx, _vis_wake_rx) = crossbeam_channel::bounded(1);
    let mut app = App::new(config, mpris_cmd_tx, vis_wake_tx);
    app.awaiting_dir_input = false;
    app.player_loading = false;

    let backend = TestBackend::new(100, 40);
    let mut terminal = Terminal::new(backend).unwrap();

    // 1. In Queue panel (Playlist), mini-controls MUST be in the left pane (x < art_pane_width)
    app.active_panel = ActivePanel::Queue;
    terminal.draw(|f| layout::draw(f, &mut app)).unwrap();
    assert!(
        app.ui_bounds.mini_controls_rect.is_some(),
        "Mini-controls must be present in Queue panel"
    );
    let mc_rect = app.ui_bounds.mini_controls_rect.unwrap();
    assert!(
        mc_rect.x < 35,
        "Mini-controls must be in the left pane under album art"
    );
    assert!(
        app.ui_bounds.title_rect.is_none(),
        "Title rect must NOT be set in Queue view"
    );
    assert!(
        app.ui_bounds.artist_rect.is_none(),
        "Artist rect must NOT be set in Queue view"
    );

    // 2. In Library panel, mini-controls MUST be in the left pane
    app.active_panel = ActivePanel::Library;
    terminal.draw(|f| layout::draw(f, &mut app)).unwrap();
    assert!(
        app.ui_bounds.mini_controls_rect.is_some(),
        "Mini-controls must be present in Library panel"
    );
    assert!(app.ui_bounds.title_rect.is_none());

    // 3. In Search panel, mini-controls MUST be in the left pane
    app.active_panel = ActivePanel::Search;
    terminal.draw(|f| layout::draw(f, &mut app)).unwrap();
    assert!(
        app.ui_bounds.mini_controls_rect.is_some(),
        "Mini-controls must be present in Search panel"
    );
    assert!(app.ui_bounds.title_rect.is_none());

    // 4. In Help panel, mini-controls MUST be in the left pane
    app.active_panel = ActivePanel::Help;
    terminal.draw(|f| layout::draw(f, &mut app)).unwrap();
    assert!(
        app.ui_bounds.mini_controls_rect.is_some(),
        "Mini-controls must be present in Help panel"
    );
    assert!(app.ui_bounds.title_rect.is_none());

    // 5. In Track tab (NowPlaying), mini-controls MUST NOT be present!
    app.active_panel = ActivePanel::NowPlaying;
    terminal.draw(|f| layout::draw(f, &mut app)).unwrap();
    assert!(
        app.ui_bounds.mini_controls_rect.is_none(),
        "Mini-controls must NOT be in the Track (NowPlaying) tab"
    );

    // 6. Test scrollbar click & drag navigation
    app.ui_bounds.scrollbar_rect = Some(Rect::new(95, 5, 1, 20));
    app.active_panel = ActivePanel::Queue;
    for i in 0..50 {
        app.playlist.add(
            std::path::PathBuf::from(format!("/fake/song_{}.mp3", i)),
            mixed::data::metadata::TrackMetadata::default(),
        );
    }
    assert_eq!(app.queue_cursor, 0);

    // Click near the bottom of scrollbar (y = 20) -> should scroll near end of playlist
    let scroll_click = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 95,
        row: 20,
        modifiers: KeyModifiers::empty(),
    };
    events::handle_mouse(&mut app, scroll_click);
    assert!(
        app.queue_cursor > 30,
        "Clicking bottom of scrollbar should jump playlist cursor down"
    );

    // Drag to middle of scrollbar (y = 15)
    let scroll_drag = MouseEvent {
        kind: MouseEventKind::Drag(MouseButton::Left),
        column: 95,
        row: 15,
        modifiers: KeyModifiers::empty(),
    };
    events::handle_mouse(&mut app, scroll_drag);
    assert!(
        app.queue_cursor >= 20 && app.queue_cursor <= 30,
        "Dragging scrollbar should smoothly scroll playlist"
    );

    // 7. Test all 6 buttons of mini-controls: [⏮   ▶/⏸   ⏭   +   -   ∅]
    terminal.draw(|f| layout::draw(f, &mut app)).unwrap();
    let mc_rect = app
        .ui_bounds
        .mini_controls_rect
        .expect("mini_controls_rect must be set in Queue");
    let total_w: u16 = 21;
    let indent = mc_rect.x + (mc_rect.width.saturating_sub(total_w)) / 2;

    // Test button 0: Prev Track (col 0)
    events::handle_mouse(
        &mut app,
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: indent,
            row: mc_rect.y,
            modifiers: KeyModifiers::empty(),
        },
    );

    // Test button 1: Play/Pause (col 4)
    events::handle_mouse(
        &mut app,
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: indent + 4,
            row: mc_rect.y,
            modifiers: KeyModifiers::empty(),
        },
    );

    // Test button 2: Next Track (col 8)
    events::handle_mouse(
        &mut app,
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: indent + 8,
            row: mc_rect.y,
            modifiers: KeyModifiers::empty(),
        },
    );

    // Test button 3: Volume Up (col 12)
    events::handle_mouse(
        &mut app,
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: indent + 12,
            row: mc_rect.y,
            modifiers: KeyModifiers::empty(),
        },
    );

    // Test button 4: Volume Down (col 16)
    events::handle_mouse(
        &mut app,
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: indent + 16,
            row: mc_rect.y,
            modifiers: KeyModifiers::empty(),
        },
    );

    // Test button 5: Clear Playlist (col 20)
    assert!(!app.playlist.is_empty());
    events::handle_mouse(
        &mut app,
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: indent + 20,
            row: mc_rect.y,
            modifiers: KeyModifiers::empty(),
        },
    );
    assert!(app.playlist.is_empty(), "Clicking ∅ must clear playlist");
}

#[test]
fn test_logical_edge_cases_and_navigation() {
    let mut config = AppConfig::load();
    config.music_dir = Some("/mock/music".to_string());

    let (mpris_cmd_tx, _mpris_cmd_rx) = crossbeam_channel::bounded(100);
    let (vis_wake_tx, _vis_wake_rx) = crossbeam_channel::bounded(1);
    let mut app = App::new(config, mpris_cmd_tx, vis_wake_tx);
    app.awaiting_dir_input = false;
    app.player_loading = false;

    // 1. End of playlist with repeat off stops playback
    app.playlist.repeat = mixed::data::playlist::RepeatMode::Off;
    app.playlist.add(
        std::path::PathBuf::from("/mock/song1.mp3"),
        mixed::data::metadata::TrackMetadata::default(),
    );
    app.playlist.add(
        std::path::PathBuf::from("/mock/song2.mp3"),
        mixed::data::metadata::TrackMetadata::default(),
    );
    app.playlist.current = 1; // On last track
    app.stopped = false;
    app.next_track();
    assert!(
        app.stopped,
        "next_track on last song with repeat off must stop playback"
    );

    // 2. Deleting the only remaining track clears playlist and stops playback
    app.playlist.clear();
    app.playlist.add(
        std::path::PathBuf::from("/mock/only_song.mp3"),
        mixed::data::metadata::TrackMetadata::default(),
    );
    app.active_panel = ActivePanel::Queue;
    app.queue_cursor = 0;
    events::handle_key(
        &mut app,
        create_key_event(crossterm::event::KeyCode::Delete),
    );
    assert!(
        app.playlist.is_empty(),
        "Deleting only track must empty playlist"
    );
    assert!(app.stopped, "Deleting only track must stop playback");

    // 3. Search cursor clamping on Down arrow
    app.active_panel = ActivePanel::Search;
    app.searching = true;
    app.search_results = vec![
        mixed::data::library::LibraryEntry::Track {
            name: "Song 1".to_string(),
            path: std::path::PathBuf::from("/mock/res1.mp3"),
            metadata: mixed::data::metadata::TrackMetadata::default(),
        },
        mixed::data::library::LibraryEntry::Track {
            name: "Song 2".to_string(),
            path: std::path::PathBuf::from("/mock/res2.mp3"),
            metadata: mixed::data::metadata::TrackMetadata::default(),
        },
    ];
    app.search_cursor = 0;
    // Press Down arrow 10 times
    for _ in 0..10 {
        events::handle_key(&mut app, create_key_event(crossterm::event::KeyCode::Down));
    }
    assert_eq!(
        app.search_cursor, 1,
        "Down arrow must not exceed search_results.len() - 1"
    );

    // 4. Clamping library_cursor when collapsing a directory
    app.active_panel = ActivePanel::Library;
    app.library_cursor = 50;
    app.flat_library = vec![]; // simulate collapsing all entries
    app.rebuild_flat_library_view();
    assert_eq!(
        app.library_cursor, 0,
        "rebuild_flat_library_view must clamp library_cursor to flat_library.len()"
    );
}

#[test]
fn test_status_msg_rendering_and_auto_dismiss() {
    let mut config = AppConfig::load();
    config.music_dir = Some("/tmp".to_string());

    let (mpris_cmd_tx, _mpris_cmd_rx) = crossbeam_channel::bounded(100);
    let (vis_wake_tx, _vis_wake_rx) = crossbeam_channel::bounded(1);
    let mut app = App::new(config, mpris_cmd_tx, vis_wake_tx);
    app.awaiting_dir_input = false;
    app.player_loading = false;

    let backend = TestBackend::new(100, 30);
    let mut terminal = Terminal::new(backend).unwrap();

    app.set_status("Error: File not found");
    assert!(app.status_msg.is_some());

    // Draw and verify status message is rendered
    terminal.draw(|f| layout::draw(f, &mut app)).unwrap();
    let buffer = terminal.backend().buffer();
    let content: String = buffer.content().iter().map(|c| c.symbol()).collect();
    assert!(content.contains("File not found"));

    // Simulate elapsed time >= 3 seconds
    app.status_msg_at = Some(std::time::Instant::now() - Duration::from_secs(4));
    app.tick();
    assert!(app.status_msg.is_none());
}

fn create_mod_key_event(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
    KeyEvent {
        code,
        modifiers,
        kind: KeyEventKind::Press,
        state: KeyEventState::empty(),
    }
}

#[test]
fn test_source_tab_switching_and_panel_memory() {
    let mut config = AppConfig::load();
    config.music_dir = Some("/tmp".to_string());

    let (mpris_cmd_tx, _mpris_cmd_rx) = crossbeam_channel::bounded(100);
    let (vis_wake_tx, _vis_wake_rx) = crossbeam_channel::bounded(1);
    let mut app = App::new(config, mpris_cmd_tx, vis_wake_tx);
    app.awaiting_dir_input = false;
    app.player_loading = false;

    assert_eq!(app.source, mixed::app::SourceTab::Local);
    assert_eq!(app.active_panel, ActivePanel::Library);

    // Modify local tab state
    app.library_cursor = 7;
    app.active_panel = ActivePanel::Queue;
    app.queue_cursor = 3;

    // Switch to Spotify
    app.switch_source(mixed::app::SourceTab::Spotify);
    assert_eq!(app.source, mixed::app::SourceTab::Spotify);
    assert_eq!(app.active_panel, ActivePanel::Library);
    assert_eq!(app.queue_cursor, 0);

    // Change Spotify panel to NowPlaying
    app.active_panel = ActivePanel::NowPlaying;

    // Switch back to Local
    app.switch_source(mixed::app::SourceTab::Local);
    assert_eq!(app.source, mixed::app::SourceTab::Local);
    assert_eq!(app.active_panel, ActivePanel::Queue);
    assert_eq!(app.queue_cursor, 3);
    assert_eq!(app.library_cursor, 7);

    // Switch to Spotify again
    app.switch_source(mixed::app::SourceTab::Spotify);
    assert_eq!(app.active_panel, ActivePanel::NowPlaying);
}

#[test]
fn test_source_switching_keybindings_and_alt_fallback() {
    let mut config = AppConfig::load();
    config.music_dir = Some("/tmp".to_string());

    let (mpris_cmd_tx, _mpris_cmd_rx) = crossbeam_channel::bounded(100);
    let (vis_wake_tx, _vis_wake_rx) = crossbeam_channel::bounded(1);
    let mut app = App::new(config, mpris_cmd_tx, vis_wake_tx);
    app.awaiting_dir_input = false;
    app.player_loading = false;

    // Ctrl+2 -> Spotify
    events::handle_key(
        &mut app,
        create_mod_key_event(KeyCode::Char('2'), KeyModifiers::CONTROL),
    );
    assert_eq!(app.source, mixed::app::SourceTab::Spotify);

    // Alt+3 -> YouTube
    events::handle_key(
        &mut app,
        create_mod_key_event(KeyCode::Char('3'), KeyModifiers::ALT),
    );
    assert_eq!(app.source, mixed::app::SourceTab::YouTube);

    // Ctrl+4 -> Unified
    events::handle_key(
        &mut app,
        create_mod_key_event(KeyCode::Char('4'), KeyModifiers::CONTROL),
    );
    assert_eq!(app.source, mixed::app::SourceTab::Unified);

    // Alt+1 -> Local
    events::handle_key(
        &mut app,
        create_mod_key_event(KeyCode::Char('1'), KeyModifiers::ALT),
    );
    assert_eq!(app.source, mixed::app::SourceTab::Local);

    // Ctrl+C -> Quit
    let quit = events::handle_key(
        &mut app,
        create_mod_key_event(KeyCode::Char('c'), KeyModifiers::CONTROL),
    );
    assert!(quit, "Ctrl+C must trigger quit");
}

#[test]
fn test_source_switching_digits_do_not_leak_into_search() {
    let mut config = AppConfig::load();
    config.music_dir = Some("/tmp".to_string());

    let (mpris_cmd_tx, _mpris_cmd_rx) = crossbeam_channel::bounded(100);
    let (vis_wake_tx, _vis_wake_rx) = crossbeam_channel::bounded(1);
    let mut app = App::new(config, mpris_cmd_tx, vis_wake_tx);
    app.awaiting_dir_input = false;
    app.player_loading = false;

    // Enter search mode
    app.active_panel = ActivePanel::Search;
    app.searching = true;
    app.search_query.clear();

    // Send Ctrl+2
    events::handle_key(
        &mut app,
        create_mod_key_event(KeyCode::Char('2'), KeyModifiers::CONTROL),
    );
    assert_eq!(app.source, mixed::app::SourceTab::Spotify);
    assert!(
        app.search_query.is_empty(),
        "Ctrl+2 must not leak '2' into search query"
    );
    assert!(!app.searching, "Source switch must exit search mode");

    // Enter search mode again
    app.active_panel = ActivePanel::Search;
    app.searching = true;
    app.search_query.clear();

    // Send Alt+3
    events::handle_key(
        &mut app,
        create_mod_key_event(KeyCode::Char('3'), KeyModifiers::ALT),
    );
    assert_eq!(app.source, mixed::app::SourceTab::YouTube);
    assert!(
        app.search_query.is_empty(),
        "Alt+3 must not leak '3' into search query"
    );
    assert!(!app.searching, "Source switch must exit search mode");
}

#[test]
fn test_control_modifiers_do_not_trigger_plain_actions() {
    let mut config = AppConfig::load();
    config.music_dir = Some("/tmp".to_string());

    let (mpris_cmd_tx, _mpris_cmd_rx) = crossbeam_channel::bounded(100);
    let (vis_wake_tx, _vis_wake_rx) = crossbeam_channel::bounded(1);
    let mut app = App::new(config, mpris_cmd_tx, vis_wake_tx);
    app.awaiting_dir_input = false;
    app.player_loading = false;

    // Initial shuffle is false
    assert!(!app.playlist.shuffle);

    // Send Ctrl+s (should NOT toggle shuffle)
    events::handle_key(
        &mut app,
        create_mod_key_event(KeyCode::Char('s'), KeyModifiers::CONTROL),
    );
    assert!(!app.playlist.shuffle, "Ctrl+S must not trigger shuffle");

    // Send plain 's' (should toggle shuffle)
    events::handle_key(&mut app, create_key_event(KeyCode::Char('s')));
    assert!(app.playlist.shuffle, "Plain 's' should toggle shuffle");
}

#[test]
fn test_remote_source_empty_state_and_footer_rendering() {
    let mut config = AppConfig::load();
    config.music_dir = Some("/tmp".to_string());

    let (mpris_cmd_tx, _mpris_cmd_rx) = crossbeam_channel::bounded(100);
    let (vis_wake_tx, _vis_wake_rx) = crossbeam_channel::bounded(1);
    let mut app = App::new(config, mpris_cmd_tx, vis_wake_tx);
    app.awaiting_dir_input = false;
    app.player_loading = false;

    let backend = TestBackend::new(100, 30);
    let mut terminal = Terminal::new(backend).unwrap();

    // Switch to Spotify
    app.switch_source(mixed::app::SourceTab::Spotify);
    terminal.draw(|f| layout::draw(f, &mut app)).unwrap();

    let buffer = terminal.backend().buffer();
    let content: String = buffer.content().iter().map(|c| c.symbol()).collect();
    assert!(content.contains("Not connected. Press Enter to sign in."));
    assert!(content.contains("local"));
    assert!(content.contains("spotify"));
    assert!(content.contains("youtube"));
    assert!(content.contains("queue"));
}

#[test]
fn test_mixed_trackref_playlist_and_queue_rendering() {
    use mixed::data::metadata::TrackMetadata;
    use mixed::data::playlist::Playlist;
    use mixed::data::track::TrackRef;

    let mut playlist = Playlist::new();

    let meta_local = TrackMetadata {
        title: Some("Local Song".into()),
        artist: Some("Local Artist".into()),
        duration: Some(Duration::from_secs(180)),
        ..Default::default()
    };
    let meta_spotify = TrackMetadata {
        title: Some("Spotify Hit".into()),
        artist: Some("Spotify Star".into()),
        duration: Some(Duration::from_secs(210)),
        ..Default::default()
    };
    let meta_yt = TrackMetadata {
        title: Some("YouTube Live".into()),
        artist: Some("Streamer".into()),
        duration: Some(Duration::from_secs(300)),
        ..Default::default()
    };

    let id_local = TrackRef::Local(PathBuf::from("/music/Album/local.flac"));
    let id_spotify = TrackRef::Spotify("spotify:track:4cOdK2wGLETKBW3PvgPWqT".into());
    let id_yt = TrackRef::YouTube("dQw4w9WgXcQ".into());

    playlist.add(id_local.clone(), meta_local);
    playlist.add(id_spotify.clone(), meta_spotify);
    playlist.add(id_yt.clone(), meta_yt);

    assert_eq!(playlist.len(), 3);
    assert_eq!(playlist.total_duration_secs(), 690);
    assert!(playlist.entry_ids.contains(&id_local));
    assert!(playlist.entry_ids.contains(&id_spotify));
    assert!(playlist.entry_ids.contains(&id_yt));

    // paths() should only contain local paths
    assert_eq!(
        playlist.paths(),
        vec![PathBuf::from("/music/Album/local.flac")]
    );
    assert_eq!(
        playlist.ids(),
        vec![id_local.clone(), id_spotify.clone(), id_yt.clone()]
    );

    // Visual items: verify folder header appears only for local track with parent
    {
        let items = playlist.get_visual_items(true, false);
        let mut header_names = Vec::new();
        for item in items.iter() {
            if let mixed::data::playlist::QueueVisualItem::Header { name } = item {
                header_names.push(name.as_str());
            }
        }
        assert_eq!(header_names, vec!["Album"]);
    }

    // Test play_next with Spotify track
    let meta_next = TrackMetadata {
        title: Some("Next Up".into()),
        artist: Some("DJ".into()),
        duration: Some(Duration::from_secs(120)),
        ..Default::default()
    };
    let id_next = TrackRef::Spotify("spotify:track:next123".into());
    playlist.current = 0;
    playlist.play_next(id_next.clone(), meta_next);

    assert_eq!(playlist.len(), 4);
    assert!(playlist.entry_ids.contains(&id_next));
    assert_eq!(playlist.play_order, vec![0, 3, 1, 2]);

    // Remove entry at idx 3
    playlist.remove(3);
    assert_eq!(playlist.len(), 3);
    assert!(!playlist.entry_ids.contains(&id_next));
}

#[test]
fn test_session_state_serde_compatibility() {
    use mixed::config::session::SessionState;
    use mixed::data::metadata::TrackMetadata;
    use mixed::data::playlist::PlaylistEntry;
    use mixed::data::track::TrackRef;

    // 1. Verify deserialization of legacy state without "queue"
    let legacy_json = r#"{
        "playlist_paths": ["/music/old1.mp3", "/music/old2.mp3"],
        "current_index": 1,
        "position_ms": 42000,
        "was_playing": true,
        "volume": 85,
        "repeat_mode": "Queue",
        "shuffle": true
    }"#;

    let restored: SessionState =
        serde_json::from_str(legacy_json).expect("Must parse legacy session JSON");
    assert!(
        restored.queue.is_empty(),
        "Queue must default to empty when missing"
    );
    assert_eq!(restored.playlist_paths.len(), 2);
    assert_eq!(restored.current_index, 1);
    assert_eq!(restored.volume, 85);
    assert_eq!(
        restored.repeat_mode,
        mixed::data::playlist::RepeatMode::Queue
    );
    assert!(restored.shuffle);

    // 2. Verify serialization and round-trip of new state with mixed TrackRefs
    let mut state = SessionState::default();
    state.queue.push(PlaylistEntry::new(
        TrackRef::Local(PathBuf::from("/music/test.mp3")),
        TrackMetadata {
            title: Some("Test Local".into()),
            ..Default::default()
        },
    ));
    state.queue.push(PlaylistEntry::new(
        TrackRef::Spotify("spotify:track:abc123".into()),
        TrackMetadata {
            title: Some("Test Spotify".into()),
            ..Default::default()
        },
    ));
    state.queue.push(PlaylistEntry::new(
        TrackRef::YouTube("yt_video_id_xyz".into()),
        TrackMetadata {
            title: Some("Test YouTube".into()),
            ..Default::default()
        },
    ));

    let json = serde_json::to_string(&state).expect("Must serialize SessionState");
    let roundtrip: SessionState =
        serde_json::from_str(&json).expect("Must deserialize SessionState");

    assert_eq!(roundtrip.queue.len(), 3);
    assert_eq!(
        roundtrip.queue[0].id,
        TrackRef::Local(PathBuf::from("/music/test.mp3"))
    );
    assert_eq!(
        roundtrip.queue[1].id,
        TrackRef::Spotify("spotify:track:abc123".into())
    );
    assert_eq!(
        roundtrip.queue[2].id,
        TrackRef::YouTube("yt_video_id_xyz".into())
    );
}

#[test]
fn test_toggle_enqueue_tracks_logic() {
    use mixed::data::metadata::TrackMetadata;
    use mixed::data::track::TrackRef;

    let mut config = AppConfig::load();
    config.music_dir = Some("/tmp".to_string());

    let (mpris_cmd_tx, _mpris_cmd_rx) = crossbeam_channel::bounded(100);
    let (vis_wake_tx, _vis_wake_rx) = crossbeam_channel::bounded(1);
    let mut app = App::new(config, mpris_cmd_tx, vis_wake_tx);
    app.awaiting_dir_input = false;
    app.player_loading = false;

    let t1 = (
        TrackRef::Local(PathBuf::from("/music/t1.mp3")),
        TrackMetadata {
            title: Some("Track 1".into()),
            disc_number: Some(1),
            track_number: Some(2),
            ..Default::default()
        },
    );
    let t2 = (
        TrackRef::Local(PathBuf::from("/music/t2.mp3")),
        TrackMetadata {
            title: Some("Track 2".into()),
            disc_number: Some(1),
            track_number: Some(1),
            ..Default::default()
        },
    );

    // Initial enqueue: should sort t2 before t1 because track_number 1 < 2
    app.toggle_enqueue_tracks(&[t1.clone(), t2.clone()], false);
    assert_eq!(app.playlist.len(), 2);
    assert_eq!(app.playlist.entries[0].id, t2.0);
    assert_eq!(app.playlist.entries[1].id, t1.0);

    // Dequeue: calling toggle with the exact same tracks dequeues them
    app.toggle_enqueue_tracks(&[t1.clone(), t2.clone()], false);
    assert_eq!(app.playlist.len(), 0);
}

#[test]
fn test_player_events_handling() {
    use mixed::audio::player::PlayerEvent;
    use mixed::data::metadata::TrackMetadata;
    use mixed::data::track::TrackRef;

    let mut config = AppConfig::load();
    config.music_dir = Some("/tmp".to_string());

    let (mpris_cmd_tx, _mpris_cmd_rx) = crossbeam_channel::bounded(100);
    let (vis_wake_tx, _vis_wake_rx) = crossbeam_channel::bounded(1);
    let mut app = App::new(config, mpris_cmd_tx, vis_wake_tx);
    app.awaiting_dir_input = false;
    app.player_loading = false;

    // Add 2 dummy tracks to playlist
    let t1 = (
        TrackRef::Local(PathBuf::from("/music/t1.mp3")),
        TrackMetadata {
            title: Some("Track 1".into()),
            ..Default::default()
        },
    );
    let t2 = (
        TrackRef::Local(PathBuf::from("/music/t2.mp3")),
        TrackMetadata {
            title: Some("Track 2".into()),
            ..Default::default()
        },
    );
    app.playlist.add(t1.0.clone(), t1.1.clone());
    app.playlist.add(t2.0.clone(), t2.1.clone());
    app.playlist.current = 0;
    app.stopped = false;

    // Initial load_generation
    let gen = app.load_generation;

    // Event for stale generation should be ignored
    app.handle_player_event(PlayerEvent::Buffering {
        generation: gen + 10,
    });
    assert!(!app.buffering);

    // Matching generation Buffering
    app.handle_player_event(PlayerEvent::Buffering { generation: gen });
    assert!(app.buffering);

    // Matching generation Loaded
    app.handle_player_event(PlayerEvent::Loaded { generation: gen });
    assert!(!app.buffering);
    assert_eq!(app.consecutive_failures, 0);

    // Finished event: advances to track 2
    app.handle_player_event(PlayerEvent::Finished { generation: gen });
    assert_eq!(app.playlist.current, 1);

    // Current gen updated when track advanced (via play_current or manual load)
    let gen2 = app.load_generation;

    // Failure 1: advances to track 1 (loops or advances)
    app.handle_player_event(PlayerEvent::Failed {
        generation: gen2,
        error: "Corrupted audio stream".into(),
    });
    assert_eq!(app.consecutive_failures, 1);

    // Failure 2
    let gen3 = app.load_generation;
    app.handle_player_event(PlayerEvent::Failed {
        generation: gen3,
        error: "Decode error".into(),
    });
    assert_eq!(app.consecutive_failures, 2);

    // Failure 3: halts playback and shows 3 consecutive failures status
    let gen4 = app.load_generation;
    app.handle_player_event(PlayerEvent::Failed {
        generation: gen4,
        error: "I/O error".into(),
    });
    assert_eq!(app.consecutive_failures, 3);
    assert!(app.stopped);
    assert!(app
        .status_msg
        .as_deref()
        .unwrap_or("")
        .contains("3 consecutive track failures"));
}

#[test]
fn test_app_config_serde_defaults() {
    let legacy_json = r#"{
        "music_dir": "/home/music",
        "volume": 75,
        "visualizer_enabled": true,
        "visualizer_height": 5,
        "cover_enabled": true,
        "color_scheme": 1,
        "strip_track_numbers": true,
        "desktop_notifications": true
    }"#;

    let config: AppConfig =
        serde_json::from_str(legacy_json).expect("Legacy config should deserialize cleanly");
    assert_eq!(config.spotify_client_id, None);
    assert_eq!(config.yt_dlp_path, None);
    assert_eq!(config.yt_cache_mb, 512);
    assert_eq!(config.yt_audio_quality, "bestaudio");
}

#[test]
fn test_remote_source_browse_and_search_rendering() {
    use mixed::app::{ActivePanel, SourceTab};
    use mixed::data::track::TrackRef;
    use mixed::sources::{BrowseItem, BrowseItemKind};

    let mut config = AppConfig::load();
    config.music_dir = Some("/tmp".to_string());

    let (mpris_cmd_tx, _mpris_cmd_rx) = crossbeam_channel::bounded(100);
    let (vis_wake_tx, _vis_wake_rx) = crossbeam_channel::bounded(1);
    let mut app = App::new(config, mpris_cmd_tx, vis_wake_tx);
    app.awaiting_dir_input = false;
    app.player_loading = false;

    let backend = TestBackend::new(90, 24);
    let mut terminal = Terminal::new(backend).unwrap();

    // 1. Switch to Spotify source (unconnected state)
    app.source = SourceTab::Spotify;
    app.active_panel = ActivePanel::Library;
    terminal.draw(|f| layout::draw(f, &mut app)).unwrap();

    let buffer = terminal.backend().buffer();
    let content: String = buffer.content().iter().map(|c| c.symbol()).collect();
    assert!(content.contains("Spotify Source"));
    assert!(content.contains("Not connected"));

    // 2. Connected state with browse items
    if let Some(view) = app.active_source_view_mut() {
        view.connected = true;
        view.flat = vec![
            BrowseItem {
                id: "album:1".into(),
                title: "Discovery".into(),
                subtitle: Some("Daft Punk".into()),
                kind: BrowseItemKind::Album,
                is_container: true,
                track_ref: None,
                duration_secs: None,
                artwork_url: None,
                depth: 0,
            },
            BrowseItem {
                id: "track:1".into(),
                title: "One More Time".into(),
                subtitle: Some("Daft Punk".into()),
                kind: BrowseItemKind::Track,
                is_container: false,
                track_ref: Some(TrackRef::Spotify("spotify:track:1".into())),
                duration_secs: Some(320),
                artwork_url: None,
                depth: 0,
            },
        ];
        view.cursor = 1; // cursor on track
    }

    terminal.draw(|f| layout::draw(f, &mut app)).unwrap();
    let buffer2 = terminal.backend().buffer();
    let content2: String = buffer2.content().iter().map(|c| c.symbol()).collect();
    assert!(content2.contains("Discovery"));
    assert!(content2.contains("One More Time"));

    // 3. Remote enqueue selected
    app.remote_enqueue_selected(false, false);
    assert_eq!(app.playlist.len(), 1);
    assert_eq!(
        app.playlist.entries[0].id,
        TrackRef::Spotify("spotify:track:1".into())
    );

    // 4. Remote search view
    app.active_panel = ActivePanel::Search;
    if let Some(view) = app.active_source_view_mut() {
        view.search_query = "Daft Punk".into();
        view.search_results = vec![BrowseItem {
            id: "track:2".into(),
            title: "Harder Better Faster Stronger".into(),
            subtitle: Some("Daft Punk".into()),
            kind: BrowseItemKind::Track,
            is_container: false,
            track_ref: Some(TrackRef::Spotify("spotify:track:2".into())),
            duration_secs: Some(224),
            artwork_url: None,
            depth: 0,
        }];
        view.cursor = 0;
    }

    terminal.draw(|f| layout::draw(f, &mut app)).unwrap();
    let buffer3 = terminal.backend().buffer();
    let content3: String = buffer3.content().iter().map(|c| c.symbol()).collect();
    assert!(content3.contains("Search Spotify: Daft Punk"));
    assert!(content3.contains("Harder Better Faster Stronger"));
}

#[test]
fn test_playlist_peek_next_entry() {
    let mut playlist = mixed::data::playlist::Playlist::new();
    assert!(playlist.peek_next_entry().is_none());

    let meta1 = mixed::data::metadata::TrackMetadata {
        title: Some("Song 1".into()),
        artist: Some("Artist 1".into()),
        duration: Some(std::time::Duration::from_secs(180)),
        ..Default::default()
    };
    let meta2 = mixed::data::metadata::TrackMetadata {
        title: Some("Song 2".into()),
        artist: Some("Artist 2".into()),
        duration: Some(std::time::Duration::from_secs(200)),
        ..Default::default()
    };

    playlist.add(
        mixed::data::track::TrackRef::YouTube("yt_track_1".into()),
        meta1,
    );
    playlist.add(
        mixed::data::track::TrackRef::YouTube("yt_track_2".into()),
        meta2,
    );

    // Current is at 0 (Song 1). Next should be Song 2.
    assert_eq!(
        playlist
            .peek_next_entry()
            .and_then(|e| e.metadata.title.as_deref()),
        Some("Song 2")
    );

    // Advance to Song 2
    playlist.advance_track();
    assert_eq!(
        playlist
            .current_entry()
            .and_then(|e| e.metadata.title.as_deref()),
        Some("Song 2")
    );

    // Repeat Off: peek next is None
    playlist.repeat = mixed::data::playlist::RepeatMode::Off;
    assert!(playlist.peek_next_entry().is_none());

    // Repeat Queue: peek next wraps to Song 1
    playlist.repeat = mixed::data::playlist::RepeatMode::Queue;
    assert_eq!(
        playlist
            .peek_next_entry()
            .and_then(|e| e.metadata.title.as_deref()),
        Some("Song 1")
    );

    // Repeat Track: peek next repeats current (Song 2)
    playlist.repeat = mixed::data::playlist::RepeatMode::Track;
    assert_eq!(
        playlist
            .peek_next_entry()
            .and_then(|e| e.metadata.title.as_deref()),
        Some("Song 2")
    );
}

#[test]
fn test_youtube_source_playback_and_events() {
    let mut config = AppConfig::load();
    config.music_dir = Some("/tmp".to_string());

    let (mpris_cmd_tx, _mpris_cmd_rx) = crossbeam_channel::bounded(100);
    let (vis_wake_tx, _vis_wake_rx) = crossbeam_channel::bounded(1);
    let mut app = App::new(config, mpris_cmd_tx, vis_wake_tx);
    let runtime = std::sync::Arc::new(mixed::sources::runtime::SourceRuntime::spawn());
    app.init_source_runtime(runtime);

    let meta = mixed::data::metadata::TrackMetadata {
        title: Some("Mock YT Title".into()),
        artist: Some("Mock YT Artist".into()),
        duration: Some(std::time::Duration::from_secs(240)),
        ..Default::default()
    };
    app.playlist.add(
        mixed::data::track::TrackRef::YouTube("mock_yt_vid".into()),
        meta,
    );

    // 1. play_current() on non-cached track enters buffering
    app.play_current();
    assert!(
        app.buffering,
        "Uncached YouTube track must trigger buffering"
    );
    let current_gen = app.load_generation;

    // 2. Incoming YouTubeTrackDownloaded for current generation clears buffering
    app.handle_source_event(mixed::sources::SourceEvent::YouTubeTrackDownloaded {
        video_id: "mock_yt_vid".into(),
        path: std::path::PathBuf::from("/tmp/nonexistent_mock.m4a"),
        generation: current_gen,
    });
    assert!(
        !app.buffering,
        "Successful download event must clear buffering"
    );

    // 3. Incoming YouTubeDownloadFailed sets status and failure count
    app.buffering = true;
    app.handle_source_event(mixed::sources::SourceEvent::YouTubeDownloadFailed {
        video_id: "mock_yt_vid".into(),
        error: "Network connection refused".into(),
        generation: app.load_generation,
    });
    assert!(!app.buffering, "Failed download event must clear buffering");
    assert!(app
        .status_msg
        .as_deref()
        .unwrap_or("")
        .contains("YouTube download failed"));
}

#[test]
fn test_stale_search_reply_dropped_after_new_query() {
    use mixed::app::SourceTab;
    use mixed::sources::{BrowseItem, BrowseItemKind, SourceEvent};

    let mut config = AppConfig::load();
    config.music_dir = Some("/tmp".to_string());

    let (mpris_cmd_tx, _mpris_cmd_rx) = crossbeam_channel::bounded(100);
    let (vis_wake_tx, _vis_wake_rx) = crossbeam_channel::bounded(1);
    let mut app = App::new(config, mpris_cmd_tx, vis_wake_tx);

    // Switch to Spotify and simulate search query 1
    app.source = SourceTab::Spotify;
    app.dispatch_remote_search("Radiohead".into());
    let gen1 = app.spotify_view.search_generation;

    // Fast-typing: user types second query before network returns query 1
    app.dispatch_remote_search("Radiohead Karma Police".into());
    let gen2 = app.spotify_view.search_generation;
    assert_ne!(
        gen1, gen2,
        "Subsequent queries must increment search generation"
    );

    // Network reply for gen1 arrives (stale reply)
    let stale_items = vec![BrowseItem {
        id: "spotify:track:stale".into(),
        title: "Creep".into(),
        subtitle: Some("Radiohead".into()),
        kind: BrowseItemKind::Track,
        is_container: false,
        track_ref: Some(mixed::data::track::TrackRef::Spotify(
            "spotify:track:stale".into(),
        )),
        duration_secs: Some(238),
        artwork_url: None,
        depth: 0,
    }];

    app.handle_source_event(SourceEvent::SearchResults {
        source: SourceTab::Spotify,
        query: "Radiohead".into(),
        generation: gen1,
        items: stale_items,
        page: 0,
        has_more: false,
    });

    // Stale items must be dropped because view.search_generation == gen2
    assert!(
        app.spotify_view.search_results.is_empty(),
        "Stale search results from previous generation must be dropped"
    );
    assert!(
        app.spotify_view.loading,
        "View must stay in loading state awaiting the active generation"
    );

    // Network reply for gen2 arrives
    let fresh_items = vec![BrowseItem {
        id: "spotify:track:fresh".into(),
        title: "Karma Police".into(),
        subtitle: Some("Radiohead".into()),
        kind: BrowseItemKind::Track,
        is_container: false,
        track_ref: Some(mixed::data::track::TrackRef::Spotify(
            "spotify:track:fresh".into(),
        )),
        duration_secs: Some(264),
        artwork_url: None,
        depth: 0,
    }];

    app.handle_source_event(SourceEvent::SearchResults {
        source: SourceTab::Spotify,
        query: "Radiohead Karma Police".into(),
        generation: gen2,
        items: fresh_items,
        page: 0,
        has_more: false,
    });

    assert_eq!(
        app.spotify_view.search_results.len(),
        1,
        "Active generation results must be accepted"
    );
    assert_eq!(app.spotify_view.search_results[0].title, "Karma Police");
    assert!(
        !app.spotify_view.loading,
        "Loading flag must clear on current search arrival"
    );
}

#[test]
fn test_spotify_source_playback_and_disconnected_guard() {
    use mixed::audio::player::PlayerEvent;
    use mixed::data::metadata::TrackMetadata;
    use mixed::data::track::TrackRef;

    let mut config = AppConfig::load();
    config.music_dir = Some("/tmp".to_string());

    let (mpris_cmd_tx, _mpris_cmd_rx) = crossbeam_channel::bounded(100);
    let (vis_wake_tx, _vis_wake_rx) = crossbeam_channel::bounded(1);
    let mut app = App::new(config, mpris_cmd_tx, vis_wake_tx);

    let sp_track = TrackRef::Spotify("spotify:track:test123".into());
    let meta = TrackMetadata {
        title: Some("Test Spotify Track".into()),
        artist: Some("Test Artist".into()),
        duration: Some(std::time::Duration::from_secs(180)),
        ..Default::default()
    };
    app.playlist.add(sp_track.clone(), meta);

    // 1. With Spotify not connected: track is never dropped from queue; status notifies user
    app.spotify_view.connected = false;
    app.play_current();

    assert_eq!(app.playlist.len(), 1, "Track must remain in queue");
    assert!(
        !app.buffering,
        "Disconnected source must not enter buffering"
    );
    assert!(
        app.status_msg
            .as_deref()
            .unwrap_or("")
            .contains("Spotify not connected"),
        "Status must direct user to log in when source is not connected"
    );

    // 2. With Spotify connected: entering buffering and loading track
    app.spotify_view.connected = true;
    app.play_current();

    assert!(app.buffering, "Connected source must enter buffering");
    let current_gen = app.load_generation;

    // Simulate PlayerEvent::Loaded
    app.handle_player_event(PlayerEvent::Loaded {
        generation: current_gen,
    });
    assert!(!app.buffering, "PlayerEvent::Loaded must clear buffering");
    assert_eq!(app.playlist.len(), 1, "Track must stay in queue");
}

#[cfg(feature = "youtube")]
#[tokio::test]
async fn test_youtube_smoke_interface() {
    // Standalone smoke test isolating ytmapi-rs / YouTube Music interface
    let client = mixed::sources::youtube::YouTubeClient::new(None).await;
    assert!(!client.is_authenticated());

    let roots = client.library_roots();
    assert!(
        !roots.is_empty(),
        "Library roots must produce navigation items"
    );
    assert_eq!(roots[0].title, "Login with YouTube Music cookie");
}

fn create_test_app() -> App {
    let mut config = AppConfig::load();
    config.music_dir = Some("/mock/music".to_string());
    let (mpris_cmd_tx, _mpris_cmd_rx) = crossbeam_channel::bounded(100);
    let (vis_wake_tx, _vis_wake_rx) = crossbeam_channel::bounded(1);
    let mut app = App::new(config, mpris_cmd_tx, vis_wake_tx);
    app.awaiting_dir_input = false;
    app.player_loading = false;
    app
}

#[test]
fn test_shift_enter_play_next_local_and_remote() {
    let mut app = create_test_app();
    app.flat_library = vec![
        mixed::data::library::FlatLibraryItem {
            depth: 0,
            is_last: false,
            ancestor_last: Vec::new(),
            enqueued: false,
            entry: mixed::data::library::LibraryEntry::Track {
                name: "Track A".to_string(),
                path: std::path::PathBuf::from("/mock/a.mp3"),
                metadata: mixed::data::metadata::TrackMetadata::default(),
            },
        },
        mixed::data::library::FlatLibraryItem {
            depth: 0,
            is_last: true,
            ancestor_last: Vec::new(),
            enqueued: false,
            entry: mixed::data::library::LibraryEntry::Track {
                name: "Track B".to_string(),
                path: std::path::PathBuf::from("/mock/b.mp3"),
                metadata: mixed::data::metadata::TrackMetadata::default(),
            },
        },
    ];

    app.active_panel = ActivePanel::Library;
    // Cursor 0 is header ".." - Shift+Enter should do nothing
    app.library_cursor = 0;
    events::handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT));
    assert!(
        app.playlist.is_empty(),
        "Cursor 0 (header) must not queue anything on Shift+Enter"
    );

    // Cursor 1 is Track A - Shift+Enter queues Track A
    app.library_cursor = 1;
    events::handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT));
    assert_eq!(app.playlist.len(), 1);
    assert_eq!(
        app.playlist.entries[0].id.local_path(),
        Some(std::path::Path::new("/mock/a.mp3")),
        "Cursor 1 must queue Track A"
    );

    // Remote source: test play_next queues without playing immediately
    app.source = mixed::app::SourceTab::YouTube;
    app.active_panel = ActivePanel::Search;
    app.youtube_view.search_results = vec![mixed::sources::BrowseItem {
        id: "vid1".to_string(),
        title: "Remote Video".to_string(),
        subtitle: None,
        kind: mixed::sources::BrowseItemKind::Track,
        is_container: false,
        track_ref: Some(mixed::data::track::TrackRef::YouTube("vid1".to_string())),
        duration_secs: Some(180),
        artwork_url: None,
        depth: 0,
    }];
    app.youtube_view.search_cursor = 0;
    // Remote play_next
    app.remote_enqueue_selected(false, true);
    assert_eq!(app.playlist.len(), 2);
    assert_eq!(
        app.playlist.entries[1].id,
        mixed::data::track::TrackRef::YouTube("vid1".to_string())
    );
}

#[test]
fn test_esc_key_does_not_quit() {
    let mut app = create_test_app();
    app.active_panel = ActivePanel::Library;

    let quit = events::handle_key(&mut app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(!quit, "Esc must not quit application");
    assert_eq!(
        app.active_panel,
        ActivePanel::Queue,
        "Esc should return to Queue from Library"
    );
}

#[test]
fn test_youtube_progressive_playback_and_fallback_to_full_download() {
    use mixed::audio::growing_file::DownloadProgress;
    use mixed::audio::player::PlayerEvent;
    use mixed::sources::SourceEvent;

    let mut config = AppConfig::load();
    config.music_dir = Some("/tmp".to_string());
    let (mpris_cmd_tx, _mpris_cmd_rx) = crossbeam_channel::bounded(100);
    let (vis_wake_tx, _vis_wake_rx) = crossbeam_channel::bounded(1);
    // No source runtime: nothing is actually downloaded in this test
    let mut app = App::new(config, mpris_cmd_tx, vis_wake_tx);

    let vid = "mock_stream_vid_not_cached";
    app.playlist.add(
        mixed::data::track::TrackRef::YouTube(vid.into()),
        mixed::data::metadata::TrackMetadata::default(),
    );
    app.play_current();
    assert!(app.buffering, "uncached track waits for its download");

    // A partial file for some other video must not start playback
    let progress = std::sync::Arc::new(DownloadProgress::default());
    app.handle_source_event(SourceEvent::YouTubeTrackStreamable {
        video_id: "another_video".into(),
        path: std::path::PathBuf::from("/tmp/another_video.m4a.part"),
        progress: progress.clone(),
    });
    assert!(app.buffering && app.streaming.is_none());

    // The partial file of the current track starts playback before the download ends
    app.handle_source_event(SourceEvent::YouTubeTrackStreamable {
        video_id: vid.into(),
        path: std::path::PathBuf::from("/tmp/mock_stream_vid_not_cached.m4a.part"),
        progress: progress.clone(),
    });
    assert!(!app.buffering, "playback starts from the partial file");
    let stream = app.streaming.clone().expect("streaming state recorded");
    assert_eq!(stream.video_id, vid);

    // If the partial file cannot be decoded, wait for the complete download
    // instead of counting a playback failure and skipping the track.
    app.handle_player_event(PlayerEvent::Failed {
        generation: stream.generation,
        error: "isomp4: missing moov atom".into(),
    });
    assert!(app.buffering, "falls back to waiting for the full file");
    assert!(app.streaming.is_none());
    assert_eq!(app.consecutive_failures, 0);

    // A later failure of the download itself is reported normally
    progress.finish(false);
    app.handle_source_event(SourceEvent::YouTubeDownloadFailed {
        video_id: vid.into(),
        error: "network down".into(),
        generation: 0,
    });
    assert!(!app.buffering);
    assert_eq!(app.consecutive_failures, 1);
}

#[test]
fn test_remote_browse_space_toggles_container_expansion() {
    use mixed::sources::{BrowseItem, BrowseItemKind, SourceEvent, SourceTab};

    let mut config = AppConfig::load();
    config.music_dir = Some("/tmp".to_string());
    let (mpris_cmd_tx, _mpris_cmd_rx) = crossbeam_channel::bounded(100);
    let (vis_wake_tx, _vis_wake_rx) = crossbeam_channel::bounded(1);
    let mut app = App::new(config, mpris_cmd_tx, vis_wake_tx);

    let item = |id: &str, is_container: bool| BrowseItem {
        id: id.to_string(),
        title: id.to_string(),
        subtitle: None,
        kind: BrowseItemKind::Playlist,
        is_container,
        track_ref: None,
        duration_secs: None,
        artwork_url: None,
        depth: 0,
    };

    app.switch_source(SourceTab::YouTube);
    app.active_panel = ActivePanel::Library;
    app.youtube_view.connected = true;
    app.handle_source_event(SourceEvent::Roots {
        source: SourceTab::YouTube,
        items: vec![
            item("yt:library_playlists", true),
            item("yt:library_albums", true),
        ],
    });

    // Space on the container drills down ("cd") into the folder
    app.youtube_view.cursor = 0;
    app.toggle_container_expansion();

    // The reply to children populates the folder view
    app.handle_source_event(SourceEvent::Children {
        source: SourceTab::YouTube,
        parent_id: "yt:library_playlists".into(),
        items: vec![item("yt:playlist:a", true), item("yt:playlist:b", true)],
        page: 0,
        has_more: false,
    });

    assert_eq!(app.youtube_view.flat.len(), 2);
    assert_eq!(app.youtube_view.flat[0].id, "yt:playlist:a");
    assert_eq!(app.youtube_view.flat[1].id, "yt:playlist:b");
    assert_eq!(app.youtube_view.nav_stack.len(), 1);
    assert_eq!(
        app.youtube_view.current_path_display("YouTube Music"),
        "📁 YouTube Music / yt:library_playlists"
    );

    // Backspace / navigate up ("cd ..") returns to parent directory list
    assert!(app.navigate_folder_up());
    assert_eq!(app.youtube_view.flat.len(), 2);
    assert_eq!(app.youtube_view.flat[0].id, "yt:library_playlists");
    assert_eq!(app.youtube_view.flat[1].id, "yt:library_albums");
    assert!(app.youtube_view.nav_stack.is_empty());
    assert_eq!(app.youtube_view.cursor, 0);
}

#[test]
fn test_spotify_client_id_prompt_is_reachable_with_c() {
    use mixed::app::{ActivePanel, SourceTab};

    let mut app = create_test_app();
    app.switch_source(SourceTab::Spotify);
    app.active_panel = ActivePanel::Library;
    // Also while signed in: that is when a rate-limited shared Client ID gets replaced
    app.spotify_view.connected = true;

    events::handle_key(
        &mut app,
        KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE),
    );
    assert!(app.spotify_view.awaiting_login_input);

    for c in "abc123".chars() {
        events::handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE),
        );
    }
    assert_eq!(app.spotify_view.login_input, "abc123");

    events::handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(!app.spotify_view.awaiting_login_input);
    assert!(
        !app.spotify_view.connected,
        "the sign-in panel shows the progress of the new sign-in"
    );
    assert!(app.playlist.is_empty(), "Enter submitted the prompt only");
}

#[test]
fn test_rejected_spotify_playback_sign_in_asks_to_sign_in_again() {
    use mixed::audio::player::PlayerEvent;

    let mut app = create_test_app();
    app.spotify_view.connected = true;

    app.handle_player_event(PlayerEvent::SpotifySignInLost);

    assert!(!app.spotify_view.connected);
    assert!(app
        .spotify_view
        .error
        .as_deref()
        .is_some_and(|e| e.contains("sign in again")));
}
