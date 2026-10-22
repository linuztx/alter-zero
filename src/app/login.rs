//! The inline `/login` onboarding: pick how you sign in — a **subscription**
//! (GitHub Copilot's device flow, ChatGPT Codex's browser or device code,
//! Anthropic's Console browser sign-in, Cline's device code), an **API key**
//! (pick a provider, paste its key), or a **local model** (pick a server —
//! Ollama, LM Studio — enter its address, and the connection is tested before
//! anything is saved). See `docs/llm.md`, `docs/copilot.md`,
//! `docs/chatgpt.md`, `docs/claude.md`, `docs/cline.md` and
//! `docs/local-models.md`.

use std::time::Duration;

use super::views::TOOL_VIEW_PAGE;
use super::*;
use crate::llm::local::Verdict;

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
    /// Choosing **how** the chosen subscription signs in, when it offers more
    /// than one way — ChatGPT Codex's browser or device code
    /// (`docs/chatgpt.md`). A titled two-row question, not a searchable
    /// list; a subscription offering one way skips it.
    SigninMethod,
    /// The chosen subscription's device-code page (its code, and the wait).
    Device,
    /// Choosing which provider to set a key for (a filterable list).
    Provider,
    /// Entering (pasting) the API key for the chosen provider.
    Key,
    /// Choosing which local model server to connect (a filterable list —
    /// Ollama, LM Studio, and any keyless server of the user's own).
    Local,
    /// The chosen server's page: its address, tested before it is saved —
    /// and its API token, when the server turns out to want one
    /// ([`ServerPage`], `docs/local-models.md`).
    Server,
}

/// The three ways in, in the order the method step lists them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginMethod {
    /// Sign in to a subscription that serves models (GitHub Copilot).
    Subscription,
    /// Paste a provider's API key.
    ApiKey,
    /// Connect a model server running on this machine or the network
    /// (Ollama, LM Studio) — no key, an address (`docs/local-models.md`).
    Local,
}

impl LoginMethod {
    /// The methods in list order.
    pub const ALL: [Self; 3] = [Self::Subscription, Self::ApiKey, Self::Local];

    /// The row label.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Subscription => "Use a subscription",
            Self::ApiKey => "Use an API key",
            Self::Local => "Use a local model",
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
    /// The environment variable the key is stored under
    /// (`OPENROUTER_API_KEY`).
    pub env_var: String,
    /// Whether the provider is already configured (the row's `✔ configured`):
    /// a key resolves — or, for a keyless server, it is pointed at.
    pub configured: bool,
    /// One line saying what the provider is, shown on the key step above the
    /// field (`providers.toml`'s `description`). Empty when the file names
    /// none, which drops the block rather than printing a blank.
    pub description: String,
    /// Where this provider's keys are created (`providers.toml`'s
    /// `api_key_url`), linked under the description. Empty when unnamed.
    pub key_url: String,
}

/// One selectable row of `/login`'s **local model** list — a provider whose
/// key is optional (`auth = "optional_key"`): a server the user runs, which
/// is configured by its **address** rather than a secret. Plain data
/// injected by the boundary, which resolves what is in force now. See
/// `docs/local-models.md`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LocalChoice {
    /// The provider id (e.g. `lmstudio`).
    pub id: String,
    /// The human-readable label (e.g. `LM Studio`).
    pub name: String,
    /// What the server is — the file's `description`, shown on its page.
    /// Empty when the file names none.
    pub description: String,
    /// Where the server comes from (the file's `api_key_url`), linked as
    /// `Install it from …`. Empty when unnamed.
    pub install_url: String,
    /// Whether the server is pointed at already (the row's `✔ configured`).
    pub configured: bool,
    /// The variable the address is saved under (`OLLAMA_HOST`,
    /// `LMSTUDIO_HOST` — the file's `api_base_env`), or `None` for a server
    /// its provider block fixes in place: there is nothing to save, and the
    /// page only tests it.
    pub host_env: Option<String>,
    /// The address in force — the variable's value, else the block's own —
    /// which the empty field shows and an empty Enter tests and saves.
    pub host: String,
    /// The variable an API token is saved under (the file's `api_key_env`:
    /// `LM_API_TOKEN`, `OLLAMA_HOST_API_KEY`), should the server want one.
    pub token_env: String,
}

