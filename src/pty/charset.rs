//! The terminal's **character sets** (`docs/interactive-shell.md`) — the one
//! part of them programs still use: **DEC Special Graphics**, the set a box
//! is drawn in when a program is not told its terminal is UTF-8.
//!
//! ncurses (`nano`, `htop`, `dialog`), S-Lang (`mc`, `whiptail`) and
//! `pstree` switch to it with `ESC ( 0`, draw a box as the letters
//! `lqqk` / `x  x` / `mqqj`, and switch back with `ESC ( B`; a program may
//! also park the set in G1 (`ESC ) 0`) and shift to it with SO (`0x0E`) and
//! back with SI (`0x0F`). [`Charsets`] follows that state for the screen and
//! the transcript alike, so a box reads as `┌──┐` / `│  │` / `└──┘` rather
//! than as a row of `q`s.
//!
//! Pure: the parsers that meet the sequences call in.

/// What a G0/G1 slot holds.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Set {
    /// US-ASCII — and every national set, taken as ASCII: nothing uses them.
    #[default]
    Ascii,
    /// DEC Special Graphics — line drawing.
    Graphics,
}

/// The G0/G1 designations and which of the two is shifted in.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Charsets {
    g0: Set,
    g1: Set,
    /// SO shifted G1 in; SI shifts G0 back.
    shifted: bool,
}

impl Charsets {
    /// `ESC {intermediate} {final_byte}`: a designation when `intermediate`
    /// is `(` (G0) or `)` (G1) — `0` names DEC Special Graphics, anything
    /// else a text set. Returns whether it was one.
    pub fn designate(&mut self, intermediate: u8, final_byte: u8) -> bool {
        let set = if final_byte == b'0' {
            Set::Graphics
        } else {
            Set::Ascii
        };
        match intermediate {
            b'(' => self.g0 = set,
            b')' => self.g1 = set,
            _ => return false,
        }
        true
    }

    /// A control character: SO (`0x0E`) shifts G1 in, SI (`0x0F`) G0.
    /// Returns whether it was one of the two.
    pub fn execute(&mut self, byte: u8) -> bool {
        match byte {
            0x0E => self.shifted = true,
            0x0F => self.shifted = false,
            _ => return false,
        }
        true
    }

    /// `c` as the set shifted in shows it.
    #[must_use]
    pub fn map(&self, c: char) -> char {
        if self.translates() {
            dec_graphics(c)
        } else {
            c
        }
    }

    /// Does the set shifted in show any text other than as itself — is the
    /// line-drawing set in use?
    #[must_use]
    pub fn translates(&self) -> bool {
        let set = if self.shifted { self.g1 } else { self.g0 };
        set == Set::Graphics
    }
}

/// A DEC Special Graphics character as Unicode — xterm's table for
/// `0x5F..=0x7E`; anything else is itself.
#[must_use]
pub fn dec_graphics(c: char) -> char {
    match c {
        '_' => ' ',
        '`' => '◆',
        'a' => '▒',
        'b' => '␉',
        'c' => '␌',
        'd' => '␍',
        'e' => '␊',
        'f' => '°',
        'g' => '±',
        'h' => '␤',
        'i' => '␋',
        'j' => '┘',
        'k' => '┐',
        'l' => '┌',
        'm' => '└',
        'n' => '┼',
        'o' => '⎺',
        'p' => '⎻',
        'q' => '─',
        'r' => '⎼',
        's' => '⎽',
        't' => '├',
        'u' => '┤',
        'v' => '┴',
        'w' => '┬',
        'x' => '│',
        'y' => '≤',
        'z' => '≥',
        '{' => 'π',
        '|' => '≠',
        '}' => '£',
        '~' => '·',
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_line_drawing_letters_are_the_box_characters() {
        let drawn: String = "lqkxmjtuvwn".chars().map(dec_graphics).collect();
        assert_eq!(drawn, "┌─┐│└┘├┤┴┬┼");
    }

    #[test]
    fn the_rest_of_the_graphics_set_maps_too() {
        let drawn: String = "`afgy{|}~z".chars().map(dec_graphics).collect();
        assert_eq!(drawn, "◆▒°±≤π≠£·≥");
        assert_eq!(dec_graphics('_'), ' ', "a blank");
        assert_eq!(dec_graphics('o'), '⎺', "the scan lines");
        assert_eq!(dec_graphics('s'), '⎽');
    }

    #[test]
    fn text_outside_the_graphics_range_is_itself() {
        for c in ['A', 'Z', '0', ' ', '^', 'é', '─'] {
            assert_eq!(dec_graphics(c), c);
        }
    }

    #[test]
    fn g0_designated_graphics_maps_until_ascii_comes_back() {
        let mut sets = Charsets::default();
        assert_eq!(sets.map('q'), 'q', "ASCII to begin with");
        assert!(sets.designate(b'(', b'0'));
        assert_eq!(sets.map('q'), '─');
        assert!(sets.designate(b'(', b'B'));
        assert_eq!(sets.map('q'), 'q');
    }

    #[test]
    fn g1_graphics_shows_only_while_shifted_in() {
        let mut sets = Charsets::default();
        assert!(sets.designate(b')', b'0'));
        assert_eq!(sets.map('x'), 'x', "G0 is still in use");
        assert!(sets.execute(0x0E), "SO");
        assert_eq!(sets.map('x'), '│');
        assert!(sets.execute(0x0F), "SI");
        assert_eq!(sets.map('x'), 'x');
    }

    #[test]
    fn it_says_when_text_would_be_translated() {
        let mut sets = Charsets::default();
        assert!(!sets.translates());
        sets.designate(b')', b'0');
        assert!(!sets.translates(), "parked in G1, not shifted in");
        sets.execute(0x0E);
        assert!(sets.translates());
        sets.execute(0x0F);
        sets.designate(b'(', b'0');
        assert!(sets.translates());
    }

    #[test]
    fn other_sequences_and_controls_are_not_ours() {
        let mut sets = Charsets::default();
        assert!(!sets.designate(b'#', b'8'), "DECALN is no designation");
        assert!(!sets.execute(b'\n'));
        assert!(
            sets.designate(b'(', b'A'),
            "a national set is a designation"
        );
        assert_eq!(sets.map('q'), 'q', "and reads as text");
    }
}
