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
  location, `docs/state/project-config.md`), then `{project_root}/.mcp.json` (the
  Claude-Code-compat convention, checked into the repo) — both at the
  nearest-`.git` root (the `AGENTS.md` walk-up,
  `project_doc::find_project_root`), so launching in `repo/src` still finds
  the repo's servers.
- **User**: `{config_home}/mcp.json` (`~/.alter-zero/mcp.json`) — personal
  servers, every project.

**A project server runs only once the user enables it**
(`docs/state/config-review.md`): a checked-in stdio entry is a process this app
would spawn at startup, so a new or changed project server waits for the
**startup review**, which asks — Claude Code's dialog — which of them to
enable. Until it is answered the server is *listed* in `/mcp` as
`⚠ awaiting review`, never launched; a declined one reads `◯ disabled` and
Enable there turns it on (Disable on a project server records the same
decision — in `trust.json`, never in the user file's disabled set). Neither
ever shadows the user's own same-named server. Each decision is pinned to
the server's whole entry, so an edited one is asked about again at the next
launch. This is the one behavioral change to the original two-scope design,
and it is deliberate: `.mcp.json` used to launch unasked, which is exactly
the clone-equals-code-execution hole the hooks doc refused to open.

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
`streamable-http` and `streamableHttp` — other clients' spellings — read as
`http`; a `"type": "stdio"` entry without its `command` says that (it used
to read as an unsupported transport); and an empty, `touch`ed file is an
empty config rather than a red "not valid JSON" at every launch.

**`${VAR}` references are expanded** (`mcp::expand_config`) the way Claude
Code defined them for `.mcp.json`, so a project's file means the same to
both: `${NAME}` and `${NAME:-default}` in every value a launch reads — a
stdio server's `command`, `args` and `env` values, a remote server's `url`
and `headers` values — never in a name or key. A set variable is its value,
an empty one too (unlike POSIX `:-`, which treats empty as unset); an unset
one takes its default, taken as written; a bare `$NAME` is literal, and
there is no escape. A variable neither set nor defaulted keeps its server
from starting, `Failed` with the variable and where it is used named — a
literal `${GITHUB_TOKEN}` sent as a credential, or run as a command, is
never what was meant. The expansion is the manager's, once, from the
session's environment (`ServerState::resolved`), and it is what every
connect, restart and token-store lookup uses; the declared config — what
`/mcp`, `mcp get` and the startup review show, and what the review's
fingerprints are taken over — stays as written, so a secret kept in the
environment stays off the screen and a changed variable is not a changed
server. The review shows the reference itself, `${AWS_SECRET_ACCESS_KEY}`
in a URL included, which is the one place a project's file can ask for a
variable it shouldn't have — and for the variables most worth stealing it
doesn't get to ask: a **project's remote** server may not have the
session's own credentials (every provider's key variable,
`McpSources::protected_env`, filled at the boundary from the provider
table) expanded into its URL or headers (`mcp::resolve_config`); it fails
naming them, never their values. The user's own file is trusted with them,
and a project's *stdio* server inherits the environment anyway, so the
guard is exactly where a value would leave the machine.

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

— the `skills.json` pattern: an empty set drops the entry, a name not
installed here is kept (the same file serves a checkout elsewhere). A
toggle is a read-modify-write of **one name** over the file as it is now
(`record_disabled`, `McpManager::set_disabled`): the old writer stored the
session's whole set, erasing a toggle another session had made in the same
project, and fell back to `{}` over a file that didn't parse — one typo,
and every declared server was gone. A file that doesn't parse is now left
as it is, the toggle holding for the session under a red toast that says
so; a write that fails says so too. Every write of the file — this one and
the CLI's — goes through `llm::mcp::write_config_file`: a temp file beside
the target, synced and renamed into place (a crash mid-write used to leave
an empty file), the file's own permissions kept (owner-only for a new one,
since its headers and `env` can hold tokens), and a symlink followed to the
file it names, so a dotfiles manager's link stays a link.

The **user** file can be edited from the command line — `alter-zero mcp add
{name} --url {url}` (and `add-json`/`remove`/`get`/`list`), resolved before
the TUI boots like `--continue`/`--resume`; see `docs/cli/mcp-cli.md`.

