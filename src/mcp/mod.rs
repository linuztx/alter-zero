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
    McpFile, McpScope, McpServerConfig, McpServerEntry, merge_scopes, parse_disabled,
    parse_mcp_file, record_disabled,
};
pub use self::names::{
    MCP_DISPLAY_SUFFIX, MCP_TOOL_PREFIX, batch_label, display_from_wire, display_server,
    is_mcp_display_name, is_mcp_tool, normalize_name, parse_wire_name, tool_display_name,
    tool_wire_name, wire_from_display,
};
pub use self::protocol::{
    CallOutcome, Incoming, MAX_TOOL_DESCRIPTION_CHARS, McpToolInfo, PROTOCOL_VERSION, RpcError,
    ServerIdentity, call_params, initialize_params, notification, parse_call_result,
    parse_incoming, parse_initialize, parse_tools_page, request, tools_list_params,
};
pub use self::sse::{SseEvent, SseParser};
pub use self::status::{
    McpAuthState, McpServerSnapshot, McpServerStatus, ToolParameter, pretty_args, primary_arg,
    tool_parameters,
};
