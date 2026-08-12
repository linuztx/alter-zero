//! What the boundary reads out of the environment before anything runs: the
//! provider table, the API-key store, the persisted `/model` selection, the
//! per-project permission rules, and where each of those files lives.
//!
//! Everything here is a *lookup* — resolve a path, read a file, merge the
//! process environment over it — with no state of its own, so the loop can
//! call any of it at any time. The rule the whole module follows is
//! dotenv's: a real process env var wins over a stored value, and an empty
//! value counts as unset (see [`resolve_env`]).
//!
//! The formats themselves are pure and live in the library (`llm::config`'s
//! `ProvidersFile`/`EnvFile`/`Settings`, `permission::PermissionsFile`); this
//! module owns only the filesystem and `std::env` side of them. Writes are
//! best-effort by design: a read-only home must never kill the TUI, so
//! [`save_settings`] / [`save_permissions`] swallow their errors.
//!
//! See `docs/llm.md`, `docs/permissions.md`, `docs/checkpoint.md`.

use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use alter_zero::app::ProviderChoice;
use alter_zero::checkpoint;
use alter_zero::llm::{
    self, EnvFile, ModelConfig, ProvidersFile, ReasoningSupport, Selection, Settings, ThinkingMode,
    ThinkingSettings, backend::DEFAULT_SYSTEM_PROMPT,
};
use alter_zero::permission::{PermissionRules, PermissionsFile};
use alter_zero::settings::SessionSettings;
use alter_zero::stream;

use super::host::{local_date, os_context};

/// Is the built-in dummy backend forced on? (`ALTER_ZERO_DUMMY` set to a truthy
/// value). Keeps `smoke.sh` — which sets nothing — on the dummy, and lets a
/// developer force it even with a key configured. See `docs/llm.md`.
pub(crate) fn dummy_forced() -> bool {
    std::env::var("ALTER_ZERO_DUMMY").ok().is_some_and(|v| {
        matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

/// Load the provider table: `ALTER_ZERO_PROVIDERS_FILE`, then `./providers.toml`,
/// then `~/.alter-zero/providers.toml`, else the built-in default. The first that
/// reads and parses wins; a malformed file falls through to the next. Boundary
/// code — env + filesystem. See `docs/llm.md`.
pub(crate) fn load_providers() -> ProvidersFile {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(path) = std::env::var_os("ALTER_ZERO_PROVIDERS_FILE") {
        candidates.push(PathBuf::from(path));
    }
    candidates.push(PathBuf::from("providers.toml"));
    if let Some(home) = std::env::var_os("HOME") {
        candidates.push(PathBuf::from(home).join(".alter-zero/providers.toml"));
    }
    for path in candidates {
        if let Ok(text) = std::fs::read_to_string(&path)
            && let Ok(file) = ProvidersFile::parse(&text)
        {
            return file;
        }
    }
    ProvidersFile::builtin()
}

/// The environment variable a provider's API key is read from (its `api_key_env`
/// or the sanitized `<ID>_API_KEY` default), for both the key lookup and the
/// "set X" hint.
pub(crate) fn key_env_name(providers: &ProvidersFile, provider: &str) -> String {
    providers.get(provider).map_or_else(
        || alter_zero::llm::config::default_key_env(provider),
        |p| p.key_env(provider),
    )
}

/// A value from the real process environment (which wins, dotenv-style) or the
/// loaded `.env` store, ignoring empty values.
pub(crate) fn resolve_env(env_file: &EnvFile, name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .filter(|v| !v.is_empty())
        .or_else(|| {
            env_file
                .get(name)
                .filter(|v| !v.is_empty())
                .map(str::to_string)
        })
}

/// Resolve a provider's API key: its own env var (process env then `.env`), else
/// the generic `ALTER_ZERO_API_KEY`. Empty values count as unset.
pub(crate) fn resolve_api_key(
    providers: &ProvidersFile,
    env_file: &EnvFile,
    provider: &str,
) -> Option<String> {
    resolve_env(env_file, &key_env_name(providers, provider))
        .or_else(|| resolve_env(env_file, "ALTER_ZERO_API_KEY"))
}

/// Build the resolved [`ModelConfig`] for a provider/model (with the resolved
/// key + temperature + thinking mode + vision support merged in), or `None`
/// when the provider isn't in the file. `thinking` is the mode riding the
/// request payload — `None` for a model with no reasoning (or a fetch that
/// doesn't care, like the `/models` listing). See `docs/reasoning.md`.
/// `vision` is the model's known image-input support — `Some(false)` makes
/// the backend degrade attachments instead of letting the provider fail the
/// turn; `None` = unknown, attach optimistically. See `docs/tools.md`.
pub(crate) fn model_config_for(
    providers: &ProvidersFile,
    env_file: &EnvFile,
    provider: &str,
    model: &str,
    temperature: Option<f32>,
    thinking: Option<ThinkingMode>,
    vision: Option<bool>,
) -> Option<ModelConfig> {
    let sel = Selection {
        provider_id: provider.to_string(),
        model: model.to_string(),
        api_key: resolve_api_key(providers, env_file, provider),
        temperature,
        thinking,
        vision,
        cache_key: Some(session_cache_key().to_string()),
    };
    providers.model_config(&sel)
}

/// The per-session cache-affinity key every backend build shares, minted once
/// per process (pid + startup time — unique enough for a routing hint whose
/// caches live minutes). Sent as `prompt_cache_key` (and OpenRouter's
/// `session_id`) so this session's requests keep landing on the same
/// provider/server and hitting its warm prompt cache; a fresh key on the next
/// run just means one cold request. Boundary code — the time read stays out
/// of the pure core (see `docs/prompt-caching.md`).
pub(crate) fn session_cache_key() -> &'static str {
    static KEY: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    KEY.get_or_init(|| {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        format!("alter-zero-{}-{now}", std::process::id())
    })
}

/// The provider rows the `/login` flow shows: every provider in the file, tagged
/// with its key env var and whether a key already resolves (the ✓). See
/// `docs/llm.md`.
pub(crate) fn provider_choices(
    providers: &ProvidersFile,
    env_file: &EnvFile,
) -> Vec<ProviderChoice> {
    providers
        .ids()
        .into_iter()
        .map(|id| {
            let name = providers
                .get(&id)
                .map_or_else(|| id.clone(), |p| p.name.clone());
            let env_var = key_env_name(providers, &id);
            let configured = resolve_api_key(providers, env_file, &id).is_some();
            ProviderChoice {
                id,
                name,
                env_var,
                configured,
            }
        })
        .collect()
}

/// The app's config home — where the `.env` key store and `config.json` live:
/// `ALTER_ZERO_CONFIG_DIR`, else `~/.alter-zero`, else `None` (no HOME and no
/// override, so file persistence is disabled). Matches where `providers.toml`
/// and the sessions dir already resolve. See `docs/llm.md`.
pub(crate) fn config_home() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("ALTER_ZERO_CONFIG_DIR") {
        return Some(PathBuf::from(dir));
    }
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".alter-zero"))
}

