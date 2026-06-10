//! A custom inline viewport — the I/O boundary that makes the live region able
//! to **grow**.
//!
//! ratatui's own `Viewport::Inline(h)` fixes `h` at startup (the field is private
//! and `Terminal::resize` reuses it), so its inline viewport can never change
//! height. To get a dynamic, content-anchored live region we own a tiny viewport
//! over a [`CrosstermBackend`], reusing the backend's cell→ANSI `draw`,
//! `append_lines` (scroll the screen up, oldest rows into real scrollback),
//! `clear`, and cursor ops — but tracking the viewport rectangle ourselves so we
//! can re-pin it to a new height every frame.
//!
//! Like `main.rs`, this is an I/O boundary verified by `scripts/smoke.sh`, not
//! unit tests; every geometry decision it makes is a pure, tested `ui` helper
//! ([`ui::live_height`], [`ui::repin`]).
//!
//! Two operations:
//! - [`InlineViewport::insert_before`] commits finished lines into scrollback
//!   above the viewport — a direct port of ratatui's portable
//!   (no-`scrolling-regions`) insert-before scroll math, including its tmux-safe
//!   "draw then clear" ordering.
//! - [`InlineViewport::draw`] re-pins the viewport to its new `height` keeping its
//!   **top anchored** (it grows downward in place, scrolling the screen up only
//!   when it would overflow the bottom, and blanking the rows a shrink vacates),
//!   repaints the live region, and places the hardware cursor.

use std::io::{self, Stdout, Write};

use ratatui::backend::{Backend, ClearType, CrosstermBackend};
use ratatui::buffer::{Buffer, Cell};
use ratatui::crossterm::cursor::Show;
use ratatui::crossterm::terminal::{
    BeginSynchronizedUpdate, EndSynchronizedUpdate, EnterAlternateScreen, LeaveAlternateScreen,
    disable_raw_mode, enable_raw_mode,
};
use ratatui::crossterm::{execute, queue};
use ratatui::layout::{Position, Rect};
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Widget};

use crate::app::App;
use crate::ui;

/// A content-anchored inline viewport whose height can change between draws (its
/// top stays put; it grows downward in place).
pub struct InlineViewport {
    backend: CrosstermBackend<Stdout>,
    /// The full terminal area, `(0, 0, width, height)`.
    screen: Rect,
    /// The live region: full width, `height` rows, anchored by its top row.
    view: Rect,
    /// The live-region buffer last painted, kept so a follow-up [`draw`] at the
    /// same geometry can send only the cells that changed (a one-char edit ships a
    /// couple of cells, not the whole region). Invalidated (`None`) by anything
    /// that moves what's on screen out from under it — `insert_before`, `reflow`,
    /// the overlay, a resize — after which the next draw repaints in full.
    ///
    /// [`draw`]: InlineViewport::draw
    prev: Option<Buffer>,
}

impl InlineViewport {
    /// Enter raw mode and reserve `min_height` rows at the bottom for the live
    /// region, anchored to the current cursor row (mirrors how ratatui seats an
    /// inline viewport). Queries the cursor over stdin — safe here because this
    /// runs on the main thread before any reply thread is spawned.
    pub fn init(min_height: u16) -> io::Result<Self> {
        install_panic_hook();
        enable_raw_mode()?;
        let mut backend = CrosstermBackend::new(io::stdout());
        let size = backend.size()?;
        let screen = Rect::new(0, 0, size.width, size.height);

        let height = min_height.clamp(1, size.height.max(1));
        let cursor = backend.get_cursor_position()?;
        // Make room below the cursor for the viewport, scrolling if we're near
        // the bottom, and anchor the viewport top to where the cursor ended up.
        let below = height.saturating_sub(1);
        backend.append_lines(below)?;
        let available = size.height.saturating_sub(cursor.y).saturating_sub(1);
        let scrolled = below.saturating_sub(available);
        let top = cursor.y.saturating_sub(scrolled);

        let view = Rect::new(0, top, size.width, height);
        backend.hide_cursor()?;
        Ok(Self {
            backend,
            screen,
            view,
            prev: None,
        })
    }

    /// The live region rectangle to render into (full width, current height).
    #[must_use]
    pub const fn screen(&self) -> Rect {
        self.screen
    }

