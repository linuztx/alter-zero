//! Cross-session input recall: [`InputHistory`] behind ↑/↓ and the Ctrl+R
//! reverse search over it ([`HistorySearch`]).
//! See `docs/input-history.md` and `docs/history-search.md`.

use super::*;

/// Shell-style ↑/↓ recall of previously submitted inputs — a port of codex's
/// `ChatComposerHistory` (its in-session `local_history`; no cross-session
/// persistence or Ctrl+R search). See `docs/input-history.md`.
#[derive(Debug, Default)]
pub struct InputHistory {
    /// The recorded texts, oldest first (newest at the end).
    pub(super) entries: Vec<String>,
    /// The entry currently recalled; `None` when not browsing.
    cursor: Option<usize>,
    /// What navigation last wrote into the composer. The gate in
    /// [`should_navigate`] compares against it so an *edited* recall counts as
    /// a fresh draft and stops browsing.
    ///
    /// [`should_navigate`]: InputHistory::should_navigate
    last_recall: Option<String>,
    /// Entries recorded *this session* that the boundary has not yet flushed to
    /// the persistent history file, newest last — the "core queues, boundary
    /// does the I/O" pattern (like `insert_before`'s pending lines). Filled by
    /// [`record`] on a genuine append, drained by [`take_unpersisted`]. Seeded
    /// entries (already on disk) never land here. See `docs/history-persistence.md`.
    ///
    /// [`record`]: InputHistory::record
    /// [`take_unpersisted`]: InputHistory::take_unpersisted
    unpersisted: Vec<String>,
    /// The last text queued for (or seeded from) the persistent file — the
    /// dedup target for persistence, kept **separate** from `entries`'s
    /// in-memory dedup so a never-persisted [`record_ephemeral`] draft can't
    /// suppress a genuine submission's write. See `docs/history-persistence.md`.
    ///
    /// [`record_ephemeral`]: InputHistory::record_ephemeral
    last_persisted: Option<String>,
}

impl InputHistory {
    /// Record a submitted input and exit browsing, **queuing it for the
    /// persistent history file**. Blank texts are ignored and an entry
    /// identical to the newest is collapsed in memory, like codex's
    /// `record_local_submission`. The persist dedup is against
    /// `last_persisted`, **not** `entries` — so a never-persisted
    /// [`record_ephemeral`] draft can't mask a genuine submission's write (the
    /// file still collapses adjacent duplicates). See `docs/history-persistence.md`.
    ///
    /// [`record_ephemeral`]: InputHistory::record_ephemeral
    pub fn record(&mut self, text: &str) {
        self.record_inner(text);
        if text.is_empty() || self.last_persisted.as_deref() == Some(text) {
            return;
        }
        self.last_persisted = Some(text.to_string());
        self.unpersisted.push(text.to_string());
    }

    /// Record an input that should recall this session but **never persist** —
    /// the Ctrl+C-cleared draft. codex keeps cleared drafts in its in-session
    /// `local_history` only, so an abandoned draft doesn't pollute the
    /// cross-session history file, and it never advances `last_persisted`
    /// (`docs/history-persistence.md`).
    ///
    pub fn record_ephemeral(&mut self, text: &str) {
        self.record_inner(text);
    }

    /// The shared record body: exit browsing, drop blanks and adjacent
    /// duplicates, append otherwise.
    fn record_inner(&mut self, text: &str) {
        self.cursor = None;
        self.last_recall = None;
        if text.is_empty() || self.entries.last().is_some_and(|prev| prev == text) {
            return;
        }
        self.entries.push(text.to_string());
    }

    /// Seed `entries` from the persistent history file at startup (oldest
    /// first), as a faithful **replay of [`record`]**: each text runs the same
    /// blank-skip + adjacent-duplicate collapse, so a messy or concurrently
    /// written file (adjacent dups) seeds the same clean buffer a fresh session
    /// would build. These are already on disk, so they are **not** queued for
    /// persistence — but the newest seeded entry becomes the `last_persisted`
    /// dedup target, so a first submission identical to it isn't re-written.
    /// Both ↑/↓ recall and Ctrl+R search read `entries`, so seeding makes both
    /// span sessions with no other change. See `docs/history-persistence.md`.
    ///
    /// [`record`]: InputHistory::record
    pub fn seed(&mut self, entries: Vec<String>) {
        for text in entries {
            self.record_inner(&text);
        }
        self.last_persisted = self.entries.last().cloned();
    }

