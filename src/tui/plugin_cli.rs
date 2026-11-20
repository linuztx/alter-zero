//! The `plugin` subcommand boundary (`docs/plugin-cli.md`): the store calls
//! and the printing behind `alter-zero plugin list/info/install/…` and
//! `plugin marketplace add/list/update/remove`, run by
//! [`super::startup::resolve_cli`] in cooked mode — before the tokio runtime
//! and the terminal, like the `mcp` family (`docs/mcp-cli.md`).
//!
//! The grammar is the pure `cli` module's and every operation is the
//! store's (`plugins::PluginStore`, `docs/plugins.md`); what lives here is
//! the order the operations run in — a marketplace never fetched is fetched
//! before anything needs it, so `plugin install x@claude-plugins-official`
//! works on a fresh machine — and what each outcome prints. Results go to
//! stdout, problems to stderr as `error: …` / `warning: …`, and progress to
//! stderr only when stderr is a terminal, git's percentages rewritten in
//! place. The text the commands print is built by the small pure functions
//! at the bottom, so it is tested without a store.

use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use alter_zero::cli::{self, HelpPage, MarketplaceCli, PluginCli};
use alter_zero::plugins::{
    CatalogTab, Components, Control, DefaultMarketplace, GitControl, GitProgress, InstallOutcome,
    KnownMarketplace, LOCAL_MARKETPLACE, MarketplaceSource, PluginCatalog, PluginDetail, PluginId,
    PluginSelector, PluginStatus, PluginStore, PluginsFile,
};
use serde::Serialize;
use unicode_width::UnicodeWidthStr;

use super::config;
use super::startup::stdout_style;

/// `println!` for stdout that a reader gone away (`plugin list | head`)
/// ends quietly instead of panicking.
macro_rules! say {
    ($($arg:tt)*) => {
        emit(&format!("{}\n", format_args!($($arg)*)))
    };
}

/// The width a listing is cut to on a terminal that will not say its own.
const FALLBACK_WIDTH: usize = 100;
/// Spaces between two columns of a table.
const COLUMN_GAP: &str = "   ";
/// The label column of `plugin info`: the widest label, `Commands`.
const INFO_LABEL_WIDTH: usize = 8;
/// The widest the name column of `list --available` grows: a longer name
/// pushes its own description over rather than every row's.
const NAME_COLUMN_MAX: usize = 28;
/// What a plugin row says when it cannot be installed here.
const UNSUPPORTED_NOTE: &str = "(can't be installed here)";

/// Run one `plugin` subcommand and return the process exit code: `0`
/// success, `1` a failure (said on stderr). Grammar errors never reach
/// here — `parse` reports them and `resolve_cli` exits 2.
pub(crate) fn run(cmd: &PluginCli) -> i32 {
    if *cmd == PluginCli::Help {
        say!("{}", cli::help(HelpPage::Plugin, stdout_style()));
        return 0;
    }
    let out = Out::new();
    let Some(store) = config::plugin_store(true) else {
        return i32::from(!out.fail("no config directory — set HOME or ALTER_ZERO_CONFIG_DIR"));
    };
    let ok = match cmd {
        PluginCli::Help => true,
        PluginCli::List { available, json } => {
            if *available {
                list_available(&store, &out, *json)
            } else {
                list_installed(&store, &out, *json)
            }
        }
        PluginCli::Info { plugin, json } => info(&store, &out, plugin, *json),
        PluginCli::Install { plugin } => install(&store, &out, plugin),
        PluginCli::Uninstall { plugin } => uninstall(&store, &out, plugin),
        PluginCli::Enable { plugin } => set_enabled(&store, &out, plugin, true),
        PluginCli::Disable { plugin } => set_enabled(&store, &out, plugin, false),
        PluginCli::Update {
            plugin: Some(plugin),
        } => update_one(&store, &out, plugin),
        PluginCli::Update { plugin: None } => update_all(&store, &out),
        PluginCli::Marketplace(MarketplaceCli::Add { source, reference }) => {
            marketplace_add(&store, &out, source, reference.as_deref())
        }
        PluginCli::Marketplace(MarketplaceCli::List { json }) => {
            marketplace_list(&store, &out, *json)
        }
        PluginCli::Marketplace(MarketplaceCli::Update { name }) => {
            marketplace_update(&store, &out, name.as_deref())
        }
        PluginCli::Marketplace(MarketplaceCli::Remove { name }) => {
            marketplace_remove(&store, &out, name)
        }
    };
    out.progress.finish();
    i32::from(!ok)
}

// ===== the commands =====

fn list_installed(store: &PluginStore, out: &Out, json: bool) -> bool {
    let file = match store.state() {
        Ok(file) => file,
        Err(error) => return out.fail(&error),
    };
    // The store's reading of each install: what it loads, and what its
    // marketplace offers now.
    let details: Vec<Option<PluginDetail>> = file
        .plugins
        .keys()
        .map(|key| {
            PluginId::parse(key)
                .ok()
                .and_then(|id| store.detail(&id).ok())
        })
        .collect();
    let offered: Vec<Option<&str>> = details
        .iter()
        .map(|detail| detail.as_ref().and_then(|d| d.version.as_deref()))
        .collect();
    if json {
        say!("{}", installed_json(&file, &offered));
        return true;
    }
    let rows: Vec<InstalledRow> = file
        .plugins
        .iter()
        .zip(&details)
        .map(|((key, record), detail)| InstalledRow {
            id: key.clone(),
            version: version_step(
                record.version.as_deref(),
                detail.as_ref().and_then(|d| d.version.as_deref()),
                detail.as_ref().is_some_and(|d| d.update_available),
            ),
            enabled: record.enabled,
            summary: detail
                .as_ref()
                .and_then(|detail| detail.components.as_ref())
                .map(Components::summary)
                .unwrap_or_default(),
        })
        .collect();
    emit(&installed_text(&rows, out.paint, out.width, &out.bin));
    true
}

fn list_available(store: &PluginStore, out: &Out, json: bool) -> bool {
    let fetched = fetch_unfetched(store, out, None);
    let catalog = match store.catalog() {
        Ok(catalog) => catalog,
        Err(error) => return out.fail(&error),
    };
    if json {
        say!("{}", available_json(&catalog));
    } else {
        emit(&available_text(&catalog, out.paint, out.width, &out.bin));
    }
    fetched
}

fn info(store: &PluginStore, out: &Out, text: &str, json: bool) -> bool {
    let id = match lookup(store, out, text) {
        Ok(id) => id,
        Err(error) => return out.fail(&error),
    };
    let detail = match store.detail(&id) {
        Ok(detail) => detail,
        Err(error) => return out.fail(&error),
    };
    if json {
        say!("{}", detail_json(&detail));
        return true;
    }
    emit(&info_text(&detail, out.paint, out.width, &out.bin));
    for warning in &detail.warnings {
        out.warning(warning);
    }
    true
}

fn install(store: &PluginStore, out: &Out, text: &str) -> bool {
    let selector = match selector(text) {
        Ok(selector) => selector,
        Err(error) => return out.fail(&error),
    };
    let before: BTreeSet<String> = match store.state() {
        Ok(file) => file.plugins.keys().cloned().collect(),
        Err(error) => return out.fail(&error),
    };
    let outcome = if let PluginSelector::Directory(dir) = &selector {
        out.progress
            .headline(&format!("Installing {}…", shown_path(dir)));
        store.install_directory(dir, &out.progress)
    } else {
        let id = match lookup(store, out, text) {
            Ok(id) => id,
            Err(error) => return out.fail(&error),
        };
        out.progress.headline(&format!("Installing {id}…"));
        store.install(&id, &out.progress)
    };
    out.progress.finish();
    match outcome {
        Ok(outcome) => {
            let again = before.contains(&outcome.id.to_string());
            emit(&installed_outcome_text(&outcome, again));
            for warning in &outcome.warnings {
                out.warning(warning);
            }
            true
        }
        Err(error) => out.fail(&error),
    }
}

fn uninstall(store: &PluginStore, out: &Out, text: &str) -> bool {
    let id = match installed_id(store, text) {
        Ok(id) => id,
        Err(error) => return out.fail(&error),
    };
    match store.uninstall(&id) {
        Ok(()) => {
            say!("Uninstalled {id}");
            true
        }
        Err(error) => out.fail(&error),
    }
}

fn set_enabled(store: &PluginStore, out: &Out, text: &str, enabled: bool) -> bool {
    let id = match installed_id(store, text) {
        Ok(id) => id,
        Err(error) => return out.fail(&error),
    };
    let already = store
        .state()
        .ok()
        .and_then(|file| file.plugin(&id).map(|record| record.enabled == enabled))
        .unwrap_or(false);
    let word = if enabled { "Enabled" } else { "Disabled" };
    if already {
        say!("Already {}: {id}", word.to_lowercase());
        return true;
    }
    match store.set_enabled(&id, enabled) {
        Ok(()) => {
            say!("{word} {id}");
            true
        }
        Err(error) => out.fail(&error),
    }
}

