//! Turning the hooks and MCP documents a plugin ships into the two files the
//! session's own parsers read.
//!
//! A plugin may carry Claude Code's shapes, Codex's, or agent-plugins.org's,
//! each with its own variable spellings and working-directory rules. Rather
//! than teach the hooks runner and the MCP loader three dialects, the
//! installer rewrites what it finds once, at install, into the one shape each
//! already reads — `hooks::HooksFile` and `mcp::parse_mcp_file` — and writes
//! the result under `.alter-zero-plugin/`. Anything alter-zero cannot run is
//! left out and named, so the detail page can say what was skipped. See
//! `docs/plugins.md`.

use std::path::Path;

use serde_json::{Map, Value, json};

use super::id::mcp_server_name;
use super::vars::{self, PluginPaths};
use crate::hooks::HookEvent;

/// The argv prefix that starts a stdio server in a working directory: the
/// MCP loader has no `cwd` field, so `sh` changes into its first argument
/// and `exec`s the rest, which reach the program unchanged.
const CWD_WRAPPER: [&str; 3] = [
    "-c",
    "cd \"$1\" && shift && exec \"$@\"",
    "alter-zero-plugin",
];

/// What the rewrite needs to know about the plugin it is rewriting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NormalizeContext<'a> {
    /// The namespace: the manifest's name.
    pub plugin: &'a str,
    /// The install root and data directory the variables stand for.
    pub paths: PluginPaths<'a>,
    /// agent-plugins.org semantics: the plugin root is every hook's and
    /// stdio server's working directory. Otherwise a process runs where the
    /// session runs, unless a server names a `cwd` (relative to the root).
    pub root_is_cwd: bool,
}

/// A plugin's hooks, rewritten.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NormalizedHooks {
    /// `{"hooks": {Event: [{matcher?, hooks: [{type:"command", command, timeout?, statusMessage?}]}]}}`,
    /// pretty JSON — exactly what `hooks::HooksFile::parse` accepts. `None`:
    /// nothing runnable.
    pub json: Option<String>,
    /// For the detail page: `"PreToolUse · Bash"`, `"Stop"` (one per matcher
    /// group kept).
    pub summary: Vec<String>,
    /// What was left out because alter-zero cannot run it, one entry per
    /// reason with its count: `"UserPromptExpansion event (1)"`,
    /// `"prompt handler (2)"`.
    pub dropped: Vec<String>,
}

/// A plugin's MCP servers, rewritten.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NormalizedMcp {
    /// `{"mcpServers": {"plugin_<p>_<s>": {…}}}`, pretty JSON — what
    /// `mcp::parse_mcp_file` accepts. `None`: no server.
    pub json: Option<String>,
    /// `"github (http)"`, `"db (stdio)"`, under the names the plugin gave.
    pub summary: Vec<String>,
    /// `"ws (unsupported transport ws)"`, `"broken (not an object)"`.
    pub dropped: Vec<String>,
}

/// Merge every hooks document a plugin ships (file contents and inline
/// objects, each either `{hooks:{…}}`, `{description, hooks}` or a bare event
/// map; anything else — `{"modules": […]}` — is ignored) into one.
///
/// Kept: the events alter-zero fires (`hooks::HookEvent`), and handlers of
/// `type: "command"` (or no type) with a non-empty command, no `if` and no
/// `powershell` shell. A shell-form command keeps the plugin variables as
/// written, for the shell to expand from the hook's environment, and, when
/// `root_is_cwd`, is prefixed `cd '<root>' && `. An agent-plugins.org 1.1
/// draft handler (`{command, args}`, no matcher level) becomes
/// `<command> <args…>`, each word expanded and sh-quoted, under no matcher. `async` is accepted and
/// ignored: alter-zero runs every hook synchronously.
#[must_use]
pub fn normalize_hooks(documents: &[Value], ctx: &NormalizeContext<'_>) -> NormalizedHooks {
    let mut events: Map<String, Value> = Map::new();
    let mut summary = Vec::new();
    let mut dropped = Tally::default();
    for map in documents.iter().filter_map(event_map) {
        for (event, entries) in map {
            let Some(entries) = entries.as_array() else {
                continue;
            };
            if HookEvent::from_name(event).is_none() {
                let handlers = entries.iter().map(handler_count).sum();
                dropped.add(&format!("{event} event"), handlers);
                continue;
            }
            let mut groups = Vec::new();
            let mut draft = Vec::new();
            for entry in entries {
                if entry.get("hooks").is_some() {
                    groups.extend(matcher_group(entry, ctx, &mut dropped));
                } else if let Some(handler) = handler(entry, ctx, &mut dropped) {
                    // agent-plugins.org 1.1: handlers straight under the
                    // event, no matcher level.
                    draft.push(handler);
                }
            }
            if !draft.is_empty() {
                groups.push(json!({ "hooks": draft }));
            }
            for group in &groups {
                summary.push(match group.get("matcher").and_then(Value::as_str) {
                    Some(matcher) if !matcher.is_empty() && matcher != "*" => {
                        format!("{event} · {matcher}")
                    }
                    _ => event.clone(),
                });
            }
            if !groups.is_empty() {
                let slot = events
                    .entry(event.clone())
                    .or_insert_with(|| Value::Array(Vec::new()));
                if let Value::Array(list) = slot {
                    list.extend(groups);
                }
            }
        }
    }
    let json = (!events.is_empty()).then(|| pretty(&json!({ "hooks": events })));
    NormalizedHooks {
        json,
        summary,
        dropped: dropped.into_phrases(),
    }
}

/// The event map inside one hooks document: the `hooks` object of a
/// `{hooks:{…}}` / `{description, hooks}` / agent-plugins.org document, or
/// the document itself when every key is an event-shaped name over a list.
/// `None` for anything else — `{"modules": […]}` is a desktop UI module, not
/// hooks at all.
fn event_map(document: &Value) -> Option<&Map<String, Value>> {
    let object = document.as_object()?;
    if let Some(hooks) = object.get("hooks") {
        return hooks.as_object();
    }
    let event_shaped = !object.is_empty()
        && object.iter().all(|(key, value)| {
            key.starts_with(|c: char| c.is_ascii_uppercase()) && value.is_array()
        });
    event_shaped.then_some(object)
}

/// How many handlers an event's entry holds — a matcher group's list, or
/// one for a draft handler — so a dropped event is counted in hooks.
fn handler_count(entry: &Value) -> usize {
    entry
        .get("hooks")
        .and_then(Value::as_array)
        .map_or(1, Vec::len)
}

