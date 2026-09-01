//! The `/v1/models` listing: the pure response parse ([`parse_models`], the
//! [`ModelEntry`] row the `/model` picker shows) and the boundary
//! [`fetch_models`] that GETs it. See `docs/llm.md`.

use std::io::Read;

use serde::Deserialize;

use super::config::ModelConfig;
use super::reasoning::{ReasoningEffort, ReasoningSupport};
use super::{LlmError, Result};
use crate::stream::CancelToken;

/// One selectable model row in the `/model` picker. Plain data (no HTTP), so the
/// pure `app`/`ui` picker code consumes it freely (like `session::SessionSummary`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelEntry {
    /// The model id sent to the API (e.g. `anthropic/claude-3.5-haiku`).
    pub id: String,
    /// The provider it came from (e.g. `openrouter`) — shown as a `[tag]`.
    pub provider: String,
    /// The friendly name (the response's `name` field, else the id) — shown in
    /// the picker's `Model Name:` line.
    pub display_name: String,
    /// The model's reasoning ("thinking") capability, when the record
    /// advertises one — drives the Ctrl+T mode cycle. `None` for a model
    /// with no reasoning. See `docs/reasoning.md`.
    pub reasoning: Option<ReasoningSupport>,
    /// Whether the model accepts **image input** (vision), when the record
    /// says either way — `Some(false)` makes the backend degrade image
    /// attachments gracefully instead of letting the provider fail the turn
    /// (OpenRouter 404s an image part sent to a text-only model). `None` =
    /// the record doesn't say (a bare OpenAI-style list) — the backend then
    /// attaches optimistically, exactly as before. See `docs/tools.md`.
    pub vision: Option<bool>,
    /// The model's context window in tokens, when the record reports one —
    /// drives the footer's `{used}/{window} ({pct}%)` gauge and the auto-compact
    /// trigger. `None` = unknown (a bare OpenAI-style list; the gauge hides
    /// and auto-compact stays off). See `docs/compact.md`.
    pub context: Option<u64>,
}

/// The OpenAI `/models` envelope: `{ "data": [ { "id", "name"? } ] }`. Each
/// record stays an **unparsed** [`RawValue`] slice borrowed from the body, so
/// the envelope parse costs one fat pointer per record: materializing every
/// record as a [`serde_json::Value`] at once cost ~10x the body's size in
/// small heap blocks the allocator then kept — +6.6 MB RSS per `/model` open
/// against a real 668 KB OpenRouter list, where the rows the picker keeps
/// ([`ModelEntry`]) total ~47 KB (`examples/mem_probe.rs`). [`parse_models`]
/// builds one record's tree at a time and drops it before the next, so the
/// peak is a single record — and one malformed aggregator entry (a
/// missing/null/non-string id, a non-object row) is still skipped per-record
/// instead of failing the whole list into a Decode error.
///
/// [`RawValue`]: serde_json::value::RawValue
#[derive(Debug, Deserialize)]
struct ModelsResponse<'a> {
    #[serde(default, borrow)]
    data: Vec<&'a serde_json::value::RawValue>,
    /// The ChatGPT backend names the same array `models` and its records
    /// `slug` (`docs/chatgpt.md`). One extra field costs nothing and keeps a
    /// second parse function from existing.
    #[serde(default, borrow)]
    models: Vec<&'a serde_json::value::RawValue>,
}

/// Parse a `/models` response body into rows tagged with `provider`, sorted by
/// id (the picker lists alphabetically, matching the mock). Records without a
/// usable string id are skipped; `name` (OpenRouter and some aggregators send
/// one, OpenAI does not) falls back to the id.
///
/// # Errors
/// Returns a decode error when the body isn't the expected envelope.
pub fn parse_models(body: &str, provider: &str) -> Result<Vec<ModelEntry>> {
    let parsed: ModelsResponse =
        serde_json::from_str(body).map_err(|e| LlmError::Decode(e.to_string()))?;
    let records = if parsed.data.is_empty() {
        &parsed.models
    } else {
        &parsed.data
    };
    let mut out: Vec<ModelEntry> = records
        .iter()
        .filter_map(|raw| {
            // One record's tree at a time (see ModelsResponse); the syntax was
            // validated by the envelope parse, so the re-parse can only fail
            // on a shape the per-record skip covers anyway.
            let record: serde_json::Value = serde_json::from_str(raw.get()).ok()?;
            entry_of(&record, provider)
        })
        .collect();
    out.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(out)
}

/// One `/models` record's [`ModelEntry`], or `None` for a record without a
/// usable string id (skipped, never fatal — aggregator lists carry the odd
/// malformed row).
fn entry_of(record: &serde_json::Value, provider: &str) -> Option<ModelEntry> {
    let id = record
        .get("id")
        .or_else(|| record.get("slug"))
        .and_then(serde_json::Value::as_str)?;
    if id.is_empty() {
        return None;
    }
    if !is_callable(record) {
        return None;
    }
    let display_name = record
        .get("name")
        .or_else(|| record.get("display_name"))
        .and_then(serde_json::Value::as_str)
        .filter(|n| !n.trim().is_empty())
        .unwrap_or(id);
    Some(ModelEntry {
        id: id.to_string(),
        provider: provider.to_string(),
        display_name: display_name.to_string(),
        reasoning: reasoning_support_of(record),
        vision: vision_support_of(record),
        context: context_window_of(record),
    })
}

/// One record's `capabilities` object, when it is GitHub Copilot's — the
/// nested `{family, type, tokenizer, limits, supports}` shape none of the
/// other providers send. Every Copilot-specific sniff hangs off this, so a
/// bare OpenAI-style list is untouched by all of them.
fn copilot_capabilities(record: &serde_json::Value) -> Option<&serde_json::Value> {
    record.get("capabilities").filter(|c| c.is_object())
}

/// One record's `capabilities` object, when it is **Anthropic's** — the
/// `{image_input, thinking, effort, …}` tree of `{supported: bool}` leaves
/// that only the Messages API's `/v1/models` sends. Told apart from Copilot's
/// same-named object by a key only this one has, so a Copilot record never
/// reaches the sniffs below (and vice versa). See `docs/claude.md`.
fn anthropic_capabilities(record: &serde_json::Value) -> Option<&serde_json::Value> {
    let caps = record.get("capabilities").filter(|c| c.is_object())?;
    (caps.get("image_input").is_some() || caps.get("thinking").is_some()).then_some(caps)
}

