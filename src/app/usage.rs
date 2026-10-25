//! The `/usage` page: every signed-in subscription's usage against its plan,
//! read off the providers themselves. See `docs/usage.md`.
//!
//! The `/context` page's sibling — a read-only composer-replacing page with
//! no text entry, the hardware cursor hidden and seated on the `❯` — whose
//! content arrives over the network rather than from App. So the page is a
//! set of **rounds**: the boundary says which subscriptions there are and
//! starts one read per subscription ([`App::begin_usage_round`]), each result
//! lands against the round it was asked for ([`App::finish_usage_fetch`]),
//! and anything naming another round — a page closed and reopened since, a
//! refresh that replaced it — is dropped here, where it can be tested,
//! rather than at the boundary.

use super::*;

use crate::llm::usage::UsageReport;

/// How long an open page waits after its last read before reading every
/// subscription again — the cadence codex polls the same ChatGPT endpoint at.
pub const USAGE_REFRESH_SECS: i64 = 60;

/// A subscription the page reads, as the boundary found it: the provider's
/// id and name, and whether it is the one the session is running on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageTarget {
    pub provider: String,
    pub name: String,
    pub in_use: bool,
}

/// One subscription's section of the page.
#[derive(Debug, Clone, PartialEq)]
pub struct UsageSection {
    /// The provider's id — what a result is matched to.
    pub provider: String,
    /// The provider's name, the section's head (`ChatGPT Codex`).
    pub name: String,
    /// Whether the session is running on this subscription.
    pub in_use: bool,
    /// The last report read — kept through a refresh, so the section never
    /// blanks while its replacement is on the way.
    pub report: Option<UsageReport>,
    /// Whether a read for this section is in flight.
    pub loading: bool,
    /// Why the last read failed, when it did.
    pub error: Option<String>,
}

/// The open `/usage` page's state (`None` on [`App`] when closed).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct UsagePage {
    /// One per signed-in subscription, the one in use first.
    pub sections: Vec<UsageSection>,
    /// Index of the highlighted section.
    pub selected: usize,
    /// Whether the boundary has said which subscriptions there are — before
    /// it has, the page is checking.
    pub ready: bool,
    /// The round of reads in flight, or the last one.
    pub round: u64,
    /// When the last round finished, in Unix seconds.
    pub updated_at: Option<i64>,
    /// The wall clock, in Unix seconds — injected by the boundary every draw
    /// ([`App::set_usage_now`]), so a countdown moves and the page reads no
    /// clock of its own.
    pub now: i64,
}

impl UsagePage {
    /// Whether any section's read is in flight.
    #[must_use]
    pub fn loading(&self) -> bool {
        self.sections.iter().any(|section| section.loading)
    }
}

impl App {
    /// Open the `/usage` page, checking — the boundary then says which
    /// subscriptions there are and starts reading them. Abandons any `?`
    /// band / palette / picker band (they share the composer the page takes
    /// over) — every picker's open rule.
    pub fn open_usage_page(&mut self) {
        self.shortcuts_open = false;
        self.command_menu = None;
        self.file_search = None;
        self.skill_picker = None;
        self.backtrack = Backtrack::default();
        self.usage_page = Some(UsagePage::default());
    }

    /// Dismiss the page (Esc, Ctrl+C): the composer returns. No view change
    /// — it was never an overlay.
    pub fn close_usage_page(&mut self) {
        self.usage_page = None;
    }

    /// Start a round of reads over `targets` — the open page's subscriptions,
    /// the one in use first — returning the round every result must name, or
    /// `None` when the page is closed. A subscription already on the page
    /// keeps its last report through the round; the highlight stays on the
    /// subscription it was on.
    pub fn begin_usage_round(&mut self, targets: Vec<UsageTarget>) -> Option<u64> {
        let page = self.usage_page.as_mut()?;
        self.usage_rounds += 1;
        let round = self.usage_rounds;
        let highlighted = page
            .sections
            .get(page.selected)
            .map(|section| section.provider.clone());
        let mut previous = std::mem::take(&mut page.sections);
        // The subscription in use first — the one whose limits stop this
        // session — then the rest in the order the boundary found them.
        let (in_use, others): (Vec<_>, Vec<_>) =
            targets.into_iter().partition(|target| target.in_use);
        page.sections = in_use
            .into_iter()
            .chain(others)
            .map(|target| {
                let report = previous
                    .iter()
                    .position(|section| section.provider == target.provider)
                    .and_then(|at| previous.swap_remove(at).report);
                UsageSection {
                    provider: target.provider,
                    name: target.name,
                    in_use: target.in_use,
                    report,
                    loading: true,
                    error: None,
                }
            })
            .collect();
        page.selected = highlighted
            .and_then(|provider| {
                page.sections
                    .iter()
                    .position(|section| section.provider == provider)
            })
            .unwrap_or(0)
            .min(page.sections.len().saturating_sub(1));
        page.ready = true;
        page.round = round;
        Some(round)
    }

