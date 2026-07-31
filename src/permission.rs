//! Tool permission requests: the pure vocabulary behind the inline approval
//! prompt, and the gate the backend thread blocks on. See `docs/permissions.md`.
//!
//! Everything here is pure data plus one small piece of cross-thread
//! coordination ([`PermissionGate`], the `Arc<Mutex<…>> + Condvar` sibling of
//! [`crate::background::BackgroundRegistry`]). Nothing reads the filesystem or
//! the environment — the boundary builds a [`PermissionRequest`] and the event
//! loop resolves it.

use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

/// How often [`PermissionGate::wait`] re-checks its cancel predicate while
/// blocked, so an Esc/quit reaps the waiting tool thread promptly.
const WAIT_POLL: Duration = Duration::from_millis(50);

/// What a pending request is asking permission to do — the prompt's title, its
/// body shape, and which option-2 label it offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionKind {
    /// A `write` that creates a brand-new file: the body is its numbered
    /// contents.
    Write,
    /// An `edit`, or a `write` over an existing file: the body is the numbered
    /// diff hunks.
    Edit,
    /// A `bash` command: no framed body — the command and its description sit
    /// indented under the title.
    Bash,
}

/// One tool call waiting on the user's approval.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionRequest {
    /// The gate id this request is resolved by ([`PermissionGate::next_id`]).
    pub id: String,
    pub kind: PermissionKind,
    /// The file path (`Write`/`Edit`) or the whole command (`Bash`).
    pub target: String,
    /// The framed body: the numbered contents / diff for a file change, empty
    /// for a command (whose `target` *is* the body).
    pub body: String,
    /// The model's own one-line description of a `bash` call (its `description`
    /// argument), shown under the command. `None` for everything else.
    pub detail: Option<String>,
    /// The subagent type that asked (`general-purpose`), so the title can say
    /// `· from the general-purpose agent`. `None` for the main agent.
    pub agent: Option<String>,
}

/// What the user chose, handed back to the blocked tool thread. Esc is *not*
/// one of these: it abandons the request (the loop releases it as a plain
/// rejection) and interrupts the turn instead — `docs/permissions.md`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionDecision {
    /// Option 1 — run this call, ask again next time.
    Approve,
    /// Option 2 — run it and remember the scope ([`PermissionRules::remember`]).
    ApproveAlways,
    /// Option 3 (or Tab's amended rejection) — don't run it; the model is told
    /// to stop and wait, with this feedback appended when present.
    Deny(Option<String>),
    /// Ctrl+E on a `bash` prompt — don't run it; ask the model to explain the
    /// command first.
    Explain,
}

/// What [`crate::llm::agent::run_agent`]'s `approve` seam returns: run the
/// call, or resolve it without running it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Approval {
    /// Proceed — emit `ToolStart`, run the tool, emit `ToolEnd`.
    Allow,
    /// Don't run it. `display` is the short red cell output the user sees;
    /// `result` is the longer text the *model* reads as the tool result. Two
    /// fields so the cell can stay a one-liner while the instruction is
    /// complete (`docs/permissions.md`).
    Reject { display: String, result: String },
}

/// The prompt's title for a request kind — coloured in the rendered prompt.
#[must_use]
pub const fn title(kind: PermissionKind) -> &'static str {
    match kind {
        PermissionKind::Write => "Create file",
        PermissionKind::Edit => "Edit file",
        PermissionKind::Bash => "Bash command",
    }
}

/// The last path component of `path` (what the question names), falling back to
/// the whole string when there is no separator.
#[must_use]
pub fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

/// The prompt's question line.
#[must_use]
pub fn question(request: &PermissionRequest) -> String {
    match request.kind {
        PermissionKind::Write => {
            format!("Do you want to create {}?", file_name(&request.target))
        }
        PermissionKind::Edit => format!(
            "Do you want to make this edit to {}?",
            file_name(&request.target)
        ),
        PermissionKind::Bash => "Do you want to proceed?".to_string(),
    }
}

