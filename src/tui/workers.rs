//! The loop's off-thread workers: three jobs that must not run on the event
//! loop, each on a short-lived `std::thread` reporting back over a channel the
//! `select!` polls.
//!
//! They exist for one reason: every one of them blocks for long enough to be
//! *seen*. A clipboard image's PNG encode, a `/v1/models` round trip, and a
//! walk of the working directory all take tens to hundreds of milliseconds —
//! inline, that freezes the spinner and swallows keystrokes. Off-thread, the
//! loop keeps animating and the result arrives as just another event.
//!
//! - [`spawn_file_search_worker`] — the `@` picker's walk + rank
//!   (`docs/file-search.md`), fed by [`dispatch_file_search`].
//! - [`spawn_image_paste`] — one Ctrl+V clipboard read (`docs/image-paste.md`).
//! - [`spawn_model_fetch`] — one provider's model list, for the `/model`
//!   picker and the startup capability probe (`docs/llm.md`).
//! - [`spawn_device_login`] — the `/login` subscription sign-in
//!   (`docs/copilot.md`), the one worker that runs for **minutes** rather
//!   than milliseconds: it polls the provider until the user approves the
//!   code, or `cancel` trips.
//!
//! **Invariant 1:** a worker only ever *sends*. None of them reads stdin, so
//! the `EventStream` stays the single stdin reader.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::thread::JoinHandle;

use alter_zero::app::{App, ToastKind};
use alter_zero::clipboard;
use alter_zero::file_search::{FileMatch, rank_files};
use alter_zero::llm::{self, ModelConfig, ModelEntry};
use alter_zero::stream::CancelToken;

use super::Session;

/// A finished `/model` fetch from one provider: its human label (for a failure
/// note) and either the provider's models or a one-line error. The picker
/// fetches every configured provider in parallel and merges these as they land.
/// Carried on the model-fetch worker's channel. See `docs/llm.md`.
pub(crate) type ModelFetch = (String, Result<Vec<ModelEntry>, String>);

/// What the `/login` device-flow worker reports back (`docs/copilot.md`).
/// Two messages, in order: the code to show, then the flow's verdict.
#[derive(Debug)]
pub(crate) enum DeviceEvent {
    /// The provider issued a code — show it, and start counting it down.
    Code {
        /// Where the user enters it.
        verification_uri: String,
        /// The one-time code itself.
        user_code: String,
        /// When it stops being valid, for the page's countdown.
        expires_at: std::time::Instant,
    },
    /// The flow finished: the long-lived OAuth token to persist, or why not.
    Done(Result<String, String>),
}

/// Run GitHub's device flow on a worker thread, reporting the code and then
/// the verdict. Long-running by nature — the poll only ends when the user
/// approves, the code expires, or `cancel` trips (Esc on the page, or the
/// flow being closed out from under it). See `docs/copilot.md`.
pub(crate) fn spawn_device_login(
    cancel: CancelToken,
    tx: tokio::sync::mpsc::UnboundedSender<DeviceEvent>,
) {
    std::thread::spawn(move || {
        let device = match llm::copilot::request_device_code() {
            Ok(device) => device,
            Err(e) => {
                let _ = tx.send(DeviceEvent::Done(Err(e.to_string())));
                return;
            }
        };
        let expires_at =
            std::time::Instant::now() + std::time::Duration::from_secs(device.expires_in);
        if tx
            .send(DeviceEvent::Code {
                verification_uri: device.verification_uri.clone(),
                user_code: device.user_code.clone(),
                expires_at,
            })
            .is_err()
        {
            return;
        }
        // Approval is not access. GitHub's device flow authenticates the
        // *user*; whether that user can actually call Copilot is only settled
        // by the token exchange — an account with no subscription, or one an
        // org's SSO has not authorised, signs in perfectly and fails later. So
        // verify here, while the page that can explain it is still up, rather
        // than reporting "Signed in ✓" and letting `/model` fail cryptically a
        // minute later (`docs/copilot.md`).
        let result = llm::copilot::poll_for_token(&device, &cancel).and_then(|token| {
            llm::copilot::authorize(&token)
                .map(|_| token)
                .map_err(|e| e.to_string())
        });
        // A cancelled flow has no page left to report to — the user already
        // walked away from it.
        if !cancel.is_cancelled() {
            let _ = tx.send(DeviceEvent::Done(result));
        }
    });
}

