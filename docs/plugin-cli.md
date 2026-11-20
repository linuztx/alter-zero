# The `plugin` command family — `alter-zero plugin …`

Plugins and the marketplaces they come from (`docs/plugins.md`) are managed
inside the app on the `/plugins` page; this is the same surface for a
script, a dotfiles installer or a CI image, resolved and answered **before
the TUI boots** like `mcp` (`docs/mcp-cli.md`) and `--continue`/`--resume`
(`docs/cli.md`).

```
alter-zero plugin install <plugin>[@<marketplace>]          (alias: add)
alter-zero plugin uninstall <plugin>[@<marketplace>]        (alias: remove)
alter-zero plugin enable <plugin>[@<marketplace>]
alter-zero plugin disable <plugin>[@<marketplace>]
alter-zero plugin update [<plugin>[@<marketplace>]]         (alias: upgrade)
alter-zero plugin list [--available] [--json]
alter-zero plugin info <plugin>[@<marketplace>] [--json]
alter-zero plugin marketplace add <source> [--ref <ref>]
alter-zero plugin marketplace add --defaults
alter-zero plugin marketplace list [--json]
alter-zero plugin marketplace update [<name>]               (alias: upgrade)
alter-zero plugin marketplace remove <name>                 (alias: rm)
```

`plugins` is accepted wherever `plugin` is.

## What the references do (findings)

- **Claude Code** — `claude plugin install|uninstall|enable|disable|update
  <plugin>[@<marketplace>]`, `claude plugin marketplace
  add|list|update|remove`, with `plugins` as an alias of the family and
  `add`/`remove` beside `install`/`uninstall`. Enable and disable take a
  scope (user, project, local).
- **Codex** — `codex plugin add|remove <PLUGIN[@MARKETPLACE]>` (or
  `--marketplace`), `codex plugin list [--json] [--available]` (the latter
  requiring `--json`), `codex plugin marketplace add <SOURCE> [--ref]
  [--sparse]`, `list`, `upgrade`, `remove`; `--json` everywhere with stable
  camelCase shapes and `after_help` examples on every command. No
  `enable`, `disable`, `info` or plugin `update`: toggling and details are
  TUI-only. Errors are anyhow's `Error:` + `Caused by:` chain.

## What we build

The union of both verb sets — **both references' spellings work**:
`add` is `install`, `remove` is `uninstall`, `upgrade` is `update` (for a
plugin and for a marketplace), `marketplace rm` is `marketplace remove` —
plus what neither has: `info`, `marketplace add --defaults` (the two
default marketplaces of `docs/plugins.md` *Marketplaces*, offered and not
added until asked), and `--json` on `list`, `info` and `marketplace list`.
A plugin is installed **for the user** and on in every directory until one
turns it off: `enable` and `disable` act on **the directory the command
runs in**, as the `/plugins` page's toggle does, and the help page says so.
There is no scope flag (`docs/plugins.md` *Known divergences*).

## The grammar (pure, `src/cli.rs`)

`plugin` (or `plugins`) routes as the **first argument only**, exactly as
`mcp` does — anywhere else it is a word of the prompt, so `alter-zero -c
plugin` still sends `plugin` into the continued session. A bare
`alter-zero plugin` used to start a session with the prompt `plugin`; it is
a usage error now, the trade `mcp` made.

```rust
pub enum PluginCli {
    Install { plugin: String },
    Uninstall { plugin: String },
    Enable { plugin: String },
    Disable { plugin: String },
    Update { plugin: Option<String> },   // None: every installed plugin
    List { available: bool, json: bool },
    Info { plugin: String, json: bool },
    Marketplace(MarketplaceCli),
    Help,
}

pub enum MarketplaceCli {
    Add { source: String, git_ref: Option<String> },
    AddDefaults,
    List { json: bool },
    Update { name: Option<String> },     // None: every marketplace
    Remove { name: String },
    Help,
}
```

A plugin, a source and a marketplace name stay **strings, as typed**: what
`owner/repo`, `git@host:…#ref` or `./dir` is, and which marketplace a bare
`<plugin>` is in, are the plugin library's to resolve (`plugins::source`,
the catalog lookup). The grammar refuses only what can never be right:

