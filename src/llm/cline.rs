//! Cline as a **subscription**: the account sign-in the Cline extension and
//! CLI run, the refresh token it fills the `.env` store with, and the
//! short-lived access token every request is minted with. See `docs/cline.md`.
//!
//! The fourth sign-in, after GitHub Copilot's device flow (`docs/copilot.md`),
//! OpenAI's browser loopback and device code (`docs/chatgpt.md`) and
//! Anthropic's Console PKCE (`docs/claude.md`), and it keeps their two-layer
//! split exactly: a long-lived token in the `.env` key store, exchanged (and
//! cached in memory, never written) for the short-lived credential the API
//! actually takes.
//!
//! # The flow
//!
//! Cline signs users in through **WorkOS**, the identity provider behind
//! app.cline.bot, and the reference is Cline's own published client
//! (`sdk/packages/core/src/auth/cline.ts`); every constant below was read
//! from it:
//!
//! 1. **The device code** — `POST {auth}/user_management/authorize/device`
//!    with the public WorkOS client id and nothing else. The answer carries
//!    the pair the poll presents plus the short code the user confirms in a
//!    browser.
//! 2. **The poll** — `POST {auth}/user_management/authenticate` with
//!    `grant_type=urn:ietf:params:oauth:grant-type:device_code` until the
//!    user confirms the code, the code expires, or they deny it.
//! 3. **The registration** — the WorkOS tokens are not what the API takes:
//!    they are exchanged **once** at `POST {api}/api/v1/auth/register`,
//!    whose answer names the account (`userInfo.email`) and hands back the
//!    **Cline** refresh token that goes into the key store. Every request
//!    after that mints a short-lived access token at
//!    `POST {api}/api/v1/auth/refresh`.
//!
//! # The wire form
//!
//! The bearer is not the access token verbatim: Cline's API takes OAuth
//! access tokens prefixed `workos:` (`Authorization: Bearer workos:<jwt>`),
//! which its own client applies when a credential is read
//! ([`bearer_for`]) — while a pasted dashboard key rides **verbatim**,
//! prefixing one 401s, which is why the pasted-key provider next door
//! (`providers.toml`'s `cline`) sends its key straight through and only this
//! module prefixes.
//!
//! The pure halves (the response parses, the poll's verdict, the freshness
//! rule, the bearer's spelling, the advice) are unit-tested; the HTTP calls
//! are boundary code.

use std::collections::HashMap;
use std::io::Read;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::Deserialize;

use super::mcp::form_encode;
use super::{LlmError, Result};
use crate::stream::CancelToken;

/// The **public** WorkOS client id Cline's clients sign in with — the
/// production entry of `shared/src/runtime/cline-environment.ts` in Cline's
/// own repo. A device-code flow has no client secret, so this is all a
/// client needs; it is not interchangeable with the staging or local ids.
pub const CLIENT_ID: &str = "client_01K3A541FN8TA3EPPHTD2325AR";

/// Where the WorkOS sign-in calls go, unless [`AUTH_BASE_ENV`] says otherwise.
const DEFAULT_AUTH_BASE: &str = "https://api.workos.com";

/// Where the register/refresh calls go, unless [`API_BASE_ENV`] says
/// otherwise. The chat completions themselves follow `providers.toml`'s
/// `kwargs.api_base`, which names the same host.
const DEFAULT_API_BASE: &str = "https://api.cline.bot";

/// The environment variable that points the sign-in at another WorkOS host —
/// the smoke suite's local stand-in (Phase 136), or a fork's own. Read at
/// the boundary and handed in once through [`set_auth_base`], the
/// `set_issuer` pattern: the pure builders never read the environment.
pub const AUTH_BASE_ENV: &str = "ALTER_ZERO_CLINE_AUTH_BASE";

/// The environment variable that points register/refresh at another Cline
/// API host — Phase 136's stub again, or a fork's own.
pub const API_BASE_ENV: &str = "ALTER_ZERO_CLINE_API_BASE";

/// The paths the four calls live on.
const DEVICE_AUTHORIZE_PATH: &str = "/user_management/authorize/device";
const AUTHENTICATE_PATH: &str = "/user_management/authenticate";
const REGISTER_PATH: &str = "/api/v1/auth/register";
const REFRESH_PATH: &str = "/api/v1/auth/refresh";

/// RFC 8628's device-code grant type.
const DEVICE_GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";

/// The prefix Cline's API wants on an OAuth access token — and not on a
/// pasted dashboard key.
const WORKOS_PREFIX: &str = "workos:";

/// The fallback code life and poll cadence when a device-authorization
/// response names neither — WorkOS's own defaults, which every real answer
/// repeats.
const DEFAULT_EXPIRES: u64 = 300;
const DEFAULT_INTERVAL: u64 = 5;

/// What a `slow_down` poll adds to the interval. One second, Cline's own
/// reading of the hint — gentler than GitHub's five, and enough to keep the
/// poll honest.
const SLOW_DOWN_STEP: u64 = 1;

/// How much of an access token's life is given back before it is re-minted,
/// so a request never leaves with a token that expires in flight.
const REFRESH_SKEW: Duration = Duration::from_secs(300);

/// The fallback cache lifetime when nothing names an expiry. An explicit
/// short lifetime always wins: caching past it would keep sending a dead
/// bearer.
const MIN_CACHE: Duration = Duration::from_secs(60);

/// Per-operation deadline for the flow's requests — a small JSON round trip
/// each, with a user watching.
const OP_TIMEOUT: Duration = Duration::from_secs(30);

/// The most any of the flow's responses may buffer — the other sign-ins'
/// posture at the size these bodies actually are. It also bounds what a
/// *failure* can put on the page: an intercepting proxy answers a blocked
/// request with an HTML page, and that page would otherwise become the
/// "reason" the user reads.
const BODY_MAX_BYTES: u64 = 64 * 1024;

/// One nap between cancel checks while the poll waits.
const POLL_NAP: Duration = Duration::from_millis(200);

/// The environment variable the refresh token lives under — the same name
/// `providers.toml` gives the provider's `api_key_env`, pinned here because
/// the rotation write-back cannot ask the provider file.
pub const REFRESH_ENV_VAR: &str = "CLINE_ACCOUNT_REFRESH_TOKEN";

/// The two overridable bases — [`DEFAULT_AUTH_BASE`] and [`DEFAULT_API_BASE`]
/// until the boundary sets one.
fn auth_base_slot() -> &'static Mutex<Option<String>> {
    static SLOT: OnceLock<Mutex<Option<String>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

fn api_base_slot() -> &'static Mutex<Option<String>> {
    static SLOT: OnceLock<Mutex<Option<String>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

