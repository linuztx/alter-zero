//! What the boundary reads out of the environment before anything runs: the
//! provider table, the API-key store, the persisted `/model` selection,
//! `/settings` knobs and `/mascot`/`/spinner` looks (each per working
//! directory, `docs/per-directory-state.md`), the per-project permission
//! rules, and where each of those files lives.
//!
//! Everything here is a *lookup* — resolve a path, read a file, merge the
//! process environment over it — with no state of its own, so the loop can
//! call any of it at any time. The rule the whole module follows is
//! dotenv's: a real process env var wins over a stored value, and an empty
//! value counts as unset (see [`resolve_env`]).
//!
//! The formats themselves are pure and live in the library (`llm::config`'s
//! `ProvidersFile`/`EnvFile`/`Settings`, `settings::SettingsFile`,
//! `permission::PermissionsFile`); this module owns only the filesystem and
//! `std::env` side of them. Writes are best-effort by design: a read-only home
//! must never kill the TUI, so [`save_selection`] / [`save_setting`] /
//! [`save_permissions`] swallow their errors.
//!
//! See `docs/llm.md`, `docs/permissions.md`, `docs/checkpoint.md`.

use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use alter_zero::app::{KeyKind, Look, LookFile, ProviderChoice, SigninKind, SubscriptionChoice};
use alter_zero::checkpoint;
use alter_zero::llm::{
    self, AuthScheme, EnvFile, ModelConfig, ModelSelection, ProvidersFile, ReasoningSupport,
    Selection, Settings, ThinkingMode, ThinkingSettings, backend::DEFAULT_SYSTEM_PROMPT,
};
use alter_zero::permission::{PermissionRules, PermissionsFile};
use alter_zero::scratchpad;
use alter_zero::settings::{SessionSettings, SettingKey, SettingsFile};
use alter_zero::stream;

use super::host::{self, local_date, os_context};

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
/// key + temperature + thinking mode + vision support + context window
/// merged in), or `None` when the provider isn't in the file. `thinking` is
/// the mode riding the request payload — `None` for a model with no
/// reasoning (or a fetch that doesn't care, like the `/models` listing). See
/// `docs/reasoning.md`. `vision` is the model's known image-input support —
/// `Some(false)` makes the backend degrade attachments instead of letting
/// the provider fail the turn; `None` = unknown, attach optimistically. See
/// `docs/tools.md`. `context` is the window the session gauges against,
/// which the Ollama wire sends as `options.num_ctx` (`docs/ollama.md`).
///
/// A provider whose base is environment-configurable (`api_base_env` —
/// Ollama's `OLLAMA_HOST`) has that variable resolved here, the way the key
/// is: the process env first, then the `.env` store.
#[allow(clippy::too_many_arguments)] // the capability trio plus the window, resolved together
pub(crate) fn model_config_for(
    providers: &ProvidersFile,
    env_file: &EnvFile,
    provider: &str,
    model: &str,
    temperature: Option<f32>,
    thinking: Option<ThinkingMode>,
    vision: Option<bool>,
    context: Option<u64>,
) -> Option<ModelConfig> {
    let sel = Selection {
        provider_id: provider.to_string(),
        model: model.to_string(),
        api_key: resolve_api_key(providers, env_file, provider),
        temperature,
        thinking,
        vision,
        context,
        api_base: resolve_api_base(providers, env_file, provider),
        cache_key: Some(session_cache_key().to_string()),
    };
    providers.model_config(&sel)
}

