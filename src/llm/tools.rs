//! The tool-calling primitives — **pure and unit-tested** (see `docs/tools.md`).
//!
//! This module holds everything the real backend needs to offer the model the
//! `bash` / `read` / `write` / `edit` tools *except* the I/O itself:
//!
//! - the tool **definitions** ([`tool_specs`]) — the Chat Completions
//!   `{"type":"function", …}` entries sent in the request's `tools` array;
//! - the **argument** structs and their JSON parse ([`BashArgs`] etc.);
//! - the **edit engine** ([`apply_edit`]) — exact `old_string`→`new_string`
//!   replacement, Claude-Code's contract;
//! - the **read formatter** ([`format_read`]) — numbered lines in the same
//!   `{n:>W} {text}` gutter format as the write/edit bodies;
//! - a small line **diff** ([`diff_lines`]) driving the `(+A −D)` summaries and
//!   the diff-coloured cells;
//! - the header **summaries** ([`summarize_call`], [`display_name`]) and the
//!   model-facing **output framing**/**truncation** helpers.
//!
//! The boundary that actually runs a command and touches the filesystem is
//! [`crate::llm::exec`]; the loop that drives it is [`crate::llm::agent`].

use serde::Deserialize;
use serde_json::{Value, json};

/// The `bash` tool's default per-command timeout when the model omits one
/// (codex uses 10 s; we allow longer for build/test commands).
pub const BASH_DEFAULT_TIMEOUT_MS: u64 = 30_000;

/// The ceiling a model-supplied `bash` `timeout_ms` is clamped to.
pub const BASH_MAX_TIMEOUT_MS: u64 = 600_000;

/// The `read` tool's default line cap when the model omits `limit`.
pub const READ_DEFAULT_LIMIT: usize = 2_000;

/// How many bytes of a tool's output are retained and sent back to the model.
/// Bounds the token cost (and memory) of a chatty command; the dropped tail is
/// marked with a truncation notice. Mirrors codex's byte-capped exec output.
pub const TOOL_OUTPUT_MAX_BYTES: usize = 64 * 1024;

/// How many diff/body lines an `edit`/`write` cell's output shows before it is
/// summarised — keeps a large write from dumping the whole file back to the
/// model (and the TUI). The Ctrl+O view still shows what is retained.
pub const DIFF_MAX_LINES: usize = 400;

/// One tool call the model asked for, accumulated from the streamed
/// `tool_calls` deltas (see [`crate::llm::openai::ToolCallAccumulator`]).
/// `arguments` is the raw JSON string the model emitted — parsed per-tool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCallRequest {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

/// The result of executing one tool: the `output` sent back to the model, the
/// `ok` outcome (green vs red cell), and whether the output was `truncated` at
/// the byte cap. Mirrors the `StreamEvent::ToolEnd` payload the loop emits —
/// except a `background: Some(id)` outcome, which the agent loop surfaces as
/// `StreamEvent::ToolBackgrounded` instead (see `docs/background.md`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolOutcome {
    pub output: String,
    pub ok: bool,
    pub truncated: bool,
    /// `Some(task_id)` when the call resolved by moving to the background
    /// (`run_in_background`, or a Ctrl+B handoff): `output` then holds the
    /// model-facing launch text, and the cell renders the fixed backgrounded
    /// row. `None` for every normal outcome.
    pub background: Option<String>,
    /// `Some(data: URL)` when the tool read an image (the `read` tool on a
    /// png/jpg/jpeg/gif/webp): the pixels the agent loop attaches as a
    /// follow-up **user** message — never part of `output`, which stays the
    /// small human/model-readable text (byte-capped, cell-displayed,
    /// token-counted, session-recorded). See `docs/tools.md`.
    pub image: Option<String>,
}

impl ToolOutcome {
    /// A successful (green) result.
    #[must_use]
    pub fn ok(output: impl Into<String>) -> Self {
        Self {
            output: output.into(),
            ok: true,
            truncated: false,
            background: None,
            image: None,
        }
    }

    /// A failed (red) result — the message is what the model sees, so it can
    /// recover (codex's `RespondToModel`).
    #[must_use]
    pub fn error(output: impl Into<String>) -> Self {
        Self {
            output: output.into(),
            ok: false,
            truncated: false,
            background: None,
            image: None,
        }
    }

    /// A call resolved by moving to the background: `output` is the
    /// model-facing launch text (task id + interim-output path). See
    /// `docs/background.md`.
    #[must_use]
    pub fn backgrounded(id: impl Into<String>, output: impl Into<String>) -> Self {
        Self {
            output: output.into(),
            ok: true,
            truncated: false,
            background: Some(id.into()),
            image: None,
        }
    }

    /// Mark the output as truncated at the byte cap.
    #[must_use]
    pub fn with_truncated(mut self, truncated: bool) -> Self {
        self.truncated = truncated;
        self
    }

    /// Attach an image's `data:` URL (the `read` tool's image branch).
    #[must_use]
    pub fn with_image(mut self, url: impl Into<String>) -> Self {
        self.image = Some(url.into());
        self
    }
}

/// The four tool definitions sent to the model, as Chat Completions
/// `{"type":"function","function":{name,description,parameters}}` entries. The
/// descriptions are prompt-engineering (ported/adapted from codex + Claude
/// Code) and the parameter schemas are the Structured-Outputs JSON-Schema
/// subset every OpenAI-compatible provider accepts.
#[must_use]
pub fn tool_specs() -> Vec<Value> {
    vec![bash_spec(), read_spec(), write_spec(), edit_spec()]
}

/// [`tool_specs`] plus the `agent` tool — the **main** backend's set when a
/// subagent registry is attached (`docs/agent-tool.md`). A subagent itself
/// never gets `agent` (no nesting), so it keeps [`tool_specs`] — or the
/// reduced [`subagent_tool_specs`] its type allows.
#[must_use]
pub fn tool_specs_with_agents() -> Vec<Value> {
    let mut specs = tool_specs();
    specs.push(agent_spec());
    specs
}

/// The tool set a subagent of `agent_type` is offered: `explore` is
/// read-only (`bash` + `read` — the reference's read-only search agent),
/// everything else (the default `general-purpose`) gets all four. Never
/// includes `agent`.
#[must_use]
pub fn subagent_tool_specs(agent_type: &str) -> Vec<Value> {
    match agent_type {
        "explore" => vec![bash_spec(), read_spec()],
        _ => tool_specs(),
    }
}

/// The tool names offered, in definition order — handy for the system prompt
/// and tests.
pub const TOOL_NAMES: [&str; 4] = ["bash", "read", "write", "edit"];

/// The wire name of the subagent-launching tool (`docs/agent-tool.md`).
pub const AGENT_TOOL_NAME: &str = "agent";

