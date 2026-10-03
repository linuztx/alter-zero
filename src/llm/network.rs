//! Telling a request that never left the machine from one that failed
//! anywhere else — the pure half of the offline wait (`docs/offline.md`).
//!
//! `reqwest`'s `Display` is `error sending request for url (…)` whatever went
//! wrong; the cause lives only in `source()`. So the two questions the
//! boundary asks of a transport failure — *was the network simply not there?*
//! and *what should the user read?* — are both answered by walking that
//! chain, here, over `&dyn Error`, so hand-built chains can stand in for the
//! real ones in the tests.

use std::error::Error;

/// Did this transport failure happen **before any connection existed**,
/// because the network is not there? A DNS lookup that failed, no route to the
/// host, a network that is down, no local address to send from, or a connect
/// that timed out — every case in which not one byte of the request left the
/// machine, which is what makes re-sending it safe for as long as it takes.
///
/// `connect` and `timeout` are `reqwest`'s own `is_connect()` /
/// `is_timeout()` verdicts, passed in so this stays pure: a failure outside
/// the connect phase is never offline (the request may have reached the
/// provider), and a connect phase that timed out is (`docs/offline.md`).
/// A refused connection is **not** offline: the host answered, so the
/// network works and the problem is the server.
#[must_use]
pub fn is_offline(connect: bool, timeout: bool, err: &(dyn Error + 'static)) -> bool {
    if !connect {
        return false;
    }
    timeout || chain(err).any(|link| is_failed_lookup(link) || is_unreachable(link))
}

/// Did the request fail with its streamed body **unsent** — the connection
/// gone before the upload finished, for a reason the error does not say?
/// `reqwest`'s blocking client pumps a streamed body itself, and when the
/// connection fails mid-upload it reports the pump's broken channel (`send
/// failed because receiver is gone`) in place of the connection's own error.
/// That is what an offline request with a sizeable conversation looks like —
/// the DNS failure behind it never surfaces — so the transport answers this
/// one by trying a fresh connection to see (`docs/offline.md`).
#[must_use]
pub fn is_unsent_body(err: &(dyn Error + 'static)) -> bool {
    chain(err).any(|link| link.to_string() == "send failed because receiver is gone")
}

/// The failure as one line: the top error and every cause under it,
/// `: `-joined — `error sending request for url (…): client error (Connect):
/// tcp connect error: Connection refused (os error 111)` — since the top
/// line alone names nothing. A cause whose text its parent already spelled
/// out is skipped, so an error that does print its source is not repeated.
#[must_use]
pub fn describe(err: &(dyn Error + 'static)) -> String {
    let mut line = String::new();
    for link in chain(err) {
        let text = link.to_string();
        let text = text.trim();
        if text.is_empty() || line.contains(text) {
            continue;
        }
        if !line.is_empty() {
            line.push_str(": ");
        }
        line.push_str(text);
    }
    line
}

/// `err` and every cause under it, top first.
fn chain<'a>(err: &'a (dyn Error + 'static)) -> impl Iterator<Item = &'a (dyn Error + 'static)> {
    std::iter::successors(Some(err), |&link| link.source())
}

/// A DNS lookup that got no answer: `hyper-util`'s `dns error` link, or the
/// system resolver's own `failed to lookup address information` under it.
fn is_failed_lookup(link: &(dyn Error + 'static)) -> bool {
    let text = link.to_string();
    text.starts_with("dns error") || text.starts_with("failed to lookup address information")
}

/// A socket error saying there is no network path at all — the kinds the
/// standard library maps `ENETUNREACH`, `EHOSTUNREACH`, `ENETDOWN` and
/// `EADDRNOTAVAIL` to on every platform it names them for.
fn is_unreachable(link: &(dyn Error + 'static)) -> bool {
    link.downcast_ref::<std::io::Error>().is_some_and(|io| {
        matches!(
            io.kind(),
            std::io::ErrorKind::NetworkUnreachable
                | std::io::ErrorKind::HostUnreachable
                | std::io::ErrorKind::NetworkDown
                | std::io::ErrorKind::AddrNotAvailable
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt;
    use std::io::{self, ErrorKind};

    /// One link of a hand-built error chain — the shape `hyper-util`'s
    /// connector errors take: a message over an optional cause.
    #[derive(Debug)]
    struct Link {
        text: &'static str,
        source: Option<Box<dyn Error + 'static>>,
    }

    impl fmt::Display for Link {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(self.text)
        }
    }

    impl Error for Link {
        fn source(&self) -> Option<&(dyn Error + 'static)> {
            self.source.as_deref()
        }
    }

    fn link(text: &'static str, source: impl Error + 'static) -> Link {
        Link {
            text,
            source: Some(Box::new(source)),
        }
    }

    fn leaf(text: &'static str) -> Link {
        Link { text, source: None }
    }

    /// `reqwest`'s top line over `hyper-util`'s connect wrapper, the way
    /// every connect-phase failure arrives (measured, `docs/offline.md`).
    fn connect_failure(cause: impl Error + 'static) -> Link {
        link(
            "error sending request for url (https://api.example.com/v1/chat/completions)",
            link("client error (Connect)", cause),
        )
    }

    fn dns_failure(resolver_says: &'static str) -> Link {
        connect_failure(link(
            "dns error",
            io::Error::other(format!(
                "failed to lookup address information: {resolver_says}"
            )),
        ))
    }

    #[test]
    fn a_failed_dns_lookup_is_offline() {
        // Linux with no network answers EAI_AGAIN; macOS answers EAI_NONAME
        // for the same thing — either way the lookup never got an answer.
        for says in [
            "Temporary failure in name resolution",
            "nodename nor servname provided, or not known",
        ] {
            assert!(is_offline(true, false, &dns_failure(says)), "{says}");
        }
    }

    #[test]
    fn a_failed_dns_lookup_for_the_proxy_is_offline_too() {
        // Through `HTTPS_PROXY` the lookup that fails is the proxy's, one
        // tunnel link further down.
        let chain = connect_failure(link(
            "tunnel error: failed to create underlying connection",
            link(
                "dns error",
                io::Error::other("failed to lookup address information: Name or service not known"),
            ),
        ));
        assert!(is_offline(true, false, &chain));
    }

    #[test]
    fn no_route_a_downed_network_and_no_local_address_are_offline() {
        for kind in [
            ErrorKind::NetworkUnreachable,
            ErrorKind::HostUnreachable,
            ErrorKind::NetworkDown,
            ErrorKind::AddrNotAvailable,
        ] {
            let chain = connect_failure(link("tcp connect error", io::Error::from(kind)));
            assert!(is_offline(true, false, &chain), "{kind:?}");
        }
    }

    #[test]
    fn a_connect_that_timed_out_is_offline() {
        let chain = connect_failure(link(
            "tcp connect error",
            io::Error::new(ErrorKind::TimedOut, "deadline has elapsed"),
        ));
        assert!(is_offline(true, true, &chain));
    }

    #[test]
    fn a_refused_connection_is_not_offline() {
        // The host sent the RST itself: the network works, the server does
        // not — an Ollama that isn't running keeps its advice.
        let chain = connect_failure(link(
            "tcp connect error",
            io::Error::from(ErrorKind::ConnectionRefused),
        ));
        assert!(!is_offline(true, false, &chain));
    }

    #[test]
    fn a_tls_failure_is_not_offline() {
        let chain = connect_failure(io::Error::other("invalid peer certificate: UnknownIssuer"));
        assert!(!is_offline(true, false, &chain));
    }

    #[test]
    fn nothing_outside_the_connect_phase_is_offline() {
        // A read that stalled or failed after the headers may have reached
        // the provider — never waited out, whatever its chain says.
        assert!(!is_offline(
            false,
            true,
            &dns_failure("Temporary failure in name resolution")
        ));
        let reset = link(
            "error decoding response body",
            io::Error::from(ErrorKind::NetworkUnreachable),
        );
        assert!(!is_offline(false, false, &reset));
    }

    #[test]
    fn a_body_the_connection_never_took_is_an_unsent_body() {
        // Measured: `reqwest`'s blocking client pumps a streamed body itself,
        // and when the connection fails before the upload is done it reports
        // the pump's broken channel — not the connection's error.
        let chain = link(
            "request or response body error for url (https://api.example.com/v1/chat/completions)",
            leaf("send failed because receiver is gone"),
        );
        assert!(is_unsent_body(&chain));
    }

    #[test]
    fn other_failures_are_not_an_unsent_body() {
        assert!(!is_unsent_body(&dns_failure("Name or service not known")));
        let reset = link(
            "error decoding response body",
            io::Error::from(ErrorKind::ConnectionReset),
        );
        assert!(!is_unsent_body(&reset));
    }

    #[test]
    fn describe_names_every_cause_under_the_top_line() {
        assert_eq!(
            describe(&dns_failure("Name or service not known")),
            "error sending request for url (https://api.example.com/v1/chat/completions): \
             client error (Connect): dns error: \
             failed to lookup address information: Name or service not known"
        );
    }

    #[test]
    fn describe_skips_a_cause_its_parent_already_spelled_out() {
        let chain = link(
            "request failed: connection reset by peer",
            leaf("connection reset by peer"),
        );
        assert_eq!(describe(&chain), "request failed: connection reset by peer");
    }

    #[test]
    fn describe_of_a_lone_error_is_its_own_text() {
        assert_eq!(describe(&leaf("builder error")), "builder error");
    }
}