/// The three option labels, in order — the numbered rows under the question.
#[must_use]
pub fn options(request: &PermissionRequest) -> [String; 3] {
    let remember = match request.kind {
        PermissionKind::Bash => format!(
            "Yes, and don't ask again for: {} (a)",
            command_scope(&request.target).label()
        ),
        _ => "Yes, allow all edits during this session (a)".to_string(),
    };
    ["Yes".to_string(), remember, "No".to_string()]
}

/// The hint row's `(key, label)` pairs — Ctrl+E only exists for a command.
#[must_use]
pub fn hints(request: &PermissionRequest) -> Vec<(&'static str, &'static str)> {
    let mut out = vec![("Esc", " to cancel"), ("Tab", " to amend")];
    if request.kind == PermissionKind::Bash {
        out.push(("ctrl+e", " to explain"));
    }
    out
}

/// The label introducing Tab's amended instructions on the rejected cell's
/// second line — the user-facing short form of the sentence
/// [`denial_result`] hands the model.
const AMEND_DISPLAY_LABEL: &str = "Instructions: ";

/// The short output recorded on the rejected call's red cell, with Tab's
/// amended instructions on a second line when the user typed some — the only
/// place the transcript records what they asked for instead (the model reads
/// the fuller [`denial_result`]). Whitespace-only feedback is no feedback.
#[must_use]
pub fn denied_display(request: &PermissionRequest, feedback: Option<&str>) -> String {
    let headline = match request.kind {
        PermissionKind::Write => format!("User rejected write to {}", file_name(&request.target)),
        PermissionKind::Edit => format!("User rejected edit to {}", file_name(&request.target)),
        PermissionKind::Bash => "User rejected command".to_string(),
    };
    match feedback.map(str::trim).filter(|f| !f.is_empty()) {
        Some(text) => format!("{headline}\n{AMEND_DISPLAY_LABEL}{text}"),
        None => headline,
    }
}

/// The model-facing tool result for a rejection — Claude Code's wording, with
/// the amended feedback appended when the user typed some.
#[must_use]
pub fn denial_result(feedback: Option<&str>) -> String {
    let base = "The user doesn't want to proceed with this tool use. The tool use was rejected \
                (eg. if it was a file edit, the new_string was NOT written to the file). STOP \
                what you are doing and wait for the user to tell you how to proceed.";
    match feedback.map(str::trim).filter(|f| !f.is_empty()) {
        Some(text) => {
            format!("{base}\nThe user provided the following instructions instead: {text}")
        }
        None => base.to_string(),
    }
}

/// The model-facing tool result for Ctrl+E — explain the command instead of
/// running it.
#[must_use]
pub fn explain_result(request: &PermissionRequest) -> String {
    format!(
        "The user asked for an explanation instead of running this command. Do NOT run it yet. \
         Explain what `{}` does, what it would change, and why you want to run it, then ask \
         whether to proceed.",
        request.target
    )
}

/// The short output recorded on the cell for a Ctrl+E rejection.
#[must_use]
pub fn explain_display() -> String {
    "User asked for an explanation first".to_string()
}

/// What a `bash` command's "don't ask again" remembers — see
/// `docs/permissions.md`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandScope {
    /// Every segment reduced to a safe prefix. `keys` are all of them (all must
    /// be allow-listed for a later command to pass); `label` names the last —
    /// the command the user reads as the action.
    Prefixes { keys: Vec<String>, label: String },
    /// A segment carries a redirect or a substitution, which a prefix can't
    /// summarize: only this exact command is ever remembered or matched.
    Exact(String),
}

impl CommandScope {
    /// The text the option-2 label shows.
    #[must_use]
    pub fn label(&self) -> &str {
        match self {
            Self::Prefixes { label, .. } => label,
            Self::Exact(command) => command,
        }
    }

    /// The keys stored in the session allowlist when the user picks option 2.
    #[must_use]
    pub fn keys(&self) -> Vec<String> {
        match self {
            Self::Prefixes { keys, .. } => keys.clone(),
            Self::Exact(command) => vec![command.clone()],
        }
    }
}

