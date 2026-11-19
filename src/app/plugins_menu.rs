//! The inline `/plugins` page (`docs/plugins.md` *The `/plugins` page*):
//! three tabs — the installed plugins, every plugin the marketplaces list,
//! and the marketplaces themselves — over a search line; a page per plugin
//! and per marketplace; the field that adds any marketplace; a question
//! before anything that cannot be undone.
//!
//! The pure half holds the boundary-injected catalog
//! ([`crate::plugins::PluginCatalog`], the `/mcp` snapshot seam) and the
//! page state. Every operation is a typed [`PluginOp`] the boundary runs on
//! a worker — one at a time, since they all write one registry — and whose
//! answer comes back with a fresh catalog. The job is marked here, the
//! moment it is asked for; its answer is taken only while it is still the
//! job the page is waiting on (`/usage`'s rounds).
//!
//! **The page never moves under the user** (`docs/plugins.md` *No
//! flicker*): a job is said in the hint's own row, and only once it has run
//! [`PLUGINS_JOB_DELAY`] — a quick one says nothing — and what went right
//! is never said at all: the rows and pages show it. A failure is said
//! until the next key. A reopened page shows the catalog it kept while the
//! read runs behind it.

use std::time::Duration;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::plugins::{
    CatalogMarketplace, CatalogPlugin, MarketplaceSuggestion, PluginCatalog, PluginId,
    SUGGESTED_MARKETPLACES,
};
use crate::textarea::TextArea;

use super::views::TOOL_VIEW_PAGE;
use super::{Action, App, Backtrack, wrap_step};

/// How long a job runs before the page says so — a toggle answers in a few
/// milliseconds, and a line shown for one frame is a flicker.
pub const PLUGINS_JOB_DELAY: Duration = Duration::from_millis(400);

/// The page's three tabs, in strip order — Claude Code's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginsTab {
    /// The installed plugins.
    Installed,
    /// Every plugin every added marketplace lists.
    Discover,
    /// The added marketplaces, the official ones not added yet, and the row
    /// that adds any other.
    Marketplaces,
}

impl PluginsTab {
    /// In strip order.
    pub const ALL: [Self; 3] = [Self::Installed, Self::Discover, Self::Marketplaces];

    /// The tab `delta` steps from this one, wrapping.
    #[must_use]
    pub fn step(self, delta: isize) -> Self {
        let at = Self::ALL.iter().position(|tab| *tab == self).unwrap_or(0);
        Self::ALL[wrap_step(at, Self::ALL.len(), delta)]
    }
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

/// The actions `plugin`'s state affords, in display order. One loaded in
/// place is always current: it has no update.
#[must_use]
pub fn plugin_actions(plugin: &CatalogPlugin) -> Vec<PluginAction> {
    use PluginAction as A;
    match &plugin.installed {
        None if plugin.unavailable.is_none() => vec![A::Install, A::Back],
        None => vec![A::Back],
        Some(state) => {
            let mut out = vec![if state.enabled { A::Disable } else { A::Enable }];
            if plugin.unavailable.is_none() && !state.in_place {
                out.push(A::Update);
            }
            out.push(A::Uninstall);
            out.push(A::Back);
            out
        }
    }
}

/// One action a marketplace's page offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarketplaceAction {
    Browse,
    Update,
    Remove,
    Back,
}

impl MarketplaceAction {
    /// The action row's label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Browse => "Browse its plugins",
            Self::Update => "Update",
            Self::Remove => "Remove",
            Self::Back => "Back to marketplaces",
        }
    }
}

/// The actions `market` affords, in display order: its plugins when it
/// lists any, an update for a git one (a directory is read in place and is
/// always current), removing it.
#[must_use]
pub fn marketplace_actions(market: &CatalogMarketplace) -> Vec<MarketplaceAction> {
    use MarketplaceAction as A;
    let mut out = Vec::new();
    if market.plugins > 0 {
        out.push(A::Browse);
    }
    if market.git {
        out.push(A::Update);
    }
    out.extend([A::Remove, A::Back]);
    out
}

