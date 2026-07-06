use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::{Arc, RwLock};

use crate::audio::player::Player;
use crate::audio::visualizer::{VisualizerEngine, VisualizerMode};
use crate::config::app_config::AppConfig;
use crate::config::session::{self, SessionState};
use crate::data::library::{self, LibraryEntry};
use crate::data::lyrics::{self, LyricsData};
use crate::data::metadata::{self, TrackMetadata};
use crate::data::playlist::{Playlist, RepeatMode};
#[cfg(target_os = "linux")]
use crate::sys::mpris::{self, SharedMprisState};
use crate::sys::MediaCommand;

use fuzzy_matcher::skim::SkimMatcherV2;
use fuzzy_matcher::FuzzyMatcher;

/// Which panel is currently active / focused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivePanel {
    Queue,
    Library,
    NowPlaying,
    Search,
    Help,
}

/// Central application state.
pub struct App {
    // -- Modules --
    pub player: Option<Player>,
    pub playlist: Playlist,
    pub config: AppConfig,
    pub visualizer_bars: Arc<RwLock<Vec<f32>>>,
    pub visualizer_mode: VisualizerMode,
    #[allow(dead_code)]
    pub visualizer_enabled: bool,

    // -- Library --
    pub library: Vec<LibraryEntry>,
    pub flat_library: Vec<library::FlatLibraryItem>,
    /// Full-expanded flat library (all dirs open) — used for search to avoid per-keystroke tree-walk.
    pub full_flat_library: Vec<library::FlatLibraryItem>,
    pub collapsed_dirs: std::collections::HashSet<std::path::PathBuf>,
    pub library_loading: bool,
    pub library_rx: Option<crossbeam_channel::Receiver<Vec<LibraryEntry>>>,

    // -- UI State --
    pub active_panel: ActivePanel,
    pub queue_cursor: usize,
    pub library_cursor: usize,
    pub lyrics_scroll: u16,
    pub show_folders: bool,
    pub refresh_needed: bool,
    pub terminal_focused: bool,
    /// True while the audio engine is still initializing on a background thread.
    /// The keybind router silently swallows playback keys in this state.
    pub player_loading: bool,
    pub player_rx: Option<crossbeam_channel::Receiver<Player>>,

    // -- Search --
    pub searching: bool,
    pub search_query: String,
    pub search_results: Vec<LibraryEntry>,
    pub search_cursor: usize,

    // -- Lyrics --
    pub current_lyrics: Option<LyricsData>,
    pub now_playing_meta: Option<TrackMetadata>,
    pub show_full_lyrics: bool,

    // -- First-run --
    pub awaiting_dir_input: bool,
    pub dir_input: String,

    // -- MPRIS (Linux only) --
    #[cfg(target_os = "linux")]
    pub mpris_state: Option<SharedMprisState>,
    #[cfg(target_os = "linux")]
    pub mpris_update_tx: Option<tokio::sync::mpsc::UnboundedSender<()>>,
    pub pending_mpris_update: bool,
    pub last_mpris_trigger: Option<std::time::Instant>,

    // -- Status message --
    pub status_msg: Option<String>,
    pub stopped: bool,
    pub pending_seek: Option<std::time::Duration>,
    pub last_seek_input: Option<std::time::Instant>,

    // -- Image Picker --
    pub picker: Option<ratatui_image::picker::Picker>,
    pub current_cover_protocol: Option<ratatui_image::protocol::StatefulProtocol>,
    /// Last /tmp cover art PNG path — tracked for cleanup on track change.
    pub last_cover_tmp_path: Option<String>,

    // -- Visualizer wake-up channel --
    /// Non-blocking sender that the FFT thread fires after each spectrum frame
    /// (~34ms). The main select! loop receives on the paired Receiver and sets
    /// refresh_needed = true, giving ~30 fps to the visualizer without tying
    /// the main tick to a 34 ms sleep.
    pub vis_wake_tx: Option<crossbeam_channel::Sender<()>>,
}

