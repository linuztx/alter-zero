//! Google's **Cloud Code Assist** API as a wire format — the backend Google
//! Antigravity talks to, reached here through the Antigravity sign-in
//! (`docs/providers/antigravity.md`).
//!
//! Like [`super::anthropic`] and [`super::responses`], everything here is a
//! **translation**, in both directions, between that format and the Chat
//! Completions currency the rest of the crate uses ([`ChatMessage`] in,
//! [`Delta`] / [`StreamOutcome`](super::openai::StreamOutcome) out), so
//! nothing above [`super::openai::OpenAiClient`] learns that a fifth wire
//! format exists.
//!
//! The request is Gemini's `generateContent` body inside an envelope naming
//! the account's project, and the backend routes some of its models to other
//! labs — which is where most of what is easy to get wrong here lives:
//!
//! - **There is no `system` or `tool` role.** The system prompt is a
//!   top-level `systemInstruction`, and a tool result is a `functionResponse`
//!   *part* of a **user** content naming its tool; consecutive same-role
//!   contents merge, so a parallel batch's results are one user turn.
//! - **The model family decides the tool schema field.** Gemini takes JSON
//!   Schema as `parametersJsonSchema`; Claude and GPT-OSS go through a bridge
//!   that takes the legacy `parameters` and refuses every keyword outside a
//!   short list ([`bridge_schema`]).
//! - **Only Claude and GPT-OSS pair calls and results by id.** Gemini pairs
//!   them by order and name, and is sent none.
//! - **Thought signatures must come back.** The first call of each model
//!   turn carries the signature the stream gave it — or Google's documented
//!   [`SKIP_THOUGHT_SIGNATURE`] — and a Claude tool loop must open on the
//!   thinking that started it. The crate keeps no reasoning in its context,
//!   so both live in a process-wide memo beside it ([`remember_round`],
//!   [`memos_for`]).
//! - **Thinking is per family**, and on Gemini the *level* is the runtime
//!   id's suffix ([`runtime_model`]), so only `includeThoughts` is sent.
//!
//! Pure and unit-tested but for the memo — the HTTP lives in
//! [`super::openai`], the sign-in and the account's project in
//! [`super::antigravity`].

use std::borrow::Cow;
use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, OnceLock};

use serde_json::{Map, Value, json};

use super::config::ModelConfig;
use super::openai::Delta;
use super::reasoning::{ReasoningEffort, ThinkingMode};
use super::tools::ToolCallRequest;
use super::{ChatMessage, ContentPart, MessageContent};
use crate::stream::{TokenUsage, ToolCallFragment};

/// The thought signature Google documents for a function call whose real
/// one is not at hand — a round another provider produced, or one from
/// before a `/resume`. The backend validates the first call of each model
/// turn and accepts this value in place of a signature.
pub const SKIP_THOUGHT_SIGNATURE: &str = "skip_thought_signature_validator";

/// The thinking budget a Claude `-thinking` route spends — the Messages
/// wire's own ([`super::anthropic::THINKING_BUDGET`]), so Claude thinks the
/// same however it is reached. Must stay below `CLAUDE_MAX_OUTPUT`.
pub const CLAUDE_THINKING_BUDGET: u64 = super::anthropic::THINKING_BUDGET;

/// The thinking budget a GPT-OSS route spends.
const GPT_OSS_THINKING_BUDGET: u64 = 8_192;

/// `maxOutputTokens` per family. A request asking for more than a model's
/// cap is refused outright, so each is the **smallest** cap among the
/// family's current models: 65535 for Gemini (the Pro models' cap, one under
/// Flash's), 64000 for Claude — which must also exceed the thinking budget —
/// and 32768 for GPT-OSS.
const GEMINI_MAX_OUTPUT: u64 = 65_535;
const CLAUDE_MAX_OUTPUT: u64 = 64_000;
const GPT_OSS_MAX_OUTPUT: u64 = 32_768;

/// The user turn appended to a conversation that would otherwise end on a
/// model turn, which the backend refuses.
const CONTINUE_TEXT: &str = "Continue.";

/// How deep a tool schema is followed. A schema nested past this — or a
/// `$ref` cycle — is cut to `{}` (any value) rather than failing the tool.
const SCHEMA_MAX_DEPTH: usize = 64;

/// The schema keywords that are never sent: they describe the document, not
/// the value, and the backend wants self-contained schemas.
const META_KEYWORDS: [&str; 8] = [
    "$schema",
    "$id",
    "$anchor",
    "$dynamicAnchor",
    "$vocabulary",
    "$comment",
    "$defs",
    "definitions",
];

/// Which lab's model a request goes to — what decides the schema field, the
/// call ids, the thinking parameters and the output cap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Family {
    Gemini,
    Claude,
    GptOss,
}

impl Family {
    fn of(model: &str) -> Self {
        let model = model.to_ascii_lowercase();
        if model.starts_with("claude-") {
            Self::Claude
        } else if model.starts_with("gpt-oss-") {
            Self::GptOss
        } else {
            Self::Gemini
        }
    }

    /// Whether calls and results carry ids — Gemini pairs them by order and
    /// name instead.
    fn pairs_by_id(self) -> bool {
        self != Self::Gemini
    }
}

/// The ladder level a model id ends with, and the id without it —
/// `gemini-3.8-flash-high` is `gemini-3.8-flash` at `high`. Only an exact
/// rung of [`ReasoningEffort::LADDER`] counts, so the id is always the base
/// and the level put back together.
#[must_use]
pub fn level_suffix(model: &str) -> Option<(&str, ReasoningEffort)> {
    let (base, last) = model.rsplit_once('-')?;
    let effort = ReasoningEffort::parse(last)?;
    (!base.is_empty() && effort.as_str() == last).then_some((base, effort))
}

/// The id a request names for `model` at `thinking`: a grouped model's
/// level is its runtime id's suffix (`docs/providers/antigravity.md` *The catalog*),
/// so an effort is appended — unless the id already names one, which a
/// listed id sent verbatim may.
#[must_use]
pub fn runtime_model(model: &str, thinking: Option<ThinkingMode>) -> String {
    match thinking {
        Some(ThinkingMode::Effort(effort)) if level_suffix(model).is_none() => {
            format!("{model}-{}", effort.as_str())
        }
        _ => model.to_string(),
    }
}

// ---------------------------------------------------------------------------
// The memo: what the next request must hand back
// ---------------------------------------------------------------------------

/// What the backend asked to be given back about one tool call.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CallMemo {
    /// The thought signature that came with the call.
    pub signature: Option<String>,
    /// The round's reasoning, kept on the round's **first** call — Claude's
    /// thinking block, which must open the tool loop it started.
    pub thought: Option<Thought>,
}

/// One round's reasoning as the stream gave it: the text, exactly, and the
/// signature that vouches for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Thought {
    pub text: String,
    pub signature: String,
}

/// The most the memo holds, in bytes of signatures and thought text. A
/// signature is a few hundred bytes and a thought a few kilobytes, so this is
/// hundreds of rounds — far more than a tool loop needs, which is the only
/// span a memo is ever read for. The oldest entries go first.
const MEMO_MAX_BYTES: usize = 4 * 1024 * 1024;

#[derive(Default)]
struct MemoStore {
    entries: HashMap<String, CallMemo>,
    order: VecDeque<String>,
    bytes: usize,
}

impl MemoStore {
    fn size(memo: &CallMemo) -> usize {
        memo.signature.as_ref().map_or(0, String::len)
            + memo
                .thought
                .as_ref()
                .map_or(0, |t| t.text.len() + t.signature.len())
    }

    fn insert(&mut self, id: String, memo: CallMemo) {
        let size = Self::size(&memo) + id.len();
        if let Some(old) = self.entries.remove(&id) {
            self.bytes = self.bytes.saturating_sub(Self::size(&old) + id.len());
            self.order.retain(|k| *k != id);
        }
        self.bytes += size;
        self.order.push_back(id.clone());
        self.entries.insert(id, memo);
        while self.bytes > MEMO_MAX_BYTES {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            if let Some(gone) = self.entries.remove(&oldest) {
                self.bytes = self.bytes.saturating_sub(Self::size(&gone) + oldest.len());
            }
        }
    }
}

fn memo_store() -> &'static Mutex<MemoStore> {
    static STORE: OnceLock<Mutex<MemoStore>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(MemoStore::default()))
}

/// Keep what a finished round must hand back: each signed call's signature,
/// and — for Claude, whose thinking must open its tool loop — the round's
/// thought on its first call. A round with no calls keeps nothing: only a
/// tool loop is ever replayed with its reasoning. Boundary state, written by
/// the drain (`openai`).
pub fn remember_round(model: &str, end: &RoundEnd) {
    let Ok(mut store) = memo_store().lock() else {
        return;
    };
    let mut memos: HashMap<&str, CallMemo> = HashMap::new();
    for (id, signature) in &end.signatures {
        memos.entry(id.as_str()).or_default().signature = Some(signature.clone());
    }
    if Family::of(model) == Family::Claude
        && let (Some(first), Some(thought)) = (end.calls.first(), &end.thought)
    {
        memos.entry(first.id.as_str()).or_default().thought = Some(thought.clone());
    }
    for (id, memo) in memos {
        store.insert(id.to_string(), memo);
    }
}