- an empty value — no plugin, `""`, no source, a blank marketplace name;
- a plugin with whitespace in it, or with nothing on one side of its `@`
  (`@m`, `p@`);
- a second positional (`unrecognized argument: b`);
- `--defaults` beside a source or `--ref`;
- `--available` anywhere but `plugin list` (`--available only applies to
  plugin list`), and any other flag a subcommand does not take
  (`unrecognized argument: --json`).

Whether `--ref` suits the source (a local folder has none) is the
library's to judge, not the grammar's.

Every subcommand reads its arguments through one shared reader,
`subcommand_arguments(args, &Flags)` — also what `mcp`'s plain
subcommands (`remove`, `get`, `list`, `add-json`) read through
`plain_arguments` now: a positional anywhere among the flags,
`--flag=value` or `--flag value` (`flag_value`), each flag at most once
(`set_once`: `--json was given more than once`), a switch refusing a value
(`--json takes no value`), `-h`/`--help` answering that level's page, and
`--` ending the flags so a dash-leading value — or a literal `--help` — is
reachable. Each subcommand's `Flags` names its switches, its value flags and
the flags it knows belong elsewhere.

Errors are the family's wording — lowercase, no trailing period, naming
the culprit and what to use:

```
plugin needs a subcommand: install, uninstall, enable, disable, update, list, info, marketplace
unrecognized plugin command: frob (use install, uninstall, enable, disable, update, list, info or marketplace)
plugin install needs a plugin name (use <plugin>[@<marketplace>])
invalid plugin "two words": a plugin name has no spaces (use <plugin>[@<marketplace>])
invalid plugin "@m": nothing before or after the @ (use <plugin>[@<marketplace>])
plugin marketplace needs a subcommand: add, list, update, remove
unrecognized plugin marketplace command: frob (use add, list, update or remove)
plugin marketplace add needs a source (owner/repo, a git URL or a local folder) or --defaults
--defaults takes no source or --ref
plugin marketplace remove needs a marketplace name
```

### The help pages

`HelpPage::Plugin` (`plugin --help`, and `--help` after any plugin
subcommand) and `HelpPage::PluginMarketplace` (`plugin marketplace
--help`, and after any of its subcommands), rendered by the one
`cli::help` like the main and MCP pages (`docs/cli.md`): titled by the
command, a description, the `Usage:` forms, `Commands:` and `Options:`
rows, and — new to the renderer, `HelpDoc::examples` — an `Examples:`
section of invocations to copy, dressed like the usage forms. The main
page gains a `alter-zero plugin <COMMAND>` usage form and a `plugin`
command row; its copies in `docs/cli.md`, `README.md` and the
`MAIN_PAGE` test move with it.

```
alter-zero plugin

Install and manage plugins — Agent Plugins, Claude Code and Codex plugins —
from the marketplaces you add. A plugin is installed for you and on in every
directory; enable and disable act on the directory they run in. A bare
<plugin> is enough while one marketplace lists it. Inside the app, /plugins
does the same.

Usage: alter-zero plugin install <plugin>[@<marketplace>]
       alter-zero plugin uninstall <plugin>[@<marketplace>]
       alter-zero plugin enable <plugin>[@<marketplace>]
       alter-zero plugin disable <plugin>[@<marketplace>]
       alter-zero plugin update [<plugin>[@<marketplace>]]
       alter-zero plugin list [--available] [--json]
       alter-zero plugin info <plugin>[@<marketplace>] [--json]
       alter-zero plugin marketplace <COMMAND>
…
```

### Which page a grammar error shows

`cli::usage_page(args) -> HelpPage` reads the arguments the way `parse`
routes them: `mcp …` the MCP page, `plugin marketplace …` (or `plugins
marketplace …`) the marketplace page, any other `plugin …` the plugin page,
anything else the main page. It replaced the boundary's inline peek at
`argv[1]`, which could not see a second word.

## Where the executor lives — `crates/plugins/src/cli.rs`