/// One `{matcher?, hooks: […]}` group with only its runnable handlers, or
/// `None` when none is left.
fn matcher_group(group: &Value, ctx: &NormalizeContext<'_>, dropped: &mut Tally) -> Option<Value> {
    let handlers: Vec<Value> = group
        .get("hooks")
        .and_then(Value::as_array)?
        .iter()
        .filter_map(|entry| handler(entry, ctx, dropped))
        .collect();
    if handlers.is_empty() {
        return None;
    }
    let mut out = Map::new();
    if let Some(matcher) = group.get("matcher").and_then(Value::as_str) {
        out.insert("matcher".into(), Value::String(matcher.to_string()));
    }
    out.insert("hooks".into(), Value::Array(handlers));
    Some(Value::Object(out))
}

/// One handler in the shape `hooks::HooksFile` reads, or `None` (counted in
/// `dropped`) when alter-zero cannot run it as its author meant.
fn handler(entry: &Value, ctx: &NormalizeContext<'_>, dropped: &mut Tally) -> Option<Value> {
    let Some(object) = entry.as_object() else {
        dropped.add("handler that is not an object", 1);
        return None;
    };
    match object.get("type").and_then(Value::as_str) {
        None | Some("command") => {}
        Some(other) => {
            dropped.add(&format!("{other} handler"), 1);
            return None;
        }
    }
    if object.get("if").is_some_and(|value| !value.is_null()) {
        dropped.add("if condition", 1);
        return None;
    }
    if object.get("shell").and_then(Value::as_str) == Some("powershell") {
        dropped.add("powershell handler", 1);
        return None;
    }
    let Some(command) = object
        .get("command")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|command| !command.is_empty())
    else {
        dropped.add("handler without a command", 1);
        return None;
    };
    let mut command = match object.get("args").and_then(Value::as_array) {
        // The exec form: one program and its words, quoted for `sh`.
        Some(args) => {
            let mut line =
                vars::sh_quote(&vars::expand(&absolute_command(command, ctx), &ctx.paths));
            for arg in args.iter().filter_map(Value::as_str) {
                line.push(' ');
                line.push_str(&vars::sh_quote(&vars::expand(arg, &ctx.paths)));
            }
            line
        }
        // The shell form names the plugin variables as written: the hook's
        // process has them in its environment, and the shell expands them
        // with the author's own quoting. Pasting the path in would split it
        // at a space and run whatever `$(…)` it holds.
        None => command.to_string(),
    };
    if ctx.root_is_cwd {
        command = format!(
            "cd {} && {command}",
            vars::sh_quote(&ctx.paths.root.to_string_lossy())
        );
    }
    let mut out = Map::new();
    out.insert("type".into(), Value::String("command".into()));
    out.insert("command".into(), Value::String(command));
    if let Some(timeout) = object.get("timeout").and_then(timeout_secs) {
        out.insert("timeout".into(), Value::from(timeout));
    }
    if let Some(message) = object.get("statusMessage").and_then(Value::as_str) {
        out.insert("statusMessage".into(), Value::String(message.to_string()));
    }
    Some(Value::Object(out))
}

/// A `timeout` in whole seconds; a fraction rounds up so a hook is never
/// given less time than it asked for.
fn timeout_secs(value: &Value) -> Option<u64> {
    value.as_u64().or_else(|| {
        value
            .as_f64()
            .filter(|secs| secs.is_finite() && *secs > 0.0)
            .map(|secs| secs.ceil() as u64)
    })
}

/// A `./`-relative program made absolute under the plugin root; anything
/// else (a bare name on `PATH`, an absolute path, a variable) as written.
fn absolute_command(command: &str, ctx: &NormalizeContext<'_>) -> String {
    command.strip_prefix("./").map_or_else(
        || command.to_string(),
        |rest| ctx.paths.root.join(rest).to_string_lossy().into_owned(),
    )
}

