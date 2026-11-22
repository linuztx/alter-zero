//! Plugins at the boundary (`docs/plugins.md`): reading what the enabled
//! plugins add to the session, reading what the `/plugins` page shows,
//! running its operations on a worker, and taking a change into the running
//! session.
//!
//! The pure side owns the page and its rounds ([`App::open_plugins_menu`],
//! [`App::finish_plugin_job`]); the store's operations
//! ([`PluginStore`]) are pure of the session too. What this module adds is
//! what neither can do: the reads the page shows — the catalog as it opens
//! and as a job lands, a plugin's contents as the page shows it, all on the
//! loop and only while it is open, so nothing about plugins is read at
//! startup or kept once it closes — the thread an install clones on, the
//! token Esc stops it with, the clock the page counts it on — and, after
//! anything that changes what is installed or on, the reload that hands the
//! session's skills, agents, hooks and MCP servers the new set, asking the
//! review about whatever runs and has not been looked at yet.
//!
//! [`App::open_plugins_menu`]: alter_zero::app::App::open_plugins_menu
//! [`App::finish_plugin_job`]: alter_zero::app::App::finish_plugin_job

use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, Instant};

use alter_zero::app::{PLUGINS_JOB_DELAY, PluginDone, PluginOp, ToastKind};
use alter_zero::llm::mcp::McpHold;
use alter_zero::mcp::{McpScope, McpServerEntry};
use alter_zero::plugins::{
    FoundMarketplace, LoadedPlugins, PluginCatalog, PluginStore, build_catalog, find_marketplaces,
    load_enabled, parse_marketplace_source, read_contents, with_project_dir,
};
use alter_zero::stream::CancelToken;

use super::Session;
use super::config;
use super::host;
use super::trust::TrustState;

/// A finished operation: the round it was asked under, what it did — and
/// whether it may have changed what the session loads, so the reload runs
/// even when the page that asked has closed. No catalog: what the page
/// shows next is read on the loop as this lands, never on the worker, whose
/// thread's heap would keep what the read once held (`docs/memory.md`).
#[derive(Debug)]
pub(crate) struct PluginResult {
    round: u64,
    outcome: Result<PluginDone, String>,
    reload: bool,
}

/// What the page says with no config home to keep plugins in.
const NO_STORE: &str = "No config home — set HOME or ALTER_ZERO_CONFIG_DIR to keep plugins";

/// How often the page is redrawn while a job runs, so its clock moves with
/// no other event.
const PLUGIN_JOB_TICK: Duration = Duration::from_secs(1);

/// The plugins on in `project`, read from `store` — none when there is no
/// store or `ALTER_ZERO_PLUGINS` turned them off for the run.
pub(crate) fn load_plugins(store: Option<&PluginStore>, project: &Path) -> LoadedPlugins {
    match store {
        Some(store) if config::plugins_enabled() => {
            load_enabled(store, &project.display().to_string())
        }
        _ => LoadedPlugins::default(),
    }
}

/// The marketplaces found for `project` without being added — the home
/// folder's and the project's `.agents/plugins/marketplace.json`
/// (`docs/agents-dir.md`). Read only while the page is open or a job runs,
/// never at startup: the catalogs are the page's to read.
pub(crate) fn found_marketplaces(project: &Path) -> Vec<FoundMarketplace> {
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    find_marketplaces(project, home.as_deref())
}

