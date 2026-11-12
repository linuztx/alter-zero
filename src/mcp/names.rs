//! The MCP naming contract (`docs/tools/mcp.md`): how a server's tool becomes the
//! fully-qualified `mcp__{server}__{tool}` wire name the model calls, and how
//! that maps back — plus the user-facing `{server} - {tool} (MCP)` display
//! form and the aggregated live-strip label.
//!
//! Claude Code's convention (`mcpStringUtils.ts` / `normalization.ts`), so a
//! permission rule or transcript written against either tool reads the same
//! here — with three departures that each keep a provider from refusing the
//! request: runs of `_` collapse and edges trim (so `__` stays the
//! delimiter), a name over [`MAX_WIRE_NAME_LEN`] is shortened, and tools
//! whose names clash are told apart ([`wire_names`]).

/// The prefix every MCP tool's wire name carries.
pub const MCP_TOOL_PREFIX: &str = "mcp__";

/// The longest wire name every provider accepts: OpenAI's function names are
/// `^[a-zA-Z0-9_-]{1,64}$`, the strictest of the APIs the backend speaks.
/// One name over it fails the **whole** request — every built-in tool with
/// it — so [`tool_wire_name`] never returns a longer one.
pub const MAX_WIRE_NAME_LEN: usize = 64;

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

/// Validate a server name for the `mcp add` CLI (`docs/cli/mcp-cli.md`): a valid
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
/// `mcp__{server}__{tool}`, both parts normalized — and never longer than
/// [`MAX_WIRE_NAME_LEN`]. A longer one keeps its server whole when it can,
/// cuts the tool, and ends on eight hex digits of the full name's SHA-256,
/// so two tools sharing a long head stay apart. The shortened name is a
/// fixed point of this function (`tool_wire_name` of its own parts gives
/// it back), which is what lets [`wire_from_display`] replay a recorded
/// call as the name the model was offered.
#[must_use]
pub fn tool_wire_name(server: &str, tool: &str) -> String {
    let server = normalize_name(server);
    let tool = normalize_name(tool);
    let full = format!("{MCP_TOOL_PREFIX}{server}__{tool}");
    if full.len() <= MAX_WIRE_NAME_LEN {
        return full;
    }
    shorten_wire_name(&full, &server, &tool)
}

/// The hex digits of the full name's SHA-256 a shortened wire name ends on.
const WIRE_HASH_LEN: usize = 8;

/// The fewest characters of the tool a shortened name keeps.
const MIN_TOOL_HEAD: usize = 8;

/// A server part cut to fit is cut to this — only a server name too long to
/// leave the tool [`MIN_TOOL_HEAD`] characters is cut at all.
const MAX_CUT_SERVER_LEN: usize = 32;

/// [`tool_wire_name`]'s over-length arm: `mcp__{server}__{head}_{hash}`,
/// the hash over the full name.
fn shorten_wire_name(full: &str, server: &str, tool: &str) -> String {
    hashed_wire_name(server, tool, full)
}

/// `mcp__{server}__{head}_{hash}` with the hash over `key` — the one shape
/// both a too-long name and a clashing one take. Both parts are normalized
/// ASCII, so cutting at a byte index is cutting at a character; a cut never
/// leaves a trailing `_`, which would put `__` in the name (the delimiter)
/// or before the hash. A tool with nothing left after normalization is
/// headed `tool`, so the name always has a tool half.
fn hashed_wire_name(server: &str, tool: &str, key: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(key.as_bytes());
    let hash: String = digest
        .iter()
        .take(WIRE_HASH_LEN / 2)
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let cut = |name: &str, max: usize| {
        name[..name.len().min(max)]
            .trim_end_matches('_')
            .to_string()
    };
    // What is left for the server and the tool's head once the fixed parts —
    // `mcp__`, `__`, the `_` before the hash, the hash — are paid for.
    let room = MAX_WIRE_NAME_LEN - MCP_TOOL_PREFIX.len() - 2 - 1 - WIRE_HASH_LEN;
    let server = if server.len() + MIN_TOOL_HEAD <= room {
        server.to_string()
    } else {
        cut(server, MAX_CUT_SERVER_LEN)
    };
    let tool = if tool.is_empty() { "tool" } else { tool };
    let head = cut(tool, room - server.len());
    format!("{MCP_TOOL_PREFIX}{server}__{head}_{hash}")
}

