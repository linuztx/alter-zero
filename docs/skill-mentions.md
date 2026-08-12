# `$` skill mentions — a skill picker below the box

Date: 2026-08-12

## Goal

Typing **`$`** in the composer opens a **skill picker** below the input box —
openai/codex's `$` skill-mention popup, presented in this TUI's band style:
type `$`, fuzzy-filter the discovered skills as you type, ↑/↓ to choose,
**Tab/Enter inserts the mention** (`$name` — the sigil stays, plus a
separating space), Esc dismisses. The mention can sit **anywhere in the
message** (`Hello world $dataviz …chart this`), and submitting a message that
carries one makes the model **load that skill's manual** before answering:

```text
────────────────────────────────────────────────────────────────────────
❯ $
────────────────────────────────────────────────────────────────────────
  dataviz        Generate or edit charts for websites, games, a…
→ skill-creator  Create or update a skill
```

## What the references do (from `/tmp/codex`, `/tmp/claude-code-source-code`)

- **codex trigger** (`tui/src/bottom_pane/chat_composer/completion_target.rs`):
  the `$` must open a whitespace-delimited token with the cursor inside it —
  `foo$bar` and `US$5` never trigger. Mention names continue over
  `[A-Za-z0-9_-]` (`is_mention_name_char`) and end at the first other byte,
  so `$dataviz,` still queries `dataviz`. A bare `$` lists everything.
