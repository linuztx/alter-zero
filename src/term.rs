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
//! A frame that shows **no** cursor ([`ui::cursor_visible`] — a permission
//! prompt's option list, which is a menu, not a text field) simply skips that
//! `Show`, so the frame-start `Hide` stands and the cursor stays away for as
//! long as the options do (`docs/permissions.md`).
//!
//! [`draw`]: InlineViewport::draw
//! [`reflow`]: InlineViewport::reflow

use std::collections::VecDeque;
use std::io::{self, Stdout, Write};
use std::sync::atomic::{AtomicBool, Ordering};

use ratatui::backend::{Backend, ClearType, CrosstermBackend};
use ratatui::buffer::{Buffer, Cell, CellDiffOption, CellWidth};
use ratatui::crossterm::cursor::{Hide, Show};
use ratatui::crossterm::event::{
    DisableBracketedPaste, EnableBracketedPaste, KeyboardEnhancementFlags,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use ratatui::crossterm::terminal::{
    BeginSynchronizedUpdate, Clear as CrosstermClear, ClearType as CrosstermClearType,
    EndSynchronizedUpdate, EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode,
    enable_raw_mode,
};
use ratatui::crossterm::{execute, queue};
use ratatui::layout::{Position, Rect};
use ratatui::style::Color;
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Widget};

