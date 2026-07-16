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
//! unit tests. The *policy* geometry — how tall the live region is, how the box
//! re-pins as it grows, where the cursor lands — comes from pure, tested `ui`
//! helpers ([`ui::live_height`], [`ui::repin`], [`ui::cursor_position`],
//! [`ui::restore_cursor_row`]); the viewport *bookkeeping* (`init`'s anchor
//! math, `write_above`'s multi-screenful scroll plan, `resized`'s re-clamp) is
//! this module's own, smoke-covered like the rest of the boundary.
//!
//! Two operations:
//! - [`InlineViewport::insert_before`] **queues** finished lines for scrollback
//!   above the viewport (codex's `pending_history_lines`); the actual write — a
//!   direct port of ratatui's portable (no-`scrolling-regions`) insert-before
//!   scroll math, including its tmux-safe "draw then clear" ordering — happens
//!   inside the next [`draw`]/[`reflow`] frame (the private `write_above`).
//! - [`InlineViewport::draw`] flushes the queued lines, re-pins the viewport to
//!   its new `height` keeping its **top anchored** (it grows downward in place,
//!   scrolling the screen up only when it would overflow the bottom, and
//!   blanking the rows a shrink vacates), repaints the live region, and places
//!   the hardware cursor — all in **one synchronized update**, so scrollback
//!   growth and the box repaint land as a single atomic frame (no flushed state
//!   ever lacks the box — the streaming flicker; see `docs/flicker.md`).
//!
//! The hardware cursor is **hidden for the whole body of every frame** and shown
//! again only at its final prompt seat (each of [`draw`]/[`reflow`] queues a
//! `Hide` right after opening its synchronized update; `paint_frame` queues the
//! matching `Show` after positioning it). codex's rule — the cursor is only ever
//! *shown* where it comes to rest, never left visible while the redraw scrolls or
//! blits it around. That keeps a terminal cursor-trail animation (kitty) from
//! streaking across the screen when a reflow homes the cursor to the top on a
//! resize / `/clear`, or a scrollback commit yanks it up and back mid-stream.
//!
//! [`draw`]: InlineViewport::draw
//! [`reflow`]: InlineViewport::reflow

use std::io::{self, Stdout, Write};
use std::sync::atomic::{AtomicBool, Ordering};

use ratatui::backend::{Backend, ClearType, CrosstermBackend};
use ratatui::buffer::{Buffer, Cell};
use ratatui::crossterm::cursor::{Hide, Show};
use ratatui::crossterm::event::{
    DisableBracketedPaste, EnableBracketedPaste, KeyboardEnhancementFlags,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
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

/// How [`InlineViewport::reflow`] prepares the screen before rebuilding it from
/// the `tail`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ReflowClear {
    /// Overwrite the screen in place, clearing only the rows *below* the rebuilt
    /// tail (`write_above`'s draw-then-clear). Spill-safe for the Ctrl+O /
    /// `/resume` overlay return (invariant 4): leaving the alternate screen
    /// restores the main screen with its stale streaming strip, and a leading
    /// full clear would make tmux push that strip into scrollback above the
    /// rebuilt conversation (`smoke.sh` Phase 7). Keeps the terminal's own
    /// scrollback, so the caller repaints only the on-screen tail.
    InPlace,
    /// Purge the terminal scrollback **and** clear the whole visible screen
    /// first, then rebuild from the `tail` — a port of codex's
    /// `clear_scrollback_and_visible_screen_ansi`. Used by `/clear` and by every
    /// resize so no duplicated or stale row can survive the rebuild (a resize's
    /// in-place overwrite left the emulator's own reflowed copy of the old
    /// content behind — the duplication this fixes). The purge drops scrollback,
    /// so the caller passes the **full** history as the tail and `write_above`
    /// scrolls the overflow back into the now-empty scrollback, reconstructing
    /// the whole conversation clean.
    Purge,
}

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
    /// Lines queued by [`insert_before`] awaiting the next frame (codex's
    /// `pending_history_lines`): [`draw`] writes them above the viewport inside
    /// the same synchronized update as the live-region repaint, so scrollback
    /// growth and the box land atomically — never a flushed frame without the
    /// box (`docs/flicker.md`). Only [`draw`]/[`reflow`]/[`restore`] flush it —
    /// never [`draw_overlay`] — so queued lines can't be written into the
    /// alternate screen even when a turn dispatched under the Ctrl+O overlay
    /// queues its user bubbles here. [`reflow`] (the overlay's return repaint)
    /// *drops* the queue instead: its rebuilt tail regenerates everything
    /// pending from history.
    ///
    /// [`insert_before`]: InlineViewport::insert_before
    /// [`draw`]: InlineViewport::draw
    /// [`reflow`]: InlineViewport::reflow
    /// [`restore`]: InlineViewport::restore
    /// [`draw_overlay`]: InlineViewport::draw_overlay
    pending: Vec<Line<'static>>,
    /// Whether [`init`] pushed the kitty keyboard-enhancement flags (so the
    /// terminal reports Shift+Enter distinctly from Enter — see
    /// `docs/shift-enter.md`). Recorded so [`restore`] and the panic hook only
    /// **pop** the stack when we actually pushed; `false` when the
    /// `INLINE_TUI_DISABLE_KEYBOARD_ENHANCEMENT` escape hatch is set.
    ///
    /// [`init`]: InlineViewport::init
    /// [`restore`]: InlineViewport::restore
    keyboard_enhanced: bool,
}

