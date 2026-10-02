# The `AskUserQuestion` tool — asking the user mid-turn

Claude Code's `AskUserQuestion`: the model asks the user 1–4 multiple-choice
questions and **blocks until they answer**, then reads the answers as the tool
result — or, when the user leaves it untouched for ten minutes, reads that
they are away and keeps working (*The timeout*, below). The UI is an inline modal — the permission prompt's sibling — with a
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
    `Declined`, or `Chat` (the "Chat about this" row) — or, with the user
    away, `TimedOut { answers, after }` (*The timeout*, below);
  - the two texts of every resolution: the **cell display**
    (`answered_display` — `User answered Alter Zero's questions:`, the
    headline naming *this* agent through the single `alter_zero::APP_NAME`,
    over `· Q → A`
    rows; `declined_display`/`chat_display` — the headline over
    `· Q (opt / opt / …)` rows) and the **model-facing result**
    (`answered_result` — the schema's `{"answers": {question: labels}}` JSON
    plus `annotations` carrying notes/previews; `declined_result`/
    `chat_result` — stop-and-wait instructions; `timed_out_display`/
    `timed_out_result` — the keep-working one);
  - `AskGate` — the `Arc<Mutex<…>> + Condvar` sibling of `PermissionGate`:
    `next_id` → `resolve(id, AskDecision)` → `wait(id, cancelled)`;
  - `AskTimer` — the question timeout's idle clock, pure over injected
    instants (*The timeout*, below).

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
  every key while open **except Ctrl+O and Ctrl+D**, stashes the composer
  draft (the ask text entry reuses `App::input`, like Tab's amend field), and
  queues cross-modal arrivals — a
  permission request landing while a question is open waits its turn, and
  vice versa (`open_next_pending`). Navigation: ←/→/Tab/Shift+Tab move
  between question tabs (and the Submit tab), ↑/↓ move rows wrapping at the
  ends, digits
  jump-activate, Enter selects/toggles/activates, `n` opens the notes field
  on a preview question, Esc **declines** (the whole call resolves declined —
  the turn continues; the model is told to stop and wait). The two read-only
  overlays are the sibling of the permission prompt's one exception
  (`docs/permissions.md`): `on_key_ask` runs `App::on_key_overlay_toggle`
  first, so Ctrl+O (the transcript) and Ctrl+D (the raw context) open over an
  open question — entry fields included, neither being an editing key — and
  the question is still there on the way back. A question *about* the
  conversation must not lock the two views that show it. The overlay's idle
  Esc is guarded too: a pending modal is not a backtrack target
  (`App::overlay_esc_backtracks`). A single-select
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
  hints → rule) whole, and `ask_height` reserves those rows clamped to the
  terminal (`ui::region_is_modal` covers the prompt so the close
  purge-rebuilds like the permission prompt's). A page taller than the
  terminal is a framed view like the rest (`docs/view-flow.md`): the paint
  **bottom-anchors** — the options, the hints and the closing rule stay on
  screen — and the skipped top (the chip strip, the question, the first
  options) **flows into the terminal's real scrollback**, where the
  terminal's own scrolling reads it. The retired top-drop clamp put those
  rows in *no* buffer at all, which on a small terminal read as "the modal
  hides the texts": the page opened mid-option with the question gone. A
  keystroke that changes the page (a tab move, an answer, the entry field)
  re-signs the flow and the boundary purge-rebuilds — the pickers'
  search-line rule — while a plain ↑/↓ between rows in the painted tail
  holds the flowed top. The chip strip marks answered questions `☒`,
  unanswered `☐`, the Submit tab `✔`, and lights the **current** chip on the
  cyan selection background. A question with option previews renders
  side-by-side: options left, the focused option's preview in a bordered
  panel right, the `Notes: …` line beneath it. The hardware cursor hides on
  the option menu (`ui::cursor_visible`, the permission rule) **while its
  seat tracks the highlighted `❯` row at the option text's column** — the
  permission prompt's seat, recorded by the builder with the rows so the two
  can never drift, which is what a terminal's cursor animation (kitty's
  trail and kin) lands on: the option being chosen, on every page (list,
  preview, review), never the far end of the bottom rule — and the cursor
  returns for the Other/notes text fields (`ask_cursor`, sharing the
  builder's geometry; a seat whose row the bottom anchor flowed into
  scrollback falls back to the region's far corner, the menus' marker-less
  rule).

