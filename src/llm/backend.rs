//! The [`ReplySource`] bridge: turns the [`OpenAiClient`]'s split deltas into the
//! app's [`StreamEvent`] protocol so a real model is a drop-in for `DummyAi`.
//!
//! Boundary code (it spawns the streaming thread), but the message assembly is
//! factored into the pure [`build_messages`] so it's unit-tested. See
//! `docs/llm.md`.

use std::path::{Path, PathBuf};
use std::thread::{self, JoinHandle};

use tokio::sync::mpsc::UnboundedSender;

use super::agent::{self, RoundOutcome};
use super::approval;
use super::config::ModelConfig;
use super::exec::{RealToolExecutor, ToolExecutor};
use super::openai::{Delta, OpenAiClient};
use super::retry::{self, AttemptResult, MAX_RETRIES};
use super::tools::{self, AgentArgs, ToolCallRequest};
use super::{ChatMessage, ContentPart, LlmError, ToolCallSpec};
use crate::agents::{AgentEvent, AgentRegistry};
use crate::context::ContextMessage;
use crate::permission::PermissionGate;
use crate::stream::{AgentCallDone, AgentSpec, CancelToken, ReplySource, StreamEvent};

/// The default system prompt for the real backend — the "Alter Zero" agent
/// identity, authored in [`prompts/alter_zero.md`](../../prompts/alter_zero.md)
/// and compiled in with `include_str!` so the wording lives in a maintainable
/// markdown file (drop in a new `prompts/*.md` and point this const at it to
/// swap personas). Kept short to save tokens. The boundary folds the runtime
/// **environment context** (date/os/cwd) onto this at startup so the agent has
/// context awareness — see [`augment_with_environment`] and
/// `docs/environment.md`. Override the persona with `ALTER_ZERO_SYSTEM_PROMPT`.
pub const DEFAULT_SYSTEM_PROMPT: &str = include_str!("../../prompts/alter_zero.md");

/// A real OpenAI-compatible backend. Holds the streaming client (carrying the
/// tool definitions when tools are enabled) plus the model id it answers as
/// (for the session footer), whether tools are enabled, and an optional system
/// prompt.
#[derive(Debug, Clone)]
pub struct LlmBackend {
    client: OpenAiClient,
    model: String,
    system_prompt: Option<String>,
    tools_enabled: bool,
    /// The shared background-shell registry, when the boundary attached one —
    /// enables the `bash` tool's `run_in_background` and Ctrl+B handoff
    /// (`docs/background.md`).
    background: Option<crate::background::BackgroundRegistry>,
    /// The model's image-input support ([`ModelConfig::vision`]): gates both
    /// the message assembly (attachments degrade to `[image omitted: …]`
    /// notes on a known non-vision model) and the `read` tool's image branch
    /// (`docs/tools.md`).
    vision: Option<bool>,
    /// The shared subagent registry, when the boundary attached one — enables
    /// the `agent` tool (`docs/agent-tool.md`). Attaching it swaps the
    /// client's tool set to include `agent`.
    agents: Option<AgentRegistry>,
    /// The shared tool-permission gate, when the boundary attached one — makes
    /// every `write`/`edit`/`bash` call ask before it runs
    /// (`docs/permissions.md`). Without it (an embedder, the live tests,
    /// `ALTER_ZERO_PERMISSIONS` off) tools run unasked, as they always did.
    permissions: Option<PermissionGate>,
}

impl LlmBackend {
    /// Build a backend from a resolved [`ModelConfig`], with the default system
    /// prompt.
    #[must_use]
    pub fn new(cfg: ModelConfig) -> Self {
        Self::with_system_prompt(cfg, Some(DEFAULT_SYSTEM_PROMPT.to_string()))
    }

    /// Build a backend with an explicit system prompt (`None` sends no system
    /// message). The boundary passes `ALTER_ZERO_SYSTEM_PROMPT` through here.
    /// Tools are enabled unless `ALTER_ZERO_TOOLS` is falsy (see `docs/tools.md`).
    #[must_use]
    pub fn with_system_prompt(cfg: ModelConfig, system_prompt: Option<String>) -> Self {
        Self::configure(cfg, system_prompt, tools_enabled_from_env())
    }

    /// Build a backend with an explicit tools toggle (the tests' seam; the app
    /// goes through [`with_system_prompt`], reading the env).
    ///
    /// [`with_system_prompt`]: LlmBackend::with_system_prompt
    #[must_use]
    pub fn configure(cfg: ModelConfig, system_prompt: Option<String>, tools_enabled: bool) -> Self {
        let model = cfg.model.clone();
        let vision = cfg.vision;
        let mut client = OpenAiClient::new(cfg);
        // Trim so the prompt-file trailing newline (or a whitespace-only
        // override) normalizes away; a now-empty prompt sends no system message.
        let mut system_prompt = system_prompt
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        if tools_enabled {
            client = client.with_tools(tools::tool_specs());
            // Tell the model the tools exist (the schemas carry the detail);
            // the Ctrl+D debug view then shows the same augmented prompt.
            if let Some(prompt) = system_prompt.as_mut() {
                prompt.push_str("\n\n");
                prompt.push_str(TOOLS_SYSTEM_SUFFIX.trim());
            }
        }
        Self {
            client,
            model,
            system_prompt,
            tools_enabled,
            background: None,
            vision,
            agents: None,
            permissions: None,
        }
    }

    /// Attach the shared background-shell registry so the executor can launch
    /// and adopt background tasks (`docs/background.md`). The boundary calls
    /// this on every backend it builds; tests and offline tools skip it.
    #[must_use]
    pub fn with_background(mut self, registry: crate::background::BackgroundRegistry) -> Self {
        self.background = Some(registry);
        self
    }

    /// Attach the shared subagent registry, **enabling the `agent` tool**
    /// (`docs/agent-tool.md`): the client's tool set gains the `agent` spec
    /// (tools must already be enabled — a tools-off backend stays agent-less).
    /// The boundary calls this on every main backend it builds; the one-off
    /// `/compact` backend and subagents themselves never do.
    #[must_use]
    pub fn with_agents(mut self, registry: AgentRegistry) -> Self {
        if self.tools_enabled {
            self.client = self
                .client
                .clone()
                .with_tools(tools::tool_specs_with_agents());
            self.agents = Some(registry);
        }
        self
    }