impl InlineViewport {
    /// Enter raw mode and reserve `min_height` rows at the bottom for the live
    /// region, anchored to the current cursor row (mirrors how ratatui seats an
    /// inline viewport). Queries the cursor over stdin — safe here because this
    /// runs on the main thread before any reply thread is spawned.
    pub fn init(min_height: u16) -> io::Result<Self> {
        // Decide up front whether to turn on keyboard enhancement, so the panic
        // hook captures the same answer and only pops the stack if we pushed.
        let keyboard_enhanced = !keyboard_enhancement_disabled(
            std::env::var(DISABLE_KEYBOARD_ENHANCEMENT_ENV)
                .ok()
                .as_deref(),
        );
        install_panic_hook(keyboard_enhanced);
        enable_raw_mode()?;
        // Every fallible step from here on runs with raw mode ON, and the
        // caller has no viewport to `restore()` when init itself fails — a
        // bare `?` would strand the parent shell in raw mode (the concrete
        // case: the DSR cursor query timing out). Unwind the terminal state
        // (the panic hook's teardown, minus the alt screen) before
        // propagating the error.
        Self::init_in_raw_mode(min_height, keyboard_enhanced).inspect_err(|_| {
            if keyboard_enhanced {
                let _ = execute!(io::stdout(), PopKeyboardEnhancementFlags);
            }
            let _ = execute!(io::stdout(), DisableBracketedPaste);
            let _ = disable_raw_mode();
            let _ = execute!(io::stdout(), Show);
        })
    }

