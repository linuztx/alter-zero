//! xAI as a **subscription**: Grok's device-code sign-in for an xAI account on
//! a SuperGrok plan, the refresh token it fills the `.env` store with, and the
//! short-lived access token every request is minted with. See `docs/xai.md`.
//!
//! The sixth sign-in, and it keeps the two-layer split every other one keeps
//! (`docs/chatgpt.md`): a long-lived token in the `.env` key store, exchanged
//! (and cached in memory, never written) for the short-lived credential the
//! API actually takes.
//!
//! # The flow
//!
//! xAI documents no OAuth flow for other apps. Its own Grok CLI signs in with
//! an RFC 8628 device code on a public client id, and this is that flow as two
//! open-source agents port it — pi (`packages/ai/src/auth/oauth/xai.ts`) and
//! hermes-agent (`hermes_cli/auth_xai.py`), whose constants agree:
//!
//! 1. **The device code** — `POST {issuer}/oauth2/device/code` with the client
//!    id and the scopes, form-encoded. The answer carries the code the poll
//!    presents, the short code the user approves, and the page to approve it
//!    at — with the code already filled in, which is the link the page shows.
//! 2. **The poll** — `POST {issuer}/oauth2/token` with the device-code grant
//!    until the user approves, the code expires, or they deny it.
//! 3. **The refresh** — the same endpoint with `grant_type=refresh_token`.
//!    Refresh tokens are **single-use**: each refresh rotates the pair and
//!    revokes the token presented, which is why refreshes here are
//!    serialized and every rotation is written straight back.
//!
//! And the API the bearer reaches has three differences from the Responses
//! wire the ChatGPT backend speaks, which [`serves`] tells the request builder
//! to make (`docs/xai.md` *The wire*).
//!
//! The pure halves (the request bodies, the response parses, the poll's
//! verdict, the link check, the freshness rule, the account's name, the
//! schema sanitizer, the advice) are unit-tested; the HTTP calls are boundary
//! code.

use std::collections::HashMap;
use std::io::Read;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::Value;

use super::config::{AuthScheme, ModelConfig};
use super::mcp::form_encode;
use super::{LlmError, Result};
use crate::stream::CancelToken;

/// The **public** client id Grok's own CLI signs in with, and the one both
/// references use. A device-code flow has no client secret, so this is all a
/// client needs.
pub const CLIENT_ID: &str = "b1a00492-073a-47ea-816f-4c329264a828";

/// The scopes the sign-in asks for: an OpenID identity (`openid profile
/// email` — the email, or the handle, is what the confirmation names), a
/// refresh token (`offline_access`), and the access the token is for —
/// `api:access` is what lets it reach `api.x.ai`.
pub const SCOPE: &str = "openid profile email offline_access grok-cli:access api:access";

/// Where the sign-in and the token mint go, unless [`ISSUER_ENV`] says
/// otherwise.
const DEFAULT_ISSUER: &str = "https://auth.x.ai";

/// The environment variable that points the sign-in and the token mint at
/// another auth host — the smoke suite's local stand-in (Phase 174), or a
/// fork's own. Read at the boundary and handed in once through
/// [`set_issuer`]: the pure builders never read the environment.
pub const ISSUER_ENV: &str = "ALTER_ZERO_XAI_ISSUER";

/// The two paths the flow lives on.
const DEVICE_CODE_PATH: &str = "/oauth2/device/code";
const TOKEN_PATH: &str = "/oauth2/token";

/// RFC 8628's device-code grant type.
const DEVICE_GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";

/// The code's life when the answer names none — fifteen minutes, the life a
/// device code commonly has — and RFC 8628's own default poll interval.
const DEFAULT_EXPIRES: u64 = 900;
const DEFAULT_INTERVAL: u64 = 5;

/// What a `slow_down` that names no interval of its own adds to the current
/// one: RFC 8628 §3.5's five seconds. One that names an interval is taken at
/// its word instead (pi's fix for a clock that drifts in a VM).
const SLOW_DOWN_STEP: u64 = 5;

/// The slowest the poll ever gets. An answer naming an interval of an hour —
/// at the start or in a `slow_down` — would otherwise leave the page counting
/// down long after the user approved, or past the code's own life.
pub const MAX_INTERVAL: Duration = Duration::from_secs(60);

/// How much of an access token's life is given back before it is re-minted,
/// so a request never leaves with a token that expires in flight.
const REFRESH_SKEW: Duration = Duration::from_secs(300);

/// The cache lifetime when nothing names an expiry. An explicit short
/// lifetime always wins: caching past it would keep sending a dead bearer.
const MIN_CACHE: Duration = Duration::from_secs(60);

/// Per-operation deadline for the flow's requests — a small round trip each,
/// with a user watching.
const OP_TIMEOUT: Duration = Duration::from_secs(30);

/// The most any of the flow's responses may buffer — the other sign-ins'
/// bound. It also bounds what a *failure* can put on the page: an
/// intercepting proxy answers a blocked request with an HTML page, and that
/// page would otherwise become the "reason" the user reads.
const BODY_MAX_BYTES: u64 = 64 * 1024;

/// One nap between cancel checks while the poll waits.
const POLL_NAP: Duration = Duration::from_millis(200);

/// The environment variable the refresh token lives under — the same name
/// `providers.toml` gives the provider's `api_key_env`, pinned here because
/// the rotation write-back cannot ask the provider file.
pub const REFRESH_ENV_VAR: &str = "XAI_GROK_REFRESH_TOKEN";

/// xAI's own domain: its API is `api.x.ai`, its auth host `auth.x.ai`.
const XAI_DOMAIN: &str = "x.ai";

/// The sentences the advice gives — one per thing a user can do.
const SIGN_IN_AGAIN: &str = "your xAI sign-in is no longer valid — run /login and sign in again.";
const REJECTED_BEARER: &str = "xAI no longer accepts this sign-in's access token — send your \
     message again; if it keeps failing, run /login and sign in again.";
const NO_SUBSCRIPTION: &str = "this xAI account has no active Grok subscription, or has used \
     what its plan includes — check grok.com/?_s=usage, or switch provider with /model.";
const OUT_OF_CREDITS: &str = "this xAI account is out of credits — add credits or upgrade at \
     grok.com, or switch provider with /model.";
const TIER_DENIED: &str = "xAI refused to renew this sign-in: the account isn't allowed API \
     access through the Grok sign-in — xAI may limit it to some SuperGrok tiers, and signing \
     in again won't change that. Check grok.com, or switch provider with /model.";

/// The overridable issuer — [`DEFAULT_ISSUER`] until the boundary sets one.
fn issuer_slot() -> &'static Mutex<Option<String>> {
    static SLOT: OnceLock<Mutex<Option<String>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

/// Point the sign-in and the token mint at `url` instead of xAI's own auth
/// host. A trailing slash is dropped so the paths built on it never double
/// one up. Boundary-only; called once at startup.
pub fn set_issuer(url: impl Into<String>) {
    let url = url.into();
    let trimmed = url.trim().trim_end_matches('/').to_string();
    if let Ok(mut slot) = issuer_slot().lock() {
        *slot = (!trimmed.is_empty()).then_some(trimmed);
    }
}

/// The issuer in force.
fn issuer() -> String {
    issuer_slot()
        .lock()
        .ok()
        .and_then(|slot| slot.clone())
        .unwrap_or_else(|| DEFAULT_ISSUER.to_string())
}

// ---------------------------------------------------------------------------
// The flow's bodies (pure)
// ---------------------------------------------------------------------------

/// The device-code request's body: the client id and the scopes — exactly
/// what hermes sends. pi adds a `referrer` naming itself, which nothing needs.
#[must_use]
pub fn device_code_body() -> String {
    form_encode(&[
        (String::from("client_id"), CLIENT_ID.to_string()),
        (String::from("scope"), SCOPE.to_string()),
    ])
}

/// One poll's body: the device-code grant, the client and the code, per
/// RFC 8628.
#[must_use]
pub fn device_poll_body(device_code: &str) -> String {
    form_encode(&[
        (String::from("grant_type"), DEVICE_GRANT.to_string()),
        (String::from("client_id"), CLIENT_ID.to_string()),
        (String::from("device_code"), device_code.to_string()),
    ])
}

/// The refresh grant's body. No scope: the grant carries the one it was
/// issued with.
#[must_use]
pub fn refresh_body(refresh_token: &str) -> String {
    form_encode(&[
        (String::from("grant_type"), String::from("refresh_token")),
        (String::from("client_id"), CLIENT_ID.to_string()),
        (String::from("refresh_token"), refresh_token.to_string()),
    ])
}

// ---------------------------------------------------------------------------
// The device flow's wire shapes (pure)
// ---------------------------------------------------------------------------

/// What the device-code request answered: the code the poll presents, and
/// the short code the user approves in a browser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceCode {
    /// The server's handle on this attempt — presented by every poll, never
    /// shown.
    device_code: String,
    /// The short code the user approves.
    user_code: String,
    /// The page to approve it at.
    verification_uri: String,
    /// The same page with the code filled in — what the page links when the
    /// answer names it, so one click is the whole step.
    verification_uri_complete: Option<String>,
    /// How long the code stays valid, in seconds.
    expires_in: u64,
    /// The minimum seconds between polls, as sent.
    interval: u64,
}

