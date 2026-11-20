//! The OAuth client a sign-in presents when the user supplies one
//! (`docs/mcp.md` *Pre-registered clients*): the client kept in the token
//! store, which of the entry's and the kept one a sign-in uses, what the
//! flow asks the `/mcp` page for when it has none, and what the page's form
//! hands back.
//!
//! An authorization server that registers no clients itself (Google's, for
//! its MCP servers) leaves a sign-in one way forward: a client the user
//! created with the provider. The spec's order is a pre-registered client,
//! then a client metadata document, then dynamic registration, then *ask the
//! user* — this module is the vocabulary of that last step. Pure: the store's
//! file and the form are their own modules'.

use serde_json::Value;

use super::config::OAuthClientConfig;
use crate::secrets::SecretValue;

/// A pre-registered client kept for one server in the token store's
/// `clients` section — apart from the login, so "Clear authentication" leaves
/// it. Two things are kept there, told apart by `client_id`: a client the
/// user **entered** in `/mcp` (its id, and whatever else they gave), and the
/// **secret alone** of the client an entry names (`mcp add --client-secret`).
#[derive(Clone, Default, PartialEq, Eq)]
pub struct StoredClient {
    /// The client's id — `None` when only a secret is kept, for the client
    /// the server's entry names.
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
    /// The port the client's registered redirect URI names, when the
    /// provider was told one.
    pub callback_port: Option<u16>,
    /// The authorization server the client was created with, as its
    /// metadata names itself. A client is that server's alone: it is never
    /// presented to another (the spec's authorization-server binding).
    pub issuer: Option<String>,
    /// An entered client that signs in with the secret its server's
    /// **entry** carries — the entry's own client, re-entered with its
    /// secret field left empty to keep it. The secret stays in the file and
    /// is read at each sign-in, never copied here; without this an entered
    /// client with no secret is a public one.
    pub secret_from_entry: bool,
}

impl StoredClient {
    /// Read one `clients` entry — `None` for one that keeps nothing, and for
    /// a client that names no authorization server: there is nothing to
    /// hold that one to, so nothing of it is read — not as a client, and
    /// not as a secret the entry's own client could borrow.
    #[must_use]
    pub fn from_json(entry: &Value) -> Option<Self> {
        let text = |key: &str| {
            entry
                .get(key)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .map(str::to_string)
        };
        let client_id = text("client_id");
        let issuer = text("issuer");
        if client_id.is_some() && issuer.is_none() {
            return None;
        }
        let client = Self {
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
            issuer,
        };
        (client != Self::default()).then_some(client)
    }

    /// The entry as the store spells it, with only the fields set.
    #[must_use]
    pub fn to_json(&self) -> Value {
        let mut entry = serde_json::Map::new();
        for (key, value) in [
            ("client_id", &self.client_id),
            ("client_secret", &self.client_secret),
            ("issuer", &self.issuer),
        ] {
            if let Some(value) = value {
                entry.insert(key.to_string(), value.as_str().into());
            }
        }
        if let Some(port) = self.callback_port {
            entry.insert("callback_port".to_string(), port.into());
        }
        if self.secret_from_entry {
            entry.insert("secret_from_entry".to_string(), true.into());
        }
        Value::Object(entry)
    }

    /// Was this client entered in `/mcp` — it names its own id — rather than
    /// being the secret kept for the client the entry names?
    #[must_use]
    pub const fn is_entered(&self) -> bool {
        self.client_id.is_some()
    }
}

impl std::fmt::Debug for StoredClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoredClient")
            .field("client_id", &self.client_id)
            .field(
                "client_secret",
                &self.client_secret.as_ref().map(|_| REDACTED),
            )
            .field("callback_port", &self.callback_port)
            .field("issuer", &self.issuer)
            .field("secret_from_entry", &self.secret_from_entry)
            .finish()
    }
}

/// What a `Debug` print shows in a secret's place.
const REDACTED: &str = "<redacted>";

/// Where the client a server signs in as came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientSource {
    /// Typed into `/mcp` and kept in the token store.
    Entered,
    /// The `oauth` object of the server's entry.
    Config,
}

/// The pre-registered client a sign-in presents — never one it registered.
#[derive(Clone, PartialEq, Eq)]
pub struct PreRegistered {
    pub client_id: String,
    pub client_secret: Option<String>,
    /// The port to listen on, exactly — or any free one.
    pub callback_port: Option<u16>,
    /// The redirect URI an **entry's** client was registered with
    /// (`callbackUrl`) — never one entered in `/mcp`, which is the user's
    /// own client on our own redirect.
    pub callback_url: Option<String>,
    pub source: ClientSource,
}

impl PreRegistered {
    /// Where a sign-in as this client listens: the port — exactly, or
    /// `None` for any free one — and the path its redirect arrives on. A
    /// callback URL decides both (its port, else `callbackPort`); without
    /// one it is `callbackPort` on `/callback`.
    ///
    /// # Errors
    /// A callback URL that is not an http address on this machine — one a
    /// `${VAR}` expanded to somewhere else.
    pub fn callback(&self) -> Result<(Option<u16>, String), String> {
        match &self.callback_url {
            Some(text) => {
                let url = LoopbackUrl::parse(text)
                    .map_err(|why| format!("the callback URL {text} {why}"))?;
                Ok((url.port.or(self.callback_port), url.url.path().to_string()))
            }
            None => Ok((self.callback_port, CALLBACK_PATH.to_string())),
        }
    }

