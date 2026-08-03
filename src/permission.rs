//! Tool permission requests: the pure vocabulary behind the inline approval
//! prompt, and the gate the backend thread blocks on. See `docs/permissions.md`.
//!
//! Everything here is pure data plus one small piece of cross-thread
//! coordination ([`PermissionGate`], the `Arc<Mutex<…>> + Condvar` sibling of
//! [`crate::background::BackgroundRegistry`]). Nothing reads the filesystem or
//! the environment — the boundary builds a [`PermissionRequest`] and the event
//! loop resolves it.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// How often [`PermissionGate::wait`] re-checks its cancel predicate while
/// blocked, so an Esc/quit reaps the waiting tool thread promptly.
const WAIT_POLL: Duration = Duration::from_millis(50);

/// The session's permission posture — which tool calls ask before running.
/// Pinned at the footer's right edge (`{model} · {cwd}      manual`), toggled
/// with **Ctrl+A** (option 2 on a `write`/`edit` prompt switches to `Edit`
/// too), and persisted per project in `permissions.json`. See
/// `docs/permissions.md`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PermissionMode {
    /// Ask before every `write`/`edit` **and** every `bash` command — Claude
    /// Code's default posture, and the only one that can't be wrong about
    /// what is safe.
    #[default]
    Manual,
    /// Auto-approve `write`/`edit` (Claude Code's "auto-accept edits on");
    /// `bash` commands still ask until allow-listed.
    Edit,
}

impl PermissionMode {
    /// The footer / permissions-file label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Edit => "edit",
        }
    }

    /// Parse a persisted label; unknown text is `None` (callers default to
    /// [`Manual`](Self::Manual), so a hand-edited file degrades safely).
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim() {
            "manual" => Some(Self::Manual),
            "edit" => Some(Self::Edit),
            _ => None,
        }
    }

    /// The other mode — Ctrl+A's toggle.
    #[must_use]
    pub const fn toggled(self) -> Self {
        match self {
            Self::Manual => Self::Edit,
            Self::Edit => Self::Manual,
        }
    }
}

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

/// How many options every prompt offers (Yes / remember / No) — the length of
/// [`options`]. Shared so the key map's clamp and the cursor's seat on the
/// highlighted row (`ui::cursor_position`) count the same rows the renderer
/// draws.
pub const OPTION_COUNT: usize = 3;

