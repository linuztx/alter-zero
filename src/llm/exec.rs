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
use std::sync::mpsc;
use std::time::{Duration, Instant};

use super::tools::{
    self, BashArgs, EditArgs, ReadArgs, TOOL_OUTPUT_MAX_BYTES, ToolCallRequest, ToolOutcome,
    WriteArgs,
};
use crate::background::{BackgroundRegistry, BgOrigin};
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
/// An attached [`BackgroundRegistry`] enables `run_in_background` and the
/// Ctrl+B handoff for `bash` (see `docs/background.md`); without one those
/// resolve as recoverable errors / are unavailable.
#[derive(Debug, Clone, Default)]
pub struct RealToolExecutor {
    background: Option<BackgroundRegistry>,
    /// The launching **subagent**, when this executor runs one's tool calls
    /// (`docs/agent-tool.md`): its `run_in_background` launches carry this
    /// origin into the shared shell list, and the Ctrl+B latch is left alone
    /// (the handoff belongs to the main turn's foreground command — a
    /// subagent's concurrent `bash` must never steal it).
    bg_origin: Option<BgOrigin>,
    /// The terminal-detach helper (`crate::subprocess`): the TUI's own binary,
    /// available as the re-exec fallback tier when the `setsid` binary is
    /// absent (macOS, minimal images) — so `bash` children run with **no
    /// controlling terminal** and a `/dev/tty` password prompt (`sudo`) fails
    /// fast instead of hijacking the TUI. `None` (tests, embedders without
    /// the hook) just shortens the tier chain.
    detach_helper: Option<std::path::PathBuf>,
    /// The active model's image-input support (`ModelConfig::vision`):
    /// `Some(false)` makes an image `read` fail as a recoverable error
    /// instead of attaching pixels the provider would reject with the whole
    /// turn (`docs/tools.md`). `None`/`Some(true)` attach as normal.
    vision: Option<bool>,
}

impl RealToolExecutor {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Attach the shared background registry (the production path — `main.rs`
    /// threads it through the backend). See `docs/background.md`.
    #[must_use]
    pub fn with_background(mut self, registry: BackgroundRegistry) -> Self {
        self.background = Some(registry);
        self
    }

    /// Attribute this executor's background launches to a **subagent**
    /// (`docs/agent-tool.md`): its `run_in_background` shells join the same
    /// shared list — stacking into the footer count — tagged with the
    /// launcher, while the Ctrl+B handoff latch stays untouched (it belongs
    /// to the main turn's foreground command).
    #[must_use]
    pub fn with_background_origin(mut self, origin: BgOrigin) -> Self {
        self.bg_origin = Some(origin);
        self
    }

    /// Route `bash` children through the terminal-detach helper (production —
    /// `main.rs` resolves its own binary once at startup and the backend
    /// copies it off the registry). See `crate::subprocess`.
    #[must_use]
    pub fn with_detach_helper(mut self, helper: Option<std::path::PathBuf>) -> Self {
        self.detach_helper = helper;
        self
    }

    /// Tell the executor the active model's image-input support (the backend
    /// copies it off its `ModelConfig`). See `docs/tools.md`.
    #[must_use]
    pub fn with_vision(mut self, vision: Option<bool>) -> Self {
        self.vision = vision;
        self
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
            "bash" => run_bash(
                &call.arguments,
                cancel,
                self.background.as_ref(),
                self.bg_origin.as_ref(),
                self.detach_helper.as_deref(),
                on_output,
            ),
            "read" => run_read(&call.arguments, self.vision),
            "write" => run_write(&call.arguments),
            "edit" => run_edit(&call.arguments),
            other => ToolOutcome::error(format!("unknown tool: {other}")),
        }
    }
}

/// The model-facing result of a backgrounded `bash` call — what the tool
/// result says (the cell shows the fixed backgrounded row instead): the
/// interim-output file to `read` mid-run and the promise of the final
/// output. No task id — nothing model-facing takes one back (kills go by
/// PID, progress by the file), so naming it would just ask the model to
/// track a token with no use. See `docs/background.md`.
#[must_use]
pub fn background_launch_text(task: &crate::background::LaunchedTask) -> String {
    format!(
        "Command running in the background. Output is streaming to {} — \
         read that file to check progress.\n\
         You will be re-invoked with the final output when it exits.",
        task.output_path.display(),
    )
}

