//! The MCP OAuth flow (`docs/tools/mcp.md`): RFC 9728 protected-resource
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

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::Value;

use super::files::{FileLock, LockMiss, write_private};
use crate::llm::loopback::{self, CallbackServer, Expected, Pages};
use crate::stream::CancelToken;

/// The whole interactive flow's budget — Claude Code's 5 minutes.
pub const AUTH_FLOW_TIMEOUT: Duration = Duration::from_secs(300);

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
/// clock skew plus in-flight latency eat a short window). A token living
/// less than twice this is renewed at half its life instead
/// ([`tokens_fresh`]).
const EXPIRY_SKEW_SECS: u64 = 300;

/// How long a store write waits for another process's read-modify-write
/// before going ahead without the lock: a write takes milliseconds, so a
/// lock held this long is a stuck process — and a rotated refresh token must
/// never be dropped on the floor for one.
const STORE_LOCK_WAIT: Duration = Duration::from_secs(5);

/// How long a renewal waits for another session renewing the **same**
/// grant: past that session's whole token request, so the wait ends with
/// its grant in the store rather than with a replay.
const REFRESH_LOCK_WAIT: Duration = Duration::from_secs(25);

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
    /// The RFC 8707 resource indicator the grant was minted for — what its
    /// refreshes send too, so the token's audience never drifts. `None` in a
    /// grant stored before it was recorded: the server URL stands in.
    pub resource: Option<String>,
    /// Unix seconds the access token was minted — with `expires_at`, the
    /// token's lifetime, which sizes its renewal window. `None` in a grant
    /// stored before it was recorded: the full skew applies.
    pub issued_at: Option<u64>,
    /// The issuer of the authorization server the client was registered
    /// with — SEP-2352 binds the client credentials to it, so they are never
    /// presented to another. `None` in a grant stored before it was
    /// recorded: the token endpoint stands in.
    pub issuer: Option<String>,
    /// The redirect URI the client registered — the port a re-sign-in tries
    /// first, since an authorization server may match it exactly.
    pub redirect_uri: Option<String>,
    /// The client's registered `token_endpoint_auth_method` (`None`: a
    /// secret, if any, rides the form body).
    pub token_auth_method: Option<String>,
}

/// The store's own lock: `mcp-auth.lock` beside `mcp-auth.json`, held for a
/// read-modify-write.
fn store_lock_path(store: &Path) -> PathBuf {
    store.with_extension("lock")
}

/// One server's renewal lock, `mcp-auth.locks/{digest}.lock`, held across
/// the whole token request — per server, so a slow authorization server
/// never holds up another server's renewal in another session either.
fn refresh_lock_path(store: &Path, server_url: &str) -> PathBuf {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(server_url.as_bytes());
    let name: String = digest.iter().take(8).map(|b| format!("{b:02x}")).collect();
    store.with_extension("locks").join(format!("{name}.lock"))
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
                    issued_at: entry.get("issued_at").and_then(Value::as_u64),
                    issuer: text("issuer"),
                    redirect_uri: text("redirect_uri"),
                    token_auth_method: text("token_auth_method"),
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
                if let Some(resource) = &tokens.resource {
                    entry.insert("resource".into(), resource.clone().into());
                }
                if let Some(issued) = tokens.issued_at {
                    entry.insert("issued_at".into(), issued.into());
                }
                for (key, value) in [
                    ("issuer", &tokens.issuer),
                    ("redirect_uri", &tokens.redirect_uri),
                    ("token_auth_method", &tokens.token_auth_method),
                ] {
                    if let Some(value) = value {
                        entry.insert(key.into(), value.clone().into());
                    }
                }
                servers.insert(server_url.to_string(), Value::Object(entry));
            }
            None => {
                servers.shift_remove(server_url);
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

/// Persist (or delete) one server's tokens — a read-modify-write under the
/// store's lock, in-process and across processes. An error when there is
/// nowhere to keep a grant, or the store can't be read or written: a sign-in
/// that "succeeded" into nothing leaves the next connect without a token.
pub fn save_tokens(
    path: Option<&Path>,
    server_url: &str,
    tokens: Option<&StoredTokens>,
) -> Result<(), String> {
    let Some(path) = path else {
        return match tokens {
            Some(_) => Err("there is no config directory to keep the grant in".to_string()),
            None => Ok(()),
        };
    };
    rewrite_store(path, |contents| {
        Some(record_tokens(contents, server_url, tokens))
    })
}

/// Keep — or, with `None`, forget — a pre-registered client's secret for
/// `server_url` (`mcp add --client-secret`, `docs/tools/mcp.md`): in the store's
/// `clients` section, owner-only and never in `mcp.json`, and apart from the
/// login, so "Clear authentication" leaves the client the user configured.
///
/// # Errors
/// Nowhere to keep a secret, or a store that can't be read or written.
pub fn save_client_secret(
    path: Option<&Path>,
    server_url: &str,
    secret: Option<&str>,
) -> Result<(), String> {
    let Some(path) = path else {
        return match secret {
            Some(_) => Err("there is no config directory to keep the secret in".to_string()),
            None => Ok(()),
        };
    };
    rewrite_store(path, |contents| {
        let mut value: Value = serde_json::from_str(contents.trim())
            .ok()
            .filter(Value::is_object)
            .unwrap_or_else(|| Value::Object(Default::default()));
        let root = value.as_object_mut().expect("object ensured above");
        let clients = root
            .entry("clients")
            .or_insert_with(|| Value::Object(Default::default()));
        let clients = clients.as_object_mut()?;
        match secret {
            Some(secret) => {
                clients.insert(
                    server_url.to_string(),
                    serde_json::json!({ "client_secret": secret }),
                );
            }
            None => {
                clients.shift_remove(server_url);
            }
        }
        if clients.is_empty() {
            root.shift_remove("clients");
        }
        serde_json::to_string_pretty(&value).ok()
    })
}

/// The pre-registered client secret kept for `server_url`, if any.
#[must_use]
pub fn load_client_secret(path: Option<&Path>, server_url: &str) -> Option<String> {
    let contents = std::fs::read_to_string(path?).ok()?;
    let value: Value = serde_json::from_str(contents.trim()).ok()?;
    value
        .get("clients")?
        .get(server_url)?
        .get("client_secret")?
        .as_str()
        .filter(|secret| !secret.is_empty())
        .map(str::to_string)
}

/// Read-modify-write the store under its locks: `rewrite` gets the current
/// contents (empty when there is no file yet) and returns the new ones, or
/// `None` to leave the file alone. A store that exists but can't be read is
/// an error, never "empty" — rewriting it would drop every other server's
/// grant.
fn rewrite_store(path: &Path, rewrite: impl FnOnce(&str) -> Option<String>) -> Result<(), String> {
    let _guard = STORE_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    // Another process's write: wait for it, but never past a stuck one — a
    // dropped rotation costs more than a raced one.
    let _lock = FileLock::acquire(&store_lock_path(path), Instant::now() + STORE_LOCK_WAIT);
    let existing = match std::fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("couldn't read {}: {e}", path.display())),
    };
    let Some(updated) = rewrite(&existing) else {
        return Ok(());
    };
    write_private(path, &updated).map_err(|e| format!("couldn't write {}: {e}", path.display()))
}

/// Is this grant still fresh (with the refresh skew)? `now` is Unix seconds.
/// A token living less than twice the skew would be stale the moment it was
/// minted — every request a renewal, every renewal a rotation — so its
/// window is half its life instead.
#[must_use]
pub fn tokens_fresh(tokens: &StoredTokens, now: u64) -> bool {
    let Some(expires) = tokens.expires_at else {
        return true;
    };
    let skew = tokens.issued_at.map_or(EXPIRY_SKEW_SECS, |issued| {
        EXPIRY_SKEW_SECS.min(expires.saturating_sub(issued) / 2)
    });
    now.saturating_add(skew) < expires
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

/// The RFC 6749 §5.2 errors that are a verdict on the **grant itself** —
/// the refresh token is dead, the client is gone, or the grant may not be
/// renewed — so clearing it is the only way out of the loop. Every other
/// error (`invalid_request`, an AS's own `temporarily_unavailable`,
/// anything new) says nothing about the grant, and must not cost the user
/// one that may be perfectly good.
const PERMANENT_REFRESH_ERRORS: &[&str] = &[
    "invalid_grant",
    "invalid_client",
    "unauthorized_client",
    "unsupported_grant_type",
    "invalid_scope",
];

/// Classify a failed token-endpoint answer. `status` is `None` when the
/// request never got one (a network failure — transient by definition).
/// Only the AS's own structured verdict on a 400/401 — the RFC 6749 §5.2
/// `{"error": …}` shape, naming one of `PERMANENT_REFRESH_ERRORS` — is
/// permanent; an unparseable body at those statuses is a proxy or gateway
/// speaking, not the AS.
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
            let detail = format!("{error}{description}");
            if PERMANENT_REFRESH_ERRORS.contains(&error) {
                return RefreshFailure::Permanent(detail);
            }
            return RefreshFailure::Transient(format!("HTTP {status}: {detail}"));
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
    ];
    params.extend(client_authentication_of(tokens).0);
    params.push((
        "resource".to_string(),
        tokens
            .resource
            .clone()
            .unwrap_or_else(|| server_url.to_string()),
    ));
    params
}

/// Refresh the stored grant for `server_url` through its token endpoint —
/// forced, regardless of freshness (the reactive 401 path needs exactly
/// that). Single-flight per server, across threads **and** processes; the
/// store is re-read once the lock is held, so a grant another thread or
/// session already rotated is used instead of burning a spent refresh token.
/// The outcome is written back only over the grant this renewal presented
/// (`settle_refresh`): a rotation is persisted (`tokens_from_response`
/// keeps the old refresh token when the AS returns none — never overwrite
/// with nothing), and a [`RefreshFailure::Permanent`] rejection clears the
/// entry, because a dead grant kept around loops the failure into every
/// later request.
pub fn refresh_grant(
    path: Option<&Path>,
    server_url: &str,
    presented: Option<&str>,
) -> Result<StoredTokens, RefreshFailure> {
    // Single-flight, per server: only one thread may present this grant's
    // refresh token to the network at a time…
    let lock = refresh_lock_for(server_url);
    let _guard = lock
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    // …and only one session: another one renewing this grant holds the same
    // file's lock across its whole token request.
    let _session_lock = match path.map(|path| {
        FileLock::acquire(
            &refresh_lock_path(path, server_url),
            Instant::now() + REFRESH_LOCK_WAIT,
        )
    }) {
        Some(Err(LockMiss::TimedOut)) => {
            return Err(RefreshFailure::Transient(
                "another session is still renewing this grant".to_string(),
            ));
        }
        held => held,
    };
    let Some(tokens) = load_tokens(path, server_url) else {
        return Err(RefreshFailure::NoGrant);
    };
    // Re-read under the lock decides it: if the store no longer holds the
    // access token we found unusable, another thread or session already
    // rotated the grant while we waited. Take its result — presenting our
    // now-spent copy of the refresh token would be a replay, and a compliant
    // server answers that by revoking the whole family.
    if let Some(presented) = presented
        && tokens.access_token != presented
    {
        return Ok(tokens);
    }
    if tokens.refresh_token.is_none() || tokens.token_endpoint.trim().is_empty() {
        return Err(RefreshFailure::NoGrant);
    }
    let params = refresh_params(&tokens, server_url);
    let (_, basic) = client_authentication_of(&tokens);
    let outcome = match post_form_raw(&tokens.token_endpoint, &params, basic.as_deref()) {
        Err(detail) => Err(RefreshFailure::Transient(detail)),
        Ok((status, body)) if !(200..300).contains(&status) => {
            Err(refresh_failure_kind(Some(status), &body))
        }
        Ok((_, body)) => match serde_json::from_str::<Value>(&body) {
            Err(_) => Err(RefreshFailure::Transient(
                "the token endpoint answered non-JSON".to_string(),
            )),
            Ok(response) => tokens_from_response(&response, &tokens).ok_or_else(|| {
                RefreshFailure::Transient("the token response carried no access_token".to_string())
            }),
        },
    };
    settle_refresh(path, server_url, &tokens, outcome)
}