impl App {
    pub fn new(
        config: AppConfig,
        command_tx: crossbeam_channel::Sender<MediaCommand>,
        vis_wake_tx: crossbeam_channel::Sender<()>,
    ) -> Self {
        let awaiting = config.music_dir.is_none();

        // Initialize ratatui-image picker
        let picker = ratatui_image::picker::Picker::from_query_stdio()
            .ok()
            .or_else(|| Some(ratatui_image::picker::Picker::from_fontsize((8, 16))));

        let visualizer_bars = Arc::new(RwLock::new(vec![0.0f32; 32]));

        // Spawn background thread to initialize the audio player
        let (player_tx, player_rx) = crossbeam_channel::bounded(1);
        std::thread::spawn(move || {
            if let Some(player) = Player::new() {
                let _ = player_tx.send(player);
            }
        });

        let mut app = Self {
            player: None,
            playlist: Playlist::new(),
            config: config.clone(),
            visualizer_bars: visualizer_bars.clone(),
            visualizer_mode: VisualizerMode::Spectrum,
            visualizer_enabled: config.visualizer_enabled,
            library: Vec::new(),
            flat_library: Vec::new(),
            full_flat_library: Vec::new(),
            collapsed_dirs: std::collections::HashSet::new(),
            library_loading: false,
            library_rx: None,
            active_panel: ActivePanel::Library,
            queue_cursor: 0,
            library_cursor: 0,
            lyrics_scroll: 0,
            show_folders: false,
            refresh_needed: true,
            terminal_focused: true,
            player_loading: true,
            player_rx: Some(player_rx),
            searching: false,
            search_query: String::new(),
            search_results: Vec::new(),
            search_cursor: 0,
            current_lyrics: None,
            now_playing_meta: None,
            show_full_lyrics: false,
            awaiting_dir_input: awaiting,
            dir_input: config.music_dir.clone().unwrap_or_default(),
            #[cfg(target_os = "linux")]
            mpris_state: None,
            #[cfg(target_os = "linux")]
            mpris_update_tx: None,
            pending_mpris_update: false,
            last_mpris_trigger: None,
            status_msg: None,
            stopped: true,
            pending_seek: None,
            last_seek_input: None,
            picker,
            current_cover_protocol: None,
            last_cover_tmp_path: None,
            vis_wake_tx: Some(vis_wake_tx),
        };

        // Load library: use cache for instant display, rescan in background for freshness
        if let Some(ref dir) = config.music_dir {
            // Load cache immediately so the library tab is populated on first render
            if let Some(cached) = App::load_library_cache(dir) {
                app.library = cached;
                app.rebuild_flat_library();
                // Mark not loading — cache is sufficient until rescan completes
                app.library_loading = false;
            }
            // Always rescan in background to pick up added/removed files
            app.scan_library(dir);
        }

        // Start MPRIS (Linux only)
        #[cfg(target_os = "linux")]
        {
            let (mpris_state, mpris_update_tx) = mpris::start_mpris(command_tx);
            app.mpris_state = Some(mpris_state);
            app.mpris_update_tx = Some(mpris_update_tx);
        }
        // Suppress unused warning on non-Linux
        #[cfg(not(target_os = "linux"))]
        let _ = command_tx;

        app
    }
    pub fn player(&self) -> Option<&Player> {
        self.player.as_ref()
    }

    pub fn player_mut(&mut self) -> Option<&mut Player> {
        self.player.as_mut()
    }

    pub fn display_elapsed_ms(&self) -> u64 {
        if let Some(seek) = self.pending_seek {
            seek.as_millis() as u64
        } else {
            self.player().map(|p| p.elapsed_ms()).unwrap_or(0)
        }
    }

    pub fn display_elapsed_secs(&self) -> f64 {
        if let Some(seek) = self.pending_seek {
            seek.as_secs_f64()
        } else {
            self.player().map(|p| p.elapsed_secs()).unwrap_or(0.0)
        }
    }

    pub fn finalize_player_init(&mut self, mut player: Player) {
        player.set_volume(self.config.volume);

        // Spawn background FFT visualizer thread.
        // After each spectrum frame it fires a non-blocking try_send on vis_wake_tx
        // so the main select! loop can immediately redraw the visualizer bars at
        // ~30 fps (34 ms cadence) without the main tick needing to run that fast.
        let visualizer_bars_clone = self.visualizer_bars.clone();
        let sample_buffer = player.sample_buffer.clone();
        let is_paused = player.is_paused.clone();
        let is_playing = player.is_playing.clone();
        // Clone the sender so the FFT thread owns it; the App retains a copy too.
        let vis_wake_tx = self.vis_wake_tx.clone();

        std::thread::spawn(move || {
            let mut engine = VisualizerEngine::new(2048, 32);
            static SILENCE: [f32; 2048] = [0.0f32; 2048];
            // Pre-allocated scratch buffer reused every frame — eliminates the
            // 8 KB Vec<f32> heap allocation that read_latest() previously caused
            // ~30 times per second. (Item 9)
            let mut sample_scratch = vec![0.0f32; 2048];
            loop {
                std::thread::sleep(std::time::Duration::from_millis(34));

                let playing = is_playing.load(Ordering::Acquire);
                let paused = is_paused.load(Ordering::Acquire);

                if playing && !paused {
                    if let Ok(buf) = sample_buffer.lock() {
                        buf.read_latest_into(&mut sample_scratch);
                        let sr = buf.sample_rate;
                        drop(buf);
                        engine.process(&sample_scratch, sr);
                    }
                } else {
                    // Decay the visualizer bars by feeding silence.
                    engine.process(&SILENCE, 44100);
                }

                // Publish the new bar data. try_write() is non-blocking:
                // if the render loop currently holds a read lock (i.e., is
                // actively drawing), we simply skip this write cycle rather
                // than stalling the FFT thread and causing ALSA underruns.
                if let Ok(mut shared) = visualizer_bars_clone.try_write() {
                    if shared.len() == engine.bars.len() {
                        shared.copy_from_slice(&engine.bars);
                    } else {
                        *shared = engine.bars.clone();
                    }
                }

                // Wake up the main render loop. try_send is non-blocking and
                // discards the signal if the channel is already full (bounded(1)),
                // which naturally rate-limits wake-ups to one per render cycle.
                if let Some(ref tx) = vis_wake_tx {
                    let _ = tx.try_send(());
                }
            }
        });

        self.player = Some(player);
        self.player_loading = false;
        self.player_rx = None;
    }
    /// Scan the music library directory asynchronously.
    pub fn scan_library(&mut self, dir: &str) {
        if self.library.is_empty() {
            self.library_loading = true;
        }
        self.refresh_needed = true;
        let (library_tx, library_rx) = crossbeam_channel::bounded::<Vec<LibraryEntry>>(1);
        self.library_rx = Some(library_rx);
        let dir = dir.to_string();
        let strip = self.config.strip_track_numbers;
        std::thread::spawn(move || {
            let path = Path::new(&dir);
            if path.is_dir() {
                let lib = library::scan_library(path, strip);
                let _ = library_tx.send(lib);
            }
        });
    }

