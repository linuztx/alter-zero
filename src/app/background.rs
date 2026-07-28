//! Background shells: the roster behind `run_in_background` and Ctrl+B, the
//! ↓ manager band, and the completion notices they post.
//! See `docs/background.md`.

use super::*;

/// One background shell's completion, committed to history as a one-line
/// notice cell (`● Background command "{description}" completed (exit code
/// 0)` — green bullet on success, red on failure or a user stop). The
/// `output_tail` rides the item for the model's context
/// ([`crate::context::context_messages`]) but is never rendered — the model
/// summarises it in the automatic follow-up turn. See `docs/background.md`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackgroundNotice {
    /// The human description shown in the headline: the model-supplied
    /// `description` argument, falling back to the command line.
    pub description: String,
    /// The registry task id (`bvyo7tkbe`, …) — lets the model pair the notice
    /// to the launch result it received.
    pub id: String,
    /// The exit code, or `None` when the process died to a signal.
    pub code: Option<i32>,
    /// Whether the user stopped it (the manager's `x` / a `/clear` sweep).
    pub killed: bool,
    /// The last lines of output at exit — context-only, never rendered.
    pub output_tail: String,
    /// Wall-clock stamp of when the completion was recorded. Recorded but not
    /// displayed, like tool stamps. See `docs/timestamps.md`.
    pub timestamp: String,
}

impl BackgroundNotice {
    /// Did the command succeed (exit code 0, not stopped by the user)? Picks
    /// the notice bullet colour: green for success, red otherwise.
    #[must_use]
    pub fn ok(&self) -> bool {
        !self.killed && self.code == Some(0)
    }

    /// The rendered one-liner: `Background command "{description}" {outcome}`.
    #[must_use]
    pub fn headline(&self) -> String {
        let outcome = self.outcome_phrase();
        format!("Background command \"{}\" {outcome}", self.description)
    }

    /// The outcome clause of the headline / context note.
    #[must_use]
    pub fn outcome_phrase(&self) -> String {
        if self.killed {
            return "was stopped by the user".to_string();
        }
        match self.code {
            Some(0) => "completed (exit code 0)".to_string(),
            Some(code) => format!("failed (exit code {code})"),
            None => "was terminated by a signal".to_string(),
        }
    }

    /// The model-facing context note: the headline plus the output tail (the
    /// bracketed user-role form `context::context_messages` sends). Also the
    /// automatic follow-up turn's prompt text.
    #[must_use]
    pub fn context_text(&self) -> String {
        let tail = if self.output_tail.trim().is_empty() {
            "(no output)"
        } else {
            self.output_tail.trim_end_matches('\n')
        };
        format!(
            "[background] Background command \"{}\" (id {}) {}.\nFinal output (tail):\n{tail}",
            self.description,
            self.id,
            self.outcome_phrase(),
        )
    }
}

/// One **running** background shell, as the pure state sees it (the process
/// itself lives in the boundary's `background::BackgroundRegistry`; its
/// events — start, output lines, exit — are applied here). An exited shell
/// leaves the list ([`App::bg_exited`]): the completion notice is the record.
/// See `docs/background.md`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackgroundShell {
    /// The registry task id (`bvyo7tkbe`, …).
    pub id: String,
    /// The command line, shown in the ↓ manager's list and details view.
    pub command: String,
    /// The model-supplied description (notices fall back to the command).
    pub description: Option<String>,
    /// Whether the model launched it — its completion then auto-starts a
    /// follow-up turn; a user-launched (`!` + Ctrl+B) shell only commits the
    /// notice.
    pub from_model: bool,
    /// The live output **tail** (capped at `BG_TAIL_MAX_BYTES`, trimmed to
    /// line boundaries) — what the details view's output box tails and the
    /// completion notice snapshots. The full output is teed to the task's
    /// interim-output file at the boundary.
    pub output: String,
    /// How long the shell has been running — boundary-injected before each
    /// draw ([`App::set_background_runtime`], the `set_status_times` pattern).
    pub runtime: Duration,
}

/// The retained size of a background shell's in-memory output tail. Trimmed
/// from the **front** on line boundaries, so the details view / completion
/// notice always see the newest lines.
const BG_TAIL_MAX_BYTES: usize = 16 * 1024;

/// How much of a finished shell's tail rides its completion notice into the
/// model's context — enough to summarise from without bloating every later
/// turn (the full output is still in the task's interim-output file).
const BG_NOTICE_TAIL_MAX_BYTES: usize = 4 * 1024;

/// …and at most this many lines of it.
const BG_NOTICE_TAIL_MAX_LINES: usize = 30;

