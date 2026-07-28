//! The startup banner: the gradient logo, name, tagline, and hint row.

use super::theme::*;
use super::wrap::{clamp_spans, cols, lerp_rgb};
use super::*;

/// Colour `text` with a left-to-right [`HEADER_GRADIENT_START`] →
/// [`HEADER_GRADIENT_END`] gradient keyed by absolute display column across
/// `total` columns, coalescing equal-colour runs into spans. The header logo's
/// cyan → blue wash (docs/header.md).
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
        let color = lerp_rgb(HEADER_GRADIENT_START, HEADER_GRADIENT_END, t);
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

/// The widest display width across `art`'s rows.
fn logo_width(art: &[&str]) -> usize {
    art.iter().map(|row| cols(row)).max().unwrap_or(0)
}

/// The startup header banner as scrollback rows (docs/header.md): the ASCII
/// wordmark (sized to `width`), a blank, then the version, tagline, cwd, and the
/// command hint. Pure chrome — `main.rs` commits it once at launch and restores
/// it atop every repaint via [`banner_tail`] (uncapped on a Purge rebuild,
/// window-capped on an InPlace overlay return); it never enters `history`, and
/// the Ctrl+O transcript shows it as chrome too ([`transcript_lines`]).
/// Returns no trailing spacer (the caller adds one, the
/// `insert_before(msg); insert_before(blank)` pattern).
///
/// `width` picks the widest wordmark that fits — the full block art, a compact
/// half-block, or a one-line text badge on a very narrow terminal — so the
/// banner never overflows; the metadata rows are clamped with a trailing `…`.
#[must_use]
pub fn header_lines(app: &App, width: u16) -> Vec<Line<'static>> {
    let w = width as usize;
    let indent = cols(HEADER_INDENT);
    let accent = Style::new().fg(HEADER_ACCENT_COLOR);
    let dim = Style::new().fg(HEADER_META_COLOR);
    let version = format!("v{}", env!("CARGO_PKG_VERSION"));

    let full_w = logo_width(HEADER_LOGO_FULL);
    let compact_w = logo_width(HEADER_LOGO_COMPACT);

    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut badge = false;

    // The widest wordmark that fits: full block → compact half-block → badge.
    if w >= indent + full_w {
        for &row in HEADER_LOGO_FULL {
            lines.push(Line::from(gradient_spans(row, full_w)));
        }
    } else if w >= indent + compact_w {
        for &row in HEADER_LOGO_COMPACT {
            lines.push(Line::from(gradient_spans(row, compact_w)));
        }
    } else {
        // One-line badge: the gradient name + a cyan version, no meta line.
        let mut spans = vec![Span::raw(HEADER_INDENT)];
        spans.extend(gradient_spans(HEADER_NAME, cols(HEADER_NAME)));
        spans.push(Span::styled(format!(" {version}"), accent));
        lines.push(clamp_spans(spans, w));
        badge = true;
    }

    // A blank between the art and the metadata, then `v… · tagline` — skipped
    // for the badge, which already carries the version inline.
    if !badge {
        lines.push(Line::default());
        lines.push(clamp_spans(
            vec![
                Span::raw(HEADER_INDENT),
                Span::styled(version, accent),
                Span::styled(FOOTER_SEPARATOR, dim),
                Span::styled(HEADER_TAGLINE, dim),
            ],
            w,
        ));
    }

    // The cwd (only with session info) and the command hint, shared by all tiers.
    if let Some(session) = &app.session {
        lines.push(clamp_spans(
            vec![
                Span::raw(HEADER_INDENT),
                Span::styled(session.cwd.clone(), dim),
            ],
            w,
        ));
    }
    let mut hint = vec![Span::raw(HEADER_INDENT)];
    for (i, token) in HEADER_HINT.iter().enumerate() {
        if i > 0 {
            hint.push(Span::styled("   ", dim));
        }
        hint.push(Span::styled(*token, accent));
    }
    lines.push(clamp_spans(hint, w));

    lines
}
