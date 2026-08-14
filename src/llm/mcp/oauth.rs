//! The MCP OAuth flow (`docs/mcp.md`): RFC 9728 protected-resource
//! discovery → RFC 8414 authorization-server metadata (with the OIDC
//! fallback) → RFC 7591 dynamic client registration when offered → the
//! authorization-code + PKCE (S256) grant with an RFC 8707 `resource`
//! indicator → a loopback callback server *and* the paste-the-redirect-URL
//! fallback → the token exchange, with refresh-token rotation on expiry.
//!
//! Tokens persist in `{config_home}/mcp-auth.json` keyed by server URL
//! (0600, best-effort). The pure pieces — URL building, the redirect parse,
//! the challenge parse, the store format, form encoding — are unit-tested;
//! the HTTP/socket work is exercised by the manager's fixtures.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::stream::CancelToken;

/// The whole interactive flow's budget — Claude Code's 5 minutes.
pub const AUTH_FLOW_TIMEOUT: Duration = Duration::from_secs(300);

/// The wait-loop poll cadence (the transport's).
const POLL: Duration = Duration::from_millis(50);

/// Discovery/token requests' per-operation deadline.
const HTTP_TIMEOUT: Duration = Duration::from_secs(20);

/// Serializes the token store's read-modify-write. The store is **one file
/// for every server**, so without this two servers renewing at once (two
/// subagents, or a tool call while the loop reconnects another) interleave
/// read/read/write/write and drop one of the two rotated refresh tokens.
/// The loser then presents an already-used token, which a spec-compliant
/// authorization server treats as a replay and answers by revoking the whole
/// family — the browser flow again, for the exact reason the refresh exists.
static STORE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Refresh a token this long before it actually expires — five minutes, the
/// production default the reference clients converged on (60 s is the floor;
/// clock skew plus in-flight latency eat a short window).
const EXPIRY_SKEW_SECS: u64 = 300;

// ---------------------------------------------------------------------------
// the token store (pure format + file I/O)

/// One server's stored grant.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StoredTokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
    /// Unix seconds; `None` = no known expiry.
    pub expires_at: Option<u64>,
    pub client_id: String,
    pub client_secret: Option<String>,
    pub token_endpoint: String,
    pub scope: Option<String>,
}

/// Parse the store document into `server_url → tokens`.
#[must_use]
pub fn parse_token_store(contents: &str) -> std::collections::BTreeMap<String, StoredTokens> {
    let Ok(value) = serde_json::from_str::<Value>(contents.trim()) else {
        return Default::default();
    };
    let Some(servers) = value.get("servers").and_then(Value::as_object) else {
        return Default::default();
    };
    servers
        .iter()
        .filter_map(|(url, entry)| {
            let text = |key: &str| entry.get(key).and_then(Value::as_str).map(str::to_string);
            Some((
                url.clone(),
                StoredTokens {
                    access_token: text("access_token")?,
                    refresh_token: text("refresh_token"),
                    expires_at: entry.get("expires_at").and_then(Value::as_u64),
                    client_id: text("client_id").unwrap_or_default(),
                    client_secret: text("client_secret"),
                    token_endpoint: text("token_endpoint").unwrap_or_default(),
                    scope: text("scope"),
                },
            ))
        })
        .collect()
}

/// Re-render the store with one server's entry replaced (`None` deletes it)
/// — the read-modify-write core.
#[must_use]
pub fn record_tokens(contents: &str, server_url: &str, tokens: Option<&StoredTokens>) -> String {
    let mut value: Value = serde_json::from_str(contents.trim())
        .ok()
        .filter(Value::is_object)
        .unwrap_or_else(|| Value::Object(Default::default()));
    let root = value.as_object_mut().expect("object ensured above");
    let servers = root
        .entry("servers")
        .or_insert_with(|| Value::Object(Default::default()));
    if let Some(servers) = servers.as_object_mut() {
        match tokens {
            Some(tokens) => {
                let mut entry = serde_json::Map::new();
                entry.insert("access_token".into(), tokens.access_token.clone().into());
                if let Some(refresh) = &tokens.refresh_token {
                    entry.insert("refresh_token".into(), refresh.clone().into());
                }
                if let Some(expires) = tokens.expires_at {
                    entry.insert("expires_at".into(), expires.into());
                }
                entry.insert("client_id".into(), tokens.client_id.clone().into());
                if let Some(secret) = &tokens.client_secret {
                    entry.insert("client_secret".into(), secret.clone().into());
                }
                entry.insert(
                    "token_endpoint".into(),
                    tokens.token_endpoint.clone().into(),
                );
                if let Some(scope) = &tokens.scope {
                    entry.insert("scope".into(), scope.clone().into());
                }
                servers.insert(server_url.to_string(), Value::Object(entry));
            }
            None => {
                servers.remove(server_url);
            }
        }
    }
    serde_json::to_string_pretty(&value).unwrap_or_else(|_| contents.to_string())
}

/// Load one server's stored tokens.
#[must_use]
pub fn load_tokens(path: Option<&Path>, server_url: &str) -> Option<StoredTokens> {
    let contents = std::fs::read_to_string(path?).ok()?;
    parse_token_store(&contents).remove(server_url)
}

/// Persist (or delete) one server's tokens — read-modify-write, owner-only,
/// best-effort like every other config write.
pub fn save_tokens(path: Option<&Path>, server_url: &str, tokens: Option<&StoredTokens>) {
    let Some(path) = path else { return };
    let _guard = STORE_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let existing = std::fs::read_to_string(path).unwrap_or_default();
    let updated = record_tokens(&existing, server_url, tokens);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    // Write-then-rename, so a crash mid-write can't leave a truncated store:
    // a half-written file parses as *no* servers, silently unauthenticating
    // every one of them at once.
    let temp = path.with_extension("json.tmp");
    if std::fs::write(&temp, &updated).is_ok() {
        set_owner_only(&temp);
        if std::fs::rename(&temp, path).is_ok() {
            return;
        }
        let _ = std::fs::remove_file(&temp);
    }
    let _ = std::fs::write(path, updated);
    set_owner_only(path);
}