/// The model-facing result of a **Ctrl+B handoff** — the *user* moved a
/// foreground `bash` call to the background mid-run. Unlike
/// [`background_launch_text`] (a `run_in_background` launch the model asked
/// for), the model here requested a foreground run and expects the full
/// output in this result — so the text must say who moved it and steer the
/// model off waiting, or it treats the acknowledgement as an anomaly and
/// re-reads the interim file round after round. The launch facts (interim
/// path, completion promise) are embedded verbatim so the two variants can
/// never drift. See `docs/background.md`.
#[must_use]
pub fn background_handoff_text(task: &crate::background::LaunchedTask) -> String {
    format!(
        "The user moved this command to the background while it was running — \
         it has not failed and keeps running there, so its remaining output \
         will not arrive in this result.\n\
         {}\n\
         Do not run the command again, and do not wait for it by repeatedly \
         reading the interim file — continue with the rest of the task, or \
         end the turn.",
        background_launch_text(task),
    )
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
fn run_bash(
    arguments: &str,
    cancel: &CancelToken,
    background: Option<&BackgroundRegistry>,
    origin: Option<&BgOrigin>,
    detach_helper: Option<&Path>,
    on_output: &mut dyn FnMut(&str),
) -> ToolOutcome {
    let args: BashArgs = match tools::parse_args(arguments) {
        Ok(a) => a,
        Err(e) => return arg_error(e),
    };
    // A Ctrl+B pressed before this command started belongs to nothing — drop
    // it so it can't instantly background us (docs/background.md). The latch
    // is the MAIN turn's alone: a subagent's executor (origin set) leaves it
    // for the foreground command it actually belongs to.
    if let Some(registry) = background
        && origin.is_none()
    {
        registry.clear_background_request();
    }
    // `run_in_background`: hand the whole run to the registry and return the
    // launch text at once — the model gets the interim-output path, and the
    // completion notification follows when the command exits. A subagent's
    // launch is attributed via `origin` (docs/agent-tool.md).
    if args.run_in_background {
        let Some(registry) = background else {
            return ToolOutcome::error(
                "run_in_background is not available here — run the command in the foreground",
            );
        };
        return match registry.launch_from(
            &args.command,
            args.description.clone(),
            true,
            origin.cloned(),
        ) {
            Ok(task) => ToolOutcome::backgrounded(task.id.clone(), background_launch_text(&task)),
            Err(err) => ToolOutcome::error(err),
        };
    }
    let timeout = Duration::from_millis(args.timeout_ms());

    // Group + terminal membership come from `subprocess::spawn_detached_shell`
    // (stdio too — stdin null, stdout/stderr piped): the child leads its own
    // process group (pgid == pid — a command that forks or backgrounds a
    // grandchild leaves it holding the stdout/stderr pipe, and only a
    // **group** kill reaps the whole tree, so the reader-thread joins can't
    // hang) and has **no controlling terminal** — a `/dev/tty` password
    // prompt (`sudo`) errors at once instead of hijacking the TUI (see
    // `crate::subprocess`, docs/tools.md).
    let mut child = match crate::subprocess::spawn_detached_shell(detach_helper, &args.command) {
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
            crate::subprocess::kill_process_group(&mut child);
            // The interrupt path owns the UI; return a terse outcome (the loop
            // discards it — the channel is already swapped). The group kill has
            // reaped any straggler, so the detached readers finish on their own.
            return ToolOutcome::error("Interrupted by user");
        }
        // Ctrl+B: hand the run off to the background registry mid-flight — it
        // replays what we already read (`combined`) and keeps streaming from
        // our pipe channel; the detached reader threads keep feeding it and
        // exit at EOF on their own. The call resolves as backgrounded, and
        // the agent loop keeps going with the HANDOFF text as the tool result
        // — the model asked for a foreground run, so it must be told the user
        // moved the command (not read a launch acknowledgement it never
        // requested). See `docs/background.md`. A subagent's executor never
        // consumes the latch — Ctrl+B targets the main turn's command (or its
        // agent group's wait loop), not whatever bash a subagent happens to
        // be running concurrently.
        if origin.is_none()
            && let Some(registry) = background
            && registry.take_background_request()
        {
            let task = registry.adopt(
                &args.command,
                args.description.clone(),
                true,
                child,
                chunk_rx,
                combined,
            );
            return ToolOutcome::backgrounded(task.id.clone(), background_handoff_text(&task));
        }
        if start.elapsed() >= timeout {
            crate::subprocess::kill_process_group(&mut child);
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
                crate::subprocess::kill_process_group(&mut child);
                return ToolOutcome::error(format!("error waiting on command: {err}"));
            }
        }
    };
    // Even on a clean exit, reap any process the command backgrounded — it holds
    // the pipe open, so joining the readers below would otherwise block on it.
    crate::subprocess::kill_process_group(&mut child);
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
    let (combined, extra_trunc) = tools::truncate_output(combined, cap);
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
        background: None,
        image: None,
        tasks: None,
        context: None,
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

/// `read`: an image file (`tools::is_image_path`) is returned visually — see
/// [`read_image`]; a text file returns `cat -n`-style numbered lines,
/// byte-capped. On a model whose `/v1/models` record said "no image input"
/// (`vision == Some(false)`), an image read fails as a recoverable error
/// before touching the file — attaching would make the provider fail the
/// whole turn instead (`docs/tools.md`).
fn run_read(arguments: &str, vision: Option<bool>) -> ToolOutcome {
    let args: ReadArgs = match tools::parse_args(arguments) {
        Ok(a) => a,
        Err(e) => return arg_error(e),
    };
    if tools::is_image_path(&args.path) && vision == Some(false) {
        return ToolOutcome::error(format!(
            "cannot view {}: the current model does not support image input — \
             work with the file another way, or ask the user to switch to a \
             vision-capable model with /model",
            args.path
        ));
    }
    let path = Path::new(&args.path);
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(err) => return ToolOutcome::error(format!("could not read {}: {err}", args.path)),
    };
    if tools::is_image_path(&args.path) {
        return read_image(&args.path, &bytes);
    }
    let content = String::from_utf8_lossy(&bytes);
    if content.is_empty() {
        return ToolOutcome::ok(format!("(file {} is empty)", args.path));
    }
    let numbered = tools::format_read(&content, args.offset, args.limit);
    let (output, truncated) = tools::truncate_output(numbered, TOOL_OUTPUT_MAX_BYTES);
    ToolOutcome::ok(output).with_truncated(truncated)
}

