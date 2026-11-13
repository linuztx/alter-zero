//! Manual check of what an MCP server actually speaks (`docs/mcp.md`) — the
//! real connect sequence (era probe → handshake → `tools/list`), printing the
//! revision the server itself settled on. NOT run by `cargo test` (it needs a
//! server on the network or on disk).
//!
//! Usage — a URL, or a command line, exactly like `alter-zero mcp add`:
//! ```bash
//! cargo run --example mcp_probe -- https://docs.mcp.cloudflare.com/mcp
//! cargo run --example mcp_probe -- npx -y @modelcontextprotocol/server-everything
//! ```
//! This is the answer to "the `/mcp` page says a revision I don't expect":
//! whatever it prints is what the server replied to this client, now.
//!
//! A server that wants a sign-in gets the sign-in's own discovery run too —
//! the metadata documents only, read-only: nothing is registered and no
//! browser opens — so "why can't I authenticate?" is answered the same way:
//! the authorization server found, where it signs in and trades the code,
//! whether it registers clients itself, and the scopes that would be asked
//! for.

use std::time::Duration;

use alter_zero::llm::mcp::{ConnectError, connect, discover};
use alter_zero::mcp::McpServerConfig;
use alter_zero::stream::CancelToken;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(first) = args.first() else {
        eprintln!("usage: mcp_probe <url|command [args…]>");
        std::process::exit(2);
    };
    let config = if first.starts_with("http://") || first.starts_with("https://") {
        McpServerConfig::Http {
            url: first.clone(),
            headers: Default::default(),
            // The spec's compat recipe: a refused streamable POST retries the
            // same URL as a legacy SSE server.
            sse_fallback: true,
            oauth: None,
        }
    } else {
        McpServerConfig::Stdio {
            command: first.clone(),
            args: args[1..].to_vec(),
            env: Default::default(),
        }
    };
    let started = std::time::Instant::now();
    match connect(
        &config,
        None,
        None,
        Duration::from_secs(30),
        &CancelToken::new(),
    ) {
        Ok(connection) => {
            let identity = &connection.identity;
            println!("server       : {} {}", identity.name, identity.version);
            println!("protocol     : {}", identity.protocol_version);
            println!("capabilities : {}", identity.capabilities.join(", "));
            println!("tools        : {}", connection.tools.len());
            for tool in &connection.tools {
                println!("  · {}", tool.name);
            }
        }
        Err(ConnectError::NeedsAuth(challenge)) => {
            println!(
                "needs authentication: {}",
                challenge.as_deref().unwrap_or("(no challenge sent)")
            );
            sign_in_discovery(first, challenge.as_deref());
        }
        Err(ConnectError::Failed(detail)) => println!("failed: {detail}"),
    }
    println!("connected in : {:?}", started.elapsed());
}

/// What a sign-in to `url` would find — the discovery `/mcp`'s Authenticate
/// runs, and nothing after it.
fn sign_in_discovery(url: &str, challenge: Option<&str>) {
    match discover(url, challenge) {
        Ok(meta) => {
            println!(
                "  issuer       : {}",
                meta.issuer.as_deref().unwrap_or("(none named)")
            );
            println!("  authorize    : {}", meta.authorization_endpoint);
            println!("  token        : {}", meta.token_endpoint);
            println!(
                "  registration : {}",
                meta.registration_endpoint
                    .as_deref()
                    .unwrap_or("none — signing in needs a pre-registered client (oauth.clientId)")
            );
            println!(
                "  scopes       : {}",
                meta.scopes.as_deref().unwrap_or("(none asked for)")
            );
            println!("  resource     : {}", meta.resource);
        }
        Err(detail) => println!("  sign-in discovery failed: {detail}"),
    }
}