use crate::app::App;
use crate::images::{self, ImageStore};
use crate::links;
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
    /// The last frame painted on the **alternate screen** — the diff baseline
    /// [`draw_overlay`] compares against, so an overlay frame writes only the
    /// cells that actually changed (and an unchanged one writes nothing).
    /// `prev` above is the *inline* live region's baseline and cannot serve:
    /// the two live on different screens, at different areas, and the overlay
    /// entry/exit deliberately clears `prev`. Dropped on every entry, exit and
    /// failed write, each of which leaves the alternate screen in a state this
    /// baseline no longer describes. See [`overlay_updates`] and
    /// `docs/overlay-repaint.md`.
    ///
    /// [`draw_overlay`]: InlineViewport::draw_overlay
    overlay_prev: Option<Buffer>,
    /// Lines queued by [`insert_before`] awaiting the next frame (codex's
    /// `pending_history_lines`): [`draw`] writes them above the viewport inside
    /// the same synchronized update as the live-region repaint, so scrollback
    /// growth and the box land atomically — never a flushed frame without the
    /// box (`docs/flicker.md`). Only [`draw`]/[`reflow`]/[`restore`] flush it —
    /// never [`draw_overlay`] — so a commit made while the Ctrl+O / Ctrl+D /
    /// `/resume` overlay covers the inline view simply **waits here**, and the
    /// overlay's return flushes the whole backlog above the live region
    /// through one ordinary [`draw`] (invariant 4: nothing an overlay-covered
    /// turn produced is lost from the terminal, and nothing is written into
    /// the alternate screen). [`reflow`] (a resize / `/clear` / history
    /// rewind) *drops* the queue instead: its purge-rebuilt tail regenerates
    /// everything pending from history.
    ///
    /// [`insert_before`]: InlineViewport::insert_before
    /// [`draw`]: InlineViewport::draw
    /// [`reflow`]: InlineViewport::reflow
    /// [`restore`]: InlineViewport::restore
    /// [`draw_overlay`]: InlineViewport::draw_overlay
    pending: Vec<Line<'static>>,
    /// Set when the screen moved **one-way while an inline modal was open**
    /// ([`ui::region_is_modal`] — the tool-permission prompt): its growth (or
    /// a commit under it) scrolled chat into the terminal's scrollback, or a
    /// [`reflow`] rebuilt the screen beneath it (a mid-prompt resize's purge,
    /// an overlay return whose prompt opened underneath).
    ///
    /// A prompt grows like any other region — the chat above it scrolls into
    /// **real** scrollback, so the user can scroll up and read while it asks
    /// (`docs/permissions.md`). But a scroll is one-way: the collapse back to
    /// the composer cannot refill the rows it vacates, and a plain shrink
    /// would strand the box above a band of blank rows. The loop reads this
    /// ([`modal_scrolled`], consumed by [`take_modal_scrolled`]) on the first
    /// draw after the prompt closes and answers with a purge rebuild — box
    /// flush at the bottom, scrollback rebuilt from history, nothing lost or
    /// doubled.
    ///
    /// [`reflow`]: InlineViewport::reflow
    /// [`modal_scrolled`]: InlineViewport::modal_scrolled
    /// [`take_modal_scrolled`]: InlineViewport::take_modal_scrolled
    modal_scrolled: bool,
    /// The screen row just below the live region as it was last **painted**
    /// ([`paint_frame`] records it) — unlike `view.height`, immune to the
    /// between-paint re-syncs of [`set_view_height`]. Read by the loop
    /// ([`painted_bottom`]) to tell a modal region pinned flush at the screen
    /// bottom from one still floating above it (`docs/permissions.md`).
    ///
    /// [`paint_frame`]: InlineViewport::paint_frame
    /// [`set_view_height`]: InlineViewport::set_view_height
    /// [`painted_bottom`]: InlineViewport::painted_bottom
    painted_bottom: u16,
    /// Whether [`init`] pushed the kitty keyboard-enhancement flags (so the
    /// terminal reports Shift+Enter distinctly from Enter — see
    /// `docs/shift-enter.md`). Recorded so [`restore`] and the panic hook only
    /// **pop** the stack when we actually pushed; `false` when the
    /// `ALTER_ZERO_DISABLE_KEYBOARD_ENHANCEMENT` escape hatch is set.
    ///
    /// [`init`]: InlineViewport::init
    /// [`restore`]: InlineViewport::restore
    keyboard_enhanced: bool,
    /// Whether link-marked cells are bracketed with the OSC 8 hyperlink
    /// escape so a wrapped URL opens whole (`docs/links.md`). Read from
    /// `ALTER_ZERO_HYPERLINKS` at [`init`] (default on; the
    /// keyboard-enhancement pattern). Off, the emitter still strips the link
    /// carrier — the interned id must never paint as a real underline colour
    /// — it just writes no escapes.
    ///
    /// [`init`]: InlineViewport::init
    hyperlinks: bool,
    /// The terminal's graphics capability and the pictures encoded for it
    /// (`docs/images.md`). Built once in [`init`] — from the environment and
    /// `TIOCGWINSZ`, deliberately **never** from a stdin round trip
    /// (invariant 1; `ImageStore::detect` explains what that cost). Every
    /// paint path stamps the reserved blocks through it just before the cells
    /// go out, which is the [`visible_cells`] rule: a path that skips the
    /// stamp shows blank rows in that view alone.
    ///
    /// [`init`]: InlineViewport::init
    images: ImageStore,
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
        // What graphics this terminal speaks and how big a cell is. No stdin
        // round trip and no query escape: the environment plus `TIOCGWINSZ`,
        // because `ratatui_image`'s own stdio probe leaks a reader thread that
        // eats keystrokes on a terminal that never answers (invariant 1 —
        // `ImageStore::detect`). Anything undetected degrades to unicode
        // half-blocks, which are ordinary cells and need no protocol at all.
        let images = if images::images_disabled(std::env::var(images::IMAGES_ENV).ok().as_deref()) {
            ImageStore::disabled()
        } else {
            ImageStore::detect()
        };
        backend.hide_cursor()?;
        Ok(Self {
            backend,
            screen,
            view,
            prev: None,
            overlay_prev: None,
            pending: Vec::new(),
            modal_scrolled: false,
            painted_bottom: top.saturating_add(height),
            keyboard_enhanced,
            hyperlinks: !links::hyperlinks_disabled(
                std::env::var(links::HYPERLINKS_ENV).ok().as_deref(),
            ),
            images,
        })
    }

    /// What this terminal can do about pictures: whether one can be drawn at
    /// all, its cell size in pixels, and the protocol's name — the three
    /// facts the loop pushes into [`crate::images::set_policy`] at bootstrap
    /// (the `set_clock` pattern). See `docs/images.md`.
    #[must_use]
    pub fn image_capability(&self) -> (bool, Option<images::FontSize>, Option<&'static str>) {
        (
            self.images.is_available(),
            self.images.font_size(),
            self.images.protocol_name(),
        )
    }

    /// Drop every encoded picture, so the next paint re-encodes from the file.
    /// The loop calls this when `/settings` changes the image geometry; the
    /// scrollback purge (`clear_scrollback_and_screen`) does it for itself.
    pub fn invalidate_images(&mut self) {
        self.images.invalidate();
    }

    /// The full terminal area, `(0, 0, width, height)` — not the live region,
    /// which is the private `view` field.
    #[must_use]
    pub const fn screen(&self) -> Rect {
        self.screen
    }

    /// The live region's top row — equivalently, how many screen rows above it
    /// hold committed conversation (the region always sits directly below the
    /// last committed row).
    #[must_use]
    pub const fn view_top(&self) -> u16 {
        self.view.y
    }

    /// The screen row just below the last **painted** live region (its bottom,
    /// exclusive) — where the region actually ended on screen, as opposed to
    /// the *tracked* `view` rect, whose height [`set_view_height`] re-syncs
    /// between paints to plan the next flush. With [`view_top`] and
    /// [`pending_rows`], what the loop's draw tick feeds
    /// [`ui::modal_needs_rebuild`] to catch a modal region about to seat
    /// short of the screen bottom it was painted flush against
    /// (`docs/permissions.md`).
    ///
    /// [`set_view_height`]: InlineViewport::set_view_height
    /// [`view_top`]: InlineViewport::view_top
    /// [`pending_rows`]: InlineViewport::pending_rows
    #[must_use]
    pub const fn painted_bottom(&self) -> u16 {
        self.painted_bottom
    }

    /// Rows queued by [`insert_before`] and not yet flushed — the lines the
    /// next frame's flush will seat the live region below.
    ///
    /// [`insert_before`]: InlineViewport::insert_before
    #[must_use]
    pub const fn pending_rows(&self) -> usize {
        self.pending.len()
    }

    /// Whether the screen moved one-way while an inline modal was open (see
    /// `modal_scrolled`) — the prompt's plain collapse cannot restore what
    /// moved, so its close must purge-rebuild. The loop's draw tick reads
    /// this to route the close; [`take_modal_scrolled`] is the consuming
    /// read.
    ///
    /// [`take_modal_scrolled`]: InlineViewport::take_modal_scrolled
    pub const fn modal_scrolled(&self) -> bool {
        self.modal_scrolled
    }

    /// Take the moved-under-a-modal note (see `modal_scrolled`), clearing it.
    /// Called by the close's purge rebuild, which regenerates everything the
    /// note stood for.
    pub const fn take_modal_scrolled(&mut self) -> bool {
        std::mem::replace(&mut self.modal_scrolled, false)
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
    /// A tool-permission prompt ([`ui::region_is_modal`]) grows the same way —
    /// its scroll keeps the conversation reachable in real scrollback while it
    /// asks — but the one-way move is noted (`modal_scrolled`) so the prompt's
    /// close can purge-rebuild instead of shrinking over the rows it can't
    /// refill (`docs/permissions.md`).
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
        let flushing = !self.pending.is_empty();
        if flushing {
            self.view.height = height;
        }
        self.flush_pending()?;
        let repin = ui::repin(self.view.y, self.view.height, height, self.screen.height);
        // A tool-permission prompt grows by this same rule — the chat above
        // it scrolls into the terminal's **real** scrollback, so the user can
        // scroll up and read while it asks (`docs/permissions.md`; the
        // covering geometry this replaces held those rows in no buffer at
        // all, which read as "terminal scroll is disabled" — worst in kitty).
        // But the move is one-way: the collapse back to the composer cannot
        // refill what a grow scrolled off (or what a commit under the open
        // prompt scrolled — a batch's cell resolving between back-to-back
        // prompts), so note it and the loop answers the prompt's close with a
        // purge rebuild instead of the plain shrink that stranded the box
        // over a blank band.
        if ui::region_is_modal(app) && (flushing || repin.scroll_up > 0) {
            self.modal_scrolled = true;
        }
        self.view = Rect::new(0, repin.top, self.screen.width, height);
        let mut buf = Buffer::empty(self.view);
        render(self.view, &mut buf);
        self.images.stamp(&mut buf);
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
                    self.draw_cells(updates.into_iter())?;
                }
            }
            _ => self.blit(buf)?,
        }
        // Record where this frame's region actually ended on screen — the
        // datum `painted_bottom` serves the loop (`view.height` alone can't:
        // `set_view_height` re-syncs it between paints for the flush plan).
        self.painted_bottom = repin.top.saturating_add(height);
        let (x, y) = ui::cursor_position(self.view, app);
        self.backend.set_cursor_position(Position::new(x, y))?;
        // Re-show the cursor at its final prompt seat, undoing the frame-start
        // Hide ([`draw`]/[`reflow`]) now that every scroll and blit that would
        // have dragged it around is done — so it appears only here, never
        // mid-flight. Queue the Show (ratatui's `show_cursor` would `execute!` —
        // an extra flush mid-synchronized-update); the frame goes out in one write.
        //
        // Unless the frame shows none at all ([`ui::cursor_visible`] — a
        // permission prompt's option list is a menu, not a text field): then
        // the frame-start Hide simply stands, and the cursor stays away for as
        // long as the options do. It is still *seated* above, so its return
        // when the prompt closes starts from a sensible row.
        if ui::cursor_visible(app) {
            queue!(self.backend, Show)?;
        }
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
        // A zero-row screen (a tiling WM mid-animation can report one) has
        // nowhere to draw *and* nowhere to scroll from: the screenful loop
        // below would spin forever on `to_draw = remaining.min(0)`. Drop the
        // batch — nothing can be shown at this size, and the resize back to a
        // real height purge-rebuilds the whole conversation from history.
        if self.screen.height == 0 {
            return Ok(());
        }
        let area = Rect::new(0, 0, self.screen.width, height);
        let mut buffer = Buffer::empty(area);
        Paragraph::new(lines).render(area, &mut buffer);
        // Turn the reserved blocks into pictures *before* the rows are
        // addressed: a committed image is written into the terminal's real
        // scrollback exactly once, and scrolls with the text from then on
        // (`docs/images.md`).
        self.images.stamp(&mut buffer);
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
    /// full. Called by [`reflow`], inside its synchronized update, so the
    /// purge and the rebuild land as one atomic frame.
    ///
    /// [`reflow`]: InlineViewport::reflow
    fn clear_scrollback_and_screen(&mut self) -> io::Result<()> {
        write!(self.backend, "\x1b[r\x1b[0m\x1b[H\x1b[2J\x1b[3J\x1b[H")?;
        self.prev = None;
        // The purge takes the pictures with it: a kitty placement transmits
        // its pixels once per encoded protocol, so one that outlived the
        // purge would place an image the terminal may already have dropped.
        // Re-encoding is the price of a rebuild that is actually correct —
        // and the rebuild is re-measuring every picture for the new width
        // anyway (`docs/images.md`).
        self.images.invalidate();
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
        // The emulator moved the painted rows to fit the new height; clamp
        // the painted-bottom note inside it (the forced repaint that follows
        // re-records the real one).
        self.painted_bottom = self.painted_bottom.min(height);
        self.prev = None; // geometry moved under us; repaint in full next draw
        // …and the alt screen the emulator just reflowed under us. The area
        // check in `overlay_paint` is NOT enough on its own: a resize burst can
        // coalesce into one frame (`on_resize` asks for a frame, but the
        // scheduler's floor is 8ms), so the net size can land back on the one
        // the baseline records while the terminal clipped and regrew the
        // alternate screen in between — diffing against that baseline would
        // leave the vacated rows stale until the overlay was closed. Dropping
        // it unconditionally, `changed` or not, costs one full repaint.
        self.overlay_prev = None;
        changed
    }

    /// Rebuild the whole screen from the conversation `tail` re-wrapped to the
    /// current width (a resize, `/clear`, a history rewind) **and** the live
    /// region, in one synchronized frame: purge the terminal scrollback and
    /// clear the screen (codex's `clear_scrollback_and_visible_screen_ansi` —
    /// nothing stale or duplicated survives, and the emulator's own reflowed
    /// copies of the old rows go with it), seat the viewport at the top, write
    /// the tail so it fills the screen and scrolls the overflow into the
    /// now-empty scrollback, then paint the box at its final position —
    /// atomically, so the rebuilt screen never flashes without the box
    /// (`docs/flicker.md`). The caller passes the **full** history as the
    /// tail, since the purge dropped everything.
    ///
    /// Lines still queued by [`insert_before`] are **dropped**, not flushed:
    /// the tail regenerates everything pending from history (the caller resets
    /// its `committed` count), so flushing them too would duplicate. (An
    /// ordinary overlay return never comes here — it *flushes* its queue
    /// through [`draw`] instead, keeping the terminal's own scrollback.)
    ///
    /// [`insert_before`]: InlineViewport::insert_before
    /// [`draw`]: InlineViewport::draw
    pub fn reflow(
        &mut self,
        tail: Vec<Line<'static>>,
        height: u16,
        render: impl FnOnce(Rect, &mut Buffer),
        app: &App,
    ) -> io::Result<()> {
        let height = height.clamp(1, self.screen.height.max(1));
        // Take, don't clear: `clear` keeps the Vec's high-water capacity for
        // the rest of the session (a turn that streamed under an overlay can
        // queue thousands of rows), while the flush path's `mem::take` already
        // releases it — the rebuild drops the stale queue the same way.
        drop(std::mem::take(&mut self.pending));
        // A modal open right now takes its seat by this rebuild's real write —
        // note it (`modal_scrolled`): the close must purge-rebuild or its
        // collapse strands the box above the rows it vacates. Recording this
        // HERE, at the one place every full rebuild goes through, is what
        // keeps every rebuild source honest — the resize purge and the
        // Ctrl+O / Ctrl+D / agent-view returns alike (`docs/permissions.md`).
        self.modal_scrolled |= ui::region_is_modal(app);
        queue!(self.backend, BeginSynchronizedUpdate)?;
        // Hide the cursor before the rebuild (see [`draw`]): a reflow homes the
        // cursor to the top (`clear_scrollback_and_screen`'s `ESC[H`, or
        // `write_above`'s scroll) before `paint_frame` re-seats it on the prompt.
        // Left shown, that top-then-prompt jog is exactly what makes kitty's
        // cursor-trail streak down from the top on a resize / `/clear`. `Purge`
        // even emits its clear as a bare `write!` outside the diff, so hiding
        // here is the only thing that keeps the cursor out of that jog.
        queue!(self.backend, Hide)?;
        let painted = self.paint_reflow(tail, height, render, app);
        let ended = queue!(self.backend, EndSynchronizedUpdate);
        self.prev = Some(painted?);
        ended?;
        Backend::flush(&mut self.backend)
    }

    /// The body of one [`reflow`] frame, bracketed by its synchronized update:
    /// purge, rebuild the screen from the `tail`, and paint the live region
    /// below it. Returns the painted buffer for `prev`.
    ///
    /// [`reflow`]: InlineViewport::reflow
    fn paint_reflow(
        &mut self,
        tail: Vec<Line<'static>>,
        height: u16,
        render: impl FnOnce(Rect, &mut Buffer),
        app: &App,
    ) -> io::Result<Buffer> {
        // Purge scrollback + clear the whole screen up front (codex's
        // clear_scrollback_and_visible_screen_ansi). `write_above` then
        // rebuilds the full tail into the freshly-blank screen, scrolling any
        // overflow into the now-empty scrollback — no duplicated or stale row
        // can survive.
        self.clear_scrollback_and_screen()?;
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
        self.images.stamp(&mut buf);
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
        // QUEUE the whole switch — no flush: the caller paints the first
        // overlay frame right after ([`draw_overlay`]), whose flush delivers
        // the switch and the painted frame as one write, so the terminal hops
        // from the live inline frame straight to the finished overlay. The old
        // flush-then-build order parked the terminal on a blank alternate
        // screen for as long as the first frame took to build — long enough on
        // a big resumed session for kitty's cursor-trail animation to streak
        // up the empty screen before the overlay appeared (and the build now
        // stalls with the inline view still intact instead of a black hole).
        // The cursor Hide leads the switch so no flushed state ever shows it
        // mid-hop: the overlay never displays a cursor ([`draw_overlay`]
        // re-asserts the hide), and the inline reflow re-seats it on the
        // prompt after [`exit_overlay`].
        //
        // [`draw_overlay`]: InlineViewport::draw_overlay
        // [`exit_overlay`]: InlineViewport::exit_overlay
        queue!(
            self.backend,
            Hide,
            EnterAlternateScreen,
            CrosstermClear(CrosstermClearType::All)
        )?;
        self.prev = None; // the inline view isn't on screen now
        // …and the Clear(All) queued above blanks the alternate screen, so the
        // previous overlay frame no longer describes it: the next
        // [`draw_overlay`] must paint every cell before it can diff again.
        //
        // [`draw_overlay`]: InlineViewport::draw_overlay
        self.overlay_prev = None;
        Ok(())
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
        // The alternate screen is gone; a re-entry clears it and starts over.
        self.overlay_prev = None;
        Backend::flush(&mut self.backend)
    }

    /// Paint a full-screen `render` onto the alternate screen (used for the
    /// tool-output overlay).
    ///
    /// **Diffed against the previous frame** (`overlay_updates`): only the
    /// cells that actually changed reach the terminal, and an unchanged frame
    /// reaches it not at all — no cells, and no synchronized-update or
    /// cursor-move escapes either, so a still page is silence on the wire.
    /// That silence is what lets the user select and copy text in the Ctrl+O /
    /// Ctrl+D views **while a turn streams**: a terminal drops a mouse
    /// selection when the cells under it are rewritten, and this used to
    /// re-serialize the whole screen on every one of up to 120 frames a second
    /// (`docs/overlay-repaint.md`).
    ///
    /// With no baseline — the first frame after [`enter_overlay`]'s clear, a
    /// resize, or a write that failed part-way — it falls back to painting
    /// every cell of the screen, so empty rows are blanked and no separate
    /// clear is needed between frames.
    ///
    /// [`enter_overlay`]: InlineViewport::enter_overlay
    pub fn draw_overlay(&mut self, render: impl FnOnce(Rect, &mut Buffer)) -> io::Result<()> {
        if self.screen.width == 0 || self.screen.height == 0 {
            return Ok(());
        }
        let mut buf = Buffer::empty(self.screen);
        render(self.screen, &mut buf);
        // Stamp before the diff, so the baseline describes the picture too and
        // a still page stays silent (`docs/images.md`, `docs/overlay-repaint.md`).
        self.images.stamp(&mut buf);
        let paint = overlay_paint(self.overlay_prev.as_ref(), &buf);
        // Nothing moved: emit NOTHING. Not a cell, not a synchronized-update
        // bracket, not a cursor move — the whole point is that a terminal
        // holding a selection over this page sees no write at all. (Skipping
        // the Hide here is safe: the alternate screen is always entered with a
        // Full frame right behind it, which asserts it.)
        if matches!(paint, OverlayPaint::Unchanged) {
            return Ok(());
        }
        // The policy seat for the hidden cursor: just past the frame's last
        // glyph — the closing `q/esc/… to quit` hint (see below).
        let (seat_x, seat_y) = ui::overlay_cursor_seat(&buf);
        // Atomic frame (see `draw`): the overlay swaps in one shot, so scrolling it
        // never tears.
        queue!(self.backend, BeginSynchronizedUpdate)?;
        // Keep the cursor hidden while painting the overlay — [`enter_overlay`]
        // already hid it, but re-assert here so a terminal that resets cursor
        // visibility on the buffer switch can't leave it visible to trail across
        // the full-screen redraw. The overlay never re-shows it; [`exit_overlay`]
        // returns to the inline view, whose reflow re-seats it on the prompt.
        queue!(self.backend, Hide)?;
        let drawn = match paint {
            OverlayPaint::Diff(updates) => self.draw_cells(updates.into_iter()),
            // No comparable frame on screen: every visible cell goes out,
            // minus the wide-glyph shadows.
            _ => {
                let width = self.screen.width as usize;
                self.draw_cells(visible_cells(&buf.content, width, Position::new(0, 0)))
            }
        };
        // Seat the (hidden) cursor at the end of the overlay's last text —
        // the closing `q/esc/… to quit` hint ([`ui::overlay_cursor_seat`])
        // — instead of leaving it wherever the cell paint ended (the blank
        // bottom-right corner). The hide keeps it invisible, but a terminal
        // with a cursor-move animation still animates toward the seat, so
        // opening Ctrl+O / Ctrl+D used to streak the animation to nowhere;
        // now it lands on the hint. The inline `paint_frame` seats its
        // cursor the same way, via [`ui::cursor_position`].
        let seated = self
            .backend
            .set_cursor_position(Position::new(seat_x, seat_y));
        let ended = queue!(self.backend, EndSynchronizedUpdate);
        let painted = drawn
            .and(seated)
            .and(ended)
            .and_then(|()| Backend::flush(&mut self.backend));
        // Keep this frame as the next one's baseline — but only if it landed
        // whole. A write that failed part-way leaves the alternate screen in a
        // state no buffer describes, so drop the baseline and let the next
        // frame repaint in full rather than diff against a fiction.
        self.overlay_prev = painted.is_ok().then_some(buf);
        painted
    }

    /// Leave raw mode and drop the cursor just below the live region so the shell
    /// prompt returns directly under it (not at the screen bottom, which would
    /// leave a blank gap when the box is anchored near the top), with the
    /// conversation left intact above. If the Ctrl+O overlay is somehow still
    /// up (an error exit that never reached [`exit_overlay`]), the alternate
    /// screen is left first — see `OVERLAY_ACTIVE`.
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

    /// Emit cells to the backend, bracketing link-marked runs with the OSC 8
    /// hyperlink escape so a wrapped URL opens whole (`docs/links.md`). The
    /// single choke point **every** cell-writing path goes through —
    /// scrollback commits + `reflow` ([`draw_lines`]), the live-region blit
    /// and diff ([`paint_frame`]), and the alt-screen overlay
    /// ([`draw_overlay`]) — so a link stays clickable wherever its cells are
    /// painted, and the id carrier is *always* stripped before the terminal
    /// could show it as a real underline colour (the `visible_cells` rule:
    /// missing one path leaves the bug alive in that view alone). Unmarked
    /// runs pass through untouched.
    ///
    /// [`draw_lines`]: InlineViewport::draw_lines
    /// [`paint_frame`]: InlineViewport::paint_frame
    /// [`draw_overlay`]: InlineViewport::draw_overlay
    fn draw_cells<'a>(
        &mut self,
        content: impl Iterator<Item = (u16, u16, &'a Cell)>,
    ) -> io::Result<()> {
        let mut run: Vec<(u16, u16, &'a Cell)> = Vec::new();
        let mut run_id: Option<u32> = None;
        for item in content {
            let id = links::carrier_id(item.2.underline_color);
            if id != run_id {
                self.flush_cell_run(&run, run_id)?;
                run.clear();
                run_id = id;
            }
            // A cell carrying a graphics protocol's whole escape sequence
            // (`ForcedWidth` — one column on screen, hundreds of bytes on the
            // wire; `docs/images.md`) can leave the cursor anywhere: a sixel
            // placement clears its area row by row first, and the backend only
            // re-addresses a cell that isn't adjacent to the one before it. So
            // the run ends *behind* the blob, and the next cell starts a fresh
            // `draw` — which always opens with a cursor move.
            let blob = matches!(item.2.diff_option, CellDiffOption::ForcedWidth(_));
            run.push(item);
            if blob {
                self.flush_cell_run(&run, run_id)?;
                run.clear();
            }
        }
        self.flush_cell_run(&run, run_id)
    }

    /// One grouped run of [`draw_cells`]: a plain run goes straight to the
    /// backend (no copies); a marked run's cells are re-emitted with the
    /// carrier cleared between an [`links::osc8_open`]/[`links::OSC8_CLOSE`]
    /// pair. Hyperlinks attach per printed cell in every terminal, so
    /// re-painting any subset of a link (the diff path) re-links exactly
    /// those cells and the rest keep theirs. With hyperlinks off — the env
    /// gate, or an id this process never interned — the strip still happens;
    /// only the escapes are skipped.
    ///
    /// [`draw_cells`]: InlineViewport::draw_cells
    fn flush_cell_run(&mut self, run: &[(u16, u16, &Cell)], id: Option<u32>) -> io::Result<()> {
        if run.is_empty() {
            return Ok(());
        }
        let Some(id) = id else {
            return self.backend.draw(run.iter().copied());
        };
        let url = links::link_url(id).filter(|_| self.hyperlinks);
        if let Some(url) = &url {
            write!(self.backend, "{}", links::osc8_open(id, url))?;
        }
        let cleaned: Vec<Cell> = run
            .iter()
            .map(|(_, _, cell)| {
                let mut cell = (*cell).clone();
                cell.underline_color = Color::Reset;
                cell
            })
            .collect();
        self.backend.draw(
            run.iter()
                .zip(&cleaned)
                .map(|(&(x, y, _), cell)| (x, y, cell)),
        )?;
        if url.is_some() {
            write!(self.backend, "{}", links::OSC8_CLOSE)?;
        }
        Ok(())
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
            self.draw_cells(visible_cells(head, width, Position::new(0, y)))?;
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
        self.draw_cells(visible_cells(
            &buffer.content,
            width,
            Position::new(area.x, area.y),
        ))
    }
}