/// What a confirmation page asks about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginsConfirm {
    /// Uninstall a plugin — its data goes with it.
    Uninstall(PluginId),
    /// Remove a marketplace, uninstalling the plugins installed from it.
    RemoveMarketplace { name: String, plugins: Vec<String> },
}

/// Which page the menu shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginsPage {
    /// The tabbed list.
    List,
    /// One plugin's page.
    Plugin(PluginId),
    /// One marketplace's page.
    Marketplace(String),
    /// The field that adds a marketplace from any source.
    Add,
    /// A question before something that cannot be undone.
    Confirm(PluginsConfirm),
}

/// One operation the page asks the boundary to run
/// ([`Action::PluginOp`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginOp {
    /// Read the catalog again.
    Refresh,
    /// Install a plugin — or update it, when it is installed.
    Install(PluginId),
    Uninstall(PluginId),
    /// Turn a plugin on or off in this directory.
    SetEnabled {
        id: PluginId,
        enabled: bool,
    },
    /// Add the marketplace this text names.
    AddMarketplace(String),
    UpdateMarketplace(String),
    RemoveMarketplace(String),
}

impl PluginOp {
    /// What the page says while it runs.
    #[must_use]
    pub fn label(&self, installed: bool) -> String {
        match self {
            Self::Refresh => "Reading the marketplaces…".to_string(),
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
    pub label: String,
}

/// What a finished operation reports.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PluginDone {
    /// What it did, for the toast an answer gets when the page it was asked
    /// from has closed — an open page shows it instead; empty for a quiet
    /// read.
    pub message: String,
    /// The marketplace an add added — what the page shows next.
    pub marketplace: Option<String>,
}

/// One row of the Marketplaces tab.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MarketplaceRow {
    /// The added marketplace of this name.
    Market(String),
    /// An official catalog not added yet — one Enter adds it.
    Suggestion(&'static MarketplaceSuggestion),
    /// The row that opens the field for any other source.
    Add,
}

/// The open `/plugins` page.
#[derive(Debug, Clone)]
pub struct PluginsMenu {
    /// The catalog, once read — or the one the session kept since.
    pub catalog: Option<PluginCatalog>,
    pub tab: PluginsTab,
    /// Whether the user chose a tab — until then, the first catalog picks
    /// the one with something on it.
    pub tab_chosen: bool,
    pub page: PluginsPage,
    /// The highlighted row of the current page: a list row, or an action.
    pub selected: usize,
    /// The list row a plugin's page was opened from, to come back to.
    pub list_selected: usize,
    /// The list's search.
    pub query: String,
    /// The add page's field.
    pub input: TextArea,
    /// The last operation's failure, said until the next key.
    pub error: Option<String>,
    /// The operation running now.
    pub job: Option<PluginJob>,
    /// How long it has run — injected per draw.
    pub job_elapsed: Duration,
    /// The wall clock in Unix seconds, injected per draw — what a
    /// marketplace's `updated 3h ago` is counted from; `0` until the first
    /// draw, which shows no age.
    pub now: i64,
    /// Whether the session loads plugins (`ALTER_ZERO_PLUGINS`).
    pub session_enabled: bool,
}

impl PluginsMenu {
    /// The tabs in strip order.
    #[must_use]
    pub const fn tabs(&self) -> [PluginsTab; 3] {
        PluginsTab::ALL
    }

    /// The plugins the current tab lists, matching the search — none on the
    /// Marketplaces tab.
    #[must_use]
    pub fn plugin_rows(&self) -> Vec<&CatalogPlugin> {
        let Some(catalog) = &self.catalog else {
            return Vec::new();
        };
        let query = self.query.to_lowercase();
        catalog
            .plugins
            .iter()
            .filter(|plugin| match self.tab {
                PluginsTab::Installed => plugin.installed.is_some(),
                PluginsTab::Discover => true,
                PluginsTab::Marketplaces => false,
            })
            .filter(|plugin| query.is_empty() || matches_query(plugin, &query))
            .collect()
    }

