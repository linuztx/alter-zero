//! The inline `/login` onboarding: pick how you sign in — a **subscription**
//! (GitHub Copilot's device flow) or an **API key** (pick a provider, paste its
//! key). See `docs/llm.md` and `docs/copilot.md`.

use std::time::Duration;

use super::views::TOOL_VIEW_PAGE;
use super::*;

/// How many rows PageUp/PageDown move the `/login` lists.
const LOGIN_PAGE: usize = TOOL_VIEW_PAGE;

/// Which step of the inline `/login` onboarding flow is showing
/// (`docs/llm.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum KeyStep {
    /// The root: which way in — a subscription, or an API key.
    #[default]
    Method,
    /// Choosing which subscription to sign in to (a filterable list).
    Subscription,
    /// The chosen subscription's device-code page (its code, and the wait).
    Device,
    /// Choosing which provider to set a key for (a filterable list).
    Provider,
    /// Entering (pasting) the API key for the chosen provider.
    Key,
}

/// The two ways in, in the order the method step lists them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginMethod {
    /// Sign in to a subscription that serves models (GitHub Copilot).
    Subscription,
    /// Paste a provider's API key.
    ApiKey,
}

impl LoginMethod {
    /// The methods in list order.
    pub const ALL: [Self; 2] = [Self::Subscription, Self::ApiKey];

    /// The row label.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Subscription => "Use a subscription",
            Self::ApiKey => "Use an API key",
        }
    }
}

/// One selectable provider row in the `/login` API-key flow — plain data
/// injected by the boundary (the `configured` flag needs boundary key
/// resolution, like the `/model` list). See `docs/llm.md`.
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

/// One selectable subscription row — the same shape as [`ProviderChoice`] but
/// carrying a *description* instead of an env var, since signing in is a flow
/// rather than a secret to paste. See `docs/copilot.md`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubscriptionChoice {
    /// The provider id the sign-in configures (e.g. `github_copilot`).
    pub id: String,
    /// The human-readable label (e.g. `GitHub Copilot`).
    pub name: String,
    /// What signing in means, shown dim beside the name.
    pub description: String,
    /// Whether this subscription is already signed in (shown with a ✓).
    pub configured: bool,
}

/// How far the device-code sign-in has got. The page stays up through every
/// one of these — only Esc (or success) takes it down.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum DeviceStatus {
    /// The code is being requested from the provider.
    #[default]
    Starting,
    /// The code is on screen; the provider is being polled for approval.
    Waiting,
    /// The flow failed — the reason is shown in place of the wait line.
    Failed(String),
}

/// The device-code page's state: what to show, and how long is left. The
/// **boundary** owns the flow itself (the worker thread polling the provider)
/// and feeds this through [`App::set_device_code`] /
/// [`App::fail_device_login`] / [`App::set_device_remaining`] — the
/// clock-injection pattern, so the pure core never reads a clock. See
/// `docs/copilot.md`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DeviceLogin {
    /// The provider id being signed in to (e.g. `github_copilot`).
    pub provider_id: String,
    /// Its label, for the `Sign in to {name}` title.
    pub provider_name: String,
    /// Where the user enters the code (empty until the code arrives).
    pub verification_uri: String,
    /// The one-time code shown in the box (empty until it arrives).
    pub user_code: String,
    /// How far along the flow is.
    pub status: DeviceStatus,
    /// How long the code is still valid — injected per draw by the boundary.
    pub remaining: Option<Duration>,
}

impl DeviceLogin {
    /// The code, once there is one to copy.
    #[must_use]
    pub fn code(&self) -> Option<&str> {
        (!self.user_code.is_empty()).then_some(self.user_code.as_str())
    }
}

