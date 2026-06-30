use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::data::lyrics::{LyricsData, WordTimestamp};

const C_ACCENT: Color = Color::Magenta;
const C_ACTIVE: Color = Color::LightMagenta;

/// Render a constrained 3-line synced view of the lyrics:
/// Line 1: Previous line (faded)
/// Line 2: Active line (bright/highlighted)
/// Line 3: Next line (faded)
pub fn render_constrained_lyrics(
    f: &mut Frame,
    area: Rect,
    lyrics: &LyricsData,
    elapsed_secs: f64,
) {
    if area.height == 0 || lyrics.lines.is_empty() {
        return;
    }

    let active_idx = lyrics.find_active_line(elapsed_secs);
    let mut display_lines = Vec::new();

    // Previous line (dimmed)
    if active_idx > 0 {
        let prev_line = &lyrics.lines[active_idx - 1];
        display_lines.push(Line::from(Span::styled(
            prev_line.text.trim(),
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        display_lines.push(Line::from(""));
    }

    // Active line (highlighted)
    let active_line = &lyrics.lines[active_idx];
    if lyrics.has_word_timestamps() && active_idx < lyrics.word_timestamps.len() {
        let words = &lyrics.word_timestamps[active_idx];
        if !words.is_empty() {
            let active_word = lyrics.find_active_word(active_idx, elapsed_secs);
            let spans = render_word_highlighted(words, active_word);
            display_lines.push(Line::from(spans));
        } else {
            display_lines.push(Line::from(vec![
                Span::styled(
                    "♪ ",
                    Style::default().fg(C_ACTIVE).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    active_line.text.trim(),
                    Style::default().fg(C_ACTIVE).add_modifier(Modifier::BOLD),
                ),
            ]));
        }
    } else {
        display_lines.push(Line::from(vec![
            Span::styled(
                "♪ ",
                Style::default().fg(C_ACTIVE).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                active_line.text.trim(),
                Style::default().fg(C_ACTIVE).add_modifier(Modifier::BOLD),
            ),
        ]));
    }

    // Next line (dimmed)
    if active_idx + 1 < lyrics.lines.len() {
        let next_line = &lyrics.lines[active_idx + 1];
        display_lines.push(Line::from(Span::styled(
            next_line.text.trim(),
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        display_lines.push(Line::from(""));
    }

    let widget = Paragraph::new(display_lines).alignment(ratatui::layout::Alignment::Center);
    f.render_widget(widget, area);
}

/// Render full timed lyrics starting from a scroll offset.
pub fn render_full_timed_lyrics(
    f: &mut Frame,
    area: Rect,
    lyrics: &LyricsData,
    elapsed_secs: f64,
    scroll: u16,
) {
    if area.height == 0 || lyrics.lines.is_empty() {
        return;
    }

    let active_idx = lyrics.find_active_line(elapsed_secs);
    let start = scroll as usize;
    let end = (start + area.height as usize).min(lyrics.lines.len());
    let mut display_lines = Vec::new();

    for i in start..end {
        let lyric_line = &lyrics.lines[i];
        if i == active_idx {
            display_lines.push(Line::from(vec![
                Span::styled(
                    "♪ ",
                    Style::default().fg(C_ACTIVE).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    lyric_line.text.trim(),
                    Style::default().fg(C_ACTIVE).add_modifier(Modifier::BOLD),
                ),
            ]));
        } else {
            display_lines.push(Line::from(Span::styled(
                lyric_line.text.trim(),
                Style::default().fg(Color::DarkGray),
            )));
        }
    }

    let widget = Paragraph::new(display_lines).alignment(ratatui::layout::Alignment::Center);
    f.render_widget(widget, area);
}

/// Render word-level highlighting for Enhanced LRC.
fn render_word_highlighted<'a>(words: &'a [WordTimestamp], active_word: usize) -> Vec<Span<'a>> {
    let mut spans = vec![Span::styled(
        "♪ ",
        Style::default().fg(C_ACTIVE).add_modifier(Modifier::BOLD),
    )];

    for (i, word) in words.iter().enumerate() {
        let style = if i <= active_word {
            Style::default().fg(C_ACTIVE).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(C_ACCENT)
        };
        spans.push(Span::styled(&word.word, style));
    }

    spans
}

/// Render untimed lyrics with a simple scroll offset.
pub fn render_untimed_lyrics(f: &mut Frame, area: Rect, lines: &[String], scroll: u16) {
    if area.height == 0 || lines.is_empty() {
        return;
    }

    let start = scroll as usize;
    let end = (start + area.height as usize).min(lines.len());
    let display: Vec<Line> = lines[start..end]
        .iter()
        .map(|l| Line::from(Span::styled(l.trim(), Style::default().fg(Color::Reset))))
        .collect();

    let widget = Paragraph::new(display).alignment(ratatui::layout::Alignment::Center);
    f.render_widget(widget, area);
}
