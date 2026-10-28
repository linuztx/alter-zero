# Collapsed read/search groups — the non-verbose tool view

An agent spends much of a turn finding its way around before it changes
anything: a listing, a search, a file, another file. Drawn as a cell per call,
that exploring fills the screen with output nobody reads — a `● Bash(ls -la)`
over a directory listing, a `● Read(src/app.rs)` over ten numbered lines, a
`● Bash(grep -rn …)` over its matches — and pushes the part of the turn that
matters, the change and the reply, off the top. Claude Code's non-verbose view
folds such a run into one line, and this is that, ported.

```
● I'll get my bearings first.

● Searching for 1 pattern, reading 2 files, listing 1 directory… (ctrl+o to expand)
  ⎿  $ grep -rn 'card()' .
```

while it runs, and once something that is not a read arrives:

```
● I'll get my bearings first.

Searched for 1 pattern, read 2 files, listed 1 directory (ctrl+o to expand)

● Bash(python3 about.py)
  ⎿  alter-zero — an autonomous AI agent that lives in your terminal
```

Every call is still recorded whole: the **Ctrl+O** transcript expands each one
exactly as before, the derived context (Ctrl+D) and the rollout are untouched,
and a `/resume` repaints the same line from the same records. The group is only
what the *conversation* shows of them.

## Verbose output

The `/settings` **Verbose output** row (`docs/settings.md`) chooses between
the two views, and it is **off by default** — the collapsed view, Claude
Code's default too. On, nothing groups at all: every call keeps the cell it
always had, byte for byte, so the row is a way back to the old design rather
than a third one. Cycling it purge-rebuilds the conversation, so what is
already in scrollback changes shape at once; `ALTER_ZERO_VERBOSE` seeds it for
a run (the smoke suite exports `1`, so its phases keep the per-call cells they
assert on).

## The two looks

**Live**, in the streaming strip, while the group is open:

```
● Reading 1 file, listing 1 directory… (ctrl+o to expand)
  ⎿  $ ls -la
```

- The bullet is the running tool's **blinking grey** (`docs/tool-pulse.md`),
  and it keeps blinking for as long as the group is open — **between calls
  too**. A read that has finished does not end the group: the model's next
  call may well be another read, and a cell that stopped blinking between
  rounds only to start again would say the work had stopped when it had not.
  It stops when the group closes (below): a call of another kind, text, an
  error, the turn's end.
- The clauses are in the reference's order — searches, then reads, then
  listings — each verb in the tense the group is in (`Searching for`,
  `reading`, `listing`), the first capitalised, the counts **bold**, the
  nouns pluralised past one (`1 pattern`, `2 patterns`; `1 directory`,
  `2 directories`), the line closed by a `…`.
- The `⎿` row says what the group is doing, **one call's value at a time,
  in the order the calls ran** (*The hint row*, below), and between rounds it
  keeps saying what the group last did. A read names its file (by the
  session's path display rule, `docs/tools.md` *Path display*); a command is
  `$ {command}` — its first line, whitespace runs closed up, a ` …` when more
  lines follow.
- The dim `(ctrl+o to expand)` is the first thing a narrow terminal drops;
  past that the clauses are cut with a `…`.

### The hint row

A parallel batch resolves in milliseconds — three reads and a listing are
done before the next frame is drawn — so a row that named the newest call
showed only the batch's **last** value, and the others never reached the
screen at all. The reference has the same problem one step removed: it holds
each hint for `MIN_HINT_DISPLAY_MS` (700 ms), but as a *throttle* (its
`useMinDisplayTime`), which drops every value that arrives during the hold
and shows the newest once the hold ends — so its batch, too, jumps from the
first value to the last.

Here the row is a **queue**. The values are the group's calls in the order
they ran — the held ones, then the running one — and the row steps through
them one at a time, each holding it for [`GROUP_HINT_HOLD`] (the reference's
700 ms) before the next replaces it, never skipping one however late a frame
comes. A read and a listing in one round read

```
● Reading 1 file, listing 1 directory… (ctrl+o to expand)
  ⎿  README.md
```

for 700 ms, then

```
● Reading 1 file, listing 1 directory… (ctrl+o to expand)
  ⎿  $ ls -la
```

while the clauses count the whole round from the start. A call still waiting
its turn in a batch is never shown ahead of the ones before it (the batch's
first call stands in until something has run), a group a round at a time
shows each call's value the moment it runs (the model's seconds of writing
its next call are longer than the hold), and the last value stays on the row
until the group closes. The queue lags reality by at most one hold per call
that finished faster than one — a few seconds for a large batch, played out
while the model writes its next call.

