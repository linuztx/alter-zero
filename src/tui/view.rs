//! Painting: the inline live region, the alternate-screen overlays, and the
//! repaints that rebuild the conversation from source.
//!
//! Two pure decisions from `ui` drive everything here, and this module is only
//! their boundary half:
//!
//! - **How tall the region is.** [`Session::live_region_height`] asks the same
//!   question the next draw will, which is why a post-stream commit can reseat
//!   the viewport *before* flushing (invariant 3 — otherwise the box rises off
//!   the bottom and leaves blank rows beneath it).
//! - **When the screen rebuilds.** A full rebuild (`InlineViewport::reflow`)
//!   purges the terminal's scrollback and repaints the whole conversation
//!   from history — every resize, `/clear`, and history rewind takes it. An
//!   ordinary overlay return does **not**: everything that committed while
//!   the overlay covered the screen sits in the viewport's pending queue, so
//!   one ordinary draw flushes it above the live region and the terminal's
//!   own scrollback survives untouched ([`Session::overlay_return_repaint`]).
//!   Only a resize that landed under the overlay forces the purge (the
//!   emulator reflowed the main screen underneath, and the queued lines were
//!   rendered at the stale width).
//!
//! A repaint must not lose the in-flight partial reply — it lives in the
//! streaming buffer, not `history` — so the rebuild re-commits it through the
//! normal `insert_before` pipeline right after the reflow (`docs/flicker.md`).
//!
//! The clock injections live here too ([`Session::update_status_times`]): time
//! is impure, so the pure `App`/`ui` only ever see already-computed durations.

use std::io;
use std::time::Instant;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::text::Line;

use alter_zero::app::{App, DebugPage, View};
use alter_zero::paste;
use alter_zero::ui;

use super::Session;

/// How often the live status re-arms its next animation frame while a turn is in
/// flight (~30 fps — codex's status-widget cadence): drives the verb's shimmer
/// sweep and keeps the timer advancing through event-less pauses.
pub(crate) const STATUS_FRAME_INTERVAL: std::time::Duration = std::time::Duration::from_millis(32);

/// How often an open `/login` device page redraws — the granularity of the
/// `mm:ss` countdown that is its only moving part. See `docs/copilot.md`.
const DEVICE_TICK_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);

/// Upper bound on the rows a purge rebuild (`/clear`, resize, a history
/// rewind) re-renders into scrollback. A purge drops the terminal's own
/// scrollback, so the whole conversation is rebuilt from history — capped here
/// so a pathologically long conversation can't turn one resize into an
/// unbounded write. codex bounds its resize reflow the same way (per-terminal
/// 1k–10k rows); 10k is effectively "everything" for any real conversation.
pub(crate) const RESIZE_REFLOW_MAX_ROWS: usize = 10_000;

