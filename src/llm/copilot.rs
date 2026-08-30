//! GitHub Copilot as a provider: the device-code sign-in, the token exchange
//! every request needs, and the request identity Copilot's API insists on.
//! See `docs/copilot.md`.
//!
//! Two token layers, and conflating them is the classic bug:
//!
//! 1. The **GitHub OAuth token** (`ghu_…`) the device flow mints. Long-lived,
//!    persisted in the `.env` key store like any other provider's secret.
//! 2. The **Copilot bearer** — an HMAC-signed `tid=…:mac` blob the API
//!    actually takes, good for ~30 minutes. Exchanged from (1) on demand,
//!    cached in memory, never written to disk.
//!
//! The pure halves (the response parses, the poll's verdict, the freshness
//! rule, the request identity) are unit-tested; the three HTTP calls are
//! boundary code.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use serde::Deserialize;

use super::config::{AuthScheme, ModelConfig};
use super::{ChatMessage, LlmError, MessageContent, Result};
use crate::stream::CancelToken;

/// The OAuth client id every unaffiliated Copilot client uses — the **legacy
/// OAuth app of the VS Code Copilot extension**, and a public identifier (the
/// device flow has no client secret). It is not interchangeable: a modern
/// GitHub App id, or the `gh` CLI's, mints a token the exchange endpoint
/// answers `404` to however good the user's Copilot subscription is.
pub const CLIENT_ID: &str = "Iv1.b507a08c87ecfe98";

/// The scope requested. There is no `copilot` scope on this app — the one that
/// takes it belongs to GitHub *Models*, a different product.
const SCOPE: &str = "read:user";

/// RFC 8628's device-code grant type.
const GRANT_TYPE: &str = "urn:ietf:params:oauth:grant-type:device_code";

/// Where the flow's three calls go.
const DEVICE_CODE_URL: &str = "https://github.com/login/device/code";
const ACCESS_TOKEN_URL: &str = "https://github.com/login/oauth/access_token";
const EXCHANGE_URL: &str = "https://api.github.com/copilot_internal/v2/token";

/// The individual-seat API host, and the fallback when the exchange names none.
pub const DEFAULT_API_BASE: &str = "https://api.githubcopilot.com";

/// The editor identity every Copilot request carries. These are **not**
/// cosmetic: the API answers `400 missing Editor-Version header for IDE auth`
/// without the first, and `403 token not authorized for this integration`
/// without a `Copilot-Integration-Id` it knows. `providers.toml` sends the
/// same pair on the chat request; these are for the two calls that go to
/// `github.com`/`api.github.com` instead, which the provider's headers don't
/// reach.
const EDITOR_VERSION: &str = "vscode/1.104.3";
const EDITOR_PLUGIN_VERSION: &str = "copilot-chat/0.26.7";
const USER_AGENT: &str = "GitHubCopilotChat/0.26.7";

/// The poll cadence GitHub falls back to when the device-code response names
/// none, and the penalty a `slow_down` adds to it (both RFC 8628's).
const DEFAULT_POLL_INTERVAL: u64 = 5;
const SLOW_DOWN_PENALTY: u64 = 5;

/// Per-operation deadline for the flow's requests — short, because each is a
/// small JSON round trip and the user is watching a countdown.
const OP_TIMEOUT: Duration = Duration::from_secs(30);

/// The most any of the flow's three responses may buffer — the
/// `MODELS_BODY_MAX_BYTES` posture at a size these bodies actually are (a
/// device code is a few hundred bytes, a token envelope a couple of KB). It
/// also bounds what a *failure* can put on the device page: an intercepting
/// proxy answers a blocked request with an HTML page, and that page would
/// otherwise become the "reason" the user reads.
const BODY_MAX_BYTES: u64 = 64 * 1024;

/// Read a response body, bounded, and trim it to something showable. Boundary
/// code — the read is I/O, the trim is the pure half of the same decision.
fn read_body(resp: reqwest::blocking::Response) -> String {
    use std::io::Read;
    let mut body = String::new();
    let _ = resp.take(BODY_MAX_BYTES).read_to_string(&mut body);
    body
}

