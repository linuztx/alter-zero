//! The `/plugins` page's shim in `App` (`docs/plugins-menu.md`). The page —
//! its state, keys, jobs and view — lives in the `alter_zero_plugins` crate,
//! so its code costs an idle session nothing (`docs/memory.md`); `App` only
//! holds it while it is open, boxed, and forwards to it. No logic here.

use std::time::Duration;

use alter_zero_plugins::page::{
    KeyOutcome, PluginContents, PluginDone, PluginId, PluginsPage, PluginsSnapshot,
};
use ratatui::crossterm::event::KeyEvent;

use super::{Action, App, Backtrack};

impl App {
    /// Open the page over `snapshot`, which the boundary read just now.
    /// Abandons the composer's bands, like every composer-replacing page.
    pub fn open_plugins_menu(&mut self, snapshot: PluginsSnapshot) {
        self.shortcuts_open = false;
        self.command_menu = None;
        self.file_search = None;
        self.skill_picker = None;
        self.backtrack = Backtrack::default();
        self.plugins_menu = Some(Box::new(PluginsPage::open(snapshot, self.plugin_rounds)));
    }

    /// Close the page, dropping it and its snapshot — keeping only the round
    /// counter, so a reopened page never takes an old job's answer.
    pub fn close_plugins_menu(&mut self) {
        if let Some(page) = self.plugins_menu.take() {
            self.plugin_rounds = page.last_round();
        }
    }

    /// [`PluginsPage::update_snapshot`].
    pub fn update_plugins_snapshot(&mut self, snapshot: PluginsSnapshot) {
        if let Some(page) = self.plugins_menu.as_mut() {
            page.update_snapshot(snapshot);
        }
    }

    /// [`PluginsPage::supply_contents`]; `false` when the page is closed.
    pub fn supply_plugin_contents(&mut self, id: &PluginId, contents: PluginContents) -> bool {
        self.plugins_menu
            .as_mut()
            .is_some_and(|page| page.supply_contents(id, contents))
    }

    /// [`PluginsPage::pending_inspect`].
    #[must_use]
    pub fn pending_plugin_inspect(&self) -> Option<PluginId> {
        self.plugins_menu.as_ref()?.pending_inspect()
    }

    /// [`PluginsPage::set_clock`].
    pub fn set_plugins_clock(&mut self, now: i64, clock: Duration) {
        if let Some(page) = self.plugins_menu.as_mut() {
            page.set_clock(now, clock);
        }
    }

    /// [`PluginsPage::job_round`].
    #[must_use]
    pub fn plugins_job_round(&self) -> Option<u64> {
        self.plugins_menu.as_ref()?.job_round()
    }

    /// Whether the page wants frames of its own — a job runs — and is on
    /// screen (never under an overlay).
    #[must_use]
    pub fn plugins_menu_ticks(&self) -> bool {
        !self.view.is_overlay() && self.plugins_job_round().is_some()
    }

    /// [`PluginsPage::report_step`]; `false` when the page is closed.
    pub fn report_plugin_step(&mut self, round: u64, step: impl Into<String>) -> bool {
        self.plugins_menu
            .as_mut()
            .is_some_and(|page| page.report_step(round, step))
    }

    /// [`PluginsPage::finish_job`]; `false` when the page is closed — the
    /// boundary then toasts the outcome.
    pub fn finish_plugin_job(
        &mut self,
        round: u64,
        snapshot: Option<PluginsSnapshot>,
        outcome: &Result<PluginDone, String>,
    ) -> bool {
        self.plugins_menu
            .as_mut()
            .is_some_and(|page| page.finish_job(round, snapshot, outcome))
    }

    /// [`PluginsPage::paste`].
    pub fn paste_into_plugins(&mut self, pasted: &str) {
        if let Some(page) = self.plugins_menu.as_mut() {
            page.paste(pasted);
        }
    }

    /// Keys while the page is open: Ctrl+O and Ctrl+D open the two read-only
    /// views over it (`/usage`'s rule); every other key is the page's, its
    /// answer wrapped in an [`Action`].
    pub(super) fn on_key_plugins(&mut self, key: KeyEvent) -> Action {
        if let Some(action) = self.on_key_overlay_toggle(key) {
            return action;
        }
        let Some(page) = self.plugins_menu.as_mut() else {
            return Action::None;
        };
        match page.on_key(key) {
            KeyOutcome::None => Action::None,
            KeyOutcome::Close => {
                self.close_plugins_menu();
                Action::ClosePluginsMenu
            }
            KeyOutcome::CancelJob => Action::CancelPluginJob,
            KeyOutcome::Op { op, round } => Action::PluginOp { op, round },
        }
    }
}
