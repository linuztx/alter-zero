//! The inline `/model` picker: the per-provider model load — kept lists shown
//! at once, fetches merged as they land (`docs/model-cache.md`) — its search
//! query, and the selection. See `docs/llm.md`.

use std::collections::BTreeMap;

use super::views::TOOL_VIEW_PAGE;
use super::*;
use crate::llm::models_cache::CachedListing;

/// How many rows PageUp/PageDown move the inline `/model` picker.
const MODEL_PAGE: usize = TOOL_VIEW_PAGE;

/// The load state of the inline `/model` picker's list: the boundary spawns a
/// worker that fetches the provider's models, so the picker shows a placeholder
/// until the result lands. See `docs/llm.md`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ModelLoad {
    /// The fetch is in flight — the picker shows `Loading models…`.
    #[default]
    Loading,
    /// The models arrived (possibly an empty list).
    Ready,
    /// The fetch failed — the picker shows the message in red.
    Error(String),
    /// No provider has a resolvable API key yet, so no list is fetched — the
    /// picker points the user at `/login` instead of showing models they can't
    /// use. See `docs/llm.md`.
    NeedsLogin,
}

/// The inline `/model` picker's state (`None` on [`App`] when closed). Unlike
/// the alternate-screen `/resume` picker, this one **replaces the composer**
/// in the bottom live region with its own `>` search prompt and a scrolling
/// model list. Its rows are the kept lists, shown at once, and each provider's
/// `/v1/models` fetch as it lands ([`App::begin_model_load`],
/// [`App::finish_model_fetch`] — `docs/model-cache.md`); the filtered view
/// derives on demand ([`matches`]).
///
/// [`matches`]: ModelPicker::matches
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ModelPicker {
    /// Every listed model, provider-tagged and sorted: the kept lists at once
    /// ([`App::begin_model_load`]), each provider's rows replaced as its
    /// fetch lands ([`App::finish_model_fetch`]). Empty while nothing has.
    pub models: Vec<ModelEntry>,
    /// Whether the list is still loading, ready, or errored.
    pub status: ModelLoad,
    /// Index of the highlighted row within the current filtered matches.
    pub selected: usize,
    /// The type-to-search query — any plain printable key appends, Backspace
    /// pops, Esc clears (the `/resume` picker's search grammar).
    pub query: String,
    /// The currently active model id, marked with a ✓ in the list.
    pub active_id: String,
    /// The provider the active model belongs to, so the ✓ marks the exact active
    /// row (a merged multi-provider list can carry the same id twice). Empty when
    /// unknown — the ✓ then falls back to matching the id alone.
    pub active_provider: String,
    /// The providers whose list is being fetched — every configured provider
    /// at once, merged as each lands (docs/llm.md), and since the lists are
    /// kept, as often a refresh behind rows already on screen as a first load
    /// (`docs/model-cache.md`). Its length ([`pending`]) drives the counter's
    /// `loading more…` / `refreshing…` note, and at zero with no models it
    /// settles the all-failed error.
    ///
    /// [`pending`]: ModelPicker::pending
    pub fetching: Vec<ModelFetchTarget>,
    /// Providers whose model fetch failed with nothing of theirs on screen,
    /// kept so the picker can show which lists are missing (a red `…
    /// unavailable` note beside a partial list, or one red row per provider
    /// when they all fail).
    pub errors: Vec<ModelFetchError>,
    /// Providers whose **refresh** failed while their kept list stayed on
    /// screen — an amber `couldn't refresh …` note, not a red one: the rows
    /// are there, only older than they might be (`docs/model-cache.md`).
    pub stale: Vec<ModelFetchError>,
    /// When each provider's rows on screen were fetched, in Unix seconds, by
    /// provider id — a kept list's stamp, or the moment a fetch landed. What
    /// the counter's `updated 2h ago` counts from.
    pub fetched_at: BTreeMap<String, i64>,
    /// The wall clock in Unix seconds, injected by the boundary on every draw
    /// ([`App::set_model_picker_now`]).
    pub now: i64,
}