    /// The raw-mode half of [`init`] — split out so a failure in any step can
    /// unwind the modes already set instead of propagating with raw mode on.
    ///
    /// [`init`]: InlineViewport::init
    fn init_in_raw_mode(min_height: u16, keyboard_enhanced: bool) -> io::Result<Self> {
        let mut backend = CrosstermBackend::new(io::stdout());
        let size = backend.size()?;
        let screen = Rect::new(0, 0, size.width, size.height);

        let height = min_height.clamp(1, size.height.max(1));
        let cursor = backend.get_cursor_position()?;
        // Turn on bracketed paste so a real paste arrives as one `Event::Paste`
        // (a large one collapses to a `[Pasted Content N chars]` placeholder —
        // see docs/paste.md) instead of a burst of synthetic key presses. Like
        // the keyboard-enhancement push below, this is a terminal mode-set with
        // no reply, so it adds no stdin reader (invariant 1 holds).
        let _ = execute!(backend, EnableBracketedPaste);
        // Push the kitty keyboard-enhancement flags *after* the cursor query (a
        // push gets no reply, so it adds no stdin reader — invariant 1 holds) and
        // before the EventStream exists. DISAMBIGUATE_ESCAPE_CODES is what makes a
        // terminal report Shift+Enter as a modified Enter instead of a bare `\r`;
        // terminals that don't support it ignore the sequence. We deliberately do
        // *not* call crossterm's `supports_keyboard_enhancement()` probe — it
        // blocks up to 2s on terminals that never answer. See docs/shift-enter.md.
        if keyboard_enhanced {
            let _ = execute!(
                backend,
                PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
            );
        }
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
            pending: Vec::new(),
            keyboard_enhanced,
        })
    }

    /// The full terminal area, `(0, 0, width, height)` — not the live region,
    /// which is the private `view` field.
    #[must_use]
    pub const fn screen(&self) -> Rect {
        self.screen
    }

    /// Repaint the live region at the new `height`, keeping it **content-anchored**
    /// (its top fixed — it grows downward, not up from the bottom), and place the
    /// hardware cursor on the input's prompt row (derived via
    /// [`ui::cursor_position`], which accounts for the streaming strip, the
    /// queue, the band and the footer). The composer keeps its cursor even while
    /// a reply streams — codex-style, typing mid-turn stays focused; the cursor
    /// is hidden only on the Ctrl+O alternate screen ([`enter_overlay`]).
    ///
    /// Any lines queued by [`insert_before`] are written above the viewport
    /// first, **inside the same frame** — scrollback growth and the box repaint
    /// land as one atomic update (codex's pending-history pattern; the streaming
    /// flicker fix, `docs/flicker.md`).
    ///
    /// The box grows in place until it reaches the screen bottom, at which point
    /// it scrolls the chat up into scrollback; a shrink blanks the rows it vacates.
    ///
    /// [`enter_overlay`]: InlineViewport::enter_overlay
    /// [`insert_before`]: InlineViewport::insert_before
    pub fn draw(
        &mut self,
        height: u16,
        render: impl FnOnce(Rect, &mut Buffer),
        app: &App,
    ) -> io::Result<()> {
        let height = height.clamp(1, self.screen.height.max(1));
        // Emit the whole frame inside a synchronized update (BSU/ESU, DEC mode 2026):
        // the terminal buffers everything between the markers and swaps it in one
        // atomic step, so neither a fast burst of keystrokes nor a scrollback commit
        // ever shows a half-painted frame, a missing box, or the cursor mid-flight.
        // This is the trick codex wraps its draws in. Terminals lacking 2026 ignore
        // the markers. `EndSynchronizedUpdate` is always *queued* (before the
        // `painted?` short-circuit below) so a clean frame closes the update; if a
        // write fails mid-frame the ESU may go unflushed with the cursor still
        // hidden, but that error unwinds the loop and `restore()` flushes + shows
        // the cursor microseconds later, so the terminal is never left that way.
        queue!(self.backend, BeginSynchronizedUpdate)?;
        // Hide the hardware cursor for the duration of the frame, before any
        // scroll (`flush_pending`'s `write_above`) or cell blit moves it — the
        // frame's closing `paint_frame` re-seats it on the prompt row and shows
        // it again. codex's rule: the cursor is only ever *shown* at its final
        // resting position, never left visible while the screen is repainted. On
        // terminals with a cursor-trail animation (kitty) a still-shown cursor
        // otherwise streaks across the screen as the redraw drags it around
        // (worst on a scrollback commit, which yanks it up then back). Hiding is
        // free on plain terminals — it lands in the same synchronized update, so
        // the cursor simply reappears at the prompt with no visible flicker.
        queue!(self.backend, Hide)?;
        let painted = self.paint_live(height, render, app);
        let ended = queue!(self.backend, EndSynchronizedUpdate);
        self.prev = Some(painted?);
        ended?;
        Backend::flush(&mut self.backend)
    }

    /// The body of one inline frame, bracketed by [`draw`]'s synchronized update:
    /// flush the pending scrollback lines, re-pin the viewport to `height`,
    /// render and paint the live region. Returns the painted buffer for `prev`.
    ///
    /// [`draw`]: InlineViewport::draw
    fn paint_live(
        &mut self,
        height: u16,
        render: impl FnOnce(Rect, &mut Buffer),
        app: &App,
    ) -> io::Result<Buffer> {
        // The pending flush reserves `self.view.height` rows *below* the lines
        // it writes ([`write_above_chunk`]'s scroll plan) — sync the tracked
        // height to THIS frame's height first. A commit that lands in the same
        // frame as a strip collapse — a forming table's whole block committing
        // at its close, shrinking the multi-row preview to one row
        // (docs/table-streaming.md) — would otherwise reserve the stale taller
        // strip, land the viewport that much higher, and leave the vacated
        // rows as a blank band under the box (invariant 3; the reported bug —
        // the turn-end `set_view_height` reseat only covers the turn-end
        // flush, not a mid-stream collapse). Gated on pending lines: without a
        // flush the height change must flow through `ui::repin` below, whose
        // shrink is what blanks the rows an in-place shrink vacates.
        if !self.pending.is_empty() {
            self.view.height = height;
        }
        self.flush_pending()?;
        let repin = ui::repin(self.view.y, self.view.height, height, self.screen.height);
        self.view = Rect::new(0, repin.top, self.screen.width, height);
        let mut buf = Buffer::empty(self.view);
        render(self.view, &mut buf);
        self.paint_frame(&buf, &repin, height, app)?;
        Ok(buf)
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
        app: &App,
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
        let (x, y) = ui::cursor_position(self.view, app);
        self.backend.set_cursor_position(Position::new(x, y))?;
        // Re-show the cursor at its final prompt seat, undoing the frame-start
        // Hide ([`draw`]/[`reflow`]) now that every scroll and blit that would
        // have dragged it around is done — so it appears only here, never
        // mid-flight. Queue the Show (ratatui's `show_cursor` would `execute!` —
        // an extra flush mid-synchronized-update); the frame goes out in one write.
        queue!(self.backend, Show)?;
        Ok(())
    }

    /// Queue `lines` for scrollback directly above the viewport. No terminal
    /// I/O happens here (codex's `pending_history_lines`): the next [`draw`] or
    /// [`reflow`] writes them inside the same synchronized update as the
    /// live-region repaint, so the commit and the box land as one atomic frame
    /// (`docs/flicker.md`). Callers already schedule a frame after every state
    /// change; [`restore`] flushes any leftovers if the app quits first.
    ///
    /// [`draw`]: InlineViewport::draw
    /// [`reflow`]: InlineViewport::reflow
    /// [`restore`]: InlineViewport::restore
    pub fn insert_before(&mut self, lines: Vec<Line<'static>>) {
        self.pending.extend(lines);
    }

    /// Write any queued [`insert_before`] lines above the viewport and empty the
    /// queue. Called inside a frame's synchronized update ([`draw`]) or, as a
    /// backstop, by [`restore`].
    ///
    /// [`insert_before`]: InlineViewport::insert_before
    /// [`draw`]: InlineViewport::draw
    /// [`restore`]: InlineViewport::restore
    fn flush_pending(&mut self) -> io::Result<()> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let lines = std::mem::take(&mut self.pending);
        self.write_above(lines)
    }

    /// Commit `lines` into scrollback directly above the viewport, pushing the
    /// viewport down to sit just below them. Port of ratatui's portable
    /// `insert_before`: draw the lines into the rows above the viewport, scrolling
    /// the screen up only as much as needed, then leave the viewport cleared for
    /// the repaint that follows in the same frame.
    fn write_above(&mut self, mut lines: Vec<Line<'static>>) -> io::Result<()> {
        // One commit can exceed u16 rows (a multi-megabyte paste expands back
        // to its full text on send): chunk it so the `as u16` below is exact —
        // a plain cast silently wrapped, dropping almost all of the batch
        // (and exactly 65,536 lines dropped everything via the height == 0
        // early-return).
        while lines.len() > usize::from(u16::MAX) {
            let tail = lines.split_off(usize::from(u16::MAX));
            self.write_above_chunk(lines)?;
            lines = tail;
        }
        self.write_above_chunk(lines)
    }

    /// One ≤ `u16::MAX`-row batch of [`write_above`] — the whole commit in the
    /// common case.
    fn write_above_chunk(&mut self, lines: Vec<Line<'static>>) -> io::Result<()> {
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
        // Clear the viewport now-stale on screen; the paint that follows in this
        // same frame redraws it. We clear *after* drawing (not before scrolling)
        // to dodge a tmux bug where a full clear immediately followed by a scroll
        // spills garbage to scrollback.
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
    /// clears, but the queued lines' flush reserves `self.view.height` rows *below*
    /// them to keep the viewport on screen — so if it still counted the strip it
    /// would over-scroll, and the box would rise as the strip cleared, leaving blank
    /// rows beneath it. Reseating the height to the idle box first lets the
    /// committed final line + spacer replace the strip's rows in place and the box
    /// stay put. (The flush happens at the next [`draw`] and uses the *latest*
    /// tracked height, so this reseat covers every line queued this turn-end.)
    ///
    /// [`insert_before`]: InlineViewport::insert_before
    /// [`draw`]: InlineViewport::draw
    pub fn set_view_height(&mut self, height: u16) {
        self.view.height = height.clamp(1, self.screen.height.max(1));
    }

    /// Purge the terminal scrollback and clear the whole visible screen with a
    /// single ANSI sequence — a port of codex's
    /// `clear_scrollback_and_visible_screen_ansi`. Reset the scroll region + SGR
    /// state (`ESC [ r`, `ESC [ 0 m`), home the cursor (`ESC [ H`), clear the
    /// screen (`ESC [ 2 J`, ED2), purge the scrollback (`ESC [ 3 J`, ED3), then
    /// home again. Emitted as one write because some terminals (Terminal.app,
    /// Warp) don't reliably drop scrollback when the clear and the purge are
    /// separate backend commands. Invalidates `prev` so the next paint redraws in
    /// full. Called by [`reflow`] under [`ReflowClear::Purge`], inside its
    /// synchronized update, so the purge and the rebuild land as one atomic frame.
    ///
    /// [`reflow`]: InlineViewport::reflow
    fn clear_scrollback_and_screen(&mut self) -> io::Result<()> {
        write!(self.backend, "\x1b[r\x1b[0m\x1b[H\x1b[2J\x1b[3J\x1b[H")?;
        self.prev = None;
        Ok(())
    }

    /// Note a new terminal size. Returns whether the size changed at all — any
    /// change forces the conversation repaint (see [`reflow`]): a width change
    /// stales every wrapped line, and a height change moves the screen contents
    /// out from under the tracked viewport row (the emulator scrolls or clips
    /// to fit the new height), so repainting from history is the only way to
    /// reseat the box. The tracked viewport is also pulled back inside the new
    /// screen here (codex re-clamps its insert area the same way), so even a
    /// draw that lands before that repaint can't chase an off-screen row and
    /// scroll a screenful of stale rows into scrollback.
    ///
    /// [`reflow`]: InlineViewport::reflow
    pub fn resized(&mut self, width: u16, height: u16) -> bool {
        let changed = (width, height) != (self.screen.width, self.screen.height);
        self.screen = Rect::new(0, 0, width, height);
        let view_height = self.view.height.clamp(1, height.max(1));
        let view_top = self.view.y.min(height.saturating_sub(view_height));
        self.view = Rect::new(0, view_top, width, view_height);
        self.prev = None; // geometry moved under us; repaint in full next draw
        changed
    }

    /// Repaint the conversation `tail` re-wrapped to the new width after a resize
    /// (or a Ctrl+O return / `/clear`) **and** the live region, in one
    /// synchronized frame: seat the viewport at the top, write the tail so it
    /// fills the screen and pushes the viewport down below it, then paint the
    /// box at its final position — atomically, so the rebuilt screen never
    /// flashes without the box (`docs/flicker.md`). `clear` chooses how the
    /// screen is prepared first — see [`ReflowClear`]: [`InPlace`] overwrites
    /// top-down (the Ctrl+O return), [`Purge`] drops scrollback + clears the
    /// screen (`/clear`, resize) so nothing stale or duplicated survives.
    ///
    /// Lines still queued by [`insert_before`] are **dropped**, not flushed: the
    /// tail regenerates everything pending from history (the caller resets its
    /// `committed` count), so flushing them too would duplicate.
    ///
    /// [`insert_before`]: InlineViewport::insert_before
    /// [`InPlace`]: ReflowClear::InPlace
    /// [`Purge`]: ReflowClear::Purge
    pub fn reflow(
        &mut self,
        tail: Vec<Line<'static>>,
        height: u16,
        clear: ReflowClear,
        render: impl FnOnce(Rect, &mut Buffer),
        app: &App,
    ) -> io::Result<()> {
        let height = height.clamp(1, self.screen.height.max(1));
        self.pending.clear();
        queue!(self.backend, BeginSynchronizedUpdate)?;
        // Hide the cursor before the rebuild (see [`draw`]): a reflow homes the
        // cursor to the top (`clear_scrollback_and_screen`'s `ESC[H`, or
        // `write_above`'s scroll) before `paint_frame` re-seats it on the prompt.
        // Left shown, that top-then-prompt jog is exactly what makes kitty's
        // cursor-trail streak down from the top on a resize / `/clear`. `Purge`
        // even emits its clear as a bare `write!` outside the diff, so hiding
        // here is the only thing that keeps the cursor out of that jog.
        queue!(self.backend, Hide)?;
        let painted = self.paint_reflow(tail, height, clear, render, app);
        let ended = queue!(self.backend, EndSynchronizedUpdate);
        self.prev = Some(painted?);
        ended?;
        Backend::flush(&mut self.backend)
    }

    /// The body of one [`reflow`] frame, bracketed by its synchronized update:
    /// rebuild the screen from the `tail` and paint the live region below it.
    /// Returns the painted buffer for `prev`.
    ///
    /// [`reflow`]: InlineViewport::reflow
    fn paint_reflow(
        &mut self,
        tail: Vec<Line<'static>>,
        height: u16,
        clear: ReflowClear,
        render: impl FnOnce(Rect, &mut Buffer),
        app: &App,
    ) -> io::Result<Buffer> {
        match clear {
            // Purge scrollback + clear the whole screen up front (codex's
            // clear_scrollback_and_visible_screen_ansi). `write_above` then
            // rebuilds the full tail into the freshly-blank screen, scrolling any
            // overflow into the now-empty scrollback — no duplicated or stale row
            // can survive. Safe even on the Ctrl+O return the InPlace arm guards:
            // the ED3 purge drops the spilled strip that a bare clear-then-scroll
            // would have left in scrollback (`/clear` and resize use this).
            ReflowClear::Purge => self.clear_scrollback_and_screen()?,
            // In place: an empty tail (an idle Ctrl+O/`/resume` return with no
            // history) never reaches `write_above`'s draw, so blank the screen
            // outright. A NON-empty tail must *not* `clear_region(All)` first —
            // `write_above` already overwrites top-down and clears the rows below
            // the tail (a spill-safe draw-then-clear); a leading full clear, when
            // the screen still holds the frame restored by leaving the Ctrl+O
            // alt-screen, makes tmux push that stale strip into scrollback.
            ReflowClear::InPlace if tail.is_empty() => {
                self.backend.clear_region(ClearType::All)?;
            }
            ReflowClear::InPlace => {}
        }
        self.backend.set_cursor_position(Position::new(0, 0))?;
        self.prev = None; // the screen is being rebuilt out from under `prev`
        self.view = Rect::new(0, 0, self.screen.width, height);
        self.write_above(tail)?;
        // `write_above` seated the viewport just below the tail; paint the live
        // region there in this same frame (a no-op repin — no scroll, no vacated
        // rows; `prev` is `None`, so `paint_frame` blits in full).
        let repin = ui::Repin {
            scroll_up: 0,
            top: self.view.y,
            clear_below: 0,
        };
        let mut buf = Buffer::empty(self.view);
        render(self.view, &mut buf);
        self.paint_frame(&buf, &repin, height, app)?;
        Ok(buf)
    }

    /// Switch to the alternate screen for the full-screen tool-output overlay,
    /// preserving the inline conversation (the main screen + its scrollback) so
    /// [`exit_overlay`] restores it untouched. The caller paints with
    /// [`draw_overlay`] and must call [`exit_overlay`] to come back.
    ///
    /// [`exit_overlay`]: InlineViewport::exit_overlay
    /// [`draw_overlay`]: InlineViewport::draw_overlay
    pub fn enter_overlay(&mut self) -> io::Result<()> {
        // Record the switch BEFORE emitting it: if the write fails part-way,
        // the exit paths still emit a LeaveAlternateScreen (harmless when the
        // terminal never actually switched) rather than risk stranding it.
        OVERLAY_ACTIVE.store(true, Ordering::SeqCst);
        // Hide the cursor BEFORE switching buffers, not after: the switch + clear
        // move the cursor to the top of the (blank) alternate screen, and a
        // still-shown cursor makes kitty's cursor-trail streak from the composer
        // up into the overlay. The overlay never shows the cursor again
        // ([`draw_overlay`] leaves it hidden), so this holds until [`exit_overlay`]
        // returns and the inline reflow re-seats it on the prompt.
        self.backend.hide_cursor()?;
        execute!(self.backend, EnterAlternateScreen)?;
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
        // Cleared only once the leave was actually written — on failure the
        // flag stays set and restore()/the panic hook retry the leave.
        OVERLAY_ACTIVE.store(false, Ordering::SeqCst);
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
        // Keep the cursor hidden while painting the overlay — [`enter_overlay`]
        // already hid it, but re-assert here so a terminal that resets cursor
        // visibility on the buffer switch can't leave it visible to trail across
        // the full-screen redraw. The overlay never re-shows it; [`exit_overlay`]
        // returns to the inline view, whose reflow re-seats it on the prompt.
        queue!(self.backend, Hide)?;
        let drawn = self.backend.draw(iter);
        let ended = queue!(self.backend, EndSynchronizedUpdate);
        drawn?;
        ended?;
        Backend::flush(&mut self.backend)
    }

    /// Leave raw mode and drop the cursor just below the live region so the shell
    /// prompt returns directly under it (not at the screen bottom, which would
    /// leave a blank gap when the box is anchored near the top), with the
    /// conversation left intact above. If the Ctrl+O overlay is somehow still
    /// up (an error exit that never reached [`exit_overlay`]), the alternate
    /// screen is left first — see [`OVERLAY_ACTIVE`].
    ///
    /// [`exit_overlay`]: InlineViewport::exit_overlay
    pub fn restore(&mut self) -> io::Result<()> {
        // Safety net for exits that break out of the event loop with the
        // Ctrl+O overlay still up (stdin closing, an EventStream error — paths
        // that never reach main's exit_overlay calls): leave the alternate
        // screen FIRST, so everything below — the pending flush, the mode
        // resets, the cursor placement — lands on the primary screen instead
        // of stranding the shell in the alt screen. The primary screen still
        // holds the inline conversation exactly as the overlay left it (the
        // overlay defers commits and never moves the viewport), so `self.view`
        // remains valid for the normal cursor-placement dance below. A no-op
        // on the clean path, where exit_overlay already cleared the flag.
        if OVERLAY_ACTIVE.swap(false, Ordering::SeqCst) {
            let _ = execute!(self.backend, LeaveAlternateScreen);
        }
        // A quit can land between an `insert_before` and the draw tick that would
        // have flushed it (e.g. `/help` then an instant Ctrl+C): write any queued
        // lines now so committed content is never lost with the session.
        self.flush_pending()?;
        // Pop the keyboard-enhancement flags we pushed in `init` (while still in
        // raw mode) so the parent shell doesn't inherit enhanced key reporting.
        if self.keyboard_enhanced {
            let _ = execute!(self.backend, PopKeyboardEnhancementFlags);
        }
        // Turn bracketed paste back off (while still in raw mode) so the parent
        // shell doesn't inherit it.
        let _ = execute!(self.backend, DisableBracketedPaste);
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
    /// the slice. Cells are addressed in absolute terminal coordinates. Queues
    /// only — the enclosing frame ([`draw`]/[`reflow`]/[`restore`]) flushes, so
    /// the commit and the live-region repaint go out in one write.
    ///
    /// [`draw`]: InlineViewport::draw
    /// [`reflow`]: InlineViewport::reflow
    /// [`restore`]: InlineViewport::restore
    fn draw_lines<'a>(&mut self, y: u16, n: u16, cells: &'a [Cell]) -> io::Result<&'a [Cell]> {
        let width = self.screen.width as usize;
        let (head, tail) = cells.split_at(width * n as usize);
        if n > 0 {
            let iter = head
                .iter()
                .enumerate()
                .map(|(i, c)| ((i % width) as u16, y + (i / width) as u16, c));
            self.backend.draw(iter)?;
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

/// Whether the terminal is currently switched to the alternate screen (the
/// Ctrl+O overlay). The runtime sibling of the `keyboard_enhanced` flag the
/// panic hook captures — but overlay state changes after the hook is
/// installed, so it lives in a process-wide atomic instead of a captured bool.
/// Set by [`enter_overlay`], cleared by [`exit_overlay`]; the panic hook and
/// [`restore`] emit a `LeaveAlternateScreen` when it is still set, so the exit
/// paths that never reach main's `exit_overlay` calls — a panic with the
/// overlay up, or a loop error / stdin close breaking out of the event loop —
/// can't strand the shell in the alt screen (scrollback invisible until
/// `reset`).
///
/// [`enter_overlay`]: InlineViewport::enter_overlay
/// [`exit_overlay`]: InlineViewport::exit_overlay
/// [`restore`]: InlineViewport::restore
static OVERLAY_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Chain a hook that leaves raw mode and shows the cursor before the default
/// panic handler runs, so a panic doesn't strand the terminal in raw mode. It
/// first leaves the alternate screen when the Ctrl+O overlay is up
/// ([`OVERLAY_ACTIVE`]), also turns bracketed paste back off, and — when
/// `keyboard_enhanced` (i.e. [`init`] pushed the flags) — pops the
/// keyboard-enhancement stack, so a panic can't leave the shell swapped to the
/// alt screen, with enhanced key reporting, or with paste bracketing.
///
/// [`init`]: InlineViewport::init
fn install_panic_hook(keyboard_enhanced: bool) {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // Leave the alternate screen FIRST: the resets below assume the
        // primary screen (kitty keeps a separate keyboard-enhancement stack
        // per screen buffer, and `init` pushed onto the primary one), and the
        // panic message itself must land where the user can read it.
        if OVERLAY_ACTIVE.swap(false, Ordering::SeqCst) {
            let _ = execute!(io::stdout(), LeaveAlternateScreen);
        }
        if keyboard_enhanced {
            let _ = execute!(io::stdout(), PopKeyboardEnhancementFlags);
        }
        let _ = execute!(io::stdout(), DisableBracketedPaste);
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), Show);
        previous(info);
    }));
}

