//! The Anthropic **Console sign-in**: Anthropic's OAuth PKCE flow, the
//! refresh-token store it fills, and the short-lived access token every
//! request is minted with. See `docs/claude.md`.
//!
//! The third sign-in, after GitHub Copilot's device flow (`docs/copilot.md`)
//! and OpenAI's browser loopback (`docs/chatgpt.md`), and it keeps their
//! two-layer split exactly: a long-lived token in the `.env` key store,
//! exchanged (and cached in memory, never written) for the short-lived
//! credential the API actually takes.
//!
//! # Which Anthropic sign-in this is — and which it deliberately is not
//!
//! There are two Anthropic OAuth clients, and conflating them is the whole
//! trap. This is the **Console** one: the flow Anthropic's own `ant` CLI
//! runs, whose tokens are documented as valid on `/v1/messages` and whose
//! usage bills to the account's API organisation, exactly as a pasted key
//! does. Every constant below was read from that CLI's published source.
//!
//! It is **not** the Claude Pro/Max subscription sign-in. Anthropic's usage
//! policy is explicit that third-party clients may not offer Claude.ai login
//! or route requests through subscription credentials, and that flow
//! additionally requires impersonating Claude Code's client id, user agent
//! and system prompt. `docs/claude.md` carries the quote and the reasoning.
//!
//! # Where it differs from the OpenAI flow next door
//!
//! Every difference is a place a copied flow breaks:
//!
//! - The two grants **disagree about everything**: the code exchange is
//!   form-encoded and sends *no* beta header, while the refresh is JSON and
//!   *requires* one. Anthropic's CLI carries a comment on each; swap either
//!   and the endpoint routes the request to a handler that refuses it.
//! - The hosted callback URI carries a **query string** (`?app=…`) that is
//!   part of the registered redirect verbatim — dropping it fails the
//!   authorize step, not the exchange.
//! - The loopback port is **ephemeral**. There is no allow-listed pair to
//!   bind, so a port is essentially always available and the hosted
//!   paste-a-code page is a fallback rather than a failure.

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

/// The OAuth client id Anthropic's own CLI signs in with. There is no
/// third-party registration path for this API, so a client that signs a user
/// in to their own Anthropic account uses it — the same posture
/// `docs/chatgpt.md` documents for Codex's client id.
pub const CLIENT_ID: &str = "41077d10-94b8-4194-be48-d251e9eb21b4";

/// The Console host the sign-in is approved on.
const CONSOLE: &str = "https://platform.claude.com";

/// Where both grants post. **The API host, not the Console one** — the two
/// halves of this flow live on different hosts.
const TOKEN_URL: &str = "https://api.anthropic.com/v1/oauth/token";

/// Anthropic's hosted callback page, which displays a code to paste. Used as
/// the `redirect_uri` when no loopback port can be bound.
///
/// The `?app=` query string is **part of the registered redirect URI**, not a
/// decoration: an authorize call whose `redirect_uri` drops it is refused
/// with "redirect_uri not supported by client".
pub const HOSTED_REDIRECT_URI: &str =
    "https://platform.claude.com/oauth/code/callback?app=anthropic-cli";

/// What the sign-in asks for. `user:inference` is the one that matters: it is
/// what lets the resulting token call `/v1/messages` at all.
const SCOPES: &str = "user:profile user:inference user:developer";

/// The `anthropic-beta` value an OAuth bearer must ride with on every API
/// request — and on the **refresh** grant, though not on the code exchange.
/// A pasted API key must never send it.
pub const OAUTH_BETA: &str = "oauth-2025-04-20";

/// The scopes that let a token call the inference API. A token without one of
/// these authenticates fine and then fails every turn.
const INFERENCE_SCOPES: [&str; 4] = [
    "user:inference",
    "user:ccr_inference",
    "org:service_key_inference",
    "workspace:inference",
];

/// How long one token-endpoint call may take.
const OP_TIMEOUT: Duration = Duration::from_secs(30);

/// How long the sign-in page waits for the browser before giving up — the
/// same patience `docs/chatgpt.md`'s flow keeps, and the number its countdown
/// shows.
pub const AUTH_TIMEOUT: Duration = Duration::from_secs(600);

