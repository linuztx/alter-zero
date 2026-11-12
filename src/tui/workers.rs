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
//!   (`docs/input/file-search.md`), fed by [`dispatch_file_search`].
//! - [`spawn_image_paste`] — one Ctrl+V clipboard read (`docs/input/image-paste.md`).
//! - [`spawn_model_fetch`] — one provider's model list, for the `/model`
//!   picker and the startup capability probe (`docs/providers/llm.md`).
//! - [`spawn_signin`] — the `/login` subscription sign-in (`docs/providers/copilot.md`,
//!   `docs/providers/chatgpt.md`, `docs/providers/claude.md`, `docs/providers/cline.md`), chosen by the
//!   row's auth scheme — the one worker that runs for **minutes** rather than
//!   milliseconds: it waits on the provider until the user approves, or
//!   `cancel` trips.
//! - [`spawn_usage_fetch`] — one subscription's usage, for the `/usage` page
//!   (`docs/providers/usage.md`).
//! - [`spawn_telemetry_ping`] — the once-a-day anonymous usage ping
//!   (`docs/ops/telemetry.md`), fire-and-forget: it reports only success, so the
//!   loop can record the day, and a dead collector costs the user nothing.
//!
//! **Invariant 1:** a worker only ever *sends*. None of them reads stdin, so
//! the `EventStream` stays the single stdin reader.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::thread::JoinHandle;

use alter_zero::app::{App, ServerTest, SigninKind, ToastKind};
use alter_zero::clipboard;
use alter_zero::file_search::{FileMatch, rank_files, sibling_order};
use alter_zero::llm::{self, AuthScheme, ModelConfig, ModelEntry};
use alter_zero::stream::CancelToken;

use super::Session;

/// A finished `/model` fetch from one provider: its human label (for a failure
/// note) and either the provider's models or a one-line error. The picker
/// fetches every configured provider in parallel and merges these as they land.
/// Carried on the model-fetch worker's channel. See `docs/providers/llm.md`.
pub(crate) type ModelFetch = (String, Result<Vec<ModelEntry>, String>);

/// What the `/login` workers report back. A sign-in sends two messages, in
/// order — the code to show, then the flow's verdict (`docs/providers/copilot.md`); a
/// local server's connection test sends one, its verdict
/// (`docs/providers/local-models.md`).
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
    /// The flow finished: the long-lived OAuth token to persist plus the seat
    /// it turned out to be, or why not.
    Done(Result<(String, Option<String>), String>),
    /// A local server's connection test finished: the test it answers (the
    /// page drops an answer to one it moved on from) and what it found.
    Tested {
        /// The address and token tried.
        test: ServerTest,
        /// The listing's outcome, worded for the page.
        verdict: llm::local::Verdict,
    },
}

/// Test a local server's connection on a worker thread: list its models the
/// way `/model` will (`llm::models::fetch_models`, over `cfg`), and report
/// the verdict — unless `cancel` tripped meanwhile (Esc stopped the test, or
/// the page went away), when nothing is sent. `name` and `label` are what
/// the verdict's sentences call the server. See `docs/providers/local-models.md`.
pub(crate) fn spawn_server_test(
    cfg: ModelConfig,
    name: String,
    label: String,
    test: ServerTest,
    cancel: CancelToken,
    tx: tokio::sync::mpsc::UnboundedSender<DeviceEvent>,
) {
    std::thread::spawn(move || {
        let listed = llm::models::fetch_models(&cfg, &cancel);
        if cancel.is_cancelled() {
            return;
        }
        let verdict = llm::local::verdict(&name, &label, &listed);
        let _ = tx.send(DeviceEvent::Tested { test, verdict });
    });
}