/// A provider's environment-configured base (`api_base_env`), when the file
/// names such a variable and it resolves (process env, then `.env`). `None`
/// leaves the file's base in force.
pub(crate) fn resolve_api_base(
    providers: &ProvidersFile,
    env_file: &EnvFile,
    provider: &str,
) -> Option<String> {
    let var = providers.get(provider)?.api_base_env.as_deref()?;
    resolve_env(env_file, var)
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

/// The **API-key** rows the `/login` flow shows: every provider whose key is
/// pasted, tagged with its env var and whether one already resolves (the ✓).
/// A subscription provider is excluded — it is signed in to, not keyed, and
/// listing it beside the others would offer a key field for a flow that has
/// none. See `docs/llm.md` and `docs/copilot.md`.
pub(crate) fn provider_choices(
    providers: &ProvidersFile,
    env_file: &EnvFile,
) -> Vec<ProviderChoice> {
    choices_where(providers, env_file, |p| !p.auth.is_subscription())
}

/// **Every** provider, however it authenticates — what the `/model` picker
/// fetches from. Deliberately not [`provider_choices`]: that one answers
/// "which providers does `/login` offer a key field for?", and a signed-in
/// GitHub Copilot is exactly the provider that has models and no key field.
pub(crate) fn all_provider_choices(
    providers: &ProvidersFile,
    env_file: &EnvFile,
) -> Vec<ProviderChoice> {
    choices_where(providers, env_file, |_| true)
}

/// The shared build behind both: one row per provider the predicate keeps.
fn choices_where(
    providers: &ProvidersFile,
    env_file: &EnvFile,
    keep: impl Fn(&alter_zero::llm::config::Provider) -> bool,
) -> Vec<ProviderChoice> {
    providers
        .ids()
        .into_iter()
        .filter(|id| providers.get(id).is_some_and(&keep))
        .map(|id| {
            let provider = providers.get(&id);
            let name = provider.map_or_else(|| id.clone(), |p| p.name.clone());
            let keyed = resolve_api_key(providers, env_file, &id).is_some();
            // A provider that needs no key is configured by being *pointed
            // at* instead: its host variable resolving (`OLLAMA_HOST`, which
            // `/login` writes), or the environment naming it the active
            // provider outright (`docs/ollama.md`). Without that rule every
            // `/model` open would fetch from a server most users don't run
            // and paint its refusal in red.
            let host_var = provider
                .filter(|p| p.auth.key_optional())
                .and_then(|p| p.api_base_env.clone());
            let pointed_at = host_var
                .as_deref()
                .is_some_and(|var| resolve_env(env_file, var).is_some())
                || std::env::var("ALTER_ZERO_PROVIDER").ok().as_deref() == Some(id.as_str());
            let configured = keyed || pointed_at;
            // What `/login` asks for, and what its Enter saves: the host
            // for a host-configured provider, the key for everyone else.
            let (env_var, key_kind) = match host_var {
                Some(var) => (
                    var,
                    KeyKind::Host {
                        default: provider.map_or_else(String::new, |p| p.kwargs.api_base.clone()),
                    },
                ),
                None => (key_env_name(providers, &id), KeyKind::Secret),
            };
            // What the key step introduces the provider with, and where its
            // keys are made: both come from the file, so adding a provider is
            // still one `[providers.<id>]` block (`docs/llm.md`).
            let description = provider
                .and_then(|p| p.description.clone())
                .unwrap_or_default();
            let key_url = provider
                .and_then(|p| p.api_key_url.clone())
                .unwrap_or_default();
            ProviderChoice {
                id,
                name,
                env_var,
                configured,
                key_kind,
                description,
                key_url,
            }
        })
        .collect()
}

/// The **subscription** rows the `/login` flow shows: every provider whose
/// `auth` names a sign-in flow, described by the file and tagged with whether
/// a token already resolves. See `docs/copilot.md`.
pub(crate) fn subscription_choices(
    providers: &ProvidersFile,
    env_file: &EnvFile,
) -> Vec<SubscriptionChoice> {
    providers
        .providers
        .iter()
        .filter(|(_, p)| p.auth.is_subscription())
        .map(|(id, p)| SubscriptionChoice {
            id: id.clone(),
            name: p.name.clone(),
            description: p.description.clone().unwrap_or_default(),
            configured: resolve_api_key(providers, env_file, id).is_some(),
            kind: signin_kind(p.auth),
        })
        .collect()
}

/// Which sign-in page a scheme opens. The provider file decides, so a row's
/// page is never guessed from what the flow happens to have filled in yet.
fn signin_kind(auth: AuthScheme) -> SigninKind {
    match auth {
        // Both are a browser page with a link and a wait; only the constants
        // behind them differ (`docs/chatgpt.md`, `docs/claude.md`).
        AuthScheme::OpenAiChatGpt | AuthScheme::AnthropicConsole => SigninKind::BrowserLink,
        AuthScheme::GithubCopilot | AuthScheme::ApiKey | AuthScheme::OptionalKey => {
            SigninKind::DeviceCode
        }
    }
}

/// The app's config home — where the `.env` key store and `config.json` live:
/// `ALTER_ZERO_CONFIG_DIR`, else `~/.alter-zero`, else `None` (no HOME and no
/// override, so file persistence is disabled). Every per-user file resolves
/// under it — `providers.toml`, the `.env` key store, the checkpoints root and
/// the sessions root (`resume::sessions_root`) alike — so moving it moves the
/// whole state dir. See `docs/llm.md`.
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

/// The persisted `/model` selections' path (`{config_home}/config.json`), or
/// `None` when there's no config home — persistence is then disabled. See
/// `docs/llm.md`.
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

/// Whether the project-level `.alter-zero` config layer is discovered at all
/// (`docs/project-config.md`) — `ALTER_ZERO_PROJECT_CONFIG`, default on.
/// Falsy = no project hooks/MCP discovery, no pending toast, `/trust`
/// explains via toast — and the smoke suite's hermeticity switch, since its
/// phases run with cwd inside a real checkout.
pub(crate) fn project_config_enabled() -> bool {
    env_flag("ALTER_ZERO_PROJECT_CONFIG")
}

/// `{config_home}/trust.json` — the per-project trust store
/// (`docs/project-config.md`). No override var of its own (the
/// `permissions.json` posture): `ALTER_ZERO_CONFIG_DIR` moves it.
pub(crate) fn trust_json_path() -> Option<PathBuf> {
    config_home().map(|dir| dir.join("trust.json"))
}

/// Load `trust.json` — the [`load_hooks`] posture, for the same reason: a
/// guard file that silently reads as "nothing trusted" (or worse) must say
/// so. Missing is the normal case; malformed/unreadable **fails closed**
/// (nothing trusted) with the message to raise.
pub(crate) fn load_trust_file(
    path: Option<&Path>,
) -> (alter_zero::trust::TrustFile, Option<String>) {
    let empty = alter_zero::trust::TrustFile::default;
    let Some(path) = path else {
        return (empty(), None);
    };
    match std::fs::read_to_string(path) {
        Ok(text) => match alter_zero::trust::TrustFile::parse(&text) {
            Ok(file) => (file, None),
            Err(err) => (
                empty(),
                Some(format!(
                    "trust.json: {err} — project config stays untrusted this session"
                )),
            ),
        },
        Err(err) if err.kind() == io::ErrorKind::NotFound => (empty(), None),
        Err(err) => (
            empty(),
            Some(format!(
                "trust.json: {err} — project config stays untrusted this session"
            )),
        ),
    }
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

// ===== the session's temp layout (docs/scratchpad.md) =====

/// This session's temp root — `{temp}/alter-zero-{uid}/{session}`, the parent
/// of both the agent's scratchpad and the background shells' `tasks` dir. The
/// uid and session id are the boundary's (`host::process_uid`/`session_id`),
/// the shape is the pure [`scratchpad::session_root`]; `TMPDIR` moves the
/// whole tree, since [`std::env::temp_dir`] honours it.
pub(crate) fn session_tmp_root(session: &str) -> PathBuf {
    scratchpad::session_root(&std::env::temp_dir(), host::process_uid(), session)
}

/// Is the scratchpad on? On by default; a falsy `ALTER_ZERO_SCRATCHPAD` turns
/// it off entirely — no directory, no `## Scratchpad` block in the system
/// prompt, and no permission exemption (`docs/scratchpad.md`).
pub(crate) fn scratchpad_enabled() -> bool {
    env_flag("ALTER_ZERO_SCRATCHPAD")
}

/// The session's scratchpad directory, **created**: `ALTER_ZERO_SCRATCHPAD_DIR`
/// puts it at an exact path (the `ALTER_ZERO_SKILLS_DIR` convention), else
/// `{session_root}/scratchpad`.
///
/// `None` means the agent is told about no scratchpad at all: the feature is
/// off, or the directory could not be created. Pointing the model at a path
/// that does not exist — and refusing its writes there in the same breath —
/// is worse than saying nothing, so the whole feature hangs off this one
/// answer (`docs/scratchpad.md`).
pub(crate) fn prepare_scratchpad(session_root: &Path) -> Option<PathBuf> {
    if !scratchpad_enabled() {
        return None;
    }
    let dir = std::env::var_os("ALTER_ZERO_SCRATCHPAD_DIR")
        .map_or_else(|| scratchpad::scratchpad_dir(session_root), PathBuf::from);
    std::fs::create_dir_all(&dir).ok().map(|()| dir)
}

/// Where this session's **pasted images** are saved: `{config_home}/image-cache/{session}`
/// (`clipboard::paste_store_dir`, docs/image-paste.md) — under the config
/// home rather than `/tmp`, so a picture pasted today is still there for a
/// `/resume` tomorrow. With no config home at all (no `HOME`, no override)
/// the same layout under the system temp dir. Created by the paste worker,
/// not here: a session that never pastes never makes the folder.
pub(crate) fn paste_store_dir(session: &str) -> PathBuf {
    match config_home() {
        Some(home) => alter_zero::clipboard::paste_store_dir(&home, session),
        None => std::env::temp_dir()
            .join("alter-zero-image-cache")
            .join(session),
    }
}

/// Read `ALTER_ZERO_IMAGE_CACHE_MAX_BYTES`, else
/// [`clipboard::IMAGE_CACHE_MAX_BYTES`]; `0` means no limit.
fn image_cache_cap() -> u64 {
    std::env::var("ALTER_ZERO_IMAGE_CACHE_MAX_BYTES")
        .ok()
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .unwrap_or(alter_zero::clipboard::IMAGE_CACHE_MAX_BYTES)
}

/// Delete the paste folders the image cache can no longer afford
/// (`clipboard::evictable_paste_dirs` — oldest first, empty ones always,
/// never the live session's `keep`), so the store a `/resume` reads from
/// stays bounded without an age rule that would drop a conversation still
/// worth resuming (`docs/image-paste.md`).
///
/// Best-effort and **stat-only**: it sums each folder's file sizes without
/// reading a byte, so a store of a few hundred sessions costs milliseconds
/// at startup. A root that isn't there yet is nothing to sweep.
pub(crate) fn sweep_image_cache(keep: &Path) {
    let Some(root) = keep.parent() else {
        return;
    };
    let Ok(read) = std::fs::read_dir(root) else {
        return;
    };
    let entries: Vec<(PathBuf, u64, std::time::SystemTime)> = read
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|entry| {
            let dir = entry.path();
            let bytes = folder_bytes(&dir);
            let modified = entry
                .metadata()
                .and_then(|meta| meta.modified())
                .unwrap_or(std::time::UNIX_EPOCH);
            (dir, bytes, modified)
        })
        .collect();
    for stale in alter_zero::clipboard::evictable_paste_dirs(&entries, image_cache_cap(), keep) {
        let _ = std::fs::remove_dir_all(stale);
    }
}

/// The bytes `dir`'s own files occupy — one `read_dir` and a `stat` each, no
/// recursion (a paste folder holds pictures, not a tree).
fn folder_bytes(dir: &Path) -> u64 {
    std::fs::read_dir(dir).map_or(0, |read| {
        read.filter_map(Result::ok)
            .filter_map(|entry| entry.metadata().ok())
            .filter(std::fs::Metadata::is_file)
            .map(|meta| meta.len())
            .sum()
    })
}

/// The session's image-payload cache, **created**: `{session_root}/images`,
/// where the backend keeps the downscaled copy of every picture it sends so a
/// later turn re-sending an attachment reads a small file instead of decoding
/// the original again (`docs/images.md` "Memory"). `None` when it could not
/// be created — payloads are then rebuilt per turn, exactly as before the
/// cache existed. Not gated with the scratchpad: it is the backend's own
/// working space, never something the model is told about.
pub(crate) fn prepare_image_cache(session_root: &Path) -> Option<PathBuf> {
    let dir = scratchpad::images_dir(session_root);
    std::fs::create_dir_all(&dir).ok().map(|()| dir)
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

/// Load the persisted `/model` selections (`config.json`); an absent,
/// unreadable, or corrupt file yields the default (all-unset) settings.
pub(crate) fn load_settings(path: Option<&Path>) -> Settings {
    path.and_then(|p| std::fs::read_to_string(p).ok())
        .map(|text| Settings::parse(&text))
        .unwrap_or_default()
}

/// Write `config.json` whole, creating the config home first. Best-effort — a
/// write failure is swallowed (like the session recorder) so it can never
/// kill the TUI; a `None` path (no config home) no-ops. See `docs/llm.md`.
fn write_settings(path: Option<&Path>, settings: &Settings) {
    let Some(path) = path else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, settings.to_json());
}

/// The `/model` selection a session in `project` (the cwd, as `config.json`
/// keys it) starts with — the directory's own entry, or, launched in for the
/// first time, the last selection made anywhere, **pinned** as the
/// directory's own right here (`Settings::adopt`, the file written only when
/// that changed) so a later switch elsewhere never moves it. `None` when
/// nothing was ever chosen. See `docs/per-directory-state.md`.
pub(crate) fn adopt_selection(path: Option<&Path>, project: &str) -> Option<ModelSelection> {
    let mut saved = load_settings(path);
    if saved.adopt(project) {
        write_settings(path, &saved);
    }
    saved.project(project).cloned()
}

/// Persist a `/model` choice made in `project` — the pair plus the model's
/// reasoning state, so the Ctrl+T cycle needs no refetch next run
/// (`docs/reasoning.md`), its image-input support, so the attachment gate
/// needs no re-probe (`docs/tools.md`), and its context window
/// (`docs/compact.md`) — as the directory's entry **and** the last selection
/// made anywhere. A read-modify-write ([`save_permissions`]' pattern): the
/// file is re-read first, so entries other directories wrote meanwhile
/// survive. Best-effort like every write here.
pub(crate) fn save_selection(path: Option<&Path>, project: &str, selection: &ModelSelection) {
    let Some(path) = path else {
        return;
    };
    let mut saved = load_settings(Some(path));
    saved.record(project, selection);
    write_settings(Some(path), &saved);
}

/// Persist what Ctrl+T or the capability probe learned about the model
/// `project` already records (`Settings::record_capabilities` — the entry is
/// touched only when it names that same pair, so an env-overridden selection
/// never writes back: env always wins, never sticks). The same
/// read-modify-write as [`save_selection`].
pub(crate) fn save_capabilities(path: Option<&Path>, project: &str, selection: &ModelSelection) {
    let Some(path) = path else {
        return;
    };
    let mut saved = load_settings(Some(path));
    if saved.record_capabilities(project, selection) {
        write_settings(Some(path), &saved);
    }
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
/// beside `config.json` (the `/model` selections) and `permissions.json` (the
/// per-project rules), so one feature's write can never clobber another's;
/// like both of those it is keyed by working directory inside
/// (`docs/per-directory-state.md`). `None` when there's no config home;
/// persistence is then disabled.
pub(crate) fn settings_json_path() -> Option<PathBuf> {
    config_home().map(|dir| dir.join("settings.json"))
}

/// The per-project skill on/off file — `{config_home}/skills.json`
/// (`docs/skills.md`). `None` (no config home) disables persistence: the
/// session's toggles still work, they just don't survive a restart.
pub(crate) fn skills_json_path() -> Option<PathBuf> {
    config_home().map(|dir| dir.join("skills.json"))
}

/// The banner-mascot file — `{config_home}/mascot.json`, its own file beside
/// the rest (one file per feature that owns it, `docs/mascot.md`), keyed by
/// working directory inside (`docs/per-directory-state.md`). `None` (no
/// config home) disables persistence: the `/mascot` switch still works, it
/// just doesn't survive a restart.
pub(crate) fn mascot_json_path() -> Option<PathBuf> {
    config_home().map(|dir| dir.join("mascot.json"))
}

/// The spinner-style file — `{config_home}/spinner.json`, its own file like
/// `mascot.json` (one file per feature that owns it, `docs/spinner.md`) and
/// keyed by working directory the same way. `None` (no config home) disables
/// persistence: the `/spinner` switch still works, it just doesn't survive a
/// restart.
pub(crate) fn spinner_json_path() -> Option<PathBuf> {
    config_home().map(|dir| dir.join("spinner.json"))
}

/// Read one look's file whole — every directory's entry over the last choice
/// (`app::LookFile`). Best-effort like [`load_permissions`]: an absent,
/// unreadable or corrupt file reads as nothing chosen, and the session keeps
/// the catalog's default rather than failing startup.
fn load_look_file<T: Look>(path: Option<&Path>) -> LookFile<T> {
    path.and_then(|p| std::fs::read_to_string(p).ok())
        .map(|text| LookFile::parse(&text))
        .unwrap_or_default()
}

/// Write a look's file whole, creating the config home first. Best-effort
/// like [`save_settings`] — a read-only home must never kill the TUI.
fn write_look_file<T: Look>(path: &Path, file: &LookFile<T>) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, file.to_json());
}

