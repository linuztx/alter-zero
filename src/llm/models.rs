//! The `/v1/models` listing: the pure response parse ([`parse_models`], the
//! [`ModelEntry`] row the `/model` picker shows) and the boundary
//! [`fetch_models`] that GETs it. See `docs/llm.md`.

use std::io::Read;

use serde::Deserialize;

use super::config::ModelConfig;
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
}

/// The OpenAI `/models` envelope: `{ "data": [ { "id", "name"? } ] }`.
#[derive(Debug, Deserialize)]
struct ModelsResponse {
    #[serde(default)]
    data: Vec<ModelRecord>,
}

#[derive(Debug, Deserialize)]
struct ModelRecord {
    id: String,
    /// OpenRouter and some aggregators include a human name; OpenAI itself does
    /// not (we fall back to the id).
    #[serde(default)]
    name: Option<String>,
}

/// Parse a `/models` response body into rows tagged with `provider`, sorted by
/// id (the picker lists alphabetically, matching the mock). Records without an
/// id are skipped.
///
/// # Errors
/// Returns a decode error when the body isn't the expected envelope.
pub fn parse_models(body: &str, provider: &str) -> Result<Vec<ModelEntry>> {
    let parsed: ModelsResponse =
        serde_json::from_str(body).map_err(|e| LlmError::Decode(e.to_string()))?;
    let mut out: Vec<ModelEntry> = parsed
        .data
        .into_iter()
        .filter(|r| !r.id.is_empty())
        .map(|r| {
            let display_name = r
                .name
                .filter(|n| !n.trim().is_empty())
                .unwrap_or_else(|| r.id.clone());
            ModelEntry {
                id: r.id,
                provider: provider.to_string(),
                display_name,
            }
        })
        .collect();
    out.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(out)
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
    let client = super::http_client()?;
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