    /// One subscription's read finished. Dropped when the page has closed or
    /// `round` is not the page's own. A failure keeps the last report beside
    /// the reason. `now` (Unix seconds) stamps the round once its last read
    /// lands — what the subtitle's `updated` and the next refresh count from.
    pub fn finish_usage_fetch(
        &mut self,
        round: u64,
        provider: &str,
        result: Result<UsageReport, String>,
        now: i64,
    ) {
        let Some(page) = self.usage_page.as_mut().filter(|page| page.round == round) else {
            return;
        };
        let Some(section) = page
            .sections
            .iter_mut()
            .find(|section| section.provider == provider)
        else {
            return;
        };
        section.loading = false;
        match result {
            Ok(report) => {
                section.report = Some(report);
                section.error = None;
            }
            Err(error) => section.error = Some(error),
        }
        if !page.loading() {
            page.updated_at = Some(now);
        }
    }

    /// Whether the open page is due a re-read: every read of the last round
    /// landed at least [`USAGE_REFRESH_SECS`] ago, by the injected clock.
    #[must_use]
    pub fn usage_refresh_due(&self) -> bool {
        self.usage_page.as_ref().is_some_and(|page| {
            page.ready
                && !page.sections.is_empty()
                && !page.loading()
                && page
                    .updated_at
                    .is_some_and(|at| page.now - at >= USAGE_REFRESH_SECS)
        })
    }

    /// Inject the wall clock (Unix seconds) into the open page.
    pub fn set_usage_now(&mut self, now: i64) {
        if let Some(page) = self.usage_page.as_mut() {
            page.now = now;
        }
    }

    /// Inject, once, how the local clock reads an instant: its offset from
    /// UTC, in seconds, at that instant (Unix seconds) — so a reset across a
    /// daylight-saving change still reads the right hour (`docs/usage.md`).
    pub fn set_local_offset(&mut self, offset: fn(i64) -> i32) {
        self.local_offset = Some(offset);
    }

    /// The local clock's offset from UTC at `at`, in seconds — UTC itself
    /// until the boundary injects the local one.
    #[must_use]
    pub fn local_offset(&self, at: i64) -> i32 {
        self.local_offset.map_or(0, |offset| offset(at))
    }

    /// Keys while the `/usage` page is open. Owns **every** key (routed at
    /// the top of [`on_key`]): ↑/↓ move between subscriptions wrapping at the
    /// ends, Home/End jump, `r` reads every subscription again (not while a
    /// read is in flight), Esc and Ctrl+C close. Everything else is
    /// swallowed — the `/donate` rule.
    ///
    /// [`on_key`]: App::on_key
    pub(super) fn on_key_usage_page(&mut self, key: KeyEvent) -> Action {
        // Ctrl+C closes the page (never quits — the picker family's rule).
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.close_usage_page();
            return Action::CloseUsagePage;
        }
        let Some(page) = self.usage_page.as_mut() else {
            return Action::None;
        };
        let count = page.sections.len();
        match key.code {
            KeyCode::Esc => {
                self.close_usage_page();
                return Action::CloseUsagePage;
            }
            KeyCode::Char('r' | 'R')
                if key.modifiers.difference(KeyModifiers::SHIFT).is_empty() =>
            {
                return if page.ready && !page.loading() {
                    Action::RefreshUsagePage
                } else {
                    Action::None
                };
            }
            KeyCode::Up => page.selected = wrap_step(page.selected, count, -1),
            KeyCode::Down => page.selected = wrap_step(page.selected, count, 1),
            KeyCode::Home => page.selected = 0,
            KeyCode::End => page.selected = count.saturating_sub(1),
            _ => {}
        }
        Action::None
    }
}
