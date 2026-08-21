use rodio::Source;
use std::io::Write;
use std::time::Duration;

#[cfg(not(target_os = "android"))]
use mixed::audio::symphonia_source::SymphoniaSource;

#[test]
#[cfg(not(target_os = "android"))]
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
#[cfg(not(target_os = "android"))]
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
#[cfg(not(target_os = "android"))]
fn test_symphonia_source_nonexistent_file() {
    let res = SymphoniaSource::open_file("/nonexistent/path/to/song.flac");
    assert!(res.is_err(), "Expected error for nonexistent file");
}

#[test]
#[cfg(not(target_os = "android"))]
fn test_symphonia_source_real_m4a_if_available() {
    let path = "/run/media/mehulgolecha/OS/New folder/Spotify_playlist/01 - QUEEN BEE.m4a";
    if !std::path::Path::new(path).exists() {
        return;
    }

    let mut source = SymphoniaSource::open_file(path).expect("Failed to open real m4a file");
    assert!(source.channels() >= 1);
    assert!(source.sample_rate() > 0);

    // Read first 1024 samples
    let samples: Vec<f32> = source.by_ref().take(1024).collect();
    assert_eq!(samples.len(), 1024, "Should read 1024 audio samples");

    // Test seek to 5 seconds
    let seek_res = source.try_seek(Duration::from_secs(5));
    assert!(seek_res.is_ok(), "Seek to 5s should succeed on real m4a");

    // Read 512 samples after seek
    let samples_after_seek: Vec<f32> = source.by_ref().take(512).collect();
    assert_eq!(
        samples_after_seek.len(),
        512,
        "Should read samples after seek"
    );
}
