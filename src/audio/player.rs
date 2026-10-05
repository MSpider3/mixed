use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;

use crossbeam_channel::{bounded, Receiver, Sender};

use crate::audio::rodio_backend::RodioBackend;
use crate::audio::viz_source::SharedSampleBuffer;

/// Media input types that the audio player can load and play.
#[derive(Debug, Clone)]
pub enum PlayInput {
    File(PathBuf),
    /// A partial file that a downloader is still appending to.
    Growing {
        path: PathBuf,
        progress: Arc<crate::audio::growing_file::DownloadProgress>,
    },
    Spotify(String),
}

impl From<PathBuf> for PlayInput {
    fn from(p: PathBuf) -> Self {
        PlayInput::File(p)
    }
}

impl From<&Path> for PlayInput {
    fn from(p: &Path) -> Self {
        PlayInput::File(p.to_path_buf())
    }
}

/// Events produced by the audio playback thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlayerEvent {
    Loaded {
        generation: u64,
    },
    Buffering {
        generation: u64,
    },
    Failed {
        generation: u64,
        error: String,
    },
    Finished {
        generation: u64,
    },
    Position {
        generation: u64,
        position_ms: u64,
    },
    /// Spotify rejected the stored playback sign-in; the user has to sign in again.
    SpotifySignInLost,
}

/// Commands sent to the background player thread.
pub enum PlayerCmd {
    Load {
        input: PlayInput,
        start_pos_ms: Option<u64>,
        duration_ms: Option<u64>,
        generation: u64,
    },
    Play,
    Pause,
    Seek(u64),
    Stop,
    SetVolume(u8),
    PreloadSpotify(String),
}

/// Unified audio backend router.
pub struct AudioBackend(RodioBackend);

impl AudioBackend {
    pub fn new(sample_buffer: SharedSampleBuffer) -> Option<Self> {
        RodioBackend::new(sample_buffer).map(AudioBackend)
    }

    pub fn play(&mut self, path: &str) -> Result<(), String> {
        self.0.play(path)
    }

    pub fn play_growing(
        &mut self,
        path: &Path,
        progress: Arc<crate::audio::growing_file::DownloadProgress>,
        fallback_duration: Option<std::time::Duration>,
    ) -> Result<(), String> {
        self.0.play_growing(path, progress, fallback_duration)
    }

    pub fn play_pcm_source(
        &mut self,
        source: crate::audio::pcm_source::PcmSource,
        duration: Option<std::time::Duration>,
    ) -> Result<(), String> {
        self.0.play_pcm_source(source, duration)
    }

    pub fn pause(&mut self) {
        self.0.pause();
    }

    pub fn resume(&mut self) {
        self.0.resume();
    }

    pub fn seek_to(&mut self, target: std::time::Duration) {
        self.0.seek_to(target);
    }

    pub fn get_position(&mut self) -> std::time::Duration {
        self.0.get_position()
    }

    pub fn sync_position(&mut self, pos_ms: u64) {
        self.0.sync_position(pos_ms);
    }

    pub fn stop(&mut self) {
        self.0.stop();
    }

    pub fn set_volume(&mut self, volume: u8) {
        self.0.set_volume(volume);
    }

    pub fn get_volume(&mut self) -> Option<u8> {
        self.0.get_volume()
    }

    pub fn get_duration(&mut self) -> Option<std::time::Duration> {
        self.0.get_duration()
    }

    pub fn is_finished(&mut self) -> bool {
        self.0.is_finished()
    }

    pub fn current_sample_rate(&self) -> u32 {
        self.0.current_sample_rate
    }
}

/// Thread-isolated audio player wrapping AudioBackend.
pub struct Player {
    cmd_tx: Sender<PlayerCmd>,
    pub event_rx: Receiver<PlayerEvent>,
    pub sample_buffer: SharedSampleBuffer,
    pub generation: u64,

    // Shared atomic states.
    pub is_paused: Arc<AtomicBool>,
    pub is_playing: Arc<AtomicBool>,
    is_finished: Arc<AtomicBool>,
    volume: Arc<AtomicU8>,
    pub elapsed_ms: Arc<AtomicU64>,
    total_duration_ms: Arc<AtomicU64>,
    current_sample_rate: Arc<AtomicU32>,
}