    /// Returns the path to the library JSON cache file for the given music directory.
    fn library_cache_path(music_dir: &str) -> Option<std::path::PathBuf> {
        let proj = directories::ProjectDirs::from("com", "mixed", "mixed")?;
        let cache_dir = proj.cache_dir().to_path_buf();
        std::fs::create_dir_all(&cache_dir).ok()?;
        // Use a hash of the music dir path so different dirs get different caches
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut h = DefaultHasher::new();
        music_dir.hash(&mut h);
        Some(cache_dir.join(format!("library_{:x}.json", h.finish())))
    }

    /// Load the library from a JSON cache file (fast, no audio-file I/O).
    pub fn load_library_cache(music_dir: &str) -> Option<Vec<LibraryEntry>> {
        let path = Self::library_cache_path(music_dir)?;
        let file = std::fs::File::open(path).ok()?;
        serde_json::from_reader(std::io::BufReader::new(file)).ok()
    }

    /// Save the library to a JSON cache file on a background thread.
    ///
    /// Uses `serde_json::to_writer` + `BufWriter` to stream JSON directly to
    /// disk without materialising a multi-MB string in memory (Item 12).
    /// Spawning a detached thread means the main event loop is never blocked by
    /// disk I/O after a scan completes (Item 13).
    pub fn save_library_cache(library: &[LibraryEntry], music_dir: &str) {
        if let Some(path) = Self::library_cache_path(music_dir) {
            // Clone the library for the background thread. Cover art bytes are
            // excluded from serialization via #[serde(skip)], so this clone is
            // only metadata strings and pathbufs — much smaller than it looks.
            let library_clone: Vec<LibraryEntry> = library.to_vec();
            std::thread::spawn(move || {
                if let Ok(file) = std::fs::File::create(&path) {
                    let writer = std::io::BufWriter::new(file);
                    if let Err(e) = serde_json::to_writer(writer, &library_clone) {
                        eprintln!("library cache save failed: {}", e);
                    }
                }
            });
        }
    }

    /// Rebuild only the UI-visible flat library (respects collapsed_dirs and
    /// the current playlist's enqueue state). Called on expand/collapse and
    /// whenever the playlist changes. O(N_visible_items).
    pub fn rebuild_flat_library_view(&mut self) {
        self.flat_library = library::flatten_library(
            &self.library,
            0,
            Vec::new(),
            &self.collapsed_dirs,
            &self.playlist.entry_paths,
        );
    }

    /// Rebuild BOTH flat views. Called only when the raw library data changes
    /// (initial cache load or background scan completion). The full_flat_library
    /// (no collapsed dirs) is expensive to recompute for large trees, so we
    /// avoid doing it on every expand/collapse or playlist mutation. O(N_total).
    pub fn rebuild_flat_library(&mut self) {
        self.flat_library = library::flatten_library(
            &self.library,
            0,
            Vec::new(),
            &self.collapsed_dirs,
            &self.playlist.entry_paths,
        );
        // full_flat_library: fully expanded, used for instant fuzzy search.
        // No collapsed dirs, but enqueued bools still computed from playlist.
        self.full_flat_library = library::flatten_library(
            &self.library,
            0,
            Vec::new(),
            &std::collections::HashSet::new(),
            &self.playlist.entry_paths,
        );
    }

    /// Expand a collapsed directory path.
    pub fn expand_dir(&mut self, path: std::path::PathBuf) {
        if self.collapsed_dirs.remove(&path) {
            // Only rebuild the view (not full_flat_library) — dir expand/collapse
            // doesn't change the underlying library data. (Item 7)
            self.rebuild_flat_library_view();
        }
    }

    /// Collapse an expanded directory path.
    pub fn collapse_dir(&mut self, path: std::path::PathBuf) {
        if self.collapsed_dirs.insert(path) {
            self.rebuild_flat_library_view();
        }
    }

    /// Load a saved session, restoring the queue and seeking to saved position.
    pub fn load_session(&mut self) {
        if let Some(state) = session::load_session() {
            // Rebuild playlist from saved paths
            for path in &state.playlist_paths {
                if path.exists() {
                    let meta = metadata::read_metadata(path);
                    self.playlist.add(path.clone(), meta);
                }
            }

            if !self.playlist.is_empty() {
                self.playlist.current = state.current_index.min(self.playlist.len() - 1);
                self.playlist.repeat = state.repeat_mode;
                self.playlist.set_shuffle(state.shuffle);
                if let Some(p) = self.player.as_mut() {
                    p.set_volume(state.volume)
                };

                // Sync queue cursor with loaded track
                self.queue_cursor = self.playlist.current_real_index().unwrap_or(0);

                // Load the track but always start paused
                if let Some(entry) = self.playlist.current_entry() {
                    let path = entry.path.clone();
                    let start_pos = if state.position_ms > 0 {
                        Some(state.position_ms)
                    } else {
                        None
                    };
                    let load_ok = if let Some(player) = self.player.as_mut() {
                        player.load_track_with_pos(&path, start_pos).is_ok()
                    } else {
                        false
                    };

                    if load_ok {
                        self.set_now_playing_meta();
                        self.load_lyrics_for_current();
                        self.generate_cover_art_protocol();
                        if let Some(player) = self.player.as_mut() {
                            if !state.was_playing {
                                player.pause(); // restore paused state
                            }
                        }
                        self.push_mpris_metadata();
                        self.push_mpris_playback();
                        self.active_panel = ActivePanel::NowPlaying;
                        self.refresh_needed = true;
                    }
                }
            }
        }
    }

