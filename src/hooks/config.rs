//! The `hooks.json` file format and handler selection (`docs/hooks.md`).
//!
//! The shape is event → matcher groups → handlers, verbatim from both
//! references:
//!
//! ```json
//! { "hooks": { "PreToolUse": [ { "matcher": "bash|write",
//!     "hooks": [ { "type": "command", "command": "./guard.sh", "timeout": 60 } ] } ] } }
//! ```
//!
//! Two parse postures, deliberately different:
//!
//! - A **handler type we don't implement** (`mcp_tool`, `prompt`, `agent`) and
//!   an **event we don't model** are tolerated: they load and are skipped with
//!   a warning. codex parses those handler types and implements none of them
//!   either, and Claude Code has twenty-seven events to our eleven, so a
//!   config shared between tools must not be an error here.
//! - A **malformed file** is an error, not an empty config. Silently reading a
//!   typo'd `hooks.json` as "no hooks" is how a user comes to believe a guard
//!   is running when it is not.

use std::collections::BTreeMap;

use serde::Deserialize;

use super::event::HookEvent;
use super::matcher;

/// A parsed `hooks.json`.
///
/// The events are a map rather than eleven named fields so that a key we
/// don't model round-trips as data instead of failing the parse.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HooksFile {
    /// Free text some configs carry; read and ignored.
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub hooks: BTreeMap<String, Vec<MatcherGroup>>,
}

/// One `{ "matcher": …, "hooks": [ … ] }` entry under an event.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct MatcherGroup {
    /// Absent, empty or `*` matches every query — see [`matcher::matches`].
    #[serde(default)]
    pub matcher: Option<String>,
    #[serde(default)]
    pub hooks: Vec<HookHandler>,
}

/// One handler inside a group.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HookHandler {
    /// `command` is the only kind implemented; the rest parse and are skipped.
    #[serde(rename = "type", default)]
    pub kind: HandlerKind,
    #[serde(default)]
    pub command: Option<String>,
    /// Seconds. `None` takes [`CommandHook::DEFAULT_TIMEOUT_SECS`].
    #[serde(default)]
    pub timeout: Option<u64>,
    /// Shown while the hook runs. Parsed for compatibility; the live cell
    /// already says what it is waiting for.
    #[serde(default)]
    pub status_message: Option<String>,
    /// Fire-and-forget. Parsed and **ignored** — every v1 hook is synchronous.
    #[serde(rename = "async", default)]
    pub is_async: bool,
}

/// A handler's `"type"`.
///
/// Hand-written `Deserialize` rather than a derive: serde's `#[serde(other)]`
/// catch-all is only available inside an internally tagged enum, and this one
/// is deserialized straight from a string. Keeping the unknown name lets the
/// skip warning say *which* type it skipped.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum HandlerKind {
    /// A shell command: the only kind that runs.
    #[default]
    Command,
    /// Anything else (`mcp_tool`, `prompt`, `agent`) — parsed so a config
    /// shared with another tool loads, then skipped with a warning.
    Other(String),
}

impl<'de> Deserialize<'de> for HandlerKind {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let name = String::deserialize(deserializer)?;
        Ok(if name == "command" {
            Self::Command
        } else {
            Self::Other(name)
        })
    }
}

/// A handler that will actually run: the `command` kind with its command
/// present, resolved against its defaults.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandHook {
    pub command: String,
    /// The resolved timeout in seconds — never `None`, so the runner has no
    /// default of its own to drift from this one.
    pub timeout_secs: u64,
    pub status_message: Option<String>,
}

impl CommandHook {
    /// Claude Code's `TOOL_HOOK_EXECUTION_TIMEOUT_MS` — ten minutes. Long,
    /// because a hook that runs a test suite is a normal thing to want; the
    /// turn's Esc is the real stop button, and the runner polls for it.
    pub const DEFAULT_TIMEOUT_SECS: u64 = 600;
}

/// What an event selected: the handlers to run, in order, plus whatever was
/// wrong with the config on the way. Warnings are surfaced once and never
/// block — a broken matcher must not take the working handlers down with it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Selection {
    pub handlers: Vec<CommandHook>,
    pub warnings: Vec<String>,
}

impl HooksFile {
    /// Parse a `hooks.json` body.
    ///
    /// # Errors
    /// The serde message when the file is not valid JSON of this shape. The
    /// caller surfaces it and continues with hooks off — unlike
    /// `PermissionsFile::parse`'s best-effort default, because a hook the user
    /// believes is guarding them and silently is not is worse than no hooks.
    pub fn parse(text: &str) -> Result<Self, String> {
        // An empty file is an empty config, not a parse error: `: > hooks.json`
        // is how a user turns hooks off by hand.
        if text.trim().is_empty() {
            return Ok(Self::default());
        }
        serde_json::from_str(text).map_err(|err| err.to_string())
    }

