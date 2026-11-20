//! The `alter-zero plugin …` commands (`docs/plugin-cli.md`): one plain
//! function per command over a [`PluginStore`], each doing the work,
//! printing its result and returning a [`CommandOutcome`] — the exit code
//! and what the caller must act on (the plugin directories whose trust
//! records must be forgotten).
//!
//! Every word, table and `--json` shape the shell sees is here, beside the
//! store it describes, rather than in the app: code in this crate is not
//! resident in an idle session (`docs/plugins.md` *Memory*), and the app's
//! side is a router that parses, builds the store and calls one of these.
//! Output goes to two injected writers so every line is tested with an
//! in-memory buffer.
//!
//! The contract: results on `out`; progress lines for the slow steps on
//! `err`, only when `Output::progress` says `err` is a terminal; one
//! failure line on `err` (`{bin} plugin: {sentence}`) and exit 1. Exit 2
//! is the grammar's, never returned here.

use std::collections::BTreeMap;
use std::fmt::Display;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::Serialize;

use super::cancel::CancelToken;
use super::components::{PluginComponents, PluginContribution};
use super::marketplace::CatalogEntry;
use super::state::{PluginId, PluginsFile};
use super::store::{PluginError, PluginStore, UpdateStatus};

/// Where a command writes, and how.
pub struct Output<'a> {
    /// Results.
    pub out: &'a mut dyn Write,
    /// Progress (when `Output::progress`) and failures.
    pub err: &'a mut dyn Write,
    /// Is `err` a terminal? Progress lines are written only then.
    pub progress: bool,
    /// The program name hints echo (`alter-zero`).
    pub bin: &'a str,
}

impl Output<'_> {
    fn say(&mut self, text: impl Display) {
        let _ = writeln!(self.out, "{text}");
    }

    /// One failure line on `err`, exit 1.
    fn fail(&mut self, error: impl Display) -> CommandOutcome {
        self.warn(error);
        CommandOutcome::failed()
    }

    /// One failure line on `err`, the command going on.
    fn warn(&mut self, error: impl Display) {
        let _ = writeln!(self.err, "{} plugin: {error}", self.bin);
    }

    /// The progress callback a store operation takes.
    fn progress(&mut self) -> impl FnMut(&str) + '_ {
        let on = self.progress;
        let err = &mut *self.err;
        move |step: &str| {
            if on {
                let _ = writeln!(err, "{step}…");
            }
        }
    }

    fn json(&mut self, value: &impl Serialize) {
        match serde_json::to_string_pretty(value) {
            Ok(text) => self.say(text),
            Err(error) => self.warn(error),
        }
    }
}

/// What a command did, for the caller.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommandOutcome {
    /// The exit code: 0 done, 1 failed.
    pub code: i32,
    /// Plugin directories whose trust records must be forgotten (`trust.json`
    /// is keyed by file path): every install, update and uninstall.
    pub forget_trust: Vec<PathBuf>,
    /// Hooks or MCP servers arrived that wait for the review at the next
    /// launch.
    pub review: bool,
}

impl CommandOutcome {
    fn failed() -> Self {
        Self {
            code: 1,
            ..Self::default()
        }
    }
}

/// The line an install or update that brought something that runs ends on.
pub const REVIEW_NOTICE: &str = "Its hooks and MCP servers wait for your review: \
     the app asks before they run, when it next starts.";

/// `{n} {noun}`, plural past one.
fn counted(n: usize, noun: &str) -> String {
    if n == 1 {
        format!("{n} {noun}")
    } else {
        format!("{n} {noun}s")
    }
}

/// How long ago `then` was, at `now` (Unix seconds).
#[must_use]
pub fn age(now: u64, then: u64) -> String {
    let secs = now.saturating_sub(then);
    match secs {
        0..60 => "just now".into(),
        60..3600 => format!("{}m ago", secs / 60),
        3600..86_400 => format!("{}h ago", secs / 3600),
        _ => format!("{}d ago", secs / 86_400),
    }
}

/// `text` cut to `max` characters, ending on `…` when cut.
fn clip(text: &str, max: usize) -> String {
    let line = text.lines().next().unwrap_or_default().trim();
    if line.chars().count() <= max {
        return line.to_string();
    }
    let kept: String = line.chars().take(max.saturating_sub(1)).collect();
    format!("{}…", kept.trim_end())
}

/// The widest cell a column is padded to: a longer one (a catalog has names
/// of sixty characters) runs past its column rather than pushing every
/// other row's description off the screen.
const COLUMN_CAP: usize = 40;

/// Rows of cells, each column padded to its widest cell up to
/// [`COLUMN_CAP`], two spaces between, trailing blanks off.
fn table(rows: &[Vec<String>], indent: &str) -> Vec<String> {
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
    let widths: Vec<usize> = (0..columns)
        .map(|c| {
            rows.iter()
                .filter_map(|row| row.get(c))
                .map(|cell| cell.chars().count())
                .filter(|width| *width <= COLUMN_CAP)
                .max()
                .unwrap_or(0)
        })
        .collect();
    rows.iter()
        .map(|row| {
            let mut line = indent.to_string();
            for (c, cell) in row.iter().enumerate() {
                line.push_str(cell);
                if c + 1 < row.len() {
                    let pad = widths[c].saturating_sub(cell.chars().count()) + 2;
                    line.push_str(&" ".repeat(pad));
                }
            }
            line.trim_end().to_string()
        })
        .collect()
}

/// A plugin's state, in the `/plugins` page's vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Status {
    Enabled,
    Disabled,
    NotLoaded,
    Available,
    Unavailable,
}

impl Status {
    const fn label(self) -> &'static str {
        match self {
            Self::Enabled => "enabled",
            Self::Disabled => "disabled",
            Self::NotLoaded => "not loaded",
            Self::Available => "available",
            Self::Unavailable => "unavailable",
        }
    }
}

// ===== installing, removing, toggling =====

/// `plugin install <plugin>[@<marketplace>]`: add an offered marketplace on
/// the way when the spec names one, install, and say what arrived.
pub fn install(store: &PluginStore, spec: &str, io: &mut Output<'_>) -> CommandOutcome {
    let cancel = CancelToken::new();
    let result = store.install(spec, &mut io.progress(), &cancel);
    let outcome = match result {
        Ok(outcome) => outcome,
        Err(error) => return io.fail(error),
    };
    if let Some(name) = &outcome.added_marketplace {
        io.say(format!("Added marketplace {name} on the way"));
    }
    let verb = if outcome.replaced {
        "Reinstalled"
    } else {
        "Installed"
    };
    let version = outcome
        .version
        .as_deref()
        .map(|v| format!(" {v}"))
        .unwrap_or_default();
    io.say(format!("{verb} {}{version}", outcome.id));
    io.say(format!("  {}", outcome.components.line()));
    if outcome.in_place {
        io.say(format!(
            "  Loaded in place from {}",
            outcome.plugin_dir.display()
        ));
    }
    for notice in &outcome.notices {
        io.say(format!("  note: {notice}"));
    }
    let review = outcome.components.runs_code();
    if review {
        io.say(format!("  {REVIEW_NOTICE}"));
    }
    CommandOutcome {
        code: 0,
        forget_trust: vec![outcome.plugin_dir],
        review,
    }
}

