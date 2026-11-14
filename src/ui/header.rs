//! The startup banner: the gradient mascot beside the name, cwd, and hint.
//! See `docs/header.md` and `docs/mascot.md`.

use super::inline::{inline_spans, wrap_inline};
use super::theme::*;
use super::wrap::{clamp_spans, cols, lerp_color, wrap_text};
use super::*;

use crate::app::{Mascot, MascotPose};

/// `text` as spans coloured cell by cell — `color_at(glyph, column)` for each
/// glyph at its display column — with equal-colour runs coalesced.
fn colored_spans(text: &str, color_at: impl Fn(&str, usize) -> Color) -> Vec<Span<'static>> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut col = 0usize;
    let mut run: Option<(Color, String)> = None;
    for g in text.graphemes(true) {
        let color = color_at(g, col);
        match &mut run {
            Some((c, s)) if *c == color => s.push_str(g),
            _ => {
                if let Some((c, s)) = run.take() {
                    spans.push(Span::styled(s, Style::new().fg(c)));
                }
                run = Some((color, g.to_string()));
            }
        }
        col += cols(g);
    }
    if let Some((c, s)) = run {
        spans.push(Span::styled(s, Style::new().fg(c)));
    }
    spans
}

/// Colour `text` with a left-to-right [`header_gradient_start`] →
/// [`header_gradient_end`] gradient keyed by absolute display column across
/// `total` columns, coalescing equal-colour runs into spans. The mascot's
/// cyan → blue wash (docs/header.md) — `total` is the art block's width, so
/// the wash is uniform down the block and a short row simply stops earlier.
pub(super) fn gradient_spans(text: &str, total: usize) -> Vec<Span<'static>> {
    colored_spans(text, |_, col| header_gradient_at(col, total))
}

/// How far a sweep of light lifts the art cell at (`col`, `row`) toward its
/// highlight, `0.0..=`[`MASCOT_SHINE_MAX_BLEND`], `progress` of the way
/// through a sweep across art `total` columns wide and `rows` tall
/// (`docs/mascot-animation.md`).
///
/// The light is a raised-cosine band [`MASCOT_SHINE_HALF_WIDTH`] columns
/// either side of its crest — the status verb's shimmer, sized for nine
/// columns — and each row sits [`MASCOT_SHINE_LEAN`] columns further along
/// the sweep than the row above, so the band crosses the art as a leaning
/// streak rather than a bar. It runs from a half-width before the art to a
/// half-width past it, so a sweep opens on the art unlit and the light
/// arrives and leaves by degrees: no cell pops.
fn shine_at(col: usize, row: usize, rows: usize, total: usize, progress: f32) -> f32 {
    let lean = |row: usize| MASCOT_SHINE_LEAN * row as f32;
    let far = total.saturating_sub(1) as f32 + lean(rows.saturating_sub(1));
    let from = -MASCOT_SHINE_HALF_WIDTH;
    let crest = from + (far + MASCOT_SHINE_HALF_WIDTH - from) * progress;
    let off = (col as f32 + lean(row) - crest).abs();
    if off >= MASCOT_SHINE_HALF_WIDTH {
        return 0.0;
    }
    let lift = 0.5 * (1.0 + (std::f32::consts::PI * off / MASCOT_SHINE_HALF_WIDTH).cos());
    MASCOT_SHINE_MAX_BLEND * lift
}

/// Whether `glyph` is a **spark** — a twinkle beside the art rather than
/// part of it. The mascots are drawn in the Block Elements; anything else a
/// frame holds is light, and is drawn in the sweep's own colour.
fn is_spark(glyph: &str) -> bool {
    glyph
        .chars()
        .next()
        .is_some_and(|c| c != ' ' && !('\u{2580}'..='\u{259F}').contains(&c))
}

