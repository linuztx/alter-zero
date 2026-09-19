//! The live status line — the session's spinner, shimmering verb, token
//! tally (`docs/spinner.md` for the styles; `gravity` is the default) — and
//! the dim `Done for Ns` summary it commits.
//! See `docs/status-indicator.md`.

use super::theme::*;
use super::wrap::{blend_color, breath, clamp_spans, hop, lerp_color, ping_pong};
use super::*;

use crate::app::Spinner;

/// One bold span per char of `text`, shimmered codex-style: a raised-cosine
/// brightness band (half-width [`SHIMMER_BAND_HALF_WIDTH`], plus
/// [`SHIMMER_PADDING`] chars of off-text run-in/out) sweeps the text once per
/// [`SHIMMER_SWEEP`], each char blending from the white-grey [`shimmer_base`]
/// toward the bright [`shimmer_highlight`] by its distance from the band's
/// crest. A faithful port of openai/codex `tui/src/shimmer.rs::shimmer_spans`,
/// made pure: the phase comes from the boundary-supplied `elapsed` (sub-second
/// resolution), not a process-wide clock — so it's deterministic in tests.
///
/// **Live regions only**: every span carries a colour sampled from one frame
/// of the wave, so committing these rows to scrollback would freeze the sweep
/// mid-stride forever.
pub(super) fn shimmer_spans(text: &str, elapsed: Duration) -> Vec<Span<'static>> {
    shimmer_spans_from(text, elapsed, shimmer_base())
}

/// [`shimmer_spans`] with the wave's **resting** colour chosen by the caller —
/// what the text reads as between crests, which is most of the sweep (the band
/// is [`SHIMMER_BAND_HALF_WIDTH`] wide inside a period of the text plus
/// `2 × `[`SHIMMER_PADDING`]).
///
/// The status verb keeps codex's grey [`shimmer_base`], so it reads as *grey
/// text with a white wave*. The thinking stream's `Thinking…`
/// (`docs/thinking-stream.md`) passes the near-white
/// [`reasoning_shimmer_base`] instead, so it reads as *bold white with a
/// brighter wave* — a header, not a metric. Same motion, different floor.
pub(super) fn shimmer_spans_from(text: &str, elapsed: Duration, base: Color) -> Vec<Span<'static>> {
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
            let color = blend_color(shimmer_highlight(), base, t * SHIMMER_MAX_BLEND);
            Span::styled(
                ch.to_string(),
                Style::new().fg(color).add_modifier(Modifier::BOLD),
            )
        })
        .collect()
}

/// A style's frame table and how long each frame shows — the catalog's
/// *look*, kept in `theme` beside every other styling decision
/// (`docs/spinner.md`). `None` for the two **track** styles, which draw
/// themselves ([`track_spans`]): their motion runs on two independent
/// periods, so a table of their frames would run to hundreds of entries. The
/// one-frame `pulse` never steps; it moves by colour alone ([`glyph_color`]).
fn spinner_frames(spinner: Spinner) -> Option<(&'static [&'static str], Duration)> {
    Some(match spinner {
        Spinner::Comet => (SPINNER_FRAMES, SPINNER_INTERVAL),
        Spinner::Sparkle => (SPINNER_SPARKLE_FRAMES, SPINNER_SPARKLE_INTERVAL),
        Spinner::Dots => (SPINNER_DOTS_FRAMES, SPINNER_DOTS_INTERVAL),
        Spinner::Blocks => (SPINNER_BLOCKS_FRAMES, SPINNER_BLOCKS_INTERVAL),
        Spinner::Pulse => (SPINNER_PULSE_FRAMES, SPINNER_PULSE_PERIOD),
        Spinner::Bars => (SPINNER_BARS_FRAMES, SPINNER_BARS_INTERVAL),
        Spinner::Line => (SPINNER_LINE_FRAMES, SPINNER_LINE_INTERVAL),
        Spinner::Gravity | Spinner::Wave => return None,
    })
}

/// The dot bits of a braille cell (`U+2800 + bits`) by dot column (0 left,
/// 1 right) and dot row (0 top … 3 bottom). Unicode numbers dots 1–3 down
/// the left and 4–6 down the right, then 7 and 8 along the bottom — which is
/// why the bottom row's bits (`0x40`, `0x80`) break the doubling pattern.
const BRAILLE_DOTS: [[u8; 4]; 2] = [[0x01, 0x02, 0x04, 0x40], [0x08, 0x10, 0x20, 0x80]];

/// A braille **track**: [`SPINNER_TRACK_CELLS`] cells of 2 × 4 dots — the
/// canvas the `gravity` ball and the `wave` draw on (`docs/spinner.md`), a
/// port of the braille canvas in the bouncing-indicator lab this pair of
/// styles comes from. Each cell carries one colour (a cell is one glyph):
/// the dim [`status_detail_color`] until something coloured lands on it.
struct Track {
    cells: [u8; SPINNER_TRACK_CELLS],
    colors: [Color; SPINNER_TRACK_CELLS],
}

