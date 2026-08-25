use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::data::lyrics::{LyricsData, WordTimestamp};

const C_ACTIVE: Color = Color::LightMagenta;
const C_INACTIVE: Color = Color::DarkGray;

/// Centers a single-span text within a given width, padding left and right with spaces.
/// This ensures every cell across the width is explicitly drawn, eliminating ghosting/bleeding.
fn center_pad(text: &str, width: usize) -> String {
    let visual_width = unicode_width::UnicodeWidthStr::width(text);
    if visual_width >= width {
        text.to_string()
    } else {
        let left_pad = (width - visual_width) / 2;
        let right_pad = width.saturating_sub(left_pad + visual_width);
        format!("{}{}{}", " ".repeat(left_pad), text, " ".repeat(right_pad))
    }
}

/// Render a constrained 3-line synced view of the lyrics:
/// Line 1: Previous line (faded)
/// Line 2: Active line (bright/highlighted, clean un-distorted font)
/// Line 3: Next line (faded)
pub fn render_constrained_lyrics(
    f: &mut Frame,
    area: Rect,
    lyrics: &LyricsData,
    elapsed_secs: f64,
) {
    if area.height == 0 || area.width == 0 || lyrics.lines.is_empty() {
        return;
    }

    let width = area.width as usize;
    let active_idx = lyrics.find_active_line(elapsed_secs);
    let mut display_lines = Vec::with_capacity(3);

    // Previous line (dimmed)
    if active_idx > 0 {
        let prev_line = &lyrics.lines[active_idx - 1];
        let padded = center_pad(prev_line.text.trim(), width);
        display_lines.push(Line::from(Span::styled(
            padded,
            Style::default().fg(C_INACTIVE),
        )));
    } else {
        display_lines.push(Line::from(Span::raw(" ".repeat(width))));
    }

    // Active line (highlighted with vibrant color; avoids synthetic bold distortion on Indic fonts)
    let active_line = &lyrics.lines[active_idx];
    if lyrics.has_word_timestamps() && active_idx < lyrics.word_timestamps.len() {
        let words = &lyrics.word_timestamps[active_idx];
        if !words.is_empty() {
            let active_word = lyrics.find_active_word(active_idx, elapsed_secs);
            let spans = render_word_highlighted(words, active_word, width);
            display_lines.push(Line::from(spans));
        } else {
            let active_text = format!("♪ {}", active_line.text.trim());
            let padded = center_pad(&active_text, width);
            display_lines.push(Line::from(Span::styled(
                padded,
                Style::default().fg(C_ACTIVE),
            )));
        }
    } else {
        let active_text = format!("♪ {}", active_line.text.trim());
        let padded = center_pad(&active_text, width);
        display_lines.push(Line::from(Span::styled(
            padded,
            Style::default().fg(C_ACTIVE),
        )));
    }

    // Next line (dimmed)
    if active_idx + 1 < lyrics.lines.len() {
        let next_line = &lyrics.lines[active_idx + 1];
        let padded = center_pad(next_line.text.trim(), width);
        display_lines.push(Line::from(Span::styled(
            padded,
            Style::default().fg(C_INACTIVE),
        )));
    } else {
        display_lines.push(Line::from(Span::raw(" ".repeat(width))));
    }

    let widget = Paragraph::new(display_lines);
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
    if area.height == 0 || area.width == 0 || lyrics.lines.is_empty() {
        return;
    }

    let width = area.width as usize;
    let active_idx = lyrics.find_active_line(elapsed_secs);
    let start = scroll as usize;
    let end = (start + area.height as usize).min(lyrics.lines.len());
    let mut display_lines = Vec::with_capacity(area.height as usize);

    for i in start..end {
        let lyric_line = &lyrics.lines[i];
        if i == active_idx {
            let active_text = format!("♪ {}", lyric_line.text.trim());
            let padded = center_pad(&active_text, width);
            display_lines.push(Line::from(Span::styled(
                padded,
                Style::default().fg(C_ACTIVE),
            )));
        } else {
            let padded = center_pad(lyric_line.text.trim(), width);
            display_lines.push(Line::from(Span::styled(
                padded,
                Style::default().fg(C_INACTIVE),
            )));
        }
    }

    // Fill any empty bottom rows with blank lines of exact width
    while display_lines.len() < area.height as usize {
        display_lines.push(Line::from(Span::raw(" ".repeat(width))));
    }

    let widget = Paragraph::new(display_lines);
    f.render_widget(widget, area);
}

/// Render word-level highlighting for Enhanced LRC with full line padding to prevent visualizer bleeding.
fn render_word_highlighted<'a>(
    words: &'a [WordTimestamp],
    active_word: usize,
    width: usize,
) -> Vec<Span<'a>> {
    let full_text: String = words.iter().map(|w| w.word.as_str()).collect();
    let text_with_icon = format!("♪ {}", full_text.trim());
    let visual_width = unicode_width::UnicodeWidthStr::width(text_with_icon.as_str());

    let left_pad = width.saturating_sub(visual_width) / 2;
    let right_pad = width.saturating_sub(left_pad + visual_width);

    let mut spans = Vec::with_capacity(words.len() + 3);

    if left_pad > 0 {
        spans.push(Span::raw(" ".repeat(left_pad)));
    }

    spans.push(Span::styled(
        "♪ ",
        Style::default().fg(C_ACTIVE).add_modifier(Modifier::BOLD),
    ));

    for (i, word) in words.iter().enumerate() {
        let style = if i <= active_word {
            Style::default().fg(C_ACTIVE)
        } else {
            Style::default().fg(C_INACTIVE)
        };
        spans.push(Span::styled(&word.word, style));
    }

    if right_pad > 0 {
        spans.push(Span::raw(" ".repeat(right_pad)));
    }

    spans
}

/// Render untimed lyrics with full line width padding.
pub fn render_untimed_lyrics(f: &mut Frame, area: Rect, lines: &[String], scroll: u16) {
    if area.height == 0 || area.width == 0 || lines.is_empty() {
        return;
    }

    let width = area.width as usize;
    let start = scroll as usize;
    let end = (start + area.height as usize).min(lines.len());
    let mut display = Vec::with_capacity(area.height as usize);

    for line in &lines[start..end] {
        let padded = center_pad(line.trim(), width);
        display.push(Line::from(Span::styled(
            padded,
            Style::default().fg(Color::Reset),
        )));
    }

    while display.len() < area.height as usize {
        display.push(Line::from(Span::raw(" ".repeat(width))));
    }

    let widget = Paragraph::new(display);
    f.render_widget(widget, area);
}