    /// Attach the shared tool-permission gate, so every `write`/`edit`/`bash`
    /// call — this backend's and its subagents' — asks the user first
    /// (`docs/permissions.md`). The boundary calls this on the backends it
    /// builds unless `ALTER_ZERO_PERMISSIONS` is falsy.
    #[must_use]
    pub fn with_permissions(mut self, gate: PermissionGate) -> Self {
        self.permissions = Some(gate);
        self
    }

    /// Whether the `bash`/`read`/`write`/`edit` tools are offered to the model.
    #[must_use]
    pub fn tools_enabled(&self) -> bool {
        self.tools_enabled
    }
}

/// Are the `bash`/`read`/`write`/`edit` tools enabled? On by default; disabled
/// by a falsy `ALTER_ZERO_TOOLS` (`0`/`false`/`no`/`off`). See `docs/tools.md`.
fn tools_enabled_from_env() -> bool {
    match std::env::var("ALTER_ZERO_TOOLS") {
        Ok(v) => !matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "0" | "false" | "no" | "off"
        ),
        Err(_) => true,
    }
}

/// The tool-capability note appended to the system prompt when tools are
/// enabled, authored in [`prompts/tools.md`](../../prompts/tools.md) and
/// compiled in with `include_str!` (same maintainable-markdown seam as
/// [`DEFAULT_SYSTEM_PROMPT`]). Joined after a blank line; the schemas carry the
/// per-parameter detail.
const TOOLS_SYSTEM_SUFFIX: &str = include_str!("../../prompts/tools.md");

/// The environment-context template appended to the system prompt for the
/// agent's runtime awareness — authored in
/// [`prompts/environment.md`](../../prompts/environment.md) (terse, in the
/// persona's own style) with `{date}`/`{os}`/`{cwd}` placeholders that
/// [`render_environment`] fills. See `docs/environment.md`.
const ENVIRONMENT_TEMPLATE: &str = include_str!("../../prompts/environment.md");

/// Fill the environment template with the session's `date`, `os`, and `cwd`.
/// Pure: the boundary (`main.rs`) gathers the values (the same clock-injection
/// pattern as [`App::set_clock`]), keeping the library free of time/CWD reads.
///
/// [`App::set_clock`]: crate::app::App::set_clock
#[must_use]
pub fn render_environment(date: &str, os: &str, cwd: &str) -> String {
    ENVIRONMENT_TEMPLATE
        .trim()
        .replace("{date}", date)
        .replace("{os}", os)
        .replace("{cwd}", cwd)
}

/// Append the environment context to a base system prompt so the agent knows
/// its date/os/cwd (`docs/environment.md`). A blank base is returned unchanged
/// so the "empty `ALTER_ZERO_SYSTEM_PROMPT` → no system message" contract holds
/// (`docs/context.md`); the tools note (when enabled) is added by
/// [`LlmBackend::configure`] afterwards, so the final prompt reads
/// persona → environment → tools.
#[must_use]
pub fn augment_with_environment(base: &str, date: &str, os: &str, cwd: &str) -> String {
    if base.trim().is_empty() {
        return base.to_string();
    }
    format!(
        "{}\n\n{}",
        base.trim_end(),
        render_environment(date, os, cwd)
    )
}

/// Extract a human distro name from `/etc/os-release` contents — the
/// `PRETTY_NAME` (e.g. `Ubuntu 24.04.4 LTS`), else `NAME`. Values may be
/// double- or single-quoted (the freedesktop os-release format). Returns
/// `None` when neither key carries a non-empty value. Pure: the boundary
/// (`main.rs`) reads the file and only on Linux, so the agent's `OS` line
/// reads e.g. `linux (Ubuntu 24.04.4 LTS)`. See `docs/environment.md`.
#[must_use]
pub fn os_release_name(contents: &str) -> Option<String> {
    let value = |key: &str| {
        contents.lines().find_map(|line| {
            // Anchored `key=` match so a suffix key (e.g. `CPE_NAME`) never
            // masquerades as `NAME`.
            let rest = line.trim().strip_prefix(key)?.strip_prefix('=')?.trim();
            let unquoted = rest
                .strip_prefix('"')
                .and_then(|r| r.strip_suffix('"'))
                .or_else(|| rest.strip_prefix('\'').and_then(|r| r.strip_suffix('\'')))
                .unwrap_or(rest)
                .trim();
            (!unquoted.is_empty()).then(|| unquoted.to_string())
        })
    };
    value("PRETTY_NAME").or_else(|| value("NAME"))
}

/// Assemble the request messages for one turn: the optional system prompt,
/// then the whole conversation context in order — the multi-turn memory (see
/// `docs/context.md`). A context message with image attachments becomes the
/// multimodal parts form, each attachment encoded to a `data:` URL by
/// `encode_image` (the injected I/O seam — `image_data_url` in production,
/// a fake in tests, keeping this pure). An attachment that fails to encode
/// (its temp file may have been cleaned away) is noted in the text instead of
/// being dropped silently. Should the context ever be empty, the bare
/// `prompt` is sent so the request is never user-less.
///
/// Equivalent to [`build_messages_for`] with unknown vision (attach
/// optimistically) — the shape every pre-vision-detection caller used.
#[must_use]
pub fn build_messages(
    system_prompt: Option<&str>,
    prompt: &str,
    context: &[ContextMessage],
    encode_image: impl Fn(&Path) -> Option<String>,
) -> Vec<ChatMessage> {
    build_messages_for(None, system_prompt, prompt, context, encode_image)
}

/// [`build_messages`] with the active model's image-input support applied:
/// when the model is a **known non-vision** one (`vision == Some(false)`),
/// every attachment — a Ctrl+V paste or a replayed read-tool image — becomes
/// a `[image omitted: …]` text note instead of an `image_url` part, because
/// the provider would otherwise fail the whole request (OpenRouter 404s
/// "No endpoints found that support image input"). `Some(true)`/`None`
/// attach exactly as before. See `docs/tools.md`.
#[must_use]
pub fn build_messages_for(
    vision: Option<bool>,
    system_prompt: Option<&str>,
    prompt: &str,
    context: &[ContextMessage],
    encode_image: impl Fn(&Path) -> Option<String>,
) -> Vec<ChatMessage> {
    let mut messages = Vec::new();
    if let Some(sys) = system_prompt {
        messages.push(ChatMessage::system(sys));
    }
    for message in context {
        messages.push(chat_message(message, vision, &encode_image));
    }
    if context.is_empty() {
        messages.push(ChatMessage::user(prompt));
    }
    messages
}

