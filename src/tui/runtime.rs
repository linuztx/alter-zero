//! The tokio runtime the interface's event loop runs on.
//!
//! `#[tokio::main]` builds its runtime with `enable_all`, and what "all" means
//! is decided by tokio's feature set for the **whole build**, not by what this
//! crate asks for: `reqwest` turns on `net`, and a headless run's signal
//! handlers need `signal` (`docs/headless.md`). So `enable_all` starts tokio's
//! I/O driver — an epoll instance, a duplicate of it and an eventfd to wake
//! it — and its signal driver — a socket pair, a duplicate, and a watch
//! channel for every signal number the platform has — in a loop that never
//! registers anything with either: the interface selects over channels and
//! timers, and its keys come from crossterm's own reader
//! (`docs/async-rewrite.md`).
//!
//! So the interface names what it uses: six file descriptors and 0.04 MB of
//! anonymous memory an idle start does not hold (`docs/memory.md` *Startup*).
//! The headless run keeps `#[tokio::main]`: it is the one that installs
//! signal handlers, and tokio delivers a signal through its I/O driver.

use std::io;

use tokio::runtime::{Builder, Runtime};

/// The interface's runtime: the current thread, with timers and nothing else
/// — no I/O driver, and so no signal driver either.
///
/// What the loop needs from it is three things, and the tests below hold it
/// to them: a timer (the frame scheduler's `sleep_until`), a wake-up from a
/// thread outside the runtime (every backend and worker sends on a tokio
/// channel) and `tokio::spawn` (the scheduler task).
pub(crate) fn interface() -> io::Result<Runtime> {
    Builder::new_current_thread().enable_time().build()
}

#[cfg(test)]
mod tests {
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::time::Duration;

    use tokio::signal::unix::{SignalKind, signal};

    use super::interface;

    #[test]
    fn the_interface_runtime_serves_what_the_loop_selects_over() {
        // A timer inside a spawned task (the frame scheduler) and a channel a
        // plain thread sends on (the reply backend, every worker).
        let runtime = interface().expect("the runtime builds");
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<u8>();
        let got = runtime.block_on(async move {
            let task = tokio::spawn(async {
                tokio::time::sleep(Duration::from_millis(5)).await;
                7_u8
            });
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(5));
                let _ = tx.send(9);
            });
            (task.await.ok(), rx.recv().await)
        });
        assert_eq!(got, (Some(7), Some(9)));
    }

    #[test]
    fn the_interface_runtime_starts_no_io_or_signal_driver() {
        // tokio delivers signals through its I/O driver, so a runtime that
        // can install a handler is one that started both. This one must not:
        // the interface never uses either, and `enable_all` — what
        // `#[tokio::main]` asks for — would start them for it.
        let runtime = interface().expect("the runtime builds");
        let installed = catch_unwind(AssertUnwindSafe(|| {
            runtime.block_on(async { signal(SignalKind::user_defined1()).is_ok() })
        }));
        assert!(
            !matches!(installed, Ok(true)),
            "the interface's runtime started tokio's I/O and signal drivers"
        );
    }
}
