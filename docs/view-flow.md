# View flow — long inline-view content flows into scrollback

## The problem

The composer-replacing inline views draw a **content-driven framed body**: the
builder (`ui::mcp_view_lines`, `ui::hooks_view_lines`, `ui::trust_view_lines`,
`ui::background_view_lines`) produces the whole page as lines — top rule,
title, content, hint, bottom rule — and the height helpers reserve
`lines.len()` rows, clamped to the terminal (`ui::layout::view_height`). The
paint was a plain top-aligned `Paragraph`, so a body taller than the terminal
silently **clipped at the bottom**: an `/mcp` tool-detail page with a long
description lost its parameter tail, its `Esc to go back` hint, and its bottom
rule — the frame just ran off the screen with no sign there was more.

An inline region cannot scroll internally the way a full-screen pager can — and
it shouldn't: this TUI's whole design is that finished content lives in the
**terminal's real scrollback**, where the terminal's own scrolling reads it
(the conversation streams there; the permission prompt displaces chat there
while it asks, `docs/permissions.md`). So the fix presents the view's content
the same way the rest of the TUI presents everything: as text.

## The design

Two rules, both pure policy in `ui`, acted on by the boundary:

1. **A framed view body is bottom-anchored.** When the built lines outnumber
   the rows the region gives them, the paint shows the **last** rows —
   `ui::view_body_skip` decides how many top rows to skip — so the interactive
   tail (action rows, the hint, the bottom rule) is always on screen. The
   hidden-cursor seat (`menu_marker_seat`) subtracts the same skip, so the `❯`
   the cursor parks on is the one actually painted. A page with no painted
   `❯` — a detail view, an unconfigured `/mcp` list, a highlight the anchor
   scrolled off — seats just past its last text row, the closing hint (the
   overlays' `overlay_cursor_seat` rule; a framing rule is chrome and is
   skipped), and only a region too short to paint even the hint keeps the
   far corner.

2. **The skipped top flows into real scrollback.** When the page exceeds the
   rows its painted tail gets — the whole terminal for a framed view (the body
   is bottom-pinned by `view_split`, which squeezes the strip first), the
   strip's own slice for the strip — the hidden top rows are **committed above
   the live region**, exactly where they visually belong: scrollback ends with the
   page's top, the screen shows the page's tail, and the terminal's own
   scrolling reads the whole thing as one piece. `ui::view_flow` is the pure
   decision — which rows flow, and a **signature** identifying the flowed
   state (the flowed rows' text at this width/count, or — for a page that
   ticks — a stable key naming it; see below).

