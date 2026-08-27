//! The backend seam: the one trait the event loop depends on.

use std::path::PathBuf;
use std::thread::JoinHandle;

use tokio::sync::mpsc::UnboundedSender;

use crate::context::ContextMessage;

use super::{CancelToken, StreamEvent};

/// A source of streamed replies. Implement this to plug a real model into the
/// app: the event loop depends only on this trait, never on a concrete backend.
///
/// `spawn` must return promptly and do the work on a background thread (or task)
/// that *only sends* on `tx` — it must never read stdin (terminal init and
/// `insert_before` own stdin; see `main.rs`). It should poll `cancel` and stop
/// early when cancellation is requested, and may send [`StreamEvent::Error`] to
/// report a failure in place of [`StreamEvent::StreamDone`].
///
/// `images` are the temp-PNG paths of any Ctrl+V-pasted images attached to the
/// turn (codex's `UserInput::LocalImage` typed channel, distinct from the text
/// `prompt`): a real vision backend reads each file and attaches it to its
/// request. The built-in [`DummyAi`](super::DummyAi) has no vision, so it only
/// acknowledges their count. See `docs/image-paste.md`.
///
/// `context` is the whole conversation so far — derived from the history by
/// [`crate::context::context_messages`] right after the turn's user message
/// was recorded, so its last entry *is* the current message (text and
/// attachments included). A real backend sends it verbatim so the model never
/// loses context; the dummy ignores it (its replies are canned). See
/// `docs/context.md`.
pub trait ReplySource {
    fn spawn(
        &self,
        prompt: String,
        images: Vec<PathBuf>,
        context: Vec<ContextMessage>,
        tx: UnboundedSender<StreamEvent>,
        cancel: CancelToken,
    ) -> JoinHandle<()>;

    /// The model id this backend answers as, shown in the session-context
    /// footer under the input box (see `docs/footer.md`). A real backend
    /// returns its real model name.
    fn model_name(&self) -> String;

    /// The system prompt this backend prepends to every request, if any —
    /// surfaced so the Ctrl+D context-debug view can show the *whole* context
    /// window (see `docs/context.md`). The dummy sends none.
    fn system_prompt(&self) -> Option<String> {
        None
    }

    /// The system prompt a **subagent** of `agent_type` is sent — surfaced
    /// like [`system_prompt`](ReplySource::system_prompt) so the *agent
    /// session view's* Ctrl+D shows the real thing (`docs/agent-tool.md`).
    /// Defaults to the backend's own prompt: without a distinct subagent
    /// prompt a launched agent would get the same one. `LlmBackend` overrides
    /// this with the type's definition — its own body when it has one, else
    /// the main prompt — plus the subagent note (`prompts/subagent.md`,
    /// `docs/subagents.md`), which is why it takes the type: two types can be
    /// sent different prompts.
    fn agent_system_prompt(&self, agent_type: &str) -> Option<String> {
        let _ = agent_type;
        self.system_prompt()
    }

    /// The `<system-reminder>` a **subagent** of `agent_type` opens on, ahead
    /// of its task prompt: the session's skills roster, which a fresh context
    /// would not otherwise carry (`docs/subagents.md`). Surfaced beside
    /// [`agent_system_prompt`](ReplySource::agent_system_prompt) and for the
    /// same reason — the agent session view's Ctrl+D has to show what that
    /// agent actually read, and its briefing is the half no history item
    /// holds. `None` when the session has no skills, or the type's `tools:`
    /// withholds `Skill` (the listing and the tool travel together).
    fn agent_system_reminder(&self, agent_type: &str) -> Option<String> {
        let _ = agent_type;
        None
    }

    /// The **auto mode classifier's task context**, rendered — the bounded
    /// task context (the turn's user request + the actions taken so far) the
    /// classifier reads before every command and MCP call, surfaced so the
    /// Ctrl+D view's classifier page (Tab) can show what the next verdict
    /// sees (`docs/permissions.md`). Read live: it grows as the turn runs
    /// and rolls its windows as the conversation goes on, so the boundary
    /// pulls it per draw rather than caching it. `None` when the backend keeps no log — the
    /// dummy, whose offline auto-mode demo answers from a pure heuristic
    /// (`permission::auto_verdict`) instead of a classifier.
    fn classifier_context(&self) -> Option<String> {
        None
    }

    /// Send a chat message into a running/settled subagent's session
    /// (`docs/agent-tool.md`, `docs/queue.md`). The backend's registry — the
    /// only thing that knows whether the loop is still running — decides
    /// which it is, and says so in the [`AgentChatDelivery`] it returns; the
    /// loop renders the message accordingly. The default (the dummy, backends
    /// without a subagent registry) declines, and the loop raises a toast.
    fn spawn_agent_chat(&self, _id: &str, _text: &str) -> AgentChatDelivery {
        AgentChatDelivery::Declined
    }
}

/// What became of a message sent into a subagent's session — the registry's
/// answer, which the loop must not second-guess: a roster status that lagged
/// the registry by one event would either strand the queued row forever or
/// record the message twice. See `docs/queue.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentChatDelivery {
    /// The agent is mid-run: the message waits on its queue and reaches its
    /// loop at the next round boundary, where
    /// [`StreamEvent::Steered`](super::StreamEvent::Steered) announces it.
    /// Shown above the box meanwhile ([`App::queue_agent_chat`]).
    ///
    /// [`App::queue_agent_chat`]: crate::app::App::queue_agent_chat
    Queued,
    /// The agent was idle: a **continuation run** started with the message as
    /// its newest user turn. Nothing will announce it — it is already in the
    /// request — so the loop records it now, like an idle submit
    /// ([`App::agent_chat`](crate::app::App::agent_chat)).
    Started,
    /// No subagents here (the dummy), or the id is unknown.
    Declined,
}