/// The most a token-endpoint response body may buffer.
const BODY_MAX_BYTES: u64 = 64 * 1024;

/// How much of an access token's life is given back before it is re-minted,
/// so a request never leaves with a token that expires in flight.
const REFRESH_SKEW: Duration = Duration::from_secs(300);

/// The floor on a cached token's life. Anthropic reports a *duration*
/// (`expires_in`) rather than an absolute expiry, so a skewed clock cannot
/// make a token look already dead — but a server that answers a very short
/// life would otherwise re-mint on every request.
const MIN_CACHE: Duration = Duration::from_secs(60);

/// The environment variable the refresh token lives under — the same name
/// `providers.toml` gives the provider's `api_key_env`, pinned here because
/// the rotation write-back cannot ask the provider file.
pub const REFRESH_ENV_VAR: &str = "ANTHROPIC_CONSOLE_REFRESH_TOKEN";

/// One nap between cancel checks while the loopback listener waits.
const POLL_NAP: Duration = Duration::from_millis(200);

// ---------------------------------------------------------------------------
// The flow's URLs and bodies (pure)
// ---------------------------------------------------------------------------

/// The loopback `redirect_uri` for a bound port.
#[must_use]
pub fn redirect_uri(port: u16) -> String {
    format!("http://localhost:{port}/callback")
}

/// The authorize URL to send the user to.
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
    ]);
    format!("{CONSOLE}/oauth/authorize?{query}")
}

/// The authorization-code grant's body — **form-encoded**, and sent with no
/// `anthropic-beta` header at all.
///
/// The pairing is load-bearing and inverted from the refresh grant below: a
/// beta header on this grant routes it to a handler that implements only the
/// JWT-bearer exchange and refuses an authorization code.
#[must_use]
pub fn code_exchange_body(code: &str, verifier: &str, state: &str, redirect_uri: &str) -> String {
    form_encode(&[
        ("grant_type".to_string(), "authorization_code".to_string()),
        ("code".to_string(), code.to_string()),
        ("code_verifier".to_string(), verifier.to_string()),
        ("client_id".to_string(), CLIENT_ID.to_string()),
        ("redirect_uri".to_string(), redirect_uri.to_string()),
        ("state".to_string(), state.to_string()),
    ])
}

/// The refresh grant's body — **JSON**, and sent *with* the
/// [`OAUTH_BETA`] header. See [`code_exchange_body`] for why the two grants
/// disagree.
#[must_use]
pub fn refresh_body(refresh_token: &str) -> serde_json::Value {
    serde_json::json!({
        "grant_type": "refresh_token",
        "refresh_token": refresh_token,
        "client_id": CLIENT_ID,
    })
}

/// What the user pasted, split into its code and the `state` that came with
/// it. Accepts the three shapes the callback can arrive in: a bare code, a
/// whole redirect URL (or its query string), and the `code#state` form the
/// hosted page shows.
///
/// A bare code is accepted deliberately: the exchange sends `state` anyway
/// and the server validates it, so refusing a paste that merely lost its
/// fragment would block a sign-in that is about to succeed.
#[must_use]
pub fn parse_code_input(text: &str) -> Option<(String, Option<String>)> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    // The hosted page's own shape, which is not a URL at all — checked first,
    // since a `#` fragment is invisible to a query-string parse.
    if !text.contains('?')
        && let Some((code, state)) = text.split_once('#')
    {
        let code = code.trim();
        let state = state.trim();
        if !code.is_empty() && !code.contains(char::is_whitespace) {
            return Some((
                code.to_string(),
                (!state.is_empty()).then(|| state.to_string()),
            ));
        }
    }
    if let Some(parsed) = parse_redirect(text) {
        return Some(parsed);
    }
    // A bare code. Anything with whitespace in it is not one.
    (!text.contains(char::is_whitespace)).then(|| (text.to_string(), None))
}

/// What a token response carried. `refresh_token` is optional on a refresh —
/// the server re-sends it only when it rotated — so a missing one means "keep
/// the one you have" rather than "you have none".
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct TokenSet {
    #[serde(default)]
    pub access_token: Option<String>,
    #[serde(default)]
    pub refresh_token: Option<String>,
    /// Seconds of life, from which the cache lifetime is computed.
    #[serde(default)]
    pub expires_in: Option<u64>,
    /// The space-delimited scope list the token actually carries.
    #[serde(default)]
    pub scope: Option<String>,
    /// The organisation the usage bills to, for the sign-in confirmation.
    #[serde(default)]
    pub organization: Option<Named>,
}

