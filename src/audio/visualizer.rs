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
            fft_buffer: vec![Complex::new(0.0, 0.0); fft_size],
            magnitudes: vec![0.0; fft_size / 2],
            new_bars: vec![0.0; num_bars],
        }
    }

    /// Update the number of display bars (e.g., on terminal resize).
    #[allow(dead_code)]
    pub fn set_num_bars(&mut self, num_bars: usize) {
        self.num_bars = num_bars;
        self.bars.resize(num_bars, 0.0);
        self.new_bars.resize(num_bars, 0.0);
    }

    /// Process raw audio samples through FFT and produce bar magnitudes.
    pub fn process(&mut self, samples: &[f32], sample_rate: u32) {
        if samples.len() < self.fft_size || self.num_bars == 0 {
            return;
        }

        // Apply window and convert to complex directly into reused buffer
        for (i, item) in samples.iter().enumerate().take(self.fft_size) {
            self.fft_buffer[i] = Complex::new(item * self.window[i], 0.0);
        }

        // Perform FFT in place using the planned runner
        self.fft.process(&mut self.fft_buffer);

        // Compute magnitudes normalized by fft_size (only first half — Nyquist)
        let half = self.fft_size / 2;
        for i in 0..half {
            let c = self.fft_buffer[i];
            self.magnitudes[i] = (c.re * c.re + c.im * c.im).sqrt() / self.fft_size as f32;
        }

        // Map frequency bins to display bars in place
        map_to_bars_inplace(
            &self.magnitudes,
            &mut self.new_bars,
            sample_rate,
            self.fft_size,
        );

        // Smooth with attack/decay (matching kew's ballistics)
        let snap_threshold = 0.2f32;
        let fast_attack = 0.6f32;
        let slow_attack = 0.15f32;
        let decay = 0.14f32;

        for i in 0..self.num_bars {
            let target = self.new_bars.get(i).copied().unwrap_or(0.0);
            let current = self.bars[i];
            let delta = target - current;

            if delta > snap_threshold {
                self.bars[i] += delta * fast_attack;
            } else if delta > 0.0 {
                self.bars[i] += delta * slow_attack;
            } else {
                self.bars[i] += delta * decay;
            }

            // Clamp small values to zero
            if self.bars[i] < 0.01 {
                self.bars[i] = 0.0;
            }
        }
    }

    /// Get the current bar heights normalized to 0.0..1.0.
    #[allow(dead_code)]
    pub fn normalized_bars(&self) -> Vec<f32> {
        self.bars.clone()
    }

    /// Get raw bar values for block-char rendering (scaled to a max height).
    #[allow(dead_code)]
    pub fn scaled_bars(&self, max_height: u16) -> Vec<u16> {
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

    let width = 2.0f32.powf(1.0f32 / 6.0f32);
    let mut center_freq = min_freq;
    let reference_freq = min_freq.max(1.0f32);
    let max_freq_for_correction = 10000.0f32;
    let correction_per_octave = 3.0f32;

    let db_floor = -60.0f32;
    let db_ceil = -18.0f32;
    let emphasis = 1.3f32;

    for bar in bars.iter_mut().take(num_bars) {
        let center = if center_freq > nyquist {
            nyquist
        } else {
            let c = center_freq;
            center_freq *= factor;
            c
        };

        if center <= 0.0 || center > nyquist {
            *bar = 0.0;
            continue;
        }

        let lo = center / width;
        let hi = center * width;

        let mut bin_lo = (lo / bin_spacing).ceil() as usize;
        let mut bin_hi = (hi / bin_spacing).floor() as usize;

        bin_lo = bin_lo.min(num_bins - 1);
        bin_hi = bin_hi.min(num_bins - 1).max(bin_lo);

        let mut sum_sq = 0.0f32;
        let mut count = 0;

        for k in bin_lo..=bin_hi {
            if k < magnitudes.len() {
                let mag = magnitudes[k];
                sum_sq += mag * mag;
                count += 1;
            }
        }

        let rms = if count > 0 {
            (sum_sq / count as f32).sqrt()
        } else {
            1e-9f32
        };
        let mut db = 20.0f32 * rms.log10();

        // Pink noise EQ compensation (+3 dB / octave)
        let freq = center.min(max_freq_for_correction);
        let octaves_above_ref = (freq / reference_freq).log2();
        let correction = octaves_above_ref.max(0.0) * correction_per_octave;
        db += correction;

        // Normalize dB range to 0.0..1.0 ratio
        let mut ratio = (db - db_floor) / (db_ceil - db_floor);
        ratio = ratio.clamp(0.0, 1.0);
        ratio = ratio.powf(emphasis);

        if ratio < 0.1 {
            *bar = 0.0;
        } else {
            *bar = ratio;
        }
    }
}