/// Run a subscription's sign-in on a worker thread, reporting what to show
/// and then the verdict. Long-running by nature — it ends when the user
/// approves, the prompt expires, or `cancel` trips (Esc on the page, or the
/// flow being closed out from under it).
///
/// Which flow runs is the row's **auth scheme** and its `kind`, not a guess:
/// GitHub's device code (`docs/providers/copilot.md`), OpenAI's browser PKCE or its
/// device code (`docs/providers/chatgpt.md` — the one scheme offering two, so its
/// `kind` is what the user picked on the method choice), Anthropic's browser
/// PKCE (`docs/providers/claude.md`), Cline's WorkOS device code (`docs/providers/cline.md`), or
/// Google's browser PKCE for Antigravity (`docs/providers/antigravity.md`) — the one
/// sign-in that also sets the account up, on `base`, the provider's own host.
/// All report on the same two messages, because every page is the same page
/// — something to show, then a wait.
///
/// The scheme, never the provider id: a user's own `providers.toml` can name
/// a row anything, and give two rows one sign-in. The match is exhaustive on
/// purpose — matching ids with a catch-all sent the ClinePass row to GitHub
/// Copilot's flow, and a finished sign-in would have stored GitHub's token as
/// a Cline refresh token.
pub(crate) fn spawn_signin(
    auth: AuthScheme,
    kind: SigninKind,
    base: String,
    cancel: CancelToken,
    tx: tokio::sync::mpsc::UnboundedSender<DeviceEvent>,
) {
    match (auth, kind) {
        (AuthScheme::ChatGptCodex, SigninKind::DeviceCode) => {
            spawn_chatgpt_device_login(cancel, tx)
        }
        (AuthScheme::ChatGptCodex, SigninKind::BrowserLink) => spawn_chatgpt_login(cancel, tx),
        (AuthScheme::AnthropicConsole, _) => spawn_claude_login(cancel, tx),
        (AuthScheme::ClineAccount, _) => spawn_cline_login(cancel, tx),
        (AuthScheme::GoogleAntigravity, _) => spawn_antigravity_login(base, cancel, tx),
        (AuthScheme::GithubCopilot, _) => spawn_copilot_login(cancel, tx),
        // A pasted key has nothing to sign in to. `start_device_login`
        // refuses such a row before it gets here; this arm only keeps the
        // match honest.
        (AuthScheme::ApiKey | AuthScheme::OptionalKey, _) => {
            let _ = tx.send(DeviceEvent::Done(Err(
                "this provider has no account to sign in to — paste its key under Sign in with \
                 an API key"
                    .to_string(),
            )));
        }
    }
}

/// Run OpenAI's device-code flow: ask for a code, publish it with the page
/// to enter it at, then poll until the user approves it there — the same
/// shape as GitHub's, on OpenAI's own endpoints (`docs/providers/chatgpt.md`).
fn spawn_chatgpt_device_login(
    cancel: CancelToken,
    tx: tokio::sync::mpsc::UnboundedSender<DeviceEvent>,
) {
    std::thread::spawn(move || {
        let device = match llm::chatgpt::request_device_code() {
            Ok(device) => device,
            Err(e) => {
                let _ = tx.send(DeviceEvent::Done(Err(e.to_string())));
                return;
            }
        };
        if tx
            .send(DeviceEvent::Code {
                verification_uri: llm::chatgpt::device_verification_url(),
                user_code: device.user_code().to_string(),
                expires_at: std::time::Instant::now() + llm::chatgpt::DEVICE_CODE_TIMEOUT,
            })
            .is_err()
        {
            return;
        }
        let result = llm::chatgpt::await_device_approval(&device, &cancel)
            // The seat rides back with the token, exactly as the browser
            // flow's does.
            .map(|(refresh, access)| {
                (
                    refresh,
                    access.plan.as_deref().map(llm::chatgpt::plan_label),
                )
            })
            .map_err(|e| e.to_string());
        if !cancel.is_cancelled() {
            let _ = tx.send(DeviceEvent::Done(result));
        }
    });
}

