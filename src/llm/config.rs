//! Provider configuration — the pure parse of `providers.toml` and the
//! resolution of a chosen provider/model/key into a [`ModelConfig`] the
//! [`super::openai::OpenAiClient`] consumes.
//!
//! Pure and unit-tested: env reads and the file read happen at the boundary
//! (`main.rs`), which hands the resolved [`Selection`] in. See `docs/llm.md`.

use std::collections::BTreeMap;

use serde::Deserialize;

use super::reasoning::ThinkingMode;

/// The default `providers.toml` shipped in the repo root, embedded so the app
/// always has the reference providers even when no file is found.
const DEFAULT_PROVIDERS_TOML: &str = include_str!("../../providers.toml");

/// How a provider authenticates — which decides both how `/login` signs you in
/// and what rides the request's `Authorization` header.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthScheme {
    /// GitHub Copilot: `/login` runs GitHub's device flow and stores the
    /// resulting OAuth token, which each request exchanges for a short-lived
    /// Copilot bearer. See `docs/copilot.md`.
    GithubCopilot,
    /// OpenAI's ChatGPT seat: `/login` runs OpenAI's PKCE loopback flow and
    /// stores the resulting **refresh** token, which each request mints a
    /// short-lived access token from. See `docs/chatgpt.md`.
    ///
    /// Renamed explicitly: `rename_all = "snake_case"` spells this variant
    /// `open_ai_chat_gpt`, and an `auth` value that doesn't match falls
    /// silently through to [`Self::ApiKey`] below — a subscription provider
    /// that looks configured and asks for a pasted key instead.
    #[serde(rename = "openai_chatgpt")]
    OpenAiChatGpt,
    /// An Anthropic **Console** account: `/login` runs Anthropic's own PKCE
    /// flow and stores the resulting **refresh** token, which each request
    /// mints a short-lived access token from. Usage bills to the account's
    /// API organisation, exactly as a pasted key does.
    ///
    /// Deliberately *not* the Claude Pro/Max subscription sign-in — see
    /// `docs/claude.md` for the policy that rules that one out.
    ///
    /// `rename_all = "snake_case"` already spells this `anthropic_console`,
    /// which is the value the provider file uses — unlike
    /// [`Self::OpenAiChatGpt`] above, no explicit rename is needed. A test
    /// pins it, since a mismatch degrades **silently** into
    /// [`Self::ApiKey`]: a sign-in provider that looks configured and asks
    /// for a pasted key instead.
    AnthropicConsole,
    /// A key that is **accepted but not required**: Ollama's local server
    /// takes no credential at all, while a hosted one (or a proxy in front
    /// of one) takes an ordinary bearer — so a stored key rides as
    /// `Authorization: Bearer` exactly as [`Self::ApiKey`]'s does, and none
    /// stored is not a reason to fall back to the dummy. See `docs/ollama.md`.
    OptionalKey,
    /// `Authorization: Bearer {api_key}` — a key the user pastes, and the
    /// scheme every OpenAI-compatible provider uses. The **fallback** for an
    /// unrecognised `auth` value too (`#[serde(other)]`, which serde requires
    /// on the last variant): a provider file written against a newer build
    /// must degrade, not fail the whole parse.
    #[default]
    #[serde(other)]
    ApiKey,
}

impl AuthScheme {
    /// Is this a subscription signed in to, rather than a key pasted? The
    /// split `/login` shows its two lists on.
    #[must_use]
    pub fn is_subscription(self) -> bool {
        !matches!(self, Self::ApiKey | Self::OptionalKey)
    }

    /// Can a request go out with no credential stored at all? Only
    /// [`Self::OptionalKey`] — every other scheme's request is unauthenticated
    /// without one, which the boundary answers with the dummy backend.
    #[must_use]
    pub fn key_optional(self) -> bool {
        matches!(self, Self::OptionalKey)
    }
}

