use lofty::{
    config::ParseOptions,
    file::{AudioFile, TaggedFileExt},
    probe::Probe,
    tag::{Accessor, ItemKey},
};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::Duration;

/// A single line in timed (LRC) lyrics with its timestamp.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LrcLine {
    pub time_secs: f64,
    pub text: String,
}

/// How lyrics are stored.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub enum LyricsKind {
    /// Timestamped lines from embedded tags or .lrc file.
    Timed(Vec<LrcLine>),
    /// Plain text lyrics without timestamps.
    Untimed(Vec<String>),
    /// No lyrics found.
    #[default]
    None,
}

/// Custom serde support for std::time::Duration (stored as milliseconds).
mod duration_millis {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::time::Duration;

    pub fn serialize<S: Serializer>(d: &Option<Duration>, s: S) -> Result<S::Ok, S::Error> {
        d.map(|dur| dur.as_millis() as u64).serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Duration>, D::Error> {
        let millis: Option<u64> = Option::deserialize(d)?;
        Ok(millis.map(Duration::from_millis))
    }
}

/// Complete metadata for a single audio track.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TrackMetadata {
    pub title: Option<String>,
    pub sanitized_title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub year: Option<u32>,
    pub track_number: Option<u32>,
    pub disc_number: Option<u32>,
    #[serde(with = "duration_millis")]
    pub duration: Option<Duration>,
    /// Cover art is never cached — re-read from the file when a track is played.
    #[serde(skip)]
    pub cover_art: Option<Vec<u8>>,
    /// Lyrics are never cached — loaded lazily on play.
    #[serde(skip)]
    pub lyrics: LyricsKind,
    pub sample_rate: Option<u32>,
    pub bitrate: Option<u32>,
    #[serde(default)]
    pub cover_url: Option<String>,
}

impl TrackMetadata {
    /// Returns the display title, falling back to the filename stem.
    pub fn display_title(&self, strip_track_numbers: bool) -> &str {
        if strip_track_numbers {
            self.sanitized_title
                .as_deref()
                .unwrap_or_else(|| self.title.as_deref().unwrap_or("Unknown Title"))
        } else {
            self.title.as_deref().unwrap_or("Unknown Title")
        }
    }

    /// Returns the display artist.
    pub fn display_artist(&self) -> &str {
        self.artist.as_deref().unwrap_or("Unknown Artist")
    }

    /// Returns the display album.
    pub fn display_album(&self) -> &str {
        self.album.as_deref().unwrap_or("Unknown Album")
    }

    /// Returns a formatted duration string like "3:45".
    #[allow(dead_code)]
    pub fn display_duration(&self) -> String {
        match self.duration {
            Some(d) => {
                let total_secs = d.as_secs();
                let mins = total_secs / 60;
                let secs = total_secs % 60;
                format!("{}:{:02}", mins, secs)
            }
            None => "--:--".to_string(),
        }
    }

    /// Returns duration in seconds as f64.
    #[allow(dead_code)]
    pub fn duration_secs(&self) -> f64 {
        self.duration.map(|d| d.as_secs_f64()).unwrap_or(0.0)
    }
}

