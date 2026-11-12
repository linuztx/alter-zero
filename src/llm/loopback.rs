//! The loopback callback every browser sign-in comes back through — an MCP
//! server's OAuth flow (`docs/tools/mcp.md`), ChatGPT's (`docs/providers/chatgpt.md`), the
//! Anthropic Console's (`docs/providers/claude.md`) and Google Antigravity's
//! (`docs/providers/antigravity.md`). One implementation, because the first three
//! used to share one hand-rolled loop and its weaknesses: a
//! browser preconnect that never sends a byte froze the wait (and Esc with
//! it), a macOS socket inheriting the listener's non-blocking mode read a
//! request still on its way as nothing, a code with no `state` was redeemed,
//! a stray or forged request could end the sign-in, and declining on the
//! consent page left the flow waiting minutes for a code that would never
//! come.
//!
//! The pure halves — the response parse, its checks, the pages the browser
//! lands on (`page`, `docs/providers/callback-page.md`) — are tested as such; the
//! server is exercised against real sockets.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use crate::stream::CancelToken;

mod page;

use page::{Detail, Page, Tone};

/// The wait loop's cadence — the cancel check, the paste channel, new
/// connections.
const POLL: Duration = Duration::from_millis(50);

/// How long a browser connection may take to send its request: a
/// preconnect that never does costs its own thread this long, nothing else.
const READ_TIMEOUT: Duration = Duration::from_secs(10);

/// The most of a request head the callback reads — the request line is all
/// it needs, and a browser's whole head fits many times over.
const HEAD_MAX: usize = 16 * 1024;

/// Browser connections served at once; more are closed unread.
const MAX_CONNECTIONS: usize = 16;

// ---------------------------------------------------------------------------
// the authorization response (pure)

/// What an authorization response carried (RFC 6749 §4.1.2 and §4.1.2.1,
/// plus RFC 9207's `iss`) — the loopback callback's query, or a pasted
/// redirect URL's.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct AuthResponse {
    pub(crate) code: Option<String>,
    pub(crate) state: Option<String>,
    pub(crate) iss: Option<String>,
    pub(crate) error: Option<String>,
    pub(crate) error_description: Option<String>,
}

/// Parse an authorization response out of a redirect URL, a request path or
/// a bare query: `None` unless it carries a code or an error.
pub(crate) fn parse_auth_response(text: &str) -> Option<AuthResponse> {
    let param = |key: &str| crate::llm::mcp::redirect_param(text, key);
    let response = AuthResponse {
        code: param("code"),
        state: param("state"),
        iss: param("iss"),
        error: param("error"),
        error_description: param("error_description"),
    };
    (response.code.is_some() || response.error.is_some()).then_some(response)
}

/// Why an authorization response was not taken.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Rejected {
    /// Not this flow's answer — no `state`, another attempt's, or another
    /// authorization server's: turned away, and the flow keeps waiting for
    /// its own.
    Foreign(String),
    /// This flow's answer, and it is a refusal: the flow ends with the
    /// authorization server's reason.
    Refused(Refusal),
}

/// The authorization server's refusal (RFC 6749 §4.1.2.1), its two parts
/// kept apart: the browser's page sets them in a box of their own, while the
/// flow and the terminal read them as one sentence (`Display`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Refusal {
    /// The `error` code — `access_denied`.
    pub(crate) error: String,
    /// The `error_description`, when the server gave one.
    pub(crate) description: Option<String>,
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "the authorization server refused the sign-in: {}",
            self.error
        )?;
        if let Some(description) = &self.description {
            write!(f, " ({description})")?;
        }
        Ok(())
    }
}

/// What a response must carry to be this sign-in's answer.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Expected<'a> {
    /// The `state` the authorize URL sent.
    pub(crate) state: &'a str,
    /// The issuer RFC 9207's `iss` must name, when the flow knows one.
    pub(crate) issuer: Option<&'a str>,
    /// The authorization server promised `iss` on every response, so a
    /// response without one is refused rather than excused.
    pub(crate) iss_required: bool,
}

