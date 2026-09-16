//! OpenAI's ChatGPT seat as a provider: the browser PKCE sign-in, its
//! device-code twin for a headless machine, the token layers every request
//! needs, and the request identity the ChatGPT backend insists on. See
//! `docs/chatgpt.md`.
//!
//! Two token layers, the same split [`super::copilot`] keeps for GitHub:
//!
//! 1. The **refresh token** the sign-in mints. Long-lived, persisted in the
//!    `.env` key store like any other provider's secret — and **rotated**:
//!    OpenAI may hand back a new one with each use, and a reused refresh
//!    token is terminal, so the new value is written straight back.
//! 2. The **access token** — a JWT the API actually takes, good for about an
//!    hour. Minted from (1) on demand, cached in memory, never written.
//!
//! The account id the backend routes on rides in that access token's own
//! claims, so nothing beyond the refresh token has to be stored.
//!
//! The pure halves (the claim parse, the URL/body builders, the device
//! flow's shapes and verdicts, the freshness rule) are unit-tested; the HTTP
//! calls and the loopback listener are boundary code.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::Deserialize;

use super::mcp::{form_encode, parse_redirect, pkce_and_state};
use super::{LlmError, Result};
use crate::stream::CancelToken;

/// The OAuth client id the Codex CLI uses — a **public** identifier (a PKCE
/// flow has no client secret). There is no third-party registration path for
/// this API, so every client that speaks it presents this id; OpenAI's
/// redirect allow-list is pinned to it.
pub const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";

/// Where the flows' calls go, unless [`ISSUER_ENV`] says otherwise.
pub const DEFAULT_ISSUER: &str = "https://auth.openai.com";

/// The environment variable that points both sign-in flows at another
/// issuer — the smoke suite's local stand-in for OpenAI's auth server
/// (`smoke.sh` Phase 119), or a fork's own. Read at the boundary and handed
/// in once through [`set_issuer`], the `set_store_path` pattern: the pure
/// builders never read the environment themselves.
pub const ISSUER_ENV: &str = "ALTER_ZERO_OPENAI_ISSUER";

/// The issuer in force — [`DEFAULT_ISSUER`] until the boundary sets one.
fn issuer_slot() -> &'static Mutex<Option<String>> {
    static ISSUER: OnceLock<Mutex<Option<String>>> = OnceLock::new();
    ISSUER.get_or_init(|| Mutex::new(None))
}

/// Point both sign-in flows (and the token endpoint they share) at `url`
/// instead of OpenAI's own auth server. Boundary-only; called once at
/// startup when [`ISSUER_ENV`] is set. A trailing slash is dropped so the
/// paths built on it never double one up.
pub fn set_issuer(url: impl Into<String>) {
    let url = url.into();
    let trimmed = url.trim().trim_end_matches('/').to_string();
    if let Ok(mut slot) = issuer_slot().lock() {
        *slot = (!trimmed.is_empty()).then_some(trimmed);
    }
}

/// The issuer the flows build their URLs on.
fn issuer() -> String {
    issuer_slot()
        .lock()
        .ok()
        .and_then(|slot| slot.clone())
        .unwrap_or_else(|| DEFAULT_ISSUER.to_string())
}

/// The loopback port the redirect URI names. **Not** a free choice: OpenAI
/// allow-lists exactly these two against [`CLIENT_ID`], so a port of our own
/// is refused at the authorize step. [`FALLBACK_PORT`] is tried when the
/// first is already bound (another sign-in, or a stale one).
const DEFAULT_PORT: u16 = 1455;
const FALLBACK_PORT: u16 = 1457;

/// The scopes requested — the minimum this client actually uses.
/// `offline_access` is the one that earns a refresh token; the connectors
/// scopes Codex additionally asks for buy nothing here.
const SCOPE: &str = "openid profile email offline_access";

/// The client identity every request carries. `originator` is **server-side
/// allow-listed**: a value OpenAI does not know earns a `403` whose message
/// does not say so. The `User-Agent` follows the same shape Codex sends.
pub const ORIGINATOR: &str = "codex_cli_rs";

/// The Codex client version this client presents as — and **not** a
/// decoration. The `/models` listing takes it as a required query parameter
/// and serves only the records whose own `minimal_client_version` it satisfies
/// (measured: `0.98.0` through `0.144.0` across one account's catalog). A
/// version below every record's gate is answered `{"models":[]}` on an HTTP
/// **200**, with nothing to say why — which is precisely how sending the
/// crate's own `CARGO_PKG_VERSION` (`0.1.0`) produced an empty `/model`
/// picker and no error.
///
/// So it is a **ceiling sentinel**, the value OpenAI's own release tooling
/// uses, rather than some real release number: OpenAI raises those per-record
/// gates as models ship, and a pinned real version would silently start
/// dropping models again — the same invisible failure, deferred. Nothing else
/// keys off it; the gate is the server's own filter, and each record still
/// declares what it needs.
pub const CLIENT_VERSION: &str = "99.99.99";

/// Per-operation deadline for the flow's requests — a small JSON round trip
/// each, with a user watching.
const OP_TIMEOUT: Duration = Duration::from_secs(30);

/// How long the sign-in page waits for the browser to come back before giving
/// up. Generous: a first sign-in may involve creating an account.
pub const AUTH_TIMEOUT: Duration = Duration::from_secs(600);

/// How long a device code stays valid — Codex's own fifteen minutes, which is
/// both the poll's deadline and what the page counts down from. OpenAI's
/// code response names no expiry of its own, unlike GitHub's.
pub const DEVICE_CODE_TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// Seconds between polls when the code response names no `interval`. Codex
/// defaults to zero there, which is a tight loop against the server; GitHub's
/// own device flow asks for five.
pub const DEVICE_DEFAULT_INTERVAL: u64 = 5;

/// The most any of the flow's responses may buffer — [`super::copilot`]'s
/// posture at the size these bodies actually are. It also bounds what a
/// *failure* can put on the sign-in page: an intercepting proxy answers with
/// an HTML page, and that page would otherwise become the "reason".
const BODY_MAX_BYTES: u64 = 64 * 1024;

/// Re-mint an access token this long before its own `exp`. Codex uses five
/// minutes; the same margin absorbs clock skew and a request already in
/// flight.
const REFRESH_SKEW: Duration = Duration::from_secs(300);

/// The **floor** on a cached access token's life. `exp` is an absolute time,
/// so a clock running ahead makes every token look already-expired and would
/// re-mint on every single request — the bug [`super::copilot`] avoids by
/// keying off a *duration* instead. There is no such duration here, so the
/// floor is the guard: a token is always cached at least this long.
const MIN_CACHE: Duration = Duration::from_secs(60);

// ---------------------------------------------------------------------------
// The token claims (pure)
// ---------------------------------------------------------------------------

/// What an OpenAI token's payload says about the seat. Read from the access
/// token itself, so a refresh keeps them current with no second store.
///
/// The signature is **never verified**: these claims route and label a request
/// we are already authorized to make (the server checks the token), they never
/// grant anything.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Claims {
    /// The workspace/account the backend routes on — the `chatgpt-account-id`
    /// header. Absent on a token that carries no auth claims.
    pub account_id: Option<String>,
    /// The raw plan slug (`plus`, `pro`, `team`, …), as sent.
    pub plan: Option<String>,
    /// The account's email, when the token says.
    pub email: Option<String>,
    /// Whether this workspace must route through the FedRAMP edge.
    pub fedramp: bool,
    /// The token's own expiry, as a Unix timestamp.
    pub expires_at: Option<u64>,
}