/// Point the sign-in's WorkOS calls at `url` instead of WorkOS itself. A
/// trailing slash is dropped so the paths built on it never double one up.
/// Boundary-only; called once at startup.
pub fn set_auth_base(url: impl Into<String>) {
    let url = url.into();
    let trimmed = url.trim().trim_end_matches('/').to_string();
    if let Ok(mut slot) = auth_base_slot().lock() {
        *slot = (!trimmed.is_empty()).then_some(trimmed);
    }
}

/// Point register/refresh at `url` instead of Cline's API. Same contract as
/// [`set_auth_base`].
pub fn set_api_base(url: impl Into<String>) {
    let url = url.into();
    let trimmed = url.trim().trim_end_matches('/').to_string();
    if let Ok(mut slot) = api_base_slot().lock() {
        *slot = (!trimmed.is_empty()).then_some(trimmed);
    }
}

/// The WorkOS base in force.
fn auth_base() -> String {
    auth_base_slot()
        .lock()
        .ok()
        .and_then(|slot| slot.clone())
        .unwrap_or_else(|| DEFAULT_AUTH_BASE.to_string())
}

/// The Cline API base in force.
fn api_base() -> String {
    api_base_slot()
        .lock()
        .ok()
        .and_then(|slot| slot.clone())
        .unwrap_or_else(|| DEFAULT_API_BASE.to_string())
}

// ---------------------------------------------------------------------------
// The flow's URLs and bodies (pure)
// ---------------------------------------------------------------------------

/// The device-authorization request's body: the client id and nothing else.
#[must_use]
pub fn device_authorize_body() -> String {
    form_encode(&[(String::from("client_id"), CLIENT_ID.to_string())])
}

/// One poll's body: the pair the code request issued, per RFC 8628.
#[must_use]
pub fn device_poll_body(device_code: &str) -> String {
    form_encode(&[
        (String::from("grant_type"), DEVICE_GRANT.to_string()),
        (String::from("device_code"), device_code.to_string()),
        (String::from("client_id"), CLIENT_ID.to_string()),
    ])
}

/// The registration body: the WorkOS pair the poll approved, exchanged for
/// Cline's own tokens.
#[must_use]
pub fn register_body(workos_access: &str, workos_refresh: &str) -> serde_json::Value {
    serde_json::json!({
        "accessToken": workos_access,
        "refreshToken": workos_refresh,
    })
}

/// The refresh grant's body. JSON — Cline's API, not a standard OAuth token
/// endpoint — and spelled with the camel-case key and the `grantType` field
/// its own client sends.
#[must_use]
pub fn refresh_body(refresh_token: &str) -> serde_json::Value {
    serde_json::json!({
        "refreshToken": refresh_token,
        "grantType": "refresh_token",
    })
}

// ---------------------------------------------------------------------------
// The device flow's wire shapes (pure)
// ---------------------------------------------------------------------------

/// What the device-authorization request answered: the pair the poll
/// presents, and the short code the user confirms in a browser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceCode {
    /// The server's handle on this attempt — presented by every poll, never
    /// shown.
    device_code: String,
    /// The short code the user confirms at the verification URI.
    user_code: String,
    /// Where they confirm it (WorkOS's AuthKit domain, so the app does not
    /// build it).
    verification_uri: String,
    /// The one-click variant with the code baked in. Unused here — the page
    /// shows the code in a box and the user types it — but parsed so the
    /// value is not silently dropped if a page ever offers to open it.
    verification_uri_complete: Option<String>,
    /// How long the pair stays valid, in seconds.
    expires_in: u64,
    /// The minimum seconds between polls, as sent.
    interval: u64,
}

/// The wire shape of the code response. `expires_in` and `interval` arrive as
/// numbers from WorkOS; both spellings are accepted, the tolerance
/// `docs/chatgpt.md`'s device code already extends.
#[derive(Debug, Deserialize)]
struct RawDeviceCode {
    #[serde(default)]
    device_code: String,
    #[serde(default)]
    user_code: String,
    #[serde(default)]
    verification_uri: String,
    #[serde(default)]
    verification_uri_complete: Option<String>,
    #[serde(default)]
    expires_in: Option<RawSeconds>,
    #[serde(default)]
    interval: Option<RawSeconds>,
}

/// A number, or a string holding one.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RawSeconds {
    Seconds(u64),
    Text(String),
}

impl RawSeconds {
    fn seconds(&self) -> Option<u64> {
        match self {
            Self::Seconds(n) => Some(*n),
            Self::Text(text) => text.trim().parse().ok(),
        }
    }
}

impl DeviceCode {
    /// Parse the code response.
    ///
    /// # Errors
    /// A body that isn't the response's shape, or that names no code or no
    /// page to enter it at, is a decode error — there is nothing to show or
    /// poll with either way.
    pub fn parse(body: &str) -> Result<Self> {
        let raw: RawDeviceCode =
            serde_json::from_str(body).map_err(|e| LlmError::Decode(e.to_string()))?;
        if raw.device_code.is_empty() || raw.user_code.is_empty() || raw.verification_uri.is_empty()
        {
            return Err(LlmError::Decode(
                "WorkOS returned no device code".to_string(),
            ));
        }
        Ok(Self {
            device_code: raw.device_code,
            user_code: raw.user_code,
            verification_uri: raw.verification_uri,
            verification_uri_complete: raw.verification_uri_complete,
            expires_in: raw
                .expires_in
                .and_then(|e| e.seconds())
                .unwrap_or(DEFAULT_EXPIRES),
            interval: raw
                .interval
                .and_then(|i| i.seconds())
                .unwrap_or(DEFAULT_INTERVAL),
        })
    }

    /// The page the user confirms the code at.
    #[must_use]
    pub fn verification_uri(&self) -> &str {
        &self.verification_uri
    }

    /// The code to show and copy.
    #[must_use]
    pub fn user_code(&self) -> &str {
        &self.user_code
    }

    /// How long the code stays valid — what the page counts down from and
    /// what the poll's deadline is.
    #[must_use]
    pub fn lifetime(&self) -> Duration {
        Duration::from_secs(self.expires_in.max(1))
    }

    /// How long to wait between polls: what the server asked, floored at a
    /// second (a zero would be a tight loop).
    #[must_use]
    pub fn poll_interval(&self) -> Duration {
        Duration::from_secs(self.interval.max(1))
    }
}

/// What one poll of `/user_management/authenticate` established. WorkOS
/// answers pending/slow_down as an OAuth **error in the body** — a 4xx from
/// the reference server — so the verdict reads the JSON rather than the
/// status, and a `2xx` carrying an error still lands where it should.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Poll {
    /// The user confirmed the code: the WorkOS pair to register.
    Approved {
        /// The WorkOS access token the register call exchanges.
        access_token: String,
        /// The WorkOS refresh token that rides registration with it.
        refresh_token: String,
    },
    /// Not confirmed yet — ask again after the interval.
    Pending,
    /// The server asked for a slower cadence; the caller grows its interval.
    SlowDown,
    /// The flow is over — the sentence to show.
    Denied(String),
}

