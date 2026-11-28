//! xAI as a **subscription**: the Grok sign-in, the refresh token it fills
//! the `.env` store with, and the short-lived access token every request is
//! minted with. See `docs/xai.md`.
//!
//! The sixth sign-in, after GitHub Copilot's (`docs/copilot.md`), OpenAI's
//! (`docs/chatgpt.md`), Anthropic's (`docs/claude.md`), Cline's
//! (`docs/cline.md`) and Google Antigravity's (`docs/antigravity.md`), and it
//! keeps their two-layer split exactly: a long-lived token in the `.env` key
//! store, exchanged (and cached in memory, never written) for the short-lived
//! credential the API takes.
//!
//! # The flow
//!
//! An RFC 8628 **device code** on xAI's OIDC issuer, `auth.x.ai` — the
//! headless sign-in xAI documents for its own CLI (`grok login
//! --device-auth`), and the only xAI sign-in hermes-agent and pi, the two
//! open clients this port follows, run:
//!
//! 1. `POST {issuer}/oauth2/device/code` with the client id and its scopes —
//!    `api:access` is what lets the token call `api.x.ai`, `offline_access`
//!    what earns the refresh token.
//! 2. `POST {issuer}/oauth2/token` with the device-code grant, every
//!    `interval`, until the user approves the code, it expires, or they deny
//!    it.
//! 3. The refresh grant at the same endpoint mints each access token after
//!    that, and may **rotate** the refresh token. A retired one is refused,
//!    so the new one is written back to the store before the request goes.
//!
//! # The wire
//!
//! There is no second exchange: the access token *is* the bearer, on
//! `https://api.x.ai/v1/responses` — the Responses API xAI recommends for its
//! reasoning models, which `responses.rs` already speaks.
//!
//! The pure halves (the bodies, the device code's parse, the poll's verdict,
//! the freshness rule, the claims, the advice) are unit-tested; the HTTP calls
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

/// The OAuth client the sign-in presents: the public client xAI's own Grok
/// CLI signs in with (its scopes name `grok-cli:access`), and the one
/// hermes-agent and pi both send. xAI publishes no third-party registration,
/// the posture `docs/chatgpt.md` documents for Codex's client id.
pub const CLIENT_ID: &str = "b1a00492-073a-47ea-816f-4c329264a828";

/// What the sign-in asks for: the client's own set, whole, as both
/// references ask for it. `api:access` lets the token call the API and
/// `offline_access` earns the refresh token; without either the sign-in
/// succeeds and then cannot do its one job.
const SCOPES: &str = "openid profile email offline_access grok-cli:access api:access";

/// xAI's OIDC issuer — the host the device code and both grants post to.
pub const DEFAULT_ISSUER: &str = "https://auth.x.ai";

const DEVICE_CODE_PATH: &str = "/oauth2/device/code";
const TOKEN_PATH: &str = "/oauth2/token";

/// RFC 8628's grant type, presented by every poll.
const DEVICE_GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";

/// A code's life when the response names none. RFC 8628 makes `expires_in`
/// required; a server that leaves it out still gets a countdown to show.
const DEFAULT_EXPIRES: u64 = 900;

/// The poll cadence when the response names none — or a zero, which RFC 8628
/// allows and which would be a tight loop against the server.
const DEFAULT_INTERVAL: u64 = 5;

/// The slowest the poll goes, whatever a server asks: past this the page
/// would count down a code nobody is checking.
pub const MAX_INTERVAL: Duration = Duration::from_secs(60);

/// RFC 8628 §3.5: a `slow_down` grows the interval by five seconds, for that
/// poll and every one after it.
pub const SLOW_DOWN_STEP: Duration = Duration::from_secs(5);

/// Re-mint an access token this long before it dies, so a request never
/// leaves with one that expires in flight.
const REFRESH_SKEW: Duration = Duration::from_secs(300);

/// The floor on a cached access token's life when nothing trustworthy says
/// otherwise (`docs/chatgpt.md` *The cached lifetime, and the clock*): an
/// uncached bearer would refresh — and rotate — on every request.
const MIN_CACHE: Duration = Duration::from_secs(60);

/// What a user is told when the credential itself is gone or refused: a
/// re-sign-in is the one thing that fixes it.
const SIGN_IN_AGAIN: &str = "your xAI sign-in is no longer valid — run /login and sign in again.";

/// What a user is told when the account, not the credential, is refused:
/// signing in again would mint the same refusal.
const NO_API_ACCESS: &str = "xAI refused this account — it needs a Grok subscription that includes \
                             API access (SuperGrok), or its usage is spent for now. Check the plan \
                             at grok.com, or switch provider with /model.";

/// What a user is told when the account is being rate-limited.
const RATE_LIMITED: &str = "xAI is rate-limiting this account — wait for the limit to reset, or switch provider with /model.";

/// The poll's refusal of an approval that came with no refresh token.
const NO_REFRESH_TOKEN: &str =
    "xAI approved the sign-in but sent no refresh token — press Esc and sign in again.";

// ---------------------------------------------------------------------------
// The flow's URLs and bodies (pure)
// ---------------------------------------------------------------------------

/// Where the device code and the two grants go, off one issuer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoints {
    /// `POST` — ask for a code.
    pub device_code: String,
    /// `POST` — poll for the grant, and refresh it after.
    pub token: String,
}

impl Endpoints {
    /// The endpoints for `issuer` — a trailing slash tolerated, so a stub's
    /// `http://127.0.0.1:8080/` builds the same paths as xAI's own.
    #[must_use]
    pub fn for_issuer(issuer: &str) -> Self {
        let issuer = issuer.trim().trim_end_matches('/');
        Self {
            device_code: format!("{issuer}{DEVICE_CODE_PATH}"),
            token: format!("{issuer}{TOKEN_PATH}"),
        }
    }
}

/// The device-authorization request's body: the client and its scopes,
/// nothing else.
#[must_use]
pub fn device_code_body() -> String {
    form_encode(&[
        ("client_id".to_string(), CLIENT_ID.to_string()),
        ("scope".to_string(), SCOPES.to_string()),
    ])
}

/// One poll's body: the code the request issued, under RFC 8628's grant.
#[must_use]
pub fn device_poll_body(device_code: &str) -> String {
    form_encode(&[
        ("grant_type".to_string(), DEVICE_GRANT.to_string()),
        ("device_code".to_string(), device_code.to_string()),
        ("client_id".to_string(), CLIENT_ID.to_string()),
    ])
}

/// The refresh grant's body — form-encoded like the poll, at the same
/// endpoint.
#[must_use]
pub fn refresh_body(refresh_token: &str) -> String {
    form_encode(&[
        ("grant_type".to_string(), "refresh_token".to_string()),
        ("refresh_token".to_string(), refresh_token.to_string()),
        ("client_id".to_string(), CLIENT_ID.to_string()),
    ])
}

/// A positive count of seconds from a JSON number or a string holding one —
/// `None` for anything else, a zero included, so the caller's default stands.
fn seconds(value: Option<&serde_json::Value>) -> Option<u64> {
    let value = value?;
    let secs = value
        .as_u64()
        .or_else(|| value.as_str()?.trim().parse().ok())?;
    (secs > 0).then_some(secs)
}

