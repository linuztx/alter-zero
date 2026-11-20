//! Running `git` for the plugin store (`docs/plugins.md` *Fetching*).
//!
//! `git` is the user's own, found on `PATH` and run as a child, so their
//! credential helper and ssh agent answer for a private repository and this
//! process links nothing. What this module adds around it:
//!
//! - **no prompt can reach the TUI.** `GIT_TERMINAL_PROMPT=0` stops git's
//!   own, askpass is refused (`SSH_ASKPASS_REQUIRE=never`), stdin is closed,
//!   and the process is built by an **injected** [`CommandBuilder`] — the
//!   app passes one over its tty-detach tiers (`docs/tty-detach.md`), so a
//!   passphrase prompt on `/dev/tty` fails in a captured error. A
//!   `GIT_SSH_COMMAND` of the user's own is kept, with `-oBatchMode=yes`
//!   after it; none is set where they have none, since that would override
//!   their `core.sshCommand`;
//! - **no transport but the ordinary ones** (`GIT_ALLOW_PROTOCOL`): `ext::`
//!   runs a command named in the URL;
//! - **no repository but the one asked for**: the variables that redirect
//!   git elsewhere (`GIT_DIR`, `GIT_WORK_TREE`, …) are removed and
//!   `safe.bareRepository=explicit`, `core.fsmonitor=false` and
//!   `core.hooksPath=/dev/null` are set on the command line;
//! - **bounded output**: stdout up to [`OUTPUT_LIMIT`], stderr's last
//!   [`ERROR_TAIL`] bytes — the tail is the error message;
//! - **Esc and a clock**: the operation's [`CancelToken`] is polled every
//!   20 ms and each command has a timeout; either kills the whole process
//!   group, the `ssh` and `git-remote-https` helpers included.

use std::collections::VecDeque;
use std::fmt;
use std::io::Read;
use std::path::Path;
use std::process::Child;
use std::time::{Duration, Instant};

use std::ffi::OsString;
use std::process::{Command, Stdio};
use std::sync::Arc;

use crate::cancel::CancelToken;

/// The variable that bounds one network git command, in seconds.
pub const GIT_TIMEOUT_ENV: &str = "ALTER_ZERO_PLUGIN_GIT_TIMEOUT_SECS";
/// The bound on one git command when [`GIT_TIMEOUT_ENV`] says nothing.
pub const DEFAULT_GIT_TIMEOUT: Duration = Duration::from_secs(300);
/// How much of a command's standard output is kept.
pub const OUTPUT_LIMIT: usize = 4 * 1024 * 1024;
/// How much of the end of a command's standard error is kept.
pub const ERROR_TAIL: usize = 8 * 1024;
/// The cancel-poll cadence.
const POLL: Duration = Duration::from_millis(20);
/// The transports a URL may name.
pub const ALLOWED_PROTOCOLS: &str = "file:git:http:https:ssh";
/// The variables that point git at another repository than the one asked
/// for — codex's list.
const REDIRECTING_VARIABLES: &[&str] = &[
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_CEILING_DIRECTORIES",
    "GIT_COMMON_DIR",
    "GIT_CONFIG",
    "GIT_CONFIG_COUNT",
    "GIT_CONFIG_PARAMETERS",
    "GIT_DIR",
    "GIT_DISCOVERY_ACROSS_FILESYSTEM",
    "GIT_GRAFT_FILE",
    "GIT_IMPLICIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_NAMESPACE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_PREFIX",
    "GIT_REPLACE_REF_BASE",
    "GIT_SHALLOW_FILE",
    "GIT_WORK_TREE",
];

/// The timeout [`GIT_TIMEOUT_ENV`]'s value asks for: whole seconds above
/// zero, else [`DEFAULT_GIT_TIMEOUT`].
#[must_use]
pub fn git_timeout(value: Option<&str>) -> Duration {
    value
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|secs| *secs > 0)
        .map_or(DEFAULT_GIT_TIMEOUT, Duration::from_secs)
}

