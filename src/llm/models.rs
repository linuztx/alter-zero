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
    let mut out: Vec<ModelEntry> = parsed
        .data
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
    let id = record.get("id")?.as_str()?;
    if id.is_empty() {
        return None;
    }
    let display_name = record
        .get("name")
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
///
/// `None` when the record doesn't say, or reports a non-positive size (a
/// meaningless window would divide the gauge by zero). See `docs/compact.md`.
fn context_window_of(record: &serde_json::Value) -> Option<u64> {
    let raw = record
        .get("context_length")
        .and_then(serde_json::Value::as_u64)
        .or_else(|| {
            record
                .get("model_spec")?
                .get("availableContextTokens")?
                .as_u64()
        })?;
    (raw > 0).then_some(raw)
}

/// The `/models` endpoint for a config: `{api_model_base}/models`.
#[must_use]
pub fn models_endpoint(cfg: &ModelConfig) -> String {
    let base = cfg.api_model_base.trim_end_matches('/');
    format!("{base}/models")
}

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
    let url = models_endpoint(cfg);
    let mut req = client.get(&url).header("accept", "application/json");
    if let Some(key) = &cfg.api_key {
        req = req.bearer_auth(key);
    }
    for (k, v) in &cfg.extra_headers {
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
    parse_models(&body, &cfg.provider_id)
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
}