    /// Drain the entries recorded this session that are not yet on disk, for
    /// the boundary to append. See `docs/history-persistence.md`.
    pub fn take_unpersisted(&mut self) -> Vec<String> {
        std::mem::take(&mut self.unpersisted)
    }

    /// Should an ↑/↓ press browse history instead of moving the cursor? Yes
    /// for an empty composer; for a non-empty one only when the text is
    /// exactly the last recalled entry (unedited) with the cursor at either
    /// end — so a typed draft is never clobbered and the arrows still move
    /// within an edited recall (codex's `should_handle_navigation`).
    #[must_use]
    pub fn should_navigate(&self, text: &str, cursor: usize) -> bool {
        if self.entries.is_empty() {
            return false;
        }
        if text.is_empty() {
            return true;
        }
        if cursor != 0 && cursor != text.len() {
            return false;
        }
        self.last_recall.as_deref() == Some(text)
    }

    /// Step to the older entry (↑), entering browsing at the newest. `None` at
    /// the oldest — the caller falls back to cursor movement, like codex.
    pub fn up(&mut self) -> Option<String> {
        let next = match self.cursor {
            None => self.entries.len().checked_sub(1)?,
            Some(0) => return None,
            Some(index) => index - 1,
        };
        self.cursor = Some(next);
        let text = self.entries[next].clone();
        self.last_recall = Some(text.clone());
        Some(text)
    }

    /// Indices of the entries whose text contains `query` (case-insensitively),
    /// newest first, keeping only the **newest** occurrence of duplicated
    /// texts — codex's Ctrl+R traversal (`chat_composer_history.rs::search`:
    /// lowercased substring match, `seen_texts` dedup). An empty query matches
    /// every entry, though the search session treats that as Idle. See
    /// `docs/history-search.md`.
    #[must_use]
    pub fn search(&self, query: &str) -> Vec<usize> {
        let needle = query.to_lowercase();
        let mut seen = HashSet::new();
        self.entries
            .iter()
            .enumerate()
            .rev()
            .filter(|(_, text)| text.to_lowercase().contains(&needle))
            .filter(|(_, text)| seen.insert(text.as_str()))
            .map(|(index, _)| index)
            .collect()
    }

    /// The recorded text at `index` (0 = oldest — as [`search`] indexes them).
    ///
    /// [`search`]: InputHistory::search
    #[must_use]
    pub fn entry(&self, index: usize) -> Option<&str> {
        self.entries.get(index).map(String::as_str)
    }

    /// Seat ↑/↓ browsing at `index`, as if that entry had just been recalled:
    /// the next ↑ steps to the entry older than it. Accepting a Ctrl+R match
    /// lands here — codex's search shares its navigation cursor the same way
    /// (`search_match` sets `history_cursor` + `last_history_text`).
    pub fn resume_at(&mut self, index: usize) {
        if let Some(text) = self.entries.get(index) {
            self.cursor = Some(index);
            self.last_recall = Some(text.clone());
        }
    }

    /// Step to the newer entry (↓). Past the newest returns an empty string —
    /// "clear the composer and stop browsing" (codex's `navigate_down`);
    /// `None` when not browsing at all.
    pub fn down(&mut self) -> Option<String> {
        let index = self.cursor?;
        if index + 1 < self.entries.len() {
            self.cursor = Some(index + 1);
            let text = self.entries[index + 1].clone();
            self.last_recall = Some(text.clone());
            Some(text)
        } else {
            self.cursor = None;
            self.last_recall = None;
            Some(String::new())
        }
    }
}

/// One open Ctrl+R reverse history search — codex's `HistorySearchSession`
/// (`chat_composer/history_search.rs`). While it is `Some` on [`App`] the
/// search owns every key: typed characters edit [`query`], Ctrl+R/↑ and
/// Ctrl+S/↓ step between matches, Enter accepts the previewed match as an
/// editable draft, and Esc/Ctrl+C restore the `snapshot`. The footer slot
/// renders it as `reverse-i-search: {query}` (`ui::search_line`). See
/// `docs/history-search.md`.
///
/// [`query`]: HistorySearch::query
#[derive(Debug)]
pub struct HistorySearch {
    /// The draft (text **and** cursor) from before the search opened, restored
    /// verbatim on cancel — and shown again while a query has no match
    /// (codex's `original_draft`).
    snapshot: TextArea,
    /// Whether the composer was in `!` shell mode when the search opened. The
    /// mode is suspended during the search (previews show entries raw) and
    /// restored with the snapshot on cancel; accepting re-derives it from the
    /// accepted text instead. See `docs/shell-command.md`.
    snapshot_shell: bool,
    /// The footer-owned query typed while the search is active.
    pub query: String,
    /// The user-visible phase: drives the footer hints and the preview.
    pub state: SearchState,
}

