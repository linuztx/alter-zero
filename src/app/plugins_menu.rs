//! The inline `/plugins` page: browse the marketplaces' plugins, install,
//! enable, disable, update and uninstall them, and manage the marketplaces
//! themselves — all on one composer-replacing page in the `/skills` /
//! `/mcp` family. See `docs/plugins.md`.
//!
//! The page is pure state, like every picker's: what it shows arrives from
//! the boundary as [`PluginEvent`]s folded in by [`App::apply_plugin_event`],
//! and everything it wants done leaves as a [`PluginOp`] inside
//! [`Action::PluginOp`]. Nothing here touches the disk, `git` or the network,
//! so every rule the page keeps — Esc going back exactly one level, a result
//! stated on the page it happened on, a failure flipping an optimistic
//! toggle back — is a unit test over plain data.
//!
//! Three shapes keep it honest:
//!
//! - **Rounds.** Every open hands out a fresh round from a counter kept on
//!   [`App`] rather than on the page (the `/usage` rule), and an answer for
//!   any other round, or for a page that has closed, is dropped — a slow
//!   fetch from a page closed a minute ago never lands on the one open now.
//! - **Rows are derived, never stored.** [`PluginsMenu::rows`] filters the
//!   catalog for the tab and the query each time it is asked, as indices
//!   into the catalog; the highlight is remembered by **identity**
//!   ([`PluginId`], a marketplace's name), so a catalog that refreshes under
//!   the cursor keeps the row the user was on rather than an index that now
//!   names another plugin.
//! - **One foreground job.** An install, update, uninstall or marketplace
//!   change is the page's [`Job`]: it is set in the key press that starts it
//!   (the next frame already shows it running), every key but Esc and
//!   Ctrl+C is swallowed while it runs, and its result becomes the page's
//!   [`Notice`]. Fetching a marketplace is not a job — several can run at
//!   once, each a [`Fetch`] — and neither is enabling or disabling a plugin,
//!   which flips the row at once and flips it back if the write fails.
//!
//! The catalog lives only while the page is open (`docs/plugins.md`
//! *Memory*): closing drops it with the rest of the state.

use std::time::Duration;

use super::views::TOOL_VIEW_PAGE;
use super::*;

use crate::plugins::catalog::{CatalogTab, PluginCatalog, PluginDetail, PluginStatus};
use crate::plugins::id::{LOCAL_MARKETPLACE, PluginId};

/// How many rows PageUp/PageDown move the list — the family's page.
const PLUGINS_PAGE: usize = TOOL_VIEW_PAGE;

/// The page's one sentence for a failed operation: `Couldn't install
/// code-review — remote: Repository not found.` The verb is the job's
/// ([`JobKind::verb`]); the reason is the store's own words, whole.
pub const PLUGINS_FAILED: &str = "Couldn't";

/// What a catalog read that failed outright says when there was a catalog
/// on the page already (a first read that fails shows its reason in the
/// list's place instead).
pub const PLUGINS_CATALOG_FAILED: &str = "Couldn't read the plugins";

/// What the page says when Esc stops a running job.
pub const PLUGINS_CANCELLED: &str = "Cancelled";

/// The dim line under [`PLUGINS_CANCELLED`] when the job had already done
/// its work by the time it was stopped.
pub const PLUGINS_CANCELLED_TOO_LATE: &str =
    "It had already finished — the page shows it as it is now.";

/// The pages of the `/plugins` walk.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PluginsPage {
    /// The catalog: a tab strip, the search line, the rows.
    #[default]
    Browse,
    /// One plugin: what it is, what it carries, what can be done with it.
    Plugin,
    /// One marketplace: where it comes from and what can be done with it.
    Marketplace,
    /// The field a new marketplace's source is typed into.
    AddMarketplace,
}

/// The tabs of the Browse page, in strip order: everything, what is
/// installed, one per marketplace (by **name**, so a refreshed catalog that
/// reorders its marketplaces keeps the tab), and the marketplaces
/// themselves.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub enum PluginsTab {
    /// Every plugin of every marketplace.
    #[default]
    All,
    /// The installed plugins, enabled or not.
    Installed,
    /// One marketplace's plugins.
    Marketplace(String),
    /// The marketplaces, with the rows that add or restore one.
    Marketplaces,
}

/// A marketplace being fetched, or one whose fetch failed — what the tab
/// strip, the subtitle and an empty tab show about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fetch {
    /// The marketplace's name as the catalog lists it (its source's display
    /// while it was never fetched).
    pub marketplace: String,
    /// How it is going.
    pub state: FetchState,
}

/// Where a [`Fetch`] stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchState {
    /// Under way: git's own phase word and percentage when it gave them, and
    /// the page clock's reading when it started (for the elapsed time).
    Running {
        phase: Option<String>,
        percent: Option<u8>,
        since: Duration,
    },
    /// It failed, for this reason — kept until the next fetch of it.
    Failed(String),
}

/// The page's one foreground operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job {
    /// What it does.
    pub kind: JobKind,
    /// What it does it to, as the page names it (a plugin's display name, a
    /// marketplace's name, a typed source).
    pub target: String,
    /// The step under way (`cloning github.com/owner/repo`), when told.
    pub step: Option<String>,
    /// How far, when git says.
    pub percent: Option<u8>,
    /// The page clock's reading when it started.
    pub since: Duration,
}

/// The kinds of [`Job`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobKind {
    Install,
    Update,
    Uninstall,
    AddMarketplace,
    /// Putting back a default marketplace the user removed, and fetching it.
    RestoreMarketplace,
    UpdateMarketplace,
    RemoveMarketplace,
}