fn agent_spec() -> Value {
    function_spec(
        AGENT_TOOL_NAME,
        "Launch a new agent to handle a task autonomously. The agent runs its \
         own tool loop (shell, file reads/writes) over a fresh context, works \
         in the same directory, and reports back: its final message is \
         returned to you as this tool's result. Launch several agents in one \
         message to run them concurrently — each is independent and cannot \
         see the others (or this conversation), so give each a complete, \
         self-contained prompt and tell it what to return. By default agents \
         run in the background: the call returns at once with an agent ID and \
         you are notified with the final response when one completes — set \
         run_in_background to false when you need the result before \
         continuing. The user can watch, stop, or message your agents while \
         they run.",
        json!({
            "type": "object",
            "properties": {
                "description": {
                    "type": "string",
                    "description": "A short (3-5 word) description of the task, \
                        shown in the UI."
                },
                "prompt": {
                    "type": "string",
                    "description": "The task for the agent to perform — \
                        complete and self-contained, including what to return."
                },
                "subagent_type": {
                    "type": "string",
                    "description": "The type of specialized agent to use: \
                        \"general-purpose\" (default — all tools) or \
                        \"explore\" (read-only: shell and file reads, for \
                        searching and research)."
                },
                "run_in_background": {
                    "type": "boolean",
                    "description": "Agents run in the background by default; \
                        you will be notified when one completes. Set to false \
                        to run this agent synchronously when you need its \
                        result before continuing."
                }
            },
            "required": ["description", "prompt"],
            "additionalProperties": false
        }),
    )
}

fn function_spec(name: &str, description: &str, parameters: Value) -> Value {
    json!({
        "type": "function",
        "function": {
            "name": name,
            "description": description,
            "parameters": parameters,
        }
    })
}

fn bash_spec() -> Value {
    function_spec(
        "bash",
        "Run a shell command with `sh -c` in the current working directory and \
         return its combined stdout and stderr. Use this for exploring the \
         project (ls, grep, find, cat), running builds and tests, and git. \
         Prefer the `read` tool over `cat` when you want to inspect a file to \
         edit it. Long output is truncated; a non-zero exit status is reported. \
         Set `run_in_background` for long-running commands: the call returns \
         immediately with a task ID and an interim-output file path, and you \
         are notified with the final output when the command completes. \
         The user may also move a running command to the background \
         themselves mid-run: the tool result then says so and reports the \
         same task ID and notification promise — do not run the command again \
         or wait for it, just continue.",
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The shell command to run."
                },
                "timeout_ms": {
                    "type": "number",
                    "description": "Maximum runtime in milliseconds before the \
                        command is killed. Defaults to 30000; capped at 600000. \
                        Ignored when run_in_background is true."
                },
                "run_in_background": {
                    "type": "boolean",
                    "description": "Set to true to run this command in the \
                        background: the tool returns at once with a task ID \
                        while the command keeps running, and you receive a \
                        notification with the final output when it completes. \
                        Use for long-running commands (servers, watchers, \
                        long benchmarks); read the reported interim-output \
                        file to check progress mid-run. Defaults to false."
                },
                "description": {
                    "type": "string",
                    "description": "A short human-readable description of what \
                        the command does (e.g. \"Ping google.com 200 times\"), \
                        shown in the UI and in background-completion \
                        notifications."
                }
            },
            "required": ["command"],
            "additionalProperties": false
        }),
    )
}

fn read_spec() -> Value {
    function_spec(
        "read",
        "Read a file from the filesystem. A text file returns its contents with \
         1-based line numbers (like `cat -n`), so you can cite exact lines to the \
         `edit` tool — up to 2000 lines by default; use `offset`/`limit` to page \
         through a large file. An image file (png/jpg/jpeg/gif/webp) is returned \
         visually: the image is attached to the conversation so you can see it \
         (`offset`/`limit` are ignored for images).",
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file, absolute or relative to the \
                        working directory."
                },
                "offset": {
                    "type": "number",
                    "description": "1-based line number to start reading from. \
                        Omit to start at the beginning."
                },
                "limit": {
                    "type": "number",
                    "description": "Maximum number of lines to read. Defaults to 2000."
                }
            },
            "required": ["path"],
            "additionalProperties": false
        }),
    )
}

fn write_spec() -> Value {
    function_spec(
        "write",
        "Write text to a file, creating it (and any missing parent directories) \
         or overwriting it entirely. Prefer the `edit` tool for changing part of \
         an existing file; use `write` for new files or full rewrites.",
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file to write."
                },
                "content": {
                    "type": "string",
                    "description": "The full contents to write to the file."
                }
            },
            "required": ["path", "content"],
            "additionalProperties": false
        }),
    )
}

fn edit_spec() -> Value {
    function_spec(
        "edit",
        "Replace an exact substring in a file. `old_string` must appear exactly \
         once (include enough surrounding context to make it unique) unless \
         `replace_all` is true. Read the file first so `old_string` matches the \
         current contents verbatim, including whitespace and indentation.",
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file to edit."
                },
                "old_string": {
                    "type": "string",
                    "description": "The exact text to replace."
                },
                "new_string": {
                    "type": "string",
                    "description": "The text to replace it with."
                },
                "replace_all": {
                    "type": "boolean",
                    "description": "Replace every occurrence instead of requiring \
                        a unique match. Defaults to false."
                }
            },
            "required": ["path", "old_string", "new_string"],
            "additionalProperties": false
        }),
    )
}

/// Parsed `agent` arguments (`docs/agent-tool.md`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AgentArgs {
    pub description: String,
    pub prompt: String,
    /// The specialized type (default `general-purpose`).
    #[serde(default)]
    pub subagent_type: Option<String>,
    /// The schema default is **true** — background launch.
    #[serde(default)]
    pub run_in_background: Option<bool>,
}

impl AgentArgs {
    /// The effective subagent type.
    #[must_use]
    pub fn agent_type(&self) -> &str {
        self.subagent_type
            .as_deref()
            .filter(|t| !t.trim().is_empty())
            .unwrap_or(crate::agents::GENERAL_PURPOSE)
    }

    /// The effective background flag (the schema default is `true`).
    #[must_use]
    pub fn background(&self) -> bool {
        self.run_in_background.unwrap_or(true)
    }
}

/// Parsed `bash` arguments.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct BashArgs {
    pub command: String,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    /// Run the command as a background task (`docs/background.md`).
    #[serde(default)]
    pub run_in_background: bool,
    /// A short human description, shown in the UI and completion notices.
    #[serde(default)]
    pub description: Option<String>,
}

impl BashArgs {
    /// The effective timeout in milliseconds, clamped to [`BASH_MAX_TIMEOUT_MS`].
    #[must_use]
    pub fn timeout_ms(&self) -> u64 {
        self.timeout_ms
            .unwrap_or(BASH_DEFAULT_TIMEOUT_MS)
            .clamp(1, BASH_MAX_TIMEOUT_MS)
    }
}

/// Parsed `read` arguments.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ReadArgs {
    pub path: String,
    #[serde(default)]
    pub offset: Option<usize>,
    #[serde(default)]
    pub limit: Option<usize>,
}

/// Parsed `write` arguments.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct WriteArgs {
    pub path: String,
    pub content: String,
}

/// Parsed `edit` arguments.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct EditArgs {
    pub path: String,
    pub old_string: String,
    pub new_string: String,
    #[serde(default)]
    pub replace_all: bool,
}