/// The enabled plugins' MCP servers as the manager takes them — each in the
/// plugin scope, shown under its plugin's MCP file — and the ones the review
/// holds: a server never decided on, or changed since, waits for it; one the
/// user turned down stays off.
pub(crate) fn plugin_mcp_entries(
    trust: &TrustState,
    loaded: &LoadedPlugins,
    project: &Path,
) -> (Vec<McpServerEntry>, BTreeMap<String, McpHold>) {
    let reviews = trust.server_reviews();
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    let mut entries = Vec::new();
    let mut held = BTreeMap::new();
    for server in loaded.servers() {
        let source = server.source.display().to_string();
        let name = server.server.name;
        let review = reviews
            .iter()
            .find(|review| review.source == source && review.name == name);
        match review.map(|review| &review.trust) {
            Some(trust) if trust.approved() => {}
            Some(trust) if !trust.needs_review() => {
                held.insert(name.clone(), McpHold::Declined);
            }
            _ => {
                held.insert(name.clone(), McpHold::Review);
            }
        }
        // `${CLAUDE_PROJECT_DIR}` is the session's directory (Claude Code's
        // rule) — never in an Agent Plugin's entry, which expands nothing
        // but its two plugin variables.
        let config = if server.server.literal {
            server.server.config
        } else {
            with_project_dir(server.server.config, project)
        };
        entries.push(McpServerEntry {
            name,
            config,
            scope: McpScope::Plugin,
            config_path: alter_zero::ui::display_cwd(&server.source, home.as_deref()),
            literal: server.server.literal,
        });
    }
    (entries, held)
}

/// `{n} {noun}`, the noun plural for anything but one.
fn counted(n: usize, noun: &str) -> String {
    if n == 1 {
        format!("{n} {noun}")
    } else {
        format!("{n} {noun}s")
    }
}

/// Run `op` against `store` — on the worker — and say what it did, and
/// whether what the session loads may have changed.
fn run_op(
    store: &PluginStore,
    op: &PluginOp,
    project: &Path,
    cancel: &CancelToken,
) -> (Result<PluginDone, String>, bool) {
    let now = host::unix_secs_signed();
    let done = |message: String| PluginDone {
        message,
        marketplace: None,
    };
    match op {
        PluginOp::Install(id) => {
            // A plugin of a marketplace found and never added adds it first
            // (`docs/agents-dir.md`) — looked for only when no marketplace
            // of its name is added, so an ordinary install reads no other
            // catalog on this thread (`docs/memory.md`).
            let known = store
                .registry()
                .is_ok_and(|registry| registry.marketplaces.contains_key(&id.marketplace));
            let found = if known {
                Vec::new()
            } else {
                found_marketplaces(project)
            };
            let result = store
                .install_found(id, &found, cancel, now)
                .map(|(installed, added)| {
                    let verb = if installed.replaced {
                        "Updated"
                    } else {
                        "Installed"
                    };
                    match added {
                        Some(added) => done(format!("{verb} {} — added {}", id.name, added.name)),
                        None => done(format!("{verb} {}", id.name)),
                    }
                });
            let ok = result.is_ok();
            (result, ok)
        }
        PluginOp::Uninstall(id) => {
            let result = store
                .uninstall(id)
                .map(|()| done(format!("Uninstalled {}", id.name)));
            let ok = result.is_ok();
            (result, ok)
        }
        PluginOp::SetEnabled { id, enabled } => {
            let result = store
                .set_enabled(&project.display().to_string(), id, *enabled)
                .map(|()| {
                    let state = if *enabled { "Enabled" } else { "Disabled" };
                    done(format!("{state} {} in this directory", id.name))
                });
            let ok = result.is_ok();
            (result, ok)
        }
        PluginOp::AddMarketplace(text) => {
            let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
            let result = parse_marketplace_source(text, project, home.as_deref())
                .and_then(|source| store.add_marketplace(&source, cancel, now))
                .map(|added| PluginDone {
                    message: if added.already {
                        format!("{} is already added — read again", added.name)
                    } else {
                        format!(
                            "Added {} — {}",
                            added.name,
                            counted(added.plugins, "plugin")
                        )
                    },
                    marketplace: Some(added.name),
                });
            (result, false)
        }
        PluginOp::UpdateMarketplace(name) => {
            let result = store.update_marketplace(name, cancel, now).map(|updated| {
                match updated.updates.len() {
                    0 if updated.changed => done(format!("Updated {}", updated.name)),
                    0 => done(format!("{} is up to date", updated.name)),
                    n => done(format!(
                        "Updated {} — {} to update: {}",
                        updated.name,
                        counted(n, "plugin"),
                        updated.updates.join(", ")
                    )),
                }
            });
            (result, false)
        }
        PluginOp::RemoveMarketplace(name) => {
            let result = store.remove_marketplace(name).map(|removed| {
                if removed.is_empty() {
                    done(format!("Removed {name}"))
                } else {
                    done(format!(
                        "Removed {name} and uninstalled {}",
                        counted(removed.len(), "plugin")
                    ))
                }
            });
            let ok = result.is_ok();
            (result, ok)
        }
    }
}

