# Emphasis — the facts inside dim meta text

## The problem

Most of what the inline TUI says *about* the work is dim on purpose. A
turn's receipt, a thought's footnote, a capped peek's hint, the status
line's metrics, the footer and the spinner tip sit back so the reply and the
tool output in front of them read first. But each of those lines carries a
fact or two the eye goes looking for: how long, how many, which key. In one
flat grey those facts were as hard to find as the filler around them:

```
Thought for 1m 5s · 1.5k tokens (ctrl+o to expand)
     … +42 lines (ctrl+o to expand)
Worked for 12s · 6.1k tokens (2.8k cached)
⣤⣀⣀⣀⣀⣀⣀⣀ Working… (12s · ↓ 1.2k tokens · esc to interrupt)
  ⎿  Tip: Press ctrl+o to see the whole transcript and every tool's output
  gpt-5.5 medium · ~/repo · 12.3k/272k (4.5%)                       auto
```

## What it does now

The line keeps its dim, and only its **facts** take the theme's
**emphasis**, a soft version of the theme's own accent. It is clearly louder
than the dim around it and clearly quieter than the accent the pickers select
with, so a fact can be found at a glance without the meta competing with the
reply. In the block above, these take the emphasis:

| line | its facts |
| --- | --- |
| `Thought for 1m 5s · 1.5k tokens (ctrl+o to expand)` | `1m`, `5s`, `1.5k`, `ctrl+o` |
| `… +42 lines (ctrl+o to expand)` | `+42`, `ctrl+o` |
| `Worked for 12s · 6.1k tokens (2.8k cached)` | `12s`, `6.1k`, `2.8k` |
| `(12s · ↓ 1.2k tokens · esc to interrupt)` | `12s`, `1.2k`, `esc` |
| `⎿  Tip: Press ctrl+o to see …` | `ctrl+o` |
| `gpt-5.5 medium · ~/repo · 12.3k/272k (4.5%)   auto` | `gpt-5.5 medium`, `4.5%`, `auto` |

A **fact** is one of:

- **a key a hint names**: `ctrl+o`, `ctrl+b`, `esc`, `shift+tab`, `tab`,
  `enter`, and an arrow the hint says what it does (`↓ to manage`). The
  status line's `↓ 1.2k tokens` arrow is the tally's direction, not a key,
  and stays dim;
- **a number the line reports**: a time (`12s`, `1m 5s`), a count (`3 tool
  uses`, `+42 lines`), a token figure (`6.1k`, `≈24.3k`), a share (`4.5%`);
- **what the session is**, in the footer: the model with its thinking mode
  and speed tier, the share of the context window used, the running shells'
  count, and the permission mode. `master` mode, which runs every call
  unasked, takes the caution amber (`footer_master_mode_color()`) instead;
- **an info toast**, whole: it answers something the user just did, and a dim
  line above the box was easy to miss. An error toast keeps its softened red
  (`docs/toast.md`);
- **what a spinner tip teaches**, the key, command or command line the
  catalog marks (below).

Never a fact: anything the app did not write. A command (`head -n 20`), a
path, a model's or an agent's words, a session id, a server name like
`context7`. Two other things stay as they were. The live group line
(`● Reading 2 files…`) is a header, so its counts keep their bold white. A
scratchpad edit's `+55 -2` stays dim beside the count it details.

## The colour: the `emphasis` palette role

`emphasis` is a role on `Palette` (`src/ui/palette.rs`) read through
`emphasis_color()` (`src/ui/theme.rs`), so it follows `/theme` like every
other colour. Each theme's value is its own accent mixed part of the way toward
its dim, enough to sit about the same perceptual step above the dim in every
theme (CIELAB ΔE ≈ 18–32). The mix is larger where the accent is vivid and
smaller where it already sits close to the dim. It never goes past halfway, so
the result is always nearer the accent than the dim:

| theme | rule | `emphasis` |
| --- | --- | --- |
| `mocha` | sky → `overlay1`, 40% | `#85B9CB` |
| `macchiato` | sky → `overlay1`, 35% | `#8BBBCC` |
| `frappe` | sky → `overlay1`, 30% | `#92BCCB` |
| `latte` | sky → `text`, 35% | `#1D87BA` |
| `onedark` | cyan → dim, 35% | `#68A7AE` |
| `dracula` | cyan → comment, 45% | `#79B3D5` |
| `nord` | `nord8` → dim, 25% | `#85B2C4` |
| `gruvbox` | aqua → `gray`, 40% | `#90A879` |
| `solarized` | cyan → `base01`, 35% | `#3A8F8C` |
| `monokai` | cyan → comment, 45% | `#6DAAAE` |
| `ansi` | the terminal's own `Cyan` | `Cyan` |