fn update_one(store: &PluginStore, out: &Out, text: &str) -> bool {
    let id = match installed_id(store, text) {
        Ok(id) => id,
        Err(error) => return out.fail(&error),
    };
    let file = match store.state() {
        Ok(file) => file,
        Err(error) => return out.fail(&error),
    };
    if id.marketplace != LOCAL_MARKETPLACE && file.marketplaces.contains_key(&id.marketplace) {
        out.progress
            .headline(&format!("Updating {}…", id.marketplace));
        let result = store.update_marketplace(&id.marketplace, &out.progress);
        out.progress.finish();
        if let Err(error) = result {
            return out.fail(&error);
        }
    }
    update_plugin(store, out, &id, &file).is_some()
}

fn update_all(store: &PluginStore, out: &Out) -> bool {
    let file = match store.state() {
        Ok(file) => file,
        Err(error) => return out.fail(&error),
    };
    if file.plugins.is_empty() {
        say!("No plugins installed.");
        return true;
    }
    let ids: Vec<PluginId> = file
        .plugins
        .keys()
        .filter_map(|key| PluginId::parse(key).ok())
        .collect();
    let mut ok = true;
    // The marketplaces the installed plugins come from — a marketplace
    // nothing was installed from has nothing for this command to update.
    let sources: BTreeSet<&str> = ids
        .iter()
        .map(|id| id.marketplace.as_str())
        .filter(|name| file.marketplaces.contains_key(*name))
        .collect();
    for name in sources {
        out.progress.headline(&format!("Updating {name}…"));
        let result = store.update_marketplace(name, &out.progress);
        out.progress.finish();
        if let Err(error) = result {
            ok = out.fail(&format!("could not update `{name}`: {error}"));
        }
    }
    let mut updated = 0;
    for id in &ids {
        match update_plugin(store, out, id, &file) {
            Some(true) => updated += 1,
            Some(false) => {}
            None => ok = false,
        }
    }
    say!("{}", checked_line(ids.len(), updated));
    ok
}

/// Update one installed plugin and say how it went; `Some(changed)`, or
/// `None` after an error was printed.
fn update_plugin(
    store: &PluginStore,
    out: &Out,
    id: &PluginId,
    before: &PluginsFile,
) -> Option<bool> {
    out.progress.headline(&format!("Updating {id}…"));
    let result = store.update_plugin(id, &out.progress);
    out.progress.finish();
    match result {
        Ok(outcome) => {
            let old = before.plugin(id).and_then(|record| record.version.clone());
            say!("{}", update_line(&outcome, old.as_deref()));
            for warning in &outcome.warnings {
                out.warning(warning);
            }
            Some(outcome.changed)
        }
        Err(error) => {
            out.fail(&error);
            None
        }
    }
}

fn marketplace_add(store: &PluginStore, out: &Out, input: &str, reference: Option<&str>) -> bool {
    let (cwd, home) = places();
    let source = match MarketplaceSource::parse(input, &cwd, home.as_deref())
        .and_then(|source| with_reference(source, input, reference, home.as_deref()))
    {
        Ok(source) => source,
        Err(error) => return out.fail(&error),
    };
    let shown = source.display(home.as_deref());
    if matches!(source, MarketplaceSource::Git { .. }) {
        out.progress.headline(&format!("Fetching {shown}…"));
    }
    let result = store.add_marketplace(&source, &out.progress);
    out.progress.finish();
    match result {
        Ok(outcome) if !outcome.changed => {
            say!(
                "Marketplace {} is already added ({})",
                outcome.name,
                plugins_count(outcome.plugins)
            );
            warn_skipped(store, out, &outcome.name);
            true
        }
        Ok(outcome) => {
            say!(
                "Added marketplace {} ({}) from {shown}",
                outcome.name,
                plugins_count(outcome.plugins)
            );
            say!(
                "Install one with: {} plugin install <plugin>@{}",
                out.bin,
                outcome.name
            );
            warn_skipped(store, out, &outcome.name);
            true
        }
        Err(error) => out.fail(&error),
    }
}

/// `warning: skipped 2 entries: …` on stderr when the marketplace `name`
/// lists entries that could not be read.
fn warn_skipped(store: &PluginStore, out: &Out, name: &str) {
    if let Some(line) = skipped_line(&store.skipped_entries(name)) {
        out.warning(&line);
    }
}

fn marketplace_list(store: &PluginStore, out: &Out, json: bool) -> bool {
    let known = match store.known_marketplaces() {
        Ok(known) => known,
        Err(error) => return out.fail(&error),
    };
    let catalog = store.catalog().unwrap_or_default();
    let home = store_home();
    let mut rows: Vec<MarketplaceRow> = known
        .iter()
        .enumerate()
        .map(|(index, known)| MarketplaceRow::new(known, &catalog, index, home.as_deref()))
        .collect();
    rows.extend(
        store
            .removed_defaults()
            .unwrap_or_default()
            .iter()
            .map(|default| MarketplaceRow::removed(default, home.as_deref())),
    );
    if json {
        say!("{}", marketplaces_json(&rows));
    } else {
        emit(&marketplaces_text(&rows, now(), out.paint, &out.bin));
    }
    true
}

fn marketplace_update(store: &PluginStore, out: &Out, name: Option<&str>) -> bool {
    let known = match store.known_marketplaces() {
        Ok(known) => known,
        Err(error) => return out.fail(&error),
    };
    let chosen: Vec<&KnownMarketplace> = match name {
        None => known.iter().collect(),
        Some(name) => {
            let found: Vec<&KnownMarketplace> = known
                .iter()
                .filter(|k| k.name.as_deref() == Some(name))
                .collect();
            if found.is_empty() {
                return out.fail(&unknown_marketplace(name, &known));
            }
            found
        }
    };
    if chosen.is_empty() {
        say!("{}", no_marketplaces_line(&out.bin));
        return true;
    }
    let home = store_home();
    let mut ok = true;
    for known in chosen {
        let shown = known.source.display(home.as_deref());
        let label = known.name.clone().unwrap_or_else(|| shown.clone());
        let result = if known.fetched {
            out.progress.headline(&format!("Updating {label}…"));
            store.update_marketplace(&label, &out.progress)
        } else {
            out.progress.headline(&format!("Fetching {shown}…"));
            store.add_marketplace(&fetch_source(known), &out.progress)
        };
        out.progress.finish();
        match result {
            Ok(outcome) => {
                let count = plugins_count(outcome.plugins);
                if !known.fetched {
                    say!("Fetched {} ({count})", outcome.name);
                } else if outcome.changed {
                    say!("Updated {} ({count})", outcome.name);
                } else {
                    say!("{} is up to date ({count})", outcome.name);
                }
                warn_skipped(store, out, &outcome.name);
            }
            Err(error) => ok = out.fail(&format!("could not update `{label}`: {error}")),
        }
    }
    ok
}

fn marketplace_remove(store: &PluginStore, out: &Out, name: &str) -> bool {
    let (known, file) = match store
        .known_marketplaces()
        .and_then(|known| Ok((known, store.state()?)))
    {
        Ok(pair) => pair,
        Err(error) => return out.fail(&error),
    };
    let staying: Vec<String> = file
        .plugins
        .keys()
        .filter(|key| PluginId::parse(key).is_ok_and(|id| id.marketplace == name))
        .cloned()
        .collect();
    match store.remove_marketplace(name) {
        Ok(()) => {
            say!("Removed marketplace {name}");
            if let Some(line) = staying_line(&staying) {
                say!("{line}");
            }
            true
        }
        Err(error) if error.starts_with("no marketplace named") => {
            out.fail(&unknown_marketplace(name, &known))
        }
        Err(error) => out.fail(&error),
    }
}

// ===== looking plugins up =====

/// What the user typed, read against this directory and home.
fn selector(text: &str) -> Result<PluginSelector, String> {
    let (cwd, home) = places();
    PluginSelector::parse(text, &cwd, home.as_deref())
}

/// A plugin by name or id, for a command that may need a marketplace the
/// user has never fetched: the one an id names, or for a bare name every
/// one — fetched first, so a fresh machine needs no `marketplace update`.
fn lookup(store: &PluginStore, out: &Out, text: &str) -> Result<PluginId, String> {
    let selector = selector(text)?;
    match &selector {
        PluginSelector::Directory(_) => {}
        PluginSelector::Id(id) => {
            fetch_unfetched(store, out, Some(&id.marketplace));
        }
        PluginSelector::Name(_) => {
            fetch_unfetched(store, out, None);
        }
    }
    store.resolve(&selector).map_err(ambiguity)
}

/// A plugin among those installed, by id (a renamed one followed) or by a
/// bare name — matched against the installed plugins alone, so a name some
/// marketplace also lists is never ambiguous here. Nothing is fetched.
fn installed_id(store: &PluginStore, text: &str) -> Result<PluginId, String> {
    let selector = selector(text)?;
    let PluginSelector::Name(name) = &selector else {
        return store.resolve(&selector).map_err(ambiguity);
    };
    let file = store.state()?;
    let mut found: Vec<PluginId> = file
        .plugins
        .keys()
        .filter_map(|key| PluginId::parse(key).ok())
        .filter(|id| id.name == *name)
        .collect();
    match found.len() {
        0 => Err(format!(
            "no installed plugin is named `{name}` (alter-zero plugin list)"
        )),
        1 => Ok(found.remove(0)),
        _ => Err(ambiguous(name, &found)),
    }
}

