//! Clickable links: bare-URL detection, the URL interner, the per-cell link
//! carrier, the OSC 8 escape framing, and how a labelled link shows its
//! target (see `docs/links.md`).
//!
//! A URL wider than its row hard-breaks across display rows, and the
//! terminal's own URL detection works on visible row text — so clicking a
//! wrapped URL used to open only its first fragment. The fix threads the
//! *whole* URL from the renderer (where it is still in one piece) to the
//! paint boundary (where the escape must be written) through the one per-cell
//! channel that survives the trip: [`linked`] interns the URL and stamps its
//! id into `Style::underline_color` (nothing else in the crate sets an
//! underline colour), and `term`'s cell emitter decodes the id back
//! ([`carrier_id`] → [`link_url`]), strips it, and brackets the run with the
//! [`osc8_open`]/[`OSC8_CLOSE`] hyperlink escape so clicking **any** fragment
//! opens the **full** URL.
//!
//! Everything here is pure except two pieces of render state, neither of
//! them app state: the interner — a process-global, append-only cache (the
//! `highlight` grammar-singleton precedent), so a reflow re-rendering from
//! history hands the same URL the same id — and the per-thread
//! [`TargetDisplay`] the boundary sets once at startup (the active theme's
//! arrangement).

use std::collections::HashMap;
use std::ops::Range;
use std::path::{Component, Path};
use std::sync::{Arc, Mutex, OnceLock};

use ratatui::style::{Color, Style};

/// Byte ranges of the bare `http://`/`https://` URLs in `text` (one prose
/// line — detection never crosses lines, which is what keeps streaming
/// prefix-stable). The scheme must open at a non-alphanumeric boundary
/// (`xhttps://…` is not a link), the body takes everything printable that is
/// not whitespace or one of `<>"`` ` ``|` (a control character — an `ESC`
/// smuggled into model output — terminates it), and the tail is trimmed of
/// closing punctuation (`.,;:!?'"`) and of **unbalanced** closers:
/// `…/Foo_(bar)` keeps its `)`, a URL inside `(see …)` gives it back.
#[must_use]
pub fn find_urls(text: &str) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some((start, body)) = next_scheme(text, from) {
        let end = trim_tail(text, start, url_end(text, body, None));
        // A bare scheme with nothing after it is prose, not a link.
        if end > body {
            out.push(start..end);
            from = end;
        } else {
            from = body;
        }
    }
    out
}

/// Whether `text` is, whole, one `http://`/`https://` URL: the scheme at its
/// very first byte, something after the scheme, and nothing anywhere a URL
/// cannot hold — whitespace, a control, one of the `<>"`` ` ``|` delimiters
/// ([`find_urls`]' body rule). Such a text is linked **verbatim**:
/// [`find_urls`] trims a closing `.`, `!` or unbalanced `)` because the prose
/// around a URL owns it, and a text that *is* the URL has no prose to give it
/// back to. The `/mcp` sign-in page shows exactly that text, its authorize
/// URL, where a trimmed target would open something other than what `c`
/// copies and the browser was handed (`docs/links.md`).
#[must_use]
pub fn is_whole_url(text: &str) -> bool {
    matches!(next_scheme(text, 0), Some((0, body)) if body < text.len())
        && text.chars().all(is_url_char)
}

/// Code's counterpart of [`find_urls`]. A URL at the start of a literal
/// (including a URL-only code span) or introduced by a quote keeps its exact
/// punctuation. A matching quote terminates it before surrounding code such
/// as `').json()` or an adjacent string. Other unquoted URLs embedded in
/// code/prose still use the conservative prose-tail rule.
///
/// This is lexical detection, not string-literal evaluation: escaped strings
/// and interpolations are not decoded into invented link targets.
pub(crate) fn find_code_urls(text: &str) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some((start, body)) = next_scheme(text, from) {
        let quote = text[..start]
            .chars()
            .next_back()
            .filter(|c| matches!(c, '\'' | '"' | '`'));
        let end = url_end(text, body, quote);
        // Decide from the prefix only: later code appended after a space
        // must not retroactively trim a URL already committed to scrollback.
        let starts_literal = text[..start].trim().is_empty();
        let end = if quote.is_some() || starts_literal {
            end
        } else {
            trim_tail(text, start, end)
        };
        if end > body {
            out.push(start..end);
            from = end;
        } else {
            from = body;
        }
    }
    out
}

/// End of a URL's printable body, optionally bounded by its opening code quote.
fn url_end(text: &str, body: usize, quote: Option<char>) -> usize {
    let mut end = body;
    for (i, c) in text[body..].char_indices() {
        if !is_url_char(c) || Some(c) == quote {
            break;
        }
        end = body + i + c.len_utf8();
    }
    end
}

/// Whether a URL could still be **forming** at the end of `text` — a
/// still-growing streamed line whose next characters could restyle text
/// already present, so the streaming committer must withhold it whole (the
/// [`crate::markdown::has_open_inline`] situation, for autolinks —
/// `docs/links.md`). Two shapes are unsettled:
///
/// - a detected scheme whose **body run reaches the end of the line** — more
///   URL characters would grow the link (`http://e` → `http://ex`), re-join
///   trimmed tail punctuation (`http://e.` → `http://e.com`), or turn a bare
///   scheme into a link at its first body char; a URL already terminated by a
///   stopper (whitespace, `<>"`` ` ``|`, a control) is settled — appended text
///   can't reach back into it; and
/// - a trailing fragment that a few more characters could complete into a
///   scheme (`…see ht` → `…see http://x`), at a boundary [`find_urls`] would
///   honour — mid-word fragments (`blah`) stay settled, matching the
///   detector's own `xhttps://` rule.
#[must_use]
pub fn has_forming_url(text: &str) -> bool {
    // Only the LAST scheme can have its body run reach the line's end: any
    // earlier candidate is separated from the next by either a stopper (which
    // seals it) or an unbroken url-char run (in which case the last one's
    // tail is a subset of its own, and both verdicts agree). One scan finds
    // it and one pass checks its tail — O(line), not O(line × URLs), which
    // matters because the committer calls this per chunk on a URL-list line.
    let mut last_body = None;
    let mut from = 0;
    while let Some((_, body)) = next_scheme(text, from) {
        last_body = Some(body);
        from = body;
    }
    // `all` over an empty tail is true: a bare scheme ending the line is
    // exactly the "first body char flips it to a link" case.
    if let Some(body) = last_body
        && text[body..].chars().all(is_url_char)
    {
        return true;
    }
    scheme_prefix_at_end(text)
}

/// Whether `text` ends with a proper prefix of `http://`/`https://` opening at
/// a boundary [`next_scheme`] would accept — the not-yet-a-scheme tail of
/// [`has_forming_url`]. Byte-wise like the detector: the scheme is pure ASCII.
fn scheme_prefix_at_end(text: &str) -> bool {
    let bytes = text.as_bytes();
    for scheme in [&b"https://"[..], &b"http://"[..]] {
        for plen in 1..scheme.len() {
            if bytes.len() < plen {
                break;
            }
            let start = bytes.len() - plen;
            if bytes[start..].eq_ignore_ascii_case(&scheme[..plen])
                && (start == 0 || !bytes[start - 1].is_ascii_alphanumeric())
            {
                return true;
            }
        }
    }
    false
}

/// The next `http(s)://` at or after byte `from` that opens at a
/// non-alphanumeric boundary, as `(scheme_start, body_start)`. The whole scan
/// is byte-wise — the scheme is pure ASCII, so a match position is always a
/// char boundary and no slice can land inside a multi-byte char.
fn next_scheme(text: &str, from: usize) -> Option<(usize, usize)> {
    let bytes = text.as_bytes();
    let mut i = from;
    while i < bytes.len() {
        // Both schemes open on an `h`, and almost no byte of prose is one:
        // go straight to the next, rather than hold every byte up against
        // both schemes — this scan runs over every prose and code line the
        // renderer sees, linked or not.
        i += bytes[i..]
            .iter()
            .position(|byte| matches!(byte, b'h' | b'H'))?;
        let scheme = [&b"https://"[..], &b"http://"[..]]
            .into_iter()
            .find(|s| bytes[i..].len() >= s.len() && bytes[i..i + s.len()].eq_ignore_ascii_case(s))
            .map(<[u8]>::len);
        if let Some(len) = scheme {
            // ASCII byte check is enough for the boundary: a multi-byte char's
            // continuation bytes are never ASCII alphanumeric.
            let bounded = i == 0 || !bytes[i - 1].is_ascii_alphanumeric();
            if bounded {
                return Some((i, i + len));
            }
            i += len;
        } else {
            i += 1;
        }
    }
    None
}

