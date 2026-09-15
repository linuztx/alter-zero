//! The reply backend and everything that selects it (`docs/llm.md`).
//!
//! [`ModelSession`] owns the answer to "who replies, and how": the provider
//! table, the resolved key, the active model, its reasoning mode
//! (`docs/reasoning.md`), its image support and context window
//! (`docs/tools.md`, `docs/compact.md`) — plus the `Box<dyn ReplySource>` all
//! of that resolves to. It exists because those knobs move **together**:
//! `/model`, `/login`, Ctrl+T and the startup capability probe each rebuild
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
//! - **The selection is the working directory's.** `config.json` keeps one
//!   entry per cwd (`docs/per-directory-state.md`): a `/model` switch here
//!   is this directory's from now on, and a directory launched in for the
//!   first time takes the last selection made anywhere and pins it as its own
//!   at startup — so a switch elsewhere never moves it afterwards.
//!
//! The dummy backend is the fallback throughout, so the app always runs
//! offline; a real model activates only when a provider, a model and a key all
//! resolve.

use std::path::Path;
use std::time::Duration;

use alter_zero::agents::AgentRegistry;
use alter_zero::app::{ProviderChoice, SubscriptionChoice, ToastKind};
use alter_zero::background::BackgroundRegistry;
use alter_zero::llm::{
    self, EnvFile, LlmBackend, ModelConfig, ModelEntry, ModelSelection, ProvidersFile,
    ReasoningSupport, ServiceTierSupport, ThinkingMode,
};
use alter_zero::permission::PermissionGate;
use alter_zero::settings::SessionSettings;
use alter_zero::stream::{self, CancelToken, DummyAi, ReplySource};
use alter_zero::ui;

use super::workers::{ModelFetch, spawn_model_fetch};
use super::{Session, config};

/// A model's capabilities as the loop tracks them: its reasoning support (and
/// the mode the Ctrl+T cycle starts at), whether it can see images, and how
/// big its context window is. `None` anywhere means "unknown" — the startup
/// probe finds out, and until it does the loop stays optimistic.
type Thinking = (ReasoningSupport, ThinkingMode);

