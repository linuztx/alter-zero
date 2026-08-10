//! Recording the conversation to a rollout file as it happens — codex's
//! `RolloutRecorder`, sized down to this loop (`docs/resume.md`).
//!
//! The pure format lives in the library's `session` module; this owns the
//! impurities and one invariant: **the file mirrors `App::history`**. That
//! holds because history only ever appends or truncates, so a single
//! watermark (`recorded`) decides what [`SessionRecorder::sync`] does —
//! append the growth, rewrite on a truncation (the Esc-Esc backtrack), or
//! nothing at all, which is what keeps streaming chunks free of I/O.
//!
//! Two behaviours worth knowing before changing anything here:
//!
//! - **Deferred create.** No file exists until the first history item, so an
//!   idle session leaves nothing on disk — a checkpoint alone must not
//!   materialize one either.
//! - **Failures are swallowed.** Recording must never kill the TUI, so a
//!   failed write advances the watermark anyway and a later rewrite (which
//!   writes the whole history) repairs it.

use std::path::{Path, PathBuf};

use alter_zero::app::HistoryItem;
use alter_zero::checkpoint;
use alter_zero::session::{self, SessionMeta};

use super::host::{session_id, utc_stamp};
use super::resume::sessions_root;

/// Records the conversation to a rollout file as it happens — codex's
/// `RolloutRecorder`, sized down to this loop (see `docs/resume.md`).
///
/// The pure format lives in [`session`]; this owns the impurities: the root
/// dir from the environment, the local clock for the dated path, the UTC
/// write stamps, and the file appends. `recorded` is the on-disk watermark
/// (the scrollback `committed` pattern): [`sync`] appends history growth,
/// rewrites on a truncation (the Esc-Esc backtrack rewind), and does nothing
/// when the length is unchanged. Every failure is swallowed — recording must
/// never kill the TUI (codex logs and carries on the same way).
///
/// [`sync`]: SessionRecorder::sync
pub(crate) struct SessionRecorder {
    /// The sessions root (`~/.alter-zero/sessions`, or
    /// `ALTER_ZERO_SESSIONS_DIR` — the smoke test points it at a temp dir);
    /// `None` disables recording (no HOME and no override).
    root: Option<PathBuf>,
    /// The active session's file + meta, once anything was recorded — created
    /// lazily on the first item so empty sessions never touch disk (codex's
    /// deferred create). The meta is kept for rewrites.
    active: Option<(PathBuf, SessionMeta)>,
    /// How many history items are already on disk.
    recorded: usize,
    /// An adopted file's last line lost its newline (a torn write): the next
    /// append prefixes one so the first new item isn't glued onto it.
    repair_newline: bool,
    /// The filesystem checkpoints recorded in this session (`docs/checkpoint.md`)
    /// — an in-memory mirror of the `checkpoint` lines, kept so a truncation
    /// rewrite can re-emit the survivors and a resume can adopt the file's own.
    /// Appended interleaved with the item lines.
    checkpoints: Vec<checkpoint::Checkpoint>,
    /// How many of [`checkpoints`](Self::checkpoints) are already on disk — the
    /// checkpoint twin of `recorded`, so an append only writes the new ones.
    checkpoints_written: usize,
    /// The meta context of a *new* session file, captured once at startup.
    cwd: String,
    model: String,
    /// The hooks' view of the rollout path (`docs/hooks.md`): published on
    /// every `active` transition, so a payload's `transcript_path` names the
    /// file this conversation is actually recorded to — `None` while no file
    /// exists (deferred create), exactly the contract's nullable field.
    transcript: alter_zero::llm::hooks::TranscriptCell,
    /// The `App::history_generation` this recorder last mirrored. The length
    /// watermark alone cannot see a **replacement** — a blocked prompt's
    /// rollback removes the submission *and* records the notice, so
    /// `history.len()` holds still while the content changed, and the file
    /// used to keep the censored prompt and lose the notice (the resume-leak
    /// bug an independent review caught). Any generation change forces a
    /// full rewrite.
    generation: u64,
}

impl SessionRecorder {
    pub(crate) fn new(model: &str, cwd: &Path) -> Self {
        Self {
            root: sessions_root(),
            active: None,
            recorded: 0,
            repair_newline: false,
            checkpoints: Vec::new(),
            checkpoints_written: 0,
            cwd: cwd.display().to_string(),
            model: model.to_string(),
            transcript: alter_zero::llm::hooks::TranscriptCell::default(),
            generation: 0,
        }
    }