    /// Is there nothing here to run? Used to report the `/settings` row as
    /// unavailable rather than advertising a toggle that does nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.hooks.values().all(|groups| {
            groups
                .iter()
                .all(|group| group.hooks.iter().all(|h| h.kind != HandlerKind::Command))
        })
    }

    /// The handlers `event` runs for `query` — the tool name for the tool
    /// events, the source/trigger/reason for the rest, `None` for an event
    /// with nothing to match on.
    ///
    /// Duplicates are dropped (first wins): a handler listed under both
    /// `"bash"` and `"*"` runs **once**, matching Claude Code, so a user
    /// broadening a matcher doesn't silently double their linter.
    #[must_use]
    pub fn select(&self, event: HookEvent, query: Option<&str>) -> Selection {
        let mut selection = Selection::default();
        let Some(groups) = self.hooks.get(event.name()) else {
            return selection;
        };
        for group in groups {
            let pattern = group.matcher.as_deref();
            if matcher::invalid_regex(pattern) {
                selection.warnings.push(format!(
                    "hooks.json: {} matcher {:?} is not a valid regex — skipped",
                    event.name(),
                    pattern.unwrap_or_default()
                ));
                continue;
            }
            // An event with nothing to match on (Stop, SessionEnd's reason
            // aside) runs every group, matcher or not — Claude Code's rule.
            if let Some(query) = query
                && !matcher::matches(pattern, query)
            {
                continue;
            }
            for handler in &group.hooks {
                match handler.resolve() {
                    Ok(hook) => {
                        // `"async": true` parses so a config shared with
                        // another tool loads, but every v1 hook is
                        // synchronous. Running it inline is the safe reading
                        // of a flag we don't honour — the alternative is
                        // skipping a guard the user thinks is armed — so say
                        // so rather than changing the timing silently.
                        if handler.is_async {
                            selection.warnings.push(format!(
                                "hooks.json: {} handler {:?} is \"async\" — run synchronously",
                                event.name(),
                                hook.command
                            ));
                        }
                        if !selection.handlers.iter().any(|h| h.command == hook.command) {
                            selection.handlers.push(hook);
                        }
                    }
                    Err(warning) => selection
                        .warnings
                        .push(format!("hooks.json: {} {warning}", event.name())),
                }
            }
        }
        selection
    }
}