/// What a background shell left behind when it exited ([`App::bg_exited`]) —
/// held in `App::pending_bg` and settled at the next **safe boundary**
/// (a tool resolution / segment flush mid-turn, else the turn end; at once
/// while idle): the loop records a [`BackgroundNotice`] for it, its
/// [`context_text`](BgCompletion::context_text) having already been posted
/// onto the registry's notice board for the in-flight agent. See
/// `docs/background.md`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BgCompletion {
    pub id: String,
    pub command: String,
    pub description: Option<String>,
    pub from_model: bool,
    /// Exit code, or `None` when the process died to a signal.
    pub code: Option<i32>,
    /// Whether the user stopped it (`x` in the manager, or a kill sweep).
    pub killed: bool,
    /// The output tail at exit (already capped for the notice).
    pub output_tail: String,
}

impl BgCompletion {
    /// The notice's display description: the model's `description` argument,
    /// falling back to the command line.
    #[must_use]
    pub fn display_description(&self) -> &str {
        self.description.as_deref().unwrap_or(&self.command)
    }

    /// The model-facing context note — byte-identical to the
    /// [`BackgroundNotice::context_text`] the settle later records for this
    /// completion, so the note the in-flight agent injects mid-turn and the
    /// one every later turn's derived context replays never diverge (the
    /// timestamp, stamped only at settle, plays no part in the text).
    #[must_use]
    pub fn context_text(&self) -> String {
        BackgroundNotice {
            description: self.display_description().to_string(),
            id: self.id.clone(),
            code: self.code,
            killed: self.killed,
            output_tail: self.output_tail.clone(),
            timestamp: String::new(),
        }
        .context_text()
    }
}

/// Which page of the ↓ background manager band is showing. The band is
/// **inline** (it replaces the composer, exactly like the `/model` picker —
/// never an alternate-screen [`View`]) and owns every key while open. See
/// `docs/background.md`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackgroundView {
    /// The shell list (`Background` / `{n} active shells` / selectable rows),
    /// or the `No tasks currently running` empty state.
    List {
        /// The highlighted row (clamped as shells exit).
        selected: usize,
    },
    /// One shell's details: status/runtime/command fields over a live output
    /// box. Keyed by id so a *different* shell exiting never retargets the
    /// view; when this shell exits the view falls back to the list.
    Details { id: String },
}

/// The completion-notice tail of a shell's output: the last
/// [`BG_NOTICE_TAIL_MAX_LINES`] lines, additionally capped at
/// [`BG_NOTICE_TAIL_MAX_BYTES`] (front-trimmed on line, then char,
/// boundaries) — what rides the notice into the model's context.
fn notice_tail(output: &str) -> String {
    let trimmed = output.trim_end_matches('\n');
    if trimmed.is_empty() {
        return String::new();
    }
    let lines: Vec<&str> = trimmed.split('\n').collect();
    let keep = lines.len().min(BG_NOTICE_TAIL_MAX_LINES);
    let mut tail = lines[lines.len() - keep..].join("\n");
    if tail.len() > BG_NOTICE_TAIL_MAX_BYTES {
        let cut = tail.len() - BG_NOTICE_TAIL_MAX_BYTES;
        let boundary = tail[cut..]
            .find('\n')
            .map_or_else(|| ceil_char_boundary(&tail, cut), |nl| cut + nl + 1);
        tail.drain(..boundary);
    }
    tail
}

/// The smallest char boundary in `s` at or after `at` (a dependency-free
/// `str::ceil_char_boundary`, which is still unstable).
fn ceil_char_boundary(s: &str, at: usize) -> usize {
    let mut at = at.min(s.len());
    while at < s.len() && !s.is_char_boundary(at) {
        at += 1;
    }
    at
}

impl App {
    /// The running background shells, oldest first (see `docs/background.md`).
    #[must_use]
    pub fn background(&self) -> &[BackgroundShell] {
        &self.background
    }

    /// The running background shell with this registry id, if it still runs.
    #[must_use]
    pub fn background_shell(&self, id: &str) -> Option<&BackgroundShell> {
        self.background.iter().find(|shell| shell.id == id)
    }

    /// Is the footer's shell indicator lit (↓ pressed, Enter pending)?
    /// [`ui::footer_line`] paints that segment on cyan while it is.
    ///
    /// [`ui::footer_line`]: crate::ui::footer_line
    #[must_use]
    pub const fn background_focused(&self) -> bool {
        self.background_focus
    }

    /// A background shell started (the registry's `BgEvent::Started`): list it
    /// so the footer count, the summary suffix, and the ↓ manager see it.
    pub fn bg_started(
        &mut self,
        id: &str,
        command: &str,
        description: Option<String>,
        from_model: bool,
    ) {
        self.background.push(BackgroundShell {
            id: id.to_string(),
            command: command.to_string(),
            description,
            from_model,
            output: String::new(),
            runtime: Duration::ZERO,
        });
    }

