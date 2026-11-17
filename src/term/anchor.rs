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

use ratatui::buffer::{Buffer, Cell, CellWidth};
use ratatui::layout::{Alignment, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

/// The block the viewport may still repaint in place. See the module docs.
#[derive(Debug)]
pub(super) struct Anchor {
    /// The rows as they were committed — what the block is put back to
    /// before it can leave the screen.
    rest: Vec<Line<'static>>,
    /// What the block's rows on screen show now, while that is **not** the
    /// rows they were committed as — `None` at rest, which is nearly
    /// always, so a block standing still keeps one copy of its rows.
    shown: Option<Vec<Line<'static>>>,
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
            shown: None,
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
        (wanted.as_slice() != self.shown()).then_some(wanted.as_slice())
    }

    /// [`due`](Self::due), **handed over**: the rows a frame is to repaint
    /// the block with, taken out of the note rather than copied — they
    /// become what the block shows ([`painted`](Self::painted)). So a pose
    /// is asked for one frame at a time, which is how the loop asks: every
    /// frame says what the block should show before it paints. A released
    /// block hands over its rest rows.
    pub(super) fn take_due(&mut self) -> Option<Vec<Line<'static>>> {
        if self.released {
            return self.shown.is_some().then(|| self.rest.clone());
        }
        let posed = self.posed.take()?;
        (posed.as_slice() != self.shown()).then_some(posed)
    }

    /// Ask the next frame to show the block **as it was committed** — what
    /// a frame asks of a block that is not to move (the animation switched
    /// off). Nothing while it stands there already, which is every such
    /// frame but the one after the switch.
    pub(super) fn stand_still(&mut self) {
        self.posed = self.shown.is_some().then(|| self.rest.clone());
    }

    /// Whether a frame has cells of the block to write with the live
    /// region's top at `view_top`: a pose is [`due`](Self::due), and the
    /// block is wholly on screen to take it.
    pub(super) fn paints(&self, view_top: u16) -> bool {
        self.due().is_some() && self.top(view_top).is_some()
    }

    /// The rest rows, when the block is off them — what must be painted
    /// before its rows may move.
    pub(super) fn unsettled(&self) -> Option<&[Line<'static>]> {
        self.shown.is_some().then_some(self.rest.as_slice())
    }

    /// What the block's rows on screen show now.
    pub(super) fn shown(&self) -> &[Line<'static>] {
        self.shown.as_deref().unwrap_or(&self.rest)
    }

    /// Record that the block's rows on screen now show `lines`.
    pub(super) fn painted(&mut self, lines: Vec<Line<'static>>) {
        self.shown = (lines != self.rest).then_some(lines);
    }
}

/// The cells a repaint of the block writes: every cell at which the rows
/// `now` differ from the rows `was`, each as a commit renders it
/// ([`Paragraph`], one line a row) on a screen `width` columns wide with the
/// block's first row at `top` — so a blink sends the eyes' two cells and
/// nothing beside them.
///
/// They are found **a row at a time, only in the rows that changed, and
/// only as far along each as it can differ** ([`changed_rows`]). Rendering
/// the whole block twice at the terminal's width and diffing the two gives
/// the same cells — a test holds this to it — but those two buffers were
/// the largest thing a frame of the animation allocated, 0.4 kB a column,
/// and the allocator keeps what a process once used (`docs/memory.md`).
pub(super) fn changed_cells(
    was: &[Line<'static>],
    now: &[Line<'static>],
    width: u16,
    top: u16,
) -> Vec<(u16, u16, Cell)> {
    let most: usize = changed_rows(was, now, width)
        .map(|changed| usize::from(changed.cols))
        .sum();
    let mut cells = Vec::with_capacity(most);
    for changed in changed_rows(was, now, width).filter(|changed| changed.cols > 0) {
        let y = top.saturating_add(u16::try_from(changed.row).unwrap_or(u16::MAX));
        let before = row_buffer(changed.was, changed.was_spans, changed.cols, y);
        let after = row_buffer(changed.now, changed.now_spans, changed.cols, y);
        cells.extend(
            before
                .diff(&after)
                .into_iter()
                .map(|(x, y, cell)| (x, y, cell.clone())),
        );
    }
    cells
}

/// One row a repaint renders to find its cells: which row of the block,
/// how many of each version's leading spans, and how many columns wide.
struct ChangedRow<'a> {
    row: usize,
    was: &'a Line<'static>,
    now: &'a Line<'static>,
    /// The spans of each version ahead of the tail the two share — all of
    /// them when they share none, or none that starts in the same column.
    was_spans: usize,
    now_spans: usize,
    /// The columns those spans are rendered at: no more than they fill,
    /// and never past the screen's edge.
    cols: u16,
}

/// The rows at which `now` differs from `was`, each with as little of it as
/// must be rendered for its cells to be the ones a render `width` columns
/// wide gives.
///
/// A left-aligned row is laid out from its first column, span after span.
/// So where its two versions end on the same spans — the banner's title,
/// directory and hint, beside a mascot that moved — and those start in the
/// same column in both, nothing from there on can differ: the spans ahead
/// of them are the whole of the change, and their width is all that is
/// rendered. With no such tail the row is rendered as far as the longer of
/// its two versions reaches. Either way never past the screen's edge, which
/// cuts a row where it cuts the commit. A row that is not left-aligned is
/// placed by the width it is rendered at, and gets all of it.
fn changed_rows<'a>(
    was: &'a [Line<'static>],
    now: &'a [Line<'static>],
    width: u16,
) -> impl Iterator<Item = ChangedRow<'a>> + 'a {
    was.iter()
        .zip(now)
        .enumerate()
        .filter(|(_, (was, now))| was != now)
        .map(move |(row, (was, now))| changed_row(row, was, now, width))
}

