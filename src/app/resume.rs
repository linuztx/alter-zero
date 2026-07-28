//! The `/resume` session picker: its filter/sort toolbar state, the search
//! query, and loading a rollout file back into history. See `docs/resume.md`.

use super::views::TOOL_VIEW_PAGE;
use super::*;

/// The `/resume` picker's sort key — codex's `Sort: [Updated] Created`
/// toolbar tab. `Updated` (the default) orders by file mtime, so a resumed
/// old session floats back up; `Created` by session start.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ResumeSort {
    /// Newest-modified first (codex's default).
    #[default]
    Updated,
    /// Newest-started first.
    Created,
}

/// The `/resume` picker's directory filter — codex's `Filter: [Cwd] All`
/// toolbar tab. `Cwd` (the default) lists only sessions whose meta recorded
/// the picker's own working directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ResumeFilter {
    /// Only sessions recorded in this working directory (codex's default).
    #[default]
    Cwd,
    /// Every saved session.
    All,
}

/// Which toolbar control ←/→ act on — codex's Tab-cycled `ToolbarControl`.
/// Two controls, so Tab and BackTab both just swap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ResumeControl {
    /// The `Filter: [Cwd] All` tab pair (the initial focus, codex's).
    #[default]
    Filter,
    /// The `Sort: [Updated] Created` tab pair.
    Sort,
}

/// The open `/resume` session picker ([`View::ResumePicker`]): the saved
/// sessions the boundary scanned when it opened, the highlighted row, the
/// type-to-search query, and the Filter/Sort toolbar — codex's
/// `resume_picker.rs` picker state, sized down (see `docs/resume.md`). The
/// filtered rows derive on demand ([`matches`], the palette's
/// `matching_commands` pattern); `selected` indexes that filtered list.
///
/// [`matches`]: ResumePicker::matches
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ResumePicker {
    /// Every eligible saved session, as scanned at open (mtime order; the
    /// active [`sort`] re-orders the derived rows). The seconds-ago values
    /// are frozen from that scan.
    ///
    /// [`sort`]: ResumePicker::sort
    pub sessions: Vec<SessionSummary>,
    /// Index of the highlighted row within the current filtered matches.
    pub selected: usize,
    /// The type-to-search query — any plain printable key appends, Backspace
    /// pops, Esc clears (codex's always-on picker search).
    pub query: String,
    /// The picker's own working directory, in the meta-line format — what
    /// the `Cwd` filter compares each session's recorded cwd against.
    pub cwd: String,
    /// The active directory filter (`Cwd` default — codex's).
    pub filter: ResumeFilter,
    /// The active sort key (`Updated` default — codex's).
    pub sort: ResumeSort,
    /// The toolbar control Tab focus is on (←/→ toggle its value).
    pub focus: ResumeControl,
}

impl ResumePicker {
    /// The rows the picker shows: the [`filter`]-passing sessions matching
    /// the query (a case-insensitive substring test on the preview — codex's
    /// client-side `Row::matches_query`; every session for an empty query),
    /// ordered by the active [`sort`] key, newest first.
    ///
    /// [`filter`]: ResumePicker::filter
    /// [`sort`]: ResumePicker::sort
    #[must_use]
    pub fn matches(&self) -> Vec<&SessionSummary> {
        let query = self.query.to_lowercase();
        let mut rows: Vec<&SessionSummary> = self
            .sessions
            .iter()
            .filter(|session| self.filter == ResumeFilter::All || session.cwd == self.cwd)
            .filter(|session| session.preview.to_lowercase().contains(&query))
            .collect();
        // Seconds-ago ascending = newest first; the sort is stable, so
        // same-second ties keep the scan's mtime order.
        match self.sort {
            ResumeSort::Updated => rows.sort_by_key(|session| session.updated_secs),
            ResumeSort::Created => rows.sort_by_key(|session| session.created_secs),
        }
        rows
    }
}

/// How many rows PageUp/PageDown move the `/resume` picker (the tool view's
/// page stride).
const RESUME_PAGE: usize = TOOL_VIEW_PAGE;

impl App {
    /// Open the `/resume` picker over `sessions` (the boundary's scan of the
    /// sessions dir, newest first) — swaps to [`View::ResumePicker`] on the
    /// alternate screen. `cwd` (the meta-line format) seeds the default `Cwd`
    /// filter. Any `?` band or in-flight backtrack gesture is abandoned, like
    /// the Ctrl+O toggle. See `docs/resume.md`.
    pub fn open_resume_picker(&mut self, sessions: Vec<SessionSummary>, cwd: String) {
        self.shortcuts_open = false;
        self.backtrack = Backtrack::default();
        self.resume_picker = Some(ResumePicker {
            sessions,
            cwd,
            ..ResumePicker::default()
        });
        self.view = View::ResumePicker;
    }

    /// Dismiss the `/resume` picker (Esc/Ctrl+C, a failed load, or right
    /// after a successful one): back to the conversation view. The loop
    /// leaves the alternate screen and repaints, the Ctrl+O return.
    pub fn close_resume_picker(&mut self) {
        self.resume_picker = None;
        self.view = View::Conversation;
    }