/// The memos the calls in `messages` have — what [`RequestSource`] is built
/// with, so the request itself stays a pure function of its inputs.
#[must_use]
pub fn memos_for(messages: &[ChatMessage]) -> HashMap<String, CallMemo> {
    let Ok(store) = memo_store().lock() else {
        return HashMap::new();
    };
    messages
        .iter()
        .flat_map(|m| &m.tool_calls)
        .filter_map(|call| {
            store
                .entries
                .get(&call.id)
                .map(|memo| (call.id.clone(), memo.clone()))
        })
        .collect()
}

// ---------------------------------------------------------------------------
// The request
// ---------------------------------------------------------------------------

/// Build the request body for `messages` as a JSON tree — the tests' view of
/// [`RequestSource`], which writes the same request from the messages **by
/// reference**.
#[must_use]
pub fn build_payload(
    cfg: &ModelConfig,
    tools: &[Value],
    messages: &[ChatMessage],
    project: Option<&str>,
    memos: &HashMap<String, CallMemo>,
) -> Value {
    serde_json::to_value(request(cfg, tools, messages, project, memos)).unwrap_or(Value::Null)
}

/// The request as owned data the serializer thread writes from
/// ([`super::body::BodySource`]): the config, the tool specs, the round's
/// messages (their pictures shared), the account's project and the memos the
/// round's calls have.
pub struct RequestSource {
    cfg: ModelConfig,
    tools: Vec<Value>,
    messages: Vec<ChatMessage>,
    project: Option<String>,
    memos: HashMap<String, CallMemo>,
}

impl RequestSource {
    #[must_use]
    pub fn new(
        cfg: ModelConfig,
        tools: Vec<Value>,
        messages: Vec<ChatMessage>,
        project: Option<String>,
        memos: HashMap<String, CallMemo>,
    ) -> Self {
        Self {
            cfg,
            tools,
            messages,
            project,
            memos,
        }
    }
}

impl super::body::BodySource for RequestSource {
    fn write_to(&self, out: &mut dyn std::io::Write) -> serde_json::Result<()> {
        serde_json::to_writer(
            out,
            &request(
                &self.cfg,
                &self.tools,
                &self.messages,
                self.project.as_deref(),
                &self.memos,
            ),
        )
    }
}

/// The envelope as it is serialized.
#[derive(serde::Serialize)]
struct Envelope<'a> {
    project: &'a str,
    model: String,
    request: Inner<'a>,
    #[serde(rename = "requestType")]
    request_type: &'static str,
    #[serde(rename = "userAgent")]
    user_agent: &'static str,
    #[serde(rename = "requestId")]
    request_id: String,
}

/// The `generateContent` body inside it: the contents borrowed from the
/// conversation, beside every other field in one small map.
#[derive(serde::Serialize)]
struct Inner<'a> {
    contents: Vec<Content<'a>>,
    #[serde(rename = "systemInstruction", skip_serializing_if = "Option::is_none")]
    system_instruction: Option<Content<'a>>,
    #[serde(flatten)]
    fields: Map<String, Value>,
}

/// One `contents` entry (or the system instruction): a role over its parts.
#[derive(Debug, serde::Serialize)]
struct Content<'a> {
    role: &'static str,
    parts: Vec<Part<'a>>,
}

/// One part — exactly one of its payloads is set.
#[derive(Debug, Default, serde::Serialize)]
struct Part<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<Cow<'a, str>>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    thought: bool,
    #[serde(rename = "inlineData", skip_serializing_if = "Option::is_none")]
    inline_data: Option<InlineData<'a>>,
    #[serde(rename = "functionCall", skip_serializing_if = "Option::is_none")]
    function_call: Option<FunctionCallPart<'a>>,
    #[serde(rename = "functionResponse", skip_serializing_if = "Option::is_none")]
    function_response: Option<FunctionResponsePart<'a>>,
    #[serde(rename = "thoughtSignature", skip_serializing_if = "Option::is_none")]
    thought_signature: Option<Cow<'a, str>>,
    /// The call's id, for the memo lookup — whether or not it rides the wire.
    #[serde(skip)]
    call_id: Option<&'a str>,
}

impl<'a> Part<'a> {
    fn text(text: Cow<'a, str>) -> Self {
        Self {
            text: Some(text),
            ..Self::default()
        }
    }
}

/// An image, its `data:` URL split into the media type and the payload —
/// both slices of the session's one shared encoding.
#[derive(Debug, serde::Serialize)]
struct InlineData<'a> {
    #[serde(rename = "mimeType")]
    mime_type: &'a str,
    data: &'a str,
}

#[derive(Debug, serde::Serialize)]
struct FunctionCallPart<'a> {
    name: &'a str,
    args: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<&'a str>,
}

#[derive(Debug, serde::Serialize)]
struct FunctionResponsePart<'a> {
    name: &'a str,
    response: Output<'a>,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<&'a str>,
}

/// A tool result's text, under the key every reference sends it as.
#[derive(Debug, serde::Serialize)]
struct Output<'a> {
    output: Cow<'a, str>,
}

fn request<'a>(
    cfg: &'a ModelConfig,
    tools: &[Value],
    messages: &'a [ChatMessage],
    project: Option<&'a str>,
    memos: &'a HashMap<String, CallMemo>,
) -> Envelope<'a> {
    let family = Family::of(&cfg.model);
    let (system, mut contents) = contents_of(messages, family);
    let mut thinking = matches!(
        cfg.thinking,
        Some(ThinkingMode::On | ThinkingMode::Effort(_))
    );
    if family == Family::Claude {
        // Claude thinks only on its `-thinking` routes — any other takes no
        // budget at all, whatever mode a stale selection carries.
        thinking &= cfg.model.to_ascii_lowercase().contains("thinking");
        if thinking {
            thinking = open_loop_on_its_thinking(&mut contents, memos);
        }
    }
    // After the thoughts are in: a turn a replayed thought vouches for is
    // never also given the sentinel.
    sign_first_calls(&mut contents, memos);
    keep_turns_in_order(&mut contents);

    let mut fields = Map::new();
    let declarations = declarations(tools, family);
    if !declarations.is_empty() {
        fields.insert(
            "tools".to_string(),
            json!([{"functionDeclarations": declarations}]),
        );
    }
    fields.insert(
        "generationConfig".to_string(),
        Value::Object(generation_config(cfg, family, thinking)),
    );
    if let Some(key) = cfg.cache_key.as_deref().filter(|k| !k.is_empty()) {
        fields.insert("sessionId".to_string(), json!(key));
    }
    for (k, v) in &cfg.extra_body {
        fields.insert(k.clone(), v.clone());
    }
    // Last, so a field the server refuses leaves whatever put it there
    // (`docs/providers/user-providers.md`).
    cfg.drop_refused(&mut fields);

    Envelope {
        project: project.unwrap_or_default(),
        model: runtime_model(&cfg.model, cfg.thinking),
        request: Inner {
            contents,
            system_instruction: (!system.is_empty()).then_some(Content {
                role: "user",
                parts: system,
            }),
            fields,
        },
        request_type: "agent",
        user_agent: "antigravity",
        request_id: format!("agent-{}", uuid_v4()),
    }
}

/// `generationConfig` for one request: the family's output cap, its thinking
/// parameters when the model thinks, and the temperature — never for Claude,
/// whose current models refuse the sampling parameters.
fn generation_config(cfg: &ModelConfig, family: Family, thinking: bool) -> Map<String, Value> {
    let mut config = Map::new();
    let (cap, budget) = match family {
        Family::Gemini => (GEMINI_MAX_OUTPUT, None),
        Family::Claude => (CLAUDE_MAX_OUTPUT, Some(CLAUDE_THINKING_BUDGET)),
        Family::GptOss => (GPT_OSS_MAX_OUTPUT, Some(GPT_OSS_THINKING_BUDGET)),
    };
    config.insert("maxOutputTokens".to_string(), json!(cap));
    if thinking {
        let mut thinking_config = Map::new();
        thinking_config.insert("includeThoughts".to_string(), json!(true));
        if let Some(budget) = budget {
            thinking_config.insert("thinkingBudget".to_string(), json!(budget));
        }
        config.insert("thinkingConfig".to_string(), Value::Object(thinking_config));
    }
    if family != Family::Claude
        && let Some(t) = cfg.temperature
    {
        config.insert("temperature".to_string(), json!(t));
    }
    config
}