impl Session<'_> {
    /// The live region's height for the current state and screen size — exactly
    /// what the next [`Session::draw_conversation`] will use. Shared so a
    /// post-stream commit can reserve that same idle height before flushing the
    /// final lines (see `InlineViewport::set_view_height`).
    pub(crate) fn live_region_height(&self) -> u16 {
        live_region_height(&self.app, self.term.screen())
    }

    /// Write the live status's times onto `App` before a draw: how long the turn
    /// has run (whole seconds for display; sub-second for the shimmer phase) and
    /// the current thinking-phase duration (`Some` while thinking). Time is
    /// impure, so this is the boundary's job — the pure `App`/`ui` only ever see
    /// the already-computed values. No-op when no turn is in flight.
    pub(crate) fn update_status_times(&mut self) {
        let elapsed = self
            .clocks
            .turn_start
            .map_or(std::time::Duration::ZERO, |start| start.elapsed());
        let thinking = self.clocks.thinking_start.map(|start| start.elapsed());
        self.app.set_status_times(elapsed, thinking);
        // The current running command's own elapsed (None when none is
        // running), gating the delayed Ctrl+B hint (docs/background.md).
        self.app
            .set_command_elapsed(self.clocks.command_start.map(|start| start.elapsed()));
        // The animation phase for the live region's pulsing bullets — a phase,
        // not a measurement: nothing displays it (docs/tool-pulse.md).
        self.app.set_pulse(self.clocks.loop_start.elapsed());
        // How long a `/login` device code has left, when one is on screen
        // (docs/copilot.md).
        if self.app.device_login_active() {
            let now = Instant::now();
            self.app.set_device_remaining(
                self.device_expires
                    .map(|at| at.saturating_duration_since(now)),
            );
        }
    }

    /// Schedule the redraw for a just-handled key. A plain typed character that
    /// lands in a `PasteBurst` asks for a *relaxed* frame a beat out instead of
    /// an immediate one; every other key (and the first characters of a run)
    /// paints at once. The scheduler keeps the soonest pending deadline and its
    /// rate limiter caps everything at 120 fps — that floor, not the burst
    /// branch, is what coalesces a paste run into a few paints (see
    /// `alter_zero::paste`).
    pub(crate) fn schedule_for_key(&mut self, key: &KeyEvent) {
        let plain_char =
            matches!(key.code, KeyCode::Char(_)) && !key.modifiers.contains(KeyModifiers::CONTROL);
        if !plain_char {
            self.burst.reset(); // a navigation/submit key ends any burst
        }
        if plain_char && self.burst.note_char(std::time::Instant::now()) {
            self.frame.schedule_frame_in(paste::BURST_CHAR_INTERVAL);
        } else {
            self.frame.schedule_frame();
        }
    }

    /// The framed-view rows that must be **flowed** into scrollback for the
    /// current state — a screen-tall `/mcp`/`/hooks`/`/trust` page's top —
    /// or `None` when nothing flows (`ui::view_flow`, `docs/view-flow.md`).
    /// Capped like every purge-rebuild write.
    pub(crate) fn current_view_flow(&self) -> Option<ui::ViewFlow> {
        let screen = self.term.screen();
        ui::view_flow(
            &self.app,
            screen.width,
            screen.height,
            RESIZE_REFLOW_MAX_ROWS,
        )
    }

    /// Whether the flow committed to scrollback no longer matches the current
    /// state — the page navigated, the search narrowed, the menu closed, a
    /// modal covered it — so this draw tick must purge-rebuild instead of
    /// painting in place: the rebuild re-establishes the current flow, or
    /// clears it and returns the composer (`docs/view-flow.md`). Cheap when
    /// no framed view is open and nothing is flowed (both sides `None`).
    pub(crate) fn view_flow_stale(&self) -> bool {
        let screen = self.term.screen();
        ui::view_flow_signature(
            &self.app,
            screen.width,
            screen.height,
            RESIZE_REFLOW_MAX_ROWS,
        ) != self.flowed_view
    }

    /// Whether this draw tick must purge-rebuild the conversation instead of
    /// painting the live region in place — the boundary read behind the pure
    /// `ui::modal_needs_rebuild`: the just-closed prompt's noted one-way move,
    /// or the still-open prompt about to seat short of the screen bottom it was
    /// painted flush against (a batch's back-to-back prompts of different
    /// heights — see the draw tick's rebuild arm and `docs/permissions.md`).
    pub(crate) fn modal_rebuild_due(&self) -> bool {
        ui::modal_needs_rebuild(
            ui::region_is_modal(&self.app),
            self.term.modal_scrolled(),
            self.term.painted_bottom(),
            self.term.view_top(),
            self.term.pending_rows(),
            self.live_region_height(),
            self.term.screen().height,
        )
    }

    /// The strip's streaming preview: **the rows scrollback does not hold
    /// yet** — the uncommitted tail of the render, which is the reply's last
    /// row for settled prose, every wrapped row of a withheld source line, the
    /// whole forming table, or nothing at all when the frontier just committed
    /// clean (capped to `ui::stream_preview_max_rows` so a tall tail
    /// tail-follows on a small screen) — computed cheaply by
    /// `ui::StreamRender::preview`; `None` when idle or while a tool runs (the
    /// tool's own header previews instead).
    /// Called before every conversation-view draw so the status animation never
    /// pays to re-render the whole reply, and **injects the row count into
    /// `App`** (`App::set_stream_preview_rows`) so `ui::preview_rows` — and with
    /// it [`Session::live_region_height`], the strip layout, and the cursor seat
    /// — reserve exactly the rows the strip draws. See `docs/markdown.md`,
    /// `docs/table-streaming.md`.
    ///
    /// **Whose reply** is the view's question, not the loop's: with an agent
    /// session view open the screen shows that agent's conversation, so the
    /// frontier comes from its buffer through `agent_render` — the very
    /// render `Session::commit_agent_view_event` commits with, which is what
    /// makes `committed ++ preview` the reply there too. Previewing the main
    /// buffer instead (or, as before, not at all) left a subagent's forming
    /// table on screen nowhere: withheld by the commit, one row of it in the
    /// strip (`docs/agent-view-streaming.md`).
    fn stream_preview_lines(&mut self) -> Option<Vec<Line<'static>>> {
        let screen = self.term.screen();
        let max_rows = ui::stream_preview_max_rows(&self.app, screen.width, screen.height);
        let preview = match self.app.viewed_agent() {
            Some(run) => run
                .streaming
                .clone()
                .filter(|text| !text.is_empty() && run.tool_queue.is_empty())
                .map(|text| self.agent_render.preview(&text, screen.width, max_rows)),
            None => match self.app.streaming_text() {
                Some(text) if !text.is_empty() && self.app.current_tool().is_none() => {
                    Some(self.render.preview(text, screen.width, max_rows))
                }
                _ => None,
            },
        };
        self.app.set_stream_preview_rows(
            preview
                .as_ref()
                .map_or(1, |p| u16::try_from(p.len()).unwrap_or(u16::MAX)),
        );
        preview
    }

    /// Render the live region at its current grown height and place the cursor.
    /// The composer keeps its cursor even while a reply streams (codex-style —
    /// typing mid-turn edits the draft, Enter queues it); only the overlays hide
    /// it (`enter_overlay`).
    ///
    /// A permission prompt needs no branch of its own: the height is its
    /// `ui::permission_height` and `render_live` paints it in the composer's
    /// place, so the region simply grows to the prompt like any other view — the
    /// chat above it scrolling into real scrollback (`InlineViewport` notes that
    /// one-way move for the close's purge rebuild; `docs/permissions.md`).
    pub(crate) fn draw_conversation(&mut self) -> io::Result<()> {
        // Compute the strip preview cheaply once per frame (O(one line); a
        // forming table re-renders just its own block), so the status animation
        // never re-renders the whole reply — and inject its row count so the
        // layout reserves it. See `docs/markdown.md`, `docs/table-streaming.md`.
        let preview = self.stream_preview_lines();
        let height = self.live_region_height();
        let app = &self.app;
        // `term` places the cursor from the final (content-anchored) viewport
        // via `ui::cursor_position`, which mirrors render_live's layout exactly.
        self.term.draw(
            height,
            |area, buf| ui::render_live_with_preview(area, buf, app, preview.as_deref()),
            app,
        )
    }

    /// Render the full-screen tool-output overlay. Clamps the scroll to the
    /// current screen first (so the last line can reach the bottom but not
    /// scroll past it), then paints the view onto the alternate screen.
    pub(crate) fn draw_tool_view(&mut self) -> io::Result<()> {
        let screen = self.term.screen();
        // An agent session view's Ctrl+O shows the *viewed agent's* transcript —
        // a fresh, bounded build (docs/agent-tool.md); the main cache below is
        // untouched, so the ordinary open stays warm.
        if let Some(lines) = ui::agent_transcript_lines(&self.app, screen.width) {
            let max = ui::tool_view_max_scroll_for(lines.len(), screen.height);
            self.app.settle_tool_scroll(max);
            let app = &self.app;
            return self
                .term
                .draw_overlay(|area, buf| ui::render_tool_view(area, buf, app, &lines));
        }
        // A backtrack preview open/step requested a scroll to its highlighted
        // message (docs/backtrack.md): apply the pure decision once — consumed,
        // so it never fights the user's own scrolling — before the normal clamp.
        if self.app.take_backtrack_scroll() {
            let selection = self.transcript.selection(&self.app, screen.width);
            if let Some(scroll) =
                ui::backtrack_scroll_for(selection, self.app.tool_scroll, screen.height)
            {
                self.app.apply_backtrack_scroll(scroll);
            }
        }
        // Build the transcript at most once here (the cache skips even that
        // while the user only scrolls): the same cached lines feed the clamp and
        // the render, so a scroll keypress no longer re-highlights all of
        // history (twice).
        let max = ui::tool_view_max_scroll_for(
            self.transcript.line_count(&self.app, screen.width),
            screen.height,
        );
        self.app.settle_tool_scroll(max);
        let lines = self.transcript.lines(&self.app, screen.width);
        let app = &self.app;
        self.term
            .draw_overlay(|area, buf| ui::render_tool_view(area, buf, app, lines))
    }

    /// Render the full-screen `/resume` session picker onto the alternate screen
    /// (the transcript overlay's twin). See `docs/resume.md`.
    pub(crate) fn draw_resume_picker(&mut self) -> io::Result<()> {
        let app = &self.app;
        self.term
            .draw_overlay(|area, buf| ui::render_resume_picker(area, buf, app))
    }

    /// Render the Ctrl+D context-debug view onto the alternate screen — the
    /// transcript pager's raw-context sibling: settle the scroll against the
    /// current screen, then paint. The window comes from the loop-owned
    /// [`ui::ContextCache`] — the view redraws every animation frame while a
    /// turn runs, and rebuilding the O(conversation) derivation per frame
    /// starved the scroll keys on a big context (the "Ctrl+D freezes" bug) —
    /// so the count feeding the clamp and the render share one build. See
    /// `docs/context.md`.
    pub(crate) fn draw_context_view(&mut self) -> io::Result<()> {
        // Tab's other page is a different body under the same chrome, and it
        // is built fresh from the backend rather than served from the cache
        // (`docs/permissions.md`).
        if self.app.debug_page == DebugPage::Classifier {
            return self.draw_classifier_page();
        }
        let screen = self.term.screen();
        let max = ui::tool_view_max_scroll_for(
            self.context.line_count(&self.app, screen.width),
            screen.height,
        );
        self.app.settle_debug_scroll(max);
        let lines = self.context.lines(&self.app, screen.width);
        let app = &self.app;
        self.term
            .draw_overlay(|area, buf| ui::render_context_view(area, buf, app, lines))
    }

    /// Render the Ctrl+D view's **classifier page** (Tab) onto the alternate
    /// screen — the same chrome, a different body (`docs/permissions.md`).
    ///
    /// The block is pulled from the **backend** per draw rather than cached:
    /// it grows as the turn runs (every tool call appends a line) and rolls
    /// its windows as the conversation goes on, so a page left open must show
    /// the log as it lands. Cheap by construction — the context is bounded to
    /// `CONTEXT_MAX_REQUESTS` + `CONTEXT_MAX_ACTIONS` short lines, which is
    /// why this needs no `ContextCache` sibling.
    fn draw_classifier_page(&mut self) -> io::Result<()> {
        // Whose review log? An open agent session view debugs the **viewed
        // agent's** run, so its classifier page must show that agent's own
        // context — its launch prompt and its own calls, the block its own
        // verdicts were actually reviewed against. The lead's log describes
        // decisions nobody on that screen made. (The other Ctrl+D page has
        // always swapped this way — `ui::context_lines`' first branch keys on
        // `viewed_agent` — this one simply never did.) An agent the registry
        // no longer holds yields `None`, which renders as the page's empty
        // placeholder rather than silently falling back to the lead.
        // See `docs/permissions.md` / `docs/agent-tool.md`.
        let context = match self.app.agent_view.as_deref() {
            Some(id) => self.agent_registry.classifier_context(id),
            None => self.models.backend().classifier_context(),
        };
        self.app.set_classifier_context(context);
        let screen = self.term.screen();
        let lines = ui::classifier_lines(&self.app, screen.width);
        let max = ui::tool_view_max_scroll_for(lines.len(), screen.height);
        self.app.settle_classifier_scroll(max);
        let app = &self.app;
        self.term
            .draw_overlay(|area, buf| ui::render_context_view(area, buf, app, &lines))
    }

    /// Paint whichever view is current — the draw tick's whole body.
    pub(crate) fn draw_active_view(&mut self) -> io::Result<()> {
        match self.app.view {
            // The modal (permission-prompt) region needs a purge rebuild
            // instead of an in-place paint — two cases, one pure decision
            // (`ui::modal_needs_rebuild`, `docs/permissions.md`). The prompt
            // just CLOSED after the screen moved one-way under it — its own
            // growth scrolled chat into scrollback (the ordinary case), a commit
            // beneath it scrolled, or a rebuild reseated it (a mid-prompt
            // resize's purge, an overlay return whose prompt opened
            // underneath): the plain collapse cannot refill what moved. Or the
            // prompt is still OPEN and this frame would SHRINK the region seated
            // at the screen bottom — a batch's back-to-back prompts of different
            // heights (the tall body-capped one answered, a short one opening as
            // the resolved cell commits out of the live region), which used to
            // strand the open prompt above a band of blank rows until it was
            // answered. Purge-rebuild instead: region flush at the bottom,
            // scrollback rebuilt from history, nothing lost or doubled — and a
            // rebuild under a still-open prompt re-arms the note
            // (`InlineViewport::reflow`), so its eventual close still purges.
            // (An open agent session view rebuilds itself —
            // `repaint_active_view` routes there.) See
            // `InlineViewport::take_modal_scrolled` and `docs/permissions.md`.
            // …or a framed view's FLOW went stale — its page navigated, its
            // menu closed, a modal covered it, or a page that must flow has
            // no flow committed yet: the same purge rebuild re-establishes
            // the current flow, or clears it and returns the composer
            // (`docs/view-flow.md`; the flow check runs first so a prompt
            // opening over a flowed menu cleans the flow in the very rebuild
            // that seats the prompt).
            View::Conversation if self.view_flow_stale() || self.modal_rebuild_due() => {
                let _ = self.term.take_modal_scrolled();
                self.repaint_active_view()
            }
            View::Conversation => self.draw_conversation(),
            View::ToolOutput => self.draw_tool_view(),
            View::ResumePicker => self.draw_resume_picker(),
            View::ContextDebug => self.draw_context_view(),
        }
    }

    /// Rebuild whatever the inline screen is showing — the **agent session
    /// view** when one is open (a purge rebuild of the agent's transcript),
    /// else the main conversation. The shared resize / rewind repaint
    /// (`docs/agent-tool.md`).
    pub(crate) fn repaint_active_view(&mut self) -> io::Result<()> {
        if self.app.agent_view.is_some() {
            self.repaint_agent_view()
        } else {
            self.repaint_conversation()
        }
    }

    /// Catch the inline screen up when an alternate-screen overlay closes.
    ///
    /// Everything that committed while the overlay was up — streamed rows,
    /// tool cells, thought cells, queued user bubbles, the turn's summary —
    /// sits in the viewport's pending queue (`InlineViewport::insert_before`
    /// only queues, and nothing flushes onto the alternate screen), so the
    /// ordinary [`Session::draw_conversation`] flushes it above the live
    /// region and repaints the box, exactly as if the overlay had never been
    /// up: the terminal's own scrollback survives, and nothing is lost or
    /// doubled. The retired history-window rebuild could re-emit at most one
    /// screenful, which silently dropped the rest of an overlay-covered turn
    /// from the terminal — the scrollback-hole bug.
    ///
    /// Two cases still need the full purge rebuild: a **resize landed under
    /// the overlay** (the emulator reflowed the main screen and the queued
    /// lines were rendered at the stale width — `overlay_resized`, consumed
    /// here so only this return purges), and an open **agent session view**,
    /// which always rebuilds from its own transcript (`docs/agent-tool.md`;
    /// its purge drops the queue, whose lines its history regenerates).
    pub(crate) fn overlay_return_repaint(&mut self) -> io::Result<()> {
        if std::mem::take(&mut self.overlay_resized) || self.app.agent_view.is_some() {
            self.repaint_active_view()
        } else {
            self.draw_conversation()
        }
    }

    /// Rebuild the inline conversation from `App`'s retained history,
    /// re-wrapped to the current width — scrollback purge, tail and live
    /// region in one synchronized frame (`InlineViewport::reflow`). Used
    /// after a resize, on `/clear`, and by every history rewind (a backtrack,
    /// a `/resume` load, an interrupt-undo); an ordinary overlay return
    /// instead just flushes its queued commits
    /// ([`Session::overlay_return_repaint`]).
    ///
    /// The purge drops the terminal's scrollback, so the **whole** history is
    /// repainted (bounded by [`RESIZE_REFLOW_MAX_ROWS`]) — otherwise older
    /// turns would be lost from the purged scrollback.
    ///
    /// A mid-stream rebuild must not lose the in-flight partial reply — it
    /// lives in `App`'s streaming buffer, not `history`. The purge dropped its
    /// committed rows with everything else, so the renderer resets and the
    /// whole partial is re-committed right after via `ui::StreamRender::commit`
    /// — the standard `insert_before` pipeline, landing in the next draw's
    /// synchronized update. (`reflow` cleared the pending queue first, so the
    /// re-commit can never double up with pre-rebuild leftovers.)
    pub(crate) fn repaint_conversation(&mut self) -> io::Result<()> {
        let screen = self.term.screen();
        // The purge drops every committed row (screen and scrollback alike),
        // so nothing is "already committed" any more: reset, and let the
        // catch-up below re-commit the whole partial at the current width.
        self.render.reset();
        // The preview comes FIRST: it injects the strip's preview row count
        // (`set_stream_preview_rows` — a forming table previews multi-row) that
        // `live_region_height` below must reserve (docs/table-streaming.md).
        let preview = self.stream_preview_lines();
        let height = self.live_region_height();
        let tail = ui::repaint_tail(
            // Only what has actually reached scrollback: a parallel MCP run
            // still in flight has cells recorded but held, and the strip is
            // showing them (`docs/mcp.md`).
            ui::committed_history(&self.app.history, self.app.tool_queue()),
            self.app.streaming_text(),
            &mut self.render,
            screen.width,
            RESIZE_REFLOW_MAX_ROWS,
        );
        // Re-emit the header banner atop the rebuilt tail — it lives outside
        // `history` and would otherwise be lost (docs/header.md).
        let mut tail = ui::banner_tail(ui::header_lines(&self.app, screen.width), tail);
        // A screen-tall framed view flows its page top into scrollback: the
        // rows ride the tail's very end, so they land directly above the live
        // region — the page reads whole across the seam. Recomputed here, at
        // the one place every purge rebuild goes through, so a resize or a
        // rewind under an open menu re-flows at the new state for free; the
        // stored signature is what the draw tick compares
        // (`docs/view-flow.md`).
        let flow = self.current_view_flow();
        self.flowed_view = flow.as_ref().map(|flow| flow.signature);
        if let Some(flow) = flow {
            tail.extend(flow.lines);
        }
        let app = &self.app;
        self.term.reflow(
            tail,
            height,
            |area, buf| ui::render_live_with_preview(area, buf, app, preview.as_deref()),
            app,
        )?;
        // Catch scrollback up on the in-flight partial the purge dropped:
        // exactly the rows live streaming would have committed, queued for
        // the next draw. Suppressed while a flow is active — the queued rows
        // would flush between the flowed page and the region, tearing it
        // (commits pause under a flow, `Session::commits_allowed`); the
        // partial stays in the streaming buffer and the first un-flowed
        // rebuild re-commits it whole.
        if self.flowed_view.is_none()
            && let Some(text) = self.app.streaming_text().filter(|text| !text.is_empty())
        {
            let lines = self.render.commit(text, screen.width);
            self.term.insert_before(lines);
        }
        Ok(())
    }

    /// Rebuild the screen as an **agent session view** (`docs/agent-tool.md`):
    /// purge scrollback + screen (the `/clear` shape — the main conversation
    /// returns the same way), then the banner over the agent's own transcript,
    /// with the live region (the agent's strip + the labelled composer + footer +
    /// roster) painted below in the same synchronized frame. The in-flight
    /// partial commits through `agent_render` so later chunks append seamlessly.
    pub(crate) fn repaint_agent_view(&mut self) -> io::Result<()> {
        let screen = self.term.screen();
        self.agent_render.reset();
        let Some(run) = self.app.viewed_agent() else {
            return Ok(());
        };
        // Only the agent's committed cells, the main view's rule
        // (`ui::committed_history`, `docs/mcp.md`).
        let history = ui::committed_history(&run.history, &run.tool_queue).to_vec();
        let streaming = run.streaming.clone().filter(|text| !text.is_empty());
        // The preview comes FIRST, as in `repaint_conversation`: it injects
        // the strip's row count (`set_stream_preview_rows`) that
        // `live_region_height` below must reserve — a rebuild that skipped it
        // squeezed a multi-row frontier back to one row for that frame
        // (`docs/agent-view-streaming.md`).
        let preview = self.stream_preview_lines();
        let height = self.live_region_height();
        let mut tail = ui::conversation_lines(&history, screen.width);
        if let Some(text) = &streaming {
            tail.extend(self.agent_render.committed_rows(text, screen.width));
        }
        let mut tail = ui::banner_tail(ui::header_lines(&self.app, screen.width), tail);
        // A framed view opened from the agent view's own palette renders over
        // it and flows the same way (`docs/view-flow.md`) — the rebuild is
        // this view's, so the flow rides this tail.
        let flow = self.current_view_flow();
        self.flowed_view = flow.as_ref().map(|flow| flow.signature);
        if let Some(flow) = flow {
            tail.extend(flow.lines);
        }
        let app = &self.app;
        self.term.reflow(
            tail,
            height,
            |area, buf| ui::render_live_with_preview(area, buf, app, preview.as_deref()),
            app,
        )?;
        // Catch scrollback up on the agent's in-flight partial the purge
        // dropped — `repaint_conversation`'s catch-up over `agent_render`, so
        // a rebuild mid-reply leaves the same rows behind here as it does in
        // the main view (suppressed under a flow, for the same reason).
        if self.flowed_view.is_none()
            && let Some(text) = streaming.as_deref()
        {
            let lines = self.agent_render.commit(text, screen.width);
            self.term.insert_before(lines);
        }
        Ok(())
    }
}

