//! The inline `/login` API-key onboarding: pick a provider, then paste its
//! key. See `docs/llm.md`.

use super::views::TOOL_VIEW_PAGE;
use super::*;

/// How many rows PageUp/PageDown move the `/login` provider list.
const LOGIN_PAGE: usize = TOOL_VIEW_PAGE;

/// Which step of the inline `/login` onboarding flow is showing (docs/llm.md).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum KeyStep {
    /// Choosing which provider to set a key for (a filterable list).
    #[default]
    Provider,
    /// Entering (pasting) the API key for the chosen provider.
    Key,
}

/// One selectable provider row in the `/login` flow — plain data injected by the
/// boundary (the `configured` flag needs boundary key resolution, like the
/// `/model` list). See `docs/llm.md`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderChoice {
    /// The provider id (e.g. `openrouter`).
    pub id: String,
    /// The human-readable label (the `providers.toml` `name`).
    pub name: String,
    /// The environment variable its key is stored under (e.g. `OPENROUTER_API_KEY`).
    pub env_var: String,
    /// Whether a key already resolves for it (shown with a ✓).
    pub configured: bool,
}

/// The inline `/login` onboarding flow's state (`None` on [`App`] when closed).
/// Like the `/model` picker it **replaces the composer** in the bottom live
/// region; unlike it, it's a two-step flow — pick a provider
/// ([`KeyStep::Provider`]), then enter its API key masked ([`KeyStep::Key`]).
/// The chosen key is persisted to `.env` by the boundary
/// ([`Action::SaveApiKey`]). See `docs/llm.md`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct KeyOnboarding {
    /// The providers to choose from, injected at open
    /// ([`App::open_key_onboarding`]).
    pub providers: Vec<ProviderChoice>,
    /// Which step is showing.
    pub step: KeyStep,
    /// Index of the highlighted provider within the current filtered matches.
    pub selected: usize,
    /// The provider filter query (step 1).
    pub query: String,
    /// The provider being keyed (an index into `providers`), set when step 2
    /// opens so the key entry keeps its provider even as the (unused) filter
    /// would otherwise reorder matches.
    pub chosen: Option<usize>,
    /// The API key being typed / pasted (step 2). Rendered masked.
    pub key_input: String,
    /// Where saved keys land (the `~`-relative `.env` path), injected at open by
    /// the boundary so the provider-step hint names the real file even under an
    /// `ALTER_ZERO_ENV_FILE` override. See `docs/llm.md`.
    pub env_path: String,
}

impl KeyOnboarding {
    /// Providers whose id or name contains the query (case-insensitive
    /// substring; every provider for an empty query), keeping the injected
    /// order.
    #[must_use]
    pub fn matches(&self) -> Vec<&ProviderChoice> {
        let query = self.query.to_lowercase();
        self.providers
            .iter()
            .filter(|p| {
                query.is_empty()
                    || p.id.to_lowercase().contains(&query)
                    || p.name.to_lowercase().contains(&query)
            })
            .collect()
    }

    /// The highlighted provider in the current filtered view (step 1).
    #[must_use]
    pub fn highlighted(&self) -> Option<&ProviderChoice> {
        self.matches().get(self.selected).copied()
    }

    /// The provider chosen for key entry (step 2), if any.
    #[must_use]
    pub fn chosen_provider(&self) -> Option<&ProviderChoice> {
        self.chosen.and_then(|i| self.providers.get(i))
    }
}

impl App {
    /// Open the inline `/login` onboarding flow with the given provider choices
    /// (built at the boundary so the `configured` ✓ reflects the real env / `.env`
    /// key resolution) and the `~`-relative `.env` path the provider-step hint
    /// names. Starts on the provider step; abandons any band / palette / file
    /// picker / model picker it shares the composer with, staying in
    /// [`View::Conversation`]. See `docs/llm.md`.
    pub fn open_key_onboarding(
        &mut self,
        providers: Vec<ProviderChoice>,
        env_path: impl Into<String>,
    ) {
        self.shortcuts_open = false;
        self.command_menu = None;
        self.file_search = None;
        self.model_picker = None;
        self.backtrack = Backtrack::default();
        self.key_onboarding = Some(KeyOnboarding {
            providers,
            env_path: env_path.into(),
            ..KeyOnboarding::default()
        });
    }

    /// Dismiss the inline `/login` flow (Esc on the empty provider filter,
    /// Ctrl+C, or right after saving a key): the composer returns. No view
    /// change — it was never an overlay.
    pub fn close_key_onboarding(&mut self) {
        self.key_onboarding = None;
    }

