//! What the `/mcp` manager and the MCP tool cells consume (`docs/mcp.md`):
//! the per-server snapshot the boundary injects into `App`, the status
//! glyph/label vocabulary, the tool-detail parameter listing, and the
//! pretty-printed argument forms the cell headers derive at render time.

use serde_json::Value;

use super::config::{McpScope, McpServerConfig};
use super::protocol::{ExcludedTool, McpToolInfo, ServerIdentity};

/// One server's live state, as the UI sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpServerStatus {
    /// Connecting/authenticating in the background — the row shows
    /// `connecting…`.
    Pending,
    /// Initialized, tools listed.
    Connected,
    /// The server wants OAuth before it will talk.
    NeedsAuth,
    /// The connect failed; the reason rides along.
    Failed(String),
    /// Disabled here (per project) — never connected.
    Disabled,
    /// Declared by the project and waiting for the startup review's answer
    /// (`docs/config-review.md`) — never launched until enabled there.
    AwaitingReview,
    /// MCP is off for the whole session — the `/settings` **MCP** row
    /// (`docs/mcp.md`): not connected, and connected again once the row is
    /// back on. The server's own `/mcp` switch is a separate thing and
    /// keeps its state.
    Off,
}

impl McpServerStatus {
    /// The list row's status glyph — Claude Code's exact vocabulary (the
    /// review's `⚠` is ours).
    #[must_use]
    pub const fn glyph(&self) -> &'static str {
        match self {
            Self::Pending | Self::Disabled | Self::Off => "◯",
            Self::Connected => "✔",
            Self::NeedsAuth => "△",
            Self::Failed(_) => "✘",
            Self::AwaitingReview => "⚠",
        }
    }

    /// The list row's status text (the tool count is appended by the row
    /// builder when connected).
    #[must_use]
    pub const fn label(&self) -> &'static str {
        match self {
            Self::Pending => "connecting…",
            Self::Connected => "connected",
            Self::NeedsAuth => "needs authentication",
            Self::Failed(_) => "failed",
            Self::Disabled => "disabled",
            Self::AwaitingReview => "awaiting review",
            Self::Off => "off",
        }
    }
}

/// A server's OAuth standing — the detail page's `Auth:` row. `None` hides
/// the row entirely, which only a stdio server earns: there is no remote
/// server to authenticate *to*.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpAuthState {
    /// A usable OAuth grant is stored for this server.
    Authenticated,
    /// The config carries its own `Authorization` header — a written-down
    /// token, so there is nothing to log into and nothing to clear.
    Header,
    /// Connected while presenting no credentials at all: the server never
    /// challenged us. A public server, not a failed login.
    NotRequired,
    /// A grant is stored but the server is refusing it — the honest reading
    /// of tokens plus a needs-auth status.
    Expired,
    /// A remote server with no stored tokens that isn't serving.
    NotAuthenticated,
}

impl McpAuthState {
    /// The `Auth:` row's glyph — the coloured half of the row's two-tone
    /// ([`McpServerStatus::glyph`]'s job, for the same reason: the detail
    /// page colours the glyph by state and prints the words plainly, and a
    /// renderer must never have to split a string it didn't build).
    #[must_use]
    pub const fn glyph(self) -> &'static str {
        match self {
            Self::Authenticated | Self::Header | Self::NotRequired => "✔",
            Self::Expired | Self::NotAuthenticated => "✘",
        }
    }

    /// The `Auth:` row's words. Only a state that wants the user to *do*
    /// something wears the red `✘` ([`glyph`](Self::glyph)).
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Authenticated => "authenticated",
            Self::Header => "authenticated (config header)",
            // Shown, never hidden: whether a login is wanted here is the
            // question the row exists to answer, and silence leaves the user
            // wondering. A server that never challenged us reports the same
            // settled row a stored grant earns, because from the user's side
            // the answer is the same — you are cleared to use it — and a
            // `not needed` shrug beside `✔ connected` read as a caveat where
            // there is none. The *state* stays distinct: it is what
            // withholds the re-authenticate and clear actions
            // (`docs/mcp.md`).
            Self::NotRequired => "authenticated",
            Self::Expired => "expired",
            Self::NotAuthenticated => "not authenticated",
        }
    }

    /// Is this a state the user should act on (red), rather than a settled
    /// fact (green) or a non-issue (dim)?
    #[must_use]
    pub const fn is_problem(self) -> bool {
        matches!(self, Self::Expired | Self::NotAuthenticated)
    }

    /// Is an OAuth grant actually stored — the only thing "re-authenticate"
    /// and "clear authentication" have to work with?
    #[must_use]
    pub const fn has_grant(self) -> bool {
        matches!(self, Self::Authenticated | Self::Expired)
    }
}

