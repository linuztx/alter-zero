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

/// A client kept for one server in the store's `clients` section — apart
/// from its login, so "Clear authentication" leaves the client the user
/// set up (`docs/mcp.md` *Pre-registered clients*). Two things write one:
/// the `/mcp` auth page's client form, which keeps the whole client it was
/// given, and `mcp add --client-secret`, which keeps only the secret of the
/// client the server's own entry names (`client_id` is `None` there).
#[derive(Clone, Default, PartialEq, Eq)]
pub struct StoredClient {
    /// The client's id, when it was entered on the auth page. `None`: the
    /// secret belongs to the entry's own `oauth.clientId`.
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
    /// The port of the redirect URI an entered client was registered with —
    /// its sign-in listens on exactly it.
    pub callback_port: Option<u16>,
    /// The authorization server the client was created with — its issuer,
    /// else the origin of its token endpoint. The spec binds static
    /// credentials to it (SEP-2352): they are never presented to another,
    /// and an entered client bound to none is presented to none. `None` for
    /// a secret kept from the command line, where no sign-in has seen the
    /// server.
    pub issuer: Option<String>,
    /// An entered client that signs in with the secret its server's entry
    /// carries in the file — the entry's own id entered on another port,
    /// its secret field left as it was. The file's secret is read at each
    /// sign-in, never copied here; only a record with an id says this.
    pub secret_from_entry: bool,
}

impl std::fmt::Debug for StoredClient {
    /// Never the secret: whether one is kept is all a log may know.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoredClient")
            .field("client_id", &self.client_id)
            .field(
                "client_secret",
                &self.client_secret.as_ref().map(|_| "<redacted>"),
            )
            .field("callback_port", &self.callback_port)
            .field("issuer", &self.issuer)
            .field("secret_from_entry", &self.secret_from_entry)
            .finish()
    }
}

impl StoredClient {
    /// Was this client entered in `/mcp` — it names its own id — rather than
    /// being the secret kept for the client the server's entry names?
    #[must_use]
    pub const fn is_entered(&self) -> bool {
        self.client_id.is_some()
    }

    /// The secret this entered client signs in with: its own, or — on its
    /// word ([`secret_from_entry`](Self::secret_from_entry)) — the one its
    /// entry spells, while the entry still names this client.
    #[must_use]
    pub fn secret_in_effect(
        &self,
        configured: Option<&crate::mcp::OAuthClientConfig>,
    ) -> Option<String> {
        self.client_secret.clone().or_else(|| {
            configured
                .filter(|config| {
                    self.secret_from_entry
                        && config.client_id.is_some()
                        && config.client_id == self.client_id
                })
                .and_then(|config| config.client_secret.clone())
        })
    }

    /// The secret kept for the client `client_id` names: this record's own
    /// client, or — a record with no id — the one the server's entry names.
    /// Never another client's.
    #[must_use]
    pub fn secret_for(&self, client_id: &str) -> Option<&str> {
        match &self.client_id {
            Some(own) if own != client_id => None,
            _ => self.client_secret.as_deref(),
        }
    }

    /// May this client be presented to the authorization server `authority`
    /// names ([`AuthServerMeta::authority`])? Only the one it was created
    /// with — compared as written, the spec's simple string comparison — or
    /// any, when none was recorded.
    #[must_use]
    pub fn belongs_to(&self, authority: &str) -> bool {
        self.issuer
            .as_deref()
            .is_none_or(|issuer| issuer == authority)
    }
}

/// Parse the store's `clients` section into `server_url → client`. An entry
/// holding neither an id nor a secret says nothing and is skipped, and so
/// is an entered client bound to no authorization server: there is nothing
/// to hold it to, so it is presented to none.
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
            let text = |key: &str| {
                entry
                    .get(key)
                    .and_then(Value::as_str)
                    .filter(|text| !text.is_empty())
                    .map(str::to_string)
            };
            let client_id = text("client_id");
            let client = StoredClient {
                // Only a client can say whose secret it uses.
                secret_from_entry: client_id.is_some()
                    && entry.get("secret_from_entry").and_then(Value::as_bool) == Some(true),
                client_id,
                client_secret: text("client_secret"),
                callback_port: entry
                    .get("callback_port")
                    .and_then(Value::as_u64)
                    .and_then(|port| u16::try_from(port).ok())
                    .filter(|port| *port != 0),
                issuer: text("issuer"),
            };
            let unbound = client.is_entered() && client.issuer.is_none();
            (!unbound && (client.client_id.is_some() || client.client_secret.is_some()))
                .then(|| (url.clone(), client))
        })
        .collect()
}

/// Re-render the store with one server's kept client replaced (`None`
/// deletes it) — [`record_tokens`]' sibling over the `clients` section,
/// which is dropped once empty. `None` when the store's `clients` is not an
/// object: nothing is written over a section that can't be read.
#[must_use]
pub fn record_client(
    contents: &str,
    server_url: &str,
    client: Option<&StoredClient>,
) -> Option<String> {
    let mut value: Value = serde_json::from_str(contents.trim())
        .ok()
        .filter(Value::is_object)
        .unwrap_or_else(|| Value::Object(Default::default()));
    let root = value.as_object_mut().expect("object ensured above");
    let clients = root
        .entry("clients")
        .or_insert_with(|| Value::Object(Default::default()));
    let clients = clients.as_object_mut()?;
    match client {
        Some(client) => {
            let mut entry = serde_json::Map::new();
            for (key, field) in [
                ("client_id", &client.client_id),
                ("client_secret", &client.client_secret),
            ] {
                if let Some(field) = field {
                    entry.insert(key.into(), field.clone().into());
                }
            }
            if let Some(port) = client.callback_port {
                entry.insert("callback_port".into(), port.into());
            }
            if let Some(issuer) = &client.issuer {
                entry.insert("issuer".into(), issuer.clone().into());
            }
            if client.secret_from_entry && client.is_entered() {
                entry.insert("secret_from_entry".into(), true.into());
            }
            clients.insert(server_url.to_string(), Value::Object(entry));
        }
        None => {
            clients.shift_remove(server_url);
        }
    }
    if clients.is_empty() {
        root.shift_remove("clients");
    }
    serde_json::to_string_pretty(&value).ok()
}

/// The client kept for `server_url`, if any.
#[must_use]
pub fn load_client(path: Option<&Path>, server_url: &str) -> Option<StoredClient> {
    let contents = std::fs::read_to_string(path?).ok()?;
    parse_client_store(&contents).remove(server_url)
}

/// Keep — or, with `None`, forget — the client for `server_url`: in the
/// store's `clients` section, owner-only and never in `mcp.json`, under the
/// store's own locked read-modify-write.
///
/// # Errors
/// Nowhere to keep a client, or a store that can't be read or written.
pub fn save_client(
    path: Option<&Path>,
    server_url: &str,
    client: Option<&StoredClient>,
) -> Result<(), String> {
    let Some(path) = path else {
        return match client {
            Some(_) => Err("there is no config directory to keep the client in".to_string()),
            None => Ok(()),
        };
    };
    // A `clients` section that is no object is left alone — and said, since
    // a save that wrote nothing would send the sign-in round in a circle.
    let mut unreadable = false;
    rewrite_store(path, |contents| {
        let updated = record_client(contents, server_url, client);
        unreadable = updated.is_none();
        updated
    })?;
    if unreadable {
        return Err(format!(
            "the clients section of {} isn't an object",
            path.display()
        ));
    }
    Ok(())
}

/// Keep — or, with `None`, forget — a pre-registered client's secret for
/// `server_url` (`mcp add --client-secret`, `docs/mcp.md`): the secret of
/// the client the server's entry names, in the store's `clients` section,
/// owner-only and never in `mcp.json`, and apart from the login, so "Clear
/// authentication" leaves the client the user configured. It replaces
/// whatever was kept for the server — the entry's own client is the one in
/// use from here on.
///
/// # Errors
/// Nowhere to keep a secret, or a store that can't be read or written.
pub fn save_client_secret(
    path: Option<&Path>,
    server_url: &str,
    secret: Option<&str>,
) -> Result<(), String> {
    let client = secret.map(|secret| StoredClient {
        client_secret: Some(secret.to_string()),
        ..Default::default()
    });
    save_client(path, server_url, client.as_ref())
}

/// The client secret kept for `server_url`, if any.
#[must_use]
pub fn load_client_secret(path: Option<&Path>, server_url: &str) -> Option<String> {
    load_client(path, server_url)?.client_secret
}

/// What the `/mcp` client form submits ([`client_to_keep`]).
#[derive(Clone, PartialEq, Eq)]
pub struct EnteredClient {
    /// The authorization server the client was entered for — the request's
    /// own `issuer`.
    pub issuer: String,
    pub client_id: String,
    /// The secret typed, if any.
    pub client_secret: Option<String>,
    /// The secret field was left empty over a secret the form said is kept
    /// (`unchanged`): the secret in effect for this client stays. Left empty
    /// otherwise, the client has none — a public client.
    pub keep_secret: bool,
    /// The port of the redirect URI the client was registered with — `None`
    /// only for the entry's own client signing in on any free port.
    pub callback_port: Option<u16>,
}

impl std::fmt::Debug for EnteredClient {
    /// Never the secret: whether one was typed is all a log may know.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EnteredClient")
            .field("issuer", &self.issuer)
            .field("client_id", &self.client_id)
            .field(
                "client_secret",
                &self.client_secret.as_ref().map(|_| "<redacted>"),
            )
            .field("keep_secret", &self.keep_secret)
            .field("callback_port", &self.callback_port)
            .finish()
    }
}

/// The record the token store keeps for a server once its client form is
/// submitted, from what it keeps now (`kept`) and the entry's own client
/// (`configured`) — `None` keeps nothing (`docs/mcp.md` *The client form*).
///
/// - **The entry's own client** — its id on the entry's exact port (none
///   for one that takes any free port): a typed secret is kept as that
///   client's secret, ahead of its file's. With none typed nothing new is
///   kept: a secret already kept for it stays, and an entered client in its
///   place goes, so the entry's client, its `callbackUrl` and its port are
///   the ones in effect.
/// - **An entered client** — anything else: the whole client as the form
///   gave it, bound to its issuer. With no secret typed it is a public
///   client, unless the field was left on a kept secret (`keep_secret`):
///   then the secret in effect for this id stays — the same entered
///   client's own, or its word to use its entry's; else, for the entry's id
///   on another port, the secret kept for the entry's client, else the
///   file's, recorded as [`StoredClient::secret_from_entry`] and never
///   copied. Never another client's secret, nor one kept for another
///   authorization server.
#[must_use]
pub fn client_to_keep(
    entered: &EnteredClient,
    kept: Option<&StoredClient>,
    configured: Option<&crate::mcp::OAuthClientConfig>,
) -> Option<StoredClient> {
    let issuer = (!entered.issuer.is_empty()).then(|| entered.issuer.clone());
    let entry_id = configured.and_then(|config| config.client_id.as_deref());
    let names_entry = entry_id == Some(entered.client_id.as_str());
    // What is kept for the entry's own client — never an entered client.
    let kept_for_entry = kept.filter(|kept| !kept.is_entered());
    if names_entry
        && configured.and_then(crate::mcp::OAuthClientConfig::exact_port) == entered.callback_port
    {
        return match &entered.client_secret {
            Some(secret) => Some(StoredClient {
                client_secret: Some(secret.clone()),
                issuer,
                ..StoredClient::default()
            }),
            None => kept_for_entry.cloned(),
        };
    }
    let (client_secret, secret_from_entry) = match &entered.client_secret {
        Some(secret) => (Some(secret.clone()), false),
        None if !entered.keep_secret => (None, false),
        None => {
            let same = kept.filter(|kept| {
                kept.client_id.as_deref() == Some(entered.client_id.as_str())
                    && kept.issuer == issuer
            });
            match same {
                Some(same) => (same.client_secret.clone(), same.secret_from_entry),
                None if names_entry => match kept_for_entry
                    .filter(|kept| kept.belongs_to(&entered.issuer))
                    .and_then(|kept| kept.client_secret.clone())
                {
                    Some(secret) => (Some(secret), false),
                    None => (None, configured.is_some_and(|c| c.client_secret.is_some())),
                },
                None => (None, false),
            }
        }
    };
    Some(StoredClient {
        client_id: Some(entered.client_id.clone()),
        client_secret,
        callback_port: entered.callback_port,
        issuer,
        secret_from_entry,
    })
}

/// What a write to the store's `clients` section did besides keeping the
/// client: a client entered in `/mcp` it took out, and a login it removed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ClientChange {
    /// A client entered in `/mcp` was kept for the server and no longer is
    /// — replaced by another, or forgotten.
    pub forgot_entered: bool,
    /// The server's login went with the client it was made as.
    pub signed_out: bool,
}

/// The `clients` and `servers` sections rewritten for `server_url` in one
/// go — the core of every client write that touches a login. `decide` gets
/// the client kept for the server and the id its login was made as, and
/// answers the record to keep in its place (`None` keeps none) and whether
/// the login goes; or `None` to leave the store as it is. `None` back when
/// there is nothing to write, or the `clients` section is no object.
fn rewrite_client_and_login(
    contents: &str,
    server_url: &str,
    decide: impl FnOnce(Option<&StoredClient>, Option<&str>) -> Option<(Option<StoredClient>, bool)>,
) -> Option<(String, ClientChange)> {
    let kept = parse_client_store(contents).remove(server_url);
    let login = parse_token_store(contents)
        .remove(server_url)
        .map(|login| login.client_id);
    let (client, drop_login) = decide(kept.as_ref(), login.as_deref())?;
    let signed_out = drop_login && login.is_some();
    // Nothing kept, nothing to keep, no login to take: no write, and no
    // store made for it.
    if kept.is_none() && client.is_none() && !signed_out {
        return None;
    }
    let mut updated = record_client(contents, server_url, client.as_ref())?;
    if signed_out {
        updated = record_tokens(&updated, server_url, None);
    }
    let kept_id = kept.as_ref().and_then(|kept| kept.client_id.as_deref());
    let now_id = client
        .as_ref()
        .and_then(|client| client.client_id.as_deref());
    let change = ClientChange {
        forgot_entered: kept_id.is_some() && kept_id != now_id,
        signed_out,
    };
    Some((updated, change))
}

/// Re-render the store with the client a submitted form describes kept for
/// `server_url` ([`client_to_keep`]) — and, in the same write, the server's
/// login removed when it was made as another client than that one: a login
/// goes with the client it was made as, and keeps its own copy of that
/// client's id and secret, so left behind it would go on presenting a
/// client nothing names. The same id — a new secret, a new port — keeps it.
/// `None` when there is nothing to write, or the `clients` section is no
/// object.
#[must_use]
pub fn record_entered_client(
    contents: &str,
    server_url: &str,
    entered: &EnteredClient,
    configured: Option<&crate::mcp::OAuthClientConfig>,
) -> Option<(String, ClientChange)> {
    rewrite_client_and_login(contents, server_url, |kept, login| {
        let client = client_to_keep(entered, kept, configured);
        let drop_login = login.is_some_and(|made_as| made_as != entered.client_id);
        Some((client, drop_login))
    })
}

/// Re-render the store with the client entered in `/mcp` for `server_url`
/// forgotten — and its login, only when that login was made as it. `None`
/// when no entered client is kept: a secret kept for the entry's own client
/// is not one, and stays.
#[must_use]
pub fn forget_entered_record(contents: &str, server_url: &str) -> Option<(String, ClientChange)> {
    rewrite_client_and_login(contents, server_url, |kept, login| {
        let id = kept?.client_id.as_deref()?;
        Some((None, login == Some(id)))
    })
}