Env knobs: `ALTER_ZERO_MCP` (seeds the `/settings` **MCP** row for the run,
never saved — falsy starts the session with MCP off, *MCP off for the
session* below), `ALTER_ZERO_MCP_FILE` (replaces the
*user* file path; the project files are still discovered),
`ALTER_ZERO_PROJECT_CONFIG` (falsy = no project files at all — the project
layer's own kill switch, shared with hooks, and the smoke suite's
hermeticity mechanism; `docs/state/project-config.md`),
`ALTER_ZERO_MCP_STARTUP_TIMEOUT_MS` (per-server initialize + tools/list
budget, default 30 000 — both references' default),
`ALTER_ZERO_MCP_TOOL_TIMEOUT_MS` (per `tools/call`, default 120 000).

## MCP off for the session — the `/settings` **MCP** row

Disabling a server in `/mcp` is a decision about *that server*, kept per
project in the user file. The `/settings` **MCP** row is the switch above
them all — `true` until a directory turns it off, the **Hooks** posture
(`docs/state/settings.md`, `docs/state/per-directory-state.md`) — and turning it off never
touches a server's own switch. That is the point of it: a session that wants
no MCP right now must not have to disable five servers one by one, and then
remember which five to enable again.

Off means **off**, not hidden (`McpManager::set_active(false)`):

- Every connection is dropped — a stdio child stopped the way a `/mcp`
  Disable stops it, a connect still in flight superseded, a running sign-in
  cancelled.
- Nothing connects again, whoever asks. The startup kick, a Reconnect, an
  Enable, the review's approval and a dead server's automatic restart all
  land on `spawn_connect`, which refuses while MCP is off, so the guard sits
  in one place rather than at every caller. A sign-in is refused with the
  same reason.
- The model is offered no `mcp__*` tool and the reminder carries no server
  instructions: no server is `Connected`, so `tool_specs`, `instructions` and
  the fingerprint the boundary rebuilds on are all empty. A call from a turn
  that began with the tools offered is answered `MCP is off for this session
  — ask the user to turn it on in /settings` rather than the generic
  unknown-tool answer.
- Each server that would have connected reads `◯ off`
  (`McpServerStatus::Off`); one the user disabled stays `◯ disabled`, one
  the startup review is holding stays `⚠ awaiting review`.

On again (`set_active(true)`) is a fresh start, the startup kick's: every
`◯ off` server connects — one that had failed or wanted a sign-in is asked
again — and one that can never start (a `${VAR}` nobody set, a name that
collides with another) reports its reason again.

`/mcp` still opens while MCP is off, the `/hooks` and `/skills` menus' rule,
with a red `MCP is off for this session — turn it on in /settings` under the
list's count — read off the `/settings` row itself, never stored, and never
over an empty list, whose placeholder is the answer there. The rows browse,
and each server's own switch still moves: an `◯ off` server's page repeats
the note — its one action can't turn it back on, so the page says what
will — and offers only **Disable**, and **Enable** on a disabled one leaves
it `◯ off`, to connect once MCP is back on. No Reconnect and no sign-in,
since neither could do anything.

The row reads `false (unavailable)` when no server is declared anywhere (the
**Skills** rule: nothing to connect). The startup review asks nothing about
a project's servers while MCP is off — nothing could run — and turning the
row on over servers that wait for it opens the review's servers page then,
the **Hooks** row's rule (`docs/state/config-review.md`). A config problem the
startup toast would have named is kept quiet while MCP is off and said when
the row turns MCP on.

`ALTER_ZERO_MCP` used to remove the manager outright, so `/mcp` could only
explain itself in a toast and a restart was the only way back. The manager
is built at every startup now — with the row off it launches nothing — so
`/settings` turns MCP on without one. `smoke.sh` Phase 152 drives the row
against a scripted stdio server that marks when it starts and when its stdin
closes, so the phase sees the process stop and come back, not only the rows.

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
  normalization collapses runs — so the split is unambiguous). A wire name
  is a **function name on every request**, and providers refuse the
  *whole* request over one bad name — OpenAI-shaped APIs cap names at 64
  characters (`MAX_WIRE_NAME_LEN`) and every API refuses a duplicate — so
  two more rules hold. A name over the cap keeps its server whole (cut only
  when it alone would leave the tool under eight characters), cuts the
  tool, and ends on `_` plus eight hex digits of the full name's SHA-256
  (`mcp__atlassian-confluence-cloud__confluence_search_usin_8124286d`); and
  `wire_names(server, tools)` — the **one** derivation the specs, the
  fingerprint, the permission prompt's description and the call routing
  all read, so no two can disagree — gives tools that normalize alike
  (`files.read`, `files_read`) or to nothing (`検索`) that same hashed shape
  keyed by the *raw* name, so each stays callable under a name that does
  not depend on the order the server lists them in. Both shapes are fixed
  points of `tool_wire_name`, which is what keeps the replay below exact.
  Two *servers* whose names normalize alike (`my.server`, `my_server`), or a
  name with nothing to normalize to, cannot be told apart that way —
  `server_name_problems` refuses the later of the pair with the reason,
  `✘ failed` in `/mcp`, and a Reconnect does not change that;
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
  `unsupported_versions` + `choose_version` (the `-32022` negotiation) —
  and the legacy side:
  the `initialize` params (`initialize_params_for`, proposing
  `LEGACY_PROTOCOL_VERSION` `2025-11-25` by default) and result parse
  (`ServerIdentity`: the settled `protocol_version`, name, version,
  capabilities, instructions); plus the era-neutral `tools/list` page parse
  (`McpToolInfo { name, description, input_schema }`, cursor-chained), the
  **model's copy of a schema** — every request carries it as the
  function's `parameters`, so one a provider refuses fails every turn:
  `model_input_schema` always roots it at `{"type": "object", "properties":
  {…}}` (both API families insist on the pair), drops `$schema` and a
  `required` that isn't a list of names, and folds a root
  `oneOf`/`anyOf`/`allOf` (refused at the top by Anthropic) into the one
  object — every branch's properties, required where `allOf` requires or
  every alternative does, `FOLDED_SCHEMA_NOTE` at the front of the
  description for alternatives — while `input_schema_problem` names what
  no repair fixes (a root type other than `object`, a parameter name
  outside `^[a-zA-Z0-9_.-]{1,64}$`), and the connect sets such a tool aside
  as an `ExcludedTool` the `/mcp` server page lists under *Not offered to
  the model*; the
  `tools/call` params builder, and the result mapping: `CallToolResult` →
  the cell/model text — text items are their text, in order (a lone one
  verbatim), and binary content is **described, never dumped**: the first
  `image` item becomes a `data:` URL riding `ToolOutcome::image` (the
  `read` tool's channel, so a vision model *sees* an MCP image) under an
  `[image: …]` line, while a second image, `audio` and a blob `resource`
  each leave a one-line placeholder naming the type and size (base64 in the
  text spent the 64 KiB output cap on noise), a `resource_link` says what it
  links to, an embedded text resource is its text, and an unknown item is
  JSON with any long string cut to its length; `structuredContent` stands
  in when nothing else came, and `is_error` → the red cell.
  `parse_incoming` reads what real servers send rather than what JSON-RPC
  says: an id echoed as `"5"` or `5.0`, an `"error": null` beside a result,
  and an error with `id: null` (`Incoming::OrphanError` — the answer to the
  one request in flight; dropped, it left that request waiting out its
  deadline), with a server's own request kept as `Incoming::Request` for
  the transport to answer. **2026-07-28's result rules**:
  `result_type_problem` passes an absent `resultType` (an earlier server)
  and `complete` and calls everything else invalid — a `task` handle reads
  as a red outcome, never as an empty success — and `input_required`
  splits in two: a `requestState`-only answer is a load-shedding server's
  "ask again" (`InputRequired::Retry`), which `call_tool` answers by
  repeating the call with the state echoed (`call_params_retrying`, up to
  three times, the first at once and then 250 ms further apart, inside the
  call's budget), while `inputRequests` ask for elicitation, sampling or
  roots this client never declared (`InputRequired::Unsupported`) and
  resolve as a red outcome that names them. A `-32021`'s
  `requiredCapabilities` ride the error text the model reads.
- `mcp::headers` — the streamable-HTTP request-metadata headers:
  `header_value` (the `Mcp-Name`/`Mcp-Param-{Name}` value encoding, the
  `=?base64?…?=` sentinel around a value that isn't plain printable ASCII,
  has whitespace at an end, matches that sentinel shape, or is empty — an
  intermediary may drop an empty header, every server decodes the
  sentinel) and **`x-mcp-header`** (2026-07-28, SEP-2243: a tool marking
  parameters whose values must also ride the request as headers, for a
  gateway that routes without reading bodies — a MUST for a client on
  streamable HTTP). `param_headers` reads a tool's declarations or refuses
  the tool, the reference TypeScript SDK's rules exactly: an RFC 9110
  token for a name, case-insensitively unique, on a property typed
  `string`, `integer` or `boolean` (no `number`, no type union) that a
  chain of `properties` keys reaches — an annotation under `items`, a
  combinator, a conditional or `$defs` makes the tool invalid.
  `param_header_values` reads each value at its exact path (absent or
  `null` → no header; a non-primitive or an integer past 2^53 − 1 → none
  rather than a malformed one), and `strip_header_annotations` takes them
  out of the model's copy of the schema, schema-aware (a *property named*
  `x-mcp-header` stays).
- `mcp::sse` — the Server-Sent-Events frame parser (`data:`/`event:` lines,
  multi-line data, comment lines) shared by both HTTP transports; pure over
  `&str` pushes, tested without a socket.
- `mcp::status` — what the UI consumes: `McpServerSnapshot { name, scope,
  config_path, status, url_or_command, auth, server_info, tools }` with
  `McpServerStatus` (`Connected`/`Pending`/`NeedsAuth`/`Failed(reason)`/
  `Disabled`/`AwaitingReview`/`Off`), the status glyph/label mapping — `glyph()` and `label()` kept
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

### Servers that list their tools to anyone (Google's) — not supported yet

Google's MCP servers (`drivemcp.googleapis.com`, `calendarmcp.googleapis.com`,
`gmailmcp.googleapis.com`, …) answer the connect and list their tools without
a token, then refuse every call with a 401. Connected that way, one read
`✔ connected` and `✔ authenticated` (`NotRequired`), and the model was
offered tools that could only fail. Their sign-in provider,
`accounts.google.com`, registers no apps by itself — no dynamic
registration, no client metadata documents — so this app can't sign in to
them without an OAuth client the user creates in Google Cloud
(*Pre-registered clients*, below).

