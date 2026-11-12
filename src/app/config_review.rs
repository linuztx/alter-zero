//! The startup review: the dialog that asks about hooks and project MCP
//! servers before they run (`docs/state/config-review.md`).
//!
//! A composer-replacing inline page in the `/hooks` family — no text entry,
//! the cursor hidden on its `❯`, every key owned while open — over pages the
//! boundary builds from what it loaded (the `open_hooks_menu` injection
//! seam): the hooks waiting for trust first, then the project's servers
//! waiting for a decision. Each page is a checkbox list grouped by the file
//! its items came from, over two action rows. Answering a page returns
//! [`Action::ReviewAnswered`] and moves on to the next; what an answer means
//! — what is recorded, what starts running — is the boundary's to apply.

use super::*;

use crate::trust::HookIdentity;

/// What a review page decides on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewKind {
    /// Hooks waiting for trust — the user's own and the project's.
    Hooks,
    /// The project's MCP servers waiting for a decision.
    McpServers,
}

/// Which thing an item is, so the boundary can record its answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewKey {
    /// A hook: the absolute path of the `hooks.json` it is configured in,
    /// and the hook itself.
    Hook {
        source: String,
        identity: HookIdentity,
    },
    /// A project server: the absolute path of the file declaring it, and
    /// its name.
    Server { source: String, name: String },
}

/// One thing to decide on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewItem {
    pub key: ReviewKey,
    /// The item's title: `PreToolUse · bash`, `github · http`.
    pub title: String,
    /// What runs, verbatim, one entry per line: a hook's command; a
    /// server's command line or URL, then its `env`.
    pub detail: Vec<String>,
    /// How a **changed** item changed. `None` for a new one.
    pub change: Option<ItemChange>,
    /// Whether it is selected.
    pub checked: bool,
}

/// How a changed item says what it was.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ItemChange {
    /// A hook's edit: the command it ran before — its `was:` line.
    Was(String),
    /// A server entry's edit: what changed, each part in words
    /// ([`crate::trust::ServerChange::describe`]) — its `changed:` line, or
    /// only the badge when nothing can be named. The entry itself is never
    /// kept, so this is all there is to say.
    Parts(Vec<String>),
}

/// The items one file contributes, under its heading.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewSection {
    /// The scope word ahead of the path (`User`, `Project`), when the page
    /// mixes scopes.
    pub scope: Option<String>,
    /// The file's display path.
    pub path: String,
    pub items: Vec<ReviewItem>,
}

/// The two action rows under every page's items.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewChoice {
    /// `Trust selected and continue` / `Enable selected`.
    Confirm,
    /// `Continue without trusting` / `Disable all`.
    Decline,
}

/// One page of the review.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewPage {
    pub kind: ReviewKind,
    pub sections: Vec<ReviewSection>,
    /// The highlighted row: an item (counted across the sections), or past
    /// the items, one of the two action rows.
    pub selected: usize,
}

impl ReviewPage {
    /// A page over `sections`, its first row highlighted.
    #[must_use]
    pub const fn new(kind: ReviewKind, sections: Vec<ReviewSection>) -> Self {
        Self {
            kind,
            sections,
            selected: 0,
        }
    }

    /// Every item, in display order.
    pub fn items(&self) -> impl Iterator<Item = &ReviewItem> {
        self.sections
            .iter()
            .flat_map(|section| section.items.iter())
    }

    /// How many items the page asks about.
    #[must_use]
    pub fn item_count(&self) -> usize {
        self.sections
            .iter()
            .map(|section| section.items.len())
            .sum()
    }

    /// The page's rows: every item, then the two actions.
    #[must_use]
    pub fn row_count(&self) -> usize {
        self.item_count() + 2
    }

    /// The action on row `row`, or `None` for an item's row.
    #[must_use]
    pub fn action_at(&self, row: usize) -> Option<ReviewChoice> {
        match row.checked_sub(self.item_count()) {
            Some(0) => Some(ReviewChoice::Confirm),
            Some(1) => Some(ReviewChoice::Decline),
            _ => None,
        }
    }

    /// How many items are new and how many changed — the page's title.
    #[must_use]
    pub fn counts(&self) -> (usize, usize) {
        let changed = self.items().filter(|item| item.change.is_some()).count();
        (self.item_count() - changed, changed)
    }

    fn item_mut(&mut self, index: usize) -> Option<&mut ReviewItem> {
        self.sections
            .iter_mut()
            .flat_map(|section| section.items.iter_mut())
            .nth(index)
    }

    /// The answer this page gives, `confirmed` or not.
    fn answer(&self, confirmed: bool) -> ReviewAnswer {
        ReviewAnswer {
            kind: self.kind,
            confirmed,
            choices: self
                .items()
                .map(|item| (item.key.clone(), item.checked))
                .collect(),
        }
    }
}

