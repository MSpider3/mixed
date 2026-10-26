//! Indic scripts (Hindi and others) in kitty.
//!
//! kitty gives a cluster such as "या" (a consonant with a vowel sign) one cell, while
//! the layout gives it two, as Unicode widths say. kitty then squeezes the letters into
//! one cell and leaves a gap after them. kitty's text sizing protocol (OSC 66) lets a
//! program state how many cells text takes, so those clusters are sent with their width.

use std::io::{self, Write};

use ratatui::backend::{Backend, ClearType, WindowSize};
use ratatui::buffer::Cell;
use ratatui::layout::{Position, Size};
use unicode_width::UnicodeWidthStr;

/// A backend that sends wide Indic clusters through the text sizing protocol when the
/// terminal supports it (see `theme::kitty_text_sizing`), and passes everything else on.
pub struct KittyText<B> {
    inner: B,
}

impl<B> KittyText<B> {
    pub fn new(inner: B) -> Self {
        Self { inner }
    }
}

fn is_indic(c: char) -> bool {
    // Devanagari to Sinhala
    ('\u{0900}'..='\u{0DFF}').contains(&c)
}

/// What to print for a cell holding `symbol`, if it is a cluster that kitty would
/// otherwise draw narrower than the layout placed it. The protocol allows 1 to 7 cells.
pub fn multicell(symbol: &str) -> Option<String> {
    let width = symbol.width();
    ((2..=7).contains(&width) && symbol.chars().any(is_indic))
        .then(|| format!("\x1b]66;w={};{}\x07", width, symbol))
}

impl<B: Backend> Backend for KittyText<B> {
    fn draw<'a, I>(&mut self, content: I) -> io::Result<()>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        if !crate::ui::theme::kitty_text_sizing() {
            return self.inner.draw(content);
        }
        let cells: Vec<(u16, u16, Cell)> = content
            .map(|(x, y, cell)| {
                let mut cell = cell.clone();
                if let Some(symbol) = multicell(cell.symbol()) {
                    cell.set_symbol(&symbol);
                }
                (x, y, cell)
            })
            .collect();
        self.inner
            .draw(cells.iter().map(|(x, y, cell)| (*x, *y, cell)))
    }

    fn append_lines(&mut self, n: u16) -> io::Result<()> {
        self.inner.append_lines(n)
    }

    fn hide_cursor(&mut self) -> io::Result<()> {
        self.inner.hide_cursor()
    }

    fn show_cursor(&mut self) -> io::Result<()> {
        self.inner.show_cursor()
    }

    fn get_cursor_position(&mut self) -> io::Result<Position> {
        self.inner.get_cursor_position()
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> io::Result<()> {
        self.inner.set_cursor_position(position)
    }

    fn clear(&mut self) -> io::Result<()> {
        self.inner.clear()
    }

    fn clear_region(&mut self, clear_type: ClearType) -> io::Result<()> {
        self.inner.clear_region(clear_type)
    }

    fn size(&self) -> io::Result<Size> {
        self.inner.size()
    }

    fn window_size(&mut self) -> io::Result<WindowSize> {
        self.inner.window_size()
    }

    fn flush(&mut self) -> io::Result<()> {
        Backend::flush(&mut self.inner)
    }
}

impl<B: Write> Write for KittyText<B> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        Write::flush(&mut self.inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_indic_clusters_are_sent_with_their_width() {
        // A consonant with the vowel sign AA: two cells in the layout, one in kitty
        assert_eq!(multicell("या").as_deref(), Some("\x1b]66;w=2;या\x07"));
        // One cell either way: nothing to fix
        assert_eq!(multicell("ग"), None);
        assert_eq!(multicell("सं"), None);
        // Other scripts are left to the terminal
        assert_eq!(multicell("夜"), None);
        assert_eq!(multicell("a"), None);
    }
}
