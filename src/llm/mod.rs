//! The real LLM backend: an OpenAI-compatible streaming client and the pieces
//! that plug it into the app's [`ReplySource`](crate::stream::ReplySource) seam.
//!
//! This is the **I/O boundary** for a real model (like `main.rs`/`term.rs`): the
//! only place that speaks HTTP. The pure cores — [`config`] (the `providers.toml`
//! parse), [`thinking`] (the reasoning splitter), [`models::parse_models`], and
//! the payload/endpoint builders in [`openai`] — are unit-tested; the network
//! calls are verified by hand. See `docs/llm.md`.
//!
//! Streaming runs on a plain OS thread (the [`ReplySource::spawn`] contract), so
//! the HTTP client is **blocking** `reqwest`: a response is a `std::io::Read` we
//! drain line-by-line for SSE frames while polling the
//! [`CancelToken`](crate::stream::CancelToken) — the same cooperative-cancel
//! shape as `DummyAi`, with no nested tokio runtime.

pub mod backend;
pub mod config;
pub mod keystore;
pub mod models;
pub mod openai;
pub mod settings;
pub mod thinking;

use std::time::Duration;

pub use backend::LlmBackend;
pub use config::{ModelConfig, ProvidersFile, Selection};
pub use keystore::EnvFile;
pub use models::ModelEntry;
pub use settings::Settings;

/// One message in a chat-completion request.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

impl ChatMessage {
    #[must_use]
    pub fn system(c: impl Into<String>) -> Self {
        Self {
            role: "system".into(),
            content: c.into(),
        }
    }

    #[must_use]
    pub fn user(c: impl Into<String>) -> Self {
        Self {
            role: "user".into(),
            content: c.into(),
        }
    }

    #[must_use]
    pub fn assistant(c: impl Into<String>) -> Self {
        Self {
            role: "assistant".into(),
            content: c.into(),
        }
    }
}

/// A failure talking to a provider. [`std::fmt::Display`] gives the user-facing
/// message surfaced as a red `StreamEvent::Error`.
#[derive(Debug)]
pub enum LlmError {
    /// A transport/connection failure (DNS, TLS, proxy, timeout).
    Http(String),
    /// A non-2xx HTTP response, with the provider's error body.
    Api { status: u16, body: String },
    /// The response body didn't match the expected shape.
    Decode(String),
    /// The request was cancelled (the user interrupted or closed the picker).
    Cancelled,
}

impl std::fmt::Display for LlmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Http(e) => write!(f, "request failed: {e}"),
            Self::Api { status, body } => {
                let body = body.trim();
                if body.is_empty() {
                    write!(f, "provider returned HTTP {status}")
                } else {
                    // Keep the surfaced error to one tidy line.
                    let flat: String = body.split_whitespace().collect::<Vec<_>>().join(" ");
                    let flat = truncate_chars(&flat, 300);
                    write!(f, "HTTP {status}: {flat}")
                }
            }
            Self::Decode(e) => write!(f, "could not decode the response: {e}"),
            Self::Cancelled => write!(f, "cancelled"),
        }
    }
}

impl std::error::Error for LlmError {}

/// The module's result alias.
pub type Result<T> = std::result::Result<T, LlmError>;

/// Truncate to at most `max` chars, adding an ellipsis when cut.
fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let head: String = s.chars().take(max).collect();
    format!("{head}…")
}

/// Build a blocking HTTP client: env proxies (`HTTPS_PROXY`) are picked up
/// automatically; the agent proxy's custom CA is added from `SSL_CERT_FILE` /
/// `INLINE_TUI_CA_FILE` so `rustls` trusts it. A short connect timeout bounds a
/// hung connect.
///
/// `op_timeout` is applied via [`reqwest::blocking::ClientBuilder::timeout`],
/// which in the blocking client is a **per-operation** deadline — it bounds the
/// send/header exchange *and* each individual body `read()` (a fresh deadline
/// per read), **not** the total request. The streaming client relies on that:
/// each body read wakes after `op_timeout` so the SSE drain can poll the
/// `CancelToken` and reap promptly on interrupt, while a legitimately long
/// stream keeps going (a read that times out is retried, not fatal — see
/// `openai::stream_chat`). So it doubles as the worst-case interrupt latency.
///
/// # Errors
/// Returns [`LlmError::Http`] if the client can't be built.
pub(crate) fn http_client(op_timeout: Duration) -> Result<reqwest::blocking::Client> {
    let mut builder = reqwest::blocking::Client::builder()
        .pool_idle_timeout(Duration::from_secs(30))
        .connect_timeout(Duration::from_secs(30))
        .timeout(op_timeout);
    for cert in extra_root_certificates() {
        builder = builder.add_root_certificate(cert);
    }
    builder.build().map_err(|e| LlmError::Http(e.to_string()))
}

/// Extra trust roots loaded from `INLINE_TUI_CA_FILE` or `SSL_CERT_FILE` — the
/// agent proxy's CA bundle, so real calls work behind it. Best-effort: an
/// unreadable or malformed file yields no extra roots (the built-in Mozilla
/// roots still apply). Boundary code — reads the environment and the filesystem.
fn extra_root_certificates() -> Vec<reqwest::Certificate> {
    let Some(path) =
        std::env::var_os("INLINE_TUI_CA_FILE").or_else(|| std::env::var_os("SSL_CERT_FILE"))
    else {
        return Vec::new();
    };
    let Ok(pem) = std::fs::read(&path) else {
        return Vec::new();
    };
    reqwest::Certificate::from_pem_bundle(&pem).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_constructors_set_role() {
        assert_eq!(ChatMessage::system("a").role, "system");
        assert_eq!(ChatMessage::user("a").role, "user");
        assert_eq!(ChatMessage::assistant("a").role, "assistant");
    }

    #[test]
    fn api_error_flattens_and_trims_the_body() {
        let e = LlmError::Api {
            status: 401,
            body: "  {\n  \"error\": \"bad key\"\n}  ".to_string(),
        };
        let shown = e.to_string();
        assert!(shown.starts_with("HTTP 401: "));
        assert!(!shown.contains('\n'), "flattened to one line");
        assert!(shown.contains("bad key"));
    }

    #[test]
    fn api_error_with_empty_body_reports_the_status() {
        let e = LlmError::Api {
            status: 500,
            body: String::new(),
        };
        assert_eq!(e.to_string(), "provider returned HTTP 500");
    }

    #[test]
    fn truncate_chars_adds_an_ellipsis_when_cut() {
        assert_eq!(truncate_chars("hello", 10), "hello");
        assert_eq!(truncate_chars("hello", 3), "hel…");
    }
}
