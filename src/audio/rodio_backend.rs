use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rodio::{OutputStream, OutputStreamHandle, Sink, Source};

use crate::audio::growing_file::DownloadProgress;
use crate::audio::symphonia_source::SymphoniaSource;
use crate::audio::viz_source::{SharedSampleBuffer, VisualizerSource};

/// `current_path` marker for a live PCM stream (no file behind it).
const PCM_STREAM_PATH: &str = "spotify:stream";

/// Minimum distance ahead of the playhead for an in-place seek on a partial file.
const FORWARD_SEEK_MARGIN_MS: u64 = 1000;

/// A track that is being played from a partially downloaded file.
struct Growing {
    part: PathBuf,
    progress: Arc<DownloadProgress>,
    /// Releases the active reader if it is waiting for more data.
    cancel: Arc<AtomicBool>,
}

pub struct RodioBackend {
    sink: Sink,
    _stream: OutputStream,
    handle: OutputStreamHandle,
    current_channels: u16,
    pub current_sample_rate: u32,
    total_duration: Option<Duration>,
    pub sample_buffer: SharedSampleBuffer,
    skip_request: Arc<AtomicU64>,
    accumulated_ms: u64,
    start_instant: Option<Instant>,
    is_paused: bool,
    current_path: Option<String>,
    volume: u8,
    /// True while a PCM stream is loaded but has not reported its first position.
    clock_held: bool,
    growing: Option<Growing>,
}

impl RodioBackend {
    pub fn new(sample_buffer: SharedSampleBuffer) -> Option<Self> {
        let stream_res = run_with_high_priority(|| {
            if let Ok(res) = OutputStream::try_default() {
                return Some(res);
            }
            use rodio::cpal::traits::HostTrait;
            let host = rodio::cpal::default_host();
            let device = host.default_output_device()?;
            OutputStream::try_from_device(&device).ok()
        });

        let (stream, handle) = stream_res?;

        let sink_res = run_with_high_priority(|| Sink::try_new(&handle));
        let sink = match sink_res {
            Ok(s) => s,
            Err(_) => return None,
        };

        sink.set_volume(0.8);

        let skip_request = Arc::new(AtomicU64::new(0));

        Some(Self {
            sink,
            _stream: stream,
            handle,
            current_channels: 2,
            current_sample_rate: 44100,
            total_duration: None,
            sample_buffer,
            skip_request,
            accumulated_ms: 0,
            start_instant: None,
            is_paused: false,
            current_path: None,
            volume: 80,
            clock_held: false,
            growing: None,
        })
    }

    pub fn play(&mut self, path: &str) -> Result<(), String> {
        self.release_current();
        self.sink.stop();
        self.current_path = Some(path.to_string());

        let source = SymphoniaSource::open_file(path)?;
        self.start_source(source, None)
    }

    /// Play a file that is still being downloaded (`part` is the partial file).
    /// `fallback_duration` is used when the container does not state a length.
    pub fn play_growing(
        &mut self,
        part: &Path,
        progress: Arc<DownloadProgress>,
        fallback_duration: Option<Duration>,
    ) -> Result<(), String> {
        self.release_current();
        self.sink.stop();
        self.current_path = Some(part.to_string_lossy().into_owned());

        let cancel = Arc::new(AtomicBool::new(false));
        let source = SymphoniaSource::open_growing(part, progress.clone(), cancel.clone())?;
        self.growing = Some(Growing {
            part: part.to_path_buf(),
            progress,
            cancel,
        });
        self.start_source(source, fallback_duration)
    }

    /// Append a freshly opened decoder to a new sink and start the clock.
    fn start_source(
        &mut self,
        source: SymphoniaSource,
        fallback_duration: Option<Duration>,
    ) -> Result<(), String> {
        let sr = source.sample_rate();
        let ch = source.channels();
        self.current_sample_rate = sr;
        self.current_channels = ch;
        self.total_duration = source.total_duration().or(fallback_duration);

        if let Ok(mut buf) = self.sample_buffer.lock() {
            buf.sample_rate = sr;
        }

        self.skip_request.store(0, Ordering::Release);

        let new_sink_res = run_with_high_priority(|| Sink::try_new(&self.handle));
        match new_sink_res {
            Ok(s) => {
                self.sink = s;
                self.sink.set_volume(self.volume as f32 / 100.0);

                let viz_source = VisualizerSource::new(
                    source,
                    self.sample_buffer.clone(),
                    self.skip_request.clone(),
                );
                self.sink.append(viz_source);
                self.sink.play();

                self.accumulated_ms = 0;
                self.start_instant = Some(Instant::now());
                self.is_paused = false;

                Ok(())
            }
            Err(e) => Err(format!("Failed to create sink: {}", e)),
        }
    }