    /// Save current state as a session.
    pub fn save_state(&self) {
        let state = SessionState {
            playlist_paths: self.playlist.paths(),
            current_index: self.playlist.current_real_index().unwrap_or(0),
            position_ms: self.player().map(|p| p.elapsed_ms()).unwrap_or(0),
            was_playing: self.player().map(|p| p.is_playing()).unwrap_or(false),
            volume: self.player().map(|p| p.volume()).unwrap_or(100),
            repeat_mode: self.playlist.repeat,
            shuffle: self.playlist.shuffle,
        };
        session::save_session(&state);
    }

    /// Set the now-playing metadata from current playlist entry.
    fn set_now_playing_meta(&mut self) {
        self.now_playing_meta = self.playlist.current_entry().map(|e| e.metadata.clone());
    }

    /// Load external .lrc lyrics for the current track.
    fn load_lyrics_for_current(&mut self) {
        self.current_lyrics = self
            .playlist
            .current_entry()
            .and_then(|e| lyrics::load_lyrics_from_lrc(&e.path));
    }

    pub fn generate_cover_art_protocol(&mut self) {
        // Clean up the previous cover art cache file
        if let Some(old_path) = self.last_cover_tmp_path.take() {
            let _ = std::fs::remove_file(&old_path);
        }
        self.current_cover_protocol = None;
        let mut extracted_art_path = String::new();

        // Resolve the path of the currently playing track.
        // Cover art is NOT stored in TrackMetadata (stripped at scan-time to save RAM).
        // We read it lazily here — once per track change — via a targeted lofty probe.
        let track_path = self.playlist.current_entry().map(|e| e.path.clone());
        let title_key = self
            .playlist
            .current_entry()
            .and_then(|e| e.metadata.title.clone())
            .unwrap_or_else(|| "unknown".to_string());

        if let Some(path) = track_path {
            if let Some(cover_bytes) = metadata::read_cover_art(&path) {
                if let Ok(dyn_img) = image::load_from_memory(&cover_bytes) {
                    // Resolve XDG/platform cache directory for cover art files
                    let cache_dir = directories::ProjectDirs::from("", "", "mixed")
                        .map(|p| p.cache_dir().to_path_buf())
                        .unwrap_or_else(std::env::temp_dir);
                    let _ = std::fs::create_dir_all(&cache_dir);

                    let safe_title = title_key.replace([' ', '/'], "_");
                    let tmp_path = cache_dir.join(format!("mixed_cover_{}.png", safe_title));

                    // Save BEFORE moving dyn_img into the protocol (move drops pixel buffer)
                    if dyn_img.save(&tmp_path).is_ok() {
                        extracted_art_path = format!("file://{}", tmp_path.display());
                        self.last_cover_tmp_path = Some(tmp_path.to_string_lossy().into_owned());
                    }

                    // Move dyn_img (no clone) — pixel buffer freed after protocol creation
                    if let Some(ref picker) = self.picker {
                        self.current_cover_protocol = Some(picker.new_resize_protocol(dyn_img));
                    }
                }
            }
        }

        // Push updated art URL to MPRIS (Linux only)
        #[cfg(target_os = "linux")]
        if let Some(ref state) = self.mpris_state {
            if let Ok(mut meta) = state.metadata.write() {
                meta.art_url = extracted_art_path;
            }
            self.trigger_mpris_update();
        }
    }

    pub fn play_current(&mut self) {
        self.stopped = false;
        self.pending_seek = None;
        self.last_seek_input = None;
        if let Some(entry) = self.playlist.current_entry() {
            let path = entry.path.clone();
            let load_res = self.player.as_mut().map(|player| player.load_track(&path));

            if let Some(res) = load_res {
                match res {
                    Ok(()) => {
                        self.set_now_playing_meta();
                        self.load_lyrics_for_current();
                        self.generate_cover_art_protocol();
                        self.push_mpris_metadata();
                        self.push_mpris_playback();

                        // Sync queue cursor with currently playing track
                        self.queue_cursor = self.playlist.current_real_index().unwrap_or(0);

                        // Send desktop notification if terminal is not in focus
                        if !self.terminal_focused {
                            if let Some(ref meta) = self.now_playing_meta {
                                let title = meta.display_title(self.config.strip_track_numbers);
                                let artist = meta.display_artist();
                                let body = format!("Current Song: {} - {}", title, artist);
                                let _ = std::process::Command::new("notify-send")
                                    .arg("mixed")
                                    .arg(&body)
                                    .spawn();
                            }
                        }

                        // Update terminal title
                        if let Some(ref meta) = self.now_playing_meta {
                            let title = format!(
                                "mixed: {} - {}",
                                meta.display_artist(),
                                meta.display_title(self.config.strip_track_numbers)
                            );
                            print!("\x1b]0;{}\x07", title);
                            let _ = std::io::Write::flush(&mut std::io::stdout());
                        }

                        // Set duration from metadata if rodio didn't report it
                        let duration = self.player.as_ref().map(|p| p.duration_ms()).unwrap_or(0);
                        if duration == 0 {
                            if let Some(ref meta) = self.now_playing_meta {
                                if let Some(dur) = meta.duration {
                                    if let Some(player) = self.player.as_mut() {
                                        player.set_duration_ms(dur.as_millis() as u64);
                                    }
                                }
                            }
                        }
                        self.refresh_needed = true;
                    }
                    Err(e) => {
                        self.status_msg = Some(format!("Error: {}", e));
                        self.refresh_needed = true;
                    }
                }
            }
        }
    }

    pub fn play(&mut self) {
        self.stopped = false;
        if let Some(p) = self.player.as_mut() {
            p.play()
        };
        self.push_mpris_playback();
        self.refresh_needed = true;
    }

