//! What the `/plugins` page's host does with the store (`docs/plugins-menu.md`
//! *The boundary*): the snapshot the page opens on, a plugin's contents for
//! its page, and every operation the page asks for — plain functions over a
//! [`PluginStore`], so the app's boundary is a thread, a channel and a
//! session reload, and the logic lives here, in the crate an idle session
//! never faults in.
//!
//! **The snapshot runs no `git`.** It is read on the loop as the page opens,
//! so it is `plugins.json`, each added marketplace's catalog (streamed into
//! typed records) and each installed plugin's manifest — files, a few
//! milliseconds. Whether an installed plugin has an update is decided the
//! cheap way: a pinned commit its record does not start with, or a version
//! its marketplace now offers that differs from the one offered at install.
//! (`plugin update` and Ctrl+U on a git marketplace's tab ask `git` for the
//! exact answer.)

use std::path::{Path, PathBuf};

use super::rows::PluginList;
use super::state::{
    ComponentSummary, InstalledState, MarketplaceRow, MarketplaceSuggestion, PluginContents,
    PluginDone, PluginOp, PluginRow, PluginsSnapshot,
};
use crate::cancel::CancelToken;
use crate::components::PluginComponents;
use crate::marketplace::{CatalogEntry, PluginSource};
use crate::state::{PluginId, PluginRecord, PluginsFile};
use crate::store::{PluginError, PluginStore, UpdateStatus};

/// What the `+ Add` tab says about the two official catalogs, by source.
const KNOWN_CATALOGS: [(&str, &str); 2] = [
    (
        "anthropics/claude-plugins-official",
        "Claude Code's official plugin directory",
    ),
    ("openai/plugins", "Codex's official plugin directory"),
];

/// Why an entry with no source cannot be installed.
const UNSUPPORTED_SOURCE: &str = "its source is not supported";

/// What a finished operation hands the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpResult {
    /// What it did, or the user sentence of why it failed.
    pub outcome: Result<PluginDone, String>,
    /// The plugin directories whose trust records are to be forgotten —
    /// what was installed, updated or removed.
    pub forget: Vec<PathBuf>,
    /// What the session loads may have changed: reload it.
    pub reload: bool,
}