/// Read one poll's answer the way Cline's own client does: a body carrying
/// both token strings is an approval regardless of status, the three terminal
/// RFC 8628 errors end the flow, and anything else is one more wait.
#[must_use]
pub fn poll_verdict(status: u16, body: &str) -> Poll {
    #[derive(Debug, Default, Deserialize)]
    #[serde(default)]
    struct Raw {
        error: Option<String>,
        error_description: Option<String>,
        access_token: Option<String>,
        refresh_token: Option<String>,
    }
    let Ok(raw) = serde_json::from_str::<Raw>(body) else {
        return Poll::Denied(format!("WorkOS answered {status} to the device code poll"));
    };
    let pair = raw.access_token.clone().zip(raw.refresh_token.clone());
    if let Some((access_token, refresh_token)) = pair
        && !access_token.is_empty()
        && !refresh_token.is_empty()
    {
        return Poll::Approved {
            access_token,
            refresh_token,
        };
    }
    // A denial carries the server's own human sentence when it wrote one;
    // the fixed line is the fallback, because a bare code is not something a
    // user can act on.
    let denial = |fallback: &str| {
        raw.error_description
            .clone()
            .filter(|d| !d.trim().is_empty())
            .unwrap_or_else(|| fallback.to_string())
    };
    match raw.error.as_deref() {
        Some("authorization_pending") => Poll::Pending,
        Some("slow_down") => Poll::SlowDown,
        Some("access_denied") => Poll::Denied(denial(
            "the sign-in was denied in the browser — press Esc and try again",
        )),
        Some("expired_token") => Poll::Denied(denial(
            "the device code expired — press Esc and sign in again",
        )),
        Some("invalid_grant") => Poll::Denied(denial(
            "the device code is no longer valid — press Esc and sign in again",
        )),
        Some(other) => Poll::Denied(format!(
            "WorkOS refused the sign-in: {other} — press Esc and try again"
        )),
        None => Poll::Denied(format!("WorkOS answered {status} to the device code poll")),
    }
}

// ---------------------------------------------------------------------------
// Cline's token envelopes (pure)
// ---------------------------------------------------------------------------

/// What `/api/v1/auth/register` and `/api/v1/auth/refresh` carry in their
/// `data` object. `refreshToken` is optional because a **refresh** re-sends
/// it only when it rotated — a missing one means "keep the one you have".
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct AuthData {
    #[serde(rename = "accessToken", default)]
    pub access_token: String,
    #[serde(rename = "refreshToken", default)]
    pub refresh_token: Option<String>,
    /// ISO 8601; the primary expiry source, parsed by [`parse_expires_at`].
    #[serde(rename = "expiresAt", default)]
    pub expires_at: Option<String>,
    #[serde(rename = "userInfo", default)]
    pub user_info: Option<AuthUser>,
}

/// The account the registration named. Only the email is used — it is what
/// the sign-in confirmation shows — but the wire shape carries the rest.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct AuthUser {
    #[serde(default)]
    pub email: Option<String>,
}

/// The `{success, data}` envelope both endpoints answer with.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawEnvelope {
    success: bool,
    data: Option<AuthData>,
}

/// Parse an envelope, refusing one that is not a success or names no access
/// token — there is nothing to authenticate with either way.
fn parse_auth_data(body: &str) -> Result<AuthData> {
    let raw: RawEnvelope =
        serde_json::from_str(body).map_err(|e| LlmError::Decode(e.to_string()))?;
    let data = raw.data.unwrap_or_default();
    if !raw.success || data.access_token.trim().is_empty() {
        return Err(LlmError::Decode(
            "Cline returned no access token".to_string(),
        ));
    }
    Ok(data)
}

/// A completed sign-in: the **Cline** refresh token to store, and the account
/// it belongs to. The WorkOS pair is deliberately not part of it — the
/// register call consumed it, and nothing after the sign-in talks to WorkOS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedIn {
    /// What goes into the `.env` store under [`REFRESH_ENV_VAR`].
    pub refresh_token: String,
    /// The account's email, for the confirmation.
    pub email: Option<String>,
}

/// Parse a registration response, refusing one with no refresh token:
/// without it the session would die at the first access-token expiry, an
/// hour in, with no way back but another sign-in — better to say so now,
/// exactly as `chatgpt::exchange_code` does.
fn parse_registration(body: &str) -> Result<SignedIn> {
    let data = parse_auth_data(body)?;
    let refresh_token = data
        .refresh_token
        .filter(|t| !t.trim().is_empty())
        .ok_or_else(|| {
            LlmError::Decode("Cline returned no refresh token — sign in again".to_string())
        })?;
    Ok(SignedIn {
        refresh_token,
        email: data
            .user_info
            .and_then(|u| u.email)
            .filter(|e| !e.trim().is_empty()),
    })
}

/// `expiresAt` as epoch seconds. ISO 8601, UTC (`…Z`) from every real answer;
/// an unparseable value is `None`, which sends the cache to the token's own
/// `exp` claim instead.
#[must_use]
pub fn parse_expires_at(text: &str) -> Option<u64> {
    chrono::DateTime::parse_from_rfc3339(text.trim())
        .ok()
        .and_then(|dt| u64::try_from(dt.timestamp()).ok())
}

/// An access token's own `exp` claim, for when the envelope named no
/// parseable `expiresAt`. No signature check — nothing here verifies a JWT,
/// only reads when it says it dies (`chatgpt::parse_claims`'s posture).
fn jwt_exp(jwt: &str) -> Option<u64> {
    let payload = jwt.split('.').nth(1)?;
    let bytes = b64url_decode(payload)?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    value.get("exp").and_then(serde_json::Value::as_u64)
}

/// Decode base64url (RFC 4648 §5), padded or not — `chatgpt`'s decoder, local
/// so this module's pure half stands alone.
fn b64url_decode(text: &str) -> Option<Vec<u8>> {
    let value = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => u32::from(c - b'A'),
            b'a'..=b'z' => u32::from(c - b'a') + 26,
            b'0'..=b'9' => u32::from(c - b'0') + 52,
            b'-' => 62,
            b'_' => 63,
            _ => return None,
        })
    };
    let trimmed = text.trim_end_matches('=');
    let mut out = Vec::with_capacity(trimmed.len() / 4 * 3);
    let mut acc: u32 = 0;
    let mut bits = 0u32;
    for byte in trimmed.bytes() {
        acc = (acc << 6) | value(byte)?;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(u8::try_from((acc >> bits) & 0xFF).ok()?);
        }
    }
    Some(out)
}

