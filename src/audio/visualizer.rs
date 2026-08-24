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
/// Calibrated for dynamic beat response, wide dynamic range, and fluid ballistics.
pub struct VisualizerEngine {
    /// Number of FFT bins.
    fft_size: usize,
    /// Smoothed bar magnitudes for display (range 0.0..1.0).
    pub bars: Vec<f32>,
    /// Number of display bars.
    pub num_bars: usize,
    /// Blackman-Harris window coefficients.
    window: Vec<f32>,
    /// Coherent gain scale factor to normalize FFT magnitudes to 0 dBFS.
    window_scale: f32,
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
        let window_sum: f32 = window.iter().sum();
        let window_scale = if window_sum > 0.0 {
            1.0 / (window_sum * 0.5)
        } else {
            1.0 / (fft_size as f32 * 0.25)
        };

        let mut planner = FftPlanner::new();
        let fft = planner.plan_fft_forward(fft_size);
        Self {
            fft_size,
            bars: vec![0.0; num_bars],
            num_bars,
            window,
            window_scale,
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
    /// Takes a sample slice of length >= `fft_size`.
    /// Applies Blackman-Harris windowing, forward FFT, logarithmic frequency band mapping,
    /// dynamic contrast expansion, and asymmetric attack/decay smoothing.
    pub fn process_samples(&mut self, samples: &[f32], sample_rate: u32) {
        if samples.len() < self.fft_size {
            // Smoothly decay existing bars if not enough samples
            for b in self.bars.iter_mut() {
                *b = (*b * 0.85 - 0.02).max(0.0);
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

        // Calculate normalized magnitude spectrum (0.0..1.0 relative to full-scale 0 dBFS)
        let num_bins = self.fft_size / 2 + 1;
        for i in 0..num_bins {
            let re = self.fft_buffer[i].re;
            let im = self.fft_buffer[i].im;
            self.magnitudes[i] = (re * re + im * im).sqrt() * self.window_scale;
        }

        // Map FFT bins to logarithmic display bars
        map_to_bars_inplace(
            &self.magnitudes,
            &mut self.new_bars,
            sample_rate,
            self.fft_size,
        );

        // Apply asymmetric smoothing: fast attack (snappy beat response), smooth rhythmic fall
        for (current, &target) in self.bars.iter_mut().zip(self.new_bars.iter()) {
            if target > *current {
                // Fast attack: jumps to target quickly when drums/beats hit
                *current = *current * 0.15 + target * 0.85;
            } else {
                // Smooth rhythmic decay: fluid falloff
                *current = *current * 0.75 + target * 0.25;
            }
            // Clamp to valid range
            *current = current.clamp(0.0, 1.0);
        }
    }

    /// Decay visualizer bars during pause/silence.
    pub fn decay(&mut self) {
        for b in self.bars.iter_mut() {
            *b = (*b * 0.85 - 0.02).max(0.0);
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

/// Blackman-Harris window function.
fn blackman_harris(size: usize) -> Vec<f32> {
    if size <= 1 {
        return vec![1.0; size];
    }
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

/// Map FFT magnitude bins to display bars using logarithmic musical frequency bands,
/// dynamic decibel normalization, power-curve contrast, and equal-loudness compensation.
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

    // Musical frequency range: 25 Hz (sub-bass) to 18 kHz (air/treble)
    let min_freq = 25.0f32;
    let max_freq = (sample_rate as f32 * 0.45).min(18000.0).max(min_freq * 2.0);
    let log_min = min_freq.ln();
    let log_max = max_freq.ln();

    for (bar_idx, bar) in bars.iter_mut().enumerate() {
        let f_low = (log_min + (bar_idx as f32 / num_bars as f32) * (log_max - log_min)).exp();
        let f_high =
            (log_min + ((bar_idx + 1) as f32 / num_bars as f32) * (log_max - log_min)).exp();

        let lower_bin = ((f_low / bin_spacing).floor() as usize).clamp(0, num_bins - 1);
        let upper_bin = ((f_high / bin_spacing).ceil() as usize).clamp(lower_bin, num_bins - 1);

        // Combine peak and RMS energy to capture both sharp transients (kicks/snares) and tonal body
        let mut peak_mag = 0.0f32;
        let mut energy_sum = 0.0f32;
        let mut bin_count = 0;
        for &mag in &magnitudes[lower_bin..=upper_bin] {
            if mag > peak_mag {
                peak_mag = mag;
            }
            energy_sum += mag * mag;
            bin_count += 1;
        }
        let rms_mag = if bin_count > 0 {
            (energy_sum / bin_count as f32).sqrt()
        } else {
            0.0
        };

        // 60% peak + 40% RMS for punchy beat reactivity
        let combined_mag = peak_mag * 0.60 + rms_mag * 0.40;

        // Convert normalized magnitude to dB (dynamic range: -45 dBFS to 0 dBFS)
        let db = if combined_mag > 1e-5 {
            20.0 * combined_mag.log10()
        } else {
            -45.0
        };

        // Normalize -45 dB..0 dB to 0.0..1.0
        let norm = ((db + 45.0) / 45.0).clamp(0.0, 1.0);

        // Power curve / Gamma correction (x^1.35) expands dynamic contrast,
        // making beats pop sharply against background noise
        let contrast_norm = norm.powf(1.35);

        // Equal-loudness / Pink noise compensation curve:
        // Boost higher frequencies so hi-hats and vocals match the visual amplitude of bass kicks
        let eq_boost = 1.0 + (bar_idx as f32 / num_bars as f32) * 0.75;
        *bar = (contrast_norm * eq_boost).clamp(0.0, 1.0);
    }
}