/// The live region's height for `app` at the current screen size — exactly what
/// the next draw will use.
///
/// A free function (not a [`Session`] method) because the commit paths call it
/// while `app` is already borrowed out of `self`.
fn live_region_height(app: &App, screen: ratatui::layout::Rect) -> u16 {
    // The `AskUserQuestion` modal replaces the whole region — checked first,
    // like its render branch (the two modals queue, so at most one is open;
    // docs/ask.md).
    if let Some(height) = ui::ask_height(app, screen.width, screen.height) {
        return height;
    }
    // A pending tool-permission request replaces the whole region — the
    // streaming strip included, since the turn is blocked on the answer. It is
    // modal, so it is checked first (docs/permissions.md).
    if let Some(height) = ui::permission_height(app, screen.width, screen.height) {
        return height;
    }
    // The inline `/model` picker, `/login` flow, `/settings` menu and ↓
    // background manager each replace the **composer** with their own framed
    // body — never the streaming strip, whose rows they reserve above
    // themselves so opening one mid-turn can't hide the running turn (see
    // docs/llm.md / docs/settings.md / docs/background.md); the open one's
    // height stands in for the composer's.
    if let Some(height) = ui::model_picker_height(app, screen.width, screen.height) {
        return height;
    }
    if let Some(height) = ui::key_onboarding_height(app, screen.width, screen.height) {
        return height;
    }
    // The inline `/settings` menu, likewise (docs/settings.md).
    if let Some(height) = ui::settings_height(app, screen.width, screen.height) {
        return height;
    }
    // The inline `/mascot` picker, likewise (docs/mascot.md).
    if let Some(height) = ui::mascot_picker_height(app, screen.width, screen.height) {
        return height;
    }
    // The inline `/skills` menu, likewise (docs/skills.md).
    if let Some(height) = ui::skills_menu_height(app, screen.width, screen.height) {
        return height;
    }
    // The read-only `/hooks` menu, likewise (docs/hooks-menu.md).
    if let Some(height) = ui::hooks_menu_height(app, screen.width, screen.height) {
        return height;
    }
    // The `/trust` review menu, likewise (docs/project-config.md).
    if let Some(height) = ui::trust_menu_height(app, screen.width, screen.height) {
        return height;
    }
    // The `/mcp` manager, likewise (docs/mcp.md).
    if let Some(height) = ui::mcp_menu_height(app, screen.width, screen.height) {
        return height;
    }
    if let Some(height) = ui::background_view_height(app, screen.width, screen.height) {
        return height;
    }
    let band = ui::band_rows(app, screen.width);
    ui::live_height(
        &app.input,
        screen.width,
        screen.height,
        ui::strip_has_status(app),
        ui::fitted_preview_rows(app, screen.width, screen.height),
        ui::task_rows(app, screen.width),
        ui::queued_rows(app, screen.width),
        ui::toast_rows(app),
        band,
        ui::footer_rows(app, band),
        ui::agent_list_rows(app),
    )
}

