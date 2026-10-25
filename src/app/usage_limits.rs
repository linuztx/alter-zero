//! The `/usage` page: every signed-in subscription's usage limits, and the
//! read-only inline page that shows them. See `docs/usage-limits.md`.
//!
//! The `/context` page's sibling — a composer-replacing page with **no text
//! entry**, the hardware cursor hidden while its seat tracks the highlighted
//! `❯`, every key owned while open — over a store the session keeps: what
//! each subscription's provider last said about its limits
//! ([`SubscriptionLimits`]). The boundary owns everything this module
//! cannot know — which subscriptions are signed in, the network, the clock
//! — and hands each in: the list when the page opens
//! ([`App::open_usage_limits`]), each answer as it lands
//! ([`App::set_plan_limits`]), the wall clock per draw
//! ([`App::set_usage_limits_now`]) and the local time zone once
//! ([`App::set_local_offset`]).

use super::*;
use crate::llm::usage_limits::PlanLimits;

/// The toast `/usage` raises with no subscription signed in — nothing to
/// show, so no page (the `/export` and `/compact` empty rule).
pub const USAGE_LIMITS_EMPTY_NOTICE: &str =
    "No subscription signed in — use /login → Use a subscription";

/// How long the page waits after a refresh began before it refreshes itself
/// again while open — Codex's own cadence for a signed-in account.
pub const USAGE_LIMITS_REFRESH_SECS: i64 = 60;

/// One signed-in subscription, as the boundary describes it when the page
/// opens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubscriptionEntry {
    /// The provider id (`chatgpt_codex`) — what an answer is matched by.
    pub id: String,
    /// The human-readable label (`ChatGPT Codex`).
    pub name: String,
    /// Is the session running on this subscription right now?
    pub active: bool,
    /// The provider's own usage page — what the page links and `c` copies.
    pub dashboard: Option<String>,
}

/// What the session knows of one subscription's limits.
#[derive(Debug, Clone, PartialEq)]
pub struct SubscriptionLimits {
    /// Which subscription.
    pub entry: SubscriptionEntry,
    /// The provider's last answer — kept through a refresh, and through a
    /// failure after it, so the page never goes blank on a bad network.
    pub limits: Option<PlanLimits>,
    /// Why the newest fetch failed, worded for the page; cleared by a
    /// success.
    pub error: Option<String>,
    /// Is a fetch out for it?
    pub refreshing: bool,
    /// When [`limits`](Self::limits) arrived, as Unix seconds.
    pub fetched_at: Option<i64>,
}

impl SubscriptionLimits {
    fn new(entry: SubscriptionEntry) -> Self {
        Self {
            entry,
            limits: None,
            error: None,
            refreshing: false,
            fetched_at: None,
        }
    }
}

/// The open `/usage` page's state (`None` on [`App`] when closed): which
/// section the `❯` sits on. What it shows is the session's store
/// ([`App::subscription_limits`]), which outlives it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UsageLimitsPage {
    /// Index of the highlighted section within the store.
    pub selected: usize,
}

impl App {
    /// Open the `/usage` page over `entries` — the signed-in subscriptions,
    /// in the order the page lists them — highlighting the one the session
    /// runs on. The store follows the sign-ins: what was fetched for a
    /// subscription still listed is kept (so the page opens on the last
    /// answer while the next is fetched), one signed out meanwhile is
    /// dropped, one signed in meanwhile starts empty. Abandons any `?` band
    /// / palette / picker band — every picker's open rule.
    pub fn open_usage_limits(&mut self, entries: Vec<SubscriptionEntry>) {
        self.shortcuts_open = false;
        self.command_menu = None;
        self.file_search = None;
        self.skill_picker = None;
        self.backtrack = Backtrack::default();
        let mut previous = std::mem::take(&mut self.subscription_limits);
        self.subscription_limits = entries
            .into_iter()
            .map(
                |entry| match previous.iter().position(|kept| kept.entry.id == entry.id) {
                    Some(at) => {
                        let mut kept = previous.swap_remove(at);
                        kept.entry = entry;
                        kept
                    }
                    None => SubscriptionLimits::new(entry),
                },
            )
            .collect();
        let selected = self
            .subscription_limits
            .iter()
            .position(|s| s.entry.active)
            .unwrap_or(0);
        self.usage_limits = Some(UsageLimitsPage { selected });
    }

    /// Dismiss the page (Esc, or Ctrl+C): the composer returns. The refresh
    /// it was waiting on is dropped — the loop cancels the fetches still out
    /// and a late answer to it is ignored — so the store never reads
    /// `refreshing` with nothing out. No view change: it was never an
    /// overlay.
    pub fn close_usage_limits(&mut self) {
        self.usage_limits = None;
        self.usage_limits_generation += 1;
        for subscription in &mut self.subscription_limits {
            subscription.refreshing = false;
        }
    }

    /// The session's store of each listed subscription's limits, in page
    /// order.
    #[must_use]
    pub fn subscription_limits(&self) -> &[SubscriptionLimits] {
        &self.subscription_limits
    }