/// A `{uuid, name}` object the token response carries for the organisation
/// (and, unused here, the account and workspace).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct Named {
    #[serde(default)]
    pub name: Option<String>,
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
                "Anthropic returned no access token".to_string(),
            ));
        }
        Ok(parsed)
    }

    /// The scope list, split.
    #[must_use]
    pub fn scopes(&self) -> Vec<&str> {
        self.scope
            .as_deref()
            .unwrap_or_default()
            .split_whitespace()
            .collect()
    }
}

/// Can a token carrying `scopes` call the Messages API?
///
/// An **empty** list passes: some responses omit `scope` entirely, and
/// absence is not evidence of a bad token. A non-empty list without an
/// inference scope is, though — and catching it here turns a sign-in that
/// granted the wrong reach into an error at the moment it happened rather
/// than an authentication failure at the first turn.
#[must_use]
pub fn scopes_allow_inference(scopes: &[&str]) -> bool {
    scopes.is_empty() || scopes.iter().any(|s| INFERENCE_SCOPES.contains(s))
}

/// How long an access token with `expires_in` seconds of life may be cached.
/// `REFRESH_SKEW` comes off the end; `MIN_CACHE` is the floor.
#[must_use]
pub fn cache_lifetime(expires_in: Option<u64>) -> Duration {
    let Some(secs) = expires_in else {
        // A token that doesn't say: trust it for a conservative single minute
        // rather than forever.
        return MIN_CACHE;
    };
    Duration::from_secs(secs)
        .saturating_sub(REFRESH_SKEW)
        .max(MIN_CACHE)
}

/// Explain a sign-in, refresh, or request failure in terms the user can act
/// on. The wire bodies name an error code and little a user can do with it,
/// and the several ways an Anthropic credential can be refused each need a
/// different answer.
#[must_use]
pub fn auth_advice(status: u16, body: &str) -> Option<String> {
    let lower = body.to_ascii_lowercase();
    // The policy block, not a token problem: retrying and signing in again
    // cannot fix it, so say the one thing that can.
    if lower.contains("oauth authentication is currently not supported")
        || lower.contains("oauth authentication is currently not allowed")
    {
        return Some(
            "Anthropic refused this OAuth credential — sign in again with /login, or use an \
             Anthropic API key instead (/login → Use an API key)."
                .to_string(),
        );
    }
    if lower.contains("enforced_spend_limit_reached") {
        return Some(
            "this account has reached its Anthropic spend limit — raise it in the Console, \
             or switch provider with /model."
                .to_string(),
        );
    }
    let code = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| {
            v.get("error").and_then(|e| {
                e.as_str().map(str::to_string).or_else(|| {
                    e.get("type")
                        .or_else(|| e.get("code"))
                        .and_then(|t| t.as_str())
                        .map(str::to_string)
                })
            })
        })
        .unwrap_or_default();
    match (status, code.as_str()) {
        (401, _) | (_, "invalid_grant" | "invalid_token" | "authentication_error") => Some(
            "your Anthropic sign-in is no longer valid — run /login and sign in again.".to_string(),
        ),
        // A valid credential without the reach: a scope problem, not an
        // expiry, so "sign in again" would be the wrong advice.
        (403, _) | (_, "permission_error") => Some(
            "this Anthropic credential isn't allowed to make that request — check the \
             account's API access in the Console."
                .to_string(),
        ),
        (429, _) => Some(
            "Anthropic is rate-limiting this account — wait for the limit to reset, \
             or switch provider with /model."
                .to_string(),
        ),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// The access-token cache (boundary state)
// ---------------------------------------------------------------------------

/// One minted access token.
#[derive(Debug, Clone)]
pub struct Access {
    /// The bearer the request carries.
    pub bearer: String,
    /// The scopes it turned out to carry.
    pub scopes: Vec<String>,
    /// The organisation the usage bills to, when the response named one.
    pub organization: Option<String>,
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
/// Rotation is not only a disk concern: the live `ModelConfig` carries the
/// token the session started with and nothing reloads it mid-run, so once
/// Anthropic retires that token the *next* refresh would present a dead one.
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
    if let Ok(mut map) = cache().lock() {
        map.remove(refresh_token);
    }
    if let Ok(mut map) = rotations().lock() {
        map.remove(refresh_token);
    }
}

// ---------------------------------------------------------------------------
// The sign-in flow and the token mint (boundary — real HTTP and a listener)
// ---------------------------------------------------------------------------

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
            format!("Anthropic answered {status}")
        } else {
            format!("Anthropic answered {status}: {trimmed}")
        }
    });
    LlmError::Api {
        status,
        body: explained,
    }
}