/// `path` as the page shows it: `~/…` under `home`, else whole.
fn tilde(path: &Path, home: Option<&Path>) -> String {
    match home.and_then(|home| path.strip_prefix(home).ok()) {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

/// Everything `/plugins` shows, read now: the added marketplaces in
/// `plugins.json`'s order, every plugin they list (installed ones with their
/// state in `project`), the installed plugins no catalog lists any more,
/// and the offered marketplaces not yet added. Contents stay unread: a
/// plugin's page asks for them ([`contents`]).
#[must_use]
pub fn snapshot(
    store: &PluginStore,
    project: &Path,
    home: Option<&Path>,
    session_enabled: bool,
) -> PluginsSnapshot {
    let mut out = PluginsSnapshot {
        session_enabled,
        ..PluginsSnapshot::default()
    };
    let state = match store.state() {
        Ok(state) => state,
        Err(error) => {
            out.error = Some(error.to_string());
            return out;
        }
    };
    let project_key = project.to_string_lossy();
    let installed = Installed::read(store, &state, project, &project_key, home);
    let mut list = PluginList::new();
    out.marketplaces = build_rows(store, &state, &installed, home, &mut list);
    out.plugins = list;
    if let Ok(listings) = store.list_marketplaces() {
        out.suggestions = listings
            .into_iter()
            .filter(|listing| !listing.added)
            .map(|listing| MarketplaceSuggestion {
                description: KNOWN_CATALOGS
                    .iter()
                    .find(|(source, _)| *source == listing.source)
                    .map(|(_, description)| (*description).to_string())
                    .unwrap_or_default(),
                name: listing.name.unwrap_or_else(|| listing.source.clone()),
                source: listing.source,
            })
            .collect();
    }
    out
}

/// Every plugin row the snapshot lists, packed onto `list` — each catalog one
/// entry at a time (decoded, pushed, dropped; never held whole), then the
/// installed plugins no catalog that reads lists — and the marketplaces'
/// rows.
fn build_rows(
    store: &PluginStore,
    state: &PluginsFile,
    installed: &Installed,
    home: Option<&Path>,
    list: &mut PluginList,
) -> Vec<MarketplaceRow> {
    let mut markets = Vec::with_capacity(state.marketplaces.len());
    let mut listed = vec![false; installed.found.len()];
    for (name, record) in &state.marketplaces {
        let mut row = MarketplaceRow {
            name: name.clone(),
            source: if Path::new(&record.source).is_absolute() {
                tilde(Path::new(&record.source), home)
            } else {
                record.source.clone()
            },
            git: !Path::new(&record.source).is_absolute(),
            revision: record.revision.clone(),
            updated_at: record.updated.and_then(|t| i64::try_from(t).ok()),
            installed: state.plugins_of(name).len(),
            ..MarketplaceRow::default()
        };
        let mark = list.mark();
        let mut entries = 0;
        let mut claimed = Vec::new();
        let read = store.catalog_entries(name, &mut |_, entry| {
            entries += 1;
            let Ok(id) = PluginId::new(&entry.name, name) else {
                return;
            };
            let mut plugin = listed_row(id, &entry);
            if let Some(at) = installed.position(&plugin.id) {
                let record = state.plugin(&plugin.id);
                installed.found[at].fill(&mut plugin, record, Some(&entry));
                claimed.push(at);
            }
            list.push(plugin);
        });
        match read {
            Ok(header) => {
                row.description = header.description;
                row.owner = header.owner;
                row.plugins = entries;
                row.notes = header.notices;
                for at in claimed {
                    listed[at] = true;
                }
            }
            // What it handed over before it failed is dropped; its installed
            // plugins stay unlisted, below.
            Err(error) => {
                list.truncate(mark);
                row.error = Some(error.to_string());
            }
        }
        markets.push(row);
    }
    // Installed, and listed by no catalog that reads — still here, so they
    // can be turned off and uninstalled.
    for (found, _) in installed
        .found
        .iter()
        .zip(&listed)
        .filter(|(_, listed)| !**listed)
    {
        let mut plugin = PluginRow {
            id: found.id.clone(),
            unavailable: Some(format!("{} no longer lists it", found.id.marketplace)),
            ..PluginRow::default()
        };
        let record = state.plugin(&found.id);
        found.fill(&mut plugin, record, None);
        list.push(plugin);
    }
    markets
}

/// A listed plugin's row, before anything about its installed copy.
fn listed_row(id: PluginId, entry: &CatalogEntry) -> PluginRow {
    PluginRow {
        id,
        display_name: entry.display_name.clone(),
        description: entry.description.clone(),
        version: entry.version.clone(),
        author: entry.author.clone(),
        category: entry.category.clone(),
        homepage: entry.homepage.clone(),
        keywords: entry.keywords.clone(),
        source: entry
            .source
            .as_ref()
            .map(PluginSource::label)
            .unwrap_or_default(),
        unavailable: entry.unavailable.clone().or_else(|| {
            entry
                .source
                .is_none()
                .then(|| UNSUPPORTED_SOURCE.to_string())
        }),
        ..PluginRow::default()
    }
}

/// The installed plugins as the page shows them, read once per snapshot.
struct Installed {
    found: Vec<Found>,
}

/// One installed plugin: its state here, and what its manifest says.
struct Found {
    id: PluginId,
    state: InstalledState,
    format: Option<String>,
    display_name: Option<String>,
    description: Option<String>,
    notes: Vec<String>,
}

impl Installed {
    /// Every installed plugin, each read as a session in `project` would
    /// load it: on or off here, why it would not load — its manifest, or an
    /// enabled plugin before it already loaded under its name.
    fn read(
        store: &PluginStore,
        state: &PluginsFile,
        project: &Path,
        project_key: &str,
        home: Option<&Path>,
    ) -> Self {
        let mut namespaces: Vec<(String, PluginId)> = Vec::new();
        let mut found = Vec::new();
        for id in state.installed() {
            let record = state.plugin(&id).cloned().unwrap_or_default();
            let enabled = state.is_enabled(project_key, &id);
            let mut item = Found {
                state: InstalledState {
                    version: record.version.clone(),
                    enabled,
                    update: false,
                    error: None,
                    in_place: record.root.is_some(),
                    loads_from: record
                        .root
                        .as_deref()
                        .map(|root| tilde(Path::new(root), home)),
                },
                id: id.clone(),
                format: None,
                display_name: None,
                description: None,
                notes: Vec::new(),
            };
            match store.inspect(&id, project) {
                Ok(contribution) => {
                    item.format = Some(contribution.format.label().to_string());
                    item.display_name = contribution.metadata.display_name.clone();
                    item.description = contribution.metadata.description.clone();
                    item.notes = contribution.notices.clone();
                    if enabled {
                        if let Some((_, first)) = namespaces
                            .iter()
                            .find(|(namespace, _)| *namespace == contribution.namespace)
                        {
                            item.state.error = Some(format!(
                                "{first} is already loaded under the name {}; turn one of them off",
                                contribution.namespace
                            ));
                        } else {
                            namespaces.push((contribution.namespace.clone(), id.clone()));
                        }
                    }
                }
                Err(error) => item.state.error = Some(error.to_string()),
            }
            found.push(item);
        }
        Self { found }
    }

    /// Where the installed plugin `id` is.
    fn position(&self, id: &PluginId) -> Option<usize> {
        self.found.iter().position(|found| found.id == *id)
    }
}

impl Found {
    /// Put what is known of the installed copy on `plugin`'s row — the
    /// manifest's words over the catalog's, and whether `entry` now offers
    /// something newer.
    fn fill(
        &self,
        plugin: &mut PluginRow,
        record: Option<&PluginRecord>,
        entry: Option<&CatalogEntry>,
    ) {
        let mut state = self.state.clone();
        if let (Some(record), Some(entry)) = (record, entry) {
            state.update = has_update(record, entry);
        }
        plugin.installed = Some(state);
        plugin.format.clone_from(&self.format);
        if self.display_name.is_some() {
            plugin.display_name.clone_from(&self.display_name);
        }
        if self.description.is_some() {
            plugin.description.clone_from(&self.description);
        }
        plugin.notes.clone_from(&self.notes);
    }
}

/// Does `entry` offer something newer than `record` — decided without
/// `git`: a pinned commit the record's revision does not start with, or a
/// version other than the one offered at install. One loaded in place is
/// always current.
fn has_update(record: &PluginRecord, entry: &CatalogEntry) -> bool {
    if record.root.is_some() || entry.unavailable.is_some() {
        return false;
    }
    let Some(source) = &entry.source else {
        return false;
    };
    if let Some(sha) = source.sha() {
        return !record
            .revision
            .as_deref()
            .is_some_and(|revision| revision.starts_with(sha));
    }
    match (&record.offered, &entry.version) {
        (Some(had), Some(now)) => had != now,
        _ => false,
    }
}

/// A store's component listing as the page's summary — a preview names
/// whether hooks and servers are there, never how many.
fn summary(components: PluginComponents, preview: bool) -> ComponentSummary {
    let hook_count = components.hook_count();
    let has_hooks = hook_count > 0 || components.hook_files > 0;
    let has_servers = !components.mcp_servers.is_empty();
    ComponentSummary {
        skills: components.skills,
        commands: components.commands,
        agents: components.agents,
        hooks: if preview {
            Vec::new()
        } else {
            components
                .hooks
                .into_iter()
                .map(|(event, n)| {
                    if n > 1 {
                        format!("{event} ({n})")
                    } else {
                        event
                    }
                })
                .collect()
        },
        hook_count: if preview { 0 } else { hook_count },
        servers: if preview {
            Vec::new()
        } else {
            components.mcp_servers
        },
        unused: components.not_used,
        preview,
        has_hooks: preview && has_hooks,
        has_servers: preview && has_servers,
    }
}

/// What `id` contains, for its page: an installed plugin read as a session
/// would load it, a listed one previewed from its marketplace (`git
/// ls-tree` for a git marketplace — which is why this runs off the loop).
#[must_use]
pub fn contents(
    store: &PluginStore,
    id: &PluginId,
    project: &Path,
    cancel: &CancelToken,
) -> PluginContents {
    let installed = match store.state() {
        Ok(state) => state.plugin(id).is_some(),
        Err(error) => return PluginContents::Unreadable(error.to_string()),
    };
    if installed {
        return match store.inspect(id, project) {
            Ok(contribution) => PluginContents::Known(summary(contribution.summary(), false)),
            Err(error) => PluginContents::Unreadable(error.to_string()),
        };
    }
    match store.preview(id, cancel) {
        Ok(Some(components)) => PluginContents::Known(summary(components, true)),
        Ok(None) => PluginContents::Unknown,
        Err(error) => PluginContents::Unreadable(error.to_string()),
    }
}

/// `{n} {noun}`, the noun plural for anything but one.
fn counted(n: usize, noun: &str) -> String {
    if n == 1 {
        format!("{n} {noun}")
    } else {
        format!("{n} {noun}s")
    }
}

/// The plugin directories `op` could replace or remove files in, read
/// before it runs: what a failure — or a worker that dies on the way — must
/// still forget the trust of. Over-counting is the safe direction: a
/// plugin forgotten for nothing is only reviewed once more.
#[must_use]
pub fn may_replace(store: &PluginStore, op: &PluginOp) -> Vec<PathBuf> {
    match op {
        PluginOp::Install(id) | PluginOp::Uninstall(id) => vec![store.plugin_dir_of(id)],
        PluginOp::RemoveMarketplace(name) => store
            .plugins_of_marketplace(name)
            .unwrap_or_default()
            .iter()
            .map(|id| store.plugin_dir_of(id))
            .collect(),
        PluginOp::Refresh
        | PluginOp::Inspect(_)
        | PluginOp::SetEnabled { .. }
        | PluginOp::AddMarketplace(_)
        | PluginOp::UpdateMarketplace(_) => Vec::new(),
    }
}

/// Run `op` — any but [`PluginOp::Inspect`], which is [`contents`] — and
/// say what it did, what trust to forget and whether to reload. A failure
/// still forgets (and reloads) every directory whose files it may have
/// replaced.
pub fn run_op(
    store: &PluginStore,
    project: &Path,
    op: &PluginOp,
    progress: &mut dyn FnMut(&str),
    cancel: &CancelToken,
) -> OpResult {
    let said = |message: String| PluginDone {
        message,
        marketplace: None,
    };
    let quiet = |outcome: Result<PluginDone, String>| OpResult {
        outcome,
        forget: Vec::new(),
        reload: false,
    };
    let failed = |error: PluginError, forget: Vec<PathBuf>| OpResult {
        outcome: Err(error.to_string()),
        reload: !forget.is_empty(),
        forget,
    };
    match op {
        PluginOp::Refresh | PluginOp::Inspect(_) => quiet(Ok(PluginDone::default())),
        PluginOp::Install(id) => {
            let installed = match store.state() {
                Ok(state) => state.plugin(id).is_some(),
                Err(error) => return quiet(Err(error.to_string())),
            };
            if installed {
                let (result, replaced) =
                    store.update_tracked(Some(&id.to_string()), progress, cancel);
                return match result {
                    Ok(updates) => {
                        let mut result = quiet(Ok(said(format!("{} is up to date", id.name))));
                        for update in updates {
                            if let UpdateStatus::Updated { old, new } = update.status {
                                result.outcome =
                                    Ok(said(format!("Updated {} {old} → {new}", id.name)));
                                result.forget.push(update.plugin_dir);
                                result.reload = true;
                            }
                        }
                        result
                    }
                    Err(error) => failed(error, replaced),
                };
            }
            let (result, replaced) = store.install_tracked(&id.to_string(), progress, cancel);
            match result {
                Ok(outcome) => OpResult {
                    outcome: Ok(said(format!("Installed {}", id.name))),
                    forget: vec![outcome.plugin_dir],
                    reload: true,
                },
                Err(error) => failed(error, replaced),
            }
        }
        PluginOp::Uninstall(id) => {
            let at_risk = may_replace(store, op);
            match store.uninstall(id) {
                Ok(outcome) => OpResult {
                    outcome: Ok(said(format!("Uninstalled {}", id.name))),
                    forget: vec![outcome.plugin_dir],
                    reload: true,
                },
                // Not installed: nothing was touched.
                Err(error @ PluginError::NotInstalled(_)) => failed(error, Vec::new()),
                Err(error) => failed(error, at_risk),
            }
        }
        PluginOp::SetEnabled { id, enabled } => match store.set_enabled(project, id, *enabled) {
            Ok(()) => OpResult {
                outcome: Ok(said(format!(
                    "{} {} in this directory",
                    if *enabled { "Enabled" } else { "Disabled" },
                    id.name
                ))),
                forget: Vec::new(),
                reload: true,
            },
            Err(error) => quiet(Err(error.to_string())),
        },
        PluginOp::AddMarketplace(source) => {
            match store.add_marketplace(source.trim(), None, progress, cancel) {
                Ok(added) => quiet(Ok(PluginDone {
                    message: if added.already {
                        format!("{} is already added", added.name)
                    } else {
                        format!(
                            "Added {} — {}",
                            added.name,
                            counted(added.plugins, "plugin")
                        )
                    },
                    marketplace: Some(added.name),
                })),
                Err(error) => quiet(Err(error.to_string())),
            }
        }
        PluginOp::UpdateMarketplace(name) => match store.update_marketplace(name, progress, cancel)
        {
            Ok(update) => {
                let message = match update.updates.len() {
                    0 if update.changed() => format!("Updated {name}"),
                    0 => format!("{name} is up to date"),
                    n => format!(
                        "Updated {name} — {} to update: {}",
                        counted(n, "plugin"),
                        update
                            .updates
                            .iter()
                            .map(|id| id.name.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                };
                quiet(Ok(said(message)))
            }
            Err(error) => quiet(Err(error.to_string())),
        },
        PluginOp::RemoveMarketplace(name) => {
            let at_risk = may_replace(store, op);
            match store.remove_marketplace(name) {
                Err(error) => failed(error, at_risk),
                Ok(removed) => {
                    let n = removed.uninstalled.len();
                    OpResult {
                        outcome: Ok(said(if n == 0 {
                            format!("Removed {name}")
                        } else {
                            format!("Removed {name} and uninstalled {}", counted(n, "plugin"))
                        })),
                        reload: n > 0,
                        forget: removed
                            .uninstalled
                            .into_iter()
                            .map(|(_, dir)| dir)
                            .collect(),
                    }
                }
            }
        }
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

    /// A marketplace named `name` at `root`: `hello` (a skill, version
    /// 1.0.0), `guard` (a hook and an MCP server) and `npmthing` (an npm
    /// source, unavailable).
    fn marketplace(root: &Path, name: &str) {
        write(
            root,
            ".claude-plugin/marketplace.json",
            &format!(
                r#"{{"name":"{name}","description":"Team tools","owner":{{"name":"Demo"}},"plugins":[
                {{"name":"hello","source":"./plugins/hello","description":"Greets people","version":"1.0.0"}},
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
        write(
            root,
            "plugins/guard/hooks/hooks.json",
            r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"true"}]},{"hooks":[{"type":"command","command":"false"}]}]}}"#,
        );
        write(
            root,
            "plugins/guard/.mcp.json",
            r#"{"mcpServers":{"db":{"command":"srv"}}}"#,
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

    fn quiet() -> impl FnMut(&str) {
        |_| {}
    }

    fn id(text: &str) -> PluginId {
        PluginId::parse(text).unwrap()
    }

    fn op(f: &Fixture, op: PluginOp) -> OpResult {
        run_op(&f.store, &f.project, &op, &mut quiet(), &CancelToken::new())
    }

    fn snap(f: &Fixture) -> PluginsSnapshot {
        snapshot(&f.store, &f.project, Some(&f.root), true)
    }

    fn row(snapshot: &PluginsSnapshot, key: &str) -> PluginRow {
        snapshot
            .plugins
            .iter()
            .find(|p| p.id().to_string() == key)
            .unwrap_or_else(|| panic!("no row {key}: {snapshot:#?}"))
            .to_row()
    }

    /// A folder marketplace `demo` added through the page's own operation.
    fn with_demo() -> Fixture {
        let f = fixture(Vec::new());
        marketplace(&f.root.join("market"), "demo");
        let result = op(
            &f,
            PluginOp::AddMarketplace(f.root.join("market").display().to_string()),
        );
        assert_eq!(
            result.outcome.as_ref().map(|d| d.marketplace.clone()),
            Ok(Some("demo".to_string()))
        );
        assert!(!result.reload);
        f
    }

    #[test]
    fn an_empty_install_offers_the_defaults_not_added() {
        let f = fixture(vec![
            DefaultMarketplace {
                name: Some("claude-plugins-official".into()),
                source: "anthropics/claude-plugins-official".into(),
            },
            DefaultMarketplace {
                name: None,
                source: "acme/tools".into(),
            },
        ]);
        let s = snapshot(&f.store, &f.project, None, false);
        assert!(s.marketplaces.is_empty() && s.plugins.is_empty());
        assert_eq!(s.error, None);
        assert!(!s.session_enabled);
        assert_eq!(
            s.suggestions,
            [
                MarketplaceSuggestion {
                    source: "anthropics/claude-plugins-official".into(),
                    name: "claude-plugins-official".into(),
                    description: "Claude Code's official plugin directory".into(),
                },
                MarketplaceSuggestion {
                    source: "acme/tools".into(),
                    name: "acme/tools".into(),
                    description: String::new(),
                },
            ]
        );
    }

    #[test]
    fn a_folder_marketplace_lists_its_plugins_and_its_header() {
        let f = with_demo();
        let s = snap(&f);
        assert_eq!(s.marketplaces.len(), 1);
        let m = &s.marketplaces[0];
        assert_eq!(m.name, "demo");
        assert!(!m.git);
        assert_eq!(m.source, "~/market");
        assert_eq!(m.description.as_deref(), Some("Team tools"));
        assert_eq!(m.owner.as_deref(), Some("Demo"));
        assert_eq!((m.plugins, m.installed), (3, 0));
        assert_eq!(m.error, None);
        assert!(m.updated_at.is_some());
        let hello = row(&s, "hello@demo");
        assert_eq!(hello.description.as_deref(), Some("Greets people"));
        assert_eq!(hello.version.as_deref(), Some("1.0.0"));
        assert_eq!(hello.source, "./plugins/hello");
        assert_eq!(hello.installed, None);
        assert_eq!(hello.unavailable, None);
        assert_eq!(hello.contents, PluginContents::NotRead);
        let npm = row(&s, "npmthing@demo");
        assert!(npm.unavailable.is_some(), "{npm:?}");
    }

    /// The page's rows carry no escape sequence of the catalog's.
    #[test]
    fn a_snapshot_row_carries_no_escape_sequence() {
        let f = fixture(Vec::new());
        let market = f.root.join("market");
        write(
            &market,
            ".claude-plugin/marketplace.json",
            r#"{"name":"demo","plugins":[{"name":"evil","source":"./evil",
               "description":"\u001b]52;c;AAAA\u0007Copies"}]}"#,
        );
        write(
            &market,
            "evil/.claude-plugin/plugin.json",
            r#"{"name":"evil"}"#,
        );
        op(&f, PluginOp::AddMarketplace(market.display().to_string()));
        let evil = row(&snap(&f), "evil@demo");
        assert_eq!(evil.description.as_deref(), Some("]52;c;AAAACopies"));
    }

    #[test]
    fn install_toggle_and_uninstall_reload_and_forget_the_plugin_dir() {
        let f = with_demo();
        let result = op(&f, PluginOp::Install(id("hello@demo")));
        let done = result.outcome.clone().unwrap();
        assert_eq!(done.message, "Installed hello");
        assert!(result.reload);
        assert_eq!(result.forget, [f.root.join("market/plugins/hello")]);

        let s = snap(&f);
        let state = row(&s, "hello@demo").installed.clone().unwrap();
        assert!(state.enabled && state.in_place && !state.update);
        assert_eq!(state.loads_from.as_deref(), Some("~/market/plugins/hello"));
        assert_eq!(state.error, None);
        assert_eq!(state.version.as_deref(), Some("1.0.0"));
        assert_eq!(row(&s, "hello@demo").format.as_deref(), Some("Claude Code"));
        assert_eq!(s.marketplaces[0].installed, 1);

        let off = op(
            &f,
            PluginOp::SetEnabled {
                id: id("hello@demo"),
                enabled: false,
            },
        );
        assert!(off.outcome.is_ok() && off.reload && off.forget.is_empty());
        assert!(!row(&snap(&f), "hello@demo").is_enabled());
        // Another directory is untouched.
        let elsewhere = snapshot(&f.store, &f.root, Some(&f.root), true);
        assert!(row(&elsewhere, "hello@demo").is_enabled());

        let gone = op(&f, PluginOp::Uninstall(id("hello@demo")));
        assert_eq!(gone.outcome.unwrap().message, "Uninstalled hello");
        assert!(gone.reload);
        assert_eq!(gone.forget, [f.root.join("market/plugins/hello")]);
        assert_eq!(row(&snap(&f), "hello@demo").installed, None);
    }

    #[test]
    fn contents_previews_a_listed_plugin_and_reads_an_installed_one() {
        let f = with_demo();
        let cancel = CancelToken::new();
        let PluginContents::Known(preview) =
            contents(&f.store, &id("guard@demo"), &f.project, &cancel)
        else {
            panic!("a preview");
        };
        assert!(preview.preview && preview.has_hooks && preview.has_servers);
        assert!(preview.hooks.is_empty() && preview.servers.is_empty());

        op(&f, PluginOp::Install(id("guard@demo")));
        let PluginContents::Known(read) =
            contents(&f.store, &id("guard@demo"), &f.project, &cancel)
        else {
            panic!("read");
        };
        assert!(!read.preview);
        assert_eq!(read.hooks, ["Stop (2)"]);
        assert_eq!(read.hook_count, 2);
        assert_eq!(read.servers, ["plugin:guard:db"]);
        assert!(read.runs_code());

        let PluginContents::Known(hello) =
            contents(&f.store, &id("hello@demo"), &f.project, &cancel)
        else {
            panic!("a preview");
        };
        assert_eq!(hello.skills, ["hello:greet"]);
        assert!(!hello.runs_code());

        assert!(matches!(
            contents(&f.store, &id("nothing@demo"), &f.project, &cancel),
            PluginContents::Unreadable(_)
        ));
    }

    #[test]
    fn an_installed_plugin_whose_files_are_gone_is_not_loaded_and_still_listed() {
        let f = with_demo();
        op(&f, PluginOp::Install(id("hello@demo")));
        // The catalog drops it, and its folder goes.
        write(
            &f.root.join("market"),
            ".claude-plugin/marketplace.json",
            r#"{"name":"demo","plugins":[]}"#,
        );
        std::fs::remove_dir_all(f.root.join("market/plugins/hello")).unwrap();
        let s = snap(&f);
        let hello = row(&s, "hello@demo");
        let state = hello.installed.as_ref().unwrap();
        assert!(state.error.is_some(), "{state:?}");
        assert!(hello.unavailable.is_some(), "no longer listed: no update");
        assert_eq!(s.installed_count(), 1);
    }

    #[test]
    fn a_catalog_that_will_not_read_is_its_marketplace_error() {
        let f = with_demo();
        write(
            &f.root.join("market"),
            ".claude-plugin/marketplace.json",
            "{ not json",
        );
        let s = snap(&f);
        assert_eq!(s.marketplaces.len(), 1);
        assert!(s.marketplaces[0].error.is_some());
        assert!(s.plugins.is_empty());
    }

    #[test]
    fn a_catalog_broken_part_way_keeps_none_of_its_rows_but_its_installed_plugins() {
        let f = with_demo();
        op(&f, PluginOp::Install(id("hello@demo")));
        // Two whole entries, then the document breaks off.
        write(
            &f.root.join("market"),
            ".claude-plugin/marketplace.json",
            r#"{"name":"demo","plugins":[{"name":"hello","source":"./plugins/hello"},{"name":"guard","source":"./plugins/guard"},{"#,
        );
        let s = snap(&f);
        assert!(s.marketplaces[0].error.is_some());
        let rows: Vec<String> = s.plugins.iter().map(|p| p.id().to_string()).collect();
        assert_eq!(rows, ["hello@demo"], "only the installed plugin, unlisted");
        assert!(row(&s, "hello@demo").unavailable.is_some());
        assert!(row(&s, "hello@demo").installed.is_some());
    }

    #[test]
    fn a_plugins_json_that_will_not_parse_is_the_snapshot_error() {
        let f = fixture(Vec::new());
        write(&f.root.join("config"), "plugins/plugins.json", "{ nope");
        // Wherever the layout keeps it.
        let path = f.store.layout().state_file();
        write(path.parent().unwrap(), "plugins.json", "{ nope");
        let s = snap(&f);
        assert!(s.error.is_some());
        assert!(s.marketplaces.is_empty());
    }

    #[test]
    fn removing_a_marketplace_uninstalls_and_forgets_its_plugins() {
        let f = with_demo();
        op(&f, PluginOp::Install(id("hello@demo")));
        let result = op(&f, PluginOp::RemoveMarketplace("demo".into()));
        assert!(result.outcome.is_ok());
        assert!(result.reload);
        assert_eq!(result.forget, [f.root.join("market/plugins/hello")]);
        let s = snap(&f);
        assert!(s.marketplaces.is_empty() && s.plugins.is_empty());
        // Nothing installed: no reload.
        let f = with_demo();
        let result = op(&f, PluginOp::RemoveMarketplace("demo".into()));
        assert!(!result.reload);
    }

    #[test]
    fn a_failure_is_the_store_sentence() {
        let f = with_demo();
        let result = op(&f, PluginOp::Uninstall(id("hello@demo")));
        assert!(result.outcome.unwrap_err().contains("hello@demo"));
        assert!(!result.reload);
        let result = op(&f, PluginOp::AddMarketplace("/no/such/folder".into()));
        assert!(result.outcome.is_err());
    }

    #[test]
    fn a_cancelled_operation_says_so_and_changes_nothing() {
        if !has_git() {
            return;
        }
        let f = fixture(Vec::new());
        let repo = f.root.join("repo");
        marketplace(&repo, "gitdemo");
        git(&repo, &["init", "-q"]);
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-qm", "one"]);
        let cancel = CancelToken::new();
        cancel.cancel();
        let result = run_op(
            &f.store,
            &f.project,
            &PluginOp::AddMarketplace(format!("file://{}", repo.display())),
            &mut quiet(),
            &cancel,
        );
        assert!(result.outcome.is_err());
        assert!(snap(&f).marketplaces.is_empty());
    }

    /// An install or update that fails once its new files are in place
    /// still forgets their trust and reloads; and what an operation could
    /// touch is known before it runs, for a worker that dies on the way.
    #[test]
    fn a_failure_after_new_files_are_in_place_forgets_and_reloads() {
        if !has_git() {
            return;
        }
        let f = fixture(Vec::new());
        let repo = f.root.join("repo");
        marketplace(&repo, "gitdemo");
        git(&repo, &["init", "-q"]);
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-qm", "one"]);
        op(
            &f,
            PluginOp::AddMarketplace(format!("file://{}", repo.display())),
        );
        let hello = id("hello@gitdemo");
        let dir = f.store.layout().installed_dir(&hello);
        assert_eq!(
            may_replace(&f.store, &PluginOp::Install(hello.clone())),
            std::slice::from_ref(&dir)
        );
        assert!(may_replace(&f.store, &PluginOp::Refresh).is_empty());
        op(&f, PluginOp::Install(hello.clone()));
        assert_eq!(
            may_replace(&f.store, &PluginOp::RemoveMarketplace("gitdemo".into())),
            std::slice::from_ref(&dir)
        );
        let data = f.store.layout().data_dir(&hello);
        std::fs::remove_dir_all(&data).unwrap();
        std::fs::write(&data, "in the way").unwrap();
        write(
            &repo,
            "plugins/hello/.claude-plugin/plugin.json",
            r#"{"name":"hello","version":"1.1.0"}"#,
        );
        git(&repo, &["commit", "-qam", "two"]);
        let failed = op(&f, PluginOp::Install(hello));
        assert!(failed.outcome.is_err(), "{failed:?}");
        assert_eq!(failed.forget, [dir]);
        assert!(failed.reload);
    }

    #[test]
    fn a_git_marketplace_installs_previews_and_updates_through_the_page() {
        if !has_git() {
            return;
        }
        let f = fixture(Vec::new());
        let repo = f.root.join("repo");
        marketplace(&repo, "gitdemo");
        git(&repo, &["init", "-q"]);
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-qm", "one"]);
        let mut steps = Vec::new();
        let added = run_op(
            &f.store,
            &f.project,
            &PluginOp::AddMarketplace(format!("file://{}", repo.display())),
            &mut |step: &str| steps.push(step.to_string()),
            &CancelToken::new(),
        );
        assert_eq!(
            added.outcome.unwrap().marketplace.as_deref(),
            Some("gitdemo")
        );
        assert!(!steps.is_empty());
        let s = snap(&f);
        assert!(s.marketplaces[0].git);
        assert!(s.marketplaces[0].revision.is_some());

        // A preview from the blobless clone's tree.
        let PluginContents::Known(preview) = contents(
            &f.store,
            &id("hello@gitdemo"),
            &f.project,
            &CancelToken::new(),
        ) else {
            panic!("a preview");
        };
        assert!(preview.preview);
        assert_eq!(preview.skills, ["hello:greet"]);

        let installed = op(&f, PluginOp::Install(id("hello@gitdemo")));
        assert_eq!(installed.outcome.unwrap().message, "Installed hello");
        let dir = f.store.layout().installed_dir(&id("hello@gitdemo"));
        assert_eq!(installed.forget, std::slice::from_ref(&dir));
        let state = row(&snap(&f), "hello@gitdemo").installed.clone().unwrap();
        assert!(!state.in_place && !state.update && state.loads_from.is_none());

        // A new version upstream, fetched: the cheap rule sees it.
        write(
            &repo,
            ".claude-plugin/marketplace.json",
            &std::fs::read_to_string(repo.join(".claude-plugin/marketplace.json"))
                .unwrap()
                .replace(r#""version":"1.0.0""#, r#""version":"1.1.0""#),
        );
        write(
            &repo,
            "plugins/hello/.claude-plugin/plugin.json",
            r#"{"name":"hello","version":"1.1.0"}"#,
        );
        git(&repo, &["commit", "-qam", "two"]);
        let fetched = op(&f, PluginOp::UpdateMarketplace("gitdemo".into()));
        assert!(
            fetched.outcome.as_ref().unwrap().message.contains("hello"),
            "{fetched:?}"
        );
        assert!(!fetched.reload);
        assert!(
            row(&snap(&f), "hello@gitdemo")
                .installed
                .as_ref()
                .unwrap()
                .update
        );

        // Install on an installed plugin is its update.
        let updated = op(&f, PluginOp::Install(id("hello@gitdemo")));
        assert_eq!(
            updated.outcome.unwrap().message,
            "Updated hello 1.0.0 → 1.1.0"
        );
        assert!(updated.reload);
        assert_eq!(updated.forget, [dir]);
        let state = row(&snap(&f), "hello@gitdemo").installed.clone().unwrap();
        assert_eq!(state.version.as_deref(), Some("1.1.0"));
        assert!(!state.update);

        // Up to date: nothing to reload.
        let again = op(&f, PluginOp::Install(id("hello@gitdemo")));
        assert_eq!(again.outcome.unwrap().message, "hello is up to date");
        assert!(!again.reload && again.forget.is_empty());
    }

    #[test]
    fn two_enabled_plugins_under_one_name_the_second_is_not_loaded() {
        let f = with_demo();
        marketplace(&f.root.join("market2"), "other");
        op(
            &f,
            PluginOp::AddMarketplace(f.root.join("market2").display().to_string()),
        );
        op(&f, PluginOp::Install(id("hello@demo")));
        op(&f, PluginOp::Install(id("hello@other")));
        let s = snap(&f);
        let errors: Vec<bool> = ["hello@demo", "hello@other"]
            .iter()
            .map(|key| row(&s, key).installed.as_ref().unwrap().error.is_some())
            .collect();
        assert_eq!(errors, [false, true]);
    }
}