The state is pure: `App::group_hint` — which call is on the row and since
when — stepped by [`App::advance_group_hint`] on the boundary's frame clock
(the reading `set_pulse` takes, injected before every draw while the turn's
32 ms animation chain runs), and reset whenever a group closes, so the next
group starts from its own first call. The permission prompt's still frame
steps through the held calls alone, since the call it asks about has a cell
of its own there.

**Settled**, in scrollback, once the group closes:

```
Read 1 file, listed 1 directory (ctrl+o to expand)
```

Bullet-less, flush at the margin and dim throughout — the settled thought's
dress (`docs/thinking-stream.md`, `Thought for 4s · 1.2k tokens (ctrl+o to
expand)`), since what is left is a fact about the turn rather than something
happening. The counts stay bold.

## Which calls join a group

A **member** is a call the group takes ([`app::is_group_member`]):

- a `Read` of a file — any but an **image** (`png`, `jpg`, `jpeg`, `gif`,
  `webp`), whose cell is the picture drawn under it (`docs/images.md`):
  folded into a group's line it would draw nothing, so an image read always
  keeps a cell of its own;
- a `Bash` command that only reads, by [`read_only::read_kind`] — a
  **listing** (`ls`, `tree`, `du`), a **search** (`grep`, `rg`, `find`, `fd`,
  `which`, …) or a **read** (`cat`, `head`, `tail`, `wc`, `jq`, `sed -n
  '1,40p'`, `awk '{print $1}'`, …). A pipeline or list takes its strongest
  kind — a listing wherever one appears, else a search, else a read, the
  reference's precedence — so `ls | grep x` lists and `cat f | grep x`
  searches; `echo`, `printf`, `true` and `cd` ride along without a kind of
  their own, so `cd src && ls` is a listing.

and it joins only once it has **run**, whether it succeeded or failed in the
running — a search that matched nothing exits 1, a read of a missing file
fails; the model read the error either way, and Ctrl+O keeps it. A call that
never ran or never finished keeps a cell of its own, because its cell is the
record of *why*: refused at the prompt or by the classifier, blocked by a hook,
cut by Esc or a backend error, refused by the tool-call limit, or moved to the
background (`wait: 0`, `run_in_background`, or Ctrl+B on a long search). So
does a `!` command the user ran, and an MCP call, which has a collapsed line of
its own (`docs/mcp.md`).

A member's own extras ride with it into Ctrl+O: the `⎿ Allowed by auto mode
classifier` note a command the strict check could not vouch for earned
(`grep -c TODO *.rs` is a search, but its glob sends it to the classifier), a
hook's `⎿` note, the full output.

`read_kind` is deliberately **generous** about what it calls a read, and
strict about one thing only: a collapsed line must never summarise a write as
a read. A glob or a variable is fine (`grep -c TODO *.rs` is still a search),
while a redirect to a file, a `$(…)` substitution, a background `&`, a
subshell, and the flags that make a reading program write or run something
(`find -delete`/`-exec`, `sort -o`, `tree -o`, `rg --pre`, `fd -x`, `uniq IN
OUT`, `awk` with `system`/`getline`/`>`/`|`, `sed` beyond `-n …p` and one
stdin `s///`) make it no read at all. Whether a command *provably* reads —
whether auto mode may skip the classifier for it — is a different, stricter
question with its own function, [`read_only::is_read_only`]
(`docs/permissions.md` *Read-only commands skip the classifier*). The two share
the tokenizer and nothing else.

### What a group counts

- **Searches** — every search command, each one a "pattern".
- **Reads** — every `Read` **path once**, however often the model re-read it,
  plus every reading command, each one a "file" (`cat a b` counts one: the
  words do not say how many files a command opens, and the reference counts
  the same way).
- **Listings** — every listing command, each one a "directory".

## The group's lifecycle

