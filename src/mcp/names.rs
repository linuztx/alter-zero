//! The MCP naming contract (`docs/mcp.md`): how a server's tool becomes the
//! fully-qualified `mcp__{server}__{tool}` wire name the model calls, and how
//! that maps back — plus the user-facing `{server} - {tool} (MCP)` display
//! form and the aggregated live-strip label.
//!
//! Claude Code's convention exactly (`mcpStringUtils.ts` /
//! `normalization.ts`), so a permission rule or transcript written against
//! either tool reads the same here.

/// The prefix every MCP tool's wire name carries.
pub const MCP_TOOL_PREFIX: &str = "mcp__";

/// The suffix every MCP tool's display name carries — what
/// [`is_mcp_display_name`] (and so the cell renderer and the context replay)
/// recognise a recorded MCP call by, with no extra record field to persist.
pub const MCP_DISPLAY_SUFFIX: &str = " (MCP)";

/// Normalize a server/tool name for the wire: anything outside
/// `[A-Za-z0-9_-]` becomes `_`, runs collapse to one, and leading/trailing
/// underscores are stripped — so a name can never smuggle the `__` delimiter
/// (Claude Code collapses only for its claude.ai prefix, but its own comment
/// calls the uncollapsed `__` a known parsing bug; collapsing for every name
/// keeps [`parse_wire_name`] unambiguous).
#[must_use]
pub fn normalize_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut last_underscore = false;
    for c in name.chars() {
        let mapped = if c.is_ascii_alphanumeric() || c == '-' {
            c
        } else {
            '_'
        };
        if mapped == '_' {
            if last_underscore {
                continue;
            }
            last_underscore = true;
        } else {
            last_underscore = false;
        }
        out.push(mapped);
    }
    out.trim_matches('_').to_string()
}

/// Validate a server name for the `mcp add` CLI (`docs/mcp-cli.md`): a valid
/// name is a non-empty one [`normalize_name`] maps to itself — the
/// `[A-Za-z0-9_-]` class both references enforce, minus the `__` runs and
/// edge underscores normalization would rewrite — so the declared name, the
/// `/mcp` list, the permission rules and the wire name always agree. The
/// error names the culprit and, when normalization has a spelling to offer,
/// suggests it.
pub fn validate_server_name(name: &str) -> Result<(), String> {
    let normalized = normalize_name(name);
    if !name.is_empty() && normalized == name {
        return Ok(());
    }
    let mut message = format!("invalid server name \"{name}\" (use letters, numbers, '-', '_')");
    if !normalized.is_empty() {
        message.push_str(&format!("; try \"{normalized}\""));
    }
    Err(message)
}

/// The fully-qualified wire name the model calls:
/// `mcp__{server}__{tool}`, both parts normalized.
#[must_use]
pub fn tool_wire_name(server: &str, tool: &str) -> String {
    format!(
        "{MCP_TOOL_PREFIX}{}__{}",
        normalize_name(server),
        normalize_name(tool)
    )
}

/// Is this tool name an MCP call? The `is_task_tool` shape — a pure prefix
/// test, so the execute dispatch and the permission seam agree without a
/// registry in hand.
#[must_use]
pub fn is_mcp_tool(name: &str) -> bool {
    name.strip_prefix(MCP_TOOL_PREFIX)
        .is_some_and(|rest| rest.contains("__"))
}

/// Split a wire name back into `(server, tool)`. The server part never
/// contains `__` ([`normalize_name`] collapses runs), so the first `__` after
/// the prefix is the delimiter; the tool keeps any later underscores.
/// `None` for a name that isn't an MCP wire name.
#[must_use]
pub fn parse_wire_name(name: &str) -> Option<(&str, &str)> {
    let rest = name.strip_prefix(MCP_TOOL_PREFIX)?;
    let split = rest.find("__")?;
    let (server, tool) = (&rest[..split], &rest[split + 2..]);
    if server.is_empty() || tool.is_empty() {
        return None;
    }
    Some((server, tool))
}