/// The look (a mascot, a spinner style) a session in `project` (the cwd, as
/// the file keys it) starts with — the directory's own entry, or, launched
/// in for the first time, the last choice made anywhere, **pinned** as the
/// directory's own right here (`LookFile::adopt`, the file written only when
/// that changed) so a later choice elsewhere never moves it — the
/// [`adopt_selection`] rule. `None` when nothing was ever chosen: the
/// catalog's default. See `docs/per-directory-state.md`.
pub(crate) fn adopt_look<T: Look>(path: Option<&Path>, project: &str) -> Option<T> {
    let mut file = load_look_file::<T>(path);
    if file.adopt(project)
        && let Some(path) = path
    {
        write_look_file(path, &file);
    }
    file.choice_for(project)
}

/// Persist a look chosen in `project` as the directory's entry **and** the
/// last choice made anywhere. A read-modify-write ([`save_selection`]'s
/// pattern): the file is re-read first, so entries other directories wrote
/// meanwhile survive. Best-effort like every write here, and a `None` path
/// (no config home) no-ops.
pub(crate) fn save_look<T: Look>(path: Option<&Path>, project: &str, look: T) {
    let Some(path) = path else {
        return;
    };
    let mut file = load_look_file::<T>(Some(path));
    file.record(project, look);
    write_look_file(path, &file);
}