    /// The `redirect_uri` a sign-in listening on `port` sends: the callback
    /// URL as written, the port filled in when it names none (codex's
    /// shape); else [`callback_redirect`].
    ///
    /// # Errors
    /// As [`callback`](Self::callback).
    pub fn redirect_uri(&self, port: u16) -> Result<String, String> {
        match &self.callback_url {
            Some(text) => {
                let url = LoopbackUrl::parse(text)
                    .map_err(|why| format!("the callback URL {text} {why}"))?;
                if url.port.is_some() {
                    return Ok(text.trim().to_string());
                }
                let mut with_port = url.url;
                with_port
                    .set_port(Some(port))
                    .map_err(|()| format!("the callback URL {text} takes no port"))?;
                Ok(with_port.to_string())
            }
            None => Ok(callback_redirect(Some(port))),
        }
    }
}

/// The path our own redirect URI names.
const CALLBACK_PATH: &str = "/callback";

/// codex's redirect for a client written its way with none of its own: on
/// `127.0.0.1`, the listening port filled in at sign-in.
pub(crate) const CODEX_CALLBACK_URL: &str = "http://127.0.0.1/callback";

/// Why a callback URL is refused: a redirect is received here, so it must
/// name this machine.
const NOT_LOOPBACK: &str =
    "must be an http address on this machine (127.0.0.1, localhost or [::1])";

/// A callback URL read: an http address on a loopback host, and the port it
/// names explicitly (an explicit `:80` included).
struct LoopbackUrl {
    url: url::Url,
    port: Option<u16>,
}

impl LoopbackUrl {
    fn parse(text: &str) -> Result<Self, &'static str> {
        let text = text.trim();
        let url = url::Url::parse(text).map_err(|_| NOT_LOOPBACK)?;
        let loopback = match url.host() {
            Some(url::Host::Domain(host)) => host == "localhost",
            Some(url::Host::Ipv4(ip)) => ip == std::net::Ipv4Addr::LOCALHOST,
            Some(url::Host::Ipv6(ip)) => ip == std::net::Ipv6Addr::LOCALHOST,
            None => false,
        };
        if url.scheme() != "http" || !loopback {
            return Err(NOT_LOOPBACK);
        }
        // `Url::port` hides the scheme's default; written out it was meant.
        let authority = text
            .split_once("://")
            .map_or("", |(_, rest)| rest)
            .split(['/', '?', '#'])
            .next()
            .unwrap_or_default();
        let written = authority
            .rsplit_once(':')
            .is_some_and(|(_, port)| !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()));
        let port = url
            .port()
            .or_else(|| written.then(|| url.port_or_known_default()).flatten());
        Ok(Self { url, port })
    }
}

/// Check a callback URL an entry writes, against its `callbackPort`.
///
/// # Errors
/// Not an http address on this machine, or a port that disagrees with
/// `port` — each worded to follow the key's name.
pub(crate) fn check_callback_url(text: &str, port: Option<u16>) -> Result<(), String> {
    let url = LoopbackUrl::parse(text).map_err(str::to_string)?;
    match (url.port, port) {
        (Some(named), Some(port)) if named != port => Err(format!(
            "names port {named}, but the callback port is {port}"
        )),
        _ => Ok(()),
    }
}

impl std::fmt::Debug for PreRegistered {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreRegistered")
            .field("client_id", &self.client_id)
            .field(
                "client_secret",
                &self.client_secret.as_ref().map(|_| REDACTED),
            )
            .field("callback_port", &self.callback_port)
            .field("callback_url", &self.callback_url)
            .field("source", &self.source)
            .finish()
    }
}

/// The pre-registered client a sign-in to the authorization server `issuer`
/// uses, from the entry's `oauth` object and what the token store keeps for
/// the server — `None` when neither names one, and the flow reuses a client
/// it registered, registers one, or asks.
///
/// **What the user entered in `/mcp` wins**: it is what they last said, and
/// the one client a project's or a plugin's entry can be given without
/// editing a file that is not theirs. It is bound to the authorization server
/// it was entered for, so another issuer never sees it — and one that names
/// none is never presented at all. **It is the whole client, as its form
/// said**: its port is its own (none is any free port — the form opened on
/// the entry's, so leaving it out was a choice), and so is its secret (none
/// is a public client), with one exception it has to state itself —
/// [`StoredClient::secret_from_entry`], the entry's own client keeping the
/// secret its entry carries.
///
/// An entry's client takes the secret kept for it (`mcp add
/// --client-secret`), and never an entered client's: that one is another
/// client's secret.
#[must_use]
pub fn preregistered_client(
    configured: Option<&OAuthClientConfig>,
    stored: Option<&StoredClient>,
    issuer: Option<&str>,
) -> Option<PreRegistered> {
    let entered = stored.filter(|client| {
        client.is_entered()
            && client
                .issuer
                .as_deref()
                .is_some_and(|bound| issuer.is_some_and(|issuer| issuers_match(bound, issuer)))
    });
    if let Some(client) = entered
        && let Some(client_id) = &client.client_id
    {
        // Its entry's secret: on its own say-so, and only while the entry
        // still names this client. Never by default — a secret the server
        // refused and the form then left out would ride along again.
        let lent = configured
            .filter(|config| {
                client.secret_from_entry && config.client_id.as_deref() == Some(client_id.as_str())
            })
            .and_then(|config| config.client_secret.clone());
        return Some(PreRegistered {
            client_id: client_id.clone(),
            client_secret: client.client_secret.clone().or(lent),
            callback_port: client.callback_port,
            callback_url: None,
            source: ClientSource::Entered,
        });
    }
    let config = configured?;
    Some(PreRegistered {
        client_id: config.client_id.clone()?,
        client_secret: config.client_secret.clone().or_else(|| {
            stored
                .filter(|client| !client.is_entered())
                .and_then(|client| client.client_secret.clone())
        }),
        callback_port: config.callback_port,
        callback_url: config.callback_url.clone(),
        source: ClientSource::Config,
    })
}