/// The `Auth:` row a server's state affords — `None` only for stdio.
///
/// The rules, in order: a stdio child has no auth story at all; a configured
/// `Authorization` header outranks a stored grant, because the transport
/// does the same (it sends the header and never the bearer, so reporting the
/// grant would offer to re-run and clear a login the server never sees);
/// tokens the server is refusing read `Expired` rather than a login that
/// plainly is not working; tokens otherwise read `Authenticated`; and a
/// server that *connected* presenting nothing was never challenged, so it
/// reads `NotRequired` instead of `✘ not authenticated` beside `✔ connected`
/// — the reported lie about a connection with nothing wrong with it.
#[must_use]
pub fn auth_state(
    is_remote: bool,
    has_header: bool,
    has_tokens: bool,
    status: &McpServerStatus,
) -> Option<McpAuthState> {
    if !is_remote {
        return None;
    }
    if has_header {
        return Some(McpAuthState::Header);
    }
    Some(match (has_tokens, status) {
        (true, McpServerStatus::NeedsAuth) => McpAuthState::Expired,
        (true, _) => McpAuthState::Authenticated,
        (false, McpServerStatus::Connected) => McpAuthState::NotRequired,
        (false, _) => McpAuthState::NotAuthenticated,
    })
}

/// One server, snapshotted whole for the UI — pure data, injected at the
/// boundary (`App::set_mcp_snapshot`) like the clock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServerSnapshot {
    pub name: String,
    pub scope: McpScope,
    /// The config file the entry came from (the `Config location:` row).
    pub config_path: String,
    pub config: McpServerConfig,
    pub status: McpServerStatus,
    /// `None` for stdio (no auth row); `Some` for remote servers.
    pub auth: Option<McpAuthState>,
    /// The server's identity once connected.
    pub identity: Option<ServerIdentity>,
    /// The tools the server listed, in the server's order.
    pub tools: Vec<McpToolInfo>,
    /// The tools it listed that the model is not offered, and why.
    pub excluded: Vec<ExcludedTool>,
    /// The id of the OAuth client entered for this server on the `/mcp`
    /// auth page and kept with its tokens (`docs/mcp.md` *Pre-registered
    /// clients*) — it outranks the entry's own `oauth.clientId`, which is in
    /// `config`. `None` when none was entered.
    pub kept_client: Option<String>,
}

/// Where the OAuth client a server signs in as comes from
/// ([`McpServerSnapshot::oauth_client`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpClientSource {
    /// Entered on the `/mcp` auth page and kept with the tokens.
    Entered,
    /// The server entry's own `oauth.clientId`.
    Entry,
}

impl McpServerSnapshot {
    /// The OAuth client a sign-in presents, and where it is from: one
    /// entered in `/mcp`, which outranks the entry's own, else the entry's
    /// `oauth.clientId` — what the `OAuth client:` row names and the
    /// Change/Forget rows work on. Which authorization server the
    /// sign-in will find is not known here, so an entered client reads as
    /// the one in effect whatever it was entered for.
    #[must_use]
    pub fn oauth_client(&self) -> Option<(&str, McpClientSource)> {
        if let Some(id) = &self.kept_client {
            return Some((id, McpClientSource::Entered));
        }
        self.config
            .oauth()?
            .client_id
            .as_deref()
            .map(|id| (id, McpClientSource::Entry))
    }

