//! The **Google Antigravity sign-in**: Google's OAuth PKCE flow on
//! Antigravity's own desktop client, the refresh-token store it fills, the
//! short-lived access token every request is minted with, and the account's
//! Cloud Code project every request names. See `docs/providers/antigravity.md`.
//!
//! The fifth sign-in keeps the other four's two-layer split exactly: a
//! long-lived token in the `.env` key store, exchanged (and cached in memory,
//! never written) for the short-lived credential the API takes. What it adds
//! is a **project**: Cloud Code Assist bills every request to the account's
//! Cloud Code project and names it in the body, so [`authorize`] answers with
//! the project beside the bearer — looked up once per process with
//! `loadCodeAssist`, and set up once per account with `onboardUser` at the
//! sign-in, the one place that may wait for it.
//!
//! # An unofficial route
//!
//! Google publishes no API for third-party clients to reach Antigravity's
//! quota. The client id below is Antigravity's own public desktop client, and
//! the `User-Agent` is the agy CLI's, because the backend gates its catalog
//! on it — the posture `docs/providers/chatgpt.md` takes with Codex's. No client
//! identity prompt is sent: the backend does not require one
//! (`docs/providers/antigravity.md` *Read this first*).

use std::collections::{BTreeMap, HashMap};
use std::io::Read;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::value::RawValue;
use serde_json::{Value, json};

use super::cloud_code::level_suffix;
use super::loopback::{self, CallbackServer, Expected, Pages};
use super::mcp::{form_encode, pkce_and_state};
use super::models::ModelEntry;
use super::reasoning::{ReasoningEffort, ReasoningSupport};
use super::{LlmError, Result};
use crate::stream::CancelToken;

/// Antigravity's desktop OAuth client id — the one every community
/// integration signs in with. There is no third-party registration for this
/// API, so a client signing a user in to their own account uses it.
pub const CLIENT_ID: &str =
    "1071006060591-tmhssin2h21lcre235vtolojh4g403ep.apps.googleusercontent.com";

/// Its secret. A desktop client's secret is not one — Google's installed-app
/// guidance treats it as public, and it ships inside the app — but the token
/// endpoint still wants it beside the PKCE verifier.
pub const CLIENT_SECRET: &str = "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";

/// Where the browser is sent to sign in.
const AUTHORIZE_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";

/// Google's OAuth host — both grants post to `{base}/token`.
const OAUTH_BASE: &str = "https://oauth2.googleapis.com";

/// Where the signed-in account's email is read.
const USERINFO_URL: &str = "https://www.googleapis.com/oauth2/v1/userinfo?alt=json";

/// What the sign-in asks for: the Hub client's own set. `cloud-platform` is
/// what lets the token call Cloud Code; the email is what the confirmation
/// names.
const SCOPES: &str = "https://www.googleapis.com/auth/cloud-platform \
                      https://www.googleapis.com/auth/userinfo.email \
                      https://www.googleapis.com/auth/userinfo.profile \
                      https://www.googleapis.com/auth/cclog \
                      https://www.googleapis.com/auth/experimentsandconfigs";

/// The loopback port Antigravity's own sign-in listens on — tried first. A
/// desktop client accepts any loopback port, so a busy one falls back to an
/// ephemeral port rather than failing the sign-in.
pub const PREFERRED_PORT: u16 = 51121;

/// The path the browser comes back to.
const CALLBACK_PATH: &str = "/oauth-callback";

/// The shared consumer project an account that names none of its own runs
/// on — what the agy CLI's consumer sign-in sends.
pub const DEFAULT_PROJECT: &str = "aicode-consumers";

/// The agy CLI release the `User-Agent` names. The backend serves its full
/// catalog only to a recent enough client, and answers a model it hides with
/// 404; `ALTER_ZERO_ANTIGRAVITY_USER_AGENT` replaces the whole string when
/// Google moves the gate.
const CLI_VERSION: &str = "1.4.0";

/// That release's changelist, which the `User-Agent` carries beside it.
const CLI_CHANGELIST: &str = "1000494485";

/// The environment variable the refresh token lives under — the same name
/// `providers.toml` gives the provider's `api_key_env`, pinned here because
/// the rotation write-back cannot ask the provider file.
pub const REFRESH_ENV_VAR: &str = "ANTIGRAVITY_REFRESH_TOKEN";

/// The variable naming a Cloud Code project to bill instead of the one the
/// account resolves to. Read at the boundary.
pub const PROJECT_ENV: &str = "ANTIGRAVITY_PROJECT_ID";

/// The variable that replaces the agy CLI `User-Agent`. Read at the boundary.
pub const USER_AGENT_ENV: &str = "ALTER_ZERO_ANTIGRAVITY_USER_AGENT";

/// The variable that moves Google's OAuth host (the token and userinfo
/// calls) — the smoke suite's stand-in. Read at the boundary.
pub const OAUTH_BASE_ENV: &str = "ALTER_ZERO_GOOGLE_OAUTH_BASE";

/// How long the sign-in page waits for the browser — the countdown it shows.
pub const AUTH_TIMEOUT: Duration = Duration::from_secs(600);

/// How long one token, userinfo or setup call may take.
const OP_TIMEOUT: Duration = Duration::from_secs(30);

/// How much of an access token's life is given back before it is re-minted,
/// so a request never leaves with a token that expires in flight.
const REFRESH_SKEW: Duration = Duration::from_secs(300);

/// The cache lifetime when the server omits the expiry.
const MIN_CACHE: Duration = Duration::from_secs(60);

/// The most a token, userinfo or setup answer may buffer.
const BODY_MAX_BYTES: u64 = 64 * 1024;

/// The most a catalog answer may buffer — every model's record, its quota
/// and its experiments.
const CATALOG_MAX_BYTES: u64 = 8 * 1024 * 1024;

/// How many times onboarding is asked whether it is done, and how far apart.
/// It only ever runs at the sign-in, on its cancellable worker.
const ONBOARD_ATTEMPTS: u32 = 10;
const ONBOARD_INTERVAL: Duration = Duration::from_secs(3);

// ---------------------------------------------------------------------------
// The flow's URLs and bodies (pure)
// ---------------------------------------------------------------------------

/// The loopback `redirect_uri` for a bound port.
#[must_use]
pub fn redirect_uri(port: u16) -> String {
    format!("http://localhost:{port}{CALLBACK_PATH}")
}

/// The authorize URL to send the user to. `access_type=offline` and
/// `prompt=consent` are both load-bearing: without them Google issues no
/// refresh token, and the session dies at the first expiry.
#[must_use]
pub fn authorize_url(challenge: &str, state: &str, redirect_uri: &str) -> String {
    let query = form_encode(&[
        ("client_id".to_string(), CLIENT_ID.to_string()),
        ("response_type".to_string(), "code".to_string()),
        ("redirect_uri".to_string(), redirect_uri.to_string()),
        ("scope".to_string(), SCOPES.to_string()),
        ("code_challenge".to_string(), challenge.to_string()),
        ("code_challenge_method".to_string(), "S256".to_string()),
        ("state".to_string(), state.to_string()),
        ("access_type".to_string(), "offline".to_string()),
        ("prompt".to_string(), "consent".to_string()),
    ]);
    format!("{AUTHORIZE_URL}?{query}")
}

/// The authorization-code grant's body — form-encoded, the client secret
/// beside the PKCE verifier.
#[must_use]
pub fn code_exchange_body(code: &str, verifier: &str, redirect_uri: &str) -> String {
    form_encode(&[
        ("client_id".to_string(), CLIENT_ID.to_string()),
        ("client_secret".to_string(), CLIENT_SECRET.to_string()),
        ("code".to_string(), code.to_string()),
        ("grant_type".to_string(), "authorization_code".to_string()),
        ("redirect_uri".to_string(), redirect_uri.to_string()),
        ("code_verifier".to_string(), verifier.to_string()),
    ])
}

/// The refresh grant's body — form-encoded too.
#[must_use]
pub fn refresh_body(refresh_token: &str) -> String {
    form_encode(&[
        ("client_id".to_string(), CLIENT_ID.to_string()),
        ("client_secret".to_string(), CLIENT_SECRET.to_string()),
        ("refresh_token".to_string(), refresh_token.to_string()),
        ("grant_type".to_string(), "refresh_token".to_string()),
    ])
}