/// Text cut to `max` characters — an authorization server's words shown to
/// the user are its to choose, their length is not.
fn clip(text: &str, max: usize) -> String {
    let mut clipped: String = text.chars().take(max).collect();
    if clipped.len() < text.len() {
        clipped.push('…');
    }
    clipped
}

/// What an authorization response means for the flow that is `expected`:
/// its code, its refusal, or not its answer at all. The state comes first
/// and is **required** — a response without one can't be told from a forged
/// one (RFC 6749 §10.12) — then the issuer (RFC 9207), and only then does
/// the content count, so a forged refusal can't end a sign-in any more than
/// a forged code can complete one.
pub(crate) fn check_auth_response(
    response: &AuthResponse,
    expected: &Expected<'_>,
) -> Result<String, Rejected> {
    match response.state.as_deref() {
        None => {
            return Err(Rejected::Foreign(
                "the response carries no state, so it can't be this sign-in's".to_string(),
            ));
        }
        Some(got) if got != expected.state => {
            return Err(Rejected::Foreign(
                "the response belongs to another sign-in attempt".to_string(),
            ));
        }
        Some(_) => {}
    }
    crate::llm::mcp::validate_issuer(
        response.iss.as_deref(),
        expected.issuer,
        expected.iss_required,
    )
    .map_err(Rejected::Foreign)?;
    if let Some(error) = &response.error {
        return Err(Rejected::Refused(Refusal {
            error: clip(error, 100),
            description: response.error_description.as_deref().map(|d| clip(d, 300)),
        }));
    }
    response
        .code
        .clone()
        .ok_or_else(|| Rejected::Foreign("the response carries no code".to_string()))
}

/// `text` as a sentence for the page: its first letter a capital and a full
/// stop closing it, so a reason worded for the terminal ("the response
/// carries no code") reads as the page's own.
fn sentence(text: &str) -> String {
    let mut chars = text.chars();
    let Some(first) = chars.next() else {
        return String::new();
    };
    let mut sentence: String = first.to_uppercase().chain(chars).collect();
    if !sentence.ends_with(['.', '!', '?']) {
        sentence.push('.');
    }
    sentence
}

/// What the browser is told, in a flow's own words.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Pages {
    /// The headline once the code is in hand — "Signed in to ChatGPT",
    /// "Authorization received".
    pub(crate) received: &'static str,
    /// Where a sign-in starts again — "/login", "/mcp".
    pub(crate) restart: &'static str,
}

/// The page the browser lands on — whether the sign-in got what it needed,
/// as a whole HTTP response (`docs/providers/callback-page.md`).
pub(crate) fn callback_page(verdict: &Result<String, Rejected>, pages: &Pages) -> String {
    let app = crate::APP_NAME;
    match verdict {
        Ok(_) => page::response(
            "200 OK",
            &Page {
                tone: Tone::Success,
                title: pages.received,
                lead: &format!("You can close this tab and return to {app}."),
                restart: None,
                detail: None,
            },
        ),
        Err(Rejected::Refused(refusal)) => page::response(
            "200 OK",
            &Page {
                tone: Tone::Refused,
                title: "Sign-in refused",
                lead: &format!(
                    "The authorization server turned the sign-in down. Return to {app} to try \
                     again."
                ),
                restart: None,
                detail: Some(Detail {
                    code: &refusal.error,
                    text: refusal.description.as_deref(),
                }),
            },
        ),
        Err(Rejected::Foreign(why)) => page::response(
            "400 Bad Request",
            &Page {
                tone: Tone::Notice,
                title: "Not this sign-in’s response",
                lead: &sentence(why),
                restart: Some(pages.restart),
                detail: None,
            },
        ),
    }
}

/// A request the callback answers itself, without asking the flow: it is
/// no answer to the sign-in at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stray {
    /// Not the callback path — a hand-typed address, a browser's own probe
    /// for `/favicon.ico`.
    Elsewhere,
    /// The callback path with neither a code nor an error.
    NoAnswer,
    /// An answer that arrived after the sign-in stopped waiting.
    Over,
}

