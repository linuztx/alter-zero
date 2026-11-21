//! A plugin's agent files, read as this app's subagent types
//! (`docs/plugins.md` *Agents*).
//!
//! The format is the one `docs/subagents.md` already reads — it came from
//! Claude Code — so what differs is only what a plugin's file *names*: a
//! `model:` of a provider this session may not be on, and a `tools:` list in
//! Claude Code's tool vocabulary. The model is dropped (the reader runs every
//! plugin agent on the session's own); the tools are translated here.

use crate::subagents::{ALL_TOOLS, AgentTools};

/// What one of Claude Code's tool names is called here, or `None` for a tool
/// with no counterpart. A search is a shell command in this app — the
/// built-in read-only `explore` type is `Bash, Read` for exactly that reason
/// — so `Glob`, `Grep` and `LS` are `Bash`.
fn translate(tool: &str) -> Option<&'static str> {
    Some(match tool.to_ascii_lowercase().as_str() {
        "read" | "notebookread" => "Read",
        "write" => "Write",
        "edit" | "multiedit" | "notebookedit" => "Edit",
        "bash" | "glob" | "grep" | "ls" | "bashoutput" | "killshell" | "killbash" => "Bash",
        "skill" => "Skill",
        _ => return None,
    })
}

/// A plugin agent's `tools:` value as this app's allowlist: each name
/// translated (`translate`), MCP names (`mcp__…`) kept as written, a
/// permission rule's argument dropped (`Bash(git status:*)` is `Bash`), each
/// tool once — and a tool with no counterpart (`WebFetch`, `TodoWrite`,
/// `Task`) left out. A list that named only such tools is an allowlist that
/// matches nothing, which is what the agent can use here.
#[must_use]
pub fn translate_tools(value: &str) -> AgentTools {
    let AgentTools::Only(entries) = AgentTools::parse(value) else {
        return AgentTools::All;
    };
    let mut tools: Vec<String> = Vec::new();
    for entry in entries {
        let name = entry
            .split_once('(')
            .map_or(entry.as_str(), |(name, _)| name)
            .trim();
        let translated = if name == ALL_TOOLS {
            return AgentTools::All;
        } else if name
            .to_ascii_lowercase()
            .starts_with(crate::mcp::MCP_TOOL_PREFIX)
        {
            Some(name.to_string())
        } else {
            translate(name).map(str::to_string)
        };
        if let Some(tool) = translated
            && !tools.iter().any(|seen| seen.eq_ignore_ascii_case(&tool))
        {
            tools.push(tool);
        }
    }
    AgentTools::Only(tools)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn only(tools: &[&str]) -> AgentTools {
        AgentTools::Only(tools.iter().map(|tool| (*tool).to_string()).collect())
    }

    #[test]
    fn claude_codes_tool_names_become_this_apps() {
        assert_eq!(
            translate_tools(
                "Glob, Grep, LS, Read, NotebookRead, WebFetch, TodoWrite, WebSearch, KillShell, BashOutput"
            ),
            only(&["Bash", "Read"])
        );
        assert_eq!(
            translate_tools("Write, Edit, MultiEdit"),
            only(&["Write", "Edit"])
        );
        assert_eq!(translate_tools("[Read, Skill]"), only(&["Read", "Skill"]));
    }

    #[test]
    fn mcp_tools_are_kept_as_written() {
        assert_eq!(
            translate_tools("Read, mcp__context7__query-docs, mcp__github__*"),
            only(&["Read", "mcp__context7__query-docs", "mcp__github__*"])
        );
    }

    #[test]
    fn a_permission_rule_is_its_tool() {
        assert_eq!(
            translate_tools("Bash(git add:*), Bash(git status:*), Read"),
            only(&["Bash", "Read"])
        );
    }

    #[test]
    fn no_list_or_a_star_is_every_tool() {
        assert_eq!(translate_tools(""), AgentTools::All);
        assert_eq!(translate_tools("*"), AgentTools::All);
        assert_eq!(translate_tools("Read, *"), AgentTools::All);
    }

    #[test]
    fn a_list_of_tools_this_app_lacks_matches_nothing() {
        let tools = translate_tools("WebFetch, TodoWrite, Task");
        assert_eq!(tools, only(&[]));
        assert!(!tools.allows("bash"));
        assert!(!tools.allows("read"));
        assert_eq!(tools.summary(), "none");
    }
}