    /// The Marketplaces tab's rows: the added marketplaces matching the
    /// search, then the official catalogs not added yet that match it, then
    /// — whatever the search — the row that adds any other. Nothing before
    /// the first read, which says it is reading where the rows will be.
    #[must_use]
    pub fn marketplace_rows(&self) -> Vec<MarketplaceRow> {
        let Some(catalog) = &self.catalog else {
            return Vec::new();
        };
        let query = self.query.to_lowercase();
        let has = |text: &str| query.is_empty() || text.to_lowercase().contains(&query);
        let mut rows: Vec<MarketplaceRow> = catalog
            .marketplaces
            .iter()
            .filter(|market| {
                has(&market.name)
                    || has(&market.source)
                    || market.description.as_deref().is_some_and(has)
                    || market.owner.as_deref().is_some_and(has)
            })
            .map(|market| MarketplaceRow::Market(market.name.clone()))
            .collect();
        rows.extend(
            SUGGESTED_MARKETPLACES
                .iter()
                .filter(|suggestion| !self.is_added(suggestion))
                .filter(|suggestion| {
                    has(suggestion.name) || has(suggestion.source) || has(suggestion.description)
                })
                .map(MarketplaceRow::Suggestion),
        );
        rows.push(MarketplaceRow::Add);
        rows
    }

    /// How many rows the current tab shows.
    #[must_use]
    pub fn row_count(&self) -> usize {
        match self.tab {
            PluginsTab::Marketplaces => self.marketplace_rows().len(),
            PluginsTab::Installed | PluginsTab::Discover => self.plugin_rows().len(),
        }
    }

    /// The plugin of `id` in the catalog.
    #[must_use]
    pub fn plugin(&self, id: &PluginId) -> Option<&CatalogPlugin> {
        self.catalog
            .as_ref()?
            .plugins
            .iter()
            .find(|plugin| plugin.id == *id)
    }

    /// The marketplace `name` in the catalog.
    #[must_use]
    pub fn marketplace(&self, name: &str) -> Option<&CatalogMarketplace> {
        self.catalog
            .as_ref()?
            .marketplaces
            .iter()
            .find(|market| market.name == name)
    }

    /// Is the official catalog `suggestion` added?
    #[must_use]
    pub fn is_added(&self, suggestion: &MarketplaceSuggestion) -> bool {
        self.marketplace(suggestion.name).is_some()
    }

    /// The highlighted plugin of the list, when a plugin tab is showing one.
    #[must_use]
    pub fn highlighted_plugin(&self) -> Option<&CatalogPlugin> {
        if self.page != PluginsPage::List {
            return None;
        }
        self.plugin_rows().get(self.selected).copied()
    }

    /// Is a job running that has run long enough to be said?
    #[must_use]
    pub fn job_shown(&self) -> bool {
        self.job.is_some() && self.job_elapsed >= PLUGINS_JOB_DELAY
    }

    /// How many actions the current page offers.
    fn action_count(&self) -> usize {
        match &self.page {
            PluginsPage::Plugin(id) => self.plugin(id).map_or(1, |p| plugin_actions(p).len()),
            PluginsPage::Marketplace(name) => self
                .marketplace(name)
                .map_or(1, |m| marketplace_actions(m).len()),
            PluginsPage::Confirm(_) => 2,
            PluginsPage::List | PluginsPage::Add => 0,
        }
    }

    /// Seat the highlight inside the current page's rows.
    fn clamp_selection(&mut self) {
        let len = match self.page {
            PluginsPage::List => self.row_count(),
            _ => self.action_count(),
        };
        self.selected = self.selected.min(len.saturating_sub(1));
    }

