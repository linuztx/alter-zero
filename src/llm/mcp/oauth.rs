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
    /// How the client authenticated at the token endpoint
    /// (`client_secret_post`/`client_secret_basic`) — the refresh must do
    /// the same. `None` reads as the body, the pre-negotiation behaviour.
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
                    auth_method: text("token_endpoint_auth_method"),
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
                if let Some(method) = &tokens.auth_method {
                    entry.insert("token_endpoint_auth_method".into(), method.clone().into());
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
    if let Some(path) = path {
        let _ = write_store(path, |existing| record_tokens(existing, server_url, tokens));
    }
}

/// The store's one write path: under [`STORE_LOCK`], read the file, apply
/// `update`, and commit the result write-then-rename, owner-only. The TUI's
/// writers ignore the result like every other config write; the CLI's
/// ([`store_client`]) must not.
fn write_store(path: &Path, update: impl FnOnce(&str) -> String) -> std::io::Result<()> {
    let _guard = STORE_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let existing = match std::fs::read_to_string(path) {
        Ok(existing) => existing,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e),
    };
    let updated = update(&existing);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // Write-then-rename, so a crash mid-write can't leave a truncated store:
    // a half-written file parses as *no* servers, silently unauthenticating
    // every one of them at once.
    let temp = path.with_extension("json.tmp");
    if std::fs::write(&temp, &updated).is_ok() {
        set_owner_only(&temp);
        if std::fs::rename(&temp, path).is_ok() {
            return Ok(());
        }
        let _ = std::fs::remove_file(&temp);
    }
    std::fs::write(path, updated)?;
    set_owner_only(path);
    Ok(())
}

/// A client identity kept for one server, apart from its grant so a cleared
/// login keeps it: credentials the user entered on the `/mcp` auth page (or
/// a secret the CLI stored beside a configured `clientId`), or a dynamic
/// registration (`docs/mcp.md` *Pre-registered clients*).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StoredClient {
    pub client_id: String,
    pub client_secret: Option<String>,
    /// The authorization server the credentials belong to — the spec binds
    /// static credentials to their issuer (SEP-2352). `None` until a flow
    /// has seen it (a secret stored from the command line).
    pub issuer: Option<String>,
    /// Set for a dynamic registration: the redirect URI it registered. Such
    /// a client is reused only by a flow listening on exactly that URI.
    pub redirect_uri: Option<String>,
    /// The token-endpoint authentication method a registration settled.
    pub auth_method: Option<String>,
}

/// Parse the store's `clients` section into `server_url → client`.
#[must_use]
pub fn parse_client_store(contents: &str) -> std::collections::BTreeMap<String, StoredClient> {
    let Ok(value) = serde_json::from_str::<Value>(contents.trim()) else {
        return Default::default();
    };
    let Some(clients) = value.get("clients").and_then(Value::as_object) else {
        return Default::default();
    };
    clients
        .iter()
        .filter_map(|(url, entry)| {
            let text = |key: &str| entry.get(key).and_then(Value::as_str).map(str::to_string);
            Some((
                url.clone(),
                StoredClient {
                    client_id: text("client_id").filter(|id| !id.is_empty())?,
                    client_secret: text("client_secret"),
                    issuer: text("issuer"),
                    redirect_uri: text("redirect_uri"),
                    auth_method: text("token_endpoint_auth_method"),
                },
            ))
        })
        .collect()
}

/// Re-render the store with one server's client replaced (`None` deletes
/// it) — [`record_tokens`]' sibling over the `clients` section.
#[must_use]
pub fn record_client(contents: &str, server_url: &str, client: Option<&StoredClient>) -> String {
    let mut value: Value = serde_json::from_str(contents.trim())
        .ok()
        .filter(Value::is_object)
        .unwrap_or_else(|| Value::Object(Default::default()));
    let root = value.as_object_mut().expect("object ensured above");
    let clients = root
        .entry("clients")
        .or_insert_with(|| Value::Object(Default::default()));
    if let Some(clients) = clients.as_object_mut() {
        match client {
            Some(client) => {
                let mut entry = serde_json::Map::new();
                entry.insert("client_id".into(), client.client_id.clone().into());
                let optional = [
                    ("client_secret", &client.client_secret),
                    ("issuer", &client.issuer),
                    ("redirect_uri", &client.redirect_uri),
                    ("token_endpoint_auth_method", &client.auth_method),
                ];
                for (key, field) in optional {
                    if let Some(field) = field {
                        entry.insert(key.into(), field.clone().into());
                    }
                }
                clients.insert(server_url.to_string(), Value::Object(entry));
            }
            None => {
                clients.remove(server_url);
            }
        }
        if clients.is_empty() {
            root.remove("clients");
        }
    }
    serde_json::to_string_pretty(&value).unwrap_or_else(|_| contents.to_string())
}

/// Load one server's stored client.
#[must_use]
pub fn load_client(path: Option<&Path>, server_url: &str) -> Option<StoredClient> {
    let contents = std::fs::read_to_string(path?).ok()?;
    parse_client_store(&contents).remove(server_url)
}

/// Persist (or delete) one server's client — the token store's own
/// locked, owner-only, write-then-rename read-modify-write, best-effort.
pub fn save_client(path: Option<&Path>, server_url: &str, client: Option<&StoredClient>) {
    if let Some(path) = path {
        let _ = store_client(path, server_url, client);
    }
}