    /// The list row's full status text: `✔ connected · 3 tools`,
    /// `△ needs authentication`, …
    #[must_use]
    pub fn status_line(&self) -> String {
        let base = format!("{} {}", self.status.glyph(), self.status.label());
        if self.status == McpServerStatus::Connected {
            let n = self.tools.len();
            let noun = if n == 1 { "tool" } else { "tools" };
            format!("{base} · {n} {noun}")
        } else {
            base
        }
    }
}

/// What a sign-in asks the user for when it has no OAuth client to present,
/// or the one it presented was turned away — the spec's last registration
/// step, "prompt the user to enter the client information" (`docs/mcp.md`
/// *Pre-registered clients*). The `/mcp` auth page turns into the client
/// form over it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct McpClientRequest {
    /// The authorization server the client is created with — its `issuer`,
    /// else the origin of its token endpoint: who the page names, and what
    /// an entered client is bound to. Empty when no sign-in has seen the
    /// server yet (the form opened from the server's page).
    pub issuer: String,
    /// The client id already known — the server entry's own (`fixed_id`),
    /// or the one entered before: the form opens with it.
    pub client_id: Option<String>,
    /// `client_id` is the server entry's `oauth.clientId`: the entry names
    /// the client, so only its secret is asked for.
    pub fixed_id: bool,
    /// A secret is already kept for `client_id` — an empty secret field
    /// keeps it.
    pub has_secret: bool,
    /// The port the sign-in listens on, so the redirect URI to register is
    /// [`oauth_redirect_uri`] of it. `None` when the sign-in takes any free
    /// port (a configured client whose entry names no `callbackPort`).
    pub callback_port: Option<u16>,
    /// The redirect URI the entry's own client registered, where its entry
    /// names one (`oauth.callbackUrl`) — what the form shows in place of
    /// [`oauth_redirect_uri`] of the port. `None` otherwise.
    pub redirect_uri: Option<String>,
    /// The documentation link the server's metadata gives — the one thing
    /// it says about itself: the authorization server's
    /// `service_documentation`, else the resource's `resource_documentation`.
    /// `None` where no discovery ran, or it gave none worth linking.
    pub documentation: Option<String>,
    /// Why the client on hand was not enough — turned away by the
    /// authorization server, its port taken — in words for the page.
    pub reason: Option<String>,
    /// The server entry's own client, when it names one: what a form whose
    /// id can change needs to tell, from what is typed, whether it still
    /// describes that client.
    pub entry: Option<McpEntryClient>,
}

/// A server entry's own OAuth client as the client form needs it
/// (`docs/mcp.md` *The client form*): typed back as its id on its exact
/// port — both empty counting as one — the form describes that client, and
/// the sign-in after it sends that client's own redirect URI.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct McpEntryClient {
    /// `oauth.clientId`.
    pub client_id: String,
    /// The port its sign-in listens on exactly; `None` takes any free port.
    pub port: Option<u16>,
    /// The redirect URI its sign-in sends — its `callbackUrl`, else the
    /// standard one on its port; `None` where it takes any free port.
    pub redirect_uri: Option<String>,
    /// A secret is in effect for it — one kept for it with the tokens, else
    /// its file's — so an empty secret field leaves that one in effect.
    pub has_secret: bool,
}

/// The redirect URI a sign-in listening on `port` sends — and so the one a
/// provider that matches redirect URIs exactly must have registered. One
/// spelling, read by the flow and by the client form alike, so what the page
/// shows is what the authorization server is sent.
#[must_use]
pub fn oauth_redirect_uri(port: u16) -> String {
    format!("http://localhost:{port}/callback")
}

/// An issuer's host — `accounts.google.com` for
/// `https://accounts.google.com` — the name a person knows a provider by.
/// An issuer that is no URL reads as written.
#[must_use]
pub fn issuer_host(issuer: &str) -> String {
    url::Url::parse(issuer)
        .ok()
        .and_then(|url| url.host_str().map(str::to_string))
        .unwrap_or_else(|| issuer.to_string())
}