/// Split a command into its pipeline/list segments on `|`, `||`, `&&`, `&`,
/// `;` and newlines, respecting single and double quotes. Empty segments are
/// dropped.
#[must_use]
pub fn command_segments(command: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut chars = command.chars().peekable();
    while let Some(c) = chars.next() {
        if let Some(q) = quote {
            current.push(c);
            if c == q {
                quote = None;
            }
            continue;
        }
        match c {
            '\'' | '"' => {
                quote = Some(c);
                current.push(c);
            }
            '|' | '&' => {
                if chars.peek() == Some(&c) {
                    chars.next(); // a doubled `||` / `&&` operator
                }
                out.push(std::mem::take(&mut current));
            }
            ';' | '\n' => out.push(std::mem::take(&mut current)),
            _ => current.push(c),
        }
    }
    out.push(current);
    out.into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Whether a segment carries a redirect or a substitution — something a prefix
/// would silently drop, so the scope degrades to an exact match.
fn has_redirect(segment: &str) -> bool {
    segment.contains('>')
        || segment.contains('<')
        || segment.contains('`')
        || segment.contains("$(")
}

/// One segment's prefix: its first token, plus the second when that isn't a
/// flag (`git status --short` → `git status`, `ls -la` → `ls`).
#[must_use]
pub fn segment_prefix(segment: &str) -> String {
    let mut tokens = segment.split_whitespace();
    let Some(first) = tokens.next() else {
        return String::new();
    };
    match tokens.next() {
        Some(second) if !second.starts_with('-') => format!("{first} {second}"),
        _ => first.to_string(),
    }
}

/// Reduce a command to what the session allowlist stores and shows.
#[must_use]
pub fn command_scope(command: &str) -> CommandScope {
    let segments = command_segments(command);
    if segments.is_empty() || segments.iter().any(|s| has_redirect(s)) {
        return CommandScope::Exact(command.trim().to_string());
    }
    let keys: Vec<String> = segments.iter().map(|s| segment_prefix(s)).collect();
    let label = keys.last().cloned().unwrap_or_default();
    CommandScope::Prefixes { keys, label }
}

/// The session's standing approvals: "allow all edits" plus the allow-listed
/// command scopes. Grows only via option 2; never persisted (a new session
/// starts asking again).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PermissionRules {
    /// Set by option 2 on a `write`/`edit` prompt — no file change asks again.
    pub allow_edits: bool,
    /// Allow-listed `bash` keys ([`CommandScope::keys`]).
    pub commands: BTreeSet<String>,
}

impl PermissionRules {
    /// Does a standing approval already cover this request?
    #[must_use]
    pub fn allows(&self, request: &PermissionRequest) -> bool {
        match request.kind {
            PermissionKind::Write | PermissionKind::Edit => self.allow_edits,
            PermissionKind::Bash => match command_scope(&request.target) {
                CommandScope::Prefixes { keys, .. } => {
                    !keys.is_empty() && keys.iter().all(|k| self.commands.contains(k))
                }
                CommandScope::Exact(command) => self.commands.contains(&command),
            },
        }
    }

    /// Record option 2's standing approval for this request.
    pub fn remember(&mut self, request: &PermissionRequest) {
        match request.kind {
            PermissionKind::Write | PermissionKind::Edit => self.allow_edits = true,
            PermissionKind::Bash => self.commands.extend(command_scope(&request.target).keys()),
        }
    }
}

/// State shared between the blocked tool threads and the event loop.
#[derive(Debug, Default)]
struct GateInner {
    rules: PermissionRules,
    /// Decisions the loop has posted, keyed by request id — each taken by the
    /// one thread waiting on it.
    decisions: HashMap<String, PermissionDecision>,
}

/// The permission handshake: the tool thread asks, blocks, and is woken by the
/// event loop's decision. Cloneable — every clone shares one set of rules and
/// one decision board. See `docs/permissions.md`.
#[derive(Debug, Clone, Default)]
pub struct PermissionGate {
    inner: Arc<Mutex<GateInner>>,
    posted: Arc<Condvar>,
    next: Arc<AtomicU64>,
}

