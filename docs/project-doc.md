# AGENTS.md in the context window — codex's project doc (Design)

Date: 2026-07-24. Revised 2026-09-10: the codex `<INSTRUCTIONS>` fragment
became the instructions section of the one `<system-reminder>` the context
leads with (see *Ours* below).

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

The discovery and the budget are codex's. The **rendering is not**: codex
folds every discovered doc into one `<INSTRUCTIONS>` body under the cwd's
name, in a fragment of its own. Ours renders each file under its own heading
inside the one `<system-reminder>` the derived context leads with — the block
the skills and agent-type listings already rode (`docs/skills.md`,
`docs/subagents.md`), so everything the session tells the model about itself
is read in one place, in one shape:

```
<system-reminder>
Use the following contexts and instructions:

Codebase and user instructions are shown below. Be sure to adhere to these instructions. IMPORTANT: These instructions OVERRIDE any default behavior and you MUST follow them exactly as written.

Contents of /work/my-app/AGENTS.md (project instructions, checked into the codebase):

{the root guide}

Contents of /work/my-app/crates/app/AGENTS.md (project instructions, checked into the codebase):

{the nested guide}

The following skills are available for use with the Skill tool:

- …

Available agent types for the Agent tool:

- …
</system-reminder>
```

Three things the fragment could not say, each a reason:

- **Which file said it.** A nested guide refines its parent's, and the fold
  lost the seam: two guides read as one document with a blank line in it.
  The per-file heading names the directory an instruction governs.
- **Whether it is checked in.** The git-ignorable `AGENTS.override.md`
  carries `(user's private project instructions, not checked in)` where a
  committed guide carries `(project instructions, checked into the
  codebase)` — the reference tool's own captions. Where an instruction came
  from is part of what it means.
- **One block, not two.** The listings were already a `<system-reminder>`
  sitting behind the fragment: two leading fragments were two places the
  prompt-cache prefix could shift, and two shapes for one kind of thing. The
  preamble line and the override paragraph are the reference's own wording —
  what a model trained on that tool already treats as binding.

The port keeps the project's pure-core / boundary split.

