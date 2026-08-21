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

use std::time::Duration;

use alter_zero::llm::mcp::{ConnectError, connect};
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
}
