# CLI plugins — `alter-zero plugin …` and `plugin marketplace …`

Everything the `/plugins` page does (`docs/plugins.md`), from a shell or a
script: browse what the marketplaces list, install a plugin by name or from
a folder, turn it off and on, update it, and add or drop marketplaces. It is
resolved and answered **before the TUI boots**, like the `mcp` family
(`docs/mcp-cli.md`) and `--continue` (`docs/cli.md`):

```
$ alter-zero plugin install frontend-design@claude-plugins-official
Fetching anthropics/claude-plugins-official…
  cloning github.com/anthropics/claude-plugins-official — Resolving deltas 100%
  reading the marketplace…
Installing frontend-design@claude-plugins-official…
  reading the plugin…
  copying files…
Installed frontend-design@claude-plugins-official (v1.1.1)
  1 skill is ready for your next message.

$ alter-zero plugin list
Installed plugins

  ✔ frontend-design@claude-plugins-official   1.1.1   enabled   1 skill

Browse and install more with /plugins, or: alter-zero plugin list --available

$ alter-zero plugin install figma
error: `figma` is in more than one marketplace — name one:
  figma@claude-plugins-official
  figma@openai-curated
```

(The `Fetching`/`Installing` lines and the steps under them are progress, on
stderr and only on a terminal; the rest is stdout.)

## What the references do (findings)

- **Codex** — `codex plugin add|list|remove` and `codex plugin marketplace
  add|list|upgrade|remove`: one selector grammar, `PLUGIN@MARKETPLACE` or
  `PLUGIN --marketplace M`, `--json` on every command, a broken pipe
  (`| head`) treated as success, `--ref` and a repeatable `--sparse` on
  `marketplace add`. No enable or disable, no plugin update (only
  `marketplace upgrade`), and `--available` only together with `--json`.
  Failures are one `Error: …` line and exit 1, clap usage errors exit 2.
- **Claude Code** — `claude plugin install|uninstall|enable|disable|update|
  list|details|…` and `claude plugin marketplace add|list|remove|update`,
  with a `--scope user|project|local` on most of them; `marketplace add`
  reads `owner/repo[#ref|@ref]`, git URLs, `user@host:path` and local
  folders (and a bare `marketplace.json` URL, whose relative plugin sources
  then cannot resolve).

## What we build

Claude Code's verbs (they include the enable, disable and update Codex
lacks) over Codex's grammar (`name@marketplace`, `--json`, `--ref`, a
`marketplace` sub-family), with one difference from both: **a marketplace
is fetched the first time something needs it.** The two defaults are known
from the start but never fetched at startup (`docs/plugins.md`), so on a
fresh machine `alter-zero plugin install code-review@claude-plugins-official`
fetches `anthropics/claude-plugins-official` and then installs; a bare name
fetches every known marketplace never fetched before it searches.

### The grammar (pure, `src/cli.rs`)

```
alter-zero plugin list [--available] [--json]
alter-zero plugin info <PLUGIN[@MARKETPLACE]> [--json]
alter-zero plugin install <PLUGIN[@MARKETPLACE] | DIR>
alter-zero plugin uninstall <PLUGIN[@MARKETPLACE]>
alter-zero plugin enable <PLUGIN[@MARKETPLACE]>
alter-zero plugin disable <PLUGIN[@MARKETPLACE]>
alter-zero plugin update [PLUGIN[@MARKETPLACE]]
alter-zero plugin marketplace add <SOURCE> [--ref <REF>]
alter-zero plugin marketplace list [--json]
alter-zero plugin marketplace update [NAME]
alter-zero plugin marketplace remove <NAME>
```

- `plugin` (or `plugins`) is recognised as the **first** argument only;
  `parse` peels it off and hands the rest to `parse_plugin`, so the session
  grammar is untouched. `Cli` gains `Plugin(PluginCli)`; `PluginCli` and
  `MarketplaceCli` are the commands above, one variant each, plus `Help`.
- Unlisted aliases: `add` = `install`, `remove`/`rm` = `uninstall`, `show`
  = `info`, `ls` = `list`, `upgrade` = `update`, and under `marketplace`
  (alias `marketplaces`) `ls`, `rm`, `upgrade`. An error names the
  canonical command.
- A plugin stays the **text the user typed**: whether `./x` is a folder and
  what `~` is depend on the working directory and the home, which the
  boundary reads (`plugins::PluginSelector::parse`).
- `--available`, `--json` and `--ref` are the family's flags; one given
  where it does not apply is refused by name (`--json does not apply to
  \`plugin install\``), anything else `-`-led is an unrecognized argument.
  `--ref` takes a value, following or `=`-attached, once. `--` ends the
  flags.
