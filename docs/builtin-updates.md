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
| nothing, text that is not a JSON object, or a manifest of this format that will not parse | no manifest — a first settle, or machine state that was lost | settles as if for the first time, and writes a good one |
| a manifest of a newer `schema`, a newer `version`, or another `scope` | somebody else is in charge of this folder | **nothing at all** — see *An older binary never undoes a newer one* |
| something at the manifest's path that is not a readable regular file — a link, a directory | nobody can say who is in charge | **nothing at all**, rather than settle afresh and take every file for the user's |
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
is a built-in nobody edited, and is replaced like any other; every file the
launch looks at then gets a record, and the table is never asked again.
Someone who later puts an old version back on purpose has edited the file,
and it stays.

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
| a path that runs through a linked folder, or something that is not a regular file — a symlink, a directory, a FIFO | `Leave` | nothing | unchanged |
| nothing | `Seed` | written | shipped |
| the shipped bytes | `Current` | nothing | shipped |
| bytes equal to the record | `Update` | replaced | shipped |
| no record, and — on a first settle — bytes the legacy table knows | `Update` | replaced | shipped |
| anything else, and the record is not the shipped digest | `Kept { news: true }` | kept, reported | shipped |
| anything else, and the record is the shipped digest | `Kept { news: false }` | kept, silent | unchanged |

The rows are tried in that order. `news` is exactly "the record is not what
the binary ships": a release changed the file since this root settled, or the
root has never settled at all — an install older than the manifest, whose
edited files are therefore named once on the first launch that has one.
Three properties fall out of it:

- **An edit is never overwritten.** The only way to `Update` is a digest the
  app itself produced.
- **A kept file is reported once per built-in version**, not once per launch:
  settling records the shipped digest, so the next launch is silent until a
  release changes that file again.
- **A deleted file comes back**, as it always did. Deleting your version of a
  file is therefore how you take the built-in.

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

> **A retired file is removed only when nothing in its unit is the user's.**

That is all of:

- the retired file itself is a regular file whose bytes equal its record
  (`builtin::retired`);
- every other retired file of the unit is untouched too;
- none of the unit's shipped files is the user's this launch — kept as their
  version, left because it is a link, or not written because the write
  failed;
- and, for a unit the binary no longer ships at all, its folder holds nothing
  but those untouched files (a `.DS_Store` is Finder's, not the user's work,
  and does not count).

Otherwise every retired file of the unit **stays** and its records are
dropped: the unit is the user's now. Removing only the untouched half would
have taken the `SKILL.md` out from under a script the user had changed, and a
kept `SKILL.md` — or one whose update could not be written — still names the
script a release renamed.

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
no record was put back by someone, and stays.

### An older binary never undoes a newer one

Two versions can share one config home: a release beside a development
build, two machines syncing dotfiles, containers on one volume. If each made
the folder match what it carries, they would rewrite each other's files on
every launch — and an older one would re-seed whatever a newer one retired.

So a binary that finds a manifest **newer than itself** — a later `version`
(`update::parse_version`'s ordering) or a later `schema` — does nothing at
all in that folder: no seed, no update, no removal, no report, and the
manifest is not rewritten (`Manifest::read` → `Read::Deferred`). The folder
converges on the newest app and stays there. A manifest whose `version` is
missing or unreadable does not outrank anything. The same deference covers a
manifest of another `scope`, which only one directory serving as both roots
could produce.

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

A seeded file gets the mode a new file gets (no executable bit, the umask's).
An update **keeps the mode the file had**: a script the user made executable
stays executable. A script another session is running at that moment is
never read half-written.

Nothing is recorded for a write that failed, and the manifest itself is
written the same way, last, and only when it changed — an ordinary launch
writes nothing at all. A crash between a file and the manifest heals itself:
the next launch finds the shipped bytes and records them.

Failures are collected, never thrown, as the seed's were: a read-only config
home costs the update, not the session. It says so at each launch that has
something to write, which is what a failed seed always did. A failure is
always tried again: a file the legacy table recognised keeps a record of the
bytes it holds, so the launch after a failed first settle still knows it for
the app's own. A removal that finds the file already gone — two launches
retiring one skill — is not a failure.

## What the user is told

A launch that changed nothing says nothing. One that did commits a few dim
rows under the banner — chrome like the banner, never in `history`:

```
  Added built-in skills: flaredantic
  Updated built-in skills: web-search, yt-dlp
  Removed built-in skills no longer shipped: old-skill
  Kept your version of built-in skill file web-search/SKILL.md (delete it to get the built-in's)
  Updated built-in agent: explore
```

`builtin::notices` writes them from the launch's `Report`, the same four
sentences for either root with `skill` or `agent` as the noun (`Kind`).
Skills and agents are named by **unit** (`builtin::unit` — the skill's
folder, the agent's file stem), a kept file by its **path**, since that is
the thing to delete. "Your version" rather than "your edit", because the
file may never have been a built-in: a skill of your own that has the name
of one a later release adds is kept the same way, and told the same thing.
Rows under the banner rather than a toast, because this happens once per
update and a toast is gone in four seconds, under whichever other toast the
launch raised.

Three launches write an absent file, and only one of them is news. They are
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

**Only a launch that can show the rows settles them.** A headless run (`-p`)
has no banner: it updates and removes like any launch and writes the
sentences to its step log (`--verbose`), but it leaves a kept file's record
where it was (`Mode::Follow { announce: false }`), so the next interactive
launch still says which file was kept. Otherwise a scripted run after an
update would use up the one notice nobody was there to read.

## What did not change

- `ALTER_ZERO_SKILLS_DIR` is still never seeded into, and
  `ALTER_ZERO_AGENTS_DIR` is still only seeded.
- `/skills` is still how a built-in skill is turned off. Deleting its folder
  lasts until the next launch.
- The agent types' embedded fallback (`with_builtins`) still backs up a root
  that could not be written, or one a newer app is in charge of.
- The walk still re-runs every turn; the update runs **once**, at startup. A
  release cannot change under a running session.

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
- A skill of your own under a name a later release ships keeps its own
  files, and the built-in's other files are written beside them, as the seed
  always did. The launch names the files that were kept.
- A skill body already loaded into a conversation is not reloaded when its
  file is replaced (`LoadedSkills`, `docs/skills.md`); a resumed session
  keeps the instructions it read.

## Module layout

| Where | What |
| --- | --- |
| `src/builtin.rs` | **pure**: `digest`, `Manifest` and `Manifest::read` → `Read`, `Legacy` and the embedded `LEGACY_TABLE`, `Disk`, `verdict`/`Verdict`, `retired`/`Retired` and `unit_retires`, `is_safe_path`, `Report`, `Kind`, `unit`, `notices` |
| `src/llm/builtin.rs` | the boundary: read the manifest, look at what is on disk without following a link, apply each verdict (write through a temp file, remove), write the manifest back; `Mode` |
| `src/llm/skill.rs`, `src/llm/subagent.rs` | `seed_builtin_skills` / `seed_default_agents` hand their embedded files to it |
| `prompts/builtin-legacy.sha256` | the legacy table, embedded |
| `scripts/builtin-legacy.sh` | generates it from git history; `--check` holds it to every release made before the manifest (CI) |
| `src/tui/bootstrap.rs` | runs both seeds before the walks, commits the notices after the first frame |

The pure half takes digests and returns verdicts, so every row of every table
above is a unit test with no file anywhere; the boundary's tests run against
a temp directory with a legacy table of their own. `smoke.sh` Phase 102b
drives real launches over a config home holding an outdated built-in, a kept
one and a retired one.
