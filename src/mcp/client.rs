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
}

impl StoredClient {
    /// Read one `clients` entry — `None` for one that keeps nothing.
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
        let client = Self {
            client_id: text("client_id"),
            client_secret: text("client_secret"),
            callback_port: entry
                .get("callback_port")
                .and_then(Value::as_u64)
                .and_then(|port| u16::try_from(port).ok())
                .filter(|port| *port != 0),
            issuer: text("issuer"),
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
    pub source: ClientSource,
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
/// it was entered for, so another issuer never sees it. Short of its own, it
/// takes the entry's secret and port when it is the entry's own client.
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
    let configured_id = configured.and_then(|config| config.client_id.as_deref());
    let configured_port = configured.and_then(|config| config.callback_port);
    let entered = stored.filter(|client| {
        client.is_entered()
            && client
                .issuer
                .as_deref()
                .is_none_or(|bound| issuer.is_some_and(|issuer| issuers_match(bound, issuer)))
    });
    if let Some(client) = entered
        && let Some(client_id) = &client.client_id
    {
        let same_client = configured_id == Some(client_id.as_str());
        return Some(PreRegistered {
            client_id: client_id.clone(),
            client_secret: client.client_secret.clone().or_else(|| {
                configured
                    .filter(|_| same_client)
                    .and_then(|config| config.client_secret.clone())
            }),
            callback_port: client.callback_port.or(configured_port),
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
/// server it was entered for, its port, and its secret. A secret field left
/// empty over a client whose secret is kept (`keep_secret`) keeps that
/// secret, when it is **this client's** — the same client entered for the
/// same authorization server, or the one kept for the entry's client
/// (`configured`) when the entry names this one — and no other's.
#[must_use]
pub fn stored_from_entered(
    entered: &EnteredClient,
    kept: Option<&StoredClient>,
    configured: Option<&OAuthClientConfig>,
) -> StoredClient {
    let this_client = |client: &&StoredClient| match &client.client_id {
        Some(id) => {
            *id == entered.client_id
                && client
                    .issuer
                    .as_deref()
                    .is_none_or(|issuer| issuers_match(issuer, &entered.issuer))
        }
        None => {
            configured.and_then(|config| config.client_id.as_deref())
                == Some(entered.client_id.as_str())
        }
    };
    let client_secret = match &entered.client_secret {
        Some(secret) => Some(secret.expose().to_string()),
        None if entered.keep_secret => kept
            .filter(this_client)
            .and_then(|client| client.client_secret.clone()),
        None => None,
    };
    StoredClient {
        client_id: Some(entered.client_id.clone()),
        client_secret,
        callback_port: entered.callback_port,
        issuer: Some(entered.issuer.clone()),
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
        }
    }

    fn entered(id: &str, secret: Option<&str>, issuer: Option<&str>) -> StoredClient {
        StoredClient {
            client_id: Some(id.to_string()),
            client_secret: secret.map(str::to_string),
            callback_port: None,
            issuer: issuer.map(str::to_string),
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
        };
        assert_eq!(
            whole.to_json(),
            json!({"client_id": "cid", "client_secret": "shh", "issuer": GOOGLE, "callback_port": 8976})
        );
        assert_eq!(StoredClient::from_json(&whole.to_json()), Some(whole));
        // A public client entered with nothing else is its id alone.
        let bare = entered("cid", None, None);
        assert_eq!(bare.to_json(), json!({"client_id": "cid"}));
        assert_eq!(StoredClient::from_json(&bare.to_json()), Some(bare));
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
            StoredClient::from_json(&json!({"client_id": "cid", "callback_port": 0}))
                .unwrap()
                .callback_port,
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
        // Another client's secret is never lent to it, but the entry's port
        // holds where it named none: the user may have had to allow-list it.
        let bare = entered("typed", None, Some(GOOGLE));
        let chosen = preregistered_client(Some(&entry), Some(&bare), Some(GOOGLE)).unwrap();
        assert_eq!(chosen.client_secret, None);
        assert_eq!(chosen.callback_port, Some(8000));
    }

    #[test]
    fn an_entered_client_that_is_the_entrys_own_takes_its_secret() {
        // `Change OAuth client` over a configured client, to give it a port:
        // the secret its entry carries still counts.
        let entry = config(Some("cid"), Some("config-secret"), None);
        let typed = StoredClient {
            callback_port: Some(9000),
            ..entered("cid", None, Some(GOOGLE))
        };
        let chosen = preregistered_client(Some(&entry), Some(&typed), Some(GOOGLE)).unwrap();
        assert_eq!(chosen.client_secret.as_deref(), Some("config-secret"));
        assert_eq!(chosen.callback_port, Some(9000));
        assert_eq!(chosen.source, ClientSource::Entered);
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
        // One entered before the issuer was recorded is used as it is.
        let unbound = entered("typed", None, None);
        assert!(preregistered_client(None, Some(&unbound), None).is_some());
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
            }
        );
        let secret_of = |form: &EnteredClient,
                         kept: Option<&StoredClient>,
                         entry: Option<&OAuthClientConfig>| {
            stored_from_entered(form, kept, entry).client_secret
        };
        // Left empty with nothing kept: a public client.
        assert_eq!(secret_of(&submitted(None, true), None, None), None);
        // Left empty over the same entered client: its secret stays — a
        // provider may show one only once.
        let same = entered("cid", Some("kept"), Some(GOOGLE));
        assert_eq!(
            secret_of(&submitted(None, true), Some(&same), None).as_deref(),
            Some("kept")
        );
        // Over another client's: never. That secret is not this client's.
        let other = entered("other", Some("kept"), Some(GOOGLE));
        assert_eq!(secret_of(&submitted(None, true), Some(&other), None), None);
        // Over the secret kept for the entry's client, when it is this one…
        let for_entry = StoredClient {
            client_secret: Some("kept".to_string()),
            ..Default::default()
        };
        let entry = config(Some("cid"), None, None);
        assert_eq!(
            secret_of(&submitted(None, true), Some(&for_entry), Some(&entry)).as_deref(),
            Some("kept")
        );
        // …and not when the entry names another, or none.
        let another = config(Some("other"), None, None);
        assert_eq!(
            secret_of(&submitted(None, true), Some(&for_entry), Some(&another)),
            None
        );
        assert_eq!(
            secret_of(&submitted(None, true), Some(&for_entry), None),
            None
        );
        // …nor a client's entered for another authorization server: a
        // secret is that server's alone, whatever the id.
        let elsewhere = entered("cid", Some("kept"), Some("https://other.test"));
        assert_eq!(
            secret_of(&submitted(None, true), Some(&elsewhere), None),
            None
        );
        // A typed secret replaces a kept one; and without the say-so an
        // empty field is a public client, whatever is kept.
        assert_eq!(
            secret_of(&submitted(Some("new"), true), Some(&same), None).as_deref(),
            Some("new")
        );
        assert_eq!(secret_of(&submitted(None, false), Some(&same), None), None);
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