/// One context message as a wire [`ChatMessage`]: an assistant tool-call turn
/// or a `tool`-role result in the provider-native shape, plain text when
/// imageless, or the multimodal parts array when attachments encode — unless
/// the model is a known non-vision one, in which case attachments become
/// `[image omitted: …]` text notes (see [`build_messages_for`]).
fn chat_message(
    message: &ContextMessage,
    vision: Option<bool>,
    encode_image: &impl Fn(&Path) -> Option<String>,
) -> ChatMessage {
    let role = message.role.wire_name();
    // A tool result carries its call id; an assistant entry may carry the native
    // tool calls it requested — both replay in the Chat Completions tool shape
    // (see docs/context.md), and neither ever has image attachments.
    if let Some(id) = &message.tool_call_id {
        return ChatMessage::tool_result(id, &message.text);
    }
    if !message.tool_calls.is_empty() {
        let specs = message
            .tool_calls
            .iter()
            .map(|c| ToolCallSpec::function(&c.id, &c.name, &c.arguments))
            .collect();
        return ChatMessage::assistant_tool_calls(&message.text, specs);
    }
    if message.images.is_empty() {
        return ChatMessage::new(role, &message.text);
    }
    if vision == Some(false) {
        // A known non-vision model: an image_url part would fail the whole
        // request, so the attachment degrades to a note the model reads —
        // it knows an image existed and why it can't see it.
        let mut text = message.text.clone();
        for path in &message.images {
            text.push_str(&format!(
                "\n[image omitted: {} — the current model does not support image input]",
                path.display()
            ));
        }
        return ChatMessage::new(role, text);
    }
    let mut text = message.text.clone();
    let mut image_parts = Vec::new();
    for path in &message.images {
        match encode_image(path) {
            Some(url) => image_parts.push(ContentPart::image(url)),
            None => {
                // The temp file is gone (e.g. cleaned between sessions) —
                // tell the model rather than silently dropping the image.
                text.push_str(&format!("\n[image unavailable: {}]", path.display()));
            }
        }
    }
    if image_parts.is_empty() {
        return ChatMessage::new(role, text);
    }
    let mut parts = vec![ContentPart::text(text)];
    parts.extend(image_parts);
    ChatMessage::with_parts(role, parts)
}

/// Read an attached image and embed it as a base64 `data:` URL — the OpenAI
/// vision shape. Boundary code (file I/O); `None` when the file is unreadable,
/// which [`chat_message`] surfaces as a text note.
fn image_data_url(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    Some(format!(
        "data:{};base64,{}",
        image_mime(path),
        crate::clipboard::base64_encode(&bytes)
    ))
}

/// The MIME type for an attachment path, by extension. The clipboard paste
/// only ever writes the accepted formats (`docs/image-paste.md`); PNG — its
/// transcode target — is the default.
fn image_mime(path: &Path) -> &'static str {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    match ext.as_deref() {
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        _ => "image/png",
    }
}

impl ReplySource for LlmBackend {
    fn spawn(
        &self,
        prompt: String,
        _images: Vec<PathBuf>, // the context's last user message carries them
        context: Vec<ContextMessage>,
        tx: UnboundedSender<StreamEvent>,
        cancel: CancelToken,
    ) -> JoinHandle<()> {
        let client = self.client.clone();
        let system = self.system_prompt.clone();
        let background = self.background.clone();
        let vision = self.vision;
        let agents = self.agents.clone();
        let subagent = self.subagent_config();
        let permissions = self.permissions.clone();
        thread::spawn(move || {
            // Encoding the attachments reads files — done here on the backend
            // thread so a large image never stalls the event loop. A known
            // non-vision model gets omission notes instead of parts
            // (docs/tools.md).
            let mut messages =
                build_messages_for(vision, system.as_deref(), &prompt, &context, image_data_url);
            let mut executor = RealToolExecutor::new().with_vision(vision);
            let notices = background.clone();
            if let Some(registry) = background.clone() {
                // The registry carries the terminal-detach helper from
                // `main.rs`; hand it to the executor so foreground `bash`
                // children detach exactly like `launch`ed ones (crate::spawn).
                executor = executor
                    .with_detach_helper(registry.detach_helper())
                    .with_background(registry);
            }
            // The agentic loop: `run_agent` streams one round, runs any tool
            // calls the model requested (via `executor`, emitting the
            // ToolStart/ToolEnd pair the TUI renders), appends the results, and
            // loops until the model answers with plain text — sending the
            // terminal StreamDone/Error itself. Each round retries transient
            // failures internally (see `stream_round`). With tools disabled the
            // model never asks for any, so this collapses to a single round —
            // the old plain-stream behaviour. See `docs/tools.md`. Before each
            // round it takes the registry's completion notice board, so a
            // background shell that finished (or was killed) since the last
            // request is known to the model within this same turn
            // (docs/background.md). The round's `agent` calls go to the
            // subagent launcher instead (docs/agent-tool.md).
            agent::run_agent(
                &tx,
                &cancel,
                agent::MAX_TOOL_ITERATIONS,
                &mut messages,
                |msgs| stream_round(&client, msgs, &tx, &cancel),
                |call, on_output| executor.execute(call, &cancel, on_output),
                || match &notices {
                    Some(registry) => registry
                        .take_pending_notices()
                        .into_iter()
                        .map(|note| note.context)
                        .collect(),
                    None => Vec::new(),
                },
                |calls| match &agents {
                    Some(registry) => run_agent_calls(
                        &subagent,
                        registry,
                        background.as_ref(),
                        &tx,
                        &cancel,
                        calls,
                    ),
                    // No registry attached (the tool isn't offered) — a call
                    // that somehow arrives is declined recoverably.
                    None => calls
                        .iter()
                        .map(|call| {
                            (
                                call.id.clone(),
                                "the agent tool is not available here".to_string(),
                            )
                        })
                        .collect(),
                },
                // The permission gate: a `write`/`edit`/`bash` call raises the
                // inline prompt and blocks this thread on the answer, unless a
                // standing approval already covers it (docs/permissions.md).
                |call| approval::approve_call(permissions.as_ref(), &tx, &cancel, None, call),
            );
        })
    }

    fn model_name(&self) -> String {
        self.model.clone()
    }

    fn system_prompt(&self) -> Option<String> {
        self.system_prompt.clone()
    }

