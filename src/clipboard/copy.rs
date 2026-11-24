//! The **write** side of the clipboard: `/copy`, and every page that copies a
//! value (`/export`, `/donate`, `/sponsor`, `/discord`, `/login`, `/mcp`,
//! `/secrets`, `/usage`), puts text on the user's clipboard through
//! [`copy_to_clipboard`]. See `docs/copy.md`.
//!
//! A copy has two halves, and where the session runs decides how they
//! combine (`CopyEnvironment` → `copy_plan`):
//!
//! - the **native** clipboard: arboard, the desktop's own;
//! - the **terminal**: the user's terminal, reached through whatever sits
//!   between it and this process. Inside tmux that is tmux's own buffer,
//!   which tmux sends on to its client's terminal (`tmux load-buffer -w -`);
//!   anywhere else, an OSC 52 escape written to the terminal, framed for GNU
//!   screen's passthrough inside screen.
//!
//! On a desktop the native clipboard *is* the user's, so the terminal is
//! only the fallback. Over SSH, and inside a multiplexer, it may not be: the
//! machine's clipboard can be a server's that nobody pastes from, and a tmux
//! or screen session outlives the terminal that started it and may be
//! attached from another. There the text goes to the terminal as well,
//! whatever the native clipboard did.
//!
//! tmux is why the terminal half is not just an escape. Its default
//! `set-clipboard external` drops an OSC 52 escape written by an application
//! in a pane, and its default `allow-passthrough off` (tmux 3.3 and later)
//! drops one wrapped in its passthrough too. So a copy that relied on the
//! escape never left a remote tmux. tmux's own buffer is the way through:
//! `load-buffer -w` has tmux send the escape itself, which the default lets
//! out.
//!
//! The decision and the order of the attempts are pure (`copy_with`, over
//! injected effects) and unit-tested, as is the escape framing. The effects
//! themselves (arboard, the `tmux` command, the `/dev/tty` write) are the
//! boundary. The smoke suite covers them: Phase 28 for the bare escape,
//! Phase 169 for a tmux on a remote host. The `tmux` runner is also tested
//! over stand-in commands.

use std::ffi::OsString;
use std::io::{Read, Write};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use super::base64_encode;

/// A handle that keeps a native clipboard selection alive. On Linux, arboard
/// serves the clipboard from a background thread tied to the
/// [`arboard::Clipboard`]'s lifetime, so dropping it right after `set_text`
/// can clear what was just set. The caller (the event loop) holds this for
/// the app's lifetime. Empty on other platforms, whose OS owns the clipboard
/// once set. See `docs/copy.md`.
pub struct ClipboardLease {
    #[cfg(target_os = "linux")]
    _clipboard: arboard::Clipboard,
}

/// Write `text` to the user's clipboard by every route this session can
/// reach it by, returning a [`ClipboardLease`] to hold for the app's
/// lifetime when the native clipboard took the text (Linux/arboard).
///
/// The native clipboard is tried first. The terminal comes next: always
/// inside tmux or GNU screen and over SSH, where the machine's clipboard may
/// not be the user's, and otherwise only when the native copy failed. Inside
/// tmux the text goes into tmux's own buffer, and tmux sends it on to its
/// client's terminal (`tmux load-buffer -w -`). Anywhere else, or when tmux
/// cannot take it, an OSC 52 escape is written to the controlling terminal.
/// `Err` only when every route failed, naming each cause in the order tried.
/// See `docs/copy.md`.
///
/// Boundary I/O. The routing it runs is pure and unit-tested, and so is the
/// escape framing.
pub fn copy_to_clipboard(text: &str) -> Result<Option<ClipboardLease>, String> {
    copy_with(
        copy_plan(CopyEnvironment::from_env()),
        text,
        arboard_copy,
        tmux_copy,
        write_to_terminal,
    )
}

/// What the environment says about how this session reaches the user's
/// terminal: the facts a copy is routed on. Read at the boundary
/// ([`CopyEnvironment::from_env`]) so the routing itself is pure.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct CopyEnvironment {
    /// Inside tmux: `TMUX` (its server's socket) or `TMUX_PANE` names one.
    tmux: bool,
    /// Inside GNU screen: `STY` names its session.
    screen: bool,
    /// Logged in over SSH: `SSH_CONNECTION`, `SSH_CLIENT` or `SSH_TTY`.
    ssh: bool,
}

