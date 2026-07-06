# Architecture Overview

> **Audience:** Contributors and maintainers looking to understand `mixed`'s runtime threading model before making code changes.

---

## Thread Model

`mixed` runs four concurrent execution contexts that communicate exclusively through lock-free atomic state and bounded crossbeam channels. There are zero shared mutexes on any performance-critical path.

```
┌─────────────────────────────────────────────────────────────────────┐
│                         MAIN PROCESS                                │
│                                                                     │
│  ┌──────────────┐   ┌──────────────┐   ┌─────────────────────────┐ │
│  │  Thread 1:   │   │  Thread 2:   │   │  Thread 3:              │ │
│  │  TUI Render  │   │  Audio       │   │  FFT Visualizer         │ │
│  │  Loop        │   │  Playback    │   │                         │ │
│  │              │   │              │   │  34ms cadence            │ │
│  │  select! {   │   │  Rodio Sink  │   │  VisualizerEngine       │ │
│  │    event_rx  │◄──│  Decoder     │──►│  SampleRingBuffer       │ │
│  │    tick_rx   │   │  Priority -10│   │  bounded(1) wake-up     │ │
│  │    vis_wake  │   │              │   │                         │ │
│  │    media_cmd │   └──────────────┘   └─────────────────────────┘ │
│  │    lib_rx    │                                                   │
│  │    player_rx │   ┌──────────────────────────────────────────┐   │
│  │  }           │   │  Thread 4: Platform Media Integration    │   │
│  │              │◄──│                                          │   │
│  │  layout::    │   │  Linux:   Tokio async D-Bus/MPRIS        │   │
│  │    draw()    │   │  Android: UDS listener (mixed.sock)      │   │
│  └──────────────┘   └──────────────────────────────────────────┘   │
│                                                                     │
└─────────────────────────────────────────────────────────────────────┘
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
| `media_cmd_rx` | `MediaCommand` | MPRIS D-Bus / Android UDS |
| `lib_rx` | `Vec<LibraryEntry>` | Background library scanner |
| `player_rx` | `Player` | One-shot player initialization |

**Rendering strategy:**

- `refresh_needed` flag guards all `terminal.draw()` calls.
- When music plays, the visualizer wake channel triggers redraws at ~30 fps.
- When paused/idle, only the 250ms tick fires (for progress bar updates), and even that only sets `refresh_needed` if the player is active or visualizer bars are still decaying.
- Terminal clears are triggered only on panel switches or track changes, to prevent Sixel image ghosting.

---

## Thread 2: Audio Playback (`audio/player.rs`)

A dedicated `std::thread::spawn` that owns the **Rodio** `OutputStream`, `Sink`, and all decoder state. The main thread communicates with it via a `crossbeam_channel::bounded(100)` command channel carrying `PlayerCmd` variants.

**Key design decisions:**

- **Thread isolation:** The `OutputStream` and audio device handle never leave this thread. This prevents the ALSA/CoreAudio backend from being touched by the UI thread.
- **Priority elevation:** On Linux, the thread calls `setpriority(PRIO_PROCESS, tid, -10)` via raw `libc::syscall(SYS_gettid)` to reduce scheduling latency for the audio device.
- **Atomic state export:** Playback state (`is_playing`, `is_paused`, `is_finished`, `elapsed_ms`, `volume`) is published via `Arc<AtomicBool>` / `Arc<AtomicU64>` with `Release/Acquire` ordering. The main thread reads these without ever blocking.
- **Hybrid seek:** `try_seek()` is attempted first (native codec seek for indexed formats). On failure, the player either:
  - **Forward seek:** Atomically stores a `skip_request` sample count that `VisualizerSource` consumes by discarding samples from the decoder iterator.
  - **Backward seek:** Stops the sink, reopens the file, creates a new decoder, and fast-forwards via `skip_request`.

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
    │ ── RwLock<MprisMetadataStrings> ──► │  (title, artist, album, art_url)
    │ ── mpsc::unbounded_channel ───────► │  (update trigger — debounced)
    │                                    │
    │ ◄── crossbeam::unbounded ──────── │  (MediaCommand: PlayPause, Next, Seek, etc.)
    │                                    │
```

All D-Bus method calls (`Play`, `Pause`, `Next`, `Seek`) are **thin routers**: they immediately enqueue a `MediaCommand` via `try_send()` and return. Zero state logic executes inside the D-Bus handler — this prevents any D-Bus client from blocking the MPRIS thread.

**Graceful shutdown:** The main thread sets `shutdown.store(true)` and drops the `mpsc` sender. The Tokio select loop detects either condition and exits, dropping the `Connection` which releases the D-Bus name immediately.

### Android: Termux UDS Bridge (`sys/android_media.rs`)

A `std::thread::spawn` that:

1. Cleans up any stale `$PREFIX/tmp/mixed.sock` from previous crashes.
2. Binds a `UnixListener` in non-blocking mode.
3. Polls for connections every 50ms, checking the `shutdown` `AtomicBool` on each iteration.
4. Reads newline-delimited commands (`playpause`, `next`, `prev`, `quit`) from each client connection.
5. Maps commands to `MediaCommand` variants and sends them on the shared crossbeam channel.

**Notification integration:** The `AndroidMediaHandle::push_metadata()` method spawns a detached `termux-notification` process with `--type media` and button actions that `echo` commands into the socket file:

```
Button ⏮  →  echo prev > '$PREFIX/tmp/mixed.sock'
Button ⏯  →  echo playpause > '$PREFIX/tmp/mixed.sock'
Button ⏭  →  echo next > '$PREFIX/tmp/mixed.sock'
```

This gives Android 13+ users native lock-screen media controls without requiring root access or a dedicated Android app.

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

This design ensures the main thread can read player state at any time without ever blocking the audio thread.

---

## Module Map

```
src/
├── main.rs              # Thread 1: Event loop, terminal setup, select! multiplexer
├── app.rs               # Central App state, tick logic, FFT thread spawn (Thread 3)
├── audio/
│   ├── player.rs        # Thread 2: Audio playback, Rodio sink management, hybrid seek
│   ├── visualizer.rs    # FFT engine: Blackman-Harris window, 1/3 octave band mapping
│   └── viz_source.rs    # VisualizerSource: sample tap with batched ring buffer writes
├── sys/
│   ├── mod.rs           # MediaCommand enum (platform-agnostic)
│   ├── mpris.rs         # Thread 4 (Linux): Tokio D-Bus MPRIS2 service
│   └── android_media.rs # Thread 4 (Android): Termux UDS notification bridge
├── ui/
│   ├── layout.rs        # Ratatui layout composition & draw()
│   ├── events.rs        # Keyboard/mouse event dispatch
│   ├── artwork.rs       # Sixel cover art protocol management
│   ├── branding.rs      # ASCII art branding & splash
│   ├── visualizer_widget.rs  # Bar/braille spectrum widget
│   └── lyrics_widget.rs # Synchronized lyrics display
├── config/              # AppConfig (TOML), SessionState (JSON)
├── data/                # Library tree, Playlist, Metadata, Lyrics
└── utils/               # Sanitizer, helpers
```