/// The user-facing display name a cell header shows — Claude Code's
/// `{server} - {tool} (MCP)`.
#[must_use]
pub fn tool_display_name(server: &str, tool: &str) -> String {
    format!("{}{MCP_DISPLAY_SUFFIX}", tool_label(server, tool))
}

/// The display name's **label** half — `{server} - {tool}`, no suffix. The
/// permission prompt renders the arguments *between* the two
/// (`deepwiki - ask_question(repoName: "…") (MCP)`) and names the label in
/// its "don't ask again" rule, so the pieces are built separately.
#[must_use]
pub fn tool_label(server: &str, tool: &str) -> String {
    format!("{server} - {tool}")
}

/// [`tool_label`] from a wire name — `None` when the name isn't one.
#[must_use]
pub fn label_from_wire(name: &str) -> Option<String> {
    parse_wire_name(name).map(|(server, tool)| tool_label(server, tool))
}

/// [`tool_display_name`] from a wire name — what
/// [`crate::llm::tools::display_name`] shows for an `mcp__…` call.
#[must_use]
pub fn display_from_wire(name: &str) -> Option<String> {
    parse_wire_name(name).map(|(server, tool)| tool_display_name(server, tool))
}

/// Does this **display** name belong to an MCP call? The ` (MCP)` suffix is
/// the marker — pure over the recorded `ToolCall::name`, so a `/resume`d cell
/// is recognised with no extra field.
#[must_use]
pub fn is_mcp_display_name(name: &str) -> bool {
    name.ends_with(MCP_DISPLAY_SUFFIX) && name.contains(" - ")
}

/// The server part of a display name (`deepwiki - ask_question (MCP)` →
/// `deepwiki`) — what the collapsed `Calling {server}…` / `Called {server}`
/// cells show. `None` when the name isn't the MCP display shape.
#[must_use]
pub fn display_server(name: &str) -> Option<&str> {
    if !is_mcp_display_name(name) {
        return None;
    }
    name.split(" - ").next().filter(|s| !s.is_empty())
}

/// Invert [`tool_display_name`] back to the wire name — the context replay's
/// arm ([`crate::context`]), so a recorded cell replays as the `tool_calls`
/// entry the model actually made. Re-normalizes both parts, which is the
/// identity for anything that came off the wire.
#[must_use]
pub fn wire_from_display(name: &str) -> Option<String> {
    let stripped = name.strip_suffix(MCP_DISPLAY_SUFFIX)?;
    let (server, tool) = stripped.split_once(" - ")?;
    if server.is_empty() || tool.is_empty() {
        return None;
    }
    Some(tool_wire_name(server, tool))
}