/// Re-exchange a Copilot bearer this long before its own deadline. The
/// exchange publishes a `refresh_in` hint (~25 min) well inside the token's
/// ~30 min life; a minute of slack on top absorbs clock skew and the latency
/// of a request already in flight.
const REFRESH_SKEW: Duration = Duration::from_secs(60);

// ---------------------------------------------------------------------------
// The device flow's wire shapes (pure)
// ---------------------------------------------------------------------------

/// `POST /login/device/code`'s answer: the code to show, the code to poll
/// with, and the pacing.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct DeviceCode {
    /// The opaque code the poll presents (never shown to the user).
    pub device_code: String,
    /// The short code the user types at [`Self::verification_uri`].
    pub user_code: String,
    /// Where the user enters it (`https://github.com/login/device`).
    pub verification_uri: String,
    /// How long the pair stays valid, in seconds.
    #[serde(default = "default_expires_in")]
    pub expires_in: u64,
    /// The minimum seconds between polls. Absent on some responses.
    #[serde(default = "default_interval")]
    pub interval: u64,
}

fn default_expires_in() -> u64 {
    900
}

fn default_interval() -> u64 {
    DEFAULT_POLL_INTERVAL
}

impl DeviceCode {
    /// Parse the device-code response.
    ///
    /// # Errors
    /// A body that isn't the expected object becomes a decode error.
    pub fn parse(body: &str) -> Result<Self> {
        let parsed: Self =
            serde_json::from_str(body).map_err(|e| LlmError::Decode(e.to_string()))?;
        if parsed.device_code.is_empty() || parsed.user_code.is_empty() {
            return Err(LlmError::Decode(
                "GitHub returned no device code".to_string(),
            ));
        }
        Ok(parsed)
    }

    /// How long the code has left, given how long the flow has been running.
    #[must_use]
    pub fn remaining(&self, elapsed: Duration) -> Duration {
        Duration::from_secs(self.expires_in).saturating_sub(elapsed)
    }
}

/// What one poll of `/login/oauth/access_token` established. GitHub answers
/// **HTTP 200 for every one of these** — the verdict is in the body's `error`
/// field, so a status check alone would read a denial as a success.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PollVerdict {
    /// The user approved: here is the long-lived GitHub OAuth token.
    Approved(String),
    /// Nothing yet — poll again at the same cadence.
    Pending,
    /// Polling too fast; wait this much longer between polls from now on.
    SlowDown(Duration),
    /// Terminal, with the message to show the user.
    Failed(String),
}

/// The poll response's fields (all optional — which one is set *is* the
/// verdict).
#[derive(Debug, Default, Deserialize)]
struct PollResponse {
    access_token: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
    /// GitHub sends the new floor alongside a `slow_down`.
    interval: Option<u64>,
}

/// Classify one poll response against the current interval. Pure — the
/// boundary just sleeps and re-calls.
#[must_use]
pub fn poll_verdict(body: &str, interval: Duration) -> PollVerdict {
    let parsed: PollResponse = serde_json::from_str(body).unwrap_or_default();
    if let Some(token) = parsed.access_token.filter(|t| !t.is_empty()) {
        return PollVerdict::Approved(token);
    }
    let described = |fallback: &str| {
        parsed
            .error_description
            .clone()
            .filter(|d| !d.trim().is_empty())
            .unwrap_or_else(|| fallback.to_string())
    };
    match parsed.error.as_deref() {
        Some("authorization_pending") => PollVerdict::Pending,
        Some("slow_down") => PollVerdict::SlowDown(
            parsed
                .interval
                .map_or(interval + Duration::from_secs(SLOW_DOWN_PENALTY), |secs| {
                    Duration::from_secs(secs)
                }),
        ),
        // GitHub's own docs disagree with themselves on the spelling — the
        // table says `expired_token`, the prose `token_expired` — so match
        // both rather than letting one read as an unknown failure.
        Some("expired_token" | "token_expired") => {
            PollVerdict::Failed("The code expired — press Esc and sign in again.".to_string())
        }
        Some("access_denied") => {
            PollVerdict::Failed("Sign-in was cancelled on GitHub.".to_string())
        }
        Some(other) => PollVerdict::Failed(described(other)),
        None => PollVerdict::Failed("GitHub sent an unexpected response.".to_string()),
    }
}

