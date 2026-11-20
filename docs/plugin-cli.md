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

## The boundary (`src/tui/plugin_cli.rs`)

`resolve_cli` matches `Cli::Plugin(cmd)` and hands it to
`tui::plugin_cli::run(&cmd) -> i32`, exiting through the same `Err(code)`
channel `--help` uses — cooked-mode stdio, no tokio, no terminal.

The **exit-code contract**, the `mcp` family's:

| Code | When | Where |
| --- | --- | --- |
| `0` | success, and every help page | results and help on stdout |
| `1` | a failure: a name nothing lists, a failed fetch, a file that will not parse, a failed write | one plain line on stderr |
| `2` | a grammar error | `error: …` over the page's `Usage:` block on stderr (`resolve_cli`, never `run`) |

**Today** `run` answers the two help pages (stdout, exit 0) and, for every
other command, prints `{bin} plugin: not available yet` on stderr and exits
1 — one match arm per command, so the executor over the plugin library
(`src/plugins/`) replaces them arm by arm.

### Output wording

> **To be written with the executor** (the second step): what each command
> prints on success, the `--json` shapes of `list`, `info` and
> `marketplace list`, the failure lines and their `{bin} …` hints, and what
> an install that brought hooks or MCP servers says about the review at the
> next launch.

## Known divergences

- **No scope flag.** Claude Code's `--scope user|project|local` is not
  taken: a plugin is installed for the user, and `enable`/`disable` act on
  the directory the command runs in.
- **No `--marketplace`/`-m`** (Codex): the marketplace rides the plugin as
  `@<marketplace>`.
- **No `--sparse`** on `marketplace add` (Codex).
- **`--available` does not need `--json`** (Codex requires it).
- **`info`** and **`marketplace add --defaults`** are ours.

## Testing

- `cli::tests` (`cargo test --lib cli::`): every subcommand and alias, the
  `plugins` spelling and first-argument-only routing, the spec checks,
  `update` with and without a plugin, `list`/`info` switches (twice, with a
  value), `--available` elsewhere, `--` ending the flags, help at every
  level, the bare and unknown subcommand messages at both levels,
  `marketplace add` with a source, `--ref` (separate and attached,
  missing, twice) and `--defaults` (alone, with a source, with `--ref`,
  twice); `usage_page` for each family; both plugin pages' titles, forms
  and examples, each under 80 columns and stripping back to plain.
- `tests/cli_help.rs` (the real binary): piped `plugin --help` and `plugin
  marketplace --help` are their plain pages byte for byte; a plugin grammar
  error exits 2 over its page's usage block.
- The executor's own tests (`tests/plugin_cli.rs`) arrive with it.
