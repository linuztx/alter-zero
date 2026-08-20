//! The `AskUserQuestion` tool's pure vocabulary and the gate its call blocks
//! on. See `docs/ask.md`.
//!
//! Claude Code's mid-turn questions: the model asks 1–4 multiple-choice
//! questions, the TUI raises an inline modal (the permission prompt's
//! sibling), and the tool thread **parks on [`AskGate`]** until the user
//! submits answers, declines, or asks to chat. Everything here is pure data —
//! the question shapes parsed from the tool-call JSON, the decision the user
//! produces, and the two texts every resolution needs (the cell display and
//! the model-facing result) — plus the one piece of cross-thread coordination,
//! [`AskGate`], the `Arc<Mutex<…>> + Condvar` sibling of
//! [`crate::permission::PermissionGate`].

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use serde::Deserialize;

/// How often [`AskGate::wait`] re-checks its cancel predicate while blocked,
/// so an Esc/quit reaps the waiting tool thread promptly.
const WAIT_POLL: Duration = Duration::from_millis(50);

/// The schema's bounds: how many questions one call may ask, and how many
/// options each may offer. Mirrored in [`parse_questions`]'s validation and
/// the tool spec (`crate::llm::tools::ask_spec`).
pub const MAX_QUESTIONS: usize = 4;
pub const MIN_OPTIONS: usize = 2;
pub const MAX_OPTIONS: usize = 4;

/// One choice of a question: the short `label` the user picks, the dim
/// `description` under it, and an optional `preview` — content rendered in the
/// side-by-side panel while the option is focused (a code snippet, a mockup).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AskOption {
    pub label: String,
    pub description: String,
    #[serde(default)]
    pub preview: Option<String>,
}

/// One question of an [`AskRequest`]: the full `question` text, the short
/// `header` chip naming its tab, its 2–4 [`AskOption`]s, and whether several
/// may be picked (`multiSelect` on the wire).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AskQuestion {
    pub question: String,
    pub header: String,
    pub options: Vec<AskOption>,
    #[serde(default, rename = "multiSelect")]
    pub multi_select: bool,
}

impl AskQuestion {
    /// Whether any option carries a preview — the question then renders the
    /// side-by-side panel and offers the `n` notes field (`docs/ask.md`).
    #[must_use]
    pub fn has_previews(&self) -> bool {
        self.options.iter().any(|o| o.preview.is_some())
    }
}

/// The wire shape of the tool call's `arguments`: `{"questions": […]}`.
#[derive(Debug, Clone, Deserialize)]
struct AskArgs {
    questions: Vec<AskQuestion>,
}

/// Parse and validate a tool call's raw JSON `arguments` into the questions to
/// ask, enforcing the schema's bounds (1–[`MAX_QUESTIONS`] questions of
/// [`MIN_OPTIONS`]–[`MAX_OPTIONS`] options each, none of the texts empty).
///
/// # Errors
/// A short model-facing message (the model retries on it) when the JSON does
/// not parse or a bound is violated.
pub fn parse_questions(arguments: &str) -> Result<Vec<AskQuestion>, String> {
    let trimmed = arguments.trim();
    let text = if trimmed.is_empty() { "{}" } else { trimmed };
    let args: AskArgs =
        serde_json::from_str(text).map_err(|e| format!("invalid tool arguments: {e}"))?;
    let questions = args.questions;
    if questions.is_empty() || questions.len() > MAX_QUESTIONS {
        return Err(format!(
            "questions must contain 1 to {MAX_QUESTIONS} entries, got {}",
            questions.len()
        ));
    }
    for (i, q) in questions.iter().enumerate() {
        if q.question.trim().is_empty() {
            return Err(format!("questions[{i}].question must not be empty"));
        }
        if q.options.len() < MIN_OPTIONS || q.options.len() > MAX_OPTIONS {
            return Err(format!(
                "questions[{i}].options must contain {MIN_OPTIONS} to {MAX_OPTIONS} entries, got {}",
                q.options.len()
            ));
        }
        if let Some(j) = q.options.iter().position(|o| o.label.trim().is_empty()) {
            return Err(format!(
                "questions[{i}].options[{j}].label must not be empty"
            ));
        }
    }
    Ok(questions)
}

