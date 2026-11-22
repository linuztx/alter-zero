//! The read-only `/sponsor` page. See `docs/sponsor.md`.
//!
//! The `/donate` page's frame and parts ([`super::donate_view`]) over the
//! const sponsor catalog: the heart and the gradient title, a dim blurb,
//! each row's `❯ 1. GitHub Sponsors  monthly or one-time` label over its
//! value in the rounded box, and the key hint. The boxed value is an OSC 8
//! link — the sponsor page's URL, `mailto:` for the address — so a click
//! opens it where the terminal follows links, and Enter copies it
//! everywhere else.

use super::donate_view::{boxed_list_page, label_line, title_line, value_box};
use super::model_view::{model_placeholder_row, model_wrapped_rows};
use super::theme::*;
use super::*;

use crate::app::SPONSOR_LINKS;

/// One row's block: its label, then — when it is `open` — its boxed value.
fn link_block(index: usize, selected: usize, open: bool, width: u16) -> Vec<Line<'static>> {
    let link = &SPONSOR_LINKS[index];
    let lit = index == selected;
    let mut block = vec![label_line(index, link.label, link.note, lit, width)];
    if open {
        block.extend(value_box(link.value, Some(link.target), lit, width));
    }
    block
}

/// The framed page as lines — the `/donate` ladder (`boxed_list_page`): the
/// whole page; then the row the highlight is not on folded to its label;
/// then that without the title and blurb; then the highlighted row alone.
/// The hint stays on every page. What [`render_sponsor_page`] paints and
/// `sponsor_menu_rows` counts, so the reserved height and the painted rows
/// can never disagree. Empty when the page is closed.
#[must_use]
pub fn sponsor_view_lines(app: &App, width: u16, height: u16) -> Vec<Line<'static>> {
    let Some(page) = app.sponsor_page.as_ref() else {
        return Vec::new();
    };
    boxed_list_page(
        height,
        width,
        page.selected,
        SPONSOR_LINKS.len(),
        &|| {
            vec![
                vec![title_line(SPONSOR_TITLE_PREFIX, "", width)],
                model_wrapped_rows(SPONSOR_BLURB, model_meta_color(), width),
            ]
        },
        &|| {
            vec![vec![model_placeholder_row(
                SPONSOR_HINT,
                model_meta_color(),
                width,
            )]]
        },
        &|index, selected, open| link_block(index, selected, open, width),
    )
}

/// The rows the page's own frame occupies — the built page's line count.
pub(super) fn sponsor_menu_rows(app: &App, width: u16, height: u16) -> u16 {
    u16::try_from(sponsor_view_lines(app, width, height).len()).unwrap_or(u16::MAX)
}

/// The inline live-region height when the `/sponsor` page is open, or `None`
/// when it isn't. Like every sibling it replaces only the composer: the
/// streaming strip keeps its rows above it. Clamped to the terminal height.
#[must_use]
pub fn sponsor_page_height(app: &App, width: u16, term_height: u16) -> Option<u16> {
    app.sponsor_page.as_ref()?;
    Some(super::layout::view_height(
        app,
        width,
        sponsor_menu_rows(app, width, term_height),
        term_height,
    ))
}

/// Render the **inline** `/sponsor` page into the live region, in place of
/// the composer — bottom-anchored like every framed page
/// (`docs/view-flow.md`). Pure — `render_live` paints this.
pub fn render_sponsor_page(area: Rect, buf: &mut Buffer, app: &App) {
    super::view_flow::render_framed_tail(
        area,
        buf,
        sponsor_view_lines(app, area.width, area.height),
    );
}
