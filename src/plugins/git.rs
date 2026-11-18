//! Running `git` for the plugin store (`docs/plugins.md` *Installing*):
//! fetching a marketplace or a plugin from a repository, one commit deep.
//!
//! `git` is the user's own, so their credential helper and ssh agent answer
//! for a private repository. What this module adds around it:
//!
//! - **no prompt can reach the TUI.** `GIT_TERMINAL_PROMPT=0` stops git's
//!   own, and the process is started **detached from the terminal** through
//!   the same tiers every shell child uses (`subprocess::spawn_shell_with`,
//!   `docs/tty-detach.md`) — ssh asks for a passphrase on `/dev/tty`, which a
//!   process without one cannot open, so it fails in a captured error
//!   instead of printing over the screen and reading the user's keys;
//! - **no transport but the ordinary ones** (`GIT_ALLOW_PROTOCOL`): the
//!   `ext::` helper runs a command named in the URL;
//! - **no repository but the one asked for**: the variables that redirect
//!   git elsewhere (`GIT_DIR`, `GIT_WORK_TREE`, …) are removed, so a session
//!   started from inside a git hook does not clone into that repository;
//! - **Esc and a clock**: the turn's [`CancelToken`] is polled every 20 ms,
//!   each command has [`GIT_TIMEOUT`], and either kills the process group.

use std::io::Read;
use std::path::Path;
use std::process::Child;
use std::time::{Duration, Instant};

use crate::stream::CancelToken;

/// How long one git command may take.
pub const GIT_TIMEOUT: Duration = Duration::from_secs(120);

/// How much of a git command's output is kept: enough for `rev-parse` and
/// for the tail of an error.
const OUTPUT_LIMIT: usize = 64 * 1024;

/// The cancel-poll cadence every blocking wait here shares with the rest of
/// the app.
const POLL: Duration = Duration::from_millis(20);

/// The transports a URL may name.
const ALLOWED_PROTOCOLS: &str = "file:git:http:https:ssh";

/// The variables that point git at another repository than the one in the
/// working directory — codex's list.
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

/// `arg` as one single-quoted shell word.
fn shell_quote(arg: &str) -> String {
    format!("'{}'", arg.replace('\'', r"'\''"))
}

/// Drain `pipe` on a thread of its own, keeping its first `limit` bytes —
/// so a chatty command can neither fill memory nor stall on a full pipe.
fn drain(pipe: Option<impl Read + Send + 'static>) -> std::thread::JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut kept = Vec::new();
        let Some(mut pipe) = pipe else {
            return kept;
        };
        let mut buf = [0u8; 8192];
        while let Ok(read) = pipe.read(&mut buf) {
            if read == 0 {
                break;
            }
            let room = OUTPUT_LIMIT.saturating_sub(kept.len());
            kept.extend_from_slice(&buf[..read.min(room)]);
        }
        kept
    })
}

/// Runs git commands for one operation.
#[derive(Debug, Clone, Copy)]
pub struct Git<'a> {
    /// This binary, for the detach tier that re-execs it where no `setsid`
    /// binary exists (`docs/tty-detach.md`).
    pub detach_helper: Option<&'a Path>,
    pub cancel: &'a CancelToken,
    pub timeout: Duration,
}

