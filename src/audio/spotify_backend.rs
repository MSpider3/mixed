use crate::audio::pcm_source::PcmSource;
#[cfg(feature = "spotify")]
use crate::audio::pcm_source::PcmWriter;
#[cfg(feature = "spotify")]
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(feature = "spotify")]
use std::sync::{Arc, Mutex};

#[cfg(feature = "spotify")]
use librespot_core::authentication::Credentials;
#[cfg(feature = "spotify")]
use librespot_core::cache::Cache;
#[cfg(feature = "spotify")]
use librespot_core::config::SessionConfig;
#[cfg(feature = "spotify")]
use librespot_core::session::Session;
#[cfg(feature = "spotify")]
use librespot_core::spotify_uri::SpotifyUri;
#[cfg(feature = "spotify")]
use librespot_playback::audio_backend::{Sink, SinkResult};
#[cfg(feature = "spotify")]
use librespot_playback::config::PlayerConfig;
#[cfg(feature = "spotify")]
use librespot_playback::convert::Converter;
#[cfg(feature = "spotify")]
use librespot_playback::decoder::AudioPacket;
#[cfg(feature = "spotify")]
use librespot_playback::mixer::NoOpVolume;
#[cfg(feature = "spotify")]
use librespot_playback::player::Player as LibrespotPlayer;

#[cfg(feature = "spotify")]
const SPOTIFY_BUFFER_CAPACITY: usize = 131_072;

#[cfg(feature = "spotify")]
struct LibrespotPcmSink {
    writer: Arc<Mutex<PcmWriter>>,
    stopped: Arc<AtomicBool>,
}

#[cfg(feature = "spotify")]
impl Sink for LibrespotPcmSink {
    fn start(&mut self) -> SinkResult<()> {
        self.stopped.store(false, Ordering::Release);
        Ok(())
    }

    fn stop(&mut self) -> SinkResult<()> {
        self.stopped.store(true, Ordering::Release);
        Ok(())
    }

    fn write(&mut self, packet: AudioPacket, _converter: &mut Converter) -> SinkResult<()> {
        if let AudioPacket::Samples(samples) = packet {
            let mut idx = 0;
            let mut batch = [0.0f32; 512];
            while idx < samples.len() && !self.stopped.load(Ordering::Acquire) {
                let chunk_size = (samples.len() - idx).min(batch.len());
                for i in 0..chunk_size {
                    batch[i] = samples[idx + i] as f32;
                }
                let written = if let Ok(writer) = self.writer.lock() {
                    writer.push_slice(&batch[..chunk_size])
                } else {
                    break;
                };
                idx += written;
                if written < chunk_size {
                    // Backpressure: sleep briefly to let the audio engine consume samples
                    std::thread::sleep(std::time::Duration::from_millis(2));
                }
            }
        }
        Ok(())
    }
}

/// How long a single Spotify access-point login may take before the load fails.
#[cfg(feature = "spotify")]
const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

pub struct SpotifyBackend {
    #[cfg(feature = "spotify")]
    rt: Option<tokio::runtime::Runtime>,
    #[cfg(feature = "spotify")]
    pub session: Option<Session>,
    #[cfg(feature = "spotify")]
    pub player: Option<Arc<LibrespotPlayer>>,
    #[cfg(feature = "spotify")]
    writer_holder: Arc<Mutex<PcmWriter>>,
    #[cfg(feature = "spotify")]
    stopped: Arc<AtomicBool>,
    /// True once `Session::connect` has succeeded on the current session.
    #[cfg(feature = "spotify")]
    connected: bool,
    /// Set by the librespot event task when the loaded track cannot be played.
    #[cfg(feature = "spotify")]
    failure: Arc<Mutex<Option<String>>>,
    /// Latest playback position (ms) reported by librespot, not yet consumed.
    #[cfg(feature = "spotify")]
    position: Arc<Mutex<Option<u64>>>,
    /// Currently loaded track URI, guarding against race conditions in EndOfTrack.
    #[cfg(feature = "spotify")]
    active_track: Arc<Mutex<Option<SpotifyUri>>>,
    pcm_source: Option<PcmSource>,
}

impl Default for SpotifyBackend {
    fn default() -> Self {
        Self::new()
    }
}

/// Upper bound for the audio files librespot keeps on disk.
#[cfg(feature = "spotify")]
const AUDIO_CACHE_LIMIT: u64 = 1024 * 1024 * 1024;

#[cfg(feature = "spotify")]
fn cache_dir() -> std::path::PathBuf {
    let mut dir = directories::ProjectDirs::from("", "", "mixed")
        .map(|d| d.cache_dir().to_path_buf())
        .unwrap_or_else(std::env::temp_dir);
    dir.push("spotify");
    dir
}