**Servers of this kind are not supported yet.** An anonymous connect to one
(remote, no stored grant, no `Authorization` header) is **held**: it reads
`△ needs authentication` and `Auth: ✘ not authenticated`, the connection is
dropped, and none of its tools are offered to the model, the main session's
or a subagent's, so neither are its instructions. The connect worker holds
it when:

- the entry names its own OAuth client (`oauth.clientId`) — a login the
  user asked for, whatever the server lets through; or
- `oauth::protected_without_registration` says so: the server publishes
  protected-resource metadata (RFC 9728, a document whose `resource` is its
  own) naming an authorization server whose metadata offers no
  `registration_endpoint`.

The second check runs on its own thread **beside** the handshake, so a
server that publishes no metadata — DeepWiki, Cloudflare's docs — connects
no slower; each document gets five seconds (`PROBE_TIMEOUT`), and a
document that can't be read never holds a server that connected: only
positive evidence does.

**Why not "publishes OAuth metadata" alone.** That was the first idea, and
it is wrong: Hugging Face (`huggingface.co/mcp`), Context7
(`mcp.context7.com/mcp`) and Exa (`mcp.exa.ai/mcp`) publish the same
metadata and serve calls without a token — sign-in is optional there — so
that rule would cut off three servers that work. Their providers register
apps; Google's doesn't. The registration check is a heuristic, not a
guarantee: a server with optional sign-in through a provider that registers
no apps would be held too. `live_google_servers_wait_for_sign_in_while_open_ones_connect`
(`tests/live_mcp.rs`, ignored by default) checks both sides against the real
servers.

