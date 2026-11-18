//! The inline `/plugins` page (`docs/plugins.md` *The `/plugins` page*):
//! codex's `/plugins` in this app's picker frame — three tabs over the
//! plugins of the added marketplaces, a search line, a page per plugin and
//! per marketplace, and the one field that adds a marketplace.
//!
//! The pure half holds the boundary-injected catalog
//! ([`crate::plugins::PluginCatalog`], the `/mcp` snapshot seam) and the
//! page state. Every operation is a typed [`PluginOp`] the boundary runs on
//! a worker — one at a time, since they all write one registry — and whose
//! answer comes back with a fresh catalog. The job is marked here, the
//! moment it is asked for, so the frame that took the key already says
//! what is happening; its answer is taken only while it is still the job
//! the page is waiting on (`/usage`'s rounds).

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::plugins::{CatalogMarketplace, CatalogPlugin, PluginCatalog, PluginId};
use crate::textarea::TextArea;

use super::views::TOOL_VIEW_PAGE;
use super::{Action, App, Backtrack, wrap_step};

/// The page's three tabs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginsTab {
    /// The installed plugins.
    Installed,
    /// Every plugin every added marketplace lists.
    Discover,
    /// The added marketplaces, and the row that adds one.
    Marketplaces,
}

impl PluginsTab {
    /// In strip order.
    pub const ALL: [Self; 3] = [Self::Installed, Self::Discover, Self::Marketplaces];

    /// The tab beside this one, `delta` steps over, wrapping.
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
            Self::Back => "Back",
        }
    }
}

/// The actions `plugin`'s state affords, in display order.
#[must_use]
pub fn plugin_actions(plugin: &CatalogPlugin) -> Vec<PluginAction> {
    use PluginAction as A;
    match &plugin.installed {
        None if plugin.unavailable.is_none() => vec![A::Install, A::Back],
        None => vec![A::Back],
        Some(state) => {
            let mut out = vec![if state.enabled { A::Disable } else { A::Enable }];
            if plugin.unavailable.is_none() {
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
            Self::Back => "Back",
        }
    }
}

/// The actions a marketplace affords: an update only for a git source.
#[must_use]
pub fn marketplace_actions(market: &CatalogMarketplace) -> Vec<MarketplaceAction> {
    use MarketplaceAction as A;
    let mut out = vec![A::Browse];
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
    /// The field that adds a marketplace.
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
    /// What to tell the user; empty for a quiet read.
    pub message: String,
    /// The marketplace an add added — what the page shows next.
    pub marketplace: Option<String>,
}

/// One row of the Marketplaces tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarketplaceRow {
    /// The added marketplace at this index of the catalog's list.
    Market(usize),
    /// The row that adds one.
    Add,
}

/// The open `/plugins` page.
#[derive(Debug, Clone)]
pub struct PluginsMenu {
    /// The catalog, once read.
    pub catalog: Option<PluginCatalog>,
    pub tab: PluginsTab,
    /// Whether the user chose a tab — until then, the first catalog picks
    /// the one with something on it.
    pub tab_chosen: bool,
    pub page: PluginsPage,
    /// The highlighted row of the current page: a list row, or an action.
    pub selected: usize,
    /// The list row a detail page was opened from, to come back to.
    pub list_selected: usize,
    /// The list's search.
    pub query: String,
    /// The add page's field.
    pub input: TextArea,
    /// The last operation's failure, shown on the page until the next.
    pub error: Option<String>,
    /// The operation running now.
    pub job: Option<PluginJob>,
    /// How long it has run, in seconds — injected per draw.
    pub job_secs: u64,
    /// The wall clock in Unix seconds, injected per draw — what a
    /// marketplace's `updated 3h ago` is counted from; `0` until the first
    /// draw, which shows no age.
    pub now: i64,
    /// Whether the session loads plugins (`ALTER_ZERO_PLUGINS`).
    pub session_enabled: bool,
}

impl PluginsMenu {
    /// The plugins the list shows on its tab, matching the search.
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

    /// The Marketplaces tab's rows: the marketplaces matching the search,
    /// then the row that adds one.
    #[must_use]
    pub fn marketplace_rows(&self) -> Vec<MarketplaceRow> {
        let query = self.query.to_lowercase();
        let mut rows: Vec<MarketplaceRow> = self
            .catalog
            .iter()
            .flat_map(|catalog| catalog.marketplaces.iter().enumerate())
            .filter(|(_, market)| {
                query.is_empty()
                    || market.name.to_lowercase().contains(&query)
                    || market.source.to_lowercase().contains(&query)
            })
            .map(|(index, _)| MarketplaceRow::Market(index))
            .collect();
        rows.push(MarketplaceRow::Add);
        rows
    }

