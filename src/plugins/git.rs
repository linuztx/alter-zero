//! The `git` invocations the plugin store makes: shallow clones,
//! one-revision fetches, remote lookups.
//!
//! This is the user's own `git`, run the way they would run it — their
//! config stays in force, so proxies, credential helpers and `insteadOf`
//! rewrites keep working — with three things changed: an inherited
//! `GIT_DIR` and its siblings are removed (the app may itself have been
//! started by git), output is read on threads with a bound so a long clone
//! never blocks the caller, and inside the TUI nothing may ask the terminal
//! for a password, since the TUI owns it. A long operation is polled: a
//! cancel or the timeout kills it, with everything it started. See
//! `docs/plugins.md`.

use std::ffi::OsStr;
use std::fmt;
use std::fs;
use std::io::{self, Read};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

/// How long one git operation may run before it is stopped.
pub const DEFAULT_GIT_TIMEOUT: Duration = Duration::from_secs(300);

/// How often a running git is polled for exit, cancellation and progress.
const POLL_INTERVAL: Duration = Duration::from_millis(20);
/// How much of git's stdout is kept — a revision or a short listing.
const STDOUT_MAX_BYTES: usize = 1024 * 1024;
/// How many of git's last stderr lines are kept for an error message.
const STDERR_MAX_LINES: usize = 40;
/// How long the quick local lookups ([`head_revision`], [`tree_id`]) may run.
const LOCAL_TIMEOUT: Duration = Duration::from_secs(30);

/// What a caller can say to a running git operation, and hear from it.
///
/// Polled between reads: a long operation stops (the child is killed) soon
/// after `cancelled()` turns true, and `progress` hears git's own progress
/// lines (`Receiving objects: 45%` → `GitProgress { phase: "Receiving
/// objects", percent: Some(45) }`).
pub trait GitControl {
    /// Should the operation stop now?
    fn cancelled(&self) -> bool {
        false
    }
    /// One progress report from git.
    fn progress(&self, _progress: GitProgress) {}
}

/// A control that never cancels and ignores progress.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoControl;

impl GitControl for NoControl {}

/// One of git's progress lines, read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitProgress {
    /// `Receiving objects`, `Resolving deltas`, `Counting objects`.
    pub phase: String,
    /// The percentage, when the line carries one.
    pub percent: Option<u8>,
}

/// How a git operation is run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GitOptions {
    /// The longest it may take ([`DEFAULT_GIT_TIMEOUT`] by default).
    pub timeout: Duration,
    /// `false` inside the TUI: `GIT_TERMINAL_PROMPT=0` and (unless the user
    /// set `GIT_SSH_COMMAND`) `ssh -oBatchMode=yes`, so nothing ever asks the
    /// terminal. `true` for the CLI, where git may prompt for credentials.
    pub interactive: bool,
}

impl Default for GitOptions {
    fn default() -> Self {
        Self {
            timeout: DEFAULT_GIT_TIMEOUT,
            interactive: false,
        }
    }
}

/// Why a git operation did not succeed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitError {
    /// There is no `git` to run.
    Missing,
    /// The caller cancelled it.
    Cancelled,
    /// It ran past its timeout and was stopped.
    TimedOut,
    /// It failed: its exit code (none when it never ran or was killed) and
    /// what it said.
    Failed { code: Option<i32>, stderr: String },
}

impl fmt::Display for GitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing => f.write_str("git is not installed — plugins are fetched with git"),
            Self::Cancelled => f.write_str("the git operation was cancelled"),
            Self::TimedOut => {
                f.write_str("git took too long and was stopped — check the network and try again")
            }
            Self::Failed { code, stderr } => match (meaningful_line(stderr), code) {
                (Some(line), _) => write!(f, "git failed: {line}"),
                (None, Some(code)) => write!(f, "git failed (exit code {code})"),
                (None, None) => f.write_str("git failed"),
            },
        }
    }
}

impl GitError {
    /// Why it failed, as a clause to follow a sentence naming what was being
    /// fetched: git's own words for a failure, else the whole message.
    #[must_use]
    pub fn reason(&self) -> String {
        match self {
            Self::Failed { stderr, .. } => {
                meaningful_line(stderr).unwrap_or_else(|| self.to_string())
            }
            other => other.to_string(),
        }
    }
}

impl std::error::Error for GitError {}

/// Is there a `git` to run?
#[must_use]
pub fn git_available() -> bool {
    let mut command = git_command(&GitOptions::default());
    command.arg("--version");
    let opts = GitOptions {
        timeout: LOCAL_TIMEOUT,
        interactive: false,
    };
    run(command, &opts, &NoControl).is_ok()
}

