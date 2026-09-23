//! The `bash_session` tool's `input` notation (`docs/interactive-shell.md`):
//! literal text with **named keys in angle brackets** — `<Enter>`, `<C-c>`,
//! `<Up>`, `<F5>` — parsed into [`InputPart`]s and encoded into the bytes a
//! terminal sends for them.
//!
//! Why a notation rather than escape codes: models are unreliable at emitting
//! `\u0003` inside a JSON string but fluent in Vim/tmux key names, and one
//! string can then interleave typing and keys in any order
//! (`y<Enter><Down><Enter>`). Anything in angle brackets that is **not** a key
//! name is typed as written, so `a<b`, `<div>` and `x < y > z` survive; `<lt>`
//! is a literal `<` for the one case that would not.
//!
//! Pure: the only terminal state encoding depends on — the cursor-key mode —
//! is passed in.

/// One key the notation names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Enter,
    Tab,
    /// Shift+Tab (`<S-Tab>`).
    BackTab,
    Esc,
    Space,
    Backspace,
    Delete,
    Insert,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    /// `F1`–`F12`.
    F(u8),
    /// A control character — the byte itself (`<C-c>` is `0x03`, `<C-@>`
    /// and `<C-Space>` are `0x00`, `<C-?>` is `0x7f`).
    Ctrl(u8),
    /// Alt/Meta + a character: `ESC` followed by it (`<M-x>`).
    Alt(char),
}

/// One run of the input: text typed as written, or one named key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputPart {
    Text(String),
    Key(Key),
}

/// Bytes to write in one go, and whether the writer should pause after them
/// before writing what follows — true after a lone `Esc` that more input
/// follows, so a program timing its escape sequences (Vim's `ttimeoutlen`)
/// reads a key press, not `Alt+`the next character.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputChunk {
    pub bytes: Vec<u8>,
    pub pause_after: bool,
}

/// The longest text between angle brackets that can still be a key name —
/// `<PageDown>`, `<Ctrl+Space>` — so a `<` far from any `>` is settled at a
/// glance rather than by scanning the rest of the input.
const MAX_KEY_NAME: usize = 12;

/// Split `input` into text runs and named keys (see the module docs).
///
/// Input **escaped twice** is read as the keys it spells: a model that writes
/// `"yes\\n"` into its JSON — a backslash and an `n` where it meant Enter, a
/// slip several models make — has the one level of escaping undone, so the
/// program gets its answer instead of a backslash and a line never submitted.
/// Only input that plainly slipped qualifies (see `undo_double_escape`); a
/// backslash anywhere else is typed as written.
#[must_use]
pub fn parse_input(input: &str) -> Vec<InputPart> {
    let parts = split_keys(input);
    let uses_notation = parts.iter().any(|part| matches!(part, InputPart::Key(_)));
    match (!uses_notation)
        .then(|| undo_double_escape(input))
        .flatten()
    {
        Some(unescaped) => split_keys(&unescaped),
        None => parts,
    }
}

/// `input` with one level of backslash escaping undone — when it is plainly
/// escaped twice: it holds no control character of its own (a real newline
/// means the model escaped correctly) and it **ends on an escaped control
/// character** (`\n`, `\r`, `\t`, `\e`, `\x03`, `\u0003` — the Enter or key
/// it meant to press last). `None` otherwise, so `print("a\nb")` and
/// `C:\\` are typed exactly as written. Escapes it does not know
/// (`\d`) are kept, backslash and all.
fn undo_double_escape(input: &str) -> Option<String> {
    if input.chars().any(char::is_control) {
        return None;
    }
    let mut out = String::with_capacity(input.len());
    let mut ends_on_control = false;
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            ends_on_control = false;
            continue;
        }
        let escaped = match chars.peek() {
            Some('n') => Some('\n'),
            Some('r') => Some('\r'),
            Some('t') => Some('\t'),
            Some('e') => Some('\u{1b}'),
            Some('\\') => Some('\\'),
            Some('"') => Some('"'),
            Some('\'') => Some('\''),
            Some('x') => hex_escape(&mut chars.clone().skip(1), 2),
            Some('u') => hex_escape(&mut chars.clone().skip(1), 4),
            _ => None,
        };
        match escaped {
            Some(e) => {
                // Consume the escape's letter and, for `\x`/`\u`, its digits.
                let digits = match chars.next() {
                    Some('x') => 2,
                    Some('u') => 4,
                    _ => 0,
                };
                for _ in 0..digits {
                    chars.next();
                }
                out.push(e);
                ends_on_control = e.is_control();
            }
            None => {
                out.push('\\');
                ends_on_control = false;
            }
        }
    }
    ends_on_control.then_some(out)
}

