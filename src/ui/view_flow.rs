//! Long inline-view content presented as text — the bottom anchor and the
//! scrollback flow. See `docs/view-flow.md`.
//!
//! The content-driven framed views (`/mcp`, `/hooks`, `/trust`, the ↓
//! background manager) build their whole page as lines; a page taller than
//! the rows the region gives it used to clip at the **bottom** — losing the
//! hint and the closing rule, with no sign there was more. Two rules fix it:
//!
//! 1. **Bottom anchor** ([`view_body_skip`], [`render_framed_tail`]): the
//!    paint shows the page's *last* rows, so the interactive tail — action
//!    rows, `Esc to go back`, the bottom rule — is always on screen.
//! 2. **Flow** ([`view_flow`]): when the page exceeds the whole terminal, the
//!    skipped top rows are committed above the live region into the
//!    terminal's **real scrollback**, where the terminal's own scrolling
//!    reads them — the page stays whole, split across scrollback and screen
//!    exactly where the region's top is. The boundary keeps the flowed
//!    state's [`signature`](ViewFlow::signature) and answers any change with
//!    the standard purge rebuild, so no stale view text survives a
//!    navigation, a resize, or the close.

use std::hash::{DefaultHasher, Hash, Hasher};

use super::*;

/// How many of a framed view body's `total` lines the paint must skip so the
/// page's tail fits an `height`-row area — the bottom anchor. 0 while it fits.
#[must_use]
pub fn view_body_skip(total: usize, height: u16) -> usize {
    total.saturating_sub(usize::from(height))
}

/// Paint a framed view body bottom-anchored: the last `area.height` rows of
/// `lines` (all of them while they fit). The shared paint of the four
/// content-driven framed views; [`view_body_skip`] is the policy, so the
/// cursor seat (`menu_marker_seat`) can subtract the same skip.
pub(super) fn render_framed_tail(area: Rect, buf: &mut Buffer, mut lines: Vec<Line<'static>>) {
    let skip = view_body_skip(lines.len(), area.height);
    if skip > 0 {
        lines.drain(..skip);
    }
    Paragraph::new(lines).render(area, buf);
}

/// The rows of an open framed view that must flow into the terminal's real
/// scrollback, with the signature identifying the flowed state. See
/// [`view_flow`].
pub struct ViewFlow {
    /// A hash of the flowed rows' text at this width — what the boundary
    /// compares each frame to decide whether the flow it committed is still
    /// the one on screen (`docs/view-flow.md`).
    pub signature: u64,
    /// The flowed rows, in page order, ending where the painted tail begins.
    pub lines: Vec<Line<'static>>,
}

/// The full page lines of the flow-eligible framed view currently **painted**
/// — `None` when none is. Eligibility mirrors `render_live_with_preview`'s
/// precedence exactly: a view flows only while it is the one on screen, so an
/// ask/permission modal (which paints *instead* of everything below it, with
/// its own cap-and-pad layout) suppresses the flow, and the boundary's
/// signature check then cleans any flowed rows up. Every content-driven
/// framed view flows — the windowed pickers included: their pages are line
/// builders like the menus' (`docs/view-flow.md`), and though their search
/// line sits in the page top, a keystroke re-signs the flow so the typed
/// query re-flows with it. The one stay-out is the ↓ background manager,
/// whose details page live-tails a running shell — per-frame content would
/// churn the flow's purge rebuild every tick, so it bottom-anchors only.
fn flow_view_lines(app: &App, width: u16) -> Option<Vec<Line<'static>>> {
    if app.ask().is_some() || app.permission().is_some() {
        return None;
    }
    if let Some(picker) = &app.model_picker {
        return Some(super::model_view::model_view_lines(picker, width));
    }
    if let Some(onboarding) = &app.key_onboarding {
        return Some(super::login_view::key_onboarding_lines(onboarding, width));
    }
    if app.settings_picker.is_some() {
        return Some(super::settings_view::settings_view_lines(app, width));
    }
    if app.skills_menu.is_some() {
        return Some(super::skills_view::skills_view_lines(app, width));
    }
    if app.hooks_menu.is_some() {
        return Some(super::hooks_view::hooks_view_lines(app, width));
    }
    if app.trust_menu.is_some() {
        return Some(super::trust_view::trust_view_lines(app, width));
    }
    if app.mcp_menu.is_some() {
        return Some(super::mcp_view::mcp_view_lines(app, width));
    }
    None
}

/// The flow decision (`docs/view-flow.md`): when the painted framed view's
/// page is taller than the whole terminal, the rows the bottom anchor skips
/// ([`view_body_skip`] — the page's top) must be committed above the live
/// region, so the terminal's own scrolling reads the whole page. `None` while
/// everything fits (the region handles it alone) or no eligible view is
/// painted. `max_rows` caps the flowed rows, keeping the ones nearest the
/// painted tail (the purge-rebuild write cap, `RESIZE_REFLOW_MAX_ROWS`-style)
/// so a pathological page can't turn one navigation into an unbounded write.
#[must_use]
pub fn view_flow(app: &App, width: u16, term_height: u16, max_rows: usize) -> Option<ViewFlow> {
    let mut lines = flow_view_lines(app, width)?;
    // The region is clamped to the terminal and the body is bottom-pinned
    // (`view_split` squeezes the strip first), so the painted tail is the
    // page's last `term_height` rows — anything above them flows.
    let skip = view_body_skip(lines.len(), term_height.max(1));
    if skip == 0 {
        return None;
    }
    lines.truncate(skip);
    if lines.len() > max_rows {
        lines.drain(..lines.len() - max_rows);
    }
    let mut hasher = DefaultHasher::new();
    width.hash(&mut hasher);
    lines.len().hash(&mut hasher);
    for line in &lines {
        for span in &line.spans {
            span.content.as_ref().hash(&mut hasher);
        }
        // A row boundary, so span text can't run together across rows.
        0xffu8.hash(&mut hasher);
    }
    Some(ViewFlow {
        signature: hasher.finish(),
        lines,
    })
}

/// The signature [`view_flow`] would return — the sig-only convenience the
/// boundary's draw tick compares every frame (the built lines are discarded;
/// the rebuild path calls [`view_flow`] itself when it needs them).
#[must_use]
pub fn view_flow_signature(
    app: &App,
    width: u16,
    term_height: u16,
    max_rows: usize,
) -> Option<u64> {
    view_flow(app, width, term_height, max_rows).map(|flow| flow.signature)
}
