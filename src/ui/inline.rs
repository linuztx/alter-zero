//! Inline-span rendering: one markdown line's `**bold**`, `` `code` ``, links
//! and emphasis turned into styled [`Span`]s, and the word-wrap that keeps those
//! spans intact across a row boundary. See `docs/markdown.md`.

use crate::links;

use super::theme::*;
use super::wrap::cols;
use super::*;

/// Flatten a parsed inline tree ([`markdown::parse_inline`]) into styled text
/// segments under `base`. Emphasis adds a modifier, `code` a colour, an image
/// its alt text; a `[text](url)` link is [`link_spans`]' — its label alone
/// where the terminal opens hyperlinks, `text (url)` anywhere else. Nesting
/// composes styles. URLs — a link's target and every bare `http(s)://…` in
/// plain text — additionally carry the link **carrier** ([`links::linked`]),
/// so each wrapped fragment still opens the whole target at the paint
/// boundary (`docs/links.md`). Code URLs carry targets too, without changing
/// their literal text or code styling.
pub(super) fn inline_spans(nodes: &[markdown::Inline], base: Style) -> Vec<(String, Style)> {
    let mut out = Vec::new();
    for node in nodes {
        match node {
            markdown::Inline::Text(t) => autolink_text(t, base, &mut out),
            markdown::Inline::Bold(inner) => {
                out.extend(inline_spans(inner, base.add_modifier(Modifier::BOLD)));
            }
            markdown::Inline::Italic(inner) => {
                out.extend(inline_spans(inner, base.add_modifier(Modifier::ITALIC)));
            }
            markdown::Inline::Strike(inner) => {
                out.extend(inline_spans(
                    inner,
                    base.add_modifier(Modifier::CROSSED_OUT),
                ));
            }
            markdown::Inline::Code(c) => {
                out.extend(linkify_code_segments(vec![(
                    c.clone(),
                    base.fg(inline_code_color()),
                )]));
            }
            markdown::Inline::Link { text, url } => link_spans(text, url, base, &mut out),
            markdown::Inline::Image { alt } => out.push((alt.clone(), base)),
            markdown::Inline::Url { text, target } => {
                // An autolink shows its bare URL (the `<>` dropped) in the
                // link dress, carrying its target like a bare URL in prose.
                out.push((text.clone(), links::linked(link_dress(base), target)));
            }
        }
    }
    out
}

/// The link dress over `style`: the link colour, underlined — what a bare
/// URL, a shown target and a label standing in for its target all wear.
fn link_dress(style: Style) -> Style {
    style
        .fg(link_url_color())
        .add_modifier(Modifier::UNDERLINED)
}

/// Append a `[text](url)` link's segments under `base` (`docs/links.md`
/// *Labelled links*), in one of two presentations:
///
/// - **The label alone** ([`hides_target`]): every run of it in the link
///   dress over whatever emphasis it carries, the target behind it as the
///   hyperlink — `Source: FIDE`, not `Source: FIDE (https://www.fide.com/…)`.
/// - **`text (url)`** everywhere else: the label in its own dress, then the
///   target in the link dress between parentheses. The parens are decoration
///   this renderer adds, not link text — they take neither the dress nor the
///   carrier, so what the eye reads as the link is exactly what a click opens
///   (`docs/links.md` *The decoration parens*).
///
/// Two links read the same in both, since the target after the label would
/// add nothing: one with **no label to see** — empty, or only spaces or
/// zero-width characters — is its target, in the link dress; and one whose
/// label **is** its web address ([`is_own_target`]) is written once.
///
/// Either way every run carries the target, stamped from one
/// [`links::Carrier`] rather than interned again per run.
fn link_spans(text: &[markdown::Inline], url: &str, base: Style, out: &mut Vec<(String, Style)>) {
    let carrier = links::carrier(url);
    let mark = |style: Style| carrier.map_or(style, |carrier| carrier.stamp(style));
    let label = inline_spans(text, base);
    if !label.iter().any(|(text, _)| cols(text.trim()) > 0) {
        out.push((url.to_string(), mark(link_dress(base))));
        return;
    }
    if hides_target(&label, url, carrier) {
        out.extend(label.into_iter().map(|(t, s)| (t, mark(link_dress(s)))));
        return;
    }
    if is_own_target(&label, url) {
        // The address is the label: it is the link, however it was written.
        out.extend(label.into_iter().map(|(t, s)| (t, mark(link_dress(s)))));
        return;
    }
    out.extend(label.into_iter().map(|(t, s)| (t, mark(s))));
    out.push((" (".to_string(), base));
    out.push((url.to_string(), mark(link_dress(base))));
    out.push((")".to_string(), base));
}

