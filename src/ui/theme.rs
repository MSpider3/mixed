use ratatui::style::Color;

use crate::app::SourceTab;

// ─── Native Terminal theme palette (follows terminal settings) ───
pub const C_FG: Color = Color::White;
pub const C_ACCENT: Color = Color::Magenta;
pub const C_ACCENT2: Color = Color::LightMagenta;
pub const C_CYAN: Color = Color::Cyan;
pub const C_GREEN: Color = Color::Green;
pub const C_ORANGE: Color = Color::Yellow;
pub const C_DIM: Color = Color::Rgb(175, 175, 175);
pub const C_SURFACE: Color = Color::DarkGray;
pub const C_RED: Color = Color::Rgb(248, 81, 73);
pub const C_ACTIVE: Color = Color::LightMagenta;
pub const C_INACTIVE: Color = Color::Rgb(150, 150, 150);

pub fn accent_for(source: SourceTab) -> Color {
    match source {
        SourceTab::Local => C_ACCENT,    // Magenta
        SourceTab::Spotify => C_GREEN,   // Green
        SourceTab::YouTube => C_RED,     // Red
        SourceTab::Unified => C_ACCENT2, // LightMagenta
    }
}