/// `plugin uninstall <plugin>[@<marketplace>]`.
pub fn uninstall(store: &PluginStore, spec: &str, io: &mut Output<'_>) -> CommandOutcome {
    let result = store
        .resolve_installed(spec)
        .and_then(|id| store.uninstall(&id));
    match result {
        Ok(outcome) => {
            if outcome.in_place {
                io.say(format!(
                    "Uninstalled {} (its folder {} is left as it is)",
                    outcome.id,
                    outcome.plugin_dir.display()
                ));
            } else {
                io.say(format!("Uninstalled {}", outcome.id));
            }
            CommandOutcome {
                code: 0,
                forget_trust: vec![outcome.plugin_dir],
                review: false,
            }
        }
        Err(error) => io.fail(error),
    }
}

/// `plugin enable` / `plugin disable`: on or off in `project`, the directory
/// the command runs in.
pub fn set_enabled(
    store: &PluginStore,
    spec: &str,
    project: &Path,
    enabled: bool,
    io: &mut Output<'_>,
) -> CommandOutcome {
    let result = store
        .resolve_installed(spec)
        .and_then(|id| store.set_enabled(project, &id, enabled).map(|()| id));
    match result {
        Ok(id) if enabled => {
            io.say(format!("Enabled {id} in {}", project.display()));
            CommandOutcome::default()
        }
        Ok(id) => {
            io.say(format!(
                "Disabled {id} in {} (it stays installed)",
                project.display()
            ));
            CommandOutcome::default()
        }
        Err(error) => io.fail(error),
    }
}

/// `plugin update [<plugin>]`: one installed plugin, or every one, each
/// reported (`up to date`, `1.0.0 → 1.1.0`).
pub fn update(
    store: &PluginStore,
    spec: Option<&str>,
    project: &Path,
    io: &mut Output<'_>,
) -> CommandOutcome {
    let cancel = CancelToken::new();
    let result = store.update(spec, &mut io.progress(), &cancel);
    let updates = match result {
        Ok(updates) => updates,
        Err(error) => return io.fail(error),
    };
    if updates.is_empty() {
        io.say("No plugins installed");
        return CommandOutcome::default();
    }
    let mut outcome = CommandOutcome::default();
    let rows: Vec<Vec<String>> = updates
        .iter()
        .map(|update| {
            let what = match &update.status {
                UpdateStatus::UpToDate => "up to date".to_string(),
                UpdateStatus::Updated { old, new } => format!("{old} → {new}"),
            };
            vec![update.id.to_string(), what]
        })
        .collect();
    for line in table(&rows, "") {
        io.say(line);
    }
    for update in updates {
        if matches!(update.status, UpdateStatus::Updated { .. }) {
            if store
                .inspect(&update.id, project)
                .is_ok_and(|c| c.summary().runs_code())
            {
                outcome.review = true;
            }
            outcome.forget_trust.push(update.plugin_dir);
        }
    }
    if outcome.review {
        io.say(REVIEW_NOTICE);
    }
    outcome
}

// ===== listing and reading =====

/// One plugin as `list` and `--json` report it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ListedPlugin {
    /// `name@marketplace`.
    pub id: String,
    /// The plugin's name.
    pub name: String,
    /// Its marketplace.
    pub marketplace: String,
    /// `enabled`, `disabled`, `not loaded`, `available` or `unavailable`.
    pub state: String,
    /// The installed version, else the one the catalog offers.
    pub version: Option<String>,
    /// Its description.
    pub description: Option<String>,
    /// What it adds (`1 skill · 1 hook`), for an installed one that loads.
    pub summary: Option<String>,
    /// Why it is not loaded, or unavailable.
    pub reason: Option<String>,
}

/// A marketplace whose catalog could not be read, in `list --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ListError {
    /// The marketplace.
    pub marketplace: String,
    /// Why.
    pub error: String,
}

/// `plugin list --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PluginList {
    /// The plugins, by marketplace then catalog order (installed ones by id
    /// without `--available`).
    pub plugins: Vec<ListedPlugin>,
    /// Catalogs that could not be read (`--available` only).
    pub errors: Vec<ListError>,
}

fn installed_row(
    store: &PluginStore,
    state: &PluginsFile,
    id: &PluginId,
    project: &Path,
) -> ListedPlugin {
    let record = state.plugin(id).cloned().unwrap_or_default();
    let mut row = ListedPlugin {
        id: id.to_string(),
        name: id.name.clone(),
        marketplace: id.marketplace.clone(),
        state: String::new(),
        version: record.version,
        description: None,
        summary: None,
        reason: None,
    };
    let status = match store.inspect(id, project) {
        Ok(contribution) => {
            row.description = contribution.metadata.description.clone();
            row.summary = Some(contribution.summary().line());
            if state.is_enabled(&project.to_string_lossy(), id) {
                Status::Enabled
            } else {
                Status::Disabled
            }
        }
        Err(error) => {
            row.reason = Some(error.to_string());
            Status::NotLoaded
        }
    };
    row.state = status.label().into();
    row
}

fn listed_row(id: &PluginId, entry: &CatalogEntry) -> ListedPlugin {
    let status = if entry.unavailable.is_some() || entry.source.is_none() {
        Status::Unavailable
    } else {
        Status::Available
    };
    ListedPlugin {
        id: id.to_string(),
        name: id.name.clone(),
        marketplace: id.marketplace.clone(),
        state: status.label().into(),
        version: entry.version.clone(),
        description: entry.description.clone(),
        summary: None,
        reason: (status == Status::Unavailable).then(|| {
            entry
                .unavailable
                .clone()
                .unwrap_or_else(|| "its source is not supported".into())
        }),
    }
}