/// The `.env` key store path: `ALTER_ZERO_ENV_FILE`, else `{config_home}/.env`,
/// else `./.env` when there's no config home. Written by the `/login` flow.
/// Write the `.env` key store **owner-only**: the file holds plaintext API
/// keys, so it is created `0o600` — and a pre-existing file's mode is
/// tightened, since `mode()` only applies at creation — matching the
/// credential-file convention of gh/codex/Claude Code. On non-unix the plain
/// write applies.
pub(crate) fn write_key_store(path: &std::path::Path, contents: &str) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        file.write_all(contents.as_bytes())
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, contents)
    }
}

pub(crate) fn env_file_path() -> PathBuf {
    if let Some(path) = std::env::var_os("ALTER_ZERO_ENV_FILE") {
        return PathBuf::from(path);
    }
    config_home().map_or_else(|| PathBuf::from(".env"), |dir| dir.join(".env"))
}

/// Load the `.env` key store; an absent or unreadable file yields an empty one.
pub(crate) fn load_env_file(path: &Path) -> EnvFile {
    std::fs::read_to_string(path)
        .map(|text| EnvFile::parse(&text))
        .unwrap_or_default()
}

/// The persisted-settings path (`{config_home}/config.json`), or `None` when
/// there's no config home — persistence is then disabled. See `docs/llm.md`.
pub(crate) fn settings_file_path() -> Option<PathBuf> {
    config_home().map(|dir| dir.join("config.json"))
}

/// The persisted per-project permissions path
/// (`{config_home}/permissions.json`) — the "don't ask again" command rules
/// and the manual/edit mode, keyed by project directory — or `None` when
/// there's no config home (persistence is then disabled, and every session
/// starts asking afresh). See `docs/permissions.md`.
pub(crate) fn permissions_file_path() -> Option<PathBuf> {
    config_home().map(|dir| dir.join("permissions.json"))
}

