use std::path::Path;

use crate::data::metadata::{parse_timestamp, LrcLine};

/// Word-level timestamp for Enhanced LRC.
#[derive(Debug, Clone)]
pub struct WordTimestamp {
    pub time_secs: f64,
    pub word: String,
}

/// Complete lyrics data for a track.
#[derive(Debug, Clone)]
pub struct LyricsData {
    pub lines: Vec<LrcLine>,
    #[allow(dead_code)]
    pub is_timed: bool,
    /// Per-line word timestamps (only for Enhanced LRC).
    pub word_timestamps: Vec<Vec<WordTimestamp>>,
}

impl LyricsData {
    /// Find the index of the active line at the given elapsed time.
    pub fn find_active_line(&self, elapsed_secs: f64) -> usize {
        if self.lines.is_empty() {
            return 0;
        }
        let idx = self
            .lines
            .partition_point(|line| line.time_secs <= elapsed_secs);
        idx.saturating_sub(1)
    }

    /// Find the index of the active word within a line at the given elapsed time.
    pub fn find_active_word(&self, line_idx: usize, elapsed_secs: f64) -> usize {
        if line_idx >= self.word_timestamps.len() {
            return 0;
        }
        let words = &self.word_timestamps[line_idx];
        if words.is_empty() {
            return 0;
        }
        let idx = words.partition_point(|w| w.time_secs <= elapsed_secs);
        idx.saturating_sub(1)
    }

    /// Check if this track has word-level timestamps.
    pub fn has_word_timestamps(&self) -> bool {
        self.word_timestamps.iter().any(|w| !w.is_empty())
    }
}

/// Load lyrics from an external .lrc file adjacent to the audio file.
pub fn load_lyrics_from_lrc(audio_path: &Path) -> Option<LyricsData> {
    let lrc_path = audio_path.with_extension("lrc");
    if !lrc_path.exists() {
        return None;
    }

    let content = std::fs::read_to_string(&lrc_path).ok()?;
    parse_lrc_content(&content)
}

/// Parse LRC file content into structured lyrics data.
pub fn parse_lrc_content(content: &str) -> Option<LyricsData> {
    let mut lines: Vec<LrcLine> = Vec::new();
    let mut word_timestamps: Vec<Vec<WordTimestamp>> = Vec::new();

    for raw_line in content.lines() {
        let raw_line = raw_line.trim();
        if raw_line.is_empty() {
            continue;
        }

        // Skip metadata tags like [ar:Artist], [ti:Title], etc.
        if raw_line.starts_with("[ar:")
            || raw_line.starts_with("[ti:")
            || raw_line.starts_with("[al:")
            || raw_line.starts_with("[by:")
            || raw_line.starts_with("[offset:")
            || raw_line.starts_with("[re:")
            || raw_line.starts_with("[ve:")
            || raw_line.starts_with("[length:")
        {
            continue;
        }

        // Parse line-level timestamp
        if !raw_line.starts_with('[') {
            continue;
        }
        let end_bracket = match raw_line.find(']') {
            Some(i) => i,
            None => continue,
        };

        let ts_str = &raw_line[1..end_bracket];
        let time = match parse_timestamp(ts_str) {
            Some(t) => t,
            None => continue,
        };

        // Collect additional timestamps on the same line: [ts1][ts2][ts3]Text
        let mut timestamps = vec![time];
        let mut remainder = raw_line[end_bracket + 1..].trim_start();
        while remainder.starts_with('[') {
            if let Some(end) = remainder.find(']') {
                let extra_ts = &remainder[1..end];
                if let Some(extra_time) = parse_timestamp(extra_ts) {
                    timestamps.push(extra_time);
                    remainder = remainder[end + 1..].trim_start();
                } else {
                    break; // Not a timestamp bracket — it's part of the lyric text
                }
            } else {
                break;
            }
        }
        let text_part = remainder;

        // Parse Enhanced LRC word-level timestamps: <mm:ss.xx>word
        let mut words = Vec::new();
        if text_part.contains('<') && text_part.contains('>') {
            let mut rem = text_part;
            while let Some(start) = rem.find('<') {
                if let Some(end) = rem[start..].find('>') {
                    let word_ts_str = &rem[start + 1..start + end];
                    if let Some(word_time) = parse_timestamp(word_ts_str) {
                        rem = &rem[start + end + 1..];
                        let word_end = rem.find('<').unwrap_or(rem.len());
                        let word = rem[..word_end].to_string();
                        rem = &rem[word_end..];
                        if !word.is_empty() {
                            words.push(WordTimestamp {
                                time_secs: word_time,
                                word,
                            });
                        }
                    } else {
                        break;
                    }
                } else {
                    break;
                }
            }
        }

        // Clean text (strip inline Enhanced LRC tags for display)
        let clean_text = if !words.is_empty() {
            words
                .iter()
                .map(|w| w.word.as_str())
                .collect::<Vec<_>>()
                .join("")
        } else {
            text_part.to_string()
        };

        // Emit one entry per timestamp (multi-timestamp lines share the same text/words)
        for ts in timestamps {
            lines.push(LrcLine {
                time_secs: ts,
                text: clean_text.clone(),
            });
            word_timestamps.push(words.clone());
        }
    }

    if lines.is_empty() {
        return None;
    }

    // Ensure sorted by time
    let mut combined: Vec<_> = lines.into_iter().zip(word_timestamps).collect();
    combined.sort_by(|a, b| a.0.time_secs.partial_cmp(&b.0.time_secs).unwrap());

    let (sorted_lines, sorted_words): (Vec<_>, Vec<_>) = combined.into_iter().unzip();

    Some(LyricsData {
        is_timed: true,
        lines: sorted_lines,
        word_timestamps: sorted_words,
    })
}
