# Project-level `.alter-zero` config — hooks + MCP behind a trust gate

A project can now carry its own alter-zero config in a `.alter-zero` directory
at its root — `/home/user/project/.alter-zero/` — so different projects load
different hooks, MCP servers and skills. The directory name is `.alter-zero`
(the crate's name, the config home's name, and the name the skills walk already
uses), not a second spelling.

The **project root** is the nearest ancestor-or-self of the cwd carrying a
`.git` entry (`project_doc::find_project_root` — the `AGENTS.md`/skills/MCP
walk-up, so launching in `repo/src` still finds `repo/.alter-zero`), falling
back to the cwd itself in a directory that isn't a repo — **except the home
directory, which is never a project** (`trust::is_project_root`, the
`checkpoint::cwd_scope` instinct): launched in `~` the fallback root would
make `{root}/.alter-zero` the user's own config home, and the layer would
rediscover the user's `hooks.json`/`mcp.json` as pending "project config"
and ask the user to trust their own files. In `~` the layer is simply off —
no pending toast, `/trust` explains — while the same files keep loading as
the user layer they are. A per-file identity guard in `tui::trust` catches
the same collision anywhere else (an `ALTER_ZERO_CONFIG_DIR` pointed inside
a project): a project path that *is* the user layer's file is never re-read
as project config.

What the project layer reads:

| file                            | what                                        | merge rule                                   |
| ------------------------------- | ------------------------------------------- | -------------------------------------------- |
| `{root}/.alter-zero/skills/`    | project skills (already shipped, unchanged) | first-root-wins walk (`docs/skills.md`)      |
| `{root}/.alter-zero/hooks.json` | project lifecycle hooks — **new**           | **union** after the user file (see below)    |
| `{root}/.alter-zero/mcp.json`   | project MCP servers — **new**               | first-name-wins, ahead of `.mcp.json`        |
| `{root}/.mcp.json`              | Claude-Code-compat MCP servers (existing)   | first-name-wins, after `.alter-zero/mcp.json` |

## The trust gate

`docs/hooks.md` deferred project hooks for exactly one reason: **cloning a
repository must not earn it code execution.** A checked-in `hooks.json` runs
shell commands on lifecycle events; a checked-in MCP entry spawns a stdio
server process at startup. (The pre-existing `.mcp.json` support had that hole
— its servers launched unasked — so the gate now covers it too; that is the
one behavioral change to an existing feature, and the fix is one keypress.)

So the project layer is **default-deny**: untrusted project hooks/MCP files
are read and parsed for review, but no handler runs and no server launches
until the user approves them.

- Trust is recorded per project in `{config_home}/trust.json`:
  `{"projects": {"/abs/root": {"files": {"/abs/file": "sha256:<hex>"}}}}` —
  a **content hash** per file (the codex `trusted_hash` answer). Editing a
  trusted file makes it untrusted again; approval re-records the new hash.
  Writes are read-modify-write so other projects' entries survive. A
  malformed `trust.json` **fails closed** (nothing trusted) with a red toast
  — a guard file is loud, never silently absent.
- At startup, pending (present-but-untrusted) project config raises one info
  toast: `Project .alter-zero config found — /trust to review`.
- **`/trust`** opens the review menu (the `/hooks` browser's sibling — a
  composer-replacing inline view, works mid-turn, Esc closes): the project
  root, then every hook command and every MCP server target **verbatim** —
  what you approve is exactly what will run — over
  `❯ 1. Trust this project's config` / `2. Revoke trust` (`3. Close`).
- Approval records the fingerprints of the **reviewed snapshot** — the
  content loaded at startup, which is also exactly what activates — never a
  fresh re-read, so a file swapped on disk mid-review can't get approved
  sight-unseen; an edit made after startup simply shows up pending again at
  the next launch. Approval **activates live**: the hooks file re-merges
  into the running session (backend rebuild — no restart) and untrusted MCP
  servers connect. Revoke deactivates the same way (project hooks drop out
  of the merge, project-scope servers disconnect and return to
  `untrusted`).
- Untrusted MCP servers still appear in `/mcp` — status `untrusted`, never
  connected, no actions; the detail page points at `/trust`. A server you
  can't see is a server you can't reason about.
- **Skills stay outside the gate**: a `SKILL.md` is inert markdown until the
  model explicitly loads it, and any command in its body still meets the
  permission gate like every other tool call. (Its one-line description does
  ride the context listing — the standing behavior since `docs/skills.md`.)

## Merge semantics