/// A provider the picker is waiting on: its id, which its rows and its
/// fetch's answer are keyed by, and its name, which a failure is reported
/// under. See `docs/model-cache.md`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelFetchTarget {
    /// The provider id (e.g. `openrouter`).
    pub provider: String,
    /// Its human label (its `name`, e.g. `OpenRouter`).
    pub name: String,
}

/// A provider whose `/model` fetch failed, with the concise reason to surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelFetchError {
    /// The provider's human label (its `name`, e.g. `OpenRouter`).
    pub provider: String,
    /// The one-line failure reason (an [`crate::llm::LlmError`] rendering).
    pub message: String,
}

impl ModelPicker {
    /// The rows the picker shows: models whose id, provider, or friendly name
    /// contains the query (case-insensitive substring; every model for an empty
    /// query), keeping the fetched (alphabetical) order.
    #[must_use]
    pub fn matches(&self) -> Vec<&ModelEntry> {
        let query = self.query.to_lowercase();
        self.models
            .iter()
            .filter(|m| {
                query.is_empty()
                    || m.id.to_lowercase().contains(&query)
                    || m.provider.to_lowercase().contains(&query)
                    || m.display_name.to_lowercase().contains(&query)
            })
            .collect()
    }

    /// The highlighted model in the current filtered view, if any (an empty
    /// list — loading, error, or no match — has none).
    #[must_use]
    pub fn highlighted(&self) -> Option<&ModelEntry> {
        let matches = self.matches();
        matches.get(self.selected).copied()
    }

    /// Whether `m` is the active model (the ✓ row): the id matches and — when the
    /// active provider is known — so does the provider, so a shared id in the
    /// merged multi-provider list marks only the row actually in use.
    #[must_use]
    pub fn is_active(&self, m: &ModelEntry) -> bool {
        m.id == self.active_id
            && (self.active_provider.is_empty() || m.provider == self.active_provider)
    }

    /// How many provider fetches are still out.
    #[must_use]
    pub fn pending(&self) -> usize {
        self.fetching.len()
    }

    /// Whether a fetch is out for a provider with no list on screen yet —
    /// the counter's `loading more…` rather than `refreshing…`.
    #[must_use]
    pub fn loading_new(&self) -> bool {
        self.fetching
            .iter()
            .any(|target| !self.fetched_at.contains_key(&target.provider))
    }

    /// When the oldest list on screen was fetched (Unix seconds) — what the
    /// counter's `updated 2h ago` is said of, since it has to hold for every
    /// row. `None` before any list carries a stamp.
    #[must_use]
    pub fn oldest_fetch(&self) -> Option<i64> {
        self.fetched_at.values().min().copied()
    }

    /// The highlighted row's `(provider, id)`, which a merge keeps the
    /// highlight on.
    fn highlighted_key(&self) -> Option<(String, String)> {
        self.highlighted()
            .map(|m| (m.provider.clone(), m.id.clone()))
    }

    /// Sort the rows by id then provider, dropping exact `(provider, id)`
    /// dupes — the merged list's one order, whichever provider landed first.
    fn sort_rows(&mut self) {
        self.models
            .sort_by(|a, b| a.id.cmp(&b.id).then_with(|| a.provider.cmp(&b.provider)));
        self.models
            .dedup_by(|a, b| a.id == b.id && a.provider == b.provider);
    }

    /// Seat the highlight on the active model when it is listed (the ✓ row,
    /// so a shared id under two providers seats on the one in use), else on
    /// the top row.
    fn seat_on_active(&mut self) {
        self.selected = self
            .matches()
            .iter()
            .position(|m| self.is_active(m))
            .unwrap_or(0);
    }