    /// Append a chunk of a background shell's live output (the registry's
    /// `BgEvent::Output`), keeping only the newest `BG_TAIL_MAX_BYTES` —
    /// trimmed from the front on line boundaries so the details view always
    /// tails whole lines. Unknown ids (a chunk racing its shell's removal)
    /// are dropped.
    pub fn bg_output(&mut self, id: &str, chunk: &str) {
        let Some(shell) = self.background.iter_mut().find(|shell| shell.id == id) else {
            return;
        };
        shell.output.push_str(chunk);
        if shell.output.len() > BG_TAIL_MAX_BYTES {
            let cut = shell.output.len() - BG_TAIL_MAX_BYTES;
            // Trim to the next line boundary past the cut so the tail never
            // opens mid-line (fall back to a char boundary when one line
            // exceeds the whole cap).
            let boundary = shell.output[cut..]
                .find('\n')
                .map_or_else(|| ceil_char_boundary(&shell.output, cut), |nl| cut + nl + 1);
            shell.output.drain(..boundary);
        }
    }

    /// Inject a background shell's runtime before a draw (the
    /// [`set_status_times`](App::set_status_times) pattern — the started
    /// clocks live at the boundary). Unknown ids are ignored.
    pub fn set_background_runtime(&mut self, id: &str, runtime: Duration) {
        if let Some(shell) = self.background.iter_mut().find(|shell| shell.id == id) {
            shell.runtime = runtime;
        }
    }

    /// A background shell exited (the registry's `BgEvent::Exited`): remove it
    /// from the list and return its completion for the loop to settle —
    /// deferred to the next safe boundary while a turn is in flight
    /// ([`defer_bg_completion`](App::defer_bg_completion)), else settled at
    /// once. A details view watching this shell falls back to the list (and
    /// the list selection re-clamps); `None` for an unknown id (already
    /// swept — e.g. by `/clear` — so no notice is owed).
    pub fn bg_exited(&mut self, id: &str, code: Option<i32>, killed: bool) -> Option<BgCompletion> {
        let index = self.background.iter().position(|shell| shell.id == id)?;
        let shell = self.background.remove(index);
        // The footer's indicator goes with the last shell — nothing left to
        // keep lit (docs/background.md).
        if self.background.is_empty() {
            self.background_focus = false;
        }
        match &mut self.background_view {
            Some(BackgroundView::Details { id: watched }) if *watched == id => {
                self.background_view = Some(BackgroundView::List {
                    selected: index.min(self.background.len().saturating_sub(1)),
                });
            }
            Some(BackgroundView::List { selected }) => {
                *selected = (*selected).min(self.background.len().saturating_sub(1));
            }
            _ => {}
        }
        Some(BgCompletion {
            id: shell.id,
            command: shell.command,
            description: shell.description,
            from_model: shell.from_model,
            code,
            killed,
            output_tail: notice_tail(&shell.output),
        })
    }

    /// Hold a completion that landed mid-turn for the next boundary settle.
    pub fn defer_bg_completion(&mut self, completion: BgCompletion) {
        self.pending_bg.push_back(completion);
    }

    /// Drain the held completions (empty when none landed) — the loop
    /// settles them at every safe boundary: each tool resolution and
    /// segment-flush point mid-turn, and every turn end (`StreamDone`,
    /// `Error`, and the Esc interrupt alike). See `docs/background.md`.
    pub fn take_pending_bg_completions(&mut self) -> Vec<BgCompletion> {
        self.pending_bg.drain(..).collect()
    }

    /// Record a completion's [`BackgroundNotice`] in history (stamped like
    /// every recorded item) and return it for the loop to commit to
    /// scrollback. The notice is what repaints on resize, lists in the Ctrl+O
    /// transcript, and rides the derived context to the model.
    pub fn record_background_notice(&mut self, completion: &BgCompletion) -> BackgroundNotice {
        let notice = BackgroundNotice {
            description: completion.display_description().to_string(),
            id: completion.id.clone(),
            code: completion.code,
            killed: completion.killed,
            output_tail: completion.output_tail.clone(),
            timestamp: self.now_stamp(),
        };
        self.history.push(HistoryItem::Background(notice.clone()));
        notice
    }

    /// Can Ctrl+B move the current command to the background? True while the
    /// front tool is a **running command** — a model `bash` call or a `!`
    /// shell turn — the only runners that poll the registry's background
    /// request — **or while a foreground agent group runs** (its wait loop
    /// polls the same latch and hands the rest of the group over,
    /// `docs/agent-tool.md`). See `docs/background.md`.
    #[must_use]
    pub fn can_move_to_background(&self) -> bool {
        if self
            .agent_group
            .as_ref()
            .is_some_and(|group| !group.background)
        {
            return true;
        }
        self.tool_queue.front().is_some_and(|tool| {
            tool.status == ToolStatus::Running && (tool.shell || tool.name == "Bash")
        })
    }

