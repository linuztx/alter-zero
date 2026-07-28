//! The live status line — comet spinner, shimmering verb, token tally — and
//! the dim `Done for Ns` summary it commits.
//! See `docs/status-indicator.md`.

use super::theme::*;
use super::wrap::blend;
use super::*;

/// One bold span per char of `text`, shimmered codex-style: a raised-cosine
/// brightness band (half-width [`SHIMMER_BAND_HALF_WIDTH`], plus
/// [`SHIMMER_PADDING`] chars of off-text run-in/out) sweeps the text once per
/// [`SHIMMER_SWEEP`], each char blending from the white-grey [`SHIMMER_BASE`]
/// toward the bright [`SHIMMER_HIGHLIGHT`] by its distance from the band's
/// crest. A faithful port of openai/codex `tui/src/shimmer.rs::shimmer_spans`,
/// made pure: the phase comes from the boundary-supplied `elapsed` (sub-second
/// resolution), not a process-wide clock — so it's deterministic in tests.
fn shimmer_spans(text: &str, elapsed: Duration) -> Vec<Span<'static>> {
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
            let (r, g, b) = blend(SHIMMER_HIGHLIGHT, SHIMMER_BASE, t * SHIMMER_MAX_BLEND);
            Span::styled(
                ch.to_string(),
                Style::new()
                    .fg(Color::Rgb(r, g, b))
                    .add_modifier(Modifier::BOLD),
            )
        })
        .collect()
}

/// The comet spinner opening the status line: the [`SPINNER_FRAMES`] frame
/// for `elapsed` (one frame per [`SPINNER_INTERVAL`], looping), split into
/// exactly [`SPINNER_SPAN_COUNT`] spans — one per cell, so each carries its
/// own fade step: the white bold [`SPINNER_HEAD`], the mid-grey
/// [`SPINNER_TAIL_MID`] behind it, and everything else (the faint `·` tail
/// end, the walls, the empty track) dim; the right wall carries the trailing
/// separator space. Pure, like [`shimmer_spans`]: the frame index derives
/// from the boundary-supplied `elapsed`, and the loop's animation re-arm
/// keeps it advancing.
fn spinner_spans(elapsed: Duration) -> Vec<Span<'static>> {
    let frame_index =
        (elapsed.as_millis() / SPINNER_INTERVAL.as_millis()) as usize % SPINNER_FRAMES.len();
    let frame = SPINNER_FRAMES[frame_index];
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

/// Humanize an elapsed count of whole seconds for the status indicator —
/// combined two-unit, so the display scales past a bare seconds counter:
/// `{s}s` under a minute (the live seconds keep ticking so a running timer
/// never looks frozen), `{m}m {s}s` under an hour, `{h}h {m}m` past an hour
/// (`h` grows unbounded). Distinct from [`crate::session::relative_age`], which
/// is a **single-unit** static age label (`2m`, `1h`). Shared by the live
/// status line, its `Thinking for …` clause, and the committed `… for …`
/// summary so all three read the same (`docs/status-indicator.md`).
#[must_use]
pub fn format_elapsed(secs: u64) -> String {
    if secs < 60 {
        return format!("{secs}s");
    }
    let minutes = secs / 60;
    if minutes < 60 {
        return format!("{minutes}m {}s", secs % 60);
    }
    format!("{}h {}m", minutes / 60, minutes % 60)
}

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
/// only while `thinking` is `Some`. Pure — it formats the (already
/// boundary-stamped) [`TurnStatus`], so it is unit-tested with explicit values.
#[must_use]
pub fn status_line(status: &TurnStatus) -> Line<'static> {
    let dim = Style::new().fg(STATUS_DETAIL_COLOR);
    let mut spans = spinner_spans(status.elapsed);
    spans.extend(shimmer_spans(
        &format!("{}{STATUS_ELLIPSIS}", status.verb),
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
    Line::from(spans)
}

/// The committed turn summary: a single dim, bullet-less `"{verb} for {elapsed}"`
/// (the seconds humanized by [`format_elapsed`] — `Done for 20s`, `Done for 1m 30s`)
/// line — with a `· {n} shells still running` suffix when background shells
/// were running at turn end (`docs/background.md`). Shown inline (it flows into
/// scrollback) and in the transcript like any other [`HistoryItem`]; `width` is
/// unused (the line never wraps) but kept for a uniform `*_lines` signature.
#[must_use]
pub fn summary_lines(summary: &TurnSummary, _width: u16) -> Vec<Line<'static>> {
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
    vec![Line::from(Span::styled(
        text,
        Style::new().fg(STATUS_DONE_COLOR),
    ))]
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