/// [`save_client`] for a caller that must report a failed write — the
/// `mcp add --client-secret` CLI (`docs/mcp-cli.md`).
pub fn store_client(
    path: &Path,
    server_url: &str,
    client: Option<&StoredClient>,
) -> std::io::Result<()> {
    write_store(path, |existing| record_client(existing, server_url, client))
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
/// the classic silent refresh loop. The client authenticates the way the
/// grant was obtained ([`StoredTokens::auth_method`]), so a body secret and
/// the Basic header (the second element) never both ride one request.
#[must_use]
pub fn refresh_params(
    tokens: &StoredTokens,
    server_url: &str,
) -> (Vec<(String, String)>, Option<String>) {
    let has_secret = tokens.client_secret.is_some();
    let method = tokens
        .auth_method
        .as_deref()
        .and_then(ClientAuth::parse)
        .filter(|_| has_secret)
        .unwrap_or_else(|| choose_client_auth(&[], has_secret, None));
    let (mut params, basic) =
        client_auth_parts(&tokens.client_id, tokens.client_secret.as_deref(), method);
    params.splice(
        0..0,
        [
            ("grant_type".to_string(), "refresh_token".to_string()),
            (
                "refresh_token".to_string(),
                tokens.refresh_token.clone().unwrap_or_default(),
            ),
        ],
    );
    params.push(("resource".to_string(), server_url.to_string()));
    (params, basic)
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
    let (params, basic) = refresh_params(&tokens, server_url);
    let (status, body) = match post_form_raw(&tokens.token_endpoint, &params, basic.as_deref()) {
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
    extra: &[(&str, &str)],
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
            params.push(((*key).to_string(), (*value).to_string()));
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

/// One query parameter out of a redirect URL/path/query, by name — the
/// `code`, the `state`, a refusal's `error`, and RFC 9207's `iss`.
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

/// `code` and `state` out of a redirect URL, path or bare query — the shape
/// the ChatGPT and Claude sign-ins read (`llm::chatgpt`, `llm::claude`);
/// [`parse_callback`] is the fuller reading the MCP flow acts on.
#[must_use]
pub fn parse_redirect(text: &str) -> Option<(String, Option<String>)> {
    match parse_callback(text) {
        Callback::Code { code, state } => Some((code, state)),
        Callback::Denied { .. } | Callback::Other => None,
    }
}

/// What one request on the loopback port — or one pasted redirect URL —
/// carried back from the authorization server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Callback {
    /// The authorization code, and the `state` it rode with.
    Code { code: String, state: Option<String> },
    /// The authorization server's refusal (RFC 6749 §4.1.2.1): `error` is
    /// `access_denied` when the user pressed Cancel on the consent screen.
    Denied {
        error: String,
        description: Option<String>,
        state: Option<String>,
    },
    /// Not the redirect at all — a favicon fetch, a stale tab.
    Other,
}

/// Read a callback request path (or a pasted redirect URL / bare query): a
/// code wins, else an `error` is the server's refusal, else it is some other
/// request on the port.
#[must_use]
pub fn parse_callback(text: &str) -> Callback {
    let state = redirect_param(text, "state");
    if let Some(code) = redirect_param(text, "code") {
        return Callback::Code { code, state };
    }
    match redirect_param(text, "error") {
        Some(error) => Callback::Denied {
            error,
            description: redirect_param(text, "error_description"),
            state,
        },
        None => Callback::Other,
    }
}

impl Callback {
    /// The refusal as the flow reports it — `access_denied: The user said
    /// no`, or the bare code when the server gave no description.
    fn refusal(error: &str, description: Option<&str>) -> String {
        match description {
            Some(description) => {
                format!("the authorization server refused ({error}: {description})")
            }
            None => format!("the authorization server refused ({error})"),
        }
    }
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
    pub scopes: Option<String>,
    /// The AS `issuer` identifier — what RFC 9207's `iss` must equal.
    pub issuer: Option<String>,
    /// `authorization_response_iss_parameter_supported`: an AS that
    /// advertises it must send `iss` on every authorization response, so a
    /// response without one is rejected rather than excused.
    pub iss_required: bool,
    /// `token_endpoint_auth_methods_supported` — how a client may
    /// authenticate at the token endpoint ([`choose_client_auth`]).
    pub auth_methods: Vec<String>,
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

/// POST a token request and hand back `(status, body)` raw — each caller
/// classifies a failure itself (`Err` only when no answer arrived at all).
/// `basic` is the `client_secret_basic` header, when that is the method.
fn post_form_raw(
    url: &str,
    params: &[(String, String)],
    basic: Option<&str>,
) -> Result<(u16, String), String> {
    let client = crate::llm::http_client(HTTP_TIMEOUT).map_err(|e| e.to_string())?;
    let mut request = client
        .post(url)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .header("Accept", "application/json");
    if let Some(basic) = basic {
        request = request.header("Authorization", basic);
    }
    let response = request
        .body(form_encode(params))
        .send()
        .map_err(|e| format!("request failed: {e}"))?;
    let status = response.status().as_u16();
    Ok((status, response.text().unwrap_or_default()))
}

/// Resolve the authorization server for `server_url`, seeded by the 401's
/// challenge when one was captured. A server's `oauth` config can pin the
/// scopes and name the metadata document outright
/// (`authServerMetadataUrl`), skipping the well-known search.
pub fn discover(
    server_url: &str,
    challenge: Option<&str>,
    config: Option<&crate::mcp::McpOAuthConfig>,
) -> Result<AuthServerMeta, String> {
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
    // 2. AS metadata: the configured document, else the named server's,
    //    else the MCP server's own origin's.
    let as_candidates = match config.and_then(|c| c.auth_server_metadata_url.clone()) {
        Some(url) => vec![url],
        None => {
            let auth_base = auth_server.unwrap_or_else(|| origin_of(server_url));
            let mut candidates = well_known_candidates(&auth_base, "oauth-authorization-server");
            candidates.extend(well_known_candidates(&auth_base, "openid-configuration"));
            candidates
        }
    };
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
                // Pinned scopes outrank both: the user chose them.
                let base = config
                    .and_then(|c| c.scopes.as_deref())
                    .or(challenged_scopes.as_deref())
                    .or(resource_scopes.as_deref());
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
                    auth_methods: string_list(&meta, "token_endpoint_auth_methods_supported"),
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

/// What a dynamic registration (RFC 7591) settled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Registration {
    pub client_id: String,
    pub client_secret: Option<String>,
    /// The `token_endpoint_auth_method` the server registered — it may
    /// differ from the `none` asked for, and the exchange must follow it.
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
            .filter(|id| !id.is_empty())
            .ok_or_else(|| "registration returned no client_id".to_string())?,
        client_secret: text("client_secret"),
        auth_method: text("token_endpoint_auth_method"),
    })
}

/// A metadata document's string array (`[]` when absent or malformed).
fn string_list(meta: &Value, key: &str) -> Vec<String> {
    meta.get(key)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Do two authorization-server identifiers name the same server? Exactly —
/// RFC 8414 §3.3 and the 2026-07-28 authorization spec forbid normalizing —
/// except for one trailing `/` on a bare origin, which both official SDKs
/// forgive because real deployments disagree on it: Google's resource
/// metadata names `https://accounts.google.com/` while its own metadata
/// says `https://accounts.google.com`.
#[must_use]
pub fn issuers_match(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    let bare_origin = |text: &str| {
        url::Url::parse(text)
            .ok()
            .is_some_and(|url| url.path() == "/" && url.query().is_none())
    };
    let (shorter, longer) = if a.len() < b.len() { (a, b) } else { (b, a) };
    longer.strip_suffix('/') == Some(shorter) && bare_origin(longer)
}

/// How a client authenticates at the token endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientAuth {
    /// A public client: `client_id` in the body, nothing else.
    None,
    /// `client_secret_post` — the secret in the form body.
    Post,
    /// `client_secret_basic` — HTTP Basic over the form-encoded pair.
    Basic,
}

impl ClientAuth {
    /// The RFC 7591 / RFC 8414 spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Post => "client_secret_post",
            Self::Basic => "client_secret_basic",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        match text {
            "none" => Some(Self::None),
            "client_secret_post" => Some(Self::Post),
            "client_secret_basic" => Some(Self::Basic),
            _ => None,
        }
    }
}

/// Choose how to present a client at the token endpoint. Without a secret
/// there is nothing to present. With one: the method a registration settled
/// while the server still lists it, else the form body — the method every
/// OAuth 2.1 server MUST accept, and this client's behaviour before it
/// negotiated — unless the server lists Basic and not the body. A server
/// that takes Basic alone answered a body secret with `invalid_client`.
#[must_use]
pub fn choose_client_auth(
    supported: &[String],
    has_secret: bool,
    registered: Option<&str>,
) -> ClientAuth {
    if !has_secret {
        return ClientAuth::None;
    }
    let listed = |method: ClientAuth| supported.iter().any(|m| m == method.as_str());
    if let Some(method) = registered.and_then(ClientAuth::parse)
        && method != ClientAuth::None
        && (supported.is_empty() || listed(method))
    {
        return method;
    }
    if !listed(ClientAuth::Post) && listed(ClientAuth::Basic) {
        return ClientAuth::Basic;
    }
    ClientAuth::Post
}

/// The `Authorization` header for `client_secret_basic` — RFC 6749 §2.3.1:
/// each half form-encoded, then the pair base64'd.
#[must_use]
pub fn basic_credentials(client_id: &str, client_secret: &str) -> String {
    let pair = format!("{}:{}", url_encode(client_id), url_encode(client_secret));
    format!("Basic {}", crate::clipboard::base64_encode(pair.as_bytes()))
}

