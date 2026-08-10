//! The reply backend and everything that selects it (`docs/llm.md`).
//!
//! [`ModelSession`] owns the answer to "who replies, and how": the provider
//! table, the resolved key, the active model, its reasoning mode
//! (`docs/reasoning.md`), its image support and context window
//! (`docs/tools.md`, `docs/compact.md`) — plus the `Box<dyn ReplySource>` all
//! of that resolves to. It exists because those knobs move **together**:
//! `/model`, `/login`, Shift+Tab and the startup capability probe each rebuild
//! the backend from the same set, and a rebuild that forgot part of it used to
//! silently drop the `agent` tool and stop asking permission for the rest of
//! the session.
//!
//! Two rules hold here:
//!
//! - **Every rebuild goes through [`ModelSession::rebuild`]**, which re-attaches
//!   the whole shared set (background registry, subagent registry, permission
//!   gate) — the reason those handles are *owned* here as clones rather than
//!   passed in per call.
//! - **The environment wins, and never sticks.** An `ALTER_ZERO_PROVIDER` /
//!   `ALTER_ZERO_MODEL` selection is used but never written back to
//!   `config.json`, so persisting its reasoning state can't hijack the saved
//!   default (see [`ModelSession::persist`]).
//!
//! The dummy backend is the fallback throughout, so the app always runs
//! offline; a real model activates only when a provider, a model and a key all
//! resolve.

use std::path::Path;
use std::time::Duration;

use alter_zero::agents::AgentRegistry;
use alter_zero::app::{ProviderChoice, ToastKind};
use alter_zero::background::BackgroundRegistry;
use alter_zero::llm::{
    self, EnvFile, LlmBackend, ModelConfig, ModelEntry, ProvidersFile, ReasoningSupport,
    ThinkingMode,
};
use alter_zero::permission::PermissionGate;
use alter_zero::settings::SessionSettings;
use alter_zero::stream::{self, CancelToken, DummyAi, ReplySource};
use alter_zero::ui;

use super::workers::{ModelFetch, spawn_model_fetch};
use super::{Session, config};

/// A model's capabilities as the loop tracks them: its reasoning support (and
/// the mode the Shift+Tab cycle starts at), whether it can see images, and how
/// big its context window is. `None` anywhere means "unknown" — the startup
/// probe finds out, and until it does the loop stays optimistic.
type Thinking = (ReasoningSupport, ThinkingMode);

/// The reply backend plus the configuration that chose it.
pub(crate) struct ModelSession {
    /// The provider table (`providers.toml` or the built-in default).
    providers: ProvidersFile,
    /// The `.env` key store the `/login` flow writes, and its path. Keys live
    /// in this map rather than the process env — `set_var` is `unsafe`, which
    /// this crate forbids.
    env_file: EnvFile,
    env_file_path: std::path::PathBuf,
    /// The `~`-relative `.env` path shown in the `/login` provider-step hint,
    /// so it names the real file even under an `ALTER_ZERO_ENV_FILE` override.
    env_path_display: String,
    /// `config.json`'s path; `None` disables persistence (no config home).
    settings_path: Option<std::path::PathBuf>,
    /// The (provider, model) pair `config.json` currently records. Thinking /
    /// vision / context writes attach to **this** pair only.
    persisted_selection: Option<(String, String)>,
    /// The sampling temperature riding every request — seeded from
    /// `ALTER_ZERO_TEMPERATURE`, then owned by the `/settings` **Temperature**
    /// knob (`docs/settings.md`).
    temperature: Option<f32>,
    /// Whether the `bash`/`read`/`write`/`edit`/`agent` tools are offered —
    /// seeded from `ALTER_ZERO_TOOLS`, then owned by the `/settings` **Tools**
    /// knob. Every rebuild reads it, so a switch mid-session sticks.
    tools: bool,
    /// The retry budget every round carries — the `/settings` **Error retry**
    /// knob (`docs/settings.md`).
    max_retries: u32,
    /// The tool-round ceiling every turn carries — the `/settings` **Max tool
    /// calls** knob, `0` (the default) being no limit at all.
    max_tool_calls: usize,
    /// The persona + environment system prompt every rebuild inherits.
    system_prompt: Option<String>,
    /// `ALTER_ZERO_STALL_MS` — the test-only wedged backend (`docs/interrupt.md`).
    stall_ms: Option<u64>,
    /// `ALTER_ZERO_CONTEXT_WINDOW`, which outranks whatever a provider reports.
    env_context_window: Option<u64>,
    /// The live selection: which provider, which model, what it can do.
    active_provider: Option<String>,
    active_model: String,
    active_vision: Option<bool>,
    active_context: Option<u64>,
    /// The thinking mode the live backend was built with, so a rebuild the
    /// user didn't ask for (a `/settings` knob) carries it forward instead of
    /// silently dropping the Shift+Tab choice (`docs/reasoning.md`).
    active_thinking: Option<ThinkingMode>,
    /// The backend itself — the dummy unless a real provider/model/key resolved.
    backend: Box<dyn ReplySource>,
    /// Whether [`Self::backend`] is a **real** model rather than the dummy (or
    /// the stalled test backend).
    ///
    /// This has to be tracked, not re-derived: `ModelConfig::is_usable` only
    /// proves that a *key* resolved, so a configured provider with no model
    /// selected still yields a "usable" config — for `active_model`, which is
    /// then whatever the dummy answers as. Deriving a one-off backend from that
    /// sent `POST /chat/completions` for `dummy_model_name` and failed the turn
    /// (`Self::compact_backend`).
    real_backend: bool,
    /// The shared attachment set every rebuild must re-attach (see the module
    /// doc). Cheap `Arc` clones of the loop's own registries.
    registry: BackgroundRegistry,
    agents: AgentRegistry,
    permissions: Option<PermissionGate>,
    ask: alter_zero::ask::AskGate,
    /// The shared task list (docs/task-tools.md) — enables the four task
    /// tools on every rebuild.
    tasks: alter_zero::tasks::TaskRegistry,
    /// The user's lifecycle hooks (`docs/hooks.md`), re-attached on every
    /// rebuild like the registries above. Held as the *setup* rather than a
    /// built sink because a payload names the model, and a `/model` switch
    /// must not leave a stale name on the wire — `session_backend` builds the
    /// sink from the config it is handed.
    hooks: Option<HookSetup>,
    /// The `/model` picker's in-flight fetch, so closing the picker cancels it.
    fetch_cancel: Option<CancelToken>,
    /// Whether the startup capability probe is still outstanding — the gate on
    /// its `select!` branch. Cleared by the first result, or by a `/model`
    /// switch that learned the same facts first-hand.
    probe_pending: bool,
    /// The thinking state [`Self::resolve`] recovered from `config.json`, handed
    /// to `App` once at bootstrap.
    thinking_seed: Option<Thinking>,
    /// The capability probe bootstrap should spawn: the active provider's **id**
    /// (the `select!` arm matches it against `active_provider`) and the config to
    /// fetch with. Taken once.
    probe: Option<(String, Option<ModelConfig>)>,
}

