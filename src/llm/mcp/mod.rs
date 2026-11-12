//! The MCP **I/O boundary** (`docs/tools/mcp.md`): the three transports, the
//! per-server connect sequence, the OAuth flow, and the session-owned
//! [`McpManager`] registry. The pure protocol/config/naming vocabulary this
//! drives is [`crate::mcp`]; this module is what actually spawns processes,
//! speaks HTTP, and holds live connections.
//!
//! Verified hermetically: the tests spawn scripted `sh` stdio servers and
//! `std::net::TcpListener` HTTP fixtures — no network.

mod client;
mod files;
mod manager;
mod oauth;
mod transport;

pub use self::client::{ConnectError, Connection, connect, connect_with_helper};
pub use self::files::write_config_file;
pub use self::manager::{
    DEFAULT_STARTUP_TIMEOUT, DEFAULT_TOOL_TIMEOUT, McpEvent, McpHold, McpManager, McpSources,
};
pub use self::oauth::{
    AUTH_FLOW_TIMEOUT, AuthProgress, AuthServerMeta, ClientIdentity, RefreshFailure, SignIn,
    StoredTokens, b64url, build_authorize_url, challenge_resource_metadata, connect_bearer,
    discover, form_encode, load_client_secret, load_tokens, parse_redirect, parse_token_store,
    pkce_and_state, provider_authorize_params, record_tokens, redirect_param, refresh_failure_kind,
    refresh_grant, refresh_if_stale, refresh_params, register_client, run_auth_flow,
    save_client_secret, save_tokens, tokens_fresh, url_decode, url_encode, validate_issuer,
    with_offline_access,
};
pub use self::transport::{
    HttpTransport, RemoteHeaders, SseTransport, StdioTransport, Transport, TransportError,
};
