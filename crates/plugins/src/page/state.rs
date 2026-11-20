//! The `/plugins` page's state and keys (`docs/plugins-menu.md`): codex's
//! `/plugins` — a tab for every plugin, one for the installed ones, one per
//! added marketplace and one that adds a marketplace, over a search line; a
//! page per plugin; the field that adds a marketplace from any source; a
//! question before anything that cannot be undone.
//!
//! **Pure.** It never reads a file, fetches or installs. The host reads the
//! catalogs when the page opens and injects them whole — a
//! [`PluginsSnapshot`] of plain rows ([`PluginsPage::open`]) — and the page
//! answers keys with a [`KeyOutcome`], typed [`PluginOp`]s among them, for the
//! host to run. Nothing about plugins is resident while the page is closed:
//! the host drops the page, and the snapshot with it.
//!
//! **Jobs.** An operation that takes time is marked running *here*, the
//! moment it is asked for, under a fresh **round** — so the frame that took
//! the key already knows — and the host answers it with
//! [`PluginsPage::finish_job`], carrying that round and a fresh snapshot. An
//! answer is taken only while it is still the page's job: one that comes
//! back after Esc let go of it is dropped here. Rounds continue from the
//! `round_base` the host passes to [`PluginsPage::open`] and reads back with
//! [`PluginsPage::last_round`] when the page closes, so a reopened page never
//! matches an old round. One job at a time: they all write one registry.
//!
//! **The page never moves under the user** (`docs/plugins-menu.md` *No
//! flicker*): a job is said in the hint's own row, and only once it has run
//! [`PLUGINS_JOB_DELAY`]; what went right is never said — the rows and pages
//! show it; a failure is said in red until the next key.

use std::time::Duration;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use unicode_width::UnicodeWidthChar;

pub use crate::state::PluginId;

use super::rows::{PluginList, PluginRef};

/// How many rows PageUp/PageDown move a list — the main crate's
/// `TOOL_VIEW_PAGE`, the picker family's page.
const PAGE_STEP: usize = 10;

/// One ↑/↓ step of a selection over `len` rows, wrapping at the ends — the
/// main crate's `wrap_step`, every selection list's grammar.
fn wrap_step(selected: usize, len: usize, delta: isize) -> usize {
    let Some(last) = len.checked_sub(1) else {
        return 0;
    };
    let current = selected.min(last) as isize;
    (current + delta).rem_euclid(len as isize) as usize
}

/// How long a job runs before the page says so — a toggle answers in a few
/// milliseconds, and a line shown for one frame is a flicker.
pub const PLUGINS_JOB_DELAY: Duration = Duration::from_millis(400);

// ===== the snapshot the boundary injects =====

/// An installed plugin's state, as the page shows it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InstalledState {
    /// The version installed.
    pub version: Option<String>,
    /// On in the directory the page was opened in. A plugin is installed for
    /// the user and on in every directory until one turns it off.
    pub enabled: bool,
    /// Its marketplace now has a newer copy.
    pub update: bool,
    /// Why a session does not load it (`✘ not loaded`), when it does not.
    pub error: Option<String>,
    /// It loads in place from its marketplace's directory: always current,
    /// nothing to update.
    pub in_place: bool,
    /// Where it loads from, `~`-relative — shown for one loaded in place.
    pub loads_from: Option<String>,
}

/// What a plugin is made of, by the names a session knows each component by
/// (`hello:greet`, `plugin:guard:probe`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ComponentSummary {
    /// Skills, by name.
    pub skills: Vec<String>,
    /// Slash commands, by name.
    pub commands: Vec<String>,
    /// Subagent types, by name.
    pub agents: Vec<String>,
    /// The events its hooks fire on, each with its count when more than one:
    /// `PreToolUse`, `Stop (2)`. Empty in a preview.
    pub hooks: Vec<String>,
    /// How many hooks that is.
    pub hook_count: usize,
    /// MCP servers, by name. Empty in a preview.
    pub servers: Vec<String>,
    /// What it ships that this app has no counterpart for (`apps`, `LSP
    /// servers`).
    pub unused: Vec<String>,
    /// A **preview** — read from the marketplace's file listing before the
    /// plugin is installed: the names of its skills, commands and agents, and
    /// only *whether* it has hooks and MCP servers
    /// ([`has_hooks`](Self::has_hooks), [`has_servers`](Self::has_servers)).
    pub preview: bool,
    /// A preview's word that it has hooks.
    pub has_hooks: bool,
    /// A preview's word that it has MCP servers.
    pub has_servers: bool,
}

/// `{n} {noun}`, the noun plural for anything but one.
fn counted(n: usize, noun: &str) -> String {
    if n == 1 {
        format!("{n} {noun}")
    } else {
        format!("{n} {noun}s")
    }
}

impl ComponentSummary {
    /// Does it bring hooks — counted, or seen in a preview?
    #[must_use]
    pub fn hooks_present(&self) -> bool {
        self.hook_count > 0 || !self.hooks.is_empty() || self.has_hooks
    }

    /// Does it bring MCP servers — named, or seen in a preview?
    #[must_use]
    pub fn servers_present(&self) -> bool {
        !self.servers.is_empty() || self.has_servers
    }

    /// Does it add anything that **runs** — a hook or a server — and so
    /// anything the review has to show before it does?
    #[must_use]
    pub fn runs_code(&self) -> bool {
        self.hooks_present() || self.servers_present()
    }

    /// The summary in one line: `1 skill · 2 commands · 3 hooks`; a preview
    /// names hooks and servers without counts.
    #[must_use]
    pub fn line(&self) -> String {
        let mut parts: Vec<String> = [
            (self.skills.len(), "skill"),
            (self.commands.len(), "command"),
            (self.agents.len(), "agent"),
        ]
        .into_iter()
        .filter(|(n, _)| *n > 0)
        .map(|(n, noun)| counted(n, noun))
        .collect();
        if self.hook_count > 0 {
            parts.push(counted(self.hook_count, "hook"));
        } else if self.hooks_present() {
            parts.push("hooks".to_string());
        }
        if !self.servers.is_empty() {
            parts.push(counted(self.servers.len(), "MCP server"));
        } else if self.has_servers {
            parts.push("MCP servers".to_string());
        }
        if parts.is_empty() {
            "nothing this app loads".to_string()
        } else {
            parts.join(" · ")
        }
    }
}