    /// Show `tab`, its search cleared and its first row highlighted.
    fn show_tab(&mut self, tab: PluginsTab) {
        self.tab = tab;
        self.tab_chosen = true;
        self.query.clear();
        self.selected = 0;
    }
}

/// Does `plugin` match the lowercased `query` — by name, id, marketplace,
/// display name, description, category or keyword?
fn matches_query(plugin: &CatalogPlugin, query: &str) -> bool {
    let has = |text: &str| text.to_lowercase().contains(query);
    has(&plugin.id.key())
        || plugin.display_name.as_deref().is_some_and(has)
        || plugin.description.as_deref().is_some_and(has)
        || plugin.category.as_deref().is_some_and(has)
        || plugin.keywords.iter().any(|keyword| has(keyword))
}

impl App {
    /// Open the `/plugins` page and read the catalog: the page opens at once
    /// on the catalog the session kept (or reading, before there is one),
    /// and the boundary runs the read under the returned round.
    /// `session_enabled` is whether the session loads plugins.
    pub fn open_plugins_menu(&mut self, session_enabled: bool) -> u64 {
        self.shortcuts_open = false;
        self.command_menu = None;
        self.file_search = None;
        self.skill_picker = None;
        self.backtrack = Backtrack::default();
        self.plugin_rounds += 1;
        let round = self.plugin_rounds;
        let catalog = self
            .plugins_kept
            .as_ref()
            .map(|(_, catalog)| catalog.clone());
        let mut menu = PluginsMenu {
            catalog,
            tab: PluginsTab::Installed,
            tab_chosen: false,
            page: PluginsPage::List,
            selected: 0,
            list_selected: 0,
            query: String::new(),
            input: TextArea::new(),
            error: None,
            job: Some(PluginJob {
                label: PluginOp::Refresh.label(false),
                op: PluginOp::Refresh,
                round,
            }),
            job_elapsed: Duration::ZERO,
            now: 0,
            session_enabled,
        };
        menu.tab = first_tab(menu.catalog.as_ref());
        self.plugins_menu = Some(menu);
        round
    }

    /// Close the page (the boundary cancels a job still running).
    pub fn close_plugins_menu(&mut self) {
        self.plugins_menu = None;
    }

    /// Keep `catalog`, read under `round`, for the next time the page opens
    /// — never over one a newer round read.
    pub fn keep_plugin_catalog(&mut self, round: u64, catalog: PluginCatalog) {
        if self
            .plugins_kept
            .as_ref()
            .is_none_or(|(kept, _)| *kept <= round)
        {
            self.plugins_kept = Some((round, catalog));
        }
    }

    /// The page's two clocks, injected per draw: how long the running job
    /// has run, and the wall clock in Unix seconds.
    pub fn set_plugins_clock(&mut self, job_elapsed: Duration, now: i64) {
        if let Some(menu) = &mut self.plugins_menu {
            menu.job_elapsed = job_elapsed;
            menu.now = now;
        }
    }

    /// The round of the job running on the page, if one is.
    #[must_use]
    pub fn plugins_job_round(&self) -> Option<u64> {
        self.plugins_menu
            .as_ref()?
            .job
            .as_ref()
            .map(|job| job.round)
    }