impl CopyEnvironment {
    /// The facts from `var`, a lookup of one environment variable: the
    /// process's own at the boundary, a table in a test. A variable that is
    /// set but empty names nothing.
    fn from_lookup(var: impl Fn(&str) -> Option<OsString>) -> Self {
        let named = |name: &str| var(name).is_some_and(|value| !value.is_empty());
        Self {
            tmux: named("TMUX") || named("TMUX_PANE"),
            screen: named("STY"),
            ssh: named("SSH_CONNECTION") || named("SSH_CLIENT") || named("SSH_TTY"),
        }
    }

    /// The facts from this process's environment.
    fn from_env() -> Self {
        Self::from_lookup(|name| std::env::var_os(name))
    }
}

/// Where the terminal half of a copy is sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TerminalRoute {
    /// Into tmux's own buffer, which tmux sends on to its client's terminal
    /// (`tmux load-buffer -w -`); the escape framed for tmux when that fails.
    Tmux,
    /// The escape, framed for GNU screen's passthrough.
    Screen,
    /// The escape, bare.
    Direct,
}

/// How a copy is made: the native clipboard first, then the terminal route.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CopyPlan {
    /// Where the terminal half goes.
    terminal: TerminalRoute,
    /// Send to the terminal even after the native clipboard took the text,
    /// not only when it failed.
    always_forward: bool,
}

/// The plan for a session in `env`. Inside tmux the copy goes through tmux,
/// whatever else is true: a screen or an SSH login inside tmux still writes
/// to tmux's pane. The terminal is a fallback only on a desktop with neither
/// a multiplexer nor an SSH login in the way, where the native clipboard is
/// the user's own.
fn copy_plan(env: CopyEnvironment) -> CopyPlan {
    let terminal = if env.tmux {
        TerminalRoute::Tmux
    } else if env.screen {
        TerminalRoute::Screen
    } else {
        TerminalRoute::Direct
    };
    CopyPlan {
        terminal,
        always_forward: env.tmux || env.screen || env.ssh,
    }
}

/// Make a copy of `text` by `plan`: the native clipboard first, then the
/// terminal, always when the plan says so and otherwise only when the native
/// copy failed.
///
/// The effects are injected so the order, the fallbacks and the errors are
/// tested without a clipboard, a tmux or a terminal. `native` sets the
/// desktop clipboard and hands back what keeps its selection alive (`L`).
/// `tmux` loads tmux's buffer. `write` puts an escape on the terminal. The
/// copy succeeds when either half did, and the native half's hold is what it
/// returns. It is an error only when both failed, naming every attempt in
/// order.
fn copy_with<L>(
    plan: CopyPlan,
    text: &str,
    native: impl FnOnce(&str) -> Result<Option<L>, String>,
    tmux: impl FnOnce(&str) -> Result<(), String>,
    write: impl FnOnce(&str) -> Result<(), String>,
) -> Result<Option<L>, String> {
    let native = native(text);
    if native.is_ok() && !plan.always_forward {
        return native;
    }
    let terminal = to_terminal(plan.terminal, text, tmux, write);
    match (native, terminal) {
        (Ok(lease), _) => Ok(lease),
        (Err(_), Ok(())) => Ok(None),
        (Err(native), Err(terminal)) => Err(format!("{native}; {terminal}")),
    }
}

/// Send `text` to the terminal by `route`, through the injected `tmux` and
/// `write` ([`copy_with`]). The error names each attempt that failed, in
/// order.
fn to_terminal(
    route: TerminalRoute,
    text: &str,
    tmux: impl FnOnce(&str) -> Result<(), String>,
    write: impl FnOnce(&str) -> Result<(), String>,
) -> Result<(), String> {
    let escape = |frame| {
        osc52_escape(text, frame)
            .and_then(|sequence| write(&sequence))
            .map_err(|reason| format!("OSC 52: {reason}"))
    };
    match route {
        TerminalRoute::Tmux => tmux(text).or_else(|tmux| {
            escape(EscapeFrame::Tmux).map_err(|osc| format!("tmux: {tmux}; {osc}"))
        }),
        TerminalRoute::Screen => escape(EscapeFrame::Screen),
        TerminalRoute::Direct => escape(EscapeFrame::Bare),
    }
}