/// What the page knows a plugin contains.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum PluginContents {
    /// Not read yet: its page asks ([`PluginOp::Inspect`]) and shows
    /// `reading…` meanwhile.
    #[default]
    NotRead,
    /// Read — from the installed copy, or a preview of one kept in its
    /// marketplace ([`ComponentSummary::preview`]).
    Known(ComponentSummary),
    /// Not knowable before it is installed: a plugin in a repository of its
    /// own (`read once it is installed`).
    Unknown,
    /// The read failed — the real message.
    Unreadable(String),
}

/// One plugin, as `/plugins` lists it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PluginRow {
    pub id: PluginId,
    /// The manifest's display name, when it gives one — the page's title.
    pub display_name: Option<String>,
    pub description: Option<String>,
    /// The version its marketplace offers.
    pub version: Option<String>,
    pub author: Option<String>,
    pub category: Option<String>,
    pub homepage: Option<String>,
    /// Searched, never shown.
    pub keywords: Vec<String>,
    /// Where it is fetched from, in one line (a repository's address is a
    /// link on its page).
    pub source: String,
    /// Why it cannot be installed (`– unavailable`), when it cannot.
    pub unavailable: Option<String>,
    /// Its format, once its files have been read (`Claude Code`, `Agent
    /// Plugins 1.0.0`).
    pub format: Option<String>,
    /// What it is made of, as far as is known.
    pub contents: PluginContents,
    pub installed: Option<InstalledState>,
    /// What reading it said that did not cost it.
    pub notes: Vec<String>,
}

impl PluginRow {
    /// The name people see: the display name, else the plugin's own.
    #[must_use]
    pub fn title(&self) -> &str {
        self.display_name.as_deref().unwrap_or(&self.id.name)
    }

    /// Is it installed and on in this directory?
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.installed.as_ref().is_some_and(|state| state.enabled)
    }

    /// The version to show: the installed one, else the one its marketplace
    /// offers.
    #[must_use]
    pub fn shown_version(&self) -> Option<&str> {
        self.installed
            .as_ref()
            .and_then(|state| state.version.as_deref())
            .or(self.version.as_deref())
    }

    /// What it is known to contain, when that is read.
    #[must_use]
    pub fn summary(&self) -> Option<&ComponentSummary> {
        match &self.contents {
            PluginContents::Known(summary) => Some(summary),
            _ => None,
        }
    }
}

/// One added marketplace, as its tab's header shows it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MarketplaceRow {
    pub name: String,
    /// Where it comes from, in one line.
    pub source: String,
    /// A git repository — one an update fetches (a directory is always
    /// current).
    pub git: bool,
    pub description: Option<String>,
    pub owner: Option<String>,
    /// The commit its checkout is at.
    pub revision: Option<String>,
    /// When it was added or last updated, in Unix seconds.
    pub updated_at: Option<i64>,
    /// How many plugins it lists.
    pub plugins: usize,
    /// How many of them are installed.
    pub installed: usize,
    /// Why its catalog could not be read (red in its header), when it could
    /// not.
    pub error: Option<String>,
    /// The entries it lists that were skipped, and why (amber).
    pub notes: Vec<String>,
}

/// A marketplace the `+ Add` tab offers, so nobody types its source to get
/// it — the official catalogs, injected by the boundary (which may replace
/// them for a run).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MarketplaceSuggestion {
    /// What adding it fetches — the text the add field takes.
    pub source: String,
    /// The name its catalog gives, which tells whether it is added.
    pub name: String,
    /// What it is, in a few words.
    pub description: String,
}

/// Everything `/plugins` shows, read by the boundary when the page opens.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PluginsSnapshot {
    /// The added marketplaces, in tab order.
    pub marketplaces: Vec<MarketplaceRow>,
    /// Every plugin of every added marketplace, in any order (the page sorts
    /// installed first, then by name) — packed, a handful of allocations
    /// whatever their number (`rows`).
    pub plugins: PluginList,
    /// The official catalogs `+ Add` offers.
    pub suggestions: Vec<MarketplaceSuggestion>,
    /// Why the plugin state could not be read, when it could not.
    pub error: Option<String>,
    /// Whether the session loads plugins (`ALTER_ZERO_PLUGINS`) — off shows
    /// a red note; the page still installs and toggles.
    pub session_enabled: bool,
}

impl PluginsSnapshot {
    /// How many plugins are installed.
    #[must_use]
    pub fn installed_count(&self) -> usize {
        self.plugins
            .iter()
            .filter(|p| p.installed().is_some())
            .count()
    }
}

/// A snapshot in its owned form — the rows as [`PluginRow`]s — for a host
/// or a test that builds one by hand; `.into()` packs it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SnapshotRows {
    pub marketplaces: Vec<MarketplaceRow>,
    pub plugins: Vec<PluginRow>,
    pub suggestions: Vec<MarketplaceSuggestion>,
    pub error: Option<String>,
    pub session_enabled: bool,
}

impl From<SnapshotRows> for PluginsSnapshot {
    fn from(rows: SnapshotRows) -> Self {
        Self {
            marketplaces: rows.marketplaces,
            plugins: rows.plugins.into(),
            suggestions: rows.suggestions,
            error: rows.error,
            session_enabled: rows.session_enabled,
        }
    }
}

// ===== the page =====

/// One of the page's tabs, in strip order: every plugin, the installed
/// ones, each added marketplace (the snapshot's order), the add tab.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub enum PluginsTab {
    #[default]
    All,
    Installed,
    /// The added marketplace of this name.
    Marketplace(String),
    /// The field that adds a marketplace, over the official catalogs.
    Add,
}

/// One action a plugin's page offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginAction {
    Install,
    Enable,
    Disable,
    Update,
    Uninstall,
    Back,
}

impl PluginAction {
    /// The action row's label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Install => "Install",
            Self::Enable => "Enable in this directory",
            Self::Disable => "Disable in this directory",
            Self::Update => "Update",
            Self::Uninstall => "Uninstall",
            Self::Back => "Back to plugins",
        }
    }
}

/// The actions `plugin`'s state affords, in display order — the primary one
/// first, `Back to plugins` last. One loaded in place is always current: it
/// has no update; one with an update leads on it.
#[must_use]
pub fn plugin_actions(plugin: PluginRef<'_>) -> Vec<PluginAction> {
    use PluginAction as A;
    match plugin.installed() {
        None if plugin.unavailable().is_none() => vec![A::Install, A::Back],
        None => vec![A::Back],
        Some(state) => {
            let toggle = if state.enabled { A::Disable } else { A::Enable };
            let can_update = plugin.unavailable().is_none() && !state.in_place;
            let mut out = Vec::with_capacity(4);
            if can_update && state.update {
                out.extend([A::Update, toggle]);
            } else {
                out.push(toggle);
                if can_update {
                    out.push(A::Update);
                }
            }
            out.push(A::Uninstall);
            out.push(A::Back);
            out
        }
    }
}

