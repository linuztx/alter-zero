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

use crate::mcp::McpOAuthConfig;
use crate::stream::CancelToken;

/// The whole interactive flow's budget — Claude Code's 5 minutes.
pub const AUTH_FLOW_TIMEOUT: Duration = Duration::from_secs(300);

/// The wait-loop poll cadence (the transport's).
const POLL: Duration = Duration::from_millis(50);

/// Discovery/token requests' per-operation deadline.
const HTTP_TIMEOUT: Duration = Duration::from_secs(20);

/// [`offers_oauth`]'s per-lookup deadline — a question asked of every
/// remote server that connects bare, so a server that never answers it
/// costs seconds, not the flow's twenty.
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Serializes the token store's read-modify-write. The store is **one file
/// for every server**, so without this two servers renewing at once (two
/// subagents, or a tool call while the loop reconnects another) interleave
/// read/read/write/write and drop one of the two rotated refresh tokens.
/// The loser then presents an already-used token, which a spec-compliant
/// authorization server treats as a replay and answers by revoking the whole
/// family — the browser flow again, for the exact reason the refresh exists.
static STORE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// One refresh lock per server. Serializing the store's *write* does not
/// stop two threads presenting the same refresh token to the **network** —
/// and that is the more expensive race: OAuth 2.1 §4.3.1 requires a
/// rotating server to detect reuse and revoke the **whole family**, so the
/// second presentation doesn't just fail, it destroys the grant permanently
/// and only the browser brings it back. Two subagents calling one MCP
/// server share the manager, so this is reachable in ordinary use.
///
/// Per server rather than global: a slow authorization server must not hold
/// up an unrelated server's renewal.
#[allow(clippy::type_complexity)]
static REFRESH_LOCKS: std::sync::Mutex<
    std::collections::BTreeMap<String, std::sync::Arc<std::sync::Mutex<()>>>,
> = std::sync::Mutex::new(std::collections::BTreeMap::new());

/// The refresh lock for one server, minted on first use.
fn refresh_lock_for(server_url: &str) -> std::sync::Arc<std::sync::Mutex<()>> {
    let mut locks = REFRESH_LOCKS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    std::sync::Arc::clone(locks.entry(server_url.to_string()).or_default())
}

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
    /// The RFC 8707 `resource` indicator the grant was issued for — sent
    /// again on every refresh, verbatim. `None` on a grant stored before it
    /// was recorded, which refreshes with the server URL as it always did.
    pub resource: Option<String>,
    /// The authorization server's `issuer` — what a stored client identity
    /// is bound to: credentials from one authorization server are never
    /// presented to another (2026-07-28, SEP-2352).
    pub issuer: Option<String>,
    /// How the client authenticates at the token endpoint
    /// (`client_secret_basic`, `client_secret_post`, `none`). `None` on an
    /// older grant: the secret, if any, rides the body as it always did.
    pub auth_method: Option<String>,
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
                    resource: text("resource"),
                    issuer: text("issuer"),
                    auth_method: text("auth_method"),
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
                for (key, value) in [
                    ("scope", &tokens.scope),
                    ("resource", &tokens.resource),
                    ("issuer", &tokens.issuer),
                    ("auth_method", &tokens.auth_method),
                ] {
                    if let Some(value) = value {
                        entry.insert(key.into(), value.clone().into());
                    }
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

/// The refresh grant's request (RFC 6749 §6 + RFC 8707): the form body and,
/// for a `client_secret_basic` client, the `Authorization` header. No
/// `scope` — it could only narrow the grant, and some ASes narrow it
/// permanently — and no PKCE, which belongs to the authorization-code grant
/// alone. The `resource` indicator rides every token request, the refresh
/// included, and it is the one the grant was **issued** for: omitting it
/// mints a token audience-bound to nothing, which the MCP server then 401s —
/// the classic silent refresh loop. The client authenticates the way it did
/// when the grant was issued.
#[must_use]
pub fn refresh_params(
    tokens: &StoredTokens,
    server_url: &str,
) -> (Vec<(String, String)>, Option<String>) {
    let mut params = vec![
        ("grant_type".to_string(), "refresh_token".to_string()),
        (
            "refresh_token".to_string(),
            tokens.refresh_token.clone().unwrap_or_default(),
        ),
    ];
    let auth = stored_token_auth(tokens);
    let (credentials, header) =
        client_credentials(auth, &tokens.client_id, tokens.client_secret.as_deref());
    params.extend(credentials);
    params.push((
        "resource".to_string(),
        tokens
            .resource
            .clone()
            .unwrap_or_else(|| server_url.to_string()),
    ));
    (params, header)
}

/// How a stored grant's client authenticates: the method recorded with it,
/// else what grants did before the method was recorded — the secret in the
/// body when there is one.
fn stored_token_auth(tokens: &StoredTokens) -> TokenAuth {
    tokens
        .auth_method
        .as_deref()
        .and_then(TokenAuth::from_name)
        .unwrap_or(if tokens.client_secret.is_some() {
            TokenAuth::ClientSecretPost
        } else {
            TokenAuth::None
        })
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
    presented: Option<&str>,
) -> Result<StoredTokens, RefreshFailure> {
    // Single-flight, per server: only one thread may present this grant's
    // refresh token to the network at a time.
    let lock = refresh_lock_for(server_url);
    let _guard = lock
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(tokens) = load_tokens(path, server_url) else {
        return Err(RefreshFailure::NoGrant);
    };
    // Re-read under the lock decides it: if the store no longer holds the
    // access token we found unusable, another thread already rotated the
    // grant while we waited. Take its result — presenting our now-spent copy
    // of the refresh token would be a replay, and a compliant server answers
    // that by revoking the whole family.
    if let Some(presented) = presented
        && tokens.access_token != presented
    {
        return Ok(tokens);
    }
    if tokens.refresh_token.is_none() || tokens.token_endpoint.trim().is_empty() {
        return Err(RefreshFailure::NoGrant);
    }
    let (params, authorization) = refresh_params(&tokens, server_url);
    let (status, body) =
        match post_form_raw(&tokens.token_endpoint, &params, authorization.as_deref()) {
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
    match refresh_grant(path, server_url, Some(&tokens.access_token)) {
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
    refresh_grant(path, server_url, Some(&tokens.access_token))
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
        resource: prior.resource.clone(),
        issuer: prior.issuer.clone(),
        auth_method: prior.auth_method.clone(),
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

/// One `key=value` auth-param out of a `WWW-Authenticate` challenge
/// (RFC 7235 §2.1): the scheme skipped, then `name=value` pairs split on
/// commas, a value a token or a quoted string (`\"` escapes honoured, and
/// spaces kept — a scope list is exactly that). Matched by **name**: a key
/// found inside another parameter's value is not that parameter, which is
/// what the step-up challenge `error="insufficient_scope", scope="…"`
/// needs of it.
#[must_use]
pub fn challenge_param(challenge: &str, key: &str) -> Option<String> {
    let mut rest = challenge.trim();
    if let Some((scheme, params)) = rest.split_once(char::is_whitespace)
        && !scheme.contains('=')
    {
        rest = params;
    }
    loop {
        rest = rest.trim_start_matches(|c: char| c == ',' || c.is_whitespace());
        let eq = rest.find('=')?;
        let name = rest[..eq].trim();
        let after = rest[eq + 1..].trim_start();
        let value = match after.strip_prefix('"') {
            Some(quoted) => {
                let mut value = String::new();
                let mut escaped = false;
                let mut end = quoted.len();
                for (at, c) in quoted.char_indices() {
                    match c {
                        _ if escaped => {
                            value.push(c);
                            escaped = false;
                        }
                        '\\' => escaped = true,
                        '"' => {
                            end = at + 1;
                            break;
                        }
                        _ => value.push(c),
                    }
                }
                rest = &quoted[end.min(quoted.len())..];
                value
            }
            None => {
                let end = after
                    .find(|c: char| c == ',' || c.is_whitespace())
                    .unwrap_or(after.len());
                rest = &after[end..];
                after[..end].to_string()
            }
        };
        if name.eq_ignore_ascii_case(key) {
            let value = value.trim();
            return (!value.is_empty()).then(|| value.to_string());
        }
    }
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

/// Build the authorization URL. `extra` is the authorization server's own
/// parameters ([`issuer_extras`]); a key it shares with one set here (only
/// `prompt` can) is sent once.
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
    extra: &[(String, String)],
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
    for (key, value) in extra {
        if !params.iter().any(|(k, _)| k == key) {
            params.push((key.clone(), value.clone()));
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

/// The scope to request — the 2026-07-28 rules, in order (`docs/mcp.md`):
///
/// 1. The user's own `oauth.scopes` wins outright, verbatim — Claude Code's
///    rule for the same key, and the only way to ask a server for less than
///    everything its metadata lists (Google's lists every Calendar scope,
///    the broadest included).
/// 2. Else the 401 challenge's `scope`, which the spec makes authoritative
///    for the operation refused — **unioned with the scope the stored grant
///    was requested with** (SEP-2350's step-up rule, so re-authorizing for
///    one tool never loses another's permission).
/// 3. Else the protected resource's `scopes_supported`; none at all means no
///    `scope` parameter.
///
/// Then `offline_access` when the authorization server advertises it
/// ([`with_offline_access`]), never on the user's own list — they named
/// exactly what they wanted.
#[must_use]
pub fn choose_scope(
    configured: Option<&str>,
    challenged: Option<&str>,
    resource: Option<&str>,
    previous: Option<&str>,
    as_supported: &[String],
) -> Option<String> {
    let words = |text: Option<&str>| -> Vec<String> {
        text.map(|t| t.split_whitespace().map(str::to_string).collect())
            .unwrap_or_default()
    };
    let configured = words(configured);
    if !configured.is_empty() {
        return Some(configured.join(" "));
    }
    let mut scope = match words(challenged) {
        challenge if !challenge.is_empty() => {
            let mut union = words(previous);
            for word in challenge {
                if !union.contains(&word) {
                    union.push(word);
                }
            }
            union
        }
        _ => words(resource),
    };
    scope.dedup();
    with_offline_access(Some(&scope.join(" ")), as_supported)
}

/// Google's authorization server — the one issuer whose quirks
/// [`issuer_extras`] knows.
const GOOGLE_ISSUER: &str = "https://accounts.google.com";

/// The authorize-request parameters an authorization server needs beyond
/// the standard ones before it issues a **renewable** grant.
///
/// Google issues a refresh token only when the request carries
/// `access_type=offline` — it rejects the `offline_access` scope outright
/// (`invalid_scope`) and does not list it, so [`with_offline_access`]
/// rightly never adds it there — and issues a *new* one to a user who
/// already consented only with `prompt=consent`. Without both, every Google
/// MCP sign-in (Calendar, Gmail, Drive, …) lasts the access token's hour.
#[must_use]
pub fn issuer_extras(issuer: Option<&str>) -> Vec<(String, String)> {
    match issuer {
        Some(issuer) if issuer.trim_end_matches('/') == GOOGLE_ISSUER => vec![
            ("access_type".to_string(), "offline".to_string()),
            ("prompt".to_string(), "consent".to_string()),
        ],
        _ => Vec::new(),
    }
}

/// How a client authenticates at the token endpoint — RFC 7591's
/// `token_endpoint_auth_method` names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenAuth {
    /// A public client: `client_id` in the body, no secret.
    None,
    /// The secret in the form body.
    ClientSecretPost,
    /// The id and secret as HTTP Basic credentials (RFC 6749 §2.3.1).
    ClientSecretBasic,
}

impl TokenAuth {
    /// The method's registered name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::ClientSecretPost => "client_secret_post",
            Self::ClientSecretBasic => "client_secret_basic",
        }
    }

    /// A registered name back to the method — `None` for one this client
    /// does not implement (`private_key_jwt`, …).
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "none" => Some(Self::None),
            "client_secret_post" => Some(Self::ClientSecretPost),
            "client_secret_basic" => Some(Self::ClientSecretBasic),
            _ => None,
        }
    }
}

/// Pick the token-endpoint authentication method: the method the client was
/// registered with when the server still supports it, then the strongest
/// one the metadata lists that the client can do, and — when the metadata
/// lists nothing — the secret in the body, as before. The MCP TypeScript
/// SDK's order, so a server tested against the reference client meets the
/// same request here.
#[must_use]
pub fn choose_token_auth(
    has_secret: bool,
    registered: Option<&str>,
    supported: &[String],
) -> TokenAuth {
    let fallback = if has_secret {
        TokenAuth::ClientSecretPost
    } else {
        TokenAuth::None
    };
    if supported.is_empty() {
        return fallback;
    }
    let usable = |method: TokenAuth| {
        supported.iter().any(|name| name == method.name())
            && (has_secret || method == TokenAuth::None)
    };
    if let Some(method) = registered.and_then(TokenAuth::from_name)
        && usable(method)
    {
        return method;
    }
    [
        TokenAuth::ClientSecretBasic,
        TokenAuth::ClientSecretPost,
        TokenAuth::None,
    ]
    .into_iter()
    .find(|method| usable(*method))
    .unwrap_or(fallback)
}

/// The client's credentials for one token request: the form pairs to add
/// and, for `client_secret_basic`, the `Authorization` header — the id and
/// secret form-encoded before they are joined (RFC 6749 §2.3.1), and the id
/// left out of the body, since a request authenticates one way only.
#[must_use]
pub fn client_credentials(
    auth: TokenAuth,
    client_id: &str,
    secret: Option<&str>,
) -> (Vec<(String, String)>, Option<String>) {
    match (auth, secret) {
        (TokenAuth::ClientSecretBasic, Some(secret)) => {
            let pair = format!("{}:{}", url_encode(client_id), url_encode(secret));
            (
                Vec::new(),
                Some(format!(
                    "Basic {}",
                    crate::clipboard::base64_encode(pair.as_bytes())
                )),
            )
        }
        (TokenAuth::ClientSecretPost, Some(secret)) => (
            vec![
                ("client_id".to_string(), client_id.to_string()),
                ("client_secret".to_string(), secret.to_string()),
            ],
            None,
        ),
        _ => (vec![("client_id".to_string(), client_id.to_string())], None),
    }
}

/// May the flow send the user (or a token request) to this endpoint? HTTPS,
/// or plain HTTP to this machine's loopback — never a `javascript:`,
/// `data:` or `file:` URL a hostile metadata document named, which the
/// browser launcher would otherwise open as given.
#[must_use]
pub fn endpoint_allowed(endpoint: &str) -> bool {
    let Ok(parsed) = url::Url::parse(endpoint) else {
        return false;
    };
    match parsed.scheme() {
        "https" => parsed.host().is_some(),
        "http" => match parsed.host() {
            Some(url::Host::Domain(host)) => host.eq_ignore_ascii_case("localhost"),
            Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
            Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
            None => false,
        },
        _ => false,
    }
}

/// How a protected-resource metadata document's `resource` relates to the
/// server it was fetched for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceMatch {
    /// The server's own URL (scheme and host case, a trailing slash and the
    /// default port aside) — RFC 9728 §3.3's identity.
    Exact,
    /// The same origin, a parent path of the server's — the MCP TypeScript
    /// SDK's allowance. Google's per-tool documents name
    /// `https://calendarmcp.googleapis.com/mcp` for a server at `…/mcp/v1`.
    Prefix,
    /// Anything else: a document naming another server is never used, or a
    /// server could collect a token minted for someone else's resource.
    Foreign,
}

/// Compare a metadata document's `resource` against the server URL.
#[must_use]
pub fn resource_match(server_url: &str, resource: &str) -> ResourceMatch {
    let (Ok(server), Ok(claimed)) = (url::Url::parse(server_url), url::Url::parse(resource)) else {
        return ResourceMatch::Foreign;
    };
    let same_origin = server.scheme() == claimed.scheme()
        && server.host_str() == claimed.host_str()
        && server.port_or_known_default() == claimed.port_or_known_default();
    if !same_origin {
        return ResourceMatch::Foreign;
    }
    let server_path = server.path().trim_end_matches('/');
    let claimed_path = claimed.path().trim_end_matches('/');
    if server_path == claimed_path {
        ResourceMatch::Exact
    } else if claimed_path.is_empty()
        || server_path
            .strip_prefix(claimed_path)
            .is_some_and(|rest| rest.starts_with('/'))
    {
        ResourceMatch::Prefix
    } else {
        ResourceMatch::Foreign
    }
}

/// What a protected resource's metadata (RFC 9728) says, once accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceMeta {
    /// The RFC 8707 `resource` indicator to send: the document's own
    /// `resource`, **verbatim** (the conformance suite holds a client to
    /// byte-identity), or the server URL when the document names none.
    pub resource: String,
    /// The authorization servers, in the document's order.
    pub authorization_servers: Vec<String>,
    /// `scopes_supported`, space-joined — `None` when absent or empty.
    pub scopes: Option<String>,
}