/// Environment escape hatch: set this to a truthy value to keep keyboard
/// enhancement **off** on terminals where the kitty protocol misbehaves (codex's
/// `CODEX_TUI_DISABLE_KEYBOARD_ENHANCEMENT`). See `docs/shift-enter.md`.
const DISABLE_KEYBOARD_ENHANCEMENT_ENV: &str = "INLINE_TUI_DISABLE_KEYBOARD_ENHANCEMENT";

/// Whether keyboard enhancement should be left **off**, given the value of
/// [`DISABLE_KEYBOARD_ENHANCEMENT_ENV`]. A truthy value (`1`/`true`/`yes`,
/// case-insensitive, surrounding whitespace ignored) disables it; anything else —
/// including unset (`None`) — leaves it enabled. Pure, so it's unit-tested while
/// the surrounding terminal I/O is only smoke-covered.
fn keyboard_enhancement_disabled(value: Option<&str>) -> bool {
    matches!(
        value.map(|v| v.trim().to_ascii_lowercase()).as_deref(),
        Some("1" | "true" | "yes")
    )
}

#[cfg(test)]
mod tests {
    use super::keyboard_enhancement_disabled;

    #[test]
    fn keyboard_enhancement_is_on_by_default() {
        // Unset (or an unrecognised value) keeps enhancement enabled.
        assert!(!keyboard_enhancement_disabled(None));
        assert!(!keyboard_enhancement_disabled(Some("")));
        assert!(!keyboard_enhancement_disabled(Some("0")));
        assert!(!keyboard_enhancement_disabled(Some("false")));
        assert!(!keyboard_enhancement_disabled(Some("nope")));
    }

    #[test]
    fn truthy_env_values_disable_keyboard_enhancement() {
        assert!(keyboard_enhancement_disabled(Some("1")));
        assert!(keyboard_enhancement_disabled(Some("true")));
        assert!(keyboard_enhancement_disabled(Some("TRUE")));
        assert!(keyboard_enhancement_disabled(Some("yes")));
        assert!(keyboard_enhancement_disabled(Some("  Yes  ")));
    }
}