    /// Install a loaded session as the conversation — the `/resume` swap.
    /// The `/clear` reset shape ([`clear_conversation`]: wipe the streaming
    /// buffer/tool/status, drain the queue into discarded images) with
    /// `items` as the new history, and the picker closed. A turn can't be
    /// *active* here (`/resume` is rejected mid-task) — the wipes are
    /// belt-and-braces. The composer draft, its attachments, and the ↑-recall
    /// history survive, like `/clear`. See `docs/resume.md`.
    ///
    /// [`clear_conversation`]: App::clear_conversation
    pub fn load_session(&mut self, items: Vec<HistoryItem>) {
        self.clear_conversation();
        self.history = items;
        // A rollout cut short mid-turn ends with its user message; that tail
        // belongs to the resumed conversation, not to any new turn — fence it
        // off from the interrupt-undo (docs/interrupt.md).
        self.undo_floor = self.history.len();
        // The gauge re-seats on the loaded conversation (docs/compact.md).
        self.refresh_context_used();
        self.close_resume_picker();
    }

    /// Keys while the `/resume` session picker is showing — codex's picker
    /// key handling, sized down: ↑/↓ move the highlight (clamped),
    /// PageUp/PageDown jump by [`RESUME_PAGE`], Home/End jump to the ends,
    /// Enter resumes the highlighted session, and Esc clears a non-empty
    /// search before it closes anything. Any plain printable character types
    /// into the search (Backspace pops) — navigation lives on the
    /// non-printable keys, codex's `allow_plain_char_navigation`. Ctrl+C and
    /// Ctrl+O are handled globally in [`on_key`] (close / inert).
    ///
    /// [`on_key`]: App::on_key
    pub(super) fn on_key_resume_picker(&mut self, key: KeyEvent) -> Action {
        let Some(picker) = self.resume_picker.as_mut() else {
            return Action::None;
        };
        let last = picker.matches().len().saturating_sub(1);
        match key.code {
            KeyCode::Up => picker.selected = picker.selected.saturating_sub(1),
            KeyCode::Down => picker.selected = (picker.selected + 1).min(last),
            KeyCode::PageUp => picker.selected = picker.selected.saturating_sub(RESUME_PAGE),
            KeyCode::PageDown => picker.selected = (picker.selected + RESUME_PAGE).min(last),
            KeyCode::Home => picker.selected = 0,
            KeyCode::End => picker.selected = last,
            KeyCode::Enter => {
                // Resume the highlighted (filtered) row; an empty list has
                // nothing to resume and the picker stays up.
                if let Some(selected) = picker.matches().get(picker.selected) {
                    return Action::ResumeSession(selected.path.clone());
                }
            }
            KeyCode::Esc => {
                if picker.query.is_empty() {
                    self.close_resume_picker();
                    return Action::CloseResumePicker;
                }
                // Esc clears the search first (codex); the next Esc closes.
                picker.query.clear();
                picker.selected = 0;
            }
            KeyCode::Backspace => {
                picker.query.pop();
                picker.selected = 0;
            }
            // The Filter/Sort toolbar (codex's): Tab moves the focus between
            // the two controls (BackTab too — prev == next with two), and
            // ←/→ toggle the focused control's value, reseating the
            // selection like a query edit (the rows re-derive).
            KeyCode::Tab | KeyCode::BackTab => {
                picker.focus = match picker.focus {
                    ResumeControl::Filter => ResumeControl::Sort,
                    ResumeControl::Sort => ResumeControl::Filter,
                };
            }
            KeyCode::Left | KeyCode::Right => {
                match picker.focus {
                    ResumeControl::Filter => {
                        picker.filter = match picker.filter {
                            ResumeFilter::Cwd => ResumeFilter::All,
                            ResumeFilter::All => ResumeFilter::Cwd,
                        };
                    }
                    ResumeControl::Sort => {
                        picker.sort = match picker.sort {
                            ResumeSort::Updated => ResumeSort::Created,
                            ResumeSort::Created => ResumeSort::Updated,
                        };
                    }
                }
                picker.selected = 0;
            }
            // Plain printable characters are search input, never navigation
            // (so `q`/`j`/`k` filter instead of closing/moving — codex).
            KeyCode::Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                picker.query.push(c);
                picker.selected = 0;
            }
            _ => {}
        }
        Action::None
    }

    /// A bracketed paste while the `/resume` picker is up: the text joins the
    /// type-to-search query — codex's `normalize_pasted_search_query` —
    /// whitespace runs collapsed to single spaces (so a multiline paste stays
    /// one query), a non-empty query gaining a separating space, and a
    /// whitespace-only paste ignored. Reseats the selection like typed input.
    pub fn paste_into_resume_search(&mut self, pasted: &str) {
        let flat = pasted.split_whitespace().collect::<Vec<_>>().join(" ");
        if flat.is_empty() {
            return;
        }
        let Some(picker) = self.resume_picker.as_mut() else {
            return;
        };
        if !picker.query.is_empty() {
            picker.query.push(' ');
        }
        picker.query.push_str(&flat);
        picker.selected = 0;
    }
}
