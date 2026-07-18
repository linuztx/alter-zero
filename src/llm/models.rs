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
    /// advertises one — drives the Shift+Tab mode cycle. `None` for a model
    /// with no reasoning. See `docs/reasoning.md`.
    pub reasoning: Option<ReasoningSupport>,
}

/// The OpenAI `/models` envelope: `{ "data": [ { "id", "name"? } ] }`. Each
/// record stays a raw [`serde_json::Value`] so one malformed aggregator entry
/// (a missing/null/non-string id, a non-object row) is skipped per-record
/// instead of failing the whole list into a Decode error.
#[derive(Debug, Deserialize)]
struct ModelsResponse {
    #[serde(default)]
    data: Vec<serde_json::Value>,
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
        .filter_map(|record| {
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
            })
        })
        .collect();
    out.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(out)
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
    // A one-shot GET: a generous per-operation timeout (the /models body can be
    // a few hundred KB across several reads, each bounded by this).
    let client = super::http_client(std::time::Duration::from_secs(30))?;
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
        let _ = resp.read_to_string(&mut body);
        return Err(LlmError::Api { status, body });
    }
    if cancel.is_cancelled() {
        return Err(LlmError::Cancelled);
    }
    let mut body = String::new();
    resp.read_to_string(&mut body)
        .map_err(|e| LlmError::Http(e.to_string()))?;
    parse_models(&body, &cfg.provider_id)
}

#[cfg(test)]
mod tests {
    use super::*;

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