    /// After the rows changed under the highlight: an `untouched` picker (no
    /// search, highlight on the top row) re-seats on the active model, which
    /// may only just have landed; otherwise the highlight stays on `kept`, the
    /// model it was on, or the nearest row still there.
    fn reseat(&mut self, kept: Option<(String, String)>, untouched: bool) {
        if untouched {
            self.seat_on_active();
            return;
        }
        let matches = self.matches();
        let clamped = self.selected.min(matches.len().saturating_sub(1));
        self.selected = kept
            .and_then(|(provider, id)| {
                matches
                    .iter()
                    .position(|m| m.provider == provider && m.id == id)
            })
            .unwrap_or(clamped);
    }

    /// Recompute the display status from the merge so far: `Ready` the moment any
    /// model is present (shown while the rest still load), else `Loading` while
    /// fetches are outstanding, else `Error` when every provider failed with no
    /// models, else `Ready` (all done, genuinely empty). The `Error` string is
    /// left empty — the picker renders the per-provider [`errors`] instead.
    ///
    /// [`errors`]: ModelPicker::errors
    fn recompute_status(&mut self) {
        self.status = if !self.models.is_empty() {
            ModelLoad::Ready
        } else if self.pending() > 0 {
            ModelLoad::Loading
        } else if !self.errors.is_empty() {
            ModelLoad::Error(String::new())
        } else {
            ModelLoad::Ready
        };
    }
}

impl App {
    /// Open the inline `/model` picker, marking `active_id` as the current
    /// model. The list starts empty in the [`ModelLoad::Loading`] state; the
    /// boundary's fetch fills it via [`set_models`]. Abandons any `?` band /
    /// palette / file picker (they share the composer the picker takes over),
    /// but stays in [`View::Conversation`] — the picker is inline, not an
    /// overlay. See `docs/llm.md`.
    ///
    /// [`set_models`]: App::set_models
    pub fn open_model_picker(&mut self, active_id: impl Into<String>) {
        self.shortcuts_open = false;
        self.command_menu = None;
        self.file_search = None;
        self.skill_picker = None;
        self.backtrack = Backtrack::default();
        self.model_picker = Some(ModelPicker {
            active_id: active_id.into(),
            ..ModelPicker::default()
        });
    }

    /// Record which provider the active model belongs to (the boundary knows it,
    /// the pure open call only has the id) so the picker's ✓ marks the exact
    /// active row in the merged multi-provider list. No-op if the picker closed.
    pub fn set_active_provider(&mut self, provider: impl Into<String>) {
        if let Some(picker) = self.model_picker.as_mut() {
            picker.active_provider = provider.into();
        }
    }

    /// Dismiss the inline `/model` picker (Esc/Ctrl+C, or right after a
    /// selection): the composer returns. No view change — it was never an
    /// overlay.
    pub fn close_model_picker(&mut self) {
        self.model_picker = None;
    }

    /// Install the fetched model list into the open picker (the boundary's
    /// worker result), marking it [`ModelLoad::Ready`]. The highlight seats on
    /// the currently-active model when present, else the top — so the picker
    /// opens focused on what's in use. No-op if the picker was closed meanwhile.
    pub fn set_models(&mut self, models: Vec<ModelEntry>) {
        let Some(picker) = self.model_picker.as_mut() else {
            return;
        };
        picker.models = models;
        picker.status = ModelLoad::Ready;
        picker.fetching.clear();
        picker.errors.clear();
        // Seat the highlight on the active model if it's in the (unfiltered)
        // list, so the picker opens focused on the current choice.
        picker.seat_on_active();
    }

    /// Start the open picker's load (`docs/model-cache.md`): show the `cached`
    /// lists at once, seated on the active model, and wait on `fetching` —
    /// behind those rows where a provider has some, else with
    /// `Loading models…` until the first list lands. No-op if the picker
    /// closed meanwhile.
    pub fn begin_model_load(
        &mut self,
        cached: Vec<CachedListing>,
        fetching: Vec<ModelFetchTarget>,
    ) {
        let Some(picker) = self.model_picker.as_mut() else {
            return;
        };
        picker.models.clear();
        picker.fetched_at.clear();
        for listing in cached {
            picker
                .fetched_at
                .insert(listing.provider, listing.fetched_at);
            picker.models.extend(listing.models);
        }
        picker.sort_rows();
        picker.fetching = fetching;
        picker.errors.clear();
        picker.stale.clear();
        picker.seat_on_active();
        picker.recompute_status();
    }