impl ModelSession {
    /// Resolve the whole model configuration from the environment, the saved
    /// selection and the provider table, and build the backend it names. The
    /// precedence is dotenv's: a real env var beats `config.json`, which beats
    /// the provider file's default.
    #[allow(clippy::too_many_arguments)] // the loop's full shared-attachment set
    pub(crate) fn resolve(
        cwd: &Path,
        home: Option<&Path>,
        registry: &BackgroundRegistry,
        agents: &AgentRegistry,
        permissions: Option<&PermissionGate>,
        ask: &alter_zero::ask::AskGate,
        tasks: &alter_zero::tasks::TaskRegistry,
        settings: &SessionSettings,
        hooks: Option<HookSetup>,
    ) -> Self {
        let providers = config::load_providers();
        // The persistent API-key store: `.env` in the config home (or
        // `ALTER_ZERO_ENV_FILE`), written by the `/login` flow and consulted
        // during key resolution (a real process env var still wins).
        let env_file_path = config::env_file_path();
        let env_file = config::load_env_file(&env_file_path);
        // The persisted `/model` selection (`~/.alter-zero/config.json`): the
        // provider + model chosen last run, so it survives a restart.
        let settings_path = config::settings_file_path();
        let saved = config::load_settings(settings_path.as_deref());
        // The `/settings` knobs the backend is built around — already merged
        // with their `ALTER_ZERO_*` overrides by the caller
        // (`config::apply_setting_overrides`, docs/settings.md).
        let temperature = settings.temperature;
        let tools = settings.tools;
        let max_retries = settings.error_retry;
        let max_tool_calls = settings.max_tool_calls;
        let system_prompt = config::system_prompt(cwd);
        // The provider the /model picker lists from and switches within: env,
        // else the saved selection, else the file's default.
        let active_provider = std::env::var("ALTER_ZERO_PROVIDER")
            .ok()
            .filter(|s| !s.is_empty())
            .or_else(|| saved.provider.clone())
            .or_else(|| providers.default_provider());
        // The saved provider/model are ONE selection: pairing the saved model
        // with a *different* (env-overridden) provider would ask that provider
        // for a model it may not serve, so the saved model applies only when
        // the resolved provider is the one it was saved with.
        let saved_model = saved
            .model
            .clone()
            .filter(|_| active_provider == saved.provider);
        let env_model = std::env::var("ALTER_ZERO_MODEL")
            .ok()
            .filter(|s| !s.is_empty())
            .or(saved_model);
        let stall_ms = config::stall_ms();
        // The saved thinking blob describes the saved (provider, model)
        // pairing — like saved_model it applies only when that exact selection
        // resolved. Outer None = support unknown (the probe below finds out);
        // Some(None) = known non-reasoner; Some(Some(state)) = seed the
        // Shift+Tab cycle. See docs/reasoning.md.
        let selection_is_saved =
            active_provider == saved.provider && env_model.is_some() && env_model == saved.model;
        let saved_thinking: Option<Option<Thinking>> = saved
            .thinking
            .as_ref()
            .filter(|_| selection_is_saved)
            .map(llm::ThinkingSettings::to_seed);
        let startup_thinking = saved_thinking.clone().flatten();
        // The saved model's image-input support and context window — like
        // saved_thinking they apply only to that exact selection; `None` =
        // unknown (the probe finds out).
        let saved_vision: Option<bool> = saved.vision.filter(|_| selection_is_saved);
        let saved_context: Option<u64> = saved.context.filter(|_| selection_is_saved);

        // A real model activates only when a provider, a model and a key all
        // resolve — and `ALTER_ZERO_DUMMY` isn't forcing the dummy. Anything else
        // falls back to the dummy, so the app always runs offline.
        let real_config = (!config::dummy_forced())
            .then(|| active_provider.as_deref().zip(env_model.as_deref()))
            .flatten()
            .and_then(|(provider, model)| {
                config::model_config_for(
                    &providers,
                    &env_file,
                    provider,
                    model,
                    temperature,
                    startup_thinking.as_ref().map(|(_, mode)| *mode),
                    saved_vision,
                )
            })
            .filter(ModelConfig::is_usable);
        // Whether the *real* backend activated (vs the dummy fallback) — the
        // capability state and its probe only make sense against a live provider.
        // `ALTER_ZERO_STALL_MS` preempts both with the test-only wedged backend.
        let real_backend = stall_ms.is_none() && real_config.is_some();
        let backend: Box<dyn ReplySource> = match (stall_ms, real_config) {
            (Some(ms), _) => Box::new(stream::StallAi::new(Duration::from_millis(ms))),
            (None, Some(cfg)) => Box::new(session_backend(
                cfg,
                system_prompt.clone(),
                tools,
                max_retries,
                max_tool_calls,
                registry,
                agents,
                permissions,
                ask,
                tasks,
                hooks.as_ref(),
            )),
            (None, None) => {
                let dummy =
                    DummyAi::with_startup_delay(config::startup_delay()).with_ask(ask.clone());
                Box::new(match permissions {
                    Some(gate) => dummy.with_permissions(gate.clone()),
                    None => dummy,
                })
            }
        };
        // The active model tracks what the backend actually answers as, so a
        // dummy fallback shows `dummy_model_name`.
        let active_model = backend.model_name();
        // The capability probe (docs/reasoning.md, docs/tools.md): when the
        // real backend is active but the saved settings don't say whether its
        // model reasons or sees images (an env-selected model, or the first run
        // since the upgrade that added the blob), fetch the provider's
        // `/models` once in the background and read the record out of it.
        let probe = (real_backend && (saved_thinking.is_none() || saved_vision.is_none()))
            .then(|| active_provider.as_deref().zip(env_model.as_deref()))
            .flatten()
            .map(|(p, m)| {
                let cfg =
                    config::model_config_for(&providers, &env_file, p, m, temperature, None, None);
                (p.to_string(), cfg)
            });
        Self {
            providers,
            env_path_display: ui::display_cwd(&env_file_path, home),
            env_file,
            env_file_path,
            settings_path,
            persisted_selection: saved.provider.clone().zip(saved.model.clone()),
            temperature,
            tools,
            max_retries,
            max_tool_calls,
            system_prompt,
            stall_ms,
            env_context_window: config::context_window_override(),
            active_provider,
            active_model,
            active_vision: real_backend.then_some(saved_vision).flatten(),
            active_context: real_backend.then_some(saved_context).flatten(),
            active_thinking: startup_thinking.as_ref().map(|(_, mode)| *mode),
            backend,
            real_backend,
            registry: registry.clone(),
            agents: agents.clone(),
            permissions: permissions.cloned(),
            ask: ask.clone(),
            tasks: tasks.clone(),
            hooks,
            fetch_cancel: None,
            probe_pending: probe.is_some(),
            thinking_seed: real_backend.then_some(startup_thinking).flatten(),
            probe,
        }
    }

