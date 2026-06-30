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
}

pub type SharedSampleBuffer = Arc<Mutex<SampleRingBuffer>>;

/// Creates a new shared sample buffer.
pub fn new_shared_buffer(capacity: usize) -> SharedSampleBuffer {
    Arc::new(Mutex::new(SampleRingBuffer::new(capacity)))
}

/// A source wrapper that taps audio samples into a shared buffer for visualization.
pub struct VisualizerSource<S> {
    inner: S,
    buffer: SharedSampleBuffer,
    channel_count: u16,
    channel_idx: u16,
}

impl<S> VisualizerSource<S>
where
    S: Source<Item = f32>,
{
    pub fn new(source: S, buffer: SharedSampleBuffer) -> Self {
        let channels = source.channels();
        Self {
            inner: source,
            buffer,
            channel_count: channels,
            channel_idx: 0,
        }
    }
}

impl<S> Iterator for VisualizerSource<S>
where
    S: Source<Item = f32>,
{
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        let sample = self.inner.next()?;

        // Only tap the first channel (mono mix for visualization)
        if self.channel_idx == 0 {
            if let Ok(mut buf) = self.buffer.try_lock() {
                buf.push(sample);
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
}