/// A model's **service tiers** and the lane chosen in them, as the loop
/// tracks them — the [`Thinking`] twin for `/fast` (`docs/fast-mode.md`).
/// The inner `None` means no lane was ever chosen, so the catalog's own
/// default (if any) applies.
type Tiers = (ServiceTierSupport, Option<String>);

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
    /// The working directory as `config.json` keys its entries — which entry
    /// this session reads and writes (`docs/per-directory-state.md`).
    project: String,
    /// The (provider, model) pair `config.json` currently records **for this
    /// directory**. Thinking / vision / context writes attach to **this** pair
    /// only.
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
    /// Its runtime half alone — the environment and scratchpad blocks — for a
    /// subagent definition whose body replaces the persona
    /// (`docs/subagents.md`). Resolved once at startup beside it, so every
    /// rebuild inherits it by clone.
    prompt_context: Option<String>,
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
    /// silently dropping the Ctrl+T choice (`docs/reasoning.md`).
    active_thinking: Option<ThinkingMode>,
    /// The **resolved** service tier the live backend was built with — what
    /// the request actually sends, carried forward by an unrelated rebuild
    /// for the same reason the thinking mode is (`docs/fast-mode.md`).
    active_service_tier: Option<String>,
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
    /// The session's mid-turn message queue (`docs/queue.md`) — re-attached
    /// on every rebuild like the registries, so a `/model` switch or a
    /// `/settings` change never leaves the loop pushing into a queue no
    /// backend is draining.
    steer: alter_zero::steer::SteerQueue,
    permissions: Option<PermissionGate>,
    ask: alter_zero::ask::AskGate,
    /// The shared task list (docs/task-tools.md) — enables the four task
    /// tools on every rebuild.
    tasks: alter_zero::tasks::TaskRegistry,
    /// The subagent definitions (`docs/subagents.md`) — the shared handle
    /// every rebuild re-attaches, so a type the per-turn rescan found is
    /// launchable without one.
    subagents: alter_zero::subagents::SubagentRegistry,
    /// The discovered skills (`docs/skills.md`) — enables the `skill` tool on
    /// every rebuild, held even when the `/settings` **Skills** row is off so
    /// turning it back on needs no rescan. `skills_enabled` is the knob.
    skills: alter_zero::skills::SkillRegistry,
    /// Whether the `skill` tool is offered at all — the `/settings` **Skills**
    /// knob, read by every rebuild so a mid-session switch sticks.
    skills_enabled: bool,
    /// Whether the **live** backend was built with the `skill` tool on it.
    ///
    /// Tracked rather than re-derived, because it is the *last build's*
    /// verdict: [`Self::refresh_skills`] runs on every turn now (the per-turn
    /// rescan) and only the turns where this disagrees with
    /// [`Self::skills_offered`] need a rebuild. Re-deriving both sides would
    /// make them equal by construction and rebuild either never or always.
    skills_attached: bool,
    /// The session's MCP manager (`docs/mcp.md`) — `None` when the feature is
    /// off. Re-attached on every rebuild like the registries above; the
    /// tracked `mcp_fingerprint` is the *last build's* offered wire-name set,
    /// so [`Self::refresh_mcp`] rebuilds only when a connection change
    /// actually flipped what the request carries.
    mcp: Option<alter_zero::llm::mcp::McpManager>,
    mcp_fingerprint: Vec<String>,
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
    /// The service-tier state [`Self::resolve`] recovered from `config.json`
    /// — the lanes and the chosen one — handed to `App` once at bootstrap so
    /// `/fast` and the footer marker work before any probe answers.
    service_tier_seed: Option<Tiers>,
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
        scratchpad: Option<&Path>,
        registry: &BackgroundRegistry,
        agents: &AgentRegistry,
        steer: &alter_zero::steer::SteerQueue,
        permissions: Option<&PermissionGate>,
        ask: &alter_zero::ask::AskGate,
        tasks: &alter_zero::tasks::TaskRegistry,
        skills: &alter_zero::skills::SkillRegistry,
        subagents: &alter_zero::subagents::SubagentRegistry,
        mcp: Option<&alter_zero::llm::mcp::McpManager>,
        settings: &SessionSettings,
        hooks: Option<HookSetup>,
    ) -> Self {
        let providers = config::load_providers();
        // The persistent API-key store: `.env` in the config home (or
        // `ALTER_ZERO_ENV_FILE`), written by the `/login` flow and consulted
        // during key resolution (a real process env var still wins).
        let env_file_path = config::env_file_path();
        let env_file = config::load_env_file(&env_file_path);
        // OpenAI rotates a refresh token as it is used, and the rotation
        // happens deep on a backend thread with no path in hand — so tell the
        // module where the store is, once. Without this the retired token
        // stays on disk and the next launch is a forced re-login
        // (`docs/chatgpt.md`).
        llm::chatgpt::set_store_path(env_file_path.clone());
        // Anthropic rotates its refresh token the same way, and the write-back
        // happens just as deep on a backend thread (`docs/claude.md`).
        llm::claude::set_store_path(env_file_path.clone());
        // The persisted `/model` selection (`~/.alter-zero/config.json`), per
        // working directory (docs/per-directory-state.md): this directory's
        // own entry, or — launched in for the first time — the last selection
        // made anywhere, pinned as this directory's own right here so a later
        // switch elsewhere never moves it.
        let project = cwd.display().to_string();
        let settings_path = config::settings_file_path();
        let saved = config::adopt_selection(settings_path.as_deref(), &project);
        let saved_provider = saved.as_ref().map(|s| s.provider.clone());
        // The `/settings` knobs the backend is built around — already merged
        // with their `ALTER_ZERO_*` overrides by the caller
        // (`config::apply_setting_overrides`, docs/settings.md).
        let temperature = settings.temperature;
        let tools = settings.tools;
        let max_retries = settings.error_retry;
        let max_tool_calls = settings.max_tool_calls;
        // The **Skills** knob and the discovered set are separate: an empty
        // registry attaches nothing either way, but holding it through an off
        // spell means turning the row back on needs no rescan.
        let skills_enabled = settings.skills;
        let skills = skills.clone();
        let subagents = subagents.clone();
        let system_prompt = config::system_prompt(cwd, scratchpad);
        // The runtime half alone, for an agent definition whose body replaces
        // the persona (docs/subagents.md).
        let prompt_context = config::prompt_context(cwd, scratchpad);
        // The provider the /model picker lists from and switches within: env,
        // else the saved selection, else the file's default.
        let active_provider = std::env::var("ALTER_ZERO_PROVIDER")
            .ok()
            .filter(|s| !s.is_empty())
            .or_else(|| saved_provider.clone())
            .or_else(|| providers.default_provider());
        // The saved provider/model are ONE selection: pairing the saved model
        // with a *different* (env-overridden) provider would ask that provider
        // for a model it may not serve, so the saved model applies only when
        // the resolved provider is the one it was saved with.
        let saved_model = saved
            .as_ref()
            .map(|s| s.model.clone())
            .filter(|_| active_provider == saved_provider);
        let env_model = std::env::var("ALTER_ZERO_MODEL")
            .ok()
            .filter(|s| !s.is_empty())
            .or(saved_model);
        let stall_ms = config::stall_ms();
        let env_context_window = config::context_window_override();
        // The saved thinking blob describes the saved (provider, model)
        // pairing — like saved_model it applies only when that exact selection
        // resolved. Outer None = support unknown (the probe below finds out);
        // Some(None) = known non-reasoner; Some(Some(state)) = seed the
        // Ctrl+T cycle. See docs/reasoning.md.
        let selection_is_saved = active_provider == saved_provider
            && env_model.is_some()
            && env_model == saved.as_ref().map(|s| s.model.clone());
        let saved_thinking: Option<Option<Thinking>> = saved
            .as_ref()
            .and_then(|s| s.thinking.as_ref())
            .filter(|_| selection_is_saved)
            .map(llm::ThinkingSettings::to_seed);
        let startup_thinking = saved_thinking.clone().flatten();
        // The saved model's image-input support and context window — like
        // saved_thinking they apply only to that exact selection; `None` =
        // unknown (the probe finds out).
        let saved_vision: Option<bool> = saved
            .as_ref()
            .and_then(|s| s.vision)
            .filter(|_| selection_is_saved);
        let saved_context: Option<u64> = saved
            .as_ref()
            .and_then(|s| s.context)
            .filter(|_| selection_is_saved);
        // The saved service-tier blob, read exactly as saved_thinking is:
        // outer None = the lanes are unknown (the probe below finds out);
        // Some(None) = a model known to publish none; Some(Some(state)) =
        // seed `/fast` and the footer marker. See docs/fast-mode.md.
        let saved_tiers: Option<Option<Tiers>> = saved
            .as_ref()
            .and_then(|s| s.service_tier.as_ref())
            .filter(|_| selection_is_saved)
            .map(llm::ServiceTierSettings::to_state);
        let startup_tiers = saved_tiers.clone().flatten();
        // What the first request carries — the chosen lane filtered through
        // the lanes the model actually offers, so a stale choice is dropped
        // rather than 400ing the opening turn.
        let startup_service_tier = startup_tiers
            .as_ref()
            .and_then(|(support, selected)| support.for_request(selected.as_deref()));

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
                    startup_service_tier.clone(),
                    saved_vision,
                    // The window the first request is built with — the env
                    // override outranking the saved one, as the gauge's own
                    // `context_window` reads them (docs/ollama.md).
                    env_context_window.or(saved_context),
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
                prompt_context.clone(),
                tools,
                max_retries,
                max_tool_calls,
                registry,
                agents,
                steer,
                permissions,
                ask,
                tasks,
                skills_enabled.then_some(&skills),
                &subagents,
                mcp,
                hooks.as_ref(),
            )),
            (None, None) => {
                // The subagent registry rides the dummy too: the "subagent"
                // demo streams a launched agent's own round on that channel,
                // which is the only offline way to drive the agent session
                // view (`docs/agent-view-streaming.md`).
                let dummy = DummyAi::with_startup_delay(config::startup_delay())
                    .with_ask(ask.clone())
                    .with_agents(agents.clone())
                    // …and the mid-turn queue, so the offline demo takes a
                    // queued message at its next tool boundary exactly as a
                    // real round boundary does (docs/queue.md).
                    .with_steer(steer.clone());
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
        let probe = (real_backend
            && (saved_thinking.is_none() || saved_vision.is_none() || saved_tiers.is_none()))
        .then(|| active_provider.as_deref().zip(env_model.as_deref()))
        .flatten()
        .map(|(p, m)| {
            let cfg = config::model_config_for(
                &providers,
                &env_file,
                p,
                m,
                temperature,
                None,
                None,
                None,
                None,
            );
            (p.to_string(), cfg)
        });
        Self {
            providers,
            env_path_display: ui::display_cwd(&env_file_path, home),
            env_file,
            env_file_path,
            settings_path,
            project,
            persisted_selection: saved
                .as_ref()
                .map(|s| (s.provider.clone(), s.model.clone())),
            temperature,
            tools,
            max_retries,
            max_tool_calls,
            system_prompt,
            prompt_context,
            stall_ms,
            env_context_window,
            active_provider,
            active_model,
            active_vision: real_backend.then_some(saved_vision).flatten(),
            active_context: real_backend.then_some(saved_context).flatten(),
            active_thinking: startup_thinking.as_ref().map(|(_, mode)| *mode),
            active_service_tier: real_backend.then_some(startup_service_tier).flatten(),
            backend,
            real_backend,
            registry: registry.clone(),
            agents: agents.clone(),
            steer: steer.clone(),
            permissions: permissions.cloned(),
            ask: ask.clone(),
            tasks: tasks.clone(),
            subagents,
            // What the backend built just above was handed: the same three
            // gates, so the first `refresh_skills` measures a real change
            // rather than the difference between two spellings of "on".
            skills_attached: real_backend && tools && skills_enabled && skills.has_enabled(),
            skills,
            skills_enabled,
            mcp_fingerprint: mcp
                .map(alter_zero::llm::mcp::McpManager::fingerprint)
                .unwrap_or_default(),
            mcp: mcp.cloned(),
            hooks,
            fetch_cancel: None,
            probe_pending: probe.is_some(),
            thinking_seed: real_backend.then_some(startup_thinking).flatten(),
            service_tier_seed: real_backend.then_some(startup_tiers).flatten(),
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

    /// The window a **subagent's** gauge runs against
    /// (`docs/agent-context-gauge.md`): a type that inherits the model shares
    /// [`context_window`](Self::context_window), while one pinned to another
    /// model has a window this session never learned — the listing is read
    /// for the *selected* model only — so only the `ALTER_ZERO_CONTEXT_WINDOW`
    /// override answers there, and `None` hides that view's gauge exactly as
    /// an unknown window hides the main one.
    pub(crate) fn agent_context_window(&self, inherits: bool) -> Option<u64> {
        if inherits {
            self.context_window()
        } else {
            self.env_context_window
        }
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

    /// The **API-key** rows the `/login` flow shows, each tagged with whether
    /// a key already resolves. A subscription provider is not among them —
    /// there is no key to paste for one (`docs/copilot.md`).
    pub(crate) fn provider_choices(&self) -> Vec<ProviderChoice> {
        config::provider_choices(&self.providers, &self.env_file)
    }

    /// The **subscription** rows the `/login` flow shows, each tagged with
    /// whether it is already signed in.
    pub(crate) fn subscription_choices(&self) -> Vec<SubscriptionChoice> {
        config::subscription_choices(&self.providers, &self.env_file)
    }

    /// Is this provider signed in to rather than keyed? What decides whether
    /// a `/login` row starts a device flow.
    pub(crate) fn is_subscription(&self, provider: &str) -> bool {
        self.providers
            .get(provider)
            .is_some_and(|p| p.auth.is_subscription())
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

    /// The service-tier state recovered from `config.json`, handed to `App`
    /// once (`docs/fast-mode.md`).
    pub(crate) fn take_service_tier_seed(&mut self) -> Option<Tiers> {
        self.service_tier_seed.take()
    }

    /// The capability probe to spawn at bootstrap, if one is due.
    pub(crate) fn take_probe(&mut self) -> Option<(String, Option<ModelConfig>)> {
        self.probe.take()
    }

    // ----- rebuilding -----

    /// The resolved config for a provider/model with the given capabilities,
    /// or `None` when the provider isn't in the table. `context` is the
    /// window a request is built with — the env override outranks it here
    /// exactly as it outranks the gauge (`docs/ollama.md`).
    #[allow(clippy::too_many_arguments)] // the capability set, resolved together
    fn config_for(
        &self,
        provider: &str,
        model: &str,
        thinking: Option<ThinkingMode>,
        service_tier: Option<String>,
        vision: Option<bool>,
        context: Option<u64>,
    ) -> Option<ModelConfig> {
        config::model_config_for(
            &self.providers,
            &self.env_file,
            provider,
            model,
            self.temperature,
            thinking,
            service_tier,
            vision,
            self.env_context_window.or(context),
        )
    }

    /// Does this provider's wire **send** the context window? Only Ollama's
    /// (`options.num_ctx`), so only there does learning the window change
    /// what a request looks like.
    fn wire_sends_context(&self, provider: &str) -> bool {
        self.providers
            .get(provider)
            .is_some_and(|p| p.wire_api == llm::WireApi::Ollama)
    }

    /// Rebuild the backend for `cfg` with the **full** shared attachment set —
    /// the background registry, the subagent registry (enabling the `agent`
    /// tool) and the permission gate. Every (re)build in the session goes
    /// through here: the initial pick and each rebind (a `/model` switch, a
    /// Ctrl+T thinking change, the capability probe) alike.
    fn rebuild(&mut self, cfg: ModelConfig) {
        // Recorded here because this is the one place a backend is actually
        // built — `set_tools`, a `/model` switch and a skill toggle all land
        // on it, and any of them can change the verdict.
        self.skills_attached = self.skills_offered();
        self.mcp_fingerprint = self
            .mcp
            .as_ref()
            .map(alter_zero::llm::mcp::McpManager::fingerprint)
            .unwrap_or_default();
        self.backend = Box::new(session_backend(
            cfg,
            self.system_prompt.clone(),
            self.prompt_context.clone(),
            self.tools,
            self.max_retries,
            self.max_tool_calls,
            &self.registry,
            &self.agents,
            &self.steer,
            self.permissions.as_ref(),
            &self.ask,
            &self.tasks,
            self.skills_enabled.then_some(&self.skills),
            &self.subagents,
            self.mcp.as_ref(),
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
                    self.active_service_tier.clone(),
                    self.active_vision,
                    self.active_context,
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

    /// Whether the hooks config resolved with anything runnable in it — the
    /// `/settings` **Hooks** row's availability (`docs/hooks.md`). The setup
    /// itself now exists even over an empty file (so a `/trust` approval can
    /// swap a project merge in mid-session, `docs/project-config.md`); an
    /// empty merge still reports unavailable, exactly as no setup used to.
    pub(crate) fn hooks_available(&self) -> bool {
        self.hooks
            .as_ref()
            .is_some_and(|setup| !setup.file.is_empty())
    }

    /// Swap the parsed hooks config the session runs — the `/trust` seam
    /// (`docs/project-config.md`): an approval rebinds the user+project
    /// merge, a revoke rebinds the user layer alone. The rebuild carries it
    /// into the next backend build; the live handles (gate, transcript,
    /// SessionStart sources) ride along untouched.
    pub(crate) fn set_hooks_file(&mut self, file: alter_zero::hooks::HooksFile) {
        if let Some(setup) = self.hooks.as_mut() {
            setup.file = std::sync::Arc::new(file);
        }
        self.rebuild_current();
    }

    /// The parsed hooks file digested for the read-only `/hooks` browser,
    /// plus whether the session actually runs it (`docs/hooks-menu.md`). The
    /// digest comes from the same `Arc<HooksFile>` every backend rebuild
    /// re-attaches, so the browser and the dispatcher can never disagree; a
    /// session with no runnable hooks browses as empty (the `/settings`
    /// row's own posture).
    pub(crate) fn hooks_browse(&self) -> (alter_zero::hooks::HooksOverview, bool) {
        match &self.hooks {
            Some(setup) => (
                alter_zero::hooks::HooksOverview::from_file(&setup.file),
                setup.enabled,
            ),
            None => (
                alter_zero::hooks::HooksOverview::from_file(
                    &alter_zero::hooks::HooksFile::default(),
                ),
                false,
            ),
        }
    }

    /// Queue a `SessionStart` source (`startup` / `resume` / `clear`) for the
    /// next turn's drain (`docs/hooks.md`). A no-op without hooks.
    pub(crate) fn queue_session_source(&self, source: &str) {
        if let Some(setup) = &self.hooks
            && let Ok(mut sources) = setup.handles.sources.lock()
        {
            sources.push(source.to_string());
        }
    }

    /// Fire the `SessionEnd` hooks (`docs/hooks.md`) — a `/clear`'s `clear`,
    /// a quit's `prompt_input_exit`. Bounded inside the sink (2 s for the
    /// whole event), so neither path can hang on a hook. A no-op without
    /// hooks.
    pub(crate) fn fire_session_end(&self, reason: &str) {
        if let Some(sink) = self
            .hooks
            .as_ref()
            .and_then(|setup| setup.sink(&self.model_name()))
        {
            sink.session_end(reason);
        }
    }

    /// Replace every pending source with `source` — the boot path's variant:
    /// a `--continue`/`--resume` boot crossed a *resume* boundary, not the
    /// startup one bootstrap seeded before it knew (`docs/hooks.md`).
    pub(crate) fn set_session_source(&self, source: &str) {
        if let Some(setup) = &self.hooks
            && let Ok(mut sources) = setup.handles.sources.lock()
        {
            sources.clear();
            sources.push(source.to_string());
        }
    }

    /// Mark the next spawned turn as **loop-initiated** (a background
    /// completion's follow-up): its prompt is synthesized, so the
    /// `UserPromptSubmit` hook must not fire for it (`docs/hooks.md`).
    pub(crate) fn mark_synthetic_turn(&self) {
        if let Some(setup) = &self.hooks {
            setup
                .handles
                .synthetic_turn
                .store(true, std::sync::atomic::Ordering::SeqCst);
        }
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

    /// The `/settings` **Skills** knob: whether the `skill` tool and the
    /// listing are offered from here on (`docs/skills.md`).
    pub(crate) fn set_skills(&mut self, enabled: bool) {
        self.skills_enabled = enabled;
        self.rebuild_current();
    }

    /// Whether a build right now would put the `agent` spec on the wire — the
    /// gate the agent-type listing rides (`docs/subagents.md`). The registry
    /// is attached to every real build, so this is the tools knob plus the
    /// dummy, which scripts its agent demo rather than being offered a spec.
    pub(crate) fn agents_offered(&self) -> bool {
        self.real_backend && self.tools
    }

    /// Whether a build right now would put the `skill` spec on the wire: the
    /// three gates `with_skills` applies (tools at all, the `/settings`
    /// **Skills** row, and something actually enabled), plus the dummy — which
    /// is never handed the registry, so it can never be out of date.
    fn skills_offered(&self) -> bool {
        self.real_backend && self.tools && self.skills_enabled && self.skills.has_enabled()
    }

    /// Re-attach the registry after its contents moved — the `/skills` menu
    /// turning a skill on or off, or the per-turn rescan finding (or losing)
    /// one. The handle is shared, so the executor and the listing already saw
    /// the change; this is only about the **tool set**, which is decided when
    /// `with_skills` runs: turning the last skill off has to withdraw the spec
    /// rather than leave a tool that can only fail (`docs/skills.md`).
    ///
    /// Rebuilds only when that verdict actually flips. Toggling one of five
    /// skills changes nothing about the request's shape, and this runs every
    /// turn — an unconditional rebuild would re-derive the whole backend on
    /// each one for nothing.
    pub(crate) fn refresh_skills(&mut self) {
        if self.skills_offered() != self.skills_attached {
            self.rebuild_current();
        }
    }

    /// Re-attach the MCP manager after its connections moved — a server
    /// connected, failed, was disabled or re-authenticated (`docs/mcp.md`).
    /// The handle is shared (the executor already routes through the live
    /// connections); this is only about the **tool set**, decided when
    /// `with_mcp` ran — so it rebuilds only when the offered wire-name set
    /// actually changed (`refresh_skills`'s exact posture, fingerprinted
    /// because the set is a list rather than a bool).
    pub(crate) fn refresh_mcp(&mut self) {
        let Some(manager) = &self.mcp else { return };
        if manager.fingerprint() != self.mcp_fingerprint {
            self.rebuild_current();
        }
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
    #[allow(clippy::too_many_arguments)] // the picked row's capability set
    pub(crate) fn switch_to(
        &mut self,
        provider: &str,
        id: &str,
        thinking: Option<&Thinking>,
        tiers: Option<&Tiers>,
        vision: Option<bool>,
        context: Option<u64>,
    ) -> bool {
        let mode = thinking.map(|(_, mode)| *mode);
        // The picked row's own lanes decide what the first request to it
        // carries — a lane chosen on the *previous* model means nothing here
        // (`docs/fast-mode.md`).
        let tier = tiers.and_then(|(support, selected)| support.for_request(selected.as_deref()));
        let Some(cfg) = self
            .config_for(provider, id, mode, tier.clone(), vision, context)
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
        self.active_service_tier = tier;
        // The switch knows its support first-hand — a still-in-flight startup
        // probe is stale.
        self.probe_pending = false;
        self.persisted_selection = Some((provider.to_string(), id.to_string()));
        // This directory's choice from now on — and the last one made
        // anywhere, which is what the next new directory starts with.
        config::save_selection(
            self.settings_path.as_deref(),
            &self.project,
            &ModelSelection::new(provider, id)
                .with_thinking(Some(config::thinking_settings_of(thinking)))
                .with_service_tier(Some(config::service_tier_settings_of(tiers)))
                .with_vision(vision)
                .with_context(context),
        );
        true
    }

    /// Rebind the *next* turn's backend so a Ctrl+T thinking mode rides its
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
                    self.active_service_tier.clone(),
                    self.active_vision,
                    self.active_context,
                )
                .filter(ModelConfig::is_usable)
        {
            self.rebuild(cfg);
        }
    }

    /// Rebind the *next* turn's backend so a `/fast` choice rides its request
    /// — [`Self::rebind_thinking`]'s twin, and mid-turn safe for the same
    /// reason: the running turn streams on its own thread with the config it
    /// was built from. `tier` is the **resolved** lane (already filtered
    /// against what the model offers), so `None` is the standard one. See
    /// `docs/fast-mode.md`.
    pub(crate) fn rebind_service_tier(&mut self, tier: Option<String>) {
        if !self.real_backend {
            return; // the dummy has no request to carry a lane
        }
        self.active_service_tier = tier.clone();
        if let Some(provider) = self.active_provider.clone()
            && let Some(cfg) = self
                .config_for(
                    &provider,
                    &self.active_model.clone(),
                    self.active_thinking,
                    tier,
                    self.active_vision,
                    self.active_context,
                )
                .filter(ModelConfig::is_usable)
        {
            self.rebuild(cfg);
        }
    }

    /// Persist the active selection's reasoning state beside it. Only writes
    /// onto the pair `config.json` already records for this directory: an
    /// env-overridden selection never writes back (env always wins, never
    /// sticks), so persisting its thinking would hijack the saved default.
    ///
    /// `tiers` carries a **third** state the thinking blob has no need for:
    /// the outer `None` means *not known yet* (a probe still pending, or one
    /// that failed) and writes nothing, where `Some(None)` is the definitive
    /// "this model publishes no lanes" marker. Collapsing the two would let a
    /// Ctrl+T pressed before the probe answers record `unsupported` for a
    /// model that does have a fast lane — and since that marker is what stops
    /// the *next* launch probing, `/fast` would then be dead there for good.
    /// See `docs/fast-mode.md`.
    pub(crate) fn persist(&self, thinking: Option<&Thinking>, tiers: Option<Option<&Tiers>>) {
        let Some(provider) = self.active_provider.as_deref() else {
            return;
        };
        if self
            .persisted_selection
            .as_ref()
            .is_some_and(|(p, m)| p == provider && *m == self.active_model)
        {
            config::save_capabilities(
                self.settings_path.as_deref(),
                &self.project,
                &ModelSelection::new(provider, &self.active_model)
                    .with_thinking(Some(config::thinking_settings_of(thinking)))
                    .with_service_tier(tiers.map(config::service_tier_settings_of))
                    .with_vision(self.active_vision)
                    .with_context(self.active_context),
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
        chosen_tier: Option<String>,
    ) -> Option<(Option<Thinking>, Option<Tiers>)> {
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
        // The lanes the record publishes. The probe learns what the model
        // *offers*, never what the user picked — a choice already made (from
        // `config.json`, or a `/fast` that raced the probe) is kept, and only
        // re-filtered against the freshly-learned lanes.
        //
        // `chosen_tier` is the **raw** choice, handed in by the caller from
        // `App`, and it has to be: `active_service_tier` is the *resolved*
        // lane, where an explicit standard choice and never having chosen are
        // both `None`. Reading that instead would let the record's own
        // `default_service_tier` apply over a user who deliberately asked for
        // the standard lane — turning fast silently back on at every launch.
        let tiers: Option<Tiers> = entry
            .and_then(|entry| entry.tiers.clone())
            .map(|support| (support, chosen_tier));
        let tier = tiers
            .as_ref()
            .and_then(|(support, selected)| support.for_request(selected.as_deref()));
        let vision = entry.and_then(|entry| entry.vision);
        let context = entry.and_then(|entry| entry.context);
        // Rebind when something actually changes a request: a thinking mode to
        // ride it, a known-blind model whose attachments must degrade
        // (Some(true)/None both attach — nothing to rebind for), or — on the
        // one wire that sends it — a window now known (docs/ollama.md).
        let window_matters = context.is_some() && self.wire_sends_context(provider);
        // A lane the request will now carry — or one it was carrying and the
        // record says the model does not offer — changes the request too.
        let tier_matters = tier != self.active_service_tier;
        if (thinking.is_some() || vision == Some(false) || window_matters || tier_matters)
            && let Some(cfg) = self
                .config_for(
                    provider,
                    &self.active_model.clone(),
                    thinking.as_ref().map(|(_, mode)| *mode),
                    tier.clone(),
                    vision,
                    context,
                )
                .filter(ModelConfig::is_usable)
        {
            self.rebuild(cfg);
        }
        self.active_vision = vision;
        self.active_context = context;
        self.active_thinking = thinking.as_ref().map(|(_, mode)| *mode);
        self.active_service_tier = tier;
        // The record answered, so the lanes are known either way.
        self.persist(thinking.as_ref(), Some(tiers.as_ref()));
        Some((thinking, tiers))
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
    pub(crate) fn compact_backend(
        &self,
        thinking: Option<ThinkingMode>,
        auto: bool,
    ) -> Option<LlmBackend> {
        if !self.real_backend {
            return None;
        }
        self.active_provider
            .as_deref()
            .and_then(|provider| {
                self.config_for(
                    provider,
                    &self.active_model,
                    thinking,
                    // The summarization turn runs in the same lane the
                    // conversation does — it is the same account's request.
                    self.active_service_tier.clone(),
                    self.active_vision,
                    self.active_context,
                )
            })
            .filter(ModelConfig::is_usable)
            .map(|cfg| {
                let model = cfg.model.clone();
                let mut backend = LlmBackend::configure(
                    cfg,
                    self.system_prompt.clone(),
                    /*tools_enabled=*/ false,
                );
                // The compact events (docs/hooks.md): PreCompact's context
                // becomes extra summarization instructions, PostCompact hears
                // the summary — via the wrapper, so the ordinary session
                // events (UserPromptSubmit, Stop, the source drain) can never
                // fire for a summarization turn.
                if let Some(setup) = self.hooks.as_ref().filter(|setup| setup.enabled) {
                    let context = alter_zero::hooks::HookContext {
                        session_id: setup.session_id.clone(),
                        transcript_path: None,
                        cwd: setup.cwd.display().to_string(),
                        model,
                        permission_mode: None,
                        agent_id: None,
                        agent_type: None,
                    };
                    if let Some(inner) = alter_zero::llm::hooks::CommandHooks::new(
                        std::sync::Arc::clone(&setup.file),
                        context,
                        setup.detach_helper.clone(),
                        setup.cwd.clone(),
                        setup.handles.clone(),
                    ) {
                        backend = backend.with_hooks(std::sync::Arc::new(
                            alter_zero::llm::hooks::CompactHooks::new(inner, auto),
                        ));
                    }
                }
                backend
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
        // Every provider with a resolving credential — a pasted key *or* a
        // subscription's stored token. `provider_choices` answers a different
        // question (which providers `/login` offers a key field for), and
        // fetching from that list would leave a signed-in GitHub Copilot with
        // no models in the picker at all (`docs/copilot.md`).
        let configured: Vec<ProviderChoice> =
            config::all_provider_choices(&self.providers, &self.env_file)
                .into_iter()
                .filter(|c| c.configured)
                .collect();
        if configured.is_empty() {
            return 0;
        }
        let cancel = CancelToken::new();
        self.fetch_cancel = Some(cancel.clone());
        for choice in &configured {
            let cfg = self.config_for(&choice.id, &self.active_model, None, None, None, None);
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
    /// The live handles — the gate (a Ctrl+T cycle reaches the very next
    /// payload), the rollout path the recorder publishes, the queued
    /// SessionStart sources, and the synthetic-turn mark — shared into every
    /// sink built, surviving each rebuild (`docs/hooks.md`).
    pub(crate) handles: alter_zero::llm::hooks::HookHandles,
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
            self.handles.clone(),
        )
        .map(|hooks| {
            std::sync::Arc::new(hooks) as std::sync::Arc<dyn alter_zero::llm::hooks::HookSink>
        })
    }
}

/// the background-shell registry, the subagent registry (enabling the `agent`
/// tool), and the permission gate. Every backend (re)build in the session goes
/// through this: the initial pick and each rebind (a `/model` switch, a
/// Ctrl+T thinking change, the startup probe) alike — a rebuild that
/// attached only part of the set silently lost the `agent` tool and stopped
/// asking permission for the rest of the session (the bug this helper fixes).
#[allow(clippy::too_many_arguments)] // a flat list of the knobs a build needs
fn session_backend(
    cfg: ModelConfig,
    system_prompt: Option<String>,
    prompt_context: Option<String>,
    tools: bool,
    max_retries: u32,
    max_tool_calls: usize,
    registry: &BackgroundRegistry,
    agents: &AgentRegistry,
    steer: &alter_zero::steer::SteerQueue,
    permissions: Option<&PermissionGate>,
    ask: &alter_zero::ask::AskGate,
    tasks: &alter_zero::tasks::TaskRegistry,
    skills: Option<&alter_zero::skills::SkillRegistry>,
    subagents: &alter_zero::subagents::SubagentRegistry,
    mcp: Option<&alter_zero::llm::mcp::McpManager>,
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
        // The runtime half of the prompt, for a subagent definition whose
        // body replaces the persona (docs/subagents.md).
        .with_prompt_context(prompt_context)
        .with_background(registry.clone())
        .with_agents(agents.clone())
        // The subagent definitions the `agent` tool's types come from
        // (docs/subagents.md) — the shared handle, so the per-turn rescan
        // reaches this backend without a rebuild.
        .with_subagents(subagents.clone())
        // The mid-turn message queue (docs/queue.md): every build drains the
        // same one, so a message queued against the turn a `/model` switch
        // replaced still reaches its successor.
        .with_steer(steer.clone())
        // The ask gate (docs/ask.md): enables the `askuserquestion` tool —
        // always attached; asking is not a permission.
        .with_ask(ask.clone())
        // The shared task list (docs/task-tools.md): enables the four task
        // tools — always attached, like the ask gate; the checklist is the
        // session's, so every rebuild re-binds the same registry.
        .with_tasks(tasks.clone());
    // The discovered skills (docs/skills.md): enables the `skill` tool, and
    // rides every rebuild so a `/model` switch keeps them. `None` is the
    // `/settings` **Skills** row off; an empty registry attaches nothing.
    if let Some(skills) = skills {
        backend = backend.with_skills(skills.clone());
    }
    // The MCP servers' tools (docs/mcp.md): one `mcp__server__tool` spec per
    // connected server's tool — a manager with nothing connected attaches
    // nothing, and `refresh_mcp` re-runs this build when that changes.
    if let Some(manager) = mcp {
        backend = backend.with_mcp(manager.clone());
    }
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

// ===== the loop's `/model`, `/login`, Ctrl+T and probe arms =====

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
        self.app.set_system_prompt(prompt);
        // The classifier's rubric rides beside it for the Ctrl+D classifier
        // page (docs/permissions.md). None for the dummy, which has no
        // classifier.
        let classifier_prompt = self.models.backend().classifier_system_prompt();
        self.app.set_classifier_system_prompt(classifier_prompt);
        self.sync_agent_view_context();
        // The footer gauge + auto-compact window (docs/compact.md).
        let window = self.models.context_window();
        self.app.set_context_window(window);
        // The skill listing is budgeted off that same window (1% of it, in
        // characters), so it is re-rendered here rather than once at startup:
        // a `/model` switch to a roomier model widens the listing with it.
        self.sync_listings();
    }

    /// Push the **viewed** subagent type's system prompt *and its briefing*
    /// into `App`, so an agent session view's Ctrl+D shows what *that* agent
    /// was sent — its definition's own body when it has one, and the skills
    /// `<system-reminder>` its fresh context opened on (`docs/subagents.md`).
    /// With no view open the default type answers, which is what a launch
    /// with no `subagent_type` gets.
    ///
    /// Run beside every `sync_backend_info` and on entering a view: both are
    /// per type — a type whose `tools:` withholds `Skill` is briefed with
    /// nothing — so a single startup read would show the wrong pair for every
    /// other type.
    pub(crate) fn sync_agent_view_context(&mut self) {
        let agent_type = self.app.viewed_agent().map_or_else(
            || alter_zero::agents::GENERAL_PURPOSE.to_string(),
            |agent| agent.agent_type.clone(),
        );
        let prompt = self.models.backend().agent_system_prompt(&agent_type);
        let briefing = self.models.backend().agent_system_reminder(&agent_type);
        self.app.set_agent_system_prompt(prompt);
        self.app.set_agent_system_reminder(briefing);
        // The view's footer, too, is the viewed type's: the model it runs on
        // when its definition pins one, and the window its context gauge runs
        // against — the session's for an inheriting type, unknown (no gauge)
        // for a pinned one (`docs/agent-context-gauge.md`).
        let pinned = self.models.backend().agent_model(&agent_type);
        let window = self.models.agent_context_window(pinned.is_none());
        self.app.set_agent_context_window(window);
        self.app.set_agent_model(pinned);
    }

    /// Render the session's `<system-reminder>` **listing sections** into
    /// `App` (`subagents::listing_sections` → `App::listings`), so the one
    /// block the derived context leads with carries them behind the
    /// project's AGENTS.md instructions (`docs/skills.md`,
    /// `docs/subagents.md`; the wrapping is `context::context_messages_full`'s).
    ///
    /// Two sections, gated separately because they are two different tools:
    ///
    /// - **skills** — `None` when no skill loaded, the `/settings` **Skills**
    ///   row is off, **or tools are off at all**: the listing and the tool set
    ///   share one gate (`skills_offered`), since a reminder naming a tool the
    ///   request never carries is worse than no reminder.
    /// - **agent types** — the same rule one tool over: they ride exactly when
    ///   the `agent` tool does (`agents_offered`). The offline dummy scripts
    ///   its agent demo rather than being handed a spec, so it sends no agent
    ///   section and an offline context is byte-identical to before the
    ///   definitions existed.
    ///
    /// The composer's `$` picker rides the skills gate: the **enabled**
    /// snapshot goes into `App` beside the reminder (`App::set_skills`), so
    /// the picker can only ever complete a mention the listing names and the
    /// registry's lookup will honour (`docs/skill-mentions.md`). Every site
    /// that changes what is offered — the per-turn rescan, a `/skills`
    /// toggle, the `/settings` Skills/Tools rows, a `/model` switch — already
    /// funnels through here, which is what keeps the two in step.
    pub(crate) fn sync_listings(&mut self) {
        // The budget is 1% of the model's context window in characters, so
        // this is re-rendered here rather than once at startup: a `/model`
        // switch to a roomier model widens both listings with it. **One**
        // budget for the whole reminder, spent skills-first and the remainder
        // to the agents (`subagents::agent_budget`) — one fragment, one 1%.
        let window = self
            .models
            .context_window()
            .and_then(|w| usize::try_from(w).ok());
        let budget = alter_zero::skills::listing_budget(window);
        let skills_offered = self.app.settings().skills_offered();
        let skills = if skills_offered {
            self.skill_registry.listing(budget)
        } else {
            String::new()
        };
        let agents = if self.models.agents_offered() {
            self.subagents
                .listing(alter_zero::subagents::agent_budget(budget, &skills))
        } else {
            String::new()
        };
        let listings = alter_zero::subagents::listing_sections(&skills, &agents);
        self.app
            .set_listings((!listings.is_empty()).then_some(listings));
        self.app.set_skills(if skills_offered {
            self.skill_registry.enabled()
        } else {
            Vec::new()
        });
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
        tiers: Option<ServiceTierSupport>,
    ) {
        self.models.cancel_model_fetch();
        // The picked entry's reasoning support seeds the Ctrl+T cycle at its
        // default mode (medium where offered) — docs/reasoning.md.
        let thinking = reasoning.map(|support| {
            let mode = support.default_mode();
            (support, mode)
        });
        // Its lanes seed `/fast` **unchosen**, so the new model's own catalog
        // default applies rather than a lane picked for the old one
        // (docs/fast-mode.md).
        let tiers: Option<Tiers> = tiers.map(|support| (support, None));
        if self.models.switch_to(
            provider,
            id,
            thinking.as_ref(),
            tiers.as_ref(),
            vision,
            context,
        ) {
            self.sync_backend_info();
            self.app.set_thinking(thinking);
            self.app.set_service_tier(tiers);
            self.toast(format!("Switched model to {id}"), ToastKind::Info);
        } else {
            // A subscription has no env var to "set" — you sign in to it, and
            // naming GITHUB_COPILOT_TOKEN would send the user looking for a
            // key to paste that does not exist (`docs/copilot.md`). A
            // host-configured provider never lands here: no key, no refusal.
            let fix = if self.models.is_subscription(provider) {
                "run /login and sign in".to_string()
            } else {
                format!("run /login to set {}", self.models.key_env(provider))
            };
            self.toast(format!("Can't switch to {id}: {fix}"), ToastKind::Error);
        }
    }

    /// Ctrl+T advanced the thinking mode (the pure state already moved —
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
        // Ctrl+T establishes nothing about the lanes: hand over what `App`
        // holds, and let `None` (probe pending) leave the saved blob alone
        // rather than record a "no lanes" this key never learned.
        let tiers = self.app_tiers();
        self.models
            .persist(thinking.as_ref(), tiers.as_ref().map(Some));
        self.toast(format!("Thinking: {}", mode.label()), ToastKind::Info);
    }

    /// The service tiers + chosen lane as `App` currently holds them — what
    /// the persist call records beside the selection (`docs/fast-mode.md`).
    fn app_tiers(&self) -> Option<Tiers> {
        self.app
            .service_tier
            .as_ref()
            .map(|state| (state.support.clone(), state.selected.clone()))
    }

    /// `/fast` switched the service tier (the pure state already moved —
    /// `docs/fast-mode.md`). Rebind the *next* turn's backend so the lane
    /// rides its request (the running turn streams on its own thread,
    /// untouched — the Ctrl+T pattern), persist the choice beside the model
    /// selection, and confirm with a transient toast naming the lane.
    pub(crate) fn set_service_tier(&mut self) {
        // The choice **resolved** against the model's own lanes — which is
        // what the wire sends: an explicit standard choice and a lane the
        // model turns out not to offer both come back `None`.
        let resolved = self.app.service_tier_for_request();
        self.models.rebind_service_tier(resolved);
        let tiers = self.app_tiers();
        let thinking = self
            .app
            .thinking
            .as_ref()
            .map(|t| (t.support.clone(), t.mode));
        // `/fast` only ever acts on a model whose lanes are known, so this
        // is definitive.
        self.models.persist(thinking.as_ref(), Some(tiers.as_ref()));
        // The toast names the lane's own word, so it reads the same as the
        // footer marker it just turned on or off.
        let state = match self.app.service_tier_label() {
            Some(label) => format!("Fast mode: on ({label} service tier)"),
            None => "Fast mode: off".to_string(),
        };
        self.toast(state, ToastKind::Info);
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
            && let Some((thinking, tiers)) = {
                // The raw choice as `App` holds it — see `apply_probe`.
                let chosen = self
                    .app
                    .service_tier
                    .as_ref()
                    .and_then(|state| state.selected.clone());
                self.models.apply_probe(provider, &entries, chosen)
            }
        {
            self.app.set_thinking(thinking);
            self.app.set_service_tier(tiers);
            let window = self.models.context_window();
            self.app.set_context_window(window);
            // An inheriting subagent's gauge shares that window
            // (`docs/agent-context-gauge.md`).
            self.sync_agent_view_context();
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
