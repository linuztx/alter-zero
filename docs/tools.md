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
tool. Neither fits here: alter-zero talks the **Chat Completions** API
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
| `bash` | `command` (req), `timeout_ms` (opt, default 30 000, cap 600 000) | `sh -c command` with **no controlling terminal** (`crate::subprocess` — a `/dev/tty` password prompt fails fast), stdin `/dev/null`, stdout+stderr captured, byte-capped, killed on timeout/cancel |
| `read` | `path` (req), `offset` (opt 1-based line), `limit` (opt, default 2000 lines) | read the file: text returns `cat -n`-style numbered lines (so the model can cite line numbers to `edit`); an **image** (png/jpg/jpeg/gif/webp) is attached visually so the model can see it (`offset`/`limit` ignored — see "Image reads" below) |
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
run_agent(tx, cancel, max_iterations, round, execute, pending_notices):
  loop:
    if cancel: return                       # silent — the UI owns the interrupt notice
    for note in pending_notices():          # background completions since the last
      messages.push(user(note))             # round — a killed shell is known to the
                                            # model within the turn (docs/background.md)
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
- `pending_notices` takes the background registry's completion notice board
  (`LlmBackend` wires it; empty without a registry) — each note a user-role
  message appended after the prior round's tool results, exactly where the
  settled history item replays it for later turns (`docs/background.md`).
- **Cancellation** is checked between rounds, after each tool, and inside the tool
  itself — so Esc reaps promptly, same contract as the plain stream.

### Why the TUI barely changes

The event loop **already** interleaves `ToolStart`/`ToolEnd` with `Chunk`s — the
dummy has scripted exactly this since day one (`stream::turn_events`), and
`main.rs::on_stream_event` already flushes the text segment, shows the tool blue,
commits it green/red, and records it into `history`. A real model driving those
same events needs almost **no new event-loop code** — the one addition is the
`StreamEvent::ToolBatch` arm that registers a **parallel batch** up front (so its
not-yet-run calls show `⎿ Waiting…`; see `docs/parallel-tools.md`), after which
the same per-call `ToolStart`/`ToolEnd` flow runs unchanged. Interrupts, the
Ctrl+O transcript, resize repaint, and `context_messages` replay (finished tools →
the native `assistant tool_calls` + `tool` result pair on the next turn, matching
this in-turn protocol — see `docs/context.md`) all work unchanged.

## The executor (`llm::exec::RealToolExecutor`)

Boundary code (file + process I/O), verified by hand / smoke, wrapping the pure
cores in `llm::tools`:

- **`bash`** — `sh -c` via [`crate::subprocess::spawn_detached_shell`]
  (detached from the terminal — see below), stdin `/dev/null`, stdout+stderr
  drained on reader threads (a chatty command can't deadlock a full pipe),
  retained up to `TOOL_OUTPUT_MAX_BYTES` (64 KiB) via a capped read, killed on
  the per-call timeout *or* a cancel. Output framed for the model as codex
  does: `Exit code: N` + the (truncated) output; a non-zero exit resolves the
  cell red (and the display reframes the frame line as `Error: Exit code N` —
  `docs/tool-streaming.md`).
- **`read`** — reads the file; a text file applies `offset`/`limit`, formats
  numbered lines (`format_read`, pure), byte-caps the result; an image file
  takes the image branch below.
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

### No controlling terminal — the sudo-prompt fix (`crate::subprocess`)

`stdin(Stdio::null())` does **not** make a command non-interactive: a password
prompt (`sudo`, `ssh`, git's credential helper) opens **`/dev/tty`** — the
controlling terminal, inherited through the process *session*, not through any
fd — writes its prompt straight over the live region and then blocks reading
the same keyboard the event loop owns. Every shell runner (this executor,
`BackgroundRegistry::launch`, the `!` shell) therefore spawns through
`subprocess::spawn_detached_shell`, which runs the command in a **fresh
session with no controlling terminal** — the `/dev/tty` open fails and `sudo`
errors out in milliseconds (`sudo: a terminal is required to read the
password`), resolving the cell red with an actionable message instead of
hanging, exactly like Claude Code. The mechanism (a `setsid`-binary →
helper-re-exec → attached tier chain shaped by the crate's `forbid(unsafe)`),
the preserved `pgid == child.id()` kill contract, and the full test matrix
live in **`docs/tty-detach.md`**.

## Image reads (`read` on a png/jpg/jpeg/gif/webp)

`read` is not text-only: a path with an image extension comes back **visually**,
Claude-Code style, so the model can look at screenshots, downloaded pictures, or
plots it just generated.

- **Detection is by extension** (`tools::is_image_path` — png/jpg/jpeg/gif/webp,
  the four formats every vision-capable OpenAI-compatible endpoint accepts;
  pure, unit-tested). The executor then **sniffs the actual bytes**
  (`image::ImageReader::with_guessed_format`, the clipboard module's pattern) so
  the `data:` URL's MIME matches the content even when the extension lies, and
  anything that isn't really one of the four fails as a recoverable error the
  model reads. `offset`/`limit` are ignored for images.
- **The tool result stays small text** — `Read image {path} ({format}, {W}x{H},
  {size})` + "attached as the next user message" (`tools::format_read_image`).
  That text is what the cell shows, what the session rollout records, and what
  the token tally counts; the pixels never enter `output`.
- **The pixels ride `ToolOutcome::image`** (a base64 `data:` URL, encoded at
  the executor boundary) and `run_agent` attaches them as a follow-up
  **user-role parts message** — the `[image] …` note
  (`tools::image_attachment_note`) plus an `image_url` part — *after* the
  round's tool results. Why not inside the `role:"tool"` message? Chat
  Completions tool content accepts only text parts on OpenAI and most
  compatibles — an `image_url` part there is a 400. A user message with image
  parts is the standard vision shape this codebase already sends for Ctrl+V
  pastes, and injecting user-role messages mid-turn is the established pattern
  (the background notices). The results stay **contiguous** before the
  attachment: strict providers require every `tool_call_id` answered directly
  after the assistant message.
- **Later turns keep seeing it**: `context_messages` detects an image read from
  the stored record (`name == "Read"` + the `Read image ` output marker — a
  text read always starts with a numbered gutter row, `(file …`, or a
  `could not read …` error, so the marker can't collide) and replays the same
  note with the path as an `images` attachment. `build_messages` re-encodes it
  each request; a since-deleted file degrades to the existing
  `[image unavailable: …]` text note instead of failing the turn.
- **Size cap**: `READ_IMAGE_MAX_BYTES` (3.75 MB raw, so the base64 form stays
  under the strictest mainstream provider's 5 MB per-image limit — Claude
  Code's own bound). An oversized file fails with a recoverable "downscale or
  convert it with a bash command first".
- **Rendering**: the cell shows the fact line as a plain `⎿` output block —
  `ui::parse_file_cell` requires the numbered gutter and falls back to the
  legacy rendering, and the terminal can't show pixels anyway (same stance as
  the `[Image #N]` paste placeholders).

### Vision detection — degrading gracefully on a text-only model

Attaching an image to a model that can't see is not a soft failure: OpenRouter
**404s the whole request** ("No endpoints found that support image input"), so
without a gate an image read (or a Ctrl+V paste) on e.g. `openai/gpt-oss-120b`
kills the turn red. The gate reuses the `/v1/models` capability pattern the
Shift+Tab thinking cycle established (`docs/reasoning.md`):

- **Detection** (`models::vision_support_of` → `ModelEntry::vision`,
  per-record like the reasoning sniff): OpenRouter's
  `architecture.input_modalities` array (`"image"` ∈ it), falling back to the
  older combined `architecture.modality` string decided by its **input** side
  (`"text+image->text"` sees; `"text->image"` — a generator — doesn't);
  Venice's `model_spec.capabilities.supportsVision`. A record that says
  nothing (a bare OpenAI-style list) yields `None` = unknown → attach
  optimistically, exactly the old behavior.
- **Flow**: the picked entry's `vision` rides `Action::SelectModel` into the
  rebuilt backend (`Selection::vision` → `ModelConfig::vision` →
  `LlmBackend`), persists in `config.json` beside the thinking blob
  (`Settings::vision` — a legacy file just re-probes), and the startup
  capability probe (now thinking **and** vision) seeds it for env-selected
  models.
- **Effect when `Some(false)`**: the `read` tool declines an image path with
  a recoverable error *before touching the file* ("the current model does not
  support image input — … ask the user to switch … with /model"), so the
  model adapts within the turn; and `build_messages_for` replaces every
  attachment — a paste or a replayed read image — with an
  `[image omitted: {path} — …]` text note, so a past image degrades instead
  of poisoning every later request. A Ctrl+V paste additionally raises a red
  toast at the boundary (`docs/image-paste.md`). Live-verified:
  `live_non_vision_model_gracefully_declines_an_image_read` /
  `live_non_vision_model_survives_a_pasted_image` (the model answers "I can't
  view images" instead of the turn dying) and the two
  `live_*_models_report_vision_support` listings.

## Enabling / disabling

Tools are **on by default for the real backend** and never for the dummy (the
dummy's canned tools are unaffected). Toggle with `ALTER_ZERO_TOOLS`
(`0`/`false`/`no` disables). When enabled, the tool-capability note in
[`prompts/tools.md`](../prompts/tools.md) is appended to the system prompt —
after the persona ([`prompts/alter_zero.md`](../prompts/alter_zero.md)) and the
runtime **environment context**
([`prompts/environment.md`](../prompts/environment.md), `docs/environment.md`),
so the assembled prompt reads persona → environment → tools. All three are
`include_str!`d into `llm::backend` so the wording lives in maintainable
markdown files (swap a persona by pointing the const at a different
`prompts/*.md`). The note just names the tools; the schemas carry the detail.

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

(It reads the proxy CA from `SSL_CERT_FILE`/`ALTER_ZERO_CA_FILE` like the app.)

## Rendering (codex's `diff_render`, in the `⎿` gutter)

A `bash` cell renders like the `!` shell cell: the coloured `● Bash(cmd)` header
over a **multi-line `⎿` output peek** — up to `TOOL_PEEK_LINES` lines of the
output then `… +N lines (ctrl+o to expand)`. Its `Exit code: N` frame (kept in
`tool.output` for the model / context replay) is stripped for display, so the
cell reads like the real command output. **While it runs the cell streams and
tails its output** — the header, the last lines, and a `+N lines (Ns)` footer —
see `docs/tool-streaming.md`.

**The whole cell reads like a normal reply — Claude-Code's noticeable look.**
The entire `(...)` header body — the command text, its framing `(`/`)`, **and** a
truncation `…` — is **bold + the white assistant colour** (`TOOL_ARGS_COLOR`),
and the **output** under the `⎿` gutter is the same white (`TOOL_OUTPUT_COLOR`),
so a `bash` command and its output are as legible as a normal message rather than
the old muted grey. Only the structural bits stay dim ([`TOOL_DIM_COLOR`]): the
`⎿` corner glyph, the `Running…`/`Waiting…`/`(no output)` placeholders and the
`… +N lines` / `+N lines (Ns)` hints. The `●` bullet keeps its lifecycle colour
(blue running · vivid green ok · red fail). This is uniform across **every** tool
— `bash`/`read`/`write`/`edit` and any future tool — because the header goes
through the shared `tool_header_lines` and command/shell output through the
shared `output_row`. (A `read`/`write`/`edit` numbered cell instead
syntax-highlights its body — already vivid — see below.)

**Long headers wrap, never clip** (`ui::tool_header_lines`). A long command —
`● Bash(curl -s "wttr.in/…" 2>/dev/null || echo "…")` — used to run off the
terminal edge and lose everything past the last column. Now the `(args)` **word-
wrap** across continuation rows, each indented to align **under the opening `(`**
(the width of `● Bash`, Claude-Code style — the wrapped rows sit directly beneath
the paren, not one column past it), so the whole command reads clean:

```
● Bash(curl -s "wttr.in/Warsaw?format=%C+%t+%w+%h" 2>/dev/null
      || echo "wttr.in unavailable, trying alternative...")
  ⎿  Partly cloudy +19°C ↓8km/h 83%
```

**A very long header is capped inline** at `TOOL_HEADER_MAX_ROWS` (3) wrapped
rows, the remainder replaced by `…)` (`TOOL_HEADER_ELLIPSIS`, fitted within the
width) so a huge command can't flood the cell:

```
● Bash(for i in {1..5}; do echo "=== Iteration $i ===" && echo "Current
      time: $(date)" && echo "System uptime: $(uptime)" && echo "Memory
      usage: $(free -h | grep Mem)…)
```

The wrap + alignment is shared by the inline peek (`tool_lines`) and the Ctrl+O
transcript (`tool_full_lines`), so a resize/reflow re-wraps to the new width
identically — but only the inline peek (and the live preview) passes the row cap
(`Some(TOOL_HEADER_MAX_ROWS)`); the Ctrl+O view passes `None` and shows the
**whole** command untruncated.

**A running backend tool previews its whole cell.** While the model's tool runs,
the streaming strip's preview slot shows the *full* live cell — the wrapped
header **plus** its output. Before any output a `bash` cell shows `⎿ Running…`;
once output streams it **tails** — the last `TOOL_PEEK_LINES` lines and a
`+N lines (Ns)` footer (`ui::running_command_lines`; see
`docs/tool-streaming.md`) — so the running state is visible and a long command
still isn't clipped mid-run. The preview slot is sized by `ui::preview_rows`
(0 idle · 1 for a streaming reply / `!` shell run · N for a running backend
tool's cell); the status line (spinner + token tally + `esc to interrupt`) stays
below it. On `ToolEnd` the strip's cell is replaced by the committed scrollback
cell (header + the head peek) in place.

A `read`/`write`/`edit` cell whose output is the numbered format above renders as
the **codex/Claude-Code file cell** (`ui::file_cell_lines`) — the `⎿` corner is
two spaces wide (`  ⎿  …`, corner content at column 5) and the numbered body
sits **one column further in** (`ui::file_body_indent`), matching Claude Code:

```
● Read(index.html)
  ⎿  Read 254 lines
        1 <!DOCTYPE html>
        2 <html lang="en">
        …
     … +244 lines (ctrl+o to expand)

● Write(index.html)
  ⎿  Created index.html (254 lines)
        1 <!DOCTYPE html>
        2 <html lang="en">
        …
     … +244 lines (ctrl+o to expand)

● Edit(index.html)
  ⎿  Updated index.html (+2 -2)
        5     <meta name="viewport" …>
        6 -   <title>Portfolio</title>
        6 +   <title>Bruce Rivero</title>
        7     <link rel="stylesheet" …>
          ⋮
       16 -   <span>Portfolio</span>
       16 +   <span>Bruce Rivero</span>
```

- The **white** summary head (`TOOL_OUTPUT_COLOR`, so it's as noticeable as the
  output — not dim) sits on the `⎿` corner row: `Read {N} lines`,
  `Created {path} ({N} lines)`, or `Updated {path} (+A -D)` with the `(+A -D)`
  counts coloured green/red (codex's header counts — `file_summary_spans`). A
  `read` cell has no head in its output, so `ui::parse_file_cell` synthesizes
  the `Read {N} lines` line.
- Body rows re-style the output's own gutter text: the right-aligned **line
  number** dim, the `+`/`-` **sign** green/red (a `read`/`created` body has no
  sign column), and the content **syntax-highlighted** by the path's extension
  (`highlight::Highlighter`, the fenced-code palette — the extension comes from
  the cell's `args`, and the lexer state resets at each `⋮` gap like codex's
  per-hunk highlighting). Because `format_read` now emits the **same**
  `{n:>W} {text}` gutter as the write/edit bodies (dynamic-width numbers, a
  single space — not the old fixed-width `cat -n` tab), all three parse and
  render identically.
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
`ui/theme.rs`. Output that **doesn't** parse as the numbered format — a rollout
recorded before this format existed, an error body, or a `read` placeholder
like `(file is empty)` / offset-past-end — falls back to the legacy rendering
(the `write`/`edit` first-char `+`/`-` colouring, or a `read`'s plain dim
peek), so old sessions and edge cases keep rendering sensibly.

## Known limitations (v1)

- No sandbox / approval prompts — the executor trusts the operator (env-gated).
- `edit` requires a unique `old_string` (or `replace_all`); it does not do fuzzy
  context matching like codex's `apply_patch`.
- Parallel tool calls in one assistant turn are executed **sequentially** — but
  the whole batch is now **visible**: it is announced up front (a
  `StreamEvent::ToolBatch`), so the running call shows live while the not-yet-run
  ones show `⎿ Waiting…`, each committing as it finishes (`docs/parallel-tools.md`;
  invariant 4 in `CLAUDE.md`). True concurrent *execution* is still future work.
- Token counts remain an app-side estimate (the protocol's `usage` is still not
  surfaced).