- **The resolved cell** — `ui::tool` special-cases the ask tool: the
  first output line ("User answered Alter Zero's questions:") becomes the `●`
  header (green for a submission, red for a decline/chat) and the `· Q → A`
  rows render in the `⎿` gutter, wrapped — the reference's committed
  transcript, in this agent's name. While the call runs the generic `● AskUserQuestion(…)` header
  stands (visible only in Ctrl+O — the modal covers the live region).

- **The loop** — `Session.ask: AskGate`, always attached (bootstrap →
  `ModelSession` → every backend build, `DummyAi` included; asking is not a
  permission and does not follow `ALTER_ZERO_PERMISSIONS`).
  `StreamEvent::AskUser` opens the prompt; `Action::ResolveAsk` posts the
  decision on the gate; the loop bottom releases abandoned requests as
  `Declined` (the permission release's twin) and `/clear` clears the board.
  The question's **idle clock** (`ask::AskTimer`, read by the draw tick in
  `tui::ask`) runs beside it and posts `TimedOut` when nobody touches a key
  for the timeout.

- **The dummy demo** — a `Play::Asked` scenario (cue: "ask" + "question")
  drives the whole round trip offline: three questions — single-select,
  multi-select, and a previews+notes code-style pick — resolved through the
  real gate, closing on the shared handoff sentence (a timed-out run closes
  on carrying on without the user — `smoke.sh` Phase 128).

## Flow

```
model calls askuserquestion
  └─ execute closure → llm::ask::ask_user
       ├─ parse_questions ── error → ToolOutcome::error (model retries)
       ├─ tx.send(AskUser(request)) ─────────► loop: App::open_ask (modal up)
       │                                      + the next draw starts its idle clock
       └─ gate.wait(id) … blocked …          user answers / declines / chats
            ▲                                 └─ Action::ResolveAsk
            ├──────── gate.resolve(id, decision) ┘
            │                                 …or no key for the timeout
            │                                 └─ App::expire_asks (draw tick)
            └──────── gate.resolve(id, TimedOut) ┘
       decision → ToolOutcome { output: display, context: result, ok }
  └─ run_agent → ToolAnswered / ToolRejected → the committed cell
       └─ result appended as the tool message → the model reads the answers
```