    // ----- what the loop reads -----

    /// The backend a turn spawns on.
    pub(crate) fn backend(&self) -> &dyn ReplySource {
        self.backend.as_ref()
    }

    /// The model id the footer shows (`docs/footer.md`).
    pub(crate) fn model_name(&self) -> String {
        self.backend.model_name()
    }

    /// The active model's id as the *selection* names it — what a rebuild and
    /// the `/model` picker's "current" marker key off.
    pub(crate) fn active_model(&self) -> &str {
        &self.active_model
    }

    /// The footer gauge + auto-compact window: the env override wins, else the
    /// active model's known window (`docs/compact.md`).
    pub(crate) fn context_window(&self) -> Option<u64> {
        self.env_context_window.or(self.active_context)
    }

    /// Whether the active model is known **not** to accept images — the Ctrl+V
    /// paste warning's gate (`docs/tools.md`).
    pub(crate) fn is_blind(&self) -> bool {
        self.active_vision == Some(false)
    }

    /// Is this the test-only stalled backend? It ignores scripted replies, so
    /// `/compact` and auto-compaction opt out (`docs/interrupt.md`).
    pub(crate) fn is_stalled_test_backend(&self) -> bool {
        self.stall_ms.is_some()
    }

    /// Is the startup capability probe still outstanding? Gates its `select!`
    /// branch, so a late result can't undo a `/model` switch that raced it.
    pub(crate) fn probe_pending(&self) -> bool {
        self.probe_pending
    }