/// Parse one protected-resource metadata document fetched for
/// `server_url`, with how its `resource` relates to that server. `None` for
/// anything that is not a JSON object.
#[must_use]
pub fn parse_resource_metadata(
    document: &Value,
    server_url: &str,
) -> Option<(ResourceMeta, ResourceMatch)> {
    let object = document.as_object()?;
    let strings = |key: &str| -> Vec<String> {
        object
            .get(key)
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::trim)
                    .filter(|item| !item.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    };
    let (resource, matched) = match object.get("resource").and_then(Value::as_str) {
        Some(resource) => (resource.to_string(), resource_match(server_url, resource)),
        None => (server_url.to_string(), ResourceMatch::Exact),
    };
    let scopes = strings("scopes_supported");
    Some((
        ResourceMeta {
            resource,
            authorization_servers: strings("authorization_servers"),
            scopes: (!scopes.is_empty()).then(|| scopes.join(" ")),
        },
        matched,
    ))
}

/// Where an authorization server's metadata may live, in the spec's order
/// (RFC 8414 §3.1 + OIDC Discovery, `docs/mcp.md`): for an issuer with a
/// path, the RFC 8414 path insertion, then OIDC's insertion, then OIDC's
/// appending; for one without, the two root documents. The issuer's
/// terminating `/` is removed first — RFC 8414's own construction rule, and
/// what turns Google's listed `https://accounts.google.com/` into the
/// documents its `issuer` names. Never a root fallback for an issuer with a
/// path: on a multi-tenant host that is another tenant's server.
#[must_use]
pub fn auth_server_metadata_candidates(issuer: &str) -> Vec<String> {
    let issuer = issuer.trim().trim_end_matches('/');
    let Ok(parsed) = url::Url::parse(issuer) else {
        return Vec::new();
    };
    let origin = parsed.origin().ascii_serialization();
    let path = parsed.path().trim_end_matches('/');
    if path.is_empty() {
        return vec![
            format!("{origin}/.well-known/oauth-authorization-server"),
            format!("{origin}/.well-known/openid-configuration"),
        ];
    }
    vec![
        format!("{origin}/.well-known/oauth-authorization-server{path}"),
        format!("{origin}/.well-known/openid-configuration{path}"),
        format!("{origin}{path}/.well-known/openid-configuration"),
    ]
}