- **codex shell rejection** (`dollar_query_kind`): what is really shell
  syntax never opens the popup — positional parameters (`$1`), `$-`/`$_`,
  and the well-known uppercase environment variables (`is_common_env_var`:
  `PATH`, `HOME`, …). Lowercase-carrying queries always search (skill names
  are lowercase; the popup's fuzzy match folds case).
- **codex popup** (`skill_popup.rs`): one row per match — name, then a dim
  description in a column aligned to the widest visible name — max 8 rows,
  ↑/↓ move, Tab/Enter insert `$name` + a trailing space (reusing an existing
  following space — `advance_past_completion_separator`), Esc dismisses and
  remembers the token so it doesn't reopen.
- **codex submit** (`core/src/session/turn.rs`,
  `skills/src/selection.rs`, `ext/skills/src/fragments.rs`): the text goes
  to the model **verbatim**; at turn start codex re-scans it for `$token`s,
  resolves unambiguous names against the discovered skills, reads each
  `SKILL.md`, and injects one `<skill>…</skill>` user message per mention.
  Codex has no skill *tool*, so eager injection is its only way to load one.
- **Claude Code**: no `$` mentions at all — skills load through the `Skill`
  tool (progressive disclosure: a budgeted listing in context, the tool
  fetches the body on demand), or a `/skill-name` slash command.

## The mapping onto this codebase

The picker is a **fourth live-region band** below the box, exactly parallel
to the `@` file picker (`docs/file-search.md`) — but its matches derive
**synchronously** from the already-discovered `SkillRegistry`, so unlike the
file picker there is no async round-trip: the palette's derive-on-demand
shape (`app::COMMANDS`) with the file picker's token trigger.

**Submitting a mention rides the `Skill` tool, not codex's eager injection.**
This TUI already has Claude Code's `skill` tool with the two-text split
(`docs/skills.md`): the visible `● Skill(name)` cell, the body on
`ToolCall::context_output`, the Ctrl+D replay, the rollout round-trip, and a
subagent's own copy all come from it. So the mention stays plain text in the
message and the **guidance** makes the model call the tool:

- `prompts/tools.md` names `$<skill-name>` beside the existing
  `/<skill-name>` shorthand;
- `skills::listing_message` closes the per-turn `<system-reminder>` with the
  mention rule, right beside the names it applies to (static text, so the
  fragment stays prompt-cache-stable while the listing doesn't change);
- verified live: `live_dollar_mention_loads_the_mentioned_skill`
  (`tests/live_openrouter.rs`) proves a real model answers a bare
  `Use $mixology — name a cocktail` by calling `skill("mixology")` and
  obeying the loaded body.

The deliberate divergence from codex: their injection is deterministic but
invisible (no cell, no record of *whether* the model needed it); the tool
path shows the user the load as the green `● Skill(name)` cell, keeps the
body out of the transcript, and costs nothing when the model already has the
manual in context (the guidance tells it not to re-load). An unmatched
`$name` is a no-op in both designs — the literal text reaches the model.

### Pure core (`skills.rs` — beside the rest of the skill model)

- `SKILL_MENTION_PREFIX` (`'$'`), `COMMON_ENV_VARS` (codex's list).
- `mention_token(text, cursor) -> Option<MentionToken>` — the codex grammar
  above; `MentionToken { range, query }` is the `AtToken` of the `$` band
  (`range` spans the `$` through the name run, never trailing punctuation).
- `shell_flavored_query(query) -> bool` — the popup-side rejection, kept a
  separate predicate so the scanner stays a truthful parse.
- `SkillMatch { name, description, score, indices }` +
  `rank_skills(query, &[SkillMetadata])` — `file_search::fuzzy_match` on
  each **name**, best score first, ties keeping discovery (precedence)
  order via the stable sort; an empty query lists everything. No cap — the
  band windows with `menu_window` like the palette.

### App state (`app/skill_picker.rs`)

- `App::skill_picker: Option<SkillPicker { selected, query }>` — the
  palette's shape: only the highlight is stored, matches derive on demand.
- `App::skills` (private) + `App::set_skills` — the boundary injects the
  registry's **enabled** snapshot beside every listing render
  (`tui::models::Session::sync_skill_listing`), so the picker, the listing
  and the tool set always agree; empty leaves `$` an ordinary dollar sign.
- `refresh_skill_picker(had_mention)` — `refresh_file_search`'s logic over
  `mention_token`: open on the None→Some transition (Esc-dismiss is sticky
  within the mention), reset the highlight on query change, close when the
  mention is gone. Suppressed in `!` shell mode (`$VAR` is real shell there)
  and while `skills` is empty.
- `skill_band_active()` — open **and** the cursor still in a usable mention:
  the rows derive from the live token, so a plain cursor move out of the
  mention hides the band and releases ↑/↓ before any edit closes the state.
- `skill_matches()` / `highlighted_skill_match()` / `move_skill_selection` /
  `accept_skill_selection` — the accept replaces the token with `$name`
  (sigil kept — it is what marks the mention in the submitted text) plus a
  separating space, **reusing** an existing following space rather than
  doubling it, cursor past the separator.

### Key dispatch (`App::on_key_conversation`)

`skill_open = skill_band_active()`, exactly parallel to the file picker's
arms and mutually exclusive with them by construction (a token starts with
exactly one sigil): Esc dismisses (sticky), ↑/↓ move, **Tab/Enter accept**
when a match is highlighted — otherwise they fall through, so an unmatched
`$query` still queues/submits. Every composer edit (Char, Backspace, Delete,
paste, image-attach) snapshots `had_mention` and re-derives the picker
beside the palette/shell/file refreshes; every composer-consuming path
(submit, queue, Ctrl+C clear, `/clear`, the composer-replacing pickers, the
Ctrl+R search, a recall) closes it beside `file_search`.

### Rendering (`ui/menu.rs`)

The band shares the palette's slot: `band_rows += skill_menu_rows`.

- `skill_menu_rows(app)` — 0 closed (cursor-left-the-mention included), one
  placeholder row (`No matching skills`) on a miss, else the match count
  capped at `SKILL_MENU_MAX_ROWS` (8; longer lists scroll).
- `skill_menu_lines(app, width)` — one row per match, windowed
  (`menu_window`): the `→` marker on the selection (the rest indent), the
  name with the query's matched bytes **bolded** (`file_menu_highlight`
  reused at offset 0), then — past a name column sized to the widest
  visible name + `FILE_MENU_GAP` — the skill's own description, `…`-cut at
  the width. The selected row lights up cyan whole, the rest dim (the
  palette's convention; `SKILL_MENU_*` in `ui/theme.rs`).

### The offline dummy and the demo

The `skills` scenario answers `$dataviz` mentions too
(`cue.mentions("$dataviz")` beside `"skill"` — `stream/dummy/scenario.rs`),
so the offline first run plays the real `● Skill(dataviz)` load for a
mention exactly as a live model would, and the smoke suite can drive the
whole round trip with no network.

## Testing

- `skills` (unit): the token grammar (mid-text, terminator, boundary rule,
  bare `$`), the shell-flavored predicate, ranking (fuzzy on names, stable
  ties, empty query = discovery order).
- `app` (unit): open/close/sticky-Esc/shell-mode suppression/no-skills
  inertness; selection move+clamp+reset; Tab/Enter accept (sigil kept,
  space reused, cursor seat) vs fall-through on a miss; mutual exclusion
  with the palette and the file picker; every consuming path closes it.
- `ui` (unit): row counts (closed/placeholder/capped), the aligned
  name/description columns, the `…` cut, matched-char bolding, selection
  colours, `band_rows` inclusion, the cursor-left-the-mention hide.
- `stream` (unit): the `$dataviz` cue selects the skills demo.
- `scripts/smoke.sh` Phase 79: against a planted skill dir — type `$`, the
  band lists both skills with descriptions; filter narrows; Tab completes
  the mention into the composer; submitting a mention plays the skill demo
  (the `● Skill(dataviz)` cell, nothing of the body inline).
- `tests/live_openrouter.rs` (`--ignored`):
  `live_dollar_mention_loads_the_mentioned_skill` — a real model, a real
  `SKILL.md`, a prompt whose only signal is the `$` mention.
