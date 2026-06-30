use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use crate::audio::player::Player;
use crate::audio::visualizer::{VisualizerEngine, VisualizerMode};
use crate::config::app_config::AppConfig;
use crate::config::session::{self, SessionState};
use crate::data::library::{self, LibraryEntry};
use crate::data::lyrics::{self, LyricsData};
use crate::data::metadata::{self, TrackMetadata};
use crate::data::playlist::{Playlist, RepeatMode};
use crate::sys::mpris::{self, MprisCommand, SharedMprisState};

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
    pub player: Player,
    pub playlist: Playlist,
    pub config: AppConfig,
    pub visualizer_bars: Arc<Mutex<Vec<f32>>>,
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

    // -- MPRIS --
    pub mpris_state: Option<SharedMprisState>,
    pub mpris_update_tx: Option<tokio::sync::mpsc::UnboundedSender<()>>,
    pub pending_mpris_update: bool,
    pub last_mpris_trigger: Option<std::time::Instant>,

    // -- Status message --
    pub status_msg: Option<String>,

    // -- Image Picker --
    pub picker: Option<ratatui_image::picker::Picker>,
    pub current_cover_protocol: Option<ratatui_image::protocol::StatefulProtocol>,
    /// Last /tmp cover art PNG path — tracked for cleanup on track change.
    pub last_cover_tmp_path: Option<String>,
}

impl App {
    pub fn new(config: AppConfig, command_tx: crossbeam_channel::Sender<MprisCommand>) -> Self {
        let player = Player::new().expect("Failed to initialize audio output");
        let awaiting = config.music_dir.is_none();

        // Initialize ratatui-image picker
        let picker = ratatui_image::picker::Picker::from_query_stdio()
            .ok()
            .or_else(|| Some(ratatui_image::picker::Picker::from_fontsize((8, 16))));

        let visualizer_bars = Arc::new(Mutex::new(vec![0.0f32; 32]));

        let mut app = Self {
            player,
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
            searching: false,
            search_query: String::new(),
            search_results: Vec::new(),
            search_cursor: 0,
            current_lyrics: None,
            now_playing_meta: None,
            show_full_lyrics: false,
            awaiting_dir_input: awaiting,
            dir_input: config.music_dir.clone().unwrap_or_default(),
            mpris_state: None,
            mpris_update_tx: None,
            pending_mpris_update: false,
            last_mpris_trigger: None,
            status_msg: None,
            picker,
            current_cover_protocol: None,
            last_cover_tmp_path: None,
        };

        // Spawn background FFT visualizer thread (throttled to 34ms / ~30 FPS)
        {
            let visualizer_bars_clone = visualizer_bars.clone();
            let sample_buffer = app.player.sample_buffer.clone();
            let is_paused = app.player.is_paused.clone();
            let is_playing = app.player.is_playing.clone();

            std::thread::spawn(move || {
                let mut engine = VisualizerEngine::new(2048, 32);
                static SILENCE: [f32; 2048] = [0.0f32; 2048];
                loop {
                    std::thread::sleep(std::time::Duration::from_millis(34));

                    let playing = is_playing.load(Ordering::Relaxed);
                    let paused = is_paused.load(Ordering::Relaxed);

                    if playing && !paused {
                        if let Ok(buf) = sample_buffer.lock() {
                            let samples = buf.read_latest(2048);
                            let sr = buf.sample_rate;
                            drop(buf);
                            engine.process(&samples, sr);
                        }
                    } else {
                        // Decay the visualizer bars by feeding silence
                        engine.process(&SILENCE, 44100);
                    }

                    if let Ok(mut shared) = visualizer_bars_clone.lock() {
                        if shared.len() == engine.bars.len() {
                            shared.copy_from_slice(&engine.bars);
                        } else {
                            *shared = engine.bars.clone();
                        }
                    }
                }
            });
        }

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

        // Set volume from config
        app.player.set_volume(config.volume);

        // Start MPRIS
        let (mpris_state, mpris_update_tx) = mpris::start_mpris(command_tx);
        app.mpris_state = Some(mpris_state);
        app.mpris_update_tx = Some(mpris_update_tx);

        app
    }

    /// Scan the music library directory asynchronously.
    pub fn scan_library(&mut self, dir: &str) {
        self.library_loading = true;
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
        let data = std::fs::read_to_string(path).ok()?;
        serde_json::from_str(&data).ok()
    }