/// Fetch the known marketplaces never fetched — the one named `target`
/// when it is one of them (or, when none is named that yet, those whose
/// name is known only once read: folders given in
/// `ALTER_ZERO_PLUGIN_MARKETPLACES`), else all of them. A failure is said
/// and the rest carry on; `false` when anything failed.
fn fetch_unfetched(store: &PluginStore, out: &Out, target: Option<&str>) -> bool {
    let Ok(known) = store.known_marketplaces() else {
        return true;
    };
    let named = |k: &KnownMarketplace, name: &str| k.name.as_deref() == Some(name);
    let chosen: Vec<&KnownMarketplace> = match target {
        Some(target) if known.iter().any(|k| k.fetched && named(k, target)) => Vec::new(),
        Some(target) if known.iter().any(|k| named(k, target)) => known
            .iter()
            .filter(|k| !k.fetched && named(k, target))
            .collect(),
        Some(_) => known
            .iter()
            .filter(|k| !k.fetched && k.name.is_none())
            .collect(),
        None => known.iter().filter(|k| !k.fetched).collect(),
    };
    let home = store_home();
    let mut ok = true;
    for known in chosen {
        let shown = known.source.display(home.as_deref());
        out.progress.headline(&format!("Fetching {shown}…"));
        let result = store.ensure_marketplace(known, &out.progress);
        out.progress.finish();
        if let Err(error) = result {
            ok = out.fail(&format!("could not fetch {shown}: {error}"));
        }
    }
    ok
}

/// The source a never-fetched known marketplace is fetched from: its own,
/// with the ref it is known with.
fn fetch_source(known: &KnownMarketplace) -> MarketplaceSource {
    match &known.source {
        MarketplaceSource::Git { url, reference } => MarketplaceSource::Git {
            url: url.clone(),
            reference: known.reference.clone().or_else(|| reference.clone()),
        },
        local @ MarketplaceSource::Local { .. } => local.clone(),
    }
}

/// `--ref` applied to a parsed source: a folder has none, and a source
/// already naming one must name the same.
fn with_reference(
    source: MarketplaceSource,
    input: &str,
    reference: Option<&str>,
    home: Option<&Path>,
) -> Result<MarketplaceSource, String> {
    let Some(wanted) = reference else {
        return Ok(source);
    };
    match source {
        MarketplaceSource::Local { .. } => Err(format!(
            "{} is a folder — --ref names a branch or tag of a git marketplace",
            source.display(home)
        )),
        MarketplaceSource::Git {
            reference: Some(given),
            ..
        } if given != wanted => Err(format!(
            "`{input}` already names `{given}` — drop --ref or make the two agree"
        )),
        MarketplaceSource::Git { url, .. } => Ok(MarketplaceSource::Git {
            url,
            reference: Some(wanted.to_string()),
        }),
    }
}

/// The working directory and `$HOME`, which a typed path is read against.
fn places() -> (PathBuf, Option<PathBuf>) {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    (cwd, store_home())
}

fn store_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// A path for a person: under `$HOME` as `~/…`.
fn shown_path(path: &Path) -> String {
    MarketplaceSource::Local {
        path: path.to_path_buf(),
    }
    .display(store_home().as_deref())
}

/// Write `text` to stdout. A closed pipe is not an error worth a panic:
/// whoever read the output has all it wanted.
fn emit(text: &str) {
    let mut stdout = std::io::stdout().lock();
    let _ = stdout
        .write_all(text.as_bytes())
        .and_then(|()| stdout.flush());
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

// ===== where the output goes =====

/// The two streams and how to dress them.
struct Out {
    /// Marks coloured on stdout.
    paint: Paint,
    /// `error:` / `warning:` coloured on stderr.
    paint_err: Paint,
    /// The width a listing is cut to: `None` on a pipe, which gets it whole.
    width: Option<usize>,
    /// How the user invoked us, for every command a message suggests.
    bin: String,
    progress: Progress,
}

impl Out {
    fn new() -> Self {
        let colour = |is_terminal: bool| {
            Paint(
                is_terminal
                    && cli::colour_enabled(
                        std::env::var("NO_COLOR").ok().as_deref(),
                        std::env::var("TERM").ok().as_deref(),
                    ),
            )
        };
        let stdout_tty = std::io::stdout().is_terminal();
        let stderr_tty = std::io::stderr().is_terminal();
        Self {
            paint: colour(stdout_tty),
            paint_err: colour(stderr_tty),
            width: terminal_width(stdout_tty),
            bin: cli::bin_name(std::env::args().next().as_deref()),
            progress: Progress::new(stderr_tty),
        }
    }

    /// Say a failure on stderr; always `false`, for `ok = out.fail(…)`.
    fn fail(&self, message: &str) -> bool {
        self.progress.finish();
        let message = with_bin(&with_hint(message), &self.bin);
        eprintln!("{} {message}", self.paint_err.error("error:"));
        false
    }

    fn warning(&self, message: &str) {
        eprintln!("{} {message}", self.paint_err.warn("warning:"));
    }
}

/// `COLUMNS` when set, else the terminal's own width, else — on a pipe —
/// `None`: a listing piped somewhere is printed whole.
fn terminal_width(stdout_tty: bool) -> Option<usize> {
    if let Some(columns) = std::env::var("COLUMNS")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|&columns| columns > 0)
    {
        return Some(columns);
    }
    stdout_tty.then(|| {
        ratatui::crossterm::terminal::size()
            .ok()
            .map(|(columns, _)| usize::from(columns))
            .filter(|&columns| columns > 0)
            .unwrap_or(FALLBACK_WIDTH)
    })
}

/// Progress on stderr, for a terminal only: a headline per operation, the
/// store's steps one line each under it, and git's percentages rewritten in
/// place on the step's own line. On a pipe it says nothing.
struct Progress {
    on: bool,
    /// The step the open line shows.
    step: RefCell<String>,
    /// A line is open — written without its newline.
    open: Cell<bool>,
}

impl Progress {
    fn new(on: bool) -> Self {
        Self {
            on,
            step: RefCell::default(),
            open: Cell::new(false),
        }
    }

    fn headline(&self, text: &str) {
        if self.on {
            self.finish();
            eprintln!("{text}");
        }
    }

    /// Close an open line.
    fn finish(&self) {
        if self.open.replace(false) {
            eprintln!();
        }
    }
}

impl GitControl for Progress {
    fn progress(&self, progress: GitProgress) {
        let Some(percent) = progress.percent.filter(|_| self.on) else {
            return;
        };
        let step = self.step.borrow();
        let lead = if step.is_empty() {
            String::new()
        } else {
            format!("{step} — ")
        };
        eprint!("\r\x1b[K  {lead}{} {percent}%", progress.phase);
        let _ = std::io::stderr().flush();
        self.open.set(true);
    }
}

impl Control for Progress {
    fn step(&self, text: &str) {
        if !self.on {
            return;
        }
        self.finish();
        text.clone_into(&mut self.step.borrow_mut());
        eprint!("  {text}…");
        let _ = std::io::stderr().flush();
        self.open.set(true);
    }
}

// ===== the text (pure) =====

/// Marks and leads dressed in the terminal's own colours, or left plain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Paint(bool);

