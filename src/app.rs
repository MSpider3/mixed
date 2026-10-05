use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::{Arc, RwLock};

use crate::audio::player::Player;
use crate::audio::visualizer::VisualizerEngine;
use crate::audio::visualizer::VisualizerMode;
use crate::config::app_config::AppConfig;
use crate::config::session::{self, SessionState};
use crate::data::library::{self, LibraryEntry};
use crate::data::lyrics::LyricsData;
use crate::data::metadata::{self, TrackMetadata};
use crate::data::playlist::{Playlist, RepeatMode};
use crate::data::track::TrackRef;
#[cfg(target_os = "linux")]
use crate::sys::mpris::{self, SharedMprisState};
use crate::sys::MediaCommand;

mod track_lyrics;
pub use track_lyrics::{LyricsPicker, PICKER_FIXED_ROWS};

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

pub use crate::sources::SourceTab;

/// Saved panel view and cursors per source tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PanelMemory {
    pub panel: ActivePanel,
    pub queue_cursor: usize,
    pub library_cursor: usize,
    pub search_cursor: usize,
}

impl Default for PanelMemory {
    fn default() -> Self {
        Self {
            panel: ActivePanel::Library,
            queue_cursor: 0,
            library_cursor: 0,
            search_cursor: 0,
        }
    }
}

pub fn tabs_for(source: SourceTab) -> &'static [(&'static str, &'static str, ActivePanel)] {
    match source {
        SourceTab::Unified => &[
            ("F2", "Queue", ActivePanel::Queue),
            ("F4", "Track", ActivePanel::NowPlaying),
            ("F6", "Help", ActivePanel::Help),
        ],
        _ => &[
            ("F2", "Playlist", ActivePanel::Queue),
            ("F3", "Library", ActivePanel::Library),
            ("F4", "Track", ActivePanel::NowPlaying),
            ("F5", "Search", ActivePanel::Search),
            ("F6", "Help", ActivePanel::Help),
        ],
    }
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
    pub local_nav_stack: Vec<(std::path::PathBuf, String, usize)>,
    pub pending_container_enqueue: Option<(String, bool)>,

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
    pub player_event_rx: Option<crossbeam_channel::Receiver<crate::audio::player::PlayerEvent>>,
    pub load_generation: u64,
    pub consecutive_failures: u8,
    pub buffering: bool,

    // -- Search --
    pub searching: bool,
    pub search_query: String,
    pub search_results: Vec<LibraryEntry>,
    pub search_cursor: usize,

    // -- Lyrics --
    pub current_lyrics: Option<LyricsData>,
    /// Plain (unsynced) lyrics of the playing track; `current_lyrics` holds synced ones.
    pub current_plain_lyrics: Option<Vec<String>>,
    /// True while the playing track's lyrics are being looked up online.
    pub lyrics_searching: bool,
    /// The list of other lyrics for the playing track, while it is open.
    pub lyrics_picker: Option<LyricsPicker>,
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
    pub status_msg_at: Option<std::time::Instant>,
    pub stopped: bool,
    pub pending_seek: Option<std::time::Duration>,
    pub last_seek_input: Option<std::time::Instant>,

    // -- Search --
    pub fuzzy_matcher: SkimMatcherV2,

    // -- Image Picker --
    pub picker: Option<ratatui_image::picker::Picker>,
    pub current_cover_protocol: Option<ratatui_image::protocol::StatefulProtocol>,
    /// Last /tmp cover art PNG path — tracked for cleanup on track change.
    pub last_cover_tmp_path: Option<String>,

    // -- Source Tabs & State Memory --
    pub source: SourceTab,
    pub source_memory: [PanelMemory; 4],

    // -- Remote Sources (Phase 4) --
    pub spotify_view: crate::sources::SourceView,
    pub youtube_view: crate::sources::SourceView,
    pub source_runtime: Option<std::sync::Arc<crate::sources::runtime::SourceRuntime>>,
    pub source_event_rx: Option<crossbeam_channel::Receiver<crate::sources::SourceEvent>>,
    pub remote_search_pending: Option<(String, std::time::Instant)>,
    pub prefetched_video_id: Option<String>,
    pub prefetched_spotify_uri: Option<String>,
    /// Set while the current YouTube track plays from a partial download.
    pub streaming: Option<StreamingTrack>,

    // -- Visualizer wake-up channel --
    /// Non-blocking sender that the FFT thread fires after each spectrum frame
    /// (~34ms). The main select! loop receives on the paired Receiver and sets
    /// refresh_needed = true, giving ~30 fps to the visualizer without tying
    /// the main tick to a 34 ms sleep.
    pub vis_wake_tx: Option<crossbeam_channel::Sender<()>>,
    /// Trigger an explicit terminal clear on seek or lyric line change to prevent CTL cursor desync.
    pub force_terminal_clear: bool,
    /// Last active lyric line index (tracked for clean repaint on line transition).
    pub last_lyric_line: Option<usize>,
    /// Bounding boxes of interactive UI elements for mouse hit testing.
    pub ui_bounds: UiBounds,
}

/// A YouTube track that started playing before its download finished.
#[derive(Debug, Clone)]
pub struct StreamingTrack {
    pub video_id: String,
    /// Player load generation of the progressive playback attempt.
    pub generation: u64,
    pub progress: Arc<crate::audio::growing_file::DownloadProgress>,
}

/// Hit-test regions recorded during render for mouse click routing.
#[derive(Debug, Clone, Default)]
pub struct UiBounds {
    pub left_panel_rect: Option<ratatui::layout::Rect>,
    pub mini_controls_rect: Option<ratatui::layout::Rect>,
    pub progress_bar_rect: Option<ratatui::layout::Rect>,
    pub title_rect: Option<ratatui::layout::Rect>,
    pub artist_rect: Option<ratatui::layout::Rect>,
    pub footer_source_rect: Option<ratatui::layout::Rect>,
    pub footer_tabs_rect: Option<ratatui::layout::Rect>,
    pub scrollbar_rect: Option<ratatui::layout::Rect>,
}

impl UiBounds {
    pub fn reset(&mut self) {
        self.left_panel_rect = None;
        self.mini_controls_rect = None;
        self.progress_bar_rect = None;
        self.title_rect = None;
        self.artist_rect = None;
        self.footer_source_rect = None;
        self.footer_tabs_rect = None;
        self.scrollbar_rect = None;
    }
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
            local_nav_stack: Vec::new(),
            pending_container_enqueue: None,
            active_panel: ActivePanel::Library,
            queue_cursor: 0,
            library_cursor: 0,
            lyrics_scroll: 0,
            show_folders: false,
            refresh_needed: true,
            terminal_focused: true,
            player_loading: true,
            player_rx: Some(player_rx),
            player_event_rx: None,
            load_generation: 0,
            consecutive_failures: 0,
            buffering: false,
            searching: false,
            search_query: String::new(),
            search_results: Vec::new(),
            search_cursor: 0,
            current_lyrics: None,
            current_plain_lyrics: None,
            lyrics_searching: false,
            lyrics_picker: None,
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
            status_msg_at: None,
            stopped: true,
            pending_seek: None,
            last_seek_input: None,
            fuzzy_matcher: fuzzy_matcher::skim::SkimMatcherV2::default(),
            picker,
            current_cover_protocol: None,
            last_cover_tmp_path: None,
            source: SourceTab::Local,
            source_memory: [
                PanelMemory {
                    panel: ActivePanel::Library,
                    ..Default::default()
                },
                PanelMemory {
                    panel: ActivePanel::Library,
                    ..Default::default()
                },
                PanelMemory {
                    panel: ActivePanel::Library,
                    ..Default::default()
                },
                PanelMemory {
                    panel: ActivePanel::Queue,
                    ..Default::default()
                },
            ],
            spotify_view: crate::sources::SourceView::default(),
            youtube_view: crate::sources::SourceView::default(),
            source_runtime: None,
            source_event_rx: None,
            remote_search_pending: None,
            prefetched_video_id: None,
            prefetched_spotify_uri: None,
            streaming: None,
            vis_wake_tx: Some(vis_wake_tx),
            force_terminal_clear: false,
            last_lyric_line: None,
            ui_bounds: UiBounds::default(),
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

