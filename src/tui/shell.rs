//! The `!` shell runner: one local command run as a turn.
//!
//! [`spawn_shell_command`] is the whole module's point — a background thread
//! that runs `sh -c {command}` and reports on the **reply channel**, as a
//! `ToolEnd`/`StreamDone` pair. Riding the same channel a model reply uses is
//! what makes the status strip, the Esc interrupt, the resize repaint and the
//! Ctrl+B hand-off work on a `!` command with no special cases anywhere else
//! (`docs/shell-command.md`).
//!
//! Three rules the runner obeys, each learned from a bug:
//!
//! - **Never block on a pipe.** Both pipes drain on their own threads
//!   ([`drain_shell_pipe`]) so a chatty command can't fill a buffer and hang
//!   forever instead of exiting.
//! - **Bound the memory.** What the wait loop *keeps* is capped as it arrives
//!   ([`append_capped`]), so `! tree ~/` can't spike RSS — the dropped tail is
//!   gone, and the cell says so with a `…`.
//! - **Kill the group, not the child.** A command that backgrounded something
//!   holds the pipes open, so every exit path goes through
//!   [`kill_shell_group`].
//!
//! This is I/O boundary code, verified by `scripts/smoke.sh` (Phase 19) — with
//! one exception: the drain/cap pair at the bottom is reader-generic and pure
//! enough to unit-test, and it is (see the tests module).

use std::io;
use std::thread::JoinHandle;
use std::time::Duration;

use alter_zero::background::BackgroundRegistry;
use alter_zero::stream::{CancelToken, StreamEvent};

/// Run `command` under `sh -c` on a background thread, streaming the result back
/// on the reply channel as a `ToolEnd`/`StreamDone` pair (the command was
/// already shown as the running tool by [`App::begin_shell`](alter_zero::app::App::begin_shell)). Reader threads
/// drain stdout/stderr so a chatty command can't deadlock on a full pipe,
/// forwarding raw chunks the wait loop merges **in arrival order** into a
/// capped buffer (`read_capped`'s memory bound, the `llm::exec` merge shape);
/// the loop polls `cancel` so an Esc interrupt kills the child and reaps it,
/// and the registry's Ctrl+B latch so a running command can be **adopted into
/// the background** mid-run — resolving as `ToolBackgrounded` instead
/// (docs/background.md). The child leads its own process group (like the
/// model-bash executor), so kills reap backgrounded grandchildren too. A
/// non-zero exit appends `[exit status: N]` and resolves the cell red. The
/// I/O boundary — verified by `scripts/smoke.sh` (Phase 19), not unit tests.
/// See `docs/shell-command.md`.
pub(crate) fn spawn_shell_command(
    command: String,
    tx: tokio::sync::mpsc::UnboundedSender<StreamEvent>,
    cancel: CancelToken,
    registry: BackgroundRegistry,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
        // A Ctrl+B pressed before this command started belongs to nothing.
        registry.clear_background_request();
        // Group + terminal membership (and stdio) come from
        // `subprocess::spawn_detached_shell`: its own process group so a kill
        // (Esc, quit, the registry) reaps the whole tree, and no controlling
        // terminal, so a password prompt (`! sudo …`) fails fast instead of
        // writing over the TUI (the `llm::exec` pattern; see
        // `subprocess`, docs/shell-command.md).
        let mut child = match alter_zero::subprocess::spawn_detached_shell(
            registry.detach_helper().as_deref(),
            &command,
        ) {
            Ok(child) => child,
            Err(err) => {
                let _ = tx.send(StreamEvent::ToolEnd {
                    output: format!("failed to run command: {err}"),
                    ok: false,
                    truncated: false,
                });
                let _ = tx.send(StreamEvent::StreamDone);
                return;
            }
        };

        // Drain both pipes on their own threads so a command that writes more
        // than the pipe buffer can't block (and thus never exit) while we
        // wait; the loop below *caps* what it retains so a command with huge
        // output (e.g. `tree ~/`) can't spike memory.
        let cap = SHELL_OUTPUT_MAX_BYTES;
        let (chunk_tx, chunk_rx) = std::sync::mpsc::channel::<Vec<u8>>();
        if let Some(pipe) = child.stdout.take() {
            let tx = chunk_tx.clone();
            std::thread::spawn(move || drain_shell_pipe(pipe, &tx));
        }
        if let Some(pipe) = child.stderr.take() {
            let tx = chunk_tx.clone();
            std::thread::spawn(move || drain_shell_pipe(pipe, &tx));
        }
        drop(chunk_tx);

        let mut combined: Vec<u8> = Vec::new();
        let mut truncated = false;
        // Wait for the child, polling so an Esc interrupt (cancel) or a Ctrl+B
        // background request acts promptly.
        let status = loop {
            while let Ok(chunk) = chunk_rx.try_recv() {
                append_capped(&mut combined, &chunk, cap, &mut truncated);
            }
            if cancel.is_cancelled() {
                // The interrupt path (App::interrupt_turn) owns the UI from
                // here — resolve the tool failed, commit the notice. Kill the
                // whole group (a reparented grandchild would otherwise hold
                // the pipe open), send nothing, and return at once so the
                // quit path's bounded wait unblocks promptly. The detached
                // readers finish at EOF on their own.
                kill_shell_group(&mut child);
                return;
            }
            // Ctrl+B: hand the run to the background registry mid-flight —
            // it replays what we read so far and keeps streaming from our
            // pipe channel. The cell resolves as backgrounded; the shell turn
            // ends with no summary as usual (docs/background.md).
            if registry.take_background_request() {
                let task = registry.adopt(&command, None, false, child, chunk_rx, combined);
                let _ = tx.send(StreamEvent::ToolBackgrounded {
                    id: task.id.clone(),
                    output: "[moved to background; the final output will follow when it completes]"
                        .to_string(),
                });
                let _ = tx.send(StreamEvent::StreamDone);
                return;
            }
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => match chunk_rx.recv_timeout(SHELL_POLL_INTERVAL) {
                    Ok(chunk) => append_capped(&mut combined, &chunk, cap, &mut truncated),
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                        std::thread::sleep(SHELL_POLL_INTERVAL);
                    }
                },
                Err(err) => {
                    kill_shell_group(&mut child);
                    let _ = tx.send(StreamEvent::ToolEnd {
                        output: format!("error waiting on command: {err}"),
                        ok: false,
                        truncated: false,
                    });
                    let _ = tx.send(StreamEvent::StreamDone);
                    return;
                }
            }
        };
        // Even on a clean exit, reap any process the command backgrounded —
        // it holds the pipes open, which used to delay the cell until the
        // straggler died; then absorb whatever the readers still buffered
        // (they hit EOF once the group is gone).
        kill_shell_group(&mut child);
        while let Ok(chunk) = chunk_rx.recv_timeout(SHELL_POLL_INTERVAL) {
            append_capped(&mut combined, &chunk, cap, &mut truncated);
        }

        // Lossy UTF-8: a command may emit non-UTF-8 bytes; the cap may also
        // cut a multi-byte char (→ one U+FFFD).
        let mut output = String::from_utf8_lossy(&combined).into_owned();
        let ok = status.success();
        if !ok {
            let code = status
                .code()
                .map_or_else(|| "signal".to_string(), |c| c.to_string());
            if !output.is_empty() && !output.ends_with('\n') {
                output.push('\n');
            }
            output.push_str(&format!("[exit status: {code}]"));
        }
        let _ = tx.send(StreamEvent::ToolEnd {
            output,
            ok,
            truncated,
        });
        let _ = tx.send(StreamEvent::StreamDone);
    })
}