/// One row of a mascot's art in the banner gradient, as `pose` lights it:
/// each cell its gradient colour, lifted toward [`mascot_shine_color`] where
/// a sweep is passing ([`shine_at`]), and a spark in that colour outright.
/// At rest this is [`gradient_spans`] exactly, so the banner scrollback
/// holds is the one it always held.
fn art_spans(
    text: &str,
    row: usize,
    rows: usize,
    total: usize,
    shine: Option<f32>,
) -> Vec<Span<'static>> {
    colored_spans(text, |glyph, col| {
        if is_spark(glyph) {
            return mascot_shine_color();
        }
        let color = header_gradient_at(col, total);
        match shine.map(|progress| shine_at(col, row, rows, total, progress)) {
            Some(lift) if lift > 0.0 => lerp_color(color, mascot_shine_color(), lift),
            _ => color,
        }
    })
}

/// A page title's spans — the one title dress every composer-replacing page
/// shares (`/hooks`, `/mcp`, `/skills`, the startup review, `/export`, the ↓ manager,
/// `/login`'s pages, `/donate`, `/secrets`): bold and washed in the active theme's
/// banner gradient across the title's own width, so a headline follows
/// `/theme` the way the banner does (`docs/theme.md`).
pub(super) fn page_title_spans(text: &str) -> Vec<Span<'static>> {
    gradient_spans(text, cols(text))
        .into_iter()
        .map(|span| Span::styled(span.content, span.style.add_modifier(Modifier::BOLD)))
        .collect()
}

/// A page's title row: [`page_title_spans`] at the pages' inset, `…`-cut to
/// a width that cannot seat it.
pub(super) fn page_title_line(text: &str, width: u16) -> Line<'static> {
    let room = (width as usize).saturating_sub(cols(MODEL_INDENT));
    let shown = super::wrap::ellipsize(text, room);
    let mut spans = vec![Span::raw(MODEL_INDENT)];
    spans.extend(page_title_spans(&shown));
    Line::from(spans)
}

/// The metadata column beside the mascot, one span-row each: the bold name +
/// dim `(v…)` version, the cwd in the footer's path colour (only with
/// session info), and the key hint — `/ commands  @ files  ? shortcuts`,
/// each key in the accent beside its dim label ([`HEADER_HINT`]), the pairs
/// [`HEADER_HINT_GAP`] columns apart.
fn meta_rows(app: &App) -> Vec<Vec<Span<'static>>> {
    let accent = Style::new().fg(header_accent_color());
    let dim = Style::new().fg(header_meta_color());
    let mut rows: Vec<Vec<Span<'static>>> = vec![vec![
        Span::styled(HEADER_NAME, Style::new().add_modifier(Modifier::BOLD)),
        Span::styled(format!(" (v{})", env!("CARGO_PKG_VERSION")), dim),
    ]];
    if let Some(session) = &app.session {
        rows.push(vec![Span::styled(
            session.cwd.clone(),
            Style::new().fg(header_cwd_color()),
        )]);
    }
    let mut hint: Vec<Span<'static>> = Vec::new();
    for (i, &(key, label)) in HEADER_HINT.iter().enumerate() {
        if i > 0 {
            hint.push(Span::styled(" ".repeat(HEADER_HINT_GAP), dim));
        }
        hint.push(Span::styled(key, accent));
        hint.push(Span::styled(format!(" {label}"), dim));
    }
    rows.push(hint);
    rows
}

/// A one-row startup notice as scrollback chrome — the checkpoint
/// pre-flight's `Snapshotting …` line (`docs/checkpoint.md`), committed
/// above the banner by the boundary. The banner's own indent and dim meta
/// colour, so the two read as one block; clamped with a trailing `…` at a
/// narrow width (a status row, not prose), like every banner metadata row.
/// Chrome like the banner, it never enters `history` — but unlike the
/// banner it is a one-time startup fact, so a purge rebuild (resize,
/// `/clear`) does not re-emit it.
#[must_use]
pub fn startup_notice_lines(text: &str, width: u16) -> Vec<Line<'static>> {
    vec![clamp_spans(
        vec![
            Span::raw(HEADER_INDENT),
            Span::styled(text.to_string(), Style::new().fg(header_meta_color())),
        ],
        width as usize,
    )]
}

