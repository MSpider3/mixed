use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use rodio::{Decoder, OutputStream, Sink, Source};

use crate::audio::viz_source::{new_shared_buffer, SharedSampleBuffer, VisualizerSource};
use crossbeam_channel::{bounded, Sender};

/// Commands sent to the background player thread.
pub enum PlayerCmd {
    Load(PathBuf),
    Play,
    Pause,
    Seek(u64),
    Stop,
    SetVolume(u8),
}

/// Thread-isolated audio player wrapping rodio Sink.
pub struct Player {
    cmd_tx: Sender<PlayerCmd>,
    pub sample_buffer: SharedSampleBuffer,

    // Shared atomic states (synchronized via Relaxed ordering)
    pub is_paused: Arc<AtomicBool>,
    pub is_playing: Arc<AtomicBool>,
    is_finished: Arc<AtomicBool>,
    volume: Arc<AtomicU8>,
    elapsed_ms: Arc<AtomicU64>,
    total_duration_ms: Arc<AtomicU64>,
    current_sample_rate: Arc<AtomicU32>,
    #[allow(dead_code)]
    pub status_msg: Arc<Mutex<Option<String>>>,
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
        let status_msg = Arc::new(Mutex::new(None));
        let sample_buffer = new_shared_buffer(4096);

        let is_paused_clone = is_paused.clone();
        let is_playing_clone = is_playing.clone();
        let is_finished_clone = is_finished.clone();
        let volume_clone = volume.clone();
        let elapsed_ms_clone = elapsed_ms.clone();
        let total_duration_ms_clone = total_duration_ms.clone();
        let current_sample_rate_clone = current_sample_rate.clone();
        let status_msg_clone = status_msg.clone();
        let sample_buffer_clone = sample_buffer.clone();

