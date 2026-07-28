//! [`Action`] — what a key press asks the loop to do.
//!
//! [`App::on_key`](super::App::on_key) is pure: it mutates state and returns one
//! of these for `main.rs` to carry out (spawn a turn, quit, copy, …). The
//! boundary each variant crosses is documented with the feature that owns it —
//! `docs/interrupt.md`, `docs/queue.md`, `docs/copy.md`, `docs/resume.md`; the
//! seam itself is `docs/design.md`.

use super::*;

/// The result of handling a key press, interpreted by the event loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Nothing to do.
    None,
    /// The user submitted a (non-empty) message; start a reply for it. Any
    /// Ctrl+V-attached images travel separately — the loop drains
    /// [`App::take_submission_images`] for their paths. See `docs/image-paste.md`.
    Submit(String),
    /// The user pressed Ctrl+V (or Ctrl+Alt+V) to paste an image. The decision is
    /// pure; the loop does the clipboard I/O ([`crate::clipboard`]) and calls
    /// [`App::attach_image`] on success (or commits a red notice on failure). See
    /// `docs/image-paste.md`.
    PasteImage,
    /// The user toggled the tool-output view (Ctrl+O, or Esc to leave it). The
    /// loop syncs the full-screen overlay to the now-updated [`App::view`].
    ToggleToolView,
    /// Enter confirmed an Esc-Esc backtrack: [`App::history`] is already
    /// truncated to the chosen message and the composer prefilled. The loop
    /// resets the code to that point's checkpoint (`docs/checkpoint.md`), then
    /// leaves the overlay and repaints — the same return path as
    /// [`Action::ToggleToolView`]'s exit branch. Distinguished from a plain
    /// overlay toggle so the loop knows a rewind (not just a view change)
    /// happened. See `docs/backtrack.md`.
    ConfirmBacktrack,
    /// The user toggled the Ctrl+D context-debug view (or closed it with
    /// q/Esc). The loop syncs the overlay to the now-updated [`App::view`],
    /// exactly like [`Action::ToggleToolView`]. See `docs/context.md`.
    ToggleContextDebug,
    /// A slash command produced a one-off system notice (e.g. `/help`'s command
    /// list, or a stub's placeholder). The loop records it as a [`Role::System`]
    /// message and commits it to scrollback, like a normal message.
    Notice(String),
    /// A slash command cleared the conversation (`/clear`). [`App::history`] is
    /// already empty; the loop repaints the now-blank inline view.
    Clear,
    /// `/copy` — write the last assistant response to the system clipboard.
    /// `Some(text)` is the text to copy ([`App::last_assistant_text`]); `None`
    /// means there was no response to copy. The decision is pure; the loop does
    /// the clipboard I/O ([`crate::clipboard::copy_to_clipboard`]) and commits
    /// the success/empty/failure notice — codex's `/copy`. See `docs/copy.md`.
    Copy(Option<String>),
    /// The user pressed Enter on a `!`-prefixed line from an idle composer: run
    /// the carried command (the text after the `!`, trimmed) locally. The loop
    /// echoes `❯ !command`, calls [`App::begin_shell`], and spawns it. See
    /// `docs/shell-command.md`.
    RunShell(String),
    /// The user pressed Esc while a turn was in flight: stop the generation
    /// (cancel + reap the backend, then [`App::interrupt_turn`]) — codex-style.
    Interrupt,
    /// `/compact` from an idle composer with a non-empty context: run the
    /// summarization turn. The loop calls [`App::begin_compact`], derives the
    /// context, pushes codex's summarization prompt as its final user entry,
    /// and spawns the request on a one-off **tools-free** backend — the
    /// summary streams into the compact buffer (never rendered) and
    /// [`App::finish_compact`] appends the marker at `StreamDone`. See
    /// `docs/compact.md`.
    Compact,
    /// `/resume` from an idle composer: open the session picker. The *loop*
    /// scans the sessions dir (the fs I/O stays at the boundary) and hands the
    /// result to [`App::open_resume_picker`]. See `docs/resume.md`.
    OpenResumePicker,
    /// The `/resume` picker was dismissed (Esc on an empty query, or Ctrl+C —
    /// codex's from-a-session picker closes rather than quits): [`App::view`]
    /// is already back on the conversation; the loop leaves the alternate
    /// screen and repaints, the Ctrl+O return.
    CloseResumePicker,
    /// Enter in the `/resume` picker: swap the conversation to the rollout
    /// file at this path. The loop reads + parses it (the I/O), calls
    /// [`App::load_session`], and adopts the file for further recording —
    /// or commits a red notice if the read fails, the current conversation
    /// unharmed (codex). See `docs/resume.md`.
    ResumeSession(PathBuf),
    /// Raise a transient info [`Toast`](crate::app::Toast) above the box — a
    /// confirmation or soft rejection that self-clears (e.g. `/resume` or `/help`
    /// run while a turn is active). The loop calls [`App::show_toast`] and arms
    /// the boundary's expiry timer; nothing is committed to scrollback. Error
    /// toasts (a `/copy` failure, a bad model switch) are raised by the boundary
    /// directly, so this carries only the info text. See `docs/toast.md`.
    Toast(String),
    /// `/model` from an idle composer: open the inline model picker. The *loop*
    /// spawns a worker to fetch the provider's model list (the HTTP stays at the
    /// boundary) and feeds it back via [`App::set_models`]. Unlike `/resume`,
    /// the picker is **inline** (it grows the bottom region in place), not an
    /// alternate-screen overlay. See `docs/llm.md`.
    OpenModelPicker,
    /// The inline model picker was dismissed (Esc on an empty query, or Ctrl+C):
    /// [`App::model_picker`] is already cleared; the loop just repaints the
    /// collapsed region.
    CloseModelPicker,
    /// Enter in the model picker: switch the active backend to this
    /// provider/model. The loop rebuilds the backend, updates the footer
    /// ([`App::set_session_info`]), and collapses the picker. See `docs/llm.md`.
    /// `reasoning` is the picked entry's parsed thinking capability (from the
    /// same `/v1/models` fetch that listed it), so a successful switch seeds
    /// the Shift+Tab cycle without refetching — `None` for a model with no
    /// reasoning. See `docs/reasoning.md`. `vision` is the entry's parsed
    /// image-input support, gating attachments on the rebuilt backend —
    /// `None` when the record didn't say. See `docs/tools.md`.
    SelectModel {
        provider: String,
        id: String,
        reasoning: Option<ReasoningSupport>,
        vision: Option<bool>,
        /// The picked entry's context window (`/v1/models` `context_length`),
        /// seeding the footer gauge + auto-compact without a refetch — `None`
        /// when the record didn't report one. See `docs/compact.md`.
        context: Option<u64>,
    },
    /// Shift+Tab cycled the thinking mode ([`App::thinking`] already advanced
    /// to the carried mode). The loop rebinds the *next* turn's backend to it,
    /// persists the choice, and presents the `Thinking: {mode}` toast (arming
    /// its expiry — why this isn't a direct `show_toast`). See
    /// `docs/reasoning.md`.
    SetThinking(ThinkingMode),
    /// `/login` from an idle composer: open the inline API-key onboarding flow.
    /// The *loop* builds the provider choices (which need boundary key
    /// resolution to mark the already-configured ones) and hands them to
    /// [`App::open_key_onboarding`]. See `docs/llm.md`.
    OpenKeyOnboarding,
    /// The onboarding flow was dismissed (Esc/Ctrl+C): [`App::key_onboarding`]
    /// is already cleared; the loop just repaints the collapsed region.
    CloseKeyOnboarding,
    /// Enter on the key-entry step: persist `key` to `env_var` in the `.env`
    /// file. The loop writes the file, updates its in-memory secrets, and
    /// commits a system notice. See `docs/llm.md`.
    SaveApiKey {
        /// The provider id the key belongs to (for the confirmation notice).
        provider: String,
        /// The environment variable to store it under (e.g. `OPENROUTER_API_KEY`).
        env_var: String,
        /// The API key the user entered.
        key: String,
    },
    /// `x` on a shell in the ↓ background manager: stop the background task
    /// with this registry id. The decision is pure; the loop kills the
    /// process group via the `background::BackgroundRegistry`, and the
    /// resulting `Exited` event removes the row / commits the stopped notice.
    /// See `docs/background.md`.
    KillBackground(String),
    /// `x` on an agent in the footer roster: stop the subagent with this id.
    /// [`App::stop_agent`] has already settled the roster entry (and hidden
    /// it — the user's `x` removes the row right away); the loop cancels the
    /// subagent thread via the `agents::AgentRegistry` and commits/settles
    /// whatever the resolution produced. See `docs/agent-tool.md`.
    StopAgent(String),
    /// Enter on an agent in the footer roster: open that agent's own inline
    /// session view ([`App::agent_view`] is already set). The loop
    /// purge-rebuilds the screen with the agent's transcript (banner + its
    /// history + live tail). See `docs/agent-tool.md`.
    ViewAgent(String),
    /// Leave the agent session view back to the main conversation
    /// ([`App::agent_view`] is already cleared): the loop purge-rebuilds the
    /// main transcript, mid-stream partial included. See `docs/agent-tool.md`.
    LeaveAgentView,
    /// Enter inside an agent session view: send `text` to that agent —
    /// queued into its running loop at the next round boundary, or spawning
    /// a chat continuation over its stored conversation when idle (the
    /// registry decides). The transcript already shows the message
    /// ([`App::agent_chat`] recorded it). See `docs/agent-tool.md`.
    AgentChat { id: String, text: String },
    /// Ctrl+B while a command is running (a model `bash` call or a `!` shell
    /// turn): move it to the background. The loop raises the registry's
    /// background request; the runner's poll loop consumes it, hands the
    /// child off, and resolves the cell as
    /// [`ToolStatus::Backgrounded`]. See `docs/background.md`.
    MoveToBackground,
    /// The user asked to quit.
    Quit,
}
