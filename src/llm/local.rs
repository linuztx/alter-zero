//! Local model servers — the pure half of `/login`'s **Use a local model**
//! (`docs/local-models.md`): the address grammar a host typed into its field
//! is read in, for a server that speaks chat completions (LM Studio, or a
//! llama.cpp / vLLM a user described in their own file — Ollama's own
//! grammar is [`super::ollama::host_url`]), and the sentences a connection
//! test answers with ([`verdict`]), which a session's request refused by a
//! stopped server on this machine is answered with too ([`refused_advice`]).
//!
//! Pure and unit-tested; the connection test itself runs at the boundary
//! (`tui::login`), over the same [`super::models::fetch_models`] the `/model`
//! picker lists with.

use std::net::Ipv6Addr;

use super::models::ModelEntry;
use super::{LlmError, Result};

/// A base URL's three parts a bare host borrows: its scheme, its explicit
/// port, and its path (`""` for none) — read off the provider file's own
/// base.
struct Template {
    scheme: String,
    port: Option<u16>,
    path: String,
}

impl Template {
    fn of(base: &str) -> Self {
        match url::Url::parse(base) {
            Ok(url) => Self {
                scheme: url.scheme().to_string(),
                port: url.port(),
                path: url.path().trim_end_matches('/').to_string(),
            },
            Err(_) => Self {
                scheme: "http".to_string(),
                port: None,
                path: String::new(),
            },
        }
    }
}

/// `host[:port]` split the way a URL's authority is: a bracketed IPv6
/// address with an optional port, a bare IPv6 address (no port can follow
/// one unbracketed), or a name or IPv4 address with an optional numeric
/// port. A port that is not a number is left on the host, for the
/// connection test to report as the address it is not.
fn split_authority(authority: &str) -> (String, Option<u16>) {
    if let Some(rest) = authority.strip_prefix('[')
        && let Some((host, after)) = rest.split_once(']')
    {
        let port = after.strip_prefix(':').and_then(|p| p.parse().ok());
        return (host.to_string(), port);
    }
    if authority.parse::<Ipv6Addr>().is_ok() {
        return (authority.to_string(), None);
    }
    match authority.rsplit_once(':') {
        Some((host, port)) if !host.contains(':') => match port.parse::<u16>() {
            Ok(port) => (host.to_string(), Some(port)),
            Err(_) => (authority.to_string(), None),
        },
        _ => (authority.to_string(), None),
    }
}

/// The API base a server address names, completed from `template` — the
/// provider file's own base (`http://127.0.0.1:1234/v1`):
///
/// - **empty** is the template itself;
/// - **no scheme** takes the template's, and **no port** the template's — so
///   `localhost:1234` and `gpu-box` are servers, the way LM Studio's own
///   server page shows them;
/// - **an explicit scheme** brings its own default port, Ollama's rule
///   (`http://gpu-box` is port 80, not 1234);
/// - **no path** takes the template's API path (`/v1`), while a path is the
///   API base itself, taken as typed — a reverse proxy's `…/openai`;
/// - an **unspecified bind address** (`0.0.0.0`, `::`) becomes the loopback a
///   client can actually connect to.
///
/// A trailing slash is trimmed either way. What comes back is not validated:
/// an address that is no URL at all is the connection test's to report.
#[must_use]
pub fn base_from_host(raw: &str, template: &str) -> String {
    let template_base = template.trim().trim_end_matches('/');
    let raw = raw.trim();
    if raw.is_empty() {
        return template_base.to_string();
    }
    let lent = Template::of(template_base);
    let (scheme, rest, typed_scheme) = match raw.split_once("://") {
        Some((scheme, rest)) => (scheme.to_ascii_lowercase(), rest, true),
        None => (lent.scheme.clone(), raw, false),
    };
    let (authority, path) = match rest.find('/') {
        Some(at) => (&rest[..at], rest[at..].trim_end_matches('/')),
        None => (rest, ""),
    };
    let (host, port) = split_authority(authority);
    let host = match host.as_str() {
        "0.0.0.0" => "127.0.0.1".to_string(),
        "::" => "::1".to_string(),
        _ => host,
    };
    let host = if host.contains(':') {
        format!("[{host}]")
    } else {
        host
    };
    // A scheme typed is a URL meant: its port is the scheme's own.
    let port = port.or(if typed_scheme { None } else { lent.port });
    let port = port.map_or_else(String::new, |port| format!(":{port}"));
    let path = if path.is_empty() {
        lent.path.as_str()
    } else {
        path
    };
    format!("{scheme}://{host}{port}{path}")
}

