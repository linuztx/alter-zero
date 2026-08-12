//! The mid-turn message queue: Enter batches into the current [`QueuedTurn`],
//! Tab opens a new one, and the loop drains one entry per turn end.
//! See `docs/queue.md`.

use super::*;

/// One queued turn awaiting its slot while a turn streams — codex's
/// action-tagged queued message (`QueuedInputAction`). The queue is a
/// `VecDeque<QueuedTurn>` drained FIFO, one entry per turn-end; the variant is
/// the dispatch discriminator, so a text batch goes to the model while a `!`
/// command runs locally. See `docs/queue.md`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueuedTurn {
    /// One or more Enter-batched text messages — sent to the backend as a
    /// single turn (newline-joined). Consecutive Enters append to the last such
    /// batch; **Tab** opens a new one (a separate follow-up turn). The Ctrl+V
    /// images attached to the queued drafts ride along as their
    /// `(placeholder, path)` pairs, in attach order: the paths travel the typed
    /// image channel when the batch dispatches, and an Alt+Up pull-back
    /// re-attaches them to the composer. See `docs/image-paste.md`.
    Messages {
        /// The batch's messages, oldest first.
        texts: Vec<String>,
        /// The attachments of those messages, oldest first.
        images: Vec<(String, PathBuf)>,
    },
    /// A standalone `!` shell command, run locally as its own turn (via
    /// [`App::begin_shell`]). **Never merged** with a neighbouring entry — the
    /// next Enter-text starts a fresh [`Messages`] batch — matching codex's
    /// per-completion shell dispatch (`submit_queued_shell_prompt`).
    ///
    /// [`Messages`]: QueuedTurn::Messages
    Shell(String),
}

impl App {
    /// Pop the front queued batch — the messages of the next turn — for the loop
    /// to send when the current turn ends; empty when nothing is queued. Each
    /// batch is its own turn, so popping one per turn-end iterates the
    /// Tab-separated follow-ups in order; a batch's own messages (consecutive
    /// Enters) join with newlines into that single turn (`start_turn`). See
    /// `docs/queue.md`.
    #[must_use]
    pub fn drain_next_batch(&mut self) -> Option<QueuedTurn> {
        self.queued.pop_front()
    }

    /// Take the **last** queued batch (`pop_back`), for Alt+Up to pull just that
    /// most-recent turn-batch into the composer as one editable draft — its
    /// messages newline-joined, the earlier batches left queued (codex's
    /// `edit_queued_message`, which pops the most recent entry). Empty when
    /// nothing is queued.
    pub(super) fn drain_last_batch(&mut self) -> Option<QueuedTurn> {
        self.queued.pop_back()
    }

    /// Queue the composer draft while a turn streams. `new_batch` picks the
    /// Enter vs Tab semantics: Enter (`false`) appends to the batch currently
    /// being accumulated so consecutive Enters batch into one next turn; **Tab
    /// (`true`) opens a new batch** so the message runs as its own follow-up
    /// turn after the batches already queued. An empty queue starts a fresh
    /// batch either way. A **shell-mode draft** instead queues as a standalone
    /// [`QueuedTurn::Shell`] entry ([`queue_shell`]) — run locally, never merged
    /// — so this only ever touches the [`QueuedTurn::Messages`] batches. The text
    /// is recorded for ↑ recall, like a normal submit.
    ///
    /// [`queue_shell`]: App::queue_shell
    pub(super) fn queue_draft(&mut self, new_batch: bool) {
        if self.shell_mode {
            return self.queue_shell();
        }
        // The draft's image attachments ride with the batch — staged before
        // take_input clears the composer, exactly like the idle submit path
        // (docs/image-paste.md).
        let images = std::mem::take(&mut self.images);
        let text = self.take_input();
        self.file_search = None; // the composer is consumed into the queue
        self.skill_picker = None;
        self.input_history.record(&text);
        match self.queued.back_mut() {
            Some(QueuedTurn::Messages {
                texts,
                images: batch_images,
            }) if !new_batch => {
                texts.push(text);
                batch_images.extend(images);
            }
            _ => self.queued.push_back(QueuedTurn::Messages {
                texts: vec![text],
                images,
            }),
        }
    }

    /// Queue the shell-mode draft as a standalone [`QueuedTurn::Shell`] entry —
    /// run locally as its own turn when the queue drains (`main.rs`'s
    /// `flush_next_queued` → `run_shell`), **never merged** with a neighbouring
    /// batch (codex's `submit_queued_shell_prompt` on a queued `RunShell`
    /// action: the next Enter-text starts a fresh batch since the back is a
    /// `Shell`). Exits the mode and records the full `!command` for ↑ recall,
    /// mirroring the idle [`Action::RunShell`] path. See `docs/shell-command.md`.
    fn queue_shell(&mut self) {
        let raw = self.take_input();
        self.shell_mode = false;
        self.input_history.record(&format!("!{raw}"));
        self.queued
            .push_back(QueuedTurn::Shell(raw.trim().to_string()));
    }
}