/// Merge every MCP document (wrapped `{mcpServers:{…}}`, a bare `{name:{…}}`
/// map, or the agent-plugins.org `{$schema, mcpServers}`), later names
/// replacing earlier.
///
/// Per server: `streamable-http` is `http`; `http`/`sse` keep `url` and
/// string `headers`; stdio keeps `command`, string `args` and string `env`
/// and gains the four plugin variables last; a `./` command is made absolute
/// under the root; an explicit `cwd`, or `root_is_cwd`, wraps the server as
/// `sh -c 'cd "$1" && shift && exec "$@"' alter-zero-plugin <cwd> <command>
/// <args…>` (the MCP loader has no working-directory field); the plugin
/// variables are expanded everywhere and every other `${…}` left for the
/// loader; a remote server's `oauth` block and server-level `scopes` become
/// the loader's `oauth` (see `oauth`); other unknown keys (`env_vars`,
/// timeouts, icons) are dropped; `ws` and unknown transports are dropped and
/// named. Server names
/// become `plugin_<plugin>_<server>`.
#[must_use]
pub fn normalize_mcp(documents: &[Value], ctx: &NormalizeContext<'_>) -> NormalizedMcp {
    // Keyed by the plugin's own server name so a later document replaces an
    // earlier one in place; the value is the rewritten entry and its
    // transport word.
    let mut kept: Map<String, Value> = Map::new();
    let mut transports: Vec<(String, &'static str)> = Vec::new();
    let mut dropped = Vec::new();
    for map in documents.iter().filter_map(server_map) {
        for (name, entry) in map {
            if name == "$schema" {
                continue;
            }
            match server(entry, ctx) {
                Ok((config, transport)) => {
                    kept.insert(name.clone(), config);
                    match transports.iter_mut().find(|(known, _)| known == name) {
                        Some(slot) => slot.1 = transport,
                        None => transports.push((name.clone(), transport)),
                    }
                }
                Err(reason) => dropped.push(format!("{name} ({reason})")),
            }
        }
    }
    if kept.is_empty() {
        return NormalizedMcp {
            dropped,
            ..NormalizedMcp::default()
        };
    }
    let servers: Map<String, Value> = kept
        .into_iter()
        .map(|(name, config)| (mcp_server_name(ctx.plugin, &name), config))
        .collect();
    NormalizedMcp {
        json: Some(pretty(&json!({ "mcpServers": servers }))),
        summary: transports
            .into_iter()
            .map(|(name, transport)| format!("{name} ({transport})"))
            .collect(),
        dropped,
    }
}

/// The server map inside one MCP document: `mcpServers` when the document
/// has it, else the document itself (Claude Code lets `.mcp.json` drop the
/// wrapper).
fn server_map(document: &Value) -> Option<&Map<String, Value>> {
    let object = document.as_object()?;
    match object.get("mcpServers") {
        Some(servers) => servers.as_object(),
        None => Some(object),
    }
}

/// One server rewritten for `mcp::parse_mcp_file`, with its transport word,
/// or why it was left out.
fn server(entry: &Value, ctx: &NormalizeContext<'_>) -> Result<(Value, &'static str), String> {
    let object = entry.as_object().ok_or("not an object")?;
    let expand = |text: &str| vars::expand(text, &ctx.paths);
    let strings = |key: &str| -> Map<String, Value> {
        object
            .get(key)
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
            .filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), Value::String(expand(v)))))
            .collect()
    };
    let kind = object.get("type").and_then(Value::as_str);
    let remote = match kind {
        Some("http" | "streamable-http" | "streamableHttp") => Some(Some("http")),
        Some("sse") => Some(Some("sse")),
        Some("stdio") => None,
        Some(other) => return Err(format!("unsupported transport {other}")),
        None if object.contains_key("command") => None,
        // No type and a URL: left untyped, so the loader keeps its
        // streamable-HTTP-then-SSE fallback.
        None if object.contains_key("url") => Some(None),
        None => return Err("no command or url".to_string()),
    };
    if let Some(kind) = remote {
        let url = object
            .get("url")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|url| !url.is_empty())
            .ok_or("no url")?;
        let mut out = Map::new();
        if let Some(kind) = kind {
            out.insert("type".into(), Value::String(kind.into()));
        }
        out.insert("url".into(), Value::String(expand(url)));
        let headers = strings("headers");
        if !headers.is_empty() {
            out.insert("headers".into(), Value::Object(headers));
        }
        if let Some(oauth) = oauth(object, &expand) {
            out.insert("oauth".into(), Value::Object(oauth));
        }
        return Ok((Value::Object(out), kind.unwrap_or("http")));
    }
    let command = object
        .get("command")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|command| !command.is_empty())
        .ok_or("no command")?;
    let mut command = expand(&absolute_command(command, ctx));
    let mut args: Vec<Value> = object
        .get("args")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(|arg| Value::String(expand(arg)))
        .collect();
    let mut env = strings("env");
    for (name, value) in vars::env_pairs(&ctx.paths) {
        env.insert(name.to_string(), Value::String(value));
    }
    let cwd = object
        .get("cwd")
        .and_then(Value::as_str)
        .map(|cwd| resolve_cwd(&expand(cwd), ctx.paths.root))
        .or_else(|| {
            ctx.root_is_cwd
                .then(|| ctx.paths.root.to_string_lossy().into_owned())
        });
    if let Some(cwd) = cwd {
        let mut wrapped: Vec<Value> = CWD_WRAPPER
            .iter()
            .map(|part| Value::String((*part).to_string()))
            .collect();
        wrapped.push(Value::String(cwd));
        wrapped.push(Value::String(command));
        wrapped.append(&mut args);
        command = "sh".to_string();
        args = wrapped;
    }
    let mut out = Map::new();
    out.insert("command".into(), Value::String(command));
    if !args.is_empty() {
        out.insert("args".into(), Value::Array(args));
    }
    out.insert("env".into(), Value::Object(env));
    Ok((Value::Object(out), "stdio"))
}

/// A remote server's sign-in client in the spelling the MCP loader reads —
/// `clientId`, `clientSecret`, `callbackPort`, `scopes` — from an `oauth`
/// block in either spelling (the snake_case one winning, as in the
/// reference implementation) and a server-level `scopes` list, which an
/// `oauth.scopes` outranks. A value the loader would refuse — a number for
/// an id, a port out of range — and an id or secret still reading
/// `<PLACEHOLDER>` are left out, so one bad field never costs the server.
/// `None` when nothing is left.
fn oauth(
    server: &Map<String, Value>,
    expand: &dyn Fn(&str) -> String,
) -> Option<Map<String, Value>> {
    let block = server.get("oauth").and_then(Value::as_object);
    let field = |snake: &str, camel: &str| {
        block.and_then(|block| block.get(snake).or_else(|| block.get(camel)))
    };
    let text = |snake: &str, camel: &str| {
        field(snake, camel)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty() && !is_placeholder(text))
            .map(|text| Value::String(expand(text)))
    };
    let mut out = Map::new();
    if let Some(id) = text("client_id", "clientId") {
        out.insert("clientId".into(), id);
    }
    if let Some(secret) = text("client_secret", "clientSecret") {
        out.insert("clientSecret".into(), secret);
    }
    if let Some(port) = field("callback_port", "callbackPort")
        .and_then(Value::as_u64)
        .and_then(|port| u16::try_from(port).ok())
        .filter(|port| *port != 0)
    {
        out.insert("callbackPort".into(), Value::from(port));
    }
    if let Some(scopes) = block
        .and_then(|block| block.get("scopes"))
        .and_then(scope_list)
        .or_else(|| server.get("scopes").and_then(scope_list))
    {
        out.insert("scopes".into(), Value::String(scopes));
    }
    (!out.is_empty()).then_some(out)
}

/// Scopes as one space-separated string, from a string or a list of them;
/// `None` for anything else or nothing.
fn scope_list(value: &Value) -> Option<String> {
    let words: Vec<&str> = match value {
        Value::String(text) => text.split_whitespace().collect(),
        Value::Array(items) => {
            let mut words = Vec::new();
            for item in items {
                words.extend(item.as_str()?.split_whitespace());
            }
            words
        }
        _ => return None,
    };
    (!words.is_empty()).then(|| words.join(" "))
}

/// `<GMAIL_PUBLIC_CLIENT_ID>`: a value a marketplace left for its own build
/// to fill in, which no provider would accept.
fn is_placeholder(text: &str) -> bool {
    text.len() > 2
        && text.starts_with('<')
        && text.ends_with('>')
        && !text.contains(char::is_whitespace)
}

/// A server's `cwd` as an absolute path: `.` is the root, a relative path is
/// below it, an absolute one stays.
fn resolve_cwd(cwd: &str, root: &Path) -> String {
    let relative = cwd.trim_start_matches("./");
    if Path::new(cwd).is_absolute() {
        cwd.to_string()
    } else if relative.is_empty() || relative == "." {
        root.to_string_lossy().into_owned()
    } else {
        root.join(relative).to_string_lossy().into_owned()
    }
}

