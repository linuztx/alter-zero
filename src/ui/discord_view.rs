//! The read-only `/discord` page. See `docs/discord.md`.
//!
//! The `/sponsor` page's frame and parts ([`super::donate_view`]) over the
//! one const invite: the heart and the gradient `Join the Alter Zero
//! Discord` title, a dim blurb, the `❯ 1. Discord  invite to the community
//! server` label over the invite in the rounded box, and the key hint. The
//! boxed invite is an OSC 8 link to itself, so a click opens it where the
//! terminal follows links, and Enter copies it everywhere else.

use super::donate_view::{boxed_list_page, label_line, title_line, value_box};
use super::model_view::{model_placeholder_row, model_wrapped_rows};
use super::theme::*;
use super::*;

use crate::app::DISCORD_INVITE;

/// The page's one row: its label, then the invite in its box — lit, the row
/// being the highlighted one, and linked to itself.
fn invite_block(width: u16) -> Vec<Line<'static>> {
    let mut block = vec![label_line(0, DISCORD_LABEL, DISCORD_NOTE, true, width)];
    block.extend(value_box(DISCORD_INVITE, Some(DISCORD_INVITE), true, width));
    block
}

/// The framed page as lines — the `/donate` ladder (`boxed_list_page`) over
/// one entry, so two of its pages differ: the whole page, then the row
/// without the title and blurb. The hint stays on both. What
/// [`render_discord_page`] paints and `discord_menu_rows` counts, so the
/// reserved height and the painted rows can never disagree. Empty when the
/// page is closed.
#[must_use]
pub fn discord_view_lines(app: &App, width: u16, height: u16) -> Vec<Line<'static>> {
    if app.discord_page.is_none() {
        return Vec::new();
    }
    boxed_list_page(
        height,
        width,
        0,
        1,
        &|| {
            vec![
                vec![title_line(
                    DISCORD_TITLE_PREFIX,
                    DISCORD_TITLE_SUFFIX,
                    width,
                )],
                model_wrapped_rows(DISCORD_BLURB, model_meta_color(), width),
            ]
        },
        &|| {
            vec![vec![model_placeholder_row(
                DISCORD_HINT,
                model_meta_color(),
                width,
            )]]
        },
        // One entry, so every rung of the ladder opens it.
        &|_, _, _| invite_block(width),
    )
}

/// The rows the page's own frame occupies — the built page's line count.
pub(super) fn discord_menu_rows(app: &App, width: u16, height: u16) -> u16 {
    u16::try_from(discord_view_lines(app, width, height).len()).unwrap_or(u16::MAX)
}

/// The inline live-region height when the `/discord` page is open, or `None`
/// when it isn't. Like every sibling it replaces only the composer: the
/// streaming strip keeps its rows above it. Clamped to the terminal height.
#[must_use]
pub fn discord_page_height(app: &App, width: u16, term_height: u16) -> Option<u16> {
    app.discord_page.as_ref()?;
    Some(super::layout::view_height(
        app,
        width,
        discord_menu_rows(app, width, term_height),
        term_height,
    ))
}

/// Render the **inline** `/discord` page into the live region, in place of
/// the composer — bottom-anchored like every framed page
/// (`docs/view-flow.md`). Pure — `render_live` paints this.
pub fn render_discord_page(area: Rect, buf: &mut Buffer, app: &App) {
    super::view_flow::render_framed_tail(
        area,
        buf,
        discord_view_lines(app, area.width, area.height),
    );
}