/// How long an access token may be cached: until the envelope's `expiresAt`
/// (or, failing that, the token's own `exp` claim), less a five-minute skew so
/// no request leaves with a token that dies in flight.
///
/// A token that says nothing about its life is trusted for a conservative
/// minute rather than forever; a short-lived one keeps at most half its
/// advertised life, which is what keeps the fallback from ever extending an
/// explicit expiry. An already-expired deadline caches nothing.
#[must_use]
pub fn cache_lifetime(expires_at: Option<u64>, jwt_exp: Option<u64>, now: u64) -> Duration {
    let Some(deadline) = expires_at.or(jwt_exp) else {
        return MIN_CACHE;
    };
    let lifetime = Duration::from_secs(deadline.saturating_sub(now));
    if lifetime.is_zero() {
        return Duration::ZERO;
    }
    lifetime
        .saturating_sub(REFRESH_SKEW)
        .max(MIN_CACHE.min(lifetime / 2))
}

/// The wire form of a Cline credential: OAuth access tokens ride
/// `workos:`-prefixed (`Authorization: Bearer workos:<jwt>`), and a value
/// that already carries the prefix is left alone so re-formatting one never
/// doubles it. A pasted dashboard key goes verbatim, which is why only this
/// module prefixes — the *pasted-key* provider next door sends its key
/// straight through.
#[must_use]
pub fn bearer_for(access_token: &str) -> String {
    let token = access_token.trim();
    if token.to_ascii_lowercase().starts_with(WORKOS_PREFIX) {
        token.to_string()
    } else {
        format!("{WORKOS_PREFIX}{token}")
    }
}

/// Explain a refused request in terms the user can act on. The three statuses
/// are Cline's own documented meanings (`docs.cline.bot/api/errors`): a 401
/// is the sign-in itself (the refresh token was rejected), a 402 is the
/// account's balance, and a 403 is the credential's reach. Two of Cline's
/// refusals are **messages rather than statuses** — its own clients detect
/// them the same way (`errors.ts`): the ClinePass window running out
/// (*"… your ClinePass limit …"*) and a `cline-pass/…` model asked for
/// without the subscription — and each wants a sentence the bare status
/// advice would get wrong. Anything else is left to the transport.
#[must_use]
pub fn auth_advice(status: u16, body: &str) -> Option<String> {
    let lower = body.to_ascii_lowercase();
    if lower.contains("clinepass limit") {
        return Some(
            "this account's ClinePass usage limit is reached for its current window — \
             wait for it to reset, or switch to a usage-billed model (/model → a Cline \
             Account model, with credits) or another provider."
                .to_string(),
        );
    }
    if lower.contains("no access to clinepass subscription models") {
        return Some(
            "this Cline account has no ClinePass subscription — subscribe at \
             app.cline.bot/dashboard/subscription, or pick a usage-billed model with /model."
                .to_string(),
        );
    }
    match status {
        401 => Some(
            "your Cline sign-in is no longer valid — run /login and sign in again.".to_string(),
        ),
        402 => Some(
            "this Cline account is out of credits — add credits at app.cline.bot/dashboard, \
             or switch provider with /model."
                .to_string(),
        ),
        403 => Some(
            "this Cline account isn't allowed to make that request — check its access in the \
             Cline dashboard, or switch model with /model."
                .to_string(),
        ),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// The access-token cache (boundary state)
// ---------------------------------------------------------------------------

/// One minted access token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Access {
    /// The bearer the request carries — the `workos:`-prefixed access token.
    pub bearer: String,
}

/// A cached [`Access`] and when it stops being usable.
#[derive(Debug, Clone)]
struct Cached {
    access: Access,
    until: Instant,
}

/// Access tokens keyed by the **refresh token they were minted from**. A
/// turn's rounds, the `/model` fetch and every subagent thread want the same
/// bearer, and re-minting per request would spend a round trip each time
/// against an endpoint that rotates its input.
fn cache() -> &'static Mutex<HashMap<String, Cached>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Cached>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Serialize cache misses through refresh, rotation, and persistence. A
/// refresh token can be single-use, so locking only the map lookup is unsafe.
/// Warm hits remain independent of an unrelated account's slow refresh.
fn refresh_gate() -> &'static Mutex<()> {
    static GATE: Mutex<()> = Mutex::new(());
    &GATE
}

/// The refresh tokens [`forget`] dropped since a refresh last settled. A
/// refresh drains it under the cache lock before keeping what it minted, and
/// a name from its own chain there means a sign-in replaced the credential
/// while the request was out: the bearer is not cached, and the next request
/// refreshes afresh. This is what lets `forget` skip the gate — it runs on
/// the TUI loop when a sign-in lands, and a refresh in flight can hold the
/// gate for the whole request timeout — without a sign-in on one account
/// costing another account's refresh its bearer. Draining the whole list is
/// safe: the gate serializes refreshes, so the one settling is the only one
/// that was in flight, and a refresh still waiting on the gate re-reads the
/// cleared cache before it starts.
fn forgotten() -> &'static Mutex<Vec<String>> {
    static FORGOTTEN: Mutex<Vec<String>> = Mutex::new(Vec::new());
    &FORGOTTEN
}

/// Resolve aliases before the lookup: another config may have refreshed the
/// same account while this config still holds an older refresh token.
fn cached_access(refresh_token: &str) -> Option<Access> {
    let presented = live_refresh_token(refresh_token);
    let map = cache().lock().ok()?;
    let entry = map.get(&presented)?;
    (entry.until > Instant::now()).then(|| entry.access.clone())
}

/// The **freshest** refresh token for one the session is still holding.
///
/// Rotation is not only a disk concern: the live `ModelConfig` carries the
/// token the session started with and nothing reloads it mid-run, so once
/// Cline retires that token the *next* refresh would present a dead one.
/// This map remembers what each retired token became — the in-memory half of
/// the fix [`persist_refresh`] makes on disk.
fn rotations() -> &'static Mutex<HashMap<String, String>> {
    static ROTATIONS: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    ROTATIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The token to actually present for `stored` — itself, or whatever it has
/// since been rotated into.
fn live_refresh_token(stored: &str) -> String {
    rotations()
        .lock()
        .ok()
        .and_then(|map| map.get(stored).cloned())
        .unwrap_or_else(|| stored.to_string())
}

/// Note that `stored` (and the token actually presented, when they differ)
/// has been superseded by `rotated`.
fn note_rotation(stored: &str, presented: &str, rotated: &str) {
    if let Ok(mut map) = rotations().lock() {
        // A rebuilt config may present a newer alias than the original
        // config. Repoint every alias, keeping the map one hop deep.
        for target in map.values_mut() {
            if target == presented {
                *target = rotated.to_string();
            }
        }
        map.insert(stored.to_string(), rotated.to_string());
        if presented != stored {
            map.insert(presented.to_string(), rotated.to_string());
        }
    }
}

/// Where a rotated refresh token is written back. Set once at the boundary
/// (`tui::models`), because the rotation happens deep on a backend thread.
fn store_path() -> &'static Mutex<Option<PathBuf>> {
    static PATH: OnceLock<Mutex<Option<PathBuf>>> = OnceLock::new();
    PATH.get_or_init(|| Mutex::new(None))
}

