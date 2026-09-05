//! The live status line — comet spinner, shimmering verb, token tally — and
//! the dim `Done for Ns` summary it commits.
//! See `docs/status-indicator.md`.

use super::theme::*;
use super::wrap::{blend, breath, clamp_spans, lerp_rgb};
use super::*;

use crate::app::Spinner;

/// One bold span per char of `text`, shimmered codex-style: a raised-cosine
/// brightness band (half-width [`SHIMMER_BAND_HALF_WIDTH`], plus
/// [`SHIMMER_PADDING`] chars of off-text run-in/out) sweeps the text once per
/// [`SHIMMER_SWEEP`], each char blending from the white-grey [`SHIMMER_BASE`]
/// toward the bright [`SHIMMER_HIGHLIGHT`] by its distance from the band's
/// crest. A faithful port of openai/codex `tui/src/shimmer.rs::shimmer_spans`,
/// made pure: the phase comes from the boundary-supplied `elapsed` (sub-second
/// resolution), not a process-wide clock — so it's deterministic in tests.
///
/// **Live regions only**: every span carries a colour sampled from one frame
/// of the wave, so committing these rows to scrollback would freeze the sweep
/// mid-stride forever.
pub(super) fn shimmer_spans(text: &str, elapsed: Duration) -> Vec<Span<'static>> {
    shimmer_spans_from(text, elapsed, SHIMMER_BASE)
}

/// [`shimmer_spans`] with the wave's **resting** colour chosen by the caller —
/// what the text reads as between crests, which is most of the sweep (the band
/// is [`SHIMMER_BAND_HALF_WIDTH`] wide inside a period of the text plus
/// `2 × `[`SHIMMER_PADDING`]).
///
/// The status verb keeps codex's grey [`SHIMMER_BASE`], so it reads as *grey
/// text with a white wave*. The thinking stream's `Thinking…`
/// (`docs/thinking-stream.md`) passes the near-white
/// [`REASONING_SHIMMER_BASE`] instead, so it reads as *bold white with a
/// brighter wave* — a header, not a metric. Same motion, different floor.
pub(super) fn shimmer_spans_from(
    text: &str,
    elapsed: Duration,
    base: (u8, u8, u8),
) -> Vec<Span<'static>> {
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return Vec::new();
    }
    let period = chars.len() + SHIMMER_PADDING * 2;
    let sweep = SHIMMER_SWEEP.as_secs_f32();
    let pos = ((elapsed.as_secs_f32() % sweep) / sweep * period as f32) as usize;

    chars
        .iter()
        .enumerate()
        .map(|(i, ch)| {
            let dist = (i as isize + SHIMMER_PADDING as isize - pos as isize).abs() as f32;
            let t = if dist <= SHIMMER_BAND_HALF_WIDTH {
                let x = std::f32::consts::PI * (dist / SHIMMER_BAND_HALF_WIDTH);
                0.5 * (1.0 + x.cos())
            } else {
                0.0
            };
            let (r, g, b) = blend(SHIMMER_HIGHLIGHT, base, t * SHIMMER_MAX_BLEND);
            Span::styled(
                ch.to_string(),
                Style::new()
                    .fg(Color::Rgb(r, g, b))
                    .add_modifier(Modifier::BOLD),
            )
        })
        .collect()
}

/// A style's frames and how long each shows — the catalog's *look*, kept in
/// `theme` beside every other styling decision (`docs/spinner.md`). The
/// one-frame styles (`pulse`, `still`) never step; `pulse` moves by colour
/// alone ([`glyph_color`]).
fn spinner_frames(spinner: Spinner) -> (&'static [&'static str], Duration) {
    match spinner {
        Spinner::Comet => (SPINNER_FRAMES, SPINNER_INTERVAL),
        Spinner::Sparkle => (SPINNER_SPARKLE_FRAMES, SPINNER_SPARKLE_INTERVAL),
        Spinner::Dots => (SPINNER_DOTS_FRAMES, SPINNER_DOTS_INTERVAL),
        Spinner::Orbit => (SPINNER_ORBIT_FRAMES, SPINNER_ORBIT_INTERVAL),
        Spinner::Blocks => (SPINNER_BLOCKS_FRAMES, SPINNER_BLOCKS_INTERVAL),
        Spinner::Pulse => (SPINNER_PULSE_FRAMES, SPINNER_PULSE_PERIOD),
        Spinner::Bars => (SPINNER_BARS_FRAMES, SPINNER_BARS_INTERVAL),
        Spinner::Line => (SPINNER_LINE_FRAMES, SPINNER_LINE_INTERVAL),
        Spinner::Still => (SPINNER_STILL_FRAMES, SPINNER_INTERVAL),
    }
}

