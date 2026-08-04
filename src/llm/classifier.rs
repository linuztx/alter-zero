//! The **auto mode classifier** (`docs/permissions.md`): the silent LLM
//! safety check a `bash` command goes through in [`PermissionMode::Auto`]
//! instead of the user prompt — Claude Code's auto-mode classifier, on the
//! session's own provider.
//!
//! The request/response shape follows the reference: a fixed system prompt
//! ([`prompts/classifier.md`](../../prompts/classifier.md)) with a strict
//! `<block>yes|no</block><reason>…</reason>` output contract (more robust
//! than JSON across arbitrary OpenAI-compatible models), one user message
//! carrying the command + the model's stated description + the cwd, and
//! nothing else — never the conversation, so a poisoned transcript can't
//! lobby the classifier. The verdict parse and the prompt build are pure and
//! unit-tested; [`SafetyClassifier::classify`] is the one HTTP boundary,
//! riding the same blocking client and cancel-polling stream machinery as
//! every other request ([`OpenAiClient::stream_chat`]).
//!
//! [`PermissionMode::Auto`]: crate::permission::PermissionMode::Auto

use super::config::ModelConfig;
use super::openai::OpenAiClient;
use crate::permission::{ClassifierVerdict, PermissionRequest};
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

    /// Classify one command request: one silent streaming completion (no
    /// events reach the UI — the asked-about cell keeps its `⎿ Waiting…`
    /// row), the reply parsed against the output contract. Blocking; polls
    /// `cancel` like every request, so an Esc reaps it promptly.
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
            super::ChatMessage::user(classifier_user_prompt(
                &request.target,
                request.detail.as_deref(),
                &self.cwd,
            )),
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
