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
//! whatever it prints is what the server replied to this client, now. For a
//! remote server it also walks the OAuth discovery a sign-in would — the
//! answer to "why does this server need a client of my own?" (`docs/mcp.md`
//! *A pre-registered client*).

use std::time::Duration;

use alter_zero::llm::mcp::{
    ConnectError, connect, discover_auth_server, discover_resource, offers_oauth,
};
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
            println!("needs authentication: {}", challenge.unwrap_or_default());
        }
        Err(ConnectError::Failed(detail)) => println!("failed: {detail}"),
    }
    println!("connected in : {:?}", started.elapsed());
    if let Some(url) = config.url() {
        oauth_story(url);
    }
}

/// What a sign-in to `url` would find: the protected resource's metadata,
/// its authorization server, and how a client identity could be had there.
fn oauth_story(url: &str) {
    println!(
        "sign-in      : {}",
        if offers_oauth(url) {
            "offered"
        } else {
            "not offered"
        }
    );
    let resource = match discover_resource(url, None) {
        Ok(Some(resource)) => resource,
        Ok(None) => {
            println!("resource     : no protected-resource metadata");
            return;
        }
        Err(e) => {
            println!("resource     : {e}");
            return;
        }
    };
    println!("resource     : {}", resource.resource);
    println!(
        "scopes       : {}",
        resource.scopes.as_deref().unwrap_or("(none listed)")
    );
    let issuer = resource.authorization_servers.first().map(String::as_str);
    match discover_auth_server(issuer, url, None) {
        Ok(meta) => {
            println!(
                "issuer       : {}",
                meta.issuer.as_deref().unwrap_or("(unnamed)")
            );
            println!("authorize    : {}", meta.authorization_endpoint);
            println!("token        : {}", meta.token_endpoint);
            println!(
                "registration : {}",
                meta.registration_endpoint.as_deref().unwrap_or(
                    "none — needs a pre-registered client (\"oauth\": {\"clientId\": …})"
                )
            );
            println!("client auth  : {}", meta.token_auth_methods.join(", "));
        }
        Err(e) => println!("auth server  : {e}"),
    }
}
