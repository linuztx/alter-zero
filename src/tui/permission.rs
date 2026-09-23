//! The tool-permission gate as the loop sees it (`docs/permissions.md`).
//!
//! A `write`/`edit`/`bash` call raises the inline prompt and its own thread
//! **blocks** on [`PermissionGate`] until the user answers. Three things had to
//! travel together for that to work — the gate, the file its rules persist to,
//! and this project's key inside that file — so [`PermissionStore`] holds all
//! three and every method that changes a rule persists it in the same breath.
//! There is no way to grant a rule and forget to save it.
//!
//! Two answers reach past the request that raised them:
//!
//! - **"Don't ask again" sweeps the queue.** Parallel agents all ask before any
//!   one is answered, so a new rule immediately approves the requests already
//!   waiting behind it ([`PermissionStore::approve_covered`]) — else three
//!   agents running the same command ask three times after you said not to.
//! - **An abandoned request must be released.** A prompt dropped without an
//!   answer (Esc, `/clear`) leaves a thread parked forever; a cancelled turn's
//!   thread reaps itself, but a background agent's has nothing to cancel it, so
//!   the loop bottom denies them explicitly
//!   ([`PermissionStore::release_abandoned`]).
//!
//! `ALTER_ZERO_PERMISSIONS=0` starts the session with **no gate**: nothing
//! asks, every tool runs unasked, and the footer shows no mode (a mode with
//! nothing asking would be a lie).

use std::path::{Path, PathBuf};

use alter_zero::app::{App, ToastKind};
use alter_zero::permission::{
    PermissionDecision, PermissionGate, PermissionKind, PermissionMode, PermissionRequest,
};
use alter_zero::ui;

use super::{Session, config};

/// The session's permission gate together with where its rules persist.
pub(crate) struct PermissionStore {
    /// The shared gate every tool thread blocks on — `None` when permissions
    /// are disabled. One gate for the whole session, so "allow all edits" and
    /// "don't ask again for X" stick across turns and across a `/model` rebuild.
    gate: Option<PermissionGate>,
    /// `~/.alter-zero/permissions.json`; `None` disables persistence, and every
    /// session then starts asking afresh.
    path: Option<PathBuf>,
    /// This working directory — the file's key, so a rule granted in one
    /// project never leaks into another.
    project: String,
}

impl PermissionStore {
    /// Open the store for `cwd`: build the gate (unless
    /// `ALTER_ZERO_PERMISSIONS` is falsy) and seed it with this project's saved
    /// command rules and mode, so "don't ask again" survives a restart in the
    /// same directory.
    pub(crate) fn open(cwd: &Path) -> Self {
        let store = Self {
            gate: config::permissions_enabled().then(PermissionGate::new),
            path: config::permissions_file_path(),
            project: cwd.display().to_string(),
        };
        if let Some(gate) = store.gate.as_ref() {
            let saved = config::load_permissions(store.path.as_deref());
            if let Some(entry) = saved.project(&store.project) {
                let (prefixes, exact) = entry.command_sets();
                gate.seed_commands(prefixes, exact);
                gate.set_mode(entry.saved_mode());
            }
        }
        store
    }

    /// The gate to attach to a backend build — `None` when permissions are off,
    /// and every tool then runs unasked.
    pub(crate) fn gate(&self) -> Option<&PermissionGate> {
        self.gate.as_ref()
    }

    /// The mode the footer's right-edge segment shows; `None` hides it entirely
    /// (permissions disabled).
    pub(crate) fn mode(&self) -> Option<PermissionMode> {
        self.gate.as_ref().map(PermissionGate::mode)
    }

    /// Remember `request`'s scope as a standing per-project rule (option 2) and
    /// persist it. Called **before** the decision is posted, so the rule is in
    /// force when [`Self::approve_covered`] asks what it now covers — the tool
    /// thread would only remember once it woke.
    pub(crate) fn remember(&self, request: &PermissionRequest) {
        if let Some(gate) = self.gate.as_ref() {
            gate.remember(request);
            // A session's approval is not a rule — nothing new to save.
            if request.kind != PermissionKind::Session {
                self.save(gate);
            }
        }
    }

    /// Post a decision, waking the tool thread parked on it.
    pub(crate) fn resolve(&self, id: &str, decision: PermissionDecision) {
        if let Some(gate) = self.gate.as_ref() {
            gate.resolve(id, decision);
        }
    }