/// Tell this module where the `.env` key store lives, so a rotated refresh
/// token can be persisted. Boundary-only; called once at startup.
pub fn set_store_path(path: impl Into<PathBuf>) {
    if let Ok(mut slot) = store_path().lock() {
        *slot = Some(path.into());
    }
}

/// Drop any cached access token minted from `refresh_token`. Signing in again
/// is how a user recovers from a token the server has stopped honouring, so
/// the cached one must not outlive the sign-in that replaced it.
pub fn forget(refresh_token: &str) {
    // Never behind the gate — a refresh in flight holds it for as long as
    // its request takes.
    let current = live_refresh_token(refresh_token);
    let mut aliases = vec![refresh_token.to_string(), current.clone()];
    if let Ok(mut map) = rotations().lock() {
        map.retain(|alias, target| {
            if alias == refresh_token || target == &current {
                aliases.push(alias.clone());
                false
            } else {
                true
            }
        });
    }
    // On record *before* the clear: a refresh that inserts between the two
    // finds its chain named under the cache lock and drops its bearer
    // instead, so nothing minted before this sign-in survives under the
    // forgotten key.
    if let Ok(mut list) = forgotten().lock() {
        list.extend(aliases.iter().cloned());
    }
    if let Ok(mut map) = cache().lock() {
        for alias in aliases {
            map.remove(&alias);
        }
    }
}

/// Epoch seconds, for the freshness rule.
fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// The sign-in flow and the token mint (boundary — real HTTP)
// ---------------------------------------------------------------------------

/// The four URLs, on the bases in force.
fn device_authorize_url() -> String {
    format!("{}{DEVICE_AUTHORIZE_PATH}", auth_base())
}

fn authenticate_url() -> String {
    format!("{}{AUTHENTICATE_PATH}", auth_base())
}

fn register_url() -> String {
    format!("{}{REGISTER_PATH}", api_base())
}

fn refresh_url() -> String {
    format!("{}{REFRESH_PATH}", api_base())
}

/// Read a response body, bounded, and trim it to something showable.
fn read_body(resp: reqwest::blocking::Response) -> String {
    let mut body = String::new();
    let _ = resp.take(BODY_MAX_BYTES).read_to_string(&mut body);
    body
}

/// Turn a non-2xx token response into the error the page shows.
fn token_failure(status: u16, body: &str) -> LlmError {
    let explained = auth_advice(status, body).unwrap_or_else(|| {
        let trimmed = body.trim();
        if trimmed.is_empty() {
            format!("Cline answered {status}")
        } else {
            format!("Cline answered {status}: {trimmed}")
        }
    });
    LlmError::Api {
        status,
        body: explained,
    }
}

/// Ask WorkOS for a device code. The caller shows [`DeviceCode::user_code`]
/// beside [`DeviceCode::verification_uri`], then blocks in
/// [`await_approval`].
///
/// # Errors
/// A code request WorkOS refused, or a transport failure.
pub fn request_device_code() -> Result<DeviceCode> {
    let client = super::http_client(OP_TIMEOUT)?;
    let resp = client
        .post(device_authorize_url())
        .header("content-type", "application/x-www-form-urlencoded")
        .header("accept", "application/json")
        .body(device_authorize_body())
        .send()
        .map_err(|e| LlmError::transport(&e))?;
    let status = resp.status().as_u16();
    let text = read_body(resp);
    if !(200..300).contains(&status) {
        let trimmed = text.trim();
        let shown = if trimmed.is_empty() {
            format!("WorkOS answered {status} to the device code request")
        } else {
            format!("WorkOS answered {status}: {trimmed}")
        };
        return Err(LlmError::Api {
            status,
            body: shown,
        });
    }
    DeviceCode::parse(&text)
}

/// Sleep `total` in [`POLL_NAP`] slices, returning `false` the moment
/// `cancel` trips — [`super::chatgpt`]'s nap, for the same reason.
fn nap(total: Duration, cancel: &CancelToken) -> bool {
    let until = Instant::now() + total;
    while Instant::now() < until {
        if cancel.is_cancelled() {
            return false;
        }
        std::thread::sleep(POLL_NAP.min(until.saturating_duration_since(Instant::now())));
    }
    !cancel.is_cancelled()
}

/// Poll until the user confirms `device`'s code on WorkOS's page, then
/// register the token pair it approved with Cline's API. Blocks for as long
/// as that takes — up to the code's own life — napping on the poll cadence so
/// Esc on the page is honoured promptly. Returns the refresh token to store
/// and the account it belongs to.
///
/// # Errors
/// A cancelled flow, an expired code, a poll WorkOS refused, or a
/// registration it refused.
pub fn await_approval(device: &DeviceCode, cancel: &CancelToken) -> Result<SignedIn> {
    let client = super::http_client(OP_TIMEOUT)?;
    let deadline = Instant::now() + device.lifetime();
    let mut interval = device.poll_interval();
    let body = device_poll_body(&device.device_code);
    loop {
        if cancel.is_cancelled() {
            return Err(LlmError::Cancelled);
        }
        if Instant::now() >= deadline {
            return Err(LlmError::Decode(
                "the device code expired — press Esc and sign in again".to_string(),
            ));
        }
        let resp = client
            .post(authenticate_url())
            .header("content-type", "application/x-www-form-urlencoded")
            .header("accept", "application/json")
            .body(body.clone())
            .send()
            .map_err(|e| LlmError::transport(&e))?;
        let status = resp.status().as_u16();
        let text = read_body(resp);
        match poll_verdict(status, &text) {
            Poll::Approved {
                access_token,
                refresh_token,
            } => return register(&access_token, &refresh_token),
            Poll::Pending => {
                if !nap(interval, cancel) {
                    return Err(LlmError::Cancelled);
                }
            }
            Poll::SlowDown => {
                interval += Duration::from_secs(SLOW_DOWN_STEP);
                if !nap(interval, cancel) {
                    return Err(LlmError::Cancelled);
                }
            }
            Poll::Denied(reason) => {
                return Err(LlmError::Api {
                    status,
                    body: reason,
                });
            }
        }
    }
}

/// Exchange the approved WorkOS pair for Cline's own tokens — the one call
/// that makes the sign-in a *Cline* one. The refresh token it returns is what
/// goes into the key store; the WorkOS pair is not stored anywhere.
fn register(workos_access: &str, workos_refresh: &str) -> Result<SignedIn> {
    let client = super::http_client(OP_TIMEOUT)?;
    let resp = client
        .post(register_url())
        .header("content-type", "application/json")
        .header("accept", "application/json")
        .json(&register_body(workos_access, workos_refresh))
        .send()
        .map_err(|e| LlmError::transport(&e))?;
    let status = resp.status().as_u16();
    let text = read_body(resp);
    if !(200..300).contains(&status) {
        return Err(token_failure(status, &text));
    }
    parse_registration(&text)
}

