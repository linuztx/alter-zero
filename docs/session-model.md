# Per-session model — a conversation remembers the model it ran on

The `/model` choice is kept **per working directory**
(`docs/per-directory-state.md`): `config.json` holds one entry per cwd, and
every session launched there starts on it. That answered "which model does
this *project* use?" and left a second question unanswered — which model
does this *conversation* use? A directory can hold two alter-zero instances
at once, one on a frontier model beside one on a cheap local one, and a
resumed conversation was produced by a particular model whatever the
directory's entry says today. Both used to lose to the directory:

- Two instances in the same directory shared one `config.json` entry. Each
  ran the model it had in memory, but a `/model` pick in either one moved
  the shared entry, so the other's *next* launch — or its `--resume` — came
  up on a model it had never used.
- `/resume`, `--resume {id}` and `--continue` restored the transcript and
  the code, then ran the rest of the conversation on the directory's
  current entry. The rollout's `session_meta` line names a model, but only
  the one the file was *created* on, and nothing ever read it back.

The rule now: **the model is a fact about the session, recorded in its
rollout, and a resume brings it back.** The directory's entry becomes what a
*new* session starts on — the seed — and nothing more.

## The record

Every rollout file (`docs/resume.md`) gains a `model` record beside the
`session_meta` line:

```json
{"timestamp":"2026-09-15T10:00:00.000Z","type":"model","payload":{"provider":"openrouter","model":"openai/gpt-4o-mini","thinking":{"supported":false},"vision":true,"context":128000}}
```

The payload is `llm::settings::ModelSelection` — the exact shape of a
`config.json` entry: the `(provider, model)` pair plus what the session knows
about the model (the `thinking` blob, `vision`, `context`, and the `speed`
blob a ChatGPT model's `/fast` tier rides in — `docs/fast-mode.md`; a fact
still unknown stays off the line, so a resume probes for it exactly as a
launch on a fact-less entry does). One format for "a saved model selection"
everywhere, so a future field reaches both files at once: `speed` arrived
that way, and a resumed conversation came back on its tier for free.

It is **append-only, and the newest wins** (`session::parse_model` — the
`parse_checkpoints` sidecar's twin, invisible to `parse_session`, so the
loaded transcript is byte-identical with or without it). A line is written:

- with the file itself — the first history item materializes the file
  (codex's deferred create is kept: a session that never says anything
  leaves no file, and a selection alone never creates one), and the model
  line lands right after the meta line;
- on every `/model` pick;
- on every Ctrl+T cycle, every `/fast` switch and on the startup capability
  probe's answer — the same writers `config.json` has, so the facts are as
  fresh as the directory entry's would be;
- with the first item of the fresh file `/clear` starts: the new
  conversation runs on the same session's model.

A resume that lands on the recorded model writes nothing (the record already
says so). Old builds skip the unknown record type (the forward-compatibility
contract) and read the transcript as before.