    /// Ask for `op`: marked running here and handed to the boundary — or,
    /// while another job runs, refused with a toast saying which.
    fn start_plugin_op(&mut self, op: PluginOp) -> Action {
        let installed = match &op {
            PluginOp::Install(id) => self
                .plugins_menu
                .as_ref()
                .and_then(|menu| menu.plugin(id))
                .is_some_and(|plugin| plugin.installed.is_some()),
            _ => false,
        };
        let Some(menu) = self.plugins_menu.as_mut() else {
            return Action::None;
        };
        if let Some(job) = &menu.job {
            return Action::Toast(format!("{} — wait for it, or Esc to cancel", job.label));
        }
        self.plugin_rounds += 1;
        let round = self.plugin_rounds;
        menu.error = None;
        // Turning a plugin off answers on its row at once; the registry
        // write follows.
        if let PluginOp::SetEnabled { id, enabled } = &op
            && let Some(plugin) = menu
                .catalog
                .as_mut()
                .and_then(|catalog| catalog.plugins.iter_mut().find(|plugin| plugin.id == *id))
            && let Some(state) = plugin.installed.as_mut()
        {
            state.enabled = *enabled;
        }
        menu.job = Some(PluginJob {
            label: op.label(installed),
            op: op.clone(),
            round,
        });
        menu.job_elapsed = Duration::ZERO;
        Action::PluginOp { op, round }
    }

    /// The answer to the job of `round`: a fresh catalog (when the boundary
    /// could read one) and what happened. Taken only while that job is still
    /// the page's; `false` for an answer that came too late.
    pub fn finish_plugin_job(
        &mut self,
        round: u64,
        catalog: Option<PluginCatalog>,
        outcome: &Result<PluginDone, String>,
    ) -> bool {
        if let Some(catalog) = &catalog {
            self.keep_plugin_catalog(round, catalog.clone());
        }
        let Some(menu) = self.plugins_menu.as_mut() else {
            return false;
        };
        let Some(job) = menu.job.take_if(|job| job.round == round) else {
            return false;
        };
        let first = menu.catalog.is_none();
        if let Some(catalog) = catalog {
            menu.catalog = Some(catalog);
        }
        match outcome {
            Err(error) => menu.error = Some(error.clone()),
            Ok(done) => {
                if let (PluginOp::AddMarketplace(_), Some(name)) = (&job.op, &done.marketplace) {
                    // The plugins it brought, straight away.
                    menu.page = PluginsPage::List;
                    menu.input.clear();
                    menu.show_tab(PluginsTab::Discover);
                    menu.query = format!("@{name}");
                }
            }
        }
        // The first read picks the tab with something on it, unless the user
        // already chose.
        if first && !menu.tab_chosen {
            menu.tab = first_tab(menu.catalog.as_ref());
        }
        // A page whose subject is gone comes back to the list.
        let gone = match &menu.page {
            PluginsPage::Plugin(id) => menu.plugin(id).is_none(),
            PluginsPage::Marketplace(name) => menu.marketplace(name).is_none(),
            _ => false,
        };
        if gone {
            menu.page = PluginsPage::List;
            menu.selected = menu.list_selected;
        }
        menu.clamp_selection();
        true
    }

    /// Keys while `/plugins` is open — routed first from
    /// [`on_key`](App::on_key). Ctrl+C closes, wherever the page is.
    pub(super) fn on_key_plugins(&mut self, key: KeyEvent) -> Action {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && key.code == KeyCode::Char('c') {
            self.close_plugins_menu();
            return Action::ClosePluginsMenu;
        }
        let Some(menu) = self.plugins_menu.as_mut() else {
            return Action::None;
        };
        // Esc stops a running job before it does anything else.
        if key.code == KeyCode::Esc && menu.job.is_some() {
            return Action::CancelPluginJob;
        }
        // A failure is said until the next key.
        menu.error = None;
        match menu.page.clone() {
            PluginsPage::List => self.on_key_plugins_list(key),
            PluginsPage::Plugin(id) => self.on_key_plugin_page(key, &id),
            PluginsPage::Marketplace(name) => self.on_key_marketplace_page(key, &name),
            PluginsPage::Add => self.on_key_plugins_add(key),
            PluginsPage::Confirm(confirm) => self.on_key_plugins_confirm(key, &confirm),
        }
    }