/// Does the store's `clients` section exist and fail to be an object?
/// Nothing is written over one that can't be read.
fn clients_unreadable(contents: &str) -> bool {
    serde_json::from_str::<Value>(contents.trim())
        .ok()
        .and_then(|value| value.get("clients").map(|clients| !clients.is_object()))
        .unwrap_or(false)
}

/// Keep the client a submitted form describes for `server_url`, removing a
/// login made as another client in the same locked read-modify-write
/// ([`record_entered_client`]). What else it did comes back.
///
/// # Errors
/// Nowhere to keep a client, or a store that can't be read or written.
pub fn keep_entered_client(
    path: Option<&Path>,
    server_url: &str,
    entered: &EnteredClient,
    configured: Option<&crate::mcp::OAuthClientConfig>,
) -> Result<ClientChange, String> {
    let Some(path) = path else {
        return Err("there is no config directory to keep the client in".to_string());
    };
    let mut change = ClientChange::default();
    let mut unreadable = false;
    rewrite_store(path, |contents| {
        unreadable = clients_unreadable(contents);
        if unreadable {
            return None;
        }
        let (updated, made) = record_entered_client(contents, server_url, entered, configured)?;
        change = made;
        Some(updated)
    })?;
    if unreadable {
        return Err(format!(
            "the clients section of {} isn't an object",
            path.display()
        ));
    }
    Ok(change)
}

/// Forget the client entered in `/mcp` for `server_url`, and its login when
/// that login was made as it, in one locked read-modify-write
/// ([`forget_entered_record`]) — and nothing else.
///
/// # Errors
/// A store that can't be read or written.
pub fn forget_entered_client(
    path: Option<&Path>,
    server_url: &str,
) -> Result<ClientChange, String> {
    // Nothing to forget where there is no store: not even its lock is made.
    let Some(path) = path.filter(|path| path.exists()) else {
        return Ok(ClientChange::default());
    };
    let mut change = ClientChange::default();
    rewrite_store(path, |contents| {
        let (updated, made) = forget_entered_record(contents, server_url)?;
        change = made;
        Some(updated)
    })?;
    Ok(change)
}

/// The server entry's own client, as the client form needs it — `None`
/// where the entry names none. A secret is in effect for it when one is
/// kept for it for this authorization server (`authority`), or its file
/// spells one.
fn entry_client(
    configured: Option<&crate::mcp::OAuthClientConfig>,
    kept: Option<&StoredClient>,
    authority: &str,
) -> Option<crate::mcp::McpEntryClient> {
    let config = configured?;
    let kept_secret = kept.is_some_and(|kept| {
        !kept.is_entered() && kept.client_secret.is_some() && kept.belongs_to(authority)
    });
    Some(crate::mcp::McpEntryClient {
        client_id: config.client_id.clone()?,
        port: config.exact_port(),
        redirect_uri: config.registered_redirect_uri(),
        has_secret: kept_secret || config.client_secret.is_some(),
    })
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
    let flat = || crate::mcp::body_line(body, 200);
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
/// and Entra's tenants publish); without one, the two root documents.
///
/// A path issuer's list ends on the root document too: a server that mounts
/// its metadata routes at the origin publishes there whatever path its
/// issuer has. For a path issuer the root is usually some *other* issuer's
/// document — which is why every candidate is held to [`issuer_matches`],
/// and that one, naming itself, is skipped.
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
        format!("{origin}/.well-known/oauth-authorization-server"),
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
    /// The documentation link the metadata gives (`documentation_link`).
    pub documentation: Option<String>,
}

impl AuthServerMeta {
    /// What names this authorization server to a kept client: its `issuer`
    /// identifier, else the origin of its token endpoint (a server found
    /// through a document that names none). A client entered for a server is
    /// bound to it and presented to no other (SEP-2352).
    #[must_use]
    pub fn authority(&self) -> String {
        self.issuer
            .clone()
            .unwrap_or_else(|| origin_of(&self.token_endpoint))
    }
}

/// The HTTP client every request of a sign-in goes out on — one that names
/// this app (`llm::mcp_auth_http_client`): an authorization server behind a
/// gateway is refused there, with the gateway's 403, to a request naming no
/// client, exactly as its MCP server is.
fn auth_client(timeout: Duration) -> Result<reqwest::blocking::Client, String> {
    crate::llm::mcp_auth_http_client(timeout).map_err(|e| e.to_string())
}

/// GET a metadata document as JSON, waiting at most `timeout`.
fn get_json_within(url: &str, timeout: Duration) -> Result<Value, String> {
    let client = auth_client(timeout)?;
    let response = client
        .get(url)
        .header("Accept", "application/json")
        .header("MCP-Protocol-Version", crate::mcp::PROTOCOL_VERSION)
        .send()
        .map_err(|e| format!("request failed: {}", crate::llm::transport_message(&e)))?;
    if !response.status().is_success() {
        return Err(format!("HTTP {}", response.status().as_u16()));
    }
    response
        .json::<Value>()
        .map_err(|e| format!("bad JSON: {e}"))
}

/// POST a form and hand back `(status, body)` raw — the exchange and the
/// refresh classify a refusal themselves (`Err` only when no answer arrived
/// at all).
/// `basic` is a client's HTTP Basic `Authorization` value
/// ([`client_authentication`]).
fn post_form_raw(
    url: &str,
    params: &[(String, String)],
    basic: Option<&str>,
) -> Result<(u16, String), String> {
    let client = auth_client(HTTP_TIMEOUT)?;
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
        .map_err(|e| format!("request failed: {}", crate::llm::transport_message(&e)))?;
    let status = response.status().as_u16();
    Ok((status, response.text().unwrap_or_default()))
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
        _ => format!("HTTP {status}: {}", crate::mcp::body_line(body, 200)),
    }
}

/// Resolve the authorization server for `server_url`, seeded by the 401's
/// challenge when one was captured — every document checked before it is
/// trusted (`docs/mcp.md`):
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
        documentation: documentation_link(&meta, resource_meta.as_ref()),
    })
}

/// The documentation link a server's metadata gives: the authorization
/// server's `service_documentation` (RFC 8414 §2), else the protected
/// resource's `resource_documentation` (RFC 9728 §2) — each kept only as a
/// plain web address a click can open whole.
#[must_use]
pub fn documentation_link(auth_meta: &Value, resource_meta: Option<&Value>) -> Option<String> {
    let usable = |url: &&str| {
        url::Url::parse(url).is_ok_and(|parsed| {
            matches!(parsed.scheme(), "http" | "https")
                && parsed.host().is_some()
                && parsed.username().is_empty()
                && parsed.password().is_none()
        }) && crate::links::osc8_uri_len(url) <= crate::links::HYPERLINK_URI_MAX_BYTES
    };
    let read = |meta: &Value, key: &str| {
        meta.get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(usable)
            .map(str::to_string)
    };
    read(auth_meta, "service_documentation")
        .or_else(|| resource_meta.and_then(|meta| read(meta, "resource_documentation")))
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
/// server this says `true` for as needing a sign-in (`docs/mcp.md`). `false`
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

/// Why a registration yielded no client.
#[derive(Debug, Clone, PartialEq, Eq)]
enum RegistrationFailure {
    /// The authorization server answered, and its answer was no — the
    /// server's own reason. Registration is not a way in here, which leaves
    /// a client of the user's own (`docs/mcp.md` *Pre-registered clients*).
    Refused(String),
    /// No answer, a server that couldn't answer, or an answer that is no
    /// registration — worded whole.
    Failed(String),
}

/// Dynamically register a client (RFC 7591) for `redirect_uri`, asking for
/// an authentication method the authorization server offers and naming the
/// scopes the sign-in is about to ask for (`scope`; none when it asks for
/// none). An authorization server may keep a client to the scopes it
/// registered with, and then turns the authorize request away
/// (`invalid_scope`) from one that registered none — after the browser has
/// opened. A refusal carries the server's own reason (RFC 7591 §3.2.2).
pub fn register_client(
    registration_endpoint: &str,
    redirect_uri: &str,
    supported_methods: &[String],
    scope: Option<&str>,
) -> Result<ClientIdentity, String> {
    try_register(
        registration_endpoint,
        redirect_uri,
        supported_methods,
        scope,
    )
    .map_err(|failure| match failure {
        RegistrationFailure::Refused(detail) => format!("registration failed: {detail}"),
        RegistrationFailure::Failed(detail) => detail,
    })
}

/// [`register_client`], telling a refusal — a 4xx that is the server's
/// decision, not a timeout or a rate limit — from a registration that
/// merely failed.
fn try_register(
    registration_endpoint: &str,
    redirect_uri: &str,
    supported_methods: &[String],
    scope: Option<&str>,
) -> Result<ClientIdentity, RegistrationFailure> {
    use RegistrationFailure::{Failed, Refused};
    let client = auth_client(HTTP_TIMEOUT).map_err(Failed)?;
    let method = registration_auth_method(supported_methods);
    let mut body = serde_json::json!({
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
    if let Some(scope) = scope.map(str::trim).filter(|scope| !scope.is_empty()) {
        body["scope"] = Value::String(scope.to_string());
    }
    let response = client
        .post(registration_endpoint)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .body(body.to_string())
        .send()
        .map_err(|e| {
            Failed(format!(
                "registration failed: {}",
                crate::llm::transport_message(&e)
            ))
        })?;
    let status = response.status().as_u16();
    let text = response.text().unwrap_or_default();
    if !(200..300).contains(&status) {
        let detail = oauth_error_detail(status, &text);
        // 408 and 429 are "not now", like a 5xx: only a decision is a
        // refusal.
        return Err(
            if (400..500).contains(&status) && status != 408 && status != 429 {
                Refused(detail)
            } else {
                Failed(format!("registration failed: {detail}"))
            },
        );
    }
    let value: Value =
        serde_json::from_str(&text).map_err(|e| Failed(format!("registration: bad JSON: {e}")))?;
    let field = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_string);
    let client_id = field("client_id")
        .filter(|id| !id.is_empty())
        .ok_or_else(|| Failed("registration returned no client_id".to_string()))?;
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

/// How long a port named exactly gets to come free before it counts as
/// taken. A sign-in that supersedes another — the client changed, or
/// Authenticate pressed again — wants the very port the old one holds, and
/// the old one lets go only at its next cancel poll.
const PORT_HANDOFF_WAIT: Duration = Duration::from_millis(400);

/// Bind the callback on exactly `port` — the one a provider was told —
/// waiting [`PORT_HANDOFF_WAIT`] for it to come free, answering on `path`.
fn bind_exactly(port: u16, path: &str) -> Result<CallbackServer, String> {
    let until = Instant::now() + PORT_HANDOFF_WAIT;
    loop {
        match CallbackServer::bind(&[port], path.to_string()) {
            Ok(callback) => return Ok(callback),
            Err(error) if Instant::now() >= until => return Err(error),
            Err(_) => std::thread::sleep(Duration::from_millis(25)),
        }
    }
}

/// Bind the callback for a sign-in whose entry may name its port: exactly
/// that one — failing with it named, since the provider was told that
/// redirect URI — else the first of `preferred` that is free; answering on
/// `path`.
fn bind_callback(
    exact: Option<u16>,
    preferred: &[u16],
    path: &str,
) -> Result<CallbackServer, String> {
    match exact {
        Some(port) => bind_exactly(port, path).map_err(|e| {
            format!("the callback port {port} this server's entry names isn't free: {e}")
        }),
        None => CallbackServer::bind(preferred, path.to_string()),
    }
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
    /// The token store: the grant, and the client kept for the server — a
    /// secret for the entry's own client, or a whole client entered on the
    /// `/mcp` auth page ([`StoredClient`]), which the flow finds itself.
    pub store_path: Option<&'a Path>,
    /// The entry's `oauth` block as the entry spells it.
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

/// The port an entered client's sign-in listens on when nothing names one
/// — fixed, never ephemeral, because the client form shows the redirect URI
/// before the user creates the client, and a provider that matches redirect
/// URIs exactly has to be told the very one the sign-in will send
/// (`docs/mcp.md` *Pre-registered clients*).
pub const DEFAULT_CALLBACK_PORT: u16 = 8976;

/// What a sign-in asks the user for — [`crate::mcp::McpClientRequest`], the
/// pure half the `/mcp` auth page's client form is built over.
pub type ClientRequest = crate::mcp::McpClientRequest;

/// Why a sign-in ended without a grant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthError {
    /// The flow has no client to present, or the one it presented was not
    /// enough: the user is asked for one — the `/mcp` auth page's client
    /// form, the spec's last registration step.
    NeedsClient(Box<ClientRequest>),
    /// Anything else, worded for the user.
    Failed(String),
}

impl From<String> for AuthError {
    fn from(detail: String) -> Self {
        Self::Failed(detail)
    }
}

impl std::fmt::Display for AuthError {
    /// The outcome in words — for a request, what to do where no page is
    /// open to ask: both ways to give the sign-in a client.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let request = match self {
            Self::Failed(detail) => return f.write_str(detail),
            Self::NeedsClient(request) => request,
        };
        let host = crate::mcp::issuer_host(&request.issuer);
        if let Some(reason) = &request.reason {
            write!(f, "{reason} ")?;
        }
        if request.fixed_id {
            return f.write_str(
                "Authenticate in /mcp to enter the client's secret, or keep it with \
                 `alter-zero mcp add … --client-secret`",
            );
        }
        if request.reason.is_none() {
            write!(f, "{host} registers no clients itself. ")?;
        }
        write!(
            f,
            "Signing in needs an OAuth client you create with {host}: Authenticate in /mcp to \
             enter it, or add it as oauth.clientId in the server's entry (`alter-zero mcp add \
             --client-id … --client-secret`)"
        )
    }
}

/// Where the client a sign-in presents came from — which decides what the
/// authorization server turning it away means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClientSource {
    /// Entered on the `/mcp` auth page and kept with the tokens.
    Entered,
    /// The server entry's `oauth.clientId`; its secret the one kept for it
    /// with the tokens, else the entry's own, else none.
    Configured,
    /// Registered by this app, now or by an earlier sign-in.
    Registered,
}

/// Is a token endpoint's refusal a verdict on the **client** — its id, its
/// secret — rather than on the login? RFC 6749 §5.2's `invalid_client` and
/// `unauthorized_client`, and any refusal whose description names the
/// client secret: Google answers a missing one with `invalid_request`
/// ("client_secret is missing."). A refused code or grant is the login's.
fn client_refused(body: &str) -> bool {
    let Ok(value) = serde_json::from_str::<Value>(body.trim()) else {
        return false;
    };
    let field = |key: &str| value.get(key).and_then(Value::as_str);
    if matches!(
        field("error"),
        Some("invalid_client" | "unauthorized_client")
    ) {
        return true;
    }
    field("error_description").is_some_and(|description| {
        let description = description.to_ascii_lowercase();
        description.contains("client_secret") || description.contains("client secret")
    })
}

