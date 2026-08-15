//! The `@` file picker's [`App`](super::App) state: the query round-trip with
//! the boundary's background walker and the Tab/Enter path insertion.
//! See `docs/file-search.md`.

use super::*;

/// The open `@` file picker (when the cursor is in an `@token`); `None` when
/// closed. Unlike the slash palette — whose matches derive from the input —
/// these come from the **filesystem** asynchronously, so they're stored here.
/// The boundary dispatches a search whenever [`App::file_search_query`] changes
/// and feeds results back via [`App::set_file_matches`]. See
/// `docs/file-search.md`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FileSearch {
    /// Index of the highlighted match.
    pub selected: usize,
    /// The `@token` query the current `matches` are for (staleness guard).
    pub query: String,
    /// The ranked file matches for `query`, capped by the boundary.
    pub matches: Vec<FileMatch>,
    /// A search for the current query is in flight (show *Searching…*).
    pub waiting: bool,
}

impl App {
    /// Is the cursor currently inside an `@token`? (The `had_token` state the
    /// editing arms snapshot *before* a change so [`refresh_file_search`]'s
    /// Esc-sticky logic mirrors the palette's `had_query`.)
    ///
    /// [`refresh_file_search`]: App::refresh_file_search
    pub(super) fn in_at_token(&self) -> bool {
        at_token(self.input.text(), self.input.cursor()).is_some()
    }

    /// Re-derive the `@` file picker after an edit — the palette's
    /// [`refresh_command_menu`] logic, applied to [`at_token`]. Opens it when the
    /// cursor *enters* an `@token` (the None→Some transition, so an Esc-dismiss
    /// stays dismissed within the same token), updates the query as it changes
    /// (marking it `waiting` and resetting the highlight so the boundary's next
    /// results refresh the band), and closes it when the token's gone. Suppressed
    /// in shell mode (a `!command` draft is a literal command).
    ///
    /// [`refresh_command_menu`]: App::refresh_command_menu
    pub(super) fn refresh_file_search(&mut self, had_token: bool) {
        if self.shell_mode {
            self.file_search = None;
            return;
        }
        match at_token(self.input.text(), self.input.cursor()) {
            None => self.file_search = None,
            Some(tok) => match &mut self.file_search {
                Some(fs) if fs.query != tok.query => {
                    fs.query = tok.query;
                    fs.waiting = true;
                    fs.selected = 0;
                }
                Some(_) => {}
                // Just entered an `@token` → open at the top.
                None if !had_token => {
                    self.file_search = Some(FileSearch {
                        selected: 0,
                        query: tok.query,
                        matches: Vec::new(),
                        waiting: true,
                    });
                }
                // Dismissed earlier and still in the same token → stay closed.
                None => {}
            },
        }
    }

    /// The active `@token` query the boundary should search for — `None` when the
    /// picker is closed (or in shell mode). The loop dispatches a file search
    /// whenever this changes; see `docs/file-search.md`.
    #[must_use]
    pub fn file_search_query(&self) -> Option<String> {
        if self.file_search.is_none() || self.shell_mode {
            return None;
        }
        at_token(self.input.text(), self.input.cursor()).map(|t| t.query)
    }

    /// Feed asynchronously-fetched file matches into the open picker — codex's
    /// `on_file_search_result`. Stale results (the token moved on since the
    /// search was dispatched) are dropped; otherwise they replace the band's
    /// matches, clear the `waiting` flag, and clamp the highlight.
    pub fn set_file_matches(&mut self, query: &str, matches: Vec<FileMatch>) {
        // Compare against the *live* token so a result that raced the user's
        // typing is discarded (the loop also re-dispatches on every change).
        let active = at_token(self.input.text(), self.input.cursor()).map(|t| t.query);
        if active.as_deref() != Some(query) {
            return;
        }
        if let Some(fs) = &mut self.file_search {
            fs.selected = fs.selected.min(matches.len().saturating_sub(1));
            fs.matches = matches;
            fs.query = query.to_string();
            fs.waiting = false;
        }
    }

    /// Move the file-picker highlight one step over the current matches,
    /// wrapping at the ends (`wrap_step` — the palette's grammar).
    pub(super) fn move_file_selection(&mut self, delta: isize) {
        if let Some(fs) = &mut self.file_search {
            fs.selected = wrap_step(fs.selected, fs.matches.len(), delta);
        }
    }

    /// The file match currently highlighted in the picker, if one is.
    #[must_use]
    pub fn highlighted_file(&self) -> Option<&FileMatch> {
        let fs = self.file_search.as_ref()?;
        fs.matches.get(fs.selected)
    }

    /// Accept the highlighted file: replace the `@token` under the cursor with the
    /// path plus a trailing space (codex's `insert_selected_path`; paths with
    /// whitespace are quoted), and close the picker. `Action::None` if nothing is
    /// highlighted.
    pub(super) fn accept_file_selection(&mut self) -> Action {
        let Some(path) = self.highlighted_file().map(|m| m.path.clone()) else {
            return Action::None;
        };
        if let Some(tok) = at_token(self.input.text(), self.input.cursor()) {
            let inserted = if path.chars().any(char::is_whitespace) && !path.contains('"') {
                format!("\"{path}\"")
            } else {
                path
            };
            self.input.replace_range(tok.range, &format!("{inserted} "));
        }
        self.file_search = None;
        Action::None
    }
}
