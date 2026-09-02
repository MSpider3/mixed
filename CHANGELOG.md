# Changelog

All notable changes to the **mixed** music player will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [1.5.0] - 2026-09-03

### Added
- **Interactive Mini-Controls in Left Pane**:
  - Compact, responsive transport widget (`[⏮  ▶/⏸  ⏭  +  -  ∅]`) displayed directly below the album art in the left pane across Playlist (F2), Library (F3), Search (F5), and Help (F6) tabs.
  - Album art and mini-controls are vertically centered together as a cohesive block in the middle of the left pane.
  - Dedicated mouse click targets with exact column periodic alignment for Previous, Play/Pause, Next, Volume Up/Down, and Clear Playlist (`∅`).
- **Visual Scrollbars on Lists**:
  - Vertical scrollbars (`ratatui::widgets::Scrollbar`) rendered alongside Playlist, Library, and Search result lists when content overflows screen height.
  - Clean, line-free aesthetic: track lines and arrows removed, keeping only the solid thumb handle block (`█`) moving with scroll position.
  - Full mouse click and drag seeking supported along the scrollbar track.
- **Native Desktop Notifications (Linux)**:
  - Track change notifications sent over D-Bus (`org.freedesktop.Notifications`) displaying song title, artist, and album.
- **Instant Browser Web Search for Song & Artist**:
  - Search the current track title or artist directly in your default web browser (Google Search).
  - Press `b` / `B` from anywhere to look up the currently playing artist.
  - In the Track (Now Playing, F4) tab, click directly on the song title to search for the song, or click on the artist name to search for the artist.
  - Fully isolated process execution: subprocess stdio is redirected to `/dev/null` to prevent browser warnings and D-Bus diagnostic logs from bleeding into the terminal.
- **Self-Contained Integration Test Suite**:
  - Added comprehensive test suite documentation in `tests/README.md` and automated headless tests for mini-controls, scrollbar interactions, and playback edge cases.

### Fixed
- **Playlist End Loop on Repeat Off**:
  - Fixed an issue where reaching the end of the playlist with `RepeatMode::Off` restarted the final track indefinitely instead of halting playback.
- **Smart `prev_track` Behavior**:
  - Pressing Previous Track (`⏮`) now restarts the current track from `0:00` if more than 3 seconds have elapsed (matching standard media player behavior), and skips to the previous song otherwise.
- **Queue Deletion & Dequeue Ghost State**:
  - Deleting or dequeueing the final track in the playlist now cleanly halts playback, cleans up temporary cover files, and resets UI/MPRIS state.
- **Search Cursor Clamping**:
  - Clamped `search_cursor` on `Down` key presses, preventing cursor drift past search results.
- **Directory Collapse Cursor Stranding**:
  - Clamped `library_cursor` to visible items when collapsing parent directories.
- **Progress Bar 100% Edge Seeking**:
  - Fixed denominator calculation to allow clicking the rightmost cell to seek to 100%.
- **Browser Search Terminal Output Bleed**:
  - Redirected `stdin`, `stdout`, and `stderr` to null when opening web search URLs, preventing browser diagnostic output from corrupting the TUI.
- **Terminal Height Underflow Guard**:
  - Added safety guard when terminal height is < 10 rows to prevent vertical layout split underflows.

---

## [1.4.3] - 2026-08-26

### Fixed
- **Indic / Hindi Character Duplication & Seeking Distortion**:
  - Resolved character duplication (e.g. `दिदिल`, `पपस`, `रहतीती`, `साँसाँस`) caused by 30 FPS terminal cursor desynchronization when rendering complex combining marks and matras.
  - Implemented automatic full-canvas clearing (`force_terminal_clear`) upon seeking, forwarding, rewinding, or active lyric line transitions to ensure a clean repaint.
  - Throttled visualizer redraws in Full Lyrics Mode to eliminate rapid partial diff artifacts while maintaining responsive 250ms lyric updates.
  - Sanitized escaped quotes and backslashes in LRC files.

---

## [1.4.2] - 2026-08-26

### Fixed
- **Hindi & Indic Script Font Readability**:
  - Resolved font distortion on Hindi (Devanagari) and other Indic scripts (Bengali, Tamil, Telugu, Gujarati, etc.) caused by synthetic bolding in terminal font shapers. Active lyric lines now render cleanly without ligature/matra corruption.
- **Visualizer Bleeding Prevention**:
  - Added explicit buffer clearing (`Clear` widget) to both lyrics and visualizer render regions, preventing visualizer characters from bleeding or ghosting into lyrics during window resize or track changes.
- **Word-by-Word Lyrics & Enhanced LRC Timestamp Stripping**:
  - Fixed an issue where word-by-word synced lyrics displayed raw inline timestamps (e.g., `<00:12.34>`, `[00:12.80]`, `(00:12.34,400)`, `{\k50}`).
  - Built an advanced LRC tokenizer supporting `<mm:ss.xx>`, `[mm:ss.xx]`, QRC `(mm:ss.xx,dur)`, and karaoke tags for both external `.lrc` files and embedded metadata tags.
  - Implemented smooth word-by-word highlighting during playback.

---

## [1.4.1] - 2026-08-24

### Fixed
- **Visualizer Shared Sample Buffer & Calibration Bug**:
  - Fixed a regression where `RodioBackend` created an isolated, unshared audio sample buffer rather than writing into the `Player`'s shared sample buffer.
  - Corrected FFT magnitude normalization by calibrating against Blackman-Harris window coherent gain, resolving the issue where bars saturated at maximum height on song start.
  - Implemented 32 logarithmic musical frequency bands (25 Hz - 18 kHz) across the entire spectrum.
  - Introduced hybrid peak/RMS transient detection with dynamic range expansion (`x^1.35`) and snappy attack / fluid decay ballistics to ensure bars bounce dynamically with the beat and rhythm.
  - Enhanced Braille visualizer with 8-dot vertical resolution patterns for continuous waveform rendering.
  - Added unit test suite in `tests/audio_playback_tests.rs` for visualizer frequency selectivity, silence decay, and sample pipeline flow.

### Changed
- **Pure Desktop PC Architecture**:
  - Completely removed Android/Termux experimental backends, scripts, and documentation in favor of a strictly optimized, unified PC desktop codebase (Linux, macOS, Windows).
  - Streamlined `release.yml` GitHub Actions CI workflow to build optimized binaries for Linux x86_64, macOS Apple Silicon, macOS Intel, and Windows x86_64.

---

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