    /// Switch the session's mode (Shift+Tab) and persist it. The approve seam reads
    /// the gate's rules, so the very next `write`/`edit`/`bash` obeys the new
    /// posture.
    pub(crate) fn set_mode(&self, mode: PermissionMode) {
        if let Some(gate) = self.gate.as_ref() {
            gate.set_mode(mode);
            self.save(gate);
        }
    }

    /// Approve every queued request the current rules now cover — the sweep a
    /// new rule or a looser mode owes the prompts already waiting.
    pub(crate) fn approve_covered(&self, app: &mut App) {
        if let Some(gate) = self.gate.as_ref() {
            for id in app.drain_covered_permissions(&|r| gate.allows(r)) {
                gate.resolve(&id, PermissionDecision::Approve);
            }
        }
    }

    /// Deny every request `app` dropped without an answer, so no thread parks
    /// on a decision that is never coming.
    pub(crate) fn release_abandoned(&self, app: &mut App) {
        if let Some(gate) = self.gate.as_ref() {
            for id in app.take_abandoned_permissions() {
                gate.resolve(&id, PermissionDecision::Deny(None));
            }
        }
    }

    /// Drop every pending decision — `/clear`'s fresh slate, whose cancelled
    /// threads reap themselves, so no unclaimed answer may linger for the next
    /// turn.
    pub(crate) fn clear(&self) {
        if let Some(gate) = self.gate.as_ref() {
            gate.clear();
        }
    }

    /// Write this project's live rules back to the file — a read-modify-write
    /// (so other projects' entries survive) and best-effort, like every write
    /// at this boundary.
    fn save(&self, gate: &PermissionGate) {
        config::save_permissions(self.path.as_deref(), &self.project, &gate.rules());
    }
}

impl Session<'_> {
    /// The user answered the inline prompt: post the decision on the gate, waking
    /// the tool thread parked on it. The prompt is already closed and the composer
    /// draft restored (the pure core did that); a queued second request has
    /// already opened. See `docs/permissions.md`.
    pub(crate) fn resolve_permission(
        &mut self,
        request: &PermissionRequest,
        decision: PermissionDecision,
    ) {
        if decision == PermissionDecision::ApproveAlways {
            // Remember the scope BEFORE posting, so the standing rule is in force
            // when the sweep below asks what it now covers (the tool thread would
            // only remember once it woke). Option 2 is a standing, per-project
            // rule: it persists, mirrors an edit prompt's mode switch onto the
            // footer segment, and confirms with a toast.
            self.permissions.remember(request);
            let toast = match request.kind {
                PermissionKind::Bash | PermissionKind::Mcp => format!(
                    "Won't ask again for {} in this project",
                    ui::permission_remember_label(request)
                ),
                // Held in memory for as long as the session runs, never saved
                // (docs/interactive-shell.md).
                PermissionKind::Session => "Won't ask again for input to this session".to_string(),
                PermissionKind::Write | PermissionKind::Edit => {
                    self.app.set_permission_mode(Some(PermissionMode::Edit));
                    "Mode: edit — file edits run without asking (shift+tab to switch back)"
                        .to_string()
                }
            };
            self.toast(toast, ToastKind::Info);
        }
        self.permissions.resolve(&request.id, decision);
        // Parallel agents ask before any of them is answered, so "don't ask
        // again" has to reach the requests already queued behind this one — else
        // three agents running the same command ask three times after you said
        // not to.
        self.permissions.approve_covered(&mut self.app);
        self.frame.schedule_frame();
    }

    /// Shift+Tab (from the composer, or on an open bash prompt): `App`'s mode already
    /// advanced — mirror it onto the gate's rules (the approve seam reads them, so
    /// the very next write/edit obeys the new posture), sweep the queued requests
    /// the looser mode now covers (parallel agents' file changes waiting behind a
    /// prompt), and persist this project's entry. See `docs/permissions.md`.
    pub(crate) fn set_permission_mode(&mut self, mode: PermissionMode) {
        self.permissions.set_mode(mode);
        self.permissions.approve_covered(&mut self.app);
        self.toast(
            match mode {
                PermissionMode::Edit => {
                    "Mode: edit — file edits run without asking, commands still ask"
                }
                PermissionMode::Auto => {
                    "Mode: auto — file edits run; a classifier reviews commands and MCP tools"
                }
                PermissionMode::Master => "Mode: master — everything runs without asking",
                PermissionMode::Manual => "Mode: manual — asking before edits and commands",
            },
            ToastKind::Info,
        );
        self.frame.schedule_frame();
    }
}