    /// Share the rollout path with the hook sink (`docs/hooks.md`): the cell
    /// is re-published on every `active` transition — the lazy create, a
    /// `/clear`'s reset, a `/resume`'s adopt.
    pub(crate) fn with_transcript(mut self, cell: alter_zero::llm::hooks::TranscriptCell) -> Self {
        self.transcript = cell;
        self.publish_transcript();
        self
    }

    /// Mirror the active path into the shared cell.
    fn publish_transcript(&self) {
        if let Ok(mut cell) = self.transcript.write() {
            *cell = self
                .active
                .as_ref()
                .map(|(path, _)| path.display().to_string());
        }
    }

    /// Record a filesystem checkpoint against the current conversation length
    /// (`docs/checkpoint.md`): held in memory now, flushed to the file by the
    /// next [`sync`](Self::sync) (a turn always grows history, so the flush
    /// rides that append — and a session that never grows history never
    /// materializes a file, keeping codex's deferred create).
    pub(crate) fn record_checkpoint(&mut self, checkpoint: checkpoint::Checkpoint) {
        self.checkpoints.push(checkpoint);
    }

    /// The checkpoints known this session — the loop reads these to pick the
    /// restore target for an Esc-Esc backtrack (`docs/backtrack.md`).
    pub(crate) fn checkpoints(&self) -> &[checkpoint::Checkpoint] {
        &self.checkpoints
    }

    /// The sessions root, for the `/resume` scan.
    pub(crate) fn root(&self) -> Option<&Path> {
        self.root.as_deref()
    }

    /// The file currently being written, if any — excluded from the picker
    /// (the session you're in is not something to "return" to).
    pub(crate) fn active_path(&self) -> Option<&Path> {
        self.active.as_ref().map(|(path, _)| path.as_path())
    }

    /// The active session's id — the `--resume` handle the exit hint prints
    /// (`docs/cli.md`). `None` until anything was recorded (deferred create),
    /// so an empty session advertises nothing.
    pub(crate) fn session_id(&self) -> Option<&str> {
        self.active.as_ref().map(|(_, meta)| meta.id.as_str())
    }

    /// Mirror the file to `history`: append newly finished items (creating
    /// the file + meta line first on the very first one) plus any pending
    /// checkpoint lines (`docs/checkpoint.md`), rewrite the whole file when
    /// history shrank (the backtrack rewind), no-op when unchanged. History is
    /// append-or-truncate only, so the watermark compare is sound. A checkpoint
    /// alone never creates a file (deferred create).
    pub(crate) fn sync(&mut self, history: &[HistoryItem], generation: u64) {
        // A non-append mutation happened (a backtrack, an interrupt-undo, a
        // blocked prompt's pop-and-notice, a `/clear`): re-serialize the
        // whole file. The length compare below cannot see a replacement —
        // `block_prompt` removes the submission and records the notice in
        // one mutation, leaving `history.len()` unchanged — which is exactly
        // why `App::history_generation` exists, and why the recorder keys on
        // it (docs/hooks.md).
        if generation != self.generation {
            self.generation = generation;
            if self.active.is_some() {
                self.rewrite(history);
                return;
            }
            // No file yet (deferred create, or a `/clear`'s fresh session):
            // nothing to rewrite — `recorded` is 0 whenever `active` is
            // `None`, so the append below records whatever the mutation
            // left, creating the file only when there is an item to write.
        }
        if history.len() < self.recorded {
            self.rewrite(history);
            return;
        }
        let items_grew = history.len() > self.recorded;
        let checkpoints_pending = self.checkpoints.len() > self.checkpoints_written;
        // Nothing new to write — neither items nor checkpoints.
        if !items_grew && !checkpoints_pending {
            return;
        }
        // A checkpoint with no file yet (the startup/`/clear` pristine snapshot)
        // must NOT materialize a rollout file — codex's deferred create: an
        // empty session leaves no file. Hold it in memory until a history item
        // creates the file, at which point `append` flushes it alongside. A
        // pending checkpoint with a file that already exists does flush now.
        if !items_grew && self.active.is_none() {
            return;
        }
        let fresh = &history[self.recorded..];
        // Advance the watermark whether or not the write lands: a failed
        // append drops those lines (a later rewrite restores them, since it
        // writes the full history) instead of retrying every event.
        self.recorded = history.len();
        self.append(fresh);
    }

    /// `/clear` starts a fresh session (codex's `/new`): the next recorded
    /// item creates a new file; the old file keeps what it had.
    pub(crate) fn start_new(&mut self) {
        self.active = None;
        self.recorded = 0;
        self.repair_newline = false;
        self.checkpoints.clear();
        self.checkpoints_written = 0;
        self.publish_transcript();
    }