/// Reads metadata from an audio file using lofty.
pub fn read_metadata(path: &Path) -> TrackMetadata {
    let mut meta = TrackMetadata {
        title: path
            .file_stem()
            .and_then(|s| s.to_str())
            .map(|s| s.to_string()),
        ..TrackMetadata::default()
    };

    let tagged = match Probe::open(path)
        .map(|p| p.options(ParseOptions::new().read_cover_art(false)))
        .and_then(|p| p.read())
    {
        Ok(t) => t,
        Err(_) => {
            if let Some(ref title) = meta.title {
                meta.sanitized_title = Some(crate::utils::sanitizer::sanitize_title(title));
            }
            return meta;
        }
    };

    // Duration from audio properties — only store if non-zero (unreadable files report Duration::ZERO)
    let dur = tagged.properties().duration();
    if dur > Duration::ZERO {
        meta.duration = Some(dur);
    }
    meta.sample_rate = tagged.properties().sample_rate();
    meta.bitrate = tagged.properties().audio_bitrate();

    // Extract from primary tag
    if let Some(tag) = tagged.primary_tag().or_else(|| tagged.first_tag()) {
        if let Some(title) = tag.title() {
            if !title.is_empty() {
                meta.title = Some(title.to_string());
            }
        }
        meta.artist = tag.artist().map(|s| s.to_string());
        meta.album = tag.album().map(|s| s.to_string());
        meta.year = tag.year();
        meta.track_number = tag.track();
        meta.disc_number = tag.disk();

        // Extract embedded lyrics
        if let Some(lyrics_val) = tag.get_string(&ItemKey::Lyrics) {
            let text = lyrics_val.to_string();
            if text.contains('[') && text.contains(']') {
                // Likely timed lyrics — will be parsed by lyrics.rs
                meta.lyrics = parse_embedded_timed(&text);
            } else if !text.is_empty() {
                let lines: Vec<String> = text.lines().map(|l| l.to_string()).collect();
                meta.lyrics = LyricsKind::Untimed(lines);
            }
        }

        // Cover art is intentionally NOT loaded here.
        // read_metadata is called for every track during library scanning.
        // Storing Vec<u8> JPEG/PNG blobs for a large library (1000+ tracks)
        // can consume gigabytes of RAM. Cover art is loaded lazily at play-time
        // by calling read_cover_art() only for the currently playing track.
    }

    if let Some(ref title) = meta.title {
        meta.sanitized_title = Some(crate::utils::sanitizer::sanitize_title(title));
    }

    meta
}

/// Maximum allowed cover art size in bytes (10 MB) to prevent memory exhaustion
const MAX_COVER_ART_BYTES: usize = 10 * 1024 * 1024;

/// Reads ONLY the cover art bytes for a specific track path.
/// Called lazily at play-time — never during library scanning.
pub fn read_cover_art(path: &Path) -> Option<Vec<u8>> {
    let tagged = Probe::open(path).and_then(|p| p.read()).ok()?;
    let tag = tagged.primary_tag().or_else(|| tagged.first_tag())?;
    for pic in tag.pictures() {
        let data = pic.data();
        if !data.is_empty() && data.len() <= MAX_COVER_ART_BYTES {
            return Some(data.to_vec());
        }
    }
    None
}

/// Parses embedded timed lyrics (LRC format within tags).
fn parse_embedded_timed(text: &str) -> LyricsKind {
    if let Some(data) = crate::data::lyrics::parse_lrc_content(text) {
        LyricsKind::Timed(data.lines)
    } else {
        LyricsKind::Untimed(text.lines().map(|l| l.to_string()).collect())
    }
}

/// Parses a timestamp string like "01:23.45", "01:23.45,450", or "01:23:45" into seconds.
pub fn parse_timestamp(ts: &str) -> Option<f64> {
    let ts = ts.trim();
    if ts.is_empty() {
        return None;
    }
    // Handle timestamps with commas: "01:23.45,450" (time, duration) -> extract time
    let ts_clean = if let Some(comma_pos) = ts.find(',') {
        ts[..comma_pos].trim()
    } else {
        ts
    };

    let parts: Vec<&str> = ts_clean.split(':').collect();
    match parts.len() {
        2 => {
            let mins: f64 = parts[0].trim().parse().ok()?;
            let secs: f64 = parts[1].trim().parse().ok()?;
            let t = mins * 60.0 + secs;
            if t.is_finite() && t >= 0.0 {
                Some(t)
            } else {
                None
            }
        }
        3 => {
            let hours: f64 = parts[0].trim().parse().ok()?;
            let mins: f64 = parts[1].trim().parse().ok()?;
            let secs: f64 = parts[2].trim().parse().ok()?;
            let t = hours * 3600.0 + mins * 60.0 + secs;
            if t.is_finite() && t >= 0.0 {
                Some(t)
            } else {
                None
            }
        }
        _ => None,
    }
}