/// `0600` — the store holds refresh tokens, which the spec requires be kept
/// confidential in storage.
fn set_owner_only(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// Is this grant still fresh (with the refresh skew)? `now` is Unix seconds.
#[must_use]
pub fn tokens_fresh(tokens: &StoredTokens, now: u64) -> bool {
    match tokens.expires_at {
        Some(expires) => now + EXPIRY_SKEW_SECS < expires,
        None => true,
    }
}

/// Why a refresh failed — the split that decides what happens to the stored
/// grant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefreshFailure {
    /// Nothing stored, or the grant carries no refresh token: interactive
    /// auth is the only way forward (the spec lets an AS withhold refresh
    /// tokens — a client MUST NOT assume one).
    NoGrant,
    /// The AS rejected the grant itself (`invalid_grant` and friends): the
    /// refresh token is dead, the stored grant is cleared, and only an
    /// interactive re-auth recovers.
    Permanent(String),
    /// Network/5xx/429 or an unreadable answer: the grant is kept untouched
    /// and the refresh simply retried later. Demanding re-auth here was the
    /// reference client's own long-standing bug (fixed in Claude Code
    /// v2.1.206) — a blip must not cost the user their session.
    Transient(String),
}

/// Classify a failed token-endpoint answer. `status` is `None` when the
/// request never got one (a network failure — transient by definition).
/// Only the AS's own structured verdict on a 400/401 — the RFC 6749 §5.2
/// `{"error": …}` shape — is permanent; an unparseable body at those
/// statuses is a proxy or gateway speaking, not the AS.
#[must_use]
pub fn refresh_failure_kind(status: Option<u16>, body: &str) -> RefreshFailure {
    let flat = || {
        let flat: String = body.split_whitespace().collect::<Vec<_>>().join(" ");
        flat.chars().take(200).collect::<String>()
    };
    let Some(status) = status else {
        return RefreshFailure::Transient(flat());
    };
    if matches!(status, 400 | 401) {
        let parsed = serde_json::from_str::<Value>(body.trim()).ok();
        let error = parsed
            .as_ref()
            .and_then(|v| v.get("error"))
            .and_then(Value::as_str);
        if let Some(error) = error {
            let description = parsed
                .as_ref()
                .and_then(|v| v.get("error_description"))
                .and_then(Value::as_str)
                .map(|d| format!(": {d}"))
                .unwrap_or_default();
            return RefreshFailure::Permanent(format!("{error}{description}"));
        }
    }
    RefreshFailure::Transient(format!("HTTP {status}: {}", flat()))
}

/// The refresh grant's form body (RFC 6749 §6 + RFC 8707): no `scope` — it
/// could only narrow the grant, and some ASes narrow it permanently — and no
/// PKCE, which belongs to the authorization-code grant alone. The `resource`
/// indicator rides every token request, the refresh included: omitting it
/// mints a token audience-bound to nothing, which the MCP server then 401s —
/// the classic silent refresh loop.
#[must_use]
pub fn refresh_params(tokens: &StoredTokens, server_url: &str) -> Vec<(String, String)> {
    let mut params = vec![
        ("grant_type".to_string(), "refresh_token".to_string()),
        (
            "refresh_token".to_string(),
            tokens.refresh_token.clone().unwrap_or_default(),
        ),
        ("client_id".to_string(), tokens.client_id.clone()),
    ];
    if let Some(secret) = &tokens.client_secret {
        params.push(("client_secret".to_string(), secret.clone()));
    }
    params.push(("resource".to_string(), server_url.to_string()));
    params
}

/// Refresh the stored grant for `server_url` through its token endpoint —
/// forced, regardless of freshness (the reactive 401 path needs exactly
/// that). The store is re-read first, so another process's newer grant is
/// used instead of burning a rotated refresh token. On success the rotated
/// grant is persisted (`tokens_from_response` keeps the old refresh token
/// when the AS returns none — never overwrite with nothing) and returned; a
/// [`RefreshFailure::Permanent`] rejection clears the store entry, because a
/// dead grant kept around loops the failure into every later request.
pub fn refresh_grant(
    path: Option<&Path>,
    server_url: &str,
) -> Result<StoredTokens, RefreshFailure> {
    let Some(tokens) = load_tokens(path, server_url) else {
        return Err(RefreshFailure::NoGrant);
    };
    if tokens.refresh_token.is_none() || tokens.token_endpoint.trim().is_empty() {
        return Err(RefreshFailure::NoGrant);
    }
    let params = refresh_params(&tokens, server_url);
    let (status, body) = match post_form_raw(&tokens.token_endpoint, &params) {
        Ok(answer) => answer,
        Err(detail) => return Err(RefreshFailure::Transient(detail)),
    };
    if !(200..300).contains(&status) {
        let failure = refresh_failure_kind(Some(status), &body);
        if matches!(failure, RefreshFailure::Permanent(_)) {
            save_tokens(path, server_url, None);
        }
        return Err(failure);
    }
    let Ok(response) = serde_json::from_str::<Value>(&body) else {
        return Err(RefreshFailure::Transient(
            "the token endpoint answered non-JSON".to_string(),
        ));
    };
    let Some(refreshed) = tokens_from_response(&response, &tokens) else {
        return Err(RefreshFailure::Transient(
            "the token response carried no access_token".to_string(),
        ));
    };
    save_tokens(path, server_url, Some(&refreshed));
    Ok(refreshed)
}

/// The bearer a connect should send: the stored access token, refreshed
/// first when it sits inside the expiry skew and a refresh token exists. A
/// transient refresh failure still sends the stored token — the skew is a
/// refresh *trigger*, not an invalidity verdict, so the token may have
/// minutes left; and the connect's own 401 path retries the refresh with the
/// classification deciding the final state. `None` only when nothing is
/// stored (or a permanent rejection just cleared it) — the connect goes out
/// bare and a 401 resolves as needs-auth, the truthful state.
#[must_use]
pub fn connect_bearer(path: Option<&Path>, server_url: &str) -> Option<String> {
    let tokens = load_tokens(path, server_url)?;
    if tokens_fresh(&tokens, unix_now()) {
        return Some(tokens.access_token);
    }
    match refresh_grant(path, server_url) {
        Ok(refreshed) => Some(refreshed.access_token),
        Err(RefreshFailure::Permanent(_)) => None,
        Err(RefreshFailure::NoGrant | RefreshFailure::Transient(_)) => Some(tokens.access_token),
    }
}