/// One connection test of a local server: the address tried, and the token
/// tried with it — the boundary's answer is matched against the test in
/// flight by these, so an answer to a test the user moved on from is
/// dropped rather than applied to the next one.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ServerTest {
    /// The address as typed (or the one in force, for an empty field).
    pub host: String,
    /// The token typed on the token step; `None` before the server asked for
    /// one (a stored token still rides — the boundary resolves it).
    pub token: Option<String>,
}

/// The server answered `unauthorized`: the token step's subject.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TokenAsk {
    /// The address the server answered at — what is saved once a token
    /// passes.
    pub host: String,
    /// The sentence saying so (`LM Studio at 127.0.0.1:1234 wants an API
    /// token.`), shown above the token field.
    pub reason: String,
}

/// The `/login` local server page ([`KeyStep::Server`]): an address field
/// whose Enter **tests** the server before anything is saved, and — when the
/// server refuses the test as unauthorized — a masked token field for the
/// same address. See `docs/local-models.md`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ServerPage {
    /// The address being typed; empty means the one in force
    /// ([`LocalChoice::host`]).
    pub address: String,
    /// The token being typed, on the token step.
    pub token: String,
    /// Asking for a token — the server at this address wants one. `None`
    /// while the address is being asked for.
    pub token_for: Option<TokenAsk>,
    /// The test in flight, when one is: the field waits, Esc stops it.
    pub testing: Option<ServerTest>,
    /// Why the last test failed, shown under the field until it is edited.
    pub failure: Option<String>,
}

impl ServerPage {
    /// Is the page asking for a token (rather than an address)?
    #[must_use]
    pub fn asks_for_token(&self) -> bool {
        self.token_for.is_some()
    }
}

/// What a passed connection test saves, for the boundary to write: the
/// provider, the `.env` entries in order (the address, then a token typed
/// for it), and how many models the server offered. See
/// `docs/local-models.md`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerSave {
    /// The provider id.
    pub provider: String,
    /// Its label, for the confirmation.
    pub name: String,
    /// `(variable, value)` pairs to store — empty for a server its block
    /// fixes, tested with no token typed.
    pub env: Vec<(String, String)>,
    /// How many models the server lists to chat with.
    pub models: usize,
}

impl ServerSave {
    /// The toast confirming the connection: how many models there are, and
    /// where to pick one — or that there is none to pick yet.
    #[must_use]
    pub fn confirmation(&self) -> String {
        let name = &self.name;
        match self.models {
            0 => format!("Connected to {name}, but it has no models to chat with yet"),
            1 => format!("Connected to {name} — 1 model · run /model to use it"),
            n => format!("Connected to {name} — {n} models · run /model to pick one"),
        }
    }

    /// The toast in [`confirmation`](Self::confirmation)'s place when a
    /// variable exported in the environment overrides what was just saved:
    /// the process env is read before `.env`, so the session keeps using the
    /// exported address (or token) until it is unset. `overridden` names the
    /// variables the boundary found exported with another value; only those
    /// this save wrote are named, in the order it wrote them. `None` when
    /// nothing overrides the save.
    #[must_use]
    pub fn overridden_notice(&self, overridden: &[String]) -> Option<String> {
        let vars: Vec<&str> = self
            .env
            .iter()
            .map(|(var, _)| var.as_str())
            .filter(|var| overridden.iter().any(|o| o == var))
            .collect();
        let (last, rest) = vars.split_last()?;
        Some(if rest.is_empty() {
            format!("Saved — unset {last} in your environment, which overrides it")
        } else {
            format!(
                "Saved — unset {} and {last} in your environment, which override them",
                rest.join(", ")
            )
        })
    }
}

