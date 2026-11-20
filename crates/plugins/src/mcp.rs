//! A plugin MCP server's transport, as plain data (`docs/plugins.md` *What
//! a plugin adds*). The app converts a [`ServerTransport`] into its own
//! `mcp::McpServerConfig`; this crate depends on nothing of the app's, so it
//! reads a legacy (`.mcp.json`) entry itself, by the same rules the app's
//! own config parser applies: `type` optional — a `command` is stdio, a
//! `url` alone is streamable HTTP with the legacy-SSE fallback armed —
//! `http` / `streamable-http` / `streamableHttp` / Codex's `streamable_http`
//! and `sse` named, unknown keys ignored.

use std::collections::BTreeMap;

use serde_json::value::RawValue;

/// How a server is reached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerTransport {
    /// A local child speaking JSON-RPC on its stdio.
    Stdio {
        /// The executable.
        command: String,
        /// Its arguments.
        args: Vec<String>,
        /// Its environment overlay.
        env: BTreeMap<String, String>,
    },
    /// Streamable HTTP; `sse_fallback` when the entry named no `type` (try
    /// streamable HTTP, then the same URL as legacy SSE).
    Http {
        /// The endpoint.
        url: String,
        /// Fixed headers.
        headers: BTreeMap<String, String>,
        /// Retry as legacy SSE when the first attempt is refused.
        sse_fallback: bool,
    },
    /// The legacy HTTP+SSE transport.
    Sse {
        /// The endpoint.
        url: String,
        /// Fixed headers.
        headers: BTreeMap<String, String>,
    },
}

impl ServerTransport {
    /// The one-line target: the URL, or the command line.
    #[must_use]
    pub fn target(&self) -> String {
        match self {
            Self::Stdio { command, args, .. } if args.is_empty() => command.clone(),
            Self::Stdio { command, args, .. } => format!("{command} {}", args.join(" ")),
            Self::Http { url, .. } | Self::Sse { url, .. } => url.clone(),
        }
    }

    /// The remote URL, `None` for stdio.
    #[must_use]
    pub fn url(&self) -> Option<&str> {
        match self {
            Self::Stdio { .. } => None,
            Self::Http { url, .. } | Self::Sse { url, .. } => Some(url),
        }
    }

    /// Every string a launch reads mapped through `f`: a stdio server's
    /// command, arguments and environment values; a remote one's URL and
    /// header values. Names and keys never.
    #[must_use]
    pub fn map_values(self, f: &dyn Fn(&str) -> String) -> Self {
        let map = |m: BTreeMap<String, String>| m.into_iter().map(|(k, v)| (k, f(&v))).collect();
        match self {
            Self::Stdio { command, args, env } => Self::Stdio {
                command: f(&command),
                args: args.iter().map(|a| f(a)).collect(),
                env: map(env),
            },
            Self::Http {
                url,
                headers,
                sse_fallback,
            } => Self::Http {
                url: f(&url),
                headers: map(headers),
                sse_fallback,
            },
            Self::Sse { url, headers } => Self::Sse {
                url: f(&url),
                headers: map(headers),
            },
        }
    }
}

/// A legacy entry's fields, each kept as text until read.
#[derive(serde::Deserialize)]
struct Entry<'a> {
    #[serde(default, rename = "type", borrow)]
    kind: Option<&'a RawValue>,
    #[serde(default, borrow)]
    command: Option<&'a RawValue>,
    #[serde(default, borrow)]
    args: Option<&'a RawValue>,
    #[serde(default, borrow)]
    env: Option<&'a RawValue>,
    #[serde(default, borrow)]
    url: Option<&'a RawValue>,
    #[serde(default, borrow)]
    headers: Option<&'a RawValue>,
    #[serde(default, borrow)]
    oauth: Option<&'a RawValue>,
    #[serde(default, borrow)]
    cwd: Option<&'a RawValue>,
}

/// One legacy server entry, read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyEntry {
    /// The transport, nothing expanded yet.
    pub transport: ServerTransport,
    /// `cwd`, as written.
    pub cwd: Option<String>,
    /// The `oauth` object's JSON text, for the app's own parser.
    pub oauth: Option<String>,
}

fn null(raw: Option<&RawValue>) -> Option<&RawValue> {
    raw.filter(|r| r.get().trim() != "null")
}

fn string_map(raw: Option<&RawValue>, key: &str) -> Result<BTreeMap<String, String>, String> {
    match null(raw) {
        None => Ok(BTreeMap::new()),
        Some(raw) => serde_json::from_str(raw.get())
            .map_err(|_| format!("\"{key}\" must be an object of strings")),
    }
}

fn text(raw: Option<&RawValue>, key: &str) -> Result<Option<String>, String> {
    null(raw)
        .map(|raw| {
            serde_json::from_str::<String>(raw.get())
                .map_err(|_| format!("\"{key}\" must be a string"))
        })
        .transpose()
        .map(|t| t.map(|t| t.trim().to_string()).filter(|t| !t.is_empty()))
}