/// What `list` finds: the installed plugins, or (`available`) every
/// marketplace's catalog with the installed ones in their places.
///
/// # Errors
/// `plugins.json` will not parse.
pub fn plugin_list(
    store: &PluginStore,
    project: &Path,
    available: bool,
) -> Result<PluginList, PluginError> {
    let state = store.state()?;
    let mut list = PluginList {
        plugins: Vec::new(),
        errors: Vec::new(),
    };
    if !available {
        for id in state.installed() {
            list.plugins
                .push(installed_row(store, &state, &id, project));
        }
        return Ok(list);
    }
    for name in state.marketplaces.keys() {
        let mut installed = state.plugins_of(name);
        match store.catalog(name) {
            Ok(catalog) => {
                for entry in &catalog.entries {
                    let Ok(id) = PluginId::new(&entry.name, name) else {
                        continue;
                    };
                    if let Some(at) = installed.iter().position(|i| *i == id) {
                        installed.remove(at);
                        list.plugins
                            .push(installed_row(store, &state, &id, project));
                    } else {
                        list.plugins.push(listed_row(&id, entry));
                    }
                }
            }
            Err(error) => list.errors.push(ListError {
                marketplace: name.clone(),
                error: error.to_string(),
            }),
        }
        // Installed, and no longer listed (or the catalog would not read).
        for id in installed {
            list.plugins
                .push(installed_row(store, &state, &id, project));
        }
    }
    // Installed from a marketplace that is no longer added.
    for id in state.installed() {
        if !state.marketplaces.contains_key(&id.marketplace) {
            list.plugins
                .push(installed_row(store, &state, &id, project));
        }
    }
    Ok(list)
}

/// The width a description is cut to in `list`.
const LIST_DESCRIPTION_CHARS: usize = 60;

/// `plugin list [--available] [--json]`.
pub fn list(
    store: &PluginStore,
    project: &Path,
    available: bool,
    json: bool,
    io: &mut Output<'_>,
) -> CommandOutcome {
    let list = match plugin_list(store, project, available) {
        Ok(list) => list,
        Err(error) => return io.fail(error),
    };
    if json {
        io.json(&list);
        return CommandOutcome::default();
    }
    if !available {
        if list.plugins.is_empty() {
            io.say(format!(
                "No plugins installed — see what is offered: {} plugin list --available",
                io.bin
            ));
            return CommandOutcome::default();
        }
        let rows: Vec<Vec<String>> = list
            .plugins
            .iter()
            .map(|p| {
                vec![
                    p.id.clone(),
                    p.version.clone().unwrap_or_default(),
                    p.state.clone(),
                    p.summary
                        .clone()
                        .or_else(|| p.reason.clone())
                        .unwrap_or_default(),
                ]
            })
            .collect();
        for line in table(&rows, "") {
            io.say(line);
        }
        return CommandOutcome::default();
    }
    if list.plugins.is_empty() && list.errors.is_empty() {
        io.say(format!(
            "No marketplace is added — add the default ones: {} plugin marketplace add --defaults",
            io.bin
        ));
        return CommandOutcome::default();
    }
    let mut groups: Vec<(String, Vec<&ListedPlugin>)> = Vec::new();
    for plugin in &list.plugins {
        match groups.iter_mut().find(|(m, _)| *m == plugin.marketplace) {
            Some((_, rows)) => rows.push(plugin),
            None => groups.push((plugin.marketplace.clone(), vec![plugin])),
        }
    }
    let mut first = true;
    for error in &list.errors {
        if !groups.iter().any(|(m, _)| *m == error.marketplace) {
            groups.push((error.marketplace.clone(), Vec::new()));
        }
    }
    for (marketplace, rows) in groups {
        if !first {
            io.say("");
        }
        first = false;
        match list.errors.iter().find(|e| e.marketplace == marketplace) {
            Some(error) => io.say(format!("{marketplace} — {}", error.error)),
            None => io.say(format!("{marketplace} — {}", counted(rows.len(), "plugin"))),
        }
        let cells: Vec<Vec<String>> = rows
            .iter()
            .map(|p| {
                let about = match p.state.as_str() {
                    "not loaded" | "unavailable" => p.reason.clone().unwrap_or_default(),
                    _ => p.description.clone().unwrap_or_default(),
                };
                vec![
                    p.name.clone(),
                    p.version.clone().unwrap_or_default(),
                    p.state.clone(),
                    clip(&about, LIST_DESCRIPTION_CHARS),
                ]
            })
            .collect();
        for line in table(&cells, "  ") {
            io.say(line);
        }
    }
    CommandOutcome::default()
}

/// A plugin's components in `info --json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct InfoComponents {
    /// Skill names as a session uses them.
    pub skills: Vec<String>,
    /// Command names.
    pub commands: Vec<String>,
    /// Agent names.
    pub agents: Vec<String>,
    /// Hook events and their handler counts.
    pub hooks: BTreeMap<String, usize>,
    /// Hooks files known to exist but not counted (a plugin not installed).
    pub hook_files: usize,
    /// MCP server names (for a plugin not installed, the files).
    pub mcp_servers: Vec<String>,
}

impl From<&PluginComponents> for InfoComponents {
    fn from(c: &PluginComponents) -> Self {
        Self {
            skills: c.skills.clone(),
            commands: c.commands.clone(),
            agents: c.agents.clone(),
            hooks: c.hooks.iter().cloned().collect(),
            hook_files: c.hook_files,
            mcp_servers: c.mcp_servers.clone(),
        }
    }
}

/// `plugin info --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PluginInfo {
    /// `name@marketplace`.
    pub id: String,
    /// The plugin's name.
    pub name: String,
    /// Its marketplace.
    pub marketplace: String,
    /// `enabled`, `disabled`, `not loaded`, `available` or `unavailable` —
    /// in the directory the command ran in.
    pub state: String,
    /// The installed version, else the offered one.
    pub version: Option<String>,
    /// The display name, when it has one.
    pub display_name: Option<String>,
    /// Its description.
    pub description: Option<String>,
    /// Where the marketplace entry fetches it from.
    pub source: Option<String>,
    /// The manifest format it reads as (installed ones).
    pub format: Option<String>,
    /// The directory it loads from (installed ones).
    pub directory: Option<String>,
    /// What it adds; `None` when that is known only once installed.
    pub components: Option<InfoComponents>,
    /// What it ships that this app does not use.
    pub not_used: Vec<String>,
    /// Notices from reading it.
    pub notices: Vec<String>,
    /// Why it is not loaded, or unavailable.
    pub reason: Option<String>,
}