    pub fn pause(&mut self) {
        if let Some(p) = self.player.as_mut() {
            p.pause()
        };
        self.push_mpris_playback();
        self.refresh_needed = true;
    }

    pub fn stop(&mut self) {
        self.stopped = true;
        if let Some(p) = self.player.as_mut() {
            p.stop()
        };
        self.push_mpris_playback();
        self.refresh_needed = true;
    }

    pub fn volume_up(&mut self) {
        if let Some(p) = self.player.as_mut() {
            p.volume_up()
        };
        self.push_mpris_playback();
        self.refresh_needed = true;
    }

    pub fn volume_down(&mut self) {
        if let Some(p) = self.player.as_mut() {
            p.volume_down()
        };
        self.push_mpris_playback();
        self.refresh_needed = true;
    }

    pub fn seek(&mut self, pos_ms: u64) {
        if let Some(p) = self.player.as_mut() {
            p.seek(pos_ms)
        };
        #[cfg(target_os = "linux")]
        if let Some(ref state) = self.mpris_state {
            let pos_us = pos_ms as i64 * 1000;
            state.position_us.store(pos_us, Ordering::Relaxed);
            state.seek_target.store(pos_us, Ordering::Relaxed);
        }
        self.trigger_mpris_update();
        self.refresh_needed = true;
    }

    pub fn trigger_mpris_update(&mut self) {
        #[cfg(target_os = "linux")]
        {
            // Idempotent: only reset the debounce timer if this is a *new* trigger.
            // Prevents chained calls (e.g. push_mpris_metadata → push_mpris_playback)
            // from resetting the timer and delaying the actual D-Bus emit. (Item 8)
            if !self.pending_mpris_update {
                self.last_mpris_trigger = Some(std::time::Instant::now());
            }
            self.pending_mpris_update = true;
        }
    }

    pub fn toggle_pause(&mut self) {
        if let Some(p) = self.player.as_mut() {
            p.toggle_pause()
        };
        self.push_mpris_playback();
        self.refresh_needed = true;
    }

    pub fn next_track(&mut self) {
        if self.playlist.is_empty() {
            self.stop();
            print!("\x1b]0;mixed\x07");
            let _ = std::io::Write::flush(&mut std::io::stdout());
            self.refresh_needed = true;
            return;
        }

        self.playlist.advance_track();
        self.play_current();
        self.refresh_needed = true;
    }

    pub fn prev_track(&mut self) {
        if self.playlist.is_empty() {
            return;
        }
        self.playlist.prev();
        self.play_current();
        self.refresh_needed = true;
    }

    pub fn toggle_shuffle(&mut self) {
        self.playlist.set_shuffle(!self.playlist.shuffle);
        #[cfg(target_os = "linux")]
        if let Some(ref mpris) = self.mpris_state {
            mpris
                .shuffle
                .store(self.playlist.shuffle, Ordering::Relaxed);
            mpris
                .can_go_next
                .store(self.playlist.can_go_next(), Ordering::Relaxed);
            mpris
                .can_go_previous
                .store(self.playlist.can_go_previous(), Ordering::Relaxed);
            self.trigger_mpris_update();
        }
        self.refresh_needed = true;
    }

    pub fn cycle_repeat(&mut self) {
        self.playlist.repeat = self.playlist.repeat.next();
        #[cfg(target_os = "linux")]
        if let Some(ref mpris) = self.mpris_state {
            let loop_status = match self.playlist.repeat {
                RepeatMode::Off => 0,
                RepeatMode::Track => 1,
                RepeatMode::Queue => 2,
            };
            mpris.loop_status.store(loop_status, Ordering::Relaxed);
            mpris
                .can_go_next
                .store(self.playlist.can_go_next(), Ordering::Relaxed);
            mpris
                .can_go_previous
                .store(self.playlist.can_go_previous(), Ordering::Relaxed);
            self.trigger_mpris_update();
        }
        self.refresh_needed = true;
    }

    pub fn toggle_visualizer(&mut self) {
        self.visualizer_mode = self.visualizer_mode.toggle();
        self.refresh_needed = true;
    }

    pub fn next_panel(&mut self) {
        self.active_panel = match self.active_panel {
            ActivePanel::Queue => ActivePanel::Library,
            ActivePanel::Library => ActivePanel::NowPlaying,
            ActivePanel::NowPlaying => ActivePanel::Search,
            ActivePanel::Search => ActivePanel::Help,
            ActivePanel::Help => ActivePanel::Queue,
        };
        self.refresh_needed = true;
    }

    pub fn prev_panel(&mut self) {
        self.active_panel = match self.active_panel {
            ActivePanel::Queue => ActivePanel::Help,
            ActivePanel::Library => ActivePanel::Queue,
            ActivePanel::NowPlaying => ActivePanel::Library,
            ActivePanel::Search => ActivePanel::NowPlaying,
            ActivePanel::Help => ActivePanel::Search,
        };
        self.refresh_needed = true;
    }