/// Why a git command did not succeed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitError {
    /// It could not be started (no `git` on `PATH`).
    Spawn(String),
    /// The operation was cancelled.
    Cancelled,
    /// It ran past the timeout.
    TimedOut {
        /// The subcommand.
        what: String,
        /// The bound, in seconds.
        secs: u64,
    },
    /// It exited non-zero.
    Failed {
        /// The subcommand.
        what: String,
        /// The tail of what it said.
        message: String,
    },
}

impl fmt::Display for GitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Spawn(error) => write!(f, "git could not be run ({error}); is it installed?"),
            Self::Cancelled => f.write_str("cancelled"),
            Self::TimedOut { what, secs } => {
                write!(f, "git {what} took longer than {secs}s and was stopped")
            }
            Self::Failed { what, message } if message.is_empty() => write!(f, "git {what} failed"),
            Self::Failed { what, message } => write!(f, "git {what} failed: {message}"),
        }
    }
}

impl std::error::Error for GitError {}

/// Turns an argv (`git`, then its arguments) into a `Command` ready to be
/// configured and spawned. It must start the child as the leader of its own
/// process group (pgid == pid — `setsid` or `process_group(0)`), so a cancel
/// or a timeout kills the child **and its helpers** (`ssh`,
/// `git-remote-https`), and should leave it no controlling terminal. The
/// runner sets the working directory, the environment and the stdio itself.
pub type CommandBuilder = Arc<dyn Fn(&[OsString]) -> Command + Send + Sync>;

/// The builder a runner uses unless given one: the program run directly in
/// a process group of its own. It keeps the controlling terminal, so the app
/// injects its detach chain instead (`docs/tty-detach.md`).
#[must_use]
pub fn plain_command(argv: &[OsString]) -> Command {
    let mut command = Command::new(argv.first().cloned().unwrap_or_default());
    command.args(argv.iter().skip(1));
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    command
}

/// The `GIT_SSH_COMMAND` a child is given, from the user's own: theirs with
/// `-oBatchMode=yes` after it, so the key or jump host it names still
/// applies and ssh is only told not to ask. `None` leaves the variable as
/// it is — unset, where setting one would override the `core.sshCommand`
/// and `GIT_SSH` a private repository may depend on; nothing can prompt
/// there either, since the child has no terminal and askpass is refused.
fn ssh_command(existing: Option<OsString>) -> Option<OsString> {
    let mut command = existing.filter(|value| !value.to_string_lossy().trim().is_empty())?;
    command.push(" -oBatchMode=yes");
    Some(command)
}

/// Kill `child`'s whole process group (it leads one — [`CommandBuilder`]'s
/// contract) and reap it.
fn kill_group(child: &mut Child) {
    #[cfg(unix)]
    if let Some(pid) = i32::try_from(child.id())
        .ok()
        .and_then(rustix::process::Pid::from_raw)
    {
        let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// What a command said on stderr, as one line: its last lines, `hint:`
/// lines dropped, cut to 400 characters from the end.
#[must_use]
pub fn error_message(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let said = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with("hint:"))
        .collect::<Vec<_>>()
        .join(" ");
    let count = said.chars().count();
    if count > 400 {
        format!("…{}", said.chars().skip(count - 400).collect::<String>())
    } else {
        said
    }
}

/// Drain `pipe` on its own thread, keeping its first `limit` bytes (`tail`
/// false) or its last (`tail` true).
fn drain(
    pipe: Option<impl Read + Send + 'static>,
    limit: usize,
    tail: bool,
) -> std::thread::JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut kept: VecDeque<u8> = VecDeque::new();
        let Some(mut pipe) = pipe else {
            return Vec::new();
        };
        let mut buf = [0u8; 8192];
        while let Ok(read) = pipe.read(&mut buf) {
            if read == 0 {
                break;
            }
            if tail {
                kept.extend(&buf[..read]);
                while kept.len() > limit {
                    kept.pop_front();
                }
            } else {
                let room = limit.saturating_sub(kept.len());
                kept.extend(&buf[..read.min(room)]);
            }
        }
        kept.into()
    })
}

/// Runs git commands for one operation.
#[derive(Clone)]
pub struct GitRunner {
    /// How a child is built ([`CommandBuilder`]'s contract).
    pub command: CommandBuilder,
    /// The bound on one command.
    pub timeout: Duration,
}