The commands are **in the plugin crate**, not the app: `plugins::cli` has
one plain function per command — `install`, `uninstall`, `set_enabled`,
`update`, `list`, `info`, `marketplace_add`, `marketplace_add_defaults`,
`marketplace_list`, `marketplace_update`, `marketplace_remove` — each
taking the `PluginStore`, the parsed arguments as plain values (strings,
bools, the directory for per-directory on/off, the clock for ages) and an
`Output { out, err, progress, bin }` of two `&mut dyn Write`, doing the
work, printing, and returning a `CommandOutcome { code, forget_trust,
review }`. Every word, table and `--json` shape is there, tested there with
in-memory writers over stores built on temp dirs and local `git`
repositories (`cargo test -p alter-zero-plugins cli::`).

Why there: code inside the main crate is resident in an idle session
whether it ever runs or not, while a cold dependency crate's is not
(`docs/plugins.md` *Memory* — rustls, 362 kB, measures 0% resident). A
`plugin` command runs in a process that exits a moment later; the session
that never runs one should not carry its wording.

## The boundary (`src/tui/plugin_cli.rs`)

`resolve_cli` matches `Cli::Plugin(cmd)` and hands it to
`tui::plugin_cli::run(&cmd) -> i32`, exiting through the same `Err(code)`
channel `--help` uses — cooked-mode stdio, no tokio, no terminal. The file
is a router:

- **`plugin_store(cwd) -> Option<PluginStore>`** (`pub(crate)`, the one
  construction the `/plugins` page reuses): `PluginsLayout::resolve` over
  the config home and `ALTER_ZERO_PLUGINS_DIR`; a `GitRunner` whose
  `CommandBuilder` is `subprocess::detached_argv_command` — the tty-detach
  chain's argv form (`setsid git …`, else the helper re-exec with every
  word quoted, else a process group of its own), so a credential prompt
  never reaches the terminal and a timeout kills the whole group —
  bounded by `ALTER_ZERO_PLUGIN_GIT_TIMEOUT_SECS`; the offered
  marketplaces (`ALTER_ZERO_PLUGIN_MARKETPLACES`); sources resolved
  against the working directory and `HOME`.
- **`run`**: the help pages; then the store, real stdout and stderr
  (progress only when stderr is a terminal) and one `plugins::cli` call per
  grammar arm; then, for every directory in `forget_trust`,
  `trust::forget_plugin_trust(config::trust_json_path(), dir)` — an
  install, an update or an uninstall is always reviewed again, so the
  records keyed under its files go (a failure to forget is a failure: exit
  1).

The **exit-code contract**, the `mcp` family's:

| Code | When | Where |
| --- | --- | --- |
| `0` | success, and every help page | results and help on stdout |
| `1` | a failure: a name nothing lists, a failed fetch, a file that will not parse, a failed write | one line on stderr, `{bin} plugin: {sentence}` |
| `2` | a grammar error | `error: …` over the page's `Usage:` block on stderr (`resolve_cli`, never `run`) |

Every failure sentence is the store's `PluginError` as it reads
(lowercase, no closing period, naming the culprit and what to do).
`marketplace add --defaults` and `marketplace update` with no name go on
past one that fails — reported, exit 1 at the end.

### Output wording

A real run against GitHub, the config home a throwaway directory:

```
$ alter-zero plugin marketplace list
claude-plugins-official  anthropics/claude-plugins-official  offered, not added — alter-zero plugin marketplace add --defaults
openai-curated           openai/plugins                      offered, not added — alter-zero plugin marketplace add --defaults

$ alter-zero plugin marketplace add --defaults
Added marketplace claude-plugins-official (317 plugins)
Added marketplace openai-curated (65 plugins)

$ alter-zero plugin marketplace list
claude-plugins-official  anthropics/claude-plugins-official  317 plugins · updated just now
openai-curated           openai/plugins                      65 plugins · updated just now

$ alter-zero plugin list --available | head -4
claude-plugins-official — 317 plugins
  42crunch-api-security-testing                 available    Automate API security directly in Claude Code with 42Crunch…
  adobe-for-creativity                          available    Harness Adobe's creative AI-powered tools to edit images, a…
  agent-sdk-dev                                 available    Development kit for working with the Claude Agent SDK

$ alter-zero plugin install frontend-design@claude-plugins-official
Installed frontend-design@claude-plugins-official 1.1.1
  1 skill

$ alter-zero plugin install consensus@openai-curated
Installed consensus@openai-curated 4.0.0
  1 MCP server
  Its hooks and MCP servers wait for your review: the app asks before they run, when it next starts.

$ alter-zero plugin info frontend-design
frontend-design@claude-plugins-official · enabled in this directory
  Frontend design skill for UI/UX implementation

  Version      1.1.1
  Marketplace  claude-plugins-official
  Source       ./plugins/frontend-design
  Format       Claude Code
  Directory    ~/.alter-zero/plugins/installed/claude-plugins-official/frontend-design
  Skills       frontend-design:frontend-design
  Commands     none
  Agents       none
  Hooks        none
  MCP servers  none

$ alter-zero plugin list
consensus@openai-curated                 4.0.0  enabled  1 MCP server
frontend-design@claude-plugins-official  1.1.1  enabled  1 skill

$ alter-zero plugin update
consensus@openai-curated                 up to date
frontend-design@claude-plugins-official  up to date

$ alter-zero plugin uninstall frontend-design
Uninstalled frontend-design@claude-plugins-official
```

(The `Directory` row prints the absolute path; it is shortened here.)

The rest, from the tests over a folder marketplace `demo`:

```
$ alter-zero plugin install hello
Installed hello@demo 1.0.0
  1 skill · 1 command
  Loaded in place from /home/me/market/plugins/hello

$ alter-zero plugin install hello@first          (an offered marketplace not yet added)
Added marketplace first on the way
Installed hello@first 1.0.0
  1 skill · 1 command

$ alter-zero plugin install hello                (two marketplaces list it)
alter-zero plugin: hello is listed by more than one marketplace (hello@demo, hello@other); name one as hello@<marketplace>

$ alter-zero plugin disable hello
Disabled hello@demo in /home/me/work (it stays installed)

$ alter-zero plugin enable hello
Enabled hello@demo in /home/me/work

$ alter-zero plugin update hello
hello@gitdemo  1.0.0 → 1.1.0
Its hooks and MCP servers wait for your review: the app asks before they run, when it next starts.

$ alter-zero plugin list --available
demo — 3 plugins
  hello     1.0.0  enabled      Greets people
  guard            available    Guards stops
  npmthing         unavailable  npm sources are not supported

$ alter-zero plugin marketplace update
Read demo again (3 plugins)                      (a folder)
gitdemo is up to date (3 plugins)                (a git one, nothing new)
Updated gitdemo (3 plugins) — 1 plugin to update: hello@gitdemo (alter-zero plugin update)

$ alter-zero plugin marketplace remove demo
Removed marketplace demo and uninstalled 2 plugins: guard@demo, hello@demo

$ alter-zero plugin list                         (nothing installed)
No plugins installed — see what is offered: alter-zero plugin list --available
```

The state words are the `/plugins` page's: `enabled` and `disabled` (in
this directory), `not loaded` (installed, but its manifest refuses it —
the reason follows), `available`, `unavailable` (the reason follows). A
column is padded to its widest cell up to 40 characters; a longer one (the
official catalog has a 60-character name) runs past it. A description is
cut to 60 characters in `list --available` and printed whole, every line
indented, in `info`. Progress (`Cloning anthropics/claude-plugins-official…`,
`Fetching …`, `Copying…`) goes to stderr, and only on a terminal.

### `--json`

Pretty-printed, one document per command; every field is always present
(`null` or empty when there is nothing to say), so a script can index
without checking.

`plugin list [--available] --json`:

```json
{
  "plugins": [
    {
      "id": "hello@demo",
      "name": "hello",
      "marketplace": "demo",
      "state": "enabled",
      "version": "1.0.0",
      "description": "Greets people",
      "summary": "1 skill · 1 command",
      "reason": null
    }
  ],
  "errors": [ { "marketplace": "broken", "error": "the catalog of broken could not be read: …" } ]
}
```

`state` is one of the five words above; `summary` is set for an installed
plugin that loads, `reason` for `not loaded` and `unavailable`; `errors`
lists the catalogs `--available` could not read (always empty without it).