/// A wrapped startup paragraph as scrollback chrome, committed under the
/// banner by the boundary. [`startup_notice_lines`]' sibling with the opposite
/// rule at a narrow width: that one clamps, because a status row is glanced at; this
/// one **wraps**, because a sentence the user is meant to finish must not
/// lose its ending. Every row wears the banner's indent and dim meta colour,
/// so the block reads as part of the banner; like the notice it never enters
/// `history`, and a purge rebuild does not re-emit it.
#[must_use]
pub fn startup_paragraph_lines(text: &str, width: u16) -> Vec<Line<'static>> {
    let indent = u16::try_from(cols(HEADER_INDENT)).unwrap_or(u16::MAX);
    // A width the indent alone would eat still wraps at one column rather
    // than disabling the wrap (`wrap_text(…, 0)` would).
    let inner = width.saturating_sub(indent).max(1);
    let dim = Style::new().fg(header_meta_color());
    wrap_text(text, inner)
        .into_iter()
        .map(|row| Line::from(vec![Span::raw(HEADER_INDENT), Span::styled(row, dim)]))
        .collect()
}

/// The one-time telemetry disclosure as a compact, themed card under the
/// banner (`docs/telemetry.md`): [`notice_card_lines`] over
/// `telemetry::notice()`.
#[must_use]
pub fn telemetry_notice_lines(width: u16) -> Vec<Line<'static>> {
    notice_card_lines(TELEMETRY_CARD_TITLE, &crate::telemetry::notice(), width)
}

/// The update notice as the same card (`docs/update.md`): [`notice_card_lines`]
/// over `update::notice()` — a newer release `latest` is out, this is
/// `current`, `repo` is where the release page and the installer live, and
/// `command` updates this install (`update::update_command`).
#[must_use]
pub fn update_notice_lines(
    width: u16,
    current: &str,
    latest: &str,
    repo: &str,
    command: &str,
) -> Vec<Line<'static>> {
    notice_card_lines(
        UPDATE_CARD_TITLE,
        &crate::update::notice(current, latest, repo, command),
        width,
    )
}