impl JobKind {
    /// The verb a failure names: `Couldn't {verb} {target}`.
    #[must_use]
    pub const fn verb(self) -> &'static str {
        match self {
            Self::Install => "install",
            Self::Update | Self::UpdateMarketplace => "update",
            Self::Uninstall => "uninstall",
            Self::AddMarketplace => "add",
            Self::RestoreMarketplace => "restore",
            Self::RemoveMarketplace => "remove",
        }
    }

    /// What a failure calls its target when the reason already names it:
    /// `Couldn't add the marketplace — /work/team does not exist`.
    #[must_use]
    pub const fn noun(self) -> &'static str {
        match self {
            Self::Install | Self::Update | Self::Uninstall => "the plugin",
            Self::AddMarketplace
            | Self::RestoreMarketplace
            | Self::UpdateMarketplace
            | Self::RemoveMarketplace => "the marketplace",
        }
    }
}

/// The last result, stated on the page it happened on until the next key
/// that moves or acts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    /// A success (a green `✔`) or a failure (red throughout).
    pub ok: bool,
    /// A job the user stopped: neither, said dim.
    pub cancelled: bool,
    /// The sentence.
    pub text: String,
    /// A dim second sentence: what waits for the next launch.
    pub detail: Option<String>,
}

/// What the page asks the boundary to do ([`Action::PluginOp`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginOp {
    /// Reload the catalog from disk, then fetch what was never fetched or is
    /// stale; `force` fetches every marketplace.
    Refresh {
        force: bool,
    },
    /// Read one plugin's detail for the Plugin page.
    LoadDetail {
        id: PluginId,
    },
    Install {
        id: PluginId,
    },
    Update {
        id: PluginId,
    },
    Uninstall {
        id: PluginId,
    },
    SetEnabled {
        id: PluginId,
        enabled: bool,
    },
    AddMarketplace {
        source: String,
    },
    /// Fetch one marketplace that was never fetched.
    FetchMarketplace {
        marketplace: String,
    },
    UpdateMarketplace {
        name: String,
    },
    RemoveMarketplace {
        name: String,
    },
    RestoreDefault {
        source: String,
    },
    /// Stop the running job (the page already shows it stopped).
    Cancel,
}

/// What the boundary reports back, folded in by [`App::apply_plugin_event`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginEvent {
    /// The catalog as it is on disk now.
    Catalog(PluginCatalog),
    /// The catalog could not be read at all (a `plugins.json` that will not
    /// parse): the reason, shown in the list's place on a page that has
    /// none yet.
    CatalogFailed(String),
    /// The canonical sources of the default marketplaces the user removed —
    /// each one a `+ Restore …` row on the Marketplaces tab. Sent beside each
    /// [`PluginEvent::Catalog`], which does not carry them.
    RemovedDefaults(Vec<String>),
    FetchStarted {
        marketplace: String,
    },
    FetchProgress {
        marketplace: String,
        phase: Option<String>,
        percent: Option<u8>,
    },
    FetchFinished {
        marketplace: String,
        result: Result<(), String>,
    },
    Detail {
        id: PluginId,
        /// Boxed: a detail is by far the largest thing the page is told.
        result: Result<Box<PluginDetail>, String>,
    },
    JobProgress {
        step: Option<String>,
        percent: Option<u8>,
    },
    JobFinished {
        result: Result<JobDone, String>,
    },
    /// A job the user cancelled has ended: whether it changed anything on
    /// disk before it stopped. The fresh catalog (and detail) it read comes
    /// just before this.
    CancelledJobEnded {
        changed: bool,
    },
    /// A plugin was enabled or disabled — not a job, since the row flipped
    /// the moment the key was pressed; a failure flips it back.
    EnabledSet {
        id: PluginId,
        enabled: bool,
        result: Result<JobDone, String>,
    },
}

/// A job that worked: the sentence the page states, a dim second sentence,
/// and where the page goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobDone {
    pub text: String,
    pub detail: Option<String>,
    pub goto: Option<JobGoto>,
}

/// Where a finished job takes the page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobGoto {
    /// Back to Browse, on the tab it was on (an uninstall, a removal).
    Browse,
    /// Browse, on this marketplace's tab (a marketplace just added).
    MarketplaceTab(String),
}

/// One row of the Browse page's list, derived on demand
/// ([`PluginsMenu::rows`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrowseRow {
    /// A plugin: its index in the catalog.
    Plugin(usize),
    /// A marketplace: its index in the catalog.
    Marketplace(usize),
    /// `+ Add a marketplace`.
    AddMarketplace,
    /// `+ Restore {source}`: a default marketplace the user removed.
    Restore(String),
}

/// The actions the Plugin page offers, in order ([`PluginsMenu::plugin_actions`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginAction {
    Install,
    Enable,
    Disable,
    CheckForUpdates,
    /// Update to this version (the marketplace's).
    UpdateTo(String),
    Uninstall,
    Back,
}

/// The actions the Marketplace page offers, in order
/// ([`PluginsMenu::marketplace_actions`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarketplaceAction {
    BrowsePlugins,
    FetchNow,
    UpdateNow,
    /// A local folder's `Update now`: read the folder again.
    ReadAgain,
    Remove,
    Back,
}

/// What a remembered highlight names — kept by identity so it survives a
/// catalog that refreshes under it.
#[derive(Debug, Clone, PartialEq, Eq)]
enum RowKey {
    Plugin(PluginId),
    Marketplace(String),
    AddMarketplace,
    Restore(String),
}