    /// Send a chat message into a subagent's session (`docs/agent-tool.md`):
    /// queued into its running loop, or a continuation run over its stored
    /// conversation when idle. `false` when agents aren't enabled here or the
    /// id is unknown/busy-less-stored.
    fn spawn_agent_chat(&self, id: &str, text: &str) -> bool {
        let Some(registry) = &self.agents else {
            return false;
        };
        if registry.queue_input(id, text) {
            return true;
        }
        let Some((mut messages, cancel)) = registry.begin_continuation(id) else {
            return false;
        };
        messages.push(ChatMessage::user(text));
        spawn_subagent_run(
            &self.subagent_config(),
            registry.clone(),
            id.to_string(),
            registry.agent_type(id),
            messages,
            cancel,
        );
        true
    }
}

/// Everything a subagent run needs off the backend, bundled so the launcher
/// closure and the chat continuation share one shape.
#[derive(Clone)]
struct SubagentConfig {
    client: OpenAiClient,
    /// The subagent's system prompt: the main prompt (persona + environment +
    /// tools note) with the subagent note appended (`prompts/subagent.md`).
    system_prompt: Option<String>,
    vision: Option<bool>,
    detach_helper: Option<std::path::PathBuf>,
    /// The shared background-shell registry, so a subagent's `bash` can
    /// `run_in_background` too — its shells stack into the same footer count
    /// and ↓ manager, attributed to the launcher (`docs/agent-tool.md`,
    /// `docs/background.md`).
    background: Option<crate::background::BackgroundRegistry>,
    /// The shared permission gate, so a subagent's `write`/`edit`/`bash` calls
    /// ask too — the prompt names the agent that asked
    /// (`docs/permissions.md`).
    permissions: Option<PermissionGate>,
}

impl LlmBackend {
    /// The bundled config a subagent run needs (see [`SubagentConfig`]).
    fn subagent_config(&self) -> SubagentConfig {
        let suffix = SUBAGENT_SYSTEM_SUFFIX.trim();
        let system_prompt = Some(match &self.system_prompt {
            Some(base) => format!("{base}\n\n{suffix}"),
            None => suffix.to_string(),
        });
        SubagentConfig {
            client: self.client.clone(),
            system_prompt,
            vision: self.vision,
            detach_helper: self
                .background
                .as_ref()
                .and_then(crate::background::BackgroundRegistry::detach_helper),
            background: self.background.clone(),
            permissions: self.permissions.clone(),
        }
    }
}

/// The subagent note appended to a subagent's system prompt, authored in
/// [`prompts/subagent.md`](../../prompts/subagent.md) (the maintainable-
/// markdown seam every prompt fragment uses).
const SUBAGENT_SYSTEM_SUFFIX: &str = include_str!("../../prompts/subagent.md");

/// How often the foreground wait loop polls its agents / the parent cancel /
/// the Ctrl+B latch.
const AGENT_WAIT_POLL: std::time::Duration = std::time::Duration::from_millis(30);

/// The model-facing acknowledgement of a background agent launch — the tool
/// result a `run_in_background` `agent` call returns at once (the reference
/// wording, adapted). See `docs/agent-tool.md`.
#[must_use]
pub fn agent_launch_text(id: &str, description: &str) -> String {
    format!(
        "Async agent launched successfully.\n\
         agentId: {id} (\"{description}\")\n\
         The agent is working in the background. You will be notified \
         automatically with its final response when it completes. Do not wait \
         or poll for it — continue with the rest of the task (or end your \
         turn) and briefly tell the user what you launched."
    )
}

/// The model-facing result of a **Ctrl+B handoff** on a running foreground
/// agent group — the user moved it to the background mid-wait, so the model
/// must not expect the response in this result (the bash handoff's twin,
/// `docs/background.md`).
#[must_use]
pub fn agent_handoff_text(id: &str, description: &str) -> String {
    format!(
        "The user moved this agent to the background while it was running — \
         it keeps running there, so its final response will not arrive in \
         this result.\n{}",
        agent_launch_text(id, description),
    )
}

/// The tool result of a settled foreground agent: its final response — or
/// the reference's placeholder when it answered with nothing, the stopped
/// note for a user kill, and a failure note otherwise.
fn agent_result_text(outcome: Option<Result<String, String>>) -> (String, bool) {
    match outcome {
        Some(Ok(text)) if text.trim().is_empty() => (
            "(Subagent completed but returned no output.)".to_string(),
            true,
        ),
        Some(Ok(text)) => (text, true),
        Some(Err(error)) if error == "stopped by the user" => {
            (crate::app::AGENT_STOPPED_OUTPUT.to_string(), false)
        }
        Some(Err(error)) => (format!("[agent failed: {error}]"), false),
        None => (crate::app::AGENT_STOPPED_OUTPUT.to_string(), false),
    }
}

/// One launched subagent as the wait loop tracks it.
struct LaunchedAgent {
    call_id: String,
    spec: AgentSpec,
}