/// Whether a link's target goes behind its `label` — one with something in
/// it to click — rather than after it. Only where the session's terminal
/// opens hyperlinks ([`links::TargetDisplay::Hidden`]), and then only when
/// all of these hold — each the case where hiding it would cost the reader
/// something:
///
/// - the link is **marked** (`carrier`): with no hyperlink behind the label
///   a hidden address is one nobody can reach;
/// - the target is a **web address** (`http://`/`https://`, whole): a path,
///   an anchor or another scheme is the information itself, and nothing here
///   can promise that clicking one opens anything;
/// - the target is one a hyperlink **can carry**
///   ([`links::HYPERLINK_URI_MAX_BYTES`]): a terminal that drops an
///   over-long hyperlink would leave the label with nothing behind it;
/// - the label does not **show another address** ([`shows_another_address`]):
///   a hidden target is taken on trust, and a label reading
///   `https://bank.example` over a link to somewhere else is the one place
///   that trust is abused.
pub(super) fn hides_target(
    label: &[(String, Style)],
    url: &str,
    carrier: Option<links::Carrier>,
) -> bool {
    links::target_display() == links::TargetDisplay::Hidden
        && carrier.is_some()
        && links::is_whole_url(url)
        && links::osc8_uri_len(url) <= links::HYPERLINK_URI_MAX_BYTES
        && !shows_another_address(label, url)
}

/// Whether `label` spells an address that is not `url`. Read two ways, since
/// either alone can be written around:
///
/// - off the label's text **as the reader sees it** — its runs joined into
///   one string, with every character that takes no column left out. A
///   label can spell `https://bank.example` with the scheme split by
///   emphasis or a code span, in an image's alt text, or around a
///   zero-width space: no single run holds an address then, and on screen
///   the label reads as one all the same;
/// - off the **carriers** its runs already wear — an autolinked address
///   (`<me@bank.example>`, a `mailto:`) is no `http(s)://` text to find, but
///   it was marked with its own target as the label rendered.
fn shows_another_address(label: &[(String, Style)], url: &str) -> bool {
    let seen: String = label
        .iter()
        .flat_map(|(text, _)| text.chars())
        .filter(|c| cols(c.encode_utf8(&mut [0; 4])) > 0)
        .collect();
    links::find_urls(&seen)
        .into_iter()
        .any(|address| !same_page(&seen[address], url))
        || label
            .iter()
            .any(|(_, style)| links::style_link(style).is_some_and(|shown| !same_page(&shown, url)))
}

/// Whether `label` is, whole, the web address it links to —
/// `[https://x.com](https://x.com/)` — so that the target written after it
/// would be the same address a second time. A label that only *holds* its
/// target (`see https://x.com now`) is a label, and a target that is no web
/// address keeps its `name (target)` shape whatever the name is.
fn is_own_target(label: &[(String, Style)], url: &str) -> bool {
    // The two can differ by a closing `/` at most, so most labels are told
    // apart by their length alone, before any text is put together.
    let len: usize = label.iter().map(|(text, _)| text.len()).sum();
    if len.abs_diff(url.len()) > 1 || !links::is_whole_url(url) {
        return false;
    }
    let text: String = label.iter().map(|(text, _)| text.as_str()).collect();
    same_page(&text, url)
}

/// Whether two addresses are one page as written: equal, or apart only by a
/// closing `/` — `https://x.com` over a link to `https://x.com/`.
fn same_page(a: &str, b: &str) -> bool {
    a.trim_end_matches('/') == b.trim_end_matches('/')
}

/// Append a plain-text node's segments under `base`, autolinking bare URLs
/// (`docs/links.md`): each detected URL takes the markdown-target dress —
/// [`link_url_color`] + underline, a URL is a URL — plus the carrier that
/// keeps every wrapped fragment opening the whole target; the prose around it
/// is untouched.
fn autolink_text(text: &str, base: Style, out: &mut Vec<(String, Style)>) {
    let mut at = 0;
    for range in links::find_urls(text) {
        if range.start > at {
            out.push((text[at..range.start].to_string(), base));
        }
        let url = &text[range.clone()];
        out.push((url.to_string(), links::linked(link_dress(base), url)));
        at = range.end;
    }
    if at < text.len() {
        out.push((text[at..].to_string(), base));
    }
}

/// Mark literal code URLs, respecting quote boundaries and preserving trailing
/// punctuation when the literal itself establishes the URL's extent.
pub(super) fn linkify_code_segments(segments: Vec<(String, Style)>) -> Vec<(String, Style)> {
    linkify_with(segments, links::find_code_urls)
}

