use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

use crate::data::playlist::{PlaylistEntry, RepeatMode};

/// Persistent session state saved on exit, restored on launch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionState {
    /// Full playlist entries with metadata and source-neutral track IDs.
    #[serde(default)]
    pub queue: Vec<PlaylistEntry>,
    /// List of file paths in the queue (legacy format, preserved for backwards compatibility).
    #[serde(default)]
    pub playlist_paths: Vec<PathBuf>,
    /// Index of the currently playing track.
    #[serde(default)]
    pub current_index: usize,
    /// Playback position in milliseconds.
    #[serde(default)]
    pub position_ms: u64,
    /// Whether playback was active when the session was saved.
    #[serde(default)]
    pub was_playing: bool,
    /// Volume at save time.
    #[serde(default = "default_volume")]
    pub volume: u8,
    /// Repeat mode.
    #[serde(default)]
    pub repeat_mode: RepeatMode,
    /// Shuffle state.
    #[serde(default)]
    pub shuffle: bool,
}

fn default_volume() -> u8 {
    80
}

impl Default for SessionState {
    fn default() -> Self {
        Self {
            queue: Vec::new(),
            playlist_paths: Vec::new(),
            current_index: 0,
            position_ms: 0,
            was_playing: false,
            volume: 80,
            repeat_mode: RepeatMode::Off,
            shuffle: false,
        }
    }
}

/// Returns the path to the session state file using XDG/platform-appropriate data directory.
pub fn session_file_path() -> Option<PathBuf> {
    let proj = directories::ProjectDirs::from("", "", "mixed")?;
    let state_dir = proj.data_local_dir().to_path_buf();
    fs::create_dir_all(&state_dir).ok()?;
    Some(state_dir.join("state.json"))
}

/// Save session state to disk.
pub fn save_session(state: &SessionState) {
    if let Some(path) = session_file_path() {
        if let Ok(json) = serde_json::to_string_pretty(state) {
            let _ = fs::write(path, json);
        }
    }
}

/// Load session state from disk.
pub fn load_session() -> Option<SessionState> {
    let path = session_file_path()?;
    if !path.exists() {
        return None;
    }
    let data = fs::read_to_string(path).ok()?;
    serde_json::from_str(&data).ok()
}