/// Run Cline's device-code sign-in: ask WorkOS for a code, publish it with
/// the page to confirm it at, then poll until the user confirms it there —
/// and, once they have, register the WorkOS pair with Cline's own API for
/// the refresh token the key store takes. See `docs/providers/cline.md`.
fn spawn_cline_login(cancel: CancelToken, tx: tokio::sync::mpsc::UnboundedSender<DeviceEvent>) {
    std::thread::spawn(move || {
        let device = match llm::cline::request_device_code() {
            Ok(device) => device,
            Err(e) => {
                let _ = tx.send(DeviceEvent::Done(Err(e.to_string())));
                return;
            }
        };
        if tx
            .send(DeviceEvent::Code {
                verification_uri: device.verification_uri().to_string(),
                user_code: device.user_code().to_string(),
                expires_at: std::time::Instant::now() + device.lifetime(),
            })
            .is_err()
        {
            return;
        }
        let result = llm::cline::await_approval(&device, &cancel)
            // The account rides back with the token, as the plan does for the
            // other two sign-ins: *whose* account this is is what the
            // confirmation leaves open.
            .map(|signed_in| (signed_in.refresh_token, signed_in.email))
            .map_err(|e| e.to_string());
        if !cancel.is_cancelled() {
            let _ = tx.send(DeviceEvent::Done(result));
        }
    });
}

/// Run Anthropic's Console PKCE flow: bind the callback port, publish the URL
/// to open, then block until the browser comes back. See `docs/providers/claude.md`.
fn spawn_claude_login(cancel: CancelToken, tx: tokio::sync::mpsc::UnboundedSender<DeviceEvent>) {
    std::thread::spawn(move || {
        let signin = match llm::claude::begin_signin() {
            Ok(signin) => signin,
            Err(e) => {
                let _ = tx.send(DeviceEvent::Done(Err(e.to_string())));
                return;
            }
        };
        // No loopback port means the browser cannot come back on its own and
        // the user would have a code to paste — which this page has nowhere
        // to take. Say so rather than showing a link that leads nowhere.
        if !signin.is_loopback() {
            let _ = tx.send(DeviceEvent::Done(Err(
                "no local port was free for the sign-in to come back on —                  close whatever is holding one and try again."
                    .to_string(),
            )));
            return;
        }
        if tx
            .send(DeviceEvent::Code {
                verification_uri: signin.url.clone(),
                user_code: String::new(),
                expires_at: std::time::Instant::now() + llm::claude::AUTH_TIMEOUT,
            })
            .is_err()
        {
            return;
        }
        let result = llm::claude::await_callback(&signin, &cancel)
            // The organisation rides back with the token, as the plan does
            // for the other two: which account the usage bills to is the
            // question this sign-in leaves open, and a user with several
            // orgs has just picked one in a browser.
            .map(|(refresh, access)| (refresh, llm::claude::plan_label(&access)))
            .map_err(|e| e.to_string());
        if !cancel.is_cancelled() {
            let _ = tx.send(DeviceEvent::Done(result));
        }
    });
}

/// Run Google's PKCE loopback flow on Antigravity's client: bind the callback
/// port (Antigravity's own first, any free one after), publish the URL to
/// open, block until the browser comes back — then set the account up on
/// `base`, onboarding it when the backend has never seen it, which is why
/// this is the one place that may wait on it. See `docs/providers/antigravity.md`.
fn spawn_antigravity_login(
    base: String,
    cancel: CancelToken,
    tx: tokio::sync::mpsc::UnboundedSender<DeviceEvent>,
) {
    std::thread::spawn(move || {
        let signin = match llm::antigravity::begin_signin() {
            Ok(signin) => signin,
            Err(e) => {
                let _ = tx.send(DeviceEvent::Done(Err(e.to_string())));
                return;
            }
        };
        if tx
            .send(DeviceEvent::Code {
                verification_uri: signin.url.clone(),
                user_code: String::new(),
                expires_at: std::time::Instant::now() + llm::antigravity::AUTH_TIMEOUT,
            })
            .is_err()
        {
            return;
        }
        let result = llm::antigravity::await_callback(&signin, &base, &cancel)
            // Which Google account, and on what plan: a user with several
            // has just picked one in a browser.
            .map(|signed_in| {
                let label = signed_in.label();
                (signed_in.refresh_token, label)
            })
            .map_err(|e| e.to_string());
        if !cancel.is_cancelled() {
            let _ = tx.send(DeviceEvent::Done(result));
        }
    });
}