/// Max files the `@` picker's worker indexes — bounds each walk's memory/time
/// (codex's nucleo walk is similarly capped).
const FILE_INDEX_CAP: usize = 10_000;
/// Max ranked matches returned per query (the picker shows up to this many).
const FILE_MENU_LIMIT: usize = 8;
/// Directory names the walk skips, on top of every hidden (dotfile) entry.
const FILE_WALK_DENYLIST: &[&str] = &["target", "node_modules"];

/// A file-search result for the `@` picker: the `query` it answers (for the
/// staleness guard in `App::set_file_matches`) and its ranked `matches`.
pub(crate) struct FileSearchResult {
    query: String,
    matches: Vec<FileMatch>,
}

/// Walk `root` breadth-first collecting up to `cap` relative paths, skipping
/// hidden entries (dotfiles — so `.git` too) and [`FILE_WALK_DENYLIST`]
/// directories; directories are listed with a trailing `/`. Dependency-free (the
/// agreed design — no `.gitignore` parsing). See `docs/file-search.md`.
fn walk_files(root: &Path, cap: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut queue = VecDeque::new();
    queue.push_back(root.to_path_buf());
    while let Some(dir) = queue.pop_front() {
        if out.len() >= cap {
            break;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.') || FILE_WALK_DENYLIST.contains(&name.as_ref()) {
                continue;
            }
            let path = entry.path();
            let rel = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                out.push(format!("{rel}/"));
                queue.push_back(path);
            } else {
                out.push(rel);
            }
            if out.len() >= cap {
                break;
            }
        }
    }
    out
}

/// Spawn the `@` file-search worker: a background thread that, for each query —
/// coalescing any that queued while it worked (the debounce) — walks `root`
/// **afresh** and ranks the result ([`rank_files`]), sending a
/// [`FileSearchResult`] back. The walk is per-query (bounded by
/// [`FILE_INDEX_CAP`]) rather than cached at startup, so a file the agent just
/// created — or anything else new on disk — appears in the picker immediately;
/// codex gets the same freshness by starting a new walk per `@`-token session.
/// Exits when the request channel closes (app exit).
/// It only *sends* on the tokio channel; it never reads stdin (invariant 1).
pub(crate) fn spawn_file_search_worker(
    root: PathBuf,
    req_rx: std::sync::mpsc::Receiver<String>,
    res_tx: tokio::sync::mpsc::UnboundedSender<FileSearchResult>,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
        while let Ok(mut query) = req_rx.recv() {
            // Coalesce: if the user kept typing, only serve the newest query.
            while let Ok(newer) = req_rx.try_recv() {
                query = newer;
            }
            // Walk fresh every time — a startup-cached list went stale the
            // moment the agent (or anyone) created a file, so `@` in a fresh
            // dir only ever showed the boot-time contents.
            let files = walk_files(&root, FILE_INDEX_CAP);
            let matches = rank_files(&query, &files, FILE_MENU_LIMIT);
            if res_tx.send(FileSearchResult { query, matches }).is_err() {
                break; // the loop is gone
            }
        }
    })
}

/// Dispatch a file search when the active `@token` query changes — codex's
/// `StartFileSearch` on every token change. `last` is the boundary's record of
/// the query last sent, so an unchanged query (or a non-edit key) sends nothing.
pub(crate) fn dispatch_file_search(
    app: &App,
    req_tx: &std::sync::mpsc::Sender<String>,
    last: &mut Option<String>,
) {
    let query = app.file_search_query();
    if query.as_deref() != last.as_deref() {
        if let Some(q) = &query {
            let _ = req_tx.send(q.clone());
        }
        *last = query;
    }
}