/// The client a server's `/mcp` page names (`OAuth client:`), and where it
/// came from. Which authorization server the sign-in will find is not known
/// here, so an entered client reads as the one in effect whatever it is
/// bound to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpClientInfo {
    pub client_id: String,
    pub source: ClientSource,
}

/// The client in effect for a server: the entered one, else the entry's.
#[must_use]
pub fn client_in_effect(
    configured: Option<&OAuthClientConfig>,
    stored: Option<&StoredClient>,
) -> Option<McpClientInfo> {
    if let Some(client_id) = stored.and_then(|client| client.client_id.clone()) {
        return Some(McpClientInfo {
            client_id,
            source: ClientSource::Entered,
        });
    }
    Some(McpClientInfo {
        client_id: configured?.client_id.clone()?,
        source: ClientSource::Config,
    })
}

/// Why a sign-in asks for an OAuth client.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum ClientNeed {
    /// The authorization server registers no clients itself.
    #[default]
    NoRegistration,
    /// It offers registration and refused this app — in its own words.
    RegistrationRefused(String),
    /// It turned the client away when the code was redeemed — in its own
    /// words.
    Rejected(String),
    /// The user asked to enter one.
    Asked,
}

/// What a sign-in asks the `/mcp` page for when it has no client to sign in
/// as, or the one it has was turned away: the form the page opens.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClientRequest {
    /// The authorization server the client is for, as its metadata names
    /// itself.
    pub issuer: String,
    pub need: ClientNeed,
    /// The client id in effect — what the form starts from.
    pub client_id: Option<String>,
    /// Is a secret kept for that client? Leaving the form's secret empty
    /// then keeps it: a provider may show a secret only once.
    pub has_secret: bool,
    /// The callback port in effect.
    pub callback_port: Option<u16>,
}

/// The client the `/mcp` form submits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnteredClient {
    /// The authorization server it was entered for.
    pub issuer: String,
    pub client_id: String,
    /// The secret typed — `None` when the field was left empty.
    pub client_secret: Option<SecretValue>,
    /// The field was left empty over a client whose secret is kept: keep it.
    pub keep_secret: bool,
    pub callback_port: Option<u16>,
}

/// The store entry a submitted form becomes — the **one** client kept for
/// its server, replacing whatever was (`kept`): its id, the authorization
/// server it was entered for, its port, and its secret.
///
/// A secret typed is the client's own. A field left empty is a **public
/// client** — unless it was left empty over a client whose secret is kept
/// (`keep_secret`), which keeps the secret **in effect** for this client:
///
/// - the same client's, entered before for the same authorization server —
///   its own secret, or its say-so to use its entry's;
/// - the one its entry (`configured`) carries **in the file**, when the entry
///   names this client: never copied here, only recorded as the one to use
///   ([`StoredClient::secret_from_entry`]), so it is read at each sign-in and
///   a `${VAR}` it came from is not written down;
/// - else the one `mcp add --client-secret` kept for the entry's client.
///
/// And no other's: another id's secret, or another authorization server's,
/// is not this client's to keep.
#[must_use]
pub fn stored_from_entered(
    entered: &EnteredClient,
    kept: Option<&StoredClient>,
    configured: Option<&OAuthClientConfig>,
) -> StoredClient {
    let before = kept.filter(|client| {
        client.client_id.as_deref() == Some(entered.client_id.as_str())
            && client
                .issuer
                .as_deref()
                .is_some_and(|issuer| issuers_match(issuer, &entered.issuer))
    });
    let entry =
        configured.filter(|config| config.client_id.as_deref() == Some(entered.client_id.as_str()));
    let (client_secret, secret_from_entry) = match &entered.client_secret {
        Some(secret) => (Some(secret.expose().to_string()), false),
        None if !entered.keep_secret => (None, false),
        None => match (before, entry) {
            (Some(client), _) => (client.client_secret.clone(), client.secret_from_entry),
            (None, Some(entry)) if entry.client_secret.is_some() => (None, true),
            (None, Some(_)) => (
                kept.filter(|client| !client.is_entered())
                    .and_then(|client| client.client_secret.clone()),
                false,
            ),
            (None, None) => (None, false),
        },
    };
    StoredClient {
        client_id: Some(entered.client_id.clone()),
        client_secret,
        callback_port: entered.callback_port,
        issuer: Some(entered.issuer.clone()),
        secret_from_entry,
    }
}