/// The open `/plugins` page's state (`None` on [`App`] when closed).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PluginsMenu {
    pub page: PluginsPage,
    pub tab: PluginsTab,
    /// The type-to-search query (Browse). A space is typed into a query
    /// already begun; into an empty one it acts on the highlighted row.
    pub query: String,
    /// `None` until the first catalog lands.
    pub catalog: Option<PluginCatalog>,
    /// The Plugin page's subject's detail; `None` while it loads.
    pub detail: Option<Result<PluginDetail, String>>,
    /// Which plugin the Plugin page shows.
    pub subject: Option<PluginId>,
    /// Which marketplace the Marketplace page shows.
    pub marketplace: Option<String>,
    /// The Add page's field.
    pub input: String,
    /// Marketplaces being fetched, or whose fetch failed.
    pub fetches: Vec<Fetch>,
    /// The one foreground operation.
    pub job: Option<Job>,
    /// The last result.
    pub notice: Option<Notice>,
    /// Answers for any other round are dropped.
    pub round: u64,
    /// The highlighted row of the current tab's list.
    selected: usize,
    /// The highlight each other tab had when it was left, by identity.
    remembered: Vec<(PluginsTab, RowKey)>,
    /// The highlighted action on the Plugin and Marketplace pages.
    action: usize,
    /// Whether `Uninstall` / `Remove` has been pressed once and waits for
    /// the second Enter.
    armed: bool,
    /// The page clock: how long the page has been open, injected per draw.
    clock: Duration,
    /// The wall clock (Unix seconds), injected per draw — the `updated 2h
    /// ago` ages.
    now: i64,
    /// The removed default marketplaces, by canonical source.
    removed_defaults: Vec<String>,
    /// The config home as the user reads it (`~/.alter-zero`), for the
    /// Marketplace page's `Kept at` row.
    home: Option<String>,
    /// Why the first catalog could not be read.
    load_error: Option<String>,
    /// How many of a plugin's hooks and MCP servers wait for the user, as
    /// the boundary read them: `(plugin, hooks, servers, next_launch)`.
    review: Option<(PluginId, usize, usize, bool)>,
}

impl PluginsMenu {
    /// The tabs in strip order: `All`, `Installed`, one per marketplace in
    /// catalog order, `Marketplaces`.
    #[must_use]
    pub fn tabs(&self) -> Vec<PluginsTab> {
        let mut tabs = vec![PluginsTab::All, PluginsTab::Installed];
        if let Some(catalog) = &self.catalog {
            tabs.extend(
                catalog
                    .marketplaces
                    .iter()
                    .map(|m| PluginsTab::Marketplace(m.name.clone())),
            );
        }
        tabs.push(PluginsTab::Marketplaces);
        tabs
    }

    /// The active tab's position in [`tabs`](Self::tabs).
    #[must_use]
    pub fn tab_index(&self) -> usize {
        self.tabs()
            .iter()
            .position(|tab| *tab == self.tab)
            .unwrap_or(0)
    }

    /// A marketplace's index in the catalog, by name.
    #[must_use]
    pub fn marketplace_index(&self, name: &str) -> Option<usize> {
        self.catalog
            .as_ref()?
            .marketplaces
            .iter()
            .position(|m| m.name == name)
    }

    /// The fetch of a marketplace, running or failed.
    #[must_use]
    pub fn fetch_for(&self, marketplace: &str) -> Option<&Fetch> {
        self.fetches.iter().find(|f| f.marketplace == marketplace)
    }

    /// Whether any fetch is under way.
    #[must_use]
    pub fn fetching(&self) -> bool {
        self.fetches
            .iter()
            .any(|f| matches!(f.state, FetchState::Running { .. }))
    }

    /// Whether anything on the page moves on its own — a fetch or a job
    /// running. Only then does the page ask for animation frames: a still
    /// page writes nothing (`docs/overlay-repaint.md`).
    #[must_use]
    pub fn is_busy(&self) -> bool {
        self.job.is_some() || self.fetching()
    }

    /// The rows the current tab lists for the query, derived on each call:
    /// plugins as catalog indices in the catalog's own order (installed
    /// first, then by name), or on the Marketplaces tab the marketplaces
    /// whose name or source holds every word of the query, then
    /// `+ Add a marketplace`, then a `+ Restore …` row per removed default.
    #[must_use]
    pub fn rows(&self) -> Vec<BrowseRow> {
        if self.tab == PluginsTab::Marketplaces {
            let words: Vec<String> = self
                .query
                .split_whitespace()
                .map(str::to_lowercase)
                .collect();
            let mut rows: Vec<BrowseRow> = self
                .catalog
                .iter()
                .flat_map(|catalog| catalog.marketplaces.iter().enumerate())
                .filter(|(_, m)| {
                    let name = m.name.to_lowercase();
                    let source = m.source.to_lowercase();
                    words
                        .iter()
                        .all(|w| name.contains(w.as_str()) || source.contains(w.as_str()))
                })
                .map(|(i, _)| BrowseRow::Marketplace(i))
                .collect();
            rows.push(BrowseRow::AddMarketplace);
            rows.extend(
                self.removed_defaults
                    .iter()
                    .map(|source| BrowseRow::Restore(source.clone())),
            );
            return rows;
        }
        let Some(catalog) = &self.catalog else {
            return Vec::new();
        };
        let tab = match &self.tab {
            PluginsTab::All => CatalogTab::All,
            PluginsTab::Installed => CatalogTab::Installed,
            PluginsTab::Marketplace(name) => match self.marketplace_index(name) {
                Some(index) => CatalogTab::Marketplace(index),
                None => return Vec::new(),
            },
            PluginsTab::Marketplaces => unreachable!("handled above"),
        };
        catalog
            .filter(tab, &self.query)
            .into_iter()
            .map(BrowseRow::Plugin)
            .collect()
    }

    /// The highlighted row's index, clamped to `len` rows.
    #[must_use]
    pub fn selected_in(&self, len: usize) -> usize {
        self.selected.min(len.saturating_sub(1))
    }

    /// The highlighted row of the current tab, if it lists any.
    #[must_use]
    pub fn highlighted(&self) -> Option<BrowseRow> {
        let rows = self.rows();
        let at = self.selected_in(rows.len());
        rows.into_iter().nth(at)
    }

    /// The highlighted action on the Plugin / Marketplace page.
    #[must_use]
    pub const fn action(&self) -> usize {
        self.action
    }

    /// Whether `Uninstall` / `Remove` waits for its second Enter.
    #[must_use]
    pub const fn armed(&self) -> bool {
        self.armed
    }

    /// The page clock's last reading.
    #[must_use]
    pub const fn clock(&self) -> Duration {
        self.clock
    }

    /// The wall clock's last reading (Unix seconds).
    #[must_use]
    pub const fn now(&self) -> i64 {
        self.now
    }

