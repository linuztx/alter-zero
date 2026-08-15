# The `AskUserQuestion` tool — asking the user mid-turn

Claude Code's `AskUserQuestion`: the model asks the user 1–4 multiple-choice
questions and **blocks until they answer**, then reads the answers as the tool
result. The UI is an inline modal — the permission prompt's sibling — with a
chip strip of question tabs, numbered options, multi-select checkboxes, an
auto-added free-text "Other" row, an optional side-by-side preview panel with
per-question notes, and (for multi-question calls) a closing review-and-submit
page.

```
────────────────────────────────────────────────────────────────────────────
 ←  ☒ Coffee style  ☒ Demo topics  ✔ Submit  →

 What's your favorite way to drink coffee?

 ❯ 1. Black ✔
      No milk, no sugar — just coffee
   2. Latte
      Espresso with steamed milk
   3. Cold brew
      Slow-steeped, served cold
   4. Type something.
   5. Chat about this

 Enter to select · Tab/Arrow keys to navigate · Esc to cancel
────────────────────────────────────────────────────────────────────────────
```

## The pieces

The feature is split exactly like tool permissions (`docs/permissions.md`) —
that seam is proven, and this is the same shape: a backend thread that must
park until the user decides.

- **`ask` (pure + the gate)** — the vocabulary and the handshake:
  - `AskQuestion`/`AskOption` parsed from the tool-call JSON
    (`parse_questions`, validating the schema's 1–4 questions of 2–4 options);
  - `AskAnswer`/`AskDecision` — what the user produced: `Submitted(answers)`,
    `Declined`, or `Chat` (the "Chat about this" row);
  - the two texts of every resolution: the **cell display**
    (`answered_display` — `User answered Claude's questions:` over `· Q → A`
    rows; `declined_display`/`chat_display` — the headline over
    `· Q (opt / opt / …)` rows) and the **model-facing result**
    (`answered_result` — the schema's `{"answers": {question: labels}}` JSON
    plus `annotations` carrying notes/previews; `declined_result`/
    `chat_result` — stop-and-wait instructions);
  - `AskGate` — the `Arc<Mutex<…>> + Condvar` sibling of `PermissionGate`:
    `next_id` → `resolve(id, AskDecision)` → `wait(id, cancelled)`.

- **`llm` half** — `tools::ask_spec()` (the function definition, wire name
  `askuserquestion`, offered only when `LlmBackend::with_ask` attached a gate
  — an embedder without one never exposes an unanswerable tool; subagents
  never get it), and `llm::ask::ask_user` — the `approve_call` twin the
  backend's execute closure routes the call to: parse, send
  `StreamEvent::AskUser(request)`, **block on the gate**, then map the
  decision onto a `ToolOutcome` whose new `context` field carries the
  model-facing result beside the displayed cell text.
  `run_agent` turns that split outcome into the matching event:
  `ToolAnswered { display, result }` (green — the `ToolRejected` twin) for a
  submission, `ToolRejected` for a decline/chat, so the recorded call keeps
  both texts (`ToolCall::context_output`) and the derived context replays
  exactly what the model read (`context::context_messages` — free, since it
  already reads `ToolCall::context_text()`; the rollout round-trips it the
  same way).

- **`app::AskPrompt`** — the modal state, `PermissionPrompt`'s shape: it owns
  every key while open, stashes the composer draft (the ask text entry reuses
  `App::input`, like Tab's amend field), and queues cross-modal arrivals — a
  permission request landing while a question is open waits its turn, and
  vice versa (`open_next_pending`). Navigation: ←/→/Tab/Shift+Tab move
  between question tabs (and the Submit tab), ↑/↓ move rows wrapping at the
  ends, digits
  jump-activate, Enter selects/toggles/activates, `n` opens the notes field
  on a preview question, Esc **declines** (the whole call resolves declined —
  the turn continues; the model is told to stop and wait). A single-select
  answer auto-advances to the next tab; a lone question resolves immediately;
  a multi-select question confirms via its own unnumbered `Submit` row. The
  entry fields (the Other row, the notes line) are real composer fields:
  **Shift+Enter / Ctrl+J** insert a newline (the wrapped rows render in
  place, continuations aligned under the text, and the accepted answer keeps
  the line breaks), and a **bracketed paste** lands exactly as in the
  composer — over the threshold it collapses to the compact
  `[Pasted Content N chars]` placeholder (Backspace removes it whole), which
  the entry's exit splices back to the real text
  (`paste::expand_pastes_consuming` — consuming only the entry's own pairs,
  so a placeholder sitting in the stashed composer draft still expands when
  that draft is eventually sent). The Submit page leads with an amber
  `⚠ You have not answered all questions` warning whenever the submission
  would be partial, reviews **only the answered questions** (`● question`
  over the green `→ answer`; an unanswered one is omitted — its ☐ chip and
  the warning already say so), and only submits when at least one question is
  answered — pressing `Submit answers` with none jumps to the first
  unanswered question instead.

- **`ui::ask_view`** — the renderer, `permission_view`'s sibling: one builder
  (`ask_lines`) produces every row (rule → chip strip → question → options →
  hints → rule) and `ask_height` reserves exactly that many
  (`ui::region_is_modal` covers the prompt so the close purge-rebuilds like
  the permission prompt's). The chip strip marks answered questions `☒`,
  unanswered `☐`, the Submit tab `✔`, and lights the **current** chip on the
  cyan selection background. A question with option previews renders
  side-by-side: options left, the focused option's preview in a bordered
  panel right, the `Notes: …` line beneath it. The hardware cursor hides on
  the option menu (`ui::cursor_visible`, the permission rule) and returns for
  the Other/notes text fields (`ask_cursor`, sharing the builder's geometry).

- **The resolved cell** — `ui::tool` special-cases the ask tool: the
  first output line ("User answered Claude's questions:") becomes the `●`
  header (green for a submission, red for a decline/chat) and the `· Q → A`
  rows render in the `⎿` gutter, wrapped — the committed transcript from the
  reference. While the call runs the generic `● AskUserQuestion(…)` header
  stands (visible only in Ctrl+O — the modal covers the live region).

- **The loop** — `Session.ask: AskGate`, always attached (bootstrap →
  `ModelSession` → every backend build, `DummyAi` included; asking is not a
  permission and does not follow `ALTER_ZERO_PERMISSIONS`).
  `StreamEvent::AskUser` opens the prompt; `Action::ResolveAsk` posts the
  decision on the gate; the loop bottom releases abandoned requests as
  `Declined` (the permission release's twin) and `/clear` clears the board.

- **The dummy demo** — a `Play::Asked` scenario (cue: "ask" + "question")
  drives the whole round trip offline: three questions — single-select,
  multi-select, and a previews+notes code-style pick — resolved through the
  real gate, closing on the shared handoff sentence.

## Flow

```
model calls askuserquestion
  └─ execute closure → llm::ask::ask_user
       ├─ parse_questions ── error → ToolOutcome::error (model retries)
       ├─ tx.send(AskUser(request)) ─────────► loop: App::open_ask (modal up)
       └─ gate.wait(id) … blocked …          user answers / declines / chats
            ▲                                 └─ Action::ResolveAsk
            └──────── gate.resolve(id, decision) ┘
       decision → ToolOutcome { output: display, context: result, ok }
  └─ run_agent → ToolAnswered / ToolRejected → the committed cell
       └─ result appended as the tool message → the model reads the answers
```

Esc/`/clear`/quit reap the blocked thread through the turn's `CancelToken`
(the gate's `wait` polls it); a request dropped without an answer is resolved
`Declined` at the loop bottom so no thread ever parks forever.
