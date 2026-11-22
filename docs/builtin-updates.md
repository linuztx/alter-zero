# Built-in updates — a release reaches the skills and agent types it changed

The five built-in skills and the two built-in agent types are files on disk:
a launch writes each one into `{config_home}/skills` or the agents directory
when it is absent, so a default is something the user can read, edit and diff
(`docs/skills.md`, `docs/subagents.md`). Until now that write was the *only*
one — a file already there was never touched again. That is what kept an
edited copy safe, and it is also why an improvement never arrived: a release
that rewrote `web-search`'s scripts reached every new install and no existing
one, and the only way to take it was to delete the folder and let the next
launch seed it again.

The seed now keeps them current. At every launch, before the walks read them:

| on disk | what happens |
| --- | --- |
| nothing | written — the seed as before, so a deleted file comes back |
| this release's version | nothing |
| an older version some release wrote, unedited | **replaced** with this release's version |
| anything else — the user's edit | **kept**, never written over; when this release changed the file, its new version is written **beside it** as `<file>.new`, once, and a card says so |
| a file this release no longer ships, unedited | **removed** — unless a file of the same built-in was edited, or it is a link |

## Telling ours from theirs: the shipped history

A copy is *ours* when its bytes are exactly bytes a release wrote.
`prompts/shipped.sha256` lists every version of every built-in file the binary
has carried, one `sha256sum` line each, and is embedded beside the files
themselves. A file on disk whose SHA-256 is listed for its path was written by
this program and edited by nobody, so it may be replaced; anything else is the
user's, and is left alone.

The rule cannot tell an untouched copy from one put back to an older release's
version on purpose: both are bytes a release wrote, so the next launch brings
either up to date. To keep an older version, change any byte of it — a blank
line will do — and it is the user's from then on.

That rule needs no record of what an install did, which is what makes it work
for every install that exists today: a v0.14 install's `web-search` scripts
carry v0.14's bytes, those bytes are listed, and the first launch of a newer
release recognises them and replaces them. The alternative — a manifest
written at seed time and trusted alone — knows nothing of the files seeded
before it existed, which are exactly the files the first update has to reach.

**Keeping it complete is a test's job.** `every_built_in_file_is_in_the_shipped_history`
(in `llm::skill` and in `llm::subagent`) fails when a built-in file's current
bytes are not listed, and names the command that fixes it — every changed
file in one line, so a skill's prompt and scripts changed together are
recorded at once:

```bash
sha256sum prompts/skills/web-search/SKILL.md >> prompts/shipped.sha256
```

Lines are only ever **appended**. An install written by an old release is
recognised only while that release's version is listed; removing a line turns
every such copy into "an edit" — kept for good and never updated again. A line
for a version no release shipped (a change revised again before it was
released) costs nothing and need not be taken out.

The file has three sections, in order:

- **The first line of each path** is the version every release up to v0.19.0
  shipped. No built-in file changed between releases until then (checked
  against every tag), so it is the version on disk in every install that
  predates the history — and the base assumed for an edited copy that has no
  record (below). Keep it first.
- **The development versions**: every other version a commit in `main`'s
  history carried before the history existed, which only a development build
  could have written. Listed so a copy a `cargo run` seeded is recognised too.
- **Every version since**, under the last heading, where the `sha256sum …
  >>` above lands it: one line per change, newest last.

## An edited copy, and its `.new`

An edited file is never written over and never deleted. What is worth telling
the user is narrower: that the release changed the file they edited, so the
two have diverged. For that the seed has to know which version the edit
started from — and that is a record, `.alter-zero-builtins.json` in the
seeded directory, named for its owner since an `ALTER_ZERO_AGENTS_DIR`
override can be anyone's directory:

```json
{
  "files": {
    "web-search/SKILL.md": {
      "base": "<sha256 of the version this copy is based on>",
      "offered": "<sha256 of the version written beside it as SKILL.md.new>"
    }
  }
}
```

Each file's **base** is the version last written there, or last found there
unedited. When a launch finds a file edited and its base is not this
release's version, the release changed it since: the seed writes this
release's version beside it as `SKILL.md.new`, says so in a card, and moves
the base forward — so the card is shown once per change, and a `.new` the
user deleted after merging is not written again. A base this binary does not
know was written by a newer release (the user went back to an older one), and
offers nothing: nothing is ever replaced by an older version, and the `.new`
the newer release wrote stays as it is.

An edited file with no record — an install from before the record, or a
record the user deleted — takes its path's **first** history line as its
base: the version every release before the record shipped.

The record says only what the user was offered; whether a file may be
replaced is the history's to say. That is why losing the record loses no
update — every unedited file is still recognised by its bytes, and its record
written again. A record trusted alone makes the opposite mistake: once it is
gone (deleted in a cleanup, never copied to a second machine), every untouched
file reads as an edit, kept for good and never updated again.

### Whose `.new` it is

