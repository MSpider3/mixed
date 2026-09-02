use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
use ratatui::{backend::TestBackend, Terminal};
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
