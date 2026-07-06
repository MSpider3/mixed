use std::sync::{Arc, Mutex};

use rodio::Source;

/// Ring buffer for sharing audio samples with the visualizer.
pub struct SampleRingBuffer {
    buffer: Vec<f32>,
    write_pos: usize,
    pub sample_rate: u32,
}

impl SampleRingBuffer {
    pub fn new(capacity: usize) -> Self {
        Self {
            buffer: vec![0.0; capacity],
            write_pos: 0,
            sample_rate: 44100,
        }
    }

    pub fn push(&mut self, sample: f32) {
        self.buffer[self.write_pos] = sample;
        self.write_pos = (self.write_pos + 1) % self.buffer.len();
    }

    /// Push a whole batch of samples in one call (avoids per-sample locking).
    pub fn push_batch(&mut self, samples: &[f32]) {
        for &s in samples {
            self.buffer[self.write_pos] = s;
            self.write_pos = (self.write_pos + 1) % self.buffer.len();
        }
    }

    /// Read the latest N samples in chronological order.
    pub fn read_latest(&self, count: usize) -> Vec<f32> {
        let len = self.buffer.len();
        let count = count.min(len);
        let mut result = Vec::with_capacity(count);
        let start = (self.write_pos + len - count) % len;
        for i in 0..count {
            result.push(self.buffer[(start + i) % len]);
        }
        result
    }

    /// Write the latest `dest.len()` samples directly into a caller-provided
    /// slice, avoiding the heap allocation that `read_latest` requires.
    /// The caller pre-allocates the buffer once and reuses it every frame.
    #[inline]
    pub fn read_latest_into(&self, dest: &mut [f32]) {
        let count = dest.len();
        let len = self.buffer.len();
        let count = count.min(len);
        let start = (self.write_pos + len - count) % len;
        for (i, slot) in dest.iter_mut().enumerate().take(count) {
            *slot = self.buffer[(start + i) % len];
        }
    }
}

pub type SharedSampleBuffer = Arc<Mutex<SampleRingBuffer>>;

/// Creates a new shared sample buffer.
pub fn new_shared_buffer(capacity: usize) -> SharedSampleBuffer {
    Arc::new(Mutex::new(SampleRingBuffer::new(capacity)))
}

/// Batch size: number of mono samples accumulated before a single lock+flush.
/// At 44100 Hz mono this means ~1.5 ms between flushes — imperceptible lag
/// for visualization, but 64× fewer mutex acquisitions in the audio hot-path.
const BATCH_SIZE: usize = 64;

/// A source wrapper that taps audio samples into a shared buffer for visualization.
/// Lock acquisitions are batched every BATCH_SIZE mono samples to prevent the
/// per-sample try_lock() from causing ALSA underruns under load.
pub struct VisualizerSource<S> {
    inner: S,
    buffer: SharedSampleBuffer,
    channel_count: u16,
    channel_idx: u16,
    /// Local accumulation buffer — stack-allocated, flushed in one lock per batch.
    batch: [f32; BATCH_SIZE],
    batch_len: usize,
    pub skip_request: Arc<std::sync::atomic::AtomicU64>,
    pub internal_skip: u64,
}

impl<S> VisualizerSource<S>
where
    S: Source<Item = f32>,
{
    pub fn new(
        source: S,
        buffer: SharedSampleBuffer,
        skip_request: Arc<std::sync::atomic::AtomicU64>,
    ) -> Self {
        let channels = source.channels();
        Self {
            inner: source,
            buffer,
            channel_count: channels,
            channel_idx: 0,
            batch: [0.0; BATCH_SIZE],
            batch_len: 0,
            skip_request,
            internal_skip: 0,
        }
    }

    /// Flush the local batch to the shared ring buffer in a single lock acquisition.
    #[inline]
    fn flush_batch(&mut self) {
        if self.batch_len == 0 {
            return;
        }
        // try_lock: if contended, we silently drop this batch. The FFT thread
        // reads the ring at 34 ms intervals so one missed flush is invisible.
        if let Ok(mut buf) = self.buffer.try_lock() {
            buf.push_batch(&self.batch[..self.batch_len]);
        }
        self.batch_len = 0;
    }
}

impl<S> Iterator for VisualizerSource<S>
where
    S: Source<Item = f32>,
{
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        use std::sync::atomic::Ordering;

        let skip = self.skip_request.swap(0, Ordering::Acquire);
        if skip > 0 {
            self.internal_skip += skip;
        }

        while self.internal_skip > 0 {
            self.inner.next()?;
            self.internal_skip -= 1;
            self.channel_idx = (self.channel_idx + 1) % self.channel_count.max(1);
        }

        let sample = self.inner.next()?;

        // Only tap the first channel (mono mix for visualization).
        if self.channel_idx == 0 {
            if self.batch_len < BATCH_SIZE {
                self.batch[self.batch_len] = sample;
                self.batch_len += 1;
            }
            if self.batch_len == BATCH_SIZE {
                self.flush_batch();
            }
        }
        self.channel_idx = (self.channel_idx + 1) % self.channel_count.max(1);

        Some(sample)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl<S> Source for VisualizerSource<S>
where
    S: Source<Item = f32>,
{
    fn current_frame_len(&self) -> Option<usize> {
        self.inner.current_frame_len()
    }

    fn channels(&self) -> u16 {
        self.inner.channels()
    }

    fn sample_rate(&self) -> u32 {
        self.inner.sample_rate()
    }

    fn total_duration(&self) -> Option<std::time::Duration> {
        self.inner.total_duration()
    }

    fn try_seek(&mut self, pos: std::time::Duration) -> Result<(), rodio::source::SeekError> {
        self.inner.try_seek(pos)
    }
}