/// The agy CLI's `User-Agent` for an OS and architecture as Rust names them
/// (`std::env::consts`) — the backend's own spellings, `darwin` and `amd64`.
#[must_use]
pub fn user_agent_for(os: &str, arch: &str) -> String {
    let os = match os {
        "macos" => "darwin",
        other => other,
    };
    let arch = match arch {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        other => other,
    };
    format!(
        "antigravity/cli/{CLI_VERSION} (aidev_client; os_type={os}; arch={arch}; \
         cl={CLI_CHANGELIST}; auth_method=consumer)"
    )
}

/// How long an access token with `expires_in` seconds of life may be cached:
/// `REFRESH_SKEW` off the end, and a short-lived token keeps at most half
/// its life rather than none.
#[must_use]
pub fn cache_lifetime(expires_in: Option<u64>) -> Duration {
    let Some(secs) = expires_in else {
        return MIN_CACHE;
    };
    let lifetime = Duration::from_secs(secs);
    lifetime
        .saturating_sub(REFRESH_SKEW)
        .max(MIN_CACHE.min(lifetime / 2))
}

/// What a token response carried. `refresh_token` is optional on a refresh —
/// Google re-sends one only when it rotated — so a missing one means "keep
/// the one you have".
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct TokenSet {
    #[serde(default)]
    pub access_token: Option<String>,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub expires_in: Option<u64>,
}

impl TokenSet {
    /// Parse a token response.
    ///
    /// # Errors
    /// A body that is not an object, or that carries no access token.
    pub fn parse(body: &str) -> Result<Self> {
        let parsed: Self =
            serde_json::from_str(body).map_err(|e| LlmError::Decode(e.to_string()))?;
        if parsed.access_token.as_ref().is_none_or(|t| t.is_empty()) {
            return Err(LlmError::Decode(
                "Google returned no access token".to_string(),
            ));
        }
        Ok(parsed)
    }
}

/// `loadCodeAssist`'s body: the IDE type alone. The `platform`/`pluginType`
/// values Gemini CLI sends are refused for an Antigravity account.
#[must_use]
pub fn load_code_assist_body() -> Value {
    json!({"metadata": {"ideType": "ANTIGRAVITY"}})
}

/// What `loadCodeAssist` said about the account.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CodeAssist {
    /// The account's Cloud Code project, when it has one.
    pub project: Option<String>,
    /// The tier to onboard an account with no project on: the allowed tier
    /// marked default, else the first allowed, else the current, else
    /// `free-tier`.
    pub onboard_tier: String,
    /// The plan's name: the paid tier's, since the current tier reads
    /// `free-tier` even on a paid account.
    pub plan: Option<String>,
}

/// Read a `loadCodeAssist` answer. Lenient: an unreadable answer is an
/// account with no project on the free tier, which onboarding then sets up.
#[must_use]
pub fn parse_code_assist(body: &str) -> CodeAssist {
    let value: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let tier_id = |tier: &Value| {
        tier.get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .map(str::to_string)
    };
    let tier_name = |key: &str| {
        value
            .get(key)
            .and_then(|tier| tier.get("name"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_string)
    };
    let allowed = value
        .get("allowedTiers")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let onboard_tier = allowed
        .iter()
        .find(|tier| tier.get("isDefault").and_then(Value::as_bool) == Some(true))
        .and_then(tier_id)
        .or_else(|| allowed.first().and_then(tier_id))
        .or_else(|| value.get("currentTier").and_then(tier_id))
        .unwrap_or_else(|| "free-tier".to_string());
    CodeAssist {
        project: project_of(&value),
        onboard_tier,
        plan: tier_name("paidTier").or_else(|| tier_name("currentTier")),
    }
}

/// The `cloudaicompanionProject` an answer names — a string, or an object
/// with an `id`.
fn project_of(value: &Value) -> Option<String> {
    let project = value.get("cloudaicompanionProject")?;
    project
        .as_str()
        .or_else(|| project.get("id").and_then(Value::as_str))
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
}

/// `onboardUser`'s body — snake_case, as the Hub sends it.
#[must_use]
pub fn onboard_body(tier: &str) -> Value {
    json!({
        "tier_id": tier,
        "metadata": {"ide_type": "ANTIGRAVITY", "ide_version": CLI_VERSION, "ide_name": "antigravity"}
    })
}

/// Read an `onboardUser` answer — a long-running operation: whether it is
/// done, and the project it set up when it says.
#[must_use]
pub fn parse_onboard(body: &str) -> (bool, Option<String>) {
    let value: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let done = value.get("done").and_then(Value::as_bool) == Some(true);
    let project = value.get("response").and_then(project_of);
    (done, project)
}

// ---------------------------------------------------------------------------
// The catalog (pure)
// ---------------------------------------------------------------------------

/// One model's quota as the catalog reports it.
#[derive(Debug, Clone, PartialEq)]
pub struct Quota {
    /// The share left, 0–1.
    pub remaining: f64,
    /// When it starts over (RFC 3339), when the record says.
    pub resets_at: Option<String>,
}

/// One `/model` row of the catalog: a model listed verbatim, or a family of
/// level siblings grouped under their base id.
#[derive(Debug, Clone, PartialEq)]
pub struct CatalogRow {
    /// The id the picker shows — the runtime id, or the base of a group.
    pub id: String,
    pub display_name: String,
    /// `Gemini`, `Claude` or `GPT-OSS` — the `/usage` page's grouping.
    pub family: &'static str,
    /// The smallest context window among the row's models.
    pub context: Option<u64>,
    /// `false` when any of its models takes no images.
    pub vision: Option<bool>,
    /// A group's levels, lowest first — the Ctrl+T ladder. Empty for a
    /// model listed verbatim.
    pub levels: Vec<ReasoningEffort>,
    /// Whether a model listed verbatim thinks.
    pub thinks: bool,
    /// The scarcest quota among its models.
    pub quota: Option<Quota>,
}

/// The catalog envelope: the `models` map, each record left unparsed until
/// it is read (`docs/architecture/memory.md` — a record carries experiments and prompts
/// this client never reads), and the backend's own curated list.
#[derive(Deserialize)]
struct CatalogBody<'a> {
    #[serde(default, borrow)]
    models: Option<BTreeMap<String, &'a RawValue>>,
    #[serde(default, rename = "agentModelSorts")]
    sorts: Vec<Sort>,
}

#[derive(Deserialize)]
struct Sort {
    #[serde(default)]
    groups: Vec<SortGroup>,
}

#[derive(Deserialize)]
struct SortGroup {
    #[serde(default, rename = "modelIds")]
    model_ids: Vec<String>,
}

/// The fields of one record this client reads.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Record {
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    max_tokens: Option<u64>,
    #[serde(default)]
    max_input_tokens: Option<u64>,
    #[serde(default)]
    supports_images: Option<bool>,
    #[serde(default)]
    supports_thinking: Option<bool>,
    #[serde(default)]
    is_internal: Option<bool>,
    #[serde(default)]
    disabled: Option<bool>,
    #[serde(default)]
    api_provider: Option<String>,
    #[serde(default)]
    quota_info: Option<QuotaInfo>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct QuotaInfo {
    #[serde(default)]
    remaining_fraction: Option<f64>,
    #[serde(default)]
    reset_time: Option<String>,
}

impl Record {
    fn window(&self) -> Option<u64> {
        self.max_input_tokens
            .filter(|n| *n > 0)
            .or(self.max_tokens.filter(|n| *n > 0))
    }

    /// The record's quota — one present with no fraction is spent.
    fn quota(&self) -> Option<Quota> {
        let info = self.quota_info.as_ref()?;
        Some(Quota {
            remaining: info.remaining_fraction.unwrap_or(0.0).clamp(0.0, 1.0),
            resets_at: info.reset_time.clone().filter(|t| !t.trim().is_empty()),
        })
    }
}

/// Whether an id names a model a chat can run: a runtime id's shape, and
/// not a router slot (`chat_…`, `tab_…`) or an image model.
fn is_chat_model(id: &str) -> bool {
    let lower = id.to_ascii_lowercase();
    let shaped = lower
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphanumeric())
        && lower
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'));
    shaped
        && !lower.starts_with("chat_")
        && !lower.starts_with("tab_")
        && !lower.starts_with("model_")
        && !lower.contains("image")
}