/// How row `row`, which reads `was` on screen and is to read `now`, is
/// rendered to find its cells ([`changed_rows`]).
fn changed_row<'a>(
    row: usize,
    was: &'a Line<'static>,
    now: &'a Line<'static>,
    width: u16,
) -> ChangedRow<'a> {
    let whole = |cols: u16| ChangedRow {
        row,
        was,
        now,
        was_spans: was.spans.len(),
        now_spans: now.spans.len(),
        cols,
    };
    let left = |line: &Line<'_>| matches!(line.alignment, None | Some(Alignment::Left));
    if !(left(was) && left(now)) {
        return whole(width);
    }
    if was.style == now.style {
        let tail = was
            .spans
            .iter()
            .rev()
            .zip(now.spans.iter().rev())
            .take_while(|(was, now)| was == now)
            .count();
        let (was_spans, now_spans) = (was.spans.len() - tail, now.spans.len() - tail);
        let head = span_cols(&was.spans[..was_spans]);
        if tail > 0 && head == span_cols(&now.spans[..now_spans]) {
            return ChangedRow {
                row,
                was,
                now,
                was_spans,
                now_spans,
                cols: width.min(head),
            };
        }
    }
    whole(width.min(span_cols(&was.spans).max(span_cols(&now.spans))))
}

