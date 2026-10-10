//! The fourteen lifecycle events a hook can attach to, and their wire names.
//!
//! The names are Claude Code's `HOOK_EVENTS` spellings and are also the keys
//! of a `hooks.json` `"hooks"` object, so they are `PascalCase` strings rather
//! than anything derived — a rename here silently stops matching real config
//! files, which is why they are written out.

/// One point in the agent's lifecycle a hook may run at (`docs/hooks.md`).
///
/// Claude Code's spellings throughout — a subset of its events, every one of
/// them fired (`docs/hooks.md`); a config naming one we don't model loads
/// fine and never fires it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum HookEvent {
    /// Before a tool call runs — may allow, deny, ask, or rewrite its input.
    PreToolUse,
    /// After a tool call ran — may block, or feed the model extra context.
    PostToolUse,
    /// After a tool call ran and **failed** — `PostToolUse` fires only for a
    /// success; this one may feed the model extra context.
    PostToolUseFailure,
    /// Instead of asking the user to approve a call — may allow or deny.
    PermissionRequest,
    /// The app wants the user's attention — `permission_prompt`: a
    /// permission prompt is on screen. Observation only.
    Notification,
    /// The user submitted a prompt — may block it, or add context.
    UserPromptSubmit,
    /// A session opened.
    SessionStart,
    /// A session is closing.
    SessionEnd,
    /// The model stopped answering — may make it keep going.
    Stop,
    /// The turn ended on an error instead of an answer. Observation only.
    StopFailure,
    /// A subagent was launched.
    SubagentStart,
    /// A subagent finished — may make it keep going.
    SubagentStop,
    /// Before a `/compact` summarization turn.
    PreCompact,
    /// After one landed.
    PostCompact,
}

impl HookEvent {
    /// Every event, in the order [`crate::hooks::HooksFile`] lists them.
    pub const ALL: &'static [Self] = &[
        Self::PreToolUse,
        Self::PostToolUse,
        Self::PostToolUseFailure,
        Self::PermissionRequest,
        Self::Notification,
        Self::UserPromptSubmit,
        Self::SessionStart,
        Self::SessionEnd,
        Self::Stop,
        Self::StopFailure,
        Self::SubagentStart,
        Self::SubagentStop,
        Self::PreCompact,
        Self::PostCompact,
    ];

    /// The wire name: the `hooks.json` key, and the payload's
    /// `hook_event_name`.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::PreToolUse => "PreToolUse",
            Self::PostToolUse => "PostToolUse",
            Self::PostToolUseFailure => "PostToolUseFailure",
            Self::PermissionRequest => "PermissionRequest",
            Self::Notification => "Notification",
            Self::UserPromptSubmit => "UserPromptSubmit",
            Self::SessionStart => "SessionStart",
            Self::SessionEnd => "SessionEnd",
            Self::Stop => "Stop",
            Self::StopFailure => "StopFailure",
            Self::SubagentStart => "SubagentStart",
            Self::SubagentStop => "SubagentStop",
            Self::PreCompact => "PreCompact",
            Self::PostCompact => "PostCompact",
        }
    }

    /// The event a `hooks.json` key names, or `None` for one we don't model
    /// (Claude Code has thirty-odd; a config mentioning `FileChanged` must
    /// load fine and simply never fire it).
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|e| e.name() == name)
    }

    /// Whether this event's match query is a **tool name** — the events whose
    /// matchers also honour the Claude Code spellings of our tools
    /// ([`crate::hooks::matcher::claude_code_alias`]). The rest match on other
    /// facts (an agent type, a source, a trigger) that have no alias to
    /// borrow.
    #[must_use]
    pub const fn matches_tool_names(self) -> bool {
        matches!(
            self,
            Self::PreToolUse
                | Self::PostToolUse
                | Self::PostToolUseFailure
                | Self::PermissionRequest
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_event_round_trips_through_its_wire_name() {
        for &event in HookEvent::ALL {
            assert_eq!(HookEvent::from_name(event.name()), Some(event));
        }
    }

    #[test]
    fn an_event_we_do_not_model_is_not_an_error_just_unknown() {
        assert_eq!(HookEvent::from_name("FileChanged"), None);
        assert_eq!(HookEvent::from_name("pretooluse"), None);
    }

    #[test]
    fn all_lists_every_variant_exactly_once() {
        let mut names: Vec<&str> = HookEvent::ALL.iter().map(|e| e.name()).collect();
        let total = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), total, "ALL has a duplicate");
        assert_eq!(total, 14);
    }

    #[test]
    fn claude_codes_failure_and_notification_events_are_modelled() {
        // A config written for Claude Code names these; they used to load and
        // never fire (`docs/hooks.md` *Known gaps*).
        for name in ["PostToolUseFailure", "StopFailure", "Notification"] {
            assert!(HookEvent::from_name(name).is_some(), "{name} is unmodelled");
        }
    }

    #[test]
    fn a_failed_calls_event_matches_on_the_tool_like_its_success_twin() {
        assert!(HookEvent::PostToolUseFailure.matches_tool_names());
        // StopFailure matches the error type, Notification its type.
        assert!(!HookEvent::StopFailure.matches_tool_names());
        assert!(!HookEvent::Notification.matches_tool_names());
    }
}