/// Is `uri` a web page this machine can offer as a link to open: `https`,
/// with a host? The device page shows the address the server named, so a
/// plain `http` one — or a `file:` one — is refused rather than shown.
fn is_https(uri: &str) -> bool {
    url::Url::parse(uri.trim())
        .is_ok_and(|url| url.scheme() == "https" && url.host_str().is_some_and(|h| !h.is_empty()))
}

// ---------------------------------------------------------------------------
// The device code (pure)
// ---------------------------------------------------------------------------

/// What the device-authorization request answered: the code the poll
/// presents, and the short code the user enters in a browser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceCode {
    /// The server's handle on this attempt — presented by every poll, never
    /// shown.
    device_code: String,
    /// The short code the user enters at the verification page.
    user_code: String,
    /// Where they enter it. The one-click `verification_uri_complete` beside
    /// it is not used: the page asks the user to type the code, which that
    /// address would make a false instruction (`docs/cline.md` keeps the
    /// same rule).
    verification_uri: String,
    /// How long the code stays valid, in seconds.
    expires_in: u64,
    /// The seconds between polls the server asked for.
    interval: u64,
}

impl DeviceCode {
    /// Parse the device-authorization response.
    ///
    /// # Errors
    /// A body that isn't the response's shape, names no code, or names a page
    /// that isn't an `https` address is a decode error — there is nothing to
    /// show or poll with.
    pub fn parse(body: &str) -> Result<Self> {
        #[derive(Deserialize)]
        struct Raw {
            #[serde(default)]
            device_code: Option<String>,
            #[serde(default)]
            user_code: Option<String>,
            #[serde(default)]
            verification_uri: Option<String>,
            #[serde(default)]
            expires_in: Option<serde_json::Value>,
            #[serde(default)]
            interval: Option<serde_json::Value>,
        }
        let raw: Raw = serde_json::from_str(body)
            .map_err(|_| LlmError::Decode("xAI sent no device code".to_string()))?;
        let nonblank = |s: Option<String>| s.filter(|s| !s.trim().is_empty());
        let (Some(device_code), Some(user_code)) =
            (nonblank(raw.device_code), nonblank(raw.user_code))
        else {
            return Err(LlmError::Decode("xAI sent no device code".to_string()));
        };
        let verification_uri = raw.verification_uri.unwrap_or_default();
        if !is_https(&verification_uri) {
            return Err(LlmError::Decode(format!(
                "xAI named a sign-in page that is not an https address: {verification_uri:?}"
            )));
        }
        Ok(Self {
            device_code,
            user_code,
            verification_uri: verification_uri.trim().to_string(),
            expires_in: seconds(raw.expires_in.as_ref()).unwrap_or(DEFAULT_EXPIRES),
            interval: seconds(raw.interval.as_ref()).unwrap_or(DEFAULT_INTERVAL),
        })
    }

    /// The page the user enters the code at.
    #[must_use]
    pub fn verification_uri(&self) -> &str {
        &self.verification_uri
    }

    /// The code to show and copy.
    #[must_use]
    pub fn user_code(&self) -> &str {
        &self.user_code
    }

    /// How long the code stays valid — what the page counts down and what
    /// the poll's deadline is.
    #[must_use]
    pub fn lifetime(&self) -> Duration {
        Duration::from_secs(self.expires_in.max(1))
    }

    /// How long to wait between polls: what the server asked, held between a
    /// second and [`MAX_INTERVAL`].
    #[must_use]
    pub fn poll_interval(&self) -> Duration {
        Duration::from_secs(self.interval).clamp(Duration::from_secs(1), MAX_INTERVAL)
    }
}

// ---------------------------------------------------------------------------
// The poll's verdict (pure)
// ---------------------------------------------------------------------------

/// What one poll of the token endpoint established.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Poll {
    /// The user approved: the token set, refresh token included.
    Approved(TokenSet),
    /// Not approved yet — ask again after the interval.
    Pending,
    /// Polling too fast: the interval the server named, or `None` for RFC
    /// 8628's five seconds more.
    SlowDown(Option<Duration>),
    /// A failure on the way there, not an answer — a proxy's error page, a
    /// rate limit, a timeout. The code is still good: ask again.
    Retry,
    /// The flow is over — the sentence to show.
    Denied(String),
}

/// Read one poll's answer. A `2xx` is the grant, and must carry both tokens;
/// anything else is an OAuth error in the body — RFC 8628's three, xAI's own
/// `authorization_denied` beside `access_denied` — or, when the body names
/// none on a status that says the trip failed, one more wait.
#[must_use]
pub fn poll_verdict(status: u16, body: &str) -> Poll {
    #[derive(Debug, Default, Deserialize)]
    #[serde(default)]
    struct RawError {
        error: Option<String>,
        error_description: Option<String>,
        interval: Option<serde_json::Value>,
    }
    if (200..300).contains(&status) {
        return match TokenSet::parse(body) {
            Ok(tokens) if tokens.refresh_token.is_some() => Poll::Approved(tokens),
            Ok(_) => Poll::Denied(NO_REFRESH_TOKEN.to_string()),
            Err(_) => Poll::Denied(
                "xAI sent an unexpected answer to the sign-in — press Esc and try again."
                    .to_string(),
            ),
        };
    }
    let transient = status == 408 || status == 429 || status >= 500;
    let raw: RawError = serde_json::from_str(body).unwrap_or_default();
    match raw.error.as_deref() {
        Some("authorization_pending") => Poll::Pending,
        Some("slow_down") => {
            Poll::SlowDown(seconds(raw.interval.as_ref()).map(Duration::from_secs))
        }
        Some("access_denied" | "authorization_denied") => Poll::Denied(
            "The sign-in was denied on xAI's page — press Esc and try again.".to_string(),
        ),
        Some("expired_token") => {
            Poll::Denied("The code expired — press Esc and sign in again.".to_string())
        }
        _ if transient => Poll::Retry,
        Some(code) => {
            let said = raw
                .error_description
                .filter(|d| !d.trim().is_empty())
                .unwrap_or_else(|| code.to_string());
            Poll::Denied(format!(
                "xAI refused the sign-in: {said} — press Esc and try again."
            ))
        }
        None => Poll::Denied(format!(
            "xAI answered {status} to the sign-in — press Esc and try again."
        )),
    }
}

// ---------------------------------------------------------------------------
// The token set (pure)
// ---------------------------------------------------------------------------

/// What a token response carried. `refresh_token` is optional because a
/// refresh re-sends it only when it rotated — a missing one means "keep the
/// one you have", never "you have none".
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TokenSet {
    pub access_token: Option<String>,
    pub refresh_token: Option<String>,
    /// Seconds of life, from which the cache lifetime is computed.
    pub expires_in: Option<u64>,
}

impl TokenSet {
    /// Parse a token response.
    ///
    /// # Errors
    /// A body that isn't an object, or that carries no access token, is a
    /// decode error — there is nothing to authenticate with either way.
    pub fn parse(body: &str) -> Result<Self> {
        #[derive(Deserialize)]
        struct Raw {
            #[serde(default)]
            access_token: Option<String>,
            #[serde(default)]
            refresh_token: Option<String>,
            #[serde(default)]
            expires_in: Option<serde_json::Value>,
        }
        let raw: Raw = serde_json::from_str(body).map_err(|e| LlmError::Decode(e.to_string()))?;
        let access = raw
            .access_token
            .filter(|t| !t.is_empty())
            .ok_or_else(|| LlmError::Decode("xAI returned no access token".to_string()))?;
        Ok(Self {
            access_token: Some(access),
            refresh_token: raw.refresh_token.filter(|t| !t.is_empty()),
            expires_in: seconds(raw.expires_in.as_ref()),
        })
    }
}

