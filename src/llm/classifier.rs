//! The **auto mode classifier** (`docs/permissions.md`): the silent LLM
//! safety check a `bash` command — or an MCP tool call (`docs/mcp.md`) —
//! goes through in [`PermissionMode::Auto`] instead of the user prompt —
//! Claude Code's auto-mode classifier, on the session's own provider.
//!
//! The request/response shape follows the reference: a fixed system prompt
//! ([`prompts/classifier.md`](../../prompts/classifier.md)) with a strict
//! `<block>yes|no</block><reason>…</reason>` output contract (more robust
//! than JSON across arbitrary OpenAI-compatible models) and one user message
//! carrying a **bounded task context** plus the request. The context
//! ([`ClassifierContext`]) is the session's recent story — the last
//! [`CONTEXT_MAX_REQUESTS`] user requests and the last
//! [`CONTEXT_MAX_ACTIONS`] actions, each a one-line `Name(args)` summary —
//! so the verdict can weigh whether an action *fits the task*: `rm -rf
//! build/` after a failed `cargo build` reads differently from `rm -rf`
//! out of nowhere, and "now clean up the build output" two turns back is
//! often the request that explains the command in front of you. It is
//! deliberately **not the conversation**: both halves are rolling windows,
//! every line is truncated (`CONTEXT_*` caps), tool *outputs*
//! never ride along (the cheapest channel for a poisoned repo to lobby
//! through), the system prompt pins the block as information-never-
//! instructions, and the action under review sits under its own
//! `## Action to review` header so what is being judged can't blur into
//! what already happened. The caps alone bound it, so a verdict on turn
//! fifty costs what one on turn one did (`docs/permissions.md`). The parse
//! build are pure and unit-tested; [`SafetyClassifier::classify`] is the one
//! HTTP boundary, riding the same blocking client and cancel-polling stream
//! machinery as every other request ([`OpenAiClient::stream_chat`]).
//!
//! [`PermissionMode::Auto`]: crate::permission::PermissionMode::Auto

use std::collections::VecDeque;

use super::config::ModelConfig;
use super::openai::OpenAiClient;
use super::tools::{display_name, summarize_call};
use crate::permission::{ClassifierVerdict, PermissionKind, PermissionRequest, mcp_label};
use crate::stream::CancelToken;

/// The classifier's system prompt — authored in
/// [`prompts/classifier.md`](../../prompts/classifier.md) (the maintainable-
/// markdown seam every prompt uses, `docs/environment.md`).
pub const CLASSIFIER_SYSTEM_PROMPT: &str = include_str!("../../prompts/classifier.md");

/// The classifier's one user message: the working directory, the agent's
/// stated description (a claim, the prompt says — omitted when it gave
/// none), and the command itself, fenced so a multi-line command stays one
/// obvious block. Pure — unit-tested.
#[must_use]
pub fn classifier_user_prompt(command: &str, description: Option<&str>, cwd: &str) -> String {
    let description = match description.map(str::trim).filter(|d| !d.is_empty()) {
        Some(description) => format!("Agent's description: {description}\n"),
        None => String::new(),
    };
    format!("Working directory: {cwd}\n{description}Command:\n```\n{command}\n```")
}

/// The classifier's one user message for a whole request (`docs/mcp.md`): a
/// `bash` request keeps the [`classifier_user_prompt`] command shape, and an
/// MCP request gets the tool-call shape — the tool named `{server} - {tool}`
/// the way the prompt and the cell name it, the server's own description when
/// it gave one (a claim about the tool, like the agent's command description),
/// and the arguments fenced like a command so a multi-line value stays one
/// obvious block, an argument-less call saying so explicitly rather than
/// leaving the classifier to wonder what was omitted. Pure — unit-tested.
#[must_use]
pub fn classifier_request_prompt(request: &PermissionRequest, cwd: &str) -> String {
    match request.kind {
        PermissionKind::Mcp => {
            let description = match request
                .detail
                .as_deref()
                .map(str::trim)
                .filter(|d| !d.is_empty())
            {
                Some(description) => format!("Server's description of the tool: {description}\n"),
                None => String::new(),
            };
            let args = Some(request.body.trim()).filter(|b| !b.is_empty());
            format!(
                "Working directory: {cwd}\n{description}MCP tool call: {label}\nArguments:\n```\n{args}\n```",
                label = mcp_label(&request.target),
                args = args.unwrap_or("(no arguments)"),
            )
        }
        _ => classifier_user_prompt(&request.target, request.detail.as_deref(), cwd),
    }
}