        std::thread::spawn(move || {
            let stream_res = run_with_high_priority(|| {
                use rodio::cpal::traits::HostTrait;
                let host = rodio::cpal::default_host();
                let device = host.default_output_device()?;
                OutputStream::try_from_device(&device).ok()
            });

            let (stream, handle) = match stream_res {
                Some(s) => s,
                None => {
                    *status_msg_clone.lock().unwrap() =
                        Some("Failed to open audio device".to_string());
                    return;
                }
            };

            // Prevent stream from dropping immediately by keeping it in scope
            let _stream = stream;

            let sink_res = run_with_high_priority(|| Sink::try_new(&handle));
            let mut sink = match sink_res {
                Ok(s) => s,
                Err(e) => {
                    *status_msg_clone.lock().unwrap() =
                        Some(format!("Failed to create sink: {}", e));
                    return;
                }
            };
            sink.set_volume(0.8);

            let mut start_instant: Option<Instant> = None;
            let mut accumulated_ms = 0u64;

            loop {
                // Poll commands using crossbeam try_recv or recv_timeout to keep real-time constraints
                match cmd_rx.recv_timeout(std::time::Duration::from_millis(10)) {
                    Ok(PlayerCmd::Load(path)) => {
                        sink.stop();
                        let new_sink_res = run_with_high_priority(|| Sink::try_new(&handle));
                        match new_sink_res {
                            Ok(s) => {
                                sink = s;
                                let vol = volume_clone.load(Ordering::Relaxed);
                                sink.set_volume(vol as f32 / 100.0);

                                match std::fs::File::open(&path) {
                                    Ok(file) => {
                                        let reader = BufReader::new(file);
                                        match Decoder::new(reader) {
                                            Ok(source) => {
                                                let sr = source.sample_rate();
                                                current_sample_rate_clone
                                                    .store(sr, Ordering::Relaxed);
                                                let duration = source.total_duration();
                                                if let Some(dur) = duration {
                                                    total_duration_ms_clone.store(
                                                        dur.as_millis() as u64,
                                                        Ordering::Relaxed,
                                                    );
                                                } else {
                                                    total_duration_ms_clone
                                                        .store(0, Ordering::Relaxed);
                                                }

                                                if let Ok(mut buf) = sample_buffer_clone.lock() {
                                                    buf.sample_rate = sr;
                                                }

                                                let viz_source = VisualizerSource::new(
                                                    source.convert_samples::<f32>(),
                                                    sample_buffer_clone.clone(),
                                                );
                                                sink.append(viz_source);
                                                sink.play();

                                                accumulated_ms = 0;
                                                start_instant = Some(Instant::now());
                                                is_paused_clone.store(false, Ordering::Relaxed);
                                                is_playing_clone.store(true, Ordering::Relaxed);
                                                is_finished_clone.store(false, Ordering::Relaxed);
                                                *status_msg_clone.lock().unwrap() = None;
                                            }
                                            Err(e) => {
                                                *status_msg_clone.lock().unwrap() =
                                                    Some(format!("Failed to decode: {}", e));
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        *status_msg_clone.lock().unwrap() =
                                            Some(format!("Failed to open file: {}", e));
                                    }
                                }
                            }
                            Err(e) => {
                                *status_msg_clone.lock().unwrap() =
                                    Some(format!("Failed to create sink: {}", e));
                            }
                        }
                    }
                    Ok(PlayerCmd::Play) => {
                        if start_instant.is_none() {
                            sink.play();
                            start_instant = Some(Instant::now());
                            is_paused_clone.store(false, Ordering::Relaxed);
                            is_playing_clone.store(true, Ordering::Relaxed);
                        }
                    }
                    Ok(PlayerCmd::Pause) => {
                        if let Some(start) = start_instant.take() {
                            accumulated_ms += start.elapsed().as_millis() as u64;
                            sink.pause();
                            is_paused_clone.store(true, Ordering::Relaxed);
                            is_playing_clone.store(false, Ordering::Relaxed);
                        }
                    }
                    Ok(PlayerCmd::Seek(pos_ms)) => {
                        let duration = std::time::Duration::from_millis(pos_ms);
                        match sink.try_seek(duration) {
                            Ok(()) => {
                                accumulated_ms = pos_ms;
                                if start_instant.is_some() {
                                    start_instant = Some(Instant::now());
                                }
                            }
                            Err(e) => {
                                *status_msg_clone.lock().unwrap() =
                                    Some(format!("Seek failed: {:?}", e));
                            }
                        }
                    }
                    Ok(PlayerCmd::Stop) => {
                        sink.stop();
                        start_instant = None;
                        accumulated_ms = 0;
                        is_paused_clone.store(false, Ordering::Relaxed);
                        is_playing_clone.store(false, Ordering::Relaxed);
                        is_finished_clone.store(true, Ordering::Relaxed);
                    }
                    Ok(PlayerCmd::SetVolume(vol)) => {
                        sink.set_volume(vol as f32 / 100.0);
                    }
                    Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                        break;
                    }
                    Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                }

                // Update elapsed tracking
                let running = if let Some(start) = &start_instant {
                    if !is_paused_clone.load(Ordering::Relaxed) {
                        start.elapsed().as_millis() as u64
                    } else {
                        0
                    }
                } else {
                    0
                };
                elapsed_ms_clone.store(accumulated_ms + running, Ordering::Relaxed);

                // Update play state
                let finished = sink.empty();
                is_finished_clone.store(finished, Ordering::Relaxed);
                if finished {
                    is_playing_clone.store(false, Ordering::Relaxed);
                }
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
            status_msg,
        })
    }

    pub fn load_track(&mut self, path: &Path) -> Result<(), String> {
        self.is_paused.store(false, Ordering::Relaxed);
        self.is_playing.store(true, Ordering::Relaxed);
        self.is_finished.store(false, Ordering::Relaxed);
        let _ = self.cmd_tx.send(PlayerCmd::Load(path.to_path_buf()));
        Ok(())
    }

    pub fn play(&mut self) {
        self.is_paused.store(false, Ordering::Relaxed);
        self.is_playing.store(true, Ordering::Relaxed);
        self.is_finished.store(false, Ordering::Relaxed);
        let _ = self.cmd_tx.send(PlayerCmd::Play);
    }

    pub fn pause(&mut self) {
        self.is_paused.store(true, Ordering::Relaxed);
        self.is_playing.store(false, Ordering::Relaxed);
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
        self.is_paused.store(false, Ordering::Relaxed);
        self.is_playing.store(false, Ordering::Relaxed);
        self.is_finished.store(true, Ordering::Relaxed);
        let _ = self.cmd_tx.send(PlayerCmd::Stop);
    }

    pub fn is_playing(&self) -> bool {
        self.is_playing.load(Ordering::Relaxed)
    }

    pub fn is_paused(&self) -> bool {
        self.is_paused.load(Ordering::Relaxed)
    }

    pub fn is_finished(&self) -> bool {
        self.is_finished.load(Ordering::Relaxed)
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

#[cfg(target_os = "linux")]
fn run_with_high_priority<F, R>(f: F) -> R
where
    F: FnOnce() -> R,
{
    unsafe {
        let tid = libc::syscall(libc::SYS_gettid) as libc::pid_t;
        let old_priority = libc::getpriority(libc::PRIO_PROCESS, tid as libc::id_t);
        libc::setpriority(libc::PRIO_PROCESS, tid as libc::id_t, -10);
        let result = f();
        libc::setpriority(libc::PRIO_PROCESS, tid as libc::id_t, old_priority);
        result
    }
}

#[cfg(not(target_os = "linux"))]
fn run_with_high_priority<F, R>(f: F) -> R
where
    F: FnOnce() -> R,
{
    f()
}
