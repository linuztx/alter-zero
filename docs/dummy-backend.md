# The dummy backend and its scenario registry

`DummyAi` is the built-in offline backend. It is what runs with no provider
configured, and — more importantly — it is the only backend `scripts/smoke.sh`
can drive deterministically. So every UI feature that would otherwise need a
live model has a **scripted turn** here: a parallel tool batch, a subagent
group, a streaming markdown table, four permission round trips, `/compact`'s
summary.

That makes the dummy an odd thing: a demo backend that is also the test rig for
half the program. It grows a new turn every time a feature lands, and it lived
in `src/stream.rs` alongside the wire protocol every *real* backend speaks.

## What was wrong

The dummy was 1,100 lines in the middle of the 2,459-line file that also
defines `StreamEvent`, `ReplySource` and `CancelToken`. Reading the seam meant
scrolling past canned `ping` output; adding a demo meant editing the file that
defines the wire format.

Selection was worse. Which demo plays was decided by **two hand-written
`if`/`else` chains in two different places**:

```rust
// in DummyAi::spawn — the gated demos
if let Some(gate) = permissions.filter(|_| prompt.to_lowercase().contains("permission")) {
    if prompt.to_lowercase().contains("auto")            { dummy_auto_permission_turn(…) }
    else if prompt.to_lowercase().contains("parallel")   { dummy_parallel_permission_turn(…) }
    else if prompt.to_lowercase().contains("staggered")  { dummy_staggered_permission_turn(…) }
    else                                                 { dummy_permission_turn(…) }
    return;
}

// in turn_events — the scripted ones
if prompt.starts_with(COMPACT_PROMPT_MARKER) { … return }
if prompt.to_lowercase().contains("table")   { … return }
if lower.contains("agents") && !lower.contains("agents.md") { … return }
…
if prompt.to_lowercase().contains("parallel") { … } else { … }
```

Nine demos, nine cues, no table. Nothing said which cues were taken, and
nothing caught a cue **shadowed** by an earlier one — `"staggered permission
demo"` also mentions `"permission"`, so ordering those four branches wrong
silently retires a demo. The smoke suite would keep passing: it would just be
testing a different turn than it thought.

## The shape now

`src/stream/` is a directory of per-area modules (see
[`module-layout.md`](module-layout.md)), and the dummy is a self-contained
subtree inside it:

| Module | Holds |
|--------|-------|
| `dummy/mod.rs` | `DummyAi` — the `ReplySource` impl, `turn_events`, and the playback pacing (`with_startup_delay` / `with_chunk_delay`, the two knobs `ALTER_ZERO_STARTUP_DELAY_MS` / `ALTER_ZERO_CHUNK_DELAY_MS` set). |
| `dummy/scenario.rs` | **The registry**: `Cue`, `Scenario`, `SCENARIOS`, `select`. |
| `dummy/script.rs` | The canned replies, the `handoff!()` sentence they close on, and the streaming/output primitives (`chunks`, `tokens` — the token-sized split the slow-stream stress demo streams by (`docs/slow-stream.md`), `dummy_response`, `reply_parts`, `image_ack`; the file-cell bodies come straight from `llm::tools::write_report`/`update_report`). |
| `dummy/turns.rs` | The **pure** scripted turns — one `Cue -> Vec<StreamEvent>` per scenario. |
| `dummy/gated.rs` | The turns that *ask*, blocking on the permission gate. |
| `dummy/agent.rs` | The two turns that stream a **launched subagent's own round** on the agent channel, so the agent session view is drivable offline (`docs/agent-view-streaming.md`) — one that streams a table, one whose own parallel `bash` batch **asks** on the shared permission gate. |

Everything above the dummy is the seam a real backend implements: `event.rs`
(the whole wire format), `source.rs` (`ReplySource`), `cancel.rs`
(`CancelToken`), `stall.rs` (the wedged-backend double).

## The registry

One ordered table, the shape `app::COMMANDS` already uses for slash commands:

```rust
pub(in crate::stream) const SCENARIOS: &[Scenario] = &[
    // Auto mode's classifier deciding a `bash` batch instead of the user.
    Scenario {
        selects: |cue| cue.mentions("permission") && cue.mentions("auto"),
        play: Play::Gated(gated::auto_permission_turn),
    },
    …
    // The default turn: think, then read/edit/run alter-zero's calling card.
    Scenario {
        selects: |_| true,          // the catch-all — keep it last
        play: Play::Script(turns::tools_turn),
    },
];
```

