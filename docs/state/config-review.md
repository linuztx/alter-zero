# Reviewing hooks and project MCP servers before they run

A hook runs a shell command on its own, every time its event fires, outside
the permission prompt. A project's MCP server is a program the session starts,
or a URL it connects to, because a file in the checkout says so. Both are code
that runs without a tool call to approve, so neither runs until the user has
looked at it.

This page replaces the `/trust` gate (`docs/state/project-config.md` keeps the
project layer's discovery and merge rules). `/trust` approved a project's
config files whole, by content hash, from a menu the user had to know to open:
it covered only the three project files, never the user's own `hooks.json`,
and its one answer covered every hook and server at once. What replaces it is
a **review dialog** the session opens by itself, at startup, whenever there is
something new or changed to decide on — the shape both references settled on
(Claude Code asks which of a project's `.mcp.json` servers to enable; codex
asks the user to trust each new or changed hook).

## What needs a decision

| what | where it is read from | the decision | recorded as |
| ---- | --------------------- | ------------ | ----------- |
| every hook that can run | the user's `hooks.json` (`~/.alter-zero/hooks.json`, or `ALTER_ZERO_HOOKS_FILE`) **and** the project's `{root}/.alter-zero/hooks.json` | trust it, or not | the hook itself, per file |
| every project MCP server | `{root}/.alter-zero/mcp.json`, `{root}/.mcp.json` | enable it, or not | a decision per server, per file, pinned to its entry |

A hook "can run" when the runner would run it: a `command` handler with a
command, under an event this app models, with no `if` condition and no
`powershell` shell (`hooks::HookHandler::runnable_command`). The others are
skipped with a warning whatever the trust says, so asking about them would ask
about nothing.

Not reviewed: the user's own MCP servers (`~/.alter-zero/mcp.json`, written by
the user or `alter-zero mcp add` — Claude Code does not ask about them
either), skills and subagent definitions (markdown that runs nothing on load,
`docs/state/project-config.md`).

## The store: `trust.json`

`{config_home}/trust.json` keeps what the user decided, keyed by the absolute
path of the file each thing was read from — never inside the project, since a
checkout that could carry its own approvals would approve itself. The user's
own file is keyed absolute too: a relative `ALTER_ZERO_HOOKS_FILE` kept as
written would be one key for a different file in every directory
(`config::user_hooks_path`).

```json
{
  "hooks": {
    "/home/me/.alter-zero/hooks.json": [
      { "event": "PreToolUse", "matcher": "bash", "command": "./guard.sh" }
    ]
  },
  "mcp": {
    "/home/me/repo/.mcp.json": {
      "github": {
        "enabled": true,
        "fingerprint": "sha256:…",
        "parts": {
          "transport": "http",
          "target": "sha256:…",
          "values": { "Authorization": "sha256:…" }
        }
      }
    }
  }
}
```

**A hook is trusted while exactly that hook is listed** for its file: the
event, the matcher (absent, empty and `*` all being the same match-everything
matcher) and the command. The matcher is kept **exactly as the runner reads
it** (`hooks::constraint`, the one definition both use) — never trimmed:
` Bash` is a regex no tool name matches, `Bash` fires on every bash call, and
a trimmed record would let the first's trust carry over to the second. The
dialog quotes a padded matcher (`PreToolUse · " Bash"`) so the two never read
the same. That is what decides what runs and when; a
`timeout` or a `statusMessage` changes neither, so editing one asks nothing.
Recording the hook itself rather than a hash of it is what lets the dialog
show what a **changed** hook used to be: an untrusted hook whose event and
matcher match a listed hook the file no longer has is that hook, edited, and
the dialog prints the old command under the new one (`was: cargo fmt`) —
which is the line that makes a tampered hook stand out. Order does not
matter: moving a trusted hook within its file asks nothing.

