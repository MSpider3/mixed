use rodio::Source;
use std::io::Write;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use std::time::Duration;

use mixed::audio::symphonia_source::SymphoniaSource;
use mixed::audio::visualizer::VisualizerEngine;
use mixed::audio::viz_source::{new_shared_buffer, VisualizerSource};

#[test]
fn test_symphonia_source_empty_file() {
    let path = std::env::temp_dir().join("mixed_test_empty.mp3");
    let _ = std::fs::File::create(&path);
    let path_str = path.to_str().unwrap();

    let res = SymphoniaSource::open_file(path_str);
    let _ = std::fs::remove_file(&path);

    match res {
        Err(e) => assert!(e.contains("0 bytes")),
        Ok(_) => panic!("Expected error for empty file"),
    }
}

#[test]
fn test_symphonia_source_corrupted_file() {
    let path = std::env::temp_dir().join("mixed_test_corrupt.m4a");
    let mut file = std::fs::File::create(&path).expect("Failed to create temp file");
    file.write_all(b"GARBAGE_DATA_NOT_A_VALID_AUDIO_FILE")
        .expect("Write failed");
    file.flush().expect("Flush failed");
    let path_str = path.to_str().unwrap();

    let res = SymphoniaSource::open_file(path_str);
    let _ = std::fs::remove_file(&path);

    assert!(res.is_err(), "Expected error for corrupted audio file");
}

#[test]
fn test_symphonia_source_nonexistent_file() {
    let res = SymphoniaSource::open_file("/nonexistent/path/to/song.flac");
    assert!(res.is_err(), "Expected error for nonexistent file");
}

#[test]
fn test_visualizer_silence_decays_to_zero() {
    let mut engine = VisualizerEngine::new(2048, 32);
    let silence = vec![0.0f32; 2048];

    // Process silence for 10 frames
    for _ in 0..10 {
        engine.process(&silence, 44100);
    }

    // All bars should be exactly 0.0 on silence
    let max_bar = engine
        .bars
        .iter()
        .cloned()
        .fold(0.0f32, |acc, x| acc.max(x));
    assert_eq!(
        max_bar, 0.0,
        "Visualizer should have 0.0 bar height during silence"
    );
}

#[test]
fn test_visualizer_bass_vs_treble_frequency_selectivity() {
    let mut bass_engine = VisualizerEngine::new(2048, 32);
    let mut treble_engine = VisualizerEngine::new(2048, 32);

    // 60 Hz bass kick tone
    let bass_samples: Vec<f32> = (0..2048)
        .map(|i| (2.0 * std::f32::consts::PI * 60.0 * (i as f32 / 44100.0)).sin() * 0.7)
        .collect();

    // 6000 Hz treble / cymbal tone
    let treble_samples: Vec<f32> = (0..2048)
        .map(|i| (2.0 * std::f32::consts::PI * 6000.0 * (i as f32 / 44100.0)).sin() * 0.7)
        .collect();

    // Feed samples through engines
    for _ in 0..5 {
        bass_engine.process(&bass_samples, 44100);
        treble_engine.process(&treble_samples, 44100);
    }

    // Bass engine should have strong energy in lower bars (0..8) and low energy in treble bars (20..31)
    let bass_low_max = bass_engine.bars[0..8]
        .iter()
        .cloned()
        .fold(0.0f32, f32::max);
    let bass_high_max = bass_engine.bars[20..32]
        .iter()
        .cloned()
        .fold(0.0f32, f32::max);
    assert!(
        bass_low_max > 0.20,
        "Bass tone should activate lower bars (got {})",
        bass_low_max
    );
    assert!(
        bass_low_max > bass_high_max * 2.0,
        "Bass tone should be concentrated in low bars (low: {}, high: {})",
        bass_low_max,
        bass_high_max
    );

    // Treble engine should have strong energy in high bars (20..31) and low energy in bass bars (0..8)
    let treble_high_max = treble_engine.bars[20..32]
        .iter()
        .cloned()
        .fold(0.0f32, f32::max);
    let treble_low_max = treble_engine.bars[0..8]
        .iter()
        .cloned()
        .fold(0.0f32, f32::max);
    assert!(
        treble_high_max > 0.20,
        "Treble tone should activate high bars (got {})",
        treble_high_max
    );
    assert!(
        treble_high_max > treble_low_max * 2.0,
        "Treble tone should be concentrated in high bars (high: {}, low: {})",
        treble_high_max,
        treble_low_max
    );
}

