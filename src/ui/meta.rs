//! The dim meta lines' highlights (`docs/meta-highlights.md`).
//!
//! A turn summary, a `Thought for …` line, the status line's metrics, a
//! fold's `… +N lines (ctrl+o to expand)`, a running command's clock, a
//! collapsed group's line and the footer are dim — facts about the
//! conversation, not the conversation — and inside them two kinds of thing
//! are lifted: the **values** a reader scans for, in the theme's second ink
//! ([`meta_value_color`]), and the **keys** a hint names, in a softened
//! accent ([`meta_key_color`]).
//!
//! Both are marked where the row is built ([`MetaSpans`]) — a row that
//! carries a path, a command or a model's words around its numbers would
//! light up digits that are nobody's value if they were found by scanning
//! its text afterwards. A hint constant's keys are read by the hints' own
//! grammar ([`hint_spans`]), so every hint the rows print is lifted the same
//! way.

use super::theme::{meta_key_color, meta_value_color};
use super::*;

/// The separator between a hint's clauses — `(↓ to manage · ctrl+o to
/// expand)`.
const CLAUSE_SEPARATOR: &str = " · ";

/// What sets a clause's key off its action — `ctrl+o to expand`.
const KEY_SEPARATOR: &str = " to ";

/// A dim meta line built piece by piece: its words in the `base` style, its
/// values lifted ([`value`](Self::value)), its hints' keys lifted
/// ([`hint`](Self::hint)). An empty piece adds no span.
#[derive(Debug)]
pub(super) struct MetaSpans {
    base: Style,
    spans: Vec<Span<'static>>,
}

impl MetaSpans {
    pub(super) fn new(base: Style) -> Self {
        Self {
            base,
            spans: Vec::new(),
        }
    }

    /// Words, in the base style.
    pub(super) fn text(&mut self, text: impl Into<String>) -> &mut Self {
        let style = self.base;
        self.push(text.into(), style)
    }

    /// A value — a duration, a count, a size — over the base style, its
    /// modifiers kept (a collapsed group's counts stay bold).
    pub(super) fn value(&mut self, text: impl Into<String>) -> &mut Self {
        let style = value_style(self.base);
        self.push(text.into(), style)
    }

    /// A hint, its keys lifted by [`hint_spans`].
    pub(super) fn hint(&mut self, hint: &str) -> &mut Self {
        self.spans.extend(hint_spans(hint, self.base));
        self
    }

    pub(super) fn into_spans(self) -> Vec<Span<'static>> {
        self.spans
    }

    /// The pieces as `(text, style)` segments — what the span-aware wrapper
    /// takes ([`super::inline::wrap_inline`]).
    pub(super) fn into_segments(self) -> Vec<(String, Style)> {
        self.spans
            .into_iter()
            .map(|span| (span.content.into_owned(), span.style))
            .collect()
    }

    fn push(&mut self, text: String, style: Style) -> &mut Self {
        if !text.is_empty() {
            self.spans.push(Span::styled(text, style));
        }
        self
    }
}

/// `base` worn by a value: the theme's second ink, `base`'s modifiers kept.
pub(super) fn value_style(base: Style) -> Style {
    base.fg(meta_value_color())
}

/// `hint` with the keys it names lifted to [`meta_key_color`] over `base`,
/// the rest in `base`.
///
/// A hint is `{key} to {action}` clauses joined by ` · ` — inside its
/// parentheses when it has them, so the words before them are never a clause
/// (`Running in the background (↓ to manage)` names `↓` alone). A clause's
/// key is its text before the first ` to `, and only when that text is one
/// word: `plain words to nowhere` names no key. Text that names none comes
/// back whole, as one span in `base`.
pub(super) fn hint_spans(hint: &str, base: Style) -> Vec<Span<'static>> {
    let (body_start, body_end) = match (hint.find('('), hint.rfind(')')) {
        (Some(open), Some(close)) if open < close => (open + 1, close),
        _ => (0, hint.len()),
    };
    let key = base.fg(meta_key_color());
    let mut spans = Vec::new();
    // The start of the run of base text not yet pushed.
    let mut plain_from = 0;
    let mut clause_start = body_start;
    for clause in hint[body_start..body_end].split(CLAUSE_SEPARATOR) {
        if let Some((name, _)) = clause.split_once(KEY_SEPARATOR)
            && !name.is_empty()
            && !name.contains(char::is_whitespace)
        {
            if plain_from < clause_start {
                spans.push(Span::styled(
                    hint[plain_from..clause_start].to_string(),
                    base,
                ));
            }
            spans.push(Span::styled(name.to_string(), key));
            plain_from = clause_start + name.len();
        }
        clause_start += clause.len() + CLAUSE_SEPARATOR.len();
    }
    if plain_from < hint.len() {
        spans.push(Span::styled(hint[plain_from..].to_string(), base));
    }
    spans
}