    fn on_key_plugins_list(&mut self, key: KeyEvent) -> Action {
        let plain = !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
        let Some(menu) = self.plugins_menu.as_mut() else {
            return Action::None;
        };
        let len = menu.row_count();
        let last = len.saturating_sub(1);
        let markets = menu.tab == PluginsTab::Marketplaces;
        match key.code {
            KeyCode::Esc => {
                if menu.query.is_empty() {
                    self.close_plugins_menu();
                    return Action::ClosePluginsMenu;
                }
                menu.query.clear();
                menu.selected = 0;
            }
            KeyCode::Left | KeyCode::BackTab => menu.show_tab(menu.tab.step(-1)),
            KeyCode::Right | KeyCode::Tab => menu.show_tab(menu.tab.step(1)),
            KeyCode::Up => menu.selected = wrap_step(menu.selected, len, -1),
            KeyCode::Down => menu.selected = wrap_step(menu.selected, len, 1),
            KeyCode::PageUp => menu.selected = menu.selected.saturating_sub(TOOL_VIEW_PAGE),
            KeyCode::PageDown => menu.selected = (menu.selected + TOOL_VIEW_PAGE).min(last),
            KeyCode::Home => menu.selected = 0,
            KeyCode::End => menu.selected = last,
            KeyCode::Enter if markets => return self.enter_marketplace_row(),
            KeyCode::Enter => {
                menu.list_selected = menu.selected;
                if let Some(id) = menu.highlighted_plugin().map(|plugin| plugin.id.clone()) {
                    menu.page = PluginsPage::Plugin(id);
                    menu.selected = 0;
                }
            }
            // Space toggles on a plugin tab, and never types into a search.
            KeyCode::Char(' ') => {
                let toggle = menu.highlighted_plugin().and_then(|plugin| {
                    plugin.installed.as_ref().map(|state| PluginOp::SetEnabled {
                        id: plugin.id.clone(),
                        enabled: !state.enabled,
                    })
                });
                if let Some(op) = toggle {
                    return self.start_plugin_op(op);
                }
            }
            KeyCode::Backspace => {
                menu.query.pop();
                menu.selected = 0;
            }
            KeyCode::Char(c) if plain => {
                menu.query.push(c);
                menu.selected = 0;
            }
            _ => {}
        }
        Action::None
    }

    /// Enter on the Marketplaces tab: a marketplace's page; an official
    /// catalog added; the field for any other source — holding what was
    /// searched for, which is often the source itself.
    fn enter_marketplace_row(&mut self) -> Action {
        let Some(menu) = self.plugins_menu.as_mut() else {
            return Action::None;
        };
        menu.list_selected = menu.selected;
        match menu.marketplace_rows().get(menu.selected).cloned() {
            Some(MarketplaceRow::Market(name)) => {
                menu.page = PluginsPage::Marketplace(name);
                menu.selected = 0;
                Action::None
            }
            Some(MarketplaceRow::Suggestion(suggestion)) => {
                self.start_plugin_op(PluginOp::AddMarketplace(suggestion.source.to_string()))
            }
            Some(MarketplaceRow::Add) => {
                menu.input = TextArea::from_text(menu.query.trim());
                menu.page = PluginsPage::Add;
                Action::None
            }
            None => Action::None,
        }
    }

    /// Back from a page to the list — on the row of the plugin or the
    /// marketplace it was about, wherever an answer since moved it (an
    /// install sorts a plugin in with the installed ones), or the add row
    /// the field was opened from; else on the row the page was opened from.
    fn plugins_back_to_list(&mut self) {
        if let Some(menu) = self.plugins_menu.as_mut() {
            let subject = match &menu.page {
                PluginsPage::Plugin(id) => menu
                    .plugin_rows()
                    .iter()
                    .position(|plugin| plugin.id == *id),
                PluginsPage::Marketplace(name) => menu
                    .marketplace_rows()
                    .iter()
                    .position(|row| matches!(row, MarketplaceRow::Market(n) if n == name)),
                PluginsPage::Add => menu
                    .marketplace_rows()
                    .iter()
                    .position(|row| *row == MarketplaceRow::Add),
                _ => None,
            };
            menu.page = PluginsPage::List;
            let len = menu.row_count();
            menu.selected = subject
                .unwrap_or(menu.list_selected)
                .min(len.saturating_sub(1));
        }
    }