/// The JWT payload shape, insofar as we read it. Every field defaults, so a
/// token that omits the auth namespace still parses.
#[derive(Debug, Default, Deserialize)]
struct RawClaims {
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    exp: Option<u64>,
    #[serde(rename = "https://api.openai.com/profile", default)]
    profile: Option<ProfileClaims>,
    #[serde(rename = "https://api.openai.com/auth", default)]
    auth: Option<AuthClaims>,
}

#[derive(Debug, Default, Deserialize)]
struct ProfileClaims {
    #[serde(default)]
    email: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct AuthClaims {
    #[serde(default)]
    chatgpt_account_id: Option<String>,
    #[serde(default)]
    chatgpt_plan_type: Option<String>,
    #[serde(default)]
    chatgpt_account_is_fedramp: bool,
}

/// Decode a JWT's payload segment. Pure — no signature check (see [`Claims`]).
///
/// # Errors
/// A token that isn't three dot-separated segments, or whose payload is not
/// base64url JSON, is a decode error.
pub fn parse_claims(jwt: &str) -> Result<Claims> {
    let mut parts = jwt.split('.');
    let payload = match (parts.next(), parts.next(), parts.next()) {
        (Some(header), Some(payload), Some(sig))
            if !header.is_empty() && !payload.is_empty() && !sig.is_empty() =>
        {
            payload
        }
        _ => return Err(LlmError::Decode("not a JWT".to_string())),
    };
    let bytes = b64url_decode(payload).ok_or_else(|| LlmError::Decode("bad JWT payload".into()))?;
    let raw: RawClaims =
        serde_json::from_slice(&bytes).map_err(|e| LlmError::Decode(e.to_string()))?;
    let auth = raw.auth.unwrap_or_default();
    Ok(Claims {
        account_id: auth.chatgpt_account_id.filter(|a| !a.is_empty()),
        plan: auth.chatgpt_plan_type.filter(|p| !p.is_empty()),
        email: raw
            .email
            .or_else(|| raw.profile.and_then(|p| p.email))
            .filter(|e| !e.is_empty()),
        fedramp: auth.chatgpt_account_is_fedramp,
        expires_at: raw.exp,
    })
}

/// Decode base64url (RFC 4648 §5), padded or not — the inverse of the
/// `mcp::b64url` encoder. `None` on any character outside the alphabet.
#[must_use]
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

/// The plan slug as a label to show. Only the spellings that read badly
/// verbatim are mapped; anything else is title-cased word by word, so a plan
/// invented after this build still reads like a name instead of vanishing.
#[must_use]
pub fn plan_label(raw: &str) -> String {
    match raw.to_ascii_lowercase().as_str() {
        "prolite" => return "Pro Lite".to_string(),
        "ent26" | "enterprise" | "hc" => return "Enterprise".to_string(),
        "education" | "edu" => return "Edu".to_string(),
        _ => {}
    }
    raw.split(['_', '-'])
        .filter(|w| !w.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

// ---------------------------------------------------------------------------
// The flow's URLs and bodies (pure)
// ---------------------------------------------------------------------------

/// The redirect URI for a bound port. **`localhost`, not `127.0.0.1`**: the
/// allow-listed string is the hostname one, even though the listener binds
/// the literal address (binding `localhost` can resolve to `::1` and miss the
/// browser's connection).
#[must_use]
pub fn redirect_uri(port: u16) -> String {
    format!("http://localhost:{port}/auth/callback")
}

/// The authorize URL to send the user to.
#[must_use]
pub fn authorize_url(challenge: &str, state: &str, port: u16) -> String {
    let query = form_encode(&[
        ("response_type".to_string(), "code".to_string()),
        ("client_id".to_string(), CLIENT_ID.to_string()),
        ("redirect_uri".to_string(), redirect_uri(port)),
        ("scope".to_string(), SCOPE.to_string()),
        ("code_challenge".to_string(), challenge.to_string()),
        ("code_challenge_method".to_string(), "S256".to_string()),
        ("id_token_add_organizations".to_string(), "true".to_string()),
        ("codex_cli_simplified_flow".to_string(), "true".to_string()),
        ("state".to_string(), state.to_string()),
        ("originator".to_string(), ORIGINATOR.to_string()),
    ]);
    format!("{}/oauth/authorize?{query}", issuer())
}

/// The token endpoint every grant posts to — the browser flow's code, the
/// device flow's code, and the refresh alike.
#[must_use]
fn token_url() -> String {
    format!("{}/oauth/token", issuer())
}

/// The authorization-code grant's body. **Form-encoded** — the refresh grant
/// on the *same endpoint* takes JSON instead, and swapping the two earns an
/// opaque 400.
#[must_use]
pub fn code_exchange_body(code: &str, verifier: &str, redirect_uri: &str) -> String {
    form_encode(&[
        ("grant_type".to_string(), "authorization_code".to_string()),
        ("code".to_string(), code.to_string()),
        ("redirect_uri".to_string(), redirect_uri.to_string()),
        ("client_id".to_string(), CLIENT_ID.to_string()),
        ("code_verifier".to_string(), verifier.to_string()),
    ])
}

/// The refresh grant's body — **JSON**, see [`code_exchange_body`].
#[must_use]
pub fn refresh_body(refresh_token: &str) -> serde_json::Value {
    serde_json::json!({
        "client_id": CLIENT_ID,
        "grant_type": "refresh_token",
        "refresh_token": refresh_token,
    })
}

/// What a token response carried. Every field is optional on a refresh (the
/// server re-sends only what changed), so a missing `refresh_token` means
/// "keep the one you have" rather than "you have none".
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct TokenSet {
    #[serde(default)]
    pub access_token: Option<String>,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub id_token: Option<String>,
}

impl TokenSet {
    /// Parse a token response.
    ///
    /// # Errors
    /// A body that isn't an object, or that carries no access token, is a
    /// decode error — there is nothing to authenticate with either way.
    pub fn parse(body: &str) -> Result<Self> {
        let parsed: Self =
            serde_json::from_str(body).map_err(|e| LlmError::Decode(e.to_string()))?;
        if parsed.access_token.as_ref().is_none_or(|t| t.is_empty()) {
            return Err(LlmError::Decode(
                "OpenAI returned no access token".to_string(),
            ));
        }
        Ok(parsed)
    }
}

/// How long an access token expiring at `expires_at` may be cached, given the
/// wall clock `now` (both Unix seconds). A five-minute `REFRESH_SKEW` comes off
/// the end; `MIN_CACHE` is the floor that keeps a skewed clock from re-minting
/// on every request.
#[must_use]
pub fn cache_lifetime(expires_at: Option<u64>, now: u64) -> Duration {
    let Some(exp) = expires_at else {
        // A token that doesn't say: trust it for a conservative single minute
        // rather than forever.
        return MIN_CACHE;
    };
    let remaining = Duration::from_secs(exp.saturating_sub(now));
    remaining.saturating_sub(REFRESH_SKEW).max(MIN_CACHE)
}

/// Explain a sign-in or refresh failure in terms the user can act on. The
/// wire bodies name an `error` code and little else, and the three that mean
/// "your refresh token is gone" all need the same answer: sign in again.
#[must_use]
pub fn auth_advice(status: u16, body: &str) -> Option<String> {
    let code = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| {
            v.get("error").and_then(|e| {
                e.as_str().map(str::to_string).or_else(|| {
                    e.get("code")
                        .or_else(|| e.get("type"))
                        .and_then(|c| c.as_str())
                        .map(str::to_string)
                })
            })
        })
        .unwrap_or_default();
    match code.as_str() {
        "refresh_token_expired"
        | "refresh_token_reused"
        | "refresh_token_invalidated"
        | "invalid_grant" => {
            Some("Your ChatGPT sign-in has expired. Run /login and sign in again.".to_string())
        }
        _ if status == 401 => {
            Some("OpenAI rejected the ChatGPT sign-in. Run /login and sign in again.".to_string())
        }
        _ if status == 403 => Some(
            "OpenAI refused this request. A ChatGPT plan that includes Codex is needed."
                .to_string(),
        ),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// The device-code flow's shapes (pure) — docs/chatgpt.md
// ---------------------------------------------------------------------------

/// Where the device-code flow's four calls go, off one issuer: Codex's own
/// paths, verbatim. The code request and the poll live under
/// `/api/accounts/deviceauth`; the page the user types the code at is
/// `/codex/device`; and the grant is bound to `/deviceauth/callback`, which
/// the exchange must repeat even though nothing ever listens there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceEndpoints {
    /// `POST` — ask for a code.
    pub usercode: String,
    /// `POST` — poll for the grant.
    pub token: String,
    /// Where the user enters the code (shown on the page).
    pub verification: String,
    /// The redirect URI the exchange repeats verbatim.
    pub redirect_uri: String,
}

impl DeviceEndpoints {
    /// The endpoints for `issuer` — a trailing slash tolerated, so a stub's
    /// `http://127.0.0.1:8080/` builds the same paths as OpenAI's own.
    #[must_use]
    pub fn for_issuer(issuer: &str) -> Self {
        let base = issuer.trim_end_matches('/');
        Self {
            usercode: format!("{base}/api/accounts/deviceauth/usercode"),
            token: format!("{base}/api/accounts/deviceauth/token"),
            verification: format!("{base}/codex/device"),
            redirect_uri: format!("{base}/deviceauth/callback"),
        }
    }
}

/// The page the user enters a device code at, on the issuer in force.
#[must_use]
pub fn device_verification_url() -> String {
    DeviceEndpoints::for_issuer(&issuer()).verification
}

/// The code request's body: the client id and nothing else — the server
/// mints the PKCE pair itself and hands it back with the grant.
#[must_use]
pub fn device_code_request_body() -> serde_json::Value {
    serde_json::json!({ "client_id": CLIENT_ID })
}

/// What the code request answered: the pair the poll presents, and how often
/// to present it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceCode {
    /// The server's handle on this attempt — presented by every poll, never
    /// shown.
    device_auth_id: String,
    /// The short code the user types at [`DeviceEndpoints::verification`].
    user_code: String,
    /// Seconds between polls, as sent; `None` when the response names none.
    interval: Option<u64>,
}

/// The wire shape of the code response. The interval arrives as a
/// **string** (`"5"`) from the reference server, so both spellings are
/// accepted; either name of the code is, too, since the reference reads both.
#[derive(Debug, Deserialize)]
struct RawDeviceCode {
    device_auth_id: String,
    #[serde(alias = "usercode")]
    user_code: String,
    #[serde(default)]
    interval: Option<RawInterval>,
}

/// A number, or a string holding one.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RawInterval {
    Seconds(u64),
    Text(String),
}

impl RawInterval {
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
    /// A body that isn't the response's shape, or that names no id or no
    /// code, is a decode error — there is nothing to poll with either way.
    pub fn parse(body: &str) -> Result<Self> {
        let raw: RawDeviceCode =
            serde_json::from_str(body).map_err(|e| LlmError::Decode(e.to_string()))?;
        if raw.device_auth_id.is_empty() || raw.user_code.is_empty() {
            return Err(LlmError::Decode(
                "OpenAI returned no device code".to_string(),
            ));
        }
        Ok(Self {
            device_auth_id: raw.device_auth_id,
            user_code: raw.user_code,
            interval: raw.interval.and_then(|i| i.seconds()),
        })
    }

