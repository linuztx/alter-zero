# MCP servers — the `/mcp` manager and the `mcp__server__tool` tools

Model Context Protocol support, Claude-Code style: the user declares MCP
servers in a config file, the session connects to each at startup, every
connected server's tools join the model's tool set under fully-qualified
`mcp__{server}__{tool}` names, and the `/mcp` command opens an inline manager
(the sixth composer-replacing picker) that walks servers → server detail →
tools → tool detail, with an OAuth authentication flow for remote servers
that need one.

## Configuration

Two scopes over three files, first occurrence of a name winning (the Claude
Code precedence, minus the `local` scope we don't model):

- **Project**: `{project_root}/.alter-zero/mcp.json` first (the app-specific
  location, `docs/project-config.md`), then `{project_root}/.mcp.json` (the
  Claude-Code-compat convention, checked into the repo) — both at the
  nearest-`.git` root (the `AGENTS.md` walk-up,
  `project_doc::find_project_root`), so launching in `repo/src` still finds
  the repo's servers.
- **User**: `{config_home}/mcp.json` (`~/.alter-zero/mcp.json`) — personal
  servers, every project.

**The project scope sits behind the trust gate**
(`docs/project-config.md`): a checked-in stdio entry is a process this app
would spawn at startup, so an untrusted project file's servers are *listed*
in `/mcp` — status `⚠ untrusted`, no actions — but never launched (and
never shadow the user's own same-named server) until the user approves the
project in `/trust`. Approval connects them live; an edited file is
untrusted again at the next launch. This is the one behavioral change to
the original two-scope design, and it is deliberate: `.mcp.json` used to
launch unasked, which is exactly the clone-equals-code-execution hole the
hooks doc refused to open.

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
streamable-HTTP handshake that fails with a client error retries the same
URL as a legacy SSE server (the MCP spec's backwards-compatibility recipe),
so a bare `{"url": …}` works against any generation of server.

## Protocol versions — the dual-era client

This client speaks the **modern `2026-07-28` revision** (stateless: no
`initialize` handshake, the protocol version + client capabilities riding
every request's `_meta`, `server/discover` the identity surface) *and* the
handshake-based **legacy revisions** (`2025-11-25` and earlier), detecting
each server's era with the spec's own recipe — see `mcp::protocol` and
`llm::mcp::client` below. Detection runs at **every** connect and the
revision it settles on is the server's own answer, never a remembered one
(the retired era cache is below); the revision shows as the detail page's
`Protocol:` row.

**Disabled state** lives per project in the **user** file — never in the
project file, which may be shared with a team:

```json
{ "mcpServers": { … },
  "projects": { "/abs/cwd": { "disabled": ["github"] } } }
```

— the `skills.json` pattern exactly: read-modify-write, best-effort, an
empty set drops the entry, a name not installed here is kept (the same file
serves a checkout elsewhere).

The **user** file can be edited from the command line — `alter-zero mcp add
{name} --url {url}` (and `add-json`/`remove`/`get`/`list`), resolved before
the TUI boots like `--continue`/`--resume`; see `docs/mcp-cli.md`.

Env knobs: `ALTER_ZERO_MCP` (falsy = the whole feature off: no connections,
no tools, `/mcp` explains via toast), `ALTER_ZERO_MCP_FILE` (replaces the
*user* file path; the project files are still discovered),
`ALTER_ZERO_PROJECT_CONFIG` (falsy = no project files at all — the project
layer's own kill switch, shared with hooks, and the smoke suite's
hermeticity mechanism; `docs/project-config.md`),
`ALTER_ZERO_MCP_STARTUP_TIMEOUT_MS` (per-server initialize + tools/list
budget, default 30 000 — both references' default),
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
  `Deepwiki, Context7 3 times` form, and `wire_from_display` inverts the
  display name for the context replay. `capitalize_server`/
  `capitalize_display` upcase the server's first character (`deepwiki` →
  `Deepwiki`) for the **render seams only** — the cell header, the
  `Calling`/`Called` lines, the permission prompt's label and the `/mcp`
  detail headline: the recorded `ToolCall::name` keeps the configured
  spelling because `wire_from_display` preserves case, so a capitalized
  record would replay as `mcp__Deepwiki__…`, a tool the model was never
  offered (the `the_display_capitalization_never_reaches_the_wire` test is
  the guard). The rule keys (`permissions.json`) and the model-facing
  classifier prompt stay on the raw spelling for the same reason.
- `mcp::protocol` — JSON-RPC 2.0 framing (`request`/`notification` builders,
  `Response` parse with `result`/`error` split, `RpcError` keeping the
  error's `data` whole — `-32022` names the server's versions there), **both
  eras' vocabularies**: the modern (2026-07-28, `PROTOCOL_VERSION`) side —
  `request_meta`/`with_meta` (the `io.modelcontextprotocol/*` `_meta` block
  every modern request carries: version, `clientInfo`, and an *empty*
  `clientCapabilities`, the schema's own "no optional capabilities"),
  `discover_params`/`parse_discover` (`server/discover`, whose `serverInfo`
  rides the result's `_meta`, never a top-level field), `is_modern_error`
  (the `-32020`/`-32021`/`-32022` allowlist era detection keys on),
  `unsupported_versions` + `choose_version` (the `-32022` negotiation),
  `header_value` (the `Mcp-Method`/`Mcp-Name` header encoding with the
  `=?base64?…?=` sentinel for non-header-safe names) — and the legacy side:
  the `initialize` params (`initialize_params_for`, proposing
  `LEGACY_PROTOCOL_VERSION` `2025-11-25` by default) and result parse
  (`ServerIdentity`: the settled `protocol_version`, name, version,
  capabilities, instructions); plus the era-neutral `tools/list` page parse
  (`McpToolInfo { name, description, input_schema }`, cursor-chained), the
  `tools/call` params builder, and the result mapping: `CallToolResult` →
  the cell/model text (a lone `text` content item is its text verbatim;
  anything else renders as compact JSON so nothing is dropped), `is_error` →
  the red cell, a modern `resultType: "input_required"` (a multi-round-trip
  request this capability-less client can't answer) → a recoverable red
  outcome, and a first `image` content item → a `data:` URL riding
  `ToolOutcome::image` (the `read` tool's channel, so a vision model *sees*
  an MCP image).
- `mcp::sse` — the Server-Sent-Events frame parser (`data:`/`event:` lines,
  multi-line data, comment lines) shared by both HTTP transports; pure over
  `&str` pushes, tested without a socket.
- `mcp::status` — what the UI consumes: `McpServerSnapshot { name, scope,
  config_path, status, url_or_command, auth, server_info, tools }` with
  `McpServerStatus` (`Connected`/`Pending`/`NeedsAuth`/`Failed(reason)`/
  `Disabled`), the status glyph/label mapping — `glyph()` and `label()` kept
  **separate** on both `McpServerStatus` and `McpAuthState` so a renderer can
  colour the glyph by state without splitting a string it didn't build, with
  `status_line()` composing the list row's `✔ connected · 3 tools` (the tool
  count is the *list's*: the detail page has a `Tools:` row of its own and
  must not say it twice) — the **`auth_state` rule** (below), and the
  parameter listing a tool detail page renders from an `input_schema`.

### The `Auth:` row — five truths, not two

Two states made "no stored token" cover both *the server never asked* and
*the login is gone*, so DeepWiki — which has no authentication at all —
reported `✘ not authenticated` beside `✔ connected`. `mcp::auth_state`
derives five, in this order:

| State | Row | When |
|---|---|---|
| `Header` | `✔ authenticated (config header)` | the config carries its own `Authorization` header |
| `Expired` | `✘ expired` | tokens stored **and** the server refusing them |
| `Authenticated` | `✔ authenticated` | tokens stored |
| `NotRequired` | `✔ authenticated` | connected while presenting nothing |
| `NotAuthenticated` | `✘ not authenticated` | remote, no tokens, not serving |

A configured header outranks a stored grant because **the transport does
the same** — it sends the header and never the bearer — so reporting the
grant would offer to re-run and clear a login the server never sees. Only
a stdio server gets no row: there is no remote server to authenticate
*to*. A public server's row is **shown, not hidden**: whether a login is
wanted here is the question the row exists to answer, and silence just
leaves the user wondering. Red is reserved for the two states the user can
act on.

**Why `NotRequired` reports the settled row rather than its own wording.**
It said `◯ not needed` for a while, which is the *precise* truth and the
wrong emphasis: it reads as a caveat hung off `✔ connected`, and the
question the user is actually asking — "am I cleared to use this server?"
— has the same answer here as for a stored grant. So the row says so, and
the **state stays distinct** where distinctness earns its keep: it is what
withholds `Re-authenticate` and `Clear authentication`, because there is
no grant to re-run or delete. Display collapses two states; behaviour does
not.

The actions follow the same derivation: a stored grant (live or expired)
affords `Re-authenticate` + `Clear authentication` — so a dead grant is
removable from the very page that reports it, where it used to be
editable only by hand — a `NotAuthenticated` server affords `Authenticate`
(reachable now from the connected and failed pages too, not only from
needs-auth), and a header or a public server affords neither.

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
    drained until the request's id answers. **Legacy mode** captures the
    `Mcp-Session-Id` response header at initialize and echoes it
    thereafter, with the `MCP-Protocol-Version` header once settled;
    **modern mode** is stateless — the `_meta` block in every request, the
    version header from the first probe (it MUST match the `_meta`), plus
    `Mcp-Method` on every request and `Mcp-Name` on `tools/call` (the
    `header_value` sentinel encoding for non-header-safe names), and no
    sessions. A 4xx whose body parses as a JSON-RPC error surfaces as that
    error (a modern server answers version/header problems as `400` + a
    modern error body), else as the refusal the SSE fallback keys on;
    notifications POST and ignore the body. A **404 to a request that
    carried a session id** means that session is gone (a restart, an idle
    timeout): the spec's answer is a fresh `initialize` at the settled
    revision, a repeated `notifications/initialized`, and a replay of the
    request — so a server recycling its sessions costs a round trip instead
    of the whole connection. The re-handshake's own failure is returned
    intact, because masking it would hide a 401 underneath, which is the
    very signal the token refresh acts on. Gated on having *sent* a session
    id, since a sessionless modern server answers an unknown **method**
    with 404.
  - **legacy HTTP+SSE**: GET opens the event stream on its own thread, the
    first `endpoint` event names the POST target (resolved against the
    base URL), requests POST there and answers arrive on the stream.
  All three poll the turn's `CancelToken` on the 20 ms cadence while
  waiting (the hooks-runner contract — a hung server must not eat Esc) and
  enforce the configured deadlines.
- `client` — the per-server connect sequence, **era detection first** (the
  2026-07-28 spec's own recipe): one `server/discover` probe under the
  modern version (a bounded slice of the connect budget — a server that
  answers *nothing* still leaves room for the handshake). A discover result
  = a modern server: its identity parsed, no initialize, no initialized
  notification, straight to `tools/list`. A **modern error** (`-32020`/
  `-32021`/`-32022` — an allowlist, never one specific code: legacy servers
  answer unknown pre-initialize requests with implementation-defined errors,
  commonly `-32601`) is a modern server that can't do our revision — a
  `-32022` naming a legacy revision we speak falls back to `initialize`
  proposing exactly that; nothing shared fails with both lists in the
  message. **Anything else** — a legacy error code, an HTTP refusal, a dead
  probe — is a legacy server: `set_legacy()` → `initialize` (proposing
  `2025-11-25`, the newest handshake revision) → `notifications/initialized`
  → `tools/list` (cursors drained), the settled revision echoed on later
  requests. **The settled revision is always the server's own answer**:
  `initialize`'s `protocolVersion` on the legacy side (our proposal only
  stands in when a pre-`protocolVersion` server names none), and on the
  modern side the newest revision the discover result's `supportedVersions`
  and ours share — a list that names no revision we can speak modern hands
  off to the handshake at the one it *did* name, which is what asking is
  for. Nothing about the previous connect is consulted. A 401 anywhere
  (or the http→sse fallback both failing with one) resolves as
  **NeedsAuth**, carrying the `WWW-Authenticate` detail for the OAuth
  discovery — auth outranks era, and detection re-runs on the authenticated
  reconnect.

  **The verdict is never remembered.** The spec allows persisting it
  (`SHOULD` cache for the server process's or origin's lifetime, `MAY`
  persist across restarts) and a `{config_home}/mcp-era.json` used to,
  keyed by the server's `target()`. It was wrong, and the way it was wrong
  is the whole reason this paragraph is here: **a dual-era server answers
  the legacy handshake happily.** So a remembered `legacy` verdict never
  failed, never re-probed, and never corrected itself — the server the user
  connected to spoke `2026-07-28`, and the `Protocol:` row read
  `2025-11-25` for good. (The cache's own escape hatch — "a server that
  changed era simply fails once and re-probes" — only fires against a
  modern-**only** server; the servers that actually gain modern support
  keep their handshake, which is exactly what makes them upgradable.)

  And the wrong verdict did not even need a wrong *server*: the fallback
  arm is "anything that is not a modern error", so a probe that timed out,
  met a proxy blip, or reached a server still finishing its boot wrote
  `legacy` just as confidently as one that got `-32601` — permanently,
  from a single unlucky first contact. A cache whose only correction
  mechanism is a failure it has just made impossible is not a cache. The
  era is a fact about the server *now*, so it is re-derived at every
  connect, and the file is swept away at startup
  (`config::remove_retired_mcp_era_cache`) rather than left holding a stale
  revision beside a feature that no longer exists.

  When a revision on the `/mcp` page still looks wrong, the answer is one
  command away and involves no TUI at all: `cargo run --example mcp_probe --
  {url|command}` runs this very connect sequence and prints what the server
  replied — identity, revision, capabilities, tools.

  What that costs is one round trip per connect — sub-millisecond on stdio,
  one RTT on HTTP (measured against three live servers: 0.43 s, 0.69 s,
  0.93 s for the *whole* connect, probe included) — and, against a server
  that **ignores** unknown methods instead of erroring them (which JSON-RPC
  forbids), the `MODERN_PROBE_TIMEOUT` slice before the handshake. That is
  paid on a background worker while the TUI runs, and it buys a revision
  that is true every time it is shown.
- `oauth` — the RFC-shaped authorization-code + PKCE flow: protected-resource
  metadata (RFC 9728) → authorization-server metadata (RFC 8414, with the
  OIDC fallback path) → dynamic client registration (RFC 7591) when offered
  (`application_type: "native"` — the 2026-07-28 requirement whose OIDC
  default of `web` rejects a CLI's loopback redirect) → the authorize URL
  (S256 challenge, `state`, RFC 8707 `resource`, and `offline_access`
  folded into the scopes **only when the AS metadata offers it** —
  SEP-2207's refresh-capable grant, never added blind) → a loopback
  `TcpListener` callback server *and* the paste-the-redirect-URL fallback —
  both accepted, whichever lands first, each validating the RFC 9207 `iss`
  against the discovered issuer (byte-for-byte, absence rejected when the
  metadata advertised the parameter) before the code is redeemed → the
  token exchange. Tokens persist in `{config_home}/mcp-auth.json` keyed by
  server URL (0600, best-effort), and ride every HTTP request as
  `Authorization: Bearer …`. "Clear authentication" deletes the entry.

  Two things the flow must get right or the grant is born unrenewable:
  **`offline_access`** is what actually buys a refresh token, and SEP-2207
  deliberately keeps it out of the *protected resource's* metadata
  ("refresh tokens are not a resource requirement"), so it can only come
  from the authorization server's own catalogue — Vercel is exactly this
  shape (resource `openid`, AS `offline_access`), and without the union its
  grant has nothing to renew. And OIDC only *releases* that refresh token
  when consent is actually shown, so the authorize URL carries
  **`prompt=consent`** whenever the scope asks for it; a silent
  re-authorization returns an access token alone, the same dead end as
  never asking. The scope is the resource's (or, ahead of it, the **401
  challenge's own `scope`**, which the spec makes authoritative for the
  operation refused) plus `offline_access` when the AS advertises it —
  **never** the AS's whole catalogue, which strict providers answer with
  `invalid_scope`, and never `offline_access` alone, which is not a
  resource request.

  **Token refresh — why a session never asks twice** (`refresh_grant` and
  friends; the reference client's own bug class, fixed here the way its
  v2.1.206/211 fixed it):
  - **Proactive**: a stored grant inside the 300-second expiry skew
    refreshes *before* it goes over the wire — at connect
    (`connect_bearer`) and before every tool call (`refresh_if_stale` →
    `Transport::set_bearer`, the mid-session re-arm seam) — so the server
    never sees a dead token in the ordinary course.
  - **Reactive**: a 401 anyway (revocation, clock skew, a grant with no
    known expiry) gets **one forced refresh and one retry** — at connect
    and per call — before anyone is asked to re-authenticate.
  - **Classification is the contract** (`RefreshFailure`,
    `refresh_failure_kind`): only the AS's own structured verdict on a
    400/401 (`invalid_grant` and friends) is **permanent** — the dead grant
    is *cleared* (kept, it would loop the failure into every request) and
    the server truthfully reads `needs authentication`. Network failures,
    5xx, 429, or an unreadable body are **transient**: the grant is kept
    untouched, the server reads `✘ failed (token refresh failed: …)` whose
    `Reconnect` retries — never needs-auth, which would walk the user into
    discarding a working refresh token over a blip.
  - **Rotation**: a returned `refresh_token` replaces the stored one
    (RFC 6749 §6's MUST — the AS may revoke the old one on rotation); an
    absent one keeps the old, never overwriting a live token with nothing.
    The refresh request carries `grant_type`/`refresh_token`/`client_id`
    (+ `client_secret` for a confidential client) and the RFC 8707
    `resource` — and **no scope** (it could only narrow the grant) and no
    PKCE (authorization-code only). The store is re-read before every
    refresh, so another process's newer grant is used instead of burning a
    rotated token.
  - **Single-flight, per server.** Serializing the store's *write* does not
    stop two threads presenting the same refresh token to the **network**,
    and that is the more expensive race: OAuth 2.1 §4.3.1 requires a
    rotating server to detect reuse and revoke the **whole family**, so the
    second presentation doesn't merely fail — it destroys the grant, and
    only the browser brings it back. Two subagents calling one server share
    the manager, so this is reachable in ordinary use (measured: 7 of 8
    concurrent refreshes were rejected as replays before the lock). Every
    refresh takes its server's lock, then **re-reads the store under it**:
    if the access token it found unusable is no longer the stored one,
    another thread already rotated the grant and its result is returned
    instead of a replay. Per server rather than global, so a slow
    authorization server can't hold up an unrelated one.
  - **The store is one file for every server**, so its read-modify-write is
    serialized behind a process-wide lock and committed **write-then-rename**.
    Without the lock, two servers renewing at once (two subagents, or a tool
    call while the loop reconnects another) interleave read/read/write/write
    and drop one of the two rotated refresh tokens; the loser then presents
    an already-used token, which a compliant AS treats as a replay and
    answers by **revoking the whole family** — the browser flow again, for
    the exact reason the refresh exists. Without the rename, a crash
    mid-write leaves a truncated file that parses as *no* servers, silently
    unauthenticating every one of them at once.
  - Every refresh runs **off the tool thread, polling the turn's
    `CancelToken` on the 20 ms cadence** — the transports' own contract,
    which anything blocking a turn's thread must obey. Run inline, a wedged
    token endpoint ate Esc for the whole 20 s HTTP timeout.
  - The 401's **`WWW-Authenticate` challenge is kept** on the server state
    and handed to the flow. Discovery runs blind without it: the challenge
    names the `resource_metadata` URL a server publishes off the well-known
    path, and the scope the resource actually wants.
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

  A connect attempt **starts from nothing**: alongside dropping the old
  transport (which kills a stdio child), it clears the identity, the tools
  and the wire map. Those are the *live* connection's facts — the protocol
  revision, the capabilities, the tool list the detail page shows — so a
  reconnect that never initializes must not leave the previous one's
  behind, under a red `✘ failed` that says the opposite. The model never
  saw them either way (`tool_specs`/`fingerprint`/`has_tools` all filter on
  `Connected`), so what this changes is only whether the page tells the
  truth while a server is down.

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

     Deepwiki - read_wiki_structure(repoName: "linuztx/flaredantic") (MCP)
     Get a list of documentation topics for a GitHub repository.

   Do you want to proceed?
   ❯ 1. Yes
     2. Yes, and don't ask again for Deepwiki - read_wiki_structure commands
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
  footer's cwd — the allowlist is per project). One asymmetry to know: a
  rejection's `User rejected Deepwiki - … (MCP)` text is **recorded** into
  the cell's output (nothing parses it), so a session resumed from before
  the capitalization shows its old lowercase text while the re-derived
  headers around it capitalize. `edit` mode still asks (a
  remote tool is not a file edit), `auto` mode sends an uncovered call to
  the **auto mode classifier** instead of the user — exactly as it does a
  `bash` command, and exactly as the reference feeds MCP calls to its
  auto-mode classifier (`mcpToolInputToAutoClassifierInput`): the classifier
  reads the tool named `{server} - {tool}`, the server's own description,
  and the arguments (`llm::classifier::classifier_request_prompt`'s MCP arm
  — the wire name alone would hide where the risk lives), all under the
  turn's truncated task context (the user request + the actions so far —
  `classifier::ClassifierContext`, `docs/permissions.md`), the allowed call
  runs with the `Allowed by auto mode classifier` note on its record (shown
  in Ctrl+O; the quiet inline `Called {server}` line stays one line — see
  the rendering section below), a denial rejects red with the classifier's
  reason, and a classifier *failure* falls back to this prompt
  (`docs/permissions.md`) — and `master` runs it unasked like everything
  else.

## Rendering — the collapsed inline cell vs the expanded transcript

An MCP cell is recognised by its display name's ` (MCP)` suffix (pure —
survives a `/resume` with no extra record field). Inline it is deliberately
quiet; Ctrl+O carries the full story:

- **Running**: `● Calling {server}… (ctrl+o to expand)` — the breathing
  grey bullet, the hint dim on the header — **and nothing else**. It used to
  carry a `⎿ "{question}"` peek row; a fragment of one argument, wrapped at
  the width, says what the header already said and costs the row.
- **A batch**: when **every** queued call is MCP, the batch is one act and
  shows as one cell — `● Calling Deepwiki 2 times…`, `● Calling Deepwiki,
  Context7 4 times…` (`mcp::names::batch_label`: distinct servers in call
  order, the total when there is more than one) — in the live strip *and*
  above a permission prompt asking about one of its calls, where the
  per-call rendering used to stack a screenful of identical `⎿ Waiting…`
  cells in the rows the question needed. The count is the **batch's**: the
  resolved siblings are read back off the history (`ui::tool::
  mcp_batch_lines`), so a running batch's label doesn't count itself down.
  A mixed batch keeps the ordinary per-cell strip, `⎿ Waiting…` rows and all
  — and per-cell **commits**: its MCP cell is never held (`held_run_len`
  and the flush in `tool_commit_lines` share the one predicate — the call
  that runs *next* is a same-batch MCP call by name), because a cell held
  by one end of the mechanism and flushed by the other printed its
  `Called {server}` line twice (the reported mixed-batch duplicate: the
  bash sibling's commit re-emitted the deepwiki line that had already
  committed at its own ToolEnd).
- **Resolved ok**: the bullet-less dim two-tone
  `Called {server} (ctrl+o to expand)` line — the settled thinking line's
  shape (`summary_lines`), because what is left is a fact about the turn,
  not output to read. The result text never reaches inline scrollback;
  that is *why* the cell can collapse. The auto mode classifier's
  `⎿ Allowed by auto mode classifier` provenance row stays off it too
  (`ui::tool::tool_cell_lines` skips the note for exactly this cell): in
  auto mode **every** server call resolves noted, so the row doubled each
  deliberately-one-line cell into noise — the reported `Called vercel` +
  note pair — while a parallel run's aggregated line never carried it
  anyway; the note still closes the expanded Ctrl+O cell and rides the
  rollout, so the record that no human approved the call survives where
  the full story lives (`docs/permissions.md`). A **parallel run**
  resolves to one such line for the whole run — `Called Deepwiki 2 times
  (ctrl+o to expand)` — because two lines saying `Called Deepwiki`
  describe the batch no better than one that counts it.
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
  column — and when even the first argument does not fit beside the name
  (forty columns leave eight past `● Deepwiki - ask_question (MCP)(`), it
  spills to the continuation row **whole**, the `(` staying with the name,
  rather than hard-breaking as `(repoName` / `: "…"` (`docs/tools.md`).

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
— so the frame shows **no hardware cursor** while the menu is up (the
permission prompt's rule, `ui::cursor_visible`: a menu has nothing for one
to point at, and a kitty cursor animation blinks at whatever seat it picks
— the reported artifact under the bottom rule), the *seat* instead tracking
the highlighted `❯` row (`ui::layout`'s marker scan; the Auth page's
`URL >` field is typed into, so its caret comes back — the amend-field
exception) — ↑/↓ (wrapping at the ends)/digits/Enter/Esc, a selection-centred
window with `↑ N more above` / `↓ N more below` overflow markers, working
mid-turn (the strip stays above it). Pages:

1. **Server list** — `Manage MCP servers` + `{n} servers`, the rows grouped
   under dim scope headings that name the config file
   (`Project MCPs ({root}/.mcp.json)`, `User MCPs (~/.alter-zero/mcp.json)`),
   each row `{name} · {glyph} {status}` (`✔ connected · 3 tools`,
   `△ needs authentication`, `◯ disabled`, `✘ failed`). The ` · `
   separators are **chrome, not status**, so they stay dim at every state
   (`MCP_ROW_SEPARATOR`) and only the glyph carries the state colour —
   riding it along with the glyph painted a connected row's first `·` green
   while the one before its tool count stayed dim, two colours of the same
   mark on one row. An empty list names both file paths — "why isn't my
   server here?" is its only question.
2. **Server detail** — the fact rows the state affords: `Status:`, `Auth:`
   (only when auth matters — the `auth_state` rule above), `Protocol:` (the
   revision the era detection settled — `2026-07-28` on a modern server,
   whatever `initialize` negotiated on a legacy one; hidden until the server
   has initialized), URL-or-command, config location (+ capabilities and
   tool count when connected), then the actions: connected → `View tools` /
   `Re-authenticate` (when OAuth tokens exist) / `Clear authentication` /
   `Reconnect` / `Disable`; needs-auth → `Authenticate` / `Disable`;
   failed → `Re-authenticate` + `Clear authentication` (when tokens exist) /
   `Reconnect` / `Disable`; disabled → `Enable`.
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

### Where the colour goes

The twin borrows the `/hooks` frame but **not** its flat white titles, and
the difference is the shape of the thing: `/hooks` is a browser you read
top-down, while this is a *walk* four pages deep, so the headline is the one
row that answers "where am I?" — and it has to be the row the eye lands on
first. Three rules, all of them `MCP_*` consts in `ui/theme.rs`:

- **Every page's headline is cyan** (`MCP_TITLE_COLOR`, the picker family's
  selection accent) and bold — `Manage MCP servers`, `Deepwiki MCP Server`,
  `Tools for deepwiki`, `ask_question`, `Authenticating with deepwiki…`.
- **A headline capitalises the name** — `deepwiki` → `Deepwiki MCP Server`
  (`mcp::capitalize_server`, the one spelling of the helper). A config key
  is lower-case by convention, which reads as a typo the moment it opens a
  sentence — and the rule now covers **every tool-cell surface** too: the
  `● Deepwiki - ask_question (MCP)(…)` header, the `Calling`/`Called`
  lines (via `batch_label`), and the permission prompt's label
  (`permission::mcp_display_label`). Everywhere the name is an *identity*
  rather than a headline (the `/mcp` list rows, `Tools for …`, the
  `Tool name:`/`Full name:` values, the wire name, the stored
  `ToolCall::name`, the rule keys, the classifier's model-facing prompt) it
  stays verbatim, because those are strings the user — or the replay — has
  to match against a file or a tool call.
- **Every field label on both detail pages is bright, and values are quiet
  by default** (`MCP_DETAIL_LABEL_COLOR` / `MCP_DETAIL_VALUE_COLOR`). The
  labels are the column the eye runs *down*; the value is what it stops on
  once it has found its row, so a page of white values had nothing to scan
  by. Addresses, paths, protocol revisions, counts, a tool's wire name, a
  parameter's type and `(required)` — all quiet.

Two exceptions, one per page, and both are the same rule: *the value that is
itself the answer keeps the light*. The page **headline** is cyan, shared
with the `/hooks` browser (`HOOKS_TITLE_COLOR` = `MCP_TITLE_COLOR`): both
menus are walks several levels deep, and the headline is the row that
answers "where am I?".

- On the **server page** (`MCP_DETAIL_STATE_COLOR`) that is `Status:`,
  `Auth:` and `Capabilities:` — "is this working, and what can it do?" —
  while `Protocol:`, `URL:`/`Command:`, `Config location:` and `Tools:` go
  quiet. The two *state* rows are drawn two-tone: **the glyph keeps its
  state's colour over white words**, because the glyph is the one thing on
  the row that still has to shout when a server is failing, and a flatly
  white row would launder `✘ failed` into something calm. `Status:` also
  drops the tool count `status_line()` appends for the list — the `Tools:`
  row three lines down already says it, and a page that says it twice has a
  duplicate on it.
- On the **tool page** it is the description (`MCP_DESCRIPTION_COLOR`), and
  it gets a tone of its own: **half white**, a step down from the label
  announcing it and a clear step up from the schema prose below. It is the
  one paragraph on the page written *for* a reader rather than derived from
  a schema, so it must not read as boilerplate — but full white made it
  shout over the labels organising the page. The parameter listing keeps the
  default split, with `● name` bright as a label (it is one) and everything
  it introduces dim.

The tool page's field rows also drop the `MCP_FIELD_COL` pad for a single
space (`MCP_TOOL_FIELD_GAP`) — its two labels are the same width, so they
line up on their own and the value sits where the eye already is instead of
across an 18-column gulf. A wrapped parameter hangs under its own name
(`MCP_PARAM_BULLET` / `MCP_PARAM_INDENT`, the same width) and its
continuation rows are dim throughout: only the row that actually carries the
name lights it.

Both pages, annotated:

```text
────────────────────────────────────────────────────────────────

  Context7 MCP Server              ← cyan, name capitalised

  Status:           ✔ connected    ← label white, ✔ green, words white
  Auth:             ✔ authenticated    (and no tool count here)
  Protocol:         2026-07-28     ← label white, value dim
  URL:              https://…      ← label white, value dim
  Config location:  ~/.alter-…     ← label white, value dim
  Capabilities:     prompts, res…  ← label white, value WHITE
  Tools:            2 tools        ← label white, value dim

────────────────────────────────────────────────────────────────

  ask_question                     ← cyan
  deepwiki                         ← dim

  Tool name: ask_question          ← label white, value dim, one space
  Full name: mcp__deepwiki__ask…   ← label white, value dim

  Description:                     ← white
  Ask any question about a GitHub  ← HALF white (the tool's own prose)
  repository…

  Parameters:                      ← white
    ● repoName (required): unknown ← "● repoName" white, the rest dim
      - GitHub repository or list… ← dim (a continuation carries no name)

  Esc to go back                   ← dim

────────────────────────────────────────────────────────────────
```

A page taller than the terminal — a real `query-docs` description plus its
parameter prose easily is — used to clip at the **bottom**, losing the hint
and the closing rule with no sign there was more. It now renders
**bottom-anchored** and its skipped top **flows into the terminal's real
scrollback** directly above the region, so the whole page reads via the
terminal's own scrolling and the interactive tail stays put; navigating away
or closing purge-rebuilds so no stale page text survives
(`docs/view-flow.md`).

Ops dispatch as `Action::McpOp(op)` to `tui::mcp::Session::apply_mcp_op`,
which runs connection work on worker threads and persists disabled state
via the user-file read-modify-write; every completion reports on the MCP
event channel, which re-injects the snapshot (`App::set_mcp_snapshot`) and
schedules a frame — the picker is always looking at live state, and a
pending row shows `… connecting` until its thread reports.

## What stays out (v1)

Resources, prompts, sampling, elicitation, and roots are not modeled — the
tools surface is the feature — and neither is their modern replacement, the
2026-07-28 multi-round-trip request: this client declares an empty
`clientCapabilities` on every request, so a compliant server never sends
`input_required`, and one that does anyway gets the recoverable red outcome
rather than a stalled turn. `subscriptions/listen`, response caching
(`ttlMs`/`cacheScope` parse-through but nothing caches), and CIMD client
registration (we are not a hosted client with an `https` client id — DCR,
deprecated but retained, is our registration path) stay out with it. Server
`instructions` are shown in the detail page but not injected into the system
prompt. WebSocket transport isn't offered (neither reference ships it for
user servers). The dummy backend scripts no MCP scenario — the manager is
real I/O end to end; the smoke suite drives `/mcp` against a scripted
(modern) stdio fixture instead.

## Testing

The pure halves are unit-tested as usual (the naming contract, the argument
pretty-printers, the `_meta`/discover/negotiation shapes, the refresh
classification, every cell/prompt/aggregation rendering, the
commit-vs-repaint agreement), and the boundary against in-process fixtures:
a modern stdio server (discover-first), a legacy one (the -32601 fallback),
a strict modern HTTP server that *rejects* requests missing the 2026-07-28
request metadata (so a green connect proves we send it), a `-32022`
negotiation server, and an OAuth pair — an MCP endpoint demanding
`tok-{n}` beside a scripted token endpoint — driving the whole refresh
matrix (mid-session 401 → refresh → retry with the server never leaving
Connected; proactive pre-call refresh with **zero** 401s seen; connect-time
refresh; `invalid_grant` → cleared grant + needs-auth; a 503 → grant kept +
`✘ failed`, never needs-auth). What
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
one `● Calling Deepwiki 2 times…` cell, the single committed
`Called Deepwiki 2 times`, and the per-call Ctrl+O headers with the model's own
argument order.