/// What a confirmation page asks about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginsConfirm {
    /// Uninstall a plugin — its data goes with it.
    Uninstall(PluginId),
    /// Remove a marketplace, uninstalling the plugins installed from it
    /// (named here).
    RemoveMarketplace { name: String, plugins: Vec<String> },
}

/// Which page the menu shows.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum PluginsScreen {
    /// The tabbed list — the add tab included.
    #[default]
    List,
    /// One plugin's page.
    Plugin(PluginId),
    /// The field that adds a marketplace from any source.
    Add,
    /// A question before something that cannot be undone.
    Confirm(PluginsConfirm),
}

/// One operation the page asks the boundary to run ([`KeyOutcome::Op`]).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PluginOp {
    /// Read the snapshot again (nothing fetched).
    Refresh,
    /// Say what a plugin contains — asked as its page opens, **not a job**
    /// (round `0`): answered with [`PluginsPage::supply_contents`].
    Inspect(PluginId),
    /// Install a plugin — or update it, when it is installed.
    Install(PluginId),
    Uninstall(PluginId),
    /// Turn an installed plugin on or off in this directory. The row has
    /// already moved; a failure moves it back.
    SetEnabled {
        id: PluginId,
        enabled: bool,
    },
    /// Add the marketplace this text names — typed, or an official catalog's
    /// source.
    AddMarketplace(String),
    /// Fetch a git marketplace again (Ctrl+U on its tab).
    UpdateMarketplace(String),
    /// Remove a marketplace, uninstalling what was installed from it.
    RemoveMarketplace(String),
}

impl PluginOp {
    /// What the page says while it runs — `installed` is whether an
    /// [`Install`](Self::Install) is an update.
    #[must_use]
    pub fn label(&self, installed: bool) -> String {
        match self {
            Self::Refresh => "Reading the marketplaces…".to_string(),
            Self::Inspect(id) => format!("Reading {}…", id.name),
            Self::Install(id) if installed => format!("Updating {}…", id.name),
            Self::Install(id) => format!("Installing {}…", id.name),
            Self::Uninstall(id) => format!("Uninstalling {}…", id.name),
            Self::SetEnabled { id, enabled: true } => format!("Enabling {}…", id.name),
            Self::SetEnabled { id, enabled: false } => format!("Disabling {}…", id.name),
            Self::AddMarketplace(source) => format!("Adding {source}…"),
            Self::UpdateMarketplace(name) => format!("Updating {name}…"),
            Self::RemoveMarketplace(name) => format!("Removing {name}…"),
        }
    }
}

/// The operation running now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginJob {
    pub op: PluginOp,
    /// Its round — what its answer must carry to be taken.
    pub round: u64,
    /// `Installing deploy…`.
    pub label: String,
    /// The boundary's latest word on it (`fetching openai/plugins`).
    pub step: Option<String>,
    /// The frame clock's reading at the first draw after it began — what
    /// its elapsed counts from.
    pub started: Option<Duration>,
}

/// What a finished operation reports.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PluginDone {
    /// What it did, for the toast an answer gets when the page it was asked
    /// from has closed — an open page shows it instead; empty for a quiet
    /// one.
    pub message: String,
    /// The marketplace an add added — the tab the page shows next.
    pub marketplace: Option<String>,
}

/// One row of the add tab.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddRow {
    /// The source typed into the field.
    Typed(String),
    /// An official catalog (an index into the snapshot's suggestions).
    Suggestion(usize),
    /// The row that opens the add page's field, for any other source — last
    /// while nothing is typed.
    Other,
}

/// The open `/plugins` page.
#[derive(Debug, Clone, Default)]
pub struct PluginsPage {
    /// What the boundary read when the page opened (or after the last job)
    /// — dropped with the page.
    pub snapshot: PluginsSnapshot,
    pub tab: PluginsTab,
    pub screen: PluginsScreen,
    /// The highlighted row of the current page: a list row, or an action.
    pub selected: usize,
    /// The list row a page was opened from, to come back to.
    pub list_selected: usize,
    /// The search on a plugin tab; the source typed on the add tab.
    pub query: String,
    /// The add page's field — the page's own, never the composer.
    pub input: LineField,
    /// The last operation's failure, said until the next key.
    pub error: Option<String>,
    /// The operation running now.
    pub job: Option<PluginJob>,
    /// The plugin whose contents the page asked for and waits on.
    pub inspecting: Option<PluginId>,
    /// The frame clock, injected per draw — what a job's elapsed counts on.
    pub clock: Duration,
    /// The wall clock in Unix seconds, injected per draw — what a
    /// marketplace's `updated 3h ago` is counted from; `0` until the first
    /// draw, which shows no age.
    pub now: i64,
    /// The newest round handed out ([`last_round`](Self::last_round)).
    rounds: u64,
}

impl PluginsPage {
    /// The tabs in strip order.
    #[must_use]
    pub fn tabs(&self) -> Vec<PluginsTab> {
        let mut tabs = vec![PluginsTab::All, PluginsTab::Installed];
        tabs.extend(
            self.snapshot
                .marketplaces
                .iter()
                .map(|market| PluginsTab::Marketplace(market.name.clone())),
        );
        tabs.push(PluginsTab::Add);
        tabs
    }

    /// The tab `delta` steps from the current one, wrapping.
    fn stepped_tab(&self, delta: isize) -> PluginsTab {
        let tabs = self.tabs();
        let at = tabs.iter().position(|tab| *tab == self.tab).unwrap_or(0);
        tabs[wrap_step(at, tabs.len(), delta)].clone()
    }

