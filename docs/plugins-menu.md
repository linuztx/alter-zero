# The `/plugins` page

The composer-replacing page that browses, installs and manages plugins —
codex's `/plugins` in this app's picker frame. What a plugin *is*, how it
is fetched and what it adds to a session is `docs/plugins.md`; this page is
what the user sees and the contract its host (the boundary) keeps.

```
────────────────────────────────────────────────────────────────────────────────────────────────────
  Plugins
  Every plugin your marketplaces list
  0 installed · 5 plugins in 2 marketplaces

   All (5)    Installed (0)    claude-plugins-official (3)    openai-curated (2)    + Add

  ❯

  → 42crunch-api-security-testing   + available   Automate API security directly in Claude Code wit…
    activecampaign                  + available   Marketing automation, CRM and email campaigns fro…
    context7                        + available   Upstash Context7 MCP server for up-to-date docume…
    github                          + available   Inspect repositories, triage pull requests and is…
    sentry                          + available   Errors.
    (1/5)

    Automate API security directly in Claude Code with 42Crunch - automatically audit OpenAPI specs…
    42crunch-api-security-testing@claude-plugins-official · v1.0.0 · 1 skill · 2 commands · 3 hooks

  ←/→ tabs · Enter to open · Esc to close
────────────────────────────────────────────────────────────────────────────────────────────────────
```

The settled design is the owner's third round of review (commit `bd1fd84`,
the page removed later only to be rebuilt): its tabs, rows, plugin page,
confirmations and *No flicker* rules are kept as they were, and this
version changes three things — **memory**, **contents before install**, and
the **crate** the page lives in.

## Where it lives, and why a crate of its own

The page — state, keys, jobs and view — is the `alter_zero_plugins` crate's
`page` module (`crates/plugins/src/page/`: `state.rs`, `view.rs`,
`rows.rs`, `host.rs`, `tests.rs`), beside the store it works on. The prior
page's view and state measured 100 % and 71 % resident in an idle session
that never opened it, and the page was moved out of the main crate for
that. Moving it was not what fixed that — code nothing runs is resident or
not by where the linker puts the code that does run, wherever the crate
boundary is (`docs/plugins.md` *Memory*) — and what does is the idle text
layout (`docs/text-layout.md`): the functions a session runs at rest are
laid out together, and the page's are not among them. The crate boundary
stays for what it gives the code: the page depends on the store and on
nothing of the app's.

The crate cannot depend on the main one, so the page draws itself through a
**kit**: `page::PageKit`, a trait of plain methods the main crate implements
(`src/ui/plugins_view.rs`, `FamilyKit`) by delegating to the picker family's
own helpers, so the page cannot look unlike its siblings and nothing is
copied. What the kit provides:

| Kit method | Main-crate helper |
| --- | --- |
| `cols`, `ellipsize`, `clamp` | `ui::wrap::{cols, ellipsize, clamp_spans}` |
| `wrap_spans`, `linked_segments`, `url_run` | `ui::inline::wrap_inline`, `model_view::{linked_segments, whole_url_run}` |
| `age`, `elapsed`, `field_width` | `model_view::age_text`, `app::format_elapsed`, `layout::text_field_width` |
| `indent`, `prompt`, `action_marker`, `chip_gap`, `value_gap`, `list_rows` | `MODEL_INDENT`, `MODEL_PROMPT`, `HOOKS_MARKER`, `ASK_CHIP_GAP`, `SETTINGS_VALUE_GAP`, `SETTINGS_MENU_MAX_ROWS` |
| `color(KitColor)` | the theme accessors (`model_meta_color`, `model_selected_color`, `model_active_color`, `tool_fail_color`, `toast_error_color`, `ask_warning_color`, the `ask_chip_*`, the `settings_value*`, the `mcp_detail_*`, `mcp_description_color`) |
| `rule`, `title`, `search_line`, `row_head`, `row_head_cols`, `counter` | `model_rule`, `header::page_title_line`, `search_line`, `marked_row_head`, `row_head_cols`, `list_counter_line` |
| `placeholder_row`, `wrapped_rows`, `linked_rows`, `description_row`, `prompt_row` | `model_placeholder_row`, `model_wrapped_rows`, `model_linked_rows`, `model_description_rows(…, OneRow, …)`, `view_prompt_row` |
| `list_window`, `picker_page`, `compact_page_with_header`, `compact_page`, `placeholder_window`, `first_page_that_fits` | the `model_view` ladder of the same names |

