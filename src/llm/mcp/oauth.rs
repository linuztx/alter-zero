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

/// Refresh a token this long before it actually expires.
const EXPIRY_SKEW_SECS: u64 = 60;

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
    let existing = std::fs::read_to_string(path).unwrap_or_default();
    let updated = record_tokens(&existing, server_url, tokens);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, updated);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
}

/// Is this grant still fresh (with the refresh skew)? `now` is Unix seconds.
#[must_use]
pub fn tokens_fresh(tokens: &StoredTokens, now: u64) -> bool {
    match tokens.expires_at {
        Some(expires) => now + EXPIRY_SKEW_SECS < expires,
        None => true,
    }
}

/// A usable bearer for `server_url`: the stored access token, refreshed
/// through the token endpoint (and re-persisted) when stale. `None` when
/// nothing is stored or the refresh fails — the connect will 401 and the
/// server goes back to needs-auth, which is the truthful state.
#[must_use]
pub fn fresh_bearer(path: Option<&Path>, server_url: &str) -> Option<String> {
    let tokens = load_tokens(path, server_url)?;
    let now = unix_now();
    if tokens_fresh(&tokens, now) {
        return Some(tokens.access_token);
    }
    let refresh = tokens.refresh_token.clone()?;
    let mut params = vec![
        ("grant_type".to_string(), "refresh_token".to_string()),
        ("refresh_token".to_string(), refresh),
        ("client_id".to_string(), tokens.client_id.clone()),
    ];
    if let Some(secret) = &tokens.client_secret {
        params.push(("client_secret".to_string(), secret.clone()));
    }
    params.push(("resource".to_string(), server_url.to_string()));
    let response = post_form(&tokens.token_endpoint, &params).ok()?;
    let refreshed = tokens_from_response(&response, &tokens)?;
    save_tokens(path, server_url, Some(&refreshed));
    Some(refreshed.access_token)
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Fold a token response onto the prior grant (rotating refresh tokens,
/// keeping the client identity).
fn tokens_from_response(response: &Value, prior: &StoredTokens) -> Option<StoredTokens> {
    let access = response.get("access_token").and_then(Value::as_str)?;
    Some(StoredTokens {
        access_token: access.to_string(),
        refresh_token: response
            .get("refresh_token")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| prior.refresh_token.clone()),
        expires_at: response
            .get("expires_in")
            .and_then(Value::as_u64)
            .map(|secs| unix_now() + secs),
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

/// The `resource_metadata` URL a 401's `WWW-Authenticate` challenge names
/// (RFC 9728's discovery seed), if any.
#[must_use]
pub fn challenge_resource_metadata(challenge: &str) -> Option<String> {
    let lower = challenge.to_ascii_lowercase();
    let at = lower.find("resource_metadata")?;
    let rest = &challenge[at + "resource_metadata".len()..];
    let rest = rest.trim_start().strip_prefix('=')?.trim_start();
    let rest = rest.strip_prefix('"').unwrap_or(rest);
    let end = rest.find(['"', ',', ' ']).unwrap_or(rest.len());
    let url = rest[..end].trim();
    (!url.is_empty()).then(|| url.to_string())
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
    }
    let query = form_encode(&params);
    let sep = if authorization_endpoint.contains('?') {
        '&'
    } else {
        '?'
    };
    format!("{authorization_endpoint}{sep}{query}")
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

fn post_form(url: &str, params: &[(String, String)]) -> Result<Value, String> {
    let client = crate::llm::http_client(HTTP_TIMEOUT).map_err(|e| e.to_string())?;
    let response = client
        .post(url)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .header("Accept", "application/json")
        .body(form_encode(params))
        .send()
        .map_err(|e| format!("request failed: {e}"))?;
    let status = response.status();
    let body = response.text().unwrap_or_default();
    if !status.is_success() {
        let flat: String = body.split_whitespace().collect::<Vec<_>>().join(" ");
        let flat: String = flat.chars().take(200).collect();
        return Err(format!("HTTP {}: {flat}", status.as_u16()));
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
                let scopes = resource_scopes.clone().or_else(|| {
                    meta.get("scopes_supported")
                        .and_then(Value::as_array)
                        .map(|scopes| {
                            scopes
                                .iter()
                                .filter_map(Value::as_str)
                                .collect::<Vec<_>>()
                                .join(" ")
                        })
                        .filter(|s| !s.is_empty())
                });
                return Ok(AuthServerMeta {
                    authorization_endpoint,
                    token_endpoint,
                    registration_endpoint: field("registration_endpoint"),
                    scopes,
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
    let code = wait_for_code(&listener, &state, paste_rx, cancel)?;
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
/// carries one.
fn wait_for_code(
    listener: &TcpListener,
    state: &str,
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
