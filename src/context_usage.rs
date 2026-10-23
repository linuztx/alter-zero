//! The `/context` page's pure core: what fills the context window, by
//! category, and what the provider said about the requests that carried it.
//! See `docs/context-usage.md`.
//!
//! [`breakdown`] takes the window **as the request carries it** — the system
//! prompt, the tool schemas exactly as the client serializes them, the
//! `<system-reminder>` text the derived context leads with, the derived
//! messages — and attributes every token to the [`Category`] it belongs to,
//! each with its own [`Part`]s for the page's detail. Nothing here models the
//! request a second time: the reminder's instructions are the text the
//! session stored and its listings are found by the headers their
//! contributors write, the tools by their wire names, the messages by their
//! roles and the origins the derivation noted, so the page cannot describe a
//! context the model does not get. The counter is injected, which is what
//! lets every attribution be checked by hand in the tests (a token is a byte
//! there).
//!
//! [`UsageLedger`] is the other half: the provider's own usage frames,
//! summed — never estimated.
//!
//! [`CountMemo`] keeps a live page cheap: the window is re-counted whenever
//! the conversation moves, and a memo of each text's count means a re-count
//! tokenizes only what is new.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Arc;

use crate::context::{ContextMessage, ContextRole, TextOrigin, TextSegment};
use crate::mcp::{is_mcp_tool, parse_wire_name};
use crate::stream::TokenUsage;

/// One tool the next request offers the model: its wire name and its schema
/// serialized exactly as the client keeps it — what the page counts a tool's
/// share of the window from. The client keeps the chat-completions shape; a
/// wire that translates it (Anthropic, Responses, Ollama) frames the same
/// fields its own way, a few tokens either side.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ToolDefinition {
    /// The wire name the model calls it by (`bash`, `mcp__deepwiki__ask`).
    pub name: String,
    /// The whole Chat Completions `{"type":"function",…}` entry, as JSON.
    pub schema: String,
}

impl ToolDefinition {
    /// The definition of one Chat Completions tool spec — the
    /// `{"type":"function","function":{"name":…}}` entry a client's `tools`
    /// array holds.
    #[must_use]
    pub fn from_spec(spec: &serde_json::Value) -> Self {
        Self {
            name: spec["function"]["name"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            schema: spec.to_string(),
        }
    }

    /// The definitions of a whole spec set, in its order.
    #[must_use]
    pub fn from_specs(specs: &[serde_json::Value]) -> Arc<[Self]> {
        specs.iter().map(Self::from_spec).collect()
    }
}

/// The provider's usage frames, summed: how many requests reported one, and
/// their input, output and cache figures — the page's **Since launch** and
/// **This agent** rows. Real numbers only: a backend that reports no usage
/// (the offline dummy) leaves it empty rather than estimated.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UsageLedger {
    /// Requests that reported usage.
    pub requests: u64,
    /// Their input tokens, cache reads included.
    pub input: u64,
    /// Their output tokens.
    pub output: u64,
    /// The input tokens served from the prompt cache.
    pub cached: u64,
    /// The input tokens written to the prompt cache.
    pub cache_write: u64,
}

impl UsageLedger {
    /// Add one request's usage frame.
    pub fn record(&mut self, usage: &TokenUsage) {
        self.requests = self.requests.saturating_add(1);
        self.input = self.input.saturating_add(usage.input);
        self.output = self.output.saturating_add(usage.output);
        self.cached = self.cached.saturating_add(usage.cached);
        self.cache_write = self.cache_write.saturating_add(usage.cache_write);
    }

    /// The share of all input that was a cache read, `None` before any input
    /// was reported.
    #[must_use]
    pub fn cache_share(&self) -> Option<f64> {
        share(self.cached, self.input)
    }
}

/// `part` over `whole`, `None` for an empty whole.
fn share(part: u64, whole: u64) -> Option<f64> {
    (whole > 0).then(|| part as f64 / whole as f64)
}

/// The share of one request's input that was a cache read — the page's
/// **Cache hit** meter — `None` for a frame reporting no input.
#[must_use]
pub fn cache_share(usage: &TokenUsage) -> Option<f64> {
    share(usage.cached, usage.input)
}

/// What a slice of the context window holds, in the order a request carries
/// them: the system prompt, the tools, the `<system-reminder>`'s sections,
/// then the conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Category {
    /// The backend's system prompt — persona, environment, scratchpad.
    SystemPrompt,
    /// The built-in tool schemas.
    SystemTools,
    /// The MCP servers' tool schemas (`mcp__server__tool`).
    McpTools,
    /// The reminder's `AGENTS.md` instructions section.
    ProjectDocs,
    /// The reminder's skills listing.
    Skills,
    /// The reminder's subagent-type listing.
    AgentTypes,
    /// The reminder's secrets listing — names and context, never a value.
    Secrets,
    /// The derived conversation.
    Messages,
}

impl Category {
    /// Every category, in the order the request carries them.
    pub const ALL: [Self; 8] = [
        Self::SystemPrompt,
        Self::SystemTools,
        Self::McpTools,
        Self::ProjectDocs,
        Self::Skills,
        Self::AgentTypes,
        Self::Secrets,
        Self::Messages,
    ];

    /// The legend's label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::SystemPrompt => "System prompt",
            Self::SystemTools => "System tools",
            Self::McpTools => "MCP tools",
            Self::ProjectDocs => "Project docs",
            Self::Skills => "Skills",
            Self::AgentTypes => "Agent types",
            Self::Secrets => "Secrets",
            Self::Messages => "Messages",
        }
    }

    /// The noun a slice's [`items`](Slice::items) are counted in — singular,
    /// then plural.
    #[must_use]
    pub const fn unit(self) -> (&'static str, &'static str) {
        match self {
            Self::SystemPrompt => ("block", "blocks"),
            Self::SystemTools | Self::McpTools => ("tool", "tools"),
            Self::ProjectDocs => ("file", "files"),
            Self::Skills => ("skill", "skills"),
            Self::AgentTypes => ("type", "types"),
            Self::Secrets => ("secret", "secrets"),
            Self::Messages => ("message", "messages"),
        }
    }
}

