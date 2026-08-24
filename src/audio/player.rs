use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;

use crossbeam_channel::{bounded, Sender};

use crate::audio::rodio_backend::RodioBackend;
use crate::audio::viz_source::SharedSampleBuffer;

/// Commands sent to the background player thread.
pub enum PlayerCmd {
    Load {
        path: PathBuf,
        start_pos_ms: Option<u64>,
    },
    Play,
    Pause,
    Seek(u64),
    Stop,
    SetVolume(u8),
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
    pub sample_buffer: SharedSampleBuffer,

    // Shared atomic states.
    pub is_paused: Arc<AtomicBool>,
    pub is_playing: Arc<AtomicBool>,
    is_finished: Arc<AtomicBool>,
    volume: Arc<AtomicU8>,
    elapsed_ms: Arc<AtomicU64>,
    total_duration_ms: Arc<AtomicU64>,
    current_sample_rate: Arc<AtomicU32>,
}

impl Player {
    pub fn new() -> Option<Self> {
        let (cmd_tx, cmd_rx) = bounded::<PlayerCmd>(100);

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

            loop {
                // Poll commands; recv_timeout keeps real-time constraints
                match cmd_rx.recv_timeout(std::time::Duration::from_millis(50)) {
                    Ok(PlayerCmd::Load { path, start_pos_ms }) => {
                        let path_str = path.to_string_lossy().to_string();
                        match backend.play(&path_str) {
                            Ok(()) => {
                                if let Some(pos_ms) = start_pos_ms {
                                    backend.seek_to(std::time::Duration::from_millis(pos_ms));
                                }
                                is_paused_clone.store(false, Ordering::Release);
                                is_playing_clone.store(true, Ordering::Release);
                                is_finished_clone.store(false, Ordering::Release);
                            }
                            Err(e) => {
                                eprintln!("Failed to play: {}", e);
                            }
                        }
                    }
                    Ok(PlayerCmd::Play) => {
                        backend.resume();
                        is_paused_clone.store(false, Ordering::Release);
                        is_playing_clone.store(true, Ordering::Release);
                    }
                    Ok(PlayerCmd::Pause) => {
                        backend.pause();
                        is_paused_clone.store(true, Ordering::Release);
                        is_playing_clone.store(false, Ordering::Release);
                    }
                    Ok(PlayerCmd::Seek(pos_ms)) => {
                        backend.seek_to(std::time::Duration::from_millis(pos_ms));
                    }
                    Ok(PlayerCmd::Stop) => {
                        backend.stop();
                        is_paused_clone.store(false, Ordering::Release);
                        is_playing_clone.store(false, Ordering::Release);
                        is_finished_clone.store(true, Ordering::Release);
                    }
                    Ok(PlayerCmd::SetVolume(vol)) => {
                        backend.set_volume(vol);
                    }
                    Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                        break;
                    }
                    Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                }

                // Update elapsed tracking
                let elapsed = backend.get_position();
                elapsed_ms_clone.store(elapsed.as_millis() as u64, Ordering::Relaxed);

                // Update duration tracking
                if let Some(dur) = backend.get_duration() {
                    total_duration_ms_clone.store(dur.as_millis() as u64, Ordering::Relaxed);
                }

                // Update finished state
                let finished = backend.is_finished();
                is_finished_clone.store(finished, Ordering::Release);
                if finished {
                    is_playing_clone.store(false, Ordering::Release);
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
            sample_buffer,
            is_paused,
            is_playing,
            is_finished,
            volume,
            elapsed_ms,
            total_duration_ms,
            current_sample_rate,
        })
    }

    pub fn load_track(&mut self, path: &Path) -> Result<(), String> {
        self.load_track_with_pos(path, None)
    }

    pub fn load_track_with_pos(
        &mut self,
        path: &Path,
        start_pos_ms: Option<u64>,
    ) -> Result<(), String> {
        self.is_paused.store(false, Ordering::Release);
        self.is_playing.store(true, Ordering::Release);
        self.is_finished.store(false, Ordering::Release);
        let pos = start_pos_ms.unwrap_or(0);
        self.elapsed_ms.store(pos, Ordering::Release);
        let _ = self.cmd_tx.send(PlayerCmd::Load {
            path: path.to_path_buf(),
            start_pos_ms,
        });
        Ok(())
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
}
