//! Compact storage for already-rendered transcript rows. Text lives in one
//! slab and repeated styles are interned; paging expands only the visible
//! rows, borrowing their text rather than allocating a string per token.

use std::collections::HashMap;
use std::ops::Range;

use ratatui::layout::Alignment;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

#[derive(Default)]
pub(super) struct CachedLines {
    text: String,
    /// Each span's text length then style id, encoded as full-width usize
    /// varints. Most highlighted tokens need only two bytes together,
    /// rather than a String and a Style (40 bytes before their text).
    spans: Vec<u8>,
    rows: Vec<CachedLine>,
    styles: Vec<Style>,
    style_ids: HashMap<Style, usize>,
}

struct CachedLine {
    /// Exclusive span-stream and text offsets. Their starts are the
    /// previous row's ends, including for rows with no spans.
    end: usize,
    text_end: usize,
    style: usize,
    alignment: Option<Alignment>,
}

impl CachedLines {
    pub(super) fn len(&self) -> usize {
        self.rows.len()
    }

    pub(super) fn clear(&mut self) {
        *self = Self::default();
    }

    fn intern_style(&mut self, style: Style) -> usize {
        if let Some(&id) = self.style_ids.get(&style) {
            return id;
        }
        let id = self.styles.len();
        self.styles.push(style);
        self.style_ids.insert(style, id);
        id
    }

    pub(super) fn extend(&mut self, lines: impl IntoIterator<Item = Line<'static>>) {
        for line in lines {
            for span in line.spans {
                self.text.push_str(&span.content);
                let style = self.intern_style(span.style);
                write_index(&mut self.spans, span.content.len());
                write_index(&mut self.spans, style);
            }
            let style = self.intern_style(line.style);
            self.rows.push(CachedLine {
                end: self.spans.len(),
                text_end: self.text.len(),
                style,
                alignment: line.alignment,
            });
        }
    }

    pub(super) fn truncate(&mut self, len: usize) {
        if len >= self.rows.len() {
            return;
        }
        let span_end = len.checked_sub(1).map_or(0, |i| self.rows[i].end);
        let text_end = len.checked_sub(1).map_or(0, |i| self.rows[i].text_end);
        self.text.truncate(text_end);
        self.spans.truncate(span_end);
        self.rows.truncate(len);
    }

    /// Expand just this window, borrowing every span's text from the slab.
    /// Styles include invisible hyperlink/image carriers verbatim.
    pub(super) fn range(&self, range: Range<usize>) -> Vec<Line<'_>> {
        let mut span_start = range.start.checked_sub(1).map_or(0, |i| self.rows[i].end);
        let mut text_start = range
            .start
            .checked_sub(1)
            .map_or(0, |i| self.rows[i].text_end);
        self.rows[range]
            .iter()
            .map(|row| {
                let mut spans = Vec::new();
                while span_start < row.end {
                    let len = read_index(&self.spans, &mut span_start);
                    let style = read_index(&self.spans, &mut span_start);
                    let end = text_start + len;
                    spans.push(Span::styled(
                        &self.text[text_start..end],
                        self.styles[style],
                    ));
                    text_start = end;
                }
                Line {
                    spans,
                    style: self.styles[row.style],
                    alignment: row.alignment,
                }
            })
            .collect()
    }

    /// Compatibility for callers that need independently owned rows. The
    /// interactive pager uses `range` and keeps its allocation to one window.
    pub(super) fn owned_range(&self, range: Range<usize>) -> Vec<Line<'static>> {
        self.range(range)
            .into_iter()
            .map(|line| Line {
                spans: line
                    .spans
                    .into_iter()
                    .map(|span| Span::styled(span.content.into_owned(), span.style))
                    .collect(),
                style: line.style,
                alignment: line.alignment,
            })
            .collect()
    }

    /// Backtrack selection changes only row styles, exactly as it does for
    /// expanded rows. Shared span styles remain untouched.
    pub(super) fn set_reversed(&mut self, range: Range<usize>, reversed: bool) {
        for i in range {
            let mut style = self.styles[self.rows[i].style];
            style.add_modifier.set(Modifier::REVERSED, reversed);
            self.rows[i].style = self.intern_style(style);
        }
    }

    #[cfg(test)]
    pub(super) fn retained_bytes(&self) -> usize {
        use std::mem::size_of;
        self.text.capacity()
            + self.spans.capacity()
            + self.rows.capacity() * size_of::<CachedLine>()
            + self.styles.capacity() * size_of::<Style>()
            // Include a control byte per bucket and conservatively count
            // the map at twice its usable capacity for unoccupied buckets.
            + self.style_ids.capacity() * (size_of::<(Style, usize)>() + 1) * 2
    }
}

/// Unsigned base-128 encoding of an internally generated usize. No offset is
/// narrowed: even usize::MAX needs at most ten bytes on a 64-bit target.
fn write_index(bytes: &mut Vec<u8>, mut value: usize) {
    while value > 0x7f {
        bytes.push((value as u8 & 0x7f) | 0x80);
        value >>= 7;
    }
    bytes.push(value as u8);
}