/// Run one round's `agent` tool calls (`docs/agent-tool.md`): parse each
/// call, spawn every subagent **concurrently**, announce the background and
/// foreground groups (`StreamEvent::AgentBatch`), resolve the background
/// group at once with launch acknowledgements, wait for the foreground group
/// — polling the parent `cancel` (an Esc kills the group) and the background
/// registry's Ctrl+B latch (moving the rest of the group over) — and resolve
/// it (`StreamEvent::AgentGroupDone`). Returns each call's `(call id, tool
/// result)`.
fn run_agent_calls(
    config: &SubagentConfig,
    registry: &AgentRegistry,
    background: Option<&crate::background::BackgroundRegistry>,
    tx: &UnboundedSender<StreamEvent>,
    cancel: &CancelToken,
    calls: &[ToolCallRequest],
) -> Vec<(String, String)> {
    let mut results: Vec<(String, String)> = Vec::new();
    let mut foreground: Vec<LaunchedAgent> = Vec::new();
    let mut launched_background: Vec<LaunchedAgent> = Vec::new();
    for call in calls {
        let args: AgentArgs = match tools::parse_args(&call.arguments) {
            Ok(args) => args,
            Err(e) => {
                results.push((call.id.clone(), e));
                continue;
            }
        };
        let (id, agent_cancel) = registry.register(args.agent_type());
        let spec = AgentSpec {
            id: id.clone(),
            description: args.description.clone(),
            agent_type: args.agent_type().to_string(),
            prompt: args.prompt.clone(),
            background: args.background(),
        };
        // A subagent conversation starts fresh: the (augmented) system
        // prompt, then the task as the first user message.
        let mut messages = Vec::new();
        if let Some(system) = &config.system_prompt {
            messages.push(ChatMessage::system(system));
        }
        messages.push(ChatMessage::user(&args.prompt));
        spawn_subagent_run(
            config,
            registry.clone(),
            id.clone(),
            args.agent_type().to_string(),
            messages,
            agent_cancel,
        );
        let launched = LaunchedAgent {
            call_id: call.id.clone(),
            spec,
        };
        if args.background() {
            launched_background.push(launched);
        } else {
            foreground.push(launched);
        }
    }
    // The background group: announced and resolved at once with launch
    // acknowledgements — the agents keep running on their own threads.
    if !launched_background.is_empty() {
        let _ = tx.send(StreamEvent::AgentBatch {
            background: true,
            agents: launched_background.iter().map(|l| l.spec.clone()).collect(),
        });
        let mut dones = Vec::new();
        for launched in &launched_background {
            let text = agent_launch_text(&launched.spec.id, &launched.spec.description);
            results.push((launched.call_id.clone(), text.clone()));
            dones.push(AgentCallDone {
                id: launched.spec.id.clone(),
                output: text,
                ok: true,
            });
        }
        let _ = tx.send(StreamEvent::AgentGroupDone {
            background: true,
            agents: dones,
        });
    }
    // The foreground group: announced, then awaited.
    if !foreground.is_empty() {
        let _ = tx.send(StreamEvent::AgentBatch {
            background: false,
            agents: foreground.iter().map(|l| l.spec.clone()).collect(),
        });
        let mut handed_off = false;
        loop {
            if foreground
                .iter()
                .all(|launched| registry.is_done(&launched.spec.id))
            {
                break;
            }
            if cancel.is_cancelled() {
                // Esc: stop the group's still-running agents (background
                // agents launched above keep running — they are independent).
                for launched in &foreground {
                    let _ = registry.kill(&launched.spec.id);
                }
                break;
            }
            if background
                .is_some_and(crate::background::BackgroundRegistry::take_background_request)
            {
                // Ctrl+B: the rest of the group moves to the background.
                handed_off = true;
                break;
            }
            std::thread::sleep(AGENT_WAIT_POLL);
        }
        let mut dones = Vec::new();
        for launched in &foreground {
            let (text, ok) = if handed_off && !registry.is_done(&launched.spec.id) {
                (
                    agent_handoff_text(&launched.spec.id, &launched.spec.description),
                    true,
                )
            } else {
                agent_result_text(registry.outcome(&launched.spec.id))
            };
            results.push((launched.call_id.clone(), text.clone()));
            dones.push(AgentCallDone {
                id: launched.spec.id.clone(),
                output: text,
                ok,
            });
        }
        let _ = tx.send(StreamEvent::AgentGroupDone {
            background: handed_off,
            agents: dones,
        });
    }
    results
}

/// Spawn one subagent run (the initial task, or a chat continuation) on its
/// own thread: its `run_agent` loop streams tagged events onto the agent
/// channel via a forwarder, its chat inputs arrive through the registry's
/// pending-input seam, and its outcome + final message list land back in the
/// registry for the parent's wait loop / a later continuation. See
/// `docs/agent-tool.md`.
fn spawn_subagent_run(
    config: &SubagentConfig,
    registry: AgentRegistry,
    id: String,
    agent_type: String,
    mut messages: Vec<ChatMessage>,
    cancel: CancelToken,
) {
    let client = config
        .client
        .clone()
        .with_tools(tools::subagent_tool_specs(&agent_type));
    let vision = config.vision;
    let detach = config.detach_helper.clone();
    let background = config.background.clone();
    let permissions = config.permissions.clone();
    thread::spawn(move || {
        // The forwarder tags every event with the agent id and tracks the
        // final reply text + terminal outcome (the last uninterrupted text
        // run is the agent's final message — a new tool round resets it).
        let (tx2, mut rx2) = tokio::sync::mpsc::unbounded_channel::<StreamEvent>();
        let forward_registry = registry.clone();
        let forward_id = id.clone();
        let forwarder = thread::spawn(move || {
            let mut final_text = String::new();
            let mut outcome: Option<Result<(), String>> = None;
            while let Some(event) = rx2.blocking_recv() {
                match &event {
                    StreamEvent::Chunk(chunk) => final_text.push_str(chunk),
                    StreamEvent::ToolBatch(_)
                    | StreamEvent::ToolStart { .. }
                    | StreamEvent::AgentBatch { .. } => final_text.clear(),
                    StreamEvent::StreamDone => outcome = Some(Ok(())),
                    StreamEvent::Error(e) => outcome = Some(Err(e.clone())),
                    _ => {}
                }
                forward_registry.send(AgentEvent::Stream {
                    id: forward_id.clone(),
                    event,
                });
            }
            (final_text, outcome)
        });
        let mut executor = RealToolExecutor::new()
            .with_vision(vision)
            .with_detach_helper(detach);
        // The shared background registry rides in so the subagent's `bash`
        // can `run_in_background` — every launch attributed to this agent,
        // and the Ctrl+B latch untouched (docs/agent-tool.md).
        if let Some(bg) = background {
            executor =
                executor
                    .with_background(bg)
                    .with_background_origin(crate::background::BgOrigin {
                        agent_id: id.clone(),
                        agent_type: agent_type.clone(),
                    });
        }
        let executor = executor;
        let inputs_registry = registry.clone();
        let inputs_id = id.clone();
        agent::run_agent(
            &tx2,
            &cancel,
            agent::MAX_TOOL_ITERATIONS,
            &mut messages,
            |msgs| stream_round(&client, msgs, &tx2, &cancel),
            |call, on_output| executor.execute(call, &cancel, on_output),
            // The chat seam: user messages sent into this agent's session
            // arrive at its next round boundary (docs/agent-tool.md).
            || inputs_registry.take_pending_inputs(&inputs_id),
            // Subagents cannot nest agents (the tool isn't offered; a
            // hallucinated call is declined recoverably).
            |calls| {
                calls
                    .iter()
                    .map(|call| {
                        (
                            call.id.clone(),
                            "agents cannot launch further agents".to_string(),
                        )
                    })
                    .collect()
            },
            // A subagent's tool calls ask too — the prompt says which agent
            // is asking (docs/permissions.md). The event rides tx2, so the
            // forwarder tags it with this agent's id like every other.
            |call| {
                approval::approve_call(permissions.as_ref(), &tx2, &cancel, Some(&agent_type), call)
            },
        );
        drop(tx2);
        let (final_text, outcome) = forwarder.join().unwrap_or_default();
        let outcome = match outcome {
            Some(Ok(())) => {
                // Store the final reply so a chat continuation resumes from
                // the complete exchange.
                if !final_text.is_empty() {
                    messages.push(ChatMessage::new("assistant", &final_text));
                }
                Ok(final_text)
            }
            Some(Err(error)) => Err(error),
            // Cancelled (killed) — the registry's kill outcome stands.
            None => Err("stopped by the user".to_string()),
        };
        registry.finish(&id, outcome, messages);
    });
}