#[test]
fn test_visualizer_pipeline_shared_buffer_and_fft() {
    // Test that audio samples flow into the shared buffer through VisualizerSource
    // and produce active frequency bars in VisualizerEngine.
    let shared_buffer = new_shared_buffer(4096);
    let skip_request = Arc::new(AtomicU64::new(0));

    // Generate a 440 Hz sine wave as mock Source
    struct MockSineSource {
        sample_idx: usize,
        sample_rate: u32,
    }
    impl Iterator for MockSineSource {
        type Item = f32;
        fn next(&mut self) -> Option<Self::Item> {
            let t = self.sample_idx as f32 / self.sample_rate as f32;
            self.sample_idx += 1;
            Some((2.0 * std::f32::consts::PI * 440.0 * t).sin() * 0.8)
        }
    }
    impl Source for MockSineSource {
        fn current_frame_len(&self) -> Option<usize> {
            None
        }
        fn channels(&self) -> u16 {
            2
        }
        fn sample_rate(&self) -> u32 {
            44100
        }
        fn total_duration(&self) -> Option<Duration> {
            None
        }
    }

    let mock = MockSineSource {
        sample_idx: 0,
        sample_rate: 44100,
    };
    let mut viz_source = VisualizerSource::new(mock, shared_buffer.clone(), skip_request);

    // Pull 2048 samples through the VisualizerSource
    for _ in 0..2048 {
        let _ = viz_source.next();
    }

    // Read latest samples from shared buffer
    let mut scratch = vec![0.0f32; 2048];
    let sr = {
        let buf = shared_buffer.lock().unwrap();
        buf.read_latest_into(&mut scratch);
        buf.sample_rate
    };

    assert_eq!(sr, 44100, "Sample rate should be 44100");

    // Process with VisualizerEngine
    let mut engine = VisualizerEngine::new(2048, 32);
    engine.process(&scratch, sr);

    // At least one frequency bar should be active (> 0.0)
    let max_bar = engine
        .bars
        .iter()
        .cloned()
        .fold(0.0f32, |acc, x| acc.max(x));
    assert!(
        max_bar > 0.05,
        "VisualizerEngine should produce active bars from sine wave (max was {})",
        max_bar
    );
}

/// Helper to create a valid synthetic PCM WAV audio file for self-contained testing.
fn create_synthetic_wav(sample_rate: u32, channels: u16, duration_secs: f32) -> std::path::PathBuf {
    let num_samples = (sample_rate as f32 * duration_secs) as usize;
    let mut data = Vec::new();

    // RIFF header
    data.extend_from_slice(b"RIFF");
    let total_size = 36 + (num_samples * channels as usize * 2) as u32;
    data.extend_from_slice(&total_size.to_le_bytes());
    data.extend_from_slice(b"WAVE");

    // fmt chunk
    data.extend_from_slice(b"fmt ");
    data.extend_from_slice(&16u32.to_le_bytes()); // Chunk size 16 for PCM
    data.extend_from_slice(&1u16.to_le_bytes()); // PCM format
    data.extend_from_slice(&channels.to_le_bytes());
    data.extend_from_slice(&sample_rate.to_le_bytes());
    let byte_rate = sample_rate * channels as u32 * 2;
    data.extend_from_slice(&byte_rate.to_le_bytes());
    let block_align = channels * 2;
    data.extend_from_slice(&block_align.to_le_bytes());
    data.extend_from_slice(&16u16.to_le_bytes()); // Bits per sample

    // data chunk
    data.extend_from_slice(b"data");
    let data_size = (num_samples * channels as usize * 2) as u32;
    data.extend_from_slice(&data_size.to_le_bytes());

    for i in 0..num_samples {
        let t = i as f32 / sample_rate as f32;
        let val = (2.0 * std::f32::consts::PI * 440.0 * t).sin();
        let sample = (val * 16000.0) as i16;
        for _ in 0..channels {
            data.extend_from_slice(&sample.to_le_bytes());
        }
    }

    let path =
        std::env::temp_dir().join(format!("mixed_test_{}ch_{}hz.wav", channels, sample_rate));
    std::fs::write(&path, data).expect("Failed to write synthetic wav");
    path
}

