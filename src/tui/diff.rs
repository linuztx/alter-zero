//! Off-thread Git reads and the alternate-screen review lifecycle.

use std::io;

use alter_zero::app::{ToastKind, View};
use alter_zero::git_diff::DiffSnapshot;
use alter_zero::ui;

use super::Session;

pub(crate) type DiffResult = (u64, DiffUpdate);

pub(crate) enum DiffUpdate {
    Repository(Result<(), String>),
    Snapshot(Result<DiffSnapshot, String>),
}

impl Session<'_> {
    pub(crate) fn open_diff_review(&mut self) {
        // Stay inline during validation. An unavailable repository should
        // produce a quiet toast without ever flashing the alternate screen.
        self.diff_generation = self.diff_generation.wrapping_add(1);
        let generation = self.diff_generation;
        let tx = self.diff_tx.clone();
        let cwd = self.cwd.clone();
        std::thread::spawn(move || {
            let result = super::git_diff_loader::repository_root(&cwd).map(|_| ());
            let _ = tx.send((generation, DiffUpdate::Repository(result)));
        });
    }

    pub(crate) fn load_diff_review(&mut self) {
        self.diff_generation = self.diff_generation.wrapping_add(1);
        let generation = self.diff_generation;
        let tx = self.diff_tx.clone();
        let cwd = self.cwd.clone();
        std::thread::spawn(move || {
            let result = super::git_diff_loader::load(&cwd);
            let _ = tx.send((generation, DiffUpdate::Snapshot(result)));
        });
    }

    pub(crate) fn on_diff_result(&mut self, (generation, update): DiffResult) -> io::Result<()> {
        if generation != self.diff_generation {
            return Ok(());
        }
        let result = match update {
            DiffUpdate::Repository(result) => {
                if self.app.view != View::Conversation || self.app.modal_open() {
                    return Ok(());
                }
                match result {
                    Ok(()) => {
                        self.app.open_diff_review();
                        self.term.enter_overlay()?;
                        self.load_diff_review();
                        self.draw_diff_review()?;
                    }
                    Err(error) => self.toast(error, ToastKind::Info),
                }
                self.frame.schedule_frame();
                return Ok(());
            }
            DiffUpdate::Snapshot(result) => result,
        };
        if self.app.view != View::DiffReview {
            return Ok(());
        }
        match result {
            Ok(snapshot) => self.app.set_diff_snapshot(snapshot),
            Err(error) => {
                if self
                    .app
                    .diff_review()
                    .is_some_and(|review| review.snapshot.is_some())
                {
                    self.app.set_diff_error(error);
                } else {
                    self.app.close_diff_review();
                    self.toast(error, ToastKind::Error);
                    self.close_diff_review()?;
                }
            }
        }
        self.frame.schedule_frame();
        Ok(())
    }

    pub(crate) fn close_diff_review(&mut self) -> io::Result<()> {
        self.diff_generation = self.diff_generation.wrapping_add(1);
        self.term.exit_overlay()?;
        self.overlay_return_repaint()
    }

    pub(crate) fn draw_diff_review(&mut self) -> io::Result<()> {
        self.app
            .settle_diff_scroll(ui::diff_body_rows(self.term.screen()));
        let app = &self.app;
        self.term
            .draw_overlay(|area, buf| ui::render_diff_view(app, area, buf))
    }
}