    /// The removed default marketplaces, by canonical source.
    #[must_use]
    pub fn removed_defaults(&self) -> &[String] {
        &self.removed_defaults
    }

    /// The config home as the user reads it, when the boundary said.
    #[must_use]
    pub fn home(&self) -> Option<&str> {
        self.home.as_deref()
    }

    /// Why the first catalog could not be read.
    #[must_use]
    pub fn load_error(&self) -> Option<&str> {
        self.load_error.as_deref()
    }

    /// How many of the Plugin page's subject's hooks and MCP servers wait
    /// for the user — `(hooks, servers, next_launch)`, `next_launch` when the
    /// session has not loaded them yet and the startup review will ask —
    /// as the boundary last said ([`App::set_plugin_review`]).
    #[must_use]
    pub fn review_waiting(&self) -> Option<(usize, usize, bool)> {
        let (id, hooks, servers, next) = self.review.as_ref()?;
        (self.subject.as_ref() == Some(id)).then_some((*hooks, *servers, *next))
    }

    /// Whether `marketplace` is one the user removed: its plugins stay
    /// installed but there is nothing to update them from.
    #[must_use]
    pub fn marketplace_removed(&self, marketplace: &str) -> bool {
        self.marketplace_index(marketplace)
            .and_then(|i| {
                self.catalog
                    .as_ref()
                    .map(|c| c.marketplaces[i].is_removed())
            })
            .unwrap_or(false)
    }

    /// How long ago (in seconds) something stamped at `at` (Unix seconds)
    /// was, on the injected wall clock.
    #[must_use]
    pub fn age_of(&self, at: u64) -> i64 {
        self.now - i64::try_from(at).unwrap_or(i64::MAX)
    }

    /// The name a plugin is shown by: its display name when it has one.
    #[must_use]
    pub fn plugin_label(&self, id: &PluginId) -> String {
        let from_detail = self.detail.as_ref().and_then(|detail| {
            detail
                .as_ref()
                .ok()
                .filter(|d| &d.id == id)
                .and_then(|d| d.display_name.clone())
        });
        let from_catalog = || {
            let catalog = self.catalog.as_ref()?;
            let index = catalog.find(id)?;
            catalog.plugins[index].display_name.clone()
        };
        from_detail
            .or_else(from_catalog)
            .unwrap_or_else(|| id.name.clone())
    }

    /// The Plugin page's actions for what is known of its subject: only
    /// `Back` while the detail loads or after it failed; else `Install`
    /// (unless blocked) for a plugin not installed, and for an installed one
    /// `Disable`/`Enable`, the update action and `Uninstall` — each list
    /// closing on `Back`.
    #[must_use]
    pub fn plugin_actions(&self) -> Vec<PluginAction> {
        let Some(Ok(detail)) = &self.detail else {
            return vec![PluginAction::Back];
        };
        let mut actions = Vec::new();
        match detail.status {
            PluginStatus::Available => {
                if detail.blocked.is_none() {
                    actions.push(PluginAction::Install);
                }
            }
            PluginStatus::Unsupported => {}
            PluginStatus::Enabled | PluginStatus::Disabled => {
                actions.push(if detail.status == PluginStatus::Enabled {
                    PluginAction::Disable
                } else {
                    PluginAction::Enable
                });
                // A removed marketplace has nothing to update from.
                if !self.marketplace_removed(&detail.id.marketplace) {
                    actions.push(match (&detail.version, detail.update_available) {
                        (Some(version), true) => PluginAction::UpdateTo(version.clone()),
                        _ => PluginAction::CheckForUpdates,
                    });
                }
                actions.push(PluginAction::Uninstall);
            }
        }
        actions.push(PluginAction::Back);
        actions
    }

    /// The Marketplace page's actions: `Fetch now` first for one never
    /// fetched, else `Browse its plugins` and `Update now` (`Read it again`
    /// for a local folder); then `Remove` and `Back`.
    #[must_use]
    pub fn marketplace_actions(&self) -> Vec<MarketplaceAction> {
        let found = self
            .marketplace
            .as_deref()
            .and_then(|name| self.marketplace_index(name))
            .and_then(|i| self.catalog.as_ref().map(|c| &c.marketplaces[i]));
        let Some(marketplace) = found else {
            return vec![MarketplaceAction::Back];
        };
        // A removed marketplace still lists the plugins installed from it,
        // and the folder installs are no marketplace at all: either can be
        // browsed, neither fetched, updated or removed.
        if marketplace.is_removed() || marketplace.name == LOCAL_MARKETPLACE {
            return vec![MarketplaceAction::BrowsePlugins, MarketplaceAction::Back];
        }
        let mut actions = if marketplace.fetched {
            vec![
                MarketplaceAction::BrowsePlugins,
                if marketplace.is_local {
                    MarketplaceAction::ReadAgain
                } else {
                    MarketplaceAction::UpdateNow
                },
            ]
        } else {
            vec![MarketplaceAction::FetchNow]
        };
        actions.extend([MarketplaceAction::Remove, MarketplaceAction::Back]);
        actions
    }

    /// The identity of a row of the current tab.
    fn key_of(&self, row: &BrowseRow) -> Option<RowKey> {
        Some(match row {
            BrowseRow::Plugin(i) => RowKey::Plugin(self.catalog.as_ref()?.id_of(*i)),
            BrowseRow::Marketplace(i) => {
                RowKey::Marketplace(self.catalog.as_ref()?.marketplaces.get(*i)?.name.clone())
            }
            BrowseRow::AddMarketplace => RowKey::AddMarketplace,
            BrowseRow::Restore(source) => RowKey::Restore(source.clone()),
        })
    }

    /// The highlighted row's identity.
    fn highlighted_key(&self) -> Option<RowKey> {
        self.highlighted().and_then(|row| self.key_of(&row))
    }

    /// Put the highlight back on `key` in the current tab's rows, or on the
    /// first row when it is no longer listed.
    fn select_key(&mut self, key: Option<&RowKey>) {
        let rows = self.rows();
        self.selected = key
            .and_then(|key| {
                rows.iter()
                    .position(|row| self.key_of(row).as_ref() == Some(key))
            })
            .unwrap_or(0);
    }
}

