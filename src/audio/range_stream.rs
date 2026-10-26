//! Playing a YouTube audio URL directly, the way YouTube Music apps do: the file is
//! fetched in pieces with HTTP range requests, starting where playback is. A seek
//! moves the fetching to the new position instead of waiting for everything before
//! it. Once every piece is in, the file is saved to the cache for the next play.

use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use symphonia::core::io::MediaSource;

/// Size of one range request. Small enough that the first sound comes quickly.
const CHUNK: u64 = 512 * 1024;
/// How often a reader waiting for a piece checks for it.
const POLL_INTERVAL: Duration = Duration::from_millis(20);
/// Give up when a needed piece has not arrived for this long.
const STALL_TIMEOUT: Duration = Duration::from_secs(30);
/// Attempts per piece before the stream fails.
const ATTEMPTS: u32 = 3;

struct Shared {
    url: String,
    len: u64,
    chunks: Mutex<Vec<Option<Arc<Vec<u8>>>>>,
    /// The piece the reader needs next; fetching continues from there.
    wanted: AtomicU64,
    failed: AtomicBool,
    /// Ends the fetching (another track plays).
    stop: AtomicBool,
}

impl Shared {
    fn chunk(&self, index: u64) -> Option<Arc<Vec<u8>>> {
        self.chunks
            .lock()
            .ok()?
            .get(index as usize)
            .cloned()
            .flatten()
    }

    /// The next piece to fetch: the first missing one from the reader's position,
    /// else the first missing one at all. `None` when all are in.
    fn next_missing(&self) -> Option<u64> {
        let chunks = self.chunks.lock().ok()?;
        let wanted = self.wanted.load(Ordering::Acquire) as usize;
        let missing = |i: &usize| chunks[*i].is_none();
        (wanted..chunks.len())
            .find(missing)
            .or_else(|| (0..wanted.min(chunks.len())).find(missing))
            .map(|i| i as u64)
    }
}

/// One audio URL being fetched. Each decoder reads it through its own `RangeStream`.
#[derive(Clone)]
pub struct StreamHandle {
    shared: Arc<Shared>,
}

/// A reader of a `StreamHandle`, used like a file. Reads wait while the piece they
/// need is fetched; setting `cancel` releases a waiting read.
pub struct RangeStream {
    shared: Arc<Shared>,
    pos: u64,
    cancel: Arc<AtomicBool>,
}

/// Fetch bytes `start..=end` of `url`, and the file's total length.
async fn fetch(
    client: &reqwest::Client,
    url: &str,
    start: u64,
    end: u64,
) -> Result<(Vec<u8>, Option<u64>), String> {
    let response = client
        .get(url)
        .header(reqwest::header::RANGE, format!("bytes={}-{}", start, end))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!("HTTP {}", response.status()));
    }
    // "bytes 0-524287/2559471"
    let total = response
        .headers()
        .get(reqwest::header::CONTENT_RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.rsplit('/').next())
        .and_then(|v| v.parse().ok());
    let bytes = response.bytes().await.map_err(|e| e.to_string())?;
    Ok((bytes.to_vec(), total))
}

async fn fetch_with_retries(
    client: &reqwest::Client,
    url: &str,
    start: u64,
    end: u64,
) -> Result<(Vec<u8>, Option<u64>), String> {
    let mut last = String::new();
    for attempt in 0..ATTEMPTS {
        match fetch(client, url, start, end).await {
            Ok(answer) => return Ok(answer),
            Err(e) => last = e,
        }
        tokio::time::sleep(Duration::from_millis(300 * u64::from(attempt + 1))).await;
    }
    Err(last)
}

impl StreamHandle {
    /// Start fetching `url`. Blocks until the first piece is in, which also tells the
    /// length. The whole file is saved as `save_to` once fetched.
    pub fn open(url: &str, save_to: PathBuf) -> Result<Self, String> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?;
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .build()
            .map_err(|e| e.to_string())?;
        let (first, total) = rt.block_on(fetch_with_retries(&client, url, 0, CHUNK - 1))?;
        let len = total.unwrap_or(first.len() as u64);
        if len == 0 {
            return Err("The stream is empty".to_string());
        }
        let mut chunks = vec![None; len.div_ceil(CHUNK) as usize];
        chunks[0] = Some(Arc::new(first));
        let shared = Arc::new(Shared {
            url: url.to_string(),
            len,
            chunks: Mutex::new(chunks),
            wanted: AtomicU64::new(0),
            failed: AtomicBool::new(false),
            stop: AtomicBool::new(false),
        });

        let fetcher = shared.clone();
        std::thread::Builder::new()
            .name("mixed-stream".into())
            .spawn(move || rt.block_on(fetch_all(fetcher, client, save_to)))
            .map_err(|e| e.to_string())?;
        Ok(Self { shared })
    }

    /// A reader from the start of the stream.
    pub fn reader(&self, cancel: Arc<AtomicBool>) -> RangeStream {
        RangeStream {
            shared: self.shared.clone(),
            pos: 0,
            cancel,
        }
    }

    /// Stop fetching; waiting reads end.
    pub fn stop(&self) {
        self.shared.stop.store(true, Ordering::Release);
    }
}

