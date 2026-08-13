# CLI MCP install — `alter-zero mcp add/remove/list/get/add-json`

Claude Code and codex both let a script install an MCP server without opening
the app; this is that surface for the user config file (`docs/mcp.md`),
resolved and answered **before the TUI boots** like `--continue`/`--resume`
(`docs/cli.md`):

```
$ alter-zero mcp add vercel --url https://mcp.vercel.com
Added http MCP server "vercel" (https://mcp.vercel.com)
File: /home/user/.alter-zero/mcp.json

$ alter-zero mcp add everything -e API_KEY=xxx -- npx -y @modelcontextprotocol/server-everything
Added stdio MCP server "everything" (npx -y @modelcontextprotocol/server-everything)
File: /home/user/.alter-zero/mcp.json

$ alter-zero mcp list
MCP servers in /home/user/.alter-zero/mcp.json:
  vercel: https://mcp.vercel.com (http)
  everything: npx -y @modelcontextprotocol/server-everything (stdio)
```

## What the references do (findings)

Both reference implementations were read end to end; the grammars differ in
one decision that matters.

- **codex** — `codex mcp add <name> (--url <URL> | -- <command>...)`: the
  transport is *which flag you passed*, enforced by a required, mutually
  exclusive arg group. `--env KEY=VALUE` is repeatable (stdio only —
  though only a runtime check catches `--url … --env`), the trailing
  command is captured raw (so `-- npx -y pkg --port 4000` keeps the child's
  own flags), and `--` is conventional rather than required. No `--header`
  flag, no SSE transport at all. A duplicate name is **silently
  overwritten**; `remove` of a missing name is a *success*. Writes
  `~/.codex/config.toml` through `toml_edit` (comments survive).
- **Claude Code** — `claude mcp add [-t stdio|sse|http] <name>
  <commandOrUrl> [args...]`: one positional serves as both command and URL,
  and the transport **defaults to stdio, never inferred** — so
  `claude mcp add name https://x` "succeeds" by writing a stdio server
  whose *command is the URL* (three warning lines on stderr, config broken
  anyway). Its own help example is broken too: the variadic `-e` swallows
  the server name (`mcp add -e K=V my-server -- npx …` parses `npx` as the
  name). On the good side: `-H/--header "Name: Value"` for remote servers,
  a hard **duplicate-name error** (never an overwrite), `add-json <name>
  <json>` for pasting a README's exact entry, and name validation
  (`[A-Za-z0-9_-]`, same class codex enforces).

## What we build

