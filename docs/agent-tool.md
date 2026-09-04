# The `Agent` tool — Claude-Code-style subagents

The model can now **launch subagents**: autonomous side-conversations that run
their own agentic tool loop (`bash`/`read`/`write`/`edit`) over their own
context, report progress live in the TUI, and hand their final response back to
the main agent as the tool result. It is Claude Code's `Agent` (Task) tool,
adapted to this inline TUI — including the parts Claude Code doesn't have: the
user can **enter a subagent's own inline session and chat with it**.

```
❯ Call two agents to fetch weather and time in Manila and Warsaw

● I'll spawn two agents...

● 2 background agents launched (↓ to manage · ctrl+o to expand)
   ├ Fetch current weather and time in Warsaw
   └ Fetch current weather and time in Manila

( ●•·  ) Working… (4s · ↓ 120 tokens · esc to interrupt)

──────────────────────────────────────────────────────────────
❯
──────────────────────────────────────────────────────────────
  kimi-k3 medium · ~/repo · 1.8k/1M (0.2%)

  ● main
  ◯ general-purpose  Fetch current weather and time in… 48s · ↓ 16.5k tokens
  ◯ general-purpose  Fetch current weather and time in… 48s · ↓ 15.9k tokens
```

## The tool

Declared beside `bash`/`read`/`write`/`edit` (`llm::tools::agent_spec`), main
backend only — a subagent never gets the `agent` tool, so agents can't nest:

| param | | |
| --- | --- | --- |
| `description` | required | a short (3-5 word) task label — the tree rows / footer list show it |
| `prompt` | required | the full task for the agent to perform |
| `subagent_type` | optional | which **agent definition** to launch — `general-purpose` (the default) and `explore` ship as `agents/*.md` files, and a project or the user can add more; the available types and their tools are named in the `<system-reminder>` listing (`docs/subagents.md`). An unknown type resolves as a recoverable error listing the real ones |
| `run_in_background` | optional | **default `true`** — the call returns at once with the launch acknowledgement (the agent named by its description; no id — nothing model-facing takes one back); `false` blocks the turn until the agent finishes and returns its final response |

(The reference schema's `isolation` param is deliberately not implemented —
out of scope for this TUI. Its `model` is here, but as a property of the
*definition* rather than of the call: which model a type runs on is a
standing choice about that type, not something to re-decide per launch.)

A subagent's conversation starts fresh: its type's system prompt — the
definition's own body when it has one, else the session's
persona/environment prompt — plus a subagent note (`prompts/subagent.md` —
"your final message is returned to the caller"), then the `prompt` as the
first user message. Its **tool set** and its **model** come from the same
definition (`docs/subagents.md`). It runs
`llm::agent::run_agent` with its own executor and its own cancel token, on its
own thread. The executor carries the **shared background registry** too, so a
subagent's `bash` can `run_in_background` like the main turn's — the shell
joins the same list (stacking into the footer's `· N shells` count in every
session view) attributed to its launcher via `BgOrigin { agent_id,
agent_type }`: the ↓ manager's details page gains a `From: {type} agent`
field, the completion notice reads `· from the {type} agent`, and the context
note says `launched by the {type} agent`. Only the **Ctrl+B latch** stays
main-only — a subagent's executor neither clears nor consumes it, so the
handoff always belongs to the main turn's foreground command. A completion
routes to its launcher first: the boundary queues the note onto the launching
agent's steer seam (`AgentRegistry::queue_input` — heard at its next round
boundary, `docs/queue.md`) and does **nothing else**, the `StreamEvent::Steered`
echo being what records it on that agent's transcript; if the launcher already
settled, the note falls to the shared board instead (the main turn /
automatic-follow-up path), so someone always hears the outcome.

## Protocol

Two layers, mirroring background shells (`docs/background.md`):