/// The inline `/login` onboarding flow's state (`None` on [`App`] when closed).
/// Like the `/model` picker it **replaces the composer** in the bottom live
/// region; unlike it, it's a multi-step flow rooted at [`KeyStep::Method`] —
/// a subscription sign-in ([`KeyStep::Subscription`] → [`KeyStep::Device`]) or
/// an API key ([`KeyStep::Provider`] → [`KeyStep::Key`]). The chosen key is
/// persisted to `.env` by the boundary ([`Action::SaveApiKey`]). See
/// `docs/llm.md` and `docs/copilot.md`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct KeyOnboarding {
    /// The API-key providers to choose from, injected at open
    /// ([`App::open_key_onboarding`]).
    pub providers: Vec<ProviderChoice>,
    /// The subscriptions to choose from, injected at the same time.
    pub subscriptions: Vec<SubscriptionChoice>,
    /// Which step is showing.
    pub step: KeyStep,
    /// Index of the highlighted row within the current step's filtered
    /// matches. The three list steps share it — only one list shows at a time,
    /// and every step change resets it.
    pub selected: usize,
    /// The current list step's filter query — shared for the same reason as
    /// [`selected`](Self::selected).
    pub query: String,
    /// The provider being keyed (an index into `providers`), set when the key
    /// step opens so the key entry keeps its provider even as the (unused)
    /// filter would otherwise reorder matches.
    pub chosen: Option<usize>,
    /// The API key being typed / pasted (the key step). Rendered masked.
    pub key_input: String,
    /// The open device-code page ([`KeyStep::Device`]); `None` otherwise.
    pub device: Option<DeviceLogin>,
    /// Where saved keys land (the `~`-relative `.env` path), injected at open by
    /// the boundary so the provider-step hint names the real file even under an
    /// `ALTER_ZERO_ENV_FILE` override. See `docs/llm.md`.
    pub env_path: String,
}

impl KeyOnboarding {
    /// The method rows in list order, filtered by the query — the labels the
    /// method step shows.
    #[must_use]
    pub fn method_matches(&self) -> Vec<LoginMethod> {
        let query = self.query.to_lowercase();
        LoginMethod::ALL
            .into_iter()
            .filter(|m| query.is_empty() || m.label().to_lowercase().contains(&query))
            .collect()
    }

    /// The method labels, for tests and the renderer.
    #[must_use]
    pub fn method_labels(&self) -> Vec<&'static str> {
        self.method_matches()
            .into_iter()
            .map(LoginMethod::label)
            .collect()
    }

    /// The highlighted method (the method step).
    #[must_use]
    pub fn highlighted_method(&self) -> Option<LoginMethod> {
        self.method_matches().get(self.selected).copied()
    }

    /// Subscriptions whose id, name or description contains the query
    /// (case-insensitive substring; all of them for an empty query), keeping
    /// the injected order.
    #[must_use]
    pub fn subscription_matches(&self) -> Vec<&SubscriptionChoice> {
        let query = self.query.to_lowercase();
        self.subscriptions
            .iter()
            .filter(|s| {
                query.is_empty()
                    || s.id.to_lowercase().contains(&query)
                    || s.name.to_lowercase().contains(&query)
                    || s.description.to_lowercase().contains(&query)
            })
            .collect()
    }

    /// The highlighted subscription (the subscription step).
    #[must_use]
    pub fn highlighted_subscription(&self) -> Option<&SubscriptionChoice> {
        self.subscription_matches().get(self.selected).copied()
    }

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

    /// The highlighted provider in the current filtered view (the provider
    /// step).
    #[must_use]
    pub fn highlighted(&self) -> Option<&ProviderChoice> {
        self.matches().get(self.selected).copied()
    }

    /// The provider chosen for key entry (the key step), if any.
    #[must_use]
    pub fn chosen_provider(&self) -> Option<&ProviderChoice> {
        self.chosen.and_then(|i| self.providers.get(i))
    }

    /// How many rows the current list step offers — the wrap/clamp bound the
    /// key handling shares across the three lists.
    fn list_len(&self) -> usize {
        match self.step {
            KeyStep::Method => self.method_matches().len(),
            KeyStep::Subscription => self.subscription_matches().len(),
            KeyStep::Provider => self.matches().len(),
            KeyStep::Device | KeyStep::Key => 0,
        }
    }

    /// Move to `step`, resetting the shared filter and selection — every list
    /// step opens at the top with a clean query.
    fn go(&mut self, step: KeyStep) {
        self.step = step;
        self.selected = 0;
        self.query.clear();
    }
}

impl App {
    /// Open the inline `/login` onboarding flow with the given API-key provider
    /// and subscription choices (built at the boundary so the `configured` ✓
    /// reflects real key resolution) and the `~`-relative `.env` path the
    /// provider-step hint names. Starts on the method step; abandons any band /
    /// palette / file picker / model picker it shares the composer with,
    /// staying in [`View::Conversation`]. See `docs/llm.md`.
    pub fn open_key_onboarding(
        &mut self,
        providers: Vec<ProviderChoice>,
        subscriptions: Vec<SubscriptionChoice>,
        env_path: impl Into<String>,
    ) {
        self.shortcuts_open = false;
        self.command_menu = None;
        self.file_search = None;
        self.skill_picker = None;
        self.model_picker = None;
        self.backtrack = Backtrack::default();
        self.key_onboarding = Some(KeyOnboarding {
            providers,
            subscriptions,
            env_path: env_path.into(),
            ..KeyOnboarding::default()
        });
    }