/// The address a field offers for a base: the server, without the API path
/// [`base_from_host`] puts back — `http://127.0.0.1:1234` for LM Studio's
/// `http://127.0.0.1:1234/v1`, which is how LM Studio's own server page
/// shows it. A base whose path is not `template`'s is shown whole.
#[must_use]
pub fn server_address(base: &str, template: &str) -> String {
    let base = base.trim().trim_end_matches('/');
    let api_path = Template::of(template.trim().trim_end_matches('/')).path;
    if api_path.is_empty() {
        return base.to_string();
    }
    match base.strip_suffix(api_path.as_str()) {
        // Only when what is left is a bare server: a prefix before the API
        // path is a reverse proxy's, and dropping the path would lose it.
        Some(server)
            if server.split_once("://").is_some_and(|(_, authority)| {
                !authority.is_empty() && !authority.contains('/')
            }) =>
        {
            server.to_string()
        }
        _ => base.to_string(),
    }
}

/// `host[:port]` of a base — what a connection test's sentences call the
/// server (`127.0.0.1:1234`), the port only when the URL names one — or
/// `None` when the base is no http(s) URL with a host: no address a test
/// could try, which the page says before trying.
#[must_use]
pub fn server_label(base: &str) -> Option<String> {
    let url = url::Url::parse(base).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    let host = url.host_str().filter(|host| !host.is_empty())?;
    Some(match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_string(),
    })
}

/// The verdict for an address no connection could be made to at all — one
/// [`server_label`] finds no server in — answered before any request, with
/// the server's own address (`example`) as the form one takes.
#[must_use]
pub fn unusable_address(typed: &str, example: &str) -> Verdict {
    Verdict::Failed(format!(
        "{typed} is not an address — type one like {example}."
    ))
}

/// What a local server's connection test found — the `/login` server page's
/// whole vocabulary (`docs/local-models.md`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// The server answered with its catalog: this many models to chat with.
    Passed(usize),
    /// The server is there and refused the request as unauthorized — it
    /// wants a token, which the page asks for next. The sentence says so.
    Unauthorized(String),
    /// Why the test failed, worded for the page.
    Failed(String),
}

/// Read a connection test's result — the server's model listing, fetched the
/// way `/model` fetches it — as a [`Verdict`]. `name` is the provider's label
/// (`LM Studio`) and `server` the address the sentences name
/// (`127.0.0.1:1234`).
///
/// The sentences say what to do rather than what went wrong inside the
/// client: a refused connection is a server not running (or not there), a
/// name that will not resolve is a typo, a 404 or a body that is not the
/// listing is something else listening on that port, and an untrusted
/// certificate is the one failure fixable from this side — by trusting it
/// (asking whether to accept it is future work, `docs/local-models.md`).
#[must_use]
pub fn verdict(name: &str, server: &str, result: &Result<Vec<ModelEntry>>) -> Verdict {
    let error = match result {
        Ok(models) => return Verdict::Passed(models.len()),
        Err(error) => error,
    };
    let text = match error {
        LlmError::Api {
            status: 401 | 403, ..
        } => return Verdict::Unauthorized(format!("{name} at {server} wants an API token.")),
        LlmError::Api { status: 404, .. } | LlmError::Decode(_) => {
            format!("{server} answered, but not as {name} — check the address and port.")
        }
        LlmError::Api { .. } => format!("{name} at {server} answered {error}"),
        LlmError::Unreachable { message, .. } => connect_failure(name, server, message),
        LlmError::Http(message) if timed_out(message) => no_answer(server),
        LlmError::Http(message) => format!("The connection to {server} failed: {message}"),
        LlmError::Cancelled => "The test was stopped.".to_string(),
    };
    Verdict::Failed(text)
}

/// The advice for a session's request that a keyless server on **this
/// machine** refused: the server is not running, which is something to start
/// rather than an outage to wait out (`docs/offline.md`), said in the `/login`
/// test's own words. `None` for a server elsewhere, whose refusal is the
/// network and is waited for like any provider's, and for any other cause.
#[must_use]
pub fn refused_advice(name: &str, base: &str, message: &str) -> Option<String> {
    let server = server_label(base)?;
    (refused(message) && super::ollama::is_local_base(base)).then(|| not_running(name, &server))
}

/// Does a transport message say the connection was refused?
fn refused(message: &str) -> bool {
    message.to_ascii_lowercase().contains("connection refused")
}

/// The sentence for a server that refused the connection.
fn not_running(name: &str, server: &str) -> String {
    format!("Nothing is answering at {server} — is {name}'s server running?")
}

