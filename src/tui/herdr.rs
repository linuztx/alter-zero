//! herdr support at the boundary (`docs/herdr.md`): finding the pane, the
//! socket worker that tells herdr what this session is doing, and the
//! loop-bottom sync that feeds it.
//!
//! Three rules hold it to herdr's own advice ("don't let Herdr slow your agent
//! down"):
//!
//! - **The loop never touches the socket.** [`Session::sync_herdr`] runs at
//!   every loop bottom, derives the state (`herdr::status`, pure) and posts a
//!   report only when it changed — a compare on the common path, no I/O. One
//!   detached worker thread writes the requests, one at a time, so herdr
//!   applies them in the order they were made.
//! - **Only the newest report matters.** The worker takes from an
//!   [`Outbox`] that keeps a single unsent report: a burst of changes while
//!   herdr is slow collapses to the last one rather than queuing behind it.
//!   Every send gets a fresh `seq` (`herdr::next_seq`), since herdr drops a
//!   report that does not outrank the last one it took from this source.
//! - **Every failure is silent, bounded, and repaired.** A request is one
//!   connection, one line out, one line back, each under
//!   [`REQUEST_TIMEOUT`]; a dead or absent herdr costs nothing the user sees.
//!   A failed send is retried on `herdr::resend_after`'s backoff, and a
//!   delivered one re-sent every `herdr::KEEPALIVE` — herdr holds a
//!   self-reported state with no expiry, so a report it lost would otherwise
//!   stand wrong until the next change. The quit waits at most
//!   [`RELEASE_WAIT`] for the release to land.

use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use alter_zero::herdr::{self, Job, Outbox, Pane, Report};

use super::Session;

/// How long one request may take to write, and its reply to arrive.
const REQUEST_TIMEOUT: Duration = Duration::from_millis(500);

/// How long a quit waits for the release to reach herdr before the process
/// exits anyway — a request in flight plus the release itself, each well
/// under this on a live herdr.
const RELEASE_WAIT: Duration = Duration::from_millis(400);

/// The session's line to herdr: what it last posted, and the worker that
/// writes it.
pub(crate) struct HerdrReporter {
    /// The resume command's first word — this binary's name, when that name
    /// is on `PATH` for herdr to run (`None` names the session but offers no
    /// way back).
    resume_bin: Option<String>,
    /// The last report posted, so an unchanged loop iteration posts nothing.
    posted: Option<Report>,
    mailbox: Arc<Mailbox>,
    /// Signalled by the worker once the release is written.
    released_rx: mpsc::Receiver<()>,
}

/// The [`Outbox`] the worker drains, behind its lock and wake-up.
struct Mailbox {
    outbox: Mutex<Outbox>,
    ready: Condvar,
}

impl HerdrReporter {
    /// The reporter for the herdr pane this process runs in — `None` outside
    /// herdr, with `ALTER_ZERO_HERDR` off, or off Unix — with its worker
    /// started.
    pub(crate) fn start() -> Option<Self> {
        if !cfg!(unix) {
            return None;
        }
        let pane = herdr::pane(|key| std::env::var(key).ok())?;
        let mailbox = Arc::new(Mailbox {
            outbox: Mutex::new(Outbox::default()),
            ready: Condvar::new(),
        });
        let (released_tx, released_rx) = mpsc::channel();
        let worker_mailbox = Arc::clone(&mailbox);
        std::thread::Builder::new()
            .name("herdr".to_string())
            .spawn(move || run_worker(&pane, &worker_mailbox, &released_tx))
            .ok()?;
        Some(Self {
            resume_bin: resume_bin(),
            posted: None,
            mailbox,
            released_rx,
        })
    }

    /// Post the report for `status` in the recorded `session`, when it
    /// differs from the last one posted — a state change, or a new session
    /// (the first message recorded, a `/resume`, a `/clear`).
    fn sync(&mut self, status: herdr::Status, session: Option<&str>) {
        // The common path — nothing changed — compares without building the
        // report (its session copy and resume command are allocations, and
        // this runs every loop iteration, at up to 120 a second).
        if self
            .posted
            .as_ref()
            .is_some_and(|posted| posted.status == status && posted.session.as_deref() == session)
        {
            return;
        }
        let report = Report::new(status, session, self.resume_bin.as_deref());
        if let Ok(mut outbox) = self.mailbox.outbox.lock() {
            outbox.post(report.clone());
        }
        self.mailbox.ready.notify_one();
        self.posted = Some(report);
    }