- `-h`/`--help` anywhere before a `--` prints the family's page (exit 0) —
  `cli::help(HelpPage::Plugin, style)`, the main page's renderer: the
  command as its title, the two `Usage:` forms, the eleven commands, the
  `<PLUGIN>` argument and the three flags. **Bare `alter-zero plugin` is a
  usage error naming the commands**, exactly as bare `alter-zero mcp` is.
- Grammar errors are the family's clap-shaped block on stderr,
  `cli::usage_error(HelpPage::Plugin, …)`, exit 2: a missing plugin
  (`` `plugin install` needs a plugin: PLUGIN[@MARKETPLACE] or a
  directory ``), an extra argument, an unknown command (`` unknown plugin
  command `foo` ``), an unknown flag, `--ref` with no value.

### The boundary (`src/tui/plugin_cli.rs`)

`resolve_cli` matches `Cli::Plugin(cmd)` and hands it to
`tui::plugin_cli::run(cmd) -> i32` — cooked-mode stdio, no tokio, no
terminal. The store is `tui::config::plugin_store(true)`: git **may** ask
for credentials here, never inside the TUI. Every operation is the store's
(`plugins::PluginStore`); this file decides the order and the words.

- **Streams.** Results on stdout, a failure as `error: …` and a store
  warning as `warning: …` on stderr. Progress goes to stderr **only when
  stderr is a terminal**: a headline per operation (`Fetching …`,
  `Installing …`, `Updating …`), the store's steps one line each under it,
  and git's percentages rewritten in place on the step's line with `\r`. A
  pipe gets none of it. A closed stdout (`| head`) ends quietly.
- **Colour** follows the `mcp` family's rule, `cli::colour_enabled` ANDed
  with `IsTerminal` per stream: the `✔` green, `◯` and `disabled` dim, `⊘`
  and its note amber, a heading bold, `error:` red — plain text on a pipe.
- **Width.** `COLUMNS` when set, else the terminal's own width, else (a
  pipe) none: `list --available` cuts each description to the width — the
  name column capped at 28 so one long name does not push every row — and
  prints it whole on a pipe; `info` wraps its values under their column.
  All width math is display columns (`unicode-width`).
- **Finding a plugin.** `info` and `install` take a name or an id and fetch
  what they need first (the marketplace an id names, or every never-fetched
  one for a bare name), then `PluginStore::resolve`, which follows a
  marketplace's `renames`. `uninstall`, `enable`, `disable` and `update`
  look among the **installed** plugins alone, so a name another marketplace
  also lists is not ambiguous there, and they never fetch.
- **`list`** — one row per installed plugin: the mark, the id, the version
  (`—` when none), the state and `Components::summary()`, read from each
  install through `PluginStore::detail` (the store's own reading of the
  copy, so the counts are what loads), wrapped between its ` · ` parts
  under its own column at the width. `--available` fetches the
  never-fetched marketplaces first, then prints a block per marketplace
  (`{name} — {source} ({n} plugins)`), installed plugins first, `⊘ … (can't
  be installed here)` for a source this build does not fetch.