impl Track {
    /// Dot columns across the track — two per cell.
    const COLS: usize = SPINNER_TRACK_CELLS * 2;
    /// Dot rows down a cell.
    const ROWS: usize = 4;

    fn new() -> Self {
        Self {
            cells: [0; SPINNER_TRACK_CELLS],
            colors: [status_detail_color(); SPINNER_TRACK_CELLS],
        }
    }

    /// Light the dot at (`col`, `row`) and tint its cell `color` — or leave
    /// the cell's colour alone for `None` (the floor). A dot off the track
    /// is dropped, so a shape may run over an edge without a check at every
    /// call.
    fn set(&mut self, col: usize, row: usize, color: Option<Color>) {
        if col >= Self::COLS || row >= Self::ROWS {
            return;
        }
        self.cells[col / 2] |= BRAILLE_DOTS[col % 2][row];
        if let Some(color) = color {
            self.colors[col / 2] = color;
        }
    }

    /// The track as one span per cell, the last carrying the separator space
    /// before the verb — the comet's shape, so a track style sits in the
    /// status line exactly as the comet does. Not bold: braille dots are
    /// dense already, and a synthesized bold blurs them.
    fn spans(self) -> Vec<Span<'static>> {
        self.cells
            .iter()
            .zip(self.colors)
            .enumerate()
            .map(|(i, (&bits, color))| {
                // Every value 0x2800..=0x28FF is an assigned braille pattern,
                // so the fallback never fires.
                let glyph = char::from_u32(0x2800 + u32::from(bits)).unwrap_or(' ');
                let text = if i == SPINNER_TRACK_CELLS - 1 {
                    format!("{glyph} ")
                } else {
                    glyph.to_string()
                };
                Span::styled(text, Style::new().fg(color))
            })
            .collect()
    }
}

/// The two track styles' frames at `elapsed`, drawn on a fresh [`Track`]
/// ([`spinner_frames`] answered `None` for exactly these two).
fn track_spans(spinner: Spinner, elapsed: Duration) -> Vec<Span<'static>> {
    let mut track = Track::new();
    match spinner {
        Spinner::Gravity => draw_gravity(&mut track, elapsed),
        Spinner::Wave => draw_wave(&mut track, elapsed),
        other => unreachable!("{other:?} has a frame table"),
    }
    track.spans()
}

/// The `gravity` ball: a floor along the bottom dot row, and a 2 × 2 dot
/// ball ping-ponging along it at constant speed (one round trip per
/// [`SPINNER_GRAVITY_SWEEP`], a hard reversal at each wall) while it hops on
/// a parabola (one hop per [`SPINNER_GRAVITY_HOP`] — four a round trip, so it
/// touches down exactly as it meets each wall). At rest its bottom row
/// shares the floor's, so it reads as landing rather than hovering. The ball
/// wears the banner gradient by where it is on the track — cyan at the left
/// wall, blue at the right — and the cell it sits in takes its colour; the
/// bare floor stays dim.
fn draw_gravity(track: &mut Track, elapsed: Duration) {
    for col in 0..Track::COLS {
        track.set(col, Track::ROWS - 1, None);
    }
    // The ball's left dot column, so its right one stays on the track.
    let reach = (Track::COLS - 2) as f32;
    let col = (ping_pong(elapsed, SPINNER_GRAVITY_SWEEP) * reach) as usize;
    // Its bottom dot row: the floor at both ends of the hop, two rows up at
    // the apex — the ball stays whole all the way up.
    let lift = (hop(elapsed, SPINNER_GRAVITY_HOP) * 2.0).round() as usize;
    let bottom = Track::ROWS - 1 - lift.min(Track::ROWS - 2);
    let color = lerp_color(
        header_gradient_start(),
        header_gradient_end(),
        col as f32 / reach,
    );
    for dx in 0..2 {
        for dy in 0..2 {
            track.set(col + dx, bottom - dy, Some(color));
        }
    }
}

/// The `wave`: one dot per dot column on a sine whose wavelength is the
/// whole track ([`SPINNER_WAVE_LENGTH`] dot columns — a crest and a trough
/// always in view), quantised to the four dot rows. The phase ping-pongs,
/// [`SPINNER_WAVE_TRAVEL`] wavelengths out and the same back per
/// [`SPINNER_WAVE_SWEEP`], so the wave rolls down the track, reflects off
/// the wall and rolls back — and the whole-number travel makes the reversal
/// frame the starting frame, with no seam. Each cell wears the banner
/// gradient by its place on the track: the mascot's own wash, rolling.
fn draw_wave(track: &mut Track, elapsed: Duration) {
    // Reduced to one turn so the reversal lands on a phase of exactly zero.
    let phase = (ping_pong(elapsed, SPINNER_WAVE_SWEEP) * SPINNER_WAVE_TRAVEL).fract()
        * std::f32::consts::TAU;
    let last_cell = (SPINNER_TRACK_CELLS - 1) as f32;
    for col in 0..Track::COLS {
        let y = (std::f32::consts::TAU * col as f32 / SPINNER_WAVE_LENGTH - phase).sin();
        // y = 1 is the crest (dot row 0), y = −1 the trough (dot row 3).
        let row = ((1.0 - y) / 2.0 * (Track::ROWS - 1) as f32)
            .round()
            .clamp(0.0, (Track::ROWS - 1) as f32) as usize;
        let color = lerp_color(
            header_gradient_start(),
            header_gradient_end(),
            (col / 2) as f32 / last_cell,
        );
        track.set(col, row, Some(color));
    }
}