/// One pending `AskUserQuestion` call: the gate id it resolves under and the
/// questions the modal shows. Built by the tool thread
/// (`crate::llm::ask::ask_user`) and carried to the loop on
/// [`crate::stream::StreamEvent::AskUser`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AskRequest {
    /// The gate id this request is resolved by ([`AskGate::next_id`]).
    pub id: String,
    pub questions: Vec<AskQuestion>,
}

/// One answered question: the question text (the `answers` object's key), the
/// chosen labels (a custom "Other" text is just another label), and the
/// optional annotations — free-text `notes` and the selected option's
/// `preview` — a preview question collects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AskAnswer {
    pub question: String,
    pub labels: Vec<String>,
    pub notes: Option<String>,
    pub preview: Option<String>,
}

/// What the user decided, handed back to the blocked tool thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AskDecision {
    /// The answered questions, in question order (an unanswered question of a
    /// partial submission simply isn't in the list).
    Submitted(Vec<AskAnswer>),
    /// Esc (or the Submit page's `Cancel`): no answers; the model is told to
    /// stop and wait.
    Declined,
    /// The `Chat about this` row: the user wants to discuss the questions
    /// before deciding; the model is told to stop and wait for their message.
    Chat,
}

/// The committed cell's headline for a submission — the first output line,
/// which the renderer promotes to the `●` header (`docs/ask.md`). It names
/// **this** agent ([`crate::APP_NAME`], pinned by the test below): the
/// reference tool's wording was ported with its own product name in it, so
/// the cell used to credit a different agent for the questions the user had
/// just answered.
pub const ANSWERED_HEADLINE: &str = "User answered Alter Zero's questions:";

/// The headline for a decline.
pub const DECLINED_HEADLINE: &str = "User declined to answer questions";

/// The headline for a `Chat about this` resolution.
pub const CHAT_HEADLINE: &str = "User wants to chat about this";

/// One `· {question} → {labels}` line of the answered cell.
fn answer_line(answer: &AskAnswer) -> String {
    format!("· {} → {}", answer.question, answer.labels.join(", "))
}

/// One `· {question} ({label} / {label} / …)` line of a declined/chat cell —
/// the reference transcript's shape, naming the options that went unanswered.
fn question_line(question: &AskQuestion) -> String {
    let labels: Vec<&str> = question.options.iter().map(|o| o.label.as_str()).collect();
    format!("· {} ({})", question.question, labels.join(" / "))
}

/// The cell display for a submission: the headline over one `· Q → A` row per
/// answered question.
#[must_use]
pub fn answered_display(answers: &[AskAnswer]) -> String {
    let mut out = ANSWERED_HEADLINE.to_string();
    for answer in answers {
        out.push('\n');
        out.push_str(&answer_line(answer));
    }
    out
}

/// The model-facing result for a submission — the tool schema's shape:
/// `{"answers": {question: labels}, "annotations": {question: {notes, preview}}}`,
/// the `annotations` key present only when some answer carries any.
#[must_use]
pub fn answered_result(answers: &[AskAnswer]) -> String {
    let mut answer_map = serde_json::Map::new();
    let mut annotations = serde_json::Map::new();
    for answer in answers {
        answer_map.insert(
            answer.question.clone(),
            serde_json::Value::String(answer.labels.join(", ")),
        );
        let mut entry = serde_json::Map::new();
        if let Some(notes) = answer.notes.as_ref().filter(|n| !n.trim().is_empty()) {
            entry.insert(
                "notes".to_string(),
                serde_json::Value::String(notes.clone()),
            );
        }
        if let Some(preview) = &answer.preview {
            entry.insert(
                "preview".to_string(),
                serde_json::Value::String(preview.clone()),
            );
        }
        if !entry.is_empty() {
            annotations.insert(answer.question.clone(), serde_json::Value::Object(entry));
        }
    }
    let mut out = serde_json::Map::new();
    out.insert("answers".to_string(), serde_json::Value::Object(answer_map));
    if !annotations.is_empty() {
        out.insert(
            "annotations".to_string(),
            serde_json::Value::Object(annotations),
        );
    }
    serde_json::Value::Object(out).to_string()
}