/// The page a stray request gets. Nothing failed, so none of them is an
/// error; only an answer that came too late has a sign-in to start again.
fn stray_page(stray: Stray, pages: &Pages) -> String {
    let (status, title, lead, restart) = match stray {
        Stray::Elsewhere => (
            "404 Not Found",
            "Nothing here",
            "This address only receives sign-in responses.",
            None,
        ),
        Stray::NoAnswer => (
            "400 Bad Request",
            "Not a sign-in response",
            "It carries neither an authorization code nor an error.",
            None,
        ),
        Stray::Over => (
            "410 Gone",
            "This sign-in has ended",
            "It stopped waiting before this answer arrived.",
            Some(pages.restart),
        ),
    };
    page::response(
        status,
        &Page {
            tone: Tone::Notice,
            title,
            lead,
            restart,
            detail: None,
        },
    )
}

// ---------------------------------------------------------------------------
// the server (I/O)

/// The loopback callback (RFC 8252 §7.3): `127.0.0.1` and, where this
/// machine has IPv6 loopback, `[::1]` on the **same** port — a redirect
/// naming `localhost` may be resolved by the browser to either.
pub(crate) struct CallbackServer {
    listeners: Vec<TcpListener>,
    /// The port both addresses listen on.
    pub(crate) port: u16,
    /// The request path a response arrives on — `/callback`,
    /// `/auth/callback`.
    path: &'static str,
}

impl CallbackServer {
    /// Bind the first of `ports` free on loopback (`0` asks for an ephemeral
    /// one), answering responses on `path`. A port free on IPv4 but taken on
    /// IPv6 alone is kept as the fallback while the rest are tried.
    pub(crate) fn bind(ports: &[u16], path: &'static str) -> Result<Self, String> {
        let mut v4_only = None;
        let mut last_error = None;
        for &port in ports {
            let v4 = match TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)) {
                Ok(listener) => listener,
                Err(e) => {
                    last_error = Some(e);
                    continue;
                }
            };
            let port = match v4.local_addr() {
                Ok(address) => address.port(),
                Err(e) => {
                    last_error = Some(e);
                    continue;
                }
            };
            match TcpListener::bind((std::net::Ipv6Addr::LOCALHOST, port)) {
                Ok(v6) => {
                    return Ok(Self {
                        listeners: vec![v4, v6],
                        port,
                        path,
                    });
                }
                Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
                    v4_only.get_or_insert(Self {
                        listeners: vec![v4],
                        port,
                        path,
                    });
                }
                // No IPv6 loopback here, so `localhost` can only mean
                // 127.0.0.1.
                Err(_) => {
                    return Ok(Self {
                        listeners: vec![v4],
                        port,
                        path,
                    });
                }
            }
        }
        v4_only.ok_or_else(|| {
            format!(
                "could not open the callback port: {}",
                last_error.map_or_else(|| "no port was free".to_string(), |e| e.to_string())
            )
        })
    }

    /// How many addresses are listening — two where IPv6 loopback exists.
    #[cfg(test)]
    pub(crate) fn addresses(&self) -> usize {
        self.listeners.len()
    }
}

/// Why a wait ended without a code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WaitEnd {
    /// The flow's cancel token fired (Esc).
    Cancelled,
    /// Nothing came back in time.
    TimedOut,
    /// The authorization server refused the sign-in, in its words.
    Refused(String),
    /// The callback port failed.
    Port(String),
}

impl std::fmt::Display for WaitEnd {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => f.write_str("cancelled"),
            Self::TimedOut => f.write_str("timed out waiting for the browser to come back"),
            Self::Refused(why) => f.write_str(why),
            Self::Port(why) => write!(f, "callback port: {why}"),
        }
    }
}

impl From<WaitEnd> for crate::llm::LlmError {
    fn from(end: WaitEnd) -> Self {
        match end {
            WaitEnd::Cancelled => Self::Cancelled,
            other => Self::Http(other.to_string()),
        }
    }
}

/// A pasted redirect URL arriving beside the callback — the headless path —
/// and who hears why one was turned away.
pub(crate) type Paste<'a> = (&'a mpsc::Receiver<String>, &'a dyn Fn(&str));

/// A callback request for the flow to judge, and where its page goes.
struct CallbackHit {
    response: AuthResponse,
    page: mpsc::Sender<String>,
}