impl HookHandler {
    /// The runnable form, or why this handler is being skipped.
    fn resolve(&self) -> Result<CommandHook, String> {
        if let HandlerKind::Other(kind) = &self.kind {
            return Err(format!(
                "handler type {kind:?} is not \"command\" — skipped"
            ));
        }
        let command = self
            .command
            .as_deref()
            .map(str::trim)
            .filter(|c| !c.is_empty())
            .ok_or_else(|| "command handler has no command — skipped".to_string())?;
        Ok(CommandHook {
            command: command.to_string(),
            timeout_secs: self
                .timeout
                .filter(|t| *t > 0)
                .unwrap_or(CommandHook::DEFAULT_TIMEOUT_SECS),
            status_message: self.status_message.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
      "description": "guards",
      "hooks": {
        "PreToolUse": [
          {
            "matcher": "bash|write",
            "hooks": [
              { "type": "command", "command": "./guard.sh", "timeout": 10,
                "statusMessage": "checking…" }
            ]
          }
        ]
      }
    }"#;

    fn parse(text: &str) -> HooksFile {
        HooksFile::parse(text).expect("fixture should parse")
    }

    #[test]
    fn the_documented_file_shape_parses_with_every_field() {
        let file = parse(SAMPLE);
        assert_eq!(file.description.as_deref(), Some("guards"));
        let selection = file.select(HookEvent::PreToolUse, Some("bash"));
        assert_eq!(selection.warnings, Vec::<String>::new());
        assert_eq!(
            selection.handlers,
            vec![CommandHook {
                command: "./guard.sh".to_string(),
                timeout_secs: 10,
                status_message: Some("checking…".to_string()),
            }]
        );
    }

    #[test]
    fn a_non_matching_query_selects_nothing() {
        assert!(
            parse(SAMPLE)
                .select(HookEvent::PreToolUse, Some("read"))
                .handlers
                .is_empty()
        );
    }

    #[test]
    fn an_event_with_no_entry_selects_nothing() {
        assert!(
            parse(SAMPLE)
                .select(HookEvent::PostToolUse, Some("bash"))
                .handlers
                .is_empty()
        );
    }

    #[test]
    fn an_absent_timeout_takes_the_default() {
        let file =
            parse(r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"./x.sh"}]}]}}"#);
        let selection = file.select(HookEvent::Stop, None);
        assert_eq!(
            selection.handlers[0].timeout_secs,
            CommandHook::DEFAULT_TIMEOUT_SECS
        );
    }

    #[test]
    fn an_event_with_no_match_query_runs_every_group_matcher_or_not() {
        // Stop has nothing to match on; a matcher on its group must not
        // silently disable it.
        let file = parse(
            r#"{"hooks":{"Stop":[{"matcher":"whatever","hooks":[
                 {"type":"command","command":"./x.sh"}]}]}}"#,
        );
        assert_eq!(file.select(HookEvent::Stop, None).handlers.len(), 1);
    }

    #[test]
    fn a_handler_type_we_do_not_implement_is_skipped_with_a_warning() {
        let file = parse(
            r#"{"hooks":{"PreToolUse":[{"hooks":[
                 {"type":"mcp_tool","command":"ignored"},
                 {"type":"command","command":"./ok.sh"}]}]}}"#,
        );
        let selection = file.select(HookEvent::PreToolUse, Some("bash"));
        assert_eq!(
            selection.handlers.len(),
            1,
            "the command handler still runs"
        );
        assert_eq!(selection.handlers[0].command, "./ok.sh");
        assert_eq!(selection.warnings.len(), 1);
        assert!(
            selection.warnings[0].contains("not \"command\""),
            "{:?}",
            selection.warnings
        );
    }

    #[test]
    fn an_event_name_we_do_not_model_loads_fine_and_never_fires() {
        let file = parse(
            r#"{"hooks":{"FileChanged":[{"hooks":[{"type":"command","command":"./x.sh"}]}]}}"#,
        );
        for &event in HookEvent::ALL {
            assert!(file.select(event, Some("bash")).handlers.is_empty());
        }
    }

    #[test]
    fn a_command_handler_with_no_command_is_skipped_with_a_warning() {
        let file = parse(r#"{"hooks":{"PreToolUse":[{"hooks":[{"type":"command"}]}]}}"#);
        let selection = file.select(HookEvent::PreToolUse, Some("bash"));
        assert!(selection.handlers.is_empty());
        assert_eq!(selection.warnings.len(), 1);
        assert!(selection.warnings[0].contains("no command"));
    }

    #[test]
    fn an_uncompilable_matcher_is_skipped_and_warned_not_silently_matched() {
        let file = parse(
            r#"{"hooks":{"PreToolUse":[
                 {"matcher":"[","hooks":[{"type":"command","command":"./bad.sh"}]},
                 {"matcher":"bash","hooks":[{"type":"command","command":"./ok.sh"}]}]}}"#,
        );
        let selection = file.select(HookEvent::PreToolUse, Some("bash"));
        assert_eq!(selection.handlers.len(), 1);
        assert_eq!(selection.handlers[0].command, "./ok.sh");
        assert_eq!(selection.warnings.len(), 1);
        assert!(selection.warnings[0].contains("not a valid regex"));
    }

    #[test]
    fn the_same_command_matched_twice_runs_once() {
        let file = parse(
            r#"{"hooks":{"PreToolUse":[
                 {"matcher":"bash","hooks":[{"type":"command","command":"./lint.sh"}]},
                 {"matcher":"*","hooks":[{"type":"command","command":"./lint.sh"}]}]}}"#,
        );
        assert_eq!(
            file.select(HookEvent::PreToolUse, Some("bash"))
                .handlers
                .len(),
            1
        );
    }

    #[test]
    fn handlers_keep_their_configured_order_across_groups() {
        let file = parse(
            r#"{"hooks":{"PreToolUse":[
                 {"matcher":"bash","hooks":[{"type":"command","command":"./a.sh"}]},
                 {"matcher":"*","hooks":[{"type":"command","command":"./b.sh"}]}]}}"#,
        );
        let selection = file.select(HookEvent::PreToolUse, Some("bash"));
        let commands: Vec<&str> = selection
            .handlers
            .iter()
            .map(|h| h.command.as_str())
            .collect();
        assert_eq!(commands, vec!["./a.sh", "./b.sh"]);
    }

    #[test]
    fn an_empty_file_is_an_empty_config_not_an_error() {
        assert_eq!(HooksFile::parse(""), Ok(HooksFile::default()));
        assert_eq!(HooksFile::parse("  \n "), Ok(HooksFile::default()));
        assert_eq!(HooksFile::parse("{}"), Ok(HooksFile::default()));
    }

    #[test]
    fn a_malformed_file_is_an_error_not_a_silent_empty_config() {
        assert!(HooksFile::parse("{ not json").is_err());
        // A typo'd key is caught too — deny_unknown_fields — so a user who
        // wrote "hook" instead of "hooks" hears about it.
        assert!(HooksFile::parse(r#"{"hook":{}}"#).is_err());
    }

    #[test]
    fn is_empty_reports_whether_anything_can_actually_run() {
        assert!(HooksFile::default().is_empty());
        assert!(parse(r#"{"hooks":{"PreToolUse":[]}}"#).is_empty());
        assert!(parse(r#"{"hooks":{"PreToolUse":[{"hooks":[{"type":"prompt"}]}]}}"#).is_empty());
        assert!(!parse(SAMPLE).is_empty());
    }
}