/// Split `messages` into the system instruction's parts and the `contents`.
///
/// System messages are hoisted; everything else folds into `user`/`model`
/// contents, consecutive same-role ones merged — a round's several results
/// are one user turn, and an image a `read` attached after them rides in it.
fn contents_of(messages: &[ChatMessage], family: Family) -> (Vec<Part<'_>>, Vec<Content<'_>>) {
    // A result names its tool, which only the call it answers says.
    let names: HashMap<&str, &str> = messages
        .iter()
        .flat_map(|m| &m.tool_calls)
        .map(|call| (call.id.as_str(), call.function.name.as_str()))
        .collect();
    let mut system = Vec::new();
    let mut out: Vec<Content<'_>> = Vec::new();
    for message in messages {
        match message.role.as_str() {
            "system" | "developer" => {
                if let Some(text) = non_empty(flatten_text(&message.content)) {
                    system.push(Part::text(text));
                }
            }
            "tool" => {
                let id = message.tool_call_id.as_deref().unwrap_or_default();
                let part = Part {
                    function_response: Some(FunctionResponsePart {
                        name: names.get(id).copied().unwrap_or("unknown"),
                        response: Output {
                            output: flatten_text(&message.content),
                        },
                        id: (family.pairs_by_id() && !id.is_empty()).then_some(id),
                    }),
                    ..Part::default()
                };
                push_part("user", part, &mut out);
            }
            "assistant" => {
                if let Some(text) = non_empty(flatten_text(&message.content)) {
                    push_part("model", Part::text(text), &mut out);
                }
                for call in &message.tool_calls {
                    // The arguments are a JSON *string* in the crate's
                    // currency and an object here; one the model cut short
                    // degrades to `{}` rather than failing the request.
                    let args = serde_json::from_str::<Value>(&call.function.arguments)
                        .ok()
                        .filter(Value::is_object)
                        .unwrap_or_else(|| json!({}));
                    let part = Part {
                        function_call: Some(FunctionCallPart {
                            name: &call.function.name,
                            args,
                            id: family.pairs_by_id().then_some(call.id.as_str()),
                        }),
                        call_id: Some(&call.id),
                        ..Part::default()
                    };
                    push_part("model", part, &mut out);
                }
            }
            _ => {
                for part in user_parts(&message.content) {
                    push_part("user", part, &mut out);
                }
            }
        }
    }
    (system, out)
}

/// Append `part` to the last content when it has `role`, else open one.
fn push_part<'a>(role: &'static str, part: Part<'a>, out: &mut Vec<Content<'a>>) {
    if let Some(last) = out.last_mut()
        && last.role == role
    {
        last.parts.push(part);
        return;
    }
    out.push(Content {
        role,
        parts: vec![part],
    });
}

/// A user message's parts: its text, and each `data:` image as `inlineData`.
/// An image this API cannot take (an `http` URL it would have to fetch) is
/// dropped rather than failing the turn.
fn user_parts(content: &MessageContent) -> Vec<Part<'_>> {
    match content {
        MessageContent::Text(text) => non_empty(Cow::Borrowed(text.as_str()))
            .map(|text| vec![Part::text(text)])
            .unwrap_or_default(),
        MessageContent::Parts(parts) => parts
            .iter()
            .filter_map(|part| match part {
                ContentPart::Text { text, .. } => {
                    non_empty(Cow::Borrowed(text.as_str())).map(Part::text)
                }
                ContentPart::ImageUrl { image_url } => {
                    let (mime_type, data) = image_url
                        .url
                        .strip_prefix("data:")?
                        .split_once(";base64,")?;
                    (!mime_type.is_empty() && !data.is_empty()).then(|| Part {
                        inline_data: Some(InlineData { mime_type, data }),
                        ..Part::default()
                    })
                }
            })
            .collect(),
    }
}

/// A message's text with any image parts dropped — borrowed when it is one
/// string.
fn flatten_text(content: &MessageContent) -> Cow<'_, str> {
    match content {
        MessageContent::Text(text) => Cow::Borrowed(text.as_str()),
        MessageContent::Parts(parts) => Cow::Owned(
            parts
                .iter()
                .filter_map(|part| match part {
                    ContentPart::Text { text, .. } => Some(text.as_str()),
                    ContentPart::ImageUrl { .. } => None,
                })
                .collect::<Vec<_>>()
                .join("\n"),
        ),
    }
}

/// `Some(text)` when it says anything — an empty text part is no part.
fn non_empty(text: Cow<'_, str>) -> Option<Cow<'_, str>> {
    (!text.trim().is_empty()).then_some(text)
}

/// The first `functionCall` of each model content carries its thought
/// signature — the memo's, else [`SKIP_THOUGHT_SIGNATURE`] — and the calls
/// after it in the same content carry none: the backend validates the first
/// and refuses a signature on a parallel call.
///
/// The one exception is a content that opens on a replayed Claude thought:
/// that thought's own signature vouches for the turn, and a sentinel beside
/// it is a pairing no reference implementation sends — so such a turn's
/// first call carries only a real signature, when the memo has one.
fn sign_first_calls(contents: &mut [Content<'_>], memos: &HashMap<String, CallMemo>) {
    for content in contents.iter_mut().filter(|c| c.role == "model") {
        let vouched = content.parts.iter().any(|p| p.thought);
        let mut first = true;
        for part in content
            .parts
            .iter_mut()
            .filter(|p| p.function_call.is_some())
        {
            part.thought_signature = if first {
                let real = part
                    .call_id
                    .and_then(|id| memos.get(id))
                    .and_then(|memo| memo.signature.clone())
                    .map(Cow::Owned);
                real.or((!vouched).then_some(Cow::Borrowed(SKIP_THOUGHT_SIGNATURE)))
            } else {
                None
            };
            first = false;
        }
    }
}

/// Open the current Claude tool loop on the thinking that started it.
///
/// The loop is every model content after the last user content that is not
/// tool results. Anthropic's rule, which the backend enforces for Claude, is
/// that the loop's first model turn must begin with its thinking block,
/// signature and all — so the memo's thought for that turn's first call goes
/// in front of it, and each later turn's too when the memo has it. Returns
/// whether the request may think: a loop whose opening thought the memo does
/// not have (a `/resume` mid-loop, a round sent without thinking) is sent
/// without thinking, and with no thought replayed, rather than with a block
/// the server would refuse.
fn open_loop_on_its_thinking<'a>(
    contents: &mut [Content<'a>],
    memos: &'a HashMap<String, CallMemo>,
) -> bool {
    let start = contents
        .iter()
        .rposition(|c| c.role == "user" && c.parts.iter().all(|p| p.function_response.is_none()))
        .map_or(0, |i| i + 1);
    let thought_of = |content: &Content<'_>| {
        content
            .parts
            .iter()
            .find_map(|p| p.call_id)
            .and_then(|id| memos.get(id))
            .and_then(|memo| memo.thought.as_ref())
    };
    let loop_turns: Vec<usize> = (start..contents.len())
        .filter(|&i| contents[i].role == "model")
        .collect();
    let Some(&opening) = loop_turns.first() else {
        return true;
    };
    if thought_of(&contents[opening]).is_none() {
        return false;
    }
    for i in loop_turns {
        let Some(thought) = contents[i]
            .parts
            .iter()
            .find_map(|p| p.call_id)
            .and_then(|id| memos.get(id))
            .and_then(|memo| memo.thought.as_ref())
        else {
            continue;
        };
        contents[i].parts.insert(
            0,
            Part {
                text: Some(Cow::Borrowed(thought.text.as_str())),
                thought: true,
                thought_signature: Some(Cow::Borrowed(thought.signature.as_str())),
                ..Part::default()
            },
        );
    }
    true
}

/// The two turn-order rules the backend enforces: a conversation opens on a
/// user turn, and never ends on a model one.
fn keep_turns_in_order(contents: &mut Vec<Content<'_>>) {
    let bridge = || Content {
        role: "user",
        parts: vec![Part::text(Cow::Borrowed(CONTINUE_TEXT))],
    };
    if contents.first().is_some_and(|c| c.role == "model") {
        contents.insert(0, bridge());
    }
    if contents.last().is_some_and(|c| c.role == "model") {
        contents.push(bridge());
    }
}

/// The `functionDeclarations` for the Chat Completions tool specs, each
/// schema translated for the family ([`gemini_schema`] / [`bridge_schema`]).
fn declarations(tools: &[Value], family: Family) -> Vec<Value> {
    tools
        .iter()
        .filter_map(|spec| {
            let function = spec.get("function")?;
            let name = function.get("name")?.as_str()?;
            let schema = function.get("parameters").cloned().unwrap_or(Value::Null);
            let mut declaration = Map::new();
            declaration.insert("name".to_string(), json!(name));
            if let Some(description) = function.get("description") {
                declaration.insert("description".to_string(), description.clone());
            }
            match family {
                Family::Gemini => {
                    declaration.insert("parametersJsonSchema".to_string(), gemini_schema(&schema));
                }
                Family::Claude | Family::GptOss => {
                    declaration.insert("parameters".to_string(), bridge_schema(&schema));
                }
            }
            Some(Value::Object(declaration))
        })
        .collect()
}

/// A tool schema as Gemini's `parametersJsonSchema` takes it: standard JSON
/// Schema, made self-contained — every local `$ref` replaced by what it
/// points at (with the referring node's own keywords over it), the
/// definitions and the meta keywords gone — and rooted on an object.
#[must_use]
pub fn gemini_schema(schema: &Value) -> Value {
    object_root(resolve(schema, schema, &mut Vec::new(), 0))
}

/// A tool schema as the Claude/GPT-OSS bridge takes it in `parameters`:
/// the refs resolved as for Gemini, then only `type`, `description`,
/// `properties`, `required`, `items` and `enum` kept — every other keyword
/// is a `400 Unknown name` there, and an allowlist cannot be broken by one
/// JSON Schema adds tomorrow. A type union keeps its first non-null type, an
/// `enum` survives only when every value is a string, `required` keeps only
/// names `properties` defines, and a property defined some other way
/// (`anyOf`) is `{}` — any value.
#[must_use]
pub fn bridge_schema(schema: &Value) -> Value {
    object_root(allowlisted(&resolve(schema, schema, &mut Vec::new(), 0)))
}

/// Resolve local `$ref`s and drop the meta keywords, walking only keyword
/// positions — under `properties` a key is a *name*, so a property called
/// `$ref` stays one.
fn resolve(node: &Value, root: &Value, refs: &mut Vec<String>, depth: usize) -> Value {
    if depth > SCHEMA_MAX_DEPTH {
        return json!({});
    }
    let Value::Object(map) = node else {
        return node.clone();
    };
    if let Some(Value::String(reference)) = map.get("$ref") {
        let target = local_target(root, reference);
        let Some(target) = target.filter(|_| !refs.contains(reference)) else {
            // An external ref, a missing target or a cycle: any value.
            return json!({});
        };
        refs.push(reference.clone());
        let mut resolved = resolve(target, root, refs, depth + 1);
        refs.pop();
        if let Value::Object(out) = &mut resolved {
            let siblings: Map<String, Value> = map
                .iter()
                .filter(|(k, _)| k.as_str() != "$ref")
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            if let Value::Object(siblings) =
                resolve(&Value::Object(siblings), root, refs, depth + 1)
            {
                out.extend(siblings);
            }
        }
        return resolved;
    }
    let mut out = Map::new();
    for (key, value) in map {
        if META_KEYWORDS.contains(&key.as_str()) {
            continue;
        }
        let walked = match key.as_str() {
            "properties" | "patternProperties" | "dependentSchemas" => match value {
                Value::Object(named) => Value::Object(
                    named
                        .iter()
                        .map(|(name, schema)| {
                            (name.clone(), resolve(schema, root, refs, depth + 1))
                        })
                        .collect(),
                ),
                other => other.clone(),
            },
            "items"
            | "additionalProperties"
            | "additionalItems"
            | "not"
            | "if"
            | "then"
            | "else"
            | "contains"
            | "propertyNames"
            | "unevaluatedItems"
            | "unevaluatedProperties"
            | "contentSchema"
            | "allOf"
            | "anyOf"
            | "oneOf"
            | "prefixItems" => match value {
                Value::Array(schemas) => Value::Array(
                    schemas
                        .iter()
                        .map(|schema| resolve(schema, root, refs, depth + 1))
                        .collect(),
                ),
                schema => resolve(schema, root, refs, depth + 1),
            },
            _ => value.clone(),
        };
        out.insert(key.clone(), walked);
    }
    Value::Object(out)
}

/// What a local `$ref` (`#`, `#/$defs/Step`) points at in `root`.
fn local_target<'a>(root: &'a Value, reference: &str) -> Option<&'a Value> {
    let pointer = reference.strip_prefix('#')?;
    if pointer.is_empty() {
        return Some(root);
    }
    root.pointer(pointer)
}