/// Wait up to `timeout` for the authorization code from **either** side:
/// the loopback callback, or a pasted redirect URL. Each response is checked
/// ([`check_auth_response`]) before its code is ever redeemed: one that isn't
/// this sign-in's is turned away and the wait goes on — a stray request, or
/// a forged one, can't end it — while this sign-in's refusal ends it with
/// the authorization server's reason.
pub(crate) fn wait_for_code(
    server: &CallbackServer,
    expected: &Expected<'_>,
    pages: &Pages,
    timeout: Duration,
    paste: Option<Paste<'_>>,
    cancel: &CancelToken,
) -> Result<String, WaitEnd> {
    for listener in &server.listeners {
        listener
            .set_nonblocking(true)
            .map_err(|e| WaitEnd::Port(e.to_string()))?;
    }
    let (hits_tx, hits_rx) = mpsc::channel::<CallbackHit>();
    let serving = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let started = Instant::now();
    loop {
        if cancel.is_cancelled() {
            return Err(WaitEnd::Cancelled);
        }
        if started.elapsed() > timeout {
            return Err(WaitEnd::TimedOut);
        }
        if let Some((paste_rx, rejected)) = paste {
            while let Ok(text) = paste_rx.try_recv() {
                let Some(response) = parse_auth_response(&text) else {
                    rejected(
                        "that isn't the address the browser landed on — paste the whole URL, \
                         with its code and state",
                    );
                    continue;
                };
                match check_auth_response(&response, expected) {
                    Ok(code) => return Ok(code),
                    Err(Rejected::Refused(refusal)) => {
                        return Err(WaitEnd::Refused(refusal.to_string()));
                    }
                    Err(Rejected::Foreign(why)) => rejected(&why),
                }
            }
        }
        // New browser connections, each served on a thread of its own.
        for listener in &server.listeners {
            loop {
                match listener.accept() {
                    Ok((stream, _)) => {
                        use std::sync::atomic::Ordering;
                        if serving.load(Ordering::SeqCst) >= MAX_CONNECTIONS {
                            continue; // closed unread
                        }
                        serving.fetch_add(1, Ordering::SeqCst);
                        let hits = hits_tx.clone();
                        let serving = std::sync::Arc::clone(&serving);
                        let (path, pages) = (server.path, *pages);
                        std::thread::spawn(move || {
                            serve_connection(stream, path, &pages, &hits);
                            serving.fetch_sub(1, Ordering::SeqCst);
                        });
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(e)
                        if matches!(
                            e.kind(),
                            std::io::ErrorKind::ConnectionAborted
                                | std::io::ErrorKind::ConnectionReset
                                | std::io::ErrorKind::Interrupted
                        ) => {}
                    Err(e) => return Err(WaitEnd::Port(e.to_string())),
                }
            }
        }
        // The requests those threads read, judged here.
        while let Ok(hit) = hits_rx.try_recv() {
            let verdict = check_auth_response(&hit.response, expected);
            let _ = hit.page.send(callback_page(&verdict, pages));
            match verdict {
                Ok(code) => return Ok(code),
                Err(Rejected::Refused(refusal)) => {
                    return Err(WaitEnd::Refused(refusal.to_string()));
                }
                Err(Rejected::Foreign(_)) => {}
            }
        }
        std::thread::sleep(POLL);
    }
}

/// Serve one browser connection on its own thread: read the request head,
/// hand an authorization response on `path` to the flow and write the page
/// it decides, answer anything else itself. A connection that never sends —
/// a browser preconnect — costs this thread its read timeout and nothing
/// else.
fn serve_connection(
    mut stream: std::net::TcpStream,
    path: &str,
    pages: &Pages,
    hits: &mpsc::Sender<CallbackHit>,
) {
    // An accepted socket inherits the listener's non-blocking mode on BSD
    // and macOS (never on Linux), and a non-blocking read of a request still
    // on its way reads nothing.
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(READ_TIMEOUT));
    let _ = stream.set_write_timeout(Some(READ_TIMEOUT));
    let head = read_request_head(&mut stream);
    let target = head
        .lines()
        .next()
        .and_then(|line| {
            let mut words = line.split_whitespace();
            (words.next() == Some("GET")).then(|| words.next())?
        })
        .filter(|target| target.split(['?', '#']).next() == Some(path));
    let reply = match target.map(parse_auth_response) {
        None => stray_page(Stray::Elsewhere, pages),
        Some(None) => stray_page(Stray::NoAnswer, pages),
        Some(Some(response)) => {
            let (page_tx, page_rx) = mpsc::channel();
            let sent = hits.send(CallbackHit {
                response,
                page: page_tx,
            });
            sent.ok()
                .and_then(|()| page_rx.recv_timeout(READ_TIMEOUT).ok())
                .unwrap_or_else(|| stray_page(Stray::Over, pages))
        }
    };
    let _ = stream.write_all(reply.as_bytes());
    let _ = stream.flush();
}