/// The three option labels, in order — the numbered rows under the question.
///
/// The `bash` remember row names the rule it would store —
/// [`CommandScope::display`]'s `{prefix} *`, the star saying "this program,
/// any arguments" (an exact-only scope shows the whole command, no star) —
/// and is picked with `2` or ↑/↓ + Enter, like Claude Code. The file-change
/// remember row carries its shortcut: **Ctrl+A**, the permission-mode toggle,
/// because choosing it *is* the switch to [`PermissionMode::Edit`].
#[must_use]
pub fn options(request: &PermissionRequest) -> [String; OPTION_COUNT] {
    let remember = match request.kind {
        PermissionKind::Bash => format!(
            "Yes, and don't ask again for: {}",
            command_scope(&request.target).display()
        ),
        _ => "Yes, allow all edits during this session (ctrl+a)".to_string(),
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
    /// The rule's bare name: the labelled prefix, or the exact command.
    #[must_use]
    pub fn label(&self) -> &str {
        match self {
            Self::Prefixes { label, .. } => label,
            Self::Exact(command) => command,
        }
    }

    /// What the option-2 label and the boundary's toast show: a prefix rule
    /// as `{prefix} *` — the star saying "this program, any arguments",
    /// Claude Code's `Bash(prefix:*)` — and an exact rule as the whole
    /// command, whose promise is exactly itself.
    #[must_use]
    pub fn display(&self) -> String {
        match self {
            Self::Prefixes { label, .. } => format!("{label} *"),
            Self::Exact(command) => command.clone(),
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
/// would silently drop, so the scope degrades to an exact match. Quote-aware:
/// `>`/`<` are literal inside quotes (`echo "a > b"` redirects nothing — the
/// old raw `contains` degraded it for no reason), while `` ` `` and `$(` stay
/// live inside **double** quotes (`"$(whoami)"` runs) and are inert only in
/// single ones.
fn has_redirect(segment: &str) -> bool {
    let mut quote: Option<char> = None;
    let mut chars = segment.chars().peekable();
    while let Some(c) = chars.next() {
        match quote {
            Some('\'') => {
                if c == '\'' {
                    quote = None;
                }
            }
            Some(_) => match c {
                '"' => quote = None,
                '`' => return true,
                '$' if chars.peek() == Some(&'(') => return true,
                _ => {}
            },
            None => match c {
                '\'' | '"' => quote = Some(c),
                '>' | '<' | '`' => return true,
                '$' if chars.peek() == Some(&'(') => return true,
                _ => {}
            },
        }
    }
    false
}

/// Programs whose second word is a *subcommand* worth keeping in the rule:
/// `git *` would cover `git push --force`, so `git status` keeps its verb —
/// while for everything else the second token is an argument
/// (`python3 script.py` → `python3`), Claude Code's `prefix:*` shape.
const SUBCOMMAND_TOOLS: &[&str] = &[
    "apt",
    "apt-get",
    "aws",
    "az",
    "brew",
    "bun",
    "bundle",
    "cargo",
    "composer",
    "conda",
    "deno",
    "dnf",
    "docker",
    "gcloud",
    "gem",
    "gh",
    "git",
    "glab",
    "go",
    "gradle",
    "helm",
    "just",
    "kubectl",
    "mise",
    "mvn",
    "npm",
    "pacman",
    "pip",
    "pip3",
    "pnpm",
    "podman",
    "poetry",
    "rake",
    "rustup",
    "snap",
    "systemctl",
    "terraform",
    "uv",
    "uvx",
    "yarn",
    "yum",
];

/// Programs that *run other commands* — shells taking inline code, privilege
/// escalators, wrappers. A prefix like `sudo` or `sh` would allow-list every
/// command they can carry, so a segment led by one is never summarized.
const COMMAND_WRAPPERS: &[&str] = &[
    "bash", "busybox", "command", "dash", "doas", "env", "eval", "exec", "fish", "ksh", "nohup",
    "setsid", "sh", "source", "su", "sudo", "time", "timeout", "watch", "xargs", "zsh",
];

/// Is this token a bare subcommand word (letters/digits/`-`/`_`) rather than a
/// flag, a path, or a value?
fn subcommand_like(token: &str) -> bool {
    !token.is_empty()
        && !token.starts_with('-')
        && token
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// One segment's allowlist prefix — the program word, plus its subcommand for
/// the tools that have them (`git status --short` → `git status`,
/// `python3 script.py` → `python3`, `ls -la` → `ls`). `None` when no prefix
/// can honestly summarize the segment: an env assignment (`FOO=1 …` can
/// redirect what the program does — think `PATH=…`) or a command wrapper
/// (`sudo …`, `sh -c …` — the "argument" is itself a command), both of which
/// degrade the whole scope to an exact match.
#[must_use]
pub fn segment_prefix(segment: &str) -> Option<String> {
    let mut tokens = segment.split_whitespace();
    let first = tokens.next()?;
    if first.contains('=') || COMMAND_WRAPPERS.contains(&first) {
        return None;
    }
    match tokens.next() {
        Some(second) if SUBCOMMAND_TOOLS.contains(&first) && subcommand_like(second) => {
            Some(format!("{first} {second}"))
        }
        _ => Some(first.to_string()),
    }
}

/// Reduce a command to what the session allowlist stores and shows.
#[must_use]
pub fn command_scope(command: &str) -> CommandScope {
    let segments = command_segments(command);
    let exact = || CommandScope::Exact(command.trim().to_string());
    if segments.is_empty() {
        return exact();
    }
    let mut keys = Vec::new();
    for segment in &segments {
        if has_redirect(segment) {
            return exact();
        }
        match segment_prefix(segment) {
            Some(prefix) => keys.push(prefix),
            None => return exact(),
        }
    }
    let label = keys.last().cloned().unwrap_or_default();
    CommandScope::Prefixes { keys, label }
}

/// The session's standing approvals: the [`PermissionMode`] plus the
/// allow-listed command scopes. Grown by option 2 and the Ctrl+A toggle, and
/// **persisted per project** (`permissions.json`, see [`PermissionsFile`]) so
/// a new session in the same directory starts where this one left off.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PermissionRules {
    /// The permission posture: [`PermissionMode::Edit`] auto-approves every
    /// file change (option 2 on a `write`/`edit` prompt, or Ctrl+A); back in
    /// [`PermissionMode::Manual`] every file change asks again. Commands ask
    /// in both modes until allow-listed below.
    pub mode: PermissionMode,
    /// Allow-listed command prefixes ([`CommandScope::Prefixes`] keys): every
    /// segment prefix of a later command must be here for it to run unasked.
    pub prefixes: BTreeSet<String>,
    /// Allow-listed exact commands ([`CommandScope::Exact`]): matched only
    /// byte-for-byte.
    pub exact: BTreeSet<String>,
}

impl PermissionRules {
    /// Does a standing approval already cover this request?
    #[must_use]
    pub fn allows(&self, request: &PermissionRequest) -> bool {
        match request.kind {
            PermissionKind::Write | PermissionKind::Edit => self.mode == PermissionMode::Edit,
            PermissionKind::Bash => match command_scope(&request.target) {
                CommandScope::Prefixes { keys, .. } => {
                    !keys.is_empty() && keys.iter().all(|k| self.prefixes.contains(k))
                }
                CommandScope::Exact(command) => self.exact.contains(&command),
            },
        }
    }

    /// Record option 2's standing approval for this request — a file prompt's
    /// switches the mode to [`PermissionMode::Edit`], a command prompt's
    /// stores its scope in the matching allowlist.
    pub fn remember(&mut self, request: &PermissionRequest) {
        match request.kind {
            PermissionKind::Write | PermissionKind::Edit => self.mode = PermissionMode::Edit,
            PermissionKind::Bash => match command_scope(&request.target) {
                CommandScope::Prefixes { keys, .. } => self.prefixes.extend(keys),
                CommandScope::Exact(command) => {
                    self.exact.insert(command);
                }
            },
        }
    }
}

/// The suffix marking a persisted **prefix** rule (`"python3 *"`); an entry
/// without it is an exact command. Exact rules only exist for commands a
/// prefix can't summarize (redirects, substitutions, wrappers, assignments),
/// none of which [`command_scope`] ever reduces to a two-token ` *` tail — so
/// the marker can't be mistaken for content that matters.
const PREFIX_RULE_SUFFIX: &str = " *";

/// The persisted permissions — `~/.alter-zero/permissions.json` — keyed by
/// **project directory** so "don't ask again" and the [`PermissionMode`] stick
/// to the project they were granted in, Claude Code's per-project settings:
///
/// ```json
/// {
///   "projects": {
///     "/home/user/project": {
///       "allow_commands": ["python3 *", "git status *"],
///       "mode": "edit"
///     }
///   }
/// }
/// ```
///
/// Pure format/parse (the `Settings` pattern, `docs/llm.md`) — the file
/// read/write lives in `main.rs`, which loads it at startup to seed the gate
/// and rewrites this project's entry on every rule change.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionsFile {
    /// One entry per project directory (its absolute path).
    #[serde(default)]
    pub projects: BTreeMap<String, ProjectPermissions>,
}

/// One project's persisted permissions.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectPermissions {
    /// The allow-listed commands: `{prefix} *` rows are prefix rules, anything
    /// else is an exact command (the `PREFIX_RULE_SUFFIX` encoding).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allow_commands: Vec<String>,
    /// The project's saved [`PermissionMode`] label; absent = `manual` (the
    /// default posture is not worth writing out, and old files stay valid).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
}

impl ProjectPermissions {
    /// The entry recording the session's live rules.
    #[must_use]
    pub fn from_rules(rules: &PermissionRules) -> Self {
        let mut allow_commands: Vec<String> = rules
            .prefixes
            .iter()
            .map(|p| format!("{p}{PREFIX_RULE_SUFFIX}"))
            .chain(rules.exact.iter().cloned())
            .collect();
        allow_commands.sort();
        Self {
            allow_commands,
            mode: (rules.mode != PermissionMode::Manual).then(|| rules.mode.label().to_string()),
        }
    }

    /// The saved mode, defaulting to `manual` for an absent or unknown label
    /// (a hand-edited file degrades to asking, never to allowing).
    #[must_use]
    pub fn saved_mode(&self) -> PermissionMode {
        self.mode
            .as_deref()
            .and_then(PermissionMode::parse)
            .unwrap_or_default()
    }

    /// Decode [`allow_commands`](Self::allow_commands) into the rules' two
    /// sets: `(prefixes, exact)`.
    #[must_use]
    pub fn command_sets(&self) -> (BTreeSet<String>, BTreeSet<String>) {
        let mut prefixes = BTreeSet::new();
        let mut exact = BTreeSet::new();
        for entry in &self.allow_commands {
            match entry.strip_suffix(PREFIX_RULE_SUFFIX) {
                Some(prefix) if !prefix.is_empty() => {
                    prefixes.insert(prefix.to_string());
                }
                _ => {
                    exact.insert(entry.clone());
                }
            }
        }
        (prefixes, exact)
    }
}

impl PermissionsFile {
    /// Parse a `permissions.json` body, best-effort: malformed or empty JSON
    /// yields the empty file rather than an error, so a corrupt file never
    /// blocks startup (the `Settings::parse` posture).
    #[must_use]
    pub fn parse(text: &str) -> Self {
        serde_json::from_str(text).unwrap_or_default()
    }

    /// Serialize to pretty JSON for writing back. A map of strings can't
    /// really fail to serialize; an impossible failure falls back to `{}`.
    #[must_use]
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".to_string())
    }

    /// The saved entry for a project directory, if any.
    #[must_use]
    pub fn project(&self, dir: &str) -> Option<&ProjectPermissions> {
        self.projects.get(dir)
    }

    /// Record `rules` as `dir`'s entry (replacing what was there), leaving
    /// every other project's entry untouched — the caller re-reads the file
    /// first, so two instances in two directories never clobber each other.
    pub fn record(&mut self, dir: &str, rules: &PermissionRules) {
        self.projects
            .insert(dir.to_string(), ProjectPermissions::from_rules(rules));
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

    /// The session's [`PermissionMode`] (the footer's right-edge segment).
    #[must_use]
    pub fn mode(&self) -> PermissionMode {
        self.lock().rules.mode
    }

    /// Set the permission mode — the Ctrl+A toggle, or the startup seed from
    /// the project's `permissions.json` entry. Takes effect for the very next
    /// `approve` consult: a `write` raised after a switch to
    /// [`PermissionMode::Edit`] never asks.
    pub fn set_mode(&self, mode: PermissionMode) {
        self.lock().rules.mode = mode;
    }

    /// Seed the command allowlists (the startup load of `permissions.json` —
    /// [`ProjectPermissions::command_sets`]).
    pub fn seed_commands(
        &self,
        prefixes: impl IntoIterator<Item = String>,
        exact: impl IntoIterator<Item = String>,
    ) {
        let mut inner = self.lock();
        inner.rules.prefixes.extend(prefixes);
        inner.rules.exact.extend(exact);
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
        // The edit option carries its shortcut — Ctrl+A, the mode toggle —
        // and choosing it IS the switch to edit mode (docs/permissions.md).
        let edits = options(&request(PermissionKind::Write, "hello.py"));
        assert_eq!(edits[0], "Yes");
        assert_eq!(
            edits[1],
            "Yes, allow all edits during this session (ctrl+a)"
        );
        assert_eq!(edits[2], "No");
        // The command prompt names the rule it would store — the last
        // segment's program with the `*` saying "any arguments", Claude
        // Code's `prefix:*` — with no `(a)` shortcut (only 2/↑↓+Enter pick it).
        let bash = options(&request(
            PermissionKind::Bash,
            r#"echo "" | python3 script.py"#,
        ));
        assert_eq!(bash[1], "Yes, and don't ask again for: python3 *");
        // An exact-only scope (here: a redirect) shows the whole command, no
        // star — nothing broader than this byte-identical command is stored.
        let exact = options(&request(PermissionKind::Bash, "python3 x.py > out.txt"));
        assert_eq!(
            exact[1],
            "Yes, and don't ask again for: python3 x.py > out.txt"
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
    fn a_segment_reduces_to_its_program_word() {
        // A file argument is an argument, not part of the rule — approving
        // `python3 script.py` offers `python3 *`, Claude Code's `python3:*`
        // (the old prefix kept the script name, so every new file re-asked).
        assert_eq!(segment_prefix("python3 script.py"), Some("python3".into()));
        assert_eq!(segment_prefix("mkdir foo"), Some("mkdir".into()));
        assert_eq!(segment_prefix("ls -la"), Some("ls".into()));
        assert_eq!(segment_prefix("pwd"), Some("pwd".into()));
        assert_eq!(
            segment_prefix("./deploy.sh prod"),
            Some("./deploy.sh".into())
        );
        assert_eq!(segment_prefix(""), None);
    }

    #[test]
    fn a_subcommand_tool_keeps_its_verb_in_the_prefix() {
        // `git *` would cover `git push --force`; the curated subcommand
        // tools keep their verb so the rule stays as narrow as the action.
        assert_eq!(
            segment_prefix("git status --short"),
            Some("git status".into())
        );
        assert_eq!(segment_prefix("cargo test"), Some("cargo test".into()));
        assert_eq!(segment_prefix("npm run build"), Some("npm run".into()));
        // …but only when the second token reads as a verb: a flag or a path
        // there is an argument, and the rule falls back to the program.
        assert_eq!(segment_prefix("git -C /tmp status"), Some("git".into()));
        assert_eq!(segment_prefix("go ./cmd/serve"), Some("go".into()));
    }

    #[test]
    fn an_env_assignment_or_command_wrapper_degrades_to_the_exact_command() {
        // `FOO=1 …` can redirect what the program does (PATH=…), and a
        // wrapper's "argument" IS a command (`sudo rm`, `sh -c '…'`): a
        // prefix like `sudo` would allow-list everything it can carry. No
        // honest prefix exists, so only the byte-identical command matches.
        assert_eq!(segment_prefix("FOO=1 python3 x.py"), None);
        assert_eq!(segment_prefix("sudo apt install x"), None);
        assert_eq!(segment_prefix("sh -c 'rm -rf /'"), None);
        assert_eq!(segment_prefix("xargs rm"), None);
        assert_eq!(
            command_scope("FOO=1 ls"),
            CommandScope::Exact("FOO=1 ls".to_string())
        );
        assert_eq!(
            command_scope("ls && sudo rm x"),
            CommandScope::Exact("ls && sudo rm x".to_string()),
            "one unsummarizable segment degrades the whole command"
        );
    }

    #[test]
    fn the_scope_keeps_every_segments_prefix_but_labels_the_last() {
        let scope = command_scope(r#"echo "" | python3 script.py"#);
        assert_eq!(scope.label(), "python3");
        assert_eq!(
            scope.keys(),
            vec!["echo".to_string(), "python3".to_string()],
            "every segment is remembered, so the identical command never re-asks"
        );
    }

    #[test]
    fn a_prefix_rule_displays_with_a_star_and_an_exact_one_verbatim() {
        assert_eq!(command_scope("python3 script.py").display(), "python3 *");
        assert_eq!(
            command_scope("git status --short").display(),
            "git status *"
        );
        assert_eq!(
            command_scope("cat a > b").display(),
            "cat a > b",
            "an exact rule promises nothing broader, so no star"
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

    #[test]
    fn quoted_redirect_characters_are_literal_not_redirects() {
        // `>` inside quotes redirects nothing — the old raw `contains('>')`
        // degraded `echo "a > b"` to an exact match for no reason. But a
        // substitution stays live inside DOUBLE quotes (`"$(whoami)"` runs),
        // so only single quotes defuse those.
        assert!(matches!(
            command_scope(r#"echo "a > b""#),
            CommandScope::Prefixes { .. }
        ));
        assert!(matches!(
            command_scope("echo '$(whoami)'"),
            CommandScope::Prefixes { .. }
        ));
        assert!(matches!(
            command_scope(r#"echo "$(whoami)""#),
            CommandScope::Exact(_)
        ));
        assert!(matches!(
            command_scope(r#"echo "`whoami`""#),
            CommandScope::Exact(_)
        ));
    }

    // --- the session rules ---

    #[test]
    fn allow_all_edits_covers_both_file_kinds_but_never_a_command() {
        // Option 2 on a file prompt IS the switch to edit mode.
        let mut rules = PermissionRules::default();
        rules.remember(&request(PermissionKind::Write, "a.py"));
        assert_eq!(rules.mode, PermissionMode::Edit);
        assert!(rules.allows(&request(PermissionKind::Write, "other.py")));
        assert!(rules.allows(&request(PermissionKind::Edit, "other.py")));
        assert!(!rules.allows(&request(PermissionKind::Bash, "ls")));
        // Back to manual: edits ask again, the allowlists untouched.
        rules.mode = PermissionMode::Manual;
        assert!(!rules.allows(&request(PermissionKind::Write, "other.py")));
        assert!(!rules.allows(&request(PermissionKind::Edit, "other.py")));
    }

    #[test]
    fn the_mode_labels_round_trip_and_toggle() {
        assert_eq!(PermissionMode::default(), PermissionMode::Manual);
        for mode in [PermissionMode::Manual, PermissionMode::Edit] {
            assert_eq!(PermissionMode::parse(mode.label()), Some(mode));
            assert_ne!(mode.toggled(), mode);
            assert_eq!(mode.toggled().toggled(), mode);
        }
        assert_eq!(PermissionMode::parse("turbo"), None);
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
        // The rule is `python3 *`: a different script is covered too — the
        // point of the star (the old script-name prefix re-asked for every
        // new file, making "don't ask again" nearly useless).
        assert!(rules.allows(&request(PermissionKind::Bash, "python3 other.py -v")));
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
        // …nor a wrapper that would carry it with privileges.
        assert!(!rules.allows(&request(PermissionKind::Bash, "sudo python3 script.py")));
    }

    #[test]
    fn an_exact_rule_matches_only_the_byte_identical_command() {
        let mut rules = PermissionRules::default();
        rules.remember(&request(PermissionKind::Bash, "cat a > b"));
        assert!(rules.exact.contains("cat a > b"));
        assert!(rules.allows(&request(PermissionKind::Bash, "cat a > b")));
        assert!(!rules.allows(&request(PermissionKind::Bash, "cat a > c")));
        assert!(
            !rules.allows(&request(PermissionKind::Bash, "cat a")),
            "an exact rule never doubles as a prefix"
        );
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
        assert!(gate.rules().prefixes.contains("cargo test"));
    }

    #[test]
    fn the_gates_mode_is_shared_and_gates_file_changes() {
        let gate = PermissionGate::new();
        assert_eq!(gate.mode(), PermissionMode::Manual);
        let edit = request(PermissionKind::Edit, "a.py");
        assert!(!gate.allows(&edit));
        gate.clone().set_mode(PermissionMode::Edit);
        assert_eq!(gate.mode(), PermissionMode::Edit);
        assert!(gate.allows(&edit), "edit mode auto-approves file changes");
        assert!(!gate.allows(&request(PermissionKind::Bash, "ls")));
        gate.set_mode(PermissionMode::Manual);
        assert!(!gate.allows(&edit), "manual mode asks again");
    }

    #[test]
    fn seeded_commands_land_in_the_right_allowlist() {
        let gate = PermissionGate::new();
        gate.seed_commands(
            ["python3".to_string(), "git status".to_string()],
            ["cat a > b".to_string()],
        );
        assert!(gate.allows(&request(PermissionKind::Bash, "python3 x.py")));
        assert!(gate.allows(&request(PermissionKind::Bash, "git status --short")));
        assert!(gate.allows(&request(PermissionKind::Bash, "cat a > b")));
        assert!(!gate.allows(&request(PermissionKind::Bash, "git push")));
    }

    // --- the permissions file (`~/.alter-zero/permissions.json`) ---

    #[test]
    fn the_permissions_file_parses_the_documented_shape() {
        let file = PermissionsFile::parse(
            r#"{
              "projects": {
                "/home/user/project": {
                  "allow_commands": ["python3 *", "git status *", "cat a > b"],
                  "mode": "edit"
                }
              }
            }"#,
        );
        let entry = file.project("/home/user/project").expect("the entry");
        assert_eq!(entry.saved_mode(), PermissionMode::Edit);
        let (prefixes, exact) = entry.command_sets();
        assert!(prefixes.contains("python3"), "a `… *` row is a prefix rule");
        assert!(prefixes.contains("git status"));
        assert!(exact.contains("cat a > b"), "no star = exact command");
        assert!(file.project("/elsewhere").is_none());
    }

    #[test]
    fn the_permissions_file_round_trips_the_live_rules() {
        let mut rules = PermissionRules::default();
        rules.remember(&request(PermissionKind::Bash, "python3 x.py"));
        rules.remember(&request(PermissionKind::Bash, "cat a > b"));
        rules.mode = PermissionMode::Edit;
        let mut file = PermissionsFile::default();
        file.record("/proj", &rules);
        let restored = PermissionsFile::parse(&file.to_json());
        let entry = restored.project("/proj").expect("the entry");
        assert_eq!(entry.saved_mode(), PermissionMode::Edit);
        assert_eq!(entry.command_sets(), (rules.prefixes, rules.exact));
    }

    #[test]
    fn recording_a_project_leaves_the_others_alone() {
        // Two instances in two directories share the file — a save from one
        // must not clobber the other's entry (read-modify-write).
        let mut file = PermissionsFile::parse(
            r#"{"projects":{"/other":{"allow_commands":["ls *"],"mode":"edit"}}}"#,
        );
        file.record("/mine", &PermissionRules::default());
        let json = file.to_json();
        let reread = PermissionsFile::parse(&json);
        let other = reread.project("/other").expect("the other project");
        assert_eq!(other.saved_mode(), PermissionMode::Edit);
        assert!(other.command_sets().0.contains("ls"));
        assert!(reread.project("/mine").is_some());
    }

    #[test]
    fn a_garbage_or_absent_permissions_file_is_the_empty_default() {
        assert_eq!(PermissionsFile::parse(""), PermissionsFile::default());
        assert_eq!(
            PermissionsFile::parse("not json"),
            PermissionsFile::default()
        );
        // A manual-mode, no-rules entry stays valid and quiet.
        let entry = ProjectPermissions::default();
        assert_eq!(entry.saved_mode(), PermissionMode::Manual);
        assert_eq!(entry.command_sets(), (BTreeSet::new(), BTreeSet::new()));
    }

    #[test]
    fn a_manual_mode_entry_omits_the_mode_key() {
        // Manual is the default posture — writing it out would just noise up
        // the file (and an old file without the key must read as manual).
        let entry = ProjectPermissions::from_rules(&PermissionRules::default());
        assert_eq!(entry.mode, None);
        let edit_rules = PermissionRules {
            mode: PermissionMode::Edit,
            ..PermissionRules::default()
        };
        assert_eq!(
            ProjectPermissions::from_rules(&edit_rules).mode.as_deref(),
            Some("edit")
        );
    }
}
