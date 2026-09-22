//! The `/export` page at the boundary: writing the conversation where the
//! picked target says (`docs/export.md`).
//!
//! The pure side already decided *what* is exported — the transcript on
//! screen, rendered by [`ui::export_text`] at the terminal's width — and
//! *where* — the [`ExportTarget`] riding [`Action::Export`]. What has to
//! *happen* lives here: the clipboard write (`/copy`'s own path, the
//! `/donate` page's `copy_donation_address` pattern), or the file write —
//! `conversation-YYYY-MM-DD-HHMMSS.txt` in the working directory, named
//! off the local clock (the rollout's own rule) and never over a file
//! already there — and the confirming toast either way.
//!
//! [`Action::Export`]: alter_zero::app::Action::Export

use std::io;
use std::path::{Path, PathBuf};

use alter_zero::app::{ExportTarget, ToastKind, export_file_name};
use alter_zero::{clipboard, ui};

use super::Session;

/// The toast confirming a clipboard export — worded for what was copied,
/// `/copy`'s `Copied last message to clipboard` sibling.
pub(crate) const EXPORT_COPIED_NOTICE: &str = "Copied the conversation to clipboard";

/// How many taken names a file export steps past before giving up — a
/// bound on the loop, not a limit anyone reaches: two exports inside one
/// second take the plain name and `-2`.
const NAME_RETRIES: u32 = 100;

impl Session<'_> {
    /// Enter or a digit on the `/export` page picked `target`: render the
    /// transcript on screen as plain text at the terminal's width and write
    /// it there, then say what happened. The page is already closed (the
    /// pick is the whole point of it), so the toast lands over the composer.
    pub(crate) fn export_conversation(&mut self, target: ExportTarget) {
        let text = ui::export_text(&self.app, self.term.screen().width);
        match target {
            ExportTarget::Clipboard => match clipboard::copy_to_clipboard(&text) {
                Ok(lease) => {
                    // Hold the native selection alive for the app's lifetime
                    // (Linux); None over OSC 52 — `/copy`'s rule.
                    self.clipboard_lease = lease;
                    self.toast(EXPORT_COPIED_NOTICE, ToastKind::Info);
                }
                Err(reason) => self.toast(format!("Copy failed: {reason}"), ToastKind::Error),
            },
            ExportTarget::File => match write_export_file(&self.cwd, &text) {
                Ok(path) => {
                    let name = path.file_name().map_or_else(
                        || path.display().to_string(),
                        |n| n.to_string_lossy().into_owned(),
                    );
                    self.toast(format!("Saved the conversation to {name}"), ToastKind::Info);
                }
                Err(reason) => self.toast(format!("Export failed: {reason}"), ToastKind::Error),
            },
        }
    }
}

/// Write `text` to a fresh `conversation-YYYY-MM-DD-HHMMSS.txt` under `dir`,
/// stamped off the local clock, and return its path. The file is created
/// with `create_new`, so an export never overwrites one already there: a
/// name that exists steps to `-2`, `-3`, … ([`export_file_name`]'s `dup`)
/// and only a directory that refuses every name is an error.
fn write_export_file(dir: &Path, text: &str) -> io::Result<PathBuf> {
    use chrono::{Datelike, Timelike};
    let now = chrono::Local::now();
    let date = (now.year(), now.month(), now.day());
    let time = (now.hour(), now.minute(), now.second());
    for dup in 0..NAME_RETRIES {
        let path = dir.join(export_file_name(date, time, dup));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut file) => {
                use std::io::Write;
                file.write_all(text.as_bytes())?;
                file.flush()?;
                return Ok(path);
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "every export file name is taken",
    ))
}