/// librespot's cache: its reusable login in `cache_dir()` and, with `audio`,
/// downloaded audio in a subfolder of its own (the size limit prunes that folder).
#[cfg(feature = "spotify")]
fn open_cache(audio: bool) -> Option<Cache> {
    let dir = cache_dir();
    let _ = std::fs::create_dir_all(&dir);
    let audio_dir = audio.then(|| dir.join("files"));
    Cache::new(Some(dir), None, audio_dir, Some(AUDIO_CACHE_LIMIT)).ok()
}

#[cfg(feature = "spotify")]
fn session_config() -> SessionConfig {
    SessionConfig {
        client_id: crate::sources::spotify::SPOTIFY_STREAMING_CLIENT_ID.to_string(),
        ..Default::default()
    }
}

/// True if a playback sign-in is stored (see `store_credentials`).
#[cfg(feature = "spotify")]
pub fn has_stored_credentials() -> bool {
    open_cache(false).and_then(|c| c.credentials()).is_some()
}

/// Sign librespot in with an access token of the streaming client ID. librespot
/// stores the reusable credentials it gets back, and playback connects with those.
#[cfg(feature = "spotify")]
pub async fn store_credentials(access_token: &str) -> Result<(), String> {
    let cache = open_cache(false).ok_or("Cannot open the Spotify cache directory")?;
    let session = Session::new(session_config(), Some(cache));
    let connect = session.connect(Credentials::with_access_token(access_token), true);
    let result = tokio::time::timeout(CONNECT_TIMEOUT, connect).await;
    session.shutdown();
    match result {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(format!("Spotify playback sign-in failed: {}", e)),
        Err(_) => Err("Spotify playback sign-in timed out".into()),
    }
}

/// Build a librespot session and player bound to `rt`, plus the task that maps
/// librespot player events onto the PCM writer and the failure slot.
#[cfg(feature = "spotify")]
fn build_session_and_player(
    rt: &tokio::runtime::Runtime,
    writer_holder: &Arc<Mutex<PcmWriter>>,
    stopped: &Arc<AtomicBool>,
    failure: &Arc<Mutex<Option<String>>>,
    position: &Arc<Mutex<Option<u64>>>,
    active_track: &Arc<Mutex<Option<SpotifyUri>>>,
) -> (Session, Arc<LibrespotPlayer>) {
    // Session::new captures the current Tokio handle, so it must run inside the runtime.
    let _guard = rt.enter();

    let session = Session::new(session_config(), open_cache(true));

    let writer_clone = writer_holder.clone();
    let stopped_clone = stopped.clone();
    let player_config = PlayerConfig {
        bitrate: librespot_playback::config::Bitrate::Bitrate320,
        gapless: true,
        position_update_interval: Some(std::time::Duration::from_millis(500)),
        ..Default::default()
    };
    let player = LibrespotPlayer::new(
        player_config,
        session.clone(),
        Box::new(NoOpVolume),
        move || -> Box<dyn Sink> {
            Box::new(LibrespotPcmSink {
                writer: writer_clone.clone(),
                stopped: stopped_clone.clone(),
            })
        },
    );

    let writer_for_events = writer_holder.clone();
    let failure_for_events = failure.clone();
    let position_for_events = position.clone();
    let active_track_for_events = active_track.clone();
    let mut events = player.get_player_event_channel();
    rt.spawn(async move {
        use librespot_playback::player::PlayerEvent;
        while let Some(event) = events.recv().await {
            match event {
                PlayerEvent::EndOfTrack { track_id, .. } => {
                    if let Ok(cur) = active_track_for_events.lock() {
                        if cur.as_ref() != Some(&track_id) {
                            log::debug!("Ignoring EndOfTrack for non-active track {:?}", track_id);
                            continue;
                        }
                    }
                    if let Ok(w) = writer_for_events.lock() {
                        w.set_ended(true);
                    }
                }
                // Audio starts (or resumes, or jumps, or advances) at this position: the UI clock
                // is synchronised to it instead of running from the load request.
                PlayerEvent::Playing { position_ms, .. }
                | PlayerEvent::Seeked { position_ms, .. }
                | PlayerEvent::PositionCorrection { position_ms, .. }
                | PlayerEvent::PositionChanged { position_ms, .. } => {
                    if let Ok(mut p) = position_for_events.lock() {
                        *p = Some(u64::from(position_ms));
                    }
                }
                PlayerEvent::Unavailable { track_id, .. } => {
                    log::warn!("Spotify player reported track unavailable: {:?}", track_id);
                    if let Ok(mut f) = failure_for_events.lock() {
                        *f = Some(
                            "Track is unavailable on Spotify (regional or format restriction)"
                                .to_string(),
                        );
                    }
                }
                PlayerEvent::Stopped { .. } => {
                    log::debug!("Spotify player stopped");
                }
                PlayerEvent::Loading { track_id, .. } => {
                    log::debug!("Spotify player loading track {:?}", track_id);
                }
                _ => {}
            }
        }
    });

    (session, player)
}