/// What `info` finds about `spec`: an installed plugin as a session in
/// `project` would load it, else a listed one from its catalog.
///
/// # Errors
/// Nothing installed or listed answers to `spec`, or `plugins.json` will
/// not parse.
pub fn plugin_info(
    store: &PluginStore,
    spec: &str,
    project: &Path,
) -> Result<PluginInfo, PluginError> {
    let (id, installed) = match store.resolve_installed(spec) {
        Ok(id) => (id, true),
        Err(PluginError::NotInstalled(_)) => (store.resolve_available(spec)?, false),
        Err(error) => return Err(error),
    };
    let entry = store
        .catalog(&id.marketplace)
        .ok()
        .and_then(|catalog| catalog.entry(&id.name).cloned());
    let mut info = PluginInfo {
        id: id.to_string(),
        name: id.name.clone(),
        marketplace: id.marketplace.clone(),
        state: String::new(),
        version: entry.as_ref().and_then(|e| e.version.clone()),
        display_name: entry.as_ref().and_then(|e| e.display_name.clone()),
        description: entry.as_ref().and_then(|e| e.description.clone()),
        source: entry
            .as_ref()
            .and_then(|e| e.source.as_ref())
            .map(super::marketplace::PluginSource::label),
        format: None,
        directory: None,
        components: None,
        not_used: Vec::new(),
        notices: Vec::new(),
        reason: None,
    };
    if installed {
        let state = store.state()?;
        if let Some(record) = state.plugin(&id) {
            info.version = record.version.clone().or(info.version);
        }
        match store.inspect(&id, project) {
            Ok(contribution) => {
                fill_from_contribution(&mut info, &contribution);
                let on = state.is_enabled(&project.to_string_lossy(), &id);
                info.state = if on {
                    Status::Enabled
                } else {
                    Status::Disabled
                }
                .label()
                .into();
            }
            Err(error) => {
                info.state = Status::NotLoaded.label().into();
                info.reason = Some(error.to_string());
            }
        }
        return Ok(info);
    }
    let Some(entry) = entry else {
        return Err(PluginError::NotListed {
            plugin: id.name.clone(),
            marketplace: id.marketplace.clone(),
        });
    };
    if entry.unavailable.is_some() || entry.source.is_none() {
        info.state = Status::Unavailable.label().into();
        info.reason = Some(
            entry
                .unavailable
                .clone()
                .unwrap_or_else(|| "its source is not supported".into()),
        );
        return Ok(info);
    }
    info.state = Status::Available.label().into();
    match store.preview(&id, &CancelToken::new()) {
        Ok(Some(components)) => {
            info.components = Some((&components).into());
            info.not_used = components.not_used;
            info.notices = components.notices;
        }
        Ok(None) => {}
        Err(error) => info.notices.push(error.to_string()),
    }
    Ok(info)
}

fn fill_from_contribution(info: &mut PluginInfo, contribution: &PluginContribution) {
    let summary = contribution.summary();
    let metadata = &contribution.metadata;
    info.version = metadata.version.clone().or(info.version.take());
    info.display_name = metadata.display_name.clone().or(info.display_name.take());
    info.description = metadata.description.clone().or(info.description.take());
    info.format = Some(contribution.format.label().into());
    info.directory = Some(contribution.root.display().to_string());
    info.components = Some((&summary).into());
    info.not_used = summary.not_used;
    info.notices = summary.notices;
}

/// The column `info`'s values start at.
const INFO_LABEL_WIDTH: usize = 13;

/// `plugin info <plugin>[@<marketplace>] [--json]`.
pub fn info(
    store: &PluginStore,
    spec: &str,
    project: &Path,
    json: bool,
    io: &mut Output<'_>,
) -> CommandOutcome {
    let info = match plugin_info(store, spec, project) {
        Ok(info) => info,
        Err(error) => return io.fail(error),
    };
    if json {
        io.json(&info);
        return CommandOutcome::default();
    }
    let state = match info.state.as_str() {
        "enabled" | "disabled" => format!("{} in this directory", info.state),
        other => other.to_string(),
    };
    io.say(format!("{} · {state}", info.id));
    let mut about = Vec::new();
    if let Some(name) = &info.display_name
        && !name.eq_ignore_ascii_case(&info.name)
    {
        about.push(name.clone());
    }
    if let Some(description) = &info.description {
        about.push(description.clone());
    }
    for text in about {
        for line in text.lines() {
            io.say(format!("  {line}").trim_end());
        }
    }
    io.say("");
    let mut field = |label: &str, values: &[String]| {
        for (i, value) in values.iter().enumerate() {
            let label = if i == 0 { label } else { "" };
            io.say(format!("  {label:<INFO_LABEL_WIDTH$}{value}"));
        }
    };
    let one = |value: &Option<String>| value.iter().cloned().collect::<Vec<_>>();
    field("Version", &one(&info.version));
    field("Marketplace", std::slice::from_ref(&info.marketplace));
    field("Source", &one(&info.source));
    field("Format", &one(&info.format));
    field("Directory", &one(&info.directory));
    let label = match info.state.as_str() {
        "not loaded" => "Not loaded",
        "unavailable" => "Unavailable",
        _ => "Reason",
    };
    field(label, &one(&info.reason));
    match &info.components {
        Some(c) => {
            let none = |list: &[String]| {
                if list.is_empty() {
                    vec!["none".to_string()]
                } else {
                    list.to_vec()
                }
            };
            field("Skills", &none(&c.skills));
            field("Commands", &none(&c.commands));
            field("Agents", &none(&c.agents));
            let mut hooks: Vec<String> = c
                .hooks
                .iter()
                .map(|(event, n)| format!("{event} ({})", counted(*n, "handler")))
                .collect();
            if c.hook_files > 0 {
                hooks.push(counted(c.hook_files, "hooks file"));
            }
            field("Hooks", &none(&hooks));
            field("MCP servers", &none(&c.mcp_servers));
        }
        None if matches!(info.state.as_str(), "available") => {
            field("Contents", &["known once installed".to_string()]);
        }
        None => {}
    }
    field("Not used", &info.not_used);
    field("Notices", &info.notices);
    CommandOutcome::default()
}

// ===== marketplaces =====

/// `plugin marketplace add <source> [--ref <ref>]`.
pub fn marketplace_add(
    store: &PluginStore,
    source: &str,
    git_ref: Option<&str>,
    io: &mut Output<'_>,
) -> CommandOutcome {
    let cancel = CancelToken::new();
    let result = store.add_marketplace(source, git_ref, &mut io.progress(), &cancel);
    match result {
        Ok(added) => {
            say_added(io, &added.name, added.plugins, added.already);
            CommandOutcome::default()
        }
        Err(error) => io.fail(error),
    }
}

fn say_added(io: &mut Output<'_>, name: &str, plugins: usize, already: bool) {
    if already {
        io.say(format!(
            "Marketplace {name} is already added ({})",
            counted(plugins, "plugin")
        ));
    } else {
        io.say(format!(
            "Added marketplace {name} ({})",
            counted(plugins, "plugin")
        ));
    }
}

/// `plugin marketplace add --defaults`: every offered marketplace not yet
/// added, each on its own — one that fails is reported and the rest added.
pub fn marketplace_add_defaults(store: &PluginStore, io: &mut Output<'_>) -> CommandOutcome {
    let cancel = CancelToken::new();
    let results = store.add_default_marketplaces(&mut io.progress(), &cancel);
    if results.is_empty() {
        io.say("Every offered marketplace is already added");
        return CommandOutcome::default();
    }
    let mut outcome = CommandOutcome::default();
    for (source, result) in results {
        match result {
            Ok(added) => say_added(io, &added.name, added.plugins, added.already),
            Err(error) => {
                io.warn(format!("{source}: {error}"));
                outcome.code = 1;
            }
        }
    }
    outcome
}