/// The proactive pre-call refresh: `Some(new bearer)` only when the stored
/// grant was stale and the refresh succeeded — the caller re-arms the
/// transport with it. `None` otherwise: a fresh grant needs nothing, and a
/// transient failure just proceeds on the old bearer with the reactive 401
/// path as backstop.
#[must_use]
pub fn refresh_if_stale(path: Option<&Path>, server_url: &str) -> Option<String> {
    let tokens = load_tokens(path, server_url)?;
    if tokens_fresh(&tokens, unix_now()) || tokens.refresh_token.is_none() {
        return None;
    }
    refresh_grant(path, server_url)
        .ok()
        .map(|refreshed| refreshed.access_token)
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Fold a token response onto the prior grant (rotating refresh tokens,
/// keeping the client identity). A returned `refresh_token` replaces the old
/// one (RFC 6749 §6's MUST — rotation is mandatory for public clients); an
/// absent one keeps the old, never overwriting a live token with nothing.
fn tokens_from_response(response: &Value, prior: &StoredTokens) -> Option<StoredTokens> {
    let access = response.get("access_token").and_then(Value::as_str)?;
    // `expires_in` is a number per RFC 6749 §5.1, but real ASes send floats
    // and strings too. Absent either way means an unknown lifetime, which
    // stays `None` rather than a fabricated hour: with rotation mandatory
    // for public clients, guessing short refreshes a long-lived token ~24×
    // a day, and every rotation is another chance to lose the grant. The
    // 401 path is the backstop for a lifetime nobody declared.
    let expires_in = response.get("expires_in").and_then(|v| match v {
        Value::Number(n) => n
            .as_u64()
            .or_else(|| n.as_f64().filter(|f| *f >= 0.0).map(|f| f as u64)),
        Value::String(s) => s
            .trim()
            .parse::<f64>()
            .ok()
            .filter(|f| *f >= 0.0)
            .map(|f| f as u64),
        _ => None,
    });
    Some(StoredTokens {
        access_token: access.to_string(),
        refresh_token: response
            .get("refresh_token")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| prior.refresh_token.clone()),
        expires_at: expires_in.map(|secs| unix_now() + secs),
        client_id: prior.client_id.clone(),
        client_secret: prior.client_secret.clone(),
        token_endpoint: prior.token_endpoint.clone(),
        scope: response
            .get("scope")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| prior.scope.clone()),
    })
}

// ---------------------------------------------------------------------------
// pure helpers: encoding, URLs, parses

/// Percent-encode one form value (RFC 3986 unreserved kept).
#[must_use]
pub fn url_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(byte as char);
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// Percent-decode (`+` as space — the form flavour).
#[must_use]
pub fn url_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
                match hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                    Some(byte) => {
                        out.push(byte);
                        i += 2;
                    }
                    None => out.push(b'%'),
                }
            }
            other => out.push(other),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `application/x-www-form-urlencoded` body from pairs.
#[must_use]
pub fn form_encode(pairs: &[(String, String)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| format!("{}={}", url_encode(k), url_encode(v)))
        .collect::<Vec<_>>()
        .join("&")
}

/// One `key="value"` parameter out of a 401's `WWW-Authenticate` challenge.
#[must_use]
pub fn challenge_param(challenge: &str, key: &str) -> Option<String> {
    let lower = challenge.to_ascii_lowercase();
    let at = lower.find(&key.to_ascii_lowercase())?;
    let rest = &challenge[at + key.len()..];
    let rest = rest.trim_start().strip_prefix('=')?.trim_start();
    // A quoted value may contain spaces (a scope list is exactly that), so
    // only an unquoted one ends at whitespace.
    let (rest, terminators): (&str, &[char]) = match rest.strip_prefix('"') {
        Some(rest) => (rest, &['"']),
        None => (rest, &['"', ',', ' ']),
    };
    let end = rest.find(terminators).unwrap_or(rest.len());
    let value = rest[..end].trim();
    (!value.is_empty()).then(|| value.to_string())
}

/// The `resource_metadata` URL a 401's `WWW-Authenticate` challenge names
/// (RFC 9728's discovery seed), if any.
#[must_use]
pub fn challenge_resource_metadata(challenge: &str) -> Option<String> {
    challenge_param(challenge, "resource_metadata")
}

/// The `scope` a 401 challenge asks for (RFC 6750 §3). The spec makes this
/// **authoritative** for the operation that was refused — ahead of the
/// resource's own `scopes_supported` — so a server that wants more than its
/// metadata advertises gets what it actually asked for.
#[must_use]
pub fn challenge_scope(challenge: &str) -> Option<String> {
    challenge_param(challenge, "scope")
}

/// The well-known URL candidates for a resource/AS `base` — path-aware first
/// (RFC 8414's rule when the issuer has a path), then the root, then (for an
/// AS) the OIDC spelling.
#[must_use]
pub fn well_known_candidates(base: &str, suffix: &str) -> Vec<String> {
    let Ok(parsed) = url::Url::parse(base) else {
        return Vec::new();
    };
    let origin = format!(
        "{}://{}",
        parsed.scheme(),
        parsed.host_str().map_or_else(String::new, |host| {
            match parsed.port() {
                Some(port) => format!("{host}:{port}"),
                None => host.to_string(),
            }
        })
    );
    let path = parsed.path().trim_end_matches('/');
    let mut out = Vec::new();
    if !path.is_empty() {
        out.push(format!("{origin}/.well-known/{suffix}{path}"));
    }
    out.push(format!("{origin}/.well-known/{suffix}"));
    out
}

/// Build the authorization URL.
#[must_use]
#[allow(clippy::too_many_arguments)] // a URL is all parameters
pub fn build_authorize_url(
    authorization_endpoint: &str,
    client_id: &str,
    redirect_uri: &str,
    code_challenge: &str,
    state: &str,
    scope: Option<&str>,
    resource: &str,
) -> String {
    let mut params = vec![
        ("response_type".to_string(), "code".to_string()),
        ("client_id".to_string(), client_id.to_string()),
        ("redirect_uri".to_string(), redirect_uri.to_string()),
        ("code_challenge".to_string(), code_challenge.to_string()),
        ("code_challenge_method".to_string(), "S256".to_string()),
        ("state".to_string(), state.to_string()),
        ("resource".to_string(), resource.to_string()),
    ];
    if let Some(scope) = scope.filter(|s| !s.trim().is_empty()) {
        params.push(("scope".to_string(), scope.to_string()));
        // OpenID Connect only releases a refresh token for `offline_access`
        // when consent is actually shown; a silent re-authorization returns
        // an access token alone — the same dead end as never asking.
        if scope.split_whitespace().any(|s| s == OFFLINE_ACCESS) {
            params.push(("prompt".to_string(), "consent".to_string()));
        }
    }
    let query = form_encode(&params);
    let sep = if authorization_endpoint.contains('?') {
        '&'
    } else {
        '?'
    };
    format!("{authorization_endpoint}{sep}{query}")
}