/// The image branch of `read` (`docs/tools.md`): bound the size, sniff the
/// real format from the bytes (the MIME must match the content, not the
/// extension, or a provider decodes garbage), read the header dimensions, and
/// return the facts as the text output with the base64 `data:` URL riding
/// [`ToolOutcome::image`] — the agent loop attaches it as a follow-up user
/// message. Every failure is a recoverable error the model reads.
fn read_image(path: &str, bytes: &[u8]) -> ToolOutcome {
    const MB: f64 = 1024.0 * 1024.0;
    // Sniff the content (magic bytes), the clipboard module's pattern — an
    // io::Error is impossible over an in-memory cursor but handled anyway.
    let reader = match image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format() {
        Ok(reader) => reader,
        Err(err) => return ToolOutcome::error(format!("could not inspect {path}: {err}")),
    };
    // The sniffed format is kept, not just its MIME: the auto-resize below
    // needs a decoder for it.
    let Some((format, (mime, label))) = reader
        .format()
        .and_then(|format| Some((format, vision_format(format)?)))
    else {
        return ToolOutcome::error(format!(
            "{path} is not a supported image (its content is not png/jpeg/gif/webp) — \
             read it as text or convert it first"
        ));
    };
    let (width, height) = match reader.into_dimensions() {
        Ok(dimensions) => dimensions,
        Err(err) => {
            return ToolOutcome::error(format!("could not decode {path} as an image: {err}"));
        }
    };
    // `/settings` **Auto-resize images**: a 12-megapixel photo is megabytes of
    // base64 the model reads no better than the same picture at 2000 pixels,
    // and past the byte cap it could not be sent at all. The *file* is
    // untouched — only what rides the request shrinks — so the fact line
    // leads with the file's own size and names the sent one after
    // (`docs/images.md`).
    let sent = crate::images::downscale_for_model(bytes, format);
    let (payload, mime, label) = match &sent {
        Some(small) => {
            let (mime, label) = vision_format(small.format).unwrap_or((mime, label));
            (small.bytes.as_slice(), mime, label)
        }
        None => (bytes, mime, label),
    };
    if payload.len() > tools::READ_IMAGE_MAX_BYTES {
        return ToolOutcome::error(format!(
            "image {path} is too large to attach ({:.1} MB; the limit is {:.2} MB) — \
             turn on Auto-resize images in /settings, or downscale it with a bash command first",
            payload.len() as f64 / MB,
            tools::READ_IMAGE_MAX_BYTES as f64 / MB,
        ));
    }
    let url = format!(
        "data:{mime};base64,{}",
        crate::clipboard::base64_encode(payload)
    );
    let text = match &sent {
        Some(small) => tools::format_read_image_resized(
            label,
            width,
            height,
            bytes.len(),
            small.size,
            small.bytes.len(),
        ),
        None => tools::format_read_image(label, width, height, bytes.len()),
    };
    ToolOutcome::ok(text).with_image(url)
}

/// The `(MIME, display label)` for a sniffed format — `None` for anything a
/// vision-capable OpenAI-compatible endpoint doesn't accept.
fn vision_format(format: image::ImageFormat) -> Option<(&'static str, &'static str)> {
    match format {
        image::ImageFormat::Png => Some(("image/png", "PNG")),
        image::ImageFormat::Jpeg => Some(("image/jpeg", "JPEG")),
        image::ImageFormat::Gif => Some(("image/gif", "GIF")),
        image::ImageFormat::WebP => Some(("image/webp", "WebP")),
        _ => None,
    }
}

/// `write`: create parent dirs and write the file, reporting a diff vs the old
/// contents (or a `Wrote …` summary for a new file).
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
    let display = describe_change(&args.path, &old, &args.content, !existed);
    // The cell keeps the numbered body; the model reads one line, because the
    // content it just sent replays verbatim on the call itself
    // (`docs/tools.md`, `docs/context.md`). A write that changed nothing has
    // no body to spare it, so it stays single-text.
    if old == args.content {
        return ToolOutcome::ok(display);
    }
    ToolOutcome::ok(display).with_context(tools::write_ack(&args.path, !existed))
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
    // [`run_write`]'s split: the diff hunks are the cell, the ack is the
    // conversation. `apply_edit` refuses a no-op, so there is always a change.
    ToolOutcome::ok(describe_change(
        &args.path,
        &old,
        &result.new_content,
        false,
    ))
    .with_context(tools::edit_ack(&args.path, result.replacements))
}