    /// Tick update — called every 250 ms (progress bar + auto-advance).
    pub fn tick(&mut self) {
        // Read player state once per tick into locals — avoids calling self.player()
        // 4–6 times with redundant atomic loads and Option unwraps. (Item 4)
        let (is_playing, is_paused, is_finished) = self
            .player()
            .map(|p| (p.is_playing(), p.is_paused(), p.is_finished()))
            .unwrap_or((false, false, false));
        let playing_and_not_paused = is_playing && !is_paused;

        // Mark dirty when actively playing (progress bar must advance)
        // OR when the visualizer has non-trivial energy (bars still decaying).
        if playing_and_not_paused {
            self.refresh_needed = true;
        } else if let Ok(bars) = self.visualizer_bars.try_read() {
            if bars.iter().any(|&b| b > 0.001) {
                self.refresh_needed = true;
            }
        }

        // Auto-advance to next track when current finishes
        if !self.stopped && !self.playlist.is_empty() && is_finished && !is_paused {
            self.next_track();
            self.refresh_needed = true;
        }

        // Update MPRIS position (Linux only)
        #[cfg(target_os = "linux")]
        self.push_mpris_position();

        // Debounce MPRIS properties changed signal (Linux only)
        #[cfg(target_os = "linux")]
        if self.pending_mpris_update {
            if let Some(last) = self.last_mpris_trigger {
                if last.elapsed().as_millis() > 300 {
                    if let Some(ref tx) = self.mpris_update_tx {
                        let _ = tx.send(());
                    }
                    self.pending_mpris_update = false;
                }
            }
        }
    }

    /// Push metadata to MPRIS (Linux only).
    #[cfg(target_os = "linux")]
    fn push_mpris_metadata(&mut self) {
        if let Some(ref state) = self.mpris_state {
            if let Some(ref meta) = self.now_playing_meta {
                {
                    if let Ok(mut s) = state.metadata.write() {
                        s.title = meta
                            .display_title(self.config.strip_track_numbers)
                            .to_string();
                        s.artist = meta.display_artist().to_string();
                        s.album = meta.display_album().to_string();
                        // art_url is set by generate_cover_art_protocol
                    }
                }

                state.length_us.store(
                    meta.duration
                        .map(|d| d.as_micros() as i64)
                        .unwrap_or_else(|| {
                            (self.player().map(|p| p.duration_ms()).unwrap_or(0) * 1000) as i64
                        }),
                    Ordering::Relaxed,
                );
                state.loop_status.store(
                    match self.playlist.repeat {
                        RepeatMode::Off => 0,
                        RepeatMode::Track => 1,
                        RepeatMode::Queue => 2,
                    },
                    Ordering::Relaxed,
                );
                state.can_play.store(true, Ordering::Relaxed);
                state.can_pause.store(true, Ordering::Relaxed);
                state
                    .can_go_next
                    .store(self.playlist.can_go_next(), Ordering::Relaxed);
                state
                    .can_go_previous
                    .store(self.playlist.can_go_previous(), Ordering::Relaxed);
            }
            self.trigger_mpris_update();
        }
    }

    /// Push playback status to MPRIS (Linux only).
    #[cfg(target_os = "linux")]
    pub fn push_mpris_playback(&mut self) {
        if let Some(ref state) = self.mpris_state {
            let status = if self.player().map(|p| p.is_paused()).unwrap_or(false) {
                2 // Paused
            } else if self.player().map(|p| p.is_playing()).unwrap_or(false) {
                1 // Playing
            } else {
                0 // Stopped
            };
            state.playback_status.store(status, Ordering::Relaxed);
            state.volume.store(
                (self.player().map(|p| p.volume()).unwrap_or(100) as f64 / 100.0).to_bits(),
                Ordering::Relaxed,
            );
            self.trigger_mpris_update();
        }
    }

    /// Push position to MPRIS (Linux only).
    #[cfg(target_os = "linux")]
    fn push_mpris_position(&mut self) {
        let mut length_changed = false;
        if let Some(ref state) = self.mpris_state {
            state.position_us.store(
                (self.player().map(|p| p.elapsed_ms()).unwrap_or(0) * 1000) as i64,
                Ordering::Relaxed,
            );
            let decoded_length_us =
                (self.player().map(|p| p.duration_ms()).unwrap_or(0) * 1000) as i64;
            if decoded_length_us > 0 && state.length_us.load(Ordering::Relaxed) != decoded_length_us
            {
                state.length_us.store(decoded_length_us, Ordering::Relaxed);
                length_changed = true;
            }
        }
        if length_changed {
            self.trigger_mpris_update();
        }
    }

    // ── Non-Linux no-op stubs so call sites compile on all platforms ─────────

    #[cfg(not(target_os = "linux"))]
    fn push_mpris_metadata(&mut self) {}

    #[cfg(not(target_os = "linux"))]
    pub fn push_mpris_playback(&mut self) {}

    #[cfg(not(target_os = "linux"))]
    fn push_mpris_position(&mut self) {}