**Latte** is the exception. On a light ground its sky accent is *lighter*
than its dim (2.5:1 against the base, where the dim is 2.8:1), so mixing it
toward the dim would make a fact harder to read than the text around it. It is
darkened toward the theme's text instead, which puts it at 3.6:1. **`ansi`**
has nothing to mix, so it keeps the terminal's cyan.

`the_emphasis_is_each_themes_accent_softened_toward_its_dim` pins the role in
every theme: its brightness lies between the dim and the text, it is nearer
the accent than the dim, it is none of those three colours, and Mocha's value
is checked exactly.

## Where it lives

`src/ui/emphasis.rs` is the whole mechanism:

- `fact_ranges(text)` reads a line of meta word by word. Each word is stripped
  of the brackets and punctuation around it (`(ctrl+o`, `50s)`, `2.8k,`) and
  kept when it is a key, an arrow followed by `to`, or a number: digits with an
  optional decimal part, an optional `+` or `≈` in front and a unit after. A
  digit inside a word (`gpt-5.5`, `v0.17.0`, `#1`) is no number.
- `meta_spans(text, base)` turns the line into spans: the facts in
  `emphasized(base)`, the rest in `base`. The text is unchanged, and a line
  with no fact is still the single `base` span it was.
- `meta_row(index, text)` is a dim `⎿` row (`result_row`'s geometry) built
  from `meta_spans`.
- `emphasized(style)` swaps only the foreground, so a bold count stays bold.

Because `fact_ranges` reads every number as a fact, it only ever runs over
text the app wrote itself. The renderers that need it call it explicitly:

| surface | renderer |
| --- | --- |
| status line metrics (main turn, agent session view, `/spinner` preview) | `ui::status::styled_status_line` |
| turn summary, per wrapped row | `ui::status::summary_lines` |
| settled thought, inline and in Ctrl+O | `ui::reasoning` |
| `… +N lines` / `… (ctrl+o to expand)` peek hints | `ui::tool::more_hint_line`, `ui::file_cell` |
| running command clock rows, the `!` shell's `Running… (Ns)` | `ui::tool::running_command_lines`, `shell_running_line` |
| `Running in the background (↓ to manage)` | `ui::tool` |
| the Ctrl+B hints | `ui::live`, `ui::agent` |
| a streaming `write`'s `Writing N lines…` corner | `ui::file_cell::draft_cell_lines` |
| `Called {server} N times` / `● Calling …` hint | `ui::tool` |
| the settled group line's counts (bold), its hint, the hint row's clock | `ui::tool_group` |
| agent tree counters, `Done (…)` receipts, hints, the roster's runtime | `ui::agent` |
| `● Context compacted · 88k → 2.1k tokens · 12s` | `ui::message::compaction_lines` |
| the idle task count line (bold numbers) | `ui::tasks` |
| the checkpoint `Snapshotting 326 files (7.9 MB)…` notice | `ui::header::startup_notice_lines` |
| footer, toast | `ui::footer` (built field by field, not scanned) |
| spinner tip | `ui::tips` (the catalog's marks, not scanned) |

## Tips mark what they teach

The tip catalog (`tips::TIPS`) is prose, and its keys and commands come in
forms a scanner would have to guess at: `Press ? on`, `Type @ to`,
`alter-zero --continue`, `.alter-zero/agents/<name>.md`. So each entry marks
what it teaches in backticks (``Press `ctrl+o` to see …``).
`tips::parts` splits an entry at its marks, `tips::plain_text` drops them, and
the row (`ui::tips::tip_lines`) draws the marked parts in the emphasis and
never shows the backticks. The catalog's tests check that every mark is
closed, that no mark is empty, and that every tip naming a key, a
`/command` or `alter-zero` marks it. The palette check reads the commands
with the marks dropped, so a marked `/command` is still checked against
`COMMANDS`. Smoke Phase 125 reads the catalog out of the source and drops the
marks the same way.

## Testing

Unit tests sit beside each renderer's existing tests: `ui::tests::emphasis`
for the scanner, plus status, reasoning, tool, file cell, live, tool group,
agent, message, tasks, header, footer, tips and palette tests. Each names the
facts a line must pick out (`emphasized_texts` in `ui::tests`) and checks that
the rest stays dim. Two smoke phases read the colour itself: Phase 28 and
Phase 120 expect an info toast to wear Mocha's emphasis (`38;2;133;185;203`).
Phase 57 matches the turn summary on the plain capture, since the summary's
time is now a span of its own.