    /// Keys while the inline `/login` flow is open. The two steps have distinct
    /// grammars:
    ///
    /// - **Provider** (a filterable list): ↑/↓/PageUp/PageDown/Home/End move,
    ///   Enter advances to key entry for the highlighted provider, type-to-filter
    ///   with Backspace, Esc clears a non-empty filter then closes, Ctrl+C closes.
    /// - **Key** (masked entry): printable keys and Backspace edit the key, Enter
    ///   saves a non-empty key ([`Action::SaveApiKey`]) and closes, Esc steps
    ///   *back* to the provider list, Ctrl+C closes.
    ///
    /// Owns **every** key while open (routed at the top of [`on_key`]).
    ///
    /// [`on_key`]: App::on_key
    pub(super) fn on_key_key_onboarding(&mut self, key: KeyEvent) -> Action {
        // Ctrl+C closes the whole flow (like the /model picker), from either step.
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.close_key_onboarding();
            return Action::CloseKeyOnboarding;
        }
        let Some(onboarding) = self.key_onboarding.as_mut() else {
            return Action::None;
        };
        match onboarding.step {
            KeyStep::Provider => {
                let last = onboarding.matches().len().saturating_sub(1);
                match key.code {
                    KeyCode::Up => onboarding.selected = onboarding.selected.saturating_sub(1),
                    KeyCode::Down => onboarding.selected = (onboarding.selected + 1).min(last),
                    KeyCode::PageUp => {
                        onboarding.selected = onboarding.selected.saturating_sub(LOGIN_PAGE);
                    }
                    KeyCode::PageDown => {
                        onboarding.selected = (onboarding.selected + LOGIN_PAGE).min(last);
                    }
                    KeyCode::Home => onboarding.selected = 0,
                    KeyCode::End => onboarding.selected = last,
                    KeyCode::Enter => {
                        // Pin the highlighted provider's index in the *unfiltered*
                        // list (the filter can reorder/shrink the matches), then
                        // switch to masked key entry for it.
                        let chosen = onboarding
                            .highlighted()
                            .map(|c| c.id.clone())
                            .and_then(|id| onboarding.providers.iter().position(|p| p.id == id));
                        if let Some(idx) = chosen {
                            onboarding.chosen = Some(idx);
                            onboarding.step = KeyStep::Key;
                            onboarding.key_input.clear();
                        }
                    }
                    KeyCode::Esc => {
                        if onboarding.query.is_empty() {
                            self.close_key_onboarding();
                            return Action::CloseKeyOnboarding;
                        }
                        onboarding.query.clear();
                        onboarding.selected = 0;
                    }
                    KeyCode::Backspace => {
                        onboarding.query.pop();
                        onboarding.selected = 0;
                    }
                    KeyCode::Char(c)
                        if !key
                            .modifiers
                            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                    {
                        onboarding.query.push(c);
                        onboarding.selected = 0;
                    }
                    _ => {}
                }
                Action::None
            }
            KeyStep::Key => match key.code {
                KeyCode::Enter => {
                    let entered = onboarding.key_input.trim().to_string();
                    if entered.is_empty() {
                        return Action::None;
                    }
                    let Some(choice) = onboarding.chosen_provider() else {
                        return Action::None;
                    };
                    let action = Action::SaveApiKey {
                        provider: choice.id.clone(),
                        env_var: choice.env_var.clone(),
                        key: entered,
                    };
                    self.close_key_onboarding();
                    action
                }
                KeyCode::Esc => {
                    // Step back to the provider list rather than closing outright.
                    onboarding.step = KeyStep::Provider;
                    onboarding.key_input.clear();
                    onboarding.chosen = None;
                    Action::None
                }
                KeyCode::Backspace => {
                    onboarding.key_input.pop();
                    Action::None
                }
                KeyCode::Char(c)
                    if !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                {
                    onboarding.key_input.push(c);
                    Action::None
                }
                _ => Action::None,
            },
        }
    }

    /// A bracketed paste while the `/login` flow is open. On the key step the
    /// pasted text is the API key — interior whitespace and control characters
    /// (a trailing newline from the paste, say) are dropped and the rest
    /// appended. On the provider step it extends the filter query like a
    /// [`paste_into_resume_search`], whitespace collapsed.
    ///
    /// [`paste_into_resume_search`]: App::paste_into_resume_search
    pub fn paste_into_key_onboarding(&mut self, pasted: &str) {
        let Some(onboarding) = self.key_onboarding.as_mut() else {
            return;
        };
        match onboarding.step {
            KeyStep::Key => {
                let cleaned: String = pasted
                    .chars()
                    .filter(|c| !c.is_whitespace() && !c.is_control())
                    .collect();
                onboarding.key_input.push_str(&cleaned);
            }
            KeyStep::Provider => {
                let flat = pasted.split_whitespace().collect::<Vec<_>>().join(" ");
                if flat.is_empty() {
                    return;
                }
                if !onboarding.query.is_empty() {
                    onboarding.query.push(' ');
                }
                onboarding.query.push_str(&flat);
                onboarding.selected = 0;
            }
        }
    }
}