/// Run OpenAI's PKCE loopback flow: bind the callback port, publish the URL
/// to open, then block until the browser comes back. See `docs/providers/chatgpt.md`.
fn spawn_chatgpt_login(cancel: CancelToken, tx: tokio::sync::mpsc::UnboundedSender<DeviceEvent>) {
    std::thread::spawn(move || {
        let signin = match llm::chatgpt::begin_signin() {
            Ok(signin) => signin,
            Err(e) => {
                let _ = tx.send(DeviceEvent::Done(Err(e.to_string())));
                return;
            }
        };
        // No code to type — the page shows the link and the browser redirects
        // back on its own, so the "expiry" shown is this flow's own patience.
        if tx
            .send(DeviceEvent::Code {
                verification_uri: signin.url.clone(),
                user_code: String::new(),
                expires_at: std::time::Instant::now() + llm::chatgpt::AUTH_TIMEOUT,
            })
            .is_err()
        {
            return;
        }
        let result = llm::chatgpt::await_callback(&signin, &cancel)
            // The seat rides back with the token, as it does for Copilot: the
            // plan is the question a sign-in leaves open, and a free account
            // that cannot reach these models is better said now than met as a
            // 403 mid-turn.
            .map(|(refresh, access)| {
                (
                    refresh,
                    access.plan.as_deref().map(llm::chatgpt::plan_label),
                )
            })
            .map_err(|e| e.to_string());
        if !cancel.is_cancelled() {
            let _ = tx.send(DeviceEvent::Done(result));
        }
    });
}

/// Run GitHub's device flow on a worker thread, reporting the code and then
/// the verdict. See `docs/providers/copilot.md`.
fn spawn_copilot_login(cancel: CancelToken, tx: tokio::sync::mpsc::UnboundedSender<DeviceEvent>) {
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
        // minute later (`docs/providers/copilot.md`).
        let result = llm::copilot::poll_for_token(&device, &cancel).and_then(|token| {
            llm::copilot::authorize(&token)
                // The seat the exchange named rides back with the token: the
                // plan is the question a sign-in leaves open, and a metered
                // free seat's remaining quota is better stated now than
                // discovered as a 402 mid-turn.
                .map(|auth| (token, auth.plan))
                .map_err(|e| e.to_string())
        });
        // A cancelled flow has no page left to report to — the user already
        // walked away from it.
        if !cancel.is_cancelled() {
            let _ = tx.send(DeviceEvent::Done(result));
        }
    });
}

/// Deliver the day's telemetry ping on a detached thread (`docs/ops/telemetry.md`),
/// sending `day` and the version the ping carried back on success
/// (`telemetry::Delivery`) so the loop — never this thread — records them in
/// `telemetry.json`. A failure sends nothing: there is nothing the user
/// could act on, and the next launch that day simply tries again. The send
/// itself is `telemetry::send_ping`, over the shared cached HTTP client; the
/// receiver closing first (a quick quit) just drops the result. It only
/// *sends* — never a stdin reader (invariant 1).
pub(crate) fn spawn_telemetry_ping(
    endpoint: String,
    ping: alter_zero::telemetry::Ping,
    day: String,
    tx: tokio::sync::mpsc::UnboundedSender<alter_zero::telemetry::Delivery>,
) {
    std::thread::spawn(move || {
        if alter_zero::telemetry::send_ping(&endpoint, &ping).is_ok() {
            let _ = tx.send(alter_zero::telemetry::Delivery {
                day,
                version: ping.version,
            });
        }
    });
}

/// Ask the repository for its newest release on a detached thread
/// (`docs/ops/update.md`), sending the version back on success so the loop —
/// never this thread — records it in `update.json` and commits the notice.
/// A failure sends nothing: the attempt's day was recorded at the spawn, so
/// a dead network costs one request per day and nothing the user sees. The
/// request itself is `update::fetch_latest`, over the shared cached HTTP
/// client; it only *sends* — never a stdin reader (invariant 1).
pub(crate) fn spawn_update_check(
    repo: String,
    version: String,
    tx: tokio::sync::mpsc::UnboundedSender<String>,
) {
    std::thread::spawn(move || {
        if let Ok(latest) = alter_zero::update::fetch_latest(&repo, &version) {
            let _ = tx.send(latest);
        }
    });
}

