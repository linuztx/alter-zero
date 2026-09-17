//! Pure state and keyboard navigation for the full-screen Git review.

use super::*;
use crate::git_diff::{DiffFile, DiffLineKind, DiffSection, DiffSnapshot};

/// Which changes the file browser lists.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DiffFilter {
    #[default]
    All,
    Unstaged,
    Staged,
    Untracked,
}

impl DiffFilter {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Unstaged => "Unstaged",
            Self::Staged => "Staged",
            Self::Untracked => "Untracked",
        }
    }

    fn includes(self, section: DiffSection) -> bool {
        matches!(
            (self, section),
            (Self::All, _)
                | (Self::Unstaged, DiffSection::Unstaged)
                | (Self::Staged, DiffSection::Staged)
                | (Self::Untracked, DiffSection::Untracked)
        )
    }
}

/// The pane receiving movement keys; Tab switches panes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DiffFocus {
    #[default]
    Files,
    Patch,
}

/// One review's snapshot and viewport. No filesystem access lives here.
#[derive(Debug, Clone, Default)]
pub struct DiffReview {
    pub snapshot: Option<DiffSnapshot>,
    pub loading: bool,
    pub error: Option<String>,
    pub filter: DiffFilter,
    pub query: String,
    pub searching: bool,
    pub selected: usize,
    pub focus: DiffFocus,
    pub scroll: usize,
    pub horizontal: usize,
    page_rows: usize,
}

impl DiffReview {
    /// Filter against the complete relative path, including a rename's source.
    #[must_use]
    pub fn visible_files(&self) -> Vec<&DiffFile> {
        let query = self.query.to_lowercase();
        self.snapshot
            .iter()
            .flat_map(|snapshot| &snapshot.files)
            .filter(|file| {
                self.filter.includes(file.section)
                    && (file.path.to_string_lossy().to_lowercase().contains(&query)
                        || file.old_path.as_ref().is_some_and(|path| {
                            path.to_string_lossy().to_lowercase().contains(&query)
                        }))
            })
            .collect()
    }

    #[must_use]
    pub fn selected_file(&self) -> Option<&DiffFile> {
        self.visible_files().get(self.selected).copied()
    }

    fn reset_position(&mut self) {
        self.selected = 0;
        self.scroll = 0;
        self.horizontal = 0;
    }

    fn select(&mut self, selected: usize) {
        let count = self.visible_files().len();
        self.selected = selected.min(count.saturating_sub(1));
        self.scroll = 0;
        self.horizontal = 0;
    }

    fn max_scroll(&self) -> usize {
        self.selected_file().map_or(0, |file| {
            file.lines.len().saturating_sub(self.page_rows.max(1))
        })
    }

    fn move_rows(&mut self, down: bool, rows: usize) {
        if self.focus == DiffFocus::Files {
            let next = if down {
                self.selected.saturating_add(rows)
            } else {
                self.selected.saturating_sub(rows)
            };
            self.select(next);
        } else {
            self.scroll = if down {
                self.scroll.saturating_add(rows).min(self.max_scroll())
            } else {
                self.scroll.saturating_sub(rows)
            };
        }
    }

    fn jump_hunk(&mut self, next: bool) {
        let Some(file) = self.selected_file() else {
            return;
        };
        let mut hunks = file
            .lines
            .iter()
            .enumerate()
            .filter_map(|(row, line)| (line.kind == DiffLineKind::Hunk).then_some(row));
        let target = if next {
            hunks.find(|row| *row > self.scroll)
        } else {
            hunks.rfind(|row| *row < self.scroll)
        };
        if let Some(target) = target {
            self.scroll = target.min(self.max_scroll());
            self.focus = DiffFocus::Patch;
        }
    }
}

impl App {
    #[must_use]
    pub fn diff_review(&self) -> Option<&DiffReview> {
        self.diff_review.as_ref()
    }

    /// Start a new review. The boundary supplies its snapshot asynchronously.
    pub fn open_diff_review(&mut self) {
        self.diff_review = Some(DiffReview {
            loading: true,
            page_rows: 1,
            ..DiffReview::default()
        });
        self.view = View::DiffReview;
    }

    pub fn close_diff_review(&mut self) {
        self.diff_review = None;
        self.view = View::Conversation;
    }

    /// Preserve the selected path/section and clamp after a refresh or resize.
    pub fn set_diff_snapshot(&mut self, snapshot: DiffSnapshot) {
        let Some(review) = self.diff_review.as_mut() else {
            return;
        };
        let previous = review
            .selected_file()
            .map(|file| (file.path.clone(), file.section));
        review.snapshot = Some(snapshot);
        review.horizontal = 0;
        review.loading = false;
        review.error = None;
        let files = review.visible_files();
        let found = previous.as_ref().and_then(|(path, section)| {
            files
                .iter()
                .position(|file| file.path == *path && file.section == *section)
        });
        if let Some(selected) = found {
            review.selected = selected;
        } else {
            review.select(review.selected);
        }
        review.scroll = review.scroll.min(review.max_scroll());
    }