/// Load the permissions file; an absent, unreadable, or corrupt file yields
/// the empty one (the `load_settings` posture — never block startup).
pub(crate) fn load_permissions(path: Option<&Path>) -> PermissionsFile {
    path.and_then(|p| std::fs::read_to_string(p).ok())
        .map(|text| PermissionsFile::parse(&text))
        .unwrap_or_default()
}

/// Persist the session's live rules as this project's entry. A
/// read-modify-write — the file is re-read first, so entries other projects
/// wrote meanwhile survive — and best-effort like `save_settings`: a write
/// failure is swallowed (a read-only home must never kill the TUI). See
/// `docs/permissions.md`.
pub(crate) fn save_permissions(path: Option<&Path>, project: &str, rules: &PermissionRules) {
    let Some(path) = path else {
        return;
    };
    let mut file = load_permissions(Some(path));
    file.record(project, rules);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, file.to_json());
}

/// Load the user's `hooks.json` (`docs/hooks.md`).
///
/// Returns the parsed file and, when something was wrong with it, the message
/// to raise. Deliberately **not** the `load_permissions` best-effort posture:
/// silently reading a typo'd hooks file as "no hooks" is how a user comes to
/// believe a guard is running when it is not, so a parse failure yields the
/// empty config *and* an error the boundary shows.
///
/// An absent file is not an error — most sessions have none.
pub(crate) fn load_hooks(path: Option<&Path>) -> (alter_zero::hooks::HooksFile, Option<String>) {
    let empty = alter_zero::hooks::HooksFile::default;
    let Some(path) = path else {
        return (empty(), None);
    };
    match std::fs::read_to_string(path) {
        Ok(text) => match alter_zero::hooks::HooksFile::parse(&text) {
            Ok(file) => (file, None),
            Err(err) => (
                empty(),
                Some(format!("hooks.json: {err} — hooks are off this session")),
            ),
        },
        // Missing is the normal case; unreadable is worth saying out loud.
        Err(err) if err.kind() == io::ErrorKind::NotFound => (empty(), None),
        Err(err) => (
            empty(),
            Some(format!("hooks.json: {err} — hooks are off this session")),
        ),
    }
}

/// Whether lifecycle hooks run at all this session — `ALTER_ZERO_HOOKS`, the
/// `ALTER_ZERO_PERMISSIONS` pattern: any falsy spelling turns the feature off
/// before a file is even read.
pub(crate) fn hooks_enabled() -> bool {
    env_flag("ALTER_ZERO_HOOKS")
}

/// The checkpoints root — `ALTER_ZERO_CHECKPOINTS_DIR` (the smoke test points
/// it at a temp dir, the `ALTER_ZERO_SESSIONS_DIR` pattern), else
/// `~/.alter-zero/checkpoints`. `None` (no HOME and no override) disables
/// checkpoints. Each working directory gets one isolated store under this root
/// ([`checkpoint::store_git_dir`]). See `docs/checkpoint.md`.
pub(crate) fn checkpoints_root() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("ALTER_ZERO_CHECKPOINTS_DIR") {
        return Some(PathBuf::from(dir));
    }
    config_home().map(|dir| dir.join("checkpoints"))
}

/// The platform temp directory (`$TMPDIR`) — `/tmp` under another name, and
/// on macOS a per-user path (`/var/folders/xx/yyy/T`) no fixed list can
/// predict. Fed to [`checkpoint::cwd_scope`] so launching *in* it is refused
/// while a `mktemp -d` project inside it still checkpoints.
pub(crate) fn tmp_dir() -> Option<PathBuf> {
    std::env::var_os("TMPDIR").map(PathBuf::from)
}

