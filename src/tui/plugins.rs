//! The `/plugins` page's boundary (`docs/plugins-menu.md`) — a **stub**
//! until the plugin library (`src/plugins/`, `docs/plugins.md`) is wired in.
//!
//! The pure page is complete: it shows a [`PluginsSnapshot`] and answers
//! keys with [`PluginOp`]s. What this file owes it is three things, each of
//! which the glue replaces:
//!
//! - **Open** ([`Session::open_plugins_menu`]): read the catalogs *now*,
//!   synchronously — a streamed, typed read of a few milliseconds, so the
//!   first frame already has its rows — and hand them to
//!   `App::open_plugins_menu`.
//!   Nothing about plugins is resident while the page is closed. The stub
//!   offers the two official catalogs and lists nothing.
//! - **Run** ([`Session::run_plugin_op`]): an operation under its round on a
//!   worker, answered with `App::finish_plugin_job(round, snapshot,
//!   outcome)` — the snapshot read again after it — or, for
//!   [`PluginOp::Inspect`] (round `0`, no job), with
//!   `App::supply_plugin_contents`. An answer the page no longer takes
//!   (`finish_plugin_job` returns `false`: it closed, or Esc let go of the
//!   job) is the one case for a toast. The stub answers every operation at
//!   once with a failure saying plugins are not connected yet.
//! - **Stop** ([`Session::cancel_plugin_job`]): Esc on a running job, or the
//!   page closing — kill the git child. The stub runs nothing.

use alter_zero::app::ToastKind;
use alter_zero_plugins::page::{
    MarketplaceSuggestion, PluginContents, PluginDone, PluginOp, PluginsSnapshot,
};

use super::Session;

/// What every operation answers until the plugin library is connected.
const NOT_CONNECTED: &str =
    "Plugins are not connected yet — this build has the /plugins page but not the plugin store";

/// The official catalogs `+ Add` offers — the library's default
/// marketplaces, until it supplies them.
fn official_catalogs() -> Vec<MarketplaceSuggestion> {
    [
        (
            "anthropics/claude-plugins-official",
            "claude-plugins-official",
            "Claude Code's official plugin directory",
        ),
        (
            "openai/plugins",
            "openai-curated",
            "Codex's official plugin directory",
        ),
    ]
    .into_iter()
    .map(|(source, name, description)| MarketplaceSuggestion {
        source: source.to_string(),
        name: name.to_string(),
        description: description.to_string(),
    })
    .collect()
}

impl Session<'_> {
    /// `/plugins`: read the plugin state and open the page over it.
    pub(crate) fn open_plugins_menu(&mut self) {
        let session_enabled = std::env::var("ALTER_ZERO_PLUGINS")
            .map_or(true, |value| !matches!(value.trim(), "0" | "false" | "off"));
        self.app.open_plugins_menu(PluginsSnapshot {
            suggestions: official_catalogs(),
            session_enabled,
            ..PluginsSnapshot::default()
        });
    }

    /// Run `op`, already marked running on the page under `round`.
    pub(crate) fn run_plugin_op(&mut self, op: PluginOp, round: u64) {
        if let PluginOp::Inspect(id) = op {
            self.app
                .supply_plugin_contents(&id, PluginContents::Unreadable(NOT_CONNECTED.into()));
            return;
        }
        let outcome: Result<PluginDone, String> = Err(NOT_CONNECTED.to_string());
        if !self.app.finish_plugin_job(round, None, &outcome) {
            self.toast(NOT_CONNECTED, ToastKind::Error);
        }
    }

    /// Esc on a running job, or the page closed: stop the job. Nothing runs
    /// in the stub.
    pub(crate) fn cancel_plugin_job(&mut self) {}
}
