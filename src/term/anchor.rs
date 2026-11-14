//! An **anchored block**: committed rows the viewport may still repaint in
//! place — the startup banner, while the conversation under it is empty
//! (`docs/mascot-animation.md`).
//!
//! Everything [`InlineViewport::insert_before`] commits is written once and
//! left alone: rows above the live region scroll into the terminal's real
//! scrollback, and scrollback cannot be rewritten. The banner is the one
//! thing committed up there that wants to move — its mascot blinks while
//! nothing has been said yet — and it can, for exactly as long as its rows
//! are still **on screen**: a row above the live region is an ordinary
//! screen row until a scroll carries it off the top.
//!
//! So the viewport keeps this one note: which rows the block is, where they
//! sit relative to the live region's top (the one row it tracks), and what
//! they show. A frame repaints the cells that differ from what is shown —
//! two, for a blink — and three rules keep the note honest:
//!
//! - **Only the rest pose may leave the screen.** Before anything scrolls —
//!   a flush of queued commits, a live region growing past the bottom, the
//!   quit — the block is put back to the rows it was committed as. What
//!   reaches scrollback is always the banner standing still.
//! - **Off screen is over.** A block any part of which has scrolled off the
//!   top is dropped: from then on those rows are scrollback like any other.
//! - **A moved screen is a new screen.** A resize or a purge rebuild drops
//!   the note outright; the rebuild that follows writes the banner again and
//!   the boundary declares it again.
//!
//! This module is the note's pure half — the bookkeeping and the cell diff,
//! unit-tested here. The writes are [`InlineViewport`]'s, smoke-covered like
//! the rest of the boundary.
//!
//! [`InlineViewport`]: super::InlineViewport
//! [`InlineViewport::insert_before`]: super::InlineViewport::insert_before

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Widget};

/// The block the viewport may still repaint in place. See the module docs.
#[derive(Debug)]
pub(super) struct Anchor {
    /// The rows as they were committed — what the block is put back to
    /// before it can leave the screen.
    rest: Vec<Line<'static>>,
    /// What the block's rows on screen show now.
    shown: Vec<Line<'static>>,
    /// What the next frame should show, when the boundary has said.
    posed: Option<Vec<Line<'static>>>,
    /// Rows written between the block and the live region — what has been
    /// committed under it since (a startup notice card, say).
    below: usize,
    /// How many of the rows still queued for a flush were queued **before**
    /// the block was declared — its own among them, as the last. They are
    /// written first, at or above it; whatever a flush writes beyond them
    /// lands below. Zero once the block is on screen.
    queued: usize,
    /// The block is to be put at rest and let go: its rows are about to be
    /// history under a conversation.
    released: bool,
}

impl Anchor {
    /// Declare the last `rest.len()` rows committed above the live region —
    /// written already, or still among the `queued` rows awaiting a flush —
    /// as the block.
    pub(super) fn new(rest: Vec<Line<'static>>, queued: usize) -> Self {
        Self {
            shown: rest.clone(),
            rest,
            posed: None,
            below: 0,
            queued,
            released: false,
        }
    }

    /// A flush wrote `rows` queued rows above the live region: the ones
    /// queued before the block (and the block itself) first, the rest below
    /// it.
    pub(super) fn flushed(&mut self, rows: usize) {
        self.below += rows.saturating_sub(self.queued);
        self.queued = 0;
    }

    /// The screen row the block's first row sits on when the live region's
    /// top is `view_top` — `None` while its rows are still queued, and once
    /// any of them has scrolled off the top of the screen.
    pub(super) fn top(&self, view_top: u16) -> Option<u16> {
        if self.queued > 0 {
            return None;
        }
        let above = self.below.checked_add(self.rest.len())?;
        u16::try_from(usize::from(view_top).checked_sub(above)?).ok()
    }

    /// Whether the block's rows have been written at all — `false` while
    /// they wait in the queue (under an overlay, say), where "not on
    /// screen" means "not yet", not "gone".
    pub(super) const fn written(&self) -> bool {
        self.queued == 0
    }

    /// Ask the next frame to show `lines` — the block's rows in a new pose.
    /// Ignored when it is not the block's own shape: a pose never adds a row.
    pub(super) fn pose(&mut self, lines: Vec<Line<'static>>) {
        if lines.len() == self.rest.len() {
            self.posed = Some(lines);
        }
    }

    /// Put the block at rest on the next frame and let it go.
    pub(super) const fn release(&mut self) {
        self.released = true;
    }

    /// Whether the block has been [`release`](Self::release)d.
    pub(super) const fn released(&self) -> bool {
        self.released
    }

    /// The rows a frame should repaint the block with — the pose asked for,
    /// or the rest pose for a released block — or `None` when the screen
    /// already shows them, which is every frame the mascot stands still.
    pub(super) fn due(&self) -> Option<&[Line<'static>]> {
        let wanted = match &self.posed {
            Some(posed) if !self.released => posed,
            _ if self.released => &self.rest,
            _ => return None,
        };
        (*wanted != self.shown).then_some(wanted.as_slice())
    }