Three types carry it:

- **`Cue`** — the turn's inputs as a scenario reads them: the prompt, its
  lowercased form, and how many Ctrl+V images rode along. Built once per turn,
  so a registry walk doesn't re-lowercase the prompt for every entry it tries.
  `cue.mentions("table")` is case-insensitive (the *user* types these cues);
  `cue.starts_with(marker)` is exact, and is used only for generated requests
  like `/compact`'s, where a fuzzy match would hijack a real question about
  compaction.
- **`Play`** — how a scenario produces events. `Script(fn(&Cue) ->
  Vec<StreamEvent>)` is a pure turn, played back by `DummyAi` with the delays
  that make streaming visible. `Gated(fn(&Stage))` streams on the channel
  itself and blocks on the permission gate between calls, exactly as a real
  backend's tool thread does.
- **`Stage`** — what a gated demo streams through: the gate, the channel, the
  cancel flag.

`select(cue, gate_attached)` walks the table in order and is **total** — the
last entry matches everything, so no prompt can fall off the end. A gated
scenario is only offered when a gate was actually attached; without one it
would block forever on an answer nobody can give, so the prompt falls through
to a script. That is what keeps `turn_events` — which has no gate — able to
answer *any* prompt.

Both entry points now dispatch through the one table:

```rust
// the pure twin, for tests and offline callers
pub fn turn_events(prompt: &str, image_count: usize) -> Vec<StreamEvent> {
    let cue = Cue::new(prompt, image_count);
    match scenario::select(&cue, false).play { … }
}

// the ReplySource impl
match (scenario::select(&cue, permissions.is_some()).play, &permissions) {
    (Play::Gated(play), Some(gate)) => play(&Stage { gate, tx: &tx, cancel: &cancel }),
    (Play::Script(script), _)       => replay(script(&cue), &tx, &cancel),
    …
}
```

## Adding a scenario

1. Write the turn — a `fn(&Cue) -> Vec<StreamEvent>` in `turns.rs`, or a
   `fn(&Stage)` in `gated.rs`.
2. Add one `SCENARIOS` entry with its cue.
3. Add its example prompt to `EXAMPLES` in `src/stream/tests/scenario.rs`.

Step 3 is not optional, and that is the point. `EXAMPLES` is a parallel list of
prompts, one per entry in order, and the suite asserts:

- **the lengths match** — a new scenario with no example fails the build's
  tests, so nobody adds a demo without stating what triggers it;
- **each example selects its own entry** — the shadowing above becomes a test
  failure naming both scenarios, instead of a demo that quietly never plays;
- **names are unique** — the key the docs and the smoke prompts use;
- **selection is total** — including for the empty prompt;
- **gated demos need a gate** — and fall through to a script without one;
- **every user-facing script acknowledges attached images**, since the dummy
  has no vision and the image channel's only visible proof is that opening
  chunk (`docs/image-paste.md`). `/compact`'s request is the one exemption: the
  loop builds it, and never attaches images to it.
- **every user-facing script ends on the hand-off** — the `/login` → `/model`
  sentence below. Same exemption, same reason.
- **`turn_events` agrees with the registry** — so the second dispatch copy
  can't grow back.

`Scenario::name` exists only under `#[cfg(test)]`. Nothing at runtime reads it
— selection needs the cue and the turn — so it would be dead weight in the
shipped binary, but it is what lets a failure say *which* scenario a prompt
picked. Same trick `ui::TranscriptCache`'s counters use.

## The scenarios