    /// The plugins the current tab lists, matching the search — installed
    /// first, then by name (codex's order); none on the add tab.
    #[must_use]
    ///
    /// Derived per call, nothing kept: the filter and the order compare in
    /// place, without a lowercase copy of any row.
    pub fn plugin_rows(&self) -> Vec<PluginRef<'_>> {
        let query = self.query.trim().to_lowercase();
        let mut rows: Vec<PluginRef<'_>> = self
            .snapshot
            .plugins
            .iter()
            .filter(|plugin| lists(&self.tab, *plugin))
            .filter(|plugin| query.is_empty() || matches_query(*plugin, &query))
            .collect();
        rows.sort_by(|a, b| {
            b.installed()
                .is_some()
                .cmp(&a.installed().is_some())
                .then_with(|| cmp_ignoring_case(a.name(), b.name()))
                .then_with(|| a.name().cmp(b.name()))
                .then_with(|| a.marketplace().cmp(b.marketplace()))
        });
        rows
    }

    /// The add tab's rows: the typed source first, then the official
    /// catalogs — all of them on an empty field, else those it names — and,
    /// while nothing is typed, the row that opens a field for any other
    /// source, so the page itself says one can be added.
    #[must_use]
    pub fn add_rows(&self) -> Vec<AddRow> {
        let typed = self.query.trim();
        let query = typed.to_lowercase();
        let suggestions = &self.snapshot.suggestions;
        let mut rows = Vec::new();
        if !typed.is_empty() && !suggestions.iter().any(|s| s.source == typed) {
            rows.push(AddRow::Typed(typed.to_string()));
        }
        rows.extend(
            suggestions
                .iter()
                .enumerate()
                .filter(|(_, suggestion)| {
                    query.is_empty()
                        || suggestion.source.to_lowercase().contains(&query)
                        || suggestion.name.to_lowercase().contains(&query)
                })
                .map(|(i, _)| AddRow::Suggestion(i)),
        );
        if typed.is_empty() {
            rows.push(AddRow::Other);
        }
        rows
    }

    /// How many rows the current tab lists with no search — what its window
    /// is sized to, so typing never changes the page's height.
    #[must_use]
    pub fn tab_total(&self) -> usize {
        match self.tab {
            // The typed source takes the place of the row for any other.
            PluginsTab::Add => self.snapshot.suggestions.len() + 1,
            _ => self
                .snapshot
                .plugins
                .iter()
                .filter(|plugin| lists(&self.tab, *plugin))
                .count(),
        }
    }

    /// How many plugins of every tab the search matches — what a tab whose
    /// rows it does not match points to (`No matching plugins here — 1 in
    /// All`). Zero with no search.
    #[must_use]
    pub fn matches_in_all(&self) -> usize {
        let query = self.query.trim().to_lowercase();
        if query.is_empty() {
            return 0;
        }
        self.snapshot
            .plugins
            .iter()
            .filter(|plugin| matches_query(*plugin, &query))
            .count()
    }

    /// How many rows the current tab shows.
    #[must_use]
    pub fn row_count(&self) -> usize {
        match self.tab {
            PluginsTab::Add => self.add_rows().len(),
            _ => self.plugin_rows().len(),
        }
    }

    /// The plugin of `id` in the snapshot.
    #[must_use]
    pub fn plugin(&self, id: &PluginId) -> Option<PluginRef<'_>> {
        self.snapshot.plugins.find(id)
    }

    /// The marketplace `name` in the snapshot.
    #[must_use]
    pub fn marketplace(&self, name: &str) -> Option<&MarketplaceRow> {
        self.snapshot
            .marketplaces
            .iter()
            .find(|market| market.name == name)
    }

    /// The marketplace whose tab is showing.
    #[must_use]
    pub fn current_marketplace(&self) -> Option<&MarketplaceRow> {
        match &self.tab {
            PluginsTab::Marketplace(name) => self.marketplace(name),
            _ => None,
        }
    }

    /// The official catalog of `index`.
    #[must_use]
    pub fn suggestion(&self, index: usize) -> Option<&MarketplaceSuggestion> {
        self.snapshot.suggestions.get(index)
    }

    /// Is the official catalog `suggestion` added?
    #[must_use]
    pub fn is_added(&self, suggestion: &MarketplaceSuggestion) -> bool {
        self.marketplace(&suggestion.name).is_some()
    }

    /// The highlighted plugin of the list, when a plugin tab is showing one.
    #[must_use]
    pub fn highlighted_plugin(&self) -> Option<PluginRef<'_>> {
        if self.screen != PluginsScreen::List {
            return None;
        }
        self.plugin_rows().get(self.selected).copied()
    }

    /// How long the running job has run, by the injected frame clock.
    #[must_use]
    pub fn job_elapsed(&self) -> Duration {
        self.job
            .as_ref()
            .and_then(|job| job.started)
            .map_or(Duration::ZERO, |started| self.clock.saturating_sub(started))
    }

    /// Is a job running that has run long enough to be said?
    #[must_use]
    pub fn job_shown(&self) -> bool {
        self.job.is_some() && self.job_elapsed() >= PLUGINS_JOB_DELAY
    }

    /// Does a key type text right now — the list's search line (the add
    /// tab's field too) or the add page's field? What shows the caret.
    #[must_use]
    pub fn takes_text(&self) -> bool {
        matches!(self.screen, PluginsScreen::List | PluginsScreen::Add)
    }

    /// How many actions the current page offers.
    fn action_count(&self) -> usize {
        match &self.screen {
            PluginsScreen::Plugin(id) => self.plugin(id).map_or(1, |p| plugin_actions(p).len()),
            PluginsScreen::Confirm(_) => 2,
            PluginsScreen::List | PluginsScreen::Add => 0,
        }
    }

    /// Seat the highlight inside the current page's rows.
    fn clamp_selection(&mut self) {
        let len = match self.screen {
            PluginsScreen::List => self.row_count(),
            _ => self.action_count(),
        };
        self.selected = self.selected.min(len.saturating_sub(1));
    }

    /// Show `tab`, its first row highlighted. The search survives a move
    /// between the plugin tabs; entering or leaving `+ Add`, whose `❯` line
    /// is a source field, clears it.
    fn show_tab(&mut self, tab: PluginsTab) {
        if (tab == PluginsTab::Add) != (self.tab == PluginsTab::Add) {
            self.query.clear();
        }
        self.tab = tab;
        self.selected = 0;
    }

    /// Move the plugin `id`'s row on or off in the page's own copy — the
    /// row answers at once; the registry write follows.
    fn set_row_enabled(&mut self, id: &PluginId, enabled: bool) {
        if let Some(at) = self.snapshot.plugins.position(id)
            && let Some(state) = self.snapshot.plugins.installed_mut(at)
        {
            state.enabled = enabled;
        }
    }
}

/// Does `tab` list `plugin` (before any search)?
fn lists(tab: &PluginsTab, plugin: PluginRef<'_>) -> bool {
    match tab {
        PluginsTab::All => true,
        PluginsTab::Installed => plugin.installed().is_some(),
        PluginsTab::Marketplace(name) => plugin.marketplace() == name,
        PluginsTab::Add => false,
    }
}