The page's own words and its own geometry (the name column's 12–32 cap,
the 8- and 30-column description floors, the fact label gap) are constants
in `view.rs`. `model_view::linked_segments` became `pub(super)` for the
kit; nothing else in the main crate changed shape.

**The main crate holds only shims**: `App::plugins_menu:
Option<Box<PluginsPage>>` (one pointer while closed) and `App::plugin_rounds`;
`src/app/plugins_menu.rs` opens, closes and forwards; `src/ui/plugins_view.rs`
is the kit plus `plugins_build` / `plugins_view_lines` /
`plugins_menu_height` / `render_plugins_menu`; `src/tui/plugins.rs` is the
boundary (below), and what it does with the store is the crate's own
`page::{snapshot, contents, run_op}` (`crates/plugins/src/page/host.rs`).

## The contract

### What the host injects

`PluginsSnapshot`, read **when the page opens** — synchronously, a streamed
typed read of a few milliseconds, so the first frame already has its rows
and there is no placeholder for the rows to push away — and dropped with the
page. Nothing about plugins is resident while the page is closed.

```rust
pub struct PluginsSnapshot {
    pub marketplaces: Vec<MarketplaceRow>,   // the added ones, in tab order
    pub plugins: Vec<PluginRow>,             // every plugin of every added marketplace
    pub suggestions: Vec<MarketplaceSuggestion>, // the official catalogs `+ Add` offers
    pub error: Option<String>,               // plugins.json unreadable — red in the header
    pub session_enabled: bool,               // ALTER_ZERO_PLUGINS; off is a red note
}
pub struct PluginRow {
    pub id: PluginId,                        // {name, marketplace}; key() = name@marketplace
    pub display_name, description, version, author, category, homepage: Option<String>,
    pub keywords: Vec<String>,               // searched, never shown
    pub source: String,                      // one line; addresses in it become links
    pub unavailable: Option<String>,         // why it cannot be installed (– unavailable)
    pub format: Option<String>,              // "Claude Code plugin", "Agent Plugins 1.0.0"
    pub contents: PluginContents,            // NotRead | Known(summary) | Unknown | Unreadable(msg)
    pub installed: Option<InstalledState>,   // None: not installed
    pub notes: Vec<String>,                  // what reading it said that did not cost it
}
pub struct InstalledState {
    pub version: Option<String>,
    pub enabled: bool,                       // on in THIS directory
    pub update: bool,                        // the marketplace has a newer copy
    pub error: Option<String>,               // a session does not load it (✘ not loaded)
    pub in_place: bool,                      // loads from its marketplace's directory: no Update
    pub loads_from: Option<String>,          // ~-relative, shown for one loaded in place
}
pub struct ComponentSummary {
    pub skills, commands, agents: Vec<String>,  // by the names a session uses (hello:greet)
    pub hooks: Vec<String>, pub hook_count: usize, // "PreToolUse", "Stop (2)"
    pub servers: Vec<String>,                   // plugin:guard:probe
    pub unused: Vec<String>,                    // "apps", "LSP servers"
    pub preview: bool, pub has_hooks: bool, pub has_servers: bool, // a preview from the file listing
}
pub struct MarketplaceRow {
    pub name, source: String, pub git: bool,
    pub description, owner, revision: Option<String>, pub updated_at: Option<i64>,
    pub plugins, installed: usize,
    pub error: Option<String>,               // its catalog could not be read — red in its header
    pub notes: Vec<String>,                  // entries skipped — amber, the first one's reason
}
pub struct MarketplaceSuggestion { pub source, name, description: String }
```