/// A connection that could not be made, by its cause.
fn connect_failure(name: &str, server: &str, message: &str) -> String {
    if refused(message) {
        return not_running(name, server);
    }
    let lower = message.to_ascii_lowercase();
    let unresolved = [
        "dns error",
        "failed to lookup",
        "name or service not known",
        "nodename nor servname",
        "no such host",
    ];
    if unresolved.iter().any(|tell| lower.contains(tell)) {
        return format!("Could not find {server} — check the address.");
    }
    if lower.contains("certificate") {
        return format!(
            "{server}'s certificate is not trusted — it may be self-signed. Point \
             ALTER_ZERO_CA_FILE at its CA certificate, then test again."
        );
    }
    // A TLS handshake answered in plain HTTP: rustls reads the reply as a
    // record of the wrong type.
    if lower.contains("corrupt message") || lower.contains("invalidcontenttype") {
        return format!("{server} does not speak https — try http:// instead.");
    }
    if timed_out(&lower) {
        return no_answer(server);
    }
    format!("Could not connect to {server}: {message}")
}

/// Does a transport message say the operation ran out of time?
fn timed_out(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("timed out") || lower.contains("timeout")
}

/// The sentence for a server that never answered.
fn no_answer(server: &str) -> String {
    format!(
        "{server} did not answer in time — check the address, and that nothing blocks the port."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const LMSTUDIO: &str = "http://127.0.0.1:1234/v1";

    #[test]
    fn an_empty_address_is_the_files_own_base() {
        assert_eq!(base_from_host("", LMSTUDIO), LMSTUDIO);
        assert_eq!(base_from_host("   ", LMSTUDIO), LMSTUDIO);
        assert_eq!(
            base_from_host("", "http://127.0.0.1:1234/v1/"),
            LMSTUDIO,
            "the template's own slash is trimmed too"
        );
    }

    #[test]
    fn a_bare_host_takes_the_templates_scheme_port_and_path() {
        // LM Studio's server page shows `http://127.0.0.1:1234`, and people
        // type less than that: every one of these is the same server.
        assert_eq!(
            base_from_host("localhost:1234", LMSTUDIO),
            "http://localhost:1234/v1"
        );
        assert_eq!(
            base_from_host("gpu-box", LMSTUDIO),
            "http://gpu-box:1234/v1"
        );
        assert_eq!(
            base_from_host("192.168.1.20:4321", LMSTUDIO),
            "http://192.168.1.20:4321/v1"
        );
    }

    #[test]
    fn an_explicit_scheme_brings_its_own_default_port() {
        // Ollama's grammar: a scheme typed is a URL meant, so its port is the
        // scheme's — `https://lms.example.com` is port 443, not 1234.
        assert_eq!(
            base_from_host("http://gpu-box", LMSTUDIO),
            "http://gpu-box/v1"
        );
        assert_eq!(
            base_from_host("https://lms.example.com", LMSTUDIO),
            "https://lms.example.com/v1"
        );
        assert_eq!(
            base_from_host("HTTP://gpu-box:5678/", LMSTUDIO),
            "http://gpu-box:5678/v1",
            "the scheme is case-insensitive, a lone slash is no path"
        );
    }

    #[test]
    fn a_path_is_the_api_base_itself() {
        assert_eq!(
            base_from_host("http://gpu-box:1234/v1", LMSTUDIO),
            "http://gpu-box:1234/v1"
        );
        assert_eq!(
            base_from_host("https://proxy.example/openai/", LMSTUDIO),
            "https://proxy.example/openai"
        );
        assert_eq!(
            base_from_host("gpu-box/lmstudio/v1", LMSTUDIO),
            "http://gpu-box:1234/lmstudio/v1",
            "a bare host with a path still takes the scheme and port"
        );
    }

    #[test]
    fn an_unspecified_bind_address_becomes_the_loopback() {
        // `0.0.0.0` is what a *server* binds, and what LM Studio's "serve on
        // the local network" setting prints; a client connecting to it fails
        // on some platforms.
        assert_eq!(
            base_from_host("0.0.0.0:1234", LMSTUDIO),
            "http://127.0.0.1:1234/v1"
        );
        assert_eq!(base_from_host("[::]", LMSTUDIO), "http://[::1]:1234/v1");
    }

    #[test]
    fn an_ipv6_address_is_bracketed_with_or_without_a_port() {
        assert_eq!(
            base_from_host("[::1]:1234", LMSTUDIO),
            "http://[::1]:1234/v1"
        );
        assert_eq!(base_from_host("::1", LMSTUDIO), "http://[::1]:1234/v1");
        assert_eq!(
            base_from_host("http://[fe80::1]", LMSTUDIO),
            "http://[fe80::1]/v1"
        );
    }

    #[test]
    fn a_fields_address_is_the_server_without_its_api_path() {
        assert_eq!(server_address(LMSTUDIO, LMSTUDIO), "http://127.0.0.1:1234");
        assert_eq!(
            server_address("http://gpu-box:1234/v1/", LMSTUDIO),
            "http://gpu-box:1234"
        );
        // A base whose path is not the API path is shown whole: the field
        // must round-trip through `base_from_host` to the base itself.
        assert_eq!(
            server_address("https://proxy.example/openai", LMSTUDIO),
            "https://proxy.example/openai"
        );
        // Ollama's base has no API path at all.
        assert_eq!(
            server_address("http://127.0.0.1:11434", "http://127.0.0.1:11434"),
            "http://127.0.0.1:11434"
        );
        for base in [LMSTUDIO, "http://gpu-box:4321/v1"] {
            assert_eq!(
                base_from_host(&server_address(base, LMSTUDIO), LMSTUDIO),
                base,
                "the address offered tests the base it came from"
            );
        }
    }

    #[test]
    fn a_server_is_named_by_its_host_and_port() {
        assert_eq!(
            server_label("http://127.0.0.1:1234/v1").as_deref(),
            Some("127.0.0.1:1234")
        );
        assert_eq!(
            server_label("https://lms.example.com/v1").as_deref(),
            Some("lms.example.com"),
            "a scheme's own port says nothing"
        );
        assert_eq!(
            server_label("http://[::1]:1234/v1").as_deref(),
            Some("[::1]:1234")
        );
    }

    #[test]
    fn an_address_that_is_no_http_url_has_no_name() {
        // What a typo becomes once completed: nothing a test could try.
        for base in [
            "http://local host:1234/v1",
            "ftp://x/v1",
            "http://:1234/v1",
            "",
        ] {
            assert_eq!(server_label(base), None, "{base:?}");
        }
    }

    #[test]
    fn an_address_that_is_no_url_is_answered_with_the_servers_own_as_an_example() {
        assert_eq!(
            unusable_address("local host:1234", "http://127.0.0.1:11434"),
            Verdict::Failed(
                "local host:1234 is not an address — type one like http://127.0.0.1:11434."
                    .to_string()
            )
        );
    }

    // --- the connection test's verdict ------------------------------------

    use crate::llm::LlmError;

    fn entry(id: &str) -> ModelEntry {
        ModelEntry {
            id: id.to_string(),
            provider: "lmstudio".to_string(),
            display_name: id.to_string(),
            reasoning: None,
            vision: None,
            context: None,
            service_tiers: Vec::new(),
        }
    }

    fn failed(error: LlmError) -> String {
        match verdict("LM Studio", "127.0.0.1:1234", &Err(error)) {
            Verdict::Failed(text) => text,
            other => panic!("expected a failure, got {other:?}"),
        }
    }

    fn unreachable(message: &str) -> LlmError {
        LlmError::Unreachable {
            host: "127.0.0.1:1234".to_string(),
            message: message.to_string(),
        }
    }

    #[test]
    fn a_listing_passes_with_its_count() {
        let listed = Ok(vec![entry("qwen/qwen3-8b"), entry("google/gemma-3-12b")]);
        assert_eq!(verdict("LM Studio", "x", &listed), Verdict::Passed(2));
        // A server with nothing to chat with is still a server — reachable,
        // and the address is right; what it lacks is a model.
        assert_eq!(
            verdict("LM Studio", "x", &Ok(Vec::new())),
            Verdict::Passed(0)
        );
    }

    #[test]
    fn a_refused_connection_asks_whether_the_server_is_running() {
        let text = failed(unreachable(
            "error sending request for url (http://127.0.0.1:1234/api/v1/models): \
             client error (Connect): tcp connect error: Connection refused (os error 111)",
        ));
        assert_eq!(
            text,
            "Nothing is answering at 127.0.0.1:1234 — is LM Studio's server running?"
        );
    }

    #[test]
    fn a_session_refused_by_a_server_on_this_machine_is_told_to_start_it() {
        // docs/offline.md: a refusal from this machine is a server that is
        // not running, said in the test's own words, rather than the network
        // to wait for.
        let refused = "error sending request for url (http://127.0.0.1:1234/v1/chat/completions): \
                       client error (Connect): tcp connect error: Connection refused (os error 111)";
        assert_eq!(
            refused_advice("LM Studio", "http://127.0.0.1:1234/v1", refused).as_deref(),
            Some("Nothing is answering at 127.0.0.1:1234 — is LM Studio's server running?")
        );
        for base in ["http://localhost:1234/v1", "http://[::1]:1234/v1"] {
            assert!(
                refused_advice("LM Studio", base, refused).is_some(),
                "{base} is this machine"
            );
        }
        // A server elsewhere is reached over the network, and is waited for.
        assert_eq!(
            refused_advice("LM Studio", "http://gpu-box:1234/v1", refused),
            None
        );
        // Any other cause keeps its own handling.
        assert_eq!(
            refused_advice(
                "LM Studio",
                "http://127.0.0.1:1234/v1",
                "operation timed out"
            ),
            None
        );
    }

    #[test]
    fn a_name_that_does_not_resolve_says_to_check_the_address() {
        let text = failed(unreachable(
            "error sending request: client error (Connect): dns error: \
             failed to lookup address information: Name or service not known",
        ));
        assert_eq!(text, "Could not find 127.0.0.1:1234 — check the address.");
    }

    #[test]
    fn an_untrusted_certificate_names_the_way_to_trust_it() {
        // A self-signed server: the one failure the user can fix without
        // touching the server, by trusting its certificate
        // (docs/local-models.md — asking to accept it is future work).
        let text = failed(unreachable(
            "error sending request: client error (Connect): \
             invalid peer certificate: UnknownIssuer",
        ));
        assert!(text.contains("certificate"), "{text}");
        assert!(text.contains("self-signed"), "{text}");
        assert!(text.contains("ALTER_ZERO_CA_FILE"), "{text}");
    }

    #[test]
    fn https_to_a_plain_http_server_says_to_drop_the_s() {
        let text = failed(unreachable(
            "error sending request: client error (Connect): \
             received corrupt message of type InvalidContentType",
        ));
        assert!(text.contains("http://"), "{text}");
    }

    #[test]
    fn a_timeout_says_the_server_did_not_answer() {
        let text = failed(unreachable(
            "error sending request: client error (Connect): operation timed out",
        ));
        assert!(text.contains("did not answer in time"), "{text}");
        let stalled = failed(LlmError::Http("operation timed out".to_string()));
        assert!(stalled.contains("did not answer in time"), "{stalled}");
    }

    #[test]
    fn any_other_connect_failure_keeps_its_cause() {
        let text = failed(unreachable("tcp connect error: Network is unreachable"));
        assert_eq!(
            text,
            "Could not connect to 127.0.0.1:1234: tcp connect error: Network is unreachable"
        );
    }

    #[test]
    fn an_unauthorized_answer_asks_for_a_token() {
        for status in [401, 403] {
            let result = Err(LlmError::Api {
                status,
                body: r#"{"error":"Missing API token"}"#.to_string(),
            });
            assert_eq!(
                verdict("LM Studio", "127.0.0.1:1234", &result),
                Verdict::Unauthorized(
                    "LM Studio at 127.0.0.1:1234 wants an API token.".to_string()
                ),
                "HTTP {status}"
            );
        }
    }

    #[test]
    fn something_else_answering_there_is_not_the_server() {
        // A wrong port with a web server on it: a 404 for the listing, or a
        // page that is not JSON at all.
        let missing = failed(LlmError::Api {
            status: 404,
            body: "<html>Not Found</html>".to_string(),
        });
        assert_eq!(
            missing,
            "127.0.0.1:1234 answered, but not as LM Studio — check the address and port."
        );
        let garbled = failed(LlmError::Decode("expected value at line 1".to_string()));
        assert_eq!(garbled, missing);
    }

    #[test]
    fn any_other_refusal_quotes_the_server() {
        let text = failed(LlmError::Api {
            status: 500,
            body: "model loader crashed".to_string(),
        });
        assert_eq!(
            text,
            "LM Studio at 127.0.0.1:1234 answered HTTP 500: model loader crashed"
        );
    }

    #[test]
    fn a_template_without_a_port_or_path_lends_only_what_it_has() {
        // A hosted provider's own variable: no port in its base, so none is
        // added — and its https is what a bare host means there.
        assert_eq!(
            base_from_host("proxy.example", "https://api.example.com/v1"),
            "https://proxy.example/v1"
        );
        assert_eq!(
            base_from_host("gpu", "http://127.0.0.1:8080"),
            "http://gpu:8080"
        );
    }
}