- **`src/project_doc.rs`** — the pure pieces:
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
  - `ProjectDoc { path, text }` — one discovered doc: the file it was read
    from and what it said.
  - `budget_docs(docs, max_bytes)` — codex's budget: truncate the
    overflowing doc to the remaining bytes (on a `char` boundary — our input
    is already `String`), skip blanks, stop when spent — the docs kept
    **separate** rather than joined, since each renders under its own
    heading. Empty when nothing survives.
  - `doc_heading(path)` — `Contents of {path} ({note}):`, the note
    `DOC_CHECKED_IN_NOTE` for a guide and `DOC_OVERRIDE_NOTE` for an
    `AGENTS.override.md`.
  - `instructions_section(docs)` — the `INSTRUCTIONS_PREAMBLE` over each
    doc's heading and text, blank-line separated; empty with no docs, so the
    section is then simply absent from the reminder.

  And the small fs boundary, tempfile-tested in-module (the `checkpoint`
  precedent for library I/O):
  - `find_project_root(cwd)` — nearest ancestor-or-self containing `.git`
    (a dir *or* a worktree's gitfile).
  - `load_user_instructions(cwd)` — root → chain → read the first
    candidate per directory (raw bytes, `from_utf8_lossy` like codex, so
    one stray invalid byte never silently drops a whole guide) → budget →
    render, returning the rendered section (or `None` when no doc exists /
    the budget is `0`). One stat per ancestor + at most a few small reads:
    cheap enough to run per turn. `load_user_instructions_with` is the same
    under an explicit budget (the testable seam).
  - **The reads are capped at the budget** (`read_capped` —
    `File::take(cap)`, `tui::shell::append_capped`'s pattern). The budget has to
    bound the *I/O*, not just the folded output: a huge file that merely
    happens to be named `AGENTS.md` would otherwise be slurped whole while
    the raw-mode terminal waits for its first frame — and because this
    loader re-runs at **every turn start**, that cost would be paid over
    and over. Same class as the "hangs in `~`" checkpoint guard.

- **`src/reminder.rs`** — the wrapper, shared with the listings:
  `reminder_message(sections)` puts the non-blank sections, blank-line
  separated, between the tags under the `Use the following contexts and
  instructions:` preamble (`REMINDER_PREAMBLE`), and is **empty when no
  section says anything** — a session with no AGENTS.md, no skills and no
  agent types sends no block at all, never a bare preamble. `join_sections`
  is the join the listings reuse for their own two parts.

- **`context::context_messages_full(instructions, listings, history)`** —
  composes the block from the two sections `App` holds — `App::user_instructions`
  (this module's section) and `App::listings` (`subagents::listing_sections`)
  — and seats it as the derived context's first **user** entry, in front of
  the normal derivation *and* the post-`/compact` shape (codex keeps its
  initial context through compaction the same way). Composed at derivation
  rather than stored assembled because the two inputs change at different
  moments — the instructions at every turn start and on the **Project docs**
  toggle, the listings on every rescan and `/model` switch — so the request,
  Ctrl+D and the token estimate can never disagree about the block.
  `context_messages_with(instructions, history)` is the instructions-only
  case the tools-free `/compact` turn sends. It rides `push_text`, so a
  first user message merges after the block under the module's alternation
  convention — the tags keep the boundary unambiguous to the model. The
  compaction budget walk (`budgeted_user_texts`) reads *history*, which the
  instructions never enter, so a compact never re-summarizes them — codex
  needs a marker check (`is_user_instructions`) for the same exclusion.

- **`App::user_instructions`** + `set_user_instructions` — the
  `set_system_prompt` pattern: the boundary injects the rendered section,
  the pure core stores it for everyone who derives context — the Ctrl+D
  view (`ui::context_lines`, which shows the block as the first user
  entry), and the offline token estimate (`App::estimate_context_tokens`, so
  the footer gauge counts what the request really carries).

- **The boundary (`tui::bootstrap`, `tui::turn`, `tui::settings`)** loads
  once at startup, **refreshes at every `start_turn`** — a re-render from
  disk right before the context derives — and reloads on the **Project
  docs** toggle. That per-turn refresh is what makes `/init` land: the turn
  that *writes* `AGENTS.md` ends, and the very next user message already
  carries the new guide (current codex re-reads per turn snapshot the same
  way). The compact turn and a background-completion follow-up derive with
  the stored value — both only ever run mid-conversation, after a
  `start_turn` seeded it.

## What deliberately isn't here

- **No `<INSTRUCTIONS>` markers, no `# AGENTS.md instructions for {cwd}`
  heading.** Codex's history filtering needs the markers to recognise its
  own fragment later; ours never enters history, so nothing has to
  recognise it, and the reminder's tags already bound the block.
- **No user-level global AGENTS.md** (codex's `~/.codex/AGENTS.md` combined
  over a `--- project-doc ---` separator). We have no other global
  instruction source; a `~/.alter-zero/AGENTS.md` would be a follow-up —
  one more `Contents of` block, with the reference's `(user's private global
  instructions for all projects)` caption.
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
  the budget (`budget_docs`: skip blanks, truncate on the boundary,
  spend-and-stop, empty when nothing survives), the env knob (`doc_budget`
  parsing, the `0` off switch), the heading (both captions), the section
  (the preamble, one heading per file, no codex markers, empty without
  docs), and the tempfile fs walk (nested docs each under their own heading,
  the override beating its directory's `AGENTS.md` *and* captioned as
  private, invalid UTF-8 surviving lossily, a `.git` *file* accepted, no
  marker → cwd only, missing docs skipped, the body cut to the budget).
- `reminder::tests` — the join, the wrapping under the preamble, the empty
  case, the wording.
- `scripts/smoke.sh` Phase 52 — the boundary end to end on the dummy: a
  planted `AGENTS.md` shows under its `Contents of … (project instructions,
  checked into the codebase):` heading inside the `<system-reminder>` in
  the Ctrl+D view before any turn, `/init` submits the bundled prompt as
  the user message, and a mid-turn `/init` is rejected with the busy toast.
- `context::tests` — the block as the leading user entry, its merge with a
  first user message, the listings inside the same block behind the
  instructions, its presence in front of the compacted shape, neither/blank
  leaving the derivation untouched, and a subagent's already-wrapped
  briefing taken verbatim (`context_messages_behind`).
- `app::tests` / `ui::tests` — the setters, the estimate counting the
  instructions, and the Ctrl+D view showing the block first.
- `tests/live_openrouter.rs` `live_agents_md_instructions_reach_the_model`
  — the whole path on the real wire: a planted guide, the section, the
  block, and the model reading a sentinel back out of it.