    /// The `~`-relative `.env` path the `/login` hint names.
    pub(crate) fn env_path_display(&self) -> &str {
        &self.env_path_display
    }

    /// The environment variable a provider's key is read from — the "run
    /// /login to set X" hint.
    pub(crate) fn key_env(&self, provider: &str) -> String {
        config::key_env_name(&self.providers, provider)
    }

    /// The provider rows the `/login` flow shows, each tagged with whether a
    /// key already resolves.
    pub(crate) fn provider_choices(&self) -> Vec<ProviderChoice> {
        config::provider_choices(&self.providers, &self.env_file)
    }

    /// The active provider id, for the `/model` picker's "switch within this
    /// provider" default.
    pub(crate) fn active_provider(&self) -> Option<&str> {
        self.active_provider.as_deref()
    }

    /// The thinking state recovered from `config.json`, handed to `App` once.
    pub(crate) fn take_thinking_seed(&mut self) -> Option<Thinking> {
        self.thinking_seed.take()
    }

    /// The capability probe to spawn at bootstrap, if one is due.
    pub(crate) fn take_probe(&mut self) -> Option<(String, Option<ModelConfig>)> {
        self.probe.take()
    }

    // ----- rebuilding -----

    /// The resolved config for a provider/model with the given capabilities,
    /// or `None` when the provider isn't in the table.
    fn config_for(
        &self,
        provider: &str,
        model: &str,
        thinking: Option<ThinkingMode>,
        vision: Option<bool>,
    ) -> Option<ModelConfig> {
        config::model_config_for(
            &self.providers,
            &self.env_file,
            provider,
            model,
            self.temperature,
            thinking,
            vision,
        )
    }

    /// Rebuild the backend for `cfg` with the **full** shared attachment set —
    /// the background registry, the subagent registry (enabling the `agent`
    /// tool) and the permission gate. Every (re)build in the session goes
    /// through here: the initial pick and each rebind (a `/model` switch, a
    /// Shift+Tab thinking change, the capability probe) alike.
    fn rebuild(&mut self, cfg: ModelConfig) {
        self.backend = Box::new(session_backend(
            cfg,
            self.system_prompt.clone(),
            self.tools,
            self.max_retries,
            self.max_tool_calls,
            &self.registry,
            &self.agents,
            self.permissions.as_ref(),
            &self.ask,
            &self.tasks,
            self.hooks.as_ref(),
        ));
    }

    /// Rebuild the *current* selection's backend — what every `/settings` knob
    /// that changes the request's shape needs. A no-op on the dummy (there is
    /// no request to reshape) and on a selection whose config isn't usable.
    /// See `docs/settings.md`.
    fn rebuild_current(&mut self) {
        if !self.real_backend {
            return;
        }
        if let Some(provider) = self.active_provider.clone()
            && let Some(cfg) = self
                .config_for(
                    &provider,
                    &self.active_model.clone(),
                    self.active_thinking,
                    self.active_vision,
                )
                .filter(ModelConfig::is_usable)
        {
            self.rebuild(cfg);
        }
    }

    /// The `/settings` **Tools** knob: offer (or withhold) the tool set from
    /// here on. See `docs/settings.md`.
    pub(crate) fn set_tools(&mut self, tools: bool) {
        self.tools = tools;
        self.rebuild_current();
    }

    /// Whether a `hooks.json` resolved with anything runnable in it — the
    /// `/settings` **Hooks** row's availability (`docs/hooks.md`).
    pub(crate) const fn hooks_available(&self) -> bool {
        self.hooks.is_some()
    }

    /// The `/settings` **Hooks** knob: run the user's lifecycle hooks, or
    /// don't. The backend is rebuilt so the next turn genuinely stops (or
    /// starts) consulting them — there is no second copy of the flag to drift.
    pub(crate) fn set_hooks(&mut self, enabled: bool) {
        if let Some(setup) = self.hooks.as_mut() {
            setup.enabled = enabled;
        }
        self.rebuild_current();
    }

    /// The `/settings` **Error retry** knob: how many times a failed request
    /// is retried from here on.
    pub(crate) fn set_max_retries(&mut self, max_retries: u32) {
        self.max_retries = max_retries;
        self.rebuild_current();
    }

    /// The `/settings` **Temperature** knob: `None` sends no `temperature` at
    /// all and leaves it to the provider.
    pub(crate) fn set_temperature(&mut self, temperature: Option<f32>) {
        self.temperature = temperature;
        self.rebuild_current();
    }

    /// The `/settings` **Max tool calls** knob: how many tool rounds a turn may
    /// run, `0` being no limit.
    pub(crate) fn set_max_tool_calls(&mut self, max_tool_calls: usize) {
        self.max_tool_calls = max_tool_calls;
        self.rebuild_current();
    }