What the signature is computed from is the one per-view choice
(`ui::view_flow`'s `FlowSign`). A page that changes only on a **keystroke**
signs its **rows** — any edit to them re-signs, and the rebuild re-flows the
new ones. That is every page but the two that move on their own: the
**streaming strip** (`docs/strip-flow.md`) and the ↓ manager's **details** page
live-tails a running shell, so its runtime, its output box and its `Showing N
lines` caption all move between keystrokes, at the 32 ms cadence an open band
keeps running (`App::wants_animation_frames`) — the strip moves at the same
cadence for the same reason. Signing such rows would purge-rebuild the whole
screen thirty times a second, so each signs **what it is of**
(`FlowSign::Frozen`) — the shell the page describes, the call the strip shows —
and the flowed top **freezes**
where it was committed — a resize, a walk back to the list or a different
shell still re-signs it, because the row count and the width are hashed in
both regimes. Frozen text is what scrollback holds for everything else on the
screen, and it is what the reported bug asked for: *"if it's not shown in the
terminal window just freeze it instead of removing the detail."*

The flowed rows are not history, so nothing may be left behind when they go
stale. The boundary (`tui::view`) keeps the signature of what it last flowed
(`Session::flowed_view`) and, on any mismatch — the page navigated, the search
narrowed, the menu closed, the terminal resized — answers with the standard
**purge rebuild** (`repaint_conversation` / `repaint_agent_view`): scrollback
is rebuilt from history with the *current* flow rows appended to the tail, or
with none when the view closed — the composer simply returns, and no stale
view text survives anywhere (the `/clear` machinery; `term::reflow`). While a
flow is active, scrollback **commits pause** (`Session::commits_allowed`, the
agent-session-view pattern): a cell committed between the flowed rows and the
region would tear the framed content in half, so it waits in history and the
flow-exit rebuild regenerates it. The in-flight partial reply survives the
same way it survives every purge: it lives in the streaming buffer, and the
first un-flowed rebuild re-commits it (`docs/flicker.md`).

The flow's tail contribution is capped like every rebuild write
(`RESIZE_REFLOW_MAX_ROWS`, keeping the rows nearest the region) so a
pathological page can't turn one navigation into an unbounded write.

## A page that sheds rows before it flows: the compact pages

Bottom-anchoring keeps a page's *tail* on screen, which is the right thing
for a page whose choices sit at the bottom — the permission prompt's options,
`/trust`'s two rows. A picker is the other shape: its `❯` search line and the
highlighted row sit near the **top**, under the page's header, and its
description or preview sits below the list. Anchored on a short terminal,
such a page kept the description and the hint and flowed the search line and
the top of the list into scrollback — the row Enter would act on was
reachable only by scrolling the terminal up.

So a picker's builder takes the rows it may have and builds the **first page
that fits** them (`model_view::first_page_that_fits`), trying the most
complete first, and what yields is what the choices don't need, in codex's
order (`selection_picker_layout.rs`: *"optional gaps and previews yield
first; headers, tabs, search, and at least one result remain visible"*):

1. a long description is cut to one row (`Description::OneRow`), where the
   page has one — a wrapped description moves the page's height with the
   selection, and a page deciding on that height alone flipped between its
   whole and its compact layout as ↑/↓ crossed a long one;
2. the description, a preview and the `(n/total)` counter go, and the list
   becomes a `COMPACT_LIST_ROWS` (3) window between two reserved overflow
   slots (`model_compact_list`): codex's arrow rows, the `↑ N more above`
   directly under the search line and the `↓ N more below` directly over the
   hint, each blank when nothing is hidden that way, so the frame is one
   height at every selection and a blank slot is the gap the page wants
   there anyway;
3. a header goes last.

Only a page shorter than even the last of them bottom-anchors and flows the
way every framed page does. The pages must shrink in that order, because the
height comes from two places: the reserved height and the flow pass the
terminal's, while the paint and the cursor seat pass the region already
reserved for the page — at least the chosen page's height and at most the
terminal's — and a builder whose pages shrink in order picks the same page
from either. A page with nothing to choose from (a search matching nothing,
an empty catalog) has no window to compact, so only its header yields.

The compact pages so far: `/theme` (`docs/theme.md`), `/skills`
(`docs/skills.md`), `/settings` (`docs/settings.md`), `/mascot`
(`docs/mascot.md`), `/spinner` (`docs/spinner.md`), `/model` and `/login`'s
lists (`docs/llm.md`), the `/secrets` list (`docs/secrets.md`), and
`/donate`, whose list is of boxes rather than rows, so it collapses before
it windows — an accordion first, the addresses the highlight is not on
folded to their label rows while the highlighted one keeps its box, then a
window one box tall (`docs/donate.md`). `smoke.sh` Phase 138 opens them on a
12-row terminal.

## Scope — which views flow

- **Flow + bottom anchor**: every content-driven framed view — the browsing
  menus (`/mcp`, `/hooks`, `/trust`, `/donate`, `/export`, `/secrets`) *and* the windowed pickers (`/model`,
  `/login`, `/settings`, `/skills`), all of them line builders now
  (`*_view_lines` / `key_onboarding_lines`, their heights the built line
  count). Their content is **static per state**: it changes only on a user
  keystroke, so the frozen rows in scrollback can never go stale between
  signature checks. Eligibility mirrors `render_live_with_preview`'s
  precedence: a view flows only while it is the one actually painted (an ask
  or permission modal covers everything below it).

  The pickers' one wrinkle: their `❯` search line sits in the page **top**,
  so on a terminal shorter than their page it is part of the flow. That
  stays correct because a keystroke changes the flowed rows and re-signs the
  flow — the purge rebuild re-flows the typed query with the page — while
  the hardware cursor, which cannot sit on a scrollback row, clamps at the
  body top (`anchored_view_row`, the seat's skip subtraction). Their pages
  are bounded by construction (windowed lists, ~15–18 rows), so this regime
  only exists on terminals shorter than that.
- **Flow + bottom anchor, with a context rule**: the tool-permission prompt
  (`docs/permissions.md`). Its body — the numbered write/diff, the command —
  shows **whole** now (the retired cap hid the middle behind a `… +N lines`
  tail; the tail survives only past the `PERMISSION_BODY_MAX_ROWS` safety
  ceiling, since the page is rebuilt and highlighted every draw tick). The
  tail block — question, options, hints — closes the page, so the anchor
  keeps it on screen; ↑/↓ restyle rows inside the painted tail, so the
  flowed top holds its signature. The **context cells** above the frame ride
  the flow like everything else, on one condition — they must be *static*
  (`context_is_stable`): a queued `⎿ Waiting…` cell is, since the approve
  seam runs before its `ToolStart` and nothing can change while the answer
  is pending, so it flows into scrollback whole and uncollapsed (there is no
  screenful left to compete for, so a `… +N more waiting` summary would hide
  siblings for nothing). A **live agent group** or a **running call** is not
  — the tree's bullet blinks at the frame pulse and its counters advance,
  a running call's streamed output grows its peek — and a flowed row is
  frozen in scrollback, so ticking content would go stale there or re-sign
  the flow into a purge rebuild per tick; that context gives way and only
  the static frame flows. Dropping the static cell instead was the reported
  regression: it left the prompt a box out of nowhere, with the cell in no
  buffer at all. The builder stays a fixpoint at the height the region
  actually gets — building the flowed context without the budget is what
  keeps the page independent of `term_height` there — so
  `permission_height` and `render_permission` can never disagree.
- **Flow (frozen) + bottom anchor**: the ↓ background manager
  (`docs/background.md`). Anchoring alone was the reported bug: on a terminal
  a couple of rows shorter than the details page, the top rule, the `Shell
  details` title and the status/runtime fields were skipped into **no buffer
  at all**, so scrolling the terminal up ran the conversation straight into a
  headless box. They flow now like every other page — the difference is only
  what the flow is signed on. Its **list** page changes on a keystroke, so it
  signs its rows like the menus. Its **details** page ticks, so it signs the
  shell's id and the flowed rows freeze (above). A runtime that flowed reads
  the value it had when the page was committed until something re-signs the
  flow — stale, where it used to be *nowhere* — while the page still on
  screen keeps tailing, unchanged, exactly as it was. Which rows those are is
  a matter of how short the terminal is: the page is 26 rows, so 24 rows
  flows the top rule alone and 18 flows the whole field block; the output box
  only starts flowing below 15. A details view whose shell has gone renders
  the *list* (`background_view_lines`' defensive fallback), so it signs like
  one.
- **Flow (frozen) + bottom anchor**: the **streaming strip** itself
  (`docs/strip-flow.md`) — the running tool cell, the `● Thinking…` block, the
  live agent tree, the spinner status line, the queued messages, the toast. It
  is the region's only elastic content, so `preview_budget` used to *throw
  away* the rows it could not afford: on a 13-row terminal a running `bash`
  call lost its `+N lines` footer and its ctrl+b hint, on a 9-row one its whole
  output block, on a 4-row one the status line. `ui::live::strip_lines` builds
  it as one page now and `render_strip` paints it through
  `render_framed_tail`, so the strip keeps its newest rows and its head flows.
  Frozen, and for the same reason as the band's: the strip moves at the turn's
  own 32 ms cadence, so it signs `ui::live::strip_flow_key` — what the strip is
  *of* — rather than its rows. A streaming reply's **frontier** is excluded:
  `StreamRender` commits its completed lines already, so nothing it drops is
  lost, and flowing it would re-sign per chunk. Only while the composer is on
  screen — a composer-replacing view splits the region with `view_split`, and
  there that view's page is the one that flows.
- **Flow + bottom anchor, per page**: the `AskUserQuestion` modal
  (`docs/ask.md`). Its builder is a line builder like the rest — one flat
  row list per *page* (a question tab, the Submit review), the side-by-side
  preview panel just spans within those rows — and nothing in it ticks: the
  chips, options and entry fields change only on a keystroke. So each page
  flows exactly like a picker: the tail block (options, hints, closing
  rule) closes the page and stays painted, the skipped top (the chip
  strip, the question, the first options) flows, a tab move or an answer
  re-signs the flow into the purge rebuild (the search-line rule), and a
  tail-only ↑/↓ holds it. The retired top-drop clamp instead cut those top
  rows into *no* buffer — on a small terminal the modal opened mid-option
  with its question unreachable, the reported "it hides the texts".

## Why not…

- **…an internal scroll (a pager inside the frame)?** It would trap the
  content behind a new key grammar (↑/↓ already move selections in these
  menus) and behind a window the terminal's own scrolling can't reach. The
  codebase's precedent is the opposite: the permission prompt deliberately
  scrolls displaced chat into *real* scrollback so the user "can scroll the
  terminal up and re-read anything while it asks".
- **…making the region itself taller than the screen?** `term::InlineViewport`
  paints the region as a screen rect; rows above the screen top are by
  definition scrollback, which is what the flow commits. Reusing the rebuild
  path means no new viewport machinery — `write_above` already seats the
  region below an arbitrarily long tail.
- **…letting the ↓ manager's details page sign its rows like the rest?** It
  ticks at 31 fps, so every frame would re-sign the flow and answer with a
  purge rebuild — the whole screen wiped and rewritten thirty times a second,
  which loses the user's scroll position and flickers. Freezing the flowed
  top is the trade: those rows go stale rather than missing.
- **…shrinking the page (fewer `BG_OUTPUT_ROWS`) so it always fits?** That
  changes the details view itself on exactly the terminals where its output
  box is worth the most, to solve a problem the flow already solves. The
  band's own TUI is untouched by this fix.
- **…the permission prompt's cap-and-pad (`… +N lines`)?** It keeps the
  options on screen but hides the middle of the content — the exact complaint
  here. It stays right for the *prompt*, whose context rows tick (a live agent
  tree) and whose answer shouldn't wait on a scroll; a browsing page's content
  is the point of the page.

## The invariants it leans on

- Invariant 3 (content-anchored viewport, rebuilds via `term::reflow`): the
  flow rides the same purge rebuild every resize and history rewind uses;
  `write_above` scrolling a long tail into the empty scrollback is exactly the
  seat the flow needs.
- Invariant 4 (`commits_allowed` gating): pausing commits under a flowed view
  is the agent-session-view rule applied to a second screen-covering view.
- The signature check runs in the draw tick before painting, so a flow is
  established/refreshed/cleared atomically with the frame that shows it.
