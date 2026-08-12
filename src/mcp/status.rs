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
}

impl McpServerStatus {
    /// The list row's status glyph — Claude Code's exact vocabulary.
    #[must_use]
    pub const fn glyph(&self) -> &'static str {
        match self {
            Self::Pending | Self::Disabled => "◯",
            Self::Connected => "✔",
            Self::NeedsAuth => "△",
            Self::Failed(_) => "✘",
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
        }
    }
}

/// A server's OAuth standing — the detail page's `Auth:` row. `None` hides
/// the row (a stdio server has no auth story).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpAuthState {
    /// Stored OAuth tokens exist for this server.
    Authenticated,
    /// A remote server with no stored tokens.
    NotAuthenticated,
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