/// How long a minted access token may be cached: from the grant's own
/// `expires_in` when the response names one — a **duration**, immune to this
/// machine's clock — else from the bearer's absolute `exp` against the wall
/// clock `now` (both Unix seconds). The rule `chatgpt::cache_lifetime` keeps:
/// a five-minute skew off the end, a short life cached for at most half of
/// itself so a known expiry is never extended, and a sixty-second floor when
/// nothing trustworthy says.
#[must_use]
pub fn cache_lifetime(expires_in: Option<u64>, expires_at: Option<u64>, now: u64) -> Duration {
    let life = expires_in.or_else(|| expires_at.filter(|&exp| exp > now).map(|exp| exp - now));
    let Some(secs) = life else {
        return MIN_CACHE;
    };
    let life = Duration::from_secs(secs);
    life.saturating_sub(REFRESH_SKEW)
        .max(MIN_CACHE.min(life / 2))
}

// ---------------------------------------------------------------------------
// The access token's claims (pure)
// ---------------------------------------------------------------------------

/// What an xAI access token's payload says, read for the sign-in's
/// confirmation and the cache's fallback expiry. The signature is **never
/// verified**: these claims label a request the server checks anyway, and
/// grant nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Claims {
    /// The account — its email, else its handle.
    pub email: Option<String>,
    /// The token's own `exp`, Unix seconds.
    pub expires_at: Option<u64>,
}

/// Decode a JWT's payload. `None` for anything that isn't three
/// dot-separated segments with a base64url JSON middle — an opaque token says
/// nothing, and that is not an error.
#[must_use]
pub fn parse_claims(jwt: &str) -> Option<Claims> {
    #[derive(Deserialize)]
    struct Raw {
        #[serde(default)]
        email: Option<String>,
        #[serde(default)]
        preferred_username: Option<String>,
        #[serde(default)]
        exp: Option<serde_json::Value>,
    }
    let mut parts = jwt.trim().split('.');
    let (Some(header), Some(payload), Some(signature), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return None;
    };
    if header.is_empty() || payload.is_empty() || signature.is_empty() {
        return None;
    }
    let raw: Raw = serde_json::from_slice(&b64url_decode(payload)?).ok()?;
    let nonblank = |s: Option<String>| s.filter(|s| !s.trim().is_empty());
    Some(Claims {
        email: nonblank(raw.email).or_else(|| nonblank(raw.preferred_username)),
        expires_at: raw.exp.as_ref().and_then(serde_json::Value::as_u64),
    })
}

/// Decode base64url (RFC 4648 §5), padded or not — `chatgpt`'s decoder, local
/// so this module's pure half stands alone (`cline` keeps the same copy).
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

// ---------------------------------------------------------------------------
// The advice (pure)
// ---------------------------------------------------------------------------

/// An OAuth `error` code, when the body is an object naming one as a string.
fn oauth_error_code(body: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()?
        .get("error")?
        .as_str()
        .map(str::to_string)
}

/// Explain an API refusal in terms the user can act on — the chat request's
/// and the model listing's. xAI answers several different problems with a
/// 403 whose `code` reads the same, and they need opposite advice: a bearer
/// it no longer honours (`[WKE=unauthenticated:…]`) is a sign-in to redo,
/// while every other 403 is the account — no subscription with API access,
/// or its usage spent — which signing in again cannot fix.
#[must_use]
pub fn auth_advice(status: u16, body: &str) -> Option<String> {
    let lower = body.to_ascii_lowercase();
    if lower.contains("[wke=unauthenticated:")
        || lower.contains("access token could not be validated")
    {
        return Some(SIGN_IN_AGAIN.to_string());
    }
    match (status, oauth_error_code(body).as_deref()) {
        (401, _) | (_, Some("invalid_grant" | "invalid_token")) => Some(SIGN_IN_AGAIN.to_string()),
        (403, _) => Some(NO_API_ACCESS.to_string()),
        (429, _) => Some(RATE_LIMITED.to_string()),
        _ => None,
    }
}

/// Explain a refused **refresh**. A 400 or a 401 from the token endpoint is
/// the refresh token itself — revoked, reused or expired — and a re-sign-in
/// is the only way back; a 403 there is the account, as on the API.
#[must_use]
pub fn refresh_advice(status: u16, body: &str) -> Option<String> {
    match status {
        400 | 401 => Some(SIGN_IN_AGAIN.to_string()),
        403 => Some(NO_API_ACCESS.to_string()),
        _ => auth_advice(status, body),
    }
}

/// The poll's interval after a `slow_down`: the interval the server named,
/// or RFC 8628's five seconds more — never faster than before, and never
/// slower than [`MAX_INTERVAL`].
#[must_use]
pub fn slowed(interval: Duration, asked: Option<Duration>) -> Duration {
    asked
        .unwrap_or(interval + SLOW_DOWN_STEP)
        .max(interval)
        .min(MAX_INTERVAL)
}

// ---------------------------------------------------------------------------
// The issuer and the store (boundary state)
// ---------------------------------------------------------------------------

/// Points the sign-in and the refresh at another issuer — the smoke suite's
/// local stand-in, or a fork's own. Read at the boundary and handed in
/// through [`set_issuer`], so the pure builders never read the environment.
pub const ISSUER_ENV: &str = "ALTER_ZERO_XAI_ISSUER";

/// The environment variable the refresh token lives under — the name
/// `providers.toml` gives the provider's `api_key_env`, pinned here because
/// the rotation write-back cannot ask the provider file.
pub const REFRESH_ENV_VAR: &str = "XAI_GROK_REFRESH_TOKEN";

/// How long one call to the issuer may take — the other sign-ins' value, so
/// they share one cached HTTP client.
const OP_TIMEOUT: Duration = Duration::from_secs(30);

/// The most a response from the issuer may buffer. It also bounds what a
/// failure can put on the sign-in page: an intercepting proxy answers with an
/// HTML page, and that page would otherwise become the reason.
const BODY_MAX_BYTES: u64 = 64 * 1024;

/// How long one nap between cancel checks lasts while the poll waits, so an
/// Esc on the device page is honoured about as fast as one mid-turn.
const POLL_NAP: Duration = Duration::from_millis(200);

/// The sentence for a code whose life ran out before it was approved.
const CODE_EXPIRED: &str = "The code expired — press Esc and sign in again.";

fn issuer_slot() -> &'static Mutex<Option<String>> {
    static SLOT: OnceLock<Mutex<Option<String>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

/// Point the sign-in and the refresh at `url` instead of xAI's issuer.
/// Boundary-only; called once at startup.
pub fn set_issuer(url: impl Into<String>) {
    let url = url.into();
    let trimmed = url.trim().trim_end_matches('/').to_string();
    if let Ok(mut slot) = issuer_slot().lock() {
        *slot = (!trimmed.is_empty()).then_some(trimmed);
    }
}

