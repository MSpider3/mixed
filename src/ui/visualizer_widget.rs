use ratatui::{
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use std::cell::RefCell;

/// Block characters for spectrum bars (8 levels of height).
const BAR_CHARS: &[char] = &[' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// Gradient colors for bar height (bottom to top).
const SPECTRUM_GRADIENT: &[Color] = &[Color::Cyan, Color::Magenta, Color::LightMagenta];

thread_local! {
    /// Cache of pre-repeated bar character strings for the current bar_width.
    /// Recomputed only when the terminal is resized (i.e. when bar_width changes).
    static BAR_CACHE: RefCell<Option<(usize, [String; 9])>> = RefCell::new(None);
}

/// Render spectrum visualizer using block characters.
pub fn render_spectrum(f: &mut Frame, area: Rect, bars: &[u16], max_height: u16) {
    if area.height == 0 || area.width == 0 || bars.is_empty() {
        return;
    }

    let height = area.height as usize;
    let width = area.width as usize;

    let num_bars = bars.len(); // exactly 32
    let gap_width = if width >= 63 { 1 } else { 0 };
    let bar_width = if gap_width > 0 {
        ((width - 31) / 32).max(1)
    } else {
        (width / 32).max(1)
    };

    let total_width = num_bars * bar_width + if gap_width > 0 { 31 } else { 0 };
    let left_padding = if width > total_width {
        (width - total_width) / 2
    } else {
        0
    };

    // Rebuild the cached strings if bar_width has changed
    BAR_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        let need_rebuild = match &*cache {
            Some((cached_width, _)) => *cached_width != bar_width,
            None => true,
        };
        if need_rebuild {
            let arr = [
                BAR_CHARS[0].to_string().repeat(bar_width),
                BAR_CHARS[1].to_string().repeat(bar_width),
                BAR_CHARS[2].to_string().repeat(bar_width),
                BAR_CHARS[3].to_string().repeat(bar_width),
                BAR_CHARS[4].to_string().repeat(bar_width),
                BAR_CHARS[5].to_string().repeat(bar_width),
                BAR_CHARS[6].to_string().repeat(bar_width),
                BAR_CHARS[7].to_string().repeat(bar_width),
                BAR_CHARS[8].to_string().repeat(bar_width),
            ];
            *cache = Some((bar_width, arr));
        }
    });

    let mut lines: Vec<Line> = Vec::with_capacity(height);

    // Render from top row to bottom row
    for row in 0..height {
        let row_from_bottom = height - 1 - row;
        let mut spans = Vec::with_capacity(num_bars + 2);

        // Left padding spaces
        if left_padding > 0 {
            spans.push(Span::raw(" ".repeat(left_padding)));
        }

        for bar_idx in 0..num_bars {
            let bar_height = bars[bar_idx] as usize;
            let full_rows = bar_height / 8;
            let partial = bar_height % 8;

            let ch_idx = if row_from_bottom < full_rows {
                8 // Full block
            } else if row_from_bottom == full_rows && partial > 0 {
                partial
            } else {
                0
            };

            // Color based on relative height
            let height_ratio = if max_height > 0 {
                (bars[bar_idx] as f32) / (max_height as f32 * 8.0)
            } else {
                0.0
            };

            let color_idx = ((height_ratio * (SPECTRUM_GRADIENT.len() - 1) as f32) as usize)
                .min(SPECTRUM_GRADIENT.len() - 1);
            let color = SPECTRUM_GRADIENT[color_idx];

            let ch_str = BAR_CACHE.with(|cache| {
                let cache = cache.borrow();
                cache.as_ref().unwrap().1[ch_idx].clone()
            });

            spans.push(Span::styled(
                ch_str,
                Style::default().fg(color),
            ));

            if gap_width > 0 && bar_idx < num_bars - 1 {
                spans.push(Span::raw(" "));
            }
        }

        // Pad right side
        let line_len: usize = spans.iter().map(|s| s.content.chars().count()).sum();
        if line_len < width {
            spans.push(Span::raw(" ".repeat(width - line_len)));
        }

        lines.push(Line::from(spans));
    }

    let widget = Paragraph::new(lines);
    f.render_widget(widget, area);
}

/// Render braille waveform visualizer using Canvas-style dots.
pub fn render_braille(f: &mut Frame, area: Rect, bars: &[f32]) {
    if area.height == 0 || area.width == 0 || bars.is_empty() {
        return;
    }

    let width = area.width as usize;
    let height = area.height as usize;

    // Map normalized bars to braille characters
    let mut lines: Vec<Line> = Vec::with_capacity(height);

    for row in 0..height {
        let row_from_bottom = height - 1 - row;
        let threshold = row_from_bottom as f32 / height as f32;
        // Pre-allocate space for 3-byte braille characters or 1-byte space characters.
        // On average, width * 2 bytes is a very safe estimate.
        let mut text = String::with_capacity(width * 3);

        for col in 0..width {
            let bar_idx = (col * bars.len()) / width;
            let value = bars.get(bar_idx).copied().unwrap_or(0.0);

            if value > threshold {
                text.push('⣿');
            } else if value > threshold - 0.1 {
                text.push('⡇');
            } else {
                text.push(' ');
            }
        }

        let color_ratio = row as f32 / height as f32;
        let color = if color_ratio < 0.33 {
            Color::LightMagenta
        } else if color_ratio < 0.66 {
            Color::Magenta
        } else {
            Color::Cyan
        };

        lines.push(Line::from(Span::styled(text, Style::default().fg(color))));
    }

    let widget = Paragraph::new(lines);
    f.render_widget(widget, area);
}
