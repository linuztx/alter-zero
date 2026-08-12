//! What a terminal event asks the loop to do.
//!
//! `App::on_key` is pure: it updates the composer, moves a selection, decides —
//! and returns an [`Action`] describing the I/O it cannot do itself. This module
//! is the other half: [`Session::on_terminal_event`] routes each key press,
//! resize and bracketed paste, and [`Session::on_action`] is the routing table
//! from an `Action` to the method that performs it.
//!
//! Most arms are one line, because the work lives in the area that owns it —
//! `turn`, `agent`, `background`, `permission`, `models`, `resume`, `view`. That
//! is the point: **adding a feature means adding a method there and one line
//! here**, not editing a 900-line match.
//!
//! The arms that do stay here are the ones with no other home: quitting, the
//! `/clear` reset (which reaches into every subsystem at once, deliberately),
//! `/copy`'s clipboard write, a system notice, and a toast.
//!
//! Every key press ends with [`Session::after_key`] — schedule the frame, re-derive
//! the `@` picker's query, delete any temp image the key discarded.

use ratatui::crossterm::event::{Event, KeyEventKind};

use alter_zero::app::{
    Action, CHECKPOINT_REWOUND_NOTICE, COPY_EMPTY_NOTICE, COPY_OK_NOTICE, ToastKind, View,
};
use alter_zero::checkpoint;
use alter_zero::clipboard;

use super::turn::TurnInput;
use super::{Session, workers};

/// Whether the event loop keeps running after an action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Flow {
    Continue,
    Quit,
}

