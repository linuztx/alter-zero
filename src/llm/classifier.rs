//! The **auto mode classifier** (`docs/permissions.md`): the silent LLM
//! safety check a `bash` command — or an MCP tool call (`docs/mcp.md`) —
//! goes through in [`PermissionMode::Auto`] instead of the user prompt —
//! Claude Code's auto-mode classifier, on the session's own provider.
//!
//! The request/response shape follows the reference: a fixed system prompt
//! ([`prompts/classifier.md`](../../prompts/classifier.md)) with a strict
//! `<block>yes|no</block><reason>…</reason>` output contract (more robust
//! than JSON across arbitrary OpenAI-compatible models), one user message
//! carrying the request — a command + the model's stated description + the
//! cwd, or an MCP tool + its arguments + the server's description
//! ([`classifier_request_prompt`]) — and nothing else — never the
//! conversation, so a poisoned transcript can't lobby the classifier. The
//! verdict parse and the prompt build are pure and unit-tested;
//! [`SafetyClassifier::classify`] is the one HTTP boundary, riding the same
//! blocking client and cancel-polling stream machinery as every other
//! request ([`OpenAiClient::stream_chat`]).
//!
//! [`PermissionMode::Auto`]: crate::permission::PermissionMode::Auto

use super::config::ModelConfig;
use super::openai::OpenAiClient;
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
    /// contract. Blocking; polls `cancel` like every request, so an Esc
    /// reaps it promptly.
    ///
    /// # Errors
    /// Transport/API failures and an unparseable reply — the caller
    /// ([`crate::llm::approval::approve_call`]) falls back to the ordinary
    /// user prompt.
    pub fn classify(
        &self,
        request: &PermissionRequest,
        cancel: &CancelToken,
    ) -> Result<ClassifierVerdict, String> {
        let messages = vec![
            super::ChatMessage::system(CLASSIFIER_SYSTEM_PROMPT.trim()),
            super::ChatMessage::user(classifier_request_prompt(request, &self.cwd)),
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
