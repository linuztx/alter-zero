# Module layout: `app/` and `ui/`

`app.rs` and `ui.rs` were the two files everything grew into. By the time they
were split they held 13,872 and 17,665 lines — one 4,038-line `impl App`, two
test modules of 7,548 and 9,688 lines, and every renderer in the program side by
side. Both are now directories of per-area modules.

The split is **pure code motion**. Every item kept its body, its doc comment and
its position relative to its neighbours; the only edits were the module headers,
the imports, and widening the visibility of items that are now reached across a
module boundary. The public API is unchanged: each `mod.rs` re-exports its areas
by name, so every `crate::app::X` and `ui::y(…)` path still resolves exactly as
before, and `main.rs`/`term.rs` did not change at all.

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
| `background.rs` | Background shells and the ↓ manager band. |
| `agent.rs` | The `Agent` tool's roster, groups, and notices. |

### `src/ui/` — pure rendering

| Module | Holds |
|--------|-------|
| `mod.rs` | The crate imports and the facade (`mod` + `pub use`). |
| `theme.rs` | **Every** styling and live-region-geometry constant. |
| `wrap.rs` | `cols` and the wrapping/truncating primitives. |
| `layout.rs` | `live_height`, `repin`, `cursor_position`, `input_box` — the geometry policy `term.rs` acts on. |
| `assistant.rs` | The markdown/code renderer (`AssistantRenderer`, `assistant_lines`). |
| `inline.rs` | Inline-span rendering: `**bold**`, `` `code` ``, links, and the wrap that keeps spans intact across a row. |
| `table.rs` | GFM table sizing, borders, and the narrow-terminal record fallback. |
| `message.rs` | Rendering one committed message, plus the `/compact` marker cell. |
| `conversation.rs` | The whole-history walk and the tail a resize or overlay return repaints. |
| `tool.rs` | Tool cells: the `● name(args)` header, the `⎿` output block, a running command's live tail. |
| `file_cell.rs` | The `read`/`write`/`edit` numbered-diff cell. |
| `status.rs` | The status line (spinner, shimmer, tally) and the `Done for Ns` summary. |
| `agent.rs` | Subagent trees, cells, and the footer roster. |
| `menu.rs` | The palette / `@` picker / `?` shortcuts bands. |
| `footer.rs` | The footer row, the toast, the queued rows, and what displaces the footer. |
| `header.rs` | The startup banner. |
| `live.rs` | `render_live` — the streaming strip, the box, the band. |
| `transcript.rs` | The Ctrl+O overlay and `TranscriptCache`. |
| `context_view.rs` | The Ctrl+D context-debug overlay. |
| `resume_view.rs`, `model_view.rs`, `login_view.rs`, `background_view.rs` | The pickers and the manager band. |
| `stream_render.rs` | `StreamRender`, the incremental commit-to-scrollback renderer. |

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
result — it names all 144 items the pre-split files exported, so a forgotten
re-export fails to compile.

Naming every re-export also surfaced work the glob had been doing silently: it
re-exported `pub(super)` items into `crate::app` / `crate::ui` at their own
visibility, which is how sibling modules were reaching them through
`use super::*`. Those are explicit imports now.

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