/// `git clone --depth 1 [--branch ref] --no-tags --single-branch <url>
/// <dest>`, with progress. `dest` must not exist; on failure nothing of it
/// is left. Returns HEAD's commit.
///
/// # Errors
/// [`GitError`]; `Failed` too when `dest` already exists.
pub fn shallow_clone(
    url: &str,
    reference: Option<&str>,
    dest: &Path,
    opts: &GitOptions,
    ctl: &dyn GitControl,
) -> Result<String, GitError> {
    if ctl.cancelled() {
        return Err(GitError::Cancelled);
    }
    if let Some(reference) = reference {
        check_reference(reference)?;
    }
    claim(dest)?;
    let result = (|| {
        let mut command = git_command(opts);
        command.args([
            "clone",
            "--depth",
            "1",
            "--no-tags",
            "--single-branch",
            "--progress",
        ]);
        if let Some(reference) = reference {
            command.args(["--branch", reference]);
        }
        command.arg("--").arg(url).arg(dest);
        run(command, opts, ctl)?;
        revision_of(dest)
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(dest);
    }
    result
}

/// A checkout of one commit or ref in a fresh `dest`: `git init`, `remote
/// add`, `fetch --depth 1 origin <sha|ref|HEAD>`, `checkout --detach
/// FETCH_HEAD`. With `subdir`, a blob-less sparse checkout of that folder is
/// tried first and a plain one is the fallback. When `sha` is given the
/// result is that commit — a server that will not hand out a commit by id
/// is asked for the branch instead and the commit checked out from it.
/// Returns the commit checked out.
///
/// # Errors
/// [`GitError`]; `Failed` too when `dest` exists, the commit is not the one
/// asked for, or `subdir` is not in it.
pub fn fetch_revision(
    url: &str,
    reference: Option<&str>,
    sha: Option<&str>,
    subdir: Option<&str>,
    dest: &Path,
    opts: &GitOptions,
    ctl: &dyn GitControl,
) -> Result<String, GitError> {
    if ctl.cancelled() {
        return Err(GitError::Cancelled);
    }
    for text in [reference, sha, subdir].into_iter().flatten() {
        check_reference(text)?;
    }
    claim(dest)?;
    let mut result = checkout(url, reference, sha, subdir, dest, opts, ctl);
    if subdir.is_some() && matches!(result, Err(GitError::Failed { .. })) {
        // The sparse, blob-less attempt is an optimisation; a server or a
        // git that will not do it gets the plain checkout.
        let _ = fs::remove_dir_all(dest);
        result = claim(dest).and_then(|()| checkout(url, reference, sha, None, dest, opts, ctl));
    }
    let result = result.and_then(|()| {
        let head = revision_of(dest)?;
        if let Some(sha) = sha
            && !head.starts_with(&sha.to_ascii_lowercase())
        {
            return Err(failure(format!(
                "the fetched commit {head} is not the pinned commit {sha}"
            )));
        }
        if let Some(subdir) = subdir
            && !dest.join(subdir).is_dir()
        {
            return Err(failure(format!("the folder {subdir} is not in {url}")));
        }
        Ok(head)
    });
    if result.is_err() {
        let _ = fs::remove_dir_all(dest);
    }
    result
}

/// One attempt at [`fetch_revision`] in the empty repository-to-be `dest`;
/// `sparse` names the folder of a blob-less sparse checkout.
fn checkout(
    url: &str,
    reference: Option<&str>,
    sha: Option<&str>,
    sparse: Option<&str>,
    dest: &Path,
    opts: &GitOptions,
    ctl: &dyn GitControl,
) -> Result<(), GitError> {
    let git = |args: &[&OsStr]| {
        let mut command = git_command(opts);
        command.arg("-C").arg(dest).args(args);
        run(command, opts, ctl).map(drop)
    };
    let os = |text: &'static str| OsStr::new(text);
    fs::create_dir(dest)
        .map_err(|error| failure(format!("could not create {}: {error}", dest.display())))?;
    git(&[os("init"), os("--quiet")])?;
    git(&[
        os("remote"),
        os("add"),
        os("origin"),
        os("--"),
        OsStr::new(url),
    ])?;
    let fetch = |target: &str, shallow: bool| {
        let mut args = vec![os("fetch"), os("--no-tags"), os("--progress")];
        if shallow {
            args.extend([os("--depth"), os("1")]);
        }
        if sparse.is_some() {
            args.push(os("--filter=blob:none"));
        }
        args.extend([os("origin"), OsStr::new(target)]);
        git(&args)
    };
    let named = reference.unwrap_or("HEAD");
    let target = match sha {
        Some(sha) => match fetch(sha, true) {
            Ok(()) => "FETCH_HEAD",
            // A server that will not hand out a commit by id: fetch the
            // branch whole and find the commit in it.
            Err(GitError::Failed { .. }) => {
                fetch(named, false)?;
                sha
            }
            Err(other) => return Err(other),
        },
        None => {
            fetch(named, true)?;
            "FETCH_HEAD"
        }
    };
    if let Some(folder) = sparse {
        git(&[
            os("sparse-checkout"),
            os("set"),
            os("--cone"),
            os("--"),
            OsStr::new(folder),
        ])?;
    }
    git(&[
        os("checkout"),
        os("--quiet"),
        os("--detach"),
        OsStr::new(target),
    ])
}

/// `git ls-remote <url> <ref|HEAD>` → the commit, without cloning; a tag is
/// read through to the commit it names.
///
/// # Errors
/// [`GitError`]; `Failed` too when the remote has no such ref.
pub fn remote_revision(
    url: &str,
    reference: Option<&str>,
    opts: &GitOptions,
    ctl: &dyn GitControl,
) -> Result<String, GitError> {
    if ctl.cancelled() {
        return Err(GitError::Cancelled);
    }
    let wanted = reference.unwrap_or("HEAD");
    check_reference(wanted)?;
    if wanted.len() == 40 && wanted.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Ok(wanted.to_ascii_lowercase());
    }
    let mut command = git_command(opts);
    // A tag's own line names the tag object; the `^{}` line, which only a
    // pattern of its own lists, names the commit.
    let peeled = format!("{wanted}^{{}}");
    command.args(["ls-remote", "--", url, wanted, &peeled]);
    let listing = run(command, opts, ctl)?;
    let refs: Vec<(&str, &str)> = listing
        .lines()
        .filter_map(|line| line.split_once('\t'))
        .collect();
    let find = |name: &str| {
        refs.iter()
            .find(|(_, known)| *known == name)
            .map(|(sha, _)| *sha)
    };
    let found = if reference.is_none() {
        find("HEAD")
    } else {
        find(&format!("refs/heads/{wanted}"))
            .or_else(|| find(&format!("refs/tags/{wanted}^{{}}")))
            .or_else(|| find(&format!("refs/tags/{wanted}")))
            .or_else(|| find(&format!("{wanted}^{{}}")))
            .or_else(|| find(wanted))
    };
    found
        .map(str::to_string)
        .ok_or_else(|| failure(format!("{url} has no branch or tag named {wanted}")))
}

