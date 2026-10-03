# Architecture Overview

> **Audience:** Contributors and maintainers looking to understand `mixed`'s runtime threading model before making code changes.

---

## Thread Model

`mixed` runs five main execution contexts that communicate through atomic state and crossbeam channels. The audio thread additionally owns a small Tokio runtime for `librespot` when the `spotify` feature is enabled. Short-lived helper threads handle input reading, the 250ms tick, library scanning, and cache writes.

The only mutexes near the audio path are the visualizer ring buffer (taken with `try_lock`, in batches) and the handle that lets the Spotify sink swap its ring-buffer writer between tracks.

```
┌────────────────────────────────────────────────────────────────────────┐
│                            MAIN PROCESS                                │
│                                                                        │
│  ┌──────────────┐   ┌──────────────┐   ┌────────────────────────────┐  │
│  │  Thread 1:   │   │  Thread 2:   │   │  Thread 3:                 │  │
│  │  TUI Render  │   │  Audio       │   │  FFT Visualizer            │  │
│  │  Loop        │   │  Playback    │   │                            │  │
│  │              │   │              │   │  34ms cadence               │  │
│  │  select! {   │   │  Rodio Sink  │   │  VisualizerEngine          │  │
│  │    event_rx  │◄──│  Decoder     │──►│  SampleRingBuffer          │  │
│  │    tick_rx   │   │  PcmSource   │   │  bounded(1) wake-up        │  │
│  │    vis_wake  │   │  Priority -10│   │                            │  │
│  │    source_rx │   └──────────────┘   └────────────────────────────┘  │
│  │    player_ev │                                                      │
│  │    media_cmd │                                                      │
│  │    lib_rx    │   ┌────────────────────────┐  ┌───────────────────┐  │
│  │    player_rx │   │ Thread 4: MPRIS D-Bus  │  │ Thread 5: Async   │  │
│  │  }           │◄──│ Tokio async D-Bus      │  │ Network Runtime   │  │
│  │              │   │ org.mpris.MediaPlayer2 │  │ Spotify / YouTube │  │
│  │  layout::    │   └────────────────────────┘  └───────────────────┘  │
│  │    draw()    │◄─────────────────────────────────────────┘           │
│  └──────────────┘                                                      │
└────────────────────────────────────────────────────────────────────────┘
```

---

## Thread 1: TUI Render Loop (`main.rs`)

The main thread owns the `Terminal`, the `App` state struct, and the `crossbeam_channel::select!` multiplexer. It never blocks on I/O or computation — every operation is either instant (atomic load) or dispatched to a background thread.

**Event sources multiplexed in `select!`:**

| Channel | Type | Source |
|---|---|---|
| `event_rx` | `crossterm::Event` | Dedicated keyboard/mouse reader thread |
| `tick_rx` | `()` | 250ms periodic timer thread |
| `vis_wake_rx` | `()` | FFT visualizer (bounded(1)) |
| `media_cmd_rx` | `MediaCommand` | MPRIS D-Bus |
| `lib_rx` | `Vec<LibraryEntry>` | Background library scanner |
| `player_rx` | `Player` | One-shot player initialization |
| `player_event_rx` | `PlayerEvent` | Audio thread: `Loaded`, `Buffering`, `Failed`, `Finished`, `Position` |
| `source_event_rx` | `SourceEvent` | Async network runtime (auth, browse, search, covers, downloads) |

After each `select!` wake-up the loop also applies two debounces: a pending seek is sent to the player 250ms after the last seek key, and a remote search is dispatched 300ms after the last keystroke.

**Load generations:** every `Player::load*` call increments a generation counter that is carried by the `PlayerCmd::Load` and echoed in each `PlayerEvent`. `App` ignores events whose generation is not the current `load_generation`, so a slow load of a skipped track cannot affect the track that replaced it. Auto-advance is driven by the `Finished` event, not by polling. A `Failed` event skips to the next track, and three consecutive failures stop playback.

**Rendering strategy:**

- `refresh_needed` flag guards all `terminal.draw()` calls.
- When music plays, the visualizer wake channel triggers redraws at ~30 fps.
- When paused/idle, only the 250ms tick fires (for progress bar updates), and even that only sets `refresh_needed` if the player is active or visualizer bars are still decaying.
- Terminal clears are triggered only on panel, source, or track changes (and lyrics/visualizer mode switches), to prevent Sixel image ghosting.