/// Whether `c` can sit inside a URL: printable, not whitespace, and not one
/// of the delimiters prose wraps URLs in (`<url>`, `"url"`, `` `url` ``, a
/// table's `|`). Controls are excluded so an escape byte can never enter a
/// link target.
fn is_url_char(c: char) -> bool {
    !c.is_whitespace() && !c.is_control() && !matches!(c, '<' | '>' | '"' | '`' | '|')
}

/// Trim the candidate's tail of closing punctuation and unbalanced closers
/// (see [`find_urls`]), returning the new end.
fn trim_tail(text: &str, start: usize, mut end: usize) -> usize {
    loop {
        let Some(last) = text[start..end].chars().next_back() else {
            return end;
        };
        let trimmed = match last {
            '.' | ',' | ';' | ':' | '!' | '?' | '\'' | '"' => true,
            ')' => unbalanced(&text[start..end], '(', ')'),
            ']' => unbalanced(&text[start..end], '[', ']'),
            '}' => unbalanced(&text[start..end], '{', '}'),
            _ => false,
        };
        if !trimmed {
            return end;
        }
        end -= last.len_utf8();
    }
}

/// Whether `s` holds more `close` than `open` — a trailing closer that
/// belongs to the surrounding prose, not the URL.
fn unbalanced(s: &str, open: char, close: char) -> bool {
    let opens = s.chars().filter(|&c| c == open).count();
    let closes = s.chars().filter(|&c| c == close).count();
    closes > opens
}

// --- The interner + the per-cell carrier ---

/// Ids are 24-bit (they ride an RGB triple) and 1-based: id `0` — plain black
/// — is reserved as "not a link", so a real `Rgb(0, 0, 0)` underline could
/// never be mistaken for one.
///
/// The **top bit is not ours**: the same channel carries the inline-image
/// blocks' per-cell marker ([`crate::images::geometry::IMAGE_CARRIER_FLAG`]),
/// so the space is split rather than shared and a picture can never decode as
/// a URL. Ids run to `0x7F_FFFF` — eight million distinct URLs in one session,
/// which no conversation reaches.
const LINK_ID_MAX: u32 = 0x7F_FFFF;

/// The process-global URL interner (append-only; render cache, not app
/// state). `ids` and `urls` grow in lockstep: `urls[id - 1]` is the URL
/// behind `id`.
struct Interner {
    ids: HashMap<Arc<str>, u32>,
    urls: Vec<Arc<str>>,
}

fn interner() -> &'static Mutex<Interner> {
    static INTERNER: OnceLock<Mutex<Interner>> = OnceLock::new();
    INTERNER.get_or_init(|| {
        Mutex::new(Interner {
            ids: HashMap::new(),
            urls: Vec::new(),
        })
    })
}

/// Intern `url`, returning its stable id — the same URL always gets the same
/// id for the life of the process. `None` only past [`LINK_ID_MAX`] distinct
/// URLs (then the text simply isn't marked — the wrap is unchanged, the
/// terminal's own detection still applies).
fn intern(url: &str) -> Option<u32> {
    let mut guard = interner().lock().ok()?;
    if let Some(&id) = guard.ids.get(url) {
        return Some(id);
    }
    let next = u32::try_from(guard.urls.len()).ok()?.checked_add(1)?;
    if next > LINK_ID_MAX {
        return None;
    }
    let url: Arc<str> = Arc::from(url);
    guard.urls.push(Arc::clone(&url));
    guard.ids.insert(url, next);
    Some(next)
}

/// The URL behind an interned id ([`carrier_id`]'s other half). `None` for an
/// id this process never interned — the paint boundary then strips the
/// carrier without emitting a link.
#[must_use]
pub fn link_url(id: u32) -> Option<Arc<str>> {
    if id == 0 {
        return None;
    }
    let guard = interner().lock().ok()?;
    guard.urls.get(id as usize - 1).cloned()
}

/// `style` carrying `url` as its link target: the URL is interned and its id
/// stamped into the style's underline colour — the one per-cell channel that
/// survives `Span` → `Cell` → every paint path. The paint boundary strips it
/// before the terminal sees it, so it never renders as a colour. Unchanged
/// when the interner is full.
#[must_use]
pub fn linked(style: Style, url: &str) -> Style {
    match carrier(url) {
        Some(carrier) => carrier.stamp(style),
        None => style,
    }
}

/// One interned link target, ready to mark any number of styles —
/// [`linked`] in two steps, for a link whose label is several styled runs:
/// the URL is looked up once and each run then takes a copy of the id,
/// where a [`linked`] per run hashes the whole URL again for every one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Carrier(u32);

/// The [`Carrier`] of `url`. `None` only when the interner is full
/// ([`linked`]'s case: the text then goes unmarked).
#[must_use]
pub fn carrier(url: &str) -> Option<Carrier> {
    intern(url).map(Carrier)
}

impl Carrier {
    /// `style` carrying this target — what [`linked`] returns for its URL.
    #[must_use]
    pub fn stamp(self, style: Style) -> Style {
        let id = self.0;
        style.underline_color(Color::Rgb((id >> 16) as u8, (id >> 8) as u8, id as u8))
    }
}

/// Decode a cell's underline colour back to the link id it carries — the
/// pure inverse of [`linked`]'s stamp. Only a non-zero RGB triple **below the
/// image flag** decodes; every other colour kind (and `Rgb(0, 0, 0)`, the
/// reserved id) is an ordinary colour, not ours — as is anything with
/// [`IMAGE_CARRIER_FLAG`] set, which is an image block's marker
/// (`docs/images.md`).
///
/// [`IMAGE_CARRIER_FLAG`]: crate::images::geometry::IMAGE_CARRIER_FLAG
#[must_use]
pub fn carrier_id(underline_color: Color) -> Option<u32> {
    let Color::Rgb(r, g, b) = underline_color else {
        return None;
    };
    let id = u32::from(r) << 16 | u32::from(g) << 8 | u32::from(b);
    (id != 0 && id <= LINK_ID_MAX).then_some(id)
}

/// The link target a marked `style` carries (test/inspection convenience:
/// [`carrier_id`] + [`link_url`] over `style.underline_color`).
#[must_use]
pub fn style_link(style: &Style) -> Option<Arc<str>> {
    style
        .underline_color
        .and_then(carrier_id)
        .and_then(link_url)
}

// --- The OSC 8 escape framing ---

/// Close the open hyperlink: `ESC ] 8 ; ; ST`.
pub const OSC8_CLOSE: &str = "\x1b]8;;\x1b\\";

/// Open a hyperlink to `url`: `ESC ] 8 ; id=az{id} ; {url} ST`. The `id=`
/// parameter groups a wrapped link's fragments so supporting terminals
/// hover-highlight them as one; the interner makes it stable per URL. Every
/// byte a terminal could mis-parse — controls, space, DEL, non-ASCII — is
/// percent-encoded (the kitty spec's rule), so a crafted "URL" can never
/// smuggle an escape into the stream; `%` passes through, an already-encoded
/// URL is not double-encoded.
#[must_use]
pub fn osc8_open(id: u32, url: &str) -> String {
    let mut out = String::with_capacity(url.len() + 24);
    out.push_str("\x1b]8;id=az");
    out.push_str(&id.to_string());
    out.push(';');
    for &b in url.as_bytes() {
        match b {
            0x21..=0x7e => out.push(b as char),
            _ => {
                out.push('%');
                out.push_str(&format!("{b:02X}"));
            }
        }
    }
    out.push_str("\x1b\\");
    out
}

/// The longest target a hyperlink is sure to carry, in bytes as written
/// ([`osc8_uri_len`]): VTE and iTerm2 drop an OSC 8 whose URI is longer,
/// and print its text unlinked. A longer target is still marked — a
/// terminal with no such cap opens it — but it must not be the only place
/// the address is (`docs/links.md` *What is never hidden*).
pub const HYPERLINK_URI_MAX_BYTES: usize = 2083;