/// The most actions the task-context block keeps. When a long run overflows
/// it, the **oldest** drop first — the recent actions are the ones the next
/// verdict reads against — behind a counted `(+N earlier actions omitted)`
/// marker so the list never reads as the whole story.
pub const CONTEXT_MAX_ACTIONS: usize = 20;

/// The most user requests the block keeps, newest last. The log spans the
/// **conversation**, not one turn: a command reads very differently under
/// "now delete the build output" than under the request three turns back
/// that a single-turn window would have shown instead. Ten is the window —
/// enough for the thread of a task, bounded so a long session's verdict
/// costs no more than a short one's.
pub const CONTEXT_MAX_REQUESTS: usize = 10;

/// The character cap on one recorded action line. One pathological call — a
/// heredoc `bash` command, a huge MCP argument object — must not spend the
/// whole context budget on itself (`docs/long-lines.md`'s posture, in
/// prompt form); the cut is closed with `…` so a clipped line can't read as
/// one that simply ended.
pub const CONTEXT_ACTION_MAX_CHARS: usize = 160;

/// The character cap on the user-request excerpt. The head of the request is
/// what names the task; a pasted essay's tail is not worth its tokens on
/// every verdict.
pub const CONTEXT_REQUEST_MAX_CHARS: usize = 500;

/// Cut `text` to at most `max` chars, closing a real cut with `…`. Char-based
/// (this is a model-facing prompt, not a terminal — display columns don't
/// apply), and always on a char boundary by construction.
fn truncate_chars(text: &str, max: usize) -> String {
    let mut out = String::with_capacity(text.len().min(max * 4));
    let mut chars = text.chars();
    for _ in 0..max {
        match chars.next() {
            Some(c) => out.push(c),
            None => return out,
        }
    }
    if chars.next().is_some() {
        out.push('…');
    }
    out
}

/// One recorded action, in the transcript cell's own header shape —
/// `Bash(cargo test)`, `Read(/path)`, `deepwiki - ask_question (MCP)({…})` —
/// via the same [`display_name`]/[`summarize_call`] pair the cells use, so
/// the classifier and the user read the turn in the same vocabulary. Capped
/// at [`CONTEXT_ACTION_MAX_CHARS`].
fn action_line(name: &str, arguments: &str) -> String {
    let display = display_name(name);
    let summary = summarize_call(name, arguments);
    let line = if summary.is_empty() {
        display
    } else {
        format!("{display}({summary})")
    };
    truncate_chars(&line, CONTEXT_ACTION_MAX_CHARS)
}

/// The **task context** the auto mode classifier reads beside each request
/// (`docs/permissions.md`): the recent user requests and a bounded,
/// truncated log of the actions the agent has taken — so a verdict can weigh
/// whether the action under review *fits the task*, not just whether it
/// looks safe in a vacuum.
///
/// Both halves are **rolling windows over the conversation**, not one turn.
/// A turn boundary is the wrong reset point for either: the request that
/// explains a command is often two turns back ("set up the project" → …→ a
/// `rm -rf` on the build output), and an agent that had a command denied and
/// re-tries a variant of it one turn later should still be seen doing so.
/// So each new user message [`push_request`](Self::push_request)s onto the
/// window rather than clearing it, and the caps alone bound the block: the
/// newest [`CONTEXT_MAX_REQUESTS`] requests and [`CONTEXT_MAX_ACTIONS`]
/// actions, each line truncated ([`CONTEXT_REQUEST_MAX_CHARS`],
/// [`CONTEXT_ACTION_MAX_CHARS`]), with counted `(+N … omitted)` markers where
/// the windows cut. One verdict therefore costs the same on turn fifty as on
/// turn one — a ceiling of roughly 8 KB of prompt, most of it usually unused.
///
/// The backend owns one per session and feeds it at the boundary: the user
/// message each `spawn` carries, executed calls, denied calls (marked), and
/// subagent launches. A subagent keeps its own, seeded from its launch
/// prompt. Everything here is pure and unit-tested.
#[derive(Debug, Clone, Default)]
pub struct ClassifierContext {
    /// The newest [`CONTEXT_MAX_REQUESTS`] user requests, oldest first, each
    /// truncated to [`CONTEXT_REQUEST_MAX_CHARS`].
    requests: VecDeque<String>,
    /// How many older requests the window pushed out.
    dropped_requests: usize,
    /// The newest [`CONTEXT_MAX_ACTIONS`] action lines, oldest first.
    actions: VecDeque<String>,
    /// How many older actions the window pushed out.
    dropped_actions: usize,
}