/// The endpoints off the issuer in force.
fn endpoints() -> Endpoints {
    let issuer = issuer_slot()
        .lock()
        .ok()
        .and_then(|slot| slot.clone())
        .unwrap_or_else(|| DEFAULT_ISSUER.to_string());
    Endpoints::for_issuer(&issuer)
}

/// Where a rotated refresh token is written back. Set once at the boundary
/// (`tui::models`), because the rotation happens deep on a backend thread.
fn store_path() -> &'static Mutex<Option<PathBuf>> {
    static PATH: OnceLock<Mutex<Option<PathBuf>>> = OnceLock::new();
    PATH.get_or_init(|| Mutex::new(None))
}

/// Tell this module where the `.env` key store lives, so a rotated refresh
/// token can be persisted. Boundary-only; called once at startup. Skipping it
/// is not a crash but a forced re-login at the next launch.
pub fn set_store_path(path: impl Into<PathBuf>) {
    if let Ok(mut slot) = store_path().lock() {
        *slot = Some(path.into());
    }
}

// ---------------------------------------------------------------------------
// The access-token cache (boundary state) — `claude`'s, account for account
// ---------------------------------------------------------------------------

/// One minted access token.
#[derive(Debug, Clone)]
pub struct Access {
    /// The bearer the request carries — the access token itself.
    pub bearer: String,
    /// The account the token belongs to, when its claims name one.
    pub email: Option<String>,
}

/// What the sign-in confirmation names beside the provider: the account.
/// That is the question a device code leaves open — the browser it was
/// approved in may have been signed in to another one.
#[must_use]
pub fn account_label(access: &Access) -> Option<String> {
    access.email.clone()
}

/// A cached [`Access`] and when it stops being usable.
#[derive(Debug, Clone)]
struct Cached {
    access: Access,
    until: Instant,
}

/// Access tokens keyed by the **refresh token they were minted from**. A
/// turn's rounds, the `/model` fetch and every subagent thread want the same
/// bearer, and re-minting per request would spend a round trip — and a
/// rotation — each time.
fn cache() -> &'static Mutex<HashMap<String, Cached>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Cached>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Serialize cache misses through refresh, rotation, and persistence. A
/// refresh token is single-use, so locking only the map lookup is unsafe.
/// Warm hits remain independent of a slow refresh.
fn refresh_gate() -> &'static Mutex<()> {
    static GATE: Mutex<()> = Mutex::new(());
    &GATE
}

/// The refresh tokens [`forget`] dropped while a refresh was out. A refresh
/// clears it as it starts (anything there is older than it, and already
/// honoured) and drains it under the cache lock before keeping what it
/// minted: a name from its own chain there means a sign-in replaced the
/// credential while the request was out, so the bearer is not cached and the
/// next request refreshes afresh. This is what lets `forget` skip the gate.
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

/// What each retired refresh token became. The live `ModelConfig` carries
/// the token the session started with and nothing reloads it mid-run, so once
/// xAI retires it the next refresh would present a dead one — this is the
/// in-memory half of the fix `persist_refresh` makes on disk.
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

/// Drop any cached access token minted from `refresh_token`. Signing in again
/// is how a user recovers from a token xAI has stopped honouring, so the
/// cached one must not outlive the sign-in that replaced it. Never behind the
/// gate — a refresh in flight holds it for as long as its request takes.
pub fn forget(refresh_token: &str) {
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

// ---------------------------------------------------------------------------
// The sign-in and the token mint (boundary — real HTTP)
// ---------------------------------------------------------------------------

/// Read a response body, bounded.
fn read_body(resp: reqwest::blocking::Response) -> String {
    let mut body = String::new();
    let _ = resp.take(BODY_MAX_BYTES).read_to_string(&mut body);
    body
}

/// The issuer's answer to anything but a poll, as the error the page or the
/// turn shows: the advice when the refusal is one a user can act on, the
/// server's own words otherwise.
fn token_failure(status: u16, body: &str) -> LlmError {
    let explained = refresh_advice(status, body).unwrap_or_else(|| {
        let trimmed = body.trim();
        if trimmed.is_empty() {
            format!("xAI answered {status}")
        } else {
            format!(
                "xAI answered {status}: {}",
                super::truncate_chars(trimmed, 300)
            )
        }
    });
    LlmError::Api {
        status,
        body: explained,
    }
}

/// Ask xAI for a device code. Boundary code.
///
/// # Errors
/// A transport failure, a refusal, or a body that isn't a device code.
pub fn request_device_code() -> Result<DeviceCode> {
    let client = super::http_client(OP_TIMEOUT)?;
    let resp = client
        .post(endpoints().device_code)
        .header("accept", "application/json")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(device_code_body())
        .send()
        .map_err(|e| LlmError::transport(&e))?;
    let status = resp.status().as_u16();
    let body = read_body(resp);
    if !(200..300).contains(&status) {
        return Err(token_failure(status, &body));
    }
    DeviceCode::parse(&body)
}

/// What a finished sign-in hands back: the refresh token the store keeps, and
/// the account it signed in to, for the confirmation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedIn {
    pub refresh_token: String,
    pub account: Option<String>,
}

/// Poll until the user approves the code, the flow fails, or `cancel` trips.
/// The wait between polls is the interval xAI asked for, slept in short naps
/// so an Esc is honoured promptly. Boundary code.
///
/// # Errors
/// The `Err` is the **sentence to show the user** — a denial, an expiry, a
/// dead connection are all read off the device page. A cancel yields an empty
/// reason, which the worker never delivers.
pub fn await_approval(
    device: &DeviceCode,
    cancel: &CancelToken,
) -> std::result::Result<SignedIn, String> {
    let client = super::http_client(OP_TIMEOUT).map_err(|e| e.to_string())?;
    let token_url = endpoints().token;
    let deadline = Instant::now() + device.lifetime();
    let mut interval = device.poll_interval();
    let body = device_poll_body(&device.device_code);
    loop {
        // Sleep *first*: the user has not read the code yet, and a poll that
        // beats the interval earns a `slow_down`.
        if !nap(interval, cancel) {
            return Err(String::new());
        }
        if Instant::now() >= deadline {
            return Err(CODE_EXPIRED.to_string());
        }
        let resp = client
            .post(&token_url)
            .header("accept", "application/json")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(body.clone())
            .send()
            .map_err(|e| format!("Could not reach xAI: {e} — press Esc and try again."))?;
        let status = resp.status().as_u16();
        let text = read_body(resp);
        match poll_verdict(status, &text) {
            Poll::Approved(tokens) => {
                // The verdict holds an approval to both tokens; this keeps
                // the refusal reachable rather than an unwrap.
                let Some(refresh_token) = tokens.refresh_token else {
                    return Err(NO_REFRESH_TOKEN.to_string());
                };
                let account = tokens
                    .access_token
                    .as_deref()
                    .and_then(parse_claims)
                    .and_then(|claims| claims.email);
                return Ok(SignedIn {
                    refresh_token,
                    account,
                });
            }
            Poll::Pending | Poll::Retry => {}
            Poll::SlowDown(asked) => interval = slowed(interval, asked),
            Poll::Denied(reason) => return Err(reason),
        }
    }
}

/// Sleep `total` in [`POLL_NAP`] slices, returning `false` the moment
/// `cancel` trips.
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

