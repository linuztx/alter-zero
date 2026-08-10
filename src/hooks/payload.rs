//! The JSON each event writes to a handler's **stdin** (`docs/hooks.md`).
//!
//! Every field name here is `snake_case`, deliberately: the payload half of
//! the contract is `snake_case` and the verdict half is `camelCase`, and both
//! references agree on that asymmetry. A hook script written for Claude Code
//! reads `.tool_name` and `.tool_input`, so these spellings are the whole
//! point of the module and are asserted verbatim in the tests below.
//!
//! Every payload carries the same base — `session_id`, `transcript_path`,
//! `cwd`, `model`, `permission_mode`, and the optional `agent_id` /
//! `agent_type` that tell a hook it was a *subagent* that acted.

use serde_json::{Map, Value, json};

use super::event::HookEvent;

/// The session-level facts every payload repeats. Built once at the boundary
/// (where the cwd, the model name and the rollout path are known) and cloned
/// into each event.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HookContext {
    /// The `/resume` rollout's session id.
    pub session_id: String,
    /// The rollout file this conversation is recorded to, when one is being
    /// written — a hook that wants the transcript reads it from here.
    pub transcript_path: Option<String>,
    pub cwd: String,
    pub model: String,
    /// `manual` / `edit` / `auto` / `master`, or `None` when the permission
    /// gate is off entirely.
    pub permission_mode: Option<String>,
    /// Set when a **subagent** is acting: Claude Code's own rule is that a
    /// hook distinguishes a subagent call from a main-thread one by
    /// `agent_id`'s presence.
    pub agent_id: Option<String>,
    pub agent_type: Option<String>,
}

impl HookContext {
    /// The shared base object, with `hook_event_name` already stamped.
    fn base(&self, event: HookEvent) -> Map<String, Value> {
        let mut map = Map::new();
        map.insert("session_id".into(), json!(self.session_id));
        // Explicitly null rather than omitted when there is no rollout —
        // codex's `NullableString`, so a script can tell "no transcript" from
        // "field missing because you are running an old build".
        map.insert("transcript_path".into(), json!(self.transcript_path));
        map.insert("cwd".into(), json!(self.cwd));
        map.insert("model".into(), json!(self.model));
        map.insert(
            "permission_mode".into(),
            json!(self.permission_mode.as_deref().unwrap_or("disabled")),
        );
        map.insert("hook_event_name".into(), json!(event.name()));
        // Omitted, not null, for the main agent — presence is the signal.
        if let Some(id) = &self.agent_id {
            map.insert("agent_id".into(), json!(id));
        }
        if let Some(kind) = &self.agent_type {
            map.insert("agent_type".into(), json!(kind));
        }
        map
    }
}

/// A tool call's arguments as a JSON **object**, which is what `tool_input`
/// is on the wire. The model's arguments arrive as a string; anything that is
/// not a JSON object (a malformed emission) degrades to `{}` rather than
/// changing the field's type out from under a `jq '.tool_input.command'`.
fn tool_input(arguments: &str) -> Value {
    match serde_json::from_str::<Value>(arguments) {
        Ok(value @ Value::Object(_)) => value,
        _ => Value::Object(Map::new()),
    }
}

fn finish(map: Map<String, Value>) -> String {
    Value::Object(map).to_string()
}

/// `PreToolUse` — before a call runs, so `tool_input` is what *would* run.
#[must_use]
pub fn pre_tool_use_payload(
    ctx: &HookContext,
    tool_name: &str,
    arguments: &str,
    tool_use_id: &str,
) -> String {
    let mut map = ctx.base(HookEvent::PreToolUse);
    map.insert("tool_name".into(), json!(tool_name));
    map.insert("tool_input".into(), tool_input(arguments));
    map.insert("tool_use_id".into(), json!(tool_use_id));
    finish(map)
}