/// The bridge's six keywords, recursively.
fn allowlisted(node: &Value) -> Value {
    let Value::Object(map) = node else {
        return json!({});
    };
    let mut out = Map::new();
    let scalar_type = match map.get("type") {
        Some(Value::String(t)) => Some(t.clone()),
        Some(Value::Array(types)) => types
            .iter()
            .filter_map(Value::as_str)
            .find(|t| *t != "null")
            .map(str::to_string),
        _ => None,
    };
    if let Some(t) = scalar_type {
        out.insert("type".to_string(), json!(t));
    }
    if let Some(Value::String(description)) = map.get("description") {
        out.insert("description".to_string(), json!(description));
    }
    let properties = match map.get("properties") {
        Some(Value::Object(properties)) => Some(properties),
        _ => None,
    };
    if let Some(properties) = properties {
        out.insert(
            "properties".to_string(),
            Value::Object(
                properties
                    .iter()
                    .map(|(name, schema)| (name.clone(), allowlisted(schema)))
                    .collect(),
            ),
        );
    }
    if let Some(Value::Array(required)) = map.get("required") {
        let named: Vec<Value> = required
            .iter()
            .filter(|name| {
                name.as_str()
                    .is_some_and(|n| properties.is_some_and(|p| p.contains_key(n)))
            })
            .cloned()
            .collect();
        if !named.is_empty() {
            out.insert("required".to_string(), Value::Array(named));
        }
    }
    match map.get("items") {
        Some(items @ Value::Object(_)) => {
            out.insert("items".to_string(), allowlisted(items));
        }
        Some(Value::Array(tuple)) => {
            if let Some(first) = tuple.first() {
                out.insert("items".to_string(), allowlisted(first));
            }
        }
        _ => {}
    }
    if let Some(Value::Array(values)) = map.get("enum")
        && !values.is_empty()
        && values.iter().all(Value::is_string)
    {
        out.insert("enum".to_string(), Value::Array(values.clone()));
    }
    Value::Object(out)
}

/// Root a schema on an object with `properties` — a tool's parameters are
/// always an object, and a root that is not one (`true`, a missing schema)
/// takes no arguments.
fn object_root(schema: Value) -> Value {
    let Value::Object(mut map) = schema else {
        return json!({"type": "object", "properties": {}});
    };
    if !map.contains_key("type") {
        map.insert("type".to_string(), json!("object"));
    }
    if map.get("type") == Some(&json!("object")) && !map.contains_key("properties") {
        map.insert("properties".to_string(), json!({}));
    }
    Value::Object(map)
}

/// A random (version 4) UUID — the shape of a request id.
fn uuid_v4() -> String {
    let mut bytes = random_bytes::<16>();
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// `N` random bytes — from the system's source, else (no source) a clock
/// reading, unique enough for an id that only has to differ from the last.
fn random_bytes<const N: usize>() -> [u8; N] {
    let mut bytes = [0u8; N];
    if getrandom::fill(&mut bytes).is_err() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        for (i, b) in nanos.to_le_bytes().iter().cycle().take(N).enumerate() {
            bytes[i] = *b;
        }
    }
    bytes
}

// ---------------------------------------------------------------------------
// The stream
// ---------------------------------------------------------------------------

/// The most of a raw (unframed) body the stream keeps while waiting for it
/// to parse: the backend writes a JSON error body after its SSE frames on a
/// failure, a few hundred bytes at most.
const STRAY_MAX_BYTES: usize = 64 * 1024;

/// What one line of a Cloud Code stream did to the round.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Text to surface — reply and/or thought, and any whole tool calls.
    Delta(Delta),
    /// Nothing the caller needs (a blank line, a comment, a usage-only frame).
    Continue,
    /// The server failed the response in-band.
    Failed(String),
}

/// Folds a Cloud Code SSE stream into the crate's
/// [`StreamOutcome`](super::openai::StreamOutcome) currency.
///
/// Every `data:` line is one `GenerateContentResponse` wrapped in
/// `{"response": …}` (an unwrapped one reads the same). A tool call arrives
/// **whole**, in one `functionCall` part with its arguments as an object —
/// there is nothing to accumulate across frames but the calls themselves,
/// the finish reason and the usage. Gemini names no call ids, so each call
/// it leaves unnamed is given one (`fresh_call_id`); the thought signature
/// riding beside a call is kept with that id, because the next request must
/// hand it back (`docs/providers/antigravity.md` *Thought signatures*).
#[derive(Debug, Default)]
pub struct StreamAccumulator {
    calls: Vec<ToolCallRequest>,
    signatures: Vec<(String, String)>,
    /// The round's thought text, exactly as it streamed — a Claude round's
    /// is replayed to open its tool loop, and its signature vouches for
    /// these very bytes.
    thought_text: String,
    /// The signature the last thought part carried.
    thought_signature: Option<String>,
    finish_reason: Option<String>,
    usage: Option<TokenUsage>,
    /// Lines that are not SSE framing at all — the start of a raw JSON error
    /// body the server wrote after its frames.
    stray: String,
}

impl StreamAccumulator {
    /// Fold one line of the stream in.
    pub fn on_line(&mut self, line: &str) -> Step {
        let line = line.trim_end_matches('\r');
        if let Some(data) = line.strip_prefix("data:") {
            let data = data.trim();
            if data.is_empty() || data == "[DONE]" {
                return Step::Continue;
            }
            return match serde_json::from_str::<Value>(data) {
                Ok(value) => self.on_value(&value),
                Err(_) => Step::Continue,
            };
        }
        let trimmed = line.trim();
        if trimmed.is_empty()
            || trimmed.starts_with(':')
            || ["event:", "id:", "retry:"]
                .iter()
                .any(|field| trimmed.starts_with(field))
        {
            return Step::Continue;
        }
        // A line of a raw body: kept until the whole of it parses.
        self.stray.push_str(trimmed);
        self.stray.push('\n');
        if self.stray.len() > STRAY_MAX_BYTES {
            let head: String = self.stray.chars().take(300).collect();
            self.stray.clear();
            return Step::Failed(format!("unreadable response from the server: {head}"));
        }
        match serde_json::from_str::<Value>(&self.stray) {
            Ok(value) => {
                self.stray.clear();
                self.on_value(&value)
            }
            Err(_) => Step::Continue,
        }
    }

