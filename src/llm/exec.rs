//! The real tool executor — the I/O boundary that actually runs a `bash`
//! command and reads/writes the filesystem for the model's tool calls (see
//! `docs/tools.md`). The pure cores it wraps (arg parse, the edit engine, the
//! read formatter, the diff) live in [`crate::llm::tools`].
//!
//! Boundary code like `main.rs`/`term.rs`: the process/file I/O is verified by
//! hand and `scripts/smoke.sh`, but the deterministic file operations
//! (read/write/edit round-trips) carry focused tests over temp files.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use super::tools::{
    self, BashArgs, EditArgs, ReadArgs, TOOL_OUTPUT_MAX_BYTES, ToolCallRequest, ToolOutcome,
    WriteArgs,
};
use crate::stream::CancelToken;

/// Runs the model's tool calls. Implemented by [`RealToolExecutor`] in
/// production and by fakes in the agent-loop tests.
pub trait ToolExecutor {
    /// Execute one tool call, returning the model-facing outcome. Never
    /// panics — every failure becomes a non-ok [`ToolOutcome`] the model reads
    /// and can recover from.
    fn execute(&self, call: &ToolCallRequest, cancel: &CancelToken) -> ToolOutcome;
}

/// How often the `bash` runner polls a running child for completion, a cancel,
/// or a timeout — short enough that Esc / a timeout reaps promptly.
const BASH_POLL_INTERVAL: Duration = Duration::from_millis(20);

/// The real executor: runs commands under `sh -c` and touches the filesystem in
/// the app's working directory. Stateless — paths resolve against the process
/// cwd, the same trust model as the `!` shell (`docs/shell-command.md`).
#[derive(Debug, Clone, Copy, Default)]
pub struct RealToolExecutor;

impl RealToolExecutor {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl ToolExecutor for RealToolExecutor {
    fn execute(&self, call: &ToolCallRequest, cancel: &CancelToken) -> ToolOutcome {
        match call.name.as_str() {
            "bash" => run_bash(&call.arguments, cancel),
            "read" => run_read(&call.arguments),
            "write" => run_write(&call.arguments),
            "edit" => run_edit(&call.arguments),
            other => ToolOutcome::error(format!("unknown tool: {other}")),
        }
    }
}

/// A one-tool argument-parse error → a model-facing failure outcome.
fn arg_error(err: String) -> ToolOutcome {
    ToolOutcome::error(err)
}

/// `bash`: run the command under `sh -c`, capture combined stdout+stderr
/// (byte-capped), enforce the per-call timeout, and kill on cancel. The output
/// is framed as codex does (`Exit code: N` + output); a non-zero exit or a
/// timeout resolves the cell red.
fn run_bash(arguments: &str, cancel: &CancelToken) -> ToolOutcome {
    let args: BashArgs = match tools::parse_args(arguments) {
        Ok(a) => a,
        Err(e) => return arg_error(e),
    };
    let timeout = Duration::from_millis(args.timeout_ms());

    let mut child = match Command::new("sh")
        .arg("-c")
        .arg(&args.command)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(err) => return ToolOutcome::error(format!("failed to run command: {err}")),
    };

    // Drain both pipes on their own threads so a chatty command can't deadlock
    // on a full pipe; each retains at most the cap (codex's read_capped).
    let cap = TOOL_OUTPUT_MAX_BYTES;
    let out_pipe = child.stdout.take();
    let out_reader = std::thread::spawn(move || match out_pipe {
        Some(pipe) => read_capped(pipe, cap),
        None => (Vec::new(), false),
    });
    let err_pipe = child.stderr.take();
    let err_reader = std::thread::spawn(move || match err_pipe {
        Some(pipe) => read_capped(pipe, cap),
        None => (Vec::new(), false),
    });

    let start = Instant::now();
    let mut timed_out = false;
    let status = loop {
        if cancel.is_cancelled() {
            let _ = child.kill();
            let _ = child.wait();
            // The interrupt path owns the UI; return a terse outcome (the loop
            // discards it — the channel is already swapped). Detach the readers
            // (a reparented grandchild could hold the pipe open).
            return ToolOutcome::error("Interrupted by user");
        }
        if start.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            timed_out = true;
            break None;
        }
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => std::thread::sleep(BASH_POLL_INTERVAL),
            Err(err) => {
                let _ = out_reader.join();
                let _ = err_reader.join();
                return ToolOutcome::error(format!("error waiting on command: {err}"));
            }
        }
    };

    let (out_bytes, out_trunc) = out_reader.join().unwrap_or((Vec::new(), false));
    let (err_bytes, err_trunc) = err_reader.join().unwrap_or((Vec::new(), false));
    let mut combined = String::from_utf8_lossy(&out_bytes).into_owned();
    let stderr = String::from_utf8_lossy(&err_bytes);
    if !stderr.is_empty() {
        if !combined.is_empty() && !combined.ends_with('\n') {
            combined.push('\n');
        }
        combined.push_str(&stderr);
    }
    let (combined, extra_trunc) = tools::truncate_output(&combined, cap);
    let truncated = out_trunc || err_trunc || extra_trunc;

    if timed_out {
        let body = tools::format_exec_output(None, &combined);
        return ToolOutcome::error(format!(
            "command timed out after {} ms\n{body}",
            timeout.as_millis()
        ))
        .with_truncated(truncated);
    }
    let exit_code = status.as_ref().and_then(std::process::ExitStatus::code);
    let ok = status
        .as_ref()
        .is_some_and(std::process::ExitStatus::success);
    let output = tools::format_exec_output(exit_code, &combined);
    ToolOutcome {
        output,
        ok,
        truncated,
    }
}

