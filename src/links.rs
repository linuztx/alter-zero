//! Clickable links: bare-URL detection, the URL interner, the per-cell link
//! carrier, and the OSC 8 escape framing (see `docs/links.md`).
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
//! Everything here is pure except the interner — a process-global,
//! append-only cache (the `highlight` grammar-singleton precedent): render
//! state, not app state, so a reflow re-rendering from history hands the same
//! URL the same id.

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
    match intern(url) {
        Some(id) => style.underline_color(Color::Rgb((id >> 16) as u8, (id >> 8) as u8, id as u8)),
        None => style,
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

// --- `file://` targets ---

/// The `file://` URI of `path` — the target a `● Read/Write/Edit({path})`
/// header's path carries (`docs/links.md` *The file tool header*), so a
/// click opens the file however the row showed it. A relative `path`
/// resolves against `cwd` first (the session's — `app::PathDisplay`); `None`
/// when it cannot be placed: a relative path with no absolute `cwd`, or an
/// empty one. The path is normalized lexically the way `header_path` reads
/// it (`.` dropped, `..` collapsed and saturating at the root, symlinks never
/// consulted). [`url::Url::from_file_path`] encodes the native path without
/// losing Unix path bytes, preserving Windows drive and UNC semantics (also
/// for their verbatim forms), and refusing device namespaces. Spaces, `#`,
/// `?`, `%` and non-ASCII bytes are encoded so nothing in a file name can
/// become a URL delimiter. [`osc8_open`] passes `%` through, so the two
/// encodings compose.
#[must_use]
pub fn file_url(path: &str, cwd: Option<&Path>) -> Option<String> {
    if path.is_empty() {
        return None;
    }
    let given = Path::new(path);
    // `C:notes.txt` depends on a drive's hidden current directory, not this
    // session's cwd. Do not guess even when that cwd happens to use C:.
    if !given.is_absolute() && matches!(given.components().next(), Some(Component::Prefix(_))) {
        return None;
    }
    let resolved = if given.is_absolute() {
        given.to_path_buf()
    } else {
        cwd.filter(|cwd| cwd.is_absolute())?.join(given)
    };
    let mut normalized = std::path::PathBuf::new();
    for component in resolved.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                // pop() cannot climb past a drive/UNC root (or Unix's `/`).
                normalized.pop();
            }
            component => normalized.push(component.as_os_str()),
        }
    }
    url::Url::from_file_path(normalized).ok().map(Into::into)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn urls(text: &str) -> Vec<&str> {
        find_urls(text).into_iter().map(|r| &text[r]).collect()
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

    // --- `file://` targets for the file tool headers (docs/links.md) ---

    // The same normalization/encoding matrix runs on both native path APIs.
    // Windows absolute files need a drive; `/tmp` alone is drive-relative there.
    fn absolute_file_fixture(path: &str) -> String {
        if cfg!(windows) {
            format!("C:{path}")
        } else {
            path.to_string()
        }
    }

    fn expected_file_fixture(url: &str) -> String {
        if cfg!(windows) {
            url.replacen("file:///", "file:///C:/", 1)
        } else {
            url.to_string()
        }
    }

    #[test]
    fn an_absolute_path_becomes_a_file_url() {
        let path = absolute_file_fixture("/tmp/notes.txt");
        let cwd = absolute_file_fixture("/home/u/proj");
        let expected = expected_file_fixture("file:///tmp/notes.txt");
        assert_eq!(file_url(&path, None).as_deref(), Some(expected.as_str()));
        // The cwd is irrelevant to an absolute path.
        assert_eq!(
            file_url(&path, Some(Path::new(&cwd))).as_deref(),
            Some(expected.as_str())
        );
    }

    #[test]
    fn a_relative_path_resolves_against_the_cwd_and_needs_one() {
        let cwd = absolute_file_fixture("/home/linuztx/Codes/tests");
        let cwd = Path::new(&cwd);
        for (path, expected) in [
            ("hello.py", "file:///home/linuztx/Codes/tests/hello.py"),
            (
                "./src/../hello.py",
                "file:///home/linuztx/Codes/tests/hello.py",
            ),
            ("../sib/f.txt", "file:///home/linuztx/Codes/sib/f.txt"),
        ] {
            assert_eq!(
                file_url(path, Some(cwd)),
                Some(expected_file_fixture(expected))
            );
        }
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
        for (path, expected) in [
            ("/tmp/./a/../x.py", "file:///tmp/x.py"),
            ("/tmp/dir/", "file:///tmp/dir"),
            ("/", "file:///"),
            ("/../..", "file:///"),
        ] {
            assert_eq!(
                file_url(&absolute_file_fixture(path), None),
                Some(expected_file_fixture(expected)),
                "{path} (a climb saturates at the root)"
            );
        }
    }

    #[test]
    fn a_file_url_percent_encodes_all_but_the_unreserved_bytes() {
        // `Path.as_uri()`'s rule: `/` and `A-Za-z0-9-._~` pass, every other
        // byte — space, `#`, `?`, `%`, each UTF-8 byte of a non-ASCII char —
        // is `%XX`, so the terminal decodes back to the exact file name and a
        // `#` or `?` in it can never read as a fragment or a query.
        for (path, expected) in [
            ("/tmp/my file#1?.txt", "file:///tmp/my%20file%231%3F.txt"),
            ("/tmp/100%.txt", "file:///tmp/100%25.txt"),
            ("/tmp/café.txt", "file:///tmp/caf%C3%A9.txt"),
            ("/home/u/a-b_c.d~e", "file:///home/u/a-b_c.d~e"),
        ] {
            assert_eq!(
                file_url(&absolute_file_fixture(path), None),
                Some(expected_file_fixture(expected))
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_file_url_preserves_non_utf8_bytes_in_its_cwd() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;
        let cwd = Path::new(OsStr::from_bytes(b"/tmp/project-\xff"));
        assert_eq!(
            file_url("./sub/../notes.txt", Some(cwd)).as_deref(),
            Some("file:///tmp/project-%FF/notes.txt")
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_file_urls_handle_drives_and_relative_paths_lexically() {
        let cwd = Path::new(r"C:\Users\Me\project");
        for (path, expected) in [
            (
                r"C:\Users\Me\my file#1.txt",
                "file:///C:/Users/Me/my%20file%231.txt",
            ),
            (r"C:\a\.\b\..\notes.txt", "file:///C:/a/notes.txt"),
            (r"C:\..\..\notes.txt", "file:///C:/notes.txt"),
            (
                r".\src\..\notes.txt",
                "file:///C:/Users/Me/project/notes.txt",
            ),
            (r"..\notes.txt", "file:///C:/Users/Me/notes.txt"),
            (r"\notes.txt", "file:///C:/notes.txt"),
            (r"C:\", "file:///C:/"),
        ] {
            assert_eq!(
                file_url(path, Some(cwd)).as_deref(),
                Some(expected),
                "{path}"
            );
        }
        assert_eq!(
            file_url(r"C:notes.txt", Some(cwd)),
            None,
            "drive-relative is ambiguous"
        );
        assert_eq!(file_url(r"D:notes.txt", Some(cwd)), None);
        assert_eq!(
            file_url(r"\notes.txt", None),
            None,
            "a root without a drive needs a cwd"
        );
        assert_eq!(file_url("notes.txt", None), None);
        assert_eq!(file_url("", Some(cwd)), None);
    }

    #[cfg(windows)]
    #[test]
    fn windows_unc_and_verbatim_files_are_urls_not_device_names() {
        for (path, expected) in [
            (
                r"\\server\share\dir\..\my file.txt",
                "file://server/share/my%20file.txt",
            ),
            (
                r"\\server\share\..\..\notes.txt",
                "file://server/share/notes.txt",
            ),
            (r"\\?\C:\dir\.\sub\..\notes.txt", "file:///C:/dir/notes.txt"),
            (
                r"\\?\UNC\server\share\dir\..\notes.txt",
                "file://server/share/notes.txt",
            ),
        ] {
            assert_eq!(file_url(path, None).as_deref(), Some(expected), "{path}");
        }
        for path in [
            r"\\.\COM1",
            r"\\.\C:\notes.txt",
            r"\\?\GLOBALROOT\Device\HarddiskVolume1\notes.txt",
        ] {
            assert_eq!(file_url(path, None), None, "not a normal file: {path}");
        }
    }

    #[test]
    fn a_file_url_rides_the_osc8_frame_without_double_encoding() {
        let url = file_url(&absolute_file_fixture("/tmp/my file.txt"), None)
            .expect("an absolute path links");
        let expected = expected_file_fixture("file:///tmp/my%20file.txt");
        assert_eq!(
            osc8_open(3, &url),
            format!("\x1b]8;id=az3;{expected}\x1b\\")
        );
    }
}