/// The access token to authenticate with, minted from `refresh_token` and
/// cached until shortly before its own expiry. Boundary code — one HTTP round
/// trip on a cache miss, none on a hit.
///
/// A rotated refresh token is written back to the key store before this
/// returns: Cline may retire the one just used, and leaving the retired
/// value on disk turns the next launch into a forced re-login.
///
/// # Errors
/// A refresh Cline refused, or a transport failure.
pub fn authorize(refresh_token: &str) -> Result<Access> {
    authorize_with(refresh_token, exchange_refresh)
}

/// Keep the refresh transport replaceable so concurrent cache misses can be
/// exercised deterministically without a live account.
fn authorize_with(
    refresh_token: &str,
    refresh: impl FnOnce(&str) -> Result<AuthData>,
) -> Result<Access> {
    if let Some(access) = cached_access(refresh_token) {
        return Ok(access);
    }
    let _refresh = refresh_gate().lock().unwrap_or_else(|e| e.into_inner());
    // A worker that was already refreshing may have filled this cache while
    // we waited. Never present its now-retired refresh token a second time.
    if let Some(access) = cached_access(refresh_token) {
        return Ok(access);
    }
    // What to actually present: the stored token, or whatever a previous
    // rotation turned it into. The config still holds the original.
    let presented = live_refresh_token(refresh_token);
    let refresh_started = Instant::now();
    let data = refresh(&presented)?;
    let access = access_from(&data)?;
    let lifetime = cache_lifetime(
        data.expires_at.as_deref().and_then(parse_expires_at),
        jwt_exp(&data.access_token),
        unix_now(),
    );
    // The alias map keeps old configs working; only the current alias needs
    // a bearer entry. Keeping one under every retired token leaks old JWTs.
    let cache_key = if let Some(rotated) = data
        .refresh_token
        .filter(|t| !t.is_empty() && t.trim() != presented.trim())
    {
        persist_refresh(&rotated, &[refresh_token, &presented]);
        note_rotation(refresh_token, &presented, &rotated);
        rotated
    } else {
        presented.clone()
    };
    if let Ok(mut map) = cache().lock() {
        map.remove(&presented);
        let now = Instant::now();
        map.retain(|_, entry| entry.until > now);
        // A `forget` of this chain while the refresh was out: a sign-in has
        // replaced the credential, so the bearer is not kept (the rotation
        // above stands — the token presented is retired either way, and an
        // old config must still find its way to the live one).
        let forgotten = forgotten()
            .lock()
            .map(|mut list| std::mem::take(&mut *list))
            .unwrap_or_default();
        let replaced = forgotten
            .iter()
            .any(|token| token == refresh_token || *token == presented || *token == cache_key);
        if !replaced
            && let Some(until) = refresh_started.checked_add(lifetime)
            && until > now
        {
            map.insert(
                cache_key,
                Cached {
                    access: access.clone(),
                    until,
                },
            );
        }
    }
    Ok(access)
}

/// The [`Access`] a token response describes.
fn access_from(data: &AuthData) -> Result<Access> {
    if data.access_token.trim().is_empty() {
        return Err(LlmError::Decode(
            "Cline returned no access token".to_string(),
        ));
    }
    Ok(Access {
        bearer: bearer_for(&data.access_token),
    })
}

fn exchange_refresh(presented: &str) -> Result<AuthData> {
    let client = super::http_client(OP_TIMEOUT)?;
    let resp = client
        .post(refresh_url())
        .header("content-type", "application/json")
        .header("accept", "application/json")
        .json(&refresh_body(presented))
        .send()
        .map_err(|e| LlmError::transport(&e))?;
    let status = resp.status().as_u16();
    let text = read_body(resp);
    if !(200..300).contains(&status) {
        return Err(token_failure(status, &text));
    }
    parse_auth_data(&text)
}

/// Write a rotated refresh token back into the `.env` key store, in place
/// — [`persist_refresh_to`] at the store the boundary named. Best-effort: a
/// store we cannot write still leaves a working session, and the failure
/// surfaces as a re-login at the next launch rather than a mid-turn error
/// the user can do nothing about.
fn persist_refresh(rotated: &str, chain: &[&str]) {
    let Some(path) = store_path().lock().ok().and_then(|p| p.clone()) else {
        return;
    };
    persist_refresh_to(&path, rotated, chain);
}