/// The model-facing result of a `write`/`edit` — also exactly what the cell
/// shows (the TUI restyles the rows; see `docs/tools.md`). A brand-new file is
/// a `Wrote {N} lines to {path}` head over the numbered contents
/// ([`tools::write_report`]); a change to existing content is an
/// `Updated {path} (+A -D)` head over the numbered diff hunks
/// ([`tools::update_report`]). The numbers match the `read` tool's, so the
/// model can cite them in a follow-up `edit`; the head's path is the compact
/// cwd-relative display form ([`tools::display_path`] — `../` climbs for a
/// target outside the cwd), while the cell header keeps the model's own
/// argument verbatim.
fn describe_change(path: &str, old: &str, new: &str, created: bool) -> String {
    let shown = shown_path(path);
    if created {
        tools::write_report(&shown, new)
    } else {
        tools::update_report(&shown, old, new)
    }
}

/// The display form of a tool path — [`tools::display_path`] against the
/// process cwd (the same directory every relative tool path resolves in), the
/// path unchanged when the cwd is unreadable.
fn shown_path(path: &str) -> String {
    match std::env::current_dir() {
        Ok(cwd) => tools::display_path(path, &cwd),
        Err(_) => path.to_string(),
    }
}

/// Create the parent directories of `path`, if any (a bare filename has none).
fn create_parents(path: &Path) -> std::io::Result<()> {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => std::fs::create_dir_all(parent),
        _ => Ok(()),
    }
}