/// Max entries the `@` picker's worker indexes — bounds each walk's
/// memory/time, and so how many matches the picker can ever hold, since
/// every match goes back (codex's nucleo walk is similarly capped). A walk
/// that reaches it says so, and the band's count owns up to it.
const FILE_INDEX_CAP: usize = 10_000;
/// Directory names the walk skips, on top of every hidden (dotfile) entry.
const FILE_WALK_DENYLIST: &[&str] = &["target", "node_modules"];

/// A file-search result for the `@` picker: the `query` it answers (for the
/// staleness guard in `App::set_file_matches`), **every** match the walk
/// found for it, ranked — what the picker scrolls through and counts — and
/// whether that walk stopped at its cap.
pub(crate) struct FileSearchResult {
    query: String,
    matches: Vec<FileMatch>,
    truncated: bool,
}

/// What one walk of the workspace found: the relative paths, and whether it
/// stopped at its cap with entries left unlisted.
struct Walk {
    paths: Vec<String>,
    truncated: bool,
}

/// Walk `root` breadth-first collecting up to `cap` relative paths, skipping
/// hidden entries (dotfiles — so `.git` too) and [`FILE_WALK_DENYLIST`]
/// directories; directories are listed with a trailing `/`. A walk is
/// `truncated` only when an entry it would have listed is left out: it reads
/// on past the cap until it meets one, so a tree that exactly fills the cap is
/// complete. Breadth-first, so a cut drops the deepest entries. Dependency-free
/// (the agreed design — no `.gitignore` parsing). See `docs/input/file-search.md`.
///
/// Each path is built **once**, as its folder's path plus its name joined by
/// `/`: one allocation per entry beside the name `read_dir` hands back, where
/// a full path, a copy relative to the root and a copy with the folder's `/`
/// appended each used to be allocated and thrown away. Joining by hand is
/// also what keeps a name's own `\` — an ordinary character on Linux and
/// macOS — rather than rewriting it into a separator. A folder's entries are
/// gathered before any is listed, never more than the cap has room for (the
/// one more that proves the walk short is noticed, not kept), so a huge last
/// folder costs no more than the cap allows.
fn walk_files(root: &Path, cap: usize) -> Walk {
    let mut paths: Vec<String> = Vec::new();
    // The folders still to read, each as the index of its own entry in
    // `paths` — its path from the root, closing `/` included — or `None` for
    // the root, so an entry's path is that text followed by its name.
    let mut queue: VecDeque<Option<usize>> = VecDeque::from([None]);
    // One folder's listable entries, gathered before any is listed: each
    // one's path from the root and whether it is a folder.
    let mut found: Vec<(String, bool)> = Vec::new();
    while let Some(dir) = queue.pop_front() {
        let folder = dir.map_or("", |i| paths[i].as_str());
        let Ok(entries) = std::fs::read_dir(root.join(folder)) else {
            continue;
        };
        let mut more = false;
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.') || FILE_WALK_DENYLIST.contains(&name.as_ref()) {
                continue;
            }
            if paths.len() + found.len() == cap {
                more = true;
                break;
            }
            let is_dir = entry.file_type().is_ok_and(|t| t.is_dir());
            let mut path = String::with_capacity(folder.len() + name.len() + 1);
            path.push_str(folder);
            path.push_str(&name);
            if is_dir {
                path.push('/');
            }
            found.push((path, is_dir));
        }
        // In listing order, so the walk as a whole reads like a bare `@`'s
        // list (`file_search::sibling_order`) and that list's sort finds
        // nothing to move. Only a folder's own entries are compared here, by
        // their names alone.
        let start = folder.len();
        found.sort_by(|(a, a_dir), (b, b_dir)| {
            sibling_order(
                own_name(a, start, *a_dir),
                *a_dir,
                own_name(b, start, *b_dir),
                *b_dir,
            )
        });
        for (path, is_dir) in found.drain(..) {
            if is_dir {
                queue.push_back(Some(paths.len()));
            }
            paths.push(path);
        }
        if more {
            return Walk {
                paths,
                truncated: true,
            };
        }
    }
    Walk {
        paths,
        truncated: false,
    }
}