/// A startup notice as a compact, themed card under the banner: `heading`
/// in the frame's top rule, `body` — inline markdown, paragraphs split on
/// newlines — inside it. Labels are bold and commands use the banner's
/// accent. Prose and commands wrap without truncation; tiny panes drop the
/// frame instead of spending their remaining columns on decoration. Like
/// other startup notices, this is scrollback chrome and never enters
/// history. One builder for every such card, so the telemetry disclosure
/// and the update notice can never drift apart in dress.
#[must_use]
pub fn notice_card_lines(heading: &str, body: &str, width: u16) -> Vec<Line<'static>> {
    if width == 0 {
        return Vec::new();
    }
    let indent = if usize::from(width) > cols(HEADER_INDENT) {
        HEADER_INDENT
    } else {
        ""
    };
    let card_width = usize::from(width)
        .saturating_sub(2 * cols(indent))
        .min(NOTICE_CARD_MAX_WIDTH);
    let framed = card_width >= NOTICE_CARD_MIN_WIDTH;
    let inner = if framed {
        card_width - 2 - 2 * cols(DEVICE_BOX_PAD)
    } else {
        usize::from(width) - cols(indent)
    };
    let border = Style::new().fg(border_color());
    let title = Style::new()
        .fg(header_accent_color())
        .add_modifier(Modifier::BOLD);
    let text = Style::new().fg(telemetry_text_color());
    let mut lines = Vec::new();
    if framed {
        let title_text = format!(" {heading} ");
        lines.push(Line::from(vec![
            Span::raw(indent),
            Span::styled(
                format!("{DEVICE_BOX_TOP_LEFT}{DEVICE_BOX_HORIZONTAL}"),
                border,
            ),
            Span::styled(title_text.clone(), title),
            Span::styled(
                format!(
                    "{}{DEVICE_BOX_TOP_RIGHT}",
                    DEVICE_BOX_HORIZONTAL.repeat(card_width - 3 - cols(&title_text))
                ),
                border,
            ),
        ]));
    } else {
        for row in wrap_inline(&[(heading.to_string(), title)], inner as u16) {
            let mut spans = vec![Span::raw(indent)];
            spans.extend(row);
            lines.push(Line::from(spans));
        }
    }
    for paragraph in body.split('\n') {
        let segments = inline_spans(&markdown::parse_inline(paragraph), text);
        for row in wrap_inline(&segments, inner as u16) {
            let used: usize = row.iter().map(|span| cols(&span.content)).sum();
            let mut spans = vec![Span::raw(indent)];
            if framed {
                spans.push(Span::styled(DEVICE_BOX_VERTICAL, border));
                spans.push(Span::raw(DEVICE_BOX_PAD));
            }
            spans.extend(row);
            if framed {
                spans.push(Span::raw(" ".repeat(inner.saturating_sub(used))));
                spans.push(Span::raw(DEVICE_BOX_PAD));
                spans.push(Span::styled(DEVICE_BOX_VERTICAL, border));
            }
            lines.push(Line::from(spans));
        }
    }
    if framed {
        lines.push(Line::from(vec![
            Span::raw(indent),
            Span::styled(
                format!(
                    "{DEVICE_BOX_BOTTOM_LEFT}{}{DEVICE_BOX_BOTTOM_RIGHT}",
                    DEVICE_BOX_HORIZONTAL.repeat(card_width - 2)
                ),
                border,
            ),
        ]));
    }
    lines
}

/// The startup header banner as scrollback rows (docs/header.md): the
/// session's mascot in the banner gradient, the metadata column beside it.
/// Pure chrome — the boundary commits it once at launch and restores it atop
/// every purge rebuild via [`banner_tail`]; it never enters `history`, and
/// the Ctrl+O transcript shows it as chrome too ([`transcript_lines`]).
/// Returns no trailing spacer (the caller adds one, the
/// `insert_before(msg); insert_before(blank)` pattern).
#[must_use]
pub fn header_lines(app: &App, width: u16) -> Vec<Line<'static>> {
    header_lines_for(app, app.mascot(), width)
}

/// The banner as it would render with `mascot` standing still — every banner
/// that is not the live one: what the boundary commits to scrollback, what
/// tops the Ctrl+O transcript and an export, and the slot the `/mascot`
/// picker sizes its preview by.
#[must_use]
pub(super) fn header_lines_for(app: &App, mascot: Mascot, width: u16) -> Vec<Line<'static>> {
    header_lines_posed(app, mascot, &MascotPose::rest(mascot), width)
}

/// Whether a banner for `mascot` seats its art at `width` — beside the full
/// title — or falls back to the one-line text badge.
fn seats_art(mascot: Mascot, width: u16) -> bool {
    let title_w = cols(HEADER_NAME) + cols(&format!(" (v{})", env!("CARGO_PKG_VERSION")));
    usize::from(width) >= mascot.art_width() + HEADER_ART_GAP + title_w
}