| Name | Cue | What it demonstrates |
|------|-----|----------------------|
| `agent-permission` | "subagent" + "permission" | one **foreground** subagent whose own parallel `bash` batch asks: the prompt raised inside its session view is about *its* calls, not the lead's `● Agent(…)` cell (`docs/agent-view-streaming.md`) |
| `permission-auto` | "permission" + "auto" | auto mode's classifier deciding a `bash` batch in the user's stead (`docs/permissions.md`) |
| `permission-parallel` | "permission" + "parallel" | two gated `bash` calls — back-to-back prompts with no pause between them |
| `permission-staggered` | "permission" + "staggered" | a screen-tall `write` prompt answered into a one-line one: the modal region's hardest shrink |
| `permission-write` | "permission" | the single `Write` approval: the options, Tab's amend, the stashed draft |
| `compact` | the summarization marker | `/compact`'s text-only handoff summary (`docs/compact.md`) |
| `agent-stream` | "subagent" | one background subagent that streams **its own session** — a thinking phase then a forming table — the only demo that drives the agent session view's strip (`docs/agent-view-streaming.md`) |
| `table` | "table" | a streaming GFM table with wide emoji (`docs/table-streaming.md`) |
| `markdown` | "markdown", not "agents.md" | the slow-stream stress tour: every markdown element in one long text-only reply, streamed in **token-sized** pieces (`tokens`) — the document `smoke.sh` Phase 121 streams at a few tokens a second (`docs/slow-stream.md`) |
| `agents` | "agents", not "agents.md" | a two-subagent group, foreground or background (`docs/agent-tool.md`) |
| `parallel-batch` | "parallel" | three parallel `Bash(ping …)` calls and their `⎿ Waiting…` cells (`docs/parallel-tools.md`) |
| `files` | "diff"/"edit"/"write", not "agents.md" | a `Write` then an `Edit` of the same file: the numbered file cell and its green/red diff hunk (`docs/tools.md`) |
| `interactive` | "interactive" | a setup wizard driven through a terminal session: a `tty` launch stopping at a prompt, an answer met by the next question, and the last answer ending the program — one call a round, every result the real `pty::report::report`, so the cells wear their dim `Waiting for input · session …` rows (`docs/interactive-shell.md`, `smoke.sh` Phase 124). The whole word only: `tty` hides in "pretty", `repl` in "reply" |
| `tools` | anything | the default turn: think, then read `about.py`, `Edit` in the credit it forgot, and run it |

Cue order is registry order, so a narrower cue sits above a broader one that
would also match it — `agent-stream` sits above `table` because the table is
what its subagent streams, so a prompt naming both means that one, and
`agent-permission` sits above the whole permission block for the same reason
(its cue is "permission" plus one more word). Two cues
carry a guard rather than an order: `agents` and `files` both exclude
`agents.md`, because `/init` submits a canned prompt that names it and says
"do not over**write**" — without the guard that turn would be answered with a
scripted fizzbuzz.

A scenario can also need a **handle** the session may not have attached, and
`select` skips it when it is missing so the prompt falls through to a script:
the permission demos need the gate, the ask demo the ask gate, and
`agent-stream` the subagent registry (`Play::Gated`/`Asked`/`Agent`).
`agent-permission` wants **both**: it is a `Play::Agent`, so the registry
gates its selection, and `AgentStage` carries the permission gate as an
`Option` — with none attached (permissions off) its subagent runs its calls
unasked, which is what the real approve seam does there too.

## What the dummy actually says

The dummy is what a first run meets — no key configured means no model, and the
session is the dummy whether the user meant to demo it or not. So the replies do
a job rather than fill space:

- **They admit what they are.** Every reply opens by saying the words are canned
  and the tool calls scripted.
- **They narrate the cells under them.** Each scenario has its own two-part
  text, so the `parallel` demo talks about `⎿ Waiting…` and the `files` demo
  talks about the diff tints. The default turn rotates three of them
  (`dummy_response`, still keyed on `prompt.chars().count() %` the table's
  length, so it stays deterministic and testable).
- **They end on the hand-off.** Every user-facing reply closes on one shared
  sentence — the `handoff!()` macro in `script.rs`:

  > Two commands away from the real thing: `/login` signs you in — a
  > subscription, or a provider API key — then `/model` picks the model to run.

  `stream::tests::scenario::every_user_facing_script_hands_the_user_off_to_a_real_model`
  walks the registry and fails any scenario that doesn't, and `smoke.sh` settles
  on that same sentence (`SETTLED_REPLY`) whatever prompt it sent.

A reply is written in **two parts split by a blank line** (`reply_parts`): the
text before the turn's work and the text after it. That is not cosmetic — a
tool call finalises the text before it as its own history message
(`App::flush_streaming_segment`, invariant 4), so a split anywhere else would
cut a paragraph, or a list, across two messages and render two broken blocks.
The separator rides the first part, so the two still concatenate to
`dummy_response`'s whole reply.