/// Does a metadata document's `issuer` name the authorization server it was
/// discovered for? Identical, the terminating `/` aside (the one character
/// RFC 8414's construction removes) — a different host, port, path or
/// case is a different issuer, and its metadata is never used.
#[must_use]
pub fn issuer_matches(listed: &str, issued: &str) -> bool {
    listed.trim().trim_end_matches('/') == issued.trim().trim_end_matches('/')
}

/// May a discovered metadata document be used? Only when its `issuer` is
/// the one the resource metadata **listed** ([`issuer_matches`]); with no
/// listed issuer — a pre-2025-06-18 server whose authorization server is
/// looked for at its own origin, a guess rather than a claim — there is
/// nothing to hold it to, and the document's own `issuer` is what RFC
/// 9207's `iss` is then checked against.
#[must_use]
pub fn issuer_accepted(listed: Option<&str>, issued: Option<&str>) -> bool {
    match (listed, issued) {
        (Some(listed), Some(issued)) => issuer_matches(listed, issued),
        _ => true,
    }
}

/// Parse an authorization server's metadata document — the endpoints the
/// flow will use, refused when unusable: both endpoints present and
/// [`endpoint_allowed`], and PKCE `S256` advertised (the spec's MUST: a
/// client refuses to proceed without it, rather than run a code flow the
/// server may not protect).
pub fn parse_auth_server_metadata(document: &Value) -> Result<AuthServerMeta, String> {
    let field = |key: &str| {
        document
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    };
    let list = |key: &str| -> Vec<String> {
        document
            .get(key)
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    };
    let endpoint = |key: &str| -> Result<String, String> {
        let url = field(key).ok_or_else(|| format!("the metadata names no {key}"))?;
        if endpoint_allowed(&url) {
            Ok(url)
        } else {
            Err(format!(
                "its {key} ({url}) is neither HTTPS nor this machine"
            ))
        }
    };
    let authorization_endpoint = endpoint("authorization_endpoint")?;
    let token_endpoint = endpoint("token_endpoint")?;
    let registration_endpoint = match field("registration_endpoint") {
        Some(_) => Some(endpoint("registration_endpoint")?),
        None => None,
    };
    if !list("code_challenge_methods_supported")
        .iter()
        .any(|method| method == "S256")
    {
        return Err(
            "the authorization server does not advertise PKCE (S256), so no code \
             flow with it can be protected"
                .to_string(),
        );
    }
    Ok(AuthServerMeta {
        authorization_endpoint,
        token_endpoint,
        registration_endpoint,
        scopes_supported: list("scopes_supported"),
        token_auth_methods: list("token_endpoint_auth_methods_supported"),
        issuer: field("issuer"),
        iss_required: document
            .get("authorization_response_iss_parameter_supported")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

/// Where the flow's client identity comes from — the spec's registration
/// priority (`docs/mcp.md`): a pre-registered client, then one this
/// authorization server registered earlier, then dynamic registration. A
/// client ID metadata document is not among them: it needs an `https`
/// client id this app would have to host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientChoice {
    /// The server entry's own `oauth` block.
    Configured { id: String, secret: Option<String> },
    /// A client the same authorization server issued before (the stored
    /// grant's).
    Stored {
        id: String,
        secret: Option<String>,
        auth_method: Option<String>,
    },
    /// Register one now (RFC 7591).
    Register { endpoint: String },
}

/// Pick the client identity, or `None` when there is no way to get one —
/// the authorization server registers nobody and the user configured none.
/// A stored client is reused only with the authorization server that
/// issued it (2026-07-28, SEP-2352): its recorded `issuer`, or — for a
/// grant stored before issuers were recorded — the same token endpoint.
#[must_use]
pub fn choose_client(
    configured: Option<&McpOAuthConfig>,
    stored: Option<&StoredTokens>,
    meta: &AuthServerMeta,
) -> Option<ClientChoice> {
    if let Some(id) = configured.and_then(|c| c.client_id.clone()) {
        return Some(ClientChoice::Configured {
            id,
            secret: configured.and_then(|c| c.client_secret.clone()),
        });
    }
    if let Some(stored) = stored.filter(|s| !s.client_id.is_empty()) {
        let same_server = match (&stored.issuer, &meta.issuer) {
            (Some(theirs), Some(ours)) => issuer_matches(theirs, ours),
            (None, _) => stored.token_endpoint == meta.token_endpoint,
            (Some(_), None) => false,
        };
        if same_server {
            return Some(ClientChoice::Stored {
                id: stored.client_id.clone(),
                secret: stored.client_secret.clone(),
                auth_method: stored.auth_method.clone(),
            });
        }
    }
    meta.registration_endpoint
        .clone()
        .map(|endpoint| ClientChoice::Register { endpoint })
}

/// What a redirect — the loopback callback's request line, or a URL the
/// user pasted — carried, validated in the order RFC 9207 and the spec
/// demand: `None` when it is not an authorization response at all (a
/// browser's `favicon.ico`), else `state` must match the one sent (absence
/// is a mismatch — it is the CSRF check), then `iss` ([`validate_issuer`]),
/// and only then is the response believed: a code, or the authorization
/// server's refusal, worded. An `error` whose `iss` does not check out is
/// reported as the mismatch, never displayed (the spec's MUST — it may not
/// be the authorization server talking).
#[must_use]
pub fn read_redirect(
    text: &str,
    state: &str,
    issuer: Option<&str>,
    iss_required: bool,
) -> Option<Result<String, String>> {
    let code = redirect_param(text, "code");
    let error = redirect_param(text, "error");
    if code.is_none() && error.is_none() {
        return None;
    }
    if redirect_param(text, "state").as_deref() != Some(state) {
        return Some(Err(
            "the redirect's state does not match this sign-in".to_string()
        ));
    }
    if let Err(reason) =
        validate_issuer(redirect_param(text, "iss").as_deref(), issuer, iss_required)
    {
        return Some(Err(reason));
    }
    match (code, error) {
        (_, Some(error)) => {
            let description = redirect_param(text, "error_description")
                .map(|d| format!(" — {d}"))
                .unwrap_or_default();
            Some(Err(format!(
                "the authorization server refused: {error}{description}"
            )))
        }
        (Some(code), None) => Some(Ok(code)),
        (None, None) => unreachable!("checked above"),
    }
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
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AuthServerMeta {
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub registration_endpoint: Option<String>,
    /// The authorization server's own `scopes_supported` — where
    /// `offline_access` can come from ([`with_offline_access`]).
    pub scopes_supported: Vec<String>,
    /// `token_endpoint_auth_methods_supported` ([`choose_token_auth`]).
    pub token_auth_methods: Vec<String>,
    /// The AS `issuer` identifier — what RFC 9207's `iss` must equal.
    pub issuer: Option<String>,
    /// `authorization_response_iss_parameter_supported`: an AS that
    /// advertises it must send `iss` on every authorization response, so a
    /// response without one is rejected rather than excused.
    pub iss_required: bool,
}

fn get_json(url: &str) -> Result<Value, String> {
    get_json_within(url, HTTP_TIMEOUT)
}

fn get_json_within(url: &str, timeout: Duration) -> Result<Value, String> {
    let client = crate::llm::http_client(timeout).map_err(|e| e.to_string())?;
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
/// `authorization` is a `client_secret_basic` client's header.
fn post_form_raw(
    url: &str,
    params: &[(String, String)],
    authorization: Option<&str>,
) -> Result<(u16, String), String> {
    let client = crate::llm::http_client(HTTP_TIMEOUT).map_err(|e| e.to_string())?;
    let mut request = client
        .post(url)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .header("Accept", "application/json");
    if let Some(authorization) = authorization {
        request = request.header("Authorization", authorization);
    }
    let response = request
        .body(form_encode(params))
        .send()
        .map_err(|e| format!("request failed: {e}"))?;
    let status = response.status().as_u16();
    Ok((status, response.text().unwrap_or_default()))
}

fn post_form(
    url: &str,
    params: &[(String, String)],
    authorization: Option<&str>,
) -> Result<Value, String> {
    let (status, body) = post_form_raw(url, params, authorization)?;
    if !(200..300).contains(&status) {
        let flat: String = body.split_whitespace().collect::<Vec<_>>().join(" ");
        let flat: String = flat.chars().take(200).collect();
        return Err(format!("HTTP {status}: {flat}"));
    }
    serde_json::from_str(&body).map_err(|e| format!("bad JSON: {e}"))
}

/// The protected resource's metadata for `server_url` (RFC 9728), seeded by
/// the 401's challenge: the challenge's `resource_metadata` URL first (the
/// spec's MUST when present), then the path-inserted and root well-known
/// URLs. A document naming this server exactly is taken at once; one naming
/// a parent path on the same origin is kept as the fallback (Google's
/// per-tool documents, which the path-inserted one then outranks); one
/// naming another server is never used. `Ok(None)` when the server
/// publishes no metadata at all — a pre-2025-06-18 server, whose
/// authorization server is then looked for at its own origin.
pub fn discover_resource(
    server_url: &str,
    challenge: Option<&str>,
) -> Result<Option<ResourceMeta>, String> {
    let mut candidates: Vec<String> = Vec::new();
    if let Some(named) = challenge.and_then(challenge_resource_metadata) {
        candidates.push(named);
    }
    for candidate in well_known_candidates(server_url, "oauth-protected-resource") {
        if !candidates.contains(&candidate) {
            candidates.push(candidate);
        }
    }
    let mut fallback: Option<ResourceMeta> = None;
    let mut foreign: Option<String> = None;
    for candidate in candidates {
        let Ok(document) = get_json(&candidate) else {
            continue;
        };
        match parse_resource_metadata(&document, server_url) {
            Some((meta, ResourceMatch::Exact)) => return Ok(Some(meta)),
            Some((meta, ResourceMatch::Prefix)) => {
                fallback.get_or_insert(meta);
            }
            Some((meta, ResourceMatch::Foreign)) => {
                foreign.get_or_insert(meta.resource);
            }
            None => {}
        }
    }
    match (fallback, foreign) {
        (Some(meta), _) => Ok(Some(meta)),
        (None, Some(resource)) => Err(format!(
            "the server's OAuth metadata names a different server ({resource}), \
             so no token for it will be requested"
        )),
        (None, None) => Ok(None),
    }
}

/// Does `server_url` publish protected-resource metadata — is it a server
/// a user can sign in to? Asked of a remote server that connected without
/// presenting anything, so the `/mcp` page can offer `Authenticate` to one
/// that answers `initialize` and `tools/list` openly but refuses the calls
/// themselves (Google's Workspace servers), instead of calling it public.
///
/// Only a document naming an authorization server counts — the one field
/// the spec requires of it — so a server answering every GET with some
/// JSON is not mistaken for one, and each lookup gets a short budget.
#[must_use]
pub fn offers_oauth(server_url: &str) -> bool {
    well_known_candidates(server_url, "oauth-protected-resource")
        .iter()
        .filter_map(|candidate| get_json_within(candidate, PROBE_TIMEOUT).ok())
        .filter_map(|document| parse_resource_metadata(&document, server_url))
        .any(|(meta, matched)| {
            matched != ResourceMatch::Foreign && !meta.authorization_servers.is_empty()
        })
}

/// The authorization server's metadata: the configured
/// `authServerMetadataUrl` when there is one (the user's override, taken as
/// given), else discovered from the issuer the resource metadata lists —
/// or, for a server that publishes none, its own origin — through
/// [`auth_server_metadata_candidates`], each document's `issuer` checked
/// against the one it was looked up for ([`issuer_matches`]).
pub fn discover_auth_server(
    listed_issuer: Option<&str>,
    server_url: &str,
    metadata_url: Option<&str>,
) -> Result<AuthServerMeta, String> {
    if let Some(url) = metadata_url {
        let document = get_json(url).map_err(|e| format!("{url}: {e}"))?;
        return parse_auth_server_metadata(&document).map_err(|e| format!("{url}: {e}"));
    }
    let issuer = listed_issuer.map_or_else(|| origin_of(server_url), str::to_string);
    let mut last_err = String::from("no authorization-server metadata found");
    for candidate in auth_server_metadata_candidates(&issuer) {
        let document = match get_json(&candidate) {
            Ok(document) => document,
            Err(e) => {
                last_err = format!("{candidate}: {e}");
                continue;
            }
        };
        let meta = match parse_auth_server_metadata(&document) {
            Ok(meta) => meta,
            Err(e) => {
                last_err = format!("{candidate}: {e}");
                continue;
            }
        };
        if !issuer_accepted(listed_issuer, meta.issuer.as_deref()) {
            last_err = format!(
                "{candidate} describes {}, not {issuer}",
                meta.issuer.as_deref().unwrap_or_default()
            );
            continue;
        }
        return Ok(meta);
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

/// What a dynamic registration (RFC 7591) issued: the client id, its secret
/// when the server made it a confidential client, and the token-endpoint
/// method it was registered with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Registration {
    pub client_id: String,
    pub client_secret: Option<String>,
    pub auth_method: Option<String>,
}

/// Dynamically register a client (RFC 7591).
pub fn register_client(
    registration_endpoint: &str,
    redirect_uri: &str,
) -> Result<Registration, String> {
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
    let text = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_string);
    Ok(Registration {
        client_id: text("client_id")
            .ok_or_else(|| "registration returned no client_id".to_string())?,
        client_secret: text("client_secret"),
        auth_method: text("token_endpoint_auth_method"),
    })
}

// ---------------------------------------------------------------------------
// the interactive flow

/// What the flow reports as it goes — the manager forwards these onto the
/// MCP event channel for the auth page.
pub trait AuthProgress: Send {
    /// The authorize URL is ready (shown + copied from the auth page).
    fn on_url(&self, url: &str);
}

/// Why an interactive flow ended without a grant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthFlowError {
    /// No client identity to sign in with: the authorization server
    /// (`issuer`) registers nobody on the fly and the server's entry names
    /// no pre-registered client — the one failure the user fixes in a file,
    /// which [`no_client_message`] words.
    NoClient { issuer: String },
    /// Anything else, worded.
    Failed(String),
}

impl From<String> for AuthFlowError {
    fn from(detail: String) -> Self {
        Self::Failed(detail)
    }
}

/// What the `/mcp` auth page says when [`AuthFlowError::NoClient`] ends a
/// flow: which authorization server, what to add and where — plus, for
/// Google, where the client is made and the one checkbox its Workspace MCP
/// servers insist on. The block is the spec's "pre-registered client"
/// (`docs/mcp.md`).
#[must_use]
pub fn no_client_message(server: &str, issuer: &str, config_path: &str) -> String {
    let host = url::Url::parse(issuer)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .unwrap_or_else(|| issuer.to_string());
    let mut message = format!(
        "{host} does not register OAuth clients automatically, so {server} needs one you \
         create there. Add it to {server}'s entry in {config_path}: \
         \"oauth\": {{\"clientId\": \"…\", \"clientSecret\": \"…\"}} — then choose \
         Authenticate again."
    );
    if issuer.trim_end_matches('/') == GOOGLE_ISSUER {
        message.push_str(
            " Create it in Google Cloud Console under APIs & Services → Credentials → \
             OAuth client ID, as a Desktop app, and tick \"Use this client for an \
             AI-powered agent\" — Google Workspace MCP servers refuse clients without it.",
        );
    } else {
        message.push_str(
            " A client whose redirect URIs must be registered exactly also needs a fixed \
             \"callbackPort\": register http://localhost:{port}/callback.",
        );
    }
    message
}

/// Run the whole interactive flow for `server_url`, blocking this (worker)
/// thread: discovery → a client identity → browser/paste → exchange →
/// persist. `client` is the server entry's `oauth` block; `paste_rx`
/// delivers the user's pasted redirect URL; `open_browser` is the
/// boundary's launcher (injected so tests need no display).
#[allow(clippy::too_many_arguments)] // the flow's inputs, each one seam
pub fn run_auth_flow(
    server_url: &str,
    challenge: Option<&str>,
    client: Option<&McpOAuthConfig>,
    store_path: Option<&Path>,
    paste_rx: &mpsc::Receiver<String>,
    progress: &dyn AuthProgress,
    open_browser: &dyn Fn(&str),
    cancel: &CancelToken,
) -> Result<(), AuthFlowError> {
    let resource = discover_resource(server_url, challenge)?;
    let meta = discover_auth_server(
        resource
            .as_ref()
            .and_then(|r| r.authorization_servers.first())
            .map(String::as_str),
        server_url,
        client.and_then(|c| c.auth_server_metadata_url.as_deref()),
    )?;
    // The loopback listener binds first so registration can name the exact
    // redirect URI: on the configured port when the client's redirect URIs
    // are registered exactly, else wherever the system has room.
    let port = client.and_then(|c| c.callback_port).unwrap_or(0);
    let listener = TcpListener::bind(("127.0.0.1", port)).map_err(|e| {
        if port == 0 {
            format!("could not open the callback port: {e}")
        } else {
            format!("could not listen on callback port {port} ({e}) — free it or change oauth.callbackPort")
        }
    })?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("callback port: {e}"))?
        .port();
    let redirect_uri = format!("http://localhost:{port}/callback");
    let stored = load_tokens(store_path, server_url);
    let (client_id, client_secret, registered_method) =
        match choose_client(client, stored.as_ref(), &meta) {
            Some(ClientChoice::Configured { id, secret }) => (id, secret, None),
            Some(ClientChoice::Stored {
                id,
                secret,
                auth_method,
            }) => (id, secret, auth_method),
            Some(ClientChoice::Register { endpoint }) => {
                let registration = register_client(&endpoint, &redirect_uri)?;
                (
                    registration.client_id,
                    registration.client_secret,
                    registration.auth_method,
                )
            }
            None => {
                return Err(AuthFlowError::NoClient {
                    issuer: meta
                        .issuer
                        .clone()
                        .unwrap_or_else(|| meta.authorization_endpoint.clone()),
                });
            }
        };
    let auth = choose_token_auth(
        client_secret.is_some(),
        registered_method.as_deref(),
        &meta.token_auth_methods,
    );
    let resource_indicator = resource
        .as_ref()
        .map_or_else(|| server_url.to_string(), |r| r.resource.clone());
    let scope = choose_scope(
        client.and_then(|c| c.scopes.as_deref()),
        challenge.and_then(challenge_scope).as_deref(),
        resource.as_ref().and_then(|r| r.scopes.as_deref()),
        stored.as_ref().and_then(|t| t.scope.as_deref()),
        &meta.scopes_supported,
    );
    let (verifier, code_challenge, state) = pkce_and_state();
    let authorize_url = build_authorize_url(
        &meta.authorization_endpoint,
        &client_id,
        &redirect_uri,
        &code_challenge,
        &state,
        scope.as_deref(),
        &resource_indicator,
        &issuer_extras(meta.issuer.as_deref()),
    );
    progress.on_url(&authorize_url);
    open_browser(&authorize_url);
    let code = wait_for_code(&listener, &state, &meta, paste_rx, cancel)?;
    let mut params = vec![
        ("grant_type".to_string(), "authorization_code".to_string()),
        ("code".to_string(), code),
        ("redirect_uri".to_string(), redirect_uri),
        ("code_verifier".to_string(), verifier),
        ("resource".to_string(), resource_indicator.clone()),
    ];
    let (credentials, authorization) =
        client_credentials(auth, &client_id, client_secret.as_deref());
    params.extend(credentials);
    let response = post_form(&meta.token_endpoint, &params, authorization.as_deref())
        .map_err(|e| format!("token exchange failed: {e}"))?;
    let prior = StoredTokens {
        client_id,
        client_secret,
        token_endpoint: meta.token_endpoint.clone(),
        scope,
        resource: Some(resource_indicator),
        issuer: meta.issuer.clone(),
        auth_method: Some(auth.name().to_string()),
        ..Default::default()
    };
    let tokens = tokens_from_response(&response, &prior)
        .ok_or_else(|| "token response carried no access_token".to_string())?;
    save_tokens(store_path, server_url, Some(&tokens));
    Ok(())
}

/// Wait for the authorization code from **either** side: the loopback
/// callback, or a pasted redirect URL — each one [`read_redirect`]
/// validates (`state`, then the RFC 9207 `iss`) before its code is ever
/// redeemed, and a refusal (`error=access_denied`) ends the wait at once
/// instead of leaving the user staring at the timeout.
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
    let read = |text: &str| read_redirect(text, state, meta.issuer.as_deref(), meta.iss_required);
    loop {
        if cancel.is_cancelled() {
            return Err("cancelled".to_string());
        }
        if started.elapsed() > AUTH_FLOW_TIMEOUT {
            return Err("timed out waiting for the browser callback".to_string());
        }
        // The pasted redirect URL — the headless/remote path. Anything that
        // is not a redirect keeps the wait going; the page shows the format.
        if let Ok(text) = paste_rx.try_recv()
            && let Some(outcome) = read(&text)
        {
            return outcome;
        }
        match listener.accept() {
            Ok((mut stream, _)) => {
                // The accepted socket must block for its read even though
                // the listener does not.
                let _ = stream.set_nonblocking(false);
                let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                let mut buffer = [0u8; 8192];
                let read_len = stream.read(&mut buffer).unwrap_or(0);
                let request = String::from_utf8_lossy(&buffer[..read_len]);
                let path = request
                    .lines()
                    .next()
                    .and_then(|line| line.split_whitespace().nth(1))
                    .unwrap_or_default()
                    .to_string();
                let outcome = read(&path);
                let body = match &outcome {
                    Some(Ok(_)) => {
                        "<html><body><h3>Authentication complete.</h3>You can close this tab and return to the terminal.</body></html>"
                    }
                    Some(Err(_)) => {
                        "<html><body><h3>Authentication failed.</h3>Return to the terminal for the details.</body></html>"
                    }
                    None => "<html><body><h3>Waiting for authentication…</h3></body></html>",
                };
                let _ = stream.write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                    .as_bytes(),
                );
                if let Some(outcome) = outcome {
                    return outcome;
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
            &[],
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
        let url = build_authorize_url(
            "https://as.test/a?x=1",
            "c",
            "r",
            "ch",
            "s",
            None,
            "res",
            &[],
        );
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
        // RFC 6750's step-up challenge: the `scope` inside the error's value
        // is not the `scope` parameter after it.
        let step_up = r#"Bearer error="insufficient_scope", scope="files:read files:write", resource_metadata="https://x.test/m""#;
        assert_eq!(
            challenge_scope(step_up).as_deref(),
            Some("files:read files:write")
        );
        assert_eq!(
            challenge_param(step_up, "error").as_deref(),
            Some("insufficient_scope")
        );
        assert_eq!(
            challenge_resource_metadata(step_up).as_deref(),
            Some("https://x.test/m")
        );
        // Escapes inside a quoted string, and a parameter named first.
        assert_eq!(
            challenge_param(r#"Bearer realm="a \"b\" c""#, "realm").as_deref(),
            Some(r#"a "b" c"#)
        );
        assert_eq!(challenge_scope(r#"scope="x""#).as_deref(), Some("x"));
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
            ..Default::default()
        };
        let (params, header) = refresh_params(&tokens, "https://mcp.test/mcp");
        assert_eq!(
            header, None,
            "a public client sends no Authorization header"
        );
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
        let (params, header) = refresh_params(&confidential, "https://mcp.test/mcp");
        assert_eq!(header, None, "an older grant keeps its secret in the body");
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
                &[],
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

    /// A token endpoint that **detects refresh-token reuse**, as OAuth 2.1
    /// §4.3.1 requires of any server that rotates: the first presentation of
    /// a token rotates it, a second presentation of the *same* token is a
    /// replay and the whole family dies. Returns `(url, reuse_count)`.
    fn spawn_rotating_token_endpoint() -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        use std::io::{BufRead, BufReader, Read, Write};
        use std::sync::atomic::{AtomicUsize, Ordering};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();
        let reuse = std::sync::Arc::new(AtomicUsize::new(0));
        let reuse_t = std::sync::Arc::clone(&reuse);
        std::thread::spawn(move || {
            let spent: std::sync::Mutex<std::collections::HashSet<String>> =
                std::sync::Mutex::new(std::collections::HashSet::new());
            let issued = AtomicUsize::new(1);
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                if reader.read_line(&mut line).is_err() {
                    continue;
                }
                let mut content_length = 0usize;
                loop {
                    let mut header = String::new();
                    if reader.read_line(&mut header).is_err() || header.trim().is_empty() {
                        break;
                    }
                    if let Some(rest) = header.to_ascii_lowercase().strip_prefix("content-length:")
                    {
                        content_length = rest.trim().parse().unwrap_or(0);
                    }
                }
                let mut body = vec![0u8; content_length];
                let _ = reader.read_exact(&mut body);
                let body = String::from_utf8_lossy(&body).to_string();
                let presented = body
                    .split('&')
                    .find_map(|pair| pair.strip_prefix("refresh_token="))
                    .unwrap_or_default()
                    .to_string();
                let replayed = !spent.lock().unwrap().insert(presented);
                let response = if replayed {
                    reuse_t.fetch_add(1, Ordering::SeqCst);
                    let body = r#"{"error":"invalid_grant","error_description":"token reuse detected; grant revoked"}"#;
                    format!(
                        "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                } else {
                    let n = issued.fetch_add(1, Ordering::SeqCst) + 1;
                    let body = format!(
                        r#"{{"access_token":"at-{n}","refresh_token":"rt-{n}","token_type":"Bearer","expires_in":3600}}"#
                    );
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                };
                let _ = stream.write_all(response.as_bytes());
            }
        });
        (format!("http://127.0.0.1:{port}/token"), reuse)
    }

    #[test]
    fn concurrent_refreshes_present_the_token_once() {
        // Serialising the *file write* does not stop two threads presenting
        // the same refresh token to the *network*. Two subagents hitting one
        // MCP server share the manager, so this is reachable in normal use —
        // and its cost is the worst failure mode in this area: a compliant
        // AS treats the replay as theft and revokes the whole family, so the
        // grant is gone permanently and only the browser brings it back.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcp-auth.json");
        let (token_url, reuse) = spawn_rotating_token_endpoint();
        let server = "https://s.test/mcp";
        save_tokens(
            Some(&path),
            server,
            Some(&StoredTokens {
                access_token: "at-1".to_string(),
                refresh_token: Some("rt-1".to_string()),
                expires_at: Some(1), // stale: every caller wants a refresh
                client_id: "cid".to_string(),
                client_secret: None,
                token_endpoint: token_url,
                scope: None,
                ..Default::default()
            }),
        );
        // Spawn every thread *before* joining any — a lazy `.map().map()`
        // chain would spawn-and-join one at a time and race nothing.
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let path = path.clone();
                std::thread::spawn(move || refresh_grant(Some(&path), server, Some("at-1")))
            })
            .collect();
        let outcomes: Vec<_> = handles
            .into_iter()
            .map(|handle| handle.join().expect("refresh thread"))
            .collect();
        assert_eq!(
            reuse.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "the refresh token was presented twice — a reuse-detecting server \
             revokes the whole grant for this"
        );
        // Every caller comes away with a usable grant, and the store holds one.
        for outcome in &outcomes {
            assert!(
                outcome.is_ok(),
                "a caller was left without a token: {outcome:?}"
            );
        }
        assert!(load_tokens(Some(&path), server).is_some());
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
                        ..Default::default()
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
            resource: Some("https://mcp.test/mcp".to_string()),
            issuer: Some("https://as".to_string()),
            auth_method: Some("client_secret_basic".to_string()),
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

    // ===== a pre-registered client, Google's quirks, and the 2026-07-28
    // authorization rules (docs/mcp.md *A pre-registered client*) =====

    #[test]
    fn resource_metadata_must_name_this_server() {
        use ResourceMatch as M;
        let cal = "https://calendarmcp.googleapis.com/mcp/v1";
        assert_eq!(resource_match(cal, cal), M::Exact);
        // Scheme and host case, a trailing slash and the default port are
        // not differences.
        assert_eq!(
            resource_match("https://x.test/mcp", "HTTPS://X.test:443/mcp/"),
            M::Exact
        );
        // Google's per-tool documents name the parent path.
        assert_eq!(
            resource_match(cal, "https://calendarmcp.googleapis.com/mcp"),
            M::Prefix
        );
        assert_eq!(
            resource_match("https://x.test/mcp", "https://x.test/"),
            M::Prefix
        );
        // A prefix ends at a segment: /mcp is not a parent of /mcpx.
        assert_eq!(
            resource_match("https://x.test/mcpx", "https://x.test/mcp"),
            M::Foreign
        );
        // Another origin is never this server, whatever its path.
        assert_eq!(
            resource_match("https://x.test/mcp", "https://evil.test/mcp"),
            M::Foreign
        );
        assert_eq!(
            resource_match("https://x.test/mcp", "http://x.test/mcp"),
            M::Foreign
        );
        assert_eq!(
            resource_match("https://x.test:8443/mcp", "https://x.test/mcp"),
            M::Foreign
        );
        assert_eq!(
            resource_match("https://x.test/mcp", "not a url"),
            M::Foreign
        );
    }

    #[test]
    fn resource_metadata_keeps_its_indicator_verbatim() {
        // Google's own document, as served for the Calendar server.
        let document = serde_json::json!({
            "authorization_servers": ["https://accounts.google.com/"],
            "bearer_methods_supported": ["header"],
            "resource": "https://calendarmcp.googleapis.com/mcp/v1",
            "scopes_supported": [
                "https://www.googleapis.com/auth/calendar",
                "https://www.googleapis.com/auth/calendar.readonly"
            ]
        });
        let (meta, matched) =
            parse_resource_metadata(&document, "https://calendarmcp.googleapis.com/mcp/v1")
                .expect("an object");
        assert_eq!(matched, ResourceMatch::Exact);
        assert_eq!(
            meta,
            ResourceMeta {
                resource: "https://calendarmcp.googleapis.com/mcp/v1".to_string(),
                authorization_servers: vec!["https://accounts.google.com/".to_string()],
                scopes: Some(
                    "https://www.googleapis.com/auth/calendar \
                     https://www.googleapis.com/auth/calendar.readonly"
                        .to_string()
                ),
            }
        );
        // The indicator is the document's own spelling — re-serializing it
        // (a URL parser appends "/") breaks byte-identity.
        let (meta, _) = parse_resource_metadata(
            &serde_json::json!({"resource": "https://X.test/mcp"}),
            "https://x.test/mcp",
        )
        .unwrap();
        assert_eq!(meta.resource, "https://X.test/mcp");
        assert_eq!(meta.scopes, None);
        // A document naming no resource stands for the server itself.
        let (meta, matched) =
            parse_resource_metadata(&serde_json::json!({}), "https://x.test/mcp").unwrap();
        assert_eq!(
            (meta.resource.as_str(), matched),
            ("https://x.test/mcp", ResourceMatch::Exact)
        );
        assert_eq!(
            parse_resource_metadata(&serde_json::json!([1]), "https://x.test"),
            None
        );
    }

    #[test]
    fn authorization_server_metadata_is_looked_for_in_the_spec_order() {
        // Google lists its issuer with a terminating slash; RFC 8414's
        // construction removes it before inserting the well-known segment.
        assert_eq!(
            auth_server_metadata_candidates("https://accounts.google.com/"),
            [
                "https://accounts.google.com/.well-known/oauth-authorization-server",
                "https://accounts.google.com/.well-known/openid-configuration",
            ]
        );
        // An issuer with a path: RFC 8414 insertion, OIDC insertion, OIDC
        // appending — and never the root, which on a multi-tenant host is
        // another tenant's server.
        assert_eq!(
            auth_server_metadata_candidates("https://auth.example.com/tenant1"),
            [
                "https://auth.example.com/.well-known/oauth-authorization-server/tenant1",
                "https://auth.example.com/.well-known/openid-configuration/tenant1",
                "https://auth.example.com/tenant1/.well-known/openid-configuration",
            ]
        );
        assert!(auth_server_metadata_candidates("not a url").is_empty());
    }

    #[test]
    fn only_a_listed_issuer_holds_the_metadata_to_its_name() {
        // Listed by the resource metadata: the document must say the same.
        assert!(issuer_accepted(
            Some("https://as.test/"),
            Some("https://as.test")
        ));
        assert!(!issuer_accepted(
            Some("https://as.test"),
            Some("https://evil.test")
        ));
        // A server publishing no resource metadata had its origin guessed;
        // a guess is no claim to hold the document to.
        assert!(issuer_accepted(None, Some("https://auth.example.com")));
        assert!(issuer_accepted(Some("https://as.test"), None));
    }

    #[test]
    fn an_issuer_matches_only_itself_and_its_terminating_slash() {
        assert!(issuer_matches(
            "https://accounts.google.com/",
            "https://accounts.google.com"
        ));
        assert!(issuer_matches("https://as.test", "https://as.test/"));
        assert!(issuer_matches("https://as.test/t1", "https://as.test/t1"));
        assert!(!issuer_matches("https://as.test", "https://evil.test"));
        assert!(!issuer_matches("https://as.test/t1", "https://as.test/t2"));
        assert!(!issuer_matches("https://as.test", "https://AS.test"));
    }

    /// accounts.google.com's metadata as it was served on 2026-10-31.
    fn google_metadata() -> Value {
        serde_json::json!({
            "issuer": "https://accounts.google.com",
            "authorization_endpoint": "https://accounts.google.com/o/oauth2/v2/auth",
            "device_authorization_endpoint": "https://oauth2.googleapis.com/device/code",
            "token_endpoint": "https://oauth2.googleapis.com/token",
            "token_endpoint_auth_methods_supported": ["client_secret_post", "client_secret_basic"],
            "code_challenge_methods_supported": ["plain", "S256"],
            "scopes_supported": ["openid", "email", "profile"],
            "authorization_response_iss_parameter_supported": true
        })
    }

    #[test]
    fn authorization_server_metadata_needs_pkce_and_endpoints_it_may_open() {
        let meta = parse_auth_server_metadata(&google_metadata()).expect("Google's metadata");
        assert_eq!(
            meta.authorization_endpoint,
            "https://accounts.google.com/o/oauth2/v2/auth"
        );
        assert_eq!(meta.token_endpoint, "https://oauth2.googleapis.com/token");
        assert_eq!(meta.registration_endpoint, None);
        assert_eq!(meta.issuer.as_deref(), Some("https://accounts.google.com"));
        assert!(meta.iss_required);
        assert_eq!(
            meta.token_auth_methods,
            ["client_secret_post", "client_secret_basic"]
        );
        // The spec's MUST: no advertised S256, no code flow.
        for methods in [serde_json::json!(null), serde_json::json!(["plain"])] {
            let mut document = google_metadata();
            document["code_challenge_methods_supported"] = methods;
            let err = parse_auth_server_metadata(&document).expect_err("no S256");
            assert!(err.contains("S256"), "{err}");
        }
        // An endpoint the browser launcher must never be handed.
        for bad in [
            "javascript:alert(1)",
            "file:///etc/passwd",
            "http://as.test/authorize",
        ] {
            let mut document = google_metadata();
            document["authorization_endpoint"] = bad.into();
            assert!(parse_auth_server_metadata(&document).is_err(), "{bad}");
        }
        let mut document = google_metadata();
        document["token_endpoint"] = Value::Null;
        assert!(parse_auth_server_metadata(&document).is_err());
        // Plain HTTP to this machine is a local server, which is fine.
        let mut document = google_metadata();
        document["authorization_endpoint"] = "http://127.0.0.1:9/authorize".into();
        assert!(parse_auth_server_metadata(&document).is_ok());
    }

    #[test]
    fn only_https_or_this_machine_is_an_endpoint() {
        for good in [
            "https://as.test/authorize",
            "http://localhost:3000/authorize",
            "http://127.0.0.1/authorize",
            "http://[::1]:8080/token",
        ] {
            assert!(endpoint_allowed(good), "{good}");
        }
        for bad in [
            "http://as.test/authorize",
            "javascript:alert(1)",
            "data:text/html,hi",
            "file:///etc/passwd",
            "",
        ] {
            assert!(!endpoint_allowed(bad), "{bad}");
        }
    }

    #[test]
    fn the_scope_follows_the_spec_and_the_users_own_list_wins() {
        let offers = ["offline_access".to_string()];
        // The user's `oauth.scopes`, verbatim — nothing added to it.
        assert_eq!(
            choose_scope(Some(" a  b "), Some("c"), Some("d"), Some("e"), &offers).as_deref(),
            Some("a b")
        );
        // The challenge outranks the resource's list…
        assert_eq!(
            choose_scope(None, Some("c"), Some("d"), None, &[]).as_deref(),
            Some("c")
        );
        // …which stands when there is no challenge.
        assert_eq!(
            choose_scope(None, None, Some("d e"), None, &[]).as_deref(),
            Some("d e")
        );
        // Step-up (SEP-2350): the challenged scope joins the scope the grant
        // was requested with, so one tool's sign-in never costs another's.
        assert_eq!(
            choose_scope(None, Some("write read"), Some("read"), Some("read"), &[]).as_deref(),
            Some("read write")
        );
        // A fresh sign-in is the resource's list, not the old grant's.
        assert_eq!(
            choose_scope(None, None, Some("d"), Some("x"), &[]).as_deref(),
            Some("d")
        );
        // `offline_access` only from an authorization server offering it.
        assert_eq!(
            choose_scope(None, None, Some("d"), None, &offers).as_deref(),
            Some("d offline_access")
        );
        assert_eq!(choose_scope(None, None, None, None, &offers), None);
    }

    #[test]
    fn google_is_asked_for_a_refresh_token_its_own_way() {
        // Google rejects the `offline_access` scope and issues a refresh
        // token only for `access_type=offline` — a new one only with
        // `prompt=consent`. Without both a Google sign-in lasts an hour.
        let google = [
            ("access_type".to_string(), "offline".to_string()),
            ("prompt".to_string(), "consent".to_string()),
        ];
        assert_eq!(issuer_extras(Some("https://accounts.google.com")), google);
        assert_eq!(issuer_extras(Some("https://accounts.google.com/")), google);
        assert!(issuer_extras(Some("https://as.test")).is_empty());
        assert!(issuer_extras(None).is_empty());
        let url = build_authorize_url(
            "https://accounts.google.com/o/oauth2/v2/auth",
            "cid",
            "http://localhost:1/callback",
            "chal",
            "st",
            Some("https://www.googleapis.com/auth/calendar offline_access"),
            "https://calendarmcp.googleapis.com/mcp/v1",
            &google,
        );
        assert_eq!(
            redirect_param(&url, "access_type").as_deref(),
            Some("offline")
        );
        // `prompt` is one parameter, however many reasons asked for it.
        assert_eq!(url.matches("prompt=").count(), 1, "{url}");
        assert_eq!(
            redirect_param(&url, "resource").as_deref(),
            Some("https://calendarmcp.googleapis.com/mcp/v1")
        );
    }

    #[test]
    fn the_token_endpoint_method_follows_the_metadata() {
        use TokenAuth as T;
        let list = |names: &[&str]| names.iter().map(|n| n.to_string()).collect::<Vec<_>>();
        // Nothing listed: the secret in the body, as before.
        assert_eq!(choose_token_auth(true, None, &[]), T::ClientSecretPost);
        assert_eq!(choose_token_auth(false, None, &[]), T::None);
        // Google lists both: Basic, the MCP TypeScript SDK's preference.
        let google = list(&["client_secret_post", "client_secret_basic"]);
        assert_eq!(choose_token_auth(true, None, &google), T::ClientSecretBasic);
        assert_eq!(
            choose_token_auth(true, None, &list(&["client_secret_post"])),
            T::ClientSecretPost
        );
        // A registration's own method, while the server still supports it.
        assert_eq!(
            choose_token_auth(true, Some("client_secret_post"), &google),
            T::ClientSecretPost
        );
        // A public client never picks a secret-based method.
        assert_eq!(
            choose_token_auth(false, None, &list(&["none", "client_secret_basic"])),
            T::None
        );
        assert_eq!(T::from_name("private_key_jwt"), None);
        assert_eq!(
            T::from_name(T::ClientSecretBasic.name()),
            Some(T::ClientSecretBasic)
        );
    }

    #[test]
    fn client_credentials_ride_one_place_per_request() {
        // RFC 6749 §2.3.1: form-encode each half, join on ':', base64 —
        // and leave the id out of the body.
        let (params, header) =
            client_credentials(TokenAuth::ClientSecretBasic, "id:x", Some("s p"));
        assert!(params.is_empty(), "{params:?}");
        assert_eq!(
            header.as_deref(),
            Some(format!(
                "Basic {}",
                crate::clipboard::base64_encode(b"id%3Ax:s%20p")
            ))
            .as_deref()
        );
        let (params, header) = client_credentials(TokenAuth::ClientSecretPost, "id", Some("s"));
        assert_eq!(
            params,
            [
                ("client_id".to_string(), "id".to_string()),
                ("client_secret".to_string(), "s".to_string())
            ]
        );
        assert_eq!(header, None);
        let (params, header) = client_credentials(TokenAuth::None, "id", None);
        assert_eq!(params, [("client_id".to_string(), "id".to_string())]);
        assert_eq!(header, None);
    }

    #[test]
    fn a_refresh_reuses_the_resource_and_method_the_grant_was_issued_with() {
        let tokens = StoredTokens {
            access_token: "at".to_string(),
            refresh_token: Some("rt".to_string()),
            client_id: "cid".to_string(),
            client_secret: Some("shh".to_string()),
            token_endpoint: "https://as/token".to_string(),
            resource: Some("https://calendarmcp.googleapis.com/mcp/v1".to_string()),
            auth_method: Some("client_secret_basic".to_string()),
            ..Default::default()
        };
        let (params, header) = refresh_params(&tokens, "https://elsewhere.test/mcp");
        let get = |key: &str| {
            params
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.as_str())
        };
        assert_eq!(
            get("resource"),
            Some("https://calendarmcp.googleapis.com/mcp/v1")
        );
        assert_eq!(get("client_secret"), None);
        assert_eq!(get("client_id"), None);
        assert_eq!(
            header,
            Some(format!(
                "Basic {}",
                crate::clipboard::base64_encode(b"cid:shh")
            ))
        );
    }

    #[test]
    fn the_client_identity_follows_the_registration_priority() {
        let meta = AuthServerMeta {
            issuer: Some("https://as.test".to_string()),
            token_endpoint: "https://as.test/token".to_string(),
            registration_endpoint: Some("https://as.test/register".to_string()),
            ..Default::default()
        };
        let configured = McpOAuthConfig {
            client_id: Some("mine".to_string()),
            client_secret: Some("s".to_string()),
            ..Default::default()
        };
        let stored = StoredTokens {
            client_id: "registered".to_string(),
            issuer: Some("https://as.test".to_string()),
            ..Default::default()
        };
        // 1. The user's own client.
        assert_eq!(
            choose_client(Some(&configured), Some(&stored), &meta),
            Some(ClientChoice::Configured {
                id: "mine".to_string(),
                secret: Some("s".to_string())
            })
        );
        // 2. One this authorization server issued before.
        assert!(matches!(
            choose_client(None, Some(&stored), &meta),
            Some(ClientChoice::Stored { id, .. }) if id == "registered"
        ));
        // …and only that one (SEP-2352): another issuer's client is never
        // presented here, so registration runs instead.
        let elsewhere = StoredTokens {
            issuer: Some("https://other.test".to_string()),
            ..stored.clone()
        };
        assert_eq!(
            choose_client(None, Some(&elsewhere), &meta),
            Some(ClientChoice::Register {
                endpoint: "https://as.test/register".to_string()
            })
        );
        // A grant stored before issuers were recorded is matched by its
        // token endpoint.
        let older = StoredTokens {
            issuer: None,
            token_endpoint: "https://as.test/token".to_string(),
            ..stored.clone()
        };
        assert!(matches!(
            choose_client(None, Some(&older), &meta),
            Some(ClientChoice::Stored { .. })
        ));
        // 4. Nothing at all: Google's case without an `oauth` block.
        let google = parse_auth_server_metadata(&google_metadata()).unwrap();
        assert_eq!(choose_client(None, None, &google), None);
        // A block that names no client is no client.
        let port_only = McpOAuthConfig {
            callback_port: Some(8080),
            ..Default::default()
        };
        assert_eq!(choose_client(Some(&port_only), None, &google), None);
    }

    #[test]
    fn a_redirect_is_validated_before_it_is_believed() {
        let iss = "https%3A%2F%2Fas.test";
        let read = |text: &str| read_redirect(text, "st", Some("https://as.test"), true);
        assert_eq!(
            read(&format!("/callback?code=c1&state=st&iss={iss}")),
            Some(Ok("c1".to_string()))
        );
        // Not an authorization response at all: keep waiting.
        assert_eq!(read("/favicon.ico"), None);
        // `state` is the CSRF check — absent is as bad as wrong.
        assert!(matches!(
            read(&format!("/callback?code=c1&iss={iss}")),
            Some(Err(_))
        ));
        assert!(matches!(
            read(&format!("/callback?code=c1&state=no&iss={iss}")),
            Some(Err(_))
        ));
        // An AS that promised `iss` must send it, and send its own.
        assert!(matches!(read("/callback?code=c1&state=st"), Some(Err(_))));
        // A refusal ends the wait with the server's own words…
        let refused = read(&format!(
            "/callback?error=access_denied&error_description=User+said+no&state=st&iss={iss}"
        ));
        match refused {
            Some(Err(reason)) => {
                assert!(reason.contains("access_denied"), "{reason}");
                assert!(reason.contains("User said no"), "{reason}");
            }
            other => panic!("{other:?}"),
        }
        // …unless it is not the server's: a mismatched `iss` is reported
        // as the mismatch and the error is never shown.
        match read(
            "/callback?error=access_denied&error_description=Pay+here&state=st&iss=https%3A%2F%2Fevil.test",
        ) {
            Some(Err(reason)) => assert!(!reason.contains("Pay here"), "{reason}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_no_client_message_says_what_to_add_and_where() {
        let google = no_client_message(
            "google-calendar",
            "https://accounts.google.com",
            "~/.alter-zero/mcp.json",
        );
        for needle in [
            "accounts.google.com",
            "google-calendar",
            "~/.alter-zero/mcp.json",
            "\"clientId\"",
            "\"clientSecret\"",
            "Desktop app",
            "Use this client for an AI-powered agent",
        ] {
            assert!(google.contains(needle), "{needle} missing: {google}");
        }
        let other = no_client_message("wiki", "https://auth.example.com", "./.mcp.json");
        assert!(other.contains("auth.example.com"), "{other}");
        assert!(other.contains("callbackPort"), "{other}");
        assert!(!other.contains("Google"), "{other}");
    }

    /// An MCP server's protected-resource metadata, its authorization server
    /// and a token endpoint, all on one loopback port — what the interactive
    /// flow walks. The authorization server is listed with a terminating
    /// slash, the way Google lists its own. Token requests are recorded as
    /// `(Authorization header, body)`.
    struct FlowFixture {
        base: String,
        token_requests: std::sync::Arc<std::sync::Mutex<Vec<(String, String)>>>,
    }

    fn spawn_flow_fixture() -> FlowFixture {
        use std::io::{BufRead, BufReader};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let token_requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorded = std::sync::Arc::clone(&token_requests);
        let root = base.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                if reader.read_line(&mut line).is_err() {
                    continue;
                }
                let path = line
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or_default()
                    .to_string();
                let (mut length, mut authorization) = (0usize, String::new());
                loop {
                    let mut header = String::new();
                    if reader.read_line(&mut header).is_err() || header.trim().is_empty() {
                        break;
                    }
                    let lower = header.to_ascii_lowercase();
                    if let Some(value) = lower.strip_prefix("content-length:") {
                        length = value.trim().parse().unwrap_or(0);
                    }
                    if lower.starts_with("authorization:") {
                        authorization = header["authorization:".len()..].trim().to_string();
                    }
                }
                let mut body = vec![0u8; length];
                let _ = reader.read_exact(&mut body);
                let body = String::from_utf8_lossy(&body).to_string();
                let json = match path.as_str() {
                    "/.well-known/oauth-protected-resource/mcp" => Some(serde_json::json!({
                        "resource": format!("{root}/mcp"),
                        "authorization_servers": [format!("{root}/")],
                        "scopes_supported": ["read", "write"],
                    })),
                    // A per-tool document naming the parent path, like
                    // Google's: usable, but the server's own outranks it.
                    "/.well-known/oauth-protected-resource/list_things" => {
                        Some(serde_json::json!({
                            "resource": format!("{root}/"),
                            "authorization_servers": [format!("{root}/")],
                            "scopes_supported": ["read"],
                        }))
                    }
                    "/foreign-metadata" => Some(serde_json::json!({
                        "resource": "https://evil.test/mcp",
                        "authorization_servers": ["https://evil.test"],
                    })),
                    "/.well-known/oauth-authorization-server" => Some(serde_json::json!({
                        "issuer": root,
                        "authorization_endpoint": format!("{root}/authorize"),
                        "token_endpoint": format!("{root}/token"),
                        "code_challenge_methods_supported": ["S256"],
                        "token_endpoint_auth_methods_supported": ["client_secret_post", "client_secret_basic"],
                        "authorization_response_iss_parameter_supported": true,
                    })),
                    "/token" => {
                        recorded.lock().unwrap().push((authorization, body));
                        Some(serde_json::json!({
                            "access_token": "at-1",
                            "refresh_token": "rt-1",
                            "token_type": "Bearer",
                            "expires_in": 3600,
                        }))
                    }
                    _ => None,
                };
                let response = match json {
                    Some(json) => {
                        let body = json.to_string();
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        )
                    }
                    None => {
                        "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                            .to_string()
                    }
                };
                let _ = stream.write_all(response.as_bytes());
            }
        });
        FlowFixture {
            base,
            token_requests,
        }
    }

    /// The browser's half of a sign-in: follow the authorize URL's redirect
    /// back to the loopback callback, appending `answer` (the code, or the
    /// authorization server's refusal) beside the right `state` and `iss`.
    fn answer_like_a_browser(authorize_url: &str, issuer: &str, answer: &str) {
        let redirect = redirect_param(authorize_url, "redirect_uri").expect("a redirect_uri");
        let state = redirect_param(authorize_url, "state").expect("a state");
        let port = url::Url::parse(&redirect)
            .unwrap()
            .port()
            .expect("a loopback port");
        let path = url::Url::parse(&redirect).unwrap().path().to_string();
        let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).expect("callback");
        let request = format!(
            "GET {path}?{answer}&state={state}&iss={} HTTP/1.1\r\nHost: localhost\r\n\r\n",
            url_encode(issuer)
        );
        stream.write_all(request.as_bytes()).unwrap();
        let mut reply = String::new();
        let _ = stream.read_to_string(&mut reply);
    }

    struct ShownUrl(std::sync::Arc<std::sync::Mutex<Option<String>>>);
    impl AuthProgress for ShownUrl {
        fn on_url(&self, url: &str) {
            *self.0.lock().unwrap() = Some(url.to_string());
        }
    }

    /// Run the flow against `fixture` with the browser answering `answer`.
    fn sign_in(
        fixture: &FlowFixture,
        server: &str,
        challenge: Option<&str>,
        client: Option<&McpOAuthConfig>,
        store: &Path,
        answer: &'static str,
    ) -> (Result<(), AuthFlowError>, Option<String>) {
        let (_paste_tx, paste_rx) = mpsc::channel();
        let shown = std::sync::Arc::new(std::sync::Mutex::new(None));
        let issuer = fixture.base.clone();
        let browser = move |url: &str| {
            let (url, issuer) = (url.to_string(), issuer.clone());
            std::thread::spawn(move || answer_like_a_browser(&url, &issuer, answer));
        };
        let result = run_auth_flow(
            server,
            challenge,
            client,
            Some(store),
            &paste_rx,
            &ShownUrl(std::sync::Arc::clone(&shown)),
            &browser,
            &CancelToken::new(),
        );
        let url = shown.lock().unwrap().clone();
        (result, url)
    }

    #[test]
    fn a_pre_registered_client_signs_in_where_nothing_registers_clients() {
        // The Google Calendar shape: no registration endpoint, the secret
        // demanded at the token endpoint, the issuer listed with a slash.
        let fixture = spawn_flow_fixture();
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("mcp-auth.json");
        let server = format!("{}/mcp", fixture.base);
        let client = McpOAuthConfig {
            client_id: Some("my-client".to_string()),
            client_secret: Some("my-secret".to_string()),
            ..Default::default()
        };
        let (result, url) = sign_in(
            &fixture,
            &server,
            None,
            Some(&client),
            &store,
            "code=the-code",
        );
        result.expect("signed in");
        let url = url.expect("the authorize URL was shown");
        assert!(
            url.starts_with(&format!("{}/authorize?", fixture.base)),
            "{url}"
        );
        let param = |key: &str| redirect_param(&url, key);
        assert_eq!(param("client_id").as_deref(), Some("my-client"));
        assert_eq!(param("resource").as_deref(), Some(server.as_str()));
        assert_eq!(param("scope").as_deref(), Some("read write"));
        assert_eq!(param("code_challenge_method").as_deref(), Some("S256"));
        assert_eq!(
            param("access_type"),
            None,
            "only Google's issuer gets its extras"
        );
        // The token request authenticates once, with Basic.
        let requests = fixture.token_requests.lock().unwrap().clone();
        assert_eq!(requests.len(), 1, "{requests:?}");
        let (authorization, body) = &requests[0];
        assert_eq!(
            authorization,
            &format!(
                "Basic {}",
                crate::clipboard::base64_encode(b"my-client:my-secret")
            )
        );
        let field = |key: &str| redirect_param(&format!("?{body}"), key);
        assert_eq!(field("grant_type").as_deref(), Some("authorization_code"));
        assert_eq!(field("code").as_deref(), Some("the-code"));
        assert!(
            field("code_verifier").is_some_and(|v| v.len() >= 43),
            "{body}"
        );
        assert_eq!(field("resource").as_deref(), Some(server.as_str()));
        assert_eq!(field("client_secret"), None, "{body}");
        // The grant remembers how it was issued, so a refresh repeats it.
        let stored = load_tokens(Some(&store), &server).expect("stored");
        assert_eq!(stored.access_token, "at-1");
        assert_eq!(stored.refresh_token.as_deref(), Some("rt-1"));
        assert_eq!(stored.client_id, "my-client");
        assert_eq!(stored.client_secret.as_deref(), Some("my-secret"));
        assert_eq!(stored.auth_method.as_deref(), Some("client_secret_basic"));
        assert_eq!(stored.issuer.as_deref(), Some(fixture.base.as_str()));
        assert_eq!(stored.resource.as_deref(), Some(server.as_str()));
        assert_eq!(stored.scope.as_deref(), Some("read write"));
    }

    #[test]
    fn without_a_client_the_flow_names_the_server_that_registers_nobody() {
        let fixture = spawn_flow_fixture();
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("mcp-auth.json");
        let server = format!("{}/mcp", fixture.base);
        let (result, url) = sign_in(&fixture, &server, None, None, &store, "code=c");
        assert_eq!(
            result,
            Err(AuthFlowError::NoClient {
                issuer: fixture.base.clone()
            })
        );
        assert_eq!(
            url, None,
            "no browser is opened for a sign-in that cannot finish"
        );
        assert!(load_tokens(Some(&store), &server).is_none());
    }

    #[test]
    fn a_refused_consent_ends_the_flow_at_once() {
        let fixture = spawn_flow_fixture();
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("mcp-auth.json");
        let server = format!("{}/mcp", fixture.base);
        let client = McpOAuthConfig {
            client_id: Some("c".to_string()),
            ..Default::default()
        };
        let started = Instant::now();
        let (result, _) = sign_in(
            &fixture,
            &server,
            None,
            Some(&client),
            &store,
            "error=access_denied",
        );
        match result {
            Err(AuthFlowError::Failed(reason)) => {
                assert!(reason.contains("access_denied"), "{reason}");
            }
            other => panic!("{other:?}"),
        }
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "it waited for the timeout"
        );
        assert!(fixture.token_requests.lock().unwrap().is_empty());
    }

    #[test]
    fn the_servers_own_metadata_outranks_a_per_tool_document() {
        let fixture = spawn_flow_fixture();
        let server = format!("{}/mcp", fixture.base);
        // The 401 named a per-tool document (Google's shape) whose resource
        // is the parent path; the document naming the server itself wins.
        let challenge = format!(
            r#"Bearer resource_metadata="{}/.well-known/oauth-protected-resource/list_things""#,
            fixture.base
        );
        let meta = discover_resource(&server, Some(&challenge))
            .unwrap()
            .unwrap();
        assert_eq!(meta.resource, server);
        assert_eq!(meta.scopes.as_deref(), Some("read write"));
        // With nothing better, the same-origin parent is used.
        let elsewhere = format!("{}/other", fixture.base);
        let meta = discover_resource(&elsewhere, Some(&challenge))
            .unwrap()
            .unwrap();
        assert_eq!(meta.resource, format!("{}/", fixture.base));
        // A document naming another server is never used at all.
        let foreign = format!(
            r#"Bearer resource_metadata="{}/foreign-metadata""#,
            fixture.base
        );
        let err = discover_resource(&elsewhere, Some(&foreign)).expect_err("foreign");
        assert!(err.contains("evil.test"), "{err}");
        // No metadata anywhere: a pre-2025-06-18 server.
        assert_eq!(
            discover_resource(&format!("{}/none", fixture.base), None),
            Ok(None)
        );
        // The probe the /mcp page asks after a bare connect.
        assert!(offers_oauth(&server));
        assert!(!offers_oauth(&format!("{}/none", fixture.base)));
    }
}