impl Paint {
    fn sgr(self, code: &str, text: &str) -> String {
        if self.0 && !text.is_empty() {
            format!("\x1b[{code}m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    }
    fn green(self, text: &str) -> String {
        self.sgr("32", text)
    }
    fn dim(self, text: &str) -> String {
        self.sgr("2", text)
    }
    fn amber(self, text: &str) -> String {
        self.sgr("33", text)
    }
    fn bold(self, text: &str) -> String {
        self.sgr("1", text)
    }
    fn error(self, text: &str) -> String {
        self.sgr("1;31", text)
    }
    fn warn(self, text: &str) -> String {
        self.sgr("1;33", text)
    }

    /// A status's mark, coloured: `✔` on, `◯` off, `⊘` not installable,
    /// a blank of the same width for one merely available.
    fn mark(self, status: PluginStatus) -> String {
        match status {
            PluginStatus::Enabled => self.green("✔"),
            PluginStatus::Disabled => self.dim("◯"),
            PluginStatus::Unsupported => self.amber("⊘"),
            PluginStatus::Available => " ".to_string(),
        }
    }
}

/// A store message that suggests a command, naming the binary as invoked.
fn with_bin(message: &str, bin: &str) -> String {
    if bin == "alter-zero" {
        message.to_string()
    } else {
        message.replace("alter-zero plugin", &format!("{bin} plugin"))
    }
}

/// A store error that a command answers, with that command after it: the
/// store says what is wrong, and only here is there a command line to
/// point at.
fn with_hint(message: &str) -> String {
    let hint =
        if message.starts_with("no plugin named `") && message.ends_with("in your marketplaces") {
            Some("alter-zero plugin list --available")
        } else if message.starts_with("no marketplace named `") && message.ends_with('`') {
            Some("alter-zero plugin marketplace list")
        } else {
            None
        };
    match hint {
        Some(hint) => format!("{message} ({hint})"),
        None => message.to_string(),
    }
}

/// The store's "in more than one marketplace" error with its ids one to a
/// line, so each can be copied whole; any other error as it is.
fn ambiguity(error: String) -> String {
    match error.split_once(" — name one: ") {
        Some((head, list)) if head.ends_with("is in more than one marketplace") => {
            let ids: Vec<&str> = list.split(", ").collect();
            format!("{head} — name one:\n  {}", ids.join("\n  "))
        }
        _ => error,
    }
}

/// `name` matches several plugins: each id on a line of its own.
fn ambiguous(name: &str, ids: &[PluginId]) -> String {
    let ids: Vec<String> = ids.iter().map(ToString::to_string).collect();
    format!(
        "`{name}` is in more than one marketplace — name one:\n  {}",
        ids.join("\n  ")
    )
}

/// `skipped 2 entries: a — why; b — why`, or `None` for none. The reader
/// gives a sentence each to the first entries and closes on one counting
/// the rest (`480 more entries could not be read`), which the count adds.
fn skipped_line(skipped: &[String]) -> Option<String> {
    let (last, _) = skipped.split_last()?;
    let more = last
        .split_once(" more entr")
        .and_then(|(n, _)| n.parse::<usize>().ok());
    let total = match more {
        Some(n) => skipped.len() - 1 + n,
        None => skipped.len(),
    };
    Some(format!(
        "skipped {total} entr{}: {}",
        if total == 1 { "y" } else { "ies" },
        skipped.join("; ")
    ))
}

/// The version a row shows: the installed one, then `→` the one the
/// marketplace offers when an update waits and names another.
fn version_step(installed: Option<&str>, offered: Option<&str>, update: bool) -> Option<String> {
    match (installed, offered) {
        (Some(installed), Some(offered)) if update && installed != offered => {
            Some(format!("{installed} → {offered}"))
        }
        (Some(installed), _) => Some(installed.to_string()),
        (None, offered) => offered.map(str::to_string),
    }
}

/// `1 plugin`, `317 plugins`.
fn plugins_count(n: usize) -> String {
    format!("{n} plugin{}", if n == 1 { "" } else { "s" })
}

/// `3 plugins checked, 1 updated`.
fn checked_line(checked: usize, updated: usize) -> String {
    format!("{} checked, {updated} updated", plugins_count(checked))
}

/// The one line an update of `outcome` prints, `old` the version installed
/// before it.
fn update_line(outcome: &InstallOutcome, old: Option<&str>) -> String {
    let id = &outcome.id;
    match (outcome.changed, outcome.version.as_deref()) {
        (false, Some(version)) => format!("{id} is up to date (v{version})"),
        (false, None) => format!("{id} is up to date"),
        (true, Some(version)) if old != Some(version) => format!("Updated {id} to v{version}"),
        (true, Some(version)) => format!("Updated {id} (v{version})"),
        (true, None) => format!("Updated {id}"),
    }
}

/// What `install` prints: the id and version, then what is live now and
/// what waits for the startup review.
fn installed_outcome_text(outcome: &InstallOutcome, again: bool) -> String {
    let verb = if again { "Reinstalled" } else { "Installed" };
    let mut text = format!("{verb} {}", outcome.id);
    if let Some(version) = &outcome.version {
        text.push_str(&format!(" (v{version})"));
    }
    text.push('\n');
    let live = outcome.live_summary();
    let review = outcome.review_summary();
    for line in [&live, &review].into_iter().flatten() {
        text.push_str(&format!("  {line}\n"));
    }
    if live.is_none() && review.is_none() {
        text.push_str(&format!(
            "  It adds nothing {} uses.\n",
            alter_zero::APP_NAME
        ));
    }
    if !outcome.components.unsupported.is_empty() {
        text.push_str(&format!(
            "  Not used here: {}.\n",
            outcome.components.unsupported.join(", ")
        ));
    }
    text
}

/// `2 plugins from it stay installed: a@team, b@team`.
fn staying_line(ids: &[String]) -> Option<String> {
    match ids {
        [] => None,
        [one] => Some(format!("1 plugin from it stays installed: {one}")),
        many => Some(format!(
            "{} plugins from it stay installed: {}",
            many.len(),
            many.join(", ")
        )),
    }
}

fn no_marketplaces_line(bin: &str) -> String {
    format!("No marketplaces. Add one with: {bin} plugin marketplace add <owner/repo>")
}

/// No marketplace by that name, and the ones there are.
fn unknown_marketplace(name: &str, known: &[KnownMarketplace]) -> String {
    let names: Vec<&str> = known.iter().filter_map(|k| k.name.as_deref()).collect();
    if names.is_empty() {
        format!("no marketplace named `{name}` — you have none")
    } else {
        format!(
            "no marketplace named `{name}` — you have: {}",
            names.join(", ")
        )
    }
}

/// `text` cut to `width` display columns, `…` closing a cut.
fn cut(text: &str, width: usize) -> String {
    if text.width() <= width {
        return text.to_string();
    }
    if width == 0 {
        return String::new();
    }
    let mut kept = String::new();
    let mut used = 0;
    for ch in text.chars() {
        let w = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + w > width - 1 {
            break;
        }
        kept.push(ch);
        used += w;
    }
    kept.truncate(kept.trim_end().len());
    kept.push('…');
    kept
}

/// Greedy word wrap at `width` display columns; a word wider than the line
/// stands alone. `None` keeps the text on one line.
fn wrap(text: &str, width: Option<usize>) -> Vec<String> {
    let Some(width) = width.filter(|&w| w > 0) else {
        return vec![text.split_whitespace().collect::<Vec<_>>().join(" ")];
    };
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        if !line.is_empty() && line.width() + 1 + word.width() > width {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() || lines.is_empty() {
        lines.push(line);
    }
    lines
}

/// Rows of cells padded to each column's widest (display columns), the
/// gap between them, trailing blanks dropped; a column empty in every row
/// takes no room. `paint` dresses a cell after it is measured, by its
/// column and row.
fn table(rows: &[Vec<String>], paint: impl Fn(usize, usize, &str) -> String) -> Vec<String> {
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
    let widths: Vec<usize> = (0..columns)
        .map(|c| {
            rows.iter()
                .filter_map(|row| row.get(c))
                .map(|cell| cell.width())
                .max()
                .unwrap_or(0)
        })
        .collect();
    rows.iter()
        .enumerate()
        .map(|(r, row)| {
            let mut line = String::new();
            for (c, cell) in row.iter().enumerate() {
                if widths[c] == 0 {
                    continue;
                }
                if c > 0 {
                    line.push_str(COLUMN_GAP);
                }
                line.push_str(&paint(c, r, cell));
                line.push_str(&" ".repeat(widths[c] - cell.width()));
            }
            line.trim_end().to_string()
        })
        .collect()
}

/// One installed plugin, as `list` shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct InstalledRow {
    id: String,
    version: Option<String>,
    enabled: bool,
    /// [`Components::summary`]: `2 skills · 1 command`.
    summary: String,
}

/// `plugin list`: a row per installed plugin, its component summary
/// wrapped under its own column at `width` (on one line when `None`), the
/// breaks between its ` · ` parts.
fn installed_text(rows: &[InstalledRow], paint: Paint, width: Option<usize>, bin: &str) -> String {
    let closing =
        format!("Browse and install more with /plugins, or: {bin} plugin list --available\n");
    if rows.is_empty() {
        return format!("No plugins installed.\n\n{closing}");
    }
    let cells: Vec<Vec<String>> = rows
        .iter()
        .map(|row| {
            vec![
                row.id.clone(),
                row.version.clone().unwrap_or_else(|| "—".to_string()),
                if row.enabled { "enabled" } else { "disabled" }.to_string(),
            ]
        })
        .collect();
    let plain = table(&cells, |_, _, cell| cell.to_string());
    let painted = table(&cells, |c, r, cell| {
        if c == 2 && !rows[r].enabled {
            paint.dim(cell)
        } else {
            cell.to_string()
        }
    });
    let facts = plain.iter().map(|line| line.width()).max().unwrap_or(0);
    // `  ✔ ` then the facts, the gap, and the summary's column.
    let column = 4 + facts + COLUMN_GAP.len();
    let mut text = String::from("Installed plugins\n\n");
    for ((row, line), bare) in rows.iter().zip(painted).zip(&plain) {
        let status = if row.enabled {
            PluginStatus::Enabled
        } else {
            PluginStatus::Disabled
        };
        let summary = wrap_parts(&row.summary, " · ", width.map(|w| w.saturating_sub(column)));
        text.push_str(&format!("  {} {line}", paint.mark(status)));
        for (i, part) in summary.iter().enumerate() {
            if i == 0 {
                let pad = facts - bare.width();
                text.push_str(&format!("{}{COLUMN_GAP}{part}", " ".repeat(pad)));
            } else {
                text.push_str(&format!("\n{}{part}", " ".repeat(column)));
            }
        }
        text.push('\n');
    }
    text.push('\n');
    text.push_str(&closing);
    text
}

/// `text`'s `separator`-joined parts on as few lines of `width` as fit, a
/// line breaking only between two parts; empty for empty text.
fn wrap_parts(text: &str, separator: &str, width: Option<usize>) -> Vec<String> {
    if text.is_empty() {
        return Vec::new();
    }
    let Some(width) = width.filter(|&w| w > 0) else {
        return vec![text.to_string()];
    };
    let mut lines: Vec<String> = Vec::new();
    for part in text.split(separator) {
        match lines.last_mut() {
            Some(line) if line.width() + separator.width() + part.width() <= width => {
                line.push_str(separator);
                line.push_str(part);
            }
            _ => lines.push(part.to_string()),
        }
    }
    lines
}

/// `plugin list --available`: a block per marketplace, its plugins
/// installed first, each description cut to `width` (whole when `None`).
fn available_text(
    catalog: &PluginCatalog,
    paint: Paint,
    width: Option<usize>,
    bin: &str,
) -> String {
    if catalog.marketplaces.is_empty() {
        return format!("{}\n", no_marketplaces_line(bin));
    }
    let mut text = String::new();
    for (index, market) in catalog.marketplaces.iter().enumerate() {
        if index > 0 {
            text.push('\n');
        }
        let state = match (&market.error, market.fetched) {
            (Some(error), _) => error.clone(),
            (None, false) => "not fetched".to_string(),
            (None, true) => plugins_count(catalog.count_in(index)),
        };
        text.push_str(&paint.bold(&format!("{} — {} ({state})", market.name, market.source)));
        text.push('\n');
        let shown = catalog.filter(CatalogTab::Marketplace(index), "");
        if shown.is_empty() {
            continue;
        }
        text.push('\n');
        let name_width = shown
            .iter()
            .map(|&i| catalog.plugins[i].name.width())
            .max()
            .unwrap_or(0)
            .min(NAME_COLUMN_MAX);
        for i in shown {
            let plugin = &catalog.plugins[i];
            // `  ✔ name   `: the indent, the mark and its space, the name.
            let lead_width = 4 + name_width.max(plugin.name.width()) + COLUMN_GAP.len();
            let unsupported = plugin.status == PluginStatus::Unsupported;
            let note_width = if unsupported {
                UNSUPPORTED_NOTE.width() + 1
            } else {
                0
            };
            let description = wrap(&plugin.description, None).remove(0);
            let mut described = match width {
                Some(width) => cut(&description, width.saturating_sub(lead_width + note_width)),
                None => description,
            };
            if unsupported {
                if !described.is_empty() {
                    described.push(' ');
                }
                described.push_str(&paint.amber(UNSUPPORTED_NOTE));
            }
            let line = format!(
                "  {} {}{}{COLUMN_GAP}{described}",
                paint.mark(plugin.status),
                plugin.name,
                " ".repeat(name_width.saturating_sub(plugin.name.width()))
            );
            text.push_str(line.trim_end());
            text.push('\n');
        }
    }
    text.push_str(&format!(
        "\nInstall one with: {bin} plugin install <plugin>@<marketplace>\n"
    ));
    text
}

/// A status as a word: `available`, `enabled`, `disabled`, `unsupported`.
fn status_word(status: PluginStatus) -> &'static str {
    match status {
        PluginStatus::Available => "available",
        PluginStatus::Enabled => "enabled",
        PluginStatus::Disabled => "disabled",
        PluginStatus::Unsupported => "unsupported",
    }
}

/// `plugin info`: the id over its state, the description, then a labelled
/// row per fact, each value wrapped under its column at `width`.
fn info_text(detail: &PluginDetail, paint: Paint, width: Option<usize>, bin: &str) -> String {
    let id = &detail.id;
    let mut text = paint.bold(&id.to_string());
    if let Some(name) = detail.display_name.as_deref().filter(|n| *n != id.name) {
        text.push_str(&format!(" — {name}"));
    }
    text.push('\n');
    let mut status: Vec<String> = match detail.status {
        PluginStatus::Enabled => vec!["installed".into(), "enabled".into()],
        PluginStatus::Disabled => vec!["installed".into(), "disabled".into()],
        PluginStatus::Available => vec!["not installed".into()],
        PluginStatus::Unsupported => vec!["can't be installed here".into()],
    };
    let installed = detail.installed_version.as_deref().filter(|_| {
        matches!(
            detail.status,
            PluginStatus::Enabled | PluginStatus::Disabled
        )
    });
    let shown = |v: &str| format!("v{v}");
    if let Some(version) = version_step(
        installed.map(shown).as_deref(),
        detail.version.as_deref().map(shown).as_deref(),
        detail.update_available,
    ) {
        status.push(version);
    }
    if detail.update_available {
        status.push("update available".into());
    }
    let mark = match detail.status {
        PluginStatus::Available => String::new(),
        other => format!("{} ", paint.mark(other)),
    };
    text.push_str(&format!("  {mark}{}\n", status.join(" · ")));
    let description = detail.description.trim();
    if !description.is_empty() {
        text.push('\n');
        for paragraph in description.lines().filter(|l| !l.trim().is_empty()) {
            for line in wrap(paragraph, width.map(|w| w.saturating_sub(2))) {
                text.push_str(&format!("  {line}\n"));
            }
        }
    }
    let mut rows: Vec<(&'static str, String)> = Vec::new();
    let mut push = |label: &'static str, value: Option<String>| {
        if let Some(value) = value.filter(|v| !v.trim().is_empty()) {
            rows.push((label, value));
        }
    };
    push("Author", detail.author.clone());
    push("Category", detail.category.clone());
    push("License", detail.license.clone());
    push("Source", Some(detail.source.clone()));
    push(
        "Homepage",
        detail
            .homepage
            .clone()
            .or_else(|| detail.source_url.clone()),
    );
    push("Blocked", detail.blocked.clone());
    match &detail.components {
        None => push(
            "Contains",
            Some("shown after install — its files are in another repository".into()),
        ),
        Some(components) if components.is_empty() => push(
            "Contains",
            Some(format!("nothing {} uses", alter_zero::APP_NAME)),
        ),
        Some(components) => component_rows(components, &mut push),
    }
    text.push('\n');
    let column = 2 + INFO_LABEL_WIDTH + COLUMN_GAP.len();
    for (label, value) in rows {
        let lines = wrap(&value, width.map(|w| w.saturating_sub(column)));
        for (i, line) in lines.iter().enumerate() {
            if i == 0 {
                text.push_str(&format!("  {label:<INFO_LABEL_WIDTH$}{COLUMN_GAP}{line}\n"));
            } else {
                text.push_str(&format!("{}{line}\n", " ".repeat(column)));
            }
        }
    }
    let next = match detail.status {
        PluginStatus::Available => Some(format!("Install it with: {bin} plugin install {id}")),
        _ if detail.update_available => Some(format!("Update it with: {bin} plugin update {id}")),
        _ => None,
    };
    if let Some(next) = next {
        text.push('\n');
        text.push_str(&next);
        text.push('\n');
    }
    text
}

/// The component rows of `plugin info`.
fn component_rows(components: &Components, push: &mut impl FnMut(&'static str, Option<String>)) {
    let joined = |items: &[String]| (!items.is_empty()).then(|| items.join(", "));
    push("Skills", joined(&components.skills));
    push("Commands", joined(&components.commands));
    push("Agents", joined(&components.agents));
    push("Hooks", joined(&components.hooks));
    push("MCP", joined(&components.mcp_servers));
    push("Not used", joined(&components.unsupported));
}

/// One known marketplace, as `marketplace list` shows it — and, its
/// skipped fields aside, as `--json` prints it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct MarketplaceRow {
    /// `None` while unfetched and unnamed.
    name: Option<String>,
    /// The canonical source.
    source: String,
    #[serde(skip)]
    shown: String,
    default: bool,
    fetched: bool,
    plugins: Option<usize>,
    updated: Option<u64>,
    revision: Option<String>,
    /// A default the user removed: listed with the command that adds it
    /// back.
    removed: bool,
    /// One sentence per entry the marketplace lists but could not be read.
    skipped: Vec<String>,
    #[serde(skip)]
    local: bool,
    #[serde(skip)]
    error: Option<String>,
}

impl MarketplaceRow {
    /// `known`, the `index`-th known marketplace, which is the catalog's
    /// `index`-th too (the catalog lists the known ones first, in order).
    fn new(
        known: &KnownMarketplace,
        catalog: &PluginCatalog,
        index: usize,
        home: Option<&Path>,
    ) -> Self {
        let market = catalog.marketplaces.get(index);
        let error = market.and_then(|m| m.error.clone());
        Self {
            name: known.name.clone(),
            source: known.source.canonical(),
            shown: known.source.display(home),
            default: known.is_default,
            fetched: known.fetched,
            plugins: (known.fetched && market.is_some() && error.is_none())
                .then(|| catalog.count_in(index)),
            updated: known.updated,
            revision: known.revision.clone(),
            removed: false,
            skipped: market.map(|m| m.skipped.clone()).unwrap_or_default(),
            local: matches!(known.source, MarketplaceSource::Local { .. }),
            error,
        }
    }

    /// A default marketplace the user removed.
    fn removed(default: &DefaultMarketplace, home: Option<&Path>) -> Self {
        Self {
            name: default.name.clone(),
            source: default.source.canonical(),
            shown: default.source.display(home),
            default: true,
            fetched: false,
            plugins: None,
            updated: None,
            revision: None,
            removed: true,
            skipped: Vec::new(),
            local: matches!(default.source, MarketplaceSource::Local { .. }),
            error: None,
        }
    }
}

/// `updated 2h ago`.
fn age(now: u64, then: u64) -> String {
    let seconds = now.saturating_sub(then);
    match seconds {
        0..60 => "updated just now".to_string(),
        60..3600 => format!("updated {}m ago", seconds / 60),
        3600..86_400 => format!("updated {}h ago", seconds / 3600),
        _ => format!("updated {}d ago", seconds / 86_400),
    }
}

/// `plugin marketplace list`.
fn marketplaces_text(rows: &[MarketplaceRow], now: u64, paint: Paint, bin: &str) -> String {
    if rows.is_empty() {
        return format!("{}\n", no_marketplaces_line(bin));
    }
    let digits = rows
        .iter()
        .filter_map(|row| row.plugins)
        .map(|n| n.to_string().len())
        .max()
        .unwrap_or(0);
    let cells: Vec<Vec<String>> = rows
        .iter()
        .map(|row| {
            let count = match (row.plugins, row.fetched) {
                _ if row.removed => "removed".to_string(),
                (Some(n), _) => format!("{n:>digits$} plugin{}", if n == 1 { "" } else { "s" }),
                (None, true) => "could not be read".to_string(),
                (None, false) => "not fetched yet".to_string(),
            };
            let when = match (row.local, row.updated) {
                _ if row.removed => String::new(),
                (true, _) => "local folder".to_string(),
                (false, Some(updated)) if row.fetched => age(now, updated),
                _ => String::new(),
            };
            vec![
                row.name.clone().unwrap_or_else(|| "—".to_string()),
                row.shown.clone(),
                count,
                when,
                if row.default { "default" } else { "" }.to_string(),
            ]
        })
        .collect();
    let lines = table(&cells, |c, r, cell| match c {
        2 if rows[r].plugins.is_none() => paint.dim(cell),
        3 | 4 => paint.dim(cell),
        _ => cell.to_string(),
    });
    let mut text = String::new();
    for line in lines {
        text.push_str(&format!("  {line}\n"));
    }
    for row in rows {
        if let Some(error) = &row.error {
            let name = row.name.as_deref().unwrap_or(&row.shown);
            text.push_str(&format!("\n{name}: {error}\n"));
        }
    }
    for row in rows.iter().filter(|row| row.removed) {
        let name = row.name.as_deref().unwrap_or(&row.shown);
        text.push_str(&format!(
            "\nRestore {name} with: {bin} plugin marketplace add {}\n",
            row.source
        ));
    }
    text
}

// ===== the JSON documents (pure) =====
//
// Each is one compact document, built from borrowed records (never a
// `Value` tree) — `docs/plugin-cli.md` lists the shapes.

#[derive(Serialize)]
struct InstalledJson<'a> {
    plugins: Vec<InstalledPluginJson<'a>>,
}

#[derive(Serialize)]
struct InstalledPluginJson<'a> {
    id: &'a str,
    name: &'a str,
    marketplace: &'a str,
    /// What its marketplace offers now, else the installed version.
    version: Option<&'a str>,
    installed_version: Option<&'a str>,
    enabled: bool,
    description: Option<&'a str>,
}