    /// Enqueue the selected library entry.
    pub fn library_enqueue_selected(&mut self, play_now: bool) {
        self.refresh_needed = true;
        let old_len = self.playlist.len();

        if self.library_cursor == 0 {
            let all_enqueued = self.flat_library.iter().all(|item| {
                if let LibraryEntry::Track { path, .. } = &item.entry {
                    self.playlist.entry_paths.contains(path)
                } else {
                    true
                }
            });

            if all_enqueued {
                // Deselect/dequeue: remove all library tracks from the playlist
                let library_paths: std::collections::HashSet<_> = self
                    .flat_library
                    .iter()
                    .filter_map(|item| {
                        if let LibraryEntry::Track { path, .. } = &item.entry {
                            Some(path.clone())
                        } else {
                            None
                        }
                    })
                    .collect();

                let mut i = 0;
                let mut current_removed = false;
                while i < self.playlist.len() {
                    if library_paths.contains(&self.playlist.entries[i].path) {
                        let is_current = Some(i) == self.playlist.current_real_index();
                        self.playlist.remove(i);
                        if is_current {
                            current_removed = true;
                        }
                    } else {
                        i += 1;
                    }
                }
                if current_removed {
                    self.play_current();
                }
            } else {
                // Enqueue all library tracks (in-memory, no disk read!)
                let was_empty = self.playlist.is_empty();
                let mut tracks_to_add = Vec::new();
                for item in &self.flat_library {
                    if let LibraryEntry::Track { path, metadata, .. } = &item.entry {
                        if !self.playlist.entry_paths.contains(path) {
                            tracks_to_add.push((path.clone(), metadata.clone()));
                        }
                    }
                }

                // Sort tracks by album
                tracks_to_add.sort_by(|(path_a, meta_a), (path_b, meta_b)| {
                    let disc_a = meta_a.disc_number.unwrap_or(1);
                    let disc_b = meta_b.disc_number.unwrap_or(1);

                    let track_a = meta_a.track_number.unwrap_or(u32::MAX);
                    let track_b = meta_b.track_number.unwrap_or(u32::MAX);

                    disc_a
                        .cmp(&disc_b)
                        .then(track_a.cmp(&track_b))
                        .then_with(|| path_a.file_name().cmp(&path_b.file_name()))
                });

                for (p, meta) in tracks_to_add {
                    self.playlist.add(p, meta);
                }

                if play_now && self.playlist.len() > old_len {
                    if let Some(pos) = self.playlist.play_order.iter().position(|&o| o == old_len) {
                        self.playlist.current = pos;
                    } else {
                        self.playlist.current = old_len;
                    }
                    self.play_current();
                    self.active_panel = ActivePanel::NowPlaying;
                } else if was_empty && !self.playlist.is_empty() {
                    self.play_current();
                    self.active_panel = ActivePanel::NowPlaying;
                }
            }
            self.rebuild_flat_library_view();
            return;
        }

        let entry_idx = self.library_cursor - 1;
        if entry_idx >= self.flat_library.len() {
            return;
        }

        let entry = self.flat_library[entry_idx].entry.clone();
        let was_empty = self.playlist.is_empty();

        match entry {
            LibraryEntry::Directory { .. } => {
                let tracks = entry.get_all_tracks();
                let all_enqueued = !tracks.is_empty()
                    && tracks
                        .iter()
                        .all(|(p, _)| self.playlist.entry_paths.contains(p));

                if all_enqueued {
                    // Dequeue: remove all files belonging to this directory from the playlist
                    let dir_paths_set: std::collections::HashSet<_> =
                        tracks.into_iter().map(|(p, _)| p).collect();
                    let mut i = 0;
                    let mut current_removed = false;
                    while i < self.playlist.len() {
                        if dir_paths_set.contains(&self.playlist.entries[i].path) {
                            let is_current = Some(i) == self.playlist.current_real_index();
                            self.playlist.remove(i);
                            if is_current {
                                current_removed = true;
                            }
                        } else {
                            i += 1;
                        }
                    }
                    if current_removed {
                        self.play_current();
                    }
                } else {
                    // Enqueue: add all files not already in the playlist (in-memory, no disk read!)
                    let mut to_add = Vec::new();
                    for (p, meta) in tracks {
                        if !self.playlist.entry_paths.contains(&p) {
                            to_add.push((p, meta));
                        }
                    }

                    // Sort tracks by album
                    to_add.sort_by(|(path_a, meta_a), (path_b, meta_b)| {
                        let disc_a = meta_a.disc_number.unwrap_or(1);
                        let disc_b = meta_b.disc_number.unwrap_or(1);

                        let track_a = meta_a.track_number.unwrap_or(u32::MAX);
                        let track_b = meta_b.track_number.unwrap_or(u32::MAX);

                        disc_a
                            .cmp(&disc_b)
                            .then(track_a.cmp(&track_b))
                            .then_with(|| path_a.file_name().cmp(&path_b.file_name()))
                    });

                    for (p, meta) in to_add {
                        self.playlist.add(p, meta);
                    }

                    if play_now && self.playlist.len() > old_len {
                        if let Some(pos) =
                            self.playlist.play_order.iter().position(|&o| o == old_len)
                        {
                            self.playlist.current = pos;
                        } else {
                            self.playlist.current = old_len;
                        }
                        self.play_current();
                        self.active_panel = ActivePanel::NowPlaying;
                    } else if was_empty && !self.playlist.is_empty() {
                        self.play_current();
                        self.active_panel = ActivePanel::NowPlaying;
                    }
                }
            }
            LibraryEntry::Track { path, metadata, .. } => {
                if self.playlist.entry_paths.contains(&path) {
                    // Dequeue: remove this single track
                    if let Some(pos) = self.playlist.entries.iter().position(|e| e.path == path) {
                        let is_current = Some(pos) == self.playlist.current_real_index();
                        self.playlist.remove(pos);
                        if is_current {
                            self.play_current();
                        }
                    }
                } else {
                    // Enqueue (in-memory, no disk read!)
                    self.playlist.add(path, metadata);
                    if play_now && self.playlist.len() > old_len {
                        if let Some(pos) =
                            self.playlist.play_order.iter().position(|&o| o == old_len)
                        {
                            self.playlist.current = pos;
                        } else {
                            self.playlist.current = old_len;
                        }
                        self.play_current();
                        self.active_panel = ActivePanel::NowPlaying;
                    } else if was_empty && !self.playlist.is_empty() {
                        self.play_current();
                        self.active_panel = ActivePanel::NowPlaying;
                    }
                }
            }
        }
        self.rebuild_flat_library_view();
    }