/// An entry's own name inside a walked `path` whose folder's part is the
/// first `start` bytes — a folder's closing `/` left off, as
/// `FileMatch::name` leaves it off, so the walk's sort and the listing's
/// compare the same text.
fn own_name(path: &str, start: usize, is_dir: bool) -> &str {
    let own = &path[start..];
    if is_dir {
        own.strip_suffix('/').unwrap_or(own)
    } else {
        own
    }
}

/// Spawn the `@` file-search worker: a background thread that, for each query —
/// coalescing any that queued while it worked (the debounce) — walks `root`
/// **afresh** and ranks the result ([`rank_files`]), sending a
/// [`FileSearchResult`] back with every match in it, not just the first
/// screenful: the picker scrolls through and counts them all, five rows at a
/// time (`docs/input/file-search.md`). The walk is per-query (bounded by
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
            // dir only ever showed the boot-time contents. The walk's list
            // moves into the ranking, so its paths are never copied, and
            // every match comes back.
            let walk = walk_files(&root, FILE_INDEX_CAP);
            let matches = rank_files(&query, walk.paths);
            let result = FileSearchResult {
                query,
                matches,
                truncated: walk.truncated,
            };
            if res_tx.send(result).is_err() {
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
pub(crate) fn spawn_image_paste(
    dir: PathBuf,
    tx: tokio::sync::mpsc::UnboundedSender<Result<PathBuf, String>>,
) {
    std::thread::spawn(move || {
        // The receiver only closes at shutdown — a failed send just means
        // there is nothing left to attach to.
        let _ = tx.send(clipboard::read_clipboard_image(&dir));
    });
}

/// Fetch one provider's `/models` list on a background thread (the image-paste
/// pattern), sending the labelled result to the loop. One of these is spawned
/// per configured provider so they fetch in parallel; a cancelled fetch (the
/// picker closed) is dropped. See `docs/providers/llm.md`.
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

/// Read one subscription's usage on a background thread (the `/model` fetch's
/// pattern), sending the result — tagged with the page's `round` and the
/// provider it read — to the loop. One of these runs per signed-in
/// subscription, so they read in parallel; a cancelled read (the page closed,
/// a newer round began) sends nothing. See `docs/providers/usage.md`.
pub(crate) fn spawn_usage_fetch(
    round: u64,
    provider: String,
    cfg: ModelConfig,
    cancel: CancelToken,
    tx: tokio::sync::mpsc::UnboundedSender<super::usage::UsageResult>,
) {
    std::thread::spawn(move || {
        let result = llm::usage::fetch_usage(&cfg, &cancel).map_err(|e| e.to_string());
        if !cancel.is_cancelled() {
            let _ = tx.send((round, provider, result));
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
    /// then repaint. See `docs/input/file-search.md`.
    pub(crate) fn on_file_matches(&mut self, result: FileSearchResult) {
        self.app
            .set_file_matches(&result.query, result.matches, result.truncated);
        self.frame.schedule_frame();
    }

    /// A finished Ctrl+V clipboard read: attach the temp image to the composer, or
    /// surface the red failure notice. Committing is view-gated (invariant 4) —
    /// the result may arrive with an overlay up, where the notice is recorded only
    /// and the return repaints it from history. See `docs/input/image-paste.md`.
    pub(crate) fn on_image_paste(&mut self, result: Result<PathBuf, String>) {
        match result {
            Ok(path) => {
                // Read the header once, here at the boundary, so the pure line
                // builders can reserve the right rows for the picture under
                // the bubble it will be sent with (`docs/ui/images.md`). A path
                // whose size never lands simply isn't drawn.
                if let Some(name) = path.to_str()
                    && let Some(px) = image_dimensions(&path)
                {
                    alter_zero::images::remember_size(name, px);
                }
                self.app.attach_image(path);
                // A known non-vision model can't see the paste: warn at once with
                // a toast — the attachment still rides the request as a text note
                // the model reads, so it can tell the user too (docs/tools/tools.md).
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

/// The pixel size of the image at `path`, read from its header — no full
/// decode, so a 12-megapixel screenshot costs a few hundred bytes of I/O.
pub(crate) fn image_dimensions(path: &std::path::Path) -> Option<(u32, u32)> {
    image::ImageReader::open(path)
        .ok()?
        .with_guessed_format()
        .ok()?
        .into_dimensions()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::walk_files;

    /// A temp tree of four listable entries — two files at the top, a
    /// directory holding a third — beside a hidden file and a denylisted
    /// directory the walk never lists.
    fn tree() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("a temp dir");
        let root = dir.path();
        for file in ["a.txt", "b.txt", ".hidden"] {
            std::fs::write(root.join(file), "").expect("a file");
        }
        for sub in ["sub", "target"] {
            std::fs::create_dir(root.join(sub)).expect("a dir");
            std::fs::write(root.join(sub).join("inner.txt"), "").expect("a file");
        }
        dir
    }

    #[test]
    fn the_walk_lists_the_workspace_and_says_it_saw_all_of_it() {
        let dir = tree();
        let walk = walk_files(dir.path(), 100);
        let mut paths = walk.paths.clone();
        paths.sort();
        assert_eq!(paths, ["a.txt", "b.txt", "sub/", "sub/inner.txt"]);
        assert!(!walk.truncated, "nothing was left out");
    }

    #[test]
    fn the_walk_says_when_its_cap_left_entries_unlisted() {
        // The picker counts what the walk found, so a walk cut short by its
        // cap must say so — or the count would claim the whole workspace.
        let dir = tree();
        let exact = walk_files(dir.path(), 4);
        assert_eq!(exact.paths.len(), 4);
        assert!(!exact.truncated, "a cap the tree exactly fills cut nothing");
        let short = walk_files(dir.path(), 3);
        assert_eq!(short.paths.len(), 3);
        assert!(short.truncated, "a fourth entry went unlisted");
    }

    #[cfg(unix)]
    #[test]
    fn a_backslash_in_a_name_stays_part_of_the_name() {
        // On Linux and macOS `\` is an ordinary character in a file name, so
        // `a\b.txt` is one file at the top — listed as `a/b.txt`, it would
        // show under a folder `a` that does not exist, and Enter would
        // insert a path that names nothing.
        let dir = tempfile::tempdir().expect("a temp dir");
        std::fs::write(dir.path().join("a\\b.txt"), "").expect("a file");
        std::fs::create_dir(dir.path().join("x\\y")).expect("a dir");
        std::fs::write(dir.path().join("x\\y").join("z.txt"), "").expect("a file");
        let mut paths = walk_files(dir.path(), 100).paths;
        paths.sort();
        assert_eq!(paths, ["a\\b.txt", "x\\y/", "x\\y/z.txt"]);
    }

    #[test]
    fn the_walk_reads_each_folder_into_listing_order() {
        // Each folder's entries are sorted as they are read — folders first,
        // names without regard to case — so the breadth-first walk comes out
        // in exactly the order a bare `@` lists, whatever order the
        // filesystem returned them in, and the listing's own sort finds
        // nothing to move.
        let dir = tempfile::tempdir().expect("a temp dir");
        let root = dir.path();
        for folder in ["c", "B", "Z", "c/X"] {
            std::fs::create_dir(root.join(folder)).expect("a dir");
        }
        for file in ["b.txt", "A.txt", "a.md", "c/y.txt", "B/q", "c/X/w"] {
            std::fs::write(root.join(file), "").expect("a file");
        }
        let walk = walk_files(root, 100);
        assert_eq!(
            walk.paths,
            [
                "B/", "c/", "Z/", "a.md", "A.txt", "b.txt", "B/q", "c/X/", "c/y.txt", "c/X/w",
            ]
        );
        let listed: Vec<String> = alter_zero::file_search::rank_files("", walk.paths.clone())
            .into_iter()
            .map(|m| m.path)
            .collect();
        assert_eq!(listed, walk.paths, "the listing found nothing to move");
    }
}