impl DeviceCode {
    /// Parse the code response.
    ///
    /// # Errors
    /// A body that isn't the response's shape, names no code or no page to
    /// approve it at, or names a page that is not a trusted link
    /// ([`is_trusted_link`]) is a decode error — there is nothing to show
    /// or poll with either way.
    pub fn parse(body: &str) -> Result<Self> {
        let raw: RawDeviceCode =
            serde_json::from_str(body).map_err(|e| LlmError::Decode(e.to_string()))?;
        if raw.device_code.is_empty() || raw.user_code.is_empty() || raw.verification_uri.is_empty()
        {
            return Err(LlmError::Decode("xAI returned no device code".to_string()));
        }
        let complete = raw
            .verification_uri_complete
            .filter(|link| !link.trim().is_empty());
        // Both links are checked, the plain one too: an answer naming a link
        // that is no web page is not one to show any part of.
        for link in std::iter::once(&raw.verification_uri).chain(complete.as_ref()) {
            if !is_trusted_link(link) {
                return Err(LlmError::Decode(format!(
                    "xAI's device code named a page that is not a secure web link: {link}"
                )));
            }
        }
        // A zero is no pacing at all — RFC 8628 allows an interval of 0,
        // which here would be a tight loop — so it reads as the default.
        let seconds = |raw: Option<RawSeconds>, default: u64| {
            raw.and_then(|s| s.seconds())
                .filter(|&s| s > 0)
                .unwrap_or(default)
        };
        Ok(Self {
            device_code: raw.device_code,
            user_code: raw.user_code,
            verification_uri: raw.verification_uri,
            verification_uri_complete: complete,
            expires_in: seconds(raw.expires_in, DEFAULT_EXPIRES),
            interval: seconds(raw.interval, DEFAULT_INTERVAL),
        })
    }

    /// The link the page shows: the prefilled page when the answer named
    /// one, else the plain one.
    #[must_use]
    pub fn verification_link(&self) -> &str {
        self.verification_uri_complete
            .as_deref()
            .unwrap_or(&self.verification_uri)
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

    /// How long to wait between polls: what the server asked, held between a
    /// second (a zero would be a tight loop) and [`MAX_INTERVAL`].
    #[must_use]
    pub fn poll_interval(&self) -> Duration {
        Duration::from_secs(self.interval).clamp(Duration::from_secs(1), MAX_INTERVAL)
    }
}

/// Is `link` one to put on the page for the user to click — `https`, or
/// `http` to this machine's loopback (the smoke suite's stub)? Anything else
/// in a device-code answer is refused rather than shown: the page is a link,
/// and pi's same check exists so a hostile answer cannot hand the user
/// something other than a web page.
#[must_use]
pub fn is_trusted_link(link: &str) -> bool {
    let Ok(url) = url::Url::parse(link.trim()) else {
        return false;
    };
    match url.scheme() {
        "https" => url.host().is_some(),
        "http" => match url.host() {
            Some(url::Host::Domain(host)) => host.eq_ignore_ascii_case("localhost"),
            Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
            Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
            None => false,
        },
        _ => false,
    }
}

/// The wire shape of the code response. `expires_in` and `interval` are read
/// as a number or a string holding one — the tolerance `docs/chatgpt.md`'s
/// device code extends.
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

/// A count of seconds as an answer spells it: a whole number, a fractional
/// one, or a string holding either.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RawSeconds {
    Whole(u64),
    Fraction(f64),
    Text(String),
}

impl RawSeconds {
    /// The whole seconds, rounded down; `None` for a negative, non-finite or
    /// unparseable value.
    fn seconds(&self) -> Option<u64> {
        // Rounded down; a float-to-integer cast saturates rather than wraps.
        let from_float = |f: f64| (f.is_finite() && f >= 0.0).then(|| f.floor() as u64);
        match self {
            Self::Whole(n) => Some(*n),
            Self::Fraction(f) => from_float(*f),
            Self::Text(text) => {
                let text = text.trim();
                text.parse::<u64>()
                    .ok()
                    .or_else(|| text.parse::<f64>().ok().and_then(from_float))
            }
        }
    }
}

/// What one poll of the token endpoint established. Pending and slow-down
/// arrive as an OAuth **error in the body** of a 4xx, so the verdict reads
/// the JSON rather than the status, and a body carrying a token is the
/// approval whatever status delivered it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Poll {
    /// The user approved the code: the token set it minted.
    Approved(TokenSet),
    /// Not approved yet — ask again after the interval.
    Pending,
    /// The server asked for a slower cadence — at the interval it named,
    /// when it named one.
    SlowDown(Option<u64>),
    /// A failure on the way there rather than an answer — a proxy's error
    /// page, a rate limit, a timeout, a server briefly down. The code is
    /// still good: ask again after the interval.
    Retry,
    /// The flow is over — the sentence to show.
    Denied(String),
}

/// Read one poll's answer: an access token is an approval, the RFC 8628
/// errors are a wait, a slower wait or the end of the flow, a status saying
/// the trip failed (408, 429, a 5xx) with no such error in its body is one
/// more wait, and any other answer ends the flow naming its status.
#[must_use]
pub fn poll_verdict(status: u16, body: &str) -> Poll {
    #[derive(Debug, Default, Deserialize)]
    #[serde(default)]
    struct Raw {
        error: Option<String>,
        error_description: Option<String>,
        interval: Option<RawSeconds>,
    }
    if let Ok(tokens) = TokenSet::parse(body) {
        return Poll::Approved(tokens);
    }
    // A timeout, a rate limit, a server or a proxy in front of it failing:
    // none of those says anything about the code, which is still good, so the
    // next poll may well succeed — unless the body is an answer this flow
    // knows, which keeps its meaning whatever status carried it.
    let transient = status == 408 || status == 429 || status >= 500;
    let Ok(raw) = serde_json::from_str::<Raw>(body) else {
        if transient {
            return Poll::Retry;
        }
        return Poll::Denied(format!("xAI answered {status} to the device code poll"));
    };
    // A denial carries the server's own sentence when it wrote one; the fixed
    // line is the fallback, because a bare code is not something a user can
    // act on.
    let denial = |fallback: &str| {
        raw.error_description
            .clone()
            .filter(|d| !d.trim().is_empty())
            .unwrap_or_else(|| fallback.to_string())
    };
    match raw.error.as_deref() {
        Some("authorization_pending") => Poll::Pending,
        Some("slow_down") => Poll::SlowDown(
            raw.interval
                .as_ref()
                .and_then(RawSeconds::seconds)
                .filter(|&secs| secs > 0),
        ),
        Some("access_denied" | "authorization_denied") => Poll::Denied(denial(
            "the sign-in was denied in the browser — press Esc and try again",
        )),
        Some("expired_token") => Poll::Denied(denial(
            "the device code expired — press Esc and sign in again",
        )),
        Some("invalid_grant") => Poll::Denied(denial(
            "the device code is no longer valid — press Esc and sign in again",
        )),
        _ if transient => Poll::Retry,
        Some(other) => Poll::Denied(format!(
            "xAI refused the sign-in: {other} — press Esc and try again"
        )),
        None => Poll::Denied(format!("xAI answered {status} to the device code poll")),
    }
}

/// What the token endpoint answers, for the device-code grant and the
/// refresh grant alike.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TokenSet {
    /// The bearer the API takes — a JWT.
    pub access_token: String,
    /// The refresh token. Required from the sign-in; a **refresh** may leave
    /// it out, meaning "keep the one you have".
    pub refresh_token: Option<String>,
    /// The access token's life, in seconds.
    pub expires_in: Option<u64>,
    /// The OpenID identity token, whose `email` — or, for an account that
    /// shares none, `preferred_username` — claim names the account.
    pub id_token: Option<String>,
}

impl TokenSet {
    /// Parse a token response.
    ///
    /// # Errors
    /// A body that isn't an object, or that carries no access token, is a
    /// decode error — there is nothing to authenticate with either way.
    pub fn parse(body: &str) -> Result<Self> {
        #[derive(Debug, Default, Deserialize)]
        #[serde(default)]
        struct Raw {
            access_token: Option<String>,
            refresh_token: Option<String>,
            expires_in: Option<RawSeconds>,
            id_token: Option<String>,
        }
        let raw: Raw = serde_json::from_str(body).map_err(|e| LlmError::Decode(e.to_string()))?;
        let access_token = raw
            .access_token
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .ok_or_else(|| LlmError::Decode("xAI returned no access token".to_string()))?;
        let text = |value: Option<String>| {
            value
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty())
        };
        Ok(Self {
            access_token,
            refresh_token: text(raw.refresh_token),
            expires_in: raw.expires_in.as_ref().and_then(RawSeconds::seconds),
            id_token: text(raw.id_token),
        })
    }
}

/// How long a minted access token may be cached: the **shorter** of the
/// grant's own `expires_in` (a duration, immune to this machine's clock) and
/// the JWT's absolute `exp` against the wall clock `now` (both Unix seconds)
/// — the two references each trust only one, and device-code logins have
/// been seen minting JWTs far shorter-lived than a token's usual hours. An
/// `exp` that reads as already past is the clock running ahead, not a dead
/// token, and is ignored. A five-minute `REFRESH_SKEW` comes off the end, a
/// short life is cached for at most half of it, and when neither says, the
/// `MIN_CACHE` floor applies.
#[must_use]
pub fn cache_lifetime(expires_in: Option<u64>, jwt_exp: Option<u64>, now: u64) -> Duration {
    let from_exp = jwt_exp.filter(|&exp| exp > now).map(|exp| exp - now);
    let life = match (expires_in, from_exp) {
        (Some(grant), Some(jwt)) => Some(grant.min(jwt)),
        (grant, jwt) => grant.or(jwt),
    };
    let Some(secs) = life else {
        return MIN_CACHE;
    };
    let life = Duration::from_secs(secs);
    life.saturating_sub(REFRESH_SKEW)
        .max(MIN_CACHE.min(life / 2))
}