/// The open review (`None` on [`App`] when closed): the page on screen
/// first, the pages after it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigReview {
    pub pages: Vec<ReviewPage>,
    /// How many pages the review has had — the `1/2` beside a title.
    pub total: usize,
    /// Whether the user asked for it — **Hooks** turned on in `/settings`
    /// over hooks waiting for trust — rather than the session opening it at
    /// startup. Its Ctrl+C then closes it the pickers' way instead of
    /// quitting.
    pub requested: bool,
}

impl ConfigReview {
    /// Which page of [`total`](Self::total) is on screen, from 1.
    #[must_use]
    pub fn position(&self) -> usize {
        self.total + 1 - self.pages.len()
    }
}

/// How the user answered one page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewAnswer {
    pub kind: ReviewKind,
    /// `true` — the selection stands (Enter on an item or the first action
    /// row). `false` — declined (the second action row, Esc, Ctrl+C).
    pub confirmed: bool,
    /// Every item the page asked about, and whether it was selected.
    pub choices: Vec<(ReviewKey, bool)>,
}

impl App {
    /// Open the review over `pages` — the startup review, which the session
    /// opens by itself — abandoning the bands that share the composer like
    /// every picker. A page with nothing on it is dropped, and a review with
    /// nothing left opens nothing. Over a review already open the new pages
    /// queue behind the one on screen — each kind once.
    pub fn open_config_review(&mut self, pages: Vec<ReviewPage>) {
        self.open_review(pages, false);
    }

    /// [`open_config_review`](Self::open_config_review) for a review the
    /// user asked for ([`ConfigReview::requested`]).
    pub fn open_requested_review(&mut self, pages: Vec<ReviewPage>) {
        self.open_review(pages, true);
    }

    fn open_review(&mut self, pages: Vec<ReviewPage>, requested: bool) {
        let mut pages: Vec<ReviewPage> = pages
            .into_iter()
            .filter(|page| page.item_count() > 0)
            .collect();
        if let Some(review) = &mut self.config_review {
            pages.retain(|page| review.pages.iter().all(|queued| queued.kind != page.kind));
            review.total += pages.len();
            review.pages.extend(pages);
            return;
        }
        if pages.is_empty() {
            return;
        }
        self.shortcuts_open = false;
        self.command_menu = None;
        self.file_search = None;
        self.skill_picker = None;
        self.backtrack = Backtrack::default();
        self.config_review = Some(ConfigReview {
            total: pages.len(),
            pages,
            requested,
        });
    }

    /// Answer the page on screen and move on to the next — the last one
    /// closes the review.
    fn answer_review_page(&mut self, confirmed: bool) -> Action {
        let Some(review) = self.config_review.as_mut() else {
            return Action::None;
        };
        if review.pages.is_empty() {
            self.config_review = None;
            return Action::None;
        }
        let page = review.pages.remove(0);
        if review.pages.is_empty() {
            self.config_review = None;
        }
        Action::ReviewAnswered(page.answer(confirmed))
    }

    /// Keys while the review is open. Owns **every** key (routed near the
    /// top of [`on_key`]): ↑/↓ move over the items and the two action rows,
    /// wrapping at the ends, Home/End jump, Space toggles the highlighted
    /// item, Enter confirms the selection from an item or the first action
    /// row and declines from the second, Esc declines. Ctrl+C on the startup
    /// review quits without answering anything — the way out of a review the
    /// user did not mean to face, where a decline would record every server
    /// disabled: nothing it asked about runs, and the next launch asks again.
    /// On a review the user asked for it closes the review unanswered, the
    /// pickers' Ctrl+C.
    ///
    /// [`on_key`]: App::on_key
    pub(super) fn on_key_config_review(&mut self, key: KeyEvent) -> Action {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            if self.config_review.as_ref().is_some_and(|r| r.requested) {
                self.config_review = None;
                return Action::None;
            }
            return Action::Quit;
        }
        let Some(page) = self
            .config_review
            .as_mut()
            .and_then(|review| review.pages.first_mut())
        else {
            return Action::None;
        };
        let rows = page.row_count();
        match key.code {
            KeyCode::Up => page.selected = wrap_step(page.selected, rows, -1),
            KeyCode::Down => page.selected = wrap_step(page.selected, rows, 1),
            KeyCode::Home => page.selected = 0,
            KeyCode::End => page.selected = rows - 1,
            KeyCode::Char(' ')
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                let selected = page.selected;
                if let Some(item) = page.item_mut(selected) {
                    item.checked = !item.checked;
                }
            }
            KeyCode::Enter => {
                let confirmed = page.action_at(page.selected) != Some(ReviewChoice::Decline);
                return self.answer_review_page(confirmed);
            }
            KeyCode::Esc => return self.answer_review_page(false),
            _ => {}
        }
        Action::None
    }
}
