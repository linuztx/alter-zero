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
//! How the keys are **written** matters as much as which bytes they are: a
//! person's key presses never arrive together, and some programs take
//! whatever one read returns as one key press — btop ignored `<Down><Down>`
//! written at once, and dropped `bash` typed into its filter in one write.
//! So every named key is its own write, and short text typed to a program
//! reading key by key goes a character at a time, each write read by the
//! program before the next goes ([`Pace::Read`]); a paste, a line for a
//! program reading whole lines, and long text go whole.
//!
//! Pure: the terminal state encoding depends on ([`Modes`]) is passed in.

use std::time::Duration;

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
    /// A special key held with modifiers — xterm's bits, Shift 1, Alt 2,
    /// Ctrl 4 — sent as the key's sequence with them as a parameter:
    /// `<C-Left>` is `ESC [ 1 ; 5 D`, `<M-F7>` is `ESC [ 18 ; 3 ~`.
    Modified(Special, u8),
}

/// A key that sends a sequence of its own, which a modifier changes
/// ([`Key::Modified`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Special {
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    Delete,
    /// `F1`–`F12`.
    F(u8),
}

impl Special {
    /// The number and the final character of the key's CSI sequence, as
    /// xterm writes it with modifiers.
    fn sequence(self) -> (u8, char) {
        match self {
            Self::Up => (1, 'A'),
            Self::Down => (1, 'B'),
            Self::Right => (1, 'C'),
            Self::Left => (1, 'D'),
            Self::Home => (1, 'H'),
            Self::End => (1, 'F'),
            Self::Insert => (2, '~'),
            Self::Delete => (3, '~'),
            Self::PageUp => (5, '~'),
            Self::PageDown => (6, '~'),
            Self::F(n @ 1..=4) => (1, char::from(b'O' + n)),
            Self::F(n) => (function_code(n), '~'),
        }
    }

    /// The key's name in the notation.
    fn name(self) -> String {
        match self {
            Self::Up => "Up".to_string(),
            Self::Down => "Down".to_string(),
            Self::Left => "Left".to_string(),
            Self::Right => "Right".to_string(),
            Self::Home => "Home".to_string(),
            Self::End => "End".to_string(),
            Self::PageUp => "PageUp".to_string(),
            Self::PageDown => "PageDown".to_string(),
            Self::Insert => "Insert".to_string(),
            Self::Delete => "Del".to_string(),
            Self::F(n) => format!("F{n}"),
        }
    }
}

/// The number in F5–F12's sequences (`ESC [ n ~`): F5 is 15, F6–F10 are
/// 17–21, F11/F12 are 23/24 — xterm's gaps, kept from the VT220 keyboard.
fn function_code(n: u8) -> u8 {
    match n {
        5 => 15,
        6..=10 => n + 11,
        _ => n + 12,
    }
}

/// One run of the input: text typed as written, or one named key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputPart {
    Text(String),
    Key(Key),
}

/// Bytes to write in one go, and what the writer waits for after them
/// before it writes what follows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputChunk {
    pub bytes: Vec<u8>,
    pub then: Pace,
}

/// What the writer waits for between one write and the next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pace {
    /// Nothing: no write follows.
    Last,
    /// The program has **read** the write — a key press of its own, as a
    /// person's keys never arrive together. The writer asks the terminal's
    /// input queue, waiting at most [`KEY_READ_WAIT`]; where the queue
    /// cannot answer for the program — it cannot be asked, or a relay (sudo,
    /// ssh, `docker exec -it`) takes each key the moment it lands and hands
    /// it on to a terminal of its own — at least [`KEY_PAUSE`] passes too.
    Read,
    /// A lone `Esc` that more input follows: read, then [`ESC_PAUSE`].
    Esc,
}

/// The least time between two writes where the terminal cannot say the
/// program read the first ([`Pace::Read`]) — long enough for a program to
/// have woken and read it (a millisecond or two) before the second arrives.
pub const KEY_PAUSE: Duration = Duration::from_millis(20);

/// The longest the writer waits for the program to read a write before it
/// sends the next ([`Pace::Read`]). A program may take a while over a key —
/// btop filters and redraws its process list on each one, a few hundred
/// milliseconds on a busy machine — but one that has read nothing in this
/// long is not reading: the rest of the keys go as typeahead, each
/// [`KEY_PAUSE`] after the last.
pub const KEY_READ_WAIT: Duration = Duration::from_secs(1);

/// The pause after a lone `Esc` that more input follows — past Vim's
/// `ttimeoutlen` (100 ms in `defaults.vim`), so `<Esc>:` reads as a key
/// press and then a colon, not Alt+`:`.
pub const ESC_PAUSE: Duration = Duration::from_millis(150);

/// The longest text typed a character at a time to a program reading key by
/// key — a search, a filter, a command line. Longer text (code typed into an
/// editor) is input rather than a key press to any program that takes that
/// much, and a character at a time it could cost seconds.
pub const TYPED_TEXT_MAX: usize = 64;

/// The most writing `chunks` can take: every wait the writer may make
/// between them ([`Pace`]). The writer says when it is done, which is
/// nearly always far sooner.
#[must_use]
pub fn typing_bound(chunks: &[InputChunk]) -> Duration {
    chunks
        .iter()
        .map(|chunk| match chunk.then {
            Pace::Last => Duration::ZERO,
            Pace::Read => KEY_READ_WAIT + KEY_PAUSE,
            Pace::Esc => KEY_READ_WAIT + KEY_PAUSE + ESC_PAUSE,
        })
        .sum()
}