    pub fn play_pcm_source(
        &mut self,
        source: crate::audio::pcm_source::PcmSource,
        duration: Option<Duration>,
    ) -> Result<(), String> {
        self.release_current();
        self.sink.stop();
        self.current_path = Some(PCM_STREAM_PATH.to_string());
        self.current_sample_rate = 44100;
        self.current_channels = 2;
        self.total_duration = duration;

        if let Ok(mut buf) = self.sample_buffer.lock() {
            buf.sample_rate = 44100;
        }

        self.skip_request.store(0, Ordering::Release);

        let new_sink_res = run_with_high_priority(|| Sink::try_new(&self.handle));
        match new_sink_res {
            Ok(s) => {
                self.sink = s;
                self.sink.set_volume(self.volume as f32 / 100.0);

                let viz_source = VisualizerSource::new(
                    source,
                    self.sample_buffer.clone(),
                    self.skip_request.clone(),
                );
                self.sink.append(viz_source);
                self.sink.play();

                // The stream is silent until the remote side starts delivering audio:
                // hold the clock at zero until `sync_position` reports the real position.
                self.accumulated_ms = 0;
                self.start_instant = None;
                self.clock_held = true;
                self.is_paused = false;

                Ok(())
            }
            Err(e) => Err(format!("Failed to create sink: {}", e)),
        }
    }

    /// Set the playback clock to a position reported by the stream itself.
    pub fn sync_position(&mut self, pos_ms: u64) {
        self.accumulated_ms = pos_ms;
        self.clock_held = false;
        self.start_instant = if self.is_paused {
            None
        } else {
            Some(Instant::now())
        };
    }

    /// Forget per-track state and release a reader blocked on a partial download.
    fn release_current(&mut self) {
        if let Some(growing) = self.growing.take() {
            growing.cancel.store(true, Ordering::Release);
        }
        self.clock_held = false;
    }

    pub fn pause(&mut self) {
        if let Some(start) = self.start_instant.take() {
            self.accumulated_ms += start.elapsed().as_millis() as u64;
            self.sink.pause();
            self.is_paused = true;
        } else if self.clock_held && !self.is_paused {
            self.sink.pause();
            self.is_paused = true;
        }
    }

    pub fn resume(&mut self) {
        if self.start_instant.is_none() {
            self.sink.play();
            if !self.clock_held {
                self.start_instant = Some(Instant::now());
            }
            self.is_paused = false;
        }
    }

    /// Current position on the playback clock, in milliseconds.
    fn clock_ms(&self) -> u64 {
        let running = match &self.start_instant {
            Some(start) if !self.is_paused => start.elapsed().as_millis() as u64,
            _ => 0,
        };
        self.accumulated_ms + running
    }

    /// Furthest position that is safely inside the downloaded part of a growing file.
    fn downloaded_limit_ms(&self, growing: &Growing) -> u64 {
        let have = match std::fs::metadata(&growing.part) {
            Ok(meta) => meta.len(),
            // Renamed to its final name: the download is complete
            Err(_) => return u64::MAX,
        };
        match (growing.progress.total_bytes(), self.total_duration) {
            (Some(total), Some(duration)) => {
                let duration_ms = duration.as_millis() as u64;
                // Assume a roughly constant bitrate and keep a 5% safety margin
                (duration_ms as u128 * have.min(total) as u128 * 95 / (total as u128 * 100)) as u64
            }
            // Without a known size there is no way to tell how far the data reaches
            _ => self.clock_ms(),
        }
    }

    /// Open a fresh decoder for the current track from its start.
    fn open_current(&mut self) -> Result<SymphoniaSource, String> {
        if let Some(growing) = self.growing.take() {
            // Stop the previous reader in case it is waiting for data
            growing.cancel.store(true, Ordering::Release);

            let final_path = growing.part.with_extension("");
            if growing.progress.is_done() && final_path.is_file() {
                // Download finished: switch to the complete, fully seekable file
                let path = final_path.to_string_lossy().into_owned();
                let source = SymphoniaSource::open_file(&path)?;
                self.current_path = Some(path);
                return Ok(source);
            }

            let cancel = Arc::new(AtomicBool::new(false));
            let source = SymphoniaSource::open_growing(
                &growing.part,
                growing.progress.clone(),
                cancel.clone(),
            );
            self.growing = Some(Growing { cancel, ..growing });
            return source;
        }

        match self.current_path.as_deref() {
            Some(path) => SymphoniaSource::open_file(path),
            None => Err("No track loaded".to_string()),
        }
    }

