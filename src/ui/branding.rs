use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    Frame,
};

/// The "mixed" ASCII art logo rendered in the accent gradient.
const LOGO: &[&str] = &[
    r"  ███╗   ███╗██╗██╗  ██╗███████╗██████╗ ",
    r"  ████╗ ████║██║╚██╗██╔╝██╔════╝██╔══██╗",
    r"  ██╔████╔██║██║ ╚███╔╝ █████╗  ██║  ██║",
    r"  ██║╚██╔╝██║██║ ██╔██╗ ██╔══╝  ██║  ██║",
    r"  ██║ ╚═╝ ██║██║██╔╝ ██╗███████╗██████╔╝",
    r"  ╚═╝     ╚═╝╚═╝╚═╝  ╚═╝╚══════╝╚═════╝ ",
];

/// Gradient colors for the logo lines (top to bottom).
const LOGO_GRADIENT: &[Color] = &[
    Color::Magenta,
    Color::LightMagenta,
    Color::Cyan,
    Color::LightCyan,
    Color::Green,
    Color::LightGreen,
];

/// Render the mixed logo into the given area.
pub fn render_logo(f: &mut Frame, area: Rect) {
    render_logo_with_song(f, area, None);
}

/// Render the mixed logo and the currently playing song next to it if playing.
pub fn render_logo_with_song(f: &mut Frame, area: Rect, song_name: Option<&str>) {
    let logo_width = 40;
    let lines: Vec<Line> = LOGO
        .iter()
        .enumerate()
        .map(|(i, &text)| {
            let color = LOGO_GRADIENT.get(i).copied().unwrap_or(Color::Magenta);
            let logo_span = Span::styled(
                text,
                Style::default()
                    .fg(color)
                    .add_modifier(Modifier::BOLD | Modifier::ITALIC),
            );

            if let Some(song) = song_name {
                if i == 2 {
                    let total_width = area.width as usize;
                    if total_width > logo_width {
                        let padding = (total_width - logo_width) / 2;
                        let max_song_len = total_width.saturating_sub(padding + logo_width + 5);
                        let display_song =
                            if song.chars().count() > max_song_len && max_song_len > 3 {
                                let mut truncated: String =
                                    song.chars().take(max_song_len - 3).collect();
                                truncated.push_str("...");
                                truncated
                            } else {
                                song.to_string()
                            };

                        if max_song_len > 0 {
                            let line_spans = vec![
                                Span::raw(" ".repeat(padding)),
                                logo_span,
                                Span::raw("  "),
                                Span::styled(
                                    format!("▶ {}", display_song),
                                    Style::default()
                                        .fg(Color::LightMagenta)
                                        .add_modifier(Modifier::BOLD),
                                ),
                            ];
                            return Line::from(line_spans);
                        }
                    }
                }
            }

            // Centered logo rendering
            let total_width = area.width as usize;
            if total_width > logo_width {
                let padding = (total_width - logo_width) / 2;
                Line::from(vec![Span::raw(" ".repeat(padding)), logo_span])
            } else {
                Line::from(logo_span)
            }
        })
        .collect();

    let widget = ratatui::widgets::Paragraph::new(lines);
    f.render_widget(widget, area);
}

/// Returns the number of rows the logo occupies.
pub fn logo_height() -> u16 {
    LOGO.len() as u16
}