    /// Repaint the live region at the new `height`, keeping it **content-anchored**
    /// (its top fixed — it grows downward, not up from the bottom), and place the
    /// hardware cursor. `cursor` is `Some(app)` while editing (the cursor sits at
    /// the end of the input, derived via [`ui::cursor_position`] which accounts for
    /// the command palette's band); `None` while streaming hides the cursor.
    ///
    /// The box grows in place until it reaches the screen bottom, at which point
    /// it scrolls the chat up into scrollback; a shrink blanks the rows it vacates.
    pub fn draw(
        &mut self,
        height: u16,
        render: impl FnOnce(Rect, &mut Buffer),
        cursor: Option<&App>,
    ) -> io::Result<()> {
        let height = height.clamp(1, self.screen.height.max(1));
        let repin = ui::repin(self.view.y, self.view.height, height, self.screen.height);
        self.view = Rect::new(0, repin.top, self.screen.width, height);

        let mut buf = Buffer::empty(self.view);
        render(self.view, &mut buf);

        // Emit the whole frame inside a synchronized update (BSU/ESU, DEC mode 2026):
        // the terminal buffers everything between the markers and swaps it in one
        // atomic step, so a fast burst of keystrokes never shows a half-painted frame
        // or the cursor mid-flight. This is what makes typing look "in sync" — the
        // same trick codex wraps its draws in. Terminals lacking 2026 ignore the
        // markers. `EndSynchronizedUpdate` always runs (even if a write failed
        // mid-frame) so the terminal is never left buffering.
        queue!(self.backend, BeginSynchronizedUpdate)?;
        let painted = self.paint_frame(&buf, &repin, height, cursor);
        let ended = queue!(self.backend, EndSynchronizedUpdate);
        painted?;
        ended?;
        self.prev = Some(buf);
        Backend::flush(&mut self.backend)
    }

    /// Paint one already-rendered frame `buf` to the backend: prepare the screen
    /// (`scroll_up` / vacated-row clear from `repin`), emit the cells, and place the
    /// cursor. Split out so [`draw`] can bracket exactly this work in a synchronized
    /// update. When the region sat still (no scroll, no vacated rows, same rect as
    /// last frame) only the cells that **changed** since `prev` are sent — a keystroke
    /// ships a couple of cells, not the whole region; otherwise it repaints in full.
    ///
    /// [`draw`]: InlineViewport::draw
    fn paint_frame(
        &mut self,
        buf: &Buffer,
        repin: &ui::Repin,
        height: u16,
        cursor: Option<&App>,
    ) -> io::Result<()> {
        self.scroll_up(repin.scroll_up)?;
        if repin.clear_below > 0 {
            self.clear_rows(repin.top.saturating_add(height), repin.clear_below)?;
        }
        match &self.prev {
            Some(prev)
                if repin.scroll_up == 0 && repin.clear_below == 0 && prev.area == buf.area =>
            {
                let updates = prev.diff(buf);
                if !updates.is_empty() {
                    self.backend.draw(updates.into_iter())?;
                }
            }
            _ => self.blit(buf)?,
        }
        match cursor {
            Some(app) => {
                let (x, y) = ui::cursor_position(self.view, app);
                self.backend.set_cursor_position(Position::new(x, y))?;
                self.backend.show_cursor()?;
            }
            None => self.backend.hide_cursor()?,
        }
        Ok(())
    }

    /// Commit `lines` into scrollback directly above the viewport, pushing the
    /// viewport down to sit just below them. Port of ratatui's portable
    /// `insert_before`: draw the lines into the rows above the viewport, scrolling
    /// the screen up only as much as needed, then leave the viewport cleared for
    /// the next [`draw`].
    ///
    /// [`draw`]: InlineViewport::draw
    pub fn insert_before(&mut self, lines: Vec<Line<'static>>) -> io::Result<()> {
        let height = lines.len() as u16;
        if height == 0 {
            return Ok(());
        }
        let area = Rect::new(0, 0, self.screen.width, height);
        let mut buffer = Buffer::empty(area);
        Paragraph::new(lines).render(area, &mut buffer);
        let mut cells = buffer.content.as_slice();

        // i32 so the running sums never underflow/overflow a u16.
        let mut drawn: i32 = self.view.top().into();
        let mut remaining: i32 = height.into();
        let viewport: i32 = self.view.height.into();
        let screen: i32 = self.screen.height.into();

        // Draw screen-fulls, scrolling up minimally, until the rest of the buffer
        // plus the viewport fits — so the final batch is a single draw.
        while remaining + viewport > screen {
            let to_draw = remaining.min(screen);
            let scroll = 0.max(drawn + to_draw - screen);
            self.scroll_up(scroll as u16)?;
            cells = self.draw_lines((drawn - scroll) as u16, to_draw as u16, cells)?;
            drawn += to_draw - scroll;
            remaining -= to_draw;
        }
        let scroll = 0.max(drawn + remaining + viewport - screen);
        self.scroll_up(scroll as u16)?;
        self.draw_lines((drawn - scroll) as u16, remaining as u16, cells)?;
        drawn += remaining - scroll;

        self.view.y = drawn as u16;
        // Clear the viewport now-stale on screen; the next `draw` repaints it. We
        // clear *after* drawing (not before scrolling) to dodge a tmux bug where a
        // full clear immediately followed by a scroll spills garbage to scrollback.
        self.backend
            .set_cursor_position(Position::new(0, self.view.y))?;
        self.backend.clear_region(ClearType::AfterCursor)?;
        self.prev = None; // the viewport area on screen is now cleared, not last-drawn
        Ok(())
    }

