//! The mid-turn message queue, in two halves (`docs/queue.md`):
//!
//! - **Steering** — Enter while a model turn runs hands the draft to *that
//!   turn* ([`App::steer_draft`]). It waits in [`App::steered`] only until the
//!   agent loop's next round boundary takes it, right after the round's tool
//!   results; [`App::deliver_steered`] then turns it into a real user message.
//!   A turn that ends without taking it hands it back as the next turn
//!   ([`App::reclaim_steered`]).
//! - **Follow-up turns** — Tab opens a new [`QueuedTurn`] and a `!` command
//!   queues as its own; the loop drains one entry per turn end.

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
    /// Can a message submitted right now be folded into the turn already
    /// running? A model turn can: its agent loop takes the queue at every
    /// round boundary, so the message reaches the model *within* the turn
    /// (`docs/queue.md`).
    ///
    /// Two turns can't, and both queue their drafts as follow-up turns exactly
    /// as they always did:
    ///
    /// - a **`!` shell turn** — nothing is reading a conversation there;
    /// - a **`/compact` turn** — its request is the fixed handoff prompt over
    ///   the context being summarized, not a conversation, and a user message
    ///   folded into it would corrupt the summary (`docs/compact.md`).
    #[must_use]
    pub fn turn_steerable(&self) -> bool {
        self.is_streaming()
            && !self.is_compacting()
            && !self.status.as_ref().is_some_and(|status| status.shell)
    }

    /// Hand the composer draft to the **running** turn: it waits in
    /// [`steered`](App::steered) — shown above the box like a queued entry —
    /// until the model's next round boundary takes it. Returns the text for
    /// the boundary to push onto the shared
    /// [`SteerQueue`](crate::steer::SteerQueue) the backend thread drains.
    ///
    /// `None` when the draft can't ride a round boundary and belongs on the
    /// follow-up queue instead: a **shell-mode** `!` command (always local,
    /// always its own turn) or a draft carrying **Ctrl+V attachments** (a
    /// boundary injection is a text `ChatMessage`; the images need the typed
    /// channel a real turn start opens — `docs/image-paste.md`). Both cases
    /// fall through to [`queue_draft`](App::queue_draft), so the caller only
    /// has to ask once.
    ///
    /// The text is recorded for ↑ recall, like a normal submit.
    pub(super) fn steer_draft(&mut self) -> Option<String> {
        if self.shell_mode || !self.images.is_empty() {
            self.queue_draft(/*new_batch*/ false);
            return None;
        }
        let text = self.take_input();
        self.file_search = None; // the composer is consumed into the turn
        self.skill_picker = None;
        self.input_history.record(&text);
        self.steered.push_back(text.clone());
        Some(text)
    }

    /// The running turn took `text` into its context
    /// ([`StreamEvent::Steered`]): stop
    /// showing it above the box and record it as a **real user message**, the
    /// model having genuinely read it now.
    ///
    /// The streamed run of assistant text ahead of it is finalised first —
    /// invariant 4's flush-before-you-interleave — so the bubble slots after
    /// it and a repaint keeps that order. Returns whether a waiting row was
    /// dropped; the message is recorded either way, because what the model
    /// read is what the transcript owes the user.
    pub fn deliver_steered(&mut self, text: &str) -> bool {
        self.flush_streaming_segment();
        let waiting = self
            .steered
            .iter()
            .position(|pending| pending == text)
            .inspect(|index| {
                self.steered.remove(*index);
            })
            .is_some();
        self.record_user_message(text);
        // The uploaded text counts into this turn's `↑` tally, exactly as a
        // turn-start message does (`docs/status-indicator.md`).
        self.count_user_input(text);
        waiting
    }

    /// The turn ended without reaching another round boundary — the model
    /// simply answered — so what it never took becomes the **next** turn:
    /// one batch at the **front** of the follow-up queue, ahead of any Tab
    /// entry that was always meant for later. A no-op when nothing is waiting,
    /// so no empty batch is ever invented.
    pub fn reclaim_steered(&mut self) {
        if self.steered.is_empty() {
            return;
        }
        let texts: Vec<String> = self.steered.drain(..).collect();
        self.queued.push_front(QueuedTurn::Messages {
            texts,
            images: Vec::new(),
        });
    }

    /// Alt+Up over a steered message: put `text` back in the composer to edit,
    /// extend or drop. The boundary reclaims it from the shared queue first
    /// ([`SteerQueue::take_last`](crate::steer::SteerQueue::take_last)) —
    /// only that side knows whether the turn has already read it — and hands
    /// the text here.
    pub fn recall_steered(&mut self, text: &str) {
        if let Some(index) = self.steered.iter().rposition(|pending| pending == text) {
            self.steered.remove(index);
        }
        self.recall_input(text);
    }

    /// Alt+Up's pure half: pull the **last queued follow-up** back into the
    /// composer (see the key arm). Split out so the boundary can fall back to
    /// it when the steered message it tried to reclaim had already been read.
    pub fn recall_last_queued(&mut self) {
        match self.drain_last_batch() {
            // A text batch returns newline-joined (oldest first), its image
            // attachments re-attached so the placeholders in the restored
            // draft are backed again (docs/image-paste.md).
            Some(QueuedTurn::Messages { texts, images }) => {
                self.recall_input(&texts.join("\n"));
                self.images = images;
            }
            // A shell entry re-enters shell mode: recalling `!command`
            // re-absorbs the bang (sync_shell_mode), so the composer shows the
            // red `! command` prompt again, ready to edit/re-run.
            Some(QueuedTurn::Shell(cmd)) => self.recall_input(&format!("!{cmd}")),
            None => {}
        }
    }

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
