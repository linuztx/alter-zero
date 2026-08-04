# Esc Esc — edit a previous message (backtrack)

A port of codex's **backtrack** feature (`tui/src/app_backtrack.rs`): pressing
**Esc** twice from an idle, empty composer steps back to a previous user
message, shows it highlighted in the transcript overlay, and — on Enter —
rewinds the conversation to just before that message and puts its text back in
the composer to edit and resend.

## The gesture

```
idle, empty composer, a previous user message exists:

  Esc            → primed: the footer slot shows "esc again to edit previous message"
  Esc (again)    → the Ctrl+O transcript overlay opens as a *preview*, the last
                   user message highlighted (reversed video)
  Esc / ←        → step the highlight to the next-older user message (stops at the oldest)
  →              → step back toward the newest (stops at the newest)
  Enter          → confirm: drop the highlighted message and everything after it
                   from the conversation, close the overlay, repaint the truncated
                   conversation, and prefill the composer with the message's text
  q / Ctrl+O     → cancel: close the overlay, conversation untouched
```

Priming is codex-faithful: **no timeout** (it persists until the second Esc or
any other key), and **any other key un-primes** — the next keypress just does
its normal job. The overlay preview can also be entered from an already-open
Ctrl+O view: pressing Esc there (idle, with a target) begins the preview in
place instead of closing the view — codex's Ctrl+T → Esc path. `q` and Ctrl+O
still close the overlay from the preview (that cancels it), and every scroll
key (↑/↓, PgUp/PgDn, Home/End) keeps working while previewing.

## What Esc means now (the precedence ladder)

Esc keeps every existing job, in the existing order — backtrack slots in
between "interrupt" and "quit":

1. an open `?` shortcuts band / slash palette / `@` file picker → dismiss it
2. empty `!` shell-mode composer → exit shell mode
3. a turn in flight → interrupt it (`docs/interrupt.md`)
4. **primed → open the backtrack preview; unprimed, empty composer with a
   previous user message → prime**
5. a typed draft → nothing (codex's composer only acts on Esc when empty;
   Ctrl+C is the composer-clear)
6. otherwise — empty composer, nothing to backtrack to → quit

The one deliberate divergence from codex: codex never quits on Esc, while our
idle Esc historically did. Esc still quits **on an empty composer with
nothing to backtrack to** — a fresh session, or right after `/clear` — so the
startup-screen behavior survives; once a user message exists, idle Esc means
"edit previous message", with a typed draft it does nothing, and quitting is
**Ctrl+C** (empty composer) or `/quit`, both long documented. The `?` shortcuts band reflects whichever is
true (`esc to quit` ↔ `esc esc to edit previous`).

## State

`App::backtrack` (a `Backtrack` struct, all pure state):

