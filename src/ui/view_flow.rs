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
//!
//! What that signature is computed from is the one per-view choice
//! ([`FlowSign`]): a page that changes only on a keystroke signs its **rows**,
//! so any edit to them re-flows; the ↓ manager's details page, which
//! live-tails a running shell at the open band's 32 ms animation cadence,
//! signs the **shell it describes** instead and its flowed top freezes in
//! scrollback — frozen text being what scrollback holds anyway, and far
//! better than the rows landing in no buffer at all (the reported bug).

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

/// What an open framed view's flow [`signature`](ViewFlow::signature) is
/// computed from — the one per-view choice in the flow policy.
///
/// The signature exists so the boundary can tell a flow that is still the one
/// on screen from one that has gone stale and needs the purge rebuild that
/// re-flows it. Which changes count is a property of the page:
enum FlowSign {
    /// Sign the **flowed rows' own text**: every change to them re-signs, and
    /// the rebuild re-flows the new rows. The rule for every page that
    /// changes only on a keystroke — a menu's navigation, a picker's search
    /// line, an answered question — which is all of them but one.
    Rows,
    /// Sign a **stable key** naming the page instead, so its flowed rows
    /// freeze in scrollback once committed and only a structural move (a
    /// different page, a resize) re-signs. The rule for a page that ticks on
    /// its own: the ↓ manager's details page advances a runtime and tails a
    /// running shell's output every frame, so signing its rows would purge-
    /// rebuild the whole screen thirty times a second — and dropping them
    /// into no buffer, which is what it used to do, is the bug this fixes.
    /// A frozen row is what the terminal's scrollback holds for everything
    /// else on the screen, so the page still reads whole across the seam.
    Frozen(u64),
}

/// A flow-eligible page: its whole content, what its flow is signed on, and
/// the rows its **painted tail** gets — the whole terminal for a framed view,
/// which the region is clamped to and whose body `view_split` pins at the
/// bottom, and the strip's own slice for the streaming strip
/// (`docs/strip-flow.md`). Everything above that tail flows.
struct FlowPage {
    lines: Vec<Line<'static>>,
    sign: FlowSign,
    painted: u16,
}

impl FlowPage {
    /// A framed view's page: signed on its rows, painted into the terminal.
    fn framed(lines: Vec<Line<'static>>, term_height: u16) -> Self {
        Self {
            lines,
            sign: FlowSign::Rows,
            painted: term_height.max(1),
        }
    }
}

/// The rows of an open framed view that must flow into the terminal's real
/// scrollback, with the signature identifying the flowed state. See
/// [`view_flow`].
pub struct ViewFlow {
    /// What the boundary compares each frame to decide whether the flow it
    /// committed is still the one on screen: the flowed rows' text at this
    /// width and count, or — for a page that ticks — the stable key naming
    /// it, so the rows freeze instead (`FlowSign`, `docs/view-flow.md`).
    pub signature: u64,
    /// The flowed rows, in page order, ending where the painted tail begins.
    pub lines: Vec<Line<'static>>,
}

