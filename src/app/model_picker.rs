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
/// model list. Its rows come from the boundary's `/v1/models` fetch
/// ([`App::set_models`]); the filtered view derives on demand ([`matches`]).
///
/// [`matches`]: ModelPicker::matches
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ModelPicker {
    /// Every fetched model, provider-tagged and sorted (empty while loading).
    /// Grows as each provider's fetch lands ([`App::add_models`]).
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
    /// Provider fetches still outstanding — the `/model` picker fetches **every**
    /// configured provider in parallel and merges their lists as they arrive
    /// (docs/llm.md). Drives the `loading more…` counter hint, and at zero with
    /// no models it settles the all-failed error.
    pub pending: usize,
    /// Providers whose model fetch failed, kept so the picker can show which
    /// lists are missing (a `⚠ … unavailable` note beside a partial list, or
    /// one red row per provider when they all fail).
    pub errors: Vec<ModelFetchError>,
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
        } else if self.pending > 0 {
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
        picker.pending = 0;
        picker.errors.clear();
        // Seat the highlight on the active model if it's in the (unfiltered)
        // list, so the picker opens focused on the current choice.
        picker.selected = picker
            .matches()
            .iter()
            .position(|m| m.id == picker.active_id)
            .unwrap_or(0);
    }

    /// Begin a multi-provider model load: record how many provider fetches are
    /// in flight and reset to the empty `Loading` state. The boundary spawns one
    /// worker per configured provider after this; each result arrives via
    /// [`add_models`] / [`add_model_error`]. No-op if the picker closed meanwhile.
    ///
    /// [`add_models`]: App::add_models
    /// [`add_model_error`]: App::add_model_error
    pub fn begin_model_load(&mut self, pending: usize) {
        if let Some(picker) = self.model_picker.as_mut() {
            picker.models.clear();
            picker.errors.clear();
            picker.selected = 0;
            picker.pending = pending;
            picker.status = ModelLoad::Loading;
        }
    }

    /// Merge one provider's fetched models into the open picker: append, re-sort
    /// by id then provider, drop exact `(provider, id)` dupes, and mark one
    /// outstanding fetch done. The list appears as soon as the first provider
    /// lands (status → `Ready`) and grows as the rest arrive. The highlight rides
    /// the same model across the merge, unless the user hasn't touched the picker
    /// yet — then it re-seats on the active model once its provider loads. No-op
    /// if the picker closed meanwhile. See `docs/llm.md`.
    pub fn add_models(&mut self, models: Vec<ModelEntry>) {
        let Some(picker) = self.model_picker.as_mut() else {
            return;
        };
        picker.pending = picker.pending.saturating_sub(1);
        // An untouched picker (no search, highlight at the top) re-seats on the
        // active model when its provider lands; once the user navigates or
        // filters, the current highlight is preserved across the merge instead.
        let untouched = picker.query.is_empty() && picker.selected == 0;
        let keep = picker
            .highlighted()
            .map(|m| (m.provider.clone(), m.id.clone()));
        picker.models.extend(models);
        picker
            .models
            .sort_by(|a, b| a.id.cmp(&b.id).then_with(|| a.provider.cmp(&b.provider)));
        picker
            .models
            .dedup_by(|a, b| a.id == b.id && a.provider == b.provider);
        picker.recompute_status();
        picker.selected = if untouched {
            picker
                .matches()
                .iter()
                .position(|m| m.id == picker.active_id)
                .unwrap_or(0)
        } else {
            let clamped = picker
                .selected
                .min(picker.matches().len().saturating_sub(1));
            keep.and_then(|(p, i)| {
                picker
                    .matches()
                    .iter()
                    .position(|m| m.provider == p && m.id == i)
            })
            .unwrap_or(clamped)
        };
    }

    /// Record that one provider's model fetch failed: keep the reason (shown as a
    /// `⚠ … unavailable` note beside a partial list, or a red row when every
    /// provider fails) and mark the fetch done. No-op if the picker closed. See
    /// `docs/llm.md`.
    pub fn add_model_error(&mut self, provider: impl Into<String>, message: impl Into<String>) {
        let Some(picker) = self.model_picker.as_mut() else {
            return;
        };
        picker.pending = picker.pending.saturating_sub(1);
        picker.errors.push(ModelFetchError {
            provider: provider.into(),
            message: message.into(),
        });
        picker.recompute_status();
        let last = picker.matches().len().saturating_sub(1);
        picker.selected = picker.selected.min(last);
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
            picker.pending = 0;
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