/// Intersect source URL ranges with **unwrapped**, already-styled runs without
/// changing their bytes or dress, independent of the detector's prose/code
/// boundary rules. Detection sees the whole line, not individual
/// syntax-highlighted segments: a highlighter can split a URL at `://`,
/// punctuation, or query parameters. Every intersecting piece carries the full
/// target before a wrapper breaks it into display rows. Segments must belong to
/// one source line; separate lines are never joined.
fn linkify_with(
    segments: Vec<(String, Style)>,
    detect: fn(&str) -> Vec<std::ops::Range<usize>>,
) -> Vec<(String, Style)> {
    let text: String = segments.iter().map(|(text, _)| text.as_str()).collect();
    let ranges = detect(&text);
    if ranges.is_empty() {
        return segments;
    }
    let mut ranges = ranges.into_iter().peekable();
    let mut out = Vec::with_capacity(segments.len());
    let mut offset = 0;
    for (content, style) in segments {
        if content.is_empty() {
            out.push((content, style));
            continue;
        }
        let end = offset + content.len();
        let mut at = offset;
        while let Some(range) = ranges.peek() {
            if range.start >= end {
                break;
            }
            let start = range.start.max(at);
            if at < start {
                out.push((text[at..start].to_string(), style));
            }
            let stop = range.end.min(end);
            out.push((
                text[start..stop].to_string(),
                links::linked(style, &text[range.clone()]),
            ));
            at = stop;
            if range.end <= end {
                ranges.next();
            } else {
                break;
            }
        }
        if at < end {
            out.push((text[at..end].to_string(), style));
        }
        offset = end;
    }
    out
}

/// Word-wrap styled inline `segments` to `width` columns, preserving each run's
/// style across wraps — the span-aware counterpart of [`wrap_text`]. Words (runs
/// of non-whitespace, which may span several styled pieces, e.g. `un**bold**`)
/// stay intact where they fit; a word wider than `width` hard-breaks on grapheme
/// boundaries; whitespace collapses to single spaces between words — plain
/// ones, except between two words of one link, where the space is the link's
/// ([`Word::joiner`]). An empty line yields one empty row (matching
/// [`wrap_text`]).
pub(super) fn wrap_inline(segments: &[(String, Style)], width: u16) -> Vec<Vec<Span<'static>>> {
    wrap_inline_hanging(segments, width, width)
}

/// [`wrap_inline`] with a **hanging indent**: the first row is wrapped to
/// `first` columns — the room left beside a lead the caller prints on that row
/// (a tool header's `● {name}`) — and every continuation row to `rest`, the
/// room left beside the indent they are printed at. Equal widths are exactly
/// [`wrap_inline`]. See [`tool_header_lines`](super::tool::tool_header_lines).
pub(super) fn wrap_inline_hanging(
    segments: &[(String, Style)],
    first: u16,
    rest: u16,
) -> Vec<Vec<Span<'static>>> {
    let words = tokenize_words(segments);
    let to_spans = |rows: Vec<Vec<(String, Style)>>| -> Vec<Vec<Span<'static>>> {
        rows.into_iter()
            .map(|r| r.into_iter().map(|(t, s)| Span::styled(t, s)).collect())
            .collect()
    };
    if first == 0 || rest == 0 {
        // No wrapping: one row, words rejoined by single spaces.
        let mut row: Vec<(String, Style)> = Vec::new();
        for (i, word) in words.iter().enumerate() {
            if i > 0 {
                push_piece(&mut row, " ", word.joiner);
            }
            for (t, s) in &word.pieces {
                push_piece(&mut row, t, *s);
            }
        }
        return to_spans(vec![row]);
    }
    let (first, rest) = (first as usize, rest as usize);
    let mut rows: Vec<Vec<(String, Style)>> = Vec::new();
    let mut row: Vec<(String, Style)> = Vec::new();
    let mut row_w = 0usize;
    for word in &words {
        // The row being filled is the first one until one has been pushed.
        let width = if rows.is_empty() { first } else { rest };
        let ww: usize = word.pieces.iter().map(|(t, _)| cols(t)).sum();
        if ww > width {
            // A word too wide for any line: hard-break it grapheme by grapheme.
            if row_w > 0 {
                rows.push(std::mem::take(&mut row));
                row_w = 0;
            }
            for (t, s) in &word.pieces {
                for g in t.graphemes(true) {
                    let gw = cols(g);
                    // Re-read the budget per grapheme: the break may have just
                    // left the (possibly narrower) first row.
                    let width = if rows.is_empty() { first } else { rest };
                    if row_w > 0 && row_w + gw > width {
                        rows.push(std::mem::take(&mut row));
                        row_w = 0;
                    }
                    push_piece(&mut row, g, *s);
                    row_w += gw;
                }
            }
            continue;
        }
        let needed = if row_w == 0 { ww } else { row_w + 1 + ww };
        if needed > width {
            rows.push(std::mem::take(&mut row));
            row_w = 0;
        } else if row_w > 0 {
            push_piece(&mut row, " ", word.joiner);
            row_w += 1;
        }
        for (t, s) in &word.pieces {
            push_piece(&mut row, t, *s);
        }
        row_w += ww;
    }
    if !row.is_empty() || rows.is_empty() {
        rows.push(row);
    }
    to_spans(rows)
}

