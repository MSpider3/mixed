use regex::Regex;
use std::sync::OnceLock;

static STRIP_TRACK_NUMBER: OnceLock<Regex> = OnceLock::new();

/// Strips terminal control and escape characters to prevent OSC/ANSI sequence injection.
pub fn sanitize_terminal_title(title: &str) -> String {
    title.chars().filter(|c| !c.is_control()).collect()
}

/// Replicates `kew`'s `format_filename` track number stripping and underscore replacement logic.
pub fn sanitize_title(title: &str) -> String {
    // 1. Replace underscores with spaces
    let mut sanitized = title.replace('_', " ");

    // 2. Strip extensions safely on UTF-8 char boundaries
    if let Some(dot_idx) = sanitized.rfind('.') {
        let ext = &sanitized[dot_idx + 1..];
        if [
            "mp3", "flac", "ogg", "opus", "wav", "m4a", "aac", "wma", "webm",
        ]
        .iter()
        .any(|&e| ext.eq_ignore_ascii_case(e))
        {
            sanitized.truncate(dot_idx);
        }
    }

    // 3. Strip track numbers
    let re = STRIP_TRACK_NUMBER
        .get_or_init(|| Regex::new(r"^([\s\p{P}]*)(\d+)([-.]\d+)?(\s*[-.,]\s*|\s+)").unwrap());

    if let Some(m) = re.find(&sanitized) {
        let remainder = &sanitized[m.end()..];
        if !remainder.trim().is_empty() {
            sanitized = remainder.to_string();
        }
    }

    // 4. Trim whitespace
    sanitized.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_title() {
        assert_eq!(sanitize_title("01 - Track_Name.mp3"), "Track Name");
        assert_eq!(sanitize_title("02. Track_Name.FLAC"), "Track Name");
        assert_eq!(sanitize_title("102 Track Name.ogg"), "Track Name");
        assert_eq!(sanitize_title("[01] Track Name"), "[01] Track Name");
        assert_eq!(sanitize_title("01-02 - Track Name.wav"), "Track Name");
        assert_eq!(sanitize_title("01 - "), "01 -"); // Should not leave empty string
        assert_eq!(
            sanitize_title("Track Without Number"),
            "Track Without Number"
        );
        assert_eq!(sanitize_title("İstanbul Track_01.MP3"), "İstanbul Track 01");
    }

    #[test]
    fn test_sanitize_terminal_title() {
        assert_eq!(
            sanitize_terminal_title("Track\x1b]0;hacked\x07Name"),
            "Track]0;hackedName"
        );
        assert_eq!(
            sanitize_terminal_title("Normal Title - Artist"),
            "Normal Title - Artist"
        );
    }
}