impl Default for GitRunner {
    fn default() -> Self {
        Self {
            command: Arc::new(plain_command),
            timeout: DEFAULT_GIT_TIMEOUT,
        }
    }
}

impl std::fmt::Debug for GitRunner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GitRunner")
            .field("timeout", &self.timeout)
            .finish_non_exhaustive()
    }
}

impl GitRunner {
    /// Run `git {args}` in `cwd` and return its standard output.
    ///
    /// # Errors
    /// [`GitError`]: not started, failed, timed out, cancelled.
    pub fn run(&self, cwd: &Path, args: &[&str], cancel: &CancelToken) -> Result<String, GitError> {
        let mut all = vec![
            "-c",
            "safe.bareRepository=explicit",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "advice.detachedHead=false",
        ];
        all.extend_from_slice(args);
        let what = args
            .iter()
            .find(|a| !a.starts_with('-'))
            .copied()
            .unwrap_or("git");
        self.run_program("git", cwd, &all, what, cancel)
    }

    /// Run `program {args}` the hardened way — the seam [`GitRunner::run`]
    /// goes through, kept apart so the wait can be tested on a program that
    /// is not git.
    fn run_program(
        &self,
        program: &str,
        cwd: &Path,
        args: &[&str],
        what: &str,
        cancel: &CancelToken,
    ) -> Result<String, GitError> {
        if cancel.is_cancelled() {
            return Err(GitError::Cancelled);
        }
        let argv: Vec<OsString> = std::iter::once(OsString::from(program))
            .chain(args.iter().map(OsString::from))
            .collect();
        let mut command = (self.command)(&argv);
        command
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GCM_INTERACTIVE", "never")
            .env("SSH_ASKPASS_REQUIRE", "never")
            .env("GIT_ALLOW_PROTOCOL", ALLOWED_PROTOCOLS)
            .env("GIT_OPTIONAL_LOCKS", "0")
            .env("LC_ALL", "C");
        if let Some(ssh) = ssh_command(std::env::var_os("GIT_SSH_COMMAND")) {
            command.env("GIT_SSH_COMMAND", ssh);
        }
        for variable in REDIRECTING_VARIABLES {
            command.env_remove(variable);
        }
        let child = command
            .spawn()
            .map_err(|err| GitError::Spawn(err.to_string()))?;
        self.wait(child, what, cancel)
    }