/// The colour-theme file — `{config_home}/theme.json`, its own file like
/// `mascot.json` and `spinner.json` (one file per feature that owns it,
/// `docs/theme.md`). `None` (no config home) disables persistence: the
/// `/theme` switch still works, it just doesn't survive a restart.
pub(crate) fn theme_json_path() -> Option<PathBuf> {
    config_home().map(|dir| dir.join("theme.json"))
}

/// The telemetry file — `{config_home}/telemetry.json`, its own file like
/// `theme.json` (one file per feature that owns it, `docs/telemetry.md`):
/// the install id, the on/off switch, the last delivered day and whether
/// the one-time notice has been shown. `None` (no config home) disables the
/// feature outright — nowhere to keep an id means no ping, since a fresh
/// random id per launch would count one person as many.
pub(crate) fn telemetry_json_path() -> Option<PathBuf> {
    config_home().map(|dir| dir.join(alter_zero::telemetry::TELEMETRY_FILE_NAME))
}

/// Read `telemetry.json`. Best-effort like [`load_theme`] — an absent,
/// unreadable or corrupt file reads as the defaults (on, no id yet), so a
/// bad file costs at most a fresh install id and a repeated notice.
pub(crate) fn load_telemetry_file(path: Option<&Path>) -> alter_zero::telemetry::TelemetryFile {
    path.and_then(|p| std::fs::read_to_string(p).ok())
        .map(|text| alter_zero::telemetry::TelemetryFile::parse(&text))
        .unwrap_or_default()
}