/// Stream one round of the conversation: one HTTP request (with the existing
/// per-request retry), emitting the `Chunk`/`Thinking*` events as text and
/// reasoning arrive, and reporting the [`RoundOutcome`] the agent loop acts on
/// — `Complete` (plain answer), `ToolCalls` (the model wants to run tools),
/// `Cancelled`, or `Failed`. Boundary code (real HTTP); the loop that calls it
/// and the retry it wraps are unit-tested separately.
fn stream_round(
    client: &OpenAiClient,
    messages: &[ChatMessage],
    tx: &UnboundedSender<StreamEvent>,
    cancel: &CancelToken,
) -> RoundOutcome {
    // The successful attempt's outcome (text + tool calls) is stashed here so
    // it survives the retry driver, which only reports the disposition.
    let mut captured = None;
    // One streaming attempt: stream the deltas as events and report the outcome
    // plus whether it emitted any content. `retry::run_attempts` re-runs it —
    // after a `Retrying` announcement + backoff — only for a retryable failure
    // that emitted nothing, so a retry can never duplicate streamed text.
    let attempt = || {
        let mut emitted = false;
        // Track the thinking phase so a reasoning burst opens exactly one
        // ThinkingStart and the first response text after it closes with a
        // ThinkingEnd — the pairing the status line expects.
        let mut thinking = false;
        let result = client.stream_chat(messages.to_vec(), cancel, |delta: Delta| {
            if !delta.reasoning.is_empty() {
                emitted = true;
                if !thinking {
                    thinking = true;
                    let _ = tx.send(StreamEvent::ThinkingStart);
                }
                let _ = tx.send(StreamEvent::ThinkingChunk(delta.reasoning));
            }
            if !delta.response.is_empty() {
                emitted = true;
                if thinking {
                    thinking = false;
                    let _ = tx.send(StreamEvent::ThinkingEnd);
                }
                let _ = tx.send(StreamEvent::Chunk(delta.response));
            }
            if !delta.tool_call.is_empty() {
                // The model is generating a tool call: count its tokens so the
                // status keeps ticking (docs/status-indicator.md). Close the
                // thinking phase first if a reasoning burst preceded the call.
                emitted = true;
                if thinking {
                    thinking = false;
                    let _ = tx.send(StreamEvent::ThinkingEnd);
                }
                let _ = tx.send(StreamEvent::ToolCallDelta(delta.tool_call));
            }
        });
        // A stream that ends while still "thinking" (reasoning only, no
        // response) must still close the phase.
        if thinking {
            let _ = tx.send(StreamEvent::ThinkingEnd);
        }
        match result {
            Ok(outcome) => {
                // A tool-calling round emits no visible reply text, so treat it
                // as "emitted" too — a retry would re-run tools, which we never
                // want (it can never duplicate here since Ok isn't retried).
                let had_tools = !outcome.tool_calls.is_empty();
                captured = Some(outcome);
                (AttemptResult::Ok, emitted || had_tools)
            }
            Err(LlmError::Cancelled) => (AttemptResult::Cancelled, emitted),
            Err(e) => (AttemptResult::Failed(e), emitted),
        }
    };
    match retry::run_attempts(tx, cancel, MAX_RETRIES, attempt, retry::sleep_cancellable) {
        AttemptResult::Ok => {
            let outcome = captured.unwrap_or_default();
            // The round's real usage frame (stream_options.include_usage):
            // forward it so the app snaps its tally to the provider's own
            // accounting — one report per round, so an agentic turn
            // accumulates them (docs/prompt-caching.md).
            if let Some(usage) = outcome.usage {
                let _ = tx.send(StreamEvent::Usage(usage));
            }
            if outcome.tool_calls.is_empty() {
                RoundOutcome::Complete
            } else {
                let assistant = ChatMessage::assistant_tool_calls(
                    outcome.text.response,
                    to_tool_call_specs(&outcome.tool_calls),
                );
                RoundOutcome::ToolCalls {
                    assistant,
                    calls: outcome.tool_calls,
                }
            }
        }
        AttemptResult::Cancelled => RoundOutcome::Cancelled,
        AttemptResult::Failed(e) => RoundOutcome::Failed(e),
    }
}

