//! Reading one handler's answer, and merging several (`docs/hooks.md`).
//!
//! A handler answers on **stdout**, in `camelCase` — the mirror of the
//! `snake_case` payload it was fed. Its **exit code** matters as much as its
//! output, and the three-way split is Claude Code's, copied verbatim because
//! scripts in the wild depend on it:
//!
//! | exit | meaning                                                          |
//! | ---- | ---------------------------------------------------------------- |
//! | `0`  | success — stdout is a verdict when it starts with `{`, else text  |
//! | `2`  | **block**, and the reason is **stderr**, not stdout               |
//! | else | a non-blocking error: surfaced to the user, no effect on the turn |
//!
//! Two failure modes are deliberately *not* blocks. A handler that could not
//! be spawned, or that ran past its timeout, **fails open** with a warning —
//! both references do this, and the alternative (a broken hook wedging every
//! tool call) is worse than the guard being briefly absent. Stdout that opens
//! with `{` but does not parse is a non-blocking error too, never a silent
//! allow: it means the script meant to say something and got it wrong.

use serde_json::Value;

use super::event::HookEvent;

/// How much of a handler's stdout/stderr is kept. codex spills an oversized
/// `additionalContext` to a file and hands the model a preview; we cap and
/// truncate until someone needs more (`docs/hooks.md`).
pub const HOOK_OUTPUT_MAX_BYTES: usize = 64 * 1024;

/// What one handler's run produced — the boundary's whole report, so every
/// rule below is testable from a fixture with no process anywhere.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HookRun {
    /// The configured command, for warning messages.
    pub command: String,
    /// `None` when the child never produced one — a spawn failure, a timeout,
    /// or a signal.
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    /// Why there is no exit code, when there isn't one.
    pub error: Option<String>,
}

/// What a hook said about a call it was asked to gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookPermission {
    /// Run it without asking the user.
    Allow,
    /// Refuse it.
    Deny,
    /// Put it to the user after all — the default path, stated explicitly.
    Ask,
}

/// One handler's answer, normalized.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedHook {
    /// Set when this handler blocked, with the reason it gave.
    pub block_reason: Option<String>,
    pub permission: Option<HookPermission>,
    /// The reason behind [`permission`](Self::permission), when given.
    pub permission_reason: Option<String>,
    /// A replacement `tool_input`, as JSON object text.
    pub updated_input: Option<String>,
    pub additional_context: Option<String>,
    /// A line for the *user*, not the model.
    pub system_message: Option<String>,
    /// `"continue": false` — stop the turn entirely.
    pub stopped: bool,
    pub stop_reason: Option<String>,
    /// Something was wrong with the hook itself. Never blocks.
    pub warning: Option<String>,
}

/// Several handlers' answers, resolved into the one the caller acts on.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HookOutcome {
    pub block_reason: Option<String>,
    pub permission: Option<HookPermission>,
    pub permission_reason: Option<String>,
    pub updated_input: Option<String>,
    /// In handler order, so a user reading two linters' notes sees them the
    /// way the config lists them.
    pub additional_context: Vec<String>,
    pub system_messages: Vec<String>,
    pub stopped: bool,
    pub stop_reason: Option<String>,
    pub warnings: Vec<String>,
}

impl HookOutcome {
    /// Did anything at all come back? A wholly silent set of hooks costs the
    /// caller nothing downstream.
    #[must_use]
    pub fn is_quiet(&self) -> bool {
        *self == Self::default()
    }

    /// The `additionalContext` entries as one block, or `None` when there
    /// were none.
    #[must_use]
    pub fn context_note(&self) -> Option<String> {
        if self.additional_context.is_empty() {
            return None;
        }
        Some(self.additional_context.join("\n\n"))
    }
}

/// Cut `text` to [`HOOK_OUTPUT_MAX_BYTES`] on a char boundary, marking it when
/// something was dropped so a truncated reason never reads as a complete one.
#[must_use]
pub fn truncate_output(text: &str) -> String {
    if text.len() <= HOOK_OUTPUT_MAX_BYTES {
        return text.to_string();
    }
    let mut end = HOOK_OUTPUT_MAX_BYTES;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n… (hook output truncated)", &text[..end])
}

/// Stands in when a hook refuses a call but says nothing about why — the cell
/// and the model both need *something*, and "the hook said no" beats a blank.
const UNEXPLAINED: &str = "blocked by a hook (no reason given)";