/// The full page lines of the flow-eligible framed view currently **painted**
/// — `None` when none is. Eligibility mirrors `render_live_with_preview`'s
/// precedence exactly: a view flows only while it is the one on screen, so
/// an open modal (which paints *instead* of everything below it) is the one
/// that flows, and the boundary's signature check cleans the covered view's
/// rows up. Every content-driven framed view flows — the two modals and the
/// windowed pickers included: their pages are line builders like the menus'
/// (`docs/view-flow.md`). The ask modal's page is static per state (its
/// chips, options and entry fields change only on a keystroke, which
/// re-signs the flow — the picker-search rule); the permission prompt's
/// builder needs `term_height` for its keep-context decision (a ticking
/// agent tree rides only a page that fits — never the flow); a picker's
/// search line sits in the page top, and a keystroke re-signs the flow so
/// the typed query re-flows with it. The ↓ background manager flows too — it
/// and the `/spinner` picker are the two pages that tick between keystrokes,
/// so they are the two signed [`FlowSign::Frozen`] rather than on their rows.
///
/// Each branch returns the page **with** what its flow is signed on, so a
/// view cannot acquire a flow without stating which changes to it matter.
fn flow_page(app: &App, width: u16, term_height: u16) -> Option<FlowPage> {
    if app.ask().is_some() {
        // The `AskUserQuestion` modal (`docs/ask.md`): its top-drop clamp
        // used to put the chip strip, the question and the first options in
        // no buffer at all on a short terminal.
        return Some(FlowPage::framed(
            super::ask_view::ask_lines(app, width),
            term_height,
        ));
    }
    if app.permission().is_some() {
        return Some(FlowPage::framed(
            super::permission_view::permission_lines(app, width, term_height),
            term_height,
        ));
    }
    if let Some(picker) = &app.model_picker {
        return Some(FlowPage::framed(
            super::model_view::model_view_lines(picker, width),
            term_height,
        ));
    }
    if let Some(onboarding) = &app.key_onboarding {
        return Some(FlowPage::framed(
            super::login_view::key_onboarding_lines(onboarding, width),
            term_height,
        ));
    }
    if app.settings_picker.is_some() {
        return Some(FlowPage::framed(
            super::settings_view::settings_view_lines(app, width),
            term_height,
        ));
    }
    if app.mascot_picker.is_some() {
        return Some(FlowPage::framed(
            super::mascot_view::mascot_view_lines(app, width),
            term_height,
        ));
    }
    if app.theme_picker.is_some() {
        // The `/theme` picker's page is still — its preview is built from
        // finished cells, nothing on it ticks — so it signs its rows like
        // the `/mascot` picker (`docs/theme.md`).
        return Some(FlowPage::framed(
            super::theme_view::theme_view_lines(app, width),
            term_height,
        ));
    }
    if let Some(picker) = &app.spinner_picker {
        // The `/spinner` picker is the other page that ticks between
        // keystrokes — every row's spinner and the preview line animate at
        // the 32 ms cadence an open picker keeps running
        // (`App::wants_animation_frames`) — so, like the manager's details
        // page, it signs the **selection and the query** rather than its
        // rows: a frame never churns a purge rebuild, while a keystroke that
        // moves the highlight or edits the search re-flows the page
        // (`docs/spinner.md`).
        let mut hasher = DefaultHasher::new();
        picker.selected.hash(&mut hasher);
        picker.query.hash(&mut hasher);
        return Some(FlowPage {
            lines: super::spinner_view::spinner_view_lines(app, width),
            sign: FlowSign::Frozen(hasher.finish()),
            painted: term_height.max(1),
        });
    }
    if app.skills_menu.is_some() {
        return Some(FlowPage::framed(
            super::skills_view::skills_view_lines(app, width),
            term_height,
        ));
    }
    if app.hooks_menu.is_some() {
        return Some(FlowPage::framed(
            super::hooks_view::hooks_view_lines(app, width),
            term_height,
        ));
    }
    if app.donate_picker.is_some() {
        // The `/donate` page is still — nothing on it ticks — so it signs
        // its rows like the `/hooks` menu (`docs/donate.md`).
        return Some(FlowPage::framed(
            super::donate_view::donate_view_lines(app, width),
            term_height,
        ));
    }
    if app.export_picker.is_some() {
        // The `/export` page is still too — its sibling's rule
        // (`docs/export.md`).
        return Some(FlowPage::framed(
            super::export_view::export_view_lines(app, width),
            term_height,
        ));
    }
    if app.trust_menu.is_some() {
        return Some(FlowPage::framed(
            super::trust_view::trust_view_lines(app, width),
            term_height,
        ));
    }
    if app.mcp_menu.is_some() {
        return Some(FlowPage::framed(
            super::mcp_view::mcp_view_lines(app, width),
            term_height,
        ));
    }
    if let Some(view) = &app.background_view {
        // The ↓ manager band, last in the render precedence. Its **details**
        // page live-tails: the runtime advances, the output box fills and the
        // `Showing N lines` caption follows it, all at the 32 ms cadence an
        // open band keeps running (`App::wants_animation_frames`). So it is
        // signed on the shell it describes and its flowed top freezes —
        // where before it was skipped into no buffer at all, leaving the
        // conversation running straight into a headless box
        // (`docs/background.md`). The **list** page changes only when a key
        // moves the `❯` or a shell exits, so it keeps the ordinary rule.
        let sign = match view {
            BackgroundView::Details { id } if app.background_shell(id).is_some() => {
                let mut hasher = DefaultHasher::new();
                id.hash(&mut hasher);
                FlowSign::Frozen(hasher.finish())
            }
            // A details view whose shell has gone renders the **list**
            // (`background_view_lines`' defensive fallback), which is a page
            // that moves — so it signs like one.
            _ => FlowSign::Rows,
        };
        return Some(FlowPage {
            lines: super::background_view::background_view_lines(app, width),
            sign,
            painted: term_height.max(1),
        });
    }
    // No framed view is painted, so the composer is on screen and the
    // **streaming strip** above it is what can overflow: a running tool
    // cell whose output the region cannot fit, the status line under it.
    // Those rows used to be trimmed into no buffer at all; they flow now,
    // frozen like the band's (`docs/strip-flow.md`). Last, because every
    // branch above replaces the composer — `view_split` squeezes the strip
    // for them, and there the view's own page is the one that flows.
    let painted = super::layout::strip_paint_rows(app, width, term_height);
    let preview_n = super::live::strip_content_preview_rows(app, width, term_height);
    // The row count first — this check runs on every draw tick, and a strip
    // that fits must not cost a full build (`strip_content_rows`).
    if super::live::strip_content_rows(app, width, preview_n) <= painted {
        return None;
    }
    let lines = super::live::strip_lines(app, width, None, preview_n);
    if lines.len() > usize::from(painted) {
        return Some(FlowPage {
            sign: FlowSign::Frozen(super::live::strip_flow_key(app)),
            lines,
            painted,
        });
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
    let FlowPage {
        mut lines,
        sign,
        painted,
    } = flow_page(app, width, term_height)?;
    // The painted tail is the page's last `painted` rows — the whole terminal
    // for a framed view, the strip's own slice for the streaming strip.
    // Anything above them flows.
    let skip = view_body_skip(lines.len(), painted);
    if skip == 0 {
        return None;
    }
    lines.truncate(skip);
    if lines.len() > max_rows {
        lines.drain(..lines.len() - max_rows);
    }
    // Both regimes hash the width and the flowed row **count** first: those
    // are what a resize moves, and the count is the skip, so a terminal that
    // grew or shrank re-signs whatever the page is signed on.
    let mut hasher = DefaultHasher::new();
    width.hash(&mut hasher);
    lines.len().hash(&mut hasher);
    match sign {
        FlowSign::Rows => {
            // A discriminant, so a frozen key can never collide with a row
            // hash that happens to reach the same state.
            0u8.hash(&mut hasher);
            for line in &lines {
                for span in &line.spans {
                    span.content.as_ref().hash(&mut hasher);
                }
                // A row boundary, so span text can't run together across rows.
                0xffu8.hash(&mut hasher);
            }
        }
        FlowSign::Frozen(key) => {
            1u8.hash(&mut hasher);
            key.hash(&mut hasher);
        }
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
