use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;
use std::time::Instant;

use rodio::{Decoder, OutputStream, Sink, Source};

use crate::audio::viz_source::{new_shared_buffer, SharedSampleBuffer, VisualizerSource};
use crossbeam_channel::{bounded, Sender};

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

/// Thread-isolated audio player wrapping rodio Sink.
pub struct Player {
    cmd_tx: Sender<PlayerCmd>,
    pub sample_buffer: SharedSampleBuffer,

    // Shared atomic states.
    // State flags use Release/Acquire ordering to establish proper happens-before
    // relationships across threads (important on non-TSO architectures like ARM).
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
        let sample_buffer = new_shared_buffer(4096);

        let is_paused_clone = is_paused.clone();
        let is_playing_clone = is_playing.clone();
        let is_finished_clone = is_finished.clone();
        let volume_clone = volume.clone();
        let elapsed_ms_clone = elapsed_ms.clone();
        let total_duration_ms_clone = total_duration_ms.clone();
        let current_sample_rate_clone = current_sample_rate.clone();
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
                None => return,
            };

            // Prevent stream from dropping immediately by keeping it in scope
            let _stream = stream;

            let sink_res = run_with_high_priority(|| Sink::try_new(&handle));
            let mut sink = match sink_res {
                Ok(s) => s,
                Err(_) => return,
            };
            sink.set_volume(0.8);

            let mut start_instant: Option<Instant> = None;
            let mut accumulated_ms = 0u64;
            let mut current_path: Option<PathBuf> = None;
            let mut current_channels = 2u16;
            let mut current_sample_rate = 44100u32;
            let skip_request = Arc::new(std::sync::atomic::AtomicU64::new(0));

            loop {
                // Poll commands; recv_timeout keeps real-time constraints
                match cmd_rx.recv_timeout(std::time::Duration::from_millis(10)) {
                    Ok(PlayerCmd::Load { path, start_pos_ms }) => {
                        sink.stop();
                        current_path = Some(path.clone());
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
                                            Ok(mut source) => {
                                                let sr = source.sample_rate();
                                                let ch = source.channels();
                                                current_sample_rate = sr;
                                                current_channels = ch;
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

                                                skip_request.store(0, Ordering::Release);

                                                let mut actual_start_ms = 0;
                                                if let Some(pos_ms) = start_pos_ms {
                                                    if pos_ms > 0 {
                                                        // Try native seek first on the decoder directly.
                                                        if source
                                                            .try_seek(
                                                                std::time::Duration::from_millis(
                                                                    pos_ms,
                                                                ),
                                                            )
                                                            .is_ok()
                                                        {
                                                            actual_start_ms = pos_ms;
                                                        } else {
                                                            // Fallback to sample-dropping skip
                                                            let samples_to_skip = (pos_ms as f64
                                                                / 1000.0
                                                                * sr as f64
                                                                * ch as f64)
                                                                as u64;
                                                            skip_request.store(
                                                                samples_to_skip,
                                                                Ordering::Release,
                                                            );
                                                            actual_start_ms = pos_ms;
                                                        }
                                                    }
                                                }

                                                let viz_source = VisualizerSource::new(
                                                    source.convert_samples::<f32>(),
                                                    sample_buffer_clone.clone(),
                                                    skip_request.clone(),
                                                );
                                                sink.append(viz_source);
                                                sink.play();

                                                accumulated_ms = actual_start_ms;
                                                start_instant = Some(Instant::now());
                                                // Release ordering: these stores must be
                                                // visible before cmd_rx receives the next msg
                                                is_paused_clone.store(false, Ordering::Release);
                                                is_playing_clone.store(true, Ordering::Release);
                                                is_finished_clone.store(false, Ordering::Release);
                                            }
                                            Err(e) => {
                                                eprintln!("Failed to decode: {}", e);
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        eprintln!("Failed to open file: {}", e);
                                    }
                                }
                            }
                            Err(e) => {
                                eprintln!("Failed to create sink: {}", e);
                            }
                        }
                    }
                    Ok(PlayerCmd::Play) => {
                        if start_instant.is_none() {
                            sink.play();
                            start_instant = Some(Instant::now());
                            is_paused_clone.store(false, Ordering::Release);
                            is_playing_clone.store(true, Ordering::Release);
                        }
                    }
                    Ok(PlayerCmd::Pause) => {
                        if let Some(start) = start_instant.take() {
                            accumulated_ms += start.elapsed().as_millis() as u64;
                            sink.pause();
                            is_paused_clone.store(true, Ordering::Release);
                            is_playing_clone.store(false, Ordering::Release);
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
                            Err(_) => {
                                // Native seek failed. Implement hybrid seek.
                                let running = if let Some(start) = &start_instant {
                                    if !is_paused_clone.load(Ordering::Acquire) {
                                        start.elapsed().as_millis() as u64
                                    } else {
                                        0
                                    }
                                } else {
                                    0
                                };
                                let current_ms = accumulated_ms + running;
                                if pos_ms >= current_ms {
                                    // Forward seek: calculate samples to skip
                                    let diff_ms = pos_ms - current_ms;
                                    let channels = current_channels;
                                    let sample_rate = current_sample_rate;
                                    let samples_to_skip = (diff_ms as f64 / 1000.0
                                        * sample_rate as f64
                                        * channels as f64)
                                        as u64;

                                    skip_request.fetch_add(samples_to_skip, Ordering::Release);

                                    accumulated_ms = pos_ms;
                                    if start_instant.is_some() {
                                        start_instant = Some(Instant::now());
                                    }
                                } else {
                                    // Backward seek: reopen and fast-forward
                                    if let Some(ref path) = current_path {
                                        sink.stop();
                                        if let Ok(new_sink) =
                                            run_with_high_priority(|| Sink::try_new(&handle))
                                        {
                                            sink = new_sink;
                                            let vol = volume_clone.load(Ordering::Relaxed);
                                            sink.set_volume(vol as f32 / 100.0);

                                            if let Ok(file) = std::fs::File::open(path) {
                                                let reader = BufReader::new(file);
                                                if let Ok(source) = Decoder::new(reader) {
                                                    let sr = source.sample_rate();
                                                    let ch = source.channels();
                                                    current_sample_rate = sr;
                                                    current_channels = ch;
                                                    current_sample_rate_clone
                                                        .store(sr, Ordering::Relaxed);

                                                    skip_request.store(0, Ordering::Release);
                                                    let samples_to_skip = (pos_ms as f64 / 1000.0
                                                        * sr as f64
                                                        * ch as f64)
                                                        as u64;

                                                    let viz_source = VisualizerSource::new(
                                                        source.convert_samples::<f32>(),
                                                        sample_buffer_clone.clone(),
                                                        skip_request.clone(),
                                                    );

                                                    skip_request
                                                        .store(samples_to_skip, Ordering::Release);

                                                    sink.append(viz_source);
                                                    if !is_paused_clone.load(Ordering::Acquire) {
                                                        sink.play();
                                                        start_instant = Some(Instant::now());
                                                    } else {
                                                        sink.pause();
                                                        start_instant = None;
                                                    }
                                                    accumulated_ms = pos_ms;
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    Ok(PlayerCmd::Stop) => {
                        sink.stop();
                        start_instant = None;
                        accumulated_ms = 0;
                        is_paused_clone.store(false, Ordering::Release);
                        is_playing_clone.store(false, Ordering::Release);
                        is_finished_clone.store(true, Ordering::Release);
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
                    if !is_paused_clone.load(Ordering::Acquire) {
                        start.elapsed().as_millis() as u64
                    } else {
                        0
                    }
                } else {
                    0
                };
                elapsed_ms_clone.store(accumulated_ms + running, Ordering::Relaxed);

                // Update finished state
                let finished = sink.empty();
                is_finished_clone.store(finished, Ordering::Release);
                if finished {
                    is_playing_clone.store(false, Ordering::Release);
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