/// The character `digits` hex digits from `chars` spell — `None` unless all
/// of them are there.
fn hex_escape(chars: &mut impl Iterator<Item = char>, digits: usize) -> Option<char> {
    let hex: String = chars.take(digits).collect();
    if hex.len() != digits || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    char::from_u32(u32::from_str_radix(&hex, 16).ok()?)
}

/// [`parse_input`]'s notation pass: text runs split around named keys.
fn split_keys(input: &str) -> Vec<InputPart> {
    let mut parts = Vec::new();
    let mut text = String::new();
    let mut rest = input;
    while let Some(open) = rest.find('<') {
        text.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let named = after
            .find('>')
            .filter(|&close| close <= MAX_KEY_NAME)
            .and_then(|close| Some((close, lookup(&after[..close])?)));
        match named {
            Some((close, Named::Literal(c))) => {
                text.push(c);
                rest = &after[close + 1..];
            }
            Some((close, Named::Key(key))) => {
                if !text.is_empty() {
                    parts.push(InputPart::Text(std::mem::take(&mut text)));
                }
                parts.push(InputPart::Key(key));
                rest = &after[close + 1..];
            }
            None => {
                text.push('<');
                rest = after;
            }
        }
    }
    text.push_str(rest);
    if !text.is_empty() {
        parts.push(InputPart::Text(text));
    }
    parts
}

/// What a bracketed name stands for: a key, or (`<lt>`) a character typed
/// as itself.
enum Named {
    Key(Key),
    Literal(char),
}

/// The key a bracketed `name` spells, case-insensitively — `None` for text
/// that names nothing (and is therefore typed as written).
fn lookup(name: &str) -> Option<Named> {
    let lower = name.to_ascii_lowercase();
    let key = match lower.as_str() {
        "lt" => return Some(Named::Literal('<')),
        "enter" | "return" | "cr" | "ret" => Key::Enter,
        "tab" => Key::Tab,
        "s-tab" | "btab" | "backtab" => Key::BackTab,
        "esc" | "escape" => Key::Esc,
        "space" | "spc" => Key::Space,
        "bs" | "backspace" | "bspace" => Key::Backspace,
        "del" | "delete" => Key::Delete,
        "ins" | "insert" => Key::Insert,
        "up" => Key::Up,
        "down" => Key::Down,
        "left" => Key::Left,
        "right" => Key::Right,
        "home" => Key::Home,
        "end" => Key::End,
        "pageup" | "pgup" | "ppage" => Key::PageUp,
        "pagedown" | "pgdn" | "npage" => Key::PageDown,
        _ => return modified(name, &lower).map(Named::Key),
    };
    Some(Named::Key(key))
}

/// The keys with a modifier or a number in their name: `F1`–`F12`,
/// `C-x`/`Ctrl-x`/`Ctrl+x`/`^x`, and `M-x`/`Alt-x`/`A-x`/`Meta-x`. `name` is
/// the original spelling — an Alt key keeps its character's case — and
/// `lower` its lowercase form, which everything else matches on.
fn modified(name: &str, lower: &str) -> Option<Key> {
    if let Some(number) = lower.strip_prefix('f') {
        let n: u8 = number.parse().ok()?;
        return (1..=12).contains(&n).then_some(Key::F(n));
    }
    for prefix in ["c-", "ctrl-", "ctrl+", "^"] {
        if let Some(rest) = lower.strip_prefix(prefix) {
            return control_byte(rest).map(Key::Ctrl);
        }
    }
    for prefix in ["m-", "alt-", "alt+", "a-", "meta-"] {
        if lower.starts_with(prefix) {
            let mut chars = name[prefix.len()..].chars();
            let c = chars.next()?;
            return chars.next().is_none().then_some(Key::Alt(c));
        }
    }
    None
}