/// The longest text between angle brackets that can still be a key name —
/// `<PageDown>`, `<Ctrl+Shift+PageDown>` — so a `<` far from any `>` is
/// settled at a glance rather than by scanning the rest of the input.
const MAX_KEY_NAME: usize = 24;

/// Split `input` into text runs and named keys (see the module docs).
///
/// Input **escaped twice** is read as the keys it spells: a model that writes
/// `"yes\\n"` into its JSON — a backslash and an `n` where it meant Enter, a
/// slip several models make — has the one level of escaping undone, so the
/// program gets its answer instead of a backslash and a line never submitted.
/// Only input that plainly slipped qualifies (see `undo_double_escape`); a
/// backslash anywhere else is typed as written. So does input **HTML-escaped**
/// ([`html_escaped`]): `&lt;Esc&gt;` is `<Esc>`, and the rest of that input
/// is unescaped with it.
#[must_use]
pub fn parse_input(input: &str) -> Vec<InputPart> {
    // Input HTML-escaped whole is read as what it escapes, once.
    let unescaped;
    let input = if html_escaped(input) {
        unescaped = unescape_html(input);
        unescaped.as_str()
    } else {
        input
    };
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

/// Did a model **HTML-escape** its input — `&lt;Esc&gt;` for `<Esc>`, as
/// one deployment did to every `<` and `>` in its tool arguments? Only an
/// escaped **key name** says so: `&lt;div&gt;` and `a &lt; b` are what an
/// author typing HTML into an editor means.
#[must_use]
pub fn html_escaped(input: &str) -> bool {
    let mut rest = input;
    while let Some(open) = rest.find("&lt;") {
        let after = &rest[open + "&lt;".len()..];
        if let Some(close) = after.find("&gt;").filter(|&close| close <= MAX_KEY_NAME)
            && matches!(lookup(&after[..close]), Some(Named::Key(_)))
        {
            return true;
        }
        rest = after;
    }
    false
}

/// `input` with one level of HTML escaping undone — `&amp;` last, so
/// `&amp;lt;` is left `&lt;`.
fn unescape_html(input: &str) -> String {
    input
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
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

/// Modifier bits as xterm numbers them in a key's sequence (`1 +` these).
const SHIFT: u8 = 1;
const ALT: u8 = 2;
const CTRL: u8 = 4;

/// How a modifier is written in front of a key name.
const MODIFIER_PREFIXES: [(&str, u8); 13] = [
    ("shift+", SHIFT),
    ("shift-", SHIFT),
    ("s-", SHIFT),
    ("ctrl+", CTRL),
    ("ctrl-", CTRL),
    ("c-", CTRL),
    ("^", CTRL),
    ("meta+", ALT),
    ("meta-", ALT),
    ("alt+", ALT),
    ("alt-", ALT),
    ("a-", ALT),
    ("m-", ALT),
];

/// The keys with a number or modifiers in their name: `F1`–`F12`, and any
/// key or character behind modifier prefixes — `C-x`/`Ctrl-x`/`Ctrl+x`/`^x`,
/// `M-x`/`Alt-x`/`A-x`/`Meta-x`, `S-Up`/`Shift+Up`, combined in any order
/// (`<C-S-Left>`). `name` is the original spelling — an Alt key keeps its
/// character's case — and `lower` its lowercase form, which everything else
/// matches on.
fn modified(name: &str, lower: &str) -> Option<Key> {
    if let Some(n) = function_number(lower) {
        return Some(Key::F(n));
    }
    let mut mods = 0;
    let mut at = 0;
    'strip: loop {
        for (prefix, bit) in MODIFIER_PREFIXES {
            let rest = &lower[at..];
            if rest.len() > prefix.len() && rest.starts_with(prefix) {
                mods |= bit;
                at += prefix.len();
                continue 'strip;
            }
        }
        break;
    }
    if mods == 0 {
        return None;
    }
    let base = &lower[at..];
    if let Some(special) = special_key(base) {
        return Some(Key::Modified(special, mods));
    }
    // A key that sends one byte: Alt puts ESC in front of it, and Shift or
    // Ctrl adds nothing a terminal sends — but Ctrl+Backspace is `^H`,
    // Ctrl+Space `^@`, and Shift+Tab has a sequence of its own.
    let byte = match base {
        "enter" | "return" | "cr" | "ret" => Some('\r'),
        "tab" => Some('\t'),
        "bs" | "backspace" | "bspace" => Some(if mods & CTRL == 0 { '\u{7f}' } else { '\u{8}' }),
        "esc" | "escape" => Some('\u{1b}'),
        "space" | "spc" => Some(if mods & CTRL == 0 { ' ' } else { '\0' }),
        _ => None,
    };
    if let Some(c) = byte {
        return Some(match c {
            _ if mods & ALT != 0 => Key::Alt(c),
            '\t' if mods & SHIFT != 0 => Key::BackTab,
            '\r' => Key::Enter,
            '\t' => Key::Tab,
            '\u{7f}' => Key::Backspace,
            '\u{1b}' => Key::Esc,
            ' ' => Key::Space,
            other => Key::Ctrl(other as u8),
        });
    }
    // A character: Shift makes it a capital, Ctrl a control byte, and Alt
    // puts ESC in front — Shift alone is just the character, typed as such.
    let mut chars = name[at..].chars();
    let c = chars.next()?;
    if chars.next().is_some() || mods == SHIFT {
        return None;
    }
    let c = if mods & SHIFT == 0 {
        c
    } else {
        c.to_ascii_uppercase()
    };
    let c = if mods & CTRL == 0 {
        c
    } else {
        char::from(control_byte(&c.to_ascii_lowercase().to_string())?)
    };
    Some(if mods & ALT != 0 {
        Key::Alt(c)
    } else {
        Key::Ctrl(c as u8)
    })
}

/// `F1`–`F12`'s number, from the lowercase name.
fn function_number(lower: &str) -> Option<u8> {
    let n: u8 = lower.strip_prefix('f')?.parse().ok()?;
    (1..=12).contains(&n).then_some(n)
}

/// The special key a lowercase `name` spells, for a modifier to hold.
fn special_key(name: &str) -> Option<Special> {
    Some(match name {
        "up" => Special::Up,
        "down" => Special::Down,
        "left" => Special::Left,
        "right" => Special::Right,
        "home" => Special::Home,
        "end" => Special::End,
        "pageup" | "pgup" | "ppage" => Special::PageUp,
        "pagedown" | "pgdn" | "npage" => Special::PageDown,
        "ins" | "insert" => Special::Insert,
        "del" | "delete" => Special::Delete,
        _ => return function_number(name).map(Special::F),
    })
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

/// The terminal modes a program sets that change what its keys look like
/// ([`encode`]) — read off the emulated screen.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Modes {
    /// Cursor-key mode (DECCKM): arrows and Home/End as `ESC O x`.
    pub app_cursor: bool,
    /// Bracketed paste (mode 2004): the program tells pasted text from
    /// typed keys.
    pub bracketed_paste: bool,
    /// The alternate screen: a full-screen program, which may read its keys
    /// a read at a time.
    pub full_screen: bool,
    /// Canonical mode is off: the program reads the terminal a key at a time
    /// — a menu, `top`, a line editor — or a relay does, for one.
    pub key_by_key: bool,
    /// Who reads what is typed, as far as the session can tell.
    pub reader: Reader,
}

/// The program reading the terminal — its foreground program — as far as
/// it decides how text reaches it ([`encode`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Reader {
    /// No one named it: no process table to ask, or a relay (sudo, ssh,
    /// `docker exec -it`) whose far end may run anything.
    #[default]
    Unknown,
    /// A shell: it runs each line it is typed, and a line that starts a
    /// program leaves the lines after it for that program to read.
    Shell,
    /// Any other program — an editor, a REPL, a menu.
    Program,
}

