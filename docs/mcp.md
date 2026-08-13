# MCP servers — the `/mcp` manager and the `mcp__server__tool` tools

Model Context Protocol support, Claude-Code style: the user declares MCP
servers in a config file, the session connects to each at startup, every
connected server's tools join the model's tool set under fully-qualified
`mcp__{server}__{tool}` names, and the `/mcp` command opens an inline manager
(the sixth composer-replacing picker) that walks servers → server detail →
tools → tool detail, with an OAuth authentication flow for remote servers
that need one.

## Configuration

Two scopes, first scope wins a name (project shadows user — the Claude Code
precedence, minus the `local` scope we don't model):

- **Project**: `{project_root}/.mcp.json` — the nearest-`.git` root's file
  (the `AGENTS.md` walk-up, `project_doc::find_project_root`), so launching
  in `repo/src` still finds the repo's servers. Claude Code's shared-project
  convention, checked into the repo.
- **User**: `{config_home}/mcp.json` (`~/.alter-zero/mcp.json`) — personal
  servers, every project.

Both files carry Claude Code's exact shape, so a `.mcp.json` written for it
loads here unchanged:

```json
{
  "mcpServers": {
    "deepwiki":  { "type": "http",  "url": "https://mcp.deepwiki.com/mcp" },
    "everything": { "type": "stdio", "command": "npx",
                    "args": ["-y", "@modelcontextprotocol/server-everything"],
                    "env": { "KEY": "value" } },
    "legacy":    { "type": "sse",   "url": "https://example.com/sse",
                   "headers": { "Authorization": "Bearer …" } }
  }
}
```

`type` is optional: an entry with `command` defaults to `stdio`; an entry
with only `url` defaults to `http` **with automatic fallback** — a
streamable-HTTP `initialize` that fails with a client error retries the same
URL as a legacy SSE server (the MCP spec's backwards-compatibility recipe),
so a bare `{"url": …}` works against either generation of server.

**Disabled state** lives per project in the **user** file — never in the
project file, which may be shared with a team:

```json
{ "mcpServers": { … },
  "projects": { "/abs/cwd": { "disabled": ["github"] } } }
```

— the `skills.json` pattern exactly: read-modify-write, best-effort, an
empty set drops the entry, a name not installed here is kept (the same file
serves a checkout elsewhere).

Env knobs: `ALTER_ZERO_MCP` (falsy = the whole feature off: no connections,
no tools, `/mcp` explains via toast), `ALTER_ZERO_MCP_FILE` (replaces the
*user* file path — what makes a smoke run hermetic; the project file is
still discovered), `ALTER_ZERO_MCP_STARTUP_TIMEOUT_MS` (per-server
initialize + tools/list budget, default 30 000 — both references' default),
`ALTER_ZERO_MCP_TOOL_TIMEOUT_MS` (per `tools/call`, default 120 000).

## The module split

The rule everywhere else applies here: **`src/mcp/` is pure**, the I/O lives
in **`src/llm/mcp/`**.

- `mcp::config` — the file parse (`McpFile`, `McpServerConfig`:
  `Stdio { command, args, env }` / `Http { url, headers }` /
  `Sse { url, headers }`, the `type`-defaulting rules, the per-project
  disabled sets, scope labels). A server whose entry won't parse is reported
  by name (a red startup toast), never silence — the `SKILL.md` posture.
- `mcp::names` — the naming contract. `normalize_name` maps anything outside
  `[A-Za-z0-9_-]` to `_` (Claude Code's `normalizeNameForMCP`);
  `tool_wire_name(server, tool)` builds `mcp__{server}__{tool}`;
  `is_mcp_tool` is the `mcp__` prefix test (the `is_task_tool` shape);
  `parse_wire_name` splits it back (the server part never contains `__` —
  normalization collapses runs — so the split is unambiguous);
  `tool_display_name(server, tool)` is Claude Code's user-facing
  `{server} - {tool} (MCP)`, `tool_label`/`label_from_wire` is that name
  without the suffix (the permission prompt renders the arguments *between*
  the two, and names the label in its rule), `batch_label` is the aggregated
  `deepwiki, context7 3 times` form, and `wire_from_display` inverts the
  display name for the context replay.
- `mcp::protocol` — JSON-RPC 2.0 framing (`request`/`notification` builders,
  `Response` parse with `result`/`error` split), the `initialize` handshake
  params (protocol version `2025-06-18`, `clientInfo` naming this crate) and
  result (`ServerIdentity`: name, version, capabilities, instructions), the
  `tools/list` page parse (`McpToolInfo { name, description, input_schema }`,
  cursor-chained), the `tools/call` params builder, and the result mapping:
  `CallToolResult` → the cell/model text (a lone `text` content item is its
  text verbatim; anything else renders as compact JSON so nothing is
  dropped), `is_error` → the red cell, and a first `image` content item →
  a `data:` URL riding `ToolOutcome::image` (the `read` tool's channel, so
  a vision model *sees* an MCP image).
- `mcp::sse` — the Server-Sent-Events frame parser (`data:`/`event:` lines,
  multi-line data, comment lines) shared by both HTTP transports; pure over
  `&str` pushes, tested without a socket.
- `mcp::status` — what the UI consumes: `McpServerSnapshot { name, scope,
  config_path, status, url_or_command, auth, server_info, tools }` with
  `McpServerStatus` (`Connected`/`Pending`/`NeedsAuth`/`Failed(reason)`/
  `Disabled`), the status glyph/label mapping (`✔ connected · 3 tools`,
  `△ needs authentication`, `◯ disabled`, `✘ failed`), and the parameter
  listing a tool detail page renders from an `input_schema`.

`src/llm/mcp/` (boundary, verified hermetically against in-process fixtures —
a scripted `sh` stdio server, `std::net::TcpListener` HTTP servers):

- `transport` — the three transports behind one `request`/`notify` surface:
  - **stdio**: the configured command spawned with piped stdio (env merged
    over the session's), a reader thread turning stdout lines into messages,
    stderr drained to a capped buffer (surfaced in a connect error), the
    child killed on drop. Requests are newline-delimited JSON, answers
    matched by id.
  - **streamable HTTP**: POST per message (`Accept: application/json,
    text/event-stream`), the response either a JSON body or an SSE stream
    drained until the request's id answers; the `Mcp-Session-Id` response
    header captured at initialize and echoed thereafter, with the
    `MCP-Protocol-Version` header; notifications POST and ignore the body.
  - **legacy HTTP+SSE**: GET opens the event stream on its own thread, the
    first `endpoint` event names the POST target (resolved against the
    base URL), requests POST there and answers arrive on the stream.
  All three poll the turn's `CancelToken` on the 20 ms cadence while
  waiting (the hooks-runner contract — a hung server must not eat Esc) and
  enforce the configured deadlines.
- `client` — the per-server connect sequence: `initialize` →
  `notifications/initialized` → `tools/list` (cursors drained). A 401 (or
  the http→sse fallback both failing with one) resolves as **NeedsAuth**,
  carrying the `WWW-Authenticate` detail for the OAuth discovery.
- `oauth` — the RFC-shaped authorization-code + PKCE flow: protected-resource
  metadata (RFC 9728) → authorization-server metadata (RFC 8414, with the
  OIDC fallback path) → dynamic client registration (RFC 7591) when offered →
  the authorize URL (S256 challenge, `state`, RFC 8707 `resource`) → a
  loopback `TcpListener` callback server (fixed port range, first free) *and*
  the paste-the-redirect-URL fallback — both accepted, whichever lands
  first — → the token exchange, refresh-token rotation on expiry. Tokens
  persist in `{config_home}/mcp-auth.json` keyed by server URL
  (0600, best-effort), and ride every HTTP request as `Authorization:
  Bearer …`. "Clear authentication" deletes the entry.
- `manager` — the session-owned registry (`McpManager`, the
  `Arc<Mutex<…>>` sibling of `BackgroundRegistry`): holds each server's
  config + live state, connects them **concurrently on worker threads at
  startup** (never blocking the first frame), exposes `tool_specs()` (the
  Chat Completions defs for every connected, enabled server's tools — the
  `input_schema` passed through as the `parameters`), executes
  `call_tool` (wire name → server + tool via the connect-time map), and
  reports every state change on the loop's **MCP event channel** (a
  `tokio` unbounded sender — the file-search/clipboard worker pattern), so
  the UI is repainted the moment a server connects, fails, or finishes
  authenticating.

## The tool integration

The backend follows the skills pattern:

- `LlmBackend::with_mcp(manager)` (gated on `tools_enabled`) attaches the
  manager; `sync_tool_specs` extends the set with `manager.tool_specs()`.
  Subagents carry it too (`SubagentConfig`) — a side agent queries a wiki
  exactly as the lead does.
- The `spawn` execute closure routes `mcp::is_mcp_tool(&call.name)` to
  `manager.call_tool(call, &cancel)` ahead of the `RealToolExecutor`
  fallthrough (the `is_task_tool`/`is_skill_tool` slot).
- The offered set can change mid-session (a server connects late, a
  reconnect, a `/mcp` disable): the boundary re-checks a fingerprint of the
  offered wire names at every MCP event and turn start
  (`Session::refresh_mcp`, the `skills_attached` pattern) and rebuilds the
  backend only when the set actually flipped.
- **Display**: `llm::tools::display_name` maps a wire name to the
  `{server} - {tool} (MCP)` header; `summarize_call` returns the **raw
  arguments JSON** for an MCP call. That choice is what makes the record
  replayable: `ToolCall.args` *is* the verbatim arguments, so
  `context::context_messages` can replay the native `tool_calls` pair with
  the model's own JSON (`reconstruct_arguments` returns it; `wire_tool_name`
  inverts the display name), a `/resume` keeps it, and a validating provider
  never sees a placeholder `{}`. The **pretty** `key: "value"` form the
  header shows is derived at render time (`ui::tool`'s MCP branch), never
  stored — **in the model's own key order**, which is why `serde_json` is
  built with `preserve_order`: a schema's `repoName` was written before the
  long `question` it qualifies, and re-sorting the pair alphabetically buried
  the short argument under the long one.
- **Permission**: an MCP call asks first, like Claude Code. A fourth
  `PermissionKind::Mcp` rides the existing gate, shaped like the `bash`
  prompt — because it answers the same question, *what exactly is about to
  run?*:

  ```text
   Tool use

     deepwiki - read_wiki_structure(repoName: "linuztx/flaredantic") (MCP)
     Get a list of documentation topics for a GitHub repository.

   Do you want to proceed?
   ❯ 1. Yes
     2. Yes, and don't ask again for deepwiki - read_wiki_structure commands
        in ~/Codes/tests
     3. No

   Esc to cancel · Tab to amend
  ```

  The body is the call **as its cell will read it** — the display label, the
  arguments in the same `key: "value"` form, the ` (MCP)` marker closing it
  dim so the eye lands on the tool — over the **server's own description** of
  the tool, dim (`McpManager::tool_description` → the `approve_call`
  `describe` seam → `PermissionRequest::detail`, the `bash` description's
  slot). No `╌` frame: the pretty-printed JSON it replaced spent five rows
  re-punctuating two arguments, and pushed the options toward the screen
  bottom. Option 2 still remembers the **exact wire name** in the
  exact-command allowlist (`mcp__deepwiki__read_wiki_structure` — persisted
  in `permissions.json` like any exact rule) while *saying* the display
  label and the project the rule is kept for (`App::project_dir`, the
  footer's cwd — the allowlist is per project). `edit` mode still asks (a
  remote tool is not a file edit), `auto` mode sends an uncovered call to
  the **auto mode classifier** instead of the user — exactly as it does a
  `bash` command, and exactly as the reference feeds MCP calls to its
  auto-mode classifier (`mcpToolInputToAutoClassifierInput`): the classifier
  reads the tool named `{server} - {tool}`, the server's own description,
  and the arguments (`llm::classifier::classifier_request_prompt`'s MCP arm
  — the wire name alone would hide where the risk lives), the allowed call
  runs with the dim `⎿ Allowed by auto mode classifier` row, a denial
  rejects red with the classifier's reason, and a classifier *failure*
  falls back to this prompt (`docs/permissions.md`) — and `master` runs it
  unasked like everything else.

## Rendering — the collapsed inline cell vs the expanded transcript

An MCP cell is recognised by its display name's ` (MCP)` suffix (pure —
survives a `/resume` with no extra record field). Inline it is deliberately
quiet; Ctrl+O carries the full story:

- **Running**: `● Calling {server}… (ctrl+o to expand)` — the breathing
  grey bullet, the hint dim on the header — **and nothing else**. It used to
  carry a `⎿ "{question}"` peek row; a fragment of one argument, wrapped at
  the width, says what the header already said and costs the row.
- **A batch**: when **every** queued call is MCP, the batch is one act and
  shows as one cell — `● Calling deepwiki 2 times…`, `● Calling deepwiki,
  context7 4 times…` (`mcp::names::batch_label`: distinct servers in call
  order, the total when there is more than one) — in the live strip *and*
  above a permission prompt asking about one of its calls, where the
  per-call rendering used to stack a screenful of identical `⎿ Waiting…`
  cells in the rows the question needed. The count is the **batch's**: the
  resolved siblings are read back off the history (`ui::tool::
  mcp_batch_lines`), so a running batch's label doesn't count itself down.
  A mixed batch keeps the ordinary per-cell strip, `⎿ Waiting…` rows and all.
- **Resolved ok**: the bullet-less dim two-tone
  `Called {server} (ctrl+o to expand)` line — the settled thinking line's
  shape (`summary_lines`), because what is left is a fact about the turn,
  not output to read. The result text never reaches inline scrollback;
  that is *why* the cell can collapse. A **parallel run** resolves to one
  such line for the whole run — `Called deepwiki 2 times (ctrl+o to
  expand)` — because two lines saying `Called deepwiki` describe the batch
  no better than one that counts it.
- **Failed**: the loud generic red cell (full header + error peek) — a
  failure must not whisper, and it ends the run it is part of (the ok cells
  before it commit as their own aggregated line, then the red cell).
- **Ctrl+O**: unchanged and per call — the full
  `● {server} - {tool} (MCP)({pretty args})` header over the complete output
  in the `⎿` gutter. The transcript is what *happened*; the inline line is
  what is left of it. A header that wide wraps with a **hanging indent**
  rather than aligning its continuation rows under the opening `(`
  (`TOOL_HEADER_ALIGN_SHARE` — past a third of the width the alignment costs
  more than it buys), so the arguments get the row instead of a ragged
  column.

**How one line can be both**: the aggregation is a property of the
*history*, not of a live buffer, so scrollback and the resize repaint derive
it from the same place (invariant 3). Every call of an announced batch
carries that batch's id (`ToolCall::batch`, stamped by `App::
start_tool_batch`, round-tripped through the rollout), and a **parallel MCP
run** is a maximal run of consecutive history cells that are MCP, resolved
ok, and share one id (`ui::tool::mcp_run`) — the id being what keeps two
*sequential* calls that merely landed next to each other from claiming they
ran in parallel. `conversation_lines` collapses each such run when it
repaints; `ui::tool_commit_lines` — the one builder every resolution commits
through — **holds** a call's line while the next call of its batch is about
to start, then writes the run's single line when it ends. Whatever ends the
run flushes it: its last call, a failure in the middle of it, an
interrupt. (A held run also holds the background-completion settle, so a
notice can never land inside one.) A subagent's calls carry no batch id: its
session view keeps a cell per call.

## The `/mcp` manager

The sixth composer-replacing picker (`app::McpMenu`, `ui::mcp_view`), and
deliberately the **hooks-menu twin** rather than a new shape: no text entry
(cursor parked in the corner), ↑/↓/digits/Enter/Esc, a selection-centred
window with `↑ N more above` / `↓ N more below` overflow markers, working
mid-turn (the strip stays above it). Pages:

1. **Server list** — `Manage MCP servers` + `{n} servers`, the rows grouped
   under dim scope headings that name the config file
   (`Project MCPs ({root}/.mcp.json)`, `User MCPs (~/.alter-zero/mcp.json)`),
   each row `{name} · {glyph} {status}` (`✔ connected · 3 tools`,
   `△ needs authentication`, `◯ disabled`, `✘ failed`). An empty list names
   both file paths — "why isn't my server here?" is its only question.
2. **Server detail** — the status/auth/URL-or-command/config-location fact
   rows (+ capabilities and tool count when connected), then the actions the
   state affords: connected → `View tools` / `Re-authenticate` (when OAuth
   tokens exist) / `Clear authentication` / `Reconnect` / `Disable`;
   needs-auth → `Authenticate` / `Disable`; failed → `Reconnect` /
   `Disable`; disabled → `Enable`.
3. **Tools list** — `Tools for {server}` over numbered tool names.
4. **Tool detail** — the tool name + server, `Tool name:` / `Full name:`
   (the wire `mcp__{server}__{tool}` the model actually calls),
   the description, and the `Parameters:` listing derived from the
   `input_schema` (`● {name} (required): {type} - {description}`).
5. **Authenticate** — `Authenticating with {server}…`: the flow opens the
   browser (`xdg-open`/`open`, detached), shows the authorize URL (`c`
   copies it via the `/copy` clipboard seam), and offers the `URL >`
   paste-the-redirect fallback (a single-line field; paste routed to it
   while the page is up); the loopback callback resolves it from either
   side. Esc backs out; the result lands as a toast + the server list
   re-sorting itself.

Ops dispatch as `Action::McpOp(op)` to `tui::mcp::Session::apply_mcp_op`,
which runs connection work on worker threads and persists disabled state
via the user-file read-modify-write; every completion reports on the MCP
event channel, which re-injects the snapshot (`App::set_mcp_snapshot`) and
schedules a frame — the picker is always looking at live state, and a
pending row shows `… connecting` until its thread reports.

## What stays out (v1)

Resources, prompts, sampling, elicitation, and roots are not modeled — the
tools surface is the feature. Server `instructions` are shown in the detail
page but not injected into the system prompt. WebSocket transport isn't
offered (neither reference ships it for user servers). The dummy backend
scripts no MCP scenario — the manager is real I/O end to end; the smoke
suite drives `/mcp` against a scripted stdio fixture instead.

## Testing

The pure halves are unit-tested as usual (the naming contract, the argument
pretty-printers, every cell/prompt/aggregation rendering, the commit-vs-repaint
agreement), and the boundary transports against in-process fixtures. What
those can't prove — that a real server's tools, descriptions and parallel
batches come out looking right — lives in **`tests/live_mcp.rs`**, `#[ignore]`d
like every other live test:

```sh
# the public DeepWiki server only (no key needed)
cargo test --test live_mcp -- --ignored --nocapture live_deepwiki_tool_descriptions
# the whole round trip: a real model issuing a real parallel batch
OPENROUTER_API_KEY=sk-or-… cargo test --test live_mcp -- --ignored --nocapture
```

The second one folds the turn's real `StreamEvent`s into `App` the way
`tui::stream` does and asserts on the lines a terminal would show: the strip's
one `● Calling deepwiki 2 times…` cell, the single committed
`Called deepwiki 2 times`, and the per-call Ctrl+O headers with the model's own
argument order.
