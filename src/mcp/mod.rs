//! MCP (Model Context Protocol) — the **pure** half (`docs/tools/mcp.md`).
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
//! - `headers`  — the streamable-HTTP header value encoding and the
//!   `x-mcp-header` parameter mirror.
//! - `sse`      — the Server-Sent-Events parser.
//! - `status`   — the per-server snapshot the UI consumes, glyphs/labels,
//!   parameter listings, and the render-time argument pretty-printers.

mod config;
mod headers;
mod instructions;
mod names;
mod protocol;
mod sse;
mod status;

pub use self::config::{
    McpFile, McpScope, McpServerConfig, McpServerEntry, McpWriteError, OAuthClientConfig,
    expand_config, expand_vars, merge_project_scopes, merge_scopes, parse_disabled, parse_mcp_file,
    parse_server_entry, record_disabled, record_server, remove_server, render_server,
    resolve_config,
};
pub use self::headers::{
    MCP_PARAM_HEADER_PREFIX, ParamHeader, X_MCP_HEADER_KEY, header_value, param_header_values,
    param_headers, strip_header_annotations,
};
pub use self::instructions::{
    INSTRUCTIONS_HEADER, INSTRUCTIONS_PREAMBLE, MAX_INSTRUCTIONS_CHARS, instructions_section,
};
pub use self::names::{
    MAX_WIRE_NAME_LEN, MCP_DISPLAY_SUFFIX, MCP_TOOL_PREFIX, batch_label, capitalize_display,
    capitalize_server, display_from_wire, display_server, display_tool, is_mcp_display_name,
    is_mcp_tool, label_from_wire, normalize_name, parse_wire_name, server_name_problems,
    tool_display_name, tool_label, tool_wire_name, validate_server_name, wire_from_display,
    wire_names,
};
pub use self::protocol::{
    CallOutcome, ExcludedTool, FOLDED_SCHEMA_NOTE, Incoming, InputRequired,
    LEGACY_PROTOCOL_VERSION, MAX_TOOL_DESCRIPTION_CHARS, McpToolInfo, PROTOCOL_VERSION, RpcError,
    SUPPORTED_VERSIONS, ServerIdentity, TOOLS_LIST_CHANGED, call_params, call_params_retrying,
    choose_version, discover_params, initialize_params, initialize_params_for, input_required,
    input_schema_problem, is_modern_error, listen_params, model_input_schema, notification,
    notification_method, parse_call_result, parse_discover, parse_incoming, parse_initialize,
    parse_tools_page, request, request_meta, result_type_problem, server_request_answer,
    tools_list_params, unsupported_versions, with_meta,
};
pub use self::sse::{SseEvent, SseParser};
pub use self::status::{
    McpAuthState, McpServerSnapshot, McpServerStatus, ToolParameter, auth_state, pretty_args,
    tool_parameters,
};