/// The cell display for a decline: the headline over one
/// `· {question} ({options})` row per question that went unanswered.
#[must_use]
pub fn declined_display(questions: &[AskQuestion]) -> String {
    let mut out = DECLINED_HEADLINE.to_string();
    for question in questions {
        out.push('\n');
        out.push_str(&question_line(question));
    }
    out
}

/// The model-facing result for a decline — the permission rejection's
/// stop-and-wait posture.
#[must_use]
pub fn declined_result() -> String {
    "The user declined to answer the question(s). STOP what you are doing and wait for the \
     user to tell you how to proceed. Do not repeat the questions — they chose not to answer \
     them."
        .to_string()
}

/// The cell display for a `Chat about this` resolution — the chat headline
/// over the same question rows as a decline.
#[must_use]
pub fn chat_display(questions: &[AskQuestion]) -> String {
    let mut out = CHAT_HEADLINE.to_string();
    for question in questions {
        out.push('\n');
        out.push_str(&question_line(question));
    }
    out
}

/// The model-facing result for `Chat about this`: the user wants to talk it
/// through before deciding.
#[must_use]
pub fn chat_result() -> String {
    "The user selected \"Chat about this\" instead of answering: they want to discuss the \
     question(s) with you before deciding. STOP and wait for their next message, then \
     continue the conversation from there."
        .to_string()
}

/// State shared between the blocked tool thread and the event loop.
#[derive(Debug, Default)]
struct GateInner {
    /// Decisions the loop has posted, keyed by request id — each taken by the
    /// one thread waiting on it.
    decisions: HashMap<String, AskDecision>,
}

/// The ask handshake: the tool thread sends its [`AskRequest`], blocks, and is
/// woken by the event loop's decision — the
/// [`crate::permission::PermissionGate`] sibling, minus the standing rules (an
/// answer never covers a later question). Cloneable; every clone shares one
/// decision board. See `docs/ask.md`.
#[derive(Debug, Clone, Default)]
pub struct AskGate {
    inner: Arc<Mutex<GateInner>>,
    posted: Arc<Condvar>,
    next: Arc<AtomicU64>,
}

impl AskGate {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A fresh request id, unique for this gate.
    #[must_use]
    pub fn next_id(&self) -> String {
        format!("ask_{}", self.next.fetch_add(1, Ordering::Relaxed))
    }

    /// Post the user's decision for `id`, waking the thread waiting on it.
    pub fn resolve(&self, id: &str, decision: AskDecision) {
        self.lock().decisions.insert(id.to_string(), decision);
        self.posted.notify_all();
    }

    /// Block until `id` is resolved, giving up (with `None`) as soon as
    /// `cancelled` returns true — so an Esc/quit reaps the waiting tool
    /// thread. The wait is a condvar with a short timeout, which is what lets
    /// the predicate be re-checked without the canceller knowing about us.
    #[must_use]
    pub fn wait(&self, id: &str, cancelled: &dyn Fn() -> bool) -> Option<AskDecision> {
        let mut guard = self.lock();
        loop {
            if let Some(decision) = guard.decisions.remove(id) {
                return Some(decision);
            }
            if cancelled() {
                return None;
            }
            let (next, _) = self
                .posted
                .wait_timeout(guard, WAIT_POLL)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            guard = next;
        }
    }

    /// Drop every posted-but-unclaimed decision (`/clear`, an interrupt): the
    /// waiting threads are cancelled separately and must not pick up a stale
    /// answer if one raced in.
    pub fn clear(&self) {
        self.lock().decisions.clear();
    }

    /// The inner state, recovering from a poisoned mutex (a panicking holder
    /// leaves the data structurally fine — dropping a decision is better than
    /// wedging the tool thread).
    fn lock(&self) -> std::sync::MutexGuard<'_, GateInner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal valid arguments object with `n` questions.
    fn args_with(n: usize) -> String {
        let question = serde_json::json!({
            "question": "Pick one?",
            "header": "Pick",
            "options": [
                {"label": "A", "description": "the first"},
                {"label": "B", "description": "the second"},
            ],
            "multiSelect": false,
        });
        serde_json::json!({ "questions": vec![question; n] }).to_string()
    }