/// Whether a record is one this account may pick: not internal, not
/// disabled, not an image model by its name.
fn is_offered(record: &Record) -> bool {
    record.is_internal != Some(true)
        && record.disabled != Some(true)
        && record.api_provider.as_deref() != Some("API_PROVIDER_INTERNAL")
        && !record
            .display_name
            .as_deref()
            .is_some_and(|name| name.to_ascii_lowercase().contains("image"))
}

/// The family a model id belongs to, as `/usage` groups it.
fn family_of(id: &str) -> &'static str {
    let lower = id.to_ascii_lowercase();
    if lower.starts_with("claude-") {
        "Claude"
    } else if lower.starts_with("gpt-oss-") {
        "GPT-OSS"
    } else {
        "Gemini"
    }
}

/// A display name without a trailing level in parentheses —
/// `Gemini 3.8 Flash (High)` is `Gemini 3.8 Flash` for the group.
fn base_display_name(name: &str) -> String {
    let trimmed = name.trim();
    match trimmed.rfind(" (") {
        Some(open) if trimmed.ends_with(')') => trimmed[..open].trim_end().to_string(),
        _ => trimmed.to_string(),
    }
}

/// Read a `fetchAvailableModels` answer into `/model` rows.
///
/// The ids the backend's `agentModelSorts` names, in its order, when it
/// names any — it is how the backend retires a route that still answers but
/// fails — else every id; minus router slots, image models, internal and
/// disabled ones. Level siblings (`-low`/`-medium`/`-high`, two or more,
/// whose base the catalog does not list itself) are one row whose ladder is
/// exactly their levels, so the id a request reconstructs is always one the
/// catalog listed.
///
/// # Errors
/// A body with no `models` map — an error object, a stranger's answer.
pub fn catalog_rows(body: &str) -> Result<Vec<CatalogRow>> {
    let parsed: CatalogBody<'_> =
        serde_json::from_str(body).map_err(|e| LlmError::Decode(e.to_string()))?;
    let Some(models) = parsed.models else {
        return Err(LlmError::Decode("the catalog named no models".to_string()));
    };
    let curated: Vec<String> = parsed
        .sorts
        .iter()
        .flat_map(|sort| &sort.groups)
        .flat_map(|group| &group.model_ids)
        .cloned()
        .collect();
    let order: Vec<&String> = if curated.is_empty() {
        models.keys().collect()
    } else {
        let mut seen = std::collections::HashSet::new();
        curated
            .iter()
            .filter(|id| models.contains_key(*id) && seen.insert(id.as_str()))
            .collect()
    };
    // Each offered model's record, decoded one at a time.
    let mut offered: Vec<(String, Record)> = Vec::new();
    for id in order {
        if !is_chat_model(id) {
            continue;
        }
        let record: Record = models
            .get(id)
            .and_then(|raw| serde_json::from_str(raw.get()).ok())
            .unwrap_or_default();
        if is_offered(&record) {
            offered.push((id.clone(), record));
        }
    }
    let listed: std::collections::HashSet<&str> =
        offered.iter().map(|(id, _)| id.as_str()).collect();
    // The bases two or more level siblings share, none of them a listed id.
    let mut siblings: HashMap<&str, usize> = HashMap::new();
    for (id, _) in &offered {
        if let Some((base, _)) = level_suffix(id) {
            *siblings.entry(base).or_default() += 1;
        }
    }
    let grouped = |id: &str| -> Option<String> {
        let (base, _) = level_suffix(id)?;
        (siblings.get(base).copied().unwrap_or(0) >= 2 && !listed.contains(base))
            .then(|| base.to_string())
    };

    let mut rows: Vec<CatalogRow> = Vec::new();
    let mut members: Vec<Vec<usize>> = Vec::new();
    let mut position: HashMap<String, usize> = HashMap::new();
    for (index, (id, _)) in offered.iter().enumerate() {
        let key = grouped(id).unwrap_or_else(|| id.clone());
        match position.get(&key) {
            Some(&row) => members[row].push(index),
            None => {
                position.insert(key.clone(), rows.len());
                members.push(vec![index]);
                rows.push(CatalogRow {
                    id: key,
                    display_name: String::new(),
                    family: family_of(id),
                    context: None,
                    vision: None,
                    levels: Vec::new(),
                    thinks: false,
                    quota: None,
                });
            }
        }
    }
    for (row, indices) in rows.iter_mut().zip(&members) {
        let variants: Vec<&(String, Record)> = indices.iter().map(|&i| &offered[i]).collect();
        let is_group = row.id != variants[0].0;
        let first_name = variants[0]
            .1
            .display_name
            .clone()
            .filter(|n| !n.trim().is_empty())
            .unwrap_or_else(|| variants[0].0.clone());
        row.display_name = if is_group {
            base_display_name(&first_name)
        } else {
            first_name
        };
        row.context = variants.iter().filter_map(|(_, r)| r.window()).min();
        row.vision = if variants
            .iter()
            .any(|(_, r)| r.supports_images == Some(false))
        {
            Some(false)
        } else if variants
            .iter()
            .any(|(_, r)| r.supports_images == Some(true))
        {
            Some(true)
        } else {
            None
        };
        if is_group {
            let mut levels: Vec<ReasoningEffort> = variants
                .iter()
                .filter_map(|(id, _)| level_suffix(id).map(|(_, level)| level))
                .collect();
            levels.sort_by_key(|level| {
                ReasoningEffort::LADDER
                    .iter()
                    .position(|rung| rung == level)
                    .unwrap_or(usize::MAX)
            });
            levels.dedup();
            row.levels = levels;
        } else {
            let (id, record) = variants[0];
            // Claude takes a thinking budget only on its `-thinking` routes;
            // anything else thinks when its record says so.
            row.thinks = if row.family == "Claude" {
                id.to_ascii_lowercase().contains("thinking")
            } else {
                record.supports_thinking == Some(true)
            };
        }
        row.quota = variants
            .iter()
            .filter_map(|(_, r)| r.quota())
            .min_by(|a, b| {
                a.remaining
                    .total_cmp(&b.remaining)
                    .then_with(|| a.resets_at.cmp(&b.resets_at))
            });
    }
    Ok(rows)
}