/// `plugin list --json`; `offered` is what each plugin's marketplace offers
/// now, in the file's order.
fn installed_json(file: &PluginsFile, offered: &[Option<&str>]) -> String {
    let plugins = file
        .plugins
        .iter()
        .enumerate()
        .map(|(index, (key, record))| {
            let (name, marketplace) = key.rsplit_once('@').unwrap_or((key.as_str(), ""));
            InstalledPluginJson {
                id: key,
                name,
                marketplace,
                version: offered
                    .get(index)
                    .copied()
                    .flatten()
                    .or(record.version.as_deref()),
                installed_version: record.version.as_deref(),
                enabled: record.enabled,
                description: record.description.as_deref(),
            }
        })
        .collect();
    serde_json::to_string(&InstalledJson { plugins }).unwrap_or_default()
}

#[derive(Serialize)]
struct AvailableJson<'a> {
    marketplaces: Vec<AvailableMarketplaceJson<'a>>,
}

#[derive(Serialize)]
struct AvailableMarketplaceJson<'a> {
    name: &'a str,
    source: &'a str,
    plugins: Vec<AvailablePluginJson<'a>>,
}

#[derive(Serialize)]
struct AvailablePluginJson<'a> {
    name: &'a str,
    description: &'a str,
    category: Option<&'a str>,
    version: Option<&'a str>,
    installed_version: Option<&'a str>,
    status: &'static str,
}