/// Do two issuer identifiers name one authorization server? Compared as
/// written but for one trailing `/`: Google's resource metadata names
/// `https://accounts.google.com/`, its own metadata
/// `https://accounts.google.com`.
#[must_use]
pub fn issuers_match(left: &str, right: &str) -> bool {
    let bare = |issuer: &str| issuer.strip_suffix('/').unwrap_or(issuer).to_string();
    bare(left) == bare(right)
}

/// The host an issuer is known by — what a page calls the authorization
/// server (`accounts.google.com`). An issuer that is no URL is itself.
#[must_use]
pub fn issuer_host(issuer: &str) -> String {
    url::Url::parse(issuer)
        .ok()
        .and_then(|url| {
            let host = url.host_str()?.to_string();
            Some(match url.port() {
                Some(port) => format!("{host}:{port}"),
                None => host,
            })
        })
        .unwrap_or_else(|| issuer.to_string())
}

/// What stands in for the port in a redirect URI nobody has fixed one for.
pub const CALLBACK_PORT_PLACEHOLDER: &str = "PORT";

/// The callback port a form for a **new** client opens on. A provider that
/// matches redirect URIs exactly — most do, for a client created in their
/// console — must be told one, and a port left to chance fails in the
/// browser, at the provider, with nothing here to say why; so the form
/// shows a redirect URI that can be registered as it stands. Only a
/// suggestion: the field takes another port, or none (any free one).
pub const SUGGESTED_CALLBACK_PORT: u16 = 8976;

/// The redirect URI a sign-in listens on — what a provider that matches
/// redirect URIs exactly must be told. Without a fixed port it carries
/// [`CALLBACK_PORT_PLACEHOLDER`]: any free port is used, which a native
/// (desktop) client's provider accepts (RFC 8252 §7.3).
#[must_use]
pub fn callback_redirect(port: Option<u16>) -> String {
    match port {
        Some(port) => format!("http://localhost:{port}/callback"),
        None => format!("http://localhost:{CALLBACK_PORT_PLACEHOLDER}/callback"),
    }
}