/// What an audio file is and its quality, e.g. "FLAC · 16-bit · 44.1 kHz" or
/// "MP3 · 320 kbps · 44.1 kHz". Only the format when the file cannot be read yet
/// (a download still in progress).
pub fn audio_summary(path: &Path) -> Option<String> {
    use lofty::file::FileType;
    let tagged = Probe::open(path)
        .ok()
        .map(|p| p.options(ParseOptions::new().read_cover_art(false)))
        .and_then(|p| p.read().ok());
    let props = tagged.as_ref().map(|t| t.properties());
    let bit_depth = props.and_then(|p| p.bit_depth());
    let format = match tagged.as_ref().map(|t| t.file_type()) {
        Some(FileType::Flac) => "FLAC".to_string(),
        Some(FileType::Mpeg) => "MP3".to_string(),
        Some(FileType::Vorbis) => "Ogg Vorbis".to_string(),
        Some(FileType::Opus) => "Opus".to_string(),
        // An MP4 file with a bit depth holds lossless ALAC
        Some(FileType::Mp4) if bit_depth.is_some() => "ALAC".to_string(),
        Some(FileType::Mp4 | FileType::Aac) => "AAC".to_string(),
        Some(FileType::Wav) => "WAV".to_string(),
        Some(FileType::Aiff) => "AIFF".to_string(),
        Some(FileType::WavPack) => "WavPack".to_string(),
        Some(FileType::Ape) => "APE".to_string(),
        _ => match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
            "m4a" | "mp4" | "aac" => "AAC".to_string(),
            "webm" | "opus" => "Opus".to_string(),
            "ogg" => "Ogg Vorbis".to_string(),
            other => other.to_ascii_uppercase(),
        },
    };
    Some(quality_summary(
        &format,
        bit_depth,
        props.and_then(|p| p.audio_bitrate()),
        props.and_then(|p| p.sample_rate()),
    ))
}

/// Lossless files state their bit depth, lossy ones their bitrate.
fn quality_summary(
    format: &str,
    bit_depth: Option<u8>,
    bitrate_kbps: Option<u32>,
    sample_rate: Option<u32>,
) -> String {
    let mut parts = vec![format.to_string()];
    if let Some(bits) = bit_depth {
        parts.push(format!("{}-bit", bits));
    } else if let Some(kbps) = bitrate_kbps.filter(|&k| k > 0) {
        parts.push(format!("{} kbps", kbps));
    }
    if let Some(hz) = sample_rate.filter(|&hz| hz > 0) {
        parts.push(if hz % 1000 == 0 {
            format!("{} kHz", hz / 1000)
        } else {
            format!("{:.1} kHz", f64::from(hz) / 1000.0)
        });
    }
    parts.join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quality_is_described_by_bit_depth_or_bitrate() {
        assert_eq!(
            quality_summary("FLAC", Some(16), Some(900), Some(44_100)),
            "FLAC · 16-bit · 44.1 kHz"
        );
        assert_eq!(
            quality_summary("MP3", None, Some(320), Some(48_000)),
            "MP3 · 320 kbps · 48 kHz"
        );
        assert_eq!(quality_summary("AAC", None, None, None), "AAC");
    }

    #[test]
    fn test_parse_timestamp_valid() {
        assert_eq!(parse_timestamp("01:23.45"), Some(83.45));
        assert_eq!(parse_timestamp("00:05"), Some(5.0));
        assert_eq!(parse_timestamp("01:02:03"), Some(3723.0));
        assert_eq!(parse_timestamp("01:23.45,450"), Some(83.45));
    }

    #[test]
    fn test_parse_timestamp_nan_and_inf_rejected() {
        assert_eq!(parse_timestamp("nan:nan"), None);
        assert_eq!(parse_timestamp("NaN:00"), None);
        assert_eq!(parse_timestamp("+nan:10"), None);
        assert_eq!(parse_timestamp("-nan:10"), None);
        assert_eq!(parse_timestamp("inf:00"), None);
        assert_eq!(parse_timestamp("-inf:00"), None);
        assert_eq!(parse_timestamp("-01:00"), None);
    }
}