/// `Couldn't {verb} {target} — {reason}`: a failure in the page's words over
/// the store's own reason, kept whole.
fn failure(verb: &str, target: &str, reason: &str) -> Notice {
    Notice {
        ok: false,
        cancelled: false,
        text: format!("{PLUGINS_FAILED} {verb} {target} — {reason}"),
        detail: None,
    }
}

/// A finished job's or toggle's result as the page's notice.
fn success(done: &JobDone) -> Notice {
    Notice {
        ok: true,
        cancelled: false,
        text: done.text.clone(),
        detail: done.detail.clone(),
    }
}

/// `text` as one line for a field: line breaks dropped, the ends trimmed —
/// what a pasted source or search becomes.
fn one_line(text: &str) -> String {
    text.chars()
        .filter(|c| !matches!(c, '\n' | '\r'))
        .collect::<String>()
        .trim()
        .to_string()
}

impl App {
    /// Open the `/plugins` page on Browse / All with no catalog yet, and
    /// return the round every answer for it must name — counted on [`App`],
    /// so a page closed and reopened is never named by an old round. The
    /// boundary then starts the first catalog read. Abandons any `?` band /
    /// palette / picker band (they share the composer the page takes over)
    /// — every picker's open rule.
    pub fn open_plugins_menu(&mut self) -> u64 {
        self.shortcuts_open = false;
        self.command_menu = None;
        self.file_search = None;
        self.skill_picker = None;
        self.backtrack = Backtrack::default();
        self.plugins_rounds += 1;
        self.plugins_menu = Some(PluginsMenu {
            round: self.plugins_rounds,
            ..PluginsMenu::default()
        });
        self.plugins_rounds
    }

    /// Close the page — and drop everything it held, the catalog included:
    /// nothing is kept for the next open (`docs/plugins.md` *Memory*). The
    /// boundary cancels whatever was still in flight; an answer that lands
    /// anyway finds no page and is dropped.
    pub fn close_plugins_menu(&mut self) {
        self.plugins_menu = None;
    }

    /// The page clock — how long the page has been open — injected per draw
    /// by the boundary (the status line's pattern): what a fetch's or a
    /// job's elapsed time and the spinner frame are read off.
    pub fn set_plugins_clock(&mut self, elapsed: Duration) {
        if let Some(menu) = self.plugins_menu.as_mut() {
            menu.clock = elapsed;
        }
    }

    /// The wall clock (Unix seconds), injected per draw — what the
    /// `updated 2h ago` ages count from.
    pub fn set_plugins_now(&mut self, now: i64) {
        if let Some(menu) = self.plugins_menu.as_mut() {
            menu.now = now;
        }
    }

    /// The config home as the user reads it (`~/.alter-zero`), for the
    /// Marketplace page's `Kept at` row — the boundary's to know.
    pub fn set_plugins_home(&mut self, home: Option<String>) {
        if let Some(menu) = self.plugins_menu.as_mut() {
            menu.home = home;
        }
    }

    /// How many of `id`'s hooks and MCP servers wait for the user — read
    /// at the boundary from what the session trusts, or from the plugin
    /// itself when the session has not loaded it yet (`next_launch`). The
    /// Plugin page of `id` says so; `0, 0` says nothing.
    pub fn set_plugin_review(
        &mut self,
        id: &PluginId,
        hooks: usize,
        servers: usize,
        next_launch: bool,
    ) {
        if let Some(menu) = self.plugins_menu.as_mut() {
            menu.review = Some((id.clone(), hooks, servers, next_launch));
        }
    }

    /// Fold one of the boundary's answers into the open page. An answer for
    /// another round, or with no page open, is dropped.
    pub fn apply_plugin_event(&mut self, round: u64, event: PluginEvent) {
        let Some(menu) = self.plugins_menu.as_mut().filter(|m| m.round == round) else {
            return;
        };
        match event {
            PluginEvent::Catalog(catalog) => menu.take_catalog(catalog),
            PluginEvent::CatalogFailed(reason) => {
                if menu.catalog.is_none() {
                    menu.load_error = Some(reason);
                } else {
                    menu.notice = Some(Notice {
                        ok: false,
                        cancelled: false,
                        text: format!("{PLUGINS_CATALOG_FAILED} — {reason}"),
                        detail: None,
                    });
                }
            }
            PluginEvent::RemovedDefaults(sources) => {
                let key = menu.highlighted_key();
                menu.removed_defaults = sources;
                menu.select_key(key.as_ref());
            }
            PluginEvent::FetchStarted { marketplace } => {
                menu.set_fetch(marketplace, None, None);
            }
            PluginEvent::FetchProgress {
                marketplace,
                phase,
                percent,
            } => menu.set_fetch(marketplace, phase, percent),
            PluginEvent::FetchFinished {
                marketplace,
                result,
            } => {
                menu.fetches.retain(|f| f.marketplace != marketplace);
                if let Err(reason) = result {
                    menu.fetches.push(Fetch {
                        marketplace,
                        state: FetchState::Failed(reason),
                    });
                }
            }
            PluginEvent::Detail { id, result } => {
                if menu.page == PluginsPage::Plugin && menu.subject.as_ref() == Some(&id) {
                    menu.detail = Some(result.map(|detail| *detail));
                    let last = menu.plugin_actions().len().saturating_sub(1);
                    menu.action = menu.action.min(last);
                }
            }
            PluginEvent::JobProgress { step, percent } => {
                if let Some(job) = menu.job.as_mut() {
                    job.step = step;
                    job.percent = percent;
                }
            }
            PluginEvent::JobFinished { result } => menu.finish_job(result),
            PluginEvent::CancelledJobEnded { changed } => {
                if changed && let Some(notice) = menu.notice.as_mut().filter(|n| n.cancelled) {
                    notice.detail = Some(PLUGINS_CANCELLED_TOO_LATE.to_string());
                }
            }
            PluginEvent::EnabledSet {
                id,
                enabled,
                result,
            } => match result {
                Ok(done) => menu.notice = Some(success(&done)),
                Err(reason) => {
                    menu.set_status(&id, !enabled);
                    let verb = if enabled { "enable" } else { "disable" };
                    menu.notice = Some(failure(verb, &menu.plugin_label(&id), &reason));
                }
            },
        }
    }