/// The client credentials one token request presents: the body pairs, and
/// the Basic header when that is the method.
fn client_auth_parts(
    client_id: &str,
    client_secret: Option<&str>,
    method: ClientAuth,
) -> (Vec<(String, String)>, Option<String>) {
    let mut params = vec![("client_id".to_string(), client_id.to_string())];
    match (method, client_secret) {
        (ClientAuth::Basic, Some(secret)) => (params, Some(basic_credentials(client_id, secret))),
        (ClientAuth::Post, Some(secret)) => {
            params.push(("client_secret".to_string(), secret.to_string()));
            (params, None)
        }
        _ => (params, None),
    }
}

/// Where a client identity came from — the error text and the cleanup on a
/// rejected client depend on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientSource {
    /// The server's `oauth.clientId` in the config file.
    Configured,
    /// Typed on the `/mcp` auth page.
    Entered,
    /// A dynamic registration made earlier.
    Registered,
}

/// The client a flow will present.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientCredentials {
    pub client_id: String,
    pub client_secret: Option<String>,
    /// The token-endpoint method a registration settled, if any.
    pub auth_method: Option<String>,
    pub source: ClientSource,
}

/// What the flow does for a client identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientPlan {
    /// Present these credentials.
    Use(ClientCredentials),
    /// Register one dynamically at this endpoint.
    Register(String),
}

/// What the user must supply for a flow to go on — the spec's last
/// registration step, "prompt the user to enter the client information".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientRequest {
    /// The authorization server, as its issuer names itself — the page says
    /// who issues the client.
    pub issuer: String,
    /// The configured client id, when only its secret is missing.
    pub client_id: Option<String>,
    /// The redirect URI the flow listens on — what a provider that matches
    /// redirect URIs exactly needs registered.
    pub redirect_uri: String,
}

/// Choose the client a flow presents, in the spec's registration order:
/// pre-registered (the config's `clientId`, or credentials the user entered
/// for this issuer) → a dynamic registration already made on this exact
/// redirect URI → a new dynamic registration → ask the user. Client ID
/// Metadata Documents would sit before dynamic registration, and need a
/// hosted document this client does not have (`docs/mcp.md`).
pub fn plan_client(
    config: Option<&crate::mcp::McpOAuthConfig>,
    stored: Option<&StoredClient>,
    meta: &AuthServerMeta,
    redirect_uri: &str,
) -> Result<ClientPlan, ClientRequest> {
    let issuer = meta.issuer.clone().unwrap_or_default();
    let request = |client_id: Option<String>| ClientRequest {
        issuer: issuer.clone(),
        client_id,
        redirect_uri: redirect_uri.to_string(),
    };
    // Stored credentials are only ever presented to the server that issued
    // them (SEP-2352) — unknown on either side passes, nothing to compare.
    let same_issuer = |client: &StoredClient| match (&client.issuer, &meta.issuer) {
        (Some(stored), Some(current)) => issuers_match(stored, current),
        _ => true,
    };
    if let Some(client_id) = config.and_then(|c| c.client_id.clone()) {
        let secret = config.and_then(|c| c.client_secret.clone()).or_else(|| {
            stored
                .filter(|s| s.client_id == client_id && s.redirect_uri.is_none() && same_issuer(s))
                .and_then(|s| s.client_secret.clone())
        });
        // A server that lists its methods and not `none` will refuse a
        // client with no secret — after the browser round, so ask first.
        let public_ok =
            meta.auth_methods.is_empty() || meta.auth_methods.iter().any(|m| m == "none");
        if secret.is_none() && !public_ok {
            return Err(request(Some(client_id)));
        }
        return Ok(ClientPlan::Use(ClientCredentials {
            client_id,
            client_secret: secret,
            auth_method: None,
            source: ClientSource::Configured,
        }));
    }
    if let Some(client) = stored.filter(|s| same_issuer(s)) {
        let credentials = |source| {
            ClientPlan::Use(ClientCredentials {
                client_id: client.client_id.clone(),
                client_secret: client.client_secret.clone(),
                auth_method: client.auth_method.clone(),
                source,
            })
        };
        match &client.redirect_uri {
            None => return Ok(credentials(ClientSource::Entered)),
            Some(registered) if registered == redirect_uri => {
                return Ok(credentials(ClientSource::Registered));
            }
            Some(_) => {} // another port: register afresh below
        }
    }
    match &meta.registration_endpoint {
        Some(endpoint) => Ok(ClientPlan::Register(endpoint.clone())),
        None => Err(request(None)),
    }
}

/// Authorization parameters a provider needs beyond the standard ones.
/// Google issues refresh tokens only for `access_type=offline` — it lists
/// no `offline_access` scope — and re-issues one only with `prompt=consent`
/// (its MCP setup guides say both), so without them a web client's grant
/// expires within the hour with nothing to renew it.
#[must_use]
pub fn provider_authorize_params(issuer: Option<&str>) -> &'static [(&'static str, &'static str)] {
    match issuer {
        Some(issuer) if issuers_match(issuer, "https://accounts.google.com") => {
            &[("access_type", "offline"), ("prompt", "consent")]
        }
        _ => &[],
    }
}

// ---------------------------------------------------------------------------
// the interactive flow

/// What the flow reports as it goes — the manager forwards these onto the
/// MCP event channel for the auth page.
pub trait AuthProgress: Send {
    /// The authorize URL is ready (shown + copied from the auth page).
    fn on_url(&self, url: &str);
}

/// The redirect port a pre-registered client's flow listens on when its
/// config fixes none. Fixed rather than ephemeral so a provider that matches
/// redirect URIs exactly — a Google *Web application* client, which is what
/// Google's Workspace MCP guides prescribe — can have
/// `http://localhost:8976/callback` registered once (`docs/mcp.md`).
pub const DEFAULT_CALLBACK_PORT: u16 = 8976;

/// What a flow needs beyond the server URL: the server's config, and the
/// boundary's default port.
#[derive(Debug, Clone, Default)]
pub struct AuthSettings {
    /// The server's `oauth` object from the config file.
    pub oauth: Option<crate::mcp::McpOAuthConfig>,
    /// The port a pre-registered client listens on when the config fixes
    /// none — [`DEFAULT_CALLBACK_PORT`]; `0` takes any free port.
    pub default_port: u16,
}

/// Why a flow ended without a grant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthError {
    /// No client to present — nothing configured or entered, and the server
    /// registers none: the `/mcp` auth page asks the user for one.
    NeedsClient(ClientRequest),
    /// Anything else, worded for the toast.
    Failed(String),
}

impl From<String> for AuthError {
    fn from(detail: String) -> Self {
        Self::Failed(detail)
    }
}

impl std::fmt::Display for AuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NeedsClient(request) => write!(
                f,
                "{} needs an OAuth client registered with it",
                request.issuer
            ),
            Self::Failed(detail) => f.write_str(detail),
        }
    }
}

/// Which port the callback listener binds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CallbackPort {
    /// The config's `callbackPort`: that port or nothing — the provider was
    /// told this redirect URI.
    Exactly(u16),
    /// A stable port worth keeping when free, any port otherwise.
    Prefer(u16),
    Any,
}

