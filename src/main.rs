//! Thin terminal shell around the [`alter_zero`] library.
//!
//! This binary does four things and delegates the rest: run the detached-exec
//! hook, resolve the CLI arguments, open the inline viewport, and run the event
//! loop — or, with `-p`, run the same session headless, with no viewport at all
//! (`docs/cli/headless.md`). Everything it drives lives in [`tui`] — one module per area, the way
//! `app/` and `ui/` are split (see `docs/architecture/module-layout.md`) — and the real logic
//! lives in the library, in pure functions the unit tests can reach.
//!
//! The order below is not arbitrary. Two rules govern it:
//!
//! **The detached-exec hook must stay the first statement of [`main`].** When
//! this process was spawned as `{exe} __alter-zero-detached-exec {cmd}` it is a
//! shell runner's child, not a TUI: it has to `setsid` away from the controlling
//! terminal and `exec` `sh` before it ever touches stdin/stdout or spawns a
//! thread — the tokio runtime and the cursor query included — or it would boot a
//! TUI into the caller's pipes and the tty detach would silently break
//! (`docs/tools/tty-detach.md`).
//!
//! **Invariant 1 (stdin):** [`InlineViewport::init`] queries the cursor position
//! over stdin *once, synchronously*, before any `EventStream` exists (any other
//! question for the terminal rides in that read) — so the loop's `EventStream`
//! is then the **sole** stdin reader; the reply backend's thread only *sends* on
//! its channel. A second stdin reader would steal the cursor-position (DSR)
//! reply — the source of the "cursor position could not be read" error.
//!
//! [`InlineViewport::init`]: alter_zero::term::InlineViewport::init

use std::io;

use alter_zero::cli;
use alter_zero::term::InlineViewport;
use alter_zero::ui;

mod tui;

use tui::startup::{Startup, resolve_cli};

fn main() -> io::Result<()> {
    // The detached-exec helper hook FIRST (see the module doc): a helper re-exec
    // never returns from here, so nothing above it may touch the terminal.
    alter_zero::subprocess::run_detached_exec_if_requested();
    // --continue/--resume resolve to a rollout *path* here — and a quoted
    // [PROMPT] rides along to become the first turn — before the tokio
    // runtime and the terminal boot (docs/cli/cli.md): --help/--version and every
    // resolution failure print to normal cooked-mode stdio and exit — no TUI
    // flash, no raw mode to restore.
    let startup = match resolve_cli() {
        Ok(startup) => startup,
        Err(code) => std::process::exit(code),
    };
    // `-p`: the same session with no screen (docs/cli/headless.md) — no viewport,
    // no raw mode, no cursor query, no key reader; its exit status is the
    // run's. Every line it wrote was flushed as it went.
    if startup.print.is_some() {
        std::process::exit(headless_main(startup));
    }
    tui_main(startup)
}

#[tokio::main(flavor = "current_thread")]
async fn headless_main(startup: Startup) -> i32 {
    tui::headless::run(startup).await
}

#[tokio::main(flavor = "current_thread")]
async fn tui_main(startup: Startup) -> io::Result<()> {
    // No terminal behind us (cron, CI, a pipe), or one that never answered
    // the cursor query: say so and name print mode rather than end on the
    // io::Error's Debug form. init unwinds the modes it set before it fails.
    let mut term = match InlineViewport::init(ui::LIVE_MIN_HEIGHT) {
        Ok(term) => term,
        Err(err) => {
            let arg0 = std::env::args().next();
            eprintln!(
                "{}",
                cli::no_terminal_error(&cli::bin_name(arg0.as_deref()), &err.to_string())
            );
            std::process::exit(1);
        }
    };
    let result = tui::event_loop::run(&mut term, startup).await;
    // Always restore the terminal (raw mode off, cursor below the box), even if
    // the loop bailed out with an I/O error — then surface the first error.
    let restored = term.restore();
    // The exit hint (docs/cli/cli.md): printed AFTER restore so its two lines land
    // below the box in normal terminal flow — only when the session recorded a
    // conversation (`run` returns the active rollout's id then), echoing the
    // bin name the user actually invoked (`argv[0]`'s basename).
    if let Ok(Some(session_id)) = &result {
        let arg0 = std::env::args().next();
        println!(
            "\n{}",
            cli::resume_hint(&cli::bin_name(arg0.as_deref()), session_id)
        );
    }
    result.map(|_| ()).and(restored)
}