    #[test]
    fn valid_arguments_parse_with_wire_field_names() {
        let questions = parse_questions(
            r#"{"questions":[{
                "question":"Which style?",
                "header":"Style",
                "options":[
                    {"label":"Arrow","description":"const f = () => {}","preview":"const x = 1"},
                    {"label":"Classic","description":"function f() {}"}
                ],
                "multiSelect":true
            }]}"#,
        )
        .expect("valid arguments");
        assert_eq!(questions.len(), 1);
        let q = &questions[0];
        assert_eq!(q.question, "Which style?");
        assert_eq!(q.header, "Style");
        assert!(q.multi_select, "multiSelect is the wire name");
        assert_eq!(q.options.len(), 2);
        assert_eq!(q.options[0].preview.as_deref(), Some("const x = 1"));
        assert!(q.has_previews());
        // multiSelect defaults to false; no previews without the field.
        let plain = parse_questions(&args_with(1)).unwrap();
        assert!(!plain[0].multi_select);
        assert!(!plain[0].has_previews());
    }

    #[test]
    fn the_schema_bounds_are_enforced_with_model_facing_messages() {
        // No questions / too many questions.
        let err = parse_questions(r#"{"questions":[]}"#).unwrap_err();
        assert!(err.contains("1 to 4"), "got {err}");
        let err = parse_questions(&args_with(5)).unwrap_err();
        assert!(err.contains("1 to 4"), "got {err}");
        // Too few options.
        let err = parse_questions(
            r#"{"questions":[{"question":"Q?","header":"H","options":[
                {"label":"A","description":"d"}],"multiSelect":false}]}"#,
        )
        .unwrap_err();
        assert!(err.contains("2 to 4"), "got {err}");
        // An empty question / label.
        let err = parse_questions(
            r#"{"questions":[{"question":"  ","header":"H","options":[
                {"label":"A","description":"d"},{"label":"B","description":"d"}],
                "multiSelect":false}]}"#,
        )
        .unwrap_err();
        assert!(err.contains("question must not be empty"), "got {err}");
        // Garbage JSON reports a parse error the model can react to.
        let err = parse_questions("not json").unwrap_err();
        assert!(err.contains("invalid tool arguments"), "got {err}");
    }

    fn question(text: &str, labels: &[&str], multi: bool) -> AskQuestion {
        AskQuestion {
            question: text.to_string(),
            header: "H".to_string(),
            options: labels
                .iter()
                .map(|l| AskOption {
                    label: (*l).to_string(),
                    description: format!("{l} described"),
                    preview: None,
                })
                .collect(),
            multi_select: multi,
        }
    }

    fn answer(q: &str, labels: &[&str]) -> AskAnswer {
        AskAnswer {
            question: q.to_string(),
            labels: labels.iter().map(|l| (*l).to_string()).collect(),
            notes: None,
            preview: None,
        }
    }

    #[test]
    fn the_answered_headline_speaks_the_products_own_name() {
        assert_eq!(
            ANSWERED_HEADLINE,
            format!("User answered {}'s questions:", crate::APP_NAME),
            "the cell speaks for THIS agent: the reference tool's own product \
             name must not survive the port"
        );
        assert!(
            !ANSWERED_HEADLINE.contains("Claude"),
            "got {ANSWERED_HEADLINE}"
        );
    }

    #[test]
    fn the_answered_cell_lists_each_question_and_its_labels() {
        let display = answered_display(&[
            answer("What's your favorite way to drink coffee?", &["Black"]),
            answer(
                "Which of these tool features would you like to see demoed next? (pick any number)",
                &["Preview panel", "Custom 'Other' input"],
            ),
        ]);
        assert_eq!(
            display,
            "User answered Alter Zero's questions:\n\
             · What's your favorite way to drink coffee? → Black\n\
             · Which of these tool features would you like to see demoed next? (pick any \
             number) → Preview panel, Custom 'Other' input"
        );
    }

    #[test]
    fn the_answered_result_is_the_schemas_answers_object() {
        let result = answered_result(&[
            answer("Pick a season", &["My answer"]),
            answer("Pick a snack (multiple allowed)", &["Chips", "Fruit"]),
        ]);
        let value: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(value["answers"]["Pick a season"], "My answer");
        assert_eq!(
            value["answers"]["Pick a snack (multiple allowed)"],
            "Chips, Fruit"
        );
        // No annotations key when nothing carries notes or previews.
        assert!(value.get("annotations").is_none(), "got {result}");
    }

    #[test]
    fn notes_and_previews_ride_the_annotations_object() {
        let with_notes = AskAnswer {
            question: "Which code style?".to_string(),
            labels: vec!["Arrow function".to_string()],
            notes: Some("prefer terse".to_string()),
            preview: Some("const greet = () => {}".to_string()),
        };
        let result = answered_result(&[with_notes, answer("Plain?", &["Yes"])]);
        let value: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(
            value["annotations"]["Which code style?"]["notes"],
            "prefer terse"
        );
        assert_eq!(
            value["annotations"]["Which code style?"]["preview"],
            "const greet = () => {}"
        );
        // The un-annotated question contributes no annotations entry.
        assert!(value["annotations"].get("Plain?").is_none());
        // Whitespace-only notes are no notes.
        let blank = AskAnswer {
            notes: Some("   ".to_string()),
            ..answer("Q?", &["A"])
        };
        let value: serde_json::Value = serde_json::from_str(&answered_result(&[blank])).unwrap();
        assert!(value.get("annotations").is_none());
    }

    #[test]
    fn a_decline_names_the_questions_and_their_options() {
        let questions = [question(
            "Which code style do you prefer for a simple greeting function?",
            &["Arrow function", "Function declaration", "One-liner"],
            false,
        )];
        assert_eq!(
            declined_display(&questions),
            "User declined to answer questions\n\
             · Which code style do you prefer for a simple greeting function? (Arrow \
             function / Function declaration / One-liner)"
        );
        let result = declined_result();
        assert!(result.contains("declined"), "got {result}");
        assert!(result.contains("STOP"), "got {result}");
    }

    #[test]
    fn chat_about_this_tells_the_model_the_user_wants_to_talk() {
        let questions = [question("Pick?", &["A", "B"], false)];
        let display = chat_display(&questions);
        assert!(display.starts_with(CHAT_HEADLINE), "got {display}");
        assert!(display.contains("· Pick? (A / B)"), "got {display}");
        let result = chat_result();
        assert!(result.contains("Chat about this"), "got {result}");
        assert!(result.contains("wait"), "got {result}");
    }

    // --- the gate ---

    #[test]
    fn ids_are_unique_per_gate() {
        let gate = AskGate::new();
        let a = gate.next_id();
        let b = gate.next_id();
        assert_ne!(a, b);
        assert!(a.starts_with("ask_"), "got {a}");
    }

    #[test]
    fn a_waiting_thread_wakes_with_the_posted_decision() {
        let gate = AskGate::new();
        let id = gate.next_id();
        let waiter = {
            let gate = gate.clone();
            let id = id.clone();
            std::thread::spawn(move || gate.wait(&id, &|| false))
        };
        std::thread::sleep(Duration::from_millis(30));
        gate.resolve(&id, AskDecision::Declined);
        assert_eq!(waiter.join().unwrap(), Some(AskDecision::Declined));
    }

    #[test]
    fn a_cancelled_wait_gives_up_instead_of_wedging_the_thread() {
        let gate = AskGate::new();
        let id = gate.next_id();
        let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let waiter = {
            let gate = gate.clone();
            let id = id.clone();
            let flag = cancelled.clone();
            std::thread::spawn(move || gate.wait(&id, &|| flag.load(Ordering::Relaxed)))
        };
        std::thread::sleep(Duration::from_millis(30));
        cancelled.store(true, Ordering::Relaxed);
        assert_eq!(waiter.join().unwrap(), None, "the wait is not a deadlock");
    }

    #[test]
    fn a_decision_posted_before_the_wait_is_picked_up_immediately() {
        let gate = AskGate::new();
        let id = gate.next_id();
        gate.resolve(&id, AskDecision::Chat);
        assert_eq!(gate.wait(&id, &|| true), Some(AskDecision::Chat));
        // …and each decision is taken by exactly one waiter.
        assert_eq!(gate.wait(&id, &|| true), None);
    }

    #[test]
    fn clearing_drops_unclaimed_decisions() {
        let gate = AskGate::new();
        let id = gate.next_id();
        gate.resolve(&id, AskDecision::Declined);
        gate.clear();
        assert_eq!(gate.wait(&id, &|| true), None);
    }
}