fn bind_callback(port: CallbackPort) -> Result<TcpListener, String> {
    let any = || {
        TcpListener::bind("127.0.0.1:0")
            .map_err(|e| format!("could not open the callback port: {e}"))
    };
    match port {
        CallbackPort::Exactly(port) => TcpListener::bind(("127.0.0.1", port))
            .map_err(|e| format!("the callback port {port} (oauth.callbackPort) is not free: {e}")),
        CallbackPort::Prefer(port) if port != 0 => {
            TcpListener::bind(("127.0.0.1", port)).or_else(|_| any())
        }
        CallbackPort::Prefer(_) | CallbackPort::Any => any(),
    }
}

/// The port of a `http://localhost:{port}/callback` redirect URI.
fn redirect_port(redirect_uri: &str) -> Option<u16> {
    url::Url::parse(redirect_uri).ok()?.port()
}

/// Run the whole interactive flow for `server_url`, blocking this (worker)
/// thread: discovery → a client (configured, entered, registered, or the
/// user asked — [`plan_client`]) → browser/paste → exchange → persist.
/// `paste_rx` delivers the user's pasted redirect URL; `open_browser` is the
/// boundary's launcher (injected so tests need no display).
#[allow(clippy::too_many_arguments)] // the flow's whole context, each one injected
pub fn run_auth_flow(
    server_url: &str,
    challenge: Option<&str>,
    store_path: Option<&Path>,
    settings: &AuthSettings,
    paste_rx: &mpsc::Receiver<String>,
    progress: &dyn AuthProgress,
    open_browser: &dyn Fn(&str),
    cancel: &CancelToken,
) -> Result<(), AuthError> {
    let config = settings.oauth.as_ref();
    let meta = discover(server_url, challenge, config)?;
    let stored = load_client(store_path, server_url);
    // The callback port: the config's, else a pre-registered client's stable
    // default, else the port a registration named (so it stays usable),
    // else any — a fresh registration names whichever it gets.
    let configured_id = config.and_then(|c| c.client_id.as_ref()).is_some();
    let entered = stored.as_ref().is_some_and(|s| s.redirect_uri.is_none());
    let port = match config.and_then(|c| c.callback_port) {
        Some(port) => CallbackPort::Exactly(port),
        None if configured_id || entered => CallbackPort::Prefer(settings.default_port),
        None => stored
            .as_ref()
            .and_then(|s| s.redirect_uri.as_deref())
            .and_then(redirect_port)
            .map_or(CallbackPort::Any, CallbackPort::Prefer),
    };
    let listener = bind_callback(port)?;
    let bound = listener
        .local_addr()
        .map_err(|e| format!("callback port: {e}"))?
        .port();
    let redirect_uri = format!("http://localhost:{bound}/callback");
    let client = match plan_client(config, stored.as_ref(), &meta, &redirect_uri) {
        Ok(ClientPlan::Use(client)) => client,
        Ok(ClientPlan::Register(endpoint)) => {
            let registration = register_client(&endpoint, &redirect_uri)?;
            save_client(
                store_path,
                server_url,
                Some(&StoredClient {
                    client_id: registration.client_id.clone(),
                    client_secret: registration.client_secret.clone(),
                    issuer: meta.issuer.clone(),
                    redirect_uri: Some(redirect_uri.clone()),
                    auth_method: registration.auth_method.clone(),
                }),
            );
            ClientCredentials {
                client_id: registration.client_id,
                client_secret: registration.client_secret,
                auth_method: registration.auth_method,
                source: ClientSource::Registered,
            }
        }
        Err(mut request) => {
            // The page asks for a client the *next* attempt will present, and
            // that attempt listens where a pre-registered client does.
            let next = config
                .and_then(|c| c.callback_port)
                .unwrap_or(settings.default_port);
            if next != 0 {
                request.redirect_uri = format!("http://localhost:{next}/callback");
            }
            return Err(AuthError::NeedsClient(request));
        }
    };
    let method = choose_client_auth(
        &meta.auth_methods,
        client.client_secret.is_some(),
        client.auth_method.as_deref(),
    );
    // Discovery and registration take seconds; a flow the user backed out of
    // meanwhile must not open a browser.
    if cancel.is_cancelled() {
        return Err(AuthError::Failed("cancelled".to_string()));
    }
    let (verifier, code_challenge, state) = pkce_and_state();
    let authorize_url = build_authorize_url(
        &meta.authorization_endpoint,
        &client.client_id,
        &redirect_uri,
        &code_challenge,
        &state,
        meta.scopes.as_deref(),
        server_url,
        provider_authorize_params(meta.issuer.as_deref()),
    );
    progress.on_url(&authorize_url);
    open_browser(&authorize_url);
    let code = wait_for_code(&listener, &state, &meta, paste_rx, cancel)?;
    let (mut params, basic) =
        client_auth_parts(&client.client_id, client.client_secret.as_deref(), method);
    params.extend([
        ("grant_type".to_string(), "authorization_code".to_string()),
        ("code".to_string(), code),
        ("redirect_uri".to_string(), redirect_uri),
        ("code_verifier".to_string(), verifier),
        ("resource".to_string(), server_url.to_string()),
    ]);
    let (status, body) = post_form_raw(&meta.token_endpoint, &params, basic.as_deref())
        .map_err(|e| format!("token exchange failed: {e}"))?;
    if !(200..300).contains(&status) {
        return Err(AuthError::Failed(rejected_exchange(
            status, &body, &client, store_path, server_url,
        )));
    }
    let response: Value =
        serde_json::from_str(&body).map_err(|e| format!("token exchange failed: bad JSON: {e}"))?;
    let prior = StoredTokens {
        client_id: client.client_id,
        client_secret: client.client_secret,
        token_endpoint: meta.token_endpoint.clone(),
        scope: meta.scopes.clone(),
        auth_method: (method != ClientAuth::None).then(|| method.as_str().to_string()),
        ..Default::default()
    };
    let tokens = tokens_from_response(&response, &prior)
        .ok_or_else(|| "token response carried no access_token".to_string())?;
    save_tokens(store_path, server_url, Some(&tokens));
    Ok(())
}

