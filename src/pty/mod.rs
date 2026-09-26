//! Pseudo-terminal sessions for the model's interactive commands — `bash`
//! with `tty: true` and the `bash_session` tool (`docs/interactive-shell.md`).
//!
//! The pure halves:
//!
//! - [`keys`] — the `input` notation (`<Enter>`, `<C-c>`, `<Up>`) and the
//!   bytes a terminal sends for it.
//! - [`charset`] — the DEC line-drawing set a box is drawn in without a
//!   UTF-8 locale, shown as the box characters it stands for.
//! - [`transcript`] — the output as lines of text: carriage returns and
//!   erases applied, escapes dropped, and the model's "since your last look".
//! - [`fold`] — a plain (piped) command's output folded the same way, one
//!   line at a time, so a `\r` progress bar reaches the model once.
//! - [`screen`] — the output as a screen (the `vt100` emulator): what a
//!   full-screen program shows, the cursor-key mode, and the replies to the
//!   terminal queries programs send.
//! - [`probe`] — what the session's processes are blocked in, from
//!   Linux's `/proc`: a read on the terminal is a program waiting for
//!   input, a tree that is all at work is busy whatever its screen shows,
//!   and an event loop whose epoll instances watch no terminal asks nothing.
//! - [`settle`] — when a call waiting on a session returns.
//! - [`report`] — what a session call tells the model: the frame line over
//!   the new output or the screen.
//! - [`session`] — what a session's threads share: both views, the waiting
//!   call's loop, and who reports the exit.
//!
//! The process side is boundary code: [`spawn`] allocates the terminal and
//! starts the command as the session leader whose controlling terminal it is,
//! and [`crate::background`]'s TTY tasks keep it running.

pub mod charset;
pub mod fold;
pub mod keys;
pub mod probe;
pub mod report;
pub mod screen;
pub mod session;
pub mod settle;
pub mod spawn;
pub mod transcript;