/// The control byte `Ctrl+{rest}` produces — a letter (either case), one of
/// `@[\]^_?`, or `space`.
fn control_byte(rest: &str) -> Option<u8> {
    if rest == "space" {
        return Some(0x00);
    }
    let mut chars = rest.chars();
    let c = chars.next()?;
    if chars.next().is_some() {
        return None;
    }
    match c {
        'a'..='z' => Some(c as u8 - b'a' + 1),
        '@' => Some(0x00),
        '[' => Some(0x1b),
        '\\' => Some(0x1c),
        ']' => Some(0x1d),
        '^' => Some(0x1e),
        '_' => Some(0x1f),
        '?' => Some(0x7f),
        _ => None,
    }
}

/// The bytes a terminal sends for `parts`, grouped into chunks at the points
/// the writer must pause (after a lone `Esc`). On a terminal Enter is `\r`, so
/// a newline in typed text is sent as `\r` too (`\r\n` as one `\r`) — what
/// cooked mode turns back into `\n`, and what raw-mode programs (menus,
/// editors) expect. `app_cursor` is the program's cursor-key mode (DECCKM):
/// arrows and Home/End are `ESC O x` under it, `ESC [ x` otherwise.
#[must_use]
pub fn encode(parts: &[InputPart], app_cursor: bool) -> Vec<InputChunk> {
    let mut chunks = Vec::new();
    let mut bytes = Vec::new();
    for (index, part) in parts.iter().enumerate() {
        match part {
            InputPart::Text(text) => push_text(&mut bytes, text),
            InputPart::Key(key) => {
                bytes.extend_from_slice(&key_bytes(*key, app_cursor));
                if *key == Key::Esc && index + 1 < parts.len() {
                    chunks.push(InputChunk {
                        bytes: std::mem::take(&mut bytes),
                        pause_after: true,
                    });
                }
            }
        }
    }
    if !bytes.is_empty() {
        chunks.push(InputChunk {
            bytes,
            pause_after: false,
        });
    }
    chunks
}

/// Typed text as terminal bytes: every line ending (`\n`, `\r\n`, a lone
/// `\r`) becomes the Enter key's `\r`.
fn push_text(bytes: &mut Vec<u8>, text: &str) {
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                bytes.push(b'\r');
            }
            '\n' => bytes.push(b'\r'),
            c => {
                let mut buf = [0u8; 4];
                bytes.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            }
        }
    }
}

/// The bytes one key sends (xterm's encoding).
fn key_bytes(key: Key, app_cursor: bool) -> Vec<u8> {
    let cursor = |c: u8| -> Vec<u8> {
        if app_cursor {
            vec![0x1b, b'O', c]
        } else {
            vec![0x1b, b'[', c]
        }
    };
    match key {
        Key::Enter => b"\r".to_vec(),
        Key::Tab => b"\t".to_vec(),
        Key::BackTab => b"\x1b[Z".to_vec(),
        Key::Esc => b"\x1b".to_vec(),
        Key::Space => b" ".to_vec(),
        Key::Backspace => b"\x7f".to_vec(),
        Key::Delete => b"\x1b[3~".to_vec(),
        Key::Insert => b"\x1b[2~".to_vec(),
        Key::Up => cursor(b'A'),
        Key::Down => cursor(b'B'),
        Key::Right => cursor(b'C'),
        Key::Left => cursor(b'D'),
        Key::Home => cursor(b'H'),
        Key::End => cursor(b'F'),
        Key::PageUp => b"\x1b[5~".to_vec(),
        Key::PageDown => b"\x1b[6~".to_vec(),
        Key::F(n) => match n {
            1 => b"\x1bOP".to_vec(),
            2 => b"\x1bOQ".to_vec(),
            3 => b"\x1bOR".to_vec(),
            4 => b"\x1bOS".to_vec(),
            n => {
                // F5 is 15; F6–F10 are 17–21; F11/F12 are 23/24 — xterm's
                // gaps, kept from the VT220 keyboard.
                let code = match n {
                    5 => 15,
                    6..=10 => n + 11,
                    _ => n + 12,
                };
                format!("\x1b[{code}~").into_bytes()
            }
        },
        Key::Ctrl(byte) => vec![byte],
        Key::Alt(c) => {
            let mut bytes = vec![0x1b];
            let mut buf = [0u8; 4];
            bytes.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            bytes
        }
    }
}