- **The reply channel** carries the *group lifecycle* inside the parent turn:
  - `StreamEvent::AgentBatch { background, agents: Vec<AgentSpec> }` — the
    round's `agent` calls announced up front (id, description, type, prompt).
    The loop seeds the roster (`App::agent_started`) and the live group cell.
  - `StreamEvent::AgentGroupDone { group: AgentGroupResult }` — the group
    resolved (all foreground agents finished, or the background launch
    acknowledged, or Ctrl+B moved the rest to the background). Carries the
    final per-agent snapshots so the committed cell can never race the agent
    channel. The loop records `HistoryItem::AgentGroup` and commits the cell.
- **The agent channel** (`agents::AgentEvent`, a dedicated `select!` source —
  agents outlive turns) carries each subagent's own stream, tagged by id:
  `AgentEvent::Stream { id, event }` for every `StreamEvent` the subagent's
  `run_agent` emits (chunks, tool batches, usage, done/error). The loop folds
  them into the roster entry (`App::apply_agent_event` →
  `agents::AgentRun::apply`), which accumulates the agent's **own transcript**
  (`Vec<HistoryItem>`), live tool queue, token tally, tool-use count, and
  status — everything the footer list, the Ctrl+O cells, and the agent
  session view render.

`llm::agent::run_agent` gains one closure, `run_agents(&[ToolCallRequest]) ->
Vec<(call_id, output)>`: a round's calls are partitioned (`agent` vs the
rest); agent calls are handed to the closure (which launches them all
concurrently, emits the events above, waits for foreground completion —
polling the parent cancel and the registry's Ctrl+B latch — and returns each
call's tool result); the rest execute exactly as before; results append in the
original call order. The closure is a no-op fake in the unit tests and
`llm::backend`'s real launcher in production.

## The registry (`agents::AgentRegistry` — boundary, like `background`)

A cloneable handle shared by the loop and the backend: allocates
Claude-Code-style ids (`a` + 8 base36 chars), keeps each running subagent's
`CancelToken` + completion flag + final message list, and owns the per-agent
**pending-input queue** (the chat feature). `kill(id)` cancels the token and
marks the completion `killed` so the parent's wait loop resolves at once
(without waiting for the thread to notice); `kill_all()` sweeps on
`/clear`/quit. Completed foreground results are read off the shared slot.

Background completion notices ride the **existing**
`BackgroundRegistry::post_notice` board (`from_model: true`), so the in-flight
agent hears a finished subagent within the same turn and
`dispatch_after_turn`'s automatic follow-up turn covers idle completions with
zero new plumbing. The TUI cell is a new `HistoryItem::AgentNotice` —
`● Agent "{description}" finished · {elapsed}` (green — the runtime
`format_elapsed`-humanized, `6m 2s` past a minute) / `was stopped by user` /
`failed` (red) — deferred to the same safe boundaries as shell notices
(`App::pending agent completions`, settled beside `settle_bg_completions`).

## App state

- `App::agents: Vec<agents::AgentRun>` — the roster. Fed by `AgentBatch`
  (create) and the agent channel (update). A finished agent **lingers** with a
  coloured `◯` (green done / red stopped/failed), then the boundary sweeps it
  (timed in `main.rs` like the toast) — deferred while the user is inside that
  agent's session. How long is the entry's own
  `AgentRun::linger()`: `AGENT_LINGER` (30s) for a natural finish — a row
  swept in a few seconds could vanish before the user had read it, and (the
  expiry being armed from the agent's *own* settle) even before its group
  cell committed — and `AGENT_STOPPED_LINGER` (also 30s) once the user's `x`
  stopped it (`AgentRun::stopped_by_user`), because a row that vanishes under
  the keypress leaves no evidence of what was stopped. Every arming site reads
  that method — the settling event, the group resolution (`or_insert`, so a
  group resolving *after* a stop can't restart or shorten its countdown), the
  per-frame re-arm — so no path can downgrade a stop to the short linger. A
  second `x` **clears** the row before either deadline (`AgentStop::Cleared`
  → `hidden`, the entry itself collected by the next sweep so a live group's
  resolution can still snapshot it) — and clearing the agent whose **session
  view** is open closes that view too: the view renders from the entry the
  sweep is about to drop, and a transcript left up over a roster that no
  longer lists it (marking no session as in view) is the broken middle state.
- `App::agent_group: Option<AgentGroupLive>` — the live group cell (ids +
  background flag), shown in the preview strip while the parent turn runs.
  Esc-interrupt / `fail_stream` resolve it locally (statuses → Interrupted)
  because the channel swap drops the backend's own `AgentGroupDone`.
- `App::agent_selection: Option<usize>` — the ↓ roster navigation: `0` is the
  `● main` row, `i+1` the i-th agent. ↓ from an idle composer steps onto the
  **shell indicator first** when one is lit-able (the existing
  `background_focus`), a second ↓ moves into the roster; with no shells ↓
  goes straight to the roster. ↑ walks the same path back: from the `● main`
  row it lands on the shell indicator when a shell is running (a second ↑
  there returns to the composer), else exits directly — so ↑/↓ traverse
  composer ⇄ indicator ⇄ roster symmetrically. ↑/↓ move, `Enter` views
  (main = leave the view / close), `x` **stops** the selected agent — and,
  on a row that has already settled, **clears** it (below), Esc
  dismisses, any other key falls through after clearing (the
  `background_focus` contract). The footer line swaps to the hint
  (`↑/↓ to select · Enter to view` on main, `Enter to view · x to stop` on a
  running agent, `x to clear` once it has settled) while the selection is
  active.
- `App::agent_selection_memory: Option<String>` — the **last picked** row, so
  the roster is a cursor rather than a menu that reopens at the top: ↓ lands
  on the remembered agent (`App::agent_selection_start`, consulted by both
  entry points — the composer's ↓ and the shell indicator's second ↓)
  instead of walking from `● main` every time. Both halves of the walk write
  it: the ↑/↓ steps, and `open_agent_view` (entering a session **is** a pick,
  which is the reported flow — Enter to read an agent, ↓ to come straight
  back to its row). It holds the **id**, not the index, so a roster that grew
  or shrank underneath still resumes on the same agent, and a remembered
  agent that is gone falls back to `● main`. Walking the `❯` onto `● main` is
  how you forget one (`remember_agent_selection(0)` clears it) — an explicit
  move to `main` must not be overridden by a memory, which is also why the
  open session view is *not* a second fallback: it already set the memory
  when it opened.
- `App::agent_view: Option<String>` — the **agent session view**: the whole
  inline screen shows that agent's own conversation (banner + its transcript,
  Purge-rebuilt like `/clear`), the input box's top rule carries the agent's
  description as a right-aligned label **embedded in the rule** — the rule
  resumes for one border cell after the text (`── {description} ─`, the
  `AGENT_VIEW_RULE_TAIL` glyph) so the label reads as part of the frame
  rather than dangling off its right end, and **clipped to half that rule**
  (`agent_view_rule_label`, `width / AGENT_VIEW_LABEL_DIVISOR`, the two
  padding spaces and the tail glyph counted in) with the rest of the
  description cut by `TOOL_HEADER_ELLIPSIS`: a `description` is the model's
  own sentence and can run the width of the terminal, and ratatui skids an
  over-wide **right**-aligned title off its **left** end, so an unclipped
  label ate the whole frame *and* lost the head of the very text it was
  showing — `─── An agent tasked with confirming it… ─`, not `h confirming
  its status and acknowledging the requested description length ─`. A rule
  that cannot hold the mark **and** one real character beside it carries no
  label at all (a lone ` … ─` names nothing, and the frame is worth more than
  the hint) — though a description that *fits* still rides at that width,
  since the mark is what needs the room and nothing is cut. And
  the composer **chats with the agent**, and it is the **main session's
  mid-turn queue one level down**
  (`docs/queue.md`): while the agent runs, Enter parks the draft on its own
  queue — shown above the box as the same inset `  ❯ …` row, `AgentRun::queued`
  — and its loop takes it at the next round boundary (`run_agent`'s
  `pending_inputs` seam), where `StreamEvent::Steered` on the agent channel
  turns it into a real user message **on that agent's transcript**, after the
  streamed segment ahead of it is finalised. When the agent is idle instead, a
  continuation turn spawns over its stored message list carrying the draft as
  its newest user turn, and the transcript records it at once — an idle submit.
  **The registry decides which**, not the roster: `spawn_agent_chat` returns an
  `AgentChatDelivery` (`Queued`/`Started`/`Declined`), because a roster status
  one event behind the registry would either strand the pending row forever or
  record the message twice. A run that settles with messages unread hands them
  back (`Session::reclaim_agent_chat`): a **naturally finished** agent gets them
  straight back as a continuation, while a failed or `x`-stopped one keeps
  nothing — there is nothing to continue, and restarting an agent the user just
  killed is the opposite of what the key meant. **Tab is the other half**, and
  it too is the main session's key one level down: it queues a **follow-up
  turn** for that agent (`AgentRun::followups`, `App::queue_agent_draft`),
  rendered below the steered rows and blank-divided from them, which
  `Session::dispatch_agent_followups` starts as its own chat continuation once
  that agent's loop settles — one entry per settle, in submission order,
  through the same `agent_chat` seam Enter's idle path uses. It asks
  `ReplySource::agent_ready_for_turn` (the registry, never the roster) before
  handing one over, so a follow-up cannot be folded into the settle-window
  continuation as a steer; a stopped agent drops them with its steered rows;
  and **Alt+Up** there is the main session's two steps over that agent's own
  sets — its last follow-up, then (via `ReplySource::reclaim_agent_input` →
  `AgentRegistry::take_last_input`, `None` once its round boundary has read
  it) the message still waiting — reaching the main session's backlog never. Tab used to read the **lead's** stream and push onto
  the **lead's** queue, so a message typed into a subagent ran as a follow-up
  turn of the main conversation (`docs/queue.md`). Esc (empty
  composer) returns to the main session (Purge-rebuild of the main
  transcript, mid-stream partial included). While the view is up the main
  turn's commits are suppressed exactly like the Ctrl+O overlay (invariant
  4); the viewed agent's events commit incrementally through a dedicated
  `StreamRender` — **and its strip previews that same render's frontier**, so
  a forming table or a fenced code block streams there exactly as it does in
  the main view, its live `● Thinking…` block included
  (`docs/agent-view-streaming.md`) — and its turns **end the way the main session's do**: the
  settle records a dim `Done for 59s · 6.1k tokens (2.8k cached)` summary on
  the agent's own transcript (`AgentRun::apply`'s StreamDone arm pushes the
  `HistoryItem::Summary`, its `tokens`/`cached` the **turn's** billed usage —
  `AgentRun::turn_usage_*`, reset by `reopen()` so each chat continuation
  gets its own receipt while the roster tally stays cumulative; the runtime
  was frozen at its live value just before the settling event, so `secs` is
  the turn's elapsed), the view commits it at StreamDone
  (`Session::commit_agent_view_event`), every rebuild and the Ctrl+O agent
  transcript render it from history, the derived context skips it as chrome
  like any summary, and a failed or interrupted run records none (main
  parity — the red notice is that record). The composer keeps its **full functionality** inside the
  view: the `/` palette (commands act on the main session, as everywhere),
  the `?` shortcuts band, Ctrl+R history search, and the `@` file picker all
  work with the roster still below them; **Ctrl+O shows the viewed agent's
  own transcript** (a fresh bounded build — `ui::agent_transcript_lines` —
  the main cache untouched) and **Ctrl+D its derived context**
  (`ui::context_lines` branches on the viewed agent: the body derives from
  the agent's transcript, the `system prompt:` block shows the prompt a
  subagent is *actually sent* — the main prompt + the subagent note,
  `ReplySource::agent_system_prompt()` injected at the boundary as
  `App::agent_system_prompt` — and no AGENTS.md fragment, since subagent
  conversations start without one; `docs/context.md`), **and Tab from there
  its own classifier context** — the launch prompt it was given plus the
  calls it has made, the block its own auto-mode verdicts are reviewed
  against, read from its registry slot rather than from the lead's backend
  (`AgentRegistry::classifier_context`, `docs/permissions.md`); only `!` shell
  mode stays off — a leading bang is literal chat text. Overlay returns and
  resizes repaint the agent view (`tui::view::Session::repaint_active_view`).

## Rendering (`ui`)

- **Live group cell** (`live_agent_group_lines`): a breathing-grey `● Running {n}
  agents… (ctrl+o to expand)` over the tree —
  `   ├ {description} · {n} tool uses · {tokens} tokens` with a
  `   │ ⎿  {activity}` status row per agent — plus the delayed
  `(ctrl+b to run in background)` hint (foreground only). The activity is
  **sticky**: `Initializing…` until the first event, then the newest tool's
  `{Name}: {detail}` — **one grammar for every call** (`agents::activity_line`):
  a `bash` call's model-supplied `description`
  (`Bash: Fetching current weather…`), else the call's args summary
  (`Write: game.py`, `Read: ~/notes/today.md` — a file tool's path shown
  by the same relative / `~`-relative / absolute rule as its cell header:
  the run keeps the newest call as it came (`AgentRun::last_call`) and the
  renderer asks `activity_shown` for the row with the session's rule
  applied, `docs/tools.md` *Path display*; never the cell header's
  `Write({args})`, whose parens read as clutter on a dim clipped
  one-liner),
  and an MCP call as the **capitalized server over the tool**
  (`Deepwiki: ask_question` — the full `deepwiki - ask_question (MCP)`
  display name is a mouthful for this row); a call with nothing to say
  after the colon shows the bare name — held between calls (never dropping
  to `Working…`) so the row keeps its context while the agent reasons over
  a result (`StreamEvent::ToolStart` carries the `detail`;
  `AgentRun::last_call`). A **lone** agent renders the tool-cell shape
  instead of a one-row tree: `● Agent({description})` over that same one
  `⎿ {activity}` row — `⎿ Initializing…`, then
  `⎿ Bash: Fetch public repos for linuztx` / `⎿ Bash: curl -s https://…`.
  The strip's `preview_rows`/`preview_lines` size and draw it like the tool
  queue.

  Every activity row — the tree's and the lone cell's alike — is **one dim
  row, clipped at the width** (`agent_activity_row` over `clip_cols`, the
  cut marked `…`; red via `agent_status_color` once an agent has failed or
  been stopped). A lone agent's *running* call used to break that shape here
  alone: its white `Bash(…)` header wrapped over several rows above a dim
  `Running…` line, which read as the
  main turn's own running cell and grew the strip under counters that tick
  every frame. The bullet's breathing grey already says the call is running,
  so the row says *what* is running and nothing more.
- **The session view's strip** (`agent_view_preview_lines`): the main strip's
  branches over the viewed agent's own state, in the same order — its live
  tool cells (the batch queue blank-separated, a running `bash` **tailing its
  streamed output** through the shared `ui::live::live_call_lines`), else its
  open thinking block (`live_reasoning_lines`), else the **uncommitted
  frontier** of its reply, which the boundary builds with the same
  `agent_render` its commits use and injects the height of
  (`App::set_stream_preview_rows`, read back by `agent_preview_rows`). See
  `docs/agent-view-streaming.md` — previewing one batch-rendered row instead
  was the "streaming disappears in the subagent TUI" bug.
- **Committed cells**: `● {n} background agents launched (↓ to manage ·
  ctrl+o to expand)` over description-only tree rows (green); `● {n} agents
  finished (ctrl+o to expand)` over the counted tree rows with `⎿ Done` /
  `⎿ Interrupted` / `⎿ Failed` per agent (green when all done, red
  otherwise). A **lone** agent commits as `● Agent({description})` over
  `⎿ Done ({n} tool uses · {tokens} tokens · {elapsed})` (the runtime
  humanized) (or the red `⎿ Interrupted`/`⎿ Failed`) and a dim `(ctrl+o to
  expand)` line — a lone background launch keeps `⎿ Running in the
  background (↓ to manage · ctrl+o to expand)`, the ctrl+o half teaching
  that the launch cell expands to the agent's prompt/tools/response in the
  transcript.
- **Ctrl+O**: each `AgentGroup` entry expands as its own cell —
  `● Agent({description})` / `⎿ Prompt:` (indented block) / the nested tool
  headers the agent ran (`Bash(curl …)`, a file tool's path shown by the
  cells' relative / `~`-relative / absolute rule — `Write(notes.md)` for the
  recorded `Write(/home/u/repo/notes.md)`: `app::tool_header_text` writes
  the one-liner and `app::file_tool_header` is its inverse, so the cell
  reads the path back without ui re-parsing the grammar; `docs/tools.md`
  *Path display*) / `⎿ Response:` (the final text) /
  `⎿ Done ({n} tool uses · {tokens} tokens · {elapsed})` or `⎿ Interrupted`. The
  **live tail** walks `App::agent_group` + the roster the same way (activity
  `Running…`), and the `TranscriptSig` fingerprints the roster generation so
  a streaming agent invalidates the cache.
- **Footer roster** (`agent_list_lines`): while agents exist, the live region
  gains rows below the footer — a blank spacer, the main row, then per agent
  `  {type}  {description trimmed}… {elapsed} · ↓ {tokens} tokens` (dim
  type, live counters, the elapsed humanized). The **filled `●` + bright
  bold row mark the session in view**: `● main` over dim `◯` agent rows
  normally, and inside an agent's session view that agent's row takes the
  `●` + highlight while main demotes to a dim `◯` (the user-report fix —
  the highlight used to stick on `main`). The `❯ ` marker belongs to the
  active ↑/↓ selection alone and leaves with it when Enter/Esc hand the keys
  back to the composer. A finished agent's bullet turns green/red for the
  30s linger window — a user `x` is what turns it red, and the row stays
  there (with the hint's `x to clear`) until the second `x` or the 30s
  sweep. The
  rows are a fifth `live_layout` area, so `live_height`, the cursor seat, and
  the overlays are untouched.

## Invariant notes

- Subagent events ride their own channel and **never** commit to scrollback
  directly — the roster is state; cells commit only at the group's reply-
  channel boundaries (or, in the agent session view, through that view's own
  render). Invariants 1-4 hold unchanged.
- The parent's tool results replay on later turns via a `context.rs` arm:
  `HistoryItem::AgentGroup` derives one assistant `tool_calls` entry (one
  `agent` call per entry, arguments reconstructed) + one `tool` result per
  agent; `AgentNotice` derives the bracketed user-role note carrying the
  final response.
- `session.rs` round-trips both new items (`agent_group` / `agent_notice`
  records); old builds skip them (the forward-compatibility contract). The
  roster itself is ephemeral, like background shells.
- The session view **commits what the fold recorded**, not what event
  arrived: `Session::commit_agent_tail` compares the transcript's length
  across `AgentRun::apply` and commits the newly-recorded `Tool`/`Summary`.
  Keying on the event is what let `write`/`edit` fall through when they moved
  onto `ToolAnswered` (`docs/agent-view-streaming.md`); the local `x` stop,
  which carries no event at all, goes through the same helper.
- A subagent's **thinking phase** settles onto its own transcript as a
  `HistoryItem::Reasoning` (`AgentRun::begin_reasoning`/`finish_reasoning`,
  the phase's elapsed boundary-injected from
  `Session::agent_thinking_clocks`), so the session view, its Ctrl+O
  expansion and the roster's token snap all behave like the main session's —
  gated by the same `/settings` **Hide thinking** knob
  (`docs/agent-view-streaming.md`).
- `/clear` and quit `kill_all()` agents; an Esc interrupt kills only the
  in-flight **foreground** group (background agents keep running, like
  background shells).
