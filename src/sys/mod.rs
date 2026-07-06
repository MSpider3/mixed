/// Platform-agnostic media commands sent from OS media controls → App event loop.
///
/// Both the Linux MPRIS/D-Bus backend and the Android termux-api backend
/// map their respective OS signals into this enum, so `app.rs` and `main.rs`
/// contain zero inline `#[cfg]` blocks for command dispatch.
#[derive(Debug, Clone)]
pub enum MediaCommand {
    PlayPause,
    Play,
    Pause,
    Stop,
    Next,
    Previous,
    /// Relative seek offset in microseconds (may be negative).
    Seek(i64),
    /// Absolute seek position in microseconds.
    SetPosition(i64),
    /// Loop status string: "None" | "Track" | "Playlist"
    SetLoopStatus(String),
    SetShuffle(bool),
    /// Volume in [0.0, 1.0] per MPRIS/media spec.
    SetVolume(f64),
    Quit,
}

/// Linux: Full MPRIS2 D-Bus service (playerctl, GNOME media controls, etc.)
#[cfg(target_os = "linux")]
pub mod mpris;

/// Android: termux-notification lock-screen widget + Unix domain socket IPC.
#[cfg(target_os = "android")]
pub mod android_media;