/// How many columns `spans` fill when nothing cuts them: their graphemes'
/// widths by the measure [`Paragraph`] lays them out with.
fn span_cols(spans: &[Span<'_>]) -> u16 {
    spans
        .iter()
        .flat_map(|span| span.styled_graphemes(Style::default()))
        .fold(0_u16, |cols, grapheme| {
            cols.saturating_add(grapheme.symbol.cell_width())
        })
}

/// One row as cells: the first `spans` spans of `line` rendered `cols`
/// columns wide at screen row `y`, the way a commit renders the line.
fn row_buffer(line: &Line<'static>, spans: usize, cols: u16, y: u16) -> Buffer {
    let area = Rect::new(0, y, cols, 1);
    let mut buffer = Buffer::empty(area);
    let head: Vec<Span<'_>> = line.spans[..spans]
        .iter()
        .map(|span| Span::styled(span.content.as_ref(), span.style))
        .collect();
    let mut head = Line::from(head).style(line.style);
    head.alignment = line.alignment;
    Paragraph::new(head).render(area, &mut buffer);
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
    fn a_block_paints_only_a_pose_the_screen_does_not_show_where_it_can() {
        // A frame of the banner's own writes nothing at all — not a cursor
        // move — unless this says there are cells to write.
        let mut anchor = Anchor::new(block(), 0);
        assert!(!anchor.paints(10), "standing still");
        anchor.pose(block());
        assert!(!anchor.paints(10), "posed as it stands");
        anchor.pose(posed());
        assert!(anchor.paints(10));
        assert!(!anchor.paints(3), "a row of it has scrolled away");
        anchor.painted(posed());
        assert!(!anchor.paints(10), "painted");
        anchor.release();
        assert!(anchor.paints(10), "let go off its rest rows: back to them");
        anchor.painted(block());
        assert!(!anchor.paints(10));
        // Rows still waiting to be written have nowhere to be painted.
        let mut queued = Anchor::new(block(), 4);
        queued.pose(posed());
        assert!(!queued.paints(10));
    }

    #[test]
    fn a_pose_is_handed_to_the_frame_that_paints_it_once() {
        // A frame takes the rows it is to paint rather than copying them —
        // they become what the block shows — so a pose is asked for one
        // frame at a time, which is how the loop asks.
        let mut anchor = Anchor::new(block(), 0);
        assert_eq!(anchor.take_due(), None, "standing still");
        anchor.pose(block());
        assert_eq!(anchor.take_due(), None, "posed as it stands");
        anchor.pose(posed());
        assert_eq!(anchor.take_due(), Some(posed()));
        assert_eq!(anchor.take_due(), None, "taken: the note keeps no copy");
        assert_eq!(anchor.due(), None);
        anchor.painted(posed());
        assert_eq!(anchor.take_due(), None, "and painted");
        // A block let go wants its rest rows, whatever is asked of it —
        // until it shows them.
        anchor.pose(posed());
        anchor.release();
        assert_eq!(anchor.take_due(), Some(block()));
        anchor.painted(block());
        assert_eq!(anchor.take_due(), None);
    }

    #[test]
    fn a_block_asked_to_stand_still_is_due_its_rest_rows_only_off_them() {
        // The animation switched off (or the mascot not animating at all):
        // every frame asks the block to stand still, which costs nothing
        // while it does — and puts it back if the switch caught it
        // mid-move.
        let mut anchor = Anchor::new(block(), 0);
        anchor.stand_still();
        assert_eq!(anchor.due(), None, "it stands still already");
        anchor.pose(posed());
        anchor.stand_still();
        assert_eq!(anchor.due(), None, "a pose not yet painted is withdrawn");
        anchor.pose(posed());
        let lines = anchor.take_due().expect("a pose");
        anchor.painted(lines);
        anchor.stand_still();
        assert_eq!(anchor.take_due(), Some(block()));
        anchor.painted(block());
        anchor.stand_still();
        assert_eq!(anchor.take_due(), None);
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

    /// The cells the whole block rendered at the terminal's width says
    /// changed — the diff [`changed_cells`] must reproduce without building
    /// it.
    fn whole_block_diff(
        was: &[Line<'static>],
        now: &[Line<'static>],
        width: u16,
        top: u16,
    ) -> Vec<(u16, u16, Cell)> {
        let render = |lines: &[Line<'static>]| {
            let rows = u16::try_from(lines.len()).expect("a short block");
            let area = Rect::new(0, top, width, rows);
            let mut buffer = Buffer::empty(area);
            Paragraph::new(lines.to_vec()).render(area, &mut buffer);
            buffer
        };
        let (before, after) = (render(was), render(now));
        before
            .diff(&after)
            .into_iter()
            .map(|(x, y, cell)| (x, y, cell.clone()))
            .collect()
    }

    /// `(x, y, symbol)` of each cell, for a readable assertion.
    fn symbols(cells: &[(u16, u16, Cell)]) -> Vec<(u16, u16, &str)> {
        cells
            .iter()
            .map(|(x, y, cell)| (*x, *y, cell.symbol()))
            .collect()
    }

    /// An app showing `mascot`'s banner, its animation seeded.
    fn banner_app(mascot: Mascot) -> App {
        let mut app = App::new();
        app.set_session_info("model", "~/project");
        app.seed_mascot_motion(7);
        app.set_mascot(mascot);
        app
    }

    #[test]
    fn a_blink_rewrites_the_eyes_two_cells_and_nothing_else() {
        // The whole point of diffing: a terminal drops a mouse selection
        // when the cells under it are rewritten, and an idle screen should
        // put next to nothing on the wire.
        let mut app = banner_app(Mascot::Crest);
        let shut = (0..MASCOT_GREETING_MS)
            .find(|&at| Mascot::Crest.pose(7, Duration::from_millis(at)).art[1] == "▝▜█████▛▘")
            .expect("the greeting blinks");
        app.set_mascot_clock(Duration::from_millis(shut));
        let rest = ui::banner_block_lines(&app, 80);
        let blink = ui::banner_frame_lines(&app, 80);
        let shut = changed_cells(&rest, &blink, 80, 5);
        assert_eq!(symbols(&shut), [(2, 6, "█"), (6, 6, "█")]);
        // …and opening them again is the same two cells back.
        let open = changed_cells(&blink, &rest, 80, 5);
        assert_eq!(symbols(&open), [(2, 6, "▄"), (6, 6, "▄")]);
    }

    #[test]
    fn a_move_of_the_whole_mascot_redraws_its_art_and_nothing_beside_it() {
        // A bounce or a lean redraws every row of the art — with rows longer
        // or shorter than the ones they replace — and still not one cell of
        // the title beside it; back at rest, every cell it touched is as it
        // was, the ones it had filled in blank again.
        let mut app = banner_app(Mascot::Crest);
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
        let there = changed_cells(&rest, &moved, 80, 5);
        assert!(there.len() > 6, "{} cells redrawn", there.len());
        let art_width = u16::try_from(crest.art_width()).expect("a narrow art");
        for (x, y, _) in &there {
            assert!(*x < art_width && (5..8).contains(y), "cell ({x}, {y})");
        }
        let back = changed_cells(&moved, &rest, 80, 5);
        assert_eq!(back.len(), there.len());
        let at_rest = whole_block_diff(&moved, &rest, 80, 5);
        assert_eq!(back, at_rest, "every cell it touched is as it was");
    }

    #[test]
    fn the_cells_land_on_the_rows_the_block_sits_on() {
        let cells = changed_cells(&block(), &posed(), 12, 7);
        assert_eq!(symbols(&cells), [(0, 8, "A"), (1, 8, "R"), (2, 8, "T")]);
    }

    #[test]
    fn a_repaint_writes_what_the_whole_block_rendered_would() {
        // The cells are found row by row, each rendered no wider than it
        // is — and they must be, cell for cell, what rendering the whole
        // block at the terminal's width and diffing it says: every pose of
        // every mascot against the one before it and against rest, on
        // terminals narrower than the art, narrower than the banner and
        // far wider than it.
        for mascot in Mascot::ALL {
            let mut app = banner_app(mascot);
            for width in [7_u16, 12, 31, 80, 240] {
                let rest = ui::banner_block_lines(&app, width);
                let mut poses: Vec<Vec<Line<'static>>> = vec![rest.clone()];
                // The greeting, a round of moves, and a nap begun and woken.
                for at in (0..200_000_u64).step_by(40) {
                    app.set_mascot_clock(Duration::from_millis(at));
                    let lines = ui::banner_frame_lines(&app, width);
                    if poses.last() != Some(&lines) {
                        poses.push(lines);
                    }
                }
                // (A terminal too narrow for the art shows a banner that
                // never moves.)
                let moves = ui::banner_shows_art(&app, width);
                assert!(
                    !moves || poses.len() > 40,
                    "{}: {} poses",
                    mascot.name(),
                    poses.len()
                );
                for pair in poses.windows(2) {
                    for (was, now) in [(&pair[0], &pair[1]), (&rest, &pair[1]), (&pair[1], &rest)] {
                        assert_eq!(
                            changed_cells(was, now, width, 3),
                            whole_block_diff(was, now, width, 3),
                            "{} at {width} columns",
                            mascot.name()
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn rows_of_any_text_are_diffed_as_the_whole_block_would_be() {
        // The block is the banner today, but the diff is the viewport's:
        // wide glyphs (a cell and its shadow), rows cut by a narrow
        // terminal, rows that only change colour, rows that are not
        // left-aligned (whose cells sit where the terminal's width puts
        // them), an empty row gaining text and a row losing all of it.
        use ratatui::style::Color;
        let cases: Vec<(Vec<Line<'static>>, Vec<Line<'static>>)> = vec![
            (vec!["日本語 abc".into()], vec!["日本 語abc".into()]),
            (vec!["ab日本".into()], vec!["a日本b".into()]),
            (
                vec!["plain text here".into()],
                vec!["plain TEXT here".into()],
            ),
            (
                vec![Line::styled("same words", Style::new().fg(Color::Red))],
                vec![Line::styled("same words", Style::new().fg(Color::Blue))],
            ),
            (
                vec![Line::from("middle").centered()],
                vec![Line::from("MIDDLE!").centered()],
            ),
            (
                vec![Line::from("end").right_aligned()],
                vec![Line::from("the end").right_aligned()],
            ),
            (vec![Line::default()], vec!["now there is text".into()]),
            (vec!["text that goes away".into()], vec![Line::default()]),
            (
                vec!["one".into(), "two".into(), "three".into()],
                vec!["one".into(), "TWO and more".into(), "thr".into()],
            ),
            (vec!["e\u{301}cole".into()], vec!["ecole".into()]),
            // A head that changes ahead of a tail that does not: the same
            // width, a wide glyph for two narrow ones, a different width,
            // a tail that is the whole row, and a row whose own style
            // changes under spans that do not.
            (
                vec![Line::from(vec!["ab".into(), " tail of the row".into()])],
                vec![Line::from(vec!["AB".into(), " tail of the row".into()])],
            ),
            (
                vec![Line::from(vec!["ab".into(), "日本 tail".into()])],
                vec![Line::from(vec!["本".into(), "日本 tail".into()])],
            ),
            (
                vec![Line::from(vec!["ab".into(), " tail of the row".into()])],
                vec![Line::from(vec!["abc".into(), " tail of the row".into()])],
            ),
            (
                vec![Line::from(vec!["".into(), "only a tail".into()])],
                vec![Line::from(vec!["only a tail".into()])],
            ),
            (
                vec![Line::from(vec!["ab".into(), " tail".into()]).style(Color::Red)],
                vec![Line::from(vec!["AB".into(), " tail".into()]).style(Color::Blue)],
            ),
            (
                vec![Line::from(vec![
                    "x".into(),
                    Span::styled("mid", Style::new().fg(Color::Green)),
                    " tail".into(),
                ])],
                vec![Line::from(vec![
                    "y".into(),
                    Span::styled("mid", Style::new().fg(Color::Yellow)),
                    " tail".into(),
                ])],
            ),
        ];
        for (was, now) in &cases {
            for width in [1_u16, 2, 3, 5, 8, 13, 40, 200] {
                assert_eq!(
                    changed_cells(was, now, width, 9),
                    whole_block_diff(was, now, width, 9),
                    "{was:?} -> {now:?} at {width} columns"
                );
            }
        }
    }

    /// `(row, columns rendered)` of each row a repaint renders.
    fn rendered(was: &[Line<'static>], now: &[Line<'static>], width: u16) -> Vec<(usize, u16)> {
        changed_rows(was, now, width)
            .map(|changed| (changed.row, changed.cols))
            .collect()
    }

    #[test]
    fn a_repaint_renders_only_the_rows_that_changed_and_only_where_they_can_differ() {
        // What a repaint builds to find its cells is what it leaves behind
        // in the heap's high-water mark (`docs/memory.md`): a blink renders
        // the one row the eyes are on, as far as the mascot's art and the
        // gap beside it — not the directory after them, however long, and
        // never the block at the terminal's width.
        let mut app = App::new();
        app.set_session_info(
            "model",
            "~/a/working/directory/with/a/long/path/that/goes/on/and/on",
        );
        app.seed_mascot_motion(7);
        let shut = (0..MASCOT_GREETING_MS)
            .find(|&at| Mascot::Crest.pose(7, Duration::from_millis(at)).art[1] == "▝▜█████▛▘")
            .expect("the greeting blinks");
        app.set_mascot_clock(Duration::from_millis(shut));
        let rest = ui::banner_block_lines(&app, 300);
        let blink = ui::banner_frame_lines(&app, 300);
        let rows = rendered(&rest, &blink, 300);
        let art = u16::try_from(Mascot::Crest.art_width()).expect("a narrow art");
        assert_eq!(rows.len(), 1, "the eyes' row alone: {rows:?}");
        assert_eq!(rows[0].0, 1);
        assert!(rows[0].1 <= art + 4, "the art and its gap: {rows:?}");
        assert!(rest[1].width() > 60, "beside a long directory");
        // A terminal narrower than the art cuts the row where it cuts the
        // banner.
        let (rest, blink) = (
            ui::banner_block_lines(&app, 5),
            ui::banner_frame_lines(&app, 5),
        );
        assert!(
            rendered(&rest, &blink, 5)
                .iter()
                .all(|&(_, cols)| cols <= 5)
        );
        // Text that changes ahead of text that does not is rendered as far
        // as the change — while the rest still starts where it did.
        let (was, now) = (
            vec![Line::from(vec!["ab".into(), " and the rest".into()])],
            vec![Line::from(vec!["AB".into(), " and the rest".into()])],
        );
        assert_eq!(rendered(&was, &now, 300), [(0, 2)]);
        // Once it starts somewhere else, every cell of it has moved.
        let now = vec![Line::from(vec!["ABC".into(), " and the rest".into()])];
        assert_eq!(rendered(&was, &now, 300), [(0, 16)]);
        // Rows that are not left-aligned are placed by the terminal's
        // width, so they are rendered at it.
        let (was, now) = (
            vec![Line::from("a").centered()],
            vec![Line::from("b").centered()],
        );
        assert_eq!(rendered(&was, &now, 300), [(0, 300)]);
        // And a block that stands still renders nothing at all.
        assert_eq!(rendered(&rest, &rest, 12), []);
    }
}