    /// Restart the current track on a new sink, positioned at `pos_ms`.
    fn reopen_at(&mut self, pos_ms: u64) {
        let Ok(mut source) = self.open_current() else {
            return;
        };
        self.sink.stop();
        let Ok(new_sink) = run_with_high_priority(|| Sink::try_new(&self.handle)) else {
            return;
        };
        self.sink = new_sink;
        self.sink.set_volume(self.volume as f32 / 100.0);

        // Prefer the decoder's own seek; otherwise decode and discard up to the target.
        let seeked = source.try_seek(Duration::from_millis(pos_ms)).is_ok();
        let sr = source.sample_rate();
        let ch = source.channels();
        self.current_sample_rate = sr;
        self.current_channels = ch;
        let samples_to_skip = if seeked {
            0
        } else {
            (pos_ms as f64 / 1000.0 * sr as f64 * ch as f64) as u64
        };

        self.skip_request.store(0, Ordering::Release);
        let viz_source = VisualizerSource::new(
            source,
            self.sample_buffer.clone(),
            self.skip_request.clone(),
        );
        self.skip_request.store(samples_to_skip, Ordering::Release);

        self.sink.append(viz_source);
        if !self.is_paused {
            self.sink.play();
            self.start_instant = Some(Instant::now());
        } else {
            self.sink.pause();
            self.start_instant = None;
        }
        self.accumulated_ms = pos_ms;
    }

    pub fn seek_to(&mut self, target: Duration) {
        let target = if let Some(total) = self.total_duration {
            target.min(total)
        } else {
            target
        };
        let mut pos_ms = target.as_millis() as u64;

        if self.current_path.as_deref() == Some(PCM_STREAM_PATH) {
            self.accumulated_ms = pos_ms;
            if self.start_instant.is_some() {
                self.start_instant = Some(Instant::now());
            }
            return;
        }

        if let Some(growing) = &self.growing {
            if growing.progress.is_done() {
                // Switches to the finished file, which seeks anywhere
                self.reopen_at(pos_ms);
                return;
            }
            // Still downloading: stay inside the data that has arrived. The partial
            // file is read as a forward-only stream, so going back means reopening.
            let now_ms = self.clock_ms();
            let limit_ms = self.downloaded_limit_ms(growing);
            if pos_ms > limit_ms {
                if limit_ms <= now_ms {
                    // Nothing further ahead has arrived yet
                    return;
                }
                pos_ms = limit_ms;
            }
            // A forward-only reader cannot step back, and a target this close to the
            // playhead may already lie behind the decoder's read position.
            if pos_ms < now_ms + FORWARD_SEEK_MARGIN_MS {
                self.reopen_at(pos_ms);
                return;
            }
        }

        let duration = Duration::from_millis(pos_ms);
        match self.sink.try_seek(duration) {
            Ok(()) => {
                self.accumulated_ms = pos_ms;
                if self.start_instant.is_some() {
                    self.start_instant = Some(Instant::now());
                }
            }
            Err(_) => {
                // Native seek failed. Implement hybrid seek.
                let current_ms = self.clock_ms();
                if pos_ms >= current_ms {
                    // Forward seek: calculate samples to skip
                    let diff_ms = pos_ms - current_ms;
                    let channels = self.current_channels;
                    let sample_rate = self.current_sample_rate;
                    let samples_to_skip =
                        (diff_ms as f64 / 1000.0 * sample_rate as f64 * channels as f64) as u64;

                    self.skip_request
                        .fetch_add(samples_to_skip, Ordering::Release);

                    self.accumulated_ms = pos_ms;
                    if self.start_instant.is_some() {
                        self.start_instant = Some(Instant::now());
                    }
                } else {
                    // Backward seek: reopen and fast-forward
                    self.reopen_at(pos_ms);
                }
            }
        }
    }

    pub fn stop(&mut self) {
        self.release_current();
        self.sink.stop();
        self.start_instant = None;
        self.accumulated_ms = 0;
        self.is_paused = false;
    }

    pub fn set_volume(&mut self, volume: u8) {
        self.volume = volume;
        self.sink.set_volume(volume as f32 / 100.0);
    }

    pub fn get_volume(&mut self) -> Option<u8> {
        Some(self.volume)
    }

    pub fn get_position(&mut self) -> Duration {
        if self.is_finished() {
            if let Some(total) = self.total_duration {
                return total;
            }
        }
        let current = Duration::from_millis(self.clock_ms());
        if let Some(total) = self.total_duration {
            current.min(total)
        } else {
            current
        }
    }

    pub fn get_duration(&mut self) -> Option<Duration> {
        self.total_duration
    }

    pub fn is_finished(&mut self) -> bool {
        self.sink.empty()
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
        let _ = libc::setpriority(libc::PRIO_PROCESS, tid as libc::id_t, -10);
        let result = f();
        let _ = libc::setpriority(libc::PRIO_PROCESS, tid as libc::id_t, old_priority);
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