/// Read `pipe` to EOF, forwarding raw chunks for the wait loop to merge (the
/// `llm::exec` drain shape). Stops early if the receiver hung up.
fn drain_shell_pipe(mut pipe: impl io::Read, tx: &std::sync::mpsc::Sender<Vec<u8>>) {
    let mut chunk = vec![0u8; 64 * 1024];
    loop {
        match pipe.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                if tx.send(chunk[..n].to_vec()).is_err() {
                    break;
                }
            }
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
}

/// Append `chunk` to the capped `buf`, marking `truncated` when the cap bites —
/// bounds peak memory regardless of how much a command emits (codex's
/// `append_capped` pattern; see `docs/shell-command.md`).
fn append_capped(buf: &mut Vec<u8>, chunk: &[u8], cap: usize, truncated: &mut bool) {
    if buf.len() < cap {
        let take = (cap - buf.len()).min(chunk.len());
        buf.extend_from_slice(&chunk[..take]);
        if take < chunk.len() {
            *truncated = true;
        }
    } else if !chunk.is_empty() {
        *truncated = true;
    }
}

/// Kill a `!` command's whole process group and reap the direct child — the
/// `llm::exec::kill_process_group` pattern (the crate forbids `unsafe`, so the
/// shell's POSIX `kill` handles the negative-pgid form). Best-effort.
fn kill_shell_group(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        let pgid = child.id();
        let _ = std::process::Command::new("sh")
            .arg("-c")
            .arg(format!("kill -KILL -{pgid} 2>/dev/null"))
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// How often [`spawn_shell_command`] polls a running child for completion while
/// watching for an interrupt — short enough that Esc kills it promptly.
pub(crate) const SHELL_POLL_INTERVAL: Duration = Duration::from_millis(20);

/// How long the quit path waits for the shell runner to observe the cancel
/// and kill/reap its child before the process exits. Bounded — a wedged kill
/// can't stall the quit — and comfortably above [`SHELL_POLL_INTERVAL`] plus
/// the kill/wait syscalls.
pub(crate) const SHELL_QUIT_KILL_WINDOW: Duration = Duration::from_millis(250);

/// A `!` shell command retains at most this many bytes of output in memory; the
/// rest is drained and dropped (the cell appends a `…` marker). This caps peak
/// memory so a command with huge output (`tree ~/`) can't spike RSS — the
/// previous "save the full output to a file" approach still read everything into
/// memory first, which is what we're avoiding (codex caps in memory too). See
/// `docs/shell-command.md`.
const SHELL_OUTPUT_MAX_BYTES: usize = 100_000;

#[cfg(test)]
mod tests {
    use std::io;

    use super::{append_capped, drain_shell_pipe};

    /// Serves its chunks one per `read` call (each far smaller than the
    /// drain's 64 KiB buffer, so every chunk arrives whole), then EOF —
    /// letting a test control exactly how the input splits across reads.
    struct ChunkedReader {
        chunks: Vec<Vec<u8>>,
        served: usize,
    }

    impl ChunkedReader {
        fn new(chunks: &[&[u8]]) -> Self {
            Self {
                chunks: chunks.iter().map(|c| c.to_vec()).collect(),
                served: 0,
            }
        }
    }

    impl io::Read for ChunkedReader {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let Some(chunk) = self.chunks.get(self.served) else {
                return Ok(0); // past the last chunk: EOF
            };
            self.served += 1;
            let n = chunk.len().min(buf.len());
            buf[..n].copy_from_slice(&chunk[..n]);
            Ok(n)
        }
    }

    /// Fails with `ErrorKind::Interrupted` on the first read (a signal landed
    /// mid-`read`), serves its data on the second, then EOF.
    struct InterruptedOnce {
        data: Vec<u8>,
        calls: usize,
    }

    impl io::Read for InterruptedOnce {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.calls += 1;
            match self.calls {
                1 => Err(io::Error::from(io::ErrorKind::Interrupted)),
                2 => {
                    let n = self.data.len().min(buf.len());
                    buf[..n].copy_from_slice(&self.data[..n]);
                    Ok(n)
                }
                _ => Ok(0),
            }
        }
    }

    /// Run `reader` through the drain + cap pair the runner's wait loop uses.
    fn drain_capped(reader: impl io::Read, cap: usize) -> (Vec<u8>, bool) {
        let (tx, rx) = std::sync::mpsc::channel();
        drain_shell_pipe(reader, &tx);
        drop(tx);
        let mut buf = Vec::new();
        let mut truncated = false;
        while let Ok(chunk) = rx.try_recv() {
            append_capped(&mut buf, &chunk, cap, &mut truncated);
        }
        (buf, truncated)
    }

    #[test]
    fn empty_input_reads_nothing_and_is_not_truncated() {
        let (out, truncated) = drain_capped(io::Cursor::new(Vec::<u8>::new()), 10);
        assert!(out.is_empty());
        assert!(!truncated);
    }

    #[test]
    fn output_landing_exactly_at_the_cap_is_not_truncated() {
        // Nothing was dropped, so the cell must not gain a `…` marker.
        let (out, truncated) = drain_capped(io::Cursor::new(vec![b'a'; 10]), 10);
        assert_eq!(out, vec![b'a'; 10]);
        assert!(!truncated);
    }

    #[test]
    fn one_byte_over_the_cap_truncates_to_exactly_the_cap() {
        let (out, truncated) = drain_capped(io::Cursor::new(vec![b'a'; 11]), 10);
        assert_eq!(out.len(), 10);
        assert!(truncated);
    }

    #[test]
    fn a_mid_chunk_cut_keeps_the_head_and_marks_truncation() {
        // One read serves 8 bytes but only 5 fit under the cap: the head is
        // retained byte-for-byte and the cut inside the chunk flags truncated.
        let (out, truncated) = drain_capped(io::Cursor::new(b"abcdefgh".to_vec()), 5);
        assert_eq!(out, b"abcde");
        assert!(truncated);
    }

    #[test]
    fn chunks_past_the_cap_are_drained_but_retain_nothing() {
        // The first chunk lands exactly at the cap (no mid-chunk cut), so only
        // the keep-draining loop can flag the later chunks as dropped.
        let mut reader = ChunkedReader::new(&[b"abcd", b"efgh", b"ijkl"]);
        let (out, truncated) = drain_capped(&mut reader, 4);
        assert_eq!(out, b"abcd");
        assert!(truncated);
        // … and the reader really was drained to EOF (so the child can't block
        // on a full pipe), not abandoned once the cap filled.
        assert_eq!(reader.served, 3);
    }

    #[test]
    fn input_larger_than_the_read_buffer_drains_across_reads() {
        // Bigger than the drain's 64 KiB chunk, so it spans several real
        // reads; only the first `cap` bytes are retained.
        let (out, truncated) = drain_capped(io::Cursor::new(vec![b'x'; 200_000]), 100);
        assert_eq!(out, vec![b'x'; 100]);
        assert!(truncated);
    }

    #[test]
    fn an_interrupted_read_is_retried_not_treated_as_eof() {
        let reader = InterruptedOnce {
            data: b"abc".to_vec(),
            calls: 0,
        };
        let (out, truncated) = drain_capped(reader, 10);
        assert_eq!(out, b"abc");
        assert!(!truncated);
    }
}