/// Every tool one server lists, under the wire name the model is offered it
/// by — `(wire name, raw tool name)` in the server's order, each wire name
/// unique. Ordinarily that is [`tool_wire_name`]; but two raw names that
/// normalize alike (`files.read` beside `files_read`, both valid MCP names)
/// would be offered twice under one function name, which a provider refuses
/// for the whole request, and a name with nothing ASCII in it (`検索`)
/// normalizes to no name at all. Those take the hashed shape keyed by the
/// **raw** name instead — so every one of them stays callable, and each
/// gets the same name whatever order the server lists them in. A raw name
/// listed twice is offered once.
#[must_use]
pub fn wire_names<'a>(server: &str, tools: &[&'a str]) -> Vec<(String, &'a str)> {
    let server_part = normalize_name(server);
    let plain: Vec<String> = tools
        .iter()
        .map(|tool| tool_wire_name(server, tool))
        .collect();
    let mut uses: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    let mut seen_raw: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for (tool, wire) in tools.iter().zip(&plain) {
        if seen_raw.insert(tool) {
            *uses.entry(wire.as_str()).or_default() += 1;
        }
    }
    let mut seen_raw: std::collections::HashSet<&str> = std::collections::HashSet::new();
    let mut taken: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut out = Vec::with_capacity(tools.len());
    for (tool, wire) in tools.iter().zip(&plain) {
        if !seen_raw.insert(tool) {
            continue;
        }
        let normalized = normalize_name(tool);
        let wire = if normalized.is_empty() || uses.get(wire.as_str()).copied() > Some(1) {
            hashed_wire_name(
                &server_part,
                &normalized,
                &format!("{MCP_TOOL_PREFIX}{server_part}__{tool}"),
            )
        } else {
            wire.clone()
        };
        // A hashed name meeting another tool's own is astronomically
        // unlikely, but a duplicate would still fail the whole request.
        if taken.insert(wire.clone()) {
            out.push((wire, *tool));
        }
    }
    out
}

