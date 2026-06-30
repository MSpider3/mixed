use std::io;
use std::time::{Duration, Instant};

use crossterm::{
    event::{
        self, DisableFocusChange, DisableMouseCapture, EnableFocusChange, EnableMouseCapture, Event,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};

use mixed::app::App;
use mixed::config::app_config::AppConfig;
use mixed::data::library::LibraryEntry;
use mixed::sys::mpris::MprisCommand;
use mixed::ui::events;
use mixed::ui::layout;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Redirect stderr to null to suppress ALSA underruns/errors from corrupting TUI, unless debugging
    // if std::env::var("MIXED_DEBUG").is_err() {
    //     redirect_stderr_to_null();
    // }

    // Load configuration
    let config = AppConfig::load();

    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(
        stdout,
        EnterAlternateScreen,
        EnableMouseCapture,
        EnableFocusChange
    )?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Set up crossbeam event channels for MPRIS commands
    let (mpris_cmd_tx, mpris_cmd_rx) = crossbeam_channel::unbounded();

    // Create app
    let mut app = App::new(config, mpris_cmd_tx);

    // Try to restore previous session
    app.load_session();

    // Install SIGINT handler for graceful shutdown
    let running = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    {
        let running_clone = running.clone();
        ctrlc::set_handler(move || {
            running_clone.store(false, std::sync::atomic::Ordering::Relaxed);
        })?;
    }

    // Set up crossbeam event channels
    let (event_tx, event_rx) = crossbeam_channel::bounded(100);
    std::thread::spawn(move || loop {
        if let Ok(ev) = event::read() {
            if event_tx.send(ev).is_err() {
                break;
            }
        }
    });

    let (tick_tx, tick_rx) = crossbeam_channel::bounded(1);
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_millis(34));
        if tick_tx.send(()).is_err() {
            break;
        }
    });

    let mut resize_pending = false;
    let mut last_resize_time = Instant::now();

    // Track state to trigger terminal clears (prevent Sixel ghosting)
    let mut last_panel = app.active_panel;
    let mut last_track_path = app.playlist.current_entry().map(|e| e.path.clone());

    let never_rx = crossbeam_channel::never::<Vec<LibraryEntry>>();

    while running.load(std::sync::atomic::Ordering::Relaxed) {
        // Clear terminal to wipe old Sixel graphic layers if panel or track changed
        let current_panel = app.active_panel;
        let current_track_path = app.playlist.current_entry().map(|e| e.path.clone());
        let mut force_clear = false;

        if current_panel != last_panel {
            force_clear = true;
            last_panel = current_panel;
        }
        if current_track_path != last_track_path {
            force_clear = true;
            last_track_path = current_track_path;
        }

        if force_clear {
            terminal.clear()?;
            app.refresh_needed = true;
        }

        if app.refresh_needed && !resize_pending {
            app.refresh_needed = false;
            terminal.draw(|f| layout::draw(f, &mut app))?;
        }

        let lib_rx = app.library_rx.as_ref().unwrap_or(&never_rx);

        crossbeam_channel::select! {
            recv(event_rx) -> ev => {
                if let Ok(event) = ev {
                    match event {
                        Event::Key(key) => {
                            app.refresh_needed = true;
                            if events::handle_key(&mut app, key) {
                                break;
                            }
                        }
                        Event::Mouse(mouse) => {
                            app.refresh_needed = true;
                            events::handle_mouse(&mut app, mouse);
                        }
                        Event::Resize(_, _) => {
                            resize_pending = true;
                            last_resize_time = Instant::now();
                        }
                        Event::FocusGained => {
                            app.terminal_focused = true;
                        }
                        Event::FocusLost => {
                            app.terminal_focused = false;
                        }
                        _ => {}
                    }
                }
            }
            recv(tick_rx) -> _ => {
                app.tick();

                if resize_pending && last_resize_time.elapsed() >= Duration::from_millis(100) {
                    resize_pending = false;
                    app.generate_cover_art_protocol(); // Force reload/scale Sixel image
                    terminal.clear()?; // Clear terminal to force a clean redraw
                    app.refresh_needed = true;
                }
            }
            recv(lib_rx) -> res => {
                if let Ok(lib) = res {
                    app.library = lib;
                    app.rebuild_flat_library();
                    app.library_loading = false;
                    app.library_rx = None;
                    // Persist cache so next launch loads instantly without "Loading Library..."
                    if let Some(ref dir) = app.config.music_dir.clone() {
                        App::save_library_cache(&app.library, dir);
                    }
                    app.refresh_needed = true;
                }
            }
            recv(mpris_cmd_rx) -> cmd => {
                if let Ok(command) = cmd {
                    app.refresh_needed = true;
                    match command {
                        MprisCommand::PlayPause => app.toggle_pause(),
                        MprisCommand::Play => app.play(),
                        MprisCommand::Pause => app.pause(),
                        MprisCommand::Stop => app.stop(),
                        MprisCommand::Next => app.next_track(),
                        MprisCommand::Previous => app.prev_track(),
                        MprisCommand::Seek(offset) => {
                            let current = app.player.elapsed_ms();
                            let target = (current as i64 + offset / 1000).max(0) as u64;
                            app.seek(target);
                        }
                        MprisCommand::SetPosition(pos) => {
                            let target = (pos / 1000).max(0) as u64;
                            app.seek(target);
                        }
                        // Writable MPRIS properties — set state then push back to MPRIS
                        MprisCommand::SetLoopStatus(status) => {
                            use mixed::data::playlist::RepeatMode;
                            app.playlist.repeat = match status.as_str() {
                                "Track"    => RepeatMode::Track,
                                "Playlist" => RepeatMode::Queue,
                                _          => RepeatMode::Off,
                            };
                            if let Some(ref mpris) = app.mpris_state {
                                use std::sync::atomic::Ordering;
                                let loop_val = match app.playlist.repeat {
                                    RepeatMode::Off   => 0,
                                    RepeatMode::Track => 1,
                                    RepeatMode::Queue => 2,
                                };
                                mpris.loop_status.store(loop_val, Ordering::Relaxed);
                            }
                            app.trigger_mpris_update();
                        }
                        MprisCommand::SetShuffle(on) => {
                            app.playlist.set_shuffle(on);
                            if let Some(ref mpris) = app.mpris_state {
                                use std::sync::atomic::Ordering;
                                mpris.shuffle.store(on, Ordering::Relaxed);
                                mpris.can_go_next.store(
                                    app.playlist.can_go_next(),
                                    Ordering::Relaxed,
                                );
                                mpris.can_go_previous.store(
                                    app.playlist.can_go_previous(),
                                    Ordering::Relaxed,
                                );
                            }
                            app.trigger_mpris_update();
                        }
                        MprisCommand::SetVolume(vol_norm) => {
                            // vol_norm is 0.0–1.0; player expects 0–100
                            let vol_u8 = (vol_norm * 100.0).round().clamp(0.0, 100.0) as u8;
                            app.player.set_volume(vol_u8);
                            if let Some(ref mpris) = app.mpris_state {
                                use std::sync::atomic::Ordering;
                                mpris.volume.store(vol_norm.to_bits(), Ordering::Relaxed);
                            }
                            app.trigger_mpris_update();
                        }
                        MprisCommand::Quit => {
                            break;
                        }
                    }
                }
            }
        }
    }

    // Save state before exit
    app.save_state();
    app.config.save();

    // Restore terminal
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture,
        DisableFocusChange
    )?;
    terminal.show_cursor()?;

    // Reset terminal title
    print!("\x1b]0;{}\x07", "Terminal");
    let _ = std::io::Write::flush(&mut std::io::stdout());

    Ok(())
}