    /// Switch to a `/model` picker row: rebuild the backend for the chosen
    /// provider/model and adopt its capabilities, persisting the choice so it
    /// is the default next run. `false` when the config isn't usable (no key) —
    /// the current backend then stands, and the caller says so.
    pub(crate) fn switch_to(
        &mut self,
        provider: &str,
        id: &str,
        thinking: Option<&Thinking>,
        vision: Option<bool>,
        context: Option<u64>,
    ) -> bool {
        let mode = thinking.map(|(_, mode)| *mode);
        let Some(cfg) = self
            .config_for(provider, id, mode, vision)
            .filter(ModelConfig::is_usable)
        else {
            return false;
        };
        self.rebuild(cfg);
        // The picked row names a model the provider serves, so from here on the
        // one-off backends may derive from it.
        self.real_backend = true;
        self.active_provider = Some(provider.to_string());
        self.active_model = id.to_string();
        self.active_vision = vision;
        self.active_context = context;
        self.active_thinking = mode;
        // The switch knows its support first-hand — a still-in-flight startup
        // probe is stale.
        self.probe_pending = false;
        self.persisted_selection = Some((provider.to_string(), id.to_string()));
        config::save_settings(
            self.settings_path.as_deref(),
            provider,
            id,
            Some(config::thinking_settings_of(thinking)),
            vision,
            context,
        );
        true
    }

    /// Rebind the *next* turn's backend so a Shift+Tab thinking mode rides its
    /// request (the running turn streams on its own thread, untouched — the
    /// `/model` pattern). A model with no usable config is left alone.
    pub(crate) fn rebind_thinking(&mut self, mode: ThinkingMode) {
        if !self.real_backend {
            return; // nothing to rebind: the dummy has no request to carry a mode
        }
        self.active_thinking = Some(mode);
        if let Some(provider) = self.active_provider.clone()
            && let Some(cfg) = self
                .config_for(
                    &provider,
                    &self.active_model.clone(),
                    Some(mode),
                    self.active_vision,
                )
                .filter(ModelConfig::is_usable)
        {
            self.rebuild(cfg);
        }
    }

    /// Persist the active selection's reasoning state beside it. Only writes
    /// onto the pair `config.json` already records: an env-overridden
    /// selection never writes back (env always wins, never sticks), so
    /// persisting its thinking would hijack the saved default.
    pub(crate) fn persist(&self, thinking: Option<&Thinking>) {
        let Some(provider) = self.active_provider.as_deref() else {
            return;
        };
        if self
            .persisted_selection
            .as_ref()
            .is_some_and(|(p, m)| p == provider && *m == self.active_model)
        {
            config::save_settings(
                self.settings_path.as_deref(),
                provider,
                &self.active_model,
                Some(config::thinking_settings_of(thinking)),
                self.active_vision,
                self.active_context,
            );
        }
    }

    /// Apply the capability probe's answer for `provider` (`docs/reasoning.md`,
    /// `docs/tools.md`): read the active model's record, adopt its reasoning /
    /// vision / context, rebind the next turn's backend when that changes what
    /// a request looks like, and persist so later startups seed from the file
    /// instead of probing. Returns the thinking state for `App`. A record for
    /// a provider that is no longer active is ignored.
    pub(crate) fn apply_probe(
        &mut self,
        provider: &str,
        models: &[ModelEntry],
    ) -> Option<Option<Thinking>> {
        if self.active_provider.as_deref() != Some(provider) {
            return None;
        }
        let entry = models.iter().find(|m| m.id == self.active_model);
        let thinking = entry
            .and_then(|entry| entry.reasoning.clone())
            .map(|support| {
                let mode = support.default_mode();
                (support, mode)
            });
        let vision = entry.and_then(|entry| entry.vision);
        // Rebind when something actually changes a request: a thinking mode to
        // ride it, or a known-blind model whose attachments must degrade
        // (Some(true)/None both attach — nothing to rebind for).
        if (thinking.is_some() || vision == Some(false))
            && let Some(cfg) = self
                .config_for(
                    provider,
                    &self.active_model.clone(),
                    thinking.as_ref().map(|(_, mode)| *mode),
                    vision,
                )
                .filter(ModelConfig::is_usable)
        {
            self.rebuild(cfg);
        }
        self.active_vision = vision;
        self.active_context = entry.and_then(|entry| entry.context);
        self.active_thinking = thinking.as_ref().map(|(_, mode)| *mode);
        self.persist(thinking.as_ref());
        Some(thinking)
    }

