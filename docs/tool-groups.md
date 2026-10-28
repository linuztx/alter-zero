# Folded tool groups — the **Verbose output** setting

A turn that explores a codebase is mostly reading: `ls`, `cat`, `grep`, a
`read` of a file, another `rg`. Drawn one cell each, a dozen of them push the
answer the user asked for a screen and a half up, and each cell is a
numbered peek of a file nobody asked to see. Claude Code folds them: with its
`verbose` setting off, a run of reads, searches and listings is **one line**
that counts what happened —

```
● Searching for 1 pattern, reading 2 files… (ctrl+o to expand)
  ⎿  $ grep -rn "fn main" src

Searched for 1 pattern, read 2 files (ctrl+o to expand)
```

— the first while the run is live, the second once it is over. Every call is
still one `ctrl+o` away, expanded exactly as it always was.

This app does the same, behind a `/settings` row: **Verbose output**,
`false` by default (Claude Code's default), `true` drawing every tool call as
its own cell — the inline view exactly as it was before the setting existed.
Seeded for a run by `ALTER_ZERO_VERBOSE`; persisted per directory in
`settings.json` like **Hide thinking**; cycling it purge-rebuilds the
conversation so every committed call redraws in the other shape at once.

## What folds

A **member** of a group is a call that only looked
(`app::tool_group_member`):

- a `read` — except an **image** read, whose picture is the point and keeps
  its own cell under it (`docs/images.md`);
- a `bash` command that `read_only::reading` recognizes as a **search**
  (`grep`, `rg`, `find`, `fd`, `which`, `git grep`), a **read** (`cat`,
  `head`, `tail`, `wc`, `stat`, `file`, `cut`, `sort`, `uniq`, `tr`, `jq`,
  `sed -n`) or a **listing** (`ls`, `tree`, `du`) — every segment of it,
  with `echo`/`printf`/`true`/`false`/`:`/`cd` saying nothing. A compound
  takes its strongest part's kind, a listing over a search over a read, so
  `cat a | grep b` is a search and `cd src && ls` a listing. A command that
  is read-only but neither (`git status`, `pwd`, `date`) keeps its own cell:
  what it printed is the point.

Only a call that **succeeded** stays a member. A failure must not whisper
(the rule the MCP cells live by, `docs/mcp.md`): a `read` of a missing file,
an `ls` of a directory that is not there, a call the user or the classifier
refused, a call an Esc interrupted — each leaves the group as its own red
cell, and the group's line commits in front of it. The one exception is a
**search that exits `1`**: `grep` and `rg` say "nothing matched" that way and
`find` "part of the tree was unreadable", both answers rather than errors, so
the search stays in the count.
A command handed back still running (a session waiting for input, a call
moved to the background) is not a member either — its cell says where the
session stands.

Everything else **breaks** a group: assistant text, a `write`/`edit`, a
command that is not read-only, an MCP call, a subagent launch, a message the
user steered in, a background notice, the turn ending. Three things pass
through without breaking it, because nothing of them sits between the calls
on screen: a **thinking** phase (the model often reasons between two reads —
its settled `Thought for …` line commits as usual, above the live group), a
task tool call (cell-less), and a hook's injected note (cell-less).

## The two looks

**Live** — while the group can still grow, it is a cell in the streaming
strip: the tool bullet **blinking** at the frame pulse (`docs/tool-pulse.md`)
over the counts in the present tense, the dim `(ctrl+o to expand)` hint, and
a `⎿` row naming what it is doing — the running call, else the newest one:
`$ {command}` (whitespace collapsed, two lines at most) or the file's path
(the path display rule, `docs/tools.md`). A command that has run two seconds
adds its own clock, `$ grep -r foo / (12s)`, and past the Ctrl+B delay the
`(ctrl+b to run in background)` row the running cell shows. The bullet keeps
blinking **between** calls — after a `cat` returns, while the model decides
what to read next — because the group is not over until something else
happens. That is what the user watches: the line counting up as the agent
reads.

**Settled** — once something breaks it, the group commits one bullet-less dim
line in the past tense — the settled-line family `Thought for …` and
`Called {server}` already belong to — and nothing else reaches scrollback:

| Counted | Present | Past |
| --- | --- | --- |
| searches | `Searching for 1 pattern` | `Searched for 2 patterns` |
| reads | `reading 1 file` | `read 3 files` |
| listings | `listing 1 directory` | `listed 2 directories` |

in that order, joined by commas, the first capitalized. A read counts
**files**, not calls: a `read` of `a.py` and a `cat a.py` are one file, and
`cat a.py b.py` is two (`read_only::Reading::targets` names the operands of
the readers whose arguments can be told apart from them; one naming none
counts once). A listing counts the directories it named the same way, a bare
`ls` counting once.

## How a line can wait in the strip

Scrollback is append-only, so a group cannot commit until it is over — and
over is decided by what comes *next*. The resolved members are therefore
**held**: recorded in `history` like any call, but not committed, while the
strip draws them. Two pieces keep the hold honest.

**Open** (`App::tool_group_open`, derived — never stored): verbose off, a
turn in flight, no assistant text streaming yet (whitespace does not count),
no live subagent group, no `write` being generated, the queue empty or its
next call a member, and held members to show. Every event re-derives it.

**The floor** (`App::tool_group_floor`): the history length at the last
moment a group line committed. The held members are the trailing run of
members (and pass-throughs) at or above it (`app::held_members`). Most
breakers leave a trace in history — the text, the `write`, the notice — and
end the trailing run by themselves; the floor covers the ones that do not
yet (a subagent launch is recorded when the group resolves, text is recorded
when the segment flushes), so a member resolving after one never re-commits
a line that is already in scrollback. History shrinking (`/clear`, a
rewind, a withdrawn reply attempt) clamps it.

The boundary commits at exactly two kinds of moment:

- **A call resolves** (`ui::folded_tool_commit_lines`): a member that leaves
  the group open commits nothing; otherwise the held members commit as their
  line, and a call that is not a member commits its own cell after it.
- **Something breaks the group** (`tui::stream`'s arms, the Esc interrupt):
  the first non-blank text chunk, a batch or lone call whose first call is
  not a member, a `write` starting to stream, a subagent launch, a steered
  message, a background notice, the stream's end or failure — each commits
  the held line (`ui::held_tool_group_lines`) **before** its own rows, so
  scrollback reads in order.

Each seals (`App::seal_tool_group`), whether or not committing is allowed
right now: under an agent session view or a flowed page the line is not
written, but the return's purge rebuild draws it from history, and an
unsealed group would be written a second time by the next close.

**Rebuilds agree by construction.** `ui::conversation_lines_with` walks
history with the same membership and the same breakers, and is handed the
open group's first member (`ToolFold::On { held_from }`, from
`App::tool_fold`), which it leaves out — the strip is showing it. A
pass-through inside a group is drawn where it lies, so a thought that
settled while the group was live sits above the group's line in a rebuild
just as it did in scrollback. The one shape a rebuild draws differently is a
read run in the same round as a subagent launch: live, the launch sealed the
reads before it and the ones after committed on their own; a rebuild sees
one unbroken run and counts it once. Nothing is lost or doubled, only
counted together.

## What does not change

- **Ctrl+O** shows every call expanded, verbose or not — it is what the hint
  points at, and Claude Code's transcript is always verbose too. `/export`
  copies the same rows.
- **The permission prompt** shows the call it asks about as its own cell —
  and, above it, the group the turn is holding, drawn at rest. Manual and
  edit mode ask for every command, read-only ones included, and the prompt
  replaces the strip that was drawing the group, which is held out of
  scrollback: without it, the calls the group counts would be on no screen
  while the user decides about the next one. Only over the main turn's own
  question — its thread is parked on the gate, so the group cannot change
  under the prompt — and the first context row to give way on a short
  terminal (`ui::permission_view`'s `held_group_lines`).
- **An agent session view** keeps a cell per call (the MCP collapse's rule,
  `docs/mcp.md`) — its commits key on the transcript items the fold
  recorded, a second commit path this design would have to hold in step.
- **The derived context, the rollout and Ctrl+D** never see a group. It is
  a way of drawing history, not something history holds.

## Read-only commands and auto mode

The same parser answers a second question: `read_only::is_read_only`. In
**auto** mode a `bash` command that only reads runs without the classifier —
there is nothing for a reviewer to review in `ls -la`, and a round trip per
`grep` made exploring slow and filled every cell with `⎿ Allowed by auto
mode classifier` (`docs/permissions.md` *Read-only commands*). Manual and
edit mode still ask: those are the postures the user chose.

The rules are Claude Code's (`BashTool/readOnlyValidation.ts`), cut to what a
model runs, and every doubt answers **no** — a wrong no costs one classifier
round, a wrong yes runs something unreviewed:

- every segment (`|`, `&&`, `||`, `;`, `&`, a newline) a known program, its
  name bare — no path, no quotes, no `VAR=` in front, no wrapper (`sudo`,
  `env`, `xargs`, `time`, `sh -c`);
- nothing redirected except into `/dev/null` or onto a descriptor (`2>&1`),
  no heredoc, no process or command substitution, no `$name` or `${…}`
  expansion (a `$` that anchors a regex is text), no subshell or brace group,
  no `$'…'` quoting;
- per program, the flags that write or run something refused — `find -exec`
  / `-delete` / `-fprint`, `sort -o`, `tree -o`, `rg --pre`, `fd -x`, `file
  -C`, `sed` anything but a quiet `p`, `uniq`'s second (output) operand,
  `date` and `hostname` positionals, `jq -f` / `--rawfile` and any filter
  reaching `env` — **including an abbreviation**: GNU `getopt` and git take
  any unambiguous prefix of a long option, so `sort --out=x` is `--output`;
- an unquoted glob only for a program with no flag it could expand into that
  does anything but read (`ls *.rs` yes, `find * …` no — a file named
  `-delete` would be an argument);
- `git` for the subcommands that only look (`status`, `log`, `diff`,
  `show`, `blame`, `grep`, listing `branch`/`tag`/`remote`, `config --get`,
  `stash list`, …), with no global option but `--no-pager`.

Paths are not restricted: the `read` tool already reads any file unasked, and
a read-only command can do no more than it.

## Testing

Pure, and unit-tested:

- `read_only` — what is read-only, what each command reads as, the operands
  it names, and the shapes that must not slip through (abbreviations,
  quoted flags, descriptor and device redirects).
- `llm::approval` — auto mode runs a read-only command with the classifier
  never consulted and no prompt; a look-alike that redirects still meets it;
  manual and edit still ask; a forced ask still asks.
- `app::tests::tool_group` — membership, the held run and the floor, when a
  group is open, the clamp on rewinds.
- `ui::tests::tool_group` — the count text in both tenses, the settled line,
  the live cell and its hint, the folded walk and its agreement with the
  commit path, the open group left out of a rebuild.
- `ui::tests::permission_view` — the held group above the call a prompt asks
  about, none with Verbose output on or over a subagent's question, and the
  group giving way before the call on a short terminal.

- `stream::tests::scenario` — the offline `explore` demo: two rounds of
  calls that all join one group, a thought between them.

On screen, `smoke.sh` runs every phase with `ALTER_ZERO_VERBOSE=1` exported —
the existing phases assert one cell per call — and Phase 146 seeds it off to
drive the folded view itself. Offline, the `explore` demo: the live line over
the thought between the rounds, the second round joining it, nothing of the
group in scrollback until it settles into one line under the two `Thought
for` lines, a height change rebuilding the conversation row for row, the
`/settings` row redrawing every call as its own cell and back, and Ctrl+O
expanding every call. Then the real backend in auto mode against a stub
provider that logs every request: an `ls -la` and a `grep` fold into one
line and run with **no** classifier request, while the `touch` after them is
classified — the positive control that proves the stub would have seen one —
its cell committed under the group's line. And the same turn in manual mode,
every command asking: the prompt for the search draws the held listing above
it, and after the prompts' rebuilds scrollback still holds one line.