/// One marketplace in `marketplace list --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ListedMarketplace {
    /// Its name (`None` for an offered source whose name is unknown).
    pub name: Option<String>,
    /// Its source, as recorded.
    pub source: String,
    /// Is it added?
    pub added: bool,
    /// Is it one of the offered defaults?
    pub default: bool,
    /// Is it a folder read in place (not a clone)?
    pub local: bool,
    /// How many plugins its catalog lists (added ones whose catalog reads).
    pub plugins: Option<usize>,
    /// How many plugins are installed from it.
    pub installed: usize,
    /// When it was last fetched, in Unix seconds.
    pub updated: Option<u64>,
    /// The commit its clone is at.
    pub revision: Option<String>,
    /// Why its catalog would not read.
    pub error: Option<String>,
}

/// `plugin marketplace list --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MarketplaceList {
    /// The added marketplaces, then the offered ones not yet added.
    pub marketplaces: Vec<ListedMarketplace>,
}

/// What `marketplace list` finds.
///
/// # Errors
/// `plugins.json` will not parse.
pub fn marketplace_listing(store: &PluginStore) -> Result<MarketplaceList, PluginError> {
    let mut out = Vec::new();
    for listing in store.list_marketplaces()? {
        let (plugins, error) = match (&listing.name, listing.added) {
            (Some(name), true) => match store.catalog(name) {
                Ok(catalog) => (Some(catalog.entries.len()), None),
                Err(error) => (None, Some(error.to_string())),
            },
            _ => (None, None),
        };
        out.push(ListedMarketplace {
            local: listing.added && Path::new(&listing.source).is_absolute(),
            name: listing.name,
            source: listing.source,
            added: listing.added,
            default: listing.default,
            plugins,
            installed: listing.installed,
            updated: listing.updated,
            revision: listing.revision,
            error,
        });
    }
    Ok(MarketplaceList { marketplaces: out })
}

/// `plugin marketplace list [--json]`; `now` (Unix seconds) dates each
/// catalog.
pub fn marketplace_list(
    store: &PluginStore,
    json: bool,
    now: u64,
    io: &mut Output<'_>,
) -> CommandOutcome {
    let list = match marketplace_listing(store) {
        Ok(list) => list,
        Err(error) => return io.fail(error),
    };
    if json {
        io.json(&list);
        return CommandOutcome::default();
    }
    if list.marketplaces.is_empty() {
        io.say(format!(
            "No marketplaces added — add one: {} plugin marketplace add <source>",
            io.bin
        ));
        return CommandOutcome::default();
    }
    let rows: Vec<Vec<String>> = list
        .marketplaces
        .iter()
        .map(|m| {
            let about = if !m.added {
                format!(
                    "offered, not added — {} plugin marketplace add --defaults",
                    io.bin
                )
            } else if let Some(error) = &m.error {
                error.clone()
            } else {
                let mut parts = vec![counted(m.plugins.unwrap_or_default(), "plugin")];
                if m.installed > 0 {
                    parts.push(format!("{} installed", m.installed));
                }
                if m.local {
                    parts.push("read in place".into());
                } else if let Some(updated) = m.updated {
                    parts.push(format!("updated {}", age(now, updated)));
                }
                parts.join(" · ")
            };
            vec![
                m.name.clone().unwrap_or_else(|| "(offered)".into()),
                m.source.clone(),
                about,
            ]
        })
        .collect();
    for line in table(&rows, "") {
        io.say(line);
    }
    CommandOutcome::default()
}

/// `plugin marketplace update [<name>]`: one marketplace, or every added
/// one — each reported, one that fails reported and the rest updated.
pub fn marketplace_update(
    store: &PluginStore,
    name: Option<&str>,
    io: &mut Output<'_>,
) -> CommandOutcome {
    let names: Vec<String> = match name {
        Some(name) => vec![name.to_string()],
        None => match store.state() {
            Ok(state) => state.marketplaces.keys().cloned().collect(),
            Err(error) => return io.fail(error),
        },
    };
    if names.is_empty() {
        io.say(format!(
            "No marketplaces added — add one: {} plugin marketplace add <source>",
            io.bin
        ));
        return CommandOutcome::default();
    }
    let cancel = CancelToken::new();
    let mut outcome = CommandOutcome::default();
    for name in names {
        let result = store.update_marketplace(&name, &mut io.progress(), &cancel);
        let update = match result {
            Ok(update) => update,
            Err(error) => {
                io.warn(format!("{name}: {error}"));
                outcome.code = 1;
                continue;
            }
        };
        let plugins = counted(update.plugins, "plugin");
        let mut line = if update.new.is_none() {
            format!("Read {name} again ({plugins})")
        } else if update.changed() {
            format!("Updated {name} ({plugins})")
        } else {
            format!("{name} is up to date ({plugins})")
        };
        if !update.updates.is_empty() {
            let ids: Vec<String> = update.updates.iter().map(ToString::to_string).collect();
            line.push_str(&format!(
                " — {} to update: {} ({} plugin update)",
                counted(ids.len(), "plugin"),
                ids.join(", "),
                io.bin
            ));
        }
        io.say(line);
    }
    outcome
}

