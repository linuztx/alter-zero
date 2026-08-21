//! MCP (Model Context Protocol) — the **pure** half (`docs/mcp.md`).
//!
//! Everything the boundary client ([`crate::llm::mcp`]) and the `/mcp`
//! manager need *except* the I/O: the config-file format, the
//! `mcp__{server}__{tool}` naming contract, the JSON-RPC protocol shapes,
//! the SSE frame parser both HTTP transports share, and the snapshot/status
//! vocabulary the UI renders. Every rule here is unit-testable from fixtures
//! — the `hooks` split exactly.
//!
//! - `config`   — the `mcpServers` file parse, scopes, per-project disabled
//!   sets (the `skills.json` pattern).
//! - `names`    — wire names (`mcp__server__tool`), display names
//!   (`{server} - {tool} (MCP)`), and the batch label the live strip shows.
//! - `protocol` — JSON-RPC framing, `initialize`, `tools/list`,
//!   `tools/call` shapes and result mapping.
//! - `sse`      — the Server-Sent-Events parser.
//! - `status`   — the per-server snapshot the UI consumes, glyphs/labels,
//!   parameter listings, and the render-time argument pretty-printers.

mod config;
mod names;
mod protocol;
mod sse;
mod status;

pub use self::config::{
    McpFile, McpScope, McpServerConfig, McpServerEntry, McpWriteError, merge_project_scopes,
    merge_scopes, parse_disabled, parse_mcp_file, parse_server_entry, record_disabled,
    record_server, remove_server, render_server,
};
pub use self::names::{
    MCP_DISPLAY_SUFFIX, MCP_TOOL_PREFIX, batch_label, capitalize_display, capitalize_server,
    display_from_wire, display_server, is_mcp_display_name, is_mcp_tool, label_from_wire,
    normalize_name, parse_wire_name, tool_display_name, tool_label, tool_wire_name,
    validate_server_name, wire_from_display,
};
pub use self::protocol::{
    CallOutcome, Incoming, LEGACY_PROTOCOL_VERSION, MAX_TOOL_DESCRIPTION_CHARS, McpToolInfo,
    PROTOCOL_VERSION, RpcError, SUPPORTED_VERSIONS, ServerIdentity, call_params, choose_version,
    discover_params, header_value, initialize_params, initialize_params_for, is_modern_error,
    notification, parse_call_result, parse_discover, parse_incoming, parse_initialize,
    parse_tools_page, request, request_meta, tools_list_params, unsupported_versions, with_meta,
};
pub use self::sse::{SseEvent, SseParser};
pub use self::status::{
    McpAuthState, McpServerSnapshot, McpServerStatus, ToolParameter, auth_state, pretty_args,
    tool_parameters,
};