/// An issuer as the client form shows it: exactly as given, except that a
/// URL carrying a user name or password is shown without them — so
/// `https://accounts.google.com@evil.example` reads `https://evil.example/`,
/// the host its token endpoint is really on. Text that is no URL is shown as
/// written, and an ordinary issuer is never normalised.
#[must_use]
pub fn issuer_display(issuer: &str) -> String {
    match url::Url::parse(issuer) {
        Ok(mut url) if !url.username().is_empty() || url.password().is_some() => {
            let _ = url.set_username("");
            let _ = url.set_password(None);
            url.to_string()
        }
        _ => issuer.to_string(),
    }
}

/// One `Parameters:` row of the tool detail page, derived from the tool's
/// input schema: `{name} ({required|optional}): {type} - {description}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolParameter {
    pub name: String,
    pub required: bool,
    pub kind: String,
    pub description: String,
}

/// Derive the parameter listing from an `input_schema`. Order: required
/// parameters first in schema-`required` order, then the rest alphabetically
/// — the schema map itself is alphabetized by the JSON parse, so the
/// `required` array is the only order the author actually chose.
#[must_use]
pub fn tool_parameters(schema: &Value) -> Vec<ToolParameter> {
    let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
        return Vec::new();
    };
    let required: Vec<&str> = schema
        .get("required")
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let param = |name: &str, value: &Value| ToolParameter {
        name: name.to_string(),
        required: required.contains(&name),
        kind: value
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string(),
        description: value
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
    };
    let mut out: Vec<ToolParameter> = Vec::new();
    for name in &required {
        if let Some(value) = properties.get(*name) {
            out.push(param(name, value));
        }
    }
    for (name, value) in properties {
        if !required.contains(&name.as_str()) {
            out.push(param(name, value));
        }
    }
    out
}