/// `PermissionRequest` — the call is about to be put to the user.
#[must_use]
pub fn permission_request_payload(
    ctx: &HookContext,
    tool_name: &str,
    arguments: &str,
    tool_use_id: &str,
) -> String {
    let mut map = ctx.base(HookEvent::PermissionRequest);
    map.insert("tool_name".into(), json!(tool_name));
    map.insert("tool_input".into(), tool_input(arguments));
    map.insert("tool_use_id".into(), json!(tool_use_id));
    finish(map)
}

/// `PostToolUse` — the call ran; `tool_response` carries what it produced.
///
/// Our tools return text, not a structured result, so the response is an
/// object with the text under `output` beside the `success` flag: a stable
/// shape a script can read, rather than sometimes-a-string.
#[must_use]
pub fn post_tool_use_payload(
    ctx: &HookContext,
    tool_name: &str,
    arguments: &str,
    output: &str,
    ok: bool,
    tool_use_id: &str,
) -> String {
    let mut map = ctx.base(HookEvent::PostToolUse);
    map.insert("tool_name".into(), json!(tool_name));
    map.insert("tool_input".into(), tool_input(arguments));
    map.insert(
        "tool_response".into(),
        json!({ "output": output, "success": ok }),
    );
    map.insert("tool_use_id".into(), json!(tool_use_id));
    finish(map)
}

/// `UserPromptSubmit` — the user sent `prompt`.
#[must_use]
pub fn user_prompt_submit_payload(ctx: &HookContext, prompt: &str) -> String {
    let mut map = ctx.base(HookEvent::UserPromptSubmit);
    map.insert("prompt".into(), json!(prompt));
    finish(map)
}

/// `SessionStart` — `source` is `startup` / `resume` / `clear`.
#[must_use]
pub fn session_start_payload(ctx: &HookContext, source: &str) -> String {
    let mut map = ctx.base(HookEvent::SessionStart);
    map.insert("source".into(), json!(source));
    finish(map)
}

/// `SessionEnd` — `reason` is `clear` / `resume` / `other`.
#[must_use]
pub fn session_end_payload(ctx: &HookContext, reason: &str) -> String {
    let mut map = ctx.base(HookEvent::SessionEnd);
    map.insert("reason".into(), json!(reason));
    finish(map)
}

/// `Stop` — the model finished answering. `stop_hook_active` is true when
/// this turn was itself started by a `Stop` hook's block, which is how a hook
/// avoids looping forever.
#[must_use]
pub fn stop_payload(ctx: &HookContext, stop_hook_active: bool, last_message: &str) -> String {
    let mut map = ctx.base(HookEvent::Stop);
    map.insert("stop_hook_active".into(), json!(stop_hook_active));
    map.insert("last_assistant_message".into(), json!(last_message));
    finish(map)
}

/// `SubagentStart` — a subagent was launched.
#[must_use]
pub fn subagent_start_payload(ctx: &HookContext, agent_id: &str, agent_type: &str) -> String {
    let mut map = ctx.base(HookEvent::SubagentStart);
    map.insert("agent_id".into(), json!(agent_id));
    map.insert("agent_type".into(), json!(agent_type));
    finish(map)
}

/// `SubagentStop` — a subagent finished.
///
/// `stop_hook_active` is always `false`: it means *this run was started by a
/// `Stop` hook's block*, the loop guard, and nothing here can do that yet.
/// Reporting a failed agent through it — the obvious-looking shortcut — would
/// hand a hook author the wrong fact under a documented name, so success rides
/// its own `success` field instead (an extension, like the `tool_response`
/// object's; codex extends these payloads the same way with `turn_id`).
#[must_use]
pub fn subagent_stop_payload(
    ctx: &HookContext,
    agent_id: &str,
    agent_type: &str,
    last_message: &str,
    ok: bool,
) -> String {
    let mut map = ctx.base(HookEvent::SubagentStop);
    map.insert("stop_hook_active".into(), json!(false));
    map.insert("agent_id".into(), json!(agent_id));
    map.insert("agent_type".into(), json!(agent_type));
    map.insert("last_assistant_message".into(), json!(last_message));
    map.insert("success".into(), json!(ok));
    finish(map)
}

