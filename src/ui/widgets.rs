use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState},
    Frame,
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::ui::theme::{C_ACCENT, C_ACCENT2, C_CYAN, C_DIM, C_FG, C_GREEN};

/// Calculate scroll offset and slice (start, end) for a scrollable list.
pub fn scroll_offset(cursor: usize, total: usize, visible_height: usize) -> (usize, usize) {
    if total == 0 || visible_height == 0 {
        return (0, 0);
    }
    let half = visible_height / 2;
    let scroll = cursor.saturating_sub(half);
    let scroll = scroll.min(total.saturating_sub(1));
    let start = scroll;
    let end = (scroll + visible_height).min(total);
    (start, end.max(start))
}

/// Render a consistent thumb-only vertical scrollbar on the right border of an area.
pub fn render_scrollbar(f: &mut Frame, area: Rect, total: usize, visible: usize, offset: usize) {
    if total > visible && visible > 0 {
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None)
            .track_symbol(None)
            .thumb_symbol("▐");
        let mut scrollbar_state =
            ScrollbarState::new(total.saturating_sub(visible)).position(offset);
        f.render_stateful_widget(scrollbar, area, &mut scrollbar_state);
    }
}

/// Compute standard item style based on cursor position and playback state.
pub fn row_style(is_cursor: bool, is_playing: bool) -> Style {
    if is_playing && is_cursor {
        Style::default().fg(C_GREEN).add_modifier(Modifier::BOLD)
    } else if is_playing {
        Style::default().fg(C_GREEN)
    } else if is_cursor {
        Style::default().fg(C_CYAN).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(C_FG)
    }
}

/// Render an empty list hint paragraph.
pub fn render_empty(f: &mut Frame, area: Rect, msg: &str) {
    let hint = Paragraph::new(Line::from(Span::styled(
        format!("  {}", msg),
        Style::default().fg(C_DIM),
    )))
    .alignment(ratatui::layout::Alignment::Left);
    f.render_widget(hint, area);
}

/// Render an interactive text input line with cursor block.
pub fn render_input_line(
    f: &mut Frame,
    area: Rect,
    prompt: &str,
    text: &str,
    active: bool,
    alignment: ratatui::layout::Alignment,
) {
    let cursor = if active { "█" } else { "" };
    let input_line = Line::from(vec![
        Span::styled(prompt, Style::default().fg(C_ACCENT)),
        Span::styled(text, Style::default().fg(C_FG)),
        Span::styled(cursor, Style::default().fg(C_ACCENT2)),
    ]);
    let input_widget = Paragraph::new(input_line).alignment(alignment);
    f.render_widget(input_widget, area);
}

/// Truncate a UTF-8 string so its displayed monospace width does not exceed `max_width`.
pub fn truncate_to_width(text: &str, max_width: usize) -> &str {
    let mut current_width = 0;
    for (byte_idx, ch) in text.char_indices() {
        let ch_width = ch.width().unwrap_or(0);
        if current_width + ch_width > max_width {
            return &text[..byte_idx];
        }
        current_width += ch_width;
    }
    text
}

/// `text` in exactly `width` cells: cut with "…" when longer, padded when shorter.
pub fn fit(text: &str, width: usize) -> String {
    let w = text.width();
    if w <= width {
        return format!("{}{}", text, " ".repeat(width - w));
    }
    if width == 0 {
        return String::new();
    }
    let cut = truncate_to_width(text, width - 1);
    format!("{}…{}", cut, " ".repeat(width - 1 - cut.width()))
}

/// `text` right-aligned in `width` cells (cut like `fit` when longer).
pub fn fit_right(text: &str, width: usize) -> String {
    let w = text.width();
    if w >= width {
        return fit(text, width);
    }
    format!("{}{}", " ".repeat(width - w), text)
}

/// A total length such as "52 min" or "3h 5m".
pub fn format_total(secs: u64) -> String {
    let (hours, mins) = (secs / 3600, (secs % 3600) / 60);
    if hours > 0 {
        format!("{}h {}m", hours, mins)
    } else {
        format!("{} min", mins)
    }
}

/// A track length such as "4:04".
pub fn format_secs(secs: u64) -> String {
    format!("{}:{:02}", secs / 60, secs % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scroll_offset() {
        // Empty
        assert_eq!(scroll_offset(0, 0, 10), (0, 0));
        // Fits entirely
        assert_eq!(scroll_offset(2, 5, 10), (0, 5));
        // Scrolled midway
        assert_eq!(scroll_offset(10, 20, 6), (7, 13));
    }

    #[test]
    fn columns_are_cut_and_padded_to_their_width() {
        assert_eq!(fit("Raabta", 8), "Raabta  ");
        assert_eq!(fit("Dancing With A Stranger", 8), "Dancing…");
        assert_eq!(fit("夜に駆ける", 6), "夜に… ");
        assert_eq!(fit_right("4:04", 6), "  4:04");
        assert_eq!(format_total(52 * 60 + 10), "52 min");
        assert_eq!(format_total(3 * 3600 + 5 * 60), "3h 5m");
        assert_eq!(format_secs(244), "4:04");
    }

    #[test]
    fn test_truncate_to_width() {
        assert_eq!(truncate_to_width("hello", 10), "hello");
        assert_eq!(truncate_to_width("hello world", 5), "hello");
        // Multi-byte Unicode: "你好" is width 4 (2 each)
        assert_eq!(truncate_to_width("你好世界", 4), "你好");
        assert_eq!(truncate_to_width("你好世界", 5), "你好");
    }
}