impl Git<'_> {
    /// Run `git {args}` in `cwd` and return its standard output.
    ///
    /// # Errors
    /// Git could not be started, it failed (the tail of what it said), it
    /// ran past the timeout, or the operation was cancelled.
    pub fn run(&self, cwd: &Path, args: &[&str]) -> Result<String, String> {
        let line = std::iter::once("exec git".to_string())
            .chain(args.iter().map(|arg| shell_quote(arg)))
            .collect::<Vec<_>>()
            .join(" ");
        let child = crate::subprocess::spawn_shell_with(self.detach_helper, &line, |command| {
            command
                .current_dir(cwd)
                .env("GIT_TERMINAL_PROMPT", "0")
                .env("GCM_INTERACTIVE", "never")
                .env("GIT_ALLOW_PROTOCOL", ALLOWED_PROTOCOLS)
                .env("GIT_OPTIONAL_LOCKS", "0")
                .env("LC_ALL", "C");
            for variable in REDIRECTING_VARIABLES {
                command.env_remove(variable);
            }
        })
        .map_err(|err| format!("could not run git: {err}"))?;
        self.wait(child, args.first().copied().unwrap_or("git"))
    }

    /// Wait for `child`, polling the cancel token and the clock.
    fn wait(&self, mut child: Child, what: &str) -> Result<String, String> {
        let stdout = drain(child.stdout.take());
        let stderr = drain(child.stderr.take());
        let started = Instant::now();
        let status = loop {
            if self.cancel.is_cancelled() {
                crate::subprocess::kill_process_group(&mut child);
                return Err("cancelled".to_string());
            }
            if started.elapsed() >= self.timeout {
                crate::subprocess::kill_process_group(&mut child);
                return Err(format!(
                    "git {what} took longer than {}s",
                    self.timeout.as_secs()
                ));
            }
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => std::thread::sleep(POLL),
                Err(err) => {
                    crate::subprocess::kill_process_group(&mut child);
                    return Err(format!("could not wait for git: {err}"));
                }
            }
        };
        let out = String::from_utf8_lossy(&stdout.join().unwrap_or_default()).into_owned();
        let err = String::from_utf8_lossy(&stderr.join().unwrap_or_default()).into_owned();
        if status.success() {
            return Ok(out);
        }
        let said = err
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with("hint:"))
            .collect::<Vec<_>>()
            .join(" ");
        let said: String = said.chars().take(400).collect();
        Err(if said.is_empty() {
            format!("git {what} failed ({status})")
        } else {
            format!("git {what} failed: {said}")
        })
    }

    /// Fetch one commit of the repository at `url` into the new directory
    /// `dest` — the commit `sha` names when one is pinned, else the head of
    /// `ref_name`, else the default branch — and check it out, only the
    /// directory `subdir` of it when one is named. Returns the commit
    /// checked out.
    ///
    /// One commit deep, since only its files are wanted; a pinned commit the
    /// server will not hand out on its own is reached by fetching the whole
    /// history instead, and the checkout must land on it.
    ///
    /// # Errors
    /// Any git command's failure, or a checkout on another commit than the
    /// one pinned.
    pub fn fetch_commit(
        &self,
        url: &str,
        ref_name: Option<&str>,
        sha: Option<&str>,
        subdir: Option<&str>,
        dest: &Path,
    ) -> Result<String, String> {
        std::fs::create_dir_all(dest).map_err(|err| format!("{}: {err}", dest.display()))?;
        self.run(dest, &["init", "-q"])?;
        self.run(dest, &["remote", "add", "origin", url])?;
        if let Some(subdir) = subdir {
            // The classic sparse checkout: a pattern file, written rather
            // than handed to `sparse-checkout set`, so no path can be read
            // as one of its options.
            self.run(dest, &["config", "core.sparseCheckout", "true"])?;
            let patterns = dest.join(".git").join("info").join("sparse-checkout");
            if let Some(parent) = patterns.parent() {
                std::fs::create_dir_all(parent).map_err(|err| err.to_string())?;
            }
            std::fs::write(&patterns, format!("/{subdir}/\n")).map_err(|err| err.to_string())?;
        }
        let full_sha = sha.filter(|sha| sha.len() == 40);
        let target = match (full_sha, sha, ref_name) {
            (Some(sha), _, _) => {
                match self.run(
                    dest,
                    &["fetch", "-q", "--depth", "1", "--no-tags", "origin", sha],
                ) {
                    Ok(_) => "FETCH_HEAD".to_string(),
                    Err(_) => {
                        self.run(dest, &["fetch", "-q", "--no-tags", "origin"])?;
                        sha.to_string()
                    }
                }
            }
            (None, Some(sha), _) => {
                // An abbreviated commit cannot be fetched by name.
                self.run(dest, &["fetch", "-q", "--no-tags", "origin"])?;
                sha.to_string()
            }
            (None, None, Some(ref_name)) => {
                self.run(
                    dest,
                    &[
                        "fetch",
                        "-q",
                        "--depth",
                        "1",
                        "--no-tags",
                        "origin",
                        ref_name,
                    ],
                )?;
                "FETCH_HEAD".to_string()
            }
            (None, None, None) => {
                self.run(
                    dest,
                    &["fetch", "-q", "--depth", "1", "--no-tags", "origin", "HEAD"],
                )?;
                "FETCH_HEAD".to_string()
            }
        };
        self.run(dest, &["checkout", "-q", "--detach", &target])?;
        let revision = self.run(dest, &["rev-parse", "HEAD"])?.trim().to_string();
        if let Some(sha) = sha
            && !revision.starts_with(&sha.to_ascii_lowercase())
        {
            return Err(format!(
                "checked out {revision}, not the pinned commit {sha}"
            ));
        }
        Ok(revision)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_argument_is_one_shell_word_whatever_it_holds() {
        assert_eq!(shell_quote("plain"), "'plain'");
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
        assert_eq!(shell_quote("$(touch x) `y` ; z"), "'$(touch x) `y` ; z'");
    }

    #[test]
    fn a_failure_says_what_git_said() {
        let cancel = CancelToken::new();
        let git = Git {
            detach_helper: None,
            cancel: &cancel,
            timeout: GIT_TIMEOUT,
        };
        let tmp = tempfile::tempdir().unwrap();
        let err = git.run(tmp.path(), &["rev-parse", "HEAD"]).unwrap_err();
        assert!(err.starts_with("git rev-parse failed"), "{err}");
        assert!(err.contains("not a git repository"), "{err}");
    }

    #[test]
    fn a_cancelled_operation_runs_nothing() {
        let cancel = CancelToken::new();
        cancel.cancel();
        let git = Git {
            detach_helper: None,
            cancel: &cancel,
            timeout: GIT_TIMEOUT,
        };
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(
            git.run(tmp.path(), &["--version"]).unwrap_err(),
            "cancelled"
        );
    }

    #[test]
    fn an_ext_transport_is_refused() {
        let cancel = CancelToken::new();
        let git = Git {
            detach_helper: None,
            cancel: &cancel,
            timeout: GIT_TIMEOUT,
        };
        let tmp = tempfile::tempdir().unwrap();
        let marker = tmp.path().join("ran");
        let url = format!("ext::sh -c touch% {}", marker.display());
        let dest = tmp.path().join("dest");
        assert!(git.fetch_commit(&url, None, None, None, &dest).is_err());
        assert!(!marker.exists(), "the transport helper must not have run");
    }
}
