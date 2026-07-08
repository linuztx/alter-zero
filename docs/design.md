# Inline Conversation TUI — Design

Date: 2026-06-07

## Goal

A minimal, readable, well-documented inline terminal UI (ratatui) for a
user ↔ AI conversation. The AI side is a **dummy** that streams a canned
response chunk-by-chunk, with **tool calls interleaved** in the reply. The
layout must be responsive to terminal width, and the visual style should echo
Claude Code (a bottom-pinned input field framed by a top/bottom rule, messages
and tool calls flowing above it in normal scrollback). Everything that can be
unit-tested must be unit-tested.

## Behaviour

- The app runs in ratatui's **inline viewport** (no alternate screen). Normal
  terminal scrollback is preserved — finished messages scroll up into your
  real terminal history, exactly like Claude Code.
- The **live region** stays pinned at the bottom:
  - an **input field framed by a top/bottom rule** (`❯ ...`),
  - and, **only while a turn is in flight**, a strip above the box: a **preview
    row** showing the in-progress AI line (or a running tool's blue header), a
    blank **gap row**, a **status line** —
    `(●•·   ) {verb}… ({elapsed}s · {↓|↑} {n} tokens · Thinking for {m}s · esc
    to interrupt)`, a
    codex/Claude-Code-style indicator (see *Status indicator* below) — then
    another blank gap row so the status clears the box's top rule. Idle, that
    strip collapses and the box sits directly under the chat.
- Type a message, press **Enter** to send. The user message is flushed to
  scrollback, then the dummy AI streams a reply.
- **Streaming → scrollback, line by line.** As the reply grows, each wrapped
  line that can no longer change (under greedy word-wrap only the last line
  can still change) is committed to scrollback via `insert_before`. The
  current partial line is shown live in the preview row, held off the box by the
  blank gap row. On completion the final line is committed too, with a blank
  spacer line after it — and as the streaming strip collapses, that committed
  spacer becomes the single blank line between the reply and the box (no double
  blank). A blank spacer is also committed after every user message.
- **Status indicator.** While a turn is in flight, the strip's status line opens
  with a **comet spinner** (a Larson-scanner sweep: a white head dragging a
  fading grey tail back and forth between dim walls, one frame per 80 ms —
  `ui::spinner_spans`),
  then a per-turn whimsical **verb** (`Working`, `Cooking`, …, picked
  deterministically by a turn counter) whose white text carries a codex-style
  **shimmer** — a bright-white raised-cosine band sweeping the chars every 2 s
  (`ui::shimmer_spans`, ported from openai/codex) — a
  **timer** in whole seconds, a cumulative **token** estimate (`↑` for uploaded
  tokens — the user's input at turn start and a tool result folded back — `↓`
  while the reply streams; never reset mid-turn),
  `Thinking for Ns` *only* while the model is in a thinking phase, and a closing
  dim **`esc to interrupt`** hint (codex's discoverability hint). The turn opens
  with a deliberate **pre-stream pause** (the backend waits before its first
  chunk — `DummyAi`'s `STARTUP_DELAY`, 3s, overridable via
  `INLINE_TUI_STARTUP_DELAY_MS`) so the indicator is visibly working first: the
  just-sent user message is counted up front (`App::count_user_input`, `↑`), so
  the pause shows `↑ N tokens` and the timer ticks. The strip reserves **no
  preview row** while there is nothing to preview (`strip_has_preview` false —
  the status sits one blank below the user message, no stray empty line, like
  codex); the preview row appears only once the reply streams (or a tool runs),
  and the first chunk flips the arrow `↓`. While a
  turn is active the draw branch re-arms an animation frame every 32 ms (codex's
  cadence), so the comet sweeps, the shimmer waves, and the timer moves even
  with no events. On
  finish the line is replaced by a dim, committed **`{done verb} for Ns`** summary
  that flows into scrollback (a `HistoryItem::Summary`, so it survives a resize
  and lists in the Ctrl+O transcript — stamp-free, like every non-user item).
  Time is impure, so — like
  the timestamp clock — the loop owns the `Instant`s and feeds the pure status
  only computed `Duration`s (`App::set_status_times`; the same value drives the
  displayed seconds and both animation phases). See `docs/status-indicator.md`.
- **Responsive:** every draw re-wraps to the current terminal width, measured in
  **display columns** (`unicode-width`) so CJK/emoji wrap and pad correctly.
- **Growing input box.** The input field is multi-line and grows downward as the
  text wraps (or as explicit newlines are added with **Ctrl+J / Alt+Enter /
  Shift+Enter** — `docs/shift-enter.md`), so a long message is never lost off the
  right edge. The live region height is
  therefore dynamic: `LIVE_MIN_HEIGHT` (a one-row box, no preview strip) at rest,
  growing one row per wrapped input line up to the terminal height (plus a preview
  + gap row while a reply streams), after which the box scrolls internally to keep
  the cursor's wrapped row in view (`ui::input_scroll` follows the cursor,
  wherever the textarea has it). The box is
  **content-anchored** like Claude Code / codex — its top stays put and it grows
  *downward*, only scrolling the chat up once it reaches the screen bottom (never
  jumping to the bottom). Geometry is pure (`ui::live_height`, `ui::repin`) and
  unit-tested.
- **Resize reflow (both directions, both dimensions).** Any size change must
  re-present the visible chat. A *width* change stales every wrapped line (the
  on-screen lines keep their old wrapping until redrawn). A *height* change is
  subtler: the emulator scrolls or clips the screen contents to fit the new
  height, moving them out from under the tracked viewport row — so repainting at
  the stale row leaves phantom input boxes behind and pushes the chat out of
  view (codex redraws everything from source on every resize and re-clamps its
  insert viewport into the new screen; we follow it). So `App` retains a
  `history` of finished messages and, on **any** dimension change,
  `term::resized` re-clamps the tracked viewport inside the new screen and the
  loop repaints from history: the viewport is seated at the top and the tail
  that fits above the live region is rewritten in place — re-wrapped to the new
  width, wider or narrower (`term::reflow`). Lines that had already scrolled
  into the terminal's own scrollback keep their original wrapping. Guarded by
  `smoke.sh` Phase 17 (a height-only shrink, a grow back, and a mid-stream
  shrink must each leave exactly one input box with the conversation tail in
  view).
- **Tool calls (Claude-Code style).** A reply can interleave tool calls. Each
  renders inline as a **coloured bullet header** `● name(args)` — **blue** while
  it runs (shown live in the bottom region's preview row), **green** when it
  succeeds, **red** when it fails — plus a **collapsed** one-line `⎿` peek of its
  output with a `(ctrl+o to expand)` hint when more is hidden. The full output is
  never shown inline. A tool call splits the assistant text around it: the run of
  text before a tool is committed as its own message so the scrollback (and the
  resize repaint) keeps text and tools in the exact order they streamed. The dummy
  runs a `Read` (green) then a `Bash` (red) per turn so all three colours show.
- **The Ctrl+O tool-output view.** Ctrl+O (from either screen, even mid-stream)
  opens a **separate full-screen overlay** — on the terminal's *alternate screen*,
  so the inline conversation is preserved — styled as **codex's Ctrl+T
  transcript pager**: a dim slash-tiled `/ T R A N S C R I P T` title row, the
  scrolling body with vi-style `~` filler rows past its end, a dim `─`
  separator carrying the **scroll percentage** right-aligned (0% top, 100%
  bottom), and two dim key-hint rows above a final blank row. The body is the
  **full conversation transcript**: every user/AI message **and** every tool
  call's **complete** (expanded) output, interleaved in the exact order they
  happened, plus the live tail (in-progress reply / running tool) **plus the
  still-queued backlog** (the inline strip's inset `  ❯ …` rows, so a queued
  message is never hidden — `docs/queue.md`), scrollable (↑/↓ PgUp/PgDn,
  Home/End jump). It **opens
  pinned to the bottom** and tail-follows new content as it streams in (scroll up
  to read back; scrolling to the bottom re-engages following). It is the
  expanded counterpart of the inline view (where tools are collapsed). The
  conversation **keeps streaming and updating underneath**: while the overlay is up
  the event loop still drains reply events into `App` (so the view updates live)
  but holds off committing to scrollback — and a turn that ends there still
  **dispatches the next queued entry immediately** (codex parity: the open
  transcript gains the new user entry and follows the new turn live, the
  deferred user bubbles regenerated from history on return); Ctrl+O (`q`, or
  Esc — unless idle with a previous user message, when Esc instead begins the
  backtrack preview in place, `docs/backtrack.md`) returns, and the inline
  view is repainted from `history` to catch up — and **so does quitting** (Ctrl+C)
  from the overlay, which repaints before exiting so a turn that finished while the
  overlay was up restores its `Done for Ns` summary rather than the stale streaming
  strip it left frozen on the main screen. The overlay is read-only (typing is
  ignored). Only the **user** message shows a **wall-clock timestamp** (12-hour,
  no seconds, e.g. `03:20 AM`): dim, **right-aligned on its own line below the
  message** (after a blank row) — the **only** stamp displayed anywhere (AI
  replies, tools, and turn summaries record one but never show it); the inline
  conversation never shows any. The clock is injected at the I/O boundary
  (`App::set_clock`, a real `chrono::Local` clock in `main.rs`; `None` in unit
  tests → empty stamp) and each item stores the pre-formatted string, so the
  pure library stays clock-free. See `docs/timestamps.md`.
- **Slash-command palette (Claude-Code style).** When the input is a **bare
  command token** — `/`, `/he`, `/help`, but *not* `ask /help` or anything past a
  space/newline — a scrollable command **palette opens below the input box** (a
  third band in the live region). It lists a registry of `SlashCommand`s
  (`app::COMMANDS`: name + description + effect — currently `/help`, `/clear`,
  `/copy`, `/resume`, `/model`, `/login`, and `/quit`),
  filtered by name-prefix as you type after the `/`; `/` alone lists everything.
  ↑/↓ move the highlight (the window scrolls, capped at `MENU_MAX_ROWS`, to keep it
  visible); descriptions line up in a column (names padded to `MENU_DESC_COL`), and
  the selection is shown **by colour** — the whole highlighted row lights up cyan
  (name *and* description the same colour) while the others are dimmed grey — **no
  caret/arrow**.
  **Tab/Enter run** the highlighted command; **Esc** dismisses the palette (instead
  of quitting) and stays dismissed within the same token (delete the `/` and retype
  to reopen). The box's top is unchanged when the palette opens — it's reserved
  *below* the box — so the cursor never jumps. Running a command **consumes the
  input** and dispatches an `Action`: `/clear` → `Clear` (a full wipe — see
  below), `/help` → `Notice` (lists the commands), `/quit` → `Quit`
  (exits — codex's `/quit`/`/exit`, "exit Codex"), and `/copy` →
  `Copy(Option<String>)` (codex's `/copy` — the last assistant response to the
  system clipboard; the loop does the arboard/OSC 52 write at the boundary and
  commits a system or red error notice, see `docs/copy.md`), and `/resume` →
  `OpenResumePicker` idle or `ErrorNotice(RESUME_BUSY_NOTICE)` mid-turn
  (codex blocks it while a task runs — see `docs/resume.md` and the /resume
  bullet below). A `Notice` is recorded as
  a `Role::System` message and committed to scrollback like any other. Adding a
  command later is a one-line registry edit + an effect arm in
  `run_selected_command` — the palette, filtering, scrolling, and dispatch don't
  change.
  **`/clear` mid-turn is a kill.** `App::clear_conversation` wipes the whole
  conversation state — `history`, the streaming buffer, a running tool, the
  live status, *and the queued backlog* — recording nothing (no partial, no
  interrupt notice, no summary), and the loop's `Clear` arm cancels + detaches
  the in-flight backend and swaps in a fresh reply channel (`abandon_inflight` —
  the Esc-interrupt dance minus the commits; never `join()`ing on the loop, the
  interrupt-lag fix in `docs/interrupt.md`) before the blank repaint, so a stale
  chunk or `ToolStart` can't repopulate the cleared state and stream into the
  fresh screen. Codex instead
  *disables* `/new`/`/clear` while a task runs (`available_during_task` → the
  red `'/clear' is disabled while a task is in progress.` notice; Tab can queue
  one for the turn's end — `QueuedInputAction::ParseSlash`); killing is a
  deliberate divergence: `/clear` here means "stop and wipe *now*". The
  ↑-recall input history still survives (drafts aren't part of the
  conversation). Guarded by `smoke.sh` Phase 16: nothing may stream in after a
  mid-turn `/clear`, and the next turn must run normally.
- **`?` shows a shortcuts band** (codex's footer shortcut overlay — see
  `docs/shortcuts.md`): pressing `?` (shift-modified or not) in an **empty
  composer** toggles a keyboard-shortcuts overview in the palette's slot below
  the box — two aligned columns of `{key} for {thing}` entries (keys cyan,
  labels dim) listing `/`, `!`, `↑`, `ctrl+r`, `alt+enter`, `ctrl+o`, `esc`,
  `ctrl+c`, `alt+↑` (edit queue), `tab` (queue next turn), and `ctrl+v`
  (image paste);
  the `esc` entry reads `to interrupt` while a turn runs and `to quit` idle.
  With a draft in the box `?` is just a character. The band is display-only,
  never modal: any other key closes it and still performs its action — except
  Esc, which only dismisses (the palette's Esc rule; idle Esc would otherwise
  quit). The band and the palette never show together, and the cursor stays
  put when the band opens (it is reserved below the box).
- **Queue a message while a turn streams** (codex's `queued_user_messages` — see
  `docs/queue.md`): pressing Enter *during a turn* doesn't wait — the message
  joins `App::queued` (consuming the composer, recorded in `input_history` for ↑
  recall) and shows **above the box**, in the streaming strip under the status
  line, inset two columns and styled exactly like a sent user message (`❯`
  bullet, dark background, wrapped) — every queued entry, uncapped. The queue is
  a sequence of **typed turn-entries** (`VecDeque<QueuedTurn>` — text
  `Messages { texts, images }` batches and standalone `Shell` commands, see
  `docs/queue.md`): **Enter appends to the current batch** (consecutive Enters
  share one next turn — Claude-Code batching) while **Tab opens a new batch**
  (codex's Tab-to-queue — its message
  runs as a *separate follow-up turn* after the ones already queued, a blank row
  dividing the batches in the strip). When the turn ends the loop pops the
  **front batch** as the next turn (`App::drain_next_batch`, FIFO —
  `main.rs::start_turn`, shared with `Submit`; each message commits as its own
  bubble, the backend gets one joined prompt), so the batches iterate
  one-per-turn-end in order, and **Esc interrupts the current turn and sends the
  front batch right away**. **Alt+Up** pulls the **last batch** (`pop_back`,
  `App::drain_last_batch`) back into an empty composer as one newline-joined draft
  (its own messages joined) to edit, extend, or drop — earlier batches stay queued
  (codex's `edit_queued_message` pops the most recent entry the same way).
  Idle/empty Tab is a no-op. Slash
  commands aren't queued (they run inline via the palette). The composer stays
  **focused** throughout: the
  hardware cursor stays visible on the box's prompt row while the turn streams
  (codex keeps the composer cursor during a running task; only the Ctrl+O
  overlay hides it).
- **A session-context footer under the box** (codex's footer status line — see
  `docs/footer.md`): the live region's last row shows `{model} · {cwd}` — the
  backend's `ReplySource::model_name()` and the home-relativized working
  directory (`display_cwd`: `~`, `~/rel`, or absolute) — dim, inset two
  columns, ellipsis-truncated at narrow widths. Present from startup, idle and
  mid-stream alike; the palette / `?` shortcuts band **displaces** it (codex's
  popups take the row the same way), and it returns when the band closes. The
  strings are injected once at the I/O boundary (`App::set_session_info`, the
  `set_clock` pattern) so the pure core never reads the environment.
- **Backend errors & cancellation.** A reply backend (`ReplySource`) may end with
  `Error(msg)` instead of `StreamDone`; the partial reply (if any) is kept and a
  red error notice is shown below it. The built-in `DummyAi` never errors — this is
  the seam for a real model. Quitting mid-stream trips a `CancelToken` so the
  backend stops promptly and its thread is reaped before exit.
- **Real LLM backend & the dummy toggle** (`docs/llm.md`). The `llm` module is a
  drop-in `ReplySource` for any OpenAI-compatible endpoint: it streams
  `/chat/completions` over blocking `reqwest` + a hand-rolled SSE reader (so it
  stays on the same plain OS thread as `DummyAi`, polling the `CancelToken`
  between frames), splitting `<think>`/native `reasoning` deltas out into the
  `Thinking for Ns` status. `main.rs` holds the backend as a `Box<dyn
  ReplySource>` so it can be swapped live. The **dummy is the default and the
  fallback** — the real backend activates only when a provider, model, and API
  key all resolve and `INLINE_TUI_DUMMY` isn't set — so the app always runs
  offline and `smoke.sh` (which configures none of that) stays on the dummy.
- **`/model` picker** (`docs/llm.md`). An **inline** picker (it replaces the
  composer in the bottom region, unlike the alternate-screen `/resume`): `/model`
  fetches **every configured provider's** `/v1/models` in parallel (one worker
  thread each), merging them into one provider-tagged list as they land (a
  provider that fails is noted beside the counter, the rest staying put), lists
  them with type-to-search (`→` marks the selection, `❯` the search prompt,
  headerless), and
  `Enter` rebuilds the backend for the chosen model, updates the footer, and
  **persists the choice** to `~/.inline-tui/config.json` (`llm::settings::Settings`)
  so it's the default next run. With no provider configured it shows a cyan
  `run /login` hint instead of a list. Rejected mid-turn like `/resume`.
- **`/login` API-key onboarding** (`docs/llm.md`). A second **inline** flow,
  two-step: pick a provider (headerless list), then paste its API key (masked,
  under a periwinkle `Enter your … API key` prompt). On save the key is written to
  `~/.inline-tui/.env` (`llm::keystore::EnvFile`, a pure `.env` reader/writer) so
  it **persists across runs** — key resolution consults the real process env
  first, then this file. Because `std::env::set_var` is `unsafe` (forbidden here),
  the loaded keys live in an in-memory map, never the process env. Rejected
  mid-turn like `/model`.
- **Esc interrupts a streaming turn** (ported from openai/codex — see
  `docs/interrupt.md`): a single Esc while a turn is in flight cancels + detaches
  the backend (never `join()`ing it on the loop — a backend parked in a blocking
  network read would otherwise freeze the UI for up to one op-timeout, the
  interrupt-lag fix), keeps the partial reply, resolves a still-running tool as
  failed (`Interrupted by user`), and commits a red
  `Conversation interrupted - tell the model what to do differently.` notice —
  with **no** `Done for Ns` summary (the notice is the turn's terminal state).
  The palette still wins: Esc with the palette open only dismisses it, even
  mid-turn. The status line's `esc to interrupt` hint advertises this.
- **Esc Esc edits a previous message** (codex's backtrack — see
  `docs/backtrack.md`): from an idle, empty composer with a previous user
  message, the first Esc **arms** the gesture (the footer slot shows
  `esc again to edit previous message`; any other key disarms), the second
  opens the Ctrl+O transcript overlay as a **preview** with the newest user
  message highlighted (reversed video, scrolled into view), Esc/← step
  older / → newer (clamped), and **Enter rewinds**: the highlighted message
  and everything after it leave the history, the overlay closes, the truncated
  conversation repaints, and the message's text lands back in the composer to
  edit and resend. `q`/Ctrl+O cancel. Esc quits only when there is nothing to
  backtrack to (a fresh session, or right after `/clear`) — quitting otherwise
  is Ctrl+C or `/quit`.
- **↑/↓ recall submitted messages** (shell-style, ported from codex's
  `ChatComposerHistory` — see `docs/input-history.md`): with an **empty
  composer**, ↑ recalls the last submitted message (older with further
  presses, clamping at the oldest), ↓ steps back toward the newest and **past
  it clears the composer**. Recall replaces the draft with the cursor at the
  end; the gate (`InputHistory::should_navigate`) keeps the arrows' day job —
  a typed draft, an edited recall, or an interior cursor falls through to
  normal cursor movement, and the open palette still intercepts ↑/↓ first.
  Adjacent duplicate submissions collapse; the history survives `/clear`
  (codex's spans whole sessions); recalling a bare `/token` re-derives the
  palette like typing it.
- **Ctrl+R reverse-searches that history** (codex's reverse incremental
  search — see `docs/history-search.md`): the footer slot becomes a
  `reverse-i-search: {query}` line (query cyan; on a match `enter accept ·
  esc cancel` hints; on a miss a red `no match`), the hardware cursor moves to
  the end of the query, and **every key belongs to the search** while it's
  open. Typing filters case-insensitively (substring, newest first, duplicate
  texts collapsed) and previews the newest match in the composer with the
  query occurrences highlighted reversed+bold; Ctrl+R/↑ step older and
  Ctrl+S/↓ step newer (clamping at both ends — the match is kept, codex's
  `AtBoundary`); Backspace/Ctrl+H pop the query, Ctrl+U clears it; a no-match
  query shows the original draft again but keeps the search open. **Enter
  accepts only an actual match** — the search closes, the text stays as an
  editable draft (cursor at the end), ↑/↓ browsing is seated at the accepted
  entry, and a bare `/token` re-opens the palette like a recall. **Esc/Ctrl+C
  cancel**, restoring the pre-search draft *and cursor* (Esc never reaches
  interrupt/quit — the palette-dismiss precedent; Ctrl+C neither clears nor
  quits); Ctrl+O cancels first, then opens the overlay.
- **`!` runs a local shell command** (codex's `!` shell mode — see
  `docs/shell-command.md`): typing `!` first **absorbs** into `App::shell_mode`
  (codex's `is_bash_mode`) — the bang becomes the composer's red `! ` prompt
  (`! pwd`, never `❯ !pwd`) and the footer flips to a red `Shell mode` hint;
  Backspace/Esc on the empty shell composer exit the mode, and the palette/`?`
  band are suppressed while it's on. Enter from an **idle** composer runs the
  draft under `sh -c` on a background thread, reusing the turn machinery
  (`App::begin_shell`) to commit a codex-style **exec cell**: the `! command`
  header on the dark user-style line (a `Role::Shell` message, recorded up
  front so a mid-run resize repaints it) with the command's `⎿` output **flush**
  below — `⎿ Running…` while it runs (the headerless shell tool's peek is the
  strip preview), the live `Running…`/`esc to interrupt` status above the box,
  **no** `Ran for Ns` summary (the cell is its own record), full output in the
  Ctrl+O view. Esc interrupts a long one (kills the child; the cell resolves
  `⎿ Interrupted by user`). The full `!command` is recorded for ↑ recall /
  Ctrl+R (recall re-absorbs the bang). A bare `!` posts a help notice; mid-turn
  a `!command` queues as a standalone `QueuedTurn::Shell` entry, run **locally**
  when its turn comes (codex parity — never merged into a text batch; see
  `docs/queue.md`).
- **`@` opens a file picker** (codex's `@` mention popup, Claude-Code-style — see
  `docs/file-search.md`): whenever the cursor sits in an `@token` (an `@` at
  start-of-line or after whitespace, so `email@host` never triggers), a fuzzy
  file list shows **below the box** (the palette's slot — the bands are mutually
  exclusive). The boundary's background worker walks the cwd **once** and ranks
  it per query off-thread — codex's async `StartFileSearch`/`FileSearchResult`
  round-trip (`App::file_search_query` changes drive a `dispatch_file_search`;
  results return via `App::set_file_matches`, a staleness guard dropping ones the
  token has outrun). ↑/↓ move the highlight, **Tab/Enter insert the path**
  (replacing the `@token`, a trailing space added, whitespace paths quoted), Esc
  dismisses (sticky within the token, like the palette). The query-matched
  characters are bolded in each row; `Searching…`/`No matching files`
  placeholders cover the in-flight/empty states. Suppressed in `!` shell mode.
  The path is inserted as **plain text** — it just becomes part of the message
  (codex leaves raw file paths literal too; no on-the-wire encoding).
- **A large paste collapses to a placeholder** (codex's large-paste handling —
  see `docs/paste.md`): a bracketed paste (`Event::Paste`) longer than
  `LARGE_PASTE_CHAR_THRESHOLD` (1000 chars) drops a compact
  `[Pasted Content N chars]` placeholder into the composer instead of the text
  (same-count collisions get a ` #2`, ` #3`, … suffix — `next_paste_placeholder`);
  the real text is remembered in `App::pasted` and spliced back in on send
  (`paste::expand_pastes`), and Backspace/Delete removes a placeholder
  **atomically** (`placeholder_to_delete`), dropping its stored text. A small
  paste inserts verbatim, indistinguishable from typing.
- **Ctrl+V pastes a clipboard image** (codex's clipboard image attach — see
  `docs/image-paste.md`): Ctrl+V / Ctrl+Alt+V returns the pure
  `Action::PasteImage`; the boundary reads the system clipboard
  (`clipboard::read_clipboard_image` — arboard, file-list first, raw RGBA
  fallback), re-encodes to a kept temp PNG, and `App::attach_image` drops an
  `[Image #N]` placeholder into the composer. On send the paths travel a
  separate typed channel to the backend (`ReplySource::spawn`'s
  `images: Vec<PathBuf>` parameter) — a real vision backend attaches the files,
  the dummy acknowledges the count. A failed read commits a red
  `Failed to paste image: {msg}` notice; a discarded attachment's temp PNG is
  deleted at the boundary (`App::take_discarded_images`).
- **`/resume` picks up a saved session** (codex's `/resume` — see
  `docs/resume.md`). Every conversation records to a rollout JSONL file
  (`~/.inline-tui/sessions/YYYY/MM/DD/rollout-…-{id}.jsonl`, overridable via
  `INLINE_TUI_SESSIONS_DIR`): a `session_meta` line, then one line per
  finished `HistoryItem` — completed items only, never streaming deltas
  (codex's persistence policy). The file is created lazily on the first
  recorded item (empty sessions never touch disk), a backtrack rewind
  rewrites it, and `/clear` starts a fresh one (codex's `/new`). `/resume`
  (rejected mid-task with a red notice, like codex) opens a **full-screen
  picker** on the alternate screen — the transcript pager's chrome with dense
  `❯ {age:12}{first-user-message}` rows (the selected one lit on a full-width
  background tint, codex's blend), newest first by the active sort key,
  type-to-search filtering, codex's **Filter/Sort toolbar** on the search
  row's right edge (`Filter: [Cwd] All   Sort: [Updated] Created` — Tab moves
  the focus, ←/→ toggle; `Cwd`/`Updated` default, the toolbar compacting then
  dropping at narrow widths), `{selected+1}/{total}` on the bottom rule — and
  Enter loads the
  chosen file's history into `App`, repaints it inline (the resize path), and
  **appends the turns that follow to the same file**; Esc clears the query
  first and cancels second, Ctrl+C cancels too (codex's from-a-session
  picker). The pure format/parse/preview logic is `session.rs`; the recorder
  and dir scan live at the boundary (`main.rs::SessionRecorder`).
- **Quit:** Ctrl+C, the `/quit` command, or Esc in the conversation while
  **idle, with an empty composer and no previous user message to edit** —
  mid-turn Esc interrupts, once a user message exists idle Esc arms the
  **Esc-Esc backtrack** (edit a previous message; `docs/backtrack.md`)
  instead of quitting, and with a typed draft Esc is a no-op like codex's
  composer (it never throws typed work away — Ctrl+C is the composer-clear).
  **Ctrl+C first clears a non-empty
  input** (codex's composer-clear step: a first press with a typed draft only
  empties the box — recording the draft so ↑ can bring it back — and closes
  the palette, since the emptied input is no longer a `/token`; the overlay
  has no input box, so Ctrl+C there always quits); with an empty input it
  quits from anywhere, even mid-stream. In the tool-output view Esc (or `q`,
  codex's pager close key) returns to the chat instead of quitting — unless
  idle with a backtrack target, when Esc begins the preview in place. Sending
  is disabled while a reply is streaming.

## Architecture

Built as a small **library** (`lib.rs`) + a thin **binary** (`main.rs`) so the
logic is unit-testable without a real terminal.

| File        | Responsibility | Tested? |
|-------------|----------------|---------|
| `stream.rs` | The backend seam: the `ReplySource` trait (sends on a **tokio** `UnboundedSender<StreamEvent>`; `model_name()` names the backend for the session footer) + built-in `DummyAi` impl (with a configurable `STARTUP_DELAY` pre-stream pause — `with_startup_delay`), a `CancelToken`, and the `StreamEvent` protocol (`Chunk`/`ToolStart`/`ToolEnd`/`ThinkingStart`/`ThinkingChunk`/`ThinkingEnd`/`Error`/`StreamDone`); plus pure `dummy_response`/`chunks`/`turn_events` (the interleaved thinking + tool script). | Pure parts, token & dummy: yes |
| `llm/`      | The **real OpenAI-compatible backend** (`docs/llm.md`): `config` (the `providers.toml` parse + `ModelConfig`/`Selection` resolution), `thinking` (`ThinkingSplitter` — peels `<think>`/native `reasoning` deltas out of the stream), `openai` (`OpenAiClient` — pure endpoint/payload/SSE-parse + the blocking SSE `stream_chat`), `models` (`parse_models` + the `/v1/models` `fetch_models`, the `ModelEntry` picker row), `keystore` (`EnvFile` — the pure `.env` reader/writer `/login` persists keys through), and `backend` (`LlmBackend: ReplySource` — bridges the split deltas to `StreamEvent`s). The dummy is the default/fallback; the real backend activates only when a provider+model+key resolve and `INLINE_TUI_DUMMY` isn't set. | Pure cores: yes (HTTP: manual) |
| `app.rs`    | State + pure update logic: `App` (its `input` is a `TextArea`), `on_key -> Action` (per `View`; routes editing/cursor keys to the textarea), `push_chunk`/`finish_stream`/`flush_streaming_segment`/`interrupt_turn`, `start_tool`/`end_tool`, the message+tool `history`, **the ↑/↓ input-history recall** (`InputHistory` — record/gate/up/down, `docs/input-history.md`), **the Ctrl+R reverse search over it** (`HistorySearch`/`SearchState` + `InputHistory::search`/`entry`/`resume_at`, every key routed to `on_key_search` while open, `docs/history-search.md`), **the `!` shell-command mode + dispatch** (`shell_mode`/`sync_shell_mode` — the absorbed bang — `shell_query`, `Action::RunShell`, `begin_shell` + `Role::Shell`, `docs/shell-command.md`), **the `?` shortcuts-band toggle** (`shortcuts_open`, `docs/shortcuts.md`), **the mid-turn message queue** (`queued` turn-batches/`drain_next_batch`/`drain_last_batch`, Enter-appends + Tab-new-batch + Alt+Up edits the last batch, `docs/queue.md`), **the session info** (`session`/`set_session_info`, boundary-injected for the footer, `docs/footer.md`), the tool-view scroll, **the slash-command palette** (`command_query`/`matching_commands`, `COMMANDS`, open/filter/scroll/dispatch), **the `@` file picker** (`FileSearch` state + `refresh_file_search`/`file_search_query`/`set_file_matches`/`move_file_selection`/`accept_file_selection` — matches arrive asynchronously from the boundary; `docs/file-search.md`), **the `/resume` picker** (`ResumePicker` state + `open_resume_picker`/`close_resume_picker`/`on_key_resume_picker`/`load_session` — the sessions arrive from the boundary's scan; `docs/resume.md`), **the inline `/model` picker** (`ModelPicker`/`ModelLoad`/`ModelFetchError` state + `open_model_picker`/`close_model_picker`/`begin_model_load`/`add_models`/`add_model_error`/`on_key_model_picker` — the per-provider model lists arrive in parallel from the boundary's fetches and merge; `docs/llm.md`), **the inline `/login` onboarding** (`KeyOnboarding`/`KeyStep`/`ProviderChoice` state + `open_key_onboarding`/`close_key_onboarding`/`on_key_key_onboarding`/`paste_into_key_onboarding` — the provider choices arrive from the boundary; `docs/llm.md`). `Action`/`Role`/`Message`/`StreamError`/`InterruptedTurn`/`ToolStatus`/`ToolCall`/`HistoryItem`/`QueuedTurn`/`View`/`SlashCommand`/`CommandEffect`/`CommandMenu`/`FileSearch`/`ResumePicker`/`ModelPicker`/`InputHistory`/`SessionInfo` types. | Yes |
| `textarea.rs` | The **codex-style editable input** (`TextArea`): `text` + a movable `cursor`, a width-keyed `wrap_cache`, and a `preferred_col` for vertical motion. Insert/delete at the cursor, grapheme ←/→, wrapped ↑/↓ (logical-line fallback when the cache is cold), Home/End, byte-range wrapping (`wrapped_rows`/`display_rows`/`cursor_row_col`/`row_count`), and `replace_range` (swap a span — the `@token` for a path). Focused port of codex's editing core; see `docs/textarea.md`. | Yes |
| `file_search.rs` | The **pure core of the `@` file picker** (`docs/file-search.md`): `at_token` (the `@token` under the cursor — byte range + query), `fuzzy_match` (ASCII-case-insensitive subsequence + score + matched-char indices), `rank_files` (filter/sort/cap), and the `AtToken`/`FileMatch` types. The filesystem walk + async plumbing are the boundary's (`main.rs`); this is all pure. | Yes |
| `markdown.rs` | The **pure block parser for assistant replies** (`docs/markdown.md`): `parse_blocks` splits prose runs from fenced code blocks (` ``` `/`~~~`, verbatim, prefix-stable), `fence_lang` extracts the info-string language, `heading_level` classifies an ATX heading line. `ui::assistant_lines` renders these; code stays byte-for-byte behind a dim gutter, headings bold. Inline emphasis / lists / tables are deliberately out (they need source-newline-gating or tail-holdback). | Yes |
| `highlight.rs` | **Dependency-free syntax highlighting** for code blocks (`docs/markdown.md`): a generic tokenizer (`highlight` → `Vec<Vec<Seg>>` of `Kind` = keyword/string/comment/number/function/plain) with per-language comment styles and left-to-right multi-line string/comment state — prefix-stable, colour-agnostic (`ui` maps `Kind`→colour). No `syntect`/grammars; degrades to plain text for unknown languages. | Yes |
| `ui.rs`     | Pure rendering: `wrap_text` (display-width via `cols`, for **messages**), `message_lines`, `tool_lines` (collapsed inline) / `transcript_lines` (full conversation + expanded tools), `stable_commit`/`final_commit`, `conversation_lines`/`repaint_lines`/`repaint_budget`, the growing-input geometry (`live_height`, `repin`, `cursor_position`, `restore_cursor_row`, `input_scroll` — follows the textarea cursor), the **command-palette band** (`menu_rows`, `menu_window`, `command_menu_lines`), the **`?` shortcuts band** sharing its slot (`shortcuts_rows`, `shortcuts_lines`), the **`@` file picker** sharing it too (`file_menu_rows`, `file_menu_lines`, `file_menu_row` — selected row cyan, query-matched chars bolded; `docs/file-search.md`), the **queued messages** rendered above the box in user-message style (`queued_rows`, `queued_lines`), the **session footer** on the region's last row (`footer_rows`, `footer_line`, `display_cwd`), the **Ctrl+R search line** taking that slot while a search is open (`search_line`, the query-end cursor in `cursor_position`, `highlight_row_spans` for the reversed match preview), the **`!` shell-mode hint** taking the same slot (`shell_mode_line`; the red `SHELL_BULLET` composer prompt; `message_lines(Role::Shell…)` exec-cell headers, headerless shell `tool_lines`, and `conversation_lines`' flush shell cells), `render_live`, `render_tool_view`, and the **`/resume` picker overlay** (`render_resume_picker` + `resume_row` — dense `❯ {age:12}{preview}` rows, the shared `overlay_header`/`rule_with_label` chrome; `docs/resume.md`). | Yes |
| `frame.rs`  | Frame scheduling (codex-style): `FrameRateLimiter` (120 fps floor) + `soonest` request-coalescing (pure), and the async `FrameRequester`/`run_scheduler` task that turns a flood of `schedule_frame` calls into one rate-limited draw tick. | Pure parts: yes (async task: smoke) |
| `paste.rs`  | Two pure paste jobs (`docs/paste.md`): **burst detection** — `PasteBurst`, a pure state machine (a run of characters within `BURST_CHAR_INTERVAL` is a burst once `BURST_MIN_CHARS` pile up, so the loop relaxes the run's redraws) — and the **paste placeholders** — `LARGE_PASTE_CHAR_THRESHOLD` + `next_paste_placeholder` (`[Pasted Content N chars]`, collision-suffixed), `next_image_placeholder` (`[Image #N]`, `docs/image-paste.md`), `expand_pastes` (splice the real text back on send), and `placeholder_to_delete` (Backspace removes a placeholder atomically). | Yes |
| `clipboard.rs` | Clipboard I/O — the boundary for **Ctrl+V image paste** (`read_clipboard_image`: clipboard image or copied image file → a kept temp PNG the backend reads by path — `docs/image-paste.md`) and **`/copy`** (`copy_to_clipboard`: arboard with an OSC 52 terminal-escape fallback for headless/SSH/tmux — `docs/copy.md`). The `base64`/OSC 52 framing is a tested pure core; the clipboard/filesystem I/O is smoke-covered like `term.rs`. | Pure parts: yes (I/O: smoke) |
| `session.rs` | The **pure core of `/resume`** (`docs/resume.md`): the rollout JSONL format (`meta_line`/`item_line` on serde'd module-local record types — the app types stay serde-free), the forward-compatible `parse_session` (malformed/unknown lines skip), the picker's `preview_of` (first user/shell message, flattened) and `relative_age` (codex's `now`/`Ns`/`Nm`/`Nh`/`Nd ago`), `rollout_rel_path` (the dated layout), and the `SessionMeta`/`SessionSummary` types. The recorder + dir scan are the boundary's (`main.rs::SessionRecorder`/`list_sessions`). | Yes |
| `term.rs`   | The custom inline viewport over `CrosstermBackend`: dynamic content-anchored height, `insert_before` (queues scrollback lines for the next frame — `docs/flicker.md`), `draw` (pending-flush + re-pin + diff + cursor, one synchronized update), `reflow` (tail rebuild + live-region paint, one frame), the alternate-screen overlay (`enter_overlay`/`exit_overlay`/`draw_overlay`), init/restore (restore flushes leftovers). | No (I/O boundary) |
| `main.rs`   | Thin glue: single-threaded **async (tokio) `select!`** loop over input (`EventStream`), reply events (tokio channel), draw ticks, **and `@` file-search results** (a fourth channel); drives `term` (commits, draw, resize/return repaint, overlay), branches rendering on `View`, backend cancel/reap on quit; runs a `!command` locally (`run_shell`/`spawn_shell_command` — `sh -c` on a thread, output back over the reply channel as a tool, `docs/shell-command.md`); runs the **`@` file-search worker** (`spawn_file_search_worker`/`walk_files`/`dispatch_file_search` — a background walk+rank thread, `docs/file-search.md`). | No (tiny I/O boundary) |

### Data flow

The loop is **async** (tokio, single-threaded `current_thread` runtime), built as a
`select!` over five sources — exactly how openai/codex drives its TUI. Terminal
input arrives on a crossterm **`EventStream`**, the reply streams in on a tokio
channel, **draw ticks** come from the frame scheduler, the **`@` file-search
worker** answers on a fourth channel (`docs/file-search.md`), and each Ctrl+V
**image-paste worker** delivers its temp file on a fifth
(`docs/image-paste.md`). `select!` polls its branches in randomized order, so
input and draws can't starve each other (codex's explicit round-robin fairness,
for free). The async-rewrite design lives in `docs/async-rewrite.md`.

The `EventStream` is the **sole** stdin reader, created *after* `InlineViewport::init`
has queried the cursor position over stdin once, synchronously. The reply backend
runs on a background thread that only *sends* on its channel — it never reads stdin.
A second stdin reader would steal the cursor-position (DSR) reply — the cause of the
"cursor position could not be read" error. (`insert_before` tracks the viewport row
itself and never queries the cursor.)

Rendering is **tick-driven**: every state change calls `frame.schedule_frame()`,
and the scheduler coalesces a burst of those into a single draw, rate-limited to
120 fps (`MIN_FRAME_INTERVAL` = 8.33 ms). A paste / fast-type run is recognised by
`PasteBurst`, so its characters request *relaxed* frames (`schedule_frame_in` — no
immediate demand per key); the rate-limit floor is what coalesces the run into a
few repaints (the scheduler keeps the soonest pending deadline, codex's
earliest-wins fold). `insert_before` only
**queues** its lines (codex's `pending_history_lines`): the draw tick writes them
above the viewport and repaints the live region in **one synchronized frame**, so a
scrollback commit can never flash a boxless state — the streaming-flicker fix, see
`docs/flicker.md` (`reflow` paints the same way; `restore` flushes anything a
quit-before-tick left queued). Guarded by `scripts/smoke.sh` Phase 6 (a 1000-char
burst must finish rendering near-instantly) and Phase 15 (a raw byte recording of a
streaming turn must show every live-region clear inside a sync block).

```
keyboard / resize ──► EventStream ─┐
reply backend ─────► tokio mpsc ───┤─► select! ─► App::on_key / push_chunk / start_tool / set_file_matches / … ─► schedule_frame
frame scheduler ───► draw-tick ────┤                                                          │
file-search worker ► tokio mpsc ───┘                           draw tick ◄── coalesce + 120fps ┘ ─► draw / draw_overlay
```

- On `Submit(text)`: `insert_before` the user message and a blank spacer, then
  `backend.spawn(text, images, tx, cancel)` — the Ctrl+V image paths drained
  from `App::take_submission_images` (`docs/image-paste.md`) — a thread that
  sends `Chunk(..)*` with `ToolStart`/`ToolEnd` pairs interleaved, then
  `StreamDone` (or `Error(msg)`). The loop keeps the thread's `JoinHandle` and
  `CancelToken` so quitting mid-stream cancels it (and detaches the thread — it
  finishes on its own; the loop never `join()`s a backend, so a wedged network
  read can't delay the terminal restore — the interrupt-lag fix in
  `docs/interrupt.md`).
- On `Chunk`: append to the streaming buffer; commit any newly-stable lines to
  scrollback; redraw (preview row shows the partial last line).
- On `ToolStart{name,args}`: `flush_streaming_segment` finalises the run of text
  before the tool (so it slots ahead of the tool in order) and commits its
  remainder; `start_tool` shows the tool running (blue) in the preview row.
- On `ToolEnd{output,ok,truncated}`: `end_tool` records the finished tool; commit it
  *collapsed* (green/red) to scrollback. The full output is kept for the Ctrl+O
  view.
- On `ThinkingStart`/`ThinkingEnd`: the loop flips its `thinking_start` `Instant`
  so the status line shows/drops `Thinking for Ns`; nothing is committed (thinking
  is live-only). Each `ThinkingChunk` in between is counted into the token tally
  (`App::push_thinking` — the text is opaque, never rendered), so the count keeps
  ticking while the model thinks.
- On `StreamDone`: clear streaming state and end the turn (record the `Done for Ns`
  summary), then commit the final text segment + spacer + the summary. Because the
  streaming strip (preview + gap + status + gap) is drawn *above* the box, the loop first
  calls `term.set_view_height` to reseat the viewport to its idle height, so the
  final commit replaces the strip's rows in place and the box stays flush at the
  bottom rather than rising and leaving blank rows beneath it (see the
  streaming-strip note under *Known limitations*).
- On `Error(msg)`: `App::fail_stream` records any non-empty partial reply, flushes
  it, resolves a still-running tool as failed (`Interrupted by a backend error` —
  the contract allows an error mid-tool, `ToolEnd` still owed) and flushes it
  collapsed, then commits a red `Role::Error` notice (and records it all in
  `history` so it repaints on resize); clears streaming state. Like `StreamDone`
  it reseats the viewport height first so the box doesn't rise as the strip
  clears.
- On `ToggleToolView` (Ctrl+O / Esc): enter or leave the alternate-screen overlay;
  on leaving, `repaint_conversation` reflows the inline view to catch up.
- On `Notice(text)` (a slash command's output — `/help`): if a reply is mid-flight,
  `flush_streaming_segment` finalises its current segment first (the same ordering
  trick a tool call uses), then `record_system_message` + an `insert_before` commit
  a `Role::System` notice to scrollback.
- On `Clear` (`/clear`): `App::clear_conversation` already wiped the app state
  (history, streaming buffer, running tool, status, queued backlog). Mid-turn
  the loop also kills the backend — `abandon_inflight` cancels + detaches it and
  swaps in a fresh reply channel, the `Interrupt` dance minus the commits — then
  resets the boundary clocks and `repaint_conversation` reflows the now-blank
  inline view (clears the visible conversation; nothing may stream in afterwards
  — `smoke.sh` Phase 16).
- On `Interrupt` (Esc while a turn is in flight — `docs/interrupt.md`):
  `abandon_inflight` cancels the backend, **detaches** its thread (never
  `join()`ing on the loop — a backend parked in a blocking network read would
  freeze the UI for up to one op-timeout: the interrupt-lag bug), and **swaps in
  a fresh reply channel** (so any event the dying thread still sends lands on the
  dropped receiver and can't reach the next turn — this is both the old join's
  "thread stopped" guarantee and the old drain, in one). Then `App::interrupt_turn`
  and commit like `StreamDone` does: reseat the viewport to its idle height,
  flush the kept partial, the cancelled tool (collapsed, red), and the red
  `Conversation interrupted` notice. No `Done for Ns` summary.
- **In the tool-output view** every reply event still updates `App` (so the view
  shows tools live), but the commit-to-scrollback steps above are **skipped** —
  they would write into the alternate screen. The turn-end queue drain still
  runs (its user bubbles only *queue* in `term`, dropped + regenerated from
  history by the return's reflow), so the open transcript follows the next
  queued turn live. The inline view is rebuilt from `history` on return.

### Key types

- `Role { User, Assistant, Error, System }` — drives bullet/colour (errors red,
  system notices cyan).
- `Action { None, Submit(String), PasteImage, ToggleToolView, Notice(String),
  Clear, Copy(Option<String>), RunShell(String), Interrupt, Quit }` — returned
  by `App::on_key` (`PasteImage` sends the loop to the clipboard for a Ctrl+V
  image — `docs/image-paste.md`; `RunShell` carries an idle `!command` to run
  locally — `docs/shell-command.md`).
- `View { Conversation, ToolOutput, ResumePicker }` — which screen is showing
  (Ctrl+O toggles the transcript; `/resume` opens the picker —
  `docs/resume.md`).
- `SlashCommand { name, description, effect }` + `CommandEffect { Clear, Help,
  Copy, Resume, Model, Login, Quit }` + the `COMMANDS` registry (`/help`,
  `/clear`, `/copy`, `/resume`, `/model`, `/login`, `/quit`) — the
  slash-command palette's data; adding a command is one registry entry (+ an
  effect arm).
- `KeyOnboarding { providers, step, selected, query, chosen, key_input }` +
  `KeyStep { Provider, Key }` + `ProviderChoice { id, name, env_var, configured }`
  — the open inline `/login` flow (`App::key_onboarding`, `None` when closed;
  `docs/llm.md`).
- `ResumePicker { sessions, selected, query, cwd, filter, sort, focus }` —
  the open `/resume` picker (`App::resume_picker`, `None` when closed); the
  rows derive on demand (`matches` — filter, query, then the active sort).
  `ResumeFilter { Cwd, All }` / `ResumeSort { Updated, Created }` /
  `ResumeControl { Filter, Sort }` are the toolbar's enums;
  `session::SessionMeta`/`session::SessionSummary` are the file meta and the
  picker-row data (`docs/resume.md`).
- `CommandMenu { selected }` — the open palette's highlight (`App::command_menu`,
  `None` when closed); the matches are derived from the input on demand.
- `Message { role, text, timestamp }` — one finished message (the `timestamp` is
  displayed only for **user** messages, in the Ctrl+O transcript).
- `ToolStatus { Running, Ok, Failed }` — a tool's lifecycle (blue/green/red).
- `ToolCall { name, args, status, output, timestamp, shell, truncated }` — one
  tool invocation; `current_tool` while running, then recorded in history
  (stamped when it finishes). `shell` marks a `!` command's headerless exec
  cell, `truncated` a `!` output cut at the in-memory cap (a dim `…` appended
  in the expanded view) — `docs/shell-command.md`.
- `TokenArrow { Down, Up }` + `TurnStatus { verb, done_verb, tokens, arrow,
  elapsed, thinking }` — the live status of the turn in flight (`App::status`);
  the `Duration`s are written by the boundary each frame (one value drives the
  displayed seconds *and* the verb's shimmer phase). See
  `docs/status-indicator.md`.
- `TurnSummary { verb, secs, timestamp }` — the committed `"{verb} for Ns"` turn
  summary recorded at turn end.
- `HistoryItem { Message(Message), Tool(ToolCall), Summary(TurnSummary) }` — one
  ordered history entry; messages, tools, and per-turn summaries share
  `App::history` so they repaint interleaved in order.
- `QueuedTurn { Messages { texts, images }, Shell(String) }` — one typed entry
  in the mid-turn queue (`App::queued`, a `VecDeque` drained one entry per
  turn-end): an Enter-batched text turn (its Ctrl+V attachments riding along as
  `(placeholder, path)` pairs) dispatched to the model, or a standalone `!`
  command run locally. See `docs/queue.md`.
- `StreamError { partial: Option<String>, tool: Option<ToolCall>, error: String }`
  — what `App::fail_stream` hands the loop to flush after a backend failure (the
  `tool` is one the error killed mid-run, resolved as failed —
  `InterruptedTurn`'s error-path twin).
- `InterruptedTurn { partial: Option<String>, tool: Option<ToolCall> }` — what
  `App::interrupt_turn` hands the loop to flush after an Esc interrupt (the
  `INTERRUPT_NOTICE` const is the committed notice text).
- `StreamEvent { Chunk(String), ToolStart{name,args}, ToolEnd{output,ok,truncated},
  ThinkingStart, ThinkingChunk(String), ThinkingEnd, Error(String), StreamDone }`
  (in `stream.rs`) — what a backend sends to the loop (`ThinkingStart`/`ThinkingEnd`
  drive the live `Thinking for Ns`; the `ThinkingChunk` reasoning deltas between
  them are counted into the token tally, never rendered; `ToolEnd`'s `truncated`
  flags a `!` output cut at the in-memory cap — a backend tool sends `false`).
- `ReplySource` (trait) + `DummyAi` (impl) + `CancelToken` (in `stream.rs`) — the
  pluggable backend seam. `spawn(prompt, images, tx, cancel) -> JoinHandle<()>`
  (`images: Vec<PathBuf>` — the Ctrl+V-pasted image paths, codex's typed
  `LocalImage` channel; the dummy only acknowledges the count); a real
  model is a drop-in `ReplySource` (emit `ToolStart`/`ToolEnd` for tool calls) and
  the loop never changes.

## Testing strategy

- `stream`: `dummy_response` deterministic & non-empty; `chunks` concatenates
  back to the original text and yields >1 chunk for multi-word input; `CancelToken`
  latches and is shared across clones; `DummyAi` delivers all chunks then
  `StreamDone`, and sends nothing when pre-cancelled; a custom `ReplySource` can
  report `Error`.
- `stream` (tools): `turn_events` interleaves ≥1 tool call whose `Chunk`s still
  concatenate to the reply, each `ToolStart` immediately resolved by a `ToolEnd`,
  with both a success and a failure; ends with `StreamDone`; `DummyAi` emits the
  tool calls.
- `stream` (thinking): `turn_events` emits exactly one `ThinkingStart`/`ThinkingEnd`
  pair, before the first tool (so tool start/end stay adjacent), with ≥1
  `ThinkingChunk` strictly inside the pair; `DummyAi` emits them.
- `app` (thinking tokens): `push_thinking` grows the tally pointing `↓` (even
  right after a tool's `↑`) without touching the reply buffer; a no-op when idle.
- `app`: typing appends; backspace; Enter with text → `Submit` + clears input;
  Ctrl+J / Alt+Enter / Shift+Enter insert a newline (box grows) without submitting
  (`docs/shift-enter.md`); Enter
  while empty / while streaming → `None`; Ctrl+C → `Quit` when idle, and Esc
  too but only on an empty composer with no previous user message to edit
  (otherwise idle Esc arms the Esc-Esc backtrack — `docs/backtrack.md` — and
  with a typed draft it is a no-op, codex-style), while
  Esc mid-turn → `Interrupt` (palette-dismiss still wins); Ctrl+C with a
  non-empty input clears the draft instead (closing the palette, leaving a
  streaming turn untouched; from the tool view it still quits), and the next
  Ctrl+C quits;
  `push_chunk`/`finish_stream` transitions; `fail_stream` records partial + error;
  `interrupt_turn` keeps the partial, resolves a running tool as failed, records
  the notice with no summary, and is a no-op when idle.
- `app` (tools & view): `start_tool`/`end_tool` move a tool through running →
  ok/failed and into history; `flush_streaming_segment` records the text before a
  tool and reopens an empty buffer; `finish_stream` records nothing for an empty
  final segment; a turn interleaves text/tool/text in order. Ctrl+O toggles the
  view (even mid-stream, stream keeps running); Esc and `q` close the overlay
  (vs quit in the chat) — except Esc idle with a previous user message, which
  begins the backtrack preview in place (`docs/backtrack.md`); the viewer
  scrolls and ignores typing; Home jumps to
  the top and End back to the bottom (re-engaging tail-follow); it opens pinned
  to the bottom and `settle_tool_scroll` tail-follows (scrolling up disengages,
  reaching the bottom re-engages). With an injected stub clock (`set_clock`), every recorded
  message/tool is stamped with the clock's value; with no clock the stamp is empty.
- `app` (status): `begin_stream` opens a `TurnStatus` (a per-turn verb, 0 tokens,
  `↓`); the verb differs turn-to-turn; `push_chunk` grows the tally (`↓`); a tool
  *adds* its output to the tally and flips the arrow `↑` without resetting, and
  resuming text flips it back `↓`; `set_status_times` writes the boundary
  durations (no-op when idle); `end_turn` records a `Summary` and clears the status;
  `fail_stream` clears it with no summary; `count_tokens` (→ `tokenizer::count`,
  the real `tiktoken` `o200k_base` count) grows with length.
- `app` (slash palette): `command_query` recognises a bare `/token` (rejecting
  past-a-space/newline and mid-line slashes); `matching_commands` prefix-filters
  case-insensitively; the registry has unique lowercase names. Typing `/` opens
  the palette and filters/clamps the selection; ↑/↓ move within bounds; Backspace
  past the slash closes it; Esc dismisses (not quits) and is **sticky** within the
  same token (re-entering command mode reopens it); Enter/Tab run the highlighted
  command, returning the right `Action` (`/clear`→`Clear` + history emptied,
  `/help`→`Notice` listing commands, `/copy`→`Copy` with the last assistant
  text, `/quit`→`Quit`) and consuming the input; an
  empty-match Enter doesn't submit; with no palette open Enter still submits
  normally.
- `app` (input history): ↑ recalls the newest submission (cursor at the end)
  and steps older, clamping at the oldest; ↓ steps newer and clears past the
  newest; ↓ never *enters* history; a typed draft is never clobbered; editing
  a recall (or an interior cursor) returns the arrows to cursor movement, the
  text edges re-enable recall; submitting restarts browsing at the newest;
  adjacent duplicates collapse; blanks are never recorded; Ctrl+C's cleared
  draft is recallable; a recalled `/token` reopens the palette; `/clear`
  keeps the recall history.
- `app` (Ctrl+R search): `InputHistory::search` lists matches newest-first,
  case-insensitively, duplicates collapsed; Ctrl+R opens Idle with no preview;
  typing previews the newest match and Ctrl+R/↑ / Ctrl+S/↓ step with clamping
  at both ends; Backspace pops the query and recovers from a miss; Ctrl+U
  clears back to Idle; a no-match query restores the draft but stays open;
  Enter accepts only a match (no submit/queue; ↑ browsing seated at it; a
  `/token` reopens the palette) and is swallowed otherwise; Esc/Ctrl+C cancel
  restoring text *and* cursor; Esc mid-turn cancels the search, not the turn;
  Ctrl+O cancels and opens the overlay; opening closes the palette and
  previews never reopen it; the highlight ranges cover the query
  case-insensitively and only while a match previews.
- `ui` (Ctrl+R search): `footer_rows` reserves the slot whenever a search is
  open (session info or not); the search line shows the dim prompt, cyan
  query, bold-cyan accept/cancel hint keys on a match, and a red `no match`;
  `cursor_position` tracks the end of the query in the footer row (clamped at
  narrow widths); the previewed match renders the query occurrences reversed
  in the input box; the shortcuts band lists `ctrl+r`.
- `app` (`!` shell): typing `!` first absorbs into the mode (mid-text it's a
  character; an edit creating a leading `!` absorbs too); Backspace/Esc on the
  empty shell composer exit it (Esc never quits from it); the palette and `?`
  band are suppressed in the mode; Ctrl+C records the re-prefixed draft; idle
  Enter → `RunShell(trimmed)` recording `!cmd` (recall re-enters the mode, a
  plain recall clears it, a Ctrl+R search suspends/restores it, accepting a
  `!entry` re-enters it); an empty bang → the help `Notice`, staying in the
  mode; mid-turn Enter queues a standalone `QueuedTurn::Shell` entry (run
  locally at its turn, never merged into a text batch — `docs/queue.md`);
  `begin_shell` records
  the `Role::Shell` header and flags the status + tool; `end_turn` returns no
  summary for a shell turn; an interrupt resolves the command failed.
- `ui` (`!` shell): `message_lines(Role::Shell…)` is the dark user-style line
  with a red `! ` bullet, width-padded; a shell tool renders headerless (`⎿`
  lines only, `⎿ Running…` while running, the `+N lines` hint kept);
  `conversation_lines` keeps the cell flush (no spacer after the Shell header);
  shell mode swaps the composer prompt to a red `! `; `footer_rows` reserves
  the slot in the mode without session info; the footer shows a red
  `Shell mode` displacing `{model} · {cwd}`; the cursor stays in the box (not
  the footer); the shortcuts band lists `!`; `tool_header` still omits the
  `()` for an argless backend tool.
- `app` (shortcuts band): `?` toggles it from an empty composer (shift-modified
  too) and types into a non-empty draft; any other key closes it but still
  acts (typing, ↑ recall, `/` palette); Esc only dismisses — no quit idle, no
  interrupt mid-turn (the turn is untouched); `?` is ignored in the tool view.
- `ui` (shortcuts band): `shortcuts_rows` 0 closed / entries-per-2 open;
  `shortcuts_lines` lists the bindings in two aligned columns (including
  `alt+↑ to edit queue`), keys cyan and
  labels dim, the `esc` entry flipping `to quit`/`to interrupt` with the turn;
  `live_height` grows by the band; `render_live` paints it below the box;
  `cursor_position` stays put when it opens.
- `app` (message queue): Enter mid-turn queues (composer cleared, FIFO order,
  recorded for ↑ recall) while idle Enter still submits — consecutive Enters
  append to one batch, **Tab opens a new follow-up batch** (recorded for ↑ too;
  idle/empty Tab is a no-op); `drain_next_batch` pops the front batch in order
  and empties; Alt+Up (`drain_last_batch`) pulls only the **last** batch into an
  empty composer newline-joined — leaving earlier batches queued, concatenating a
  multi-message last batch — and is a no-op against a draft or an empty queue.
- `ui` (queued messages): `queued_rows` 0 empty / counts the queue / counts
  wrapped lines (incl. the blank divider between batches) / uncapped (the whole
  backlog shows); `queued_lines` separates each turn-batch with a blank row and
  insets
  every row two columns (the indent outside the dark block) and past it styles
  each message exactly like a user message (`❯` bullet, dark background),
  wrapping long ones; `live_height` grows with the queue; `render_live` draws it
  *above* the box, in its own strip slot independent of the shortcuts band below.
- `stream`/`app`/`ui` (session footer): `DummyAi` reports its `model_name`;
  `App.session` defaults unset and `set_session_info` stores the strings;
  `footer_rows` 0 without session info / 1 with it / 0 when a band is open;
  `footer_line` renders `  {model} · {cwd}` all-dim and truncates with `…` at
  narrow widths; `display_cwd` maps home → `~`, under-home → `~/sub`
  (component-wise), else absolute; `live_height` adds the row; `render_live`
  paints it on the last row (idle and mid-stream, never with the palette);
  the cursor stays put when it shows.
- `ui`: `wrap_text` (word wrap, hard-break long words, newlines, width 0, **wide
  & zero-width chars**); `message_lines` (bullet on first line, indented
  continuation; user lines carry a dark background padded to the full display
  width; error lines get a red bullet; system notices a cyan one; **assistant
  replies are markdown-rendered** via `assistant_lines` — fenced code blocks
  verbatim behind a dim gutter and **syntax-highlighted** (`highlight` +
  `code_content_rows`, One Dark palette), ATX headings bold, `docs/markdown.md`); `tool_lines`
  (status-coloured bullet header, collapsed peek + `(ctrl+o to expand)` hint,
  width-truncated); `transcript_lines`/`render_tool_view` (full conversation —
  messages interleaved with each tool's complete output, plus the live tail,
  plus the queued backlog's inset rows — status colour, scroll, wrapped in
  codex's pager chrome: the slash-tiled title row via `tool_view_header`, `~`
  filler, the percentage separator via `tool_view_separator`, the dim hint
  rows; the **user** message's **timestamp right-aligned on its
  own line below it** in a dim colour — no stamp on assistant/tool/summary items,
  and **never** any in the inline `conversation_lines`); the
  **status indicator** — `status_line` formats each phase (`(0s)` with the token
  clause dropped at 0; `↓`/`↑` arrows; `Thinking for Ns` only when set; a
  comet spinner — a white bold head with a fading grey tail between dim walls —
  that steps a frame per interval, reverses at the right wall (the tail
  whipping around behind it), and loops; dim metrics; and a
  per-char bold greyscale-white shimmering verb whose
  crest outshines off-band chars and moves as `elapsed` advances), `summary_lines`
  is one dim bullet-less `"{verb} for
  Ns"` line, `render_live` stacks preview / gap / status / gap in the streaming
  strip, and a committed `Summary` flows through `conversation_lines`/`transcript`
  (stamp-free) like any non-user item; the
  **command palette** — `menu_window` keeps the
  selection visible, `menu_rows` reserves the band (0 closed, capped, 1 for no
  matches), `command_menu_lines` lists the matches in aligned columns and
  **highlights the whole selected row in one cyan colour** (name and description
  alike, vs dimmed grey, no caret; placeholder when empty), and `render_live` draws
  it below the box with the cursor unmoved; the
  growing-input geometry — `live_height` grows a row per wrapped line, adds the
  preview + gap + status + gap strip only while streaming, the palette band
  below the box, and the session footer on the last row, and clamps to the
  screen; `render_live` grows the box, scrolls the input to
  keep the end visible, stacks the streaming preview (or a running tool's blue
  header), a blank gap, the status line, then another blank gap above the box, and
  shows no strip when
  idle; `cursor_position` follows
  the last wrapped row, stays put when the palette opens, and sits on the
  prompt row mid-stream too — the streaming strip and queue above the box are
  part of its layout, so the always-visible cursor never lands on a strip
  row; and
  `repin` keeps the box top-anchored (scrolling up only on overflow, clearing rows
  on shrink); `restore_cursor_row` lands the exit cursor just below the box (no
  blank gap on quit when the box is near the top); the `BULLET_WIDTH` / `repaint_budget`
  single-source-of-truth invariants; and `stable_commit`/`final_commit` proven to
  reconstruct a whole streamed reply with no gaps or duplicates, and to clamp
  safely under a mid-stream resize.
- `session` (`/resume`): every role/tool/summary round-trips through
  `parse_session` (multiline + quoted + unicode text; the summary verb
  restored to its `DONE_VERBS` static, unknown verbs falling back to `Done`);
  malformed, blank, unknown-type, and unknown-role lines skip without failing
  the file; no meta line parses to `None`; `preview_of` finds the first
  user/shell message (flattened, `! ` for shell) and `None` without one;
  `relative_age` matches codex's buckets; `rollout_rel_path` pads and
  dashes the stamp. See `docs/resume.md`.
- `app` (`/resume` picker): the palette runs `/resume` to `OpenResumePicker`
  idle and the red busy notice mid-turn; opening disarms a primed backtrack
  and seats codex's defaults (filter `Cwd`, sort `Updated`, focus `Filter`);
  ↑/↓/PageUp/PageDown/Home/End move with clamping; typing filters
  (case-insensitive) and reseats the selection, Backspace pops, a paste
  joins the query flattened, Esc clears the query first and closes second,
  Ctrl+C closes (never quits), Ctrl+O is inert; the `Cwd` filter hides other
  directories until → toggles `All`; Tab/BackTab swap the toolbar focus and
  ←/→ toggle the focused control (`Created` re-orders by the start stamp),
  reseating the selection; Enter yields `ResumeSession` with the selected
  *filtered* row's path (nothing on an empty list); `load_session` installs
  the history, returns to the conversation, and wipes dead-turn leftovers.
- `ui` (`/resume` picker): the slash-tiled `R E S U M E` title; the search
  placeholder vs the `Search: {query}` echo; the right-aligned Filter/Sort
  toolbar (active values bracketed and following the toggles, compact at
  narrow widths, dropped at the narrowest); dense marker + padded-age +
  preview rows — the age following the active sort key — with the whole
  selected row lit on the full-width `RESUME_SELECTED_BG` tint; the
  `{selected+1}/{total}` count on the bottom rule; both empty states;
  narrow-width truncation; the row window following the selection.

## The custom inline viewport (`term.rs`)

ratatui's `Viewport::Inline(h)` fixes `h` at startup — the field is private and
`Terminal::resize` reuses the stored height, so the live region cannot grow. To
get a **dynamic, content-anchored** live region we drop ratatui's `Terminal` and
own a tiny viewport over a `CrosstermBackend` (`term::InlineViewport`), reusing
the backend's cell→ANSI `draw`, `append_lines` (scroll-up-into-scrollback),
`clear`, and cursor ops:

- `insert_before(lines)` — commit finished lines into real scrollback above the
  viewport. A direct port of ratatui's portable (no-`scrolling-regions`)
  `insert_before` scroll math: it pushes the viewport *down* while there's room
  and scrolls only once the screen is full, including the tmux-safe "don't
  full-clear then scroll" ordering.
- `draw(height, render, cursor)` — repaint the live region at its new `height`,
  keeping its **top anchored** so it grows *downward* in place. It scrolls the
  screen up (via `append_lines`, oldest chat into scrollback) only when the box
  would overflow the bottom, and blanks the rows a shrink vacates just below it.
  The decision (`scroll_up` / new `top` / `clear_below`) comes from the pure
  `ui::repin` helper; the cursor is placed from the final viewport. The region
  buffer is kept in `prev` and, when the geometry didn't move (no scroll, no
  vacated rows, same rect), only the cells that **changed** since the last draw are
  emitted (`Buffer::diff`) — so a keystroke ships a couple of cells (~35 bytes),
  not the whole region (~700 bytes). `prev` is invalidated (full repaint next) by
  anything that moves the screen under it: `insert_before`, `reflow`, the overlay,
  a resize. This is what keeps typing crisp on latency-bound terminals. The whole
  frame (prepare + cells + cursor, factored into `paint_frame`) is bracketed in a
  **synchronized update** (`BeginSynchronizedUpdate`/`EndSynchronizedUpdate`, DEC
  mode 2026), so the terminal swaps it in atomically and a fast keystroke burst
  never shows a half-painted frame or the cursor mid-flight — the same trick codex
  wraps its draws in (terminals without 2026 ignore the markers). `draw_overlay`
  brackets its full-screen paint the same way.
- `set_view_height(height)` — reseat the tracked viewport height *without*
  redrawing. `insert_before` reserves `view.height` rows *below* the lines it
  commits (to keep the box on screen), and the streaming strip (preview + gap +
  status + gap) inflates that height while a reply streams. So at `StreamDone`/`Error`
  `main` calls this first to drop the strip's rows, letting the final commit (the
  reply's last line, then the `Done for Ns` summary) replace them in place —
  otherwise `insert_before` over-scrolls and the box rises off the bottom (see
  *Known limitations*).
- `enter_overlay` / `draw_overlay` / `exit_overlay` — the Ctrl+O tool-output view.
  `enter_overlay` switches to the terminal's **alternate screen** (so the inline
  conversation — main screen + its real scrollback — is preserved untouched);
  `draw_overlay` paints a full-screen buffer (`ui::render_tool_view`) every frame;
  `exit_overlay` switches back, after which `main` reflows the inline view to catch
  up (on a normal return *and* on a quit-from-overlay, so the restored screen is
  rebuilt either way). This is the *only* use of the alternate screen — the
  conversation itself stays inline.
- `reflow` rebuilds the inline view from a re-wrapped `tail` (after a resize, a
  Ctrl+O return, or `/clear`). It lets `insert_before` **overwrite the screen in
  place** (draw top-down, then clear the rows below the tail) and `clear_region(All)`s
  *only* for an empty tail. A leading full clear before `insert_before`'s scroll
  makes tmux spill the on-screen frame into scrollback; after a Ctrl+O return that
  frame is the **stale streaming strip**, so the clear would push `Working… (…
  tokens)` into scrollback above the rebuilt conversation (`smoke.sh` Phase 7).

`term.rs` is, like `main.rs`, an I/O boundary verified via `scripts/smoke.sh`
rather than unit tests; all the geometry it consumes is pure and tested in `ui`.

## Known limitations (v1 — iterate later)

- The box is content-anchored, so when it grows past the screen bottom the chat
  scrolls into the terminal's real scrollback; shrinking it again can't pull that
  chat back (terminals can't reverse-scroll their own scrollback), so after a
  grow-past-bottom-then-shrink the box stays where it scrolled to with blank rows
  below. Normal short messages never hit this.
- **Streaming strip collapse.** The streaming strip (preview + gap + status + gap)
  is drawn *above* the box, so it grows the live region *upward*. When a reply finishes,
  that strip's rows are handed back to scrollback as the committed final line +
  spacer + the `Done for Ns` summary, and the box must stay put. The fix is to reseat the viewport to its idle height
  (`term.set_view_height`) *before* the final `insert_before`, so the commit reserves
  only the idle box below it; without it the strip-still-counted height makes
  `insert_before` over-scroll and the box rises off the bottom, leaving blank rows
  beneath. Covered by `scripts/smoke.sh` Phase 5 (a short terminal where one
  exchange overflows the screen, asserting no blank rows below the settled box).
- On resize the on-screen chat is repainted (any dimension — a width change
  re-wraps, a height change reseats the box; `smoke.sh` Phase 17), but lines
  already in the terminal's own scrollback keep their original wrapping (so
  after resizing a long chat, boundary messages can appear twice — once
  old-width above, once new-width below; codex shares this edge — terminal
  scrollback can't be rewritten, only re-emitted below). Guarded against panics
  at every size by `ui::tests::render_pipeline_survives_extreme_terminal_sizes`.
- Resizing *mid-stream* recovers by re-committing the in-progress reply, but the
  partial reply's already-committed lines are absent from the rebuilt screen until
  the next chunk re-commits them (the reflow frame itself is atomic — the box never
  flashes; the reply text just lands a beat later).
- Returning from the tool-output view repaints the inline conversation from
  `history` (the same path as a resize), so it shares the same edge: anything that
  had already scrolled into the terminal's own scrollback before the overlay
  opened keeps its old position, and a reply segment that was *partially* committed
  when the overlay opened is re-committed on return a chunk later (no data loss).
- Tool output shown inline is always collapsed to a one-line peek; the only way to
  read it in full is the Ctrl+O conversation view, which shows the whole transcript
  with every tool expanded (by design — keeps the inline chat compact).
- The slash-command palette only matches a **bare** `/token` (a leading slash, no
  whitespace); there's no argument parsing yet. The registry is intentionally small
  for now (`/help`, `/clear`, `/copy`, `/resume`, `/quit`) — adding a command is a one-line `COMMANDS`
  entry plus an effect arm in `run_selected_command`. Esc's dismissal reopens on
  the next keystroke only if you leave and re-enter command mode; and running
  `/help` mid-stream finalises the reply's current segment first (so the notice
  never splits the reply), the same ordering rule tool calls use.
- Timestamps are shown **only** in the Ctrl+O transcript, and only for the
  **user's** messages (12-hour `hh:mm AM/PM`, no seconds, right-aligned below the
  message); assistant/tool/summary items record a stamp but never display it. The
  inline conversation has none, and there is no per-token/relative time. See
  `docs/timestamps.md`.
- The status indicator's token counts are an app-side **estimate** (≈ chars/4), not
  real model usage — the dummy has no tokenizer; a real `ReplySource` could report
  exact counts later. The working/done verbs cycle deterministically (a turn
  counter), not at random. See `docs/status-indicator.md`.
- `!` shell commands (`docs/shell-command.md`) run under `sh -c` with **no
  sandbox** and no timeout (codex caps at 1 hour); stdout and stderr are
  concatenated, not interleaved; the interrupt notice is the shared
  `Conversation interrupted…` text. (Mid-turn `!command`s are no longer a
  limitation: they queue as standalone `QueuedTurn::Shell` entries and run
  locally when their turn comes — codex parity, `docs/queue.md`.)
- The `@` file picker (`docs/file-search.md`) walks the cwd **once per worker
  lifetime** (cached on first use), so files created mid-session don't appear
  until restart; the walk is **dependency-free** — it skips hidden entries and a
  small denylist (`target`, `node_modules`) but does **not** parse `.gitignore`
  (codex uses the `ignore` crate + nucleo). Fuzzy ranking is a hand-rolled
  subsequence scorer, not nucleo. The selected path is inserted as plain text
  (no tool/skill mentions, no image attachment, no on-the-wire encoding). Moving
  the cursor out of an `@token` leaves the band open until the next edit (it
  re-derives on edits only, like the palette).
- `/resume` (`docs/resume.md`) has no pagination, sort/filter toolbar, density
  toggle, or per-row transcript preview (the scan is capped and loaded whole
  at open, always mtime-descending, always dense rows); no `/resume <id>`
  args or CLI subcommands; no cwd prompt on a cross-directory resume; and
  resuming the *same* saved session from two instances interleaves appends
  unguarded (each new session's file is unique per pid, but there is no
  codex-style state-db arbitration).
- No markdown rendering or scrollback nav keys (YAGNI).