- **`info`** — the id and its state (`✔ installed · enabled · v1.0.0`), the
  description, then `Author`, `Category`, `License`, `Source`, `Homepage`
  (the plugin's, else the browsable source), `Blocked`, and the components
  by name (`Skills`, `Commands`, `Agents`, `Hooks`, `MCP`, `Not used`) —
  or `Contains   shown after install — its files are in another
  repository`. It closes on the command to run next (install, or update
  when one is available); warnings follow on stderr.
- **`install`** — a folder through `install_directory` (as `name@local`),
  anything else through `install`. Installing what is installed reinstalls
  it — how a broken copy is repaired — and says `Reinstalled`. Then the
  store's two sentences: what is live from the next message
  (`InstallOutcome::live_summary`) and what waits for the startup review
  (`review_summary`), plus `Not used here: 1 app.` when the plugin carries
  something this app does not load.
- **`uninstall`**, **`enable`**, **`disable`** — one line each;
  `Already enabled: x@y` (exit 0) when there is nothing to do.
- **`update [plugin]`** — with a plugin, its marketplace first
  (`update_marketplace`; `@local` copies its folder again), then
  `update_plugin`: `Updated x@m to v1.1.0`, `x@m is up to date (v1.0.0)`.
  Without, the marketplaces the installed plugins came from, then every
  installed plugin, a line each, then `3 plugins checked, 1 updated`; one
  failure is said and the rest carry on, exit 1.
- **`marketplace add`** — `MarketplaceSource::parse`, `--ref` applied (an
  error for a folder, and for a source already naming a different ref),
  then `add_marketplace`: `Added marketplace team (4 plugins) from
  ~/work/team-plugins` over `Install one with: alter-zero plugin install
  <plugin>@team`; the same source again is `Marketplace team is already
  added (4 plugins)`, exit 0.
- **`marketplace list`** — one row per known marketplace: its name (`—`
  for a folder named only once read), the source, the plugin count right
  aligned (or `not fetched yet`), `updated 2h ago` or `local folder`, and
  `default`.
- **`marketplace update [name]`** — `Updated …`, `… is up to date`, or
  `Fetched …` for a default never fetched. **`marketplace remove`** —
  `Removed marketplace team`, then which installed plugins came from it and
  stay. An unknown marketplace name lists the ones there are.
- **`--json`** — one compact document and a newline, built from typed
  borrowing structs, never a `Value` tree: `list` →
  `{"plugins":[{id,name,marketplace,version,enabled,description}]}`;
  `list --available` → `{"marketplaces":[{name,source,plugins:[{name,
  description,category,version,status}]}]}`, `status` one of `available`,
  `enabled`, `disabled`, `unsupported`; `info` → the `PluginDetail` in
  camelCase (`id`, `displayName`, …, `updateAvailable`, `blocked`,
  `components{skills,commands,agents,hooks,mcpServers,unsupported}` or
  `null`, `warnings`); `marketplace list` →
  `{"marketplaces":[{name,source,default,fetched,plugins,updated,revision}]}`
  (`name` null while unfetched and unnamed, `updated` unix seconds).
- Every command a message suggests names the binary as it was invoked
  (`cli::bin_name`), the store's own messages included.

Exit codes: **0** success (an already-done enable, disable or add
included), **1** a failure (one `error:` line per failure), **2** a grammar
error.

## Environment

- `ALTER_ZERO_CONFIG_DIR` moves the config home, and with it `plugins.json`
  and `plugins/`. No config home at all (no `HOME`, no override) is exit 1:
  `error: no config directory — set HOME or ALTER_ZERO_CONFIG_DIR`.
- `ALTER_ZERO_PLUGIN_MARKETPLACES` replaces the two default marketplaces
  (`plugins::defaults_from_env`): `none` leaves none, a list of sources —
  folders included — stands in for them. A folder given there has no name
  until it is read, so `marketplace list` shows `—` for it until then, and
  an id naming a marketplace nobody knows yet reads those folders first.
- `COLUMNS` sets the width a listing is cut to.

## Known divergences

- **Bare `alter-zero plugin` is a usage error, exit 2**, as bare
  `alter-zero mcp` is — not a help page.
- **No `--scope`, no `--marketplace`, no `--sparse`.** Plugins are the
  user's (one `plugins.json`); `name@marketplace` is the one way to name a
  marketplace; a marketplace is always cloned whole (depth 1).
- **`update` without a plugin updates only the marketplaces something was
  installed from**: one nothing came from has nothing this command would
  update. `marketplace update` refreshes them all.
- **A folder marketplace has no revision**, so `update` copies its in-repo
  plugins again every time and counts them updated (`Updated x@m (v1.0.0)`
  — the version did not move).
- **`--available` works without `--json`** (Codex requires both).

## Testing

- `cli`: a test per row of the grammar and per error — each command and
  alias, the flags and where they do not apply, `--ref`'s value forms and a
  second `--ref`, a missing, extra or unknown argument, help anywhere, bare
  `plugin` and `plugin marketplace`, `plugin` as a first argument only, and
  the page under 80 columns.
- `tui::plugin_cli` (`cargo test --bin alter-zero plugin_cli`): the pure
  text — table alignment in display columns, cutting and wrapping, the
  ambiguity list, the update and install sentences, each command's text
  plain and coloured, and every JSON document's keys.
- `tests/plugin_cli.rs`: the real binary with its own config home and
  `HOME`, `ALTER_ZERO_PLUGIN_MARKETPLACES=none` (or a fixture repository as
  the default), against marketplaces built in temp dirs
  (`tests/support/plugin_fixtures.rs`): the whole lifecycle over a git
  marketplace added by `file://` URL (add, `list --available`, `info`,
  `install`, `list`, `disable`, `enable`, `update` after a new commit,
  `uninstall`, `marketplace remove`); a Claude-format folder marketplace;
  a Codex-format one as a folder and as a repository; a plugin pinned by
  `sha` in a second repository; a renamed plugin and an npm one; the
  ambiguity error; the directory install and its reinstall; a default
  fetched on first use; every `--json` shape parsed; the exit codes; and an
  unparseable `plugins.json` refused and left byte for byte.
- `tests/cli_help.rs`: the main page lists the family, `plugin --help` is
  the plain plugin page, and a grammar error carries its usage block.
