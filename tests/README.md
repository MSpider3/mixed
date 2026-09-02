# Test Suite & Verification Guide (`tests/`)

This directory contains the integration and system tests for `mixed`. The test suite is fully self-contained, portable, and runs in any CI environment without requiring external audio files.

---

## 📋 Overview of Test Suites

| Test File | Description |
| :--- | :--- |
| [`audio_playback_tests.rs`](audio_playback_tests.rs) | Tests audio decoding (`SymphoniaSource`), accurate sample rate detection (44.1 kHz, 96 kHz high-res), sample-accurate seeking, and FFT visualizer processing via self-contained synthetic audio generators. |
| [`headless_tests.rs`](headless_tests.rs) | Headless TUI rendering tests (first-run flow, library navigation, UI boundary layout limits, and MPRIS event bridge concurrency stress). |
| [`mpris_dbus.rs`](mpris_dbus.rs) | D-Bus MPRIS media controls dispatch tests (Linux). |
| `src/lib.rs` (Unit Tests) | Unit tests for CLI parsing, lyrics parser (standard LRC, enhanced LRC, word-by-word timestamps, Devanagari/Hindi syllable preservation), playlist order, and metadata sanitizer. |

---

## 🏃 Running Tests

### Run All Tests
```bash
cargo test
```

### Run a Specific Test Suite
```bash
# Audio playback & visualizer tests
cargo test --test audio_playback_tests

# Headless TUI integration tests
cargo test --test headless_tests

# MPRIS D-Bus integration tests
cargo test --test mpris_dbus

# Unit tests only
cargo test --lib
```

### Run a Single Specific Test
```bash
cargo test test_symphonia_source_high_res_96khz_detection
```

---

## 🔍 Testing with Real / Custom Audio Files

### 1. Automated Integration Test with Custom File
You can pass any custom audio file (e.g. `.m4a`, `.flac`, `.mp3`, `.wav`, `.ogg`, `.opus`) to the integration test suite using the `TEST_AUDIO_FILE` environment variable:

```bash
TEST_AUDIO_FILE="/path/to/my_song.m4a" cargo test --test audio_playback_tests test_symphonia_source_optional_external_file
```

### 2. Audio Inspection & Verification Tool
A built-in diagnostic tool is available under `examples/check_audio.rs` to inspect any audio file across 4 verification stages:
1. **Metadata & Lyrics (`lofty`)**: Tags, sample rate, bitrate, embedded timed LRC lines.
2. **Decoder Probe (`SymphoniaSource`)**: Decoded bitstream sample rate, channel layout, track duration.
3. **Stream Decoding**: Reads PCM samples, measures peak amplitude (dBFS) and RMS energy.
4. **Seek & Visualizer FFT**: Tests seeking to a timestamp and renders a live ASCII frequency spectrum.

```bash
cargo run --example check_audio -- "/path/to/my_song.m4a"
```

---

## 🛠️ Personal & Local Testing (`personal_tests/`)

For developer-specific testing with private playlists or large music collections:
- The `personal_tests/` folder is git-ignored and will never be committed to the repository.
- Use `personal_tests/quick_check.sh <file>` for quick single-file inspections.
- Use `personal_tests/check_playlist.sh <folder>` to batch-scan an entire directory of tracks for decoding errors, corrupted headers, or missing metadata.
