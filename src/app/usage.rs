//! The `/usage` page: what is left of each subscription this machine is
//! signed in to. See `docs/usage.md`.
//!
//! The `/context` page's sibling — a read-only composer-replacing page with
//! **no text entry**, the hardware cursor hidden while its seat tracks the
//! highlighted `❯`, every key owned while open — over the answers each
//! subscription's own server gave (`llm::plan_usage`). Which subscriptions
//! are signed in is the boundary's to know (the providers table and the key
//! store), so `/usage` only asks ([`Action::OpenUsagePage`]) and the
//! boundary opens the page with its rows ([`App::open_usage_page`]), fetches
//! each on a worker, and hands the answers in as they land
//! ([`App::set_usage_result`]).

use super::*;
use crate::llm::plan_usage::PlanUsage;

/// What one subscription's row holds.
#[derive(Debug, Clone, PartialEq)]
pub enum UsageState {
    /// Its first answer is still out.
    Fetching,
    /// What the provider answered, and when (Unix seconds) — the instant
    /// its reset times are worded against.
    Ready {
        /// The answer.
        usage: PlanUsage,
        /// When it was fetched.
        fetched_at: i64,
    },
    /// Why it could not be read, worded for the row.
    Failed(String),
}

/// One subscription on the page.
#[derive(Debug, Clone, PartialEq)]
pub struct UsageEntry {
    /// The provider's id in the providers table — what an answer names.
    pub provider_id: String,
    /// Its name, as `/login` lists it.
    pub name: String,
    /// Whether the session's model runs on it — the row the page opens on.
    pub in_use: bool,
    /// Where the provider shows the same numbers, with history.
    pub dashboard: Option<String>,
    /// What the row holds.
    pub state: UsageState,
    /// Whether a refresh is out for it — its previous answer stays on
    /// screen until the new one lands.
    pub refreshing: bool,
}

impl UsageEntry {
    /// A subscription whose answer is not in yet.
    #[must_use]
    pub fn new(
        provider_id: impl Into<String>,
        name: impl Into<String>,
        in_use: bool,
        dashboard: Option<&str>,
    ) -> Self {
        Self {
            provider_id: provider_id.into(),
            name: name.into(),
            in_use,
            dashboard: dashboard.map(str::to_string),
            state: UsageState::Fetching,
            refreshing: false,
        }
    }

    /// Is an answer for it still out — its first, or a refresh's?
    #[must_use]
    pub fn is_fetching(&self) -> bool {
        self.refreshing || self.state == UsageState::Fetching
    }
}

/// The open `/usage` page's state (`None` on [`App`] when closed).
#[derive(Debug, Clone, PartialEq)]
pub struct UsagePage {
    /// Every subscription signed in to, the one in use first.
    pub entries: Vec<UsageEntry>,
    /// The highlighted subscription.
    pub selected: usize,
    /// When the page last asked (Unix seconds) — the subtitle's `as of`.
    pub asked_at: i64,
    /// The local clock's offset from UTC, in seconds east — what reset
    /// times are shown in. Injected at the boundary, the clock pattern.
    pub utc_offset: i32,
    /// Which fetch the rows belong to: an answer tagged with another is a
    /// superseded fetch's, and is dropped.
    generation: u64,
}

impl UsagePage {
    /// The fetch generation the page's answers must carry.
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// Is any answer still out?
    #[must_use]
    pub fn is_fetching(&self) -> bool {
        self.entries.iter().any(UsageEntry::is_fetching)
    }
}

impl App {
    /// Open the `/usage` page over `entries` — every subscription signed in
    /// to, the one in use first — all fetching, asked at `asked_at` (Unix
    /// seconds) in a clock `utc_offset` seconds east of UTC. Returns the
    /// generation the boundary tags its fetches with. Abandons any `?`
    /// band / palette / picker band (they share the composer the page takes
    /// over) — every picker's open rule.
    pub fn open_usage_page(
        &mut self,
        entries: Vec<UsageEntry>,
        asked_at: i64,
        utc_offset: i32,
    ) -> u64 {
        self.shortcuts_open = false;
        self.command_menu = None;
        self.file_search = None;
        self.skill_picker = None;
        self.backtrack = Backtrack::default();
        self.usage_generation += 1;
        self.usage_page = Some(UsagePage {
            entries,
            selected: 0,
            asked_at,
            utc_offset,
            generation: self.usage_generation,
        });
        self.usage_generation
    }

    /// Dismiss the page (Esc, Ctrl+C): the composer returns. No view change
    /// — it was never an overlay.
    pub fn close_usage_page(&mut self) {
        self.usage_page = None;
    }

    /// Start reading every subscription again (`r`), asked at `asked_at`:
    /// each row keeps what it shows until its new answer lands. Returns the
    /// new generation, or `None` with no page open.
    pub fn begin_usage_refresh(&mut self, asked_at: i64) -> Option<u64> {
        let page = self.usage_page.as_mut()?;
        self.usage_generation += 1;
        page.generation = self.usage_generation;
        page.asked_at = asked_at;
        for entry in &mut page.entries {
            entry.refreshing = true;
        }
        Some(self.usage_generation)
    }

    /// One subscription's answer, fetched at `fetched_at`: lands on its row
    /// when it belongs to the page's current fetch, and is dropped
    /// otherwise — a superseded fetch's, a provider the page doesn't list,
    /// or one arriving after the page closed. Returns whether it landed.
    pub fn set_usage_result(
        &mut self,
        generation: u64,
        provider_id: &str,
        result: Result<PlanUsage, String>,
        fetched_at: i64,
    ) -> bool {
        let Some(page) = self.usage_page.as_mut() else {
            return false;
        };
        if page.generation != generation {
            return false;
        }
        let Some(entry) = page
            .entries
            .iter_mut()
            .find(|entry| entry.provider_id == provider_id)
        else {
            return false;
        };
        entry.state = match result {
            Ok(usage) => UsageState::Ready { usage, fetched_at },
            Err(reason) => UsageState::Failed(reason),
        };
        entry.refreshing = false;
        true
    }

    /// Keys while the `/usage` page is open — the `/context` grammar plus
    /// `r`. Owns **every** key (routed at the top of [`on_key`]): ↑/↓ move
    /// the highlight wrapping at the ends, Home/End jump, `r` asks the loop
    /// to read everything again, Esc and Ctrl+C close. Ctrl+D and Ctrl+O
    /// pass through to the two read-only overlays, and the page waits behind
    /// them.
    ///
    /// [`on_key`]: App::on_key
    pub(super) fn on_key_usage_page(&mut self, key: KeyEvent) -> Action {
        // Ctrl+C closes the page (never quits — the picker family's rule).
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.close_usage_page();
            return Action::CloseUsagePage;
        }
        if let Some(action) = self.on_key_overlay_toggle(key) {
            return action;
        }
        let Some(page) = self.usage_page.as_mut() else {
            return Action::None;
        };
        let count = page.entries.len();
        match key.code {
            KeyCode::Up => page.selected = wrap_step(page.selected, count, -1),
            KeyCode::Down => page.selected = wrap_step(page.selected, count, 1),
            KeyCode::Home => page.selected = 0,
            KeyCode::End => page.selected = count.saturating_sub(1),
            KeyCode::Char('r' | 'R') => return Action::RefreshUsage,
            KeyCode::Esc => {
                self.close_usage_page();
                return Action::CloseUsagePage;
            }
            _ => {}
        }
        Action::None
    }
}