impl Reader {
    /// The reader a foreground program's name (`/proc/PID/comm`) makes.
    #[must_use]
    pub fn of(program: &str) -> Self {
        const SHELLS: [&str; 20] = [
            "sh", "bash", "dash", "zsh", "fish", "ksh", "ksh93", "mksh", "pdksh", "oksh", "yash",
            "ash", "busybox", "tcsh", "csh", "nu", "elvish", "xonsh", "pwsh", "osh",
        ];
        const RELAYS: [&str; 22] = [
            "sudo",
            "su",
            "doas",
            "run0",
            "pkexec",
            "ssh",
            "mosh-client",
            "telnet",
            "docker",
            "podman",
            "nerdctl",
            "kubectl",
            "oc",
            "lxc",
            "incus",
            "machinectl",
            "systemd-run",
            "script",
            "tmux",
            "screen",
            "socat",
            "expect",
        ];
        if program.is_empty() || RELAYS.contains(&program) {
            Self::Unknown
        } else if SHELLS.contains(&program) {
            Self::Shell
        } else {
            Self::Program
        }
    }
}

/// The bytes a terminal sends for `parts`, as the writes to send them in
/// (see the module docs). On a terminal Enter is `\r`, so a newline in typed
/// text is sent as `\r` too (`\r\n` as one `\r`) — what cooked mode turns
/// back into `\n`, and what raw-mode programs (menus, editors) expect.
/// `modes` are the program's: under cursor-key mode (DECCKM) arrows and
/// Home/End are `ESC O x`, `ESC [ x` otherwise; under bracketed paste, text
/// running onto indented lines goes as a paste (`paste_split`) — never to a
/// shell, and to a reader no one named only on the alternate screen; to a
/// program reading key by key, or on the alternate screen, text up to
/// [`TYPED_TEXT_MAX`] goes a character at a time.
#[must_use]
pub fn encode(parts: &[InputPart], modes: Modes) -> Vec<InputChunk> {
    let app_cursor = modes.app_cursor;
    // A paste goes to a program that inserts it: never a shell, which runs
    // the lines it is typed; and to a reader no one named, only an editor's
    // full screen.
    let pastes = modes.bracketed_paste
        && match modes.reader {
            Reader::Shell => false,
            Reader::Program => true,
            Reader::Unknown => modes.full_screen,
        };
    // Lines parted by `<Enter>` keys are lines like any other: the same
    // byte, and — to a program that takes pastes — the same paste.
    let merged;
    let parts = if pastes {
        merged = enters_as_newlines(parts);
        merged.as_slice()
    } else {
        parts
    };
    let mut chunks = Vec::new();
    let mut write = |bytes: Vec<u8>, then: Pace| {
        if !bytes.is_empty() {
            chunks.push(InputChunk { bytes, then });
        }
    };
    for part in parts {
        match part {
            InputPart::Text(text) => {
                let mut bytes = Vec::new();
                if let Some((first, pasted, enter)) = paste_split(text).filter(|_| pastes) {
                    push_text(&mut bytes, first);
                    bytes.extend_from_slice(PASTE_START);
                    // A paste cannot end itself early.
                    push_text(&mut bytes, &pasted.replace("\u{1b}[201~", ""));
                    bytes.extend_from_slice(PASTE_END);
                    if enter {
                        bytes.push(b'\r');
                    }
                    write(bytes, Pace::Read);
                } else if (modes.full_screen || modes.key_by_key)
                    && text.chars().count() <= TYPED_TEXT_MAX
                {
                    push_text(&mut bytes, text);
                    for c in String::from_utf8_lossy(&bytes).chars() {
                        let mut buf = [0u8; 4];
                        write(c.encode_utf8(&mut buf).as_bytes().to_vec(), Pace::Read);
                    }
                } else {
                    push_text(&mut bytes, text);
                    write(bytes, Pace::Read);
                }
            }
            InputPart::Key(key) => {
                let then = if *key == Key::Esc {
                    Pace::Esc
                } else {
                    Pace::Read
                };
                write(key_bytes(*key, app_cursor), then);
            }
        }
    }
    // Nothing follows the last write to keep apart from it.
    if let Some(last) = chunks.last_mut() {
        last.then = Pace::Last;
    }
    chunks
}