    /// Begin a refresh at `now` (Unix seconds): every listed subscription is
    /// marked as fetching, and the refresh's **generation** — what the loop
    /// hands each fetch, and each answer must echo — is returned. An answer
    /// to any earlier refresh is dropped from then on.
    pub fn begin_usage_limits_refresh(&mut self, now: i64) -> u64 {
        self.usage_limits_generation += 1;
        self.usage_limits_refreshed_at = Some(now);
        for subscription in &mut self.subscription_limits {
            subscription.refreshing = true;
        }
        self.usage_limits_generation
    }

    /// One subscription's answer to refresh `generation`, landing at `now`:
    /// its limits — replacing the last and clearing a failure — or why
    /// there are none, which keeps the last answer beside it. Dropped when
    /// it answers an older refresh, or names a subscription no longer listed.
    pub fn set_plan_limits(
        &mut self,
        id: &str,
        generation: u64,
        result: std::result::Result<PlanLimits, String>,
        now: i64,
    ) {
        if generation != self.usage_limits_generation {
            return;
        }
        let Some(subscription) = self
            .subscription_limits
            .iter_mut()
            .find(|s| s.entry.id == id)
        else {
            return;
        };
        subscription.refreshing = false;
        match result {
            Ok(limits) => {
                subscription.limits = Some(limits);
                subscription.error = None;
                subscription.fetched_at = Some(now);
            }
            Err(reason) => subscription.error = Some(reason),
        }
    }

    /// Is any listed subscription's fetch still out?
    #[must_use]
    pub fn usage_limits_refreshing(&self) -> bool {
        self.subscription_limits.iter().any(|s| s.refreshing)
    }

    /// Should the open page refresh itself at `now`? Once a
    /// [`USAGE_LIMITS_REFRESH_SECS`] after the last refresh began, while the
    /// page is open and no fetch is out — never doubled, never closed.
    #[must_use]
    pub fn usage_limits_refresh_due(&self, now: i64) -> bool {
        self.usage_limits.is_some()
            && !self.usage_limits_refreshing()
            && self
                .usage_limits_refreshed_at
                .is_some_and(|began| now - began >= USAGE_LIMITS_REFRESH_SECS)
    }

    /// Inject the wall clock, as Unix seconds — the boundary does it before
    /// every draw while the page is open, so its countdowns and its
    /// freshness come from no clock of the page's own.
    pub fn set_usage_limits_now(&mut self, now: i64) {
        self.usage_limits_now = Some(now);
    }

    /// The wall clock the page was last handed.
    #[must_use]
    pub fn usage_limits_now(&self) -> Option<i64> {
        self.usage_limits_now
    }

    /// Inject the local time zone, as the UTC offset (in seconds east) in
    /// force at a given Unix time — the boundary's `chrono::Local`, set once
    /// (the [`App::set_clock`] pattern). An offset per instant rather than
    /// one number, so a reset across a DST change lands on its own hour.
    pub fn set_local_offset(&mut self, offset: fn(i64) -> i32) {
        self.local_offset = Some(offset);
    }

    /// The local UTC offset at `at` — `0` (UTC) until the boundary injects
    /// one.
    #[must_use]
    pub fn local_offset(&self, at: i64) -> i32 {
        self.local_offset.map_or(0, |offset| offset(at))
    }

    /// The highlighted section's subscription, if the page is open.
    #[must_use]
    pub fn highlighted_subscription_limits(&self) -> Option<&SubscriptionLimits> {
        let page = self.usage_limits.as_ref()?;
        self.subscription_limits.get(page.selected)
    }

    /// Keys while the `/usage` page is open — the `/context` grammar plus a
    /// refresh and a copy. Owns **every** key (routed at the top of
    /// [`on_key`]): ↑/↓ move between sections wrapping at the ends, Home/End
    /// jump, `r` refreshes when no fetch is out, `c` copies the highlighted
    /// subscription's usage link, Esc and Ctrl+C close. Ctrl+D and Ctrl+O
    /// pass through to the two read-only overlays — the page waits behind
    /// them.
    ///
    /// [`on_key`]: App::on_key
    pub(super) fn on_key_usage_limits(&mut self, key: KeyEvent) -> Action {
        // Ctrl+C closes the page (never quits — the picker family's rule).
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.close_usage_limits();
            return Action::CloseUsageLimits;
        }
        if let Some(action) = self.on_key_overlay_toggle(key) {
            return action;
        }
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            return Action::None;
        }
        let len = self.subscription_limits.len();
        let refreshing = self.usage_limits_refreshing();
        let link = self
            .highlighted_subscription_limits()
            .and_then(|s| s.entry.dashboard.clone());
        let Some(page) = self.usage_limits.as_mut() else {
            return Action::None;
        };
        match key.code {
            KeyCode::Up => page.selected = wrap_step(page.selected, len, -1),
            KeyCode::Down => page.selected = wrap_step(page.selected, len, 1),
            KeyCode::Home => page.selected = 0,
            KeyCode::End => page.selected = len.saturating_sub(1),
            KeyCode::Char('r' | 'R') if !refreshing => return Action::RefreshUsageLimits,
            KeyCode::Char('c' | 'C') => {
                if let Some(link) = link {
                    return Action::CopyUsageLink(link);
                }
            }
            KeyCode::Esc => {
                self.close_usage_limits();
                return Action::CloseUsageLimits;
            }
            _ => {}
        }
        Action::None
    }
}
