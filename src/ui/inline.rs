//! Inline-span rendering: one markdown line's `**bold**`, `` `code` ``, links
//! and emphasis turned into styled [`Span`]s, and the word-wrap that keeps those
//! spans intact across a row boundary. See `docs/markdown.md`.

use crate::links;

use super::theme::*;
use super::wrap::cols;
use super::*;

/// Flatten a parsed inline tree ([`markdown::parse_inline`]) into styled text
/// segments under `base`. Emphasis adds a modifier, `code` a colour, a link its
/// text plus a ` (url)` suffix, an image its alt text. Nesting composes styles.
/// URLs — a `[text](url)` target and every bare `http(s)://…` in plain text —
/// additionally carry the link **carrier** ([`links::linked`]), so each
/// wrapped fragment of a URL still opens the whole target at the paint
/// boundary (`docs/links.md`). Code URLs carry targets too, without changing
/// their literal text or code styling; the suffix's own `(`/`)` stay prose —
/// only the target between them is dressed and marked as a link.
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
            markdown::Inline::Link { text, url } => {
                // The text keeps its own dress and gains the target; the
                // ` (url)` suffix splits so exactly the URL carries it. The
                // parens are decoration this renderer adds, not link text —
                // they take neither the link dress nor the carrier, so what
                // the eye reads as the link is exactly what a click opens
                // (`docs/links.md` *The decoration parens*).
                out.extend(
                    inline_spans(text, base)
                        .into_iter()
                        .map(|(t, s)| (t, links::linked(s, url))),
                );
                let url_style = base.fg(link_url_color()).add_modifier(Modifier::UNDERLINED);
                out.push((" (".to_string(), base));
                out.push((url.clone(), links::linked(url_style, url)));
                out.push((")".to_string(), base));
            }
            markdown::Inline::Image { alt } => out.push((alt.clone(), base)),
        }
    }
    out
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
        let style = base.fg(link_url_color()).add_modifier(Modifier::UNDERLINED);
        out.push((url.to_string(), links::linked(style, url)));
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

/// Attach bare URL targets to **unwrapped**, already-styled text without
/// changing its bytes or dress (code and headings). Detection sees the whole
/// line, not individual syntax-highlighted segments: a highlighter can split a
/// URL at `://`, punctuation, or query parameters. Every intersecting piece
/// carries the full target before either wrapper breaks it into display rows.
/// Segments must belong to one source line; separate lines are never joined.
pub(super) fn linkify_segments(segments: Vec<(String, Style)>) -> Vec<(String, Style)> {
    linkify_with(segments, links::find_urls)
}

/// Intersect source URL ranges with styled runs, independent of the detector's
/// prose/code boundary rules.
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
/// boundaries; whitespace collapses to single base-styled spaces between words.
/// An empty line yields one empty row (matching [`wrap_text`]).
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
                push_piece(&mut row, " ", Style::default());
            }
            for (t, s) in word {
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
        let ww: usize = word.iter().map(|(t, _)| cols(t)).sum();
        if ww > width {
            // A word too wide for any line: hard-break it grapheme by grapheme.
            if row_w > 0 {
                rows.push(std::mem::take(&mut row));
                row_w = 0;
            }
            for (t, s) in word {
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
            push_piece(&mut row, " ", Style::default());
            row_w += 1;
        }
        for (t, s) in word {
            push_piece(&mut row, t, *s);
        }
        row_w += ww;
    }
    if !row.is_empty() || rows.is_empty() {
        rows.push(row);
    }
    to_spans(rows)
}

/// Split styled `segments` into words — each a run of non-whitespace pieces
/// (`Vec<(text, style)>`) that may cross piece/style boundaries — dropping the
/// whitespace between them ([`wrap_inline`] re-inserts single spaces).
pub(super) fn tokenize_words(segments: &[(String, Style)]) -> Vec<Vec<(String, Style)>> {
    let mut words: Vec<Vec<(String, Style)>> = Vec::new();
    let mut word: Vec<(String, Style)> = Vec::new();
    let mut piece = String::new();
    let mut piece_style = Style::default();
    for (text, style) in segments {
        for ch in text.chars() {
            if ch.is_whitespace() {
                if !piece.is_empty() {
                    word.push((std::mem::take(&mut piece), piece_style));
                }
                if !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                }
            } else {
                if !piece.is_empty() && *style != piece_style {
                    word.push((std::mem::take(&mut piece), piece_style));
                }
                piece_style = *style;
                piece.push(ch);
            }
        }
    }
    if !piece.is_empty() {
        word.push((piece, piece_style));
    }
    if !word.is_empty() {
        words.push(word);
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