/// The commit checked out in `repo`.
#[must_use]
pub fn head_revision(repo: &Path) -> Option<String> {
    local(repo, &["rev-parse", "--verify", "--quiet", "HEAD"])
}

/// The tree id of `subdir` at `repo`'s HEAD (`""` for the root) — what an
/// in-repo plugin's revision is, since it changes exactly when a file in
/// that folder does.
#[must_use]
pub fn tree_id(repo: &Path, subdir: &str) -> Option<String> {
    let path = subdir.trim_matches('/');
    let id = local(
        repo,
        &["rev-parse", "--verify", "--quiet", &format!("HEAD:{path}")],
    )?;
    let kind = local(repo, &["cat-file", "-t", &id])?;
    (kind == "tree").then_some(id)
}

/// A quick read-only git command in `repo`: its trimmed stdout, or `None`
/// for any failure.
fn local(repo: &Path, args: &[&str]) -> Option<String> {
    let opts = GitOptions {
        timeout: LOCAL_TIMEOUT,
        interactive: false,
    };
    let mut command = git_command(&opts);
    command.arg("-C").arg(repo).args(args);
    let out = run(command, &opts, &NoControl).ok()?;
    let out = out.trim();
    (!out.is_empty()).then(|| out.to_string())
}

/// The commit checked out in a repository this module just made.
fn revision_of(repo: &Path) -> Result<String, GitError> {
    head_revision(repo)
        .ok_or_else(|| failure(format!("{} has no commit checked out", repo.display())))
}

/// A failure of ours rather than git's, worded the same way.
fn failure(message: String) -> GitError {
    GitError::Failed {
        code: None,
        stderr: format!("fatal: {message}"),
    }
}

/// A branch, tag, commit or folder name that git could read as an option is
/// refused: it comes from a marketplace file, not from the user.
fn check_reference(text: &str) -> Result<(), GitError> {
    if text.starts_with('-') || text.contains('\0') {
        return Err(failure(format!(
            "{text:?} is not a usable branch, tag or folder name"
        )));
    }
    Ok(())
}

/// `dest` must not exist; its parent is created.
fn claim(dest: &Path) -> Result<(), GitError> {
    if fs::symlink_metadata(dest).is_ok() {
        return Err(failure(format!("{} already exists", dest.display())));
    }
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| failure(format!("could not create {}: {error}", parent.display())))?;
    }
    Ok(())
}

/// `git` with the environment every call here shares.
fn git_command(opts: &GitOptions) -> Command {
    let mut command = Command::new("git");
    command
        .args(["-c", "core.fsmonitor=false"])
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("LC_ALL", "C");
    // The app may have been started by git; these must not point the call
    // at another repository.
    for variable in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_COMMON_DIR",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    ] {
        command.env_remove(variable);
    }
    if !opts.interactive {
        command.env("GIT_TERMINAL_PROMPT", "0");
        if std::env::var_os("GIT_SSH_COMMAND").is_none() && std::env::var_os("GIT_SSH").is_none() {
            command.env("GIT_SSH_COMMAND", "ssh -oBatchMode=yes");
        }
    }
    command
}

