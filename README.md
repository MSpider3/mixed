# mixed

![Rust](https://img.shields.io/badge/Language-Rust-orange?logo=rust&logoColor=white)
![Ratatui](https://img.shields.io/badge/TUI-Ratatui-blue?logo=terminal&logoColor=white)
![Rodio](https://img.shields.io/badge/Audio-Rodio%20%2F%20Symphonia-purple?logo=audacity&logoColor=white)
![Linux](https://img.shields.io/badge/Platform-Linux-green?logo=linux&logoColor=white)
![macOS](https://img.shields.io/badge/Platform-macOS-lightgrey?logo=apple&logoColor=white)
![Windows](https://img.shields.io/badge/Platform-Windows-blue?logo=windows&logoColor=white)
[![License: GPL v3](https://img.shields.io/badge/License-GPLv3-blue.svg)](https://www.gnu.org/licenses/gpl-3.0)

> A next-generation terminal music player — built with Rust, designed for performance freaks.

---

## Origin & Inspiration

**mixed** is heavily inspired by the phenomenal C-based TUI player **[kew](https://github.com/ravachol/kew)**. `kew` is an incredible piece of software with elegant design and rock-solid audio playback. So why build another one?

This project was born from a desire to push beyond what's possible with a C foundation:

- **Design Overhaul** — A unique high-contrast **Neon-Noir / Material You** aesthetic with Sixel album art rendering, real-time spectrum & braille visualizers, and a deeply customizable layout system.
- **Lock-Free Concurrency** — Cross-thread state synchronization uses atomic primitives (`AtomicBool`, `AtomicU64`, `AtomicU8`) instead of mutexes, eliminating contention on the audio hot-path.
- **Native MPRIS Media Controls** — Full MPRIS D-Bus integration for Linux desktop environments (GNOME, KDE, Sway, Waybar, playerctl).
- **High-Performance Rendering** — Sixel image pooling with XDG-cached cover art protocols, zero-allocation FFT spectrum frames, and an adaptive rendering loop that idles at 0% CPU when paused.

**The Real Reason** - I started this project at the beginning of January 2026 as part of my new year resolution that after learning Rust for more than 6 months I will try to build a major project on my own in Rust. *'kew'* was major inspiration to try build a minimalist music player with things I like, but I wanted to try build it on my own for practice.

---

## Visual Showcase

### Now Playing
![Now Playing](docs/output.webp)

### Library
![Library Tab](docs/libary_tab.png)

### Playlist
![Playlist Tab](docs/playlist_tab.png)

### Search
![Search Tab](docs/search_tab.png)

---

## Core Features

### 🔊 High-Performance Audio Engine
Powered by **Rodio** and **Symphonia** with lock-free, zero-allocation sample tracking. Audio decoding runs on an isolated background thread with `nice(-10)` priority elevation on Linux. Sample data flows to the visualizer through a batched ring buffer (`BATCH_SIZE = 64`) that reduces mutex acquisitions by 64× compared to per-sample locking.

### ⏩ Hybrid Seek System
Bulletproof seeking that natively calls `try_seek()` on indexed audio formats (FLAC, WAV) and seamlessly falls back to a **sample-discarding iterator** for unseekable or variable-bitrate files (MP3, Ogg). Backward seeks gracefully reopen the decoder and fast-forward via atomic `skip_request` counters — no stalls, no glitches.

### 🐧 MPRIS Background Media Controls (Linux)
Full **MPRIS D-Bus** integration for Linux compositors (GNOME, KDE, Sway, Hyprland) with real-time `PropertiesChanged` signal emission for metadata, playback status, volume, shuffle, and loop state.

### ⚡ Zero-Spin Event Loop
Intelligent, adaptive rendering powered by `crossbeam_channel::select!` multiplexing. The visualizer thread fires wake-up signals at **~30 fps** (34ms cadence) through a `bounded(1)` channel when music is playing. When idle, the FFT thread decays to silence and the main loop drops to **near 0% CPU** — no busy-waiting, no wasted cycles.

### 🎨 Native Terminal Theme Integration
**mixed** inherits standard ANSI color palette codes and uses `Color::Reset` for default text. This means the player automatically respects and adapts to your terminal emulator's custom theme (e.g. Catppuccin, Gruvbox, Nord, Dracula) for a consistent, native desktop look.

### 🖼 Sixel & Kitty Cover Art
Album art is rendered directly in the terminal using high-performance image protocols (Sixel and Kitty Graphics Protocol) powered by `ratatui-image`. To see high-resolution cover art, use an image-enabled terminal emulator such as Kitty, WezTerm, Foot, Konsole, or Alacritty (v0.14+ with Sixel support enabled). The player automatically falls back to a text placeholder if image protocols are unsupported.

---

## Installation

> **Note:** `mixed` is distributed as a **standalone, self-contained binary** with all audio decoders, TUI components, and metadata parsers statically compiled in. It requires **no prior installation or runtime libraries** (except standard ALSA libraries on minimal headless Linux distributions).

### Method 1: Pre-compiled Binaries (Recommended)

You can download the pre-compiled binary for your system from the **[Releases](../../releases)** page.

#### Linux (x86_64)
1. Download `mixed-v1.4.2-x86_64-unknown-linux-gnu.tar.gz`.
2. Extract the archive:
   ```bash
   tar -xzf mixed-v1.4.2-x86_64-unknown-linux-gnu.tar.gz
   ```
3. Move the `mixed` binary to your system PATH (e.g. `/usr/local/bin`):
   ```bash
   sudo mv mixed /usr/local/bin/
   ```
4. Run the player by typing `mixed` in your terminal.

#### macOS (Apple Silicon or Intel)
1. Download either `mixed-v1.4.2-aarch64-apple-darwin.tar.gz` (Apple Silicon) or `mixed-v1.4.2-x86_64-apple-darwin.tar.gz` (Intel).
2. Extract the archive:
   ```bash
   tar -xzf mixed-v1.4.2-*.tar.gz
   ```
3. Move `mixed` to a directory in your PATH (e.g. `/usr/local/bin`):
   ```bash
   mv mixed /usr/local/bin/
   ```
4. *Note:* If macOS Gatekeeper prevents execution, run the following to bypass the developer warning:
   ```bash
   xattr -cr /usr/local/bin/mixed
   ```

#### Windows (x86_64)
1. Download `mixed-v1.4.2-x86_64-pc-windows-msvc.zip`.
2. Extract the `.zip` file.
3. Move `mixed.exe` to a folder of your choice and run it in a terminal emulator (Windows Terminal, PowerShell, or Command Prompt).

---

### Method 2: Build from Source

If you have Rust and Cargo installed:

```bash
# Clone the repository
git clone https://github.com/MSpider3/mixed.git
cd mixed

# Build the optimized release binary
cargo build --release

# Run
./target/release/mixed
```

The release profile uses `opt-level = 3`, fat LTO, single codegen unit, and symbol stripping for maximum performance.

---

## CLI Usage

```bash
# Print help documentation
mixed --help

# Print version
mixed --version

# Open mixed with a specific music directory
mixed ~/Music
# or
mixed --dir ~/Music

# Directly play an audio file on startup
mixed /path/to/song.mp3
# or
mixed --play /path/to/song.flac
```

---

## Keybind Reference

| Keybind | Action | Context |
|---|---|---|
| `F2 - F6` | Switch views (Queue/Library/Now Playing/Search/Help) | Navigation |
| `Tab` / `Shift+Tab` | Cycle active panel / view | Navigation |
| `k` / `j` / `↑` / `↓` | Scroll / Navigate list items | Navigation |
| `q` / `Esc` | Quit / Graceful Exit | System |
| `Space` / `p` | Play / Pause toggle | Playback |
| `S` | Stop playback | Playback |
| `n` / `l` / `→` | Next track | Playback |
| `p` / `h` / `←` | Previous track | Playback |
| `a` / `d` | Seek backward / forward 5 seconds | Playback |
| `+` / `=` | Volume up | Playback |
| `-` / `[` | Volume down | Playback |
| `s` | Toggle shuffle mode | Playback |
| `r` | Cycle repeat mode (off → track → queue) | Playback |
| `Enter` | Enqueue selected item / Play selected queue item | Queue / Library |
| `Alt + Enter` | Enqueue selected item and play immediately | Queue / Library |
| `o` / `←` / `→` | Toggle / Collapse/Expand directory tree | Library |
| `Delete` | Remove selected track from the queue | Queue |
| `Backspace` | Clear the entire queue (stops playback) | Queue |
| `f` / `g` | Move selected queue item up / down | Queue |
| `/` | Open search prompt to filter library | Library |
| `v` | Toggle spectrum/braille visualizer mode | Display |
| `m` | Toggle full lyrics / 3-line timed lyrics view | Display |

---

## License

This project is licensed under the **GNU General Public License v3.0 (GPLv3)** — see the [LICENSE](LICENSE) file for details.