/// Which request/response wire format a provider speaks. Kept apart from
/// [`AuthScheme`] on purpose: how you *authenticate* and what shape the
/// request takes are two questions, and an OpenAI API key can reach the
/// Responses API just as a ChatGPT sign-in can. See `docs/chatgpt.md`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WireApi {
    /// OpenAI's **Responses** API: `{api_base}/responses`, an `input` array of
    /// typed items under a top-level `instructions`, and its own SSE event
    /// vocabulary. See `docs/chatgpt.md`.
    Responses,
    /// Anthropic's **Messages** API: `{api_base}/messages`, a top-level
    /// `system` beside a `messages` array of typed content blocks, and its own
    /// SSE event vocabulary. Reached by a pasted key *or* by a Claude
    /// subscription — which is exactly why it is a `wire_api` and not a
    /// consequence of [`AuthScheme`]. See `docs/claude.md`.
    Anthropic,
    /// Ollama's **native** chat API: `{api_base}/api/chat`, an NDJSON stream
    /// rather than SSE, tool-call arguments as objects, images as bare
    /// base64, and — the reason it exists beside Ollama's own OpenAI-compatible
    /// `/v1` — an `options.num_ctx` the session sets the context window with.
    /// See `docs/ollama.md`.
    Ollama,
    /// **Chat Completions**: `{api_base}/chat/completions` with `messages`.
    /// The default, and the **fallback** for an unrecognised value
    /// (`#[serde(other)]`, which serde requires on the last variant) — a
    /// provider file written against a newer build must degrade, not fail the
    /// whole parse, exactly as [`AuthScheme`] does.
    #[default]
    #[serde(other)]
    Chat,
}

/// One `[providers.<id>]` block. Unknown keys are ignored so the file can carry
/// provider-specific extras without breaking the parse.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Provider {
    /// Human-readable label shown in the picker footer / logs.
    pub name: String,
    /// How this provider authenticates — a pasted key by default.
    #[serde(default)]
    pub auth: AuthScheme,
    /// The wire format its requests take — Chat Completions by default.
    #[serde(default)]
    pub wire_api: WireApi,
    /// One line saying what this provider *is*. A subscription's says what
    /// signing in means; a pasted-key provider's introduces it on `/login`'s
    /// key step, above the field and the page its keys are made on — "Enter
    /// your OpenRouter API key" being an instruction, not an explanation.
    /// Either way it also steers `/login`'s type-to-search, so a provider is
    /// findable by what it does and not only by what it is called.
    #[serde(default)]
    pub description: Option<String>,
    /// Where this provider's API keys are created — linked under the
    /// description on `/login`'s key step, so a user who hasn't got a key can
    /// go and make one instead of leaving the flow to search for the page. A
    /// provider that takes no key (Ollama) names where the **server** comes
    /// from instead; the field is the link, the wording is the view's.
    #[serde(default)]
    pub api_key_url: Option<String>,
    /// Endpoint whose `/models` the picker lists. Falls back to
    /// [`Kwargs::api_base`] when unset.
    #[serde(default)]
    pub api_model_base: Option<String>,
    /// Environment variable the API key is read from. Defaults to
    /// `<ID_UPPERCASE>_API_KEY` (see [`Provider::key_env`]).
    #[serde(default)]
    pub api_key_env: Option<String>,
    /// An environment variable whose value **replaces** [`Kwargs::api_base`]
    /// when set — Ollama's own `OLLAMA_HOST`, in Ollama's own grammar
    /// (`docs/ollama.md`). Resolved at the boundary like the key (process
    /// env, then the `.env` store) and handed in as [`Selection::api_base`].
    /// `None` for a provider whose base only the file names.
    #[serde(default)]
    pub api_base_env: Option<String>,
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

/// The default environment variable a provider's API key is read from:
/// `<ID_UPPERCASE>_API_KEY`, with any character that isn't valid in an
/// env-var name (`-`, `.`, …) mapped to `_` — `EnvFile::parse` (rightly)
/// skips invalid keys, so an unsanitized name would let `/login` write a key
/// that never resolves again. The one place this rule lives (`main.rs`'s
/// unknown-provider fallback shares it).
#[must_use]
pub fn default_key_env(id: &str) -> String {
    let sanitized: String = id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect();
    format!("{sanitized}_API_KEY")
}

