use std::path::PathBuf;
use std::sync::Arc;
use tokio::process::Command;

use crate::audio::growing_file::DownloadProgress;

/// A partial download is handed to the player once it holds this many bytes
/// (container headers plus the first few seconds of audio).
const STREAMABLE_BYTES: u64 = 192 * 1024;
/// How often the partial file is checked while yt-dlp is running.
const STREAM_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(100);

/// Wrapper around `yt-dlp` subprocess for streaming audio caching.
#[derive(Debug, Clone)]
pub struct YtDlp {
    pub custom_path: Option<String>,
}

impl YtDlp {
    pub fn new(custom_path: Option<String>) -> Self {
        Self { custom_path }
    }

    /// Check if yt-dlp is available either at custom_path or on PATH.
    pub async fn check_available(&self) -> Result<String, String> {
        let binary = self.custom_path.as_deref().unwrap_or("yt-dlp");
        let mut cmd = Command::new(binary);
        cmd.arg("--version");
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());

        match cmd.output().await {
            Ok(output) if output.status.success() => {
                let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
                Ok(version)
            }
            _ => Err("yt-dlp not found. Install it to play YouTube Music.".to_string()),
        }
    }

    /// Cache directory for downloaded YouTube audio files (~/.cache/mixed/yt).
    pub fn cache_dir() -> PathBuf {
        let proj = directories::ProjectDirs::from("", "", "mixed");
        let base = proj
            .map(|d| d.cache_dir().to_path_buf())
            .unwrap_or_else(std::env::temp_dir);
        let path = base.join("yt");
        let _ = std::fs::create_dir_all(&path);
        path
    }

    /// Find an already cached file for the given video ID.
    pub fn find_cached(video_id: &str) -> Option<PathBuf> {
        let dir = Self::cache_dir();
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    if stem == video_id {
                        return Some(path);
                    }
                }
            }
        }
        None
    }

    /// Find the partial file yt-dlp is currently writing for the given video ID
    /// (`<id>.<ext>.part`), if any.
    pub fn find_part(video_id: &str) -> Option<PathBuf> {
        let prefix = format!("{}.", video_id);
        std::fs::read_dir(Self::cache_dir())
            .ok()?
            .flatten()
            .map(|entry| entry.path())
            .find(|path| {
                path.extension().is_some_and(|ext| ext == "part")
                    && path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with(&prefix))
            })
    }

    /// Download audio stream for video_id using yt-dlp into cache.
    ///
    /// `progress` is marked finished when yt-dlp exits. `on_streamable` is called
    /// once with the partial file as soon as enough of it has arrived to start
    /// playback; it is not called when the track was already cached.
    pub async fn download_audio(
        &self,
        video_id: &str,
        format: &str,
        progress: &Arc<DownloadProgress>,
        on_streamable: impl FnOnce(PathBuf),
    ) -> Result<PathBuf, String> {
        let result = self
            .run_download(video_id, format, progress, on_streamable)
            .await;
        progress.finish(result.is_ok());
        result
    }

    async fn run_download(
        &self,
        video_id: &str,
        format: &str,
        progress: &Arc<DownloadProgress>,
        on_streamable: impl FnOnce(PathBuf),
    ) -> Result<PathBuf, String> {
        use tokio::io::{AsyncBufReadExt, AsyncReadExt};

        if let Some(cached) = Self::find_cached(video_id) {
            return Ok(cached);
        }

        self.check_available().await?;

        let cache_dir = Self::cache_dir();
        let out_template = cache_dir.join(format!("{}.%(ext)s", video_id));
        let out_str = out_template.to_string_lossy().to_string();

        let binary = self.custom_path.as_deref().unwrap_or("yt-dlp");
        let mut cmd = Command::new(binary);
        cmd.args([
            "-f",
            format,
            "--no-playlist",
            // Keep the file exactly as downloaded: the m4a fix-up would rewrite it
            // after the download, while it may already be playing.
            "--fixup",
            "never",
            // Print the expected size up front (and nothing else) but still download
            "--no-simulate",
            "--print",
            "%(filesize,filesize_approx)s",
            "-o",
            &out_str,
            "--",
            video_id,
        ]);
        cmd.stdin(std::process::Stdio::null());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());
        cmd.kill_on_drop(true);

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("Failed to spawn yt-dlp: {}", e))?;

        // stdout carries the expected file size; stderr is collected for error reporting.
        let size_task = child.stdout.take().map(|stdout| {
            let progress = progress.clone();
            tokio::spawn(async move {
                let mut lines = tokio::io::BufReader::new(stdout).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    if let Ok(bytes) = line.trim().parse::<u64>() {
                        progress.set_total_bytes(bytes);
                    }
                }
            })
        });
        let stderr_task = child.stderr.take().map(|mut stderr| {
            tokio::spawn(async move {
                let mut text = String::new();
                let _ = stderr.read_to_string(&mut text).await;
                text
            })
        });

        // While yt-dlp runs, watch the partial file and report it once it is playable.
        let mut on_streamable = Some(on_streamable);
        let status = loop {
            match tokio::time::timeout(STREAM_POLL_INTERVAL, child.wait()).await {
                Ok(status) => break status,
                Err(_) => {
                    if on_streamable.is_none() {
                        continue;
                    }
                    let Some(part) = Self::find_part(video_id) else {
                        continue;
                    };
                    let len = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
                    if len >= STREAMABLE_BYTES {
                        if let Some(notify) = on_streamable.take() {
                            notify(part);
                        }
                    }
                }
            }
        };

        if let Some(task) = size_task {
            let _ = task.await;
        }
        let stderr_text = match stderr_task {
            Some(task) => task.await.unwrap_or_default(),
            None => String::new(),
        };

        let status = status.map_err(|e| format!("Failed to run yt-dlp: {}", e))?;
        if !status.success() {
            let first_err_line = stderr_text
                .lines()
                .find(|l| l.contains("ERROR:"))
                .unwrap_or("yt-dlp download failed");
            return Err(first_err_line.to_string());
        }

        Self::find_cached(video_id)
            .ok_or_else(|| "Audio file downloaded but not found in cache".to_string())
    }

    /// Evict oldest cache files until total cache size is under max_mb.
    pub fn evict_cache(max_mb: usize) {
        let cache_dir = Self::cache_dir();
        let max_bytes = (max_mb as u64) * 1024 * 1024;
        let Ok(entries) = std::fs::read_dir(&cache_dir) else {
            return;
        };

        let mut files = Vec::new();
        let mut total_size = 0u64;

        for entry in entries.flatten() {
            if let Ok(meta) = entry.metadata() {
                if meta.is_file() {
                    let size = meta.len();
                    total_size += size;
                    let mtime = meta.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                    files.push((entry.path(), size, mtime));
                }
            }
        }

        if total_size <= max_bytes {
            return;
        }

        // Sort oldest first
        files.sort_by_key(|f| f.2);

        for (path, size, _) in files {
            if total_size <= max_bytes {
                break;
            }
            if std::fs::remove_file(&path).is_ok() {
                total_size = total_size.saturating_sub(size);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ytdlp_cache_dir_creation() {
        let dir = YtDlp::cache_dir();
        assert!(dir.exists());
    }

    #[test]
    fn test_ytdlp_cache_eviction_no_panic() {
        YtDlp::evict_cache(512);
    }
}