    /// The rest rows, when the block is off them — what must be painted
    /// before its rows may move.
    pub(super) fn unsettled(&self) -> Option<&[Line<'static>]> {
        (self.shown != self.rest).then_some(self.rest.as_slice())
    }

    /// What the block's rows on screen show now.
    pub(super) fn shown(&self) -> &[Line<'static>] {
        &self.shown
    }

    /// Record that the block's rows on screen now show `lines`.
    pub(super) fn painted(&mut self, lines: Vec<Line<'static>>) {
        self.shown = lines;
    }
}

/// The block's rows as cells: `lines` rendered `width` columns wide at
/// screen row `top`, the way a commit renders them ([`Paragraph`], one line a
/// row). Two of these — what is shown and what is due — are what
/// [`Buffer::diff`] turns into the cells a repaint writes, so a blink sends
/// the eyes' two cells and nothing beside them.
pub(super) fn block_buffer(lines: &[Line<'static>], width: u16, top: u16) -> Buffer {
    let area = Rect::new(
        0,
        top,
        width,
        u16::try_from(lines.len()).unwrap_or(u16::MAX),
    );
    let mut buffer = Buffer::empty(area);
    Paragraph::new(lines.to_vec()).render(area, &mut buffer);
    buffer
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use ratatui::text::Line;

    use super::*;
    use crate::app::{App, MASCOT_GREETING_MS, Mascot};
    use crate::ui;

    /// A four-row block: three rows of banner over its spacer.
    fn block() -> Vec<Line<'static>> {
        vec![
            "art 0".into(),
            "art 1".into(),
            "art 2".into(),
            Line::default(),
        ]
    }

    /// The block in another pose.
    fn posed() -> Vec<Line<'static>> {
        vec![
            "art 0".into(),
            "ART 1".into(),
            "art 2".into(),
            Line::default(),
        ]
    }

    #[test]
    fn a_block_declared_while_its_rows_are_queued_seats_once_they_flush() {
        // Startup: the banner and its spacer are queued, the block declared,
        // and the first frame's flush writes them directly above the region.
        let mut anchor = Anchor::new(block(), 4);
        assert!(!anchor.written());
        assert_eq!(anchor.top(10), None, "nothing is on screen yet");
        anchor.flushed(4);
        assert!(anchor.written());
        assert_eq!(anchor.top(10), Some(6));
    }

    #[test]
    fn rows_queued_ahead_of_the_block_are_not_under_it() {
        // A startup notice committed *above* the banner waits in the same
        // queue: it is written first and never comes between the block and
        // the region.
        let mut anchor = Anchor::new(block(), 5);
        anchor.flushed(5);
        assert_eq!(anchor.top(9), Some(5));
    }

    #[test]
    fn rows_committed_after_the_block_sit_between_it_and_the_region() {
        // The first-run telemetry card lands under the banner — in the very
        // flush that writes it, or in a later one.
        let mut together = Anchor::new(block(), 4);
        together.flushed(4 + 7);
        assert_eq!(together.top(20), Some(20 - 7 - 4));

        let mut later = Anchor::new(block(), 0);
        assert_eq!(
            later.top(20),
            Some(16),
            "declared over rows already written"
        );
        later.flushed(7);
        assert_eq!(later.top(20), Some(9));
        later.flushed(2);
        assert_eq!(later.top(20), Some(7));
    }

    #[test]
    fn a_block_with_a_row_off_the_top_has_no_seat() {
        let mut anchor = Anchor::new(block(), 0);
        assert_eq!(anchor.top(4), Some(0), "flush against the top row");
        assert_eq!(anchor.top(3), None, "its first row has scrolled away");
        anchor.flushed(30);
        assert_eq!(anchor.top(20), None, "pushed off by what came under it");
    }

    #[test]
    fn a_still_block_has_nothing_due() {
        // The mascot stands still nearly always: those frames must find
        // nothing to write.
        let mut anchor = Anchor::new(block(), 0);
        assert_eq!(anchor.due(), None);
        assert_eq!(anchor.unsettled(), None);
        anchor.pose(block());
        assert_eq!(anchor.due(), None, "posed as it already stands");
    }

    #[test]
    fn a_pose_is_due_until_it_is_painted() {
        let mut anchor = Anchor::new(block(), 0);
        anchor.pose(posed());
        assert_eq!(anchor.due(), Some(posed().as_slice()));
        assert_eq!(anchor.shown(), block().as_slice());
        anchor.painted(posed());
        assert_eq!(anchor.due(), None);
        assert_eq!(anchor.shown(), posed().as_slice());
        // …and back to rest is a pose like any other.
        anchor.pose(block());
        assert_eq!(anchor.due(), Some(block().as_slice()));
    }

    #[test]
    fn a_pose_of_another_shape_is_not_the_blocks() {
        let mut anchor = Anchor::new(block(), 0);
        anchor.pose(vec!["one row".into()]);
        assert_eq!(anchor.due(), None);
    }

    #[test]
    fn only_a_block_off_its_rest_rows_needs_settling() {
        let mut anchor = Anchor::new(block(), 0);
        anchor.pose(posed());
        assert_eq!(anchor.unsettled(), None, "asked for, not yet painted");
        anchor.painted(posed());
        assert_eq!(anchor.unsettled(), Some(block().as_slice()));
        anchor.painted(block());
        assert_eq!(anchor.unsettled(), None);
        // Settling does not forget the pose: the frame that scrolled paints
        // it again where the block now sits.
        assert_eq!(anchor.due(), Some(posed().as_slice()));
    }

    #[test]
    fn a_released_block_wants_its_rest_rows_whatever_was_posed() {
        let mut anchor = Anchor::new(block(), 0);
        anchor.pose(posed());
        anchor.painted(posed());
        assert!(!anchor.released());
        anchor.release();
        assert!(anchor.released());
        assert_eq!(anchor.due(), Some(block().as_slice()));
        anchor.pose(posed());
        assert_eq!(
            anchor.due(),
            Some(block().as_slice()),
            "a late pose changes nothing"
        );
        anchor.painted(block());
        assert_eq!(anchor.due(), None);
    }

    #[test]
    fn a_blink_rewrites_the_eyes_two_cells_and_nothing_else() {
        // The whole point of diffing: a terminal drops a mouse selection
        // when the cells under it are rewritten, and an idle screen should
        // put next to nothing on the wire.
        let mut app = App::new();
        app.set_session_info("model", "~/project");
        app.seed_mascot_motion(7);
        let shut = (0..MASCOT_GREETING_MS)
            .find(|&at| Mascot::Crest.pose(7, Duration::from_millis(at)).art[1] == "▝▜█████▛▘")
            .expect("the greeting blinks");
        app.set_mascot_clock(Duration::from_millis(shut));
        let rest = ui::banner_block_lines(&app, 80);
        let blink = ui::banner_frame_lines(&app, 80);
        let (was, now) = (block_buffer(&rest, 80, 5), block_buffer(&blink, 80, 5));
        let cells: Vec<(u16, u16, &str)> = was
            .diff(&now)
            .into_iter()
            .map(|(x, y, cell)| (x, y, cell.symbol()))
            .collect();
        assert_eq!(cells, [(2, 6, "█"), (6, 6, "█")]);
        // …and opening them again is the same two cells back.
        let back: Vec<(u16, u16, &str)> = now
            .diff(&was)
            .into_iter()
            .map(|(x, y, cell)| (x, y, cell.symbol()))
            .collect();
        assert_eq!(back, [(2, 6, "▄"), (6, 6, "▄")]);
    }

    #[test]
    fn a_move_of_the_whole_mascot_redraws_its_art_and_nothing_beside_it() {
        // A bounce or a lean redraws every row of the art — with rows longer
        // or shorter than the ones they replace — and still not one cell of
        // the title beside it; back at rest, every cell it touched is as it
        // was, the ones it had filled in blank again.
        let mut app = App::new();
        app.set_session_info("model", "~/project");
        app.seed_mascot_motion(7);
        let crest = Mascot::Crest;
        let whole = (MASCOT_GREETING_MS..600_000)
            .find(|&at| {
                let art = crest.pose(7, Duration::from_millis(at)).art;
                art.iter().zip(crest.art()).all(|(now, rest)| now != rest)
            })
            .expect("the crest moves as a whole within ten minutes");
        let rest = ui::banner_block_lines(&app, 80);
        app.set_mascot_clock(Duration::from_millis(whole));
        let moved = ui::banner_frame_lines(&app, 80);
        let (was, now) = (block_buffer(&rest, 80, 5), block_buffer(&moved, 80, 5));
        let there = was.diff(&now);
        assert!(there.len() > 6, "{} cells redrawn", there.len());
        let art_width = u16::try_from(crest.art_width()).expect("a narrow art");
        for (x, y, _) in &there {
            assert!(*x < art_width && (5..8).contains(y), "cell ({x}, {y})");
        }
        let back = now.diff(&was);
        assert_eq!(back.len(), there.len());
        for (x, y, cell) in back {
            assert_eq!(cell.symbol(), was[(x, y)].symbol(), "cell ({x}, {y})");
        }
    }

    #[test]
    fn the_block_is_rendered_where_it_was_committed() {
        let buffer = block_buffer(&block(), 12, 7);
        assert_eq!(buffer.area, Rect::new(0, 7, 12, 4));
        let row = |y: u16| -> String { (0..12).map(|x| buffer[(x, y)].symbol()).collect() };
        assert_eq!(row(7).trim_end(), "art 0");
        assert_eq!(row(9).trim_end(), "art 2");
        assert_eq!(row(10).trim_end(), "", "the spacer");
    }
}