    /// The one-off **tools-free** backend a `/compact` turn runs on (codex
    /// sends the summarize request with no tools): the same persona +
    /// environment prompt, no background-notice injection. See `docs/compact.md`.
    ///
    /// `None` whenever the session itself isn't talking to a real model — the
    /// dummy, `ALTER_ZERO_DUMMY`, or the stalled test backend — and the caller
    /// then falls back to the session backend (the dummy scripts a text-only
    /// canned summary). That single [`Self::real_backend`] check is load-bearing:
    /// gating on a *usable config* instead only proved a key had resolved, so a
    /// configured provider with no model selected summarized against
    /// `dummy_model_name` and failed the turn with an HTTP error.
    pub(crate) fn compact_backend(&self, thinking: Option<ThinkingMode>) -> Option<LlmBackend> {
        if !self.real_backend {
            return None;
        }
        self.active_provider
            .as_deref()
            .and_then(|provider| {
                self.config_for(provider, &self.active_model, thinking, self.active_vision)
            })
            .filter(ModelConfig::is_usable)
            .map(|cfg| {
                LlmBackend::configure(
                    cfg,
                    self.system_prompt.clone(),
                    /*tools_enabled=*/ false,
                )
            })
    }

    // ----- the `/model` fetch and the `/login` key write -----

    /// Start the `/model` picker's fetch: cancel any in-flight one, then GET
    /// **every** configured provider's list off-thread in parallel (the picker
    /// shows each as it lands and merges them). Returns how many providers are
    /// being fetched — `0` means none is configured, and the caller points the
    /// user at `/login` instead.
    pub(crate) fn begin_model_fetch(
        &mut self,
        tx: &tokio::sync::mpsc::UnboundedSender<ModelFetch>,
    ) -> usize {
        self.cancel_model_fetch();
        let configured: Vec<ProviderChoice> = self
            .provider_choices()
            .into_iter()
            .filter(|c| c.configured)
            .collect();
        if configured.is_empty() {
            return 0;
        }
        let cancel = CancelToken::new();
        self.fetch_cancel = Some(cancel.clone());
        for choice in &configured {
            let cfg = self.config_for(&choice.id, &self.active_model, None, None);
            spawn_model_fetch(choice.name.clone(), cfg, cancel.clone(), tx.clone());
        }
        configured.len()
    }

    /// Cancel an in-flight `/model` fetch — the picker closed, a row was
    /// picked, or the app is quitting. Its detached worker exits on the cancel.
    pub(crate) fn cancel_model_fetch(&mut self) {
        if let Some(cancel) = self.fetch_cancel.take() {
            cancel.cancel();
        }
    }

    /// Clear the capability-probe gate: its answer arrived (or is now moot).
    pub(crate) fn settle_probe(&mut self) {
        self.probe_pending = false;
    }

    /// Persist an API key to the `.env` store and refresh the in-memory copy,
    /// so the next `/model` fetch or switch resolves it immediately. The value
    /// never enters the process env — `set_var` is `unsafe`, which this crate
    /// forbids. `Err` carries the message the caller shows.
    pub(crate) fn save_api_key(&mut self, env_var: &str, key: &str) -> Result<(), String> {
        let current = std::fs::read_to_string(&self.env_file_path).unwrap_or_default();
        let updated = EnvFile::upsert(&current, env_var, key);
        // The config home (`~/.alter-zero`) may not exist yet — create it
        // before the first write.
        if let Some(parent) = self.env_file_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match config::write_key_store(&self.env_file_path, &updated) {
            Ok(()) => {
                self.env_file = EnvFile::parse(&updated);
                Ok(())
            }
            Err(e) => Err(format!(
                "Couldn't write {}: {e}",
                self.env_file_path.display()
            )),
        }
    }
}

/// Build a session [`LlmBackend`] with the **full shared attachment set** —
/// Everything the boundary knows about the user's lifecycle hooks
/// (`docs/hooks.md`), held so that **every** backend rebuild re-attaches them —
/// the registries' pattern, and for the same reason: a rebuild that dropped
/// them would silently stop guarding for the rest of the session.
///
/// The *sink* is built per rebuild rather than stored, because a payload
/// carries the model name and a `/model` switch must not leave a stale one on
/// the wire.
#[derive(Debug, Clone)]
pub(crate) struct HookSetup {
    /// The parsed `hooks.json`.
    pub(crate) file: std::sync::Arc<alter_zero::hooks::HooksFile>,
    /// This run's id, the payload's `session_id`.
    pub(crate) session_id: String,
    /// Where handlers run, and what `*_PROJECT_DIR` points at.
    pub(crate) cwd: std::path::PathBuf,
    /// The tty-detach helper, so a hook child is severed from the terminal
    /// like every other shell child (`docs/tty-detach.md`).
    pub(crate) detach_helper: Option<std::path::PathBuf>,
    /// The `/settings` **Hooks** row. `false` attaches nothing, so toggling it
    /// off mid-session genuinely stops running them.
    pub(crate) enabled: bool,
    /// The permission gate, read at **dispatch** time so a payload's
    /// `permission_mode` is the mode the session is in when the hook fires —
    /// a Ctrl+A cycle reaches the very next call. `None` when the gate is off.
    pub(crate) gate: Option<alter_zero::permission::PermissionGate>,
    /// The rollout path the recorder publishes (`docs/hooks.md`), read per
    /// payload for the same reason.
    pub(crate) transcript: alter_zero::llm::hooks::TranscriptCell,
}

