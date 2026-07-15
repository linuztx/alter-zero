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
use std::sync::mpsc;
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
    ///
    /// `on_output` is a sink for the tool's **live output**: a streaming tool
    /// (`bash`) calls it with each newly-completed line as the command runs, so
    /// the TUI can tail the running cell (surfaced as
    /// [`crate::stream::StreamEvent::ToolOutput`]; see `docs/tool-streaming.md`).
    /// A tool that produces its result all at once (`read`/`write`/`edit`)
    /// simply never calls it — the full result still comes back in the returned
    /// [`ToolOutcome`].
    fn execute(
        &self,
        call: &ToolCallRequest,
        cancel: &CancelToken,
        on_output: &mut dyn FnMut(&str),
    ) -> ToolOutcome;
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
    fn execute(
        &self,
        call: &ToolCallRequest,
        cancel: &CancelToken,
        on_output: &mut dyn FnMut(&str),
    ) -> ToolOutcome {
        match call.name.as_str() {
            "bash" => run_bash(&call.arguments, cancel, on_output),
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
/// (byte-capped, merged in arrival order), enforce the per-call timeout, and
/// kill on cancel. As output arrives it is **streamed line-by-line** through
/// `on_output` so the TUI tails the running cell (`docs/tool-streaming.md`); the
/// full output is still framed as codex does (`Exit code: N` + output) for the
/// model — a non-zero exit or a timeout resolves the cell red.
fn run_bash(arguments: &str, cancel: &CancelToken, on_output: &mut dyn FnMut(&str)) -> ToolOutcome {
    let args: BashArgs = match tools::parse_args(arguments) {
        Ok(a) => a,
        Err(e) => return arg_error(e),
    };
    let timeout = Duration::from_millis(args.timeout_ms());

    let mut command = Command::new("sh");
    command
        .arg("-c")
        .arg(&args.command)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Put `sh` in its own process group (its pgid == its pid) so we can kill the
    // **whole tree** — a command that forks or backgrounds a child (`sleep 5 &`,
    // launching a server) leaves a grandchild holding the stdout/stderr pipe;
    // killing `sh` alone would orphan it, so the reader thread never sees EOF and
    // a join would hang, defeating the timeout. Killing the group reaps them all.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(err) => return ToolOutcome::error(format!("failed to run command: {err}")),
    };

    // Drain both pipes on their own threads (so a chatty command can't deadlock
    // on a full pipe), forwarding raw chunks over a channel; the poll loop below
    // merges them into the capped `combined` buffer in arrival order and streams
    // each newly-completed line out via `on_output`.
    let cap = TOOL_OUTPUT_MAX_BYTES;
    let (chunk_tx, chunk_rx) = mpsc::channel::<Vec<u8>>();
    let out_pipe = child.stdout.take();
    let out_tx = chunk_tx.clone();
    let out_reader = std::thread::spawn(move || {
        if let Some(pipe) = out_pipe {
            drain_pipe(pipe, &out_tx);
        }
    });
    let err_pipe = child.stderr.take();
    let err_tx = chunk_tx.clone();
    let err_reader = std::thread::spawn(move || {
        if let Some(pipe) = err_pipe {
            drain_pipe(pipe, &err_tx);
        }
    });
    drop(chunk_tx); // only the readers hold senders now, so the channel ends at EOF

    let mut combined: Vec<u8> = Vec::new();
    let mut forwarded = 0usize; // bytes already streamed out via on_output
    let mut truncated = false;
    let start = Instant::now();
    let mut timed_out = false;
    let status = loop {
        // Absorb whatever output is available right now, streaming its lines.
        while let Ok(chunk) = chunk_rx.try_recv() {
            absorb_chunk(
                &chunk,
                &mut combined,
                &mut forwarded,
                cap,
                &mut truncated,
                on_output,
            );
        }
        if cancel.is_cancelled() {
            kill_process_group(&mut child);
            // The interrupt path owns the UI; return a terse outcome (the loop
            // discards it — the channel is already swapped). The group kill has
            // reaped any straggler, so the detached readers finish on their own.
            return ToolOutcome::error("Interrupted by user");
        }
        if start.elapsed() >= timeout {
            kill_process_group(&mut child);
            timed_out = true;
            break None;
        }
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            // Wait briefly for the next chunk (so we tail promptly) or wake to
            // re-poll the cancel/timeout above; `Disconnected` (both readers done
            // before the child is reaped) just paces the re-poll.
            Ok(None) => match chunk_rx.recv_timeout(BASH_POLL_INTERVAL) {
                Ok(chunk) => {
                    absorb_chunk(
                        &chunk,
                        &mut combined,
                        &mut forwarded,
                        cap,
                        &mut truncated,
                        on_output,
                    );
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    std::thread::sleep(BASH_POLL_INTERVAL);
                }
            },
            Err(err) => {
                kill_process_group(&mut child);
                return ToolOutcome::error(format!("error waiting on command: {err}"));
            }
        }
    };
    // Even on a clean exit, reap any process the command backgrounded — it holds
    // the pipe open, so joining the readers below would otherwise block on it.
    kill_process_group(&mut child);
    let _ = out_reader.join();
    let _ = err_reader.join();
    // Drain any output buffered after the last poll, then stream the trailing
    // partial line (never newline-terminated) so `on_output` has seen it all.
    while let Ok(chunk) = chunk_rx.try_recv() {
        absorb_chunk(
            &chunk,
            &mut combined,
            &mut forwarded,
            cap,
            &mut truncated,
            on_output,
        );
    }
    if forwarded < combined.len() {
        on_output(&String::from_utf8_lossy(&combined[forwarded..]));
    }

    let combined = String::from_utf8_lossy(&combined).into_owned();
    // `combined` is already byte-capped; truncate_output only trims to a clean
    // char boundary (and re-confirms the flag) so the framed body is valid UTF-8.
    let (combined, extra_trunc) = tools::truncate_output(&combined, cap);
    let truncated = truncated || extra_trunc;

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

/// Read `pipe` to EOF in chunks, forwarding each raw chunk over `tx` so the
/// caller can merge and stream it. Draining to EOF (rather than stopping at a
/// cap) keeps a chatty command from blocking on a full pipe; the caller bounds
/// what it *retains*. Stops early if the receiver has hung up.
fn drain_pipe(mut pipe: impl Read, tx: &mpsc::Sender<Vec<u8>>) {
    let mut chunk = vec![0u8; 64 * 1024];
    loop {
        match pipe.read(&mut chunk) {
            Ok(0) => break, // EOF
            Ok(n) => {
                if tx.send(chunk[..n].to_vec()).is_err() {
                    break; // receiver gone — stop draining
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
}

/// Append `chunk` to the capped `combined` buffer (in arrival order) and stream
/// every newly-**completed** line — `combined[forwarded ..= last '\n']` — out
/// through `on_output`, advancing `forwarded`. Retaining and forwarding are both
/// bounded by `cap` (setting `truncated` when it bites). Forwarding whole lines
/// from the merged buffer means a multi-byte UTF-8 char is never split across a
/// chunk boundary (a `'\n'` is never inside a char), so the tail never shows a
/// stray replacement glyph.
fn absorb_chunk(
    chunk: &[u8],
    combined: &mut Vec<u8>,
    forwarded: &mut usize,
    cap: usize,
    truncated: &mut bool,
    on_output: &mut dyn FnMut(&str),
) {
    if combined.len() < cap {
        let take = (cap - combined.len()).min(chunk.len());
        combined.extend_from_slice(&chunk[..take]);
        if take < chunk.len() {
            *truncated = true;
        }
    } else if !chunk.is_empty() {
        *truncated = true;
    }
    if let Some(rel) = combined[*forwarded..].iter().rposition(|&b| b == b'\n') {
        let upto = *forwarded + rel + 1;
        on_output(&String::from_utf8_lossy(&combined[*forwarded..upto]));
        *forwarded = upto;
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

/// The model-facing result of a `write`/`edit` — also exactly what the cell
/// shows (the TUI restyles the rows; see `docs/tools.md`). A brand-new file is
/// a `Created {path} ({N} lines)` head over the numbered contents
/// ([`tools::render_numbered_content`]); a change to existing content is an
/// `Updated {path} (+A -D)` head over the numbered diff hunks
/// ([`tools::render_numbered_diff`]). The numbers match the `read` tool's, so
/// the model can cite them in a follow-up `edit`.
fn describe_change(path: &str, old: &str, new: &str, created: bool) -> String {
    let diff = tools::diff_lines(old, new);
    if created {
        let lines = new.lines().count();
        let head = format!(
            "Created {path} ({lines} line{})",
            if lines == 1 { "" } else { "s" }
        );
        let body = tools::render_numbered_content(new);
        if body.is_empty() {
            return head;
        }
        return format!("{head}\n{body}");
    }
    if diff.added == 0 && diff.removed == 0 {
        return format!("No changes to {path}");
    }
    let summary = tools::diff_summary(diff.added, diff.removed);
    let body = tools::render_numbered_diff(&diff);
    format!("Updated {path} {summary}\n{body}")
}

/// Create the parent directories of `path`, if any (a bare filename has none).
fn create_parents(path: &Path) -> std::io::Result<()> {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => std::fs::create_dir_all(parent),
        _ => Ok(()),
    }
}

/// Kill `child`'s entire process group and reap it. Because `run_bash` spawns
/// `sh` with `process_group(0)`, the child leads its own group (pgid == pid), so
/// a **negative pid** targets the whole tree — reaping any process the command
/// forked or backgrounded, which would otherwise keep the stdout/stderr pipe
/// open and hang a reader-thread join (defeating the timeout). Best-effort;
/// errors are ignored.
///
/// This crate `forbid`s `unsafe`, so it can't call `libc::kill(-pid, …)`
/// directly; instead it uses the shell's POSIX `kill` builtin, which treats a
/// negative operand as a process group (`sh -c "kill -KILL -<pgid>"`). The
/// helper `sh` starts in its own group, so it never signals itself.
#[cfg(unix)]
fn kill_process_group(child: &mut std::process::Child) {
    let pgid = child.id();
    let _ = Command::new("sh")
        .arg("-c")
        .arg(format!("kill -KILL -{pgid} 2>/dev/null"))
        .status();
    let _ = child.kill(); // reap the direct child too (no-op if already gone)
    let _ = child.wait();
}

/// Non-unix fallback: no process groups — just kill and reap the direct child.
#[cfg(not(unix))]
fn kill_process_group(child: &mut std::process::Child) {
    let _ = child.kill();
    let _ = child.wait();
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
        RealToolExecutor::new().execute(&call(name, args), &CancelToken::new(), &mut |_| {})
    }

    #[test]
    fn bash_streams_its_output_to_the_sink() {
        // The live-output sink receives the command's output as it runs; by the
        // time execute returns it has seen all of it, and the framed final still
        // carries the same body (docs/tool-streaming.md).
        let mut streamed = String::new();
        let out = RealToolExecutor::new().execute(
            &call("bash", r#"{"command":"printf 'a\nb\nc\n'"}"#),
            &CancelToken::new(),
            &mut |chunk| streamed.push_str(chunk),
        );
        assert!(out.ok, "got {}", out.output);
        assert_eq!(
            streamed, "a\nb\nc\n",
            "the sink tails the full output as complete lines"
        );
        assert!(
            out.output.contains("a\nb\nc"),
            "the framed final still carries the body: {}",
            out.output
        );
    }

    #[test]
    fn bash_streams_a_trailing_line_without_a_newline() {
        // A final line with no trailing '\n' is still flushed to the sink once,
        // at the end (before the ToolEnd overwrite), so the sink sees everything.
        let mut streamed = String::new();
        let out = RealToolExecutor::new().execute(
            &call("bash", r#"{"command":"printf 'x\ny'"}"#),
            &CancelToken::new(),
            &mut |chunk| streamed.push_str(chunk),
        );
        assert!(out.ok, "got {}", out.output);
        assert_eq!(streamed, "x\ny", "the partial trailing line is flushed too");
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
    fn bash_does_not_hang_on_a_backgrounded_grandchild() {
        // `sleep 10 &` makes `sh` fork `sleep` into the background and exit 0 at
        // once. `sleep` inherits the stdout/stderr pipe, so read_capped never
        // sees EOF — joining the reader would block ~10s (the timeout defeated).
        // Killing the whole process group reaps the straggler, so the call
        // returns promptly. Assert it finishes well under the 10s straggler.
        let start = std::time::Instant::now();
        let out = exec("bash", r#"{"command":"sleep 10 &","timeout_ms":30000}"#);
        let elapsed = start.elapsed();
        assert!(
            elapsed < std::time::Duration::from_secs(5),
            "run_bash returned in {elapsed:?} — a backgrounded grandchild made it hang"
        );
        assert!(out.ok, "the command itself exits 0");
    }

    #[test]
    fn bash_timeout_is_honoured_even_when_a_child_survives() {
        // A foreground long child that `sh` forked (not exec'd): on timeout,
        // killing `sh` alone would orphan the child holding the pipe and the
        // join would wait it out. The group kill must make the timeout prompt.
        let start = std::time::Instant::now();
        let out = exec(
            "bash",
            r#"{"command":"(sleep 10 & wait)","timeout_ms":300}"#,
        );
        let elapsed = start.elapsed();
        assert!(!out.ok);
        assert!(out.output.contains("timed out"), "got {}", out.output);
        assert!(
            elapsed < std::time::Duration::from_secs(5),
            "the timeout took {elapsed:?} — a surviving child defeated it"
        );
    }

    #[test]
    fn read_returns_numbered_lines() {
        let path = temp_path("read.txt");
        std::fs::write(&path, "alpha\nbeta\n").unwrap();
        let out = exec("read", &format!(r#"{{"path":"{}"}}"#, path.display()));
        std::fs::remove_file(&path).ok();
        assert!(out.ok);
        assert!(out.output.contains("1 alpha"), "got {}", out.output);
        assert!(out.output.contains("2 beta"));
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
        // The body echoes the new file as numbered lines (the TUI's preview
        // and the model's reference for follow-up edits).
        assert!(out.output.contains("1 one"), "got {}", out.output);
        assert!(out.output.contains("2 two"));
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
        // The diff body carries line numbers (codex's numbered hunks).
        assert!(out.output.contains("2 -old"), "got {}", out.output);
        assert!(out.output.contains("2 +new"));
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
        assert!(out.output.contains("1 +let x = 42;"), "got {}", out.output);
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