    /// Hand the pane back to herdr on the way out, waiting at most
    /// [`RELEASE_WAIT`] for the worker to write it. The release replaces any
    /// report the worker has not sent, and nothing is reported after it.
    pub(crate) fn release(&mut self) {
        if let Ok(mut outbox) = self.mailbox.outbox.lock() {
            outbox.release();
        }
        self.mailbox.ready.notify_one();
        let _ = self.released_rx.recv_timeout(RELEASE_WAIT);
    }
}

impl Session<'_> {
    /// Tell herdr what the session is doing now, when that changed — run at
    /// every loop bottom (after the recorder's sync, so a conversation's
    /// first message names the session its file was just created under) and
    /// once at the end of bootstrap. A no-op outside herdr.
    pub(crate) fn sync_herdr(&mut self) {
        let Some(reporter) = self.herdr.as_mut() else {
            return;
        };
        reporter.sync(herdr::status(&self.app), self.recorder.session_id());
    }
}

/// The worker: take the next job and write it — or, when none comes before
/// the resend is due, write the current report again — until the release.
///
/// It owns the sequence (one writer, so every request outranks the last) and
/// the current report, which a resend writes again under a fresh `seq`.
fn run_worker(pane: &Pane, mailbox: &Mailbox, released: &mpsc::Sender<()>) {
    let mut seq = 0u64;
    let mut current: Option<Report> = None;
    let mut failures = 0u32;
    let mut due = Instant::now() + herdr::resend_after(0);
    loop {
        let report = match next_work(mailbox, due, current.is_some()) {
            None => return,
            Some(Work::Post(Job::Release)) => {
                seq = herdr::next_seq(seq, unix_micros());
                let request = herdr::release_request(pane, seq);
                let _ = herdr::send(&pane.socket, &request, REQUEST_TIMEOUT);
                let _ = released.send(());
                return;
            }
            Some(Work::Post(Job::Report(report))) => report,
            Some(Work::Resend) => match current.take() {
                Some(report) => report,
                None => continue,
            },
        };
        seq = herdr::next_seq(seq, unix_micros());
        let request = herdr::report_request(pane, &report, seq);
        // A refused or timed-out report is retried on the backoff; a
        // delivered one rests until the keepalive.
        failures = if herdr::send(&pane.socket, &request, REQUEST_TIMEOUT).is_ok() {
            0
        } else {
            failures.saturating_add(1)
        };
        current = Some(report);
        due = Instant::now() + herdr::resend_after(failures);
    }
}

/// What the worker does next.
enum Work {
    /// Write what the loop posted.
    Post(Job),
    /// Write the current report again: the resend came due with nothing new.
    Resend,
}

/// Block until there is work: a posted job, or — once `due` passes with a
/// report to repeat — a resend. `None` when the mailbox is poisoned (the loop
/// thread panicked holding it; there is no one left to report for).
fn next_work(mailbox: &Mailbox, due: Instant, resendable: bool) -> Option<Work> {
    let mut outbox = mailbox.outbox.lock().ok()?;
    loop {
        if let Some(job) = outbox.take() {
            return Some(Work::Post(job));
        }
        let now = Instant::now();
        if resendable && now >= due {
            return Some(Work::Resend);
        }
        outbox = if resendable {
            mailbox.ready.wait_timeout(outbox, due - now).ok()?.0
        } else {
            mailbox.ready.wait(outbox).ok()?
        };
    }
}

/// Microseconds since the Unix epoch — the clock `herdr::next_seq` follows,
/// in herdr's own reporters' unit, so a relaunch in the same pane outranks
/// the process before it.
fn unix_micros() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_micros()).unwrap_or(u64::MAX)
        })
}

/// The resume command's first word: the name this binary was invoked by, when
/// an executable of that name is on `PATH` — herdr types the resume command
/// into the pane's shell, so a name it cannot find there would fail exactly
/// when it is needed.
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