/// Is this input exactly one `<C-c>` — an interrupt, which a shell without a
/// terminal still honours (as `SIGINT` to its process group) and which asks
/// no permission (`docs/interactive-shell.md`)?
#[must_use]
pub fn is_interrupt(parts: &[InputPart]) -> bool {
    match parts {
        [InputPart::Key(Key::Ctrl(0x03))] => true,
        [InputPart::Text(text)] => text == "\u{3}",
        _ => false,
    }
}

/// Does the input end on **typed text** — no Enter after it? In a terminal
/// that reads whole lines, such text sits in the line being edited and has
/// not reached the program: the session report says so, since a model that
/// forgot the Enter sees its answer on the screen and takes it as given.
#[must_use]
pub fn leaves_line_open(parts: &[InputPart]) -> bool {
    matches!(parts.last(), Some(InputPart::Text(text)) if !text.ends_with(['\n', '\r']))
}

/// The text left on the line being edited when the input is done — the last
/// line typed after its last Enter, trailing spaces trimmed — or `None` when
/// the input ends on a key or an Enter ([`leaves_line_open`]) or on blanks.
/// What a line editor (a REPL's readline) echoes at its prompt and holds
/// until Enter: the session report looks for it at the cursor.
#[must_use]
pub fn typed_tail(parts: &[InputPart]) -> Option<&str> {
    if !leaves_line_open(parts) {
        return None;
    }
    let Some(InputPart::Text(text)) = parts.last() else {
        return None;
    };
    let line = text.rsplit(['\n', '\r']).next().unwrap_or(text).trim_end();
    (!line.trim_start().is_empty()).then_some(line)
}

/// `input` on one line for a cell header or a permission prompt: Enter as
/// `⏎`, Tab as `⇥`, other control characters as `^X`, and every named key in
/// its canonical notation (`<Down>`, `<C-c>`, `<M-x>`).
#[must_use]
pub fn display_input(input: &str) -> String {
    let mut out = String::new();
    for part in parse_input(input) {
        match part {
            InputPart::Text(text) => {
                let mut chars = text.chars().peekable();
                while let Some(c) = chars.next() {
                    match c {
                        '\r' | '\n' => {
                            if c == '\r' && chars.peek() == Some(&'\n') {
                                chars.next();
                            }
                            out.push('⏎');
                        }
                        '\t' => out.push('⇥'),
                        '\u{7f}' => out.push_str("^?"),
                        c if (c as u32) < 0x20 => {
                            out.push('^');
                            out.push((c as u8 + b'@') as char);
                        }
                        c => out.push(c),
                    }
                }
            }
            InputPart::Key(key) => out.push_str(&key_name(key)),
        }
    }
    out
}