/// `PreCompact` — `trigger` is `manual` or `auto`.
#[must_use]
pub fn pre_compact_payload(ctx: &HookContext, trigger: &str, custom_instructions: &str) -> String {
    let mut map = ctx.base(HookEvent::PreCompact);
    map.insert("trigger".into(), json!(trigger));
    map.insert("custom_instructions".into(), json!(custom_instructions));
    finish(map)
}

/// `PostCompact` — the summary that replaced the conversation.
#[must_use]
pub fn post_compact_payload(ctx: &HookContext, trigger: &str, summary: &str) -> String {
    let mut map = ctx.base(HookEvent::PostCompact);
    map.insert("trigger".into(), json!(trigger));
    map.insert("compact_summary".into(), json!(summary));
    finish(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> HookContext {
        HookContext {
            session_id: "sess-1".into(),
            transcript_path: Some("/r/sess-1.jsonl".into()),
            cwd: "/work".into(),
            model: "gpt-x".into(),
            permission_mode: Some("manual".into()),
            agent_id: None,
            agent_type: None,
        }
    }

    fn parse(json_text: &str) -> Value {
        serde_json::from_str(json_text).expect("payload must be valid JSON")
    }

    #[test]
    fn every_payload_carries_the_snake_case_base() {
        let v = parse(&pre_tool_use_payload(&ctx(), "bash", "{}", "call-1"));
        assert_eq!(v["session_id"], json!("sess-1"));
        assert_eq!(v["transcript_path"], json!("/r/sess-1.jsonl"));
        assert_eq!(v["cwd"], json!("/work"));
        assert_eq!(v["model"], json!("gpt-x"));
        assert_eq!(v["permission_mode"], json!("manual"));
        assert_eq!(v["hook_event_name"], json!("PreToolUse"));
    }

    #[test]
    fn an_absent_transcript_is_null_not_missing() {
        let mut c = ctx();
        c.transcript_path = None;
        let v = parse(&session_start_payload(&c, "startup"));
        assert_eq!(v["transcript_path"], Value::Null);
        assert!(v.as_object().unwrap().contains_key("transcript_path"));
    }

    #[test]
    fn the_main_agent_omits_agent_fields_and_a_subagent_carries_them() {
        let v = parse(&pre_tool_use_payload(&ctx(), "bash", "{}", "c"));
        let obj = v.as_object().unwrap();
        assert!(!obj.contains_key("agent_id"), "presence is the signal");
        assert!(!obj.contains_key("agent_type"));

        let mut c = ctx();
        c.agent_id = Some("agent-7".into());
        c.agent_type = Some("general-purpose".into());
        let v = parse(&pre_tool_use_payload(&c, "bash", "{}", "c"));
        assert_eq!(v["agent_id"], json!("agent-7"));
        assert_eq!(v["agent_type"], json!("general-purpose"));
    }

    #[test]
    fn a_disabled_permission_gate_still_reports_a_mode_string() {
        let mut c = ctx();
        c.permission_mode = None;
        let v = parse(&pre_tool_use_payload(&c, "bash", "{}", "c"));
        assert_eq!(v["permission_mode"], json!("disabled"));
    }

    #[test]
    fn tool_input_is_the_parsed_object_a_script_can_index() {
        let v = parse(&pre_tool_use_payload(
            &ctx(),
            "bash",
            r#"{"command":"rm -rf /","timeout_ms":1000}"#,
            "call-9",
        ));
        assert_eq!(v["tool_name"], json!("bash"));
        assert_eq!(v["tool_input"]["command"], json!("rm -rf /"));
        assert_eq!(v["tool_input"]["timeout_ms"], json!(1000));
        assert_eq!(v["tool_use_id"], json!("call-9"));
    }

    #[test]
    fn unparseable_arguments_degrade_to_an_empty_object_not_a_string() {
        for arguments in ["", "not json", "[1,2]", "\"a string\""] {
            let v = parse(&pre_tool_use_payload(&ctx(), "bash", arguments, "c"));
            assert_eq!(
                v["tool_input"],
                json!({}),
                "{arguments:?} must stay an object"
            );
        }
    }

    #[test]
    fn post_tool_use_reports_the_output_and_the_success_flag() {
        let v = parse(&post_tool_use_payload(
            &ctx(),
            "bash",
            r#"{"command":"ls"}"#,
            "a\nb\n",
            true,
            "call-2",
        ));
        assert_eq!(v["hook_event_name"], json!("PostToolUse"));
        assert_eq!(v["tool_response"]["output"], json!("a\nb\n"));
        assert_eq!(v["tool_response"]["success"], json!(true));

        let v = parse(&post_tool_use_payload(
            &ctx(),
            "bash",
            "{}",
            "boom",
            false,
            "c",
        ));
        assert_eq!(v["tool_response"]["success"], json!(false));
    }

    #[test]
    fn each_remaining_event_stamps_its_own_name_and_fields() {
        let c = ctx();
        let cases: Vec<(String, &str)> = vec![
            (
                permission_request_payload(&c, "bash", "{}", "c"),
                "PermissionRequest",
            ),
            (user_prompt_submit_payload(&c, "hi"), "UserPromptSubmit"),
            (session_start_payload(&c, "startup"), "SessionStart"),
            (session_end_payload(&c, "other"), "SessionEnd"),
            (stop_payload(&c, false, "done"), "Stop"),
            (subagent_start_payload(&c, "a1", "explore"), "SubagentStart"),
            (
                subagent_stop_payload(&c, "a1", "explore", "found it", true),
                "SubagentStop",
            ),
            (pre_compact_payload(&c, "manual", ""), "PreCompact"),
            (post_compact_payload(&c, "auto", "summary"), "PostCompact"),
        ];
        for (text, name) in cases {
            assert_eq!(parse(&text)["hook_event_name"], json!(name));
        }

        assert_eq!(
            parse(&user_prompt_submit_payload(&c, "hi"))["prompt"],
            json!("hi")
        );
        assert_eq!(
            parse(&session_start_payload(&c, "resume"))["source"],
            json!("resume")
        );
        assert_eq!(
            parse(&session_end_payload(&c, "clear"))["reason"],
            json!("clear")
        );
        let stop = parse(&stop_payload(&c, true, "the answer"));
        assert_eq!(stop["stop_hook_active"], json!(true));
        assert_eq!(stop["last_assistant_message"], json!("the answer"));
        let sub = parse(&subagent_stop_payload(&c, "a1", "explore", "found", true));
        assert_eq!(sub["agent_id"], json!("a1"));
        assert_eq!(sub["agent_type"], json!("explore"));
        assert_eq!(sub["last_assistant_message"], json!("found"));
        assert_eq!(sub["success"], json!(true));
        // Never repurposed to mean "the agent failed": it means the run was
        // started by a Stop hook's block, which nothing here does yet.
        assert_eq!(sub["stop_hook_active"], json!(false));
        assert_eq!(
            parse(&subagent_stop_payload(&c, "a1", "explore", "boom", false))["success"],
            json!(false)
        );
        let pre = parse(&pre_compact_payload(&c, "manual", "keep the API notes"));
        assert_eq!(pre["trigger"], json!("manual"));
        assert_eq!(pre["custom_instructions"], json!("keep the API notes"));
        assert_eq!(
            parse(&post_compact_payload(&c, "auto", "s"))["compact_summary"],
            json!("s")
        );
    }

    #[test]
    fn a_subagents_own_fields_win_over_the_contexts() {
        // SubagentStart is *about* an agent, so its explicit ids are the ones
        // that must land even when the launching context carried none.
        let v = parse(&subagent_start_payload(&ctx(), "a2", "explore"));
        assert_eq!(v["agent_id"], json!("a2"));
        assert_eq!(v["agent_type"], json!("explore"));
    }

    #[test]
    fn payloads_are_one_line_so_a_handler_can_read_a_single_record() {
        let text = pre_tool_use_payload(&ctx(), "bash", r#"{"command":"a\nb"}"#, "c");
        assert!(!text.contains('\n'), "embedded newlines must stay escaped");
    }
}