/// Change `telemetry.json` through `edit`, as a **read-modify-write** over
/// the file itself — re-read, edited, written back only when the edit
/// changed something — and return the file as it now stands. Every writer
/// goes through here (the id mint, the notice mark, the delivered day, the
/// `/settings` toggle), and all of them run on the loop thread, so two
/// writers can never interleave and one field's write can never clobber
/// another's. Best-effort like [`save_theme`]: a failed write is swallowed
/// (a read-only home must never kill the TUI) and a `None` path (no config
/// home) edits nothing — the feature is off there anyway
/// (`docs/telemetry.md`).
pub(crate) fn update_telemetry_file(
    path: Option<&Path>,
    edit: impl FnOnce(&mut alter_zero::telemetry::TelemetryFile),
) -> alter_zero::telemetry::TelemetryFile {
    let before = load_telemetry_file(path);
    let mut file = before.clone();
    edit(&mut file);
    if let Some(path) = path
        && file != before
    {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(path, file.to_json());
    }
    file
}

/// Where the daily ping goes: `ALTER_ZERO_TELEMETRY_URL` when set and
/// non-empty (a fork's own collector, the smoke suite's local stub), else
/// the built-in collector (`telemetry::DEFAULT_ENDPOINT`).
pub(crate) fn telemetry_endpoint() -> String {
    std::env::var(alter_zero::telemetry::ENDPOINT_ENV)
        .ok()
        .map(|url| url.trim().to_string())
        .filter(|url| !url.is_empty())
        .unwrap_or_else(|| alter_zero::telemetry::DEFAULT_ENDPOINT.to_string())
}