/// The bytes `url` takes in the escape [`osc8_open`] writes: one for each
/// byte it passes through, three for each it percent-encodes.
#[must_use]
pub fn osc8_uri_len(url: &str) -> usize {
    url.bytes()
        .map(|byte| if (0x21..=0x7e).contains(&byte) { 1 } else { 3 })
        .sum()
}

// --- `file://` targets ---

/// The `file://` URI of `path` — the target a `● Read/Write/Edit({path})`
/// header's path carries (`docs/links.md` *The file tool header*), so a
/// click opens the file however the row showed it. A relative `path`
/// resolves against `cwd` first (the session's — `app::PathDisplay`); `None`
/// when it cannot be placed: a relative path with no absolute `cwd`, or an
/// empty one. The path is normalized lexically the way `header_path` reads
/// it (`.` dropped, `..` collapsed and saturating at the root, symlinks never
/// consulted) and percent-encoded by `Path.as_uri()`'s rule: `/` and the
/// unreserved `A-Za-z0-9-._~` pass, every other byte — a space, `#`, `?`,
/// `%`, each UTF-8 byte of a non-ASCII name — is `%XX`, so the terminal
/// decodes the exact name back and nothing in it can read as a fragment or
/// a query. [`osc8_open`] passes `%` through, so the two encodings compose.
#[must_use]
pub fn file_url(path: &str, cwd: Option<&Path>) -> Option<String> {
    if path.is_empty() {
        return None;
    }
    let given = Path::new(path);
    let resolved = if given.is_absolute() {
        given.to_path_buf()
    } else {
        cwd.filter(|cwd| cwd.is_absolute())?.join(given)
    };
    let mut prefix = String::new();
    let mut parts: Vec<String> = Vec::new();
    for component in resolved.components() {
        match component {
            Component::Prefix(p) => prefix = p.as_os_str().to_string_lossy().into_owned(),
            Component::RootDir | Component::CurDir => {}
            Component::ParentDir => {
                parts.pop();
            }
            Component::Normal(seg) => parts.push(seg.to_string_lossy().into_owned()),
        }
    }
    let mut url = String::from("file://");
    if !prefix.is_empty() {
        // A Windows drive rides as `file:///C:/…`, its own spelling kept.
        url.push('/');
        url.push_str(&prefix);
    }
    if parts.is_empty() {
        url.push('/');
    }
    for part in &parts {
        url.push('/');
        push_percent_encoded(&mut url, part);
    }
    Some(url)
}

/// Append `segment` to `out` percent-encoded for a `file://` path: the
/// unreserved bytes (`A-Za-z0-9-._~`) as they are, every other byte as
/// uppercase `%XX` — per byte of the UTF-8, the form every decoder reads.
fn push_percent_encoded(out: &mut String, segment: &str) {
    for &b in segment.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(b as char);
            }
            _ => {
                out.push('%');
                out.push_str(&format!("{b:02X}"));
            }
        }
    }
}

// --- The env gate ---

/// Environment gate: a falsy value turns OSC 8 emission off for a terminal
/// that misbehaves (the carrier is still stripped — it must never paint as a
/// colour). Default on.
pub const HYPERLINKS_ENV: &str = "ALTER_ZERO_HYPERLINKS";

/// Whether hyperlink emission should be **off**, given [`HYPERLINKS_ENV`]'s
/// value: the standard falsy spellings (`0`/`false`/`no`/`off`,
/// case-insensitive, surrounding whitespace ignored) disable it; anything
/// else — including unset — leaves it on. Pure, so it's unit-tested while
/// the env read stays at the boundary (`term`'s init).
#[must_use]
pub fn hyperlinks_disabled(value: Option<&str>) -> bool {
    matches!(
        value.map(|v| v.trim().to_ascii_lowercase()).as_deref(),
        Some("0" | "false" | "no" | "off")
    )
}

// --- How a labelled link shows its target ---

/// How a markdown link — `[label](https://…)` — shows where it goes
/// (`docs/links.md` *Labelled links*).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TargetDisplay {
    /// The target printed after the label: `label (https://…)`. The default,
    /// because it is the one that cannot lose anything: a caller that never
    /// learned the terminal opens hyperlinks never hides the only usable URL.
    #[default]
    Shown,
    /// The label alone, dressed as a link, the target behind it as the
    /// hyperlink a click opens — for a terminal known to open one.
    Hidden,
}

thread_local! {
    /// How the renderers on this thread show a labelled link's target. The
    /// event loop draws on one thread, so one cell is the whole of the
    /// session's choice; each test thread has its own, at the default — the
    /// active theme's arrangement (`ui::activate_theme`), for its reasons.
    static TARGET_DISPLAY: std::cell::Cell<TargetDisplay> =
        const { std::cell::Cell::new(TargetDisplay::Shown) };
}

/// How a labelled link's target is shown on this thread, right now.
#[must_use]
pub fn target_display() -> TargetDisplay {
    TARGET_DISPLAY.with(std::cell::Cell::get)
}

/// Show labelled links' targets as `display` on this thread from now on —
/// the boundary's call, once, before the first row is rendered
/// ([`detect_target_display`] is what it passes).
pub fn set_target_display(display: TargetDisplay) {
    TARGET_DISPLAY.with(|cell| cell.set(display));
}

/// Run `f` with labelled links' targets shown as `display`, then put back
/// whatever was in force — for a render that goes somewhere a hyperlink
/// cannot follow (the `/export` text keeps every URL), and for tests.
pub fn with_target_display<T>(display: TargetDisplay, f: impl FnOnce() -> T) -> T {
    struct Restore(TargetDisplay);
    impl Drop for Restore {
        fn drop(&mut self) {
            set_target_display(self.0);
        }
    }
    let _restore = Restore(target_display());
    set_target_display(display);
    f()
}

/// Environment override for [`TargetDisplay`]: `hide` shows a labelled link
/// as its label alone, `show` keeps `label (url)`, and anything else — unset
/// included — leaves it to the terminal ([`detect_target_display`]).
pub const LINK_TARGETS_ENV: &str = "ALTER_ZERO_LINK_TARGETS";

/// The [`TargetDisplay`] this session calls for. `var` reads one environment
/// variable; `answered` is the name the terminal gave for itself when the
/// startup read asked it (`XTVERSION`, `term::query`) — `None` when the read
/// did not ask or heard nothing. Pure, so every rule is unit-tested while
/// both reads stay at the boundary.
///
/// A target is hidden only where something is known to open it, since hidden
/// anywhere else it is simply gone:
///
/// 1. With [`HYPERLINKS_ENV`] off no hyperlink is written at all, so the
///    target is shown — whatever the override says.
/// 2. [`LINK_TARGETS_ENV`] decides when it says `hide` or `show`: a tmux
///    configured to forward hyperlinks, or a terminal not named below, is
///    the user's to speak for.
/// 3. Under a multiplexer the target is shown. tmux forwards a hyperlink
///    only to a terminal its own `terminal-features` list says takes one,
///    screen never does, and the variables naming the terminal the session
///    was *started* from say nothing of the one attached now.
/// 4. The innermost thing drawing is the one that counts, and the variables
///    an outer terminal exported are still in the environment of whatever
///    was started inside it. So an editor's own terminal (`NVIM`,
///    `VIM_TERMINAL`, `INSIDE_EMACS`, `TERMINAL_EMULATOR`) is unknown
///    whatever surrounds it.
/// 5. A terminal that **answered** with the name of one known to open
///    hyperlinks is that terminal: the answer came down the line a hyperlink
///    goes up, which no variable can say of itself. It is what names the
///    terminal over `ssh`, where the variables it set for its own children
///    stay behind and `TERM` is often set down to one the remote host has an
///    entry for. An answer only ever adds a yes — any other name, or none,
///    leaves the environment to decide.
/// 6. A terminal that names itself in `TERM_PROGRAM` is judged by that name
///    alone, rule 4's reason again.
/// 7. Otherwise the variables and `TERM` values the terminals known to open
///    hyperlinks set decide; with none of them, the target is shown.
#[must_use]
pub fn detect_target_display(
    var: impl Fn(&str) -> Option<String>,
    answered: Option<&str>,
) -> TargetDisplay {
    if hyperlinks_disabled(var(HYPERLINKS_ENV).as_deref()) {
        return TargetDisplay::Shown;
    }
    match var(LINK_TARGETS_ENV)
        .map(|value| value.trim().to_ascii_lowercase())
        .as_deref()
    {
        Some("hide") => return TargetDisplay::Hidden,
        Some("show") => return TargetDisplay::Shown,
        _ => {}
    }
    if opens_hyperlinks(&var, answered) {
        TargetDisplay::Hidden
    } else {
        TargetDisplay::Shown
    }
}