    /// The highlighted action of a detail page: Enter's, or a digit's.
    fn chosen_action(&mut self, key: KeyEvent, count: usize) -> Option<usize> {
        let menu = self.plugins_menu.as_mut()?;
        match key.code {
            KeyCode::Up => menu.selected = wrap_step(menu.selected, count, -1),
            KeyCode::Down => menu.selected = wrap_step(menu.selected, count, 1),
            KeyCode::Home => menu.selected = 0,
            KeyCode::End => menu.selected = count.saturating_sub(1),
            KeyCode::Enter => return Some(menu.selected),
            KeyCode::Char(c @ '1'..='9') => {
                let index = (c as usize) - ('1' as usize);
                if index < count {
                    menu.selected = index;
                    return Some(index);
                }
            }
            _ => {}
        }
        None
    }

    fn on_key_plugin_page(&mut self, key: KeyEvent, id: &PluginId) -> Action {
        if matches!(key.code, KeyCode::Esc | KeyCode::Backspace) {
            self.plugins_back_to_list();
            return Action::None;
        }
        let Some(plugin) = self
            .plugins_menu
            .as_ref()
            .and_then(|menu| menu.plugin(id))
            .cloned()
        else {
            self.plugins_back_to_list();
            return Action::None;
        };
        let actions = plugin_actions(&plugin);
        let Some(index) = self.chosen_action(key, actions.len()) else {
            return Action::None;
        };
        match actions[index] {
            PluginAction::Install | PluginAction::Update => {
                self.start_plugin_op(PluginOp::Install(id.clone()))
            }
            PluginAction::Enable => self.start_plugin_op(PluginOp::SetEnabled {
                id: id.clone(),
                enabled: true,
            }),
            PluginAction::Disable => self.start_plugin_op(PluginOp::SetEnabled {
                id: id.clone(),
                enabled: false,
            }),
            // The question highlights the answer the user just chose: Enter
            // confirms it.
            PluginAction::Uninstall => {
                if let Some(menu) = self.plugins_menu.as_mut() {
                    menu.page = PluginsPage::Confirm(PluginsConfirm::Uninstall(id.clone()));
                    menu.selected = 0;
                }
                Action::None
            }
            PluginAction::Back => {
                self.plugins_back_to_list();
                Action::None
            }
        }
    }

    fn on_key_marketplace_page(&mut self, key: KeyEvent, name: &str) -> Action {
        if matches!(key.code, KeyCode::Esc | KeyCode::Backspace) {
            self.plugins_back_to_list();
            return Action::None;
        }
        let Some(market) = self
            .plugins_menu
            .as_ref()
            .and_then(|menu| menu.marketplace(name))
            .cloned()
        else {
            self.plugins_back_to_list();
            return Action::None;
        };
        let actions = marketplace_actions(&market);
        let Some(index) = self.chosen_action(key, actions.len()) else {
            return Action::None;
        };
        match actions[index] {
            MarketplaceAction::Browse => {
                if let Some(menu) = self.plugins_menu.as_mut() {
                    menu.page = PluginsPage::List;
                    menu.show_tab(PluginsTab::Discover);
                    menu.query = format!("@{name}");
                }
                Action::None
            }
            MarketplaceAction::Update => {
                self.start_plugin_op(PluginOp::UpdateMarketplace(name.to_string()))
            }
            MarketplaceAction::Remove => {
                if let Some(menu) = self.plugins_menu.as_mut() {
                    let plugins = menu
                        .catalog
                        .iter()
                        .flat_map(|catalog| catalog.plugins.iter())
                        .filter(|plugin| {
                            plugin.installed.is_some() && plugin.id.marketplace == name
                        })
                        .map(|plugin| plugin.id.name.clone())
                        .collect();
                    menu.page = PluginsPage::Confirm(PluginsConfirm::RemoveMarketplace {
                        name: name.to_string(),
                        plugins,
                    });
                    menu.selected = 0;
                }
                Action::None
            }
            MarketplaceAction::Back => {
                self.plugins_back_to_list();
                Action::None
            }
        }
    }