impl Session<'_> {
    /// Ctrl+O: `App::on_key` already flipped the view — sync the overlay. The
    /// transcript cache is deliberately RETAINED across the close
    /// (`docs/tool-view-performance.md`): its frozen prefix is what makes the
    /// next Ctrl+O instant, and the loop-bottom `transcript.warm` keeps appending
    /// to it as items commit.
    pub(crate) fn toggle_tool_view(&mut self) -> io::Result<()> {
        if self.app.view == View::ToolOutput {
            self.term.enter_overlay()?;
            // Paint the overlay now rather than on the next tick — the
            // freshly-cleared alt screen would show as a black flash for a frame
            // otherwise.
            self.draw_tool_view()
        } else {
            self.term.exit_overlay()?;
            // Catch the inline view up on whatever committed — or was
            // dispatched off the queue — while the overlay was showing: it all
            // sits in the pending queue, and this draw flushes it above the
            // live region, keeping the terminal's own scrollback (invariant 4
            // / Phase 7). A resize under the overlay forces the purge-rebuild
            // every resize gets, and an open agent session view rebuilds
            // itself (docs/agent-tool.md).
            self.overlay_return_repaint()
        }
    }

    /// Ctrl+D: the raw-context view — the same overlay dance as Ctrl+O
    /// (`docs/context.md`).
    pub(crate) fn toggle_context_debug(&mut self) -> io::Result<()> {
        if self.app.view == View::ContextDebug {
            self.term.enter_overlay()?;
            self.draw_context_view()
        } else {
            // Return the rendered window's memory with the view: the cache
            // makes redraws cheap while it is up, but kept past the close it
            // is a second full rendered copy of the conversation resident
            // for the rest of the session. The next Ctrl+D rebuilds once.
            self.context.release();
            // …and the classifier page's injected block, which the next open
            // refreshes from the backend anyway.
            self.app.set_classifier_context(None);
            self.term.exit_overlay()?;
            self.overlay_return_repaint()
        }
    }
}