/// The colour of a one-cell style's glyph — what makes the styles more than
/// glyph sets, and where the theme's accent reaches the status line:
///
/// - `sparkle` and `blocks` walk the banner's cyan → blue gradient
///   (`docs/header.md`) — the spark by its bloom level (`·` cyan, `✽` blue,
///   back down the fade), the block by its turn;
/// - `pulse` breathes the running tool bullet's raised cosine
///   ([`breath`], `docs/tool-pulse.md`) from [`SPINNER_PULSE_DIM`] to white;
/// - `bars` brightens with height, [`SPINNER_BARS_LOW`] at `▁` to white at `█`;
/// - everything else wears the comet head's white.
///
/// `index` is the frame showing out of `len`; a rise-and-fall sequence's
/// level is its distance from the closed end, so the fade mirrors the bloom.
fn glyph_color(spinner: Spinner, index: usize, len: usize, elapsed: Duration) -> Color {
    let level = |index: usize| -> f32 {
        let peak = len / 2;
        let level = if index <= peak { index } else { len - index };
        level as f32 / peak.max(1) as f32
    };
    match spinner {
        Spinner::Sparkle => lerp_rgb(HEADER_GRADIENT_START, HEADER_GRADIENT_END, level(index)),
        Spinner::Blocks => lerp_rgb(
            HEADER_GRADIENT_START,
            HEADER_GRADIENT_END,
            index as f32 / len.saturating_sub(1).max(1) as f32,
        ),
        Spinner::Pulse => {
            let (r, g, b) = blend(
                SPINNER_PULSE_BRIGHT,
                SPINNER_PULSE_DIM,
                breath(elapsed, SPINNER_PULSE_PERIOD),
            );
            Color::Rgb(r, g, b)
        }
        Spinner::Bars => {
            let (r, g, b) = blend(SPINNER_BARS_HIGH, SPINNER_BARS_LOW, level(index));
            Color::Rgb(r, g, b)
        }
        Spinner::Comet | Spinner::Dots | Spinner::Orbit | Spinner::Line | Spinner::Still => {
            STATUS_COLOR
        }
    }
}

/// The spinner opening the status line, in the session's chosen `spinner`
/// style (`docs/spinner.md`): the style's frame for `elapsed` (one frame per
/// its interval, looping), as spans that end in the separator space before
/// the verb. The comet is its own shape ([`comet_spans`], one span per cell);
/// every other style is one glyph in one span, bold, coloured by
/// [`glyph_color`]. Pure, like [`shimmer_spans`]: the frame index derives
/// from the boundary-supplied `elapsed`, and the loop's animation re-arm
/// keeps it advancing — which is also what lets the `/spinner` picker draw
/// each row's live spinner with it.
pub(super) fn spinner_spans(spinner: Spinner, elapsed: Duration) -> Vec<Span<'static>> {
    let (frames, interval) = spinner_frames(spinner);
    let index = (elapsed.as_millis() / interval.as_millis().max(1)) as usize % frames.len().max(1);
    let frame = frames[index];
    if spinner == Spinner::Comet {
        return comet_spans(frame);
    }
    vec![Span::styled(
        format!("{frame} "),
        Style::new()
            .fg(glyph_color(spinner, index, frames.len(), elapsed))
            .add_modifier(Modifier::BOLD),
    )]
}

/// The comet's `frame` split into exactly [`SPINNER_SPAN_COUNT`] spans — one
/// per cell, so each carries its own fade step: the white bold
/// [`SPINNER_HEAD`], the mid-grey [`SPINNER_TAIL_MID`] behind it, and
/// everything else (the faint `·` tail end, the walls, the empty track) dim;
/// the right wall carries the trailing separator space.
fn comet_spans(frame: &str) -> Vec<Span<'static>> {
    let dim = Style::new().fg(STATUS_DETAIL_COLOR);
    let spans: Vec<Span<'static>> = frame
        .chars()
        .enumerate()
        .map(|(i, c)| {
            let style = match c {
                SPINNER_HEAD => Style::new().fg(STATUS_COLOR).add_modifier(Modifier::BOLD),
                SPINNER_TAIL_MID => Style::new().fg(SPINNER_TAIL_COLOR),
                _ => dim,
            };
            let text = if i == SPINNER_SPAN_COUNT - 1 {
                format!("{c} ")
            } else {
                c.to_string()
            };
            Span::styled(text, style)
        })
        .collect();
    debug_assert_eq!(spans.len(), SPINNER_SPAN_COUNT);
    spans
}