impl SpotifyBackend {
    pub fn new() -> Self {
        #[cfg(feature = "spotify")]
        let (source, _writer) = PcmSource::new(2, 44100, SPOTIFY_BUFFER_CAPACITY);
        #[cfg(not(feature = "spotify"))]
        let (source, _writer) = PcmSource::new(2, 44100, 32768);

        #[cfg(feature = "spotify")]
        {
            // Dedicated single-worker Tokio runtime for librespot internals
            let rt = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .enable_all()
                .thread_name("mixed-spotify-rt")
                .build()
                .ok();

            let writer_holder = Arc::new(Mutex::new(_writer));
            let stopped = Arc::new(AtomicBool::new(false));
            let failure = Arc::new(Mutex::new(None));
            let position = Arc::new(Mutex::new(None));
            let active_track = Arc::new(Mutex::new(None));

            let (session, player) = match rt.as_ref() {
                Some(rt) => {
                    let (s, p) = build_session_and_player(
                        rt,
                        &writer_holder,
                        &stopped,
                        &failure,
                        &position,
                        &active_track,
                    );
                    (Some(s), Some(p))
                }
                None => (None, None),
            };

            Self {
                rt,
                session,
                player,
                writer_holder,
                stopped,
                connected: false,
                failure,
                position,
                active_track,
                pcm_source: Some(source),
            }
        }
        #[cfg(not(feature = "spotify"))]
        {
            Self {
                pcm_source: Some(source),
            }
        }
    }

    /// Connect the librespot session if it is not connected yet. Blocks the calling
    /// (player) thread for at most `CONNECT_TIMEOUT`.
    #[cfg(feature = "spotify")]
    fn ensure_connected(&mut self) -> Result<(), String> {
        let Some(rt) = self.rt.as_ref() else {
            return Err("Spotify runtime failed to start".into());
        };

        // A session that dropped its connection cannot be reused: rebuild it.
        if self.session.as_ref().is_none_or(|s| s.is_invalid()) {
            let (session, player) = build_session_and_player(
                rt,
                &self.writer_holder,
                &self.stopped,
                &self.failure,
                &self.position,
                &self.active_track,
            );
            self.session = Some(session);
            self.player = Some(player);
            self.connected = false;
        }
        if self.connected {
            return Ok(());
        }
        let Some(session) = self.session.clone() else {
            return Err("Spotify session not initialized".into());
        };

        let Some(creds) = session.cache().and_then(|c| c.credentials()) else {
            log::warn!("No stored Spotify playback sign-in");
            return Err(
                "Spotify playback is not signed in. Sign in from the Spotify Library tab.".into(),
            );
        };

        let res = rt.block_on(async move {
            tokio::time::timeout(CONNECT_TIMEOUT, session.connect(creds, true)).await
        });
        let err = match res {
            Ok(Ok(())) => {
                log::info!("Spotify streaming session connected");
                self.connected = true;
                return Ok(());
            }
            Ok(Err(e)) if e.kind == librespot_core::error::ErrorKind::PermissionDenied => {
                // Spotify rejected the stored sign-in: forget it, so that the user
                // is asked to sign in again instead of failing the same way.
                let _ = std::fs::remove_file(cache_dir().join("credentials.json"));
                format!(
                    "Spotify playback sign-in was rejected ({}). Sign in again from the Spotify Library tab.",
                    e
                )
            }
            Ok(Err(e)) => format!("Spotify connection failed: {}", e),
            Err(_) => "Spotify connection failed: connection timed out".to_string(),
        };
        log::error!("{}", err);
        // A session can only be connected once: the next attempt needs a new one.
        self.session = None;
        self.player = None;
        Err(err)
    }

    /// True if playback cannot connect until the user signs in again.
    pub fn needs_sign_in(&self) -> bool {
        #[cfg(feature = "spotify")]
        {
            !has_stored_credentials()
        }
        #[cfg(not(feature = "spotify"))]
        {
            false
        }
    }

    /// Take the pending playback failure reported by librespot, if any.
    pub fn take_failure(&mut self) -> Option<String> {
        #[cfg(feature = "spotify")]
        {
            self.failure.lock().ok().and_then(|mut f| f.take())
        }
        #[cfg(not(feature = "spotify"))]
        {
            None
        }
    }