/// Read a callback port as typed: empty is none, else a port number.
///
/// # Errors
/// Anything that is not a number from 1 to 65535.
pub fn parse_callback_port(text: &str) -> Result<Option<u16>, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    text.parse::<u16>()
        .ok()
        .filter(|port| *port != 0)
        .map(Some)
        .ok_or_else(|| "The callback port is a number from 1 to 65535".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn config(id: Option<&str>, secret: Option<&str>, port: Option<u16>) -> OAuthClientConfig {
        OAuthClientConfig {
            client_id: id.map(str::to_string),
            client_secret: secret.map(str::to_string),
            callback_port: port,
            scopes: None,
            callback_url: None,
        }
    }

    fn with_url(url: &str, port: Option<u16>) -> PreRegistered {
        PreRegistered {
            client_id: "cid".to_string(),
            client_secret: None,
            callback_port: port,
            callback_url: Some(url.to_string()),
            source: ClientSource::Config,
        }
    }

    /// An entry's client with a callback URL listens where that URL says —
    /// its port, else `callbackPort`, else any — on its path, and is sent as
    /// written, the listening port filled in when it names none.
    #[test]
    fn a_callback_url_decides_where_the_sign_in_listens_and_what_it_sends() {
        let github = with_url("http://127.0.0.1:12799/callback/ymAt1Jnt6ghN", Some(12799));
        assert_eq!(
            github.callback(),
            Ok((Some(12799), "/callback/ymAt1Jnt6ghN".to_string()))
        );
        assert_eq!(
            github.redirect_uri(12799),
            Ok("http://127.0.0.1:12799/callback/ymAt1Jnt6ghN".to_string())
        );
        let codex = with_url("http://127.0.0.1/callback", None);
        assert_eq!(codex.callback(), Ok((None, "/callback".to_string())));
        assert_eq!(
            codex.redirect_uri(41234),
            Ok("http://127.0.0.1:41234/callback".to_string())
        );
        let fixed = with_url("http://127.0.0.1/callback", Some(12798));
        assert_eq!(fixed.callback(), Ok((Some(12798), "/callback".to_string())));
        assert_eq!(
            fixed.redirect_uri(12798),
            Ok("http://127.0.0.1:12798/callback".to_string())
        );
        let local = with_url("http://localhost/oauth", None);
        assert_eq!(
            local.redirect_uri(5000),
            Ok("http://localhost:5000/oauth".to_string())
        );
        // An explicit default port is still the one registered.
        let eighty = with_url("http://localhost:80/cb", None);
        assert_eq!(eighty.callback(), Ok((Some(80), "/cb".to_string())));
        assert_eq!(
            eighty.redirect_uri(80),
            Ok("http://localhost:80/cb".to_string())
        );
        // Without one, our own redirect.
        let plain = PreRegistered {
            callback_url: None,
            ..with_url("", Some(8976))
        };
        assert_eq!(plain.callback(), Ok((Some(8976), "/callback".to_string())));
        assert_eq!(
            plain.redirect_uri(8976),
            Ok("http://localhost:8976/callback".to_string())
        );
        // One that expanded to somewhere else is refused at the sign-in.
        let elsewhere = with_url("http://example.com/cb", None);
        assert!(elsewhere.callback().unwrap_err().contains("this machine"));
    }

    /// A client entered in `/mcp` is the user's own: it never takes the
    /// redirect its server's entry names.
    #[test]
    fn an_entered_client_never_takes_the_entrys_callback_url() {
        let entry = OAuthClientConfig {
            callback_url: Some("http://127.0.0.1:12799/callback/x".to_string()),
            ..config(Some("cid"), Some("shh"), Some(12799))
        };
        let from_entry = preregistered_client(Some(&entry), None, Some(GOOGLE)).unwrap();
        assert_eq!(
            from_entry.callback_url.as_deref(),
            Some("http://127.0.0.1:12799/callback/x")
        );
        let typed = StoredClient {
            callback_port: Some(12799),
            ..entered("cid", None, Some(GOOGLE))
        };
        let chosen = preregistered_client(Some(&entry), Some(&typed), Some(GOOGLE)).unwrap();
        assert_eq!(chosen.source, ClientSource::Entered);
        assert_eq!(chosen.callback_url, None);
        assert_eq!(
            chosen.redirect_uri(12799),
            Ok("http://localhost:12799/callback".to_string())
        );
    }

    fn entered(id: &str, secret: Option<&str>, issuer: Option<&str>) -> StoredClient {
        StoredClient {
            client_id: Some(id.to_string()),
            client_secret: secret.map(str::to_string),
            callback_port: None,
            issuer: issuer.map(str::to_string),
            secret_from_entry: false,
        }
    }

    const GOOGLE: &str = "https://accounts.google.com";

    #[test]
    fn a_stored_client_round_trips_with_only_the_fields_set() {
        let whole = StoredClient {
            client_id: Some("cid".to_string()),
            client_secret: Some("shh".to_string()),
            callback_port: Some(8976),
            issuer: Some(GOOGLE.to_string()),
            secret_from_entry: false,
        };
        assert_eq!(
            whole.to_json(),
            json!({"client_id": "cid", "client_secret": "shh", "issuer": GOOGLE, "callback_port": 8976})
        );
        assert_eq!(StoredClient::from_json(&whole.to_json()), Some(whole));
        // A public client entered with nothing else is its id and the
        // authorization server it is for.
        let bare = entered("cid", None, Some(GOOGLE));
        assert_eq!(
            bare.to_json(),
            json!({"client_id": "cid", "issuer": GOOGLE})
        );
        assert_eq!(StoredClient::from_json(&bare.to_json()), Some(bare));
        // One that signs in with its entry's secret says so — the secret
        // itself stays in the file.
        let lent = StoredClient {
            secret_from_entry: true,
            ..entered("cid", None, Some(GOOGLE))
        };
        assert_eq!(
            lent.to_json(),
            json!({"client_id": "cid", "issuer": GOOGLE, "secret_from_entry": true})
        );
        assert_eq!(StoredClient::from_json(&lent.to_json()), Some(lent));
        // Only a client can say it: beside a bare secret it means nothing.
        let kept =
            StoredClient::from_json(&json!({"client_secret": "shh", "secret_from_entry": true}))
                .unwrap();
        assert!(!kept.secret_from_entry);
    }

    #[test]
    fn the_cli_s_secret_only_entry_reads_as_a_secret_kept_for_the_entry() {
        // What `mcp add --client-secret` has always written.
        let kept = StoredClient::from_json(&json!({"client_secret": "shh"})).unwrap();
        assert_eq!(kept.client_secret.as_deref(), Some("shh"));
        assert!(!kept.is_entered());
        // An entry that keeps nothing is no client; a zero port is no port.
        assert_eq!(StoredClient::from_json(&json!({})), None);
        assert_eq!(StoredClient::from_json(&json!({"client_id": "  "})), None);
        assert_eq!(
            StoredClient::from_json(
                &json!({"client_id": "cid", "issuer": GOOGLE, "callback_port": 0})
            )
            .unwrap()
            .callback_port,
            None
        );
        // A client that names no authorization server is nobody's — there
        // is nothing to hold it to — so nothing of it is read: not as a
        // client, and not as a secret the entry's client could borrow.
        assert_eq!(
            StoredClient::from_json(&json!({"client_id": "cid", "client_secret": "shh"})),
            None
        );
    }

    #[test]
    fn no_secret_reaches_a_debug_print() {
        let client = entered("cid", Some("hunter2"), Some(GOOGLE));
        let printed = format!("{client:?}");
        assert!(!printed.contains("hunter2"), "{printed}");
        assert!(
            printed.contains("cid") && printed.contains("<redacted>"),
            "{printed}"
        );
        let chosen = preregistered_client(None, Some(&client), Some(GOOGLE)).unwrap();
        assert!(!format!("{chosen:?}").contains("hunter2"));
        let submitted = EnteredClient {
            issuer: GOOGLE.to_string(),
            client_id: "cid".to_string(),
            client_secret: Some(SecretValue::new("hunter2")),
            keep_secret: false,
            callback_port: None,
        };
        assert!(!format!("{submitted:?}").contains("hunter2"));
    }

    #[test]
    fn an_entered_client_outranks_the_entrys() {
        let entry = config(Some("from-config"), Some("config-secret"), Some(8000));
        let typed = StoredClient {
            callback_port: Some(9000),
            ..entered("typed", Some("typed-secret"), Some(GOOGLE))
        };
        let chosen = preregistered_client(Some(&entry), Some(&typed), Some(GOOGLE)).unwrap();
        assert_eq!(chosen.client_id, "typed");
        assert_eq!(chosen.client_secret.as_deref(), Some("typed-secret"));
        assert_eq!(chosen.callback_port, Some(9000));
        assert_eq!(chosen.source, ClientSource::Entered);
        // What the form said is the whole client: another client's secret
        // is never lent to it, and a port field left empty is any free port
        // — the form opened on the entry's, so leaving it out was a choice.
        let bare = entered("typed", None, Some(GOOGLE));
        let chosen = preregistered_client(Some(&entry), Some(&bare), Some(GOOGLE)).unwrap();
        assert_eq!(chosen.client_secret, None);
        assert_eq!(chosen.callback_port, None);
    }

    #[test]
    fn an_entered_client_takes_its_entrys_secret_only_on_its_say_so() {
        // `Change OAuth client` over a configured client, to give it a port:
        // the secret its entry carries still counts — the form's secret was
        // left empty to keep it, and the kept client says so.
        let entry = config(Some("cid"), Some("config-secret"), None);
        let lent = StoredClient {
            callback_port: Some(9000),
            secret_from_entry: true,
            ..entered("cid", None, Some(GOOGLE))
        };
        let chosen = preregistered_client(Some(&entry), Some(&lent), Some(GOOGLE)).unwrap();
        assert_eq!(chosen.client_secret.as_deref(), Some("config-secret"));
        assert_eq!(chosen.callback_port, Some(9000));
        assert_eq!(chosen.source, ClientSource::Entered);
        // Without it the same id is a public client, whatever the entry
        // carries: a secret the server just refused, left out on the form,
        // must not ride along again.
        let public = entered("cid", None, Some(GOOGLE));
        let chosen = preregistered_client(Some(&entry), Some(&public), Some(GOOGLE)).unwrap();
        assert_eq!(chosen.client_secret, None);
        // It is the entry's own client's say-so alone: an entry that names
        // another id since lends nothing…
        let renamed = config(Some("other"), Some("config-secret"), None);
        let chosen = preregistered_client(Some(&renamed), Some(&lent), Some(GOOGLE)).unwrap();
        assert_eq!(chosen.client_secret, None);
        // …and a secret of its own comes first.
        let own = StoredClient {
            client_secret: Some("typed-secret".to_string()),
            ..lent
        };
        let chosen = preregistered_client(Some(&entry), Some(&own), Some(GOOGLE)).unwrap();
        assert_eq!(chosen.client_secret.as_deref(), Some("typed-secret"));
    }

    /// The loop a refused secret used to make: the entry carries a secret
    /// the server turns away, the form reopens, its secret is left empty —
    /// a public client — and the very next sign-in must not present the
    /// entry's secret again.
    #[test]
    fn a_secret_left_out_after_a_refusal_stays_out() {
        let entry = config(Some("cid"), Some("refused"), Some(8000));
        let form = EnteredClient {
            issuer: GOOGLE.to_string(),
            client_id: "cid".to_string(),
            client_secret: None,
            // The request said no secret is kept: nothing to keep.
            keep_secret: false,
            callback_port: Some(8000),
        };
        let kept = stored_from_entered(&form, None, Some(&entry));
        assert!(!kept.secret_from_entry);
        let chosen = preregistered_client(Some(&entry), Some(&kept), Some(GOOGLE)).unwrap();
        assert_eq!(chosen.client_id, "cid");
        assert_eq!(chosen.client_secret, None, "the refused secret came back");
    }

    #[test]
    fn an_entered_client_is_never_presented_to_another_authorization_server() {
        let typed = entered("typed", Some("typed-secret"), Some(GOOGLE));
        // Google's two spellings of itself are one server.
        assert!(
            preregistered_client(None, Some(&typed), Some("https://accounts.google.com/"))
                .is_some()
        );
        // Another issuer, or none discovered, never sees it…
        assert_eq!(
            preregistered_client(None, Some(&typed), Some("https://evil.test")),
            None
        );
        assert_eq!(preregistered_client(None, Some(&typed), None), None);
        // …and the entry's own client then signs in without its secret.
        let entry = config(Some("from-config"), None, None);
        let chosen =
            preregistered_client(Some(&entry), Some(&typed), Some("https://evil.test")).unwrap();
        assert_eq!(chosen.client_id, "from-config");
        assert_eq!(chosen.client_secret, None);
        assert_eq!(chosen.source, ClientSource::Config);
        // One that names no authorization server is nobody's: there is
        // nothing to hold it to, so it is never presented.
        let unbound = entered("typed", None, None);
        assert_eq!(
            preregistered_client(None, Some(&unbound), Some(GOOGLE)),
            None
        );
        assert_eq!(preregistered_client(None, Some(&unbound), None), None);
    }

    #[test]
    fn an_entrys_client_takes_the_secret_kept_for_it() {
        let entry = config(Some("cid"), None, Some(8000));
        let kept = StoredClient {
            client_secret: Some("kept".to_string()),
            ..Default::default()
        };
        let chosen = preregistered_client(Some(&entry), Some(&kept), Some(GOOGLE)).unwrap();
        assert_eq!(chosen.client_id, "cid");
        assert_eq!(chosen.client_secret.as_deref(), Some("kept"));
        assert_eq!(chosen.callback_port, Some(8000));
        assert_eq!(chosen.source, ClientSource::Config);
        // The file's own secret comes first.
        let entry = config(Some("cid"), Some("in-file"), None);
        let chosen = preregistered_client(Some(&entry), Some(&kept), Some(GOOGLE)).unwrap();
        assert_eq!(chosen.client_secret.as_deref(), Some("in-file"));
    }

    #[test]
    fn a_submitted_form_becomes_the_one_client_kept() {
        let submitted = |secret: Option<&str>, keep_secret: bool| EnteredClient {
            issuer: GOOGLE.to_string(),
            client_id: "cid".to_string(),
            client_secret: secret.map(SecretValue::new),
            keep_secret,
            callback_port: Some(8976),
        };
        assert_eq!(
            stored_from_entered(&submitted(Some("typed"), false), None, None),
            StoredClient {
                client_id: Some("cid".to_string()),
                client_secret: Some("typed".to_string()),
                callback_port: Some(8976),
                issuer: Some(GOOGLE.to_string()),
                secret_from_entry: false,
            }
        );
        // The secret a form keeps: its own, and whether it is its entry's.
        let secret_of = |form: &EnteredClient,
                         kept: Option<&StoredClient>,
                         entry: Option<&OAuthClientConfig>| {
            let stored = stored_from_entered(form, kept, entry);
            (stored.client_secret, stored.secret_from_entry)
        };
        let own = |secret: &str| (Some(secret.to_string()), false);
        const PUBLIC: (Option<String>, bool) = (None, false);
        // Left empty with nothing kept: a public client.
        assert_eq!(secret_of(&submitted(None, true), None, None), PUBLIC);
        // Left empty over the same entered client: its secret stays — a
        // provider may show one only once.
        let same = entered("cid", Some("kept"), Some(GOOGLE));
        assert_eq!(
            secret_of(&submitted(None, true), Some(&same), None),
            own("kept")
        );
        // Over another client's: never. That secret is not this client's.
        let other = entered("other", Some("kept"), Some(GOOGLE));
        assert_eq!(
            secret_of(&submitted(None, true), Some(&other), None),
            PUBLIC
        );
        // Over the secret kept for the entry's client, when it is this one…
        let for_entry = StoredClient {
            client_secret: Some("kept".to_string()),
            ..Default::default()
        };
        let entry = config(Some("cid"), None, None);
        assert_eq!(
            secret_of(&submitted(None, true), Some(&for_entry), Some(&entry)),
            own("kept")
        );
        // …and not when the entry names another, or none.
        let another = config(Some("other"), None, None);
        assert_eq!(
            secret_of(&submitted(None, true), Some(&for_entry), Some(&another)),
            PUBLIC
        );
        assert_eq!(
            secret_of(&submitted(None, true), Some(&for_entry), None),
            PUBLIC
        );
        // …nor a client's entered for another authorization server, or for
        // none: a secret is that server's alone, whatever the id.
        let elsewhere = entered("cid", Some("kept"), Some("https://other.test"));
        assert_eq!(
            secret_of(&submitted(None, true), Some(&elsewhere), None),
            PUBLIC
        );
        let unbound = entered("cid", Some("kept"), None);
        assert_eq!(
            secret_of(&submitted(None, true), Some(&unbound), None),
            PUBLIC
        );
        // A typed secret replaces a kept one; and without the say-so an
        // empty field is a public client, whatever is kept.
        assert_eq!(
            secret_of(&submitted(Some("new"), true), Some(&same), None),
            own("new")
        );
        assert_eq!(
            secret_of(&submitted(None, false), Some(&same), None),
            PUBLIC
        );
    }

    /// The secret an entry carries **in its file** is never copied into the
    /// store: a form left empty to keep it records only that the entry's
    /// secret is the one to use, read from the file at each sign-in.
    #[test]
    fn keeping_an_entrys_own_secret_copies_nothing() {
        let keep = EnteredClient {
            issuer: GOOGLE.to_string(),
            client_id: "cid".to_string(),
            client_secret: None,
            keep_secret: true,
            callback_port: Some(9000),
        };
        let in_file = config(Some("cid"), Some("in-file"), None);
        let lent = stored_from_entered(&keep, None, Some(&in_file));
        assert_eq!(lent.client_secret, None, "the file's secret was copied");
        assert!(lent.secret_from_entry);
        let chosen = preregistered_client(Some(&in_file), Some(&lent), Some(GOOGLE)).unwrap();
        assert_eq!(chosen.client_secret.as_deref(), Some("in-file"));
        assert_eq!(chosen.callback_port, Some(9000));
        // The file's secret is the one in effect ahead of one the CLI kept:
        // keeping "the secret already kept" keeps that one.
        let from_cli = StoredClient {
            client_secret: Some("from-cli".to_string()),
            ..Default::default()
        };
        let lent = stored_from_entered(&keep, Some(&from_cli), Some(&in_file));
        assert_eq!((lent.client_secret, lent.secret_from_entry), (None, true));
        // Saved again, still left empty: the say-so stays.
        let again = stored_from_entered(&keep, Some(&lent_client()), Some(&in_file));
        assert_eq!((again.client_secret, again.secret_from_entry), (None, true));
        // The entry's client was the one in effect while a client entered
        // for another authorization server sat unused: the same.
        let elsewhere = entered("cid", Some("theirs"), Some("https://other.test"));
        let lent = stored_from_entered(&keep, Some(&elsewhere), Some(&in_file));
        assert_eq!((lent.client_secret, lent.secret_from_entry), (None, true));
        // A typed secret, or a public client, takes the say-so back.
        let typed = EnteredClient {
            client_secret: Some(SecretValue::new("typed")),
            keep_secret: false,
            ..keep.clone()
        };
        let stored = stored_from_entered(&typed, Some(&lent_client()), Some(&in_file));
        assert_eq!(stored.client_secret.as_deref(), Some("typed"));
        assert!(!stored.secret_from_entry);
        let public = EnteredClient {
            keep_secret: false,
            ..keep
        };
        let stored = stored_from_entered(&public, Some(&lent_client()), Some(&in_file));
        assert_eq!(
            (stored.client_secret, stored.secret_from_entry),
            (None, false)
        );
    }

    /// The entry's own client as a form left empty over it keeps it.
    fn lent_client() -> StoredClient {
        StoredClient {
            secret_from_entry: true,
            ..entered("cid", None, Some(GOOGLE))
        }
    }

    #[test]
    fn nothing_named_is_no_client() {
        assert_eq!(preregistered_client(None, None, Some(GOOGLE)), None);
        // A port alone, or a secret alone, names no client.
        let port_only = config(None, None, Some(8000));
        let kept = StoredClient {
            client_secret: Some("kept".to_string()),
            ..Default::default()
        };
        assert_eq!(
            preregistered_client(Some(&port_only), Some(&kept), Some(GOOGLE)),
            None
        );
    }

    #[test]
    fn the_client_in_effect_says_where_it_came_from() {
        let entry = config(Some("from-config"), None, None);
        assert_eq!(
            client_in_effect(Some(&entry), None),
            Some(McpClientInfo {
                client_id: "from-config".to_string(),
                source: ClientSource::Config
            })
        );
        let typed = entered("typed", None, Some(GOOGLE));
        assert_eq!(
            client_in_effect(Some(&entry), Some(&typed)),
            Some(McpClientInfo {
                client_id: "typed".to_string(),
                source: ClientSource::Entered
            })
        );
        // A secret kept for the entry's client is not a client of its own.
        let kept = StoredClient {
            client_secret: Some("kept".to_string()),
            ..Default::default()
        };
        assert_eq!(
            client_in_effect(Some(&entry), Some(&kept)).unwrap().source,
            ClientSource::Config
        );
        assert_eq!(client_in_effect(None, Some(&kept)), None);
        assert_eq!(client_in_effect(None, None), None);
    }

    #[test]
    fn issuers_match_but_for_one_trailing_slash() {
        assert!(issuers_match(GOOGLE, "https://accounts.google.com/"));
        assert!(issuers_match(
            "https://as.test/realm",
            "https://as.test/realm"
        ));
        assert!(!issuers_match(GOOGLE, "https://accounts.google.com//"));
        assert!(!issuers_match(
            GOOGLE,
            "https://accounts.google.com.evil.test"
        ));
        assert!(!issuers_match(GOOGLE, "http://accounts.google.com"));
    }

    #[test]
    fn an_issuer_is_known_by_its_host() {
        assert_eq!(
            issuer_host("https://accounts.google.com/"),
            "accounts.google.com"
        );
        assert_eq!(issuer_host("https://login.test/realms/acme"), "login.test");
        assert_eq!(issuer_host("http://127.0.0.1:8123"), "127.0.0.1:8123");
        assert_eq!(issuer_host("not a url"), "not a url");
    }

    #[test]
    fn the_redirect_names_its_port_or_stands_in_for_it() {
        assert_eq!(
            callback_redirect(Some(8976)),
            "http://localhost:8976/callback"
        );
        assert_eq!(callback_redirect(None), "http://localhost:PORT/callback");
    }

    #[test]
    fn a_callback_port_is_empty_or_a_port_number() {
        assert_eq!(parse_callback_port(""), Ok(None));
        assert_eq!(parse_callback_port("  "), Ok(None));
        assert_eq!(parse_callback_port(" 8976 "), Ok(Some(8976)));
        assert_eq!(parse_callback_port("65535"), Ok(Some(65535)));
        for bad in ["0", "65536", "80a", "-1", "8 0"] {
            assert!(parse_callback_port(bad).is_err(), "{bad}");
        }
    }
}
