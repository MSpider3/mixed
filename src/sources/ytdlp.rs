use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::process::Command;

use crate::audio::growing_file::DownloadProgress;

/// A partial download is handed to the player once it holds this many bytes
/// (container headers plus the first few seconds of audio).
const STREAMABLE_BYTES: u64 = 192 * 1024;
/// How often the partial file is checked while yt-dlp is running.
const STREAM_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(100);

/// What yt-dlp prints when YouTube takes the download for a bot's.
const SIGN_IN_PROMPT: &str = "Sign in to confirm";

/// Wrapper around `yt-dlp` subprocess for streaming audio caching.
#[derive(Debug, Clone)]
pub struct YtDlp {
    pub custom_path: Option<String>,
    /// Arguments from the user's configuration, added to every download.
    extra_args: Vec<String>,
    /// Set once YouTube has asked to sign in: from then on downloads carry the
    /// user's cookie. Until then it stays out of yt-dlp's hands.
    sign_in_needed: Arc<AtomicBool>,
}

/// The user's YouTube cookie (a `Cookie` header value) as a Netscape cookie file,
/// the format yt-dlp reads.
fn netscape_cookies(header: &str, expires: u64) -> String {
    let header = header.trim();
    let header = header
        .get(..7)
        .filter(|prefix| prefix.eq_ignore_ascii_case("cookie:"))
        .map_or(header, |_| &header[7..]);
    let mut file = String::from("# Netscape HTTP Cookie File\n");
    for pair in header.split(';') {
        if let Some((name, value)) = pair.trim().split_once('=') {
            file.push_str(&format!(
                ".youtube.com\tTRUE\t/\tTRUE\t{}\t{}\t{}\n",
                expires, name, value
            ));
        }
    }
    file
}

/// Cookie file for yt-dlp made from the user's YouTube cookie, if they set one.
/// yt-dlp keeps the file up to date itself, so it is named after the cookie and
/// only written again when the user signs in with another one.
fn cookie_file() -> Option<PathBuf> {
    let cookie = crate::config::credentials::Credentials::load()
        .youtube_cookie
        .filter(|c| !c.trim().is_empty())?;
    let dir = crate::config::credentials::credentials_path()
        .parent()?
        .to_path_buf();
    let name = format!("yt_cookies_{}.txt", crate::utils::hash::fnv1a_hex(&cookie));
    let path = dir.join(&name);
    if path.exists() {
        return Some(path);
    }

    // Files of earlier cookies are of no use any more
    for entry in std::fs::read_dir(&dir).ok()?.flatten() {
        let old = entry.file_name();
        let old = old.to_string_lossy();
        if old.starts_with("yt_cookies_") && old.ends_with(".txt") {
            let _ = std::fs::remove_file(entry.path());
        }
    }

    let year = 365 * 24 * 60 * 60;
    let expires = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
        + year;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&path).ok()?;
    std::io::Write::write_all(&mut file, netscape_cookies(&cookie, expires).as_bytes()).ok()?;
    Some(path)
}

/// Turn yt-dlp's error output into a message that says what to do about it.
fn explain_failure(stderr: &str, signed_in: bool) -> String {
    if stderr.contains(SIGN_IN_PROMPT) {
        return if signed_in {
            "YouTube rejected the saved cookie: sign in again with a fresh one"
        } else {
            "YouTube asks to sign in: add your cookie in the YouTube Library tab"
        }
        .to_string();
    }
    if stderr.contains("challenge solving failed") || stderr.contains("Signature solving failed") {
        return "yt-dlp needs a JavaScript runtime for YouTube: see docs/youtube.md".to_string();
    }
    stderr
        .lines()
        .find(|l| l.contains("ERROR:"))
        .unwrap_or("yt-dlp download failed")
        .to_string()
}

impl YtDlp {
    pub fn new(custom_path: Option<String>, extra_args: Vec<String>) -> Self {
        Self {
            custom_path,
            extra_args,
            sign_in_needed: Arc::default(),
        }
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
        if let Some(cached) = Self::find_cached(video_id) {
            return Ok(cached);
        }

        self.check_available().await?;

        let cookies = cookie_file();
        let mut on_streamable = Some(on_streamable);
        let mut signed_in = self.sign_in_needed.load(Ordering::Relaxed);
        loop {
            let jar = cookies.as_deref().filter(|_| signed_in);
            let Some(stderr) = self
                .run_once(video_id, format, progress, &mut on_streamable, jar)
                .await?
            else {
                break;
            };
            // YouTube asks suspected bots to sign in: try again as the user
            if stderr.contains(SIGN_IN_PROMPT) && !signed_in && cookies.is_some() {
                self.sign_in_needed.store(true, Ordering::Relaxed);
                signed_in = true;
                continue;
            }
            log::warn!("yt-dlp failed for {}: {}", video_id, stderr.trim());
            return Err(explain_failure(&stderr, signed_in));
        }

        Self::find_cached(video_id)
            .ok_or_else(|| "Audio file downloaded but not found in cache".to_string())
    }

    /// Run yt-dlp once. `Ok(None)` if the download succeeded, otherwise what
    /// yt-dlp printed about the failure.
    async fn run_once(
        &self,
        video_id: &str,
        format: &str,
        progress: &Arc<DownloadProgress>,
        on_streamable: &mut Option<impl FnOnce(PathBuf)>,
        cookies: Option<&std::path::Path>,
    ) -> Result<Option<String>, String> {
        use tokio::io::{AsyncBufReadExt, AsyncReadExt};

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
        ]);
        if let Some(cookies) = cookies {
            cmd.arg("--cookies").arg(cookies);
        }
        cmd.args(&self.extra_args);
        cmd.args(["--", video_id]);
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
        Ok((!status.success()).then_some(stderr_text))
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
    fn a_cookie_header_becomes_a_netscape_cookie_file() {
        let file = netscape_cookies("Cookie: SID=abc; __Secure-1PSID=d=e ;", 1_900_000_000);
        assert_eq!(
            file,
            "# Netscape HTTP Cookie File\n\
             .youtube.com\tTRUE\t/\tTRUE\t1900000000\tSID\tabc\n\
             .youtube.com\tTRUE\t/\tTRUE\t1900000000\t__Secure-1PSID\td=e\n"
        );
        // Without the header name, as copied from the browser's cookie field
        assert_eq!(
            netscape_cookies("SID=abc", 1),
            netscape_cookies("cookie:SID=abc", 1)
        );
    }

    #[test]
    fn yt_dlp_failures_say_what_to_do() {
        let bot = "WARNING: [youtube] No supported JavaScript runtime could be found.\n\
                   ERROR: [youtube] x: Sign in to confirm you\u{2019}re not a bot.";
        assert!(explain_failure(bot, false).contains("add your cookie"));
        assert!(explain_failure(bot, true).contains("fresh one"));

        let challenge =
            "WARNING: [youtube] x: n challenge solving failed: Some formats may be missing.\n\
                         ERROR: [youtube] x: The page needs to be reloaded.";
        assert!(explain_failure(challenge, true).contains("JavaScript runtime"));

        // The warning about a missing runtime alone does not explain another error
        let other = "WARNING: [youtube] No supported JavaScript runtime could be found.\n\
                     ERROR: [youtube] x: Video unavailable";
        assert_eq!(
            explain_failure(other, false),
            "ERROR: [youtube] x: Video unavailable"
        );
        assert_eq!(explain_failure("", false), "yt-dlp download failed");
    }

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