/// One word of a line being wrapped ([`tokenize_words`]).
pub(super) struct Word {
    /// The word's styled pieces — a run of non-whitespace that may cross
    /// piece and style boundaries (`un**bold**`).
    pub(super) pieces: Vec<(String, Style)>,
    /// The style of the single space [`wrap_inline`] puts between this word
    /// and the one before it when the two share a row: the default, except
    /// between two words of **one link** ([`Gap`]), where it is the style
    /// the link wrote its own space in — so a label of several words is
    /// underlined across its spaces and painted as one hyperlink, rather
    /// than a link per word with holes between them (`docs/links.md`).
    pub(super) joiner: Style,
}

/// The whitespace between two words, as far as a link is concerned.
#[derive(Clone, Copy)]
enum Gap {
    /// No whitespace seen since the last word.
    Closed,
    /// All of it so far was written inside one link, in this style.
    Linked(Style),
    /// Some of it was written outside a link, or it runs from one link into
    /// another: a plain space.
    Plain,
}

impl Gap {
    /// The gap after more whitespace written in `style`.
    fn widened(self, style: Style) -> Self {
        let link = style
            .underline_color
            .filter(|&carrier| links::carrier_id(carrier).is_some());
        match (self, link) {
            (Self::Closed, Some(_)) => Self::Linked(style),
            (Self::Linked(first), Some(_)) if first.underline_color == link => Self::Linked(style),
            _ => Self::Plain,
        }
    }

    /// The style of the space this gap becomes between a word ending in a
    /// piece styled `before` and one opening in `after`: the link's own,
    /// when the gap and both of its neighbours belong to the same link.
    fn joiner(self, before: Option<Style>, after: Style) -> Style {
        match self {
            Self::Linked(style)
                if before.is_some_and(|before| before.underline_color == style.underline_color)
                    && after.underline_color == style.underline_color =>
            {
                style
            }
            _ => Style::default(),
        }
    }
}

/// Split styled `segments` into [`Word`]s — each a run of non-whitespace
/// pieces that may cross piece/style boundaries — dropping the whitespace
/// between them, which [`wrap_inline`] puts back as one space in the word's
/// [`joiner`](Word::joiner) style.
///
/// Every wrapped row of every reply, table cell and tool header starts here,
/// so it takes each run of a segment as one slice — a word's text is copied
/// once, not pushed a character at a time — and follows a link's spaces only
/// on a line that has a link in it at all.
pub(super) fn tokenize_words(segments: &[(String, Style)]) -> Vec<Word> {
    let linked = segments
        .iter()
        .any(|(_, style)| style.underline_color.is_some());
    let mut words: Vec<Word> = Vec::new();
    let mut word: Vec<(String, Style)> = Vec::new();
    let mut joiner = Style::default();
    let mut gap = Gap::Closed;
    for (text, style) in segments {
        let mut rest = text.as_str();
        loop {
            let run = rest.trim_start();
            if run.len() != rest.len() {
                // Whitespace ends the word in progress, wherever it began.
                if !word.is_empty() {
                    words.push(Word {
                        pieces: std::mem::take(&mut word),
                        joiner,
                    });
                }
                if linked {
                    gap = gap.widened(*style);
                }
            }
            if run.is_empty() {
                break;
            }
            let end = run.find(char::is_whitespace).unwrap_or(run.len());
            if word.is_empty() && linked {
                // A word opens: settle the space in front of it.
                let before = words
                    .last()
                    .and_then(|word| word.pieces.last())
                    .map(|(_, style)| *style);
                joiner = gap.joiner(before, *style);
                gap = Gap::Closed;
            }
            match word.last_mut() {
                Some((piece, piece_style)) if piece_style == style => piece.push_str(&run[..end]),
                _ => word.push((run[..end].to_string(), *style)),
            }
            rest = &run[end..];
        }
    }
    if !word.is_empty() {
        words.push(Word {
            pieces: word,
            joiner,
        });
    }
    words
}

/// Append `text` to a row of styled pieces, coalescing with the last piece when
/// the style matches so a run of same-styled graphemes stays one span.
fn push_piece(row: &mut Vec<(String, Style)>, text: &str, style: Style) {
    if let Some((last_text, last_style)) = row.last_mut()
        && *last_style == style
    {
        last_text.push_str(text);
        return;
    }
    row.push((text.to_string(), style));
}