/// `parts` with every `<Enter>` folded into the text around it as a newline
/// — which [`encode`] sends as the same `\r` — so text written as
/// `def f():<Enter>    return 1<Enter>` is seen as the lines it is.
fn enters_as_newlines(parts: &[InputPart]) -> Vec<InputPart> {
    let mut out = Vec::with_capacity(parts.len());
    let mut text = String::new();
    for part in parts {
        match part {
            InputPart::Text(run) => text.push_str(run),
            InputPart::Key(Key::Enter) => text.push('\n'),
            InputPart::Key(key) => {
                if !text.is_empty() {
                    out.push(InputPart::Text(std::mem::take(&mut text)));
                }
                out.push(InputPart::Key(*key));
            }
        }
    }
    if !text.is_empty() {
        out.push(InputPart::Text(text));
    }
    out
}

/// What a terminal sends around pasted text to a program in bracketed-paste
/// mode (2004).
const PASTE_START: &[u8] = b"\x1b[200~";
/// See [`PASTE_START`].
const PASTE_END: &[u8] = b"\x1b[201~";

/// How text with indented lines reaches a program that takes pastes: the
/// first line typed, the lines after it as one paste, and whether a line
/// break ended the text — sent after the paste as the Enter key. `None` for
/// text to type as it is: one line, or no indented line after the first.
///
/// Why a paste: an editor's or a REPL's auto-indent adds its own indent to
/// every line typed after a line break, so typed code comes out as a
/// staircase; a paste is inserted as it came, which is how a person gets
/// code in. The first line is typed so a command in front of the text (vim's
/// `i`) is still a command; text with no indented line (vim's
/// `:%s/a/b/\n:wq\n`, answers to prompts, commands for a shell) is keys to
/// act on, and is typed.
fn paste_split(text: &str) -> Option<(&str, &str, bool)> {
    let first_break = text.find(['\n', '\r'])?;
    let (first, after) = text.split_at(first_break);
    if !after
        .split(['\n', '\r'])
        .any(|line| line.starts_with([' ', '\t']))
    {
        return None;
    }
    let body = after
        .strip_suffix("\r\n")
        .or_else(|| after.strip_suffix(['\n', '\r']));
    Some(match body {
        Some(body) => (first, body, true),
        None => (first, after, false),
    })
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
        Key::F(n @ 1..=4) => vec![0x1b, b'O', b'O' + n],
        Key::F(n) => format!("\x1b[{}~", function_code(n)).into_bytes(),
        Key::Ctrl(byte) => vec![byte],
        Key::Alt(c) => {
            let mut bytes = vec![0x1b];
            let mut buf = [0u8; 4];
            bytes.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            bytes
        }
        // Held, a key is always the CSI form: cursor-key mode changes only
        // the bare arrows.
        Key::Modified(special, mods) => {
            let (code, last) = special.sequence();
            format!("\x1b[{code};{}{last}", mods + 1).into_bytes()
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

/// Do these bytes reach a program reading **whole lines**? In canonical mode
/// the terminal holds the line being edited until an Enter (`\r`, `\n`)
/// submits it, and acts at once only on its signal keys (`^C`, `^Z`, `^\`)
/// and end-of-file (`^D`); text, Backspace and arrows stay on the line. So a
/// password typed without its Enter has not reached `sudo` yet.
#[must_use]
pub fn reaches_line_reader(bytes: &[u8]) -> bool {
    bytes
        .iter()
        .any(|byte| matches!(byte, b'\r' | b'\n' | 0x03 | 0x04 | 0x1a | 0x1c))
}

/// Do these bytes **submit** the line being edited — an Enter (`\r`, `\n`)
/// — rather than hold text on it or interrupt the read? What makes a
/// password typed at a prompt a password handed over (`pty::session`).
#[must_use]
pub fn submits_line(bytes: &[u8]) -> bool {
    bytes.iter().any(|byte| matches!(byte, b'\r' | b'\n'))
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

/// Remove from the typed text the codes a terminal **writes to** a program
/// and no keyboard sends — colour (`ESC [ … m`), mode switches
/// (`ESC [ ? 25 h`), erases (`ESC [ 2 J`, `ESC [ K`) — returning each as
/// written (`\e[?25h`), for the report to say it was dropped.
///
/// Models copy these from what they have read about terminals, and a
/// program takes each as keys: the ESC as one, the rest as text typed into
/// a file or onto a command line. What terminals do send is kept: arrows,
/// function keys, and mouse reports, whose `<` marks them as input.
pub fn strip_output_codes(parts: &mut Vec<InputPart>) -> Vec<String> {
    let mut removed = Vec::new();
    for part in parts.iter_mut() {
        if let InputPart::Text(text) = part
            && text.contains('\u{1b}')
        {
            *text = strip_from_text(text, &mut removed);
        }
    }
    parts.retain(|part| !matches!(part, InputPart::Text(text) if text.is_empty()));
    removed
}

/// [`strip_output_codes`] over one text run.
fn strip_from_text(text: &str, removed: &mut Vec<String>) -> String {
    let mut kept = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("\u{1b}[") {
        kept.push_str(&rest[..at]);
        let body = &rest[at + 2..];
        // Parameters (and a private marker), intermediates, a final byte.
        let params = body
            .find(|c: char| !('\u{30}'..='\u{3f}').contains(&c))
            .unwrap_or(body.len());
        let inter = body[params..]
            .find(|c: char| !('\u{20}'..='\u{2f}').contains(&c))
            .map_or(body.len(), |n| params + n);
        let final_byte = body[inter..].chars().next();
        let len = 2 + inter + final_byte.map_or(0, char::len_utf8);
        let output_only =
            matches!(final_byte, Some('h' | 'l' | 'm' | 'J' | 'K')) && !body.starts_with('<');
        if output_only {
            removed.push(format!("\\e{}", &rest[at + 1..at + len]));
        } else {
            kept.push_str(&rest[at..at + len]);
        }
        rest = &rest[at + len..];
    }
    kept.push_str(rest);
    kept
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
        Key::Ctrl(byte) => named(&format!("C-{}", control_name(byte))),
        Key::Alt(c) => match c {
            '\r' => named("M-Enter"),
            '\t' => named("M-Tab"),
            '\u{7f}' => named("M-BS"),
            '\u{1b}' => named("M-Esc"),
            ' ' => named("M-Space"),
            c if u32::from(c) < 0x20 => named(&format!("C-M-{}", control_name(c as u8))),
            c => named(&format!("M-{c}")),
        },
        Key::Modified(special, mods) => {
            let mut held = String::new();
            for (bit, prefix) in [(CTRL, "C-"), (ALT, "M-"), (SHIFT, "S-")] {
                if mods & bit != 0 {
                    held.push_str(prefix);
                }
            }
            named(&format!("{held}{}", special.name()))
        }
    }
}

/// What a control byte is Ctrl plus — `c` for `0x03`, `@` for `0x00`.
fn control_name(byte: u8) -> String {
    match byte {
        0x01..=0x1a => ((byte - 1 + b'a') as char).to_string(),
        0x00 => "@".to_string(),
        0x7f => "?".to_string(),
        other => ((other + b'@') as char).to_string(),
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

    const CURSOR_KEYS: Modes = Modes {
        app_cursor: true,
        ..NONE
    };

    const FULL_SCREEN: Modes = Modes {
        full_screen: true,
        ..NONE
    };

    /// No mode set, and a reader no one named.
    const NONE: Modes = Modes {
        app_cursor: false,
        bracketed_paste: false,
        full_screen: false,
        key_by_key: false,
        reader: Reader::Unknown,
    };

    /// What the writer does with `chunks`: each write, and what it waits
    /// for after it.
    fn writes(chunks: &[InputChunk]) -> Vec<(Vec<u8>, Pace)> {
        chunks.iter().map(|c| (c.bytes.clone(), c.then)).collect()
    }

    fn write(bytes: &[u8], then: Pace) -> (Vec<u8>, Pace) {
        (bytes.to_vec(), then)
    }

    fn bytes(chunks: &[InputChunk]) -> Vec<u8> {
        chunks.iter().flat_map(|c| c.bytes.clone()).collect()
    }

    #[test]
    fn input_a_model_html_escaped_is_read_as_the_keys_it_names() {
        // Seen live: a deployment HTML-escaped every `<` and `>` in its tool
        // arguments, and `&lt;Esc&gt;` was typed into vim as eleven letters
        // — insert mode never left, the file never saved.
        assert!(html_escaped("&lt;Esc&gt;:wq&lt;Enter&gt;"));
        assert_eq!(
            parse_input("&lt;Esc&gt;:wq&lt;Enter&gt;"),
            vec![key(Key::Esc), text(":wq"), key(Key::Enter)]
        );
        // The rest of that input was escaped the same way.
        assert_eq!(
            parse_input("if n &lt;= 1 &amp;&amp; m &gt; 0: pass&lt;Enter&gt;"),
            vec![text("if n <= 1 && m > 0: pass"), key(Key::Enter)]
        );
    }

    #[test]
    fn escaped_text_that_names_no_key_is_typed_as_written() {
        // HTML typed into an editor: `&lt;` is what the author wants there.
        for input in ["&lt;div&gt;", "a &lt; b", "&amp;lt;Esc&amp;gt;"] {
            assert!(!html_escaped(input), "{input}");
            assert_eq!(parse_input(input), vec![text(input)], "{input}");
        }
    }

    /// A program that takes pastes, named and no shell: a REPL.
    const PASTES: Modes = Modes {
        bracketed_paste: true,
        reader: Reader::Program,
        ..NONE
    };

    #[test]
    fn indented_lines_reach_a_program_that_takes_pastes_as_a_paste() {
        // Seen live: two models typed a Python function into vim, whose
        // auto-indent added each line's indent to the one before it — the
        // file came out as a staircase, and neither model got out of it. A
        // person pastes code: the first line is typed (a leading `i` still
        // enters insert mode), the lines after it arrive as one paste, and
        // the line break that ends the text is still an Enter.
        let chunks = encode(&parse_input("def f():\n    return 1\n"), PASTES);
        assert_eq!(
            bytes(&chunks),
            b"def f():\x1b[200~\r    return 1\x1b[201~\r".to_vec()
        );
        let chunks = encode(&parse_input("idef f():\n\treturn 1"), PASTES);
        assert_eq!(
            bytes(&chunks),
            b"idef f():\x1b[200~\r\treturn 1\x1b[201~".to_vec(),
            "no line break at the end, no Enter"
        );
    }

    #[test]
    fn lines_parted_by_enter_keys_are_pasted_like_lines_parted_by_newlines() {
        // Seen live: `def fib(n):<Enter>    a, b = 0, 1<Enter>…` — the same
        // bytes as newlines, and the same staircase if typed.
        let as_keys = "def f():<Enter>    return 1<Enter>";
        let as_newlines = "def f():\n    return 1\n";
        assert_eq!(
            bytes(&encode(&parse_input(as_keys), PASTES)),
            bytes(&encode(&parse_input(as_newlines), PASTES))
        );
        assert_eq!(
            bytes(&encode(&parse_input(as_keys), Modes::default())),
            b"def f():\r    return 1\r".to_vec(),
            "typed, an Enter is an Enter either way"
        );
    }

    #[test]
    fn a_blank_line_in_the_middle_goes_in_the_paste() {
        let chunks = encode(
            &parse_input("def f(n):\n    x = n\n\n    return x\n\n"),
            PASTES,
        );
        assert_eq!(
            bytes(&chunks),
            b"def f(n):\x1b[200~\r    x = n\r\r    return x\r\x1b[201~\r".to_vec()
        );
    }

    #[test]
    fn text_without_indented_lines_is_typed_even_where_pastes_are_taken() {
        // `:%s/a/b/g` then `:wq` in vim's normal mode, answers to prompts,
        // commands for a shell: keys to act on, not text to insert.
        for input in [":%s/a/b/g\n:wq\n", "y\nn\n", "ls\npwd\n", "    x = 1\n"] {
            let typed = encode(&parse_input(input), Modes::default());
            assert_eq!(
                bytes(&encode(&parse_input(input), PASTES)),
                bytes(&typed),
                "{input:?}"
            );
        }
    }

    #[test]
    fn a_shell_gets_indented_lines_typed_never_pasted() {
        // Seen driving an interactive bash: `python3` and a loop for it in
        // one call went to bash as a paste — bash read every line, python3
        // started with nothing to read, and the loop ran as shell commands.
        // A shell runs each line it is typed, and a line that starts a
        // program leaves the lines after it for that program to read.
        let shell = Modes {
            reader: Reader::Shell,
            ..PASTES
        };
        let input = "python3\nfor i in range(3):\n    print(i * 7)\n\n";
        assert_eq!(
            bytes(&encode(&parse_input(input), shell)),
            b"python3\rfor i in range(3):\r    print(i * 7)\r\r".to_vec()
        );
    }

    #[test]
    fn a_reader_no_one_named_gets_pastes_only_on_the_full_screen() {
        // Behind a relay (sudo, ssh), or where no process table says who
        // reads: an editor's full screen takes code as a paste, and a line
        // on the main screen may be a shell's — typed.
        let input = "def f():\n    return 1\n";
        let main = Modes {
            bracketed_paste: true,
            ..NONE
        };
        assert_eq!(
            bytes(&encode(&parse_input(input), main)),
            b"def f():\r    return 1\r".to_vec()
        );
        let full = Modes {
            full_screen: true,
            ..main
        };
        assert_eq!(
            bytes(&encode(&parse_input(input), full)),
            b"def f():\x1b[200~\r    return 1\x1b[201~\r".to_vec()
        );
    }

    #[test]
    fn a_reader_is_named_by_its_program() {
        for shell in [
            "bash", "sh", "dash", "zsh", "fish", "ksh", "mksh", "tcsh", "nu",
        ] {
            assert_eq!(Reader::of(shell), Reader::Shell, "{shell}");
        }
        // A relay passes keys to a program out of sight.
        for relay in [
            "sudo", "su", "ssh", "docker", "podman", "kubectl", "script", "tmux",
        ] {
            assert_eq!(Reader::of(relay), Reader::Unknown, "{relay}");
        }
        for program in [
            "python3",
            "python3.13",
            "ipython",
            "node",
            "vim",
            "nano",
            "btop",
        ] {
            assert_eq!(Reader::of(program), Reader::Program, "{program}");
        }
        assert_eq!(Reader::of(""), Reader::Unknown);
    }

    #[test]
    fn short_text_is_typed_a_character_at_a_time_to_a_key_by_key_reader() {
        // Seen driving top: `Mq` written at once was one read, which top
        // took for no key — it never sorted and never quit. A program in
        // raw mode reads a key at a time, on the main screen as on the full
        // one.
        let key_by_key = Modes {
            key_by_key: true,
            ..NONE
        };
        assert_eq!(
            writes(&encode(&parse_input("Mq"), key_by_key)),
            vec![write(b"M", Pace::Read), write(b"q", Pace::Last)]
        );
        assert_eq!(
            writes(&encode(&parse_input("ls<Enter>"), NONE)),
            vec![write(b"ls", Pace::Read), write(b"\r", Pace::Last)],
            "a line read whole goes whole"
        );
    }

    #[test]
    fn a_program_that_takes_no_pastes_gets_indented_lines_typed() {
        let input = "def f():\n    return 1\n";
        assert_eq!(
            bytes(&encode(&parse_input(input), Modes::default())),
            b"def f():\r    return 1\r".to_vec()
        );
    }

    #[test]
    fn codes_a_terminal_writes_to_a_program_are_not_typed() {
        // Seen live: a model saved in nano with `\u000f<Enter>\u001b[?25h`
        // — Ctrl+O, Enter, and "show the cursor" — and nano took the ESC as
        // a key and typed `25h` into the file. No keyboard sends that code.
        let mut parts = parse_input("\u{f}<Enter>\u{1b}[?25h");
        let removed = strip_output_codes(&mut parts);
        assert_eq!(parts, vec![text("\u{f}"), key(Key::Enter)]);
        assert_eq!(removed, vec!["\\e[?25h".to_string()]);
        // Colour and erases, in the middle of text, go too.
        let mut parts = vec![text("ls\u{1b}[0m -l\u{1b}[2J\u{1b}[K\r")];
        let removed = strip_output_codes(&mut parts);
        assert_eq!(parts, vec![text("ls -l\r")]);
        assert_eq!(removed, vec!["\\e[0m", "\\e[2J", "\\e[K"]);
    }

    #[test]
    fn keys_spelled_as_their_codes_are_kept() {
        // Arrows, function keys, a mouse report: what terminals do send.
        for input in [
            "\u{1b}[A",
            "\u{1b}OB",
            "\u{1b}[15~",
            "\u{1b}[1;5C",
            "\u{1b}[<0;10;5m",
            "\u{1b}[H",
            "plain text",
        ] {
            let mut parts = vec![text(input)];
            assert!(strip_output_codes(&mut parts).is_empty(), "{input:?}");
            assert_eq!(parts, vec![text(input)], "{input:?}");
        }
    }

    #[test]
    fn input_that_was_only_output_codes_is_left_empty() {
        let mut parts = vec![text("\u{1b}[?1049h")];
        assert_eq!(strip_output_codes(&mut parts), vec!["\\e[?1049h"]);
        assert!(parts.is_empty());
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
        let chunks = encode(&parse_input("print(1)\n"), Modes::default());
        assert_eq!(bytes(&chunks), b"print(1)\r");
        // CRLF is one Enter, not two.
        let chunks = encode(&parse_input("a\r\nb\n"), Modes::default());
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
            assert_eq!(
                bytes(&encode(&[key(k)], Modes::default())),
                expected,
                "{k:?}"
            );
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
            assert_eq!(bytes(&encode(&[key(k)], CURSOR_KEYS)), expected, "{k:?}");
        }
        // Everything else is the same in both modes.
        assert_eq!(bytes(&encode(&[key(Key::Enter)], CURSOR_KEYS)), b"\r");
    }

    #[test]
    fn every_key_is_written_on_its_own_a_moment_after_the_last() {
        // Seen driving btop: a program that takes whatever one read returns
        // as one key press ignored `<Down><Down>` written together — no key
        // it knows — where a person's two presses never arrive at once.
        let chunks = encode(&parse_input("ab<Down><Down><Enter>"), Modes::default());
        assert_eq!(
            writes(&chunks),
            vec![
                write(b"ab", Pace::Read),
                write(b"\x1b[B", Pace::Read),
                write(b"\x1b[B", Pace::Read),
                write(b"\r", Pace::Last),
            ]
        );
    }

    #[test]
    fn short_text_is_typed_a_character_at_a_time_into_a_full_screen_program() {
        // btop's filter took `bash` written at once as one unknown key and
        // dropped it; typed a letter at a time, it filtered.
        let chunks = encode(&parse_input("fbash"), FULL_SCREEN);
        assert_eq!(
            writes(&chunks),
            vec![
                write(b"f", Pace::Read),
                write(b"b", Pace::Read),
                write(b"a", Pace::Read),
                write(b"s", Pace::Read),
                write(b"h", Pace::Last),
            ]
        );
        // A character is never split, and CRLF is still one Enter.
        let chunks = encode(&parse_input("\u{e9}\r\n"), FULL_SCREEN);
        assert_eq!(
            writes(&chunks),
            vec![
                write("\u{e9}".as_bytes(), Pace::Read),
                write(b"\r", Pace::Last)
            ]
        );
    }

    #[test]
    fn long_text_and_text_at_a_line_prompt_go_in_one_write() {
        // Code typed into an editor is input, not a key press, and pacing
        // it would cost seconds; a shell or a REPL reads a line however it
        // arrives.
        let long = "x".repeat(TYPED_TEXT_MAX + 1);
        assert_eq!(encode(&parse_input(&long), FULL_SCREEN).len(), 1);
        assert_eq!(
            writes(&encode(&parse_input("print(6*7)<Enter>"), Modes::default())),
            vec![write(b"print(6*7)", Pace::Read), write(b"\r", Pace::Last)]
        );
        // A paste is one write wherever it goes.
        let pastes = Modes {
            full_screen: true,
            ..PASTES
        };
        assert_eq!(
            encode(&parse_input("def f():\n    return 1\n"), pastes).len(),
            1
        );
    }

    #[test]
    fn typing_is_bounded_by_every_wait_the_writer_may_make() {
        // Each paced write may wait out the program's read and the gap, an
        // Esc its pause after that: the most the keys can take, which the
        // writer cuts short by saying when it is done.
        let chunks = encode(&parse_input("ab<Esc>:q<Enter>"), Modes::default());
        assert_eq!(
            typing_bound(&chunks),
            (KEY_READ_WAIT + KEY_PAUSE) * 3 + ESC_PAUSE
        );
        assert_eq!(typing_bound(&[]), Duration::ZERO);
    }

    #[test]
    fn a_lone_esc_followed_by_more_input_pauses_after_itself() {
        // Vim reads `ESC :` inside its timeout as Alt+: — the pause makes it
        // a key press followed by a command-line colon.
        let chunks = encode(&parse_input("hello<Esc>:wq<Enter>"), Modes::default());
        assert_eq!(
            writes(&chunks),
            vec![
                write(b"hello", Pace::Read),
                write(b"\x1b", Pace::Esc),
                write(b":wq", Pace::Read),
                write(b"\r", Pace::Last),
            ]
        );
        // A trailing Esc has nothing to be confused with.
        let chunks = encode(&parse_input("x<Esc>"), Modes::default());
        assert_eq!(
            writes(&chunks),
            vec![write(b"x", Pace::Read), write(b"\x1b", Pace::Last)]
        );
    }

    #[test]
    fn empty_input_encodes_to_nothing() {
        assert!(encode(&[], Modes::default()).is_empty());
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
    fn only_an_enter_or_a_signal_key_reaches_a_program_reading_a_line() {
        let reaches = |input: &str| {
            encode(&parse_input(input), Modes::default())
                .iter()
                .any(|chunk| reaches_line_reader(&chunk.bytes))
        };
        assert!(reaches("hunter2\n"));
        assert!(reaches("hunter2<Enter>"));
        assert!(reaches("<C-j>"), "a bare line feed");
        for key in ["<C-c>", "<C-d>", "<C-z>", r"<C-\>"] {
            assert!(reaches(key), "{key} acts at once");
        }
        // The line being edited holds everything else until Enter.
        assert!(!reaches("hunter2"));
        assert!(!reaches("pässwörd"));
        assert!(!reaches("abc<BS><Left><Up><M-x>"));
        assert!(!reaches(""));
    }

    #[test]
    fn an_enter_submits_the_line_being_edited() {
        let submits = |input: &str| {
            encode(&parse_input(input), Modes::default())
                .iter()
                .any(|chunk| submits_line(&chunk.bytes))
        };
        assert!(submits("hunter2\n"));
        assert!(submits("hunter2<Enter>"));
        assert!(submits("<C-j>"));
        assert!(!submits("hunter2"));
        assert!(!submits("<C-c>"), "an interrupt submits nothing");
        assert!(!submits(""));
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
    fn keys_held_with_modifiers_send_what_xterm_sends() {
        // Seen live: `<M-F7>` — Alt+F7 — was no key the notation knew, so
        // mc's command line got the six characters `<M-F7>`.
        let cases: Vec<(&str, &[u8])> = vec![
            ("<C-Left>", b"\x1b[1;5D"),
            ("<S-Up>", b"\x1b[1;2A"),
            ("<M-F7>", b"\x1b[18;3~"),
            ("<Alt+F4>", b"\x1b[1;3S"),
            ("<C-S-End>", b"\x1b[1;6F"),
            ("<Ctrl+Shift+Right>", b"\x1b[1;6C"),
            ("<S-Delete>", b"\x1b[3;2~"),
            ("<C-PageDown>", b"\x1b[6;5~"),
            ("<M-C-Home>", b"\x1b[1;7H"),
            ("<S-F12>", b"\x1b[24;2~"),
            ("<M-Enter>", b"\x1b\r"),
            ("<M-BS>", b"\x1b\x7f"),
            ("<C-M-x>", b"\x1b\x18"),
            ("<M-S-a>", b"\x1bA"),
            // What a terminal sends for these: the modifier has no byte.
            ("<S-Enter>", b"\r"),
            ("<C-Tab>", b"\t"),
            ("<C-BS>", b"\x08"),
        ];
        for (input, expected) in cases {
            let parts = parse_input(input);
            assert!(
                matches!(parts.as_slice(), [InputPart::Key(_)]),
                "{input}: {parts:?}"
            );
            assert_eq!(
                bytes(&encode(&parts, Modes::default())),
                expected,
                "{input}"
            );
        }
        // Cursor-key mode changes the bare arrows only.
        assert_eq!(
            bytes(&encode(&parse_input("<C-Left>"), CURSOR_KEYS)),
            b"\x1b[1;5D"
        );
        // Nothing a modifier can hold, or no modifier at all: text.
        for input in ["<S-foo>", "<C-Left-x>", "<X-Up>", "<S-a>"] {
            assert_eq!(parse_input(input), vec![text(input)], "{input}");
        }
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
        assert_eq!(
            display_input("<ctrl+shift+left><Alt+F7><M-Enter>"),
            "<C-S-Left><M-F7><M-Enter>"
        );
    }
}