**Hooks: layers union.** The merged file is the user file with the project
file's matcher groups appended per event (`HooksFile::merged`) — purely
additive, the `docs/hooks.md` pre-commitment: one more entry from the loader,
no precedence rules to invent. The existing `select()` rules already answer
every collision: duplicates dedup by command (first wins — the user's copy),
any block wins, `deny` > `ask` > `allow`, contexts concatenate. A malformed
project file is a red toast naming it and the project layer stays off; the
user's own hooks still load (each layer fails alone).

**MCP: first name wins** across `[{root}/.alter-zero/mcp.json,
{root}/.mcp.json, {config_home}/mcp.json]` — the existing project-shadows-user
precedence with the app-specific file ahead of the compat file (the skills
convention: `.alter-zero` outranks `.claude`). Both project files are
`McpScope::Project`, so the `/mcp` grouping is unchanged; each entry's
`Config location:` row names its actual file.

## Lifecycle and knobs

Project hooks/MCP files keep their subsystems' **read-once-at-bootstrap**
lifetime; the one extra read is `/trust`'s approval (activating exactly the
bytes it hashed). Mid-session edits otherwise need a restart, exactly like
the user-level `hooks.json`/`mcp.json`. Skills keep their per-turn rescan.

- `ALTER_ZERO_PROJECT_CONFIG` — default on; `0`/`false`/`no`/`off` turns the
  whole project layer off: no project hooks/MCP discovery, no toast, `/trust`
  explains via toast (the `ALTER_ZERO_MCP` posture). **This is the smoke
  suite's hermeticity switch** — `scripts/smoke.sh` sets it to `0` in the
  base env (the suite runs with cwd = a real checkout; a developer repo
  carrying `.alter-zero/hooks.json` would otherwise toast in every phase —
  the skills Phase 36 lesson), and the project-config phase turns it back on
  inside a `mktemp -d` workdir.
- `ALTER_ZERO_HOOKS_FILE` keeps replacing the **user** file only, project
  discovery still runs — the `ALTER_ZERO_MCP_FILE` convention, now shared by
  both subsystems; the project layer's own kill switch is
  `ALTER_ZERO_PROJECT_CONFIG=0`.
- `trust.json` hangs off the config home like `permissions.json` — no
  override var of its own; `ALTER_ZERO_CONFIG_DIR` moves it.

## Module split

Pure (unit-tested, TDD):

- `src/trust.rs` — the trust store: `fingerprint` (SHA-256, `sha256:<hex>`),
  `TrustFile` parse/serialize, `trusted_files`, `record_trust` /
  `revoke_trust` (string-in/string-out read-modify-write cores, the
  `record_disabled` shape), and the project-file path builders
  (`project_hooks_file`, `project_mcp_files`).
- `hooks::HooksFile::merged` — the union.
- `mcp::merge_scopes` — widened to `(project_alter, project_compat, user)`.
- `app/trust_menu.rs` — the menu's pure state; `ui/trust_view.rs` — its rows.
- `mcp::status` — the `Untrusted` server status; `app::mcp_menu`'s action
  list answers it with no actions.

Boundary (smoke-verified):

- `tui/config.rs` — `project_config_enabled()`, `trust_json_path()`,
  `load_trust_file` (fail-closed + loud).
- `tui/trust.rs` — gathering the project layer at bootstrap
  (`load_project_config`), the `/trust` open/apply arms, and live
  (de)activation: `models.set_hooks_file(merged)` +
  `manager.set_trusted(name, bool)`.
- `tui/bootstrap.rs` — the project hooks read + trust check + merge before
  the emptiness check; `HookSetup` is now built unconditionally (so a later
  approval can swap the file in) with `hooks_available()` reading the file's
  non-emptiness instead.
- `tui/mcp.rs` — `load_mcp_sources` reads the second project file and feeds
  the manager an `untrusted` name set (the `disabled` twin).
- `llm/mcp/manager.rs` — untrusted servers never connect;
  `set_trusted` flips status and connects/kills without touching the
  disabled persistence.

Smoke: the base env gains `ALTER_ZERO_PROJECT_CONFIG=0`; **Phase 83** builds
a temp project (`.alter-zero/hooks.json` + `.mcp.json` with a scripted stdio
fixture), asserts the pending toast and the `untrusted` `/mcp` row, walks
`/trust` (review rows → approve), asserts the hooks source joined into
`/hooks` and the server connecting, then relaunches to prove trust persisted
(no toast, server connects unprompted).

## Deliberately not doing

- **No blocking startup modal.** Default-deny + a toast + `/trust` keeps
  bootstrap non-interactive (nothing may block the first paint) and the
  approval reviewable at leisure. The modal machinery (permission gate) is
  turn-scoped and earns nothing here.
- **No per-file or per-server partial trust.** One project, one verdict —
  matching Claude Code's folder trust. The `/mcp` disable toggle already
  answers "trusted but I don't want this server".
- **No project-level `permissions.json`/`settings.json`.** Those are
  preference files, not capabilities; nothing in them is worth the trust
  surface today.
- **No file watcher / per-turn rescan for hooks+MCP.** They keep their
  established read-once lifetime; skills keep their rescan. A project layer
  that appears mid-session is a restart away, like every other hooks/MCP
  edit.
