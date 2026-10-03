//! Reading an audio file while it is still being downloaded.
//!
//! The downloader shares a [`DownloadProgress`] with the reader. [`GrowingFile`]
//! behaves like a normal file, except that reaching the current end of the file
//! waits for more data instead of reporting end-of-file, until the download is
//! marked finished (or the reader is cancelled).

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use symphonia::core::io::MediaSource;

/// How often a reader waiting at the end of the file checks for new data.
const POLL_INTERVAL: Duration = Duration::from_millis(20);
/// Give up waiting when the file has not grown for this long.
const STALL_TIMEOUT: Duration = Duration::from_secs(30);

/// State of one download, shared between the downloader and the audio reader.
#[derive(Debug, Default)]
pub struct DownloadProgress {
    done: AtomicBool,
    failed: AtomicBool,
    /// Expected size of the finished file in bytes; 0 while unknown.
    total_bytes: AtomicU64,
}

impl DownloadProgress {
    /// True once the downloader has stopped writing, successfully or not.
    pub fn is_done(&self) -> bool {
        self.done.load(Ordering::Acquire)
    }

    pub fn has_failed(&self) -> bool {
        self.failed.load(Ordering::Acquire)
    }

    /// Mark the download as finished. Must be called after the last byte is written.
    pub fn finish(&self, success: bool) {
        self.failed.store(!success, Ordering::Release);
        self.done.store(true, Ordering::Release);
    }

    pub fn set_total_bytes(&self, bytes: u64) {
        self.total_bytes.store(bytes, Ordering::Release);
    }

    pub fn total_bytes(&self) -> Option<u64> {
        match self.total_bytes.load(Ordering::Acquire) {
            0 => None,
            n => Some(n),
        }
    }
}

/// A file that is still being appended to by a downloader.
pub struct GrowingFile {
    file: File,
    progress: Arc<DownloadProgress>,
    cancel: Arc<AtomicBool>,
}

impl GrowingFile {
    /// Open `path` for reading. Setting `cancel` makes a waiting read return
    /// end-of-file immediately, so a blocked audio thread can be released.
    pub fn open(
        path: &Path,
        progress: Arc<DownloadProgress>,
        cancel: Arc<AtomicBool>,
    ) -> std::io::Result<Self> {
        Ok(Self {
            file: File::open(path)?,
            progress,
            cancel,
        })
    }
}

impl Read for GrowingFile {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let mut waited = Duration::ZERO;
        loop {
            if self.cancel.load(Ordering::Acquire) {
                return Ok(0);
            }
            // Sample `done` before reading: anything written before the download
            // finished is then guaranteed to be seen by this read.
            let done = self.progress.is_done();
            let n = self.file.read(buf)?;
            if n > 0 || buf.is_empty() {
                return Ok(n);
            }
            if done || waited >= STALL_TIMEOUT {
                return Ok(0);
            }
            std::thread::sleep(POLL_INTERVAL);
            waited += POLL_INTERVAL;
        }
    }
}

impl Seek for GrowingFile {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        match pos {
            // The final length is not known while the file is still growing.
            SeekFrom::End(_) if !self.progress.is_done() => Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "cannot seek from the end of a file that is still downloading",
            )),
            other => self.file.seek(other),
        }
    }
}

impl MediaSource for GrowingFile {
    /// Reported as a forward-only stream: a seekable source makes container
    /// readers scan the whole file up front, which would wait for the entire
    /// download before the first sample is decoded.
    fn is_seekable(&self) -> bool {
        false
    }

    fn byte_len(&self) -> Option<u64> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("mixed_growing_{}_{}", std::process::id(), name))
    }

    #[test]
    fn read_waits_for_data_written_later() {
        let path = temp_path("waits");
        std::fs::write(&path, b"abc").unwrap();
        let progress = Arc::new(DownloadProgress::default());

        let writer_path = path.clone();
        let writer_progress = progress.clone();
        let writer = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            let mut f = std::fs::OpenOptions::new()
                .append(true)
                .open(&writer_path)
                .unwrap();
            f.write_all(b"def").unwrap();
            f.flush().unwrap();
            writer_progress.finish(true);
        });

        let mut reader =
            GrowingFile::open(&path, progress, Arc::new(AtomicBool::new(false))).unwrap();
        let mut out = Vec::new();
        // read_to_end only returns once the download is marked finished
        reader.read_to_end(&mut out).unwrap();
        writer.join().unwrap();
        assert_eq!(out, b"abcdef");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn cancel_releases_a_waiting_read() {
        let path = temp_path("cancel");
        std::fs::write(&path, b"").unwrap();
        let progress = Arc::new(DownloadProgress::default());
        let cancel = Arc::new(AtomicBool::new(false));

        let canceller = cancel.clone();
        let handle = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            canceller.store(true, Ordering::Release);
        });

        let mut reader = GrowingFile::open(&path, progress, cancel).unwrap();
        let mut buf = [0u8; 16];
        let started = std::time::Instant::now();
        assert_eq!(reader.read(&mut buf).unwrap(), 0);
        assert!(started.elapsed() < Duration::from_secs(5));
        handle.join().unwrap();
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn finished_download_reads_like_a_plain_file() {
        let path = temp_path("finished");
        std::fs::write(&path, b"xyz").unwrap();
        let progress = Arc::new(DownloadProgress::default());
        progress.set_total_bytes(3);
        progress.finish(true);
        assert_eq!(progress.total_bytes(), Some(3));
        assert!(!progress.has_failed());

        let mut reader =
            GrowingFile::open(&path, progress, Arc::new(AtomicBool::new(false))).unwrap();
        let mut out = Vec::new();
        reader.read_to_end(&mut out).unwrap();
        assert_eq!(out, b"xyz");
        assert_eq!(reader.seek(SeekFrom::End(0)).unwrap(), 3);
        let _ = std::fs::remove_file(&path);
    }
}