/// The events whose plain-text stdout becomes context. Claude Code's rule:
/// for these, a handler that just prints is feeding the model — for
/// `PreCompact` specifically, printing appends **compact instructions**
/// (`hooks.ts` merges successful stdout into the summarization prompt);
/// everywhere else printing is chatter and only a JSON verdict carries
/// meaning.
const PLAIN_STDOUT_IS_CONTEXT: &[HookEvent] = &[
    HookEvent::UserPromptSubmit,
    HookEvent::SessionStart,
    HookEvent::SubagentStart,
    HookEvent::PreCompact,
];

/// Read one handler's run.
#[must_use]
pub fn parse_run(run: &HookRun, event: HookEvent) -> ParsedHook {
    // The child never reported a status: it could not be spawned, it timed
    // out, or a signal took it. Fail **open** with a warning — a broken hook
    // wedging every tool call is worse than the guard being briefly absent,
    // and it is what both references do.
    if run.exit_code.is_none() || run.error.is_some() {
        let why = run.error.as_deref().unwrap_or("no exit status");
        return ParsedHook {
            warning: Some(format!("hook {} did not run: {why}", run.command)),
            ..ParsedHook::default()
        };
    }
    let code = run.exit_code.unwrap_or_default();
    // Exit 2 is the shell-native block, and its reason is **stderr** — the
    // one place the contract reads the other stream.
    if code == 2 {
        let reason = run.stderr.trim();
        let reason = if reason.is_empty() {
            UNEXPLAINED
        } else {
            reason
        };
        return ParsedHook {
            block_reason: Some(truncate_output(reason)),
            ..ParsedHook::default()
        };
    }
    if code != 0 {
        let detail = first_non_empty(&[&run.stderr, &run.stdout]).unwrap_or("no output");
        return ParsedHook {
            warning: Some(format!(
                "hook {} failed (exit {code}): {}",
                run.command,
                truncate_output(detail)
            )),
            ..ParsedHook::default()
        };
    }

    let stdout = run.stdout.trim();
    if stdout.is_empty() {
        return ParsedHook::default();
    }
    if !stdout.starts_with('{') {
        // Plain text. For the context-taking events it *is* the answer;
        // elsewhere the handler simply printed something.
        let additional_context = PLAIN_STDOUT_IS_CONTEXT
            .contains(&event)
            .then(|| truncate_output(stdout));
        return ParsedHook {
            additional_context,
            ..ParsedHook::default()
        };
    }
    let Ok(value) = serde_json::from_str::<Value>(stdout) else {
        // It meant to say something and got it wrong. Never a silent allow.
        return ParsedHook {
            warning: Some(format!(
                "hook {} printed invalid JSON: {}",
                run.command,
                truncate_output(stdout)
            )),
            ..ParsedHook::default()
        };
    };
    parse_verdict(&value, run, event)
}

/// The `camelCase` verdict object, field by field. Unknown keys are ignored
/// rather than rejected: Claude Code has twenty-seven events' worth of
/// `hookSpecificOutput` shapes, and a script written against a newer one must
/// still be understood as far as it goes.
fn parse_verdict(value: &Value, run: &HookRun, event: HookEvent) -> ParsedHook {
    let mut parsed = ParsedHook {
        stopped: value["continue"].as_bool() == Some(false),
        stop_reason: text(&value["stopReason"]),
        system_message: text(&value["systemMessage"]),
        ..ParsedHook::default()
    };
    let mut warnings: Vec<String> = Vec::new();

    // The legacy top-level `decision`.
    match value["decision"].as_str() {
        Some("block") => match text(&value["reason"]) {
            Some(reason) => parsed.block_reason = Some(truncate_output(&reason)),
            // codex's rule, and a good one: a refusal nobody can explain is
            // more likely a script bug than a policy.
            None => warnings.push(format!(
                "hook {} returned decision \"block\" with no reason — ignored",
                run.command
            )),
        },
        Some("approve") => parsed.permission = Some(HookPermission::Allow),
        _ => {}
    }

    let specific = &value["hookSpecificOutput"];
    if let Some(context) = text(&specific["additionalContext"]) {
        parsed.additional_context = Some(truncate_output(&context));
    }
    if event == HookEvent::PermissionRequest {
        // This event alone nests its verdict: `decision: {behavior, message}`.
        match specific["decision"]["behavior"].as_str() {
            Some("allow") => parsed.permission = Some(HookPermission::Allow),
            Some("deny") => {
                parsed.permission = Some(HookPermission::Deny);
                let message = text(&specific["decision"]["message"]);
                parsed.permission_reason.clone_from(&message);
                parsed.block_reason =
                    Some(truncate_output(message.as_deref().unwrap_or(UNEXPLAINED)));
            }
            _ => {}
        }
    }
    if let Some(decision) = specific["permissionDecision"].as_str() {
        let reason = text(&specific["permissionDecisionReason"]);
        match decision {
            "allow" => parsed.permission = Some(HookPermission::Allow),
            "ask" => parsed.permission = Some(HookPermission::Ask),
            "deny" => {
                parsed.permission = Some(HookPermission::Deny);
                // A deny *is* a block, so callers that only look at
                // `block_reason` (every one that cannot ask a user) behave.
                parsed.block_reason =
                    Some(truncate_output(reason.as_deref().unwrap_or(UNEXPLAINED)));
            }
            other => warnings.push(format!(
                "hook {} returned unknown permissionDecision {other:?} — ignored",
                run.command
            )),
        }
        if parsed.permission.is_some() {
            parsed.permission_reason = reason;
        }
    }
    match &specific["updatedInput"] {
        Value::Null => {}
        object @ Value::Object(_) => parsed.updated_input = Some(object.to_string()),
        // Anything else would change `tool_input`'s type out from under the
        // tool, so it is refused rather than applied.
        _ => warnings.push(format!(
            "hook {} returned a non-object updatedInput — ignored",
            run.command
        )),
    }

    if !warnings.is_empty() {
        parsed.warning = Some(warnings.join("; "));
    }
    parsed
}

