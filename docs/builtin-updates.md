# Built-in files follow the release

The built-in skills (`docs/skills.md`) and agent definitions
(`docs/subagents.md`) are real files in the user's config home, so they can
be read, edited and diffed. They used to be written **once**: the seed
skipped any file that was already there, so an install made on one release
kept that release's `web-search` scripts and `explore` prompt forever. A
provider changing its API, a description reworded so it triggers, a script
fixed — none of it reached anyone who had already launched the app.

Now one rule decides, per file:

> **A built-in file you have not edited follows the release. A file you
> edited is yours.**

```
~/.alter-zero/skills/web-search/SKILL.md               untouched → replaced by the new release's
~/.alter-zero/skills/web-search/scripts/exa_search.py  edited    → kept, and you are told once
~/.alter-zero/skills/.builtin.json                     what the app wrote, so it can tell the two apart
```

It runs where the seed always ran: at startup, before the walk, so the
session that follows an update already has the new files.

## Which folders

Only the two the app owns: `{config_home}/skills` and `{config_home}/agents`.

An override is somebody else's folder. `ALTER_ZERO_SKILLS_DIR` is still never
written into at all. `ALTER_ZERO_AGENTS_DIR` is still **seeded** — the
`agent` tool's default type has to resolve — and nothing more: an absent
definition is written, a present one is never replaced, nothing is removed,
and no manifest is left there (`Mode::SeedOnly`). A directory a team commits
its agents to must not be rewritten by whoever runs the newest build.

A project's `.alter-zero/skills`, `.claude/skills` and `~/.claude/skills` are
the user's alone, as before.

## Telling an untouched file from an edited one

"Was this file edited?" has no answer in the file itself, so each folder
keeps a **manifest** of what the app put there — `{root}/.builtin.json`:

```json
{
  "schema": 1,
  "scope": "skills",
  "version": "0.20.0",
  "files": {
    "web-search/SKILL.md": "2e236c135fa7…",
    "web-search/scripts/exa_search.py": "58e05587c927…"
  }
}
```

`files` maps each built-in file, by its path under the root, to the SHA-256
of the built-in version this root last **settled against**: the bytes the app
wrote, found already in place, or declined to write over the user's own.
`version` is the app version that settled the root, `scope` says which set of
built-ins it describes, and `schema` is the format. A file on disk whose
digest equals its record is exactly what the app left there, so it is safe to
replace; any other digest is somebody's edit.

The manifest lives **in the folder it describes**, not beside `config.json`,
so it travels with the files when a config home moves or a `skills/` folder
is synced between machines. Neither walk reads it: skills are
`<root>/<name>/SKILL.md` and agents are `<root>/<name>.md`.

What is found at the manifest's path decides what the launch may do:

| What is read | Meaning | What the launch does |
| --- | --- | --- |
| nothing, anything that is not a JSON object, or a manifest of this format that will not parse | no manifest — a first settle, or machine state that was lost | settles as if for the first time, and writes a good one |
| a manifest of a newer `schema`, a newer `version`, or another `scope` | somebody else is in charge of this folder | **nothing at all** — see *Deferring to a newer app* |
| something at the manifest's path that is not a regular file — a link, a directory, a FIFO — or a regular file over `MAX_FILE_BYTES` (1 MiB) | nobody can say who is in charge | **nothing at all**, silently, rather than settle afresh and take every file for the user's |
| a manifest that cannot be looked at or read at all — the root itself a regular file, a folder without permission, a disk error | the folder is broken | **nothing at all**, with an error naming the manifest's path: deferred as silently as a link, a broken root would never be mentioned again |
| anything else | ours | the decision below |

The `schema` is read on its own first (`Manifest::read`), so a manifest a
later release reshaped is recognised as *newer*, never mistaken for garbage
and written over — which would turn every file that release wrote into an
"edit" the moment it ran again. Keys this binary does not know are kept, in
the order they were written, when it writes the manifest back.

### Installs older than the manifest

Every install made before this feature has the files and no manifest, and
those are the installs the feature exists for. For them the binary carries a
**legacy table**, `prompts/builtin-legacy.sha256`: the digest of every
version of every built-in file the history it was generated from ever
carried, as `sha256sum` lines over the installed path.

```
58e05587c9277efe…  skills/web-search/scripts/exa_search.py
c280435f80aeabe1…  skills/web-search/scripts/exa_search.py
9147f347f5065ed2…  agents/explore.md
```