    /// A paste while the page is open: the Add page's field and the Browse
    /// search take it as one line (line breaks dropped, the ends trimmed);
    /// every other page — and any page while a job runs — swallows it, so a
    /// paste never reaches the composer draft hidden underneath.
    pub fn paste_into_plugins(&mut self, text: &str) {
        let Some(menu) = self.plugins_menu.as_mut() else {
            return;
        };
        if menu.job.is_some() {
            return;
        }
        let text = one_line(text);
        match menu.page {
            PluginsPage::AddMarketplace => menu.input.push_str(&text),
            PluginsPage::Browse => {
                menu.query.push_str(&text);
                menu.selected = 0;
            }
            PluginsPage::Plugin | PluginsPage::Marketplace => {}
        }
    }

    /// Keys while the `/plugins` page is open — routed at the top of
    /// [`on_key`](App::on_key), so the page owns every key. Ctrl+C closes it
    /// from anywhere; while a job runs, Esc cancels it and nothing else does
    /// anything; otherwise each page has its own grammar (`docs/plugins.md`
    /// *Keys*), and Esc always goes back exactly one level. Any key the page
    /// acts on clears the last result first.
    pub(super) fn on_key_plugins(&mut self, key: KeyEvent) -> Action {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.close_plugins_menu();
            return Action::ClosePluginsMenu;
        }
        let Some(menu) = self.plugins_menu.as_mut() else {
            return Action::None;
        };
        if menu.job.is_some() {
            let cancels = key.code == KeyCode::Esc
                || (key.code == KeyCode::Backspace
                    && matches!(menu.page, PluginsPage::Plugin | PluginsPage::Marketplace));
            if cancels {
                if let Some(job) = menu.job.take() {
                    menu.notice = Some(Notice {
                        ok: false,
                        cancelled: true,
                        text: format!("{PLUGINS_CANCELLED} — {} {}", job.kind.verb(), job.target),
                        detail: None,
                    });
                }
                return Action::PluginOp(PluginOp::Cancel);
            }
            return Action::None;
        }
        menu.notice = None;
        match menu.page {
            PluginsPage::Browse => self.on_key_plugins_browse(key),
            PluginsPage::Plugin => self.on_key_plugins_detail(key, Self::run_plugin_action),
            PluginsPage::Marketplace => {
                self.on_key_plugins_detail(key, Self::run_marketplace_action)
            }
            PluginsPage::AddMarketplace => self.on_key_plugins_add(key),
        }
    }

    /// The Browse page's keys.
    fn on_key_plugins_browse(&mut self, key: KeyEvent) -> Action {
        let Some(menu) = self.plugins_menu.as_mut() else {
            return Action::None;
        };
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let len = menu.rows().len();
        let last = len.saturating_sub(1);
        let at = menu.selected_in(len);
        match key.code {
            KeyCode::Char('r') if ctrl => {
                if menu.fetching() {
                    return Action::None;
                }
                return Action::PluginOp(PluginOp::Refresh { force: true });
            }
            KeyCode::Up => menu.selected = wrap_step(at, len, -1),
            KeyCode::Down => menu.selected = wrap_step(at, len, 1),
            KeyCode::PageUp => menu.selected = at.saturating_sub(PLUGINS_PAGE),
            KeyCode::PageDown => menu.selected = (at + PLUGINS_PAGE).min(last),
            KeyCode::Home => menu.selected = 0,
            KeyCode::End => menu.selected = last,
            KeyCode::Left | KeyCode::BackTab => menu.step_tab(-1),
            KeyCode::Right | KeyCode::Tab => menu.step_tab(1),
            KeyCode::Enter => return menu.open_highlighted(),
            KeyCode::Char(' ') if menu.query.is_empty() && !ctrl => {
                return menu.toggle_or_open();
            }
            KeyCode::Esc => {
                if menu.query.is_empty() {
                    self.close_plugins_menu();
                    return Action::ClosePluginsMenu;
                }
                menu.clear_query();
            }
            KeyCode::Char('u') if ctrl => menu.clear_query(),
            KeyCode::Backspace => {
                if menu.query.chars().count() <= 1 {
                    menu.clear_query();
                } else {
                    menu.query.pop();
                    menu.selected = 0;
                }
            }
            KeyCode::Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                menu.query.push(c);
                menu.selected = 0;
            }
            _ => {}
        }
        Action::None
    }

    /// The Plugin and Marketplace pages' keys — one grammar over two action
    /// lists: ↑/↓/Home/End and a digit move the highlight (and disarm a
    /// confirmation), Enter runs the highlighted action, Esc and Backspace
    /// disarm or go back. Any other key disarms too. Only Enter runs
    /// anything, so a search phrase typed a moment too long — its keys
    /// landing on a page Space opened — never installs or removes.
    fn on_key_plugins_detail(&mut self, key: KeyEvent, run: fn(&mut Self) -> Action) -> Action {
        let Some(menu) = self.plugins_menu.as_mut() else {
            return Action::None;
        };
        let len = match menu.page {
            PluginsPage::Plugin => menu.plugin_actions().len(),
            _ => menu.marketplace_actions().len(),
        };
        match key.code {
            KeyCode::Up => {
                menu.armed = false;
                menu.action = wrap_step(menu.action, len, -1);
            }
            KeyCode::Down => {
                menu.armed = false;
                menu.action = wrap_step(menu.action, len, 1);
            }
            KeyCode::Home => {
                menu.armed = false;
                menu.action = 0;
            }
            KeyCode::End => {
                menu.armed = false;
                menu.action = len.saturating_sub(1);
            }
            KeyCode::Char(c @ '1'..='9') => {
                let index = c as usize - '1' as usize;
                if index != menu.action || index >= len {
                    menu.armed = false;
                }
                if index < len {
                    menu.action = index;
                }
            }
            KeyCode::Enter => return run(self),
            KeyCode::Esc | KeyCode::Backspace => {
                if menu.armed {
                    menu.armed = false;
                } else {
                    menu.back_to_browse();
                }
            }
            _ => menu.armed = false,
        }
        Action::None
    }

    /// Run the Plugin page's highlighted action.
    fn run_plugin_action(&mut self) -> Action {
        let Some(menu) = self.plugins_menu.as_mut() else {
            return Action::None;
        };
        let Some(action) = menu.plugin_actions().into_iter().nth(menu.action) else {
            return Action::None;
        };
        let Some(id) = menu.subject.clone() else {
            menu.back_to_browse();
            return Action::None;
        };
        let target = menu.plugin_label(&id);
        match action {
            PluginAction::Install => {
                menu.start_job(JobKind::Install, target);
                Action::PluginOp(PluginOp::Install { id })
            }
            PluginAction::Enable | PluginAction::Disable => {
                let enabled = action == PluginAction::Enable;
                menu.set_status(&id, enabled);
                Action::PluginOp(PluginOp::SetEnabled { id, enabled })
            }
            PluginAction::CheckForUpdates | PluginAction::UpdateTo(_) => {
                menu.start_job(JobKind::Update, target);
                Action::PluginOp(PluginOp::Update { id })
            }
            PluginAction::Uninstall => {
                if !menu.armed {
                    menu.armed = true;
                    return Action::None;
                }
                menu.armed = false;
                menu.start_job(JobKind::Uninstall, target);
                Action::PluginOp(PluginOp::Uninstall { id })
            }
            PluginAction::Back => {
                menu.back_to_browse();
                Action::None
            }
        }
    }

    /// Run the Marketplace page's highlighted action.
    fn run_marketplace_action(&mut self) -> Action {
        let Some(menu) = self.plugins_menu.as_mut() else {
            return Action::None;
        };
        let Some(action) = menu.marketplace_actions().into_iter().nth(menu.action) else {
            return Action::None;
        };
        let Some(name) = menu.marketplace.clone() else {
            menu.back_to_browse();
            return Action::None;
        };
        match action {
            MarketplaceAction::BrowsePlugins => {
                menu.back_to_browse();
                menu.switch_tab(PluginsTab::Marketplace(name));
                Action::None
            }
            MarketplaceAction::FetchNow => {
                menu.set_fetch(name.clone(), None, None);
                Action::PluginOp(PluginOp::FetchMarketplace { marketplace: name })
            }
            MarketplaceAction::UpdateNow | MarketplaceAction::ReadAgain => {
                menu.start_job(JobKind::UpdateMarketplace, name.clone());
                Action::PluginOp(PluginOp::UpdateMarketplace { name })
            }
            MarketplaceAction::Remove => {
                if !menu.armed {
                    menu.armed = true;
                    return Action::None;
                }
                menu.armed = false;
                menu.start_job(JobKind::RemoveMarketplace, name.clone());
                Action::PluginOp(PluginOp::RemoveMarketplace { name })
            }
            MarketplaceAction::Back => {
                menu.back_to_browse();
                Action::None
            }
        }
    }

    /// The Add page's keys: a plain text field, Enter adding what it holds.
    fn on_key_plugins_add(&mut self, key: KeyEvent) -> Action {
        let Some(menu) = self.plugins_menu.as_mut() else {
            return Action::None;
        };
        match key.code {
            KeyCode::Esc => {
                menu.back_to_browse();
                menu.switch_tab(PluginsTab::Marketplaces);
            }
            KeyCode::Enter => {
                let source = menu.input.trim().to_string();
                if source.is_empty() {
                    return Action::None;
                }
                menu.start_job(JobKind::AddMarketplace, source.clone());
                return Action::PluginOp(PluginOp::AddMarketplace { source });
            }
            KeyCode::Backspace => {
                menu.input.pop();
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                menu.input.clear();
            }
            KeyCode::Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                menu.input.push(c);
            }
            _ => {}
        }
        Action::None
    }
}