/// Fetch the pieces the reader needs first, then the rest; save the file when done.
async fn fetch_all(shared: Arc<Shared>, client: reqwest::Client, save_to: PathBuf) {
    while !shared.stop.load(Ordering::Acquire) {
        let Some(index) = shared.next_missing() else {
            save(&shared, &save_to);
            return;
        };
        let start = index * CHUNK;
        let end = (start + CHUNK).min(shared.len) - 1;
        match fetch_with_retries(&client, &shared.url, start, end).await {
            Ok((bytes, _)) => {
                if let Ok(mut chunks) = shared.chunks.lock() {
                    chunks[index as usize] = Some(Arc::new(bytes));
                }
            }
            Err(e) => {
                log::warn!("YouTube stream stopped at byte {}: {}", start, e);
                shared.failed.store(true, Ordering::Release);
                return;
            }
        }
    }
}

/// Write the complete file next to its final name, then move it there, so a cache
/// lookup never finds half a file.
fn save(shared: &Shared, save_to: &std::path::Path) {
    let Ok(chunks) = shared.chunks.lock() else {
        return;
    };
    let mut data = Vec::with_capacity(shared.len as usize);
    for chunk in chunks.iter() {
        match chunk {
            Some(bytes) => data.extend_from_slice(bytes),
            None => return,
        }
    }
    drop(chunks);
    let partial = save_to.with_extension("stream");
    if std::fs::write(&partial, &data).is_ok() && std::fs::rename(&partial, save_to).is_ok() {
        log::info!("Saved streamed audio to {}", save_to.display());
    } else {
        let _ = std::fs::remove_file(&partial);
    }
}

impl Read for RangeStream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.pos >= self.shared.len || buf.is_empty() {
            return Ok(0);
        }
        let index = self.pos / CHUNK;
        self.shared.wanted.store(index, Ordering::Release);
        let mut waited = Duration::ZERO;
        let chunk = loop {
            if self.cancel.load(Ordering::Acquire) || self.shared.stop.load(Ordering::Acquire) {
                return Ok(0);
            }
            if let Some(chunk) = self.shared.chunk(index) {
                break chunk;
            }
            if self.shared.failed.load(Ordering::Acquire) || waited >= STALL_TIMEOUT {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "the YouTube stream stopped",
                ));
            }
            std::thread::sleep(POLL_INTERVAL);
            waited += POLL_INTERVAL;
        };
        let offset = (self.pos - index * CHUNK) as usize;
        let n = buf.len().min(chunk.len().saturating_sub(offset));
        buf[..n].copy_from_slice(&chunk[offset..offset + n]);
        self.pos += n as u64;
        Ok(n)
    }
}

impl Seek for RangeStream {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        let target = match pos {
            SeekFrom::Start(p) => Some(p),
            SeekFrom::End(d) => self.shared.len.checked_add_signed(d),
            SeekFrom::Current(d) => self.pos.checked_add_signed(d),
        };
        let target = target.ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "seek before the start")
        })?;
        self.pos = target;
        Ok(target)
    }
}

impl MediaSource for RangeStream {
    fn is_seekable(&self) -> bool {
        true
    }

    fn byte_len(&self) -> Option<u64> {
        Some(self.shared.len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shared(len: u64, have: &[usize]) -> Shared {
        let mut chunks = vec![None; len.div_ceil(CHUNK) as usize];
        for &i in have {
            chunks[i] = Some(Arc::new(vec![i as u8; CHUNK as usize]));
        }
        Shared {
            url: String::new(),
            len,
            chunks: Mutex::new(chunks),
            wanted: AtomicU64::new(0),
            failed: AtomicBool::new(false),
            stop: AtomicBool::new(false),
        }
    }

    #[test]
    fn pieces_are_fetched_from_where_playback_is_then_the_rest() {
        let s = shared(5 * CHUNK, &[0, 3]);
        assert_eq!(s.next_missing(), Some(1));
        // After a seek to piece 3: piece 4 next, then the gap before it
        s.wanted.store(3, Ordering::Release);
        assert_eq!(s.next_missing(), Some(4));
        let s = shared(5 * CHUNK, &[0, 3, 4]);
        s.wanted.store(3, Ordering::Release);
        assert_eq!(s.next_missing(), Some(1));
        let s = shared(2 * CHUNK, &[0, 1]);
        assert_eq!(s.next_missing(), None);
    }

    #[test]
    fn reads_and_seeks_go_through_the_pieces() {
        let handle = StreamHandle {
            shared: Arc::new(shared(2 * CHUNK, &[0, 1])),
        };
        let mut stream = handle.reader(Arc::default());
        let mut buf = [0u8; 4];
        stream.seek(SeekFrom::Start(CHUNK - 2)).unwrap();
        // A read stops at the end of its piece
        assert_eq!(stream.read(&mut buf).unwrap(), 2);
        assert_eq!(&buf[..2], &[0, 0]);
        assert_eq!(stream.read(&mut buf).unwrap(), 4);
        assert_eq!(buf, [1, 1, 1, 1]);
        assert_eq!(stream.seek(SeekFrom::End(0)).unwrap(), 2 * CHUNK);
        assert_eq!(stream.read(&mut buf).unwrap(), 0);
    }
}