/// Claude Code tool names a plugin agent's `tools:` may list, rewritten to
/// alter-zero's: `Glob`, `Grep`, `LS`, `BashOutput` and `KillShell` are
/// `Bash`; `MultiEdit` and `NotebookEdit` are `Edit`; `NotebookRead` is
/// `Read`; `WebFetch` and `WebSearch` are `Bash` and `Skill` (the shipped
/// web skills run through both); `Task` is `Agent`. Names alter-zero has
/// pass through as written (MCP wire names and `*` globs included), a
/// permission-rule suffix (`Bash(git *)`) is dropped, and anything else is
/// left out. The result is comma-separated, deduplicated, in order.
#[must_use]
pub fn map_agent_tools(tools: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut push = |name: &str| {
        if !out.iter().any(|known| known.eq_ignore_ascii_case(name)) {
            out.push(name.to_string());
        }
    };
    for entry in tool_entries(tools) {
        // `Bash(git *)` is a permission rule; the tool is its head.
        let name = entry.split('(').next().unwrap_or_default().trim();
        match name.to_ascii_lowercase().as_str() {
            "" => {}
            "glob" | "grep" | "ls" | "bashoutput" | "killshell" => push("Bash"),
            "multiedit" | "notebookedit" => push("Edit"),
            "notebookread" => push("Read"),
            "webfetch" | "websearch" => {
                push("Bash");
                push("Skill");
            }
            "task" => push("Agent"),
            lower if KNOWN_TOOLS.contains(&lower) || lower.starts_with("mcp__") || lower == "*" => {
                push(name);
            }
            _ => {}
        }
    }
    out.join(", ")
}

/// The tools alter-zero has, by wire name — what a `tools:` list may name
/// as it is.
const KNOWN_TOOLS: [&str; 19] = [
    "bash",
    "read",
    "write",
    "edit",
    "skill",
    "agent",
    "askuserquestion",
    "taskcreate",
    "taskget",
    "tasklist",
    "taskupdate",
    "bashsend",
    "bashwait",
    "bashkill",
    "bashlist",
    "agentsend",
    "agentoutput",
    "agentkill",
    "agentlist",
];

/// The entries of a `tools:` value in any of its spellings — `A, B`,
/// `[A, B]`, a folded YAML list `- A - B` — split on commas, newlines and
/// whitespace outside parentheses, so `Bash(git push:*)` stays one entry.
fn tool_entries(tools: &str) -> Vec<String> {
    let mut entries = Vec::new();
    let mut current = String::new();
    let mut depth = 0_usize;
    for c in tools.chars() {
        match c {
            '(' => {
                depth += 1;
                current.push(c);
            }
            ')' => {
                depth = depth.saturating_sub(1);
                current.push(c);
            }
            c if depth == 0 && (c == ',' || c.is_whitespace()) => {
                entries.push(std::mem::take(&mut current));
            }
            c => current.push(c),
        }
    }
    entries.push(current);
    entries
        .into_iter()
        .map(|entry| {
            entry
                .trim_matches(['[', ']', '"', '\''])
                .trim_start_matches('-')
                .trim()
                .to_string()
        })
        .filter(|entry| !entry.is_empty())
        .collect()
}

/// Dropped things counted per reason, in the order first seen.
#[derive(Default)]
struct Tally(Vec<(String, usize)>);

impl Tally {
    fn add(&mut self, reason: &str, count: usize) {
        match self.0.iter_mut().find(|(known, _)| known == reason) {
            Some(slot) => slot.1 += count,
            None => self.0.push((reason.to_string(), count)),
        }
    }

    fn into_phrases(self) -> Vec<String> {
        self.0
            .into_iter()
            .map(|(reason, count)| format!("{reason} ({count})"))
            .collect()
    }
}