/// How a subscription's sign-in asks the user to prove who they are. The two
/// are genuinely different pages, not one page with a blank: a device flow
/// shows a **code** to type at a short URL, a browser flow shows a long
/// **link** that comes back on its own. See `docs/copilot.md` and
/// `docs/chatgpt.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SigninKind {
    /// A short code the user types at a short URL (GitHub Copilot).
    #[default]
    DeviceCode,
    /// A long authorize link the user opens; the browser redirects back to a
    /// loopback listener, so there is nothing to type (ChatGPT Codex).
    BrowserLink,
}

impl SigninKind {
    /// The row this kind takes on the sign-in method choice
    /// ([`KeyStep::SigninMethod`]): `Browser login (default)` / `Device code
    /// login (headless)`. The first kind a subscription lists is its default
    /// and says so; otherwise the device code says what it is *for*, since
    /// "headless" is the one word that tells a user on an SSH box which row
    /// is theirs, and the browser row needs no qualifier at all.
    #[must_use]
    pub fn method_label(self, default: bool) -> String {
        let (name, note) = match self {
            Self::BrowserLink => (LOGIN_BROWSER_METHOD, None),
            Self::DeviceCode => (LOGIN_DEVICE_METHOD, Some(LOGIN_HEADLESS_NOTE)),
        };
        match (default, note) {
            (true, _) => format!("{name} ({LOGIN_DEFAULT_NOTE})"),
            (false, Some(note)) => format!("{name} ({note})"),
            (false, None) => name.to_string(),
        }
    }
}

/// The method rows' wording — the two kinds by name, and the parenthesised
/// notes: the default (whichever kind is listed first) and the device code's
/// reason to exist.
const LOGIN_BROWSER_METHOD: &str = "Browser login";
const LOGIN_DEVICE_METHOD: &str = "Device code login";
const LOGIN_DEFAULT_NOTE: &str = "default";
const LOGIN_HEADLESS_NOTE: &str = "headless";

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
    /// The sign-in pages this row can open, **the first being the default**.
    /// One kind opens its page at once; more than one puts the choice to the
    /// user first ([`KeyStep::SigninMethod`]) — ChatGPT Codex offers its
    /// browser flow and, for a headless machine, a device code
    /// (`docs/chatgpt.md`).
    pub kinds: Vec<SigninKind>,
}

impl SubscriptionChoice {
    /// The page Enter opens when there is no choice to make — the first kind
    /// listed, or the device page for a row that (impossibly) lists none.
    #[must_use]
    pub fn default_kind(&self) -> SigninKind {
        self.kinds.first().copied().unwrap_or_default()
    }

    /// Does this row have more than one way in, so that Enter asks which?
    #[must_use]
    pub fn offers_choice(&self) -> bool {
        self.kinds.len() > 1
    }
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
    /// Which page this is (see [`SigninKind`]) — it decides what the
    /// instruction says and what `c` copies.
    pub kind: SigninKind,
}

impl DeviceLogin {
    /// The code, once there is one to copy.
    #[must_use]
    pub fn code(&self) -> Option<&str> {
        (!self.user_code.is_empty()).then_some(self.user_code.as_str())
    }

    /// What `c` copies: the code on a device page, the link on a browser one.
    /// A browser flow has no code at all, so copying "the code" there would
    /// be a key that does nothing on the only page that really needs it —
    /// its URL is far too long to retype.
    #[must_use]
    pub fn copy_target(&self) -> Option<&str> {
        match self.kind {
            SigninKind::DeviceCode => self.code(),
            SigninKind::BrowserLink => {
                (!self.verification_uri.is_empty()).then_some(self.verification_uri.as_str())
            }
        }
    }
}

