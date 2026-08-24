use rustfft::{num_complex::Complex, Fft, FftPlanner};
use std::sync::Arc;

/// Visualizer mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisualizerMode {
    Spectrum,
    Braille,
}

impl VisualizerMode {
    pub fn toggle(self) -> Self {
        match self {
            VisualizerMode::Spectrum => VisualizerMode::Braille,
            VisualizerMode::Braille => VisualizerMode::Spectrum,
        }
    }
}

/// FFT-based audio visualizer engine.
/// Ported from kew's visuals.c and crav's audio processing.
pub struct VisualizerEngine {
    /// Number of FFT bins.
    fft_size: usize,
    /// Smoothed bar magnitudes for display (range 0.0..1.0).
    pub bars: Vec<f32>,
    /// Number of display bars.
    pub num_bars: usize,
    /// Blackman-Harris window coefficients.
    window: Vec<f32>,
    /// Pre-planned FFT forward runner
    fft: Arc<dyn Fft<f32>>,
    /// Pre-allocated FFT complex buffer
    fft_buffer: Vec<Complex<f32>>,
    /// Pre-allocated magnitudes buffer
    magnitudes: Vec<f32>,
    /// Pre-allocated temporary bars buffer
    new_bars: Vec<f32>,
}

impl VisualizerEngine {
    pub fn new(fft_size: usize, num_bars: usize) -> Self {
        let window = blackman_harris(fft_size);
        let mut planner = FftPlanner::new();
        let fft = planner.plan_fft_forward(fft_size);
        Self {
            fft_size,
            bars: vec![0.0; num_bars],
            num_bars,
            window,
            fft,
            fft_buffer: vec![Complex { re: 0.0, im: 0.0 }; fft_size],
            magnitudes: vec![0.0; fft_size / 2 + 1],
            new_bars: vec![0.0; num_bars],
        }
    }

    /// Resize display bars to match terminal width.
    pub fn resize_bars(&mut self, num_bars: usize) {
        if num_bars != self.num_bars {
            self.num_bars = num_bars;
            self.bars = vec![0.0; num_bars];
            self.new_bars = vec![0.0; num_bars];
        }
    }

    /// Process raw audio samples into smoothed frequency bars.
    pub fn process(&mut self, samples: &[f32], sample_rate: u32) {
        self.process_samples(samples, sample_rate);
    }

    /// Process raw audio samples into smoothed frequency bars.
    ///
    /// Takes a mono/stereo sample slice of length >= `fft_size`.
    /// Applies Blackman-Harris windowing, forward FFT, 1/3 octave grouping,
    /// and asymmetric smoothing (fast attack, slow release).
    pub fn process_samples(&mut self, samples: &[f32], sample_rate: u32) {
        if samples.len() < self.fft_size {
            // Decay existing bars if not enough samples
            for b in self.bars.iter_mut() {
                *b = (*b - 0.05).max(0.0);
            }
            return;
        }

        // Apply Blackman-Harris window to input samples (read most recent fft_size samples)
        let offset = samples.len() - self.fft_size;
        for i in 0..self.fft_size {
            let sample = samples[offset + i];
            self.fft_buffer[i] = Complex {
                re: sample * self.window[i],
                im: 0.0,
            };
        }

        // Run FFT
        self.fft.process(&mut self.fft_buffer);

        // Calculate magnitude spectrum (first N/2+1 bins)
        let num_bins = self.fft_size / 2 + 1;
        for i in 0..num_bins {
            let re = self.fft_buffer[i].re;
            let im = self.fft_buffer[i].im;
            self.magnitudes[i] = (re * re + im * im).sqrt();
        }

        // Map FFT bins to display bars
        map_to_bars_inplace(
            &self.magnitudes,
            &mut self.new_bars,
            sample_rate,
            self.fft_size,
        );

        // Apply asymmetric smoothing: fast rise, slow fall
        for (current, &target) in self.bars.iter_mut().zip(self.new_bars.iter()) {
            if target > *current {
                // Fast attack
                *current = *current * 0.3 + target * 0.7;
            } else {
                // Smooth decay
                *current = *current * 0.85 + target * 0.15;
            }
            // Clamp to valid range
            *current = current.clamp(0.0, 1.0);
        }
    }

    /// Decay visualizer bars during pause/silence.
    pub fn decay(&mut self) {
        for b in self.bars.iter_mut() {
            *b = (*b - 0.03).max(0.0);
        }
    }

    /// Convert smoothed bar magnitudes into display heights (for terminal rendering).
    pub fn bar_heights(&self, max_height: u16) -> Vec<u16> {
        self.bars
            .iter()
            .map(|&b| (b * max_height as f32).round() as u16)
            .collect()
    }
}

/// Blackman-Harris window function (from kew's visuals.c).
fn blackman_harris(size: usize) -> Vec<f32> {
    let a0 = 0.35875;
    let a1 = 0.48829;
    let a2 = 0.14128;
    let a3 = 0.01168;
    (0..size)
        .map(|n| {
            let x = 2.0 * std::f32::consts::PI * n as f32 / (size - 1) as f32;
            a0 - a1 * x.cos() + a2 * (2.0 * x).cos() - a3 * (3.0 * x).cos()
        })
        .collect()
}

/// Map FFT magnitude bins to display bars using logarithmic 1/3 octave bands,
/// dB scale normalization, and pink noise EQ compensation.
fn map_to_bars_inplace(magnitudes: &[f32], bars: &mut [f32], sample_rate: u32, fft_size: usize) {
    let num_bars = bars.len();
    if magnitudes.is_empty() || num_bars == 0 || sample_rate == 0 {
        for val in bars.iter_mut() {
            *val = 0.0;
        }
        return;
    }

    let num_bins = fft_size / 2 + 1;
    let bin_spacing = sample_rate as f32 / fft_size as f32;
    let nyquist = 0.5f32 * sample_rate as f32;

    // Center frequencies for 1/3 octave bands
    let min_freq = 25.0f32;
    let octave_fraction = 1.0f32 / 3.0f32;
    let factor = 2.0f32.powf(octave_fraction);

    for (bar_idx, bar) in bars.iter_mut().enumerate() {
        let center_freq = min_freq * factor.powi(bar_idx as i32);
        if center_freq > nyquist {
            *bar = 0.0;
            continue;
        }

        let lower_freq = center_freq / 2.0f32.powf(octave_fraction / 2.0);
        let upper_freq = center_freq * 2.0f32.powf(octave_fraction / 2.0);

        let lower_bin = ((lower_freq / bin_spacing).floor() as usize).min(num_bins - 1);
        let upper_bin = ((upper_freq / bin_spacing).ceil() as usize).min(num_bins - 1);

        let mut sum = 0.0f32;
        let mut count = 0;
        for &mag in &magnitudes[lower_bin..=upper_bin] {
            sum += mag;
            count += 1;
        }

        let avg = if count > 0 { sum / count as f32 } else { 0.0 };

        // Convert magnitude to dB (dynamic range ~60dB)
        let db = if avg > 1e-6 {
            20.0 * avg.log10()
        } else {
            -60.0
        };

        // Normalize -60dB..0dB to 0.0..1.0
        let norm = ((db + 60.0) / 60.0).clamp(0.0, 1.0);

        // Pink noise EQ curve compensation (boost highs by ~3dB/octave)
        let eq_boost = 1.0 + (bar_idx as f32 / num_bars as f32) * 0.6;
        *bar = (norm * eq_boost).clamp(0.0, 1.0);
    }
}