/// Parse a tool's raw JSON `arguments` string into a typed struct, mapping a
/// serde error to a short model-facing message (the model retries on it).
///
/// # Errors
/// Returns a human-readable message when the arguments aren't valid JSON or are
/// missing a required field.
pub fn parse_args<T: for<'de> Deserialize<'de>>(arguments: &str) -> Result<T, String> {
    let trimmed = arguments.trim();
    // A tool with no arguments sometimes arrives as "" — treat it as "{}".
    let text = if trimmed.is_empty() { "{}" } else { trimmed };
    serde_json::from_str(text).map_err(|e| format!("invalid tool arguments: {e}"))
}

/// The display name shown in the `● name(args)` cell header for a model tool
/// (title-cased), falling back to the raw name for anything unrecognised.
#[must_use]
pub fn display_name(name: &str) -> String {
    match name {
        "bash" => "Bash".to_string(),
        "read" => "Read".to_string(),
        "write" => "Write".to_string(),
        "edit" => "Edit".to_string(),
        AGENT_TOOL_NAME => "Agent".to_string(),
        other => other.to_string(),
    }
}

/// The call's model-supplied human description, when its arguments carry one
/// — a `bash` call's optional `description` argument (an `agent` call's
/// `description` already *is* its summary). `None` for everything else. The
/// agent roster's tree rows prefer it for their `{Name}: {detail}` activity
/// line (`docs/agent-tool.md`).
#[must_use]
pub fn call_description(name: &str, arguments: &str) -> Option<String> {
    if name != "bash" {
        return None;
    }
    let value: Value = serde_json::from_str(arguments.trim()).ok()?;
    value
        .get("description")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|d| !d.is_empty())
        .map(str::to_string)
}

/// A short one-line summary of a tool call for the cell header's `(args)` —
/// the command for `bash`, the path for the file tools. Falls back to a
/// flattened slice of the raw arguments when they don't parse.
#[must_use]
pub fn summarize_call(name: &str, arguments: &str) -> String {
    let value: Option<Value> = serde_json::from_str(arguments.trim()).ok();
    let field = |key: &str| -> Option<String> {
        value
            .as_ref()
            .and_then(|v| v.get(key))
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    let summary = match name {
        "bash" => field("command"),
        "read" | "write" | "edit" => field("path"),
        AGENT_TOOL_NAME => field("description"),
        _ => None,
    };
    let summary = summary.unwrap_or_else(|| arguments.trim().to_string());
    flatten_one_line(&summary)
}

/// Collapse a possibly multi-line string to a single spaced line (for a header).
fn flatten_one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The outcome of an [`apply_edit`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditResult {
    /// The whole new file contents.
    pub new_content: String,
    /// How many occurrences were replaced.
    pub replacements: usize,
}

/// Why an [`apply_edit`] could not be applied — each carries a model-facing
/// message via [`std::fmt::Display`] so the tool result explains the failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditError {
    /// `old_string` was empty (nothing to find).
    EmptyOldString,
    /// `old_string` == `new_string` — the edit is a no-op.
    NoChange,
    /// `old_string` did not appear in the file.
    NotFound,
    /// `old_string` appeared `count` times but `replace_all` was not set.
    NotUnique(usize),
}

impl std::fmt::Display for EditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyOldString => write!(f, "old_string must not be empty"),
            Self::NoChange => write!(
                f,
                "old_string and new_string are identical — no change to make"
            ),
            Self::NotFound => write!(
                f,
                "old_string was not found in the file — read the file and copy the exact text (including whitespace)"
            ),
            Self::NotUnique(count) => write!(
                f,
                "old_string appears {count} times — add surrounding context to make it unique, or set replace_all to true"
            ),
        }
    }
}

/// Apply an exact-substring edit to `content`: replace `old` with `new`,
/// requiring a unique match unless `replace_all`. The pure core of the `edit`
/// tool (Claude-Code's contract).
///
/// # Errors
/// [`EditError`] when `old` is empty, equals `new`, is absent, or is ambiguous.
pub fn apply_edit(
    content: &str,
    old: &str,
    new: &str,
    replace_all: bool,
) -> Result<EditResult, EditError> {
    if old.is_empty() {
        return Err(EditError::EmptyOldString);
    }
    if old == new {
        return Err(EditError::NoChange);
    }
    let mut old = std::borrow::Cow::Borrowed(old);
    let mut new = std::borrow::Cow::Borrowed(new);
    let mut count = content.matches(old.as_ref()).count();
    // The `read` tool shows CRLF files with the \r stripped (str::lines), so a
    // faithfully-copied multi-line old_string arrives \n-joined and can never
    // match the raw file across a line boundary. When the exact match misses
    // on a CRLF file, retry with both strings normalized to \r\n endings —
    // otherwise the documented read-then-edit workflow fails deterministically.
    if count == 0 && content.contains("\r\n") && old.contains('\n') && !old.contains('\r') {
        let old_crlf = old.replace('\n', "\r\n");
        let crlf_count = content.matches(&old_crlf).count();
        if crlf_count > 0 {
            old = std::borrow::Cow::Owned(old_crlf);
            if !new.contains('\r') {
                new = std::borrow::Cow::Owned(new.replace('\n', "\r\n"));
            }
            count = crlf_count;
        }
    }
    let (old, new) = (old.as_ref(), new.as_ref());
    match count {
        0 => Err(EditError::NotFound),
        n if n > 1 && !replace_all => Err(EditError::NotUnique(n)),
        _ => {
            let (new_content, replacements) = if replace_all {
                (content.replace(old, new), count)
            } else {
                (content.replacen(old, new, 1), 1)
            };
            Ok(EditResult {
                new_content,
                replacements,
            })
        }
    }
}

/// Format a file's contents as numbered lines for the `read` tool, honouring a
/// 1-based `offset` and a line `limit`. Each line is `{n:>W} {text}` — the
/// number right-aligned to the widest one shown, a single space, then the text
/// — the **same** gutter format as the `write`/`edit` bodies
/// ([`render_numbered_content`]/[`render_numbered_diff`]). So the model reads
/// consistent numbering it can cite back to `edit`, and the TUI renders `read`
/// and `write`/`edit` as one numbered, syntax-highlighted cell
/// (`ui::file_cell_lines`). Returns a placeholder note when the range is empty
/// (offset past EOF).
#[must_use]
pub fn format_read(content: &str, offset: Option<usize>, limit: Option<usize>) -> String {
    let start = offset.unwrap_or(1).max(1); // 1-based
    let limit = limit.unwrap_or(READ_DEFAULT_LIMIT);
    // `lines()` drops a single trailing newline, which is what we want (an empty
    // final line isn't a real line to number).
    let all: Vec<&str> = content.lines().collect();
    if start > all.len() {
        return format!(
            "(file has {} line{}; offset {start} is past the end)",
            all.len(),
            if all.len() == 1 { "" } else { "s" }
        );
    }
    let end = start.saturating_add(limit).min(all.len() + 1);
    // Right-align every number to the last (largest) one shown, so the gutter
    // is exactly as wide as it needs to be — the same rule as the write/edit
    // body, and what lets the two render identically.
    let width = (end - 1).max(1).to_string().len();
    let mut out = String::new();
    for (i, line) in all[start - 1..end - 1].iter().enumerate() {
        let n = start + i;
        out.push_str(&format!("{n:>width$} {line}\n"));
    }
    // Drop the trailing newline so the block ends cleanly.
    if out.ends_with('\n') {
        out.pop();
    }
    out
}