    /// How many rows the list shows on its tab.
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

    /// The highlighted plugin of the list, when a plugin tab is showing one.
    #[must_use]
    pub fn highlighted_plugin(&self) -> Option<&CatalogPlugin> {
        if self.page != PluginsPage::List {
            return None;
        }
        self.plugin_rows().get(self.selected).copied()
    }

    /// How many actions the current detail page offers.
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
    /// Open the `/plugins` page, reading the catalog: the page opens at once
    /// on a placeholder, and the boundary runs the read under the returned
    /// round. `session_enabled` is whether the session loads plugins.
    pub fn open_plugins_menu(&mut self, session_enabled: bool) -> u64 {
        self.shortcuts_open = false;
        self.command_menu = None;
        self.file_search = None;
        self.skill_picker = None;
        self.backtrack = Backtrack::default();
        self.plugin_rounds += 1;
        let round = self.plugin_rounds;
        self.plugins_menu = Some(PluginsMenu {
            catalog: None,
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
            job_secs: 0,
            now: 0,
            session_enabled,
        });
        round
    }

    /// Close the page (the boundary cancels a job still running).
    pub fn close_plugins_menu(&mut self) {
        self.plugins_menu = None;
    }

    /// The page's two clocks, injected per draw: how long the running job
    /// has run, and the wall clock in Unix seconds.
    pub fn set_plugins_clock(&mut self, job_secs: u64, now: i64) {
        if let Some(menu) = &mut self.plugins_menu {
            menu.job_secs = job_secs;
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
        menu.job_secs = 0;
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
            Ok(done) => match &job.op {
                PluginOp::AddMarketplace(_) => {
                    // The plugins it brought, straight away.
                    menu.page = PluginsPage::List;
                    menu.tab = PluginsTab::Discover;
                    menu.tab_chosen = true;
                    menu.query = done.marketplace.clone().unwrap_or_default();
                    menu.selected = 0;
                    menu.input = TextArea::new();
                }
                PluginOp::RemoveMarketplace(_) => {
                    menu.page = PluginsPage::List;
                    menu.tab = PluginsTab::Marketplaces;
                    menu.selected = 0;
                }
                _ => {}
            },
        }
        // The first read picks the tab with something on it, unless the user
        // already chose: what is installed; else what can be; else where a
        // marketplace is added.
        if first
            && !menu.tab_chosen
            && let Some(catalog) = &menu.catalog
        {
            menu.tab = if catalog.installed_count() > 0 {
                PluginsTab::Installed
            } else if catalog.plugins.is_empty() {
                PluginsTab::Marketplaces
            } else {
                PluginsTab::Discover
            };
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
        let len = match menu.page {
            PluginsPage::List => menu.row_count(),
            _ => menu.action_count(),
        };
        if len > 0 {
            menu.selected = menu.selected.min(len - 1);
        }
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
        let Some(menu) = self.plugins_menu.as_ref() else {
            return Action::None;
        };
        // Esc stops a running job before it does anything else.
        if key.code == KeyCode::Esc && menu.job.is_some() {
            return Action::CancelPluginJob;
        }
        match menu.page.clone() {
            PluginsPage::List => self.on_key_plugins_list(key),
            PluginsPage::Plugin(id) => self.on_key_plugin_page(key, &id),
            PluginsPage::Marketplace(name) => self.on_key_marketplace_page(key, &name),
            PluginsPage::Add => self.on_key_plugins_add(key),
            PluginsPage::Confirm(confirm) => self.on_key_plugins_confirm(key, &confirm),
        }
    }

    fn on_key_plugins_list(&mut self, key: KeyEvent) -> Action {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let plain = !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
        let Some(menu) = self.plugins_menu.as_mut() else {
            return Action::None;
        };
        let len = menu.row_count();
        let last = len.saturating_sub(1);
        let switch = |menu: &mut PluginsMenu, delta: isize| {
            menu.tab = menu.tab.step(delta);
            menu.tab_chosen = true;
            menu.query.clear();
            menu.selected = 0;
        };
        match key.code {
            KeyCode::Esc => {
                if menu.query.is_empty() {
                    self.close_plugins_menu();
                    return Action::ClosePluginsMenu;
                }
                menu.query.clear();
                menu.selected = 0;
            }
            KeyCode::Left | KeyCode::BackTab => switch(menu, -1),
            KeyCode::Right | KeyCode::Tab => switch(menu, 1),
            KeyCode::Up => menu.selected = wrap_step(menu.selected, len, -1),
            KeyCode::Down => menu.selected = wrap_step(menu.selected, len, 1),
            KeyCode::PageUp => menu.selected = menu.selected.saturating_sub(TOOL_VIEW_PAGE),
            KeyCode::PageDown => menu.selected = (menu.selected + TOOL_VIEW_PAGE).min(last),
            KeyCode::Home => menu.selected = 0,
            KeyCode::End => menu.selected = last,
            KeyCode::Char('r') if ctrl => return self.start_plugin_op(PluginOp::Refresh),
            KeyCode::Enter => {
                menu.list_selected = menu.selected;
                if menu.tab == PluginsTab::Marketplaces {
                    match menu.marketplace_rows().get(menu.selected).copied() {
                        Some(MarketplaceRow::Market(index)) => {
                            let name = menu
                                .catalog
                                .as_ref()
                                .map(|catalog| catalog.marketplaces[index].name.clone());
                            if let Some(name) = name {
                                menu.page = PluginsPage::Marketplace(name);
                                menu.selected = 0;
                            }
                        }
                        Some(MarketplaceRow::Add) => {
                            menu.page = PluginsPage::Add;
                            menu.input = TextArea::new();
                            menu.error = None;
                        }
                        None => {}
                    }
                } else if let Some(id) = menu.highlighted_plugin().map(|plugin| plugin.id.clone()) {
                    menu.page = PluginsPage::Plugin(id);
                    menu.selected = 0;
                }
            }
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

    /// Back from a detail page to the list — on the row of the plugin or
    /// marketplace the page was about, wherever an answer since moved it (an
    /// install sorts a plugin in with the installed ones); else on the row
    /// the page was opened from.
    fn plugins_back_to_list(&mut self) {
        if let Some(menu) = self.plugins_menu.as_mut() {
            let subject = match &menu.page {
                PluginsPage::Plugin(id) => menu
                    .plugin_rows()
                    .iter()
                    .position(|plugin| plugin.id == *id),
                PluginsPage::Marketplace(name) => {
                    let markets = menu.catalog.as_ref().map(|c| c.marketplaces.as_slice());
                    menu.marketplace_rows().iter().position(|row| {
                        matches!(row, MarketplaceRow::Market(index)
                            if markets.and_then(|m| m.get(*index)).is_some_and(|m| m.name == *name))
                    })
                }
                _ => None,
            };
            menu.page = PluginsPage::List;
            menu.error = None;
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
            PluginAction::Uninstall => {
                if let Some(menu) = self.plugins_menu.as_mut() {
                    menu.page = PluginsPage::Confirm(PluginsConfirm::Uninstall(id.clone()));
                    menu.selected = 1;
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
                    menu.tab = PluginsTab::Discover;
                    menu.tab_chosen = true;
                    menu.query = name.to_string();
                    menu.selected = 0;
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
                    menu.selected = 1;
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

    fn on_key_plugins_add(&mut self, key: KeyEvent) -> Action {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let plain = !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
        let Some(menu) = self.plugins_menu.as_mut() else {
            return Action::None;
        };
        match key.code {
            KeyCode::Esc => {
                menu.page = PluginsPage::List;
                menu.tab = PluginsTab::Marketplaces;
                menu.error = None;
                menu.selected = menu.list_selected;
            }
            KeyCode::Enter => {
                let source = menu.input.text().trim().to_string();
                if !source.is_empty() {
                    return self.start_plugin_op(PluginOp::AddMarketplace(source));
                }
            }
            KeyCode::Char('u') if ctrl => {
                menu.input = TextArea::new();
                menu.error = None;
            }
            KeyCode::Char(c) if plain => {
                menu.input.insert_char(c);
                menu.error = None;
            }
            KeyCode::Backspace => {
                menu.input.delete_backward();
                menu.error = None;
            }
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
        match menu.page {
            PluginsPage::Add => {
                for c in text.chars() {
                    menu.input.insert_char(c);
                }
                menu.error = None;
            }
            PluginsPage::List => {
                menu.query.push_str(text);
                menu.selected = 0;
            }
            _ => {}
        }
    }
}