impl Player {
    pub fn new() -> Option<Self> {
        let (cmd_tx, cmd_rx) = bounded::<PlayerCmd>(100);
        let (event_tx, event_rx) = bounded::<PlayerEvent>(100);

        let is_paused = Arc::new(AtomicBool::new(false));
        let is_playing = Arc::new(AtomicBool::new(false));
        let is_finished = Arc::new(AtomicBool::new(true));
        let volume = Arc::new(AtomicU8::new(80));
        let elapsed_ms = Arc::new(AtomicU64::new(0));
        let total_duration_ms = Arc::new(AtomicU64::new(0));
        let current_sample_rate = Arc::new(AtomicU32::new(44100));

        let sample_buffer = crate::audio::viz_source::new_shared_buffer(4096);

        let is_paused_clone = is_paused.clone();
        let is_playing_clone = is_playing.clone();
        let is_finished_clone = is_finished.clone();
        let volume_clone = volume.clone();
        let elapsed_ms_clone = elapsed_ms.clone();
        let total_duration_ms_clone = total_duration_ms.clone();
        let current_sample_rate_clone = current_sample_rate.clone();
        let sample_buffer_clone = sample_buffer.clone();

        std::thread::spawn(move || {
            let mut backend = match AudioBackend::new(sample_buffer_clone) {
                Some(b) => b,
                None => return,
            };

            // Sync initial volume
            backend.set_volume(80);
            let mut current_generation = 0u64;
            let mut was_finished = true;

            let mut spotify_backend = crate::audio::spotify_backend::SpotifyBackend::new();
            // librespot is only driven while the loaded track is a Spotify stream.
            let mut current_is_spotify = false;

            loop {
                // Poll commands; recv_timeout keeps real-time constraints
                match cmd_rx.recv_timeout(std::time::Duration::from_millis(50)) {
                    Ok(PlayerCmd::Load {
                        input,
                        start_pos_ms,
                        duration_ms,
                        generation,
                    }) => {
                        current_generation = generation;
                        was_finished = false;
                        match input {
                            input @ (PlayInput::File(_) | PlayInput::Growing { .. }) => {
                                current_is_spotify = false;
                                spotify_backend.stop();
                                let played = match input {
                                    PlayInput::Growing { path, progress } => backend.play_growing(
                                        &path,
                                        progress,
                                        duration_ms.map(std::time::Duration::from_millis),
                                    ),
                                    PlayInput::File(path) => backend.play(&path.to_string_lossy()),
                                    PlayInput::Spotify(_) => unreachable!(),
                                };
                                match played {
                                    Ok(()) => {
                                        if let Some(pos_ms) = start_pos_ms {
                                            backend
                                                .seek_to(std::time::Duration::from_millis(pos_ms));
                                        }
                                        is_paused_clone.store(false, Ordering::Release);
                                        is_playing_clone.store(true, Ordering::Release);
                                        is_finished_clone.store(false, Ordering::Release);
                                        let _ = event_tx.send(PlayerEvent::Loaded { generation });
                                    }
                                    Err(e) => {
                                        is_playing_clone.store(false, Ordering::Release);
                                        is_finished_clone.store(true, Ordering::Release);
                                        was_finished = true;
                                        let _ = event_tx.send(PlayerEvent::Failed {
                                            generation,
                                            error: e,
                                        });
                                    }
                                }
                            }
                            PlayInput::Spotify(uri) => {
                                current_is_spotify = true;
                                // Silence the previous track while the stream connects.
                                backend.stop();
                                let _ = event_tx.send(PlayerEvent::Buffering { generation });
                                let pos = start_pos_ms.unwrap_or(0) as u32;
                                match spotify_backend.load(&uri, pos) {
                                    Ok(()) => {
                                        let source = spotify_backend.get_or_create_source();
                                        let duration_dur =
                                            duration_ms.map(std::time::Duration::from_millis);
                                        if let Some(dur) = duration_ms {
                                            total_duration_ms_clone.store(dur, Ordering::Release);
                                        }
                                        match backend.play_pcm_source(source, duration_dur) {
                                            Ok(()) => {
                                                is_paused_clone.store(false, Ordering::Release);
                                                is_playing_clone.store(true, Ordering::Release);
                                                is_finished_clone.store(false, Ordering::Release);
                                                let _ = event_tx
                                                    .send(PlayerEvent::Loaded { generation });
                                            }
                                            Err(e) => {
                                                is_playing_clone.store(false, Ordering::Release);
                                                is_finished_clone.store(true, Ordering::Release);
                                                was_finished = true;
                                                let _ = event_tx.send(PlayerEvent::Failed {
                                                    generation,
                                                    error: e,
                                                });
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        if spotify_backend.needs_sign_in() {
                                            let _ = event_tx.send(PlayerEvent::SpotifySignInLost);
                                        }
                                        is_playing_clone.store(false, Ordering::Release);
                                        is_finished_clone.store(true, Ordering::Release);
                                        was_finished = true;
                                        let _ = event_tx.send(PlayerEvent::Failed {
                                            generation,
                                            error: e,
                                        });
                                    }
                                }
                            }
                        }
                    }
                    Ok(PlayerCmd::Play) => {
                        backend.resume();
                        if current_is_spotify {
                            spotify_backend.resume();
                        }
                        is_paused_clone.store(false, Ordering::Release);
                        is_playing_clone.store(true, Ordering::Release);
                    }
                    Ok(PlayerCmd::Pause) => {
                        backend.pause();
                        if current_is_spotify {
                            spotify_backend.pause();
                        }
                        is_paused_clone.store(true, Ordering::Release);
                        is_playing_clone.store(false, Ordering::Release);
                    }
                    Ok(PlayerCmd::Seek(pos_ms)) => {
                        backend.seek_to(std::time::Duration::from_millis(pos_ms));
                        if current_is_spotify {
                            spotify_backend.seek(pos_ms as u32);
                        }
                    }
                    Ok(PlayerCmd::Stop) => {
                        backend.stop();
                        spotify_backend.stop();
                        was_finished = true;
                        is_paused_clone.store(false, Ordering::Release);
                        is_playing_clone.store(false, Ordering::Release);
                        is_finished_clone.store(true, Ordering::Release);
                    }
                    Ok(PlayerCmd::SetVolume(vol)) => {
                        backend.set_volume(vol);
                    }
                    Ok(PlayerCmd::PreloadSpotify(uri)) => {
                        if current_is_spotify {
                            spotify_backend.preload(&uri);
                        }
                    }
                    Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                        break;
                    }
                    Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                }

                // librespot reported the loaded track as unplayable
                if let Some(error) = spotify_backend.take_failure() {
                    if current_is_spotify && !was_finished {
                        backend.stop();
                        spotify_backend.stop();
                        was_finished = true;
                        is_playing_clone.store(false, Ordering::Release);
                        is_finished_clone.store(true, Ordering::Release);
                        let _ = event_tx.send(PlayerEvent::Failed {
                            generation: current_generation,
                            error,
                        });
                    }
                }

                // Align the clock with the position librespot is actually playing
                if let Some(pos_ms) = spotify_backend.take_position_ms() {
                    if current_is_spotify {
                        backend.sync_position(pos_ms);
                    }
                }

                // Update elapsed tracking
                let elapsed = backend.get_position();
                elapsed_ms_clone.store(elapsed.as_millis() as u64, Ordering::Relaxed);

                // Update duration tracking
                if let Some(dur) = backend.get_duration() {
                    total_duration_ms_clone.store(dur.as_millis() as u64, Ordering::Relaxed);
                }

                // Update finished state and send Finished event on transition
                let finished = backend.is_finished();
                is_finished_clone.store(finished, Ordering::Release);
                if finished && !was_finished {
                    was_finished = true;
                    is_playing_clone.store(false, Ordering::Release);
                    let total = total_duration_ms_clone.load(Ordering::Relaxed);
                    if total > 0 {
                        elapsed_ms_clone.store(total, Ordering::Relaxed);
                    }
                    let _ = event_tx.send(PlayerEvent::Finished {
                        generation: current_generation,
                    });
                }

                // Update volume atomic to match backend volume
                if let Some(vol) = backend.get_volume() {
                    volume_clone.store(vol, Ordering::Relaxed);
                }

                current_sample_rate_clone.store(backend.current_sample_rate(), Ordering::Relaxed);
            }
        });

        Some(Self {
            cmd_tx,
            event_rx,
            sample_buffer,
            generation: 0,
            is_paused,
            is_playing,
            is_finished,
            volume,
            elapsed_ms,
            total_duration_ms,
            current_sample_rate,
        })
    }

    pub fn next_generation(&mut self) -> u64 {
        self.generation = self.generation.wrapping_add(1);
        self.generation
    }

    pub fn load(&mut self, input: impl Into<PlayInput>) -> Result<(), String> {
        self.load_with_pos(input, None)
    }

    pub fn load_with_pos(
        &mut self,
        input: impl Into<PlayInput>,
        start_pos_ms: Option<u64>,
    ) -> Result<(), String> {
        self.load_with_pos_and_duration(input, start_pos_ms, None)
    }

    pub fn load_with_pos_and_duration(
        &mut self,
        input: impl Into<PlayInput>,
        start_pos_ms: Option<u64>,
        duration_ms: Option<u64>,
    ) -> Result<(), String> {
        self.generation = self.generation.wrapping_add(1);
        let gen = self.generation;
        self.is_paused.store(false, Ordering::Release);
        self.is_playing.store(true, Ordering::Release);
        self.is_finished.store(false, Ordering::Release);
        let pos = start_pos_ms.unwrap_or(0);
        self.elapsed_ms.store(pos, Ordering::Release);
        if let Some(dur) = duration_ms {
            self.total_duration_ms.store(dur, Ordering::Release);
        }
        let _ = self.cmd_tx.send(PlayerCmd::Load {
            input: input.into(),
            start_pos_ms,
            duration_ms,
            generation: gen,
        });
        Ok(())
    }

    pub fn load_track(&mut self, path: &Path) -> Result<(), String> {
        self.load_with_pos(path.to_path_buf(), None)
    }

    pub fn load_track_with_pos(
        &mut self,
        path: &Path,
        start_pos_ms: Option<u64>,
    ) -> Result<(), String> {
        self.load_with_pos(path.to_path_buf(), start_pos_ms)
    }

    pub fn play(&mut self) {
        self.is_paused.store(false, Ordering::Release);
        self.is_playing.store(true, Ordering::Release);
        self.is_finished.store(false, Ordering::Release);
        let _ = self.cmd_tx.send(PlayerCmd::Play);
    }

    pub fn pause(&mut self) {
        self.is_paused.store(true, Ordering::Release);
        self.is_playing.store(false, Ordering::Release);
        let _ = self.cmd_tx.send(PlayerCmd::Pause);
    }

    pub fn toggle_pause(&mut self) {
        if self.is_paused() {
            self.play();
        } else {
            self.pause();
        }
    }

    pub fn seek(&mut self, pos_ms: u64) {
        self.elapsed_ms.store(pos_ms, Ordering::Relaxed);
        let _ = self.cmd_tx.send(PlayerCmd::Seek(pos_ms));
    }

    pub fn stop(&mut self) {
        self.is_paused.store(false, Ordering::Release);
        self.is_playing.store(false, Ordering::Release);
        self.is_finished.store(true, Ordering::Release);
        let _ = self.cmd_tx.send(PlayerCmd::Stop);
    }

    pub fn is_playing(&self) -> bool {
        self.is_playing.load(Ordering::Acquire)
    }

    pub fn is_paused(&self) -> bool {
        self.is_paused.load(Ordering::Acquire)
    }

    pub fn is_finished(&self) -> bool {
        self.is_finished.load(Ordering::Acquire)
    }

    pub fn elapsed_ms(&self) -> u64 {
        self.elapsed_ms.load(Ordering::Relaxed)
    }

    pub fn elapsed_secs(&self) -> f64 {
        self.elapsed_ms() as f64 / 1000.0
    }

    pub fn duration_ms(&self) -> u64 {
        self.total_duration_ms.load(Ordering::Relaxed)
    }

    pub fn set_duration_ms(&self, ms: u64) {
        self.total_duration_ms.store(ms, Ordering::Relaxed);
    }

    pub fn volume(&self) -> u8 {
        self.volume.load(Ordering::Relaxed)
    }

    pub fn volume_up(&mut self) {
        let current = self.volume.load(Ordering::Relaxed);
        let next = (current + 5).min(100);
        self.volume.store(next, Ordering::Relaxed);
        let _ = self.cmd_tx.send(PlayerCmd::SetVolume(next));
    }

    pub fn volume_down(&mut self) {
        let current = self.volume.load(Ordering::Relaxed);
        let next = current.saturating_sub(5);
        self.volume.store(next, Ordering::Relaxed);
        let _ = self.cmd_tx.send(PlayerCmd::SetVolume(next));
    }

    pub fn set_volume(&mut self, vol: u8) {
        let vol = vol.min(100);
        self.volume.store(vol, Ordering::Relaxed);
        let _ = self.cmd_tx.send(PlayerCmd::SetVolume(vol));
    }

    pub fn current_sample_rate(&self) -> u32 {
        self.current_sample_rate.load(Ordering::Relaxed)
    }

    pub fn preload_spotify(&self, uri: String) {
        let _ = self.cmd_tx.send(PlayerCmd::PreloadSpotify(uri));
    }
}