/// The raw-byte ceiling for an image `read` — 3.75 MB, so the base64 form
/// (4/3 inflation) stays under the strictest mainstream provider's 5 MB
/// per-image limit. Claude Code's Read uses the same bound. Past it the read
/// fails with a recoverable message telling the model to downscale first.
pub const READ_IMAGE_MAX_BYTES: usize = 3 * 1024 * 1024 + 768 * 1024;

/// The output head that marks an image read ([`format_read_image`]).
/// [`is_image_read_output`] keys on it when the context replay reconstructs
/// the attachment for later turns (`crate::context`). A text read can never
/// collide: its output starts with a numbered gutter row, `(file …`, or a
/// `could not read …` error.
const READ_IMAGE_HEAD: &str = "Read image ";

/// Does this path name an image the `read` tool should return visually?
/// Extension-keyed (case-insensitive) over the four formats every
/// vision-capable OpenAI-compatible endpoint accepts. Pure — the executor
/// sniffs the actual bytes before attaching (`llm::exec`).
#[must_use]
pub fn is_image_path(path: &str) -> bool {
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    matches!(
        ext.as_deref(),
        Some("png" | "jpg" | "jpeg" | "gif" | "webp")
    )
}

/// The model-facing (and cell-displayed) output of an image `read`: the
/// [`READ_IMAGE_HEAD`] marker with the path, then the sniffed format, the
/// pixel dimensions, and the humanized byte size — plus where the pixels are
/// (the attachment note the agent loop appends; see `docs/tools.md`).
#[must_use]
pub fn format_read_image(
    path: &str,
    format: &str,
    width: u32,
    height: u32,
    bytes: usize,
) -> String {
    format!(
        "{READ_IMAGE_HEAD}{path} ({format}, {width}x{height}, {size})\n\
         The image is attached as the next user message.",
        size = human_size(bytes),
    )
}

/// Was this `read` output an image read ([`format_read_image`])? The context
/// replay uses it to reconstruct the follow-up attachment message.
#[must_use]
pub fn is_image_read_output(output: &str) -> bool {
    output.starts_with(READ_IMAGE_HEAD)
}

/// The text of the user-role message that carries an image read's pixels —
/// bracket-prefixed like the other injected notes (`[error]`/`[background]`),
/// naming the path so the model pairs it to the tool result above. Shared by
/// the live agent loop and the context replay so both turns' wire shapes match.
#[must_use]
pub fn image_attachment_note(path: &str) -> String {
    format!(
        "[image] The image file {path} from the read tool call above is attached to this message."
    )
}

/// `512 B` / `240 KB` / `2.5 MB` — the size clause of [`format_read_image`].
fn human_size(bytes: usize) -> String {
    const KB: usize = 1024;
    const MB: usize = 1024 * 1024;
    if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{} KB", bytes / KB)
    } else {
        format!("{bytes} B")
    }
}

/// One line of a [`diff_lines`] result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffLine {
    /// Unchanged (shown for context).
    Context(String),
    /// Added (`+`, green).
    Add(String),
    /// Removed (`-`, red).
    Remove(String),
}

/// A line-level diff of two texts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diff {
    pub lines: Vec<DiffLine>,
    pub added: usize,
    pub removed: usize,
}

/// The most LCS-table cells [`diff_lines`] will allocate (~8 MB of `usize`s).
/// The common prefix/suffix are trimmed first, so a typical edit — however
/// large the file — only needs the table for its changed middle; a middle
/// whose old×new line product exceeds this budget is rendered as a plain
/// remove/add block instead. Without the bound, an `edit`/`write` touching a
/// 30k-line file (a lockfile, a generated bundle) allocates an O(n×m) table
/// in the gigabytes and can OOM-abort the whole TUI.
const DIFF_LCS_MAX_CELLS: usize = 1_000_000;

