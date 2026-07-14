#![cfg(not(target_os = "android"))]

use std::io::BufReader;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink, Source};

use crate::audio::viz_source::{new_shared_buffer, SharedSampleBuffer, VisualizerSource};

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
}

impl RodioBackend {
    pub fn new() -> Option<Self> {
        let stream_res = run_with_high_priority(|| {
            use rodio::cpal::traits::HostTrait;
            let host = rodio::cpal::default_host();
            let device = host.default_output_device()?;
            OutputStream::try_from_device(&device).ok()
        });

        let (stream, handle) = match stream_res {
            Some(s) => s,
            None => return None,
        };

        let sink_res = run_with_high_priority(|| Sink::try_new(&handle));
        let sink = match sink_res {
            Ok(s) => s,
            Err(_) => return None,
        };

        sink.set_volume(0.8);

        let sample_buffer = new_shared_buffer(4096);
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
        })
    }

    pub fn play(&mut self, path: &str) -> Result<(), String> {
        self.sink.stop();
        self.current_path = Some(path.to_string());

        let path_buf = PathBuf::from(path);
        let file = std::fs::File::open(&path_buf).map_err(|e| e.to_string())?;
        let reader = BufReader::new(file);
        let source = Decoder::new(reader).map_err(|e| e.to_string())?;

        let sr = source.sample_rate();
        let ch = source.channels();
        self.current_sample_rate = sr;
        self.current_channels = ch;
        self.total_duration = source.total_duration();

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
                    source.convert_samples::<f32>(),
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

    pub fn pause(&mut self) {
        if let Some(start) = self.start_instant.take() {
            self.accumulated_ms += start.elapsed().as_millis() as u64;
            self.sink.pause();
            self.is_paused = true;
        }
    }

    pub fn resume(&mut self) {
        if self.start_instant.is_none() {
            self.sink.play();
            self.start_instant = Some(Instant::now());
            self.is_paused = false;
        }
    }

    pub fn seek_to(&mut self, target: Duration) {
        let pos_ms = target.as_millis() as u64;
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
                let running = if let Some(start) = &self.start_instant {
                    if !self.is_paused {
                        start.elapsed().as_millis() as u64
                    } else {
                        0
                    }
                } else {
                    0
                };
                let current_ms = self.accumulated_ms + running;
                if pos_ms >= current_ms {
                    // Forward seek: calculate samples to skip
                    let diff_ms = pos_ms - current_ms;
                    let channels = self.current_channels;
                    let sample_rate = self.current_sample_rate;
                    let samples_to_skip = (diff_ms as f64 / 1000.0
                        * sample_rate as f64
                        * channels as f64)
                        as u64;

                    self.skip_request.fetch_add(samples_to_skip, Ordering::Release);

                    self.accumulated_ms = pos_ms;
                    if self.start_instant.is_some() {
                        self.start_instant = Some(Instant::now());
                    }
                } else {
                    // Backward seek: reopen and fast-forward
                    if let Some(ref path) = self.current_path {
                        self.sink.stop();
                        if let Ok(new_sink) =
                            run_with_high_priority(|| Sink::try_new(&self.handle))
                        {
                            self.sink = new_sink;
                            self.sink.set_volume(self.volume as f32 / 100.0);

                            if let Ok(file) = std::fs::File::open(path) {
                                let reader = BufReader::new(file);
                                if let Ok(source) = Decoder::new(reader) {
                                    let sr = source.sample_rate();
                                    let ch = source.channels();
                                    self.current_sample_rate = sr;
                                    self.current_channels = ch;

                                    self.skip_request.store(0, Ordering::Release);
                                    let samples_to_skip = (pos_ms as f64 / 1000.0
                                        * sr as f64
                                        * ch as f64)
                                        as u64;

                                    let viz_source = VisualizerSource::new(
                                        source.convert_samples::<f32>(),
                                        self.sample_buffer.clone(),
                                        self.skip_request.clone(),
                                    );

                                    self.skip_request
                                        .store(samples_to_skip, Ordering::Release);

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
                            }
                        }
                    }
                }
            }
        }
    }

    pub fn stop(&mut self) {
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
        let running = if let Some(start) = &self.start_instant {
            if !self.is_paused {
                start.elapsed().as_millis() as u64
            } else {
                0
            }
        } else {
            0
        };
        Duration::from_millis(self.accumulated_ms + running)
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
