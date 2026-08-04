//! Putting something in front of the user: committed to scrollback, or shown
//! as a transient toast.
//!
//! **Invariant 4 lives here.** [`Session::commits_allowed`] is the single
//! predicate every commit site consults: two inline views hold finished lines
//! back — the alternate-screen overlays (Ctrl+O / `/resume` / Ctrl+D) and an
//! open agent session view — and both end the same way. `App` records the item
//! regardless, and the return repaints the inline view from that history, so
//! nothing is lost by waiting. An open **permission prompt** is deliberately
//! *not* on the list: its region sits at the bottom like any other, so a cell
//! resolving beneath it simply scrolls in above the question — visible at once,
//! Claude-Code style (`docs/permissions.md`).
//!
//! Everything that commits mid-turn flushes the in-flight streamed segment
//! first ([`App::flush_streaming_segment`]), which is what makes a notice slot
//! *after* the text it interrupted in both scrollback and history — the same
//! ordering trick a tool call uses (invariant 4).
//!
//! The alternative to committing is [`Session::toast`]: a one-row,
//! self-clearing line above the box for confirmations and soft rejections the
//! user should see but never keep (`docs/toast.md`). It never enters history.
//!
//! [`App::flush_streaming_segment`]: alter_zero::app::App::flush_streaming_segment

use std::time::{Duration, Instant};

use ratatui::text::Line;

use alter_zero::app::{AgentGroup, App, Role, ToastKind, ToolCall, View};
use alter_zero::ui;

use super::Session;

/// How long a transient toast stays above the box before it self-clears. See
/// `docs/toast.md`.
const TOAST_TTL: Duration = Duration::from_secs(4);

impl Session<'_> {
    /// Whether finished lines may be written to the terminal's scrollback right
    /// now (invariant 4 — see the module doc).
    pub(crate) fn commits_allowed(&self) -> bool {
        self.app.view == View::Conversation && self.app.agent_view.is_none()
    }

    /// Raise a transient toast and arm its expiry: set the text, stamp the
    /// deadline `TOAST_TTL` out, and ask the frame scheduler for a draw then —
    /// so it shows now (the caller schedules that frame) and self-clears later
    /// even with no turn active. The draw tick clears it when due and re-arms
    /// while it lingers. See `docs/toast.md`.
    pub(crate) fn toast(&mut self, text: impl Into<String>, kind: ToastKind) {
        self.app.show_toast(text, kind);
        self.toast_deadline = Some(Instant::now() + TOAST_TTL);
        self.frame.schedule_frame_in(TOAST_TTL);
    }

    /// Commit a one-off notice to scrollback + history, finalising any in-flight
    /// streamed segment first so the notice slots in order — the same flush
    /// trick a tool call uses. The shared body of [`Session::commit_error_notice`]
    /// and [`Session::commit_system_notice`] (they differ only in role and
    /// recorder). Only reached in the conversation view, so it never writes the
    /// alternate screen.
    fn commit_notice(&mut self, role: Role, record: fn(&mut App, &str), text: &str) {
        let width = self.term.screen().width;
        let committing = self.app.agent_view.is_none();
        if let Some(segment) = self.app.flush_streaming_segment()
            && committing
        {
            self.term.insert_before(self.render.finish(&segment, width));
            self.term.insert_before(vec![Line::default()]);
            self.render.reset();
        }
        record(&mut self.app, text);
        if committing {
            self.term
                .insert_before(ui::message_lines(role, text, width));
            self.term.insert_before(vec![Line::default()]);
        }
    }

    /// Commit a red error notice (a Ctrl+V clipboard failure, a failed
    /// `/resume` load) — [`Session::commit_notice`] as [`Role::Error`]. See
    /// `docs/image-paste.md`.
    pub(crate) fn commit_error_notice(&mut self, text: &str) {
        self.commit_notice(Role::Error, App::record_error_message, text);
    }

    /// Commit a cyan system notice — `/help`'s command list —
    /// [`Session::commit_notice`] as [`Role::System`].
    pub(crate) fn commit_system_notice(&mut self, text: &str) {
        self.commit_notice(Role::System, App::record_system_message, text);
    }

    /// Commit a dead turn's remains to scrollback — the kept partial reply, the
    /// tool the death resolved as failed, the agent group it interrupted, then
    /// the red notice, each with a trailing blank spacer — after reseating the
    /// viewport to its idle height (the `StreamDone` dance, so the box stays
    /// flush at the bottom as the streaming strip clears). The shape shared by
    /// the Esc interrupt ([`App::interrupt_turn`]) and a backend
    /// `StreamEvent::Error` ([`App::fail_stream`]); the caller guarantees the
    /// conversation view.
    ///
    /// `notice` is `None` for a `!` shell interrupt, whose `⎿ Interrupted by
    /// user` cell (the `tool`) already says it — committing a second
    /// `Conversation interrupted` line would be redundant (`docs/interrupt.md`,
    /// req 2). A backend error always passes `Some` (the error text is its
    /// terminal notice).
    ///
    /// [`App::interrupt_turn`]: alter_zero::app::App::interrupt_turn
    /// [`App::fail_stream`]: alter_zero::app::App::fail_stream
    pub(crate) fn commit_turn_failure(
        &mut self,
        partial: Option<String>,
        tool: Option<ToolCall>,
        agents: Option<AgentGroup>,
        notice: Option<&str>,
    ) {
        let width = self.term.screen().width;
        let height = self.live_region_height();
        self.term.set_view_height(height);
        if let Some(partial) = partial {
            self.term.insert_before(self.render.finish(&partial, width));
            self.term.insert_before(vec![Line::default()]);
        }
        if let Some(tool) = tool {
            self.term.insert_before(ui::tool_lines(&tool, width));
            self.term.insert_before(vec![Line::default()]);
        }
        // The agent group the death resolved (its members marked interrupted) —
        // the red tree cell, before the notice (docs/agent-tool.md).
        if let Some(group) = agents {
            self.term
                .insert_before(ui::agent_group_lines(&group, width));
            self.term.insert_before(vec![Line::default()]);
        }
        if let Some(notice) = notice {
            self.term
                .insert_before(ui::message_lines(Role::Error, notice, width));
            self.term.insert_before(vec![Line::default()]);
        }
    }
}