impl Session<'_> {
    /// A coalesced draw tick: refresh the injected times, expire the toast, sweep
    /// the agent roster, paint — then, while a turn is in flight, **re-arm** the
    /// next animation frame (codex's status-widget pattern) so the verb's shimmer
    /// sweeps and the timer advances even when no reply event arrives (a tool run,
    /// a thinking pause). The chain seeds from the Submit keypress and stops by
    /// itself on the first draw after the turn ends.
    pub(crate) fn on_draw_tick(&mut self) -> io::Result<()> {
        self.update_status_times();
        // Inject each background shell's runtime (the boundary owns the started
        // clocks — docs/background.md), so the manager's details view ticks.
        for (id, started) in &self.bg_clocks {
            self.app.set_background_runtime(id, started.elapsed());
        }
        // …and each running agent's, plus the linger sweep (docs/agent-tool.md).
        self.tick_agent_roster();
        self.expire_toast();
        self.draw_active_view()?;
        // The ↓ manager band re-arms frames like an active turn: its details
        // view's Runtime ticks with no events otherwise — and so does a non-empty
        // agent roster (its elapsed counters tick, and the linger sweep needs the
        // frames to fire). None of that re-arms under an alternate-screen
        // overlay, where none of it is on screen and a repaint for no reason
        // costs the user their text selection ([`App::wants_animation_frames`],
        // `docs/overlay-repaint.md`); the chain re-seeds on the first draw after
        // the return.
        if self.app.wants_animation_frames() {
            self.frame.schedule_frame_in(STATUS_FRAME_INTERVAL);
        }
        // The `/login` device page keeps its own, much slower chain. Its only
        // moving part is an `mm:ss` countdown, so a frame a second is exactly
        // the rate its content changes at — and every frame costs a cursor
        // Hide plus a re-seat, which at the status chain's thirty a second is
        // what a terminal with a cursor-trail animation renders as a permanent
        // shimmer over the page (`docs/copilot.md`).
        if self.app.device_login_active() {
            self.frame.schedule_frame_in(DEVICE_TICK_INTERVAL);
        }
        Ok(())
    }

    /// Expire the transient toast when its deadline passes (so this very frame
    /// paints without it); while it still lingers, keep a frame pending for the
    /// eventual clear — a coalesced keystroke frame can consume the one
    /// [`Session::toast`] scheduled, so re-arming here guarantees the clear fires.
    /// See `docs/toast.md`.
    fn expire_toast(&mut self) {
        if let Some(deadline) = self.toast_deadline {
            let now = std::time::Instant::now();
            if now >= deadline {
                self.app.clear_toast();
                self.toast_deadline = None;
            } else {
                self.frame.schedule_frame_in(deadline - now);
            }
        }
    }
}