    /// Dismiss the inline `/login` flow (Esc on the empty method filter,
    /// Ctrl+C, or right after saving a key): the composer returns. No view
    /// change — it was never an overlay.
    pub fn close_key_onboarding(&mut self) {
        self.key_onboarding = None;
    }

    /// Is a device-code sign-in on screen? The frame chain re-arms while one
    /// is (its countdown ticks), and the boundary only feeds a flow that is
    /// still being watched.
    #[must_use]
    pub fn device_login_active(&self) -> bool {
        self.key_onboarding
            .as_ref()
            .is_some_and(|o| o.device.is_some())
    }

    /// The device page's mutable state, when one is open.
    fn device_mut(&mut self) -> Option<&mut DeviceLogin> {
        self.key_onboarding.as_mut()?.device.as_mut()
    }

    /// The boundary got a code back from the provider: show it, and start
    /// waiting. See `docs/copilot.md`.
    pub fn set_device_code(&mut self, verification_uri: &str, user_code: &str) {
        if let Some(device) = self.device_mut() {
            device.verification_uri = verification_uri.to_string();
            device.user_code = user_code.to_string();
            device.status = DeviceStatus::Waiting;
        }
    }

    /// The device flow failed: report it **on the page**, which stays up so the
    /// reason can be read (Esc takes it down).
    pub fn fail_device_login(&mut self, reason: impl Into<String>) {
        if let Some(device) = self.device_mut() {
            device.status = DeviceStatus::Failed(reason.into());
        }
    }

    /// How long the shown code is still valid — injected per draw by the
    /// boundary, exactly like the status line's elapsed
    /// ([`App::set_status_times`]).
    pub fn set_device_remaining(&mut self, remaining: Option<Duration>) {
        if let Some(device) = self.device_mut() {
            device.remaining = remaining;
        }
    }