impl PluginsMenu {
    /// A new catalog: the tab (by name — a marketplace that vanished sends
    /// the strip back to `All`), the query, and the highlight (by identity)
    /// all kept; the Plugin page's subject is kept too, its detail refreshed
    /// separately by the boundary. A Marketplace page whose marketplace is
    /// gone goes back to Browse.
    fn take_catalog(&mut self, catalog: PluginCatalog) {
        let key = self.highlighted_key();
        let remembered_here = key.clone();
        self.catalog = Some(catalog);
        self.load_error = None;
        if let PluginsTab::Marketplace(name) = &self.tab
            && self.marketplace_index(name).is_none()
        {
            self.tab = PluginsTab::All;
            let remembered = self.recall(&PluginsTab::All);
            self.select_key(remembered.as_ref());
        } else {
            self.select_key(remembered_here.as_ref());
        }
        if self.page == PluginsPage::Marketplace
            && self
                .marketplace
                .as_deref()
                .is_none_or(|name| self.marketplace_index(name).is_none())
        {
            self.back_to_browse();
        }
    }

    /// Record a fetch as running — started, or with git's progress.
    fn set_fetch(&mut self, marketplace: String, phase: Option<String>, percent: Option<u8>) {
        let since = match self.fetch_for(&marketplace).map(|f| &f.state) {
            Some(FetchState::Running { since, .. }) => *since,
            _ => self.clock,
        };
        self.fetches.retain(|f| f.marketplace != marketplace);
        self.fetches.push(Fetch {
            marketplace,
            state: FetchState::Running {
                phase,
                percent,
                since,
            },
        });
    }

    /// Start the page's job, on the page clock.
    fn start_job(&mut self, kind: JobKind, target: String) {
        self.job = Some(Job {
            kind,
            target,
            step: None,
            percent: None,
            since: self.clock,
        });
    }

