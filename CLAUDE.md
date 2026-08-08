# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Commands

```bash
cargo run                                   # run the TUI
cargo test                                  # all unit tests
cargo test <name_substring>                 # run a single test by name fragment
cargo test app::tests                       # run one module's tests
cargo clippy --all-targets -- -D warnings   # lint (warnings are errors here)
cargo fmt --check                           # formatting gate
cargo doc --no-deps --lib                   # intra-doc links must resolve
cargo build && bash scripts/smoke.sh        # drive the real binary in tmux
```

The standard pre-commit gate used throughout this project is: `cargo fmt --check`
+ `cargo clippy --all-targets -- -D warnings` + `cargo test` + `cargo doc
--no-deps --lib` all clean. The doc build is part of the gate because the crate
denies warnings, which promotes a broken intra-doc link to an error: a public
item's docs may not link to a private one, so moving an item between modules (or
narrowing its visibility) breaks the links that pointed at it. When that happens
the fix is to qualify the path if the target is still public
(`[`x`]` → `` [`x`](App::x) ``), else demote the link to a plain code span
(`[`x`]` → `` `x` ``) so the prose still names it.

Toolchain: Rust **edition 2024**, `ratatui = 0.30.1` (crossterm is re-exported as
`ratatui::crossterm` — import it from there, not as a separate crate), plus
`unicode-width` for display-width math, `unicode-segmentation` for the textarea's
grapheme-aware cursor/wrapping, and **`tokio`** (current-thread runtime) +
`tokio-stream` for the async event loop. The `Cargo.toml` `crossterm` entry exists
*only* to enable its `event-stream` feature (for `EventStream`); code still imports
crossterm through `ratatui::crossterm`, never as `crossterm::…`. `rust-toolchain.toml`
pins the toolchain; a `[lints]` table in `Cargo.toml` bakes the gate into every
build (`unsafe_code = "forbid"`, plus `warnings` and `clippy::all` denied).

## Architecture