/// Decode only the span stream we wrote; it never contains external data.
fn read_index(bytes: &[u8], cursor: &mut usize) -> usize {
    let mut value = 0;
    let mut shift = 0;
    loop {
        let byte = bytes[*cursor];
        *cursor += 1;
        value |= usize::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return value;
        }
        shift += 7;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::layout::Alignment;
    use ratatui::style::{Color, Modifier, Style};
    use ratatui::text::{Line, Span};
    use std::borrow::Cow;
    use std::mem::size_of;

    fn fixture() -> Vec<Line<'static>> {
        let style = Style::new()
            .fg(Color::Rgb(1, 2, 3))
            .bg(Color::Indexed(17))
            .underline_color(Color::Rgb(0x80, 1, 2))
            .add_modifier(Modifier::BOLD | Modifier::ITALIC)
            .remove_modifier(Modifier::DIM);
        vec![
            Line::default(),
            Line::from(vec![Span::raw(""), Span::styled("你好 👩‍💻", style)])
                .style(Style::new().fg(Color::Yellow))
                .alignment(Alignment::Right),
            Line::from(vec![Span::styled("e\u{301}", style), Span::raw("")])
                .alignment(Alignment::Center),
        ]
    }

    #[test]
    fn cached_rows_round_trip_every_span_and_line_property() {
        let expected = fixture();
        let mut cache = CachedLines::default();
        cache.extend(expected.clone());
        assert_eq!(cache.len(), expected.len());
        assert_eq!(cache.range(0..cache.len()), expected);
        assert_eq!(cache.owned_range(1..3), expected[1..3]);
        assert!(cache.range(0..0).is_empty());
        assert!(cache.range(1..3).iter().all(|line| {
            line.spans
                .iter()
                .all(|span| matches!(span.content, Cow::Borrowed(_)))
        }));
    }

    #[test]
    fn truncating_then_appending_keeps_text_and_span_boundaries() {
        let expected = fixture();
        let mut cache = CachedLines::default();
        cache.extend(expected.clone());
        cache.truncate(2);
        cache.extend([Line::from("replacement")]);
        assert_eq!(cache.range(0..2), expected[..2]);
        assert_eq!(cache.range(2..3), [Line::from("replacement")]);
        cache.truncate(99);
        assert_eq!(cache.len(), 3);
        cache.clear();
        assert_eq!(cache.len(), 0);
        cache.extend([Line::from("fresh")]);
        assert_eq!(cache.range(0..1), [Line::from("fresh")]);
    }

    #[test]
    fn long_unicode_spans_and_many_carrier_styles_round_trip() {
        let rows: Vec<Line<'static>> = (0..300u16)
            .map(|id| {
                Line::from(vec![Span::styled(
                    "界👩‍💻".repeat(100),
                    Style::new().underline_color(Color::Rgb(0x80, (id >> 8) as u8, id as u8)),
                )])
            })
            .collect();
        let mut cache = CachedLines::default();
        cache.extend(rows.clone());
        assert_eq!(cache.range(120..140), rows[120..140]);
        assert_eq!(cache.owned_range(280..300), rows[280..300]);
    }

    #[test]
    fn backtrack_restyles_rows_without_touching_span_styles() {
        let expected = fixture();
        let mut cache = CachedLines::default();
        cache.extend(expected.clone());
        cache.set_reversed(1..3, true);
        let reversed = cache.range(0..3);
        assert_eq!(reversed[0], expected[0]);
        for i in 1..3 {
            assert_eq!(reversed[i].spans, expected[i].spans);
            assert_eq!(reversed[i].alignment, expected[i].alignment);
            assert_eq!(
                reversed[i].style,
                expected[i].style.add_modifier(Modifier::REVERSED)
            );
        }
        cache.set_reversed(1..3, false);
        assert_eq!(cache.range(0..3), expected);
    }

    #[test]
    fn repeated_code_styles_cost_less_than_a_third_of_expanded_rows() {
        let rows: Vec<Line<'static>> = (0..2_000)
            .map(|_| {
                Line::from(
                    vec![
                        Span::raw("      "),
                        Span::styled("123 ", Style::new().fg(Color::DarkGray)),
                        Span::styled("const", Style::new().fg(Color::Magenta)),
                        Span::raw(" item "),
                        Span::styled("=", Style::new().fg(Color::Cyan)),
                        Span::raw(" "),
                        Span::styled("\"task\"", Style::new().fg(Color::Green)),
                        Span::raw(";"),
                    ]
                    .into_iter()
                    .map(|span| Span::styled(span.content.into_owned(), span.style))
                    .collect::<Vec<_>>(),
                )
            })
            .collect();
        let expanded = rows.capacity() * size_of::<Line<'static>>()
            + rows
                .iter()
                .map(|line| {
                    line.spans.capacity() * size_of::<Span<'static>>()
                        + line
                            .spans
                            .iter()
                            .map(|span| match &span.content {
                                Cow::Owned(text) => text.capacity(),
                                Cow::Borrowed(_) => 0,
                            })
                            .sum::<usize>()
                })
                .sum::<usize>();
        let mut cache = CachedLines::default();
        cache.extend(rows);
        assert!(
            cache.retained_bytes() * 3 < expanded,
            "compact {} bytes vs expanded {expanded}",
            cache.retained_bytes()
        );
        let retained = cache.retained_bytes();
        // Paging borrows the text of only the requested rows; it does not
        // expand or otherwise grow the stored transcript.
        assert_eq!(cache.range(900..940).len(), 40);
        assert_eq!(cache.retained_bytes(), retained);
    }

    #[test]
    fn compact_indices_keep_the_full_usize_range() {
        let values = [
            0,
            1,
            127,
            128,
            16_383,
            16_384,
            u32::MAX as usize,
            usize::MAX,
        ];
        let mut bytes = Vec::new();
        for value in values {
            write_index(&mut bytes, value);
        }
        let mut cursor = 0;
        for expected in values {
            assert_eq!(read_index(&bytes, &mut cursor), expected);
        }
        assert_eq!(cursor, bytes.len());
    }
}