/// Whether the environment **forbids** telemetry this run — a falsy
/// `ALTER_ZERO_TELEMETRY`, or a set `DO_NOT_TRACK`.
///
/// This is what makes the opt-out *hard* rather than a mere seed
/// (`docs/telemetry.md`): the `/settings` row reports itself unavailable, so
/// it cannot be cycled back on, and `telemetry_active()` stays false however
/// the row is set. Every other `ALTER_ZERO_*` override only seeds its row —
/// but the others are preferences, and this one is a statement that nothing
/// should be sent. The asymmetry is deliberate and one-directional: an
/// environment that forces telemetry *on* leaves the row cyclable, because
/// turning it **off** must always be possible.
pub(crate) fn telemetry_forbidden_by_env() -> bool {
    telemetry_env_override() == Some(false)
}

/// What the environment says about telemetry for this run —
/// `ALTER_ZERO_TELEMETRY` in the app's on/off grammar, outranked by a set
/// `DO_NOT_TRACK` — or `None` to defer to `telemetry.json`
/// (`telemetry::enabled_by_env`, `docs/telemetry.md`).
fn telemetry_env_override() -> Option<bool> {
    let telemetry = std::env::var(alter_zero::telemetry::TELEMETRY_ENV).ok();
    let dnt = std::env::var(alter_zero::telemetry::DNT_ENV).ok();
    alter_zero::telemetry::enabled_by_env(telemetry.as_deref(), dnt.as_deref())
}