    /// Sync the tracked live-region `height` *without* redrawing, just before the
    /// final [`insert_before`]s when a reply ends.
    ///
    /// The streaming strip (preview + gap) is drawn *above* the box, so it inflates
    /// the viewport height while a reply streams. When the reply finishes that strip
    /// clears, but [`insert_before`] reserves `self.view.height` rows *below* the
    /// committed lines to keep the viewport on screen — so if it still counted the
    /// strip it would over-scroll, and the box would rise as the strip cleared,
    /// leaving blank rows beneath it. Reseating the height to the idle box first lets
    /// the committed final line + spacer replace the strip's rows in place and the
    /// box stay put.
    ///
    /// [`insert_before`]: InlineViewport::insert_before
    pub fn set_view_height(&mut self, height: u16) {
        self.view.height = height.clamp(1, self.screen.height.max(1));
    }

    /// Note a new terminal size. Returns whether the *width* changed (the only
    /// change that forces the conversation to re-wrap; see [`reflow`]).
    ///
    /// [`reflow`]: InlineViewport::reflow
    pub fn resized(&mut self, width: u16, height: u16) -> bool {
        let changed = width != self.screen.width;
        self.screen = Rect::new(0, 0, width, height);
        self.prev = None; // geometry moved under us; repaint in full next draw
        changed
    }

    /// Repaint the conversation `tail` re-wrapped to the new width after a resize
    /// (or a Ctrl+O return / `/clear`): seat the viewport at the top, then
    /// `insert_before` the tail so it fills the screen and pushes the viewport down
    /// below it — the next [`draw`] paints the live region. `insert_before`
    /// overwrites the screen in place (drawing top-down, then clearing the rows
    /// below the tail), so a full `clear_region(All)` is needed only for an *empty*
    /// tail (see the body for the tmux-spill reason). Mirrors the old
    /// `repaint_after_resize`.
    ///
    /// [`draw`]: InlineViewport::draw
    pub fn reflow(&mut self, tail: Vec<Line<'static>>, height: u16) -> io::Result<()> {
        let height = height.clamp(1, self.screen.height.max(1));
        // An empty tail (e.g. `/clear`) never reaches `insert_before`'s draw, so
        // blank the screen outright for it. For a NON-empty tail we must *not*
        // `clear_region(All)` first: `insert_before` already overwrites the screen
        // top-down and clears the rows below the tail (a spill-safe draw-then-clear).
        // A leading full clear, when the screen still holds the frame restored by
        // *leaving the Ctrl+O alt-screen*, makes tmux push that stale frame into
        // scrollback — the leak that left the old `Working… (… tokens)` status strip
        // sitting above the rebuilt conversation. Overwriting in place avoids the
        // clear-then-scroll sequence entirely.
        if tail.is_empty() {
            self.backend.clear_region(ClearType::All)?;
        }
        self.backend.set_cursor_position(Position::new(0, 0))?;
        self.prev = None; // screen is being rebuilt; repaint in full next draw
        self.view = Rect::new(0, 0, self.screen.width, height);
        self.insert_before(tail)
    }

    /// Switch to the alternate screen for the full-screen tool-output overlay,
    /// preserving the inline conversation (the main screen + its scrollback) so
    /// [`exit_overlay`] restores it untouched. The caller paints with
    /// [`draw_overlay`] and must call [`exit_overlay`] to come back.
    ///
    /// [`exit_overlay`]: InlineViewport::exit_overlay
    /// [`draw_overlay`]: InlineViewport::draw_overlay
    pub fn enter_overlay(&mut self) -> io::Result<()> {
        execute!(self.backend, EnterAlternateScreen)?;
        self.backend.hide_cursor()?;
        self.backend.clear_region(ClearType::All)?;
        self.prev = None; // the inline view isn't on screen now
        Backend::flush(&mut self.backend)
    }

    /// Leave the alternate screen, restoring the inline conversation exactly as it
    /// was. The caller then repaints it (via [`reflow`]) to catch up on anything
    /// that streamed while the overlay was showing.
    ///
    /// [`reflow`]: InlineViewport::reflow
    pub fn exit_overlay(&mut self) -> io::Result<()> {
        execute!(self.backend, LeaveAlternateScreen)?;
        self.prev = None; // returning to a screen the inline view will repaint
        Backend::flush(&mut self.backend)
    }

