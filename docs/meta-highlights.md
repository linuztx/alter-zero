# Meta highlights: values and keys inside the dim lines

Most of what the inline TUI prints about its own work is **dim**: the turn
summary under a reply, the `Thought for …` line, the status line's metrics,
a folded peek's `… +N lines (ctrl+o to expand)`, a running command's clock,
a collapsed read/search group, the footer. That is deliberate — those rows
are facts about the conversation, not the conversation — but an all-dim row
makes the reader hunt for the one thing it was printed to say: how long, how
many, which key. So inside those rows the words stay dim and two kinds of
thing are lifted, in colours every theme already owns:

| what | example | colour | accessor |
| --- | --- | --- | --- |
| a **value** — a duration, a count, a size, a share | `45s`, `12.3k`, `+48`, `3` | the theme's second ink, `text_muted` | `meta_value_color()` |
| a **key** a hint names | `ctrl+o`, `esc`, `ctrl+b`, `↓` | the accent, mixed `META_TINT_DIM_MIX` toward the dim | `meta_key_color()` |

```
  Thought for 12s · 1.2k tokens (ctrl+o to expand)
              ^^^   ^^^^         ^^^^^^
              value value        key
  Worked for 45s · 12.3k tokens (8k cached)
⣤⣀⣀⣀⣀ Working… (12s · ↓ 1.2k tokens · Thinking for 3s · esc to interrupt)
```

## Why two tones and not one colour

A value and a key are read for different reasons. A value is **read** — the
eye scans the row for the number — so it is lifted by **brightness**: the
theme's own secondary ink (Catppuccin's `subtext1`, One Dark's `#C8C8C8`, …),
brighter than the dim around it and never as loud as the reply text. A key
is **acted on**, and everywhere else in this TUI a key is coloured: the
permission prompt's and the ask modal's hint keys, the Ctrl+R search line's,
the backtrack hint's, the `?` band's, all in the accent. So a hint's key
takes the accent's **hue** — softened, because these hints repeat down the
whole scrollback (every folded peek, every thought, every group carries
one), and the full accent there would outshout the inline code and the
picker selections that wear it at full strength.

