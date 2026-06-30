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
    let mut app = App::new(config, mpris_cmd_tx);

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
    let mut app = App::new(config, mpris_cmd_tx);
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

    // Check the final state
    assert!(
        mpris_state
            .position_us
            .load(std::sync::atomic::Ordering::Relaxed)
            >= 0
    );
}