/// Run `command` to completion under `opts` and `ctl`, returning its
/// stdout.
///
/// Outside interactive use the command leads a process group of its own, so
/// a cancel or a timeout stops everything it started (`git clone` runs
/// `git-remote-https`, `ssh`, `git index-pack`), not only `git` — anything
/// left running would also hold the output pipes open. An interactive one
/// stays in the terminal's group so it may prompt there.
fn run(mut command: Command, opts: &GitOptions, ctl: &dyn GitControl) -> Result<String, GitError> {
    if ctl.cancelled() {
        return Err(GitError::Cancelled);
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let grouped = !opts.interactive;
    #[cfg(unix)]
    if grouped {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command.spawn().map_err(|error| match error.kind() {
        io::ErrorKind::NotFound => GitError::Missing,
        _ => GitError::Failed {
            code: None,
            stderr: format!("fatal: could not run git: {error}"),
        },
    })?;
    let (Some(stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take()) else {
        stop(&mut child, grouped);
        return Err(failure("could not read git's output".to_string()));
    };
    let stdout_reader = thread::spawn(move || read_bounded(stdout, STDOUT_MAX_BYTES));
    let (lines_tx, lines) = mpsc::channel();
    let stderr_reader = thread::spawn(move || {
        let mut stderr = stderr;
        let mut splitter = LineSplitter::default();
        let mut buffer = [0_u8; 4096];
        while let Ok(count) = stderr.read(&mut buffer) {
            if count == 0 {
                break;
            }
            for line in splitter.push(&buffer[..count]) {
                if lines_tx.send(line).is_err() {
                    return;
                }
            }
        }
        if let Some(line) = splitter.finish() {
            let _ = lines_tx.send(line);
        }
    });
    let mut said: Vec<String> = Vec::new();
    let mut hear = |line: String| match parse_progress(&line) {
        Some(progress) => ctl.progress(progress),
        None if line.trim().is_empty() => {}
        None => {
            if said.len() == STDERR_MAX_LINES {
                said.remove(0);
            }
            said.push(line.chars().take(500).collect());
        }
    };
    let started = Instant::now();
    let status = loop {
        while let Ok(line) = lines.try_recv() {
            hear(line);
        }
        if ctl.cancelled() {
            stop(&mut child, grouped);
            return Err(GitError::Cancelled);
        }
        if started.elapsed() >= opts.timeout {
            stop(&mut child, grouped);
            return Err(GitError::TimedOut);
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => thread::sleep(POLL_INTERVAL),
            Err(error) => {
                stop(&mut child, grouped);
                return Err(failure(format!("could not wait for git: {error}")));
            }
        }
    };
    let _ = stderr_reader.join();
    for line in lines.try_iter() {
        hear(line);
    }
    let stdout = stdout_reader.join().unwrap_or_default();
    if status.success() {
        Ok(String::from_utf8_lossy(&stdout).into_owned())
    } else {
        Err(GitError::Failed {
            code: status.code(),
            stderr: said.join("\n"),
        })
    }
}

/// Stop a running command — its whole process group when it leads one —
/// and reap it. The reader threads are left to finish on their own: once
/// the group is gone the pipes close.
fn stop(child: &mut Child, grouped: bool) {
    #[cfg(unix)]
    if grouped {
        crate::subprocess::kill_process_group(child);
        return;
    }
    let _ = grouped;
    let _ = child.kill();
    let _ = child.wait();
}

/// Everything `reader` gives, up to `limit` bytes; past that the rest is
/// read and thrown away so the writer never blocks on a full pipe.
fn read_bounded(mut reader: impl Read, limit: usize) -> Vec<u8> {
    let mut kept = Vec::new();
    let mut buffer = [0_u8; 8192];
    while let Ok(count) = reader.read(&mut buffer) {
        if count == 0 {
            break;
        }
        let room = limit.saturating_sub(kept.len()).min(count);
        kept.extend_from_slice(&buffer[..room]);
    }
    kept
}

/// Git's progress line `line`, read — `None` for any other line.
///
/// A progress line is `<phase>: <number>…` — `Receiving objects:  45% (1/2)`
/// or `Enumerating objects: 8, done.` — with the remote's own lines prefixed
/// `remote: `.
fn parse_progress(line: &str) -> Option<GitProgress> {
    let line = line.trim();
    let line = line.strip_prefix("remote:").map_or(line, str::trim_start);
    let (phase, rest) = line.split_once(':')?;
    let phase = phase.trim();
    if phase.is_empty() || !phase.chars().all(|c| c.is_ascii_alphabetic() || c == ' ') {
        return None;
    }
    let rest = rest.trim_start();
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 {
        return None;
    }
    let percent = rest[digits..]
        .starts_with('%')
        .then(|| rest[..digits].parse::<u8>().ok())
        .flatten();
    Some(GitProgress {
        phase: phase.to_string(),
        percent,
    })
}

/// Splits git's stderr into lines at `\n` and at `\r`, which is how git
/// redraws a progress line in place. A line longer than [`LINE_MAX_BYTES`]
/// keeps only its tail — what a progress line ends on — so a stream with no
/// line ends never grows without bound.
#[derive(Debug, Default)]
struct LineSplitter {
    pending: Vec<u8>,
}

/// The most of one stderr line [`LineSplitter`] holds.
const LINE_MAX_BYTES: usize = 4096;

impl LineSplitter {
    /// The lines `bytes` completes.
    fn push(&mut self, bytes: &[u8]) -> Vec<String> {
        let mut lines = Vec::new();
        for &byte in bytes {
            if byte == b'\n' || byte == b'\r' {
                // `\r\n` ends one line, not two.
                if !self.pending.is_empty() {
                    lines.push(String::from_utf8_lossy(&self.pending).into_owned());
                    self.pending.clear();
                }
            } else {
                if self.pending.len() == LINE_MAX_BYTES {
                    // Keep the newer half: a progress line's end is the
                    // part worth reading.
                    self.pending.drain(..LINE_MAX_BYTES / 2);
                }
                self.pending.push(byte);
            }
        }
        lines
    }

    /// The last, unterminated line, if any.
    fn finish(self) -> Option<String> {
        (!self.pending.is_empty()).then(|| String::from_utf8_lossy(&self.pending).into_owned())
    }
}

/// What git says last when something before it went wrong, adding nothing
/// to the line that said what.
const GENERIC_GIT_LINES: [&str; 5] = [
    "could not read from remote repository",
    "the remote end hung up unexpectedly",
    "early eof",
    "index-pack failed",
    "fetch-pack: invalid index-pack output",
];

/// The line of git's stderr an error message should quote.
///
/// The first `fatal:` or `error:` line without its prefix — git says the
/// cause first and a generic line after it — joined with the last such line
/// when that one says something more; else the last line that is not a
/// hint, a warning or progress.
fn meaningful_line(stderr: &str) -> Option<String> {
    let lines = || {
        stderr
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
    };
    let failures: Vec<&str> = lines()
        .filter_map(|line| {
            line.strip_prefix("fatal:")
                .or_else(|| line.strip_prefix("error:"))
                .map(str::trim)
        })
        .filter(|line| !line.is_empty())
        .collect();
    if let (Some(first), Some(last)) = (failures.first(), failures.last()) {
        let generic = |line: &str| {
            let lower = line.trim_end_matches('.').to_ascii_lowercase();
            GENERIC_GIT_LINES.contains(&lower.as_str())
        };
        return Some(if first == last || generic(last) {
            (*first).to_string()
        } else {
            format!("{first}; {last}")
        });
    }
    lines()
        .rfind(|line| {
            !line.starts_with("hint:")
                && !line.starts_with("warning:")
                && parse_progress(line).is_none()
        })
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::fs;
    use std::path::PathBuf;
    use std::time::Instant;

    use super::*;

    // --- pure pieces ------------------------------------------------------

    #[test]
    fn progress_lines_are_read() {
        assert_eq!(
            parse_progress("Receiving objects:  45% (123/273), 1.2 MiB | 800 KiB/s"),
            Some(GitProgress {
                phase: "Receiving objects".into(),
                percent: Some(45)
            })
        );
        assert_eq!(
            parse_progress("remote: Counting objects: 100% (7/7), done.        "),
            Some(GitProgress {
                phase: "Counting objects".into(),
                percent: Some(100)
            })
        );
        assert_eq!(
            parse_progress("remote: Enumerating objects: 8, done."),
            Some(GitProgress {
                phase: "Enumerating objects".into(),
                percent: None
            })
        );
        for other in [
            "fatal: repository '/x' does not exist",
            "Cloning into 'dest'...",
            "remote: Total 8 (delta 0), reused 0 (delta 0), pack-reused 0",
            "warning: filtering not recognized by server, ignoring",
            "",
        ] {
            assert_eq!(parse_progress(other), None, "{other}");
        }
    }

    #[test]
    fn stderr_splits_at_carriage_returns_and_newlines() {
        let mut splitter = LineSplitter::default();
        let mut lines = splitter.push(b"Receiving objects:  12% (1/8)\rReceiving obj");
        lines.extend(splitter.push(b"ects:  25% (2/8)\r\nfatal: gone\nwarn"));
        assert_eq!(
            lines,
            [
                "Receiving objects:  12% (1/8)",
                "Receiving objects:  25% (2/8)",
                "fatal: gone"
            ]
        );
        assert_eq!(splitter.finish().as_deref(), Some("warn"));
        assert_eq!(LineSplitter::default().finish(), None);
    }

    #[test]
    fn a_line_with_no_end_keeps_only_its_tail() {
        let mut splitter = LineSplitter::default();
        for _ in 0..1024 {
            assert!(splitter.push(&[b'x'; 1024]).is_empty());
        }
        assert!(splitter.pending.len() <= LINE_MAX_BYTES);
        let lines = splitter.push(b"Receiving objects:  45% (1/2)\n");
        assert_eq!(lines.len(), 1);
        assert!(lines[0].len() <= LINE_MAX_BYTES);
        assert!(
            lines[0].ends_with("Receiving objects:  45% (1/2)"),
            "{}",
            &lines[0][..40]
        );
    }

    #[test]
    fn the_meaningful_line_is_the_first_fatal_or_error() {
        let stderr = "Cloning into 'x'...\nfatal: repository 'x' does not exist\nhint: try again\n";
        assert_eq!(
            meaningful_line(stderr).as_deref(),
            Some("repository 'x' does not exist")
        );
        assert_eq!(
            meaningful_line("hint: a\nsomething broke\n").as_deref(),
            Some("something broke")
        );
        assert_eq!(meaningful_line("  \n"), None);
    }

    #[test]
    fn a_generic_closing_line_never_hides_the_cause() {
        let stderr = "fatal: '/nonexistent/repo.git' does not appear to be a git repository\n\
                      fatal: Could not read from remote repository.\n\n\
                      Please make sure you have the correct access rights\n\
                      and the repository exists.\n";
        assert_eq!(
            meaningful_line(stderr).as_deref(),
            Some("'/nonexistent/repo.git' does not appear to be a git repository")
        );
        // A later line that says more is joined on.
        let stderr = "error: RPC failed; HTTP 403\nfatal: unable to access 'x': denied\n";
        assert_eq!(
            meaningful_line(stderr).as_deref(),
            Some("RPC failed; HTTP 403; unable to access 'x': denied")
        );
        let failed = GitError::Failed {
            code: Some(128),
            stderr: "fatal: gone\nfatal: the remote end hung up unexpectedly\n".into(),
        };
        assert_eq!(failed.reason(), "gone");
        assert_eq!(failed.to_string(), "git failed: gone");
    }

    #[test]
    fn errors_read_as_sentences() {
        assert_eq!(
            GitError::Missing.to_string(),
            "git is not installed — plugins are fetched with git"
        );
        assert_eq!(
            GitError::Cancelled.to_string(),
            "the git operation was cancelled"
        );
        assert!(GitError::TimedOut.to_string().contains("took too long"));
        assert_eq!(
            GitError::Failed {
                code: Some(128),
                stderr: "Cloning…\nfatal: could not read from remote repository\n".into()
            }
            .to_string(),
            "git failed: could not read from remote repository"
        );
        assert_eq!(
            GitError::Failed {
                code: Some(1),
                stderr: String::new()
            }
            .to_string(),
            "git failed (exit code 1)"
        );
    }

    // --- the runner -------------------------------------------------------

    struct CancelAfter {
        polls: Cell<usize>,
        after: usize,
    }

    impl GitControl for CancelAfter {
        fn cancelled(&self) -> bool {
            self.polls.set(self.polls.get() + 1);
            self.polls.get() > self.after
        }
    }

    #[derive(Default)]
    struct Recorder(RefCell<Vec<GitProgress>>);

    impl GitControl for Recorder {
        fn progress(&self, progress: GitProgress) {
            self.0.borrow_mut().push(progress);
        }
    }

    /// Is `pid` still a running process? A zombie is not: once its parent is
    /// killed it waits on whoever reaps orphans, which in a container may be
    /// nobody. A kill lands asynchronously, so this waits a little for it.
    #[cfg(unix)]
    fn alive(pid: &str) -> bool {
        let start = Instant::now();
        loop {
            let out = Command::new("ps")
                .args(["-o", "stat=", "-p", pid])
                .output()
                .expect("ps runs");
            let stat = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if stat.is_empty() || stat.starts_with('Z') {
                return false;
            }
            if start.elapsed() > Duration::from_secs(2) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[cfg(unix)]
    fn background_sleeper(pid_file: &Path) -> Command {
        let mut command = Command::new("sh");
        command.arg("-c").arg(format!(
            "sleep 30 & echo $! > '{}'; wait",
            pid_file.display()
        ));
        command
    }

    #[cfg(unix)]
    fn wait_for_pid(pid_file: &Path) -> String {
        let start = Instant::now();
        loop {
            if let Ok(text) = fs::read_to_string(pid_file)
                && text.ends_with('\n')
            {
                return text.trim().to_string();
            }
            assert!(start.elapsed() < Duration::from_secs(5), "no pid");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_timeout_kills_the_whole_operation() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pid_file = dir.path().join("pid");
        let opts = GitOptions {
            timeout: Duration::from_millis(300),
            interactive: false,
        };
        let start = Instant::now();
        let result = run(background_sleeper(&pid_file), &opts, &NoControl);
        assert_eq!(result, Err(GitError::TimedOut));
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "{:?}",
            start.elapsed()
        );
        let pid = wait_for_pid(&pid_file);
        assert!(!alive(&pid), "the background sleep {pid} survived");
    }

    #[cfg(unix)]
    #[test]
    fn a_cancel_kills_the_whole_operation() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pid_file = dir.path().join("pid");
        let ctl = CancelAfter {
            polls: Cell::new(0),
            after: 10,
        };
        let start = Instant::now();
        let result = run(background_sleeper(&pid_file), &GitOptions::default(), &ctl);
        assert_eq!(result, Err(GitError::Cancelled));
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "{:?}",
            start.elapsed()
        );
        let pid = wait_for_pid(&pid_file);
        assert!(!alive(&pid), "the background sleep {pid} survived");
    }

    #[test]
    fn an_already_cancelled_call_runs_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let marker = dir.path().join("ran");
        let mut command = Command::new("sh");
        command
            .arg("-c")
            .arg(format!("touch '{}'", marker.display()));
        let ctl = CancelAfter {
            polls: Cell::new(0),
            after: 0,
        };
        assert_eq!(
            run(command, &GitOptions::default(), &ctl),
            Err(GitError::Cancelled)
        );
        let dest = dir.path().join("clone");
        let result = shallow_clone("/nowhere", None, &dest, &GitOptions::default(), &ctl);
        assert_eq!(result, Err(GitError::Cancelled));
        assert!(!marker.exists());
        assert!(!dest.exists());
    }

    #[test]
    fn a_program_that_is_not_there_is_missing() {
        let command = Command::new("alter-zero-no-such-program-for-git-tests");
        assert_eq!(
            run(command, &GitOptions::default(), &NoControl),
            Err(GitError::Missing)
        );
    }

    #[cfg(unix)]
    #[test]
    fn the_runner_returns_stdout_and_reports_progress() {
        let mut command = Command::new("sh");
        command.arg("-c").arg(
            "printf 'Receiving objects:  45%% (1/2)\\rReceiving objects: 100%% (2/2), done.\\n' >&2; printf out",
        );
        let ctl = Recorder::default();
        assert_eq!(run(command, &GitOptions::default(), &ctl), Ok("out".into()));
        assert_eq!(
            ctl.0.into_inner(),
            [
                GitProgress {
                    phase: "Receiving objects".into(),
                    percent: Some(45)
                },
                GitProgress {
                    phase: "Receiving objects".into(),
                    percent: Some(100)
                }
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_failure_keeps_what_git_said() {
        let mut command = Command::new("sh");
        command
            .arg("-c")
            .arg("echo 'fatal: no such remote' >&2; exit 3");
        let error = run(command, &GitOptions::default(), &NoControl).expect_err("fails");
        assert!(
            matches!(error, GitError::Failed { code: Some(3), .. }),
            "{error:?}"
        );
        assert_eq!(error.to_string(), "git failed: no such remote");
    }

    // --- real repositories ------------------------------------------------

    /// `git` with a fixed identity, so no test depends on the global config.
    fn git(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"])
            .args([
                "-c",
                "init.defaultBranch=main",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(["-c", "tag.gpgsign=false"])
            .args(args)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .output()
            .expect("git runs");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    struct Origin {
        _dir: tempfile::TempDir,
        path: PathBuf,
        first: String,
        second: String,
    }

    impl Origin {
        fn url(&self) -> String {
            format!("file://{}", self.path.display())
        }
    }

    /// An origin with two commits on `main`, a `v1` tag on the first and a
    /// `side` branch; `allow_filter` decides whether it serves partial
    /// clones.
    fn origin(allow_filter: bool) -> Origin {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("origin repo");
        fs::create_dir(&path).expect("origin");
        git(&path, &["init", "-q"]);
        fs::create_dir_all(path.join("plugins/p/skills")).expect("dirs");
        fs::write(path.join("plugins/p/skills/a.md"), "one").expect("file");
        fs::write(path.join("README.md"), "readme").expect("file");
        git(&path, &["add", "-A"]);
        git(&path, &["commit", "-qm", "one"]);
        let first = git(&path, &["rev-parse", "HEAD"]);
        git(&path, &["tag", "-a", "v1", "-m", "v1"]);
        fs::write(path.join("plugins/p/skills/a.md"), "two").expect("file");
        git(&path, &["commit", "-qam", "two"]);
        let second = git(&path, &["rev-parse", "HEAD"]);
        git(&path, &["branch", "side", &first]);
        // A shallow fetch of a commit that is not a branch tip needs the
        // server's permission; GitHub gives it, and so must the fixture.
        git(&path, &["config", "uploadpack.allowAnySHA1InWant", "true"]);
        if allow_filter {
            git(&path, &["config", "uploadpack.allowFilter", "true"]);
        }
        Origin {
            _dir: dir,
            path,
            first,
            second,
        }
    }

    fn opts() -> GitOptions {
        GitOptions {
            timeout: Duration::from_secs(60),
            interactive: false,
        }
    }

    #[test]
    fn git_is_available_here() {
        assert!(git_available());
    }

    #[test]
    fn shallow_clone_by_path_and_by_url() {
        let origin = origin(false);
        let dir = tempfile::tempdir().expect("tempdir");
        let by_path = dir.path().join("a");
        let sha = shallow_clone(
            &origin.path.to_string_lossy(),
            None,
            &by_path,
            &opts(),
            &NoControl,
        )
        .expect("clones");
        assert_eq!(sha, origin.second);
        assert_eq!(
            fs::read_to_string(by_path.join("plugins/p/skills/a.md")).expect("file"),
            "two"
        );
        let by_url = dir.path().join("b");
        let ctl = Recorder::default();
        let sha = shallow_clone(&origin.url(), None, &by_url, &opts(), &ctl).expect("clones");
        assert_eq!(sha, origin.second);
        assert_eq!(
            head_revision(&by_url).as_deref(),
            Some(origin.second.as_str())
        );
        assert_eq!(git(&by_url, &["rev-list", "--count", "HEAD"]), "1");
    }

    #[test]
    fn shallow_clone_of_a_branch_and_a_tag() {
        let origin = origin(false);
        let dir = tempfile::tempdir().expect("tempdir");
        let side = shallow_clone(
            &origin.url(),
            Some("side"),
            &dir.path().join("s"),
            &opts(),
            &NoControl,
        )
        .expect("branch");
        assert_eq!(side, origin.first);
        let tag = shallow_clone(
            &origin.url(),
            Some("v1"),
            &dir.path().join("t"),
            &opts(),
            &NoControl,
        )
        .expect("tag");
        assert_eq!(tag, origin.first);
    }

    #[test]
    fn shallow_clone_refuses_an_existing_destination() {
        let origin = origin(false);
        let dir = tempfile::tempdir().expect("tempdir");
        let error = shallow_clone(&origin.url(), None, dir.path(), &opts(), &NoControl)
            .expect_err("exists");
        assert!(error.to_string().contains("already exists"), "{error}");
    }

    #[test]
    fn a_missing_remote_fails_with_gits_message_and_leaves_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let dest = dir.path().join("clone");
        let missing = dir.path().join("no-such-repo");
        let error = shallow_clone(&missing.to_string_lossy(), None, &dest, &opts(), &NoControl)
            .expect_err("missing");
        assert!(
            matches!(error, GitError::Failed { code: Some(_), .. }),
            "{error:?}"
        );
        assert!(error.to_string().contains("does not exist"), "{error}");
        assert!(!dest.exists());
        let error = remote_revision(
            &format!("file://{}", missing.display()),
            None,
            &opts(),
            &NoControl,
        )
        .expect_err("missing");
        assert!(matches!(error, GitError::Failed { .. }), "{error:?}");
    }

    #[test]
    fn fetch_revision_at_a_pinned_commit_that_is_not_the_tip() {
        let origin = origin(false);
        let dir = tempfile::tempdir().expect("tempdir");
        let dest = dir.path().join("f");
        let sha = fetch_revision(
            &origin.url(),
            Some("main"),
            Some(&origin.first),
            None,
            &dest,
            &opts(),
            &NoControl,
        )
        .expect("fetches");
        assert_eq!(sha, origin.first);
        assert_eq!(
            fs::read_to_string(dest.join("plugins/p/skills/a.md")).expect("file"),
            "one"
        );
    }

    #[test]
    fn fetch_revision_of_a_ref_and_of_head() {
        let origin = origin(false);
        let dir = tempfile::tempdir().expect("tempdir");
        let side = fetch_revision(
            &origin.url(),
            Some("side"),
            None,
            None,
            &dir.path().join("s"),
            &opts(),
            &NoControl,
        )
        .expect("side");
        assert_eq!(side, origin.first);
        let head = fetch_revision(
            &origin.url(),
            None,
            None,
            None,
            &dir.path().join("h"),
            &opts(),
            &NoControl,
        )
        .expect("head");
        assert_eq!(head, origin.second);
    }

    #[test]
    fn fetch_revision_of_a_wrong_commit_fails_and_leaves_nothing() {
        let origin = origin(false);
        let dir = tempfile::tempdir().expect("tempdir");
        let dest = dir.path().join("w");
        let wrong = "0123456789012345678901234567890123456789";
        let result = fetch_revision(
            &origin.url(),
            None,
            Some(wrong),
            None,
            &dest,
            &opts(),
            &NoControl,
        );
        assert!(matches!(result, Err(GitError::Failed { .. })), "{result:?}");
        assert!(!dest.exists());
    }

    fn subdir_fetch(allow_filter: bool) {
        let origin = origin(allow_filter);
        let dir = tempfile::tempdir().expect("tempdir");
        let dest = dir.path().join("sub");
        let sha = fetch_revision(
            &origin.url(),
            None,
            Some(&origin.second),
            Some("plugins/p"),
            &dest,
            &opts(),
            &NoControl,
        )
        .expect("fetches");
        assert_eq!(sha, origin.second);
        assert_eq!(
            fs::read_to_string(dest.join("plugins/p/skills/a.md")).expect("file"),
            "two"
        );
        assert_eq!(
            tree_id(&dest, "plugins/p"),
            Some(git(&origin.path, &["rev-parse", "HEAD:plugins/p"]))
        );
    }

    #[test]
    fn a_subdir_fetch_from_an_origin_that_serves_partial_clones() {
        subdir_fetch(true);
    }

    #[test]
    fn a_subdir_fetch_from_an_origin_that_refuses_the_filter() {
        subdir_fetch(false);
    }

    #[test]
    fn a_subdir_that_is_not_there_fails() {
        let origin = origin(true);
        let dir = tempfile::tempdir().expect("tempdir");
        let dest = dir.path().join("sub");
        let error = fetch_revision(
            &origin.url(),
            None,
            None,
            Some("plugins/nope"),
            &dest,
            &opts(),
            &NoControl,
        )
        .expect_err("no folder");
        assert!(error.to_string().contains("plugins/nope"), "{error}");
        assert!(!dest.exists());
    }

    #[test]
    fn remote_revision_reads_head_branches_and_tags() {
        let origin = origin(false);
        assert_eq!(
            remote_revision(&origin.url(), None, &opts(), &NoControl).as_deref(),
            Ok(origin.second.as_str())
        );
        assert_eq!(
            remote_revision(
                &origin.path.to_string_lossy(),
                Some("side"),
                &opts(),
                &NoControl
            )
            .as_deref(),
            Ok(origin.first.as_str())
        );
        assert_eq!(
            remote_revision(&origin.url(), Some("v1"), &opts(), &NoControl).as_deref(),
            Ok(origin.first.as_str())
        );
        let error =
            remote_revision(&origin.url(), Some("nope"), &opts(), &NoControl).expect_err("no ref");
        assert!(error.to_string().contains("nope"), "{error}");
    }

    #[test]
    fn a_reference_that_looks_like_an_option_is_refused() {
        let origin = origin(false);
        let dir = tempfile::tempdir().expect("tempdir");
        let dest = dir.path().join("x");
        let error = shallow_clone(
            &origin.url(),
            Some("--upload-pack=touch x"),
            &dest,
            &opts(),
            &NoControl,
        )
        .expect_err("refused");
        assert!(
            matches!(error, GitError::Failed { code: None, .. }),
            "{error:?}"
        );
        assert!(!dest.exists());
    }

    #[test]
    fn tree_ids_follow_the_folder() {
        let origin = origin(false);
        let path = &origin.path;
        let before = tree_id(path, "plugins/p").expect("tree");
        let root_before = tree_id(path, "").expect("root tree");
        fs::write(path.join("README.md"), "changed elsewhere").expect("file");
        git(path, &["commit", "-qam", "elsewhere"]);
        assert_eq!(tree_id(path, "plugins/p").as_deref(), Some(before.as_str()));
        assert_ne!(tree_id(path, "").as_deref(), Some(root_before.as_str()));
        fs::write(path.join("plugins/p/new.md"), "new").expect("file");
        git(path, &["add", "-A"]);
        git(path, &["commit", "-qm", "inside"]);
        assert_ne!(
            tree_id(path, "plugins/p/").as_deref(),
            Some(before.as_str())
        );
        assert_eq!(tree_id(path, "plugins/p/skills/a.md"), None);
        assert_eq!(tree_id(path, "plugins/missing"), None);
        assert_eq!(head_revision(&path.join("plugins")), head_revision(path));
        let empty = tempfile::tempdir().expect("tempdir");
        assert_eq!(head_revision(empty.path()), None);
    }
}