Colouring the values too was tried and refused: in a theme whose link hue is
already muted (Gruvbox's `#83A598`) a softened second hue vanished into the
dim, and a number in the accent's hue reads as code or as something
selectable. Brightness carries "this is the fact" in every theme, light ones
included — on Catppuccin Latte `text_muted` is the *darker* ink, which is
exactly what stands out on a light ground.

## Softened: `META_TINT_DIM_MIX`

Every hue this feature adds is the theme's own colour **mixed 35% of the way
toward its dim** (`lerp_color(hue, dim, 0.35)`, the error toast's softening
rule, `docs/toast.md`): the key's accent, and the footer's mode and gauge
colours below. Mixing toward the dim — rather than inventing a pastel —
keeps the result inside the theme's own palette and makes it calmer by the
same amount in all eleven themes. Under the `ansi` theme, whose colours are
the terminal's own and have no components to mix, `lerp_color` steps to the
nearer end, which is the hue itself — the terminal's own cyan, red, amber —
so that theme keeps following the terminal (`docs/theme.md`).

## The footer: what is worth a colour there

The footer is the one dim row that is always on screen, so it gets the
treatment segment by segment rather than token by token:

- **The model** is the session's identity — a value: `text_muted`. Its
  thinking mode and speed tier beside it stay dim; they qualify it.
- **The context gauge** — `12.3k/272k (4.5%)` — is a value while the window
  has room, then **turns** as it fills, on the `/context` page's own
  thresholds (`docs/context-usage.md`): the softened warning amber from
  `CONTEXT_USAGE_WARN_SHARE` (75%), the softened error red from
  `CONTEXT_USAGE_FULL_SHARE` (90%, where auto-compact runs). It is the one
  footer fact that changes *toward* a problem, so it is the one that says so.
- **The shells count** — the `2` of `2 shells` — is a value; the ↓-focused
  chip keeps its own fill.
- **The permission mode** at the right edge is coloured by **how much runs
  without asking**, Claude Code's mode line in this TUI's palette: `manual`
  stays dim (everything asks — nothing to flag), `edit` wears the softened
  `purple` (Claude Code's accept-edits colour), `auto` — the default — the
  softened accent, and `master`, which runs everything unasked, the softened
  error red (Claude Code's bypass-permissions red). The mode is the one
  segment with a safety meaning (`docs/permissions.md`), and now it reads at
  a glance.
- **The cwd** and the separators stay dim.

## Where it applies

Values and keys are marked where the row is **built**, never found by
scanning text afterwards: a row that carries a path, a command or a model's
words around its numbers would otherwise light up digits that are nobody's
value. The pure builder is `ui::meta::MetaSpans` (`text`, `value`, `hint`),
and a hint constant's keys are found by its own grammar —
`ui::meta::hint_spans`: `{key} to {action}` clauses joined by ` · `,
inside the parentheses when the hint has them (`Running in the background
(↓ to manage · ctrl+o to expand)` lifts `↓` and `ctrl+o`, never the words
before the parenthesis). Every hint constant the inline rows print is pinned
to that grammar by a test, so a new one either parses or fails the suite.

| row | values | keys |
| --- | --- | --- |
| status line metrics (`docs/status-indicator.md`) | elapsed, token count, thinking time | `esc` |
| turn summary `Worked for …` | elapsed, tokens (with `≈`), cached, written, shells | — |
| `Thought for …` (`docs/thinking-stream.md`) | elapsed, tokens | `ctrl+o` |
| folded peek `… +N lines (ctrl+o to expand)`, the file peek's cut hint | `+N` | `ctrl+o` |
| running command clock `(22s · wait 1m 50s)`, `Running… (12s)` | elapsed, wait, `+N` | — |
| `(ctrl+b to run in background)` / `(ctrl+b to stop waiting)` | — | `ctrl+b` |
| collapsed group line (`docs/tool-groups.md`) | counts (still bold), `+55 -2` | `ctrl+o` |
| an open group's `⎿ $ grep … (12s · 34 lines)` row | the time, the lines printed — never a digit of the command | — |
| MCP `Called …` / `● Calling …` lines (`docs/mcp.md`) | — | `ctrl+o` |
| `Running in the background (↓ to manage)` | — | `↓` (and `ctrl+o` on an agent's) |
| agent tree and cells (`docs/agent-tool.md`) | tool uses, tokens, runtime | `ctrl+o`, `↓` |
| the agent roster's rows over the footer | runtime, tokens | — |
| the task list's `3 tasks (1 done, 1 in progress, 1 open)` (`docs/task-tools.md`) | every count (still bold) | — |
| footer (`docs/footer.md`) | model, gauge, shells count | — (the mode is coloured by risk) |

The same builders paint the Ctrl+O transcript, so a summary or a thought
reads the same there. What is **not** touched: the reply, the tool output,
the user's bubble, an agent's activity row (what it is doing is content — a
command, its own words — not a fact about the turn), the pickers' own hint
rows (their selection already wears the accent), toasts and the spinner tip.

A wrapped row keeps its dress: the turn summary is built as styled pieces
and wrapped by the span-aware `wrap_inline`, which makes exactly the rows the
plain `wrap_text` made (`the_summary_wraps_exactly_as_its_plain_text_did`
pins it at every width from 4 to 100), so a value split across two rows is
lifted on both.

## Tests

- `ui/tests/meta.rs` — the builder's three kinds of piece, the hint grammar
  (one key, two keys, a lead before the parenthesis, no clause), every
  inline hint constant parsing with its keys, and the two tones in every
  theme: the key softer than the accent and distinct from the dim, the value
  the theme's second ink, `ansi` keeping its terminal colours.
- The renderers' own suites (`status.rs`, `reasoning.rs`, `tool.rs`,
  `file_cell.rs`, `tool_group.rs`, `agent.rs`, `tasks.rs`, `live.rs`,
  `footer.rs`) — each row's values and keys in their tones and its words
  still dim, its text unchanged; a command's own digits never lifted; the
  footer's mode per mode, its gauge across both thresholds, and the focused
  shell count one chip.