/// The page shown once the browser comes back.
const SUCCESS_PAGE: &str = "<html><body style=\"font-family:system-ui;padding:3rem\"><h3>Signed in to Anthropic.</h3><p>You can close this tab and return to the terminal.</p></body></html>";

/// The sign-in page's live state, reported as the flow runs.
pub struct Signin {
    /// The URL to open — shown on the page and copyable.
    pub url: String,
    /// The listener the browser comes back to, when a port was free. Without
    /// one the flow uses Anthropic's hosted callback page, which shows a code
    /// to paste instead.
    listener: Option<TcpListener>,
    /// The PKCE verifier the exchange redeems the code with.
    verifier: String,
    /// The `state` the callback must echo — a value of its own here, not the
    /// verifier: putting the verifier in the URL would publish the secret the
    /// challenge exists to keep.
    state: String,
    /// The redirect URI the exchange must repeat verbatim.
    redirect_uri: String,
}

impl Signin {
    /// Whether the browser will come back on its own. `false` means the user
    /// has a code to bring back by hand.
    #[must_use]
    pub fn is_loopback(&self) -> bool {
        self.listener.is_some()
    }
}

/// Open the loopback listener (when a port is free) and build the URL to send
/// the user to. The caller shows [`Signin::url`], then blocks in
/// [`await_callback`].
///
/// Unlike OpenAI's flow this takes **any** port — this client is not pinned
/// to an allow-listed pair — so a bound port is the normal case and the
/// hosted page is a fallback rather than a failure.
///
/// # Errors
/// Infallible today; the signature matches the other flows' so a future
/// binding requirement needs no call-site change.
pub fn begin_signin() -> Result<Signin> {
    let (verifier, challenge, state) = pkce_and_state();
    let listener = TcpListener::bind(("127.0.0.1", 0)).ok();
    let port = listener
        .as_ref()
        .and_then(|listener| listener.local_addr().ok())
        .map(|addr| addr.port());
    let redirect_uri = port.map_or_else(|| HOSTED_REDIRECT_URI.to_string(), redirect_uri);
    // A listener whose address could not be read is not one we can wait on.
    let listener = listener.filter(|_| port.is_some());
    Ok(Signin {
        url: authorize_url(&challenge, &state, &redirect_uri),
        listener,
        verifier,
        state,
        redirect_uri,
    })
}

/// Wait for the browser's callback, then redeem the code. Returns the refresh
/// token to store and the access token it came with.
///
/// # Errors
/// A cancelled flow, a timeout, a `state` that doesn't match, a token
/// exchange Anthropic refused, or a token without an inference scope.
pub fn await_callback(signin: &Signin, cancel: &CancelToken) -> Result<(String, Access)> {
    let Some(listener) = signin.listener.as_ref() else {
        return Err(LlmError::Http(
            "no loopback port was free — paste the code from the browser instead".to_string(),
        ));
    };
    let code = wait_for_code(listener, &signin.state, cancel)?;
    redeem(&code, &signin.verifier, &signin.state, &signin.redirect_uri)
}