The table is asked on a folder's **first settle** only — no manifest yet, or
one that holds no record. There, a file whose digest is listed for its path
is a built-in nobody edited, and is replaced like any other. What the settle
decides is recorded — a file written, found current, or kept and named — and
once the manifest holds any record, the table is never asked again: someone
who later puts an old version back on purpose has edited the file, and it
stays. Not every file gets a record, though. A link or anything else that is
not a regular file gets none, nor does a seed that failed, nor a kept file
in a launch that cannot name it (*What the user is told*); a settle that
recorded nothing at all is followed by another first settle, which asks the
table again. A file of the app's own that the settle leaves for a later
launch — its update or removal failed, or its retirement waits — keeps a
record of the bytes it holds instead, so the next launch, no longer a first
settle, still knows it (*Writing*).

The table is generated from history by `scripts/builtin-legacy.sh` and is
**frozen** by design: a release that keeps a manifest records what it writes,
so nothing shipped after this feature ever needs a line there. It only has to
cover what manifest-less releases wrote, which is why it is regenerated only
if a built-in file changes before the first release that carries this
feature.

That "only if" is not left to memory. `scripts/builtin-legacy.sh --check`
reads every release tag whose tree has no table — a release made before this
feature — and fails, naming each line, unless every built-in file that
release installed is listed; CI runs it in the release-tooling job, over a
checkout with every tag (`fetch-depth: 0`). A table missing a release's
version fails nowhere else: those installs would take their untouched copies
for the user's edits, say so once, and never update them — the exact failure
the table exists to prevent. Once releases carry the table, every older tag
has already been checked, so the check keeps passing with no upkeep; it
refuses (exit 2) on a shallow clone or a clone with no tags rather than pass
having checked nothing.

A table of digests that had to grow with every edit to a skill was the
alternative (no manifest at all), and was refused: it turns every change
under `prompts/skills/` into a second edit that a test has to demand, and a
version missing from it fails silently as "looks edited".

## The decision

One pure function, `builtin::verdict`, over what is at the file's path
(`Disk`), the digest the binary ships, the file's record, and whether the
legacy table knows the copy on disk.

| At the path | Verdict | What happens | Record after |
| --- | --- | --- | --- |
| a path that runs through a linked folder, something that is not a regular file — a symlink, a directory, a FIFO — or a file that cannot be read | `Leave` | nothing | unchanged |
| nothing | `Seed` | written | shipped |
| the shipped bytes | `Current` | nothing | shipped |
| bytes equal to the record | `Update` | replaced | shipped |
| no record, and — on a first settle — bytes the legacy table knows | `Update` | replaced | shipped |
| anything else, and the record is not the shipped digest | `Kept { news: true }` | kept, reported | shipped — unchanged in a launch that cannot show the row |
| anything else, and the record is the shipped digest | `Kept { news: false }` | kept, silent | unchanged |

The rows are tried in that order. `news` is exactly "the record is not what
the binary ships": a release changed the file since this root settled, or the
root has never settled at all — an install older than the manifest, whose
edited files are therefore named once on the first launch that has one.

A regular file standing where one of the file's *folders* must be is not a
`Leave`: nothing can be at the path, so it is a `Seed`, and the seed fails
and says so, as the old seed's did. Taken for something at the path, as a
link somebody manages is, it would be left without a word, and the built-in
would quietly never arrive.

Three properties fall out of the table:

- **An edit is never overwritten.** The only way to `Update` is a digest the
  app itself produced.
- **A kept file is reported once per built-in version**, not once per launch:
  settling records the shipped digest, so the next launch is silent until a
  release changes that file again. The launch that settles it is the first
  that can both show the row and write the manifest (*What the user is
  told*).
- **A deleted file comes back** at the next launch in charge of the folder,
  as it always did — not at one that defers to a newer app, which touches
  nothing (*Deferring to a newer app*). Deleting your version of a file is
  therefore how you take the built-in.

What is at a path is found without trusting it. A **symlink is never written
through** — the file itself, or a folder on the way to it below the root (a
skill folder a dotfiles manager linked into its repository): a link is
somebody managing those files, and the app leaves them, silently. The root
being a link is not that; a config home kept elsewhere is still the app's.
Only a regular file is read, and only up to `MAX_FILE_BYTES` (1 MiB, far
above any built-in): a larger one is kept as the user's without being read,
and a FIFO or a device at a built-in path can no longer stall the launch the
way opening it would.

The decision is **per file**, not per skill. A skill whose `SKILL.md` you
reworded still gets a fixed script, which is the point: an all-or-nothing
rule would freeze five scripts over one changed sentence, and say nothing.
The other direction — a new body over a script you edited — can name a flag
your script lacks; the script's own usage error says so, and the launch that
kept your file told you which one it was.

### Files a release no longer ships

