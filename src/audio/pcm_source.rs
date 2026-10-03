use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crossbeam_queue::ArrayQueue;
use rodio::Source;

/// Writer handle for feeding PCM samples into a `PcmSource`.
#[derive(Clone)]
pub struct PcmWriter {
    queue: Arc<ArrayQueue<f32>>,
    ended: Arc<AtomicBool>,
}

impl PcmWriter {
    /// Push a single audio sample into the queue.
    /// Returns Err(sample) if the queue is full.
    pub fn push(&self, sample: f32) -> Result<(), f32> {
        self.queue.push(sample)
    }

    /// Push a slice of audio samples. Returns the number of samples actually written.
    pub fn push_slice(&self, samples: &[f32]) -> usize {
        let mut written = 0;
        for &s in samples {
            if self.queue.push(s).is_ok() {
                written += 1;
            } else {
                break;
            }
        }
        written
    }

    /// Flush all buffered samples immediately (e.g., on seek or track change).
    pub fn flush(&self) {
        while self.queue.pop().is_some() {}
    }

    /// Mark whether the stream has reached the end of track.
    pub fn set_ended(&self, ended: bool) {
        self.ended.store(ended, Ordering::Release);
    }

    /// Returns true if the stream is marked as ended.
    pub fn is_ended(&self) -> bool {
        self.ended.load(Ordering::Acquire)
    }

    /// Returns the number of samples currently buffered.
    pub fn len(&self) -> usize {
        self.queue.len()
    }

    /// Returns true if the buffer is empty.
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }
}

/// A lock-free audio source implementing `rodio::Source<Item = f32>` over a bounded ring buffer.
/// Yields silence (0.0) on underruns to prevent premature stream termination during network jitter.
pub struct PcmSource {
    queue: Arc<ArrayQueue<f32>>,
    ended: Arc<AtomicBool>,
    channels: u16,
    sample_rate: u32,
}

impl PcmSource {
    /// Create a new `PcmSource` and corresponding `PcmWriter` with specified parameters.
    pub fn new(channels: u16, sample_rate: u32, capacity: usize) -> (Self, PcmWriter) {
        let queue = Arc::new(ArrayQueue::new(capacity));
        let ended = Arc::new(AtomicBool::new(false));

        let writer = PcmWriter {
            queue: queue.clone(),
            ended: ended.clone(),
        };

        let source = Self {
            queue,
            ended,
            channels,
            sample_rate,
        };

        (source, writer)
    }

    /// Flush all buffered samples from the reading side.
    pub fn flush(&self) {
        while self.queue.pop().is_some() {}
    }
}

impl Iterator for PcmSource {
    type Item = f32;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        if let Some(sample) = self.queue.pop() {
            Some(sample)
        } else if self.ended.load(Ordering::Acquire) {
            None
        } else {
            // Buffer underrun: yield silence so rodio sink doesn't terminate prematurely
            Some(0.0)
        }
    }
}

impl Source for PcmSource {
    #[inline]
    fn current_frame_len(&self) -> Option<usize> {
        None
    }

    #[inline]
    fn channels(&self) -> u16 {
        self.channels
    }

    #[inline]
    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    #[inline]
    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pcm_source_push_and_read() {
        let (mut source, writer) = PcmSource::new(2, 44100, 1024);
        assert_eq!(source.channels(), 2);
        assert_eq!(source.sample_rate(), 44100);

        let samples = [0.1f32, -0.2, 0.3, -0.4];
        let written = writer.push_slice(&samples);
        assert_eq!(written, 4);

        assert_eq!(source.next(), Some(0.1));
        assert_eq!(source.next(), Some(-0.2));
        assert_eq!(source.next(), Some(0.3));
        assert_eq!(source.next(), Some(-0.4));
    }

    #[test]
    fn test_pcm_source_underrun_yields_silence() {
        let (mut source, _writer) = PcmSource::new(2, 44100, 1024);
        // Queue is empty and ended is false -> should yield silence (0.0)
        assert_eq!(source.next(), Some(0.0));
        assert_eq!(source.next(), Some(0.0));
    }

    #[test]
    fn test_pcm_source_ended_returns_none_when_empty() {
        let (mut source, writer) = PcmSource::new(2, 44100, 1024);
        writer.push(0.5).unwrap();
        writer.set_ended(true);

        // First sample was queued
        assert_eq!(source.next(), Some(0.5));
        // Queue now empty and ended is true -> should return None
        assert_eq!(source.next(), None);
    }

    #[test]
    fn test_pcm_source_flush() {
        let (mut source, writer) = PcmSource::new(2, 44100, 1024);
        writer.push_slice(&[0.1, 0.2, 0.3, 0.4]);
        assert_eq!(writer.len(), 4);

        writer.flush();
        assert_eq!(writer.len(), 0);

        // When ended, it should immediately be None
        writer.set_ended(true);
        assert_eq!(source.next(), None);
    }
}
