//! The `plugin` subcommand boundary (`docs/plugin-cli.md`): what runs behind
//! `alter-zero plugin …` and `alter-zero plugin marketplace …`, by
//! [`super::startup::resolve_cli`] in cooked mode — before the tokio runtime
//! and the terminal, like every other CLI resolution (`docs/cli.md`).
//!
//! A router and nothing more. The grammar is the pure `cli` module's; the
//! work, every word printed and the `--json` shapes are the plugin crate's
//! (`plugins::cli`). What this file adds is what only the app
//! has: the store over the config home with `git` spawned through the
//! tty-detach chain ([`plugin_store`], which the `/plugins` page reuses),
//! the real stdout and stderr, and forgetting `trust.json`'s records under
//! every plugin directory a command installed, updated or removed.

use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use alter_zero::cli::{self, HelpPage, MarketplaceCli, PluginCli};
use alter_zero::plugins::cli as commands;
use alter_zero::plugins::{
    CommandBuilder, CommandOutcome, GIT_TIMEOUT_ENV, GitRunner, MARKETPLACES_ENV, Output,
    PLUGINS_DIR_ENV, PluginStore, PluginsLayout, default_marketplaces, git_timeout,
};

use super::config;
use super::startup::stdout_style;

/// The plugin store a run uses (`docs/plugins.md` *On disk*): the layout
/// under the config home (or `ALTER_ZERO_PLUGINS_DIR`), `git` spawned
/// through the tty-detach chain so a credential prompt can never reach the
/// terminal (`docs/tty-detach.md`), each child leading its own process
/// group, bounded by `ALTER_ZERO_PLUGIN_GIT_TIMEOUT_SECS`, the offered
/// marketplaces (`ALTER_ZERO_PLUGIN_MARKETPLACES`), and typed sources
/// resolved against `cwd` and `HOME`. `None` with no config home.
pub(crate) fn plugin_store(cwd: &Path) -> Option<PluginStore> {
    let layout = PluginsLayout::resolve(
        config::config_home().as_deref(),
        std::env::var_os(PLUGINS_DIR_ENV).as_deref(),
    )?;
    let helper = std::env::current_exe().ok();
    let command: CommandBuilder = Arc::new(move |argv| {
        alter_zero::subprocess::detached_argv_command(helper.as_deref(), argv)
    });
    let git = GitRunner {
        command,
        timeout: git_timeout(std::env::var(GIT_TIMEOUT_ENV).ok().as_deref()),
    };
    let defaults = default_marketplaces(std::env::var(MARKETPLACES_ENV).ok().as_deref());
    let home = std::env::var_os("HOME").map(PathBuf::from);
    Some(
        PluginStore::new(layout)
            .with_git(git)
            .with_defaults(defaults)
            .with_paths(cwd, home),
    )
}

/// Run one `plugin` subcommand and return the process exit code: `0` done
/// (and every help page), `1` a failure. Grammar errors never reach here —
/// `parse` reports them and `resolve_cli` exits 2.
pub(crate) fn run(cmd: &PluginCli) -> i32 {
    match cmd {
        PluginCli::Help => return help(HelpPage::Plugin),
        PluginCli::Marketplace(MarketplaceCli::Help) => {
            return help(HelpPage::PluginMarketplace);
        }
        _ => {}
    }
    let bin = cli::bin_name(std::env::args().next().as_deref());
    let cwd = match std::env::current_dir() {
        Ok(dir) => dir,
        Err(err) => {
            eprintln!("{bin} plugin: the working directory cannot be read: {err}");
            return 1;
        }
    };
    let Some(store) = plugin_store(&cwd) else {
        eprintln!(
            "{bin} plugin: there is no config home to keep plugins in; \
             set HOME or ALTER_ZERO_CONFIG_DIR"
        );
        return 1;
    };
    let (stdout, stderr) = (std::io::stdout(), std::io::stderr());
    let progress = stderr.is_terminal();
    let outcome = {
        let (mut out, mut err) = (stdout.lock(), stderr.lock());
        let mut io = Output {
            out: &mut out,
            err: &mut err,
            progress,
            bin: &bin,
        };
        dispatch(&store, cmd, &cwd, &mut io)
    };
    let mut code = outcome.code;
    let trust = config::trust_json_path();
    for dir in &outcome.forget_trust {
        if let Err(message) = super::trust::forget_plugin_trust(trust.as_deref(), dir) {
            eprintln!("{bin} plugin: {message}");
            code = 1;
        }
    }
    code
}

/// One grammar arm, one plugin command.
fn dispatch(
    store: &PluginStore,
    cmd: &PluginCli,
    cwd: &Path,
    io: &mut Output<'_>,
) -> CommandOutcome {
    match cmd {
        PluginCli::Install { plugin } => commands::install(store, plugin, io),
        PluginCli::Uninstall { plugin } => commands::uninstall(store, plugin, io),
        PluginCli::Enable { plugin } => commands::set_enabled(store, plugin, cwd, true, io),
        PluginCli::Disable { plugin } => commands::set_enabled(store, plugin, cwd, false, io),
        PluginCli::Update { plugin } => commands::update(store, plugin.as_deref(), cwd, io),
        PluginCli::List { available, json } => commands::list(store, cwd, *available, *json, io),
        PluginCli::Info { plugin, json } => commands::info(store, plugin, cwd, *json, io),
        PluginCli::Marketplace(cmd) => match cmd {
            MarketplaceCli::Add { source, git_ref } => {
                commands::marketplace_add(store, source, git_ref.as_deref(), io)
            }
            MarketplaceCli::AddDefaults => commands::marketplace_add_defaults(store, io),
            MarketplaceCli::List { json } => {
                commands::marketplace_list(store, *json, super::host::unix_secs(), io)
            }
            MarketplaceCli::Update { name } => {
                commands::marketplace_update(store, name.as_deref(), io)
            }
            MarketplaceCli::Remove { name } => commands::marketplace_remove(store, name, io),
            MarketplaceCli::Help => CommandOutcome::default(),
        },
        PluginCli::Help => CommandOutcome::default(),
    }
}

/// A help page on stdout, through the same renderer and colour rule as the
/// main `--help` (`docs/cli.md`).
fn help(page: HelpPage) -> i32 {
    println!("{}", cli::help(page, stdout_style()));
    0
}