- `primed: bool` — the first Esc armed the gesture (codex's `primed`).
- `selected: Option<usize>` — `Some(i)` while the overlay preview is active:
  an index into the conversation's **user messages** (`Role::User` history
  messages, oldest = 0 — codex's `nth_user_message`). `Role::Shell` exec
  headers are user-*typed* but not user *prompts*; like codex, stepping skips
  them (they still get dropped if a truncation point precedes them).
- `scroll_pending: bool` — set when the preview opens or steps; the next
  overlay draw scrolls the highlight into view (codex's
  `scroll_chunk_into_view`), then clears it, so manual scrolling isn't fought
  with afterwards.

`toggle_tool_view` (any Ctrl+O / q / overlay-Esc close) and
`clear_conversation` (`/clear`) reset the whole struct — codex resets its
`BacktrackState` on overlay close and on Ctrl+L clear the same way.

## Confirm

`App::confirm_backtrack` (Enter while previewing):

1. finds the selected user message's position in `history` and **truncates
   the history there** — the selected message and everything after it (later
   replies, tool calls, summaries, shell cells) are dropped; everything
   before stays (codex's `trim_transcript_cells_to_nth_user`);
2. resets the backtrack state and returns to `View::Conversation`;
3. prefills the composer with the message's text via the same path as a ↑
   history recall (cursor at the end, palette/shell-mode re-derived).

The key arm returns `Action::ConfirmBacktrack` (a dedicated action, not
`ToggleToolView` — the loop also resets the code to that point's checkpoint,
`docs/checkpoint.md`), then runs `exit_overlay` + a **`ReflowClear::Purge`**
`repaint_conversation`, which rebuilds the inline view **from the truncated
history** (invariant 3's repaint — truncation is just a shorter tail; an empty
result repaints like `/clear`). No scrollback commits happen while the overlay
is up (invariant 4), so there is nothing stale to retract — the repaint *is*
the rewind. The submitted-input ↑ history is untouched (codex's cross-session
input history likewise survives a fork).

**Why Purge, not the in-place overlay-return repaint.** Backtrack *shrinks*
history, so — like `/resume` (which replaces it) and every resize — the repaint
must **purge scrollback and clear the whole screen** before rebuilding
(invariant 3). An in-place overwrite only rewrites the visible rows: it left the
dropped exchange lingering in the terminal's own scrollback (and still *on
screen* when the conversation had overflowed), clearing only on the next resize
— the "it's still there after I rewind" duplication bug. The purge makes the
rewound conversation the whole record, screen and scrollback alike. (This is the
one deliberate divergence from codex, whose post-fork scrollback keeps the
dropped cells; the duplication it caused here wasn't worth the parity.)

There is no backend session to fork: a `ReplySource` gets one prompt per turn
(`docs/design.md`), so truncating `App::history` *is* the whole rollback —
codex's `Op::ThreadRollback` round-trip has no equivalent here. A real
stateful backend would hook its own rewind into the same confirm.

Prefill restores attachments too (updated with `docs/context.md`): a
`[Pasted Content N chars]` placeholder was already expanded at submit time so
the full text comes back, and `[Image #N]` placeholders come back **backed** —
the message records its temp-PNG paths now, so the confirm re-keys them to the
restored placeholders (the interrupt-undo dance: any pairs backing a clobbered
draft are discarded first, and the dropped *later* user messages' orphaned
attachments are queued for temp-file deletion), codex's retained-local-images
re-attach.

## Rendering

- **Highlight** — `ui::transcript_lines` renders the selected user message's
  rows with `Modifier::REVERSED` patched over the normal dark user style
  (codex's `user_message_style().reversed()`); its timestamp line stays
  normal. The selection's line range comes from the same single walk
  (`transcript_selection`), so the highlight and the scroll target can't
  drift apart.
- **Scroll-into-view** — the pure `ui::backtrack_scroll` decides the new
  `tool_scroll` (only when `scroll_pending`): scroll up just enough to show
  the highlight's top, or down just enough to show its bottom, else stay put.
  `tui::view::Session::draw_tool_view` applies it before the settle/clamp.
- **Footer hint** — while primed, the footer slot (the search-line / shell-hint
  slot, `docs/footer.md`) shows `esc again to edit previous message` — the
  `esc` key cyan-bold like the search hints, the label dim (codex's
  `esc_backtrack_hint` footer). It takes the slot over the session footer and
  shows even with no session info injected, like the search line.
- **Overlay hints** — while previewing, the overlay's second key-hint row
  swaps to the backtrack hints (`esc/← to edit prev · → to edit next ·
  enter to edit message · q to cancel`), codex's highlighted-pager footer.
- **Shortcuts band** — the `esc` entry is now three-way context-sensitive:
  `esc to interrupt` while a turn runs (as before), `esc esc to edit
  previous` when idle with a target, `esc to quit` when idle without one.

All the new styling lives in the `ui/theme.rs` consts block (`BACKTRACK_*`,
`TOOL_VIEW_HINT_BACKTRACK`), per the conventions.

## Edge cases

- **Non-empty composer**: Esc never primes (codex requires
  `composer_is_empty`) — and never quits either: with a typed draft idle Esc
  is a no-op, codex-style, so it can't throw typed work away.
- **Mid-turn**: `turn_active()` Esc still interrupts — priming requires idle
  (codex's `!is_task_running`). After the interrupt lands, the next Esc
  primes: Esc-Esc-Esc from a streaming turn is interrupt → prime → preview,
  exactly codex's chain. A queued backlog implies an active turn, so a
  preview can never coexist with pending dispatches.
- **No user messages** (fresh, `/clear`-ed, or errors/system notices only):
  nothing to target — Esc quits as it always did, and an overlay Esc just
  closes the overlay (codex posts "No previous message to edit."; we simply
  don't enter the mode).
- **`/clear` and Ctrl+O reset**: both wipe the backtrack state; nothing
  survives a view toggle except via the explicit Esc re-entry.
- **Oldest/newest bounds**: stepping saturates at both ends (codex clamps the
  same way).

## Tests

Unit tests live beside the code they lock (`app::tests`, `ui::tests`):
priming/unpriming and the precedence ladder, preview open/step/clamp,
confirm's truncation + prefill + view flip, cancel paths (`q`, Ctrl+O),
`/clear` reset, overlay-Esc entry, the reversed highlight + selection range,
the scroll-into-view decision, the footer hint line and its `footer_rows`
slot, and the three-way shortcuts entry. `scripts/smoke.sh`'s backtrack phase
drives the real binary end-to-end: two exchanges, Esc-Esc, a step older,
Enter, then asserts the composer holds the first message, the second exchange
left the repainted screen, and a resubmitted turn still streams to "Done".