/// Whose account the sign-in's tokens name, for the confirmation: an `email`
/// claim — the `id_token`'s, else the access token's — else, for an account
/// whose tokens name no email, its `preferred_username` handle, looked for
/// the same way. Read and never verified: nothing here checks a JWT
/// signature.
#[must_use]
pub fn account_name(tokens: &TokenSet) -> Option<String> {
    let claims: Vec<Value> = [
        tokens.id_token.as_deref(),
        Some(tokens.access_token.as_str()),
    ]
    .into_iter()
    .flatten()
    .filter_map(jwt_claims)
    .collect();
    let named = |claim: &str| {
        claims.iter().find_map(|claims| {
            claims
                .get(claim)?
                .as_str()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        })
    };
    named("email").or_else(|| named("preferred_username"))
}

/// A JWT's claims, read without a signature check.
fn jwt_claims(jwt: &str) -> Option<Value> {
    let payload = jwt.split('.').nth(1)?;
    let bytes = b64url_decode(payload)?;
    serde_json::from_slice(&bytes).ok()
}

/// An access token's own `exp` claim.
fn jwt_exp(jwt: &str) -> Option<u64> {
    jwt_claims(jwt)?.get("exp")?.as_u64()
}

/// Decode base64url (RFC 4648 §5), padded or not — `chatgpt`'s decoder,
/// local so this module's pure half stands alone.
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
// What a request to xAI's API does differently (pure)
// ---------------------------------------------------------------------------

/// Is `base` an xAI host (`x.ai`, or any `*.x.ai`)?
#[must_use]
pub fn is_api_base(base: &str) -> bool {
    url::Url::parse(base.trim())
        .ok()
        .and_then(|url| {
            url.host_str().map(|host| {
                let host = host.to_ascii_lowercase();
                host == XAI_DOMAIN || host.ends_with(&format!(".{XAI_DOMAIN}"))
            })
        })
        .unwrap_or(false)
}

/// Does `cfg` send its requests to xAI's API — the sign-in, or any provider
/// whose base is an xAI host (a user's own pasted-key block)? What decides
/// the three request differences, and the two-listing catalog, which are the
/// *API's* and not the credential's (`docs/xai.md`).
#[must_use]
pub fn serves(cfg: &ModelConfig) -> bool {
    cfg.auth == AuthScheme::XaiGrok || is_api_base(&cfg.api_base)
}

/// Drop what xAI's `/responses` refuses from a tool's parameter `schema`, in
/// place: the `pattern` and `format` keywords, and an `enum` with a `/` in a
/// value (which takes its `enum` away, never its `type`). Each answers with a
/// 400 (hermes #27197), and an MCP server's schemas often carry one.
///
/// It walks the schema by its **subschemas** — `properties`, `items`,
/// `anyOf` and the rest — so a property *named* `pattern` or `format` is
/// kept, as is every other key a property map holds.
pub fn sanitize_schema(schema: &mut Value) {
    let Some(object) = schema.as_object_mut() else {
        // `true`/`false` schemas, and the arrays an owner walks itself below.
        return;
    };
    object.remove("pattern");
    object.remove("format");
    let slashed = object
        .get("enum")
        .and_then(Value::as_array)
        .is_some_and(|values| {
            values
                .iter()
                .any(|v| v.as_str().is_some_and(|text| text.contains('/')))
        });
    if slashed {
        object.remove("enum");
    }
    // Maps from a name to a subschema: the names are data, the values schemas.
    for key in [
        "properties",
        "patternProperties",
        "$defs",
        "definitions",
        "dependentSchemas",
    ] {
        if let Some(map) = object.get_mut(key).and_then(Value::as_object_mut) {
            map.values_mut().for_each(sanitize_schema);
        }
    }
    // A subschema — or, for `items` in its older tuple form, a list of them.
    for key in [
        "items",
        "additionalItems",
        "additionalProperties",
        "unevaluatedItems",
        "unevaluatedProperties",
        "propertyNames",
        "contains",
        "not",
        "if",
        "then",
        "else",
    ] {
        match object.get_mut(key) {
            Some(Value::Array(list)) => list.iter_mut().for_each(sanitize_schema),
            Some(sub) => sanitize_schema(sub),
            None => {}
        }
    }
    // Lists of subschemas.
    for key in ["anyOf", "oneOf", "allOf", "prefixItems"] {
        if let Some(list) = object.get_mut(key).and_then(Value::as_array_mut) {
            list.iter_mut().for_each(sanitize_schema);
        }
    }
}

// ---------------------------------------------------------------------------
// The advice (pure)
// ---------------------------------------------------------------------------

/// Is this a refusal of the **bearer** — a 401, or the 403 xAI answers an
/// access token it can no longer validate with (`[WKE=unauthenticated:
/// bad-credentials]`)? Such a bearer must not be sent again: [`reject_access`]
/// drops it.
#[must_use]
pub fn is_rejected_bearer(status: u16, body: &str) -> bool {
    if status == 401 {
        return true;
    }
    let lowered = body.to_ascii_lowercase();
    lowered.contains("bad-credentials")
        || lowered.contains("access token could not be validated")
        || lowered.contains("wke=unauthenticated")
}

/// Explain a refused request in terms the user can act on. xAI's refusals
/// come in a `{"code": …, "error": …}` envelope, and the ones a user can do
/// something about are 403s that each want a different answer: a bearer it
/// no longer accepts, no subscription, no credits — and any other 403 is the
/// plan's reach. Everything else keeps xAI's own words, lifted out of the
/// envelope; a refusal with none passes through untouched (`None`).
///
/// `status` 0 is a failure reported inside the stream, whose `body` is the
/// message alone — the no-subscription refusal can arrive that way too.
#[must_use]
pub fn auth_advice(status: u16, body: &str) -> Option<String> {
    if is_rejected_bearer(status, body) {
        return Some(REJECTED_BEARER.to_string());
    }
    let lowered = body.to_ascii_lowercase();
    // Credits first: their sentence names a "Grok subscription" too.
    if lowered.contains("spending-limit") || lowered.contains("run out of credits") {
        return Some(OUT_OF_CREDITS.to_string());
    }
    if lowered.contains("active grok subscription")
        || lowered.contains("out of available resources")
    {
        return Some(NO_SUBSCRIPTION.to_string());
    }
    let words = envelope_message(body);
    let lead = |words: Option<String>, fallback: &str| match words {
        Some(words) => words.trim_end_matches('.').to_string(),
        None => fallback.to_string(),
    };
    match status {
        // Any other 403 on a sign-in is the plan's reach (hermes #26847).
        403 => Some(format!(
            "{} — this account's Grok plan may not include it; see grok.com/?_s=usage, or \
             switch provider with /model.",
            lead(words, "xAI refused this request")
        )),
        429 => Some(format!(
            "{} — xAI is limiting this account's requests; wait a little and try again, or \
             switch provider with /model.",
            lead(words, "Too many requests")
        )),
        _ => words,
    }
}

/// [`auth_advice`] for a sign-in's refused request, dropping the cached
/// bearer minted from `stored` (the config's refresh token) when the refusal
/// was of the bearer itself ([`is_rejected_bearer`]) — so the next request
/// mints a new one instead of sending the dead one for the rest of its cached
/// life. What a turn's failure and the `/model` listing's both go through.
#[must_use]
pub fn advise(status: u16, body: &str, stored: Option<&str>) -> Option<String> {
    if is_rejected_bearer(status, body)
        && let Some(refresh) = stored.filter(|r| !r.is_empty())
    {
        reject_access(refresh);
    }
    auth_advice(status, body)
}

/// Explain a refused **refresh** — the token endpoint answering the stored
/// refresh token. A dead grant (a 400/401, or an `invalid_grant`-family code
/// on any status) is a sign-in to redo; a 403 is xAI limiting the sign-in's
/// API access to some plans, which signing in again won't change; anything
/// else keeps the server's words.
#[must_use]
pub fn refresh_advice(status: u16, body: &str) -> String {
    let code = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|value| value.get("error")?.as_str().map(str::to_ascii_lowercase))
        .unwrap_or_default();
    if matches!(
        code.as_str(),
        "invalid_grant" | "invalid_token" | "invalid_client" | "unauthorized_client"
    ) {
        return SIGN_IN_AGAIN.to_string();
    }
    match status {
        403 => TIER_DENIED.to_string(),
        400 | 401 => SIGN_IN_AGAIN.to_string(),
        _ => match envelope_message(body) {
            Some(words) => format!("xAI could not renew the sign-in: {words}"),
            None => "xAI could not renew the sign-in".to_string(),
        },
    }
}

// ---------------------------------------------------------------------------
// The token cache (boundary)
// ---------------------------------------------------------------------------

/// One minted access token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Access {
    /// The bearer the request carries — the access token itself.
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
/// bearer, and re-minting per request would spend a round trip — and a
/// single-use refresh token — each time.
fn cache() -> &'static Mutex<HashMap<String, Cached>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Cached>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Serialize cache misses through refresh, rotation and persistence. xAI's
/// refresh tokens are single-use — two threads presenting one token at once
/// strand the second with `invalid_grant` — so locking only the map lookup
/// is unsafe. Warm hits stay independent of a slow refresh.
fn refresh_gate() -> &'static Mutex<()> {
    static GATE: Mutex<()> = Mutex::new(());
    &GATE
}

/// The refresh tokens [`forget`] dropped while a refresh was out — the
/// `cline` module's rule, with one difference: a refresh clears it as it
/// starts and drains it under the cache lock before keeping what it minted,
/// so a name from its own chain there means a sign-in replaced the credential
/// *while the request was out*, and the bearer is not cached. This is what
/// lets `forget` skip the gate.
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

/// What each retired refresh token became — the in-memory half of the fix
/// [`persist_refresh`] makes on disk: the live `ModelConfig` carries the
/// token the session started with and nothing reloads it mid-run, so once
/// xAI retires that token the *next* refresh would present a dead one.
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