/// Does `plugin` match the lowercased `query` — by name, id, marketplace,
/// display name, description, category or keyword? Every word of it must.
fn matches_query(plugin: PluginRef<'_>, query: &str) -> bool {
    query.split_whitespace().all(|word| {
        let id = match word.split_once('@') {
            Some((name, marketplace)) => {
                ends_with_ignoring_case(plugin.name(), name)
                    && starts_with_ignoring_case(plugin.marketplace(), marketplace)
            }
            None => false,
        };
        id || [
            Some(plugin.name()),
            Some(plugin.marketplace()),
            plugin.display_name(),
            plugin.description(),
            plugin.category(),
        ]
        .into_iter()
        .flatten()
        .chain(plugin.keywords())
        .any(|field| contains_ignoring_case(field, word))
    })
}

/// `text` lowercased, a character at a time — no copy.
fn lower(text: &str) -> impl Iterator<Item = char> + '_ {
    text.chars().flat_map(char::to_lowercase)
}

/// The two in order, ignoring case.
fn cmp_ignoring_case(a: &str, b: &str) -> std::cmp::Ordering {
    lower(a).cmp(lower(b))
}

/// Does `text` start with the lowercased `prefix`, ignoring case?
fn starts_with_ignoring_case(text: &str, prefix: &str) -> bool {
    let mut text = lower(text);
    prefix.chars().all(|c| text.next() == Some(c))
}

/// Does `text` end with the lowercased `suffix`, ignoring case?
fn ends_with_ignoring_case(text: &str, suffix: &str) -> bool {
    suffix.is_empty()
        || text
            .char_indices()
            .any(|(at, _)| lower(&text[at..]).eq(suffix.chars()))
}

/// Does `text` hold the lowercased `word`, ignoring case?
fn contains_ignoring_case(text: &str, word: &str) -> bool {
    word.is_empty()
        || text
            .char_indices()
            .any(|(at, _)| starts_with_ignoring_case(&text[at..], word))
}

/// The tab a page opens on: what is installed; else everything there is;
/// else where a marketplace is added.
fn first_tab(snapshot: &PluginsSnapshot) -> PluginsTab {
    if snapshot.installed_count() > 0 {
        PluginsTab::Installed
    } else if snapshot.plugins.is_empty() && snapshot.marketplaces.is_empty() {
        PluginsTab::Add
    } else {
        PluginsTab::All
    }
}

/// What a key on the page asks of its host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyOutcome {
    /// Nothing beyond the page's own redraw.
    None,
    /// Close the page (Esc from a list with nothing to clear, or Ctrl+C) —
    /// and stop a job still running.
    Close,
    /// Esc while a job runs: the page has let go of it; stop it.
    CancelJob,
    /// Run `op`, already marked running under `round` — `0` for
    /// [`PluginOp::Inspect`], which is no job.
    Op { op: PluginOp, round: u64 },
}

/// The add page's field: one line of text and a caret — the page's own,
/// never the composer's. A paste is typed at the caret.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LineField {
    text: String,
    /// A byte offset on a char boundary.
    caret: usize,
}

impl LineField {
    /// What it holds.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Is it empty?
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// The caret, as a byte offset.
    #[must_use]
    pub const fn caret(&self) -> usize {
        self.caret
    }

    /// Type `c` at the caret.
    pub fn insert_char(&mut self, c: char) {
        self.text.insert(self.caret, c);
        self.caret += c.len_utf8();
    }

    /// Type `s` at the caret — a paste, its line breaks already folded.
    pub fn insert_str(&mut self, s: &str) {
        self.text.insert_str(self.caret, s);
        self.caret += s.len();
    }

    /// Backspace.
    pub fn delete_backward(&mut self) {
        if let Some(c) = self.text[..self.caret].chars().next_back() {
            self.caret -= c.len_utf8();
            self.text.remove(self.caret);
        }
    }

    /// Delete.
    pub fn delete_forward(&mut self) {
        if self.caret < self.text.len() {
            self.text.remove(self.caret);
        }
    }

    pub fn move_left(&mut self) {
        if let Some(c) = self.text[..self.caret].chars().next_back() {
            self.caret -= c.len_utf8();
        }
    }

    pub fn move_right(&mut self) {
        if let Some(c) = self.text[self.caret..].chars().next() {
            self.caret += c.len_utf8();
        }
    }

    pub const fn move_home(&mut self) {
        self.caret = 0;
    }

    pub fn move_end(&mut self) {
        self.caret = self.text.len();
    }

    /// Empty it.
    pub fn clear(&mut self) {
        self.text.clear();
        self.caret = 0;
    }

    /// The text hard-wrapped to rows of at most `width` columns (a source
    /// has no words to break at), and the caret's `(row, column)` among them
    /// — a caret at the end of a full row starts the next one, so it always
    /// has a cell of its own.
    #[must_use]
    pub fn rows(&self, width: usize) -> (Vec<String>, (usize, usize)) {
        let width = width.max(1);
        let mut rows = vec![String::new()];
        let mut used = 0;
        let mut caret = (0, 0);
        for (at, c) in self.text.char_indices() {
            let w = c.width().unwrap_or(0);
            if used + w > width {
                rows.push(String::new());
                used = 0;
            }
            if at == self.caret {
                caret = (rows.len() - 1, used);
            }
            if let Some(row) = rows.last_mut() {
                row.push(c);
            }
            used += w;
        }
        if self.caret >= self.text.len() {
            if used >= width {
                rows.push(String::new());
                used = 0;
            }
            caret = (rows.len() - 1, used);
        }
        (rows, caret)
    }
}

/// Whether `key` is a plain character — no Ctrl, no Alt.
fn plain(key: KeyEvent) -> bool {
    !key.modifiers
        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
}

impl PluginsPage {
    /// The page over `snapshot`, which the host read just now — so the first
    /// frame already has its rows — on the tab with something on it:
    /// *Installed* when anything is, else *All*, else `+ Add`. Its jobs'
    /// rounds continue from `round_base`.
    #[must_use]
    pub fn open(snapshot: impl Into<PluginsSnapshot>, round_base: u64) -> Self {
        let snapshot = snapshot.into();
        let tab = first_tab(&snapshot);
        Self {
            snapshot,
            tab,
            rounds: round_base,
            ..Self::default()
        }
    }

    /// The newest round the page handed out — what the host keeps when the
    /// page closes and passes to the next [`open`](Self::open).
    #[must_use]
    pub const fn last_round(&self) -> u64 {
        self.rounds
    }