/// The rows as `/model` picker entries: a group's ladder is its levels (no
/// Off — every rung is a model that thinks), a thinking model listed
/// verbatim thinks or not at all.
#[must_use]
pub fn model_entries(rows: &[CatalogRow], provider: &str) -> Vec<ModelEntry> {
    rows.iter()
        .map(|row| {
            let reasoning = if !row.levels.is_empty() {
                Some(ReasoningSupport {
                    efforts: row.levels.clone(),
                    can_disable: false,
                    default_effort: None,
                })
            } else {
                row.thinks.then(ReasoningSupport::default)
            };
            ModelEntry {
                id: row.id.clone(),
                provider: provider.to_string(),
                display_name: row.display_name.clone(),
                reasoning,
                vision: row.vision,
                context: row.context,
                service_tiers: Vec::new(),
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Refusals worth explaining (pure)
// ---------------------------------------------------------------------------

/// The `error` object of a Google error body — `{"error": {code, status,
/// message, details}}` — or the OAuth one, `{"error": "invalid_grant"}`.
fn error_parts(body: &str) -> (String, String, Vec<Value>) {
    let value: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let error = value.get("error").cloned().unwrap_or(Value::Null);
    if let Some(code) = error.as_str() {
        let description = value
            .get("error_description")
            .and_then(Value::as_str)
            .unwrap_or_default();
        return (code.to_string(), description.to_string(), Vec::new());
    }
    let status = error
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let details = error
        .get("details")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    (status, message, details)
}

/// The account-verification link a 403 carries, when it is Google's own —
/// `https://accounts.google.com/…` and nothing else, since the link is shown
/// to be opened and a body is no proof of where it came from.
#[must_use]
pub fn validation_url(body: &str) -> Option<String> {
    let (_, _, details) = error_parts(body);
    details.iter().find_map(|detail| {
        let url = detail
            .get("metadata")
            .and_then(|m| m.get("validation_url"))
            .and_then(Value::as_str)?;
        let parsed = reqwest::Url::parse(url).ok()?;
        (parsed.scheme() == "https" && parsed.host_str() == Some("accounts.google.com"))
            .then(|| url.to_string())
    })
}

/// When a spent quota starts over, as the message says it —
/// `Your quota will reset after 2h13m5s.`, `Resets in 3h.`
fn reset_after(message: &str) -> Option<String> {
    let lower = message.to_ascii_lowercase();
    let start = ["reset after ", "resets after ", "resets in ", "reset in "]
        .iter()
        .find_map(|marker| lower.find(marker).map(|at| at + marker.len()))?;
    let rest = &message[start..];
    // A comma or a line ends it, and so does a period — unless a digit
    // follows, which makes it a decimal point (`3.957s`).
    let end = rest
        .char_indices()
        .find(|&(at, c)| {
            c == ','
                || c == '\n'
                || (c == '.'
                    && !rest[at + 1..]
                        .chars()
                        .next()
                        .is_some_and(|next| next.is_ascii_digit()))
        })
        .map_or(rest.len(), |(at, _)| at);
    let when = rest[..end].trim();
    (!when.is_empty()).then(|| when.to_string())
}

/// Explain a sign-in, refresh, or request failure in terms the user can act
/// on (`docs/providers/antigravity.md` *When it doesn't work*).
#[must_use]
pub fn auth_advice(status: u16, body: &str) -> Option<String> {
    if let Some(url) = validation_url(body) {
        return Some(format!(
            "Google wants this account verified before it serves more requests — open {url} \
             in a browser, then try again."
        ));
    }
    let (code, message, _) = error_parts(body);
    let lower = format!("{code} {message} {body}").to_ascii_lowercase();
    if status == 401 || code == "invalid_grant" || code == "UNAUTHENTICATED" {
        return Some(
            "your Google sign-in is no longer valid — run /login and sign in to Google \
             Antigravity again."
                .to_string(),
        );
    }
    if lower.contains("no capacity available") || lower.contains("model_capacity_exhausted") {
        return Some(
            "Google has no capacity for this model right now — try again in a moment, or \
             pick another with /model."
                .to_string(),
        );
    }
    match status {
        403 => {
            let reason = message.trim();
            Some(if reason.is_empty() {
                "this Google account can't use Antigravity's models.".to_string()
            } else {
                format!("this Google account can't use Antigravity's models: {reason}")
            })
        }
        404 => Some(
            "this model isn't available to this account — pick another with /model.".to_string(),
        ),
        429 => {
            if lower.contains("rate limit")
                || lower.contains("rate_limit")
                || lower.contains("too many requests")
            {
                return Some(
                    "Google is rate-limiting this account — wait a moment and try again."
                        .to_string(),
                );
            }
            Some(match reset_after(&message) {
                Some(when) => format!(
                    "this model's Antigravity quota is used up — it resets after {when}. Pick \
                     another with /model meanwhile."
                ),
                None => "this model's Antigravity quota is used up — pick another with /model, \
                         or wait for it to reset (/usage shows when)."
                    .to_string(),
            })
        }
        _ => None,
    }
}

/// A Cloud Code refusal in the words of [`auth_advice`], when it has any —
/// every call this module makes outside the chat stream (whose refusals
/// `OpenAiClient::explain` words) answers through it, so a refused
/// `loadCodeAssist` or catalog never reaches the screen as raw JSON.
#[must_use]
pub fn explained(error: LlmError) -> LlmError {
    match error {
        LlmError::Api { status, body } => LlmError::Api {
            status,
            body: auth_advice(status, &body).unwrap_or(body),
        },
        other => other,
    }
}

// ---------------------------------------------------------------------------
// Boundary state: the overrides, the token cache, the project
// ---------------------------------------------------------------------------

fn setting(slot: &'static OnceLock<Mutex<Option<String>>>) -> Option<String> {
    slot.get_or_init(|| Mutex::new(None))
        .lock()
        .ok()
        .and_then(|value| value.clone())
}

fn set_setting(slot: &'static OnceLock<Mutex<Option<String>>>, value: String) {
    if let Ok(mut held) = slot.get_or_init(|| Mutex::new(None)).lock() {
        *held = Some(value);
    }
}

static OAUTH_BASE_OVERRIDE: OnceLock<Mutex<Option<String>>> = OnceLock::new();
static USER_AGENT_OVERRIDE: OnceLock<Mutex<Option<String>>> = OnceLock::new();
static PROJECT_OVERRIDE: OnceLock<Mutex<Option<String>>> = OnceLock::new();
static STORE_PATH: OnceLock<Mutex<Option<String>>> = OnceLock::new();

/// Point the token and userinfo calls somewhere other than Google — the
/// smoke suite's stand-in. Boundary-only; called once at startup.
pub fn set_oauth_base(base: impl Into<String>) {
    set_setting(
        &OAUTH_BASE_OVERRIDE,
        base.into().trim_end_matches('/').to_string(),
    );
}

/// Replace the agy CLI `User-Agent`. Boundary-only.
pub fn set_user_agent(agent: impl Into<String>) {
    set_setting(&USER_AGENT_OVERRIDE, agent.into());
}

/// Bill a Cloud Code project of the user's choosing instead of the one the
/// account resolves to. Boundary-only.
pub fn set_project(project: impl Into<String>) {
    set_setting(&PROJECT_OVERRIDE, project.into());
}

/// Tell this module where the `.env` key store lives, so a rotated refresh
/// token can be persisted. Boundary-only; called once at startup.
pub fn set_store_path(path: impl Into<PathBuf>) {
    set_setting(&STORE_PATH, path.into().display().to_string());
}

/// The `User-Agent` every Cloud Code request carries.
#[must_use]
pub fn user_agent() -> String {
    setting(&USER_AGENT_OVERRIDE)
        .unwrap_or_else(|| user_agent_for(std::env::consts::OS, std::env::consts::ARCH))
}

fn token_url() -> String {
    format!(
        "{}/token",
        setting(&OAUTH_BASE_OVERRIDE).unwrap_or_else(|| OAUTH_BASE.to_string())
    )
}

fn userinfo_url() -> String {
    setting(&OAUTH_BASE_OVERRIDE).map_or_else(
        || USERINFO_URL.to_string(),
        |base| format!("{base}/oauth2/v1/userinfo?alt=json"),
    )
}

/// What a request authenticates with.
#[derive(Debug, Clone)]
pub struct Access {
    /// The bearer the request carries.
    pub bearer: String,
    /// The Cloud Code project the request names.
    pub project: String,
    /// The plan's name, when `loadCodeAssist` said.
    pub plan: Option<String>,
}

#[derive(Debug, Clone)]
struct Cached {
    bearer: String,
    until: Instant,
}

/// Access tokens keyed by the refresh token they were minted from.
fn bearers() -> &'static Mutex<HashMap<String, Cached>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Cached>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The account set-up `loadCodeAssist` (or the sign-in's onboarding)
/// resolved, keyed by the refresh token — for the life of the process.
fn setups() -> &'static Mutex<HashMap<String, CodeAssist>> {
    static CACHE: OnceLock<Mutex<HashMap<String, CodeAssist>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// What each rotated refresh token became, so a config still holding the
/// old one presents the new.
fn rotations() -> &'static Mutex<HashMap<String, String>> {
    static ROTATIONS: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    ROTATIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Serializes cache misses, so concurrent rounds never refresh twice.
fn refresh_gate() -> &'static Mutex<()> {
    static GATE: Mutex<()> = Mutex::new(());
    &GATE
}

fn live_refresh_token(stored: &str) -> String {
    rotations()
        .lock()
        .ok()
        .and_then(|map| map.get(stored).cloned())
        .unwrap_or_else(|| stored.to_string())
}

fn cached_bearer(refresh_token: &str) -> Option<String> {
    let presented = live_refresh_token(refresh_token);
    let map = bearers().lock().ok()?;
    let entry = map.get(&presented)?;
    (entry.until > Instant::now()).then(|| entry.bearer.clone())
}

/// Drop the bearer cached for `refresh_token` — a sign-in that replaced it
/// must not be answered with what it replaced.
///
/// The account's **setup** (its project and plan) is kept: it is a fact
/// about the account rather than a credential that goes stale, and the
/// `/login` boundary forgets the very token a sign-in just stored — whose
/// setup that sign-in has only now found, onboarding the account to get it.
pub fn forget(refresh_token: &str) {
    let current = live_refresh_token(refresh_token);
    if let Ok(mut map) = rotations().lock() {
        map.retain(|alias, target| alias != refresh_token && *target != current);
    }
    if let Ok(mut map) = bearers().lock() {
        map.remove(refresh_token);
        map.remove(&current);
    }
}

/// The plan the account resolved to, once a request or the sign-in has
/// asked — the `/usage` page's heading.
#[must_use]
pub fn plan_of(refresh_token: &str) -> Option<String> {
    let key = live_refresh_token(refresh_token);
    setups().lock().ok()?.get(&key)?.plan.clone()
}

// ---------------------------------------------------------------------------
// The calls (boundary — real HTTP)
// ---------------------------------------------------------------------------

/// Read a response body, bounded.
fn read_body(resp: reqwest::blocking::Response, max: u64) -> String {
    let mut body = String::new();
    let _ = resp.take(max).read_to_string(&mut body);
    body
}

/// A non-2xx answer as the error the user reads.
fn failure(status: u16, body: &str) -> LlmError {
    let explained = auth_advice(status, body).unwrap_or_else(|| {
        let trimmed = body.trim();
        if trimmed.is_empty() {
            format!("Google answered {status}")
        } else {
            format!("Google answered {status}: {trimmed}")
        }
    });
    LlmError::Api {
        status,
        body: explained,
    }
}

fn post_form(url: &str, body: String) -> Result<TokenSet> {
    let client = super::http_client(OP_TIMEOUT)?;
    let resp = client
        .post(url)
        .header("content-type", "application/x-www-form-urlencoded")
        .header("accept", "application/json")
        .body(body)
        .send()
        .map_err(|e| LlmError::transport(&e))?;
    let status = resp.status().as_u16();
    let text = read_body(resp, BODY_MAX_BYTES);
    if !(200..300).contains(&status) {
        return Err(failure(status, &text));
    }
    TokenSet::parse(&text)
}

/// `POST {base}/v1internal:{method}` with the bearer and the agy CLI
/// `User-Agent`, answering the body.
fn post_internal(base: &str, method: &str, bearer: &str, body: &Value, max: u64) -> Result<String> {
    let client = super::http_client(OP_TIMEOUT)?;
    let url = format!("{}/v1internal:{method}", base.trim_end_matches('/'));
    let resp = client
        .post(url)
        .bearer_auth(bearer)
        .header("content-type", "application/json")
        .header("user-agent", user_agent())
        .json(body)
        .send()
        .map_err(|e| LlmError::transport(&e))?;
    let status = resp.status().as_u16();
    let text = read_body(resp, max);
    if !(200..300).contains(&status) {
        return Err(LlmError::Api { status, body: text });
    }
    Ok(text)
}

/// What the browser is told at the end of the sign-in.
const PAGES: Pages = Pages {
    received: "Signed in to Google Antigravity",
    restart: "/login",
};

/// The sign-in page's live state.
pub struct Signin {
    /// The URL to open — shown on the page and copyable.
    pub url: String,
    callback: CallbackServer,
    verifier: String,
    /// A value of its own, never the verifier: putting the verifier in the
    /// URL would publish the secret the challenge exists to keep.
    state: String,
    redirect_uri: String,
}

/// What a finished sign-in hands back: the refresh token to store, and what
/// the confirmation names.
#[derive(Debug, Clone)]
pub struct SignedIn {
    pub refresh_token: String,
    pub email: Option<String>,
    pub plan: Option<String>,
}

impl SignedIn {
    /// The confirmation's name for the seat: the account and its plan.
    #[must_use]
    pub fn label(&self) -> Option<String> {
        match (&self.email, &self.plan) {
            (Some(email), Some(plan)) => Some(format!("{email}, {plan}")),
            (Some(email), None) => Some(email.clone()),
            (None, Some(plan)) => Some(plan.clone()),
            (None, None) => None,
        }
    }
}

/// Bind the callback (Antigravity's own port first, any free one after) and
/// build the URL to send the user to.
///
/// # Errors
/// No random source for the PKCE verifier and the state, or no loopback port
/// at all.
pub fn begin_signin() -> Result<Signin> {
    let (verifier, challenge, state) = pkce_and_state().map_err(LlmError::Http)?;
    let callback =
        CallbackServer::bind(&[PREFERRED_PORT, 0], CALLBACK_PATH).map_err(LlmError::Http)?;
    let redirect_uri = redirect_uri(callback.port);
    Ok(Signin {
        url: authorize_url(&challenge, &state, &redirect_uri),
        callback,
        verifier,
        state,
        redirect_uri,
    })
}

/// Wait for the browser, redeem the code, then set the account up on `base`
/// — find its project, onboarding it when it has none — and name it.
///
/// # Errors
/// A cancelled or timed-out wait, a refused exchange, a response with no
/// refresh token, or an account Cloud Code refuses outright.
pub fn await_callback(signin: &Signin, base: &str, cancel: &CancelToken) -> Result<SignedIn> {
    let expected = Expected {
        state: &signin.state,
        issuer: None,
        iss_required: false,
    };
    let code = loopback::wait_for_code(
        &signin.callback,
        &expected,
        &PAGES,
        AUTH_TIMEOUT,
        None,
        cancel,
    )?;
    let tokens = post_form(
        &token_url(),
        code_exchange_body(&code, &signin.verifier, &signin.redirect_uri),
    )?;
    let refresh = tokens
        .refresh_token
        .clone()
        .filter(|t| !t.is_empty())
        .ok_or_else(|| {
            LlmError::Decode(
                "Google returned no refresh token — sign in again and allow offline access"
                    .to_string(),
            )
        })?;
    let bearer = tokens.access_token.clone().unwrap_or_default();
    if let Ok(mut map) = bearers().lock() {
        map.insert(
            refresh.clone(),
            Cached {
                bearer: bearer.clone(),
                until: Instant::now() + cache_lifetime(tokens.expires_in),
            },
        );
    }
    let email = fetch_email(&bearer);
    let setup = set_up_account(base, &bearer, cancel)?;
    let plan = setup.plan.clone();
    if let Ok(mut map) = setups().lock() {
        map.insert(refresh.clone(), setup);
    }
    Ok(SignedIn {
        refresh_token: refresh,
        email,
        plan,
    })
}

/// The account's set-up at the sign-in: `loadCodeAssist`, and — for an
/// account the backend has never set up — `onboardUser`, asked again until
/// it is done. A refusal of the account itself (401/403) ends the sign-in
/// with the reason; anything else leaves the project to the first request.
fn set_up_account(base: &str, bearer: &str, cancel: &CancelToken) -> Result<CodeAssist> {
    let mut assist = match post_internal(
        base,
        "loadCodeAssist",
        bearer,
        &load_code_assist_body(),
        BODY_MAX_BYTES,
    ) {
        Ok(body) => parse_code_assist(&body),
        Err(LlmError::Api { status, body }) if matches!(status, 401 | 403) => {
            return Err(failure(status, &body));
        }
        Err(LlmError::Cancelled) => return Err(LlmError::Cancelled),
        Err(_) => return Ok(CodeAssist::default()),
    };
    if assist.project.is_some() {
        return Ok(assist);
    }
    let body = onboard_body(&assist.onboard_tier);
    for attempt in 0..ONBOARD_ATTEMPTS {
        if cancel.is_cancelled() {
            return Err(LlmError::Cancelled);
        }
        if attempt > 0 {
            super::retry::sleep_cancellable(ONBOARD_INTERVAL, cancel);
            if cancel.is_cancelled() {
                return Err(LlmError::Cancelled);
            }
        }
        let Ok(answer) = post_internal(base, "onboardUser", bearer, &body, BODY_MAX_BYTES) else {
            break;
        };
        let (done, project) = parse_onboard(&answer);
        if done {
            assist.project = project;
            break;
        }
    }
    Ok(assist)
}

/// The signed-in account's email, best-effort.
fn fetch_email(bearer: &str) -> Option<String> {
    let client = super::http_client(OP_TIMEOUT).ok()?;
    let resp = client.get(userinfo_url()).bearer_auth(bearer).send().ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let body = read_body(resp, BODY_MAX_BYTES);
    let value: Value = serde_json::from_str(&body).ok()?;
    value
        .get("email")
        .and_then(Value::as_str)
        .filter(|e| !e.is_empty())
        .map(str::to_string)
}

/// The bearer and the project to authenticate a request on `base` with,
/// minted from `refresh_token` and cached. Boundary code — on a cold cache,
/// one token refresh and one `loadCodeAssist`.
///
/// # Errors
/// A refresh Google refused, a `loadCodeAssist` that failed, or a transport
/// failure.
pub fn authorize(refresh_token: &str, base: &str) -> Result<Access> {
    let bearer = bearer_for(refresh_token)?;
    let key = live_refresh_token(refresh_token);
    let known = setups().lock().ok().and_then(|map| map.get(&key).cloned());
    let setup = match known {
        Some(setup) => setup,
        None => {
            let body = post_internal(
                base,
                "loadCodeAssist",
                &bearer,
                &load_code_assist_body(),
                BODY_MAX_BYTES,
            )
            .map_err(explained)?;
            let setup = parse_code_assist(&body);
            if let Ok(mut map) = setups().lock() {
                map.insert(key, setup.clone());
            }
            setup
        }
    };
    let project = setting(&PROJECT_OVERRIDE)
        .or(setup.project)
        .unwrap_or_else(|| DEFAULT_PROJECT.to_string());
    Ok(Access {
        bearer,
        project,
        plan: setup.plan,
    })
}

/// The bearer for `refresh_token`: cached, else minted under the gate.
fn bearer_for(refresh_token: &str) -> Result<String> {
    if let Some(bearer) = cached_bearer(refresh_token) {
        return Ok(bearer);
    }
    let _gate = refresh_gate().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(bearer) = cached_bearer(refresh_token) {
        return Ok(bearer);
    }
    let presented = live_refresh_token(refresh_token);
    let started = Instant::now();
    let tokens = post_form(&token_url(), refresh_body(&presented))?;
    let bearer = tokens.access_token.clone().unwrap_or_default();
    let key = match tokens
        .refresh_token
        .filter(|t| !t.is_empty() && *t != presented)
    {
        Some(rotated) => {
            persist_refresh(&rotated, &[refresh_token, &presented]);
            if let Ok(mut map) = rotations().lock() {
                for target in map.values_mut() {
                    if *target == presented {
                        target.clone_from(&rotated);
                    }
                }
                map.insert(refresh_token.to_string(), rotated.clone());
                map.insert(presented.clone(), rotated.clone());
            }
            if let Ok(mut map) = setups().lock()
                && let Some(setup) = map.remove(&presented)
            {
                map.insert(rotated.clone(), setup);
            }
            rotated
        }
        None => presented,
    };
    if let Ok(mut map) = bearers().lock() {
        let now = Instant::now();
        map.retain(|_, entry| entry.until > now);
        if let Some(until) = started.checked_add(cache_lifetime(tokens.expires_in))
            && until > now
        {
            map.insert(
                key,
                Cached {
                    bearer: bearer.clone(),
                    until,
                },
            );
        }
    }
    Ok(bearer)
}

/// Write a rotated refresh token back into the `.env` store — only while the
/// store still holds this session's own chain (or nothing), so a sign-in
/// that landed meanwhile is never overwritten. Best-effort.
fn persist_refresh(rotated: &str, chain: &[&str]) {
    let Some(path) = setting(&STORE_PATH) else {
        return;
    };
    let _ = super::EnvFile::update_if(
        std::path::Path::new(&path),
        REFRESH_ENV_VAR,
        rotated,
        |held| held.is_none_or(|held| held.is_empty() || chain.contains(&held)),
    );
}

/// The catalog answer for the account — `fetchAvailableModels` for its
/// project. The `/model` listing and the `/usage` page both read it.
///
/// # Errors
/// A refusal, or a transport failure.
pub fn fetch_catalog(base: &str, bearer: &str, project: &str) -> Result<String> {
    post_internal(
        base,
        "fetchAvailableModels",
        bearer,
        &json!({"project": project}),
        CATALOG_MAX_BYTES,
    )
}

/// The account's email for the `/usage` heading, best-effort.
#[must_use]
pub fn account_email(bearer: &str) -> Option<String> {
    fetch_email(bearer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::reasoning::{ReasoningSupport, ThinkingMode};

    #[test]
    fn the_authorize_url_asks_for_offline_access_on_antigravitys_client() {
        let url = authorize_url("CHAL", "STATE", &redirect_uri(PREFERRED_PORT));
        assert!(
            url.starts_with("https://accounts.google.com/o/oauth2/v2/auth?"),
            "{url}"
        );
        let query = url.split_once('?').unwrap().1;
        let param = |key: &str| crate::llm::mcp::redirect_param(&format!("?{query}"), key);
        assert_eq!(param("client_id").as_deref(), Some(CLIENT_ID));
        assert_eq!(param("response_type").as_deref(), Some("code"));
        assert_eq!(
            param("redirect_uri").as_deref(),
            Some("http://localhost:51121/oauth-callback")
        );
        assert_eq!(param("code_challenge").as_deref(), Some("CHAL"));
        assert_eq!(param("code_challenge_method").as_deref(), Some("S256"));
        assert_eq!(param("state").as_deref(), Some("STATE"));
        // Without both of these Google issues no refresh token.
        assert_eq!(param("access_type").as_deref(), Some("offline"));
        assert_eq!(param("prompt").as_deref(), Some("consent"));
        let scope = param("scope").unwrap();
        for wanted in [
            "https://www.googleapis.com/auth/cloud-platform",
            "https://www.googleapis.com/auth/userinfo.email",
            "https://www.googleapis.com/auth/userinfo.profile",
            "https://www.googleapis.com/auth/cclog",
            "https://www.googleapis.com/auth/experimentsandconfigs",
        ] {
            assert!(
                scope.split(' ').any(|s| s == wanted),
                "{wanted} missing from {scope}"
            );
        }
        assert!(CLIENT_ID.ends_with(".apps.googleusercontent.com"));
        assert_eq!(PREFERRED_PORT, 51121);
    }

    #[test]
    fn both_grants_are_form_encoded_and_carry_the_client_secret() {
        let exchange = code_exchange_body("the code", "VER", "http://localhost:9/oauth-callback");
        let field =
            |body: &str, key: &str| crate::llm::mcp::redirect_param(&format!("?{body}"), key);
        assert_eq!(
            field(&exchange, "grant_type").as_deref(),
            Some("authorization_code")
        );
        assert_eq!(field(&exchange, "code").as_deref(), Some("the code"));
        assert_eq!(field(&exchange, "code_verifier").as_deref(), Some("VER"));
        assert_eq!(
            field(&exchange, "redirect_uri").as_deref(),
            Some("http://localhost:9/oauth-callback")
        );
        assert_eq!(field(&exchange, "client_id").as_deref(), Some(CLIENT_ID));
        assert_eq!(
            field(&exchange, "client_secret").as_deref(),
            Some(CLIENT_SECRET)
        );
        assert!(!CLIENT_SECRET.is_empty());

        let refresh = refresh_body("1//rt");
        assert_eq!(
            field(&refresh, "grant_type").as_deref(),
            Some("refresh_token")
        );
        assert_eq!(field(&refresh, "refresh_token").as_deref(), Some("1//rt"));
        assert_eq!(field(&refresh, "client_id").as_deref(), Some(CLIENT_ID));
        assert_eq!(
            field(&refresh, "client_secret").as_deref(),
            Some(CLIENT_SECRET)
        );
    }

    #[test]
    fn a_token_response_needs_an_access_token_and_may_omit_the_refresh_token() {
        let tokens =
            TokenSet::parse(r#"{"access_token":"ya29.x","expires_in":3599,"token_type":"Bearer"}"#)
                .unwrap();
        assert_eq!(tokens.access_token.as_deref(), Some("ya29.x"));
        assert_eq!(
            tokens.refresh_token, None,
            "a refresh keeps the token it has"
        );
        assert_eq!(tokens.expires_in, Some(3599));
        assert!(TokenSet::parse(r#"{"refresh_token":"1//x"}"#).is_err());
        assert!(TokenSet::parse("not json").is_err());
    }

    #[test]
    fn an_access_token_is_cached_for_its_life_less_five_minutes() {
        assert_eq!(cache_lifetime(Some(3599)), Duration::from_secs(3299));
        assert_eq!(
            cache_lifetime(None),
            Duration::from_secs(60),
            "no expiry: a minute"
        );
        assert_eq!(
            cache_lifetime(Some(120)),
            Duration::from_secs(60),
            "a short life keeps half of itself rather than none"
        );
    }

    #[test]
    fn the_user_agent_is_the_agy_clis_for_this_machine() {
        assert_eq!(
            user_agent_for("linux", "x86_64"),
            "antigravity/cli/1.4.0 (aidev_client; os_type=linux; arch=amd64; cl=1000494485; auth_method=consumer)"
        );
        assert!(user_agent_for("macos", "aarch64").contains("os_type=darwin; arch=arm64;"));
        assert!(user_agent_for("windows", "x86_64").contains("os_type=windows; arch=amd64;"));
    }

    #[test]
    fn load_code_assist_names_only_the_ide_type() {
        // `platform`/`pluginType` values are refused there.
        assert_eq!(
            load_code_assist_body(),
            serde_json::json!({"metadata": {"ideType": "ANTIGRAVITY"}})
        );
    }

    #[test]
    fn the_project_is_a_string_or_an_object_with_an_id() {
        assert_eq!(
            parse_code_assist(r#"{"cloudaicompanionProject":"proj-a","currentTier":{"id":"free-tier","name":"Free"}}"#)
                .project
                .as_deref(),
            Some("proj-a")
        );
        assert_eq!(
            parse_code_assist(r#"{"cloudaicompanionProject":{"id":"proj-b","name":"x"}}"#)
                .project
                .as_deref(),
            Some("proj-b")
        );
        assert_eq!(
            parse_code_assist(r#"{"cloudaicompanionProject":""}"#).project,
            None
        );
        assert_eq!(parse_code_assist("{}").project, None);
        assert_eq!(parse_code_assist("not json").project, None);
    }

    #[test]
    fn an_account_with_no_project_is_onboarded_on_its_default_tier() {
        let assist = parse_code_assist(
            r#"{"allowedTiers":[{"id":"legacy-tier"},{"id":"standard-tier","isDefault":true}],"currentTier":{"id":"free-tier"}}"#,
        );
        assert_eq!(
            assist.onboard_tier, "standard-tier",
            "the tier marked default"
        );
        assert_eq!(
            parse_code_assist(r#"{"allowedTiers":[{"id":"legacy-tier"}]}"#).onboard_tier,
            "legacy-tier",
            "else the first allowed"
        );
        assert_eq!(
            parse_code_assist(r#"{"currentTier":{"id":"g1-pro-tier"}}"#).onboard_tier,
            "g1-pro-tier",
            "else the current one"
        );
        assert_eq!(
            parse_code_assist("{}").onboard_tier,
            "free-tier",
            "else the free tier"
        );
        assert_eq!(
            onboard_body("free-tier"),
            serde_json::json!({
                "tier_id": "free-tier",
                "metadata": {"ide_type": "ANTIGRAVITY", "ide_version": "1.4.0", "ide_name": "antigravity"}
            })
        );
        assert_eq!(
            parse_onboard(
                r#"{"done":true,"response":{"cloudaicompanionProject":{"id":"proj-c"}}}"#
            ),
            (true, Some("proj-c".to_string()))
        );
        assert_eq!(parse_onboard(r#"{"name":"operations/1"}"#), (false, None));
        assert_eq!(parse_onboard(r#"{"done":true}"#), (true, None));
    }

    #[test]
    fn the_plan_is_the_paid_tier_since_the_current_one_reads_free_regardless() {
        assert_eq!(
            parse_code_assist(r#"{"currentTier":{"id":"free-tier","name":"Gemini Code Assist for individuals"},"paidTier":{"id":"g1-pro-tier","name":"Google AI Pro"}}"#)
                .plan
                .as_deref(),
            Some("Google AI Pro")
        );
        assert_eq!(
            parse_code_assist(r#"{"currentTier":{"id":"free-tier","name":"Free"}}"#)
                .plan
                .as_deref(),
            Some("Free")
        );
        assert_eq!(parse_code_assist("{}").plan, None);
    }

    /// A `fetchAvailableModels` answer in the backend's shape.
    fn catalog_body() -> String {
        serde_json::json!({
            "models": {
                "gemini-3.8-flash-low": {"displayName": "Gemini 3.8 Flash (Low)", "maxTokens": 1048576, "maxOutputTokens": 65536, "supportsImages": true, "supportsThinking": true, "quotaInfo": {"remainingFraction": 0.8, "resetTime": "2026-11-02T13:00:00Z"}},
                "gemini-3.8-flash-medium": {"displayName": "Gemini 3.8 Flash (Medium)", "maxTokens": 1048576, "supportsImages": true, "supportsThinking": true, "quotaInfo": {"remainingFraction": 0.6, "resetTime": "2026-11-02T12:00:00Z"}},
                "gemini-3.8-flash-high": {"displayName": "Gemini 3.8 Flash (High)", "maxInputTokens": 900000, "maxTokens": 1048576, "supportsImages": true, "supportsThinking": true},
                "claude-opus-4-6-thinking": {"displayName": "Claude Opus 4.6 (Thinking)", "maxTokens": 200000, "supportsImages": true, "supportsThinking": true, "quotaInfo": {"resetTime": "2026-11-02T18:00:00Z"}},
                "claude-sonnet-4-6": {"displayName": "Claude Sonnet 4.6", "maxTokens": 200000, "supportsThinking": true},
                "gpt-oss-120b-medium": {"displayName": "GPT-OSS 120B (Medium)", "maxTokens": 131072, "supportsImages": false, "supportsThinking": true},
                "gemini-3-pro-image": {"displayName": "Gemini 3 Pro Image", "maxTokens": 65536},
                "chat_20706": {"model": "MODEL_CHAT_20706"},
                "tab_flash_lite_preview": {"model": "MODEL_PLACEHOLDER_M9"},
                "gemini-internal-x": {"displayName": "Internal", "isInternal": true},
                "gemini-retired": {"displayName": "Retired", "disabled": true},
                "gemini-pro-agent": {"displayName": "Gemini Pro Agent", "maxTokens": 1048576}
            },
            "agentModelSorts": [{"displayName": "Recommended", "groups": [
                {"modelIds": ["gemini-3.8-flash-high", "gemini-3.8-flash-medium", "gemini-3.8-flash-low"]},
                {"modelIds": ["claude-opus-4-6-thinking", "claude-sonnet-4-6", "gpt-oss-120b-medium", "gemini-3-pro-image", "chat_20706"]}
            ]}]
        })
        .to_string()
    }

    #[test]
    fn the_catalog_lists_what_the_backend_curates_minus_routers_images_and_internals() {
        let rows = catalog_rows(&catalog_body()).unwrap();
        let ids: Vec<&str> = rows.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(
            ids,
            vec![
                "gemini-3.8-flash",
                "claude-opus-4-6-thinking",
                "claude-sonnet-4-6",
                "gpt-oss-120b-medium"
            ],
            "in the curated order; `gemini-pro-agent` is listed but not curated"
        );
    }

    #[test]
    fn level_siblings_are_one_row_with_a_ladder_and_the_strictest_facts() {
        let rows = catalog_rows(&catalog_body()).unwrap();
        let flash = &rows[0];
        assert_eq!(flash.display_name, "Gemini 3.8 Flash");
        assert_eq!(flash.family, "Gemini");
        assert_eq!(
            flash.levels,
            vec![
                ReasoningEffort::Low,
                ReasoningEffort::Medium,
                ReasoningEffort::High
            ]
        );
        assert_eq!(
            flash.context,
            Some(900000),
            "the smallest window among the variants"
        );
        assert_eq!(flash.vision, Some(true));
        assert_eq!(
            flash.quota,
            Some(Quota {
                remaining: 0.6,
                resets_at: Some("2026-11-02T12:00:00Z".to_string()),
            }),
            "the scarcest variant's share and its reset"
        );
    }

    #[test]
    fn claude_thinks_only_on_its_thinking_route_and_a_bare_quota_is_spent() {
        let rows = catalog_rows(&catalog_body()).unwrap();
        let opus = &rows[1];
        assert!(opus.thinks && opus.levels.is_empty());
        assert_eq!(opus.family, "Claude");
        assert_eq!(
            opus.quota.as_ref().map(|q| q.remaining),
            Some(0.0),
            "a quotaInfo with no fraction left is a spent one"
        );
        let sonnet = &rows[2];
        assert!(
            !sonnet.thinks,
            "a Claude route that takes no thinking budget"
        );
        let oss = &rows[3];
        assert_eq!(oss.family, "GPT-OSS");
        assert!(oss.thinks);
        assert_eq!(oss.vision, Some(false));
    }

    #[test]
    fn with_no_curation_every_listed_model_is_offered() {
        let body = serde_json::json!({"models": {
            "gemini-b": {"displayName": "B"},
            "gemini-a": {"displayName": "A", "maxTokens": 0}
        }})
        .to_string();
        let rows = catalog_rows(&body).unwrap();
        let ids: Vec<&str> = rows.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, vec!["gemini-a", "gemini-b"]);
        assert_eq!(rows[0].context, None, "a zero window is no window");
        assert!(
            catalog_rows(r#"{"error":{"message":"nope"}}"#).is_err(),
            "no models map is no catalog"
        );
    }

    #[test]
    fn a_lone_level_suffix_stays_part_of_its_id() {
        let body = serde_json::json!({"models": {
            "claude-opus-5-5-high": {"displayName": "Claude Opus 5.5 (High)"},
            "gemini-4-argon-low": {"displayName": "Gemini 4 Argon (Low)", "supportsThinking": true},
            "gemini-4-argon-high": {"displayName": "Gemini 4 Argon (High)", "supportsThinking": true},
            "gemini-3-flash": {"displayName": "Gemini 3 Flash"},
            "gemini-3-flash-high": {"displayName": "Gemini 3 Flash (High)"}
        }})
        .to_string();
        let rows = catalog_rows(&body).unwrap();
        let ids: Vec<&str> = rows.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(
            ids,
            vec![
                "claude-opus-5-5-high",
                "gemini-3-flash",
                "gemini-3-flash-high",
                "gemini-4-argon"
            ],
            "a base the catalog lists itself is never a group: the ids stay as listed"
        );
    }

    #[test]
    fn rows_become_picker_entries_with_their_ladders() {
        let rows = catalog_rows(&catalog_body()).unwrap();
        let entries = model_entries(&rows, "antigravity");
        assert_eq!(entries[0].id, "gemini-3.8-flash");
        assert_eq!(entries[0].provider, "antigravity");
        assert_eq!(entries[0].display_name, "Gemini 3.8 Flash");
        assert_eq!(entries[0].context, Some(900000));
        let ladder = entries[0].reasoning.clone().expect("a ladder");
        assert_eq!(
            ladder.modes(),
            vec![
                ThinkingMode::Effort(ReasoningEffort::Low),
                ThinkingMode::Effort(ReasoningEffort::Medium),
                ThinkingMode::Effort(ReasoningEffort::High),
            ],
            "no Off: every rung is a model that thinks"
        );
        assert_eq!(
            ladder.default_mode(),
            ThinkingMode::Effort(ReasoningEffort::Medium)
        );
        assert_eq!(
            entries[1].reasoning,
            Some(ReasoningSupport {
                efforts: Vec::new(),
                can_disable: false,
                default_effort: None,
            }),
            "a thinking route thinks, on or not at all"
        );
        assert_eq!(entries[2].reasoning, None);
        assert!(entries.iter().all(|e| e.service_tiers.is_empty()));
    }

    #[test]
    fn forgetting_a_token_drops_its_bearer_but_not_the_accounts_setup() {
        // The `/login` boundary forgets the token a sign-in just stored, so a
        // stale bearer can never answer for it — but the project the sign-in
        // set the account up on is a fact about the account, not a
        // credential, and the first request must not lose it.
        let token = "1//forget-test";
        if let Ok(mut map) = bearers().lock() {
            map.insert(
                token.to_string(),
                Cached {
                    bearer: "ya29.old".to_string(),
                    until: Instant::now() + Duration::from_secs(600),
                },
            );
        }
        if let Ok(mut map) = setups().lock() {
            map.insert(
                token.to_string(),
                CodeAssist {
                    project: Some("proj-kept".to_string()),
                    onboard_tier: "free-tier".to_string(),
                    plan: Some("Free".to_string()),
                },
            );
        }
        forget(token);
        assert_eq!(cached_bearer(token), None, "the bearer is gone");
        assert_eq!(plan_of(token).as_deref(), Some("Free"), "the setup is kept");
    }

    #[test]
    fn a_refusal_from_cloud_code_reaches_the_user_explained() {
        let refused = explained(LlmError::Api {
            status: 403,
            body: r#"{"error":{"code":403,"status":"PERMISSION_DENIED","message":"This account is not eligible."}}"#
                .to_string(),
        });
        assert!(
            matches!(&refused, LlmError::Api { status: 403, body } if body.contains("can't use Antigravity's models: This account is not eligible.")),
            "{refused:?}"
        );
        let unexplained = explained(LlmError::Api {
            status: 400,
            body: "Invalid JSON payload".to_string(),
        });
        assert!(
            matches!(&unexplained, LlmError::Api { status: 400, body } if body == "Invalid JSON payload"),
            "a refusal with nothing to add passes through: {unexplained:?}"
        );
        assert!(matches!(
            explained(LlmError::Cancelled),
            LlmError::Cancelled
        ));
    }

    #[test]
    fn a_verification_link_is_shown_only_when_it_is_googles_own() {
        let body = |url: &str| {
            serde_json::json!({"error": {"code": 403, "status": "PERMISSION_DENIED", "message": "Verify your account to continue.",
                "details": [{"@type": "type.googleapis.com/google.rpc.ErrorInfo", "reason": "VALIDATION_REQUIRED",
                    "metadata": {"validation_url": url}}]}})
            .to_string()
        };
        assert_eq!(
            validation_url(&body("https://accounts.google.com/signin/continue?x=1")).as_deref(),
            Some("https://accounts.google.com/signin/continue?x=1")
        );
        assert_eq!(
            validation_url(&body("https://evil.example/accounts.google.com")),
            None
        );
        assert_eq!(
            validation_url(&body("http://accounts.google.com/x")),
            None,
            "never over http"
        );
        let advice = auth_advice(403, &body("https://accounts.google.com/v")).unwrap();
        assert!(
            advice.contains("https://accounts.google.com/v") && advice.contains("verif"),
            "{advice}"
        );
    }

    #[test]
    fn the_refusals_a_user_can_act_on_are_explained() {
        let error = |code: u16, status: &str, message: &str| {
            serde_json::json!({"error": {"code": code, "status": status, "message": message}})
                .to_string()
        };
        let signed_out = auth_advice(
            400,
            r#"{"error":"invalid_grant","error_description":"Token has been expired or revoked."}"#,
        )
        .unwrap();
        assert!(signed_out.contains("/login"), "{signed_out}");
        assert!(
            auth_advice(401, &error(401, "UNAUTHENTICATED", "x"))
                .unwrap()
                .contains("/login")
        );
        let quota = auth_advice(
            429,
            &error(429, "RESOURCE_EXHAUSTED", "You have exhausted your capacity on this model. Your quota will reset after 2h13m5s."),
        )
        .unwrap();
        assert!(
            quota.contains("quota") && quota.contains("2h13m5s"),
            "{quota}"
        );
        let sentence = auth_advice(
            429,
            &error(
                429,
                "RESOURCE_EXHAUSTED",
                "Individual quota reached. Resets in 3h. Please wait.",
            ),
        )
        .unwrap();
        assert!(
            sentence.contains("resets after 3h. Pick another"),
            "{sentence}"
        );
        let fraction = auth_advice(
            429,
            &error(
                429,
                "RESOURCE_EXHAUSTED",
                "Your quota will reset after 3.957s.",
            ),
        )
        .unwrap();
        assert!(
            fraction.contains("resets after 3.957s. Pick another"),
            "{fraction}"
        );
        let rate = auth_advice(
            429,
            &error(
                429,
                "RESOURCE_EXHAUSTED",
                "Rate limit exceeded. Too many requests.",
            ),
        )
        .unwrap();
        assert!(rate.contains("rate-limiting"), "{rate}");
        let capacity = auth_advice(
            503,
            &error(
                503,
                "UNAVAILABLE",
                "No capacity available for model gemini-3.8-flash-medium on the server",
            ),
        )
        .unwrap();
        assert!(capacity.contains("capacity"), "{capacity}");
        let gone = auth_advice(
            404,
            &error(404, "NOT_FOUND", "Requested entity was not found."),
        )
        .unwrap();
        assert!(gone.contains("/model"), "{gone}");
        let denied = auth_advice(
            403,
            &error(
                403,
                "PERMISSION_DENIED",
                "The caller does not have permission",
            ),
        )
        .unwrap();
        assert!(
            denied.contains("The caller does not have permission"),
            "{denied}"
        );
        assert_eq!(
            auth_advice(
                400,
                &error(400, "INVALID_ARGUMENT", "Invalid JSON payload received.")
            ),
            None
        );
    }
}