/// Echo the model's requested tool calls back as the assistant message's
/// `tool_calls` array, so the provider can pair each `role:"tool"` result to
/// its call.
fn to_tool_call_specs(calls: &[ToolCallRequest]) -> Vec<ToolCallSpec> {
    calls
        .iter()
        .map(|c| ToolCallSpec::function(&c.id, &c.name, &c.arguments))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::context::{ContextMessage, ContextRole, ContextToolCall};
    use crate::llm::MessageContent;

    /// A fake encoder for the pure tests: every path "encodes" to a data URL
    /// naming its file, so assertions can tell attachments apart.
    fn fake_encode(path: &Path) -> Option<String> {
        Some(format!("data:image/png;base64,{}", path.display()))
    }

    /// An encoder whose files are all unreadable.
    fn failing_encode(_path: &Path) -> Option<String> {
        None
    }

    #[test]
    fn build_messages_sends_the_system_prompt_then_the_whole_context() {
        let context = vec![
            ContextMessage::new(ContextRole::User, "hello"),
            ContextMessage::new(ContextRole::Assistant, "hi!"),
            ContextMessage::new(ContextRole::User, "and again"),
        ];
        let msgs = build_messages(Some("be nice"), "and again", &context, fake_encode);
        assert_eq!(msgs.len(), 4);
        assert_eq!(msgs[0].role, "system");
        assert_eq!(msgs[0].content, MessageContent::Text("be nice".into()));
        assert_eq!(msgs[1].role, "user");
        assert_eq!(msgs[1].content, MessageContent::Text("hello".into()));
        assert_eq!(msgs[2].role, "assistant");
        assert_eq!(msgs[2].content, MessageContent::Text("hi!".into()));
        assert_eq!(msgs[3].role, "user");
        assert_eq!(msgs[3].content, MessageContent::Text("and again".into()));
    }

    #[test]
    fn build_messages_translates_native_tool_calls_and_results() {
        // A replayed tool round from history: the assistant `tool_calls` entry
        // and its `tool`-role result become the provider-native wire messages
        // (not the old bracketed text) — see docs/context.md.
        let context = vec![
            ContextMessage::assistant_tool_calls(
                "let me check",
                vec![ContextToolCall::new("call_0", "read", r#"{"path":"f"}"#)],
            ),
            ContextMessage::tool_result("call_0", "L1"),
        ];
        let msgs = build_messages(None, "", &context, fake_encode);
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, "assistant");
        assert_eq!(msgs[0].content, MessageContent::Text("let me check".into()));
        assert_eq!(
            msgs[0].tool_calls,
            vec![ToolCallSpec::function("call_0", "read", r#"{"path":"f"}"#)]
        );
        assert!(msgs[0].tool_call_id.is_none());
        assert_eq!(msgs[1].role, "tool");
        assert_eq!(msgs[1].content, MessageContent::Text("L1".into()));
        assert!(msgs[1].tool_calls.is_empty());
        assert_eq!(msgs[1].tool_call_id.as_deref(), Some("call_0"));
    }

    #[test]
    fn build_messages_omits_the_system_prompt_when_none() {
        let context = vec![ContextMessage::new(ContextRole::User, "hello")];
        let msgs = build_messages(None, "hello", &context, fake_encode);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].role, "user");
    }

    #[test]
    fn build_messages_falls_back_to_the_prompt_on_an_empty_context() {
        // The context always ends with the just-recorded user message, but if
        // it were ever empty the request must still carry the user's text.
        let msgs = build_messages(None, "hello", &[], fake_encode);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].role, "user");
        assert_eq!(msgs[0].content, MessageContent::Text("hello".into()));
    }

    #[test]
    fn a_context_message_with_images_becomes_multimodal_parts() {
        let context = vec![ContextMessage {
            role: ContextRole::User,
            text: "[Image #1] what is this?".into(),
            images: vec![PathBuf::from("/tmp/shot.png")],
            tool_calls: vec![],
            tool_call_id: None,
        }];
        let msgs = build_messages(None, "", &context, fake_encode);
        assert_eq!(
            msgs[0].content,
            MessageContent::Parts(vec![
                ContentPart::text("[Image #1] what is this?"),
                ContentPart::image("data:image/png;base64,/tmp/shot.png"),
            ])
        );
    }

    #[test]
    fn a_known_non_vision_model_gets_omission_notes_instead_of_parts() {
        // The model's record said "no image input": sending the parts array
        // anyway fails the whole request (OpenRouter 404s it), so every
        // attachment — a Ctrl+V paste or a replayed read-tool image — becomes
        // a text note naming the omitted file. The message stays plain text
        // (no parts), and the encoder is never consulted.
        let context = vec![ContextMessage {
            role: ContextRole::User,
            text: "[Image #1] what is this?".into(),
            images: vec![PathBuf::from("/tmp/shot.png")],
            tool_calls: vec![],
            tool_call_id: None,
        }];
        let msgs = build_messages_for(
            Some(false),
            None,
            "",
            &context,
            |_: &Path| -> Option<String> { panic!("a blind model must not encode attachments") },
        );
        assert_eq!(
            msgs[0].content,
            MessageContent::Text(
                "[Image #1] what is this?\n\
                 [image omitted: /tmp/shot.png — the current model does not support image input]"
                    .into()
            )
        );
        // Some(true) / None (unknown) keep today's optimistic parts form.
        for vision in [Some(true), None] {
            let msgs = build_messages_for(vision, None, "", &context, fake_encode);
            assert!(
                matches!(msgs[0].content, MessageContent::Parts(_)),
                "vision {vision:?} still attaches"
            );
        }
    }

    #[test]
    fn an_unreadable_image_is_noted_in_the_text_not_dropped_silently() {
        let context = vec![ContextMessage {
            role: ContextRole::User,
            text: "look".into(),
            images: vec![PathBuf::from("/tmp/gone.png")],
            tool_calls: vec![],
            tool_call_id: None,
        }];
        let msgs = build_messages(None, "", &context, failing_encode);
        // No image encoded → back to plain text, with the loss noted.
        assert_eq!(
            msgs[0].content,
            MessageContent::Text("look\n[image unavailable: /tmp/gone.png]".into())
        );
    }

    #[test]
    fn a_readable_and_an_unreadable_image_mix_into_parts_plus_a_note() {
        let context = vec![ContextMessage {
            role: ContextRole::User,
            text: "both".into(),
            images: vec![PathBuf::from("/tmp/gone.png"), PathBuf::from("/tmp/ok.png")],
            tool_calls: vec![],
            tool_call_id: None,
        }];
        let encode = |path: &Path| {
            (path == Path::new("/tmp/ok.png")).then(|| "data:image/png;base64,OK".to_string())
        };
        let msgs = build_messages(None, "", &context, encode);
        assert_eq!(
            msgs[0].content,
            MessageContent::Parts(vec![
                ContentPart::text("both\n[image unavailable: /tmp/gone.png]"),
                ContentPart::image("data:image/png;base64,OK"),
            ])
        );
    }

    #[test]
    fn image_mime_maps_the_accepted_extensions() {
        assert_eq!(image_mime(Path::new("a.png")), "image/png");
        assert_eq!(image_mime(Path::new("a.JPG")), "image/jpeg");
        assert_eq!(image_mime(Path::new("a.jpeg")), "image/jpeg");
        assert_eq!(image_mime(Path::new("a.gif")), "image/gif");
        assert_eq!(image_mime(Path::new("a.webp")), "image/webp");
        assert_eq!(image_mime(Path::new("no-extension")), "image/png");
    }

    #[test]
    fn image_data_url_embeds_the_file_as_base64() {
        // The one boundary helper, exercised with a real temp file.
        let dir = std::env::temp_dir();
        let path = dir.join("alter-zero-test-image-data-url.png");
        std::fs::write(&path, b"foobar").unwrap();
        let url = image_data_url(&path).expect("readable file encodes");
        std::fs::remove_file(&path).ok();
        assert_eq!(url, "data:image/png;base64,Zm9vYmFy");
        assert!(image_data_url(Path::new("/definitely/not/here.png")).is_none());
    }

    #[test]
    fn backend_reports_the_configured_model_name() {
        let mut cfg = ModelConfig::fallback();
        cfg.model = "anthropic/claude-fable-5".to_string();
        let backend = LlmBackend::new(cfg);
        assert_eq!(backend.model_name(), "anthropic/claude-fable-5");
    }

    #[test]
    fn backend_surfaces_its_system_prompt_for_the_debug_view() {
        // Tools off so the prompt isn't augmented — deterministic regardless of
        // the ambient ALTER_ZERO_TOOLS (the augmented case has its own test).
        let backend = LlmBackend::configure(ModelConfig::fallback(), Some("be nice".into()), false);
        assert_eq!(
            ReplySource::system_prompt(&backend).as_deref(),
            Some("be nice")
        );
    }

    #[test]
    fn blank_system_prompt_is_dropped() {
        let backend = LlmBackend::with_system_prompt(ModelConfig::fallback(), Some("  ".into()));
        assert!(backend.system_prompt.is_none());
    }

    #[test]
    fn enabling_tools_augments_the_system_prompt_and_flags_the_backend() {
        let backend = LlmBackend::configure(ModelConfig::fallback(), Some("be nice".into()), true);
        assert!(backend.tools_enabled());
        let prompt = backend.system_prompt.as_deref().unwrap();
        assert!(prompt.starts_with("be nice"));
        assert!(prompt.contains("bash"), "the tools are named in the prompt");
        assert!(prompt.contains("edit"));
    }

    #[test]
    fn disabling_tools_leaves_the_prompt_untouched() {
        let backend = LlmBackend::configure(ModelConfig::fallback(), Some("be nice".into()), false);
        assert!(!backend.tools_enabled());
        assert_eq!(backend.system_prompt.as_deref(), Some("be nice"));
    }

    #[test]
    fn render_environment_fills_every_placeholder() {
        let block = render_environment("Sunday 2026-07-19", "linux", "/home/user/proj");
        assert!(block.contains("Sunday 2026-07-19"), "date is in: {block}");
        assert!(block.contains("linux"), "os is in: {block}");
        assert!(block.contains("/home/user/proj"), "cwd is in: {block}");
        // Every `{token}` placeholder is substituted — none survive.
        assert!(!block.contains('{'), "no leftover placeholder: {block}");
    }

    #[test]
    fn augment_with_environment_appends_the_block_after_the_base() {
        let out =
            augment_with_environment("You are Alter Zero", "Sunday 2026-07-19", "linux", "/tmp/x");
        assert!(
            out.starts_with("You are Alter Zero"),
            "persona leads: {out}"
        );
        let persona = out.find("You are Alter Zero").unwrap();
        let cwd = out.find("/tmp/x").unwrap();
        assert!(persona < cwd, "environment follows the persona: {out}");
        assert!(
            out.contains("Sunday 2026-07-19") && out.contains("linux"),
            "{out}"
        );
    }

    #[test]
    fn augment_with_environment_leaves_a_blank_base_unchanged() {
        // The "empty ALTER_ZERO_SYSTEM_PROMPT → no system message" contract
        // (docs/context.md) must survive: a blank base gains no environment
        // block, so `configure` still drops it to `None`.
        assert_eq!(augment_with_environment("   ", "d", "o", "c"), "   ");
        assert_eq!(augment_with_environment("", "d", "o", "c"), "");
    }

    #[test]
    fn boundary_order_is_persona_then_environment_then_tools() {
        // The full assembly the boundary produces: augment first (persona +
        // environment), then `configure` appends the tools note — so the
        // Ctrl+D debug view reads persona → environment → tools.
        let base =
            augment_with_environment("You are Alter Zero", "Sunday 2026-07-19", "linux", "/repo");
        let backend = LlmBackend::configure(ModelConfig::fallback(), Some(base), true);
        let prompt = backend.system_prompt.as_deref().unwrap();
        let persona = prompt.find("Alter Zero").expect("persona present");
        let env = prompt.find("/repo").expect("environment present");
        let tools = prompt.find("bash").expect("tools note present");
        assert!(
            persona < env && env < tools,
            "order persona<env<tools: {prompt}"
        );
    }

    #[test]
    fn os_release_name_prefers_pretty_name() {
        let contents = "\
PRETTY_NAME=\"Ubuntu 24.04.4 LTS\"
NAME=\"Ubuntu\"
VERSION_ID=\"24.04\"
ID=ubuntu
";
        assert_eq!(
            os_release_name(contents).as_deref(),
            Some("Ubuntu 24.04.4 LTS")
        );
    }

    #[test]
    fn os_release_name_falls_back_to_name_without_pretty_name() {
        // Single-quoted value, no PRETTY_NAME — some minimal distros ship this.
        let contents = "NAME='Alpine Linux'\nVERSION_ID=3.20.0\n";
        assert_eq!(os_release_name(contents).as_deref(), Some("Alpine Linux"));
    }

    #[test]
    fn os_release_name_is_none_when_absent_or_blank() {
        assert_eq!(os_release_name("ID=void\nVERSION_ID=rolling\n"), None);
        assert_eq!(os_release_name("PRETTY_NAME=\"\"\n"), None);
        assert_eq!(os_release_name(""), None);
    }

    #[test]
    fn os_release_name_does_not_match_a_suffix_key() {
        // A key that merely ends in NAME (CPE_NAME) must not be read as NAME.
        let contents = "CPE_NAME=\"cpe:/o:fedoraproject:fedora:40\"\n";
        assert_eq!(os_release_name(contents), None);
    }

    #[test]
    fn to_tool_call_specs_echoes_id_name_and_arguments() {
        let calls = vec![ToolCallRequest {
            id: "c1".into(),
            name: "bash".into(),
            arguments: r#"{"command":"ls"}"#.into(),
        }];
        let specs = to_tool_call_specs(&calls);
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].id, "c1");
        assert_eq!(specs[0].function.name, "bash");
        assert_eq!(specs[0].function.arguments, r#"{"command":"ls"}"#);
    }
}