    /// Should ↓ light up the footer's shell indicator? Only from an
    /// idle-looking composer — empty, not in shell mode, no palette/file band
    /// open — and only while a shell is actually **running**: the highlight
    /// lands *on* the footer's `{n} shell(s)` segment, so with no segment
    /// there is nothing to light and ↓ keeps its history-recall/cursor
    /// meaning. The manager has no hidden keybinding — no count, no way in.
    pub(super) fn background_focusable(&self) -> bool {
        !self.background.is_empty()
            && self.input.is_empty()
            && !self.shell_mode
            && self.command_menu.is_none()
            && self.file_search.is_none()
    }

    /// Open the ↓ manager band on the shell list, dismissing whatever shared
    /// the composer (the shortcuts band; the pickers own their keys, so they
    /// can't be open here) — including the footer highlight the band replaces.
    pub fn open_background_view(&mut self) {
        self.shortcuts_open = false;
        self.backtrack = Backtrack::default();
        self.background_focus = false;
        self.background_view = Some(BackgroundView::List { selected: 0 });
    }

    /// Keys while the footer's shell indicator is lit — the step ↓ takes
    /// *before* the band opens (Claude-Code-style, `docs/background.md`).
    /// `Enter` opens the manager the indicator points at; `Esc`, `↑` and
    /// `Ctrl+C` dismiss the highlight; a second `↓` keeps it (there is only
    /// the one indicator). Returns `None` for every other key — the highlight
    /// clears and the key goes on to do its normal job (codex's
    /// reset-after-activity, the `?` band's rule).
    pub(super) fn on_key_background_focus(&mut self, key: KeyEvent) -> Option<Action> {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.background_focus = false;
            return Some(Action::None);
        }
        match key.code {
            // A *plain* Enter opens the band. Alt/Shift+Enter stay the newline
            // keys (docs/shift-enter.md) — they fall through below, dismissing
            // the highlight and inserting the newline, like Ctrl+J does.
            KeyCode::Enter
                if !key
                    .modifiers
                    .intersects(KeyModifiers::ALT | KeyModifiers::SHIFT) =>
            {
                self.open_background_view();
                Some(Action::None)
            }
            // A second ↓ steps past the shell indicator into the agent
            // roster's selection when one is listed (`docs/agent-tool.md`);
            // with no roster it keeps the highlight (the one indicator).
            KeyCode::Down => {
                if self.agent_selectable() {
                    self.background_focus = false;
                    self.agent_selection = Some(0);
                }
                Some(Action::None)
            }
            KeyCode::Esc | KeyCode::Up => {
                self.background_focus = false;
                Some(Action::None)
            }
            _ => {
                self.background_focus = false;
                None
            }
        }
    }

    /// Close the ↓ manager band; the composer returns on the next draw.
    pub fn close_background_view(&mut self) {
        self.background_view = None;
    }

    /// Keys while the ↓ background manager band is open — it owns **every**
    /// key (routed at the top of [`on_key`](App::on_key)), like the `/model`
    /// picker. List: ↑/↓ move, Enter views the highlighted shell, `x` stops
    /// it, Esc/Ctrl+C close. Details: ← back to the list, Esc/Enter/Space
    /// close, `x` stops. See `docs/background.md`.
    pub(super) fn on_key_background(&mut self, key: KeyEvent) -> Action {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.close_background_view();
            return Action::None;
        }
        let Some(view) = self.background_view.as_mut() else {
            return Action::None;
        };
        match view {
            BackgroundView::List { selected } => {
                let last = self.background.len().saturating_sub(1);
                match key.code {
                    KeyCode::Up => *selected = selected.saturating_sub(1),
                    KeyCode::Down => *selected = (*selected + 1).min(last),
                    KeyCode::Enter => {
                        if let Some(shell) = self.background.get(*selected) {
                            let id = shell.id.clone();
                            self.background_view = Some(BackgroundView::Details { id });
                        }
                    }
                    KeyCode::Char('x') => {
                        if let Some(shell) = self.background.get(*selected) {
                            return Action::KillBackground(shell.id.clone());
                        }
                    }
                    KeyCode::Esc => self.close_background_view(),
                    _ => {}
                }
            }
            BackgroundView::Details { id } => match key.code {
                KeyCode::Left => {
                    let selected = self
                        .background
                        .iter()
                        .position(|shell| shell.id == *id)
                        .unwrap_or(0);
                    self.background_view = Some(BackgroundView::List { selected });
                }
                KeyCode::Esc | KeyCode::Enter | KeyCode::Char(' ') => self.close_background_view(),
                KeyCode::Char('x') => return Action::KillBackground(id.clone()),
                _ => {}
            },
        }
        Action::None
    }
}