/// What one checkpoint snapshot may cost before the feature switches itself
/// off for the session — `ALTER_ZERO_CHECKPOINT_MAX_FILES` /
/// `ALTER_ZERO_CHECKPOINT_MAX_BYTES` over the defaults, each accepting a
/// plain count or a `k`/`m`/`g` suffix and each taking `0` as *no limit*.
/// See `docs/checkpoint.md`.
pub(crate) fn checkpoint_budget() -> checkpoint::SnapshotBudget {
    let limit = |name: &str, default: u64| {
        checkpoint::parse_size_limit(std::env::var(name).ok().as_deref(), default)
    };
    checkpoint::SnapshotBudget {
        max_files: limit(
            "ALTER_ZERO_CHECKPOINT_MAX_FILES",
            checkpoint::DEFAULT_MAX_FILES,
        ),
        max_bytes: limit(
            "ALTER_ZERO_CHECKPOINT_MAX_BYTES",
            checkpoint::DEFAULT_MAX_BYTES,
        ),
        max_time: checkpoint::DEFAULT_PROBE_TIME,
    }
}

/// Load the persisted `/model` selection; an absent, unreadable, or corrupt
/// file yields the default (all-unset) settings.
pub(crate) fn load_settings(path: Option<&Path>) -> Settings {
    path.and_then(|p| std::fs::read_to_string(p).ok())
        .map(|text| Settings::parse(&text))
        .unwrap_or_default()
}

/// Persist the chosen provider/model — plus the model's reasoning state, so
/// the Shift+Tab cycle needs no refetch next run (`docs/reasoning.md`), and
/// its image-input support, so the attachment gate needs no re-probe
/// (`docs/tools.md`) — to `config.json`, creating the config home first.
/// Best-effort — a write failure is swallowed (like the session recorder) so
/// it can never kill the TUI; a `None` path (no config home) no-ops. See
/// `docs/llm.md`.
pub(crate) fn save_settings(
    path: Option<&Path>,
    provider: &str,
    model: &str,
    thinking: Option<ThinkingSettings>,
    vision: Option<bool>,
    context: Option<u64>,
) {
    let Some(path) = path else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(
        path,
        Settings::for_selection(provider, model)
            .with_thinking(thinking)
            .with_vision(vision)
            .with_context(context)
            .to_json(),
    );
}

/// The [`ThinkingSettings`] blob recording a *definitively known* reasoning
/// state — a real state, or the "no thinking" marker for a model whose record
/// said so (so startup doesn't re-probe it). Use only when the support is
/// known; an unknown (probe pending/failed) should persist `None` instead.
pub(crate) fn thinking_settings_of(
    thinking: Option<&(ReasoningSupport, ThinkingMode)>,
) -> ThinkingSettings {
    match thinking {
        Some((support, mode)) => ThinkingSettings::from_state(support, *mode),
        None => ThinkingSettings::unsupported(),
    }
}

/// Does this session ask before a `write`/`edit`/`bash` runs? On by default;
/// disabled by a falsy `ALTER_ZERO_PERMISSIONS` (`0`/`false`/`no`/`off`), which
/// starts the session with no gate so every tool runs unasked — the pre-feature
/// behaviour. See `docs/permissions.md`.
pub(crate) fn permissions_enabled() -> bool {
    env_flag("ALTER_ZERO_PERMISSIONS")
}

/// A feature toggle read from the environment: **on** unless `name` is set to
/// one of `0`/`false`/`no`/`off` (case- and whitespace-insensitive). The one
/// grammar every `ALTER_ZERO_*` on/off flag uses.
fn env_flag(name: &str) -> bool {
    match std::env::var(name) {
        Ok(v) => !matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "0" | "false" | "no" | "off"
        ),
        Err(_) => true,
    }
}

/// An **explicitly set** `ALTER_ZERO_*` on/off flag, or `None` when the
/// variable is absent. The `/settings` seed needs the distinction [`env_flag`]
/// erases: an unset variable must leave the saved value alone, where a set one
/// overrides it for this run. See `docs/settings.md`.
fn env_flag_set(name: &str) -> Option<bool> {
    std::env::var(name).ok().map(|v| {
        !matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "0" | "false" | "no" | "off"
        )
    })
}

// ===== the `/settings` menu's persisted knobs (docs/settings.md) =====

/// The `/settings` file path (`{config_home}/settings.json`) — its own file
/// beside `config.json` (the `/model` selection) and `permissions.json` (the
/// per-project rules), so one feature's write can never clobber another's.
/// `None` when there's no config home; persistence is then disabled.
pub(crate) fn settings_json_path() -> Option<PathBuf> {
    config_home().map(|dir| dir.join("settings.json"))
}

/// The per-project skill on/off file — `{config_home}/skills.json`
/// (`docs/skills.md`). `None` (no config home) disables persistence: the
/// session's toggles still work, they just don't survive a restart.
pub(crate) fn skills_json_path() -> Option<PathBuf> {
    config_home().map(|dir| dir.join("skills.json"))
}