impl HookSetup {
    /// The sink for a backend answering as `model`, or `None` when hooks are
    /// switched off (or the file had nothing runnable).
    fn sink(&self, model: &str) -> Option<std::sync::Arc<dyn alter_zero::llm::hooks::HookSink>> {
        if !self.enabled {
            return None;
        }
        let context = alter_zero::hooks::HookContext {
            session_id: self.session_id.clone(),
            // Resolved live per payload from the shared cell / the gate —
            // these are only the fall-backs when neither handle answers.
            transcript_path: None,
            cwd: self.cwd.display().to_string(),
            model: model.to_string(),
            permission_mode: None,
            agent_id: None,
            agent_type: None,
        };
        alter_zero::llm::hooks::CommandHooks::new(
            std::sync::Arc::clone(&self.file),
            context,
            self.detach_helper.clone(),
            self.cwd.clone(),
            self.gate.clone(),
            self.transcript.clone(),
        )
        .map(|hooks| {
            std::sync::Arc::new(hooks) as std::sync::Arc<dyn alter_zero::llm::hooks::HookSink>
        })
    }
}

/// the background-shell registry, the subagent registry (enabling the `agent`
/// tool), and the permission gate. Every backend (re)build in the session goes
/// through this: the initial pick and each rebind (a `/model` switch, a
/// Shift+Tab thinking change, the startup probe) alike — a rebuild that
/// attached only part of the set silently lost the `agent` tool and stopped
/// asking permission for the rest of the session (the bug this helper fixes).
#[allow(clippy::too_many_arguments)] // a flat list of the knobs a build needs
fn session_backend(
    cfg: ModelConfig,
    system_prompt: Option<String>,
    tools: bool,
    max_retries: u32,
    max_tool_calls: usize,
    registry: &BackgroundRegistry,
    agents: &AgentRegistry,
    permissions: Option<&PermissionGate>,
    ask: &alter_zero::ask::AskGate,
    tasks: &alter_zero::tasks::TaskRegistry,
    hooks: Option<&HookSetup>,
) -> LlmBackend {
    // Captured before `configure` consumes the config: the hooks payload
    // names the model that is about to answer (docs/hooks.md).
    let model = cfg.model.clone();
    // `configure` rather than `with_system_prompt`: the tool toggle is the
    // `/settings` **Tools** knob now (seeded from `ALTER_ZERO_TOOLS`), so it
    // is passed in rather than re-read from the environment per build
    // (`docs/settings.md`).
    let mut backend = LlmBackend::configure(cfg, system_prompt, tools)
        .with_max_retries(max_retries)
        .with_max_tool_calls(max_tool_calls)
        .with_background(registry.clone())
        .with_agents(agents.clone())
        // The ask gate (docs/ask.md): enables the `askuserquestion` tool —
        // always attached; asking is not a permission.
        .with_ask(ask.clone())
        // The shared task list (docs/task-tools.md): enables the four task
        // tools — always attached, like the ask gate; the checklist is the
        // session's, so every rebuild re-binds the same registry.
        .with_tasks(tasks.clone());
    // The tool-permission gate (docs/permissions.md) — absent when
    // `ALTER_ZERO_PERMISSIONS` is falsy, and every tool then runs unasked.
    if let Some(gate) = permissions {
        backend = backend.with_permissions(gate.clone());
    }
    // The user's lifecycle hooks (docs/hooks.md). Built here, per rebuild, so
    // the payload's `model` is whatever is about to answer — a `/model` switch
    // cannot leave a stale name on the wire. `None` (no file, nothing runnable
    // in it, or the `/settings` **Hooks** row off) leaves the backend's no-op
    // sink in place and costs nothing.
    if let Some(sink) = hooks.and_then(|setup| setup.sink(&model)) {
        backend = backend.with_hooks(sink);
    }
    backend
}

// ===== the loop's `/model`, `/login`, Shift+Tab and probe arms =====