    fn on_key_plugins_confirm(&mut self, key: KeyEvent, confirm: &PluginsConfirm) -> Action {
        let back = |app: &mut Self| {
            if let Some(menu) = app.plugins_menu.as_mut() {
                menu.page = match confirm {
                    PluginsConfirm::Uninstall(id) => PluginsPage::Plugin(id.clone()),
                    PluginsConfirm::RemoveMarketplace { name, .. } => {
                        PluginsPage::Marketplace(name.clone())
                    }
                };
                menu.selected = 0;
            }
        };
        if matches!(key.code, KeyCode::Esc | KeyCode::Backspace) {
            back(self);
            return Action::None;
        }
        match self.chosen_action(key, 2) {
            Some(0) => {
                let op = match confirm {
                    PluginsConfirm::Uninstall(id) => PluginOp::Uninstall(id.clone()),
                    PluginsConfirm::RemoveMarketplace { name, .. } => {
                        PluginOp::RemoveMarketplace(name.clone())
                    }
                };
                back(self);
                self.start_plugin_op(op)
            }
            Some(_) => {
                back(self);
                Action::None
            }
            None => Action::None,
        }
    }

    /// Keys on the add page: its field edits like the composer's line, Enter
    /// adds what it holds, Esc goes back to the row it was opened from.
    fn on_key_plugins_add(&mut self, key: KeyEvent) -> Action {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let plain = !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
        let Some(menu) = self.plugins_menu.as_mut() else {
            return Action::None;
        };
        match key.code {
            KeyCode::Esc => self.plugins_back_to_list(),
            KeyCode::Enter => {
                let source = menu.input.text().trim().to_string();
                if !source.is_empty() {
                    return self.start_plugin_op(PluginOp::AddMarketplace(source));
                }
            }
            KeyCode::Char('u') if ctrl => menu.input.clear(),
            KeyCode::Char(c) if plain => menu.input.insert_char(c),
            KeyCode::Backspace => menu.input.delete_backward(),
            KeyCode::Delete => menu.input.delete_forward(),
            KeyCode::Left => menu.input.move_left(),
            KeyCode::Right => menu.input.move_right(),
            KeyCode::Home => menu.input.move_home(),
            KeyCode::End => menu.input.move_end(),
            _ => {}
        }
        Action::None
    }

    /// A bracketed paste while `/plugins` is open: into the add page's field
    /// — a marketplace URL is what gets pasted — or the list's search; never
    /// the composer underneath.
    pub fn paste_into_plugins(&mut self, pasted: &str) {
        let Some(menu) = self.plugins_menu.as_mut() else {
            return;
        };
        let text: String = pasted.replace(['\n', '\r'], " ");
        let text = text.trim();
        menu.error = None;
        match menu.page {
            PluginsPage::Add => menu.input.insert_str(text),
            PluginsPage::List => {
                menu.query.push_str(text);
                menu.selected = 0;
            }
            _ => {}
        }
    }
}

/// The tab a page opens on, before the user chooses: what is installed;
/// else everything there is to install; else where a marketplace is added.
fn first_tab(catalog: Option<&PluginCatalog>) -> PluginsTab {
    match catalog {
        Some(catalog) if catalog.installed_count() > 0 => PluginsTab::Installed,
        Some(catalog) if catalog.plugins.is_empty() => PluginsTab::Marketplaces,
        Some(_) => PluginsTab::Discover,
        None => PluginsTab::Installed,
    }
}
