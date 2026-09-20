//! The thinking stream's cells: the live block the strip previews, the
//! collapsed `Thought for …` line it commits, and the Ctrl+O expansion.
//! See `docs/thinking-stream.md`.
//!
//! While a phase runs it wears the **tool cell's shape** — a `● Thinking…`
//! header over the chain-of-thought in the `⎿` gutter — because that is what
//! it is: something working, with output under it. When it settles the shape
//! goes away entirely: the committed line is bullet-less
//! (`Thought for 3s · 228 tokens (ctrl+o to expand)`), the
//! [`summary_lines`](super::summary_lines) shape, because a finished thought
//! is turn meta rather than a cell — dim on both surfaces, `Done for Ns`'s
//! exact dress. The weight and the motion live in the block above, which is
//! where something is still happening.
//!
//! Nothing here ever renders the chain-of-thought into a *committed* row —
//! [`reasoning_lines`] (the one renderer scrollback and the resize repaint
//! use) is a single line. The text lives in the live block while the phase
//! runs and in [`reasoning_full_lines`] afterwards, which is exactly the
//! collapsed-inline / expanded-in-Ctrl+O contract a tool call has.

use super::file_cell::gutter_row_styled;
use super::status::shimmer_spans_from;
use super::theme::*;
use super::tool::bullet_span;
use super::wrap::{cols, wrap_output};
use super::*;

use crate::app::Reasoning;

/// One row of the thought's body in the `⎿` gutter — dim and italic, the cue
/// that separates it from a tool's output in the same gutter. Row 0 opens with
/// the corner; the rest indent under it ([`gutter_row_styled`]).
fn body_row(index: usize, text: String) -> Line<'static> {
    gutter_row_styled(
        index,
        text,
        Style::new()
            .fg(reasoning_text_color())
            .add_modifier(REASONING_TEXT_MODIFIER),
    )
}

/// Columns the body wraps into — the width left of the `⎿` gutter, like a
/// tool's output block.
fn body_width(width: u16) -> u16 {
    width
        .saturating_sub(u16::try_from(cols(TOOL_RESULT_PREFIX)).unwrap_or(u16::MAX))
        .max(1)
}

/// The `● Thinking…` header of an open phase: the same [`TOOL_BULLET`] a
/// running tool wears (because it means the same thing), in the running grey,
/// over the label built by `label`.
///
/// The two callers differ only in how alive the row is allowed to look. In the
/// strip the bullet blinks (`blink` is the frame clock, [`bullet_span`]) and
/// the label **shimmers** ([`shimmer_spans_from`]); in the Ctrl+O pager both
/// render flat (`blink: None`), because that view's cache signature is
/// deliberately clock-free (`docs/tool-pulse.md`).
fn thinking_header(blink: Option<Duration>, label: Vec<Span<'static>>) -> Line<'static> {
    let mut spans = vec![bullet_span(tool_running_color(), blink)];
    spans.extend(label);
    Line::from(spans)
}

/// The settled phase's label — `Thought for {elapsed}` — and, separately, its
/// `· {n} tokens` metrics. Split because the two surfaces compose them
/// differently: inline the `EXPAND_HINT` follows, in Ctrl+O nothing does.
///
/// [`format_elapsed`] humanizes the seconds and [`format_token_count`] the
/// tokens, so this reads like every other duration and count in the TUI. A
/// zero token count (an old rollout, a backend that reported none and
/// estimated nothing) yields empty metrics rather than claiming `· 0 tokens`.
fn thought_label(reasoning: &Reasoning) -> String {
    format!("{REASONING_DONE}{}", format_elapsed(reasoning.secs))
}

/// The settled line's span: dim, the same on both surfaces. A finished
/// thought is a footnote about work already done — the weight belongs to the
/// live block, which is where something is still happening.
fn settled_span(text: String) -> Span<'static> {
    Span::styled(text, Style::new().fg(reasoning_label_color()))
}

/// See [`thought_label`] — `" · 1.5k tokens"`, or empty when unknown.
fn thought_metrics(reasoning: &Reasoning) -> String {
    if reasoning.tokens == 0 {
        return String::new();
    }
    format!(" · {} tokens", format_token_count(reasoning.tokens))
}