/// Why each configured server can't be offered under its name — `None` for
/// the ones that can, in the order given. A name that normalizes to nothing
/// gives its tools no `mcp__{server}__` prefix to live under, and two names
/// that normalize alike (`my.server`, `my_server`) would offer the model the
/// same tool names twice; the later of a clashing pair is the one refused,
/// so the server declared first keeps working.
#[must_use]
pub fn server_name_problems(names: &[&str]) -> Vec<Option<String>> {
    let mut seen: Vec<(String, &str)> = Vec::new();
    names
        .iter()
        .map(|name| {
            let normalized = normalize_name(name);
            if normalized.is_empty() {
                return Some(format!(
                    "the name \"{name}\" has no letters or digits to name its tools after — \
                     rename it"
                ));
            }
            if let Some((_, first)) = seen.iter().find(|(n, _)| *n == normalized) {
                return Some(format!(
                    "its tools would share the mcp__{normalized}__ names of \"{first}\" — \
                     rename one of the two"
                ));
            }
            seen.push((normalized, name));
            None
        })
        .collect()
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

/// The tool part of a display name (`deepwiki - ask_question (MCP)` →
/// `ask_question`) — the agent activity row's `{Server}: {tool}` detail half
/// (`docs/agents/agent-tool.md`). `None` when the name isn't the MCP display shape.
#[must_use]
pub fn display_tool(name: &str) -> Option<&str> {
    if !is_mcp_display_name(name) {
        return None;
    }
    name.strip_suffix(MCP_DISPLAY_SUFFIX)?
        .split_once(" - ")
        .map(|(_, tool)| tool)
        .filter(|tool| !tool.is_empty())
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

/// A server name with its first character upper-cased for **display** —
/// `deepwiki` → `Deepwiki`, the way Claude Code titles a server. Display
/// only: [`normalize_name`] preserves case, so a capitalized name that
/// entered the record would invert ([`wire_from_display`]) to
/// `mcp__Deepwiki__…`, a tool the model was never offered. The wire name,
/// the permission-rule key and the `/mcp` identity rows keep the configured
/// spelling (`docs/tools/mcp.md`).
#[must_use]
pub fn capitalize_server(server: &str) -> String {
    let mut chars = server.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

/// A display name or label with its **server half** capitalized for display
/// — `deepwiki - ask_question (MCP)` → `Deepwiki - ask_question (MCP)`, the
/// suffix-less permission label the same way. Anything without the
/// `{server} - …` shape comes back verbatim, so `Bash` and an unknown tool
/// name are untouched. Applied at the render seams only, never to the
/// recorded name (see [`capitalize_server`]).
#[must_use]
pub fn capitalize_display(name: &str) -> String {
    match name.split_once(" - ") {
        Some((server, rest)) if !server.is_empty() => {
            format!("{} - {rest}", capitalize_server(server))
        }
        _ => name.to_string(),
    }
}

/// The aggregated live-strip label for a batch of MCP calls
/// (`docs/tools/mcp.md`): the **distinct servers in call order**, then the total
/// call count when there is more than one call — `Deepwiki`,
/// `Deepwiki 2 times`, `Deepwiki, Context7 3 times`. Display-only (only the
/// `Calling …`/`Called …` cells read it), so the servers come back through
/// [`capitalize_server`]; the dedup still keys on the raw name.
#[must_use]
pub fn batch_label(servers: &[&str]) -> String {
    let mut distinct: Vec<&str> = Vec::new();
    for server in servers {
        if !distinct.contains(server) {
            distinct.push(server);
        }
    }
    let names = distinct
        .iter()
        .map(|server| capitalize_server(server))
        .collect::<Vec<_>>()
        .join(", ");
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
        // The `mcp add` name rule (docs/cli/mcp-cli.md): a valid name is one
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

    /// OpenAI-shaped APIs refuse a function name over 64 characters — and a
    /// refused name fails the **whole** request, every built-in tool with
    /// it. `mcp__atlassian-confluence-cloud__` alone is 33 of them.
    #[test]
    fn a_long_wire_name_is_shortened_to_the_provider_limit() {
        let server = "atlassian-confluence-cloud";
        let tool = "confluence_search_using_cql_query_with_pagination";
        let wire = tool_wire_name(server, tool);
        assert!(wire.len() <= MAX_WIRE_NAME_LEN, "{wire} ({})", wire.len());
        // The server half survives whole, so `mcp__server__*` allowlists and
        // the per-server grouping still find it.
        assert!(
            wire.starts_with("mcp__atlassian-confluence-cloud__"),
            "{wire}"
        );
        assert!(is_mcp_tool(&wire));
        // Deterministic: the permission rule and the context replay re-derive
        // the same name from the same pair.
        assert_eq!(wire, tool_wire_name(server, tool));
        // Two long tools sharing a head still get distinct names.
        let sibling = tool_wire_name(
            server,
            "confluence_search_using_cql_query_with_pagination_v2",
        );
        assert_ne!(wire, sibling);
        assert!(sibling.len() <= MAX_WIRE_NAME_LEN);
    }

    #[test]
    fn a_name_within_the_limit_is_left_alone() {
        // 64 exactly: `mcp__` + 27 + `__` + 30.
        let server = "s".repeat(27);
        let tool = "t".repeat(30);
        let wire = tool_wire_name(&server, &tool);
        assert_eq!(wire.len(), MAX_WIRE_NAME_LEN);
        assert_eq!(wire, format!("mcp__{server}__{tool}"));
    }

    /// The recorded cell carries the display name, and the context replay
    /// turns it back into the wire name the model called — so a shortened
    /// name has to survive display → wire as well as wire → display.
    #[test]
    fn a_shortened_wire_name_round_trips_through_display_and_replay() {
        for (server, tool) in [
            (
                "atlassian-confluence-cloud",
                "confluence_search_using_cql_query_with_pagination",
            ),
            // A name whose cut lands on an underscore must not leave `__`.
            ("srv", &format!("{}_{}", "a".repeat(52), "tail")),
            // A server too long to keep whole is cut too.
            (&"long-server-name-".repeat(5), "query"),
        ] {
            let wire = tool_wire_name(server, tool);
            assert!(wire.len() <= MAX_WIRE_NAME_LEN, "{wire}");
            let (wire_server, wire_tool) = parse_wire_name(&wire).expect("parses");
            assert!(
                !wire_server.contains("__") && !wire_tool.starts_with('_'),
                "{wire}"
            );
            let display = display_from_wire(&wire).expect("display");
            assert_eq!(wire_from_display(&display).as_deref(), Some(wire.as_str()));
            assert_eq!(normalize_name(wire_tool), wire_tool, "{wire}");
        }
    }

    /// Two tools that normalize to one name — `files.read` beside
    /// `files_read`, both valid MCP names — would be offered twice under the
    /// same function name, which providers refuse for the whole request.
    /// Each gets a name of its own, the same whichever order they arrive in.
    #[test]
    fn tools_whose_names_collide_are_told_apart_in_any_order() {
        let names = wire_names("srv", &["files.read", "files_read", "list"]);
        let wires: Vec<&str> = names.iter().map(|(wire, _)| wire.as_str()).collect();
        assert_eq!(wires.len(), 3);
        assert_eq!(wires[2], "mcp__srv__list");
        assert_ne!(wires[0], wires[1]);
        for wire in &wires[..2] {
            assert!(wire.starts_with("mcp__srv__files_read_"), "{wire}");
            // Still a fixed point, so the replay finds it.
            let (server, tool) = parse_wire_name(wire).unwrap();
            assert_eq!(&tool_wire_name(server, tool), wire);
        }
        let reversed = wire_names("srv", &["files_read", "files.read", "list"]);
        assert_eq!(reversed[0].0, names[1].0);
        assert_eq!(reversed[1].0, names[0].0);
        // Each wire name still maps back to its own raw tool.
        assert_eq!(names[0].1, "files.read");
        assert_eq!(names[1].1, "files_read");
    }

    #[test]
    fn a_tool_whose_name_normalizes_to_nothing_still_gets_a_name() {
        // A CJK tool name has no ASCII to keep; `mcp__srv__` would be no name
        // at all.
        let names = wire_names("srv", &["検索", "翻訳"]);
        assert_eq!(names.len(), 2);
        assert_ne!(names[0].0, names[1].0);
        for (wire, _) in &names {
            let (server, tool) = parse_wire_name(wire).expect("parses");
            assert_eq!(server, "srv");
            assert_eq!(&tool_wire_name(server, tool), wire);
        }
    }

    #[test]
    fn a_tool_listed_twice_is_offered_once() {
        let names = wire_names("srv", &["echo", "echo"]);
        assert_eq!(names, vec![("mcp__srv__echo".to_string(), "echo")]);
    }

    /// Two servers whose names normalize alike would hand the model the same
    /// `mcp__{server}__` names twice; a name with nothing to normalize to
    /// would hand it none. The later of a clashing pair is the one refused.
    #[test]
    fn servers_whose_names_collide_are_refused_with_the_reason() {
        let problems = server_name_problems(&["my.server", "github", "my_server", "!!!"]);
        assert_eq!(problems[0], None);
        assert_eq!(problems[1], None);
        let clash = problems[2].as_deref().expect("a clash");
        assert!(
            clash.contains("my.server") && clash.contains("mcp__my_server__"),
            "{clash}"
        );
        assert!(
            problems[3]
                .as_deref()
                .unwrap()
                .contains("letters or digits")
        );
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
    fn display_tool_extracts_the_tool_half() {
        // The agent activity row's `{Server}: {tool}` needs the tool half of
        // a display name; the tool keeps its own underscores.
        assert_eq!(
            display_tool("deepwiki - ask_question (MCP)"),
            Some("ask_question")
        );
        assert_eq!(
            display_tool("srv - read_wiki_structure (MCP)"),
            Some("read_wiki_structure")
        );
        assert_eq!(display_tool("Bash"), None);
        assert_eq!(display_tool("just a suffix (MCP)"), None);
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
        // Display-only (the `Calling …`/`Called …` cells), so the servers
        // read capitalized; the dedup still keys on the raw name.
        assert_eq!(batch_label(&["deepwiki"]), "Deepwiki");
        assert_eq!(batch_label(&["deepwiki", "deepwiki"]), "Deepwiki 2 times");
        assert_eq!(
            batch_label(&["deepwiki", "deepwiki", "context7"]),
            "Deepwiki, Context7 3 times"
        );
        assert_eq!(batch_label(&[]), "");
    }

    #[test]
    fn capitalize_server_upcases_only_the_first_character() {
        assert_eq!(capitalize_server("deepwiki"), "Deepwiki");
        assert_eq!(capitalize_server("context7"), "Context7");
        assert_eq!(
            capitalize_server("plugin_context7_context7"),
            "Plugin_context7_context7"
        );
        assert_eq!(capitalize_server("Already"), "Already");
        assert_eq!(capitalize_server(""), "");
        // Unicode-aware, not `to_ascii_uppercase`: é upcases (and ß expands).
        assert_eq!(capitalize_server("élan"), "Élan");
        assert_eq!(capitalize_server("ßerver"), "SServer");
    }

    #[test]
    fn capitalize_display_names_the_server_and_leaves_the_rest() {
        assert_eq!(
            capitalize_display("deepwiki - read_wiki_structure (MCP)"),
            "Deepwiki - read_wiki_structure (MCP)"
        );
        // The suffix-less permission label capitalizes the same way.
        assert_eq!(
            capitalize_display("deepwiki - ask_question"),
            "Deepwiki - ask_question"
        );
        // Anything without the `{server} - …` shape comes back verbatim.
        assert_eq!(capitalize_display("Bash"), "Bash");
        assert_eq!(capitalize_display("mystery"), "mystery");
    }

    #[test]
    fn the_display_capitalization_never_reaches_the_wire() {
        // The stored `ToolCall::name` keeps the configured spelling: the
        // context replay inverts it with `wire_from_display`, whose
        // `normalize_name` preserves case — a capitalized record would
        // replay as `mcp__Deepwiki__…`, a tool the model was never offered.
        let display = tool_display_name("deepwiki", "ask_question");
        assert!(display.starts_with("deepwiki"), "the record stays raw");
        assert_eq!(
            wire_from_display(&display).as_deref(),
            Some("mcp__deepwiki__ask_question")
        );
        // Inverting the capitalized form names a *different* wire tool —
        // exactly why it is applied at the render seams and never stored.
        assert_eq!(
            wire_from_display(&capitalize_display(&display)).as_deref(),
            Some("mcp__Deepwiki__ask_question")
        );
    }
}