    /// Keys while the inline `/login` flow is open. The steps have distinct
    /// grammars:
    ///
    /// - **The three lists** (method / subscription / provider): ↑/↓ move
    ///   wrapping at the ends, PageUp/PageDown/Home/End jump (clamped),
    ///   type-to-filter with Backspace, `Enter` activates the highlighted row,
    ///   `Esc` clears a non-empty filter then steps *back* (the method step,
    ///   being the root, closes instead), `Ctrl+C` closes.
    /// - **Device** (the code page): `c` copies the code once there is one,
    ///   `Esc` cancels the sign-in back to the subscription list, `Ctrl+C`
    ///   closes.
    /// - **Key** (masked entry): printable keys and Backspace edit the key,
    ///   `Enter` saves a non-empty key ([`Action::SaveApiKey`]) and closes,
    ///   `Esc` steps *back* to the provider list, `Ctrl+C` closes.
    ///
    /// Owns **every** key while open (routed at the top of [`on_key`]).
    ///
    /// [`on_key`]: App::on_key
    pub(super) fn on_key_key_onboarding(&mut self, key: KeyEvent) -> Action {
        // Ctrl+C closes the whole flow (like the /model picker), from any step.
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            let cancelled = self.device_login_active();
            self.close_key_onboarding();
            return if cancelled {
                Action::CancelDeviceLogin
            } else {
                Action::CloseKeyOnboarding
            };
        }
        let Some(step) = self.key_onboarding.as_ref().map(|o| o.step) else {
            return Action::None;
        };
        match step {
            KeyStep::Method | KeyStep::Subscription | KeyStep::Provider => {
                self.on_key_login_list(key)
            }
            KeyStep::Device => self.on_key_device(key),
            KeyStep::Key => self.on_key_login_key(key),
        }
    }

    /// The shared grammar of the three list steps — navigation and
    /// type-to-filter are identical; only `Enter` and `Esc` differ, and both
    /// are one match on the step.
    fn on_key_login_list(&mut self, key: KeyEvent) -> Action {
        let Some(onboarding) = self.key_onboarding.as_mut() else {
            return Action::None;
        };
        let len = onboarding.list_len();
        let last = len.saturating_sub(1);
        match key.code {
            KeyCode::Up => onboarding.selected = wrap_step(onboarding.selected, len, -1),
            KeyCode::Down => onboarding.selected = wrap_step(onboarding.selected, len, 1),
            KeyCode::PageUp => onboarding.selected = onboarding.selected.saturating_sub(LOGIN_PAGE),
            KeyCode::PageDown => onboarding.selected = (onboarding.selected + LOGIN_PAGE).min(last),
            KeyCode::Home => onboarding.selected = 0,
            KeyCode::End => onboarding.selected = last,
            KeyCode::Enter => return self.activate_login_row(),
            KeyCode::Esc => {
                if !onboarding.query.is_empty() {
                    onboarding.query.clear();
                    onboarding.selected = 0;
                    return Action::None;
                }
                // The method step is the root: Esc there closes the flow; the
                // two lists below it step back to it.
                if onboarding.step == KeyStep::Method {
                    self.close_key_onboarding();
                    return Action::CloseKeyOnboarding;
                }
                onboarding.go(KeyStep::Method);
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

    /// `Enter` on a list step: descend into what the highlighted row names.
    fn activate_login_row(&mut self) -> Action {
        let Some(onboarding) = self.key_onboarding.as_mut() else {
            return Action::None;
        };
        match onboarding.step {
            KeyStep::Method => {
                match onboarding.highlighted_method() {
                    Some(LoginMethod::Subscription) => onboarding.go(KeyStep::Subscription),
                    Some(LoginMethod::ApiKey) => onboarding.go(KeyStep::Provider),
                    None => {}
                }
                Action::None
            }
            KeyStep::Subscription => {
                let Some(choice) = onboarding.highlighted_subscription().cloned() else {
                    return Action::None;
                };
                onboarding.go(KeyStep::Device);
                onboarding.device = Some(DeviceLogin {
                    provider_id: choice.id.clone(),
                    provider_name: choice.name,
                    ..DeviceLogin::default()
                });
                Action::StartDeviceLogin(choice.id)
            }
            KeyStep::Provider => {
                // Pin the highlighted provider's index in the *unfiltered*
                // list (the filter can reorder/shrink the matches), then
                // switch to masked key entry for it.
                let chosen = onboarding
                    .highlighted()
                    .map(|c| c.id.clone())
                    .and_then(|id| onboarding.providers.iter().position(|p| p.id == id));
                if let Some(idx) = chosen {
                    onboarding.go(KeyStep::Key);
                    onboarding.chosen = Some(idx);
                    onboarding.key_input.clear();
                }
                Action::None
            }
            KeyStep::Device | KeyStep::Key => Action::None,
        }
    }

    /// Keys on the device-code page: `c` copies the code, `Esc` cancels the
    /// sign-in back to the subscription list. Everything else is swallowed —
    /// the page is a wait, not a field.
    fn on_key_device(&mut self, key: KeyEvent) -> Action {
        let Some(onboarding) = self.key_onboarding.as_mut() else {
            return Action::None;
        };
        match key.code {
            KeyCode::Char('c' | 'C') => onboarding
                .device
                .as_ref()
                .and_then(DeviceLogin::code)
                .map_or(Action::None, |code| {
                    Action::CopyDeviceCode(code.to_string())
                }),
            KeyCode::Esc => {
                onboarding.device = None;
                onboarding.go(KeyStep::Subscription);
                Action::CancelDeviceLogin
            }
            _ => Action::None,
        }
    }

    /// Keys on the masked key-entry step.
    fn on_key_login_key(&mut self, key: KeyEvent) -> Action {
        let Some(onboarding) = self.key_onboarding.as_mut() else {
            return Action::None;
        };
        match key.code {
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
                onboarding.go(KeyStep::Provider);
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
        }
    }

    /// A bracketed paste while the `/login` flow is open. On the key step the
    /// pasted text is the API key — interior whitespace and control characters
    /// (a trailing newline from the paste, say) are dropped and the rest
    /// appended. On a list step it extends the filter query like a
    /// [`paste_into_resume_search`], whitespace collapsed; the device page has
    /// no field, so a paste there is ignored.
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
            KeyStep::Device => {}
            KeyStep::Method | KeyStep::Subscription | KeyStep::Provider => {
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
