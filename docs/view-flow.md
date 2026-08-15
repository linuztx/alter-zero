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
   the cursor parks on is the one actually painted.

2. **The skipped top flows into real scrollback.** When the body exceeds the
   whole terminal (`lines.len() > term_height` — the only case rule 1 can skip
   anything, since the view body is bottom-pinned by `view_split` and squeezes
   the strip first), the hidden top rows are **committed above the live
   region**, exactly where they visually belong: scrollback ends with the
   page's top, the screen shows the page's tail, and the terminal's own
   scrolling reads the whole thing as one piece. `ui::view_flow` is the pure
   decision — which rows flow, and a **signature** (a hash of the flowed rows'
   text at this width/height) identifying the flowed state.

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

## Scope — which views flow

- **Flow + bottom anchor**: every content-driven framed view — the browsing
  menus (`/mcp`, `/hooks`, `/trust`) *and* the windowed pickers (`/model`,
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
- **Bottom anchor only**: the ↓ background manager. Its details page
  live-tails a running shell — per-frame content whose flow signature would
  churn a purge rebuild every tick — and it is bounded by design
  (`BG_OUTPUT_ROWS`), so anchoring alone keeps its interactive tail visible
  on a squeezed terminal.
- **Unchanged**: the permission/ask modals keep their own engineered
  cap-and-pad layout, whose options are already pinned against the bottom
  rule (`docs/permissions.md`, `docs/ask.md`).

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