/// The settled thinking phase as **committed** lines: one dim, **bullet-less**
/// `Thought for {elapsed} · {n} tokens (ctrl+o to expand)` — the
/// [`summary_lines`] shape (`width` is unused, kept for the uniform `*_lines`
/// signature).
///
/// No bullet, and dim throughout — `Done for Ns`'s exact dress. The
/// `● Thinking…` header meant *something is happening*, and nothing is any
/// more; what is left is a fact about the turn, so it settles into the
/// transcript rather than competing with the reply it sits above. The
/// `EXPAND_HINT` says where the thought itself went, the same promise a capped
/// tool peek makes.
#[must_use]
pub fn reasoning_lines(reasoning: &Reasoning, _width: u16) -> Vec<Line<'static>> {
    vec![Line::from(settled_span(format!(
        "{}{}{EXPAND_HINT}",
        thought_label(reasoning),
        thought_metrics(reasoning)
    )))]
}

/// The Ctrl+O transcript's expanded cell: the same dim settled line — minus
/// the `(ctrl+o to expand)` hint, since this *is* the expansion — over the
/// whole chain-of-thought in the `⎿` gutter.
pub(super) fn reasoning_full_lines(reasoning: &Reasoning, width: u16) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(settled_span(format!(
        "{}{}",
        thought_label(reasoning),
        thought_metrics(reasoning)
    )))];
    lines.extend(gutter_body(&reasoning.text, width));
    lines
}

/// The Ctrl+O transcript's view of an **open** phase: the `● Thinking…` header
/// over the whole thought so far. The pager has no row budget — it
/// tail-follows the frontier the way it does a streaming `bash` call's output
/// (`docs/tool-view-performance.md`) — so unlike the strip's block this is not
/// windowed.
///
/// Rendered **at rest**, bullet and label both: the transcript's cache
/// signature is deliberately clock-free, so an animation here would either not
/// move or cost a full-tail re-render every 32 ms for something nobody is
/// watching (`docs/tool-pulse.md`).
pub(super) fn reasoning_live_full_lines(text: &str, width: u16) -> Vec<Line<'static>> {
    let label = vec![Span::styled(
        REASONING_RUNNING.to_string(),
        Style::new().fg(reasoning_text_color()),
    )];
    let mut lines = vec![thinking_header(None, label)];
    lines.extend(gutter_body(text, width));
    lines
}

/// The **whole** thought in the `⎿` gutter, its own blank lines kept as
/// paragraph breaks — the un-windowed body both pager views render.
/// [`wrap_output`] preserves spaces and breaks at words, like a tool's
/// expanded output.
fn gutter_body(text: &str, width: u16) -> Vec<Line<'static>> {
    let inner = body_width(width);
    let mut rows: Vec<Line<'static>> = Vec::new();
    for source in text.split('\n') {
        if source.trim().is_empty() {
            rows.push(Line::default());
            continue;
        }
        for row in wrap_output(source, inner) {
            rows.push(body_row(rows.len(), row));
        }
    }
    rows
}

/// The **live** block for an open thinking phase, drawn in the strip's preview
/// slot: a `● Thinking…` header — the bullet blinking at the frame `pulse`
/// and the label carrying the status line's **shimmer** sweep
/// ([`shimmer_spans_from`], the same wave the `Working…` verb below it wears,
/// but floored at the near-white [`reasoning_shimmer_base`] so it reads as
/// bold white between crests rather than codex's grey) — over the **tail** of
/// the thought so far in the `⎿` gutter: the last [`REASONING_PEEK_LINES`]
/// wrapped rows, dim and italic.
///
/// Two animations off one clock, and neither can ever be committed: the whole
/// block is live-only by construction (only the strip calls this), which is
/// why the pulse and the shimmer are unconditional here and absent from every
/// other renderer in this module.
///
/// Blank source lines are skipped: reasoning is full of paragraph breaks, and
/// spending the small window on them would show a third as much thought.
/// Walking newest-first wraps only what the window can show, so redrawing this
/// every animation frame costs O(window), not O(reasoning).
pub(super) fn live_reasoning_lines(text: &str, pulse: Duration, width: u16) -> Vec<Line<'static>> {
    let mut lines = vec![thinking_header(
        Some(pulse),
        shimmer_spans_from(REASONING_RUNNING, pulse, reasoning_shimmer_base()),
    )];
    let inner = body_width(width);
    let mut window: VecDeque<String> = VecDeque::new();
    for source in text.split('\n').rev() {
        if source.trim().is_empty() {
            continue;
        }
        for row in wrap_output(source, inner).into_iter().rev() {
            window.push_front(row);
        }
        if window.len() >= REASONING_PEEK_LINES {
            break;
        }
    }
    while window.len() > REASONING_PEEK_LINES {
        window.pop_front();
    }
    lines.extend(
        window
            .into_iter()
            .enumerate()
            .map(|(i, row)| body_row(i, row)),
    );
    lines
}