impl ClassifierContext {
    /// An empty context — no requests, no actions.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a user request onto the rolling window (the boundary calls this
    /// once per `spawn`). Blank text is ignored: a synthetic follow-up turn —
    /// the one a finished background shell auto-starts — carries no request
    /// of the user's, and an empty `> ` quote would only read as one.
    pub fn push_request(&mut self, request: &str) {
        let request = request.trim();
        if request.is_empty() {
            return;
        }
        if self.requests.len() >= CONTEXT_MAX_REQUESTS {
            self.requests.pop_front();
            self.dropped_requests += 1;
        }
        self.requests
            .push_back(truncate_chars(request, CONTEXT_REQUEST_MAX_CHARS));
    }

    /// Record a tool call the agent ran (or is running).
    pub fn record_call(&mut self, name: &str, arguments: &str) {
        self.push_action(action_line(name, arguments));
    }

    /// Record a call that was refused — by the user, a hook, or the
    /// classifier itself. The marker is appended *after* the line's own
    /// truncation, so it can never be eaten by the cut.
    pub fn record_denied(&mut self, name: &str, arguments: &str) {
        self.push_action(format!(
            "{} — denied, not run",
            action_line(name, arguments)
        ));
    }

    fn push_action(&mut self, line: String) {
        if self.actions.len() >= CONTEXT_MAX_ACTIONS {
            self.actions.pop_front();
            self.dropped_actions += 1;
        }
        self.actions.push_back(line);
    }

    /// Render the `## Task context` block the classifier prompt opens with.
    /// Each request is quoted line by line (`> `) so one carrying markdown of
    /// its own — headers included — stays visibly quoted material rather than
    /// becoming structure, and both lists say `(none yet)` when empty rather
    /// than leaving the classifier to wonder what was omitted. The ordering
    /// is stated in the headers, since which request is the *current* task is
    /// the one thing the model must not have to guess.
    #[must_use]
    pub fn render(&self) -> String {
        let mut out = String::from(
            "## Task context\nUser requests (oldest first; the last is the current task):",
        );
        if self.requests.is_empty() {
            out.push_str(" (none yet)\n");
        } else {
            out.push('\n');
            if self.dropped_requests > 0 {
                out.push_str(&format!(
                    "(+{} earlier requests omitted)\n",
                    self.dropped_requests
                ));
            }
            for request in &self.requests {
                for line in request.lines() {
                    out.push_str("> ");
                    out.push_str(line);
                    out.push('\n');
                }
            }
        }
        out.push_str("\nRecent actions (oldest first):");
        if self.actions.is_empty() {
            out.push_str(" (none yet)");
        } else {
            out.push('\n');
            if self.dropped_actions > 0 {
                out.push_str(&format!(
                    "(+{} earlier actions omitted)\n",
                    self.dropped_actions
                ));
            }
            for action in &self.actions {
                out.push_str("- ");
                out.push_str(action);
                out.push('\n');
            }
        }
        out.trim_end().to_string()
    }
}