/// Whether asking the terminal its name could change what
/// [`detect_target_display`] decides in this environment: without an answer
/// the target stays on screen, and an answer naming a terminal that opens
/// hyperlinks would hide it. Nothing ahead of the answer has decided (the
/// hyperlink switch, the override, a multiplexer, a console, an editor's
/// terminal), and the environment has not already named a terminal that
/// opens hyperlinks. Pure; the boundary passes `std::env::var`.
///
/// The startup read asks the name on this account even where it asks
/// nothing about graphics (`term::query`). That matters over `ssh` from
/// iTerm2: `LC_TERMINAL=iTerm2` is the one variable of the terminal's that
/// crosses, and it is what makes the picture detection name iTerm2's
/// protocol and skip its own question. `LC_TERMINAL` itself is not read,
/// since it reaches a mosh session too, where no hyperlink gets through; a
/// mosh in between passes on no answer either, so there the target stays.
#[must_use]
pub fn wants_terminal_name(var: impl Fn(&str) -> Option<String>) -> bool {
    /// Any answer [`answers_as_a_linking_terminal`] accepts will do.
    const LINKING_ANSWER: &str = "kitty";
    detect_target_display(&var, None) == TargetDisplay::Shown
        && detect_target_display(&var, Some(LINKING_ANSWER)) == TargetDisplay::Hidden
}

/// The variables an editor sets for the terminal it embeds — Neovim's and
/// Vim's `:terminal`, an Emacs terminal buffer, a JetBrains IDE's. Such a
/// terminal draws for itself and may open no hyperlink at all, while its
/// environment still carries every variable of the terminal the editor was
/// started from; it sets no `TERM_PROGRAM` of its own to be judged by.
const EDITOR_TERMINAL_VARS: [&str; 4] =
    ["NVIM", "VIM_TERMINAL", "INSIDE_EMACS", "TERMINAL_EMULATOR"];

/// Whether this is a terminal known to open an OSC 8 hyperlink — rules 3 to
/// 7 of [`detect_target_display`].
fn opens_hyperlinks(var: &impl Fn(&str) -> Option<String>, answered: Option<&str>) -> bool {
    let term = var("TERM").unwrap_or_default();
    let sessions = crate::images::MULTIPLEXER_SESSION_VARS.map(var);
    if crate::images::under_multiplexer(
        Some(term.as_str()),
        &sessions.each_ref().map(Option::as_deref),
    ) || term == "dumb"
        || term.starts_with("linux")
    {
        return false;
    }
    let set = |name: &str| var(name).is_some_and(|value| !value.is_empty());
    if EDITOR_TERMINAL_VARS.into_iter().any(set) {
        return false;
    }
    if answered.is_some_and(answers_as_a_linking_terminal) {
        return true;
    }
    if let Some(program) =
        var("TERM_PROGRAM").filter(|name| !name.is_empty() && !name.eq_ignore_ascii_case("tmux"))
    {
        return names_a_linking_terminal(&program);
    }
    // VTE reads the escape from 0.50 on; its version is `major * 10000 +
    // minor * 100 + micro`.
    let vte = var("VTE_VERSION")
        .and_then(|version| version.trim().parse::<u32>().ok())
        .is_some_and(|version| version >= 5000);
    term.contains("kitty")
        || term.contains("ghostty")
        || term.starts_with("wezterm")
        || term.starts_with("alacritty")
        || term.starts_with("foot")
        || vte
        || [
            "KITTY_WINDOW_ID",
            "GHOSTTY_RESOURCES_DIR",
            "WEZTERM_PANE",
            "WEZTERM_EXECUTABLE",
            "ITERM_SESSION_ID",
            "ALACRITTY_WINDOW_ID",
            "ALACRITTY_SOCKET",
            "GNOME_TERMINAL_SCREEN",
            "WT_SESSION",
        ]
        .into_iter()
        .any(set)
}

/// Whether the name a terminal answered with (`XTVERSION`) is one known to
/// open hyperlinks: `kitty(0.36.4)`, `ghostty 1.1.0`,
/// `WezTerm 20240203-…`, `foot(1.20.2)`, `iTerm2 3.5.11` — a name, then its
/// version, so the name is the answer's first word: whatever follows it is
/// no letter or digit of a longer name.
///
/// The list is the terminals whose answer is known to read this way. Left
/// off on purpose: `libghostty`, which is whatever program embeds it (a
/// herdr pane names itself in `TERM_PROGRAM` instead), `XTerm(…)`, which
/// reads no hyperlink, and `tmux …`, a multiplexer answering for itself.
fn answers_as_a_linking_terminal(answer: &str) -> bool {
    let name = answer.trim_start().to_ascii_lowercase();
    ["kitty", "ghostty", "wezterm", "foot", "iterm2"]
        .into_iter()
        .any(|known| {
            name.strip_prefix(known)
                .is_some_and(|rest| !rest.starts_with(|c: char| c.is_ascii_alphanumeric()))
        })
}

