# AGENTS.md in the context window — codex's project doc (Design)

Date: 2026-07-24

## Goal

`/init` generates an `AGENTS.md` contributor guide (`docs/init.md`) — but
until now nothing ever read it back, so the guide the model wrote never
reached the model again. Codex closes that loop: every conversation carries
the project's `AGENTS.md` instructions in the context window, as **user
instructions** the model sees before the conversation. This is the port.

## What codex does

Codex's discovery (`codex-rs/core/src/agents_md.rs`) and injection
(`core/src/context/user_instructions.rs`):

1. **Find the project root**: walk up from the cwd until a directory holding
   a project-root marker (default `.git`) is found. No marker anywhere →
   only the cwd is considered. Never walk past the root.
2. **Collect docs root→cwd**: every `AGENTS.md` found on the path from the
   project root *down to* the cwd (inclusive), concatenated in that order —
   an inner directory's guide refines the outer one.
3. **Cap the total** at `project_doc_max_bytes` (default **32 KiB**): each
   file is truncated to the remaining budget, blank files are skipped, and
   discovery stops when the budget is spent.
4. **Inject as a user-role message** at the front of the conversation — not
   the system prompt — rendered with recognisable markers
   (`ContextualUserFragment::render`):

   ```
   # AGENTS.md instructions for {cwd}

   <INSTRUCTIONS>
   {concatenated docs}
   </INSTRUCTIONS>
   ```

   The markers let codex recognise the injected fragment later (its history
   filtering); the `for {cwd}` clause names the project the instructions
   describe.

## Ours

The port keeps the project's pure-core / boundary split.

- **`src/project_doc.rs`** (new module) — the pure pieces:
  - `PROJECT_DOC_MAX_BYTES` — codex's 32 KiB default; `doc_budget` parses
    the `ALTER_ZERO_PROJECT_DOC_MAX_BYTES` override (codex's
    `project_doc_max_bytes` config knob in the house env style — a number
    wins, `0` disables loading entirely, anything else is the default).
  - `PROJECT_DOC_FILENAMES` — per directory the first existing candidate
    contributes: `AGENTS.override.md` (codex's git-ignorable local
    override) beats the checked-in `AGENTS.md`.
  - `doc_chain(root, cwd)` — the directories from the project root down to
    the cwd, codex's cursor walk (cwd up to root, reversed). No root → just
    the cwd.
  - `combine_docs(docs, max_bytes)` — codex's budget: truncate the
    overflowing doc to the remaining bytes (on a `char` boundary — our input
    is already `String`), skip blanks, join with a blank line, stop when
    spent. `None` when nothing survives.
  - `instructions_message(text, directory)` — the exact codex fragment
    above; `directory: None` drops the `for …` clause.

  And the small fs boundary, tempfile-tested in-module (the `checkpoint`
  precedent for library I/O):
  - `find_project_root(cwd)` — nearest ancestor-or-self containing `.git`
    (a dir *or* a worktree's gitfile).
  - `load_user_instructions(cwd)` — root → chain → read the first
    candidate per directory (raw bytes, `from_utf8_lossy` like codex, so
    one stray invalid byte never silently drops a whole guide) → combine
    under the env-resolved budget → render, returning the full instructions
    message (or `None` when no doc exists / the budget is `0`). One stat
    per ancestor + at most a few small reads: cheap enough to run per turn.
    `load_user_instructions_with` is the same under an explicit budget (the
    testable seam).
  - **The reads are capped at the budget** (`read_capped` —
    `File::take(cap)`, `main.rs::read_capped`'s pattern). The budget has to
    bound the *I/O*, not just the folded output: a huge file that merely
    happens to be named `AGENTS.md` would otherwise be slurped whole while
    the raw-mode terminal waits for its first frame — and because this
    loader re-runs at **every turn start**, that cost would be paid over
    and over. Same class as the "hangs in `~`" checkpoint guard.

- **`context::context_messages_with(instructions, history)`** — the derived
  context grows an optional leading **user** entry holding the rendered
  instructions, in front of both the normal derivation *and* the
  post-`/compact` shape (codex keeps its initial context through compaction
  the same way). It rides `push_text`, so a first user message merges after
  it under the module's alternation convention — the `<INSTRUCTIONS>`
  markers keep the boundary unambiguous to the model. The existing
  `context_messages(history)` delegates with `None`; the compaction budget
  walk (`budgeted_user_texts`) reads *history*, which the instructions never
  enter, so a compact never re-summarizes them — codex needs a marker check
  (`is_user_instructions`) for the same exclusion.

- **`App::user_instructions`** + `set_user_instructions` — the
  `set_system_prompt` pattern: the boundary injects the rendered message,
  the pure core stores it for everyone who derives context — the Ctrl+D
  view (`ui::context_lines`, which now shows the instructions as the first
  user entry), and the offline token estimate
  (`App::estimate_context_tokens`, so the footer gauge counts what the
  request really carries).

- **The boundary (`main.rs`)** loads once at startup and **refreshes at
  every `start_turn`** — a re-render from disk right before the context
  derives. That per-turn refresh is what makes `/init` land: the turn that
  *writes* `AGENTS.md` ends, and the very next user message already carries
  the new guide (current codex re-reads per turn snapshot the same way).
  The compact turn and a background-completion follow-up derive with the
  stored value — both only ever run mid-conversation, after a `start_turn`
  seeded it.

## What deliberately isn't here

- **No user-level global AGENTS.md** (codex's `~/.codex/AGENTS.md` combined
  over a `--- project-doc ---` separator). We have no other global
  instruction source; a `~/.alter-zero/AGENTS.md` would be a follow-up, and
  the separator logic with it.
- **No fallback-filename / root-marker config** — codex's
  `project_doc_fallback_filenames` and `project_root_markers` are config
  surface we don't have; the candidates (`AGENTS.override.md`,
  `AGENTS.md`) and the `.git` marker are compiled in. The byte budget *is*
  overridable (`ALTER_ZERO_PROJECT_DOC_MAX_BYTES`, `0` = off) — it doubles
  as the feature's kill switch.
- **No history recording** — the instructions are derived context, injected
  per request like the system prompt; they never enter `App::history`, the
  rollout file, or the transcript. `/resume` and the Esc-Esc backtrack
  therefore need no changes: the next turn re-derives.

## Tests

- `project_doc::tests` — the chain order (root→cwd, root == cwd, no root),
  the budget (skip blanks, truncate on the boundary, spend-and-stop, `None`
  when empty), the env knob (`doc_budget` parsing, the `0` off switch), the
  fragment format (both marker lines, the `for` clause dropped without a
  directory), and the tempfile fs walk (nested docs collected in order, the
  override beating its directory's `AGENTS.md`, invalid UTF-8 surviving
  lossily, a `.git` *file* accepted, no marker → cwd only, missing docs
  skipped).
- `scripts/smoke.sh` Phase 52 — the boundary end to end on the dummy: a
  planted `AGENTS.md` shows under `# AGENTS.md instructions` in the Ctrl+D
  view before any turn, `/init` submits the bundled prompt as the user
  message, and a mid-turn `/init` is rejected with the busy toast.
- `context::tests` — the leading user entry, its merge with a first user
  message, its presence in front of the compacted shape, and `None`/blank
  leaving the derivation untouched.
- `app::tests` / `ui::tests` — the setter, the estimate counting the
  instructions, and the Ctrl+D view listing them first.