    /// Save the library to a JSON cache file for instant loading on next launch.
    pub fn save_library_cache(library: &[LibraryEntry], music_dir: &str) {
        if let Some(path) = Self::library_cache_path(music_dir) {
            if let Ok(json) = serde_json::to_string(library) {
                let _ = std::fs::write(path, json);
            }
        }
    }

    /// Rebuild the flattened library tree.
    pub fn rebuild_flat_library(&mut self) {
        self.flat_library =
            library::flatten_library(&self.library, 0, Vec::new(), &self.collapsed_dirs);
        // Also maintain a fully-expanded copy (no collapsed dirs) for instant search
        self.full_flat_library = library::flatten_library(
            &self.library,
            0,
            Vec::new(),
            &std::collections::HashSet::new(),
        );
    }

    /// Expand a collapsed directory path.
    pub fn expand_dir(&mut self, path: std::path::PathBuf) {
        if self.collapsed_dirs.remove(&path) {
            self.rebuild_flat_library();
        }
    }

    /// Collapse an expanded directory path.
    pub fn collapse_dir(&mut self, path: std::path::PathBuf) {
        if self.collapsed_dirs.insert(path) {
            self.rebuild_flat_library();
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
                self.player.set_volume(state.volume);

                // Sync queue cursor with loaded track
                self.queue_cursor = self.playlist.current_real_index().unwrap_or(0);

                // Load the track but always start paused
                if let Some(entry) = self.playlist.current_entry() {
                    let path = entry.path.clone();
                    if self.player.load_track(&path).is_ok() {
                        self.set_now_playing_meta();
                        self.load_lyrics_for_current();
                        self.generate_cover_art_protocol();
                        self.player.seek(state.position_ms);
                        if !state.was_playing {
                            self.player.pause(); // restore paused state
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
            position_ms: self.player.elapsed_ms(),
            was_playing: self.player.is_playing(),
            volume: self.player.volume(),
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
        // Clean up the previous cover art tmp file to prevent /tmp leaks
        if let Some(old_path) = self.last_cover_tmp_path.take() {
            let _ = std::fs::remove_file(&old_path);
        }
        self.current_cover_protocol = None;
        let mut extracted_art_path = String::new();

        if let Some(entry) = self.playlist.current_entry() {
            if let Some(ref cover_bytes) = entry.metadata.cover_art {
                if let Ok(dyn_img) = image::load_from_memory(cover_bytes) {
                    if let Some(ref picker) = self.picker {
                        self.current_cover_protocol =
                            Some(picker.new_resize_protocol(dyn_img.clone()));
                    }

                    let title = entry
                        .metadata
                        .title
                        .as_deref()
                        .unwrap_or("unknown")
                        .replace(" ", "_")
                        .replace("/", "_");
                    let tmp_path = format!("/tmp/mixed_cover_{}.png", title);
                    if dyn_img.save(&tmp_path).is_ok() {
                        extracted_art_path = format!("file://{}", tmp_path);
                        self.last_cover_tmp_path = Some(tmp_path); // track for next cleanup
                    }
                }
            }
        }

        if let Some(ref state) = self.mpris_state {
            if let Ok(mut meta) = state.metadata.write() {
                meta.art_url = extracted_art_path;
            }
            self.trigger_mpris_update();
        }
    }

    /// Play the current track in the playlist.
    pub fn play_current(&mut self) {
        if let Some(entry) = self.playlist.current_entry() {
            let path = entry.path.clone();
            match self.player.load_track(&path) {
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
                    if self.player.duration_ms() == 0 {
                        if let Some(ref meta) = self.now_playing_meta {
                            if let Some(dur) = meta.duration {
                                self.player.set_duration_ms(dur.as_millis() as u64);
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

    pub fn play(&mut self) {
        self.player.play();
        self.push_mpris_playback();
        self.refresh_needed = true;
    }

    pub fn pause(&mut self) {
        self.player.pause();
        self.push_mpris_playback();
        self.refresh_needed = true;
    }

    pub fn stop(&mut self) {
        self.player.stop();
        self.push_mpris_playback();
        self.refresh_needed = true;
    }

    pub fn volume_up(&mut self) {
        self.player.volume_up();
        self.push_mpris_playback();
        self.refresh_needed = true;
    }

    pub fn volume_down(&mut self) {
        self.player.volume_down();
        self.push_mpris_playback();
        self.refresh_needed = true;
    }

    pub fn seek(&mut self, pos_ms: u64) {
        self.player.seek(pos_ms);
        if let Some(ref state) = self.mpris_state {
            let pos_us = pos_ms as i64 * 1000;
            state.position_us.store(pos_us, Ordering::Relaxed);
            state.seek_target.store(pos_us, Ordering::Relaxed);
        }
        self.trigger_mpris_update();
        self.refresh_needed = true;
    }

    pub fn trigger_mpris_update(&mut self) {
        self.pending_mpris_update = true;
        self.last_mpris_trigger = Some(std::time::Instant::now());
    }

    pub fn toggle_pause(&mut self) {
        self.player.toggle_pause();
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

        self.playlist.next();
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

    /// Tick update — called every frame (~33ms).
    pub fn tick(&mut self) {
        // Mark dirty when actively playing (progress bar must advance)
        // OR when the visualizer has non-trivial energy (bars animating).
        // Keeping them separate avoids blanket 30fps redraws while paused.
        let playing_and_not_paused = self.player.is_playing() && !self.player.is_paused();
        if playing_and_not_paused {
            self.refresh_needed = true;
        } else if let Ok(bars) = self.visualizer_bars.lock() {
            if bars.iter().any(|&b| b > 0.001) {
                self.refresh_needed = true;
            }
        }

        // Auto-advance to next track when current finishes
        if !self.playlist.is_empty() && self.player.is_finished() && !self.player.is_paused() {
            self.next_track();
            self.refresh_needed = true;
        }

        // Update MPRIS position
        self.push_mpris_position();

        // Debounce MPRIS properties changed signal
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

    /// Push metadata to MPRIS.
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
                        .unwrap_or_else(|| (self.player.duration_ms() * 1000) as i64),
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

    /// Push playback status to MPRIS.
    pub fn push_mpris_playback(&mut self) {
        if let Some(ref state) = self.mpris_state {
            let status = if self.player.is_paused() {
                2 // Paused
            } else if self.player.is_playing() {
                1 // Playing
            } else {
                0 // Stopped
            };
            state.playback_status.store(status, Ordering::Relaxed);
            state.volume.store(
                (self.player.volume() as f64 / 100.0).to_bits(),
                Ordering::Relaxed,
            );
            self.trigger_mpris_update();
        }
    }

    /// Push position to MPRIS.
    fn push_mpris_position(&mut self) {
        let mut length_changed = false;
        if let Some(ref state) = self.mpris_state {
            state
                .position_us
                .store((self.player.elapsed_ms() * 1000) as i64, Ordering::Relaxed);
            let decoded_length_us = (self.player.duration_ms() * 1000) as i64;
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

    /// Enqueue the selected library entry.
    pub fn library_enqueue_selected(&mut self, play_now: bool) {
        self.refresh_needed = true;
        let old_len = self.playlist.len();
        // Use the pre-built HashSet for O(1) membership checks (vs O(N) Vec::contains)
        let enqueued_paths = self.playlist.entry_paths.clone();

        if self.library_cursor == 0 {
            let all_enqueued = self.flat_library.iter().all(|item| {
                if let LibraryEntry::Track { path, .. } = &item.entry {
                    enqueued_paths.contains(path)
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
                        if !enqueued_paths.contains(path) {
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
                let all_enqueued =
                    !tracks.is_empty() && tracks.iter().all(|(p, _)| enqueued_paths.contains(p));

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
                        if !enqueued_paths.contains(&p) {
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
                if enqueued_paths.contains(&path) {
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
        // Use the pre-built HashSet for O(1) membership checks (vs O(N) Vec::contains)
        let enqueued_paths = self.playlist.entry_paths.clone();

        match entry {
            LibraryEntry::Directory { .. } => {
                let tracks = entry.get_all_tracks();
                let all_enqueued =
                    !tracks.is_empty() && tracks.iter().all(|(p, _)| enqueued_paths.contains(p));

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
                        if !enqueued_paths.contains(&p) {
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
                if enqueued_paths.contains(&path) {
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
        let mut scored: Vec<(i64, LibraryEntry)> = self.full_flat_library
            .iter()
            .filter_map(|item| {
                matcher
                    .fuzzy_match(item.entry.name(), &self.search_query)
                    .map(|score| (score, item.entry.clone()))
            })
            .collect();

        scored.sort_by(|a, b| b.0.cmp(&a.0));
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
        self.player.stop();
        self.queue_cursor = 0;
        self.now_playing_meta = None;
        self.current_cover_protocol = None;
        self.show_full_lyrics = false;
        self.lyrics_scroll = 0;
        self.refresh_needed = true;

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