codex's explicit skeleton (the transport is which form you used — a URL
flag or a trailing command — so nothing can be silently misread), with the
Claude Code conveniences that fit our config format: `--transport` to name
`sse` (codex can't say it, our `McpServerConfig` can), `--header` for
remote servers, `add-json` for README snippets, and the duplicate-name
error. All of it targets the **user** file only
(`ALTER_ZERO_MCP_FILE`, else `{config_home}/mcp.json` — the same
resolution the TUI uses); the project files are shared with a team and sit
behind the `/trust` gate, so a CLI that wrote them would either bypass the
gate or immediately re-pend it (`docs/project-config.md`).

### The grammar (pure, `src/cli.rs`)

```
alter-zero mcp add <name> --url <URL> [--transport http|sse] [-H|--header "Name: Value"]...
alter-zero mcp add <name> [--transport stdio] [-e|--env KEY=VALUE]... [--] <command> [args...]
alter-zero mcp add-json <name> <json>
alter-zero mcp remove <name>
alter-zero mcp get <name>
alter-zero mcp list
```

- `mcp` is recognised as the **first** argument only; `parse` peels it off
  and hands the rest to `parse_mcp`, so the session-flag grammar is
  untouched. `Cli` gains one variant, `Mcp(McpCli)`.
- **Transport comes from the form.** `--url` ⇒ remote; a trailing command
  ⇒ stdio. `--transport` may then *refine* the remote kind (`http` pins
  streamable HTTP, `sse` the legacy transport); given without a matching
  form it is a usage error (`--transport http` with a command,
  `--transport stdio` with `--url`). **Bare `--url` (no `--transport`)
  maps to the type-less `{"url": …}` entry** — `Http { sse_fallback:
  true }`, the spec's try-http-then-sse compat recipe — so the default add
  works against either generation of server. That fallback shape is
  exactly why the writer must not emit a `"type"` key for it
  (`docs/mcp.md`).
- **The command is captured raw from its first token.** After `<name>`,
  flags parse until the first non-flag argument or `--`; from there
  everything belongs to the command verbatim, so
  `mcp add docs npx -y pkg --port 4000` keeps `-y`/`--port` as child args
  with no separator needed (codex's `trailing_var_arg`; Claude Code
  errors with `unknown option '-y'` there). A command whose first word
  itself starts with `-` needs the `--`.
- `-e/--env KEY=VALUE` splits on the **first** `=` (`A=b=c` keeps `b=c`),
  rejects a missing `=` or empty key, keeps an empty value, and applies to
  stdio only (`--env` with `--url` is a usage error, not codex's late
  runtime one). `-H/--header "Name: Value"` splits on the first `:`,
  trims both sides, rejects a missing `:` or empty name, and applies to
  remote only. Both are one-value-per-flag (never variadic — the trap
  that broke Claude Code's own help example). Repeated keys: last wins.
- The **name** must be non-empty `[A-Za-z0-9_-]+` (both references'
  rule, `mcp::validate_server_name`). That is exactly the class
  `mcp::normalize_name` keeps, so the `/mcp` list, the permission rules
  and the `mcp__{server}__{tool}` wire name all agree with what was typed.
- `add-json <name> <json>` feeds the string to the config parser
  (`mcp::parse_server_entry` — the same `parse_server` every file read
  uses), so a README's `{"type":"http","url":…}` snippet round-trips
  through the one schema; there is no second parser to drift.
- `-h/--help` anywhere in the `mcp` arguments prints `MCP_USAGE` (exit 0)
  — except after `--`, where tokens are the command's own. Bare
  `alter-zero mcp` is a usage error naming the subcommands.

### The writers (pure, `src/mcp/config.rs`)

`record_server` / `remove_server` are `record_disabled`'s siblings — a
string-in/string-out read-modify-write over the whole document — with two
deliberate differences:

- **A file that doesn't parse is refused, never clobbered.**
  `record_disabled`'s silent `{}` fallback is the in-TUI never-kill-the-TUI
  posture; for a CLI edit it would discard every declared server. Both
  writers return `Err(McpWriteError::InvalidJson(reason))` instead (the
  boundary exits 1: `{path} is not valid JSON … refusing to rewrite it`).
  Empty/whitespace-only contents are a fresh `{}` (a `touch`ed file, or
  the boundary's missing-file read), not an error.
- **`remove_server` uses `Map::shift_remove`.** serde_json is built with
  `preserve_order`, where plain `remove` is a `swap_remove` that moves the
  *last* server into the removed slot — removing the first of five would
  silently reorder the file (and the `/mcp` list that renders it).

`record_server` errors `Exists` on a duplicate key (checked on the raw
JSON, so even an entry that doesn't parse can't be silently replaced);
`remove_server` errors `Missing` when the name isn't there. Everything
else — the `projects` disabled sets, unknown keys, the other servers'
entries — passes through untouched, and an entry serializes exactly as
`parse_server` reads it: `{"type":"stdio","command",…}` /
`{"type":"http"|"sse","url",…}`, empty `args`/`env`/`headers` omitted, and
the `sse_fallback` Http shape as a bare `{"url": …}` with **no** `type`
key. `parse_server_entry(render_server(config))` round-trips every shape.

### The boundary (`src/tui/mcp_cli.rs`)

`resolve_cli` matches `Cli::Mcp(cmd)` and hands it to
`tui::mcp_cli::run(cmd) -> i32`, exiting with the returned code through
the same `Err(code)` channel `--help` uses — cooked-mode stdio, no tokio,
no terminal, before `InlineViewport::init` (`docs/cli.md`'s ordering).
The file is `config::mcp_user_file_path()` (`ALTER_ZERO_MCP_FILE`, else
`{config_home}/mcp.json`); `None` — no `HOME`, no override — is a hard
exit 1, unlike the TUI's best-effort writers, because there is nowhere to
persist. A missing file reads as `{}`; the write `create_dir_all`s the
parent and surfaces its error. `ALTER_ZERO_MCP=0` does not gate the CLI —
it edits a file, launches nothing; the feature switch keeps governing the
TUI.

- `add`/`add-json`: RMW through `record_server`, then two stdout lines —
  `Added {stdio|http|sse} MCP server "{name}" ({target})` over
  `File: {path}`.
- `remove`: RMW through `remove_server` — `Removed MCP server "{name}"` +
  the `File:` line.
- `list`: `parse_mcp_file` over the user file, one `  {name}: {target}
  ({transport})` row per entry **in file order** under a header naming the
  path; an empty (or absent) file prints `No MCP servers in {path}` and
  the `Add one with: {bin} mcp add <name> --url <url>` hint; entries that
  don't parse are stderr `warning:` lines, and the good ones still list.
- `get`: the entry's facts (`Type:`, `Command:`/`URL:`, `Args:`, `Env:`/
  `Headers:` with **values masked** `*****` — headers routinely carry
  bearer tokens, and codex masks for the same reason — then `File:`),
  closed by the `Remove with: {bin} mcp remove {name}` hint line.

Exit codes follow the house contract (`docs/cli.md`): **2** for grammar
errors (`{message}` + `MCP_USAGE` on stderr — unknown flag, missing name,
bad `--env`/`--header`/JSON, conflicting forms), **1** for resolution
failures (duplicate name, unknown name, unparseable file, no config home,
a failed write — one plain stderr line), **0** for success and help.

## Known divergences from the references

- **No `--scope`**: project files (`.mcp.json`, `.alter-zero/mcp.json`)
  are read-only here — they're shared with a team and trust-gated, so
  installing into them belongs to an edit + `/trust` review, not a CLI
  write. Claude Code's `local` scope doesn't exist in our two-scope model.
- **`list`/`get` read the config, they don't connect.** Claude Code's
  `mcp list` spawns every stdio server for a health check (its own help
  warns "only use this command in directories you trust"); codex probes
  OAuth metadata over the network. Ours is deterministic and side-effect
  free — connection status lives in the TUI's `/mcp` manager, which
  already shows `✔ connected · 3 tools` live. For the same reason `list`
  shows the file the CLI manages (the user scope), not the trust-gated
  project merge.
- **A duplicate name is an error** (Claude Code's rule): codex's silent
  overwrite loses a working entry to a typo. The message names the
  `mcp remove` escape hatch.
- **`remove` of a missing name fails** (exit 1, Claude Code again):
  codex's "success" would let a scripted cleanup misspell a name forever.
- **OAuth flags are out**: `--client-id`/`--callback-port` configure a
  flow our `/mcp` Authenticate page negotiates by RFC discovery instead
  (`docs/mcp.md`); nothing to persist means nothing to flag.

## Testing

- `cli`: the whole `mcp` grammar — each subcommand's happy path, the
  raw-command capture (child flags kept, `--` for a leading-dash command,
  no help after `--`), `--env`/`--header` parsing and their form
  conflicts, `--transport` refinement and its conflicts, name validation,
  help everywhere, bare `mcp`/unknown-subcommand errors.
- `mcp::config`: `render_server`/`parse_server_entry` round-trips for all
  four shapes (the type-less fallback included), `record_server` (fresh
  file, append order, sibling-key preservation, `Exists`, `InvalidJson`,
  empty-contents-as-fresh), `remove_server` (**order preservation** —
  the `shift_remove` guarantee — `Missing`, last-entry removal dropping
  the `mcpServers` key never the document).
- `mcp::names`: `validate_server_name` accepts the wire-safe class,
  rejects and names anything else.
- `tests/mcp_cli.rs` (the `detached_exec.rs` pattern — the real binary
  via `CARGO_BIN_EXE_alter-zero`, `ALTER_ZERO_MCP_FILE` into a temp dir):
  add stdio + http + sse + add-json, the file's exact JSON, list order,
  get's masking, remove keeping order, duplicate/missing/invalid-file
  exit codes, help, and the invalid file left byte-identical.
- `scripts/smoke.sh` Phase 84: the CLI installs the Phase 80 scripted
  stdio fixture into a temp user file, then the TUI boots against that
  same file and `/mcp` shows the server `✔ connected · 1 tool` — proving
  the file the CLI writes is the file the session reads.