// ---------------------------------------------------------------------------
// The token exchange (pure parse)
// ---------------------------------------------------------------------------

/// `GET /copilot_internal/v2/token`'s answer, narrowed to what a chat client
/// needs. The envelope carries two dozen feature flags besides; reading only
/// these four keeps a schema that changes monthly from breaking the parse.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ExchangedToken {
    /// The bearer every Copilot API request sends.
    pub token: String,
    /// Unix seconds. Reported but **not** trusted on its own — see
    /// [`ExchangedToken::lifetime`].
    #[serde(default)]
    pub expires_at: i64,
    /// Seconds until the client should exchange again (~1500).
    #[serde(default)]
    pub refresh_in: u64,
    /// The account's own endpoints. A Business/Enterprise seat is served from
    /// its own host, so this — not a hardcoded default — is the base to use.
    #[serde(default)]
    pub endpoints: Option<Endpoints>,
}

/// The `endpoints` object, narrowed to the chat API's base.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Endpoints {
    #[serde(default)]
    pub api: Option<String>,
}

impl ExchangedToken {
    /// Parse the exchange response.
    ///
    /// # Errors
    /// A body that isn't the expected object — or that carries no token —
    /// becomes a decode error.
    pub fn parse(body: &str) -> Result<Self> {
        let parsed: Self =
            serde_json::from_str(body).map_err(|e| LlmError::Decode(e.to_string()))?;
        if parsed.token.is_empty() {
            return Err(LlmError::Decode(
                "GitHub returned no Copilot token".to_string(),
            ));
        }
        Ok(parsed)
    }

    /// How long this token may be used for before re-exchanging.
    ///
    /// Deliberately derived from `refresh_in` rather than `expires_at`: a user
    /// whose clock runs ahead gets an `expires_at` already in the past, which
    /// would re-exchange on every single request. `refresh_in` is a *duration*
    /// and immune to that. A response that names neither falls back to a
    /// conservative five minutes — short enough to be safe, long enough that a
    /// turn's rounds share one exchange.
    #[must_use]
    pub fn lifetime(&self) -> Duration {
        if self.refresh_in == 0 {
            return Duration::from_secs(300);
        }
        Duration::from_secs(self.refresh_in).saturating_sub(REFRESH_SKEW)
    }

    /// The account's chat API base, trailing slash trimmed — its own host for
    /// a Business/Enterprise seat, else the individual default.
    #[must_use]
    pub fn api_base(&self) -> String {
        self.endpoints
            .as_ref()
            .and_then(|e| e.api.as_deref())
            .map(str::trim)
            .filter(|base| !base.is_empty())
            .unwrap_or(DEFAULT_API_BASE)
            .trim_end_matches('/')
            .to_string()
    }
}

// ---------------------------------------------------------------------------
// Request identity (pure)
// ---------------------------------------------------------------------------

/// The `X-Initiator` a round carries — and it is **billing-relevant**, not
/// cosmetic: GitHub charges a premium request for a user-initiated turn and
/// nothing for the agent's own tool round trips, so marking every round `user`
/// burns the user's quota several times over per turn.
///
/// The rule is the last message's role: a round the user's own text closes is
/// theirs; a round closing on a tool result (or the assistant's own text, as a
/// continuation does) is the agent's.
#[must_use]
pub fn initiator(messages: &[ChatMessage]) -> &'static str {
    match messages.last().map(|m| m.role.as_str()) {
        Some("user") => "user",
        _ => "agent",
    }
}