/// Word a refused code exchange. `invalid_client` is a verdict on the
/// client, not the login, so a client this app stored — typed credentials,
/// or a registration — is forgotten and the next attempt starts fresh; a
/// configured one points at the config.
fn rejected_exchange(
    status: u16,
    body: &str,
    client: &ClientCredentials,
    store_path: Option<&Path>,
    server_url: &str,
) -> String {
    let error = serde_json::from_str::<Value>(body.trim())
        .ok()
        .and_then(|v| v.get("error").and_then(Value::as_str).map(str::to_string));
    if error.as_deref() == Some("invalid_client") {
        return match client.source {
            ClientSource::Configured => "the authorization server rejected the configured \
                 client (invalid_client) — check the server's oauth.clientId and its secret"
                .to_string(),
            ClientSource::Entered => {
                save_client(store_path, server_url, None);
                "the authorization server rejected the client ID or secret \
                 (invalid_client) — authenticate again to enter them"
                    .to_string()
            }
            ClientSource::Registered => {
                save_client(store_path, server_url, None);
                "the authorization server no longer accepts this app's registration \
                 (invalid_client) — authenticate again to register afresh"
                    .to_string()
            }
        };
    }
    let flat: String = body.split_whitespace().collect::<Vec<_>>().join(" ");
    let flat: String = flat.chars().take(200).collect();
    format!("token exchange failed: HTTP {status}: {flat}")
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
    let mut pending: Vec<PendingRequest> = Vec::new();
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
                if let Some(verdict) = callback_verdict(&text, state, meta, "pasted URL") {
                    return verdict;
                }
                // Not parseable — keep waiting; the page shows the format.
            }
            Err(mpsc::TryRecvError::Empty | mpsc::TryRecvError::Disconnected) => {}
        }
        // Take every connection the browser opened, then service each without
        // blocking: a speculative preconnect sends nothing, and a blocking
        // read on it froze this loop — the real callback, the paste field and
        // Esc with it.
        loop {
            match listener.accept() {
                Ok((stream, _)) => {
                    if stream.set_nonblocking(true).is_ok() {
                        pending.push(PendingRequest::new(stream));
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) => return Err(format!("callback port: {e}")),
            }
        }
        let mut index = 0;
        while index < pending.len() {
            match pending[index].poll() {
                RequestRead::Waiting if pending[index].opened.elapsed() < IDLE_CONNECTION => {
                    index += 1;
                }
                RequestRead::Waiting | RequestRead::Closed => {
                    pending.swap_remove(index);
                }
                RequestRead::Line(path) => {
                    let request = pending.swap_remove(index);
                    let verdict = callback_verdict(&path, state, meta, "callback");
                    // The browser hears the verdict, never a guess made
                    // before the state and `iss` were checked.
                    let page = match &verdict {
                        Some(Ok(_)) => CALLBACK_COMPLETE_PAGE,
                        Some(Err(_)) => CALLBACK_FAILED_PAGE,
                        None => CALLBACK_WAITING_PAGE,
                    };
                    request.respond(page);
                    if let Some(verdict) = verdict {
                        return verdict;
                    }
                }
            }
        }
        std::thread::sleep(POLL);
    }
}

/// How long a connection may sit on the callback port without sending its
/// request line before it is dropped — a preconnect the browser never used.
const IDLE_CONNECTION: Duration = Duration::from_secs(30);

/// The pages the browser lands on. None of them echo anything the
/// authorization server sent: the terminal has the details.
const CALLBACK_COMPLETE_PAGE: &str = "<html><body><h3>Authentication complete.</h3>You can close this tab and return to the terminal.</body></html>";
const CALLBACK_FAILED_PAGE: &str = "<html><body><h3>Authentication failed.</h3>Return to the terminal for the reason.</body></html>";
const CALLBACK_WAITING_PAGE: &str =
    "<html><body><h3>Waiting for authentication…</h3></body></html>";

/// One connection on the callback port, read until its request line.
struct PendingRequest {
    stream: std::net::TcpStream,
    buffer: Vec<u8>,
    opened: Instant,
}

/// What one non-blocking read of a [`PendingRequest`] found.
enum RequestRead {
    /// Nothing complete yet.
    Waiting,
    /// The peer went away (or the read failed) before a request line.
    Closed,
    /// The request target of a complete request line.
    Line(String),
}

impl PendingRequest {
    /// The request line must fit in this much — a redirect with a long code
    /// and an `iss` is a few hundred bytes.
    const MAX_HEAD: usize = 16 * 1024;

    fn new(stream: std::net::TcpStream) -> Self {
        Self {
            stream,
            buffer: Vec::new(),
            opened: Instant::now(),
        }
    }

    fn poll(&mut self) -> RequestRead {
        let mut chunk = [0u8; 4096];
        loop {
            if let Some(end) = self.buffer.windows(2).position(|w| w == b"\r\n") {
                let line = String::from_utf8_lossy(&self.buffer[..end]).into_owned();
                return match line.split_whitespace().nth(1) {
                    Some(target) => RequestRead::Line(target.to_string()),
                    None => RequestRead::Closed,
                };
            }
            if self.buffer.len() > Self::MAX_HEAD {
                return RequestRead::Closed;
            }
            match self.stream.read(&mut chunk) {
                Ok(0) => return RequestRead::Closed,
                Ok(read) => self.buffer.extend_from_slice(&chunk[..read]),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    return RequestRead::Waiting;
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => return RequestRead::Closed,
            }
        }
    }

    /// Answer with `page` and close — best-effort, bounded, blocking.
    fn respond(mut self, page: &str) {
        let _ = self.stream.set_nonblocking(false);
        let _ = self.stream.set_write_timeout(Some(Duration::from_secs(2)));
        let _ = self.stream.write_all(
            format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{page}",
                page.len()
            )
            .as_bytes(),
        );
    }
}

/// What one redirect (a callback path or a pasted URL) settles, if
/// anything: the code, or the end of the flow. `None` keeps waiting — a
/// request that isn't the redirect, or a refusal carrying another flow's
/// `state` (a stale tab must not end this one). The RFC 9207 `iss` check
/// runs before a code is redeemed **and** before a refusal is acted on: an
/// error from an unvalidated server is not shown (2026-07-28 authorization).
fn callback_verdict(
    text: &str,
    state: &str,
    meta: &AuthServerMeta,
    source: &str,
) -> Option<Result<String, String>> {
    let check_issuer = || {
        validate_issuer(
            redirect_param(text, "iss").as_deref(),
            meta.issuer.as_deref(),
            meta.iss_required,
        )
    };
    match parse_callback(text) {
        Callback::Code {
            code,
            state: got_state,
        } => {
            if got_state.as_deref().is_some_and(|got| got != state) {
                return Some(Err(format!("the {source}'s state doesn't match")));
            }
            Some(check_issuer().map(|()| code))
        }
        Callback::Denied {
            error,
            description,
            state: got_state,
        } => {
            if got_state.as_deref().is_some_and(|got| got != state) {
                return None;
            }
            Some(
                check_issuer()
                    .and_then(|()| Err(Callback::refusal(&error, description.as_deref()))),
            )
        }
        Callback::Other => None,
    }
}

/// Test fixtures shared with the manager's tests.
#[cfg(test)]
pub(super) mod fixtures {
    use super::*;

    /// One token request the fixture's `/token` received.
    #[derive(Debug, Clone)]
    pub(in crate::llm::mcp) struct TokenRequest {
        pub authorization: Option<String>,
        pub form: std::collections::BTreeMap<String, String>,
    }

    /// A Google-shaped authorization server on one socket: resource
    /// metadata naming the AS **with** a trailing slash, AS metadata whose
    /// issuer has none, no registration endpoint, `iss` required on every
    /// redirect, and a token endpoint that takes only the listed client
    /// authentication methods and answers `invalid_client` otherwise.
    pub(in crate::llm::mcp) struct AuthFixture {
        pub base: String,
        pub server_url: String,
        pub token_requests: std::sync::Arc<std::sync::Mutex<Vec<TokenRequest>>>,
    }