impl PermissionGate {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A fresh request id, unique for this gate.
    #[must_use]
    pub fn next_id(&self) -> String {
        format!("perm_{}", self.next.fetch_add(1, Ordering::Relaxed))
    }

    /// Is this request already covered by a standing approval (so no prompt is
    /// raised at all)?
    #[must_use]
    pub fn allows(&self, request: &PermissionRequest) -> bool {
        self.lock().rules.allows(request)
    }

    /// Record option 2's standing approval.
    pub fn remember(&self, request: &PermissionRequest) {
        self.lock().rules.remember(request);
    }

    /// Post the user's decision for `id`, waking the thread waiting on it.
    pub fn resolve(&self, id: &str, decision: PermissionDecision) {
        self.lock().decisions.insert(id.to_string(), decision);
        self.posted.notify_all();
    }

    /// Block until `id` is resolved, giving up (with `None`) as soon as
    /// `cancelled` returns true — so an Esc/quit reaps a waiting tool thread.
    /// The wait is a condvar with a short timeout, which is what lets
    /// the predicate be re-checked without the canceller knowing about us.
    #[must_use]
    pub fn wait(&self, id: &str, cancelled: &dyn Fn() -> bool) -> Option<PermissionDecision> {
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

    /// The session rules, for tests and the boundary's introspection.
    #[must_use]
    pub fn rules(&self) -> PermissionRules {
        self.lock().rules.clone()
    }

    /// The inner state, recovering from a poisoned mutex (a panicking holder
    /// leaves the data structurally fine — dropping a decision is better than
    /// wedging every tool thread).
    fn lock(&self) -> std::sync::MutexGuard<'_, GateInner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(kind: PermissionKind, target: &str) -> PermissionRequest {
        PermissionRequest {
            id: "perm_0".to_string(),
            kind,
            target: target.to_string(),
            body: String::new(),
            detail: None,
            agent: None,
        }
    }

    // --- the prompt's text ---

    #[test]
    fn each_kind_has_its_own_title_and_question() {
        assert_eq!(title(PermissionKind::Write), "Create file");
        assert_eq!(title(PermissionKind::Edit), "Edit file");
        assert_eq!(title(PermissionKind::Bash), "Bash command");
        assert_eq!(
            question(&request(PermissionKind::Write, "src/hello.py")),
            "Do you want to create hello.py?"
        );
        assert_eq!(
            question(&request(PermissionKind::Edit, "src/script.py")),
            "Do you want to make this edit to script.py?"
        );
        assert_eq!(
            question(&request(PermissionKind::Bash, "ls -la")),
            "Do you want to proceed?"
        );
    }

    #[test]
    fn the_second_option_names_what_it_remembers() {
        let edits = options(&request(PermissionKind::Write, "hello.py"));
        assert_eq!(edits[0], "Yes");
        assert_eq!(edits[1], "Yes, allow all edits during this session (a)");
        assert_eq!(edits[2], "No");
        // The command prompt names the prefix it would allow-list — the last
        // segment, the one the user reads as the action.
        let bash = options(&request(
            PermissionKind::Bash,
            r#"echo "" | python3 script.py"#,
        ));
        assert_eq!(
            bash[1],
            "Yes, and don't ask again for: python3 script.py (a)"
        );
    }

    #[test]
    fn only_a_command_prompt_offers_ctrl_e() {
        assert_eq!(
            hints(&request(PermissionKind::Bash, "ls"))
                .iter()
                .map(|(k, _)| *k)
                .collect::<Vec<_>>(),
            vec!["Esc", "Tab", "ctrl+e"]
        );
        assert_eq!(
            hints(&request(PermissionKind::Edit, "a.py"))
                .iter()
                .map(|(k, _)| *k)
                .collect::<Vec<_>>(),
            vec!["Esc", "Tab"]
        );
    }