/// `plugin list --available --json`.
fn available_json(catalog: &PluginCatalog) -> String {
    let marketplaces = catalog
        .marketplaces
        .iter()
        .enumerate()
        .map(|(index, market)| AvailableMarketplaceJson {
            name: &market.name,
            source: &market.source,
            plugins: catalog
                .filter(CatalogTab::Marketplace(index), "")
                .into_iter()
                .map(|i| {
                    let plugin = &catalog.plugins[i];
                    AvailablePluginJson {
                        name: &plugin.name,
                        description: &plugin.description,
                        category: plugin.category.as_deref(),
                        version: plugin.version.as_deref(),
                        installed_version: plugin.installed_version.as_deref(),
                        status: status_word(plugin.status),
                    }
                })
                .collect(),
        })
        .collect();
    serde_json::to_string(&AvailableJson { marketplaces }).unwrap_or_default()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DetailJson<'a> {
    id: String,
    display_name: Option<&'a str>,
    description: &'a str,
    version: Option<&'a str>,
    installed_version: Option<&'a str>,
    author: Option<&'a str>,
    category: Option<&'a str>,
    homepage: Option<&'a str>,
    license: Option<&'a str>,
    source: &'a str,
    source_url: Option<&'a str>,
    format: &'static str,
    status: &'static str,
    update_available: bool,
    blocked: Option<&'a str>,
    components: Option<ComponentsJson<'a>>,
    warnings: &'a [String],
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ComponentsJson<'a> {
    skills: &'a [String],
    commands: &'a [String],
    agents: &'a [String],
    hooks: &'a [String],
    mcp_servers: &'a [String],
    unsupported: &'a [String],
}

/// `plugin info --json`.
fn detail_json(detail: &PluginDetail) -> String {
    let doc = DetailJson {
        id: detail.id.to_string(),
        display_name: detail.display_name.as_deref(),
        description: &detail.description,
        version: detail.version.as_deref(),
        installed_version: detail.installed_version.as_deref(),
        author: detail.author.as_deref(),
        category: detail.category.as_deref(),
        homepage: detail.homepage.as_deref(),
        license: detail.license.as_deref(),
        source: &detail.source,
        source_url: detail.source_url.as_deref(),
        format: detail.format.as_str(),
        status: status_word(detail.status),
        update_available: detail.update_available,
        blocked: detail.blocked.as_deref(),
        components: detail.components.as_ref().map(|c| ComponentsJson {
            skills: &c.skills,
            commands: &c.commands,
            agents: &c.agents,
            hooks: &c.hooks,
            mcp_servers: &c.mcp_servers,
            unsupported: &c.unsupported,
        }),
        warnings: &detail.warnings,
    };
    serde_json::to_string(&doc).unwrap_or_default()
}

#[derive(Serialize)]
struct MarketplacesJson<'a> {
    marketplaces: &'a [MarketplaceRow],
}