/// The colour of a one-cell style's glyph — what makes the styles more than
/// glyph sets, and where the theme's accent reaches the status line:
///
/// - `sparkle` and `blocks` walk the banner's cyan → blue gradient
///   (`docs/header.md`) — the spark by its bloom level (`·` cyan, `✽` blue,
///   back down the fade), the block by its turn;
/// - `pulse` breathes the running tool bullet's raised cosine
///   ([`breath`], `docs/tool-pulse.md`) from [`spinner_pulse_dim`] to white;
/// - `bars` brightens with height, [`spinner_bars_low`] at `▁` to white at `█`;
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
        Spinner::Sparkle => {
            lerp_color(header_gradient_start(), header_gradient_end(), level(index))
        }
        Spinner::Blocks => lerp_color(
            header_gradient_start(),
            header_gradient_end(),
            index as f32 / len.saturating_sub(1).max(1) as f32,
        ),
        Spinner::Pulse => blend_color(
            spinner_pulse_bright(),
            spinner_pulse_dim(),
            breath(elapsed, SPINNER_PULSE_PERIOD),
        ),
        Spinner::Bars => blend_color(spinner_bars_high(), spinner_bars_low(), level(index)),
        Spinner::Comet | Spinner::Dots | Spinner::Line => status_color(),
        // Never asked: the tracks colour per cell (`draw_gravity`, `draw_wave`).
        Spinner::Gravity | Spinner::Wave => status_color(),
    }
}

/// The spinner opening the status line, in the session's chosen `spinner`
/// style (`docs/spinner.md`): the style's frame for `elapsed` (one frame per
/// its interval, looping), as spans that end in the separator space before
/// the verb. The comet is its own shape ([`comet_spans`], one span per cell),
/// the two track styles draw themselves ([`track_spans`], one span per cell
/// too), and every other style is one glyph in one span, bold, coloured by
/// [`glyph_color`]. Pure, like [`shimmer_spans`]: the frame index derives
/// from the boundary-supplied `elapsed`, and the loop's animation re-arm
/// keeps it advancing — which is also what lets the `/spinner` picker draw
/// each row's live spinner with it.
pub(super) fn spinner_spans(spinner: Spinner, elapsed: Duration) -> Vec<Span<'static>> {
    let Some((frames, interval)) = spinner_frames(spinner) else {
        return track_spans(spinner, elapsed);
    };
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
    let dim = Style::new().fg(status_detail_color());
    let spans: Vec<Span<'static>> = frame
        .chars()
        .enumerate()
        .map(|(i, c)| {
            let style = match c {
                SPINNER_HEAD => Style::new().fg(status_color()).add_modifier(Modifier::BOLD),
                SPINNER_TAIL_MID => Style::new().fg(spinner_tail_color()),
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
pub use crate::app::{format_elapsed, format_timeout};

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
/// `⣤⣀⣀⣀⣀⣀⣀⣀ {verb}… ({elapsed}[ · {arrow} {n} tokens][ · Thinking for {m}] · esc to
/// interrupt)`.
///
/// It opens with the spinner (`spinner_spans`) and the verb
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
/// This is [`styled_status_line`] in the catalog's default style
/// ([`Spinner::Gravity`], the ball on its braille track); the strip itself
/// passes the session's own chosen style (`docs/spinner.md`).
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
/// so completing the task snaps it back mid-turn. In the default style, like
/// [`status_line`].
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
    let dim = Style::new().fg(status_detail_color());
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
            Style::new().fg(status_retry_color()),
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
        // them — the visible proof prompt caching worked — and the share
        // this turn *wrote* to the cache, so the turn that primed it shows
        // it did (an explicit-caching provider bills that write at a
        // premium). A zero half is omitted; absent usage (the dummy, a `!`
        // shell) keeps the bare shape. See docs/prompt-caching.md.
        text.push_str(&format!(" · {} tokens", format_token_count(summary.tokens)));
        let mut cache: Vec<String> = Vec::new();
        if summary.cached > 0 {
            cache.push(format!("{} cached", format_token_count(summary.cached)));
        }
        if summary.cache_write > 0 {
            cache.push(format!(
                "{} written",
                format_token_count(summary.cache_write)
            ));
        }
        if !cache.is_empty() {
            text.push_str(&format!(" ({})", cache.join(" · ")));
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
        .map(|row| Line::from(Span::styled(row, Style::new().fg(status_done_color()))))
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
        bg_notice_ok_color()
    } else {
        bg_notice_fail_color()
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