    #[test]
    fn a_rejection_shows_a_short_cell_and_tells_the_model_to_stop() {
        assert_eq!(
            denied_display(&request(PermissionKind::Write, "src/hello.py"), None),
            "User rejected write to hello.py"
        );
        assert_eq!(
            denied_display(&request(PermissionKind::Bash, "rm -rf /"), None),
            "User rejected command"
        );
        let plain = denial_result(None);
        assert!(plain.contains("doesn't want to proceed"), "got {plain}");
        assert!(!plain.contains("instructions instead"), "got {plain}");
        let amended = denial_result(Some("just print it instead"));
        assert!(
            amended.ends_with("instructions instead: just print it instead"),
            "got {amended}"
        );
        // Whitespace-only feedback is no feedback.
        assert_eq!(denial_result(Some("   ")), plain);
    }

    #[test]
    fn an_amended_rejection_records_the_typed_instructions_on_the_cell_too() {
        // Tab's feedback is the only trace of what the user typed — without it
        // on the cell, the transcript says the call was rejected and nothing
        // about why. It rides a second line, under the rejection headline.
        let display = denied_display(
            &request(PermissionKind::Write, "src/hello.py"),
            Some("just print it instead"),
        );
        assert_eq!(
            display,
            "User rejected write to hello.py\nInstructions: just print it instead"
        );
        // Whitespace-only feedback is no feedback — the plain one-liner stands.
        assert_eq!(
            denied_display(&request(PermissionKind::Bash, "rm -rf /"), Some("  ")),
            "User rejected command"
        );
    }

    #[test]
    fn ctrl_e_asks_the_model_to_explain_the_exact_command() {
        let result = explain_result(&request(PermissionKind::Bash, "python3 script.py"));
        assert!(result.contains("Do NOT run it yet"), "got {result}");
        assert!(result.contains("`python3 script.py`"), "got {result}");
    }

    // --- command scopes ---