/// `plugin marketplace remove <name>`: the marketplace and every plugin
/// installed from it, named.
pub fn marketplace_remove(store: &PluginStore, name: &str, io: &mut Output<'_>) -> CommandOutcome {
    match store.remove_marketplace(name) {
        Ok(removed) => {
            if removed.uninstalled.is_empty() {
                io.say(format!("Removed marketplace {}", removed.name));
            } else {
                let ids: Vec<String> = removed
                    .uninstalled
                    .iter()
                    .map(|(id, _)| id.to_string())
                    .collect();
                io.say(format!(
                    "Removed marketplace {} and uninstalled {}: {}",
                    removed.name,
                    counted(ids.len(), "plugin"),
                    ids.join(", ")
                ));
            }
            CommandOutcome {
                code: 0,
                forget_trust: removed
                    .uninstalled
                    .into_iter()
                    .map(|(_, dir)| dir)
                    .collect(),
                review: false,
            }
        }
        Err(error) => io.fail(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::PluginsLayout;
    use crate::state::DefaultMarketplace;
    use std::process::Command;

    fn has_git() -> bool {
        Command::new("git").arg("--version").output().is_ok()
    }

    fn write(root: &Path, relative: &str, contents: &str) {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    fn git(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "init.defaultBranch=main",
            ])
            .args(args)
            .current_dir(dir)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {out:?}");
    }

    /// A marketplace named `name` at `root`: `hello` (a skill and a
    /// command, version 1.0.0), `guard` (a hook) and `npmthing` (an npm
    /// source, unavailable).
    fn marketplace(root: &Path, name: &str) {
        write(
            root,
            ".claude-plugin/marketplace.json",
            &format!(
                r#"{{"name":"{name}","owner":{{"name":"Demo"}},"plugins":[
                {{"name":"hello","source":"./plugins/hello","description":"Greets people"}},
                {{"name":"guard","source":"./plugins/guard","description":"Guards stops"}},
                {{"name":"npmthing","source":{{"source":"npm","package":"x"}}}}]}}"#
            ),
        );
        write(
            root,
            "plugins/hello/.claude-plugin/plugin.json",
            r#"{"name":"hello","version":"1.0.0","description":"Greets people"}"#,
        );
        write(
            root,
            "plugins/hello/skills/greet/SKILL.md",
            "---\nname: greet\ndescription: Greet.\n---\nSay hello.",
        );
        write(root, "plugins/hello/commands/hi.md", "Say hi.");
        write(
            root,
            "plugins/guard/hooks/hooks.json",
            r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"true"}]}]}}"#,
        );
    }

    struct Fixture {
        _dir: tempfile::TempDir,
        root: PathBuf,
        store: PluginStore,
        project: PathBuf,
    }

    fn fixture(defaults: Vec<DefaultMarketplace>) -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let project = root.join("work");
        std::fs::create_dir_all(&project).unwrap();
        let store = PluginStore::new(PluginsLayout::for_config_home(&root.join("config")))
            .with_defaults(defaults)
            .with_paths(&project, Some(root.clone()));
        Fixture {
            _dir: dir,
            root,
            store,
            project,
        }
    }

    /// A folder marketplace `demo` added.
    fn with_demo() -> Fixture {
        let f = fixture(Vec::new());
        marketplace(&f.root.join("market"), "demo");
        let (out, _, outcome) =
            run(|io| marketplace_add(&f.store, &f.root.join("market").to_string_lossy(), None, io));
        assert_eq!(outcome.code, 0, "{out}");
        f
    }

    /// Run a command; its stdout, stderr and outcome.
    fn run(
        command: impl FnOnce(&mut Output<'_>) -> CommandOutcome,
    ) -> (String, String, CommandOutcome) {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let outcome = command(&mut Output {
            out: &mut out,
            err: &mut err,
            progress: false,
            bin: "alter-zero",
        });
        (
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
            outcome,
        )
    }

    #[test]
    fn ages_read_in_the_largest_whole_unit() {
        assert_eq!(age(100, 100), "just now");
        assert_eq!(age(100, 200), "just now");
        assert_eq!(age(1000, 100), "15m ago");
        assert_eq!(age(10_000, 100), "2h ago");
        assert_eq!(age(400_000, 100), "4d ago");
    }

    #[test]
    fn a_table_pads_every_column_but_the_last() {
        let rows = vec![
            vec!["a".to_string(), "bb".into(), "x".into()],
            vec!["ccc".to_string(), String::new(), "y z".into()],
        ];
        assert_eq!(table(&rows, "  "), ["  a    bb  x", "  ccc      y z"]);
        let long = "n".repeat(COLUMN_CAP + 1);
        let rows = vec![
            vec!["a".to_string(), "x".into()],
            vec![long.clone(), "y".into()],
        ];
        assert_eq!(table(&rows, ""), ["a  x".to_string(), format!("{long}  y")]);
        assert_eq!(clip("abcdef", 4), "abc…");
        assert_eq!(clip("abc\nmore", 4), "abc");
    }

    #[test]
    fn adding_a_folder_marketplace_counts_its_plugins_and_again_says_so() {
        let f = fixture(Vec::new());
        marketplace(&f.root.join("market"), "demo");
        let source = f.root.join("market").to_string_lossy().into_owned();
        let (out, err, outcome) = run(|io| marketplace_add(&f.store, &source, None, io));
        assert_eq!(
            (out.as_str(), err.as_str()),
            ("Added marketplace demo (3 plugins)\n", "")
        );
        assert_eq!(outcome, CommandOutcome::default());
        let (out, _, _) = run(|io| marketplace_add(&f.store, &source, None, io));
        assert_eq!(out, "Marketplace demo is already added (3 plugins)\n");
        let (out, err, outcome) = run(|io| marketplace_add(&f.store, "./nowhere", None, io));
        assert_eq!(outcome.code, 1);
        assert!(out.is_empty());
        assert!(err.starts_with("alter-zero plugin: "), "{err}");
        assert!(err.contains("nowhere"), "{err}");
        assert_eq!(err.lines().count(), 1, "{err}");
    }

    #[test]
    fn install_says_what_arrived_and_that_hooks_wait_for_the_review() {
        let f = with_demo();
        let (out, err, outcome) = run(|io| install(&f.store, "hello", io));
        assert_eq!(err, "");
        assert!(
            out.starts_with("Installed hello@demo 1.0.0\n  1 skill · 1 command\n"),
            "{out}"
        );
        assert!(out.contains("  Loaded in place from "), "{out}");
        assert!(!out.contains(REVIEW_NOTICE), "{out}");
        assert!(!outcome.review);
        assert_eq!(outcome.forget_trust, [f.root.join("market/plugins/hello")]);
        let (out, _, outcome) = run(|io| install(&f.store, "guard@demo", io));
        assert!(out.starts_with("Installed guard@demo\n  1 hook\n"), "{out}");
        assert!(out.ends_with(&format!("  {REVIEW_NOTICE}\n")), "{out}");
        assert!(outcome.review);
        let (out, _, _) = run(|io| install(&f.store, "hello", io));
        assert!(out.starts_with("Reinstalled hello@demo 1.0.0\n"), "{out}");
        let (_, err, outcome) = run(|io| install(&f.store, "npmthing", io));
        assert_eq!(outcome.code, 1);
        assert!(err.contains("npmthing@demo cannot be installed"), "{err}");
        let (_, err, _) = run(|io| install(&f.store, "nothing", io));
        assert!(
            err.contains("no added marketplace lists a plugin named nothing"),
            "{err}"
        );
    }

    #[test]
    fn a_bare_name_two_marketplaces_list_is_an_error_naming_both() {
        let f = with_demo();
        marketplace(&f.root.join("other"), "other");
        let source = f.root.join("other").to_string_lossy().into_owned();
        run(|io| marketplace_add(&f.store, &source, None, io));
        let (out, err, outcome) = run(|io| install(&f.store, "hello", io));
        assert_eq!(outcome.code, 1);
        assert!(out.is_empty());
        assert!(err.contains("hello@demo, hello@other"), "{err}");
        let (out, _, outcome) = run(|io| install(&f.store, "hello@other", io));
        assert_eq!(outcome.code, 0, "{out}");
    }

    #[test]
    fn enable_and_disable_act_on_one_directory() {
        let f = with_demo();
        run(|io| install(&f.store, "hello", io));
        let elsewhere = f.root.join("elsewhere");
        let (out, _, outcome) = run(|io| set_enabled(&f.store, "hello", &f.project, false, io));
        assert_eq!(outcome.code, 0);
        assert_eq!(
            out,
            format!(
                "Disabled hello@demo in {} (it stays installed)\n",
                f.project.display()
            )
        );
        let (out, _, _) = run(|io| list(&f.store, &f.project, false, false, io));
        assert!(out.contains("hello@demo  1.0.0  disabled"), "{out}");
        let (out, _, _) = run(|io| list(&f.store, &elsewhere, false, false, io));
        assert_eq!(out, "hello@demo  1.0.0  enabled  1 skill · 1 command\n");
        let (out, _, _) = run(|io| set_enabled(&f.store, "hello@demo", &f.project, true, io));
        assert_eq!(
            out,
            format!("Enabled hello@demo in {}\n", f.project.display())
        );
        let (_, err, outcome) = run(|io| set_enabled(&f.store, "guard", &f.project, true, io));
        assert_eq!(outcome.code, 1);
        assert_eq!(err, "alter-zero plugin: guard is not installed\n");
    }

    #[test]
    fn list_shows_the_installed_and_available_adds_the_catalogs() {
        let f = with_demo();
        let (out, _, _) = run(|io| list(&f.store, &f.project, false, false, io));
        assert_eq!(
            out,
            "No plugins installed — see what is offered: alter-zero plugin list --available\n"
        );
        run(|io| install(&f.store, "hello", io));
        // The reason is the catalog's own sentence.
        let reason = plugin_list(&f.store, &f.project, true).unwrap().plugins[2]
            .reason
            .clone()
            .unwrap();
        assert!(reason.contains("npm"), "{reason}");
        let (out, _, _) = run(|io| list(&f.store, &f.project, true, false, io));
        assert_eq!(
            out,
            format!(
                "demo — 3 plugins\n  \
                 hello     1.0.0  enabled      Greets people\n  \
                 guard            available    Guards stops\n  \
                 npmthing         unavailable  {reason}\n"
            )
        );
        let (json, _, _) = run(|io| list(&f.store, &f.project, true, true, io));
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["plugins"][0]["id"], "hello@demo");
        assert_eq!(value["plugins"][0]["state"], "enabled");
        assert_eq!(value["plugins"][0]["summary"], "1 skill · 1 command");
        assert_eq!(value["plugins"][1]["state"], "available");
        assert_eq!(value["plugins"][2]["state"], "unavailable");
        assert_eq!(value["errors"], serde_json::json!([]));
        let (json, _, _) = run(|io| list(&f.store, &f.project, false, true, io));
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["plugins"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn info_names_each_component_as_a_session_uses_it() {
        let f = with_demo();
        let (out, _, _) = run(|io| info(&f.store, "hello", &f.project, false, io));
        assert!(
            out.starts_with("hello@demo · available\n  Greets people\n\n"),
            "{out}"
        );
        assert!(out.contains("  Source       ./plugins/hello\n"), "{out}");
        assert!(out.contains("  Skills       hello:greet\n"), "{out}");
        run(|io| install(&f.store, "hello", io));
        let (out, _, _) = run(|io| info(&f.store, "hello", &f.project, false, io));
        assert!(
            out.starts_with("hello@demo · enabled in this directory\n"),
            "{out}"
        );
        for row in [
            "  Version      1.0.0\n",
            "  Marketplace  demo\n",
            "  Format       Claude Code\n",
            "  Skills       hello:greet\n",
            "  Commands     hello:hi\n",
            "  Agents       none\n",
            "  Hooks        none\n",
            "  MCP servers  none\n",
        ] {
            assert!(out.contains(row), "{row:?} in\n{out}");
        }
        run(|io| install(&f.store, "guard", io));
        let (out, _, _) = run(|io| info(&f.store, "guard", &f.project, false, io));
        assert!(out.contains("  Hooks        Stop (1 handler)\n"), "{out}");
        let (out, _, _) = run(|io| info(&f.store, "npmthing", &f.project, false, io));
        assert!(out.starts_with("npmthing@demo · unavailable\n"), "{out}");
        assert!(out.contains("  Unavailable  "), "{out}");
        let (json, _, _) = run(|io| info(&f.store, "hello", &f.project, true, io));
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["state"], "enabled");
        assert_eq!(value["components"]["skills"][0], "hello:greet");
        assert_eq!(value["format"], "Claude Code");
        let (_, err, outcome) = run(|io| info(&f.store, "nope", &f.project, false, io));
        assert_eq!(outcome.code, 1);
        assert!(err.contains("no added marketplace lists"), "{err}");
    }

    #[test]
    fn info_indents_every_line_of_a_long_description() {
        let f = with_demo();
        write(
            &f.root.join("market"),
            "plugins/hello/.claude-plugin/plugin.json",
            r#"{"name":"hello","displayName":"Hello","description":"One.\n\nTwo."}"#,
        );
        run(|io| install(&f.store, "hello", io));
        let (out, _, _) = run(|io| info(&f.store, "hello", &f.project, false, io));
        assert!(
            out.starts_with("hello@demo · enabled in this directory\n  One.\n\n  Two.\n\n"),
            "{out}"
        );
    }

    #[test]
    fn uninstall_names_the_plugin_and_hands_back_its_directory() {
        let f = with_demo();
        run(|io| install(&f.store, "hello", io));
        let (out, _, outcome) = run(|io| uninstall(&f.store, "hello", io));
        assert!(
            out.starts_with("Uninstalled hello@demo (its folder "),
            "{out}"
        );
        assert_eq!(outcome.forget_trust, [f.root.join("market/plugins/hello")]);
        assert!(
            f.root.join("market/plugins/hello").is_dir(),
            "the user's folder stays"
        );
        let (_, err, outcome) = run(|io| uninstall(&f.store, "hello", io));
        assert_eq!(outcome.code, 1);
        assert_eq!(err, "alter-zero plugin: hello is not installed\n");
    }

    #[test]
    fn removing_a_marketplace_uninstalls_its_plugins_and_names_them() {
        let f = with_demo();
        run(|io| install(&f.store, "hello", io));
        run(|io| install(&f.store, "guard", io));
        let (out, _, outcome) = run(|io| marketplace_remove(&f.store, "demo", io));
        assert_eq!(
            out,
            "Removed marketplace demo and uninstalled 2 plugins: guard@demo, hello@demo\n"
        );
        assert_eq!(outcome.forget_trust.len(), 2);
        let (out, _, _) = run(|io| list(&f.store, &f.project, false, false, io));
        assert!(out.starts_with("No plugins installed"), "{out}");
        let (_, err, outcome) = run(|io| marketplace_remove(&f.store, "demo", io));
        assert_eq!(outcome.code, 1);
        assert!(err.contains("no marketplace named demo is added"), "{err}");
    }

    #[test]
    fn marketplace_list_shows_the_added_and_the_offered() {
        let f = fixture(vec![DefaultMarketplace {
            name: Some("official".into()),
            source: "acme/official".into(),
        }]);
        marketplace(&f.root.join("market"), "demo");
        let source = f.root.join("market").to_string_lossy().into_owned();
        run(|io| marketplace_add(&f.store, &source, None, io));
        run(|io| install(&f.store, "hello", io));
        let (out, _, _) = run(|io| marketplace_list(&f.store, false, 0, io));
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 2, "{out}");
        assert!(lines[0].starts_with("demo      "), "{out}");
        assert!(
            lines[0].ends_with("3 plugins · 1 installed · read in place"),
            "{out}"
        );
        assert_eq!(
            lines[1],
            format!(
                "official  acme/official{}offered, not added — alter-zero plugin marketplace add --defaults",
                " ".repeat(source.chars().count() - "acme/official".len() + 2)
            )
        );
        let (json, _, _) = run(|io| marketplace_list(&f.store, true, 0, io));
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["marketplaces"][0]["name"], "demo");
        assert_eq!(value["marketplaces"][0]["plugins"], 3);
        assert_eq!(value["marketplaces"][0]["local"], true);
        assert_eq!(value["marketplaces"][1]["added"], false);
        assert_eq!(value["marketplaces"][1]["plugins"], serde_json::Value::Null);
    }

    #[test]
    fn with_nothing_added_the_lists_say_how_to_start() {
        let f = fixture(Vec::new());
        let (out, _, _) = run(|io| marketplace_list(&f.store, false, 0, io));
        assert_eq!(
            out,
            "No marketplaces added — add one: alter-zero plugin marketplace add <source>\n"
        );
        let (out, _, _) = run(|io| list(&f.store, &f.project, true, false, io));
        assert!(out.contains("plugin marketplace add --defaults"), "{out}");
        let (out, _, _) = run(|io| update(&f.store, None, &f.project, io));
        assert_eq!(out, "No plugins installed\n");
        let (out, _, _) = run(|io| marketplace_add_defaults(&f.store, io));
        assert_eq!(out, "Every offered marketplace is already added\n");
    }

    #[test]
    fn a_broken_registry_is_one_failure_line_everywhere() {
        let f = with_demo();
        std::fs::write(f.store.layout().state_file(), "{oops").unwrap();
        for (_, err, outcome) in [
            run(|io| list(&f.store, &f.project, false, false, io)),
            run(|io| marketplace_list(&f.store, false, 0, io)),
            run(|io| install(&f.store, "hello", io)),
        ] {
            assert_eq!(outcome.code, 1);
            assert_eq!(err.lines().count(), 1, "{err}");
            assert!(err.contains("plugins.json"), "{err}");
        }
    }

    /// A git marketplace `gitdemo` in a repository at `root/repo`, its
    /// `file://` URL.
    fn git_marketplace(root: &Path) -> String {
        let repo = root.join("repo");
        marketplace(&repo, "gitdemo");
        git(&repo, &["init", "-q"]);
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-qm", "one"]);
        format!("file://{}", repo.display())
    }

    #[test]
    fn a_git_marketplace_installs_updates_and_reports_each_plugin() {
        if !has_git() {
            return;
        }
        let f = fixture(Vec::new());
        let url = git_marketplace(&f.root);
        let (out, err, outcome) = run(|io| marketplace_add(&f.store, &url, None, io));
        assert_eq!((outcome.code, err.as_str()), (0, ""), "{out}");
        assert_eq!(out, "Added marketplace gitdemo (3 plugins)\n");
        let (out, _, outcome) = run(|io| install(&f.store, "hello", io));
        assert!(out.starts_with("Installed hello@gitdemo 1.0.0\n"), "{out}");
        assert!(!out.contains("in place"), "{out}");
        assert_eq!(
            outcome.forget_trust,
            [f.store
                .layout()
                .installed_dir(&PluginId::parse("hello@gitdemo").unwrap())]
        );
        run(|io| install(&f.store, "guard", io));
        let (out, _, outcome) = run(|io| update(&f.store, None, &f.project, io));
        assert_eq!(
            out,
            "guard@gitdemo  up to date\nhello@gitdemo  up to date\n"
        );
        assert!(outcome.forget_trust.is_empty());
        let (out, _, _) = run(|io| marketplace_update(&f.store, None, io));
        assert_eq!(out, "gitdemo is up to date (3 plugins)\n");
        // A new version upstream.
        let repo = f.root.join("repo");
        write(
            &repo,
            "plugins/hello/.claude-plugin/plugin.json",
            r#"{"name":"hello","version":"1.1.0"}"#,
        );
        write(
            &repo,
            "plugins/hello/hooks/hooks.json",
            r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"true"}]}]}}"#,
        );
        git(&repo, &["commit", "-qam", "two"]);
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-qm", "three"]);
        let (out, err, outcome) = run(|io| update(&f.store, Some("hello"), &f.project, io));
        assert_eq!(err, "");
        assert_eq!(
            out,
            format!("hello@gitdemo  1.0.0 → 1.1.0\n{REVIEW_NOTICE}\n")
        );
        assert!(outcome.review);
        assert_eq!(outcome.forget_trust.len(), 1);
        let (out, _, _) = run(|io| marketplace_list(&f.store, false, u64::MAX, io));
        assert!(out.contains("3 plugins · 2 installed · updated "), "{out}");
    }

    #[test]
    fn progress_goes_to_err_only_on_a_terminal() {
        if !has_git() {
            return;
        }
        let f = fixture(Vec::new());
        let url = git_marketplace(&f.root);
        let (mut out, mut err) = (Vec::new(), Vec::new());
        marketplace_add(
            &f.store,
            &url,
            None,
            &mut Output {
                out: &mut out,
                err: &mut err,
                progress: true,
                bin: "alter-zero",
            },
        );
        let err = String::from_utf8(err).unwrap();
        assert!(err.starts_with("Cloning "), "{err}");
        assert!(err.lines().all(|l| l.ends_with('…')), "{err}");
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "Added marketplace gitdemo (3 plugins)\n"
        );
    }

    #[test]
    fn install_adds_an_offered_marketplace_on_the_way() {
        if !has_git() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let url = git_marketplace(&dir.path().canonicalize().unwrap());
        let f = fixture(vec![DefaultMarketplace {
            name: Some("gitdemo".into()),
            source: url,
        }]);
        let (out, err, outcome) = run(|io| install(&f.store, "hello@gitdemo", io));
        assert_eq!((outcome.code, err.as_str()), (0, ""), "{out}");
        assert!(
            out.starts_with(
                "Added marketplace gitdemo on the way\nInstalled hello@gitdemo 1.0.0\n"
            ),
            "{out}"
        );
        let (out, _, _) = run(|io| marketplace_add_defaults(&f.store, io));
        assert_eq!(out, "Every offered marketplace is already added\n");
    }
}