/// Whether a `TERM_PROGRAM` value is the name of a terminal known to open
/// hyperlinks. The spelling is each terminal's own — `iTerm.app`, `WezTerm`,
/// `vscode` — so it is compared with its case, spaces, dots, dashes and
/// underscores taken out.
///
/// Left off on purpose: Apple's Terminal, which does not read the escape;
/// Konsole, which reads it only once its profile allows it; and every name
/// not listed, since a wrong yes hides a URL and a wrong no only keeps it on
/// screen.
fn names_a_linking_terminal(program: &str) -> bool {
    let name: String = program
        .chars()
        .filter(|c| !matches!(c, ' ' | '-' | '_' | '.'))
        .map(|c| c.to_ascii_lowercase())
        .collect();
    matches!(
        name.as_str(),
        "ghostty"
            | "iterm"
            | "iterm2"
            | "itermapp"
            | "wezterm"
            | "kitty"
            | "vscode"
            | "alacritty"
            | "gnometerminal"
            | "vte"
            | "windowsterminal"
            // A herdr pane (`docs/herdr.md` *Links in a pane*).
            | "herdr"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn urls(text: &str) -> Vec<&str> {
        find_urls(text).into_iter().map(|r| &text[r]).collect()
    }

    #[test]
    fn a_text_that_is_one_url_is_whole_whatever_it_ends_on() {
        // Prose detection gives a closing `.`/`!`/`)` back to the sentence
        // around a URL; a text that IS the URL has no sentence, so what it
        // ends on stays part of it (`docs/links.md`).
        for text in [
            "https://as.test/authorize?client_id=c1&state=xyz",
            "https://as.test/authorize?resource=https%3A%2F%2Fmcp.test.",
            "https://as.test/authorize?state=s!",
            "https://as.test/o(auth)/authorize?client_id=c1)",
            "HTTPS://AS.TEST/A",
            "http://127.0.0.1:8080/authorize?x=1",
        ] {
            assert!(is_whole_url(text), "{text:?}");
        }
    }

    #[test]
    fn a_text_with_anything_but_one_url_in_it_is_not_whole() {
        for text in [
            "",
            "https://",
            "see https://as.test/a",
            " https://as.test/a",
            "https://as.test/a and more",
            "https://as.test/a\nnext",
            "https://as.test/a\u{1b}]8;;x",
            "https://as.test/<a>",
            "xhttps://as.test/a",
            "ftp://as.test/a",
            "file:///etc/passwd",
            "javascript:alert(1)",
        ] {
            assert!(!is_whole_url(text), "{text:?}");
        }
    }

    #[test]
    fn code_url_boundaries_are_literal_without_changing_prose_detection() {
        let cases = [
            ("'https://e.test/x!').json()", vec!["https://e.test/x!"]),
            (
                "['https://one.test/a','https://two.test/b']",
                vec!["https://one.test/a", "https://two.test/b"],
            ),
            ("\"https://e.test/x?\"", vec!["https://e.test/x?"]),
            ("`https://e.test/x;`", vec!["https://e.test/x;"]),
            ("  https://e.test/x!  ", vec!["https://e.test/x!"]),
            ("https://e.test/x'", vec!["https://e.test/x'"]),
            ("// see https://e.test/x. next", vec!["https://e.test/x"]),
            ("'https://e.test/x\x1b]8;;bad'", vec!["https://e.test/x"]),
            ("'https://e.test/x\nnext'", vec!["https://e.test/x"]),
            ("'https://'", vec![]),
            ("'nothhttps://e.test/x'", vec![]),
        ];
        for (text, expected) in cases {
            let found: Vec<_> = find_code_urls(text).into_iter().map(|r| &text[r]).collect();
            assert_eq!(found, expected, "{text:?}");
        }
        assert_eq!(urls("https://e.test/x!"), vec!["https://e.test/x"]);
    }

    #[test]
    fn a_forming_url_at_the_end_of_a_line_is_unsettled() {
        // Every prefix of a line ending in a growing URL is unsettled — the
        // next chars could restyle the word — from the first scheme byte on.
        for tail in [
            "h",
            "ht",
            "htt",
            "http",
            "http:",
            "http:/",
            "http://",
            "HTTPS:/",
            "http://e",
            "http://e.",
            "see http://example.com/path",
            "(http://e",
            "x http",
        ] {
            assert!(has_forming_url(tail), "{tail:?} could still grow a URL");
        }
    }

    #[test]
    fn a_terminated_or_impossible_url_is_settled() {
        // A stopper after the URL seals it; a mid-word fragment can never
        // become a scheme (the detector's own boundary rule).
        for tail in [
            "",
            "plain words",
            "blah", // ends in 'h' but mid-word — `xhttp://` is no link
            "see http://example.com/x done", // terminated by the space
            "http://e |", // a delimiter stopper sealed it
            "words myhttp", // 'p' tail mid-word
        ] {
            assert!(!has_forming_url(tail), "{tail:?} is settled");
        }
    }

    #[test]
    fn finds_a_bare_url_mid_sentence() {
        assert_eq!(
            urls("the credit lives at https://github.com/linuztx for real"),
            vec!["https://github.com/linuztx"]
        );
        assert_eq!(
            urls("see http://example.com now"),
            vec!["http://example.com"]
        );
    }

    #[test]
    fn scheme_is_case_insensitive_and_needs_a_boundary() {
        assert_eq!(urls("go HTTPS://X.COM/y"), vec!["HTTPS://X.COM/y"]);
        // Glued to a word it is not a link; after punctuation it is.
        assert_eq!(urls("nothttps://x.com"), Vec::<&str>::new());
        assert_eq!(urls("(https://x.com/y)"), vec!["https://x.com/y"]);
        assert_eq!(urls("→https://x.com/y"), vec!["https://x.com/y"]);
    }

    #[test]
    fn a_bare_scheme_is_not_a_link() {
        assert_eq!(urls("the https:// prefix alone"), Vec::<&str>::new());
        assert_eq!(urls("https://"), Vec::<&str>::new());
    }

    #[test]
    fn trailing_punctuation_stays_prose() {
        assert_eq!(
            urls("at https://github.com/linuztx. That is"),
            vec!["https://github.com/linuztx"]
        );
        assert_eq!(urls("see https://x.com/y, then"), vec!["https://x.com/y"]);
        assert_eq!(urls("really https://x.com/y?!"), vec!["https://x.com/y"]);
        assert_eq!(urls("'https://x.com/y'"), vec!["https://x.com/y"]);
        assert_eq!(
            urls("ends with colon https://x.com/y:"),
            vec!["https://x.com/y"]
        );
    }

    #[test]
    fn parens_balance_inside_the_url() {
        // A Wikipedia-style path keeps its closing paren…
        assert_eq!(
            urls("read https://en.wikipedia.org/wiki/Foo_(bar) today"),
            vec!["https://en.wikipedia.org/wiki/Foo_(bar)"]
        );
        // …while a URL wrapped in prose parens gives the closer back.
        assert_eq!(urls("(see https://x.com/y)"), vec!["https://x.com/y"]);
        assert_eq!(urls("[https://x.com/y]"), vec!["https://x.com/y"]);
    }

    #[test]
    fn delimiters_and_controls_terminate_the_url() {
        assert_eq!(urls("<https://x.com/y> ok"), vec!["https://x.com/y"]);
        assert_eq!(urls("\"https://x.com/y\" ok"), vec!["https://x.com/y"]);
        assert_eq!(urls("a|https://x.com/y|b"), vec!["https://x.com/y"]);
        // An ESC smuggled into model output can never enter a link target.
        assert_eq!(
            urls("https://x.com/y\u{1b}]8;;evil"),
            vec!["https://x.com/y"]
        );
    }

    #[test]
    fn finds_every_url_and_keeps_byte_ranges_exact() {
        let text = "a https://one.example/x then http://two.example/y.";
        let ranges = find_urls(text);
        assert_eq!(
            ranges.iter().map(|r| &text[r.clone()]).collect::<Vec<_>>(),
            vec!["https://one.example/x", "http://two.example/y"]
        );
        // Ranges index the original text (multi-byte safe).
        let unicode = "→ https://x.com/日本語 ←";
        let got = urls(unicode);
        assert_eq!(got, vec!["https://x.com/日本語"]);
    }

    #[test]
    fn scanning_multibyte_text_never_slices_mid_char() {
        // The scheme window must be checked byte-wise: `text[i..i+8]` lands
        // inside '世' (3 bytes) and panics — the bug a CJK table cell hit.
        assert_eq!(urls("世界世界"), Vec::<&str>::new());
        assert_eq!(urls("h世界世界世界"), Vec::<&str>::new());
        assert_eq!(
            urls("日本語 https://x.com/y 日本語"),
            vec!["https://x.com/y"]
        );
    }

    #[test]
    fn interner_hands_the_same_url_the_same_id() {
        let a = linked(Style::new(), "https://stable.example/a");
        let b = linked(Style::new(), "https://stable.example/a");
        let c = linked(Style::new(), "https://stable.example/other");
        assert_eq!(a.underline_color, b.underline_color, "same URL, same id");
        assert_ne!(
            a.underline_color, c.underline_color,
            "different URL, different id"
        );
        assert!(a.underline_color.is_some(), "the carrier is stamped");
    }

    #[test]
    fn carrier_roundtrips_through_the_style() {
        let url = "https://roundtrip.example/path?q=1";
        let style = linked(
            Style::new().add_modifier(ratatui::style::Modifier::BOLD),
            url,
        );
        assert_eq!(style_link(&style).as_deref(), Some(url));
        // The rest of the style is untouched.
        assert!(style.add_modifier.contains(ratatui::style::Modifier::BOLD));
    }

    #[test]
    fn only_a_nonzero_rgb_underline_decodes() {
        assert_eq!(carrier_id(Color::Rgb(0, 0, 0)), None, "id 0 is reserved");
        assert_eq!(carrier_id(Color::Reset), None);
        assert_eq!(carrier_id(Color::Indexed(7)), None);
        assert_eq!(carrier_id(Color::Cyan), None);
        assert_eq!(carrier_id(Color::Rgb(0, 0, 3)), Some(3));
        assert_eq!(carrier_id(Color::Rgb(1, 2, 3)), Some(0x01_02_03));
        // The top bit belongs to the inline-image blocks (`docs/images.md`),
        // so a picture's marker must never decode as a URL here.
        assert_eq!(carrier_id(Color::Rgb(0x80, 0x01, 0x00)), None);
        assert_eq!(carrier_id(Color::Rgb(0xFF, 0xFF, 0xFF)), None);
        assert_eq!(
            carrier_id(Color::Rgb(0x7F, 0xFF, 0xFF)),
            Some(LINK_ID_MAX),
            "and the last id below the flag still does"
        );
    }

    #[test]
    fn an_uninterned_id_looks_up_to_nothing() {
        assert_eq!(link_url(0), None);
        assert_eq!(link_url(LINK_ID_MAX), None, "never interned this run");
        let plain = Style::new().underline_color(Color::Rgb(0xAB, 0xCD, 0xEF));
        assert_eq!(style_link(&plain), None, "a raw colour is not a link");
    }

    #[test]
    fn osc8_frames_open_and_close() {
        let open = osc8_open(7, "https://x.com/y");
        assert_eq!(open, "\x1b]8;id=az7;https://x.com/y\x1b\\");
        assert_eq!(OSC8_CLOSE, "\x1b]8;;\x1b\\");
    }

    #[test]
    fn osc8_percent_encodes_what_a_terminal_could_misparse() {
        // Controls, space, DEL and non-ASCII are encoded; plain ASCII — `%`
        // included, so an already-encoded URL is not double-encoded — passes.
        let open = osc8_open(1, "https://x.com/a b\u{1b}c\u{7f}日%20");
        // The URI field — between the params' `;` and the closing ST — must
        // never hold a raw escape byte, whatever the "URL" text tried.
        let uri = open
            .strip_prefix("\x1b]8;id=az1;")
            .and_then(|s| s.strip_suffix("\x1b\\"))
            .expect("the frame around the URI");
        assert!(!uri.contains('\u{1b}'), "no embedded escape: {uri:?}");
        assert!(open.contains("a%20b%1Bc%7F"), "encoded bytes: {open:?}");
        assert!(open.contains("%E6%97%A5"), "UTF-8 bytes encoded: {open:?}");
        assert!(
            open.ends_with("%20\x1b\\"),
            "the %% passes through: {open:?}"
        );
    }

    #[test]
    fn the_bytes_a_target_takes_in_the_escape_count_its_encoding() {
        assert_eq!(osc8_uri_len("https://x.com/y"), "https://x.com/y".len());
        // A space, a control and each byte of a non-ASCII character are
        // written as three (`osc8_open`'s encoding); `%` passes through.
        assert_eq!(osc8_uri_len("a b"), 5);
        assert_eq!(osc8_uri_len("é"), 6);
        assert_eq!(osc8_uri_len("%20"), 3);
        for url in ["https://x.com/a b\u{1b}c\u{7f}日%20", "https://x.com/y"] {
            let open = osc8_open(1, url);
            let uri = open
                .strip_prefix("\x1b]8;id=az1;")
                .and_then(|rest| rest.strip_suffix("\x1b\\"))
                .expect("the frame around the URI");
            assert_eq!(osc8_uri_len(url), uri.len(), "{url:?}");
        }
    }

    #[test]
    fn hyperlinks_are_on_by_default() {
        assert!(!hyperlinks_disabled(None));
        assert!(!hyperlinks_disabled(Some("")));
        assert!(!hyperlinks_disabled(Some("1")));
        assert!(!hyperlinks_disabled(Some("yes")));
    }

    #[test]
    fn falsy_env_values_disable_hyperlinks() {
        assert!(hyperlinks_disabled(Some("0")));
        assert!(hyperlinks_disabled(Some("false")));
        assert!(hyperlinks_disabled(Some("No")));
        assert!(hyperlinks_disabled(Some(" OFF ")));
    }

    // --- How a labelled link shows its target (docs/links.md) ---

    /// [`detect_target_display`] in an environment of exactly `vars`, on a
    /// terminal the startup read heard nothing from.
    fn display_in(vars: &[(&str, &str)]) -> TargetDisplay {
        display_answering(vars, None)
    }

    /// [`detect_target_display`] in an environment of exactly `vars`, on a
    /// terminal that gave the startup read `answered` as its own name.
    fn display_answering(vars: &[(&str, &str)], answered: Option<&str>) -> TargetDisplay {
        detect_target_display(
            |name| {
                vars.iter()
                    .find(|(key, _)| *key == name)
                    .map(|(_, value)| (*value).to_string())
            },
            answered,
        )
    }

    #[test]
    fn a_terminal_known_to_open_hyperlinks_hides_the_target() {
        // Each names itself in a variable it sets for its own children.
        for vars in [
            &[("TERM", "xterm-kitty")][..],
            &[("TERM", "xterm-256color"), ("KITTY_WINDOW_ID", "3")],
            &[("TERM", "xterm-ghostty")],
            &[
                ("TERM", "xterm-256color"),
                ("GHOSTTY_RESOURCES_DIR", "/usr/share/ghostty"),
            ],
            &[("TERM", "xterm-256color"), ("TERM_PROGRAM", "ghostty")],
            &[("TERM", "xterm-256color"), ("TERM_PROGRAM", "iTerm.app")],
            &[("TERM", "xterm-256color"), ("ITERM_SESSION_ID", "w0t0p0:1")],
            &[("TERM", "xterm-256color"), ("TERM_PROGRAM", "WezTerm")],
            &[("TERM", "xterm-256color"), ("WEZTERM_PANE", "0")],
            &[
                ("TERM", "xterm-256color"),
                ("WEZTERM_EXECUTABLE", "/usr/bin/wezterm-gui"),
            ],
            &[("TERM", "wezterm")],
            &[("TERM", "xterm-256color"), ("TERM_PROGRAM", "vscode")],
            &[("TERM", "alacritty")],
            &[("TERM", "xterm-256color"), ("ALACRITTY_WINDOW_ID", "9")],
            &[("TERM", "foot")],
            &[("TERM", "foot-extra")],
            &[("TERM", "xterm-256color"), ("VTE_VERSION", "7802")],
            &[
                ("TERM", "xterm-256color"),
                ("GNOME_TERMINAL_SCREEN", "/org/gnome/Terminal/screen/1"),
            ],
            &[("TERM", "xterm-256color"), ("WT_SESSION", "f1e2")],
            // A herdr pane: its emulator keeps a hyperlink clickable
            // (docs/herdr.md *Links in a pane*).
            &[("TERM", "xterm-256color"), ("TERM_PROGRAM", "herdr")],
        ] {
            assert_eq!(display_in(vars), TargetDisplay::Hidden, "{vars:?}");
        }
    }

    #[test]
    fn a_terminal_that_cannot_be_named_keeps_the_target_on_screen() {
        // Hiding the target where nothing opens it loses the URL outright, so
        // every case that is not a known yes is a no.
        for vars in [
            &[][..],
            &[("TERM", "xterm-256color")],
            &[("TERM", "dumb")],
            &[("TERM", "linux")],
            &[
                ("TERM", "xterm-256color"),
                ("TERM_PROGRAM", "Apple_Terminal"),
            ],
            &[("TERM", "xterm-256color"), ("TERM_PROGRAM", "WarpTerminal")],
            &[("TERM", "xterm-256color"), ("TERM_PROGRAM", "zed")],
            // A VTE from before it read the escape (0.50).
            &[("TERM", "xterm-256color"), ("VTE_VERSION", "4803")],
            &[("TERM", "xterm-256color"), ("VTE_VERSION", "soon")],
            // Konsole reads the escape only once its profile allows it.
            &[("TERM", "xterm-256color"), ("KONSOLE_VERSION", "230804")],
            // An empty value names nothing.
            &[("TERM", "xterm-256color"), ("KITTY_WINDOW_ID", "")],
            // `LC_TERMINAL` is the one variable of iTerm2's that `ssh`
            // carries — and so the one that reaches a mosh session too,
            // which drops every hyperlink on its way to the terminal. It is
            // not read: over a plain `ssh` iTerm2 answers for itself.
            &[("TERM", "xterm-256color"), ("LC_TERMINAL", "iTerm2")],
            // A variable no terminal sets.
            &[("TERM", "xterm-256color"), ("WEZTERM_VERSION", "20240203")],
        ] {
            assert_eq!(display_in(vars), TargetDisplay::Shown, "{vars:?}");
        }
    }

    #[test]
    fn a_terminal_that_names_itself_at_startup_hides_the_target() {
        // Over ssh the variables a terminal sets for its own children stay
        // behind, and `TERM` is often set down to one the remote host has an
        // entry for. The name the terminal answers the startup read with
        // comes down the very line a hyperlink goes up, so it is this
        // terminal's — each of these is a real answer.
        for name in [
            "kitty(0.36.4)",
            "ghostty 1.1.0",
            "WezTerm 20240203-110809-5046fc22",
            "foot(1.20.2)",
            "iTerm2 3.5.11",
            "  kitty(0.36.4)\n",
        ] {
            assert_eq!(
                display_answering(&[("TERM", "xterm-256color")], Some(name)),
                TargetDisplay::Hidden,
                "{name:?}"
            );
        }
    }

    #[test]
    fn an_answer_naming_no_linking_terminal_leaves_it_to_the_environment() {
        // xterm reads no hyperlink; tmux forwards one only where its
        // configuration says so; `libghostty` is whatever embeds it. An
        // answer can only add a yes — a no stays the environment's to give.
        for name in [
            "XTerm(390)",
            "tmux 3.4",
            "libghostty",
            "Konsole 23.08.5",
            // A name that only opens on a known one is another name.
            "footage(1.0)",
            "kittycat 2",
            "",
            "   ",
        ] {
            assert_eq!(
                display_answering(&[("TERM", "xterm-256color")], Some(name)),
                TargetDisplay::Shown,
                "{name:?}"
            );
            assert_eq!(
                display_answering(
                    &[("TERM", "xterm-256color"), ("TERM_PROGRAM", "vscode")],
                    Some(name)
                ),
                TargetDisplay::Hidden,
                "{name:?}"
            );
        }
    }

    #[test]
    fn the_terminals_answer_outranks_a_name_an_outer_one_left_behind() {
        // `TERM_PROGRAM` is inherited by whatever is started inside a
        // terminal; the answer is given by the one drawing now.
        assert_eq!(
            display_answering(
                &[
                    ("TERM", "xterm-256color"),
                    ("TERM_PROGRAM", "Apple_Terminal"),
                ],
                Some("WezTerm 20240203-110809-5046fc22")
            ),
            TargetDisplay::Hidden
        );
    }

    #[test]
    fn the_terminals_answer_does_not_outrank_what_keeps_the_target() {
        // Everything that keeps the target for a reason the terminal's name
        // does not answer still does: no hyperlink written, the user's own
        // word, a multiplexer in between, an editor's terminal.
        let kitty = Some("kitty(0.36.4)");
        let term = ("TERM", "xterm-256color");
        for vars in [
            &[term, (HYPERLINKS_ENV, "0")][..],
            &[term, (LINK_TARGETS_ENV, "show")],
            &[term, ("TMUX", "/tmp/tmux-0/default,1,0")],
            &[("TERM", "screen-256color")],
            &[term, ("STY", "1234.pts-0.host")],
            &[("TERM", "linux")],
            &[("TERM", "dumb")],
            &[term, ("NVIM", "/run/user/1000/nvim.4242.0")],
            &[term, ("TERMINAL_EMULATOR", "JetBrains-JediTerm")],
        ] {
            assert_eq!(
                display_answering(vars, kitty),
                TargetDisplay::Shown,
                "{vars:?}"
            );
        }
    }

    #[test]
    fn the_terminal_is_asked_its_name_where_the_answer_could_decide() {
        // `ssh` from iTerm2 carries `LC_TERMINAL` and nothing else of the
        // terminal's, and that variable is what makes the picture detection
        // name iTerm2's protocol and skip its own question. So the name is
        // asked on the links' own account wherever the answer could decide:
        // iTerm2 at the other end of the `ssh` says who it is, and a mosh in
        // between, which passes on no hyperlink, passes on no answer either.
        let asks = |vars: &[(&str, &str)]| {
            wants_terminal_name(|name| {
                vars.iter()
                    .find(|(key, _)| *key == name)
                    .map(|(_, value)| (*value).to_string())
            })
        };
        for vars in [
            &[("TERM", "xterm-256color"), ("LC_TERMINAL", "iTerm2")][..],
            &[("TERM", "xterm-256color")],
            &[],
            // The answer outranks a name the environment does not know.
            &[
                ("TERM", "xterm-256color"),
                ("TERM_PROGRAM", "SomeNewTerminal"),
            ],
        ] {
            assert!(asks(vars), "{vars:?}");
        }
        // Nothing to ask where the environment has already named a terminal
        // that opens hyperlinks, or where something ahead of the answer keeps
        // the target on screen whatever it says.
        for vars in [
            &[("TERM", "xterm-kitty")][..],
            &[("TERM", "xterm-256color"), ("TERM_PROGRAM", "iTerm.app")],
            &[("TERM", "xterm-256color"), ("WEZTERM_PANE", "0")],
            &[("TERM", "xterm-256color"), ("TERM_PROGRAM", "herdr")],
            &[
                ("TERM", "tmux-256color"),
                ("TMUX", "/tmp/tmux-0/default,1,0"),
                ("LC_TERMINAL", "iTerm2"),
            ],
            &[("TERM", "xterm-256color"), ("STY", "1234.pts-0.host")],
            &[("TERM", "linux")],
            &[("TERM", "dumb")],
            &[
                ("TERM", "xterm-256color"),
                ("NVIM", "/run/user/1000/nvim.4242.0"),
            ],
            &[("TERM", "xterm-256color"), (HYPERLINKS_ENV, "0")],
            &[("TERM", "xterm-256color"), (LINK_TARGETS_ENV, "show")],
            &[("TERM", "xterm-256color"), (LINK_TARGETS_ENV, "hide")],
        ] {
            assert!(!asks(vars), "{vars:?}");
        }
    }

    #[test]
    fn lc_terminal_alone_never_hides_the_target() {
        // `LC_TERMINAL` crosses `ssh` and so reaches a mosh session too,
        // where no hyperlink gets through. The answer decides instead.
        let lc = [("TERM", "xterm-256color"), ("LC_TERMINAL", "iTerm2")];
        assert_eq!(display_in(&lc), TargetDisplay::Shown);
        assert_eq!(
            display_answering(&lc, Some("iTerm2 3.5.11")),
            TargetDisplay::Hidden
        );
    }

    #[test]
    fn a_multiplexer_keeps_the_target_whatever_terminal_started_it() {
        // tmux forwards a hyperlink only to a terminal its own configuration
        // says takes one, and screen never does — and the variables the outer
        // terminal exported are still in the pane's environment.
        for vars in [
            &[
                ("TERM", "tmux-256color"),
                ("TMUX", "/tmp/tmux-0/default,1,0"),
                ("KITTY_WINDOW_ID", "3"),
            ][..],
            &[("TERM", "xterm-kitty"), ("TMUX", "/tmp/tmux-0/default,1,0")],
            &[("TERM", "screen-256color"), ("TERM_PROGRAM", "iTerm.app")],
            &[("TERM", "xterm-kitty"), ("STY", "1234.pts-0.host")],
            &[("TERM", "xterm-ghostty"), ("ZELLIJ", "0")],
            &[
                ("TERM", "tmux-256color"),
                ("TERM_PROGRAM", "tmux"),
                ("WEZTERM_PANE", "0"),
            ],
        ] {
            assert_eq!(display_in(vars), TargetDisplay::Shown, "{vars:?}");
        }
    }

    #[test]
    fn a_terminal_naming_itself_outranks_what_an_outer_one_left_behind() {
        // A terminal started from inside another inherits that one's
        // variables. `TERM_PROGRAM` is the name of the one actually drawing,
        // so an unknown name is unknown however much kitty is still around.
        assert_eq!(
            display_in(&[
                ("TERM", "xterm-256color"),
                ("TERM_PROGRAM", "SomeNewTerminal"),
                ("KITTY_WINDOW_ID", "3"),
            ]),
            TargetDisplay::Shown
        );
        assert_eq!(
            display_in(&[
                ("TERM", "xterm-256color"),
                ("TERM_PROGRAM", "Apple_Terminal"),
                ("VTE_VERSION", "7802"),
            ]),
            TargetDisplay::Shown
        );
        // And a known one decides the other way just as firmly.
        assert_eq!(
            display_in(&[
                ("TERM", "xterm-256color"),
                ("TERM_PROGRAM", "vscode"),
                ("KONSOLE_VERSION", "230804"),
            ]),
            TargetDisplay::Hidden
        );
        // Spelling is the terminal's own: case, dots, dashes and spaces vary.
        for name in [
            "iTerm.app",
            "iterm2",
            "WezTerm",
            "wezterm",
            "VSCode",
            "Ghostty",
        ] {
            assert_eq!(
                display_in(&[("TERM", "xterm-256color"), ("TERM_PROGRAM", name)]),
                TargetDisplay::Hidden,
                "{name}"
            );
        }
    }

    #[test]
    fn an_editors_own_terminal_is_not_the_one_the_editor_was_started_from() {
        // An editor started from kitty hands kitty's variables to the
        // terminal it embeds — which draws for itself, names itself in no
        // `TERM_PROGRAM`, and may open no hyperlink at all. It is the
        // innermost thing drawing, so it outranks every name around it.
        for marker in [
            ("NVIM", "/run/user/1000/nvim.4242.0"),
            ("VIM_TERMINAL", "901"),
            ("INSIDE_EMACS", "29.4,vterm"),
            ("TERMINAL_EMULATOR", "JetBrains-JediTerm"),
        ] {
            for outer in [
                ("KITTY_WINDOW_ID", "3"),
                ("TERM_PROGRAM", "vscode"),
                ("VTE_VERSION", "7802"),
                ("TERM_PROGRAM", "iTerm.app"),
            ] {
                assert_eq!(
                    display_in(&[("TERM", "xterm-256color"), outer, marker]),
                    TargetDisplay::Shown,
                    "{marker:?} inside {outer:?}"
                );
            }
            // The override is still the user's word.
            assert_eq!(
                display_in(&[
                    ("TERM", "xterm-256color"),
                    marker,
                    (LINK_TARGETS_ENV, "hide"),
                ]),
                TargetDisplay::Hidden,
                "{marker:?}"
            );
        }
        // An empty marker marks nothing.
        assert_eq!(
            display_in(&[("TERM", "xterm-kitty"), ("NVIM", "")]),
            TargetDisplay::Hidden
        );
    }

    #[test]
    fn the_override_decides_for_any_terminal() {
        let kitty = ("TERM", "xterm-kitty");
        let tmux = ("TMUX", "/tmp/tmux-0/default,1,0");
        assert_eq!(
            display_in(&[kitty, tmux, (LINK_TARGETS_ENV, "hide")]),
            TargetDisplay::Hidden,
            "a tmux configured to forward hyperlinks is the user's to say"
        );
        assert_eq!(
            display_in(&[kitty, (LINK_TARGETS_ENV, "show")]),
            TargetDisplay::Shown
        );
        assert_eq!(
            display_in(&[("TERM", "xterm-256color"), (LINK_TARGETS_ENV, " Hide ")]),
            TargetDisplay::Hidden,
            "trimmed, any case"
        );
        // Anything else leaves the terminal to decide.
        for value in ["", "auto", "1", "yes"] {
            assert_eq!(
                display_in(&[kitty, (LINK_TARGETS_ENV, value)]),
                TargetDisplay::Hidden,
                "{value:?}"
            );
            assert_eq!(
                display_in(&[("TERM", "xterm-256color"), (LINK_TARGETS_ENV, value)]),
                TargetDisplay::Shown,
                "{value:?}"
            );
        }
    }

    #[test]
    fn nothing_hides_behind_a_hyperlink_that_is_never_written() {
        // `ALTER_ZERO_HYPERLINKS=0` writes no escape at all, so a hidden
        // target would be a URL nobody can reach — the gate outranks even an
        // explicit `hide`.
        let kitty = ("TERM", "xterm-kitty");
        assert_eq!(
            display_in(&[kitty, (HYPERLINKS_ENV, "0")]),
            TargetDisplay::Shown
        );
        assert_eq!(
            display_in(&[kitty, (HYPERLINKS_ENV, "off"), (LINK_TARGETS_ENV, "hide")]),
            TargetDisplay::Shown
        );
        assert_eq!(
            display_in(&[kitty, (HYPERLINKS_ENV, "1")]),
            TargetDisplay::Hidden
        );
    }

    #[test]
    fn the_target_is_shown_until_someone_says_the_terminal_opens_links() {
        // The renderer's own default: a caller that never names a terminal —
        // a test, the export, a headless run — never hides the only URL.
        assert_eq!(target_display(), TargetDisplay::Shown);
        assert_eq!(TargetDisplay::default(), TargetDisplay::Shown);
    }

    #[test]
    fn a_scoped_target_display_ends_with_its_scope() {
        let inside = with_target_display(TargetDisplay::Hidden, || {
            // A scope inside a scope restores the outer one, not the default.
            let nested = with_target_display(TargetDisplay::Shown, target_display);
            (nested, target_display())
        });
        assert_eq!(inside, (TargetDisplay::Shown, TargetDisplay::Hidden));
        assert_eq!(target_display(), TargetDisplay::Shown, "restored");
        set_target_display(TargetDisplay::Hidden);
        assert_eq!(target_display(), TargetDisplay::Hidden, "set until changed");
        set_target_display(TargetDisplay::Shown);
    }

    #[test]
    fn a_carrier_stamps_what_linked_stamps() {
        let url = "https://carrier.example/one?two=3";
        let style = Style::new().add_modifier(ratatui::style::Modifier::ITALIC);
        let carrier = carrier(url).expect("an interned target");
        assert_eq!(carrier.stamp(style), linked(style, url));
        assert_eq!(style_link(&carrier.stamp(style)).as_deref(), Some(url));
        // One carrier marks any number of styles with the same target.
        let other = Style::new().add_modifier(ratatui::style::Modifier::BOLD);
        assert_eq!(
            carrier.stamp(other).underline_color,
            carrier.stamp(style).underline_color
        );
    }

    // --- `file://` targets for the file tool headers (docs/links.md) ---

    #[test]
    fn an_absolute_path_becomes_a_file_url() {
        assert_eq!(
            file_url("/tmp/notes.txt", None).as_deref(),
            Some("file:///tmp/notes.txt")
        );
        // The cwd is irrelevant to an absolute path.
        assert_eq!(
            file_url("/tmp/notes.txt", Some(Path::new("/home/u/proj"))).as_deref(),
            Some("file:///tmp/notes.txt")
        );
    }

    #[test]
    fn a_relative_path_resolves_against_the_cwd_and_needs_one() {
        let cwd = Path::new("/home/linuztx/Codes/tests");
        assert_eq!(
            file_url("hello.py", Some(cwd)).as_deref(),
            Some("file:///home/linuztx/Codes/tests/hello.py")
        );
        assert_eq!(
            file_url("./src/../hello.py", Some(cwd)).as_deref(),
            Some("file:///home/linuztx/Codes/tests/hello.py")
        );
        assert_eq!(
            file_url("../sib/f.txt", Some(cwd)).as_deref(),
            Some("file:///home/linuztx/Codes/sib/f.txt")
        );
        assert_eq!(
            file_url("hello.py", None),
            None,
            "nothing to resolve against"
        );
        assert_eq!(
            file_url("hello.py", Some(Path::new("relative"))),
            None,
            "a relative cwd is no anchor either"
        );
        assert_eq!(file_url("", Some(cwd)), None, "no path, no link");
    }

    #[test]
    fn a_file_url_is_normalized_lexically() {
        assert_eq!(
            file_url("/tmp/./a/../x.py", None).as_deref(),
            Some("file:///tmp/x.py")
        );
        assert_eq!(
            file_url("/tmp/dir/", None).as_deref(),
            Some("file:///tmp/dir")
        );
        assert_eq!(file_url("/", None).as_deref(), Some("file:///"));
        assert_eq!(
            file_url("/../..", None).as_deref(),
            Some("file:///"),
            "a climb saturates at the root"
        );
    }

    #[test]
    fn a_file_url_percent_encodes_all_but_the_unreserved_bytes() {
        // `Path.as_uri()`'s rule: `/` and `A-Za-z0-9-._~` pass, every other
        // byte — space, `#`, `?`, `%`, each UTF-8 byte of a non-ASCII char —
        // is `%XX`, so the terminal decodes back to the exact file name and a
        // `#` or `?` in it can never read as a fragment or a query.
        assert_eq!(
            file_url("/tmp/my file#1?.txt", None).as_deref(),
            Some("file:///tmp/my%20file%231%3F.txt")
        );
        assert_eq!(
            file_url("/tmp/100%.txt", None).as_deref(),
            Some("file:///tmp/100%25.txt")
        );
        assert_eq!(
            file_url("/tmp/café.txt", None).as_deref(),
            Some("file:///tmp/caf%C3%A9.txt")
        );
        assert_eq!(
            file_url("/home/u/a-b_c.d~e", None).as_deref(),
            Some("file:///home/u/a-b_c.d~e")
        );
    }

    #[test]
    fn a_file_url_rides_the_osc8_frame_without_double_encoding() {
        let url = file_url("/tmp/my file.txt", None).expect("an absolute path links");
        assert_eq!(
            osc8_open(3, &url),
            "\x1b]8;id=az3;file:///tmp/my%20file.txt\x1b\\"
        );
    }
}