/// The inline `/login` onboarding flow's state (`None` on [`App`] when closed).
/// Like the `/model` picker it **replaces the composer** in the bottom live
/// region; unlike it, it's a multi-step flow rooted at [`KeyStep::Method`] —
/// a subscription sign-in ([`KeyStep::Subscription`] → [`KeyStep::Device`],
/// with [`KeyStep::SigninMethod`] between them for a subscription that offers
/// two ways in), an API key ([`KeyStep::Provider`] → [`KeyStep::Key`]), or a
/// local model server ([`KeyStep::Local`] → [`KeyStep::Server`]). The chosen
/// key is persisted to `.env` by the boundary ([`Action::SaveApiKey`]); a
/// server's address only once its connection test passes
/// ([`App::finish_server_test`]). See `docs/llm.md`, `docs/copilot.md` and
/// `docs/local-models.md`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct KeyOnboarding {
    /// The API-key providers to choose from, injected at open
    /// ([`App::open_key_onboarding`]).
    pub providers: Vec<ProviderChoice>,
    /// The subscriptions to choose from, injected at the same time.
    pub subscriptions: Vec<SubscriptionChoice>,
    /// The local model servers to choose from, injected at the same time.
    pub locals: Vec<LocalChoice>,
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
    /// The subscription whose sign-in method is being chosen (an index into
    /// `subscriptions`), set when [`KeyStep::SigninMethod`] opens and kept
    /// through the page it opens — so Esc from that page returns to the
    /// choice rather than the list. `None` for a subscription that offered
    /// one way in and never asked.
    pub chosen_subscription: Option<usize>,
    /// The local server whose page is open (an index into `locals`), pinned
    /// like [`chosen`](Self::chosen) so a filter cannot reorder it away.
    pub chosen_local: Option<usize>,
    /// The API key being typed / pasted (the key step). Rendered masked.
    pub key_input: String,
    /// The open device-code page ([`KeyStep::Device`]); `None` otherwise.
    pub device: Option<DeviceLogin>,
    /// The open local server page ([`KeyStep::Server`]); `None` otherwise.
    pub server: Option<ServerPage>,
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

    /// Providers whose id, name or description contains the query
    /// (case-insensitive substring; every provider for an empty query),
    /// keeping the injected order. The description is matched for the same
    /// reason the subscription list matches its own: the row shows a name,
    /// and a user hunting for `claude` or `local` is describing what they
    /// want rather than naming it.
    #[must_use]
    pub fn matches(&self) -> Vec<&ProviderChoice> {
        let query = self.query.to_lowercase();
        self.providers
            .iter()
            .filter(|p| {
                query.is_empty()
                    || p.id.to_lowercase().contains(&query)
                    || p.name.to_lowercase().contains(&query)
                    || p.description.to_lowercase().contains(&query)
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

    /// Local servers whose id, name or description contains the query
    /// (case-insensitive substring; every one for an empty query), keeping
    /// the injected order — so `ollama`, `studio` and `local` all find
    /// theirs.
    #[must_use]
    pub fn local_matches(&self) -> Vec<&LocalChoice> {
        let query = self.query.to_lowercase();
        self.locals
            .iter()
            .filter(|l| {
                query.is_empty()
                    || l.id.to_lowercase().contains(&query)
                    || l.name.to_lowercase().contains(&query)
                    || l.description.to_lowercase().contains(&query)
            })
            .collect()
    }

    /// The highlighted local server (the local list).
    #[must_use]
    pub fn highlighted_local(&self) -> Option<&LocalChoice> {
        self.local_matches().get(self.selected).copied()
    }

    /// The local server whose page is open, if any.
    #[must_use]
    pub fn chosen_local(&self) -> Option<&LocalChoice> {
        self.chosen_local.and_then(|i| self.locals.get(i))
    }

    /// Does the server page's field take typing right now? Not while its test
    /// runs, and not for an address the provider block fixes — the page is
    /// then a wait or a button, and the caret hides.
    #[must_use]
    pub fn server_takes_typing(&self) -> bool {
        let Some(page) = &self.server else {
            return false;
        };
        page.testing.is_none()
            && (page.asks_for_token() || self.chosen_local().is_some_and(|c| c.host_env.is_some()))
    }

    /// The subscription whose sign-in method is being chosen (the method
    /// step, and the page it opened), if any.
    #[must_use]
    pub fn chosen_subscription(&self) -> Option<&SubscriptionChoice> {
        self.chosen_subscription
            .and_then(|i| self.subscriptions.get(i))
    }

    /// The ways in the chosen subscription offers, in row order — the method
    /// step's rows. Empty when no subscription is chosen.
    #[must_use]
    pub fn signin_kinds(&self) -> &[SigninKind] {
        self.chosen_subscription()
            .map_or(&[], |s| s.kinds.as_slice())
    }

    /// The highlighted way in (the method step).
    #[must_use]
    pub fn highlighted_signin_kind(&self) -> Option<SigninKind> {
        self.signin_kinds().get(self.selected).copied()
    }

    /// The method step's row labels, in order — the first marked as the
    /// default ([`SigninKind::method_label`]).
    #[must_use]
    pub fn signin_method_labels(&self) -> Vec<String> {
        self.signin_kinds()
            .iter()
            .enumerate()
            .map(|(i, kind)| kind.method_label(i == 0))
            .collect()
    }

    /// How many rows the current list step offers — the wrap/clamp bound the
    /// key handling shares across the lists.
    fn list_len(&self) -> usize {
        match self.step {
            KeyStep::Method => self.method_matches().len(),
            KeyStep::Subscription => self.subscription_matches().len(),
            KeyStep::SigninMethod => self.signin_kinds().len(),
            KeyStep::Provider => self.matches().len(),
            KeyStep::Local => self.local_matches().len(),
            KeyStep::Device | KeyStep::Key | KeyStep::Server => 0,
        }
    }

    /// Open the sign-in page for `choice` with `kind`'s flow, and tell the
    /// boundary to run it. The row's name titles the page; the kind decides
    /// its wording and which flow the worker runs (`docs/chatgpt.md`).
    fn open_signin_page(&mut self, choice: &SubscriptionChoice, kind: SigninKind) -> Action {
        self.go(KeyStep::Device);
        self.device = Some(DeviceLogin {
            provider_id: choice.id.clone(),
            provider_name: choice.name.clone(),
            kind,
            ..DeviceLogin::default()
        });
        Action::StartDeviceLogin {
            provider: choice.id.clone(),
            kind,
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
    /// Open the inline `/login` onboarding flow with the given API-key
    /// provider, subscription and local server choices (built at the boundary
    /// so the `configured` ✓ reflects real key resolution) and the
    /// `~`-relative `.env` path the provider-step hint names. Starts on the
    /// method step; abandons any band / palette / file picker / model picker
    /// it shares the composer with, staying in [`View::Conversation`]. See
    /// `docs/llm.md`.
    pub fn open_key_onboarding(
        &mut self,
        providers: Vec<ProviderChoice>,
        subscriptions: Vec<SubscriptionChoice>,
        locals: Vec<LocalChoice>,
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
            locals,
            env_path: env_path.into(),
            ..KeyOnboarding::default()
        });
    }

    /// Is a local server's connection test in flight? Ctrl+C then cancels it
    /// with the flow, and the boundary feeds only an answer still awaited.
    #[must_use]
    pub fn server_test_active(&self) -> bool {
        self.key_onboarding
            .as_ref()
            .and_then(|o| o.server.as_ref())
            .is_some_and(|page| page.testing.is_some())
    }

    /// The boundary's answer to a connection test ([`Action::TestLocalServer`],
    /// `docs/local-models.md`). Applied only when it answers the test still in
    /// flight — one the user stopped, or moved on from, is dropped:
    ///
    /// - **passed** closes the flow and hands back what to save — the address
    ///   (unless the provider block fixes it) and the token typed for it;
    /// - **unauthorized** asks for a token for the same address — or, when
    ///   a token was just tried, says it was refused;
    /// - **failed** keeps the page, the reason under the field.
    pub fn finish_server_test(
        &mut self,
        test: &ServerTest,
        verdict: Verdict,
    ) -> Option<ServerSave> {
        let onboarding = self.key_onboarding.as_mut()?;
        let choice = onboarding.chosen_local()?.clone();
        let page = onboarding.server.as_mut()?;
        if page.testing.as_ref() != Some(test) {
            return None;
        }
        page.testing = None;
        match verdict {
            Verdict::Passed(models) => {
                let mut env = Vec::new();
                if let Some(var) = &choice.host_env {
                    env.push((var.clone(), test.host.clone()));
                }
                if let Some(token) = &test.token {
                    env.push((choice.token_env.clone(), token.clone()));
                }
                self.close_key_onboarding();
                Some(ServerSave {
                    provider: choice.id,
                    name: choice.name,
                    env,
                    models,
                })
            }
            // The token just tried was the wrong one: say so where it was
            // typed, and keep it there to fix.
            Verdict::Unauthorized(_) if test.token.is_some() => {
                page.failure = Some(format!(
                    "{} refused that token — check it, then test again.",
                    choice.name
                ));
                None
            }
            Verdict::Unauthorized(reason) => {
                page.token_for = Some(TokenAsk {
                    host: test.host.clone(),
                    reason,
                });
                page.token.clear();
                None
            }
            Verdict::Failed(reason) => {
                page.failure = Some(reason);
                None
            }
        }
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
    /// - **Sign-in method** (a subscription offering two ways in): ↑/↓ move
    ///   wrapping, Home/End jump, `Enter` opens the highlighted way's page,
    ///   `Esc` steps back to the subscription list, `Ctrl+C` closes. No
    ///   filter — it is a question with two answers — so typing does nothing.
    /// - **Device** (the sign-in page): `c` copies the code — or, on a
    ///   browser flow, the link — once there is one, `Esc` cancels the
    ///   sign-in back to where it was opened from (the method choice, else
    ///   the subscription list), `Ctrl+C` closes.
    /// - **Key** (masked entry — or a host shown plain, [`KeyKind`]):
    ///   printable keys and Backspace edit the field, `Enter` saves a
    ///   non-empty value ([`Action::SaveApiKey`]) — or a host field's default
    ///   when nothing was typed — and closes, `Esc` steps *back* to the
    ///   provider list, `Ctrl+C` closes.
    ///
    /// Owns **every** key while open (routed at the top of [`on_key`]).
    ///
    /// [`on_key`]: App::on_key
    pub(super) fn on_key_key_onboarding(&mut self, key: KeyEvent) -> Action {
        // Ctrl+C closes the whole flow (like the /model picker), from any step
        // — and stops whatever worker the page it was on had running.
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            let signing_in = self.device_login_active();
            let testing = self.server_test_active();
            self.close_key_onboarding();
            return if signing_in {
                Action::CancelDeviceLogin
            } else if testing {
                Action::CancelServerTest
            } else {
                Action::CloseKeyOnboarding
            };
        }
        let Some(step) = self.key_onboarding.as_ref().map(|o| o.step) else {
            return Action::None;
        };
        match step {
            KeyStep::Method | KeyStep::Subscription | KeyStep::Provider | KeyStep::Local => {
                self.on_key_login_list(key)
            }
            KeyStep::SigninMethod => self.on_key_signin_method(key),
            KeyStep::Device => self.on_key_device(key),
            KeyStep::Key => self.on_key_login_key(key),
            KeyStep::Server => self.on_key_server(key),
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
                    Some(LoginMethod::Local) => onboarding.go(KeyStep::Local),
                    None => {}
                }
                Action::None
            }
            KeyStep::Subscription => {
                let Some(choice) = onboarding.highlighted_subscription().cloned() else {
                    return Action::None;
                };
                // A row with two ways in asks which first; the index is
                // pinned in the *unfiltered* list, the provider step's rule.
                if choice.offers_choice() {
                    let idx = onboarding
                        .subscriptions
                        .iter()
                        .position(|s| s.id == choice.id);
                    onboarding.go(KeyStep::SigninMethod);
                    onboarding.chosen_subscription = idx;
                    return Action::None;
                }
                onboarding.chosen_subscription = None;
                let kind = choice.default_kind();
                onboarding.open_signin_page(&choice, kind)
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
            KeyStep::Local => {
                // Pinned in the unfiltered list, the provider step's rule.
                let chosen = onboarding
                    .highlighted_local()
                    .map(|l| l.id.clone())
                    .and_then(|id| onboarding.locals.iter().position(|l| l.id == id));
                if let Some(idx) = chosen {
                    onboarding.go(KeyStep::Server);
                    onboarding.chosen_local = Some(idx);
                    onboarding.server = Some(ServerPage::default());
                }
                Action::None
            }
            KeyStep::SigninMethod | KeyStep::Device | KeyStep::Key | KeyStep::Server => {
                Action::None
            }
        }
    }

    /// Keys on the sign-in method choice: ↑/↓ (wrapping) and Home/End move,
    /// `Enter` opens the highlighted way's page, `Esc` steps back to the
    /// subscription list. Everything else is swallowed — two rows are a
    /// question, not a list to search, so there is no filter to type into.
    fn on_key_signin_method(&mut self, key: KeyEvent) -> Action {
        let Some(onboarding) = self.key_onboarding.as_mut() else {
            return Action::None;
        };
        let len = onboarding.list_len();
        match key.code {
            KeyCode::Up => onboarding.selected = wrap_step(onboarding.selected, len, -1),
            KeyCode::Down => onboarding.selected = wrap_step(onboarding.selected, len, 1),
            KeyCode::Home => onboarding.selected = 0,
            KeyCode::End => onboarding.selected = len.saturating_sub(1),
            KeyCode::Enter => {
                let Some(kind) = onboarding.highlighted_signin_kind() else {
                    return Action::None;
                };
                let Some(choice) = onboarding.chosen_subscription().cloned() else {
                    return Action::None;
                };
                return onboarding.open_signin_page(&choice, kind);
            }
            KeyCode::Esc => {
                onboarding.chosen_subscription = None;
                onboarding.go(KeyStep::Subscription);
            }
            _ => {}
        }
        Action::None
    }

    /// Keys on the device-code page: `c` copies the code, `Esc` cancels the
    /// sign-in back to where it was opened from — the method choice, with the
    /// way just tried still highlighted (codex's own browser page says "on a
    /// headless machine, press Esc and choose the device code", so the choice
    /// must be one Esc away), else the subscription list. Everything else is
    /// swallowed — the page is a wait, not a field.
    fn on_key_device(&mut self, key: KeyEvent) -> Action {
        let Some(onboarding) = self.key_onboarding.as_mut() else {
            return Action::None;
        };
        match key.code {
            KeyCode::Char('c' | 'C') => onboarding
                .device
                .as_ref()
                .and_then(DeviceLogin::copy_target)
                .map_or(Action::None, |target| {
                    Action::CopyDeviceCode(target.to_string())
                }),
            KeyCode::Esc => {
                let tried = onboarding.device.take().map(|d| d.kind);
                if onboarding.chosen_subscription().is_some() {
                    onboarding.go(KeyStep::SigninMethod);
                    onboarding.selected = tried
                        .and_then(|kind| onboarding.signin_kinds().iter().position(|k| *k == kind))
                        .unwrap_or(0);
                } else {
                    onboarding.go(KeyStep::Subscription);
                }
                Action::CancelDeviceLogin
            }
            _ => Action::None,
        }
    }

    /// Keys on the (masked) key-entry step.
    fn on_key_login_key(&mut self, key: KeyEvent) -> Action {
        let Some(onboarding) = self.key_onboarding.as_mut() else {
            return Action::None;
        };
        match key.code {
            KeyCode::Enter => {
                let Some(choice) = onboarding.chosen_provider() else {
                    return Action::None;
                };
                // An empty key is nothing to save.
                let typed = onboarding.key_input.trim();
                if typed.is_empty() {
                    return Action::None;
                }
                let action = Action::SaveApiKey {
                    provider: choice.id.clone(),
                    env_var: choice.env_var.clone(),
                    key: typed.to_string(),
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

    /// Keys on a local server's page ([`KeyStep::Server`],
    /// `docs/local-models.md`):
    ///
    /// - **Testing**: the page waits — `Esc` stops the test
    ///   ([`Action::CancelServerTest`]) and keeps the page; everything else
    ///   is swallowed.
    /// - **The address**: printable keys and Backspace edit it (unless the
    ///   provider block fixes the address), clearing a failure they no longer
    ///   describe; `Enter` tests what was typed — or, typed nothing, the
    ///   address in force — ([`Action::TestLocalServer`]); `Esc` steps back
    ///   to the server list with that row still highlighted.
    /// - **The token** (the server asked for one): the same editing, masked;
    ///   `Enter` tests a typed token at the address the server asked from;
    ///   `Esc` returns to the address.
    fn on_key_server(&mut self, key: KeyEvent) -> Action {
        let Some(onboarding) = self.key_onboarding.as_mut() else {
            return Action::None;
        };
        let Some(choice) = onboarding.chosen_local().cloned() else {
            return Action::None;
        };
        let Some(page) = onboarding.server.as_mut() else {
            return Action::None;
        };
        if page.testing.is_some() {
            if key.code == KeyCode::Esc {
                page.testing = None;
                return Action::CancelServerTest;
            }
            return Action::None;
        }
        let typing = !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
        let editable = choice.host_env.is_some();
        let test = match (page.token_for.clone(), key.code) {
            (Some(ask), KeyCode::Enter) => {
                let token = page.token.trim();
                if token.is_empty() {
                    return Action::None;
                }
                ServerTest {
                    host: ask.host,
                    token: Some(token.to_string()),
                }
            }
            (None, KeyCode::Enter) => {
                let typed = page.address.trim();
                ServerTest {
                    host: if typed.is_empty() {
                        choice.host.clone()
                    } else {
                        typed.to_string()
                    },
                    token: None,
                }
            }
            (Some(_), KeyCode::Esc) => {
                page.token_for = None;
                page.token.clear();
                page.failure = None;
                return Action::None;
            }
            (None, KeyCode::Esc) => {
                // Back to the list, the row just left still highlighted.
                let left = onboarding.chosen_local.take();
                onboarding.server = None;
                onboarding.go(KeyStep::Local);
                onboarding.selected = left.unwrap_or(0);
                return Action::None;
            }
            (token_for, KeyCode::Backspace) => {
                let field = match token_for {
                    Some(_) => &mut page.token,
                    None if editable => &mut page.address,
                    None => return Action::None,
                };
                field.pop();
                page.failure = None;
                return Action::None;
            }
            (token_for, KeyCode::Char(c)) if typing => {
                let field = match token_for {
                    Some(_) => &mut page.token,
                    None if editable => &mut page.address,
                    None => return Action::None,
                };
                field.push(c);
                page.failure = None;
                return Action::None;
            }
            _ => return Action::None,
        };
        page.failure = None;
        page.testing = Some(test.clone());
        Action::TestLocalServer {
            provider: choice.id,
            test,
        }
    }

    /// A bracketed paste while the `/login` flow is open. On the key step the
    /// pasted text is the API key — interior whitespace and control characters
    /// (a trailing newline from the paste, say) are dropped and the rest
    /// appended; a server page's address or token field takes it the same
    /// way, unless its test is running or its address is fixed. On a list
    /// step it extends the filter query like a [`paste_into_resume_search`],
    /// whitespace collapsed; the device page and the sign-in method choice
    /// have no field, so a paste there is ignored.
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
            KeyStep::Server => {
                let editable = onboarding
                    .chosen_local()
                    .is_some_and(|choice| choice.host_env.is_some());
                let Some(page) = onboarding.server.as_mut() else {
                    return;
                };
                if page.testing.is_some() {
                    return;
                }
                let cleaned: String = pasted
                    .chars()
                    .filter(|c| !c.is_whitespace() && !c.is_control())
                    .collect();
                let field = if page.asks_for_token() {
                    &mut page.token
                } else if editable {
                    &mut page.address
                } else {
                    return;
                };
                field.push_str(&cleaned);
                page.failure = None;
            }
            KeyStep::SigninMethod | KeyStep::Device => {}
            KeyStep::Method | KeyStep::Subscription | KeyStep::Provider | KeyStep::Local => {
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