**Terminal safety:** stderr is redirected to `mixed.log` in the cache directory before raw mode is entered (rotated at 5 MB; set `MIXED_DEBUG` to keep it on the terminal). The panic hook restores the terminal only for a panic on the main thread; a panic on a worker thread is logged and leaves the TUI running.

**Key routing (`ui/events.rs`):** source switching (`Ctrl+1..4` / `Alt+1..4`) is handled first, before the loading guard and before any text input, so the digit never reaches a search or login field. When the terminal supports it, the kitty keyboard protocol's disambiguation flag is pushed at startup so `Ctrl+digit` and `Shift+Enter` are reported.

---

## Thread 2: Audio Playback (`audio/player.rs`)

A dedicated `std::thread::spawn` that owns the **Rodio** `OutputStream`, `Sink`, and all decoder state. The main thread communicates with it via a `crossbeam_channel::bounded(100)` command channel carrying `PlayerCmd` variants.

**Key design decisions:**

- **Thread isolation:** The `OutputStream` and audio device handle never leave this thread. This prevents the ALSA/CoreAudio backend from being touched by the UI thread.
- **Priority elevation:** On Linux, the thread calls `setpriority(PRIO_PROCESS, tid, -10)` via raw `libc::syscall(SYS_gettid)` to reduce scheduling latency for the audio device.
- **Atomic state export:** Playback state (`is_playing`, `is_paused`, `is_finished`, `elapsed_ms`, `volume`) is published via `Arc<AtomicBool>` / `Arc<AtomicU64>` with `Release/Acquire` ordering. The main thread reads these without ever blocking.
- **Hybrid seek:** `try_seek()` is attempted first (native codec seek for indexed formats). On failure, the player either:
  - **Forward seek:** Atomically stores a `skip_request` sample count that `VisualizerSource` consumes by discarding samples from the decoder iterator.
  - **Backward seek:** Stops the sink, reopens the file, and positions the new decoder with its own seek, falling back to fast-forwarding via `skip_request`.

**Inputs (`PlayInput`):** the thread plays three kinds of input, all through the same Rodio sink and `VisualizerSource`, so volume and the visualizer behave identically for every source.

| Input | Decoder | Used for |
|---|---|---|
| `File(path)` | `SymphoniaSource` over a seekable file | Local files, cached YouTube audio |
| `Growing { path, progress }` | `SymphoniaSource` over `GrowingFile` | YouTube audio that is still downloading |
| `Spotify(uri)` | `PcmSource` fed by `librespot` | Spotify tracks |

**Playing a file while it downloads (`audio/growing_file.rs`):** `GrowingFile` reads the downloader's `.part` file. At the current end of the file it waits for more data instead of reporting end-of-file, until the shared `DownloadProgress` is marked finished, the reader is cancelled (track change or stop), or the file has not grown for 30 seconds. It reports itself as a forward-only stream, because a seekable source makes the MP4 reader scan the whole file before decoding. Seeking is therefore handled in `rodio_backend.rs`:

- targets are limited to the downloaded range, estimated from the bytes on disk and the total size reported by `yt-dlp`;
- a forward seek uses the decoder's forward skip, a backward seek reopens the partial file;
- once the download has finished, the next seek reopens the completed file, which seeks freely.

**Spotify playback (`audio/spotify_backend.rs`):** `SpotifyBackend` lives on this thread and owns a one-worker Tokio runtime (`mixed-spotify-rt`), the `librespot` `Session`, and the `librespot` `Player`.

- The session connects on the first Spotify load (cached `librespot` credentials first, then the Web API access token), waiting at most 15 seconds, and is rebuilt if it has dropped.
- `librespot` writes decoded samples into a bounded lock-free ring (`PcmWriter` → `PcmSource`, 44.1 kHz stereo `f32`). The sink applies backpressure by sleeping while the ring is full; the source yields silence on underrun so the Rodio sink stays alive.
- `librespot` player events are mapped back: `EndOfTrack` ends the `PcmSource` (which produces `Finished`), `Unavailable` produces `Failed`, and `Playing` / `Seeked` / `PositionCorrection` synchronise the playback clock. The clock is held at zero until the first position arrives, so elapsed time does not run ahead while a track buffers.
- `librespot`'s own volume is disabled; volume is applied on the Rodio sink.

