use ratatui::buffer::Buffer;
use ratatui::style::Color;
use std::sync::{OnceLock, RwLock};

use crate::app::SourceTab;

// ─── Terminal theme palette ───
// Text uses the terminal's own foreground, accents its palette. Every frame passes
// through `adapt`, which swaps any colour that would be hard to read on the
// terminal's actual background for one that is not.
pub const C_FG: Color = Color::Reset;
pub const C_ACCENT: Color = Color::Magenta;
pub const C_ACCENT2: Color = Color::LightMagenta;
pub const C_CYAN: Color = Color::Cyan;
pub const C_GREEN: Color = Color::Green;
pub const C_ORANGE: Color = Color::Yellow;
/// Secondary text. A placeholder: `adapt` turns it into a shade between the
/// terminal's foreground and background.
pub const C_DIM: Color = Color::Rgb(175, 175, 175);
pub const C_SURFACE: Color = Color::DarkGray;
pub const C_RED: Color = Color::LightRed;
pub const C_ACTIVE: Color = Color::LightMagenta;
/// Tertiary text, fainter than `C_DIM`. A placeholder like `C_DIM`.
pub const C_INACTIVE: Color = Color::Rgb(150, 150, 150);
/// Background of the selected row. A placeholder: `adapt` turns it into the
/// terminal's background moved a little towards its foreground.
pub const C_ROW_BG: Color = Color::Rgb(38, 38, 38);

pub fn accent_for(source: SourceTab) -> Color {
    match source {
        SourceTab::Local => C_ACCENT,    // Magenta
        SourceTab::Spotify => C_GREEN,   // Green
        SourceTab::YouTube => C_RED,     // Red
        SourceTab::Unified => C_ACCENT2, // LightMagenta
    }
}

/// The colours a terminal reported for itself.
#[derive(Debug, Clone, PartialEq)]
pub struct TerminalColors {
    pub fg: (u8, u8, u8),
    pub bg: (u8, u8, u8),
    /// The 16 ANSI colours, where the terminal reported them.
    pub ansi: [Option<(u8, u8, u8)>; 16],
}

// Replaced when the terminal's colours change while mixed runs (see `redetect_colors`)
static TERMINAL_COLORS: RwLock<Option<TerminalColors>> = RwLock::new(None);
static KITTY_TEXT_SIZING: OnceLock<bool> = OnceLock::new();

/// True if the terminal is a kitty that supports the text sizing protocol (0.40 and later).
pub fn kitty_text_sizing() -> bool {
    KITTY_TEXT_SIZING.get().copied().unwrap_or(false)
}

/// Read a kitty version from the reply to XTVERSION, e.g. `ESC P >| kitty(0.47.1) ESC \`.
fn kitty_version(replies: &str) -> Option<(u32, u32)> {
    let version = replies.split("kitty(").nth(1)?.split(')').next()?;
    let mut parts = version.split('.').map(|p| p.parse::<u32>().ok());
    Some((parts.next()??, parts.next()??))
}

/// Use these colours for `adapt` (normally found by `detect_terminal_colors`).
pub fn set_terminal_colors(colors: Option<TerminalColors>) {
    *TERMINAL_COLORS.write().unwrap_or_else(|e| e.into_inner()) = colors;
}

/// Text needs this contrast with the background to stay easy to read (WCAG AA).
const TEXT_CONTRAST: f64 = 4.5;
/// Accents (titles, highlights, markers) need at least this much (WCAG AA large text).
const ACCENT_CONTRAST: f64 = 3.0;

fn luminance((r, g, b): (u8, u8, u8)) -> f64 {
    let channel = |c: u8| {
        let c = f64::from(c) / 255.0;
        if c <= 0.039_28 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b)
}

fn contrast(a: (u8, u8, u8), b: (u8, u8, u8)) -> f64 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// The shade a `share` of the way from `fg` towards `bg`, backed off towards `fg`
/// until it keeps `min_contrast` with the background.
fn shade(colors: &TerminalColors, share: f64, min_contrast: f64) -> Color {
    let mix = |t: f64| {
        let channel = |f: u8, b: u8| (f64::from(f) * (1.0 - t) + f64::from(b) * t).round() as u8;
        let (f, b) = (colors.fg, colors.bg);
        (channel(f.0, b.0), channel(f.1, b.1), channel(f.2, b.2))
    };
    let mut t = share;
    while t > 0.0 && contrast(mix(t), colors.bg) < min_contrast {
        t -= 0.05;
    }
    if t <= 0.0 {
        return Color::Reset;
    }
    let (r, g, b) = mix(t);
    Color::Rgb(r, g, b)
}

