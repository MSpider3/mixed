use std::path::Path;

use crate::data::metadata::{parse_timestamp, LrcLine};

/// Word-level timestamp for Enhanced LRC.
#[derive(Debug, Clone, PartialEq)]
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

/// Load embedded lyrics directly from track metadata/tags.
pub fn load_lyrics_from_metadata(audio_path: &Path) -> Option<LyricsData> {
    use lofty::{file::TaggedFileExt, probe::Probe, tag::ItemKey};
    let tagged = Probe::open(audio_path).and_then(|p| p.read()).ok()?;
    let tag = tagged.primary_tag().or_else(|| tagged.first_tag())?;
    let lyrics_str = tag.get_string(&ItemKey::Lyrics)?;
    parse_lrc_content(lyrics_str)
}

/// Parse LRC file content into structured lyrics data with full word-by-word timestamp support.
pub fn parse_lrc_content(content: &str) -> Option<LyricsData> {
    let mut lines: Vec<LrcLine> = Vec::new();
    let mut word_timestamps: Vec<Vec<WordTimestamp>> = Vec::new();

    for raw_line in content.lines() {
        let raw_line = raw_line.trim();
        if raw_line.is_empty() {
            continue;
        }

        // Skip metadata tags like [ar:Artist], [ti:Title], [length:03:45], etc.
        let lower = raw_line.to_lowercase();
        if lower.starts_with("[ar:")
            || lower.starts_with("[ti:")
            || lower.starts_with("[al:")
            || lower.starts_with("[by:")
            || lower.starts_with("[offset:")
            || lower.starts_with("[re:")
            || lower.starts_with("[ve:")
            || lower.starts_with("[length:")
            || lower.starts_with("[id:")
            || lower.starts_with("[la:")
        {
            continue;
        }

        // Parse leading line-level timestamp(s) in brackets: [mm:ss.xx]
        if !raw_line.starts_with('[') {
            continue;
        }

        let mut timestamps = Vec::new();
        let mut rem = raw_line;

        while rem.starts_with('[') {
            if let Some(close_bracket) = rem.find(']') {
                let tag = rem[1..close_bracket].trim();
                if let Some(ts) = parse_timestamp(tag) {
                    timestamps.push(ts);
                    rem = rem[close_bracket + 1..].trim_start();
                } else {
                    break;
                }
            } else {
                break;
            }
        }

        if timestamps.is_empty() {
            continue;
        }

        let default_time = timestamps[0];
        let (clean_text, words) = parse_line_tokens(rem, default_time);

        if clean_text.is_empty() && words.is_empty() {
            continue;
        }

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

    // Ensure sorted chronologically by time
    let mut combined: Vec<_> = lines.into_iter().zip(word_timestamps).collect();
    combined.sort_by(|a, b| {
        a.0.time_secs
            .partial_cmp(&b.0.time_secs)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let (sorted_lines, sorted_words): (Vec<_>, Vec<_>) = combined.into_iter().unzip();

    Some(LyricsData {
        is_timed: true,
        lines: sorted_lines,
        word_timestamps: sorted_words,
    })
}

/// Tokenizes a line body to extract word-level timestamps and strip raw timestamp tags.
/// Supports `<mm:ss.xx>`, `[mm:ss.xx]`, `(mm:ss.xx,duration)`, and `{\k...}` karaoke tags.
/// Protects Indic scripts (Devanagari, etc.) by binding timestamps to complete words.
pub fn parse_line_tokens(line_body: &str, line_start_time: f64) -> (String, Vec<WordTimestamp>) {
    let mut words: Vec<WordTimestamp> = Vec::new();
    let mut clean_text = String::with_capacity(line_body.len());
    let mut has_explicit_word_ts = false;

    let mut current_word = String::new();
    let mut current_word_time: Option<f64> = None;
    let mut pending_time: Option<f64> = None;

    let chars: Vec<char> = line_body.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        let ch = chars[i];

        // Check for timestamp or karaoke tags enclosed in <...>, [...], (...), or {\...}
        if ch == '<' || ch == '[' || ch == '(' || (ch == '{' && i + 1 < len && chars[i + 1] == '\\')
        {
            let closing_char = match ch {
                '<' => '>',
                '[' => ']',
                '(' => ')',
                '{' => '}',
                _ => ' ',
            };

            let mut j = i + 1;
            while j < len && chars[j] != closing_char {
                j += 1;
            }

            if j < len {
                let tag_content: String = chars[i + 1..j].iter().collect();
                let trimmed_tag = tag_content.trim();

                if let Some(ts) = parse_timestamp(trimmed_tag) {
                    has_explicit_word_ts = true;
                    if current_word_time.is_none() {
                        current_word_time = Some(ts);
                    } else {
                        pending_time = Some(ts);
                    }
                    i = j + 1;
                    continue;
                } else if trimmed_tag.starts_with("\\k")
                    || trimmed_tag.starts_with("\\K")
                    || trimmed_tag.starts_with("\\kf")
                    || trimmed_tag.starts_with("\\ko")
                {
                    // Karaoke syllable timing tag: strip tag from clean_text
                    i = j + 1;
                    continue;
                }
            }
        }

        if ch.is_whitespace() {
            current_word.push(ch);
            clean_text.push(ch);
            let w_time = current_word_time
                .or(pending_time)
                .unwrap_or(line_start_time);
            if !current_word.trim().is_empty() {
                words.push(WordTimestamp {
                    time_secs: w_time,
                    word: current_word.clone(),
                });
            }
            current_word.clear();
            current_word_time = pending_time.take();
        } else {
            current_word.push(ch);
            clean_text.push(ch);
        }

        i += 1;
    }

    // Flush trailing word
    if !current_word.trim().is_empty() {
        let w_time = current_word_time
            .or(pending_time)
            .unwrap_or(line_start_time);
        words.push(WordTimestamp {
            time_secs: w_time,
            word: current_word,
        });
    }

    if !has_explicit_word_ts {
        words.clear();
    }

    let clean = clean_text.trim().to_string();
    (clean, words)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_enhanced_lrc_standard() {
        let lrc = r#"
[ti:Tum Hi Ho]
[ar:Arijit Singh]
[00:10.00]<00:10.00>Hum <00:10.50>tere <00:11.00>bin <00:11.80>ab <00:12.20>reh <00:12.60>nahi <00:13.00>sakte
[00:15.00]<00:15.00>Tere <00:15.50>bina <00:16.00>kya <00:16.50>wajood <00:17.00>mera
"#;
        let data = parse_lrc_content(lrc).expect("Should parse LRC");
        assert_eq!(data.lines.len(), 2);
        assert_eq!(data.lines[0].time_secs, 10.0);
        assert_eq!(data.lines[0].text, "Hum tere bin ab reh nahi sakte");
        assert_eq!(data.lines[1].time_secs, 15.0);
        assert_eq!(data.lines[1].text, "Tere bina kya wajood mera");

        assert!(data.has_word_timestamps());
        assert_eq!(data.word_timestamps[0].len(), 7);
        assert_eq!(data.word_timestamps[0][0].word.trim(), "Hum");
        assert_eq!(data.word_timestamps[0][0].time_secs, 10.0);
        assert_eq!(data.word_timestamps[0][1].word.trim(), "tere");
        assert_eq!(data.word_timestamps[0][1].time_secs, 10.5);

        // Test active line & active word queries
        assert_eq!(data.find_active_line(9.5), 0);
        assert_eq!(data.find_active_line(10.2), 0);
        assert_eq!(data.find_active_line(15.2), 1);

        assert_eq!(data.find_active_word(0, 10.0), 0); // "Hum"
        assert_eq!(data.find_active_word(0, 10.6), 1); // "tere"
        assert_eq!(data.find_active_word(0, 11.2), 2); // "bin"
        assert_eq!(data.find_active_word(0, 13.5), 6); // "sakte"
    }

    #[test]
    fn test_parse_hindi_devanagari_word_by_word() {
        let lrc = r#"
[00:20.00]<00:20.00>क्योंकि <00:20.60>तुम <00:21.00>ही <00:21.50>हो <00:22.00>अब <00:22.50>तुम <00:23.00>ही <00:23.50>हो
[00:25.00]<00:25.00>जिंदगी <00:25.80>अब <00:26.30>तुम <00:26.80>ही <00:27.40>हो
"#;
        let data = parse_lrc_content(lrc).expect("Should parse Hindi LRC");
        assert_eq!(data.lines.len(), 2);
        assert_eq!(data.lines[0].text, "क्योंकि तुम ही हो अब तुम ही हो");
        assert_eq!(data.lines[1].text, "जिंदगी अब तुम ही हो");
        assert_eq!(data.word_timestamps[0].len(), 8);
        assert_eq!(data.word_timestamps[0][0].word.trim(), "क्योंकि");
        assert_eq!(data.word_timestamps[0][1].word.trim(), "तुम");
    }

    #[test]
    fn test_parse_hindi_syllable_level_does_not_break_words() {
        let lrc = r#"
[00:10.00]<00:10.00>दि<00:10.25>ल <00:10.50>का <00:11.00>द<00:11.25>रि<00:11.40>या <00:11.50>ब<00:11.75>ह <00:12.00>ही <00:12.50>ग<00:12.75>या
"#;
        let data = parse_lrc_content(lrc).expect("Should parse syllable Hindi LRC");
        assert_eq!(data.lines[0].text, "दिल का दरिया बह ही गया");
        assert_eq!(data.word_timestamps[0].len(), 6);
        assert_eq!(data.word_timestamps[0][0].word.trim(), "दिल");
        assert_eq!(data.word_timestamps[0][1].word.trim(), "का");
        assert_eq!(data.word_timestamps[0][2].word.trim(), "दरिया");
        assert_eq!(data.word_timestamps[0][3].word.trim(), "बह");
        assert_eq!(data.word_timestamps[0][4].word.trim(), "ही");
        assert_eq!(data.word_timestamps[0][5].word.trim(), "गया");
    }

    #[test]
    fn test_parse_inline_bracket_word_timestamps() {
        let lrc = r#"
[00:05.00][00:05.00]First [00:05.50]second [00:06.00]third
"#;
        let data = parse_lrc_content(lrc).expect("Should parse inline bracket timestamps");
        assert_eq!(data.lines[0].text, "First second third");
        assert_eq!(data.word_timestamps[0].len(), 3);
        assert_eq!(data.word_timestamps[0][0].word.trim(), "First");
        assert_eq!(data.word_timestamps[0][1].word.trim(), "second");
        assert_eq!(data.word_timestamps[0][2].word.trim(), "third");
    }

    #[test]
    fn test_parse_qrc_and_karaoke_tags() {
        let lrc = r#"
[00:08.00]Hello(00:08.00,400) world(00:08.40,600)
[00:12.00]{\k50}Karaoke {\k80}style {\k100}line
"#;
        let data = parse_lrc_content(lrc).expect("Should parse QRC and karaoke LRC");
        assert_eq!(data.lines[0].text, "Hello world");
        assert_eq!(data.word_timestamps[0].len(), 2);
        assert_eq!(data.word_timestamps[0][0].word.trim(), "Hello");
        assert_eq!(data.word_timestamps[0][0].time_secs, 8.0);
        assert_eq!(data.word_timestamps[0][1].word.trim(), "world");
        assert_eq!(data.word_timestamps[0][1].time_secs, 8.4);
        assert_eq!(data.lines[1].text, "Karaoke style line");
    }

    #[test]
    fn test_plain_lines_with_bracket_text_preserved() {
        let lrc = r#"
[00:01.00]Simple line (Guitar Solo) [Chorus]
"#;
        let data = parse_lrc_content(lrc).expect("Should parse plain line");
        assert_eq!(data.lines[0].text, "Simple line (Guitar Solo) [Chorus]");
        assert!(!data.has_word_timestamps());
    }
}