/// The cells of a rendered buffer that should actually reach the terminal: each
/// `width`-column row's cells at their `origin`-offset coordinates, **minus the
/// cells shadowed by a preceding wide glyph** — the blank continuation cells
/// `Buffer::set_stringn` resets behind an emoji/CJK cluster (a glyph's
/// `cell_width - 1` followers).
///
/// Emitting a shadowed cell prints its `" "` one column PAST the glyph (the
/// terminal's cursor already advanced the glyph's full width), shifting the rest
/// of the row right by one per wide glyph — which misaligned every row-final
/// `│` and pushed full-width table rows past the terminal edge, where the wrap
/// "cut" the grid whenever a cell held an emoji (`docs/table-streaming.md`).
/// Skipping them mirrors ratatui's own `Buffer::diff`, whose `Terminal::flush`
/// never emits shadowed cells: the backend's position check then issues an
/// absolute `MoveTo` across the gap, so the next glyph lands on its true column
/// no matter how wide the terminal actually drew the cluster. Widths come from
/// ratatui's own [`CellWidth`] — the same measure `set_stringn` reserved the
/// shadows with — so the reservation and the skip can never disagree, and the
/// shadow is clamped to the row so it never crosses a line boundary.
///
/// One targeted exception, ported from `Buffer::diff`'s VS16 workaround:
/// terminals disagree on the width of an emoji **presentation sequence**
/// (`…\u{FE0F}`), so on one that draws it narrow a skipped shadow could keep
/// stale screen content visible beside the glyph. For a VS16-bearing wide cell
/// the shadow cells are emitted *first* (scrubbing what the glyph may not
/// cover — the backend `MoveTo`s them absolutely), then the glyph itself, drawn
/// last so it lands whole on the terminals that do render it wide.
///
/// Shared by **all three** full-cell paints — [`InlineViewport::draw_lines`]
/// (scrollback commits + `reflow`), [`InlineViewport::blit`] (live-region
/// repaints), and [`InlineViewport::draw_overlay`] (the alt-screen transcript,
/// `/resume` picker and Ctrl+D context view). Missing any one of them leaves the
/// tear alive in that view alone, which is why `smoke.sh` Phase 41 now checks
/// the inline pane *and* the Ctrl+O overlay.
/// What one alternate-screen frame has to write, given the frame the overlay
/// last painted there (`prev`) and the one being drawn (`next`).
///
/// The alternate screen's answer to the inline region's diff
/// ([`InlineViewport::paint_frame`]), and it exists for the same reason plus a
/// sharper one. The overlay used to repaint in full on **every** frame, and
/// while a turn streams underneath the loop schedules up to 120 of those a
/// second (every reply event asks for one). Terminals drop an in-progress
/// mouse selection when cells inside it are **written**, not when their
/// content changes — so re-sending an identical screen at that rate made the
/// text physically unselectable: nothing could be copied out of Ctrl+O /
/// Ctrl+D until the turn finished. Ctrl+D was the plainest case, its
/// `ContextSig` ignoring the streaming length, so the page it repainted was
/// byte-identical every time.
///
/// Hence [`Unchanged`], which short-circuits [`InlineViewport::draw_overlay`]
/// before it touches the backend at all: not one cell, and none of the
/// synchronized-update or cursor-seat escapes either. A [`Full`] paint is the
/// answer whenever no comparable frame is on screen — none recorded (the
/// alternate screen was just cleared by [`InlineViewport::enter_overlay`]), or
/// a geometry that no longer matches (`Buffer::diff` needs equal areas).
///
/// Pure, so the decision is unit-tested while the surrounding terminal I/O is
/// only smoke-covered. The diff is [`Buffer::diff`]'s, which is where
/// [`visible_cells`] took its wide-glyph and VS16 handling from, so a partial
/// repaint cannot tear a table the full paint gets right.
/// See `docs/overlay-repaint.md`.
///
/// [`Unchanged`]: OverlayPaint::Unchanged
/// [`Full`]: OverlayPaint::Full
fn overlay_paint<'a>(prev: Option<&Buffer>, next: &'a Buffer) -> OverlayPaint<'a> {
    match prev {
        Some(prev) if prev.area == next.area => {
            let updates = prev.diff(next);
            if updates.is_empty() {
                OverlayPaint::Unchanged
            } else {
                OverlayPaint::Diff(updates)
            }
        }
        _ => OverlayPaint::Full,
    }
}