/// ANSI index of a named colour, and of its normal/bright counterpart.
fn ansi_pair(color: Color) -> Option<(usize, usize)> {
    let index = match color {
        Color::Black => 0,
        Color::Red => 1,
        Color::Green => 2,
        Color::Yellow => 3,
        Color::Blue => 4,
        Color::Magenta => 5,
        Color::Cyan => 6,
        Color::Gray => 7,
        Color::DarkGray => 8,
        Color::LightRed => 9,
        Color::LightGreen => 10,
        Color::LightYellow => 11,
        Color::LightBlue => 12,
        Color::LightMagenta => 13,
        Color::LightCyan => 14,
        Color::White => 15,
        _ => return None,
    };
    Some((index, (index + 8) % 16))
}

fn ansi_color(index: usize) -> Color {
    Color::Indexed(index as u8)
}

/// The colour to draw text in instead of `color`, on these terminal colours.
fn readable(color: Color, colors: Option<&TerminalColors>) -> Color {
    match (color, colors) {
        (C_DIM, Some(colors)) => shade(colors, 0.35, TEXT_CONTRAST),
        (C_INACTIVE, Some(colors)) => shade(colors, 0.45, ACCENT_CONTRAST),
        // Without the terminal's colours a fixed grey may vanish: use its foreground
        (C_DIM | C_INACTIVE, None) => Color::Reset,
        (_, Some(colors)) => {
            let Some((index, pair)) = ansi_pair(color) else {
                return color;
            };
            let fits = |i: usize| {
                colors.ansi[i].is_some_and(|c| contrast(c, colors.bg) >= ACCENT_CONTRAST)
            };
            if colors.ansi[index].is_none() || fits(index) {
                color
            } else if fits(pair) {
                ansi_color(pair)
            } else {
                Color::Reset
            }
        }
        (_, None) => color,
    }
}

/// The selected row's background: the terminal's own, a little towards its text colour.
fn row_background(colors: &TerminalColors) -> Color {
    let channel = |b: u8, f: u8| (f64::from(b) * 0.88 + f64::from(f) * 0.12).round() as u8;
    let (b, f) = (colors.bg, colors.fg);
    Color::Rgb(channel(b.0, f.0), channel(b.1, f.1), channel(b.2, f.2))
}

/// Make every text colour of a finished frame readable on the terminal's background.
pub fn adapt(buf: &mut Buffer) {
    let guard = TERMINAL_COLORS.read().unwrap_or_else(|e| e.into_inner());
    let colors = guard.as_ref();
    let row_bg = colors.map(row_background);
    for cell in &mut buf.content {
        cell.fg = readable(cell.fg, colors);
        if cell.bg == C_ROW_BG {
            match row_bg {
                Some(bg) => cell.bg = bg,
                // Unknown background: mark the row by swapping its colours instead
                None => {
                    cell.bg = Color::Reset;
                    cell.modifier.insert(ratatui::style::Modifier::REVERSED);
                }
            }
        }
    }
}

/// Parse a colour report such as `rgb:1e1e/2727/5252` (1 to 4 hex digits per channel).
fn parse_rgb(spec: &str) -> Option<(u8, u8, u8)> {
    let mut channels = spec.strip_prefix("rgb:")?.split('/').map(|hex| {
        let value = u32::from_str_radix(hex, 16).ok()?;
        let max = (1u32 << (4 * hex.len() as u32)) - 1;
        (hex.len() <= 4).then(|| (value * 255 / max) as u8)
    });
    let rgb = (channels.next()??, channels.next()??, channels.next()??);
    channels.next().is_none().then_some(rgb)
}