// Re-exported from the pure core so the historic `ui::format_elapsed` path
// (and every in-module unqualified use) keeps working: the app's own display
// strings (an agent notice's `finished · 6m 2s`) humanize with the same
// helper, so it lives beside the state that formats with it.
pub use crate::app::format_elapsed;

/// Humanize a token count for the status line and the turn summary: bare under
/// a thousand (`842`), one-decimal thousands up to a million (`8.1k`, a
/// trailing `.0` dropped — `15k`), one-decimal millions past that (`1.2M`).
/// Real provider usage counts the whole re-sent context per round, so an
/// agentic turn's tally runs to six digits — unreadable raw in a one-line
/// status (`docs/prompt-caching.md`).
#[must_use]
pub fn format_token_count(tokens: usize) -> String {
    /// One-decimal `value/scale` with a trailing `.0` dropped (`8.1`, `15`).
    fn scaled(tokens: usize, scale: f64, suffix: &str) -> String {
        #[allow(clippy::cast_precision_loss)] // display only — 1dp anyway
        let value = (tokens as f64 / scale * 10.0).round() / 10.0;
        if value.fract() == 0.0 {
            format!("{value:.0}{suffix}")
        } else {
            format!("{value:.1}{suffix}")
        }
    }
    if tokens < 1_000 {
        tokens.to_string()
    } else if tokens < 1_000_000 {
        scaled(tokens, 1_000.0, "k")
    } else {
        scaled(tokens, 1_000_000.0, "M")
    }
}

/// The live status line shown in the strip above the box while a turn is in
/// flight:
/// `(●•·   ) {verb}… ({elapsed}[ · {arrow} {n} tokens][ · Thinking for {m}] · esc to
/// interrupt)`.
///
/// It opens with the comet spinner (`spinner_spans`) and the verb
/// text **shimmers** — a bright-white band sweeping its white-grey chars
/// (`shimmer_spans`) — both animations phase-driven by the boundary-supplied
/// `elapsed`; the parenthesised metrics are dim. The token clause is omitted
/// while the tally is 0 (the "just submitted" state), and the thinking clause
/// only while `thinking` is `Some`. Clamped to `width` with a dim `…`
/// (`clamp_spans`): the line is one animated strip row by design
/// (`STATUS_ROWS` is fixed, and its per-frame shimmer colours must never
/// reach scrollback), so a narrow terminal degrades it honestly instead of
/// paint-clipping the retry warning, the thinking clause, and the esc hint
/// with no cue. Pure — it formats the (already boundary-stamped)
/// [`TurnStatus`], so it is unit-tested with explicit values.
///
/// This is [`styled_status_line`] in the default [`Spinner::Comet`] style;
/// the strip itself passes the session's chosen style (`docs/spinner.md`).
#[must_use]
pub fn status_line(status: &TurnStatus, width: u16) -> Line<'static> {
    status_line_with_verb(status, None, width)
}

/// [`status_line`] with the verb **overridden** — the task checklist's
/// spinner rule (`docs/task-tools.md`): while some task is in progress the
/// line wears its `activeForm` (`Setting up project structure…`) instead of
/// the turn's whimsical verb, Claude Code's
/// `currentTodo.activeForm ?? randomVerb`. `None` keeps the turn's own verb;
/// the caller derives the override per frame ([`crate::app::App::task_verb`])
/// so completing the task snaps it back mid-turn. In the default comet style,
/// like [`status_line`].
#[must_use]
pub fn status_line_with_verb(status: &TurnStatus, verb: Option<&str>, width: u16) -> Line<'static> {
    styled_status_line(status, verb, Spinner::default(), width)
}