A path on record that the binary does not carry is **retired**: a renamed
script, a built-in skill that was withdrawn. Deleting is the one thing here
that cannot be taken back, so it is decided for the whole **unit** — a skill's
folder, an agent's file — never file by file:

> **A retired file is removed only when no built-in file of its unit is the
> user's — and, for a unit the release withdrew whole, nothing else is in its
> folder.**

`builtin::retired_unit` decides it, over the state of each retired file
(`builtin::retired`) and what became of the unit's shipped files this launch.
The unit's retired files are removed (`RetiredUnit::Remove`) when all of
these hold:

- the retired file itself is a regular file whose bytes equal its record —
  or, with no record on a first settle, a version the legacy table lists;
- every other retired file of the unit is the app's own too, or already
  gone;
- none of the unit's shipped files is the user's this launch — kept as their
  version, or left because it is a link;
- none of them failed to be written this launch;
- and, for a unit the binary no longer ships at all, its folder holds nothing
  but those files (a `.DS_Store` is Finder's, not the user's work, and does
  not count). A skill the release still ships may hold files of the user's
  own beside its built-ins: those do not hold its retired files back.

When something in the unit is the user's — a retired file of theirs, a
shipped one kept or left, or anything else in a withdrawn unit's folder —
every retired file of the unit **stays** and its records are dropped
(`RetiredUnit::Forget`): the unit is the user's now. Removing only the
untouched half would have taken the `SKILL.md` out from under a script the
user had changed, and a kept `SKILL.md` still names the script a release
renamed. Each retired file that stays this way is named once, in a row of
its own (*What the user is told*), when it is a regular file; a link, or
anything else that is not one, is left silently, as one at a shipped path
is.

The user's comes first: a failed write in a unit that is theirs anyway
changes nothing. But a shipped file that could not be written is not the
user's on its own, and ends differently (`RetiredUnit::Retry`): nothing in
the unit is removed this launch — the new `SKILL.md` may not have landed,
and the old one still names the script — and every record stays, one only
the legacy table knew recorded by the bytes it holds, so the next launch
decides the unit again. Taken for the user's, its retired files would lose
their records, read as the user's at the next launch, and stay for good. A
retired path with nothing at it just loses its record.

A removal unlinks the file, then each directory it leaves empty, walking up
the path as written and stopping below the root (`remove_dir`, so a folder
with anything in it stops the walk). A symlink is never followed and never
removed. A record's path is acted on only if it is relative and made of plain
names (`builtin::is_safe_path`), since a manifest is a file anyone can write
and a record must never reach outside its root; and a retired path that
differs from a shipped one only in letter case is forgotten, not removed, or
a case-insensitive filesystem would delete the file just written.

The legacy table retires a file on that same **first settle** and never
after — an install that jumps straight from a manifest-less release to one
that withdrew a skill. After that, a withdrawn built-in that reappears with
no record was put back by someone — a release from before this feature, say
(*Limits*) — and stays.

### Deferring to a newer app

Two versions can share one config home: a release beside a development
build, two machines syncing dotfiles, containers on one volume. If each made
the folder match what it carries, they would rewrite each other's files on
every launch — and an older one would re-seed whatever a newer one retired.