    /// Paint a full-screen `render` onto the alternate screen (used for the
    /// tool-output overlay). Draws every cell of the screen, so empty rows are
    /// blanked — no separate clear needed between frames.
    pub fn draw_overlay(&mut self, render: impl FnOnce(Rect, &mut Buffer)) -> io::Result<()> {
        if self.screen.width == 0 || self.screen.height == 0 {
            return Ok(());
        }
        let mut buf = Buffer::empty(self.screen);
        render(self.screen, &mut buf);
        let width = self.screen.width as usize;
        let iter = buf
            .content
            .iter()
            .enumerate()
            .map(|(i, c)| ((i % width) as u16, (i / width) as u16, c));
        // Atomic frame (see `draw`): the overlay swaps in one shot, so scrolling it
        // never tears.
        queue!(self.backend, BeginSynchronizedUpdate)?;
        let drawn = self.backend.draw(iter);
        let ended = queue!(self.backend, EndSynchronizedUpdate);
        drawn?;
        ended?;
        Backend::flush(&mut self.backend)
    }

    /// Leave raw mode and drop the cursor just below the live region so the shell
    /// prompt returns directly under it (not at the screen bottom, which would
    /// leave a blank gap when the box is anchored near the top), with the
    /// conversation left intact above.
    pub fn restore(&mut self) -> io::Result<()> {
        match ui::restore_cursor_row(self.view.y, self.view.height, self.screen.height) {
            // Room below the box: land there and wipe anything beneath it (there
            // shouldn't be any — the box is the bottom-most content) so the prompt
            // resumes on a clean line right under the box.
            Some(row) => {
                self.backend.set_cursor_position(Position::new(0, row))?;
                self.backend.clear_region(ClearType::AfterCursor)?;
                self.backend.show_cursor()?;
                disable_raw_mode()?;
            }
            // The box occupies the last screen row: a newline scrolls it up one
            // and lands the shell prompt at the bottom.
            None => {
                self.backend
                    .set_cursor_position(Position::new(0, self.screen.height.saturating_sub(1)))?;
                self.backend.show_cursor()?;
                disable_raw_mode()?;
                write!(self.backend, "\r\n")?;
            }
        }
        Backend::flush(&mut self.backend)
    }

    // --- low-level helpers (ports of ratatui's inline-viewport primitives) ---

    /// Scroll the whole screen up `n` rows by appending lines at the bottom, so
    /// the rows that fall off the top enter the terminal's real scrollback.
    fn scroll_up(&mut self, n: u16) -> io::Result<()> {
        if n > 0 {
            self.backend
                .set_cursor_position(Position::new(0, self.screen.height.saturating_sub(1)))?;
            self.backend.append_lines(n)?;
        }
        Ok(())
    }

    /// Blank `n` rows starting at terminal row `y` — the rows a shrinking box
    /// vacates just below itself (the box is the bottom-most content, so the
    /// rows beneath it are free).
    fn clear_rows(&mut self, y: u16, n: u16) -> io::Result<()> {
        for row in y..y.saturating_add(n) {
            self.backend.set_cursor_position(Position::new(0, row))?;
            self.backend.clear_region(ClearType::CurrentLine)?;
        }
        Ok(())
    }

    /// Write `n` rows of `cells` at terminal row `y`, returning the unused tail of
    /// the slice. Cells are addressed in absolute terminal coordinates.
    fn draw_lines<'a>(&mut self, y: u16, n: u16, cells: &'a [Cell]) -> io::Result<&'a [Cell]> {
        let width = self.screen.width as usize;
        let (head, tail) = cells.split_at(width * n as usize);
        if n > 0 {
            let iter = head
                .iter()
                .enumerate()
                .map(|(i, c)| ((i % width) as u16, y + (i / width) as u16, c));
            self.backend.draw(iter)?;
            Backend::flush(&mut self.backend)?;
        }
        Ok(tail)
    }

    /// Paint a live-region `buffer` (its area is the current viewport) to the
    /// screen in one shot — the region is small, so a full repaint is cheap and
    /// flicker-free.
    fn blit(&mut self, buffer: &Buffer) -> io::Result<()> {
        let area = buffer.area;
        let width = area.width as usize;
        if width == 0 {
            return Ok(());
        }
        let iter = buffer
            .content
            .iter()
            .enumerate()
            .map(|(i, c)| (area.x + (i % width) as u16, area.y + (i / width) as u16, c));
        self.backend.draw(iter)
    }
}

/// Chain a hook that leaves raw mode and shows the cursor before the default
/// panic handler runs, so a panic doesn't strand the terminal in raw mode.
fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), Show);
        previous(info);
    }));
}