/// How an OSC 52 escape is framed for what lies between this process and
/// the terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EscapeFrame {
    /// Straight to the terminal.
    Bare,
    /// Through tmux, once the tmux route has failed: the escape wrapped in
    /// tmux's passthrough (`ESC P tmux; … ESC \`, each escape inside
    /// doubled), then once more bare. tmux delivers the first when it allows
    /// passthrough (always before 3.3, with `allow-passthrough on` since),
    /// and takes the second with `set-clipboard on`. A tmux allowing both
    /// sets the clipboard twice, to the same text.
    Tmux,
    /// Through GNU screen: cut into pieces of at most [`SCREEN_PIECE_BYTES`],
    /// each wrapped in screen's passthrough (`ESC P … ESC \`).
    Screen,
}

/// The most bytes of the escape one piece of GNU screen's passthrough
/// carries. With its four bytes of framing, that fits the 256 bytes the
/// oldest screens hold in one string.
const SCREEN_PIECE_BYTES: usize = 252;

/// The OSC 52 escape that sets the clipboard to `text`, framed for `frame`.
/// `Err` when `text` is too large for an escape ([`OSC52_MAX_BYTES`]).
fn osc52_escape(text: &str, frame: EscapeFrame) -> Result<String, String> {
    let bare = osc52_sequence(text)?;
    Ok(match frame {
        EscapeFrame::Bare => bare,
        EscapeFrame::Tmux => format!("\x1bPtmux;{}\x1b\\{bare}", bare.replace('\x1b', "\x1b\x1b")),
        EscapeFrame::Screen => screen_pieces(&bare),
    })
}

/// `escape` cut into pieces of at most [`SCREEN_PIECE_BYTES`], each wrapped
/// in GNU screen's passthrough (`ESC P … ESC \`). Screen hands each piece's
/// content to the terminal as it is, so the terminal reads the pieces back
/// as the one escape. The escape is ASCII (base64 between ASCII framing), so
/// every cut falls between two characters.
fn screen_pieces(escape: &str) -> String {
    let pieces = escape.len().div_ceil(SCREEN_PIECE_BYTES);
    let mut out = String::with_capacity(escape.len() + pieces * 4);
    let mut start = 0;
    while start < escape.len() {
        let end = (start + SCREEN_PIECE_BYTES).min(escape.len());
        out.push_str("\x1bP");
        out.push_str(&escape[start..end]);
        out.push_str("\x1b\\");
        start = end;
    }
    out
}

/// What the tmux route runs: `load-buffer` reading the text from stdin (`-`)
/// into a new paste buffer, and `-w` (tmux 3.2 and later) to have tmux send
/// that buffer on to its client's terminal as its own clipboard escape.
/// tmux's default `set-clipboard external` lets tmux's own escape out, where
/// it drops one an application writes.
const TMUX_LOAD_BUFFER_ARGS: [&str; 3] = ["load-buffer", "-w", "-"];

/// How long the tmux route waits for `tmux` before killing it and falling
/// back to the escape. tmux answers in milliseconds; this bounds what a
/// wedged server can cost the event loop the copy runs on.
const TMUX_COPY_TIMEOUT: Duration = Duration::from_secs(2);

/// How often [`wait_at_most`] checks whether its command has finished.
const WAIT_POLL_INTERVAL: Duration = Duration::from_millis(5);

/// The most of a failed command's stderr kept for its error message.
const STDERR_MAX_BYTES: u64 = 4096;

/// Hand `text` to tmux: the tmux route. Boundary I/O over [`run_with_input`].
fn tmux_copy(text: &str) -> Result<(), String> {
    run_with_input(
        "tmux",
        &TMUX_LOAD_BUFFER_ARGS,
        text.as_bytes(),
        TMUX_COPY_TIMEOUT,
    )
}

