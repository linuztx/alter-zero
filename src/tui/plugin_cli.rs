//! The `plugin` subcommand boundary (`docs/plugins.md` *From a shell*): the
//! store operations behind `alter-zero plugin …`, run by
//! [`super::startup::resolve_cli`] in cooked mode — before the tokio runtime
//! and the terminal, like every other CLI resolution (`docs/cli.md`).
//!
//! The grammar is the pure `cli` module's and every operation the pure
//! `plugins` store's — the same ones the `/plugins` page runs. What this file
//! adds is resolving a bare `<plugin>` name against what is listed or
//! installed, printing, and the exit code: `0` done, `1` anything that could
//! not be (a name nothing lists, a failed clone, a registry that will not
//! parse). Grammar errors never reach here — `parse` reports them and
//! `resolve_cli` exits 2.

use std::path::Path;

use alter_zero::cli::{self, HelpPage, PluginCli};
use alter_zero::plugins::{
    CatalogPlugin, PluginCatalog, PluginId, PluginStore, build_catalog, parse_marketplace_source,
};
use alter_zero::stream::CancelToken;

use super::config;
use super::host;
use super::plugins::found_marketplaces;
use super::startup::stdout_style;

/// Run one `plugin` subcommand and return the process exit code.
pub(crate) fn run(cmd: &PluginCli) -> i32 {
    if *cmd == PluginCli::Help {
        println!("{}", cli::help(HelpPage::Plugin, stdout_style()));
        return 0;
    }
    let Some(store) = config::plugin_store() else {
        eprintln!("No config home to keep plugins in — set HOME or ALTER_ZERO_CONFIG_DIR");
        return 1;
    };
    let cwd = match std::env::current_dir() {
        Ok(dir) => dir,
        Err(err) => {
            eprintln!("The working directory cannot be read: {err}");
            return 1;
        }
    };
    match execute(&store, cmd, &cwd) {
        Ok(()) => 0,
        Err(message) => {
            eprintln!("{message}");
            1
        }
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

/// The catalog — the added marketplaces', then those found without being
/// added (`docs/agents-dir.md`) — or why it cannot be read.
fn catalog(store: &PluginStore, cwd: &Path) -> Result<PluginCatalog, String> {
    let catalog = build_catalog(store, &cwd.display().to_string(), &found_marketplaces(cwd));
    match catalog.error {
        Some(error) => Err(error),
        None => Ok(catalog),
    }
}

/// The plugin `text` names among `candidates`: a `name@marketplace` as
/// written, or a bare name exactly one of them has. `what` says which
/// plugins were looked through, for the message when none matches.
fn resolve(text: &str, candidates: &[PluginId], what: &str) -> Result<PluginId, String> {
    if text.contains('@') {
        let id = PluginId::parse(text)?;
        return if candidates.contains(&id) {
            Ok(id)
        } else {
            Err(format!("No {what} plugin `{id}`"))
        };
    }
    let found: Vec<&PluginId> = candidates.iter().filter(|id| id.name == text).collect();
    match found.as_slice() {
        [one] => Ok((*one).clone()),
        [] => Err(format!("No {what} plugin named `{text}`")),
        many => Err(format!(
            "More than one marketplace lists `{text}` — name one: {}",
            many.iter()
                .map(|id| id.key())
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

/// The plugins every added marketplace lists.
fn listed(catalog: &PluginCatalog) -> Vec<PluginId> {
    catalog
        .plugins
        .iter()
        .map(|plugin| plugin.id.clone())
        .collect()
}

/// The installed plugins.
fn installed(catalog: &PluginCatalog) -> Vec<PluginId> {
    catalog
        .plugins
        .iter()
        .filter(|plugin| plugin.installed.is_some())
        .map(|plugin| plugin.id.clone())
        .collect()
}

/// A version as printed: `v1.2.0`.
fn version_label(version: Option<&str>) -> String {
    match version {
        Some(version) if version.starts_with('v') => version.to_string(),
        Some(version) => format!("v{version}"),
        None => String::new(),
    }
}

/// Install (or update) `id` and say so — with what it adds, and, when it
/// brings something that runs, that the app asks before it does. A plugin
/// of a marketplace found in `cwd` or the home folder, and never added, adds
/// that marketplace first (`docs/agents-dir.md`).
fn install(store: &PluginStore, id: &PluginId, cwd: &Path) -> Result<(), String> {
    let found = found_marketplaces(cwd);
    let (installed, added) =
        store.install_found(id, &found, &CancelToken::new(), host::unix_secs_signed())?;
    if let Some(added) = added {
        let from = found
            .iter()
            .find(|found| {
                found
                    .catalog
                    .as_ref()
                    .is_ok_and(|catalog| catalog.name == added.name)
            })
            .map_or_else(String::new, |found| {
                format!(" from {}", found.root.display())
            });
        println!(
            "Added marketplace {} ({}){from}",
            added.name,
            counted(added.plugins, "plugin"),
        );
    }
    let verb = if installed.replaced {
        "Updated"
    } else {
        "Installed"
    };
    let version = version_label(installed.package.manifest.version.as_deref());
    println!("{}", format!("{verb} {id} {version}").trim_end());
    let summary = installed.package.components.summary();
    println!("  {}", summary.line());
    for note in &installed.notes {
        println!("  {note}");
    }
    if summary.runs_code() {
        println!(
            "  Its hooks and MCP servers run once you trust them — the app asks when it next starts."
        );
    }
    Ok(())
}

/// One row of `plugin list`: the id, the version, and its state.
fn list_row(plugin: &CatalogPlugin, id_width: usize) -> String {
    let version = version_label(
        plugin
            .installed
            .as_ref()
            .and_then(|state| state.version.as_deref())
            .or(plugin.version.as_deref()),
    );
    let state = match &plugin.installed {
        Some(state) if state.error.is_some() => {
            format!("error: {}", state.error.clone().unwrap_or_default())
        }
        Some(state) => {
            let mut words = if state.enabled {
                "enabled".to_string()
            } else {
                "disabled here".to_string()
            };
            if state.update {
                words.push_str(", update available");
            }
            words
        }
        None => match &plugin.unavailable {
            Some(reason) => format!("unavailable: {reason}"),
            None => "available".to_string(),
        },
    };
    let key = plugin.id.key();
    format!("{key:<id_width$}  {version:<10}  {state}")
        .trim_end()
        .to_string()
}

fn execute(store: &PluginStore, cmd: &PluginCli, cwd: &Path) -> Result<(), String> {
    let now = host::unix_secs_signed();
    let project = cwd.display().to_string();
    match cmd {
        PluginCli::Help => Ok(()),
        PluginCli::Install { plugin } => {
            let catalog = catalog(store, cwd)?;
            let id = resolve(plugin, &listed(&catalog), "listed").map_err(|err| {
                if catalog.marketplaces.is_empty() {
                    format!(
                        "{err} — add a marketplace first: alter-zero plugin marketplace add <source>"
                    )
                } else {
                    err
                }
            })?;
            install(store, &id, cwd)
        }
        PluginCli::Uninstall { plugin } => {
            let id = resolve(plugin, &installed(&catalog(store, cwd)?), "installed")?;
            store.uninstall(&id)?;
            println!("Uninstalled {id}");
            Ok(())
        }
        PluginCli::Enable { plugin } | PluginCli::Disable { plugin } => {
            let enabled = matches!(cmd, PluginCli::Enable { .. });
            let id = resolve(plugin, &installed(&catalog(store, cwd)?), "installed")?;
            store.set_enabled(&project, &id, enabled)?;
            let state = if enabled { "Enabled" } else { "Disabled" };
            println!("{state} {id} in {project}");
            Ok(())
        }
        PluginCli::Update { plugin } => {
            let before = catalog(store, cwd)?;
            let wanted = match plugin {
                Some(text) => vec![resolve(text, &installed(&before), "installed")?],
                None => installed(&before),
            };
            if wanted.is_empty() {
                println!("No plugins installed");
                return Ok(());
            }
            // Their marketplaces read again first, so "newer" is today's.
            let markets: std::collections::BTreeSet<&str> =
                wanted.iter().map(|id| id.marketplace.as_str()).collect();
            for market in markets {
                if before
                    .marketplaces
                    .iter()
                    .any(|m| m.name == market && m.git)
                {
                    store.update_marketplace(market, &CancelToken::new(), now)?;
                }
            }
            let after = catalog(store, cwd)?;
            let mut updated = 0;
            for id in &wanted {
                let due = after
                    .plugins
                    .iter()
                    .find(|plugin| plugin.id == *id)
                    .and_then(|plugin| plugin.installed.as_ref())
                    .is_some_and(|state| state.update);
                if due {
                    install(store, id, cwd)?;
                    updated += 1;
                } else if plugin.is_some() {
                    println!("{id} is up to date");
                }
            }
            if plugin.is_none() && updated == 0 {
                println!("Every installed plugin is up to date");
            }
            Ok(())
        }
        PluginCli::List { available } => {
            let catalog = catalog(store, cwd)?;
            let rows: Vec<&CatalogPlugin> = catalog
                .plugins
                .iter()
                .filter(|plugin| *available || plugin.installed.is_some())
                .collect();
            if rows.is_empty() {
                println!(
                    "{}",
                    if *available {
                        "No marketplace lists a plugin — add one: alter-zero plugin marketplace add <source>"
                    } else {
                        "No plugins installed — see what is offered: alter-zero plugin list --available"
                    }
                );
                return Ok(());
            }
            let width = rows.iter().map(|p| p.id.key().len()).max().unwrap_or(0);
            for plugin in rows {
                println!("{}", list_row(plugin, width));
            }
            Ok(())
        }
        PluginCli::MarketplaceAdd { source } => {
            let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
            let source = parse_marketplace_source(source, cwd, home.as_deref())?;
            let added = store.add_marketplace(&source, &CancelToken::new(), now)?;
            if added.already {
                println!(
                    "Marketplace {} is already added ({}) — read again",
                    added.name,
                    counted(added.plugins, "plugin")
                );
            } else {
                println!(
                    "Added marketplace {} ({}) from {}",
                    added.name,
                    counted(added.plugins, "plugin"),
                    source.label()
                );
            }
            Ok(())
        }
        PluginCli::MarketplaceList => {
            let catalog = catalog(store, cwd)?;
            if catalog.marketplaces.is_empty() {
                println!(
                    "No marketplaces added — add one: alter-zero plugin marketplace add <source>"
                );
                return Ok(());
            }
            let width = catalog
                .marketplaces
                .iter()
                .map(|m| m.name.len())
                .max()
                .unwrap_or(0);
            for market in &catalog.marketplaces {
                let mut state = match &market.error {
                    Some(error) => format!("error: {error}"),
                    None if market.installed > 0 => format!(
                        "{}, {} installed",
                        counted(market.plugins, "plugin"),
                        market.installed
                    ),
                    None => counted(market.plugins, "plugin"),
                };
                if let Some(found_in) = market.found {
                    state.push_str(&format!(", found {}, not added", found_in.phrase()));
                }
                println!("{:<width$}  {}  ({state})", market.name, market.source);
            }
            Ok(())
        }
        PluginCli::MarketplaceUpdate { name } => {
            let catalog = catalog(store, cwd)?;
            let names: Vec<String> = match name {
                Some(name) => {
                    if !catalog.marketplaces.iter().any(|m| m.name == *name) {
                        return Err(format!("No marketplace named `{name}` is added"));
                    }
                    vec![name.clone()]
                }
                None => catalog
                    .marketplaces
                    .iter()
                    .filter(|m| m.git)
                    .map(|m| m.name.clone())
                    .collect(),
            };
            if names.is_empty() {
                println!("No git marketplaces to update — a directory is read in place");
                return Ok(());
            }
            for name in names {
                let updated = store.update_marketplace(&name, &CancelToken::new(), now)?;
                match updated.updates.len() {
                    0 if updated.changed => println!("Updated {}", updated.name),
                    0 => println!("{} is up to date", updated.name),
                    n => println!(
                        "Updated {} — {} to update: {} (alter-zero plugin update)",
                        updated.name,
                        counted(n, "plugin"),
                        updated.updates.join(", ")
                    ),
                }
            }
            Ok(())
        }
        PluginCli::MarketplaceRemove { name } => {
            let removed = store.remove_marketplace(name)?;
            if removed.is_empty() {
                println!("Removed marketplace {name}");
            } else {
                println!(
                    "Removed marketplace {name} and uninstalled {}: {}",
                    counted(removed.len(), "plugin"),
                    removed.join(", ")
                );
            }
            Ok(())
        }
    }
}