A group is a property of the **history**, like the parallel MCP run
(`docs/mcp.md`): the records stay ordinary `HistoryItem::Tool`s, and a group is
any maximal run of members, allowing through the items that have no inline
cell of their own (a task call, a hook note) and the **thoughts** between them
([`app::group_run`]). That is what lets every rebuild — a resize, an overlay
return, a `/resume` — derive the same line from the same records
(`ui::conversation_lines`), with no group id to record or to drift.

What scrollback has not taken yet is the **open** group: `App::tool_group`,
the history index it opened at, set the moment its first member resolves.
While it is open nothing it holds reaches scrollback — the strip speaks for it
— and every rebuild leaves it out the same way it leaves out an MCP run still
in flight (`ui::committed_history`). Each resolution is placed by one call,
[`App::place_resolved_tool`]: a member is **held** (opening the group if none
is open), anything else **closes** the open group ahead of itself, so the
group's line commits above the call that closed it.

### What closes a group

Whatever is not part of it, at the moment it would reach the screen — the
boundary closes the group ahead of each (`tui::commit::close_tool_group`, a
no-op with none open):

- **reply text that says something** — the line lands above the reply's first
  row; whitespace closes nothing, since it records nothing;
- **a call of another kind**: its batch announcement, its start, or its
  resolution, whichever comes first — a parallel batch led by a read joins the
  group, one led by anything else closes it;
- a **subagent launch**, a **`write` starting to stream** into its cell, a
  **steered message** landing, a **notice** (an error notice, a background
  shell's or an agent's completion);
- a **permission prompt** about a call that is no read, so the line commits
  above the question; a prompt about a *read* (manual mode asking about `ls`)
  keeps the group open and shows it at rest above the asked-about call's own
  cell in the prompt's context;
- a **backend error**, an **Esc interrupt**, and the **turn's end** — with a
  backstop at every turn end (`dispatch_after_turn`), so no path can leave a
  group holding its reads out of scrollback.

### Thoughts

A thought **between** two reads is part of the work the line stands for: it
settles into the history inside the group (`tui::stream`'s
`settle_reasoning` holds it while a group is open) and shows in Ctrl+O, not
inline. While it streams, its live `● Thinking…` block sits **under** the open
group's cell, which stays up through it. A thought that settles **after** the
group's last read — the model thinking before it answers — is shown after the
group's line as its own `Thought for …` cell when the group closes on it, the
order it happened in (`ui::tool_group_lines`).

## Where it does not apply

- **Ctrl+O** is always verbose: the transcript is what happened, every call
  whole.
- **An agent's session view** keeps a cell per call: its commits are keyed on
  the records its own run makes (`Session::commit_agent_tail`,
  `docs/agent-view-streaming.md`), and a held group there would need a second
  open-group state over a second history. The main view's collapsed line is
  the lead's own calls.
- **A `!` command** is the user's own, and always its exec cell
  (`docs/shell-command.md`).

## Limitations

- A connection lost **mid-reply** right after a group closed on that reply's
  first words voids the attempt (`docs/offline.md`): the reply goes, the
  closed group stays closed, and if the restarted round reads again a second
  group opens beside the first. Scrollback then shows two lines where the next
  rebuild shows one — harmless, and rare enough not to carry a reopen path.

## Testing

The pure halves are unit-tested: membership, runs, counts, placement and the
hint row's queue (every value in order, each held, none skipped, a new group
starting over) in `app/tests/tool_group.rs`; the two looks, the path display,
the narrow-width fit, a parallel batch's first value then its next, and the
rebuild in `ui/tests/tool_group.rs`; `read_kind` and
`is_read_only` over the reference's own cases plus adversarial ones (redirect
spellings, unicode spaces, `find` grouping, line continuations) in
`read_only/tests.rs`. The offline dummy's **`explore`** scenario (a prompt
naming "explore") drives a whole group with no network: a listing, a thought
between the rounds, a parallel search and two reads, a second thought — a
live model's pause before its next call, in which the hint row plays the
round's values through — then a script run that closes it, every call
resolving with the real executor's output.
`scripts/smoke.sh` Phase 146 plays it against the real binary — the live cell
over the streaming thought, the clauses growing, the parallel round's three
values taking the hint row in turn, the one settled line above the closing
call with no per-read cell, the thought between the reads absorbed and the one
after them under the line, Ctrl+O expanding every call, a resize rebuild
painting the same line, and **Verbose output** unfolding it — while every
other phase runs with `ALTER_ZERO_VERBOSE=1`.