/// Read the skill on/off file. Best-effort like [`load_permissions`] — a
/// corrupt file reads as "nothing disabled" rather than costing the session
/// its skills.
pub(crate) fn load_skills_file(path: Option<&Path>) -> alter_zero::skills::SkillsFile {
    path.and_then(|p| std::fs::read_to_string(p).ok())
        .map(|text| alter_zero::skills::SkillsFile::parse(&text))
        .unwrap_or_default()
}

/// Persist this project's turned-off skills. A read-modify-write — the file is
/// re-read first, so entries other projects wrote meanwhile survive — and
/// best-effort like [`save_permissions`]: a write failure is swallowed, since
/// a read-only home must never kill the TUI.
pub(crate) fn save_skills_file(
    path: Option<&Path>,
    project: &str,
    disabled: &std::collections::BTreeSet<String>,
) {
    let Some(path) = path else {
        return;
    };
    let mut file = load_skills_file(Some(path));
    file.record(project, disabled);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, file.to_json());
}

/// The `/settings` knobs **as the file holds them** — no environment merged
/// in. Kept beside the live values so a save can be a read-modify-write that
/// never persists an override (`Session::saved_settings`, `docs/settings.md`).
/// An absent, unreadable or corrupt file yields the defaults
/// (`load_settings`'s posture — never block startup).
pub(crate) fn load_saved_settings(path: Option<&Path>) -> SessionSettings {
    path.and_then(|p| std::fs::read_to_string(p).ok())
        .map(|text| SessionSettings::parse(&text))
        .unwrap_or_default()
}

/// The saved knobs with the `ALTER_ZERO_*` overrides applied on top — the
/// app's standing precedence rule: **the environment wins**, per setting, and
/// only when it is actually set. This is what the session *runs* with;
/// [`load_saved_settings`]'s blob is what it *writes back* to.
pub(crate) fn apply_setting_overrides(mut settings: SessionSettings) -> SessionSettings {
    // `ALTER_ZERO_SHOW_THINKING` names the *shown* side; the row is its
    // inverse (docs/thinking-stream.md).
    if let Some(show) = env_flag_set("ALTER_ZERO_SHOW_THINKING") {
        settings.hide_thinking = !show;
    }
    if let Some(on) = env_flag_set("ALTER_ZERO_TOOLS") {
        settings.tools = on;
    }
    // Checkpoints keep their own (identical) pure predicate, so the one
    // grammar stays in the module that owns the feature.
    if let Ok(value) = std::env::var("ALTER_ZERO_CHECKPOINTS") {
        settings.checkpoints = alter_zero::checkpoint::enabled_by_env(Some(&value));
    }
    // The project-doc knob's env form is a byte budget, whose documented off
    // switch is `0` (docs/project-doc.md).
    if let Some(budget) = std::env::var("ALTER_ZERO_PROJECT_DOC_MAX_BYTES")
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
    {
        settings.project_docs = budget > 0;
    }
    if let Some(on) = env_flag_set("ALTER_ZERO_SKILLS") {
        settings.skills = on;
    }
    if let Some(t) = temperature() {
        settings.temperature = Some(t);
    }
    settings
}

/// Persist the `/settings` knobs. Best-effort like [`save_settings`] — a
/// read-only home must never kill the TUI — and a `None` path (no config home)
/// no-ops. See `docs/settings.md`.
pub(crate) fn save_session_settings(path: Option<&Path>, settings: &SessionSettings) {
    let Some(path) = path else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, settings.to_json());
}

/// The dummy's pre-stream pause, so the status indicator is visibly working
/// before the first chunk: [`stream::STARTUP_DELAY`] unless
/// `ALTER_ZERO_STARTUP_DELAY_MS` overrides it (the smoke test runs with a short
/// delay; one phase uses a longer one). A real backend's own first-token
/// latency replaces it.
pub(crate) fn startup_delay() -> Duration {
    std::env::var("ALTER_ZERO_STARTUP_DELAY_MS")
        .ok()
        .and_then(|ms| ms.parse::<u64>().ok())
        .map_or(stream::STARTUP_DELAY, Duration::from_millis)
}