**A server's decision is pinned to its whole entry** — command, args and env,
or url and headers (`trust::server_fingerprint` over the entry as
`mcp::render_server` writes it) — so a changed entry is asked about again,
whatever its name. **No value of the entry is kept** — not the URL, not the
command line, not an `env` value or a header: a token can sit in any of them
(`?api_key=` in a URL, `--api-key` in the args), and `trust.json` is a file
people sync with their dotfiles. What is kept beside the fingerprint is the
entry's shape, `trust::ServerParts`: the transport, a fingerprint of the
command line or URL, and the names of its `env` variables or headers, each
over a fingerprint of its value. That is enough for a changed entry to say
**what** changed (`ServerParts::changes`) — `changed: env NODE_OPTIONS added,
the command line` — under the entry as it is now, which is the line that
makes an injected variable stand out; a change the parts cannot name (an
`http` entry's `type` dropped, which may now fall back to SSE) shows only
the `changed` badge.

Every write is a read-modify-write that replaces one file's record and leaves
every other file's alone — over `trust.json` **as it is on disk when the
decision is made** (`trust::update_store`, the boundary's `persist`), never
over the copy the session read at startup: two sessions open at once each
keep the other's answers. A hook record is rewritten to the file's current
hooks: a hook removed from the file is dropped, so the same command added back
later is asked about again.

A malformed `trust.json` **fails closed** — nothing trusted, nothing enabled,
a red toast naming the file — and is **never written over**: what the user
decided before is still in it, waiting for the typo to be fixed, so a
decision made meanwhile holds for the session only, and a red toast says so
(`trust.json: … — not saved, so this choice lasts this session only`).

### Carrying `/trust` approvals over

The `/trust` era kept a `projects` key: project root → file → the
fingerprint of the bytes it approved. Those records are read, and carried
over at startup for the project being opened (`TrustFile::migrate_legacy`):
a file `/trust` approved at **exactly its current bytes** keeps its approval
— its hooks recorded trusted, its servers enabled unless one was decided on
here since — and its old record goes. A file edited since keeps its record
unread (what `/trust` approved is not what is there now), so the dialog asks
about it. The projects not opened since keep their records through every
write, for when they are. The user's own hooks were never under `/trust`, so
they are reviewed once.

## The review dialog

At startup, once the banner is queued and before anything runs, the session
opens the review if there is anything to decide. It is a composer-replacing
inline page in the `/hooks` family — framed by the pickers' rules, no text
entry, the hidden cursor seated on its `❯` — with up to two pages: the hooks
first, then the project's MCP servers.

```
──────────────────────────────────────────────────────────────────────────
  2 hooks to review · 1 new, 1 changed  1/2
  Hooks run shell commands on their own whenever their event fires, outside
  the permission prompt. Only hooks you trust will run.

  User · ~/.alter-zero/hooks.json
  ❯ [✔] Stop  changed
        cargo fmt && cargo clippy --all-targets -- -D warnings
        was: cargo fmt

  Project · ~/repo/.alter-zero/hooks.json
    [✔] PreToolUse · bash
        ${ALTER_ZERO_PROJECT_DIR}/.alter-zero/hooks/guard.sh

    Trust selected and continue
    Continue without trusting

  Space to select · Enter to confirm · Esc to continue without trusting
──────────────────────────────────────────────────────────────────────────
```

```
──────────────────────────────────────────────────────────────────────────
  2 new MCP servers found in this project  2/2
  Select any you wish to enable.
  An enabled server runs its command, or connects to its URL, whenever a
  session starts here. Its tool calls still follow your permission mode.

  ~/repo/.mcp.json
  ❯ [✔] brave-devtools · stdio
        npx -y @brave/devtools-mcp
        env  BRAVE_API_KEY=${BRAVE_API_KEY}
    [✔] github · http
        https://api.githubcopilot.com/mcp/

    Enable selected
    Disable all

  Space to select · Enter to confirm · Esc to disable all
──────────────────────────────────────────────────────────────────────────
```

Every item shows **verbatim** what would run — a hook's whole command, a
server's command line or URL and its environment — word-wrapped, never cut,
since the tail of a command (`| sh`, a redirect) is the part worth reading.
Items are grouped by the file they came from. A changed item says so in amber
and shows what it was. Everything starts selected, as in Claude Code: the
dialog exists to make sure the user saw what runs, not to make running it
hard.

Keys: ↑/↓ move over the items and the two action rows (wrapping at the ends),
Home/End jump, **Space** toggles the highlighted item, **Enter** confirms the
selection from an item or the first action row and declines from the second,
**Esc** declines. **Ctrl+C quits** without answering anything — unlike the
pickers, whose Ctrl+C only closes them: this page opens by itself, a user
who did not mean to face it wants out, and a decline on the servers page
would record every server disabled. Nothing it asked about runs, and the
next launch asks again. The one review the user opens on purpose — **Hooks**
turned on in `/settings` (*After startup*) — is a picker they asked for, so
its Ctrl+C closes it the pickers' way: nothing answered, nothing recorded,
the session goes on (`ConfigReview::requested`).

What the answers mean:

| page | Enter (confirm) | the second row / Esc |
| ---- | --------------- | -------------------- |
| hooks | the selected hooks are recorded as trusted and run from now on; the others are not recorded, stay off for this session, and are asked about again at the next launch | nothing is recorded, none of the listed hooks run this session, and they are asked about again at the next launch |
| MCP servers | the selected servers are recorded enabled and connect at once; the others are recorded disabled | every listed server is recorded disabled |

The two pages' declines differ on purpose. A server has a settled, visible
"disabled" state and a place to change it — `/mcp` lists a declined server
as `◯ disabled` with an Enable action — so a decline is worth remembering, the
way Claude Code remembers it. A hook has no such state: an untrusted hook is
silent, and most hooks are the user's own, so a declined one is not
remembered; the next launch asks again, and the way to stop being asked is to
trust it, remove it, or turn **Hooks** off for the directory in `/settings`.

While the review is open nothing waits on it unannounced. In a
[herdr](../architecture/herdr.md) pane the session reports `blocked`, naming what the page
asks (`Trust hooks: Stop, PreToolUse · bash`, `Enable MCP servers: github`).
The `SessionStart` hooks are held (`ModelSession::hold_session_start`) and
fire when the review
closes, so a hook trusted on the first page still hears the `startup` it was
configured for, and a `[PROMPT]` from the command line is submitted then too,
so the first turn starts with the servers just enabled. A pending server is
`⚠ awaiting review` and never launched until its answer.

### A page that fits

The review opens on the first frame, so it must never flow its top into
scrollback: a page that flows purge-rebuilds the screen when it closes, which
at startup would wipe the terminal's own scrollback above the launch
(`docs/ui/view-flow.md`). So it sheds rows the way the compact pickers do
(`model_view::first_page_that_fits`), most complete first:

1. the whole page;
2. without the explanation under the title;
3. **folded**: every item one row — its title and the first row of what it
   runs, `…`-cut — but the highlighted one, which keeps its full detail;
4. the folded list **windowed** around the highlight between `↑ N more
   above` / `↓ N more below` rows, the file headings dropped (the highlighted
   item names its file instead), the window shrinking a row at a time;
5. the highlighted item's detail cut to three rows, then to one;
6. the title.

The two action rows and the hint stay on every rung.

## After startup

- **`/hooks`** shows every configured hook, trusted or not. An untrusted hook
  is marked `⚠ not trusted` in its list and its detail page says it does not
  run; Enter there trusts it, and on a trusted hook stops trusting it. Either
  is recorded and takes effect at once (the backend rebuild carries the new
  set; a running turn finishes on the old one).
- **`/settings` Hooks** turned on with hooks waiting for trust opens the hooks
  page of the review; **`/settings` MCP** turned on with project servers
  waiting for an answer opens the servers page (`docs/tools/mcp.md`).
- **`/mcp`** Enable on a declined project server records it enabled and
  connects it; Disable on a project server records it disabled. A project
  server disabled through `/mcp` before this design (the user file's
  per-directory `disabled` list) stays disabled until enabled here, which
  clears it.

## What is off

- `ALTER_ZERO_HOOKS=0`, or **Hooks** off for the directory: no hooks page —
  nothing would run.
- **MCP** off for the directory, or `ALTER_ZERO_MCP=0` for the run: no MCP
  page — nothing would run. Turning the row on asks then.
- `ALTER_ZERO_PROJECT_CONFIG=0`: no project layer at all; the user's own hooks
  are still reviewed. The smoke suite sets it in its base environment, and the
  phases that load a `hooks.json` answer the hooks page first.
- No config home (no `$HOME`, no `ALTER_ZERO_CONFIG_DIR`): decisions hold for
  the session only and are asked again at the next launch.

## What it does not do

- **It pins the command, not what the command reads.** `./fmt.sh` stays
  trusted when `fmt.sh` changes; both references draw the same line.
- **No per-turn rescan.** Hooks and MCP files keep their read-once-at-startup
  lifetime (`docs/state/project-config.md`); a hook added mid-session is reviewed at
  the next launch.

## Module split

Pure, unit-tested:

- `src/trust.rs` — the store (`TrustFile`: `review_hooks`, `server_trust`,
  `record_hooks`, `record_servers`, `migrate_legacy`), the read-modify-write
  every answer goes through (`update_store`, refusing a malformed store),
  the hook identity (`HookIdentity`, its matcher by `hooks::constraint`),
  `trusted_only`, `server_fingerprint` and the entry's value-free shape
  (`ServerParts`, `ServerChange`), `review_servers`, plus the project-root
  helpers it always had.
- `src/app/config_review.rs` — the dialog's state and keys;
  `src/ui/config_review_view.rs` — its page and the shedding ladder.
- `mcp::merge_project_scopes` — approved project servers shadow the user's,
  held ones never do; `mcp::McpServerStatus::AwaitingReview`.
- `hooks::HooksOverview` — every hook with its trust, for `/hooks`.
- `herdr::activity` — the review's page as herdr's `blocked` message.

Boundary, smoke-verified:

- `src/tui/trust.rs` — reading the layers, carrying `/trust` approvals over,
  building the pages, applying each answer (the session's copy, then
  `persist` over the file as it is on disk; the hooks rebind; the MCP release
  or hold), and finishing the review (`SessionStart`, the `[PROMPT]`).
- `src/llm/mcp/manager.rs` — the held set (`McpHold::{Review, Declined}`) and
  `set_approved`.
- Smoke Phase 83 drives both pages, the persistence, a changed hook and a
  changed server, the `/settings` review's Ctrl+C, two sessions answering at
  once, a malformed store left alone and a `/trust` approval carried over.