**Sample tap pipeline:**

```
Decoder → convert_samples::<f32>() → VisualizerSource → Sink
                                          │
                                          ▼
                                   SampleRingBuffer
                                   (Arc<Mutex<...>>)
```

The `VisualizerSource` wraps the Rodio source iterator and taps mono samples (channel 0 only) into a fixed-size ring buffer. To avoid per-sample mutex contention on the audio hot-path, samples are batched locally in a stack-allocated `[f32; 64]` array and flushed in a single `try_lock()` call every 64 samples (~1.5ms at 44.1 kHz). If the lock is contended, the batch is silently dropped — the FFT thread reads at 34ms intervals so one missed flush is imperceptible.

---

## Thread 3: FFT Visualizer (`app.rs::finalize_player_init`)

A background thread spawned after the audio `Player` initializes. It runs a tight loop with a 34ms sleep cadence (~30 fps):

```rust
loop {
    sleep(34ms);

    if playing && !paused {
        // Lock ring buffer → read 2048 samples into scratch buffer
        // Process FFT (Blackman-Harris window → forward FFT → magnitude → 1/3 octave bands)
        // Apply attack/decay smoothing (kew-style ballistics)
    } else {
        // Feed silence → bars decay gracefully
    }

    // Publish bars via try_write() on Arc<RwLock<Vec<f32>>>
    // Wake main loop via try_send(()) on bounded(1) channel
}
```

**Key properties:**

- **Zero allocation per frame:** The `sample_scratch` buffer is allocated once at thread startup and reused via `read_latest_into()`.
- **Non-blocking writes:** `try_write()` on the `RwLock` and `try_send()` on the wake channel ensure this thread never stalls, even if the main thread is busy drawing.
- **Graceful decay:** When paused, silence is fed through the FFT engine so the visualizer bars smoothly decay to zero rather than freezing.

---

## Thread 4: Platform Media Integration

This thread is conditionally compiled based on the target platform. Both implementations converge on the same `MediaCommand` enum, so the main event loop contains zero platform-specific branching for command dispatch.

### Linux: MPRIS D-Bus Service (`sys/mpris.rs`)

An isolated **Tokio** `current_thread` runtime that:

1. Connects to the session D-Bus.
2. Registers `org.mpris.MediaPlayer2` and `org.mpris.MediaPlayer2.Player` interfaces via `zbus`.
3. Requests the well-known name `org.mpris.MediaPlayer2.mixed` with `ReplaceExisting | AllowReplacement` flags.
4. Runs an async select loop:
   - `update_rx.recv()` — Triggered by the main thread when state changes (debounced at 300ms).
   - `sleep(100ms)` — Periodic shutdown flag check.

**State flow:**

```
Main Thread                        MPRIS Thread
    │                                    │
    │ ── AtomicU8/Bool/U64 stores ─────► │  (playback_status, volume, position, etc.)
    │ ── RwLock<MprisMetadataStrings> ──► │  (title, artist, album, art_url, url, track_id)
    │ ── mpsc::unbounded_channel ───────► │  (update trigger — debounced)
    │                                    │
    │ ◄── crossbeam::unbounded ──────── │  (MediaCommand: PlayPause, Next, Seek, etc.)
    │                                    │
```

Each track gets its own `mpris:trackid` object path and an `xesam:url` (`file://` for local files, an `https://` link for Spotify and YouTube tracks).

All D-Bus method calls (`Play`, `Pause`, `Next`, `Seek`) are **thin routers**: they immediately enqueue a `MediaCommand` via `try_send()` and return. Zero state logic executes inside the D-Bus handler — this prevents any D-Bus client from blocking the MPRIS thread.

**Graceful shutdown:** The main thread sets `shutdown.store(true)` and drops the `mpsc` sender. The Tokio select loop detects either condition and exits, dropping the `Connection` which releases the D-Bus name immediately.

---

## Thread 5: Async Network Runtime (`sources/runtime.rs`)

A dedicated multi-threaded **Tokio** runtime that handles all asynchronous network I/O, Web API requests, and external streaming processes without ever blocking the TUI event loop or audio playback thread.