/// The aggregated live-strip label for a batch of MCP calls
/// (`docs/mcp.md`): the **distinct servers in call order**, then the total
/// call count when there is more than one call — `deepwiki`,
/// `deepwiki 2 times`, `deepwiki, context7 3 times`.
#[must_use]
pub fn batch_label(servers: &[&str]) -> String {
    let mut distinct: Vec<&str> = Vec::new();
    for server in servers {
        if !distinct.contains(server) {
            distinct.push(server);
        }
    }
    let names = distinct.join(", ");
    match servers.len() {
        0 | 1 => names,
        n => format!("{names} {n} times"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_accepts_exactly_what_normalize_keeps() {
        // The `mcp add` name rule (docs/mcp-cli.md): a valid name is one
        // `normalize_name` maps to itself, so the declared name, the /mcp
        // list and the wire name can never disagree.
        assert_eq!(validate_server_name("deepwiki"), Ok(()));
        assert_eq!(validate_server_name("Server_1-x"), Ok(()));
        for bad in ["", "my server", "a.b", "caf\u{e9}", "a/b", "a__b", "_x"] {
            let err = validate_server_name(bad).expect_err("invalid name");
            assert!(err.contains("letters, numbers"), "{err}");
        }
        // The message names the culprit and suggests the wire-safe spelling.
        let err = validate_server_name("my server").unwrap_err();
        assert!(err.contains("my server"), "{err}");
        assert!(err.contains("\"my_server\""), "{err}");
        // Nothing to suggest for a name that normalizes to nothing.
        assert!(!validate_server_name("___").unwrap_err().contains("try"));
    }

    #[test]
    fn normalize_maps_invalid_chars_to_underscores() {
        assert_eq!(normalize_name("deepwiki"), "deepwiki");
        assert_eq!(normalize_name("my server.name"), "my_server_name");
        assert_eq!(
            normalize_name("plugin:context7:context7"),
            "plugin_context7_context7"
        );
        assert_eq!(normalize_name("with-dash_ok"), "with-dash_ok");
    }

    #[test]
    fn normalize_collapses_runs_and_trims_edges() {
        // A `__` inside a server name would break the wire-name split, so
        // runs collapse and edges trim.
        assert_eq!(normalize_name("a__b"), "a_b");
        assert_eq!(normalize_name("_edge_"), "edge");
        assert_eq!(normalize_name("a...b"), "a_b");
    }

    #[test]
    fn wire_name_round_trips_through_parse() {
        let wire = tool_wire_name("deepwiki", "ask_question");
        assert_eq!(wire, "mcp__deepwiki__ask_question");
        assert_eq!(parse_wire_name(&wire), Some(("deepwiki", "ask_question")));
    }

    #[test]
    fn a_tool_keeps_its_own_underscores() {
        // The tool part may contain later `__`-free underscores; the split is
        // on the FIRST `__` after the prefix.
        assert_eq!(
            parse_wire_name("mcp__srv__read_wiki_structure"),
            Some(("srv", "read_wiki_structure"))
        );
    }

    #[test]
    fn is_mcp_tool_matches_only_wire_names() {
        assert!(is_mcp_tool("mcp__deepwiki__ask_question"));
        assert!(!is_mcp_tool("bash"));
        assert!(!is_mcp_tool("mcp__loneserver"));
        assert!(!is_mcp_tool("mcpish__x__y"));
    }

    #[test]
    fn display_name_is_the_claude_code_shape() {
        assert_eq!(
            tool_display_name("deepwiki", "ask_question"),
            "deepwiki - ask_question (MCP)"
        );
        assert_eq!(
            display_from_wire("mcp__deepwiki__ask_question").as_deref(),
            Some("deepwiki - ask_question (MCP)")
        );
        // The label is the same name without the suffix — what the permission
        // prompt puts the arguments between.
        assert_eq!(
            label_from_wire("mcp__deepwiki__ask_question").as_deref(),
            Some("deepwiki - ask_question")
        );
        assert_eq!(label_from_wire("bash"), None);
    }

    #[test]
    fn display_name_recognition_and_server_extraction() {
        assert!(is_mcp_display_name("deepwiki - ask_question (MCP)"));
        assert!(!is_mcp_display_name("Bash"));
        assert!(!is_mcp_display_name("just a suffix (MCP)"));
        assert_eq!(
            display_server("deepwiki - ask_question (MCP)"),
            Some("deepwiki")
        );
        assert_eq!(display_server("Bash"), None);
    }

    #[test]
    fn wire_from_display_inverts_the_display_name() {
        assert_eq!(
            wire_from_display("deepwiki - ask_question (MCP)").as_deref(),
            Some("mcp__deepwiki__ask_question")
        );
        assert_eq!(wire_from_display("Bash"), None);
        // A display built from raw names re-normalizes on the way back.
        assert_eq!(
            wire_from_display("my server - do it (MCP)").as_deref(),
            Some("mcp__my_server__do_it")
        );
    }

    #[test]
    fn batch_label_names_distinct_servers_and_counts_calls() {
        assert_eq!(batch_label(&["deepwiki"]), "deepwiki");
        assert_eq!(batch_label(&["deepwiki", "deepwiki"]), "deepwiki 2 times");
        assert_eq!(
            batch_label(&["deepwiki", "deepwiki", "context7"]),
            "deepwiki, context7 3 times"
        );
        assert_eq!(batch_label(&[]), "");
    }
}