/// Read one legacy (`.mcp.json`) server entry.
///
/// # Errors
/// Why the entry cannot be used — said by name to the user, never dropped.
pub fn parse_legacy_entry(text_of_entry: &str) -> Result<LegacyEntry, String> {
    let entry: Entry<'_> = serde_json::from_str(text_of_entry)
        .map_err(|_| "the entry is not an object".to_string())?;
    let kind = text(entry.kind, "type")?;
    let headers = || string_map(entry.headers, "headers");
    let url = || text(entry.url, "url")?.ok_or_else(|| "missing \"url\"".to_string());
    let has_command = null(entry.command).is_some();
    let transport = match kind.as_deref() {
        Some("stdio") | None if has_command => {
            let command =
                text(entry.command, "command")?.ok_or("\"command\" must be a non-empty string")?;
            let args = match null(entry.args) {
                None => Vec::new(),
                Some(raw) => serde_json::from_str(raw.get())
                    .map_err(|_| "\"args\" must be an array of strings".to_string())?,
            };
            ServerTransport::Stdio {
                command,
                args,
                env: string_map(entry.env, "env")?,
            }
        }
        Some("stdio") => return Err("a stdio server needs a \"command\"".into()),
        Some("http" | "streamable-http" | "streamableHttp" | "streamable_http") => {
            ServerTransport::Http {
                url: url()?,
                headers: headers()?,
                sse_fallback: false,
            }
        }
        Some("sse") => ServerTransport::Sse {
            url: url()?,
            headers: headers()?,
        },
        Some(other) => return Err(format!("the {other} transport is not supported")),
        None if null(entry.url).is_some() => ServerTransport::Http {
            url: url()?,
            headers: headers()?,
            sse_fallback: true,
        },
        None => return Err("the entry needs a \"command\" (stdio) or a \"url\" (http/sse)".into()),
    };
    if matches!(transport, ServerTransport::Stdio { .. }) && null(entry.oauth).is_some() {
        return Err("\"oauth\" is for a remote server, not a command".into());
    }
    Ok(LegacyEntry {
        transport,
        cwd: text(entry.cwd, "cwd")?,
        oauth: null(entry.oauth).map(|raw| raw.get().to_string()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_is_stdio_and_a_bare_url_is_http_with_the_sse_fallback() {
        let stdio = parse_legacy_entry(
            r#"{"command": "npx", "args": ["-y", "x"], "env": {"A": "b"}, "cwd": "."}"#,
        )
        .unwrap();
        assert_eq!(stdio.transport.target(), "npx -y x");
        assert_eq!(stdio.cwd.as_deref(), Some("."));
        let bare = parse_legacy_entry(r#"{"url": "https://h/mcp"}"#).unwrap();
        assert!(matches!(
            bare.transport,
            ServerTransport::Http {
                sse_fallback: true,
                ..
            }
        ));
        for kind in [
            "http",
            "streamable-http",
            "streamableHttp",
            "streamable_http",
        ] {
            let entry = parse_legacy_entry(&format!(r#"{{"type": "{kind}", "url": "https://h"}}"#))
                .unwrap();
            assert!(
                matches!(
                    entry.transport,
                    ServerTransport::Http {
                        sse_fallback: false,
                        ..
                    }
                ),
                "{kind}"
            );
        }
        assert!(matches!(
            parse_legacy_entry(r#"{"type": "sse", "url": "https://h"}"#)
                .unwrap()
                .transport,
            ServerTransport::Sse { .. }
        ));
    }

    #[test]
    fn unknown_keys_are_ignored_and_oauth_is_kept_for_the_app() {
        let entry = parse_legacy_entry(
            r#"{"type": "http", "url": "https://mcp.notion.com/mcp", "oauth_resource": "x", "oauth": {"client_id": "c"}}"#,
        )
        .unwrap();
        assert_eq!(entry.oauth.as_deref(), Some(r#"{"client_id": "c"}"#));
    }

    #[test]
    fn a_bad_entry_says_why() {
        for (bad, says) in [
            ("3", "object"),
            (r#"{"type": "ws", "url": "wss://x"}"#, "ws"),
            (r#"{"type": "stdio"}"#, "command"),
            (r#"{"command": ""}"#, "command"),
            (r#"{"command": "x", "args": "y"}"#, "args"),
            (r#"{"command": "x", "env": {"A": 1}}"#, "env"),
            (r#"{"type": "http"}"#, "url"),
            (r#"{}"#, "command"),
            (r#"{"command": "x", "oauth": {}}"#, "oauth"),
        ] {
            let err = parse_legacy_entry(bad).unwrap_err();
            assert!(err.contains(says), "{bad}: {err}");
        }
    }

    #[test]
    fn mapping_touches_values_never_names() {
        let t = ServerTransport::Stdio {
            command: "$c".into(),
            args: vec!["$a".into()],
            env: BTreeMap::from([("$k".to_string(), "$v".to_string())]),
        }
        .map_values(&|s| s.replace('$', "!"));
        assert_eq!(
            t,
            ServerTransport::Stdio {
                command: "!c".into(),
                args: vec!["!a".into()],
                env: BTreeMap::from([("$k".to_string(), "!v".to_string())]),
            }
        );
    }
}