**Communication flow:**
- **Requests (`app` → `runtime`):** Dispatched via `crossbeam_channel::unbounded<SourceRequest>()`: `CheckAuth`, `Login`, `FetchRoots`, `FetchChildren`, `Search`, `FetchCover`, `DownloadYouTubeTrack`. Each request runs as its own Tokio task.
- **Events (`runtime` → `app`):** Sent back via `crossbeam_channel::unbounded<SourceEvent>()` and multiplexed in Thread 1's `select!` event loop.
- **Generational Search Cancellation:** Each search query increments a monotonic generation counter. If a user types a new character before an earlier network query responds, the stale reply is silently discarded upon arrival to avoid race conditions.

**Streaming integrations:**
- **Spotify (`sources/spotify.rs`):**
  - Web API client over `reqwest`: `/search` (10 results per type per page), `/me/tracks`, `/me/playlists`, `/me/albums`, `/playlists/{id}/items`, `/albums/{id}/tracks`. A rate-limited search (HTTP 429) is retried once after `Retry-After`.
  - PKCE login through `librespot-oauth` with redirect `http://127.0.0.1:8898/login`. The access token is refreshed at startup and before a request once it is older than 45 minutes; refreshed tokens are written back to `credentials.json`.
  - Audio does not pass through this thread; see *Spotify playback* under Thread 2.
- **YouTube Music (`sources/youtube.rs` & `sources/ytdlp.rs`):**
  - Searches and browses YouTube Music via `ytmapi-rs`, authenticated with a browser cookie or anonymously (search only).
  - Spawns background `yt-dlp` processes that download audio into `~/.cache/mixed/yt/` (`--fixup never`, so a file is never rewritten while it plays). `yt-dlp` prints the expected file size, which is stored in the download's `DownloadProgress`.
  - While `yt-dlp` runs, the partial `.part` file is polled; once it holds about 190 KB a `YouTubeTrackStreamable` event lets the app start playback from it. `YouTubeTrackDownloaded` / `YouTubeDownloadFailed` follow when the process exits. If the partial file cannot be decoded, the app waits for the completed file instead.
  - Downloads in flight are tracked by video id, so a prefetch and a play request for the same track share one process.
  - The cache is trimmed oldest-first to `yt_cache_mb` at startup and after each download. When the current track passes 50%, the next YouTube track in the queue is prefetched.
- **Cover art:** `FetchCover` downloads remote artwork into the `covers` cache folder; the cached file feeds the same image pipeline as embedded local covers.

**Remote browse state (`sources/mod.rs`):** each remote source has a `SourceView` holding its library tree (`flat`), search results, cursors and login prompt. `BrowseItem.depth` records nesting; `SourceView::expand` splices a container's children below it and `collapse` removes the whole subtree.

**Credentials (`config/credentials.rs`):** the Spotify Client ID and tokens and the YouTube cookie are stored in `credentials.json`, separate from `config.json`, written atomically with mode `0600` on Unix.

**Feature flags:** `spotify` and `youtube` are Cargo features, both enabled by default. `--no-default-features` builds a local-only player; the remote clients compile to stubs.

---

## Safety Barriers & Resource Throttling

### Visualizer Wake-Up: `bounded(1)` Throttling

The most critical safety barrier in the architecture is the `crossbeam_channel::bounded::<()>(1)` channel connecting the FFT thread to the main render loop.

**Problem it solves:**

Without throttling, the FFT thread would fire 30 wake-up signals per second. If the main thread is temporarily slow (e.g., Sixel re-encoding on resize), signals would queue up and cause a burst of redundant redraws when the main thread catches up.

**How it works:**

```rust
// FFT Thread (producer):
let _ = tx.try_send(());   // Non-blocking. If channel is full → signal dropped.

// Main Thread (consumer):
crossbeam_channel::select! {
    recv(vis_wake_rx) -> _ => {
        app.refresh_needed = true;  // Exactly one redraw per consumed signal.
    }
}
```

- `bounded(1)` means at most **one** pending wake-up signal exists at any time.
- `try_send()` is non-blocking: if the channel already has a signal queued, the new one is silently discarded.
- The main thread consumes the signal and redraws. The next signal from the FFT thread will succeed because the channel is now empty.
- **Net effect:** The main loop redraws at most once per `select!` iteration, regardless of how fast the FFT thread runs. This prevents CPU spikes from rendering storms.