/// The verdict of [`overlay_paint`]: how much of an overlay frame reaches the
/// terminal.
enum OverlayPaint<'a> {
    /// Identical to the frame already on the alternate screen — write nothing,
    /// so a mouse selection over it survives.
    Unchanged,
    /// Only these cells differ from the frame on screen.
    Diff(Vec<(u16, u16, &'a Cell)>),
    /// No comparable frame on screen — emit every visible cell
    /// ([`visible_cells`]).
    Full,
}

fn visible_cells(
    cells: &[Cell],
    width: usize,
    origin: Position,
) -> impl Iterator<Item = (u16, u16, &Cell)> {
    let width = width.max(1);
    let at = move |i: usize| {
        (
            origin.x + (i % width) as u16,
            origin.y + (i / width) as u16,
            &cells[i],
        )
    };
    let mut i = 0usize;
    let mut queued: VecDeque<usize> = VecDeque::new(); // emission order, by index
    std::iter::from_fn(move || {
        loop {
            if let Some(j) = queued.pop_front() {
                return Some(at(j));
            }
            if i >= cells.len() {
                return None;
            }
            let cell = &cells[i];
            // A cell the graphics protocol covers must never be written: the
            // escape already painted those columns, and a space over them
            // punches a hole in the picture (`docs/images.md`). `Buffer::diff`
            // honours this itself, so only the direct-emit paths need it.
            if matches!(cell.diff_option, CellDiffOption::Skip) {
                i += 1;
                continue;
            }
            let col = i % width;
            // `Cell::cell_width`, never `cell.symbol().cell_width()`: an image
            // cell's symbol is the whole escape sequence, hundreds of bytes
            // wide as text and exactly one column on screen, and only the
            // `Cell` impl honours the `ForcedWidth` that says so.
            let shadow = (cell.cell_width() as usize)
                .saturating_sub(1)
                .min(width - 1 - col);
            if shadow > 0 && cell.symbol().contains('\u{FE0F}') {
                queued.extend(i + 1..=i + shadow); // scrub the shadow first…
            }
            queued.push_back(i); // …then the glyph itself (or the lone cell)
            i += 1 + shadow;
        }
    })
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
const DISABLE_KEYBOARD_ENHANCEMENT_ENV: &str = "ALTER_ZERO_DISABLE_KEYBOARD_ENHANCEMENT";

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
    use ratatui::buffer::{Buffer, Cell};
    use ratatui::layout::{Position, Rect};
    use ratatui::style::Style;

    use super::{OverlayPaint, keyboard_enhancement_disabled, overlay_paint, visible_cells};

    /// Render `line` into a `width`-wide one-row buffer the way every full-cell
    /// paint does, then list what [`visible_cells`] would actually send to the
    /// backend, as `(column, symbol)` in emission order.
    fn drawn(line: &str, width: u16) -> Vec<(u16, String)> {
        let mut buf = Buffer::empty(Rect::new(0, 0, width, 1));
        buf.set_string(0, 0, line, Style::default());
        visible_cells(&buf.content, width as usize, Position::new(0, 0))
            .map(|(x, _, c)| (x, c.symbol().to_string()))
            .collect()
    }

    #[test]
    fn visible_cells_skips_the_cells_shadowed_by_a_wide_glyph() {
        // A wide glyph occupies one buffer cell plus a reset continuation cell
        // for the column it covers on screen. Emitting that continuation cell
        // prints a real space one column PAST the glyph (the terminal's cursor
        // already advanced two), drifting the rest of the row right — the bug
        // that misaligned and wrapped ("cut") table rows holding emoji. The
        // emitter must skip shadowed cells, exactly like `Buffer::diff`.
        assert_eq!(
            drawn("a🔥b", 6),
            vec![
                (0, "a".to_string()),
                (1, "🔥".to_string()),
                // column 2 is 🔥's shadow — the terminal covers it itself.
                (3, "b".to_string()),
                (4, " ".to_string()),
                (5, " ".to_string()),
            ]
        );
    }

    #[test]
    fn visible_cells_skips_the_shadow_of_every_wide_cluster_kind() {
        // CJK, a ZWJ family sequence, and a halfwidth-katakana dakuten pair all
        // occupy two columns per ratatui's `CellWidth` — the measure the buffer
        // used when it reset the continuation cell — so each shadows exactly the
        // one cell after it.
        for glyph in ["你", "👨\u{200D}👩\u{200D}👧\u{200D}👦", "ｶ\u{FF9E}"] {
            let out = drawn(&format!("{glyph}x"), 6);
            assert_eq!(out[0].1, glyph, "the cluster survives whole");
            assert_eq!(
                out[1],
                (2, "x".to_string()),
                "the next emitted cell sits AFTER the shadow: {glyph:?}"
            );
            assert!(
                !out.iter().any(|(x, _)| *x == 1),
                "the shadowed column is never emitted: {glyph:?}"
            );
        }
    }

    #[test]
    fn visible_cells_emits_every_cell_of_a_narrow_row() {
        // The common case is untouched: every cell of an all-ASCII row is sent,
        // in order, with no gaps for the backend to `MoveTo` across.
        assert_eq!(
            drawn("ab", 3),
            vec![
                (0, "a".to_string()),
                (1, "b".to_string()),
                (2, " ".to_string()),
            ]
        );
    }

    #[test]
    fn back_to_back_wide_glyphs_each_skip_one_shadow() {
        // Two adjacent emoji: columns 1 and 3 are shadows, so the run is drawn
        // at 0 and 2 — and the trailing pad keeps its own columns.
        assert_eq!(
            drawn("✅✅x", 6),
            vec![
                (0, "✅".to_string()),
                (2, "✅".to_string()),
                (4, "x".to_string()),
                (5, " ".to_string()),
            ]
        );
    }

    #[test]
    fn visible_cells_scrubs_then_redraws_a_vs16_shadow() {
        // Terminals disagree on VS16 emoji-presentation width, so the shadowed
        // cell can be left showing stale screen content on one that draws the
        // glyph narrow. For a VS16-bearing wide glyph the shadow is emitted
        // FIRST — scrubbing whatever the glyph won't cover — and the glyph
        // itself last, whole, over its own column. `Buffer::diff` carries the
        // same targeted workaround; every other wide cluster keeps the plain
        // skip (asserted above).
        assert_eq!(
            drawn("e⚠\u{FE0F}w", 6),
            vec![
                (0, "e".to_string()),
                (2, " ".to_string()),
                (1, "⚠\u{FE0F}".to_string()),
                (3, "w".to_string()),
                (4, " ".to_string()),
                (5, " ".to_string()),
            ],
            "the scrub space goes out before the glyph, the walk resumes after"
        );
    }

    #[test]
    fn visible_cells_shadow_stays_in_its_row_and_the_origin_offsets() {
        // A wide glyph ending row 0 must not swallow row 1's first cell, and
        // blit's area offset must land on every coordinate.
        let mut buf = Buffer::empty(Rect::new(0, 0, 4, 2));
        buf.set_string(0, 0, "ab🔥", Style::default());
        buf.set_string(0, 1, "cd", Style::default());
        let out: Vec<(u16, u16, &Cell)> =
            visible_cells(&buf.content, 4, Position::new(2, 5)).collect();
        let coords: Vec<(u16, u16)> = out.iter().map(|&(x, y, _)| (x, y)).collect();
        assert_eq!(
            coords,
            vec![(2, 5), (3, 5), (4, 5), (2, 6), (3, 6), (4, 6), (5, 6)],
            "row 0 drops only its shadowed column; row 1 emits in full"
        );
        assert_eq!(out[2].2.symbol(), "🔥");
        assert_eq!(
            out[3].2.symbol(),
            "c",
            "row 1 starts fresh — a shadow never crosses a row boundary"
        );
    }

    /// Two `width`-wide overlay frames from their row texts.
    fn frames(a: &[&str], b: &[&str], width: u16) -> (Buffer, Buffer) {
        let build = |rows: &[&str]| {
            let mut buf = Buffer::empty(Rect::new(0, 0, width, rows.len() as u16));
            for (y, row) in rows.iter().enumerate() {
                buf.set_string(0, y as u16, row, Style::default());
            }
            buf
        };
        (build(a), build(b))
    }

    #[test]
    fn an_unchanged_overlay_frame_writes_nothing() {
        // THE fix (docs/overlay-repaint.md): a terminal drops the user's mouse
        // selection the moment the cells under it are rewritten, and the
        // overlay used to re-serialize every cell of the screen up to 120
        // times a second while a turn streamed — so text in Ctrl+O / Ctrl+D
        // could not be copied until the turn ended. An identical frame must
        // now put NOTHING on the wire.
        let (prev, next) = frames(&["hello", "world"], &["hello", "world"], 5);
        assert!(matches!(
            overlay_paint(Some(&prev), &next),
            OverlayPaint::Unchanged
        ));
    }

    #[test]
    fn an_overlay_frame_emits_only_the_cells_that_changed() {
        // A streaming Ctrl+O tail-follows, so its live tail genuinely moves —
        // but the rows above it (where a reader has scrolled back and selected)
        // must stay untouched.
        let (prev, next) = frames(&["hello", "world"], &["hello", "WORLD"], 5);
        let OverlayPaint::Diff(updates) = overlay_paint(Some(&prev), &next) else {
            panic!("a changed frame diffs");
        };
        assert_eq!(updates.len(), 5, "only the second row's cells");
        assert!(
            updates.iter().all(|(_, y, _)| *y == 1),
            "row 0 is never rewritten, so a selection on it survives"
        );
    }

    #[test]
    fn the_first_overlay_frame_paints_in_full() {
        // `enter_overlay` queues a Clear(All) and drops the baseline: there is
        // nothing on the alternate screen to diff against.
        let (_, next) = frames(&[], &["hello"], 5);
        assert!(matches!(overlay_paint(None, &next), OverlayPaint::Full));
    }

    #[test]
    fn a_resized_overlay_paints_in_full() {
        // The emulator reflowed the alternate screen out from under the
        // baseline — and `Buffer::diff` needs matching areas anyway. (The
        // same-size case `resized` also has to answer is not visible here:
        // it drops the baseline itself, which lands on the `None` arm above.)
        let prev = Buffer::empty(Rect::new(0, 0, 5, 2));
        let next = Buffer::empty(Rect::new(0, 0, 8, 2));
        assert!(matches!(
            overlay_paint(Some(&prev), &next),
            OverlayPaint::Full
        ));
    }

    #[test]
    fn an_overlay_diff_skips_the_cells_shadowed_by_a_wide_glyph() {
        // The wide-glyph rule every full-cell paint owes (docs/table-streaming.md
        // *Wide glyphs*): emitting a shadow cell prints its space one column
        // past the glyph and drifts the rest of the row. `Buffer::diff` skips
        // them itself — this pins that the diff path inherits it.
        let (prev, next) = frames(&["ab"], &["\u{1f525}b"], 3);
        let OverlayPaint::Diff(updates) = overlay_paint(Some(&prev), &next) else {
            panic!("a changed frame diffs");
        };
        assert!(
            !updates.iter().any(|(x, _, _)| *x == 1),
            "the shadowed column is never emitted: {updates:?}"
        );
    }

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