/// The banner as it would render with `mascot` in `pose` — the one builder
/// behind every banner (docs/mascot.md), so the startup banner, the live one
/// and the `/mascot` picker's preview can never disagree.
///
/// Two tiers: the mascot banner (the pose's art rows flush-left wearing the
/// gradient and whatever light the pose carries, each metadata row at
/// `art width + gap`, clamped with a trailing `…`), or — when the width
/// can't seat the art beside the full title — a one-line text badge over the
/// same clamped metadata rows, which has no art and so no pose.
///
/// A pose changes what the art's cells hold and nothing else: its rows are
/// the rest art's count and never wider than [`Mascot::art_width`], which is
/// where the metadata column is measured from (docs/mascot-animation.md).
#[must_use]
pub(super) fn header_lines_posed(
    app: &App,
    mascot: Mascot,
    pose: &MascotPose,
    width: u16,
) -> Vec<Line<'static>> {
    let w = width as usize;
    let meta = meta_rows(app);
    let art = pose.art;
    let art_w = mascot.art_width();

    // Too narrow to seat the art beside the title: the one-line badge — the
    // gradient name + an accent version — over the same metadata rows.
    if !seats_art(mascot, width) {
        let accent = Style::new().fg(header_accent_color());
        let mut badge = vec![Span::raw(HEADER_INDENT)];
        badge.extend(gradient_spans(HEADER_NAME, cols(HEADER_NAME)));
        badge.push(Span::styled(
            format!(" v{}", env!("CARGO_PKG_VERSION")),
            accent,
        ));
        let mut lines = vec![clamp_spans(badge, w)];
        // The cwd and hint rows (the title row is the badge itself).
        for row in meta.into_iter().skip(1) {
            let mut spans = vec![Span::raw(HEADER_INDENT)];
            spans.extend(row);
            lines.push(clamp_spans(spans, w));
        }
        return lines;
    }

    // The mascot banner: art rows on the left (wearing the gradient), the
    // metadata column beside them. The metadata block sits **vertically
    // centered** in a taller mascot, ties resolving downward — the text seats
    // low rather than hanging off the art's top, so the pair reads as one
    // composed badge. Today's catalog is uniformly 3 rows, which is exactly
    // the metadata's height *with* a session; the live case is the
    // session-less banner (title + hint only), whose two rows seat at 1–2.
    // Art rows without a metadata neighbour render alone (no trailing pad),
    // and metadata rows past the art would keep their column.
    let offset = art.len().saturating_sub(meta.len()).div_ceil(2);
    let rows = art.len().max(offset + meta.len());
    let mut lines: Vec<Line<'static>> = Vec::new();
    for i in 0..rows {
        let mut spans: Vec<Span<'static>> = Vec::new();
        let art_cols = art.get(i).map_or(0, |row| cols(row));
        if let Some(row) = art.get(i) {
            spans.extend(art_spans(row, i, art.len(), art_w, pose.shine));
        }
        if let Some(meta_row) = i.checked_sub(offset).and_then(|k| meta.get(k)) {
            spans.push(Span::raw(
                " ".repeat(art_w.saturating_sub(art_cols) + HEADER_ART_GAP),
            ));
            spans.extend(meta_row.iter().cloned());
        }
        lines.push(clamp_spans(spans, w));
    }
    lines
}

/// The banner over its blank spacer, standing still — the block the boundary
/// commits above an empty conversation (`banner_tail` of no tail), and the
/// rest state the live banner is restored to before it can scroll away
/// (docs/mascot-animation.md).
#[must_use]
pub fn banner_block_lines(app: &App, width: u16) -> Vec<Line<'static>> {
    let mut lines = header_lines(app, width);
    lines.push(Line::default());
    lines
}

/// The banner over its blank spacer **as it should look now**: the session's
/// mascot in its animation's current pose ([`App::banner_pose`]) while the
/// conversation under it is empty, and [`banner_block_lines`] exactly
/// whenever nothing is moving — so a frame at rest has nothing to repaint.
/// What the boundary repaints the committed block with, in place
/// (docs/mascot-animation.md).
#[must_use]
pub fn banner_frame_lines(app: &App, width: u16) -> Vec<Line<'static>> {
    let mut lines = header_lines_posed(app, app.mascot(), &app.banner_pose(), width);
    lines.push(Line::default());
    lines
}

/// Whether the session's banner draws its mascot at `width` — `false` on a
/// terminal so narrow it shows the text badge, which has nothing to animate
/// and so nothing for the loop to schedule a frame for.
#[must_use]
pub fn banner_shows_art(app: &App, width: u16) -> bool {
    seats_art(app.mascot(), width)
}