/// Does this round carry an image? Copilot rejects image parts unless the
/// request also declares `Copilot-Vision-Request`.
#[must_use]
pub fn has_image(messages: &[ChatMessage]) -> bool {
    messages.iter().any(|m| match &m.content {
        MessageContent::Text(_) => false,
        MessageContent::Parts(parts) => parts
            .iter()
            .any(|p| matches!(p, super::ContentPart::ImageUrl { .. })),
    })
}

/// Form-encode one `key=value` pair's value (`application/x-www-form-urlencoded`).
/// RFC 8628 specifies form bodies for the device flow, and GitHub's endpoints
/// are strict about the encoding of `grant_type`'s colons if nothing else.
#[must_use]
fn form_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// A form body from `key=value` pairs.
#[must_use]
fn form_body(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| format!("{}={}", form_encode(k), form_encode(v)))
        .collect::<Vec<_>>()
        .join("&")
}

// ---------------------------------------------------------------------------
// The boundary: three HTTP calls and the bearer cache
// ---------------------------------------------------------------------------

/// One exchanged bearer, with the instant it stops being usable.
#[derive(Debug, Clone)]
struct CachedBearer {
    bearer: String,
    api_base: String,
    good_until: std::time::Instant,
}

/// The exchanged bearers, keyed by the OAuth token that minted them. A process
/// global for the same reason [`super::http_client`]'s client cache is one: a
/// turn's rounds, the `/model` fetch and every subagent's thread all need the
/// same bearer, and re-exchanging per request would spend a network round trip
/// on each while GitHub rate-limits the endpoint.
static BEARERS: OnceLock<Mutex<HashMap<String, CachedBearer>>> = OnceLock::new();

/// What a request to a Copilot-backed config actually sends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopilotAuth {
    /// The `Authorization: Bearer` value.
    pub bearer: String,
    /// The account's own API base, which outranks the configured one.
    pub api_base: String,
}

/// The live Copilot credentials for a stored OAuth token: cached while fresh,
/// exchanged when not. Boundary code — a cache miss is one HTTPS GET.
///
/// # Errors
/// A refused or unparseable exchange becomes an [`LlmError`].
pub fn authorize(oauth_token: &str) -> Result<CopilotAuth> {
    let cache = BEARERS.get_or_init(|| Mutex::new(HashMap::new()));
    let now = std::time::Instant::now();
    if let Ok(map) = cache.lock()
        && let Some(hit) = map.get(oauth_token)
        && hit.good_until > now
    {
        return Ok(CopilotAuth {
            bearer: hit.bearer.clone(),
            api_base: hit.api_base.clone(),
        });
    }
    let exchanged = exchange(oauth_token)?;
    let auth = CopilotAuth {
        bearer: exchanged.token.clone(),
        api_base: exchanged.api_base(),
    };
    if let Ok(mut map) = cache.lock() {
        map.insert(
            oauth_token.to_string(),
            CachedBearer {
                bearer: auth.bearer.clone(),
                api_base: auth.api_base.clone(),
                good_until: now + exchanged.lifetime(),
            },
        );
    }
    Ok(auth)
}

/// Forget a stored token's cached bearer — what a fresh sign-in owes, so the
/// next request exchanges rather than reusing the previous account's.
pub fn forget(oauth_token: &str) {
    if let Some(cache) = BEARERS.get()
        && let Ok(mut map) = cache.lock()
    {
        map.remove(oauth_token);
    }
}

/// Exchange a GitHub OAuth token for a Copilot bearer. Boundary code.
///
/// # Errors
/// Transport failures and non-2xx responses become an [`LlmError`]. A `404`
/// here is reported specially: it means the *token* was minted by the wrong
/// OAuth client, not that the user lacks a subscription — the single most
/// misdiagnosed failure in this flow.
fn exchange(oauth_token: &str) -> Result<ExchangedToken> {
    let client = super::http_client(OP_TIMEOUT)?;
    let resp = client
        .get(EXCHANGE_URL)
        // `token`, not `Bearer` — Microsoft's own client's scheme, and the one
        // every third-party implementation converged on.
        .header("authorization", format!("token {oauth_token}"))
        .header("accept", "application/json")
        .header("editor-version", EDITOR_VERSION)
        .header("editor-plugin-version", EDITOR_PLUGIN_VERSION)
        .header("user-agent", USER_AGENT)
        .send()
        .map_err(|e| LlmError::Http(e.to_string()))?;
    let status = resp.status().as_u16();
    let body = read_body(resp);
    if status == 404 {
        return Err(LlmError::Api {
            status,
            body: "GitHub refused this token for Copilot. Run /login and sign in again."
                .to_string(),
        });
    }
    if !(200..300).contains(&status) {
        return Err(LlmError::Api { status, body });
    }
    ExchangedToken::parse(&body)
}

