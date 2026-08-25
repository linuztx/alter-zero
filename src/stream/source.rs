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

    /// The system prompt a **subagent** launched by this backend is sent —
    /// surfaced like [`system_prompt`](ReplySource::system_prompt) so the
    /// *agent session view's* Ctrl+D shows the real thing
    /// (`docs/agent-tool.md`). Defaults to the backend's own prompt: without
    /// a distinct subagent prompt a launched agent would get the same one.
    /// `LlmBackend` overrides this with the main prompt + the subagent note
    /// (`prompts/subagent.md`).
    fn agent_system_prompt(&self) -> Option<String> {
        self.system_prompt()
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
    /// (`docs/agent-tool.md`): queued into its loop at the next round
    /// boundary, or a continuation run when it is idle. Returns whether the
    /// message was accepted. The default (the dummy, backends without a
    /// subagent registry) declines — the loop raises a toast.
    fn spawn_agent_chat(&self, _id: &str, _text: &str) -> bool {
        false
    }
}