/// Read the replies to OSC 10 (foreground), 11 (background) and 4 (palette) queries.
fn parse_replies(replies: &str) -> Option<TerminalColors> {
    let mut fg = None;
    let mut bg = None;
    let mut ansi = [None; 16];
    for reply in replies.split("\x1b]").skip(1) {
        let reply = reply.split(['\x07', '\x1b']).next().unwrap_or("");
        let mut parts = reply.split(';');
        match parts.next() {
            Some("10") => fg = parts.next().and_then(parse_rgb),
            Some("11") => bg = parts.next().and_then(parse_rgb),
            Some("4") => {
                let index = parts.next().and_then(|i| i.parse::<usize>().ok());
                if let (Some(i), Some(rgb)) = (index, parts.next().and_then(parse_rgb)) {
                    if i < 16 {
                        ansi[i] = Some(rgb);
                    }
                }
            }
            _ => {}
        }
    }
    Some(TerminalColors {
        fg: fg?,
        bg: bg?,
        ansi,
    })
}

/// Ask the terminal for its colours, so that `adapt` can keep text readable on any
/// colour scheme, and for its version (see `kitty_text_sizing`). Must run in raw
/// mode, before anything else reads the terminal.
pub fn detect_terminal() {
    let replies = query_terminal();
    let colors = parse_replies(&replies);
    log::info!("Terminal colours: {:?}", colors);
    set_terminal_colors(colors);
    let _ = KITTY_TEXT_SIZING.set(kitty_version(&replies).is_some_and(|v| v >= (0, 40)));
}

/// Ask the terminal for its colours again, as its theme may have changed. Must run
/// where nothing else reads the terminal meanwhile. True if the colours changed.
pub fn redetect_colors() -> bool {
    // No answer: keep what is known
    let Some(colors) = parse_replies(&query_terminal()) else {
        return false;
    };
    let mut current = TERMINAL_COLORS.write().unwrap_or_else(|e| e.into_inner());
    if current.as_ref() == Some(&colors) {
        return false;
    }
    log::info!("Terminal colours changed: {:?}", colors);
    *current = Some(colors);
    true
}

#[cfg(unix)]
fn query_terminal() -> String {
    use std::io::Write;
    use std::time::{Duration, Instant};

    if unsafe { libc::isatty(0) } == 0 {
        return String::new();
    }
    let mut query = String::from("\x1b]10;?\x1b\\\x1b]11;?\x1b\\");
    for i in 0..16 {
        query.push_str(&format!("\x1b]4;{};?\x1b\\", i));
    }
    // The terminal's name and version
    query.push_str("\x1b[>0q");
    // Every terminal answers the device attributes query, and answers in order:
    // its reply marks the end of the other replies.
    query.push_str("\x1b[c");
    let mut out = std::io::stdout();
    if out
        .write_all(query.as_bytes())
        .and_then(|_| out.flush())
        .is_err()
    {
        return String::new();
    }

    let deadline = Instant::now() + Duration::from_millis(300);
    let mut replies = Vec::new();
    loop {
        let left = deadline
            .saturating_duration_since(Instant::now())
            .as_millis() as i32;
        let mut poll = libc::pollfd {
            fd: 0,
            events: libc::POLLIN,
            revents: 0,
        };
        if left <= 0 || unsafe { libc::poll(&mut poll, 1, left) } <= 0 {
            break;
        }
        let mut chunk = [0u8; 1024];
        let n = unsafe { libc::read(0, chunk.as_mut_ptr().cast(), chunk.len()) };
        if n <= 0 {
            break;
        }
        replies.extend_from_slice(&chunk[..n as usize]);
        // The device attributes reply looks like ESC [ ? ... c
        let text = String::from_utf8_lossy(&replies);
        if text
            .rfind("\x1b[?")
            .is_some_and(|start| text[start..].contains('c'))
        {
            break;
        }
    }
    String::from_utf8_lossy(&replies).into_owned()
}