/// A key's canonical notation for [`display_input`] — Enter and Tab as the
/// same symbols typed text gets, so `x\n` and `x<Enter>` read alike.
fn key_name(key: Key) -> String {
    let named = |name: &str| format!("<{name}>");
    match key {
        Key::Enter => "⏎".to_string(),
        Key::Tab => "⇥".to_string(),
        Key::BackTab => named("S-Tab"),
        Key::Esc => named("Esc"),
        Key::Space => named("Space"),
        Key::Backspace => named("BS"),
        Key::Delete => named("Del"),
        Key::Insert => named("Insert"),
        Key::Up => named("Up"),
        Key::Down => named("Down"),
        Key::Left => named("Left"),
        Key::Right => named("Right"),
        Key::Home => named("Home"),
        Key::End => named("End"),
        Key::PageUp => named("PageUp"),
        Key::PageDown => named("PageDown"),
        Key::F(n) => named(&format!("F{n}")),
        Key::Ctrl(byte) => {
            let shown = match byte {
                0x01..=0x1a => ((byte - 1 + b'a') as char).to_string(),
                0x00 => "@".to_string(),
                0x7f => "?".to_string(),
                other => ((other + b'@') as char).to_string(),
            };
            named(&format!("C-{shown}"))
        }
        Key::Alt(c) => named(&format!("M-{c}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(s: &str) -> InputPart {
        InputPart::Text(s.to_string())
    }

    fn key(k: Key) -> InputPart {
        InputPart::Key(k)
    }

    fn bytes(chunks: &[InputChunk]) -> Vec<u8> {
        chunks.iter().flat_map(|c| c.bytes.clone()).collect()
    }

    #[test]
    fn plain_text_is_one_text_part() {
        assert_eq!(parse_input("print(1)"), vec![text("print(1)")]);
        assert_eq!(parse_input(""), Vec::<InputPart>::new());
    }

    #[test]
    fn named_keys_split_the_text_around_them() {
        assert_eq!(
            parse_input("y<Enter><Down><Enter>"),
            vec![text("y"), key(Key::Enter), key(Key::Down), key(Key::Enter)]
        );
    }

    #[test]
    fn key_names_are_case_insensitive_and_take_their_common_aliases() {
        for (name, expected) in [
            ("<enter>", Key::Enter),
            ("<RETURN>", Key::Enter),
            ("<CR>", Key::Enter),
            ("<Esc>", Key::Esc),
            ("<escape>", Key::Esc),
            ("<Tab>", Key::Tab),
            ("<S-Tab>", Key::BackTab),
            ("<BTab>", Key::BackTab),
            ("<Space>", Key::Space),
            ("<BS>", Key::Backspace),
            ("<Backspace>", Key::Backspace),
            ("<Del>", Key::Delete),
            ("<Delete>", Key::Delete),
            ("<Insert>", Key::Insert),
            ("<Up>", Key::Up),
            ("<down>", Key::Down),
            ("<Left>", Key::Left),
            ("<RIGHT>", Key::Right),
            ("<Home>", Key::Home),
            ("<End>", Key::End),
            ("<PageUp>", Key::PageUp),
            ("<PgUp>", Key::PageUp),
            ("<PageDown>", Key::PageDown),
            ("<PgDn>", Key::PageDown),
            ("<F1>", Key::F(1)),
            ("<f12>", Key::F(12)),
        ] {
            assert_eq!(parse_input(name), vec![key(expected)], "{name}");
        }
    }

    #[test]
    fn control_keys_take_every_common_spelling() {
        for name in ["<C-c>", "<c-C>", "<Ctrl-c>", "<Ctrl+C>", "<CTRL-c>", "<^C>"] {
            assert_eq!(parse_input(name), vec![key(Key::Ctrl(0x03))], "{name}");
        }
        assert_eq!(parse_input("<C-d>"), vec![key(Key::Ctrl(0x04))]);
        assert_eq!(parse_input("<C-z>"), vec![key(Key::Ctrl(0x1a))]);
        assert_eq!(parse_input("<C-@>"), vec![key(Key::Ctrl(0x00))]);
        assert_eq!(parse_input("<C-Space>"), vec![key(Key::Ctrl(0x00))]);
        assert_eq!(parse_input("<C-[>"), vec![key(Key::Ctrl(0x1b))]);
        assert_eq!(parse_input("<C-\\>"), vec![key(Key::Ctrl(0x1c))]);
        assert_eq!(parse_input("<C-]>"), vec![key(Key::Ctrl(0x1d))]);
        assert_eq!(parse_input("<C-?>"), vec![key(Key::Ctrl(0x7f))]);
    }

    #[test]
    fn alt_keys_are_esc_prefixed_characters() {
        for name in ["<M-x>", "<Alt-x>", "<alt+x>", "<A-x>", "<Meta-x>"] {
            assert_eq!(parse_input(name), vec![key(Key::Alt('x'))], "{name}");
        }
        // Case is the character's own here — Alt+X is not Alt+x.
        assert_eq!(parse_input("<M-X>"), vec![key(Key::Alt('X'))]);
    }

    #[test]
    fn angle_brackets_that_name_no_key_are_typed_as_written() {
        assert_eq!(parse_input("a<b"), vec![text("a<b")]);
        assert_eq!(parse_input("<div>hi</div>"), vec![text("<div>hi</div>")]);
        assert_eq!(parse_input("x < y > z"), vec![text("x < y > z")]);
        assert_eq!(
            parse_input("if a<b and c>d:"),
            vec![text("if a<b and c>d:")]
        );
        assert_eq!(parse_input("<>"), vec![text("<>")]);
        assert_eq!(parse_input("<<Enter>"), vec![text("<"), key(Key::Enter)]);
        assert_eq!(parse_input("<C-cc>"), vec![text("<C-cc>")]);
        assert_eq!(parse_input("<F13>"), vec![text("<F13>")]);
    }

    #[test]
    fn lt_is_a_literal_open_bracket() {
        assert_eq!(
            parse_input("<lt>Enter>"),
            vec![text("<Enter>")],
            "the one way to type a key name literally"
        );
    }

    #[test]
    fn input_escaped_twice_is_read_as_the_keys_it_spells() {
        // `"Ada Lovelace\\n"` in the JSON: the model escaped its own
        // escape, and the program would get a backslash, an `n`, and no
        // Enter — the input ending on an escaped control character is what
        // says so.
        assert_eq!(parse_input(r"Ada Lovelace\n"), vec![text("Ada Lovelace\n")]);
        assert_eq!(parse_input(r"y\r"), vec![text("y\r")]);
        assert_eq!(parse_input(r"1\n2\n"), vec![text("1\n2\n")]);
        assert_eq!(parse_input(r"ls /usr/bi\t"), vec![text("ls /usr/bi\t")]);
        assert_eq!(parse_input(r"\x03"), vec![text("\u{3}")]);
        assert_eq!(parse_input(r"\u0003"), vec![text("\u{3}")]);
        assert_eq!(parse_input(r"\e"), vec![text("\u{1b}")]);
        // One level only: what the model escaped *inside* its text stays
        // escaped, as it meant it.
        assert_eq!(
            parse_input(r#"print(\"a\\nb\")\n"#),
            vec![text("print(\"a\\nb\")\n")]
        );
        assert_eq!(parse_input(r"a\qb\n"), vec![text("a\\qb\n")], "unknown");
    }

    #[test]
    fn a_backslash_that_is_not_a_doubled_escape_is_typed_as_written() {
        // Nothing escaped at the end: a backslash the model meant.
        assert_eq!(
            parse_input(r#"print("a\nb")"#),
            vec![text(r#"print("a\nb")"#)]
        );
        assert_eq!(parse_input(r"C:\\"), vec![text(r"C:\\")]);
        assert_eq!(parse_input(r"trailing\"), vec![text(r"trailing\")]);
        // A real newline, or a named key, means the model used the notation.
        assert_eq!(parse_input("a\\nb\n"), vec![text("a\\nb\n")]);
        assert_eq!(
            parse_input(r"x\n<Enter>"),
            vec![text(r"x\n"), key(Key::Enter)]
        );
    }

    #[test]
    fn a_newline_in_text_is_sent_as_the_enter_key_byte() {
        let chunks = encode(&parse_input("print(1)\n"), false);
        assert_eq!(bytes(&chunks), b"print(1)\r");
        // CRLF is one Enter, not two.
        let chunks = encode(&parse_input("a\r\nb\n"), false);
        assert_eq!(bytes(&chunks), b"a\rb\r");
    }

    #[test]
    fn keys_encode_to_the_bytes_a_terminal_sends() {
        let cases: Vec<(Key, &[u8])> = vec![
            (Key::Enter, b"\r"),
            (Key::Tab, b"\t"),
            (Key::BackTab, b"\x1b[Z"),
            (Key::Space, b" "),
            (Key::Backspace, b"\x7f"),
            (Key::Delete, b"\x1b[3~"),
            (Key::Insert, b"\x1b[2~"),
            (Key::Up, b"\x1b[A"),
            (Key::Down, b"\x1b[B"),
            (Key::Right, b"\x1b[C"),
            (Key::Left, b"\x1b[D"),
            (Key::Home, b"\x1b[H"),
            (Key::End, b"\x1b[F"),
            (Key::PageUp, b"\x1b[5~"),
            (Key::PageDown, b"\x1b[6~"),
            (Key::F(1), b"\x1bOP"),
            (Key::F(4), b"\x1bOS"),
            (Key::F(5), b"\x1b[15~"),
            (Key::F(12), b"\x1b[24~"),
            (Key::Ctrl(0x03), b"\x03"),
            (Key::Alt('x'), b"\x1bx"),
        ];
        for (k, expected) in cases {
            assert_eq!(bytes(&encode(&[key(k)], false)), expected, "{k:?}");
        }
    }

    #[test]
    fn application_cursor_mode_changes_the_arrows_and_home_end() {
        for (k, expected) in [
            (Key::Up, b"\x1bOA"),
            (Key::Down, b"\x1bOB"),
            (Key::Right, b"\x1bOC"),
            (Key::Left, b"\x1bOD"),
            (Key::Home, b"\x1bOH"),
            (Key::End, b"\x1bOF"),
        ] {
            assert_eq!(bytes(&encode(&[key(k)], true)), expected, "{k:?}");
        }
        // Everything else is the same in both modes.
        assert_eq!(bytes(&encode(&[key(Key::Enter)], true)), b"\r");
    }

    #[test]
    fn everything_up_to_a_pause_is_written_in_one_chunk() {
        let chunks = encode(&parse_input("ab<Down><Down><Enter>"), false);
        assert_eq!(
            chunks,
            vec![InputChunk {
                bytes: b"ab\x1b[B\x1b[B\r".to_vec(),
                pause_after: false,
            }]
        );
    }

    #[test]
    fn a_lone_esc_followed_by_more_input_pauses_after_itself() {
        // Vim reads `ESC :` inside its timeout as Alt+: — the pause makes it
        // a key press followed by a command-line colon.
        let chunks = encode(&parse_input("hello<Esc>:wq<Enter>"), false);
        assert_eq!(
            chunks,
            vec![
                InputChunk {
                    bytes: b"hello\x1b".to_vec(),
                    pause_after: true,
                },
                InputChunk {
                    bytes: b":wq\r".to_vec(),
                    pause_after: false,
                },
            ]
        );
        // A trailing Esc has nothing to be confused with.
        let chunks = encode(&parse_input("x<Esc>"), false);
        assert_eq!(
            chunks,
            vec![InputChunk {
                bytes: b"x\x1b".to_vec(),
                pause_after: false,
            }]
        );
    }

    #[test]
    fn empty_input_encodes_to_nothing() {
        assert!(encode(&[], false).is_empty());
    }

    #[test]
    fn only_a_lone_ctrl_c_is_an_interrupt() {
        assert!(is_interrupt(&parse_input("<C-c>")));
        assert!(is_interrupt(&parse_input("\u{3}")), "the raw byte too");
        assert!(!is_interrupt(&parse_input("<C-c><Enter>")));
        assert!(!is_interrupt(&parse_input("<C-d>")));
        assert!(!is_interrupt(&parse_input("")));
        assert!(!is_interrupt(&parse_input("q")));
    }

    #[test]
    fn input_that_ends_on_typed_text_leaves_its_line_unsubmitted() {
        assert!(leaves_line_open(&parse_input("Ada Lovelace")));
        assert!(leaves_line_open(&parse_input("<Up>ls -la")));
        assert!(!leaves_line_open(&parse_input("Ada Lovelace\n")));
        assert!(!leaves_line_open(&parse_input("y\r")));
        assert!(!leaves_line_open(&parse_input("y<Enter>")));
        assert!(!leaves_line_open(&parse_input(r"y\n")), "the slip undone");
        // A key is its own action — a menu's arrow, an interrupt.
        assert!(!leaves_line_open(&parse_input("<Down>")));
        assert!(!leaves_line_open(&parse_input("abc<C-d>")));
        assert!(!leaves_line_open(&parse_input("")));
    }

    #[test]
    fn the_typed_tail_is_the_last_line_typed_after_the_last_enter() {
        assert_eq!(typed_tail(&parse_input("abc")), Some("abc"));
        assert_eq!(
            typed_tail(&parse_input("def f():\n    x\n\nprint(1)")),
            Some("print(1)"),
            "only what sits on the line being edited"
        );
        assert_eq!(typed_tail(&parse_input("abc   ")), Some("abc"));
        assert_eq!(typed_tail(&parse_input("x<Enter>")), None);
        assert_eq!(typed_tail(&parse_input("<Up>")), None);
        assert_eq!(typed_tail(&parse_input("   ")), None, "nothing to see");
    }

    #[test]
    fn display_puts_the_input_on_one_readable_line() {
        assert_eq!(display_input("print(1)\n"), "print(1)⏎");
        assert_eq!(display_input("a\tb"), "a⇥b");
        assert_eq!(display_input("y<Enter><down>"), "y⏎<Down>");
        assert_eq!(display_input("<ctrl+c>"), "<C-c>");
        assert_eq!(display_input("<Alt-x>"), "<M-x>");
        assert_eq!(display_input("\u{3}"), "^C");
        assert_eq!(display_input("<Esc>:wq<CR>"), "<Esc>:wq⏎");
        assert_eq!(display_input("<S-Tab><F5><PgDn>"), "<S-Tab><F5><PageDown>");
        assert_eq!(display_input("a<b"), "a<b");
        assert_eq!(display_input("<lt>x>"), "<x>");
    }
}
