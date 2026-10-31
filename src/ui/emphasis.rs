//! The facts inside dim meta text, picked out in the theme's
//! [`emphasis_color`] while the words around them stay dim
//! (`docs/emphasis.md`).
//!
//! The meta lines are dim on purpose: a turn's receipt, a thought's footnote,
//! a capped peek's `… +42 lines (ctrl+o to expand)` are about the work rather
//! than the work itself, so they sit back. But each carries a fact or two the
//! eye goes looking for, how long, how many, which key, and in one flat grey
//! those were as hard to find as the filler around them. So the line keeps its
//! dim and only the facts take the theme's soft accent:
//!
//! ```text
//! Thought for 3s · 120 tokens (ctrl+o to expand)
//!             ^^   ^^^         ^^^^^^
//! ```
//!
//! A fact is either a **key** a hint names (`ctrl+o`, `esc`, `shift+tab`, or
//! an arrow the hint says what it does: `↓ to manage`) or a **number** the
//! line reports (`12s`, `6.1k`, `+42`, `4.5%`). [`fact_ranges`] finds them
//! word by word, so it is only ever run over meta the app wrote itself: a
//! command, a path or a model's output may hold anything, and never goes
//! through here.

use super::file_cell::gutter_row_spans;
use super::theme::*;
use super::*;

/// `base` with its foreground swapped for the theme's emphasis, every
/// modifier it carries kept: the settled group line's bold counts stay bold.
pub(super) fn emphasized(base: Style) -> Style {
    base.fg(emphasis_color())
}

/// `text`, a line of meta the app wrote, as spans: each fact
/// ([`fact_ranges`]) in [`emphasized`] `base`, everything between them in
/// `base`. The text itself is untouched, and a line with no fact is the one
/// `base` span it always was.
pub(super) fn meta_spans(text: &str, base: Style) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut at = 0;
    for range in fact_ranges(text) {
        if range.start > at {
            spans.push(Span::styled(text[at..range.start].to_string(), base));
        }
        spans.push(Span::styled(
            text[range.clone()].to_string(),
            emphasized(base),
        ));
        at = range.end;
    }
    if at < text.len() || spans.is_empty() {
        spans.push(Span::styled(text[at..].to_string(), base));
    }
    spans
}

/// A dim `⎿` row of meta: the [`result_row`](super::tool::result_row)
/// geometry (row 0 opens with the corner, the rest indent under it) with the
/// row's facts emphasized ([`meta_spans`]). For the rows the app writes about
/// a call (a clock, a hint, the background row), never for its output.
pub(super) fn meta_row(index: usize, text: String) -> Line<'static> {
    gutter_row_spans(index, meta_spans(&text, Style::new().fg(tool_dim_color())))
}

/// The byte ranges of the facts in `text`, in order. The text is read as
/// whitespace-separated words, each stripped of the brackets and punctuation
/// that hug it (`(ctrl+o`, `50s)`, `2.8k,`), and a word is a fact when it is:
///
/// - a **key**: `esc`, `tab`, `enter`, or a `ctrl+`/`shift+`/`alt+` chord;
/// - an **arrow** followed by `to`: `↓ to manage` names the key, while the
///   status line's `↓ 1.2k tokens` is the tally's direction and no key;
/// - a **number**: digits, a decimal part, an optional `+` or `≈` in front
///   and a unit after (`12s`, `1.2k`, `+42`, `≈24.3k`, `4.5%`). A digit
///   inside a word (`context7`, `gpt-5.5`, `v0.17.0`, `#1`) is no number.
pub(super) fn fact_ranges(text: &str) -> Vec<Range<usize>> {
    let words: Vec<Range<usize>> = words(text)
        .into_iter()
        .map(|word| core(text, word))
        .collect();
    let mut facts = Vec::new();
    for (i, word) in words.iter().enumerate() {
        let core = &text[word.clone()];
        let arrow_key = is_arrow(core)
            && words
                .get(i + 1)
                .is_some_and(|next| &text[next.clone()] == "to");
        if is_key(core) || arrow_key || is_number(core) {
            facts.push(word.clone());
        }
    }
    facts
}

/// The byte ranges of `text`'s whitespace-separated words.
fn words(text: &str) -> Vec<Range<usize>> {
    let mut start = None;
    let mut out = Vec::new();
    for (at, ch) in text.char_indices() {
        match (ch.is_whitespace(), start) {
            (true, Some(from)) => {
                out.push(from..at);
                start = None;
            }
            (false, None) => start = Some(at),
            _ => {}
        }
    }
    if let Some(from) = start {
        out.push(from..text.len());
    }
    out
}

/// `word` without the brackets that open it or the brackets and punctuation
/// that close it — `(ctrl+o` is `ctrl+o`, `50s)` is `50s`, `2.8k,` is `2.8k`.
fn core(text: &str, word: Range<usize>) -> Range<usize> {
    let slice = &text[word.clone()];
    let lead = slice.len() - slice.trim_start_matches(['(', '[']).len();
    let trimmed = slice[lead..].trim_end_matches([')', ']', ',', ';', ':']);
    word.start + lead..word.start + lead + trimmed.len()
}

/// A key as the hints spell it: lowercase, chords joined with `+`.
fn is_key(word: &str) -> bool {
    matches!(word, "esc" | "tab" | "enter")
        || ["ctrl+", "shift+", "alt+"]
            .iter()
            .any(|chord| word.strip_prefix(chord).is_some_and(|key| !key.is_empty()))
}

fn is_arrow(word: &str) -> bool {
    matches!(word, "↑" | "↓" | "←" | "→")
}

/// Digits with an optional decimal part, an optional `+` or `≈` in front and
/// an optional unit after: `12s`, `1.2k`, `10,000`, `+42`, `≈24.3k`, `4.5%`.
fn is_number(word: &str) -> bool {
    let body = word.strip_prefix(['+', '≈']).unwrap_or(word);
    let unit_at = body
        .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == ','))
        .unwrap_or(body.len());
    let (number, unit) = body.split_at(unit_at);
    number
        .split(['.', ','])
        .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
        && unit.chars().all(|c| c.is_ascii_alphabetic() || c == '%')
}