    /// Enqueue the selected search result.
    pub fn search_enqueue_selected(&mut self, play_now: bool) {
        self.refresh_needed = true;
        if self.search_cursor >= self.search_results.len() {
            return;
        }

        let entry = self.search_results[self.search_cursor].clone();
        let was_empty = self.playlist.is_empty();
        let old_len = self.playlist.len();

        match entry {
            LibraryEntry::Directory { .. } => {
                let tracks = entry.get_all_tracks();
                let all_enqueued = !tracks.is_empty()
                    && tracks
                        .iter()
                        .all(|(p, _)| self.playlist.entry_paths.contains(p));

                if all_enqueued {
                    // Dequeue
                    let dir_paths_set: std::collections::HashSet<_> =
                        tracks.into_iter().map(|(p, _)| p).collect();
                    let mut i = 0;
                    let mut current_removed = false;
                    while i < self.playlist.len() {
                        if dir_paths_set.contains(&self.playlist.entries[i].path) {
                            let is_current = Some(i) == self.playlist.current_real_index();
                            self.playlist.remove(i);
                            if is_current {
                                current_removed = true;
                            }
                        } else {
                            i += 1;
                        }
                    }
                    if current_removed {
                        self.play_current();
                    }
                } else {
                    // Enqueue
                    let mut to_add = Vec::new();
                    for (p, meta) in tracks {
                        if !self.playlist.entry_paths.contains(&p) {
                            to_add.push((p, meta));
                        }
                    }

                    // Sort tracks by album
                    to_add.sort_by(|(path_a, meta_a), (path_b, meta_b)| {
                        let disc_a = meta_a.disc_number.unwrap_or(1);
                        let disc_b = meta_b.disc_number.unwrap_or(1);

                        let track_a = meta_a.track_number.unwrap_or(u32::MAX);
                        let track_b = meta_b.track_number.unwrap_or(u32::MAX);

                        disc_a
                            .cmp(&disc_b)
                            .then(track_a.cmp(&track_b))
                            .then_with(|| path_a.file_name().cmp(&path_b.file_name()))
                    });

                    for (p, meta) in to_add {
                        self.playlist.add(p, meta);
                    }

                    if play_now && self.playlist.len() > old_len {
                        if let Some(pos) =
                            self.playlist.play_order.iter().position(|&o| o == old_len)
                        {
                            self.playlist.current = pos;
                        } else {
                            self.playlist.current = old_len;
                        }
                        self.play_current();
                        self.active_panel = ActivePanel::NowPlaying;
                    } else if was_empty && !self.playlist.is_empty() {
                        self.play_current();
                        self.active_panel = ActivePanel::NowPlaying;
                    }
                }
            }
            LibraryEntry::Track { path, metadata, .. } => {
                if self.playlist.entry_paths.contains(&path) {
                    // Dequeue
                    if let Some(pos) = self.playlist.entries.iter().position(|e| e.path == path) {
                        let is_current = Some(pos) == self.playlist.current_real_index();
                        self.playlist.remove(pos);
                        if is_current {
                            self.play_current();
                        }
                    }
                } else {
                    // Enqueue
                    self.playlist.add(path, metadata);
                    if play_now && self.playlist.len() > old_len {
                        if let Some(pos) =
                            self.playlist.play_order.iter().position(|&o| o == old_len)
                        {
                            self.playlist.current = pos;
                        } else {
                            self.playlist.current = old_len;
                        }
                        self.play_current();
                        self.active_panel = ActivePanel::NowPlaying;
                    } else if was_empty && !self.playlist.is_empty() {
                        self.play_current();
                        self.active_panel = ActivePanel::NowPlaying;
                    }
                }
            }
        }
        self.rebuild_flat_library_view();
    }

    /// Run fuzzy search against the flat library.
    pub fn run_search(&mut self) {
        self.refresh_needed = true;
        if self.search_query.is_empty() {
            self.search_results.clear();
            self.search_cursor = 0;
            return;
        }

        let matcher = SkimMatcherV2::default();
        // Reuse the pre-computed fully-expanded flat library — zero allocation per keystroke.
        let mut scored: Vec<(i64, LibraryEntry)> = self
            .full_flat_library
            .iter()
            .filter_map(|item| {
                matcher
                    .fuzzy_match(item.entry.name(), &self.search_query)
                    .map(|score| (score, item.entry.clone()))
            })
            .collect();

        scored.sort_by_key(|b| std::cmp::Reverse(b.0));
        self.search_results = scored.into_iter().map(|(_, e)| e).collect();
        self.search_cursor = 0;
    }

    /// Finalize directory input from first-run setup.
    pub fn finalize_dir_input(&mut self) {
        self.refresh_needed = true;
        let dir = self.dir_input.trim().to_string();
        if Path::new(&dir).is_dir() {
            self.config.music_dir = Some(dir.clone());
            self.config.save();
            self.scan_library(&dir);
            self.awaiting_dir_input = false;
            self.active_panel = ActivePanel::Library;
        } else {
            self.status_msg = Some("Invalid directory path".to_string());
        }
    }

    /// Halt playback and purge all playlist items.
    pub fn clear_playlist(&mut self) {
        self.playlist.clear();
        if let Some(p) = self.player.as_mut() {
            p.stop()
        };
        self.queue_cursor = 0;
        self.now_playing_meta = None;
        self.current_cover_protocol = None;
        self.show_full_lyrics = false;
        self.lyrics_scroll = 0;
        self.refresh_needed = true;
        self.rebuild_flat_library_view();

        // Push Stopped state to MPRIS so the control center shows no track
        self.push_mpris_playback();

        // Clean up dangling tmp cover art file
        if let Some(old_path) = self.last_cover_tmp_path.take() {
            let _ = std::fs::remove_file(&old_path);
        }

        // Clear terminal title
        print!("\x1b]0;mixed\x07");
        let _ = std::io::Write::flush(&mut std::io::stdout());
    }
}
