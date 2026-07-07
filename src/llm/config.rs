//! Provider configuration — the pure parse of `providers.toml` and the
//! resolution of a chosen provider/model/key into a [`ModelConfig`] the
//! [`super::openai::OpenAiClient`] consumes.
//!
//! Pure and unit-tested: env reads and the file read happen at the boundary
//! (`main.rs`), which hands the resolved [`Selection`] in. See `docs/llm.md`.

use std::collections::BTreeMap;

use serde::Deserialize;

/// The default `providers.toml` shipped in the repo root, embedded so the app
/// always has the reference providers even when no file is found.
const DEFAULT_PROVIDERS_TOML: &str = include_str!("../../providers.toml");

/// One `[providers.<id>]` block. Unknown keys are ignored so the file can carry
/// provider-specific extras without breaking the parse.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Provider {
    /// Human-readable label shown in the picker footer / logs.
    pub name: String,
    /// Endpoint whose `/models` the picker lists. Falls back to
    /// [`Kwargs::api_base`] when unset.
    #[serde(default)]
    pub api_model_base: Option<String>,
    /// Environment variable the API key is read from. Defaults to
    /// `<ID_UPPERCASE>_API_KEY` (see [`Provider::key_env`]).
    #[serde(default)]
    pub api_key_env: Option<String>,
    /// Static request headers merged into every call (e.g. OpenRouter's
    /// `HTTP-Referer` / `X-Title`). Empty by default.
    #[serde(default)]
    pub extra_headers: BTreeMap<String, String>,
    /// The pass-through client kwargs: `api_base` (the chat-completions base)
    /// plus any provider-specific extras (e.g. `venice_parameters`) forwarded
    /// verbatim in the request body.
    #[serde(default)]
    pub kwargs: Kwargs,
}

/// The `[providers.<id>.kwargs]` table: the chat base plus arbitrary extras.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Kwargs {
    /// The chat-completions base URL (`{api_base}/chat/completions`).
    #[serde(default)]
    pub api_base: String,
    /// Everything else under `kwargs` — forwarded into the request body as-is
    /// (nested tables like `venice_parameters` round-trip).
    #[serde(flatten)]
    pub extra: BTreeMap<String, toml::Value>,
}

impl Provider {
    /// The environment variable this provider's API key is read from: the
    /// explicit `api_key_env`, else `<ID_UPPERCASE>_API_KEY`.
    #[must_use]
    pub fn key_env(&self, id: &str) -> String {
        self.api_key_env
            .clone()
            .unwrap_or_else(|| format!("{}_API_KEY", id.to_uppercase()))
    }

    /// Where the picker lists models from: `api_model_base` if set, else the
    /// chat `api_base`. Trailing slash trimmed.
    #[must_use]
    pub fn models_base(&self) -> String {
        let base = self
            .api_model_base
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or(&self.kwargs.api_base);
        base.trim_end_matches('/').to_string()
    }

    /// The pass-through kwargs (minus `api_base`) as a JSON object to merge into
    /// the request body. Empty when the provider adds nothing.
    #[must_use]
    pub fn extra_body(&self) -> serde_json::Map<String, serde_json::Value> {
        let mut out = serde_json::Map::new();
        for (k, v) in &self.kwargs.extra {
            if let Ok(json) = serde_json::to_value(v) {
                out.insert(k.clone(), json);
            }
        }
        out
    }
}

/// The parsed `providers.toml`: id → provider. A `BTreeMap` so iteration is
/// deterministic (alphabetical by id), which the picker and default-provider
/// pick both rely on.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ProvidersFile {
    #[serde(default)]
    pub providers: BTreeMap<String, Provider>,
}

impl ProvidersFile {
    /// Parse a `providers.toml` string.
    ///
    /// # Errors
    /// Returns the toml error message when the document is malformed.
    pub fn parse(text: &str) -> Result<Self, String> {
        toml::from_str(text).map_err(|e| e.to_string())
    }

    /// The built-in providers (the repo's `providers.toml`), used when no file
    /// is found. Panics only if the embedded file is malformed — a build-time
    /// guarantee, covered by a test.
    #[must_use]
    pub fn builtin() -> Self {
        Self::parse(DEFAULT_PROVIDERS_TOML).expect("embedded providers.toml parses")
    }