impl Session<'_> {
    /// `/plugins`: read the catalog on the loop and open the page on it, so
    /// its first frame has its rows. A few milliseconds even over the two
    /// official catalogs: the read takes what the rows need and leaves what
    /// a plugin adds until the page shows it (`read_shown_plugin`).
    pub(crate) fn open_plugins_menu(&mut self) {
        self.cancel_plugin_job();
        self.plugin_job_started = None;
        let catalog = self.read_plugin_catalog();
        self.app
            .open_plugins_menu(config::plugins_enabled(), catalog);
    }

    /// The catalog as it is on disk now — or, with no config home, one that
    /// says why there is none.
    fn read_plugin_catalog(&self) -> PluginCatalog {
        match &self.plugin_store {
            Some(store) => build_catalog(
                store,
                &self.cwd.display().to_string(),
                &found_marketplaces(&self.cwd),
            ),
            None => PluginCatalog {
                error: Some(NO_STORE.to_string()),
                ..PluginCatalog::default()
            },
        }
    }

    /// What the plugin the page shows adds, read before the frame that
    /// shows it — the highlighted row's facts, a plugin's own page — when
    /// the catalog left it to be read: one plugin's files, a fraction of a
    /// millisecond.
    fn read_shown_plugin(&mut self) {
        let Some(mut plugin) = self.app.plugin_to_read().cloned() else {
            return;
        };
        match &self.plugin_store {
            Some(store) => read_contents(store, &mut plugin),
            None => plugin.unread = None,
        }
        self.app.supply_plugin_contents(plugin);
    }

    /// Run the page's operation `op` under `round` on a worker — one at a
    /// time: the page refuses a second while one runs. The worker answers
    /// with what the operation did and nothing it read: the catalog the page
    /// shows next is read on the loop as the answer lands.
    pub(crate) fn run_plugin_op(&mut self, op: PluginOp, round: u64) {
        self.cancel_plugin_job();
        self.plugin_job_started = Some(Instant::now());
        let tx = self.plugin_tx.clone();
        let Some(store) = self.plugin_store.clone() else {
            let _ = tx.send(PluginResult {
                round,
                outcome: Err(NO_STORE.to_string()),
                reload: false,
            });
            return;
        };
        let cancel = CancelToken::new();
        self.plugin_cancel = Some(cancel.clone());
        let project = self.cwd.clone();
        std::thread::spawn(move || {
            let (outcome, reload) = run_op(&store, &op, &project, &cancel);
            let _ = tx.send(PluginResult {
                round,
                outcome,
                reload,
            });
        });
        self.frame.schedule_frame();
    }

    /// Stop the job running — Esc on the page, the page closing, the app
    /// quitting. A clone or a fetch is killed; the worker still answers, so
    /// an open page learns its job ended.
    pub(crate) fn cancel_plugin_job(&mut self) {
        if let Some(cancel) = self.plugin_cancel.take() {
            cancel.cancel();
        }
    }

    /// A job answered: hand it to the page — which takes it only while it is
    /// still the job the page waits on, with the catalog read again here,
    /// whatever the job left on disk, cancelled or not; shows what changed in
    /// its rows and says a failure in red — and take any change into the
    /// session. What a page that has closed did not take is a toast instead,
    /// the one place left to say what it did: never one over the page, which
    /// would move it (docs/plugins.md *No flicker*).
    pub(crate) fn on_plugin_result(&mut self, result: PluginResult) {
        let PluginResult {
            round,
            outcome,
            reload,
        } = result;
        let waited = self.app.plugins_job_round() == Some(round);
        if self
            .app
            .plugins_job_round()
            .is_none_or(|current| current == round)
        {
            self.plugin_cancel = None;
            self.plugin_job_started = None;
        }
        let catalog = waited.then(|| self.read_plugin_catalog());
        let taken = self.app.finish_plugin_job(round, catalog, &outcome);
        if !taken {
            match &outcome {
                Ok(done) if !done.message.is_empty() => {
                    self.toast(done.message.clone(), ToastKind::Info);
                }
                Err(error) => self.toast(error.clone(), ToastKind::Error),
                _ => {}
            }
        }
        if reload {
            self.reload_plugins();
        }
        self.frame.schedule_frame();
    }

    /// The page's clocks, every draw: how long its job has run, and the wall
    /// clock its ages count from — and, while a job runs, a frame the moment
    /// it has run long enough to be said (`PLUGINS_JOB_DELAY`), then one a
    /// second so the count moves on a page nothing else redraws. First, what
    /// the plugin it shows adds, when that is still to be read.
    pub(crate) fn tick_plugins_page(&mut self) {
        if self.app.plugins_menu.is_none() {
            return;
        }
        self.read_shown_plugin();
        let elapsed = self
            .plugin_job_started
            .map_or(Duration::ZERO, |start| start.elapsed());
        self.app
            .set_plugins_clock(elapsed, host::unix_secs_signed());
        if self.app.plugins_job_round().is_some() && !self.app.view.is_overlay() {
            let next = PLUGINS_JOB_DELAY
                .checked_sub(elapsed)
                .filter(|wait| !wait.is_zero())
                .unwrap_or(PLUGIN_JOB_TICK);
            self.frame.schedule_frame_in(next);
        }
    }

    /// The plugins that could not be read, as one red toast — the first,
    /// and how many more (the agent files' rule).
    pub(crate) fn report_plugin_errors(&mut self) {
        let Some(first) = self.plugins.errors.first() else {
            return;
        };
        let more = self.plugins.errors.len() - 1;
        let tail = if more > 0 {
            format!(" (+{more} more)")
        } else {
            String::new()
        };
        self.toast(
            format!("Plugin not loaded: {first}{tail}"),
            ToastKind::Error,
        );
    }

    /// Take what is installed and on now into the running session
    /// (`docs/plugins.md`): the plugins read again; their hooks and servers
    /// handed to the trust layer and from there to the runner and the MCP
    /// manager — a server gone is stopped, one new is held for the review —
    /// their skills and agents re-listed; and, when something that runs
    /// waits for a decision, the review asked at once.
    pub(crate) fn reload_plugins(&mut self) {
        // What already waited for a decision: a hook or server the user has
        // just declined is asked about again at the next launch, not every
        // time another plugin changes.
        let hooks_before = self.trust.untrusted_plugin_hooks();
        let servers_before: std::collections::BTreeSet<String> =
            self.mcp.awaiting_review().into_iter().collect();
        self.plugins = load_plugins(self.plugin_store.as_ref(), &self.cwd);
        // An open page says it on the plugin's own row (`✘ not loaded`): a
        // toast over the page would move it.
        if self.app.plugins_menu.is_none() {
            self.report_plugin_errors();
        }
        self.trust.set_plugins(&self.plugins);
        self.models.set_hooks_file(self.trust.runner_hooks());
        let (entries, held) = plugin_mcp_entries(&self.trust, &self.plugins, &self.cwd);
        let refused = self.mcp.set_plugin_servers(entries, held);
        for name in refused {
            self.toast(
                format!(
                    "The plugin MCP server `{name}` is not started — the name is already in use"
                ),
                ToastKind::Error,
            );
        }
        self.rescan_skills();
        self.rescan_agents();
        self.models.refresh_mcp();
        self.sync_listings();
        self.sync_setting_availability();
        let new_hooks = !self.trust.untrusted_plugin_hooks().is_subset(&hooks_before);
        let new_servers = self
            .mcp
            .awaiting_review()
            .iter()
            .any(|name| !servers_before.contains(name));
        if new_hooks || new_servers {
            let pages = self.review_pages(true, true);
            self.app.open_requested_review(pages);
        }
    }
}