/// The [`Access`] a token response describes.
fn access_from(tokens: &TokenSet) -> Result<Access> {
    let bearer = tokens
        .access_token
        .clone()
        .filter(|t| !t.is_empty())
        .ok_or_else(|| LlmError::Decode("xAI returned no access token".to_string()))?;
    let email = parse_claims(&bearer).and_then(|claims| claims.email);
    Ok(Access { bearer, email })
}

/// The wall clock in Unix seconds, for the bearer's absolute `exp`.
fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// The access token to authenticate with, minted from `refresh_token` and
/// cached until shortly before its own expiry. Boundary code — one HTTP round
/// trip on a cache miss, none on a hit.
///
/// A rotated refresh token is written back to the key store before this
/// returns: xAI retires the one just used, and leaving the retired value on
/// disk turns the next launch into a forced re-login.
///
/// # Errors
/// A refresh xAI refused, or a transport failure.
pub fn authorize(refresh_token: &str) -> Result<Access> {
    authorize_with(refresh_token, exchange_refresh)
}

/// Keep the refresh transport replaceable so concurrent cache misses can be
/// exercised deterministically without a live account.
fn authorize_with(
    refresh_token: &str,
    refresh: impl FnOnce(&str) -> Result<TokenSet>,
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
    // What earlier forgets left on record is already honoured — each cleared
    // the cache itself — and none of them raced this refresh, which has not
    // started. Kept, they would make the settle below throw away the bearer
    // of the first refresh after every sign-in, which forgets the very token
    // it stores; the next request would then refresh, and rotate, again.
    if let Ok(mut list) = forgotten().lock() {
        list.clear();
    }
    let presented = live_refresh_token(refresh_token);
    let refresh_started = Instant::now();
    let tokens = refresh(&presented)?;
    let access = access_from(&tokens)?;
    let expires_at = parse_claims(&access.bearer).and_then(|claims| claims.expires_at);
    let lifetime = cache_lifetime(tokens.expires_in, expires_at, unix_now());
    // The alias map keeps old configs working; only the current alias needs
    // a bearer entry. Keeping one under every retired token leaks old JWTs.
    let cache_key = if let Some(rotated) = tokens
        .refresh_token
        .filter(|t| !t.is_empty() && *t != presented)
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
        // above stands — the token presented is retired either way).
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

/// One refresh grant at the issuer's token endpoint.
fn exchange_refresh(presented: &str) -> Result<TokenSet> {
    let client = super::http_client(OP_TIMEOUT)?;
    let resp = client
        .post(endpoints().token)
        .header("accept", "application/json")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(refresh_body(presented))
        .send()
        .map_err(|e| LlmError::transport(&e))?;
    let status = resp.status().as_u16();
    let text = read_body(resp);
    if !(200..300).contains(&status) {
        return Err(token_failure(status, &text));
    }
    TokenSet::parse(&text)
}

/// Write a rotated refresh token back into the `.env` key store, in place —
/// `persist_refresh_to` at the store the boundary named. Best-effort: a store
/// that cannot be written still leaves a working session, and the failure
/// surfaces as a re-login at the next launch rather than a mid-turn error.
fn persist_refresh(rotated: &str, chain: &[&str]) {
    let Some(path) = store_path().lock().ok().and_then(|p| p.clone()) else {
        return;
    };
    persist_refresh_to(&path, rotated, chain);
}

/// Write `rotated` under [`REFRESH_ENV_VAR`] in the store at `path`, but only
/// while the store still holds this session's own `chain` — the token it
/// started with and the one it presented — or no token at all. A sign-in that
/// landed while the refresh was in flight wrote its own token there, and a
/// rotation of the chain it replaced must not overwrite it. Returns whether
/// it wrote.
fn persist_refresh_to(path: &std::path::Path, rotated: &str, chain: &[&str]) -> bool {
    super::EnvFile::update_if(path, REFRESH_ENV_VAR, rotated, |current| {
        current.is_none_or(|held| held.is_empty() || chain.contains(&held))
    })
    .is_ok_and(|written| written.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The pairs of a form-encoded body, decoded — so a test reads what the
    /// server will read, not the percent-escapes.
    fn form_pairs(body: &str) -> Vec<(String, String)> {
        url::form_urlencoded::parse(body.as_bytes())
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect()
    }

    fn field<'a>(pairs: &'a [(String, String)], name: &str) -> Option<&'a str> {
        pairs
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    /// A JWT whose payload is `claims` — unsigned, since nothing here
    /// verifies one.
    fn jwt(claims: &str) -> String {
        let encode = |bytes: &[u8]| {
            const ALPHABET: &[u8; 64] =
                b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
            let mut out = String::new();
            for chunk in bytes.chunks(3) {
                let n = chunk
                    .iter()
                    .enumerate()
                    .fold(0u32, |acc, (i, &b)| acc | (u32::from(b) << (16 - 8 * i)));
                for i in 0..=chunk.len() {
                    out.push(char::from(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize]));
                }
            }
            out
        };
        format!(
            "{}.{}.sig",
            encode(br#"{"alg":"RS256"}"#),
            encode(claims.as_bytes())
        )
    }

    // --- the flow's bodies ---

    #[test]
    fn the_device_code_request_asks_for_api_access_and_a_refresh_token() {
        let pairs = form_pairs(&device_code_body());
        assert_eq!(field(&pairs, "client_id"), Some(CLIENT_ID));
        let scope = field(&pairs, "scope").expect("a scope");
        let scopes: Vec<&str> = scope.split(' ').collect();
        // `api:access` is what lets the token call api.x.ai at all, and
        // `offline_access` is what earns the refresh token the store keeps.
        for wanted in ["openid", "offline_access", "api:access", "grok-cli:access"] {
            assert!(scopes.contains(&wanted), "{scope}");
        }
        assert_eq!(pairs.len(), 2, "nothing else: {pairs:?}");
    }

    #[test]
    fn a_poll_presents_the_device_code_under_rfc_8628s_grant() {
        let pairs = form_pairs(&device_poll_body("DEV-123"));
        assert_eq!(
            field(&pairs, "grant_type"),
            Some("urn:ietf:params:oauth:grant-type:device_code")
        );
        assert_eq!(field(&pairs, "device_code"), Some("DEV-123"));
        assert_eq!(field(&pairs, "client_id"), Some(CLIENT_ID));
    }

    #[test]
    fn the_refresh_grant_is_form_encoded_with_the_client_id() {
        let pairs = form_pairs(&refresh_body("rt-1"));
        assert_eq!(field(&pairs, "grant_type"), Some("refresh_token"));
        assert_eq!(field(&pairs, "refresh_token"), Some("rt-1"));
        assert_eq!(field(&pairs, "client_id"), Some(CLIENT_ID));
    }

    #[test]
    fn the_endpoints_hang_off_the_issuer_with_a_trailing_slash_tolerated() {
        let xai = Endpoints::for_issuer(DEFAULT_ISSUER);
        assert_eq!(xai.device_code, "https://auth.x.ai/oauth2/device/code");
        assert_eq!(xai.token, "https://auth.x.ai/oauth2/token");
        // A stub's address with a slash on the end builds the same paths.
        let stub = Endpoints::for_issuer("http://127.0.0.1:8080/");
        assert_eq!(stub.device_code, "http://127.0.0.1:8080/oauth2/device/code");
        assert_eq!(stub.token, "http://127.0.0.1:8080/oauth2/token");
    }

    // --- the device code ---

    #[test]
    fn a_device_code_response_parses_with_its_pacing() {
        let device = DeviceCode::parse(
            r#"{"device_code":"dc","user_code":"ABCD-EFGH",
                "verification_uri":"https://accounts.x.ai/oauth2/device",
                "verification_uri_complete":"https://accounts.x.ai/oauth2/device?user_code=ABCD-EFGH",
                "expires_in":600,"interval":5}"#,
        )
        .unwrap();
        assert_eq!(device.user_code(), "ABCD-EFGH");
        // The page asks the user to type the code, so it names the plain
        // page — the one-click variant would make that instruction a lie.
        assert_eq!(
            device.verification_uri(),
            "https://accounts.x.ai/oauth2/device"
        );
        assert_eq!(device.lifetime(), Duration::from_secs(600));
        assert_eq!(device.poll_interval(), Duration::from_secs(5));
        // Seconds may arrive as strings too, a tolerance the other device
        // flows extend.
        let text = DeviceCode::parse(
            r#"{"device_code":"dc","user_code":"U","verification_uri":"https://x.ai/d",
                "expires_in":"300","interval":"7"}"#,
        )
        .unwrap();
        assert_eq!(text.lifetime(), Duration::from_secs(300));
        assert_eq!(text.poll_interval(), Duration::from_secs(7));
    }

    #[test]
    fn a_device_code_with_no_usable_interval_polls_every_five_seconds() {
        // RFC 8628 lets the interval be absent, and a zero would be a tight
        // loop against the server.
        for interval in [r#""interval":0,"#, "", r#""interval":"soon","#] {
            let device = DeviceCode::parse(&format!(
                r#"{{"device_code":"dc","user_code":"U",{interval}
                    "verification_uri":"https://accounts.x.ai/d","expires_in":600}}"#
            ))
            .unwrap();
            assert_eq!(device.poll_interval(), Duration::from_secs(5), "{interval}");
        }
        // And a code that names no life still gets one to count down.
        let device = DeviceCode::parse(
            r#"{"device_code":"dc","user_code":"U","verification_uri":"https://accounts.x.ai/d"}"#,
        )
        .unwrap();
        assert!(device.lifetime() > Duration::ZERO);
    }

    #[test]
    fn a_device_code_naming_a_page_that_is_not_https_is_refused() {
        // The page is shown as a link to open, so it must be a web page this
        // machine can trust: never plain http, a file, or not a URL at all.
        for uri in [
            "http://accounts.x.ai/d",
            "file:///etc/passwd",
            "not a url",
            "",
        ] {
            let body = format!(
                r#"{{"device_code":"dc","user_code":"U","verification_uri":"{uri}","expires_in":600}}"#
            );
            assert!(DeviceCode::parse(&body).is_err(), "{uri}");
        }
    }

    #[test]
    fn a_device_code_response_without_a_code_is_refused() {
        for body in [
            r#"{"user_code":"U","verification_uri":"https://accounts.x.ai/d"}"#,
            r#"{"device_code":"dc","verification_uri":"https://accounts.x.ai/d"}"#,
            r#"{"device_code":"","user_code":"U","verification_uri":"https://accounts.x.ai/d"}"#,
            "<html>gateway timeout</html>",
        ] {
            assert!(DeviceCode::parse(body).is_err(), "{body}");
        }
    }

    // --- the poll ---

    #[test]
    fn an_approval_carries_the_token_set() {
        let verdict = poll_verdict(
            200,
            r#"{"access_token":"at","refresh_token":"rt","expires_in":21600,
                "token_type":"Bearer","id_token":"x.y.z","scope":"openid api:access"}"#,
        );
        let Poll::Approved(tokens) = verdict else {
            panic!("{verdict:?}");
        };
        assert_eq!(tokens.access_token.as_deref(), Some("at"));
        assert_eq!(tokens.refresh_token.as_deref(), Some("rt"));
        assert_eq!(tokens.expires_in, Some(21600));
    }

    #[test]
    fn an_approval_without_a_refresh_token_is_refused_at_once() {
        // Without one the session dies at the first expiry with no way back
        // but another sign-in — better said on the page that can redo it.
        let verdict = poll_verdict(200, r#"{"access_token":"at","expires_in":3600}"#);
        let Poll::Denied(reason) = verdict else {
            panic!("{verdict:?}");
        };
        assert!(reason.contains("refresh token"), "{reason}");
    }

    #[test]
    fn the_poll_reads_rfc_8628s_errors() {
        assert_eq!(
            poll_verdict(400, r#"{"error":"authorization_pending"}"#),
            Poll::Pending
        );
        assert_eq!(
            poll_verdict(400, r#"{"error":"slow_down"}"#),
            Poll::SlowDown(None)
        );
        assert_eq!(
            poll_verdict(400, r#"{"error":"slow_down","interval":10}"#),
            Poll::SlowDown(Some(Duration::from_secs(10)))
        );
        // xAI also spells a denial `authorization_denied`.
        for denied in ["access_denied", "authorization_denied"] {
            let Poll::Denied(reason) = poll_verdict(400, &format!(r#"{{"error":"{denied}"}}"#))
            else {
                panic!("{denied}");
            };
            assert!(reason.contains("denied"), "{reason}");
        }
        let Poll::Denied(expired) = poll_verdict(400, r#"{"error":"expired_token"}"#) else {
            panic!("expired");
        };
        assert!(expired.contains("expired"), "{expired}");
        // Anything else ends the flow in the server's own words.
        let Poll::Denied(other) = poll_verdict(
            400,
            r#"{"error":"invalid_client","error_description":"Client is disabled"}"#,
        ) else {
            panic!("other");
        };
        assert!(other.contains("Client is disabled"), "{other}");
    }

    #[test]
    fn a_transient_failure_is_polled_again() {
        // A proxy's error page, a rate limit, a timeout: the code is still
        // good, so the next poll may well succeed.
        for (status, body) in [
            (503, "<html>unavailable</html>"),
            (502, ""),
            (429, ""),
            (408, "timeout"),
        ] {
            assert_eq!(poll_verdict(status, body), Poll::Retry, "{status}");
        }
        // A client error the server did not explain is not one to retry.
        let Poll::Denied(reason) = poll_verdict(400, "<html>bad request</html>") else {
            panic!("400");
        };
        assert!(reason.contains("400"), "{reason}");
    }

    // --- the token set ---

    #[test]
    fn a_token_set_needs_an_access_token() {
        assert!(TokenSet::parse(r#"{"refresh_token":"rt"}"#).is_err());
        assert!(TokenSet::parse(r#"{"access_token":""}"#).is_err());
        assert!(TokenSet::parse("not json").is_err());
    }

    #[test]
    fn a_refresh_that_names_no_new_token_keeps_the_one_presented() {
        // xAI may leave the refresh token out when it did not rotate it.
        let tokens = TokenSet::parse(r#"{"access_token":"at","expires_in":3600}"#).unwrap();
        assert_eq!(tokens.refresh_token, None);
        // A life may arrive as a string, as the device code's pacing may.
        let text = TokenSet::parse(r#"{"access_token":"at","expires_in":"3600"}"#).unwrap();
        assert_eq!(text.expires_in, Some(3600));
    }

    // --- the cached lifetime ---

    #[test]
    fn the_cached_lifetime_keys_off_expires_in_then_the_bearers_exp() {
        let now = 1_000_000;
        // The grant's own duration, less the skew: immune to this clock.
        assert_eq!(
            cache_lifetime(Some(21_600), Some(now - 50), now),
            Duration::from_secs(21_600 - 300)
        );
        // Without one, the bearer's own `exp` against the clock.
        assert_eq!(
            cache_lifetime(None, Some(now + 3_600), now),
            Duration::from_secs(3_300)
        );
        // An `exp` this clock reads as past cannot be a fresh token's: the
        // floor, never a mint (and a rotation) per request.
        assert_eq!(cache_lifetime(None, Some(now - 10), now), MIN_CACHE);
        assert_eq!(cache_lifetime(None, None, now), MIN_CACHE);
        // A short life is cached for at most half of itself, never extended.
        assert_eq!(cache_lifetime(Some(90), None, now), Duration::from_secs(45));
        assert_eq!(
            cache_lifetime(Some(900), None, now),
            Duration::from_secs(600)
        );
    }

    // --- the claims ---

    #[test]
    fn the_claims_name_the_account_and_when_the_bearer_dies() {
        let claims = parse_claims(&jwt(
            r#"{"sub":"u1","email":"grok@example.com","exp":1700000000}"#,
        ))
        .unwrap();
        assert_eq!(claims.email.as_deref(), Some("grok@example.com"));
        assert_eq!(claims.expires_at, Some(1_700_000_000));
        // A handle stands in for an account with no email claim.
        let handle = parse_claims(&jwt(r#"{"preferred_username":"grokker"}"#)).unwrap();
        assert_eq!(handle.email.as_deref(), Some("grokker"));
        // An opaque token is not a JWT, and says nothing.
        assert_eq!(parse_claims("opaque-token"), None);
        assert_eq!(parse_claims("a.!!!.c"), None);
    }

    // --- the advice ---

    #[test]
    fn the_advice_tells_a_stale_token_from_a_missing_subscription() {
        let again = auth_advice(401, r#"{"error":"invalid_token"}"#).unwrap();
        assert!(again.contains("/login"), "{again}");
        // xAI answers a bearer it no longer honours with a 403 of its own
        // shape, and that one *is* fixed by signing in again.
        let stale = auth_advice(
            403,
            r#"{"code":"The caller does not have permission to execute the specified operation",
                "error":"The OAuth2 access token could not be validated. [WKE=unauthenticated:bad-credentials]"}"#,
        )
        .unwrap();
        assert!(stale.contains("/login"), "{stale}");
        // Every other 403 is the account, and signing in again cannot fix it.
        let unsubscribed = auth_advice(
            403,
            r#"{"code":"The caller does not have permission to execute the specified operation",
                "error":"You have either run out of available resources or do not have an active Grok subscription. Manage subscriptions at https://grok.com"}"#,
        )
        .unwrap();
        assert!(unsubscribed.contains("subscription"), "{unsubscribed}");
        assert!(!unsubscribed.contains("/login"), "{unsubscribed}");
        let limited = auth_advice(429, "{}").unwrap();
        assert!(limited.contains("rate-limiting"), "{limited}");
        // Nothing to add to a failure that is not about the account.
        assert_eq!(auth_advice(500, "{}"), None);
        assert_eq!(auth_advice(400, r#"{"error":"bad schema"}"#), None);
    }

    #[test]
    fn a_refused_refresh_is_a_sign_in_to_redo_but_a_403_is_the_account() {
        for (status, body) in [
            (
                400,
                r#"{"error":"invalid_grant","error_description":"refresh token revoked"}"#,
            ),
            (401, r#"{"error":"invalid_client"}"#),
        ] {
            let advice = refresh_advice(status, body).unwrap();
            assert!(advice.contains("/login"), "{status}: {advice}");
        }
        // Signing in again would mint the same refusal.
        let tier = refresh_advice(403, "{}").unwrap();
        assert!(!tier.contains("/login"), "{tier}");
        assert!(tier.contains("subscription"), "{tier}");
    }

    // --- the poll's pacing ---

    #[test]
    fn a_slow_down_grows_the_interval_and_never_past_the_cap() {
        let five = Duration::from_secs(5);
        // RFC 8628's five seconds more, unless the server named its own.
        assert_eq!(slowed(five, None), Duration::from_secs(10));
        assert_eq!(
            slowed(five, Some(Duration::from_secs(12))),
            Duration::from_secs(12)
        );
        // A slow-down never speeds the poll up, nor slows it past the cap.
        assert_eq!(slowed(five, Some(Duration::from_secs(1))), five);
        assert_eq!(slowed(five, Some(Duration::from_secs(3_600))), MAX_INTERVAL);
        assert_eq!(slowed(MAX_INTERVAL, None), MAX_INTERVAL);
    }

    // --- the access-token cache ---

    fn cache_test_tokens(rotated: &str) -> TokenSet {
        TokenSet {
            access_token: Some(format!("access-for-{rotated}")),
            refresh_token: Some(rotated.to_string()),
            expires_in: Some(3600),
        }
    }

    #[test]
    fn a_minted_bearer_names_the_account_it_belongs_to() {
        let key = "xai-account-original";
        let bearer = jwt(r#"{"email":"grok@example.com","exp":4102444800}"#);
        let access = authorize_with(key, |_| {
            Ok(TokenSet {
                access_token: Some(bearer.clone()),
                refresh_token: None,
                expires_in: Some(21_600),
            })
        })
        .unwrap();
        assert_eq!(access.bearer, bearer, "the access token is the bearer");
        assert_eq!(account_label(&access).as_deref(), Some("grok@example.com"));
        // Cached: a second request mints nothing.
        let hit = authorize_with(key, |_| panic!("a fresh bearer is cached")).unwrap();
        assert_eq!(hit.bearer, bearer);
        forget(key);
    }

    #[test]
    fn auth_cache_regression_forget_does_not_wait_for_a_refresh_in_flight() {
        // `forget` runs on the TUI loop when a sign-in lands. A refresh in
        // flight on a backend thread can hold the gate for the whole request
        // timeout, and the loop must never wait behind it — while the
        // refresh that was in flight must not leave its bearer behind under
        // the forgotten key: the next request exchanges afresh.
        use std::sync::Barrier;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let key = "xai-forget-inflight";
        let started = Barrier::new(2);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                authorize_with(key, |_| {
                    started.wait();
                    std::thread::sleep(Duration::from_millis(1500));
                    Ok(cache_test_tokens("xai-forget-inflight-rotated"))
                })
                .unwrap();
            });
            started.wait();
            let asked = Instant::now();
            forget(key);
            assert!(
                asked.elapsed() < Duration::from_millis(700),
                "forget waited on the refresh: {:?}",
                asked.elapsed()
            );
        });
        let exchanges = AtomicUsize::new(0);
        authorize_with(key, |_| {
            exchanges.fetch_add(1, Ordering::SeqCst);
            Ok(cache_test_tokens("xai-forget-inflight-again"))
        })
        .unwrap();
        assert_eq!(
            exchanges.load(Ordering::SeqCst),
            1,
            "the forgotten account exchanges afresh"
        );
        forget(key);
    }

    #[test]
    fn auth_cache_regression_a_refresh_after_a_sign_ins_forget_is_cached() {
        // A sign-in forgets the token it stores before anything has refreshed
        // it. That forget is about bearers minted *before* it, which it has
        // already dropped; the first refresh after it must keep what it
        // mints, or the next request refreshes — and rotates — again
        // (`smoke.sh` Phase 174 counted two refreshes where one does).
        let key = "xai-forgotten-before";
        forget(key);
        let first =
            authorize_with(key, |_| Ok(cache_test_tokens("xai-forgotten-before-r1"))).unwrap();
        let hit = authorize_with(key, |_| {
            panic!("the first refresh after a sign-in keeps its bearer")
        })
        .unwrap();
        assert_eq!(hit.bearer, first.bearer);
        forget(key);
    }

    #[test]
    fn auth_cache_regression_a_rotation_never_overwrites_a_newer_sign_in() {
        // The rotated token is written back only while the store still holds
        // this session's own chain. A sign-in that landed while the refresh
        // was in flight wrote its own token there, and overwriting it would
        // make the next launch present a retired token.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".env");
        let stored = |path: &std::path::Path| {
            crate::llm::EnvFile::parse(&std::fs::read_to_string(path).unwrap())
                .get(REFRESH_ENV_VAR)
                .map(str::to_string)
        };
        std::fs::write(&path, format!("{REFRESH_ENV_VAR}=original\nOTHER=keep\n")).unwrap();
        assert!(persist_refresh_to(
            &path,
            "rotated-1",
            &["original", "original"]
        ));
        assert_eq!(stored(&path).as_deref(), Some("rotated-1"));
        assert!(persist_refresh_to(
            &path,
            "rotated-2",
            &["original", "rotated-1"]
        ));
        assert_eq!(stored(&path).as_deref(), Some("rotated-2"));
        // A sign-in landed meanwhile: the in-flight rotation keeps its hands off.
        std::fs::write(&path, format!("{REFRESH_ENV_VAR}=newer-sign-in\n")).unwrap();
        assert!(!persist_refresh_to(
            &path,
            "rotated-3",
            &["original", "rotated-2"]
        ));
        assert_eq!(stored(&path).as_deref(), Some("newer-sign-in"));
        // No token in the store (a credential from the process environment)
        // or no store at all: written.
        std::fs::write(&path, "OTHER=keep\n").unwrap();
        assert!(persist_refresh_to(
            &path,
            "rotated-4",
            &["original", "original"]
        ));
        assert_eq!(stored(&path).as_deref(), Some("rotated-4"));
        let fresh = dir.path().join("fresh").join(".env");
        assert!(persist_refresh_to(
            &fresh,
            "rotated-5",
            &["original", "original"]
        ));
        assert_eq!(stored(&fresh).as_deref(), Some("rotated-5"));
    }

    #[test]
    fn auth_cache_regression_concurrent_misses_refresh_once() {
        // xAI's refresh tokens are single-use: a second worker presenting the
        // one the first just retired would be refused.
        use std::sync::Barrier;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let key = "xai-concurrent-original";
        let barrier = Barrier::new(32);
        let exchanges = AtomicUsize::new(0);
        std::thread::scope(|scope| {
            let threads: Vec<_> = (0..32)
                .map(|_| {
                    scope.spawn(|| {
                        barrier.wait();
                        authorize_with(key, |presented| {
                            let count = exchanges.fetch_add(1, Ordering::SeqCst);
                            std::thread::sleep(Duration::from_millis(25));
                            if count != 0 || presented != key {
                                return Err(LlmError::Decode("refresh token reused".to_string()));
                            }
                            Ok(cache_test_tokens("xai-concurrent-rotated"))
                        })
                    })
                })
                .collect();
            let results: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
            assert_eq!(exchanges.load(Ordering::SeqCst), 1);
            assert!(results.iter().all(Result::is_ok));
            assert!(results.windows(2).all(|pair| {
                pair[0].as_ref().unwrap().bearer == pair[1].as_ref().unwrap().bearer
            }));
        });
        forget(key);
    }

    #[test]
    fn auth_cache_regression_old_alias_hits_the_newest_bearer_after_expiry() {
        let original = "xai-expiry-original";
        let second = "xai-expiry-second";
        let third = "xai-expiry-third";
        authorize_with(original, |_| Ok(cache_test_tokens(second))).unwrap();
        cache().lock().unwrap().get_mut(second).unwrap().until = Instant::now();
        let refreshed = authorize_with(second, |presented| {
            assert_eq!(presented, second);
            Ok(cache_test_tokens(third))
        })
        .unwrap();
        let from_original = authorize_with(original, |_| {
            panic!("old aliases must hit the freshly minted bearer")
        })
        .unwrap();
        assert_eq!(from_original.bearer, refreshed.bearer);
        assert_eq!(live_refresh_token(original), third);
        forget(original);
        forget(second);
        forget(third);
    }

    #[test]
    fn auth_cache_regression_forget_clears_every_rotation_alias() {
        let original = "xai-forget-original";
        let second = "xai-forget-second";
        let third = "xai-forget-third";
        authorize_with(original, |_| Ok(cache_test_tokens(second))).unwrap();
        cache().lock().unwrap().get_mut(second).unwrap().until = Instant::now();
        authorize_with(second, |_| Ok(cache_test_tokens(third))).unwrap();
        forget(original);
        for key in [original, second, third] {
            assert_eq!(live_refresh_token(key), key);
            assert!(!cache().lock().unwrap().contains_key(key));
        }
    }

    #[test]
    fn auth_cache_regression_a_failed_refresh_can_be_retried() {
        let key = "xai-retry-original";
        assert!(
            authorize_with(key, |_| Err(LlmError::Http(
                "temporary failure".to_string()
            )))
            .is_err()
        );
        let fresh = authorize_with(key, |presented| {
            assert_eq!(presented, key);
            Ok(cache_test_tokens("xai-retry-rotated"))
        })
        .unwrap();
        let hit = authorize_with(key, |_| panic!("successful retry must be cached")).unwrap();
        assert_eq!(hit.bearer, fresh.bearer);
        forget(key);
    }

    #[test]
    fn auth_cache_regression_rotations_do_not_retain_obsolete_bearers() {
        let original = "xai-bounded-original";
        for index in 0..64 {
            let current = live_refresh_token(original);
            if let Some(entry) = cache().lock().unwrap().get_mut(&current) {
                entry.until = Instant::now();
            }
            authorize_with(original, |_| {
                Ok(cache_test_tokens(&format!("xai-bounded-{index}")))
            })
            .unwrap();
        }
        let entries = cache()
            .lock()
            .unwrap()
            .keys()
            .filter(|key| key.starts_with("xai-bounded-"))
            .count();
        assert_eq!(entries, 1);
        forget(original);
    }

    #[test]
    fn auth_cache_regression_rotation_from_a_reloaded_config_updates_old_configs() {
        let original = "xai-reloaded-original";
        let second = "xai-reloaded-second";
        let third = "xai-reloaded-third";
        note_rotation(original, original, second);
        note_rotation(second, second, third);
        assert_eq!(live_refresh_token(original), third);
        forget(original);
        forget(second);
    }
}