/// Run `program` with `args` and `input` on its stdin, and wait at most
/// `timeout` for it, killing it past that. `Ok` on a zero exit. Otherwise
/// the error is what the program said on stderr, or else how it ended.
fn run_with_input(
    program: &str,
    args: &[&str],
    input: &[u8],
    timeout: Duration,
) -> Result<(), String> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        // The TUI owns the terminal: nothing the command prints may reach it.
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not run {program}: {e}"))?;
    let stdin = child.stdin.take();
    let status = std::thread::scope(|scope| {
        if let Some(mut stdin) = stdin {
            // Written from a thread of its own so a command that never reads
            // its input cannot hold the caller past the timeout: the kill
            // below breaks the pipe, and the write returns. A command that
            // exits without reading everything breaks it too — its exit
            // status is its answer, not the failed write. A thread the system
            // refuses is an error rather than a panic: this runs on the event
            // loop, and the escape is still there to fall back to.
            let writer = std::thread::Builder::new().spawn_scoped(scope, move || {
                let _ = stdin.write_all(input);
            });
            if let Err(e) = writer {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("could not start a thread to feed {program}: {e}"));
            }
        }
        wait_at_most(&mut child, timeout)
    })?;
    if status.success() {
        return Ok(());
    }
    let mut said = String::new();
    if let Some(stderr) = child.stderr.take() {
        let _ = stderr.take(STDERR_MAX_BYTES).read_to_string(&mut said);
    }
    // One line: the reason ends up in a one-row toast.
    let said = said.split_whitespace().collect::<Vec<_>>().join(" ");
    Err(if said.is_empty() {
        format!("{program} failed ({status})")
    } else {
        said
    })
}

/// Wait for `child` to exit, at most `timeout`. Past it the child is killed
/// and reaped, and the wait is an error, as is a wait that fails: in either
/// case the child is gone when this returns.
fn wait_at_most(child: &mut Child, timeout: Duration) -> Result<ExitStatus, String> {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(WAIT_POLL_INTERVAL),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("did not finish within {}s", timeout.as_secs_f32()));
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("could not wait for it: {e}"));
            }
        }
    }
}

/// Set the clipboard via arboard (the native path). On **Linux** the live
/// `Clipboard` is returned in a [`ClipboardLease`] so the X11/Wayland
/// selection survives the call; on other platforms the OS retains the text
/// once set, so `Ok(None)`.
#[cfg(target_os = "linux")]
fn arboard_copy(text: &str) -> Result<Option<ClipboardLease>, String> {
    let mut clipboard =
        arboard::Clipboard::new().map_err(|e| format!("clipboard unavailable: {e}"))?;
    clipboard
        .set_text(text)
        .map_err(|e| format!("could not set the clipboard: {e}"))?;
    Ok(Some(ClipboardLease {
        _clipboard: clipboard,
    }))
}

#[cfg(not(target_os = "linux"))]
fn arboard_copy(text: &str) -> Result<Option<ClipboardLease>, String> {
    let mut clipboard =
        arboard::Clipboard::new().map_err(|e| format!("clipboard unavailable: {e}"))?;
    clipboard
        .set_text(text)
        .map_err(|e| format!("could not set the clipboard: {e}"))?;
    Ok(None)
}

/// Write `escape` to the controlling terminal (`/dev/tty`, falling back to
/// stdout). Boundary I/O.
fn write_to_terminal(escape: &str) -> Result<(), String> {
    // Prefer /dev/tty: the escape then bypasses the TUI's buffered stdout (no
    // interleaving with a half-written frame) and still reaches the terminal
    // if stdout is redirected. Fall back to stdout when there's no
    // controlling tty.
    if let Ok(mut tty) = std::fs::OpenOptions::new().write(true).open("/dev/tty") {
        write_all_flush(&mut tty, escape).map_err(|e| format!("could not write to /dev/tty: {e}"))
    } else {
        let mut stdout = std::io::stdout().lock();
        write_all_flush(&mut stdout, escape).map_err(|e| format!("could not write to stdout: {e}"))
    }
}

/// Write the whole string and flush it (one atomic escape).
fn write_all_flush(w: &mut impl Write, s: &str) -> std::io::Result<()> {
    w.write_all(s.as_bytes())?;
    w.flush()
}

