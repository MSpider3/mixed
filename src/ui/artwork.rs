use ratatui::{
    layout::Rect,
    style::{Color, Style},
    widgets::Paragraph,
    Frame,
};

#[cfg(target_os = "linux")]
fn get_cell_aspect_ratio() -> f32 {
    unsafe {
        let mut ws = libc::winsize {
            ws_row: 0,
            ws_col: 0,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        if libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut ws) == 0
            && ws.ws_col > 0 && ws.ws_row > 0 && ws.ws_xpixel > 0 && ws.ws_ypixel > 0 {
                let cell_w = ws.ws_xpixel / ws.ws_col;
                let cell_h = ws.ws_ypixel / ws.ws_row;
                if cell_w > 0 && cell_h > 0 {
                    return (cell_h as f32) / (cell_w as f32);
                }
            }
    }
    2.0 // fallback
}

#[cfg(not(target_os = "linux"))]
fn get_cell_aspect_ratio() -> f32 {
    2.0
}

/// Renders cover art using Sixel/Kitty protocol via ratatui-image, falling back to a text placeholder.
pub fn render_artwork(
    f: &mut Frame,
    area: Rect,
    protocol: &mut Option<ratatui_image::protocol::StatefulProtocol>,
) {
    let aspect_ratio = get_cell_aspect_ratio();

    // Calculate largest square in pixel-space mapping to cell grid
    let h_if_full_w = (area.width as f32 / aspect_ratio) as u16;
    let (w, h) = if h_if_full_w <= area.height {
        (area.width, h_if_full_w)
    } else {
        (((area.height as f32) * aspect_ratio) as u16, area.height)
    };

    let w = w.max(4).min(area.width);
    let h = h.max(4).min(area.height);

    // Center the square within the original area
    let x_offset = area.x + (area.width - w) / 2;
    let y_offset = area.y + (area.height - h) / 2;
    let centered_area = Rect::new(x_offset, y_offset, w, h);

    if let Some(ref mut prot) = protocol {
        if centered_area.width >= 4 && centered_area.height >= 4 {
            let widget = ratatui_image::StatefulImage::default();
            f.render_stateful_widget(widget, centered_area, prot);
        }
        return;
    }

    // No cover art — show placeholder
    let placeholder = build_no_art_placeholder(centered_area);
    let widget = Paragraph::new(placeholder).style(Style::default().fg(Color::DarkGray));
    f.render_widget(widget, centered_area);
}

fn build_no_art_placeholder(area: Rect) -> Vec<ratatui::text::Line<'static>> {
    let w = area.width as usize;
    let h = area.height as usize;
    let mut lines = Vec::new();

    for row in 0..h {
        if row == h / 2 {
            let label = "No Cover Art";
            let pad = w.saturating_sub(label.len()) / 2;
            let text = format!("{}{}", " ".repeat(pad), label);
            lines.push(ratatui::text::Line::from(text));
        } else {
            lines.push(ratatui::text::Line::from(" ".repeat(w)));
        }
    }
    lines
}

/// Returns the dynamic width for the album art pane, collapsing below 65 columns.
pub fn art_pane_width(terminal_width: u16) -> u16 {
    if terminal_width < 65 {
        0
    } else {
        let width = (terminal_width * 35) / 100; // 35% of terminal width
        width.clamp(20, 50) // clamp between 20 and 50 columns
    }
}