/// `plugin marketplace list --json`.
fn marketplaces_json(rows: &[MarketplaceRow]) -> String {
    serde_json::to_string(&MarketplacesJson { marketplaces: rows }).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use alter_zero::plugins::{CatalogMarketplace, CatalogPlugin, ManifestFormat, PluginRecord};

    const PLAIN: Paint = Paint(false);

    fn id(text: &str) -> PluginId {
        PluginId::parse(text).expect("id")
    }

    fn outcome(text: &str, version: Option<&str>, changed: bool) -> InstallOutcome {
        InstallOutcome {
            id: id(text),
            version: version.map(str::to_string),
            changed,
            components: Components::default(),
            warnings: Vec::new(),
        }
    }

    #[test]
    fn a_table_aligns_on_display_columns_and_drops_trailing_blanks() {
        let rows = vec![
            vec!["日本".to_string(), "a".to_string(), String::new()],
            vec!["abc".to_string(), "bb".to_string(), String::new()],
        ];
        let lines = table(&rows, |_, _, cell| cell.to_string());
        assert_eq!(lines, ["日本   a", "abc    bb"]);
    }

    #[test]
    fn cut_keeps_display_columns_and_closes_on_an_ellipsis() {
        assert_eq!(cut("short", 10), "short");
        assert_eq!(cut("a long description here", 10), "a long de…");
        assert_eq!(cut("a long  description", 9), "a long…");
        assert_eq!(cut("日本語の説明", 7).width(), 7);
        assert!(cut("日本語の説明", 7).ends_with('…'));
        assert_eq!(cut("anything", 0), "");
    }

    #[test]
    fn wrap_breaks_between_words_and_keeps_a_long_word_whole() {
        assert_eq!(
            wrap("one two three four", Some(9)),
            ["one two", "three", "four"]
        );
        assert_eq!(
            wrap("averyveryverylongword x", Some(5)),
            ["averyveryverylongword", "x"]
        );
        assert_eq!(wrap("one   two\nthree", None), ["one two three"]);
    }

    #[test]
    fn an_ambiguous_name_lists_each_id_on_its_own_line() {
        let error = "`review` is in more than one marketplace — name one: review@official, \
                     review@team"
            .to_string();
        assert_eq!(
            ambiguity(error),
            "`review` is in more than one marketplace — name one:\n  review@official\n  \
             review@team"
        );
        assert_eq!(ambiguity("other".into()), "other");
    }

    #[test]
    fn a_store_error_gains_the_command_that_helps() {
        assert_eq!(
            with_hint("no plugin named `x` in your marketplaces"),
            "no plugin named `x` in your marketplaces (alter-zero plugin list --available)"
        );
        assert_eq!(
            with_hint("no marketplace named `m`"),
            "no marketplace named `m` (alter-zero plugin marketplace list)"
        );
        let listed = "no marketplace named `m` — you have: team";
        assert_eq!(with_hint(listed), listed);
    }

    #[test]
    fn a_waiting_update_shows_as_a_step_from_the_installed_version() {
        assert_eq!(
            version_step(Some("1.0.0"), Some("2.0.0"), true).as_deref(),
            Some("1.0.0 → 2.0.0")
        );
        assert_eq!(
            version_step(Some("1.0.0"), Some("2.0.0"), false).as_deref(),
            Some("1.0.0")
        );
        assert_eq!(
            version_step(Some("1.0.0"), Some("1.0.0"), true).as_deref(),
            Some("1.0.0")
        );
        assert_eq!(
            version_step(None, Some("2.0.0"), false).as_deref(),
            Some("2.0.0")
        );
        assert_eq!(version_step(None, None, true), None);
        assert_eq!(with_hint("something else"), "something else");
    }

    #[test]
    fn a_suggested_command_names_the_binary_as_invoked() {
        let message = "no plugin named `x` (alter-zero plugin list --available)";
        assert_eq!(with_bin(message, "alter-zero"), message);
        assert_eq!(
            with_bin(message, "az"),
            "no plugin named `x` (az plugin list --available)"
        );
    }

    #[test]
    fn an_update_says_whether_the_version_moved() {
        assert_eq!(
            update_line(&outcome("a@m", Some("1.1.0"), true), Some("1.0.0")),
            "Updated a@m to v1.1.0"
        );
        assert_eq!(
            update_line(&outcome("a@m", Some("1.0.0"), false), Some("1.0.0")),
            "a@m is up to date (v1.0.0)"
        );
        assert_eq!(
            update_line(&outcome("a@local", Some("1.0.0"), true), Some("1.0.0")),
            "Updated a@local (v1.0.0)"
        );
        assert_eq!(
            update_line(&outcome("a@m", None, true), None),
            "Updated a@m"
        );
        assert_eq!(checked_line(3, 1), "3 plugins checked, 1 updated");
        assert_eq!(checked_line(1, 0), "1 plugin checked, 0 updated");
    }

    #[test]
    fn an_install_says_what_is_live_and_what_waits_for_review() {
        let mut done = outcome("code-review@official", Some("1.0.0"), true);
        done.components.skills = vec!["code-review:a".into(), "code-review:b".into()];
        done.components.commands = vec!["/code-review:go".into()];
        done.components.hooks = vec!["PreToolUse · Bash".into()];
        done.components.mcp_servers = vec!["db (stdio)".into()];
        assert_eq!(
            installed_outcome_text(&done, false),
            "Installed code-review@official (v1.0.0)\n  \
             2 skills and 1 command are ready for your next message.\n  \
             1 hook and 1 MCP server wait for your review at the next launch.\n"
        );
        let empty = outcome("x@local", None, true);
        let text = installed_outcome_text(&empty, true);
        assert!(text.starts_with("Reinstalled x@local\n"), "{text}");
        assert!(text.contains("adds nothing Alter Zero uses"), "{text}");
    }

    #[test]
    fn the_installed_list_aligns_its_columns_and_points_at_more() {
        let rows = [
            InstalledRow {
                id: "code-review@claude-plugins-official".into(),
                version: Some("1.0.0".into()),
                enabled: true,
                summary: "2 skills · 1 command".into(),
            },
            InstalledRow {
                id: "notes@local".into(),
                version: None,
                enabled: false,
                summary: String::new(),
            },
        ];
        assert_eq!(
            installed_text(&rows, PLAIN, None, "az"),
            "Installed plugins\n\n  \
             ✔ code-review@claude-plugins-official   1.0.0   enabled    2 skills · 1 command\n  \
             ◯ notes@local                           —       disabled\n\n\
             Browse and install more with /plugins, or: az plugin list --available\n"
        );
        assert_eq!(
            installed_text(&[], PLAIN, None, "alter-zero"),
            "No plugins installed.\n\n\
             Browse and install more with /plugins, or: alter-zero plugin list --available\n"
        );
    }

    #[test]
    fn a_long_summary_wraps_under_its_column_between_parts() {
        let rows = [InstalledRow {
            id: "figma@openai-curated".into(),
            version: Some("2.0.20".into()),
            enabled: true,
            summary: "12 skills · 4 commands · 1 hook · 1 MCP server".into(),
        }];
        let text = installed_text(&rows, PLAIN, Some(70), "az");
        assert!(
            text.contains(&format!(
                "\n  ✔ figma@openai-curated   2.0.20   enabled   12 skills · 4 commands\n\
                 {}1 hook · 1 MCP server\n",
                " ".repeat(46)
            )),
            "{text}"
        );
        assert_eq!(wrap_parts("", " · ", Some(5)), Vec::<String>::new());
        assert_eq!(wrap_parts("a · b", " · ", None), ["a · b"]);
    }

    #[test]
    fn colour_dresses_only_the_marks_and_the_disabled_state() {
        let rows = [InstalledRow {
            id: "a@m".into(),
            version: None,
            enabled: false,
            summary: String::new(),
        }];
        let text = installed_text(&rows, Paint(true), None, "az");
        assert!(text.contains("\x1b[2m◯\x1b[0m"), "{text:?}");
        assert!(text.contains("\x1b[2mdisabled\x1b[0m"), "{text:?}");
        assert!(!installed_text(&rows, PLAIN, None, "az").contains('\x1b'));
    }

    fn catalog() -> PluginCatalog {
        let market = |name: &str| CatalogMarketplace {
            skipped: Vec::new(),
            name: name.into(),
            title: name.into(),
            source: format!("acme/{name}"),
            web_url: None,
            is_default: true,
            is_local: false,
            fetched: true,
            updated: None,
            revision: None,
            error: None,
        };
        let plugin = |name: &str, status, description: &str| CatalogPlugin {
            installed_version: None,
            name: name.into(),
            display_name: None,
            marketplace: 0,
            description: description.into(),
            category: Some("development".into()),
            author: None,
            version: None,
            status,
            update_available: false,
        };
        let mut unfetched = market("openai-curated");
        unfetched.fetched = false;
        PluginCatalog {
            marketplaces: vec![market("official"), unfetched],
            plugins: vec![
                plugin(
                    "agent-sdk",
                    PluginStatus::Available,
                    "Agent SDK development plugin",
                ),
                plugin(
                    "xcode",
                    PluginStatus::Unsupported,
                    "Xcode's own agent plugin",
                ),
                plugin(
                    "review",
                    PluginStatus::Enabled,
                    "Automated code review for pull requests",
                ),
            ],
        }
    }

    #[test]
    fn the_available_list_is_a_block_per_marketplace_installed_first() {
        let text = available_text(&catalog(), PLAIN, None, "az");
        assert_eq!(
            text,
            "official — acme/official (3 plugins)\n\n  \
             ✔ review      Automated code review for pull requests\n  \
             \x20 agent-sdk   Agent SDK development plugin\n  \
             ⊘ xcode       Xcode's own agent plugin (can't be installed here)\n\n\
             openai-curated — acme/openai-curated (not fetched)\n\n\
             Install one with: az plugin install <plugin>@<marketplace>\n"
        );
    }

    #[test]
    fn the_available_list_cuts_descriptions_to_the_width_keeping_the_note() {
        let text = available_text(&catalog(), PLAIN, Some(50), "az");
        // Every row but the closing command, which is never cut.
        for line in text.lines().filter(|line| line.starts_with("  ")) {
            assert!(line.width() <= 50, "{line:?}");
        }
        assert!(
            text.contains("\n  ✔ review      Automated code review for pull re…\n"),
            "{text}"
        );
        assert!(
            text.contains("\n  ⊘ xcode       Xcode's… (can't be installed here)\n"),
            "{text}"
        );
    }

    fn detail() -> PluginDetail {
        PluginDetail {
            installed_version: None,
            id: id("code-review@official"),
            display_name: None,
            description: "Automated code review for pull requests using several agents".into(),
            version: Some("1.0.0".into()),
            author: Some("Anthropic".into()),
            category: Some("development".into()),
            homepage: None,
            license: None,
            source: "plugins/code-review".into(),
            source_url: Some(
                "https://github.com/acme/official/tree/HEAD/plugins/code-review".into(),
            ),
            format: ManifestFormat::Claude,
            status: PluginStatus::Enabled,
            update_available: false,
            blocked: None,
            components: Some(Components {
                skills: vec!["code-review:lint".into(), "code-review:triage".into()],
                commands: vec!["/code-review:code-review".into()],
                agents: Vec::new(),
                hooks: vec!["PreToolUse · Bash".into()],
                mcp_servers: vec!["github (http)".into()],
                unsupported: vec!["1 app".into()],
            }),
            warnings: Vec::new(),
        }
    }

    #[test]
    fn info_lists_the_facts_under_one_value_column() {
        assert_eq!(
            info_text(&detail(), PLAIN, None, "az"),
            "code-review@official\n  \
             ✔ installed · enabled · v1.0.0\n\n  \
             Automated code review for pull requests using several agents\n\n  \
             Author     Anthropic\n  \
             Category   development\n  \
             Source     plugins/code-review\n  \
             Homepage   https://github.com/acme/official/tree/HEAD/plugins/code-review\n  \
             Skills     code-review:lint, code-review:triage\n  \
             Commands   /code-review:code-review\n  \
             Hooks      PreToolUse · Bash\n  \
             MCP        github (http)\n  \
             Not used   1 app\n"
        );
    }

    #[test]
    fn info_wraps_values_under_their_column_and_says_what_is_next() {
        let mut detail = detail();
        detail.status = PluginStatus::Available;
        detail.components = None;
        let text = info_text(&detail, PLAIN, Some(40), "az");
        // Every row but an address and the closing command, never cut.
        for line in text
            .lines()
            .filter(|line| line.starts_with("  ") && !line.contains("://"))
        {
            assert!(line.width() <= 40, "{line:?}");
        }
        assert!(text.contains("  not installed · v1.0.0\n"), "{text}");
        assert!(
            text.contains(
                "  Contains   shown after install — its\n             files are in another\n             \
                 repository\n"
            ),
            "{text}"
        );
        assert!(text.ends_with("\nInstall it with: az plugin install code-review@official\n"));
    }

    #[test]
    fn age_reads_in_the_largest_whole_unit() {
        assert_eq!(age(1000, 990), "updated just now");
        assert_eq!(age(10_000, 10_000 - 300), "updated 5m ago");
        assert_eq!(age(100_000, 100_000 - 7300), "updated 2h ago");
        assert_eq!(age(1_000_000, 1_000_000 - 3 * 86_400), "updated 3d ago");
    }

    fn row(
        name: Option<&str>,
        plugins: Option<usize>,
        fetched: bool,
        local: bool,
    ) -> MarketplaceRow {
        MarketplaceRow {
            name: name.map(str::to_string),
            source: format!("acme/{}", name.unwrap_or("x")),
            shown: format!("acme/{}", name.unwrap_or("x")),
            default: !local,
            fetched,
            plugins,
            updated: fetched.then_some(1000),
            revision: None,
            removed: false,
            skipped: Vec::new(),
            local,
            error: None,
        }
    }

    #[test]
    fn the_marketplace_list_right_aligns_counts_and_tags_defaults() {
        let rows = [
            row(Some("official"), Some(317), true, false),
            row(Some("openai-curated"), None, false, false),
            row(Some("team"), Some(4), true, true),
        ];
        assert_eq!(
            marketplaces_text(&rows, 1000 + 7200, PLAIN, "az"),
            "  official         acme/official         317 plugins       updated 2h ago   default\n  \
             openai-curated   acme/openai-curated   not fetched yet                    default\n  \
             team             acme/team               4 plugins       local folder\n"
        );
        assert_eq!(
            marketplaces_text(&[], 0, PLAIN, "az"),
            "No marketplaces. Add one with: az plugin marketplace add <owner/repo>\n"
        );
    }

    #[test]
    fn the_json_documents_parse_with_the_documented_keys() {
        let mut file = PluginsFile::default();
        file.plugins.insert(
            "review@official".into(),
            PluginRecord {
                enabled: true,
                version: Some("1.0.0".into()),
                ..PluginRecord::default()
            },
        );
        let installed: serde_json::Value =
            serde_json::from_str(&installed_json(&file, &[Some("2.0.0")])).expect("json");
        assert_eq!(installed["plugins"][0]["version"], "2.0.0");
        assert_eq!(installed["plugins"][0]["installed_version"], "1.0.0");
        assert_eq!(installed["plugins"][0]["id"], "review@official");
        assert_eq!(installed["plugins"][0]["name"], "review");
        assert_eq!(installed["plugins"][0]["marketplace"], "official");
        assert_eq!(installed["plugins"][0]["enabled"], true);

        let available: serde_json::Value =
            serde_json::from_str(&available_json(&catalog())).expect("json");
        let plugins = &available["marketplaces"][0]["plugins"];
        assert_eq!(plugins[0]["name"], "review");
        assert_eq!(plugins[0]["status"], "enabled");
        assert_eq!(plugins[2]["status"], "unsupported");
        assert_eq!(
            available["marketplaces"][1]["plugins"],
            serde_json::json!([])
        );

        let info: serde_json::Value = serde_json::from_str(&detail_json(&detail())).expect("json");
        assert_eq!(info["id"], "code-review@official");
        assert_eq!(info["status"], "enabled");
        assert_eq!(info["format"], "claude");
        assert_eq!(info["updateAvailable"], false);
        assert_eq!(info["components"]["mcpServers"][0], "github (http)");

        let rows = [
            row(Some("official"), Some(3), true, false),
            row(None, None, false, false),
        ];
        let list: serde_json::Value =
            serde_json::from_str(&marketplaces_json(&rows)).expect("json");
        assert_eq!(list["marketplaces"][0]["plugins"], 3);
        assert_eq!(list["marketplaces"][1]["name"], serde_json::Value::Null);
        assert_eq!(list["marketplaces"][1]["updated"], serde_json::Value::Null);
        assert!(list["marketplaces"][0].get("shown").is_none());

        for doc in [installed_json(&file, &[]), detail_json(&detail())] {
            assert!(!doc.contains('\n'), "one compact document");
        }
    }

    #[test]
    fn a_marketplace_that_is_not_known_names_the_ones_that_are() {
        let known = |name: &str| KnownMarketplace {
            name: Some(name.into()),
            source: MarketplaceSource::Git {
                url: "https://example.invalid/x.git".into(),
                reference: None,
            },
            reference: None,
            revision: None,
            updated: None,
            is_default: false,
            fetched: true,
        };
        assert_eq!(
            unknown_marketplace("tema", &[known("team"), known("official")]),
            "no marketplace named `tema` — you have: team, official"
        );
        assert_eq!(
            unknown_marketplace("x", &[]),
            "no marketplace named `x` — you have none"
        );
        assert_eq!(staying_line(&[]), None);
        assert_eq!(
            staying_line(&["a@team".into(), "b@team".into()]).as_deref(),
            Some("2 plugins from it stay installed: a@team, b@team")
        );
    }

    #[test]
    fn ref_is_refused_for_a_folder_and_for_a_source_naming_another() {
        let home = Path::new("/home/me");
        let folder = MarketplaceSource::Local {
            path: PathBuf::from("/home/me/team"),
        };
        let err = with_reference(folder, "~/team", Some("v1"), Some(home)).expect_err("folder");
        assert!(err.starts_with("~/team is a folder"), "{err}");
        let git = |reference: Option<&str>| MarketplaceSource::Git {
            url: "https://github.com/acme/p.git".into(),
            reference: reference.map(str::to_string),
        };
        assert_eq!(
            with_reference(git(None), "acme/p", Some("v1"), None),
            Ok(git(Some("v1")))
        );
        assert_eq!(
            with_reference(git(Some("v1")), "acme/p@v1", Some("v1"), None),
            Ok(git(Some("v1")))
        );
        assert!(with_reference(git(Some("v2")), "acme/p@v2", Some("v1"), None).is_err());
    }
}
