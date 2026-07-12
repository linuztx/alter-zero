# LLM tool calling — `bash` / `read` / `write` / `edit`

The real backend can now **call tools**: the model asks to run a shell command,
read a file, write a file, or edit one; the backend executes it locally, streams
the call into the TUI exactly like the dummy's scripted tools, feeds the result
back to the model, and loops until the model answers with plain text. This turns
the app from a chat client into a small coding agent — the capability
`docs/llm.md`'s "Known limitations" listed as future work ("No streaming
tool-call support from the model yet").

This document covers the wire format, the agentic loop, the executor, and the
rendering. It complements `docs/llm.md` (the backend seam) and
`docs/shell-command.md` (the `!` local shell, which is unrelated — that is the
*user* running a command, this is the *model*).

## Why Chat Completions function-calling (not codex's Responses/apply_patch)

openai/codex uses the **Responses API** and a freeform `apply_patch` **grammar**
tool. Neither fits here: inline-tui talks the **Chat Completions** API
(`/chat/completions`) for broad OpenAI-compatible/OpenRouter coverage, and a
Lark-grammar custom tool only works on the handful of models that support it.

So we use **standard Chat Completions function-calling** — the shape every
OpenAI-compatible provider implements — with four **JSON-schema function tools**
modelled on Claude Code's toolset (the names the user asked for). Codex still
informs the design: its tool *descriptions*, its execution *semantics* (10 s
default shell timeout, byte-capped middle/head-truncated output, `Exit code:` +
output framing), and its TUI *diff cells* (`• Edited path (+N −M)`, green adds /
red deletes) are ported.

## The four tools

All four are declared to the model as
`{"type":"function","function":{"name","description","parameters":<schema>}}`
entries in the request's `tools` array, with `tool_choice:"auto"`. The pure
definitions + JSON schemas live in [`llm::tools`](../src/llm/tools.rs)
(`tool_specs()`), unit-tested for shape.

| tool | params | executes |
| --- | --- | --- |
| `bash` | `command` (req), `timeout_ms` (opt, default 30 000, cap 600 000) | `sh -c command`, stdin `/dev/null`, stdout+stderr captured, byte-capped, killed on timeout/cancel |
| `read` | `path` (req), `offset` (opt 1-based line), `limit` (opt, default 2000 lines) | read the file, return `cat -n`-style numbered lines (so the model can cite line numbers to `edit`) |
| `write` | `path` (req), `content` (req) | create parent dirs, write the file; report `Created {path} ({N} lines)` over the numbered contents for a new file, or the numbered diff hunks vs the previous content |
| `edit` | `path` (req), `old_string` (req), `new_string` (req), `replace_all` (opt) | exact string replacement; error if `old_string` is absent, or non-unique without `replace_all`; report `Updated {path} (+A -D)` over the numbered diff hunks |