/// Run one Ctrl+V clipboard read ([`clipboard::read_clipboard_image`]) on a
/// short-lived background thread, delivering the result on the loop's image
/// channel (`select!` branch 5). Detached: at quit a straggler finishes
/// writing a temp file harmlessly (like the shell pipe readers); it only
/// *sends* — never a stdin reader (invariant 1). Each Ctrl+V spawns its own
/// worker, so a double-press attaches two placeholders in completion order —
/// what codex's synchronous handler does too, minus the UI freeze.
pub(crate) fn spawn_image_paste(tx: tokio::sync::mpsc::UnboundedSender<Result<PathBuf, String>>) {
    std::thread::spawn(move || {
        // The receiver only closes at shutdown — a failed send just means
        // there is nothing left to attach to.
        let _ = tx.send(clipboard::read_clipboard_image());
    });
}

/// Fetch one provider's `/models` list on a background thread (the image-paste
/// pattern), sending the labelled result to the loop. One of these is spawned
/// per configured provider so they fetch in parallel; a cancelled fetch (the
/// picker closed) is dropped. See `docs/llm.md`.
pub(crate) fn spawn_model_fetch(
    label: String,
    cfg: Option<ModelConfig>,
    cancel: CancelToken,
    tx: tokio::sync::mpsc::UnboundedSender<ModelFetch>,
) {
    std::thread::spawn(move || {
        let result: Result<Vec<ModelEntry>, String> = match cfg {
            Some(cfg) => llm::models::fetch_models(&cfg, &cancel).map_err(|e| e.to_string()),
            None => Err("No provider configured — set providers.toml / ALTER_ZERO_PROVIDER".into()),
        };
        // Don't deliver a result the picker no longer wants.
        if !cancel.is_cancelled() {
            let _ = tx.send((label, result));
        }
    });
}

impl Session<'_> {
    /// Kick off a file search when the active `@token` query changed — codex's
    /// `StartFileSearch` on every token change. The last query sent is the
    /// boundary's own record, so an unchanged query (or a non-edit key) sends
    /// nothing.
    pub(crate) fn dispatch_file_search(&mut self) {
        dispatch_file_search(&self.app, &self.file_req_tx, &mut self.last_file_query);
    }

    /// A file-search result from the worker: feed it into the open `@` picker
    /// (stale results — the token moved on — are dropped by `set_file_matches`),
    /// then repaint. See `docs/file-search.md`.
    pub(crate) fn on_file_matches(&mut self, result: FileSearchResult) {
        self.app.set_file_matches(&result.query, result.matches);
        self.frame.schedule_frame();
    }

    /// A finished Ctrl+V clipboard read: attach the temp image to the composer, or
    /// surface the red failure notice. Committing is view-gated (invariant 4) —
    /// the result may arrive with an overlay up, where the notice is recorded only
    /// and the return repaints it from history. See `docs/image-paste.md`.
    pub(crate) fn on_image_paste(&mut self, result: Result<PathBuf, String>) {
        match result {
            Ok(path) => {
                self.app.attach_image(path);
                // A known non-vision model can't see the paste: warn at once with
                // a toast — the attachment still rides the request as a text note
                // the model reads, so it can tell the user too (docs/tools.md).
                if self.models.is_blind() {
                    let model = self.models.active_model().to_string();
                    self.toast(
                        format!("{model} does not support image input"),
                        ToastKind::Error,
                    );
                }
            }
            Err(reason) => {
                let text = format!("Failed to paste image: {reason}");
                if self.commits_allowed() {
                    self.commit_error_notice(&text);
                } else {
                    self.app.record_error_message(&text);
                }
            }
        }
        // The inserted placeholder may sit in an `@token` — re-derive the picker
        // like any other composer edit.
        self.dispatch_file_search();
        self.frame.schedule_frame();
    }
}