Esc/`/clear`/quit reap the blocked thread through the turn's `CancelToken`
(the gate's `wait` polls it); a request dropped without an answer is resolved
`Declined` at the loop bottom so no thread ever parks forever.

## The timeout — when nobody answers

A question blocks the turn, and the user may have walked away. Before this,
a question asked of an empty room parked the agent until the user came back,
however long that took — an hour-long task left running over lunch spent the
hour waiting on its first question. Now a question waits on an **idle** user
for **ten minutes** (`ask::DEFAULT_ASK_TIMEOUT`) and then resolves
*unanswered*: the tool returns, the model reads that the user is not
available, and the turn keeps going.

- **Idle, not elapsed.** The clock runs while any question is pending — the
  open modal, or one queued behind a permission prompt — and **every key press
  or paste starts it over**, wherever the key lands: an option, an entry
  field, the Ctrl+O transcript opened over the modal. A user reading the
  options, typing an answer or scrolling the conversation to decide never runs
  it out; ten untouched minutes means the user is not there.
- **What the model reads** (`ask::timed_out_result`), short because it rides
  every later request:

  > The user did not answer within 10m and is not available. Continue working
  > without them: decide using your best judgment, preferring the safest, most
  > reversible option, and state your assumptions in your final response. Do
  > not ask again until the user sends a message.

  It is the opposite of a decline's stop-and-wait: a decline is the user
  *present* and saying no, a timeout is the user *absent*. "Safest, most
  reversible" keeps an unattended agent off the destructive branch of the
  question it could not get answered; "state your assumptions" is what the
  user reads when they come back; and "do not ask again until the user sends
  a message" keeps the next question from stalling another ten minutes on the
  same empty room — scoped to the next message, so it never outlives the
  user's return.
- **Partial answers are kept.** A user who picked the first of three
  questions and then left gave a real answer, so the timeout delivers it:
  `TimedOut { answers, .. }` carries the same answered set a Submit would
  (an entry field's unaccepted text is not an answer, and its paste pairs go
  with it), and the result becomes *The user answered some questions, then
  did not respond for 10m…* with the answers JSON last. A question still
  queued — never shown — resolves with none.
- **The cell** is the decline's shape, red: `User did not answer within 10m`
  (`User did not finish answering within 10m` when something was answered)
  over a `· Q → A` row per answer and a `· Q (A / B)` row per question left
  open.
- **The countdown.** While the clock runs, the modal's closing rule carries
  it right-aligned — `── continues without you in 9:41 ─`, dim, amber inside
  its last minute. The rule, because it is the one row a bottom-anchored page
  always paints: a ticking label there never re-signs the flow
  (`docs/view-flow.md`) and never costs the page a row. The count rounds up,
  so a fresh clock reads `10:00` and the last second `0:01`, never `0:00`
  over a question that is still open; a rule too narrow for it stays plain.
- **Why ten minutes.** Long enough for a user who is present but busy — back
  from another window, reading a preview, gone for a coffee — since any key
  starts it over; past it the user is away, and every further minute is the
  agent idling for nobody. Twenty doubles that idle time for the common case
  without catching a meeting or a lunch either, and a provider's prompt cache
  has usually expired by ten minutes anyway, so the longer wait buys nothing
  back.
- **Changing it.** The `/settings` **Ask timeout** row cycles `5m` / `10m` /
  `20m` / `30m` / `1h` / `never`, per directory (`docs/settings.md`);
  `ALTER_ZERO_ASK_TIMEOUT_SECS` seeds it for a run in seconds, `0` meaning
  never — any value, which is how the smoke suite sits one out in eight
  seconds (Phase 128).

The clock's rules are pure — `ask::AskTimer`, read with injected instants
and unit-tested like everything else here — and only time itself lives at
the boundary (`tui::ask`), the toast deadline's pattern. The timer keeps
**when the wait started**, never a deadline: no wait is ever added to an
`Instant`, so an `ALTER_ZERO_ASK_TIMEOUT_SECS` past anything a clock can
reach reads as a very long countdown instead of overflowing. Each reading
(`AskTimer::tick`) takes whether a question waits (`App::has_pending_asks`),
the open one's id (a different one opening starts the wait over, so every
question gets the whole wait on screen) and the row's wait, and answers
`Idle`, `Running(left)` or `Expired(wait)`; `AskTimer::touch` is every key
press or paste (`Session::note_user_activity`), and starts a running wait
over without arming one when nothing waits. The draw tick reads it
**before** the paint (`Session::tick_ask_clock`). Expired, `App::expire_asks`
closes the modal — handing the composer draft back — and drops every queued
question, and each decision goes up on the gate exactly as an answer would,
from the loop's own thread, so a key and the expiry can never race; the
parked tool thread wakes into `ask_user`'s `TimedOut` arm, that same frame
paints the modal closed, and a permission prompt queued behind it opens.
Running, the open modal's remaining time is injected
(`App::set_ask_remaining`) and a frame is kept booked a second away — no
later than the expiry, capped at an hour, under an overlay, where none of
it is on screen and the status chain stops re-arming — so the expiry needs
no input at all to fire. A timeout that lands under the Ctrl+O transcript
closes the modal underneath it, and the transcript follows the turn as it
carries on.

The tool's own description says so too: the user "may instead decline, ask
to chat, or not answer in time; the result then tells you how to proceed" —
it used to promise that every unanswered outcome meant stop and wait, which a
timeout's keep-working result now contradicts.