/// A JSON string field, `None` when absent, not a string, or blank — a hook
/// that sets `"reason": ""` has not given one.
fn text(value: &Value) -> Option<String> {
    let text = value.as_str()?.trim();
    (!text.is_empty()).then(|| text.to_string())
}

fn first_non_empty<'a>(candidates: &[&'a String]) -> Option<&'a str> {
    candidates.iter().map(|s| s.trim()).find(|s| !s.is_empty())
}

/// How firmly a verdict speaks, so the strictest of several wins.
const fn rank(permission: HookPermission) -> u8 {
    match permission {
        HookPermission::Allow => 0,
        HookPermission::Ask => 1,
        HookPermission::Deny => 2,
    }
}

/// Resolve several handlers' answers: **any** block wins and the **first**
/// reason is kept; a `deny` outranks an `ask`, which outranks an `allow`; the
/// first `updatedInput` wins; contexts and warnings accumulate in order.
#[must_use]
pub fn merge(parsed: Vec<ParsedHook>) -> HookOutcome {
    let mut outcome = HookOutcome::default();
    for hook in parsed {
        if outcome.block_reason.is_none() {
            outcome.block_reason = hook.block_reason;
        }
        // Strictest wins; among equals the first, so a config's order still
        // decides whose reason the user reads.
        let stricter = match (outcome.permission, hook.permission) {
            (Some(have), Some(new)) => rank(new) > rank(have),
            (None, Some(_)) => true,
            _ => false,
        };
        if stricter {
            outcome.permission = hook.permission;
            outcome.permission_reason = hook.permission_reason;
        }
        if outcome.updated_input.is_none() {
            outcome.updated_input = hook.updated_input;
        }
        outcome.additional_context.extend(hook.additional_context);
        outcome.system_messages.extend(hook.system_message);
        outcome.warnings.extend(hook.warning);
        outcome.stopped |= hook.stopped;
        if outcome.stop_reason.is_none() {
            outcome.stop_reason = hook.stop_reason;
        }
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok_stdout(stdout: &str) -> HookRun {
        HookRun {
            command: "./h.sh".into(),
            exit_code: Some(0),
            stdout: stdout.into(),
            stderr: String::new(),
            error: None,
        }
    }

    fn parse(stdout: &str) -> ParsedHook {
        parse_run(&ok_stdout(stdout), HookEvent::PreToolUse)
    }

    // --- exit codes ---

    #[test]
    fn exit_two_blocks_with_stderr_as_the_reason_not_stdout() {
        let run = HookRun {
            command: "./h.sh".into(),
            exit_code: Some(2),
            stdout: "this is not the reason".into(),
            stderr: "no destructive deletes".into(),
            error: None,
        };
        let parsed = parse_run(&run, HookEvent::PreToolUse);
        assert_eq!(
            parsed.block_reason.as_deref(),
            Some("no destructive deletes")
        );
        assert!(parsed.warning.is_none());
    }

    #[test]
    fn exit_two_with_no_stderr_still_blocks_with_a_stand_in_reason() {
        let run = HookRun {
            command: "./h.sh".into(),
            exit_code: Some(2),
            ..HookRun::default()
        };
        let parsed = parse_run(&run, HookEvent::PreToolUse);
        assert!(parsed.block_reason.is_some(), "exit 2 always blocks");
    }

    #[test]
    fn any_other_non_zero_exit_is_a_warning_and_never_blocks() {
        let run = HookRun {
            command: "./h.sh".into(),
            exit_code: Some(1),
            stderr: "command not found".into(),
            ..HookRun::default()
        };
        let parsed = parse_run(&run, HookEvent::PreToolUse);
        assert_eq!(parsed.block_reason, None);
        let warning = parsed.warning.expect("a failing hook is reported");
        assert!(warning.contains("command not found"), "{warning}");
    }

    #[test]
    fn a_hook_that_could_not_run_fails_open_with_a_warning() {
        let run = HookRun {
            command: "./missing.sh".into(),
            exit_code: None,
            error: Some("timed out after 60s".into()),
            ..HookRun::default()
        };
        let parsed = parse_run(&run, HookEvent::PreToolUse);
        assert_eq!(
            parsed.block_reason, None,
            "a broken hook must not wedge the agent"
        );
        assert_eq!(parsed.permission, None);
        let warning = parsed.warning.expect("the failure is reported");
        assert!(warning.contains("timed out"), "{warning}");
    }

    // --- stdout shapes ---

    #[test]
    fn a_silent_success_says_nothing_at_all() {
        assert_eq!(parse(""), ParsedHook::default());
        assert_eq!(parse("   \n"), ParsedHook::default());
    }

    #[test]
    fn plain_stdout_is_context_only_for_the_events_that_take_it() {
        let run = ok_stdout("remember: the API moved");
        assert_eq!(
            parse_run(&run, HookEvent::UserPromptSubmit)
                .additional_context
                .as_deref(),
            Some("remember: the API moved")
        );
        assert_eq!(
            parse_run(&run, HookEvent::SessionStart)
                .additional_context
                .as_deref(),
            Some("remember: the API moved")
        );
        // PreCompact's stdout is Claude Code's "custom compact
        // instructions" channel — context too.
        assert_eq!(
            parse_run(&run, HookEvent::PreCompact)
                .additional_context
                .as_deref(),
            Some("remember: the API moved")
        );
        // A tool-path hook's chatter is informational, not context.
        assert_eq!(
            parse_run(&run, HookEvent::PreToolUse).additional_context,
            None
        );
    }

    #[test]
    fn stdout_that_opens_a_brace_but_does_not_parse_is_a_warning_not_a_silent_allow() {
        let parsed = parse("{ oops");
        assert!(
            parsed.warning.is_some(),
            "a broken verdict must be reported"
        );
        assert_eq!(parsed.block_reason, None);
        assert_eq!(parsed.permission, None);
    }

    // --- the universal envelope ---

    #[test]
    fn continue_false_stops_the_turn_and_keeps_its_reason() {
        let parsed = parse(r#"{"continue":false,"stopReason":"budget spent"}"#);
        assert!(parsed.stopped);
        assert_eq!(parsed.stop_reason.as_deref(), Some("budget spent"));
    }

    #[test]
    fn continue_defaults_to_true_when_the_field_is_absent() {
        assert!(!parse(r#"{"systemMessage":"hi"}"#).stopped);
    }

    #[test]
    fn a_system_message_is_carried_for_the_user() {
        assert_eq!(
            parse(r#"{"systemMessage":"linted 3 files"}"#)
                .system_message
                .as_deref(),
            Some("linted 3 files")
        );
    }

    // --- decisions ---

    #[test]
    fn the_legacy_decision_block_form_blocks_with_its_reason() {
        let parsed = parse(r#"{"decision":"block","reason":"not on main"}"#);
        assert_eq!(parsed.block_reason.as_deref(), Some("not on main"));
    }

    #[test]
    fn a_block_with_no_reason_is_a_warning_rather_than_a_mystery_refusal() {
        let parsed = parse(r#"{"decision":"block"}"#);
        assert_eq!(parsed.block_reason, None);
        assert!(parsed.warning.is_some());
        let parsed = parse(r#"{"decision":"block","reason":"  "}"#);
        assert_eq!(parsed.block_reason, None);
        assert!(parsed.warning.is_some());
    }

    #[test]
    fn decision_approve_is_an_allow_on_pre_tool_use() {
        assert_eq!(
            parse(r#"{"decision":"approve"}"#).permission,
            Some(HookPermission::Allow)
        );
    }

    #[test]
    fn the_permission_decision_triple_maps_straight_through() {
        for (text, want) in [
            ("allow", HookPermission::Allow),
            ("deny", HookPermission::Deny),
            ("ask", HookPermission::Ask),
        ] {
            let json = format!(
                r#"{{"hookSpecificOutput":{{"hookEventName":"PreToolUse",
                     "permissionDecision":"{text}","permissionDecisionReason":"because"}}}}"#
            );
            let parsed = parse(&json);
            assert_eq!(parsed.permission, Some(want), "{text}");
            assert_eq!(parsed.permission_reason.as_deref(), Some("because"));
        }
    }

    #[test]
    fn a_pre_tool_use_deny_also_reads_as_a_block_so_one_path_handles_both() {
        let parsed = parse(
            r#"{"hookSpecificOutput":{"hookEventName":"PreToolUse",
                 "permissionDecision":"deny","permissionDecisionReason":"read-only day"}}"#,
        );
        assert_eq!(parsed.permission, Some(HookPermission::Deny));
        assert_eq!(parsed.block_reason.as_deref(), Some("read-only day"));
    }

    #[test]
    fn permission_request_uses_its_own_nested_behaviour_object() {
        let allow = parse_run(
            &ok_stdout(
                r#"{"hookSpecificOutput":{"hookEventName":"PermissionRequest",
                     "decision":{"behavior":"allow"}}}"#,
            ),
            HookEvent::PermissionRequest,
        );
        assert_eq!(allow.permission, Some(HookPermission::Allow));

        let deny = parse_run(
            &ok_stdout(
                r#"{"hookSpecificOutput":{"hookEventName":"PermissionRequest",
                     "decision":{"behavior":"deny","message":"never in prod"}}}"#,
            ),
            HookEvent::PermissionRequest,
        );
        assert_eq!(deny.permission, Some(HookPermission::Deny));
        assert_eq!(deny.block_reason.as_deref(), Some("never in prod"));
    }

    #[test]
    fn updated_input_comes_back_as_object_text_the_caller_can_hand_to_the_tool() {
        let parsed = parse(
            r#"{"hookSpecificOutput":{"hookEventName":"PreToolUse",
                 "updatedInput":{"command":"ls -la"}}}"#,
        );
        let text = parsed.updated_input.expect("an updated input");
        let value: Value = serde_json::from_str(&text).expect("valid JSON object");
        assert_eq!(value["command"], Value::String("ls -la".into()));
    }

    #[test]
    fn a_non_object_updated_input_is_refused_rather_than_corrupting_the_call() {
        let parsed =
            parse(r#"{"hookSpecificOutput":{"hookEventName":"PreToolUse","updatedInput":"ls"}}"#);
        assert_eq!(parsed.updated_input, None);
        assert!(parsed.warning.is_some());
    }

    #[test]
    fn additional_context_is_read_from_the_hook_specific_object() {
        let parsed = parse(
            r#"{"hookSpecificOutput":{"hookEventName":"PostToolUse",
                 "additionalContext":"the linter reformatted it"}}"#,
        );
        assert_eq!(
            parsed.additional_context.as_deref(),
            Some("the linter reformatted it")
        );
    }

    #[test]
    fn an_unknown_hook_specific_field_does_not_fail_the_verdict() {
        // Claude Code has twenty-seven events' worth of these; a config or a
        // script written against a newer one must still be understood as far
        // as it goes.
        let parsed = parse(
            r#"{"systemMessage":"ok","hookSpecificOutput":{"hookEventName":"PreToolUse",
                 "watchPaths":["/a"],"additionalContext":"c"}}"#,
        );
        assert_eq!(parsed.additional_context.as_deref(), Some("c"));
        assert_eq!(parsed.system_message.as_deref(), Some("ok"));
        assert!(parsed.warning.is_none());
    }

    // --- merging ---

    #[test]
    fn merging_nothing_is_quiet() {
        assert!(merge(Vec::new()).is_quiet());
        assert!(merge(vec![ParsedHook::default()]).is_quiet());
    }

    #[test]
    fn any_block_wins_and_the_first_reason_is_kept() {
        let outcome = merge(vec![
            ParsedHook::default(),
            ParsedHook {
                block_reason: Some("first".into()),
                ..ParsedHook::default()
            },
            ParsedHook {
                block_reason: Some("second".into()),
                ..ParsedHook::default()
            },
        ]);
        assert_eq!(outcome.block_reason.as_deref(), Some("first"));
    }

    #[test]
    fn deny_outranks_ask_which_outranks_allow_whatever_the_order() {
        let allow = ParsedHook {
            permission: Some(HookPermission::Allow),
            ..ParsedHook::default()
        };
        let ask = ParsedHook {
            permission: Some(HookPermission::Ask),
            ..ParsedHook::default()
        };
        let deny = ParsedHook {
            permission: Some(HookPermission::Deny),
            permission_reason: Some("no".into()),
            ..ParsedHook::default()
        };
        assert_eq!(
            merge(vec![allow.clone(), deny.clone()]).permission,
            Some(HookPermission::Deny)
        );
        assert_eq!(
            merge(vec![deny.clone(), allow.clone()]).permission,
            Some(HookPermission::Deny)
        );
        assert_eq!(
            merge(vec![allow.clone(), ask.clone()]).permission,
            Some(HookPermission::Ask)
        );
        assert_eq!(
            merge(vec![ask, allow.clone()]).permission,
            Some(HookPermission::Ask)
        );
        assert_eq!(merge(vec![allow]).permission, Some(HookPermission::Allow));
        // The winning verdict's own reason travels with it.
        assert_eq!(
            merge(vec![
                ParsedHook {
                    permission: Some(HookPermission::Allow),
                    permission_reason: Some("yes".into()),
                    ..ParsedHook::default()
                },
                deny
            ])
            .permission_reason
            .as_deref(),
            Some("no")
        );
    }

    #[test]
    fn contexts_concatenate_in_handler_order() {
        let outcome = merge(vec![
            ParsedHook {
                additional_context: Some("one".into()),
                ..ParsedHook::default()
            },
            ParsedHook::default(),
            ParsedHook {
                additional_context: Some("two".into()),
                ..ParsedHook::default()
            },
        ]);
        assert_eq!(outcome.additional_context, vec!["one", "two"]);
        assert_eq!(outcome.context_note().as_deref(), Some("one\n\ntwo"));
    }

    #[test]
    fn the_first_updated_input_wins_so_two_rewriters_cannot_fight() {
        let outcome = merge(vec![
            ParsedHook {
                updated_input: Some(r#"{"a":1}"#.into()),
                ..ParsedHook::default()
            },
            ParsedHook {
                updated_input: Some(r#"{"b":2}"#.into()),
                ..ParsedHook::default()
            },
        ]);
        assert_eq!(outcome.updated_input.as_deref(), Some(r#"{"a":1}"#));
    }

    #[test]
    fn warnings_system_messages_and_stops_all_accumulate() {
        let outcome = merge(vec![
            ParsedHook {
                warning: Some("w1".into()),
                system_message: Some("s1".into()),
                ..ParsedHook::default()
            },
            ParsedHook {
                warning: Some("w2".into()),
                system_message: Some("s2".into()),
                stopped: true,
                stop_reason: Some("halt".into()),
                ..ParsedHook::default()
            },
        ]);
        assert_eq!(outcome.warnings, vec!["w1", "w2"]);
        assert_eq!(outcome.system_messages, vec!["s1", "s2"]);
        assert!(outcome.stopped);
        assert_eq!(outcome.stop_reason.as_deref(), Some("halt"));
    }

    #[test]
    fn a_warning_alone_is_not_quiet_so_the_user_hears_about_a_broken_hook() {
        let outcome = merge(vec![ParsedHook {
            warning: Some("boom".into()),
            ..ParsedHook::default()
        }]);
        assert!(!outcome.is_quiet());
    }

    // --- truncation ---

    #[test]
    fn short_output_is_returned_untouched() {
        assert_eq!(truncate_output("hello"), "hello");
    }

    #[test]
    fn oversized_output_is_cut_and_marked() {
        let big = "x".repeat(HOOK_OUTPUT_MAX_BYTES + 100);
        let cut = truncate_output(&big);
        assert!(cut.len() < big.len());
        assert!(cut.ends_with("(hook output truncated)"));
    }

    #[test]
    fn truncation_never_splits_a_multibyte_character() {
        let big = "é".repeat(HOOK_OUTPUT_MAX_BYTES);
        let cut = truncate_output(&big);
        assert!(cut.starts_with('é'), "still valid UTF-8");
    }
}