/// `read`: read the file and return `cat -n`-style numbered lines, byte-capped.
fn run_read(arguments: &str) -> ToolOutcome {
    let args: ReadArgs = match tools::parse_args(arguments) {
        Ok(a) => a,
        Err(e) => return arg_error(e),
    };
    let path = Path::new(&args.path);
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(err) => return ToolOutcome::error(format!("could not read {}: {err}", args.path)),
    };
    let content = String::from_utf8_lossy(&bytes);
    if content.is_empty() {
        return ToolOutcome::ok(format!("(file {} is empty)", args.path));
    }
    let numbered = tools::format_read(&content, args.offset, args.limit);
    let (output, truncated) = tools::truncate_output(&numbered, TOOL_OUTPUT_MAX_BYTES);
    ToolOutcome::ok(output).with_truncated(truncated)
}

/// `write`: create parent dirs and write the file, reporting a diff vs the old
/// contents (or a `Created …` summary for a new file).
fn run_write(arguments: &str) -> ToolOutcome {
    let args: WriteArgs = match tools::parse_args(arguments) {
        Ok(a) => a,
        Err(e) => return arg_error(e),
    };
    let path = Path::new(&args.path);
    let existed = path.exists();
    let old = if existed {
        std::fs::read_to_string(path).unwrap_or_default()
    } else {
        String::new()
    };
    if let Err(err) = create_parents(path) {
        return ToolOutcome::error(format!("could not create parent directories: {err}"));
    }
    if let Err(err) = std::fs::write(path, &args.content) {
        return ToolOutcome::error(format!("could not write {}: {err}", args.path));
    }
    ToolOutcome::ok(describe_change(&args.path, &old, &args.content, !existed))
}

/// `edit`: exact-string replacement (the pure [`tools::apply_edit`]), written
/// back to disk, reported as a diff.
fn run_edit(arguments: &str) -> ToolOutcome {
    let args: EditArgs = match tools::parse_args(arguments) {
        Ok(a) => a,
        Err(e) => return arg_error(e),
    };
    let path = Path::new(&args.path);
    let old = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(err) => return ToolOutcome::error(format!("could not read {}: {err}", args.path)),
    };
    let result = match tools::apply_edit(&old, &args.old_string, &args.new_string, args.replace_all)
    {
        Ok(r) => r,
        Err(e) => return ToolOutcome::error(e.to_string()),
    };
    if let Err(err) = std::fs::write(path, &result.new_content) {
        return ToolOutcome::error(format!("could not write {}: {err}", args.path));
    }
    ToolOutcome::ok(describe_change(
        &args.path,
        &old,
        &result.new_content,
        false,
    ))
}

/// The model-facing summary of a `write`/`edit`: a `Created …`/`Updated …`
/// header with the `(+A −D)` counts, and — for a change to existing content —
/// the diff body (the `+`/`-` rows the TUI colours). A brand-new file just
/// reports its line count (no point echoing the content the model just wrote).
fn describe_change(path: &str, old: &str, new: &str, created: bool) -> String {
    let diff = tools::diff_lines(old, new);
    let summary = tools::diff_summary(diff.added, diff.removed);
    if created {
        let lines = new.lines().count();
        return format!(
            "Created {path} ({lines} line{}) {summary}",
            if lines == 1 { "" } else { "s" }
        );
    }
    if diff.added == 0 && diff.removed == 0 {
        return format!("No changes to {path}");
    }
    let body = tools::render_diff(&diff);
    format!("Updated {path} {summary}\n{body}")
}

/// Create the parent directories of `path`, if any (a bare filename has none).
fn create_parents(path: &Path) -> std::io::Result<()> {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => std::fs::create_dir_all(parent),
        _ => Ok(()),
    }
}

