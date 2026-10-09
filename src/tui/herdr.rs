//! herdr support at the boundary (`docs/herdr.md`): finding the pane and the
//! name herdr can resume this binary by, and feeding the library's
//! [`Tracker`] what the session is doing at every loop bottom, its reports
//! into the [`Reporter`]'s worker.
//!
//! The loop never touches the socket: [`Session::sync_herdr`] derives the
//! session's activity (`herdr::activity`, pure) and posts a report only when
//! the tracker says something changed — a compare on the common path, no
//! I/O — and the worker writes it, retries a failed one, keeps a delivered
//! one alive and releases the pane last, under bounded waits throughout.

use std::path::PathBuf;

use alter_zero::herdr::{self, Reporter, Tracker};

use super::Session;

/// The session's line to herdr: what to tell it, and the worker that does.
pub(crate) struct HerdrReporter {
    tracker: Tracker,
    reporter: Reporter,
}

impl HerdrReporter {
    /// The reporter for the herdr pane this process runs in, its worker
    /// started — `None` outside herdr, with `ALTER_ZERO_HERDR` off, off Unix,
    /// or when the thread cannot be started.
    pub(crate) fn start() -> Option<Self> {
        if !cfg!(unix) {
            return None;
        }
        let pane = herdr::pane(|key| std::env::var(key).ok())?;
        Some(Self {
            tracker: Tracker::new(resume_bin()),
            reporter: Reporter::spawn(pane)?,
        })
    }

    /// Hand the pane back to herdr on the way out, waiting at most
    /// `herdr::RELEASE_WAIT` for it to land. (Dropping the reporter releases
    /// it too, which is what covers a loop that bails out on an error.)
    pub(crate) fn release(&mut self) {
        self.reporter.release(herdr::RELEASE_WAIT);
    }
}

impl Session<'_> {
    /// Tell herdr what the session is doing now, when that changed — run at
    /// every loop bottom (after the recorder's sync, so a conversation's
    /// first message names the session its file was just created under) and
    /// once at the end of bootstrap. A no-op outside herdr.
    pub(crate) fn sync_herdr(&mut self) {
        let Some(herdr) = self.herdr.as_mut() else {
            return;
        };
        let activity = herdr::activity(&self.app);
        if let Some(report) = herdr.tracker.update(&activity, self.recorder.session_id()) {
            herdr.reporter.post(report);
        }
    }

    /// The turn ended on a backend `error`: hold the pane blocked on it —
    /// what herdr flags as needing attention, where idle would announce the
    /// work done — until a turn starts again or the conversation is replaced
    /// or rewound.
    pub(crate) fn herdr_turn_failed(&mut self, error: &str) {
        if let Some(herdr) = self.herdr.as_mut() {
            herdr.tracker.fail(error, self.app.history_generation());
        }
    }
}

/// The resume command's first word: the name this binary was invoked by, when
/// an executable of that name is on `PATH` — herdr types the resume command
/// into the pane's shell, so a name it cannot find there would fail exactly
/// when it is needed (and herdr replays no history for a pane that has one).
fn resume_bin() -> Option<String> {
    let arg0 = std::env::args().next();
    let bin = alter_zero::cli::bin_name(arg0.as_deref());
    alter_zero::subprocess::find_on_path(
        &bin,
        std::env::var_os("PATH").as_deref(),
        alter_zero::subprocess::is_executable_file,
    )
    .map(|_: PathBuf| bin)
}