Pressing **Authenticate** on a held server with no client configured ends
with the existing explanation: the provider registers no clients itself,
so add one as `oauth.clientId` or with `alter-zero mcp add --client-id …
--client-secret`. Signing in with such a client is **experimental** — it
follows Google's published metadata (the trailing-slash issuer, the 401
whose metadata URL names the tool rather than the server, the parameters
that get a refresh token) but has not been tried end to end. Fuller support
is future work: a guided client setup on the Authenticate page, Client ID
Metadata Documents for the providers that offer them (Devin's), and a
verified Google sign-in.

`src/llm/mcp/` (boundary, verified hermetically against in-process fixtures —
a scripted `sh` stdio server, `std::net::TcpListener` HTTP servers):

- `transport` — the three transports behind one `request`/`notify` surface:
  - **stdio**: the configured command launched **detached from the TUI's
    terminal** — `exec {command line}`, each word quoted for the shell,
    through `subprocess`'s tier walk (`setsid`, then this binary's helper
    re-exec, `docs/tools/tty-detach.md`), so the server leads a session and
    process group of its own with no controlling terminal: one that runs
    `sudo`, an SSH passphrase prompt or git's credential prompt fails fast
    on `/dev/tty` instead of reading the keystrokes meant for the TUI. A
    command that isn't there is refused by name before the launch (the
    shell would otherwise turn it into an exit after the fact). Env merged
    over the session's, minus the herdr pane id — a server can be an agent
    whose herdr hook would take the TUI's pane over (`docs/architecture/herdr.md`).
    Writes go through a **writer thread** — a server that stops reading
    its stdin (busy in a synchronous tool) costs a call its deadline, where
    a request bigger than the pipe used to block the turn's thread in
    `write_all` for good — and a reader thread turns stdout lines into
    messages, decoded lossily (one line in Latin-1 used to end the
    session) and bounded at 16 MiB. stderr is drained in chunks, never
    lines — a progress bar redrawn with `\r` never ends one, and a reader
    that stopped would leave the server to die of `EPIPE` — into a 4 KiB
    tail that a failed connect folds into its message, read *after* the
    failure (it was read at spawn, and always empty). Requests are
    newline-delimited JSON, answers matched by id; a late answer to a
    request already given up on is dropped, as the spec says, instead of
    kept for good. A request given up on — Esc, its deadline — is
    **cancelled on the server** with `notifications/cancelled` naming it
    (a MUST on stdio, where there is no stream to close; never for
    `initialize`), and a turn already cancelled sends nothing at all. A
    legacy server's own request (`ping`) is answered on stdin; a modern
    client MUST NOT write responses, and doesn't. Stopping a server follows
    the spec: stdin closed, time to exit on its own, `SIGTERM` to its
    process group, then `SIGKILL` — anything it left running in its group
    (`npx`'s real server, a `sleep &`) going with it. A transport dropped
    mid-session (a reconnect, a disable) is stopped off the dropping thread
    with 2 s per step; a quit stops every server at once, 300 ms per step
    and one bound for all (`McpManager::shutdown`), so a quit waits for a
    well-behaved server's exit and never for a stuck one.
  - **streamable HTTP**: POST per message (`Accept: application/json,
    text/event-stream`), the response either a JSON body or an SSE stream
    drained until the request's id answers. **Legacy mode** captures the
    `Mcp-Session-Id` response header at initialize and echoes it
    thereafter, with the `MCP-Protocol-Version` header once settled;
    **modern mode** is stateless — the `_meta` block in every request, the
    version header from the first probe (it MUST match the `_meta`), plus
    `Mcp-Method` on every request, `Mcp-Name` on `tools/call` (the
    `header_value` sentinel encoding for non-header-safe names) and the
    tool's `Mcp-Param-{Name}` mirrors, and no sessions. The connect's
    tools listing (`client::list_tools`, shared by both eras) refuses a tool
    whose `x-mcp-header` annotations break the rules — an `ExcludedTool`
    like any other, so it costs only itself — and arms the transport with
    the rest's declarations; a call the server still refuses `400
    HeaderMismatch` (`-32020`) lists the tools again, records the fresh
    listing, and is retried once, the spec's SHOULD (a changed schema is
    the usual cause). A 4xx whose body parses as a JSON-RPC error — its
    id or `id: null`, the SDKs' transport-level shape — surfaces as that
    error (a modern server answers version/header problems as `400` + a
    modern error body); **only 400, 404 and 405** without one are the
    refusal the SSE fallback keys on (`TransportError::HttpRefused` — the
    spec's three), while a 403, a 429 or a redirect not followed is the
    server's answer, reported as such, and a **403 whose challenge says
    `insufficient_scope`** is an authentication state like a 401
    (`NeedsAuth`, the challenge kept for the step-up). A notification's 2xx
    is its acceptance whatever the body, which is never read — a server
    holding it open must not hold up the handshake.

    Every POST runs on a worker with **its own request's budget as the
    HTTP timeout** (`RequestBuilder::timeout`, which in reqwest's blocking
    client bounds the wait for the headers *and* each body read). The
    pooled client's 60 s stall guard used to be the effective limit, so a
    tool taking 61–120 s failed on HTTP while the same tool worked on stdio,
    and raising `ALTER_ZERO_MCP_TOOL_TIMEOUT_MS` did nothing. A wait that
    gives up (Esc, the deadline) marks the POST abandoned and the worker
    drops the response at its next line — closing the response stream *is*
    streamable HTTP's cancellation. Each message is bounded at 16 MiB
    (`MAX_MESSAGE_BYTES`, Claude Code's bound) and read with lossy UTF-8.
    The client follows a **redirect only within the request's origin**
    (`llm::mcp_http_client`, ten hops): every MCP request carries the
    config's headers, which reqwest — stripping only `Authorization` and
    cookies across hosts — would otherwise hand to wherever a server
    bounced it.

    On a response stream, a **legacy** server may send its own request
    (`ping`, mostly) and wait for the answer before its result: answered
    at once — `ping` with `{}`, anything else `-32601`
    (`mcp::server_request_answer`) — by a POST of its own, session id and
    all; unanswered, the call sat out its whole deadline. A modern stream
    that **ends before its answer** loses the request — 2026-07-28 removed
    resumption and says the client MUST re-issue it under a new id — so it
    is re-issued once. A session id is captured only in legacy mode: a
    modern answer naming one anyway is never echoed back. A **404 to a request that
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
    base URL — and **refused when it names another origin**, host, port or
    scheme: every POST carries the bearer and the configured headers, so a
    stream pointing elsewhere, or downgrading `https` to `http`, would hand
    them over; the reference SDK refuses the same), requests POST there on
    a worker thread under the same deadline and cancel polls as every other
    wait (it used to block the turn's thread), and answers arrive on the
    stream. A `ping` the server sends on the stream is answered with a
    POST. A 401 or step-up 403 keeps its `WWW-Authenticate` challenge,
    which the POST side used to drop. The stream thread stops at its next
    line once the transport is dropped — a keep-alive comment is a line —
    where it used to hold the connection, and itself, for good. A
    byte-order mark opening the stream is stripped (the SSE spec's rule;
    kept, it made the `endpoint` event never arrive).
  All three poll the turn's `CancelToken` on the 20 ms cadence while
  waiting (the hooks-runner contract — a hung server must not eat Esc) and
  enforce the configured deadlines; a notification takes the caller's
  deadline and cancel too, where it used to wait up to 60 s on a token
  nothing could cancel.
- `client` — the per-server connect sequence, under **one deadline for
  the whole sequence** (the probe, the handshake, every page of the
  listing and an SSE fallback all draw on it — each request used to restart
  its own clock, so a slow, many-paged server took page count × budget),
  **era detection first** (the
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
  → `tools/list` (cursors drained — an empty-string cursor continues the
  listing, as 2026-07-28 requires, and the drain stops at **any** cursor
  it has already sent, not only an immediate repeat, or at one over
  64 KiB; a name repeated across pages is listed once), the settled
  revision echoed on later
  requests. **The settled revision is always the server's own answer**:
  `initialize`'s `protocolVersion` on the legacy side (our proposal only
  stands in when a pre-`protocolVersion` server names none), and on the
  modern side the newest revision the discover result's `supportedVersions`
  and ours share — a list that names no revision we can speak modern hands
  off to the handshake at the one it *did* name, which is what asking is
  for. Nothing about the previous connect is consulted. A bare-`url`
  config whose `initialize` is refused (`Handshake::Refused`) tries the
  legacy transport, and when that fails too the message keeps **both**
  answers, the streamable one first — it is usually the one that explains.
  A 401 anywhere
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
  callback server *and* the paste-the-redirect-URL fallback — both
  accepted, whichever lands first → the token exchange.

  **The response is checked before anything is redeemed**
  (`loopback::check_auth_response`, the same rules for the callback and a
  paste):
  the `state` is **required** and must be this flow's — a response without
  one can't be told from a forged one (RFC 6749 §10.12), where it used to
  be accepted — then the RFC 9207 `iss` must name the discovered issuer
  (byte-for-byte, absence rejected when the metadata advertised the
  parameter), and only then does the content count. A response that fails
  either check is **turned away and the wait goes on**: a stray request, a
  previous attempt's redirect or a forged one can't end the sign-in (the
  browser gets a 400 page saying so; a paste gets a reason on the auth page
  through `AuthProgress::on_paste_rejected`). One that passes and carries
  `error=` — the user declining on the consent screen — **ends the
  sign-in with the authorization server's own reason**, where it used to
  leave the flow waiting five minutes for a code that would never come.

  **The callback server** is `llm::loopback`, the one every browser
  sign-in shares — this flow, ChatGPT's and the Anthropic Console's, which
  carried the same hand-rolled loop and the same weaknesses
  (`CallbackServer::bind` over a list of ports, `0` for an ephemeral one,
  and a path, `wait_for_code` over the flow's `Expected` state and issuer,
  its `Pages` wording and an optional paste channel). It listens on
  `127.0.0.1` **and** `[::1]` on the same port, since the redirect names
  `localhost` and a browser may resolve it to either (IPv4 alone where the
  machine has no IPv6 loopback). Each connection is served on a thread of
  its own (`serve_connection`, at most `MAX_CONNECTIONS` at once) with a
  read timeout, reading the
  whole request head before it answers: the old loop read each connection
  inline with no timeout, so a browser **preconnect** — a socket opened and
  never written — blocked the wait, Esc and the paste field with it, until
  the browser gave up on the socket; and on macOS an accepted socket
  inherits the listener's non-blocking mode, so a request still on its way
  read as empty and its code was lost. Anything but `GET /callback` is a
  404, and the page the browser lands on says what actually happened —
  received, refused (the authorization server's own words set apart), or
  not this sign-in's — in the project's own dress: the mascot in pixels
  over a serif headline, its colours and motion saying how it went, every
  word from outside escaped and nothing fetched (`docs/providers/callback-page.md`).
  The page carries its own icon, so the browser no longer asks the callback
  for `/favicon.ico`.

  **The client** follows SEP-2352: client credentials are kept with the
  issuer that registered them (`StoredTokens::issuer`; a grant stored before
  that is matched by its token endpoint) and are **never presented to
  another authorization server** — a server whose metadata now names a new
  one registers afresh (`reusable_client`). A reused client signs in on the
  port it registered (`StoredTokens::redirect_uri`) when that port is free,
  since an authorization server may match the registered redirect exactly —
  RFC 8252's loopback port leniency is a SHOULD — and **registers again**
  when it isn't, where the old flow sent a redirect the server had never
  seen and left the user on the server's error page. Registration asks for
  `token_endpoint_auth_method: "none"` unless the metadata lists methods
  without it, then for the secret method it offers (`client_secret_basic`
  first, RFC 6749's default); a client registered with
  `client_secret_basic` authenticates with HTTP Basic, neither half in the
  body (`client_authentication`, the exchange and every refresh alike).
  A refused registration or exchange carries the server's own `error` and
  `error_description` (`oauth_error_detail`) rather than a bare status, and
  a server offering no registration says how to add a pre-registered
  client (below).

  **Pre-registered clients.** Some authorization servers register no
  clients at all — Google's, for its MCP servers (`calendarmcp.googleapis.com`
  and friends): the user creates an OAuth client in the provider's console
  and the MCP client signs in as it. A remote entry names it in the `oauth`
  object other MCP clients write (`mcp::OAuthClientConfig`):

  ```json
  "calendar": {
    "type": "http",
    "url": "https://calendarmcp.googleapis.com/mcp/v1",
    "oauth": { "clientId": "123.apps.googleusercontent.com",
               "callbackPort": 8976,
               "scopes": ["https://www.googleapis.com/auth/calendar"] }
  }
  ```

  `clientId` is the client; `callbackPort` the port its registered redirect
  URI names (`http://localhost:{port}/callback`) — bound **exactly**, the
  sign-in failing with the port named when it is taken, since the provider
  was told that one (a native/desktop client, which takes any loopback port
  per RFC 8252, can leave it out); `scopes`, a string or a list, is asked
  for as written in place of what discovery suggests. The secret is best
  kept out of the file: `alter-zero mcp add … --client-id … --client-secret`
  keeps it in the token store's `clients` section (`save_client_secret` —
  owner-only, apart from the login so "Clear authentication" leaves it), and
  a `clientSecret` in the entry, `${VAR}`-expanded like the rest of it, is
  read too — under the same provider-key guard, which covers the `oauth`
  values of a project's remote server. `run_auth_flow` (over a `SignIn`:
  the server, its challenge, the store, this client) uses a configured
  client ahead of any reused or registered one and never registers;
  authenticates it as a public client without a secret, or with one in the
  form body unless the server takes only HTTP Basic
  (`configured_auth_method`); and stores the id and secret with the grant,
  so a refresh presents them too. An authorization server with needs of its
  own gets them on the authorize URL (`provider_authorize_params`): Google
  issues a refresh token only for `access_type=offline` and re-issues one
  only with `prompt=consent`, or the login lapses within the hour. A
  server whose entry names a client waits for that login even when it let
  an anonymous connect through (*Servers that list their tools to anyone*,
  above), and signing in to Google's this way is **experimental**: it
  follows Google's published metadata, but has not been tried end to end.
  The review's `ServerParts` names an `oauth` change (`the OAuth client`).

  **The flow stops where it should.** The cancel token is checked between
  discovery, registration and the browser, so Esc during discovery never
  opens a browser afterwards; with no config home to keep the grant in, the
  flow says so before the browser opens; and the PKCE verifier and `state`
  come from the system's random source or the flow fails
  (`pkce_and_state`) — the old code fell back to zeroed bytes, a guessable
  state and a decorative PKCE. Tokens persist in `{config_home}/mcp-auth.json` keyed by
  server URL — the file created owner-only (0600) through a temp file of
  its own, never written first and narrowed after, in a 0700 directory
  when the save creates one — and ride every HTTP request as
  `Authorization: Bearer …`. A grant that can't be stored fails the
  sign-in instead of "succeeding" into nothing (no config home, a store
  that can't be written), and a store that exists but can't be read is an
  error, never "empty" — rewriting it would drop every other server's
  grant. "Clear authentication" deletes the entry, and says so only when
  the delete was written.

  **Every discovered document is checked before it is trusted** — the
  2026-07-28 authorization spec's MUSTs, and the reference TypeScript SDK's
  reading of them:
  - The **resource metadata** must be *this* server's (RFC 9728 §3.3,
    `resource_allowed`: same origin, the server's path under the
    document's `resource`); one for another resource is skipped, since it
    would point the flow at someone else's authorization server. Its
    `resource` is the RFC 8707 indicator the flow sends — authorize, token
    exchange **and** every refresh (kept on the grant as
    `StoredTokens::resource`, so a refreshed token's audience never drifts);
    without resource metadata, the canonical server URL (`canonical_resource`:
    lowercase scheme and host, no fragment, no lone `/`).
  - The **authorization-server metadata** is looked for exactly where the
    spec says (`authorization_server_candidates`): for an issuer with a
    path, RFC 8414 path insertion, OIDC path insertion, then OIDC path
    **appending** — where Keycloak realms and Entra tenants publish, and
    which was never tried — and never the root, which for a path issuer is
    some other issuer's document; and that document's `issuer` must be the
    issuer its URL was built from (RFC 8414 §3.3, `issuer_matches`, with the
    SDK's one trailing-slash tolerance) or it is skipped. That validated
    issuer is the one RFC 9207's `iss` is then held to.
  - Its endpoints must be **HTTPS** (`endpoint_is_secure`; loopback exempt
    for a server under development), and it must support **PKCE with
    S256**: metadata listing `code_challenge_methods_supported` without it
    is refused, while metadata listing none is let through as the SDK does —
    the spec's "MUST refuse" would turn away servers that support PKCE and
    merely don't say so (`pkce_problem`).
  - A `WWW-Authenticate` challenge is parsed as RFC 9110 auth-params, the
    parameter matched **by name** (`challenge_param`): the old substring
    search found `scope` inside `insufficient_scope`, so the step-up
    challenge the spec itself shows carried no scope.
  - A re-authorization asks for the **union** of the scopes granted before
    and the ones the server now wants (`scope_union`, the step-up rule) —
    a `403 insufficient_scope` arrives as `NeedsAuth` with its challenge,
    and Authenticate on `/mcp` then asks for exactly that.

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
  - **Proactive**: a stored grant inside the 300-second expiry skew — or,
    for a token living less than ten minutes, inside the second half of
    its life (`issued_at` is kept beside `expires_at`; a five-minute token
    was otherwise stale the moment it was minted, every request a renewal
    and every renewal a rotation) — refreshes *before* it goes over the
    wire — at connect
    (`connect_bearer`) and before every tool call (`refresh_if_stale` →
    `Transport::set_bearer`, the mid-session re-arm seam) — so the server
    never sees a dead token in the ordinary course.
  - **Reactive**: a 401 anyway (revocation, clock skew, a grant with no
    known expiry) gets **one forced refresh and one retry** — at connect
    and per call — before anyone is asked to re-authenticate.
  - **Classification is the contract** (`RefreshFailure`,
    `refresh_failure_kind`): only the AS's own structured verdict on a
    400/401 naming the **grant itself** — `invalid_grant`,
    `invalid_client`, `unauthorized_client`, `unsupported_grant_type`,
    `invalid_scope` — is **permanent**: the dead grant is *cleared* (kept,
    it would loop the failure into every request) and the server
    truthfully reads `needs authentication`. Everything else is
    **transient** — network failures, 5xx, 429, an unreadable body, and
    the errors that say nothing about the grant (`invalid_request`, an
    AS's own `temporarily_unavailable`, an error code nobody has heard of),
    which used to clear a grant that may have been perfectly good: the
    grant is kept
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
    rotated token — and the outcome is written back **only over the grant
    the refresh presented** (`settle_refresh`, a compare-and-swap on the
    refresh token): a grant "Clear authentication" deleted while the
    request was out stays deleted (the clear wins), and a newer grant a
    sign-in stored meanwhile stays and is what the caller gets, so a
    permanent rejection can never clear a grant it didn't reject.
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
    authorization server can't hold up an unrelated one. And **per session
    too**: two sessions in different terminals share one store, and the
    replay is as fatal when the second presenter is another process, so the
    renewal also holds an exclusive `flock` on
    `mcp-auth.locks/{digest}.lock` across its whole token request (released
    on drop, and by the kernel if the process dies, so a crash never
    strands it). A second session waits for it — up to `REFRESH_LOCK_WAIT`,
    past the first one's token request — then re-reads and takes the grant
    that session minted.
  - **The store is one file for every server**, so its read-modify-write is
    serialized behind a process-wide lock **and** a `flock` on
    `mcp-auth.lock` (another session's write is waited for, never past a
    stuck one — a dropped rotation costs more than a raced one) and
    committed **write-then-rename**, through a temp name unique to the
    write so two processes saving at once never write into each other's
    file.
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
  `parameters` being `mcp::model_input_schema`'s repaired copy of the
  server's `inputSchema`, below), executes
  `call_tool` (wire name → server + tool via the connect-time map), and
  reports every state change on the loop's **MCP event channel** (a
  `tokio` unbounded sender — the file-search/clipboard worker pattern), so
  the UI is repainted the moment a server connects, fails, or finishes
  authenticating.

  A connect attempt **starts from nothing**: alongside dropping the old
  transport (which stops a stdio child), it clears the identity, the tools
  and the set-aside tools (`ServerState::drop_connection`, the one teardown
  a disable, a needs-auth refusal and a fresh connect share). Those are the
  *live* connection's facts — the protocol revision, the capabilities, the
  tool list the detail page shows — so a reconnect that never initializes
  must not leave the previous one's behind, under a red `✘ failed` that
  says the opposite. The model never saw them either way
  (`tool_specs`/`fingerprint`/`has_tools` all read `ServerState::offered`,
  which is empty unless `Connected`), so what this changes is only whether
  the page tells the truth while a server is down.

  `call_tool` guards the connection it runs on. Two calls to one server
  share it, so the second **waits for the first polling the turn's
  cancel** (`lock_unless_cancelled`) — it used to take the lock blind, and
  a call the user had pressed Esc on still went out once the lock was
  free. A `401` tears down only **the connection the call went out on**
  (its generation): a reconnect that landed meanwhile is a newer one. And a
  call that finds its connection **dead** — a stdio server that exited, a
  legacy stream that ended (`Transport::is_alive`) — marks the server
  failed with the reason and **restarts it** at once (the stdio spec's
  "SHOULD restart"), telling the model to try again; it used to sit
  `✔ connected` over a dead pipe, every later call failing until the user
  reconnected by hand. At most once per 30 s (`RESTART_COOLDOWN`): a server
  that dies again that soon stays failed for a manual Reconnect, never a
  crash loop. **Clear authentication** reconnects a connected (or failed)
  server once the grant is gone, since the live connection still carried
  the old bearer and the server kept working on a login the page said was
  cleared.

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
  offered tools — each wire name beside a digest of its description and
  schema, so a reconnect that changed only a schema (a stdio server's own
  dev loop) counts too — at every MCP event and turn start
  (`Session::refresh_mcp`, the `skills_attached` pattern) and rebuilds the
  backend only when it actually moved.
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
  `classifier::ClassifierContext`, `docs/state/permissions.md`), the allowed call
  runs with the `Allowed by auto mode classifier` note on its record (shown
  in Ctrl+O; the quiet inline `Called {server}` line stays one line — see
  the rendering section below), a denial rejects red with the classifier's
  reason, and a classifier *failure* falls back to this prompt
  (`docs/state/permissions.md`) — and `master` runs it unasked like everything
  else.

## Rendering — the collapsed inline cell vs the expanded transcript

An MCP cell is recognised by its display name's ` (MCP)` suffix (pure —
survives a `/resume` with no extra record field). Inline it is deliberately
quiet; Ctrl+O carries the full story:

- **Running**: `● Calling {server}… (ctrl+o to expand)` — the blinking
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
  the full story lives (`docs/state/permissions.md`). A **parallel run**
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
  rather than hard-breaking as `(repoName` / `: "…"` (`docs/tools/tools.md`).

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
the highlighted `❯` row (`ui::layout`'s marker scan — and on a page with no
`❯` to track, the empty list or the tool detail, resting just past the
closing hint the way the Ctrl+O/Ctrl+D/`/resume` overlays do
(`overlay_cursor_seat`'s rule), never on the framing rule under it: the
far corner of the bottom rule is where an unconfigured `/mcp` used to send
a kitty cursor animation on every open, `docs/ui/view-flow.md`; the Auth
page's `URL >` field is typed into, so its caret comes back — the
amend-field exception) — ↑/↓ (wrapping at the ends)/digits/Enter/Esc, a selection-centred
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
   `Reconnect` / `Disable`; connecting → `Reconnect` / `Disable`; disabled →
   `Enable`; off (MCP off for the session, above) → `Disable`. When a fresh snapshot changes the list under the user the
   **selection follows its action**, not its index — a Reconnect turned the
   server `connecting…`, whose list was `[Disable]` alone, so a second Enter
   disabled the server and saved that — and a connecting server's first
   action is the harmless one. A remote server's `URL:` row is a link to
   its address, every wrapped row opening the whole URL, and a URL in a
   failed server's `Error:` reason is a link too (`docs/ui/links.md`).
3. **Tools list** — `Tools for {server}` over numbered tool names.
4. **Tool detail** — the tool name + server, `Tool name:` / `Full name:`
   (the wire `mcp__{server}__{tool}` the model actually calls),
   the description, and the `Parameters:` listing derived from the
   `input_schema` (`● {name} (required): {type} - {description}`).
5. **Authenticate** — `Authenticating with {server}…`: the flow opens the
   browser (`open` on macOS, `xdg-open` elsewhere — never `open` on Linux,
   which is `openvt` — the launcher reaped on a thread of its own), shows
   the authorize URL (`c` copies it via the `/copy` clipboard seam), and
   offers the `URL >` paste-the-redirect fallback (a single-line field;
   paste routed to it while the page is up); the loopback callback
   resolves it from either side. A paste the flow turns away — not a
   redirect, or another attempt's — comes back as
   `McpEvent::AuthPasteRejected`, its reason red under the field, which
   takes another paste (`App::reject_mcp_auth_paste`; editing clears it).

   **The authorize URL is a real hyperlink**, as the `/login` sign-in
   pages' is (`docs/ui/links.md`). It is far wider than any row — the PKCE
   challenge, the state, the encoded redirect and resource — so it always
   hard-breaks, and it used to be printed as plain rows: a terminal's own
   URL detection reads row text, so a click opened the first row's
   truncated URL and the rows under it were no links at all. Every row now
   carries the whole URL as its OSC 8 target, **verbatim**
   (`model_url_rows`): the click opens exactly what `c` copies and the
   browser was handed, whatever character the URL ends on. It keeps its
   accent and gains an underline; a URL inside a rejected paste's reason is
   linked the same way. `smoke.sh` Phase 149 reads the link off the raw
   byte stream against a stub server that demands OAuth.
   Esc backs out; the result lands as a toast + the server list
   re-sorting itself.

   **One sign-in at a time, and each knows whether it still is the one.**
   The manager keeps the running flow as an `AuthFlow` with an id; a second
   Authenticate cancels the first and installs its own. The old flow, when
   it ends, clears only its own entry: it used to clear whatever was
   installed — the *new* flow's paste channel and cancel token, so the new
   page's paste went nowhere and its Esc stopped nothing — and reported
   `cancelled` as a red failure that closed the new page. A flow that ends
   cancelled (Esc) or superseded reports nothing but a snapshot refresh,
   and `App::finish_mcp_auth` closes only the page of the server whose flow
   finished. `McpSources::open_browser` injects the launcher, so a test
   signs in without one.

### Where the colour goes

The twin borrows the `/hooks` frame but **not** its flat white titles, and
the difference is the shape of the thing: `/hooks` is a browser you read
top-down, while this is a *walk* four pages deep, so the headline is the one
row that answers "where am I?" — and it has to be the row the eye lands on
first. Three rules, all of them `MCP_*` consts in `ui/theme.rs`:

- **Every page's headline wears the pages' one title dress** — bold, washed
  in the active theme's banner gradient (`ui::header::page_title_line`,
  `docs/ui/theme.md`) — `Manage MCP servers`, `Deepwiki MCP Server`,
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
  by default** (`mcp_detail_label_color()` / `mcp_detail_value_color()`). The
  labels are the column the eye runs *down*; the value is what it stops on
  once it has found its row, so a page of white values had nothing to scan
  by. Addresses, paths, protocol revisions, counts, a tool's wire name, a
  parameter's type and `(required)` — all quiet.

Two exceptions, one per page, and both are the same rule: *the value that is
itself the answer keeps the light*. The page **headline** wears the title
dress every page shares, the `/hooks` browser's included: both menus are
walks several levels deep, and the headline is the row that answers "where
am I?".

- On the **server page** (`mcp_detail_state_color()`) that is `Status:`,
  `Auth:` and `Capabilities:` — "is this working, and what can it do?" —
  while `Protocol:`, `URL:`/`Command:`, `Config location:` and `Tools:` go
  quiet. The two *state* rows are drawn two-tone: **the glyph keeps its
  state's colour over white words**, because the glyph is the one thing on
  the row that still has to shout when a server is failing, and a flatly
  white row would launder `✘ failed` into something calm. `Status:` also
  drops the tool count `status_line()` appends for the list — the `Tools:`
  row three lines down already says it, and a page that says it twice has a
  duplicate on it.
- On the **tool page** it is the description (`mcp_description_color()`), and
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
(`docs/ui/view-flow.md`).

Ops dispatch as `Action::McpOp(op)` to `tui::mcp::Session::apply_mcp_op`,
which runs connection work on worker threads and persists disabled state
via the user-file read-modify-write; every completion reports on the MCP
event channel, which re-injects the snapshot (`App::set_mcp_snapshot`) and
schedules a frame — the picker is always looking at live state, and a
pending row shows `… connecting` until its thread reports.

## What stays out (v1)

Resources, prompts, sampling, elicitation, and roots are not modeled — the
tools surface is the feature — and neither is their modern replacement, the
2026-07-28 multi-round-trip request's `inputRequests`: this client declares
an empty `clientCapabilities` on every request, so a compliant server only
ever sends it a `requestState`-only `input_required` (retried, above), and
one asking for input anyway gets the recoverable red outcome naming what it
asked for rather than a stalled turn. Response caching (`ttlMs`/`cacheScope`
parse-through but nothing caches) and CIMD client registration (we are not a
hosted client with an `https` client id — DCR, deprecated but retained, is
our registration path) stay out with it.

**A server that says its tools changed is listed again.** A
`notifications/tools/list_changed` reaches the manager from wherever the
transport reads the server (`Transport::watch_changes`, a `ChangeSink` each
connect registers): the stdio reader thread and the legacy SSE stream
thread see every message whether or not a call waits, and a streamable-HTTP
response stream hands over any notification it carries. A server whose
capabilities promise changes (`tools.listChanged`,
`ServerIdentity::tools_list_changed`) is also asked for them, since a
modern one MUST NOT send what it was not asked for: on stdio a
`subscriptions/listen` with the `toolsListChanged` filter is written once
and never waited on (its eventual answer is a late reply the request loop
drops); over streamable HTTP a background thread holds the change stream
open (`listen_for_changes`) — the `subscriptions/listen` response stream on
a modern server, the optional GET stream on an earlier one — reopening it
when it ends with a growing pause, giving up on a server that offers none
(400/401/403/404/405, or an answer that isn't an event stream) or fails
every opening in a row, reading each reopening's bearer and session off the
connection as the last request left them, and stopping with the transport.
The manager then lists the server's tools again on a worker
(`McpManager::relist`) — never on the reader thread, and never under the
manager lock while it waits for a call holding the connection — coalescing a
burst of notifications into one listing plus one more for any that arrived
while it ran, and keeping the old tools if the listing fails. The fresh
tools replace what the model is offered, the boundary re-attaches the
backend when the set moved, and the change reaches the next request. The
sink holds the manager weakly: the transport lives inside the manager. Server
`instructions` — the spec's hint for how a server's tools are meant to be
used — ride the `<system-reminder>` as its **last section**
(`mcp::instructions_section`, the reference client's shape: `# MCP Server
Instructions` over one `## {server}` part each), built at the boundary by
`sync_listings` from `McpManager::instructions` and re-rendered on every MCP
event, so a server that connects after the first frame is in the next
turn's context. Only a connected server with at least one tool the model is
offered contributes (its instructions are about tools; with **Tools** off
none are offered, so none ride), in name order so the reminder holds still
across reconnects, each cut at `MAX_INSTRUCTIONS_CHARS` (4,096). Last,
because servers come and go and only what follows a changed section
re-renders into the prompt cache. The text is the server's, so it is held
inside its part: every line that starts with a heading's `#`s is pushed two
levels down — inside a code fence too, since a reader of the raw text can't
tell a fenced `# …` from a section, while a `#` comment stays a comment —
and a fence left open is closed, so no server can open a section of its own
or pose as another server's. A subagent's briefing carries the parts of the
servers whose tools its type may call (`briefing_for`, its `tools:`
allowlist), since the lead's reminder never reaches its fresh context.
`/context` counts the section as **MCP instructions**, one part per server
(`docs/architecture/context-usage.md`). WebSocket transport isn't offered: the MCP spec defines stdio and
Streamable HTTP (and the legacy HTTP+SSE this client also speaks), and
Claude Code's `"type": "ws"` is its own extension, which an entry here
reports as an unsupported transport rather than dropping silently. The dummy backend scripts no MCP scenario — the manager is
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