/// Start the device flow: ask GitHub for a code pair. Boundary code.
///
/// # Errors
/// Transport failures, non-2xx responses and unparseable bodies become an
/// [`LlmError`].
pub fn request_device_code() -> Result<DeviceCode> {
    let client = super::http_client(OP_TIMEOUT)?;
    let resp = client
        .post(DEVICE_CODE_URL)
        .header("accept", "application/json")
        .header("content-type", "application/x-www-form-urlencoded")
        .header("user-agent", USER_AGENT)
        .body(form_body(&[("client_id", CLIENT_ID), ("scope", SCOPE)]))
        .send()
        .map_err(|e| LlmError::Http(e.to_string()))?;
    let status = resp.status().as_u16();
    let body = read_body(resp);
    if !(200..300).contains(&status) {
        return Err(LlmError::Api { status, body });
    }
    DeviceCode::parse(&body)
}

/// Poll until the user approves the code, the flow fails, or `cancel` trips.
/// The wait between polls is the interval GitHub asked for — slept in short
/// naps so an Esc is honoured within `POLL_NAP` rather than at the end of a
/// five-second block. Boundary code.
///
/// # Errors
/// The `Err` is the **sentence to show the user**, not a diagnostic: every way
/// this can fail — a denial, an expiry, a dead connection — is something a
/// person reads off the device page and acts on, and an `LlmError`'s
/// `request failed:` prefix in front of "The code expired" is noise. A cancel
/// yields an empty reason, which the worker never delivers.
pub fn poll_for_token(
    device: &DeviceCode,
    cancel: &CancelToken,
) -> std::result::Result<String, String> {
    let client = super::http_client(OP_TIMEOUT).map_err(|e| e.to_string())?;
    let deadline = std::time::Instant::now() + Duration::from_secs(device.expires_in);
    let mut interval = Duration::from_secs(device.interval.max(1));
    let body = form_body(&[
        ("client_id", CLIENT_ID),
        ("device_code", &device.device_code),
        ("grant_type", GRANT_TYPE),
    ]);
    loop {
        // Sleep *first*: the user has not even read the code yet, and GitHub
        // answers `slow_down` to a poll that beats the interval.
        if !nap(interval, cancel) {
            return Err(String::new());
        }
        if std::time::Instant::now() >= deadline {
            return Err("The code expired — press Esc and sign in again.".to_string());
        }
        let resp = client
            .post(ACCESS_TOKEN_URL)
            .header("accept", "application/json")
            .header("content-type", "application/x-www-form-urlencoded")
            .header("user-agent", USER_AGENT)
            .body(body.clone())
            .send()
            .map_err(|e| format!("Could not reach GitHub: {e}"))?;
        let text = read_body(resp);
        match poll_verdict(&text, interval) {
            PollVerdict::Approved(token) => return Ok(token),
            PollVerdict::Pending => {}
            PollVerdict::SlowDown(next) => interval = next,
            PollVerdict::Failed(reason) => return Err(reason),
        }
    }
}

/// How long one nap between cancel checks lasts — the transport's own cadence,
/// so an Esc on the device page is honoured about as fast as one mid-turn.
const POLL_NAP: Duration = Duration::from_millis(200);