    /// One parsed frame — an error, or a (wrapped) `GenerateContentResponse`.
    fn on_value(&mut self, value: &Value) -> Step {
        if let Some(message) = error_message(value) {
            return Step::Failed(message);
        }
        let response = value.get("response").unwrap_or(value);
        if let Some(usage) = response.get("usageMetadata") {
            self.merge_usage(usage);
        }
        let Some(candidate) = response.get("candidates").and_then(|c| c.get(0)) else {
            return Step::Continue;
        };
        let mut delta = Delta::default();
        let parts = candidate
            .pointer("/content/parts")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        for part in parts {
            if let Some(call) = part.get("functionCall") {
                self.take_call(call, part, &mut delta);
                continue;
            }
            let Some(text) = part.get("text").and_then(Value::as_str) else {
                continue;
            };
            if part.get("thought").and_then(Value::as_bool) == Some(true) {
                delta.reasoning.push_str(text);
                self.thought_text.push_str(text);
                if let Some(signature) = part
                    .get("thoughtSignature")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                {
                    self.thought_signature = Some(signature.to_string());
                }
            } else {
                delta.response.push_str(text);
            }
        }
        if let Some(reason) = candidate
            .get("finishReason")
            .and_then(Value::as_str)
            .filter(|r| !r.is_empty() && *r != "FINISH_REASON_UNSPECIFIED")
        {
            self.finish_reason = Some(reason.to_string());
        }
        if delta == Delta::default() {
            Step::Continue
        } else {
            Step::Delta(delta)
        }
    }

    /// A whole `functionCall` part: recorded as a call, surfaced under the
    /// next slot, and its signature kept beside its id.
    fn take_call(&mut self, call: &Value, part: &Value, delta: &mut Delta) {
        let name = call
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if name.is_empty() {
            return;
        }
        // An absent argument object is a call with no parameters — `{}`,
        // never the empty string, which no tool can parse back.
        let arguments = match call.get("args") {
            None | Some(Value::Null) => "{}".to_string(),
            Some(args) => args.to_string(),
        };
        let id = call
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .map_or_else(fresh_call_id, str::to_string);
        if let Some(signature) = part
            .get("thoughtSignature")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        {
            self.signatures.push((id.clone(), signature.to_string()));
        }
        delta.tool_calls.push(ToolCallFragment {
            index: self.calls.len(),
            name: name.clone(),
            arguments: arguments.clone(),
        });
        self.calls.push(ToolCallRequest {
            id,
            name,
            arguments,
        });
    }

    /// Merge one frame's `usageMetadata`. The counters are cumulative, so
    /// each is kept at the most any frame reported — a later frame that omits
    /// one never reads as "none". `promptTokenCount` already includes the
    /// cached share, which is what [`TokenUsage::input`] means; the output is
    /// the reply **and** the thinking, which is what the round billed.
    fn merge_usage(&mut self, usage: &Value) {
        let count = |key: &str| usage.get(key).and_then(Value::as_u64);
        let merged = self.usage.get_or_insert_with(TokenUsage::default);
        if let Some(prompt) = count("promptTokenCount") {
            merged.input = merged.input.max(prompt);
        }
        if let Some(cached) = count("cachedContentTokenCount") {
            merged.cached = merged.cached.max(cached);
        }
        let thoughts = count("thoughtsTokenCount").unwrap_or(0);
        merged.reasoning = merged.reasoning.max(thoughts);
        if let Some(reply) = count("candidatesTokenCount") {
            merged.output = merged.output.max(reply.saturating_add(thoughts));
        }
    }

    /// What the round produced.
    ///
    /// # Errors
    /// A stream that ended without a finish reason: the server stopped
    /// writing before the model finished — a reply cut short, not a reply.
    pub fn finish(self) -> Result<RoundEnd, String> {
        let Some(raw) = self.finish_reason else {
            return Err("the stream ended before the model finished its reply".to_string());
        };
        // A round that requested tools finished *for* those tools, which is
        // what the agent loop reads to decide to loop again.
        let finish_reason = if self.calls.is_empty() {
            crate_reason(&raw)
        } else {
            "tool_calls".to_string()
        };
        // The thought's own signature, else the one its round's first call
        // carried — a thought with neither cannot be handed back at all.
        let signature = self.thought_signature.or_else(|| {
            let first = self.calls.first()?;
            self.signatures
                .iter()
                .find(|(id, _)| *id == first.id)
                .map(|(_, signature)| signature.clone())
        });
        let thought = signature
            .filter(|_| !self.thought_text.is_empty())
            .map(|signature| Thought {
                text: self.thought_text,
                signature,
            });
        Ok(RoundEnd {
            calls: self.calls,
            finish_reason: Some(finish_reason),
            usage: self.usage,
            signatures: self.signatures,
            thought,
        })
    }
}

/// A Gemini `finishReason` in the crate's own words — the spellings
/// [`StreamOutcome::stop_reason`](super::openai::StreamOutcome::stop_reason)
/// reads. A reason with no counterpart passes through lower-cased, which
/// reads as a finish: `MALFORMED_FUNCTION_CALL` ends a round that answered
/// nothing, and the agent loop asks the model to carry on
/// (`docs/architecture/unfinished-replies.md`).
fn crate_reason(raw: &str) -> String {
    match raw {
        "STOP" => "stop".to_string(),
        "MAX_TOKENS" => "length".to_string(),
        "SAFETY" | "RECITATION" | "BLOCKLIST" | "PROHIBITED_CONTENT" | "SPII" | "IMAGE_SAFETY" => {
            "content_filter".to_string()
        }
        other => other.to_ascii_lowercase(),
    }
}

/// The message of an in-band error object — `{"error": {"message": …}}` or
/// `{"error": "…"}` — when the frame is one.
fn error_message(value: &Value) -> Option<String> {
    let error = value.get("error")?;
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .or_else(|| error.as_str())
        .filter(|m| !m.trim().is_empty())
        .map_or_else(|| error.to_string(), str::to_string);
    Some(message)
}

/// A call id of this client's own, for a call the model named none for.
/// Random rather than counted: ids are recorded with the conversation and
/// replayed after a `/resume`, where a counter starting over would hand two
/// calls of one conversation the same id — and a thought signature is
/// looked up by it.
fn fresh_call_id() -> String {
    let hex: String = random_bytes::<8>()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    format!("call_{hex}")
}