    /// Replace the snapshot in place — the host read it again — keeping the
    /// tab, the search, the screen and the highlighted row's plugin; and
    /// the contents learnt since the page opened, of every row whose copy
    /// is the one they were read from (not installed then and now, or the
    /// same version installed). A tab or a page whose subject is gone comes
    /// back to the list.
    pub fn update_snapshot(&mut self, snapshot: impl Into<PluginsSnapshot>) {
        let snapshot = snapshot.into();
        let highlighted = self.highlighted_plugin().map(|plugin| plugin.id());
        // What a row held is what its copy held: a preview read before an
        // install, or the copy before an update, is not carried over.
        let copy = |plugin: PluginRef<'_>| plugin.installed().map(|state| state.version.clone());
        let known: Vec<(PluginId, Option<Option<String>>, PluginContents)> = self
            .snapshot
            .plugins
            .iter()
            .filter(|plugin| *plugin.contents() != PluginContents::NotRead)
            .map(|plugin| (plugin.id(), copy(plugin), plugin.contents().clone()))
            .collect();
        self.snapshot = snapshot;
        for (id, was, contents) in known {
            let at = self.snapshot.plugins.iter().position(|plugin| {
                plugin.is(&id)
                    && *plugin.contents() == PluginContents::NotRead
                    && copy(plugin) == was
            });
            if let Some(at) = at {
                self.snapshot.plugins.set_contents(at, contents);
            }
        }
        self.settle(highlighted.as_ref());
    }

    /// The host's answer to [`PluginOp::Inspect`]: what `id` contains, kept
    /// on its row so its page and the facts under the list show it. `false`
    /// when the snapshot no longer lists it.
    pub fn supply_contents(&mut self, id: &PluginId, contents: PluginContents) -> bool {
        if self.inspecting.as_ref() == Some(id) {
            self.inspecting = None;
        }
        let Some(at) = self.snapshot.plugins.position(id) else {
            return false;
        };
        self.snapshot.plugins.set_contents(at, contents);
        true
    }

    /// The plugin whose page is open with its contents unread and not yet
    /// asked for — what the host should [`Inspect`](PluginOp::Inspect) after
    /// a job changed it (an install drops what was a preview).
    #[must_use]
    pub fn pending_inspect(&self) -> Option<PluginId> {
        let PluginsScreen::Plugin(id) = &self.screen else {
            return None;
        };
        let plugin = self.plugin(id)?;
        (*plugin.contents() == PluginContents::NotRead && self.inspecting.as_ref() != Some(id))
            .then(|| id.clone())
    }

    /// Inject the page's two clocks, per draw: `now`, the wall clock in Unix
    /// seconds (a marketplace's age), and `clock`, the frame clock a job's
    /// elapsed counts on — from the first reading after it began.
    pub fn set_clock(&mut self, now: i64, clock: Duration) {
        self.now = now;
        self.clock = clock;
        if let Some(job) = self.job.as_mut()
            && job.started.is_none()
        {
            job.started = Some(clock);
        }
    }

    /// The round of the job running, if one is — what wants the host's
    /// slow frame chain.
    #[must_use]
    pub fn job_round(&self) -> Option<u64> {
        self.job.as_ref().map(|job| job.round)
    }

    /// The host's word on the job of `round` — `fetching openai/plugins` —
    /// said after its label. Dropped unless that is the page's job.
    pub fn report_step(&mut self, round: u64, step: impl Into<String>) -> bool {
        match self.job.as_mut().filter(|job| job.round == round) {
            Some(job) => {
                job.step = Some(step.into());
                true
            }
            None => false,
        }
    }

    /// Ask for `op`: marked running here and handed to the host — or, while
    /// another job runs, refused in the status row (never a toast over the
    /// page) naming the one running. [`PluginOp::Inspect`] is no job: it goes
    /// out under round `0`, and the page keeps working.
    fn start(&mut self, op: PluginOp) -> KeyOutcome {
        if let PluginOp::Inspect(id) = &op {
            self.inspecting = Some(id.clone());
            return KeyOutcome::Op { op, round: 0 };
        }
        if let Some(job) = &self.job {
            self.error = Some(format!(
                "{} — wait for it, or Esc to cancel",
                job.label.trim_end_matches('…')
            ));
            return KeyOutcome::None;
        }
        let installed = match &op {
            PluginOp::Install(id) => self.plugin(id).is_some_and(|p| p.installed().is_some()),
            _ => false,
        };
        self.rounds += 1;
        let round = self.rounds;
        self.error = None;
        // Turning a plugin off answers on its row at once; the registry
        // write follows.
        if let PluginOp::SetEnabled { id, enabled } = &op {
            self.set_row_enabled(id, *enabled);
        }
        self.job = Some(PluginJob {
            label: op.label(installed),
            op: op.clone(),
            round,
            step: None,
            started: None,
        });
        KeyOutcome::Op { op, round }
    }

    /// The answer to the job of `round`: a fresh snapshot (when the host
    /// could read one) and what happened. Taken only while that job is still
    /// the page's; `false` for an answer that came too late — the host then
    /// toasts `outcome`, the one case a toast is raised for.
    ///
    /// What went right shows on the page itself — the row, the page's state
    /// line, the new marketplace's tab — never in a line of its own. A
    /// failure is said in red until the next key; a toggle that failed
    /// without a fresh snapshot moves its row back.
    pub fn finish_job(
        &mut self,
        round: u64,
        snapshot: Option<PluginsSnapshot>,
        outcome: &Result<PluginDone, String>,
    ) -> bool {
        if self.job.as_ref().is_none_or(|job| job.round != round) {
            return false;
        }
        let fresh = snapshot.is_some();
        if let Some(snapshot) = snapshot {
            self.update_snapshot(snapshot);
        }
        let Some(job) = self.job.take() else {
            return false;
        };
        match outcome {
            Err(error) => {
                self.error = Some(error.clone());
                if let (PluginOp::SetEnabled { id, enabled }, false) = (&job.op, fresh) {
                    self.set_row_enabled(id, !enabled);
                }
            }
            Ok(done) => {
                if let (PluginOp::AddMarketplace(_), Some(name)) = (&job.op, &done.marketplace) {
                    // The plugins it brought, straight away.
                    self.screen = PluginsScreen::List;
                    self.input.clear();
                    self.query.clear();
                    self.tab = PluginsTab::Marketplace(name.clone());
                    self.selected = 0;
                }
                // An install or an uninstall changes what the plugin's page
                // can say it holds: read it again.
                if let PluginOp::Install(id) | PluginOp::Uninstall(id) = &job.op
                    && !fresh
                    && let Some(at) = self.snapshot.plugins.position(id)
                {
                    self.snapshot
                        .plugins
                        .set_contents(at, PluginContents::NotRead);
                }
            }
        }
        self.settle(None);
        true
    }