## Scripted tools resolve with *real* tool output

A demo that renders differently from the live agent is a demo of the wrong
thing. So every scripted call resolves with the output the real executor would
have produced, built by the executor's own renderers (`llm::tools` is pure, so
this costs nothing):

| Call | Output | What that buys |
|------|--------|----------------|
| `Read` | `tools::format_read` | the `{n:>W} {text}` gutter `ui::file_cell_lines` parses — a numbered, syntax-highlighted (in the active theme's code theme, `docs/theme.md`) cell under a `Read N lines` head, instead of a plain text peek |
| `Write` | `tools::write_report` — `Wrote {N} lines to {path}` + the numbered contents | the numbered new-file body (the executor's own `describe_change` core, shared with the gated permission demos) |
| `Edit` | `tools::update_report` — `Updated {path} (+A -D)` + the numbered diff hunks | only the touched hunk, `+` rows on the green tint and `-` rows on the red one |
| `Bash` | `Exit code: N` + the body | the frame `ui::command_display_output` reads: dropped on success, rewritten to a red `Error: Exit code N` head on failure, so a red cell says *why* |

Parity runs past the cell, to what the *model* would have read: `Write` and
`Edit` resolve through the same **two-text** `ToolAnswered` the live executor
sends (`ScriptedCall::end`), the numbered body on the cell and
`tools::write_ack`/`edit_ack`'s one line as the result, and every scripted
`ToolStart` carries the verbatim arguments a real call would
(`ScriptedCall::start`). So the demo's Ctrl+D shows the same replayed shape
the live one does — a `write` whose content rides its own call, and a result
that is one line (`docs/tools.md`, `docs/context.md`).

## The default turn is a story, not a sampler

The turn that answers *anything* used to be two unrelated calls — read
`src/main.rs`, then ping a host that doesn't resolve. Both cells rendered, and
neither meant anything: the demo read as a widget sampler.

It is one errand in three steps now, on one file — and the file is chosen so
the errand has a *point*. `about.py` is alter-zero's calling card, and it
carries `CREATOR = "linuztx"` and `HOME = "https://github.com/linuztx"` right at
the top, then prints a card that mentions neither:

1. **`Read(about.py)`** — the file, numbered and highlighted. The two unused
   constants sit in the visible peek, so the reader sees the omission before
   the demo fixes it. The file runs a few lines past the cell's ten-row peek on
   purpose, so the committed cell carries the `… +N lines (ctrl+o to expand)`
   tail and the transcript has something to expand that the inline cell doesn't
   show.
2. **`Edit(about.py)`** — one line rewritten, wiring those constants into what
   the card prints. A single `+`/`-` pair inside its context: the smallest diff
   that still shows both tints.
3. **`Bash(python3 about.py)`** — the run, whose output *is* the credit:

   ```
   alter-zero — an autonomous AI agent that lives in your terminal
     created by linuztx · https://github.com/linuztx
   ```

   The last cell is proof the middle one landed, and the demo has said who
   wrote the thing you are looking at.

That ordering is what the reply narrates, and what
`stream::tests::turns::the_default_turn_is_one_errand_in_three_steps` pins:
three calls, same path, in that order, the command naming the file the edit
changed. The turn stays green throughout — a gratuitously failing call would
muddle the story, so the red cell lives in the `parallel` batch, whose third
ping can't resolve its host.

The `bash` framing has a second half worth keeping straight: the executor
**streams the raw output lines while the command runs** and frames the result
only when it resolves. `ScriptedCall` mirrors that — `output` is the body it
streams as `ToolOutput`, `result()` is the framed `ToolEnd` — so the live tail
never shows an `Exit code:` line the real one wouldn't.

## What did not change

The event *shape*. Every scenario still emits the same sequence it did: the
`ToolBatch` up front, one running call at a time, `ToolOutput` tails on the
`bash` cells and none on the file ones, the image acknowledgement in front of
everything but the compact summary.

The shared work moved rather than multiplied. The four gated demos each carried
their own copy of the same 40-line approval body — build the request, consult
the allowlist, raise it, block on the gate, map the five decisions, send
`ToolStart` then `ToolEnd`-or-`ToolRejected`. That lives once on `Stage` now
(`ask`, `judge`, `resolve`, `close`, `announce`, `request`), and each demo
reads as its own script of calls.
