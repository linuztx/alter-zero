//! The inline `/model` picker: the per-provider model load, its search query,
//! and the selection. See `docs/llm.md`.

use super::views::TOOL_VIEW_PAGE;
use super::*;

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
/// model list. Its rows are every configured provider's listing — kept from
/// an earlier fetch, or fetched by this round ([`App::begin_model_round`],
/// [`App::finish_model_fetch`], `docs/model-cache.md`); the filtered view
/// derives on demand ([`matches`]).
///
/// [`matches`]: ModelPicker::matches
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ModelPicker {
    /// Every listed model, provider-tagged and sorted (empty while loading).
    /// Each provider's rows are its kept listing until its fetch lands and
    /// replaces them ([`App::finish_model_fetch`]).
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
    /// Every configured provider's list, in the order the boundary found
    /// them — where its rows on screen came from, whether this round is
    /// fetching it, and why its fetch failed — the `/model` picker fetches
    /// **every** configured provider in parallel and merges their lists as
    /// they arrive (`docs/llm.md`, `docs/model-cache.md`). What the counter's
    /// notes are read off (`loading more…`, `refreshing…`, `updated 2h ago`,
    /// `… unavailable`, `… not refreshed`), and with no rows and nothing
    /// out, whether the page settles on the all-failed error.
    pub lists: Vec<ProviderList>,
    /// The round of fetches in flight, or the last one: a fetch's answer is
    /// merged only when it names it ([`App::finish_model_fetch`]).
    pub round: u64,
    /// The wall clock, in Unix seconds — injected by the boundary every draw
    /// ([`App::set_model_picker_now`]), so the page says how old its lists
    /// are and reads no clock of its own.
    pub now: i64,
    /// Whether the user has moved the highlight or typed: an untouched
    /// picker re-seats on the model in use as lists land, a touched one
    /// keeps the row it is on.
    pub touched: bool,
}

/// One configured provider's list in the open picker
/// (`docs/model-cache.md`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderList {
    /// The provider's id — what its rows carry as [`ModelEntry::provider`].
    pub id: String,
    /// Its name, for the notes about it (`OpenRouter unavailable`).
    pub name: String,
    /// When the rows on screen were listed, in Unix seconds — the cache's
    /// date for kept rows, the fetch's for fresh ones; `None` while nothing
    /// of the provider's has been listed.
    pub listed_at: Option<i64>,
    /// Whether this round is fetching it.
    pub fetching: bool,
    /// Why its fetch failed, when it did.
    pub error: Option<String>,
}