/// What a finished Cloud Code round produced: its tool calls, the
/// `finish_reason` the rest of the crate speaks, the provider's usage, and
/// the thought signatures that came with the calls.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RoundEnd {
    pub calls: Vec<ToolCallRequest>,
    pub finish_reason: Option<String>,
    pub usage: Option<TokenUsage>,
    /// `(call id, signature)` for every call that came signed.
    pub signatures: Vec<(String, String)>,
    /// The round's reasoning and the signature vouching for it, when it came
    /// signed.
    pub thought: Option<Thought>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Feed every line, collecting the steps.
    fn drive(lines: &[&str]) -> (StreamAccumulator, Vec<Step>) {
        let mut acc = StreamAccumulator::default();
        let steps = lines.iter().map(|line| acc.on_line(line)).collect();
        (acc, steps)
    }

    fn frame(response: &str) -> String {
        format!(r#"data: {{"response": {response}}}"#)
    }

    #[test]
    fn a_text_part_is_reply_and_a_thought_part_is_reasoning() {
        let thought = frame(
            r#"{"candidates":[{"content":{"role":"model","parts":[{"text":"weighing it","thought":true}]}}]}"#,
        );
        let text =
            frame(r#"{"candidates":[{"content":{"role":"model","parts":[{"text":"Hello"}]}}]}"#);
        let (_, steps) = drive(&[&thought, &text]);
        assert_eq!(
            steps,
            vec![
                Step::Delta(Delta {
                    reasoning: "weighing it".to_string(),
                    ..Delta::default()
                }),
                Step::Delta(Delta {
                    response: "Hello".to_string(),
                    ..Delta::default()
                }),
            ]
        );
    }

    #[test]
    fn a_function_call_arrives_whole_and_is_surfaced_under_its_slot() {
        let call = frame(
            r#"{"candidates":[{"content":{"role":"model","parts":[{"functionCall":{"name":"read","args":{"path":"/tmp/a"}},"thoughtSignature":"c2ln"}]}}]}"#,
        );
        let (acc, steps) = drive(&[&call]);
        let Step::Delta(delta) = &steps[0] else {
            panic!("{steps:?}");
        };
        assert_eq!(delta.tool_calls.len(), 1);
        assert_eq!(delta.tool_calls[0].index, 0);
        assert_eq!(delta.tool_calls[0].name, "read");
        assert_eq!(delta.tool_calls[0].arguments, r#"{"path":"/tmp/a"}"#);
        let end = {
            let mut acc = acc;
            acc.on_line(&frame(r#"{"candidates":[{"finishReason":"STOP"}]}"#));
            acc.finish().unwrap()
        };
        assert_eq!(end.calls.len(), 1);
        assert_eq!(end.calls[0].name, "read");
        assert_eq!(end.calls[0].arguments, r#"{"path":"/tmp/a"}"#);
        assert!(
            end.calls[0].id.starts_with("call_") && end.calls[0].id.len() > "call_".len(),
            "a call Gemini named no id for gets one of its own: {}",
            end.calls[0].id
        );
        assert_eq!(
            end.signatures,
            vec![(end.calls[0].id.clone(), "c2ln".to_string())],
            "the signature rides with the call it signed"
        );
        assert_eq!(end.finish_reason.as_deref(), Some("tool_calls"));
    }

    #[test]
    fn a_call_id_the_model_named_is_kept_and_every_synthetic_one_is_distinct() {
        let calls = frame(
            &r#"{"candidates":[{"content":{"role":"model","parts":[
                {"functionCall":{"id":"toolu_01","name":"bash","args":{"command":"ls"}}},
                {"functionCall":{"name":"read","args":{}}},
                {"functionCall":{"name":"read","args":{}}}
            ]},"finishReason":"STOP"}]}"#
                .replace('\n', ""),
        );
        let (acc, steps) = drive(&[&calls]);
        let Step::Delta(delta) = &steps[0] else {
            panic!("{steps:?}");
        };
        let slots: Vec<usize> = delta.tool_calls.iter().map(|c| c.index).collect();
        assert_eq!(slots, vec![0, 1, 2], "one slot per call, in order");
        let end = acc.finish().unwrap();
        assert_eq!(end.calls[0].id, "toolu_01");
        assert_ne!(end.calls[1].id, end.calls[2].id);
        assert_eq!(end.calls[1].arguments, "{}", "no arguments is `{{}}`");
        assert!(end.signatures.is_empty(), "nothing came signed");
    }

    #[test]
    fn calls_across_frames_keep_counting_their_slots() {
        let first = frame(
            r#"{"candidates":[{"content":{"parts":[{"functionCall":{"name":"read","args":{"path":"a"}}}]}}]}"#,
        );
        let second = frame(
            r#"{"candidates":[{"content":{"parts":[{"functionCall":{"name":"read","args":{"path":"b"}}}]},"finishReason":"STOP"}]}"#,
        );
        let (acc, steps) = drive(&[&first, &second]);
        let slot = |step: &Step| match step {
            Step::Delta(delta) => delta.tool_calls[0].index,
            other => panic!("{other:?}"),
        };
        assert_eq!((slot(&steps[0]), slot(&steps[1])), (0, 1));
        assert_eq!(acc.finish().unwrap().calls.len(), 2);
    }

    #[test]
    fn the_finish_reason_is_spoken_in_the_crates_own_words() {
        for (raw, want) in [
            ("STOP", "stop"),
            ("MAX_TOKENS", "length"),
            ("SAFETY", "content_filter"),
            ("RECITATION", "content_filter"),
            ("PROHIBITED_CONTENT", "content_filter"),
            ("MALFORMED_FUNCTION_CALL", "malformed_function_call"),
        ] {
            let (acc, _) = drive(&[&frame(&format!(
                r#"{{"candidates":[{{"content":{{"parts":[{{"text":"x"}}]}},"finishReason":"{raw}"}}]}}"#
            ))]);
            assert_eq!(
                acc.finish().unwrap().finish_reason.as_deref(),
                Some(want),
                "{raw}"
            );
        }
    }

    #[test]
    fn usage_counts_the_whole_prompt_and_thinking_as_output() {
        let first = frame(
            r#"{"candidates":[{"content":{"parts":[{"text":"a"}]}}],"usageMetadata":{"promptTokenCount":90}}"#,
        );
        let last = frame(
            r#"{"candidates":[{"content":{"parts":[{"text":"b"}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":100,"cachedContentTokenCount":40,"candidatesTokenCount":7,"thoughtsTokenCount":3,"totalTokenCount":110}}"#,
        );
        let (acc, _) = drive(&[&first, &last]);
        assert_eq!(
            acc.finish().unwrap().usage,
            Some(TokenUsage {
                input: 100,
                output: 10,
                cached: 40,
                cache_write: 0,
                reasoning: 3,
            }),
            "the last frame's counts stand; the prompt count already includes the cached share"
        );
    }

    #[test]
    fn an_unwrapped_frame_reads_the_same_as_a_wrapped_one() {
        let (_, steps) =
            drive(&[r#"data: {"candidates":[{"content":{"parts":[{"text":"hi"}]}}]}"#]);
        assert_eq!(
            steps,
            vec![Step::Delta(Delta {
                response: "hi".to_string(),
                ..Delta::default()
            })]
        );
    }

    #[test]
    fn an_error_frame_fails_the_stream_with_its_message() {
        let (_, steps) = drive(&[
            r#"data: {"error":{"code":503,"message":"No capacity available for model gemini-3.8-flash-medium on the server","status":"UNAVAILABLE"}}"#,
        ]);
        assert_eq!(
            steps,
            vec![Step::Failed(
                "No capacity available for model gemini-3.8-flash-medium on the server".to_string()
            )]
        );
    }

    #[test]
    fn a_raw_json_error_after_the_frames_fails_the_stream() {
        // The backend can follow its SSE frames with a multi-line JSON error
        // body that is no `data:` line at all.
        let thought =
            frame(r#"{"candidates":[{"content":{"parts":[{"text":"planning","thought":true}]}}]}"#);
        let (_, steps) = drive(&[
            &thought,
            "{",
            r#"  "error": { "code": 503, "message": "No capacity available", "status": "UNAVAILABLE" }"#,
            "}",
        ]);
        assert_eq!(
            steps.last(),
            Some(&Step::Failed("No capacity available".to_string()))
        );
    }

    #[test]
    fn blank_lines_comments_and_done_are_ignored() {
        let (acc, steps) = drive(&["", ": keepalive", "data: [DONE]", "data:"]);
        assert!(steps.iter().all(|s| *s == Step::Continue), "{steps:?}");
        assert!(acc.finish().is_err(), "no finish reason was ever seen");
    }

    #[test]
    fn a_stream_that_ends_without_a_finish_reason_is_cut_short() {
        let (acc, _) = drive(&[&frame(
            r#"{"candidates":[{"content":{"parts":[{"text":"half a sen"}]}}]}"#,
        )]);
        let error = acc.finish().unwrap_err();
        assert!(error.contains("ended before"), "{error}");
    }

    #[test]
    fn a_round_keeps_its_thought_with_the_signature_that_vouches_for_it() {
        let lines = [
            frame(
                r#"{"candidates":[{"content":{"parts":[{"text":"I should ","thought":true}]}}]}"#,
            ),
            frame(
                r#"{"candidates":[{"content":{"parts":[{"text":"read it.","thought":true},{"text":"","thought":true,"thoughtSignature":"sig-1"}]}}]}"#,
            ),
            frame(
                r#"{"candidates":[{"content":{"parts":[{"functionCall":{"id":"toolu_1","name":"read","args":{}}}]},"finishReason":"OTHER"}]}"#,
            ),
        ];
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        let (acc, _) = drive(&refs);
        let end = acc.finish().unwrap();
        assert_eq!(
            end.thought,
            Some(Thought {
                text: "I should read it.".to_string(),
                signature: "sig-1".to_string(),
            })
        );
        assert_eq!(end.finish_reason.as_deref(), Some("tool_calls"));
        // An unsigned thought cannot be handed back, so none is kept.
        let (acc, _) = drive(&[&frame(
            r#"{"candidates":[{"content":{"parts":[{"text":"hmm","thought":true},{"text":"Hi"}]},"finishReason":"STOP"}]}"#,
        )]);
        assert_eq!(acc.finish().unwrap().thought, None);
    }

    #[test]
    fn an_empty_text_part_carrying_only_a_signature_surfaces_nothing() {
        let (_, steps) = drive(&[&frame(
            r#"{"candidates":[{"content":{"parts":[{"text":"","thoughtSignature":"c2ln"}]}}]}"#,
        )]);
        assert_eq!(steps, vec![Step::Continue]);
    }

    // -----------------------------------------------------------------------
    // The request
    // -----------------------------------------------------------------------

    use super::super::config::ModelConfig;
    use super::super::reasoning::{ReasoningEffort, ThinkingMode};
    use super::super::{ChatMessage, ContentPart, ToolCallSpec};
    use std::collections::HashMap;

    fn cfg(model: &str) -> ModelConfig {
        let mut cfg = ModelConfig::fallback();
        cfg.model = model.to_string();
        cfg.api_base = "https://daily-cloudcode-pa.googleapis.com".to_string();
        cfg.cache_key = Some("alter-zero-7".to_string());
        cfg
    }

    fn payload(cfg: &ModelConfig, tools: &[Value], messages: &[ChatMessage]) -> Value {
        build_payload(cfg, tools, messages, Some("proj-1"), &HashMap::new())
    }

    fn read_tool() -> Value {
        serde_json::json!({
            "type": "function",
            "function": {
                "name": "read",
                "description": "Read a file.",
                "parameters": {
                    "$schema": "http://json-schema.org/draft-07/schema#",
                    "type": "object",
                    "properties": {"path": {"type": "string", "description": "The path."}},
                    "required": ["path"],
                    "additionalProperties": false
                }
            }
        })
    }

    #[test]
    fn the_envelope_names_the_project_the_model_and_the_agent() {
        let body = payload(&cfg("gemini-3-flash"), &[], &[ChatMessage::user("hi")]);
        assert_eq!(body["project"], "proj-1");
        assert_eq!(body["model"], "gemini-3-flash");
        assert_eq!(body["requestType"], "agent");
        assert_eq!(body["userAgent"], "antigravity");
        let id = body["requestId"].as_str().expect("a request id");
        assert!(id.starts_with("agent-") && id.len() > 20, "{id}");
        assert_eq!(body["request"]["sessionId"], "alter-zero-7");
        assert_eq!(
            body["request"]["contents"],
            serde_json::json!([{"role": "user", "parts": [{"text": "hi"}]}])
        );
        let other = payload(&cfg("gemini-3-flash"), &[], &[ChatMessage::user("hi")]);
        assert_ne!(other["requestId"], body["requestId"], "one id per request");
    }

    #[test]
    fn the_system_prompt_is_hoisted_into_the_system_instruction() {
        let body = payload(
            &cfg("gemini-3-flash"),
            &[],
            &[ChatMessage::system("Be terse."), ChatMessage::user("hi")],
        );
        assert_eq!(
            body["request"]["systemInstruction"],
            serde_json::json!({"role": "user", "parts": [{"text": "Be terse."}]})
        );
        assert_eq!(body["request"]["contents"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn no_client_identity_is_ever_injected_into_the_system_instruction() {
        // Antigravity's own clients open with "You are Antigravity…"; the
        // backend does not require it, and this app sends the user's own
        // system prompt and nothing else (`docs/providers/antigravity.md`).
        for model in [
            "gemini-3-flash",
            "claude-opus-4-6-thinking",
            "gpt-oss-120b-medium",
        ] {
            let with = payload(
                &cfg(model),
                &[read_tool()],
                &[
                    ChatMessage::system("You are Alter Zero."),
                    ChatMessage::user("hi"),
                ],
            );
            assert_eq!(
                with["request"]["systemInstruction"]["parts"],
                serde_json::json!([{"text": "You are Alter Zero."}]),
                "{model}"
            );
            let without = payload(&cfg(model), &[read_tool()], &[ChatMessage::user("hi")]);
            assert!(
                without["request"].get("systemInstruction").is_none(),
                "{model}: no system prompt, no system instruction"
            );
            assert!(!with.to_string().contains("Antigravity,"), "{model}");
        }
    }

    #[test]
    fn a_tool_round_becomes_call_and_response_parts_with_results_in_one_user_turn() {
        let messages = vec![
            ChatMessage::user("look"),
            ChatMessage::assistant_tool_calls(
                "Reading both.",
                vec![
                    ToolCallSpec::function("c1", "read", r#"{"path":"a"}"#),
                    ToolCallSpec::function("c2", "read", r#"{"path":"b"}"#),
                ],
            ),
            ChatMessage::tool_result("c1", "A"),
            ChatMessage::tool_result("c2", "B"),
        ];
        let body = payload(&cfg("gemini-3-flash"), &[], &messages);
        let contents = body["request"]["contents"].as_array().unwrap();
        assert_eq!(contents.len(), 3, "{contents:#?}");
        assert_eq!(contents[1]["role"], "model");
        assert_eq!(
            contents[1]["parts"][0],
            serde_json::json!({"text": "Reading both."})
        );
        assert_eq!(
            contents[1]["parts"][1]["functionCall"],
            serde_json::json!({"name": "read", "args": {"path": "a"}}),
            "Gemini is sent no call ids"
        );
        assert_eq!(contents[2]["role"], "user");
        assert_eq!(
            contents[2]["parts"],
            serde_json::json!([
                {"functionResponse": {"name": "read", "response": {"output": "A"}}},
                {"functionResponse": {"name": "read", "response": {"output": "B"}}}
            ]),
            "a batch's results are one user turn, each naming its tool"
        );
    }

    #[test]
    fn claude_and_gpt_oss_pair_results_with_calls_by_id() {
        let messages = vec![
            ChatMessage::user("look"),
            ChatMessage::assistant_tool_calls(
                "",
                vec![ToolCallSpec::function(
                    "toolu_1",
                    "bash",
                    r#"{"command":"ls"}"#,
                )],
            ),
            ChatMessage::tool_result("toolu_1", "a.txt"),
        ];
        for model in ["claude-sonnet-4-6", "gpt-oss-120b-medium"] {
            let body = payload(&cfg(model), &[], &messages);
            let contents = body["request"]["contents"].as_array().unwrap();
            assert_eq!(
                contents[1]["parts"][0]["functionCall"]["id"], "toolu_1",
                "{model}"
            );
            assert_eq!(
                contents[2]["parts"][0]["functionResponse"]["id"], "toolu_1",
                "{model}"
            );
            assert_eq!(
                contents[1]["parts"].as_array().unwrap().len(),
                1,
                "{model}: an empty reply text is no part"
            );
        }
    }

    #[test]
    fn the_first_call_of_each_model_turn_is_signed_and_the_rest_are_not() {
        let messages = vec![
            ChatMessage::user("go"),
            ChatMessage::assistant_tool_calls(
                "",
                vec![
                    ToolCallSpec::function("c1", "read", "{}"),
                    ToolCallSpec::function("c2", "read", "{}"),
                ],
            ),
            ChatMessage::tool_result("c1", "1"),
            ChatMessage::tool_result("c2", "2"),
            ChatMessage::assistant_tool_calls("", vec![ToolCallSpec::function("c3", "bash", "{}")]),
            ChatMessage::tool_result("c3", "3"),
        ];
        let mut memos = HashMap::new();
        memos.insert(
            "c1".to_string(),
            CallMemo {
                signature: Some("real-signature".to_string()),
                thought: None,
            },
        );
        let body = build_payload(&cfg("gemini-3-flash"), &[], &messages, Some("p"), &memos);
        let contents = body["request"]["contents"].as_array().unwrap();
        assert_eq!(
            contents[1]["parts"][0]["thoughtSignature"],
            "real-signature"
        );
        assert!(
            contents[1]["parts"][1].get("thoughtSignature").is_none(),
            "a parallel call after the first carries none"
        );
        assert_eq!(
            contents[3]["parts"][0]["thoughtSignature"], SKIP_THOUGHT_SIGNATURE,
            "a round the memo does not know is vouched for by the sentinel"
        );
    }

    #[test]
    fn an_image_rides_as_inline_data_split_out_of_its_data_url() {
        let messages = vec![ChatMessage::with_parts(
            "user",
            vec![
                ContentPart::text("what is this?"),
                ContentPart::image("data:image/png;base64,iVBORw0KGgo="),
                ContentPart::image("https://example.com/cat.png"),
            ],
        )];
        let body = payload(&cfg("gemini-3-flash"), &[], &messages);
        assert_eq!(
            body["request"]["contents"][0]["parts"],
            serde_json::json!([
                {"text": "what is this?"},
                {"inlineData": {"mimeType": "image/png", "data": "iVBORw0KGgo="}}
            ]),
            "a URL the API cannot fetch is dropped rather than failing the turn"
        );
    }

    #[test]
    fn consecutive_turns_of_one_role_merge_and_empty_ones_vanish() {
        let messages = vec![
            ChatMessage::user("one"),
            ChatMessage::user("two"),
            ChatMessage::assistant(""),
            ChatMessage::user("three"),
        ];
        let body = payload(&cfg("gemini-3-flash"), &[], &messages);
        assert_eq!(
            body["request"]["contents"],
            serde_json::json!([{"role": "user", "parts": [{"text": "one"}, {"text": "two"}, {"text": "three"}]}])
        );
    }

    #[test]
    fn a_request_never_ends_on_a_model_turn() {
        let messages = vec![ChatMessage::user("hi"), ChatMessage::assistant("Hello.")];
        let body = payload(&cfg("gemini-3-flash"), &[], &messages);
        let contents = body["request"]["contents"].as_array().unwrap();
        assert_eq!(contents.last().unwrap()["role"], "user");
        assert_eq!(
            contents.last().unwrap()["parts"],
            serde_json::json!([{"text": "Continue."}])
        );
    }

    #[test]
    fn gemini_tools_take_json_schema_with_refs_inlined_and_meta_keywords_gone() {
        let tool = serde_json::json!({
            "type": "function",
            "function": {
                "name": "plan",
                "description": "Plan.",
                "parameters": {
                    "$schema": "http://json-schema.org/draft-07/schema#",
                    "$id": "x",
                    "type": "object",
                    "$defs": {"Step": {"type": "object", "properties": {"title": {"type": "string"}}}},
                    "properties": {
                        "steps": {"type": "array", "items": {"$ref": "#/$defs/Step", "description": "one step"}},
                        "$ref": {"type": "string"}
                    }
                }
            }
        });
        let body = payload(&cfg("gemini-3-flash"), &[tool], &[ChatMessage::user("hi")]);
        let declaration = &body["request"]["tools"][0]["functionDeclarations"][0];
        assert_eq!(declaration["name"], "plan");
        assert_eq!(declaration["description"], "Plan.");
        assert!(declaration.get("parameters").is_none());
        assert_eq!(
            declaration["parametersJsonSchema"],
            serde_json::json!({
                "type": "object",
                "properties": {
                    "steps": {"type": "array", "items": {
                        "type": "object",
                        "properties": {"title": {"type": "string"}},
                        "description": "one step"
                    }},
                    "$ref": {"type": "string"}
                }
            }),
            "the ref resolved with its siblings over it; a property named `$ref` is a name"
        );
    }

    #[test]
    fn claude_tools_take_only_the_keywords_the_bridge_accepts() {
        let tool = serde_json::json!({
            "type": "function",
            "function": {
                "name": "edit",
                "description": "Edit.",
                "parameters": {
                    "$schema": "http://json-schema.org/draft-07/schema#",
                    "type": "object",
                    "properties": {
                        "path": {"type": ["string", "null"], "format": "uri", "minLength": 1},
                        "mode": {"type": "string", "enum": ["a", "b"], "default": "a"},
                        "count": {"type": "integer", "enum": [1, 2]},
                        "either": {"anyOf": [{"type": "string"}, {"type": "integer"}]},
                        "tags": {"type": "array", "items": {"type": "string", "pattern": "x"}}
                    },
                    "required": ["path", "ghost"],
                    "additionalProperties": false
                }
            }
        });
        let body = payload(
            &cfg("claude-opus-4-6-thinking"),
            &[tool],
            &[ChatMessage::user("hi")],
        );
        let declaration = &body["request"]["tools"][0]["functionDeclarations"][0];
        assert!(declaration.get("parametersJsonSchema").is_none());
        assert_eq!(
            declaration["parameters"],
            serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "mode": {"type": "string", "enum": ["a", "b"]},
                    "count": {"type": "integer"},
                    "either": {},
                    "tags": {"type": "array", "items": {"type": "string"}}
                },
                "required": ["path"]
            })
        );
    }

    #[test]
    fn a_schema_with_no_object_root_is_given_one() {
        let tool = serde_json::json!({
            "type": "function",
            "function": {"name": "ping", "description": "Ping.", "parameters": true}
        });
        for model in ["gemini-3-flash", "claude-sonnet-4-6"] {
            let body = payload(
                &cfg(model),
                std::slice::from_ref(&tool),
                &[ChatMessage::user("hi")],
            );
            let declaration = &body["request"]["tools"][0]["functionDeclarations"][0];
            let schema = declaration
                .get("parametersJsonSchema")
                .or_else(|| declaration.get("parameters"))
                .unwrap();
            assert_eq!(
                schema,
                &serde_json::json!({"type": "object", "properties": {}}),
                "{model}"
            );
        }
    }

    #[test]
    fn gemini_asks_for_its_thoughts_and_leaves_the_level_to_the_model_id() {
        let mut config = cfg("gemini-3.8-flash");
        config.thinking = Some(ThinkingMode::Effort(ReasoningEffort::High));
        let body = payload(&config, &[], &[ChatMessage::user("hi")]);
        assert_eq!(
            body["model"], "gemini-3.8-flash-high",
            "the level is the runtime id's suffix"
        );
        assert_eq!(
            body["request"]["generationConfig"],
            serde_json::json!({"maxOutputTokens": 65535, "thinkingConfig": {"includeThoughts": true}})
        );
        config.thinking = None;
        let body = payload(&config, &[], &[ChatMessage::user("hi")]);
        assert_eq!(body["model"], "gemini-3.8-flash");
        assert_eq!(
            body["request"]["generationConfig"],
            serde_json::json!({"maxOutputTokens": 65535}),
            "a model with no thinking mode is asked for none"
        );
        config.thinking = Some(ThinkingMode::On);
        config.model = "gpt-oss-120b-medium".to_string();
        let body = payload(&config, &[], &[ChatMessage::user("hi")]);
        assert_eq!(
            body["model"], "gpt-oss-120b-medium",
            "a listed id is sent verbatim"
        );
        assert_eq!(
            body["request"]["generationConfig"],
            serde_json::json!({"maxOutputTokens": 32768, "thinkingConfig": {"includeThoughts": true, "thinkingBudget": 8192}})
        );
    }

    #[test]
    fn claude_thinks_on_a_budget_below_its_output_cap_and_is_never_sent_a_temperature() {
        let mut config = cfg("claude-opus-4-6-thinking");
        config.thinking = Some(ThinkingMode::On);
        config.temperature = Some(0.2);
        let body = payload(&config, &[], &[ChatMessage::user("hi")]);
        assert_eq!(
            body["request"]["generationConfig"],
            serde_json::json!({
                "maxOutputTokens": 64000,
                "thinkingConfig": {"includeThoughts": true, "thinkingBudget": CLAUDE_THINKING_BUDGET}
            })
        );
        // A Claude id that is not a thinking route takes no budget at all,
        // whatever mode a stale selection carries.
        config.model = "claude-sonnet-4-6".to_string();
        let body = payload(&config, &[], &[ChatMessage::user("hi")]);
        assert_eq!(
            body["request"]["generationConfig"],
            serde_json::json!({"maxOutputTokens": 64000})
        );
        // Gemini takes the temperature it is given.
        let mut gemini = cfg("gemini-3-flash");
        gemini.temperature = Some(0.5);
        let body = payload(&gemini, &[], &[ChatMessage::user("hi")]);
        assert_eq!(body["request"]["generationConfig"]["temperature"], 0.5);
    }

    #[test]
    fn a_claude_tool_loop_opens_on_the_thinking_that_started_it() {
        let messages = vec![
            ChatMessage::user("fix it"),
            ChatMessage::assistant_tool_calls("", vec![ToolCallSpec::function("t1", "read", "{}")]),
            ChatMessage::tool_result("t1", "x"),
            ChatMessage::assistant_tool_calls("", vec![ToolCallSpec::function("t2", "edit", "{}")]),
            ChatMessage::tool_result("t2", "y"),
        ];
        let mut config = cfg("claude-opus-4-6-thinking");
        config.thinking = Some(ThinkingMode::On);
        let mut memos = HashMap::new();
        memos.insert(
            "t1".to_string(),
            CallMemo {
                signature: Some("call-sig".to_string()),
                thought: Some(Thought {
                    text: "I should read first.".to_string(),
                    signature: "thought-sig".to_string(),
                }),
            },
        );
        let body = build_payload(&config, &[], &messages, Some("p"), &memos);
        let contents = body["request"]["contents"].as_array().unwrap();
        assert_eq!(
            contents[1]["parts"][0],
            serde_json::json!({"text": "I should read first.", "thought": true, "thoughtSignature": "thought-sig"})
        );
        assert_eq!(contents[1]["parts"][1]["functionCall"]["name"], "read");
        assert_eq!(contents[1]["parts"][1]["thoughtSignature"], "call-sig");
        assert!(body["request"]["generationConfig"]["thinkingConfig"].is_object());
        // A later turn of the loop the memo has nothing for keeps the
        // sentinel; a turn that opens on a replayed thought is vouched for by
        // it, and never also by the sentinel — a pairing no reference sends.
        assert_eq!(
            contents[3]["parts"][0]["thoughtSignature"],
            SKIP_THOUGHT_SIGNATURE
        );
        memos.get_mut("t1").unwrap().signature = None;
        let body = build_payload(&config, &[], &messages, Some("p"), &memos);
        let contents = body["request"]["contents"].as_array().unwrap();
        assert!(
            contents[1]["parts"][0]["thought"]
                .as_bool()
                .unwrap_or(false)
        );
        assert!(
            contents[1]["parts"][1].get("thoughtSignature").is_none(),
            "{:?}",
            contents[1]
        );

        // The memo lost the loop's opening round (a `/resume` mid-loop): the
        // round goes out without thinking rather than with a block the
        // server would refuse.
        let body = build_payload(&config, &[], &messages, Some("p"), &HashMap::new());
        assert!(
            body["request"]["generationConfig"]
                .get("thinkingConfig")
                .is_none()
        );
        assert!(
            !body.to_string().contains("\"thought\":true"),
            "no thought is replayed into a round sent without thinking"
        );
    }

    #[test]
    fn a_claude_conversation_outside_a_tool_loop_thinks_as_usual() {
        let messages = vec![
            ChatMessage::user("fix it"),
            ChatMessage::assistant_tool_calls("", vec![ToolCallSpec::function("t1", "read", "{}")]),
            ChatMessage::tool_result("t1", "x"),
            ChatMessage::assistant("Done."),
            ChatMessage::user("thanks — now the docs"),
        ];
        let mut config = cfg("claude-opus-4-6-thinking");
        config.thinking = Some(ThinkingMode::On);
        let body = build_payload(&config, &[], &messages, Some("p"), &HashMap::new());
        assert!(
            body["request"]["generationConfig"]["thinkingConfig"].is_object(),
            "an earlier turn's thinking is not needed: a new turn starts on its own"
        );
    }

    #[test]
    fn provider_kwargs_reach_the_inner_request_and_refused_fields_leave_it() {
        let mut config = cfg("gemini-3-flash");
        config
            .extra_body
            .insert("labels".to_string(), serde_json::json!({"k": "v"}));
        config.drop_params = vec!["sessionId".to_string()];
        let body = payload(&config, &[], &[ChatMessage::user("hi")]);
        assert_eq!(body["request"]["labels"], serde_json::json!({"k": "v"}));
        assert!(body["request"].get("sessionId").is_none());
    }

    #[test]
    fn the_runtime_model_appends_only_an_effort_level() {
        assert_eq!(
            runtime_model(
                "gemini-3.8-flash",
                Some(ThinkingMode::Effort(ReasoningEffort::Low))
            ),
            "gemini-3.8-flash-low"
        );
        assert_eq!(
            runtime_model("gemini-3.8-flash", Some(ThinkingMode::On)),
            "gemini-3.8-flash"
        );
        assert_eq!(runtime_model("gemini-3.8-flash", None), "gemini-3.8-flash");
        assert_eq!(
            runtime_model(
                "gpt-oss-120b-medium",
                Some(ThinkingMode::Effort(ReasoningEffort::High))
            ),
            "gpt-oss-120b-medium",
            "an id that already names a level is never given a second one"
        );
    }
}