`SessionRecorder` carries the selection beside the meta (`set_model`, a
`selection_written` watermark — the checkpoint pattern), flushing a changed
selection at the loop-bottom `sync` like every other line. It is the one
place a model line is written, so a rewrite (a backtrack's truncation)
re-emits the current selection right after the meta.

## Restoring it

`/resume` (the picker), `--resume {id}` and `--continue` all parse the file's
newest record and hand it to `ModelSession::restore` — one path, the boot
directive included (`Session::restore_session_model`; the startup capability
probe is spawned *after* the directive is applied, so a resumed session
probes for its own model's facts rather than the directory entry's). The
restore is a `/model` switch minus the write: the backend rebuilds for the
recorded pair with the recorded thinking mode, vision, window and speed
tier, the Ctrl+T cycle and `/fast` seed where they left off, the footer and
the context gauge follow — and `config.json` is **not** touched. A resume chooses nothing; the
directory's entry stays what the last `/model` pick there made it.

A record naming exactly what the session already runs — the common
`--continue` in a directory whose entry the conversation ran on — restores
nothing: no rebuild, and no second probe for facts a queued one will learn.
The whole selection is compared, not the pair, so a record carrying another
Ctrl+T mode still restores. Three cases stop it:

- **An environment pin.** `ALTER_ZERO_MODEL` pins the model for the run
  whatever the record says, and `ALTER_ZERO_PROVIDER` pins the provider — a
  saved pair applies only under the provider it was saved with (the startup
  precedence, `docs/llm.md`; the pure `llm::settings::env_outranks` answers
  it). The session keeps the pinned model, silently — the pin is the user's
  own doing — and the record is left alone (the environment wins and never
  sticks, `config.json`'s rule extended to the rollout).
- **No usable config.** The recorded provider is not in the table, or its
  key is gone (`/login` never run on this machine). The session stays on
  whatever it resolved — the directory's entry, or the dummy — under a red
  `Can't resume on {model}: run /login …` toast, and the record is left
  alone: a fallback forced by a missing key is not a choice, so the file
  still names the model the conversation was on, and a resume after
  `/login` brings it back.
- **No record.** A rollout recorded on the dummy, or by a build from before
  the record existed: nothing to restore, the session keeps its model.

## What `config.json` is now

| | before | now |
|---|---|---|
| what a **new** session starts on | the directory's entry | the directory's entry — unchanged |
| what a **resumed** session runs on | the directory's entry | **the rollout's newest `model` record** |
| a `/model` pick | the directory's entry *and* the last selection | the same — **and** the session's record |
| Ctrl+T / the probe | the directory's entry, when it names this same model | the same — **and** the session's record, always |
| a resume | — | writes nothing |

The one asymmetry worth stating: a `/model` pick made in a *resumed*
conversation still moves the directory's entry. The entry is "the last
choice made here", and that pick is one. The other instance in the same
directory is untouched either way — it runs the model it has in memory, its
rollout names that model, and its own resume brings that one back.
`ModelSession::persisted_selection` keeps meaning "the pair `config.json`
records for this directory", so Ctrl+T in a session whose model differs
from the directory's entry writes the rollout and refuses the entry, exactly
as an env-overridden selection always did.

## Two instances, one directory

Instance A on `x`, instance B on `y`, same cwd:

1. B picks `y` in `/model`: the directory's entry and the last selection are
   `y`; B's rollout records `y`. A's rollout still records `x`; A keeps
   running `x`.
2. A quits. `alter-zero --resume {A}` comes back on `x`. `alter-zero
   --continue` reopens the newest conversation here — B's — on `y`. A plain
   `alter-zero` starts a new session on `y`, the directory's entry.
3. Resuming A while B still runs interleaves nothing: they are two files.
   (Resuming the *same* file from two instances is still the documented
   `docs/resume.md` limitation.)

## Testing

- `session`: `model_line` is a tagged `model` line carrying the selection
  with unknown facts omitted; `parse_model` returns the **newest** record
  whole (the thinking blob included), `None` for a file without one, an
  empty file, or a malformed line; model lines are invisible to
  `parse_session` and `parse_checkpoints`.
- `llm::settings`: `env_outranks` — no pin lets the record apply, a pinned
  model outranks it, a pinned provider outranks a record saved under another
  provider and lets one saved under the same provider apply.
- `scripts/smoke.sh` Phase 118 (the boundary): two instances in one
  directory, the first on the directory's entry and the second on an
  `ALTER_ZERO_MODEL` pin, each recording its own `model` line; the
  directory's entry then moved as a `/model` pick elsewhere would move it;
  `--resume {path}` of a conversation on the entry's own model appends no
  redundant line; the entry then moved, `--resume {path}` comes back on the
  first session's model and leaves `config.json` untouched, `--continue` on
  the second's, a fresh launch on the directory's entry, the `/resume`
  picker inside that fresh session on the first's, an env pin over
  `--resume` on the pinned model, a record naming a provider this machine
  cannot reach on the fresh launch's model under the `Can't resume on …`
  toast, and a rollout with no record (a pre-feature file) on the
  directory's entry, which it then records.