impl Provider {
    /// The environment variable this provider's API key is read from: the
    /// explicit `api_key_env`, else [`default_key_env`].
    #[must_use]
    pub fn key_env(&self, id: &str) -> String {
        self.api_key_env
            .clone()
            .unwrap_or_else(|| default_key_env(id))
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
        // The boundary's resolved `api_base_env` value outranks the file's
        // base — and on the Ollama wire both are spelled in `OLLAMA_HOST`'s
        // grammar, so both go through it (a full URL comes out unchanged).
        let override_base = sel
            .api_base
            .as_deref()
            .map(str::trim)
            .filter(|base| !base.is_empty());
        let api_base = match (provider.wire_api, override_base) {
            (WireApi::Ollama, base) => {
                super::ollama::host_url(base.unwrap_or(&provider.kwargs.api_base))
            }
            (_, Some(base)) => base.trim_end_matches('/').to_string(),
            (_, None) => provider.kwargs.api_base.clone(),
        };
        // The listing follows the chat base wherever the file doesn't name a
        // separate one — an overridden host lists from that host too.
        let api_model_base = match provider
            .api_model_base
            .as_deref()
            .filter(|base| !base.is_empty())
        {
            Some(explicit) if override_base.is_none() => explicit.trim_end_matches('/').to_string(),
            _ => api_base.trim_end_matches('/').to_string(),
        };
        Some(ModelConfig {
            provider_id: sel.provider_id.clone(),
            provider_name: provider.name.clone(),
            model: sel.model.clone(),
            api_base,
            api_model_base,
            api_key: sel.api_key.clone(),
            auth: provider.auth,
            wire_api: provider.wire_api,
            temperature: sel.temperature,
            thinking: sel.thinking,
            vision: sel.vision,
            context: sel.context,
            cache_key: sel.cache_key.clone(),
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
    /// The model's active thinking mode, when it supports reasoning — rides
    /// into the request payload. `None` sends no reasoning parameter at all.
    /// See `docs/reasoning.md`.
    pub thinking: Option<ThinkingMode>,
    /// Whether the model accepts image input, when known (from the same
    /// `/v1/models` records the picker lists — `ModelEntry::vision`).
    /// `Some(false)` makes the backend degrade attachments gracefully instead
    /// of letting the provider fail the turn; `None` attaches optimistically.
    /// See `docs/tools.md`.
    pub vision: Option<bool>,
    /// The context window the session gauges against, when known — the
    /// same number the footer's `{used}/{window}` reads. It rides the config
    /// because the Ollama wire **sends** it (`options.num_ctx`): the window
    /// the server holds must be the one the gauge claims (`docs/ollama.md`).
    /// Every other wire ignores it. `None` = unknown.
    pub context: Option<u64>,
    /// The boundary's resolved [`Provider::api_base_env`] value, replacing the
    /// file's base when set (Ollama's `OLLAMA_HOST`). `None` or empty leaves
    /// the file's base in force.
    pub api_base: Option<String>,
    /// A stable per-session cache-affinity key, sent as the request's
    /// `prompt_cache_key` (and, for OpenRouter, `session_id`) so repeated
    /// requests land on the same provider/server and hit its warm prompt
    /// cache. The boundary mints one per process. See `docs/prompt-caching.md`.
    pub cache_key: Option<String>,
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
    /// How [`api_key`](Self::api_key) authenticates the request — a bearer as
    /// stored, or (Copilot/ChatGPT) an OAuth token to exchange for one first.
    pub auth: AuthScheme,
    /// The wire format the request takes (see [`WireApi`]).
    pub wire_api: WireApi,
    pub temperature: Option<f32>,
    /// The active thinking mode (see [`Selection::thinking`]).
    pub thinking: Option<ThinkingMode>,
    /// The model's image-input support (see [`Selection::vision`]).
    pub vision: Option<bool>,
    /// The session's context window (see [`Selection::context`]) — sent as
    /// `options.num_ctx` on the Ollama wire, ignored by every other.
    pub context: Option<u64>,
    /// The per-session cache-affinity key (see [`Selection::cache_key`]).
    pub cache_key: Option<String>,
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
            auth: AuthScheme::ApiKey,
            wire_api: WireApi::Chat,
            temperature: None,
            thinking: None,
            vision: None,
            context: None,
            cache_key: None,
            extra_headers: Vec::new(),
            extra_body: serde_json::Map::new(),
        }
    }

    /// Is there enough here to talk to a real endpoint? A non-empty base, and
    /// a key — unless the scheme says a key is optional (a local Ollama
    /// server). The boundary falls back to the dummy backend when this is
    /// false.
    #[must_use]
    pub fn is_usable(&self) -> bool {
        let keyed = self.api_key.as_ref().is_some_and(|k| !k.is_empty());
        !self.api_base.is_empty() && (keyed || self.auth.key_optional())
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
    fn key_env_of_a_punctuated_id_is_a_valid_env_var_name() {
        // A provider id like "my-provider.v2" must not derive "MY-PROVIDER.V2_
        // API_KEY": EnvFile::parse (rightly) skips keys with '-'/'.', so a
        // /login-saved key would never resolve again. Punctuation maps to '_'.
        assert_eq!(default_key_env("my-provider.v2"), "MY_PROVIDER_V2_API_KEY");
        let file = ProvidersFile::parse(
            "[providers.my-provider]\nname = \"X\"\n[providers.my-provider.kwargs]\napi_base = \"https://x/v1\"\n",
        )
        .unwrap();
        let p = file.get("my-provider").unwrap();
        assert_eq!(p.key_env("my-provider"), "MY_PROVIDER_API_KEY");
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
        // The nested venice_parameters table forwards verbatim. Its presence
        // is also the Venice-family marker the payload builder keys the
        // per-mode `disable_thinking` toggle on (docs/reasoning.md) — so the
        // file must keep the table, but no longer pin `disable_thinking`
        // statically (the Ctrl+T mode owns it now).
        let params = body
            .get("venice_parameters")
            .expect("venice_parameters present");
        assert_eq!(
            params["include_venice_system_prompt"],
            serde_json::json!(false)
        );
        assert!(
            params.get("disable_thinking").is_none(),
            "thinking is per-mode, not pinned in the file"
        );
        // api_base is NOT forwarded — it's the endpoint, not a body param.
        assert!(!body.contains_key("api_base"));
    }

    // --- auth schemes: a pasted key vs a subscription sign-in (docs/copilot.md) ---

    #[test]
    fn a_provider_authenticates_with_a_pasted_key_by_default() {
        let file = ProvidersFile::builtin();
        assert_eq!(file.get("openrouter").unwrap().auth, AuthScheme::ApiKey);
        assert!(!AuthScheme::ApiKey.is_subscription());
    }

    #[test]
    fn a_provider_can_declare_a_subscription_sign_in() {
        // `/login` splits its two lists on this: a subscription provider is
        // signed in to, never keyed.
        let text = r#"
[providers.github_copilot]
name = "GitHub Copilot"
auth = "github_copilot"
description = "Sign in with your GitHub account"
[providers.github_copilot.kwargs]
api_base = "https://api.githubcopilot.com"
"#;
        let file = ProvidersFile::parse(text).unwrap();
        let copilot = file.get("github_copilot").unwrap();
        assert_eq!(copilot.auth, AuthScheme::GithubCopilot);
        assert!(copilot.auth.is_subscription());
        assert_eq!(
            copilot.description.as_deref(),
            Some("Sign in with your GitHub account")
        );
    }

    #[test]
    fn a_provider_can_declare_the_chatgpt_subscription_sign_in() {
        // The OpenAI ChatGPT seat is a sign-in like Copilot's, not a key: the
        // stored secret is a refresh token, and every request mints the
        // short-lived access token from it (`docs/chatgpt.md`).
        let text = r#"
[providers.openai_chatgpt]
name = "OpenAI (ChatGPT)"
auth = "openai_chatgpt"
description = "Sign in with your ChatGPT account"
[providers.openai_chatgpt.kwargs]
api_base = "https://chatgpt.com/backend-api/codex"
"#;
        let file = ProvidersFile::parse(text).unwrap();
        let chatgpt = file.get("openai_chatgpt").unwrap();
        assert_eq!(chatgpt.auth, AuthScheme::OpenAiChatGpt);
        assert!(chatgpt.auth.is_subscription());
    }

    // --- the wire format a provider speaks (docs/chatgpt.md) ---

    #[test]
    fn a_provider_speaks_chat_completions_by_default() {
        let file = ProvidersFile::builtin();
        assert_eq!(file.get("openrouter").unwrap().wire_api, WireApi::Chat);
        assert_eq!(ModelConfig::fallback().wire_api, WireApi::Chat);
    }

    #[test]
    fn a_provider_can_declare_the_responses_wire_format() {
        let text = r#"
[providers.p]
name = "P"
wire_api = "responses"
[providers.p.kwargs]
api_base = "https://x/v1"
"#;
        let file = ProvidersFile::parse(text).unwrap();
        assert_eq!(file.get("p").unwrap().wire_api, WireApi::Responses);
    }

    #[test]
    fn an_unknown_wire_format_falls_back_to_chat_completions() {
        // Same degrade-don't-fail rule as `auth`: a file written against a
        // newer build must still parse.
        let text = r#"
[providers.p]
name = "P"
wire_api = "telepathy"
[providers.p.kwargs]
api_base = "https://x/v1"
"#;
        let file = ProvidersFile::parse(text).expect("the file still parses");
        assert_eq!(file.get("p").unwrap().wire_api, WireApi::Chat);
    }

    #[test]
    fn model_config_carries_the_providers_wire_format() {
        let file = ProvidersFile::builtin();
        let sel = Selection {
            provider_id: "openai_chatgpt".to_string(),
            model: "gpt-5.5".to_string(),
            ..Selection::default()
        };
        let cfg = file.model_config(&sel).expect("shipped");
        assert_eq!(cfg.wire_api, WireApi::Responses);
        assert_eq!(cfg.auth, AuthScheme::OpenAiChatGpt);
    }

    #[test]
    fn a_provider_can_declare_the_anthropic_console_sign_in() {
        // The third sign-in: the stored secret is an OAuth refresh token, and
        // every request mints the short-lived access token from it
        // (`docs/claude.md`).
        let text = r#"
[providers.anthropic_console]
name = "Anthropic Console"
auth = "anthropic_console"
description = "Sign in with your Anthropic account"
[providers.anthropic_console.kwargs]
api_base = "https://api.anthropic.com/v1"
"#;
        let file = ProvidersFile::parse(text).unwrap();
        let console = file.get("anthropic_console").unwrap();
        assert_eq!(console.auth, AuthScheme::AnthropicConsole);
        assert!(console.auth.is_subscription());
    }

    #[test]
    fn every_shipped_key_provider_describes_itself_and_names_its_key_page() {
        // `/login`'s key step introduces the provider it is asking a secret
        // for, over a link to the page that secret is made on. Both come from
        // this file, so a provider added without them silently ships a page
        // that asks for a key and says nothing about where to get one — which
        // is the page this pair exists to replace.
        let file = ProvidersFile::builtin();
        for id in file.ids() {
            let provider = file.get(&id).expect("listed");
            if provider.auth.is_subscription() {
                continue; // signed in to, never keyed — its page is the sign-in
            }
            let described = provider.description.as_deref().unwrap_or_default();
            assert!(!described.is_empty(), "{id} has no description");
            let url = provider.api_key_url.as_deref().unwrap_or_default();
            assert!(
                url.starts_with("https://"),
                "{id} names no https key page: {url:?}"
            );
        }
    }

    #[test]
    fn a_provider_can_declare_the_anthropic_wire_format() {
        let text = r#"
[providers.p]
name = "P"
wire_api = "anthropic"
[providers.p.kwargs]
api_base = "https://api.anthropic.com/v1"
"#;
        let file = ProvidersFile::parse(text).unwrap();
        assert_eq!(file.get("p").unwrap().wire_api, WireApi::Anthropic);
    }

    #[test]
    fn the_builtin_file_ships_both_anthropic_providers() {
        // The same API, reached two ways: a pasted key and a sign-in. Both
        // speak the Messages wire format; only `auth` differs.
        let file = ProvidersFile::builtin();
        let key = file.get("anthropic").expect("shipped");
        assert_eq!(key.auth, AuthScheme::ApiKey);
        assert_eq!(key.wire_api, WireApi::Anthropic);
        assert_eq!(key.key_env("anthropic"), "ANTHROPIC_API_KEY");
        assert_eq!(key.kwargs.api_base, "https://api.anthropic.com/v1");

        let console = file.get("anthropic_console").expect("shipped");
        assert_eq!(console.auth, AuthScheme::AnthropicConsole);
        assert_eq!(console.wire_api, WireApi::Anthropic);
        assert!(console.description.is_some(), "a sign-in row needs one");
        assert_eq!(
            console.key_env("anthropic_console"),
            "ANTHROPIC_CONSOLE_REFRESH_TOKEN"
        );
    }

    #[test]
    fn both_anthropic_providers_send_the_api_version_header() {
        // `anthropic-version` is required on every Messages API request; it is
        // the API's own version pin, so it rides the file rather than the code.
        let file = ProvidersFile::builtin();
        for id in ["anthropic", "anthropic_console"] {
            let provider = file.get(id).expect("shipped");
            assert_eq!(
                provider
                    .extra_headers
                    .get("anthropic-version")
                    .map(String::as_str),
                Some("2023-06-01"),
                "{id} must pin the API version"
            );
        }
    }

    #[test]
    fn an_unknown_auth_scheme_falls_back_to_a_pasted_key() {
        // A provider file from a newer build must not fail the whole parse —
        // an unrecognised scheme degrades to the one every provider supports.
        let text = r#"
[providers.x]
name = "X"
auth = "retina-scan"
[providers.x.kwargs]
api_base = "https://x/v1"
"#;
        let file = ProvidersFile::parse(text).expect("the file still parses");
        assert_eq!(file.get("x").unwrap().auth, AuthScheme::ApiKey);
    }

    #[test]
    fn the_builtin_file_ships_github_copilot_as_a_subscription() {
        let file = ProvidersFile::builtin();
        let copilot = file.get("github_copilot").expect("shipped");
        assert_eq!(copilot.auth, AuthScheme::GithubCopilot);
        assert_eq!(copilot.name, "GitHub Copilot");
        assert_eq!(copilot.kwargs.api_base, "https://api.githubcopilot.com");
        assert_eq!(copilot.key_env("github_copilot"), "GITHUB_COPILOT_TOKEN");
    }

    #[test]
    fn model_config_carries_the_providers_auth_scheme() {
        // The request builder reads it to decide what the Authorization header
        // gets: the stored key, or a token exchanged from it.
        let file = ProvidersFile::builtin();
        let sel = Selection {
            provider_id: "github_copilot".to_string(),
            model: "gpt-4o".to_string(),
            api_key: Some("gho_test".to_string()),
            ..Default::default()
        };
        let cfg = file.model_config(&sel).expect("resolves");
        assert_eq!(cfg.auth, AuthScheme::GithubCopilot);
        assert_eq!(ModelConfig::fallback().auth, AuthScheme::ApiKey);
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
            thinking: None,
            vision: None,
            context: None,
            api_base: None,
            cache_key: None,
        };
        let cfg = file.model_config(&sel).expect("resolves");
        assert_eq!(cfg.model, "anthropic/claude-3.5-haiku");
        assert_eq!(cfg.api_base, "https://openrouter.ai/api/v1");
        assert_eq!(cfg.provider_name, "OpenRouter");
        assert!(cfg.is_usable());
    }

    #[test]
    fn model_config_carries_the_selections_vision() {
        // The picked model's detected image-input support rides the resolved
        // config so the backend can gate attachments (docs/tools.md).
        let file = ProvidersFile::builtin();
        let sel = Selection {
            provider_id: "openrouter".to_string(),
            model: "openai/gpt-oss-120b".to_string(),
            api_key: Some("k".to_string()),
            vision: Some(false),
            ..Default::default()
        };
        let cfg = file.model_config(&sel).expect("resolves");
        assert_eq!(cfg.vision, Some(false));
        assert_eq!(
            ModelConfig::fallback().vision,
            None,
            "unknown by default — attach optimistically"
        );
    }

    #[test]
    fn model_config_carries_the_selections_cache_key() {
        // The boundary's per-session affinity key rides the resolved config so
        // the payload builder can pin requests to a warm cache
        // (docs/prompt-caching.md).
        let file = ProvidersFile::builtin();
        let sel = Selection {
            provider_id: "openrouter".to_string(),
            model: "openai/gpt-4o-mini".to_string(),
            api_key: Some("k".to_string()),
            cache_key: Some("alter-zero-42".to_string()),
            ..Default::default()
        };
        let cfg = file.model_config(&sel).expect("resolves");
        assert_eq!(cfg.cache_key.as_deref(), Some("alter-zero-42"));
        assert_eq!(ModelConfig::fallback().cache_key, None);
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

    // --- Ollama: the fourth wire format, and a key that is optional (docs/ollama.md) ---

    #[test]
    fn a_provider_can_declare_the_ollama_wire_format() {
        let text = r#"
[providers.p]
name = "P"
wire_api = "ollama"
[providers.p.kwargs]
api_base = "http://127.0.0.1:11434"
"#;
        let file = ProvidersFile::parse(text).unwrap();
        assert_eq!(file.get("p").unwrap().wire_api, WireApi::Ollama);
    }

    #[test]
    fn an_optional_key_provider_is_usable_without_a_key() {
        // A local server takes no credential at all, so "no key" must not
        // mean "fall back to the dummy" — the rule every other provider
        // lives by. A stored key still rides as a bearer (a hosted server).
        let text = r#"
[providers.p]
name = "P"
auth = "optional_key"
wire_api = "ollama"
[providers.p.kwargs]
api_base = "http://127.0.0.1:11434"
"#;
        let file = ProvidersFile::parse(text).unwrap();
        let provider = file.get("p").unwrap();
        assert_eq!(provider.auth, AuthScheme::OptionalKey);
        assert!(!provider.auth.is_subscription(), "it is keyed, optionally");
        assert!(provider.auth.key_optional());
        assert!(!AuthScheme::ApiKey.key_optional());
        let sel = Selection {
            provider_id: "p".to_string(),
            model: "qwen3:8b".to_string(),
            ..Selection::default()
        };
        let cfg = file.model_config(&sel).expect("resolves");
        assert!(cfg.is_usable(), "no key needed");
        let mut blank = cfg.clone();
        blank.api_base = String::new();
        assert!(!blank.is_usable(), "a base is still needed");
    }

    #[test]
    fn the_builtin_file_ships_ollama_local_and_cloud() {
        let file = ProvidersFile::builtin();
        let local = file.get("ollama").expect("shipped");
        assert_eq!(local.wire_api, WireApi::Ollama);
        assert_eq!(local.auth, AuthScheme::OptionalKey);
        assert_eq!(local.key_env("ollama"), "OLLAMA_API_KEY");
        assert_eq!(local.api_base_env.as_deref(), Some("OLLAMA_HOST"));
        assert_eq!(local.kwargs.api_base, "http://127.0.0.1:11434");

        let cloud = file.get("ollama_cloud").expect("shipped");
        assert_eq!(cloud.wire_api, WireApi::Ollama);
        assert_eq!(cloud.auth, AuthScheme::ApiKey, "the cloud needs its key");
        assert_eq!(cloud.key_env("ollama_cloud"), "OLLAMA_API_KEY");
        assert_eq!(cloud.api_base_env, None);
        assert_eq!(cloud.kwargs.api_base, "https://ollama.com");
    }

    #[test]
    fn a_selections_base_override_is_normalized_for_the_ollama_wire() {
        // `OLLAMA_HOST` is spelled in Ollama's own grammar (`localhost:11434`,
        // `0.0.0.0`, `https://host`) and resolved at the boundary; the config
        // carries it as a full URL, for the listing and the chat alike.
        let file = ProvidersFile::builtin();
        let sel = Selection {
            provider_id: "ollama".to_string(),
            model: "qwen3:8b".to_string(),
            api_base: Some("myhost:11434".to_string()),
            ..Selection::default()
        };
        let cfg = file.model_config(&sel).expect("resolves");
        assert_eq!(cfg.api_base, "http://myhost:11434");
        assert_eq!(cfg.api_model_base, "http://myhost:11434");
        // No override: the file's base, still normalized.
        let plain = file
            .model_config(&Selection {
                provider_id: "ollama".to_string(),
                model: "qwen3:8b".to_string(),
                ..Selection::default()
            })
            .unwrap();
        assert_eq!(plain.api_base, "http://127.0.0.1:11434");
        // An empty override is no override.
        let empty = file
            .model_config(&Selection {
                provider_id: "ollama".to_string(),
                model: "qwen3:8b".to_string(),
                api_base: Some(String::new()),
                ..Selection::default()
            })
            .unwrap();
        assert_eq!(empty.api_base, "http://127.0.0.1:11434");
        // On any other wire the override is taken as the URL it is, slash
        // trimmed like the file's own base.
        let other = file
            .model_config(&Selection {
                provider_id: "openrouter".to_string(),
                model: "m".to_string(),
                api_base: Some("https://proxy.example/v1/".to_string()),
                ..Selection::default()
            })
            .unwrap();
        assert_eq!(other.api_base, "https://proxy.example/v1");
        assert_eq!(other.api_model_base, "https://proxy.example/v1");
    }

    #[test]
    fn model_config_carries_the_selections_context_window() {
        // The window the session gauges against rides the config, because the
        // Ollama wire sends it as `options.num_ctx` (docs/ollama.md).
        let file = ProvidersFile::builtin();
        let sel = Selection {
            provider_id: "ollama".to_string(),
            model: "qwen3:8b".to_string(),
            context: Some(32_768),
            ..Selection::default()
        };
        let cfg = file.model_config(&sel).expect("resolves");
        assert_eq!(cfg.context, Some(32_768));
        assert_eq!(ModelConfig::fallback().context, None);
    }
}
