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
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use ratatui::layout::{Position, Rect};
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Widget};

use crate::ui;

/// A content-anchored inline viewport whose height can change between draws (its
/// top stays put; it grows downward in place).
pub struct InlineViewport {
    backend: CrosstermBackend<Stdout>,
    /// The full terminal area, `(0, 0, width, height)`.
    screen: Rect,
    /// The live region: full width, `height` rows, anchored by its top row.
    view: Rect,
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
        })
    }

    /// The live region rectangle to render into (full width, current height).
    #[must_use]
    pub const fn screen(&self) -> Rect {
        self.screen
    }

    /// Repaint the live region at the new `height`, keeping it **content-anchored**
    /// (its top fixed — it grows downward, not up from the bottom), and place the
    /// hardware cursor. `cursor` is the input text while editing (the cursor sits
    /// at its end); `None` while streaming hides the cursor.
    ///
    /// The box grows in place until it reaches the screen bottom, at which point
    /// it scrolls the chat up into scrollback; a shrink blanks the rows it vacates.
    pub fn draw(
        &mut self,
        height: u16,
        render: impl FnOnce(Rect, &mut Buffer),
        cursor: Option<&str>,
    ) -> io::Result<()> {
        let height = height.clamp(1, self.screen.height.max(1));
        let repin = ui::repin(self.view.y, self.view.height, height, self.screen.height);
        self.scroll_up(repin.scroll_up)?;
        if repin.clear_below > 0 {
            self.clear_rows(repin.top.saturating_add(height), repin.clear_below)?;
        }
        self.view = Rect::new(0, repin.top, self.screen.width, height);

        let mut buf = Buffer::empty(self.view);
        render(self.view, &mut buf);
        self.blit(&buf)?;

        match cursor {
            Some(input) => {
                let (x, y) = ui::cursor_position(self.view, input);
                self.backend.set_cursor_position(Position::new(x, y))?;
                self.backend.show_cursor()?;
            }
            None => self.backend.hide_cursor()?,
        }
        Backend::flush(&mut self.backend)
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
        Ok(())
    }

    /// Note a new terminal size. Returns whether the *width* changed (the only
    /// change that forces the conversation to re-wrap; see [`reflow`]).
    ///
    /// [`reflow`]: InlineViewport::reflow
    pub fn resized(&mut self, width: u16, height: u16) -> bool {
        let changed = width != self.screen.width;
        self.screen = Rect::new(0, 0, width, height);
        changed
    }

    /// Repaint the conversation `tail` re-wrapped to the new width after a resize:
    /// clear the screen, seat the viewport at the top, then `insert_before` the
    /// tail so it fills the screen and pushes the viewport down below it — the next
    /// [`draw`] paints the live region. Mirrors the old `repaint_after_resize`.
    ///
    /// [`draw`]: InlineViewport::draw
    pub fn reflow(&mut self, tail: Vec<Line<'static>>, height: u16) -> io::Result<()> {
        let height = height.clamp(1, self.screen.height.max(1));
        self.backend.clear_region(ClearType::All)?;
        self.backend.set_cursor_position(Position::new(0, 0))?;
        self.view = Rect::new(0, 0, self.screen.width, height);
        self.insert_before(tail)
    }

    /// Leave raw mode and drop the cursor below the live region so the shell
    /// prompt returns on a fresh line, with the conversation left intact above.
    pub fn restore(&mut self) -> io::Result<()> {
        self.backend
            .set_cursor_position(Position::new(0, self.screen.height.saturating_sub(1)))?;
        self.backend.show_cursor()?;
        disable_raw_mode()?;
        // A final newline scrolls the box up one and lands the shell prompt below.
        write!(self.backend, "\r\n")?;
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
