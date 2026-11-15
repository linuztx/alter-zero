# Finding your way in the Ctrl+O transcript: pinned headers and entry steps

The Ctrl+O transcript shows the whole conversation with every tool call
expanded, so a single `cargo test`, a `Read` of a long file or a `! seq 1 500`
is hundreds of rows. Scrolled into the middle of one, nothing on screen said
which command printed it: the `● Bash(…)` header was a screenful or more
above. To see the command you scrolled up, then scrolled back to find your
place.

The Ctrl+D context view had already solved this for the raw context window
(`docs/context.md` *Finding your way*): the entry the window opens inside is
pinned over the top row, and ←/→ step between entries. The transcript pager
now does the same:

```
/ T R A N S C R I P T / / / / / / / / / / / / / / / / / / / / / / / / / / /
● Bash(cargo test --lib -- app::tests::login term:: --skip pty:: --skip      ← pinned
       background::tests::)
     test app::tests::login::device_code_lands_on_the_page ... ok
     test app::tests::login::the_key_step_names_the_provider ... ok
     …
────────────────────────────────────────────────────────────────────── 46% ─
 ↑/↓ to scroll   ←/→ prev/next entry   pgup/pgdn to page   home/end to jump
 q/esc/ctrl+o to quit
```

1. **The header of the cell the window opens inside is pinned over the
   body's top rows** while its own header is scrolled off: a tool call's
   `● name(args)`, a `!` command's dark `! cmd` row over its output, an
   agent's `● Agent(description) · id`, a thought's `Thought for 1m 5s`, a
   hook-run cell's `● Ran 2 PreToolUse hooks`, a compaction's
   `● Context compacted`. While a running command streams under a
   tail-following Ctrl+O, the pin names the running command.
2. **←/→ step between entries**, opening the previous or next one at the
   window's top — a message, a tool call, a thought, one agent of a group.
3. **Inside an agent session view the title names the agent**:
   `T R A N S C R I P T · explore agent · Fetch the weather`, Ctrl+D's own
   naming (`view_title`, now shared by both overlays), since nothing else on
   the page tells the lead's transcript from an agent's.

## Entries

A built transcript records its **entries** beside its rows
(`ui::TranscriptEntry`, `ui::TranscriptPage`): for each cell, the row it opens
on (`start`), the first row past its header (`body`, equal to `start` for a
cell with no header) and one past its last row (`end`, its trailing blank not
included). ←/→ step between the starts; the pin reads the header rows
`start..body`.

Each cell builder reports how many of its rows are its header —
`tool::tool_full_cell` (the `● name(args)` rows however the command wraps, an
answered question's one headline row, none for a `!` command's output),
`hook_cell::hook_report_full_cell`, `file_cell::draft_cell` (a `write` still
streaming its file) — so the count comes from the code that drew the rows
rather than from a second guess at the same wrap. A thought, a compaction, a
hook note and each agent have one-row headers. Messages and notices have none
and never pin: a reply's first row is content, and pinned it would read as a
line printed twice.

What is **not** an entry: the startup banner, the empty transcript's
placeholder, and a turn's receipt (`Done for 3s`) — a fact about the turn,
not a cell to step to, so → from a turn's last cell goes on to the next
prompt.

**A `!` command is one entry across two history items.** Its dark `! cmd`
header is a message of its own, and its `⎿` output, flush under it, is the
next item — or, while it runs, the live tail. `EntryWalk` holds the header
back until it sees what follows: the output joins it, its header the command;
anything else leaves the header an entry of its own. The walk is the one walk
the cache's frozen prefix, its live tail and an agent's page all go through,
so the three cannot disagree. The frozen walk's state — a header held at the
end of the prefix for output that is still the tail — lives in the cache, and
the tail walks on from a copy, so every refresh sees it again.

## The pin

`transcript::pinned_rows(entries, scroll, rows)` is the whole rule, pure:

- The pin is the entry's **own header rows**, its first ones: at most
  `TRANSCRIPT_PIN_MAX_ROWS` (2, the inline cell's ceiling,
  `TOOL_HEADER_MAX_ROWS`), and never more than leaves two rows of the window
  for what it names. A header that goes on past the pin is **cut**, and its
  last pinned row ends on `…` — right after the text, so a padded `! cmd` row
  keeps its width.
- Nothing while the window starts on the header's first row (it is already on
  top), nor while the header's own later rows would still show under the pin
  — a tall header printed twice with a gap in it. A five-row script header
  pins once only two of its rows are left on screen, and the pin takes their
  place.
- Fewer rows as the entry's last row comes up, and none once a pinned row
  would cover it: past that, the pin would stack on the next cell's header.
- None in a window shorter than `CONTEXT_PIN_MIN_ROWS` (3), where the pin
  would cost the text it names.

For a one-row header this is exactly Ctrl+D's `pinned_header`. The two-row
ceiling is the difference: a wrapped command pins as the two rows the
conversation showed it in, and the second row is usually the part that tells
two similar commands apart (`cargo test --lib -- app::tests::login` and
`… term::`).

The pin is applied **at paint time** over the window's rows
(`TranscriptCache::render`, `render_tool_view`), never written into the page:
`/export` and every other reader of the transcript's rows see the page as it
is. It costs no storage — it is a copy of rows the page already holds — and a
binary search over the entries per draw. A still page paints the same buffer
every frame, so the overlay's diffed paint still writes nothing
(`docs/overlay-repaint.md`). A pin over a picture's reserved rows hides only
those rows: the stamp slices the block from the first carrier it finds
(`docs/images.md`).

## ←/→

The key records only the direction (`App::tool_jump`, an `EntryJump`), since
where the neighbouring entry starts is a fact about the transcript as built —
its width, its wrapping. The draw resolves it against the entries of the very
build it is about to paint (`App::resolve_tool_jump`, before the scroll
settle), by the rule Ctrl+D steps with (`app::views::step_target`, now shared):
← opens the start of the entry being read, or the one before it when the
window already starts there; → the next one; past the first, the top, and
past the last, the end, which re-engages tail-follow. A close drops a step
still pending.

**The backtrack preview keeps ←/→.** While Esc-Esc highlights a user message,
← and → step the highlight (`docs/backtrack.md`) and queue no entry step; the
first hint row then names only the scroll keys
(`TOOL_VIEW_HINT_SCROLL_KEYS`), since the row under it says what ← and → do
there.

## Considered and left out

- **A one-row pin**, Ctrl+D's. Rejected for headers that wrap, for the reason
  above.
- **A separately rendered short header per cell** (the inline cell's
  collapsed form). Rejected: the first rows of the cell's own header carry the
  same text, and a copy of the page's rows costs nothing to keep and cannot
  disagree with the cell when the width or the theme changes.
- **A count on the bottom rule** like Ctrl+D's `5 messages · ≈1.2k tokens`.
  Left out: Ctrl+D's count is the size of the request, which is the question
  that page answers. A count of transcript entries answers no question the
  percentage beside it does not.

## What guards it

- `ui::tests::transcript_navigation` — the entries each kind of cell records,
  the `!` command's two items as one entry (committed and running), the cache's
  entries against a from-scratch build at every step, the pin rule's every
  boundary, the pinned rows on a painted pager (a tool, a wrapped header, a
  cut one, a `!` command, a running command followed live), the cached pager
  painting exactly what a fresh page paints, the hint rows, and an agent's
  title and pins.
- `app::tests::views` — the keys, the preview's own ←/→, and the step's
  resolution at both ends.
- `scripts/smoke.sh` Phase 157 — two long `!` commands on the real binary:
  Ctrl+O opens with the second one's header pinned over its tail, ← opens each
  command at its own first row, ↓ scrolls the output under the pinned header,
  → steps on.
