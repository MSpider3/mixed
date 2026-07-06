use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

/// Application configuration persisted to disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    /// Path to the music library directory.
    pub music_dir: Option<String>,
    /// Default volume (0-100).
    pub volume: u8,
    /// Whether the visualizer is enabled by default.
    pub visualizer_enabled: bool,
    /// Visualizer height in rows.
    pub visualizer_height: u16,
    /// Whether cover art display is enabled.
    pub cover_enabled: bool,
    /// Color scheme index.
    pub color_scheme: usize,
    /// Whether to strip track numbers from display titles.
    pub strip_track_numbers: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            music_dir: None,
            volume: 80,
            visualizer_enabled: true,
            visualizer_height: 5,
            cover_enabled: true,
            color_scheme: 0,
            strip_track_numbers: true,
        }
    }
}

/// Returns the path to the config file.
pub fn config_path() -> PathBuf {
    let is_test = std::env::current_exe()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .map(|name| name.contains("test") || name.contains("mpris"))
        .unwrap_or(false);

    if is_test {
        let mut path = std::env::temp_dir();
        path.push("mixed_test_config.json");
        path
    } else if let Some(dir) = directories::ProjectDirs::from("", "", "mixed") {
        let mut path = dir.config_dir().to_path_buf();
        let _ = fs::create_dir_all(&path);
        path.push("config.json");
        path
    } else {
        PathBuf::from("/tmp/mixed_config.json")
    }
}

impl AppConfig {
    /// Load config from disk or create default.
    pub fn load() -> Self {
        let path = config_path();
        if path.exists() {
            if let Ok(data) = fs::read_to_string(&path) {
                if let Ok(config) = serde_json::from_str(&data) {
                    return config;
                }
            }
        }
        Self::default()
    }

    /// Save config to disk.
    pub fn save(&self) {
        let path = config_path();
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = fs::write(path, json);
        }
    }
}
