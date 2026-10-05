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

/// How often a track is loaded before it is reported as failed. On a connection
/// that loses packets, librespot's request for the decryption key often times out
/// (it waits 1.5 s), and asking again usually succeeds.
#[cfg(feature = "spotify")]
const MAX_LOAD_ATTEMPTS: u32 = 3;

/// How long a single Spotify access-point login may take before the load fails.
#[cfg(feature = "spotify")]
const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

/// A track URI and the lyrics Spotify has for it, if any.
pub type LyricsAnswer = (String, Option<String>);

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
    /// Loads of the active track so far, and the position they start from.
    #[cfg(feature = "spotify")]
    load_attempts: u32,
    #[cfg(feature = "spotify")]
    start_position_ms: u32,
    /// Answers to `fetch_lyrics`, not yet consumed: track URI and its lyrics, if any.
    #[cfg(feature = "spotify")]
    lyrics: Arc<Mutex<Vec<LyricsAnswer>>>,
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

/// How long Spotify may take to answer a lyrics request.
#[cfg(feature = "spotify")]
const LYRICS_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Convert the answer of Spotify's lyrics service to LRC text, or to plain text
/// if the lyrics are not synced. `None` if it holds no lyrics.
#[cfg(feature = "spotify")]
fn lrc_from_spotify(json: &[u8]) -> Option<String> {
    let value: serde_json::Value = serde_json::from_slice(json).ok()?;
    let lyrics = value.get("lyrics")?;
    let synced = lyrics.get("syncType")?.as_str()? == "LINE_SYNCED";
    let mut text = String::new();
    for line in lyrics.get("lines")?.as_array()? {
        let words = line.get("words")?.as_str()?;
        if synced {
            let ms: u64 = line.get("startTimeMs")?.as_str()?.parse().ok()?;
            text.push_str(&format!(
                "[{:02}:{:02}.{:02}]",
                ms / 60_000,
                ms / 1000 % 60,
                ms / 10 % 100
            ));
        }
        text.push_str(words);
        text.push('\n');
    }
    (!text.trim().is_empty()).then_some(text)
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
        let is_active = |track_id: &SpotifyUri| {
            active_track_for_events
                .lock()
                .is_ok_and(|cur| cur.as_ref() == Some(track_id))
        };
        while let Some(event) = events.recv().await {
            match event {
                PlayerEvent::EndOfTrack { track_id, .. } => {
                    if !is_active(&track_id) {
                        log::debug!("Ignoring EndOfTrack for non-active track {:?}", track_id);
                        continue;
                    }
                    if let Ok(w) = writer_for_events.lock() {
                        w.set_ended(true);
                    }
                }
                // Audio starts (or resumes, or jumps, or advances) at this position: the UI clock
                // is synchronised to it instead of running from the load request.
                PlayerEvent::Playing {
                    track_id,
                    position_ms,
                    ..
                }
                | PlayerEvent::Seeked {
                    track_id,
                    position_ms,
                    ..
                }
                | PlayerEvent::PositionCorrection {
                    track_id,
                    position_ms,
                    ..
                }
                | PlayerEvent::PositionChanged {
                    track_id,
                    position_ms,
                    ..
                } => {
                    // A late report of the previous track says nothing about this one
                    if is_active(&track_id) {
                        if let Ok(mut p) = position_for_events.lock() {
                            *p = Some(u64::from(position_ms));
                        }
                    }
                }
                PlayerEvent::Unavailable { track_id, .. } => {
                    // Also sent when preloading the next track fails, which must not
                    // end the track that is playing: it is loaded again when its turn comes.
                    if !is_active(&track_id) {
                        log::debug!("Ignoring failed preload of {:?}", track_id);
                        continue;
                    }
                    log::warn!("Spotify player could not load track {:?}", track_id);
                    if let Ok(mut f) = failure_for_events.lock() {
                        *f = Some(
                            "Spotify could not load the track (unavailable, or the connection to Spotify timed out)"
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
                load_attempts: 0,
                start_position_ms: 0,
                lyrics: Arc::default(),
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

    /// Take the pending playback failure reported by librespot, if any. A track
    /// that failed to load is first loaded again, up to `MAX_LOAD_ATTEMPTS` times.
    pub fn take_failure(&mut self) -> Option<String> {
        #[cfg(feature = "spotify")]
        {
            let failure = self.failure.lock().ok().and_then(|mut f| f.take())?;
            let track = self.active_track.lock().ok().and_then(|cur| cur.clone());
            if let (Some(player), Some(track)) = (self.player.as_ref(), track) {
                if self.load_attempts < MAX_LOAD_ATTEMPTS {
                    self.load_attempts += 1;
                    log::warn!(
                        "Loading Spotify track again (attempt {} of {})",
                        self.load_attempts,
                        MAX_LOAD_ATTEMPTS
                    );
                    player.load(track, true, self.start_position_ms);
                    return None;
                }
            }
            Some(failure)
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

    /// Ask Spotify for the lyrics of a track. The answer is picked up with `take_lyrics`
    /// and holds no lyrics if Spotify has none or cannot be asked.
    pub fn fetch_lyrics(&self, uri: &str) {
        #[cfg(feature = "spotify")]
        {
            let answers = self.lyrics.clone();
            let uri = uri.to_string();
            let answer = move |uri: String, lyrics: Option<String>| {
                if let Ok(mut answers) = answers.lock() {
                    answers.push((uri, lyrics));
                }
            };

            let session = self.session.clone().filter(|_| self.connected);
            let id = SpotifyUri::from_uri(&uri)
                .ok()
                .and_then(|u| librespot_core::SpotifyId::try_from(&u).ok());
            let (Some(rt), Some(session), Some(id)) = (self.rt.as_ref(), session, id) else {
                answer(uri, None);
                return;
            };
            rt.spawn(async move {
                let request = session.spclient().get_lyrics(&id);
                let lyrics = match tokio::time::timeout(LYRICS_TIMEOUT, request).await {
                    Ok(Ok(json)) => lrc_from_spotify(&json),
                    Ok(Err(e)) => {
                        log::debug!("No Spotify lyrics for {}: {}", uri, e);
                        None
                    }
                    Err(_) => {
                        log::warn!("Spotify lyrics request for {} timed out", uri);
                        None
                    }
                };
                answer(uri, lyrics);
            });
        }
        #[cfg(not(feature = "spotify"))]
        {
            let _ = uri;
        }
    }

    /// Take the answers to `fetch_lyrics` that have arrived: track URI and lyrics.
    pub fn take_lyrics(&mut self) -> Vec<LyricsAnswer> {
        #[cfg(feature = "spotify")]
        {
            self.lyrics
                .lock()
                .map(|mut answers| std::mem::take(&mut *answers))
                .unwrap_or_default()
        }
        #[cfg(not(feature = "spotify"))]
        {
            Vec::new()
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

            self.load_attempts = 1;
            self.start_position_ms = position_ms;

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

#[cfg(all(test, feature = "spotify"))]
mod tests {
    use super::*;

    #[test]
    fn spotify_lyrics_become_lrc_or_plain_text() {
        let synced = br#"{"lyrics":{"syncType":"LINE_SYNCED","lines":[
            {"startTimeMs":"12650","words":"first","syllables":[],"endTimeMs":"0"},
            {"startTimeMs":"83370","words":"second","syllables":[],"endTimeMs":"0"}]}}"#;
        assert_eq!(
            lrc_from_spotify(synced).as_deref(),
            Some("[00:12.65]first\n[01:23.37]second\n")
        );

        let unsynced = br#"{"lyrics":{"syncType":"UNSYNCED","lines":[
            {"startTimeMs":"0","words":"just words"}]}}"#;
        assert_eq!(lrc_from_spotify(unsynced).as_deref(), Some("just words\n"));

        assert_eq!(
            lrc_from_spotify(br#"{"lyrics":{"syncType":"UNSYNCED","lines":[]}}"#),
            None
        );
        assert_eq!(lrc_from_spotify(b"not json"), None);
    }
}