#[test]
fn test_symphonia_source_synthetic_wav_playback_and_seeking() {
    let wav_path = create_synthetic_wav(44100, 2, 3.0);
    let path_str = wav_path.to_str().unwrap();

    let mut source =
        SymphoniaSource::open_file(path_str).expect("Failed to open synthetic WAV file");
    assert_eq!(
        source.channels(),
        2,
        "Synthetic WAV should be stereo (2 channels)"
    );
    assert_eq!(
        source.sample_rate(),
        44100,
        "Synthetic WAV should have 44100 Hz sample rate"
    );

    // Read first 1024 samples
    let samples: Vec<f32> = source.by_ref().take(1024).collect();
    assert_eq!(samples.len(), 1024, "Should read 1024 audio samples");
    let max_sample = samples
        .iter()
        .cloned()
        .fold(0.0f32, |acc, x| acc.max(x.abs()));
    assert!(
        max_sample > 0.1,
        "Samples should contain non-silent audio data"
    );

    // Test seek to 1 second
    let seek_res = source.try_seek(Duration::from_secs(1));
    assert!(seek_res.is_ok(), "Seek to 1s should succeed on WAV");

    // Read 512 samples after seek
    let samples_after_seek: Vec<f32> = source.by_ref().take(512).collect();
    assert_eq!(
        samples_after_seek.len(),
        512,
        "Should read samples after seek"
    );

    let _ = std::fs::remove_file(wav_path);
}

#[test]
fn test_symphonia_source_high_res_96khz_detection() {
    let wav_path = create_synthetic_wav(96000, 2, 2.0);
    let path_str = wav_path.to_str().unwrap();

    let mut source = SymphoniaSource::open_file(path_str).expect("Failed to open 96kHz WAV file");
    assert_eq!(source.channels(), 2, "Channel count should be 2");
    assert_eq!(
        source.sample_rate(),
        96000,
        "Sample rate should accurately detect 96000 Hz"
    );

    // Read 2048 samples
    let samples: Vec<f32> = source.by_ref().take(2048).collect();
    assert_eq!(samples.len(), 2048, "Should decode 2048 samples at 96kHz");

    let max_sample = samples
        .iter()
        .cloned()
        .fold(0.0f32, |acc, x| acc.max(x.abs()));
    assert!(
        max_sample > 0.1,
        "Decoded samples should contain audio data"
    );

    let _ = std::fs::remove_file(wav_path);
}

#[test]
fn test_symphonia_source_optional_external_file() {
    // If developer specifies TEST_AUDIO_FILE in env, verify it
    if let Ok(path) = std::env::var("TEST_AUDIO_FILE") {
        if std::path::Path::new(&path).exists() {
            let mut source =
                SymphoniaSource::open_file(&path).expect("Failed to open TEST_AUDIO_FILE");
            assert!(source.channels() >= 1);
            assert!(source.sample_rate() > 0);
            let samples: Vec<f32> = source.by_ref().take(1024).collect();
            assert_eq!(samples.len(), 1024);
        }
    }
}