`PluginContents::NotRead` is what a plugin's contents are until its page
asks; the host may fill `Known` in the snapshot itself when it already
knows them (an installed plugin's), and the list's facts row shows them.

### What the page asks of the host

Every key goes to `PluginsPage::on_key(KeyEvent) -> KeyOutcome` (the main
crate wraps it: `Close` → `Action::ClosePluginsMenu`, `CancelJob` →
`Action::CancelPluginJob`, `Op { op, round }` → `Action::PluginOp { op,
round }`). Ctrl+O and Ctrl+D never reach the page: the main crate opens the
two read-only views over it first (`/usage`'s rule).

```rust
pub enum PluginOp {
    Refresh,                          // read the snapshot again (nothing fetched)
    Inspect(PluginId),                // NOT a job (round 0): answer with supply_plugin_contents
    Install(PluginId),                // install — or update, when installed
    Uninstall(PluginId),              // confirmed on the page first
    SetEnabled { id, enabled },       // this directory; the row already moved
    AddMarketplace(String),           // typed source, or an official catalog's source
    UpdateMarketplace(String),        // Ctrl+U on a git marketplace's tab
    RemoveMarketplace(String),        // Ctrl+R, confirmed, uninstalls what it installed
}
```

### What the host calls back (`App`'s forwarding setters)

| Call | When |
| --- | --- |
| `open_plugins_menu(snapshot)` | `Action::OpenPluginsMenu`, after reading the snapshot |
| `close_plugins_menu()` | the page closes itself; the host stops a running job on `ClosePluginsMenu` / `CancelPluginJob` |
| `update_plugins_snapshot(snapshot)` | the host read the state again (keeps tab, search, screen, the highlighted plugin, contents learnt) |
| `supply_plugin_contents(&id, contents) -> bool` | the answer to `Inspect` |
| `pending_plugin_inspect() -> Option<PluginId>` | after a job: the open plugin page whose contents an install or uninstall dropped |
| `report_plugin_step(round, step) -> bool` | a job's progress (`fetching openai/plugins`) |
| `finish_plugin_job(round, Option<snapshot>, &Result<PluginDone, String>) -> bool` | a job's end; `false` = the page no longer waits on it → the host toasts the outcome (the one toast this page ever causes) |
| `set_plugins_clock(now_unix, frame_clock)` | every draw while open (`update_status_times`) |
| `plugins_job_round() -> Option<u64>`, `plugins_menu_ticks() -> bool` | the slow frame chain (200 ms) while a job runs, never under an overlay |
| `paste_into_plugins(text)` | routed by `App::on_paste` |

`PluginDone { message, marketplace }`: `message` is for that late toast;
`marketplace` names what an add added, and the page lands on its tab.

**Rounds.** A job is marked running on the page the moment it is asked for,
under a fresh round; only an answer carrying the page's current round is
taken. `App::plugin_rounds` is passed to `PluginsPage::open` and read back
from `last_round()` when the page closes, so a reopened page never takes an
old job's answer. One job at a time: asking for a second says in the status
row `Installing deploy — wait for it, or Esc to cancel`.

## What the page shows

- **Tabs** — *All*, *Installed*, one per added marketplace, `+ Add` — the ask
  modal's chips, each with its count, the current one lit; a strip too wide
  shows a window around the current tab with dim `‹`/`›`. ←/→ and
  Tab/Shift+Tab move, wrapping. It opens on *Installed* when anything is,
  else *All*, else `+ Add`.
- **Each tab's header**: a subtitle and counts; a marketplace's tab its
  description, counts and age, source and commit, in red why its catalog
  could not be read, in amber what it skipped and the first one's reason.
- **Rows**: `→ {name}  {mark} {status}  [{marketplace}  ]{description}`.
  Statuses: `✔ enabled` (the one green mark), `◯ disabled`, `✘ not loaded`
  (red), `+ available`, `– unavailable`; an update opens the description on
  an amber `▲ update ·`. Installed first, then by name. The name column is
  capped at two fifths of what the row has past its status (12–32); the
  marketplace is a column only where the rows come from more than one and
  the description keeps 30 columns; a description gets 8 columns or none; a
  row never ends on padding; two rows of one name show their whole ids.
- **Under the list, exactly two rows** — the description cut to one row
  over the facts (id · version · what it is made of) — so ↑/↓ never resize
  the page.
- **Search** matches name, id, marketplace, display name, description,
  category and keywords; every word of the query must match, compared in
  place (no lowercase copy of a row is made, per keystroke or ever). It
  survives a move between the plugin tabs and is cleared entering or
  leaving `+ Add`, whose `❯` line is a source field. A search that matches
  nothing on Installed or a marketplace's tab but something in All says so
  on its row — `No matching plugins here — 1 in All`, counted over All with
  the same search — so ←/→ is the next key; it never switches tab by
  itself.
- **`+ Add`**: the typed source first, then the official catalogs (`✔ added`
  on one already added, where Enter shows its tab), and last while nothing
  is typed `Add a marketplace…`, which opens the one-field add page. A
  success lands on the new marketplace's tab; a failure says why in red and
  keeps what was typed.
- **A plugin's page**: its name; its state leading the line under it (`✔
  enabled in this directory · id · v1.0.0 · by …`, breaking only between
  clauses); the description with its addresses linked; facts on a label
  column sized to their own labels (`Update:`, `Format:`, `Source:`, `Loads
  from:`, `Homepage:`, `Category:`, then `Skills:` `Commands:` `Agents:`
  `Hooks:` `MCP servers:` by the names a session uses — hooks and servers
  followed by `· reviewed before they run` — and `Not used here:`, or one
  `Contents:` row: `reading…`, `read once it is installed`, `could not be
  read — {why}`); the trust warning before an install that runs something,
  or whose contents are unknown or unreadable; the numbered actions, the
  primary first — `Update` leads when there is one — and `Back to plugins`
  last.
- **Confirmations** (uninstall; remove a marketplace, naming the plugins it
  uninstalls) highlight the action the user just chose.

### Keys

Lists: ↑/↓ (wrapping), PageUp/PageDown, Home/End; Enter opens; Space toggles
an installed plugin for this directory (the row answers at once); Ctrl+U
updates a git marketplace on its tab (a directory says it is always
current); Ctrl+R removes it; Esc clears the search, then closes; Ctrl+C
closes; Ctrl+O/Ctrl+D open the read-only views. A plugin's page and the
confirmations: ↑/↓, digits, Enter; Esc or Backspace goes back to the
plugin's row wherever it has since sorted. The add page: a line editor
(←/→, Home/End, Backspace/Delete, Ctrl+U, paste); Enter adds; Esc goes back.

### No flicker

Acting on the page never moves it. A job is said in the hint's own row and
only once it has run `PLUGINS_JOB_DELAY` (400 ms): `Disabling hookify…
writing plugins.json (4s · esc to cancel)`. What went right is never said —
the row, the counts, the plugin page's state line, the new tab show it. A
failure is said in red **in the hint's own row**, cut to that row with `…`,
until the next key brings the hint back — it never adds rows above it. No
toast is raised over an open page. While a job runs the page asks for one
frame when it has run `PLUGINS_JOB_DELAY` and then one a second; nothing at
all otherwise.

### Terminal size

The list pages are pickers and shed rows down the family ladder, never
flowing: a **ten-row** window where the terminal has room (the `/theme`
picker's), then the five-row page, then the description row, then the facts
and the counter (compact window between `↑`/`↓` slots, header kept), then
the header (the tab strip kept, since ←/→ still move), then the compact
rungs down to the search line alone. **The page is one height on a given
terminal while scrolling or typing**: a window holds as many rows as the
tab lists with no search (up to its size) and keeps them, blank, when a
search matches fewer; a search that matches nothing says so inside that
same window, the counter's and the detail's rows kept blank — so the rung
that fits never depends on the selection or the search. The name column is
sized off the whole filtered list, not the window. A plugin's page, a confirmation and the add page bottom-anchor
instead, their actions and status row on screen while the top flows
(signed on its rows — the job clock ticks only on the status row, which is
always painted). Every page hugs its rules at every height; the reserved
height is the built line count; the caret is found in the built page
(search line, add field) and hidden elsewhere, the hidden cursor seated on
the highlighted `❯`.

## The snapshot's shape

`PluginsSnapshot::plugins` is a `PluginList`, **packed**
(`crates/plugins/src/page/rows.rs`): one text buffer every row's strings
are appended to (keywords `\n`-joined — they are only searched), and one
`Vec` of fixed-size rows holding spans into it; what a row owns beyond text
— an installed plugin's state, the contents read for its page, notes — is
one optional box, absent on the plain listed row. A snapshot of 382 plugins
is a handful of allocations (~93 kB of text, ~40 kB of rows), where the
owned rows were some ten thousand small ones. The page reads a row through
`PluginRef`, a `Copy` view over the list and an index whose methods are
named after the owned row's fields; the owned `PluginRow` stays the input
form a host pushes (and `SnapshotRows`, the owned snapshot, what tests
build and `.into()` packs).

It is built **one catalog entry at a time** (`marketplace::stream_catalog`,
`PluginStore::catalog_entries`): a serde visitor over the bounded reader
hands each `plugins` element — decoded and validated by `read_catalog`'s
own rules, which is now this with the entries collected — to the builder,
which packs it and drops it; no catalog is ever held whole. (An entry whose
bare source needs a `metadata.pluginRoot` the document has not reached yet
waits for the end of the document — the one case anything is held.) A
catalog that fails part way is truncated back off the list.

**Why**: glibc keeps what a heap of small interleaved allocations once
held (`docs/memory.md` *The rule*). The owned rows left +656 kB after the
first open and close of 382 plugins and grew to +1 024 kB over four cycles;
packed, +288–296 kB after the first, settling within a few tens of kB.
What remains is one copy of the packed data (the frame buffers that grow to
the page's height after it are allocated above it, so it is reused, not
returned) plus what any tall page costs (`/settings`: +156 kB). Exact
sizing by a counting pass, compacting into exact buffers and reserving
large buffers to be mapped were each measured and made it worse — glibc's
dynamic mmap threshold sends the second same-size request to the heap.

The snapshot after an operation is read **on the loop** as the answer
arrives, and only for a page still open — never on the worker, whose
thread arena would keep what the read once held.

## The boundary

Two halves, so the app's crate stays a forwarder and the logic is tested:

**In the crate** (`crates/plugins/src/page/host.rs`), plain functions over a
`PluginStore`, unit-tested against folder and `file://` git marketplaces:

- `snapshot(store, project, home, session_enabled) -> PluginsSnapshot` —
  `plugins.json`, each added marketplace's catalog and each installed
  plugin's manifest, **no `git`**: the added marketplaces in `plugins.json`'s
  order (a catalog that will not read is its marketplace's `error`, what it
  skipped its `notes`); every listed plugin (no source, or one this app
  cannot fetch, is `unavailable` with the reason); each installed one with
  its state in `project` — on or off here, `✘ not loaded` with the manifest's
  refusal or with the enabled plugin before it that already loaded under its
  name, loaded in place with where from (`~`-relative), and `update` decided
  the cheap way (a pinned commit the record does not start with, or a
  version other than the one offered at install; `plugin update` and Ctrl+U
  ask `git` exactly); installed plugins no catalog lists any more, kept so
  they can be turned off and uninstalled; the offered marketplaces not yet
  added as `suggestions` (the two official ones described); a `plugins.json`
  that will not parse as the snapshot's `error`. Contents stay `NotRead`.
- `contents(store, &id, project, cancel) -> PluginContents` — an installed
  plugin read as a session loads it; a listed one previewed (`Known` with
  `preview: true`, from `git ls-tree` in a git marketplace), `Unknown` when
  its files are elsewhere, `Unreadable` with the store's sentence.
- `run_op(store, project, &op, progress, cancel) -> OpResult { outcome,
  forget, reload }` — every operation but `Inspect`. `Install` on an
  installed plugin is its update (`Updated hello 1.0.0 → 1.1.0`, or `is up
  to date` with nothing to reload). `forget` is the plugin directories whose
  trust is dropped (installed, updated, uninstalled — a removed
  marketplace's) — on a failure too, every directory whose files the
  operation may have replaced, with `reload` set; `reload` is set whenever
  what the session loads may have changed.
- `may_replace(store, &op) -> Vec<PathBuf>` — the directories an operation
  could touch, read before it runs: what a failed uninstall or marketplace
  removal, and a worker that panics, still forget.

**In the app** (`src/tui/plugins.rs`, `#[cold]`, reached only with the
page), over one channel (`PluginsBoundary`, boxed events — all an idle
session holds):

- **Open**: `reload_plugins(&[])` first (another process may have changed
  what is on), then the store (`plugin_cli::plugin_store`) and the snapshot,
  read on the loop — 4–7 ms over the two official catalogs (382 plugins).
  No config home: the page opens with that as its `error`. The store lives
  while the page is open and is dropped when it closes.
- **Run**: one job at a time on a thread with the crate's `CancelToken`; its
  progress lines go to `report_plugin_step`, its answer to
  `finish_plugin_job` with the snapshot read on the loop as it lands,
  cancelled or not. An answer the
  page does not take is a toast only once the page has closed; one Esc let
  go of while the page stayed open just updates its rows. Then, when
  `reload`, `reload_plugins(&forget)`. A worker that panics still answers —
  with `forget` set to what `may_replace` read before it ran, and a reload.
  `Inspect` runs the same way (a preview may run `git`) and lands with
  `supply_plugin_contents`; after a job, a plugin page whose contents it
  dropped is read again (a fresh snapshot keeps what was read only for rows
  whose copy did not change, so an install replaces the preview).
- **Stop**: Esc on a job, the page closing and the quit cancel it, which
  kills the git child's process group; nothing waits on the thread.
- **No toasts over the page**: `reload_plugins` keeps its load notices to
  itself while the page is open — the row says `✘ not loaded`.

**Install → review → page.** An install that brings hooks or servers ends in
`reload_plugins`, whose `set_plugin_contributions` opens the startup
review's pages on what is new: the review takes the screen over the page
(the modal outranks it), the plugin's data directory exists and nothing of
it has run; answering returns to the plugin's page, which by then shows the
installed copy (`Format:`, its hook events and server names). Smoke Phase
163 drives it.

## Testing

- `cargo test -p alter-zero-plugins page::host` — the boundary's functions
  against real folder and git marketplaces.
- `cargo test -p alter-zero-plugins page::` — the state machine: tabs,
  search, rounds and stale answers, the optimistic toggle and its revert,
  Inspect, the action sets, confirmations, the add field, the line editor.
- `cargo test --lib -- app::tests::plugins_menu ui::tests::plugins_view` —
  the shims, and the view through the real kit: every screen hugging its
  frame at 40–120 columns, reserving the rows it paints at heights 1–40, the
  lists never flowing, the caret, wide glyphs, the columns, the job line, a
  printout (`print_the_pages_at_100_columns -- --nocapture`).
- `scripts/smoke.sh 163` — the page on the real store, offline: add,
  install, the review over the page, the `$` picker, Space, a failure on the
  hint's row, a folder marketplace removed with its plugin, Esc killing a
  fetch, uninstall, and `plugin list` agreeing.