    /// Provider ids in deterministic (alphabetical) order.
    #[must_use]
    pub fn ids(&self) -> Vec<String> {
        self.providers.keys().cloned().collect()
    }

    /// The provider to use by default: `openrouter` when present (the most
    /// common OpenAI-compatible aggregator), else the first id alphabetically,
    /// else `None` when the file is empty.
    #[must_use]
    pub fn default_provider(&self) -> Option<String> {
        if self.providers.contains_key("openrouter") {
            return Some("openrouter".to_string());
        }
        self.providers.keys().next().cloned()
    }

    /// Look up one provider by id.
    #[must_use]
    pub fn get(&self, id: &str) -> Option<&Provider> {
        self.providers.get(id)
    }

    /// Resolve a [`Selection`] into a [`ModelConfig`], or `None` when the
    /// selected provider isn't in the file.
    #[must_use]
    pub fn model_config(&self, sel: &Selection) -> Option<ModelConfig> {
        let provider = self.providers.get(&sel.provider_id)?;
        Some(ModelConfig {
            provider_id: sel.provider_id.clone(),
            provider_name: provider.name.clone(),
            model: sel.model.clone(),
            api_base: provider.kwargs.api_base.clone(),
            api_model_base: provider.models_base(),
            api_key: sel.api_key.clone(),
            temperature: sel.temperature,
            extra_headers: provider
                .extra_headers
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            extra_body: provider.extra_body(),
        })
    }
}

/// A chosen provider/model/key, assembled at the boundary from env vars and the
/// `/model` picker's selection, then resolved via [`ProvidersFile::model_config`].
#[derive(Debug, Clone, Default)]
pub struct Selection {
    pub provider_id: String,
    pub model: String,
    pub api_key: Option<String>,
    pub temperature: Option<f32>,
}

/// The fully-resolved config one [`super::openai::OpenAiClient`] talks with.
#[derive(Debug, Clone, Default)]
pub struct ModelConfig {
    pub provider_id: String,
    pub provider_name: String,
    pub model: String,
    /// Chat-completions base (`{api_base}/chat/completions`).
    pub api_base: String,
    /// Models-listing base (`{api_model_base}/models`).
    pub api_model_base: String,
    pub api_key: Option<String>,
    pub temperature: Option<f32>,
    pub extra_headers: Vec<(String, String)>,
    /// Provider kwargs merged into the request body (e.g. `venice_parameters`).
    pub extra_body: serde_json::Map<String, serde_json::Value>,
}

impl ModelConfig {
    /// A bare config pointing at OpenAI itself — the test/`fallback` shape.
    #[must_use]
    pub fn fallback() -> Self {
        Self {
            provider_id: "openai".to_string(),
            provider_name: "OpenAI".to_string(),
            model: "gpt-4o-mini".to_string(),
            api_base: "https://api.openai.com/v1".to_string(),
            api_model_base: "https://api.openai.com/v1".to_string(),
            api_key: None,
            temperature: None,
            extra_headers: Vec::new(),
            extra_body: serde_json::Map::new(),
        }
    }