/// What the boundary knows of a configured provider as a round of fetches
/// begins ([`App::begin_model_round`]): what was kept of its list, and
/// whether to fetch it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderLoad {
    /// The provider's id.
    pub id: String,
    /// Its name.
    pub name: String,
    /// When the kept rows were listed, in Unix seconds — `None` when nothing
    /// was kept.
    pub listed_at: Option<i64>,
    /// The kept rows, empty when nothing was kept.
    pub models: Vec<ModelEntry>,
    /// Whether this round fetches it.
    pub fetch: bool,
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

    /// Whether a provider with nothing on screen yet is being fetched — the
    /// counter's `loading more…`, and why a search with no match cannot say
    /// so yet: the provider still on its way may list the very model.
    #[must_use]
    pub fn loading(&self) -> bool {
        self.lists
            .iter()
            .any(|list| list.fetching && list.listed_at.is_none())
    }

    /// Whether a provider whose kept rows are on screen is being fetched
    /// again — the counter's `refreshing…` (`docs/model-cache.md`).
    #[must_use]
    pub fn refreshing(&self) -> bool {
        self.lists
            .iter()
            .any(|list| list.fetching && list.listed_at.is_some())
    }

    /// Whether any fetch of this round is still out.
    #[must_use]
    pub fn fetching(&self) -> bool {
        self.lists.iter().any(|list| list.fetching)
    }

    /// When the oldest list on screen was listed, in Unix seconds — how old
    /// the page can be, which is what its `updated` says. `None` while
    /// nothing has been listed.
    #[must_use]
    pub fn listed_at(&self) -> Option<i64> {
        self.lists.iter().filter_map(|list| list.listed_at).min()
    }

    /// The highlighted row's `(provider, id)`, the key a merge finds it
    /// again by.
    fn highlighted_key(&self) -> Option<(String, String)> {
        self.highlighted()
            .map(|m| (m.provider.clone(), m.id.clone()))
    }

    /// Sort the rows by id then provider and drop exact `(provider, id)`
    /// duplicates — the merged list's one order, however the rows arrived.
    fn sort_rows(&mut self) {
        self.models
            .sort_by(|a, b| a.id.cmp(&b.id).then_with(|| a.provider.cmp(&b.provider)));
        self.models
            .dedup_by(|a, b| a.id == b.id && a.provider == b.provider);
    }

    /// Seat the highlight after the rows changed under it: an untouched
    /// picker on the model in use (the top when it is not listed), a touched
    /// one on the row it was on (`keep`), else as near its old place as the
    /// list now allows.
    fn reseat(&mut self, keep: Option<(String, String)>) {
        let matches = self.matches();
        let selected = if self.touched {
            keep.and_then(|(provider, id)| {
                matches
                    .iter()
                    .position(|m| m.provider == provider && m.id == id)
            })
            .unwrap_or_else(|| self.selected.min(matches.len().saturating_sub(1)))
        } else {
            matches.iter().position(|m| self.is_active(m)).unwrap_or(0)
        };
        self.selected = selected;
    }

    /// Recompute the display status from the merge so far: `Ready` the moment any
    /// model is present (shown while the rest still load), else `Loading` while
    /// fetches are outstanding, else `Error` when every provider failed with no
    /// models, else `Ready` (all done, genuinely empty). The `Error` string is
    /// left empty — the picker renders each failed list's reason instead.
    fn recompute_status(&mut self) {
        self.status = if !self.models.is_empty() {
            ModelLoad::Ready
        } else if self.fetching() {
            ModelLoad::Loading
        } else if self.lists.iter().any(|list| list.error.is_some()) {
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
        picker.lists.clear();
        // Seat the highlight on the active model if it's in the (unfiltered)
        // list, so the picker opens focused on the current choice.
        picker.selected = picker
            .matches()
            .iter()
            .position(|m| m.id == picker.active_id)
            .unwrap_or(0);
    }

    /// Lay a round of fetches over the open picker — on open, and on every
    /// Ctrl+R — returning the round every result must name, or `None` when
    /// the picker is closed. `loads` is every configured provider, in the
    /// order the boundary found them (`docs/model-cache.md`):
    ///
    /// - a provider the picker already shows keeps the rows on screen, and
    ///   their date, through the round — a refresh never blanks a list it
    ///   is about to replace;
    /// - any other shows the rows the boundary kept for it, at once;
    /// - a provider the picker shows that is not in `loads` any more (its
    ///   key went, its block left `providers.toml`) leaves with its rows.
    ///
    /// A provider the round fetches starts clean of its last failure. The
    /// highlight follows [`ModelPicker::touched`]'s rule.
    pub fn begin_model_round(&mut self, loads: Vec<ProviderLoad>) -> Option<u64> {
        let picker = self.model_picker.as_mut()?;
        self.model_rounds += 1;
        picker.round = self.model_rounds;
        let keep = picker.highlighted_key();
        let previous = std::mem::take(&mut picker.lists);
        let mut shown = std::mem::take(&mut picker.models);
        let mut rows = Vec::new();
        for load in loads {
            let on_screen = previous
                .iter()
                .find(|list| list.id == load.id)
                .and_then(|list| list.listed_at);
            let listed_at = if let Some(at) = on_screen {
                let (own, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut shown)
                    .into_iter()
                    .partition(|m| m.provider == load.id);
                shown = rest;
                rows.extend(own);
                Some(at)
            } else {
                rows.extend(load.models);
                load.listed_at
            };
            picker.lists.push(ProviderList {
                id: load.id,
                name: load.name,
                listed_at,
                fetching: load.fetch,
                error: None,
            });
        }
        picker.models = rows;
        picker.sort_rows();
        picker.recompute_status();
        picker.reseat(keep);
        Some(picker.round)
    }

    /// One provider's fetch landed. Dropped when the picker has closed, or
    /// when `round` is not the picker's own — an earlier round's answer, or
    /// a closed picker's. A listing **replaces** the provider's rows (a
    /// model it no longer lists goes) and dates them `listed_at`, the time
    /// the boundary kept them at; a failure keeps whatever rows the
    /// provider showed beside the reason — kept rows stay usable, and the
    /// page says they could not be refreshed.
    pub fn finish_model_fetch(
        &mut self,
        round: u64,
        provider: &str,
        result: Result<Vec<ModelEntry>, String>,
        listed_at: i64,
    ) {
        let Some(picker) = self
            .model_picker
            .as_mut()
            .filter(|picker| picker.round == round)
        else {
            return;
        };
        let keep = picker.highlighted_key();
        let Some(list) = picker
            .lists
            .iter_mut()
            .find(|list| list.id == provider && list.fetching)
        else {
            return;
        };
        list.fetching = false;
        match result {
            Ok(models) => {
                list.listed_at = Some(listed_at);
                list.error = None;
                picker.models.retain(|m| m.provider != provider);
                picker.models.extend(models);
                picker.sort_rows();
            }
            Err(message) => list.error = Some(message),
        }
        picker.recompute_status();
        picker.reseat(keep);
    }

    /// Inject the wall clock (Unix seconds) into the open picker — the
    /// boundary does it every draw, as it does for `/usage`.
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
            picker.lists.clear();
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
        picker.touched = true;
    }

    /// Keys while the inline `/model` picker is open. Mirrors the `/resume`
    /// picker's grammar: ↑/↓ move (wrapping at the ends), PageUp/PageDown jump
    /// by [`MODEL_PAGE`] (clamped), Home/End to the ends, Enter selects the
    /// highlighted model, Esc clears a non-empty search before it closes,
    /// Backspace pops, Ctrl+C closes, and any plain printable character types
    /// into the search. Owns **every** key while open (routed at the top of
    /// [`on_key`]).
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
        // Ctrl+R asks every provider again (`docs/model-cache.md`) — not a
        // plain `r`, which has to keep typing into the search
        // (`deepseek-r1`). Not while a round is still out: it already
        // answers the question, the `/usage` page's `r` rule.
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('r') {
            return if picker.fetching() {
                Action::None
            } else {
                Action::RefreshModels
            };
        }
        let len = picker.matches().len();
        let last = len.saturating_sub(1);
        // Every key but Enter, Esc on an empty search and the ones that do
        // nothing is the user taking the highlight over.
        if !matches!(key.code, KeyCode::Enter | KeyCode::Esc) {
            picker.touched = true;
        }
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
                picker.touched = true;
            }
            KeyCode::Backspace => {
                picker.query.pop();
                picker.selected = 0;
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