A **library** (`src/lib.rs` → `app`, `stream`, `ui`, `term`, `frame`, `paste`,
`session`, `subprocess`, `history`, `textarea`, `file_search`, `clipboard`, `context`, `background`, `agents`, `ask`, `tasks`, `checkpoint`, `project_doc`, `permission`, `settings`, `cli`) holds the logic; **`src/main.rs`** is a 77-line shell —
the detached-exec hook, the CLI resolution, the viewport, the loop — over
**`src/tui/`**, the binary-private tree that drives the codex-style **async
(tokio) `select!`** loop (`event_loop`, `actions`, `turn`, `stream`, `agent`,
`background`, `permission`, `view`, `commit`, `models`, `config`, `bootstrap`,
`startup`, `recorder`, `resume`, `history_store`, `settings`, `shell`, `workers`, `host`,
with the **`Session`** struct itself in `mod.rs` — every handler is an `impl
Session` block in its area module, reaching the private fields the way `app/`'s
submodules reach `App`'s). The four big ones are **directories
of per-area modules**, not single files — `src/app/` (`types`, `action`, `keys`,
`composer`, `commands`, `file_picker`, `input_history`, `queue`, `tools`, `turn`,
`compact`, `backtrack`, `views`, `resume`, `model_picker`, `login`, `settings`, `background`,
`agent`, `status`, `permission`, with the `App` struct itself in `mod.rs` so every submodule and
the test tree keeps its private-field access), `src/ui/` (`theme`, `wrap`,
`layout`, `assistant`, `inline`, `table`, `message`, `conversation`, `tool`,
`file_cell`, `status`, `agent`, `menu`, `footer`, `header`, `live`, `transcript`,
`context_view`, `resume_view`, `model_view`, `login_view`, `background_view`,
`permission_view`, `settings_view`, `stream_render`), and **`src/stream/`** — the backend seam
kept apart from the offline demo that used to crowd it: `event` (the whole
`StreamEvent` wire format), `source` (the `ReplySource` trait), `cancel`
(`CancelToken`), `stall` (`StallAi`), and the self-contained **`dummy/`**
subtree (`mod` — `DummyAi` + `turn_events` + the playback pacing, `scenario` —
**the registry that decides which demo a prompt plays**, `script` — the canned
replies, `turns` — the pure `Cue → Vec<StreamEvent>` turns, `gated` — the ones
that block on the permission gate); adding a demo is one `SCENARIOS` entry plus
its turn function plus its example prompt in the suite, and the suite proves
every entry is still reachable (the two hand-written `if`/`else` chains it
replaced could retire a demo silently by shadowing its cue). The dummy is what a
first run *meets*, so it behaves like one: each scenario carries its own
two-part reply (split at a blank line — a tool call finalises the text before it
as its own history message, so a mid-paragraph split would break the block)
narrating the cells it is drawing, every user-facing one closes on the shared
`handoff!()` sentence pointing at **`/login`** then **`/model`** (the suite fails
a scenario that doesn't; `smoke.sh` settles on that sentence), and every scripted
call resolves with **the real executor's output** — `tools::format_read`'s
numbered gutter for `Read`, `Created …`/`Updated …` over
`render_numbered_content`/`render_numbered_diff` for `Write`/`Edit`, and the
`Exit code: N` frame for `Bash` (streamed body first, framed only at the
`ToolEnd`, exactly as `llm::exec` does) — so the offline cells are numbered,
syntax-highlighted and red-on-failure like the live ones instead of plain text
peeks — see `docs/dummy-backend.md`. The three library `mod.rs`es re-export their areas **by name** — never a glob,
so the public surface is auditable and `tests/api_surface.rs` can lock it — and
every `crate::app::X` / `ui::y(…)` / `stream::Z` path is what it always was; `src/tui/` needs
no facade (nothing outside the binary can name it — `main.rs` reaches exactly
`tui::startup::resolve_cli` and `tui::event_loop::run`);
see `docs/module-layout.md` for the map. The pure, unit-tested logic lives in
`app`/`stream`/`ui`/`textarea`/`file_search`/`session`/`history`/`context` (plus the pure cores of `frame`/`paste`/`subprocess`) so behavior
is testable with a plain `Buffer`/`TestBackend` and no real terminal. `src/tui/`
**and `term.rs`** are the I/O boundary (as is `clipboard.rs`'s Ctrl+V read, the
`/resume` session recording + dir scan — `tui::recorder::SessionRecorder`/`list_sessions`,
whose JSONL format/parse core is the pure `session` module — and the
cross-session input-history file — `tui::history_store::InputHistoryStore`, whose JSONL
format/parse core is the pure `history` module, `docs/history-persistence.md`) — verified via `scripts/smoke.sh`, not
unit-tested save for the odd pure helper that has no terminal in it (like
`term`'s `keyboard_enhancement_disabled` env predicate — see
`docs/shift-enter.md` — or its `visible_cells` cell emitter, which skips the
cells shadowed by a wide emoji/CJK glyph so painted rows never drift — see
`docs/table-streaming.md` *Wide glyphs*); `frame`'s async scheduler **task** is smoke-covered too
(its rate-limit/coalesce math is unit-tested). Keep logic out of the boundary;
the geometry *policy* `term.rs` acts on — live-region height, the box's re-pin,
the cursor seat — comes from pure `ui` helpers it calls (`ui::live_height`,
`ui::repin`, `ui::cursor_position`, `ui::restore_cursor_row`); only the viewport
bookkeeping (`init`'s anchor math, `write_above`'s scroll plan, `resized`'s
re-clamp) is its own, smoke-covered.

The design rationale lives in `docs/design.md`; the async-loop design in
`docs/async-rewrite.md`; the editable input (textarea) design in
`docs/textarea.md`; the Esc-interrupt design in `docs/interrupt.md`; the ↑/↓
input-history recall in `docs/input-history.md`; the Ctrl+R reverse search over
that history in `docs/history-search.md`; the **cross-session persistence** of
that input history (an on-disk `history.jsonl` seeding `App::input_history` at
startup, so ↑/↓ recall *and* Ctrl+R span sessions) in
`docs/history-persistence.md`; the `!` local shell commands in
`docs/shell-command.md`; the `?` shortcuts band in
`docs/shortcuts.md`; the Shift+Enter / Ctrl+J newline keys in
`docs/shift-enter.md`; the mid-turn message queue in `docs/queue.md`; the
session-context footer in `docs/footer.md`; the **Shift+Tab thinking-mode
cycle** (a reasoning-capable model's effort — detected per model from the
provider's `/v1/models`, shown beside the model name in the footer, cycled
with a `Thinking: {mode}` toast, riding the request as the unified `reasoning`
parameter, persisted beside the `/model` selection) in `docs/reasoning.md`;
the **thinking stream** — that reasoning, *shown* (a phase's
chain-of-thought streams live in the strip wearing the **tool cell's shape**:
a `● Thinking…` header — the same `TOOL_BULLET` a running tool wears, because
it means the same thing, breathing at the frame pulse beside a label carrying
the status line's **shimmer** (`ui::status::shimmer_spans_from`, the same wave
`Working…` wears but floored at the near-white `REASONING_SHIMMER_BASE`, since
codex's grey base is right for a metric and unreadable for a header) — over the
thought in the `⎿` gutter, dim and **italic** (the one cue separating it from a
tool's output there), tail-following its last `REASONING_PEEK_LINES` wrapped
rows; at the phase's end the cell shape goes away entirely and it **collapses**
into one committed **bullet-less** two-tone
`Thought for 1m 5s · 1.5k tokens (ctrl+o to expand)` line — the `summary_lines`
shape, because nothing is happening any more and what is left is a fact about
the turn — **dim throughout** (`REASONING_LABEL_COLOR` = `STATUS_DONE_COLOR`,
`Done for Ns`'s exact dress, so the pair bracketing a turn reads as a pair);
the text itself never reaches immutable scrollback (which is *why* it can
collapse) but expands in Ctrl+O under that same dim line, minus the hint since
the expansion has none to make room for; `HistoryItem::Reasoning` records it, the rollout keeps it
across a `/resume`, `context::context_messages` **skips** it (a Chat
Completions request has nowhere to put a previous round's chain-of-thought,
so Ctrl+D shows no trace either), the settle points are `ThinkingEnd`/Esc/a
backend error via the one `tui::stream::Session::settle_reasoning`, and the
cell's tokenizer estimate is **snapped** to the provider's own
`completion_tokens_details.reasoning_tokens` — `TokenUsage::reasoning` — when
the round's usage frame lands, split by weight across a round's several
phases; gated by `ALTER_ZERO_SHOW_THINKING`, whose falsy value restores the
old counted-and-dropped behaviour exactly) in `docs/thinking-stream.md`;
the **running bullet's pulse** (a tool in flight no longer
shows a blue `●` — it shows the permission prompt's grey, and in the live
region that grey *breathes* dim→bright→dim once a second, Claude-Code's
running dot: a raised cosine over the boundary-injected `App::set_pulse`
frame clock (one shared phase, so a mixed round's tool cells and its agent
tree blink in step and a background agent animates between turns), applied
by `ui::live_tool_lines` in the strip only — `tool_lines` renders at rest so
a scrollback commit can never freeze a frame mid-breath, and the Ctrl+O
transcript stays still to keep its cache's signature clock-free) in
`docs/tool-pulse.md`; the flicker-free frame pipeline
(scrollback commits deferred into the draw's synchronized update) in
`docs/flicker.md`; the `@` file-path picker (async walk+rank file search below
the box) in `docs/file-search.md`; the large-paste `[Pasted Content N chars]`
placeholder (bracketed paste → a compact placeholder, expanded back on send) in
`docs/paste.md`; the **Ctrl+V image paste** (clipboard image → temp PNG → an
`[Image #N]` composer placeholder whose path rides a separate typed channel to
the backend) in `docs/image-paste.md`; the **Esc-Esc backtrack** (edit a
previous user message: prime → transcript preview → rewind + prefill) in
`docs/backtrack.md`; the **`/resume` session picker** (every conversation
recorded to a rollout JSONL file, listed in a full-screen picker whose Enter
loads it back and appends the turns that follow to the same file — and its
**CLI twins**: `--continue` reopens the newest session recorded in this cwd,
`--resume {id}` one by id (bare `--resume` boots into the picker), the flags
resolved to a rollout path in `main()` *after* the detached-exec hook and
*before* the terminal boots (fail-fast on stderr, the pure parse in `cli`,
the id lookup via `session::rollout_file_id`/`latest_for_cwd`), the loaded
transcript committed under the banner through `insert_before` — never a
startup Purge, the user's terminal scrollback survives — and a quit that
recorded anything printing `Resume this session with: {bin} --resume {id}`
after `term.restore()`, `docs/cli.md`) in
`docs/resume.md`; the **filesystem checkpoints** (every turn snapshots the whole
cwd into an *isolated* git store — never the user's real `.git` — keyed to the
conversation length, so the Esc-Esc backtrack **and** `/resume` **reset the
code**, not just the transcript: rewinding restores the working directory to the
`checkpoint::restore_target` at that point, backing up the current tree first;
the pure mapping/format is `checkpoint` + `session::parse_checkpoints`, the git
I/O is `checkpoint::CheckpointStore`, turn-end snapshots ride
`dispatch_after_turn`, and restores hang off the `ResumeSession` /
`ConfirmBacktrack` arms; gated by `ALTER_ZERO_CHECKPOINTS` **and by the cwd
being project-scoped** — `checkpoint::cwd_allows_checkpoints` refuses the home
dir itself, its ancestors, and filesystem roots, since the session-start
snapshot's whole-cwd `git add -A` over `~` blocked the raw-mode terminal for
minutes before the first frame, the "hangs in `~`" bug) in
`docs/checkpoint.md`; the **parallel tool-call batch** (the model's several tool
calls in one round announced up front so the running one shows live while the
not-yet-run ones show `⎿ Waiting…`, executed sequentially) in
`docs/parallel-tools.md`; the **live-streaming `bash` tool** (a running command
tails its output — the last rows, long lines word-wrapped to the width with
spaces preserved, + a `+N lines (Ns)` footer — via a
`StreamEvent::ToolOutput` channel, collapsing to the head peek `… +N lines
(ctrl+o to expand)` when it finishes, Claude-Code style) in
`docs/tool-streaming.md`; and the **background shells** (the `bash` tool's
`run_in_background` arg — the call resolves at once with a task id while a
`BackgroundRegistry` process streams on its own channel; **Ctrl+B** moves a
running model-`bash`/`!` command to the background mid-run (the live cell hints
it with a dim `(ctrl+b to run in background)` row that waits a few seconds —
`ui::TOOL_BACKGROUND_HINT_DELAY`, gated on the command's own boundary-injected
`App::command_elapsed` — so a fast command never flashes it, Claude-Code-style;
Ctrl+B itself works the whole time); the cell resolves `⎿ Running in the
background (↓ to manage)`, the footer
counts `· N shells` — and that count is the band's **entry point**: **↓ from an
empty composer lights the indicator on cyan** (`App::background_focus`, the
rest of the footer untouched; Esc/↑/Ctrl+C dismiss it, any other key clears it
and acts) and **Enter opens the inline manager band**
(list → per-shell details with a live-tailing output box → `x` stops), and a
completion is **immediate feedback**: its model-facing note posts onto the
registry's notice board the moment it exits (the in-flight agent takes the
board before each round, so a shell the model just `kill`ed is known to it
within the same turn, right after the killing call's tool result) and the
green/red `● Background command "…" completed` notice cell commits at the next
safe boundary — a tool resolution mid-turn, else the turn end — while a
model-launched note **no agent read** auto-starts a follow-up turn that tells
the model the result when nothing else is queued (an agent that already heard
it mid-turn owes no follow-up), `Done for Ns · N shells still running` on the
summary) in `docs/background.md`; and the **`Agent` tool** (Claude-Code-style subagents,
`docs/agent-tool.md`: the model launches autonomous side-agents —
`description`/`prompt`/`subagent_type`/`run_in_background` (default true) —
each running its own `run_agent` tool loop over a fresh context on its own
thread, reporting on a dedicated `agents::AgentEvent` channel (a seventh
`select!` source — agents outlive turns); a foreground group shows the live
breathing-grey `● Running {n} agents…` tree (per-agent description · tool uses · tokens
· a **sticky** `{Name}: {detail}` activity — a bash call's own `description`,
held between calls — Ctrl+B moves the group to the background; a **lone**
agent renders `● Agent({description})` over its live tool header instead)
committing as
`● {n} agents finished (ctrl+o to expand)` with `⎿ Done`/`⎿ Interrupted` rows
(a lone agent as `● Agent({description})` + `⎿ Done ({n} tool uses · {tokens}
tokens · {s}s)`),
a background launch resolves at once as `● {n} background agents launched
(↓ to manage)` with each completion posting its model-facing note on the
shared notice board (the in-flight turn hears it mid-round, an idle
completion auto-starts the follow-up turn — the background-shell pattern) and
its green/red `● Agent "…" finished · Ns` cell settling at the same safe
boundaries; the footer gains a persistent roster — `● main` over
`◯ {type}  {description} {elapsed} · ↓ {tokens} tokens` rows — that ↓ steps
into **after** the shell indicator (`❯` selection, Enter views, `x` stops
immediately, hint lines in the footer slot; finished agents linger coloured
`AGENT_LINGER` then sweep); Enter on an agent opens its **inline session
view** — a purge-rebuild showing the agent's own transcript under the banner,
the composer's top rule labelled with its description, typing **chats with
the agent** (queued into its running loop at the next round boundary via the
registry's pending-input seam, or a continuation run over its stored message
list when idle) while the composer keeps its full functionality — the `/`
palette, `?` band, Ctrl+R, the `@` picker, **Ctrl+O showing the agent's own
transcript** and **Ctrl+D its derived context** (`!` shell mode stays
literal chat text) — Esc returning to the purge-rebuilt main view (main
commits are suppressed while the view is up, invariant-4 style); the Ctrl+O
transcript expands each agent as `● Agent({description})` with `⎿ Prompt:`,
the nested tool headers, `⎿ Response:`, and `⎿ Done ({n} tool uses ·
{tokens} tokens · {s}s)`; the parent's calls replay as native `agent`
tool_calls + results (`context.rs`), the records round-trip (`session.rs`),
and subagents never get the `agent` tool — no nesting); and the **tty detach** (every shell child —
model `bash`, `!`, background — spawned into a fresh session with no
controlling terminal via `subprocess::spawn_detached_shell`'s
setsid-binary → helper-re-exec → attached tier chain, so a `/dev/tty`
password prompt like `sudo`'s fails fast in a captured error instead of
hijacking the TUI and hanging) in `docs/tty-detach.md`; and the **tool
permission requests** (Claude-Code's ask-before-you-change: the `approve` seam
`llm::agent::run_agent` consults before every `write`/`edit`/`bash` call's
`ToolStart` raises an inline modal — a coloured `Create file`/`Edit file`/`Bash
command` title (`· from the {type} agent` when a subagent asked), the target,
the **whole** numbered content/diff framed by `╌` rules (capped only to fit the
terminal, with a `… +N lines` tail), the question, and `❯ 1. Yes` / `2. Yes,
allow all edits during this session (ctrl+a)` — for `bash`, `2. Yes, and don't
ask again for: {rule}` where the rule reads `python3 *` for a prefix scope
(the star = any arguments; an exact-only scope shows the whole command, no
star, and there is no letter shortcut any more) — / `3. No` over `Esc to cancel · Tab to amend`
(`· ctrl+e to explain` on a command; the options show **no hardware cursor at
all** — `ui::cursor_visible`, the frame just skips its closing `Show`: a menu
has nothing for one to point at, and a kitty cursor trail drew a streak on
every open and every ↑/↓ — while its *seat* still tracks the highlighted
option, found `PERMISSION_TAIL_ROWS` up from the region's bottom, which is why
a capped prompt pads *above* the question; Tab's amend field is typed into, so
the caret comes back with it) — while the tool thread **blocks** on the
shared `permission::PermissionGate` (the `Arc<Mutex<…>> + Condvar` sibling of
the background/agent registries, its `wait` polling the turn's `CancelToken` so
an Esc reaps it); the prompt is modal (routed first in `on_key`, replacing the
composer, the status line, the bands and the footer — but **never the cells that
raised it**: the call being asked about keeps its `● Write(tt.py)` header over
the same dim `⎿ Waiting…` its batch siblings show (the approve seam runs before
`ToolStart`, so it genuinely is waiting — Claude Code's look; a truly running
call, the main turn's own under a subagent's request, keeps its `⎿ Running…`
at rest), and a subagent's request keeps the whole live
`● Running 3 agents…` tree, with `App::command_elapsed` reading `None`
meanwhile so the delayed Ctrl+B hint never advertises a key the modal
swallows — but the context is **budgeted**: a big parallel batch's screenful
of `⎿ Waiting…` siblings used to squeeze the body's budget to zero (a prompt
with no content) and push the options off the bottom, so `permission_lines`
reserves its fixed rows plus a body floor (`PERMISSION_MIN_BODY_ROWS`, the
peek size; a shorter body reserves only its own height) and collapses the
cells that don't fit into one dim `… +N more waiting` row, the first chunk —
the asked-about call, or the tree that asked — never dropped); the region
**grows like any other** (ordinary `ui::repin`, invariant 3): the chat above
the prompt scrolls into the terminal's **real scrollback**, so the newest
messages sit right above the question — Claude Code's picture — and the user
can scroll the terminal up and re-read anything while it asks (the retired
covering geometry held the newest screenful in *no* buffer, which read as
"terminal scroll is disabled while it asks", worst in kitty — `smoke.sh`
Phase 58); commits flow under it too, so a batch's **back-to-back prompts**
(the next request landing in the same frame gap as the previous cell's
commit) just scroll the resolved cell in above the still-open prompt, visible
at once and exactly once (`smoke.sh` Phase 59); the scrolls are one-way
though, so every such move under an open prompt — its growth, a commit
beneath it, or a rebuild while it was open (a mid-prompt resize's purge, an
overlay return whose prompt opened underneath — Ctrl+O/Ctrl+D up when the
request arrived) — is noted on the viewport itself (`term::paint_live` and
`term.reflow` set the modal-scrolled flag, the two places one-way moves
happen), and the first draw after the prompt closes consumes it with a
**purge rebuild** (`InlineViewport::take_modal_scrolled`, `smoke.sh` Phases
58/60/62) — box flush at the bottom, scrollback rebuilt from history, nothing
lost or doubled — instead of a plain shrink stranding the box above the rows
the collapse vacates; the same rebuild answers a **shrink while the prompt is
still open** (`ui::modal_needs_rebuild` over the viewport's `painted_bottom`,
Phase 63): back-to-back prompts differ in height — a body-capped screen-tall
prompt answered into a one-line file's, the resolved cell committing out of
the live region between them, a subagent's tree-topped prompt giving way to a
main-turn one — and a frame that would seat the pinned region short of the
screen bottom it was *painted* flush against used to strand the open prompt
above a band of blank rows until it was answered (the reported
empty-newlines-under-the-prompt bug), where the draw tick now purge-rebuilds
first, `reflow` re-arming the note so the eventual close still purges; it
**stashes the composer draft**
and hands it straight back on close so a request landing mid-sentence costs
nothing, Tab swaps the options for that same textarea as an amend field whose
Enter rejects *with* the typed feedback, Esc cancels (reject + the ordinary
turn interrupt, the abandoned id released on the gate so a background agent's
thread never parks), a rejection still commits the red `⎿ User rejected write
to hello.py` cell — Tab's typed feedback on a second `Instructions: …` line,
the transcript's only record of it — while the *model* reads the longer
stop-and-wait text (`Approval::Reject`'s two fields, streamed together as
`StreamEvent::ToolRejected` in place of the `ToolEnd`), and **that
model-facing text is what the conversation keeps**: it rides the recorded call
as `ToolCall::context_output` and `context::context_messages` replays it —
`ToolCall::context_text()` — as the `tool` result, so Ctrl+D shows what the
model was actually told, every later turn still carries the user's
instructions (they used to survive exactly one round, history having kept only
the one-line cell), a `/resume` restores them (`session::ToolRecord`, the
field omitted when absent so old rollouts still parse), a subagent's refused
call keeps both texts on its own transcript, and the token tally charges the
uploaded text rather than the cell line; and option 2's allowlist remembers
every segment's **program-word prefix** (`python3 script.py` → `python3 *`;
the curated subcommand tools keep their verb — `git status`, `npm run` —
Claude-Code-style) — degrading to the exact command when a
redirect/substitution (quote-aware), a leading env assignment, or a command
wrapper (`sudo`, `sh -c`, …) means a prefix would hide what matters — and
**sweeps the requests already queued** that the new rule covers
(`App::drain_covered_permissions` — parallel agents all ask before any is
answered, so one answer covers them all); the session carries a **permission
mode** (`permission::PermissionMode` — `manual` asks for everything, `edit`
auto-approves `write`/`edit` while commands still ask, `auto` additionally
sends a non-allowlisted `bash` command to the **auto mode classifier** — a
silent LLM safety check on the session's own provider
(`llm::classifier::SafetyClassifier`, prompt in `prompts/classifier.md`,
`ALTER_ZERO_CLASSIFIER_MODEL` overrides the model) consulted by the approve
seam in the user's stead: the asked-about cell just keeps its `⎿ Waiting…`
row while the verdict streams (silently — no UI events), an **allow** runs
the call with a dim `⎿ Allowed by auto mode classifier` row appended to the
resolved cell (the `Approval::AllowNoted` → `StreamEvent::ToolNote` →
`ToolCall::approval_note` chain — rendered inline and in Ctrl+O, recorded in
the rollout so a `/resume` keeps it, and on a subagent's own transcript via
the same event), a **deny** rejects it red (`Denied by auto mode
classifier` + `Reason: …`) through the ordinary `ToolRejected` path with a
Claude-Code-style stop-or-adjust model text, and a classifier **failure**
falls back to the ordinary prompt (never an allow); `master` runs
*everything* unasked — no prompt, no classifier, Claude Code's
bypass-permissions) pinned flush at the
footer's **right edge** (`{model} · {cwd}      manual` — its columns reserved
off the left chain's budget, so the `…` truncation can never eat it) and
**cycled** with **Ctrl+A** (manual → edit → auto → master → manual, one step
per press — from the composer, or on an open prompt: a file prompt's option
2 *is* the switch to `edit`, with a `Mode: edit …` toast; back to `manual`
and file changes ask again; a step onto `master` sweeps the open/queued
prompts it now covers; the offline dummy demos auto mode with the pure
heuristic `permission::auto_verdict` instead of an LLM); the rules **persist
per project** in
`~/.alter-zero/permissions.json` (`{"projects": {"/abs/cwd":
{"allow_commands": ["python3 *", …], "mode": "edit"}}}` — prefix rules
star-suffixed, exact commands verbatim, `auto`/`master` labels round-tripping
the same way, the pure format in
`permission::PermissionsFile`, the read-modify-write I/O + startup gate seed
in `tui::config`/`tui::permission`), so "don't ask again" and the mode survive a restart in the
same directory; gated by `ALTER_ZERO_PERMISSIONS` (disabled = no gate, no
footer segment, Ctrl+A explains via toast)) in
`docs/permissions.md`; and the **`AskUserQuestion` tool** (Claude-Code's
mid-turn questions, `docs/ask.md`: the model asks 1–4 multiple-choice
questions — `askuserquestion`, offered only when `LlmBackend::with_ask`
attached the session's `ask::AskGate` (always, in the app; subagents never
get it) — and its thread **blocks on the gate** exactly like a permission
request while an inline modal (the permission prompt's sibling: modal keys
routed first, composer draft stashed/restored, `ui::region_is_modal` so the
close purge-rebuilds, the two modals queueing behind each other via
`App::open_next_pending`) walks the user through a chip strip of question
tabs (`☐`/`☒`/`✔ Submit`, the **current chip lit on the cyan selection
background**, ←/→/Tab/Shift+Tab moving, a lone question showing no Submit
tab), numbered options with dim descriptions (digits jump-activate; Enter on
a single-select records + advances — a lone question resolves at once —
while multi-select `[✔]` checkboxes toggle and confirm via their own
unnumbered `Submit` row), an auto-added free-text **`Type something.`** row
(the entry reuses `App::input` like Tab's amend field — a real composer
field: **Shift+Enter/Ctrl+J** newlines render as wrapped rows in place and
survive into the answer, and a large **bracketed paste** collapses to the
`[Pasted Content N chars]` placeholder (atomic Backspace), spliced back to
the real text on the entry's exit via `paste::expand_pastes_consuming` — the
stashed composer draft's own pairs survive the modal; Enter accepts —
single-select advances with the custom text as the answer, multi-select
checks it — Esc keeps the draft unchosen), a side-by-side **preview panel**
when any option carries `preview` content (options left, the focused
option's bordered panel right, the `Notes: press n to add notes` line
beneath — `n` opens the notes field, the same multi-line/paste-capable
entry, Enter/Esc both keep the text), a
**`Chat about this`** row resolving the whole call as "the user wants to
talk" (red cell + stop-and-wait result), and — for several questions — a
closing **review page** (an amber `⚠ You have not answered all questions`
warning whenever the submission would be partial, then `● question` over the
**green** `→ answer` for the **answered questions only** — an unanswered one
is omitted; its ☐ chip and the warning already say so —
`❯ 1. Submit answers / 2. Cancel`) whose empty submission walks to the first
unanswered question instead of submitting nothing; Esc anywhere **declines**
(the turn continues: red `User declined to answer questions` cell over the
`· question (options)` rows, the model reading the stop-and-wait result and
reacting in the same turn — never a forced interrupt); a submission resolves
green via the new `StreamEvent::ToolAnswered { display, result }` (the
`ToolRejected` twin, from `ToolOutcome::context` — the split the executor
`llm::ask::ask_user` builds with the pure `ask::answered_display` /
`answered_result`), the committed cell rendering the headline as its `●`
header (`ui::tool`'s ask special case) over the `⎿ · Q → A` gutter rows
while the model reads the schema's `{"answers": {question: labels},
"annotations": {question: {notes, preview}}}` JSON — kept on
`ToolCall::context_output`, so the derived context, Ctrl+D, and a `/resume`
all replay exactly what was sent; abandoned requests (Esc-cancelled
permission turns, `/clear`) release on the gate as declines at the loop
bottom so no thread parks; the offline dummy's `Play::Asked` scenario (cue
"ask" + "question") drives the whole round trip through the real
`ask_user` mapping — three questions: single-select coffee, multi-select
demo topics, preview+notes code style) in `docs/ask.md`; and the **task
tools** (Claude-Code's structured task list, `docs/task-tools.md`: the model
plans multi-step work with `taskcreate`/`taskget`/`tasklist`/`taskupdate` —
Claude Code's schemas minus the `owner`/`metadata` parameters this
single-agent TUI has no use for — over a shared `tasks::TaskRegistry`
(`LlmBackend::with_tasks`, the ask-gate pattern; subagents and the `/compact`
backend never get it), and the calls render **no tool cells anywhere
inline** (Claude Code hides them): each resolves through the single
`StreamEvent::TaskCall` — `run_agent` skips the batch announcement, the
permission seam, and the Start/End pair for task calls, the post-call
snapshot riding `ToolOutcome::tasks` — whose loop arm flushes the streamed
segment (each round's narration stays its own `●` bullet) and records a
cell-less `HistoryItem::TaskCall`; what the user sees instead is the **live
checklist under the status line** (`ui::task_rows`/`checklist_lines`, its
rows threaded through `live_height`/`live_layout`/`cursor_position` beside
the queued rows, inside the status slot so the `⎿ ◻ subject` rows hang off
the spinner): `◻` pending, `◼` in progress (cyan glyph, bold subject), `✔`
completed (green glyph, dim struck-through subject), a dim `› blocked by #1`
suffix naming only **open** blockers, one-row truncated subjects, and past
`TASK_MAX_ROWS` a prioritised fold into a dim `… +N pending` row — while the
spinner **wears the active task's `activeForm`** (`App::task_verb` →
`ui::status_line_with_verb`, Claude Code's `currentTodo.activeForm ??
randomVerb`, derived per frame so completing the task snaps the verb back);
**at rest** the same rows sit above the composer under Claude Code's dim
`1 tasks (0 done, 1 open)` count line (`ui::idle_task_lines` — the same
glyphs/fold, the `⎿` gutter swapped for the composer's inset, since there is
no spinner to hang from), so work left over stays in view between turns;
and a **finished** plan *retires* — the turn that ticked the last task keeps
its all-green rows, then `dispatch_after_turn` drops the list whole
(`App::retire_finished_tasks` → `TaskStore::retire_if_finished`, re-syncing
the registry), so it is gone rather than hidden and the next `taskcreate`
opens a new plan at `#4` instead of appending to the old ticks (the id
high-water mark survives; a `/resume`/backtrack applies the same rule to the
snapshot it restores);
the record keeps everything the cell-less display doesn't: Ctrl+O expands
each call as an ordinary tool cell (`TaskCallRecord::as_tool_call`), the
derived context replays the native `tool_calls`+result pair (args `{}` like
the ask tool — the result text is what the model reasons from), the rollout
round-trips the call **with its post-call snapshot** (`session`'s
`task_call` record, ids on a high-water mark that survives deletion), and
all three history rewinds restore the list exactly — `/resume` and the
Esc-Esc backtrack from the last record('s snapshot) before the cut
(`App::reset_tasks_from_history`), `/clear` to empty — with the boundary
syncing the shared registry after each (`Session::sync_task_registry`) so
the model's next `tasklist` agrees with the strip; the offline dummy's
`tasks` scenario (cue `todo`/`task`) drives a real `TaskStore` through the
whole lifecycle so every scripted result string and snapshot is
byte-for-byte the live executor's) in `docs/task-tools.md`; and the **Ctrl+O
performance work** (the incrementally-built, boundary-warmed transcript cache
and the atomic queued overlay switch, so the transcript opens instantly on a
big resumed session with no blank alt screen / kitty cursor-trail streak) in
`docs/tool-view-performance.md`; and **`/compact` + auto-compact** (codex's
context compaction, ported append-only: a summarization turn streams the
model's handoff summary invisibly into `App::compact_buffer`,
`finish_compact` appends a `HistoryItem::Compaction` marker — the transcript,
recorder, checkpoint keys, and backtrack all untouched — and
`context::context_messages` derives codex's compacted shape from the *last*
marker: the 20k-approx-token budget of recent user texts + the
`SUMMARY_PREFIX\n{summary}` bridge in place of everything before it, the
`● Context compacted · {before} → {after} tokens` cell the visible record;
with the model's **context window** known — `/v1/models` `context_length`
via `ModelEntry::context`, persisted in `config.json`, overridable via
`ALTER_ZERO_CONTEXT_WINDOW` — the footer shows a `{used}/{window} ({pct}%)`
gauge (usage-frame fed, tokenizer-estimated offline) and the loop **auto-runs** the
same turn past codex's 90% threshold (`App::should_auto_compact`, one
attempt per user turn, the cell tagged `· auto`)) in `docs/compact.md`; and
the **`/settings` menu** (`docs/settings.md`: the knobs that were only ever
`ALTER_ZERO_*` environment variables — plus a hard-coded `retry::MAX_RETRIES`,
a hard-coded `agent::MAX_TOOL_ITERATIONS`, and an always-on auto-compaction —
made *visible and changeable mid-session*
in the `/model` picker's inline frame, the third composer-replacing picker:
nine rows (**Hide thinking**, **Error retry**, **Tools**, **Permission
mode**, **Checkpoints**, **Auto compact**, **Project docs**, **Temperature**,
**Max tool calls** — whose `0` default means *no limit*, since a cap that
trips mid-task abandons the work half-done and Esc is already the stop
button; it counts the **calls**, not the rounds, because a round can be a
whole parallel batch, and a round the budget can only partly afford is
clamped rather than refused whole)
of `{label}  {value}` in an aligned column over a `(n/total)` counter, the
highlighted row's description, and a `Type to search · Enter/Space to change ·
Esc to cancel` hint; every value **cycles** — there is no free-text field, so
Enter and Space mean the same thing on every row and Space never reaches the
type-to-search (which matches the label *and* the description, so `agents.md`
finds **Project docs**). The pure model is `settings::SessionSettings` +
`SettingKey`; the rows are **derived, never stored** (`App::setting_rows`),
so the value column can't drift from what the session is doing, and
**Permission mode** is a second door onto `App::permission_mode` — cycling it
returns the existing `Action::SetPermissionMode` so Ctrl+A's whole path (the
gate, the covered-request sweep, the per-project persist) still runs. A knob
the host can't serve is **unavailable** — `SettingAvailability`, injected at
the boundary like the clock: it renders `false (unavailable)`, refuses to
cycle with an explanatory toast, and is never persisted. Everything else
returns `Action::SettingChanged(key)` and `tui::settings::Session::apply_setting`
does the work: **Tools**/**Error retry**/**Temperature**/**Max tool calls**
rebuild the backend (`ModelSession::set_tools`/`set_max_retries`/
`set_temperature`/`set_max_tool_calls` — the `/model` switch's full-attachment
rebuild, carrying the active thinking mode forward; the retry budget and the
tool-round ceiling ride `LlmBackend::with_max_retries`/`with_max_tool_calls`
into every round, a subagent's included), **Checkpoints** flips `CheckpointStore::set_enabled`
(which can only ever turn a *capable* store on or off), **Project docs**
reloads or drops `App::user_instructions` at once, and **Hide thinking** /
**Auto compact** need nothing — they are read where they are used
(`tui::stream`'s `ThinkingStart` arm and `App::should_auto_compact`), so
there is no second copy to drift. It persists to its own
`~/.alter-zero/settings.json` — beside `config.json` and `permissions.json`,
one file per feature that owns it — as a **diff from the defaults** (only
what the user changed reaches the wire), written as a **read-modify-write**
over the blob the file itself holds (`Session::saved_settings` +
`SessionSettings::copy_value`) so an `ALTER_ZERO_*` override merged in at
startup can never *stick*: the environment wins for the run, per setting,
and only the row the user actually cycled is saved).

### The runtime model and its invariants

This is an **inline** TUI: finished messages *and tool calls* flow into the
terminal's real scrollback; a live region (a rule-framed input box — a codex-style
**`textarea`** whose cursor moves anywhere (←/→ by grapheme, ↑/↓ across *wrapped*
rows, Home/End) with insert/delete at the cursor, growing as the input wraps;
from an **empty composer (or an unedited recall) ↑/↓ instead step through
previously submitted inputs** shell-style (`App::input_history`, codex's
`ChatComposerHistory` — ↓ past the newest clears; **persisted across sessions**
in an on-disk `history.jsonl` seeded at startup, `docs/history-persistence.md`;
see `docs/input-history.md`),
and **Ctrl+R reverse-searches them** codex-style (`App::history_search` — the
footer slot becomes a `reverse-i-search: {query}` line owning **every** key,
the newest case-insensitive substring match previews in the composer with the
query occurrences highlighted, Ctrl+R/↑ and Ctrl+S/↓ step older/newer clamping
at the ends, Enter accepts the preview as an editable draft seating ↑ at it,
Esc/Ctrl+C cancel restoring the pre-search draft and cursor; see
`docs/history-search.md`) —
plus, *while a turn is in flight*, a strip above it — a streaming preview row (the
preview shows a running tool's cell when one is executing — its bullet a
**breathing grey**, `docs/tool-pulse.md` — a backend tool's
**whole** collapsed cell, the wrapped `● name(args)` header *plus* its output;
before any output a `⎿ Running…` row, and once a `bash` command **streams** it
**tails** its output — the last `TOOL_PEEK_LINES` display **rows**, long lines
word-wrapped like the Ctrl+O view (`ui::wrap_output` — never clipped at the
width, spaces preserved), + a
`+N lines (Ns)` footer counting the fully hidden source lines
(`ui::running_command_lines`, `docs/tool-streaming.md`) — so a long
command isn't clipped and the running state shows; a **parallel
batch** previews the *whole* `tool_queue` — the running call over each dim
`⎿ Waiting…` sibling, blank-separated, `docs/parallel-tools.md`; the preview slot
is sized by `ui::preview_rows`, a running `!` shell/streaming reply staying one
row; `docs/tools.md`), a blank gap row,
a codex-style **status line** (`(●•·   ) {verb}… ({elapsed}s · {↓|↑} {n} tokens ·
Thinking for {m}s · esc to interrupt)` — opened by a comet spinner (a
Larson-scanner sweep: a white head dragging a fading grey tail back and forth
between dim walls), the verb text
shimmering with a white sweep ported from
codex's `shimmer_spans`; on finish a dim `{done verb} for {n}s` summary commits to
scrollback, while **Esc mid-turn interrupts** instead (codex-style — cancel + reap
the backend, drain the channel, keep the partial, resolve a running tool as
failed, commit the red `Conversation interrupted` notice, **no** summary — but
when **nothing had streamed** (no partial, no tool, empty queue) it instead
**undoes** the submission, the message back in the composer and no notice, and a
**`!` shell interrupt** commits no notice either (its `⎿ Interrupted by user`
cell is the record); see
`docs/interrupt.md`; **idle Esc instead arms the Esc-Esc backtrack** — a second
Esc previews previous user messages in the transcript overlay and Enter rewinds
the conversation to the highlighted one, its text back in the composer
(`App::backtrack`, codex's `BacktrackState`; see `docs/backtrack.md`) — Esc
quits only with an empty composer and no user message to backtrack to, a
typed draft making it a codex-style no-op) — see `docs/status-indicator.md`),
then another blank gap row so the
status clears the box's top rule — plus a
scrollable **slash-command palette** band *below* the box when the input is a bare
`/token` (the same slot shows a **`?` shortcuts band** — codex's footer shortcut
overlay, two dim columns of `{key} for {thing}` entries — when `?` is pressed in
an empty composer; any other key dismisses it, Esc dismiss-only; see
`docs/shortcuts.md`; **and the same slot shows an `@` file picker** — a fuzzy
file list — whenever the cursor is in an `@token`: the boundary's background
worker walks the cwd **afresh per query** and ranks it off-thread (so a file
the agent just created appears immediately — never a startup-cached index;
codex's `StartFileSearch`/`FileSearchResult` round-trip — `App::file_search_query`
changes drive a `dispatch_file_search`, results come back via
`App::set_file_matches` with a staleness guard), the rows **columned**
codex-style — `→ name  parent/  File|Dir`: the selected row's `→` marker, the
name column sized to the widest visible name, the parent dir (`./` at the
root), and the kind label pinned at the right edge, at most 8 rows — ↑/↓ move
and **Tab/Enter insert
the path** (replacing the `@token`, a trailing space added, whitespace paths
quoted), Esc dismisses sticky-per-token; the matched characters are bolded in
each row (remapped across the name/parent split); suppressed in `!` shell mode
and mutually exclusive with the palette;
see `docs/file-search.md`); plus, *above* the box while a turn streams, **messages
submitted with Enter queue** instead of waiting (shown like sent user messages,
inset two columns — `  ❯ {msg}` rows in the strip under the status line —
`App::queued`, a `VecDeque<QueuedTurn>` of **typed entries** (text `Messages`
batches and standalone `Shell` commands — codex's action-tagged
`queued_user_messages`): **Enter appends to the current batch** (those messages
auto-sent together as one next turn) while **Tab opens a new batch** (codex's
Tab-to-queue — its message runs as a *separate follow-up turn* after the first
queue, a blank row dividing them), the loop dispatching one entry per turn end
(a text batch to the model, a queued `!command` run locally) —
**Esc interrupts and sends the front batch right away**, Alt+Up pulls the **last
batch** (its messages newline-joined) back into the composer to edit, leaving
earlier batches queued; see
`docs/queue.md`; plus **`!command` runs a local shell command** (codex's `!`
shell mode: a leading `!` is **absorbed** into `App::shell_mode` and rendered
back as the composer's red `! ` prompt — `! pwd`, never `❯ !pwd` — with a red
`Shell mode` hint in the footer slot (Backspace/Esc on the empty shell composer
exit the mode; the palette/`?` band are suppressed in it); Enter from an idle
composer runs the draft under `sh -c` on a background thread as a turn
(`App::begin_shell`; every shell child — this, the model's `bash`, a
background launch — spawns **detached from the controlling terminal** via the
shared `subprocess` module's setsid detach chain, so a `/dev/tty` password
prompt like `sudo`'s fails fast instead of printing over the TUI and fighting
the loop for the keyboard, `docs/tools.md`) committing a codex-style **exec cell** — the `! command`
header on the dark user-style line (a `Role::Shell` message) with the `⎿`
output **flush** below, `⎿ Running… (Ns)` while it runs (the elapsed rides the
preview — a shell turn **hides the spinner status line** entirely,
`ui::strip_has_status`), **no** `Ran for Ns` summary, Esc killing the child
(resolving `⎿ Interrupted by user` with **no** `Conversation interrupted`
notice — the cell is the record);
mid-turn it queues as a standalone `Shell` entry run locally when its turn comes
— codex parity, never merged into a text batch, Alt+Up over it re-enters shell
mode; see `docs/shell-command.md` and `docs/queue.md`); plus a **transient toast**
— a one-row, self-clearing status line pinned at the *bottom of the strip, just
above the box's top rule* (`App::toast`/`ui::toast_rows`/`toast_line`, threaded
beside `queued_rows`; dim for info, red for errors). It's raised for
confirmations and soft rejections the user should see but never keep — `/copy`
(`Copied last message to clipboard`), a model switch, a mid-turn `/resume`/`/help`
rejection — instead of committing a scrollback bullet; it never enters `history`,
and its expiry is timed at the boundary (`Session::toast_deadline` +
`Session::toast`, the timestamp pattern, cleared in the draw tick). See
`docs/toast.md`; plus a one-row
**session footer** on the region's last row —
codex's footer status line, `{model} · {cwd}` dim and two-space inset
(`dummy_model_name · ~/repo      manual` — the Ctrl+A **permission mode**
pinned flush at the row's right edge, `docs/permissions.md`, hidden when
permissions are off; a reasoning-capable model carries its Shift+Tab
thinking mode beside the name — `{model} {mode} · {cwd}`, `docs/reasoning.md`)
— whenever no band is open (the palette/shortcuts
band displaces it, and the Ctrl+R search line / `!` shell-mode hint take its
slot; `App::set_session_info` injects the strings at the boundary
like the clock, the model name coming from `ReplySource::model_name`; see
`docs/footer.md`) stays pinned at the bottom. The alternate screen hosts the
full-screen overlays: the **Ctrl+O tool-output view**, a full-screen overlay listing every tool
call's complete output while the conversation keeps streaming underneath (see
invariant 4), the **`/resume` picker**, and the **Ctrl+D context-debug view**
(the raw LLM context window — the derived conversation the real backend sends
each turn, tool calls in the provider-native `tool_calls`/`tool` wire format
(an assistant `→ name(args)` request + a `tool:` result entry) and `[Image #N]`
placeholders unrendered; `ui::render_context_view`/`ui::context_lines` over
the pure `context::context_messages`, see `docs/context.md`). ratatui's `Viewport::Inline` can't change height after startup, so
`term::InlineViewport` is a *custom* inline viewport over a `CrosstermBackend`
whose height is **dynamic** — the input box grows with the wrapped input, the
streaming strip, the palette band, and the session footer (`ui::live_height`). Four non-obvious
invariants hold the whole thing together — breaking any one reintroduces a class
of bug:

1. **One stdin reader, created after the init cursor query.** The async loop reads
   keys from a single crossterm `EventStream`; the reply backend (a
   `stream::ReplySource`, e.g. `DummyAi`) streams on a background thread that *only
   sends* `StreamEvent`s on a tokio channel. `InlineViewport::init` queries the
   cursor position (DSR) over stdin *once, synchronously, before the `EventStream`
   exists*; a second stdin reader would steal that reply and cause "cursor position
   could not be read". So: create the `EventStream` only after init, and never add
   another thread or task that reads stdin (`insert_before` tracks the viewport row
   itself and never queries the cursor). **Relatedly, the detached-exec hook
   (`subprocess::run_detached_exec_if_requested`) must stay the *first statement*
   of `main()`** — before the tokio runtime and any terminal I/O: in a helper
   re-exec (`{exe} __alter-zero-detached-exec {cmd}`) the process must `setsid`
   away and `exec` `sh` before it ever touches stdin/stdout or spawns a thread, or
   it would boot a TUI into the caller's pipes and the tty detach would silently
   break (`docs/tty-detach.md`). Never move it, and never let anything run above it.

2. **Greedy word-wrap is prefix-stable** (`ui::wrap_text`): appending text only
   ever changes the *last* wrapped line. This is what makes streaming-to-scrollback
   safe — the **incremental** `ui::StreamRender` flushes every completed line to
   scrollback via `term::InlineViewport::insert_before` as the reply grows,
   *caching* the rendered rows of already-complete source lines and only rendering
   the newly-arrived tail (so streaming a reply is **O(reply)**, not O(reply²) — it
   replaced a per-chunk *whole-reply* re-render that starved the status animation on
   long code; see `docs/markdown.md`). `StreamRender::commit` withholds the
   still-growing last line (and, inside a fenced code block, the **whole**
   in-progress line — a code line's colour isn't final until its closing `(`/`//`
   streams in, and it may span several wrapped rows), plus any **trailing blank
   rows** (a model's `…\n\n` before a tool call — trimmed so they don't stack on
   the boundary's single spacer into three blank rows; the batch `assistant_lines`
   trims them too, both gated on `!in_code` so a blank inside an open fence
   survives), `StreamRender::preview` renders
   just that last line for the strip (cheap enough to redraw every animation frame),
   and `StreamRender::finish` flushes the remainder on `StreamDone`. The tests
   `ui::tests::incremental_commits_reconstruct_the_whole_reply`,
   `stream_render_is_prefix_stable_over_every_prefix`, and
   `streamed_code_never_recolours_a_committed_row` lock this property (committed rows
   + final flush == the fully-rendered message, and no committed row ever changes
   text *or* colour). Don't change the wrap algorithm — or the markdown/highlight
   line renderers (`AssistantRenderer`, driven by `markdown::BlockScanner` +
   `highlight::Highlighter`) — without re-checking those invariants.

3. **The viewport is content-anchored (top fixed), and resize reflows both
   directions** (`tui::view::Session::repaint_conversation`). Like Claude Code / codex, the
   box grows *downward* in place — `term::draw` keeps its top put and only scrolls
   the screen *up* (oldest chat into scrollback) once the box would overflow the
   bottom; a shrink blanks the rows it vacates (the decision is the pure
   `ui::repin`). Never force it to `screen.height - height` — that reintroduces
   the "box jumps to the bottom" bug. **One view closes differently: an inline
   *modal*** — the tool-permission prompt, `ui::region_is_modal`, the one region
   that can be as tall as the terminal. It *grows* by the same `ui::repin` as
   everything else — the chat it displaces scrolls into the terminal's real
   scrollback, staying reachable while it asks (`docs/permissions.md`) — but
   those scrolls are one-way, so a plain shrink at the close would strand the
   box above the rows it vacates. `InlineViewport` notes every one-way move
   under an open prompt (`modal_scrolled`, set by `paint_live`'s scrolls and by
   any `reflow` run while the prompt is open), and the first draw after the
   prompt closes consumes the note with a purge rebuild — box flush at the
   bottom, scrollback rebuilt from history, nothing lost or doubled
   (`smoke.sh` Phases 58/60/62). A shrink **while the prompt is still open**
   gets the same answer *before* the paint (back-to-back prompts of different
   heights): `ui::modal_needs_rebuild` rebuilds when the frame's plan —
   `view_top` + pending rows + the new height — would seat the region short
   of the screen bottom the last frame was *painted* flush against
   (`InlineViewport::painted_bottom`; the tracked height re-syncs between
   paints, so it can't serve), since painted in place it would strand the
   open prompt above that same blank band for as long as it asks (Phase 63).
   The streaming strip (preview + gap + status
   + gap) sits *above* the box, so it grows the region upward; when a reply ends the
   strip's rows become the committed final line + spacer + the `Done for Ns` summary
   and the box must **stay put**, so `StreamDone`/`Error` call `term::set_view_height`
   to reseat the viewport to its idle height *before* the final `insert_before`s —
   the queued lines flush with the *latest* tracked height, so skipping the reseat
   makes the flush over-scroll, the box rise off the bottom, and blank rows appear
   beneath it (guarded by `smoke.sh` Phase 5; and because a strip collapse can now
   coincide with a flush *mid-stream* too — a forming table's whole block commits
   at its close while the multi-row preview drops to one row — `term::paint_live`
   syncs the tracked height to the frame's height before `flush_pending` whenever
   lines are pending, `docs/table-streaming.md`, guarded by Phase 41). (`insert_before` itself only
   **queues**: the next `term::draw` writes the lines and repaints the live region
   inside one synchronized update, so a commit can never flash a boxless frame —
   `docs/flicker.md`, guarded by `smoke.sh` Phase 15.) On **any** size change the
   conversation repaints from source — a width change stales every wrapped line, and
   a height-only change moves the screen contents out from under the tracked
   viewport row (the emulator scrolls/clips to fit; repainting at the stale row
   leaves phantom input boxes — `term::resized` re-clamps the viewport like codex,
   and the repaint reseats it; guarded by `smoke.sh` Phase 17). `App` retains a
   `history: Vec<HistoryItem>` of finished messages *and
   tool calls* (kept for two reasons: this repaint, and listing tools in the Ctrl+O
   view) and `term::reflow` seats the viewport at the top, writes the re-wrapped
   tail (`ui::repaint_lines`) and paints the live region below it — all in
   the same synchronized frame (stale queued lines are dropped: the tail regenerates
   them). How the screen is prepped first is a `term::ReflowClear` mode: **`InPlace`**
   (the Ctrl+O / `/resume` overlay return) **overwrites the screen top-down** then
   clears the rows below the tail, and `clear_region(All)`s *only* for an empty
   tail — because a leading full clear before the tail-write's scroll makes tmux
   spill the on-screen frame into scrollback, and after a Ctrl+O return (invariant 4)
   that pushes the **stale streaming strip** (`Working… (… tokens)`) into scrollback
   above the rebuilt conversation (guarded by `smoke.sh` Phase 7). **`Purge`** (`/clear`
   *and* every resize) instead **purges scrollback + clears the whole screen** first
   (codex's `clear_scrollback_and_visible_screen_ansi` — `ESC[2J` then the `ESC[3J`
   scrollback purge, emitted as one ANSI write) and rebuilds the **full** history
   (`RESIZE_REFLOW_MAX_ROWS`-capped) into the blank screen, `write_above` scrolling
   the overflow back into the now-empty scrollback: so `/clear` truly wipes
   scrollback (scrolling up shows nothing, not the old chat), and a resize can't
   leave the emulator's own reflowed copy of the old rows behind — the TUI-text
   duplication that in-place overwrite showed on a width change (guarded by
   `smoke.sh` Phases 16 and 17). A mid-stream repaint must not lose the
   in-flight partial reply (it lives in the streaming buffer, not `history`):
   the tail carries the rows the stream already committed
   (`ui::repaint_tail` / `StreamRender::committed_rows`) and the rows that
   arrived since — chunks drained under the overlay, or the whole partial after
   a purge reset the render — are queued right after via the normal
   `insert_before` pipeline, reaching scrollback exactly once (guarded by
   `smoke.sh` Phase 35; a resize that lands *while* an overlay is up upgrades
   that return's repaint from `InPlace` to `Purge` — `overlay_resized`).

4. **Tool calls interleave with text, and Ctrl+O opens a separate overlay.** A
   tool call splits the assistant text around it: `App::flush_streaming_segment`
   finalises the run of text before a `ToolStart` as its own history message so the
   tool slots *after* it in order (the scrollback and the resize/return repaint
   must agree). Inline a tool is collapsed (`ui::tool_lines` — coloured bullet +
   one-line peek). When the model requests a **parallel batch** of calls in one
   round, they are announced up front (`StreamEvent::ToolBatch` →
   `App::start_tool_batch`, filling the `App::tool_queue` `VecDeque`) so the live
   region shows *every* call at once — the running one live (pulsing grey), the not-yet-run
   siblings as dim `⎿ Waiting…` cells (`ToolStatus::Waiting`), each committing to
   scrollback as its `ToolEnd` arrives. Execution stays **sequential** (only the
   front of the queue is ever `Running`, so the invariant is "at most one running
   tool", not "at most one live tool"); an interrupt/error resolves the running
   call and drops the un-started `Waiting` siblings (`docs/parallel-tools.md`). The
   Ctrl+O overlay (`ui::render_tool_view` on the alternate
   screen) is codex's **Ctrl+T transcript pager** — a slash-tiled dim
   `/ T R A N S C R I P T` title row, the scrolling transcript body with
   vi-style `~` filler past its end, a `─` separator carrying the scroll
   percentage right-aligned, and two dim key-hint rows (↑/↓, pgup/pgdn,
   home/end jump; q/esc/ctrl+o close — though Esc when idle with a previous
   user message instead *begins the backtrack preview* in place, and while one
   highlights a message the second hint row swaps to the backtrack keys;
   `docs/backtrack.md`) — showing the **full conversation
   transcript**: `ui::transcript_lines`
   walks `history` (messages + each tool's *expanded* output) plus the live tail
   (in-progress reply / the running tool **and any `⎿ Waiting…` batch siblings**,
   the whole `tool_queue` in order) plus the still-queued backlog
   (`ui::queued_lines`' inset rows, so Ctrl+O never hides a queued message —
   `docs/queue.md`). That walk is **O(history)** and re-runs the markdown +
   syntax highlighter over the whole transcript, so the loop drives it through a
   **`ui::TranscriptCache`** (a `Session`-owned cache, like `StreamRender`) that
   builds **incrementally** (`docs/tool-view-performance.md`): committed items
   are immutable and history otherwise only grows — every non-append mutation
   (a `/clear`, a `/resume` load, a backtrack truncation, an interrupt-undo
   pop) bumps `App::history_generation` — so the cache keeps a **frozen
   prefix** of per-item rendered rows pinned on `(generation, width, cwd)` and
   each refresh re-renders only the newly committed items + the volatile live
   tail (the Esc-Esc highlight is an in-place style diff on the frozen rows,
   never a re-render). A cheap signature (generation, history length, live-tail
   length, the tool queue's shape — its length + front-call status **+ front
   output length**, so a Waiting→Running flip, a batch call committing, *or a
   running `bash` call streaming its output* invalidates it — the last is what
   makes the overlay show the **live streaming output** (unlike Claude Code,
   which only shows a tool's output once it finishes; the overlay tail-follows
   the frontier, `docs/tool-streaming.md`) — backtrack selection, width)
   short-circuits a refresh entirely, so a scroll keypress is a cache hit
   (O(viewport)) and a streamed chunk costs O(live tail), not O(history).
   `draw_tool_view` refreshes it **once** per draw (shared by the scroll clamp
   and the render); it is **retained across overlay closes** and pre-warmed at
   the loop bottom (`TranscriptCache::warm` — a no-op when nothing committed,
   the whole loaded history on the iteration a `/resume` swaps it in), so
   Ctrl+O never opens cold: the switch itself is atomic (`term::enter_overlay`
   only *queues* hide+switch+clear; the first `draw_overlay` flush delivers
   them **with** the painted frame as one write — no blank alt screen for a
   kitty cursor-trail to streak across, `docs/tool-view-performance.md`).
   Only the **user** message shows its
   wall-clock `timestamp` (`hh:mm AM/PM`, no seconds): dim, **right-aligned on
   its own line below the message** — the *only* stamp displayed anywhere
   (AI/tool/summary stamps are recorded but never shown; never inline; the
   clock is injected via `App::set_clock`, see `docs/timestamps.md`). **While
   the overlay is up the loop keeps
   draining reply events into `App` but does *not* commit to scrollback** (that
   would write into the alt screen); on return, `repaint_conversation` rebuilds the
   inline view from `history`. Never commit to scrollback while
   `app.view == View::ToolOutput` — nor while an agent session view
   covers the inline screen (the one gate, `tui::commit::Session::commits_allowed`; an open
   permission prompt is deliberately *not* on the list — a commit beneath it
   scrolls in above the region, visible at once, and the scroll it causes is
   one of the one-way moves the close's purge rebuild answers —
   `docs/permissions.md`). **Quitting from the overlay is also a return**:
   the `Action::Quit` arm must `exit_overlay` *then* `repaint_conversation` + `draw`
   before breaking — otherwise `restore` lands on the stale streaming strip a turn
   that finished in the overlay left behind, instead of the committed `Done for Ns`
   summary (`smoke.sh` Phase 7).

### Data flow

The loop is an async (`tokio`, current-thread) `select!` over five sources —
input, reply events, draw ticks, `@` file-search results, and finished Ctrl+V
clipboard reads (each paste's read + decode + encode runs on its own worker
thread so the loop — and the status animations — never block on it);
`select!`'s randomized branch order gives input/draw fairness for free. Every state
change calls `frame.schedule_frame()`; the `frame` scheduler coalesces those into a
single draw tick, rate-limited to 120 fps (`MIN_FRAME_INTERVAL`). A paste/fast-type
run is caught by `paste::PasteBurst` so its characters request relaxed frames
(`schedule_frame_in` — the rate-limit floor does the coalescing; the scheduler
keeps the soonest pending deadline). *While a turn is active* the draw branch **re-arms** the next
animation frame (`schedule_frame_in(32ms)`, codex's status-widget cadence) so the
status line's shimmer sweeps and its timer advances with no events; before each
draw the loop writes the computed `elapsed`/`thinking` `Duration`s onto the status
(`App::set_status_times`), keeping time out of the pure core (the timestamp-clock
pattern). `insert_before` only **queues** its lines: the draw tick writes them and
repaints the live region in **one synchronized frame** (codex's pending-history
pattern — no flushed state ever lacks the box, the flicker fix; `reflow` paints the
same way, and `restore` flushes any quit-before-tick leftovers; `docs/flicker.md`).
(See `docs/async-rewrite.md`, `docs/status-indicator.md`.)

```
keyboard / resize ──► EventStream ─┐
reply backend ─────► tokio mpsc ───┼─► select! ─► App::on_key / push_chunk / start_tool / set_file_matches / attach_image / set_status_times / … ─► schedule_frame
frame scheduler ───► draw-tick ────┤                                        coalesce + 120fps ─► draw / draw_overlay
file-search worker ► tokio mpsc ───┤
image-paste worker ► tokio mpsc ───┘
                                        └─ turn active? re-arm a frame in 32ms (status shimmer + timer)
```

`Submit(text)` runs `tui::turn::Session::start_turn` — a batch of one: it records each
user message (the Enter arm also records the text into `App::input_history` for
the ↑/↓ shell-style recall — adjacent duplicates collapse, `/clear` doesn't
touch it), `insert_before`s it, then spawns one reply via the selected
`ReplySource` (`backend.spawn(texts.join("\n"), images, tx, cancel)` — the
Ctrl+V image paths drained from `App::take_submission_images`, see
`docs/image-paste.md`), keeping the
thread handle + `CancelToken` so a quit mid-stream cancels and reaps it.
**Enter *while a turn is in flight* queues** the message into `App::queued`
(codex's `queued_user_messages`) instead of producing `Submit` — shown like
sent user messages (`❯` rows) in the strip above the box. The queue is a
sequence of **typed entries** (`QueuedTurn`): Enter appends to the last
`Messages` batch (`queue_draft(false)`), **Tab opens a new batch**
(`queue_draft(true)`, a separate follow-up turn), and a **mid-turn `!command`
queues as a standalone `Shell` entry** (`queue_shell`, run locally, never
merged); `tui::turn::Session::flush_next_queued` dispatches **one entry**
(`drain_next_batch`) per turn end — a `Messages` batch via `start_turn`, a
`Shell` via `run_shell` — so Enter messages batch into one turn while Tab
follow-ups and `!` commands iterate in order (Alt+Up pulls the **last entry**
(`drain_last_batch`, `pop_back`) back into the composer to edit — a `Messages`
batch newline-joined, a `Shell` entry as `!command` re-entering shell mode —
earlier entries stay queued; see `docs/queue.md`). The
backend interleaves `StreamEvent::ToolStart{name,args}`/`ToolEnd{output,ok,truncated}` pairs
(with `ToolOutput(chunk)` **live-output** deltas streamed in between — the
running `bash` cell tails them via `App::push_tool_output`, `docs/tool-streaming.md`)
and a `ThinkingStart`/`ThinkingEnd` pair (with `ThinkingChunk` reasoning
deltas streamed in between) between `Chunk`s; the loop shows the tool
running (pulsing grey) then commits it collapsed (green/red), and flips its `thinking_start`
`Instant` so the status line shows/drops `Thinking for Ns` — while the
reasoning deltas themselves accumulate in `App::reasoning` for the live
`● Thinking…` block, collapsing at `ThinkingEnd` into the committed
`Thought for …` cell (`docs/thinking-stream.md`; with
`ALTER_ZERO_SHOW_THINKING` falsy no buffer is ever opened and they stay opaque,
counted-only, as they were). Before a tool runs, the backend also streams the model **generating** the call as
`ToolCallDelta(fragment)` events (the `name`/`arguments` pieces of a `tool_calls`
delta — `openai::Delta::tool_call`, surfaced ahead of the `ToolStart`); the loop
counts them via `App::push_tool_call_progress` (never rendered) so the tally ticks
while the call is produced, exactly like reasoning. The just-sent
**user message is counted up front** (`App::count_user_input` after
`begin_stream`, arrow `↑` — uploaded input), so the status shows `↑ N tokens`
through the backend's **pre-stream pause** (`DummyAi` waits `STARTUP_DELAY`/3s
before its first chunk so the indicator is visibly working first — overridable
via `ALTER_ZERO_STARTUP_DELAY_MS`; the strip reserves **no preview row** while
there's nothing to preview — `ui::preview_rows` 0 — so the pause is status +
gap only, no stray empty line, like codex). Then `Chunk`s,
`ThinkingChunk`s (counted via `App::push_thinking`, and kept for the thinking
stream's live block when a phase is open — `docs/thinking-stream.md`), the
tool-call generation deltas, and a tool's
output grow the cumulative token tally on `App::status` (`↓` while replying,
thinking, or generating a tool call, `↑` for the input and right after a tool — never reset); on `StreamDone` `App::end_turn`
records the `Done for Ns` summary. A backend may send `StreamEvent::Error(msg)` instead of
`StreamDone` — even mid-tool; the loop turns that into a red `Role::Error` notice via
`App::fail_stream` (which also resolves a still-running tool as failed —
`Interrupted by a backend error` — and clears the status). **Esc while the turn is in
flight returns `Action::Interrupt`** (palette-dismiss still wins; when idle Esc
arms the Esc-Esc backtrack instead, is a no-op over a typed draft, and quits
only with an empty composer and no user message to edit —
`docs/backtrack.md`): the loop cancels + detaches the backend, **swaps the channel** (a stale
`ToolStart` would wedge a phantom running tool), and `App::interrupt_turn` returns
either `Kept` — keep the partial, resolve a running tool as failed
(`Interrupted by user`), record the red `INTERRUPT_NOTICE` (**`None` for a shell
turn** — its cell is the record), clear the status with no summary — **or**
`Undone` when nothing had streamed and nothing is queued: the submission rolls
back, its message returned to the composer and dropped from history (the loop
purge-repaints, no notice; `docs/interrupt.md`). On any turn-end — `StreamDone`, `Error`, *or* the Esc
interrupt (its `Kept` branch) — the loop pops the front queued batch (`App::drain_next_batch`) and
`start_turn`s it as the next turn (the remaining batches iterate at the
following turn-ends), so **Esc sends the front batch right away**; the drain
runs **under the Ctrl+O overlay too** (codex's queue dispatches at turn end
regardless of its Ctrl+T view, the transcript following the new turn live) —
dispatching only records history and *queues* the user bubbles, which the
return's reflow drops + regenerates, so invariant 4 holds.
`App` (`src/app/`) is pure state +
`on_key` (dispatched per `View`); `Action`, `Role`, `Message`, `StreamError`,
`InterruptedTurn`, `ToolStatus`, `ToolCall`, `TokenArrow`, `TurnStatus`,
`TurnSummary`, `HistoryItem`, `QueuedTurn`, `Toast`, `ToastKind`, `FileSearch`, `ResumePicker`, `View` live there too
(the `@`-picker primitives `AtToken`/`FileMatch`/`at_token`/`fuzzy_match`/`rank_files`
live in the pure `file_search` module, and the `/resume` primitives
`SessionMeta`/`SessionSummary`/`meta_line`/`item_line`/`parse_session`/`preview_of`/
`relative_age`/`rollout_rel_path` in the pure `session` module — `docs/resume.md`).

Typing a bare `/token` opens a **slash-command palette** below the input box (a
third live-region band): `App::command_menu` holds the highlight, the registry
`app::COMMANDS` (`SlashCommand { name, description, effect }` — currently `/help`,
`/clear`, `/copy`, `/init`, `/compact`, `/resume`, `/model`, `/login`, `/settings`, and `/quit`) is filtered by `matching_commands`, and ↑/↓ scroll / Tab+Enter run
the highlighted command. Descriptions line up in a column, and the selection is
shown **by colour** — the whole highlighted row lights up cyan (name *and*
description the same colour) while the others are dimmed grey, no caret. A command
dispatches an `Action`
(`/clear`→`Clear`, `/help`→`Notice(String)` committed as a `Role::System`
message — **but mid-turn `/help` is rejected with a `Toast`** (its list would
interleave with the reply; `docs/toast.md`),
`/quit`→`Quit`, **`/copy`→`Copy(Option<String>)`** — codex's `/copy`:
the pure core picks the last assistant message (`App::last_assistant_text`) and
the loop writes it to the system clipboard, arboard with an OSC 52 fallback for
headless/SSH/tmux, then shows a transient `Copied last message to clipboard` toast
(or a red `No agent response to copy`/`Copy failed` toast) — **a self-clearing
line above the box, not a scrollback bullet** (`docs/toast.md`); the clipboard write is
the I/O boundary, the `base64`/OSC 52 framing a tested pure core in `clipboard`;
see `docs/copy.md`, **and `/resume`→`OpenResumePicker`** — codex's `/resume`:
every conversation records to a rollout JSONL file (the recorder + dir scan at
the boundary, the format/parse in the pure `session` module) and the command
opens a full-screen alt-screen picker (`View::ResumePicker`,
`ui::render_resume_picker` — dense `❯ {age:12}{preview}` rows with the
selection lit on a full-width background tint, type-to-search, and codex's
Filter/Sort toolbar on the search row (`Filter: [Cwd] All   Sort: [Updated]
Created` — Tab moves the focus, ←/→ toggle, `Cwd`/`Updated` default),
Enter → `ResumeSession(path)` loading the file's history via
`App::load_session` and appending later turns to the same file; Esc clears the
query first then closes, Ctrl+C closes, mid-turn `/resume` is rejected with a
transient `Toast` (was a red `ErrorNotice`; `docs/toast.md`) like codex; see
`docs/resume.md`, `smoke.sh` Phase 31. **`/model` and `/login` (the inline
pickers, `docs/llm.md`) now open *mid-turn* too** — they only replace the
composer, never the running turn, so their old busy rejections are gone; their
confirmations are toasts (`smoke.sh` Phase 33).
**`/init`→`Submit(INIT_PROMPT.trim_end())`** — codex's `/init`
(`docs/init.md`): the canned `prompts/init.md` prompt (generate
an `AGENTS.md` contributor guide, never overwriting an existing one) submitted as
a regular user turn — echoed as the `❯` message, recorded, checkpointed — that
the model's agentic tool loop answers by exploring the repo and writing the file;
mid-turn it is rejected with a `Toast` like `/compact` (codex's
`available_during_task = false`), it never records into the ↑-recall history,
and an Esc-undo of the turn restores the literal `/init` (palette reopened),
not the prompt. **The generated guide feeds back into the model's context**
(`docs/project-doc.md` — codex's project doc): every turn start re-reads the
project's `AGENTS.md` files (the pure `project_doc` module — nearest-`.git`
root→cwd discovery, the first of `AGENTS.override.md`/`AGENTS.md` per dir
read bytes-lossily, codex's 32 KiB cap via `ALTER_ZERO_PROJECT_DOC_MAX_BYTES`
with `0` disabling, the `# AGENTS.md instructions … <INSTRUCTIONS>` fragment;
the read is
`tui::turn`'s `Session::start_turn` + a startup seed) into
`App::user_instructions`, and
`context::context_messages_with` injects it as the derived context's leading
user entry — in front of the post-`/compact` shape too, never entering
`history` — so the Ctrl+D view shows it and `App::estimate_context_tokens`
counts it.
**`/compact`→`Compact`** —
codex's manual compaction (`docs/compact.md`): the loop runs the summarization
turn on a one-off tools-free `LlmBackend::configure(cfg, system_prompt,
false)` (the dummy scripts a text-only summary), the reply diverts into
`App::compact_buffer` (never rendered), and `StreamDone` appends the
`HistoryItem::Compaction` marker + commits the cyan `● Context compacted`
cell; mid-turn it is rejected with a `Toast` like `/resume`, an empty derived
context with `Nothing to compact` (`smoke.sh` Phase 50)).
**`/clear` mid-turn is a kill**, not codex's
"disabled while a task is in progress" rejection: `App::clear_conversation`
wipes history, the streaming buffer, the running tool, the status, and the
queued backlog (recording no partial/notice/summary; ↑-recall survives), and
the loop's `Clear` arm cancels + reaps the backend and drains its channel —
the Esc-interrupt dance minus the commits — before the blank repaint, so
nothing can stream into the cleared screen (`smoke.sh` Phase 16). Adding a command later is a one-line `COMMANDS` entry plus an effect arm
in `App::run_selected_command`; the palette/filter/scroll don't change. **Ctrl+C
first clears a non-empty input** (codex's composer-clear: one press empties the
draft — recording it in `App::input_history` so ↑ brings it back, closing the
palette, never touching a streaming turn — and only an empty-input Ctrl+C quits;
an open Ctrl+R search wins over both: that Ctrl+C only cancels the search,
restoring the pre-search draft; the Ctrl+O overlay has no input box, so Ctrl+C
there always quits).

## Working style

Two practices shaped this codebase — TDD and design-first. Follow them as
standing instructions.

### Test-driven development (rigid — this is how the tested crates are built)

The Iron Law: **no production code without a failing test first.** Work in
Red → Green → Refactor cycles:

1. **Red** — write one minimal test naming the behavior you want, then run it and
   **watch it fail for the right reason** (feature missing, not a typo). A test
   you didn't see fail proves nothing.
2. **Green** — write the *minimal* code to pass. No speculative features (YAGNI).
3. **Refactor** — clean up with tests staying green.

If you wrote production code before its test, delete it and re-derive it from the
test. `src/main.rs` + `src/tui/` are the **only** exception (terminal I/O
boundary) — verify changes there by running the app and `scripts/smoke.sh` in
tmux, not unit tests. Every
gate (`fmt`, `clippy -D warnings`, `test`) must be clean before you call work done.

### Design before implementing (for features / non-trivial changes)

"Add X" / "build X" states *what*, not "skip the design." Before non-trivial work:
explore the existing code, ask clarifying questions one at a time, propose 2–3
approaches with a recommendation, and get agreement on a design first. Capture the
agreed design in `docs/` and keep `docs/design.md` updated when behavior changes
(it already documents the architecture and known limitations). Trivial,
self-evident edits and bug fixes with an obvious cause don't need this ceremony —
but bug fixes still get a failing test first (TDD applies to fixes too).

## Conventions

- **All styling is centralized** as `const`s in `ui/theme.rs` — bullets,
  prompt, colours (including the red error bullet and the cyan system bullet),
  border, the tool-call styling (`TOOL_*` — dim-waiting/grey-running/green/red status
  colours (`TOOL_WAITING_COLOR` for a batch's not-yet-run `⎿ Waiting…` calls,
  `docs/parallel-tools.md`), the
  `⎿` peek prefix, the `(ctrl+o to expand)` hint), tool-view chrome
  (`TOOL_VIEW_*`), the thinking stream (`REASONING_*` — it *borrows* the tool
  cell's `TOOL_BULLET`/`TOOL_RESULT_PREFIX` while it runs rather than owning a
  glyph, so its own consts are the dim italic `REASONING_TEXT_COLOR`/
  `REASONING_TEXT_MODIFIER` the chain-of-thought renders in, the
  `REASONING_RUNNING`/`REASONING_DONE` labels, the near-white
  `REASONING_SHIMMER_BASE` the live label's sweep rests at (codex's grey
  `SHIMMER_BASE` would read as dim), the `REASONING_LABEL_COLOR` the settled
  line takes from `STATUS_DONE_COLOR` on **both** surfaces, and the
  `REASONING_PEEK_LINES` live tail window — see `docs/thinking-stream.md`), the transcript timestamp (`TIMESTAMP_COLOR` — the dim
  `hh:mm AM/PM` stamp right-aligned on its own line under the *user* message,
  the only stamp shown, only in the Ctrl+O view), the status
  indicator (`STATUS_*` — the comet spinner's white head + mid-grey
  `SPINNER_TAIL_COLOR` fading tail + dim walls and the
  `SPINNER_FRAMES`/`SPINNER_INTERVAL` animation, dim metrics, the `↓`/`↑` arrows
  and `…` ellipsis, the `STATUS_INTERRUPT_HINT` (`esc to interrupt`, the detail's
  closing clause), the dim committed-summary colour, and `STATUS_ROWS`/`STATUS_GAP_ROWS`;
  the verb's white shimmer wave is the `SHIMMER_*` consts — base/highlight
  colours, sweep period, padding, band half-width, max blend — a port of codex's
  `shimmer_spans`; the verbs themselves are `WORKING_VERBS`/`DONE_VERBS` in
  `app/turn.rs`, picked per-turn), the
  slash-command palette (`MENU_*` — the `MENU_DESC_COL`
  description column, the cyan/dimmed colours that light up the whole selected row
  — name and description alike — and the `MENU_MAX_ROWS` cap), the `@` file
  picker (`FILE_MENU_*` — it reuses the palette's `MENU_SELECTED_COLOR`/
  `MENU_DIM_COLOR`, additionally bolding the query-matched characters; the
  columned row geometry is `FILE_MENU_MARKER`/`FILE_MENU_INDENT` (the selected
  `→ ` and the matching inset), `FILE_MENU_GAP` (name column = widest visible
  name + gap), `FILE_MENU_TYPE_WIDTH` with the `FILE_MENU_FILE_LABEL`/
  `FILE_MENU_DIR_LABEL` kind labels pinned at the right edge, and
  `FILE_MENU_ROOT_DIR` (`./`) for root-level parents, with a
  `FILE_MENU_MAX_ROWS` cap and the `FILE_MENU_SEARCHING`/`FILE_MENU_NO_MATCH`
  placeholder rows; `file_menu_rows`/`file_menu_lines`/`file_menu_row` mirror the
  palette helpers — see `docs/file-search.md`), the `?` shortcuts
  band (`SHORTCUTS*` — the entry list, the second-entry column, and the cyan
  key / dim label colours), the inline `/settings` menu (`SETTINGS_*` — it
  reuses the `/model` picker's frame, indent, `❯` prompt, `→` marker and cyan
  selection, adding only the value column's geometry (`SETTINGS_VALUE_GAP`,
  sized off the widest visible label) and its two-tone colouring
  (`SETTINGS_VALUE_COLOR` for a live value, `SETTINGS_VALUE_OFF_COLOR` for the
  `SETTINGS_OFF_VALUES` — `false`/`default`/`0`/`disabled` — and anything
  unavailable), the `SETTINGS_HINT` key line, `SETTINGS_NO_MATCH`,
  `SETTINGS_MENU_MAX_ROWS`, and the `SETTINGS_CHROME_ROWS`/`SETTINGS_SEARCH_ROW`
  geometry `settings_height`/`cursor_position` share — see `docs/settings.md`), the queued entries (the `QUEUED_INDENT` two-space
  inset, `queued_rows`/`queued_lines` — uncapped; a text `Messages` batch
  rendered by `message_lines(Role::User…)` and a standalone `Shell` command by
  `message_lines(Role::Shell…)` (the red `! ` header), so they reuse the
  user-/shell-message style, with a blank row dividing each entry from the next),
  the
  session footer (`FOOTER_*` — the two-space `FOOTER_INDENT`, the ` · `
  `FOOTER_SEPARATOR`, the dim `FOOTER_COLOR`, plus the cyan
  `FOOTER_FOCUS_BG`/`FOOTER_FOCUS_FG` that light the ↓-focused shell-count
  segment (`docs/background.md`); `footer_rows`/`footer_line`,
  ellipsis-truncated at narrow widths, with `display_cwd` formatting the
  `~`-relative path), the transient toast row above the box (`TOAST_*` — the
  two-space `TOAST_INDENT`, the dim `TOAST_COLOR` (info) / red `TOAST_ERROR_COLOR`
  (failure); `toast_rows`/`toast_line`, ellipsis-truncated like the footer — see
  `docs/toast.md`), the Ctrl+R search line that takes the footer's slot while
  a search is open (`SEARCH_*` — the dim `SEARCH_PROMPT`, the cyan
  `SEARCH_QUERY_COLOR` shared by the bold accept/cancel hint keys, the red
  `SEARCH_NO_MATCH` notice, and `SEARCH_HIGHLIGHT` — the reversed+bold styling
  of the query occurrences in the previewed match; `search_line`, the
  query-end cursor in `cursor_position`, `highlight_row_spans`), the Esc-Esc
  backtrack (`BACKTRACK_*`/`SHORTCUTS_BACKTRACK`/`TOOL_VIEW_HINT_BACKTRACK` —
  the primed `esc again to edit previous message` hint that takes the same
  footer slot (`backtrack_hint_line`), the preview's reversed user-message
  highlight + scroll target from the single `transcript_build` walk
  (`transcript_selection`/`backtrack_scroll`), and the overlay's swapped
  key-hint row; see `docs/backtrack.md`), the `!`
  shell mode (`SHELL_MODE_*`/`SHELL_BULLET` — the red `Shell mode` footer
  hint (`shell_mode_line`) and the red `! ` that doubles as the composer
  prompt while `App::shell_mode` is on and as the `Role::Shell` exec-cell
  header bullet in `message_lines`; shell `tool_lines`/`tool_full_lines` are
  headerless `⎿` blocks — inline up to `TOOL_PEEK_LINES` aligned source
  lines, each fully wrapped
  (`result_row` does the corner/continuation indent; a line wider than the
  terminal **word-wraps with spaces preserved** like the Ctrl+O view
  (`wrap_output`, via `result_peek_block`)
  rather than clipping, the `TOOL_PEEK_MAX_ROWS` display-row ceiling keeping
  one huge line from ballooning the cell) then `… +N lines (ctrl+o
  to expand)`, `⎿ Running…` live, the retained output uncapped in the Ctrl+O view;
  output over `tui::shell`'s `SHELL_OUTPUT_MAX_BYTES` is **capped in memory** as it's
  read (`tui::shell::append_capped`, codex's pattern — bounds peak RSS so `! tree ~/`
  can't spike memory; the dropped tail is gone, not saved) and the expanded cell
  appends a dim `TOOL_TRUNCATED_MARKER` (`…`) when `tool.truncated` —
  kept flush by `conversation_lines`), and
  the live-region row geometry (`GAP_ROWS`/`STATUS_ROWS`/`STATUS_GAP_ROWS`/`INPUT_CHROME_ROWS`/`LIVE_MIN_HEIGHT`;
  the status + gap strip shows *while a turn is active*, and the preview + gap
  is added *only when there's content to preview* (`strip_rows(streaming,
  preview_rows)`/`preview_rows` — the **count** of preview content rows: 0 idle,
  1 for a streaming reply or `!` shell run, N for a running backend tool's whole
  cell (or the whole parallel `tool_queue`: every batched call's cell, running +
  `⎿ Waiting…`, blank-separated — `docs/parallel-tools.md`); the pre-stream pause
  reserves **no** empty preview row, like codex) —
  (`render_live` draws the status line under the preview's gap, or at the strip
  top during the pause) — with the
  **queued messages stacked below the status, *above* the box** (`queued_rows`,
  user-message style), the
  command palette *or* the shortcuts band forms the band *below* the box —
  `menu_rows` + `shortcuts_rows` — and the session footer takes the region's
  **last** row whenever no band is open (`footer_rows` — the band displaces
  it) — so the box's
  dynamic `live_height` is streaming-, queue-, band- and footer-aware, and idle with no band
  there is exactly one blank above the box: the committed spacer after the last
  message. `render_live` and `cursor_position` share the `input_box` helper, which
  reserves the band and footer so the cursor stays put when they open; `tool_lines`
  and `tool_view_lines` share `tool_header`). Retheme or re-size there, not inline.
- **All width math goes through `cols()`** (display columns via `unicode-width`),
  never `chars().count()` — so CJK/emoji wrap and pad correctly. Measuring right
  is only half of it: a **wide glyph occupies one `Buffer` cell plus a blank
  shadow** for each column it covers, and ratatui only skips those shadows inside
  `Buffer::diff`. **Every** paint that hands cells to `Backend::draw` directly
  must go through `term::visible_cells` — there are three (`draw_lines` for
  scrollback + `reflow`, `blit` for the live region, `draw_overlay` for the alt
  screen), and missing any one leaves the bug alive in that view alone. Printing
  a shadow spends a third column on a two-column glyph, shifting the rest of the
  row: the emoji that tore a table's right border off the grid
  (`docs/table-streaming.md` *Wide glyphs*, `smoke.sh` Phase 41).
- **The input line is a `textarea::TextArea`, not a `String`.** Route all editing
  through it (`insert_char`/`delete_backward`/`move_*`/`take`/…), never raw string
  `push`/`pop`; read it with `.text()`. Its cursor is a byte offset on a grapheme
  boundary and the wrap cache is filled by the render path (`wrapped_rows`), which
  is why `App::on_key` (and so `move_up`/`move_down`) stays width-agnostic. The
  textarea wraps faithfully (preserving spaces) into byte ranges — distinct from
  `ui::wrap_text`, which is for **messages** and collapses whitespace. See
  `docs/textarea.md`.
- **Swapping in a real AI** means implementing `stream::ReplySource` (use `DummyAi`
  as a template) and changing the single `let backend = …;` line in
  `tui::event_loop::run`. `spawn(prompt, images, tx, cancel)` hands you the text prompt
  **plus** the paths of any Ctrl+V-pasted images (`images: Vec<PathBuf>` — codex's
  `UserInput::LocalImage` typed channel; a real vision backend reads each file and
  attaches it, the dummy only acknowledges the count — see `docs/image-paste.md`).
  Stream `StreamEvent::Chunk(..)` per token on the `tokio`
  `UnboundedSender` (its `send` is sync — callable straight from your background
  thread, no runtime needed), poll the `CancelToken` so a quit can stop you, then
  send `StreamEvent::StreamDone` — or `StreamEvent::Error(msg)` on failure. For tool calls, optionally
  stream `StreamEvent::ToolCallDelta(fragment)`s as the model *generates* the call
  (its `name`/`arguments` pieces — counted like reasoning so the tally ticks while
  the call is produced, the text never shown), then send a
  `StreamEvent::ToolStart{name,args}`, optionally stream `ToolOutput(chunk)`
  live-output deltas while the tool runs (the running cell tails them —
  `docs/tool-streaming.md`; the real `bash` executor streams completed lines),
  then a `ToolEnd{output,ok,truncated}`
  (`truncated: false` from a backend tool — only the `!` shell runner caps); wrap a reasoning
  phase in a `ThinkingStart`/`ThinkingEnd` pair to drive the `Thinking for Ns`
  status, streaming each reasoning delta as a `ThinkingChunk(text)` in between so
  the token tally keeps ticking while the model thinks (the text is never shown —
  only counted; see `stream::turn_events` for the dummy's interleaved script). Return
  your real model id from `model_name()` — the session footer under the box
  displays it. A real backend's own first-token latency replaces `DummyAi`'s
  artificial `STARTUP_DELAY` (the deliberate pre-stream pause that shows off the
  status indicator); the loop already counts the user's input into the tally
  (`↑`) at turn start, so the status reads `↑ N tokens` until your first chunk.
  The loop and
  rendering treat chunks and tool output as opaque text, and count the status
  tokens app-side with a real `tiktoken` `o200k_base` tokenizer via the
  `app::count_tokens` → `tokenizer::count` seam — exact for OpenAI models,
  close for the rest — **as the live estimate between usage frames**: a real
  backend forwards each round's final `usage` frame as `StreamEvent::Usage`
  and `App::apply_usage` snaps the tally to the provider's own accounting
  (the `Done for Ns` summary appending `· {n} tokens ({c} cached)`), and every
  request is shaped for **prompt caching** — `llm::cache`'s `cache_control`
  breakpoints on the models that need them, a per-session `prompt_cache_key`
  (+ OpenRouter `session_id`) for affinity, `stream_options.include_usage` in
  the payload — see `docs/prompt-caching.md`.
  **The real `LlmBackend` also drives an agentic tool loop** (`docs/tools.md`):
  it offers the model `bash`/`read`/`write`/`edit` as Chat Completions function
  tools, and `llm::agent::run_agent` streams a round, runs the tools the model
  requested (emitting the same `ToolStart`/`ToolEnd` events the dummy scripts,
  via the `llm::exec::ToolExecutor` seam), feeds the results back, and loops
  until the model answers with plain text. The pure pieces — the tool defs +
  edit engine (`llm::tools`), the streamed `tool_calls` accumulator
  (`llm::openai::ToolCallAccumulator`), and the loop itself — are unit-tested;
  the executor's file/process I/O is boundary code. Tools are on by default,
  off via `ALTER_ZERO_TOOLS`. A `read`/`edit`/`write` cell renders its output as
  a **numbered file change** (codex's `diff_render` look in the `⎿` gutter —
  `ui/file_cell.rs`'s `file_cell_lines`): the executor emits `Created {path} ({N} lines)`
  over the numbered contents, `Updated {path} (+A -D)` over numbered diff
  **hunks** (3 context lines, `⋮` between distant hunks — the pure
  `tools::render_numbered_content`/`render_numbered_diff`), or — for `read` —
  the file numbered by `tools::format_read` in the **same** `{n:>W} {text}`
  gutter (dynamic-width numbers + a space, not the old `cat -n` tab; the UI
  synthesizes the `Read {N} lines` corner; a `read` of an **image**
  (png/jpg/jpeg/gif/webp) instead returns a small `Read image {path} (…)` fact
  line while the pixels ride `ToolOutcome::image` as a base64 `data:` URL —
  `run_agent` attaches them after the round's tool results as a user-role parts
  message (tool-role content rejects image parts on most providers) and
  `context_messages` replays the same note on later turns from the output
  marker, the path re-encoded per request like a Ctrl+V paste; and a model
  whose `/v1/models` record says it **can't** see images (`ModelEntry::vision`
  — detected beside the reasoning support, riding the selection into
  `ModelConfig::vision` and `config.json`) degrades gracefully instead of
  letting the provider 404 the turn: the `read` tool declines the image with a
  recoverable error, attachments become `[image omitted: …]` notes in
  `build_messages_for`, and a Ctrl+V paste raises a red toast;
  `docs/tools.md` "Image reads"/"Vision detection"). The cell re-styles those rows — dim
  line numbers, green/red signs (`read`/`created` have none), the content
  syntax-highlighted by the path's extension, added/removed rows on
  dark-green/red background tints (`TOOL_DIFF_*_BG`), a 10-row inline peek
  (`FILE_PEEK_LINES`) with the `… +N lines` hint, everything in Ctrl+O;
  unparseable output (old rollouts, error bodies, a `read` placeholder) keeps
  the legacy rendering. The backend's **system prompt** is assembled from three
  `include_str!`d markdown files — the persona (`prompts/alter_zero.md`), the
  runtime **environment context** of date/os/cwd (`prompts/environment.md`,
  folded in at the boundary via `backend::augment_with_environment` so the agent
  has context awareness), and the tools note (`prompts/tools.md`) — in the order
  persona → environment → tools (`docs/environment.md`).