/// One query parameter out of a redirect URL/path/query —
/// [`parse_redirect`]'s extractor generalized, because RFC 9207's `iss`
/// rides beside `code` and the validation wants it by name.
#[must_use]
pub fn redirect_param(text: &str, key: &str) -> Option<String> {
    let text = text.trim();
    let query = text
        .split_once('?')
        .map_or(text, |(_, query)| query)
        .split('#')
        .next()
        .unwrap_or_default();
    query
        .split('&')
        .find_map(|pair| {
            let (k, value) = pair.split_once('=')?;
            (k == key).then(|| url_decode(value))
        })
        .filter(|value| !value.is_empty())
}

/// RFC 9207 issuer identification — the 2026-07-28 authorization spec's
/// MUST: a redirect carrying `iss` must name the authorization server the
/// flow started with, compared byte for byte (the spec forbids case folding,
/// port elision and every other normalization), and an AS whose metadata
/// advertises `authorization_response_iss_parameter_supported` must send it
/// — absence there is a rejection, not a shrug.
pub fn validate_issuer(
    iss: Option<&str>,
    expected: Option<&str>,
    required: bool,
) -> Result<(), String> {
    let Some(expected) = expected else {
        return Ok(());
    };
    match iss {
        Some(iss) if iss == expected => Ok(()),
        Some(iss) => Err(format!(
            "the redirect names a different authorization server (got {iss}, expected {expected})"
        )),
        None if required => Err(
            "the authorization server advertises iss support but the redirect carried no iss"
                .to_string(),
        ),
        None => Ok(()),
    }
}

/// The OIDC scope that actually buys a refresh token.
const OFFLINE_ACCESS: &str = "offline_access";

/// The scope to request, given the resource's own scopes (`base`) and the
/// authorization server's catalogue — SEP-2207.
///
/// `offline_access` is what actually buys a **refresh token**, and the spec
/// deliberately keeps it out of the protected resource's metadata ("refresh
/// tokens are not a resource requirement"), so it can only ever come from
/// the authorization server's `scopes_supported`. Without the union a server
/// like Vercel (resource scope `openid`, AS catalogue including
/// `offline_access`) issues an access token with **nothing to renew it**,
/// and the session dies at expiry with a browser round as the only cure.
///
/// Three guards: never ask for a scope the server doesn't advertise (strict
/// providers answer `invalid_scope` and the whole flow fails), never
/// duplicate it, and never send it *alone* — a bare `offline_access` request
/// is not a resource request.
#[must_use]
pub fn with_offline_access(base: Option<&str>, as_supported: &[String]) -> Option<String> {
    let base = base.map(str::trim).filter(|scope| !scope.is_empty())?;
    let already = base.split_whitespace().any(|s| s == OFFLINE_ACCESS);
    let offered = as_supported.iter().any(|s| s == OFFLINE_ACCESS);
    if already || !offered {
        return Some(base.to_string());
    }
    Some(format!("{base} {OFFLINE_ACCESS}"))
}

/// Parse `code` and `state` out of a pasted redirect URL (or a bare
/// `?code=…` query, or the callback request path).
#[must_use]
pub fn parse_redirect(text: &str) -> Option<(String, Option<String>)> {
    let text = text.trim();
    let query = text
        .split_once('?')
        .map_or(text, |(_, query)| query)
        .split('#')
        .next()
        .unwrap_or_default();
    let mut code = None;
    let mut state = None;
    for pair in query.split('&') {
        let (key, value) = pair.split_once('=')?;
        match key {
            "code" => code = Some(url_decode(value)),
            "state" => state = Some(url_decode(value)),
            _ => {}
        }
    }
    code.filter(|c| !c.is_empty()).map(|c| (c, state))
}

/// The PKCE pair (verifier, S256 challenge) and a `state`, from fresh
/// entropy. The encodes are pure ([`b64url`]); only the byte source is I/O.
#[must_use]
pub fn pkce_and_state() -> (String, String, String) {
    let mut verifier_bytes = [0u8; 48];
    let mut state_bytes = [0u8; 24];
    // Entropy failure is unrecoverable for a security parameter; getrandom
    // only fails on broken platforms.
    let _ = getrandom::fill(&mut verifier_bytes);
    let _ = getrandom::fill(&mut state_bytes);
    let verifier = b64url(&verifier_bytes);
    let challenge = {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(verifier.as_bytes());
        b64url(&hasher.finalize())
    };
    (verifier, challenge, b64url(&state_bytes))
}

/// Base64url (RFC 4648 §5, no padding) — the PKCE alphabet.
#[must_use]
pub fn b64url(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(ALPHABET[(n >> 12) as usize & 63] as char);
        if chunk.len() > 1 {
            out.push(ALPHABET[(n >> 6) as usize & 63] as char);
        }
        if chunk.len() > 2 {
            out.push(ALPHABET[n as usize & 63] as char);
        }
    }
    out
}

// ---------------------------------------------------------------------------
// discovery + registration (HTTP)

/// What discovery resolved: where to send the user, where to trade the code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthServerMeta {
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub registration_endpoint: Option<String>,
    pub scopes: Option<String>,
    /// The AS `issuer` identifier — what RFC 9207's `iss` must equal.
    pub issuer: Option<String>,
    /// `authorization_response_iss_parameter_supported`: an AS that
    /// advertises it must send `iss` on every authorization response, so a
    /// response without one is rejected rather than excused.
    pub iss_required: bool,
}