/// Byte ranges in `text` where `query` matches case-insensitively — a port of
/// codex's `case_insensitive_match_ranges`: both sides are folded with
/// `char::to_lowercase`, and a span map keeps the folded match positions
/// aligned with the original bytes even when a fold changes length (e.g. `İ`
/// lowercases to two chars). Non-overlapping, left to right.
fn case_insensitive_match_ranges(text: &str, query: &str) -> Vec<Range<usize>> {
    let query_lower: String = query.chars().flat_map(char::to_lowercase).collect();
    if query_lower.is_empty() {
        return Vec::new();
    }
    // The folded text, and each folded char's originating byte range.
    let mut folded = String::new();
    let mut spans: Vec<(Range<usize>, Range<usize>)> = Vec::new();
    for (start, ch) in text.char_indices() {
        let original = start..start + ch.len_utf8();
        for lower in ch.to_lowercase() {
            let folded_start = folded.len();
            folded.push(lower);
            spans.push((folded_start..folded.len(), original.clone()));
        }
    }
    let mut ranges = Vec::new();
    let mut from = 0;
    while let Some(found) = folded.get(from..).and_then(|rest| rest.find(&query_lower)) {
        let fold = from + found..from + found + query_lower.len();
        let hit = |f: &Range<usize>| f.end > fold.start && f.start < fold.end;
        let first = spans.iter().find(|(f, _)| hit(f));
        let last = spans.iter().rev().find(|(f, _)| hit(f));
        if let (Some((_, first)), Some((_, last))) = (first, last) {
            ranges.push(first.start..last.end);
        }
        from = fold.end;
    }
    ranges
}

/// The phase of an open [`HistorySearch`] (codex's `HistorySearchStatus`,
/// minus `Searching` — we have no async persistent history to wait on).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchState {
    /// Empty query: nothing searched yet — the original draft still shows
    /// (opening Ctrl+R never previews the latest entry by itself).
    Idle,
    /// A match is previewed in the composer; `selected` indexes the
    /// newest-first unique match list ([`InputHistory::search`]).
    Match {
        /// Index into the current query's match list (0 = newest).
        selected: usize,
    },
    /// The query matches nothing: the original draft shows again, but the
    /// search stays open for more typing.
    NoMatch,
}

impl App {
    /// Should this ↑/↓ press browse [`input_history`] instead of moving the
    /// cursor?
    ///
    /// [`input_history`]: App::input_history
    pub(super) fn should_browse_history(&self) -> bool {
        let text = self.input.text();
        // A recalled `!command`'s bang lives in [`shell_mode`], not the text
        // (sync_shell_mode absorbed it), while the recorded entry keeps it —
        // reconstruct the `!`-prefixed form for the unedited-recall
        // comparison, mapping the composer's ends onto the recorded ends, or
        // browsing strands permanently on a shell entry. An empty shell
        // composer browses like any empty composer.
        //
        // [`shell_mode`]: App::shell_mode
        if self.shell_mode && !text.is_empty() {
            let recorded = format!("!{text}");
            let cursor = self.input.cursor();
            let cursor = if cursor == 0 { 0 } else { cursor + 1 };
            return self.input_history.should_navigate(&recorded, cursor);
        }
        self.input_history
            .should_navigate(text, self.input.cursor())
    }

    /// Replace the draft with a recalled history entry. `set_text` puts the
    /// cursor at the end (codex's recall placement), and the palette is
    /// re-derived so recalling a bare `/token` reopens it like typing one. The
    /// shell mode is re-derived from scratch — a recalled `!command` re-enters
    /// it (codex re-absorbs the bang), a plain entry leaves it.
    pub(super) fn recall_input(&mut self, text: &str) {
        self.shell_mode = false;
        // A recalled draft never pops the `@` picker (recall isn't typing an
        // `@token`); close any open one, like the search snapshot restore.
        self.file_search = None;
        let had_query = command_query(self.input.text()).is_some();
        self.input.set_text(text);
        self.refresh_command_menu(had_query);
        self.sync_shell_mode();
    }