/// One row of a slice's detail: a label and its tokens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Part {
    /// What it is — a tool's wire name, a file's path, a skill's name.
    pub label: String,
    /// What it costs.
    pub tokens: usize,
}

/// One category's share of the window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Slice {
    /// Which category.
    pub category: Category,
    /// Its tokens.
    pub tokens: usize,
    /// How many things it holds, in [`Category::unit`]s — the tools, the
    /// files, the messages.
    pub items: usize,
    /// Its breakdown, largest first.
    pub parts: Vec<Part>,
}

/// The whole window, by category: the slices that hold anything, in
/// [`Category::ALL`] order — **Messages** always among them, since an empty
/// conversation is a fact worth stating.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Breakdown {
    /// The non-empty slices, in request order.
    pub slices: Vec<Slice>,
}

impl Breakdown {
    /// Every slice's tokens, summed — the page's headline.
    #[must_use]
    pub fn total(&self) -> usize {
        self.slices.iter().map(|slice| slice.tokens).sum()
    }

    /// The slice for `category`, when the window holds any.
    #[must_use]
    pub fn slice(&self, category: Category) -> Option<&Slice> {
        self.slices.iter().find(|slice| slice.category == category)
    }

    /// The category holding the most tokens — what the page opens on, the
    /// earlier one on a tie. `None` only for an empty breakdown.
    #[must_use]
    pub fn largest(&self) -> Option<Category> {
        self.slices
            .iter()
            .rev()
            .max_by_key(|slice| slice.tokens)
            .map(|slice| slice.category)
    }
}

/// Token counts remembered by text, so re-counting a window that mostly did
/// not change tokenizes only what is new. The page re-counts the whole
/// window each time the conversation moves, and putting a long
/// conversation's every message through the tokenizer again would stall a
/// frame on each commit of a busy turn. A text is known by its length and
/// hash for as long as the passes keep counting it:
/// [`finish_pass`](Self::finish_pass) forgets whatever the pass it closes
/// did not count, so the memo never holds more than two windows' entries.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CountMemo {
    /// What the pass in progress has counted.
    current: HashMap<(usize, u64), usize>,
    /// What the last finished pass counted.
    previous: HashMap<(usize, u64), usize>,
}

impl CountMemo {
    /// `text`'s tokens: remembered when this pass or the last one counted
    /// it, else `count`'s answer, remembered from now on.
    pub fn count(&mut self, text: &str, count: &dyn Fn(&str) -> usize) -> usize {
        let mut hasher = DefaultHasher::new();
        text.hash(&mut hasher);
        let key = (text.len(), hasher.finish());
        if let Some(&tokens) = self.current.get(&key) {
            return tokens;
        }
        let tokens = self.previous.remove(&key).unwrap_or_else(|| count(text));
        self.current.insert(key, tokens);
        tokens
    }

    /// Close a pass: what it counted is what the next one may reuse, and
    /// whatever it did not count is forgotten.
    pub fn finish_pass(&mut self) {
        self.previous = std::mem::take(&mut self.current);
    }
}

/// Everything a request carries, borrowed — the inputs [`breakdown`]
/// attributes.
#[derive(Debug, Clone, Copy, Default)]
pub struct Sources<'a> {
    /// The system prompt, as sent.
    pub system_prompt: Option<&'a str>,
    /// The tools the request offers.
    pub tools: &'a [ToolDefinition],
    /// The `<system-reminder>` the derived context leads with, whole — the
    /// main session's composed block (`reminder::reminder_message`), or a
    /// subagent's briefing exactly as its launch wrapped it.
    pub reminder: Option<&'a str>,
    /// The instructions section that reminder opens with, exactly as the
    /// session stored it (`App::user_instructions`). `None` when it carries
    /// none, as a subagent's briefing never does.
    pub instructions: Option<&'a str>,
    /// The derived conversation **without** the reminder
    /// (`context::context_messages_with_origins`).
    pub messages: &'a [ContextMessage],
    /// Where each user-role text in those messages came from, in message
    /// order — what splits the user's share by origin. A user-role text with
    /// no noted origin counts as typed.
    pub origins: &'a [TextSegment],
    /// What one image attachment is counted as.
    pub image_tokens: usize,
}