/// [`status_line_with_verb`] opening with the `spinner` **style** the session
/// chose in `/spinner` (`docs/spinner.md`) — the one renderer behind the
/// strip's status row (main turn and agent session view alike, passing
/// [`crate::app::App::spinner`]) and behind the picker's live preview, so the
/// two can never disagree. Every style ends its spans in the separator space,
/// so the verb's shimmer starts at the same distance whatever the style's
/// width. `None` keeps the turn's own verb.
#[must_use]
pub fn styled_status_line(
    status: &TurnStatus,
    verb: Option<&str>,
    spinner: Spinner,
    width: u16,
) -> Line<'static> {
    let dim = Style::new().fg(STATUS_DETAIL_COLOR);
    let mut spans = spinner_spans(spinner, status.elapsed);
    spans.extend(shimmer_spans(
        &format!("{}{STATUS_ELLIPSIS}", verb.unwrap_or(status.verb)),
        status.elapsed,
    ));
    // The parenthesised metrics are dim, except the retry clause, which carries
    // its own warning colour — so it is built as its own span between the
    // (dim) token and hint clauses.
    spans.push(Span::styled(
        format!(" ({}", format_elapsed(status.elapsed.as_secs())),
        dim,
    ));
    if status.tokens > 0 {
        let arrow = match status.arrow {
            TokenArrow::Down => STATUS_ARROW_DOWN,
            TokenArrow::Up => STATUS_ARROW_UP,
        };
        spans.push(Span::styled(
            format!(" · {arrow} {} tokens", format_token_count(status.tokens)),
            dim,
        ));
    }
    if let Some(retry) = status.retry {
        spans.push(Span::styled(
            format!(" · retrying {}/{}", retry.attempt, retry.max),
            Style::new().fg(STATUS_RETRY_COLOR),
        ));
    }
    if let Some(thinking) = status.thinking {
        spans.push(Span::styled(
            format!(" · Thinking for {}", format_elapsed(thinking.as_secs())),
            dim,
        ));
    }
    spans.push(Span::styled(format!(" · {STATUS_INTERRUPT_HINT})"), dim));
    clamp_spans(spans, width as usize)
}

/// The committed turn summary: dim, bullet-less `"{verb} for {elapsed}"`
/// (the seconds humanized by [`format_elapsed`] — `Done for 20s`, `Done for 1m 30s`)
/// — with a `· {n} tokens ({c} cached)` receipt and a `· {n} shells still
/// running` suffix when either applies (`docs/background.md`,
/// `docs/prompt-caching.md`). Shown inline (it flows into scrollback) and in
/// the transcript like any other [`HistoryItem`]. **Word-wrapped** to
/// `width`: the full chain runs past 60 columns and the rows are permanent
/// scrollback, so a narrow terminal keeps the whole receipt instead of
/// paint-clipping its tail (the old `_width` was ignored on a "the line
/// never wraps" premise the token clause outgrew).
#[must_use]
pub fn summary_lines(summary: &TurnSummary, width: u16) -> Vec<Line<'static>> {
    let mut text = format!("{} for {}", summary.verb, format_elapsed(summary.secs));
    if summary.tokens > 0 {
        // The turn's real billed tokens, with the cache-served share beside
        // them — the visible proof prompt caching worked. Absent (the dummy,
        // a `!` shell) the summary keeps its bare shape. See
        // docs/prompt-caching.md.
        text.push_str(&format!(" · {} tokens", format_token_count(summary.tokens)));
        if summary.cached > 0 {
            text.push_str(&format!(" ({} cached)", format_token_count(summary.cached)));
        }
    }
    if summary.shells > 0 {
        let plural = if summary.shells == 1 { "" } else { "s" };
        text.push_str(&format!(
            " · {} shell{plural} still running",
            summary.shells
        ));
    }
    wrap_text(&text, width)
        .into_iter()
        .map(|row| Line::from(Span::styled(row, Style::new().fg(STATUS_DONE_COLOR))))
        .collect()
}

/// A background shell's completion notice as committed lines: the coloured
/// `●` bullet — green for a clean exit, red for a failure or a user stop —
/// over the wrapped one-line headline (`Background command "{description}"
/// completed (exit code 0)`). The output tail the notice carries is
/// context-only and never rendered. See `docs/background.md`.
#[must_use]
pub fn background_notice_lines(
    notice: &crate::app::BackgroundNotice,
    width: u16,
) -> Vec<Line<'static>> {
    let color = if notice.ok() {
        BG_NOTICE_OK_COLOR
    } else {
        BG_NOTICE_FAIL_COLOR
    };
    let bullet_style = Style::new().fg(color).add_modifier(Modifier::BOLD);
    let content_width = width.saturating_sub(BULLET_WIDTH).max(1);
    wrap_text(&notice.headline(), content_width)
        .into_iter()
        .enumerate()
        .map(|(i, line)| {
            if i == 0 {
                Line::from(vec![
                    Span::styled(AI_BULLET.to_string(), bullet_style),
                    Span::raw(line),
                ])
            } else {
                Line::from(vec![Span::raw(INDENT.to_string()), Span::raw(line)])
            }
        })
        .collect()
}