/// The sampling temperature every request carries (`ALTER_ZERO_TEMPERATURE`),
/// or `None` to leave it to the provider's default.
pub(crate) fn temperature() -> Option<f32> {
    std::env::var("ALTER_ZERO_TEMPERATURE")
        .ok()
        .and_then(|t| t.trim().parse::<f32>().ok())
}

/// `ALTER_ZERO_STALL_MS` selects a test-only backend that ignores the cancel
/// for N ms — modelling a real network backend wedged in a blocking read
/// during the pre-first-token pause — so `scripts/smoke.sh` can prove an Esc
/// interrupt stays responsive even then. Never used in normal operation (it
/// preempts the real/dummy backend only when the env var is set). See
/// `docs/interrupt.md`.
pub(crate) fn stall_ms() -> Option<u64> {
    std::env::var("ALTER_ZERO_STALL_MS")
        .ok()
        .and_then(|ms| ms.parse::<u64>().ok())
}

/// `ALTER_ZERO_CONTEXT_WINDOW`: the model's context window, overriding
/// whatever the provider reports (and the only way to get a footer gauge on
/// the dummy). Zero — a window that would divide by nothing — is ignored. See
/// `docs/compact.md`.
pub(crate) fn context_window_override() -> Option<u64> {
    std::env::var("ALTER_ZERO_CONTEXT_WINDOW")
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .filter(|&w| w > 0)
}

/// The real backend's system prompt: the "Alter Zero" persona
/// (`prompts/alter_zero.md`) unless `ALTER_ZERO_SYSTEM_PROMPT` overrides it
/// (an empty value sends no system prompt at all — `with_system_prompt` drops
/// blanks). Either way we fold in the runtime environment — date, os, cwd —
/// so the agent has context awareness (`docs/environment.md`); the values are
/// gathered here at the boundary (the `set_clock` pattern), the assembly is
/// the pure `backend::augment_with_environment`. Resolved once at startup, so
/// every backend the loop rebuilds (a `/model` switch, a Shift+Tab thinking
/// change, the capability probe) inherits it by clone.
pub(crate) fn system_prompt(cwd: &Path) -> Option<String> {
    std::env::var("ALTER_ZERO_SYSTEM_PROMPT")
        .ok()
        .or_else(|| Some(DEFAULT_SYSTEM_PROMPT.to_string()))
        .map(|base| {
            llm::backend::augment_with_environment(
                &base,
                &local_date(),
                &os_context(),
                &cwd.display().to_string(),
            )
        })
}

/// Is the MCP feature on? On by default; a falsy `ALTER_ZERO_MCP` turns the
/// whole thing off — no connections, no tools, `/mcp` explains via toast
/// (`docs/mcp.md`).
pub(crate) fn mcp_enabled() -> bool {
    env_flag("ALTER_ZERO_MCP")
}

/// The **user** MCP config file: `ALTER_ZERO_MCP_FILE` (what makes a smoke
/// run hermetic — the project `.mcp.json` is still discovered), else
/// `{config_home}/mcp.json`.
pub(crate) fn mcp_user_file_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("ALTER_ZERO_MCP_FILE") {
        return Some(PathBuf::from(path));
    }
    config_home().map(|dir| dir.join("mcp.json"))
}

/// The OAuth token store (`docs/mcp.md`).
pub(crate) fn mcp_auth_path() -> Option<PathBuf> {
    config_home().map(|dir| dir.join("mcp-auth.json"))
}

/// One MCP timeout knob in milliseconds, defaulting when unset/unparseable.
fn mcp_timeout_ms(name: &str, default: std::time::Duration) -> std::time::Duration {
    std::env::var(name)
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|ms| *ms > 0)
        .map_or(default, std::time::Duration::from_millis)
}

/// The per-server connect budget (`ALTER_ZERO_MCP_STARTUP_TIMEOUT_MS`).
pub(crate) fn mcp_startup_timeout() -> std::time::Duration {
    mcp_timeout_ms(
        "ALTER_ZERO_MCP_STARTUP_TIMEOUT_MS",
        alter_zero::llm::mcp::DEFAULT_STARTUP_TIMEOUT,
    )
}

/// The per-call budget (`ALTER_ZERO_MCP_TOOL_TIMEOUT_MS`).
pub(crate) fn mcp_tool_timeout() -> std::time::Duration {
    mcp_timeout_ms(
        "ALTER_ZERO_MCP_TOOL_TIMEOUT_MS",
        alter_zero::llm::mcp::DEFAULT_TOOL_TIMEOUT,
    )
}