/// Redeem an authorization code for tokens.
///
/// # Errors
/// A mismatched `state`, a refusal from the token endpoint, or a token whose
/// scopes cannot reach the inference API.
pub fn redeem(
    input: &str,
    verifier: &str,
    state: &str,
    redirect_uri: &str,
) -> Result<(String, Access)> {
    let (code, pasted_state) = parse_code_input(input)
        .ok_or_else(|| LlmError::Decode("that isn't an authorization code".to_string()))?;
    if let Some(pasted) = pasted_state.filter(|s| !s.is_empty())
        && pasted != state
    {
        return Err(LlmError::Http(
            "the callback's state doesn't match — start the sign-in again".to_string(),
        ));
    }
    let client = super::http_client(OP_TIMEOUT)?;
    let resp = client
        .post(TOKEN_URL)
        .header("content-type", "application/x-www-form-urlencoded")
        .header("accept", "application/json")
        // Deliberately no `anthropic-beta` here — see `code_exchange_body`.
        .body(code_exchange_body(&code, verifier, state, redirect_uri))
        .send()
        .map_err(|e| LlmError::Http(e.to_string()))?;
    let status = resp.status().as_u16();
    let text = read_body(resp);
    if !(200..300).contains(&status) {
        return Err(token_failure(status, &text));
    }
    let tokens = TokenSet::parse(&text)?;
    let access = access_from(&tokens)?;
    let refresh = tokens
        .refresh_token
        .filter(|t| !t.is_empty())
        .ok_or_else(|| {
            // Without one the session dies at the first token expiry with no
            // way back but another sign-in — better said now.
            LlmError::Decode("Anthropic returned no refresh token — sign in again".to_string())
        })?;
    Ok((refresh, access))
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
                // not the callback — keep waiting rather than failing.
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(POLL_NAP);
            }
            Err(e) => return Err(LlmError::Http(format!("callback port: {e}"))),
        }
    }
}

/// The [`Access`] a token response describes, refusing one that cannot reach
/// the inference API.
fn access_from(tokens: &TokenSet) -> Result<Access> {
    let bearer = tokens
        .access_token
        .clone()
        .ok_or_else(|| LlmError::Decode("Anthropic returned no access token".to_string()))?;
    let scopes = tokens.scopes();
    if !scopes_allow_inference(&scopes) {
        return Err(LlmError::Decode(
            "that sign-in cannot reach the Claude API — it granted no inference scope. \
             Sign in again from the /login page."
                .to_string(),
        ));
    }
    Ok(Access {
        bearer,
        scopes: scopes.iter().map(|s| (*s).to_string()).collect(),
        organization: tokens
            .organization
            .as_ref()
            .and_then(|o| o.name.clone())
            .filter(|n| !n.trim().is_empty()),
    })
}

/// The access token to authenticate with, minted from `refresh_token` and
/// cached until shortly before its own expiry. Boundary code — one HTTP round
/// trip on a cache miss, none on a hit.
///
/// A rotated refresh token is written back to the key store before this
/// returns: Anthropic may retire the one just used, and leaving the retired
/// value on disk turns the next launch into a forced re-login.
///
/// # Errors
/// A refresh Anthropic refused, or a transport failure.
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
        .post(TOKEN_URL)
        .header("content-type", "application/json")
        .header("accept", "application/json")
        // Required on *this* grant, and only this one — see `refresh_body`.
        .header("anthropic-beta", OAUTH_BETA)
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
    let lifetime = cache_lifetime(tokens.expires_in);
    let store = |key: String, access: &Access| {
        if let Ok(mut map) = cache().lock() {
            map.insert(
                key,
                Cached {
                    access: access.clone(),
                    until: Instant::now() + lifetime,
                },
            );
        }
    };
    store(refresh_token.to_string(), &access);
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
        store(rotated, &access);
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
        set_owner_only(&path);
    }
}