`read`/`write`/`edit` are separate JSON tools rather than one `apply_patch`
grammar: they work on any function-calling model, and `edit`'s exact
`old_string`→`new_string` contract (Claude Code's) is simpler and safer for
arbitrary models than fuzzy-context patch matching.

## Wire format (Chat Completions)

### Request

`OpenAiClient::build_payload` gains, when the client carries tools:

```json
"tools": [ {"type":"function","function":{"name":"bash", "...": "..."}}, ... ],
"tool_choice": "auto"
```

### The assistant's tool-call turn

Streamed back as `choices[].delta.tool_calls[]`, each delta carrying an `index`
(the key to accumulate on), an `id` + `function.name` on the first fragment, and
`function.arguments` string fragments that **concatenate** into the final JSON
argument object. `finish_reason:"tool_calls"` (or simply: any accumulated tool
calls) means the model wants to run them. The pure
[`ToolCallAccumulator`](../src/llm/openai.rs) folds the indexed deltas into
`Vec<ToolCallRequest { id, name, arguments }>` — unit-tested against fragmented
streams.

### Feeding results back

The backend appends two messages to its running request list per round:

1. the **assistant** message it just received, carrying `tool_calls` (and any
   text), and
2. one **`role:"tool"`** message per call: `{"role":"tool","tool_call_id":<id>,
   "content":<output>}`.

`ChatMessage` grew two optional, `skip_serializing_if`-guarded fields
(`tool_calls`, `tool_call_id`) so ordinary system/user/assistant messages
serialize byte-for-byte as before.

## The agentic loop (`llm::agent::run_agent`)

The heart of the feature is a **pure, generic driver** — like `llm::retry`'s
`run_stream` — so the whole loop is unit-tested with fakes and no network:

```
run_agent(tx, cancel, max_iterations, round, execute):
  loop:
    if cancel: return                       # silent — the UI owns the interrupt notice
    match round(&messages):                 # one streaming request (with its own retry)
      Complete            => tx.send(StreamDone); return
      Cancelled           => return
      Failed(e)           => tx.send(Error(e)); return
      ToolCalls{assistant, calls}:
        if iters == max_iterations: tx.send(Error("tool-call limit reached")); return
        messages.push(assistant)
        for call in calls:
          tx.send(ToolStart{ name: display_name(call), args: summary(call) })
          out = execute(&call, cancel)       # runs the tool locally
          tx.send(ToolEnd{ out.output, out.ok, out.truncated })
          messages.push(tool_result(call.id, out.output))
          if cancel: return
        iters += 1
```

- `round` is the real streamer: it builds the payload (with tools), streams one
  response — emitting `Chunk`/`Thinking*` exactly as today, plus a
  `ToolCallDelta(fragment)` for each streamed `tool_calls` piece so the status
  token tally ticks while the model *generates* the call (counted like reasoning,
  never rendered; see `docs/status-indicator.md`) — accumulates tool
  calls, and applies the **existing per-request retry** (`llm::retry`) internally,
  returning a `RoundOutcome`. Retry is unchanged and still per-request.
- `execute` is the real executor (below). Both are plain closures in the tests, a
  real streamer + real executor in production.
- `max_iterations` (`MAX_TOOL_ITERATIONS`, 20) bounds a runaway tool loop.
- **Cancellation** is checked between rounds, after each tool, and inside the tool
  itself — so Esc reaps promptly, same contract as the plain stream.

### Why the TUI barely changes

The event loop **already** interleaves `ToolStart`/`ToolEnd` with `Chunk`s — the
dummy has scripted exactly this since day one (`stream::turn_events`), and
`main.rs::on_stream_event` already flushes the text segment, shows the tool blue,
commits it green/red, and records it into `history`. A real model driving those
same events needs **no new event-loop code**. Interrupts, the Ctrl+O transcript,
resize repaint, and `context_messages` replay (finished tools → the native
`assistant tool_calls` + `tool` result pair on the next turn, matching this
in-turn protocol — see `docs/context.md`) all work unchanged.

## The executor (`llm::exec::RealToolExecutor`)

Boundary code (file + process I/O), verified by hand / smoke, wrapping the pure
cores in `llm::tools`:

- **`bash`** — `sh -c`, stdin `/dev/null`, stdout+stderr drained on reader threads
  (a chatty command can't deadlock a full pipe), retained up to
  `TOOL_OUTPUT_MAX_BYTES` (64 KiB) via a capped read, killed on the per-call
  timeout *or* a cancel. Output framed for the model as codex does:
  `Exit code: N` + the (truncated) output; a non-zero exit resolves the cell red.
- **`read`** — reads the file, applies `offset`/`limit`, formats numbered lines
  (`format_read`, pure), byte-caps the result.
- **`write`** — creates parent dirs, writes, returns `describe_change`: a brand-new
  file is a `Created <path> (N lines)` head over the **numbered contents**
  (`render_numbered_content` — `{n:>W} {text}` rows, the numbers matching `read`'s
  so the model can cite them to `edit`); overwriting is reported like an edit.
- **`edit`** — the pure `apply_edit` engine does the exact replacement; the
  executor writes it back and reports `Updated <path> (+A -D)` over the
  **numbered diff hunks** (`render_numbered_diff` — only each change run plus
  `DIFF_CONTEXT_LINES` (3) context lines each side, touching runs merged, distant
  hunks separated by a `⋮` gap row; `{n:>W} {sign}{text}` rows — context/added
  lines numbered by the *new* file, removed by the *old*, codex's
  `diff_render` numbering). Both bodies cap at `DIFF_MAX_LINES` with a
  `… N more lines` tail.

Every failure is returned as a **non-ok `ToolOutcome`** with a human/model-readable
message (never a panic) — the model sees the error string as the tool result and
can recover, exactly like codex's `RespondToModel`.

## Enabling / disabling

Tools are **on by default for the real backend** and never for the dummy (the
dummy's canned tools are unaffected). Toggle with `INLINE_TUI_TOOLS`
(`0`/`false`/`no` disables). When enabled, the tool-capability note in
[`prompts/tools.md`](../prompts/tools.md) is appended to the base system prompt
([`prompts/alter_zero.md`](../prompts/alter_zero.md)) — both `include_str!`d into
`llm::backend` so the wording lives in maintainable markdown files (swap a
persona by pointing the const at a different `prompts/*.md`). It names the tools
and the "prefer `read` before `edit`" convention; the schemas carry the detail.

The executor runs commands and writes files **in the app's working directory with
no sandbox** — the same trust model as the `!` shell. A future revision could add
approval prompts (codex's sandbox/approval flow); v1 trusts the operator, and the
env toggle is the off-switch.

### Manual live check

`examples/tool_smoke.rs` drives the whole loop against a real provider without
the TUI — handy for eyeballing the ToolStart/ToolEnd/answer flow:

```bash
OPENROUTER_API_KEY=sk-... cargo run --example tool_smoke -- \
  openai/gpt-4o-mini "Create hello.py, read it back, then edit its message."
```

(It reads the proxy CA from `SSL_CERT_FILE`/`INLINE_TUI_CA_FILE` like the app.)

## Rendering (codex's `diff_render`, in the `⎿` gutter)

`read`/`bash` cells render like any tool: the coloured `● Read(path)` /
`● Bash(cmd)` header + the dim `⎿` output peek (`docs/shell-command.md`'s gutter).

A `write`/`edit` cell whose output is the numbered format above renders as the
**codex/Claude-Code file cell** (`ui::file_cell_lines`):

```
● Write(index.html)
  ⎿ Created index.html (254 lines)
      1 <!DOCTYPE html>
      2 <html lang="en">
      …
    … +244 lines (ctrl+o to expand)

● Edit(index.html)
  ⎿ Updated index.html (+2 -2)
      5     <meta name="viewport" …>
      6 -   <title>Portfolio</title>
      6 +   <title>Bruce Rivero</title>
      7     <link rel="stylesheet" …>
        ⋮
     16 -   <span>Portfolio</span>
     16 +   <span>Bruce Rivero</span>
```

- The dim summary head sits on the `⎿` corner row, its `(+A -D)` counts
  coloured green/red (codex's header counts — `file_summary_spans`).
- Body rows re-style the output's own gutter text: the right-aligned **line
  number** dim, the `+`/`-` **sign** green/red, and the content
  **syntax-highlighted** by the path's extension (`highlight::Highlighter`,
  the fenced-code palette — the extension comes from the cell's `args`, and
  the lexer state resets at each `⋮` gap like codex's per-hunk highlighting).
- **Added rows** sit on a dark-green background tint and **removed rows**
  (their text dimmed) on a dark-red one — codex's dark-theme
  `#213A2B`/`#4A221D` tints — both padded to the full width like the
  user-message block. Context rows carry no tint. The `⋮` gap and `…` note
  rows stay dim.
- Long rows **wrap** (`code_content_rows`), continuations indented under the
  content column, keeping colour and tint.
- The **inline peek** shows the first `FILE_PEEK_LINES` (10) body rows (whole
  source rows only) then the `… +N lines (ctrl+o to expand)` hint; the Ctrl+O
  transcript shows everything.

All the styling is centralized `TOOL_DIFF_*`/`FILE_PEEK_LINES` consts in
`ui.rs`. Output that **doesn't** parse as the numbered format — a rollout
recorded before this format existed, or an error body — falls back to the
legacy colouring (a line starting `+` green, `-` red, the rest dim), so old
sessions keep rendering sensibly.

## Known limitations (v1)

- No sandbox / approval prompts — the executor trusts the operator (env-gated).
- `edit` requires a unique `old_string` (or `replace_all`); it does not do fuzzy
  context matching like codex's `apply_patch`.
- Parallel tool calls in one assistant turn are executed **sequentially** (the
  TUI shows one running tool at a time — invariant 4 in `CLAUDE.md`).
- Token counts remain an app-side estimate (the protocol's `usage` is still not
  surfaced).