/// Drop any cached access token minted from `refresh_token`, and the chain
/// of rotations behind it. Signing in again is how a user recovers from a
/// grant the server has stopped honouring, so the cached bearer must not
/// outlive the sign-in that replaced it.
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

/// Drop the cached bearer minted from `refresh_token` — and only that: the
/// grant behind it is still good. What a request whose bearer xAI rejected
/// ([`is_rejected_bearer`]) calls, so the next request mints a new one
/// instead of sending the dead one until its cached life runs out.
pub fn reject_access(refresh_token: &str) {
    let current = live_refresh_token(refresh_token);
    if let Ok(mut map) = cache().lock() {
        map.remove(refresh_token);
        map.remove(&current);
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

/// The two URLs, on the issuer in force.
fn device_code_url() -> String {
    format!("{}{DEVICE_CODE_PATH}", issuer())
}

fn token_url() -> String {
    format!("{}{TOKEN_PATH}", issuer())
}

/// Read a response body, bounded.
fn read_body(resp: reqwest::blocking::Response) -> String {
    let mut body = String::new();
    let _ = resp.take(BODY_MAX_BYTES).read_to_string(&mut body);
    body
}

/// POST a form to `url`, returning the status and the bounded body.
fn post_form(url: &str, body: String) -> Result<(u16, String)> {
    let client = super::http_client(OP_TIMEOUT)?;
    let resp = client
        .post(url)
        .header("content-type", "application/x-www-form-urlencoded")
        .header("accept", "application/json")
        .body(body)
        .send()
        .map_err(|e| LlmError::transport(&e))?;
    let status = resp.status().as_u16();
    Ok((status, read_body(resp)))
}

/// Ask xAI for a device code. The caller shows [`DeviceCode::user_code`]
/// beside [`DeviceCode::verification_link`], then blocks in
/// [`await_approval`].
///
/// # Errors
/// A code request xAI refused, an answer [`DeviceCode::parse`] refuses, or a
/// transport failure.
pub fn request_device_code() -> Result<DeviceCode> {
    let (status, text) = post_form(&device_code_url(), device_code_body())?;
    if !(200..300).contains(&status) {
        let shown = match envelope_message(&text) {
            Some(words) => format!("xAI refused the device code request: {words}"),
            None => "xAI refused the device code request".to_string(),
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

/// What a finished sign-in hands the key store, and whose account it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedIn {
    /// The refresh token to store.
    pub refresh_token: String,
    /// Whose account it is, for the confirmation — its email, else its
    /// handle; `None` when the tokens name neither.
    pub account: Option<String>,
}

/// The [`SignedIn`] an approved poll's tokens make.
///
/// # Errors
/// A token set without a refresh token: without one the session would die
/// at the first access-token expiry with no way back but another sign-in.
fn signed_in(tokens: &TokenSet) -> Result<SignedIn> {
    let refresh_token = tokens.refresh_token.clone().ok_or_else(|| {
        LlmError::Decode(
            "xAI returned no refresh token, so the sign-in could not be kept — press Esc and \
             try again"
                .to_string(),
        )
    })?;
    Ok(SignedIn {
        refresh_token,
        account: account_name(tokens),
    })
}

/// The interval after a `slow_down`: the one the server named, else RFC 8628
/// §3.5's five seconds more — never faster than the current one, since the
/// server has just said it was too fast, and never slower than
/// [`MAX_INTERVAL`].
#[must_use]
pub fn slowed(interval: Duration, named: Option<u64>) -> Duration {
    named
        .map_or(
            interval + Duration::from_secs(SLOW_DOWN_STEP),
            Duration::from_secs,
        )
        .max(interval)
        .min(MAX_INTERVAL)
}

/// Poll until the user approves `device`'s code on xAI's page. Blocks for as
/// long as that takes — up to the code's own life — napping on the poll
/// cadence so Esc on the page is honoured promptly, and napping **first**:
/// an immediate poll only ever answers `authorization_pending`. Returns the
/// refresh token to store and the account it belongs to.
///
/// # Errors
/// A cancelled flow, an expired code, a denial, or a poll xAI refused.
pub fn await_approval(device: &DeviceCode, cancel: &CancelToken) -> Result<SignedIn> {
    let deadline = Instant::now() + device.lifetime();
    let mut interval = device.poll_interval();
    let body = device_poll_body(&device.device_code);
    loop {
        let wait = interval.min(deadline.saturating_duration_since(Instant::now()));
        if !nap(wait, cancel) {
            return Err(LlmError::Cancelled);
        }
        if Instant::now() >= deadline {
            return Err(LlmError::Decode(
                "the device code expired — press Esc and sign in again".to_string(),
            ));
        }
        let (status, text) = post_form(&token_url(), body.clone())?;
        match poll_verdict(status, &text) {
            Poll::Approved(tokens) => return signed_in(&tokens),
            Poll::Pending | Poll::Retry => {}
            Poll::SlowDown(named) => interval = slowed(interval, named),
            Poll::Denied(reason) => {
                return Err(LlmError::Api {
                    status,
                    body: reason,
                });
            }
        }
    }
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
/// A refresh xAI refused (worded by [`refresh_advice`]), or a transport
/// failure.
pub fn authorize(refresh_token: &str) -> Result<Access> {
    authorize_with(refresh_token, exchange_refresh)
}

/// Keep the refresh transport replaceable so the cache, the rotation and the
/// gate can be exercised deterministically without a live account.
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
    // Forgets recorded before this refresh began are done already — each
    // dropped its own entries — so only one landing while the request is out
    // may cost this refresh its bearer. Without the clear, a sign-in's own
    // `forget` of the token it then stores threw away the first bearer minted
    // from it, and the next request spent a second single-use token. Safe
    // under the gate: no other refresh is out to have its forgets cleared.
    if let Ok(mut list) = forgotten().lock() {
        list.clear();
    }
    // What to actually present: the stored token, or whatever a previous
    // rotation turned it into. The config still holds the original.
    let presented = live_refresh_token(refresh_token);
    let refresh_started = Instant::now();
    let tokens = refresh(&presented)?;
    let access = Access {
        bearer: tokens.access_token.clone(),
    };
    let lifetime = cache_lifetime(tokens.expires_in, jwt_exp(&tokens.access_token), unix_now());
    // The token presented is retired the moment a rotation names another:
    // write the new one back, and point the old ones at it.
    let cache_key = if let Some(rotated) = tokens
        .refresh_token
        .filter(|t| t.trim() != presented.trim())
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

fn exchange_refresh(presented: &str) -> Result<TokenSet> {
    let (status, text) = post_form(&token_url(), refresh_body(presented))?;
    if !(200..300).contains(&status) {
        return Err(LlmError::Api {
            status,
            body: refresh_advice(status, &text),
        });
    }
    TokenSet::parse(&text)
}

/// Write a rotated refresh token back into the `.env` key store, in place —
/// [`persist_refresh_to`] at the store the boundary named. Best-effort: a
/// store we cannot write still leaves a working session, and the failure
/// surfaces as a re-login at the next launch rather than a mid-turn error the
/// user can do nothing about.
fn persist_refresh(rotated: &str, chain: &[&str]) {
    let Some(path) = store_path().lock().ok().and_then(|p| p.clone()) else {
        return;
    };
    persist_refresh_to(&path, rotated, chain);
}

/// Write `rotated` under [`REFRESH_ENV_VAR`] in the store at `path`, but only
/// while the store still holds this session's own `chain` — the token it
/// started with and the one it actually presented — or no token at all (a
/// credential from the process environment, or a store not created yet). A
/// sign-in that landed while the refresh was in flight wrote its own token
/// there, and a rotation of the chain it replaced must not overwrite it.
/// Returns whether it wrote.
fn persist_refresh_to(path: &std::path::Path, rotated: &str, chain: &[&str]) -> bool {
    super::EnvFile::update_if(path, REFRESH_ENV_VAR, rotated, |current| {
        current.is_none_or(|held| held.is_empty() || chain.contains(&held))
    })
    .is_ok_and(|written| written.is_some())
}

/// The sentence inside an xAI refusal: an OAuth `error_description`, its
/// `{"code", "error"}` envelope's `error` string, an OpenAI-shaped
/// `error.message`, or a top-level `message` — `None` for a body that is
/// none of them.
fn envelope_message(body: &str) -> Option<String> {
    let value: Value = serde_json::from_str(body.trim()).ok()?;
    let text = |field: Option<&Value>| {
        field
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_string)
    };
    // An OAuth error (RFC 6749 §5.2) puts a machine code in `error` and the
    // sentence in `error_description`, so a description outranks it; xAI's
    // API envelope has none, and its `error` is the sentence.
    let error = value.get("error");
    text(value.get("error_description"))
        .or_else(|| text(error))
        .or_else(|| text(error.and_then(|e| e.get("message"))))
        .or_else(|| text(value.get("message")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::config::ProvidersFile;
    use serde_json::json;

    /// The test's own base64url encode, so the decoder has a known-good
    /// input rather than one built with the code under test's neighbours.
    fn base64url(bytes: &[u8]) -> String {
        const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
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

    /// An unsigned JWT carrying `claims` — nothing here checks a signature.
    fn jwt(claims: &Value) -> String {
        format!(
            "eyJhbGciOiJub25lIn0.{}.sig",
            base64url(claims.to_string().as_bytes())
        )
    }

    #[test]
    fn the_client_id_and_scopes_are_the_ones_both_references_send() {
        // pi's `xai.ts` and hermes' `auth_constants.py` agree on both. A
        // different id is a client nobody can sign in with, and without
        // `offline_access` there is no refresh token to keep.
        assert_eq!(CLIENT_ID, "b1a00492-073a-47ea-816f-4c329264a828");
        assert_eq!(
            SCOPE,
            "openid profile email offline_access grok-cli:access api:access"
        );
    }

    #[test]
    fn the_device_code_request_names_the_client_and_the_scopes_and_nothing_else() {
        assert_eq!(
            device_code_body(),
            "client_id=b1a00492-073a-47ea-816f-4c329264a828\
             &scope=openid%20profile%20email%20offline_access%20grok-cli%3Aaccess%20api%3Aaccess"
        );
    }

    #[test]
    fn the_poll_presents_the_grant_the_client_and_the_code() {
        assert_eq!(
            device_poll_body("dev-1"),
            "grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Adevice_code\
             &client_id=b1a00492-073a-47ea-816f-4c329264a828&device_code=dev-1"
        );
    }

    #[test]
    fn the_refresh_grant_presents_the_token_and_the_client() {
        assert_eq!(
            refresh_body("rt-1"),
            "grant_type=refresh_token&client_id=b1a00492-073a-47ea-816f-4c329264a828\
             &refresh_token=rt-1"
        );
    }

    #[test]
    fn a_device_code_answer_parses_and_links_the_prefilled_page() {
        // pi's fixture, with the prefilled link it prefers: one click is the
        // whole step.
        let device = DeviceCode::parse(
            r#"{"device_code":"device-code","user_code":"ABCD-1234",
                "verification_uri":"https://accounts.x.ai/oauth2/device",
                "verification_uri_complete":"https://accounts.x.ai/oauth2/device?user_code=ABCD-1234",
                "expires_in":900,"interval":5}"#,
        )
        .unwrap();
        assert_eq!(device.user_code(), "ABCD-1234");
        assert_eq!(
            device.verification_link(),
            "https://accounts.x.ai/oauth2/device?user_code=ABCD-1234"
        );
        assert_eq!(device.lifetime(), Duration::from_secs(900));
        assert_eq!(device.poll_interval(), Duration::from_secs(5));
        assert_eq!(
            device_poll_body("device-code"),
            device_poll_body(&device.device_code)
        );
    }

    #[test]
    fn without_a_prefilled_page_the_plain_one_is_linked() {
        let device = DeviceCode::parse(
            r#"{"device_code":"d","user_code":"U","verification_uri":"https://accounts.x.ai/oauth2/device",
                "expires_in":"600","interval":"3"}"#,
        )
        .unwrap();
        assert_eq!(
            device.verification_link(),
            "https://accounts.x.ai/oauth2/device"
        );
        // A number or a string holding one.
        assert_eq!(device.lifetime(), Duration::from_secs(600));
        assert_eq!(device.poll_interval(), Duration::from_secs(3));
    }

    #[test]
    fn a_code_answer_naming_no_pacing_falls_back_to_rfc_8628() {
        // RFC 8628 allows an interval of 0 (no minimum wait), which would be a
        // tight loop here: it reads as the default, as pi reads it.
        for pacing in ["", r#","expires_in":0,"interval":0"#] {
            let body = format!(
                r#"{{"device_code":"d","user_code":"U","verification_uri":"https://x.ai/device"{pacing}}}"#
            );
            let device = DeviceCode::parse(&body).unwrap();
            assert_eq!(device.lifetime(), Duration::from_secs(900), "{pacing}");
            assert_eq!(device.poll_interval(), Duration::from_secs(5), "{pacing}");
        }
    }

    #[test]
    fn a_code_answer_naming_an_untrusted_page_is_refused() {
        for link in [
            "http://accounts.x.ai/device",
            "file:///etc/passwd",
            "not a url",
            "javascript:alert(1)",
        ] {
            for field in ["verification_uri", "verification_uri_complete"] {
                let mut body = json!({"device_code":"d","user_code":"U",
                    "verification_uri":"https://accounts.x.ai/device"});
                body[field] = json!(link);
                assert!(
                    DeviceCode::parse(&body.to_string()).is_err(),
                    "{field} = {link} was accepted"
                );
            }
        }
    }

    #[test]
    fn a_code_answer_missing_a_part_is_a_decode_error() {
        for missing in ["device_code", "user_code", "verification_uri"] {
            let mut body = json!({"device_code":"d","user_code":"U",
                "verification_uri":"https://accounts.x.ai/device"});
            body.as_object_mut().unwrap().remove(missing);
            assert!(DeviceCode::parse(&body.to_string()).is_err(), "{missing}");
        }
        assert!(DeviceCode::parse("<html>blocked</html>").is_err());
    }

    #[test]
    fn a_trusted_link_is_https_or_this_machine_s_loopback() {
        assert!(is_trusted_link("https://accounts.x.ai/oauth2/device"));
        assert!(is_trusted_link("HTTPS://auth.x.ai/device?user_code=A"));
        // The smoke suite's stub.
        assert!(is_trusted_link("http://127.0.0.1:8123/device"));
        assert!(is_trusted_link("http://localhost:8123/device"));
        assert!(is_trusted_link("http://[::1]:8123/device"));
        assert!(!is_trusted_link("http://accounts.x.ai/device"));
        assert!(!is_trusted_link("http://127.0.0.1.example.com/device"));
        assert!(!is_trusted_link("ftp://accounts.x.ai/device"));
        assert!(!is_trusted_link(""));
    }

    #[test]
    fn a_pending_poll_waits_whichever_status_carries_it() {
        for status in [400, 428, 200] {
            assert_eq!(
                poll_verdict(status, r#"{"error":"authorization_pending"}"#),
                Poll::Pending
            );
        }
    }

    #[test]
    fn a_slow_down_takes_the_interval_it_names() {
        assert_eq!(
            poll_verdict(400, r#"{"error":"slow_down","interval":10}"#),
            Poll::SlowDown(Some(10))
        );
        assert_eq!(
            poll_verdict(400, r#"{"error":"slow_down"}"#),
            Poll::SlowDown(None)
        );
        assert_eq!(
            poll_verdict(400, r#"{"error":"slow_down","interval":0}"#),
            Poll::SlowDown(None),
            "a zero is no interval"
        );
    }

    #[test]
    fn an_approval_carries_the_token_set() {
        let verdict = poll_verdict(
            200,
            r#"{"access_token":"access-token","refresh_token":"refresh-token",
                "expires_in":21600,"token_type":"Bearer"}"#,
        );
        let Poll::Approved(tokens) = verdict else {
            panic!("not an approval: {verdict:?}");
        };
        assert_eq!(tokens.access_token, "access-token");
        assert_eq!(tokens.refresh_token.as_deref(), Some("refresh-token"));
        assert_eq!(tokens.expires_in, Some(21600));
    }

    #[test]
    fn the_terminal_poll_errors_end_the_flow_with_a_sentence() {
        for (error, says) in [
            ("access_denied", "denied"),
            ("authorization_denied", "denied"),
            ("expired_token", "expired"),
            ("invalid_grant", "no longer valid"),
        ] {
            let Poll::Denied(sentence) = poll_verdict(400, &json!({ "error": error }).to_string())
            else {
                panic!("{error} did not end the flow");
            };
            assert!(sentence.contains(says), "{error}: {sentence}");
            assert!(sentence.contains("press Esc"), "{error}: {sentence}");
        }
        // The server's own sentence when it wrote one.
        assert_eq!(
            poll_verdict(
                400,
                r#"{"error":"access_denied","error_description":"The user declined."}"#
            ),
            Poll::Denied("The user declined.".to_string())
        );
        // An error this flow does not know, and an answer that is no error.
        let Poll::Denied(other) = poll_verdict(400, r#"{"error":"server_error"}"#) else {
            panic!("an unknown error did not end the flow");
        };
        assert!(other.contains("server_error"), "{other}");
        let Poll::Denied(html) = poll_verdict(400, "<html>bad request</html>") else {
            panic!("a client error that is no answer did not end the flow");
        };
        assert!(html.contains("400"), "{html}");
    }

    #[test]
    fn a_transient_failure_is_polled_again() {
        // A proxy's error page, a rate limit, a timeout, a server that says it
        // is unavailable: the code is still good, so the next poll may well
        // succeed (hermes keeps polling through the same).
        for (status, body) in [
            (503, "<html>unavailable</html>"),
            (502, ""),
            (429, ""),
            (408, "timeout"),
            (503, r#"{"error":"temporarily_unavailable"}"#),
        ] {
            assert_eq!(poll_verdict(status, body), Poll::Retry, "{status} {body}");
        }
        // The RFC 8628 answers keep their meaning whatever status carries
        // them, a transient one included.
        assert_eq!(
            poll_verdict(503, r#"{"error":"authorization_pending"}"#),
            Poll::Pending
        );
        assert!(matches!(
            poll_verdict(429, r#"{"error":"expired_token"}"#),
            Poll::Denied(_)
        ));
    }

    #[test]
    fn a_slow_down_grows_the_interval_and_never_past_the_cap() {
        let five = Duration::from_secs(5);
        // RFC 8628's five seconds more, unless the server named its own.
        assert_eq!(slowed(five, None), Duration::from_secs(10));
        assert_eq!(slowed(five, Some(12)), Duration::from_secs(12));
        // A slow-down never speeds the poll up, nor slows it past the cap.
        assert_eq!(slowed(five, Some(1)), five);
        assert_eq!(slowed(five, Some(3_600)), MAX_INTERVAL);
        assert_eq!(slowed(MAX_INTERVAL, None), MAX_INTERVAL);
    }

    #[test]
    fn a_poll_interval_is_held_between_a_second_and_the_cap() {
        let device = |interval: u64| {
            DeviceCode::parse(
                &json!({"device_code": "d", "user_code": "U",
                        "verification_uri": "https://accounts.x.ai/device",
                        "interval": interval})
                .to_string(),
            )
            .unwrap()
        };
        assert_eq!(device(3_600).poll_interval(), MAX_INTERVAL);
        assert_eq!(device(7).poll_interval(), Duration::from_secs(7));
    }

    #[test]
    fn a_token_answer_needs_an_access_token() {
        assert!(TokenSet::parse(r#"{"refresh_token":"r"}"#).is_err());
        assert!(TokenSet::parse(r#"{"access_token":""}"#).is_err());
        assert!(TokenSet::parse("[]").is_err());
        // A refresh may keep the token it presented: no `refresh_token`, or
        // an empty one, is "keep the one you have".
        for refresh in ["", r#","refresh_token":"""#, r#","refresh_token":null"#] {
            let tokens = TokenSet::parse(&format!(
                r#"{{"access_token":"a","expires_in":"3600"{refresh}}}"#
            ))
            .unwrap();
            assert_eq!(tokens.refresh_token, None, "{refresh}");
            assert_eq!(tokens.expires_in, Some(3600));
        }
    }

    #[test]
    fn a_sign_in_without_a_refresh_token_is_refused() {
        let tokens = TokenSet {
            access_token: "a".to_string(),
            ..TokenSet::default()
        };
        assert!(signed_in(&tokens).is_err());
        let tokens = TokenSet {
            refresh_token: Some("r".to_string()),
            id_token: Some(jwt(&json!({"email": "dev@example.com"}))),
            ..tokens
        };
        assert_eq!(
            signed_in(&tokens).unwrap(),
            SignedIn {
                refresh_token: "r".to_string(),
                account: Some("dev@example.com".to_string()),
            }
        );
    }

    #[test]
    fn the_account_is_named_by_the_identity_token_then_the_access_token() {
        let both = TokenSet {
            access_token: jwt(&json!({"email": "access@example.com"})),
            id_token: Some(jwt(&json!({"email": "id@example.com", "sub": "u-1"}))),
            ..TokenSet::default()
        };
        assert_eq!(account_name(&both).as_deref(), Some("id@example.com"));
        let access_only = TokenSet {
            id_token: Some("not-a-jwt".to_string()),
            ..both.clone()
        };
        assert_eq!(
            account_name(&access_only).as_deref(),
            Some("access@example.com")
        );
        let neither = TokenSet {
            access_token: "opaque".to_string(),
            id_token: Some(jwt(&json!({"email": "  "}))),
            ..TokenSet::default()
        };
        assert_eq!(account_name(&neither), None);
    }

    #[test]
    fn an_account_with_no_email_is_named_by_its_handle() {
        // Tokens that name no email still name a handle, which says whose
        // account this is. An email in either token outranks a handle in
        // either.
        let handle_only = TokenSet {
            access_token: "opaque".to_string(),
            id_token: Some(jwt(&json!({"preferred_username": "grok_fan"}))),
            ..TokenSet::default()
        };
        assert_eq!(account_name(&handle_only).as_deref(), Some("grok_fan"));
        let email_in_access = TokenSet {
            access_token: jwt(&json!({"email": "dev@example.com"})),
            ..handle_only.clone()
        };
        assert_eq!(
            account_name(&email_in_access).as_deref(),
            Some("dev@example.com")
        );
    }

    #[test]
    fn the_cached_life_is_the_shorter_of_the_grant_and_the_jwt() {
        let now = 1_900_000_000;
        // The grant alone: a duration, less the skew.
        assert_eq!(
            cache_lifetime(Some(21_600), None, now),
            Duration::from_secs(21_600 - 300)
        );
        // The JWT alone: its `exp` against the clock.
        assert_eq!(
            cache_lifetime(None, Some(now + 3600), now),
            Duration::from_secs(3300)
        );
        // Both: the JWT dies first — a device-code login's short token under a
        // grant naming hours — so the JWT decides.
        assert_eq!(
            cache_lifetime(Some(21_600), Some(now + 900), now),
            Duration::from_secs(600)
        );
        // An `exp` already past is the clock running ahead: the grant stands.
        assert_eq!(
            cache_lifetime(Some(3600), Some(now - 60), now),
            Duration::from_secs(3300)
        );
        // Neither: the floor.
        assert_eq!(cache_lifetime(None, None, now), MIN_CACHE);
        assert_eq!(cache_lifetime(None, Some(now - 60), now), MIN_CACHE);
    }

    #[test]
    fn a_short_life_is_never_cached_past_half_of_it() {
        let now = 1_900_000_000;
        assert_eq!(
            cache_lifetime(Some(120), None, now),
            Duration::from_secs(60)
        );
        assert_eq!(cache_lifetime(Some(60), None, now), Duration::from_secs(30));
        assert_eq!(cache_lifetime(Some(0), None, now), Duration::ZERO);
    }

    #[test]
    fn a_jwt_s_exp_is_read_without_a_signature_check() {
        assert_eq!(
            jwt_exp(&jwt(&json!({"exp": 1_893_456_000}))),
            Some(1_893_456_000)
        );
        assert_eq!(jwt_exp("not-a-jwt"), None);
        assert_eq!(jwt_exp("eyJ4.$$$.y"), None);
        assert_eq!(jwt_claims(&jwt(&json!({"a": 1}))), Some(json!({"a": 1})));
        assert_eq!(b64url_decode("aGk"), Some(b"hi".to_vec()));
        assert_eq!(b64url_decode("aGk="), Some(b"hi".to_vec()));
    }

    #[test]
    fn xai_s_api_is_told_by_its_host() {
        assert!(is_api_base("https://api.x.ai/v1"));
        assert!(is_api_base("https://us-east-1.api.x.ai/v1/"));
        assert!(is_api_base("https://API.X.AI/v1"));
        assert!(!is_api_base("https://api.x.ai.example.com/v1"));
        assert!(!is_api_base("https://openrouter.ai/api/v1"));
        assert!(!is_api_base("https://example.com/x.ai"));
        assert!(!is_api_base("not a url"));
    }

    #[test]
    fn the_api_s_rules_follow_the_sign_in_and_the_host() {
        let mut cfg = ModelConfig::fallback();
        cfg.api_base = "http://127.0.0.1:9/v1".to_string();
        assert!(!serves(&cfg));
        // The sign-in wherever its base is pointed (the smoke suite's stub).
        cfg.auth = AuthScheme::XaiGrok;
        assert!(serves(&cfg));
        // And any provider whose base is xAI's own — a user's pasted key.
        cfg.auth = AuthScheme::ApiKey;
        cfg.api_base = "https://api.x.ai/v1".to_string();
        assert!(serves(&cfg));
    }

    #[test]
    fn the_keywords_xai_refuses_leave_the_schema_and_property_names_stay() {
        let mut schema = json!({
            "type": "object",
            "properties": {
                "url": {"type": "string", "format": "uri"},
                "pattern": {"type": "string", "description": "a regex", "pattern": "^.+$"},
                "format": {"type": "string", "enum": ["json", "text"]},
                "mime": {"type": "string", "enum": ["text/plain", "image/png"]},
                "tags": {"type": "array", "items": {"type": "string", "pattern": "^[a-z]+$"}},
                "either": {"anyOf": [{"type": "string", "format": "date"}, {"type": "null"}]},
                "nested": {"type": "object", "properties": {"when": {"type": "string", "format": "date-time"}},
                           "additionalProperties": {"type": "string", "format": "email"}},
            },
            "$defs": {"id": {"type": "string", "pattern": "^[0-9]+$"}},
            "required": ["url"],
            "additionalProperties": false,
        });
        sanitize_schema(&mut schema);
        assert_eq!(
            schema,
            json!({
                "type": "object",
                "properties": {
                    "url": {"type": "string"},
                    "pattern": {"type": "string", "description": "a regex"},
                    "format": {"type": "string", "enum": ["json", "text"]},
                    "mime": {"type": "string"},
                    "tags": {"type": "array", "items": {"type": "string"}},
                    "either": {"anyOf": [{"type": "string"}, {"type": "null"}]},
                    "nested": {"type": "object", "properties": {"when": {"type": "string"}},
                               "additionalProperties": {"type": "string"}},
                },
                "$defs": {"id": {"type": "string"}},
                "required": ["url"],
                "additionalProperties": false,
            })
        );
    }

    #[test]
    fn a_schema_with_nothing_to_refuse_is_untouched() {
        let original = json!({"type": "object", "properties": {
            "status": {"type": "string", "enum": ["pending", "in_progress", "completed"]}}});
        let mut schema = original.clone();
        sanitize_schema(&mut schema);
        assert_eq!(schema, original);
        let mut not_an_object = json!(true);
        sanitize_schema(&mut not_an_object);
        assert_eq!(not_an_object, json!(true));
    }

    #[test]
    fn a_rejected_bearer_is_a_401_or_xai_s_bad_credentials_403() {
        // hermes' fixture: an access token xAI can no longer validate is a
        // 403, not a 401.
        let stale = r#"{"code":"The caller does not have permission to execute the specified operation","error":"The OAuth2 access token could not be validated. [WKE=unauthenticated:bad-credentials]"}"#;
        assert!(is_rejected_bearer(403, stale));
        assert!(is_rejected_bearer(401, "{}"));
        assert!(!is_rejected_bearer(
            403,
            r#"{"code":"The caller does not have permission to execute the specified operation","error":"You have either run out of available resources or do not have an active Grok subscription. Manage subscriptions at https://grok.com"}"#
        ));
        assert!(!is_rejected_bearer(400, r#"{"error":"bad request"}"#));
    }

    #[test]
    fn the_advice_names_the_one_thing_to_do_about_each_refusal() {
        let permission = "The caller does not have permission to execute the specified operation";
        let stale = json!({"code": permission,
            "error": "The OAuth2 access token could not be validated. [WKE=unauthenticated:bad-credentials]"});
        assert_eq!(
            auth_advice(403, &stale.to_string()).as_deref(),
            Some(REJECTED_BEARER)
        );
        assert_eq!(auth_advice(401, "{}").as_deref(), Some(REJECTED_BEARER));
        let unsubscribed = json!({"code": permission,
            "error": "You have either run out of available resources or do not have an active Grok subscription. Manage subscriptions at https://grok.com"});
        assert_eq!(
            auth_advice(403, &unsubscribed.to_string()).as_deref(),
            Some(NO_SUBSCRIPTION)
        );
        // The same refusal can arrive as a status-less stream error.
        assert_eq!(
            auth_advice(0, "You do not have an active Grok subscription").as_deref(),
            Some(NO_SUBSCRIPTION)
        );
        let credits = json!({"code": "personal-team-blocked:spending-limit",
            "error": "You have run out of credits or need a Grok subscription. Add credits at Grok or upgrade at Grok."});
        assert_eq!(
            auth_advice(403, &credits.to_string()).as_deref(),
            Some(OUT_OF_CREDITS)
        );
    }

    #[test]
    fn any_other_403_is_the_plan_s_reach_in_xai_s_words() {
        let advice = auth_advice(
            403,
            r#"{"code":"forbidden","error":"This model is not available on your plan."}"#,
        )
        .unwrap();
        assert!(
            advice.starts_with("This model is not available on your plan"),
            "{advice}"
        );
        assert!(advice.contains("grok.com/?_s=usage"), "{advice}");
        assert!(advice.contains("/model"), "{advice}");
        // With no words to lift, the hint alone.
        let bare = auth_advice(403, "").unwrap();
        assert!(bare.contains("grok.com/?_s=usage"), "{bare}");
    }

    #[test]
    fn a_rate_limit_says_to_wait() {
        let advice = auth_advice(
            429,
            r#"{"code":"rate-limited","error":"Too many requests."}"#,
        )
        .unwrap();
        assert!(advice.starts_with("Too many requests"), "{advice}");
        assert!(advice.contains("wait"), "{advice}");
    }

    #[test]
    fn any_other_refusal_keeps_xai_s_words_without_the_envelope() {
        assert_eq!(
            auth_advice(
                400,
                r#"{"code":"invalid-argument","error":"Request contains an invalid argument: Invalid PNG image."}"#
            )
            .as_deref(),
            Some("Request contains an invalid argument: Invalid PNG image.")
        );
        assert_eq!(
            auth_advice(
                400,
                r#"{"error":{"message":"bad tool","type":"invalid_request_error"}}"#
            )
            .as_deref(),
            Some("bad tool")
        );
        // Nothing to lift: the refusal passes through as it came.
        assert_eq!(auth_advice(500, "upstream exploded"), None);
        assert_eq!(auth_advice(0, "the stream ended"), None);
    }

    #[test]
    fn a_refused_refresh_is_a_sign_in_to_redo_or_the_plan_s_limit() {
        for (status, body) in [
            (
                400,
                r#"{"error":"invalid_grant","error_description":"refresh token revoked"}"#,
            ),
            (401, r#"{"error":"invalid_client"}"#),
            (500, r#"{"error":"invalid_grant"}"#),
            (400, ""),
        ] {
            assert_eq!(
                refresh_advice(status, body),
                SIGN_IN_AGAIN,
                "{status} {body}"
            );
        }
        assert_eq!(
            refresh_advice(403, r#"{"error":"permission_denied"}"#),
            TIER_DENIED
        );
        let other = refresh_advice(
            503,
            r#"{"error":"temporarily_unavailable","error_description":"try later"}"#,
        );
        assert!(other.contains("try later"), "{other}");
    }

    #[test]
    fn the_env_var_matches_the_shipped_provider_file() {
        // The rotation write-back cannot ask the provider file, so the name is
        // pinned here — and pinned *to* the file, since a mismatch would write
        // rotations someplace key resolution never reads.
        let file = ProvidersFile::builtin();
        let grok = file.get("xai_grok").expect("shipped");
        assert_eq!(grok.key_env("xai_grok"), REFRESH_ENV_VAR);
        assert_eq!(grok.auth, AuthScheme::XaiGrok);
        assert_eq!(grok.wire_api, crate::llm::WireApi::Responses);
        assert!(
            is_api_base(&grok.kwargs.api_base),
            "the shipped base is xAI's API"
        );
    }

    #[test]
    fn a_rotation_write_back_never_overwrites_a_newer_sign_in() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".env");
        let stored = |path: &std::path::Path| {
            crate::llm::EnvFile::parse(&std::fs::read_to_string(path).unwrap())
                .get(REFRESH_ENV_VAR)
                .map(str::to_string)
        };
        std::fs::write(&path, format!("{REFRESH_ENV_VAR}=original\nOTHER=keep\n")).unwrap();
        // This session's own chain: the write lands, and the rest stays.
        assert!(persist_refresh_to(
            &path,
            "rotated",
            &["original", "original"]
        ));
        assert_eq!(stored(&path).as_deref(), Some("rotated"));
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .contains("OTHER=keep")
        );
        // The store holding the token this session presented — the chain
        // rotated once already — is still its own.
        assert!(persist_refresh_to(
            &path,
            "rotated-2",
            &["original", "rotated"]
        ));
        assert_eq!(stored(&path).as_deref(), Some("rotated-2"));
        // A sign-in landed meanwhile: the rotation of the replaced chain must
        // not take the store back to a retired token.
        std::fs::write(&path, format!("{REFRESH_ENV_VAR}=newer-sign-in\n")).unwrap();
        assert!(!persist_refresh_to(
            &path,
            "stale",
            &["original", "rotated-2"]
        ));
        assert_eq!(stored(&path).as_deref(), Some("newer-sign-in"));
        // No token stored (a credential from the environment), or no store
        // at all: the write lands, so the next launch finds one.
        std::fs::write(&path, "OTHER=keep\n").unwrap();
        assert!(persist_refresh_to(
            &path,
            "first-write",
            &["original", "original"]
        ));
        assert_eq!(stored(&path).as_deref(), Some("first-write"));
        let fresh = dir.path().join("not-made-yet").join(".env");
        assert!(persist_refresh_to(
            &fresh,
            "fresh",
            &["original", "original"]
        ));
        assert_eq!(stored(&fresh).as_deref(), Some("fresh"));
    }

    #[test]
    fn a_warm_cache_answers_without_refreshing_again() {
        let token = format!("xai-cache-test-{}", std::process::id());
        let mut calls = 0;
        let access = authorize_with(&token, |presented| {
            calls += 1;
            assert_eq!(presented, token);
            Ok(TokenSet {
                access_token: "eyJx.eyJy.s".to_string(),
                expires_in: Some(3600),
                ..TokenSet::default()
            })
        })
        .unwrap();
        assert_eq!(calls, 1);
        assert_eq!(access.bearer, "eyJx.eyJy.s", "the access token, verbatim");
        let again =
            authorize_with(&token, |_| unreachable!("a warm cache must not refresh")).unwrap();
        assert_eq!(again, access);
        forget(&token);
    }

    #[test]
    fn a_rotated_refresh_token_is_presented_next_time() {
        let stored = format!("xai-rotation-test-{}", std::process::id());
        let mut presentations = Vec::new();
        authorize_with(&stored, |presented| {
            presentations.push(presented.to_string());
            Ok(TokenSet {
                access_token: "one".to_string(),
                refresh_token: Some(format!("{stored}-rotated")),
                // Already expired: never cached, so the next resolve misses
                // and presents the rotated token this one noted.
                expires_in: Some(0),
                ..TokenSet::default()
            })
        })
        .unwrap();
        let second = authorize_with(&stored, |presented| {
            presentations.push(presented.to_string());
            Ok(TokenSet {
                access_token: "two".to_string(),
                expires_in: Some(3600),
                ..TokenSet::default()
            })
        })
        .unwrap();
        assert_eq!(second.bearer, "two");
        // The config still holds `stored`; the second refresh must present
        // what the first rotation turned it into — a single-use token is never
        // presented twice.
        assert_eq!(
            presentations,
            vec![stored.clone(), format!("{stored}-rotated")]
        );
        forget(&stored);
    }

    #[test]
    fn a_rejected_bearer_is_minted_afresh_but_the_grant_is_kept() {
        let stored = format!("xai-reject-test-{}", std::process::id());
        let first = authorize_with(&stored, |_| {
            Ok(TokenSet {
                access_token: "dead".to_string(),
                refresh_token: Some(format!("{stored}-r2")),
                expires_in: Some(3600),
                ..TokenSet::default()
            })
        })
        .unwrap();
        assert_eq!(first.bearer, "dead");
        reject_access(&stored);
        let mut presented_again = None;
        let second = authorize_with(&stored, |presented| {
            presented_again = Some(presented.to_string());
            Ok(TokenSet {
                access_token: "fresh".to_string(),
                expires_in: Some(3600),
                ..TokenSet::default()
            })
        })
        .unwrap();
        assert_eq!(second.bearer, "fresh");
        // The rotation survives the rejection: the grant was never the
        // problem, and its live token is the one presented.
        assert_eq!(presented_again, Some(format!("{stored}-r2")));
        forget(&stored);
    }

    #[test]
    fn advising_on_a_rejected_bearer_drops_it_and_any_other_refusal_keeps_it() {
        let stored = format!("xai-advise-test-{}", std::process::id());
        let mint = |bearer: &'static str| {
            move |_: &str| {
                Ok(TokenSet {
                    access_token: bearer.to_string(),
                    expires_in: Some(3600),
                    ..TokenSet::default()
                })
            }
        };
        authorize_with(&stored, mint("first")).unwrap();
        // A refusal about the plan leaves the bearer cached: it was not the
        // bearer that was refused.
        let unsubscribed = r#"{"code":"x","error":"You do not have an active Grok subscription"}"#;
        assert_eq!(
            advise(403, unsubscribed, Some(&stored)).as_deref(),
            Some(NO_SUBSCRIPTION)
        );
        assert_eq!(
            authorize_with(&stored, |_| unreachable!("still cached"))
                .unwrap()
                .bearer,
            "first"
        );
        // A rejected bearer is dropped, so the next request mints a new one.
        let stale = r#"{"code":"x","error":"The OAuth2 access token could not be validated. [WKE=unauthenticated:bad-credentials]"}"#;
        assert_eq!(
            advise(403, stale, Some(&stored)).as_deref(),
            Some(REJECTED_BEARER)
        );
        assert_eq!(
            authorize_with(&stored, mint("second")).unwrap().bearer,
            "second"
        );
        // With no stored token there is nothing to drop, and the advice is
        // the same.
        assert_eq!(advise(401, "{}", None).as_deref(), Some(REJECTED_BEARER));
        forget(&stored);
    }

    #[test]
    fn a_sign_in_s_own_forget_does_not_cost_the_first_bearer() {
        // A sign-in forgets the very token it then stores, before any refresh
        // presents it. The first refresh after it must keep its bearer:
        // throwing it away spends a second single-use refresh token on the
        // very next request.
        let stored = format!("xai-sign-in-forget-test-{}", std::process::id());
        forget(&stored);
        authorize_with(&stored, |_| {
            Ok(TokenSet {
                access_token: "first".to_string(),
                expires_in: Some(3600),
                ..TokenSet::default()
            })
        })
        .unwrap();
        let again = authorize_with(&stored, |_| unreachable!("the first bearer was kept"));
        assert_eq!(again.unwrap().bearer, "first");
        forget(&stored);
    }

    #[test]
    fn a_bearer_minted_across_a_sign_in_is_not_kept() {
        // A sign-in that lands while the refresh is out has replaced the
        // credential: the bearer minted from the old one is not cached.
        let stored = format!("xai-mid-refresh-forget-test-{}", std::process::id());
        authorize_with(&stored, |presented| {
            forget(presented);
            Ok(TokenSet {
                access_token: "stale".to_string(),
                expires_in: Some(3600),
                ..TokenSet::default()
            })
        })
        .unwrap();
        let mut refreshed = false;
        authorize_with(&stored, |_| {
            refreshed = true;
            Ok(TokenSet {
                access_token: "fresh".to_string(),
                expires_in: Some(3600),
                ..TokenSet::default()
            })
        })
        .unwrap();
        assert!(refreshed, "the bearer minted across the sign-in was cached");
        forget(&stored);
    }

    #[test]
    fn a_failed_refresh_caches_nothing_and_surfaces_its_advice() {
        let stored = format!("xai-failure-test-{}", std::process::id());
        let err = authorize_with(&stored, |_| {
            Err(LlmError::Api {
                status: 400,
                body: SIGN_IN_AGAIN.to_string(),
            })
        })
        .unwrap_err();
        assert!(err.to_string().contains("sign in again"), "{err}");
        let mut called = false;
        let _ = authorize_with(&stored, |_| {
            called = true;
            Err(LlmError::Cancelled)
        });
        assert!(called, "a failure must not leave anything cached");
        forget(&stored);
    }

    /// A refresh answer rotating to `rotated`, its bearer named after it.
    fn rotation_to(rotated: &str) -> TokenSet {
        TokenSet {
            access_token: format!("access-for-{rotated}"),
            refresh_token: Some(rotated.to_string()),
            expires_in: Some(3600),
            ..TokenSet::default()
        }
    }

    #[test]
    fn forget_never_waits_for_a_refresh_in_flight() {
        // `forget` runs on the TUI loop when a sign-in lands, while a refresh
        // on a backend thread can hold the gate for a whole request timeout.
        // The loop must not wait behind it, and the refresh that was out
        // must leave no bearer under the forgotten token: the next request
        // mints afresh.
        use std::sync::Barrier;

        let stored = "xai-forget-in-flight";
        let started = Barrier::new(2);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                authorize_with(stored, |_| {
                    started.wait();
                    std::thread::sleep(Duration::from_millis(1_500));
                    Ok(rotation_to("xai-forget-in-flight-r1"))
                })
                .unwrap();
            });
            started.wait();
            let asked = Instant::now();
            forget(stored);
            assert!(
                asked.elapsed() < Duration::from_millis(700),
                "forget waited on the refresh: {:?}",
                asked.elapsed()
            );
        });
        let mut refreshes = 0;
        authorize_with(stored, |_| {
            refreshes += 1;
            Ok(rotation_to("xai-forget-in-flight-r2"))
        })
        .unwrap();
        assert_eq!(refreshes, 1, "the forgotten token's bearer was kept");
        forget(stored);
    }

    #[test]
    fn concurrent_misses_present_the_token_once() {
        // xAI's refresh tokens are single-use: a second thread presenting the
        // one the first just retired would be refused. Thirty-two threads
        // missing at once mint one bearer and share it.
        use std::sync::Barrier;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let stored = "xai-concurrent-misses";
        let threads = 32;
        let barrier = Barrier::new(threads);
        let refreshes = AtomicUsize::new(0);
        let bearers: Vec<String> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..threads)
                .map(|_| {
                    scope.spawn(|| {
                        barrier.wait();
                        authorize_with(stored, |presented| {
                            let earlier = refreshes.fetch_add(1, Ordering::SeqCst);
                            std::thread::sleep(Duration::from_millis(25));
                            if earlier != 0 || presented != stored {
                                return Err(LlmError::Decode("refresh token reused".to_string()));
                            }
                            Ok(rotation_to("xai-concurrent-misses-r1"))
                        })
                        .map(|access| access.bearer)
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|handle| handle.join().unwrap().unwrap())
                .collect()
        });
        assert_eq!(refreshes.load(Ordering::SeqCst), 1);
        assert!(
            bearers
                .iter()
                .all(|bearer| bearer == "access-for-xai-concurrent-misses-r1"),
            "{bearers:?}"
        );
        forget(stored);
    }

    #[test]
    fn an_old_alias_finds_the_newest_bearer_after_an_expiry() {
        // A config still holding the first token, after another config's
        // refreshes rotated the chain twice, reads the newest bearer — never
        // a refresh of its own, which would present a retired token.
        let (first, second, third) = ("xai-alias-r0", "xai-alias-r1", "xai-alias-r2");
        authorize_with(first, |_| Ok(rotation_to(second))).unwrap();
        cache().lock().unwrap().get_mut(second).unwrap().until = Instant::now();
        let newest = authorize_with(second, |presented| {
            assert_eq!(presented, second);
            Ok(rotation_to(third))
        })
        .unwrap();
        let from_first =
            authorize_with(first, |_| unreachable!("an old alias refreshed again")).unwrap();
        assert_eq!(from_first, newest);
        assert_eq!(live_refresh_token(first), third);
        forget(first);
    }

    #[test]
    fn forget_drops_every_token_of_the_chain() {
        let (first, second, third) = ("xai-chain-r0", "xai-chain-r1", "xai-chain-r2");
        authorize_with(first, |_| Ok(rotation_to(second))).unwrap();
        cache().lock().unwrap().get_mut(second).unwrap().until = Instant::now();
        authorize_with(second, |_| Ok(rotation_to(third))).unwrap();
        forget(first);
        for token in [first, second, third] {
            assert_eq!(live_refresh_token(token), token, "{token} still rotates");
            assert!(
                !cache().lock().unwrap().contains_key(token),
                "{token} is still cached"
            );
        }
    }

    #[test]
    fn a_refresh_that_failed_is_retried_with_the_same_token() {
        let stored = "xai-retried";
        assert!(
            authorize_with(stored, |_| Err(LlmError::Http(
                "temporary failure".to_string()
            )))
            .is_err()
        );
        let retried = authorize_with(stored, |presented| {
            assert_eq!(presented, stored, "a failure rotated nothing");
            Ok(rotation_to("xai-retried-r1"))
        })
        .unwrap();
        let hit = authorize_with(stored, |_| unreachable!("the retry's bearer is cached")).unwrap();
        assert_eq!(hit, retried);
        forget(stored);
    }

    #[test]
    fn a_chain_of_rotations_keeps_one_bearer() {
        // Every refresh retires the token it presented; the cache must not
        // keep a bearer under each retired one for the session's life.
        let stored = "xai-bounded-r0";
        for index in 1..=64 {
            let current = live_refresh_token(stored);
            if let Some(entry) = cache().lock().unwrap().get_mut(&current) {
                entry.until = Instant::now();
            }
            authorize_with(stored, |_| {
                Ok(rotation_to(&format!("xai-bounded-r{index}")))
            })
            .unwrap();
        }
        let kept = cache()
            .lock()
            .unwrap()
            .keys()
            .filter(|token| token.starts_with("xai-bounded-"))
            .count();
        assert_eq!(kept, 1);
        forget(stored);
    }

    #[test]
    fn a_rotation_from_a_reloaded_config_repoints_the_older_ones() {
        // A config rebuilt mid-session holds a newer token than the first
        // one; its refresh must move the first config along too.
        let (first, second, third) = ("xai-reloaded-r0", "xai-reloaded-r1", "xai-reloaded-r2");
        note_rotation(first, first, second);
        note_rotation(second, second, third);
        assert_eq!(live_refresh_token(first), third);
        assert_eq!(live_refresh_token(second), third);
        forget(first);
    }
}