    /// Is there enough here to talk to a real endpoint? (a non-empty base and a
    /// key). The boundary falls back to the dummy backend when this is false.
    #[must_use]
    pub fn is_usable(&self) -> bool {
        !self.api_base.is_empty() && self.api_key.as_ref().is_some_and(|k| !k.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_providers_parse() {
        let file = ProvidersFile::builtin();
        let ids = file.ids();
        assert!(ids.contains(&"openrouter".to_string()));
        assert!(ids.contains(&"a0_venice".to_string()));
    }

    #[test]
    fn parse_reads_name_and_bases() {
        let file = ProvidersFile::builtin();
        let openrouter = file.get("openrouter").expect("openrouter present");
        assert_eq!(openrouter.name, "OpenRouter");
        assert_eq!(openrouter.kwargs.api_base, "https://openrouter.ai/api/v1");
        assert_eq!(openrouter.models_base(), "https://openrouter.ai/api/v1");
    }

    #[test]
    fn key_env_defaults_to_uppercased_id() {
        let file = ProvidersFile::builtin();
        let p = file.get("openrouter").unwrap();
        assert_eq!(p.key_env("openrouter"), "OPENROUTER_API_KEY");
        assert_eq!(p.key_env("sambanova"), "SAMBANOVA_API_KEY");
    }

    #[test]
    fn key_env_honours_an_explicit_override() {
        let text = r#"
[providers.custom]
name = "Custom"
api_key_env = "MY_SECRET"
[providers.custom.kwargs]
api_base = "https://x/v1"
"#;
        let file = ProvidersFile::parse(text).unwrap();
        assert_eq!(file.get("custom").unwrap().key_env("custom"), "MY_SECRET");
    }

    #[test]
    fn models_base_prefers_api_model_base_then_falls_back() {
        // a0_venice has a distinct api_model_base (model listing goes to Venice
        // directly) from its chat api_base (the Agent Zero proxy, which doesn't
        // serve /models) — the two must not be conflated.
        let builtin = ProvidersFile::builtin();
        let venice = builtin.get("a0_venice").unwrap();
        assert_eq!(venice.models_base(), "https://api.venice.ai/api/v1");
        assert_eq!(
            venice.kwargs.api_base,
            "https://api.agent-zero.ai/venice/v1"
        );

        // A provider with no api_model_base falls back to its chat api_base
        // (slash trimmed).
        let text = r#"
[providers.single]
name = "Single"
[providers.single.kwargs]
api_base = "https://one.example/v1/"
"#;
        let file = ProvidersFile::parse(text).unwrap();
        let single = file.get("single").unwrap();
        assert!(single.api_model_base.is_none());
        assert_eq!(single.models_base(), "https://one.example/v1");
    }

    #[test]
    fn extra_kwargs_pass_through_as_body() {
        let file = ProvidersFile::builtin();
        let venice = file.get("a0_venice").unwrap();
        let body = venice.extra_body();
        // The nested venice_parameters table forwards verbatim.
        let params = body
            .get("venice_parameters")
            .expect("venice_parameters present");
        assert_eq!(params["disable_thinking"], serde_json::json!(true));
        assert_eq!(
            params["include_venice_system_prompt"],
            serde_json::json!(false)
        );
        // api_base is NOT forwarded — it's the endpoint, not a body param.
        assert!(!body.contains_key("api_base"));
    }

    #[test]
    fn default_provider_prefers_openrouter() {
        let file = ProvidersFile::builtin();
        assert_eq!(file.default_provider().as_deref(), Some("openrouter"));
    }

    #[test]
    fn default_provider_falls_back_to_first_alphabetical() {
        let text = r#"
[providers.zzz]
name = "Z"
[providers.zzz.kwargs]
api_base = "https://z/v1"
[providers.aaa]
name = "A"
[providers.aaa.kwargs]
api_base = "https://a/v1"
"#;
        let file = ProvidersFile::parse(text).unwrap();
        assert_eq!(file.default_provider().as_deref(), Some("aaa"));
    }

    #[test]
    fn model_config_resolves_a_selection() {
        let file = ProvidersFile::builtin();
        let sel = Selection {
            provider_id: "openrouter".to_string(),
            model: "anthropic/claude-3.5-haiku".to_string(),
            api_key: Some("sk-test".to_string()),
            temperature: Some(0.7),
        };
        let cfg = file.model_config(&sel).expect("resolves");
        assert_eq!(cfg.model, "anthropic/claude-3.5-haiku");
        assert_eq!(cfg.api_base, "https://openrouter.ai/api/v1");
        assert_eq!(cfg.provider_name, "OpenRouter");
        assert!(cfg.is_usable());
    }

    #[test]
    fn model_config_is_none_for_unknown_provider() {
        let file = ProvidersFile::builtin();
        let sel = Selection {
            provider_id: "nope".to_string(),
            ..Default::default()
        };
        assert!(file.model_config(&sel).is_none());
    }

    #[test]
    fn a_config_without_a_key_is_not_usable() {
        let mut cfg = ModelConfig::fallback();
        assert!(!cfg.is_usable(), "no key");
        cfg.api_key = Some(String::new());
        assert!(!cfg.is_usable(), "empty key");
        cfg.api_key = Some("k".to_string());
        assert!(cfg.is_usable());
    }

    #[test]
    fn malformed_toml_is_an_error_not_a_panic() {
        assert!(ProvidersFile::parse("this is not = = toml").is_err());
    }
}