#[cfg(not(unix))]
fn query_terminal() -> String {
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(h: &str) -> (u8, u8, u8) {
        let v = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).unwrap();
        (v(1), v(3), v(5))
    }

    /// A light theme whose magenta and bright white are almost the background colour.
    fn light() -> TerminalColors {
        let mut ansi = [None; 16];
        for (i, c) in [
            "#F7F3E8", "#7f203a", "#136420", "#6c6b00", "#31478c", "#D5DEF6", "#334786", "#232425",
            "#b4b6b9", "#a03d57", "#2e933e", "#8a8600", "#314a9a", "#6274ae", "#7885ae", "#c4c5c9",
        ]
        .iter()
        .enumerate()
        {
            ansi[i] = Some(hex(c));
        }
        TerminalColors {
            fg: hex("#1e2752"),
            bg: hex("#efeadb"),
            ansi,
        }
    }

    fn dark() -> TerminalColors {
        let mut colors = light();
        colors.fg = hex("#dddddd");
        colors.bg = hex("#000000");
        colors.ansi[5] = Some(hex("#cb1ed1"));
        colors.ansi[6] = Some(hex("#0dcdcd"));
        colors.ansi[15] = Some(hex("#ffffff"));
        colors
    }

    fn text_contrast(color: Color, colors: &TerminalColors) -> f64 {
        let rgb = match color {
            Color::Reset => colors.fg,
            Color::Rgb(r, g, b) => (r, g, b),
            Color::Indexed(i) => colors.ansi[i as usize].unwrap(),
            other => colors.ansi[ansi_pair(other).unwrap().0].unwrap(),
        };
        contrast(rgb, colors.bg)
    }

    #[test]
    fn every_colour_stays_readable_on_a_light_and_a_dark_theme() {
        let used = [
            C_FG,
            C_ACCENT,
            C_ACCENT2,
            C_CYAN,
            C_GREEN,
            C_ORANGE,
            C_DIM,
            C_SURFACE,
            C_RED,
            C_ACTIVE,
            C_INACTIVE,
            Color::White,
            Color::LightCyan,
            Color::LightGreen,
        ];
        for colors in [light(), dark()] {
            for color in used {
                let shown = readable(color, Some(&colors));
                assert!(
                    text_contrast(shown, &colors) >= ACCENT_CONTRAST,
                    "{:?} shown as {:?} on {:?}",
                    color,
                    shown,
                    colors.bg
                );
            }
            assert!(text_contrast(readable(C_DIM, Some(&colors)), &colors) >= TEXT_CONTRAST);
        }
    }

    #[test]
    fn readable_colours_are_left_alone() {
        let colors = dark();
        assert_eq!(readable(C_ACCENT, Some(&colors)), C_ACCENT);
        assert_eq!(readable(C_CYAN, Some(&colors)), C_CYAN);
        // The light theme's magenta is replaced by its bright counterpart
        assert_eq!(readable(C_ACCENT, Some(&light())), ansi_color(13));
        // Without the terminal's colours, only the fixed greys change
        assert_eq!(readable(C_ACCENT, None), C_ACCENT);
        assert_eq!(readable(C_DIM, None), Color::Reset);
    }

    #[test]
    fn terminal_replies_are_parsed() {
        let replies = "\x1b]10;rgb:1e1e/2727/5252\x1b\\\x1b]11;rgb:efef/eaea/dbdb\x07\
                       \x1b]4;5;rgb:d5/de/f6\x1b\\\x1b[?62;c";
        let colors = parse_replies(replies).unwrap();
        assert_eq!(colors.fg, hex("#1e2752"));
        assert_eq!(colors.bg, hex("#efeadb"));
        assert_eq!(colors.ansi[5], Some(hex("#d5def6")));
        assert_eq!(colors.ansi[0], None);
        // No background reported: nothing to compare against
        assert!(parse_replies("\x1b]10;rgb:ff/ff/ff\x1b\\").is_none());
        assert_eq!(parse_rgb("rgb:f/0/8"), Some((255, 0, 136)));
        assert_eq!(parse_rgb("rgb:12345/0/0"), None);
    }

    #[test]
    fn text_stays_readable_on_the_selected_row() {
        for colors in [light(), dark()] {
            let Color::Rgb(r, g, b) = row_background(&colors) else {
                panic!("the row background is an exact colour");
            };
            assert_ne!((r, g, b), colors.bg, "the row stands out");
            assert!(contrast(colors.fg, (r, g, b)) >= TEXT_CONTRAST);
        }
    }

    #[test]
    fn kitty_versions_are_read_from_xtversion() {
        assert_eq!(kitty_version("\x1bP>|kitty(0.47.1)\x1b\\"), Some((0, 47)));
        assert_eq!(kitty_version("\x1bP>|WezTerm 20240203\x1b\\"), None);
        assert!(kitty_version("\x1bP>|kitty(0.39.1)\x1b\\").unwrap() < (0, 40));
    }
}
