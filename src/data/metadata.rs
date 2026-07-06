use lofty::{
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

    pub fn deserialize<'de, D: Deserializer<'de>>(
        d: D,
    ) -> Result<Option<Duration>, D::Error> {
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

    let tagged = match Probe::open(path).and_then(|p| p.read()) {
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

/// Reads ONLY the cover art bytes for a specific track path.
/// Called lazily at play-time — never during library scanning.
pub fn read_cover_art(path: &Path) -> Option<Vec<u8>> {
    let tagged = Probe::open(path).and_then(|p| p.read()).ok()?;
    let tag = tagged.primary_tag().or_else(|| tagged.first_tag())?;
    for pic in tag.pictures() {
        let data = pic.data();
        if !data.is_empty() {
            return Some(data.to_vec());
        }
    }
    None
}

/// Parses embedded timed lyrics (LRC format within tags).
fn parse_embedded_timed(text: &str) -> LyricsKind {
    let mut lines = Vec::new();

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some((time, text)) = parse_lrc_line(line) {
            lines.push(LrcLine {
                time_secs: time,
                text,
            });
        }
    }

    if lines.is_empty() {
        LyricsKind::Untimed(text.lines().map(|l| l.to_string()).collect())
    } else {
        lines.sort_by(|a, b| a.time_secs.partial_cmp(&b.time_secs).unwrap());
        LyricsKind::Timed(lines)
    }
}

/// Parses a single LRC line like "[01:23.45]Some lyrics text".
fn parse_lrc_line(line: &str) -> Option<(f64, String)> {
    if !line.starts_with('[') {
        return None;
    }
    let end_bracket = line.find(']')?;
    let timestamp = &line[1..end_bracket];
    let text = line[end_bracket + 1..].trim().to_string();

    let time = parse_timestamp(timestamp)?;
    Some((time, text))
}

/// Parses a timestamp string like "01:23.45" or "01:23:45" into seconds.
pub fn parse_timestamp(ts: &str) -> Option<f64> {
    let parts: Vec<&str> = ts.split(':').collect();
    match parts.len() {
        2 => {
            let mins: f64 = parts[0].parse().ok()?;
            let secs: f64 = parts[1].parse().ok()?;
            Some(mins * 60.0 + secs)
        }
        3 => {
            let hours: f64 = parts[0].parse().ok()?;
            let mins: f64 = parts[1].parse().ok()?;
            let secs: f64 = parts[2].parse().ok()?;
            Some(hours * 3600.0 + mins * 60.0 + secs)
        }
        _ => None,
    }
}