impl Session<'_> {
    /// Push the active backend's identity into `App`: the footer's model name,
    /// the system prompts the Ctrl+D view shows, and the context-window gauge.
    /// Run after any switch that changes which model answers.
    pub(crate) fn sync_backend_info(&mut self) {
        let name = self.models.model_name();
        self.app.set_session_info(name, self.cwd_display.clone());
        // The backend's system prompt rides into App so the Ctrl+D view shows the
        // whole context window (docs/context.md). None for the dummy. The
        // subagent variant (the main prompt + the subagent note) rides beside it
        // so an agent session view's Ctrl+D shows what a launched agent is
        // actually sent (docs/agent-tool.md).
        let prompt = self.models.backend().system_prompt();
        let agent_prompt = self.models.backend().agent_system_prompt();
        self.app.set_system_prompt(prompt);
        self.app.set_agent_system_prompt(agent_prompt);
        // The footer gauge + auto-compact window (docs/compact.md).
        let window = self.models.context_window();
        self.app.set_context_window(window);
    }

    /// `/model` from an idle composer (`docs/llm.md`): open the inline picker (it
    /// replaces the composer — no alternate screen) and fetch every configured
    /// provider's list off-thread in parallel; the picker shows each as it lands
    /// and merges them into one list. With none configured, skip the fetch and
    /// point the user at `/login` instead.
    pub(crate) fn open_model_picker(&mut self) {
        let current = self.models.active_model().to_string();
        self.app.open_model_picker(current);
        if let Some(provider) = self.models.active_provider() {
            self.app.set_active_provider(provider);
        }
        match self.models.begin_model_fetch(&self.model_tx) {
            0 => self.app.set_models_needs_login(),
            providers => self.app.begin_model_load(providers),
        }
    }

    /// Esc/Ctrl+C dismissed the inline picker: cancel a pending fetch and let the
    /// region collapse back to the composer on the next draw.
    pub(crate) fn close_model_picker(&mut self) {
        self.models.cancel_model_fetch();
    }

    /// Enter on a picker row: rebuild the backend for the chosen provider/model
    /// (`docs/llm.md`). The picker is already closed (`on_key` did it); cancel any
    /// pending fetch, then switch if the config is usable (has a key), else keep
    /// the current backend. The outcome is a transient toast — a mid-turn switch
    /// must not split the streaming reply in scrollback (`docs/toast.md`).
    pub(crate) fn select_model(
        &mut self,
        provider: &str,
        id: &str,
        reasoning: Option<ReasoningSupport>,
        vision: Option<bool>,
        context: Option<u64>,
    ) {
        self.models.cancel_model_fetch();
        // The picked entry's reasoning support seeds the Shift+Tab cycle at its
        // default mode (medium where offered) — docs/reasoning.md.
        let thinking = reasoning.map(|support| {
            let mode = support.default_mode();
            (support, mode)
        });
        if self
            .models
            .switch_to(provider, id, thinking.as_ref(), vision, context)
        {
            self.sync_backend_info();
            self.app.set_thinking(thinking);
            self.toast(format!("Switched model to {id}"), ToastKind::Info);
        } else {
            let env = self.models.key_env(provider);
            self.toast(
                format!("Can't switch to {id}: run /login to set {env}"),
                ToastKind::Error,
            );
        }
    }

    /// Shift+Tab advanced the thinking mode (the pure state already moved —
    /// `docs/reasoning.md`). Rebind the *next* turn's backend so the mode rides
    /// its request (the running turn streams on its own thread, untouched — the
    /// `/model` pattern), persist the choice beside the model selection, and
    /// confirm with a transient toast.
    pub(crate) fn set_thinking(&mut self, mode: ThinkingMode) {
        self.models.rebind_thinking(mode);
        let thinking = self
            .app
            .thinking
            .as_ref()
            .map(|t| (t.support.clone(), t.mode));
        self.models.persist(thinking.as_ref());
        self.toast(format!("Thinking: {}", mode.label()), ToastKind::Info);
    }

    /// `/login` from an idle composer (`docs/llm.md`): open the inline onboarding,
    /// its provider choices built from the file with the ✓ reflecting real env /
    /// `.env` key resolution. The hint names the real `.env` path.
    pub(crate) fn open_key_onboarding(&mut self) {
        let choices = self.models.provider_choices();
        let env_path = self.models.env_path_display().to_string();
        self.app.open_key_onboarding(choices, env_path);
    }

    /// Persist a key to the `.env` store and confirm — or report why it couldn't
    /// be written. The in-memory copy refreshes with it, so the next `/model`
    /// fetch or switch resolves it immediately.
    pub(crate) fn save_api_key(&mut self, provider: &str, env_var: &str, key: &str) {
        match self.models.save_api_key(env_var, key) {
            Ok(()) => self.toast(
                format!("Saved {env_var} — run /model to use {provider}"),
                ToastKind::Info,
            ),
            Err(reason) => self.toast(reason, ToastKind::Error),
        }
    }

    /// The startup capability probe answered (`docs/reasoning.md`,
    /// `docs/tools.md`): adopt the active model's reasoning / image / context
    /// facts, rebind the next turn's backend if that changes a request, and
    /// persist — so later startups seed from the file instead of probing. A
    /// `/model` switch that raced the probe already knows those facts first-hand
    /// and wins; a failed fetch just leaves the support unknown (no toast — this
    /// is background bookkeeping the user never asked for).
    pub(crate) fn apply_capability_probe(
        &mut self,
        provider: &str,
        result: Result<Vec<ModelEntry>, String>,
    ) {
        self.models.settle_probe();
        if let Ok(entries) = result
            && let Some(thinking) = self.models.apply_probe(provider, &entries)
        {
            self.app.set_thinking(thinking);
            let window = self.models.context_window();
            self.app.set_context_window(window);
            self.frame.schedule_frame();
        }
    }
}

impl Session<'_> {
    /// A finished `/model` fetch from one provider's worker thread: merge its
    /// models into the open picker, or record the failure beside the providers
    /// that did load. Both are no-ops if the picker was already dismissed. See
    /// `docs/llm.md`.
    pub(crate) fn on_model_fetch(
        &mut self,
        label: String,
        result: Result<Vec<ModelEntry>, String>,
    ) {
        match result {
            Ok(models) => self.app.add_models(models),
            Err(reason) => self.app.add_model_error(label, reason),
        }
        self.frame.schedule_frame();
    }
}