    fn wait(&self, mut child: Child, what: &str, cancel: &CancelToken) -> Result<String, GitError> {
        let stdout = drain(child.stdout.take(), OUTPUT_LIMIT, false);
        let stderr = drain(child.stderr.take(), ERROR_TAIL, true);
        let started = Instant::now();
        let status = loop {
            if cancel.is_cancelled() {
                kill_group(&mut child);
                return Err(GitError::Cancelled);
            }
            if started.elapsed() >= self.timeout {
                kill_group(&mut child);
                return Err(GitError::TimedOut {
                    what: what.to_string(),
                    secs: self.timeout.as_secs(),
                });
            }
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => std::thread::sleep(POLL),
                Err(err) => {
                    kill_group(&mut child);
                    return Err(GitError::Spawn(err.to_string()));
                }
            }
        };
        let out = String::from_utf8_lossy(&stdout.join().unwrap_or_default()).into_owned();
        let err = stderr.join().unwrap_or_default();
        if status.success() {
            return Ok(out);
        }
        let mut message = error_message(&err);
        if message.is_empty() {
            message = status.to_string();
        }
        Err(GitError::Failed {
            what: what.to_string(),
            message,
        })
    }

    /// The commit `HEAD` names in the repository at `repo`.
    ///
    /// # Errors
    /// [`GitRunner::run`]'s.
    pub fn head(&self, repo: &Path, cancel: &CancelToken) -> Result<String, GitError> {
        Ok(self
            .run(repo, &["rev-parse", "HEAD"], cancel)?
            .trim()
            .to_string())
    }

    /// Clone the marketplace at `url` into the new directory `dest`, one
    /// commit deep, blobless and sparse over the catalog directories — the
    /// whole repository's trees but only the catalogs' files — falling back
    /// to a plain shallow clone where the server refuses a filter. Returns
    /// the commit checked out.
    ///
    /// # Errors
    /// The clone failed both ways, or was cancelled.
    pub fn clone_marketplace(
        &self,
        url: &str,
        git_ref: Option<&str>,
        dest: &Path,
        cancel: &CancelToken,
    ) -> Result<String, GitError> {
        let parent = dest.parent().unwrap_or(dest);
        let dest_text = dest.to_string_lossy();
        let mut sparse = vec![
            "clone",
            "-q",
            "--depth",
            "1",
            "--no-tags",
            "--filter=blob:none",
            "--sparse",
        ];
        let mut plain = vec!["clone", "-q", "--depth", "1", "--no-tags"];
        if let Some(r) = git_ref {
            sparse.extend(["--branch", r]);
            plain.extend(["--branch", r]);
        }
        sparse.extend(["--", url, &dest_text]);
        plain.extend(["--", url, &dest_text]);
        match self.run(parent, &sparse, cancel) {
            Ok(_) => {
                self.run(
                    dest,
                    &[
                        "sparse-checkout",
                        "set",
                        "--",
                        ".agents/plugins",
                        ".claude-plugin",
                        ".cursor-plugin",
                        ".codex-plugin",
                    ],
                    cancel,
                )?;
            }
            Err(GitError::Failed { .. }) => {
                let _ = std::fs::remove_dir_all(dest);
                self.run(parent, &plain, cancel)?;
            }
            Err(other) => return Err(other),
        }
        self.head(dest, cancel)
    }

    /// Fetch `git_ref` (else `HEAD`) of the marketplace clone at `repo` one
    /// commit deep and check it out. Returns the commit now checked out.
    ///
    /// # Errors
    /// [`GitRunner::run`]'s.
    pub fn update_clone(
        &self,
        repo: &Path,
        git_ref: Option<&str>,
        cancel: &CancelToken,
    ) -> Result<String, GitError> {
        let target = git_ref.unwrap_or("HEAD");
        self.run(
            repo,
            &["fetch", "-q", "--depth", "1", "--no-tags", "origin", target],
            cancel,
        )?;
        self.run(repo, &["checkout", "-q", "--detach", "FETCH_HEAD"], cancel)?;
        self.head(repo, cancel)
    }

    /// Make the folder `path` of a sparse marketplace clone present (its
    /// blobs fetched on demand) — `""`, the whole repository (a marketplace
    /// of one). A clone that is not sparse has it already.
    ///
    /// # Errors
    /// [`GitRunner::run`]'s.
    pub fn materialize(
        &self,
        repo: &Path,
        path: &str,
        cancel: &CancelToken,
    ) -> Result<(), GitError> {
        let sparse = self
            .run(repo, &["config", "--bool", "core.sparseCheckout"], cancel)
            .map(|out| out.trim() == "true")
            .unwrap_or(false);
        if !sparse {
            return Ok(());
        }
        if path.is_empty() {
            self.run(repo, &["sparse-checkout", "disable"], cancel)?;
        } else {
            self.run(repo, &["sparse-checkout", "add", "--", path], cancel)?;
        }
        Ok(())
    }

    /// The tree id of the folder `path` at `HEAD` (the commit's own tree for
    /// `""`) — what an in-repo plugin's revision is.
    ///
    /// # Errors
    /// [`GitRunner::run`]'s.
    pub fn tree_of(
        &self,
        repo: &Path,
        path: &str,
        cancel: &CancelToken,
    ) -> Result<String, GitError> {
        let spec = format!("HEAD:{path}");
        Ok(self
            .run(repo, &["rev-parse", "--verify", "-q", &spec], cancel)?
            .trim()
            .to_string())
    }

    /// Every file path under `path` at `HEAD`, relative to `path` — what a
    /// blobless clone knows without fetching a byte.
    ///
    /// # Errors
    /// [`GitRunner::run`]'s.
    pub fn list_tree(
        &self,
        repo: &Path,
        path: &str,
        cancel: &CancelToken,
    ) -> Result<Vec<String>, GitError> {
        let out = if path.is_empty() {
            self.run(repo, &["ls-tree", "-r", "--name-only", "HEAD"], cancel)?
        } else {
            self.run(
                repo,
                &["ls-tree", "-r", "--name-only", "HEAD", "--", path],
                cancel,
            )?
        };
        let prefix = if path.is_empty() {
            String::new()
        } else {
            format!("{path}/")
        };
        Ok(out
            .lines()
            .filter_map(|line| line.strip_prefix(&prefix).map(str::to_string))
            .collect())
    }

    /// Fetch one commit of the repository at `url` into the new directory
    /// `dest` — `sha` when pinned, else the head of `git_ref`, else the
    /// default branch — only `subdir` checked out when one is named. A
    /// pinned commit the server will not serve on its own is reached by
    /// fetching the history instead, and the checkout must land on it.
    /// Returns the commit checked out.
    ///
    /// # Errors
    /// Any command's failure, or a checkout on another commit than the one
    /// pinned.
    pub fn fetch_commit(
        &self,
        url: &str,
        git_ref: Option<&str>,
        sha: Option<&str>,
        subdir: Option<&str>,
        dest: &Path,
        cancel: &CancelToken,
    ) -> Result<String, GitError> {
        std::fs::create_dir_all(dest)
            .map_err(|err| GitError::Spawn(format!("{}: {err}", dest.display())))?;
        self.run(dest, &["init", "-q"], cancel)?;
        self.run(dest, &["remote", "add", "origin", url], cancel)?;
        if let Some(subdir) = subdir {
            // The pattern file is written rather than handed to
            // `sparse-checkout set`, so no path is ever read as an option.
            self.run(dest, &["config", "core.sparseCheckout", "true"], cancel)?;
            let patterns = dest.join(".git").join("info").join("sparse-checkout");
            if let Some(parent) = patterns.parent() {
                std::fs::create_dir_all(parent).map_err(|err| GitError::Spawn(err.to_string()))?;
            }
            std::fs::write(&patterns, format!("/{subdir}/\n"))
                .map_err(|err| GitError::Spawn(err.to_string()))?;
        }
        let fetch_shallow = |target: &str| {
            self.run(
                dest,
                &["fetch", "-q", "--depth", "1", "--no-tags", "origin", target],
                cancel,
            )
        };
        let target = match (sha, git_ref) {
            (Some(sha), _) if sha.len() == 40 => match fetch_shallow(sha) {
                Ok(_) => "FETCH_HEAD".to_string(),
                Err(GitError::Failed { .. }) => {
                    self.run(dest, &["fetch", "-q", "--no-tags", "origin"], cancel)?;
                    sha.to_string()
                }
                Err(other) => return Err(other),
            },
            (Some(sha), _) => {
                self.run(dest, &["fetch", "-q", "--no-tags", "origin"], cancel)?;
                sha.to_string()
            }
            (None, Some(r)) => {
                fetch_shallow(r)?;
                "FETCH_HEAD".to_string()
            }
            (None, None) => {
                fetch_shallow("HEAD")?;
                "FETCH_HEAD".to_string()
            }
        };
        self.run(dest, &["checkout", "-q", "--detach", &target], cancel)?;
        let revision = self.head(dest, cancel)?;
        if let Some(sha) = sha
            && !revision.starts_with(&sha.to_ascii_lowercase())
        {
            return Err(GitError::Failed {
                what: "checkout".into(),
                message: format!("it checked out {revision}, not the pinned commit {sha}"),
            });
        }
        Ok(revision)
    }

    /// The commit `git_ref` (else `HEAD`) names on the remote `url`, asked
    /// without cloning.
    ///
    /// # Errors
    /// [`GitRunner::run`]'s, or a ref the remote does not have.
    pub fn remote_head(
        &self,
        cwd: &Path,
        url: &str,
        git_ref: Option<&str>,
        cancel: &CancelToken,
    ) -> Result<String, GitError> {
        let target = git_ref.unwrap_or("HEAD");
        let out = self.run(cwd, &["ls-remote", "--", url, target], cancel)?;
        out.split_whitespace()
            .next()
            .map(str::to_string)
            .ok_or_else(|| GitError::Failed {
                what: "ls-remote".into(),
                message: format!("the repository has no {target}"),
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runner() -> GitRunner {
        GitRunner {
            command: Arc::new(plain_command),
            timeout: Duration::from_secs(30),
        }
    }

    #[test]
    fn the_timeout_comes_from_the_environment_or_the_default() {
        assert_eq!(git_timeout(None), DEFAULT_GIT_TIMEOUT);
        assert_eq!(git_timeout(Some("12")), Duration::from_secs(12));
        assert_eq!(git_timeout(Some("0")), DEFAULT_GIT_TIMEOUT);
        assert_eq!(git_timeout(Some("x")), DEFAULT_GIT_TIMEOUT);
    }

    #[test]
    fn the_user_s_own_ssh_command_is_kept_and_only_told_not_to_prompt() {
        // Their own command (a key, a jump host) still runs, with BatchMode
        // after it: ssh takes the first value an option is given, so a
        // BatchMode of their own still wins.
        assert_eq!(
            ssh_command(Some(OsString::from("ssh -i ~/.ssh/work"))),
            Some(OsString::from("ssh -i ~/.ssh/work -oBatchMode=yes"))
        );
        // With none set the variable is left alone: setting it would
        // override `core.sshCommand` and `GIT_SSH`, which the user's
        // private repositories may depend on. Nothing can prompt anyway —
        // the child has no terminal and askpass is refused.
        assert_eq!(ssh_command(None), None);
        assert_eq!(ssh_command(Some(OsString::from("  "))), None);
    }

    #[test]
    fn the_error_message_is_the_tail_without_hints() {
        assert_eq!(error_message(b"hint: x\nfatal: no\n\n"), "fatal: no");
        let long = "e".repeat(1000);
        let message = error_message(long.as_bytes());
        assert_eq!(message.chars().count(), 401);
        assert!(message.starts_with('…'));
    }

    #[test]
    fn a_failure_says_what_git_said() {
        if Command::new("git").arg("--version").output().is_err() {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let err = runner()
            .run(tmp.path(), &["rev-parse", "HEAD"], &CancelToken::new())
            .unwrap_err();
        let text = err.to_string();
        assert!(text.starts_with("git rev-parse failed"), "{text}");
        assert!(text.contains("not a git repository"), "{text}");
    }

    #[test]
    fn a_cancelled_operation_runs_nothing() {
        let cancel = CancelToken::new();
        cancel.cancel();
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(
            runner().run(tmp.path(), &["--version"], &cancel),
            Err(GitError::Cancelled)
        );
    }

    #[test]
    fn cancelling_mid_run_kills_the_whole_group_within_the_poll() {
        let cancel = CancelToken::new();
        let stopper = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            stopper.cancel();
        });
        let tmp = tempfile::tempdir().unwrap();
        let started = Instant::now();
        let result = runner().run_program(
            "sh",
            tmp.path(),
            &["-c", "sleep 30 & sleep 30"],
            "sh",
            &cancel,
        );
        assert_eq!(result, Err(GitError::Cancelled));
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
    }

    #[test]
    fn a_command_past_its_timeout_is_stopped() {
        let tmp = tempfile::tempdir().unwrap();
        let quick = GitRunner {
            timeout: Duration::from_millis(200),
            ..runner()
        };
        let err = quick
            .run_program("sleep", tmp.path(), &["30"], "sleep", &CancelToken::new())
            .unwrap_err();
        assert!(matches!(err, GitError::TimedOut { .. }), "{err}");
    }

    #[test]
    fn the_error_kept_is_the_end_of_a_long_stderr() {
        let tmp = tempfile::tempdir().unwrap();
        let err = runner()
            .run_program(
                "sh",
                tmp.path(),
                &["-c", "i=0; while [ $i -lt 3000 ]; do echo noise $i >&2; i=$((i+1)); done; echo LAST >&2; exit 3"],
                "sh",
                &CancelToken::new(),
            )
            .unwrap_err();
        assert!(err.to_string().ends_with("LAST"), "{err}");
    }

    #[test]
    fn an_ext_transport_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let marker = tmp.path().join("ran");
        let url = format!("ext::sh -c touch% {}", marker.display());
        let dest = tmp.path().join("dest");
        assert!(
            runner()
                .fetch_commit(&url, None, None, None, &dest, &CancelToken::new())
                .is_err()
        );
        assert!(!marker.exists(), "the transport helper must not have run");
    }
}
