# Module layout: `app/`, `ui/`, `tui/`, and `stream/`

`app.rs` and `ui.rs` were the two files everything grew into. By the time they
were split they held 13,872 and 17,665 lines — one 4,038-line `impl App`, two
test modules of 7,548 and 9,688 lines, and every renderer in the program side by
side. Both are now directories of per-area modules.

The split is **pure code motion**. Every item kept its body, its doc comment and
its position relative to its neighbours; the only edits were the module headers,
the imports, and widening the visibility of items that are now reached across a
module boundary. The public API is unchanged: each `mod.rs` re-exports its areas
by name, so every `crate::app::X` and `ui::y(…)` path still resolves exactly as
before, and `term.rs` did not change at all.

`main.rs` was the third such file — 5,304 lines, one 2,058-line `run()` — and it
is now `src/main.rs` (77 lines) plus `src/tui/`, 20 area modules built on the
same pattern. That split needed one thing the other two did not: the loop's state
had to become a struct before the code could move. See
[`src/tui/` — the terminal shell](#srctui--the-terminal-shell).

`stream.rs` was the fourth — 2,459 lines, of which the backend *seam* every real
model implements was about 250 and the offline demo backend was 1,100. See
[`src/stream/` — the backend seam](#srcstream--the-backend-seam).

## Where things live

### `src/app/` — conversation state and the pure update logic

`mod.rs` keeps the `App` struct itself. That is deliberate: a struct's private
fields are visible to the module that defines it *and all its descendants*, so
leaving `App` in `mod.rs` lets every submodule — and the whole test tree — keep
the direct field access it had when this was one file. No field had to be
widened for the split.

| Module | Holds |
|--------|-------|
| `mod.rs` | The `App` struct, its core message/toast/clock methods, and the facade (`mod` + `pub use`). |
| `types.rs` | `Role`, `Message`, `HistoryItem`, `View`, `Toast`, `SessionInfo` — the shared vocabulary. Types that belong to one feature live with it instead: `ToolCall` in `tools.rs`, `Compaction` in `compact.rs`, `StreamError`/`InterruptedTurn` in `turn.rs`. |
| `status.rs` | `TurnStatus`, `TurnSummary`, `TokenArrow`, `RetryInfo` and the token tally the spinner line renders. |
| `action.rs` | `Action`: what a key press asks the loop to do. |
| `keys.rs` | `on_key` (dispatch by `View`) and the conversation view's key map. |
| `composer.rs` | Paste placeholders, Ctrl+V image attachments, the `!` shell mode. |
| `commands.rs` | The `COMMANDS` registry, the `/token` filter, and running the highlighted command. |
| `file_picker.rs` | The `@` picker's query round-trip and path insertion. |
| `input_history.rs` | `InputHistory` (↑/↓ recall) and `HistorySearch` (Ctrl+R). |
| `queue.rs` | `QueuedTurn` and the mid-turn queue. |
| `tools.rs` | The tool-call batch queue and how a call resolves. |
| `turn.rs` | Turn lifecycle: begin/stream/interrupt/finish, the status tally, the summary. |
| `compact.rs` | `/compact`, auto-compaction, and the context gauge. |
| `backtrack.rs` | The Esc-Esc backtrack. |
| `views.rs` | The Ctrl+O and Ctrl+D overlays' state. |
| `resume.rs` | The `/resume` picker. |
| `model_picker.rs` | The inline `/model` picker. |
| `login.rs` | The inline `/login` onboarding. |
| `mascot.rs` | The banner-mascot catalog, the `/mascot` picker's state, the `mascot.json` format (`docs/mascot.md`). |
| `permission.rs` | The inline tool-permission prompt: the stashed draft, the option/amend key map (`docs/permissions.md`). |
| `ask.rs` | The inline `AskUserQuestion` modal: the tab/row state, answers under construction, the Other/notes entries, the queue against the permission prompt (`docs/ask.md`). |
| `background.rs` | Background shells and the ↓ manager band. |
| `agent.rs` | The `Agent` tool's roster, groups, and notices. |
| `reasoning.rs` | The thinking stream: the live reasoning buffer, the `Reasoning` cell it settles into, and the provider's reasoning-token snap (`docs/thinking-stream.md`). |

### `src/ui/` — pure rendering

| Module | Holds |
|--------|-------|
| `mod.rs` | The crate imports and the facade (`mod` + `pub use`). |
| `theme.rs` | **Every** styling and live-region-geometry constant. |
| `wrap.rs` | `cols` and the wrapping/truncating primitives. |
| `layout.rs` | `live_height`, `repin` (plus `region_is_modal` — the predicate `term.rs` reads to note a permission prompt's one-way scrolls for its close's purge rebuild, `docs/permissions.md`), `cursor_position`/`cursor_visible` (where the hardware cursor rests, and whether the frame shows one at all), `input_box` — the geometry policy `term.rs` acts on. |
| `assistant.rs` | The markdown/code renderer (`AssistantRenderer`, `assistant_lines`). |
| `inline.rs` | Inline-span rendering: `**bold**`, `` `code` ``, links, and the wrap that keeps spans intact across a row. |
| `table.rs` | GFM table sizing, borders, and the narrow-terminal record fallback. |
| `message.rs` | Rendering one committed message, plus the `/compact` marker cell. |
| `conversation.rs` | The whole-history walk and the tail a purge rebuild (resize, `/clear`, a rewind) repaints. |
| `tool.rs` | Tool cells: the `● name(args)` header, the `⎿` output block, a running command's live tail. |
| `file_cell.rs` | The `read`/`write`/`edit` numbered-diff cell. |
| `status.rs` | The status line (spinner, shimmer, tally) and the `Done for Ns` summary. |
| `agent.rs` | Subagent trees, cells, and the footer roster. |
| `menu.rs` | The palette / `@` picker / `?` shortcuts bands. |
| `footer.rs` | The footer row, the toast, the queued rows, and what displaces the footer. |
| `header.rs` | The startup banner — the gradient mascot beside the metadata column (`docs/mascot.md`). |
| `mascot_view.rs` | The inline `/mascot` picker with its live banner preview (`docs/mascot.md`). |
| `live.rs` | `render_live` — the streaming strip, the box, the band. |
| `transcript.rs` | The Ctrl+O overlay and `TranscriptCache`. |
| `classifier_view.rs` | The Ctrl+D view's classifier page body (`docs/permissions.md`). |
| `context_view.rs` | The Ctrl+D context-debug overlay. |
| `resume_view.rs`, `model_view.rs`, `login_view.rs`, `background_view.rs` | The pickers and the manager band. |
| `permission_view.rs` | The tool-permission modal (`docs/permissions.md`). |
| `ask_view.rs` | The `AskUserQuestion` modal: the chip strip, option pages, preview panel, review page (`docs/ask.md`). |
| `reasoning.rs` | The thinking stream's cells: the live `● Thinking…` block, the collapsed `Thought for …` line, and its Ctrl+O expansion (`docs/thinking-stream.md`). |
| `stream_render.rs` | `StreamRender`, the incremental commit-to-scrollback renderer. |

### `src/tui/` — the terminal shell

`main.rs` is the I/O boundary, so it is the one file unit tests can't reach (bar
the odd pure helper) — which is exactly why it grew worst. At 5,304 lines it held
a dozen unrelated boundary concerns (the CLI, provider config, session recording,
input-history persistence, the `/resume` scan, the `!` shell runner, three worker
threads, the stream fold, the agent fold, rendering) and one `run()` of 2,058
lines whose `select!` input branch carried ~40 inline `Action` arms.

`main.rs` now does four things — the detached-exec hook, the CLI resolution,
opening the viewport, running the loop — and everything else lives here:

| Module | Holds |
|--------|-------|
| `mod.rs` | The `Session` struct and `StatusClocks` — the loop's state. |
| `event_loop.rs` | `run`: the `select!` over the nine sources — one handler call per branch. |
| `actions.rs` | The `Action` dispatch — one arm per key-press outcome — plus `Flow`, `/clear`, `/copy`, the resize and paste routing. |
| `turn.rs` | Turn lifecycle: `start_turn`, `run_shell`, the background follow-up, `/compact`, `dispatch_after_turn`, `abandon_inflight`. |
| `stream.rs` | `on_stream_event`: folding one reply event into `App` + scrollback. |
| `agent.rs` | Subagent events, the roster's clocks, the linger sweep, the session view's arms (`docs/agent-tool.md`). |
| `background.rs` | Background-shell events and `settle_bg_completions` (`docs/background.md`). |
| `permission.rs` | `PermissionStore` — the gate, its file, this project's key — and the prompt's answers (`docs/permissions.md`). |
| `view.rs` | Drawing: the draw tick, the overlays, the repaints, `live_region_height`, the injected clocks. |
| `commit.rs` | Scrollback commits — the one place invariant 4 is enforced — and the toast. |
| `models.rs` | `ModelSession`: the backend and every knob that selects it, plus the `/model`, `/login`, Ctrl+T and probe arms (`docs/llm.md`). |
| `config.rs` | Reading the environment: providers, keys, settings, permission rules, paths. |
| `bootstrap.rs` | `Session::bootstrap` / `shutdown` / `after_iteration` — assembly, teardown, loop-bottom work. |
| `startup.rs` | The `--continue`/`--resume` argument resolution (`docs/cli.md`). |
| `recorder.rs` | `SessionRecorder`: mirroring history to a rollout file (`docs/resume.md`). |
| `resume.rs` | Finding recorded sessions on disk, and the `/resume` + backtrack arms. |
| `history_store.rs` | `InputHistoryStore` (`docs/history-persistence.md`). |
| `mascot.rs` | Applying a `/mascot` selection: the `mascot.json` write + banner repaint (`docs/mascot.md`). |
| `shell.rs` | The `!` command runner and its drain/cap unit tests (`docs/shell-command.md`). |
| `workers.rs` | The off-thread file-search / clipboard / model-list jobs. |
| `host.rs` | Clocks, dates, the OS string, the uid, ids — the raw impurities. |

### `src/stream/` — the backend seam

`stream.rs` held two things that have nothing to do with each other: the
protocol *every* backend speaks — `StreamEvent`, `ReplySource`, `CancelToken`,
about 250 lines — and `DummyAi`, the 1,100-line offline demo whose canned turns
grow every time a UI feature lands (it is the only backend `scripts/smoke.sh`
can drive). Reading the seam meant scrolling past canned `ping` output.

| Module | Holds |
|--------|-------|
| `mod.rs` | The facade (`mod` + `pub use`). |
| `event.rs` | `StreamEvent` and its payloads (`ToolCallSummary`, `AgentSpec`, `AgentCallDone`, `TokenUsage`) — the whole wire format. |
| `source.rs` | `ReplySource`: the one trait the event loop depends on. |
| `cancel.rs` | `CancelToken`. |
| `stall.rs` | `StallAi`, the wedged-backend double (`docs/interrupt.md`). |
| `dummy/mod.rs` | `DummyAi` — the `ReplySource` impl, `turn_events`, the playback pacing. |
| `dummy/scenario.rs` | The scenario registry: which demo a prompt selects. |
| `dummy/script.rs` | Canned replies and the streaming primitives. |
| `dummy/turns.rs` | The **pure** scripted turns, one `Cue -> Vec<StreamEvent>` each. |
| `dummy/gated.rs` | The turns that block on a gate: the permission demos and the `AskUserQuestion` round trip (`docs/ask.md`). |

The dummy is a subtree rather than four sibling files because it is genuinely
separable: it is the one backend that could be deleted without touching the
seam, and now that is a directory rather than a excision.

Unlike the other three splits this one is not *only* code motion. Which demo
plays used to be decided by two hand-written `if`/`else` chains in two different
files, with nine cues and no table — so a cue shadowed by an earlier one
retired a demo silently, and the smoke suite would keep passing while testing a
different turn than it thought. That is one ordered `SCENARIOS` table now, and
the suite proves every entry is still reachable. See
[`dummy-backend.md`](dummy-backend.md).

#### Why this split needed a struct first

`app.rs` and `ui.rs` were collections of items, so moving them was enough. `run()`
was one function holding ~50 locals, and its helpers took them nine arguments at a
time (five `#[allow(clippy::too_many_arguments)]` marked where that hurt most).
Cutting *that* into modules without a state type would just have widened the
signatures.

So the state became **`Session`** (40 fields), and every handler is a method on
it. That is the same trick `app/mod.rs` plays with `App`: a struct's private
fields are visible to the module that defines it *and all its descendants*, so
each area module holds an `impl Session` block and reaches the state directly. No
field is public, and no accessor exists purely to cross a module boundary.

Three cohesive groups became types of their own, because their pieces only ever
move together:

- **`ModelSession`** — the backend plus the provider table, key store, active
  model, its reasoning mode, image support and context window. It owns *clones* of
  the background/subagent registries and the permission gate, so
  `ModelSession::rebuild` can re-attach the whole set; a rebuild that attached
  only part of it used to drop the `agent` tool silently.
- **`PermissionStore`** — the gate, the file its rules persist to, and this
  project's key inside that file. Every method that changes a rule persists it in
  the same breath, so there is no way to grant one and forget to save it.
- **`StatusClocks`** — already existed; it gained `start_turn`/`end_turn` so the
  four turn-start paths can't drift on which clocks they reset.

#### The channels live on it too

Both ends of all nine — the senders *and* the receivers the `select!` polls.

The first cut of this split put the receivers in a separate `Sources` struct, on
the belief that `select!` needs them as separate places to borrow. **That is
false**, and it cost real plumbing: the reply receiver had to be threaded
`on_terminal_event` → `on_action` → `interrupt_turn`/`clear_conversation` just so
the bottom of that chain could swap the channel — exactly the hand-threaded state
this refactor exists to remove.

`select!` scopes its futures. The nine it builds borrow nine *distinct fields* of
one `&mut` place, which is allowed, and they are all dropped before the winning
branch's body runs — so that body can take `&mut session` freely. A handler that
drains a second channel does it the same way, since `recv`/`try_recv` return
owned values and hold no borrow past the call:

```rust
pub(crate) fn drain_agent_events(&mut self) {
    while let Ok(AgentEvent::Stream { id, event }) = self.agent_rx.try_recv() {
        self.on_agent_event(&id, event);   // &mut self, mid-drain
    }
}
```

So the receivers are fields, `Session::abandon_inflight` swaps both ends of the
reply channel in place, and three signatures lost a parameter.

#### What the loop reads like now

```rust
Some(event) = sources.reply_rx.recv() => session.on_reply_event(event, &mut sources.agent_rx),
Some(()) = sources.draw_rx.recv() => session.on_draw_tick()?,
Some(result) = sources.file_rx.recv() => session.on_file_matches(result),
```

Adding a feature is now: a method in the area module that owns it, and one line in
`actions.rs`'s dispatch or one branch in the `select!`.

#### Behaviour is unchanged

This is a refactor: the same 1,807 unit tests, the same 7 shell drain/cap tests
(moved with the runner into `tui/shell.rs`), the same integration tests, and a
full `scripts/smoke.sh` run — all 64 phases — pass. Every arm's body, ordering and
comment moved verbatim; the loop-bottom sequence, the commit ordering, and the
turn-end dispatch order are the same statements in the same order.

Four things were tidied along the way, all of them latent bugs in comments rather
than code: `main.rs` had four doc comments orphaned onto the *next* function by
later insertions (`build_backend`'s onto `permissions_enabled`,
`dispatch_after_turn`'s onto `checkpoint_turn_end`, `overlay_return_clear`'s onto
`repaint_active_view`, `spawn_file_search_worker`'s onto `spawn_image_paste`).
Each doc now sits with the item it describes.

#### Reading the feature docs

The per-feature docs under `docs/` name the boundary as `main.rs` in prose —
"the boundary (`main.rs`)", "`main.rs` reads the env", and so on. Every
*symbol-qualified* reference was updated to its new path (`tui::turn`'s
`Session::start_turn`, `tui::config::save_settings`, …), so a search for the
name lands in the right module; where a doc says `main.rs` on its own, read it
as "the boundary", and the table above says which module holds it.

## The conventions the split had to preserve

**Styling stays centralised.** Every `const` from `ui.rs` moved into `ui/theme.rs`
— none were scattered to the module that uses them. Retheme or re-size there, as
before; the other modules `use super::theme::*`.

**All width math still goes through `cols`.** It lives in `ui/wrap.rs` now, and
the modules that measure import it explicitly.

## The facade

Each `mod.rs` re-exports its areas **by name**, not with `pub use self::m::*;`.
A glob keeps the paths working but leaves the public surface implicit: nothing
states what the module exports, and a `pub` added to a submodule later escapes
the crate without anyone deciding it should. `tests/api_surface.rs` locks the
result — it names all 144 items the pre-split `app.rs`/`ui.rs` exported, plus
the 18 `stream.rs` did, so a forgotten re-export fails to compile.

Naming every re-export also surfaced work the glob had been doing silently: it
re-exported `pub(super)` items into `crate::app` / `crate::ui` at their own
visibility, which is how sibling modules were reaching them through
`use super::*`. Those are explicit imports now.

`src/tui/` has no facade and needs none: it belongs to the **binary** crate, so
nothing outside the binary can name it and there is no public surface to lock.
`main.rs` reaches exactly two paths — `tui::startup::resolve_cli` and
`tui::event_loop::run`.

## Doc links

`cargo doc --no-deps --lib` is part of the gate. The crate denies warnings, which
promotes a broken intra-doc link to an error — and a module split breaks links by
construction, since a bare `[`name`]` resolves against a different scope in its
new home and a public item may not link to a private one. Both classes are fixed
here (77 errors before the split, 0 now): qualify the path when the target is
still public, demote `[`X`]` to `` `X` `` when it is not.

## Visibility

Splitting one file into many turns intra-file access into cross-module access,
so items that other areas reach for widened from private to `pub(super)` —
`pub(in crate::app)` / `pub(in crate::ui)`. Nothing that was crate-private became
public, and nothing that was public changed. Where an item is only used inside
its own module, it stayed private.

In `src/tui/` the equivalent is `pub(crate)`, which reaches no further than the
binary. **Every** `Session` field stayed private — the descendant-visibility trick
above is what makes that work, and it is why no accessor exists purely to cross a
module boundary.

## Tests

The tests moved with the code into `src/app/tests/` and `src/ui/tests/`, one file
per area. Because those files are descendants of `crate::app` / `crate::ui` they
keep exactly the reach into private state the single flat `mod tests` had.

A helper lives in `tests/mod.rs` only when two or more area files use it —
resolved transitively, since a helper can be reached through another helper.
Everything else was pushed down to its one caller.

The test set is unchanged by the split: the same 1591 unit tests with the same
names, all green, plus a clean `cargo fmt --check`, `cargo clippy --all-targets
-- -D warnings`, `cargo doc --no-deps --lib`, and a full `scripts/smoke.sh` run.

`src/tui/` is the I/O boundary, so it is verified by `scripts/smoke.sh` rather
than unit tests — with one exception, the same one it always had: the `!` runner's
reader-generic drain/cap pair, whose 7 tests moved with it into `tui/shell.rs`.
The suite after the `tui/` split is 1,807 unit tests (the library has grown since
the first two splits), those 7 per binary target, and the integration tests, all
green, with all 64 smoke phases passing.