/// Read the saved theme. Best-effort like [`load_spinner`] — an absent,
/// unreadable, or corrupt file reads as `None` and the session keeps the
/// default theme rather than failing startup.
pub(crate) fn load_theme(path: Option<&Path>) -> Option<alter_zero::app::Theme> {
    path.and_then(|p| std::fs::read_to_string(p).ok())
        .as_deref()
        .and_then(alter_zero::app::parse_theme_file)
}

/// Persist the chosen theme. Best-effort like [`save_spinner`] — a
/// read-only home must never kill the TUI — and a `None` path no-ops.
pub(crate) fn save_theme(path: Option<&Path>, theme: alter_zero::app::Theme) {
    let Some(path) = path else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, alter_zero::app::theme_file_json(theme));
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

/// The `settings.json` file **as it holds them** — every directory's entry
/// over the seed, no environment merged in (`settings::SettingsFile`,
/// `docs/per-directory-state.md`). The caller picks the cwd's entry
/// (`SettingsFile::settings_for`) and merges the overrides over that
/// ([`apply_setting_overrides`]). An absent, unreadable or corrupt file yields
/// the defaults (`load_settings`'s posture — never block startup).
pub(crate) fn load_settings_file(path: Option<&Path>) -> SettingsFile {
    path.and_then(|p| std::fs::read_to_string(p).ok())
        .map(|text| SettingsFile::parse(&text))
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
    // `ALTER_ZERO_HOOKS` seeds the **Hooks** row the way `ALTER_ZERO_TOOLS`
    // seeds Tools — an override for this run, never saved (docs/hooks.md).
    // It used to be ANDed over the row instead, which could only ever turn
    // hooks *off*; with the row off by default it has to turn them on too.
    if let Some(on) = env_flag_set("ALTER_ZERO_HOOKS") {
        settings.hooks = on;
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
    // The Telemetry row: `ALTER_ZERO_TELEMETRY` (and the cross-tool
    // `DO_NOT_TRACK`, which outranks it) seed it for the run over the value
    // `telemetry.json` supplied — an override, never saved
    // (`docs/telemetry.md`).
    if let Some(on) = telemetry_env_override() {
        settings.telemetry = on;
    }
    settings
}

/// Persist one cycled `/settings` knob for `project` (the cwd, as the file
/// keys it): a read-modify-write over the file itself — re-read, the
/// directory's entry (else the seed) taking only `key`'s value from the
/// `live` blob (`SettingsFile::record_value`), written back — so an
/// `ALTER_ZERO_*` override merged in at startup never sticks and entries
/// other directories wrote meanwhile survive. Best-effort like
/// [`save_permissions`]: a write failure is swallowed (a read-only home must
/// never kill the TUI) and a `None` path (no config home) no-ops. See
/// `docs/settings.md`, `docs/per-directory-state.md`.
pub(crate) fn save_setting(
    path: Option<&Path>,
    project: &str,
    key: SettingKey,
    live: &SessionSettings,
) {
    let Some(path) = path else {
        return;
    };
    let mut file = load_settings_file(Some(path));
    file.record_value(project, key, live);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, file.to_json());
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
/// so the agent has context awareness (`docs/environment.md`), and — when the
/// session has one — the `## Scratchpad` block pointing every temporary file
/// at the session's own directory (`docs/scratchpad.md`), for an assembled
/// persona → environment → scratchpad. The values are
/// gathered here at the boundary (the `set_clock` pattern), the assembly is
/// the pure `backend::augment_with_environment`/`augment_with_scratchpad`.
/// Resolved once at startup, so
/// every backend the loop rebuilds (a `/model` switch, a Ctrl+T thinking
/// change, the capability probe) inherits it by clone.
pub(crate) fn system_prompt(cwd: &Path, scratchpad: Option<&Path>) -> Option<String> {
    std::env::var("ALTER_ZERO_SYSTEM_PROMPT")
        .ok()
        .or_else(|| Some(DEFAULT_SYSTEM_PROMPT.to_string()))
        .map(|base| {
            let base = llm::backend::augment_with_environment(
                &base,
                &local_date(),
                &os_context(),
                &cwd.display().to_string(),
            );
            match scratchpad {
                Some(dir) => {
                    llm::backend::augment_with_scratchpad(&base, &dir.display().to_string())
                }
                None => base,
            }
        })
}

/// The **runtime half** of [`system_prompt`] alone: the environment block and
/// — when the session has one — the scratchpad block, without the persona.
///
/// What a subagent definition whose body *replaces* the persona still carries
/// (`docs/subagents.md`): the date, the os, the cwd and where temporary files
/// go are facts about this session, and an agent that doesn't know them
/// writes into `/tmp` and guesses the year. Built from the same two pure
/// renderers `system_prompt` composes with, from the same boundary reads, so
/// the two can never describe different environments.
pub(crate) fn prompt_context(cwd: &Path, scratchpad: Option<&Path>) -> Option<String> {
    let environment =
        llm::backend::render_environment(&local_date(), &os_context(), &cwd.display().to_string());
    Some(match scratchpad {
        Some(dir) => format!(
            "{environment}\n\n{}",
            llm::backend::render_scratchpad(&dir.display().to_string())
        ),
        None => environment,
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

/// Sweep away the **retired** era cache (`{config_home}/mcp-era.json`).
/// Nothing reads it any more — the era and the protocol revision are
/// re-derived from the server at every connect (`docs/mcp.md`) — and a file
/// left behind implies a feature that no longer exists, with a stale
/// revision written in it. Best-effort: our own cache, our own to drop.
pub(crate) fn remove_retired_mcp_era_cache() {
    if let Some(path) = config_home().map(|dir| dir.join("mcp-era.json")) {
        let _ = std::fs::remove_file(path);
    }
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