/// Maximum raw byte length pushed through an OSC 52 escape. Terminals bound
/// the escape's payload, so an oversized selection is refused rather than
/// silently truncated. The tmux route takes more: tmux keeps the whole text
/// in its buffer whatever the terminal does with tmux's own escape.
const OSC52_MAX_BYTES: usize = 100_000;

/// The OSC 52 "set clipboard" escape carrying `text` (base64-encoded), or an
/// error when `text` exceeds [`OSC52_MAX_BYTES`]. The `c` selects the
/// clipboard (vs the primary selection); `\x1b]52;c;{b64}\x07` is the
/// BEL-terminated form terminals and tmux (`set-clipboard on`) understand.
/// Pure, so it's unit-tested. See `docs/copy.md`.
fn osc52_sequence(text: &str) -> Result<String, String> {
    if text.len() > OSC52_MAX_BYTES {
        return Err(format!(
            "too large for the escape ({} bytes, max {OSC52_MAX_BYTES})",
            text.len()
        ));
    }
    Ok(format!("\x1b]52;c;{}\x07", base64_encode(text.as_bytes())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// An environment holding exactly `vars`.
    fn env_with(vars: &[(&str, &str)]) -> CopyEnvironment {
        CopyEnvironment::from_lookup(|name| {
            vars.iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| OsString::from(value))
        })
    }

    /// One effect a copy ran, with what it was handed.
    #[derive(Debug, Clone, PartialEq, Eq)]
    enum Call {
        Native(String),
        Tmux(String),
        Write(String),
    }

    /// Run a copy of `text` in `env`, each effect answering as told, and
    /// return what the copy came to and every effect it ran, in order.
    fn run_copy(
        env: CopyEnvironment,
        text: &str,
        native: Result<(), &str>,
        tmux: Result<(), &str>,
        write: Result<(), &str>,
    ) -> (Result<Option<&'static str>, String>, Vec<Call>) {
        let calls = RefCell::new(Vec::new());
        let result = copy_with(
            copy_plan(env),
            text,
            |text| {
                calls.borrow_mut().push(Call::Native(text.to_string()));
                native.map(|()| Some("lease")).map_err(str::to_string)
            },
            |text| {
                calls.borrow_mut().push(Call::Tmux(text.to_string()));
                tmux.map_err(str::to_string)
            },
            |escape| {
                calls.borrow_mut().push(Call::Write(escape.to_string()));
                write.map_err(str::to_string)
            },
        );
        (result, calls.into_inner())
    }

    fn escape(text: &str, frame: EscapeFrame) -> String {
        osc52_escape(text, frame).expect("small enough for an escape")
    }

    const SSH: (&str, &str) = ("SSH_CONNECTION", "198.51.100.7 52144 203.0.113.10 22");
    const TMUX: (&str, &str) = ("TMUX", "/tmp/tmux-1000/default,4242,0");
    const STY: (&str, &str) = ("STY", "4242.pts-0.host");

    // ===== reading the environment =====

    #[test]
    fn a_desktop_session_names_no_multiplexer_and_no_ssh_login() {
        assert_eq!(env_with(&[]), CopyEnvironment::default());
        assert_eq!(
            env_with(&[("DISPLAY", ":0"), ("TERM", "xterm-256color")]),
            CopyEnvironment::default(),
            "only the variables a copy is routed on count"
        );
    }

    #[test]
    fn tmux_is_named_by_its_socket_or_by_its_pane() {
        for var in [TMUX, ("TMUX_PANE", "%3")] {
            assert_eq!(
                env_with(&[var]),
                CopyEnvironment {
                    tmux: true,
                    ..CopyEnvironment::default()
                },
                "{} alone",
                var.0
            );
        }
    }

    #[test]
    fn screen_is_named_by_its_session() {
        assert_eq!(
            env_with(&[STY]),
            CopyEnvironment {
                screen: true,
                ..CopyEnvironment::default()
            }
        );
    }

    #[test]
    fn any_of_the_three_ssh_variables_names_an_ssh_login() {
        for var in [
            SSH,
            ("SSH_CLIENT", "198.51.100.7 52144 22"),
            ("SSH_TTY", "/dev/pts/3"),
        ] {
            assert_eq!(
                env_with(&[var]),
                CopyEnvironment {
                    ssh: true,
                    ..CopyEnvironment::default()
                },
                "{} alone",
                var.0
            );
        }
    }

    #[test]
    fn a_variable_set_but_empty_names_nothing() {
        let empty = [
            ("TMUX", ""),
            ("TMUX_PANE", ""),
            ("STY", ""),
            ("SSH_CONNECTION", ""),
            ("SSH_CLIENT", ""),
            ("SSH_TTY", ""),
        ];
        assert_eq!(env_with(&empty), CopyEnvironment::default());
    }

    // ===== on a desktop: the native clipboard, the terminal its fallback =====

    #[test]
    fn on_a_desktop_a_native_copy_is_the_whole_copy() {
        let (result, calls) = run_copy(env_with(&[]), "hi", Ok(()), Ok(()), Ok(()));
        assert_eq!(result, Ok(Some("lease")));
        assert_eq!(calls, [Call::Native("hi".into())]);
    }

    #[test]
    fn on_a_desktop_the_bare_escape_is_the_fallback() {
        let (result, calls) = run_copy(env_with(&[]), "hi", Err("no display"), Ok(()), Ok(()));
        assert_eq!(result, Ok(None), "the terminal took it: nothing to hold");
        assert_eq!(
            calls,
            [
                Call::Native("hi".into()),
                Call::Write(escape("hi", EscapeFrame::Bare)),
            ]
        );
    }

    #[test]
    fn when_every_route_fails_each_cause_is_named_in_order() {
        let (result, _) = run_copy(
            env_with(&[]),
            "hi",
            Err("clipboard unavailable: no display"),
            Ok(()),
            Err("could not write to /dev/tty: gone"),
        );
        assert_eq!(
            result,
            Err(
                "clipboard unavailable: no display; OSC 52: could not write to /dev/tty: gone"
                    .into()
            )
        );
    }

    #[test]
    fn a_copy_too_large_for_an_escape_is_refused_without_writing_one() {
        let big = "a".repeat(OSC52_MAX_BYTES + 1);
        let (result, calls) = run_copy(env_with(&[]), &big, Err("no display"), Ok(()), Ok(()));
        let reason = result.expect_err("nothing could take it");
        assert!(reason.contains("too large"), "{reason}");
        assert!(
            !calls.iter().any(|call| matches!(call, Call::Write(_))),
            "nothing is written: a terminal would drop a truncated escape"
        );
    }

    // ===== over SSH: the terminal too, whatever the native clipboard did =====

    #[test]
    fn over_ssh_the_escape_goes_out_beside_a_native_copy() {
        // The machine's own clipboard may be a server's nobody pastes from:
        // the user's terminal gets the text either way.
        let (result, calls) = run_copy(env_with(&[SSH]), "hi", Ok(()), Ok(()), Ok(()));
        assert_eq!(result, Ok(Some("lease")), "the native hold is still kept");
        assert_eq!(
            calls,
            [
                Call::Native("hi".into()),
                Call::Write(escape("hi", EscapeFrame::Bare)),
            ]
        );
    }

    #[test]
    fn a_failed_escape_does_not_undo_a_native_copy() {
        let (result, _) = run_copy(env_with(&[SSH]), "hi", Ok(()), Ok(()), Err("no tty"));
        assert_eq!(result, Ok(Some("lease")));
    }

    #[test]
    fn over_ssh_without_a_native_clipboard_the_escape_is_the_copy() {
        let (result, calls) = run_copy(env_with(&[SSH]), "hi", Err("no display"), Ok(()), Ok(()));
        assert_eq!(result, Ok(None));
        assert_eq!(
            calls.last(),
            Some(&Call::Write(escape("hi", EscapeFrame::Bare)))
        );
    }

    // ===== inside tmux: tmux's own buffer =====

    #[test]
    fn inside_tmux_the_text_goes_to_tmux_beside_a_native_copy() {
        // A tmux session can be attached from another terminal than the one
        // that started it: tmux sends the text on to whichever is attached.
        let (result, calls) = run_copy(env_with(&[TMUX]), "hi", Ok(()), Ok(()), Ok(()));
        assert_eq!(result, Ok(Some("lease")));
        assert_eq!(calls, [Call::Native("hi".into()), Call::Tmux("hi".into())]);
    }

    #[test]
    fn on_a_remote_host_inside_tmux_tmux_takes_the_copy_and_no_escape_is_written() {
        // The reported setup: no clipboard on the VPS, tmux's defaults. An
        // escape written there would be dropped, and tmux already sent its
        // own on to the terminal.
        let (result, calls) = run_copy(
            env_with(&[TMUX, SSH]),
            "hi",
            Err("no display"),
            Ok(()),
            Ok(()),
        );
        assert_eq!(result, Ok(None));
        assert_eq!(calls, [Call::Native("hi".into()), Call::Tmux("hi".into())]);
    }

    #[test]
    fn when_tmux_cannot_take_it_the_escape_framed_for_tmux_is_written() {
        // A tmux too old for `load-buffer -w`, or no `tmux` to run.
        let (result, calls) = run_copy(
            env_with(&[TMUX]),
            "hi",
            Err("no display"),
            Err("unknown flag -w"),
            Ok(()),
        );
        assert_eq!(result, Ok(None));
        assert_eq!(
            calls,
            [
                Call::Native("hi".into()),
                Call::Tmux("hi".into()),
                Call::Write(escape("hi", EscapeFrame::Tmux)),
            ]
        );
    }

    #[test]
    fn inside_tmux_every_failure_is_named_in_order() {
        let (result, _) = run_copy(
            env_with(&[TMUX]),
            "hi",
            Err("clipboard unavailable: no display"),
            Err("could not run tmux: not found"),
            Err("could not write to /dev/tty: gone"),
        );
        assert_eq!(
            result,
            Err(
                "clipboard unavailable: no display; tmux: could not run tmux: not found; \
                 OSC 52: could not write to /dev/tty: gone"
                    .into()
            )
        );
    }

    #[test]
    fn a_copy_too_large_for_an_escape_still_goes_through_tmux() {
        // tmux keeps the whole text in its buffer, whatever the terminal
        // makes of tmux's own escape: a long `/export` is not refused there.
        let big = "a".repeat(OSC52_MAX_BYTES + 1);
        let (result, calls) = run_copy(env_with(&[TMUX]), &big, Err("no display"), Ok(()), Ok(()));
        assert_eq!(result, Ok(None));
        assert_eq!(calls.last(), Some(&Call::Tmux(big)));
    }

    #[test]
    fn tmux_wins_over_screen_and_ssh_when_all_are_named() {
        // Whatever else is true, a process inside tmux writes to tmux's pane.
        let (_, calls) = run_copy(
            env_with(&[TMUX, STY, SSH]),
            "hi",
            Err("no display"),
            Ok(()),
            Ok(()),
        );
        assert_eq!(calls.last(), Some(&Call::Tmux("hi".into())));
    }

    // ===== inside GNU screen =====

    #[test]
    fn inside_screen_the_escape_is_framed_for_screen_beside_a_native_copy() {
        let (result, calls) = run_copy(env_with(&[STY]), "hi", Ok(()), Ok(()), Ok(()));
        assert_eq!(result, Ok(Some("lease")));
        assert_eq!(
            calls,
            [
                Call::Native("hi".into()),
                Call::Write(escape("hi", EscapeFrame::Screen)),
            ]
        );
    }

    // ===== the escape's framing =====

    #[test]
    fn the_bare_frame_is_the_escape_itself() {
        assert_eq!(escape("hello", EscapeFrame::Bare), "\x1b]52;c;aGVsbG8=\x07");
    }

    #[test]
    fn the_tmux_frame_wraps_the_escape_in_passthrough_then_repeats_it_bare() {
        assert_eq!(
            escape("hello", EscapeFrame::Tmux),
            "\x1bPtmux;\x1b\x1b]52;c;aGVsbG8=\x07\x1b\\\x1b]52;c;aGVsbG8=\x07"
        );
    }

    #[test]
    fn a_short_escape_is_one_piece_of_screen_passthrough() {
        assert_eq!(
            escape("hello", EscapeFrame::Screen),
            "\x1bP\x1b]52;c;aGVsbG8=\x07\x1b\\"
        );
    }

    #[test]
    fn a_long_escape_is_cut_into_pieces_screen_can_hold() {
        let text = "x".repeat(1000);
        let bare = escape(&text, EscapeFrame::Bare);
        let framed = escape(&text, EscapeFrame::Screen);
        let pieces: Vec<&str> = framed.split_inclusive("\x1b\\").collect();
        assert!(pieces.len() > 1, "1000 bytes take several pieces");
        let mut joined = String::new();
        for piece in &pieces {
            assert!(piece.len() <= 256, "a piece of {} bytes", piece.len());
            let inner = piece
                .strip_prefix("\x1bP")
                .and_then(|piece| piece.strip_suffix("\x1b\\"))
                .unwrap_or_else(|| panic!("an unframed piece: {piece:?}"));
            joined.push_str(inner);
        }
        assert_eq!(joined, bare, "the pieces put back together are the escape");
    }

    #[test]
    fn every_frame_refuses_a_text_too_large_for_an_escape() {
        let big = "a".repeat(OSC52_MAX_BYTES + 1);
        for frame in [EscapeFrame::Bare, EscapeFrame::Tmux, EscapeFrame::Screen] {
            assert!(osc52_escape(&big, frame).is_err(), "{frame:?}");
        }
    }

    #[test]
    fn osc52_sequence_frames_the_base64_in_the_set_clipboard_escape() {
        assert_eq!(osc52_sequence("foobar").unwrap(), "\x1b]52;c;Zm9vYmFy\x07");
    }

    #[test]
    fn osc52_sequence_rejects_oversized_input() {
        let big = "a".repeat(OSC52_MAX_BYTES + 1);
        assert!(osc52_sequence(&big).is_err());
    }

    #[test]
    fn osc52_sequence_allows_input_at_the_cap() {
        let at_cap = "a".repeat(OSC52_MAX_BYTES);
        assert!(osc52_sequence(&at_cap).is_ok());
    }

    #[test]
    fn the_tmux_route_asks_tmux_to_send_the_buffer_on_to_the_terminal() {
        // `-w` is the whole fix: without it tmux keeps the text in its own
        // buffer and the user's clipboard never hears of it.
        assert_eq!(TMUX_LOAD_BUFFER_ARGS, ["load-buffer", "-w", "-"]);
    }

    // ===== the tmux runner, over stand-in commands =====

    #[cfg(unix)]
    #[test]
    fn the_runner_feeds_the_input_and_reports_success() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("got");
        let out_arg = out.to_str().unwrap();
        run_with_input(
            "sh",
            &["-c", "cat > \"$0\"", out_arg],
            b"the copied text",
            Duration::from_secs(5),
        )
        .expect("a zero exit");
        assert_eq!(std::fs::read(&out).unwrap(), b"the copied text");
    }

    #[cfg(unix)]
    #[test]
    fn the_runner_reports_what_a_failing_command_said() {
        let err = run_with_input(
            "sh",
            &["-c", "echo 'unknown flag -w' >&2; exit 1"],
            b"",
            Duration::from_secs(5),
        )
        .expect_err("a non-zero exit");
        assert!(err.contains("unknown flag -w"), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn a_failure_with_nothing_on_stderr_says_how_it_ended() {
        let err = run_with_input("sh", &["-c", "exit 3"], b"", Duration::from_secs(5))
            .expect_err("a non-zero exit");
        assert!(err.contains('3'), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn a_command_that_never_finishes_is_killed_at_the_timeout() {
        // More input than a pipe holds, to a program that never reads it:
        // neither the write nor the wait may hold the caller past the bound.
        let input = vec![b'x'; 1 << 20];
        let started = Instant::now();
        let err = run_with_input("sleep", &["30"], &input, Duration::from_millis(300))
            .expect_err("killed");
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "took {:?}",
            started.elapsed()
        );
        assert!(err.contains("did not finish"), "{err}");
    }

    #[test]
    fn a_missing_program_is_an_error() {
        let err = run_with_input(
            "alter-zero-no-such-program",
            &[],
            b"",
            Duration::from_secs(5),
        )
        .expect_err("nothing to run");
        assert!(err.contains("could not run"), "{err}");
    }
}
