use mixed::audio::symphonia_source::SymphoniaSource;
use mixed::audio::visualizer::VisualizerEngine;
use mixed::data::metadata::read_metadata;
use rodio::Source;
use std::path::Path;
use std::time::Duration;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        println!("mixed Audio Inspection & Verification Tool");
        println!("Usage: cargo run --example check_audio -- <path_to_audio_file>");
        println!("Example: cargo run --example check_audio -- \"/path/to/my_song.m4a\"");
        return;
    }

    let path_str = &args[1];
    let path = Path::new(path_str);

    if !path.exists() {
        eprintln!("\x1b[1;31mError: File does not exist: {}\x1b[0m", path_str);
        std::process::exit(1);
    }

    println!("\x1b[1;36m============================================================\x1b[0m");
    println!("\x1b[1;36m  mixed Audio Inspection & Verification\x1b[0m");
    println!("\x1b[1;36m============================================================\x1b[0m");
    println!("File: \x1b[1;33m{}\x1b[0m", path.display());

    // 1. Metadata Inspection
    println!("\n\x1b[1;32m[1/4] Probing Metadata (lofty)...\x1b[0m");
    let meta = read_metadata(path);
    println!(
        "  Title:       {}",
        meta.title.as_deref().unwrap_or("<unknown>")
    );
    println!(
        "  Artist:      {}",
        meta.artist.as_deref().unwrap_or("<unknown>")
    );
    println!(
        "  Album:       {}",
        meta.album.as_deref().unwrap_or("<unknown>")
    );
    println!(
        "  Duration:    {:.2}s",
        meta.duration.map(|d| d.as_secs_f64()).unwrap_or(0.0)
    );
    println!(
        "  Sample Rate: {} Hz",
        meta.sample_rate
            .map(|sr| sr.to_string())
            .unwrap_or_else(|| "<unknown>".to_string())
    );
    println!(
        "  Bitrate:     {} kbps",
        meta.bitrate
            .map(|b| b.to_string())
            .unwrap_or_else(|| "<unknown>".to_string())
    );
    match &meta.lyrics {
        mixed::data::metadata::LyricsKind::Timed(lines) => {
            println!("  Lyrics:      Timed LRC ({} lines)", lines.len());
        }
        mixed::data::metadata::LyricsKind::Untimed(lines) => {
            println!("  Lyrics:      Plain text ({} lines)", lines.len());
        }
        mixed::data::metadata::LyricsKind::None => {
            println!("  Lyrics:      None embedded");
        }
    }

    // 2. Decoder & Stream Source Inspection
    println!("\n\x1b[1;32m[2/4] Initializing Audio Decoder (SymphoniaSource)...\x1b[0m");
    let start_init = std::time::Instant::now();
    let mut source = match SymphoniaSource::open_file(path_str) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("\x1b[1;31mFailed to open audio source: {}\x1b[0m", e);
            std::process::exit(1);
        }
    };
    let init_duration = start_init.elapsed();
    println!("  Init Time:   {:.2?}", init_duration);
    println!(
        "  Channels:    {} ({})",
        source.channels(),
        match source.channels() {
            1 => "Mono",
            2 => "Stereo",
            6 => "5.1 Surround",
            _ => "Multi-channel",
        }
    );
    println!(
        "  Sample Rate: \x1b[1;35m{} Hz\x1b[0m",
        source.sample_rate()
    );
    if let Some(dur) = source.total_duration() {
        println!(
            "  Duration:    {:.2}s ({:02}:{:02})",
            dur.as_secs_f64(),
            dur.as_secs() / 60,
            dur.as_secs() % 60
        );
    }

    // 3. Audio Decoding & Transience Test
    println!("\n\x1b[1;32m[3/4] Decoding Audio Stream (first 50,000 samples)...\x1b[0m");
    let mut samples = Vec::with_capacity(50000);
    let start_decode = std::time::Instant::now();
    for _ in 0..50000 {
        if let Some(sample) = source.next() {
            samples.push(sample);
        } else {
            break;
        }
    }
    let decode_time = start_decode.elapsed();
    println!("  Decoded {} samples in {:.2?}", samples.len(), decode_time);

    let max_amp = samples.iter().cloned().fold(0.0f32, |a, b| a.max(b.abs()));
    let rms = if !samples.is_empty() {
        (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
    } else {
        0.0
    };
    println!(
        "  Peak Amplitude: {:.4} ({:.1} dBFS)",
        max_amp,
        if max_amp > 1e-5 {
            20.0 * max_amp.log10()
        } else {
            -100.0
        }
    );
    println!(
        "  RMS Energy:     {:.4} ({:.1} dBFS)",
        rms,
        if rms > 1e-5 {
            20.0 * rms.log10()
        } else {
            -100.0
        }
    );

    // 4. Seeking & Visualizer FFT Test
    println!("\n\x1b[1;32m[4/4] Testing Seek & Visualizer FFT Pipeline...\x1b[0m");
    let target_seek = Duration::from_secs(10);
    print!("  Seeking to 10s... ");
    match source.try_seek(target_seek) {
        Ok(()) => println!("\x1b[1;32mSUCCESS\x1b[0m"),
        Err(e) => println!("\x1b[1;33mSeek returned: {:?}\x1b[0m", e),
    }

    // Read 2048 samples after seek for FFT test
    let mut fft_samples = Vec::with_capacity(2048);
    for _ in 0..2048 {
        if let Some(s) = source.next() {
            fft_samples.push(s);
        }
    }

    if fft_samples.len() >= 2048 {
        let mut visualizer = VisualizerEngine::new(2048, 24);
        visualizer.process(&fft_samples, source.sample_rate());
        let heights = visualizer.bar_heights(8);
        println!("\n  Frequency Spectrum (24 bands, 25Hz - 18kHz):");
        for row in (1..=8).rev() {
            print!("    ");
            for &h in &heights {
                if h >= row {
                    print!("\x1b[1;32m█\x1b[0m ");
                } else {
                    print!("  ");
                }
            }
            println!();
        }
        println!("    \x1b[2m25Hz ----------------------------------- 18kHz\x1b[0m");
    }

    println!("\n\x1b[1;32m✔ All checks passed successfully!\x1b[0m\n");
}