    /// The code to show and copy.
    #[must_use]
    pub fn user_code(&self) -> &str {
        &self.user_code
    }

    /// How long to wait between polls: what the server asked, floored at a
    /// second (a zero would be a tight loop), [`DEVICE_DEFAULT_INTERVAL`]
    /// when it asked nothing.
    #[must_use]
    pub fn poll_interval(&self) -> Duration {
        Duration::from_secs(self.interval.unwrap_or(DEVICE_DEFAULT_INTERVAL).max(1))
    }
}

/// The poll's body: the pair the code request issued.
#[must_use]
pub fn device_poll_body(device: &DeviceCode) -> serde_json::Value {
    serde_json::json!({
        "device_auth_id": device.device_auth_id,
        "user_code": device.user_code,
    })
}

/// What an approved poll carries: an authorization code and the PKCE pair
/// the **server** minted for it, whose verifier the exchange repeats.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct DeviceGrant {
    /// The code to redeem at the token endpoint.
    pub authorization_code: String,
    /// The challenge the server bound the code to (informational here).
    #[serde(default)]
    pub code_challenge: String,
    /// The verifier the exchange must present.
    pub code_verifier: String,
}

/// One poll's outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DevicePoll {
    /// Not approved yet — ask again after the interval.
    Pending,
    /// Approved: redeem this.
    Approved(DeviceGrant),
    /// The flow is over — the sentence to show.
    Failed(String),
}

/// Read a poll response the way Codex does: `403` and `404` both mean "not
/// yet" (the server answers a pending code with either), a `2xx` carries the
/// grant, and anything else ends the flow.
#[must_use]
pub fn device_poll_verdict(status: u16, body: &str) -> DevicePoll {
    match status {
        403 | 404 => DevicePoll::Pending,
        200..=299 => match serde_json::from_str::<DeviceGrant>(body) {
            Ok(grant) if !grant.authorization_code.is_empty() => DevicePoll::Approved(grant),
            _ => DevicePoll::Failed("OpenAI approved the code but sent no grant".to_string()),
        },
        _ => DevicePoll::Failed(format!("OpenAI answered {status} to the device code poll")),
    }
}

/// Explain a failed code request in terms the user can act on. A `404` is
/// Codex's own reading: device code login is not enabled for this server —
/// and the other row is one Esc away, so name it.
#[must_use]
pub fn device_code_advice(status: u16, body: &str) -> Option<String> {
    match status {
        404 => Some(
            "OpenAI's device code sign-in is not available right now — press Esc and choose Browser login instead."
                .to_string(),
        ),
        _ => auth_advice(status, body),
    }
}