/// Read a request head — to its blank line, the cap, or the timeout. The
/// whole head, not just the request line: closing a socket with unread
/// bytes resets it, and a reset can discard the page before the browser
/// reads it.
fn read_request_head(stream: &mut impl Read) -> String {
    let mut head = Vec::new();
    let mut chunk = [0u8; 2048];
    while head.len() < HEAD_MAX && !head.windows(4).any(|w| w == b"\r\n\r\n") {
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => head.extend_from_slice(&chunk[..n]),
        }
    }
    String::from_utf8_lossy(&head).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGES: Pages = Pages {
        received: "Authorization received",
        restart: "/mcp",
    };

    fn expected(issuer: Option<&'static str>, iss_required: bool) -> Expected<'static> {
        Expected {
            state: "s1",
            issuer,
            iss_required,
        }
    }

    #[test]
    fn auth_responses_parse_codes_refusals_and_their_state() {
        let response =
            parse_auth_response("/callback?code=abc&state=xyz&iss=https%3A%2F%2Fas.test").unwrap();
        assert_eq!(response.code.as_deref(), Some("abc"));
        assert_eq!(response.state.as_deref(), Some("xyz"));
        assert_eq!(response.iss.as_deref(), Some("https://as.test"));
        let refusal = parse_auth_response(
            "http://localhost:1/callback?error=access_denied&error_description=User%20said%20no&state=xyz#x",
        )
        .unwrap();
        assert_eq!(refusal.error.as_deref(), Some("access_denied"));
        assert_eq!(refusal.error_description.as_deref(), Some("User said no"));
        assert_eq!(refusal.code, None);
        // Neither a code nor an error: not an authorization response.
        assert_eq!(parse_auth_response("/callback?foo=bar"), None);
        assert_eq!(parse_auth_response("/callback?code=&state=x"), None);
        assert_eq!(parse_auth_response("/favicon.ico"), None);
        assert_eq!(parse_auth_response("not a url at all"), None);
    }

    /// A response is this flow's only when it carries the flow's own
    /// `state` and names the right issuer; only then can it end the flow —
    /// with its code, or with the authorization server's refusal.
    #[test]
    fn an_auth_response_counts_only_with_its_state_and_issuer() {
        let expected = expected(Some("https://as.test"), true);
        let check =
            |text: &str| check_auth_response(&parse_auth_response(text).unwrap(), &expected);
        assert_eq!(
            check("?code=c&state=s1&iss=https%3A%2F%2Fas.test"),
            Ok("c".to_string())
        );
        // No state: it can't be told from a forged one.
        assert!(matches!(
            check("?code=c&iss=https%3A%2F%2Fas.test"),
            Err(Rejected::Foreign(_))
        ));
        assert!(matches!(
            check("?code=c&state=s2&iss=https%3A%2F%2Fas.test"),
            Err(Rejected::Foreign(_))
        ));
        // Another authorization server's response (RFC 9207), or none named
        // where the metadata promised one.
        assert!(matches!(
            check("?code=c&state=s1&iss=https%3A%2F%2Fevil.test"),
            Err(Rejected::Foreign(_))
        ));
        assert!(matches!(
            check("?code=c&state=s1"),
            Err(Rejected::Foreign(_))
        ));
        // A refusal is this flow's answer only under the same checks.
        match check(
            "?error=access_denied&error_description=no%20thanks&state=s1&iss=https%3A%2F%2Fas.test",
        ) {
            Err(Rejected::Refused(refusal)) => {
                assert_eq!(refusal.error, "access_denied");
                assert_eq!(refusal.description.as_deref(), Some("no thanks"));
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            check("?error=access_denied"),
            Err(Rejected::Foreign(_))
        ));
        // A flow with no issuer to hold `iss` to takes the state alone.
        let plain = Expected {
            state: "s1",
            issuer: None,
            iss_required: false,
        };
        assert_eq!(
            check_auth_response(&parse_auth_response("?code=c&state=s1").unwrap(), &plain),
            Ok("c".to_string())
        );
    }

    /// What the flow and the terminal say of a refusal reads as it always
    /// has — the code, then the description when there is one.
    #[test]
    fn a_refusal_reads_as_the_servers_words() {
        let refusal = Refusal {
            error: "access_denied".to_string(),
            description: Some("no thanks".to_string()),
        };
        assert_eq!(
            refusal.to_string(),
            "the authorization server refused the sign-in: access_denied (no thanks)"
        );
        let bare = Refusal {
            description: None,
            ..refusal
        };
        assert_eq!(
            bare.to_string(),
            "the authorization server refused the sign-in: access_denied"
        );
    }

    /// A reason written for the terminal reads as a sentence on the page.
    #[test]
    fn a_reason_becomes_a_sentence() {
        assert_eq!(
            sentence("the response carries no code"),
            "The response carries no code."
        );
        assert_eq!(sentence("Already one."), "Already one.");
        assert_eq!(sentence("évidemment"), "Évidemment.");
        assert_eq!(sentence(""), "");
    }

    /// Each verdict's page says what happened, in the flow's own words and
    /// the page's tone, and names the app to return to.
    #[test]
    fn callback_pages_say_what_happened() {
        let ok = callback_page(&Ok("c".to_string()), &PAGES);
        assert!(ok.starts_with("HTTP/1.1 200"), "{ok}");
        assert!(ok.contains("<h1>Authorization received</h1>"), "{ok}");
        assert!(
            ok.contains(&format!(
                "You can close this tab and return to {}.",
                crate::APP_NAME
            )),
            "{ok}"
        );
        assert!(ok.contains(r#"<main class="success">"#), "{ok}");

        let refused = callback_page(
            &Err(Rejected::Refused(Refusal {
                error: "access_denied".to_string(),
                description: Some("no <script>alert(1)</script>".to_string()),
            })),
            &PAGES,
        );
        assert!(refused.starts_with("HTTP/1.1 200"), "{refused}");
        assert!(refused.contains("<h1>Sign-in refused</h1>"), "{refused}");
        assert!(refused.contains(r#"<main class="refused">"#), "{refused}");
        assert!(
            refused.contains(&format!("Return to {} to try again.", crate::APP_NAME)),
            "{refused}"
        );
        assert!(
            refused.contains(
                r#"<div class="detail"><code>access_denied</code>no &lt;script&gt;alert(1)&lt;/script&gt;</div>"#
            ),
            "{refused}"
        );
        assert!(!refused.contains("<script>"), "{refused}");

        let foreign = callback_page(
            &Err(Rejected::Foreign(
                "the response belongs to another sign-in attempt".to_string(),
            )),
            &PAGES,
        );
        assert!(foreign.starts_with("HTTP/1.1 400"), "{foreign}");
        assert!(foreign.contains(r#"<main class="notice">"#), "{foreign}");
        assert!(
            foreign.contains(&format!(
                "The response belongs to another sign-in attempt. To sign in, start again \
                 from <code>/mcp</code> in {}.",
                crate::APP_NAME
            )),
            "{foreign}"
        );
    }

    /// A request that is not this sign-in's answer gets a page of its own:
    /// nothing failed, so none of them is an error, and only the one that
    /// came too late has a sign-in to start again.
    #[test]
    fn stray_requests_get_pages_of_their_own() {
        let elsewhere = stray_page(Stray::Elsewhere, &PAGES);
        assert!(elsewhere.starts_with("HTTP/1.1 404"), "{elsewhere}");
        assert!(elsewhere.contains("<h1>Nothing here</h1>"), "{elsewhere}");
        let no_answer = stray_page(Stray::NoAnswer, &PAGES);
        assert!(no_answer.starts_with("HTTP/1.1 400"), "{no_answer}");
        assert!(
            no_answer.contains("<h1>Not a sign-in response</h1>"),
            "{no_answer}"
        );
        let over = stray_page(Stray::Over, &PAGES);
        assert!(over.starts_with("HTTP/1.1 410"), "{over}");
        assert!(over.contains("<h1>This sign-in has ended</h1>"), "{over}");
        assert!(over.contains("<code>/mcp</code>"), "{over}");
        for page in [&elsewhere, &no_answer, &over] {
            assert!(page.contains(r#"<main class="notice">"#), "{page}");
        }
        assert!(!elsewhere.contains("<code>"), "{elsewhere}");
        assert!(!no_answer.contains("<code>"), "{no_answer}");
    }

    /// Send one raw request to the callback port and read the whole reply.
    fn hit(port: u16, request: &str) -> String {
        let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect");
        stream.write_all(request.as_bytes()).unwrap();
        let mut reply = String::new();
        let _ = stream.read_to_string(&mut reply);
        reply
    }

    type Rejections = std::sync::Arc<std::sync::Mutex<Vec<String>>>;

    /// Wait for the code on a thread; the receiver yields its result.
    fn waiting(
        server: CallbackServer,
        paste_rx: Option<mpsc::Receiver<String>>,
        rejected: Rejections,
    ) -> mpsc::Receiver<Result<String, WaitEnd>> {
        let (done_tx, done_rx) = mpsc::channel();
        std::thread::spawn(move || {
            let note = |reason: &str| rejected.lock().unwrap().push(reason.to_string());
            let paste = paste_rx.as_ref().map(|rx| (rx, &note as &dyn Fn(&str)));
            let result = wait_for_code(
                &server,
                &expected(None, false),
                &PAGES,
                Duration::from_secs(30),
                paste,
                &CancelToken::new(),
            );
            let _ = done_tx.send(result);
        });
        done_rx
    }

    fn bound() -> CallbackServer {
        CallbackServer::bind(&[0], "/callback").expect("bind")
    }

    /// A browser preconnect — a connection that never sends a byte — must
    /// not stall the callback: the real request on the next connection is
    /// answered at once.
    #[test]
    fn an_idle_connection_never_stalls_the_callback() {
        let server = bound();
        let port = server.port;
        let done = waiting(server, None, Default::default());
        let _idle = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        std::thread::sleep(Duration::from_millis(150));
        let reply = hit(
            port,
            "GET /callback?code=c1&state=s1 HTTP/1.1\r\nHost: localhost\r\n\r\n",
        );
        assert!(reply.starts_with("HTTP/1.1 200"), "{reply}");
        assert_eq!(
            done.recv_timeout(Duration::from_secs(3)).expect("answered"),
            Ok("c1".to_string())
        );
    }

    /// Stray requests — a favicon, another path, a response from another
    /// sign-in, one with no state — are answered and turned away without
    /// ending the flow; the real one still lands.
    #[test]
    fn a_stray_request_never_ends_the_sign_in() {
        let server = bound();
        let port = server.port;
        let done = waiting(server, None, Default::default());
        let favicon = hit(port, "GET /favicon.ico HTTP/1.1\r\n\r\n");
        assert!(favicon.starts_with("HTTP/1.1 404"), "{favicon}");
        assert!(favicon.contains("<h1>Nothing here</h1>"), "{favicon}");
        let elsewhere = hit(port, "GET /other?code=c1&state=s1 HTTP/1.1\r\n\r\n");
        assert!(elsewhere.starts_with("HTTP/1.1 404"), "{elsewhere}");
        let foreign = hit(port, "GET /callback?code=evil&state=s2 HTTP/1.1\r\n\r\n");
        assert!(foreign.starts_with("HTTP/1.1 400"), "{foreign}");
        let stateless = hit(port, "GET /callback?code=evil HTTP/1.1\r\n\r\n");
        assert!(stateless.starts_with("HTTP/1.1 400"), "{stateless}");
        let forged_refusal = hit(port, "GET /callback?error=access_denied HTTP/1.1\r\n\r\n");
        assert!(
            forged_refusal.starts_with("HTTP/1.1 400"),
            "{forged_refusal}"
        );
        assert!(
            done.recv_timeout(Duration::from_millis(200)).is_err(),
            "the flow ended early"
        );
        let real = hit(port, "GET /callback?code=c1&state=s1 HTTP/1.1\r\n\r\n");
        assert!(real.starts_with("HTTP/1.1 200"), "{real}");
        assert_eq!(
            done.recv_timeout(Duration::from_secs(3)).unwrap(),
            Ok("c1".to_string())
        );
    }

    /// A user who declines on the consent screen ends the sign-in at once
    /// with the authorization server's reason, instead of a long wait for a
    /// code that will never come.
    #[test]
    fn a_refusal_ends_the_sign_in_with_its_reason() {
        let server = bound();
        let port = server.port;
        let done = waiting(server, None, Default::default());
        let page = hit(
            port,
            "GET /callback?error=access_denied&error_description=The%20user%20declined&state=s1 HTTP/1.1\r\n\r\n",
        );
        assert!(page.contains("access_denied"), "{page}");
        match done.recv_timeout(Duration::from_secs(3)).unwrap() {
            Err(WaitEnd::Refused(why)) => {
                assert!(
                    why.contains("access_denied") && why.contains("The user declined"),
                    "{why}"
                );
            }
            other => panic!("{other:?}"),
        }
    }

    /// `localhost` may resolve to `::1` first; the callback answers there
    /// too, on the same port, wherever this machine has IPv6 loopback.
    #[test]
    fn the_callback_answers_on_both_loopback_addresses() {
        let server = bound();
        let port = server.port;
        if std::net::TcpListener::bind(("::1", 0)).is_err() {
            return; // no IPv6 loopback on this machine
        }
        assert_eq!(
            server.addresses(),
            2,
            "IPv6 loopback is available but unbound"
        );
        let done = waiting(server, None, Default::default());
        let mut stream = std::net::TcpStream::connect(("::1", port)).expect("connect over IPv6");
        stream
            .write_all(b"GET /callback?code=c6&state=s1 HTTP/1.1\r\n\r\n")
            .unwrap();
        let mut reply = String::new();
        let _ = stream.read_to_string(&mut reply);
        assert!(reply.starts_with("HTTP/1.1 200"), "{reply}");
        assert_eq!(
            done.recv_timeout(Duration::from_secs(3)).unwrap(),
            Ok("c6".to_string())
        );
    }

    /// A pasted URL that isn't this sign-in's — another attempt's, or not a
    /// redirect at all — is turned away with a reason, and the flow keeps
    /// waiting for the right one.
    #[test]
    fn a_wrong_paste_is_turned_away_not_fatal() {
        let server = bound();
        let (paste_tx, paste_rx) = mpsc::channel();
        let rejected: Rejections = Default::default();
        let done = waiting(server, Some(paste_rx), std::sync::Arc::clone(&rejected));
        paste_tx
            .send("http://localhost:1/callback?code=old&state=s0".to_string())
            .unwrap();
        paste_tx.send("hello".to_string()).unwrap();
        std::thread::sleep(Duration::from_millis(300));
        assert!(done.try_recv().is_err(), "a wrong paste ended the flow");
        assert_eq!(rejected.lock().unwrap().len(), 2);
        paste_tx
            .send("http://localhost:1/callback?code=new&state=s1".to_string())
            .unwrap();
        assert_eq!(
            done.recv_timeout(Duration::from_secs(3)).unwrap(),
            Ok("new".to_string())
        );
    }

    /// The first free port of the list is the one taken — what ChatGPT's
    /// allow-listed pair needs — and a list with none free says so.
    #[test]
    fn binding_takes_the_first_free_port() {
        let taken = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let busy = taken.local_addr().unwrap().port();
        let server = CallbackServer::bind(&[busy, 0], "/callback").expect("the second");
        assert_ne!(server.port, busy);
        assert!(CallbackServer::bind(&[busy], "/callback").is_err());
    }
}