        // Suppress unused warning on non-Linux platforms
        #[cfg(not(target_os = "linux"))]
        let _ = command_tx;

        if let Err(e) = crate::config::credentials::Credentials::load_result() {
            app.set_status(e);
        }

        app
    }

    /// Set an auto-dismissing status message.
    pub fn set_status(&mut self, msg: impl Into<String>) {
        self.status_msg = Some(msg.into());
        self.status_msg_at = Some(std::time::Instant::now());
        self.refresh_needed = true;
    }

    pub fn player(&self) -> Option<&Player> {
        self.player.as_ref()
    }

    pub fn player_mut(&mut self) -> Option<&mut Player> {
        self.player.as_mut()
    }

    pub fn display_elapsed_ms(&self) -> u64 {
        let elapsed = if let Some(seek) = self.pending_seek {
            seek.as_millis() as u64
        } else {
            self.player().map(|p| p.elapsed_ms()).unwrap_or(0)
        };
        let duration = self.player().map(|p| p.duration_ms()).unwrap_or(0);
        if duration > 0 {
            elapsed.min(duration)
        } else {
            elapsed
        }
    }

    pub fn display_elapsed_secs(&self) -> f64 {
        let elapsed = if let Some(seek) = self.pending_seek {
            seek.as_secs_f64()
        } else {
            self.player().map(|p| p.elapsed_secs()).unwrap_or(0.0)
        };
        let duration = self.player().map(|p| p.duration_ms()).unwrap_or(0);
        if duration > 0 {
            elapsed.min(duration as f64 / 1000.0)
        } else {
            elapsed
        }
    }

    pub fn finalize_player_init(&mut self, mut player: Player) {
        player.set_volume(self.config.volume);

        // Spawn background FFT visualizer thread only if visualizer is enabled (A3).
        // After each spectrum frame it fires a non-blocking try_send on vis_wake_tx
        // so the main select! loop can immediately redraw the visualizer bars at
        // ~30 fps (34 ms cadence) without the main tick needing to run that fast.
        if self.config.visualizer_enabled {
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
                    // If receiver dropped (main loop exited), shut down this thread (B2).
                    if let Some(ref tx) = vis_wake_tx {
                        if let Err(crossbeam_channel::TrySendError::Disconnected(_)) =
                            tx.try_send(())
                        {
                            break;
                        }
                    }
                }
            });
        }

        self.player_event_rx = Some(player.event_rx.clone());
        self.player = Some(player);
        self.player_loading = false;
        self.player_rx = None;
    }

    /// Handle events emitted asynchronously by the audio player thread.
    pub fn handle_player_event(&mut self, event: crate::audio::player::PlayerEvent) {
        use crate::audio::player::PlayerEvent;
        match event {
            PlayerEvent::Loaded { generation } => {
                if generation == self.load_generation {
                    self.buffering = false;
                    self.consecutive_failures = 0;
                    self.refresh_needed = true;
                }
            }
            PlayerEvent::Buffering { generation } => {
                if generation == self.load_generation {
                    self.buffering = true;
                    self.refresh_needed = true;
                }
            }
            PlayerEvent::Failed { generation, error } => {
                if generation == self.load_generation {
                    // The partial file could not be decoded as it arrives (e.g. its
                    // index sits at the end): play the complete download instead.
                    if let Some(stream) = self.streaming.take_if(|s| s.generation == generation) {
                        if let Some(path) =
                            crate::sources::ytdlp::YtDlp::find_cached(&stream.video_id)
                        {
                            self.load_youtube_audio(crate::audio::player::PlayInput::File(path));
                            self.refresh_needed = true;
                            return;
                        }
                        if !stream.progress.is_done() {
                            self.buffering = true;
                            self.refresh_needed = true;
                            return;
                        }
                    }
                    self.buffering = false;
                    self.consecutive_failures = self.consecutive_failures.saturating_add(1);
                    self.set_status(format!("Playback error: {}", error));
                    if self.consecutive_failures >= 3 {
                        self.stop();
                        self.set_status("Playback stopped: 3 consecutive track failures");
                    } else if !self.playlist.is_empty() {
                        self.next_track();
                    }
                    self.refresh_needed = true;
                }
            }
            PlayerEvent::SpotifyLyrics { uri, lyrics } => self.handle_spotify_lyrics(uri, lyrics),
            PlayerEvent::SpotifySignInLost => {
                self.spotify_view.connected = false;
                self.spotify_view.error = Some(
                    "Playback sign-in is no longer valid. Press Enter to sign in again.".into(),
                );
                self.refresh_needed = true;
            }
            PlayerEvent::Finished { generation } => {
                if generation == self.load_generation {
                    self.consecutive_failures = 0;
                    if !self.stopped && !self.playlist.is_empty() {
                        self.next_track();
                    }
                    self.refresh_needed = true;
                }
            }
            PlayerEvent::Position {
                generation,
                position_ms,
            } => {
                if generation == self.load_generation {
                    if let Some(ref player) = self.player {
                        player.elapsed_ms.store(position_ms, Ordering::Relaxed);
                    }
                }
            }
        }
    }

    /// Initialize the background async network runtime.
    pub fn init_source_runtime(
        &mut self,
        rt: std::sync::Arc<crate::sources::runtime::SourceRuntime>,
    ) {
        let _ = rt.send(crate::sources::SourceRequest::CheckAuth(SourceTab::Spotify));
        let _ = rt.send(crate::sources::SourceRequest::CheckAuth(SourceTab::YouTube));
        self.source_event_rx = Some(rt.event_rx().clone());
        self.source_runtime = Some(rt);
    }

    /// Access the SourceView for the currently active source (if remote).
    pub fn active_source_view(&self) -> Option<&crate::sources::SourceView> {
        match self.source {
            SourceTab::Spotify => Some(&self.spotify_view),
            SourceTab::YouTube => Some(&self.youtube_view),
            _ => None,
        }
    }

    /// Mutably access the SourceView for the currently active source (if remote).
    pub fn active_source_view_mut(&mut self) -> Option<&mut crate::sources::SourceView> {
        match self.source {
            SourceTab::Spotify => Some(&mut self.spotify_view),
            SourceTab::YouTube => Some(&mut self.youtube_view),
            _ => None,
        }
    }

    /// Mutably access the SourceView for a specific source tab.
    pub fn source_view_mut(
        &mut self,
        source: SourceTab,
    ) -> Option<&mut crate::sources::SourceView> {
        match source {
            SourceTab::Spotify => Some(&mut self.spotify_view),
            SourceTab::YouTube => Some(&mut self.youtube_view),
            _ => None,
        }
    }

    /// Dispatch remote search to the async network runtime.
    pub fn dispatch_remote_search(&mut self, query: String) {
        let source = self.source;
        if let Some(view) = self.source_view_mut(source) {
            view.search_query = query.clone();
            if query.trim().is_empty() {
                view.search_results.clear();
                view.loading = false;
                view.search_cursor = 0;
                self.refresh_needed = true;
                return;
            }
            view.loading = true;
            view.search_cursor = 0;
            view.search_generation = view.search_generation.wrapping_add(1);
            let gen = view.search_generation;
            if let Some(ref rt) = self.source_runtime {
                let _ = rt.send(crate::sources::SourceRequest::Search {
                    source,
                    query,
                    generation: gen,
                    page: 0,
                });
            }
        }
        self.refresh_needed = true;
    }

    /// True while playback of the current track is waiting on this video's download.
    fn awaiting_youtube_download(&self, video_id: &str) -> bool {
        self.buffering
            && !self.stopped
            && matches!(
                self.playlist.current_entry().map(|e| &e.id),
                Some(TrackRef::YouTube(v)) if v == video_id
            )
    }

    /// Load the current YouTube track's audio (complete or still downloading).
    fn load_youtube_audio(&mut self, input: crate::audio::player::PlayInput) {
        self.buffering = false;
        self.streaming = None;
        let duration_ms = self
            .playlist
            .current_entry()
            .and_then(|e| e.metadata.duration)
            .map(|d| d.as_millis() as u64);
        if let Some(player) = self.player.as_mut() {
            let _ = player.load_with_pos_and_duration(input, None, duration_ms);
            self.load_generation = player.generation;
        }
        self.load_lyrics_for_current();
        self.generate_cover_art_protocol();
        self.push_mpris_metadata();
        self.push_mpris_playback();
    }

    /// Handle events emitted asynchronously by the network runtime.
    pub fn handle_source_event(&mut self, event: crate::sources::SourceEvent) {
        use crate::sources::SourceEvent;
        match event {
            SourceEvent::AuthState {
                source,
                connected,
                user_name,
                error,
            } => {
                if let Some(view) = self.source_view_mut(source) {
                    view.connected = connected;
                    view.user_name = user_name;
                    view.error = error;
                    view.login_status = None;
                    self.refresh_needed = true;
                }
            }
            SourceEvent::LoginProgress { source, message } => {
                if let Some(view) = self.source_view_mut(source) {
                    view.error = None;
                    view.login_status = Some(message);
                    self.refresh_needed = true;
                }
            }
            SourceEvent::Roots { source, items } => {
                if let Some(view) = self.source_view_mut(source) {
                    view.roots = items.clone();
                    view.flat = items;
                    view.expanded.clear();
                    view.nav_stack.clear();
                    view.cursor = 0;
                    view.loading = false;
                    self.refresh_needed = true;
                }
            }
            SourceEvent::Children {
                source,
                parent_id,
                items,
                page,
                has_more,
            } => {
                if let Some(view) = self.source_view_mut(source) {
                    if page == 0 {
                        view.expand(&parent_id, items.clone());
                    } else {
                        view.append_page(&parent_id, items.clone());
                    }
                    view.loading = false;
                }
                if let Some((pending_id, play_now)) = self.pending_container_enqueue.clone() {
                    if pending_id == parent_id {
                        let tracks: Vec<(TrackRef, TrackMetadata)> = items
                            .into_iter()
                            .filter_map(|it| {
                                let tr = it.track_ref?;
                                let meta = TrackMetadata {
                                    title: Some(it.title),
                                    artist: it.subtitle,
                                    album: it.album,
                                    duration: it.duration_secs.map(std::time::Duration::from_secs),
                                    cover_url: it.artwork_url,
                                    ..Default::default()
                                };
                                Some((tr, meta))
                            })
                            .collect();
                        if !tracks.is_empty() {
                            let count = tracks.len();
                            self.toggle_enqueue_tracks(&tracks, play_now && page == 0);
                            self.set_status(format!("Enqueued {} tracks", count));
                        }
                        if !has_more {
                            self.pending_container_enqueue = None;
                        }
                    }
                }
                self.refresh_needed = true;
            }
            SourceEvent::SearchResults {
                source,
                generation,
                items,
                page,
                ..
            } => {
                if let Some(view) = self.source_view_mut(source) {
                    if view.search_generation == generation {
                        if page == 0 {
                            view.search_results = items;
                            view.search_cursor = 0;
                            // Expansions inside the replaced results no longer exist
                            let flat = &view.flat;
                            view.expanded
                                .retain(|id| flat.iter().any(|it| &it.id == id));
                        } else {
                            view.search_results.extend(items);
                        }
                        view.loading = false;
                        self.refresh_needed = true;
                    }
                }
            }
            SourceEvent::LyricsFetched { track, result } => {
                self.handle_lyrics_fetched(track, result)
            }
            SourceEvent::LyricsMatches { track, result } => {
                self.handle_lyrics_matches(track, result)
            }
            SourceEvent::CoverFetched { .. } => {
                self.generate_cover_art_protocol();
                #[cfg(target_os = "linux")]
                self.push_mpris_metadata();
                self.refresh_needed = true;
            }
            SourceEvent::YouTubeTrackDownloaded { video_id, path, .. } => {
                // Matched by video id: a prefetch already in flight for this track
                // absorbs the foreground request, so its generation is not ours.
                if self.awaiting_youtube_download(&video_id) {
                    self.load_youtube_audio(crate::audio::player::PlayInput::File(path));
                    self.refresh_needed = true;
                }
            }
            SourceEvent::YouTubeTrackStreamable {
                video_id,
                path,
                progress,
            } => {
                // Start playing the partial file; the reader waits for the rest.
                if self.awaiting_youtube_download(&video_id) {
                    self.load_youtube_audio(crate::audio::player::PlayInput::Growing {
                        path,
                        progress: progress.clone(),
                    });
                    self.streaming = Some(StreamingTrack {
                        video_id,
                        generation: self.load_generation,
                        progress,
                    });
                    self.refresh_needed = true;
                }
            }
            SourceEvent::YouTubeDownloadFailed {
                video_id, error, ..
            } => {
                if self.awaiting_youtube_download(&video_id) {
                    self.buffering = false;
                    self.consecutive_failures = self.consecutive_failures.saturating_add(1);
                    self.set_status(format!("YouTube download failed: {}", error));
                    if self.consecutive_failures >= 3 {
                        self.stop();
                        self.set_status("Playback stopped: 3 consecutive track failures");
                    } else if !self.playlist.is_empty() {
                        self.next_track();
                    }
                    self.refresh_needed = true;
                } else if self
                    .streaming
                    .as_ref()
                    .is_some_and(|st| st.video_id == video_id)
                {
                    // Already playing the partial file: it will end where the data stops
                    self.set_status(format!("YouTube download interrupted: {}", error));
                }
            }
            SourceEvent::Error { source, error } => {
                if let Some(view) = self.source_view_mut(source) {
                    view.loading = false;
                    view.error = Some(error.clone());
                }
                self.set_status(format!("Remote error: {}", error));
                self.refresh_needed = true;
            }
        }
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
        let proj = directories::ProjectDirs::from("", "", "mixed")?;
        let cache_dir = proj.cache_dir().to_path_buf();
        std::fs::create_dir_all(&cache_dir).ok()?;
        // Stable deterministic FNV-1a hash of the music dir path (B3)
        let hash = crate::utils::hash::fnv1a_hex(music_dir);
        Some(cache_dir.join(format!("library_{}.json", hash)))
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
    /// Atomic write via temp file + rename prevents corrupted cache on kill/power loss (B7).
    pub fn save_library_cache(library: &[LibraryEntry], music_dir: &str) {
        if let Some(path) = Self::library_cache_path(music_dir) {
            // Clone the library for the background thread. Cover art bytes are
            // excluded from serialization via #[serde(skip)], so this clone is
            // only metadata strings and pathbufs — much smaller than it looks.
            let library_clone: Vec<LibraryEntry> = library.to_vec();
            std::thread::spawn(move || {
                let tmp_path = path.with_extension("json.tmp");
                if let Ok(file) = std::fs::File::create(&tmp_path) {
                    let mut writer = std::io::BufWriter::new(file);
                    use std::io::Write;
                    if serde_json::to_writer(&mut writer, &library_clone).is_ok()
                        && writer.flush().is_ok()
                    {
                        let _ = std::fs::rename(&tmp_path, &path);
                    }
                }
            });
        }
    }

    /// Returns the entries for the currently active directory in local browsing.
    pub fn current_local_entries(&self) -> &[LibraryEntry] {
        let mut cur = self.library.as_slice();
        for (path, _, _) in &self.local_nav_stack {
            let mut found = None;
            for entry in cur {
                if let LibraryEntry::Directory {
                    path: p, children, ..
                } = entry
                {
                    if p == path {
                        found = Some(children.as_slice());
                        break;
                    }
                }
            }
            if let Some(children) = found {
                cur = children;
            } else {
                return &[];
            }
        }
        cur
    }

    /// Returns the breadcrumb string for the current local directory navigation state.
    pub fn current_local_path_display(&self) -> String {
        if self.local_nav_stack.is_empty() {
            "📁 Local Library".to_string()
        } else {
            let trail = self
                .local_nav_stack
                .iter()
                .map(|(_, name, _)| name.as_str())
                .collect::<Vec<_>>()
                .join(" / ");
            format!("📁 Local / {}", trail)
        }
    }

    /// Rebuild only the UI-visible flat library (respects current directory level and
    /// the current playlist's enqueue state).
    pub fn rebuild_flat_library_view(&mut self) {
        let entries = self.current_local_entries().to_vec();
        self.flat_library = entries
            .into_iter()
            .map(|entry| {
                let enqueued = entry.all_tracks_enqueued(&self.playlist.entry_ids);
                library::FlatLibraryItem {
                    entry,
                    depth: 0,
                    is_last: false,
                    ancestor_last: Vec::new(),
                    enqueued,
                }
            })
            .collect();
        if self.library_cursor > self.flat_library.len() {
            self.library_cursor = self.flat_library.len();
        }
    }

    /// Rebuild BOTH flat views. Called only when the raw library data changes
    /// (initial cache load or background scan completion).
    pub fn rebuild_flat_library(&mut self) {
        self.rebuild_flat_library_view();
        // full_flat_library: fully expanded, used for instant fuzzy search.
        self.full_flat_library = library::flatten_library(
            &self.library,
            0,
            Vec::new(),
            &std::collections::HashSet::new(),
            &self.playlist.entry_ids,
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
            // Rebuild playlist from saved entries if present, else fallback to legacy paths
            if !state.queue.is_empty() {
                for entry in state.queue {
                    if let Some(path) = entry.id.local_path() {
                        if path.exists() {
                            self.playlist.add(entry.id, entry.metadata);
                        }
                    } else {
                        // Remote tracks restored without network calls
                        self.playlist.add(entry.id, entry.metadata);
                    }
                }
            } else {
                use rayon::prelude::*;
                let items: Vec<_> = state
                    .playlist_paths
                    .par_iter()
                    .filter(|p| p.exists())
                    .map(|p| (p.clone(), metadata::read_metadata(p)))
                    .collect();
                for (path, meta) in items {
                    self.playlist.add(path, meta);
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
                    let start_pos = if state.position_ms > 0 {
                        Some(state.position_ms)
                    } else {
                        None
                    };
                    let load_ok = if let Some(path) = entry.id.local_path() {
                        if let Some(player) = self.player.as_mut() {
                            let ok = player.load_track_with_pos(path, start_pos).is_ok();
                            self.load_generation = player.generation;
                            ok
                        } else {
                            false
                        }
                    } else {
                        false
                    };

                    if load_ok {
                        self.stopped = false;
                        self.set_now_playing_meta();
                        self.load_lyrics_for_current();
                        self.generate_cover_art_protocol();
                        if let Some(player) = self.player.as_mut() {
                            if !state.was_playing {
                                player.pause(); // restore paused state
                            }
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
            queue: self.playlist.entries.clone(),
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
        let track_path = self
            .playlist
            .current_entry()
            .and_then(|e| e.id.local_path().map(|p| p.to_path_buf()));
        let title_key = self
            .playlist
            .current_entry()
            .and_then(|e| e.metadata.title.clone())
            .unwrap_or_else(|| "unknown".to_string());

        if let Some(path) = track_path {
            if let Some(cover_bytes) = metadata::read_cover_art(&path) {
                let reader = image::ImageReader::new(std::io::Cursor::new(&cover_bytes));
                let dyn_img = reader.with_guessed_format().ok().and_then(|mut r| {
                    let mut limits = image::Limits::default();
                    limits.max_image_width = Some(4096);
                    limits.max_image_height = Some(4096);
                    r.limits(limits);
                    r.decode().ok()
                });

                if let Some(dyn_img) = dyn_img {
                    // Resolve XDG/platform cache directory for cover art files
                    let cache_dir = directories::ProjectDirs::from("", "", "mixed")
                        .map(|p| p.cache_dir().to_path_buf())
                        .unwrap_or_else(|| {
                            let user =
                                std::env::var("USER").unwrap_or_else(|_| "default".to_string());
                            let dir = std::env::temp_dir().join(format!("mixed-{}", user));
                            let _ = std::fs::create_dir_all(&dir);
                            #[cfg(unix)]
                            {
                                use std::os::unix::fs::PermissionsExt;
                                let _ = std::fs::set_permissions(
                                    &dir,
                                    std::fs::Permissions::from_mode(0o700),
                                );
                            }
                            dir
                        });
                    let _ = std::fs::create_dir_all(&cache_dir);

                    let cover_input = format!("{}:{}", path.display(), title_key);
                    let cover_hash = crate::utils::hash::fnv1a_hex(&cover_input);
                    let tmp_path = cache_dir.join(format!("mixed_cover_{}.png", cover_hash));

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
        } else if let Some(cover_url) = self
            .playlist
            .current_entry()
            .and_then(|e| e.metadata.cover_url.as_deref())
            .map(crate::sources::youtube::full_size_cover)
        {
            let cover_hash = crate::utils::hash::fnv1a_hex(&cover_url);
            let cache_dir = directories::ProjectDirs::from("", "", "mixed")
                .map(|p| p.cache_dir().to_path_buf())
                .unwrap_or_else(std::env::temp_dir)
                .join("covers");
            let cached_path = cache_dir.join(format!("{}.jpg", cover_hash));
            if cached_path.exists() {
                if let Ok(dyn_img) = image::open(&cached_path) {
                    extracted_art_path = format!("file://{}", cached_path.display());
                    if let Some(ref picker) = self.picker {
                        self.current_cover_protocol = Some(picker.new_resize_protocol(dyn_img));
                    }
                }
            } else if let Some(ref rt) = self.source_runtime {
                let _ = rt.send(crate::sources::SourceRequest::FetchCover { url: cover_url });
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
        self.prefetched_video_id = None;
        self.prefetched_spotify_uri = None;
        self.streaming = None;
        self.lyrics_scroll = 0;
        self.pending_seek = None;
        self.last_seek_input = None;
        self.last_lyric_line = None;
        self.force_terminal_clear = true;
        let current_track = self.playlist.current_entry().map(|e| e.id.clone());
        if let Some(track_ref) = current_track {
            match track_ref {
                TrackRef::Local(path) => {
                    let mut gen = 0;
                    let load_res = self.player.as_mut().map(|player| {
                        let res = player.load_with_pos(path.clone(), None);
                        gen = player.generation;
                        res
                    });
                    self.load_generation = gen;

                    if let Some(res) = load_res {
                        match res {
                            Ok(()) => {
                                self.buffering = false;
                                self.set_now_playing_meta();
                                self.load_lyrics_for_current();
                                self.generate_cover_art_protocol();
                                self.push_mpris_metadata();
                                self.push_mpris_playback();

                                // Sync queue cursor with currently playing track
                                self.queue_cursor = self.playlist.current_real_index().unwrap_or(0);

                                // Send native desktop notification if enabled
                                if self.config.desktop_notifications {
                                    if let Some(ref meta) = self.now_playing_meta {
                                        let title =
                                            meta.display_title(self.config.strip_track_numbers);
                                        let artist = meta.display_artist();
                                        let album = meta.display_album();
                                        crate::sys::notifications::show_notification(
                                            title, artist, album,
                                        );
                                    }
                                }

                                // Update terminal title
                                if let Some(ref meta) = self.now_playing_meta {
                                    let title = format!(
                                        "mixed: {} - {}",
                                        meta.display_artist(),
                                        meta.display_title(self.config.strip_track_numbers)
                                    );
                                    let safe_title =
                                        crate::utils::sanitizer::sanitize_terminal_title(&title);
                                    print!("\x1b]0;{}\x07", safe_title);
                                    let _ = std::io::Write::flush(&mut std::io::stdout());
                                }

                                // Set duration from metadata if rodio didn't report it
                                let duration =
                                    self.player.as_ref().map(|p| p.duration_ms()).unwrap_or(0);
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
                                self.set_status(format!("Error: {}", e));
                            }
                        }
                    }
                }
                TrackRef::Spotify(uri) => {
                    self.set_now_playing_meta();
                    self.queue_cursor = self.playlist.current_real_index().unwrap_or(0);
                    if !self.spotify_view.connected {
                        if let Some(player) = self.player.as_mut() {
                            player.stop();
                        }
                        self.buffering = false;
                        self.set_status(
                            "Spotify not connected. Switch to Spotify (Ctrl+2/Alt+2) to log in.",
                        );
                        self.push_mpris_metadata();
                        self.push_mpris_playback();
                        self.refresh_needed = true;
                        return;
                    }
                    self.buffering = true;
                    if let Some(player) = self.player.as_mut() {
                        let dur_ms = self
                            .now_playing_meta
                            .as_ref()
                            .and_then(|m| m.duration)
                            .map(|d| d.as_millis() as u64);
                        let _ = player.load_with_pos_and_duration(
                            crate::audio::player::PlayInput::Spotify(uri.clone()),
                            None,
                            dur_ms,
                        );
                        self.load_generation = player.generation;
                    }
                    // Clears what the previous track showed and requests this track's
                    // lyrics and cover. Spotify's own lyrics need the session that the
                    // load above connects, so this comes after it.
                    self.load_lyrics_for_current();
                    self.generate_cover_art_protocol();
                    self.push_mpris_metadata();
                    self.push_mpris_playback();
                    self.refresh_needed = true;
                }
                TrackRef::YouTube(vid) => {
                    self.queue_cursor = self.playlist.current_real_index().unwrap_or(0);
                    self.set_now_playing_meta();
                    self.push_mpris_metadata();
                    self.push_mpris_playback();

                    if let Some(cached_path) = crate::sources::ytdlp::YtDlp::find_cached(&vid) {
                        self.load_youtube_audio(crate::audio::player::PlayInput::File(cached_path));
                    } else {
                        self.buffering = true;
                        let next_gen = if let Some(player) = self.player.as_mut() {
                            player.stop();
                            player.next_generation()
                        } else {
                            self.load_generation.wrapping_add(1)
                        };
                        self.load_generation = next_gen;
                        if let Some(ref runtime) = self.source_runtime {
                            let _ =
                                runtime.send(crate::sources::SourceRequest::DownloadYouTubeTrack {
                                    video_id: vid.clone(),
                                    generation: next_gen,
                                });
                        }
                    }
                    self.refresh_needed = true;
                }
            }
        } else {
            self.stop();
            self.now_playing_meta = None;
            self.current_cover_protocol = None;
            self.clear_lyrics();
            self.refresh_needed = true;
        }
    }

    /// After a stop nothing is loaded any more, so playing means starting the
    /// current track again. Returns false if playback was not stopped.
    fn restart_if_stopped(&mut self) -> bool {
        let restart = self.stopped && !self.playlist.is_empty();
        if restart {
            self.play_current();
            self.refresh_needed = true;
        }
        restart
    }

    pub fn play(&mut self) {
        if self.restart_if_stopped() {
            return;
        }
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
        self.lyrics_scroll = 0;
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
        self.force_terminal_clear = true;
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
        if self.restart_if_stopped() {
            return;
        }
        if let Some(p) = self.player.as_mut() {
            let was_paused = p.is_paused();
            p.toggle_pause();
            if was_paused {
                self.stopped = false;
            }
        };
        self.push_mpris_playback();
        self.refresh_needed = true;
    }

    /// Skip to the next track at the user's request. Unlike the end of a track,
    /// this leaves the last track of the queue playing.
    pub fn skip_to_next(&mut self) {
        if !self.playlist.is_empty() && !self.playlist.can_go_next() {
            self.set_status("This is the last track of the queue");
            return;
        }
        self.next_track();
    }

    pub fn next_track(&mut self) {
        if self.playlist.is_empty() {
            self.stop();
            print!("\x1b]0;mixed\x07");
            let _ = std::io::Write::flush(&mut std::io::stdout());
            self.refresh_needed = true;
            return;
        }

        if !self.playlist.advance_track() {
            self.stop();
            self.refresh_needed = true;
            return;
        }

        self.play_current();
        self.refresh_needed = true;
    }

    pub fn prev_track(&mut self) {
        if self.playlist.is_empty() {
            return;
        }
        // If track has been playing for more than 3 seconds, restart current track
        if self.display_elapsed_secs() > 3.0 {
            self.seek_to_ratio(0.0);
            self.refresh_needed = true;
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

    pub fn switch_source(&mut self, new_source: SourceTab) {
        if self.source == new_source {
            return;
        }

        // Save current panel and cursors
        let cur_idx = self.source.index();
        self.source_memory[cur_idx] = PanelMemory {
            panel: self.active_panel,
            queue_cursor: self.queue_cursor,
            library_cursor: self.library_cursor,
            search_cursor: self.search_cursor,
        };

        // Switch source
        self.source = new_source;

        // Restore panel and cursors from memory
        let new_idx = new_source.index();
        let mem = self.source_memory[new_idx];
        self.active_panel = mem.panel;
        self.queue_cursor = mem.queue_cursor;
        self.library_cursor = mem.library_cursor;
        self.search_cursor = mem.search_cursor;

        if self.searching {
            self.searching = false;
            self.search_query.clear();
        }

        if self.source == SourceTab::Unified
            && (self.active_panel == ActivePanel::Library
                || self.active_panel == ActivePanel::Search)
        {
            self.active_panel = ActivePanel::Queue;
        }

        self.refresh_needed = true;
    }

    pub fn next_panel(&mut self) {
        let tabs = tabs_for(self.source);
        if let Some(pos) = tabs.iter().position(|(_, _, p)| *p == self.active_panel) {
            let next_idx = (pos + 1) % tabs.len();
            self.active_panel = tabs[next_idx].2;
        } else if let Some(first) = tabs.first() {
            self.active_panel = first.2;
        }
        self.refresh_needed = true;
    }

    pub fn prev_panel(&mut self) {
        let tabs = tabs_for(self.source);
        if let Some(pos) = tabs.iter().position(|(_, _, p)| *p == self.active_panel) {
            let prev_idx = if pos == 0 { tabs.len() - 1 } else { pos - 1 };
            self.active_panel = tabs[prev_idx].2;
        } else if let Some(first) = tabs.first() {
            self.active_panel = first.2;
        }
        self.refresh_needed = true;
    }

    /// Tick update — called every 250 ms (progress bar + auto-advance).
    pub fn tick(&mut self) {
        // Read player state once per tick into locals — avoids calling self.player()
        // 4–6 times with redundant atomic loads and Option unwraps. (Item 4)
        let (is_playing, is_paused) = self
            .player()
            .map(|p| (p.is_playing(), p.is_paused()))
            .unwrap_or((false, false));
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

        // Track lyric line transition for clean repaint without cursor drift
        let active_lyric = self
            .current_lyrics
            .as_ref()
            .map(|l| l.find_active_line(self.display_elapsed_secs()));

        if active_lyric != self.last_lyric_line {
            self.last_lyric_line = active_lyric;
            self.refresh_needed = true;
            if self.show_full_lyrics {
                self.force_terminal_clear = true;
            }
        }

        // Auto-advance is handled event-driven via PlayerEvent::Finished (Phase 3)

        // Prefetch next YouTube track or preload next Spotify track if current track progress > 50%
        let duration = self.player.as_ref().map(|p| p.duration_ms()).unwrap_or(0);
        let elapsed = self.player.as_ref().map(|p| p.elapsed_ms()).unwrap_or(0);
        if duration > 0 && elapsed * 2 >= duration {
            if let Some(next_entry) = self.playlist.peek_next_entry() {
                match next_entry.id {
                    TrackRef::YouTube(ref next_vid) => {
                        if self.prefetched_video_id.as_deref() != Some(next_vid.as_str())
                            && crate::sources::ytdlp::YtDlp::find_cached(next_vid).is_none()
                        {
                            self.prefetched_video_id = Some(next_vid.clone());
                            if let Some(ref runtime) = self.source_runtime {
                                let _ = runtime.send(
                                    crate::sources::SourceRequest::DownloadYouTubeTrack {
                                        video_id: next_vid.clone(),
                                        generation: 0,
                                    },
                                );
                            }
                        }
                    }
                    TrackRef::Spotify(ref uri)
                        if self.prefetched_spotify_uri.as_deref() != Some(uri.as_str()) =>
                    {
                        self.prefetched_spotify_uri = Some(uri.clone());
                        if let Some(ref player) = self.player {
                            player.preload_spotify(uri.clone());
                        }
                    }
                    _ => {}
                }
            }
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

        // Auto-dismiss status message after 3 seconds (B1/Q1)
        if let Some(at) = self.status_msg_at {
            if at.elapsed().as_secs() >= 3 {
                self.status_msg = None;
                self.status_msg_at = None;
                self.refresh_needed = true;
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
                        if let Some(entry) = self.playlist.current_entry() {
                            let (url, id_suffix) = match &entry.id {
                                TrackRef::Local(p) => {
                                    let u = format!("file://{}", p.display());
                                    use std::hash::{Hash, Hasher};
                                    let mut hasher =
                                        std::collections::hash_map::DefaultHasher::new();
                                    p.hash(&mut hasher);
                                    (u, format!("local_{:016x}", hasher.finish()))
                                }
                                TrackRef::Spotify(uri) => {
                                    let id = uri.strip_prefix("spotify:track:").unwrap_or(uri);
                                    let u = format!("https://open.spotify.com/track/{}", id);
                                    let safe_id: String = id
                                        .chars()
                                        .map(|c| if c.is_alphanumeric() { c } else { '_' })
                                        .collect();
                                    (u, format!("spotify_{}", safe_id))
                                }
                                TrackRef::YouTube(id) => {
                                    let u = format!("https://music.youtube.com/watch?v={}", id);
                                    let safe_id: String = id
                                        .chars()
                                        .map(|c| if c.is_alphanumeric() { c } else { '_' })
                                        .collect();
                                    (u, format!("youtube_{}", safe_id))
                                }
                            };
                            s.url = url;
                            s.track_id = format!("/org/mpris/MediaPlayer2/Track/{}", id_suffix);
                        }
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

    // ── Android notification helpers ──────────────────────────────────────────

    /// Unified helper to toggle enqueuing a slice of tracks.
    /// If all tracks are already in the playlist, they are dequeued.
    /// Otherwise, any missing tracks are sorted by album and enqueued.
    pub fn toggle_enqueue_tracks(&mut self, tracks: &[(TrackRef, TrackMetadata)], play_now: bool) {
        if tracks.is_empty() {
            return;
        }

        let all_enqueued = tracks
            .iter()
            .all(|(id, _)| self.playlist.entry_ids.contains(id));
        let was_empty = self.playlist.is_empty();
        let old_len = self.playlist.len();

        if all_enqueued {
            // Dequeue: remove all matching tracks from the playlist
            let remove_set: std::collections::HashSet<_> =
                tracks.iter().map(|(id, _)| id.clone()).collect();
            let mut i = 0;
            let mut current_removed = false;
            while i < self.playlist.len() {
                if remove_set.contains(&self.playlist.entries[i].id) {
                    let is_current = Some(i) == self.playlist.current_real_index();
                    self.playlist.remove(i);
                    if is_current {
                        current_removed = true;
                    }
                } else {
                    i += 1;
                }
            }
            if self.playlist.is_empty() {
                self.clear_playlist();
                return;
            }
            if current_removed {
                self.play_current();
            }
            if self.queue_cursor >= self.playlist.len() && !self.playlist.is_empty() {
                self.queue_cursor = self.playlist.len() - 1;
            }
        } else {
            // Enqueue: add all tracks not already in the playlist
            let mut to_add: Vec<(TrackRef, TrackMetadata)> = tracks
                .iter()
                .filter(|(id, _)| !self.playlist.entry_ids.contains(id))
                .cloned()
                .collect();

            // Sort tracks by album (A1)
            crate::data::playlist::sort_tracks_by_album(&mut to_add);

            for (id, meta) in to_add {
                self.playlist.add(id, meta);
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
    }

    /// Enqueue the selected library entry.
    pub fn library_enqueue_selected(&mut self, play_now: bool) {
        self.refresh_needed = true;

        if self.library_cursor == 0 {
            // ".." or root: toggle all library tracks
            let all_tracks: Vec<(TrackRef, TrackMetadata)> = self
                .flat_library
                .iter()
                .filter_map(|item| {
                    if let LibraryEntry::Track { path, metadata, .. } = &item.entry {
                        Some((TrackRef::Local(path.clone()), metadata.clone()))
                    } else {
                        None
                    }
                })
                .collect();
            self.toggle_enqueue_tracks(&all_tracks, play_now);
            self.rebuild_flat_library_view();
            return;
        }

        let entry_idx = self.library_cursor - 1;
        if entry_idx >= self.flat_library.len() {
            return;
        }

        let entry = self.flat_library[entry_idx].entry.clone();
        match entry {
            LibraryEntry::Directory { .. } => {
                let tracks = entry.get_all_tracks();
                self.toggle_enqueue_tracks(&tracks, play_now);
            }
            LibraryEntry::Track { path, metadata, .. } => {
                let tracks = [(TrackRef::Local(path), metadata)];
                self.toggle_enqueue_tracks(&tracks, play_now);
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
        match entry {
            LibraryEntry::Directory { .. } => {
                let tracks = entry.get_all_tracks();
                self.toggle_enqueue_tracks(&tracks, play_now);
            }
            LibraryEntry::Track { path, metadata, .. } => {
                let tracks = [(TrackRef::Local(path), metadata)];
                self.toggle_enqueue_tracks(&tracks, play_now);
            }
        }
        self.rebuild_flat_library_view();
    }

    /// Navigate up one directory level ("cd .."). Returns true if handled.
    pub fn navigate_folder_up(&mut self) -> bool {
        if self.active_panel != ActivePanel::Library {
            return false;
        }
        if self.source == SourceTab::Local {
            if let Some((_, _, saved_cursor)) = self.local_nav_stack.pop() {
                self.rebuild_flat_library_view();
                self.library_cursor = saved_cursor.min(self.flat_library.len().max(1));
                self.refresh_needed = true;
                return true;
            }
        } else if self.source == SourceTab::Spotify || self.source == SourceTab::YouTube {
            if let Some(view) = self.active_source_view_mut() {
                if view.navigate_up() {
                    self.refresh_needed = true;
                    return true;
                }
            }
        }
        false
    }

    pub fn toggle_container_expansion(&mut self) -> bool {
        if self.active_panel == ActivePanel::Library && self.source == SourceTab::Local {
            if self.library_cursor > 0 {
                let idx = self.library_cursor - 1;
                if idx < self.flat_library.len() {
                    let entry = &self.flat_library[idx].entry;
                    if entry.is_dir() {
                        let path = entry.path().to_path_buf();
                        let name = entry.name().to_string();
                        // Drill down ("cd") into the local directory
                        self.local_nav_stack.push((path, name, self.library_cursor));
                        self.library_cursor = 1;
                        self.rebuild_flat_library_view();
                        self.refresh_needed = true;
                        return true;
                    }
                }
            }
        } else if (self.source == SourceTab::Spotify || self.source == SourceTab::YouTube)
            && (self.active_panel == ActivePanel::Library
                || self.active_panel == ActivePanel::Search)
        {
            let panel = self.active_panel;
            let item = match self.active_source_view_mut() {
                Some(view) => {
                    if panel == ActivePanel::Search {
                        view.search_results.get(view.search_cursor).cloned()
                    } else {
                        view.flat.get(view.cursor).cloned()
                    }
                }
                None => None,
            };

            if let Some(item) = item {
                if item.is_container {
                    if let Some(view) = self.active_source_view_mut() {
                        let in_cache = view.drill_down(&item);
                        if !in_cache {
                            if let Some(ref rt) = self.source_runtime {
                                let _ = rt.send(crate::sources::SourceRequest::FetchChildren {
                                    source: self.source,
                                    parent_id: item.id.clone(),
                                    page: 0,
                                });
                            }
                        }
                    }
                    self.refresh_needed = true;
                    return true;
                }
            }
        }
        false
    }

    /// Enqueue or interact with the selected remote item (Spotify or YouTube).
    pub fn remote_enqueue_selected(&mut self, play_now: bool, play_next: bool) {
        let panel = self.active_panel;
        let source = self.source;
        let (item, is_login_action) = match self.active_source_view_mut() {
            Some(view) => {
                if panel == ActivePanel::Search {
                    if source == SourceTab::Spotify && !view.connected {
                        self.set_status(
                            "Spotify not connected. Switch to Library (F3) to sign in.",
                        );
                        (None, false)
                    } else {
                        let item = view.search_results.get(view.search_cursor).cloned();
                        (item, false)
                    }
                } else if view.awaiting_login_input {
                    let input = view.login_input.trim().to_string();
                    view.awaiting_login_input = false;
                    view.loading = true;
                    // Signing in again (Spotify, with another Client ID) replaces the session
                    view.connected = false;
                    if let Some(ref rt) = self.source_runtime {
                        let _ = rt.send(crate::sources::SourceRequest::Login {
                            source,
                            payload: input,
                        });
                    }
                    (None, true)
                } else if !view.connected {
                    if source == SourceTab::Spotify {
                        // Immediately launch browser OAuth with default official client ID
                        view.loading = true;
                        if let Some(ref rt) = self.source_runtime {
                            let _ = rt.send(crate::sources::SourceRequest::Login {
                                source,
                                payload: String::new(),
                            });
                        }
                    } else {
                        view.awaiting_login_input = true;
                    }
                    (None, true)
                } else {
                    let item = view.flat.get(view.cursor).cloned();
                    (item, false)
                }
            }
            None => (None, false),
        };

        if is_login_action {
            self.refresh_needed = true;
            return;
        }

        if let Some(item) = item {
            if item.is_container {
                // If container tracks are cached, enqueue them immediately
                let tracks_in_cache: Vec<(TrackRef, TrackMetadata)> = self
                    .active_source_view()
                    .and_then(|v| v.cache.get(&item.id))
                    .map(|children| {
                        children
                            .iter()
                            .filter_map(|child| {
                                let tr = child.track_ref.clone()?;
                                let meta = TrackMetadata {
                                    title: Some(child.title.clone()),
                                    artist: child.subtitle.clone(),
                                    album: child.album.clone(),
                                    duration: child
                                        .duration_secs
                                        .map(std::time::Duration::from_secs),
                                    cover_url: child.artwork_url.clone(),
                                    ..Default::default()
                                };
                                Some((tr, meta))
                            })
                            .collect()
                    })
                    .unwrap_or_default();

                if !tracks_in_cache.is_empty() {
                    let count = tracks_in_cache.len();
                    self.toggle_enqueue_tracks(&tracks_in_cache, play_now);
                    self.set_status(format!("Enqueued {} tracks", count));
                } else {
                    self.set_status(format!("Loading tracks for {}...", item.title));
                    self.pending_container_enqueue = Some((item.id.clone(), play_now));
                    if let Some(ref rt) = self.source_runtime {
                        let _ = rt.send(crate::sources::SourceRequest::FetchChildren {
                            source: self.source,
                            parent_id: item.id.clone(),
                            page: 0,
                        });
                    }
                }
                self.refresh_needed = true;
            } else if let Some(tr) = item.track_ref {
                let meta = crate::data::metadata::TrackMetadata {
                    title: Some(item.title),
                    artist: item.subtitle,
                    album: item.album,
                    duration: item.duration_secs.map(std::time::Duration::from_secs),
                    cover_url: item.artwork_url.clone(),
                    ..Default::default()
                };
                if play_next {
                    let was_empty = self.playlist.is_empty();
                    if !self.playlist.entry_ids.contains(&tr) {
                        self.playlist.play_next(tr, meta);
                        self.set_status("Track queued next");
                        if was_empty && !self.playlist.is_empty() {
                            self.play_current();
                        }
                    }
                } else {
                    self.toggle_enqueue_tracks(&[(tr, meta)], play_now);
                }
            }
        }
        self.refresh_needed = true;
    }

    /// Run fuzzy search against the flat library.
    pub fn run_search(&mut self) {
        self.refresh_needed = true;
        if self.search_query.is_empty() {
            self.search_results.clear();
            self.search_cursor = 0;
            return;
        }

        // Reuse the pre-computed fully-expanded flat library with cached matcher (B5)
        let mut scored: Vec<(i64, LibraryEntry)> = self
            .full_flat_library
            .iter()
            .filter_map(|item| {
                self.fuzzy_matcher
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
            self.set_status("Invalid directory path");
        }
    }

    /// Halt playback and purge all playlist items.
    pub fn clear_playlist(&mut self) {
        self.playlist.clear();
        self.prefetched_video_id = None;
        self.prefetched_spotify_uri = None;
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

    /// Seek directly to a position specified as a ratio [0.0, 1.0] of total duration.
    pub fn seek_to_ratio(&mut self, ratio: f64) {
        let duration_ms = self.player.as_ref().map(|p| p.duration_ms()).unwrap_or(0);
        if duration_ms > 0 {
            let target_ms = (duration_ms as f64 * ratio.clamp(0.0, 1.0)) as u64;
            self.pending_seek = Some(std::time::Duration::from_millis(target_ms));
            self.last_seek_input = Some(std::time::Instant::now());
            self.refresh_needed = true;
        }
    }

    /// Launch web browser searching Google for the current artist: "Artist: <name>"
    pub fn search_artist_web(&self) {
        if let Some(ref meta) = self.now_playing_meta {
            let artist = meta.display_artist();
            if !artist.is_empty() && artist != "Unknown Artist" {
                let query = format!("Artist: {}", artist);
                let encoded = url_encode(&query);
                let url = format!("https://www.google.com/search?q={}", encoded);
                open_browser_url(url);
            }
        }
    }

    /// Launch web browser searching Google for the current song: "Song: <title> <artist>"
    pub fn search_song_web(&self) {
        if let Some(ref meta) = self.now_playing_meta {
            let title = meta.display_title(self.config.strip_track_numbers);
            let artist = meta.display_artist();
            let query = if !artist.is_empty() && artist != "Unknown Artist" {
                format!("Song: {} {}", title, artist)
            } else {
                format!("Song: {}", title)
            };
            let encoded = url_encode(&query);
            let url = format!("https://www.google.com/search?q={}", encoded);
            open_browser_url(url);
        }
    }
}

/// Helper to percent-encode query strings for search URLs.
fn url_encode(s: &str) -> String {
    let mut result = String::new();
    for b in s.bytes() {
        match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                result.push(b as char);
            }
            b' ' => result.push('+'),
            _ => {
                result.push_str(&format!("%{:02X}", b));
            }
        }
    }
    result
}

/// Open a URL in the user's default browser on a detached thread without blocking TUI or bleeding output.
fn open_browser_url(url: String) {
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return;
    }
    std::thread::spawn(move || {
        #[cfg(target_os = "linux")]
        {
            let _ = std::process::Command::new("xdg-open")
                .arg(&url)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn();
        }
        #[cfg(target_os = "macos")]
        {
            let _ = std::process::Command::new("open")
                .arg(&url)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn();
        }
        #[cfg(target_os = "windows")]
        {
            let _ = std::process::Command::new("rundll32")
                .args(["url.dll,FileProtocolHandler", &url])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_source_tab_indices_and_labels() {
        assert_eq!(SourceTab::Local.index(), 0);
        assert_eq!(SourceTab::Spotify.index(), 1);
        assert_eq!(SourceTab::YouTube.index(), 2);
        assert_eq!(SourceTab::Unified.index(), 3);

        assert_eq!(SourceTab::from_index(0), Some(SourceTab::Local));
        assert_eq!(SourceTab::from_index(1), Some(SourceTab::Spotify));
        assert_eq!(SourceTab::from_index(2), Some(SourceTab::YouTube));
        assert_eq!(SourceTab::from_index(3), Some(SourceTab::Unified));
        assert_eq!(SourceTab::from_index(4), None);

        assert_eq!(SourceTab::Local.label(), "local");
        assert_eq!(SourceTab::Spotify.label(), "spotify");
        assert_eq!(SourceTab::YouTube.label(), "youtube");
        assert_eq!(SourceTab::Unified.label(), "queue");
    }

    #[test]
    fn test_tabs_for_sources() {
        let local_tabs = tabs_for(SourceTab::Local);
        assert_eq!(local_tabs.len(), 5);

        let unified_tabs = tabs_for(SourceTab::Unified);
        assert_eq!(unified_tabs.len(), 3);
        assert_eq!(unified_tabs[0].1, "Queue");
        assert_eq!(unified_tabs[1].1, "Track");
        assert_eq!(unified_tabs[2].1, "Help");
    }
}
