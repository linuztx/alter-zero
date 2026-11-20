//! The `plugin` subcommand boundary (`docs/plugin-cli.md`): what runs
//! behind `alter-zero plugin …` and `alter-zero plugin marketplace …`, by
//! [`super::startup::resolve_cli`] in cooked mode — before the tokio runtime
//! and the terminal, like every other CLI resolution (`docs/cli.md`).
//!
//! The grammar is the pure `cli` module's. This first step answers the two
//! help pages and nothing else: every other command says it is not
//! available yet and exits 1, one arm per command so the executor over the
//! plugin library can replace them one at a time.

use alter_zero::cli::{self, HelpPage, MarketplaceCli, PluginCli};

use super::startup::stdout_style;

/// Run one `plugin` subcommand and return the process exit code: `0` for
/// success and help, `1` for a failure. Grammar errors never reach here —
/// `parse` reports them and `resolve_cli` exits 2.
pub(crate) fn run(cmd: &PluginCli) -> i32 {
    match cmd {
        PluginCli::Help => help(HelpPage::Plugin),
        PluginCli::Marketplace(cmd) => run_marketplace(cmd),
        PluginCli::Install { .. }
        | PluginCli::Uninstall { .. }
        | PluginCli::Enable { .. }
        | PluginCli::Disable { .. }
        | PluginCli::Update { .. }
        | PluginCli::List { .. }
        | PluginCli::Info { .. } => not_available(),
    }
}

/// Run one `plugin marketplace` subcommand — [`run`]'s contract.
fn run_marketplace(cmd: &MarketplaceCli) -> i32 {
    match cmd {
        MarketplaceCli::Help => help(HelpPage::PluginMarketplace),
        MarketplaceCli::Add { .. }
        | MarketplaceCli::AddDefaults
        | MarketplaceCli::List { .. }
        | MarketplaceCli::Update { .. }
        | MarketplaceCli::Remove { .. } => not_available(),
    }
}

/// A help page on stdout, through the same renderer and colour rule as the
/// main `--help` (`docs/cli.md`).
fn help(page: HelpPage) -> i32 {
    println!("{}", cli::help(page, stdout_style()));
    0
}

/// The stand-in for a command whose executor is not written yet.
fn not_available() -> i32 {
    let bin = cli::bin_name(std::env::args().next().as_deref());
    eprintln!("{bin} plugin: not available yet");
    1
}