// ---------------------------------------------------------------------------
// The access-token cache (boundary state)
// ---------------------------------------------------------------------------

/// One minted access token and what it is good for.
#[derive(Debug, Clone)]
pub struct Access {
    /// The bearer the request carries.
    pub bearer: String,
    /// The account the backend routes on (`chatgpt-account-id`).
    pub account_id: Option<String>,
    /// The seat's plan, for the sign-in confirmation.
    pub plan: Option<String>,
    /// Whether this account needs the FedRAMP edge header.
    pub fedramp: bool,
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

/// The **freshest** refresh token for one the session is still holding.
///
/// Rotation is not only a disk concern: the live [`ModelConfig`] carries the
/// token the session started with, and nothing reloads it mid-run — so once
/// OpenAI retires that token, the *next* refresh would present a dead one and
/// earn the terminal `refresh_token_reused`, killing a working session about
/// an hour in. This map is the in-memory half of the same fix
/// `persist_refresh` makes on disk: it remembers what each retired token
/// became, so a config holding the old value keeps refreshing successfully.
///
/// [`ModelConfig`]: super::ModelConfig
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
        map.insert(stored.to_string(), rotated.to_string());
        if presented != stored {
            map.insert(presented.to_string(), rotated.to_string());
        }
    }
}

/// Where a rotated refresh token is written back. Set once at the boundary
/// (`tui::bootstrap`) because the rotation happens deep on a backend thread:
/// OpenAI may retire a refresh token as it is used, and keeping the retired
/// one on disk turns the next launch into a forced re-login.
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
    if let Ok(mut map) = cache().lock() {
        map.remove(refresh_token);
    }
    // The chain too: a fresh sign-in mints a token unrelated to whatever the
    // old one had rotated into, and following a stale link would present a
    // token from the account the user just replaced.
    if let Ok(mut map) = rotations().lock() {
        map.remove(refresh_token);
    }
}

/// Wall-clock seconds since the epoch — the one impure read the freshness
/// rule needs (the rule itself is [`cache_lifetime`]).
fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

// ---------------------------------------------------------------------------
// The sign-in flow and the token mint (boundary — real HTTP and a listener)
// ---------------------------------------------------------------------------

/// Read a response body, bounded, and trim it to something showable — the
/// boundary half of the same decision [`auth_advice`] makes.
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
            format!("OpenAI answered {status}")
        } else {
            format!("OpenAI answered {status}: {trimmed}")
        }
    });
    LlmError::Api {
        status,
        body: explained,
    }
}

/// Bind the loopback listener on the one port OpenAI allow-lists, falling
/// back to the second. Returns the listener and the port it actually took —
/// a port of our own choosing would be refused at the authorize step, so a
/// second sign-in already holding both is a real failure, not a retry.
fn bind_listener() -> Result<(TcpListener, u16)> {
    for port in [DEFAULT_PORT, FALLBACK_PORT] {
        if let Ok(listener) = TcpListener::bind(("127.0.0.1", port)) {
            return Ok((listener, port));
        }
    }
    Err(LlmError::Http(format!(
        "ports {DEFAULT_PORT} and {FALLBACK_PORT} are both in use — close the other sign-in and try again"
    )))
}

/// What the browser is shown once the code is in hand. Plain and self-closing
/// — the terminal is where the flow continues.
const SUCCESS_PAGE: &str = "<html><body style=\"font-family:system-ui;padding:3rem\"><h3>Signed in to ChatGPT.</h3><p>You can close this tab and return to the terminal.</p></body></html>";

/// How long one nap between cancel checks lasts, matching the device flow's
/// cadence so Esc is honoured about as fast here as there.
const POLL_NAP: Duration = Duration::from_millis(200);

/// The sign-in page's live state, reported as the flow runs.
pub struct Signin {
    /// The URL to open — shown on the page and copyable.
    pub url: String,
    /// The listener the browser comes back to.
    listener: TcpListener,
    /// The PKCE verifier the exchange redeems the code with.
    verifier: String,
    /// The `state` the callback must echo.
    state: String,
    /// The redirect URI the exchange must repeat verbatim.
    redirect_uri: String,
}

/// Open the loopback listener and build the URL to send the user to. The
/// caller shows [`Signin::url`], then blocks in [`await_callback`].
///
/// # Errors
/// A port neither of the two allow-listed numbers can take.
pub fn begin_signin() -> Result<Signin> {
    let (listener, port) = bind_listener()?;
    let (verifier, challenge, state) = pkce_and_state();
    Ok(Signin {
        url: authorize_url(&challenge, &state, port),
        listener,
        verifier,
        state,
        redirect_uri: redirect_uri(port),
    })
}

/// Wait for the browser's callback, then redeem the code. Returns the refresh
/// token to store and the seat it turned out to be.
///
/// # Errors
/// A cancelled flow, a timeout, a `state` that doesn't match, or a token
/// exchange OpenAI refused.
pub fn await_callback(signin: &Signin, cancel: &CancelToken) -> Result<(String, Access)> {
    let code = wait_for_code(&signin.listener, &signin.state, cancel)?;
    exchange_code(&code, &signin.verifier, &signin.redirect_uri)
}

/// Redeem an authorization code for the token set — the one exchange both
/// sign-ins end in, differing only in whose PKCE verifier and which redirect
/// they repeat. Returns the refresh token to store and the seat it turned
/// out to be.
///
/// # Errors
/// A token exchange OpenAI refused, or a token set with no refresh token.
fn exchange_code(code: &str, verifier: &str, redirect_uri: &str) -> Result<(String, Access)> {
    let body = code_exchange_body(code, verifier, redirect_uri);
    let client = super::http_client(OP_TIMEOUT)?;
    let resp = client
        .post(token_url())
        .header("content-type", "application/x-www-form-urlencoded")
        .header("accept", "application/json")
        .body(body)
        .send()
        .map_err(|e| LlmError::Http(e.to_string()))?;
    let status = resp.status().as_u16();
    let text = read_body(resp);
    if !(200..300).contains(&status) {
        return Err(token_failure(status, &text));
    }
    let tokens = TokenSet::parse(&text)?;
    let refresh = tokens
        .refresh_token
        .clone()
        .filter(|t| !t.is_empty())
        .ok_or_else(|| {
            // Without one the session would die at the first token expiry, an
            // hour in, with no way back but another sign-in — better to say so now.
            LlmError::Decode("OpenAI returned no refresh token — sign in again".to_string())
        })?;
    let access = access_from(&tokens)?;
    Ok((refresh, access))
}