    /// Open the Ctrl+R reverse history search: snapshot the draft — text and
    /// cursor (codex's `snapshot_draft`) — close the palette (the search owns
    /// the keys from here), and start Idle with an empty query: no preview
    /// until something is typed. See `docs/history-search.md`.
    pub(super) fn begin_history_search(&mut self) {
        self.command_menu = None;
        self.file_search = None; // the search owns the keys from here
        self.history_search = Some(HistorySearch {
            snapshot: self.input.clone(),
            snapshot_shell: self.shell_mode,
            query: String::new(),
            state: SearchState::Idle,
        });
        // Suspend shell mode while the search owns the composer: previewed
        // entries are raw text (`❯ !cmd`), not shell-mode drafts. Cancel
        // restores the flag with the snapshot; accept re-derives it.
        self.shell_mode = false;
    }

    /// Every key while the search is open — codex's
    /// `handle_history_search_key`: all of them are consumed here, so the
    /// normal composer handling (and the global Ctrl+C/Ctrl+O arms) never see
    /// a keystroke mid-search.
    pub(super) fn on_key_search(&mut self, key: KeyEvent) -> Action {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        // The global overlay toggle still works, but the search cancels first
        // so its preview never leaks into (or lingers under) the overlay.
        if ctrl && key.code == KeyCode::Char('o') {
            self.cancel_history_search();
            self.toggle_tool_view();
            return Action::ToggleToolView;
        }
        // Ctrl+R/↑ step to an older match, Ctrl+S/↓ back to a newer one.
        if (ctrl && key.code == KeyCode::Char('r')) || key.code == KeyCode::Up {
            self.step_history_search(true);
            return Action::None;
        }
        if (ctrl && key.code == KeyCode::Char('s')) || key.code == KeyCode::Down {
            self.step_history_search(false);
            return Action::None;
        }
        match key.code {
            // Esc and Ctrl+C cancel, restoring the snapshotted draft: Ctrl+C
            // neither clears it nor quits, and Esc never reaches the
            // interrupt/quit arms (the palette-dismiss precedent).
            KeyCode::Esc => {
                self.cancel_history_search();
                Action::None
            }
            KeyCode::Char('c') if ctrl => {
                self.cancel_history_search();
                Action::None
            }
            KeyCode::Enter => {
                self.accept_history_search();
                Action::None
            }
            // Backspace (or Ctrl+H, its classic alias) pops the query; Ctrl+U
            // clears it; plain characters extend it. Every edit restarts the
            // search from the newest entry.
            KeyCode::Backspace => self.edit_search_query(|query| {
                query.pop();
            }),
            KeyCode::Char('h') if ctrl => self.edit_search_query(|query| {
                query.pop();
            }),
            KeyCode::Char('u') if ctrl => self.edit_search_query(String::clear),
            KeyCode::Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.edit_search_query(|query| query.push(c))
            }
            // Everything else is swallowed (codex consumes unknown keys too).
            _ => Action::None,
        }
    }

    /// Apply `edit` to the query and re-run the search from the newest entry
    /// (codex's `update_history_search_query` — every query edit restarts).
    pub(super) fn edit_search_query(&mut self, edit: impl FnOnce(&mut String)) -> Action {
        if let Some(search) = self.history_search.as_mut() {
            edit(&mut search.query);
            self.rerun_history_search();
        }
        Action::None
    }

    /// Re-run the search after a query edit, restarting at the newest match.
    /// An empty query goes back to Idle with the original draft showing.
    fn rerun_history_search(&mut self) {
        let Some(search) = self.history_search.as_ref() else {
            return;
        };
        if search.query.is_empty() {
            self.restore_search_snapshot();
            if let Some(search) = self.history_search.as_mut() {
                search.state = SearchState::Idle;
            }
            return;
        }
        let matches = self.input_history.search(&search.query);
        self.show_search_match(&matches, 0);
    }

    /// Step the previewed match older/newer, clamping at both ends so the
    /// current match is kept (codex's `AtBoundary` — never a "no match"
    /// flicker at the end of the list). Stepping with an empty query stays
    /// Idle: opening Ctrl+R never previews the latest entry by itself.
    fn step_history_search(&mut self, older: bool) {
        let Some(search) = self.history_search.as_ref() else {
            return;
        };
        if search.query.is_empty() {
            return;
        }
        let matches = self.input_history.search(&search.query);
        let selected = match search.state {
            SearchState::Match { selected } if older => {
                (selected + 1).min(matches.len().saturating_sub(1))
            }
            SearchState::Match { selected } => selected.saturating_sub(1),
            // NoMatch: the matches are still empty (the query hasn't changed
            // since they came up empty), so this re-shows NoMatch below.
            _ => 0,
        };
        self.show_search_match(&matches, selected);
    }

    /// Preview match `selected` of `matches` (entry indices, newest first) in
    /// the composer — or show NoMatch when there is none: the original draft
    /// comes back but the search stays open for more typing (codex's
    /// `apply_history_search_result`).
    fn show_search_match(&mut self, matches: &[usize], selected: usize) {
        let entry = matches
            .get(selected)
            .and_then(|&index| self.input_history.entry(index))
            .map(str::to_string);
        match entry {
            Some(text) => {
                // Cursor at the end — codex's recall placement.
                self.input.set_text(&text);
                if let Some(search) = self.history_search.as_mut() {
                    search.state = SearchState::Match { selected };
                }
            }
            None => {
                self.restore_search_snapshot();
                if let Some(search) = self.history_search.as_mut() {
                    search.state = SearchState::NoMatch;
                }
            }
        }
    }

    /// Cancel the search, restoring the draft — text *and* cursor — from
    /// before it opened (codex's `cancel_history_search` → `restore_draft`).
    fn cancel_history_search(&mut self) {
        if let Some(search) = self.history_search.take() {
            self.input = search.snapshot;
            self.shell_mode = search.snapshot_shell;
            self.reconcile_images_with_input();
        }
    }

    /// Accept the previewed match (Enter): the search closes, the text stays
    /// as an ordinary editable draft (cursor already at the end), ↑/↓
    /// browsing is seated at the accepted entry — so ↑ continues *older* from
    /// it, codex's shared history cursor — and the palette re-derives like
    /// ↑-recall, so accepting a bare `/token` reopens it. Enter on
    /// Idle/NoMatch is swallowed: only an actual match accepts.
    fn accept_history_search(&mut self) {
        let Some(search) = self.history_search.as_ref() else {
            return;
        };
        let SearchState::Match { selected } = search.state else {
            return;
        };
        let entry = self
            .input_history
            .search(&search.query)
            .get(selected)
            .copied();
        let Some(entry) = entry else {
            return;
        };
        self.history_search = None;
        self.input_history.resume_at(entry);
        // The accepted entry replaced the pre-search draft, unanchoring the
        // pairs that backed it — discard them (the boundary deletes the
        // orphaned temp files) so a stale attachment can't silently ride the
        // next submission; any `[Image #N]` in the accepted text is an
        // unbacked marker (docs/image-paste.md). Mirrors the interrupt-undo
        // and backtrack draft-replacement paths.
        self.discard_attachments();
        self.refresh_command_menu(false);
        // An accepted `!entry` re-enters shell mode (the recall rule).
        self.sync_shell_mode();
    }

    /// Show the pre-search draft again without closing the search — what a
    /// no-match query (and a query cleared back to empty) displays.
    fn restore_search_snapshot(&mut self) {
        if let Some(search) = self.history_search.as_ref() {
            self.input = search.snapshot.clone();
            self.reconcile_images_with_input();
        }
    }

    /// Byte ranges of the query's occurrences in the composer text, **only
    /// while a match is previewed** — once the search closes the accepted text
    /// is an ordinary draft again, so this returns nothing (codex's
    /// `history_search_highlight_ranges`). `ui::render_live` styles these
    /// reversed+bold in the input box.
    #[must_use]
    pub fn search_highlight_ranges(&self) -> Vec<Range<usize>> {
        let Some(search) = self.history_search.as_ref() else {
            return Vec::new();
        };
        if !matches!(search.state, SearchState::Match { .. }) || search.query.is_empty() {
            return Vec::new();
        }
        case_insensitive_match_ranges(self.input.text(), &search.query)
    }
}