    /// After the snapshot changed: a tab or a page whose subject is gone
    /// comes back to the list, and the highlight follows `highlighted`'s row
    /// (or stays in range).
    fn settle(&mut self, highlighted: Option<&PluginId>) {
        if !self.tabs().contains(&self.tab) {
            self.tab = PluginsTab::All;
            self.screen = PluginsScreen::List;
            self.selected = 0;
        }
        if let PluginsScreen::Plugin(id) = &self.screen
            && self.plugin(id).is_none()
        {
            self.screen = PluginsScreen::List;
            self.selected = self.list_selected;
        }
        if self.screen == PluginsScreen::List
            && let Some(id) = highlighted
            && let Some(at) = self.plugin_rows().iter().position(|p| p.is(id))
        {
            self.selected = at;
        }
        self.clamp_selection();
    }

    /// A bracketed paste: into the add page's field or the list's `❯` line
    /// — the add tab's source field, or a plugin tab's search — one line
    /// either way; swallowed anywhere else.
    pub fn paste(&mut self, pasted: &str) {
        let text: String = pasted.replace(['\n', '\r'], " ");
        let text = text.trim();
        self.error = None;
        match self.screen {
            PluginsScreen::Add => self.input.insert_str(text),
            PluginsScreen::List => {
                self.query.push_str(text);
                self.selected = 0;
            }
            _ => {}
        }
    }

    /// A key while the page is open. Ctrl+C closes, wherever the page is;
    /// Esc stops a running job before anything else; a failure is said until
    /// the next key. (Ctrl+O and Ctrl+D are the host's: it opens its two
    /// read-only views over the page before a key reaches here.)
    pub fn on_key(&mut self, key: KeyEvent) -> KeyOutcome {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && key.code == KeyCode::Char('c') {
            return KeyOutcome::Close;
        }
        if key.code == KeyCode::Esc && self.job.is_some() {
            // A toggle's row keeps what was asked: the write may have
            // landed, and the host's next snapshot says what is.
            self.job = None;
            return KeyOutcome::CancelJob;
        }
        self.error = None;
        match self.screen.clone() {
            PluginsScreen::List => self.on_key_list(key),
            PluginsScreen::Plugin(id) => self.on_key_plugin(key, &id),
            PluginsScreen::Add => self.on_key_add(key),
            PluginsScreen::Confirm(confirm) => self.on_key_confirm(key, &confirm),
        }
    }

    fn on_key_list(&mut self, key: KeyEvent) -> KeyOutcome {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let len = self.row_count();
        let last = len.saturating_sub(1);
        let adding = self.tab == PluginsTab::Add;
        match key.code {
            KeyCode::Esc => {
                if self.query.is_empty() {
                    return KeyOutcome::Close;
                }
                self.query.clear();
                self.selected = 0;
            }
            KeyCode::Left | KeyCode::BackTab => {
                let tab = self.stepped_tab(-1);
                self.show_tab(tab);
            }
            KeyCode::Right | KeyCode::Tab => {
                let tab = self.stepped_tab(1);
                self.show_tab(tab);
            }
            KeyCode::Up => self.selected = wrap_step(self.selected, len, -1),
            KeyCode::Down => self.selected = wrap_step(self.selected, len, 1),
            KeyCode::PageUp => self.selected = self.selected.saturating_sub(PAGE_STEP),
            KeyCode::PageDown => self.selected = (self.selected + PAGE_STEP).min(last),
            KeyCode::Home => self.selected = 0,
            KeyCode::End => self.selected = last,
            KeyCode::Char('u') if ctrl && adding => {
                self.query.clear();
                self.selected = 0;
            }
            KeyCode::Char('u') if ctrl => return self.update_current_marketplace(),
            KeyCode::Char('r') if ctrl => self.ask_to_remove_current_marketplace(),
            KeyCode::Enter if adding => return self.add_highlighted_marketplace(),
            KeyCode::Enter => {
                self.list_selected = self.selected;
                if let Some(plugin) = self.highlighted_plugin() {
                    let id = plugin.id();
                    let unread = *plugin.contents() == PluginContents::NotRead;
                    self.screen = PluginsScreen::Plugin(id.clone());
                    self.selected = 0;
                    if unread {
                        return self.start(PluginOp::Inspect(id));
                    }
                }
            }
            // Space types on the add tab — a path may hold one.
            KeyCode::Char(' ') if !adding => {
                let toggle = self.highlighted_plugin().and_then(|plugin| {
                    plugin.installed().map(|state| PluginOp::SetEnabled {
                        id: plugin.id(),
                        enabled: !state.enabled,
                    })
                });
                if let Some(op) = toggle {
                    return self.start(op);
                }
            }
            KeyCode::Backspace => {
                self.query.pop();
                self.selected = 0;
            }
            KeyCode::Char(c) if plain(key) => {
                self.query.push(c);
                self.selected = 0;
            }
            _ => {}
        }
        KeyOutcome::None
    }

    /// Ctrl+U on a marketplace's tab: fetch it again — a directory is read
    /// in place and is always current, which the status row says.
    fn update_current_marketplace(&mut self) -> KeyOutcome {
        let Some(market) = self.current_marketplace() else {
            return KeyOutcome::None;
        };
        if !market.git {
            self.error = Some(format!(
                "{} is a directory — it is always current",
                market.name
            ));
            return KeyOutcome::None;
        }
        let name = market.name.clone();
        self.start(PluginOp::UpdateMarketplace(name))
    }

    /// Ctrl+R on a marketplace's tab: ask before removing it, naming the
    /// plugins it uninstalls.
    fn ask_to_remove_current_marketplace(&mut self) {
        let Some(name) = self.current_marketplace().map(|market| market.name.clone()) else {
            return;
        };
        let plugins = self
            .snapshot
            .plugins
            .iter()
            .filter(|plugin| plugin.installed().is_some() && plugin.marketplace() == name)
            .map(|plugin| plugin.name().to_string())
            .collect();
        self.list_selected = self.selected;
        self.screen = PluginsScreen::Confirm(PluginsConfirm::RemoveMarketplace { name, plugins });
        // The question highlights the answer the user just chose: Enter
        // confirms it.
        self.selected = 0;
    }

