//! The `/sponsor` page: where to sponsor the developer and how to reach
//! them, and the read-only inline page that shows both. See
//! `docs/sponsor.md`.
//!
//! The `/donate` page's twin ([`super::donate`]): **no text entry** —
//! navigation and copy are the whole grammar — so the hardware cursor hides
//! while its seat tracks the highlighted `❯` row, and every key is owned
//! while the page is open. The catalog is a const for `/donate`'s reason: a
//! page that read a sponsor link from disk would be a page anyone with write
//! access to the config home could point somewhere else.

use super::*;

/// One way to sponsor or reach the developer: the row's label (`GitHub
/// Sponsors`), the dim note beside it, the value in the box — what Enter
/// copies, verbatim — the target a click on it opens, and the noun the copy
/// toast names it by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SponsorLink {
    /// The bold label the row leads with.
    pub label: &'static str,
    /// The dim note after the label: what this row is for, in a few words.
    pub note: &'static str,
    /// What the box shows and Enter copies — a bare URL or address, ASCII
    /// with no whitespace (the catalog tests pin that).
    pub value: &'static str,
    /// The link the boxed value opens: the URL itself, or `mailto:` in
    /// front of an address.
    pub target: &'static str,
    /// What the copy toast calls the value: `Copied the {noun} to
    /// clipboard`.
    pub noun: &'static str,
}

/// The sponsor page and the contact address, in the order the page lists
/// them.
pub const SPONSOR_LINKS: &[SponsorLink] = &[
    SponsorLink {
        label: "GitHub Sponsors",
        note: "monthly or one-time",
        value: "https://github.com/sponsors/linuztx",
        target: "https://github.com/sponsors/linuztx",
        noun: "sponsor link",
    },
    SponsorLink {
        label: "Email",
        note: "company sponsorships and anything else",
        value: "linuztx@gmail.com",
        target: "mailto:linuztx@gmail.com",
        noun: "email address",
    },
];

/// The open `/sponsor` page's state (`None` on [`App`] when closed): which
/// row the `❯` sits on. Nothing else — the catalog is a const, and the page
/// takes no text.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SponsorPage {
    /// Index of the highlighted row within [`SPONSOR_LINKS`].
    pub selected: usize,
}

impl App {
    /// Open the `/sponsor` page, the highlight on the first row. Abandons
    /// any `?` band / palette / file picker (they share the composer the
    /// page takes over) — every picker's open rule.
    pub fn open_sponsor_page(&mut self) {
        self.shortcuts_open = false;
        self.command_menu = None;
        self.file_search = None;
        self.skill_picker = None;
        self.backtrack = Backtrack::default();
        self.sponsor_page = Some(SponsorPage::default());
    }

    /// Dismiss the page (Esc, or Ctrl+C): the composer returns. No view
    /// change — it was never an overlay.
    pub fn close_sponsor_page(&mut self) {
        self.sponsor_page = None;
    }

    /// The highlighted row, if the page is open.
    #[must_use]
    pub fn highlighted_sponsor_link(&self) -> Option<SponsorLink> {
        let page = self.sponsor_page.as_ref()?;
        SPONSOR_LINKS.get(page.selected).copied()
    }

    /// Keys while the `/sponsor` page is open — the `/donate` grammar. Owns
    /// **every** key (routed at the top of [`on_key`]): ↑/↓ move wrapping at
    /// the ends, Home/End jump, digits jump to their row **and copy it**,
    /// Enter and `c` copy the highlighted value, Esc and Ctrl+C close.
    /// Copying keeps the page open, so the toast lands with the value still
    /// in view and the other row is one key away.
    ///
    /// [`on_key`]: App::on_key
    pub(super) fn on_key_sponsor_page(&mut self, key: KeyEvent) -> Action {
        // Ctrl+C closes the page (never quits — the picker family's rule).
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.close_sponsor_page();
            return Action::CloseSponsorPage;
        }
        let len = SPONSOR_LINKS.len();
        let Some(page) = self.sponsor_page.as_mut() else {
            return Action::None;
        };
        match key.code {
            KeyCode::Up => page.selected = wrap_step(page.selected, len, -1),
            KeyCode::Down => page.selected = wrap_step(page.selected, len, 1),
            KeyCode::Home => page.selected = 0,
            KeyCode::End => page.selected = len.saturating_sub(1),
            KeyCode::Enter | KeyCode::Char('c' | 'C') => {
                if let Some(link) = SPONSOR_LINKS.get(page.selected) {
                    return Action::CopySponsorLink(*link);
                }
            }
            // Digits jump to their absolute row and copy it; one past the
            // rows names nothing and is ignored.
            KeyCode::Char(c @ '1'..='9')
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                let index = (c as usize) - ('1' as usize);
                if let Some(link) = SPONSOR_LINKS.get(index) {
                    page.selected = index;
                    return Action::CopySponsorLink(*link);
                }
            }
            KeyCode::Esc => {
                self.close_sponsor_page();
                return Action::CloseSponsorPage;
            }
            _ => {}
        }
        Action::None
    }
}