    /// Adopt a resumed session's file: further items append there (codex's
    /// resume-mode open), and a rewrite re-serializes its own meta. `torn`
    /// flags a file whose last line lost its newline — the next append
    /// repairs it first. `checkpoints` are the file's own recorded snapshots
    /// (`docs/checkpoint.md`), taken over as already-written so later turns
    /// extend the same chain and a backtrack after resuming restores against
    /// them.
    pub(crate) fn adopt(
        &mut self,
        path: PathBuf,
        meta: SessionMeta,
        recorded: usize,
        torn: bool,
        checkpoints: Vec<checkpoint::Checkpoint>,
        generation: u64,
    ) {
        self.active = Some((path, meta));
        self.recorded = recorded;
        self.repair_newline = torn;
        self.checkpoints_written = checkpoints.len();
        self.checkpoints = checkpoints;
        // The load that adopted this file bumped the generation; swallowing
        // it here keeps the next sync on the append path instead of
        // pointlessly rewriting the file that was just read.
        self.generation = generation;
        self.publish_transcript();
    }

    /// Append `items` as rollout lines, materializing the file (date dirs +
    /// meta line) on the first-ever append. Failures are dropped.
    fn append(&mut self, items: &[HistoryItem]) {
        use std::io::Write;
        if self.active.is_none() {
            self.active = self.create_session();
            self.publish_transcript();
        }
        let Some((path, _)) = self.active.as_ref() else {
            return; // recording disabled, or the create failed
        };
        let Ok(mut file) = std::fs::OpenOptions::new().append(true).open(path) else {
            return;
        };
        let stamp = utc_stamp();
        let mut text = String::new();
        // Terminate an adopted file's torn last line so the first new item
        // starts a line of its own (the junk stays, skipped by the reader).
        if std::mem::take(&mut self.repair_newline) {
            text.push('\n');
        }
        for item in items {
            text.push_str(&session::item_line(item, &stamp));
            text.push('\n');
        }
        // Flush any checkpoints recorded since the last append (docs/checkpoint.md)
        // — interleaved with the item lines; parsed back by their own reader.
        for checkpoint in &self.checkpoints[self.checkpoints_written..] {
            text.push_str(&session::checkpoint_line(checkpoint, &stamp));
            text.push('\n');
        }
        self.checkpoints_written = self.checkpoints.len();
        let _ = file.write_all(text.as_bytes());
    }

    /// Create the session file lazily: derive the dated path from the local
    /// clock (codex's layout), create the date dirs, and write the meta line.
    /// `None` when recording is disabled or any I/O fails.
    fn create_session(&self) -> Option<(PathBuf, SessionMeta)> {
        use chrono::{Datelike, Timelike};
        let root = self.root.as_ref()?;
        let now = chrono::Local::now();
        let id = session_id();
        let rel = session::rollout_rel_path(
            (now.year(), now.month(), now.day()),
            (now.hour(), now.minute(), now.second()),
            &id,
        );
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent()?).ok()?;
        let meta = SessionMeta {
            id,
            timestamp: utc_stamp(),
            cwd: self.cwd.clone(),
            model: self.model.clone(),
            originator: "alter-zero".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
        };
        let first_line = format!("{}\n", session::meta_line(&meta, &meta.timestamp));
        std::fs::write(&path, first_line).ok()?;
        Some((path, meta))
    }

    /// Rewrite the whole file (meta + every item) — the truncation path: the
    /// Esc-Esc backtrack rewound history, and the file must follow. (This
    /// normalizes the file to what this build parsed — lines a future build
    /// wrote and this one skipped are dropped; see `docs/resume.md`.)
    fn rewrite(&mut self, history: &[HistoryItem]) {
        self.recorded = history.len();
        self.repair_newline = false;
        // A truncation (backtrack) drops the checkpoints describing the
        // rewound-away future, so the file mirrors the survivors
        // (docs/checkpoint.md), in lockstep with the in-memory list.
        checkpoint::retain_surviving(&mut self.checkpoints, history.len());
        self.checkpoints_written = self.checkpoints.len();
        let Some((path, meta)) = self.active.as_ref() else {
            return;
        };
        let stamp = utc_stamp();
        let mut text = format!("{}\n", session::meta_line(meta, &meta.timestamp));
        for item in history {
            text.push_str(&session::item_line(item, &stamp));
            text.push('\n');
        }
        for checkpoint in &self.checkpoints {
            text.push_str(&session::checkpoint_line(checkpoint, &stamp));
            text.push('\n');
        }
        let _ = std::fs::write(path, text);
    }
}