/// The client form a server's own `/mcp` page opens once its authorization
/// server is known ([`client_lookup`]): under that server's issuer and the
/// documentation link its metadata gives, the client in effect, to change —
/// one entered for that authorization server, else the entry's own, whole,
/// on its exact port — and, where none is, a new one, on the port the next
/// sign-in listens on. The form always knows the entry's own client, to
/// tell from what is typed whether it still describes that client.
#[must_use]
pub fn client_request_for(
    configured: Option<&crate::mcp::OAuthClientConfig>,
    kept: Option<&StoredClient>,
    meta: &AuthServerMeta,
) -> ClientRequest {
    let authority = meta.authority();
    let entry_port = configured.and_then(|config| config.callback_port);
    let entry = entry_client(configured, kept, &authority);
    let base = ClientRequest {
        documentation: meta.documentation.clone(),
        entry: entry.clone(),
        ..ClientRequest::default()
    };
    let entered =
        kept.filter(|kept| kept.is_entered() && kept.issuer.as_deref() == Some(authority.as_str()));
    if let Some((kept, client_id)) = entered.and_then(|kept| Some((kept, kept.client_id.clone()?)))
    {
        return ClientRequest {
            issuer: authority,
            client_id: Some(client_id),
            has_secret: kept.secret_in_effect(configured).is_some(),
            callback_port: Some(
                kept.callback_port
                    .or(entry_port)
                    .unwrap_or(DEFAULT_CALLBACK_PORT),
            ),
            ..base
        };
    }
    if let Some(entry) = entry {
        return ClientRequest {
            issuer: authority,
            client_id: Some(entry.client_id),
            has_secret: entry.has_secret,
            callback_port: entry.port,
            ..base
        };
    }
    ClientRequest {
        issuer: authority,
        callback_port: Some(
            entry_port
                .or(kept.and_then(|kept| kept.callback_port))
                .unwrap_or(DEFAULT_CALLBACK_PORT),
        ),
        ..base
    }
}

/// The lookup `Set`/`Change OAuth client` runs before its form opens: discovery
/// alone — nothing registered, no URL shown, no token asked for — then the
/// form over the client on hand ([`client_request_for`]), so the form names
/// the issuer a client is created with and the server's documentation link.
/// `cancel` is checked on either side of discovery.
///
/// # Errors
/// Discovery failed, or the lookup was called off — in words.
pub fn client_lookup(target: &SignIn<'_>, cancel: &CancelToken) -> Result<ClientRequest, String> {
    let still_wanted = || {
        if cancel.is_cancelled() {
            Err("cancelled".to_string())
        } else {
            Ok(())
        }
    };
    still_wanted()?;
    let meta = discover(target.server_url, target.challenge)?;
    still_wanted()?;
    let kept = load_client(target.store_path, target.server_url);
    Ok(client_request_for(target.client, kept.as_ref(), &meta))
}

/// What `mcp get` says of the OAuth client a server signs in with
/// (`docs/mcp-cli.md`), a line each: the entry's own — its port, its scopes,
/// and where its secret is — or, where the entry names none, the client
/// entered on the `/mcp` auth page and the port it signs in on. Where a
/// secret is, never what it is. Empty when the sign-in has no client of the
/// user's.
#[must_use]
pub fn client_summary(
    configured: Option<&crate::mcp::OAuthClientConfig>,
    kept: Option<&StoredClient>,
) -> Vec<String> {
    let mut lines = Vec::new();
    let entry_id = configured.and_then(|config| config.client_id.as_deref());
    let entered = kept.filter(|kept| entry_id.is_none() && kept.client_id.is_some());
    let entry_port = configured.and_then(|config| config.callback_port);
    match (entry_id, entered.and_then(|kept| kept.client_id.as_deref())) {
        (Some(id), _) => {
            lines.push(format!("OAuth client: {id}"));
            lines.extend(entry_port.map(|port| format!("Callback port: {port}")));
            lines.extend(
                configured
                    .filter(|config| config.callback_url.is_some())
                    .and_then(crate::mcp::OAuthClientConfig::registered_redirect_uri)
                    .map(|uri| format!("Redirect URI: {uri}")),
            );
        }
        (None, Some(id)) => {
            lines.push(format!("OAuth client: {id} (entered in /mcp)"));
            let port = entered
                .and_then(|kept| kept.callback_port)
                .or(entry_port)
                .unwrap_or(DEFAULT_CALLBACK_PORT);
            lines.push(format!("Callback port: {port}"));
        }
        (None, None) => lines.extend(entry_port.map(|port| format!("Callback port: {port}"))),
    }
    if let Some(scopes) = configured.and_then(|config| config.scopes.as_deref()) {
        lines.push(format!("Scopes: {scopes}"));
    }
    let in_file = configured.is_some_and(|config| config.client_secret.is_some());
    let kept_secret = match entry_id {
        Some(id) => kept.is_some_and(|kept| kept.secret_for(id).is_some()),
        None => entered.is_some_and(|kept| kept.client_secret.is_some()),
    };
    if in_file && entry_id.is_some() {
        lines.push("Client secret: ***** (in the file)".to_string());
    } else if kept_secret {
        lines.push("Client secret: kept with the tokens".to_string());
    }
    lines
}

/// Why a client kept for the server is not presented: it was created with
/// another authorization server than the one the server signs in through
/// now (SEP-2352) — named by host, or whole where the hosts agree.
fn moved_reason(kept: &StoredClient, authority: &str) -> Option<String> {
    let old = kept.issuer.as_deref()?;
    if kept.client_id.is_none() || kept.belongs_to(authority) {
        return None;
    }
    let (old_host, new_host) = (
        crate::mcp::issuer_host(old),
        crate::mcp::issuer_host(authority),
    );
    let (old, new) = if old_host == new_host {
        (old.to_string(), authority.to_string())
    } else {
        (old_host, new_host)
    };
    Some(format!(
        "The client kept for this server was created with {old}, and the server now signs \
         in through {new}."
    ))
}