    #[test]
    fn a_command_splits_into_segments_around_operators_but_not_inside_quotes() {
        assert_eq!(
            command_segments(r#"echo "a | b" && ls -la; pwd"#),
            vec![r#"echo "a | b""#, "ls -la", "pwd"]
        );
        assert_eq!(command_segments("  "), Vec::<String>::new());
    }

    #[test]
    fn a_segment_reduces_to_its_command_and_first_non_flag_argument() {
        assert_eq!(segment_prefix("python3 script.py"), "python3 script.py");
        assert_eq!(segment_prefix("git status --short"), "git status");
        assert_eq!(segment_prefix("ls -la"), "ls");
        assert_eq!(segment_prefix("pwd"), "pwd");
        assert_eq!(segment_prefix(""), "");
    }

    #[test]
    fn the_scope_keeps_every_segments_prefix_but_labels_the_last() {
        let scope = command_scope(r#"echo "" | python3 script.py"#);
        assert_eq!(scope.label(), "python3 script.py");
        assert_eq!(
            scope.keys(),
            vec![r#"echo """#.to_string(), "python3 script.py".to_string()],
            "every segment is remembered, so the identical command never re-asks"
        );
    }

    #[test]
    fn a_redirect_degrades_the_scope_to_the_exact_command() {
        // A prefix would drop the part that matters, so nothing is summarized.
        let scope = command_scope("python3 script.py > /etc/passwd");
        assert_eq!(
            scope,
            CommandScope::Exact("python3 script.py > /etc/passwd".to_string())
        );
        assert_eq!(scope.label(), "python3 script.py > /etc/passwd");
        assert!(matches!(
            command_scope("echo `whoami`"),
            CommandScope::Exact(_)
        ));
        assert!(matches!(
            command_scope("echo $(whoami)"),
            CommandScope::Exact(_)
        ));
    }

    // --- the session rules ---

    #[test]
    fn allow_all_edits_covers_both_file_kinds_but_never_a_command() {
        let mut rules = PermissionRules::default();
        rules.remember(&request(PermissionKind::Write, "a.py"));
        assert!(rules.allow_edits);
        assert!(rules.allows(&request(PermissionKind::Write, "other.py")));
        assert!(rules.allows(&request(PermissionKind::Edit, "other.py")));
        assert!(!rules.allows(&request(PermissionKind::Bash, "ls")));
    }

    #[test]
    fn an_allow_listed_command_repeats_silently_but_a_new_tail_still_asks() {
        let mut rules = PermissionRules::default();
        let approved = request(PermissionKind::Bash, r#"echo "" | python3 script.py"#);
        rules.remember(&approved);
        assert!(
            rules.allows(&approved),
            "the identical command never re-asks"
        );
        assert!(rules.allows(&request(PermissionKind::Bash, "python3 script.py")));
        // Every segment must be allow-listed — a smuggled tail is not.
        assert!(!rules.allows(&request(
            PermissionKind::Bash,
            "python3 script.py; rm -rf /"
        )));
        // …nor is a redirect the approved prefix never mentioned.
        assert!(!rules.allows(&request(
            PermissionKind::Bash,
            "python3 script.py > /etc/passwd"
        )));
    }

    #[test]
    fn nothing_is_allowed_by_default() {
        let rules = PermissionRules::default();
        for kind in [PermissionKind::Write, PermissionKind::Edit] {
            assert!(!rules.allows(&request(kind, "a.py")));
        }
        for command in ["ls", "cat README.md", "rm -rf /"] {
            assert!(
                !rules.allows(&request(PermissionKind::Bash, command)),
                "there is no built-in safe list: {command}"
            );
        }
    }

    // --- the gate ---

    #[test]
    fn ids_are_unique_per_gate() {
        let gate = PermissionGate::new();
        let a = gate.next_id();
        let b = gate.next_id();
        assert_ne!(a, b);
        assert!(a.starts_with("perm_"), "got {a}");
    }

    #[test]
    fn a_waiting_thread_wakes_with_the_posted_decision() {
        let gate = PermissionGate::new();
        let id = gate.next_id();
        let waiter = {
            let gate = gate.clone();
            let id = id.clone();
            std::thread::spawn(move || gate.wait(&id, &|| false))
        };
        // Post from "the event loop" once the waiter is surely parked.
        std::thread::sleep(Duration::from_millis(30));
        gate.resolve(&id, PermissionDecision::ApproveAlways);
        assert_eq!(
            waiter.join().unwrap(),
            Some(PermissionDecision::ApproveAlways)
        );
    }

    #[test]
    fn a_cancelled_wait_gives_up_instead_of_wedging_the_thread() {
        let gate = PermissionGate::new();
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
        // The loop can resolve before the tool thread reaches its wait (a
        // remembered scope, a fast key) — the decision board holds it.
        let gate = PermissionGate::new();
        let id = gate.next_id();
        gate.resolve(&id, PermissionDecision::Approve);
        assert_eq!(gate.wait(&id, &|| true), Some(PermissionDecision::Approve));
    }

    #[test]
    fn each_decision_is_taken_by_exactly_one_waiter() {
        let gate = PermissionGate::new();
        let id = gate.next_id();
        gate.resolve(&id, PermissionDecision::Approve);
        assert!(gate.wait(&id, &|| false).is_some());
        assert_eq!(gate.wait(&id, &|| true), None, "the decision was consumed");
    }

    #[test]
    fn clearing_drops_unclaimed_decisions() {
        let gate = PermissionGate::new();
        let id = gate.next_id();
        gate.resolve(&id, PermissionDecision::Approve);
        gate.clear();
        assert_eq!(gate.wait(&id, &|| true), None);
    }

    #[test]
    fn remembering_through_the_gate_is_shared_by_every_clone() {
        let gate = PermissionGate::new();
        let req = request(PermissionKind::Bash, "cargo test");
        assert!(!gate.allows(&req));
        gate.clone().remember(&req);
        assert!(gate.allows(&req), "the rules live behind the shared Arc");
        assert!(gate.rules().commands.contains("cargo test"));
    }
}