/// The newest user-authored text in `messages` — the prompt that opened a
/// subagent run (or the chat message that continued it), for seeding its
/// [`ClassifierContext`]. A multimodal parts message yields its text part;
/// whitespace-only entries are skipped. Called **before** the run pushes its
/// skill reminder / hook notes (those are user-role too, and would win the
/// scan). Pure — unit-tested.
#[must_use]
pub fn latest_user_text(messages: &[super::ChatMessage]) -> String {
    messages
        .iter()
        .rev()
        .filter(|message| message.role == "user")
        .find_map(|message| match &message.content {
            super::MessageContent::Text(text) if !text.trim().is_empty() => Some(text.clone()),
            super::MessageContent::Parts(parts) => parts.iter().find_map(|part| match part {
                super::ContentPart::Text { text } if !text.trim().is_empty() => Some(text.clone()),
                _ => None,
            }),
            _ => None,
        })
        .unwrap_or_default()
}

/// The classifier's whole user message: the rendered task context (when
/// there is one), then the one action being judged under its own
/// `## Action to review` header — the highlight that keeps what is being
/// *decided* from blurring into what already happened. An empty `context`
/// (an embedder that built no log) degrades to the bare request shape, no
/// headers. Pure — unit-tested.
#[must_use]
pub fn classifier_prompt(request: &PermissionRequest, cwd: &str, context: &str) -> String {
    let action = classifier_request_prompt(request, cwd);
    if context.is_empty() {
        return action;
    }
    format!("{context}\n\n## Action to review\n{action}")
}

/// Parse the classifier's reply against the output contract:
/// `<block>no</block>` allows, `<block>yes</block><reason>…</reason>` denies
/// with the reason. Tolerates leading noise (a chatty model), whitespace
/// inside the tags, and any casing of the verdict word; anything without a
/// recognisable `<block>` verdict is an `Err` — the caller falls back to the
/// ordinary prompt, never to allowing. Pure — unit-tested.
///
/// # Errors
/// The reply carried no parseable `<block>yes|no</block>` verdict.
pub fn parse_verdict(text: &str) -> Result<ClassifierVerdict, String> {
    let tag = |open: &str, close: &str| -> Option<String> {
        let lower = text.to_ascii_lowercase();
        let start = lower.find(open)? + open.len();
        let end = lower[start..].find(close)? + start;
        Some(text[start..end].trim().to_string())
    };
    let verdict = tag("<block>", "</block>").ok_or_else(|| {
        format!(
            "no <block> verdict in the classifier reply: {:?}",
            text.chars().take(200).collect::<String>()
        )
    })?;
    match verdict.to_ascii_lowercase().as_str() {
        "no" => Ok(ClassifierVerdict {
            allow: true,
            reason: String::new(),
        }),
        "yes" => Ok(ClassifierVerdict {
            allow: false,
            reason: tag("<reason>", "</reason>").unwrap_or_default(),
        }),
        other => Err(format!("unrecognised <block> verdict: {other:?}")),
    }
}

/// The auto mode classifier bound to one provider: a tools-free client on
/// the session's model (or the `ALTER_ZERO_CLASSIFIER_MODEL` override — a
/// cheap fast model on the same provider, Claude Code's small-model slot),
/// plus the working directory baked into every user prompt.
#[derive(Debug, Clone)]
pub struct SafetyClassifier {
    client: OpenAiClient,
    cwd: String,
}

impl SafetyClassifier {
    /// Build the classifier for the backend's resolved config. The clone
    /// drops the session's thinking mode (a verdict needs no visible
    /// reasoning budget — the reference disables thinking for its
    /// classifier) and never carries tools; `ALTER_ZERO_CLASSIFIER_MODEL`
    /// swaps the model id on the same provider. Boundary code (env + cwd
    /// reads), like the rest of the backend constructors.
    #[must_use]
    pub fn new(cfg: &ModelConfig) -> Self {
        let mut cfg = cfg.clone();
        if let Some(model) = std::env::var("ALTER_ZERO_CLASSIFIER_MODEL")
            .ok()
            .map(|m| m.trim().to_string())
            .filter(|m| !m.is_empty())
        {
            cfg.model = model;
        }
        cfg.thinking = None;
        let cwd = std::env::current_dir()
            .map(|d| d.display().to_string())
            .unwrap_or_else(|_| "(unknown)".to_string());
        Self {
            client: OpenAiClient::new(cfg),
            cwd,
        }
    }