/// Run the whole interactive flow for `target`, blocking this (worker)
/// thread: discovery → a client → the authorize URL → callback/paste →
/// exchange → persist. The client: one entered on the `/mcp` auth page for
/// this authorization server — what the user last said, and the one way to
/// give a project's or a plugin's server another client without writing to
/// a file that is not theirs — then the entry's pre-registered one, then a
/// reused or new registration; and where none of those is to be had, or the
/// authorization server turns the user's own client away, the flow ends
/// asking for one ([`AuthError::NeedsClient`]). The URL is handed to `progress` and nothing
/// else: no browser is launched, the user opens it from the auth page.
/// `paste_rx` delivers the user's pasted redirect URL. `cancel` is checked
/// between the steps, so an Esc during discovery never shows a URL.
pub fn run_auth_flow(
    target: &SignIn<'_>,
    paste_rx: &mpsc::Receiver<String>,
    progress: &dyn AuthProgress,
    cancel: &CancelToken,
) -> Result<(), AuthError> {
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
    // A grant with nowhere to go is a dead end — said before the URL is
    // shown, not after the user has signed in.
    if store_path.is_none() {
        return Err("there is no config directory to keep the grant in"
            .to_string()
            .into());
    }
    let meta = discover(server_url, challenge)?;
    still_wanted()?;
    let stored = load_tokens(store_path, server_url);
    // The client kept for this server — a secret for the entry's own, or a
    // whole one entered on the auth page — presented only to the
    // authorization server it was created with (SEP-2352).
    let authority = meta.authority();
    let host = crate::mcp::issuer_host(&authority);
    let kept = load_client(store_path, server_url);
    let entered_here = kept
        .as_ref()
        .filter(|client| client.is_entered() && client.issuer.as_deref() == Some(&authority));
    let entry_secret_kept = kept
        .as_ref()
        .filter(|client| !client.is_entered() && client.belongs_to(&authority))
        .and_then(|client| client.client_secret.clone());
    // The entry's own client, for a form whose id can change to tell
    // whether it still describes that client.
    let entry = entry_client(configured, kept.as_ref(), &authority);
    // Re-authorizing asks for what was granted before *and* what the server
    // now wants — the step-up rule ("include these scopes alongside any
    // previously granted scopes"), so a scope upgrade never drops the rest;
    // another authorization server's scopes mean nothing here.
    let granted = stored
        .as_ref()
        .filter(|tokens| same_authorization_server(tokens, &meta))
        .and_then(|tokens| tokens.scope.as_deref());
    // The entry's own scopes, when it names them, are what the user
    // registered the client for — asked for as they are. Settled before any
    // registration, which names them too.
    let scopes = match configured.and_then(|config| config.scopes.clone()) {
        Some(scopes) => Some(scopes),
        None => scope_union(&[meta.scopes.as_deref(), granted]),
    };
    // The flow's one way of asking the user for a client.
    let ask = |client_id: Option<String>,
               fixed_id: bool,
               has_secret: bool,
               callback_port: Option<u16>,
               reason: Option<String>| {
        AuthError::NeedsClient(Box::new(ClientRequest {
            issuer: authority.clone(),
            client_id,
            fixed_id,
            has_secret,
            callback_port,
            redirect_uri: None,
            documentation: meta.documentation.clone(),
            reason,
            entry: entry.clone(),
        }))
    };
    let entry_port = configured.and_then(|config| config.callback_port);
    let (callback, redirect_uri, client, source) = if let Some((entered, client_id)) =
        entered_here.and_then(|kept| Some((kept, kept.client_id.clone()?)))
    {
        // The client entered on the auth page — it outranks the entry's
        // own: on exactly its own port — the redirect URI the form showed,
        // which the user may have had to register — and never a
        // registration. A port that is taken asks again, the form's own
        // field being where it changes.
        let port = entered
            .callback_port
            .or(entry_port)
            .unwrap_or(DEFAULT_CALLBACK_PORT);
        let client_secret = entered.secret_in_effect(configured);
        let callback = bind_exactly(port, "/callback").map_err(|_| {
            ask(
                Some(client_id.clone()),
                false,
                client_secret.is_some(),
                Some(port),
                Some(format!(
                    "Port {port} is in use, so the sign-in can't listen on it. Free it, or \
                     enter another port and add its redirect URI to the client."
                )),
            )
        })?;
        let redirect_uri = crate::mcp::oauth_redirect_uri(callback.port);
        let client = ClientIdentity {
            client_id,
            auth_method: configured_auth_method(&meta.token_auth_methods, client_secret.is_some()),
            client_secret,
            redirect_uri: Some(redirect_uri.clone()),
        };
        (callback, redirect_uri, client, ClientSource::Entered)
    } else if let Some((config, client_id)) =
        configured.and_then(|config| Some((config, config.client_id.clone()?)))
    {
        // The client the user registered with the provider: on exactly the
        // port its redirect URI names — the provider was told that one —
        // else any (a native client takes any loopback port, RFC 8252). Its
        // secret is the one kept for it with the tokens, else the entry's.
        let client_secret = entry_secret_kept
            .clone()
            .or_else(|| config.client_secret.clone());
        // The redirect URI its entry names (`callbackUrl`), the bound port
        // added where it names none, answered on its own path.
        let callback = bind_callback(config.exact_port(), &[0; 9], &config.callback_path())?;
        let redirect_uri = config.redirect_uri_on(callback.port);
        let client = ClientIdentity {
            client_id,
            auth_method: configured_auth_method(&meta.token_auth_methods, client_secret.is_some()),
            client_secret,
            redirect_uri: Some(redirect_uri.clone()),
        };
        (callback, redirect_uri, client, ClientSource::Configured)
    } else {
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
        let ports: Vec<u16> = preferred
            .into_iter()
            .chain(std::iter::repeat_n(0, 8))
            .collect();
        let callback = bind_callback(entry_port, &ports, "/callback")?;
        let redirect_uri = crate::mcp::oauth_redirect_uri(callback.port);
        // The port a client entered next would sign in on — what the form
        // shows as its redirect URI.
        let next_port = entry_port
            .or(kept.as_ref().and_then(|client| client.callback_port))
            .unwrap_or(DEFAULT_CALLBACK_PORT);
        let client = match (reused, &meta.registration_endpoint) {
            (Some(client), _) if client.redirect_uri.as_deref() == Some(redirect_uri.as_str()) => {
                client
            }
            (_, Some(endpoint)) => match try_register(
                endpoint,
                &redirect_uri,
                &meta.token_auth_methods,
                scopes.as_deref(),
            ) {
                Ok(client) => client,
                // Registration is no way in here: what is left is a client
                // of the user's own.
                Err(RegistrationFailure::Refused(detail)) => {
                    return Err(ask(
                        None,
                        false,
                        false,
                        Some(next_port),
                        Some(format!("{host} wouldn't register this app. ({detail})")),
                    ));
                }
                Err(RegistrationFailure::Failed(detail)) => return Err(detail.into()),
            },
            // Nothing to register with: the old client on a new port is
            // the one chance (an authorization server honouring RFC 8252
            // §7.3 accepts it).
            (Some(client), None) => client,
            // No client anywhere and none to be registered: the user's own
            // is the last step.
            (None, None) => {
                return Err(ask(
                    None,
                    false,
                    false,
                    Some(next_port),
                    kept.as_ref()
                        .and_then(|kept| moved_reason(kept, &authority)),
                ));
            }
        };
        (callback, redirect_uri, client, ClientSource::Registered)
    };
    still_wanted()?;
    let (verifier, code_challenge, state) = pkce_and_state()?;
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
    let port = callback.port;
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
    let (status, body) = post_form_raw(&meta.token_endpoint, &params, basic.as_deref())
        .map_err(|e| format!("token exchange failed: {e}"))?;
    if !(200..300).contains(&status) {
        let detail = oauth_error_detail(status, &body);
        // A verdict on the user's own client is theirs to answer: the form
        // again, with what the server said. A registration's is this app's.
        if client_refused(&body) {
            let has_secret = client.client_secret.is_some();
            match source {
                ClientSource::Entered => {
                    return Err(ask(
                        Some(client.client_id),
                        false,
                        has_secret,
                        Some(port),
                        Some(format!(
                            "{host} turned this client away — check its ID and its secret. \
                             ({detail})"
                        )),
                    ));
                }
                // The entry's own client, whichever secret it presented: a
                // secret typed now outranks the file's, so it is asked for,
                // none offered as kept.
                ClientSource::Configured => {
                    let mut request = ask(
                        Some(client.client_id),
                        true,
                        false,
                        configured.and_then(crate::mcp::OAuthClientConfig::exact_port),
                        Some(format!(
                            "{host} turned away the client this server's entry names — enter \
                             its secret. ({detail})"
                        )),
                    );
                    // The redirect URI the entry names is the one to show.
                    if let (AuthError::NeedsClient(request), Some(config)) = (
                        &mut request,
                        configured.filter(|c| c.callback_url.is_some()),
                    ) {
                        request.redirect_uri = config.registered_redirect_uri();
                    }
                    return Err(request);
                }
                ClientSource::Registered => {}
            }
        }
        return Err(format!("token exchange failed: {detail}").into());
    }
    let response: Value =
        serde_json::from_str(&body).map_err(|e| format!("token exchange failed: bad JSON: {e}"))?;
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
        .map_err(|e| format!("signed in, but the grant couldn't be saved: {e}").into())
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
                // Last, the root: where a server that mounts its metadata at
                // the origin publishes, whatever its issuer's path.
                "https://auth.test/.well-known/oauth-authorization-server",
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

    /// What a server says about itself is a documentation link: the
    /// authorization server's `service_documentation` (RFC 8414), else the
    /// resource's `resource_documentation` (RFC 9728).
    #[test]
    fn discovery_reads_the_documentation_link_and_prefers_the_servers() {
        let routes = |as_docs: &'static str| {
            vec![
                (
                    "/.well-known/oauth-protected-resource/mcp",
                    r#"{"resource":"{base}/mcp","authorization_servers":["{base}"],"resource_documentation":"https://docs.example.test/resource"}"#,
                ),
                ("/.well-known/oauth-authorization-server", as_docs),
            ]
        };
        let base = discovery_fixture(routes(
            r#"{"issuer":"{base}","authorization_endpoint":"{base}/auth","token_endpoint":"{base}/token","code_challenge_methods_supported":["S256"],"service_documentation":"https://docs.example.test/oauth"}"#,
        ));
        let meta = discover(&format!("{base}/mcp"), None).unwrap();
        assert_eq!(
            meta.documentation.as_deref(),
            Some("https://docs.example.test/oauth")
        );
        let base = discovery_fixture(routes(
            r#"{"issuer":"{base}","authorization_endpoint":"{base}/auth","token_endpoint":"{base}/token","code_challenge_methods_supported":["S256"]}"#,
        ));
        let meta = discover(&format!("{base}/mcp"), None).unwrap();
        assert_eq!(
            meta.documentation.as_deref(),
            Some("https://docs.example.test/resource")
        );
    }

    /// A documentation link is kept only as an `http(s)` URL with a host,
    /// no user name or password, and no longer than a hyperlink carries.
    #[test]
    fn a_documentation_link_is_kept_only_when_it_is_a_plain_web_address() {
        let long = format!(
            "https://docs.example.test/{}",
            "a".repeat(crate::links::HYPERLINK_URI_MAX_BYTES)
        );
        let as_meta = |docs: &str| serde_json::json!({ "service_documentation": docs });
        for kept in [
            "https://developers.example.test/identity/oauth2",
            "http://127.0.0.1:8080/docs",
        ] {
            assert_eq!(
                documentation_link(&as_meta(kept), None).as_deref(),
                Some(kept)
            );
        }
        for dropped in [
            "javascript:alert(1)",
            "file:///etc/passwd",
            "ftp://docs.example.test/",
            "https://accounts.example.test@evil.example/docs",
            "https://user:pw@docs.example.test/",
            "not a url",
            long.as_str(),
        ] {
            assert_eq!(
                documentation_link(&as_meta(dropped), None),
                None,
                "{dropped}"
            );
        }
        // A dropped server link leaves the resource's in its place.
        let resource = serde_json::json!({ "resource_documentation": "https://r.example.test/" });
        assert_eq!(
            documentation_link(&as_meta("ftp://x/"), Some(&resource)).as_deref(),
            Some("https://r.example.test/")
        );
        assert_eq!(documentation_link(&serde_json::json!({}), None), None);
        assert_eq!(
            documentation_link(&serde_json::json!({ "service_documentation": 5 }), None),
            None
        );
    }

    /// An issuer with a path whose server mounts its metadata at the origin
    /// — the one place it publishes — is found there, last: the document
    /// names that issuer, so it is that issuer's. A root document naming
    /// another issuer is skipped like any other, wherever it is.
    #[test]
    fn a_path_issuer_publishing_only_at_the_root_is_found_there() {
        let resource = (
            "/.well-known/oauth-protected-resource/mcp",
            r#"{"resource":"{base}/mcp","authorization_servers":["{base}/mcp"]}"#,
        );
        let base = discovery_fixture(vec![
            resource,
            (
                "/.well-known/oauth-authorization-server",
                r#"{"issuer":"{base}/mcp","authorization_endpoint":"{base}/mcp/authorize","token_endpoint":"{base}/mcp/token","code_challenge_methods_supported":["S256"]}"#,
            ),
        ]);
        let meta = discover(&format!("{base}/mcp"), None).expect("found at the root");
        assert_eq!(meta.issuer.as_deref(), Some(format!("{base}/mcp").as_str()));
        assert_eq!(meta.token_endpoint, format!("{base}/mcp/token"));
        // The origin's own issuer at the root is not the one asked for.
        let base = discovery_fixture(vec![
            resource,
            (
                "/.well-known/oauth-authorization-server",
                r#"{"issuer":"{base}","authorization_endpoint":"{base}/authorize","token_endpoint":"{base}/token"}"#,
            ),
        ]);
        let refused = discover(&format!("{base}/mcp"), None).expect_err("another issuer's");
        assert!(refused.contains("names issuer"), "{refused}");
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

    /// A client entered on the `/mcp` auth page is kept whole — its id, its
    /// secret, its port and the authorization server it was created with —
    /// in the same `clients` section, apart from the login, beside the
    /// secret-only entry `mcp add --client-secret` writes.
    #[test]
    fn an_entered_client_is_kept_whole_and_apart_from_the_login() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcp-auth.json");
        let server = "https://calendarmcp.googleapis.com/mcp/v1";
        let entered = StoredClient {
            client_id: Some("123.apps.example".to_string()),
            client_secret: Some("s3cret".to_string()),
            callback_port: Some(8976),
            issuer: Some("https://accounts.google.com".to_string()),
            secret_from_entry: false,
        };
        save_client(Some(&path), server, Some(&entered)).unwrap();
        assert_eq!(load_client(Some(&path), server).as_ref(), Some(&entered));
        // The login comes and goes; the client stays.
        let login = StoredTokens {
            access_token: "at".to_string(),
            ..Default::default()
        };
        save_tokens(Some(&path), server, Some(&login)).unwrap();
        save_tokens(Some(&path), server, None).unwrap();
        assert_eq!(load_client(Some(&path), server).as_ref(), Some(&entered));
        // The command line's entry is a secret with no id of its own.
        let other = "https://other.test/mcp";
        save_client_secret(Some(&path), other, Some("cli")).unwrap();
        assert_eq!(
            load_client(Some(&path), other),
            Some(StoredClient {
                client_secret: Some("cli".to_string()),
                ..Default::default()
            })
        );
        assert_eq!(
            load_client_secret(Some(&path), server).as_deref(),
            Some("s3cret")
        );
        // A public client keeps no secret, and a forgotten one is gone.
        let public = StoredClient {
            client_secret: None,
            ..entered.clone()
        };
        save_client(Some(&path), server, Some(&public)).unwrap();
        assert_eq!(load_client(Some(&path), server).as_ref(), Some(&public));
        assert_eq!(load_client_secret(Some(&path), server), None);
        save_client(Some(&path), server, None).unwrap();
        assert_eq!(load_client(Some(&path), server), None);
        assert_eq!(
            load_client(Some(&path), other).map(|c| c.client_secret),
            Some(Some("cli".to_string()))
        );
        assert!(save_client(None, server, Some(&entered)).is_err());
        assert!(save_client(None, server, None).is_ok());
    }

    /// A kept secret belongs to one client: the entry's own, or — an entry
    /// with no id, the command line's — the one the server's config names.
    /// And a kept client belongs to the authorization server it was created
    /// with (SEP-2352), one recorded before any sign-in saw it to any.
    #[test]
    fn a_kept_client_answers_only_for_itself_and_its_issuer() {
        let entered = StoredClient {
            client_id: Some("mine".to_string()),
            client_secret: Some("s".to_string()),
            callback_port: Some(8976),
            issuer: Some("https://as.test".to_string()),
            secret_from_entry: false,
        };
        assert_eq!(entered.secret_for("mine"), Some("s"));
        assert_eq!(entered.secret_for("another"), None);
        assert!(entered.belongs_to("https://as.test"));
        assert!(!entered.belongs_to("https://as.test/"));
        assert!(!entered.belongs_to("https://evil.test"));
        let cli = StoredClient {
            client_secret: Some("s".to_string()),
            ..Default::default()
        };
        assert_eq!(cli.secret_for("whatever"), Some("s"));
        assert!(cli.belongs_to("https://anything.test"));
    }

    /// A kept client printed for a log or a failed assertion says a secret
    /// is there, never what it is — and so does a submitted form.
    #[test]
    fn a_kept_client_never_prints_its_secret() {
        let client = StoredClient {
            client_id: Some("cid".to_string()),
            client_secret: Some("GOCSPX-hunter2".to_string()),
            callback_port: Some(8976),
            issuer: Some("https://as.test".to_string()),
            secret_from_entry: false,
        };
        let printed = format!("{client:?}");
        assert!(!printed.contains("GOCSPX"), "{printed}");
        assert!(
            printed.contains("cid") && printed.contains("redacted"),
            "{printed}"
        );
        let submitted = EnteredClient {
            issuer: "https://as.test".to_string(),
            client_id: "cid".to_string(),
            client_secret: Some("GOCSPX-hunter2".to_string()),
            keep_secret: false,
            callback_port: Some(8976),
        };
        let printed = format!("{submitted:?}");
        assert!(!printed.contains("GOCSPX"), "{printed}");
    }

    /// An entered client that signs in with its entry's secret says so in
    /// the store — the secret itself stays in the file — and only a client
    /// can say it. An entered client bound to no authorization server is
    /// presented to none, so the store reads it as nothing.
    #[test]
    fn the_store_keeps_whose_secret_a_client_uses_and_drops_an_unbound_one() {
        let server = "https://s.test/mcp";
        let lent = StoredClient {
            client_id: Some("cid".to_string()),
            callback_port: Some(9000),
            issuer: Some("https://as.test".to_string()),
            secret_from_entry: true,
            ..Default::default()
        };
        let written = record_client("", server, Some(&lent)).unwrap();
        assert!(written.contains("\"secret_from_entry\": true"), "{written}");
        assert!(!written.contains("client_secret"), "{written}");
        assert_eq!(
            parse_client_store(&written).remove(server),
            Some(lent.clone())
        );
        let own = StoredClient {
            secret_from_entry: false,
            ..lent
        };
        let written = record_client("", server, Some(&own)).unwrap();
        assert!(!written.contains("secret_from_entry"), "{written}");
        // Beside a bare secret it means nothing.
        let bare = r#"{"clients":{"https://s.test/mcp":{"client_secret":"shh","secret_from_entry":true}}}"#;
        assert!(!parse_client_store(bare)[server].secret_from_entry);
        // A client bound to no authorization server is nobody's.
        let unbound =
            r#"{"clients":{"https://s.test/mcp":{"client_id":"cid","client_secret":"shh"}}}"#;
        assert_eq!(parse_client_store(unbound).get(server), None);
    }

    /// A store holding `client` and a login made as `login_as` for `server`
    /// — and another server's login, which no write here may touch.
    fn store_with(server: &str, client: Option<&StoredClient>, login_as: Option<&str>) -> String {
        let other = StoredTokens {
            access_token: "other-at".to_string(),
            client_id: "other-cid".to_string(),
            ..Default::default()
        };
        let mut contents = record_tokens("", "https://other.test/mcp", Some(&other));
        if let Some(client) = client {
            contents = record_client(&contents, server, Some(client)).unwrap();
        }
        if let Some(id) = login_as {
            let login = StoredTokens {
                access_token: "at".to_string(),
                client_id: id.to_string(),
                ..Default::default()
            };
            contents = record_tokens(&contents, server, Some(&login));
        }
        contents
    }

    /// A login goes with the client it was made as: keeping a client of
    /// another id than the login's removes the login in the same write as
    /// the client, and the same id — a new secret, a new port — keeps it.
    /// Putting the entry's own client back takes an entered one out, and
    /// its login with it.
    #[test]
    fn keeping_a_client_removes_a_login_made_as_another() {
        const AS: &str = "https://as.test";
        let server = "https://s.test/mcp";
        let typed = StoredClient {
            client_id: Some("typed".to_string()),
            client_secret: Some("typed-secret".to_string()),
            callback_port: Some(9001),
            issuer: Some(AS.to_string()),
            secret_from_entry: false,
        };
        let form = |id: &str, secret: Option<&str>, port: Option<u16>| EnteredClient {
            issuer: AS.to_string(),
            client_id: id.to_string(),
            client_secret: secret.map(str::to_string),
            keep_secret: true,
            callback_port: port,
        };
        let login_of = |contents: &str| {
            parse_token_store(contents)
                .remove(server)
                .map(|login| login.client_id)
        };
        let before = store_with(server, Some(&typed), Some("typed"));
        // Another id: the login goes too.
        let (after, change) =
            record_entered_client(&before, server, &form("mine", Some("s"), Some(9002)), None)
                .expect("written");
        assert_eq!(login_of(&after), None);
        assert_eq!(
            change,
            ClientChange {
                forgot_entered: true,
                signed_out: true
            }
        );
        assert_eq!(
            parse_client_store(&after)[server].client_id.as_deref(),
            Some("mine")
        );
        assert!(parse_token_store(&after).contains_key("https://other.test/mcp"));
        // The same id on another port: the login stays.
        let (after, change) =
            record_entered_client(&before, server, &form("typed", None, Some(9002)), None)
                .expect("written");
        assert_eq!(login_of(&after).as_deref(), Some("typed"));
        assert_eq!(change, ClientChange::default());
        assert_eq!(parse_client_store(&after)[server].callback_port, Some(9002));
        // The entry's own client back: the entered one goes, and the login
        // it made with it.
        let entry = crate::mcp::OAuthClientConfig {
            client_id: Some("file-cid".to_string()),
            callback_port: Some(8000),
            ..Default::default()
        };
        let (after, change) = record_entered_client(
            &before,
            server,
            &form("file-cid", None, Some(8000)),
            Some(&entry),
        )
        .expect("written");
        assert_eq!(parse_client_store(&after).get(server), None);
        assert_eq!(login_of(&after), None);
        assert!(change.forgot_entered && change.signed_out);
        // A login made as the entry's own client stays with it.
        let entrys_login = store_with(server, None, Some("file-cid"));
        let (after, change) = record_entered_client(
            &entrys_login,
            server,
            &form("file-cid", Some("new"), Some(8000)),
            Some(&entry),
        )
        .expect("written");
        assert_eq!(login_of(&after).as_deref(), Some("file-cid"));
        assert_eq!(change, ClientChange::default());
        // Nothing kept, nothing to keep and no login: nothing to write.
        assert_eq!(
            record_entered_client(
                "",
                server,
                &form("file-cid", None, Some(8000)),
                Some(&entry)
            ),
            None
        );
    }

    /// Forgetting an entered client is one write: the client goes, and its
    /// login only when that login was made as it. A secret kept for the
    /// entry's own client is no entered client: nothing to forget.
    #[test]
    fn forgetting_an_entered_client_takes_only_its_own_login() {
        let server = "https://s.test/mcp";
        let typed = StoredClient {
            client_id: Some("typed".to_string()),
            issuer: Some("https://as.test".to_string()),
            ..Default::default()
        };
        let login_of = |contents: &str| {
            parse_token_store(contents)
                .remove(server)
                .map(|login| login.client_id)
        };
        let (after, change) =
            forget_entered_record(&store_with(server, Some(&typed), Some("typed")), server)
                .expect("written");
        assert_eq!(parse_client_store(&after).get(server), None);
        assert_eq!(login_of(&after), None);
        assert_eq!(
            change,
            ClientChange {
                forgot_entered: true,
                signed_out: true
            }
        );
        assert!(parse_token_store(&after).contains_key("https://other.test/mcp"));
        let (after, change) =
            forget_entered_record(&store_with(server, Some(&typed), Some("someone")), server)
                .expect("written");
        assert_eq!(parse_client_store(&after).get(server), None);
        assert_eq!(login_of(&after).as_deref(), Some("someone"));
        assert!(change.forgot_entered && !change.signed_out);
        let cli = StoredClient {
            client_secret: Some("kept".to_string()),
            ..Default::default()
        };
        assert_eq!(
            forget_entered_record(&store_with(server, Some(&cli), Some("x")), server),
            None
        );
        assert_eq!(forget_entered_record("", server), None);
    }

    /// The writes behind the form and Forget: each one locked
    /// read-modify-write, a store whose `clients` is no object refused
    /// rather than written over, and nothing made where there is nothing to
    /// forget.
    #[test]
    fn the_client_writes_are_one_locked_rewrite_each() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcp-auth.json");
        let server = "https://s.test/mcp";
        let typed = EnteredClient {
            issuer: "https://as.test".to_string(),
            client_id: "typed".to_string(),
            client_secret: Some("s".to_string()),
            keep_secret: false,
            callback_port: Some(9001),
        };
        assert_eq!(
            forget_entered_client(Some(&path), server),
            Ok(ClientChange::default())
        );
        assert!(!path.exists(), "nothing to forget made a store");
        assert_eq!(
            keep_entered_client(Some(&path), server, &typed, None),
            Ok(ClientChange::default())
        );
        assert_eq!(
            load_client(Some(&path), server).and_then(|c| c.client_id),
            Some("typed".to_string())
        );
        assert_eq!(
            forget_entered_client(Some(&path), server),
            Ok(ClientChange {
                forgot_entered: true,
                signed_out: false
            })
        );
        assert_eq!(load_client(Some(&path), server), None);
        std::fs::write(&path, r#"{"clients": []}"#).unwrap();
        assert!(keep_entered_client(Some(&path), server, &typed, None).is_err());
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            r#"{"clients": []}"#
        );
        assert!(keep_entered_client(None, server, &typed, None).is_err());
    }

    /// The record a submitted form becomes. The entry's own client — its id
    /// on the entry's exact port — keeps at most a secret: a typed one, else
    /// the one already kept for it, and an entered client in its place goes.
    /// Anything else is an entered client, whole and bound to its issuer —
    /// its secret the one typed; with none typed a public client, unless the
    /// field was left on a kept secret: the same client's own, or, for the
    /// entry's id on another port, the secret kept for the entry's client,
    /// else the file's, read from the entry at each sign-in, never copied.
    #[test]
    fn a_submitted_form_becomes_the_record_kept() {
        const AS: &str = "https://as.test";
        let entry = crate::mcp::OAuthClientConfig {
            client_id: Some("file-cid".to_string()),
            client_secret: Some("in-file".to_string()),
            callback_port: Some(8000),
            ..Default::default()
        };
        let form =
            |id: &str, secret: Option<&str>, keep_secret: bool, port: Option<u16>| EnteredClient {
                issuer: AS.to_string(),
                client_id: id.to_string(),
                client_secret: secret.map(str::to_string),
                keep_secret,
                callback_port: port,
            };
        let cli = StoredClient {
            client_secret: Some("kept".to_string()),
            ..Default::default()
        };
        let typed = StoredClient {
            client_id: Some("typed".to_string()),
            client_secret: Some("typed-secret".to_string()),
            callback_port: Some(9001),
            issuer: Some(AS.to_string()),
            secret_from_entry: false,
        };
        // The entry's own client, a secret typed: kept as its secret.
        assert_eq!(
            client_to_keep(
                &form("file-cid", Some("new"), false, Some(8000)),
                Some(&typed),
                Some(&entry)
            ),
            Some(StoredClient {
                client_secret: Some("new".to_string()),
                issuer: Some(AS.to_string()),
                ..Default::default()
            })
        );
        // None typed: what is kept for it stays, an entered client goes.
        let own = form("file-cid", None, true, Some(8000));
        assert_eq!(
            client_to_keep(&own, Some(&cli), Some(&entry)),
            Some(cli.clone())
        );
        assert_eq!(client_to_keep(&own, Some(&typed), Some(&entry)), None);
        assert_eq!(client_to_keep(&own, None, Some(&entry)), None);
        // An entry that takes any free port: an empty port field is its.
        let any_port = crate::mcp::OAuthClientConfig {
            callback_port: None,
            ..entry.clone()
        };
        let own = form("file-cid", None, true, None);
        assert_eq!(client_to_keep(&own, Some(&typed), Some(&any_port)), None);
        // Another client, whole, with what was typed.
        assert_eq!(
            client_to_keep(
                &form("other", Some("s"), false, Some(9001)),
                Some(&cli),
                Some(&entry)
            ),
            Some(StoredClient {
                client_id: Some("other".to_string()),
                client_secret: Some("s".to_string()),
                callback_port: Some(9001),
                issuer: Some(AS.to_string()),
                secret_from_entry: false,
            })
        );
        // None typed and none kept: a public client.
        let public = client_to_keep(&form("other", None, false, Some(9001)), Some(&typed), None);
        assert_eq!(
            public.map(|c| (c.client_secret, c.secret_from_entry)),
            Some((None, false))
        );
        // Left on the same client's kept secret: it stays.
        let same = client_to_keep(&form("typed", None, true, Some(9002)), Some(&typed), None);
        assert_eq!(
            same.map(|c| (c.client_secret, c.callback_port)),
            Some((Some("typed-secret".to_string()), Some(9002)))
        );
        // Never another client's, nor one kept for another authorization
        // server.
        let other = client_to_keep(&form("other", None, true, Some(9001)), Some(&typed), None);
        assert_eq!(other.and_then(|c| c.client_secret), None);
        let elsewhere = StoredClient {
            issuer: Some("https://other.test".to_string()),
            ..typed.clone()
        };
        let moved = client_to_keep(
            &form("typed", None, true, Some(9001)),
            Some(&elsewhere),
            None,
        );
        assert_eq!(moved.and_then(|c| c.client_secret), None);
        // The entry's id on another port, left on the secret in effect: the
        // one kept for the entry's client…
        let on_another_port = form("file-cid", None, true, Some(9000));
        let kept_one = client_to_keep(&on_another_port, Some(&cli), Some(&entry)).unwrap();
        assert_eq!(
            (
                kept_one.client_id.as_deref(),
                kept_one.client_secret.as_deref(),
                kept_one.secret_from_entry,
                kept_one.callback_port
            ),
            (Some("file-cid"), Some("kept"), false, Some(9000))
        );
        // …else the file's, never copied, and kept on saving it again.
        let lent = client_to_keep(&on_another_port, None, Some(&entry)).unwrap();
        assert_eq!(
            (lent.client_secret.clone(), lent.secret_from_entry),
            (None, true)
        );
        let again = client_to_keep(&on_another_port, Some(&lent), Some(&entry)).unwrap();
        assert_eq!((again.client_secret, again.secret_from_entry), (None, true));
        // Not left on it, a public client, whatever the file holds.
        let public = client_to_keep(
            &form("file-cid", None, false, Some(9000)),
            None,
            Some(&entry),
        );
        assert_eq!(
            public.map(|c| (c.client_secret, c.secret_from_entry)),
            Some((None, false))
        );
    }

    /// The form a server's own page opens, once its authorization server is
    /// known: under that server's issuer and documentation link, the client
    /// in effect, to change — one entered here for that server, else the
    /// entry's own, whole, on its exact port — or a new client, on the port
    /// the next sign-in listens on. Every form knows the entry's own client,
    /// to tell from what is typed whether it still describes it.
    #[test]
    fn the_client_on_hand_fills_the_form_the_server_page_opens() {
        let meta = AuthServerMeta {
            documentation: Some("https://as.test/docs".to_string()),
            ..meta_for("https://as.test", false)
        };
        let entered = StoredClient {
            client_id: Some("typed".to_string()),
            client_secret: Some("s".to_string()),
            callback_port: Some(9001),
            issuer: Some("https://as.test".to_string()),
            secret_from_entry: false,
        };
        assert_eq!(
            client_request_for(None, Some(&entered), &meta),
            ClientRequest {
                issuer: "https://as.test".to_string(),
                client_id: Some("typed".to_string()),
                fixed_id: false,
                has_secret: true,
                callback_port: Some(9001),
                redirect_uri: None,
                documentation: Some("https://as.test/docs".to_string()),
                reason: None,
                entry: None,
            }
        );
        // The entry's own client, whole: its id and its exact port.
        let configured = crate::mcp::OAuthClientConfig {
            client_id: Some("file-cid".to_string()),
            callback_port: Some(8080),
            ..Default::default()
        };
        let request = client_request_for(Some(&configured), None, &meta);
        assert_eq!(request.client_id.as_deref(), Some("file-cid"));
        assert!(!request.fixed_id && !request.has_secret);
        assert_eq!(request.callback_port, Some(8080));
        assert_eq!(request.issuer, "https://as.test");
        assert_eq!(
            request.entry,
            Some(crate::mcp::McpEntryClient {
                client_id: "file-cid".to_string(),
                port: Some(8080),
                redirect_uri: Some("http://localhost:8080/callback".to_string()),
                has_secret: false,
            })
        );
        // A secret kept for it, or its file's, is in effect; one kept for
        // another authorization server is not.
        let cli = StoredClient {
            client_secret: Some("kept".to_string()),
            ..Default::default()
        };
        assert!(client_request_for(Some(&configured), Some(&cli), &meta).has_secret);
        let in_file = crate::mcp::OAuthClientConfig {
            client_secret: Some("in the file".to_string()),
            ..configured.clone()
        };
        let request = client_request_for(Some(&in_file), None, &meta);
        assert!(request.has_secret && request.entry.is_some_and(|entry| entry.has_secret));
        let cli_elsewhere = StoredClient {
            issuer: Some("https://other.test".to_string()),
            ..cli.clone()
        };
        assert!(!client_request_for(Some(&configured), Some(&cli_elsewhere), &meta).has_secret);
        // One entered for this server outranks it, and the form still knows
        // the entry's; one entered for another is not in effect here.
        let request = client_request_for(Some(&configured), Some(&entered), &meta);
        assert_eq!(request.client_id.as_deref(), Some("typed"));
        assert!(request.entry.is_some());
        let elsewhere = StoredClient {
            issuer: Some("https://other.test".to_string()),
            ..entered.clone()
        };
        let request = client_request_for(Some(&configured), Some(&elsewhere), &meta);
        assert_eq!(request.client_id.as_deref(), Some("file-cid"));
        // An entry that takes any free port: no port, no address to show.
        let any_port = crate::mcp::OAuthClientConfig {
            client_id: Some("file-cid".to_string()),
            ..Default::default()
        };
        let request = client_request_for(Some(&any_port), None, &meta);
        assert_eq!(request.callback_port, None);
        assert_eq!(request.entry.and_then(|entry| entry.redirect_uri), None);
        // No client anywhere: a new one, on the port the next sign-in
        // listens on — the entry's, where it names one.
        let new = client_request_for(None, None, &meta);
        assert_eq!(new.client_id, None);
        assert!(!new.fixed_id && !new.has_secret);
        assert_eq!(new.callback_port, Some(DEFAULT_CALLBACK_PORT));
        assert_eq!(new.documentation.as_deref(), Some("https://as.test/docs"));
        assert_eq!(client_request_for(None, Some(&cli), &meta).client_id, None);
        let port_only = crate::mcp::OAuthClientConfig {
            callback_port: Some(8080),
            ..Default::default()
        };
        let request = client_request_for(Some(&port_only), None, &meta);
        assert_eq!(request.client_id, None);
        assert_eq!(request.callback_port, Some(8080));
    }

    /// The lookup Set and Change OAuth client run before the form opens:
    /// discovery alone — nothing registered, no URL, no token requested —
    /// and the form's request under the issuer it found and the
    /// documentation link that server's metadata gives, over the client on
    /// hand. A lookup called off says so.
    #[test]
    fn a_client_lookup_names_the_issuer_and_registers_nothing() {
        use std::sync::atomic::Ordering;
        let fixture = spawn_auth_server(None);
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("mcp-auth.json");
        let server = format!("{}/mcp", fixture.base);
        let target = SignIn {
            server_url: &server,
            challenge: None,
            store_path: Some(&store),
            client: None,
        };
        let request = client_lookup(&target, &CancelToken::new()).expect("looked up");
        assert_eq!(request.issuer, fixture.base);
        assert_eq!(
            request.documentation,
            Some(format!("{}/docs/oauth", fixture.base))
        );
        assert_eq!(request.client_id, None);
        assert_eq!(fixture.registrations.load(Ordering::SeqCst), 0);
        assert!(fixture.token_requests.lock().unwrap().is_empty());
        // Over a client entered for the server: its values.
        save_client(Some(&store), &server, Some(&entered(&fixture, 9001))).unwrap();
        let request = client_lookup(&target, &CancelToken::new()).expect("looked up");
        assert_eq!(request.client_id.as_deref(), Some("typed-cid"));
        assert_eq!(request.callback_port, Some(9001));
        assert!(request.has_secret);
        let cancel = CancelToken::new();
        cancel.cancel();
        assert!(client_lookup(&target, &cancel).is_err());
    }

    /// What `mcp get` says of the client a server signs in with: the
    /// entry's own and where its secret is, or the one entered in `/mcp`
    /// with its port — and never a secret itself.
    #[test]
    fn the_client_a_server_signs_in_with_is_summed_up_without_its_secret() {
        let entered = StoredClient {
            client_id: Some("typed-cid".to_string()),
            client_secret: Some("typed-secret".to_string()),
            callback_port: Some(9001),
            issuer: Some("https://as.test".to_string()),
            secret_from_entry: false,
        };
        assert_eq!(
            client_summary(None, Some(&entered)),
            [
                "OAuth client: typed-cid (entered in /mcp)",
                "Callback port: 9001",
                "Client secret: kept with the tokens",
            ]
        );
        // A public client entered here: no secret line; the port falls back
        // to the one every entered client signs in on.
        let public = StoredClient {
            client_secret: None,
            callback_port: None,
            ..entered.clone()
        };
        assert_eq!(
            client_summary(None, Some(&public)),
            [
                "OAuth client: typed-cid (entered in /mcp)".to_string(),
                format!("Callback port: {DEFAULT_CALLBACK_PORT}"),
            ]
        );
        // The entry's own client, as before — its secret in the file, kept
        // with the tokens, or nowhere.
        let configured = crate::mcp::OAuthClientConfig {
            client_id: Some("file-cid".to_string()),
            callback_port: Some(8976),
            scopes: Some("calendar openid".to_string()),
            ..Default::default()
        };
        let cli = StoredClient {
            client_secret: Some("kept".to_string()),
            ..Default::default()
        };
        assert_eq!(
            client_summary(Some(&configured), Some(&cli)),
            [
                "OAuth client: file-cid",
                "Callback port: 8976",
                "Scopes: calendar openid",
                "Client secret: kept with the tokens",
            ]
        );
        // Another client's secret is not this one's.
        assert_eq!(
            client_summary(Some(&configured), Some(&entered)),
            [
                "OAuth client: file-cid",
                "Callback port: 8976",
                "Scopes: calendar openid",
            ]
        );
        let in_file = crate::mcp::OAuthClientConfig {
            client_secret: Some("in-the-file".to_string()),
            ..configured.clone()
        };
        assert_eq!(
            client_summary(Some(&in_file), None)
                .last()
                .map(String::as_str),
            Some("Client secret: ***** (in the file)")
        );
        // An entry with scopes alone still shows them beside an entered client.
        let scopes_only = crate::mcp::OAuthClientConfig {
            scopes: Some("read".to_string()),
            ..Default::default()
        };
        assert_eq!(
            client_summary(Some(&scopes_only), Some(&entered)),
            [
                "OAuth client: typed-cid (entered in /mcp)",
                "Callback port: 9001",
                "Scopes: read",
                "Client secret: kept with the tokens",
            ]
        );
        assert!(client_summary(None, None).is_empty());
        assert!(client_summary(None, Some(&cli)).is_empty());
        for lines in [
            client_summary(None, Some(&entered)),
            client_summary(Some(&in_file), Some(&cli)),
        ] {
            let all = lines.join("\n");
            assert!(
                !all.contains("typed-secret")
                    && !all.contains("in-the-file")
                    && !all.contains("kept\n"),
                "{all}"
            );
        }
    }

    /// A token endpoint's refusal is a verdict on the client when it says
    /// so — `invalid_client`, `unauthorized_client` — or names the secret
    /// (Google's `invalid_request`: "client_secret is missing."); a refused
    /// code or grant is about the login.
    #[test]
    fn a_refusal_of_the_client_is_told_from_a_refusal_of_the_login() {
        assert!(client_refused(r#"{"error":"invalid_client"}"#));
        assert!(client_refused(
            r#"{"error":"unauthorized_client","error_description":"not allowed this grant"}"#
        ));
        assert!(client_refused(
            r#"{"error":"invalid_request","error_description":"client_secret is missing."}"#
        ));
        assert!(client_refused(
            r#"{"error":"invalid_request","error_description":"Missing Client Secret"}"#
        ));
        assert!(!client_refused(r#"{"error":"invalid_grant"}"#));
        assert!(!client_refused(
            r#"{"error":"invalid_request","error_description":"redirect_uri mismatch"}"#
        ));
        assert!(!client_refused("<html>502</html>"));
        assert!(!client_refused(""));
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
            documentation: None,
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
    /// paste — and, with `approve` set, the user who opens that URL by hand
    /// and approves with that code (the flow itself launches nothing).
    #[derive(Default)]
    struct Recorded {
        urls: std::sync::Mutex<Vec<String>>,
        rejected: std::sync::Mutex<Vec<String>>,
        approve: Option<&'static str>,
    }

    impl Recorded {
        fn approving(code: &'static str) -> Self {
            Self {
                approve: Some(code),
                ..Self::default()
            }
        }
    }

    impl AuthProgress for std::sync::Arc<Recorded> {
        fn on_url(&self, url: &str) {
            self.urls.lock().unwrap().push(url.to_string());
            if let Some(code) = self.approve {
                approve(url, code);
            }
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

    /// Each request an authorization-server fixture saw: its path beside the
    /// `User-Agent` it carried.
    type SeenAgents = std::sync::Arc<std::sync::Mutex<Vec<(String, Option<String>)>>>;

    /// One fake authorization server: resource and AS metadata for
    /// `{base}/mcp`, dynamic registration (counted, its reply overridable),
    /// and a token endpoint recording each request's headers and body.
    struct AuthServerFixture {
        base: String,
        registrations: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        token_requests: std::sync::Arc<std::sync::Mutex<Vec<(String, String)>>>,
        /// Every request's path beside the `User-Agent` it carried.
        agents: SeenAgents,
        /// Each registration request's body.
        registered: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
        /// What the token endpoint answers instead of a grant, once set —
        /// a refused client, a refused code.
        token_reply: std::sync::Arc<std::sync::Mutex<Option<(u16, &'static str)>>>,
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
        spawn_auth_server_behind(false, registration, register_reply)
    }

    /// [`spawn_auth_server_with`], behind a gateway when `gated`: one that
    /// turns away any request naming no client — no `User-Agent`, a
    /// firewall's stock rule — with a 403 web page, whatever it asked for.
    fn spawn_auth_server_behind(
        gated: bool,
        registration: bool,
        register_reply: Option<(u16, &'static str)>,
    ) -> AuthServerFixture {
        use std::io::{BufRead, BufReader};
        use std::sync::atomic::{AtomicUsize, Ordering};
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let registrations = std::sync::Arc::new(AtomicUsize::new(0));
        let token_requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let agents = SeenAgents::default();
        let registered = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let token_reply = std::sync::Arc::new(std::sync::Mutex::new(None));
        let fixture = AuthServerFixture {
            base: base.clone(),
            registrations: std::sync::Arc::clone(&registrations),
            token_requests: std::sync::Arc::clone(&token_requests),
            agents: std::sync::Arc::clone(&agents),
            registered: std::sync::Arc::clone(&registered),
            token_reply: std::sync::Arc::clone(&token_reply),
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
                let agent = head.lines().find_map(|header| {
                    let (name, value) = header.split_once(':')?;
                    name.eq_ignore_ascii_case("user-agent")
                        .then(|| value.trim().to_string())
                });
                let named = agent.is_some();
                agents.lock().unwrap().push((path.clone(), agent));
                if gated && !named {
                    let page = "<html>\r\n<head><title>403 Forbidden</title></head>\r\n\
                        <body>\r\n<center><h1>403 Forbidden</h1></center>\r\n</body>\r\n</html>\r\n";
                    let _ = stream.write_all(
                        format!(
                            "HTTP/1.1 403 Forbidden\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{page}",
                            page.len()
                        )
                        .as_bytes(),
                    );
                    continue;
                }
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
                            r#"{{"issuer":"{base}","authorization_endpoint":"{base}/authorize","token_endpoint":"{base}/token",{registration}"code_challenge_methods_supported":["S256"],"service_documentation":"{base}/docs/oauth"}}"#,
                            registration = if registration {
                                format!(r#""registration_endpoint":"{base}/register","#)
                            } else {
                                String::new()
                            }
                        ),
                    ),
                    "/register" => {
                        registered.lock().unwrap().push(body.clone());
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
                        match *token_reply.lock().unwrap() {
                            Some((status, reply)) => (status, reply.to_string()),
                            None => (
                                200,
                                r#"{"access_token":"at","refresh_token":"rt","expires_in":3600}"#
                                    .to_string(),
                            ),
                        }
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

    /// A user who approves at once: they follow the authorize URL's
    /// redirect back to the callback with `code`, the URL's own state, and
    /// no `iss` (this fixture's metadata doesn't advertise it).
    fn approve(authorize_url: &str, code: &'static str) {
        let redirect = redirect_param(authorize_url, "redirect_uri").expect("redirect_uri");
        let state = redirect_param(authorize_url, "state").expect("state");
        let redirect = url::Url::parse(&redirect).unwrap();
        let port = redirect.port().expect("a port");
        let path = redirect.path().to_string();
        std::thread::spawn(move || {
            let _ = hit(
                port,
                &format!(
                    "GET {path}?code={code}&state={} HTTP/1.1\r\nHost: localhost\r\n\r\n",
                    url_encode(&state)
                ),
            );
        });
    }

    fn sign_in(
        fixture: &AuthServerFixture,
        store: &Path,
        code: &'static str,
        cancel: &CancelToken,
    ) -> (Result<(), String>, std::sync::Arc<Recorded>) {
        sign_in_with(fixture, store, code, cancel, None)
    }

    fn sign_in_with(
        fixture: &AuthServerFixture,
        store: &Path,
        code: &'static str,
        cancel: &CancelToken,
        client: Option<&crate::mcp::OAuthClientConfig>,
    ) -> (Result<(), String>, std::sync::Arc<Recorded>) {
        let (result, progress) = sign_in_outcome(fixture, store, code, cancel, client);
        (result.map_err(|end| end.to_string()), progress)
    }

    /// [`sign_in_with`], keeping how the flow ended: asking for a client,
    /// or failed.
    fn sign_in_outcome(
        fixture: &AuthServerFixture,
        store: &Path,
        code: &'static str,
        cancel: &CancelToken,
        client: Option<&crate::mcp::OAuthClientConfig>,
    ) -> (Result<(), AuthError>, std::sync::Arc<Recorded>) {
        let (_paste_tx, paste_rx) = mpsc::channel();
        let progress = std::sync::Arc::new(Recorded::approving(code));
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
            callback_url: None,
            scopes: Some("calendar openid".to_string()),
        };
        let (result, progress) =
            sign_in_with(&fixture, &store, "c", &CancelToken::new(), Some(&client));
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
        let (result, progress) =
            sign_in_with(&fixture, &store, "c", &CancelToken::new(), Some(&client));
        result.expect("signed in");
        assert_eq!(fixture.registrations.load(Ordering::SeqCst), 1);
        assert_eq!(port_of(&progress.urls.lock().unwrap()[0]), port);
    }

    /// Without one, such a server's sign-in asks for one — the spec's last
    /// registration step — naming who issues it and the port the next
    /// attempt listens on; and where no page can ask, its words say both
    /// ways in.
    #[test]
    fn no_registration_and_no_client_asks_for_one() {
        let fixture = spawn_auth_server_with(false, None);
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("mcp-auth.json");
        let (result, progress) = sign_in_outcome(&fixture, &store, "c", &CancelToken::new(), None);
        let Err(AuthError::NeedsClient(request)) = result else {
            panic!("expected a request for a client, got {result:?}");
        };
        assert_eq!(
            *request,
            ClientRequest {
                issuer: fixture.base.clone(),
                client_id: None,
                fixed_id: false,
                has_secret: false,
                callback_port: Some(DEFAULT_CALLBACK_PORT),
                redirect_uri: None,
                documentation: Some(format!("{}/docs/oauth", fixture.base)),
                reason: None,
                entry: None,
            }
        );
        assert!(progress.urls.lock().unwrap().is_empty(), "no URL to show");
        let words = AuthError::NeedsClient(request).to_string();
        assert!(
            words.contains("/mcp")
                && words.contains("oauth.clientId")
                && words.contains("--client-id"),
            "{words}"
        );
    }

    /// An entry whose `oauth` block names no client — a template's unfilled
    /// slots dropped, a port and scopes left — asks for one on its port,
    /// and whichever client then signs in does so on that port and asks
    /// for the entry's scopes, at a registration too.
    #[test]
    fn an_entry_naming_no_client_keeps_its_port_and_scopes() {
        use std::sync::atomic::Ordering;
        let fixture = spawn_auth_server_with(false, None);
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("mcp-auth.json");
        let server = format!("{}/mcp", fixture.base);
        let port = free_port();
        let config = crate::mcp::OAuthClientConfig {
            callback_port: Some(port),
            scopes: Some("mail.read mail.send".to_string()),
            ..Default::default()
        };
        let (result, _) =
            sign_in_outcome(&fixture, &store, "c", &CancelToken::new(), Some(&config));
        let Err(AuthError::NeedsClient(request)) = result else {
            panic!("expected a request for a client, got {result:?}");
        };
        assert_eq!(request.callback_port, Some(port));
        assert!(!request.fixed_id && request.client_id.is_none());
        assert_eq!(request.redirect_uri, None);
        // The form, prefilled with that port, keeps the client entered.
        let client = StoredClient {
            client_id: Some("typed-cid".to_string()),
            client_secret: None,
            callback_port: request.callback_port,
            issuer: Some(request.issuer.clone()),
            secret_from_entry: false,
        };
        save_client(Some(&store), &server, Some(&client)).unwrap();
        let (result, progress) =
            sign_in_outcome(&fixture, &store, "c", &CancelToken::new(), Some(&config));
        result.expect("signed in");
        let url = progress.urls.lock().unwrap()[0].clone();
        assert_eq!(port_of(&url), port);
        assert_eq!(
            redirect_param(&url, "redirect_uri").as_deref(),
            Some(crate::mcp::oauth_redirect_uri(port).as_str())
        );
        assert_eq!(
            redirect_param(&url, "client_id").as_deref(),
            Some("typed-cid")
        );
        assert_eq!(
            redirect_param(&url, "scope").as_deref(),
            Some("mail.read mail.send")
        );
        // A server that registers clients is asked for the same scopes.
        let registering = spawn_auth_server(None);
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("mcp-auth.json");
        let port = free_port();
        let config = crate::mcp::OAuthClientConfig {
            callback_port: Some(port),
            ..config
        };
        let (result, progress) = sign_in_with(
            &registering,
            &store,
            "c",
            &CancelToken::new(),
            Some(&config),
        );
        result.expect("signed in");
        assert_eq!(registering.registrations.load(Ordering::SeqCst), 1);
        let registered = registering.registered.lock().unwrap()[0].clone();
        assert!(registered.contains("mail.read mail.send"), "{registered}");
        let url = progress.urls.lock().unwrap()[0].clone();
        assert_eq!(port_of(&url), port);
        assert_eq!(
            redirect_param(&url, "scope").as_deref(),
            Some("mail.read mail.send")
        );
    }

    /// The entry's own client sends the redirect URI its entry names
    /// (`callbackUrl`) — on the authorize URL, at the token exchange, and
    /// kept with the grant — listening on its port and answering on its
    /// path.
    #[test]
    fn an_entrys_callback_url_is_the_redirect_uri_its_client_sends() {
        let fixture = spawn_auth_server_with(false, None);
        let port = free_port();
        let cases = [
            (
                Some(format!("http://127.0.0.1:{port}/callback/made-up-id")),
                None,
            ),
            (
                Some("http://localhost/cb/made-up-id".to_string()),
                Some(port),
            ),
            (Some("http://localhost/cb/made-up-id".to_string()), None),
        ];
        for (callback_url, callback_port) in cases {
            let dir = tempfile::tempdir().unwrap();
            let store = dir.path().join("mcp-auth.json");
            let config = crate::mcp::OAuthClientConfig {
                client_id: Some("pre-cid".to_string()),
                client_secret: Some("pre-secret".to_string()),
                callback_port,
                callback_url: callback_url.clone(),
                scopes: None,
            };
            let (result, progress) =
                sign_in_with(&fixture, &store, "c", &CancelToken::new(), Some(&config));
            result.unwrap_or_else(|e| panic!("{callback_url:?}: {e}"));
            let url = progress.urls.lock().unwrap()[0].clone();
            let sent = redirect_param(&url, "redirect_uri").unwrap();
            let bound = port_of(&url);
            if callback_port.is_some() || callback_url.as_deref().unwrap().contains("127.0.0.1") {
                assert_eq!(bound, port, "{callback_url:?}");
            }
            let expected = config.redirect_uri_on(bound);
            assert_eq!(sent, expected);
            assert!(sent.ends_with("/made-up-id") && sent.contains(&format!(":{bound}/")));
            let requests = fixture.token_requests.lock().unwrap();
            let (_, body) = requests.last().unwrap();
            assert!(
                body.contains(&format!("redirect_uri={}", url_encode(&expected))),
                "{body}"
            );
            drop(requests);
            let stored = load_tokens(Some(&store), &format!("{}/mcp", fixture.base)).unwrap();
            assert_eq!(stored.redirect_uri.as_deref(), Some(expected.as_str()));
        }
    }

    /// A port the entry names that is taken is said as the entry's, naming
    /// no key — it may have come from `callbackUrl` as well as
    /// `callbackPort`.
    #[test]
    fn a_taken_entry_port_is_named_without_a_key() {
        let holder = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = holder.local_addr().unwrap().port();
        let Err(error) = bind_callback(Some(port), &[0], "/callback") else {
            panic!("the port is taken");
        };
        assert!(
            error.starts_with(&format!(
                "the callback port {port} this server's entry names isn't free: "
            )),
            "{error}"
        );
        assert!(!error.contains("oauth."), "{error}");
    }

    /// A redirect URI registered at the root is a real one: a sign-in whose
    /// entry names a root-path `callbackUrl` is answered on `/`, where
    /// `/callback` is then a stray.
    #[test]
    fn a_root_path_callback_url_is_answered_on_the_root() {
        let port = free_port();
        let config = crate::mcp::OAuthClientConfig {
            client_id: Some("pre-cid".to_string()),
            client_secret: Some("pre-secret".to_string()),
            callback_url: Some(format!("http://127.0.0.1:{port}/")),
            ..Default::default()
        };
        assert_eq!(config.callback_path(), "/");
        let callback =
            bind_callback(config.exact_port(), &[0], &config.callback_path()).expect("bind");
        assert_eq!(callback.port, port);
        let (done_tx, done_rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = done_tx.send(loopback::wait_for_code(
                &callback,
                &Expected {
                    state: "s1",
                    issuer: None,
                    iss_required: false,
                },
                &MCP_PAGES,
                Duration::from_secs(10),
                None,
                &CancelToken::new(),
            ));
        });
        let stray = hit(port, "GET /callback?code=c1&state=s1 HTTP/1.1\r\n\r\n");
        assert!(stray.starts_with("HTTP/1.1 404"), "{stray}");
        let real = hit(port, "GET /?code=c1&state=s1 HTTP/1.1\r\n\r\n");
        assert!(real.starts_with("HTTP/1.1 200"), "{real}");
        assert_eq!(
            done_rx.recv_timeout(Duration::from_secs(3)).unwrap().ok(),
            Some("c1".to_string())
        );
        // And a whole sign-in through it lands.
        let fixture = spawn_auth_server_with(false, None);
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("mcp-auth.json");
        let (result, progress) =
            sign_in_with(&fixture, &store, "c", &CancelToken::new(), Some(&config));
        result.expect("signed in");
        let url = progress.urls.lock().unwrap()[0].clone();
        assert_eq!(
            redirect_param(&url, "redirect_uri").as_deref(),
            Some(format!("http://127.0.0.1:{port}/").as_str())
        );
    }

    /// A `callbackUrl` is the entry's own client's: a client entered in
    /// `/mcp` keeps the standard redirect URI on its own port.
    #[test]
    fn a_callback_url_is_never_sent_for_an_entered_client() {
        let fixture = spawn_auth_server_with(false, None);
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("mcp-auth.json");
        let server = format!("{}/mcp", fixture.base);
        let port = free_port();
        save_client(Some(&store), &server, Some(&entered(&fixture, port))).unwrap();
        let config = crate::mcp::OAuthClientConfig {
            callback_port: Some(port),
            callback_url: Some(format!("http://127.0.0.1:{port}/elsewhere")),
            ..Default::default()
        };
        let (result, progress) =
            sign_in_outcome(&fixture, &store, "c", &CancelToken::new(), Some(&config));
        result.expect("signed in");
        let url = progress.urls.lock().unwrap()[0].clone();
        assert_eq!(
            redirect_param(&url, "redirect_uri").as_deref(),
            Some(crate::mcp::oauth_redirect_uri(port).as_str())
        );
    }

    /// The redirect URI the entry's own client sends is the one its entry
    /// names, wherever it is shown: the form the server's page opens, the
    /// request for its secret after a refusal, and `mcp get`.
    #[test]
    fn the_entrys_redirect_uri_is_the_one_shown() {
        let configured = crate::mcp::OAuthClientConfig {
            client_id: Some("file-cid".to_string()),
            callback_url: Some("http://127.0.0.1:5555/callback/file-cid".to_string()),
            ..Default::default()
        };
        let meta = meta_for("https://as.test", false);
        let request = client_request_for(Some(&configured), None, &meta);
        assert_eq!(request.callback_port, Some(5555));
        assert_eq!(
            request
                .entry
                .and_then(|entry| entry.redirect_uri)
                .as_deref(),
            Some("http://127.0.0.1:5555/callback/file-cid")
        );
        assert!(
            client_summary(Some(&configured), None)
                .contains(&"Redirect URI: http://127.0.0.1:5555/callback/file-cid".to_string()),
            "{:?}",
            client_summary(Some(&configured), None)
        );
        // An entry naming no URL sends the standard one on its port.
        let plain = crate::mcp::OAuthClientConfig {
            callback_url: None,
            callback_port: Some(8080),
            ..configured.clone()
        };
        assert_eq!(
            client_request_for(Some(&plain), None, &meta)
                .entry
                .and_then(|entry| entry.redirect_uri)
                .as_deref(),
            Some("http://localhost:8080/callback")
        );
        // Turned away for want of a secret, the request says it too.
        let fixture = spawn_auth_server_with(false, None);
        *fixture.token_reply.lock().unwrap() = Some((
            400,
            r#"{"error":"invalid_request","error_description":"client_secret is missing."}"#,
        ));
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("mcp-auth.json");
        let port = free_port();
        let url = format!("http://127.0.0.1:{port}/callback/file-cid");
        let configured = crate::mcp::OAuthClientConfig {
            callback_url: Some(url.clone()),
            ..configured
        };
        let (result, _) = sign_in_outcome(
            &fixture,
            &store,
            "c",
            &CancelToken::new(),
            Some(&configured),
        );
        let Err(AuthError::NeedsClient(request)) = result else {
            panic!("expected a request for the secret, got {result:?}");
        };
        assert!(request.fixed_id);
        assert_eq!(request.callback_port, Some(port));
        assert_eq!(request.redirect_uri.as_deref(), Some(url.as_str()));
    }

    fn entered(fixture: &AuthServerFixture, port: u16) -> StoredClient {
        StoredClient {
            client_id: Some("typed-cid".to_string()),
            client_secret: Some("typed-secret".to_string()),
            callback_port: Some(port),
            issuer: Some(fixture.base.clone()),
            secret_from_entry: false,
        }
    }

    /// The client entered on the `/mcp` auth page signs in: its id, its
    /// secret, the redirect on exactly its port, never a registration — and
    /// it stays kept, apart from the login it made.
    #[test]
    fn an_entered_client_signs_in_on_its_own_port() {
        use std::sync::atomic::Ordering;
        let fixture = spawn_auth_server_with(false, None);
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("mcp-auth.json");
        let server = format!("{}/mcp", fixture.base);
        let port = free_port();
        let client = entered(&fixture, port);
        save_client(Some(&store), &server, Some(&client)).unwrap();
        let (result, progress) = sign_in_outcome(&fixture, &store, "c", &CancelToken::new(), None);
        result.expect("signed in");
        assert_eq!(fixture.registrations.load(Ordering::SeqCst), 0);
        let url = progress.urls.lock().unwrap()[0].clone();
        assert_eq!(port_of(&url), port);
        assert_eq!(
            redirect_param(&url, "redirect_uri").as_deref(),
            Some(crate::mcp::oauth_redirect_uri(port).as_str())
        );
        assert_eq!(
            redirect_param(&url, "client_id").as_deref(),
            Some("typed-cid")
        );
        let requests = fixture.token_requests.lock().unwrap();
        let (_, body) = &requests[0];
        assert!(
            body.contains("client_id=typed-cid") && body.contains("client_secret=typed-secret"),
            "{body}"
        );
        assert_eq!(load_client(Some(&store), &server), Some(client));
        let grant = load_tokens(Some(&store), &server).unwrap();
        assert_eq!(grant.client_id, "typed-cid");
    }

    /// An entered client wins over a registration the server would make —
    /// the user's own client is the one they asked to sign in with.
    #[test]
    fn an_entered_client_outranks_a_registration() {
        use std::sync::atomic::Ordering;
        let fixture = spawn_auth_server(None);
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("mcp-auth.json");
        let server = format!("{}/mcp", fixture.base);
        save_client(Some(&store), &server, Some(&entered(&fixture, free_port()))).unwrap();
        let (result, progress) = sign_in_outcome(&fixture, &store, "c", &CancelToken::new(), None);
        result.expect("signed in");
        assert_eq!(fixture.registrations.load(Ordering::SeqCst), 0);
        let url = progress.urls.lock().unwrap()[0].clone();
        assert_eq!(
            redirect_param(&url, "client_id").as_deref(),
            Some("typed-cid")
        );
    }

    /// A kept client is presented only to the authorization server it was
    /// created with (SEP-2352): when the server signs in through another,
    /// the flow asks again and says why, the old id and secret unsent.
    #[test]
    fn an_entered_client_is_never_presented_to_another_authorization_server() {
        let fixture = spawn_auth_server_with(false, None);
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("mcp-auth.json");
        let server = format!("{}/mcp", fixture.base);
        let foreign = StoredClient {
            issuer: Some("https://other.test".to_string()),
            ..entered(&fixture, free_port())
        };
        save_client(Some(&store), &server, Some(&foreign)).unwrap();
        let (result, progress) = sign_in_outcome(&fixture, &store, "c", &CancelToken::new(), None);
        let Err(AuthError::NeedsClient(request)) = result else {
            panic!("expected a request for a client, got {result:?}");
        };
        assert_eq!(request.client_id, None);
        assert!(!request.has_secret);
        let reason = request.reason.expect("says why");
        assert!(reason.contains("other.test"), "{reason}");
        assert!(progress.urls.lock().unwrap().is_empty());
        assert!(fixture.token_requests.lock().unwrap().is_empty());
        // Where the new authorization server registers clients itself, the
        // sign-in just goes on with a registration.
        let registering = spawn_auth_server(None);
        let server = format!("{}/mcp", registering.base);
        save_client(Some(&store), &server, Some(&foreign)).unwrap();
        let (result, _) = sign_in_outcome(&registering, &store, "c", &CancelToken::new(), None);
        result.expect("signed in");
        let requests = registering.token_requests.lock().unwrap();
        assert!(
            requests[0].1.contains("client_id=cid-1"),
            "{}",
            requests[0].1
        );
        assert!(!requests[0].1.contains("typed"), "{}", requests[0].1);
    }

    /// An entered client the authorization server turns away — a mistyped
    /// id or secret — reopens the form with what was entered and the
    /// server's own reason, instead of ending on an error the user can only
    /// read.
    #[test]
    fn an_entered_client_the_server_turns_away_is_asked_for_again() {
        let fixture = spawn_auth_server_with(false, None);
        *fixture.token_reply.lock().unwrap() = Some((
            401,
            r#"{"error":"invalid_client","error_description":"The OAuth client was not found."}"#,
        ));
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("mcp-auth.json");
        let server = format!("{}/mcp", fixture.base);
        let port = free_port();
        save_client(Some(&store), &server, Some(&entered(&fixture, port))).unwrap();
        let (result, progress) = sign_in_outcome(&fixture, &store, "c", &CancelToken::new(), None);
        let Err(AuthError::NeedsClient(request)) = result else {
            panic!("expected a request for a client, got {result:?}");
        };
        assert_eq!(request.client_id.as_deref(), Some("typed-cid"));
        assert!(!request.fixed_id);
        assert!(request.has_secret);
        assert_eq!(request.callback_port, Some(port));
        let reason = request.reason.expect("the server's reason");
        assert!(
            reason.contains("invalid_client") && reason.contains("The OAuth client was not found."),
            "{reason}"
        );
        assert!(!reason.contains("typed-secret"), "{reason}");
        assert_eq!(progress.urls.lock().unwrap().len(), 1);
        assert!(load_tokens(Some(&store), &server).is_none());
        // A refusal of the *login* — a bad code — is no verdict on the client.
        *fixture.token_reply.lock().unwrap() = Some((
            400,
            r#"{"error":"invalid_grant","error_description":"bad code"}"#,
        ));
        let (result, _) = sign_in_outcome(&fixture, &store, "c", &CancelToken::new(), None);
        let Err(AuthError::Failed(detail)) = result else {
            panic!("expected a failure, got {result:?}");
        };
        assert!(
            detail.contains("token exchange failed") && detail.contains("invalid_grant"),
            "{detail}"
        );
    }

    /// An entered client's port is the one its redirect URI was registered
    /// with: taken, the sign-in asks again with the port named — the form's
    /// own field is where it changes — and shows no URL.
    #[test]
    fn an_entered_clients_taken_port_reopens_the_form() {
        let fixture = spawn_auth_server_with(false, None);
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("mcp-auth.json");
        let server = format!("{}/mcp", fixture.base);
        let squatter = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = squatter.local_addr().unwrap().port();
        save_client(Some(&store), &server, Some(&entered(&fixture, port))).unwrap();
        let (result, progress) = sign_in_outcome(&fixture, &store, "c", &CancelToken::new(), None);
        let Err(AuthError::NeedsClient(request)) = result else {
            panic!("expected a request for a client, got {result:?}");
        };
        assert_eq!(request.client_id.as_deref(), Some("typed-cid"));
        assert_eq!(request.callback_port, Some(port));
        assert!(request.has_secret);
        let reason = request.reason.expect("says the port is taken");
        assert!(reason.contains(&port.to_string()), "{reason}");
        assert!(progress.urls.lock().unwrap().is_empty());
    }

    /// A sign-in that takes over from another — the client was changed, or
    /// Authenticate pressed again — finds its port still held by the one it
    /// supersedes, which lets go within its own cancel poll. A port named
    /// exactly gets that moment to come free before it counts as taken.
    #[test]
    fn a_port_another_sign_in_is_letting_go_of_is_waited_for() {
        let fixture = spawn_auth_server_with(false, None);
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("mcp-auth.json");
        let server = format!("{}/mcp", fixture.base);
        let leaving = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = leaving.local_addr().unwrap().port();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(120));
            drop(leaving);
        });
        save_client(Some(&store), &server, Some(&entered(&fixture, port))).unwrap();
        let (result, progress) = sign_in_outcome(&fixture, &store, "c", &CancelToken::new(), None);
        result.expect("signed in once the port came free");
        assert_eq!(port_of(&progress.urls.lock().unwrap()[0]), port);
    }

    /// A secret kept with the tokens (`mcp add --client-secret`) is the
    /// configured client's — found by the flow and presented with it.
    #[test]
    fn a_kept_secret_is_presented_with_the_configured_client() {
        let fixture = spawn_auth_server_with(false, None);
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("mcp-auth.json");
        let server = format!("{}/mcp", fixture.base);
        save_client_secret(Some(&store), &server, Some("kept-secret")).unwrap();
        let client = crate::mcp::OAuthClientConfig {
            client_id: Some("pre-cid".to_string()),
            ..Default::default()
        };
        let (result, _) =
            sign_in_outcome(&fixture, &store, "c", &CancelToken::new(), Some(&client));
        result.expect("signed in");
        let requests = fixture.token_requests.lock().unwrap();
        let (_, body) = &requests[0];
        assert!(
            body.contains("client_id=pre-cid") && body.contains("client_secret=kept-secret"),
            "{body}"
        );
    }

    /// The secret kept for the entry's own client — typed in `/mcp`, or kept
    /// by `mcp add --client-secret` — outranks the one its file spells; the
    /// file's is the one sent where none is kept.
    #[test]
    fn the_secret_kept_for_the_entrys_client_outranks_the_files() {
        let fixture = spawn_auth_server_with(false, None);
        let dir = tempfile::tempdir().unwrap();
        let server = format!("{}/mcp", fixture.base);
        let client = crate::mcp::OAuthClientConfig {
            client_id: Some("pre-cid".to_string()),
            client_secret: Some("file-secret".to_string()),
            ..Default::default()
        };
        let plain = dir.path().join("plain.json");
        let (result, _) =
            sign_in_outcome(&fixture, &plain, "c", &CancelToken::new(), Some(&client));
        result.expect("signed in");
        let kept = dir.path().join("kept.json");
        save_client_secret(Some(&kept), &server, Some("kept-secret")).unwrap();
        let (result, _) = sign_in_outcome(&fixture, &kept, "c", &CancelToken::new(), Some(&client));
        result.expect("signed in");
        let requests = fixture.token_requests.lock().unwrap();
        assert!(
            requests[0].1.contains("client_secret=file-secret"),
            "{}",
            requests[0].1
        );
        assert!(
            requests[1].1.contains("client_secret=kept-secret")
                && !requests[1].1.contains("file-secret"),
            "{}",
            requests[1].1
        );
    }

    /// A client entered in `/mcp` outranks the one the server's entry names:
    /// it signs in on its own port with its own secret — so a plugin's or a
    /// project's shipped client can be replaced without writing to a file
    /// that is not the user's.
    #[test]
    fn an_entered_client_outranks_the_entrys_own() {
        let fixture = spawn_auth_server_with(false, None);
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("mcp-auth.json");
        let server = format!("{}/mcp", fixture.base);
        let port = free_port();
        save_client(Some(&store), &server, Some(&entered(&fixture, port))).unwrap();
        let configured = crate::mcp::OAuthClientConfig {
            client_id: Some("file-cid".to_string()),
            client_secret: Some("file-secret".to_string()),
            callback_port: Some(free_port()),
            callback_url: None,
            scopes: None,
        };
        let (result, progress) = sign_in_outcome(
            &fixture,
            &store,
            "c",
            &CancelToken::new(),
            Some(&configured),
        );
        result.expect("signed in");
        let url = progress.urls.lock().unwrap()[0].clone();
        assert_eq!(
            redirect_param(&url, "client_id").as_deref(),
            Some("typed-cid")
        );
        assert_eq!(
            redirect_param(&url, "redirect_uri").as_deref(),
            Some(crate::mcp::oauth_redirect_uri(port).as_str())
        );
        let requests = fixture.token_requests.lock().unwrap();
        let (_, body) = &requests[0];
        assert!(
            body.contains("client_id=typed-cid")
                && body.contains("client_secret=typed-secret")
                && !body.contains("file-"),
            "{body}"
        );
    }

    /// An entered client that names no authorization server is bound to
    /// none, so it is never presented: the entry's own client signs in in
    /// its place, and with none the flow registers one.
    #[test]
    fn an_entered_client_bound_to_no_authorization_server_is_never_presented() {
        let fixture = spawn_auth_server(None);
        let dir = tempfile::tempdir().unwrap();
        let server = format!("{}/mcp", fixture.base);
        let unbound = format!(
            r#"{{"clients":{{"{server}":{{"client_id":"unbound","client_secret":"s","callback_port":{}}}}}}}"#,
            free_port()
        );
        let configured = crate::mcp::OAuthClientConfig {
            client_id: Some("file-cid".to_string()),
            ..Default::default()
        };
        for (file, config, presented) in [
            ("named.json", Some(&configured), "file-cid"),
            ("none.json", None, "cid-1"),
        ] {
            let store = dir.path().join(file);
            std::fs::write(&store, &unbound).unwrap();
            let (result, progress) =
                sign_in_outcome(&fixture, &store, "c", &CancelToken::new(), config);
            result.expect("signed in");
            let url = progress.urls.lock().unwrap()[0].clone();
            assert_eq!(
                redirect_param(&url, "client_id").as_deref(),
                Some(presented)
            );
        }
    }

    /// The entry's own id entered on another port, its secret left as it
    /// was, signs in on that port with the secret its entry carries — read
    /// from the entry at the sign-in, never copied into the store — and only
    /// while the entry still names that id.
    #[test]
    fn an_entered_client_on_its_word_signs_in_with_its_entrys_secret() {
        let fixture = spawn_auth_server_with(false, None);
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("mcp-auth.json");
        let server = format!("{}/mcp", fixture.base);
        let port = free_port();
        let lent = StoredClient {
            client_id: Some("pre-cid".to_string()),
            callback_port: Some(port),
            issuer: Some(fixture.base.clone()),
            secret_from_entry: true,
            ..Default::default()
        };
        save_client(Some(&store), &server, Some(&lent)).unwrap();
        let configured = crate::mcp::OAuthClientConfig {
            client_id: Some("pre-cid".to_string()),
            client_secret: Some("file-secret".to_string()),
            callback_port: Some(free_port()),
            ..Default::default()
        };
        let (result, progress) = sign_in_outcome(
            &fixture,
            &store,
            "c",
            &CancelToken::new(),
            Some(&configured),
        );
        result.expect("signed in");
        assert_eq!(port_of(&progress.urls.lock().unwrap()[0]), port);
        // An entry naming another id since lends it nothing.
        let renamed = crate::mcp::OAuthClientConfig {
            client_id: Some("other-cid".to_string()),
            ..configured
        };
        let (result, _) =
            sign_in_outcome(&fixture, &store, "c", &CancelToken::new(), Some(&renamed));
        result.expect("signed in");
        let requests = fixture.token_requests.lock().unwrap();
        assert!(
            requests[0].1.contains("client_id=pre-cid")
                && requests[0].1.contains("client_secret=file-secret"),
            "{}",
            requests[0].1
        );
        assert!(
            requests[1].1.contains("client_id=pre-cid") && !requests[1].1.contains("client_secret"),
            "{}",
            requests[1].1
        );
        assert_eq!(load_client(Some(&store), &server), Some(lent));
    }

    /// A configured client turned away for its secret — none at all, the
    /// one kept for it, or the one its file spells — is asked for one, the
    /// id fixed since the entry names it, and no secret offered as kept: a
    /// secret typed then outranks the file's.
    #[test]
    fn a_configured_client_turned_away_is_asked_for_its_secret() {
        let fixture = spawn_auth_server_with(false, None);
        *fixture.token_reply.lock().unwrap() = Some((
            400,
            r#"{"error":"invalid_request","error_description":"client_secret is missing."}"#,
        ));
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("mcp-auth.json");
        let port = free_port();
        let client = crate::mcp::OAuthClientConfig {
            client_id: Some("pre-cid".to_string()),
            callback_port: Some(port),
            ..Default::default()
        };
        let (result, _) =
            sign_in_outcome(&fixture, &store, "c", &CancelToken::new(), Some(&client));
        let Err(AuthError::NeedsClient(request)) = result else {
            panic!("expected a request for the secret, got {result:?}");
        };
        assert_eq!(request.client_id.as_deref(), Some("pre-cid"));
        assert!(request.fixed_id);
        assert!(!request.has_secret);
        assert_eq!(request.callback_port, Some(port));
        let reason = request.reason.expect("the server's reason");
        assert!(reason.contains("client_secret is missing."), "{reason}");
        // The file's own secret, turned away: asked for one all the same.
        *fixture.token_reply.lock().unwrap() = Some((401, r#"{"error":"invalid_client"}"#));
        let with_secret = crate::mcp::OAuthClientConfig {
            client_secret: Some("file-secret".to_string()),
            callback_port: Some(free_port()),
            ..client
        };
        let (result, _) = sign_in_outcome(
            &fixture,
            &store,
            "c",
            &CancelToken::new(),
            Some(&with_secret),
        );
        let Err(AuthError::NeedsClient(request)) = result else {
            panic!("expected a request for the secret, got {result:?}");
        };
        assert!(request.fixed_id && !request.has_secret);
        assert_eq!(request.client_id.as_deref(), Some("pre-cid"));
        assert_eq!(request.callback_port, with_secret.callback_port);
        let reason = request.reason.expect("the server's reason");
        assert!(reason.contains("invalid_client"), "{reason}");
        assert!(!reason.contains("file-secret"), "{reason}");
        // So is a secret kept for it.
        let server = format!("{}/mcp", fixture.base);
        save_client_secret(Some(&store), &server, Some("kept-secret")).unwrap();
        let (result, _) = sign_in_outcome(
            &fixture,
            &store,
            "c",
            &CancelToken::new(),
            Some(&with_secret),
        );
        let Err(AuthError::NeedsClient(request)) = result else {
            panic!("expected a request for the secret, got {result:?}");
        };
        assert!(request.fixed_id && !request.has_secret);
    }

    /// A registration the authorization server refuses leaves one way in —
    /// a client of the user's own — so the flow asks for it with the
    /// server's reason; a server that merely failed to answer is an error.
    #[test]
    fn a_refused_registration_asks_for_a_client() {
        let fixture = spawn_auth_server(Some((
            403,
            r#"{"error":"access_denied","error_description":"registration is closed"}"#,
        )));
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("mcp-auth.json");
        let (result, progress) = sign_in_outcome(&fixture, &store, "c", &CancelToken::new(), None);
        let Err(AuthError::NeedsClient(request)) = result else {
            panic!("expected a request for a client, got {result:?}");
        };
        assert_eq!(request.client_id, None);
        assert_eq!(request.callback_port, Some(DEFAULT_CALLBACK_PORT));
        let reason = request.reason.expect("the server's reason");
        assert!(
            reason.contains("access_denied") && reason.contains("registration is closed"),
            "{reason}"
        );
        assert!(progress.urls.lock().unwrap().is_empty());
        let unavailable = spawn_auth_server(Some((503, "try later")));
        let (result, _) = sign_in_outcome(&unavailable, &store, "c", &CancelToken::new(), None);
        let Err(AuthError::Failed(detail)) = result else {
            panic!("expected a failure, got {result:?}");
        };
        assert!(
            detail.contains("registration failed") && detail.contains("503"),
            "{detail}"
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
        let (result, progress) =
            sign_in_with(&fixture, &store, "c", &CancelToken::new(), Some(&client));
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
        let (result, progress) = sign_in(&fixture, &store, "code-1", &CancelToken::new());
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
        let (result, progress) = sign_in(&fixture, &store, "code-2", &CancelToken::new());
        result.expect("signed in again");
        assert_eq!(fixture.registrations.load(Ordering::SeqCst), 1);
        assert_eq!(port_of(&progress.urls.lock().unwrap()[0]), port);
        // With that port taken, a new registration names the new one.
        let _squatter = TcpListener::bind(("127.0.0.1", port)).expect("take the port");
        let (result, progress) = sign_in(&fixture, &store, "code-3", &CancelToken::new());
        result.expect("signed in on another port");
        assert_eq!(fixture.registrations.load(Ordering::SeqCst), 2);
        assert_ne!(port_of(&progress.urls.lock().unwrap()[0]), port);
        assert_eq!(
            load_tokens(Some(&store), &server).unwrap().client_id,
            "cid-2"
        );
    }

    /// A sign-in's own requests name this client too — both metadata
    /// documents, the registration, the exchange and every renewal after it.
    /// An authorization server behind the gateway its MCP server sits behind
    /// turns away a request naming none, and the sign-in ended at discovery
    /// with that gateway's 403.
    #[test]
    fn a_sign_in_gets_through_a_gateway_that_refuses_unnamed_clients() {
        let fixture = spawn_auth_server_behind(true, true, None);
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("mcp-auth.json");
        let server = format!("{}/mcp", fixture.base);
        let (result, _) = sign_in(&fixture, &store, "code-1", &CancelToken::new());
        assert_eq!(result, Ok(()));
        refresh_grant(Some(&store), &server, None).expect("the renewal got through");
        let agents = fixture.agents.lock().unwrap();
        let asked = |path: &str| agents.iter().filter(|(asked, _)| asked == path).count();
        assert_eq!(asked("/.well-known/oauth-protected-resource/mcp"), 1);
        assert_eq!(asked("/.well-known/oauth-authorization-server"), 1);
        assert_eq!(asked("/register"), 1);
        assert_eq!(asked("/token"), 2, "the exchange, then the renewal");
        let ours = crate::mcp::user_agent();
        for (path, agent) in agents.iter() {
            assert_eq!(agent.as_deref(), Some(ours.as_str()), "{path}");
        }
    }

    /// A registration names the scopes the sign-in is about to ask for. An
    /// authorization server that keeps a client to the scopes it registered
    /// with turns the authorize request away (`invalid_scope`) from one that
    /// registered none — after the user has opened the URL; and with nothing to
    /// ask for, the registration names none.
    #[test]
    fn a_registration_names_the_scopes_the_sign_in_asks_for() {
        let fixture = spawn_auth_server(None);
        let dir = tempfile::tempdir().unwrap();
        let server_url = format!("{}/mcp", fixture.base);
        let run = |store: &Path, challenge: Option<&str>| {
            let (_paste_tx, paste_rx) = mpsc::channel();
            let progress = std::sync::Arc::new(Recorded::approving("code-1"));
            run_auth_flow(
                &SignIn {
                    server_url: &server_url,
                    challenge,
                    store_path: Some(store),
                    client: None,
                },
                &paste_rx,
                &progress,
                &CancelToken::new(),
            )
            .expect("signed in");
            progress.urls.lock().unwrap()[0].clone()
        };
        // The refusal's own challenge says what the server wants.
        let authorize_url = run(
            &dir.path().join("challenged.json"),
            Some(r#"Bearer error="invalid_token", scope="read write""#),
        );
        assert_eq!(
            redirect_param(&authorize_url, "scope").as_deref(),
            Some("read write")
        );
        // Nothing asked for: no `scope` on the URL, none in the registration.
        let authorize_url = run(&dir.path().join("plain.json"), None);
        assert_eq!(redirect_param(&authorize_url, "scope"), None);
        let registered = fixture.registered.lock().unwrap();
        let bodies: Vec<Value> = registered
            .iter()
            .map(|body| serde_json::from_str(body).expect("a JSON registration"))
            .collect();
        assert_eq!(bodies.len(), 2);
        assert_eq!(bodies[0]["scope"], "read write");
        assert!(bodies[1].get("scope").is_none(), "{}", bodies[1]);
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
        let (result, _) = sign_in(&fixture, &store, "c", &CancelToken::new());
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
        let (result, _) = sign_in(&fixture, &store, "c", &CancelToken::new());
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
        let (result, _) = sign_in(&fixture, &store, "c", &CancelToken::new());
        let error = result.expect_err("refused");
        assert!(
            error.contains("invalid_redirect_uri")
                && error.contains("loopback redirects are not allowed"),
            "{error}"
        );
    }

    /// A refusal that is a web page — a gateway's, not the authorization
    /// server's — is described by its title in a sign-in's reason and in a
    /// renewal's, where its markup used to be printed whole.
    #[test]
    fn a_gateway_s_page_is_described_in_a_refusal() {
        let page = "<html>\r\n<head><title>403 Forbidden</title></head>\r\n<body>\r\n\
            <center><h1>403 Forbidden</h1></center>\r\n</body>\r\n</html>\r\n";
        assert_eq!(
            oauth_error_detail(403, page),
            "HTTP 403: a web page titled \"403 Forbidden\""
        );
        assert_eq!(
            refresh_failure_kind(Some(403), page),
            RefreshFailure::Transient("HTTP 403: a web page titled \"403 Forbidden\"".to_string())
        );
        // A gateway's page at the statuses a verdict on the grant arrives
        // with is still the gateway speaking: the grant is kept.
        assert_eq!(
            refresh_failure_kind(Some(400), page),
            RefreshFailure::Transient("HTTP 400: a web page titled \"403 Forbidden\"".to_string())
        );
    }

    /// Esc during discovery means no authorize URL is ever shown; and with
    /// nowhere to keep the grant the flow says so before it is.
    #[test]
    fn a_sign_in_that_cannot_finish_never_shows_the_url() {
        let fixture = spawn_auth_server(None);
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("mcp-auth.json");
        let cancel = CancelToken::new();
        cancel.cancel();
        let (result, progress) = sign_in(&fixture, &store, "c", &cancel);
        assert!(result.is_err());
        assert!(progress.urls.lock().unwrap().is_empty(), "shown after Esc");
        let (_paste_tx, paste_rx) = mpsc::channel();
        let server_url = format!("{}/mcp", fixture.base);
        let progress = std::sync::Arc::new(Recorded::default());
        let result = run_auth_flow(
            &SignIn {
                server_url: &server_url,
                challenge: None,
                store_path: None,
                client: None,
            },
            &paste_rx,
            &progress,
            &CancelToken::new(),
        );
        assert!(result.is_err());
        assert!(
            progress.urls.lock().unwrap().is_empty(),
            "shown with no store"
        );
    }
}