/// A unique temp path under the system temp dir for a test file.
#[cfg(test)]
fn temp_path(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("alter-zero-exec-test-{name}"))
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
    fn bash_survives_a_stale_detach_helper() {
        // The helper is a fallback TIER (`subprocess::tiers`), not the only
        // route: with the `setsid` binary present the chain never needs it,
        // and even a helper path that no longer exists (the TUI binary
        // replaced or deleted mid-session) must not break the tool — the
        // chain lands on a working tier either way. The real helper's conduct
        // (setsid → no controlling terminal, then exec `sh -c`) is covered by
        // tests/detached_exec.rs against the built binary, and by smoke.
        let out = RealToolExecutor::new()
            .with_detach_helper(Some(temp_path("no-such-helper")))
            .execute(
                &call("bash", r#"{"command":"echo hi"}"#),
                &CancelToken::new(),
                &mut |_| {},
            );
        assert!(out.ok, "the chain survives a stale helper: {}", out.output);
        assert!(out.output.contains("hi"), "got {}", out.output);
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
        // The live schema spelling — the sibling tests below keep the
        // `timeout_ms` alias covered.
        let out = exec("bash", r#"{"command":"sleep 5","timeout":150}"#);
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

    // ===== image reads (docs/tools.md) =====

    #[test]
    fn read_of_a_png_attaches_the_image() {
        let path = temp_path("read-image.png");
        image::RgbaImage::from_pixel(3, 2, image::Rgba([220, 20, 20, 255]))
            .save_with_format(&path, image::ImageFormat::Png)
            .unwrap();
        let out = exec("read", &format!(r#"{{"path":"{}"}}"#, path.display()));
        std::fs::remove_file(&path).ok();
        assert!(out.ok, "got {}", out.output);
        assert!(
            out.output.starts_with("Read image "),
            "the marker head leads: {}",
            out.output
        );
        assert!(out.output.contains("PNG"), "sniffed format: {}", out.output);
        assert!(out.output.contains("3x2"), "dimensions: {}", out.output);
        // Concise, Claude-Code style: one fact line, the path left to the
        // `● Read({path})` header and the follow-up `[image]` note.
        assert_eq!(
            out.output.lines().count(),
            1,
            "one concise line: {}",
            out.output
        );
        assert!(
            !out.output.contains(&path.display().to_string()),
            "the path never repeats in the fact line: {}",
            out.output
        );
        let url = out.image.expect("the data: URL rides beside the text");
        assert!(url.starts_with("data:image/png;base64,"), "got {url}");
        assert!(
            !out.output.contains("base64"),
            "the URL never pollutes the text output: {}",
            out.output
        );
    }

    #[test]
    fn read_image_mime_follows_the_sniffed_content_not_the_extension() {
        // JPEG bytes misnamed .png: the data: URL must say image/jpeg or a
        // provider decodes garbage.
        let path = temp_path("read-image-mislabeled.png");
        image::RgbImage::from_pixel(2, 2, image::Rgb([1, 2, 3]))
            .save_with_format(&path, image::ImageFormat::Jpeg)
            .unwrap();
        let out = exec("read", &format!(r#"{{"path":"{}"}}"#, path.display()));
        std::fs::remove_file(&path).ok();
        assert!(out.ok, "got {}", out.output);
        assert!(out.output.contains("JPEG"), "got {}", out.output);
        let url = out.image.expect("attached");
        assert!(url.starts_with("data:image/jpeg;base64,"), "got {url}");
    }

    #[test]
    fn read_of_a_misnamed_non_image_is_a_recoverable_error() {
        let path = temp_path("read-not-an-image.png");
        std::fs::write(&path, "just text pretending to be pixels").unwrap();
        let out = exec("read", &format!(r#"{{"path":"{}"}}"#, path.display()));
        std::fs::remove_file(&path).ok();
        assert!(!out.ok, "got {}", out.output);
        assert!(out.image.is_none());
        assert!(
            out.output.contains("not"),
            "the model learns the file isn't an image: {}",
            out.output
        );
    }

    #[test]
    fn an_oversized_file_that_is_not_an_image_says_exactly_that() {
        // Megabytes of zeroes named `.png`. The sniff runs before the size
        // check now — auto-resize can rescue an oversized *picture*, so
        // "too large" would be the wrong first answer — and what this file
        // actually is is not a picture at all.
        let path = temp_path("read-image-huge.png");
        std::fs::write(&path, vec![0u8; tools::READ_IMAGE_MAX_BYTES + 1]).unwrap();
        let out = exec("read", &format!(r#"{{"path":"{}"}}"#, path.display()));
        std::fs::remove_file(&path).ok();
        assert!(!out.ok, "got {}", out.output);
        assert!(out.image.is_none());
        assert!(
            out.output.contains("not a supported image"),
            "the model learns what is actually wrong: {}",
            out.output
        );
    }

    #[test]
    fn an_image_past_the_resize_cap_is_downscaled_not_refused() {
        // `/settings` **Auto-resize images**, on by default: the file keeps
        // its own size (which is what the cell draws), and the fact line names
        // the original first and what was uploaded after (`docs/images.md`).
        let path = temp_path("read-image-big.png");
        image::RgbImage::from_pixel(3000, 2000, image::Rgb([10, 120, 220]))
            .save_with_format(&path, image::ImageFormat::Png)
            .unwrap();
        let out = exec("read", &format!(r#"{{"path":"{}"}}"#, path.display()));
        std::fs::remove_file(&path).ok();
        assert!(out.ok, "got {}", out.output);
        assert!(
            out.output.starts_with("Read image (PNG, 3000x2000,"),
            "the file's own facts lead: {}",
            out.output
        );
        assert!(
            out.output.contains("sent resized to 2000x1333"),
            "and the uploaded size is named: {}",
            out.output
        );
        assert_eq!(
            crate::images::read_image_size(&out.output),
            Some((3000, 2000)),
            "the renderer still reads the size of the file it will draw"
        );
        assert!(out.image.is_some(), "the pixels still ride along");
    }

    #[test]
    fn an_image_read_on_a_known_non_vision_model_is_a_recoverable_error() {
        // The model's /v1/models record said "no image input": attaching
        // anyway would make the provider fail the whole turn (OpenRouter
        // 404s "No endpoints found that support image input"), so the read
        // resolves as an error the model reads and can act on instead.
        let path = temp_path("read-image-no-vision.png");
        image::RgbaImage::from_pixel(2, 2, image::Rgba([1, 2, 3, 255]))
            .save_with_format(&path, image::ImageFormat::Png)
            .unwrap();
        let out = RealToolExecutor::new().with_vision(Some(false)).execute(
            &call("read", &format!(r#"{{"path":"{}"}}"#, path.display())),
            &CancelToken::new(),
            &mut |_| {},
        );
        std::fs::remove_file(&path).ok();
        assert!(!out.ok, "got {}", out.output);
        assert!(out.image.is_none(), "nothing is attached");
        assert!(
            out.output.contains("does not support image input"),
            "the model learns why: {}",
            out.output
        );
        assert!(
            out.output.contains("/model"),
            "and how the user could fix it: {}",
            out.output
        );
    }

    #[test]
    fn vision_true_or_unknown_still_attaches_and_text_reads_ignore_the_gate() {
        let path = temp_path("read-image-vision-ok.png");
        image::RgbaImage::from_pixel(2, 2, image::Rgba([9, 9, 9, 255]))
            .save_with_format(&path, image::ImageFormat::Png)
            .unwrap();
        for vision in [Some(true), None] {
            let out = RealToolExecutor::new().with_vision(vision).execute(
                &call("read", &format!(r#"{{"path":"{}"}}"#, path.display())),
                &CancelToken::new(),
                &mut |_| {},
            );
            assert!(out.ok, "vision {vision:?}: {}", out.output);
            assert!(out.image.is_some(), "vision {vision:?} attaches");
        }
        std::fs::remove_file(&path).ok();
        // The gate never touches a text read.
        let text = temp_path("read-text-no-vision.txt");
        std::fs::write(&text, "alpha\n").unwrap();
        let out = RealToolExecutor::new().with_vision(Some(false)).execute(
            &call("read", &format!(r#"{{"path":"{}"}}"#, text.display())),
            &CancelToken::new(),
            &mut |_| {},
        );
        std::fs::remove_file(&text).ok();
        assert!(out.ok, "got {}", out.output);
        assert!(out.output.contains("1 alpha"));
    }

    #[test]
    fn a_text_read_carries_no_image() {
        let path = temp_path("read-plain.txt");
        std::fs::write(&path, "alpha\n").unwrap();
        let out = exec("read", &format!(r#"{{"path":"{}"}}"#, path.display()));
        std::fs::remove_file(&path).ok();
        assert!(out.ok);
        assert!(out.image.is_none(), "text reads stay text-only");
    }

    #[test]
    fn read_of_a_missing_file_is_a_recoverable_error() {
        let out = exec("read", r#"{"path":"/definitely/not/here.txt"}"#);
        assert!(!out.ok);
        assert!(out.output.contains("could not read"));
    }

    /// The head's expected display path — the same cwd-relative form
    /// `describe_change` derives, computed against the test process's cwd so
    /// the assertion holds wherever the temp dir lives.
    fn shown(path: &Path) -> String {
        tools::display_path(
            &path.display().to_string(),
            &std::env::current_dir().unwrap(),
        )
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
        // The Claude-Code head over the cwd-relative display path — the temp
        // dir is outside the cwd, so the path shows as a `../` climb, never
        // the whole absolute argument.
        assert_eq!(
            out.output.lines().next(),
            Some(format!("Wrote 2 lines to {}", shown(&path)).as_str()),
            "got {}",
            out.output
        );
        assert!(
            shown(&path).starts_with("../"),
            "the temp file lives outside the cwd: {}",
            shown(&path)
        );
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
        assert_eq!(
            out.output.lines().next(),
            Some(format!("Updated {} (+1 -1)", shown(&path)).as_str()),
            "got {}",
            out.output
        );
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
    fn write_hands_the_model_one_line_and_the_cell_the_numbered_body() {
        // The two-text split (`ToolOutcome::context`, docs/tools.md): the
        // numbered content is the *cell*, and what rides the conversation is
        // one line — the arguments already carry the content verbatim.
        let path = temp_path("write-ack-new.txt");
        std::fs::remove_file(&path).ok();
        let out = exec(
            "write",
            &format!(r#"{{"path":"{}","content":"one\ntwo\n"}}"#, path.display()),
        );
        std::fs::remove_file(&path).ok();
        assert!(out.output.contains("1 one"), "the cell keeps the body");
        assert_eq!(
            out.context.as_deref(),
            Some(tools::write_ack(&path.display().to_string(), /*created=*/ true).as_str()),
            "the model reads the ack, naming its own absolute argument"
        );
    }

    #[test]
    fn write_over_existing_content_acknowledges_an_update() {
        let path = temp_path("write-ack-over.txt");
        std::fs::write(&path, "keep\nold\n").unwrap();
        let out = exec(
            "write",
            &format!(r#"{{"path":"{}","content":"keep\nnew\n"}}"#, path.display()),
        );
        std::fs::remove_file(&path).ok();
        assert!(out.output.contains("2 +new"), "the cell keeps the diff");
        assert_eq!(
            out.context.as_deref(),
            Some(tools::write_ack(&path.display().to_string(), /*created=*/ false).as_str())
        );
    }

    #[test]
    fn a_write_that_changes_nothing_has_no_second_text() {
        // `No changes to …` is already the whole truth: there is no body to
        // spare the model, so the outcome stays a plain single-text ToolEnd.
        let path = temp_path("write-ack-same.txt");
        std::fs::write(&path, "same\n").unwrap();
        let out = exec(
            "write",
            &format!(r#"{{"path":"{}","content":"same\n"}}"#, path.display()),
        );
        std::fs::remove_file(&path).ok();
        assert!(
            out.output.starts_with("No changes to "),
            "got {}",
            out.output
        );
        assert_eq!(out.context, None);
    }

    #[test]
    fn a_failed_write_or_edit_carries_no_second_text() {
        // An error's display *is* what the model must read to recover, so the
        // split would hide the reason. Both tools return the error before
        // they can attach an ack — this pins that they keep doing so.
        let missing = temp_path("edit-no-file.txt");
        std::fs::remove_file(&missing).ok();
        let no_file = exec(
            "edit",
            &format!(
                r#"{{"path":"{}","old_string":"a","new_string":"b"}}"#,
                missing.display()
            ),
        );
        assert!(!no_file.ok && no_file.context.is_none(), "{no_file:?}");

        let seeded = temp_path("edit-no-match.txt");
        std::fs::write(&seeded, "abc\n").unwrap();
        let no_match = exec(
            "edit",
            &format!(
                r#"{{"path":"{}","old_string":"zzz","new_string":"y"}}"#,
                seeded.display()
            ),
        );
        std::fs::remove_file(&seeded).ok();
        assert!(!no_match.ok && no_match.context.is_none(), "{no_match:?}");

        // A `write` that cannot create its parent (a file stands where the
        // directory would go) fails the same way.
        let blocker = temp_path("write-blocked.txt");
        std::fs::write(&blocker, "not a dir\n").unwrap();
        let blocked = exec(
            "write",
            &format!(
                r#"{{"path":"{}/inside.txt","content":"x"}}"#,
                blocker.display()
            ),
        );
        std::fs::remove_file(&blocker).ok();
        assert!(!blocked.ok && blocked.context.is_none(), "{blocked:?}");
    }

    #[test]
    fn edit_hands_the_model_one_line_and_counts_a_replace_all() {
        let path = temp_path("edit-ack.txt");
        std::fs::write(&path, "a\na\n").unwrap();
        let out = exec(
            "edit",
            &format!(
                r#"{{"path":"{}","old_string":"a","new_string":"b","replace_all":true}}"#,
                path.display()
            ),
        );
        std::fs::remove_file(&path).ok();
        assert!(out.output.contains("1 +b"), "the cell keeps the diff");
        assert_eq!(
            out.context.as_deref(),
            Some(tools::edit_ack(&path.display().to_string(), 2).as_str()),
            "the ack counts what `replace_all` really touched"
        );
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

    // ===== background (docs/background.md) =====

    #[test]
    fn background_launch_text_names_the_shell_by_its_interim_file_alone() {
        // No task-id label rides the text: nothing model-facing takes one
        // back (kills go by PID, progress by the interim file), so naming it
        // just asks the model to track a token with no use. The id cannot be
        // scrubbed outright — the interim file is named after it — so the
        // pin is that it appears exactly once, as that basename: any
        // reintroduced `ID: {id}` label makes it two.
        let task = crate::background::LaunchedTask {
            id: "bvyo7tkbe".to_string(),
            output_path: std::path::PathBuf::from("/tmp/alter-zero-0/s1/bvyo7tkbe.output"),
        };
        let text = background_launch_text(&task);
        assert_eq!(
            text.matches("bvyo7tkbe").count(),
            1,
            "the id appears only as the interim file's basename: {text}"
        );
        assert!(
            text.contains("/tmp/alter-zero-0/s1/bvyo7tkbe.output"),
            "the interim path is the model's progress channel: {text}"
        );
        assert!(text.lines().count() <= 2, "short: {text}");
    }

    #[test]
    fn background_handoff_text_leads_with_the_user_move_over_the_launch_facts() {
        // Ctrl+B: the model expected a foreground run's full output, so the
        // handoff must say the USER moved the command, carry the launch facts
        // (interim path + completion promise) verbatim, and steer
        // the model off re-running/polling (docs/background.md).
        let task = crate::background::LaunchedTask {
            id: "bash_7".to_string(),
            output_path: std::path::PathBuf::from("/tmp/tasks/bash_7.output"),
        };
        let text = background_handoff_text(&task);
        assert!(
            text.starts_with("The user moved this command to the background"),
            "got {text}"
        );
        assert!(
            text.contains(&background_launch_text(&task)),
            "the launch facts ride along verbatim: {text}"
        );
        assert!(
            text.contains("Do not run the command again"),
            "the model is steered off re-running/waiting: {text}"
        );
    }

    fn test_registry() -> (
        crate::background::BackgroundRegistry,
        tokio::sync::mpsc::UnboundedReceiver<crate::background::BgEvent>,
    ) {
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let dir = std::env::temp_dir().join(format!(
            "alter-zero-exec-bg-test-{}-{seq}",
            std::process::id()
        ));
        (crate::background::BackgroundRegistry::new(tx, dir), rx)
    }

    #[test]
    fn run_in_background_returns_a_backgrounded_launch_at_once() {
        let (registry, mut rx) = test_registry();
        let executor = RealToolExecutor::new().with_background(registry);
        let start = std::time::Instant::now();
        let out = executor.execute(
            &call(
                "bash",
                r#"{"command":"sleep 0.2; printf done","run_in_background":true,"description":"nap"}"#,
            ),
            &CancelToken::new(),
            &mut |_| {},
        );
        assert!(
            start.elapsed() < std::time::Duration::from_millis(150),
            "the call returns before the command finishes"
        );
        assert!(out.ok);
        let task_id = out
            .background
            .clone()
            .expect("the outcome carries the task id");
        assert!(
            task_id.starts_with('b') && task_id.len() == 9,
            "a claude-code-style id: {task_id}"
        );
        assert!(
            out.output.matches(task_id.as_str()).count() == 1 && out.output.contains(".output"),
            "the id appears only as the interim file's basename: {}",
            out.output
        );
        assert!(
            !out.output.contains("user moved"),
            "a launch the model asked for must not claim a user action: {}",
            out.output
        );
        // The registry reports the launch and, later, the completion.
        let mut saw_started = false;
        let mut saw_exit = false;
        for _ in 0..500 {
            match rx.try_recv() {
                Ok(crate::background::BgEvent::Started {
                    id,
                    from_model,
                    description,
                    ..
                }) => {
                    assert_eq!(id, task_id);
                    assert!(from_model, "an executor launch is model-launched");
                    assert_eq!(description.as_deref(), Some("nap"));
                    saw_started = true;
                }
                Ok(crate::background::BgEvent::Exited { code, killed, .. }) => {
                    assert_eq!(code, Some(0));
                    assert!(!killed);
                    saw_exit = true;
                    break;
                }
                Ok(crate::background::BgEvent::Output { .. }) => {}
                Err(_) => std::thread::sleep(std::time::Duration::from_millis(10)),
            }
        }
        assert!(
            saw_started && saw_exit,
            "the registry reported the lifecycle"
        );
    }

    #[test]
    fn run_in_background_without_a_registry_is_a_recoverable_error() {
        let out = exec("bash", r#"{"command":"echo hi","run_in_background":true}"#);
        assert!(!out.ok);
        assert!(out.background.is_none());
        assert!(
            out.output.contains("not available"),
            "the model is told to run it in the foreground: {}",
            out.output
        );
    }

    #[test]
    fn a_background_request_hands_a_running_command_off_mid_run() {
        // The Ctrl+B path: raise the latch mid-run; the poll loop consumes it
        // and adopts the child — the outcome flips to backgrounded and the
        // already-produced output replays into the background stream.
        let (registry, mut rx) = test_registry();
        let executor = RealToolExecutor::new().with_background(registry.clone());
        let flag = registry.clone();
        // Raise the latch shortly after the command starts producing output.
        let raiser = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(120));
            flag.request_background();
        });
        let mut streamed = String::new();
        let out = executor.execute(
            &call(
                "bash",
                r#"{"command":"printf 'early\n'; sleep 3; printf 'late\n'","timeout_ms":30000}"#,
            ),
            &CancelToken::new(),
            &mut |chunk| streamed.push_str(chunk),
        );
        raiser.join().unwrap();
        assert!(out.ok, "got {}", out.output);
        let task_id = out
            .background
            .clone()
            .expect("the outcome carries the task id");
        assert!(
            task_id.starts_with('b') && task_id.len() == 9,
            "a claude-code-style id: {task_id}"
        );
        // The model asked for a FOREGROUND run — the result must say the
        // *user* moved it (not read like a run_in_background acknowledgement),
        // or the model keeps waiting for the full output and polls the interim
        // file round after round (docs/background.md).
        assert!(
            out.output
                .starts_with("The user moved this command to the background"),
            "the model is told the user moved it: {}",
            out.output
        );
        assert!(
            out.output.matches(task_id.as_str()).count() == 1 && out.output.contains(".output"),
            "the interim file (and no id label) rides the handoff text: {}",
            out.output
        );
        assert!(streamed.contains("early"), "the foreground tail ran first");
        // The adopted task replays the prior output and finishes on its own.
        let mut replayed = String::new();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            match rx.try_recv() {
                Ok(crate::background::BgEvent::Output { chunk, .. }) => replayed.push_str(&chunk),
                Ok(crate::background::BgEvent::Exited { code, .. }) => {
                    assert_eq!(code, Some(0));
                    break;
                }
                Ok(crate::background::BgEvent::Started { .. }) => {}
                Err(_) => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "the adopted task finishes on its own"
                    );
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
            }
        }
        assert!(
            replayed.contains("early") && replayed.contains("late"),
            "prior output replays and the tail keeps streaming: {replayed:?}"
        );
    }

    #[test]
    fn a_stale_background_request_is_cleared_when_a_command_starts() {
        // A Ctrl+B that missed its command must not background the next one.
        let (registry, _rx) = test_registry();
        registry.request_background();
        let executor = RealToolExecutor::new().with_background(registry);
        let out = executor.execute(
            &call("bash", r#"{"command":"echo hi"}"#),
            &CancelToken::new(),
            &mut |_| {},
        );
        assert!(out.ok);
        assert!(
            out.background.is_none(),
            "the stale request was dropped, the command ran in the foreground"
        );
    }

    #[test]
    fn a_subagent_executor_launches_in_background_with_its_origin() {
        // A subagent's run_in_background works like the main turn's — the
        // shell joins the shared registry — with the launcher attributed on
        // the Started event (docs/agent-tool.md).
        let (registry, mut rx) = test_registry();
        let executor = RealToolExecutor::new()
            .with_background(registry)
            .with_background_origin(crate::background::BgOrigin {
                agent_id: "a7k2m9x4q".into(),
                agent_type: "general-purpose".into(),
            });
        let out = executor.execute(
            &call(
                "bash",
                r#"{"command":"echo bg","run_in_background":true,"description":"noop"}"#,
            ),
            &CancelToken::new(),
            &mut |_| {},
        );
        assert!(out.ok, "got {}", out.output);
        assert!(out.background.is_some(), "resolves as backgrounded");
        let mut saw_origin = false;
        for _ in 0..500 {
            match rx.try_recv() {
                Ok(crate::background::BgEvent::Started { origin, .. }) => {
                    let origin = origin.expect("the launcher rides the event");
                    assert_eq!(origin.agent_id, "a7k2m9x4q");
                    assert_eq!(origin.agent_type, "general-purpose");
                    saw_origin = true;
                    break;
                }
                Ok(_) => {}
                Err(_) => std::thread::sleep(std::time::Duration::from_millis(10)),
            }
        }
        assert!(saw_origin, "the Started event carried the origin");
    }

    #[test]
    fn a_subagent_executor_never_consumes_the_ctrl_b_latch() {
        // Ctrl+B belongs to the MAIN turn's foreground command: a subagent's
        // concurrently-running bash must neither clear a pending request as
        // it starts nor take it mid-run (docs/agent-tool.md).
        let (registry, _rx) = test_registry();
        registry.request_background();
        let executor = RealToolExecutor::new()
            .with_background(registry.clone())
            .with_background_origin(crate::background::BgOrigin {
                agent_id: "a1".into(),
                agent_type: "explore".into(),
            });
        let out = executor.execute(
            &call("bash", r#"{"command":"echo hi"}"#),
            &CancelToken::new(),
            &mut |_| {},
        );
        assert!(out.ok);
        assert!(
            out.background.is_none(),
            "the pending latch never backgrounds a subagent's command"
        );
        assert!(
            registry.take_background_request(),
            "…and the latch is still raised for the main turn's runner"
        );
    }
}