impl Session<'_> {
    /// Handle one terminal event: a key press (through `App::on_key` and the
    /// action it returns), a resize, or a bracketed paste. Anything else — key
    /// releases, focus, mouse — is ignored.
    pub(crate) fn on_terminal_event(&mut self, event: Event) -> std::io::Result<Flow> {
        match event {
            Event::Key(key) if key.kind == KeyEventKind::Press => {
                let action = self.app.on_key(key);
                let flow = self.on_action(action)?;
                if flow == Flow::Quit {
                    return Ok(Flow::Quit);
                }
                self.after_key(&key);
                Ok(Flow::Continue)
            }
            Event::Resize(width, height) => {
                self.on_resize(width, height)?;
                Ok(Flow::Continue)
            }
            // A real bracketed paste (term::init enables it). A large paste
            // collapses to a `[Pasted Content N chars]` placeholder in the
            // composer, expanded back on send — docs/paste.md.
            Event::Paste(pasted) => {
                self.on_paste(&pasted);
                Ok(Flow::Continue)
            }
            _ => Ok(Flow::Continue),
        }
    }

    /// Perform the I/O an `Action` asks for. One arm per variant; the work lives
    /// in the module that owns the feature (see the module doc).
    fn on_action(&mut self, action: Action) -> std::io::Result<Flow> {
        match action {
            Action::None => {}
            Action::Quit => {
                // Drop back to the main screen before the loop exits if an
                // overlay (the Ctrl+O transcript or the /resume picker) is up,
                // so restore() lands on the chat. A turn may have finished
                // while the overlay was showing — its scrollback commits are
                // queued (invariant 4) — so flush them and repaint the inline
                // view the same way a normal Ctrl+O return does; otherwise
                // restore() lands on the stale live status strip
                // ("Working… (… tokens)") instead of the committed
                // "Done for Ns" summary.
                if self.app.view != View::Conversation {
                    self.term.exit_overlay()?;
                    self.overlay_return_repaint()?;
                }
                return Ok(Flow::Quit);
            }
            Action::Submit(text) => {
                // Drain any Ctrl+V-attached images staged by the submit
                // (docs/image-paste.md) to deliver them on the turn's typed
                // image channel.
                let images = self.app.take_submission_images();
                self.start_turn(TurnInput {
                    texts: vec![text],
                    images,
                });
            }
            Action::PasteImage => {
                // Ctrl+V: the clipboard I/O happens at the boundary (on_key
                // stayed pure) but on a worker thread, not here — the read +
                // decode + PNG encode of a large screenshot takes long enough to
                // freeze the status animations and swallow keystrokes if run
                // inline. The result arrives on the image channel, which attaches
                // it or commits the red notice.
                workers::spawn_image_paste(self.img_tx.clone());
            }
            Action::RunShell(command) => {
                // `!command` from an idle composer: echo it, then run it locally
                // as a turn (docs/shell-command.md). Reuses the streamed-reply
                // channel + inflight handle, so the status strip, Esc-interrupt,
                // and resize repaint all work exactly like an AI turn.
                self.run_shell(command);
            }
            Action::Interrupt => self.interrupt_turn()?,
            Action::Compact => {
                // /compact (docs/compact.md): run codex's summarization turn —
                // the whole current context plus the fixed handoff prompt — with
                // the reply diverted into the compact buffer (never rendered);
                // `finish_compact` appends the marker at StreamDone and the
                // derivation compacts the model's context from there on. The turn
                // rides the normal `inflight` slot so Esc, /clear, and quit reap
                // it like any other turn.
                self.start_compact_turn(/*auto=*/ false);
            }
            Action::Clear => self.clear_conversation()?,
            Action::KillBackground(id) => self.kill_background(&id),
            Action::MoveToBackground => self.move_to_background(),
            Action::StopAgent(id) => self.stop_agent(&id),
            Action::ViewAgent(_) => self.enter_agent_view()?,
            Action::LeaveAgentView => self.leave_agent_view()?,
            Action::AgentChat { id, text } => self.agent_chat(&id, &text),
            Action::ToggleToolView => self.toggle_tool_view()?,
            Action::ToggleContextDebug => self.toggle_context_debug()?,
            Action::ConfirmBacktrack => self.confirm_backtrack()?,
            Action::OpenResumePicker => self.open_resume_picker()?,
            Action::CloseResumePicker => self.close_resume_picker()?,
            Action::ResumeSession(path) => self.resume_session(path)?,
            Action::ResolvePermission { request, decision } => {
                self.resolve_permission(&request, decision);
            }
            Action::ResolveAsk { id, decision } => {
                // Post the user's answers (or decline/chat) on the ask gate,
                // waking the tool thread parked on it — the modal is already
                // closed and the composer draft restored (docs/ask.md).
                self.ask.resolve(&id, decision);
                self.frame.schedule_frame();
            }
            Action::SetPermissionMode(mode) => self.set_permission_mode(mode),
            Action::OpenModelPicker => self.open_model_picker(),
            Action::CloseModelPicker => self.close_model_picker(),
            Action::SelectModel {
                provider,
                id,
                reasoning,
                vision,
                context,
            } => self.select_model(&provider, &id, reasoning, vision, context),
            Action::SetThinking(mode) => self.set_thinking(mode),
            Action::OpenKeyOnboarding => self.open_key_onboarding(),
            Action::CloseKeyOnboarding => {
                // Esc/Ctrl+C dismissed the flow: nothing to reap; the region
                // collapses back to the composer on next draw.
            }
            Action::SaveApiKey {
                provider,
                env_var,
                key,
            } => self.save_api_key(&provider, &env_var, &key),
            Action::OpenSettings => self.open_settings(),
            Action::CloseSettings => {
                // Esc/Ctrl+C dismissed the menu: nothing to reap; the region
                // collapses back to the composer on the next draw.
            }
            Action::SettingChanged(key) => self.apply_setting(key),
            Action::OpenHooksMenu => self.open_hooks_menu(),
            Action::OpenSkillsMenu => self.open_skills_menu(),
            Action::CloseSkillsMenu => {
                // Esc/Ctrl+C dismissed the menu: nothing to reap; the region
                // collapses back to the composer on the next draw.
            }
            Action::SkillToggled { name, enabled } => self.apply_skill_toggle(&name, enabled),
            Action::CloseHooksMenu => {
                // Esc/Ctrl+C dismissed the browser: nothing to reap; the
                // region collapses back to the composer on the next draw.
            }
            Action::OpenMcpMenu => self.open_mcp_menu(),
            Action::CloseMcpMenu => {
                // Esc/Ctrl+C dismissed the manager: nothing to reap; the
                // region collapses back to the composer on the next draw.
            }
            Action::McpOp(op) => self.apply_mcp_op(op),
            Action::Notice(text) => {
                // A slash command's one-off system notice. The helper finalises
                // any mid-flight reply segment first (same ordering trick as a
                // tool call) so the notice slots after it in scrollback and
                // history alike.
                self.commit_system_notice(&text);
            }
            Action::Toast(text) => {
                // A slash command's transient info toast — a soft rejection
                // (/resume, /help run mid-turn) that self-clears above the box
                // instead of landing in scrollback. See docs/toast.md.
                self.toast(text, ToastKind::Info);
            }
            Action::Copy(maybe_text) => self.copy_to_clipboard(maybe_text),
        }
        Ok(Flow::Continue)
    }

    /// The bookkeeping every handled key press owes: schedule its redraw, kick
    /// off a file search if the active `@token` changed, and delete the temp PNGs
    /// of any attachments the key discarded (an atomic placeholder delete, a
    /// Ctrl+C clear, a `/clear`ed queue) — the pure core records the drops, the
    /// file I/O lives here (`docs/image-paste.md`).
    fn after_key(&mut self, key: &ratatui::crossterm::event::KeyEvent) {
        self.schedule_for_key(key);
        self.dispatch_file_search();
        for path in self.app.take_discarded_images() {
            let _ = std::fs::remove_file(path);
        }
    }

    /// `/copy`: do the clipboard I/O here at the boundary (`run_selected_command`
    /// stayed pure). The pure core already decided *what* to copy — `Some(text)`
    /// or, for an empty conversation, `None`. The result surfaces as a transient
    /// toast, not a scrollback bullet. See `docs/copy.md` / `docs/toast.md`.
    fn copy_to_clipboard(&mut self, maybe_text: Option<String>) {
        match maybe_text {
            None => self.toast(COPY_EMPTY_NOTICE, ToastKind::Error),
            Some(text) => match clipboard::copy_to_clipboard(&text) {
                Ok(lease) => {
                    // Hold the native selection alive for the app's lifetime
                    // (Linux); None over OSC 52.
                    self.clipboard_lease = lease;
                    self.toast(COPY_OK_NOTICE, ToastKind::Info);
                }
                Err(reason) => self.toast(format!("Copy failed: {reason}"), ToastKind::Error),
            },
        }
    }

    /// Esc mid-generation (codex-style): stop the backend but NEVER join it on
    /// the loop. A real backend can be parked in a blocking network read (the
    /// pre-first-token pause) for up to one op-timeout before it observes the
    /// cancel; join()ing there froze the whole UI — spinner, timer, composer —
    /// for that long (the interrupt-lag bug). Instead
    /// [`Session::abandon_inflight`] cancels + detaches the thread and swaps in a
    /// fresh reply channel, so any stale event it still emits (a final chunk, or a
    /// ToolStart that would otherwise wedge a phantom running tool) lands on the
    /// dropped receiver and can't reach the next turn. See `docs/interrupt.md`.
    fn interrupt_turn(&mut self) -> std::io::Result<()> {
        self.abandon_inflight();
        // A thinking phase caught mid-thought settles first, so its
        // `Thought for …` cell sits ahead of everything the interrupt keeps.
        // Recording it is also what makes this a *Kept* interrupt: real work
        // streamed, and codex never retracts what streamed. With nothing
        // thought (or the display off) it is a no-op and the undo path is
        // untouched (docs/thinking-stream.md).
        self.settle_reasoning();
        // Interrupt only arises in the conversation view (overlay Esc returns
        // instead), so nothing here touches the alternate screen.
        match self.app.interrupt_turn() {
            Some(alter_zero::app::InterruptedTurn::Undone) => {
                // Nothing had streamed and nothing was queued: the submission is
                // undone — interrupt_turn put the message back in the composer
                // and dropped it from history. Repaint scrollback without it (a
                // purge rebuild from the truncated history, like /clear — it
                // resets `render` itself). No notice, and no queue flush — the
                // empty queue is the undo's precondition — but background
                // completions held during the turn still settle
                // (docs/background.md).
                self.repaint_conversation()?;
                self.dispatch_after_turn();
            }
            Some(alter_zero::app::InterruptedTurn::Kept {
                partial,
                tool,
                notice,
                agents,
            }) => {
                // Something streamed — keep it, commit the notice (None for a `!`
                // shell turn, whose `⎿ Interrupted by user` cell already says it
                // — req 2). A live agent group resolved as interrupted: stop its
                // subagent threads (the abandoned backend's own kill sweep may
                // still be parked in a network read) and commit its red tree cell
                // (docs/agent-tool.md). `commit_turn_failure`'s
                // `render.finish(partial)` needs the committed-lines cache intact
                // (it flushes only the not-yet-committed tail), so reset `render`
                // AFTER it, never before.
                if let Some(group) = &agents {
                    for entry in &group.agents {
                        let _ = self.agent_registry.kill(&entry.id);
                        self.agent_clocks.remove(&entry.id);
                    }
                }
                self.commit_turn_failure(partial, tool, agents, notice);
                self.render.reset();
                // The user interrupted to send their queued follow-ups right away
                // (their spec; codex's submit-pending-steers-after-interrupt).
                // The front entry — the first queue — goes out now (a text batch
                // to the model, or a `!` command run locally); any later batches
                // iterate at the following turn-ends. Held background completions
                // settle first, like every turn end (docs/background.md).
                self.dispatch_after_turn();
            }
            None => self.render.reset(),
        }
        Ok(())
    }

    /// `/clear`: the app state is already wiped (history, streaming buffer,
    /// running tool, status, queued backlog). Mid-turn it is also a **kill** — the
    /// user asked for a fresh slate, not a finished turn — so stop the backend and
    /// isolate it from the blank screen, then wipe every subsystem that outlives a
    /// turn and rebuild the screen from nothing.
    fn clear_conversation(&mut self) -> std::io::Result<()> {
        // `abandon_inflight` cancels + detaches the backend and hands back a
        // fresh channel (never join() on the loop — the interrupt-lag freeze):
        // any stale chunk or ToolStart the dying thread still emits lands on the
        // dropped receiver, so it can't repopulate the cleared state. See
        // docs/interrupt.md.
        self.abandon_inflight();
        self.render.reset();
        // A fresh slate kills the background shells too (clear_conversation
        // already forgot them, so their Exited events find nothing and owe no
        // notice — docs/background.md). Notes already posted for the wiped
        // conversation are dropped with it, else the next turn end would start a
        // phantom follow-up turn about them.
        self.registry.kill_all();
        let _ = self.registry.take_pending_notices();
        // …and the subagents (docs/agent-tool.md): the wiped roster drops their
        // late events.
        self.agent_registry.kill_all();
        self.agent_clocks.clear();
        self.agent_expiry.clear();
        // …and the permission gate: `clear_conversation` dropped the prompt, the
        // cancelled threads reap themselves, so no unclaimed decision may linger
        // for the next turn (docs/permissions.md). The ask gate's board goes
        // the same way (docs/ask.md).
        self.permissions.clear();
        self.ask.clear();
        // …and the task list: `clear_conversation` wiped App's snapshot; the
        // shared registry follows so the model's next `tasklist` agrees
        // (docs/task-tools.md).
        self.sync_task_registry();
        // A cleared conversation starts a fresh session file (codex's /new); the
        // old one keeps what it had (docs/resume.md). Re-seed the checkpoint chain
        // with a pristine snapshot of the current tree so a backtrack in the new
        // session can restore its starting point (docs/checkpoint.md).
        self.recorder.start_new();
        // The session boundary for the hooks: SessionEnd(clear) fires now —
        // bounded, so /clear cannot hang — and SessionStart(clear) at the
        // next turn's top, the codex drain (docs/hooks.md). Claude Code
        // fires the same pair for its /clear.
        self.models.fire_session_end("clear");
        self.models.queue_session_source("clear");
        if let Some(commit) = self.checkpoints.snapshot("session start") {
            self.recorder
                .record_checkpoint(checkpoint::Checkpoint { after: 0, commit });
        }
        // Purge scrollback + clear the screen (codex's /clear), not just blank
        // the visible screen — the old conversation must be gone from scrollback
        // too, so scrolling up shows nothing.
        self.repaint_conversation()
    }

    /// A terminal resize. Repaint from history on ANY dimension change (codex
    /// redraws from source on every resize): a width change stales the wrapping,
    /// and a height change moves the screen contents out from under the tracked
    /// viewport row — repainting at a stale row leaves phantom input boxes
    /// behind. Purge scrollback + clear the screen first (codex's resize replay),
    /// rebuilding the whole conversation from history — the in-place overwrite
    /// otherwise left the emulator's own reflowed copy of the old content on
    /// screen, duplicating the TUI text. Only the inline view reflows; the overlay
    /// just redraws at the new size (reflowing would write the alternate screen).
    fn on_resize(&mut self, width: u16, height: u16) -> std::io::Result<()> {
        let size_changed = self.term.resized(width, height);
        if size_changed && self.app.view == View::Conversation {
            // A resize under an open permission prompt reseats it with this
            // purge; the reflow notes it itself (`term.take_modal_scrolled`) so
            // the prompt's close purge-rebuilds too instead of stranding the box
            // (docs/permissions.md).
            self.repaint_active_view()?;
        } else if size_changed {
            // Under an overlay the inline view can't reflow (it would write the
            // alternate screen) — remember to purge-rebuild on return instead of
            // the usual in-place overwrite. A prompt open beneath the overlay is
            // reseated by that return's reflow, which notes it then
            // (`term.take_modal_scrolled`).
            self.overlay_resized = true;
        }
        self.burst.reset();
        self.frame.schedule_frame();
        Ok(())
    }

    /// A bracketed paste, routed to whatever field the user is looking at. The
    /// `/login` flow takes pastes (an API key is always pasted) and so does the
    /// `/model` filter (a model id is copied from a provider's dashboard far more
    /// often than it is typed), so both route to their own handler and the paste
    /// never reaches the composer draft underneath. The `/resume` picker's
    /// type-to-search accepts pastes too (codex normalizes them into the query);
    /// only the overlays ignore them, like typing there.
    fn on_paste(&mut self, pasted: &str) {
        match self.app.view {
            // The ask modal owns every key, so it owns pastes too: the live
            // entry field takes them (placeholder-collapsed over the
            // threshold), the option pages swallow them (docs/ask.md).
            View::Conversation if self.app.ask().is_some() => {
                self.app.paste_into_ask(pasted);
            }
            View::Conversation if self.app.key_onboarding.is_some() => {
                self.app.paste_into_key_onboarding(pasted);
            }
            View::Conversation if self.app.model_picker.is_some() => {
                self.app.paste_into_model_filter(pasted);
            }
            // The `/settings` menu has a search field too, but nothing anyone
            // pastes is a setting name — swallow it rather than letting it
            // reach the composer draft underneath (docs/settings.md).
            View::Conversation if self.app.settings_picker.is_some() => {}
            // The `/mcp` manager: the auth page's `URL >` field takes pastes
            // (the redirect URL is always pasted — that is the field's whole
            // point); every other page swallows them (docs/mcp.md).
            View::Conversation if self.app.mcp_menu.is_some() => {
                let _ = self.app.paste_into_mcp_auth(pasted);
            }
            View::Conversation => {
                self.app.on_paste(pasted);
                // The paste may have changed the active `@token`.
                self.dispatch_file_search();
            }
            View::ResumePicker => self.app.paste_into_resume_search(pasted),
            View::ToolOutput | View::ContextDebug => {}
        }
        self.burst.reset();
        self.frame.schedule_frame();
    }

    /// Esc-Esc backtrack confirmed: history is already truncated to the chosen
    /// message and the composer prefilled. Reset the code to that point's
    /// checkpoint (`docs/checkpoint.md`) before returning to the inline view.
    ///
    /// The restore target is the latest checkpoint at or before the new history
    /// length; the current tree is backed up first (recoverable via the store's
    /// reflog). The loop-bottom `recorder.sync` sees history shrink and rewrites
    /// the file, dropping the rewound-away checkpoints in lockstep.
    fn confirm_backtrack(&mut self) -> std::io::Result<()> {
        let restored = self.restore_checkpoint_at(self.app.history.len());
        // `App::confirm_backtrack` already rewound the checklist to the
        // snapshot at the cut; the shared registry follows
        // (docs/task-tools.md).
        self.sync_task_registry();
        // Consume the overlay-resized flag (a resize under the overlay must not
        // leak to the next return); we purge unconditionally below anyway.
        self.overlay_resized = false;
        self.term.exit_overlay()?;
        // (The transcript cache needs no explicit clear: the truncation bumped
        // the history generation, so the loop-bottom warm rebuilds the kept
        // prefix.)
        // Backtrack TRUNCATES history, so lines already on screen or queued
        // for scrollback may belong to the dropped exchange. Purge-rebuild
        // like /resume and resize (the reflow drops the queue): the truncated
        // conversation replaces the screen AND scrollback cleanly
        // (invariant 3).
        self.repaint_conversation()?;
        if restored {
            self.toast(CHECKPOINT_REWOUND_NOTICE, ToastKind::Info);
        }
        Ok(())
    }

    /// Sync the shared task registry to `App`'s checklist — after any history
    /// rewind (`/clear`, a `/resume` load, a backtrack truncation), so the
    /// model's next `tasklist` call agrees with what the strip shows
    /// (`docs/task-tools.md`). The forward direction never needs this: a task
    /// call *starts* at the registry and reaches `App` on its `TaskCall`
    /// event.
    pub(crate) fn sync_task_registry(&self) {
        self.task_registry.replace(self.app.tasks().clone());
    }
}
