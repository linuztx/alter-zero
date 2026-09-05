//! The startup banner: the gradient mascot beside the name, cwd, and hint.
//! See `docs/header.md` and `docs/mascot.md`.

use super::theme::*;
use super::wrap::{clamp_spans, cols, lerp_color};
use super::*;

use crate::app::Mascot;

/// Colour `text` with a left-to-right [`header_gradient_start`] →
/// [`header_gradient_end`] gradient keyed by absolute display column across
/// `total` columns, coalescing equal-colour runs into spans. The mascot's
/// cyan → blue wash (docs/header.md) — `total` is the art block's width, so
/// the wash is uniform down the block and a short row simply stops earlier.
fn gradient_spans(text: &str, total: usize) -> Vec<Span<'static>> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut col = 0usize;
    let mut run: Option<(Color, String)> = None;
    for g in text.graphemes(true) {
        let t = if total <= 1 {
            0.0
        } else {
            col as f32 / (total - 1) as f32
        };
        let color = lerp_color(header_gradient_start(), header_gradient_end(), t);
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

/// The metadata column beside the mascot, one span-row each: the bold name +
/// dim `(v…)` version, the dim cwd (only with session info), and the cyan
/// command hint.
fn meta_rows(app: &App) -> Vec<Vec<Span<'static>>> {
    let accent = Style::new().fg(header_accent_color());
    let dim = Style::new().fg(header_meta_color());
    let mut rows: Vec<Vec<Span<'static>>> = vec![vec![
        Span::styled(HEADER_NAME, Style::new().add_modifier(Modifier::BOLD)),
        Span::styled(format!(" (v{})", env!("CARGO_PKG_VERSION")), dim),
    ]];
    if let Some(session) = &app.session {
        rows.push(vec![Span::styled(session.cwd.clone(), dim)]);
    }
    let mut hint: Vec<Span<'static>> = Vec::new();
    for (i, token) in HEADER_HINT.iter().enumerate() {
        if i > 0 {
            hint.push(Span::styled("   ", dim));
        }
        hint.push(Span::styled(*token, accent));
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

/// The banner as it would render with `mascot` — the `/mascot` picker's live
/// preview calls this directly, so the preview and the real banner share one
/// builder and can never disagree (docs/mascot.md).
///
/// Two tiers: the mascot banner (art rows flush-left wearing the gradient,
/// each metadata row at `art width + gap`, clamped with a trailing `…`), or —
/// when the width can't seat the art beside the full title — a one-line text
/// badge over the same clamped metadata rows.
#[must_use]
pub(super) fn header_lines_for(app: &App, mascot: Mascot, width: u16) -> Vec<Line<'static>> {
    let w = width as usize;
    let meta = meta_rows(app);
    let art = mascot.art();
    let art_w = mascot.art_width();
    let title_w = cols(HEADER_NAME) + cols(&format!(" (v{})", env!("CARGO_PKG_VERSION")));

    // Too narrow to seat the art beside the title: the one-line badge — the
    // gradient name + an accent version — over the same metadata rows.
    if w < art_w + HEADER_ART_GAP + title_w {
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
            spans.extend(gradient_spans(row, art_w));
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