`plugin info --json`:

```json
{
  "id": "guard@demo",
  "name": "guard",
  "marketplace": "demo",
  "state": "enabled",
  "version": null,
  "display_name": null,
  "description": "Guards stops",
  "source": "./plugins/guard",
  "format": "no manifest",
  "directory": "/home/me/market/plugins/guard",
  "components": {
    "skills": [],
    "commands": [],
    "agents": [],
    "hooks": { "Stop": 1 },
    "hook_files": 0,
    "mcp_servers": []
  },
  "not_used": [],
  "notices": [],
  "reason": null
}
```

`components` is `null` when what a plugin adds is known only once it is
installed (a plugin from a repository of its own); for a plugin not
installed it is read from the marketplace's tree, where `hook_files` counts
hooks files whose handlers are not counted yet and `mcp_servers` names the
files.

`plugin marketplace list --json`:

```json
{
  "marketplaces": [
    {
      "name": "claude-plugins-official",
      "source": "anthropics/claude-plugins-official",
      "added": true,
      "default": true,
      "local": false,
      "plugins": 317,
      "installed": 1,
      "updated": 1795180000,
      "revision": "1a2b3c4d…",
      "error": null
    },
    {
      "name": "openai-curated",
      "source": "openai/plugins",
      "added": false,
      "default": true,
      "local": false,
      "plugins": null,
      "installed": 0,
      "updated": null,
      "revision": null,
      "error": null
    }
  ]
}
```

## Known divergences

- **No scope flag.** Claude Code's `--scope user|project|local` is not
  taken: a plugin is installed for the user, and `enable`/`disable` act on
  the directory the command runs in.
- **No `--marketplace`/`-m`** (Codex): the marketplace rides the plugin as
  `@<marketplace>`.
- **No `--sparse`** on `marketplace add` (Codex).
- **`--available` does not need `--json`** (Codex requires it).
- **`info`** and **`marketplace add --defaults`** are ours.
- **Removing a marketplace uninstalls its plugins** and names them
  (`docs/plugins.md` *Installing*).
- **No cancellation from the shell**: Ctrl+C ends the command, and a `git`
  child it started (in a session of its own, so the terminal's SIGINT does
  not reach it) runs on until it finishes or its timeout, and nothing is
  recorded for it.

## Testing

- `cli::tests` (`cargo test --lib cli::`): the grammar — every subcommand
  and alias, the `plugins` spelling and first-argument-only routing, the
  spec checks, `update` with and without a plugin, `list`/`info` switches
  (twice, with a value), `--available` elsewhere, `--` ending the flags,
  help at every level, the bare and unknown subcommand messages at both
  levels, `marketplace add` with a source, `--ref` and `--defaults`;
  `usage_page` for each family; both plugin pages' titles, forms and
  examples, each under 80 columns and stripping back to plain.
- `plugins::cli::tests` (`cargo test -p alter-zero-plugins cli::`): the
  wording of every command over a folder marketplace and `file://` git
  ones — install by name and by id, the ambiguity, the review notice,
  reinstall, an unavailable entry, enable/disable in one directory,
  `list`/`--available`/`info` and their `--json`, uninstall, marketplace
  remove taking its plugins, update with and without a change, an offered
  marketplace added on the way, progress only on a terminal, a broken
  `plugins.json`. Tests that need `git` return early without it.
- `tests/plugin_cli.rs` (the real binary, a temp `ALTER_ZERO_CONFIG_DIR`
  and `HOME`, `ALTER_ZERO_PLUGIN_MARKETPLACES` pointing the defaults at
  local repositories): every verb end to end offline, exit codes, one-line
  failures, a disable in one directory and the plugin still on in another,
  an update forgetting the `trust.json` record under the updated copy, the
  defaults added on demand and on the way, a git marketplace's clone and
  plugins gone with it, and a broken `plugins.json` never written over.
- `tests/cli_help.rs`: piped `plugin --help` and `plugin marketplace
  --help` are their plain pages byte for byte; a plugin grammar error exits
  2 over its page's usage block; a parsed command reaches the executor.