/// Read `reader` to EOF (so the child never blocks on a full pipe) but retain at
/// most `cap` bytes; return the retained head and whether anything was dropped.
/// Mirrors `main.rs::read_capped`.
fn read_capped(mut reader: impl Read, cap: usize) -> (Vec<u8>, bool) {
    let mut buf = Vec::new();
    let mut chunk = vec![0u8; 64 * 1024];
    let mut truncated = false;
    loop {
        match reader.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                if buf.len() < cap {
                    let take = (cap - buf.len()).min(n);
                    buf.extend_from_slice(&chunk[..take]);
                    if take < n {
                        truncated = true;
                    }
                } else {
                    truncated = true;
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
    (buf, truncated)
}

/// A unique temp path under the system temp dir for a test file.
#[cfg(test)]
fn temp_path(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("inline-tui-exec-test-{name}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(name: &str, args: &str) -> ToolCallRequest {
        ToolCallRequest {
            id: "c".to_string(),
            name: name.to_string(),
            arguments: args.to_string(),
        }
    }

    fn exec(name: &str, args: &str) -> ToolOutcome {
        RealToolExecutor::new().execute(&call(name, args), &CancelToken::new())
    }

    #[test]
    fn bash_runs_a_command_and_frames_the_exit_code() {
        let out = exec("bash", r#"{"command":"echo hello"}"#);
        assert!(out.ok);
        assert!(out.output.starts_with("Exit code: 0"), "got {}", out.output);
        assert!(out.output.contains("hello"));
    }

    #[test]
    fn bash_reports_a_nonzero_exit_as_a_failure() {
        let out = exec("bash", r#"{"command":"exit 3"}"#);
        assert!(!out.ok);
        assert!(out.output.contains("Exit code: 3"));
    }

    #[test]
    fn bash_merges_stderr_into_the_output() {
        let out = exec("bash", r#"{"command":"echo oops 1>&2"}"#);
        assert!(out.output.contains("oops"));
    }

    #[test]
    fn bash_times_out_a_slow_command() {
        let out = exec("bash", r#"{"command":"sleep 5","timeout_ms":150}"#);
        assert!(!out.ok);
        assert!(out.output.contains("timed out"), "got {}", out.output);
    }

    #[test]
    fn read_returns_numbered_lines() {
        let path = temp_path("read.txt");
        std::fs::write(&path, "alpha\nbeta\n").unwrap();
        let out = exec("read", &format!(r#"{{"path":"{}"}}"#, path.display()));
        std::fs::remove_file(&path).ok();
        assert!(out.ok);
        assert!(out.output.contains("     1\talpha"), "got {}", out.output);
        assert!(out.output.contains("     2\tbeta"));
    }

    #[test]
    fn read_of_a_missing_file_is_a_recoverable_error() {
        let out = exec("read", r#"{"path":"/definitely/not/here.txt"}"#);
        assert!(!out.ok);
        assert!(out.output.contains("could not read"));
    }

    #[test]
    fn write_creates_a_new_file_and_reports_it() {
        let path = temp_path("write-new.txt");
        std::fs::remove_file(&path).ok();
        let out = exec(
            "write",
            &format!(r#"{{"path":"{}","content":"one\ntwo\n"}}"#, path.display()),
        );
        let written = std::fs::read_to_string(&path).unwrap();
        std::fs::remove_file(&path).ok();
        assert!(out.ok);
        assert_eq!(written, "one\ntwo\n");
        assert!(out.output.starts_with("Created"), "got {}", out.output);
        assert!(out.output.contains("2 lines"));
    }

    #[test]
    fn write_over_existing_content_shows_a_diff() {
        let path = temp_path("write-over.txt");
        std::fs::write(&path, "keep\nold\n").unwrap();
        let out = exec(
            "write",
            &format!(r#"{{"path":"{}","content":"keep\nnew\n"}}"#, path.display()),
        );
        std::fs::remove_file(&path).ok();
        assert!(out.ok);
        assert!(out.output.starts_with("Updated"), "got {}", out.output);
        assert!(out.output.contains("-old"));
        assert!(out.output.contains("+new"));
    }

    #[test]
    fn edit_replaces_and_writes_back() {
        let path = temp_path("edit.txt");
        std::fs::write(&path, "let x = 1;\nlet y = 2;\n").unwrap();
        let out = exec(
            "edit",
            &format!(
                r#"{{"path":"{}","old_string":"let x = 1;","new_string":"let x = 42;"}}"#,
                path.display()
            ),
        );
        let after = std::fs::read_to_string(&path).unwrap();
        std::fs::remove_file(&path).ok();
        assert!(out.ok);
        assert_eq!(after, "let x = 42;\nlet y = 2;\n");
        assert!(out.output.contains("+let x = 42;"));
    }

    #[test]
    fn edit_reports_a_missing_match_without_writing() {
        let path = temp_path("edit-miss.txt");
        std::fs::write(&path, "abc\n").unwrap();
        let out = exec(
            "edit",
            &format!(
                r#"{{"path":"{}","old_string":"zzz","new_string":"y"}}"#,
                path.display()
            ),
        );
        let after = std::fs::read_to_string(&path).unwrap();
        std::fs::remove_file(&path).ok();
        assert!(!out.ok);
        assert_eq!(after, "abc\n", "the file is untouched on a failed edit");
        assert!(out.output.contains("not found"));
    }

    #[test]
    fn an_unknown_tool_is_a_recoverable_error() {
        let out = exec("teleport", "{}");
        assert!(!out.ok);
        assert!(out.output.contains("unknown tool"));
    }

    #[test]
    fn bad_arguments_are_a_recoverable_error() {
        let out = exec("bash", "not json");
        assert!(!out.ok);
        assert!(out.output.contains("invalid tool arguments"));
    }
}
