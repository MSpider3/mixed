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
use mixed::sys::MediaCommand;
use mixed::ui::events;
use mixed::ui::layout;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(target_os = "linux")]
    if std::env::var("MIXED_DEBUG").is_err() {
        silence_alsa();
    }

    // Install panic hook BEFORE raw mode so any crash restores the terminal cleanly.
    // Without this, a panic leaves raw mode active and the cursor hidden.
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = crossterm::terminal::disable_raw_mode();
        let _ = crossterm::execute!(
            std::io::stdout(),
            crossterm::terminal::LeaveAlternateScreen,
            crossterm::event::DisableMouseCapture,
            crossterm::cursor::Show,  // restore cursor visibility
        );
        original_hook(info);
    }));

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

    // Set up crossbeam event channels for media commands (MPRIS on Linux, termux on Android)
    let (media_cmd_tx, media_cmd_rx) = crossbeam_channel::unbounded::<MediaCommand>();

    // Bounded(1) wake-up channel: the FFT visualizer thread fires try_send() after
    // each 34 ms spectrum frame. The main loop receives and repaints at ~30 fps
    // when music is playing, then drops back to 0% CPU when paused.
    let (vis_wake_tx, vis_wake_rx) = crossbeam_channel::bounded::<()>(1);

    // Create app
    let mut app = App::new(config, media_cmd_tx, vis_wake_tx);

    // Try to restore previous session (deferred if player_loading; handled in select! loop)
    // Note: Actually deferred now, will load when player finishes initializing

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
        // 250ms tick: sufficient for progress bar updates (4fps) while cutting
        // idle CPU usage from ~15% (34ms) down to <1%.
        std::thread::sleep(Duration::from_millis(250));
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
    let never_player_rx = crossbeam_channel::never::<mixed::audio::player::Player>();
    // Before the player arrives vis_wake_rx is the live channel; we always select on it.
    // After player init it stays live — the FFT thread keeps sending.

    while running.load(std::sync::atomic::Ordering::Relaxed) {
        // Clear terminal to wipe old Sixel graphic layers if panel or track changed
        let current_panel = app.active_panel;
        let current_track_path = app.playlist.current_entry().map(|e| &e.path);
        let mut force_clear = false;

        if current_panel != last_panel {
            force_clear = true;
            last_panel = current_panel;
        }
        if current_track_path != last_track_path.as_ref() {
            force_clear = true;
            last_track_path = current_track_path.cloned();
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
        let player_rx = app.player_rx.as_ref().unwrap_or(&never_player_rx);

        crossbeam_channel::select! {
            recv(player_rx) -> player_res => {
                if let Ok(player) = player_res {
                    app.finalize_player_init(player);
                    app.load_session();
                    app.refresh_needed = true;
                }
            }
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
            // Visualizer wake-up: the FFT thread fires this after each spectrum frame
            // (~34 ms). When music is playing this gives the UI ~30 fps for smooth bars;
            // when paused the FFT thread decays to silence and sends less often, so
            // the main loop gracefully idles at near-zero CPU.
            recv(vis_wake_rx) -> _ => {
                app.refresh_needed = true;
            }
            recv(lib_rx) -> res => {
                app.library_rx = None;
                app.library_loading = false;
                if let Ok(lib) = res {
                    app.library = lib;
                    app.rebuild_flat_library();
                    // Persist cache so next launch loads instantly without "Loading Library..."
                    if let Some(ref dir) = app.config.music_dir.clone() {
                        App::save_library_cache(&app.library, dir);
                    }
                }
                app.refresh_needed = true;
            }
            recv(media_cmd_rx) -> cmd => {
                if let Ok(command) = cmd {
                    app.refresh_needed = true;
                    match command {
                        MediaCommand::PlayPause => app.toggle_pause(),
                        MediaCommand::Play => app.play(),
                        MediaCommand::Pause => app.pause(),
                        MediaCommand::Stop => app.stop(),
                        MediaCommand::Next => app.next_track(),
                        MediaCommand::Previous => app.prev_track(),
                        MediaCommand::Seek(offset) => {
                            let current = app.player().map(|p| p.elapsed_ms()).unwrap_or(0);
                            let target = (current as i64 + offset / 1000).max(0) as u64;
                            app.seek(target);
                        }
                        MediaCommand::SetPosition(pos) => {
                            let target = (pos / 1000).max(0) as u64;
                            app.seek(target);
                        }
                        // Writable MPRIS properties — set state then push back to MPRIS
                        MediaCommand::SetLoopStatus(status) => {
                            use mixed::data::playlist::RepeatMode;
                            app.playlist.repeat = match status.as_str() {
                                "Track"    => RepeatMode::Track,
                                "Playlist" => RepeatMode::Queue,
                                _          => RepeatMode::Off,
                            };
                            #[cfg(target_os = "linux")]
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
                        MediaCommand::SetShuffle(on) => {
                            app.playlist.set_shuffle(on);
                            #[cfg(target_os = "linux")]
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
                        MediaCommand::SetVolume(vol_norm) => {
                            // vol_norm is 0.0–1.0; player expects 0–100
                            let vol_u8 = (vol_norm * 100.0).round().clamp(0.0, 100.0) as u8;
                            if let Some(player) = app.player_mut() {
                                player.set_volume(vol_u8);
                            }
                            #[cfg(target_os = "linux")]
                            if let Some(ref mpris) = app.mpris_state {
                                use std::sync::atomic::Ordering;
                                mpris.volume.store(vol_norm.to_bits(), Ordering::Relaxed);
                            }
                            app.trigger_mpris_update();
                        }
                        MediaCommand::Quit => {
                            break;
                        }
                    }
                }
            }
        }

        if let (Some(target), Some(last_input)) = (app.pending_seek, app.last_seek_input) {
            if last_input.elapsed() >= Duration::from_millis(250) {
                app.seek(target.as_millis() as u64);
                app.pending_seek = None;
                app.last_seek_input = None;
                app.refresh_needed = true;
            }
        }
    }

    // Save state before exit
    app.save_state();
    app.config.save();

    // Signal MPRIS background thread to release the D-Bus name gracefully.
    // Dropping update_tx closes the channel; the tokio select loop will also
    // catch the shutdown flag. Both paths converge to dropping the Connection,
    // which releases org.mpris.MediaPlayer2.mixed immediately.
    #[cfg(target_os = "linux")]
    {
        if let Some(ref state) = app.mpris_state {
            state.shutdown.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        // Close the update channel so recv() in the tokio loop returns None
        drop(app.mpris_update_tx.take());
        // Brief wait to let the background thread release the D-Bus name before exit
        std::thread::sleep(Duration::from_millis(100));
    }

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
    print!("\x1b]0;Terminal\x07");
    let _ = std::io::Write::flush(&mut std::io::stdout());

    Ok(())
}

#[cfg(target_os = "linux")]
fn silence_alsa() {
    unsafe {
        let lib = libc::dlopen(c"libasound.so.2".as_ptr(), libc::RTLD_NOW);
        if !lib.is_null() {
            let set_handler = libc::dlsym(lib, c"snd_lib_error_set_handler".as_ptr());
            if !set_handler.is_null() {
                type HandlerFn = unsafe extern "C" fn(*const libc::c_char, libc::c_int, *const libc::c_char, libc::c_int, *const libc::c_char);
                let set_handler_fn: unsafe extern "C" fn(Option<HandlerFn>) -> libc::c_int = std::mem::transmute(set_handler);
                
                unsafe extern "C" fn dummy_handler(
                    _file: *const libc::c_char,
                    _line: libc::c_int,
                    _function: *const libc::c_char,
                    _err: libc::c_int,
                    _fmt: *const libc::c_char,
                ) {}
                
                let _ = set_handler_fn(Some(dummy_handler));
            }
        }
    }
}