/// Write `rotated` under [`REFRESH_ENV_VAR`] in the store at `path`, but
/// only while the store still holds this session's own `chain` — the token
/// it started with and the one it actually presented — or no token at all
/// (a credential from the process environment, or a store not created
/// yet). A sign-in that landed while the refresh was in flight wrote its
/// own token there, and a rotation of the chain that sign-in replaced must
/// not overwrite it: the next launch would present a retired token and
/// force the re-login the sign-in had just done. Returns whether it wrote.
fn persist_refresh_to(path: &std::path::Path, rotated: &str, chain: &[&str]) -> bool {
    super::EnvFile::update_if(path, REFRESH_ENV_VAR, rotated, |current| {
        current.is_none_or(|held| held.is_empty() || chain.contains(&held))
    })
    .is_ok_and(|written| written.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::config::ProvidersFile;

    /// An RFC 3339 timestamp `secs` from now — the shape an `expiresAt`
    /// response carries, kept near-now so `Instant::checked_add` can hold it.
    fn iso_in(secs: i64) -> String {
        (chrono::Utc::now() + chrono::Duration::seconds(secs)).to_rfc3339()
    }

    #[test]
    fn the_client_id_is_the_one_cline_ships() {
        // Read from Cline's own client (`shared/src/runtime/cline-environment.ts`,
        // production). A different id is a client nobody can sign in with —
        // exactly the failure the SDK's constants exist to prevent.
        assert_eq!(CLIENT_ID, "client_01K3A541FN8TA3EPPHTD2325AR");
    }

    #[test]
    fn the_device_code_request_names_the_client_id_and_nothing_else() {
        assert_eq!(
            device_authorize_body(),
            "client_id=client_01K3A541FN8TA3EPPHTD2325AR"
        );
    }

    #[test]
    fn the_poll_presents_the_grant_the_code_and_the_client() {
        assert_eq!(
            device_poll_body("device-123"),
            "grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Adevice_code\
             &device_code=device-123&client_id=client_01K3A541FN8TA3EPPHTD2325AR"
        );
    }

    #[test]
    fn a_device_code_response_parses_into_the_pair_and_the_pacing() {
        let device = DeviceCode::parse(
            r#"{"device_code":"dc","user_code":"RRGQ-BJVS",
                "verification_uri":"https://example.authkit.app/device",
                "verification_uri_complete":"https://example.authkit.app/device?user_code=RRGQ-BJVS",
                "expires_in":300,"interval":5}"#,
        )
        .unwrap();
        assert_eq!(device.user_code(), "RRGQ-BJVS");
        assert_eq!(
            device.verification_uri(),
            "https://example.authkit.app/device"
        );
        assert_eq!(device.lifetime(), Duration::from_secs(300));
        assert_eq!(device.poll_interval(), Duration::from_secs(5));
    }

    #[test]
    fn a_code_response_naming_no_pacing_falls_back_to_workos_defaults() {
        let device = DeviceCode::parse(
            r#"{"device_code":"dc","user_code":"RRGQ-BJVS",
                "verification_uri":"https://example.authkit.app/device"}"#,
        )
        .unwrap();
        assert_eq!(device.lifetime(), Duration::from_secs(DEFAULT_EXPIRES));
        assert_eq!(
            device.poll_interval(),
            Duration::from_secs(DEFAULT_INTERVAL)
        );
    }

    #[test]
    fn a_bodyless_code_response_is_a_decode_error() {
        assert!(DeviceCode::parse("{}").is_err());
        assert!(DeviceCode::parse("not json").is_err());
    }

    #[test]
    fn a_pending_poll_is_not_a_denial_whichever_status_carries_it() {
        // The reference server answers pending as a 4xx OAuth error; the
        // verdict reads the body, so a 200 carrying the same error still
        // waits rather than reading an approval that is not there.
        let body = r#"{"error":"authorization_pending"}"#;
        assert_eq!(poll_verdict(400, body), Poll::Pending);
        assert_eq!(poll_verdict(200, body), Poll::Pending);
    }

    #[test]
    fn a_slow_down_poll_asks_for_a_slower_cadence() {
        assert_eq!(
            poll_verdict(400, r#"{"error":"slow_down"}"#),
            Poll::SlowDown
        );
    }

    #[test]
    fn an_approval_carries_the_workos_pair() {
        assert_eq!(
            poll_verdict(
                200,
                r#"{"access_token":"wa","refresh_token":"wr","token_type":"Bearer"}"#
            ),
            Poll::Approved {
                access_token: "wa".to_string(),
                refresh_token: "wr".to_string(),
            }
        );
    }

    #[test]
    fn the_terminal_poll_errors_explain_themselves() {
        // A denial keeps the server's own human sentence when it wrote one.
        assert_eq!(
            poll_verdict(
                400,
                r#"{"error":"access_denied","error_description":"The user denied the request."}"#
            ),
            Poll::Denied("The user denied the request.".to_string())
        );
        // The bare codes fall back to lines a user can act on.
        assert!(matches!(
            poll_verdict(400, r#"{"error":"expired_token"}"#),
            Poll::Denied(reason) if reason.contains("expired")
        ));
        assert!(matches!(
            poll_verdict(400, r#"{"error":"invalid_grant"}"#),
            Poll::Denied(reason) if reason.contains("no longer valid")
        ));
        assert!(matches!(
            poll_verdict(502, "upstream hiccup"),
            Poll::Denied(reason) if reason.contains("502")
        ));
    }

    #[test]
    fn the_register_body_names_both_workos_tokens() {
        assert_eq!(
            register_body("wa", "wr"),
            serde_json::json!({"accessToken": "wa", "refreshToken": "wr"})
        );
    }

    #[test]
    fn the_refresh_grant_is_json_with_the_grant_type_cline_expects() {
        assert_eq!(
            refresh_body("tok"),
            serde_json::json!({"refreshToken": "tok", "grantType": "refresh_token"})
        );
    }

    #[test]
    fn a_registration_parses_into_the_store_and_the_account() {
        let signed_in = parse_registration(
            r#"{"success":true,"data":{"accessToken":"eyJx.eyJy.s",
                "refreshToken":"clr-1","tokenType":"Bearer",
                "expiresAt":"2030-01-01T00:00:00.000Z",
                "userInfo":{"email":"dev@example.com","clineUserId":"u-1"}}}"#,
        )
        .unwrap();
        assert_eq!(signed_in.refresh_token, "clr-1");
        assert_eq!(signed_in.email.as_deref(), Some("dev@example.com"));
    }

    #[test]
    fn a_registration_without_a_refresh_token_is_refused() {
        // Without one the session would die at the first access-token
        // expiry with no way back but another sign-in.
        let body = r#"{"success":true,"data":{"accessToken":"eyJx.eyJy.s"}}"#;
        assert!(parse_registration(body).is_err());
        let failed = r#"{"success":false,"data":{"accessToken":"eyJx.eyJy.s","refreshToken":"r"}}"#;
        assert!(parse_registration(failed).is_err());
    }

    #[test]
    fn a_refresh_may_keep_the_token_it_presented() {
        // The server re-sends `refreshToken` only when it rotated; a missing
        // one means "keep the one you have", never "you have none".
        let data = parse_auth_data(
            r#"{"success":true,"data":{"accessToken":"eyJx.eyJy.s",
                "expiresAt":"2030-01-01T00:00:00Z"}}"#,
        )
        .unwrap();
        assert_eq!(data.refresh_token, None);
        assert!(data.expires_at.is_some());
    }

    #[test]
    fn an_expires_at_in_iso_form_reads_as_epoch_seconds() {
        assert_eq!(
            parse_expires_at("2030-01-01T00:00:00.000Z"),
            Some(1893456000)
        );
        assert_eq!(
            parse_expires_at("2030-01-01T01:00:00+01:00"),
            Some(1893456000)
        );
        assert_eq!(parse_expires_at("soon"), None);
        assert_eq!(parse_expires_at(""), None);
    }

    #[test]
    fn the_effective_expiry_is_the_envelope_then_the_jwt_claim() {
        // `expiresAt` wins when parseable; the token's own `exp` answers
        // when it is not; nothing means a conservative minute.
        let now = 1_893_452_000;
        assert_eq!(
            cache_lifetime(Some(now + 3600), Some(now + 7200), now),
            Duration::from_secs(3300)
        );
        assert_eq!(
            cache_lifetime(None, Some(now + 3600), now),
            Duration::from_secs(3300)
        );
        assert_eq!(cache_lifetime(None, None, now), MIN_CACHE);
        // An already-expired deadline caches nothing.
        assert_eq!(cache_lifetime(Some(now - 1), None, now), Duration::ZERO);
    }

    #[test]
    fn a_short_lived_token_is_never_cached_past_half_its_life() {
        let now = 1_000_000;
        // 120 s of life, 300 s of skew: the floor is half the lifetime.
        assert_eq!(
            cache_lifetime(Some(now + 120), None, now),
            Duration::from_secs(60)
        );
        // 40 s: half is 20, under the minute cap, and stays 20.
        assert_eq!(
            cache_lifetime(Some(now + 40), None, now),
            Duration::from_secs(20)
        );
    }

    #[test]
    fn a_jwt_claim_is_read_without_a_signature_check() {
        fn jwt(exp: u64) -> String {
            let payload = serde_json::json!({"exp": exp}).to_string();
            format!("eyJhbGciOiJub25lIn0.{}.sig", base64url(payload.as_bytes()))
        }
        /// The test's own base64url encode, so the decoder has a known-good
        /// input rather than one built with the code under test's neighbours.
        fn base64url(bytes: &[u8]) -> String {
            const ALPHABET: &[u8] =
                b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
            let mut out = String::new();
            for chunk in bytes.chunks(3) {
                let mut acc = 0u32;
                for (i, b) in chunk.iter().enumerate() {
                    acc |= u32::from(*b) << (16 - 8 * i);
                }
                for i in 0..=chunk.len() {
                    let idx = ((acc >> (18 - 6 * i)) & 0x3F) as usize;
                    out.push(ALPHABET[idx] as char);
                }
            }
            out
        }
        assert_eq!(jwt_exp(&jwt(1893456000)), Some(1893456000));
        assert_eq!(jwt_exp("not-a-jwt"), None);
        assert_eq!(jwt_exp("eyJ4.$$$.y"), None);
    }

    #[test]
    fn a_workos_token_rides_prefixed_and_the_prefix_never_doubles() {
        assert_eq!(
            bearer_for("eyJhbGciOi.eyJzdWIi.sig"),
            "workos:eyJhbGciOi.eyJzdWIi.sig"
        );
        assert_eq!(bearer_for("workos:eyJx.eyJy.s"), "workos:eyJx.eyJy.s");
        // The prefix test is case-insensitive, like the reference's.
        assert_eq!(bearer_for("WORKOS:eyJx.eyJy.s"), "WORKOS:eyJx.eyJy.s");
    }

    #[test]
    fn the_advice_names_the_one_thing_to_do_about_each_refusal() {
        assert!(auth_advice(401, "").unwrap().contains("/login"));
        assert!(auth_advice(402, "").unwrap().contains("credits"));
        assert!(auth_advice(403, "").unwrap().contains("isn't allowed"));
        assert_eq!(auth_advice(429, "slow down"), None);
        // The ClinePass refusals are message-borne — Cline's own clients
        // detect them the same way — and outrank the status that carried
        // them: a Pass window running out must not read as "add credits".
        let limit = auth_advice(
            429,
            "You have reached your ClinePass limit. Please try again later.",
        )
        .unwrap();
        assert!(limit.contains("ClinePass usage limit"), "{limit}");
        let unsubscribed =
            auth_advice(403, "No access to ClinePass subscription models yet.").unwrap();
        assert!(
            unsubscribed.contains("no ClinePass subscription"),
            "{unsubscribed}"
        );
    }

    #[test]
    fn the_env_var_matches_the_shipped_provider_file() {
        // The rotation write-back cannot ask the provider file, so the name
        // is pinned here — and pinned *to* the file, since a mismatch would
        // silently write rotations someplace key resolution never reads.
        let file = ProvidersFile::builtin();
        let account = file.get("cline_account").expect("shipped");
        assert_eq!(account.key_env("cline_account"), REFRESH_ENV_VAR);
        assert_eq!(account.auth, crate::llm::AuthScheme::ClineAccount);
        // The pasted-key provider next door stays a key: two rows, two ways
        // in, exactly as the Anthropic pair works.
        let keyed = file.get("cline").expect("shipped");
        assert_eq!(keyed.auth, crate::llm::AuthScheme::ApiKey);
        assert_eq!(keyed.key_env("cline"), "CLINE_API_KEY");
    }

    #[test]
    fn a_rotation_write_back_never_overwrites_a_newer_sign_in() {
        let path = std::env::temp_dir().join(format!(
            "alter-zero-cline-rotation-{}-{:?}.env",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::write(&path, format!("{REFRESH_ENV_VAR}=original\nOTHER=keep\n")).unwrap();
        // This session's own chain: the write lands.
        assert!(persist_refresh_to(&path, "rotated", &["original"]));
        // A sign-in landed meanwhile: the rotation of the replaced chain
        // must not take the store back to a retired token.
        std::fs::write(&path, format!("{REFRESH_ENV_VAR}=newer-sign-in\n")).unwrap();
        assert!(!persist_refresh_to(&path, "stale-rotation", &["original"]));
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("newer-sign-in"));
        assert!(!text.contains("stale-rotation"));
        // And with no token stored at all (a credential from the process
        // environment), the write lands so the next launch finds one.
        std::fs::write(&path, "OTHER=keep\n").unwrap();
        assert!(persist_refresh_to(&path, "first-write", &["original"]));
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .contains("first-write")
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_warm_cache_answers_without_refreshing_again() {
        let token = format!("cache-test-{}", std::process::id());
        let mut calls = 0;
        let access = authorize_with(&token, |presented| {
            calls += 1;
            assert_eq!(presented, token);
            Ok(AuthData {
                access_token: "eyJx.eyJy.s".to_string(),
                expires_at: Some(iso_in(3600)),
                ..AuthData::default()
            })
        })
        .unwrap();
        assert_eq!(calls, 1);
        assert_eq!(access.bearer, "workos:eyJx.eyJy.s");
        // The second resolve is a warm hit: no second exchange.
        let again = authorize_with(&token, |_| {
            calls += 1;
            unreachable!("a warm cache must not refresh");
        })
        .unwrap();
        assert_eq!(again.bearer, access.bearer);
        assert_eq!(calls, 1);
        forget(&token);
    }

    #[test]
    fn a_rotated_refresh_token_is_not_presented_before_itself() {
        let stored = format!("rotation-test-{}", std::process::id());
        let mut presentations = Vec::new();
        let first = authorize_with(&stored, |presented| {
            presentations.push(presented.to_string());
            Ok(AuthData {
                access_token: "eyJx.eyJy.one".to_string(),
                refresh_token: Some("rotated-1".to_string()),
                // Already expired: never cached, so the next resolve misses
                // and presents the rotated token this one noted.
                expires_at: Some(iso_in(-3600)),
                ..AuthData::default()
            })
        })
        .unwrap();
        assert_eq!(first.bearer, "workos:eyJx.eyJy.one");
        let second = authorize_with(&stored, |presented| {
            presentations.push(presented.to_string());
            Ok(AuthData {
                access_token: "eyJx.eyJy.two".to_string(),
                expires_at: Some(iso_in(3600)),
                ..AuthData::default()
            })
        })
        .unwrap();
        assert_eq!(second.bearer, "workos:eyJx.eyJy.two");
        // The config still holds `stored`; the second refresh must present
        // what the first rotation turned it into.
        assert_eq!(presentations, vec![stored.clone(), "rotated-1".to_string()]);
        forget(&stored);
        forget("rotated-1");
    }
}