/// Sleep `total` in [`POLL_NAP`] slices, returning `false` the moment `cancel`
/// trips.
fn nap(total: Duration, cancel: &CancelToken) -> bool {
    let until = std::time::Instant::now() + total;
    while std::time::Instant::now() < until {
        if cancel.is_cancelled() {
            return false;
        }
        std::thread::sleep(
            POLL_NAP.min(until.saturating_duration_since(std::time::Instant::now())),
        );
    }
    !cancel.is_cancelled()
}

/// The `Authorization` bearer and base URL a request against `cfg` uses. For
/// every ordinary provider that is the stored key and the configured base,
/// resolved with no I/O at all; for GitHub Copilot the stored value is an
/// OAuth token, exchanged here (cached) for the bearer the API takes and the
/// account's own host.
///
/// # Errors
/// A Copilot exchange that fails becomes an [`LlmError`]; every other scheme
/// is infallible.
pub(crate) fn request_auth(cfg: &ModelConfig) -> Result<(Option<String>, Option<String>)> {
    match cfg.auth {
        AuthScheme::ApiKey => Ok((cfg.api_key.clone(), None)),
        AuthScheme::GithubCopilot => {
            let Some(oauth) = cfg.api_key.as_deref().filter(|k| !k.is_empty()) else {
                return Ok((None, None));
            };
            let auth = authorize(oauth)?;
            Ok((Some(auth.bearer), Some(auth.api_base)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- the device-code response ---

    #[test]
    fn a_device_code_response_parses() {
        let body = r#"{"device_code":"abc123","user_code":"C363-262E",
            "verification_uri":"https://github.com/login/device",
            "expires_in":899,"interval":5}"#;
        let code = DeviceCode::parse(body).unwrap();
        assert_eq!(code.user_code, "C363-262E");
        assert_eq!(code.device_code, "abc123");
        assert_eq!(code.verification_uri, "https://github.com/login/device");
        assert_eq!(code.expires_in, 899);
        assert_eq!(code.interval, 5);
    }

    #[test]
    fn a_device_code_response_without_an_interval_takes_the_default() {
        // GitHub omits `interval` on some responses; polling with no floor
        // earns an immediate `slow_down`.
        let body = r#"{"device_code":"a","user_code":"B","verification_uri":"u"}"#;
        let code = DeviceCode::parse(body).unwrap();
        assert_eq!(code.interval, DEFAULT_POLL_INTERVAL);
        assert_eq!(code.expires_in, 900);
    }

    #[test]
    fn a_codeless_device_response_is_an_error_not_an_empty_page() {
        assert!(DeviceCode::parse(r#"{"device_code":"","user_code":""}"#).is_err());
        assert!(DeviceCode::parse("not json").is_err());
    }

    #[test]
    fn remaining_counts_the_code_down_and_floors_at_zero() {
        let code = DeviceCode::parse(
            r#"{"device_code":"a","user_code":"b","verification_uri":"u","expires_in":900}"#,
        )
        .unwrap();
        assert_eq!(
            code.remaining(Duration::from_secs(49)),
            Duration::from_secs(851)
        );
        assert_eq!(code.remaining(Duration::from_secs(9000)), Duration::ZERO);
    }

    // --- the poll's verdict ---

    #[test]
    fn an_approved_poll_yields_the_oauth_token() {
        let body = r#"{"access_token":"ghu_secret","token_type":"bearer","scope":"read:user"}"#;
        assert_eq!(
            poll_verdict(body, Duration::from_secs(5)),
            PollVerdict::Approved("ghu_secret".to_string())
        );
    }

    #[test]
    fn a_pending_poll_keeps_waiting() {
        let body = r#"{"error":"authorization_pending","error_description":"…"}"#;
        assert_eq!(
            poll_verdict(body, Duration::from_secs(5)),
            PollVerdict::Pending
        );
    }

    #[test]
    fn slow_down_lengthens_the_interval() {
        // GitHub's own new floor wins when it sends one; otherwise the RFC's
        // five-second penalty applies to the interval in force.
        assert_eq!(
            poll_verdict(
                r#"{"error":"slow_down","interval":10}"#,
                Duration::from_secs(5)
            ),
            PollVerdict::SlowDown(Duration::from_secs(10))
        );
        assert_eq!(
            poll_verdict(r#"{"error":"slow_down"}"#, Duration::from_secs(5)),
            PollVerdict::SlowDown(Duration::from_secs(10))
        );
    }

    #[test]
    fn both_spellings_of_an_expired_code_are_terminal() {
        // GitHub's docs contradict themselves on this one; matching only the
        // table's spelling turned a routine expiry into "unexpected response".
        for body in [
            r#"{"error":"expired_token"}"#,
            r#"{"error":"token_expired"}"#,
        ] {
            let PollVerdict::Failed(reason) = poll_verdict(body, Duration::from_secs(5)) else {
                panic!("expected a terminal verdict for {body}");
            };
            assert!(reason.contains("expired"), "{reason}");
        }
    }

    #[test]
    fn a_cancelled_sign_in_says_so() {
        let PollVerdict::Failed(reason) =
            poll_verdict(r#"{"error":"access_denied"}"#, Duration::from_secs(5))
        else {
            panic!("expected a terminal verdict");
        };
        assert!(reason.contains("cancelled"), "{reason}");
    }

    #[test]
    fn an_unknown_error_surfaces_githubs_own_description() {
        let body = r#"{"error":"device_flow_disabled","error_description":"Device flow is off"}"#;
        assert_eq!(
            poll_verdict(body, Duration::from_secs(5)),
            PollVerdict::Failed("Device flow is off".to_string())
        );
        // With no description the code itself is the message.
        assert_eq!(
            poll_verdict(
                r#"{"error":"device_flow_disabled"}"#,
                Duration::from_secs(5)
            ),
            PollVerdict::Failed("device_flow_disabled".to_string())
        );
    }

    #[test]
    fn a_bodyless_poll_is_a_failure_not_a_silent_approval() {
        // Every poll answers HTTP 200, so a body we can't read must never be
        // mistaken for "keep waiting" — that loops until the code expires.
        let PollVerdict::Failed(_) = poll_verdict("", Duration::from_secs(5)) else {
            panic!("an unreadable body must be terminal");
        };
    }

    // --- the exchange ---

    #[test]
    fn an_exchange_response_parses_its_four_useful_fields() {
        let body = r#"{"token":"tid=abc;exp=123:mac","expires_at":1750000000,
            "refresh_in":1500,"sku":"monthly_subscriber_quota","chat_enabled":true,
            "endpoints":{"api":"https://api.business.githubcopilot.com","proxy":"…"}}"#;
        let token = ExchangedToken::parse(body).unwrap();
        assert_eq!(token.token, "tid=abc;exp=123:mac");
        assert_eq!(token.refresh_in, 1500);
        assert_eq!(token.api_base(), "https://api.business.githubcopilot.com");
    }

    #[test]
    fn an_exchange_without_endpoints_falls_back_to_the_individual_host() {
        let token = ExchangedToken::parse(r#"{"token":"t","refresh_in":1500}"#).unwrap();
        assert_eq!(token.api_base(), DEFAULT_API_BASE);
        // …as does one whose endpoints object names no api, or an empty one.
        let blank = ExchangedToken::parse(r#"{"token":"t","endpoints":{"api":"  "}}"#).unwrap();
        assert_eq!(blank.api_base(), DEFAULT_API_BASE);
    }

    #[test]
    fn an_endpoint_keeps_no_trailing_slash() {
        // `{base}/chat/completions` would otherwise double the separator.
        let token =
            ExchangedToken::parse(r#"{"token":"t","endpoints":{"api":"https://x.example/"}}"#)
                .unwrap();
        assert_eq!(token.api_base(), "https://x.example");
    }

    #[test]
    fn a_tokenless_exchange_is_an_error() {
        assert!(ExchangedToken::parse(r#"{"expires_at":1}"#).is_err());
        assert!(ExchangedToken::parse("{").is_err());
    }

    #[test]
    fn the_lifetime_comes_from_refresh_in_not_the_wall_clock() {
        // A user whose clock runs ahead gets an `expires_at` already in the
        // past; keying off it would re-exchange on every single request.
        let token =
            ExchangedToken::parse(r#"{"token":"t","expires_at":1,"refresh_in":1500}"#).unwrap();
        assert_eq!(token.lifetime(), Duration::from_secs(1500 - 60));
    }

    #[test]
    fn a_refresh_hint_shorter_than_the_skew_still_yields_a_usable_lifetime() {
        let token = ExchangedToken::parse(r#"{"token":"t","refresh_in":30}"#).unwrap();
        assert_eq!(token.lifetime(), Duration::ZERO, "always re-exchange");
        // No hint at all is the conservative five minutes, not zero — else
        // every request in a turn would pay an exchange.
        let none = ExchangedToken::parse(r#"{"token":"t"}"#).unwrap();
        assert_eq!(none.lifetime(), Duration::from_secs(300));
    }

    // --- request identity ---

    #[test]
    fn a_round_the_user_closed_is_user_initiated() {
        let messages = vec![ChatMessage::system("s"), ChatMessage::user("hello")];
        assert_eq!(initiator(&messages), "user");
    }

    #[test]
    fn a_round_closing_on_a_tool_result_is_agent_initiated() {
        // This is what keeps a turn's tool loop off the user's premium-request
        // quota: only the round they actually typed is billed.
        let mut tool = ChatMessage::user("{\"ok\":true}");
        tool.role = "tool".to_string();
        assert_eq!(initiator(&[ChatMessage::user("hi"), tool]), "agent");
        // …as is a continuation the assistant's own text closes.
        assert_eq!(
            initiator(&[ChatMessage::assistant("half an answer")]),
            "agent"
        );
        assert_eq!(initiator(&[]), "agent", "no messages is not a user turn");
    }

    #[test]
    fn an_image_part_is_detected_anywhere_in_the_round() {
        let plain = vec![ChatMessage::user("no pictures here")];
        assert!(!has_image(&plain));
        let withimage = vec![
            ChatMessage::user("look"),
            ChatMessage::with_parts(
                "user",
                vec![
                    super::super::ContentPart::text("what is this?"),
                    super::super::ContentPart::image("data:image/png;base64,AAAA"),
                ],
            ),
        ];
        assert!(has_image(&withimage));
    }

    // --- form encoding ---

    #[test]
    fn the_grant_type_survives_form_encoding() {
        // Its colons must be escaped or GitHub reads a truncated grant type.
        let body = form_body(&[("grant_type", GRANT_TYPE), ("client_id", CLIENT_ID)]);
        assert_eq!(
            body,
            "grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Adevice_code\
             &client_id=Iv1.b507a08c87ecfe98"
        );
    }

    #[test]
    fn a_scope_with_a_colon_encodes_too() {
        assert_eq!(form_encode(SCOPE), "read%3Auser");
        assert_eq!(form_encode("a b"), "a+b");
    }

    // --- the request-auth seam ---

    #[test]
    fn an_ordinary_provider_authenticates_with_no_network_at_all() {
        let mut cfg = ModelConfig::fallback();
        cfg.api_key = Some("sk-test".to_string());
        let (bearer, base) = request_auth(&cfg).unwrap();
        assert_eq!(bearer.as_deref(), Some("sk-test"));
        assert_eq!(base, None, "the configured base stands");
    }

    #[test]
    fn a_copilot_config_with_no_stored_token_resolves_to_nothing_rather_than_calling_out() {
        // The `/model` picker builds configs for providers that were never
        // signed in to; an exchange attempt there would hang the fetch on a
        // request that can only 401.
        let mut cfg = ModelConfig::fallback();
        cfg.auth = AuthScheme::GithubCopilot;
        cfg.api_key = None;
        assert_eq!(request_auth(&cfg).unwrap(), (None, None));
        cfg.api_key = Some(String::new());
        assert_eq!(request_auth(&cfg).unwrap(), (None, None));
    }
}