/// Record a renewal's outcome — but only over the grant it presented. The
/// store may have moved on while the request was out: "Clear
/// authentication" deleted the entry (the clear wins — a renewal never
/// brings a grant back, and the caller gets [`RefreshFailure::NoGrant`]), or
/// a sign-in stored a newer grant (it stays, and is what the caller gets).
/// A permanent rejection clears the entry on the same condition, so it can
/// never take a grant it didn't reject.
fn settle_refresh(
    path: Option<&Path>,
    server_url: &str,
    presented: &StoredTokens,
    outcome: Result<StoredTokens, RefreshFailure>,
) -> Result<StoredTokens, RefreshFailure> {
    let Some(path) = path else {
        return outcome;
    };
    let mut moved_on = None;
    let written = rewrite_store(path, |contents| {
        let current = parse_token_store(contents).remove(server_url);
        let ours = current
            .as_ref()
            .is_some_and(|current| current.refresh_token == presented.refresh_token);
        if !ours {
            moved_on = Some(current.ok_or(RefreshFailure::NoGrant));
            return None;
        }
        match &outcome {
            Ok(refreshed) => Some(record_tokens(contents, server_url, Some(refreshed))),
            Err(RefreshFailure::Permanent(_)) => Some(record_tokens(contents, server_url, None)),
            Err(_) => None,
        }
    });
    // A rotation that couldn't be written still gave this session a working
    // bearer; the next session finds the spent grant and signs in again.
    let _ = written;
    moved_on.unwrap_or(outcome)
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
        // The dead grant is cleared — unless a newer one replaced it.
        Err(RefreshFailure::Permanent(_)) => {
            load_tokens(path, server_url).map(|tokens| tokens.access_token)
        }
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
    let now = unix_now();
    Some(StoredTokens {
        access_token: access.to_string(),
        refresh_token: response
            .get("refresh_token")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| prior.refresh_token.clone()),
        expires_at: expires_in.map(|secs| now.saturating_add(secs)),
        client_id: prior.client_id.clone(),
        client_secret: prior.client_secret.clone(),
        token_endpoint: prior.token_endpoint.clone(),
        scope: response
            .get("scope")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| prior.scope.clone()),
        resource: prior.resource.clone(),
        issued_at: expires_in.map(|_| now),
        issuer: prior.issuer.clone(),
        redirect_uri: prior.redirect_uri.clone(),
        token_auth_method: prior.token_auth_method.clone(),
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

/// One parameter out of a `WWW-Authenticate` challenge, read as RFC 9110
/// auth-params — `name=token` or `name="quoted, string"` pairs, the name
/// matched case-insensitively and **whole**. It used to take the first
/// *substring* match, so `scope` was found inside `insufficient_scope` (and
/// inside an `error_description` mentioning scope), and the step-up
/// challenge the spec itself shows yielded no scope at all.
#[must_use]
pub fn challenge_param(challenge: &str, key: &str) -> Option<String> {
    let chars: Vec<char> = challenge.chars().collect();
    let n = chars.len();
    let tchar = |c: char| c.is_ascii_alphanumeric() || "!#$%&'*+-.^_`|~".contains(c);
    let mut i = 0;
    while i < n {
        while i < n && (chars[i].is_whitespace() || chars[i] == ',') {
            i += 1;
        }
        let start = i;
        while i < n && tchar(chars[i]) {
            i += 1;
        }
        if i == start {
            i += 1; // a character no parameter starts with
            continue;
        }
        let name: String = chars[start..i].iter().collect();
        let mut j = i;
        while j < n && chars[j] == ' ' {
            j += 1;
        }
        if j >= n || chars[j] != '=' {
            i = j; // an auth scheme (`Bearer`) or a token68, not a parameter
            continue;
        }
        j += 1;
        while j < n && chars[j] == ' ' {
            j += 1;
        }
        let mut value = String::new();
        if j < n && chars[j] == '"' {
            j += 1;
            while j < n && chars[j] != '"' {
                if chars[j] == '\\' && j + 1 < n {
                    j += 1;
                }
                value.push(chars[j]);
                j += 1;
            }
            j += 1; // the closing quote
        } else {
            while j < n && chars[j] != ',' && !chars[j].is_whitespace() {
                value.push(chars[j]);
                j += 1;
            }
        }
        if name.eq_ignore_ascii_case(key) {
            let value = value.trim();
            return (!value.is_empty()).then(|| value.to_string());
        }
        i = j;
    }
    None
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

/// Where an authorization server's metadata is looked for — the spec's
/// list, in its order. For an issuer with a path, RFC 8414 path insertion,
/// OIDC path insertion, then OIDC path **appending** (where Keycloak's realms
/// and Entra's tenants publish); without one, the two root documents. Never
/// the root for a path issuer: that is some other issuer's metadata.
#[must_use]
pub fn authorization_server_candidates(issuer: &str) -> Vec<String> {
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

/// RFC 8414 §3.3: does a metadata document's `issuer` name the issuer its
/// URL was built from? It MUST, or the document MUST NOT be used — one that
/// names another issuer could be anyone's. Identical strings, with the
/// reference SDK's one tolerance: a trailing `/` on the issuer asked for (or
/// on a bare origin either way). A document naming none is refused.
#[must_use]
pub fn issuer_matches(expected: &str, issuer: Option<&str>) -> bool {
    let Some(issuer) = issuer else {
        return false;
    };
    let bare_origin = || {
        url::Url::parse(expected)
            .is_ok_and(|u| u.path() == "/" && u.query().is_none() && u.fragment().is_none())
    };
    issuer == expected
        || expected.strip_suffix('/') == Some(issuer)
        || (issuer.strip_suffix('/') == Some(expected) && bare_origin())
}

/// RFC 9728 §3.3: is a protected-resource document's `resource` this
/// server's? Same origin, and the server's path under the resource's — the
/// reference SDK's `checkResourceAllowed`. A document for another resource
/// MUST NOT be used: it would point the flow at someone else's
/// authorization server and audience.
#[must_use]
pub fn resource_allowed(server_url: &str, resource: &str) -> bool {
    let (Ok(server), Ok(resource)) = (url::Url::parse(server_url), url::Url::parse(resource))
    else {
        return false;
    };
    if server.origin() != resource.origin() || server.path().len() < resource.path().len() {
        return false;
    }
    let with_slash = |path: &str| {
        if path.ends_with('/') {
            path.to_string()
        } else {
            format!("{path}/")
        }
    };
    with_slash(server.path()).starts_with(&with_slash(resource.path()))
}

/// The server URL as an RFC 8707 resource indicator: lowercase scheme and
/// host, no fragment ("MUST NOT include a fragment"), and no lone `/` for a
/// URL without a path (the spec's "SHOULD consistently use the form without
/// the trailing slash"). What is sent when the server publishes no
/// resource metadata to take the value from.
#[must_use]
pub fn canonical_resource(server_url: &str) -> String {
    let Ok(mut parsed) = url::Url::parse(server_url.trim()) else {
        return server_url.trim().to_string();
    };
    parsed.set_fragment(None);
    let text = parsed.to_string();
    if parsed.path() == "/" && parsed.query().is_none() {
        return text.trim_end_matches('/').to_string();
    }
    text
}

/// "All authorization server endpoints MUST be served over HTTPS" — with
/// loopback exempt, as the reference SDK does, for a server under
/// development on this machine.
#[must_use]
pub fn endpoint_is_secure(endpoint: &str) -> bool {
    let Ok(parsed) = url::Url::parse(endpoint) else {
        return false;
    };
    match parsed.scheme() {
        "https" => true,
        "http" => match parsed.host() {
            Some(url::Host::Domain(host)) => host == "localhost" || host.ends_with(".localhost"),
            Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
            Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
            None => false,
        },
        _ => false,
    }
}

/// Why an authorization server can't be used for a PKCE flow — `None` when
/// it can. One that lists its `code_challenge_methods_supported` without
/// `S256` is refused: the code it hands out would be bound to nothing. One
/// that lists none at all is let through, as the reference SDK does (the
/// spec's "MUST refuse" would turn away servers that support PKCE and merely
/// don't say so); `state` is required either way.
#[must_use]
pub fn pkce_problem(metadata: &Value) -> Option<String> {
    let methods = metadata
        .get("code_challenge_methods_supported")?
        .as_array()?;
    (!methods.iter().any(|m| m.as_str() == Some("S256"))).then(|| {
        "the authorization server doesn't support PKCE with S256, which this client requires"
            .to_string()
    })
}

/// The union of scope strings — each scope once, first-seen order — or
/// `None` when nothing is left. What a re-authorization asks for: the
/// scopes granted before and the ones the server now wants (the step-up
/// rule, "include these scopes alongside any previously granted scopes").
#[must_use]
pub fn scope_union(scopes: &[Option<&str>]) -> Option<String> {
    let mut out: Vec<&str> = Vec::new();
    for scope in scopes.iter().flatten().flat_map(|s| s.split_whitespace()) {
        if !out.contains(&scope) {
            out.push(scope);
        }
    }
    (!out.is_empty()).then(|| out.join(" "))
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
///
/// # Errors
/// The system's random source failing. There is no fallback: a guessable
/// `state` is a CSRF hole and a guessable verifier makes PKCE decorative.
pub fn pkce_and_state() -> Result<(String, String, String), String> {
    let mut verifier_bytes = [0u8; 48];
    let mut state_bytes = [0u8; 24];
    getrandom::fill(&mut verifier_bytes)
        .and_then(|()| getrandom::fill(&mut state_bytes))
        .map_err(|e| format!("no secure random source: {e}"))?;
    let verifier = b64url(&verifier_bytes);
    let challenge = {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(verifier.as_bytes());
        b64url(&hasher.finalize())
    };
    Ok((verifier, challenge, b64url(&state_bytes)))
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
    /// The RFC 8707 resource indicator the flow sends (authorize, token,
    /// refresh): the server's own resource metadata's `resource` when it
    /// publishes one — validated as this server's — else the canonical
    /// server URL.
    pub resource: String,
    /// `token_endpoint_auth_methods_supported` — what a registration may ask
    /// to authenticate with (empty when the metadata doesn't say).
    pub token_auth_methods: Vec<String>,
}

/// GET a metadata document as JSON, waiting at most `timeout`.
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
/// `basic` is a client's HTTP Basic `Authorization` value
/// ([`client_authentication`]).
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

fn post_form(url: &str, params: &[(String, String)], basic: Option<&str>) -> Result<Value, String> {
    let (status, body) = post_form_raw(url, params, basic)?;
    if !(200..300).contains(&status) {
        return Err(oauth_error_detail(status, &body));
    }
    serde_json::from_str(&body).map_err(|e| format!("bad JSON: {e}"))
}

/// An OAuth error answer in words: the `error` and `error_description` of
/// RFC 6749 §5.2 / RFC 7591 §3.2.2 when the body has them — the server's own
/// reason — else the status and the start of the body.
fn oauth_error_detail(status: u16, body: &str) -> String {
    let parsed = serde_json::from_str::<Value>(body.trim()).ok();
    let field = |key: &str| parsed.as_ref()?.get(key)?.as_str().map(str::to_string);
    match (field("error"), field("error_description")) {
        (Some(error), Some(description)) => format!("HTTP {status}: {error}: {description}"),
        (Some(error), None) => format!("HTTP {status}: {error}"),
        _ => {
            let flat: String = body.split_whitespace().collect::<Vec<_>>().join(" ");
            let flat: String = flat.chars().take(200).collect();
            format!("HTTP {status}: {flat}")
        }
    }
}

/// Resolve the authorization server for `server_url`, seeded by the 401's
/// challenge when one was captured — every document checked before it is
/// trusted (`docs/tools/mcp.md`):
///
/// 1. **Protected-resource metadata** (RFC 9728): the challenge's named URL
///    first, then the well-known candidates on the server itself; a
///    document whose `resource` isn't this server's is skipped (§3.3).
/// 2. **Authorization-server metadata** (RFC 8414 / OIDC Discovery) at the
///    spec's candidates for the issuer the resource names — or, without
///    resource metadata, the server's origin (2025-03-26 servers); a
///    document naming another `issuer` is skipped (§3.3), and the one used
///    must serve its endpoints over HTTPS and support PKCE with S256.
pub fn discover(server_url: &str, challenge: Option<&str>) -> Result<AuthServerMeta, String> {
    let resource_meta = protected_resource(server_url, challenge, HTTP_TIMEOUT);
    // The challenge's own `scope` outranks the resource's metadata: the spec
    // makes it authoritative for the operation that was refused, so a server
    // wanting more than it advertises gets what it actually asked for.
    let challenged_scopes = challenge.and_then(challenge_scope);
    let field = |meta: &Value, key: &str| meta.get(key).and_then(Value::as_str).map(str::to_string);
    let (auth_server, resource_scopes, resource) = match &resource_meta {
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
            field(meta, "resource").unwrap_or_else(|| canonical_resource(server_url)),
        ),
        None => (None, None, canonical_resource(server_url)),
    };
    // The named authorization server, else the MCP server's own origin.
    let auth_base = auth_server.unwrap_or_else(|| origin_of(server_url));
    if !endpoint_is_secure(&auth_base) {
        return Err(format!(
            "the authorization server {auth_base} isn't served over HTTPS"
        ));
    }
    let IssuerMetadata {
        meta,
        authorization_endpoint,
        token_endpoint,
    } = issuer_metadata(&auth_base, HTTP_TIMEOUT)?;
    let registration_endpoint = field(&meta, "registration_endpoint");
    for endpoint in [
        Some(&authorization_endpoint),
        Some(&token_endpoint),
        registration_endpoint.as_ref(),
    ]
    .into_iter()
    .flatten()
    {
        if !endpoint_is_secure(endpoint) {
            return Err(format!(
                "the authorization server's endpoint {endpoint} isn't served over HTTPS"
            ));
        }
    }
    if let Some(problem) = pkce_problem(&meta) {
        return Err(problem);
    }
    let as_scopes = string_list(&meta, "scopes_supported");
    // SEP-2207: the *resource's* scopes, plus `offline_access` when this
    // authorization server advertises it — the only way a refresh token is
    // ever issued, and the reason an authenticated server stops needing the
    // browser every hour. Deliberately never the AS's whole catalogue as a
    // fallback: asking for scopes the resource never wanted is what strict
    // providers answer with `invalid_scope`, failing the flow outright.
    let base = challenged_scopes.as_deref().or(resource_scopes.as_deref());
    let scopes = with_offline_access(base, &as_scopes);
    Ok(AuthServerMeta {
        authorization_endpoint,
        token_endpoint,
        registration_endpoint,
        scopes,
        issuer: field(&meta, "issuer"),
        iss_required: meta
            .get("authorization_response_iss_parameter_supported")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        resource,
        token_auth_methods: string_list(&meta, "token_endpoint_auth_methods_supported"),
    })
}

/// How long the connect-time probe ([`protected_without_registration`])
/// waits on each document: a server that publishes none answers 404 at
/// once, and one that hangs must not hold its connect for long.
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Is `server_url` an OAuth-protected resource whose authorization server
/// can't register this app? It publishes protected-resource metadata (RFC
/// 9728, a document whose `resource` is this server's) naming an
/// authorization server whose own metadata offers no `registration_endpoint`
/// — Google's MCP servers, which list their tools to anyone and refuse every
/// call. Hugging Face's, Context7's and Exa's publish the same metadata but
/// answer calls without a token, and their authorization servers register
/// apps, so they read `false`. The manager holds an anonymous connect to a
/// server this says `true` for as needing a sign-in (`docs/tools/mcp.md`). `false`
/// too whenever a document couldn't be read: only positive evidence holds a
/// server that connected.
#[must_use]
pub fn protected_without_registration(server_url: &str) -> bool {
    let Some(resource) = protected_resource(server_url, None, PROBE_TIMEOUT) else {
        return false;
    };
    let Some(auth_base) = resource
        .get("authorization_servers")
        .and_then(Value::as_array)
        .and_then(|list| list.first())
        .and_then(Value::as_str)
    else {
        return false;
    };
    issuer_metadata(auth_base, PROBE_TIMEOUT).is_ok_and(|found| {
        found
            .meta
            .get("registration_endpoint")
            .and_then(Value::as_str)
            .is_none()
    })
}

/// The server's protected-resource metadata (RFC 9728): the challenge's
/// named URL first, then the well-known candidates on the server itself —
/// the first document whose `resource` is this server's, any other skipped
/// (§3.3).
fn protected_resource(
    server_url: &str,
    challenge: Option<&str>,
    timeout: Duration,
) -> Option<Value> {
    let mut candidates: Vec<String> = Vec::new();
    if let Some(named) = challenge.and_then(challenge_resource_metadata) {
        candidates.push(named);
    }
    candidates.extend(well_known_candidates(
        server_url,
        "oauth-protected-resource",
    ));
    candidates.into_iter().find_map(|candidate| {
        let value = get_json_within(&candidate, timeout).ok()?;
        value
            .get("resource")
            .and_then(Value::as_str)
            .is_some_and(|resource| resource_allowed(server_url, resource))
            .then_some(value)
    })
}

/// An authorization server's metadata document and the two endpoints every
/// flow needs from it.
struct IssuerMetadata {
    meta: Value,
    authorization_endpoint: String,
    token_endpoint: String,
}

/// The first of the issuer's metadata documents
/// ([`authorization_server_candidates`]) that names `auth_base` as its
/// issuer (RFC 8414 §3.3) and carries both endpoints — one naming another
/// issuer, or missing an endpoint, skipped for the next.
fn issuer_metadata(auth_base: &str, timeout: Duration) -> Result<IssuerMetadata, String> {
    let field = |meta: &Value, key: &str| meta.get(key).and_then(Value::as_str).map(str::to_string);
    let mut last_err = String::from("no authorization-server metadata found");
    for candidate in authorization_server_candidates(auth_base) {
        let meta = match get_json_within(&candidate, timeout) {
            Ok(meta) => meta,
            Err(e) => {
                last_err = format!("{candidate}: {e}");
                continue;
            }
        };
        let issuer = field(&meta, "issuer");
        if !issuer_matches(auth_base, issuer.as_deref()) {
            last_err = format!(
                "{candidate} names issuer {}, not {auth_base}",
                issuer.as_deref().unwrap_or("none")
            );
            continue;
        }
        let (Some(authorization_endpoint), Some(token_endpoint)) = (
            field(&meta, "authorization_endpoint"),
            field(&meta, "token_endpoint"),
        ) else {
            last_err = format!("{candidate}: missing endpoints");
            continue;
        };
        return Ok(IssuerMetadata {
            meta,
            authorization_endpoint,
            token_endpoint,
        });
    }
    Err(format!(
        "could not discover the authorization server ({last_err})"
    ))
}

/// A metadata field's list of strings (empty when absent or malformed).
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

/// A registered OAuth client: what the flow authenticates to the token
/// endpoint with, and the redirect it registered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientIdentity {
    pub client_id: String,
    pub client_secret: Option<String>,
    /// The redirect URI the registration named (`None` for a grant stored
    /// before it was recorded).
    pub redirect_uri: Option<String>,
    /// The registered `token_endpoint_auth_method` (`None`: a secret, if
    /// any, rides the form body).
    pub auth_method: Option<String>,
}

/// Was the stored grant's client registered with `meta`'s authorization
/// server? SEP-2352 keys client credentials by the issuer that registered
/// them; a grant stored before the issuer was recorded is matched by its
/// token endpoint.
fn same_authorization_server(stored: &StoredTokens, meta: &AuthServerMeta) -> bool {
    match (&stored.issuer, &meta.issuer) {
        (Some(registered), Some(issuer)) => registered == issuer,
        (Some(_), None) => false,
        (None, _) => {
            !stored.token_endpoint.is_empty() && stored.token_endpoint == meta.token_endpoint
        }
    }
}

/// The stored client, when it may be presented to `meta`'s authorization
/// server — never to another one (SEP-2352).
fn reusable_client(stored: Option<&StoredTokens>, meta: &AuthServerMeta) -> Option<ClientIdentity> {
    let stored = stored.filter(|t| !t.client_id.is_empty())?;
    same_authorization_server(stored, meta).then(|| ClientIdentity {
        client_id: stored.client_id.clone(),
        client_secret: stored.client_secret.clone(),
        redirect_uri: stored.redirect_uri.clone(),
        auth_method: stored.token_auth_method.clone(),
    })
}

/// The `token_endpoint_auth_method` a registration asks for: a public client
/// (`none` — a CLI can't keep a secret, so a native app is public) unless the
/// authorization server lists methods without it; then the secret method it
/// offers, HTTP Basic (RFC 6749's default) first.
fn registration_auth_method(supported: &[String]) -> &'static str {
    let offers = |method: &str| supported.iter().any(|s| s == method);
    if supported.is_empty() || offers("none") {
        "none"
    } else if offers("client_secret_basic") {
        "client_secret_basic"
    } else if offers("client_secret_post") {
        "client_secret_post"
    } else {
        "none"
    }
}

/// How a client authenticates at the token endpoint: the form parameters it
/// adds and the `Authorization` value it sends, if any. A public client
/// names itself; `client_secret_basic` is HTTP Basic over the form-encoded
/// id and secret (RFC 6749 §2.3.1), neither in the body; any other secret
/// rides the body (`client_secret_post`).
fn client_authentication(
    client_id: &str,
    secret: Option<&str>,
    method: Option<&str>,
) -> (Vec<(String, String)>, Option<String>) {
    let public = vec![("client_id".to_string(), client_id.to_string())];
    match (secret, method) {
        (None, _) | (Some(_), Some("none")) => (public, None),
        (Some(secret), Some("client_secret_basic")) => {
            let pair = format!("{}:{}", url_encode(client_id), url_encode(secret));
            let encoded = crate::clipboard::base64_encode(pair.as_bytes());
            (Vec::new(), Some(format!("Basic {encoded}")))
        }
        (Some(secret), _) => {
            let mut params = public;
            params.push(("client_secret".to_string(), secret.to_string()));
            (params, None)
        }
    }
}

/// [`client_authentication`] for a stored grant's client.
fn client_authentication_of(tokens: &StoredTokens) -> (Vec<(String, String)>, Option<String>) {
    client_authentication(
        &tokens.client_id,
        tokens.client_secret.as_deref(),
        tokens.token_auth_method.as_deref(),
    )
}

/// Dynamically register a client (RFC 7591) for `redirect_uri`, asking for
/// an authentication method the authorization server offers. A refusal
/// carries the server's own reason (RFC 7591 §3.2.2).
pub fn register_client(
    registration_endpoint: &str,
    redirect_uri: &str,
    supported_methods: &[String],
) -> Result<ClientIdentity, String> {
    let client = crate::llm::http_client(HTTP_TIMEOUT).map_err(|e| e.to_string())?;
    let method = registration_auth_method(supported_methods);
    let body = serde_json::json!({
        "client_name": "alter-zero",
        "redirect_uris": [redirect_uri],
        "grant_types": ["authorization_code", "refresh_token"],
        "response_types": ["code"],
        "token_endpoint_auth_method": method,
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
    let status = response.status().as_u16();
    let text = response.text().unwrap_or_default();
    if !(200..300).contains(&status) {
        return Err(format!(
            "registration failed: {}",
            oauth_error_detail(status, &text)
        ));
    }
    let value: Value =
        serde_json::from_str(&text).map_err(|e| format!("registration: bad JSON: {e}"))?;
    let field = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_string);
    let client_id = field("client_id")
        .filter(|id| !id.is_empty())
        .ok_or_else(|| "registration returned no client_id".to_string())?;
    let client_secret = field("client_secret");
    // The method the server registered, else the one asked for — unless it
    // asked for none and handed back a secret anyway, which then rides the
    // body.
    let auth_method = field("token_endpoint_auth_method")
        .or_else(|| (client_secret.is_some() && method != "none").then(|| method.to_string()));
    Ok(ClientIdentity {
        client_id,
        client_secret,
        redirect_uri: Some(redirect_uri.to_string()),
        auth_method,
    })
}

/// A redirect URI's port.
fn redirect_port(redirect_uri: &str) -> Option<u16> {
    url::Url::parse(redirect_uri).ok()?.port()
}

/// What the browser is told at the end of an MCP sign-in.
const MCP_PAGES: Pages = Pages {
    received: "Authorization received",
    restart: "/mcp",
};

// ---------------------------------------------------------------------------
// the interactive flow

/// What the flow reports as it goes — the manager forwards these onto the
/// MCP event channel for the auth page.
pub trait AuthProgress: Send {
    /// The authorize URL is ready (shown + copied from the auth page).
    fn on_url(&self, url: &str);
    /// A pasted URL was turned away — not a redirect, or not this
    /// sign-in's — and the flow keeps waiting; `reason` is for the page.
    fn on_paste_rejected(&self, _reason: &str) {}
}

/// What a sign-in is for: the server, the challenge its refusal carried,
/// where its grant is kept, and the pre-registered client its entry names.
#[derive(Debug, Clone, Copy)]
pub struct SignIn<'a> {
    pub server_url: &'a str,
    pub challenge: Option<&'a str>,
    pub store_path: Option<&'a Path>,
    /// The entry's `oauth` block, its secret already resolved (the file's,
    /// else the one kept with the tokens).
    pub client: Option<&'a crate::mcp::OAuthClientConfig>,
}

/// How a pre-registered client authenticates at the token endpoint: as a
/// public client without a secret; with one, in the form body unless the
/// authorization server takes only HTTP Basic (the reference SDK's choice
/// when the metadata lists both, or nothing).
fn configured_auth_method(supported: &[String], has_secret: bool) -> Option<String> {
    if !has_secret {
        return None;
    }
    let offers = |method: &str| supported.iter().any(|s| s == method);
    let method = if offers("client_secret_basic") && !offers("client_secret_post") {
        "client_secret_basic"
    } else {
        "client_secret_post"
    };
    Some(method.to_string())
}

/// What an authorization server needs on the authorize URL beyond the
/// standard parameters. Google issues a refresh token only for
/// `access_type=offline`, and issues it again only when consent is shown —
/// without both, the login lapses within the hour.
#[must_use]
pub fn provider_authorize_params(issuer: Option<&str>) -> Vec<(&'static str, &'static str)> {
    match issuer.map(|issuer| issuer.trim_end_matches('/')) {
        Some("https://accounts.google.com") => {
            vec![("access_type", "offline"), ("prompt", "consent")]
        }
        _ => Vec::new(),
    }
}

/// Run the whole interactive flow for `target`, blocking this (worker)
/// thread: discovery → a client (the entry's pre-registered one, a reused
/// one, or a registration) → browser/paste → exchange → persist.
/// `paste_rx` delivers the user's pasted redirect URL; `open_browser` is the
/// boundary's launcher (injected so tests need no display). `cancel` is
/// checked between the steps, so an Esc during discovery never opens the
/// browser.
pub fn run_auth_flow(
    target: &SignIn<'_>,
    paste_rx: &mpsc::Receiver<String>,
    progress: &dyn AuthProgress,
    open_browser: &dyn Fn(&str),
    cancel: &CancelToken,
) -> Result<(), String> {
    let SignIn {
        server_url,
        challenge,
        store_path,
        client: configured,
    } = *target;
    let still_wanted = || {
        if cancel.is_cancelled() {
            Err("cancelled".to_string())
        } else {
            Ok(())
        }
    };
    // A grant with nowhere to go is a dead end — said before the browser
    // opens, not after the user has signed in.
    if store_path.is_none() {
        return Err("there is no config directory to keep the grant in".to_string());
    }
    let meta = discover(server_url, challenge)?;
    still_wanted()?;
    let stored = load_tokens(store_path, server_url);
    let (callback, redirect_uri, client) = match configured
        .and_then(|config| Some((config, config.client_id.clone()?)))
    {
        // The client the user registered with the provider: on exactly the
        // port its redirect URI names — the provider was told that one —
        // else any (a native client takes any loopback port, RFC 8252).
        Some((config, client_id)) => {
            let ports = config
                .callback_port
                .map_or_else(|| vec![0; 9], |port| vec![port]);
            let callback = CallbackServer::bind(&ports, "/callback").map_err(|e| {
                match config.callback_port {
                    Some(port) => {
                        format!("the callback port {port} (oauth.callbackPort) isn't free: {e}")
                    }
                    None => e,
                }
            })?;
            let redirect_uri = format!("http://localhost:{}/callback", callback.port);
            let client = ClientIdentity {
                client_id,
                client_secret: config.client_secret.clone(),
                redirect_uri: Some(redirect_uri.clone()),
                auth_method: configured_auth_method(
                    &meta.token_auth_methods,
                    config.client_secret.is_some(),
                ),
            };
            (callback, redirect_uri, client)
        }
        None => {
            let reused = reusable_client(stored.as_ref(), &meta);
            // The callback binds before any registration so the redirect is
            // exact — on a reused client's own port when it is free, since
            // an authorization server may match the registered redirect
            // exactly (RFC 8252's loopback port leniency is a SHOULD, and
            // plenty don't).
            let preferred = reused
                .as_ref()
                .and_then(|client| client.redirect_uri.as_deref())
                .and_then(redirect_port);
            // An entry's own callbackPort holds here too — exactly, as for a
            // configured client: the user may have had to allow-list it.
            let fixed = configured.and_then(|config| config.callback_port);
            let ports: Vec<u16> = match fixed {
                Some(port) => vec![port],
                None => preferred
                    .into_iter()
                    .chain(std::iter::repeat_n(0, 8))
                    .collect(),
            };
            let callback = CallbackServer::bind(&ports, "/callback").map_err(|e| match fixed {
                Some(port) => {
                    format!("the callback port {port} (oauth.callbackPort) isn't free: {e}")
                }
                None => e,
            })?;
            let redirect_uri = format!("http://localhost:{}/callback", callback.port);
            let client = match (reused, &meta.registration_endpoint) {
                (Some(client), _)
                    if client.redirect_uri.as_deref() == Some(redirect_uri.as_str()) =>
                {
                    client
                }
                (_, Some(endpoint)) => {
                    register_client(endpoint, &redirect_uri, &meta.token_auth_methods)?
                }
                // Nothing to register with: the old client on a new port is
                // the one chance (an authorization server honouring RFC 8252
                // §7.3 accepts it).
                (Some(client), None) => client,
                (None, None) => {
                    return Err(format!(
                        "{} registers no clients itself, so signing in needs the OAuth client \
                         you created with the provider: add it as oauth.clientId (and \
                         oauth.callbackPort, if its redirect URI names one) in the server's \
                         entry, or with `alter-zero mcp add --client-id … --client-secret`",
                        meta.issuer.as_deref().unwrap_or("The authorization server")
                    ));
                }
            };
            (callback, redirect_uri, client)
        }
    };
    still_wanted()?;
    let (verifier, code_challenge, state) = pkce_and_state()?;
    // Re-authorizing asks for what was granted before *and* what the server
    // now wants — the step-up rule ("include these scopes alongside any
    // previously granted scopes"), so a scope upgrade never drops the rest;
    // another authorization server's scopes mean nothing here.
    let granted = stored
        .as_ref()
        .filter(|tokens| same_authorization_server(tokens, &meta))
        .and_then(|tokens| tokens.scope.as_deref());
    // The entry's own scopes, when it names them, are what the user
    // registered the client for — asked for as they are.
    let scopes = match configured.and_then(|config| config.scopes.clone()) {
        Some(scopes) => Some(scopes),
        None => scope_union(&[meta.scopes.as_deref(), granted]),
    };
    let mut authorize_url = build_authorize_url(
        &meta.authorization_endpoint,
        &client.client_id,
        &redirect_uri,
        &code_challenge,
        &state,
        scopes.as_deref(),
        &meta.resource,
    );
    for (key, value) in provider_authorize_params(meta.issuer.as_deref()) {
        if redirect_param(&authorize_url, key).is_none() {
            authorize_url.push_str(&format!("&{key}={}", url_encode(value)));
        }
    }
    progress.on_url(&authorize_url);
    open_browser(&authorize_url);
    let rejected = |reason: &str| progress.on_paste_rejected(reason);
    let code = loopback::wait_for_code(
        &callback,
        &Expected {
            state: &state,
            issuer: meta.issuer.as_deref(),
            iss_required: meta.iss_required,
        },
        &MCP_PAGES,
        AUTH_FLOW_TIMEOUT,
        Some((paste_rx, &rejected)),
        cancel,
    )
    .map_err(|end| end.to_string())?;
    drop(callback);
    let (client_params, basic) = client_authentication(
        &client.client_id,
        client.client_secret.as_deref(),
        client.auth_method.as_deref(),
    );
    let mut params = vec![
        ("grant_type".to_string(), "authorization_code".to_string()),
        ("code".to_string(), code),
        ("redirect_uri".to_string(), redirect_uri),
        ("code_verifier".to_string(), verifier),
        ("resource".to_string(), meta.resource.clone()),
    ];
    params.extend(client_params);
    let response = post_form(&meta.token_endpoint, &params, basic.as_deref())
        .map_err(|e| format!("token exchange failed: {e}"))?;
    let prior = StoredTokens {
        client_id: client.client_id,
        client_secret: client.client_secret,
        token_endpoint: meta.token_endpoint.clone(),
        scope: scopes,
        resource: Some(meta.resource.clone()),
        issuer: meta.issuer.clone(),
        redirect_uri: client.redirect_uri,
        token_auth_method: client.auth_method,
        ..Default::default()
    };
    let tokens = tokens_from_response(&response, &prior)
        .ok_or_else(|| "token response carried no access_token".to_string())?;
    save_tokens(store_path, server_url, Some(&tokens))
        .map_err(|e| format!("signed in, but the grant couldn't be saved: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

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
        let (verifier, challenge, state) = pkce_and_state().expect("entropy");
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

    /// A challenge is parsed by parameter, never by substring: `scope`
    /// used to be found inside `insufficient_scope` (and inside an
    /// `error_description` mentioning scope), so the step-up challenge the
    /// spec itself shows yielded no scope at all.
    #[test]
    fn challenge_parameters_are_read_by_name() {
        let step_up = r#"Bearer error="insufficient_scope", scope="files:write files:read", resource_metadata="https://x.test/.well-known/oauth-protected-resource""#;
        assert_eq!(
            challenge_scope(step_up).as_deref(),
            Some("files:write files:read")
        );
        assert_eq!(
            challenge_param(step_up, "error").as_deref(),
            Some("insufficient_scope")
        );
        let described = r#"Bearer error_description="the scope was wrong, try again", scope="a b""#;
        assert_eq!(challenge_scope(described).as_deref(), Some("a b"));
        // Names are case-insensitive; a quoted value keeps commas, and an
        // escaped quote is a quote.
        let odd = r#"Bearer Realm="x, y", SCOPE=read, note="say \"hi\"""#;
        assert_eq!(challenge_param(odd, "realm").as_deref(), Some("x, y"));
        assert_eq!(challenge_scope(odd).as_deref(), Some("read"));
        assert_eq!(challenge_param(odd, "note").as_deref(), Some(r#"say "hi""#));
        assert_eq!(challenge_scope("Bearer realm=\"x\""), None);
    }

    /// The spec's discovery list, exactly: an issuer with a path tries RFC
    /// 8414 path insertion, OIDC path insertion, then OIDC path *appending*
    /// (Keycloak's realms, Entra's tenants) — and never the root, which is
    /// another issuer's metadata.
    #[test]
    fn authorization_server_metadata_is_looked_for_where_the_spec_says() {
        assert_eq!(
            authorization_server_candidates("https://auth.test/realms/acme"),
            [
                "https://auth.test/.well-known/oauth-authorization-server/realms/acme",
                "https://auth.test/.well-known/openid-configuration/realms/acme",
                "https://auth.test/realms/acme/.well-known/openid-configuration",
            ]
        );
        assert_eq!(
            authorization_server_candidates("https://auth.test"),
            [
                "https://auth.test/.well-known/oauth-authorization-server",
                "https://auth.test/.well-known/openid-configuration",
            ]
        );
        assert_eq!(
            authorization_server_candidates("https://auth.test/"),
            authorization_server_candidates("https://auth.test")
        );
    }

    /// RFC 8414 §3.3: the metadata's `issuer` MUST be the issuer its URL was
    /// built from, or the document MUST NOT be used — the one trailing-slash
    /// difference on a bare origin aside (the reference SDK's tolerance).
    #[test]
    fn metadata_names_the_issuer_it_was_fetched_for() {
        assert!(issuer_matches(
            "https://auth.test/realms/acme",
            Some("https://auth.test/realms/acme")
        ));
        assert!(issuer_matches(
            "https://auth.test",
            Some("https://auth.test/")
        ));
        assert!(issuer_matches(
            "https://auth.test/",
            Some("https://auth.test")
        ));
        assert!(!issuer_matches(
            "https://auth.test/realms/acme",
            Some("https://auth.test/realms/other")
        ));
        assert!(!issuer_matches(
            "https://auth.test/realms/acme",
            Some("https://auth.test")
        ));
        assert!(!issuer_matches("https://auth.test", None));
    }

    /// RFC 9728 §3.3: the protected-resource metadata must be *for* this
    /// server — same origin, a path the server's sits under — or a server
    /// could hand out another's. The resource indicator sent is then that
    /// metadata's own `resource`; without metadata, the server URL in its
    /// canonical form (lowercase scheme and host, no fragment, no lone `/`).
    #[test]
    fn the_resource_metadata_must_be_this_servers() {
        let server = "https://mcp.test/mcp";
        assert!(resource_allowed(server, "https://mcp.test/mcp"));
        assert!(resource_allowed(server, "https://mcp.test/"));
        assert!(resource_allowed(server, "https://mcp.test"));
        assert!(!resource_allowed(server, "https://other.test/mcp"));
        assert!(!resource_allowed(server, "http://mcp.test/mcp"));
        assert!(!resource_allowed(server, "https://mcp.test/mcp-two"));
        assert!(!resource_allowed(
            "https://mcp.test/api",
            "https://mcp.test/api/v2"
        ));
        assert_eq!(
            canonical_resource("HTTPS://MCP.Test/mcp#frag"),
            "https://mcp.test/mcp"
        );
        assert_eq!(canonical_resource("https://mcp.test/"), "https://mcp.test");
        assert_eq!(
            canonical_resource("https://mcp.test/a?b=1"),
            "https://mcp.test/a?b=1"
        );
    }

    /// "All authorization server endpoints MUST be served over HTTPS" —
    /// loopback aside, for a server under development.
    #[test]
    fn authorization_endpoints_must_be_https_or_loopback() {
        assert!(endpoint_is_secure("https://auth.test/token"));
        assert!(endpoint_is_secure("http://127.0.0.1:8080/token"));
        assert!(endpoint_is_secure("http://localhost/token"));
        assert!(endpoint_is_secure("http://[::1]:9/token"));
        assert!(!endpoint_is_secure("http://auth.test/token"));
        assert!(!endpoint_is_secure("ftp://auth.test/token"));
        assert!(!endpoint_is_secure("not a url"));
    }

    /// Re-authorizing asks for what was granted before *and* what the server
    /// now wants (the step-up rule), each scope once, in a stable order.
    #[test]
    fn a_step_up_asks_for_the_union_of_old_and_new_scopes() {
        assert_eq!(
            scope_union(&[Some("a b"), Some("b c"), None]).as_deref(),
            Some("a b c")
        );
        assert_eq!(scope_union(&[None, Some("  ")]), None);
    }

    /// A loopback HTTP server answering GETs from a path → JSON map (404
    /// for anything else), its base URL returned. `{base}` in a body is
    /// replaced by that URL.
    fn discovery_fixture(routes: Vec<(&'static str, &'static str)>) -> String {
        use std::io::{BufRead, BufReader};
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let routes: Vec<(String, String)> = routes
            .into_iter()
            .map(|(path, body)| (path.to_string(), body.replace("{base}", &base)))
            .collect();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                let _ = reader.read_line(&mut line);
                loop {
                    let mut header = String::new();
                    if reader.read_line(&mut header).is_err() || header.trim().is_empty() {
                        break;
                    }
                }
                let path = line.split_whitespace().nth(1).unwrap_or_default();
                let reply = match routes.iter().find(|(route, _)| route == path) {
                    Some((_, body)) => format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    ),
                    None => {
                        "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                            .to_string()
                    }
                };
                let _ = stream.write_all(reply.as_bytes());
            }
        });
        base
    }

    /// A Keycloak-shaped issuer publishes only the OIDC path-appended
    /// document; a document at the root names another issuer and is never
    /// used. The resource metadata is the server's own, and its `resource`
    /// is what the flow will send.
    #[test]
    fn discovery_finds_a_path_issuer_and_trusts_only_its_own_documents() {
        let base = discovery_fixture(vec![
            (
                "/.well-known/oauth-protected-resource/mcp",
                r#"{"resource":"{base}/","authorization_servers":["{base}/realms/acme"],"scopes_supported":["mcp:tools"]}"#,
            ),
            (
                "/.well-known/oauth-authorization-server",
                r#"{"issuer":"{base}/","authorization_endpoint":"{base}/root/auth","token_endpoint":"{base}/root/token"}"#,
            ),
            (
                "/realms/acme/.well-known/openid-configuration",
                r#"{"issuer":"{base}/realms/acme","authorization_endpoint":"{base}/realms/acme/auth","token_endpoint":"{base}/realms/acme/token","code_challenge_methods_supported":["S256"]}"#,
            ),
        ]);
        let meta = discover(&format!("{base}/mcp"), None).expect("discovered");
        assert_eq!(
            meta.authorization_endpoint,
            format!("{base}/realms/acme/auth")
        );
        assert_eq!(
            meta.issuer.as_deref(),
            Some(format!("{base}/realms/acme").as_str())
        );
        assert_eq!(meta.resource, format!("{base}/"));
        assert_eq!(meta.scopes.as_deref(), Some("mcp:tools"));
    }

    /// Resource metadata for another server is skipped; with none of its
    /// own, the flow falls back to the server's origin as the issuer and
    /// sends the canonical server URL as the resource.
    #[test]
    fn foreign_resource_metadata_is_never_used() {
        let base = discovery_fixture(vec![
            (
                "/.well-known/oauth-protected-resource/mcp",
                r#"{"resource":"https://elsewhere.test/mcp","authorization_servers":["https://evil.test"]}"#,
            ),
            (
                "/.well-known/oauth-authorization-server",
                r#"{"issuer":"{base}","authorization_endpoint":"{base}/auth","token_endpoint":"{base}/token"}"#,
            ),
        ]);
        let meta = discover(&format!("{base}/mcp"), None).expect("discovered");
        assert_eq!(meta.authorization_endpoint, format!("{base}/auth"));
        assert_eq!(meta.resource, format!("{base}/mcp"));
    }

    /// An authorization server that lists its PKCE methods without S256 is
    /// refused, and so is one whose endpoints aren't HTTPS (or loopback).
    #[test]
    fn discovery_refuses_no_s256_and_plain_http_endpoints() {
        let base = discovery_fixture(vec![(
            "/.well-known/oauth-authorization-server",
            r#"{"issuer":"{base}","authorization_endpoint":"{base}/auth","token_endpoint":"{base}/token","code_challenge_methods_supported":["plain"]}"#,
        )]);
        let refused = discover(&format!("{base}/mcp"), None).expect_err("no S256");
        assert!(refused.contains("S256"), "{refused}");
        let base = discovery_fixture(vec![(
            "/.well-known/oauth-authorization-server",
            r#"{"issuer":"{base}","authorization_endpoint":"http://auth.test/auth","token_endpoint":"{base}/token"}"#,
        )]);
        let refused = discover(&format!("{base}/mcp"), None).expect_err("plain http");
        assert!(refused.contains("HTTPS"), "{refused}");
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
            resource: None,
            ..Default::default()
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
        // A grant minted for the resource the server's metadata named is
        // refreshed for that same resource — never a different audience.
        let named = StoredTokens {
            resource: Some("https://mcp.test".to_string()),
            ..confidential
        };
        let params = refresh_params(&named, "https://mcp.test/mcp");
        assert!(
            params
                .iter()
                .any(|(k, v)| k == "resource" && v == "https://mcp.test")
        );
        // And the store keeps it.
        let store = record_tokens("", "https://mcp.test/mcp", Some(&named));
        assert_eq!(
            parse_token_store(&store)["https://mcp.test/mcp"]
                .resource
                .as_deref(),
            Some("https://mcp.test")
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
            "unauthorized_client",
            "unsupported_grant_type",
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
        // An error that says nothing about the grant itself — a malformed
        // request, an AS having a bad moment — must not cost the user a
        // grant that may be perfectly good: kept, and retried later.
        for error in [
            "invalid_request",
            "temporarily_unavailable",
            "server_error",
            "something_new",
        ] {
            let body = format!(r#"{{"error":"{error}"}}"#);
            match refresh_failure_kind(Some(400), &body) {
                F::Transient(detail) => assert!(detail.contains(error), "{detail}"),
                other => panic!("{error} classified {other:?}"),
            }
        }
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
                resource: None,
                ..Default::default()
            }),
        )
        .expect("the store write");
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
                        resource: None,
                        ..Default::default()
                    };
                    save_tokens(
                        Some(&path),
                        &format!("https://s{i}.test/mcp"),
                        Some(&tokens),
                    )
                    .expect("the store write");
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
            resource: None,
            ..Default::default()
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

    /// A token living less than twice the skew would be stale the moment it
    /// was minted — every request a renewal, every renewal a rotation. Its
    /// window is half its life instead; a long-lived token keeps the full
    /// five minutes.
    #[test]
    fn a_short_lived_token_is_renewed_at_half_life() {
        let short = StoredTokens {
            issued_at: Some(1000),
            expires_at: Some(1120),
            ..Default::default()
        };
        assert!(tokens_fresh(&short, 1059));
        assert!(!tokens_fresh(&short, 1060));
        let long = StoredTokens {
            issued_at: Some(1000),
            expires_at: Some(1000 + 3600),
            ..Default::default()
        };
        assert!(tokens_fresh(&long, 1000 + 3600 - EXPIRY_SKEW_SECS - 1));
        assert!(!tokens_fresh(&long, 1000 + 3600 - EXPIRY_SKEW_SECS));
        // A mint records when it was issued, and the store keeps it.
        let minted = tokens_from_response(
            &serde_json::json!({"access_token": "a", "expires_in": 120}),
            &StoredTokens::default(),
        )
        .unwrap();
        assert_eq!(minted.expires_at.unwrap() - minted.issued_at.unwrap(), 120);
        let store = record_tokens("", "https://s.test/mcp", Some(&minted));
        assert_eq!(
            parse_token_store(&store)["https://s.test/mcp"].issued_at,
            minted.issued_at
        );
    }

    /// The store holds refresh tokens, which the spec requires be kept
    /// confidential: the file is the owner's from the moment it exists, a
    /// directory the save creates is too, and no temp file is left behind.
    #[cfg(unix)]
    #[test]
    fn the_store_is_private_from_creation() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let path = home.join("mcp-auth.json");
        let tokens = StoredTokens {
            access_token: "at".to_string(),
            ..Default::default()
        };
        save_tokens(Some(&path), "https://s.test/mcp", Some(&tokens)).unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&path), 0o600);
        assert_eq!(mode(&home), 0o700);
        let leftovers: Vec<String> = std::fs::read_dir(&home)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }

    /// A grant that can't be kept is an error the sign-in reports — never a
    /// silent success that leaves the next connect without a token — and a
    /// store that can't be read is never rewritten over (that would drop
    /// every other server's grant).
    #[test]
    fn a_grant_that_cannot_be_kept_is_an_error() {
        let tokens = StoredTokens {
            access_token: "at".to_string(),
            ..Default::default()
        };
        assert!(save_tokens(None, "https://s.test/mcp", Some(&tokens)).is_err());
        assert!(save_tokens(None, "https://s.test/mcp", None).is_ok());
        let dir = tempfile::tempdir().unwrap();
        let blocker = dir.path().join("file");
        std::fs::write(&blocker, "x").unwrap();
        let inside = blocker.join("mcp-auth.json");
        assert!(save_tokens(Some(&inside), "https://s.test/mcp", Some(&tokens)).is_err());
        // A store that is a directory reads as an error, not as empty.
        let unreadable = dir.path().join("store-dir");
        std::fs::create_dir(&unreadable).unwrap();
        assert!(save_tokens(Some(&unreadable), "https://s.test/mcp", Some(&tokens)).is_err());
    }

    /// A pre-registered client's secret is kept with the tokens —
    /// owner-only, never in `mcp.json` — under its server, and apart from
    /// the login: "Clear authentication" deletes the login, not the client.
    #[test]
    fn a_client_secret_is_kept_apart_from_the_login() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcp-auth.json");
        let server = "https://calendarmcp.googleapis.com/mcp/v1";
        save_client_secret(Some(&path), server, Some("s3cret")).unwrap();
        let login = StoredTokens {
            access_token: "at".to_string(),
            ..Default::default()
        };
        save_tokens(Some(&path), server, Some(&login)).unwrap();
        assert_eq!(
            load_client_secret(Some(&path), server).as_deref(),
            Some("s3cret")
        );
        save_tokens(Some(&path), server, None).unwrap();
        assert!(load_tokens(Some(&path), server).is_none());
        assert_eq!(
            load_client_secret(Some(&path), server).as_deref(),
            Some("s3cret")
        );
        assert_eq!(
            load_client_secret(Some(&path), "https://other.test/mcp"),
            None
        );
        save_client_secret(Some(&path), server, None).unwrap();
        assert_eq!(load_client_secret(Some(&path), server), None);
        assert!(save_client_secret(None, server, Some("x")).is_err());
    }

    /// A one-route token endpoint answering each request through `answer`
    /// (given the form body) with `(status, JSON body)`; its URL returned.
    fn spawn_token_endpoint(answer: impl Fn(&str) -> (u16, String) + Send + 'static) -> String {
        use std::io::{BufRead, BufReader};
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                if reader.read_line(&mut line).is_err() {
                    continue;
                }
                let mut length = 0usize;
                loop {
                    let mut header = String::new();
                    if reader.read_line(&mut header).is_err() || header.trim().is_empty() {
                        break;
                    }
                    if let Some(rest) = header.to_ascii_lowercase().strip_prefix("content-length:")
                    {
                        length = rest.trim().parse().unwrap_or(0);
                    }
                }
                let mut body = vec![0u8; length];
                let _ = reader.read_exact(&mut body);
                let (status, reply) = answer(&String::from_utf8_lossy(&body));
                let _ = stream.write_all(
                    format!(
                        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                        reply.len()
                    )
                    .as_bytes(),
                );
            }
        });
        format!("http://127.0.0.1:{port}/token")
    }

    fn grant(access: &str, refresh: &str, token_endpoint: &str) -> StoredTokens {
        StoredTokens {
            access_token: access.to_string(),
            refresh_token: Some(refresh.to_string()),
            expires_at: Some(1),
            client_id: "cid".to_string(),
            token_endpoint: token_endpoint.to_string(),
            ..Default::default()
        }
    }

    /// Another session renewing the same grant holds its lock across the
    /// whole token request; this one waits, then takes the grant that
    /// session minted instead of presenting the spent refresh token again —
    /// which a reuse-detecting server answers by revoking the family.
    #[cfg(unix)]
    #[test]
    fn a_refresh_waits_for_another_session_and_takes_its_grant() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcp-auth.json");
        let server = "https://s.test/mcp";
        let requests = std::sync::Arc::new(AtomicUsize::new(0));
        let token_url = {
            let requests = std::sync::Arc::clone(&requests);
            spawn_token_endpoint(move |_| {
                requests.fetch_add(1, Ordering::SeqCst);
                (
                    200,
                    r#"{"access_token":"at-2","refresh_token":"rt-2"}"#.to_string(),
                )
            })
        };
        save_tokens(
            Some(&path),
            server,
            Some(&grant("at-1", "rt-1", &token_url)),
        )
        .unwrap();
        // The other session takes the server's renewal lock…
        let other = FileLock::acquire(
            &refresh_lock_path(&path, server),
            Instant::now() + Duration::from_secs(1),
        )
        .expect("the other session's lock");
        let ours = {
            let path = path.clone();
            std::thread::spawn(move || refresh_grant(Some(&path), server, Some("at-1")))
        };
        std::thread::sleep(Duration::from_millis(250));
        assert_eq!(
            requests.load(Ordering::SeqCst),
            0,
            "the refresh token went out while another session was renewing it"
        );
        // …rotates the grant, and lets go.
        let rotated = StoredTokens {
            expires_at: Some(u64::MAX / 4),
            ..grant("at-9", "rt-9", &token_url)
        };
        save_tokens(Some(&path), server, Some(&rotated)).unwrap();
        drop(other);
        let got = ours.join().unwrap();
        assert_eq!(got, Ok(rotated));
        assert_eq!(requests.load(Ordering::SeqCst), 0);
    }

    /// A permanent rejection clears only the grant it rejected: a sign-in
    /// that landed while the renewal was out keeps its newer grant, and the
    /// caller gets that one.
    #[test]
    fn a_permanent_failure_never_clears_a_newer_grant() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcp-auth.json");
        let server = "https://s.test/mcp";
        let newer = StoredTokens {
            expires_at: Some(u64::MAX / 4),
            ..grant("at-new", "rt-new", "https://as.test/token")
        };
        let token_url = {
            let (path, newer) = (path.clone(), newer.clone());
            spawn_token_endpoint(move |_| {
                save_tokens(Some(&path), server, Some(&newer)).unwrap();
                (400, r#"{"error":"invalid_grant"}"#.to_string())
            })
        };
        save_tokens(
            Some(&path),
            server,
            Some(&grant("at-1", "rt-1", &token_url)),
        )
        .unwrap();
        let result = refresh_grant(Some(&path), server, Some("at-1"));
        assert_eq!(result, Ok(newer.clone()));
        assert_eq!(load_tokens(Some(&path), server), Some(newer));
    }

    /// And a renewal never brings back a grant "Clear authentication"
    /// deleted while the request was out — the clear wins.
    #[test]
    fn a_refresh_never_resurrects_a_cleared_grant() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcp-auth.json");
        let server = "https://s.test/mcp";
        let token_url = {
            let path = path.clone();
            spawn_token_endpoint(move |_| {
                save_tokens(Some(&path), server, None).unwrap();
                (
                    200,
                    r#"{"access_token":"at-2","refresh_token":"rt-2"}"#.to_string(),
                )
            })
        };
        save_tokens(
            Some(&path),
            server,
            Some(&grant("at-1", "rt-1", &token_url)),
        )
        .unwrap();
        let result = refresh_grant(Some(&path), server, Some("at-1"));
        assert_eq!(result, Err(RefreshFailure::NoGrant));
        assert_eq!(load_tokens(Some(&path), server), None);
    }

    fn meta_for(issuer: &str, iss_required: bool) -> AuthServerMeta {
        AuthServerMeta {
            authorization_endpoint: format!("{issuer}/authorize"),
            token_endpoint: format!("{issuer}/token"),
            registration_endpoint: None,
            scopes: None,
            issuer: Some(issuer.to_string()),
            iss_required,
            resource: "https://mcp.test/mcp".to_string(),
            token_auth_methods: Vec::new(),
        }
    }

    /// SEP-2352: client credentials belong to the authorization server that
    /// issued them and are never presented to another. A grant stored before
    /// the issuer was recorded is matched by its token endpoint.
    #[test]
    fn client_credentials_are_reused_only_with_their_issuer() {
        let meta = meta_for("https://as.test", false);
        let stored = StoredTokens {
            client_id: "cid".to_string(),
            issuer: Some("https://as.test".to_string()),
            redirect_uri: Some("http://localhost:4242/callback".to_string()),
            ..Default::default()
        };
        let client = reusable_client(Some(&stored), &meta).expect("same issuer");
        assert_eq!(client.client_id, "cid");
        assert_eq!(
            client.redirect_uri.as_deref(),
            Some("http://localhost:4242/callback")
        );
        let elsewhere = StoredTokens {
            issuer: Some("https://other.test".to_string()),
            ..stored.clone()
        };
        assert_eq!(reusable_client(Some(&elsewhere), &meta), None);
        let legacy = StoredTokens {
            issuer: None,
            token_endpoint: "https://as.test/token".to_string(),
            ..stored.clone()
        };
        assert!(reusable_client(Some(&legacy), &meta).is_some());
        let legacy_elsewhere = StoredTokens {
            token_endpoint: "https://other.test/token".to_string(),
            ..legacy
        };
        assert_eq!(reusable_client(Some(&legacy_elsewhere), &meta), None);
        let anonymous = StoredTokens {
            client_id: String::new(),
            ..stored
        };
        assert_eq!(reusable_client(Some(&anonymous), &meta), None);
        assert_eq!(reusable_client(None, &meta), None);
    }

    /// A client authenticates at the token endpoint the way it registered:
    /// a public client names itself, a `client_secret_basic` one sends HTTP
    /// Basic (RFC 6749 §2.3.1, each half form-encoded) and nothing in the
    /// body, a `client_secret_post` one puts both in the body.
    #[test]
    fn a_client_authenticates_the_way_it_registered() {
        let (params, header) = client_authentication("cid", None, None);
        assert_eq!(params, [("client_id".to_string(), "cid".to_string())]);
        assert_eq!(header, None);
        let (params, header) =
            client_authentication("c id", Some("s:cret"), Some("client_secret_basic"));
        assert!(params.is_empty(), "{params:?}");
        let expected = format!(
            "Basic {}",
            crate::clipboard::base64_encode(b"c%20id:s%3Acret")
        );
        assert_eq!(header.as_deref(), Some(expected.as_str()));
        for method in [Some("client_secret_post"), None] {
            let (params, header) = client_authentication("cid", Some("sec"), method);
            assert_eq!(
                params,
                [
                    ("client_id".to_string(), "cid".to_string()),
                    ("client_secret".to_string(), "sec".to_string())
                ]
            );
            assert_eq!(header, None);
        }
        // A registration that came back public never sends a secret.
        let (params, _) = client_authentication("cid", Some("sec"), Some("none"));
        assert_eq!(params, [("client_id".to_string(), "cid".to_string())]);
    }

    /// Registration asks to be a public client unless the authorization
    /// server says it can't serve one, then for the secret method it offers.
    #[test]
    fn registration_asks_for_a_method_the_server_offers() {
        let methods = |list: &[&str]| list.iter().map(|m| m.to_string()).collect::<Vec<_>>();
        assert_eq!(registration_auth_method(&methods(&[])), "none");
        assert_eq!(
            registration_auth_method(&methods(&["client_secret_basic", "none"])),
            "none"
        );
        assert_eq!(
            registration_auth_method(&methods(&["client_secret_post", "client_secret_basic"])),
            "client_secret_basic"
        );
        assert_eq!(
            registration_auth_method(&methods(&["client_secret_post"])),
            "client_secret_post"
        );
        assert_eq!(
            registration_auth_method(&methods(&["private_key_jwt"])),
            "none"
        );
    }

    /// The flow's progress, captured: the authorize URL and every rejected
    /// paste.
    #[derive(Default)]
    struct Recorded {
        urls: std::sync::Mutex<Vec<String>>,
        rejected: std::sync::Mutex<Vec<String>>,
    }

    impl AuthProgress for std::sync::Arc<Recorded> {
        fn on_url(&self, url: &str) {
            self.urls.lock().unwrap().push(url.to_string());
        }
        fn on_paste_rejected(&self, reason: &str) {
            self.rejected.lock().unwrap().push(reason.to_string());
        }
    }

    /// Send one raw request to the callback port and read the whole reply.
    fn hit(port: u16, request: &str) -> String {
        use std::io::Read as _;
        let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect");
        stream.write_all(request.as_bytes()).unwrap();
        let mut reply = String::new();
        let _ = stream.read_to_string(&mut reply);
        reply
    }

    /// One fake authorization server: resource and AS metadata for
    /// `{base}/mcp`, dynamic registration (counted, its reply overridable),
    /// and a token endpoint recording each request's headers and body.
    struct AuthServerFixture {
        base: String,
        registrations: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        token_requests: std::sync::Arc<std::sync::Mutex<Vec<(String, String)>>>,
    }

    fn spawn_auth_server(register_reply: Option<(u16, &'static str)>) -> AuthServerFixture {
        spawn_auth_server_with(true, register_reply)
    }

    /// [`spawn_auth_server`], with or without a registration endpoint — an
    /// authorization server like Google's registers no clients itself.
    fn spawn_auth_server_with(
        registration: bool,
        register_reply: Option<(u16, &'static str)>,
    ) -> AuthServerFixture {
        use std::io::{BufRead, BufReader};
        use std::sync::atomic::{AtomicUsize, Ordering};
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let registrations = std::sync::Arc::new(AtomicUsize::new(0));
        let token_requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let fixture = AuthServerFixture {
            base: base.clone(),
            registrations: std::sync::Arc::clone(&registrations),
            token_requests: std::sync::Arc::clone(&token_requests),
        };
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                if reader.read_line(&mut line).is_err() {
                    continue;
                }
                let mut head = String::new();
                let mut length = 0usize;
                loop {
                    let mut header = String::new();
                    if reader.read_line(&mut header).is_err() || header.trim().is_empty() {
                        break;
                    }
                    if let Some(rest) = header.to_ascii_lowercase().strip_prefix("content-length:")
                    {
                        length = rest.trim().parse().unwrap_or(0);
                    }
                    head.push_str(&header);
                }
                let mut body = vec![0u8; length];
                let _ = reader.read_exact(&mut body);
                let body = String::from_utf8_lossy(&body).to_string();
                let path = line
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or_default()
                    .to_string();
                let (status, reply) = match path.as_str() {
                    "/.well-known/oauth-protected-resource/mcp" => (
                        200,
                        format!(
                            r#"{{"resource":"{base}/mcp","authorization_servers":["{base}"]}}"#
                        ),
                    ),
                    "/.well-known/oauth-authorization-server" => (
                        200,
                        format!(
                            r#"{{"issuer":"{base}","authorization_endpoint":"{base}/authorize","token_endpoint":"{base}/token",{registration}"code_challenge_methods_supported":["S256"]}}"#,
                            registration = if registration {
                                format!(r#""registration_endpoint":"{base}/register","#)
                            } else {
                                String::new()
                            }
                        ),
                    ),
                    "/register" => {
                        let n = registrations.fetch_add(1, Ordering::SeqCst) + 1;
                        match register_reply {
                            Some((status, reply)) => (status, reply.to_string()),
                            None => (201, format!(r#"{{"client_id":"cid-{n}"}}"#)),
                        }
                    }
                    "/token" => {
                        token_requests
                            .lock()
                            .unwrap()
                            .push((head.clone(), body.clone()));
                        (
                            200,
                            r#"{"access_token":"at","refresh_token":"rt","expires_in":3600}"#
                                .to_string(),
                        )
                    }
                    _ => (404, String::new()),
                };
                let _ = stream.write_all(
                    format!(
                        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                        reply.len()
                    )
                    .as_bytes(),
                );
            }
        });
        fixture
    }

    /// A browser that approves at once: it follows the authorize URL's
    /// redirect back to the callback with `code`, the URL's own state, and
    /// no `iss` (this fixture's metadata doesn't advertise it).
    fn approving_browser(code: &'static str) -> impl Fn(&str) {
        move |authorize_url: &str| {
            let redirect = redirect_param(authorize_url, "redirect_uri").expect("redirect_uri");
            let state = redirect_param(authorize_url, "state").expect("state");
            let port = url::Url::parse(&redirect).unwrap().port().expect("a port");
            std::thread::spawn(move || {
                let _ = hit(
                    port,
                    &format!(
                        "GET /callback?code={code}&state={} HTTP/1.1\r\nHost: localhost\r\n\r\n",
                        url_encode(&state)
                    ),
                );
            });
        }
    }

    fn sign_in(
        fixture: &AuthServerFixture,
        store: &Path,
        browser: &dyn Fn(&str),
        cancel: &CancelToken,
    ) -> (Result<(), String>, std::sync::Arc<Recorded>) {
        sign_in_with(fixture, store, browser, cancel, None)
    }

    fn sign_in_with(
        fixture: &AuthServerFixture,
        store: &Path,
        browser: &dyn Fn(&str),
        cancel: &CancelToken,
        client: Option<&crate::mcp::OAuthClientConfig>,
    ) -> (Result<(), String>, std::sync::Arc<Recorded>) {
        let (_paste_tx, paste_rx) = mpsc::channel();
        let progress = std::sync::Arc::new(Recorded::default());
        let server_url = format!("{}/mcp", fixture.base);
        let result = run_auth_flow(
            &SignIn {
                server_url: &server_url,
                challenge: None,
                store_path: Some(store),
                client,
            },
            &paste_rx,
            &progress,
            browser,
            cancel,
        );
        (result, progress)
    }

    fn free_port() -> u16 {
        TcpListener::bind(("127.0.0.1", 0))
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    /// A server whose authorization server registers no clients (Google's)
    /// signs in with the pre-registered client its entry names: its id, its
    /// secret, the redirect on exactly its port, the scopes it asks for —
    /// and no registration is ever attempted.
    #[test]
    fn a_preregistered_client_signs_in_without_registration() {
        use std::sync::atomic::Ordering;
        let fixture = spawn_auth_server_with(false, None);
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("mcp-auth.json");
        let port = free_port();
        let client = crate::mcp::OAuthClientConfig {
            client_id: Some("pre-cid".to_string()),
            client_secret: Some("pre-secret".to_string()),
            callback_port: Some(port),
            scopes: Some("calendar openid".to_string()),
        };
        let (result, progress) = sign_in_with(
            &fixture,
            &store,
            &approving_browser("c"),
            &CancelToken::new(),
            Some(&client),
        );
        result.expect("signed in");
        assert_eq!(fixture.registrations.load(Ordering::SeqCst), 0);
        let url = progress.urls.lock().unwrap()[0].clone();
        assert_eq!(port_of(&url), port);
        assert_eq!(
            redirect_param(&url, "client_id").as_deref(),
            Some("pre-cid")
        );
        assert_eq!(
            redirect_param(&url, "scope").as_deref(),
            Some("calendar openid")
        );
        let requests = fixture.token_requests.lock().unwrap();
        let (_, body) = &requests[0];
        assert!(
            body.contains("client_id=pre-cid") && body.contains("client_secret=pre-secret"),
            "{body}"
        );
        let stored = load_tokens(Some(&store), &format!("{}/mcp", fixture.base)).unwrap();
        assert_eq!(stored.client_id, "pre-cid");
        assert_eq!(stored.client_secret.as_deref(), Some("pre-secret"));
    }

    /// An `oauth` block naming only a `callbackPort` keeps the registration
    /// on that port — a redirect the user may have had to allow-list.
    #[test]
    fn a_configured_callback_port_holds_for_a_registered_client_too() {
        use std::sync::atomic::Ordering;
        let fixture = spawn_auth_server(None);
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("mcp-auth.json");
        let port = free_port();
        let client = crate::mcp::OAuthClientConfig {
            callback_port: Some(port),
            ..Default::default()
        };
        let (result, progress) = sign_in_with(
            &fixture,
            &store,
            &approving_browser("c"),
            &CancelToken::new(),
            Some(&client),
        );
        result.expect("signed in");
        assert_eq!(fixture.registrations.load(Ordering::SeqCst), 1);
        assert_eq!(port_of(&progress.urls.lock().unwrap()[0]), port);
    }

    /// Without one, such a server's sign-in says how to add one, instead of
    /// a dead end.
    #[test]
    fn no_registration_and_no_client_says_how_to_add_one() {
        let fixture = spawn_auth_server_with(false, None);
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("mcp-auth.json");
        let (result, _) = sign_in(
            &fixture,
            &store,
            &approving_browser("c"),
            &CancelToken::new(),
        );
        let error = result.unwrap_err();
        assert!(
            error.contains("oauth.clientId") && error.contains("--client-id"),
            "{error}"
        );
    }

    /// A configured callback port is the one the provider was told: when
    /// it's taken the sign-in says so, rather than listening elsewhere.
    #[test]
    fn a_taken_callback_port_is_an_error() {
        let fixture = spawn_auth_server_with(false, None);
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("mcp-auth.json");
        let squatter = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = squatter.local_addr().unwrap().port();
        let client = crate::mcp::OAuthClientConfig {
            client_id: Some("pre-cid".to_string()),
            callback_port: Some(port),
            ..Default::default()
        };
        let (result, progress) = sign_in_with(
            &fixture,
            &store,
            &approving_browser("c"),
            &CancelToken::new(),
            Some(&client),
        );
        let error = result.unwrap_err();
        assert!(error.contains(&port.to_string()), "{error}");
        assert!(progress.urls.lock().unwrap().is_empty());
    }

    /// Google issues a refresh token only for an offline grant consented to
    /// on the screen — asked for of that issuer alone.
    #[test]
    fn google_is_asked_for_an_offline_grant() {
        assert_eq!(
            provider_authorize_params(Some("https://accounts.google.com")),
            [("access_type", "offline"), ("prompt", "consent")]
        );
        assert!(provider_authorize_params(Some("https://as.test")).is_empty());
        assert!(provider_authorize_params(None).is_empty());
    }

    fn port_of(authorize_url: &str) -> u16 {
        let redirect = redirect_param(authorize_url, "redirect_uri").unwrap();
        url::Url::parse(&redirect).unwrap().port().unwrap()
    }

    /// The whole flow against a fake authorization server: discovery,
    /// registration, the browser's return, the exchange — and a second
    /// sign-in reusing the client on the port it registered, where a taken
    /// port registers again rather than sending the server a redirect it
    /// never saw.
    #[test]
    fn a_sign_in_runs_end_to_end_and_reuses_its_client() {
        use std::sync::atomic::Ordering;
        let fixture = spawn_auth_server(None);
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("mcp-auth.json");
        let server = format!("{}/mcp", fixture.base);
        let (result, progress) = sign_in(
            &fixture,
            &store,
            &approving_browser("code-1"),
            &CancelToken::new(),
        );
        result.expect("signed in");
        assert_eq!(fixture.registrations.load(Ordering::SeqCst), 1);
        let first_url = progress.urls.lock().unwrap()[0].clone();
        let port = port_of(&first_url);
        {
            let requests = fixture.token_requests.lock().unwrap();
            let (_, body) = &requests[0];
            for expected in [
                "grant_type=authorization_code".to_string(),
                "code=code-1".to_string(),
                "code_verifier=".to_string(),
                "client_id=cid-1".to_string(),
                format!(
                    "redirect_uri={}",
                    url_encode(&format!("http://localhost:{port}/callback"))
                ),
                format!("resource={}", url_encode(&server)),
            ] {
                assert!(body.contains(&expected), "{expected} missing from {body}");
            }
        }
        let stored = load_tokens(Some(&store), &server).expect("stored");
        assert_eq!(stored.access_token, "at");
        assert_eq!(stored.client_id, "cid-1");
        assert_eq!(stored.issuer.as_deref(), Some(fixture.base.as_str()));
        assert_eq!(
            stored.redirect_uri.as_deref(),
            Some(format!("http://localhost:{port}/callback").as_str())
        );
        // Again: the same client, on the port it registered.
        let (result, progress) = sign_in(
            &fixture,
            &store,
            &approving_browser("code-2"),
            &CancelToken::new(),
        );
        result.expect("signed in again");
        assert_eq!(fixture.registrations.load(Ordering::SeqCst), 1);
        assert_eq!(port_of(&progress.urls.lock().unwrap()[0]), port);
        // With that port taken, a new registration names the new one.
        let _squatter = TcpListener::bind(("127.0.0.1", port)).expect("take the port");
        let (result, progress) = sign_in(
            &fixture,
            &store,
            &approving_browser("code-3"),
            &CancelToken::new(),
        );
        result.expect("signed in on another port");
        assert_eq!(fixture.registrations.load(Ordering::SeqCst), 2);
        assert_ne!(port_of(&progress.urls.lock().unwrap()[0]), port);
        assert_eq!(
            load_tokens(Some(&store), &server).unwrap().client_id,
            "cid-2"
        );
    }

    /// A client registered with another authorization server is never
    /// presented to this one (SEP-2352): the flow registers afresh.
    #[test]
    fn a_sign_in_never_presents_another_servers_client() {
        let fixture = spawn_auth_server(None);
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("mcp-auth.json");
        let server = format!("{}/mcp", fixture.base);
        save_tokens(
            Some(&store),
            &server,
            Some(&StoredTokens {
                access_token: "old".to_string(),
                client_id: "foreign-cid".to_string(),
                client_secret: Some("foreign-secret".to_string()),
                issuer: Some("https://other.test".to_string()),
                token_endpoint: "https://other.test/token".to_string(),
                ..Default::default()
            }),
        )
        .unwrap();
        let (result, _) = sign_in(
            &fixture,
            &store,
            &approving_browser("c"),
            &CancelToken::new(),
        );
        result.expect("signed in");
        let requests = fixture.token_requests.lock().unwrap();
        let (head, body) = &requests[0];
        assert!(body.contains("client_id=cid-1"), "{body}");
        assert!(
            !body.contains("foreign") && !head.contains("foreign"),
            "{head}{body}"
        );
    }

    /// A registration that comes back confidential with `client_secret_basic`
    /// authenticates that way at the token endpoint.
    #[test]
    fn a_basic_auth_client_signs_in_with_basic_auth() {
        let fixture = spawn_auth_server(Some((
            201,
            r#"{"client_id":"cid","client_secret":"sec","token_endpoint_auth_method":"client_secret_basic"}"#,
        )));
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("mcp-auth.json");
        let (result, _) = sign_in(
            &fixture,
            &store,
            &approving_browser("c"),
            &CancelToken::new(),
        );
        result.expect("signed in");
        let requests = fixture.token_requests.lock().unwrap();
        let (head, body) = &requests[0];
        let expected = format!(
            "authorization: basic {}",
            crate::clipboard::base64_encode(b"cid:sec")
        );
        assert!(
            head.to_ascii_lowercase()
                .contains(&expected.to_ascii_lowercase()),
            "{head}"
        );
        assert!(!body.contains("client_secret"), "{body}");
    }

    /// The registration endpoint's own reason reaches the user (RFC 7591
    /// §3.2.2), not a bare status.
    #[test]
    fn a_refused_registration_says_why() {
        let fixture = spawn_auth_server(Some((
            400,
            r#"{"error":"invalid_redirect_uri","error_description":"loopback redirects are not allowed"}"#,
        )));
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("mcp-auth.json");
        let (result, _) = sign_in(
            &fixture,
            &store,
            &approving_browser("c"),
            &CancelToken::new(),
        );
        let error = result.expect_err("refused");
        assert!(
            error.contains("invalid_redirect_uri")
                && error.contains("loopback redirects are not allowed"),
            "{error}"
        );
    }

    /// Esc during discovery means the browser never opens; and with nowhere
    /// to keep the grant the flow says so before it does.
    #[test]
    fn a_sign_in_that_cannot_finish_never_opens_the_browser() {
        let fixture = spawn_auth_server(None);
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("mcp-auth.json");
        let opened = std::sync::atomic::AtomicBool::new(false);
        let browser = |_: &str| opened.store(true, std::sync::atomic::Ordering::SeqCst);
        let cancel = CancelToken::new();
        cancel.cancel();
        let (result, _) = sign_in(&fixture, &store, &browser, &cancel);
        assert!(result.is_err());
        assert!(
            !opened.load(std::sync::atomic::Ordering::SeqCst),
            "opened after Esc"
        );
        let (_paste_tx, paste_rx) = mpsc::channel();
        let server_url = format!("{}/mcp", fixture.base);
        let result = run_auth_flow(
            &SignIn {
                server_url: &server_url,
                challenge: None,
                store_path: None,
                client: None,
            },
            &paste_rx,
            &std::sync::Arc::new(Recorded::default()),
            &browser,
            &CancelToken::new(),
        );
        assert!(result.is_err());
        assert!(
            !opened.load(std::sync::atomic::Ordering::SeqCst),
            "opened with no store"
        );
    }
}