/// Ask OpenAI for a device code. The caller shows [`DeviceCode::user_code`]
/// beside [`device_verification_url`], then blocks in
/// [`await_device_approval`]. Codex's client identity rides the request as
/// it rides every other call to this issuer.
///
/// # Errors
/// A code request OpenAI refused — a `404` meaning the flow is not offered
/// here, which the message says — or a transport failure.
pub fn request_device_code() -> Result<DeviceCode> {
    let endpoints = DeviceEndpoints::for_issuer(&issuer());
    let client = super::http_client(OP_TIMEOUT)?;
    let resp = client
        .post(endpoints.usercode)
        .header("content-type", "application/json")
        .header("accept", "application/json")
        .header("originator", ORIGINATOR)
        .header("user-agent", user_agent())
        .json(&device_code_request_body())
        .send()
        .map_err(|e| LlmError::Http(e.to_string()))?;
    let status = resp.status().as_u16();
    let text = read_body(resp);
    if !(200..300).contains(&status) {
        let explained = device_code_advice(status, &text).unwrap_or_else(|| {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                format!("OpenAI answered {status} to the device code request")
            } else {
                format!("OpenAI answered {status}: {trimmed}")
            }
        });
        return Err(LlmError::Api {
            status,
            body: explained,
        });
    }
    DeviceCode::parse(&text)
}

/// Poll until the user approves `device`'s code on OpenAI's page, then redeem
/// the grant it carries. Blocks for as long as that takes — up to
/// [`DEVICE_CODE_TIMEOUT`] — napping on the transport's cadence so Esc on
/// the page is honoured promptly. Returns the refresh token to store and the
/// seat it turned out to be.
///
/// # Errors
/// A cancelled flow, an expired code, a poll OpenAI refused, or a token
/// exchange it refused.
pub fn await_device_approval(
    device: &DeviceCode,
    cancel: &CancelToken,
) -> Result<(String, Access)> {
    let endpoints = DeviceEndpoints::for_issuer(&issuer());
    let client = super::http_client(OP_TIMEOUT)?;
    let deadline = Instant::now() + DEVICE_CODE_TIMEOUT;
    let interval = device.poll_interval();
    let body = device_poll_body(device);
    loop {
        // Sleep *first*: the user has not even read the code yet.
        if !nap(interval, cancel) {
            return Err(LlmError::Cancelled);
        }
        if Instant::now() >= deadline {
            return Err(LlmError::Http(
                "The code expired — press Esc and sign in again.".to_string(),
            ));
        }
        let resp = client
            .post(&endpoints.token)
            .header("content-type", "application/json")
            .header("accept", "application/json")
            .header("originator", ORIGINATOR)
            .header("user-agent", user_agent())
            .json(&body)
            .send()
            .map_err(|e| LlmError::Http(format!("Could not reach OpenAI: {e}")))?;
        let status = resp.status().as_u16();
        let text = read_body(resp);
        match device_poll_verdict(status, &text) {
            DevicePoll::Pending => {}
            DevicePoll::Approved(grant) => {
                return exchange_code(
                    &grant.authorization_code,
                    &grant.code_verifier,
                    &endpoints.redirect_uri,
                );
            }
            DevicePoll::Failed(reason) => {
                return Err(LlmError::Api {
                    status,
                    body: reason,
                });
            }
        }
    }
}

/// Sleep `total` in [`POLL_NAP`] slices, returning `false` the moment
/// `cancel` trips — [`super::copilot`]'s nap, for the same reason.
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

/// Block until the callback arrives on `listener`, answering the browser
/// either way. Polls `cancel` on the same cadence a stream does, so Esc on
/// the sign-in page is honoured promptly.
fn wait_for_code(listener: &TcpListener, state: &str, cancel: &CancelToken) -> Result<String> {
    listener
        .set_nonblocking(true)
        .map_err(|e| LlmError::Http(format!("callback port: {e}")))?;
    let started = Instant::now();
    loop {
        if cancel.is_cancelled() {
            return Err(LlmError::Cancelled);
        }
        if started.elapsed() > AUTH_TIMEOUT {
            return Err(LlmError::Http(
                "timed out waiting for the browser to come back".to_string(),
            ));
        }
        match listener.accept() {
            Ok((mut stream, _)) => {
                let mut buffer = [0u8; 8192];
                let read = stream.read(&mut buffer).unwrap_or(0);
                let request = String::from_utf8_lossy(&buffer[..read]);
                let path = request
                    .lines()
                    .next()
                    .and_then(|line| line.split_whitespace().nth(1))
                    .unwrap_or_default();
                let parsed = parse_redirect(path);
                let _ = stream.write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{SUCCESS_PAGE}",
                        SUCCESS_PAGE.len()
                    )
                    .as_bytes(),
                );
                if let Some((code, got_state)) = parsed {
                    if got_state.as_deref().is_some_and(|got| got != state) {
                        return Err(LlmError::Http(
                            "the callback's state doesn't match — start the sign-in again".into(),
                        ));
                    }
                    return Ok(code);
                }
                // Anything else on the port (a favicon fetch, a stale tab) is
                // not the callback — keep waiting rather than failing the flow.
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(POLL_NAP);
            }
            Err(e) => return Err(LlmError::Http(format!("callback port: {e}"))),
        }
    }
}

/// The [`Access`] a token response describes — its claims read off the access
/// token itself, which is why nothing but the refresh token is ever stored.
fn access_from(tokens: &TokenSet) -> Result<Access> {
    let bearer = tokens
        .access_token
        .clone()
        .ok_or_else(|| LlmError::Decode("OpenAI returned no access token".to_string()))?;
    // A token whose claims won't parse still authenticates — it is the
    // *server* that validates it. Only the routing detail is lost, so fall
    // back to empty claims rather than failing a usable sign-in.
    let claims = parse_claims(&bearer).unwrap_or_default();
    Ok(Access {
        bearer,
        account_id: claims.account_id,
        plan: claims.plan,
        fedramp: claims.fedramp,
    })
}

/// The access token to authenticate with, minted from `refresh_token` and
/// cached until shortly before its own expiry. Boundary code — one HTTP round
/// trip on a cache miss, none on a hit.
///
/// A rotated refresh token is written back to the key store before this
/// returns: OpenAI may retire the one just used, and leaving the retired
/// value on disk turns the next launch into a forced re-login.
///
/// # Errors
/// A refresh OpenAI refused, or a transport failure.
pub fn authorize(refresh_token: &str) -> Result<Access> {
    if let Ok(map) = cache().lock()
        && let Some(entry) = map.get(refresh_token)
        && entry.until > Instant::now()
    {
        return Ok(entry.access.clone());
    }
    // What to actually present: the stored token, or whatever a previous
    // rotation turned it into. The config still holds the original.
    let presented = live_refresh_token(refresh_token);
    let client = super::http_client(OP_TIMEOUT)?;
    let resp = client
        .post(token_url())
        .header("content-type", "application/json")
        .header("accept", "application/json")
        .header("originator", ORIGINATOR)
        .header("user-agent", user_agent())
        .json(&refresh_body(&presented))
        .send()
        .map_err(|e| LlmError::Http(e.to_string()))?;
    let status = resp.status().as_u16();
    let text = read_body(resp);
    if !(200..300).contains(&status) {
        return Err(token_failure(status, &text));
    }
    let tokens = TokenSet::parse(&text)?;
    let access = access_from(&tokens)?;
    let lifetime = cache_lifetime(
        parse_claims(&access.bearer).ok().and_then(|c| c.expires_at),
        unix_now(),
    );
    if let Ok(mut map) = cache().lock() {
        map.insert(
            refresh_token.to_string(),
            Cached {
                access: access.clone(),
                until: Instant::now() + lifetime,
            },
        );
    }
    // Rotation: remember what the retired token became — on disk for the next
    // launch, and in memory for this session, whose config still holds the
    // original — and cache the access token under the new key too, so a
    // config that *has* been rebuilt hits rather than re-minting.
    if let Some(rotated) = tokens
        .refresh_token
        .filter(|t| !t.is_empty() && *t != presented)
    {
        persist_refresh(&rotated);
        note_rotation(refresh_token, &presented, &rotated);
        if let Ok(mut map) = cache().lock() {
            map.insert(
                rotated,
                Cached {
                    access: access.clone(),
                    until: Instant::now() + lifetime,
                },
            );
        }
    }
    Ok(access)
}