fn get_json(url: &str) -> Result<Value, String> {
    let client = crate::llm::http_client(HTTP_TIMEOUT).map_err(|e| e.to_string())?;
    let response = client
        .get(url)
        .header("Accept", "application/json")
        .header("MCP-Protocol-Version", crate::mcp::PROTOCOL_VERSION)
        .send()
        .map_err(|e| format!("request failed: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("HTTP {}", response.status().as_u16()));
    }
    response
        .json::<Value>()
        .map_err(|e| format!("bad JSON: {e}"))
}

/// POST a form and hand back `(status, body)` raw — the refresh path
/// classifies the failure itself (`Err` only when no answer arrived at all).
fn post_form_raw(url: &str, params: &[(String, String)]) -> Result<(u16, String), String> {
    let client = crate::llm::http_client(HTTP_TIMEOUT).map_err(|e| e.to_string())?;
    let response = client
        .post(url)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .header("Accept", "application/json")
        .body(form_encode(params))
        .send()
        .map_err(|e| format!("request failed: {e}"))?;
    let status = response.status().as_u16();
    Ok((status, response.text().unwrap_or_default()))
}

fn post_form(url: &str, params: &[(String, String)]) -> Result<Value, String> {
    let (status, body) = post_form_raw(url, params)?;
    if !(200..300).contains(&status) {
        let flat: String = body.split_whitespace().collect::<Vec<_>>().join(" ");
        let flat: String = flat.chars().take(200).collect();
        return Err(format!("HTTP {status}: {flat}"));
    }
    serde_json::from_str(&body).map_err(|e| format!("bad JSON: {e}"))
}

/// Resolve the authorization server for `server_url`, seeded by the 401's
/// challenge when one was captured.
pub fn discover(server_url: &str, challenge: Option<&str>) -> Result<AuthServerMeta, String> {
    // 1. Protected-resource metadata: the challenge's named URL first, then
    //    the well-known candidates on the server itself.
    let mut resource_meta: Option<Value> = None;
    let mut candidates: Vec<String> = Vec::new();
    if let Some(named) = challenge.and_then(challenge_resource_metadata) {
        candidates.push(named);
    }
    candidates.extend(well_known_candidates(
        server_url,
        "oauth-protected-resource",
    ));
    for candidate in candidates {
        if let Ok(value) = get_json(&candidate) {
            resource_meta = Some(value);
            break;
        }
    }
    // The challenge's own `scope` outranks the resource's metadata: the spec
    // makes it authoritative for the operation that was refused, so a server
    // wanting more than it advertises gets what it actually asked for.
    let challenged_scopes = challenge.and_then(challenge_scope);
    let (auth_server, resource_scopes) = match &resource_meta {
        Some(meta) => (
            meta.get("authorization_servers")
                .and_then(Value::as_array)
                .and_then(|list| list.first())
                .and_then(Value::as_str)
                .map(str::to_string),
            meta.get("scopes_supported")
                .and_then(Value::as_array)
                .map(|scopes| {
                    scopes
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .filter(|s| !s.is_empty()),
        ),
        None => (None, None),
    };
    // 2. AS metadata: the named server, else the MCP server's own origin.
    let auth_base = auth_server.unwrap_or_else(|| origin_of(server_url));
    let mut as_candidates = well_known_candidates(&auth_base, "oauth-authorization-server");
    as_candidates.extend(well_known_candidates(&auth_base, "openid-configuration"));
    let mut last_err = String::from("no authorization-server metadata found");
    for candidate in as_candidates {
        match get_json(&candidate) {
            Ok(meta) => {
                let field = |key: &str| meta.get(key).and_then(Value::as_str).map(str::to_string);
                let (Some(authorization_endpoint), Some(token_endpoint)) =
                    (field("authorization_endpoint"), field("token_endpoint"))
                else {
                    last_err = format!("{candidate}: missing endpoints");
                    continue;
                };
                let as_scopes: Vec<String> = meta
                    .get("scopes_supported")
                    .and_then(Value::as_array)
                    .map(|scopes| {
                        scopes
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default();
                // SEP-2207: the *resource's* scopes, plus `offline_access`
                // when this authorization server advertises it — the only
                // way a refresh token is ever issued, and the reason an
                // authenticated server stops needing the browser every hour.
                // Deliberately never the AS's whole catalogue as a fallback:
                // asking for scopes the resource never wanted is what strict
                // providers answer with `invalid_scope`, failing the flow
                // outright.
                let base = challenged_scopes.as_deref().or(resource_scopes.as_deref());
                let scopes = with_offline_access(base, &as_scopes);
                return Ok(AuthServerMeta {
                    authorization_endpoint,
                    token_endpoint,
                    registration_endpoint: field("registration_endpoint"),
                    scopes,
                    issuer: field("issuer"),
                    iss_required: meta
                        .get("authorization_response_iss_parameter_supported")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                });
            }
            Err(e) => last_err = format!("{candidate}: {e}"),
        }
    }
    Err(format!(
        "could not discover the authorization server ({last_err})"
    ))
}

fn origin_of(url_text: &str) -> String {
    url::Url::parse(url_text)
        .ok()
        .and_then(|u| {
            let host = u.host_str()?.to_string();
            Some(match u.port() {
                Some(port) => format!("{}://{host}:{port}", u.scheme()),
                None => format!("{}://{host}", u.scheme()),
            })
        })
        .unwrap_or_else(|| url_text.to_string())
}

/// Dynamically register a client (RFC 7591). Returns `(client_id,
/// client_secret)`.
pub fn register_client(
    registration_endpoint: &str,
    redirect_uri: &str,
) -> Result<(String, Option<String>), String> {
    let client = crate::llm::http_client(HTTP_TIMEOUT).map_err(|e| e.to_string())?;
    let body = serde_json::json!({
        "client_name": "alter-zero",
        "redirect_uris": [redirect_uri],
        "grant_types": ["authorization_code", "refresh_token"],
        "response_types": ["code"],
        "token_endpoint_auth_method": "none",
        // 2026-07-28 (SEP-837): a registration MUST name its application
        // type — a CLI with a loopback redirect is a native app, and the
        // OIDC default of "web" rejects exactly that redirect.
        "application_type": "native",
    });
    let response = client
        .post(registration_endpoint)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .body(body.to_string())
        .send()
        .map_err(|e| format!("registration failed: {e}"))?;
    let status = response.status();
    let body = response.text().unwrap_or_default();
    if !status.is_success() {
        return Err(format!("registration failed: HTTP {}", status.as_u16()));
    }
    let value: Value =
        serde_json::from_str(&body).map_err(|e| format!("registration: bad JSON: {e}"))?;
    let client_id = value
        .get("client_id")
        .and_then(Value::as_str)
        .ok_or_else(|| "registration returned no client_id".to_string())?
        .to_string();
    Ok((
        client_id,
        value
            .get("client_secret")
            .and_then(Value::as_str)
            .map(str::to_string),
    ))
}

// ---------------------------------------------------------------------------
// the interactive flow

/// What the flow reports as it goes — the manager forwards these onto the
/// MCP event channel for the auth page.
pub trait AuthProgress: Send {
    /// The authorize URL is ready (shown + copied from the auth page).
    fn on_url(&self, url: &str);
}

/// Run the whole interactive flow for `server_url`, blocking this (worker)
/// thread: discovery → registration → browser/paste → exchange → persist.
/// `paste_rx` delivers the user's pasted redirect URL; `open_browser` is the
/// boundary's launcher (injected so tests need no display).
pub fn run_auth_flow(
    server_url: &str,
    challenge: Option<&str>,
    store_path: Option<&Path>,
    paste_rx: &mpsc::Receiver<String>,
    progress: &dyn AuthProgress,
    open_browser: &dyn Fn(&str),
    cancel: &CancelToken,
) -> Result<(), String> {
    let meta = discover(server_url, challenge)?;
    // The loopback listener binds first (port 0 = ephemeral) so registration
    // can name the exact redirect URI.
    let listener = TcpListener::bind("127.0.0.1:0")
        .map_err(|e| format!("could not open the callback port: {e}"))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("callback port: {e}"))?
        .port();
    let redirect_uri = format!("http://localhost:{port}/callback");
    // A client identity: reuse the stored one (its registration named an
    // older port-specific redirect, but ASes match the URI from the request,
    // and loopback URIs are exempt from exact matching per RFC 8252 —
    // re-register when the server insists).
    let stored = load_tokens(store_path, server_url);
    let (client_id, client_secret) = match stored
        .as_ref()
        .filter(|t| !t.client_id.is_empty())
        .map(|t| (t.client_id.clone(), t.client_secret.clone()))
    {
        Some(identity) => identity,
        None => match &meta.registration_endpoint {
            Some(endpoint) => register_client(endpoint, &redirect_uri)?,
            None => {
                return Err(
                    "the server offers no client registration; add a pre-registered client".into(),
                );
            }
        },
    };
    let (verifier, code_challenge, state) = pkce_and_state();
    let authorize_url = build_authorize_url(
        &meta.authorization_endpoint,
        &client_id,
        &redirect_uri,
        &code_challenge,
        &state,
        meta.scopes.as_deref(),
        server_url,
    );
    progress.on_url(&authorize_url);
    open_browser(&authorize_url);
    let code = wait_for_code(&listener, &state, &meta, paste_rx, cancel)?;
    let mut params = vec![
        ("grant_type".to_string(), "authorization_code".to_string()),
        ("code".to_string(), code),
        ("redirect_uri".to_string(), redirect_uri),
        ("client_id".to_string(), client_id.clone()),
        ("code_verifier".to_string(), verifier),
        ("resource".to_string(), server_url.to_string()),
    ];
    if let Some(secret) = &client_secret {
        params.push(("client_secret".to_string(), secret.clone()));
    }
    let response = post_form(&meta.token_endpoint, &params)
        .map_err(|e| format!("token exchange failed: {e}"))?;
    let prior = StoredTokens {
        client_id,
        client_secret,
        token_endpoint: meta.token_endpoint.clone(),
        scope: meta.scopes.clone(),
        ..Default::default()
    };
    let tokens = tokens_from_response(&response, &prior)
        .ok_or_else(|| "token response carried no access_token".to_string())?;
    save_tokens(store_path, server_url, Some(&tokens));
    Ok(())
}

/// Wait for the authorization code from **either** side: the loopback
/// callback, or a pasted redirect URL. Validates `state` when the redirect
/// carries one, and the RFC 9207 `iss` against the discovered issuer before
/// the code is ever redeemed.
fn wait_for_code(
    listener: &TcpListener,
    state: &str,
    meta: &AuthServerMeta,
    paste_rx: &mpsc::Receiver<String>,
    cancel: &CancelToken,
) -> Result<String, String> {
    listener
        .set_nonblocking(true)
        .map_err(|e| format!("callback port: {e}"))?;
    let started = Instant::now();
    loop {
        if cancel.is_cancelled() {
            return Err("cancelled".to_string());
        }
        if started.elapsed() > AUTH_FLOW_TIMEOUT {
            return Err("timed out waiting for the browser callback".to_string());
        }
        // The pasted redirect URL — the headless/remote path.
        match paste_rx.try_recv() {
            Ok(text) => {
                if let Some((code, got_state)) = parse_redirect(&text) {
                    if got_state.as_deref().is_some_and(|got| got != state) {
                        return Err("the pasted URL's state doesn't match".to_string());
                    }
                    validate_issuer(
                        redirect_param(&text, "iss").as_deref(),
                        meta.issuer.as_deref(),
                        meta.iss_required,
                    )?;
                    return Ok(code);
                }
                // Not parseable — keep waiting; the page shows the format.
            }
            Err(mpsc::TryRecvError::Empty | mpsc::TryRecvError::Disconnected) => {}
        }
        match listener.accept() {
            Ok((mut stream, _)) => {
                let mut buffer = [0u8; 4096];
                let read = stream.read(&mut buffer).unwrap_or(0);
                let request = String::from_utf8_lossy(&buffer[..read]);
                let path = request
                    .lines()
                    .next()
                    .and_then(|line| line.split_whitespace().nth(1))
                    .unwrap_or_default();
                let parsed = parse_redirect(path);
                let ok = parsed.is_some();
                let body = if ok {
                    "<html><body><h3>Authentication complete.</h3>You can close this tab and return to the terminal.</body></html>"
                } else {
                    "<html><body><h3>Waiting for authentication…</h3></body></html>"
                };
                let _ = stream.write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                    .as_bytes(),
                );
                if let Some((code, got_state)) = parsed {
                    if got_state.as_deref().is_some_and(|got| got != state) {
                        return Err("the callback's state doesn't match".to_string());
                    }
                    validate_issuer(
                        redirect_param(path, "iss").as_deref(),
                        meta.issuer.as_deref(),
                        meta.iss_required,
                    )?;
                    return Ok(code);
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(POLL);
            }
            Err(e) => return Err(format!("callback port: {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn b64url_matches_the_rfc_vectors() {
        assert_eq!(b64url(b""), "");
        assert_eq!(b64url(b"f"), "Zg");
        assert_eq!(b64url(b"fo"), "Zm8");
        assert_eq!(b64url(b"foo"), "Zm9v");
        assert_eq!(b64url(&[0xfb, 0xff]), "-_8");
    }

    #[test]
    fn pkce_challenge_is_the_s256_of_the_verifier() {
        let (verifier, challenge, state) = pkce_and_state();
        assert!(verifier.len() >= 43, "verifier long enough for PKCE");
        assert!(!state.is_empty());
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(verifier.as_bytes());
        assert_eq!(challenge, b64url(&hasher.finalize()));
    }

    #[test]
    fn url_encoding_round_trips() {
        assert_eq!(url_encode("a b&c"), "a%20b%26c");
        assert_eq!(url_decode("a%20b%26c"), "a b&c");
        assert_eq!(url_decode("a+b"), "a b");
        assert_eq!(
            form_encode(&[("k".to_string(), "v v".to_string())]),
            "k=v%20v"
        );
    }

    #[test]
    fn challenge_resource_metadata_parses_the_header() {
        assert_eq!(
            challenge_resource_metadata(
                r#"Bearer resource_metadata="https://x/.well-known/oauth-protected-resource", error="unauthorized""#
            )
            .as_deref(),
            Some("https://x/.well-known/oauth-protected-resource")
        );
        assert_eq!(challenge_resource_metadata("Bearer realm=\"x\""), None);
    }

    #[test]
    fn well_known_candidates_are_path_aware() {
        assert_eq!(
            well_known_candidates("https://x.test/mcp", "oauth-protected-resource"),
            [
                "https://x.test/.well-known/oauth-protected-resource/mcp",
                "https://x.test/.well-known/oauth-protected-resource",
            ]
        );
        assert_eq!(
            well_known_candidates("https://x.test", "oauth-authorization-server"),
            ["https://x.test/.well-known/oauth-authorization-server"]
        );
        assert_eq!(
            well_known_candidates("https://x.test:8443/a", "openid-configuration"),
            [
                "https://x.test:8443/.well-known/openid-configuration/a",
                "https://x.test:8443/.well-known/openid-configuration",
            ]
        );
    }

    #[test]
    fn authorize_url_carries_the_grant_parameters() {
        let url = build_authorize_url(
            "https://as.test/authorize",
            "client-1",
            "http://localhost:3999/callback",
            "CHAL",
            "STATE",
            Some("read write"),
            "https://mcp.test/mcp",
        );
        assert!(url.starts_with("https://as.test/authorize?"));
        for needle in [
            "response_type=code",
            "client_id=client-1",
            "code_challenge=CHAL",
            "code_challenge_method=S256",
            "state=STATE",
            "scope=read%20write",
            "resource=https%3A%2F%2Fmcp.test%2Fmcp",
        ] {
            assert!(url.contains(needle), "{url} missing {needle}");
        }
        // An endpoint that already has a query appends with `&`.
        let url = build_authorize_url("https://as.test/a?x=1", "c", "r", "ch", "s", None, "res");
        assert!(url.contains("?x=1&response_type=code"));
        assert!(!url.contains("scope="));
    }

    #[test]
    fn redirect_parses_from_urls_paths_and_queries() {
        assert_eq!(
            parse_redirect("http://localhost:1/callback?code=abc&state=s1"),
            Some(("abc".to_string(), Some("s1".to_string())))
        );
        assert_eq!(
            parse_redirect("/callback?code=a%2Fb"),
            Some(("a/b".to_string(), None))
        );
        assert_eq!(
            parse_redirect("code=xyz&state=q"),
            Some(("xyz".to_string(), Some("q".to_string())))
        );
        assert_eq!(parse_redirect("https://x/cb?error=denied"), None);
        assert_eq!(parse_redirect("plain text"), None);
    }

    #[test]
    fn a_challenge_yields_its_metadata_url_and_its_scope() {
        // Discovery runs blind without these: the server publishes its
        // resource metadata off the well-known path and names the scope it
        // actually wants, and both live in the 401 we used to throw away.
        let challenge = r#"Bearer error="invalid_token", resource_metadata="https://x.test/.well-known/oauth-protected-resource/mcp", scope="read:wiki write:wiki""#;
        assert_eq!(
            challenge_resource_metadata(challenge).as_deref(),
            Some("https://x.test/.well-known/oauth-protected-resource/mcp")
        );
        // A quoted scope list contains spaces — it must not end at the first.
        assert_eq!(
            challenge_scope(challenge).as_deref(),
            Some("read:wiki write:wiki")
        );
        // An unquoted value still ends at whitespace or a comma.
        assert_eq!(
            challenge_scope("Bearer scope=read, realm=x").as_deref(),
            Some("read")
        );
        assert_eq!(challenge_scope("Bearer realm=\"x\""), None);
    }

    #[test]
    fn refresh_params_shape_the_grant() {
        let tokens = StoredTokens {
            access_token: "at".to_string(),
            refresh_token: Some("rt-1".to_string()),
            expires_at: Some(1),
            client_id: "cid".to_string(),
            client_secret: None,
            token_endpoint: "https://as/token".to_string(),
            scope: Some("read write".to_string()),
        };
        let params = refresh_params(&tokens, "https://mcp.test/mcp");
        let get = |key: &str| {
            params
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.as_str())
        };
        assert_eq!(get("grant_type"), Some("refresh_token"));
        assert_eq!(get("refresh_token"), Some("rt-1"));
        assert_eq!(get("client_id"), Some("cid"));
        // RFC 8707: the resource indicator rides every token request, the
        // refresh included — omitting it mints a token the server rejects.
        assert_eq!(get("resource"), Some("https://mcp.test/mcp"));
        // No scope (it could only narrow the grant), no PKCE verifier (the
        // refresh grant has none), no absent secret.
        assert_eq!(get("scope"), None);
        assert_eq!(get("code_verifier"), None);
        assert_eq!(get("client_secret"), None);
        // A confidential client authenticates with its secret.
        let confidential = StoredTokens {
            client_secret: Some("shh".to_string()),
            ..tokens
        };
        let params = refresh_params(&confidential, "https://mcp.test/mcp");
        assert!(
            params
                .iter()
                .any(|(k, v)| k == "client_secret" && v == "shh")
        );
    }

    #[test]
    fn refresh_failures_classify_permanent_vs_transient() {
        use RefreshFailure as F;
        // The AS rejecting the grant itself is permanent — the refresh token
        // is dead and retrying only loops the failure.
        for error in [
            "invalid_grant",
            "invalid_client",
            "invalid_request",
            "unauthorized_client",
            "invalid_scope",
        ] {
            let body = format!(r#"{{"error":"{error}","error_description":"no"}}"#);
            match refresh_failure_kind(Some(400), &body) {
                F::Permanent(detail) => assert!(detail.contains(error), "{detail}"),
                other => panic!("{error} classified {other:?}"),
            }
        }
        assert!(matches!(
            refresh_failure_kind(Some(401), r#"{"error":"invalid_client"}"#),
            F::Permanent(_)
        ));
        // Everything else is transient: the grant is kept and the refresh
        // retried later (flagging needs-auth on a network blip was the bug).
        assert!(matches!(
            refresh_failure_kind(Some(503), "upstream down"),
            F::Transient(_)
        ));
        assert!(matches!(
            refresh_failure_kind(Some(429), ""),
            F::Transient(_)
        ));
        assert!(matches!(
            refresh_failure_kind(Some(400), "<html>proxy error</html>"),
            F::Transient(_)
        ));
        assert!(matches!(
            refresh_failure_kind(None, "connect timeout"),
            F::Transient(_)
        ));
    }

    #[test]
    fn offline_access_rides_only_an_as_that_offers_it() {
        // SEP-2207: `offline_access` is what actually buys a refresh token,
        // and the spec keeps it out of the *resource's* metadata ("refresh
        // tokens are not a resource requirement"), so it can only come from
        // the authorization server's catalogue. Vercel is exactly this shape
        // — resource `openid`, AS `offline_access` — and without the union
        // its grant has nothing to renew.
        let offered = ["openid".to_string(), "offline_access".to_string()];
        let bare = ["read".to_string()];
        assert_eq!(
            with_offline_access(Some("read write"), &offered).as_deref(),
            Some("read write offline_access")
        );
        assert_eq!(
            with_offline_access(Some("read"), &bare).as_deref(),
            Some("read")
        );
        // Already granted: never doubled.
        assert_eq!(
            with_offline_access(Some("offline_access read"), &offered).as_deref(),
            Some("offline_access read")
        );
        // Never *alone*: a bare `offline_access` is not a resource request,
        // so no resource scope means no scope parameter at all.
        assert_eq!(with_offline_access(None, &offered), None);
        assert_eq!(with_offline_access(Some("  "), &offered), None);
    }

    #[test]
    fn the_authorize_url_asks_for_consent_only_when_it_wants_a_refresh_token() {
        let url = |scope: Option<&str>| {
            build_authorize_url(
                "https://as.test/authorize",
                "cid",
                "http://127.0.0.1:1/cb",
                "chal",
                "st",
                scope,
                "https://mcp.test/mcp",
            )
        };
        // OIDC releases a refresh token for `offline_access` only when
        // consent is actually shown — a silent re-authorization returns an
        // access token alone, the same dead end as never asking.
        assert!(url(Some("openid offline_access")).contains("prompt=consent"));
        // Nothing to consent to otherwise: don't force a click.
        assert!(!url(Some("openid")).contains("prompt="));
        assert!(!url(None).contains("prompt="));
        assert!(!url(None).contains("scope="));
    }

    #[test]
    fn redirect_params_extract_iss() {
        assert_eq!(
            redirect_param("https://x/cb?code=a&iss=https%3A%2F%2Fas.test", "iss").as_deref(),
            Some("https://as.test")
        );
        assert_eq!(redirect_param("https://x/cb?code=a", "iss"), None);
    }

    #[test]
    fn issuer_validation_follows_rfc_9207() {
        // No recorded issuer: nothing to validate against.
        assert!(validate_issuer(Some("https://as"), None, false).is_ok());
        assert!(validate_issuer(None, None, true).is_ok());
        // A present iss must equal the recorded issuer byte for byte — no
        // normalization (the spec forbids case folding and friends).
        assert!(validate_issuer(Some("https://as.test"), Some("https://as.test"), false).is_ok());
        assert!(validate_issuer(Some("https://AS.test"), Some("https://as.test"), false).is_err());
        assert!(validate_issuer(Some("https://evil"), Some("https://as.test"), true).is_err());
        // An AS that advertises iss support must send it — absence is a
        // rejection when the metadata promised the parameter.
        assert!(validate_issuer(None, Some("https://as.test"), true).is_err());
        assert!(validate_issuer(None, Some("https://as.test"), false).is_ok());
    }

    #[test]
    fn concurrent_saves_never_drop_a_rotation() {
        // The store is one file for every server, so two servers renewing at
        // once (two subagents, or a tool call while the loop reconnects
        // another) interleave read/read/write/write and the second write
        // clobbers the first's rotation. The loser then presents an
        // already-used refresh token, which a spec-compliant AS treats as a
        // replay and answers by revoking the whole family — the browser flow
        // again, which is the bug this whole file exists to prevent.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcp-auth.json");
        let handles: Vec<_> = (0..16)
            .map(|i| {
                let path = path.clone();
                std::thread::spawn(move || {
                    let tokens = StoredTokens {
                        access_token: format!("at-{i}"),
                        refresh_token: Some(format!("rt-{i}")),
                        expires_at: Some(999),
                        client_id: "cid".to_string(),
                        client_secret: None,
                        token_endpoint: "https://as/token".to_string(),
                        scope: None,
                    };
                    save_tokens(
                        Some(&path),
                        &format!("https://s{i}.test/mcp"),
                        Some(&tokens),
                    );
                })
            })
            .collect();
        for handle in handles {
            handle.join().expect("save thread");
        }
        for i in 0..16 {
            let stored = load_tokens(Some(&path), &format!("https://s{i}.test/mcp"))
                .unwrap_or_else(|| panic!("server {i}'s grant survived the concurrent writes"));
            assert_eq!(stored.access_token, format!("at-{i}"));
            assert_eq!(stored.refresh_token.as_deref(), Some(&*format!("rt-{i}")));
        }
    }

    #[test]
    fn token_store_round_trips_and_deletes() {
        let tokens = StoredTokens {
            access_token: "at".to_string(),
            refresh_token: Some("rt".to_string()),
            expires_at: Some(1000),
            client_id: "cid".to_string(),
            client_secret: None,
            token_endpoint: "https://as/token".to_string(),
            scope: Some("read".to_string()),
        };
        let written = record_tokens("", "https://mcp.test/mcp", Some(&tokens));
        let parsed = parse_token_store(&written);
        assert_eq!(parsed.get("https://mcp.test/mcp"), Some(&tokens));
        // Another server's entry is preserved by the RMW.
        let two = record_tokens(&written, "https://other/mcp", Some(&tokens));
        assert_eq!(parse_token_store(&two).len(), 2);
        let deleted = record_tokens(&two, "https://mcp.test/mcp", None);
        let parsed = parse_token_store(&deleted);
        assert!(!parsed.contains_key("https://mcp.test/mcp"));
        assert!(parsed.contains_key("https://other/mcp"));
    }

    #[test]
    fn freshness_honours_the_skew() {
        let tokens = StoredTokens {
            expires_at: Some(1000),
            ..Default::default()
        };
        assert!(tokens_fresh(&tokens, 1000 - EXPIRY_SKEW_SECS - 1));
        assert!(!tokens_fresh(&tokens, 1000 - EXPIRY_SKEW_SECS));
        let no_expiry = StoredTokens::default();
        assert!(tokens_fresh(&no_expiry, u64::MAX - EXPIRY_SKEW_SECS - 1));
    }
}