So a binary that finds a manifest **newer than itself** — a later `version`
(`update::parse_version`'s ordering, a pre-release below its release) or a
later `schema` — does nothing at all in that folder: no seed, no update, no
removal, no report, and the manifest is not rewritten (`Manifest::read` →
`Read::Deferred`). Among the binaries that carry this feature, an older one
never undoes a newer one: the folder converges on the newest app and stays
there. A release from before the feature (v0.19.0 and earlier) does not read
`.builtin.json` at all and still writes whatever built-in it carries that is
missing, so it can bring back a file a newer build retired (*Limits*). A
manifest whose `version` is missing or unreadable does not outrank anything.
The same deference covers a manifest of another `scope`, which only one
directory serving as both roots could produce.

Two builds of the **same** version with different files (a checkout with a
skill under edit, beside the release it was cut from) have no order, so each
does make the folder match itself. That is confined to a developer's own
machine, loses nothing — only untouched files move — and
`ALTER_ZERO_CONFIG_DIR` keeps a checkout out of the real config home.

## Writing

Every file is written through a temp file beside its target and a rename —
a seed as much as an update. A plain write that dies halfway (a full disk, a
kill) leaves a truncated file with no record, and the next launch would take
it for an edit and keep it broken for good. The temp name is unique per
write, so two launches starting together never rename each other's
half-written file into place, and it does not end in `.md`, so a leftover
one is never read as an agent definition.

The rename is what makes a killed write harmless — it leaves a stray temp
file, never a truncated one — so the flush before it (`sync_all`) is
best-effort. On macOS that flush is `F_FULLFSYNC`, which some network
filesystems (SMB) refuse; treated as a failure, it would fail every seed and
update there, where the old seed's plain write always worked.

A seeded file gets the mode a new file gets (no executable bit, the umask's).
An update **keeps the mode the file had**: a script the user made executable
stays executable. A script another session is running at that moment is
never read half-written.

A write that failed is never recorded as done, and the manifest itself is
written the same way, last, and only when it changed — an ordinary launch
writes nothing at all. A crash between a file and the manifest heals itself:
the next launch finds the shipped bytes and records them.

Failures are collected, never thrown, as the seed's were: a read-only config
home costs the update, not the session. A built-in file that could not be
written or removed is an error at each launch that has that to do — what a
failed seed always was — and so is a manifest that cannot be read at all
(*Telling an untouched file from an edited one*). A manifest that cannot be
**written** is silent: a skills folder whose files are all current but
which the app cannot write to (a read-only mount, a folder a Nix or dotfiles
setup links in) said nothing before this feature, and an error there would
come back at every launch. What it costs is the record that tells a kept or
retained file only once, so that launch tells neither
(`Report::withdraw_unsettled`), and they wait for the first launch that can
write the manifest; what it did write or remove is still told.

A failure is always tried again. A file of the app's own whose update or
removal failed keeps its record — or, when only the legacy table knew it,
gets a record of the bytes it holds — so the launch after a failed first
settle still knows it for the app's own; a unit whose shipped file could not
be written keeps its retired files on record the same way, for the next
launch to retire (*Files a release no longer ships*). A removal that finds
the file already gone — two launches retiring one skill — is not a failure.

## What the user is told

A launch with nothing to tell says nothing. One that has something commits
a few dim rows under the banner — chrome like the banner, never in
`history`:

```
  Added built-in skill: flaredantic
  Updated built-in skills: web-search, yt-dlp
  Removed built-in skill no longer shipped: old-skill
  Kept your version of built-in skill file ~/.alter-zero/skills/web-search/SKILL.md (delete it to get the built-in's)
  Kept built-in skill file no longer shipped, since the skill has your changes: ~/.alter-zero/skills/web-search/scripts/old.py
  Updated built-in agent: explore
```

`builtin::notices` writes them from the launch's `Report`, the same five
sentences for either root with `skill` or `agent` as the noun (`Kind`), each
in the singular for one name and the plural for several. Skills and agents
are named by **unit** (`builtin::unit` — the skill's folder, the agent's
file stem), a file by **where it is**, since that is the thing to delete:
the root as the user is shown it (`ui::display_cwd` — `~/.alter-zero/skills`
under the home, the absolute path elsewhere), then the file's path under
it. Units and files are named sorted and once each, never in the order the
binary embeds them. A row longer than the terminal wraps, never cut.

The last of a root's rows names each retired file that stays because its
unit is the user's (`Report::retained`, *Files a release no longer ships*) —
often there because of the kept file named just above it: `since the skill
has your changes`, or `the skills have` when the files belong to several.

"Your version" rather than "your edit", because the file may never have
been a built-in: a skill of your own that has the name of one a later
release adds is kept the same way, and told the same thing. Rows under the
banner rather than a toast, because this happens once per update and a
toast is gone in four seconds, under whichever other toast the launch
raised.

Four kinds of absent file are written, and two of them are news. They are
told apart by what the root held **before** anything was written:

| The file that was seeded | Reported |
| --- | --- |
| has a record — it was deleted, and is coming back | nothing |
| has none, in a root with no built-in at all — a fresh install | nothing |
| has none, and its unit was not there — a built-in new in this release | `Added` |
| has none, and its unit was there — a skill that gained a file | `Updated` |

A unit "was there" when any of its files was on disk or on record. A unit
that lost a retired file but still ships others was updated, not removed;
`Removed` names only a unit the binary no longer carries at all.

**Only a launch that can show the rows settles them.** A kept or retained
file is told once because telling it settles it — the kept file's record
moves to the shipped digest, the retained file's record goes — so a launch
that cannot show the rows leaves those records where they were
(`Mode::Follow { announce: false }`), recording the bytes of a retired file
of the app's own that only the legacy table knew, and the next launch that
can still names them. Two cannot:

- A **headless run** (`-p`) has no banner. It updates and removes like any
  launch and writes the sentences to its step log (`--verbose`).
- A **bare `--resume`** boots into the `/resume` picker, and a pick
  purge-rebuilds the screen, taking the rows queued under the banner with
  it. It updates and removes like any launch too; on a pick its added,
  updated and removed rows are lost with the rest (the files are still
  updated), and a dismissed picker's return shows them.

Otherwise a scripted run, or a session picked straight after an update,
would use up the one notice nobody was there to read. A launch whose
manifest cannot be written tells no kept or retained file either, for the
same reason (*Writing*).

## What did not change

- `ALTER_ZERO_SKILLS_DIR` is still never seeded into, and
  `ALTER_ZERO_AGENTS_DIR` is still only seeded.
- `/skills` is still how a built-in skill is turned off. Deleting its folder
  lasts until the next launch in charge of the folder brings it back.
- The agent types' embedded fallback (`with_builtins`) still backs up a root
  that could not be written, or one a newer app is in charge of.
- The walk still re-runs every turn; a launch's update runs **once**, at
  startup, and nothing in the session re-settles the folders after it.
  Another launch can, though: a newer binary started in another terminal
  updates the shared folders, and this session's next walk picks up the new
  files (a skill body already loaded stays as it was read — *Limits*).

## Limits

- A file put back to an *older* built-in after the folder's first settle is
  an edit, and is kept. Made byte-identical to the *current* built-in, it is
  simply current again.
- A manifest lost after this feature (a `skills/` folder copied without its
  dotfiles) leaves post-feature versions unrecognised: they are kept, and
  reported once as the user's. Deleting the file restores the built-in.
- A folder a newer app settled is left alone **silently**, including the
  files a pre-release wrote before the user went back to the stable build:
  they stay until a release at least as new runs.
- A release from before this feature (v0.19.0 and earlier) ignores
  `.builtin.json` and writes any built-in it carries that is missing — the
  old seed's rule — so run on the same config home, it can re-create a file
  a newer build retired, or write its own older version of one deleted
  since. Back on the newer build, neither is the app's: the re-created file
  has no record, and the folder is past its first settle, so the legacy
  table is not asked — it stays, unnamed, as a file of the user's would;
  the older version is an edit, and is kept (the first bullet). Deleting the
  file is how to be rid of it, or to take the built-in.
- On a folder's first settle, a launch that cannot show the rows records
  nothing for a retired file whose bytes the legacy table does not know — a
  withdrawn built-in the user changed — since a record of the user's bytes
  would make them the app's. The next launch, past its first settle, no
  longer looks for it: the file stays, but is never named.
- A skill of your own under a name a later release ships keeps its own
  files, and the built-in's other files are written beside them, as the seed
  always did. The launch names the files that were kept.
- A skill body already loaded into a conversation is not reloaded when its
  file is replaced (`LoadedSkills`, `docs/skills.md`); a resumed session
  keeps the instructions it read.

## Module layout

| Where | What |
| --- | --- |
| `src/builtin.rs` | **pure**: `digest`, `Manifest` and `Manifest::read` → `Read`, `Legacy` and the embedded `LEGACY_TABLE`, `Disk`, `verdict`/`Verdict`, `retired`/`Retired` and `retired_unit`/`RetiredUnit`, `is_safe_path`, `Report` and `Report::withdraw_unsettled`, `Kind`, `unit`, `notices` |
| `src/llm/builtin.rs` | the boundary: read the manifest (an error when it cannot be read at all), look at what is on disk without following a link, apply each verdict (write through a temp file, remove), write the manifest back (withdrawing the kept and retained rows when it cannot be written); `Mode` |
| `src/llm/skill.rs`, `src/llm/subagent.rs` | `seed_builtin_skills` / `seed_default_agents` hand their embedded files to it; `builtin_skill_notices` / `default_agent_notices` word a report under the root as the user is shown it |
| `prompts/builtin-legacy.sha256` | the legacy table, embedded |
| `scripts/builtin-legacy.sh` | generates it from git history; `--check` holds it to every release made before the manifest (CI) |
| `src/tui/bootstrap.rs` | runs both seeds before the walks — with `announce` off in a headless run and a bare `--resume` — names each root by `ui::display_cwd`, and commits the notices after the first frame |

The pure half takes digests and returns verdicts, so every row of every table
above is a unit test with no file anywhere; the boundary's tests run against
a temp directory with a legacy table of their own. `smoke.sh` Phase 102b
drives real launches over a config home staged one release behind: an
outdated built-in replaced while an edited script beside it is kept, kept
files named by where they are, a withdrawn skill removed and the retired
files of units that are the user's kept and named, a symlink left alone, a
bare `--resume` that settles nothing it is told once, a folder a newer app
settled, and an override folder that is only seeded.