/// Is one `{ "supported": bool }` leaf on?
fn supported(node: Option<&serde_json::Value>) -> bool {
    node.and_then(|n| n.get("supported"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

/// An Anthropic record's reasoning capability.
///
/// The `effort` sub-object **is the Ctrl+T ladder** — a per-model list of
/// which rungs the model accepts, the third provider to name one natively
/// (`docs/reasoning.md`). Where it says nothing but `thinking` is supported,
/// the model takes the older token-budget form instead, which has no levels:
/// that is an on/off-only reasoner, and [`ThinkingMode::On`] is what drives
/// it (`super::anthropic::thinking_body` reads the same distinction from the
/// other end).
///
/// `None` when the record advertises no thinking at all.
///
/// [`ThinkingMode`]: super::reasoning::ThinkingMode
fn anthropic_reasoning(caps: &serde_json::Value) -> Option<ReasoningSupport> {
    let thinking = caps.get("thinking")?;
    if !supported(Some(thinking)) {
        return None;
    }
    let effort = caps.get("effort").filter(|e| supported(Some(e)));
    let efforts: Vec<ReasoningEffort> = effort.map_or_else(Vec::new, |effort| {
        ReasoningEffort::LADDER
            .iter()
            .copied()
            .filter(|level| supported(effort.get(level.as_str())))
            .collect()
    });
    Some(ReasoningSupport {
        efforts,
        // Most Anthropic models can be asked to stop thinking, but the ones
        // whose thinking is always on answer `{"type": "disabled"}` with a
        // 400 — so where the record names the leaf, it decides. Absent, the
        // permissive answer stands: an explicit disable on a model that takes
        // one, or (on the budgeted family) simply no `thinking` field.
        can_disable: thinking
            .get("types")
            .and_then(|types| types.get("disabled"))
            .is_none_or(|leaf| supported(Some(leaf))),
        default_effort: None,
    })
}

/// Can this client actually call the model? Copilot's list carries records
/// this one can't use, and offering them puts models in the picker that fail
/// every turn:
///
/// - **embeddings / completion** models (`capabilities.type`), which have no
///   chat surface at all;
/// - models served **only** from `/responses` (`supported_endpoints`) — most
///   of the current GPT-5 reasoning family — which answer a
///   `/chat/completions` request with a 400.
///
/// An absent endpoint list means chat completions (the legacy GPT records
/// omit it), and a record with no `capabilities` object at all isn't
/// Copilot's — every other provider's list passes untouched.
fn is_callable(record: &serde_json::Value) -> bool {
    // The ChatGPT backend marks a record it does not want offered at all
    // (`docs/chatgpt.md`). `hide` only means "not a headline model" — it is
    // still selectable, so only `none` is filtered.
    if record.get("visibility").and_then(serde_json::Value::as_str) == Some("none") {
        return false;
    }
    let Some(capabilities) = copilot_capabilities(record) else {
        return true;
    };
    if let Some(kind) = capabilities.get("type").and_then(serde_json::Value::as_str)
        && kind != "chat"
    {
        return false;
    }
    match record
        .get("supported_endpoints")
        .and_then(serde_json::Value::as_array)
    {
        Some(list) => list.iter().any(|e| e.as_str() == Some("/chat/completions")),
        None => true,
    }
}

/// The effort ladder offered when a provider says "reasoning-capable" without
/// enumerating levels (OpenRouter's null `supported_efforts` means "all
/// accepted"; Venice's `supportsReasoningEffort` names none) — the three
/// levels every effort-taking provider accepts.
const DEFAULT_EFFORTS: [ReasoningEffort; 3] = [
    ReasoningEffort::Low,
    ReasoningEffort::Medium,
    ReasoningEffort::High,
];

/// Read one `/models` record's reasoning capability, sniffing both provider
/// shapes (per record, so a custom OpenAI-compatible provider using either
/// works unconfigured):
///
/// - **OpenRouter**: a `reasoning` object — `supported_efforts` (canonically
///   re-ordered; an offered `"none"` marks the off switch, not a rung; null
///   means "all accepted" → the default ladder), `mandatory` (no Off), and
///   `default_effort`. Absent that object, `"reasoning"`/`"reasoning_effort"`
///   in `supported_parameters` still marks support (default ladder).
/// - **Venice**: `model_spec.capabilities.supportsReasoning` (+
///   `supportsReasoningEffort` for the ladder; without it the model is an
///   on/off-only reasoner — efforts empty).
///
/// `None` when the record advertises no reasoning at all.
fn reasoning_support_of(record: &serde_json::Value) -> Option<ReasoningSupport> {
    if let Some(caps) = anthropic_capabilities(record) {
        return anthropic_reasoning(caps);
    }
    // The OpenRouter `reasoning` object.
    if let Some(reasoning) = record.get("reasoning").filter(|r| r.is_object()) {
        let mandatory = reasoning
            .get("mandatory")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        let (efforts, offers_none) = match reasoning
            .get("supported_efforts")
            .and_then(serde_json::Value::as_array)
        {
            Some(list) => {
                let labels: Vec<&str> = list.iter().filter_map(|v| v.as_str()).collect();
                let mut efforts: Vec<ReasoningEffort> = labels
                    .iter()
                    .filter_map(|l| ReasoningEffort::parse(l))
                    .collect();
                efforts.sort_by_key(|e| ReasoningEffort::LADDER.iter().position(|l| l == e));
                efforts.dedup();
                let offers_none = labels.iter().any(|l| l.eq_ignore_ascii_case("none"));
                (efforts, offers_none)
            }
            None => (DEFAULT_EFFORTS.to_vec(), false),
        };
        let default_effort = reasoning
            .get("default_effort")
            .and_then(serde_json::Value::as_str)
            .and_then(ReasoningEffort::parse);
        return Some(ReasoningSupport {
            efforts,
            can_disable: !mandatory || offers_none,
            default_effort,
        });
    }
    // OpenRouter's supported_parameters, when no reasoning object was sent.
    if let Some(params) = record
        .get("supported_parameters")
        .and_then(serde_json::Value::as_array)
    {
        let has = |name: &str| params.iter().any(|p| p.as_str() == Some(name));
        if has("reasoning") || has("reasoning_effort") {
            return Some(ReasoningSupport {
                efforts: DEFAULT_EFFORTS.to_vec(),
                can_disable: true,
                default_effort: None,
            });
        }
    }
    // GitHub Copilot's `capabilities.supports`. Uniquely among the providers
    // it publishes the **exact** effort levels the model accepts, so the
    // Ctrl+T cycle offers what the API will take instead of the default
    // ladder — and never sends a level this model would reject.
    if let Some(supports) = copilot_capabilities(record)
        .and_then(|c| c.get("supports"))
        .filter(|s| s.is_object())
    {
        let flag = |name: &str| {
            supports
                .get(name)
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false)
        };
        let ladder = supports
            .get("reasoning_effort")
            .and_then(serde_json::Value::as_array);
        let budgeted = supports.get("max_thinking_budget").is_some() || flag("adaptive_thinking");
        let Some(levels) = ladder else {
            // No ladder: an Anthropic-shaped model that takes a thinking
            // *budget* instead. It still reasons, so Ctrl+T must offer On.
            return budgeted.then(|| ReasoningSupport {
                efforts: Vec::new(),
                can_disable: true,
                default_effort: None,
            });
        };
        let mut efforts: Vec<ReasoningEffort> = levels
            .iter()
            .filter_map(|l| l.as_str())
            .filter_map(ReasoningEffort::parse)
            .collect();
        efforts.sort_by_key(|e| ReasoningEffort::LADDER.iter().position(|l| l == e));
        efforts.dedup();
        if efforts.is_empty() && !budgeted {
            return None;
        }
        return Some(ReasoningSupport {
            // Reasoning is opt-in on Copilot — omitting the parameter is
            // always allowed, so Off is always a rung.
            can_disable: true,
            efforts,
            default_effort: None,
        });
    }
    // The ChatGPT backend's `supported_reasoning_levels` — like Copilot it
    // publishes the **exact** rungs the model takes, so Ctrl+T offers what the
    // API will accept. Entries are objects (`{effort, description}`) on the
    // live endpoint and bare strings in some fixtures; both are read.
    if let Some(levels) = record
        .get("supported_reasoning_levels")
        .and_then(serde_json::Value::as_array)
    {
        let labels = |value: &serde_json::Value| -> Option<String> {
            value
                .as_str()
                .or_else(|| value.get("effort")?.as_str())
                .map(str::to_string)
        };
        let named: Vec<String> = levels.iter().filter_map(labels).collect();
        let mut efforts: Vec<ReasoningEffort> = named
            .iter()
            .filter_map(|l| ReasoningEffort::parse(l))
            .collect();
        efforts.sort_by_key(|e| ReasoningEffort::LADDER.iter().position(|l| l == e));
        efforts.dedup();
        if efforts.is_empty() {
            return None;
        }
        return Some(ReasoningSupport {
            // An offered `"none"` is the off switch, not a rung — and
            // omitting the parameter is always allowed here besides.
            can_disable: true,
            efforts,
            default_effort: record
                .get("default_reasoning_level")
                .and_then(serde_json::Value::as_str)
                .and_then(ReasoningEffort::parse),
        });
    }
    // Venice's model_spec.capabilities booleans.
    let capabilities = record.get("model_spec")?.get("capabilities")?;
    let flag = |name: &str| {
        capabilities
            .get(name)
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
    };
    if !flag("supportsReasoning") {
        return None;
    }
    Some(ReasoningSupport {
        efforts: if flag("supportsReasoningEffort") {
            DEFAULT_EFFORTS.to_vec()
        } else {
            Vec::new()
        },
        can_disable: true,
        default_effort: None,
    })
}

/// Read one `/models` record's **image-input** (vision) capability, sniffing
/// both provider shapes like [`reasoning_support_of`]:
///
/// - **OpenRouter**: `architecture.input_modalities` — an array; `"image"` in
///   it means vision. Absent that, the older combined `architecture.modality`
///   string (`"text+image->text"`) decides by its **input** side only — an
///   image *generator* (`"text->image"`) is not image input.
/// - **Venice**: `model_spec.capabilities.supportsVision`.
///
/// `None` when the record says nothing either way (a bare OpenAI-style list) —
/// the backend then attaches images optimistically, exactly as before.
fn vision_support_of(record: &serde_json::Value) -> Option<bool> {
    // Anthropic's own `capabilities.image_input.supported`. Every leaf here
    // is present with an explicit `true`/`false`, so absence of the key is
    // "no vision" rather than "unknown".
    if let Some(caps) = anthropic_capabilities(record) {
        return Some(supported(caps.get("image_input")));
    }
    if let Some(arch) = record.get("architecture").filter(|a| a.is_object()) {
        if let Some(inputs) = arch
            .get("input_modalities")
            .and_then(serde_json::Value::as_array)
        {
            return Some(inputs.iter().any(|m| m.as_str() == Some("image")));
        }
        if let Some(modality) = arch.get("modality").and_then(serde_json::Value::as_str) {
            let input_side = modality.split("->").next().unwrap_or("");
            return Some(
                input_side
                    .split('+')
                    .any(|token| token.trim().eq_ignore_ascii_case("image")),
            );
        }
    }
    // GitHub Copilot's `capabilities.supports.vision`. It never sends a
    // `false` — an unsupported capability is simply absent — so once the
    // object exists, absence *is* the answer rather than "unknown".
    if let Some(supports) = copilot_capabilities(record)
        .and_then(|c| c.get("supports"))
        .filter(|s| s.is_object())
    {
        return Some(
            supports
                .get("vision")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
        );
    }
    // The ChatGPT backend's `input_modalities` — a bare top-level array,
    // where OpenRouter nests the same idea under `architecture`.
    if let Some(inputs) = record
        .get("input_modalities")
        .and_then(serde_json::Value::as_array)
    {
        return Some(inputs.iter().any(|m| m.as_str() == Some("image")));
    }
    record
        .get("model_spec")?
        .get("capabilities")?
        .get("supportsVision")?
        .as_bool()
}

/// Read one `/models` record's context-window size, sniffing both provider
/// shapes (per record, like the vision sniff):
///
/// - **OpenRouter** (and most aggregators): a top-level `context_length`.
/// - **Venice**: `model_spec.availableContextTokens`.
/// - **GitHub Copilot**: `capabilities.limits.max_prompt_tokens`, falling back
///   to `max_context_window_tokens` (`docs/copilot.md`).
///
/// `None` when the record doesn't say, or reports a non-positive size (a
/// meaningless window would divide the gauge by zero). See `docs/compact.md`.
fn context_window_of(record: &serde_json::Value) -> Option<u64> {
    let raw = record
        .get("context_length")
        .and_then(serde_json::Value::as_u64)
        .or_else(|| {
            // Anthropic names the window `max_input_tokens` — there is no
            // `context_length` and no `context_window` on this list at all,
            // and `max_tokens` beside it is the *output* cap, so reading the
            // wrong one gauges a 1M window against 128K.
            record
                .get("max_input_tokens")
                .and_then(serde_json::Value::as_u64)
        })
        .or_else(|| {
            record
                .get("model_spec")?
                .get("availableContextTokens")?
                .as_u64()
        })
        .or_else(|| {
            // The ChatGPT backend's own window, scaled by the share of it the
            // backend will actually accept a prompt in
            // (`effective_context_window_percent`, 95 by default). Same rule
            // as Copilot's `max_prompt_tokens` below: gauge against the limit
            // the API *enforces*, not the nominal one.
            let window = record
                .get("context_window")
                .and_then(serde_json::Value::as_u64)
                .or_else(|| {
                    record
                        .get("max_context_window")
                        .and_then(serde_json::Value::as_u64)
                })?;
            let percent = record
                .get("effective_context_window_percent")
                .and_then(serde_json::Value::as_u64)
                .filter(|p| (1..=100).contains(p))
                .unwrap_or(DEFAULT_EFFECTIVE_CONTEXT_PERCENT);
            Some(window.saturating_mul(percent) / 100)
        })
        .or_else(|| {
            // GitHub Copilot's `capabilities.limits`. Its **prompt** cap is
            // what to gauge against, not the nominal window: Copilot sets the
            // two apart (gpt-4o windows 128000 but accepts a 63997-token
            // prompt), and the gauge exists to keep a turn inside the limit
            // the API enforces.
            let limits = copilot_capabilities(record)?.get("limits")?;
            limits
                .get("max_prompt_tokens")
                .and_then(serde_json::Value::as_u64)
                .or_else(|| {
                    limits
                        .get("max_context_window_tokens")
                        .and_then(serde_json::Value::as_u64)
                })
        })?;
    (raw > 0).then_some(raw)
}

/// The share of a ChatGPT model's nominal window the backend actually accepts
/// a prompt in, when the record doesn't say. Codex's own default — and the
/// live listing does *not* send the field, so this is the value in force.
const DEFAULT_EFFECTIVE_CONTEXT_PERCENT: u64 = 95;

/// A parsed catalog, or the reason an **empty** one is a failure rather than
/// a fact.
///
/// For an ordinary provider an empty list is just an empty list. For a
/// signed-in ChatGPT account it never is: the request authenticated, so
/// listing nothing means either the plan reaches no models or the backend's
/// per-record client-version gate has moved past what
/// [`chatgpt::CLIENT_VERSION`] claims. Both are actionable, and both used to
/// arrive as a provider that simply contributed no rows to the `/model`
/// picker — a silence that reads like the fetch never happened.
///
/// [`chatgpt::CLIENT_VERSION`]: super::chatgpt::CLIENT_VERSION
///
/// # Errors
/// An empty ChatGPT catalog becomes an [`LlmError`] naming both causes.
fn catalog_or_error(models: Vec<ModelEntry>, auth: super::AuthScheme) -> Result<Vec<ModelEntry>> {
    if models.is_empty() && auth == super::AuthScheme::OpenAiChatGpt {
        return Err(LlmError::Api {
            status: 200,
            body: "the account listed no models — its ChatGPT plan may not include Codex, \
                   or this client is too old for the models it serves"
                .to_string(),
        });
    }
    Ok(models)
}

/// The `/models` endpoint for a config: `{api_model_base}/models`.
///
/// The ChatGPT backend additionally **requires** a `client_version` query
/// parameter; without it the listing is refused rather than defaulted
/// (`docs/chatgpt.md`).
#[must_use]
pub fn models_endpoint(cfg: &ModelConfig) -> String {
    models_url(&cfg.api_model_base, cfg.auth, cfg.wire_api)
}

/// The `/models` URL for a base, auth scheme and wire format — the one place
/// each listing's required query parameter is added.
///
/// The two that need one need it for opposite reasons, which is why they read
/// different fields: the ChatGPT backend's `client_version` is a property of
/// the **credential's** backend, while Anthropic's page size is a property of
/// the **listing**, and its pasted-key provider is an ordinary
/// [`AuthScheme::ApiKey`](super::AuthScheme::ApiKey).
#[must_use]
fn models_url(base: &str, auth: super::AuthScheme, wire: super::WireApi) -> String {
    let base = base.trim_end_matches('/');
    if wire == super::WireApi::Anthropic {
        return format!("{base}/models?limit={ANTHROPIC_PAGE_SIZE}");
    }
    // Ollama's native listing — its `/v1/models` names the models and
    // nothing else, where `/api/tags` (and each `/api/show`) carries the
    // window and the capabilities (`docs/ollama.md`).
    if wire == super::WireApi::Ollama {
        return format!("{base}/api/tags");
    }
    match auth {
        super::AuthScheme::OpenAiChatGpt => {
            // Required, and load-bearing: the backend filters the catalog by
            // it (`chatgpt::CLIENT_VERSION`), answering a version below every
            // record's own gate with an empty list on an HTTP 200.
            format!(
                "{base}/models?client_version={}",
                super::chatgpt::CLIENT_VERSION
            )
        }
        _ => format!("{base}/models"),
    }
}

/// The page size the Anthropic listing asks for. Its default is **20**, which
/// silently truncates the catalog to whichever models happen to sort first —
/// a `/model` picker missing half the models with no error anywhere. 1000 is
/// the endpoint's own maximum and comfortably one page.
const ANTHROPIC_PAGE_SIZE: u32 = 1000;

/// GET the provider's model list (boundary — real HTTP). Polls `cancel` so a
/// closed picker doesn't leave the worker running.
///
/// # Errors
/// HTTP/decoding failures and non-2xx responses become an [`LlmError`].
pub fn fetch_models(cfg: &ModelConfig, cancel: &CancelToken) -> Result<Vec<ModelEntry>> {
    if cancel.is_cancelled() {
        return Err(LlmError::Cancelled);
    }
    // A one-shot GET on the session's **shared** HTTP client — the chat
    // stream's own per-operation stall deadline, on purpose: `http_client`
    // caches one client per timeout, so a models-only deadline paid a second
    // full client stack (blocking-runtime thread, pool, TLS config) for the
    // life of the process (see `openai::NET_OP_TIMEOUT`).
    let client = super::http_client(super::openai::NET_OP_TIMEOUT)?;
    // The same auth seam the chat request uses: a subscription's stored token
    // is exchanged for the bearer the API takes, the account's own host
    // outranks the configured one (`docs/copilot.md`), and the credential's
    // own identity headers ride along (`docs/chatgpt.md`).
    let auth = super::auth::request_auth(cfg)?;
    let url = auth.base.as_ref().map_or_else(
        || models_endpoint(cfg),
        |base| models_url(base, cfg.auth, cfg.wire_api),
    );
    let mut req = client.get(&url).header("accept", "application/json");
    if let Some(key) = &auth.bearer {
        req = req.bearer_auth(key);
    }
    for (k, v) in &cfg.extra_headers {
        req = req.header(k, v);
    }
    for (k, v) in &auth.headers {
        req = req.header(k, v);
    }
    let mut resp = req.send().map_err(|e| LlmError::Http(e.to_string()))?;
    if !resp.status().is_success() {
        let status = resp.status().as_u16();
        let mut body = String::new();
        let _ = (&mut resp)
            .take(MODELS_BODY_MAX_BYTES)
            .read_to_string(&mut body);
        return Err(LlmError::Api { status, body });
    }
    if cancel.is_cancelled() {
        return Err(LlmError::Cancelled);
    }
    let mut body = String::new();
    (&mut resp)
        .take(MODELS_BODY_MAX_BYTES)
        .read_to_string(&mut body)
        .map_err(|e| LlmError::Http(e.to_string()))?;
    if cfg.wire_api == super::WireApi::Ollama {
        let base = auth
            .base
            .clone()
            .unwrap_or_else(|| cfg.api_model_base.clone());
        return ollama_catalog(&client, cfg, &auth, &base, &body, cancel);
    }
    catalog_or_error(parse_models(&body, &cfg.provider_id)?, cfg.auth)
}

/// The Ollama catalog (boundary — real HTTP): the `/api/tags` body already
/// in hand names the models, and one `POST /api/show` per model fills in
/// what the listing may not say — the window, the capabilities, and the
/// Modelfile's own `num_ctx`, which only the show carries and which the
/// window rule must honour (`docs/ollama.md`). A show that fails keeps the
/// tags record as it was rather than dropping the model. Sequential, the
/// cancel polled between calls: a closed picker stops the walk.
///
/// The server's configured default window is read off this process's
/// `OLLAMA_CONTEXT_LENGTH`, a mirror of the server's own; the cloud, which
/// serves every model at its maximum, is told by its host.
fn ollama_catalog(
    client: &reqwest::blocking::Client,
    cfg: &ModelConfig,
    auth: &super::auth::RequestAuth,
    base: &str,
    tags: &str,
    cancel: &CancelToken,
) -> Result<Vec<ModelEntry>> {
    use super::ollama;
    let server_default = std::env::var(ollama::CONTEXT_LENGTH_ENV)
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|&n| n > 0);
    let uncapped = ollama::is_cloud_host(base);
    let show_url = format!("{}/api/show", base.trim_end_matches('/'));
    let mut out = Vec::new();
    for record in ollama::tags_records(tags)? {
        if cancel.is_cancelled() {
            return Err(LlmError::Cancelled);
        }
        let shown = ollama_show(client, cfg, auth, &show_url, &record.name)
            .and_then(|body| ollama::show_record(&record.name, &body))
            .ok();
        let record = match shown {
            // The show is the fuller account, but a listing that already
            // said something is not contradicted by a show that says nothing.
            Some(mut shown) => {
                if shown.capabilities.is_none() {
                    shown.capabilities = record.capabilities;
                }
                if shown.context_length.is_none() {
                    shown.context_length = record.context_length;
                }
                if shown.family.is_empty() {
                    shown.family = record.family;
                }
                shown.remote |= record.remote;
                shown
            }
            None => record,
        };
        if let Some(entry) = ollama::entry_of(&record, &cfg.provider_id, server_default, uncapped) {
            out.push(entry);
        }
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(out)
}

/// One `POST /api/show` for `model`, its body bounded like the listing's.
fn ollama_show(
    client: &reqwest::blocking::Client,
    cfg: &ModelConfig,
    auth: &super::auth::RequestAuth,
    url: &str,
    model: &str,
) -> Result<String> {
    let mut req = client
        .post(url)
        .header("accept", "application/json")
        .json(&serde_json::json!({"model": model}));
    if let Some(key) = &auth.bearer {
        req = req.bearer_auth(key);
    }
    for (k, v) in &cfg.extra_headers {
        req = req.header(k, v);
    }
    for (k, v) in &auth.headers {
        req = req.header(k, v);
    }
    let mut resp = req.send().map_err(|e| LlmError::Http(e.to_string()))?;
    if !resp.status().is_success() {
        let status = resp.status().as_u16();
        let mut body = String::new();
        let _ = (&mut resp)
            .take(MODELS_BODY_MAX_BYTES)
            .read_to_string(&mut body);
        return Err(LlmError::Api { status, body });
    }
    let mut body = String::new();
    (&mut resp)
        .take(MODELS_BODY_MAX_BYTES)
        .read_to_string(&mut body)
        .map_err(|e| LlmError::Http(e.to_string()))?;
    Ok(body)
}

/// The most a `/models` response body may buffer — the `SHELL_OUTPUT_MAX_BYTES`
/// posture: an unbounded `read_to_string` off the network lets one broken (or
/// hostile) endpoint balloon resident memory without limit. Generous — the
/// largest real list (OpenRouter's whole catalog) is under 1 MiB; a body cut
/// at the cap fails the parse as an ordinary decode error.
const MODELS_BODY_MAX_BYTES: u64 = 8 * 1024 * 1024;

#[cfg(test)]
mod tests {
    use super::*;

    // --- Anthropic's `/v1/models` (docs/claude.md) ---

    /// One record in the shape Anthropic's listing actually sends.
    fn anthropic_record(extra: &str) -> String {
        format!(
            r#"{{"data":[{{"id":"claude-opus-5","type":"model",
                "display_name":"Claude Opus 5","created_at":"2026-07-24T00:00:00Z",
                "max_input_tokens":1000000,"max_tokens":128000,
                "capabilities":{{{extra}}}}}]}}"#
        )
    }

    #[test]
    fn an_anthropic_record_reports_its_window_from_max_input_tokens() {
        // There is no `context_length` and no `context_window` on this list,
        // and the `max_tokens` beside it is the *output* cap — reading the
        // wrong one gauges a 1M window against 128K.
        let body = anthropic_record(r#""image_input":{"supported":true}"#);
        let models = parse_models(&body, "anthropic").unwrap();
        assert_eq!(models[0].context, Some(1_000_000));
        assert_eq!(models[0].display_name, "Claude Opus 5");
    }

    #[test]
    fn an_anthropic_record_reports_vision_from_image_input() {
        let seeing = anthropic_record(r#""image_input":{"supported":true}"#);
        assert_eq!(
            parse_models(&seeing, "anthropic").unwrap()[0].vision,
            Some(true)
        );
        // Every leaf here is explicit, so `false` is an answer, not a silence.
        let blind = anthropic_record(r#""image_input":{"supported":false}"#);
        assert_eq!(
            parse_models(&blind, "anthropic").unwrap()[0].vision,
            Some(false)
        );
    }

    #[test]
    fn an_anthropic_effort_object_is_the_ctrl_t_ladder_itself() {
        // The third provider to name the ladder natively (docs/reasoning.md).
        let body = anthropic_record(
            r#""image_input":{"supported":true},
               "thinking":{"supported":true,"types":{"adaptive":{"supported":true}}},
               "effort":{"supported":true,
                 "low":{"supported":true},"medium":{"supported":true},
                 "high":{"supported":true},"xhigh":{"supported":true},
                 "max":{"supported":true}}"#,
        );
        let reasoning = parse_models(&body, "anthropic").unwrap()[0]
            .reasoning
            .clone()
            .expect("a reasoning-capable model");
        assert_eq!(
            reasoning.efforts,
            vec![
                ReasoningEffort::Low,
                ReasoningEffort::Medium,
                ReasoningEffort::High,
                ReasoningEffort::XHigh,
                ReasoningEffort::Max,
            ]
        );
        assert!(reasoning.can_disable);
    }

    #[test]
    fn an_anthropic_effort_object_offers_only_the_rungs_it_names() {
        let body = anthropic_record(
            r#""thinking":{"supported":true},
               "effort":{"supported":true,
                 "low":{"supported":true},"medium":{"supported":true},
                 "high":{"supported":true},
                 "xhigh":{"supported":false},"max":{"supported":false}}"#,
        );
        let reasoning = parse_models(&body, "anthropic").unwrap()[0]
            .reasoning
            .clone()
            .expect("reasoning");
        assert_eq!(
            reasoning.efforts,
            vec![
                ReasoningEffort::Low,
                ReasoningEffort::Medium,
                ReasoningEffort::High
            ]
        );
    }

    #[test]
    fn an_anthropic_model_that_cannot_stop_thinking_is_not_offered_the_off_rung() {
        // `thinking: {"type": "disabled"}` is a 400 on the models whose
        // thinking is always on. Ctrl+T must not be able to reach a mode that
        // fails every request — so where the record says so, `Off` goes away.
        let body = anthropic_record(
            r#""thinking":{"supported":true,
                 "types":{"adaptive":{"supported":true},"disabled":{"supported":false}}},
               "effort":{"supported":true,"high":{"supported":true},"max":{"supported":true}}"#,
        );
        let reasoning = parse_models(&body, "anthropic").unwrap()[0]
            .reasoning
            .clone()
            .expect("reasoning");
        assert!(!reasoning.can_disable);
        assert!(
            !reasoning.modes().contains(&crate::llm::ThinkingMode::Off),
            "{:?}",
            reasoning.modes()
        );
        // A record that says nothing keeps the old, permissive answer.
        let quiet = anthropic_record(r#""thinking":{"supported":true}"#);
        assert!(
            parse_models(&quiet, "anthropic").unwrap()[0]
                .reasoning
                .as_ref()
                .unwrap()
                .can_disable
        );
    }

    #[test]
    fn an_anthropic_model_with_thinking_but_no_efforts_is_an_on_off_reasoner() {
        // The older budgeted family: it thinks, but takes no effort level —
        // which is the mode `anthropic::thinking_body` maps to `budget_tokens`.
        let body = anthropic_record(
            r#""thinking":{"supported":true,"types":{"enabled":{"supported":true}}}"#,
        );
        let reasoning = parse_models(&body, "anthropic").unwrap()[0]
            .reasoning
            .clone()
            .expect("reasoning");
        assert!(reasoning.efforts.is_empty());
        assert_eq!(reasoning.modes().len(), 2, "just Off and On");
    }

    #[test]
    fn an_anthropic_model_that_says_it_cannot_think_offers_no_ctrl_t() {
        let body = anthropic_record(r#""thinking":{"supported":false}"#);
        assert_eq!(parse_models(&body, "anthropic").unwrap()[0].reasoning, None);
        let silent = anthropic_record(r#""image_input":{"supported":true}"#);
        assert_eq!(
            parse_models(&silent, "anthropic").unwrap()[0].reasoning,
            None
        );
    }

    #[test]
    fn a_copilot_capabilities_object_never_reaches_the_anthropic_sniffs() {
        // Both providers send a `capabilities` object and they are nothing
        // alike; telling them apart by a key only one has is what keeps each
        // sniff to its own shape.
        let copilot = r#"{"data":[{"id":"gpt-4o","capabilities":{"type":"chat",
            "limits":{"max_prompt_tokens":63997},"supports":{"vision":true}}}]}"#;
        let models = parse_models(copilot, "github_copilot").unwrap();
        assert_eq!(models[0].context, Some(63_997), "copilot's own rule stands");
        assert_eq!(models[0].vision, Some(true));
    }

    #[test]
    fn the_anthropic_listing_asks_for_a_page_big_enough_to_hold_the_catalog() {
        // Its default page is 20: without an explicit limit the picker
        // silently shows whichever models sort first, with no error anywhere.
        let mut cfg = ModelConfig::fallback();
        cfg.api_model_base = "https://api.anthropic.com/v1".to_string();
        cfg.wire_api = super::super::WireApi::Anthropic;
        // The pasted-key provider is an ordinary ApiKey scheme, so the page
        // size has to key on the wire format rather than on `auth`.
        assert_eq!(
            models_endpoint(&cfg),
            "https://api.anthropic.com/v1/models?limit=1000"
        );
        cfg.auth = super::super::AuthScheme::AnthropicConsole;
        assert_eq!(
            models_endpoint(&cfg),
            "https://api.anthropic.com/v1/models?limit=1000"
        );
        // Every other provider's URL is what it always was.
        let plain = ModelConfig::fallback();
        assert_eq!(models_endpoint(&plain), "https://api.openai.com/v1/models");
    }

    // --- context window (docs/compact.md: the footer gauge + auto-compact) ---

    #[test]
    fn context_length_parses_from_an_openrouter_record() {
        let body = r#"{"data":[{"id":"m","context_length":262144}]}"#;
        let models = parse_models(body, "openrouter").unwrap();
        assert_eq!(models[0].context, Some(262_144));
    }

    #[test]
    fn context_window_sniffs_venice_and_defaults_to_unknown() {
        let venice = r#"{"data":[{"id":"m","model_spec":{"availableContextTokens":131072}}]}"#;
        assert_eq!(
            parse_models(venice, "venice").unwrap()[0].context,
            Some(131_072)
        );
        let bare = r#"{"data":[{"id":"m"}]}"#;
        assert_eq!(parse_models(bare, "openai").unwrap()[0].context, None);
        // A zero/negative length is meaningless — treat as unknown.
        let zero = r#"{"data":[{"id":"m","context_length":0}]}"#;
        assert_eq!(parse_models(zero, "p").unwrap()[0].context, None);
    }

    // --- the ChatGPT backend's own record shape (docs/chatgpt.md) ---

    /// One record of the ChatGPT backend's `/models` listing, in its own
    /// envelope. Nothing about it matches the OpenAI-compatible shape: the
    /// array is `models`, the id is `slug`, and every capability is a
    /// top-level field.
    fn chatgpt_catalog(record: &str) -> Vec<ModelEntry> {
        parse_models(&format!(r#"{{"models":[{record}]}}"#), "openai_chatgpt").unwrap()
    }

    #[test]
    fn a_chatgpt_record_lists_under_its_slug_and_display_name() {
        let models = chatgpt_catalog(
            r#"{"slug":"gpt-5.5","display_name":"GPT-5.5","context_window":272000,
                "input_modalities":["text","image"]}"#,
        );
        assert_eq!(models[0].id, "gpt-5.5");
        assert_eq!(models[0].display_name, "GPT-5.5");
    }

    #[test]
    fn a_chatgpt_records_context_window_is_the_share_the_backend_enforces() {
        // The gauge exists to keep a turn inside the limit the API actually
        // accepts a prompt in — the same rule Copilot's `max_prompt_tokens`
        // gets, since the nominal window is not what is enforced.
        let models = chatgpt_catalog(r#"{"slug":"m","context_window":272000}"#);
        assert_eq!(models[0].context, Some(272_000 * 95 / 100));
        let explicit = chatgpt_catalog(
            r#"{"slug":"m","context_window":200000,"effective_context_window_percent":50}"#,
        );
        assert_eq!(explicit[0].context, Some(100_000));
        // With only a ceiling reported, that is what there is to gauge against.
        let ceiling = chatgpt_catalog(r#"{"slug":"m","max_context_window":100000}"#);
        assert_eq!(ceiling[0].context, Some(95_000));
    }

    #[test]
    fn a_chatgpt_records_input_modalities_decide_vision() {
        let seeing = chatgpt_catalog(r#"{"slug":"m","input_modalities":["text","image"]}"#);
        assert_eq!(seeing[0].vision, Some(true));
        let blind = chatgpt_catalog(r#"{"slug":"m","input_modalities":["text"]}"#);
        assert_eq!(blind[0].vision, Some(false));
    }

    #[test]
    fn a_chatgpt_records_reasoning_levels_are_the_ctrl_t_ladder() {
        // Like Copilot, this backend publishes the exact rungs the model
        // takes — including `ultra`, which no other provider names.
        let models = chatgpt_catalog(
            r#"{"slug":"gpt-5.6-sol","default_reasoning_level":"low",
                "supported_reasoning_levels":[
                    {"effort":"low","description":"…"},{"effort":"medium","description":"…"},
                    {"effort":"high","description":"…"},{"effort":"xhigh","description":"…"},
                    {"effort":"max","description":"…"},{"effort":"ultra","description":"…"}]}"#,
        );
        let support = models[0].reasoning.clone().expect("reasoning-capable");
        assert_eq!(
            support.efforts,
            vec![
                ReasoningEffort::Low,
                ReasoningEffort::Medium,
                ReasoningEffort::High,
                ReasoningEffort::XHigh,
                ReasoningEffort::Max,
                ReasoningEffort::Ultra,
            ]
        );
        assert!(support.can_disable, "omitting the parameter is allowed");
        assert_eq!(support.default_effort, Some(ReasoningEffort::Low));
    }

    #[test]
    fn a_chatgpt_reasoning_ladder_of_bare_strings_reads_the_same() {
        let models = chatgpt_catalog(r#"{"slug":"m","supported_reasoning_levels":["high","low"]}"#);
        let support = models[0].reasoning.clone().unwrap();
        // Canonically re-ordered, like every other provider's unordered list.
        assert_eq!(
            support.efforts,
            vec![ReasoningEffort::Low, ReasoningEffort::High]
        );
    }

    #[test]
    fn a_chatgpt_record_marked_invisible_is_not_offered() {
        // `hide` only means "not a headline model" — still selectable. Only
        // `none` is withheld.
        assert!(chatgpt_catalog(r#"{"slug":"m","visibility":"none"}"#).is_empty());
        assert_eq!(
            chatgpt_catalog(r#"{"slug":"m","visibility":"hide"}"#).len(),
            1
        );
        assert_eq!(
            chatgpt_catalog(r#"{"slug":"m","visibility":"list"}"#).len(),
            1
        );
    }

    #[test]
    fn the_chatgpt_models_url_sends_a_client_version_past_the_catalog_gate() {
        // The reported bug: this query is not decoration. Every record the
        // backend holds carries a `minimal_client_version` (0.98.0 … 0.144.0
        // when measured), and it serves only the records at or below what the
        // caller claims — so the crate's own `0.1.0` came back
        // `{"models":[]}` with an HTTP 200 and no hint why. It must be the
        // Codex client version this client presents as, never
        // `CARGO_PKG_VERSION`, which means nothing to OpenAI.
        let mut cfg = ModelConfig::fallback();
        cfg.api_model_base = "https://chatgpt.com/backend-api/codex".to_string();
        cfg.auth = super::super::AuthScheme::OpenAiChatGpt;
        let url = models_endpoint(&cfg);
        assert!(
            url.ends_with(&format!(
                "/models?client_version={}",
                super::super::chatgpt::CLIENT_VERSION
            )),
            "{url}"
        );
        assert!(
            !url.contains(env!("CARGO_PKG_VERSION")),
            "the crate version is not a Codex client version: {url}"
        );
    }

    #[test]
    fn an_empty_chatgpt_catalog_is_an_error_rather_than_a_silent_blank_picker() {
        // A signed-in account that lists nothing is a *failure* — a plan with
        // no access, or a gate raised past the version we send — and the
        // picker showing an empty provider says none of that. Every other
        // provider's empty list stays an ordinary empty list.
        let chatgpt = super::super::AuthScheme::OpenAiChatGpt;
        let err = catalog_or_error(Vec::new(), chatgpt).expect_err("empty is an error here");
        let shown = err.to_string();
        assert!(shown.contains("no models"), "{shown}");
        assert!(shown.contains("plan"), "names the likeliest cause: {shown}");
        // A non-empty one passes straight through.
        let one = vec![ModelEntry {
            id: "gpt-5.5".to_string(),
            provider: "openai_chatgpt".to_string(),
            display_name: "GPT-5.5".to_string(),
            reasoning: None,
            vision: None,
            context: None,
        }];
        assert_eq!(catalog_or_error(one.clone(), chatgpt).unwrap(), one);
        // And an ordinary provider's empty list is not an error at all.
        assert!(
            catalog_or_error(Vec::new(), super::super::AuthScheme::ApiKey)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn the_chatgpt_models_url_carries_the_client_version_it_requires() {
        // Without it the listing is refused rather than defaulted.
        let mut cfg = ModelConfig::fallback();
        cfg.api_model_base = "https://chatgpt.com/backend-api/codex".to_string();
        cfg.auth = super::super::AuthScheme::OpenAiChatGpt;
        let url = models_endpoint(&cfg);
        assert!(url.starts_with("https://chatgpt.com/backend-api/codex/models?client_version="));
        // Every other provider's URL is untouched.
        cfg.auth = super::super::AuthScheme::ApiKey;
        assert_eq!(
            models_endpoint(&cfg),
            "https://chatgpt.com/backend-api/codex/models"
        );
    }

    // --- GitHub Copilot's own record shape (docs/copilot.md) ---

    /// One real `GET https://api.githubcopilot.com/models` record, trimmed to
    /// the keys this parse reads — the shape three independent captures agree
    /// on.
    fn copilot_body(extra_supports: &str, extra_top: &str) -> String {
        format!(
            r#"{{"data":[{{"id":"claude-sonnet-4.6","name":"Claude Sonnet 4.6",
              "vendor":"Anthropic","model_picker_enabled":true{extra_top},
              "capabilities":{{"family":"claude-sonnet-4.6","type":"chat",
                "tokenizer":"o200k_base",
                "limits":{{"max_context_window_tokens":1000000,
                  "max_prompt_tokens":936000,"max_output_tokens":64000,
                  "vision":{{"max_prompt_images":5}}}},
                "supports":{{"streaming":true,"tool_calls":true{extra_supports}}}}}}}]}}"#
        )
    }

    #[test]
    fn a_copilot_record_reports_its_prompt_budget_as_the_context_window() {
        // Copilot caps the *prompt* well below the nominal window (gpt-4o:
        // 128000 vs 63997), and the prompt cap is what the footer gauge and
        // auto-compaction must respect — a gauge against the window would let
        // a turn sail past the limit the API actually enforces.
        let models = parse_models(&copilot_body("", ""), "github_copilot").unwrap();
        assert_eq!(models[0].context, Some(936_000));
        assert_eq!(models[0].display_name, "Claude Sonnet 4.6");
    }

    #[test]
    fn a_copilot_record_without_a_prompt_cap_falls_back_to_the_window() {
        let body = r#"{"data":[{"id":"m","capabilities":{"type":"chat",
            "limits":{"max_context_window_tokens":128000}}}]}"#;
        assert_eq!(
            parse_models(body, "github_copilot").unwrap()[0].context,
            Some(128_000)
        );
    }

    #[test]
    fn copilot_vision_is_read_off_the_supports_object() {
        // Copilot never sends `false` — an unsupported capability is simply
        // absent — so absence must read as "no", not "unknown".
        let seeing = parse_models(&copilot_body(r#","vision":true"#, ""), "p").unwrap();
        assert_eq!(seeing[0].vision, Some(true));
        let blind = parse_models(&copilot_body("", ""), "p").unwrap();
        assert_eq!(blind[0].vision, Some(false));
    }

    #[test]
    fn a_copilot_effort_array_is_the_thinking_ladder_itself() {
        // Unlike every other provider, Copilot publishes the exact levels the
        // model accepts — so Ctrl+T cycles what the API will take rather than
        // a hardcoded guess, and a level outside the list is never sent.
        let body = copilot_body(
            r#","reasoning_effort":["minimal","low","medium","high"]"#,
            "",
        );
        let support = parse_models(&body, "p").unwrap()[0]
            .reasoning
            .clone()
            .expect("a reasoning model");
        assert_eq!(
            support.efforts,
            vec![
                ReasoningEffort::Minimal,
                ReasoningEffort::Low,
                ReasoningEffort::Medium,
                ReasoningEffort::High
            ]
        );
        assert!(support.can_disable, "reasoning is opt-in on Copilot");
    }

    #[test]
    fn a_copilot_effort_array_is_ordered_canonically() {
        let body = copilot_body(r#","reasoning_effort":["high","none","low"]"#, "");
        let support = parse_models(&body, "p").unwrap()[0]
            .reasoning
            .clone()
            .unwrap();
        assert_eq!(
            support.efforts,
            vec![ReasoningEffort::Low, ReasoningEffort::High],
            "sorted, and the `none` rung is the off switch rather than a level"
        );
    }

    #[test]
    fn a_copilot_thinking_budget_marks_an_on_off_reasoner() {
        // The Anthropic-shaped models advertise a budget instead of a ladder;
        // they still reason, so Ctrl+T must offer On.
        let budget = copilot_body(r#","max_thinking_budget":32000"#, "");
        let support = parse_models(&budget, "p").unwrap()[0]
            .reasoning
            .clone()
            .unwrap();
        assert!(support.efforts.is_empty(), "no ladder — on/off only");
        let adaptive = copilot_body(r#","adaptive_thinking":true"#, "");
        assert!(parse_models(&adaptive, "p").unwrap()[0].reasoning.is_some());
    }

    #[test]
    fn a_copilot_record_with_no_reasoning_at_all_reports_none() {
        assert!(
            parse_models(&copilot_body("", ""), "p").unwrap()[0]
                .reasoning
                .is_none()
        );
    }

    #[test]
    fn copilot_lists_only_what_this_client_can_actually_call() {
        // The list mixes in embeddings and — the trap — reasoning models
        // served *only* from `/responses`. Offering one puts a model in the
        // picker that 400s every turn.
        let body = r#"{"data":[
            {"id":"chat-ok","capabilities":{"type":"chat"},
             "supported_endpoints":["/chat/completions","/v1/messages"]},
            {"id":"responses-only","capabilities":{"type":"chat"},
             "supported_endpoints":["/responses","ws:/responses"]},
            {"id":"embeddings","capabilities":{"type":"embeddings"}},
            {"id":"legacy-chat","capabilities":{"type":"chat"}}
        ]}"#;
        let ids: Vec<String> = parse_models(body, "github_copilot")
            .unwrap()
            .into_iter()
            .map(|m| m.id)
            .collect();
        assert_eq!(
            ids,
            vec!["chat-ok".to_string(), "legacy-chat".to_string()],
            "an absent endpoint list means chat completions, as the legacy models send"
        );
    }

    #[test]
    fn an_ordinary_providers_records_are_untouched_by_the_copilot_filter() {
        // The filter keys off Copilot's own `capabilities` object, so an
        // OpenAI-style bare list — which has neither a type nor an endpoint
        // list — must still list in full.
        let body = r#"{"data":[{"id":"gpt-4o-mini"},{"id":"o3"}]}"#;
        assert_eq!(parse_models(body, "openai").unwrap().len(), 2);
    }

    #[test]
    fn entry_of_reads_one_record_and_skips_an_unusable_one() {
        // The per-record conversion: one record's parsed tree in, one
        // ModelEntry out — `parse_models` materializes records one at a time
        // through this, so the whole-list `Value` tree (megabytes of small
        // heap blocks for a real aggregator body) never exists at once.
        let record: serde_json::Value =
            serde_json::from_str(r#"{"id":"m","name":"M","context_length":8192}"#).unwrap();
        let entry = entry_of(&record, "p").expect("a usable record");
        assert_eq!(entry.id, "m");
        assert_eq!(entry.display_name, "M");
        assert_eq!(entry.provider, "p");
        assert_eq!(entry.context, Some(8192));
        // The per-record skip: a non-object / id-less record is None, never
        // an error (the list must survive one malformed aggregator row).
        assert!(entry_of(&serde_json::Value::from(42), "p").is_none());
        assert!(entry_of(&serde_json::json!({"name": "no id"}), "p").is_none());
    }

    #[test]
    fn envelope_keys_around_data_are_ignored() {
        // OpenAI sends `{"object":"list","data":[…]}` — keys before *and*
        // after `data` must be skipped, whatever the parse's internal shape.
        let body = r#"{"object":"list","data":[{"id":"m"}],"has_more":false}"#;
        let models = parse_models(body, "p").unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "m");
    }

    #[test]
    fn parses_ids_and_names() {
        let body = r#"{"data":[
            {"id":"anthropic/claude-3.5-haiku","name":"Anthropic: Claude 3.5 Haiku"},
            {"id":"moonshotai/kimi-k2.6","name":"MoonshotAI: Kimi K2.6"}
        ]}"#;
        let models = parse_models(body, "openrouter").unwrap();
        assert_eq!(models.len(), 2);
        // Sorted by id: anthropic before moonshotai.
        assert_eq!(models[0].id, "anthropic/claude-3.5-haiku");
        assert_eq!(models[0].display_name, "Anthropic: Claude 3.5 Haiku");
        assert_eq!(models[0].provider, "openrouter");
        assert_eq!(models[1].id, "moonshotai/kimi-k2.6");
    }

    #[test]
    fn missing_name_falls_back_to_id() {
        let body = r#"{"data":[{"id":"gpt-4o-mini"}]}"#;
        let models = parse_models(body, "openai").unwrap();
        assert_eq!(models[0].display_name, "gpt-4o-mini");
    }

    #[test]
    fn blank_name_falls_back_to_id() {
        let body = r#"{"data":[{"id":"x","name":"   "}]}"#;
        let models = parse_models(body, "p").unwrap();
        assert_eq!(models[0].display_name, "x");
    }

    #[test]
    fn empty_data_is_an_empty_list_not_an_error() {
        let models = parse_models(r#"{"data":[]}"#, "p").unwrap();
        assert!(models.is_empty());
    }

    #[test]
    fn records_without_an_id_are_skipped() {
        let body = r#"{"data":[{"id":""},{"id":"keep"}]}"#;
        let models = parse_models(body, "p").unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "keep");
    }

    #[test]
    fn one_malformed_record_does_not_fail_the_list() {
        // A single aggregator entry with a missing / null / non-string id (or
        // a non-object row) must be skipped — not turn the provider's whole
        // list into a Decode error and mark it unavailable in the picker.
        let body = r#"{"data":[
            {"name":"no id at all"},
            {"id":null},
            {"id":42},
            "not even an object",
            {"id":"keep","name":"Keeper"}
        ]}"#;
        let models = parse_models(body, "p").unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "keep");
        assert_eq!(models[0].display_name, "Keeper");
    }

    #[test]
    fn malformed_body_is_a_decode_error() {
        assert!(parse_models("not json", "p").is_err());
    }

    #[test]
    fn results_are_sorted_alphabetically() {
        let body = r#"{"data":[{"id":"zzz"},{"id":"aaa"},{"id":"mmm"}]}"#;
        let models = parse_models(body, "p").unwrap();
        let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["aaa", "mmm", "zzz"]);
    }

    use crate::llm::reasoning::ReasoningEffort;

    #[test]
    fn openrouter_reasoning_object_parses_efforts_and_default() {
        // The OpenRouter record shape: a `reasoning` object naming the
        // accepted efforts (arbitrary order) and the model's default.
        let body = r#"{"data":[{
            "id":"anthropic/claude-sonnet-5",
            "supported_parameters":["reasoning","tools"],
            "reasoning":{"mandatory":false,"supported_efforts":["max","xhigh","high","medium","low"],"default_effort":"medium"}
        }]}"#;
        let models = parse_models(body, "openrouter").unwrap();
        let support = models[0].reasoning.as_ref().expect("reasoning-capable");
        assert_eq!(
            support.efforts,
            vec![
                ReasoningEffort::Low,
                ReasoningEffort::Medium,
                ReasoningEffort::High,
                ReasoningEffort::XHigh,
                ReasoningEffort::Max,
            ],
            "canonically ordered regardless of the wire order"
        );
        assert!(support.can_disable);
        assert_eq!(support.default_effort, Some(ReasoningEffort::Medium));
    }

    #[test]
    fn openrouter_mandatory_reasoning_cannot_be_disabled() {
        let body = r#"{"data":[{
            "id":"moonshotai/kimi-k3",
            "reasoning":{"mandatory":true,"supported_efforts":["max"]}
        }]}"#;
        let models = parse_models(body, "openrouter").unwrap();
        let support = models[0].reasoning.as_ref().unwrap();
        assert_eq!(support.efforts, vec![ReasoningEffort::Max]);
        assert!(!support.can_disable);
    }

    #[test]
    fn a_none_effort_maps_to_disable_not_a_level() {
        // Some records list "none" among the efforts — that's the off switch,
        // not a ladder rung.
        let body = r#"{"data":[{
            "id":"openai/gpt-5.6",
            "reasoning":{"mandatory":true,"supported_efforts":["high","medium","low","none"]}
        }]}"#;
        let models = parse_models(body, "openrouter").unwrap();
        let support = models[0].reasoning.as_ref().unwrap();
        assert_eq!(
            support.efforts,
            vec![
                ReasoningEffort::Low,
                ReasoningEffort::Medium,
                ReasoningEffort::High
            ]
        );
        assert!(
            support.can_disable,
            "an offered \"none\" overrides the mandatory flag"
        );
    }

    #[test]
    fn a_reasoning_object_without_efforts_gets_the_default_ladder() {
        // OpenRouter documents a null/absent supported_efforts as "all
        // accepted" — offer the standard trio rather than nothing.
        let body = r#"{"data":[{
            "id":"deepseek/deepseek-v3.2",
            "reasoning":{"mandatory":false,"default_enabled":false}
        }]}"#;
        let models = parse_models(body, "openrouter").unwrap();
        let support = models[0].reasoning.as_ref().unwrap();
        assert_eq!(
            support.efforts,
            vec![
                ReasoningEffort::Low,
                ReasoningEffort::Medium,
                ReasoningEffort::High
            ]
        );
        assert!(support.can_disable);
        assert_eq!(support.default_effort, None);
    }

    #[test]
    fn supported_parameters_alone_marks_reasoning_support() {
        // A record with no `reasoning` object but "reasoning" among its
        // supported parameters still gets the default ladder.
        let body = r#"{"data":[
            {"id":"thinker","supported_parameters":["reasoning","max_tokens"]},
            {"id":"plain","supported_parameters":["max_tokens","tools"]}
        ]}"#;
        let models = parse_models(body, "openrouter").unwrap();
        let thinker = models.iter().find(|m| m.id == "thinker").unwrap();
        let support = thinker.reasoning.as_ref().expect("supported");
        assert_eq!(support.efforts.len(), 3);
        assert!(support.can_disable);
        let plain = models.iter().find(|m| m.id == "plain").unwrap();
        assert!(plain.reasoning.is_none());
    }

    #[test]
    fn venice_capabilities_parse_both_reasoning_shapes() {
        // Venice's shape: model_spec.capabilities booleans. With
        // supportsReasoningEffort the standard ladder applies; without it the
        // model is an on/off-only reasoner (efforts empty).
        let body = r#"{"data":[
            {"id":"qwen","model_spec":{"capabilities":{"supportsReasoning":true,"supportsReasoningEffort":true}}},
            {"id":"claude","model_spec":{"capabilities":{"supportsReasoning":true,"supportsReasoningEffort":false}}},
            {"id":"plain","model_spec":{"capabilities":{"supportsReasoning":false,"supportsReasoningEffort":false}}}
        ]}"#;
        let models = parse_models(body, "a0_venice").unwrap();
        let by_id = |id: &str| models.iter().find(|m| m.id == id).unwrap();
        let qwen = by_id("qwen").reasoning.as_ref().expect("effort-capable");
        assert_eq!(
            qwen.efforts,
            vec![
                ReasoningEffort::Low,
                ReasoningEffort::Medium,
                ReasoningEffort::High
            ]
        );
        assert!(qwen.can_disable);
        let claude = by_id("claude").reasoning.as_ref().expect("on/off-only");
        assert!(claude.efforts.is_empty());
        assert!(claude.can_disable);
        assert!(by_id("plain").reasoning.is_none());
    }

    #[test]
    fn unknown_effort_labels_are_skipped_not_fatal() {
        let body = r#"{"data":[{
            "id":"x",
            "reasoning":{"supported_efforts":["medium","turbo-think","high"]}
        }]}"#;
        let models = parse_models(body, "p").unwrap();
        let support = models[0].reasoning.as_ref().unwrap();
        assert_eq!(
            support.efforts,
            vec![ReasoningEffort::Medium, ReasoningEffort::High]
        );
    }

    #[test]
    fn a_record_without_reasoning_hints_has_no_support() {
        let body = r#"{"data":[{"id":"gpt-4o-mini"}]}"#;
        let models = parse_models(body, "openai").unwrap();
        assert!(models[0].reasoning.is_none());
    }

    // ===== vision (image-input) support =====

    #[test]
    fn openrouter_input_modalities_mark_vision_support() {
        // The live OpenRouter shape: every record carries
        // architecture.input_modalities; "image" in it means the model can
        // see attachments, its absence means the provider 404s an image part
        // ("No endpoints found that support image input").
        let body = r#"{"data":[
            {"id":"openai/gpt-4o-mini","architecture":{"modality":"text+image+file->text","input_modalities":["text","image","file"],"output_modalities":["text"]}},
            {"id":"openai/gpt-oss-120b","architecture":{"modality":"text->text","input_modalities":["text"],"output_modalities":["text"]}}
        ]}"#;
        let models = parse_models(body, "openrouter").unwrap();
        let by_id = |id: &str| models.iter().find(|m| m.id == id).unwrap();
        assert_eq!(by_id("openai/gpt-4o-mini").vision, Some(true));
        assert_eq!(by_id("openai/gpt-oss-120b").vision, Some(false));
    }

    #[test]
    fn the_modality_string_alone_marks_vision_by_its_input_side() {
        // An older/other aggregator record with only the combined string: the
        // input side ("text+image") decides — an image *output* (a generator,
        // "text->image") must not read as image input.
        let body = r#"{"data":[
            {"id":"sees","architecture":{"modality":"text+image->text"}},
            {"id":"blind","architecture":{"modality":"text->text"}},
            {"id":"paints","architecture":{"modality":"text->image"}}
        ]}"#;
        let models = parse_models(body, "p").unwrap();
        let by_id = |id: &str| models.iter().find(|m| m.id == id).unwrap();
        assert_eq!(by_id("sees").vision, Some(true));
        assert_eq!(by_id("blind").vision, Some(false));
        assert_eq!(by_id("paints").vision, Some(false));
    }

    #[test]
    fn venice_supports_vision_capability_parses() {
        // The live Venice shape: model_spec.capabilities.supportsVision,
        // beside the supportsReasoning flag already sniffed.
        let body = r#"{"data":[
            {"id":"claude-fable-5","model_spec":{"capabilities":{"supportsVision":true,"supportsReasoning":true}}},
            {"id":"zai-org-glm-5","model_spec":{"capabilities":{"supportsVision":false,"supportsReasoning":false}}}
        ]}"#;
        let models = parse_models(body, "a0_venice").unwrap();
        let by_id = |id: &str| models.iter().find(|m| m.id == id).unwrap();
        assert_eq!(by_id("claude-fable-5").vision, Some(true));
        assert_eq!(by_id("zai-org-glm-5").vision, Some(false));
    }

    #[test]
    fn a_record_without_modality_hints_has_unknown_vision() {
        // A bare OpenAI-style list says nothing either way — unknown, so the
        // backend keeps attaching optimistically (today's behavior).
        let body = r#"{"data":[{"id":"gpt-4o-mini"}]}"#;
        let models = parse_models(body, "openai").unwrap();
        assert_eq!(models[0].vision, None);
    }

    #[test]
    fn models_endpoint_appends_models() {
        let mut cfg = ModelConfig::fallback();
        cfg.api_model_base = "https://openrouter.ai/api/v1".to_string();
        assert_eq!(models_endpoint(&cfg), "https://openrouter.ai/api/v1/models");
    }

    #[test]
    fn models_endpoint_trims_trailing_slash() {
        let mut cfg = ModelConfig::fallback();
        cfg.api_model_base = "https://x/v1/".to_string();
        assert_eq!(models_endpoint(&cfg), "https://x/v1/models");
    }

    // --- lenient fields: one odd field must not cost the whole record ---
    //
    // The parse reads a handful of fields out of records it otherwise ignores,
    // and aggregators are loose about types. A record whose `name`,
    // `context_length` or capability block arrives in an unexpected shape
    // still names a real, selectable model, so it stays in the list with that
    // one field unread — it is never dropped, and it never fails the body.

    #[test]
    fn a_non_string_name_falls_back_to_the_id() {
        let models = parse_models(r#"{"data":[{"id":"m","name":42}]}"#, "p").unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].display_name, "m");
    }

    #[test]
    fn a_non_numeric_context_length_reads_as_unknown() {
        let body = r#"{"data":[{"id":"m","context_length":"262144"}]}"#;
        let models = parse_models(body, "p").unwrap();
        assert_eq!(models.len(), 1, "the model is still listed");
        assert_eq!(models[0].context, None);
    }

    #[test]
    fn a_misshapen_capability_block_reads_as_no_capability() {
        // `reasoning` as a string, `supported_parameters` as a string, and
        // `architecture` as an array — none is the shape the sniffs expect.
        let body = r#"{"data":[
            {"id":"a","reasoning":"high"},
            {"id":"b","supported_parameters":"reasoning"},
            {"id":"c","architecture":[]},
            {"id":"d","model_spec":"none"}
        ]}"#;
        let models = parse_models(body, "p").unwrap();
        assert_eq!(models.len(), 4, "every record is still listed");
        assert!(models.iter().all(|m| m.reasoning.is_none()));
        assert!(models.iter().all(|m| m.vision.is_none()));
    }

    #[test]
    fn a_misshapen_reasoning_field_reads_as_a_default_reasoner() {
        // The object is there, so the model reasons; its inner fields are junk,
        // so the ladder falls back to the default rungs rather than vanishing.
        let body = r#"{"data":[{"id":"m","reasoning":{"mandatory":"no","supported_efforts":"low","default_effort":7}}]}"#;
        let models = parse_models(body, "p").unwrap();
        let support = models[0].reasoning.as_ref().expect("reasoning-capable");
        assert_eq!(support.efforts, DEFAULT_EFFORTS.to_vec());
        assert!(support.can_disable);
        assert_eq!(support.default_effort, None);
    }
}