### Audio Sample Batching: Mutex Contention Guard

The `VisualizerSource` uses a second throttling mechanism: sample batching.

```rust
const BATCH_SIZE: usize = 64;
batch: [f32; BATCH_SIZE],  // Stack-allocated accumulator
```

Instead of calling `try_lock()` on the shared ring buffer for every audio sample (44,100 times/sec for mono), the source accumulates 64 samples locally and flushes in one mutex acquisition. This reduces lock contention by **64×** and eliminates ALSA underrun risks caused by per-sample locking overhead.

### Atomic State: Lock-Free Cross-Thread Communication

All frequently-read player state uses `Arc<Atomic*>` with explicit memory ordering:

| Atomic | Ordering | Rationale |
|---|---|---|
| `is_playing` / `is_paused` / `is_finished` | `Release` (write) / `Acquire` (read) | Establishes happens-before on ARM (non-TSO) |
| `elapsed_ms` / `volume` | `Relaxed` | Eventual consistency is sufficient for display |
| `skip_request` | `Release` (store) / `Acquire` (swap) | Ensures sample count is fully visible before consumer reads |
| `shutdown` | `Relaxed` | Checked periodically; exact timing is not critical |
| `DownloadProgress` (`done`, `failed`, `total_bytes`) | `Release` / `Acquire` | `done` is set after the last byte is written and read before each file read, so no data is missed |

This design ensures the main thread can read player state at any time without ever blocking the audio thread.

---

## Module Map

```
src/
├── main.rs              # Thread 1: Event loop, terminal setup, select! multiplexer
├── app.rs               # Central App state, tick logic, player/source event handling, FFT thread spawn (Thread 3)
├── cli.rs               # Command-line parsing and --help text
├── audio/
│   ├── player.rs        # Thread 2: command loop, PlayInput/PlayerCmd/PlayerEvent, atomic state
│   ├── rodio_backend.rs # Rodio sink management, playback clock, hybrid seek
│   ├── symphonia_source.rs # Symphonia decoder as a Rodio source (files and growing files)
│   ├── growing_file.rs  # DownloadProgress + reader that waits for a file still being downloaded
│   ├── pcm_source.rs    # PcmWriter -> PcmSource lock-free bounded ring buffer
│   ├── spotify_backend.rs # librespot session/player, PCM sink, event mapping
│   ├── visualizer.rs    # FFT engine: Blackman-Harris window, 1/3 octave band mapping
│   └── viz_source.rs    # VisualizerSource: sample tap with batched ring buffer writes
├── sources/             # Thread 5: Multi-source async streaming subsystem
│   ├── mod.rs           # SourceTab, BrowseItem, SourceView (browse tree), Request/Event enums
│   ├── runtime.rs       # Async Tokio runtime, request handling, in-flight download tracking
│   ├── local.rs         # Local library adapter to BrowseItem
│   ├── spotify.rs       # Spotify Web API client, PKCE login, token refresh
│   ├── youtube.rs       # YouTube Music search and browse client
│   └── ytdlp.rs         # yt-dlp runner, partial-file detection, cache eviction
├── sys/
│   ├── mod.rs           # MediaCommand enum (platform-agnostic)
│   ├── mpris.rs         # Thread 4 (Linux): Tokio D-Bus MPRIS2 service
│   └── notifications.rs # Desktop notifications over D-Bus
├── ui/
│   ├── layout.rs        # Ratatui layout composition & draw()
│   ├── events.rs        # Keyboard/mouse event dispatch
│   ├── theme.rs         # Colour constants and per-source accent colours
│   ├── widgets.rs       # Shared helpers: scroll offset, scrollbar, row style, input line, truncation
│   ├── artwork.rs       # Sixel cover art protocol management
│   ├── branding.rs      # ASCII art branding & splash
│   ├── visualizer_widget.rs # Bar/braille spectrum widget
│   └── lyrics_widget.rs # Synchronized lyrics display
├── config/              # AppConfig (config.json), SessionState (state.json), Credentials (credentials.json)
├── data/                # TrackRef, Playlist (the single queue), Library, Metadata, Lyrics
└── utils/               # Stable FNV-1a hash for cache names, terminal-title sanitizer
```