    pub fn set_diff_error(&mut self, error: String) {
        if let Some(review) = self.diff_review.as_mut() {
            review.loading = false;
            review.error = Some(error);
        }
    }

    /// Geometry is injected by the renderer's boundary, keeping keys pure.
    pub fn settle_diff_scroll(&mut self, page_rows: usize) {
        if let Some(review) = self.diff_review.as_mut() {
            review.page_rows = page_rows.max(1);
            review.scroll = review.scroll.min(review.max_scroll());
        }
    }

    /// Bracketed paste belongs only to the filename search, never the composer.
    pub fn paste_into_diff_search(&mut self, text: &str) {
        if let Some(review) = self.diff_review.as_mut().filter(|review| review.searching) {
            review
                .query
                .extend(text.chars().filter(|c| !c.is_control()));
            review.reset_position();
        }
    }

    pub(super) fn on_key_diff_review(&mut self, key: KeyEvent) -> Action {
        let Some(review) = self.diff_review.as_mut() else {
            self.view = View::Conversation;
            return Action::CloseDiffReview;
        };
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.close_diff_review();
            return Action::CloseDiffReview;
        }
        if review.searching {
            match key.code {
                KeyCode::Esc => {
                    review.searching = false;
                    review.query.clear();
                    review.reset_position();
                }
                KeyCode::Enter => review.searching = false,
                KeyCode::Backspace => {
                    review.query.pop();
                    review.reset_position();
                }
                KeyCode::Char(c)
                    if !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                {
                    review.query.push(c);
                    review.reset_position();
                }
                _ => {}
            }
            return Action::None;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            return Action::None;
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.close_diff_review();
                return Action::CloseDiffReview;
            }
            KeyCode::Char('r') if !review.loading => {
                review.loading = true;
                review.error = None;
                return Action::RefreshDiffReview;
            }
            KeyCode::Char('/') => review.searching = true,
            KeyCode::Tab | KeyCode::BackTab => {
                review.focus = match review.focus {
                    DiffFocus::Files => DiffFocus::Patch,
                    DiffFocus::Patch => DiffFocus::Files,
                };
            }
            KeyCode::Enter => review.focus = DiffFocus::Patch,
            KeyCode::Char(c @ '1'..='4') => {
                review.filter = match c {
                    '2' => DiffFilter::Unstaged,
                    '3' => DiffFilter::Staged,
                    '4' => DiffFilter::Untracked,
                    _ => DiffFilter::All,
                };
                review.reset_position();
            }
            KeyCode::Up | KeyCode::Char('k') => review.move_rows(false, 1),
            KeyCode::Down | KeyCode::Char('j') => review.move_rows(true, 1),
            KeyCode::PageUp | KeyCode::PageDown => {
                let rows = match review.focus {
                    DiffFocus::Files => (review.page_rows / 2).max(1),
                    DiffFocus::Patch => review.page_rows,
                };
                review.move_rows(key.code == KeyCode::PageDown, rows);
            }
            KeyCode::Home | KeyCode::Char('g') => review.move_rows(false, usize::MAX),
            KeyCode::End | KeyCode::Char('G') => review.move_rows(true, usize::MAX),
            KeyCode::Left | KeyCode::Char('h') => {
                review.horizontal = review.horizontal.saturating_sub(4)
            }
            KeyCode::Right | KeyCode::Char('l') => {
                let max = review.selected_file().map_or(0, |file| {
                    file.lines
                        .iter()
                        .map(|line| {
                            line.text
                                .chars()
                                .map(|c| match c {
                                    '\t' => 4,
                                    '\n' | '\r' => 2,
                                    c if c.is_control() => c.escape_default().count(),
                                    c => unicode_width::UnicodeWidthChar::width(c).unwrap_or(0),
                                })
                                .sum::<usize>()
                        })
                        .max()
                        .unwrap_or(0)
                });
                review.horizontal = review.horizontal.saturating_add(4).min(max);
            }
            KeyCode::Char(']') => review.select(review.selected.saturating_add(1)),
            KeyCode::Char('[') => review.select(review.selected.saturating_sub(1)),
            KeyCode::Char('n') => review.jump_hunk(true),
            KeyCode::Char('N') => review.jump_hunk(false),
            _ => {}
        }
        Action::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git_diff::parse_patch;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn snapshot() -> DiffSnapshot {
        DiffSnapshot {
            root: PathBuf::from("/project"),
            branch: "main".into(),
            files: [
                ("src/a.rs", DiffSection::Unstaged),
                ("src/a.rs", DiffSection::Staged),
                ("notes.md", DiffSection::Untracked),
            ]
            .into_iter()
            .map(|(path, section)| DiffFile {
                path: path.into(),
                old_path: None,
                section,
                status: "M".into(),
                additions: 2,
                deletions: 1,
                lines: parse_patch(
                    "@@ -1,2 +1,2 @@\n context\n-old\n+new\n@@ -9 +9 @@\n-end\n+last\n",
                ),
                truncated: false,
            })
            .collect(),
        }
    }

    fn ready() -> App {
        let mut app = App::new();
        app.open_diff_review();
        app.set_diff_snapshot(snapshot());
        app.settle_diff_scroll(2);
        app
    }

    #[test]
    fn slash_diff_dispatches_without_starting_a_turn() {
        let mut app = App::new();
        for c in "/diff".chars() {
            app.on_key(key(KeyCode::Char(c)));
        }
        assert_eq!(app.on_key(key(KeyCode::Enter)), Action::OpenDiffReview);
        assert!(!app.turn_active());
        assert!(app.input.text().is_empty());
    }

    #[test]
    fn filters_keep_index_and_worktree_as_distinct_entries() {
        let mut app = ready();
        assert_eq!(app.diff_review().unwrap().visible_files().len(), 3);
        app.on_key(key(KeyCode::Char('3')));
        let review = app.diff_review().unwrap();
        assert_eq!(review.visible_files().len(), 1);
        assert_eq!(review.selected_file().unwrap().section, DiffSection::Staged);
        app.on_key(key(KeyCode::Char('4')));
        assert_eq!(
            app.diff_review().unwrap().selected_file().unwrap().path,
            PathBuf::from("notes.md")
        );
        app.on_key(key(KeyCode::Char('1')));
        assert_eq!(app.diff_review().unwrap().visible_files().len(), 3);
    }

    #[test]
    fn search_owns_text_and_escape_before_view_keys() {
        let mut app = ready();
        app.on_key(key(KeyCode::Char('/')));
        app.paste_into_diff_search("NOTES\n");
        app.on_key(key(KeyCode::Char('q')));
        assert_eq!(app.diff_review().unwrap().query, "NOTESq");
        assert!(app.diff_review().unwrap().visible_files().is_empty());
        app.on_key(key(KeyCode::Backspace));
        assert_eq!(app.diff_review().unwrap().visible_files().len(), 1);
        app.on_key(key(KeyCode::Esc));
        assert_eq!(app.view, View::DiffReview);
        assert!(app.diff_review().unwrap().query.is_empty());
        assert!(app.input.text().is_empty());
        assert_eq!(app.on_key(key(KeyCode::Esc)), Action::CloseDiffReview);
        assert_eq!(app.view, View::Conversation);
        assert!(app.diff_review().is_none());
    }

    #[test]
    fn pane_navigation_hunks_and_resize_clamp_the_patch() {
        let mut app = ready();
        app.on_key(key(KeyCode::Down));
        assert_eq!(app.diff_review().unwrap().selected, 1);
        app.on_key(key(KeyCode::Enter));
        app.on_key(key(KeyCode::Char('n')));
        assert_eq!(app.diff_review().unwrap().scroll, 4);
        app.on_key(key(KeyCode::Char('N')));
        assert_eq!(app.diff_review().unwrap().scroll, 0);
        app.on_key(key(KeyCode::End));
        assert_eq!(app.diff_review().unwrap().scroll, 5);
        app.settle_diff_scroll(100);
        assert_eq!(app.diff_review().unwrap().scroll, 0);
        app.on_key(key(KeyCode::Char(']')));
        assert_eq!(app.diff_review().unwrap().selected, 2);
        app.on_key(key(KeyCode::Char(']')));
        assert_eq!(app.diff_review().unwrap().selected, 2);
    }

    #[test]
    fn refresh_preserves_identity_and_missing_selection_is_clamped() {
        let mut app = ready();
        app.on_key(key(KeyCode::Down));
        assert_eq!(
            app.on_key(key(KeyCode::Char('r'))),
            Action::RefreshDiffReview
        );
        assert_eq!(app.on_key(key(KeyCode::Char('r'))), Action::None);
        let mut updated = snapshot();
        updated.files.remove(0);
        app.set_diff_snapshot(updated);
        assert_eq!(app.diff_review().unwrap().selected, 0);
        assert_eq!(
            app.diff_review().unwrap().selected_file().unwrap().section,
            DiffSection::Staged
        );
        let mut empty = snapshot();
        empty.files.clear();
        app.set_diff_snapshot(empty);
        for code in [
            KeyCode::Down,
            KeyCode::End,
            KeyCode::PageDown,
            KeyCode::Char('n'),
            KeyCode::Right,
        ] {
            app.on_key(key(code));
        }
        assert!(app.diff_review().unwrap().selected_file().is_none());
        assert_eq!(app.diff_review().unwrap().scroll, 0);
    }

    #[test]
    fn loading_review_can_close_and_ctrl_keys_do_not_leak() {
        let mut app = App::new();
        app.open_diff_review();
        for c in ['o', 'd'] {
            assert_eq!(
                app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)),
                Action::None
            );
            assert_eq!(app.view, View::DiffReview);
        }
        assert_eq!(
            app.on_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Action::CloseDiffReview
        );
        app.set_diff_snapshot(snapshot());
        assert!(app.diff_review().is_none());
        assert_eq!(app.view, View::Conversation);
    }
}