/// Attribute every token `sources` carries to its [`Category`], `count`
/// being the token counter (`app::count_tokens` in the app, anything in a
/// test).
#[must_use]
pub fn breakdown(sources: &Sources<'_>, count: &dyn Fn(&str) -> usize) -> Breakdown {
    let mut slices: Vec<Slice> = Vec::new();
    if let Some(prompt) = sources.system_prompt.filter(|p| !p.trim().is_empty()) {
        let parts: Vec<Part> = prompt_blocks(prompt)
            .into_iter()
            .map(|(label, text)| Part {
                label,
                tokens: count(text),
            })
            .collect();
        slices.push(Slice::of(
            Category::SystemPrompt,
            count(prompt),
            parts.len(),
            parts,
        ));
    }
    let (mcp, built_in): (Vec<&ToolDefinition>, Vec<&ToolDefinition>) = sources
        .tools
        .iter()
        .partition(|tool| is_mcp_tool(&tool.name));
    let tool_parts: Vec<Part> = built_in
        .iter()
        .map(|tool| Part {
            label: tool.name.clone(),
            tokens: count(&tool.schema),
        })
        .collect();
    slices.push(Slice::summed(
        Category::SystemTools,
        built_in.len(),
        tool_parts,
    ));
    slices.push(Slice::summed(
        Category::McpTools,
        mcp.len(),
        server_parts(&mcp, count),
    ));
    // What the reminder carries that no section header claims — nothing the
    // session writes, but a token must never fall out of the total. It rides
    // as a user message, so it is counted with the messages.
    let mut stray = 0;
    if let Some(reminder) = sources.reminder.filter(|r| !r.trim().is_empty()) {
        let sections = reminder_sections(reminder, sources.instructions);
        if sections.is_empty() {
            stray = count(reminder);
        }
        for (category, text) in sections {
            let items = section_items(category, text);
            let parts: Vec<Part> = items
                .iter()
                .map(|(label, item)| Part {
                    label: label.clone(),
                    tokens: count(item),
                })
                .collect();
            slices.push(Slice::of(category, count(text), items.len(), parts));
        }
    }
    let message_parts = message_parts(sources, count, stray);
    slices.push(Slice::summed(
        Category::Messages,
        sources.messages.len(),
        message_parts,
    ));
    slices.retain(|slice| slice.tokens > 0 || slice.category == Category::Messages);
    slices.sort_by_key(|slice| slice.category);
    Breakdown { slices }
}

impl Slice {
    /// A slice of `tokens`, its parts sorted largest first (a stable sort, so
    /// ties keep the order the request carries them in).
    fn of(category: Category, tokens: usize, items: usize, mut parts: Vec<Part>) -> Self {
        parts.retain(|part| part.tokens > 0);
        parts.sort_by(|a, b| b.tokens.cmp(&a.tokens));
        Self {
            category,
            tokens,
            items,
            parts,
        }
    }

    /// A slice whose tokens are exactly its parts' — the tools, the messages.
    fn summed(category: Category, items: usize, parts: Vec<Part>) -> Self {
        let tokens = parts.iter().map(|part| part.tokens).sum();
        Self::of(category, tokens, items, parts)
    }
}

/// The system prompt's blocks, one per markdown heading — the persona's
/// `# System prompt`, the `## Environment` and `## Scratchpad` blocks the
/// boundary appends (`docs/environment.md`) — each labelled by its heading
/// and running to the next one; a `#` line inside a fenced example is that
/// block's text. Text ahead of the first heading (an override prompt with no
/// headings at all) is a block of its own.
fn prompt_blocks(prompt: &str) -> Vec<(String, &str)> {
    let mut starts: Vec<(usize, String)> = Vec::new();
    let mut offset = 0;
    // A `#` line inside a fenced example is that block's text.
    let mut scanner = crate::markdown::BlockScanner::new();
    for line in prompt.split_inclusive('\n') {
        let text = line.trim_end_matches(['\n', '\r']);
        if scanner.classify(text) == crate::markdown::LineKind::Prose
            && let Some((_, heading)) = crate::markdown::heading_level(text)
        {
            starts.push((offset, heading.to_string()));
        }
        offset += line.len();
    }
    if starts.first().is_none_or(|(start, _)| *start > 0) {
        starts.insert(0, (0, Category::SystemPrompt.label().to_string()));
    }
    let mut blocks = Vec::new();
    for (i, (start, label)) in starts.iter().enumerate() {
        let end = starts.get(i + 1).map_or(prompt.len(), |(next, _)| *next);
        let text = prompt[*start..end].trim_end();
        if !text.trim().is_empty() {
            blocks.push((label.clone(), text));
        }
    }
    blocks
}

/// The MCP tools folded by server — the granularity `/mcp` acts on — each
/// labelled with its tool count, in the order the request lists them.
fn server_parts(tools: &[&ToolDefinition], count: &dyn Fn(&str) -> usize) -> Vec<Part> {
    let mut servers: Vec<(String, usize, usize)> = Vec::new();
    for tool in tools {
        let server = parse_wire_name(&tool.name).map_or(tool.name.as_str(), |(server, _)| server);
        let tokens = count(&tool.schema);
        match servers.iter_mut().find(|(name, _, _)| name == server) {
            Some((_, tools, sum)) => {
                *tools += 1;
                *sum += tokens;
            }
            None => servers.push((server.to_string(), 1, tokens)),
        }
    }
    servers
        .into_iter()
        .map(|(server, tools, tokens)| Part {
            label: format!(
                "{server} · {tools} {}",
                if tools == 1 { "tool" } else { "tools" }
            ),
            tokens,
        })
        .collect()
}

/// The reminder cut into its sections. The instructions section is the
/// text the session stored (`instructions`), found whole where the reminder
/// carries it: an `AGENTS.md` is the user's own text and may quote any
/// header, so no header inside it may cut it. The listings after it — the
/// skills, agent types and secrets the session renders itself, in that
/// order — are cut at their headers, each found **from the end**, where the
/// session writes it. The slices cover the whole text — the wrapper's
/// opening counts with the first section and its closing tag with the last
/// — so the sections add up to the reminder. Empty when no section is
/// recognised.
fn reminder_sections<'a>(
    reminder: &'a str,
    instructions: Option<&str>,
) -> Vec<(Category, &'a str)> {
    let mut cuts: Vec<(usize, Category)> = Vec::new();
    // Where the listings may start: past the instructions, when there are
    // any.
    let mut listings = 0;
    if let Some(docs) = instructions.map(str::trim).filter(|docs| !docs.is_empty())
        && let Some(at) = reminder.find(docs)
    {
        cuts.push((0, Category::ProjectDocs));
        listings = at + docs.len();
    }
    let mut end = reminder.len();
    for (category, header) in [
        (Category::Secrets, crate::secrets::SECRET_LISTING_HEADER),
        (Category::AgentTypes, crate::subagents::AGENT_LISTING_HEADER),
        (Category::Skills, crate::skills::SKILL_LISTING_HEADER),
    ] {
        if let Some(at) = line_start_rfind(&reminder[..end], header).filter(|&at| at >= listings) {
            cuts.push((at, category));
            end = at;
        }
    }
    cuts.sort_by_key(|(at, _)| *at);
    let mut sections = Vec::with_capacity(cuts.len());
    for (i, (at, category)) in cuts.iter().enumerate() {
        let start = if i == 0 { 0 } else { *at };
        let stop = cuts.get(i + 1).map_or(reminder.len(), |(next, _)| *next);
        sections.push((*category, &reminder[start..stop]));
    }
    sections
}