/// `0600` on the key store — the same guard `/login`'s own write applies.
fn set_owner_only(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// What the sign-in confirmation names beside the provider: the organisation
/// the usage will bill to. That is the question this flow leaves open — a
/// user with several Anthropic orgs has just picked one in a browser — where
/// the two subscription flows leave the *plan* open instead.
#[must_use]
pub fn plan_label(access: &Access) -> Option<String> {
    access.organization.clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_authorize_url_is_the_console_flow_with_pkce() {
        let url = authorize_url("CHAL", "STATE", "http://localhost:5000/callback");
        assert!(
            url.starts_with("https://platform.claude.com/oauth/authorize?"),
            "{url}"
        );
        assert!(url.contains(&format!("client_id={CLIENT_ID}")), "{url}");
        assert!(url.contains("response_type=code"), "{url}");
        assert!(url.contains("code_challenge=CHAL"), "{url}");
        assert!(url.contains("code_challenge_method=S256"), "{url}");
        assert!(url.contains("state=STATE"), "{url}");
        assert!(
            url.contains("redirect_uri=http%3A%2F%2Flocalhost%3A5000%2Fcallback"),
            "{url}"
        );
        assert!(url.contains("user%3Ainference"), "the scope that matters");
    }

    #[test]
    fn the_state_is_never_the_verifier() {
        // Putting the verifier in the URL would publish the one secret the
        // challenge exists to keep.
        let signin = begin_signin().expect("a port, or the hosted fallback");
        assert_ne!(signin.state, signin.verifier);
        assert!(!signin.url.contains(&signin.verifier), "verifier leaked");
        assert!(signin.url.contains(&signin.state));
    }

    #[test]
    fn the_hosted_redirect_keeps_its_query_string() {
        // The `?app=` is part of the registered redirect URI: an authorize
        // call that drops it is refused before the exchange ever runs.
        assert!(HOSTED_REDIRECT_URI.contains("?app="));
        let url = authorize_url("c", "s", HOSTED_REDIRECT_URI);
        assert!(url.contains("%3Fapp%3D"), "{url}");
    }

    #[test]
    fn the_code_exchange_is_form_encoded() {
        // Form for the code, JSON for the refresh — swapping either routes
        // the request to a handler that refuses it.
        let body = code_exchange_body("CODE", "VER", "STATE", "http://localhost:1/callback");
        assert!(body.contains("grant_type=authorization_code"), "{body}");
        assert!(body.contains("&code=CODE"), "{body}");
        assert!(body.contains("&code_verifier=VER"), "{body}");
        assert!(body.contains("&state=STATE"), "{body}");
        assert!(body.contains(&format!("&client_id={CLIENT_ID}")), "{body}");
        assert!(
            body.contains("&redirect_uri=http%3A%2F%2Flocalhost%3A1%2Fcallback"),
            "{body}"
        );
    }

    #[test]
    fn the_refresh_grant_is_json() {
        let body = refresh_body("RT");
        assert_eq!(body["grant_type"], "refresh_token");
        assert_eq!(body["refresh_token"], "RT");
        assert_eq!(body["client_id"], CLIENT_ID);
        // No scope list on this grant — the token keeps what it was minted
        // with.
        assert!(body.get("scope").is_none());
    }

    #[test]
    fn a_code_arrives_bare_as_a_url_or_as_the_hosted_pages_fragment() {
        assert_eq!(parse_code_input("abc123"), Some(("abc123".into(), None)));
        assert_eq!(
            parse_code_input("http://localhost:1/callback?code=abc&state=xyz"),
            Some(("abc".into(), Some("xyz".into())))
        );
        // The hosted page's own shape, which is not a URL at all.
        assert_eq!(
            parse_code_input("abc#xyz"),
            Some(("abc".into(), Some("xyz".into())))
        );
        assert_eq!(
            parse_code_input("  abc#xyz  "),
            Some(("abc".into(), Some("xyz".into())))
        );
        assert_eq!(parse_code_input(""), None);
        assert_eq!(parse_code_input("   "), None);
        assert_eq!(parse_code_input("not a code"), None);
    }

    #[test]
    fn a_bare_code_is_accepted_rather_than_refused_for_its_missing_fragment() {
        // The exchange sends `state` anyway and the server validates it, so
        // refusing a paste that lost its fragment blocks a sign-in that is
        // about to succeed.
        let (code, state) = parse_code_input("justthecode").expect("a code");
        assert_eq!(code, "justthecode");
        assert_eq!(state, None);
    }

    #[test]
    fn a_token_without_an_inference_scope_is_refused_at_the_sign_in() {
        assert!(scopes_allow_inference(&["user:profile", "user:inference"]));
        assert!(scopes_allow_inference(&["workspace:inference"]));
        assert!(!scopes_allow_inference(&["user:profile", "user:developer"]));
        // Absence of a scope list is not evidence of a bad token.
        assert!(scopes_allow_inference(&[]));
    }

    #[test]
    fn a_token_response_parses_and_a_bodyless_one_fails() {
        let tokens = TokenSet::parse(
            r#"{"access_token":"at","refresh_token":"rt","expires_in":3600,
                "token_type":"Bearer","scope":"user:inference user:profile",
                "organization":{"uuid":"o","name":"Acme"},
                "account":{"uuid":"a","email_address":"x@y.z"}}"#,
        )
        .unwrap();
        assert_eq!(tokens.access_token.as_deref(), Some("at"));
        assert_eq!(tokens.refresh_token.as_deref(), Some("rt"));
        assert_eq!(tokens.expires_in, Some(3600));
        assert_eq!(tokens.scopes(), vec!["user:inference", "user:profile"]);
        let access = access_from(&tokens).unwrap();
        assert_eq!(access.organization.as_deref(), Some("Acme"));
        assert_eq!(plan_label(&access).as_deref(), Some("Acme"));

        assert!(TokenSet::parse(r#"{"refresh_token":"rt"}"#).is_err());
        assert!(TokenSet::parse("not json").is_err());
    }

    #[test]
    fn a_refresh_that_rotated_nothing_keeps_the_token_it_was_given() {
        // The server re-sends `refresh_token` only when it rotated.
        let tokens = TokenSet::parse(r#"{"access_token":"at","expires_in":3600}"#).unwrap();
        assert_eq!(tokens.refresh_token, None);
        assert_eq!(access_from(&tokens).unwrap().organization, None);
    }

    #[test]
    fn the_cache_lifetime_gives_back_the_skew_and_keeps_a_floor() {
        assert_eq!(
            cache_lifetime(Some(3600)),
            Duration::from_secs(3600 - 300),
            "an hour, minus the five-minute skew"
        );
        // A very short life must not re-mint on every request.
        assert_eq!(cache_lifetime(Some(60)), MIN_CACHE);
        assert_eq!(cache_lifetime(Some(0)), MIN_CACHE);
        // A token that doesn't say gets a conservative single minute.
        assert_eq!(cache_lifetime(None), MIN_CACHE);
    }

    #[test]
    fn auth_advice_names_the_thing_to_do_about_each_refusal() {
        // The policy block is not an expiry: signing in again cannot fix it,
        // so the advice has to name the other door.
        let blocked = auth_advice(
            401,
            r#"{"error":{"message":"OAuth authentication is currently not supported."}}"#,
        )
        .expect("advice");
        assert!(blocked.contains("API key"), "{blocked}");

        let expired =
            auth_advice(401, r#"{"error":{"type":"authentication_error"}}"#).expect("advice");
        assert!(expired.contains("/login"), "{expired}");

        // A scope problem, not an expiry — "sign in again" would be wrong.
        let forbidden =
            auth_advice(403, r#"{"error":{"type":"permission_error"}}"#).expect("advice");
        assert!(!forbidden.contains("sign in again"), "{forbidden}");

        let capped = auth_advice(
            429,
            r#"{"error":{"type":"rate_limit_error","details":{"error_code":"enforced_spend_limit_reached"}}}"#,
        )
        .expect("advice");
        assert!(capped.contains("spend limit"), "{capped}");

        let limited = auth_advice(429, r#"{"error":{"type":"rate_limit_error"}}"#).expect("advice");
        assert!(limited.contains("rate-limiting"), "{limited}");

        // An ordinary 400 has nothing useful to add — pass it through.
        assert_eq!(auth_advice(400, r#"{"error":"bad_request"}"#), None);
    }

    #[test]
    fn the_env_var_matches_the_shipped_provider_file() {
        // The rotation write-back cannot ask the provider file, so the two
        // must be pinned to each other.
        let file = super::super::ProvidersFile::builtin();
        let provider = file.get("anthropic_console").expect("shipped");
        assert_eq!(provider.key_env("anthropic_console"), REFRESH_ENV_VAR);
    }

    #[test]
    fn a_loopback_sign_in_names_its_own_port() {
        let signin = begin_signin().expect("a port, or the hosted fallback");
        if signin.is_loopback() {
            assert!(
                signin.redirect_uri.starts_with("http://localhost:"),
                "{}",
                signin.redirect_uri
            );
            assert!(signin.redirect_uri.ends_with("/callback"));
        } else {
            assert_eq!(signin.redirect_uri, HOSTED_REDIRECT_URI);
        }
        assert!(signin.url.contains("code_challenge="));
    }
}