/// Pretty-print a call's raw arguments JSON for the cell header:
/// `repoName: "linuztx/flaredantic", question: "What is…"` — Claude Code's
/// `key: json(value)` pairs. Non-object (or non-JSON) arguments render
/// verbatim, so an old record still shows *something*.
#[must_use]
pub fn pretty_args(arguments: &str) -> String {
    let Ok(Value::Object(map)) = serde_json::from_str::<Value>(arguments.trim()) else {
        return arguments.trim().to_string();
    };
    map.iter()
        .map(|(key, value)| {
            let rendered = serde_json::to_string(value).unwrap_or_else(|_| "null".to_string());
            format!("{key}: {rendered}")
        })
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::BTreeMap;

    fn snapshot(status: McpServerStatus, tools: usize) -> McpServerSnapshot {
        McpServerSnapshot {
            name: "deepwiki".to_string(),
            scope: McpScope::User,
            config_path: "~/.alter-zero/mcp.json".to_string(),
            config: McpServerConfig::Http {
                url: "https://mcp.deepwiki.com/mcp".to_string(),
                headers: BTreeMap::new(),
                sse_fallback: false,
                oauth: None,
            },
            status,
            auth: Some(McpAuthState::NotAuthenticated),
            identity: None,
            excluded: Vec::new(),
            kept_client: None,
            tools: (0..tools)
                .map(|i| McpToolInfo {
                    name: format!("t{i}"),
                    description: String::new(),
                    input_schema: json!({"type": "object", "properties": {}}),
                })
                .collect(),
        }
    }

    /// The OAuth client a server's page names: one entered in `/mcp`, which
    /// outranks the entry's own, else the entry's — and where it is from.
    #[test]
    fn the_client_in_effect_is_the_entered_one_else_the_entrys() {
        let mut server = snapshot(McpServerStatus::NeedsAuth, 0);
        assert_eq!(server.oauth_client(), None);
        server.config = McpServerConfig::Http {
            url: "https://mcp.deepwiki.com/mcp".to_string(),
            headers: BTreeMap::new(),
            sse_fallback: false,
            oauth: Some(crate::mcp::OAuthClientConfig {
                client_id: Some("file-cid".to_string()),
                ..Default::default()
            }),
        };
        assert_eq!(
            server.oauth_client(),
            Some(("file-cid", McpClientSource::Entry))
        );
        server.kept_client = Some("typed-cid".to_string());
        assert_eq!(
            server.oauth_client(),
            Some(("typed-cid", McpClientSource::Entered))
        );
    }

    #[test]
    fn status_lines_match_the_reference_vocabulary() {
        assert_eq!(
            snapshot(McpServerStatus::Connected, 3).status_line(),
            "✔ connected · 3 tools"
        );
        assert_eq!(
            snapshot(McpServerStatus::Connected, 1).status_line(),
            "✔ connected · 1 tool"
        );
        assert_eq!(
            snapshot(McpServerStatus::NeedsAuth, 0).status_line(),
            "△ needs authentication"
        );
        assert_eq!(
            snapshot(McpServerStatus::Disabled, 0).status_line(),
            "◯ disabled"
        );
        assert_eq!(
            snapshot(McpServerStatus::Failed("x".to_string()), 0).status_line(),
            "✘ failed"
        );
        assert_eq!(
            snapshot(McpServerStatus::Pending, 0).status_line(),
            "◯ connecting…"
        );
        // The startup review's holding state (`docs/config-review.md`):
        // declared by the project, never launched, waiting for an answer.
        assert_eq!(
            snapshot(McpServerStatus::AwaitingReview, 0).status_line(),
            "⚠ awaiting review"
        );
        // MCP is off for the whole session (the `/settings` **MCP** row):
        // not connected, and connected again once the row is back on.
        assert_eq!(snapshot(McpServerStatus::Off, 0).status_line(), "◯ off");
    }

    #[test]
    fn auth_rows_tell_the_five_truths_apart() {
        use McpServerStatus as S;
        // A stdio server has no auth story at all — there is no server to
        // authenticate *to*, so no row.
        assert_eq!(auth_state(false, false, false, &S::Connected), None);
        assert_eq!(auth_state(false, false, true, &S::Connected), None);
        // A remote server that connected presenting nothing was never
        // challenged: say so plainly. `✘ not authenticated` beside
        // `✔ connected` reads as a problem where there is none (deepwiki).
        assert_eq!(
            auth_state(true, false, false, &S::Connected),
            Some(McpAuthState::NotRequired)
        );
        // Stored tokens: the row reports the grant.
        assert_eq!(
            auth_state(true, false, true, &S::Connected),
            Some(McpAuthState::Authenticated)
        );
        assert_eq!(
            auth_state(true, false, true, &S::Failed("x".to_string())),
            Some(McpAuthState::Authenticated)
        );
        // Tokens *plus* a server refusing them is the honest reading of a
        // dead grant — not "authenticated", and not "never logged in".
        assert_eq!(
            auth_state(true, false, true, &S::NeedsAuth),
            Some(McpAuthState::Expired)
        );
        // No tokens and not connected: nothing to show but the truth.
        assert_eq!(
            auth_state(true, false, false, &S::NeedsAuth),
            Some(McpAuthState::NotAuthenticated)
        );
        assert_eq!(
            auth_state(true, false, false, &S::Pending),
            Some(McpAuthState::NotAuthenticated)
        );
        // A written-down `Authorization` header outranks everything: the
        // transport sends it and never the bearer, so reporting a stored
        // grant would offer to re-run and clear a login the server never
        // sees.
        assert_eq!(
            auth_state(true, true, false, &S::Connected),
            Some(McpAuthState::Header)
        );
        assert_eq!(
            auth_state(true, true, true, &S::NeedsAuth),
            Some(McpAuthState::Header)
        );
    }

    #[test]
    fn an_auth_row_splits_into_a_coloured_glyph_and_plain_words() {
        // The `McpServerStatus` shape, for the same reason: the detail page
        // two-tones the row (glyph in the state's colour, words white), and
        // a renderer must never have to split a string it didn't build.
        assert_eq!(McpAuthState::Authenticated.label(), "authenticated");
        assert_eq!(
            McpAuthState::Header.label(),
            "authenticated (config header)"
        );
        assert_eq!(McpAuthState::Expired.label(), "expired");
        assert_eq!(McpAuthState::NotAuthenticated.label(), "not authenticated");
        // A server that never asked for credentials is one you are cleared
        // to use, so it reports the settled row rather than a `not needed`
        // that reads as a shrug. The *state* stays distinct — it is what
        // withholds the re-authenticate/clear actions.
        assert_eq!(McpAuthState::NotRequired.label(), "authenticated");
    }

    #[test]
    fn parameters_derive_required_first_then_the_rest() {
        let params = tool_parameters(&json!({
            "type": "object",
            "properties": {
                "question": {"type": "string", "description": "The question."},
                "repoName": {"description": "owner/repo"},
                "limit": {"type": "number"}
            },
            "required": ["repoName", "question"]
        }));
        let names: Vec<&str> = params.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["repoName", "question", "limit"]);
        assert!(params[0].required);
        assert_eq!(params[0].kind, "unknown"); // no "type" on repoName
        assert_eq!(params[0].description, "owner/repo");
        assert!(!params[2].required);
        assert_eq!(params[2].kind, "number");
        assert!(tool_parameters(&json!({"type": "object"})).is_empty());
    }

    #[test]
    fn pretty_args_render_key_value_pairs_in_the_models_own_order() {
        // The model chose the order it emitted the arguments in — a schema's
        // `repoName` before the long `question` it qualifies — so the header
        // keeps it instead of re-sorting alphabetically (`docs/mcp.md`).
        assert_eq!(
            pretty_args(r#"{"repoName":"a/b","question":"What?"}"#),
            r#"repoName: "a/b", question: "What?""#
        );
        assert_eq!(
            pretty_args(r#"{"n": 3, "deep": {"a": 1}}"#),
            r#"n: 3, deep: {"a":1}"#
        );
        // Non-JSON falls back verbatim (an old record, a truncated string).
        assert_eq!(pretty_args("not json"), "not json");
        assert_eq!(pretty_args("{}"), "");
    }

    /// The redirect URI is spelled one way — `localhost`, the port, the
    /// `/callback` path — so the one the client form shows is the one the
    /// sign-in sends.
    #[test]
    fn the_redirect_uri_names_localhost_the_port_and_the_callback_path() {
        assert_eq!(oauth_redirect_uri(8976), "http://localhost:8976/callback");
        assert_eq!(oauth_redirect_uri(1), "http://localhost:1/callback");
    }

    /// A provider is named by its host, the name a person knows it by; an
    /// issuer that is no URL reads as written.
    #[test]
    fn an_issuer_is_shown_whole_without_user_info() {
        for ordinary in [
            "https://accounts.google.com",
            "https://accounts.google.com/",
            "https://login.example.com:8443/tenant/v2.0",
            "HTTPS://Login.Example.com/Tenant",
        ] {
            assert_eq!(issuer_display(ordinary), ordinary);
        }
        assert_eq!(
            issuer_display("https://accounts.google.com@evil.example"),
            "https://evil.example/"
        );
        assert_eq!(
            issuer_display("https://accounts.google.com:pw@evil.example/tenant"),
            "https://evil.example/tenant"
        );
        assert_eq!(
            issuer_display("https://:secret@evil.example"),
            "https://evil.example/"
        );
        assert_eq!(issuer_display("not a url"), "not a url");
    }

    #[test]
    fn an_issuer_is_named_by_its_host() {
        assert_eq!(
            issuer_host("https://accounts.google.com"),
            "accounts.google.com"
        );
        assert_eq!(
            issuer_host("https://login.example.com:8443/tenant/v2.0"),
            "login.example.com"
        );
        // A name before an `@` is credentials, not the host: the page names
        // who the client is really created with.
        assert_eq!(
            issuer_host("https://accounts.google.com@evil.example/"),
            "evil.example"
        );
        assert_eq!(
            issuer_host("https://accounts.google.com.evil.example"),
            "accounts.google.com.evil.example"
        );
        assert_eq!(issuer_host("not a url"), "not a url");
        assert_eq!(issuer_host(""), "");
    }
}