    /// Ctrl+R's refresh: wait on `fetching` — every configured provider —
    /// behind the rows already on screen, which stay until each list lands.
    /// A provider no longer among them leaves the list now, and the last
    /// round's failures are forgotten. No-op if the picker closed.
    pub fn begin_model_refresh(&mut self, fetching: Vec<ModelFetchTarget>) {
        let Some(picker) = self.model_picker.as_mut() else {
            return;
        };
        let kept = picker.highlighted_key();
        let configured = |provider: &str| fetching.iter().any(|t| t.provider == provider);
        picker.models.retain(|m| configured(&m.provider));
        picker.fetched_at.retain(|provider, _| configured(provider));
        picker.fetching = fetching;
        picker.errors.clear();
        picker.stale.clear();
        picker.reseat(kept, false);
        picker.recompute_status();
    }

    /// One provider's fetch landed, at `now` (Unix seconds). A list
    /// **replaces** that provider's rows — models it no longer lists go — and
    /// the highlight rides the same model across the merge, re-seating on the
    /// active model while the user hasn't moved. A failure keeps rows the
    /// provider already had on screen (a refresh that didn't happen, noted in
    /// [`ModelPicker::stale`]); with none it is an [`ModelPicker::errors`]
    /// entry. Ignored unless the picker is waiting on that provider, and a
    /// no-op once it has closed. See `docs/model-cache.md`.
    pub fn finish_model_fetch(
        &mut self,
        provider: &str,
        result: Result<Vec<ModelEntry>, String>,
        now: i64,
    ) {
        let Some(picker) = self.model_picker.as_mut() else {
            return;
        };
        let Some(at) = picker.fetching.iter().position(|t| t.provider == provider) else {
            return;
        };
        let target = picker.fetching.remove(at);
        match result {
            Ok(models) => {
                let untouched = picker.query.is_empty() && picker.selected == 0;
                let kept = picker.highlighted_key();
                picker.models.retain(|m| m.provider != provider);
                picker.models.extend(models);
                picker.sort_rows();
                picker.fetched_at.insert(provider.to_string(), now);
                picker.reseat(kept, untouched);
            }
            Err(message) => {
                let failure = ModelFetchError {
                    provider: target.name,
                    message,
                };
                if picker.models.iter().any(|m| m.provider == provider) {
                    picker.stale.push(failure);
                } else {
                    picker.errors.push(failure);
                }
            }
        }
        picker.recompute_status();
        let last = picker.matches().len().saturating_sub(1);
        picker.selected = picker.selected.min(last);
    }

    /// Inject the wall clock (Unix seconds) into the open picker — what the
    /// counter's `updated 2h ago` counts to.
    pub fn set_model_picker_now(&mut self, now: i64) {
        if let Some(picker) = self.model_picker.as_mut() {
            picker.now = now;
        }
    }

    /// Record that the model fetch failed — the picker shows `message` in red.
    /// No-op if the picker was closed meanwhile.
    pub fn set_models_error(&mut self, message: impl Into<String>) {
        if let Some(picker) = self.model_picker.as_mut() {
            picker.status = ModelLoad::Error(message.into());
            picker.selected = 0;
        }
    }

    /// Record that no provider is configured yet — the picker shows a `/login`
    /// hint in place of a model list (the boundary skips the fetch entirely, so
    /// the user isn't offered models they have no key for). No-op if the picker
    /// was closed meanwhile. See `docs/llm.md`.
    pub fn set_models_needs_login(&mut self) {
        if let Some(picker) = self.model_picker.as_mut() {
            picker.status = ModelLoad::NeedsLogin;
            picker.models.clear();
            picker.errors.clear();
            picker.stale.clear();
            picker.fetching.clear();
            picker.fetched_at.clear();
            picker.selected = 0;
        }
    }

