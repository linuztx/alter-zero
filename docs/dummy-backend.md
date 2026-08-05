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
| `dummy/mod.rs` | `DummyAi` — the `ReplySource` impl, `turn_events`, and the playback pacing. |
| `dummy/scenario.rs` | **The registry**: `Cue`, `Scenario`, `SCENARIOS`, `select`. |
| `dummy/script.rs` | The canned replies and the streaming primitives (`chunks`, `dummy_response`, `image_ack`). |
| `dummy/turns.rs` | The **pure** scripted turns — one `Cue -> Vec<StreamEvent>` per scenario. |
| `dummy/gated.rs` | The turns that *ask*, blocking on the permission gate. |

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
    // The default turn: think, then a compact `Read`+`Bash` batch.
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
- **`turn_events` agrees with the registry** — so the second dispatch copy
  can't grow back.

`Scenario::name` exists only under `#[cfg(test)]`. Nothing at runtime reads it
— selection needs the cue and the turn — so it would be dead weight in the
shipped binary, but it is what lets a failure say *which* scenario a prompt
picked. Same trick `ui::TranscriptCache`'s counters use.

## The scenarios

| Name | Cue | What it demonstrates |
|------|-----|----------------------|
| `permission-auto` | "permission" + "auto" | auto mode's classifier deciding a `bash` batch in the user's stead (`docs/permissions.md`) |
| `permission-parallel` | "permission" + "parallel" | two gated `bash` calls — back-to-back prompts with no pause between them |
| `permission-staggered` | "permission" + "staggered" | a screen-tall `write` prompt answered into a one-line one: the modal region's hardest shrink |
| `permission-write` | "permission" | the single `Write` approval: the options, Tab's amend, the stashed draft |
| `compact` | the summarization marker | `/compact`'s text-only handoff summary (`docs/compact.md`) |
| `table` | "table" | a streaming GFM table with wide emoji (`docs/table-streaming.md`) |
| `agents` | "agents", not "agents.md" | a two-subagent group, foreground or background (`docs/agent-tool.md`) |
| `parallel-batch` | "parallel" | three parallel `Bash(ping …)` calls and their `⎿ Waiting…` cells (`docs/parallel-tools.md`) |
| `tools` | anything | the default turn: think, then a compact `Read`+`Bash` batch |

Cue order is registry order, so a narrower cue sits above a broader one that
would also match it.

## What did not change

The events. Every scenario emits exactly the sequence it did before — the same
`ToolBatch` up front, the same one-running-call-at-a-time execution, the same
`ToolOutput` tails on the `bash` cells and none on the `read`, the same
image acknowledgement in front of everything but the compact summary. The
pre-existing suite (which asserts those orders event by event) passes
unchanged, and so does `scripts/smoke.sh`.

The shared work moved rather than multiplied. The four gated demos each carried
their own copy of the same 40-line approval body — build the request, consult
the allowlist, raise it, block on the gate, map the five decisions, send
`ToolStart` then `ToolEnd`-or-`ToolRejected`. That lives once on `Stage` now
(`ask`, `judge`, `resolve`, `close`, `announce`, `request`), and each demo
reads as its own script of calls.
