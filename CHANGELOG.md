# Changelog

All notable changes to the **mixed** music player will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.4] - 2026-08-21

### Added
- **Command-Line Interface (CLI) Support**:
  - `--help` / `-h`: Displays formatted help documentation with usage, options, and keyboard shortcuts.
  - `--version` / `-v` / `-V`: Displays application version information (`mixed 0.1.4`).
  - `--dir <PATH>` / `-d <PATH>`: Launch directly into a specified music library folder.
  - `--play <FILE>` / `-p <FILE>`: Launch directly and immediately play a specified audio file.
  - Positional argument `mixed [PATH]`: Automatically opens the folder or plays the audio file on startup.
- **Dedicated Panic-Free Symphonia Audio Engine (`SymphoniaSource`)**:
  - Direct integration with `symphonia` for audio decoding and format probing.
  - `SizedFileSource` preserves exact file byte lengths to prevent container probes from failing with `Unseekable`.
  - Extension hints provided directly during format probing for instant container detection.
  - Full native seeking support across all codecs (M4A/AAC, MP3, FLAC, WAV, Ogg Vorbis, Opus, ALAC).
  - Robust error handling returning clean `Result<SymphoniaSource, String>` without uncatchable panics.
- **Automated Audio Integration Tests**:
  - Added test suite for empty (0-byte), corrupted, non-existent, and real `.m4a` audio files.

### Fixed
- Fixed an unhandled thread panic (`internal error: entered unreachable code: Seek errors should not occur during initialization`) in rodio's symphonia wrapper when opening `.m4a`/ISO MP4 audio files.
- Prevented application crashes on 0-byte or corrupted audio files during library scanning and playback.

---

## [0.1.3] - 2026-08-15

### Added
- MPRIS D-Bus integration for Linux desktop environments with real-time metadata and control signals.
- Sixel image caching and high-performance album art rendering.
- Batch sample accumulation buffer (`BATCH_SIZE = 64`) for lock-free audio visualization.
- Rayon parallel library scanner for multi-core directory indexing.

### Fixed
- Terminal raw mode clean restoration on unexpected panics via custom panic hook.
- CPU idle usage optimizations when music is paused.

---

## [0.1.2] - 2026-07-20

### Added
- Real-time FFT audio visualizer with Braille characters.
- Session state persistence (restoring playback position, volume, shuffle, repeat mode).
- Fuzzy search across library tracks and artists.

---

## [0.1.1] - 2026-06-10

### Added
- Synced LRC and unsynced lyrics parser with auto-scrolling.
- Hybrid seek engine with backward fast-forwarding.
- Keyboard shortcuts for playlist navigation and volume control.

---

## [0.1.0] - 2026-05-01

### Added
- Initial release of `mixed` terminal music player written in Rust.
- Core TUI interface powered by `ratatui` and `crossterm`.
- Multi-format audio playback backend.