    /// Enter on the add tab: add the highlighted source — or, for an
    /// official catalog already added, show its tab; on the row for any
    /// other source, the field that takes one.
    fn add_highlighted_marketplace(&mut self) -> KeyOutcome {
        match self.add_rows().get(self.selected).cloned() {
            Some(AddRow::Typed(source)) => self.start(PluginOp::AddMarketplace(source)),
            Some(AddRow::Suggestion(index)) => {
                let Some(suggestion) = self.suggestion(index).cloned() else {
                    return KeyOutcome::None;
                };
                if self.is_added(&suggestion) {
                    self.show_tab(PluginsTab::Marketplace(suggestion.name));
                    return KeyOutcome::None;
                }
                self.start(PluginOp::AddMarketplace(suggestion.source))
            }
            Some(AddRow::Other) => {
                self.list_selected = self.selected;
                self.input.clear();
                self.screen = PluginsScreen::Add;
                KeyOutcome::None
            }
            None => KeyOutcome::None,
        }
    }

    /// Back from a page to the list — on the row of the plugin it was
    /// about, wherever an answer since moved it (an install sorts a plugin
    /// in with the installed ones), or the add tab's row the field was
    /// opened from; else on the row the page was opened from.
    fn back_to_list(&mut self) {
        let subject = match &self.screen {
            PluginsScreen::Plugin(id) => self.plugin_rows().iter().position(|p| p.is(id)),
            PluginsScreen::Add => self.add_rows().iter().position(|row| *row == AddRow::Other),
            _ => None,
        };
        self.screen = PluginsScreen::List;
        let len = self.row_count();
        self.selected = subject
            .unwrap_or(self.list_selected)
            .min(len.saturating_sub(1));
    }

    /// The highlighted action of a detail page: Enter's, or a digit's.
    fn chosen_action(&mut self, key: KeyEvent, count: usize) -> Option<usize> {
        match key.code {
            KeyCode::Up => self.selected = wrap_step(self.selected, count, -1),
            KeyCode::Down => self.selected = wrap_step(self.selected, count, 1),
            KeyCode::Home => self.selected = 0,
            KeyCode::End => self.selected = count.saturating_sub(1),
            KeyCode::Enter => return Some(self.selected.min(count.saturating_sub(1))),
            KeyCode::Char(c @ '1'..='9') => {
                let index = (c as usize) - ('1' as usize);
                if index < count {
                    self.selected = index;
                    return Some(index);
                }
            }
            _ => {}
        }
        None
    }

    fn on_key_plugin(&mut self, key: KeyEvent, id: &PluginId) -> KeyOutcome {
        if matches!(key.code, KeyCode::Esc | KeyCode::Backspace) {
            self.back_to_list();
            return KeyOutcome::None;
        }
        let Some(plugin) = self.plugin(id) else {
            self.back_to_list();
            return KeyOutcome::None;
        };
        let actions = plugin_actions(plugin);
        let Some(index) = self.chosen_action(key, actions.len()) else {
            return KeyOutcome::None;
        };
        match actions[index] {
            PluginAction::Install | PluginAction::Update => {
                self.start(PluginOp::Install(id.clone()))
            }
            PluginAction::Enable => self.start(PluginOp::SetEnabled {
                id: id.clone(),
                enabled: true,
            }),
            PluginAction::Disable => self.start(PluginOp::SetEnabled {
                id: id.clone(),
                enabled: false,
            }),
            // The question highlights the answer the user just chose: Enter
            // confirms it.
            PluginAction::Uninstall => {
                self.screen = PluginsScreen::Confirm(PluginsConfirm::Uninstall(id.clone()));
                self.selected = 0;
                KeyOutcome::None
            }
            PluginAction::Back => {
                self.back_to_list();
                KeyOutcome::None
            }
        }
    }

    /// Back from a confirmation: to the plugin's page on its Uninstall row,
    /// or to the marketplace's tab on the row it was asked from.
    fn back_from_confirm(&mut self, confirm: &PluginsConfirm) {
        match confirm {
            PluginsConfirm::Uninstall(id) => {
                let at = self
                    .plugin(id)
                    .map(plugin_actions)
                    .and_then(|actions| actions.iter().position(|a| *a == PluginAction::Uninstall))
                    .unwrap_or(0);
                self.screen = PluginsScreen::Plugin(id.clone());
                self.selected = at;
            }
            PluginsConfirm::RemoveMarketplace { .. } => {
                self.screen = PluginsScreen::List;
                self.selected = self.list_selected;
                self.clamp_selection();
            }
        }
    }

    fn on_key_confirm(&mut self, key: KeyEvent, confirm: &PluginsConfirm) -> KeyOutcome {
        if matches!(key.code, KeyCode::Esc | KeyCode::Backspace) {
            self.back_from_confirm(confirm);
            return KeyOutcome::None;
        }
        match self.chosen_action(key, 2) {
            Some(0) => {
                let op = match confirm {
                    PluginsConfirm::Uninstall(id) => PluginOp::Uninstall(id.clone()),
                    PluginsConfirm::RemoveMarketplace { name, .. } => {
                        PluginOp::RemoveMarketplace(name.clone())
                    }
                };
                self.back_from_confirm(confirm);
                if let PluginsConfirm::Uninstall(_) = confirm {
                    // Back on the plugin's page, its first action
                    // highlighted — what an uninstalled plugin offers is
                    // Install.
                    self.selected = 0;
                }
                self.start(op)
            }
            Some(_) => {
                self.back_from_confirm(confirm);
                KeyOutcome::None
            }
            None => KeyOutcome::None,
        }
    }

    /// Keys on the add page: its field edits like a line, Enter adds what
    /// it holds, Esc goes back to the row it was opened from.
    fn on_key_add(&mut self, key: KeyEvent) -> KeyOutcome {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => self.back_to_list(),
            KeyCode::Enter => {
                let source = self.input.text().trim().to_string();
                if !source.is_empty() {
                    return self.start(PluginOp::AddMarketplace(source));
                }
            }
            KeyCode::Char('u') if ctrl => self.input.clear(),
            KeyCode::Char(c) if plain(key) => self.input.insert_char(c),
            KeyCode::Backspace => self.input.delete_backward(),
            KeyCode::Delete => self.input.delete_forward(),
            KeyCode::Left => self.input.move_left(),
            KeyCode::Right => self.input.move_right(),
            KeyCode::Home => self.input.move_home(),
            KeyCode::End => self.input.move_end(),
            _ => {}
        }
        KeyOutcome::None
    }
}