    /// Take the latest position reported by librespot, corrected for the samples
    /// still queued in the ring buffer (decoded but not yet audible).
    pub fn take_position_ms(&mut self) -> Option<u64> {
        #[cfg(feature = "spotify")]
        {
            let pos = self.position.lock().ok().and_then(|mut p| p.take())?;
            let queued = self.writer_holder.lock().map(|w| w.len()).unwrap_or(0) as u64;
            // 44.1 kHz stereo: 88.2 samples per millisecond
            Some(pos.saturating_sub(queued * 10 / 882))
        }
        #[cfg(not(feature = "spotify"))]
        {
            None
        }
    }

    /// Return the source prepared by `load`, or construct a new one wired to the writer.
    pub fn get_or_create_source(&mut self) -> PcmSource {
        if let Some(source) = self.pcm_source.take() {
            source
        } else {
            #[cfg(feature = "spotify")]
            let (source, writer) = PcmSource::new(2, 44100, SPOTIFY_BUFFER_CAPACITY);
            #[cfg(not(feature = "spotify"))]
            let (source, writer) = PcmSource::new(2, 44100, 32768);

            #[cfg(feature = "spotify")]
            if let Ok(mut w) = self.writer_holder.lock() {
                *w = writer;
            }
            #[cfg(not(feature = "spotify"))]
            let _ = writer;
            source
        }
    }

    pub fn preload(&self, uri: &str) {
        #[cfg(feature = "spotify")]
        {
            if let Some(ref player) = self.player {
                let track_uri = if uri.starts_with("spotify:track:") {
                    SpotifyUri::from_uri(uri)
                } else {
                    SpotifyUri::from_uri(&format!("spotify:track:{}", uri))
                };
                if let Ok(uri) = track_uri {
                    log::debug!("Preloading next Spotify track: {:?}", uri);
                    player.preload(uri);
                }
            }
        }
        #[cfg(not(feature = "spotify"))]
        {
            let _ = uri;
        }
    }

    pub fn load(&mut self, uri: &str, position_ms: u32) -> Result<(), String> {
        #[cfg(feature = "spotify")]
        {
            let track_uri = if uri.starts_with("spotify:track:") {
                SpotifyUri::from_uri(uri)
            } else {
                SpotifyUri::from_uri(&format!("spotify:track:{}", uri))
            }
            .map_err(|e| format!("Invalid Spotify URI: {:?}", e))?;

            self.ensure_connected()?;

            // Fresh source/writer pair for this track load
            let (new_source, new_writer) = PcmSource::new(2, 44100, SPOTIFY_BUFFER_CAPACITY);
            if let Ok(mut w) = self.writer_holder.lock() {
                w.flush();
                *w = new_writer;
            }
            self.pcm_source = Some(new_source);
            self.stopped.store(false, Ordering::Release);
            if let Ok(mut f) = self.failure.lock() {
                *f = None;
            }
            if let Ok(mut p) = self.position.lock() {
                *p = None;
            }
            if let Ok(mut cur) = self.active_track.lock() {
                *cur = Some(track_uri.clone());
            }

            let Some(ref player) = self.player else {
                return Err("Spotify player not initialized".into());
            };
            player.load(track_uri, true, position_ms);
            Ok(())
        }
        #[cfg(not(feature = "spotify"))]
        {
            let _ = (uri, position_ms);
            Err("Spotify feature not enabled in build".into())
        }
    }

    pub fn pause(&self) {
        #[cfg(feature = "spotify")]
        if let Some(ref player) = self.player {
            player.pause();
        }
    }

    pub fn resume(&self) {
        #[cfg(feature = "spotify")]
        if let Some(ref player) = self.player {
            player.play();
        }
    }

    pub fn seek(&mut self, position_ms: u32) {
        #[cfg(feature = "spotify")]
        {
            if let Ok(w) = self.writer_holder.lock() {
                w.flush();
            }
            if let Some(ref player) = self.player {
                player.seek(position_ms);
            }
        }
        #[cfg(not(feature = "spotify"))]
        {
            let _ = position_ms;
        }
    }

    pub fn stop(&mut self) {
        #[cfg(feature = "spotify")]
        {
            self.stopped.store(true, Ordering::Release);
            if let Ok(mut cur) = self.active_track.lock() {
                *cur = None;
            }
            if let Ok(w) = self.writer_holder.lock() {
                w.flush();
            }
            if let Some(ref player) = self.player {
                player.stop();
            }
        }
    }
}
