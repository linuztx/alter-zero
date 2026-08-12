//! Inline-span rendering: one markdown line's `**bold**`, `` `code` ``, links
//! and emphasis turned into styled [`Span`]s, and the word-wrap that keeps those
//! spans intact across a row boundary. See `docs/markdown.md`.

use super::theme::*;
use super::wrap::cols;
use super::*;

/// Flatten a parsed inline tree ([`markdown::parse_inline`]) into styled text
/// segments under `base`. Emphasis adds a modifier, `code` a colour, a link its
/// text plus a ` (url)` suffix, an image its alt text. Nesting composes styles.
pub(super) fn inline_spans(nodes: &[markdown::Inline], base: Style) -> Vec<(String, Style)> {
    let mut out = Vec::new();
    for node in nodes {
        match node {
            markdown::Inline::Text(t) => out.push((t.clone(), base)),
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
            markdown::Inline::Code(c) => out.push((c.clone(), base.fg(INLINE_CODE_COLOR))),
            markdown::Inline::Link { text, url } => {
                out.extend(inline_spans(text, base));
                out.push((
                    format!(" ({url})"),
                    base.fg(LINK_URL_COLOR).add_modifier(Modifier::UNDERLINED),
                ));
            }
            markdown::Inline::Image { alt } => out.push((alt.clone(), base)),
        }
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