/// Compute a line-level diff of `old` → `new` via a longest-common-subsequence
/// match, so unchanged lines are shared context and only the real changes are
/// marked `+`/`-`. Pure; drives the `(+A −D)` summaries and the diff cells.
/// Memory-bounded: the shared prefix/suffix never enter the LCS table, and a
/// changed middle past [`DIFF_LCS_MAX_CELLS`] falls back to remove-all/add-all.
#[must_use]
pub fn diff_lines(old: &str, new: &str) -> Diff {
    let a: Vec<&str> = if old.is_empty() {
        Vec::new()
    } else {
        old.lines().collect()
    };
    let b: Vec<&str> = if new.is_empty() {
        Vec::new()
    } else {
        new.lines().collect()
    };
    // Trim the common prefix and suffix — context that never needs the table.
    let prefix = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let suffix = a[prefix..]
        .iter()
        .rev()
        .zip(b[prefix..].iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let mut lines: Vec<DiffLine> = a[..prefix]
        .iter()
        .map(|l| DiffLine::Context(l.to_string()))
        .collect();
    let am = &a[prefix..a.len() - suffix];
    let bm = &b[prefix..b.len() - suffix];
    let (mut added, mut removed) = (0usize, 0usize);
    if am.len().saturating_mul(bm.len()) > DIFF_LCS_MAX_CELLS {
        // Past the budget: a plain replacement block, no shared-context search.
        for l in am {
            lines.push(DiffLine::Remove(l.to_string()));
            removed += 1;
        }
        for l in bm {
            lines.push(DiffLine::Add(l.to_string()));
            added += 1;
        }
    } else {
        // LCS table (lengths) over the changed middle only.
        let (n, m) = (am.len(), bm.len());
        let mut lcs = vec![vec![0usize; m + 1]; n + 1];
        for i in (0..n).rev() {
            for j in (0..m).rev() {
                lcs[i][j] = if am[i] == bm[j] {
                    lcs[i + 1][j + 1] + 1
                } else {
                    lcs[i + 1][j].max(lcs[i][j + 1])
                };
            }
        }
        // Walk the table to reconstruct the diff.
        let (mut i, mut j) = (0usize, 0usize);
        while i < n && j < m {
            if am[i] == bm[j] {
                lines.push(DiffLine::Context(am[i].to_string()));
                i += 1;
                j += 1;
            } else if lcs[i + 1][j] >= lcs[i][j + 1] {
                lines.push(DiffLine::Remove(am[i].to_string()));
                removed += 1;
                i += 1;
            } else {
                lines.push(DiffLine::Add(bm[j].to_string()));
                added += 1;
                j += 1;
            }
        }
        while i < n {
            lines.push(DiffLine::Remove(am[i].to_string()));
            removed += 1;
            i += 1;
        }
        while j < m {
            lines.push(DiffLine::Add(bm[j].to_string()));
            added += 1;
            j += 1;
        }
    }
    lines.extend(
        a[a.len() - suffix..]
            .iter()
            .map(|l| DiffLine::Context(l.to_string())),
    );
    Diff {
        lines,
        added,
        removed,
    }
}

/// The `(+A −D)` count summary string for a diff.
#[must_use]
pub fn diff_summary(added: usize, removed: usize) -> String {
    format!("(+{added} -{removed})")
}

/// How many unchanged context lines a numbered diff body shows around each
/// change run (git's and codex's default), the runs merged into one hunk when
/// their context ranges touch.
pub const DIFF_CONTEXT_LINES: usize = 3;

/// Render numbered gutter rows — `(line number, rest)` pairs, a `None` number
/// for the `⋮` hunk-gap row — right-aligning every number to the widest one
/// shown, capped at [`DIFF_MAX_LINES`] rows with a `… N more lines` tail note.
/// The shared body format of [`render_numbered_content`] (`{n:>W} {text}`) and
/// [`render_numbered_diff`] (`{n:>W} {sign}{text}`): what the model reads back
/// *and* what the TUI re-styles into the codex-look cell (`ui::tool_lines`).
fn render_gutter_rows(rows: &[(Option<usize>, String)]) -> String {
    let shown = &rows[..rows.len().min(DIFF_MAX_LINES)];
    let width = shown
        .iter()
        .filter_map(|(no, _)| *no)
        .max()
        .unwrap_or(1)
        .to_string()
        .len();
    let mut out = String::new();
    for (no, rest) in shown {
        match no {
            Some(n) => out.push_str(&format!("{n:>width$} {rest}\n")),
            None => out.push_str(&format!("{:>width$} {rest}\n", "")),
        }
    }
    let hidden = rows[shown.len()..]
        .iter()
        .filter(|(no, _)| no.is_some())
        .count();
    if hidden > 0 {
        out.push_str(&format!("… {hidden} more lines\n"));
    }
    if out.ends_with('\n') {
        out.pop();
    }
    out
}

/// Render a brand-new file's contents as numbered lines (`{n:>W} {text}`,
/// Claude-Code's `Write` preview) — the body under the `Created …` head.
/// Capped like the diff body; empty content renders as an empty string.
#[must_use]
pub fn render_numbered_content(content: &str) -> String {
    let rows: Vec<(Option<usize>, String)> = content
        .lines()
        .enumerate()
        .map(|(i, line)| (Some(i + 1), line.to_string()))
        .collect();
    render_gutter_rows(&rows)
}

/// Render a diff as numbered hunks (codex's `diff_render` body): only the
/// change runs plus [`DIFF_CONTEXT_LINES`] of context each side, touching runs
/// merged, distant hunks separated by a `⋮` gap row. Each row is
/// `{n:>W} {sign}{text}` — sign `+`/`-`/space — context and added lines
/// numbered by the **new** file, removed lines by the **old** one. This is the
/// `output` sent back to the model *and* shown in the cell (the TUI restyles
/// the rows — see `ui.rs`).
#[must_use]
pub fn render_numbered_diff(diff: &Diff) -> String {
    // Number every diff line first; remember which are changes.
    let mut numbered: Vec<(usize, char, &str, bool)> = Vec::with_capacity(diff.lines.len());
    let (mut old_no, mut new_no) = (1usize, 1usize);
    for line in &diff.lines {
        match line {
            DiffLine::Context(t) => {
                numbered.push((new_no, ' ', t, false));
                old_no += 1;
                new_no += 1;
            }
            DiffLine::Add(t) => {
                numbered.push((new_no, '+', t, true));
                new_no += 1;
            }
            DiffLine::Remove(t) => {
                numbered.push((old_no, '-', t, true));
                old_no += 1;
            }
        }
    }
    // Hunks: each change's context range, merged while the ranges touch.
    let mut hunks: Vec<(usize, usize)> = Vec::new();
    for (i, &(_, _, _, changed)) in numbered.iter().enumerate() {
        if !changed {
            continue;
        }
        let start = i.saturating_sub(DIFF_CONTEXT_LINES);
        let end = (i + DIFF_CONTEXT_LINES).min(numbered.len() - 1);
        match hunks.last_mut() {
            Some((_, last_end)) if start <= *last_end + 1 => *last_end = (*last_end).max(end),
            _ => hunks.push((start, end)),
        }
    }
    let mut rows: Vec<(Option<usize>, String)> = Vec::new();
    for (h, &(start, end)) in hunks.iter().enumerate() {
        if h > 0 {
            rows.push((None, "⋮".to_string()));
        }
        for &(no, sign, text, _) in &numbered[start..=end] {
            rows.push((Some(no), format!("{sign}{text}")));
        }
    }
    render_gutter_rows(&rows)
}

/// Frame a `bash` command's captured output for the model the way codex does:
/// an `Exit code: N` line, then the (already-truncated) output. A zero exit
/// with empty output reports `(no output)`.
#[must_use]
pub fn format_exec_output(exit_code: Option<i32>, output: &str) -> String {
    let code = match exit_code {
        Some(c) => c.to_string(),
        None => "killed by signal".to_string(),
    };
    let body = if output.trim().is_empty() {
        "(no output)"
    } else {
        output
    };
    format!("Exit code: {code}\n{body}")
}

/// Truncate `s` to at most `max_bytes` bytes on a char boundary, returning the
/// retained head and whether anything was dropped. Used to bound a tool's
/// output before it goes back to the model.
#[must_use]
pub fn truncate_output(s: &str, max_bytes: usize) -> (String, bool) {
    if s.len() <= max_bytes {
        return (s.to_string(), false);
    }
    let mut cut = max_bytes;
    while cut > 0 && !s.is_char_boundary(cut) {
        cut -= 1;
    }
    (s[..cut].to_string(), true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_specs_offers_the_four_named_tools() {
        let specs = tool_specs();
        let names: Vec<&str> = specs
            .iter()
            .map(|s| s["function"]["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, TOOL_NAMES);
    }

    #[test]
    fn every_spec_is_a_function_with_a_description_and_object_schema() {
        for spec in tool_specs() {
            assert_eq!(spec["type"], "function");
            assert!(spec["function"]["description"].as_str().unwrap().len() > 20);
            assert_eq!(spec["function"]["parameters"]["type"], "object");
            // Structured-outputs friendly: additionalProperties is false.
            assert_eq!(
                spec["function"]["parameters"]["additionalProperties"],
                Value::Bool(false)
            );
        }
    }

    #[test]
    fn bash_description_warns_the_user_may_background_a_running_command() {
        // The model should know IN ADVANCE that the user can move its
        // foreground command to the background mid-run (Ctrl+B): the tool
        // result then says so, and the model must not run the command again
        // (docs/background.md).
        let specs = tool_specs();
        let desc = specs[0]["function"]["description"].as_str().unwrap();
        assert!(
            desc.contains("The user may also move a running command to the background"),
            "got {desc}"
        );
        assert!(desc.contains("do not run the command again"), "got {desc}");
    }

    #[test]
    fn bash_requires_command_and_edit_requires_the_three_strings() {
        let specs = tool_specs();
        let bash = &specs[0]["function"]["parameters"]["required"];
        assert_eq!(*bash, json!(["command"]));
        let edit = &specs[3]["function"]["parameters"]["required"];
        assert_eq!(*edit, json!(["path", "old_string", "new_string"]));
    }

    #[test]
    fn bash_args_parse_and_clamp_the_timeout() {
        let a: BashArgs = parse_args(r#"{"command":"ls -la"}"#).unwrap();
        assert_eq!(a.command, "ls -la");
        assert_eq!(a.timeout_ms(), BASH_DEFAULT_TIMEOUT_MS);
        let b: BashArgs = parse_args(r#"{"command":"x","timeout_ms":5000}"#).unwrap();
        assert_eq!(b.timeout_ms(), 5000);
        let c: BashArgs = parse_args(r#"{"command":"x","timeout_ms":9999999}"#).unwrap();
        assert_eq!(c.timeout_ms(), BASH_MAX_TIMEOUT_MS, "clamped to the cap");
    }

    #[test]
    fn parse_args_treats_empty_arguments_as_an_empty_object() {
        // A no-arg call sometimes streams as "" — must not be a hard error for
        // a tool whose fields are all optional.
        #[derive(Deserialize)]
        struct Empty {}
        assert!(parse_args::<Empty>("").is_ok());
    }

    #[test]
    fn parse_args_reports_a_missing_required_field() {
        let err = parse_args::<BashArgs>("{}").unwrap_err();
        assert!(err.contains("invalid tool arguments"), "got {err}");
    }

    #[test]
    fn display_name_title_cases_the_known_tools() {
        assert_eq!(display_name("bash"), "Bash");
        assert_eq!(display_name("read"), "Read");
        assert_eq!(display_name("write"), "Write");
        assert_eq!(display_name("edit"), "Edit");
        assert_eq!(display_name("mystery"), "mystery");
    }

    #[test]
    fn summarize_call_shows_the_command_or_path() {
        assert_eq!(
            summarize_call("bash", r#"{"command":"cargo test"}"#),
            "cargo test"
        );
        assert_eq!(
            summarize_call("read", r#"{"path":"src/app.rs","offset":10}"#),
            "src/app.rs"
        );
        assert_eq!(
            summarize_call("edit", r#"{"path":"a/b.rs","old_string":"x"}"#),
            "a/b.rs"
        );
    }

    #[test]
    fn summarize_call_flattens_a_multiline_command() {
        assert_eq!(
            summarize_call("bash", "{\"command\":\"echo one\\n echo two\"}"),
            "echo one echo two"
        );
    }

    #[test]
    fn summarize_call_falls_back_to_raw_args_when_unparseable() {
        assert_eq!(summarize_call("bash", "not json"), "not json");
    }

    #[test]
    fn apply_edit_replaces_a_unique_occurrence() {
        let out = apply_edit(
            "let x = 1;\nlet y = 2;\n",
            "let x = 1;",
            "let x = 3;",
            false,
        )
        .unwrap();
        assert_eq!(out.new_content, "let x = 3;\nlet y = 2;\n");
        assert_eq!(out.replacements, 1);
    }

    #[test]
    fn apply_edit_rejects_an_absent_old_string() {
        assert_eq!(
            apply_edit("abc", "zzz", "y", false),
            Err(EditError::NotFound)
        );
    }

    #[test]
    fn apply_edit_rejects_a_non_unique_match_without_replace_all() {
        assert_eq!(
            apply_edit("a\na\na", "a", "b", false),
            Err(EditError::NotUnique(3))
        );
    }

    #[test]
    fn apply_edit_replace_all_changes_every_occurrence() {
        let out = apply_edit("a\na\na", "a", "b", true).unwrap();
        assert_eq!(out.new_content, "b\nb\nb");
        assert_eq!(out.replacements, 3);
    }

    #[test]
    fn edit_matches_lf_old_string_against_a_crlf_file() {
        // `read` shows CRLF files with the \r stripped (str::lines), so a
        // faithfully-copied old_string arrives \n-joined; the edit must still
        // land, and the file must stay CRLF.
        let out = apply_edit(
            "alpha\r\nbeta\r\ngamma\r\n",
            "alpha\nbeta",
            "alpha\nBETA",
            false,
        )
        .unwrap();
        assert_eq!(out.new_content, "alpha\r\nBETA\r\ngamma\r\n");
        assert_eq!(out.replacements, 1);
    }

    #[test]
    fn crlf_fallback_counts_uniqueness_over_the_normalized_form() {
        assert_eq!(
            apply_edit("x\r\ny\r\nx\r\ny\r\n", "x\ny", "x\nY", false),
            Err(EditError::NotUnique(2))
        );
    }

    #[test]
    fn crlf_fallback_replace_all_changes_every_occurrence() {
        let out = apply_edit("x\r\ny\r\nx\r\ny\r\n", "x\ny", "x\nY", true).unwrap();
        assert_eq!(out.new_content, "x\r\nY\r\nx\r\nY\r\n");
        assert_eq!(out.replacements, 2);
    }

    #[test]
    fn an_exact_match_wins_over_the_crlf_fallback() {
        // Mixed-endings file: the literal \n match exists, so it is taken
        // as-is and no normalization happens.
        let out = apply_edit("a\nb--a\r\nb", "a\nb", "a\nB", false).unwrap();
        assert_eq!(out.new_content, "a\nB--a\r\nb");
    }

    #[test]
    fn apply_edit_rejects_empty_and_noop() {
        assert_eq!(
            apply_edit("abc", "", "x", false),
            Err(EditError::EmptyOldString)
        );
        assert_eq!(apply_edit("abc", "a", "a", false), Err(EditError::NoChange));
    }

    #[test]
    fn edit_error_messages_are_actionable() {
        assert!(EditError::NotFound.to_string().contains("read the file"));
        assert!(EditError::NotUnique(2).to_string().contains("2 times"));
    }

    #[test]
    fn format_read_numbers_lines_from_one() {
        // The same gutter format as the write/edit body: `{n:>W} {text}`, the
        // number right-aligned to the widest shown, a single space, no tab —
        // so the TUI renders read and write as one numbered, highlighted cell.
        let out = format_read("alpha\nbeta\ngamma", None, None);
        assert_eq!(out, "1 alpha\n2 beta\n3 gamma");
    }

    #[test]
    fn format_read_right_aligns_numbers_to_the_widest() {
        let content: String = (1..=12).map(|i| format!("line {i}\n")).collect();
        let out = format_read(&content, None, None);
        assert!(out.starts_with(" 1 line 1\n"), "single digits pad: {out}");
        assert!(out.contains("\n10 line 10\n"), "two digits flush: {out}");
        assert!(out.ends_with("\n12 line 12"), "got {out}");
    }

    #[test]
    fn format_read_honours_offset_and_limit() {
        let out = format_read("a\nb\nc\nd\ne", Some(2), Some(2));
        assert_eq!(out, "2 b\n3 c");
    }

    #[test]
    fn format_read_width_follows_the_last_line_number_shown() {
        // A window ending at a 3-digit line pads every shown number to 3.
        let content: String = (1..=120).map(|i| format!("l{i}\n")).collect();
        let out = format_read(&content, Some(99), Some(2));
        assert_eq!(out, " 99 l99\n100 l100");
    }

    #[test]
    fn format_read_notes_an_offset_past_the_end() {
        let out = format_read("only one line", Some(5), None);
        assert!(out.contains("past the end"), "got {out}");
    }

    #[test]
    fn diff_lines_counts_adds_and_removes() {
        let d = diff_lines("a\nb\nc\n", "a\nB\nc\nd\n");
        assert_eq!(d.added, 2, "B and d are added");
        assert_eq!(d.removed, 1, "b is removed");
        assert_eq!(diff_summary(d.added, d.removed), "(+2 -1)");
    }

    #[test]
    fn diff_lines_of_a_new_file_is_all_adds() {
        let d = diff_lines("", "one\ntwo\n");
        assert_eq!((d.added, d.removed), (2, 0));
        assert!(d.lines.iter().all(|l| matches!(l, DiffLine::Add(_))));
    }

    #[test]
    fn diff_lines_of_identical_text_has_no_changes() {
        let d = diff_lines("same\ntext", "same\ntext");
        assert_eq!((d.added, d.removed), (0, 0));
        assert!(d.lines.iter().all(|l| matches!(l, DiffLine::Context(_))));
    }

    #[test]
    fn a_diff_past_the_lcs_budget_falls_back_to_plain_replacement() {
        // Two unrelated ~1100-line middles exceed DIFF_LCS_MAX_CELLS. The
        // budget is what keeps an edit to a 30k-line file from allocating a
        // multi-gigabyte O(n×m) table — past it, the changed middle renders
        // as a plain remove/add block (the shared "COMMON" line is the
        // context a full LCS would have found; the fallback trades it for
        // bounded memory).
        let old: String = (0..1100)
            .map(|i| {
                if i == 550 {
                    "COMMON\n".to_string()
                } else {
                    format!("old {i}\n")
                }
            })
            .collect();
        let new: String = (0..1100)
            .map(|i| {
                if i == 550 {
                    "COMMON\n".to_string()
                } else {
                    format!("new {i}\n")
                }
            })
            .collect();
        let d = diff_lines(&old, &new);
        assert_eq!((d.added, d.removed), (1100, 1100));
        assert!(
            d.lines.iter().all(|l| !matches!(l, DiffLine::Context(_))),
            "past the budget the middle is a plain remove/add block"
        );
    }

    #[test]
    fn a_one_line_edit_in_a_huge_file_diffs_exactly() {
        // The OOM trigger: one changed line in a 50k-line file. The trimmed
        // prefix/suffix keep the LCS to the single changed line, so the diff
        // stays exact — with the old unbounded table this test allocated
        // ~20 GB and aborted the process.
        let old: String = (0..50_000).map(|i| format!("line {i}\n")).collect();
        let new = old.replace("line 25000\n", "line 25000 CHANGED\n");
        let d = diff_lines(&old, &new);
        assert_eq!((d.added, d.removed), (1, 1));
        assert_eq!(d.lines.len(), 50_001, "one remove + one add + all context");
        assert!(
            d.lines
                .iter()
                .any(|l| matches!(l, DiffLine::Add(t) if t.contains("CHANGED")))
        );
    }

    #[test]
    fn numbered_content_numbers_every_line() {
        let out = render_numbered_content("alpha\nbeta\ngamma");
        assert_eq!(out, "1 alpha\n2 beta\n3 gamma");
    }

    #[test]
    fn numbered_content_right_aligns_the_numbers() {
        let content: String = (1..=12).map(|i| format!("line {i}\n")).collect();
        let out = render_numbered_content(&content);
        assert!(out.starts_with(" 1 line 1\n"), "got {out}");
        assert!(out.contains("\n10 line 10\n"), "got {out}");
    }

    #[test]
    fn numbered_content_caps_a_huge_file() {
        let big: String = (0..DIFF_MAX_LINES + 50)
            .map(|i| format!("l{i}\n"))
            .collect();
        let out = render_numbered_content(&big);
        assert_eq!(out.lines().count(), DIFF_MAX_LINES + 1);
        assert!(
            out.ends_with("… 50 more lines"),
            "tail = {}",
            &out[out.len().saturating_sub(40)..]
        );
    }

    #[test]
    fn numbered_diff_shows_only_the_changed_hunk_with_context() {
        let old: String = (1..=20).map(|i| format!("line {i}\n")).collect();
        let new = old.replace("line 10\n", "line ten\n");
        let d = diff_lines(&old, &new);
        let out = render_numbered_diff(&d);
        let rows: Vec<&str> = out.lines().collect();
        assert_eq!(rows.len(), 8, "3 ctx + del + add + 3 ctx: {out}");
        assert_eq!(rows[0], " 7  line 7");
        assert_eq!(rows[3], "10 -line 10");
        assert_eq!(rows[4], "10 +line ten");
        assert_eq!(rows[7], "13  line 13");
        assert!(!out.contains("line 1\n"), "far context is elided");
    }

    #[test]
    fn numbered_diff_separates_distant_hunks_with_a_gap_row() {
        let old: String = (1..=30).map(|i| format!("line {i}\n")).collect();
        let new = old
            .replace("line 5\n", "line five\n")
            .replace("line 25\n", "line twentyfive\n");
        let d = diff_lines(&old, &new);
        let out = render_numbered_diff(&d);
        assert!(out.contains(" ⋮\n"), "hunks separated by a gap row: {out}");
        assert!(!out.contains("line 15"), "the unchanged middle is elided");
        assert!(out.contains("25 -line 25"), "second hunk numbered: {out}");
        assert!(out.contains("25 +line twentyfive"));
    }

    #[test]
    fn numbered_diff_merges_touching_hunks() {
        let old: String = (1..=20).map(|i| format!("line {i}\n")).collect();
        let new = old
            .replace("line 8\n", "line eight\n")
            .replace("line 11\n", "line eleven\n");
        let d = diff_lines(&old, &new);
        let out = render_numbered_diff(&d);
        assert!(
            !out.contains('⋮'),
            "changes two context-widths apart share a hunk: {out}"
        );
    }

    #[test]
    fn numbered_diff_clamps_context_at_the_file_edges() {
        let d = diff_lines("first\nb\nc\n", "FIRST\nb\nc\n");
        let out = render_numbered_diff(&d);
        let rows: Vec<&str> = out.lines().collect();
        assert_eq!(rows[0], "1 -first");
        assert_eq!(rows[1], "1 +FIRST");
        assert_eq!(rows.last().unwrap(), &"3  c");
    }

    #[test]
    fn numbered_diff_numbers_context_and_adds_by_the_new_file() {
        let d = diff_lines("a\nb\n", "a\nNEW\nb\n");
        let out = render_numbered_diff(&d);
        let rows: Vec<&str> = out.lines().collect();
        assert_eq!(rows, vec!["1  a", "2 +NEW", "3  b"]);
    }

    #[test]
    fn numbered_diff_numbers_removes_by_the_old_file() {
        let d = diff_lines("a\nGONE\nb\n", "a\nb\n");
        let out = render_numbered_diff(&d);
        let rows: Vec<&str> = out.lines().collect();
        // The context after the removal continues the NEW file's numbering.
        assert_eq!(rows, vec!["1  a", "2 -GONE", "2  b"]);
    }

    #[test]
    fn numbered_diff_caps_a_huge_change() {
        let big: String = (0..DIFF_MAX_LINES + 50)
            .map(|i| format!("l{i}\n"))
            .collect();
        let d = diff_lines("", &big);
        let out = render_numbered_diff(&d);
        assert_eq!(out.lines().count(), DIFF_MAX_LINES + 1);
        let tail = out.lines().last().unwrap();
        assert!(
            tail.starts_with('…') && tail.contains("more lines"),
            "tail = {tail}"
        );
    }

    // ===== image reads (docs/tools.md) =====

    #[test]
    fn is_image_path_detects_the_vision_formats_by_extension() {
        assert!(is_image_path("shot.png"));
        assert!(is_image_path("photo.JPG"), "case-insensitive");
        assert!(is_image_path("a/b/pic.jpeg"));
        assert!(is_image_path("anim.gif"));
        assert!(is_image_path("modern.webp"));
        assert!(!is_image_path("main.rs"));
        assert!(!is_image_path("notes.txt"));
        assert!(!is_image_path("png"), "no extension");
        assert!(
            !is_image_path("archive.png.zip"),
            "only the final extension"
        );
        assert!(!is_image_path("diagram.svg"), "svg is text — read as text");
    }

    #[test]
    fn format_read_image_reports_the_facts_and_the_attachment() {
        let out = format_read_image("assets/logo.png", "PNG", 1920, 1080, 245_760);
        assert!(
            out.starts_with("Read image assets/logo.png"),
            "the marker head leads: {out}"
        );
        assert!(out.contains("PNG"), "got {out}");
        assert!(out.contains("1920x1080"), "got {out}");
        assert!(out.contains("240 KB"), "got {out}");
        assert!(
            out.contains("attached"),
            "the model is told where the pixels are: {out}"
        );
    }

    #[test]
    fn format_read_image_humanizes_the_size() {
        let bytes = format_read_image("a.png", "PNG", 1, 1, 512);
        assert!(bytes.contains("512 B"), "got {bytes}");
        let mb = format_read_image("a.png", "PNG", 1, 1, 2_621_440);
        assert!(mb.contains("2.5 MB"), "got {mb}");
    }

    #[test]
    fn is_image_read_output_matches_only_the_image_head() {
        assert!(is_image_read_output(&format_read_image(
            "a.png", "PNG", 1, 1, 10
        )));
        // A text read starts with a numbered gutter row — never the marker.
        assert!(!is_image_read_output("1 alpha\n2 beta"));
        assert!(!is_image_read_output("(file a.txt is empty)"));
        assert!(!is_image_read_output("could not read a.png: missing"));
    }

    #[test]
    fn image_attachment_note_names_the_path_and_the_tool() {
        let note = image_attachment_note("assets/logo.png");
        assert!(
            note.starts_with("[image] "),
            "a bracketed note like the other injected notes: {note}"
        );
        assert!(note.contains("assets/logo.png"), "got {note}");
        assert!(
            note.contains("read"),
            "the model can pair it to the tool call: {note}"
        );
    }

    #[test]
    fn read_spec_description_mentions_images() {
        let specs = tool_specs();
        let desc = specs[1]["function"]["description"].as_str().unwrap();
        assert!(
            desc.contains("image") && desc.contains("attached"),
            "the model is told image files come back attached: {desc}"
        );
    }

    #[test]
    fn format_exec_output_frames_the_exit_code() {
        assert_eq!(format_exec_output(Some(0), "hi\n"), "Exit code: 0\nhi\n");
        assert_eq!(format_exec_output(Some(1), ""), "Exit code: 1\n(no output)");
        assert!(format_exec_output(None, "x").starts_with("Exit code: killed by signal"));
    }

    #[test]
    fn truncate_output_cuts_on_a_char_boundary() {
        let (head, cut) = truncate_output("hello world", 5);
        assert_eq!(head, "hello");
        assert!(cut);
        let (whole, uncut) = truncate_output("short", 100);
        assert_eq!(whole, "short");
        assert!(!uncut);
        // A multi-byte char at the boundary is not split.
        let (h, _) = truncate_output("aé", 2);
        assert_eq!(h, "a", "the 2-byte é is dropped rather than split");
    }

    #[test]
    fn the_agent_tool_joins_the_main_set_but_never_a_subagents() {
        let names: Vec<String> = tool_specs_with_agents()
            .iter()
            .map(|spec| spec["function"]["name"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(names, ["bash", "read", "write", "edit", "agent"]);
        let base: Vec<String> = tool_specs()
            .iter()
            .map(|spec| spec["function"]["name"].as_str().unwrap().to_string())
            .collect();
        assert!(!base.contains(&"agent".to_string()));
        // The agent schema: description+prompt required, the optional pair.
        let spec = &tool_specs_with_agents()[4]["function"]["parameters"];
        assert_eq!(
            spec["required"],
            serde_json::json!(["description", "prompt"])
        );
        assert!(spec["properties"]["run_in_background"].is_object());
        assert!(spec["properties"]["subagent_type"].is_object());
        // Subagent sets: explore is read-only, everything else the full four.
        let explore: Vec<String> = subagent_tool_specs("explore")
            .iter()
            .map(|spec| spec["function"]["name"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(explore, ["bash", "read"]);
        let general: Vec<String> = subagent_tool_specs("general-purpose")
            .iter()
            .map(|spec| spec["function"]["name"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(general, ["bash", "read", "write", "edit"]);
    }

    #[test]
    fn agent_args_default_to_background_general_purpose() {
        let args: AgentArgs =
            parse_args(r#"{"description":"Fetch Warsaw","prompt":"weather?"}"#).unwrap();
        assert_eq!(args.agent_type(), "general-purpose");
        assert!(args.background(), "the schema default is background");
        let args: AgentArgs = parse_args(
            r#"{"description":"d","prompt":"p","subagent_type":"explore","run_in_background":false}"#,
        )
        .unwrap();
        assert_eq!(args.agent_type(), "explore");
        assert!(!args.background());
        assert!(
            parse_args::<AgentArgs>(r#"{"prompt":"p"}"#).is_err(),
            "description is required"
        );
    }

    #[test]
    fn call_description_reads_a_bash_calls_description_only() {
        assert_eq!(
            call_description(
                "bash",
                r#"{"command":"curl x","description":"Fetching weather"}"#
            ),
            Some("Fetching weather".to_string())
        );
        assert_eq!(call_description("bash", r#"{"command":"curl x"}"#), None);
        assert_eq!(
            call_description("bash", r#"{"command":"x","description":"  "}"#),
            None
        );
        assert_eq!(call_description("write", r#"{"path":"game.py"}"#), None);
    }

    #[test]
    fn agent_calls_display_and_summarize_by_description() {
        assert_eq!(display_name("agent"), "Agent");
        assert_eq!(
            summarize_call(
                "agent",
                r#"{"description":"Fetch Warsaw","prompt":"long..."}"#
            ),
            "Fetch Warsaw"
        );
    }
}