    pub(in crate::llm::mcp) fn spawn_auth_fixture(
        methods: &'static [&'static str],
        secret: &'static str,
    ) -> AuthFixture {
        use std::io::{BufRead, BufReader};
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let token_requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let (base_t, requests_t) = (base.clone(), std::sync::Arc::clone(&token_requests));
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                if reader.read_line(&mut line).is_err() {
                    continue;
                }
                let path = line.split_whitespace().nth(1).unwrap_or("").to_string();
                let (mut length, mut authorization) = (0usize, None);
                loop {
                    let mut header = String::new();
                    if reader.read_line(&mut header).is_err() || header.trim().is_empty() {
                        break;
                    }
                    let (name, value) = header.split_once(':').unwrap_or_default();
                    match name.to_ascii_lowercase().as_str() {
                        "content-length" => length = value.trim().parse().unwrap_or(0),
                        "authorization" => authorization = Some(value.trim().to_string()),
                        _ => {}
                    }
                }
                let mut body = vec![0u8; length];
                let _ = reader.read_exact(&mut body);
                let body = String::from_utf8_lossy(&body).to_string();
                let json = |status: &str, body: String| {
                    format!(
                        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                };
                let response = if path == "/.well-known/oauth-protected-resource/mcp" {
                    json(
                        "200 OK",
                        serde_json::json!({
                            "resource": format!("{base_t}/mcp"),
                            "authorization_servers": [format!("{base_t}/")],
                            "scopes_supported": ["calendar"],
                        })
                        .to_string(),
                    )
                } else if path == "/.well-known/oauth-authorization-server" {
                    json(
                        "200 OK",
                        serde_json::json!({
                            "issuer": base_t,
                            "authorization_endpoint": format!("{base_t}/authorize"),
                            "token_endpoint": format!("{base_t}/token"),
                            "token_endpoint_auth_methods_supported": methods,
                            "code_challenge_methods_supported": ["S256"],
                            "authorization_response_iss_parameter_supported": true,
                        })
                        .to_string(),
                    )
                } else if path == "/token" {
                    let form: std::collections::BTreeMap<String, String> = body
                        .split('&')
                        .filter_map(|pair| pair.split_once('='))
                        .map(|(k, v)| (url_decode(k), url_decode(v)))
                        .collect();
                    requests_t.lock().unwrap().push(TokenRequest {
                        authorization: authorization.clone(),
                        form: form.clone(),
                    });
                    let by_basic = authorization.as_deref()
                        == Some(basic_credentials("cid", secret).as_str())
                        && methods.contains(&"client_secret_basic");
                    let by_post = form.get("client_secret").map(String::as_str) == Some(secret)
                        && methods.contains(&"client_secret_post");
                    if (by_basic || by_post) && form.contains_key("code_verifier") {
                        json(
                            "200 OK",
                            r#"{"access_token":"at-1","refresh_token":"rt-1","expires_in":3599}"#
                                .to_string(),
                        )
                    } else {
                        json(
                            "401 Unauthorized",
                            r#"{"error":"invalid_client"}"#.to_string(),
                        )
                    }
                } else {
                    json("404 Not Found", "{}".to_string())
                };
                let _ = stream.write_all(response.as_bytes());
            }
        });
        AuthFixture {
            server_url: format!("{base}/mcp"),
            base,
            token_requests,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::*;
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
    fn callbacks_parse_from_urls_paths_and_queries() {
        let code = |code: &str, state: Option<&str>| Callback::Code {
            code: code.to_string(),
            state: state.map(str::to_string),
        };
        assert_eq!(
            parse_callback("http://localhost:1/callback?code=abc&state=s1"),
            code("abc", Some("s1"))
        );
        assert_eq!(parse_callback("/callback?code=a%2Fb"), code("a/b", None));
        assert_eq!(parse_callback("code=xyz&state=q"), code("xyz", Some("q")));
        // A parameter with no `=` (a trailing `&`) costs nothing.
        assert_eq!(
            parse_callback("/callback?code=abc&state=s1&"),
            code("abc", Some("s1"))
        );
        assert_eq!(parse_callback("plain text"), Callback::Other);
        // The sign-ins' narrower reading: a code, or nothing.
        assert_eq!(
            parse_redirect("/callback?code=abc&state=s1"),
            Some(("abc".to_string(), Some("s1".to_string())))
        );
        assert_eq!(parse_redirect("https://x/cb?error=denied"), None);
    }

    #[test]
    fn a_callback_names_its_code_its_refusal_or_nothing() {
        assert_eq!(
            parse_callback("/callback?code=abc&state=s1"),
            Callback::Code {
                code: "abc".to_string(),
                state: Some("s1".to_string()),
            }
        );
        // RFC 6749 §4.1.2.1: a refused authorization comes back as `error`
        // (Cancel on the consent screen is `access_denied`) — a verdict, not
        // a stray request to wait past.
        assert_eq!(
            parse_callback(
                "/callback?error=access_denied&error_description=The+user+said+no&state=s1"
            ),
            Callback::Denied {
                error: "access_denied".to_string(),
                description: Some("The user said no".to_string()),
                state: Some("s1".to_string()),
            }
        );
        assert_eq!(parse_callback("/favicon.ico"), Callback::Other);
        assert_eq!(parse_callback("/callback"), Callback::Other);
    }

    #[test]
    fn a_denied_consent_ends_the_wait_instead_of_the_timeout() {
        // Cancel on the consent screen used to read as "not the callback",
        // so the flow sat on the port for the whole five-minute budget.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (_paste_tx, paste_rx) = mpsc::channel();
        std::thread::spawn(move || browser_get(port, "/callback?error=access_denied&state=st"));
        let started = Instant::now();
        let err = wait_for_code(
            &listener,
            "st",
            &bare_meta(),
            &paste_rx,
            &CancelToken::new(),
        )
        .expect_err("a refusal ends the flow");
        assert!(err.contains("access_denied"), "{err}");
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    /// `GET {path}` against the loopback port; returns the response page.
    fn browser_get(port: u16, path: &str) -> String {
        let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream
            .write_all(format!("GET {path} HTTP/1.1\r\nHost: localhost\r\n\r\n").as_bytes())
            .unwrap();
        let mut page = String::new();
        let _ = stream.read_to_string(&mut page);
        page
    }

    fn bare_meta() -> AuthServerMeta {
        AuthServerMeta::default()
    }

    #[test]
    fn an_idle_connection_never_holds_up_the_callback() {
        // Browsers open speculative connections to a page's origin and may
        // send nothing on them. A blocking read on one froze the whole wait
        // — the real callback, the paste field and Esc included.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let idle = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(3));
            drop(idle);
        });
        let browser = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            browser_get(port, "/callback?code=abc&state=st")
        });
        let (_paste_tx, paste_rx) = mpsc::channel();
        let started = Instant::now();
        let code = wait_for_code(
            &listener,
            "st",
            &bare_meta(),
            &paste_rx,
            &CancelToken::new(),
        );
        assert_eq!(code.as_deref(), Ok("abc"));
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "the idle socket held the callback up for {:?}",
            started.elapsed()
        );
        assert!(browser.join().unwrap().contains("Authentication complete"));
    }

    #[test]
    fn the_browser_hears_complete_only_for_a_code_the_flow_accepted() {
        // The page used to say "complete" before the state was checked, so a
        // forged or stale redirect read as a success in the browser while
        // the terminal reported the failure.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let browser =
            std::thread::spawn(move || browser_get(port, "/callback?code=abc&state=forged"));
        let (_paste_tx, paste_rx) = mpsc::channel();
        let err = wait_for_code(
            &listener,
            "st",
            &bare_meta(),
            &paste_rx,
            &CancelToken::new(),
        )
        .expect_err("a mismatched state ends the flow");
        assert!(err.contains("state"), "{err}");
        let page = browser.join().unwrap();
        assert!(!page.contains("Authentication complete"), "{page}");
        assert!(page.contains("Authentication failed"), "{page}");
    }

    fn meta_with(methods: &[&str], registration: Option<&str>) -> AuthServerMeta {
        AuthServerMeta {
            issuer: Some("https://as.test".to_string()),
            registration_endpoint: registration.map(str::to_string),
            auth_methods: methods.iter().map(|m| (*m).to_string()).collect(),
            ..bare_meta()
        }
    }

    fn configured(id: Option<&str>, secret: Option<&str>) -> crate::mcp::McpOAuthConfig {
        crate::mcp::McpOAuthConfig {
            client_id: id.map(str::to_string),
            client_secret: secret.map(str::to_string),
            ..Default::default()
        }
    }

    #[test]
    fn the_token_endpoint_hears_the_client_the_way_its_server_takes_it() {
        use ClientAuth::{Basic, None as Public, Post};
        let methods = |list: &[&str]| list.iter().map(|m| (*m).to_string()).collect::<Vec<_>>();
        // No secret, no client authentication: a public client.
        assert_eq!(
            choose_client_auth(&methods(&["client_secret_basic"]), false, None),
            Public
        );
        // The body is what every OAuth 2.1 server MUST accept, and what an
        // unlisted method defaults to.
        assert_eq!(choose_client_auth(&[], true, None), Post);
        assert_eq!(
            choose_client_auth(
                &methods(&["client_secret_post", "client_secret_basic"]),
                true,
                None
            ),
            Post
        );
        // A server that takes Basic alone gets Basic — sending the secret in
        // the body there was an `invalid_client` that cleared the grant.
        assert_eq!(
            choose_client_auth(&methods(&["client_secret_basic"]), true, None),
            Basic
        );
        // What a registration settled wins while the server still lists it.
        assert_eq!(
            choose_client_auth(
                &methods(&["client_secret_post", "client_secret_basic"]),
                true,
                Some("client_secret_basic")
            ),
            Basic
        );
    }

    #[test]
    fn basic_client_authentication_form_encodes_the_credentials() {
        // RFC 6749 §2.3.1: each half is form-encoded before the base64.
        assert_eq!(
            basic_credentials("a b", "s:/"),
            format!(
                "Basic {}",
                crate::clipboard::base64_encode(b"a%20b:s%3A%2F")
            )
        );
    }

    #[test]
    fn a_configured_client_comes_first_with_its_stored_secret() {
        let stored = StoredClient {
            client_id: "cid".to_string(),
            client_secret: Some("from-store".to_string()),
            ..Default::default()
        };
        let plan = plan_client(
            Some(&configured(Some("cid"), None)),
            Some(&stored),
            &meta_with(&["client_secret_post"], Some("https://as.test/register")),
            "http://localhost:1/callback",
        );
        assert_eq!(
            plan,
            Ok(ClientPlan::Use(ClientCredentials {
                client_id: "cid".to_string(),
                client_secret: Some("from-store".to_string()),
                auth_method: None,
                source: ClientSource::Configured,
            }))
        );
        // A secret stored for another client id is not this client's.
        let other = StoredClient {
            client_id: "old".to_string(),
            ..stored.clone()
        };
        assert!(matches!(
            plan_client(
                Some(&configured(Some("cid"), Some("in-config"))),
                Some(&other),
                &meta_with(&[], None),
                "r",
            ),
            Ok(ClientPlan::Use(ClientCredentials { client_secret: Some(s), .. })) if s == "in-config"
        ));
    }

    #[test]
    fn a_configured_client_without_the_secret_its_server_needs_asks_for_it() {
        // Google's token endpoint takes client_secret_post/basic only: a
        // client id alone would fail at the exchange, after the browser.
        let plan = plan_client(
            Some(&configured(Some("cid"), None)),
            None,
            &meta_with(&["client_secret_post", "client_secret_basic"], None),
            "http://localhost:8976/callback",
        );
        assert_eq!(
            plan,
            Err(ClientRequest {
                issuer: "https://as.test".to_string(),
                client_id: Some("cid".to_string()),
                redirect_uri: "http://localhost:8976/callback".to_string(),
            })
        );
        // A server that takes public clients needs no secret.
        assert!(matches!(
            plan_client(
                Some(&configured(Some("cid"), None)),
                None,
                &meta_with(&["none", "client_secret_post"], None),
                "r",
            ),
            Ok(ClientPlan::Use(ClientCredentials {
                client_secret: None,
                ..
            }))
        ));
    }

    #[test]
    fn with_nothing_to_use_or_register_the_user_is_asked() {
        // The spec's last registration step — the Google Calendar dead end.
        assert_eq!(
            plan_client(
                None,
                None,
                &meta_with(&["client_secret_post"], None),
                "redir"
            ),
            Err(ClientRequest {
                issuer: "https://as.test".to_string(),
                client_id: None,
                redirect_uri: "redir".to_string(),
            })
        );
        // Dynamic registration when the server offers it.
        assert_eq!(
            plan_client(
                None,
                None,
                &meta_with(&[], Some("https://as.test/reg")),
                "r"
            ),
            Ok(ClientPlan::Register("https://as.test/reg".to_string()))
        );
    }

    #[test]
    fn stored_credentials_belong_to_the_server_that_issued_them() {
        let entered = StoredClient {
            client_id: "typed".to_string(),
            client_secret: Some("s".to_string()),
            issuer: Some("https://as.test".to_string()),
            ..Default::default()
        };
        assert!(matches!(
            plan_client(None, Some(&entered), &meta_with(&[], None), "r"),
            Ok(ClientPlan::Use(ClientCredentials {
                source: ClientSource::Entered,
                ..
            }))
        ));
        // SEP-2352: credentials are keyed by issuer — never presented to an
        // authorization server that did not issue them.
        let moved = AuthServerMeta {
            issuer: Some("https://elsewhere.test".to_string()),
            ..meta_with(&[], None)
        };
        assert!(matches!(
            plan_client(None, Some(&entered), &moved, "r"),
            Err(ClientRequest {
                client_id: None,
                ..
            })
        ));
        // A registration is reused only on the redirect URI it registered —
        // an exact-match server refuses any other.
        let registered = StoredClient {
            client_id: "dyn".to_string(),
            issuer: Some("https://as.test".to_string()),
            redirect_uri: Some("http://localhost:5000/callback".to_string()),
            ..Default::default()
        };
        let meta = meta_with(&[], Some("https://as.test/reg"));
        assert!(matches!(
            plan_client(
                None,
                Some(&registered),
                &meta,
                "http://localhost:5000/callback"
            ),
            Ok(ClientPlan::Use(ClientCredentials {
                source: ClientSource::Registered,
                ..
            }))
        ));
        assert_eq!(
            plan_client(
                None,
                Some(&registered),
                &meta,
                "http://localhost:6000/callback"
            ),
            Ok(ClientPlan::Register("https://as.test/reg".to_string()))
        );
    }

    #[test]
    fn issuers_compare_exactly_but_for_a_root_trailing_slash() {
        // Google's resource metadata names `https://accounts.google.com/`
        // while its metadata says `https://accounts.google.com` — the one
        // difference both official SDKs forgive.
        assert!(issuers_match(
            "https://accounts.google.com/",
            "https://accounts.google.com"
        ));
        assert!(issuers_match("https://a.test", "https://a.test"));
        assert!(!issuers_match(
            "https://a.test/tenant/",
            "https://a.test/tenant"
        ));
        assert!(!issuers_match("https://A.test", "https://a.test"));
        assert!(!issuers_match("https://a.test", "https://b.test"));
    }

    #[test]
    fn the_client_store_keeps_its_section_beside_the_grants() {
        let client = StoredClient {
            client_id: "cid".to_string(),
            client_secret: Some("s".to_string()),
            issuer: Some("https://as.test".to_string()),
            redirect_uri: Some("http://localhost:1/callback".to_string()),
            auth_method: Some("client_secret_basic".to_string()),
        };
        let tokens = StoredTokens {
            access_token: "at".to_string(),
            auth_method: Some("client_secret_basic".to_string()),
            ..Default::default()
        };
        let both = record_tokens(
            &record_client("", "https://s/mcp", Some(&client)),
            "https://s/mcp",
            Some(&tokens),
        );
        assert_eq!(
            parse_client_store(&both).get("https://s/mcp"),
            Some(&client)
        );
        assert_eq!(parse_token_store(&both).get("https://s/mcp"), Some(&tokens));
        // Clearing the grant keeps the client; clearing the client keeps the grant.
        let no_grant = record_tokens(&both, "https://s/mcp", None);
        assert!(parse_client_store(&no_grant).contains_key("https://s/mcp"));
        let no_client = record_client(&both, "https://s/mcp", None);
        assert!(parse_token_store(&no_client).contains_key("https://s/mcp"));
        assert!(parse_client_store(&no_client).is_empty());
    }

    #[test]
    fn google_is_asked_for_a_refresh_token_its_own_way() {
        // Google issues refresh tokens only for `access_type=offline` (it
        // lists no `offline_access` scope), and re-issues one only with
        // `prompt=consent` — without them a web client's grant dies hourly.
        let url = build_authorize_url(
            "https://accounts.google.com/o/oauth2/v2/auth",
            "cid",
            "http://localhost:8976/callback",
            "chal",
            "st",
            Some("https://www.googleapis.com/auth/calendar"),
            "https://calendarmcp.googleapis.com/mcp/v1",
            provider_authorize_params(Some("https://accounts.google.com")),
        );
        assert!(url.contains("access_type=offline"), "{url}");
        assert_eq!(url.matches("prompt=").count(), 1, "{url}");
        assert!(url.contains("prompt=consent"), "{url}");
        assert!(provider_authorize_params(Some("https://as.test")).is_empty());
    }

    struct Silent;
    impl AuthProgress for Silent {
        fn on_url(&self, _: &str) {}
    }

    /// The user's browser: follow the authorize URL straight back to its
    /// redirect URI with a code, the flow's `state`, and the issuer's `iss`.
    fn consenting_browser(issuer: String) -> impl Fn(&str) {
        move |authorize: &str| {
            let redirect = redirect_param(authorize, "redirect_uri").unwrap();
            let state = redirect_param(authorize, "state").unwrap();
            let port = redirect_port(&redirect).unwrap();
            let path = format!(
                "/callback?code=good&state={state}&iss={}",
                url_encode(&issuer)
            );
            std::thread::spawn(move || browser_get(port, &path));
        }
    }

    fn run_flow(
        fixture: &AuthFixture,
        store: &Path,
        oauth: Option<crate::mcp::McpOAuthConfig>,
    ) -> Result<(), AuthError> {
        let (_paste_tx, paste_rx) = mpsc::channel();
        run_auth_flow(
            &fixture.server_url,
            None,
            Some(store),
            &AuthSettings {
                oauth,
                default_port: 0,
            },
            &paste_rx,
            &Silent,
            &consenting_browser(fixture.base.clone()),
            &CancelToken::new(),
        )
    }

    #[test]
    fn a_configured_client_signs_in_where_nothing_can_be_registered() {
        // The Google Calendar case end to end: no registration endpoint, a
        // token endpoint taking Basic only, `iss` required, and the resource
        // metadata naming the issuer with a trailing slash.
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("mcp-auth.json");
        let fixture = spawn_auth_fixture(&["client_secret_basic"], "shh");
        let result = run_flow(&fixture, &store, Some(configured(Some("cid"), Some("shh"))));
        assert_eq!(result, Ok(()));
        let tokens = load_tokens(Some(&store), &fixture.server_url).expect("a stored grant");
        assert_eq!(tokens.access_token, "at-1");
        assert_eq!(tokens.client_id, "cid");
        assert_eq!(tokens.auth_method.as_deref(), Some("client_secret_basic"));
        let requests = fixture.token_requests.lock().unwrap().clone();
        assert_eq!(requests.len(), 1);
        // One method per request: the Basic header, no secret in the body.
        assert_eq!(
            requests[0].authorization.as_deref(),
            Some(basic_credentials("cid", "shh").as_str())
        );
        assert!(!requests[0].form.contains_key("client_secret"));
        assert_eq!(
            requests[0].form.get("grant_type").map(String::as_str),
            Some("authorization_code")
        );
    }

    #[test]
    fn with_no_client_to_present_the_flow_asks_for_one() {
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("mcp-auth.json");
        let fixture = spawn_auth_fixture(&["client_secret_post"], "shh");
        match run_flow(&fixture, &store, None) {
            Err(AuthError::NeedsClient(request)) => {
                assert_eq!(request.issuer, fixture.base);
                assert_eq!(request.client_id, None);
                assert!(request.redirect_uri.starts_with("http://localhost:"));
            }
            other => panic!("expected a client request, got {other:?}"),
        }
        assert!(fixture.token_requests.lock().unwrap().is_empty());
    }

    #[test]
    fn credentials_entered_for_a_server_are_used_and_a_rejected_pair_forgotten() {
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("mcp-auth.json");
        let fixture = spawn_auth_fixture(&["client_secret_post"], "shh");
        let entered = |secret: &str| StoredClient {
            client_id: "cid".to_string(),
            client_secret: Some(secret.to_string()),
            issuer: Some(fixture.base.clone()),
            ..Default::default()
        };
        save_client(Some(&store), &fixture.server_url, Some(&entered("wrong")));
        // A mistyped secret: the server says invalid_client, and the pair is
        // dropped so the next attempt asks again instead of failing forever.
        match run_flow(&fixture, &store, None) {
            Err(AuthError::Failed(detail)) => {
                assert!(detail.contains("invalid_client"), "{detail}")
            }
            other => panic!("expected the rejection, got {other:?}"),
        }
        assert_eq!(load_client(Some(&store), &fixture.server_url), None);
        save_client(Some(&store), &fixture.server_url, Some(&entered("shh")));
        assert_eq!(run_flow(&fixture, &store, None), Ok(()));
        let requests = fixture.token_requests.lock().unwrap().clone();
        assert_eq!(
            requests
                .last()
                .unwrap()
                .form
                .get("client_secret")
                .map(String::as_str),
            Some("shh")
        );
        assert_eq!(
            load_tokens(Some(&store), &fixture.server_url)
                .unwrap()
                .auth_method
                .as_deref(),
            Some("client_secret_post")
        );
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
            auth_method: None,
        };
        let (params, basic) = refresh_params(&tokens, "https://mcp.test/mcp");
        assert_eq!(basic, None);
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
        let (params, basic) = refresh_params(&confidential, "https://mcp.test/mcp");
        assert!(
            params
                .iter()
                .any(|(k, v)| k == "client_secret" && v == "shh")
        );
        assert_eq!(basic, None);
        // A grant obtained with Basic refreshes with Basic — and never also
        // puts the secret in the body (one method per request).
        let basic_grant = StoredTokens {
            auth_method: Some("client_secret_basic".to_string()),
            ..confidential
        };
        let (params, basic) = refresh_params(&basic_grant, "https://mcp.test/mcp");
        assert_eq!(basic, Some(basic_credentials("cid", "shh")));
        assert!(!params.iter().any(|(k, _)| k == "client_secret"));
        assert!(params.iter().any(|(k, v)| k == "client_id" && v == "cid"));
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
                auth_method: None,
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
                        auth_method: None,
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
            auth_method: None,
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