/// Pretty JSON with a closing newline — the generated files are read by
/// people too.
fn pretty(value: &Value) -> String {
    let mut text = serde_json::to_string_pretty(value).unwrap_or_default();
    text.push('\n');
    text
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use serde_json::json;

    use super::*;
    use crate::hooks::{HandlerKind, HooksFile};
    use crate::mcp::{McpServerConfig, OAuthClientConfig, parse_mcp_file};

    const ROOT: &str = "/cache/m/it's a plugin";
    const DATA: &str = "/data/p-m";

    fn ctx(root_is_cwd: bool) -> NormalizeContext<'static> {
        NormalizeContext {
            plugin: "p",
            paths: PluginPaths {
                root: Path::new(ROOT),
                data: Path::new(DATA),
            },
            root_is_cwd,
        }
    }

    /// The normalized hooks parsed back by the real parser.
    fn parsed(out: &NormalizedHooks) -> HooksFile {
        HooksFile::parse(out.json.as_deref().expect("hooks json")).expect("parses")
    }

    fn commands(file: &HooksFile, event: &str) -> Vec<String> {
        file.hooks
            .get(event)
            .into_iter()
            .flatten()
            .flat_map(|group| &group.hooks)
            .map(|h| h.command.clone().unwrap_or_default())
            .collect()
    }

    // --- hooks: document shapes ------------------------------------------

    #[test]
    fn the_description_wrapper_is_read() {
        let doc = json!({
          "description": "Ralph Loop plugin stop hook for self-referential loops",
          "hooks": { "Stop": [ { "hooks": [ {
            "type": "command",
            "command": "bash \"${CLAUDE_PLUGIN_ROOT}/hooks/stop-hook.sh\""
          } ] } ] }
        });
        let out = normalize_hooks(&[doc], &ctx(false));
        let file = parsed(&out);
        assert_eq!(
            commands(&file, "Stop"),
            ["bash \"${CLAUDE_PLUGIN_ROOT}/hooks/stop-hook.sh\""]
        );
        assert_eq!(file.hooks["Stop"][0].matcher, None);
        assert_eq!(out.summary, ["Stop"]);
        assert!(out.dropped.is_empty());
    }

    #[test]
    fn a_bare_event_map_is_read() {
        let doc = json!({ "PostToolUse": [ { "matcher": "Write|Edit",
            "hooks": [ { "type": "command", "command": "\"${PLUGIN_ROOT}/scripts/format.sh\"" } ] } ] });
        let out = normalize_hooks(&[doc], &ctx(false));
        let file = parsed(&out);
        assert_eq!(
            commands(&file, "PostToolUse"),
            ["\"${PLUGIN_ROOT}/scripts/format.sh\""]
        );
        assert_eq!(
            file.hooks["PostToolUse"][0].matcher.as_deref(),
            Some("Write|Edit")
        );
        assert_eq!(out.summary, ["PostToolUse · Write|Edit"]);
    }

    #[test]
    fn a_document_that_is_no_event_map_is_ignored() {
        let out = normalize_hooks(
            &[
                json!({ "modules": ["./register.tsx"] }),
                json!({}),
                json!([1]),
                json!("x"),
            ],
            &ctx(false),
        );
        assert_eq!(out, NormalizedHooks::default());
    }

    #[test]
    fn several_documents_merge_per_event_in_order() {
        let a = json!({ "hooks": { "Stop": [ { "hooks": [ { "command": "one" } ] } ] } });
        let b = json!({ "Stop": [ { "hooks": [ { "command": "two" } ] } ],
                        "PreToolUse": [ { "matcher": "Bash", "hooks": [ { "command": "three" } ] } ] });
        let out = normalize_hooks(&[a, b], &ctx(false));
        let file = parsed(&out);
        assert_eq!(commands(&file, "Stop"), ["one", "two"]);
        assert_eq!(commands(&file, "PreToolUse"), ["three"]);
        assert_eq!(out.summary, ["Stop", "Stop", "PreToolUse · Bash"]);
    }

    // --- hooks: what is kept and dropped ----------------------------------

    #[test]
    fn the_claude_security_file_keeps_what_can_run() {
        let doc = json!({
          "description": "Shows the banner…",
          "hooks": {
            "UserPromptExpansion": [ { "matcher": "^claude-security:claude-security$",
              "hooks": [ { "type": "command", "command": "sh \"${CLAUDE_PLUGIN_ROOT}/hooks/hooks.sh\" banner" } ] } ],
            "PostToolUse": [ { "matcher": "Bash", "hooks": [
              { "type": "command", "command": "sh \"${CLAUDE_PLUGIN_ROOT}/hooks/hooks.sh\" metrics || true",
                "if": "Bash(py* *claude-security*scripts/*.py *)", "async": true, "asyncRewake": true, "timeout": 10 },
              { "type": "command", "command": "sh \"${CLAUDE_PLUGIN_ROOT}/hooks/hooks.sh\" tip push || true",
                "if": "Bash(git push *)", "timeout": 5 } ] } ],
            "PermissionRequest": [ { "matcher": "AskUserQuestion", "hooks": [
              { "type": "command", "command": "sh \"${CLAUDE_PLUGIN_ROOT}/hooks/hooks.sh\" unanswered || true",
                "timeout": 90, "statusMessage": "Checking…" } ] } ]
          }
        });
        let out = normalize_hooks(&[doc], &ctx(false));
        let file = parsed(&out);
        assert!(!file.hooks.contains_key("UserPromptExpansion"));
        assert!(!file.hooks.contains_key("PostToolUse"));
        let handler = &file.hooks["PermissionRequest"][0].hooks[0];
        assert_eq!(handler.kind, HandlerKind::Command);
        assert_eq!(
            handler.command.as_deref(),
            Some("sh \"${CLAUDE_PLUGIN_ROOT}/hooks/hooks.sh\" unanswered || true")
        );
        assert_eq!(handler.timeout, Some(90));
        assert_eq!(handler.status_message.as_deref(), Some("Checking…"));
        assert_eq!(out.summary, ["PermissionRequest · AskUserQuestion"]);
        assert_eq!(
            out.dropped,
            ["UserPromptExpansion event (1)", "if condition (2)"]
        );
    }

    #[test]
    fn async_is_kept_and_run_synchronously() {
        let doc = json!({ "Stop": [ { "hooks": [ { "type": "command", "command": "x", "async": true } ] } ] });
        let out = normalize_hooks(&[doc], &ctx(false));
        let file = parsed(&out);
        assert_eq!(commands(&file, "Stop"), ["x"]);
        assert!(!file.hooks["Stop"][0].hooks[0].is_async);
        assert!(out.dropped.is_empty());
    }

    #[test]
    fn handlers_that_cannot_run_are_dropped_and_counted() {
        let doc = json!({ "Stop": [ { "hooks": [
            { "type": "prompt", "prompt": "Is it done?" },
            { "type": "agent" },
            { "type": "command", "command": "  " },
            { "type": "command" },
            { "type": "command", "command": "Get-Item", "shell": "powershell" },
            { "type": "command", "command": "kept", "shell": "bash" }
        ] } ] });
        let out = normalize_hooks(&[doc], &ctx(false));
        assert_eq!(commands(&parsed(&out), "Stop"), ["kept"]);
        assert_eq!(
            out.dropped,
            [
                "prompt handler (1)",
                "agent handler (1)",
                "handler without a command (2)",
                "powershell handler (1)"
            ]
        );
    }

    #[test]
    fn nothing_runnable_is_no_file() {
        let doc = json!({ "UserPromptExpansion": [ { "hooks": [ { "command": "x" } ] } ] });
        let out = normalize_hooks(&[doc], &ctx(false));
        assert_eq!(out.json, None);
        assert!(out.summary.is_empty());
        assert_eq!(out.dropped, ["UserPromptExpansion event (1)"]);
    }

    #[test]
    fn root_is_cwd_runs_each_command_in_the_root() {
        let doc = json!({ "hooks": { "PostToolUse": [ { "matcher": "Write|Edit",
            "hooks": [ { "type": "command", "command": "./scripts/post_write_figma_parity_check.sh" } ] } ] } });
        let out = normalize_hooks(&[doc], &ctx(true));
        let command = format!(
            "cd {} && ./scripts/post_write_figma_parity_check.sh",
            crate::plugins::vars::sh_quote(ROOT)
        );
        assert_eq!(
            commands(&parsed(&out), "PostToolUse"),
            std::slice::from_ref(&command)
        );
        assert!(command.starts_with(r"cd '/cache/m/it'\''s a plugin' && "));
    }

    #[test]
    fn the_agent_plugins_draft_handler_becomes_one_command() {
        let doc = json!({
            "$schema": "https://agent-plugins.org/schemas/1.1.0/hooks.schema.json",
            "hooks": { "PreToolUse": [ { "command": "./bin/check", "args": ["--data", "${PLUGIN_DATA}/x", "a b"] } ] }
        });
        let out = normalize_hooks(&[doc], &ctx(false));
        let file = parsed(&out);
        let group = &file.hooks["PreToolUse"][0];
        assert_eq!(group.matcher, None);
        assert_eq!(
            commands(&file, "PreToolUse"),
            [format!(
                "{} '--data' '{DATA}/x' 'a b'",
                crate::plugins::vars::sh_quote(&format!("{ROOT}/bin/check"))
            )]
        );
        assert_eq!(out.summary, ["PreToolUse"]);
    }

    #[cfg(unix)]
    #[test]
    fn a_kept_command_runs_under_sh_in_the_root() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join("it's a plugin");
        std::fs::create_dir(&root).expect("root");
        let data = dir.path().join("data");
        let ctx = NormalizeContext {
            plugin: "p",
            paths: PluginPaths {
                root: &root,
                data: &data,
            },
            root_is_cwd: true,
        };
        let doc = json!({ "Stop": [ { "hooks": [ { "command": "pwd -P; printf '%s' \"${PLUGIN_DATA}\"" } ] } ] });
        let file = parsed(&normalize_hooks(&[doc], &ctx));
        let command = commands(&file, "Stop").remove(0);
        let out = std::process::Command::new("sh")
            .arg("-c")
            .arg(command)
            .envs(vars::env_pairs(&ctx.paths))
            .output()
            .expect("sh");
        let text = String::from_utf8_lossy(&out.stdout).into_owned();
        let canonical = root.canonicalize().expect("canonical");
        assert_eq!(text, format!("{}\n{}", canonical.display(), data.display()));
    }

    #[test]
    fn a_shell_form_command_names_the_plugin_variables_as_written() {
        let doc = json!({ "PreToolUse": [ { "matcher": "Bash", "hooks": [
            { "command": "python3 ${CLAUDE_PLUGIN_ROOT}/hooks/a.py --state ${PLUGIN_DATA}" } ] } ] });
        let out = normalize_hooks(&[doc], &ctx(false));
        assert_eq!(
            commands(&parsed(&out), "PreToolUse"),
            ["python3 ${CLAUDE_PLUGIN_ROOT}/hooks/a.py --state ${PLUGIN_DATA}"]
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_root_with_spaces_and_a_substitution_is_never_run_as_shell_text() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join("Jane Doe $(touch pwned)");
        std::fs::create_dir(&root).expect("root");
        let data = dir.path().join("data");
        let ctx = NormalizeContext {
            plugin: "p",
            paths: PluginPaths {
                root: &root,
                data: &data,
            },
            root_is_cwd: false,
        };
        let doc = json!({ "Stop": [ { "hooks": [
            { "command": "printf '[%s]' \"${CLAUDE_PLUGIN_ROOT}\"" } ] } ] });
        let command = commands(&parsed(&normalize_hooks(&[doc], &ctx)), "Stop").remove(0);
        let out = std::process::Command::new("sh")
            .arg("-c")
            .arg(command)
            .current_dir(dir.path())
            .envs(vars::env_pairs(&ctx.paths))
            .output()
            .expect("sh");
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            format!("[{}]", root.display())
        );
        assert!(
            !dir.path().join("pwned").exists(),
            "nothing in the path ran"
        );
    }

    // --- MCP --------------------------------------------------------------

    fn servers(out: &NormalizedMcp) -> Vec<(String, McpServerConfig)> {
        let file = parse_mcp_file(out.json.as_deref().expect("mcp json"));
        assert!(file.errors.is_empty(), "{:?}", file.errors);
        file.servers
    }

    fn plugin_env(extra: &[(&str, &str)]) -> std::collections::BTreeMap<String, String> {
        let mut env: std::collections::BTreeMap<String, String> = extra
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        for (k, v) in [
            ("PLUGIN_ROOT", ROOT),
            ("CLAUDE_PLUGIN_ROOT", ROOT),
            ("PLUGIN_DATA", DATA),
            ("CLAUDE_PLUGIN_DATA", DATA),
        ] {
            env.insert(k.to_string(), v.to_string());
        }
        env
    }

    #[test]
    fn a_wrapped_stdio_server_gains_the_plugin_variables() {
        let doc = json!({ "mcpServers": { "discord": {
            "command": "bun",
            "args": ["run", "--cwd", "${CLAUDE_PLUGIN_ROOT}", "--shell=bun"],
            "env": { "PLUGIN_ROOT": "overridden", "TOKEN": "${DISCORD_TOKEN}", "N": 3 }
        } } });
        let out = normalize_mcp(&[doc], &ctx(false));
        assert_eq!(
            servers(&out),
            [(
                "plugin_p_discord".to_string(),
                McpServerConfig::Stdio {
                    command: "bun".into(),
                    args: vec![
                        "run".into(),
                        "--cwd".into(),
                        ROOT.into(),
                        "--shell=bun".into()
                    ],
                    env: plugin_env(&[("TOKEN", "${DISCORD_TOKEN}")]),
                }
            )]
        );
        assert_eq!(out.summary, ["discord (stdio)"]);
        assert!(out.dropped.is_empty());
    }

    #[test]
    fn a_bare_http_map_keeps_its_headers_for_the_loader() {
        let doc = json!({ "github": { "type": "http", "url": "https://api.githubcopilot.com/mcp/",
            "headers": { "Authorization": "Bearer ${GITHUB_PERSONAL_ACCESS_TOKEN}", "X": 1 } } });
        let out = normalize_mcp(&[doc], &ctx(false));
        assert_eq!(
            servers(&out),
            [(
                "plugin_p_github".to_string(),
                McpServerConfig::Http {
                    url: "https://api.githubcopilot.com/mcp/".into(),
                    headers: [(
                        "Authorization".to_string(),
                        "Bearer ${GITHUB_PERSONAL_ACCESS_TOKEN}".to_string()
                    )]
                    .into(),
                    sse_fallback: false,
                    oauth: None,
                }
            )]
        );
        assert_eq!(out.summary, ["github (http)"]);
    }

    #[test]
    fn a_url_without_a_type_keeps_the_sse_fallback() {
        let doc = json!({ "mcpServers": { "notes": { "url": "https://example.com/mcp" } } });
        let out = normalize_mcp(&[doc], &ctx(false));
        assert!(matches!(
            &servers(&out)[0].1,
            McpServerConfig::Http {
                sse_fallback: true,
                ..
            }
        ));
        assert_eq!(out.summary, ["notes (http)"]);
    }

    #[test]
    fn the_agent_plugins_shape_is_read() {
        let doc = json!({
          "$schema": "https://agent-plugins.org/schemas/1.0.0/mcp.schema.json",
          "mcpServers": {
            "deployment-api": { "type": "streamable-http", "url": "https://deploy.example.com/mcp",
                                "headers": { "X-Tenant": "public-tenant" } },
            "legacy-events": { "type": "sse", "url": "https://legacy.example.com/sse" }
          }
        });
        let out = normalize_mcp(&[doc], &ctx(true));
        let got = servers(&out);
        assert_eq!(got[0].0, "plugin_p_deployment-api");
        assert!(matches!(
            &got[0].1,
            McpServerConfig::Http { sse_fallback: false, url, headers, .. }
                if url == "https://deploy.example.com/mcp" && headers["X-Tenant"] == "public-tenant"
        ));
        assert_eq!(got[1].0, "plugin_p_legacy-events");
        assert!(
            matches!(&got[1].1, McpServerConfig::Sse { url, .. } if url == "https://legacy.example.com/sse")
        );
        assert_eq!(
            out.summary,
            ["deployment-api (http)", "legacy-events (sse)"]
        );
    }

    #[test]
    fn a_codex_server_runs_in_its_cwd_without_its_extras() {
        let doc = json!({ "mcpServers": { "codex-security": {
            "command": "./scripts/launch_codex_security_mcp",
            "args": ["--stdio"],
            "cwd": ".",
            "env_vars": ["CODEX_HOME", "OPENAI_API_KEY"],
            "startup_timeout_sec": 120,
            "tool_timeout_sec": 349200,
            "icons": [ { "src": "x.png" } ]
        } } });
        let out = normalize_mcp(&[doc], &ctx(false));
        assert_eq!(
            servers(&out),
            [(
                "plugin_p_codex-security".to_string(),
                McpServerConfig::Stdio {
                    command: "sh".into(),
                    args: vec![
                        "-c".into(),
                        "cd \"$1\" && shift && exec \"$@\"".into(),
                        "alter-zero-plugin".into(),
                        ROOT.into(),
                        format!("{ROOT}/scripts/launch_codex_security_mcp"),
                        "--stdio".into(),
                    ],
                    env: plugin_env(&[]),
                }
            )]
        );
    }

    #[test]
    fn root_is_cwd_wraps_every_stdio_server_and_resolves_a_relative_cwd() {
        let doc = json!({
          "$schema": "https://agent-plugins.org/schemas/1.0.0/mcp.schema.json",
          "mcpServers": {
            "a": { "type": "stdio", "command": "node", "args": ["./mcp/server.bundle.mjs"] },
            "b": { "type": "stdio", "command": "./bin/v", "cwd": "${PLUGIN_DATA}/work" },
            "c": { "command": "x", "cwd": "./sub" }
          }
        });
        let out = normalize_mcp(&[doc], &ctx(true));
        let argv: Vec<Vec<String>> = servers(&out)
            .into_iter()
            .map(|(_, config)| match config {
                McpServerConfig::Stdio { command, args, .. } => {
                    std::iter::once(command).chain(args).collect()
                }
                other => panic!("{other:?}"),
            })
            .collect();
        let wrapper = |cwd: &str, rest: &[&str]| -> Vec<String> {
            [
                "sh",
                "-c",
                "cd \"$1\" && shift && exec \"$@\"",
                "alter-zero-plugin",
                cwd,
            ]
            .iter()
            .chain(rest)
            .map(|s| (*s).to_string())
            .collect()
        };
        assert_eq!(argv[0], wrapper(ROOT, &["node", "./mcp/server.bundle.mjs"]));
        assert_eq!(
            argv[1],
            wrapper(&format!("{DATA}/work"), &[&format!("{ROOT}/bin/v")])
        );
        assert_eq!(argv[2], wrapper(&format!("{ROOT}/sub"), &["x"]));
    }

    #[test]
    fn a_relative_command_is_made_absolute_without_a_cwd() {
        let doc = json!({ "s": { "command": "./scripts/launch" } });
        let out = normalize_mcp(&[doc], &ctx(false));
        assert!(matches!(
            &servers(&out)[0].1,
            McpServerConfig::Stdio { command, args, .. }
                if *command == format!("{ROOT}/scripts/launch") && args.is_empty()
        ));
    }

    #[test]
    fn codex_oauth_placeholders_do_not_fail_the_server() {
        let doc = json!({ "mcpServers": { "gmail": {
            "type": "http",
            "url": "https://gmailmcp.googleapis.com/mcp/v1",
            "scopes": ["https://mail.google.com/", "https://www.googleapis.com/auth/gmail.modify"],
            "oauth": { "callback_port": 12798, "client_id": "<GMAIL_PUBLIC_CLIENT_ID>",
                       "client_secret": "<GMAIL_CLIENT_SECRET>" },
            "oauth_resource": "x"
        } } });
        let out = normalize_mcp(&[doc], &ctx(false));
        assert_eq!(
            servers(&out)[0].1,
            McpServerConfig::Http {
                url: "https://gmailmcp.googleapis.com/mcp/v1".into(),
                headers: std::collections::BTreeMap::new(),
                sse_fallback: false,
                oauth: Some(OAuthClientConfig {
                    client_id: None,
                    client_secret: None,
                    callback_port: Some(12798),
                    scopes: Some(
                        "https://mail.google.com/ https://www.googleapis.com/auth/gmail.modify"
                            .into()
                    ),
                }),
            }
        );
        assert!(!out.json.as_deref().unwrap_or_default().contains("GMAIL"));
    }

    #[test]
    fn a_remote_servers_oauth_client_is_carried_in_the_loaders_spelling() {
        let doc = json!({ "mcpServers": { "github": {
            "type": "http",
            "url": "https://api.githubcopilot.com/mcp/",
            "oauth": { "client_id": "Iv23li", "client_secret": "824c9c", "callback_port": 12799,
                       "callback_url": "http://127.0.0.1:12799/callback/x" }
        }, "slack": { "type": "sse", "url": "https://mcp.slack.com/sse",
            "oauth": { "clientId": "1184.1190", "scopes": "chat:write  users:read" } },
           "odd": { "url": "https://odd.example/mcp",
            "oauth": { "client_id": 7, "callback_port": "12799", "scopes": [1] } } } });
        let out = normalize_mcp(&[doc], &ctx(false));
        let got = servers(&out);
        let oauth = |name: &str| match &got.iter().find(|(n, _)| n == name).expect(name).1 {
            McpServerConfig::Http { oauth, .. } | McpServerConfig::Sse { oauth, .. } => {
                oauth.clone()
            }
            other => panic!("{other:?}"),
        };
        assert_eq!(
            oauth("plugin_p_github"),
            Some(OAuthClientConfig {
                client_id: Some("Iv23li".into()),
                client_secret: Some("824c9c".into()),
                callback_port: Some(12799),
                scopes: None,
            })
        );
        assert_eq!(
            oauth("plugin_p_slack"),
            Some(OAuthClientConfig {
                client_id: Some("1184.1190".into()),
                client_secret: None,
                callback_port: None,
                scopes: Some("chat:write users:read".into()),
            })
        );
        // Fields of a shape the loader would refuse are left out rather than
        // costing the server.
        assert_eq!(oauth("plugin_p_odd"), None);
    }

    #[test]
    fn server_level_scopes_alone_make_an_oauth_block() {
        let doc = json!({ "s": { "type": "http", "url": "https://x/mcp", "scopes": ["a", "b"] } });
        let out = normalize_mcp(&[doc], &ctx(false));
        assert!(matches!(
            &servers(&out)[0].1,
            McpServerConfig::Http { oauth: Some(oauth), .. }
                if oauth.scopes.as_deref() == Some("a b") && oauth.client_id.is_none()
        ));
    }

    #[test]
    fn unusable_servers_are_dropped_and_named() {
        let doc = json!({ "mcpServers": {
            "live": { "type": "ws", "url": "wss://x" },
            "odd": { "type": "grpc", "url": "https://x" },
            "broken": "npx thing",
            "nourl": { "type": "http" },
            "nocmd": { "type": "stdio", "command": " " },
            "ok": { "command": "srv" }
        } });
        let out = normalize_mcp(&[doc], &ctx(false));
        assert_eq!(servers(&out).len(), 1);
        assert_eq!(out.summary, ["ok (stdio)"]);
        assert_eq!(
            out.dropped,
            [
                "live (unsupported transport ws)",
                "odd (unsupported transport grpc)",
                "broken (not an object)",
                "nourl (no url)",
                "nocmd (no command)"
            ]
        );
    }

    #[test]
    fn server_names_are_sanitized() {
        let doc = json!({ "my.server": { "command": "x" }, "a b/c": { "command": "y" } });
        let out = normalize_mcp(&[doc], &ctx(false));
        let names: Vec<String> = servers(&out).into_iter().map(|(n, _)| n).collect();
        assert_eq!(names, ["plugin_p_my_server", "plugin_p_a_b_c"]);
        assert_eq!(out.summary, ["my.server (stdio)", "a b/c (stdio)"]);
    }

    #[test]
    fn a_later_document_replaces_a_same_named_server() {
        let a = json!({ "mcpServers": { "db": { "command": "old" }, "keep": { "command": "k" } } });
        let b = json!({ "db": { "type": "sse", "url": "https://new" } });
        let out = normalize_mcp(&[a, b], &ctx(false));
        let got = servers(&out);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].0, "plugin_p_db");
        assert!(matches!(&got[0].1, McpServerConfig::Sse { url, .. } if url == "https://new"));
        assert_eq!(out.summary, ["db (sse)", "keep (stdio)"]);
    }

    #[test]
    fn other_placeholders_are_left_for_the_loader() {
        let doc = json!({ "telegram": { "command": "bun",
            "args": ["${TELEGRAM_STATE_DIR:-/tmp/x}"],
            "env": { "TOKEN": "${GITHUB_PERSONAL_ACCESS_TOKEN}" } } });
        let out = normalize_mcp(&[doc], &ctx(false));
        assert!(matches!(
            &servers(&out)[0].1,
            McpServerConfig::Stdio { args, env, .. }
                if args == &["${TELEGRAM_STATE_DIR:-/tmp/x}"]
                    && env["TOKEN"] == "${GITHUB_PERSONAL_ACCESS_TOKEN}"
        ));
    }

    #[test]
    fn no_server_is_no_file() {
        let out = normalize_mcp(
            &[
                json!({}),
                json!({ "mcpServers": {} }),
                json!({ "mcpServers": [1] }),
                json!(3),
            ],
            &ctx(false),
        );
        assert_eq!(out, NormalizedMcp::default());
    }

    #[cfg(unix)]
    #[test]
    fn the_cwd_wrapper_runs_in_the_directory_and_passes_the_arguments() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join("plugin root");
        std::fs::create_dir(&root).expect("root");
        let data = dir.path().join("data");
        let ctx = NormalizeContext {
            plugin: "p",
            paths: PluginPaths {
                root: &root,
                data: &data,
            },
            root_is_cwd: true,
        };
        let doc = json!({ "s": { "command": "sh",
            "args": ["-c", "pwd -P; printf '[%s]\\n' \"$@\"", "inner", "a b", "${PLUGIN_ROOT}"] } });
        let out = normalize_mcp(&[doc], &ctx);
        let (command, args) = match servers(&out).remove(0).1 {
            McpServerConfig::Stdio { command, args, .. } => (command, args),
            other => panic!("{other:?}"),
        };
        let run = std::process::Command::new(command)
            .args(args)
            .output()
            .expect("runs");
        let text = String::from_utf8_lossy(&run.stdout).into_owned();
        let canonical = root.canonicalize().expect("canonical");
        assert_eq!(
            text,
            format!("{}\n[a b]\n[{}]\n", canonical.display(), root.display())
        );
    }

    // --- map_agent_tools --------------------------------------------------

    #[test]
    fn claude_code_tools_map_onto_alter_zeros() {
        assert_eq!(map_agent_tools("Glob, Grep, LS, Read"), "Bash, Read");
        assert_eq!(
            map_agent_tools("[Read, MultiEdit, NotebookEdit, NotebookRead, Write]"),
            "Read, Edit, Write"
        );
        assert_eq!(map_agent_tools("WebFetch, WebSearch"), "Bash, Skill");
        assert_eq!(map_agent_tools("BashOutput KillShell Task"), "Bash, Agent");
    }

    #[test]
    fn known_names_pass_through_and_the_rest_drop() {
        assert_eq!(
            map_agent_tools("- bash\n- mcp__deepwiki__*\n- TodoWrite\n- Skill\n- AskUserQuestion"),
            "bash, mcp__deepwiki__*, Skill, AskUserQuestion"
        );
        assert_eq!(map_agent_tools("TodoWrite, ExitPlanMode"), "");
        assert_eq!(map_agent_tools("*"), "*");
        assert_eq!(map_agent_tools(""), "");
    }

    #[test]
    fn a_rule_suffix_is_dropped_and_duplicates_collapse() {
        assert_eq!(
            map_agent_tools("Bash(git push:*), Bash(gh pr create *), Read(.claude/x.md), Grep"),
            "Bash, Read"
        );
        assert_eq!(map_agent_tools("Read, read, READ"), "Read");
    }
}