/// Write a rotated refresh token back into the `.env` key store, in place.
/// Best-effort: a store we cannot write still leaves a working session, and
/// the failure surfaces as a re-login at the next launch rather than a
/// mid-turn error the user can do nothing about.
fn persist_refresh(refresh_token: &str) {
    let Some(path) = store_path().lock().ok().and_then(|p| p.clone()) else {
        return;
    };
    let current = std::fs::read_to_string(&path).unwrap_or_default();
    let updated = super::EnvFile::upsert(&current, REFRESH_ENV_VAR, refresh_token);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if std::fs::write(&path, updated).is_ok() {
        // The store holds plaintext secrets. An existing file keeps the mode
        // it was created with, but a rotation is also the one write that can
        // *create* it — a real `OPENAI_CHATGPT_REFRESH_TOKEN` in the process
        // environment resolves with no `.env` on disk at all — and creating
        // it world-readable would be a quiet downgrade of the store the
        // `/login` path takes care to lock down.
        set_owner_only(&path);
    }
}

/// `0600` on the key store — the same guard `/login`'s own write applies, and
/// the same one the MCP token store uses.
fn set_owner_only(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// The environment variable the refresh token lives under — the same name
/// `providers.toml` gives the provider's `api_key_env`, pinned here because
/// the rotation write-back cannot ask the provider file.
pub const REFRESH_ENV_VAR: &str = "OPENAI_CHATGPT_REFRESH_TOKEN";

/// The per-request headers the ChatGPT backend keys its **prompt cache** on:
/// the session's cache-affinity key under Codex's two names, `session_id`
/// and `conversation_id`. Verified live — the body's `prompt_cache_key`
/// alone earned no cache reads on an identical 6.7k-token prefix, the same
/// request with these read 6.4k of it back, and `OpenAI-Beta` on its own
/// changed nothing (`docs/chatgpt.md`, `docs/prompt-caching.md`). Pure: no
/// key means no headers, since an empty session id is a *different*
/// (invalid) routing hint rather than none.
#[must_use]
pub fn session_headers(cache_key: Option<&str>) -> Vec<(String, String)> {
    cache_key
        .filter(|key| !key.is_empty())
        .map(|key| {
            vec![
                ("session_id".to_string(), key.to_string()),
                ("conversation_id".to_string(), key.to_string()),
            ]
        })
        .unwrap_or_default()
}

/// Codex's per-request routing hint header (`docs/fast-mode.md`).
pub const ROUTING_HINT_HEADER: &str = "x-codex-routing-hint";

/// The routing hint's value: `model={model}` on every request, and
/// `;tier={tier}` appended when a speed tier is selected — codex's
/// `build_routing_hint_header`, which sends the model half on every request
/// to this backend whether or not a tier is. Pure.
#[must_use]
pub fn routing_hint(model: &str, tier: Option<&str>) -> String {
    match tier {
        Some(tier) => format!("model={model};tier={tier}"),
        None => format!("model={model}"),
    }
}

/// The `User-Agent` the backend expects to see — Codex's shape, since the
/// client id is Codex's, carrying the same [`CLIENT_VERSION`] the `/models`
/// query does (a request whose two version claims disagreed would be a
/// needless thing to have a server notice). Built once per process.
#[must_use]
pub fn user_agent() -> &'static str {
    static AGENT: OnceLock<String> = OnceLock::new();
    AGENT.get_or_init(|| {
        format!(
            "{ORIGINATOR}/{CLIENT_VERSION} ({} {}; {}) {}",
            std::env::consts::OS,
            "0.0.0",
            std::env::consts::ARCH,
            crate::APP_NAME.to_ascii_lowercase().replace(' ', "_"),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A JWT whose payload is `claims` — header and signature are never read,
    /// so any non-empty pair will do.
    fn jwt(claims: serde_json::Value) -> String {
        let payload = crate::llm::mcp::b64url(claims.to_string().as_bytes());
        format!("eyJhbGciOiJub25lIn0.{payload}.sig")
    }

    // --- the token claims ---

    #[test]
    fn claims_read_the_account_the_backend_routes_on() {
        let token = jwt(serde_json::json!({
            "exp": 1_700_000_000u64,
            "email": "a@b.c",
            "https://api.openai.com/auth": {
                "chatgpt_account_id": "acct-123",
                "chatgpt_plan_type": "pro",
            }
        }));
        let claims = parse_claims(&token).unwrap();
        assert_eq!(claims.account_id.as_deref(), Some("acct-123"));
        assert_eq!(claims.plan.as_deref(), Some("pro"));
        assert_eq!(claims.email.as_deref(), Some("a@b.c"));
        assert_eq!(claims.expires_at, Some(1_700_000_000));
        assert!(!claims.fedramp);
    }

    #[test]
    fn claims_fall_back_to_the_profile_namespace_for_the_email() {
        let token = jwt(serde_json::json!({
            "https://api.openai.com/profile": {"email": "p@q.r"},
        }));
        assert_eq!(
            parse_claims(&token).unwrap().email.as_deref(),
            Some("p@q.r")
        );
    }

    #[test]
    fn a_token_without_auth_claims_still_parses() {
        // The token authenticates either way — it is the server that
        // validates it. Only the routing detail is missing.
        let claims = parse_claims(&jwt(serde_json::json!({"exp": 12u64}))).unwrap();
        assert_eq!(claims.account_id, None);
        assert_eq!(claims.plan, None);
        assert_eq!(claims.expires_at, Some(12));
    }

    #[test]
    fn a_fedramp_account_is_flagged() {
        let token = jwt(serde_json::json!({
            "https://api.openai.com/auth": {"chatgpt_account_is_fedramp": true},
        }));
        assert!(parse_claims(&token).unwrap().fedramp);
    }

    #[test]
    fn something_that_is_not_a_jwt_is_a_decode_error() {
        assert!(parse_claims("").is_err());
        assert!(parse_claims("one.two").is_err());
        assert!(parse_claims("a..c").is_err());
        assert!(parse_claims("a.!!!.c").is_err());
    }

    #[test]
    fn b64url_decode_round_trips_the_encoder() {
        for sample in [b"" as &[u8], b"f", b"fo", b"foo", b"foob", &[0xfb, 0xff]] {
            let encoded = crate::llm::mcp::b64url(sample);
            assert_eq!(
                b64url_decode(&encoded).as_deref(),
                Some(sample),
                "{encoded}"
            );
        }
        // Padded input decodes too — some issuers pad their JWT segments.
        assert_eq!(b64url_decode("Zm9v").as_deref(), Some(b"foo" as &[u8]));
        assert_eq!(b64url_decode("Zg==").as_deref(), Some(b"f" as &[u8]));
    }

    // --- the plan label ---

    #[test]
    fn plan_labels_read_like_names() {
        assert_eq!(plan_label("plus"), "Plus");
        assert_eq!(plan_label("pro"), "Pro");
        assert_eq!(plan_label("prolite"), "Pro Lite");
        assert_eq!(plan_label("hc"), "Enterprise");
        assert_eq!(plan_label("edu"), "Edu");
        assert_eq!(
            plan_label("self_serve_business_prolite"),
            "Self Serve Business Prolite"
        );
        // A plan invented after this build still reads as a name rather than
        // vanishing — the list churns, so unknown must degrade, not drop.
        assert_eq!(plan_label("quantum_max"), "Quantum Max");
    }

    // --- the flow's URLs and bodies ---

    #[test]
    fn the_authorize_url_carries_every_parameter_openai_expects() {
        let url = authorize_url("CHAL", "STATE", 1455);
        assert!(url.starts_with("https://auth.openai.com/oauth/authorize?"));
        for expected in [
            "response_type=code",
            "client_id=app_EMoamEEZ73f0CkXaXp7hrann",
            // The redirect must be the allow-listed `localhost` spelling,
            // percent-encoded.
            "redirect_uri=http%3A%2F%2Flocalhost%3A1455%2Fauth%2Fcallback",
            "code_challenge=CHAL",
            "code_challenge_method=S256",
            "id_token_add_organizations=true",
            "codex_cli_simplified_flow=true",
            "state=STATE",
            // Server-side allow-listed: a value OpenAI doesn't know is a 403.
            "originator=codex_cli_rs",
        ] {
            assert!(url.contains(expected), "missing {expected} in {url}");
        }
        assert!(
            url.contains("scope=openid%20profile%20email%20offline_access"),
            "offline_access is what earns a refresh token: {url}"
        );
    }

    #[test]
    fn the_redirect_uri_names_localhost_not_the_loopback_literal() {
        // The allow-listed string is the hostname one; the listener binds
        // 127.0.0.1 because binding "localhost" can land on ::1 and miss the
        // browser entirely.
        assert_eq!(redirect_uri(1455), "http://localhost:1455/auth/callback");
        assert_eq!(redirect_uri(1457), "http://localhost:1457/auth/callback");
    }

    #[test]
    fn the_code_grant_is_form_encoded_and_the_refresh_grant_is_json() {
        // Two grants, one endpoint, two content types. Swapping them earns an
        // opaque 400, so the shapes are pinned here.
        let form = code_exchange_body("CODE", "VERIFIER", "http://localhost:1455/auth/callback");
        assert!(form.starts_with("grant_type=authorization_code"));
        assert!(form.contains("&code=CODE"));
        assert!(form.contains("&code_verifier=VERIFIER"));
        assert!(form.contains("&client_id=app_EMoamEEZ73f0CkXaXp7hrann"));
        assert!(form.contains("&redirect_uri=http%3A%2F%2Flocalhost%3A1455%2Fauth%2Fcallback"));

        let json = refresh_body("REFRESH");
        assert_eq!(json["grant_type"], "refresh_token");
        assert_eq!(json["refresh_token"], "REFRESH");
        assert_eq!(json["client_id"], CLIENT_ID);
        // No scope on a refresh — the current server takes none.
        assert!(json.get("scope").is_none());
    }

    #[test]
    fn a_token_response_parses_and_a_tokenless_one_does_not() {
        let set = TokenSet::parse(
            r#"{"access_token":"at","refresh_token":"rt","id_token":"it","token_type":"Bearer"}"#,
        )
        .unwrap();
        assert_eq!(set.access_token.as_deref(), Some("at"));
        assert_eq!(set.refresh_token.as_deref(), Some("rt"));
        // A refresh may re-send only what changed: no refresh_token means
        // "keep the one you have", not "you have none".
        let refreshed = TokenSet::parse(r#"{"access_token":"at2"}"#).unwrap();
        assert_eq!(refreshed.refresh_token, None);
        assert!(TokenSet::parse(r#"{"error":"invalid_grant"}"#).is_err());
        assert!(TokenSet::parse("not json").is_err());
    }

    // --- the cache freshness rule ---

    // --- the per-request session headers ---

    #[test]
    fn the_session_headers_carry_the_cache_key_under_both_names() {
        // Verified live against the ChatGPT backend: the body's
        // `prompt_cache_key` alone earns *no* cache reads on an identical
        // 6.7k-token prefix, while the same request with Codex's
        // `session_id` + `conversation_id` headers reads 6.4k of it back —
        // and `OpenAI-Beta` on its own changes nothing. Both names carry the
        // one per-session key, the reference client's shape.
        assert_eq!(
            session_headers(Some("alter-zero-42")),
            vec![
                ("session_id".to_string(), "alter-zero-42".to_string()),
                ("conversation_id".to_string(), "alter-zero-42".to_string()),
            ]
        );
    }

    #[test]
    fn no_cache_key_means_no_session_headers() {
        // An empty session id is a *different* (invalid) routing hint than
        // none, the account-id rule again.
        assert!(session_headers(None).is_empty());
        assert!(session_headers(Some("")).is_empty());
    }

    #[test]
    fn a_token_is_cached_until_shortly_before_its_own_expiry() {
        // An hour-long token, five minutes held back.
        assert_eq!(
            cache_lifetime(Some(3600), 0),
            Duration::from_secs(3600 - 300)
        );
    }

    #[test]
    fn a_clock_running_ahead_cannot_make_every_request_re_mint() {
        // The bug the Copilot module avoids by keying off a *duration*: an
        // absolute `exp` already in the past would otherwise re-mint on every
        // single request. The floor is the guard.
        assert_eq!(cache_lifetime(Some(10), 1_000_000), MIN_CACHE);
        assert_eq!(cache_lifetime(Some(3600), 3599), MIN_CACHE);
        // And a token that names no expiry is trusted for the floor, not
        // forever.
        assert_eq!(cache_lifetime(None, 0), MIN_CACHE);
    }

    // --- the rotation chain ---

    #[test]
    fn a_rotated_refresh_token_is_what_the_next_refresh_presents() {
        // The session's config still holds the token it started with, and
        // nothing reloads it mid-run. Without this chain the next refresh
        // would present the retired token and earn the terminal
        // `refresh_token_reused` about an hour into a working session.
        let stored = "rot-test-original";
        assert_eq!(live_refresh_token(stored), stored, "unrotated: itself");
        note_rotation(stored, stored, "rot-test-second");
        assert_eq!(live_refresh_token(stored), "rot-test-second");
        // A second rotation re-points both the original and what was actually
        // presented, so either config keeps working.
        note_rotation(stored, "rot-test-second", "rot-test-third");
        assert_eq!(live_refresh_token(stored), "rot-test-third");
        assert_eq!(live_refresh_token("rot-test-second"), "rot-test-third");
        // Signing in again drops the chain: a fresh token is unrelated to
        // whatever the old one had become.
        forget(stored);
        assert_eq!(live_refresh_token(stored), stored);
    }

    // --- failure advice ---

    #[test]
    fn a_dead_refresh_token_says_to_sign_in_again() {
        for code in [
            "refresh_token_expired",
            "refresh_token_reused",
            "refresh_token_invalidated",
            "invalid_grant",
        ] {
            let body = format!(r#"{{"error":"{code}"}}"#);
            let advice = auth_advice(400, &body).unwrap_or_else(|| panic!("{code} needs advice"));
            assert!(advice.contains("/login"), "{code}: {advice}");
        }
    }

    #[test]
    fn a_nested_error_object_is_read_too() {
        let body = r#"{"error":{"code":"refresh_token_reused","message":"…"}}"#;
        assert!(auth_advice(400, body).unwrap().contains("/login"));
    }

    #[test]
    fn a_403_blames_the_plan_and_an_unknown_400_is_left_alone() {
        assert!(auth_advice(403, "{}").unwrap().contains("plan"));
        assert!(auth_advice(400, r#"{"error":"server_hiccup"}"#).is_none());
        assert!(auth_advice(500, "").is_none());
    }

    // --- the device-code flow (docs/chatgpt.md) ---

    #[test]
    fn the_device_endpoints_hang_off_the_issuer() {
        // Codex's own four: the code request and the poll under
        // `/api/accounts/deviceauth`, the page the user types the code at,
        // and the redirect the server binds the PKCE pair to.
        let ep = DeviceEndpoints::for_issuer("https://auth.openai.com");
        assert_eq!(
            ep.usercode,
            "https://auth.openai.com/api/accounts/deviceauth/usercode"
        );
        assert_eq!(
            ep.token,
            "https://auth.openai.com/api/accounts/deviceauth/token"
        );
        assert_eq!(ep.verification, "https://auth.openai.com/codex/device");
        assert_eq!(
            ep.redirect_uri,
            "https://auth.openai.com/deviceauth/callback"
        );
        // An override's trailing slash (a stub's `http://127.0.0.1:8080/`)
        // must not double up.
        let ep = DeviceEndpoints::for_issuer("http://127.0.0.1:8080/");
        assert_eq!(ep.verification, "http://127.0.0.1:8080/codex/device");
    }

    #[test]
    fn the_issuer_defaults_to_openai_and_the_override_has_a_name() {
        assert_eq!(DEFAULT_ISSUER, "https://auth.openai.com");
        assert_eq!(ISSUER_ENV, "ALTER_ZERO_OPENAI_ISSUER");
        // And the code's life is Codex's own fifteen minutes — what the
        // page counts down from.
        assert_eq!(DEVICE_CODE_TIMEOUT, Duration::from_secs(15 * 60));
    }

    #[test]
    fn the_device_code_request_names_only_the_client_id() {
        assert_eq!(
            device_code_request_body(),
            serde_json::json!({"client_id": CLIENT_ID})
        );
    }

    #[test]
    fn a_device_code_response_parses_its_interval_as_a_string_or_a_number() {
        // The reference sends the interval as a **string** ("5"); a number
        // is accepted too, since nothing says it will stay one.
        let device = DeviceCode::parse(
            r#"{"device_auth_id":"device-auth-123","user_code":"CODE-12345","interval":"5"}"#,
        )
        .unwrap();
        assert_eq!(device.user_code(), "CODE-12345");
        assert_eq!(device.poll_interval(), Duration::from_secs(5));
        let device =
            DeviceCode::parse(r#"{"device_auth_id":"d","user_code":"C","interval":7}"#).unwrap();
        assert_eq!(device.poll_interval(), Duration::from_secs(7));
        // Absent: a sensible default rather than the reference's zero, which
        // is a tight loop against the server — and zero itself is floored.
        let device = DeviceCode::parse(r#"{"device_auth_id":"d","user_code":"C"}"#).unwrap();
        assert_eq!(
            device.poll_interval(),
            Duration::from_secs(DEVICE_DEFAULT_INTERVAL)
        );
        let device =
            DeviceCode::parse(r#"{"device_auth_id":"d","user_code":"C","interval":"0"}"#).unwrap();
        assert_eq!(device.poll_interval(), Duration::from_secs(1));
        // No id, or no code, is nothing to poll with.
        assert!(DeviceCode::parse(r#"{"user_code":"C"}"#).is_err());
        assert!(DeviceCode::parse(r#"{"device_auth_id":"d","user_code":""}"#).is_err());
        assert!(DeviceCode::parse("not json").is_err());
    }

    #[test]
    fn the_poll_presents_the_pair_the_code_request_issued() {
        let device = DeviceCode::parse(
            r#"{"device_auth_id":"device-auth-123","user_code":"CODE-12345","interval":"5"}"#,
        )
        .unwrap();
        assert_eq!(
            device_poll_body(&device),
            serde_json::json!({"device_auth_id": "device-auth-123", "user_code": "CODE-12345"})
        );
    }

    #[test]
    fn a_poll_is_pending_until_the_grant_arrives() {
        // Codex's reading of the poll: 403 and 404 both mean "not yet", a
        // 200 carries the grant, and anything else is a real failure.
        assert_eq!(device_poll_verdict(403, ""), DevicePoll::Pending);
        assert_eq!(device_poll_verdict(404, "{}"), DevicePoll::Pending);
        let grant = r#"{"authorization_code":"poll-code-321","code_challenge":"cc","code_verifier":"code-verifier-321"}"#;
        match device_poll_verdict(200, grant) {
            DevicePoll::Approved(grant) => {
                assert_eq!(grant.authorization_code, "poll-code-321");
                assert_eq!(grant.code_verifier, "code-verifier-321");
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            device_poll_verdict(200, "not json"),
            DevicePoll::Failed(_)
        ));
        assert!(matches!(
            device_poll_verdict(500, ""),
            DevicePoll::Failed(_)
        ));
        assert!(matches!(
            device_poll_verdict(400, r#"{"error":"expired"}"#),
            DevicePoll::Failed(_)
        ));
    }

    #[test]
    fn the_grant_is_redeemed_with_its_own_verifier_at_the_device_callback() {
        // The *server* minted the PKCE pair here; the exchange repeats its
        // verifier and the device callback the pair was bound to — never a
        // loopback port, since none was opened.
        let ep = DeviceEndpoints::for_issuer("https://auth.openai.com");
        let body = code_exchange_body("poll-code-321", "code-verifier-321", &ep.redirect_uri);
        assert!(body.starts_with("grant_type=authorization_code"));
        assert!(body.contains("&code=poll-code-321"));
        assert!(body.contains("&code_verifier=code-verifier-321"));
        assert!(
            body.contains("&redirect_uri=https%3A%2F%2Fauth.openai.com%2Fdeviceauth%2Fcallback")
        );
    }

    #[test]
    fn a_missing_device_endpoint_points_at_the_browser_row() {
        // Codex's own reading of a 404 on the code request: device code
        // login is not enabled for this server. The page can name the other
        // row, since the choice is one Esc away.
        let advice = device_code_advice(404, "").unwrap();
        assert!(advice.contains("Browser login"), "{advice}");
        assert!(device_code_advice(500, "").is_none());
    }
}
