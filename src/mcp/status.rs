//! What the `/mcp` manager and the MCP tool cells consume (`docs/mcp.md`):
//! the per-server snapshot the boundary injects into `App`, the status
//! glyph/label vocabulary, the tool-detail parameter listing, and the
//! pretty-printed argument forms the cell headers derive at render time.

use serde_json::Value;

use super::config::{McpScope, McpServerConfig};
use super::protocol::{McpToolInfo, ServerIdentity};

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
    /// Declared by the project but not yet trusted (`docs/project-config.md`)
    /// — never launched; `/trust` is the way in.
    Untrusted,
}

impl McpServerStatus {
    /// The list row's status glyph — Claude Code's exact vocabulary (the
    /// trust gate's `⚠` is ours).
    #[must_use]
    pub const fn glyph(&self) -> &'static str {
        match self {
            Self::Pending | Self::Disabled => "◯",
            Self::Connected => "✔",
            Self::NeedsAuth => "△",
            Self::Failed(_) => "✘",
            Self::Untrusted => "⚠",
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
            Self::Untrusted => "untrusted",
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
    /// The `Auth:` row's text. Only a state that wants the user to *do*
    /// something wears the red `✘`.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Authenticated => "✔ authenticated",
            Self::Header => "✔ authenticated (config header)",
            // Shown, never hidden: "this server needs no login" is the
            // answer to the question the row exists to ask, and silence
            // leaves the user wondering (`docs/mcp.md`).
            Self::NotRequired => "◯ not needed",
            Self::Expired => "✘ expired",
            Self::NotAuthenticated => "✘ not authenticated",
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
}

impl McpServerSnapshot {
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
            },
            status,
            auth: Some(McpAuthState::NotAuthenticated),
            identity: None,
            tools: (0..tools)
                .map(|i| McpToolInfo {
                    name: format!("t{i}"),
                    description: String::new(),
                    input_schema: json!({"type": "object", "properties": {}}),
                })
                .collect(),
        }
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
        // The project-config trust gate's holding state
        // (`docs/project-config.md`): declared by the project, never
        // launched, waiting on `/trust`.
        assert_eq!(
            snapshot(McpServerStatus::Untrusted, 0).status_line(),
            "⚠ untrusted"
        );
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
}