    /// The running job finished: its result becomes the page's notice, and
    /// a success may move the page. Nothing happens for a job the user has
    /// already cancelled.
    fn finish_job(&mut self, result: Result<JobDone, String>) {
        let Some(job) = self.job.take() else {
            return;
        };
        self.armed = false;
        match result {
            Ok(done) => {
                self.notice = Some(success(&done));
                match done.goto {
                    Some(JobGoto::Browse) => self.back_to_browse(),
                    Some(JobGoto::MarketplaceTab(name)) => {
                        self.back_to_browse();
                        self.switch_tab(PluginsTab::Marketplace(name));
                    }
                    None => self.action = 0,
                }
            }
            Err(reason) => {
                let target = if reason.contains(job.target.as_str()) {
                    job.kind.noun()
                } else {
                    job.target.as_str()
                };
                self.notice = Some(failure(job.kind.verb(), target, &reason));
            }
        }
    }

    /// Set a plugin's status to enabled or disabled — the row's mark and the
    /// open detail alike — at once, before the write lands.
    fn set_status(&mut self, id: &PluginId, enabled: bool) {
        let status = if enabled {
            PluginStatus::Enabled
        } else {
            PluginStatus::Disabled
        };
        if let Some(catalog) = self.catalog.as_mut()
            && let Some(index) = catalog.find(id)
        {
            catalog.plugins[index].status = status;
        }
        if let Some(Ok(detail)) = self.detail.as_mut()
            && &detail.id == id
        {
            detail.status = status;
        }
    }

    /// Empty the search, keeping the highlight on the row it was on — the
    /// plugin found stays found when the list comes back whole. While a
    /// query is typed the highlight is its best match instead.
    fn clear_query(&mut self) {
        let key = self.highlighted_key();
        self.query.clear();
        self.select_key(key.as_ref());
    }

    /// Back to Browse, on the tab and the row the page was opened from.
    fn back_to_browse(&mut self) {
        self.page = PluginsPage::Browse;
        self.subject = None;
        self.detail = None;
        self.marketplace = None;
        self.action = 0;
        self.armed = false;
    }

    /// Step the tab strip by `delta`, wrapping.
    fn step_tab(&mut self, delta: isize) {
        let tabs = self.tabs();
        let next = wrap_step(self.tab_index(), tabs.len(), delta);
        if let Some(tab) = tabs.into_iter().nth(next) {
            self.switch_tab(tab);
        }
    }

    /// Make `tab` the active tab: the highlight of the tab being left is
    /// remembered, the one `tab` had is put back.
    fn switch_tab(&mut self, tab: PluginsTab) {
        if let Some(key) = self.highlighted_key() {
            let left = self.tab.clone();
            self.remembered.retain(|(t, _)| *t != left);
            self.remembered.push((left, key));
        }
        self.tab = tab;
        let key = self.recall(&self.tab.clone());
        self.select_key(key.as_ref());
    }

    /// The highlight `tab` had when it was last left.
    fn recall(&self, tab: &PluginsTab) -> Option<RowKey> {
        self.remembered
            .iter()
            .find(|(t, _)| t == tab)
            .map(|(_, key)| key.clone())
    }

    /// Enter on the highlighted row: a plugin opens its page and asks for
    /// its detail; a marketplace opens its page; `+ Add a marketplace` opens
    /// the field; `+ Restore …` restores the default and fetches it.
    fn open_highlighted(&mut self) -> Action {
        let Some(row) = self.highlighted() else {
            return Action::None;
        };
        self.action = 0;
        self.armed = false;
        match row {
            BrowseRow::Plugin(index) => {
                let Some(catalog) = self.catalog.as_ref() else {
                    return Action::None;
                };
                let id = catalog.id_of(index);
                self.page = PluginsPage::Plugin;
                self.subject = Some(id.clone());
                self.detail = None;
                Action::PluginOp(PluginOp::LoadDetail { id })
            }
            BrowseRow::Marketplace(index) => {
                let Some(name) = self
                    .catalog
                    .as_ref()
                    .and_then(|c| c.marketplaces.get(index))
                    .map(|m| m.name.clone())
                else {
                    return Action::None;
                };
                self.page = PluginsPage::Marketplace;
                self.marketplace = Some(name);
                Action::None
            }
            BrowseRow::AddMarketplace => {
                self.page = PluginsPage::AddMarketplace;
                Action::None
            }
            BrowseRow::Restore(source) => {
                self.start_job(JobKind::RestoreMarketplace, source.clone());
                Action::PluginOp(PluginOp::RestoreDefault { source })
            }
        }
    }

    /// Space on the highlighted row: an installed plugin is enabled or
    /// disabled in place (the row flips at once); anything else opens like
    /// Enter.
    fn toggle_or_open(&mut self) -> Action {
        let installed = match self.highlighted() {
            Some(BrowseRow::Plugin(index)) => self.catalog.as_ref().and_then(|catalog| {
                let plugin = &catalog.plugins[index];
                matches!(
                    plugin.status,
                    PluginStatus::Enabled | PluginStatus::Disabled
                )
                .then(|| {
                    (
                        catalog.id_of(index),
                        plugin.status == PluginStatus::Disabled,
                    )
                })
            }),
            _ => None,
        };
        let Some((id, enabled)) = installed else {
            return self.open_highlighted();
        };
        self.set_status(&id, enabled);
        Action::PluginOp(PluginOp::SetEnabled { id, enabled })
    }
}

/// What the page asks of a catalog marketplace beyond its fields.
pub trait MarketplaceState {
    /// Whether it is one the user removed whose installed plugins are still
    /// listed: never fetched, yet carrying a reason — a marketplace that was
    /// simply never fetched has none (`docs/plugins.md`). Such a one is shown
    /// dim, as removed, never as a failed fetch, and offers nothing to fetch.
    fn is_removed(&self) -> bool;
}

impl MarketplaceState for crate::plugins::catalog::CatalogMarketplace {
    fn is_removed(&self) -> bool {
        !self.fetched && self.error.is_some()
    }
}