/// Where `needle` starts a line of `hay` last.
fn line_start_rfind(hay: &str, needle: &str) -> Option<usize> {
    hay.rmatch_indices(needle)
        .map(|(at, _)| at)
        .find(|&at| at == 0 || hay[..at].ends_with('\n'))
}

/// A section's items, each labelled: the `AGENTS.md` files under their
/// `Contents of {path} (…):` headings, and the listings' `- name: …` rows.
fn section_items(category: Category, section: &str) -> Vec<(String, &str)> {
    if category == Category::ProjectDocs {
        return doc_files(section);
    }
    section
        .lines()
        .filter_map(|line| {
            let row = line.strip_prefix("- ")?;
            Some((listing_label(row).to_string(), line))
        })
        .collect()
}

/// A listing row's name: a skill's or an agent type's up to its `: ` (or
/// its ` (Tools: …)` suffix), a secret's out of its `<secret:NAME>`
/// placeholder.
fn listing_label(row: &str) -> &str {
    if let Some(rest) = row.strip_prefix("<secret:") {
        return rest.split('>').next().unwrap_or(rest);
    }
    let end = [row.find(": "), row.find(" (")]
        .into_iter()
        .flatten()
        .min()
        .unwrap_or(row.len());
    row[..end].trim()
}

/// The instructions section's files: each `Contents of {path} ({note}):`
/// heading (`project_doc::doc_heading`) to the next, labelled by its path.
fn doc_files(section: &str) -> Vec<(String, &str)> {
    let mut starts: Vec<(usize, String)> = Vec::new();
    let mut offset = 0;
    for line in section.split_inclusive('\n') {
        if let Some(path) = doc_heading_path(line.trim_end()) {
            starts.push((offset, path.to_string()));
        }
        offset += line.len();
    }
    starts
        .iter()
        .enumerate()
        .map(|(i, (start, path))| {
            let end = starts.get(i + 1).map_or(section.len(), |(next, _)| *next);
            (path.clone(), section[*start..end].trim_end())
        })
        .collect()
}

/// The path a `Contents of {path} ({note}):` heading names.
fn doc_heading_path(line: &str) -> Option<&str> {
    let rest = line.strip_prefix("Contents of ")?;
    [
        crate::project_doc::DOC_CHECKED_IN_NOTE,
        crate::project_doc::DOC_OVERRIDE_NOTE,
    ]
    .iter()
    .find_map(|note| rest.strip_suffix(&format!(" ({note}):")))
}