    /// Extend the picker's type-to-search with a bracketed paste — the
    /// `/login` provider step's and the `/resume` picker's handler, with one
    /// divergence: it appends **flush**, no space separator. A model filter
    /// matches one id (`openai/gpt-4o-mini`), so pasting the tail of a
    /// half-typed id has to complete the token rather than start a new word.
    /// Whitespace runs (a copied id's trailing newline, a multi-line
    /// selection) still flatten to single spaces so no control character
    /// reaches the one-line filter, an all-blank paste is a no-op, and the
    /// highlight re-seats at the top of the narrowed list. A model id is copied from a
    /// provider's dashboard far more often than it is typed, and the paste
    /// used to be dropped on the floor to keep it out of the composer draft
    /// underneath — routing it here keeps that draft untouched *and* makes
    /// the key work. No-op if the picker was closed meanwhile. See
    /// `docs/llm.md`.
    pub fn paste_into_model_filter(&mut self, pasted: &str) {
        let flat = pasted.split_whitespace().collect::<Vec<_>>().join(" ");
        if flat.is_empty() {
            return;
        }
        let Some(picker) = self.model_picker.as_mut() else {
            return;
        };
        picker.query.push_str(&flat);
        picker.selected = 0;
    }

    /// Keys while the inline `/model` picker is open. Mirrors the `/resume`
    /// picker's grammar: ↑/↓ move (wrapping at the ends), PageUp/PageDown jump
    /// by [`MODEL_PAGE`] (clamped), Home/End to the ends, Enter selects the
    /// highlighted model, Esc clears a non-empty search before it closes,
    /// Backspace pops, Ctrl+C closes, Ctrl+R fetches every list again while
    /// no fetch is out (`docs/model-cache.md`), and any plain printable
    /// character types into the search. Owns **every** key while open (routed
    /// at the top of [`on_key`]).
    ///
    /// [`on_key`]: App::on_key
    pub(super) fn on_key_model_picker(&mut self, key: KeyEvent) -> Action {
        // Ctrl+C closes the picker (never quits — the composer-clear/quit rules
        // don't apply while the picker owns the keys), like the /resume picker.
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.close_model_picker();
            return Action::CloseModelPicker;
        }
        let Some(picker) = self.model_picker.as_mut() else {
            return Action::None;
        };
        let len = picker.matches().len();
        let last = len.saturating_sub(1);
        match key.code {
            KeyCode::Up => picker.selected = wrap_step(picker.selected, len, -1),
            KeyCode::Down => picker.selected = wrap_step(picker.selected, len, 1),
            KeyCode::PageUp => picker.selected = picker.selected.saturating_sub(MODEL_PAGE),
            KeyCode::PageDown => picker.selected = (picker.selected + MODEL_PAGE).min(last),
            KeyCode::Home => picker.selected = 0,
            KeyCode::End => picker.selected = last,
            KeyCode::Enter => {
                if let Some(model) = picker.matches().get(picker.selected) {
                    let action = Action::SelectModel {
                        provider: model.provider.clone(),
                        id: model.id.clone(),
                        reasoning: model.reasoning.clone(),
                        vision: model.vision,
                        context: model.context,
                        service_tiers: model.service_tiers.clone(),
                    };
                    self.close_model_picker();
                    return action;
                }
            }
            KeyCode::Esc => {
                if picker.query.is_empty() {
                    self.close_model_picker();
                    return Action::CloseModelPicker;
                }
                picker.query.clear();
                picker.selected = 0;
            }
            KeyCode::Backspace => {
                picker.query.pop();
                picker.selected = 0;
            }
            // Fetch every list again — not while a fetch is out, which would
            // only ask again for a list already on its way
            // (docs/model-cache.md). A bare `r` is a search character.
            KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if picker.pending() == 0 {
                    return Action::RefreshModels;
                }
            }
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
}