The **offered** hash is what makes a `.new` this program's. One holding the
version the record says was offered there, or this release's own, is ours: a
newer offer replaces it, and once its file holds a version a release wrote
again (the user copied the `.new` over it, or deleted the file and let the
seed write the new one), it is removed with nothing lost.

Anything else at that path is the user's — a `.new` they opened and started
merging into, one they made themselves, a link — and is never written over or
removed. A new offer waits until it is out of the way, the record left as it
was so the launch after that makes it; and a `.new` no offer wrote is never
tidied away, whatever it holds. Neither walk reads a `.new` (a skill folder's
`SKILL.md` alone, the agents directory's `*.md` alone — `explore.md.new` is
not one).

### Only a launch that can show it offers

The offer is made once, so it is made only by a launch that can show its
card. A headless run (`-p`) makes none: it seeds, replaces, retires and tidies
like any launch, but leaves an edited copy and its record exactly as it found
them, so the next interactive launch makes the offer and says so. Writing the
`.new` from a script's `-p` and recording it as offered would leave the user a
file nobody told them about.

## Per file — and retirements per built-in

Each file is decided on its own: a `web-search` whose `SKILL.md` the user tuned
still gets this release's scripts, and gets `SKILL.md.new` to merge from. The
one decision taken per built-in is removing a file a release stopped
shipping: it goes only when every file of its built-in — the skill's folder,
or the agent's one file — is as a release wrote it. A `SKILL.md` the user
edited may run the script that was retired, and a removed file cannot be put
back from the binary, which carries only the hashes of the versions it no
longer ships. A folder left empty by removals is removed with them.

A built-in path that is a symbolic link is read and written **through** — a
dotfiles checkout keeps its link and still gets the update — but never
removed: the seed writes files, never links, so a link is the user's
arrangement, and a retired file reached through one stays.

A file the user added to a built-in's folder is never looked at: the history
names the files the seed owns, and nothing else is touched.

## What the user sees

- **A copy brought up to date** — one dim toast, `Updated the built-in
  web-search skill`, raised before the startup's other reports so any error
  among them covers it.
- **An edited copy kept** — an `Edited built-ins` card naming each `.new`
  written and the two ways on: merge from it, or delete the copy and the next
  launch writes the new version. It is committed under the banner after the
  first frame, like the telemetry and update cards, once the screen can take
  it: while anything else holds the screen — the `/resume` picker a bare
  `--resume` opens on, another overlay, a turn in flight, an agent's session
  view — it waits, and lands at the first loop bottom that can, once. (A
  `[PROMPT]`'s turn starts after it, so its bubble lands under the card.) The
  `.new` is written at launch; only the card waits, since a card committed
  under the picker would be gone with the rebuild the pick makes.
- A first launch, a restored file and a release that changed nothing say
  nothing.

A headless run (`-p`) shows no card and makes no offer (above); a copy it
brought up to date is a step-log line on stderr under `--verbose`
(`docs/headless.md`).

## Writes

Every write goes to a temp file beside its target, synced, then renamed over
it, so a crash mid-write leaves the old file or the new one — never a torn
file, which would read as an edit for good. A file being replaced keeps its
mode (a script the user made executable stays so), and a symlinked file is
written through, so a dotfiles checkout keeps its link. Two launches at once
compute the same plan and write the same bytes.

`.alter-zero-builtins.json` is written last, and only when it changed. A step
that failed keeps its file's old record, so the next launch tries it again,
and every failure is a red startup toast — never a failed launch.
`ALTER_ZERO_SKILLS_DIR` is still never written into; an
`ALTER_ZERO_AGENTS_DIR` override is seeded and kept current like the config
home's directory, its record included.

## Where it lives

| file | role |
| --- | --- |
| `prompts/shipped.sha256` | the shipped history |
| `src/builtins.rs` | the history and `.alter-zero-builtins.json` formats, the per-launch plan (pure, over an injected read), the executor that carries it out, and the toast's and card's wording |
| `src/llm/skill.rs` | `BUILTIN_SKILL_FILES` and `sync_builtin_skills` |
| `src/llm/subagent.rs` | the agents' `BUILTIN_FILES` and `sync_builtin_agents` |
| `src/tui/bootstrap.rs` | both syncs before the walks (offering only off `-p`), the toast, and the card once the screen can take it (`Session::flush_builtin_offers`) |

`smoke.sh` Phase 165 plays an install an older build made against the real
binary, once for each way a release can first be launched. Interactive: the
unedited `explore.md` it seeded replaced and the toast saying so, an edited
`skill-creator/SKILL.md` kept with its `.new` and the card, and a second
launch — the `.new` deleted — quiet and writing nothing back. Headless first:
the agent brought up to date, no `.new` written, and the interactive launch
after it making the offer. A bare `--resume` first: the card landing once the
picked conversation is back on screen. The older `explore.md` it plants is a
development version the history lists, byte for byte. Retirement and a `.new`
mid-merge are `builtins::tests`' alone: no built-in file has been retired yet,
so the real history names none to plant.