    /// Classify one request — a `bash` command or an MCP tool call: one
    /// silent streaming completion (no events reach the UI — the asked-about
    /// cell keeps its `⎿ Waiting…` row), the reply parsed against the output
    /// contract. `context` is the turn's rendered [`ClassifierContext`]
    /// block (empty for none — the bare request shape); the caller renders
    /// it *before* the call so no lock is held across the network. Blocking;
    /// polls `cancel` like every request, so an Esc reaps it promptly.
    ///
    /// # Errors
    /// Transport/API failures and an unparseable reply — the caller
    /// ([`crate::llm::approval::approve_call`]) falls back to the ordinary
    /// user prompt.
    pub fn classify(
        &self,
        request: &PermissionRequest,
        context: &str,
        cancel: &CancelToken,
    ) -> Result<ClassifierVerdict, String> {
        let messages = vec![
            super::ChatMessage::system(CLASSIFIER_SYSTEM_PROMPT.trim()),
            super::ChatMessage::user(classifier_prompt(request, &self.cwd, context)),
        ];
        let outcome = self
            .client
            .stream_chat(messages, cancel, |_delta| {})
            .map_err(|e| e.to_string())?;
        parse_verdict(&outcome.text.response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permission::PermissionKind;

    fn mcp_request(target: &str, body: &str, detail: Option<&str>) -> PermissionRequest {
        PermissionRequest {
            id: String::new(),
            kind: PermissionKind::Mcp,
            target: target.to_string(),
            body: body.to_string(),
            detail: detail.map(str::to_string),
            agent: None,
        }
    }

    #[test]
    fn an_mcp_request_prompts_with_the_tool_its_args_and_the_server_description() {
        // Auto mode classifies MCP calls too (`docs/mcp.md`): the classifier
        // reads the tool named the way the user knows it, the server's own
        // description of it, and the arguments — fenced like a command, so a
        // multi-line value stays one obvious block.
        let request = mcp_request(
            "mcp__deepwiki__ask_question",
            r#"repoName: "a/b", question: "What?""#,
            Some("Ask about a repo."),
        );
        let prompt = classifier_request_prompt(&request, "/home/user/proj");
        assert!(
            prompt.contains("Working directory: /home/user/proj"),
            "got {prompt}"
        );
        assert!(prompt.contains("deepwiki - ask_question"), "got {prompt}");
        assert!(prompt.contains("Ask about a repo."), "got {prompt}");
        assert!(
            prompt.contains("```\nrepoName: \"a/b\", question: \"What?\"\n```"),
            "got {prompt}"
        );
        // No description line when the server gave none; an argument-less
        // call says so explicitly, so nothing reads as hidden.
        let bare = classifier_request_prompt(&mcp_request("mcp__s__t", "", None), "/p");
        assert!(!bare.contains("description"), "got {bare}");
        assert!(bare.contains("(no arguments)"), "got {bare}");
    }

    #[test]
    fn a_bash_request_keeps_the_command_prompt_shape() {
        let request = PermissionRequest {
            id: String::new(),
            kind: PermissionKind::Bash,
            target: "ls -la".to_string(),
            body: String::new(),
            detail: Some("List files".to_string()),
            agent: None,
        };
        assert_eq!(
            classifier_request_prompt(&request, "/p"),
            classifier_user_prompt("ls -la", Some("List files"), "/p")
        );
    }

    #[test]
    fn the_system_prompt_briefs_the_classifier_on_mcp_tool_calls() {
        // The system prompt must set up both judgements — a classifier told
        // only about shell commands has no rubric for a server tool.
        assert!(
            CLASSIFIER_SYSTEM_PROMPT.contains("MCP"),
            "prompts/classifier.md never mentions MCP tool calls"
        );
    }

    #[test]
    fn the_user_prompt_carries_command_description_and_cwd() {
        let prompt = classifier_user_prompt("ls -la", Some("List files"), "/home/user/proj");
        assert!(prompt.contains("Working directory: /home/user/proj"));
        assert!(prompt.contains("Agent's description: List files"));
        assert!(prompt.contains("```\nls -la\n```"));
        // No description line when the model gave none (or only whitespace).
        let bare = classifier_user_prompt("ls", None, "/p");
        assert!(!bare.contains("description"), "got {bare}");
        assert!(!classifier_user_prompt("ls", Some("  "), "/p").contains("description"));
    }

    // ===== the task context (docs/permissions.md "Auto mode: the classifier") =====

    /// A context carrying one user request — the common test shape.
    fn ctx(request: &str) -> ClassifierContext {
        let mut context = ClassifierContext::new();
        context.push_request(request);
        context
    }

    #[test]
    fn the_context_records_actions_and_renders_the_task_block() {
        // The classifier judges an action inside the task it serves: the block
        // carries the user's request and the actions already taken this turn,
        // each in the transcript cell's own `Name(args)` shape.
        let mut context = ctx("Improve the project");
        context.record_call("read", r#"{"path":"/home/user/proj/src/main.rs"}"#);
        context.record_call(
            "edit",
            r#"{"path":"/home/user/proj/src/main.rs","old_string":"a","new_string":"b"}"#,
        );
        context.record_call(
            "bash",
            r#"{"command":"cargo test","description":"Run tests"}"#,
        );
        context.record_call(
            "mcp__deepwiki__ask_question",
            r#"{"repoName":"a/b","question":"What?"}"#,
        );
        let block = context.render();
        assert!(block.contains("## Task context"), "got {block}");
        assert!(block.contains("> Improve the project"), "got {block}");
        assert!(
            block.contains("Read(/home/user/proj/src/main.rs)"),
            "got {block}"
        );
        assert!(
            block.contains("Edit(/home/user/proj/src/main.rs)"),
            "got {block}"
        );
        assert!(block.contains("Bash(cargo test)"), "got {block}");
        // An MCP action keeps the user-facing `{server} - {tool} (MCP)` name
        // and its compact-JSON arguments, so the classifier sees where the
        // remote call's risk lives.
        assert!(
            block.contains("deepwiki - ask_question (MCP)"),
            "got {block}"
        );
        assert!(block.contains(r#""repoName":"a/b""#), "got {block}");
    }

    #[test]
    fn the_context_truncates_a_long_user_request() {
        // Truncation is the token budget: a pasted essay of a prompt reaches
        // the classifier as its head, cut at the cap and closed with an
        // ellipsis so nothing reads as complete when it isn't.
        let long = "x".repeat(CONTEXT_REQUEST_MAX_CHARS + 400);
        let context = ctx(&long);
        let block = context.render();
        let quoted_len = block
            .lines()
            .find(|l| l.starts_with("> "))
            .expect("the request is quoted")
            .chars()
            .count();
        assert!(
            quoted_len <= CONTEXT_REQUEST_MAX_CHARS + 3,
            "the request line stays within the cap: {quoted_len}"
        );
        assert!(block.contains('…'), "the cut is visible: {block}");
    }

    #[test]
    fn the_context_quotes_every_line_of_a_multi_line_request() {
        // `> ` per line keeps a request that contains markdown of its own —
        // headers included — visibly quoted material rather than structure.
        let context = ctx("do this\n## and that");
        let block = context.render();
        assert!(block.contains("> do this"), "got {block}");
        assert!(block.contains("> ## and that"), "got {block}");
        assert!(!block.contains("\n## and that"), "got {block}");
    }

    #[test]
    fn the_context_truncates_a_long_action_line() {
        let mut context = ctx("task");
        let long = format!("echo {}", "y".repeat(CONTEXT_ACTION_MAX_CHARS * 2));
        context.record_call("bash", &serde_json::json!({"command": long}).to_string());
        let block = context.render();
        let line = block
            .lines()
            .find(|l| l.contains("Bash("))
            .expect("the action is listed");
        assert!(
            line.chars().count() <= CONTEXT_ACTION_MAX_CHARS + 8,
            "one action can't spend the whole budget: {} chars",
            line.chars().count()
        );
        assert!(line.contains('…'), "the cut is visible: {line}");
    }

    #[test]
    fn the_context_caps_the_action_count_keeping_the_most_recent() {
        // A long agentic turn drops its oldest actions, not its newest — the
        // recent ones are the context the next verdict needs — and says how
        // many it dropped so the list never reads as the whole story.
        let mut context = ctx("task");
        for n in 0..(CONTEXT_MAX_ACTIONS + 5) {
            context.record_call(
                "bash",
                &serde_json::json!({"command": format!("step-{n}")}).to_string(),
            );
        }
        let block = context.render();
        assert!(!block.contains("Bash(step-0)"), "oldest dropped: {block}");
        assert!(!block.contains("Bash(step-4)"), "oldest dropped: {block}");
        assert!(
            block.contains("Bash(step-5)"),
            "the cap keeps the rest: {block}"
        );
        assert!(
            block.contains(&format!("Bash(step-{})", CONTEXT_MAX_ACTIONS + 4)),
            "newest kept: {block}"
        );
        assert!(
            block.contains("+5 earlier actions omitted"),
            "the drop is visible: {block}"
        );
    }

    #[test]
    fn a_denied_action_is_marked_and_the_marker_survives_truncation() {
        // A call the classifier (or the user) refused is context for the next
        // verdict — an agent re-trying a variant of a denied command should be
        // seen doing so. The marker lands after the cut so it can't be eaten.
        let mut context = ctx("task");
        let long = format!("sudo {}", "z".repeat(CONTEXT_ACTION_MAX_CHARS * 2));
        context.record_denied("bash", &serde_json::json!({"command": long}).to_string());
        let block = context.render();
        let line = block
            .lines()
            .find(|l| l.contains("Bash("))
            .expect("the denied action is listed");
        assert!(line.ends_with("— denied, not run"), "got {line}");
        assert!(line.contains('…'), "still truncated: {line}");
    }

    #[test]
    fn an_actionless_context_says_so() {
        let context = ctx("Improve the project");
        let block = context.render();
        assert!(block.contains("(none yet)"), "got {block}");
    }

    #[test]
    fn user_requests_accumulate_across_turns_newest_last() {
        // The window spans the conversation, not one turn: the request that
        // explains a command is often two turns back, so a new one is pushed
        // onto the log rather than replacing it.
        let mut context = ClassifierContext::new();
        context.push_request("set up the project");
        context.record_call("bash", r#"{"command":"cargo build"}"#);
        context.push_request("now clean up the build output");
        let block = context.render();
        let first = block
            .find("> set up the project")
            .expect("the older request survives");
        let second = block
            .find("> now clean up the build output")
            .expect("and the newest is there");
        assert!(first < second, "oldest first, current task last: {block}");
        // …and the action recorded under the earlier request is still there:
        // an agent's history does not reset with the turn either.
        assert!(block.contains("- Bash(cargo build)"), "got {block}");
    }

    #[test]
    fn the_request_window_drops_the_oldest_behind_a_counted_marker() {
        let mut context = ClassifierContext::new();
        for n in 0..(CONTEXT_MAX_REQUESTS + 3) {
            context.push_request(&format!("request-{n}"));
        }
        let block = context.render();
        assert!(!block.contains("> request-0"), "oldest dropped: {block}");
        assert!(!block.contains("> request-2"), "oldest dropped: {block}");
        assert!(
            block.contains("> request-3"),
            "the window keeps the rest: {block}"
        );
        assert!(
            block.contains(&format!("> request-{}", CONTEXT_MAX_REQUESTS + 2)),
            "newest kept: {block}"
        );
        assert!(
            block.contains("+3 earlier requests omitted"),
            "the drop is visible: {block}"
        );
    }

    #[test]
    fn a_blank_request_is_not_recorded() {
        // A loop-initiated follow-up turn (a background shell finishing)
        // carries no request of the user's; an empty `> ` quote would read as
        // one.
        let mut context = ClassifierContext::new();
        context.push_request("   \n  ");
        let block = context.render();
        assert!(block.contains("(none yet)"), "got {block}");
    }

    #[test]
    fn an_empty_context_says_so_on_both_halves() {
        let block = ClassifierContext::new().render();
        assert_eq!(block.matches("(none yet)").count(), 2, "got {block}");
    }

    #[test]
    fn the_full_prompt_highlights_the_action_under_review() {
        // Context first, then the one action being judged under its own
        // header — so what is being decided can never blur into what already
        // happened.
        let request = PermissionRequest {
            id: String::new(),
            kind: PermissionKind::Bash,
            target: "ls -la".to_string(),
            body: String::new(),
            detail: Some("List files".to_string()),
            agent: None,
        };
        let mut context = ctx("Improve the project");
        context.record_call("read", r#"{"path":"/p/a.rs"}"#);
        let prompt = classifier_prompt(&request, "/p", &context.render());
        let task_at = prompt.find("## Task context").expect("task block");
        let review_at = prompt.find("## Action to review").expect("review header");
        assert!(task_at < review_at, "context precedes the action: {prompt}");
        assert!(
            prompt.contains("```\nls -la\n```"),
            "the command block survives: {prompt}"
        );
        // Without context the prompt is exactly the bare request shape.
        assert_eq!(
            classifier_prompt(&request, "/p", ""),
            classifier_request_prompt(&request, "/p")
        );
    }

    #[test]
    fn the_seed_is_the_newest_nonempty_user_text() {
        // A subagent's context seeds from the launch prompt (its "user
        // request") — the newest user text when the run starts, before the
        // skill reminder / hook notes are pushed. Tool results and blank
        // messages don't count; a parts message yields its text part.
        use crate::llm::{ChatMessage, ContentPart};
        let messages = vec![
            ChatMessage::system("persona"),
            ChatMessage::user("explore the repo"),
            ChatMessage::new("assistant", "ok"),
            ChatMessage::tool_result("c1", "file contents"),
            ChatMessage::user("   "),
        ];
        assert_eq!(latest_user_text(&messages), "explore the repo");
        let with_parts = vec![ChatMessage::with_parts(
            "user",
            vec![
                ContentPart::text("look at this"),
                ContentPart::image("data:x"),
            ],
        )];
        assert_eq!(latest_user_text(&with_parts), "look at this");
        assert_eq!(latest_user_text(&[]), "");
    }

    #[test]
    fn the_system_prompt_briefs_the_task_context_as_data_not_instructions() {
        // The context block quotes the user and the transcript — the exact
        // channel a poisoned repo would lobby through — so the system prompt
        // must pin it as information only, never authorization.
        let prompt = CLASSIFIER_SYSTEM_PROMPT.to_ascii_lowercase();
        assert!(
            prompt.contains("task context"),
            "prompts/classifier.md never introduces the task context block"
        );
        assert!(
            prompt.contains("never instructions"),
            "prompts/classifier.md must pin the context as data, not instructions"
        );
    }

    #[test]
    fn a_block_no_reply_allows() {
        let verdict = parse_verdict("<block>no</block>").expect("parses");
        assert!(verdict.allow);
        assert_eq!(verdict.reason, "");
    }

    #[test]
    fn a_block_yes_reply_denies_with_the_reason() {
        let verdict = parse_verdict("<block>yes</block><reason>privilege escalation</reason>")
            .expect("parses");
        assert!(!verdict.allow);
        assert_eq!(verdict.reason, "privilege escalation");
        // A yes with no reason still denies.
        let bare = parse_verdict("<block>yes</block>").expect("parses");
        assert!(!bare.allow);
        assert_eq!(bare.reason, "");
    }

    #[test]
    fn the_parse_tolerates_noise_case_and_whitespace() {
        // A chatty model that ignored "begin with <block>" is still read —
        // the verdict is unambiguous wherever it sits.
        let verdict = parse_verdict("Looking at this command…\n<BLOCK> Yes </BLOCK>\n<reason>\ndeletes the home directory\n</reason>")
            .expect("parses");
        assert!(!verdict.allow);
        assert_eq!(verdict.reason, "deletes the home directory");
        assert!(parse_verdict("`<block>No</block>` — safe.").unwrap().allow);
    }

    #[test]
    fn an_unparseable_reply_is_an_error_never_an_allow() {
        for reply in [
            "",
            "sure, go ahead",
            "<block>maybe</block>",
            "<reason>x</reason>",
        ] {
            assert!(
                parse_verdict(reply).is_err(),
                "{reply:?} must not produce a verdict"
            );
        }
    }
}