/// The conversation's parts: what the user typed, the `!` commands they ran
/// with their output, the app's notices to the model, the compaction
/// summary, the replies, the tool calls (each one's name and arguments), the
/// tool results (an image read's attachment note with them), the image
/// attachments — and `stray` reminder text no header claims. A user-role
/// message counts whole, the gauge's rule (`app::estimate_messages_tokens`);
/// the texts the derivation noted as coming from elsewhere are counted out
/// of it, so the parts still add up to the message.
fn message_parts(sources: &Sources<'_>, count: &dyn Fn(&str) -> usize, stray: usize) -> Vec<Part> {
    let (mut typed, mut shell, mut notices, mut summary) = (0, 0, 0, 0);
    let (mut replies, mut calls, mut results, mut images, mut reminder) = (0, 0, 0, 0, stray);
    // The origins arrive in message order, so one cursor walks them.
    let mut origins = sources.origins.iter().peekable();
    for (index, message) in sources.messages.iter().enumerate() {
        while origins.next_if(|segment| segment.message < index).is_some() {}
        match message.role {
            ContextRole::User | ContextRole::System => {
                let whole = count(&message.text);
                let mut elsewhere = 0;
                while let Some(segment) = origins.next_if(|segment| segment.message == index) {
                    let bucket = match segment.origin {
                        TextOrigin::Typed => continue,
                        TextOrigin::Shell => &mut shell,
                        TextOrigin::Notice => &mut notices,
                        TextOrigin::Attachment => &mut results,
                        TextOrigin::Summary => &mut summary,
                        TextOrigin::Reminder => &mut reminder,
                    };
                    let tokens = message
                        .text
                        .get(segment.range.clone())
                        .map_or(0, count)
                        .min(whole - elsewhere);
                    *bucket += tokens;
                    elsewhere += tokens;
                }
                typed += whole - elsewhere;
            }
            ContextRole::Assistant => replies += count(&message.text),
            ContextRole::Tool => results += count(&message.text),
        }
        images += message.images.len() * sources.image_tokens;
        calls += message
            .tool_calls
            .iter()
            .map(|call| count(&call.name) + count(&call.arguments))
            .sum::<usize>();
    }
    [
        ("Your messages", typed),
        ("Shell commands", shell),
        ("App notices", notices),
        ("Summary", summary),
        ("Replies", replies),
        ("Tool calls", calls),
        ("Tool results", results),
        ("Images", images),
        ("Reminder", reminder),
    ]
    .into_iter()
    .map(|(label, tokens)| Part {
        label: label.to_string(),
        tokens,
    })
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::{ContextToolCall, TextOrigin, TextSegment};

    /// The test counter: a token is a byte, so every sum is checkable by hand.
    fn bytes(text: &str) -> usize {
        text.len()
    }

    fn tool(name: &str, schema: &str) -> ToolDefinition {
        ToolDefinition {
            name: name.to_string(),
            schema: schema.to_string(),
        }
    }

    fn of<'a>(sources: Sources<'a>) -> Breakdown {
        breakdown(&sources, &bytes)
    }

    fn tokens(breakdown: &Breakdown, category: Category) -> usize {
        breakdown.slice(category).map_or(0, |slice| slice.tokens)
    }

    fn parts(breakdown: &Breakdown, category: Category) -> Vec<(String, usize)> {
        breakdown
            .slice(category)
            .map(|slice| {
                slice
                    .parts
                    .iter()
                    .map(|part| (part.label.clone(), part.tokens))
                    .collect()
            })
            .unwrap_or_default()
    }

    // ===== tool definitions =====

    #[test]
    fn a_definition_carries_the_wire_name_and_the_schema_as_sent() {
        let spec = serde_json::json!({
            "type": "function",
            "function": {"name": "bash", "description": "Run it.", "parameters": {}}
        });
        let definition = ToolDefinition::from_spec(&spec);
        assert_eq!(definition.name, "bash");
        assert_eq!(definition.schema, spec.to_string());
        let set = ToolDefinition::from_specs(&[spec.clone(), spec]);
        assert_eq!(set.len(), 2);
    }

    // ===== the ledger =====

    #[test]
    fn the_ledger_sums_every_frame_and_counts_the_requests() {
        let mut ledger = UsageLedger::default();
        assert_eq!(ledger.cache_share(), None, "nothing reported yet");
        ledger.record(&TokenUsage {
            input: 1_000,
            output: 50,
            cached: 800,
            cache_write: 100,
            reasoning: 10,
        });
        ledger.record(&TokenUsage {
            input: 3_000,
            output: 150,
            cached: 2_200,
            cache_write: 0,
            reasoning: 0,
        });
        assert_eq!(
            ledger,
            UsageLedger {
                requests: 2,
                input: 4_000,
                output: 200,
                cached: 3_000,
                cache_write: 100,
            }
        );
        assert_eq!(ledger.cache_share(), Some(0.75));
    }

    #[test]
    fn one_requests_cache_share_is_its_reads_over_its_input() {
        let usage = TokenUsage {
            input: 400,
            output: 9,
            cached: 100,
            ..TokenUsage::default()
        };
        assert_eq!(cache_share(&usage), Some(0.25));
        assert_eq!(cache_share(&TokenUsage::default()), None);
    }

    // ===== the system prompt and the tools =====

    #[test]
    fn the_system_prompt_is_one_slice_split_at_its_headings() {
        let prompt =
            "# System prompt\n\nBe terse.\n\n## Environment\n\nDate x\n\n## Scratchpad\n\n/tmp/s";
        let breakdown = of(Sources {
            system_prompt: Some(prompt),
            ..Sources::default()
        });
        assert_eq!(tokens(&breakdown, Category::SystemPrompt), prompt.len());
        assert_eq!(
            parts(&breakdown, Category::SystemPrompt),
            [
                (
                    "System prompt".to_string(),
                    "# System prompt\n\nBe terse.".len()
                ),
                ("Environment".to_string(), "## Environment\n\nDate x".len()),
                ("Scratchpad".to_string(), "## Scratchpad\n\n/tmp/s".len()),
            ]
        );
        assert_eq!(breakdown.slice(Category::SystemPrompt).unwrap().items, 3);
    }

    #[test]
    fn a_hash_inside_a_code_fence_is_not_a_heading() {
        // A shell comment in a fenced example is the block's own text, not a
        // block of its own — and neither is a `#` with no space after it.
        let prompt = "# System prompt\n\n#1 rule. Run:\n\n```bash\n# install first\nmake\n```\n\n## Environment\n\nDate x";
        let breakdown = of(Sources {
            system_prompt: Some(prompt),
            ..Sources::default()
        });
        assert_eq!(
            parts(&breakdown, Category::SystemPrompt),
            [
                (
                    "System prompt".to_string(),
                    "# System prompt\n\n#1 rule. Run:\n\n```bash\n# install first\nmake\n```".len()
                ),
                ("Environment".to_string(), "## Environment\n\nDate x".len()),
            ]
        );
    }

    #[test]
    fn a_prompt_with_no_heading_is_one_block() {
        let breakdown = of(Sources {
            system_prompt: Some("You are a helpful bot."),
            ..Sources::default()
        });
        assert_eq!(
            parts(&breakdown, Category::SystemPrompt),
            [("System prompt".to_string(), 22)]
        );
    }

    #[test]
    fn built_in_tools_and_mcp_tools_are_two_slices() {
        let tools = [
            tool("bash", "aaaaaaaaaa"),
            tool("read", "bbbbb"),
            tool("mcp__deepwiki__ask_question", "cccccccc"),
            tool("mcp__deepwiki__read_wiki", "dddd"),
            tool("mcp__github__create_issue", "eeeeeeeeeeee"),
        ];
        let breakdown = of(Sources {
            tools: &tools,
            ..Sources::default()
        });
        assert_eq!(tokens(&breakdown, Category::SystemTools), 15);
        assert_eq!(
            parts(&breakdown, Category::SystemTools),
            [("bash".to_string(), 10), ("read".to_string(), 5)]
        );
        assert_eq!(breakdown.slice(Category::SystemTools).unwrap().items, 2);
        // MCP tools fold by server — the granularity `/mcp` can act on — and
        // a part says how many tools it holds; the slice counts the tools.
        // The two servers tie, so they keep the order the request lists them.
        assert_eq!(tokens(&breakdown, Category::McpTools), 24);
        assert_eq!(
            parts(&breakdown, Category::McpTools),
            [
                ("deepwiki · 2 tools".to_string(), 12),
                ("github · 1 tool".to_string(), 12),
            ]
        );
        assert_eq!(breakdown.slice(Category::McpTools).unwrap().items, 3);
    }

    #[test]
    fn parts_are_largest_first_and_ties_keep_their_order() {
        let tools = [tool("a", "x"), tool("b", "xxx"), tool("c", "xxx")];
        let breakdown = of(Sources {
            tools: &tools,
            ..Sources::default()
        });
        assert_eq!(
            parts(&breakdown, Category::SystemTools),
            [
                ("b".to_string(), 3),
                ("c".to_string(), 3),
                ("a".to_string(), 1)
            ]
        );
    }

    // ===== the reminder =====

    /// The reminder exactly as the session composes it.
    fn reminder(sections: &[&str]) -> String {
        crate::reminder::reminder_message(sections)
    }

    fn docs_section() -> String {
        format!(
            "{}\n\nContents of /repo/AGENTS.md (project instructions, checked into the codebase):\n\nUse TDD.\n\nContents of /repo/app/AGENTS.md (project instructions, checked into the codebase):\n\nKeep it pure.",
            crate::project_doc::INSTRUCTIONS_PREAMBLE
        )
    }

    fn skills_section() -> String {
        crate::skills::skill_section("- dataviz: Charts and plots\n- haiku-writer: Poems")
    }

    fn agents_section() -> String {
        crate::subagents::agent_section(
            "- general-purpose: Anything (Tools: All)\n- explore (Tools: Read, Grep)",
        )
    }

    fn secrets_section() -> String {
        crate::secrets::secret_section("- <secret:GITHUB_TOKEN>: the CI bot\n- <secret:NPM>")
    }

    #[test]
    fn the_reminder_splits_into_its_sections_and_adds_up() {
        let (docs, skills, agents, secrets) = (
            docs_section(),
            skills_section(),
            agents_section(),
            secrets_section(),
        );
        let text = reminder(&[&docs, &skills, &agents, &secrets]);
        let breakdown = of(Sources {
            reminder: Some(&text),
            instructions: Some(&docs),
            ..Sources::default()
        });
        // Every byte of the reminder is attributed to exactly one section.
        let sum: usize = [
            Category::ProjectDocs,
            Category::Skills,
            Category::AgentTypes,
            Category::Secrets,
        ]
        .iter()
        .map(|&category| tokens(&breakdown, category))
        .sum();
        assert_eq!(sum, text.len());
        // The wrapper's opening — the tag and the preamble — counts with the
        // first section, and each later section runs from its own header to
        // the next one's (the closing tag riding the last).
        let opening = text
            .find(&docs)
            .expect("the docs section is in the reminder");
        assert_eq!(
            tokens(&breakdown, Category::ProjectDocs),
            opening + docs.len() + 2
        );
        assert_eq!(tokens(&breakdown, Category::Skills), skills.len() + 2);
        assert_eq!(tokens(&breakdown, Category::AgentTypes), agents.len() + 2);
        assert_eq!(
            tokens(&breakdown, Category::Secrets),
            secrets.len() + "\n</system-reminder>".len()
        );
    }

    #[test]
    fn each_section_names_its_items() {
        let (docs, skills, agents, secrets) = (
            docs_section(),
            skills_section(),
            agents_section(),
            secrets_section(),
        );
        let text = reminder(&[&docs, &skills, &agents, &secrets]);
        let breakdown = of(Sources {
            reminder: Some(&text),
            instructions: Some(&docs),
            ..Sources::default()
        });
        let labels = |category| -> Vec<String> {
            let mut labels: Vec<String> = parts(&breakdown, category)
                .into_iter()
                .map(|(label, _)| label)
                .collect();
            labels.sort();
            labels
        };
        assert_eq!(
            labels(Category::ProjectDocs),
            ["/repo/AGENTS.md", "/repo/app/AGENTS.md"]
        );
        assert_eq!(labels(Category::Skills), ["dataviz", "haiku-writer"]);
        assert_eq!(labels(Category::AgentTypes), ["explore", "general-purpose"]);
        assert_eq!(labels(Category::Secrets), ["GITHUB_TOKEN", "NPM"]);
        // A listed item costs its own row.
        assert!(
            parts(&breakdown, Category::Skills)
                .contains(&("dataviz".to_string(), "- dataviz: Charts and plots".len()))
        );
        assert_eq!(breakdown.slice(Category::ProjectDocs).unwrap().items, 2);
        assert_eq!(breakdown.slice(Category::Secrets).unwrap().items, 2);
    }

    #[test]
    fn a_subagents_briefing_is_split_the_same_way() {
        // A launched agent opens on the skills section wrapped alone.
        let text = crate::skills::listing_message("- dataviz: Charts and plots");
        let breakdown = of(Sources {
            reminder: Some(&text),
            ..Sources::default()
        });
        assert_eq!(tokens(&breakdown, Category::Skills), text.len());
        assert_eq!(tokens(&breakdown, Category::ProjectDocs), 0);
        assert_eq!(
            parts(&breakdown, Category::Skills),
            [("dataviz".to_string(), "- dataviz: Charts and plots".len())]
        );
    }

    #[test]
    fn instructions_quoting_a_listing_header_stay_instructions() {
        // An AGENTS.md is the user's own text and may quote anything — even
        // the skills header. The listings are looked for only after the
        // instructions the session stored, so the quote stays in the docs.
        let docs = format!(
            "{}\n\nContents of /repo/AGENTS.md (project instructions, checked into the codebase):\n\nThe reminder reads:\n{}\n- not-a-skill: quoted",
            crate::project_doc::INSTRUCTIONS_PREAMBLE,
            crate::skills::SKILL_LISTING_HEADER
        );
        let skills = skills_section();
        let text = reminder(&[&docs, &skills]);
        let breakdown = of(Sources {
            reminder: Some(&text),
            instructions: Some(&docs),
            ..Sources::default()
        });
        assert_eq!(
            tokens(&breakdown, Category::Skills),
            skills.len() + "\n</system-reminder>".len()
        );
        let mut labels: Vec<String> = parts(&breakdown, Category::Skills)
            .into_iter()
            .map(|(label, _)| label)
            .collect();
        labels.sort();
        assert_eq!(labels, ["dataviz", "haiku-writer"]);
    }

    #[test]
    fn instructions_quoting_a_header_the_reminder_lacks_stay_instructions() {
        // The instructions are counted as the session stored them, never
        // found by a header: an AGENTS.md quoting the skills and secrets
        // headers, in a session listing neither, stays the user's text whole.
        let docs = format!(
            "{}\n\nContents of /repo/AGENTS.md (project instructions, checked into the codebase):\n\nThe reminder reads:\n{}\n- not-a-skill: quoted\n{}\n- <secret:NOT_ONE>",
            crate::project_doc::INSTRUCTIONS_PREAMBLE,
            crate::skills::SKILL_LISTING_HEADER,
            crate::secrets::SECRET_LISTING_HEADER,
        );
        let agents = agents_section();
        let text = reminder(&[&docs, &agents]);
        let breakdown = of(Sources {
            reminder: Some(&text),
            instructions: Some(&docs),
            ..Sources::default()
        });
        let opening = text
            .find(&docs)
            .expect("the docs section is in the reminder");
        assert_eq!(
            tokens(&breakdown, Category::ProjectDocs),
            opening + docs.len() + 2
        );
        assert_eq!(tokens(&breakdown, Category::Skills), 0);
        assert_eq!(tokens(&breakdown, Category::Secrets), 0);
        assert_eq!(
            tokens(&breakdown, Category::AgentTypes),
            agents.len() + "\n</system-reminder>".len()
        );
    }

    #[test]
    fn a_reminder_with_no_known_section_still_counts_as_conversation() {
        // Nothing the session writes looks like this, but a token must never
        // fall out of the total: an unrecognised reminder rides as a user
        // message, so it is counted with the messages.
        let text = "<system-reminder>\nsomething else\n</system-reminder>";
        let breakdown = of(Sources {
            reminder: Some(text),
            ..Sources::default()
        });
        assert_eq!(tokens(&breakdown, Category::Messages), text.len());
        assert_eq!(
            parts(&breakdown, Category::Messages),
            [("Reminder".to_string(), text.len())]
        );
    }

    // ===== the messages =====

    fn message(role: ContextRole, text: &str) -> ContextMessage {
        ContextMessage::new(role, text)
    }

    #[test]
    fn messages_split_by_what_they_are() {
        let mut user = message(ContextRole::User, "look at this");
        user.images = vec!["/a.png".into(), "/b.png".into()];
        let calls = ContextMessage::assistant_tool_calls(
            "On it.",
            vec![ContextToolCall {
                id: "call_0".into(),
                name: "read".into(),
                arguments: "{\"path\":\"x\"}".into(),
            }],
        );
        let result = ContextMessage::tool_result("call_0", "file contents here");
        let reply = message(ContextRole::Assistant, "Done.");
        let messages = [user, calls, result, reply];
        let breakdown = of(Sources {
            messages: &messages,
            image_tokens: 100,
            ..Sources::default()
        });
        assert_eq!(
            parts(&breakdown, Category::Messages),
            [
                ("Images".to_string(), 200),
                ("Tool results".to_string(), "file contents here".len()),
                (
                    "Tool calls".to_string(),
                    "read".len() + "{\"path\":\"x\"}".len()
                ),
                ("Your messages".to_string(), "look at this".len()),
                ("Replies".to_string(), "On it.".len() + "Done.".len()),
            ]
        );
        assert_eq!(
            tokens(&breakdown, Category::Messages),
            200 + 18 + 4 + 12 + 12 + 6 + 5
        );
        assert_eq!(breakdown.slice(Category::Messages).unwrap().items, 4);
    }

    fn segment(range: std::ops::Range<usize>, origin: TextOrigin) -> TextSegment {
        TextSegment {
            message: 0,
            range,
            origin,
        }
    }

    #[test]
    fn the_users_share_splits_by_where_each_text_came_from() {
        // A `!` command and the app's notices ride as user-role text, merged
        // with what was typed around them: the origins the derivation noted
        // say which is which, so "Your messages" is only what was typed.
        let text = "hello\n\n$ ls\nfile.txt\n\n[system] Copied\n\nand now";
        let messages = [message(ContextRole::User, text)];
        let origins = [
            segment(0..5, TextOrigin::Typed),
            segment(7..20, TextOrigin::Shell),
            segment(22..37, TextOrigin::Notice),
            segment(39..46, TextOrigin::Typed),
        ];
        let breakdown = of(Sources {
            messages: &messages,
            origins: &origins,
            ..Sources::default()
        });
        assert_eq!(
            parts(&breakdown, Category::Messages),
            [
                ("Your messages".to_string(), text.len() - 13 - 15),
                ("App notices".to_string(), 15),
                ("Shell commands".to_string(), 13),
            ]
        );
        assert_eq!(
            tokens(&breakdown, Category::Messages),
            text.len(),
            "the parts still add up to the message"
        );
    }

    #[test]
    fn the_compaction_summary_and_an_image_reads_note_are_their_own_parts() {
        let bridge = "PREFIX\nwe did the thing";
        let merged = format!("do the thing\n\n{bridge}");
        let note = "[image read: /a.png]";
        let mut attached = message(ContextRole::User, note);
        attached.images = vec!["/a.png".into()];
        let messages = [
            message(ContextRole::User, &merged),
            message(ContextRole::Assistant, "ok"),
            attached,
        ];
        let origins = [
            segment(0..12, TextOrigin::Typed),
            segment(14..merged.len(), TextOrigin::Summary),
            TextSegment {
                message: 2,
                range: 0..note.len(),
                origin: TextOrigin::Attachment,
            },
        ];
        let breakdown = of(Sources {
            messages: &messages,
            origins: &origins,
            image_tokens: 100,
            ..Sources::default()
        });
        assert_eq!(
            parts(&breakdown, Category::Messages),
            [
                ("Images".to_string(), 100),
                ("Summary".to_string(), bridge.len()),
                ("Tool results".to_string(), note.len()),
                ("Your messages".to_string(), merged.len() - bridge.len()),
                ("Replies".to_string(), 2),
            ]
        );
    }

    #[test]
    fn user_text_with_no_noted_origin_is_the_users() {
        let messages = [message(ContextRole::User, "typed")];
        let breakdown = of(Sources {
            messages: &messages,
            ..Sources::default()
        });
        assert_eq!(
            parts(&breakdown, Category::Messages),
            [("Your messages".to_string(), 5)]
        );
    }

    #[test]
    fn an_empty_conversation_still_lists_its_messages() {
        let breakdown = of(Sources::default());
        assert_eq!(breakdown.slices.len(), 1, "only Messages: {breakdown:?}");
        let messages = breakdown.slice(Category::Messages).expect("always listed");
        assert_eq!(messages.tokens, 0);
        assert!(messages.parts.is_empty());
        assert_eq!(breakdown.total(), 0);
    }

    // ===== the memo =====

    #[test]
    fn a_memo_counts_each_text_once_across_passes() {
        let calls = std::cell::Cell::new(0);
        let counter = |text: &str| {
            calls.set(calls.get() + 1);
            text.len()
        };
        let mut memo = CountMemo::default();
        assert_eq!(memo.count("hello", &counter), 5);
        assert_eq!(memo.count("hello", &counter), 5, "the same pass, again");
        memo.finish_pass();
        assert_eq!(memo.count("hello", &counter), 5, "the next pass");
        assert_eq!(memo.count("world!", &counter), 6);
        assert_eq!(calls.get(), 2, "only a text never seen was counted");
    }

    #[test]
    fn a_memo_forgets_what_the_last_pass_did_not_count() {
        let calls = std::cell::Cell::new(0);
        let counter = |text: &str| {
            calls.set(calls.get() + 1);
            text.len()
        };
        let mut memo = CountMemo::default();
        memo.count("left the window", &counter);
        memo.finish_pass();
        memo.count("stayed", &counter);
        memo.finish_pass();
        memo.count("stayed", &counter);
        assert_eq!(calls.get(), 2, "a text every pass counts is counted once");
        memo.count("left the window", &counter);
        assert_eq!(calls.get(), 3, "one a pass skipped is counted afresh");
    }

    #[test]
    fn a_memoized_count_is_the_counters_own() {
        let tools = [tool("bash", "xxxx"), tool("mcp__deepwiki__ask", "yy")];
        let skills = skills_section();
        let text = reminder(&[&skills]);
        let messages = [
            message(ContextRole::User, "hi"),
            message(ContextRole::Assistant, "hello"),
            message(ContextRole::User, "hi"),
        ];
        let sources = Sources {
            system_prompt: Some("# A\n\nBe terse.\n\n# B\n\nBe kind."),
            tools: &tools,
            reminder: Some(&text),
            messages: &messages,
            ..Sources::default()
        };
        let memo = std::cell::RefCell::new(CountMemo::default());
        let memoized = |text: &str| memo.borrow_mut().count(text, &bytes);
        assert_eq!(breakdown(&sources, &memoized), of(sources));
        memo.borrow_mut().finish_pass();
        assert_eq!(
            breakdown(&sources, &memoized),
            of(sources),
            "and from the memo"
        );
    }

    // ===== the whole =====

    #[test]
    fn slices_run_in_request_order_and_empty_ones_are_left_out() {
        let tools = [tool("bash", "xxxx")];
        let skills = skills_section();
        let text = reminder(&[&skills]);
        let messages = [message(ContextRole::User, "hi")];
        let breakdown = of(Sources {
            system_prompt: Some("Be terse."),
            tools: &tools,
            reminder: Some(&text),
            messages: &messages,
            ..Sources::default()
        });
        let order: Vec<Category> = breakdown.slices.iter().map(|s| s.category).collect();
        assert_eq!(
            order,
            [
                Category::SystemPrompt,
                Category::SystemTools,
                Category::Skills,
                Category::Messages
            ]
        );
        assert_eq!(breakdown.total(), 9 + 4 + text.len() + 2);
        assert_eq!(breakdown.largest(), Some(Category::Skills));
    }

    #[test]
    fn the_categories_read_as_the_page_names_them() {
        let labels: Vec<&str> = Category::ALL.iter().map(|c| c.label()).collect();
        assert_eq!(
            labels,
            [
                "System prompt",
                "System tools",
                "MCP tools",
                "Project docs",
                "Skills",
                "Agent types",
                "Secrets",
                "Messages",
            ]
        );
        assert_eq!(Category::McpTools.unit(), ("tool", "tools"));
    }
}
