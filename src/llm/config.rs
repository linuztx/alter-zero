//! Provider configuration — the pure parse of `providers.toml`, the user's own
//! file laid over the shipped one ([`ProvidersFile::with_user_layer`],
//! `docs/user-providers.md`), and the resolution of a chosen
//! provider/model/key into a [`ModelConfig`] the
//! [`super::openai::OpenAiClient`] consumes.
//!
//! Pure and unit-tested: env reads and the file read happen at the boundary
//! (`tui::config`), which hands the file's text and the resolved
//! [`Selection`] in. See `docs/llm.md`.

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
    /// `chat_gpt_codex`, and an `auth` value that doesn't match falls
    /// silently through to [`Self::ApiKey`] below — a subscription provider
    /// that looks configured and asks for a pasted key instead. The alias
    /// is the scheme's name before the provider was renamed, so a provider
    /// file written against that build still reads as a sign-in.
    #[serde(rename = "chatgpt_codex", alias = "openai_chatgpt")]
    ChatGptCodex,
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
    /// [`Self::ChatGptCodex`] above, no explicit rename is needed. A test
    /// pins it, since a mismatch degrades **silently** into
    /// [`Self::ApiKey`]: a sign-in provider that looks configured and asks
    /// for a pasted key instead.
    AnthropicConsole,
    /// A **Cline account**: `/login` runs Cline's own device-code sign-in
    /// (WorkOS, the flow its extension and CLI use) and stores the resulting
    /// refresh token, which each request mints a short-lived access token
    /// from — sent as `Bearer workos:<jwt>`. See `docs/cline.md`.
    ///
    /// A separate provider from the pasted-key `cline` next door
    /// (providers.toml), exactly as `anthropic_console` is separate from
    /// `anthropic`: one row per way in, and the credential in the store is a
    /// different kind of value.
    ///
    /// `rename_all = "snake_case"` spells this `cline_account`, which is the
    /// value the provider file uses. A test pins it for the same silent
    /// degradation reason [`Self::AnthropicConsole`]'s does.
    ClineAccount,
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
    /// Human-readable label shown in the picker footer / logs. Required of
    /// every provider the app loads — but defaulted in the parse, so a block
    /// without one still reads and its loader can say what is missing rather
    /// than fail the whole file (`docs/user-providers.md`).
    #[serde(default)]
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
    /// Top-level request fields this provider's server refuses, taken out of
    /// every request after everything else is set (`docs/user-providers.md`).
    /// Every request carries `stream_options` and `prompt_cache_key`, and a
    /// strict server answers a field it does not know with an error — while
    /// TOML has no null, so a kwarg can replace such a field but never remove
    /// it. Empty for every shipped provider.
    #[serde(default)]
    pub drop_params: Vec<String>,
    /// Every top-level key of the block this build does not read. Kept rather
    /// than dropped so a user's own file can be told about a typo — a misspelt
    /// key otherwise reads as configuration while changing nothing
    /// (`docs/user-providers.md`). Always empty for a shipped provider, which
    /// a test pins.
    #[serde(flatten)]
    pub unread: BTreeMap<String, toml::Value>,
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

    /// Is this provider configured by its definition alone? One whose key is
    /// optional and that names no host variable has nothing left to set: the
    /// block pointing at it *is* the configuration — a local LM Studio,
    /// llama.cpp or vLLM server described in a user's own file
    /// (`docs/user-providers.md`). Ollama names `OLLAMA_HOST`, so it is
    /// configured by being pointed at instead (`docs/ollama.md`), and no
    /// shipped provider qualifies — otherwise every `/model` open would fetch
    /// from a server the user never asked for.
    #[must_use]
    pub fn configured_by_definition(&self) -> bool {
        self.auth.key_optional() && self.api_base_env.is_none()
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

    /// The built-in providers with a user's own `providers.toml` laid over
    /// them (`docs/user-providers.md`), and what was wrong with it — one row
    /// per problem, worded for the toast that reports it.
    ///
    /// The file **adds**: a block whose id is new is a new provider, beside
    /// every shipped one. A block naming a shipped provider **patches** it,
    /// key by key — a table meeting a table merges, anything else replaces —
    /// so `[providers.venice.kwargs.venice_parameters]` can add one body
    /// parameter without restating the rest.
    ///
    /// A problem costs only the block it is in: that block is skipped (or the
    /// shipped provider it patched is kept as shipped) and the rest of the
    /// file still loads. A file that does not parse at all leaves the
    /// built-ins alone in force.
    #[must_use]
    pub fn with_user_layer(text: &str) -> (Self, Vec<String>) {
        Self::layered(&builtin_blocks(), text)
    }

    /// A provider file standing **alone** — only its providers, none of the
    /// built-ins (`ALTER_ZERO_PROVIDERS_FILE`, which is what keeps a smoke run
    /// from fetching every shipped provider beside its stub). Its blocks are
    /// checked as [`Self::with_user_layer`] checks a layer's; a file that does
    /// not parse at all leaves the built-ins in force instead of nothing.
    #[must_use]
    pub fn standalone(text: &str) -> (Self, Vec<String>) {
        Self::layered(&toml::Table::new(), text)
    }

    /// `text`'s blocks laid over `base` (shipped blocks as written, by id).
    fn layered(base: &toml::Table, text: &str) -> (Self, Vec<String>) {
        let mut layer: toml::Table = match toml::from_str(text) {
            Ok(layer) => layer,
            Err(err) => {
                let issue = format!("{} — {UNLOADED_FILE}", syntax_issue(text, &err));
                return (Self::builtin(), vec![issue]);
            }
        };
        let blocks = match layer.remove("providers") {
            None => toml::Table::new(),
            Some(toml::Value::Table(blocks)) => blocks,
            Some(_) => {
                let issue = format!("providers is not a table — {UNLOADED_FILE}");
                return (Self::builtin(), vec![issue]);
            }
        };
        // Whatever sits beside `providers` is read by nothing — most often a
        // provider written without its `providers.` prefix, or a table of
        // them under another name (`[provider.groq]`, codex's
        // `[model_providers.groq]`), which is no provider called `provider`.
        let mut issues: Vec<String> = layer
            .iter()
            .map(|(key, value)| match value {
                toml::Value::Table(block)
                    if block.contains_key("name") || block.contains_key("kwargs") =>
                {
                    format!("[{key}] is not read — a provider goes under [providers.{key}]")
                }
                toml::Value::Table(_) => {
                    format!("[{key}] is not read — providers go under [providers.<id>]")
                }
                _ if key == "model" => format!("model is not a known key{MODEL_FIX}"),
                _ => format!("{key} is not a known key"),
            })
            .collect();
        let mut providers: BTreeMap<String, Provider> = base
            .iter()
            .filter_map(|(id, block)| Some((id.clone(), block.clone().try_into().ok()?)))
            .collect();
        for (id, block) in blocks {
            let shipped = base.get(&id).and_then(toml::Value::as_table);
            if let Some(provider) = layer_block(&id, shipped, block, &mut issues) {
                providers.insert(id, provider);
            }
        }
        (Self { providers }, issues)
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
            service_tier: sel.service_tier.clone(),
            extra_headers: provider
                .extra_headers
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            extra_body: provider.extra_body(),
            drop_params: provider.drop_params.clone(),
        })
    }
}

/// What a file that cannot be read at all comes to: the shipped providers,
/// none of its own.
const UNLOADED_FILE: &str = "using the built-in providers";

/// The shipped file's `[providers.*]` blocks as written — what a user's own
/// blocks are merged into, key by key, before either is read as a
/// [`Provider`].
fn builtin_blocks() -> toml::Table {
    let mut file: toml::Table =
        toml::from_str(DEFAULT_PROVIDERS_TOML).expect("embedded providers.toml parses");
    match file.remove("providers") {
        Some(toml::Value::Table(blocks)) => blocks,
        _ => toml::Table::new(),
    }
}

/// One block of a user's file, merged over the shipped block of the same id
/// when there is one and read as a [`Provider`] — or `None`, the reason
/// pushed onto `issues`. Warnings (a key nothing reads, a scheme or wire
/// format this build doesn't know) are pushed too, but cost the block
/// nothing.
fn layer_block(
    id: &str,
    shipped: Option<&toml::Table>,
    block: toml::Value,
    issues: &mut Vec<String>,
) -> Option<Provider> {
    // What becomes of a block that cannot be used.
    let dropped = if shipped.is_some() {
        "kept the built-in"
    } else {
        "skipped"
    };
    let toml::Value::Table(block) = block else {
        issues.push(format!("providers.{id} is not a table — {dropped}"));
        return None;
    };
    let mut merged = shipped.cloned().unwrap_or_default();
    merge_tables(&mut merged, block);
    // The schemes' own spellings, for the unknown-value warnings below: the
    // parse folds an unknown one into the default, which is right for a file
    // from a newer build and wrong to do in silence for a user's typo.
    let spelled = |key: &str| {
        merged
            .get(key)
            .and_then(toml::Value::as_str)
            .map(str::to_string)
    };
    let (auth, wire) = (spelled("auth"), spelled("wire_api"));
    let provider: Provider = match toml::Value::Table(merged).try_into() {
        Ok(provider) => provider,
        Err(err) => {
            issues.push(format!(
                "providers.{id}: {} — {dropped}",
                one_line(&err.to_string())
            ));
            return None;
        }
    };
    issues.extend(provider.unread.keys().map(|key| unread_key_issue(id, key)));
    if provider.auth == AuthScheme::ApiKey
        && let Some(auth) = auth.filter(|auth| auth != "api_key")
    {
        issues.push(format!(
            "providers.{id}.auth = {auth:?} is not known — using a pasted key"
        ));
    }
    if provider.wire_api == WireApi::Chat
        && let Some(wire) = wire.filter(|wire| wire != "chat")
    {
        issues.push(format!(
            "providers.{id}.wire_api = {wire:?} is not known — using chat completions"
        ));
    }
    issues.extend(
        provider
            .drop_params
            .iter()
            .filter(|field| REQUIRED_FIELDS.contains(&field.as_str()))
            .map(|field| {
                format!("providers.{id}.drop_params cannot drop {field:?} — every request needs it")
            }),
    );
    if provider.name.trim().is_empty() {
        issues.push(format!("providers.{id} has no name — {dropped}"));
        return None;
    }
    let base = provider.kwargs.api_base.trim();
    if base.is_empty() && provider.api_base_env.is_none() {
        issues.push(format!(
            "providers.{id} has no api_base under [providers.{id}.kwargs] — {dropped}"
        ));
        return None;
    }
    // Ollama's bases are hosts in its own grammar (`gpu-box:11434`,
    // `docs/ollama.md`); every other wire needs the URL itself.
    if provider.wire_api != WireApi::Ollama {
        let bases = [
            ("kwargs.api_base", Some(base)),
            ("api_model_base", provider.api_model_base.as_deref()),
        ];
        for (key, url) in bases {
            if let Some(url) = url.map(str::trim).filter(|url| !url.is_empty())
                && !is_http_url(url)
            {
                issues.push(format!(
                    "providers.{id}.{key} {url:?} is not an http(s) URL — {dropped}"
                ));
                return None;
            }
        }
    }
    Some(provider)
}

/// Merge `over` into `base`, key by key: a table meeting a table merges
/// recursively, anything else replaces what was there.
fn merge_tables(base: &mut toml::Table, over: toml::Table) {
    for (key, value) in over {
        match (base.get_mut(&key), value) {
            (Some(toml::Value::Table(into)), toml::Value::Table(from)) => merge_tables(into, from),
            (_, value) => {
                base.insert(key, value);
            }
        }
    }
}

/// The issue a key nothing reads raises — with what this file calls it, for
/// the spellings other tools' provider files use, since a key that is merely
/// ignored is a provider that silently never works.
fn unread_key_issue(id: &str, key: &str) -> String {
    let fix = match key {
        "api_base" | "base_url" => format!(" — set api_base under [providers.{id}.kwargs]"),
        "env_key" => " — name the key's variable with api_key_env".to_string(),
        "headers" | "http_headers" => {
            format!(" — set headers under [providers.{id}.extra_headers]")
        }
        // Keys live in the `.env` store `/login` writes, never in a file
        // meant to be read, shared and copied between machines.
        "api_key" => " — keys are not read from this file: use /login".to_string(),
        "model" | "models" => MODEL_FIX.to_string(),
        _ => String::new(),
    };
    format!("providers.{id}.{key} is not a known key{fix}")
}

/// The fix a `model` key is told, in a block or beside them: the model is
/// the picker's to choose (or `ALTER_ZERO_MODEL`'s), never the file's.
const MODEL_FIX: &str = " — choose a model with /model";

/// Does `url` name the scheme every non-Ollama wire needs?
fn is_http_url(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
}

/// Where a file that does not parse went wrong, as one row —
/// `line 3, column 10: …`, toml's own message after the colon.
fn syntax_issue(text: &str, err: &toml::de::Error) -> String {
    let message = one_line(err.message());
    let Some(span) = err.span() else {
        return message;
    };
    let mut start = span.start.min(text.len());
    while !text.is_char_boundary(start) {
        start -= 1;
    }
    let before = &text[..start];
    let line = before.matches('\n').count() + 1;
    let column = before
        .rsplit('\n')
        .next()
        .map_or(0, |row| row.chars().count())
        + 1;
    format!("line {line}, column {column}: {message}")
}

/// A toml error, which runs over several lines (the key path it happened in
/// sits on a line of its own), joined into the one row a toast has.
fn one_line(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .filter(|row| !row.is_empty())
        .collect::<Vec<_>>()
        .join(", ")
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
    /// The selected **speed tier**'s wire id (`priority` for codex's fast
    /// mode) — the `/fast` choice, riding the request as `service_tier`.
    /// `None` is standard: no field at all. Only ever set to a tier the
    /// model's own record listed (`docs/fast-mode.md`).
    pub service_tier: Option<String>,
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
    /// The selected speed tier's wire id (see [`Selection::service_tier`]).
    pub service_tier: Option<String>,
    pub extra_headers: Vec<(String, String)>,
    /// Provider kwargs merged into the request body (e.g. `venice_parameters`).
    pub extra_body: serde_json::Map<String, serde_json::Value>,
    /// Request fields the provider's server refuses (see
    /// [`Provider::drop_params`]), taken out by [`Self::drop_refused`].
    pub drop_params: Vec<String>,
}

/// The fields no request goes without, which [`ModelConfig::drop_refused`]
/// never takes out — a body with no model or no stream is no request to make.
const REQUIRED_FIELDS: [&str; 2] = ["model", "stream"];

impl ModelConfig {
    /// Take the fields the provider's server refuses out of a request body —
    /// the last step of every wire's request, after the kwargs and the
    /// thinking mode have been merged in, so whatever put a field there it
    /// does not leave (`docs/user-providers.md`). The model and the stream
    /// flag are never taken out.
    pub fn drop_refused(&self, fields: &mut serde_json::Map<String, serde_json::Value>) {
        for field in &self.drop_params {
            if !REQUIRED_FIELDS.contains(&field.as_str()) {
                fields.remove(field);
            }
        }
    }

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
            service_tier: None,
            extra_headers: Vec::new(),
            extra_body: serde_json::Map::new(),
            drop_params: Vec::new(),
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
    fn openrouter_ships_app_attribution_headers() {
        let file = ProvidersFile::builtin();
        let headers = &file
            .get("openrouter")
            .expect("openrouter present")
            .extra_headers;
        assert_eq!(
            headers.get("HTTP-Referer").map(String::as_str),
            Some("https://github.com/linuztx/alter-zero")
        );
        assert_eq!(
            headers.get("X-OpenRouter-Title").map(String::as_str),
            Some("Alter Zero")
        );
        assert_eq!(
            headers.get("X-OpenRouter-Categories").map(String::as_str),
            Some("cli-agent")
        );
    }

    #[test]
    fn cline_ships_the_public_openai_compatible_api() {
        let file = ProvidersFile::builtin();
        let cline = file.get("cline").expect("cline present");
        assert_eq!(cline.name, "Cline");
        assert_eq!(cline.kwargs.api_base, "https://api.cline.bot/api/v1");
        // The chat base's own `/models` names the models and nothing else, so
        // the picker lists the catalog Cline's clients read instead —
        // `{base}/models` is `/api/v1/ai/cline/models`, whose OpenRouter-shaped
        // records carry each model's window, image input and reasoning
        // (docs/cline.md).
        assert_eq!(cline.models_base(), "https://api.cline.bot/api/v1/ai/cline");
        assert_eq!(cline.key_env("cline"), "CLINE_API_KEY");
        assert_eq!(cline.auth, AuthScheme::ApiKey);
        assert_eq!(cline.wire_api, WireApi::Chat);
        // `/login`'s key step links the page the key is made on.
        assert_eq!(
            cline.api_key_url.as_deref(),
            Some("https://app.cline.bot/dashboard/account")
        );
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

    #[test]
    fn the_builtin_file_ships_venice_direct_beside_the_agent_zero_proxy() {
        // Venice.ai reached two ways over one wire (`docs/venice.md`): through
        // the Agent Zero proxy (`a0_venice`) and directly (`venice`), with a
        // key from the user's own Venice account. Everything the request
        // builder keys on is shared — the chat-completions wire, the
        // pasted-key scheme, and the `venice_parameters` table whose presence
        // is the Venice-family marker (`docs/reasoning.md`) — so the two must
        // agree on all of it; only where the request goes and what unlocks
        // it differ.
        let file = ProvidersFile::builtin();
        let venice = file.get("venice").expect("shipped");
        let proxy = file.get("a0_venice").expect("shipped");
        assert_eq!(venice.name, "Venice");
        assert_eq!(venice.auth, AuthScheme::ApiKey);
        assert_eq!(venice.wire_api, WireApi::Chat);
        assert_eq!(venice.key_env("venice"), "VENICE_API_KEY");
        assert_eq!(venice.kwargs.api_base, "https://api.venice.ai/api/v1");
        // The proxy already lists its models from Venice itself; the direct
        // provider chats where it lists, so the file names no second base.
        assert_eq!(venice.api_model_base, None);
        assert_eq!(venice.models_base(), proxy.models_base());
        assert_eq!(
            venice.api_key_url.as_deref(),
            Some("https://venice.ai/settings/api")
        );
        assert!(
            venice
                .description
                .as_deref()
                .is_some_and(|d| d.contains("Venice")),
            "the key step introduces the provider by name"
        );
        assert_eq!(
            venice.extra_body(),
            proxy.extra_body(),
            "the same venice_parameters table, so the thinking toggle syncs for both"
        );
        assert!(venice.extra_headers.is_empty(), "no proxy identity to send");
    }

    #[test]
    fn a_venice_selection_resolves_to_venice_itself_and_needs_its_key() {
        // Resolved the way the boundary resolves it: the chat and the listing
        // both go to Venice's own base, and — unlike a local Ollama — a key
        // is required, so a keyless selection still falls back to the dummy.
        let file = ProvidersFile::builtin();
        let sel = Selection {
            provider_id: "venice".to_string(),
            model: "qwen3-235b".to_string(),
            api_key: Some("venice-key".to_string()),
            ..Selection::default()
        };
        let cfg = file.model_config(&sel).expect("resolves");
        assert_eq!(cfg.provider_name, "Venice");
        assert_eq!(cfg.api_base, "https://api.venice.ai/api/v1");
        assert_eq!(cfg.api_model_base, "https://api.venice.ai/api/v1");
        assert_eq!(cfg.auth, AuthScheme::ApiKey);
        assert_eq!(cfg.wire_api, WireApi::Chat);
        assert!(cfg.is_usable());
        let keyless = file
            .model_config(&Selection {
                api_key: None,
                ..sel
            })
            .expect("resolves");
        assert!(!keyless.is_usable(), "a Venice key is required");
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
[providers.chatgpt_codex]
name = "ChatGPT Codex"
auth = "chatgpt_codex"
description = "Sign in with your ChatGPT account"
[providers.chatgpt_codex.kwargs]
api_base = "https://chatgpt.com/backend-api/codex"
"#;
        let file = ProvidersFile::parse(text).unwrap();
        let chatgpt = file.get("chatgpt_codex").unwrap();
        assert_eq!(chatgpt.auth, AuthScheme::ChatGptCodex);
        assert!(chatgpt.auth.is_subscription());
        // A provider file written against the old build still spells the
        // scheme `openai_chatgpt`: the alias keeps it a sign-in rather than
        // letting it fall through to a pasted key (docs/chatgpt.md).
        let legacy =
            ProvidersFile::parse(&text.replace("chatgpt_codex", "openai_chatgpt")).unwrap();
        assert_eq!(
            legacy.get("openai_chatgpt").unwrap().auth,
            AuthScheme::ChatGptCodex
        );
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
            provider_id: "chatgpt_codex".to_string(),
            model: "gpt-5.5".to_string(),
            ..Selection::default()
        };
        let cfg = file.model_config(&sel).expect("shipped");
        assert_eq!(cfg.wire_api, WireApi::Responses);
        assert_eq!(cfg.auth, AuthScheme::ChatGptCodex);
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
    fn a_provider_can_declare_the_cline_account_sign_in() {
        // The fourth sign-in: the stored secret is a Cline refresh token, and
        // every request mints the short-lived access token from it — spelled
        // `workos:` when it leaves (`docs/cline.md`). The spelling is pinned
        // because a mismatch degrades *silently* into a pasted-key row.
        let text = r#"
[providers.cline_account]
name = "Cline Account"
auth = "cline_account"
description = "Sign in with your Cline account"
[providers.cline_account.kwargs]
api_base = "https://api.cline.bot/api/v1"
"#;
        let file = ProvidersFile::parse(text).unwrap();
        let account = file.get("cline_account").unwrap();
        assert_eq!(account.auth, AuthScheme::ClineAccount);
        assert!(account.auth.is_subscription());
    }

    #[test]
    fn every_key_the_shipped_file_sets_is_one_the_parse_reads() {
        // Unknown keys are ignored, so a file written for a newer build still
        // parses — which also means a key the shipped file sets and nothing
        // reads does nothing, silently: a misspelt `api_key_ev`, or a key a
        // refactor retired, reads as configuration while changing nothing.
        // The parse keeps what it did not read (`Provider::unread`), so the
        // struct itself is the list of keys — none to keep in step by hand.
        let file = ProvidersFile::builtin();
        assert!(!file.ids().is_empty());
        for id in file.ids() {
            let provider = file.get(&id).expect("listed");
            let unread: Vec<&String> = provider.unread.keys().collect();
            assert!(
                unread.is_empty(),
                "providers.{id} sets {unread:?}, which the parse never reads"
            );
            assert!(
                !provider.name.trim().is_empty(),
                "providers.{id} has no name"
            );
        }
    }

    #[test]
    fn a_key_the_parse_does_not_read_is_kept_aside_rather_than_dropped() {
        // What lets a user's own file be told about a typo: the block still
        // reads, and the key it did not understand is named.
        let file = ProvidersFile::parse(
            "[providers.p]\nname = \"P\"\ndescripton = \"typo\"\n[providers.p.kwargs]\napi_base = \"https://x/v1\"\n",
        )
        .unwrap();
        let provider = file.get("p").unwrap();
        assert_eq!(provider.description, None);
        assert_eq!(
            provider.unread.keys().collect::<Vec<_>>(),
            ["descripton"],
            "the misspelt key is named"
        );
        // A `kwargs` extra is a body parameter, not an unread key.
        assert!(!provider.unread.contains_key("api_base"));
    }

    #[test]
    fn the_builtin_file_ships_cline_pass_as_the_one_cline_sign_in() {
        // ClinePass is the Cline account sign-in: a subscription, whose
        // scheme is also what lists the plan's `cline-pass/…` models from
        // the list Cline publishes, since the public `/models` does not
        // carry them (`docs/cline.md`).
        let file = ProvidersFile::builtin();
        let pass = file.get("cline_pass").expect("shipped");
        assert_eq!(pass.name, "ClinePass");
        assert_eq!(pass.auth, AuthScheme::ClineAccount);
        assert_eq!(pass.key_env("cline_pass"), "CLINE_ACCOUNT_REFRESH_TOKEN");
        assert!(pass.description.is_some(), "a sign-in row needs one");
        assert_eq!(pass.kwargs.api_base, "https://api.cline.bot/api/v1");
        // And it is the only Cline sign-in. A second row running the same
        // flow for the usage-billed catalog was the same sign-in twice under
        // "Use a subscription", for models that bill credits rather than a
        // plan — those are the pasted key's (`cline`), and a row with this
        // scheme lists the plan's.
        let cline_sign_ins: Vec<String> = file
            .ids()
            .into_iter()
            .filter(|id| file.get(id).unwrap().auth == AuthScheme::ClineAccount)
            .collect();
        assert_eq!(cline_sign_ins, ["cline_pass"]);
        assert_eq!(file.get("cline").unwrap().auth, AuthScheme::ApiKey);
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
            service_tier: None,
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
            service_tier: None,
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
    fn no_two_shipped_providers_read_the_same_key_variable() {
        // One provider, one variable. Two that share an `api_key_env` share a
        // credential store: a key pasted for either marks *both* configured —
        // the ✓ in `/login` and the set `/model` fetches from — so signing in
        // to Ollama Cloud used to light the local Ollama row and send every
        // `/model` open to a server most users don't run. The collision is a
        // property of the shipped file, so it is pinned here.
        let file = ProvidersFile::builtin();
        let mut owner: std::collections::BTreeMap<String, String> =
            std::collections::BTreeMap::new();
        for id in file.ids() {
            let var = file.get(&id).expect("listed").key_env(&id);
            if let Some(first) = owner.insert(var.clone(), id.clone()) {
                panic!("{first} and {id} both read {var}");
            }
        }
    }

    #[test]
    fn the_builtin_file_ships_ollama_local_and_cloud() {
        let file = ProvidersFile::builtin();
        let local = file.get("ollama").expect("shipped");
        assert_eq!(local.wire_api, WireApi::Ollama);
        assert_eq!(local.auth, AuthScheme::OptionalKey);
        // The server's *own* key — the one a proxy in front of it may want —
        // and deliberately not the cloud's `OLLAMA_API_KEY`: sharing that one
        // made a pasted cloud key configure this provider too.
        assert_eq!(local.key_env("ollama"), "OLLAMA_HOST_API_KEY");
        assert_eq!(local.api_base_env.as_deref(), Some("OLLAMA_HOST"));
        assert_eq!(local.kwargs.api_base, "http://127.0.0.1:11434");

        let cloud = file.get("ollama_cloud").expect("shipped");
        assert_eq!(cloud.wire_api, WireApi::Ollama);
        assert_eq!(cloud.auth, AuthScheme::ApiKey, "the cloud needs its key");
        // Ollama's own name for the hosted API's key, so a key already
        // exported for the `ollama` CLI configures this row and nothing else.
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

    #[test]
    fn the_selection_carries_its_speed_tier_into_the_config() {
        // The `/fast` choice rides the request the way the thinking mode
        // does (docs/fast-mode.md): `None` is standard — no field at all.
        let file = ProvidersFile::builtin();
        let sel = Selection {
            provider_id: "chatgpt_codex".to_string(),
            model: "gpt-5.5".to_string(),
            service_tier: Some("priority".to_string()),
            ..Selection::default()
        };
        let cfg = file.model_config(&sel).expect("a built-in provider");
        assert_eq!(cfg.service_tier.as_deref(), Some("priority"));
        assert_eq!(ModelConfig::fallback().service_tier, None);
        let standard = file
            .model_config(&Selection {
                service_tier: None,
                ..sel
            })
            .unwrap();
        assert_eq!(standard.service_tier, None);
    }

    // --- a user's own providers.toml, laid over the built-ins (docs/user-providers.md) ---

    /// The smallest whole provider a user writes: a name and a chat base.
    const GROQ: &str = r#"
[providers.groq]
name = "Groq"
description = "Fast inference on open models."
api_key_url = "https://console.groq.com/keys"

[providers.groq.kwargs]
api_base = "https://api.groq.com/openai/v1"
"#;

    #[test]
    fn a_user_file_adds_its_providers_beside_the_builtins() {
        let (file, issues) = ProvidersFile::with_user_layer(GROQ);
        assert_eq!(issues, Vec::<String>::new());
        let groq = file
            .get("groq")
            .expect("the user's provider is in the table");
        assert_eq!(groq.name, "Groq");
        assert_eq!(groq.kwargs.api_base, "https://api.groq.com/openai/v1");
        assert_eq!(groq.key_env("groq"), "GROQ_API_KEY");
        assert_eq!(groq.auth, AuthScheme::ApiKey);
        assert_eq!(groq.wire_api, WireApi::Chat);
        assert_eq!(
            groq.api_key_url.as_deref(),
            Some("https://console.groq.com/keys")
        );
        // The file adds; it does not replace — every shipped provider stays.
        let builtin = ProvidersFile::builtin();
        for id in builtin.ids() {
            assert!(file.get(&id).is_some(), "{id} went missing");
        }
        assert_eq!(file.ids().len(), builtin.ids().len() + 1);
        // And it resolves like any shipped provider: chat and listing at the
        // one base, usable once its key is there.
        let cfg = file
            .model_config(&Selection {
                provider_id: "groq".to_string(),
                model: "llama-3.3-70b-versatile".to_string(),
                api_key: Some("gsk_test".to_string()),
                ..Selection::default()
            })
            .expect("resolves");
        assert_eq!(cfg.provider_name, "Groq");
        assert_eq!(cfg.api_base, "https://api.groq.com/openai/v1");
        assert_eq!(cfg.api_model_base, "https://api.groq.com/openai/v1");
        assert!(cfg.is_usable());
    }

    #[test]
    fn an_empty_or_comment_only_user_file_is_the_builtin_table() {
        for text in ["", "# nothing here yet\n"] {
            let (file, issues) = ProvidersFile::with_user_layer(text);
            assert_eq!(issues, Vec::<String>::new());
            assert_eq!(file.ids(), ProvidersFile::builtin().ids());
        }
    }

    #[test]
    fn a_user_block_naming_a_shipped_provider_patches_only_what_it_sets() {
        let (file, issues) = ProvidersFile::with_user_layer(
            r#"
[providers.openrouter.extra_headers]
X-Team = "research"

[providers.venice.kwargs.venice_parameters]
enable_web_search = "auto"
"#,
        );
        assert_eq!(issues, Vec::<String>::new());
        let builtin = ProvidersFile::builtin();
        let openrouter = file.get("openrouter").unwrap();
        assert_eq!(
            openrouter.extra_headers.get("X-Team").map(String::as_str),
            Some("research")
        );
        // Beside the shipped attribution headers, which stay.
        assert_eq!(
            openrouter.extra_headers.get("HTTP-Referer"),
            builtin
                .get("openrouter")
                .unwrap()
                .extra_headers
                .get("HTTP-Referer")
        );
        assert_eq!(openrouter.name, "OpenRouter");
        assert_eq!(openrouter.kwargs.api_base, "https://openrouter.ai/api/v1");
        // A nested kwargs table merges key by key: the shipped
        // `include_venice_system_prompt = false` stays beside the addition.
        let body = file.get("venice").unwrap().extra_body();
        let params = &body["venice_parameters"];
        assert_eq!(
            params["include_venice_system_prompt"],
            serde_json::json!(false)
        );
        assert_eq!(params["enable_web_search"], serde_json::json!("auto"));
    }

    #[test]
    fn a_user_value_replaces_the_shipped_one_and_leaves_the_rest() {
        let (file, issues) = ProvidersFile::with_user_layer(
            r#"
[providers.openrouter]
name = "OpenRouter (work)"

[providers.openrouter.kwargs]
api_base = "https://proxy.example/v1"
"#,
        );
        assert_eq!(issues, Vec::<String>::new());
        let openrouter = file.get("openrouter").unwrap();
        assert_eq!(openrouter.name, "OpenRouter (work)");
        assert_eq!(openrouter.kwargs.api_base, "https://proxy.example/v1");
        // What the block did not set is still the shipped value.
        assert_eq!(
            openrouter.api_model_base.as_deref(),
            Some("https://openrouter.ai/api/v1")
        );
        assert_eq!(openrouter.key_env("openrouter"), "OPENROUTER_API_KEY");
    }

    #[test]
    fn a_user_file_that_does_not_parse_leaves_the_builtins_and_says_where() {
        let text = "[providers.groq]\nname = \"Groq\"\napi_base https://x\n";
        let (file, issues) = ProvidersFile::with_user_layer(text);
        assert_eq!(file.ids(), ProvidersFile::builtin().ids());
        assert_eq!(issues.len(), 1, "{issues:?}");
        assert!(issues[0].starts_with("line 3, column "), "{issues:?}");
        assert!(
            issues[0].ends_with(" — using the built-in providers"),
            "{issues:?}"
        );
        assert!(!issues[0].contains('\n'), "one row: {issues:?}");
    }

    #[test]
    fn a_new_provider_without_a_name_is_skipped_and_said() {
        let (file, issues) = ProvidersFile::with_user_layer(
            "[providers.groq.kwargs]\napi_base = \"https://api.groq.com/openai/v1\"\n",
        );
        assert!(file.get("groq").is_none());
        assert_eq!(issues, ["providers.groq has no name — skipped"]);
        // A blank one is no name either.
        let (file, issues) = ProvidersFile::with_user_layer(
            "[providers.groq]\nname = \"  \"\n[providers.groq.kwargs]\napi_base = \"https://x/v1\"\n",
        );
        assert!(file.get("groq").is_none());
        assert_eq!(issues, ["providers.groq has no name — skipped"]);
    }

    #[test]
    fn a_new_provider_without_a_chat_base_is_skipped_and_said() {
        let (file, issues) = ProvidersFile::with_user_layer("[providers.groq]\nname = \"Groq\"\n");
        assert!(file.get("groq").is_none());
        assert_eq!(
            issues,
            ["providers.groq has no api_base under [providers.groq.kwargs] — skipped"]
        );
    }

    #[test]
    fn a_host_variable_stands_in_for_the_chat_base() {
        // A provider whose base only the environment names is complete: the
        // boundary resolves the variable the way it resolves `OLLAMA_HOST`.
        let (file, issues) = ProvidersFile::with_user_layer(
            "[providers.lab]\nname = \"Lab\"\napi_base_env = \"LAB_HOST\"\n",
        );
        assert_eq!(issues, Vec::<String>::new());
        assert!(file.get("lab").is_some());
    }

    #[test]
    fn a_chat_base_set_beside_the_name_is_told_where_it_belongs() {
        // The likeliest slip: the base at the top of the block, where codex's
        // `base_url` sits — a provider that would otherwise just never work.
        for key in ["api_base", "base_url"] {
            let text = format!(
                "[providers.groq]\nname = \"Groq\"\n{key} = \"https://api.groq.com/openai/v1\"\n"
            );
            let (file, issues) = ProvidersFile::with_user_layer(&text);
            assert!(file.get("groq").is_none());
            assert_eq!(
                issues,
                [
                    format!(
                        "providers.groq.{key} is not a known key — set api_base under [providers.groq.kwargs]"
                    ),
                    "providers.groq has no api_base under [providers.groq.kwargs] — skipped"
                        .to_string(),
                ]
            );
        }
    }

    #[test]
    fn an_unknown_key_is_said_and_the_provider_still_loads() {
        let (file, issues) = ProvidersFile::with_user_layer(
            r#"
[providers.groq]
name = "Groq"
descripton = "typo"

[providers.groq.kwargs]
api_base = "https://api.groq.com/openai/v1"
"#,
        );
        assert!(
            file.get("groq").is_some(),
            "a typo costs the key, not the provider"
        );
        assert_eq!(issues, ["providers.groq.descripton is not a known key"]);
    }

    #[test]
    fn a_key_other_tools_spell_differently_is_told_this_files_name() {
        let cases = [
            (
                "env_key = \"GROQ_KEY\"",
                "providers.groq.env_key is not a known key — name the key's variable with api_key_env",
            ),
            (
                "http_headers = { X-Team = \"a\" }",
                "providers.groq.http_headers is not a known key — set headers under [providers.groq.extra_headers]",
            ),
            (
                "headers = { X-Team = \"a\" }",
                "providers.groq.headers is not a known key — set headers under [providers.groq.extra_headers]",
            ),
            (
                "api_key = \"gsk_live\"",
                "providers.groq.api_key is not a known key — keys are not read from this file: use /login",
            ),
            (
                "model = \"llama-3.3-70b-versatile\"",
                "providers.groq.model is not a known key — choose a model with /model",
            ),
        ];
        for (line, expected) in cases {
            let text = format!(
                "[providers.groq]\nname = \"Groq\"\n{line}\n[providers.groq.kwargs]\napi_base = \"https://api.groq.com/openai/v1\"\n"
            );
            let (file, issues) = ProvidersFile::with_user_layer(&text);
            assert!(file.get("groq").is_some(), "{line}");
            assert_eq!(issues, [expected]);
            // A key pasted into the file is never echoed back.
            assert!(!issues[0].contains("gsk_live"));
        }
    }

    #[test]
    fn a_wrongly_typed_block_costs_only_itself() {
        let text = format!(
            r#"
[providers.broken]
name = "Broken"
extra_headers = "not a table"

[providers.broken.kwargs]
api_base = "https://broken.example/v1"
{GROQ}"#
        );
        let (file, issues) = ProvidersFile::with_user_layer(&text);
        assert!(file.get("broken").is_none());
        assert!(file.get("groq").is_some(), "the next block still loads");
        assert_eq!(issues.len(), 1, "{issues:?}");
        assert!(issues[0].starts_with("providers.broken: "), "{issues:?}");
        assert!(
            issues[0].contains("extra_headers"),
            "names the key: {issues:?}"
        );
        assert!(issues[0].ends_with(" — skipped"), "{issues:?}");
        assert!(!issues[0].contains('\n'), "one row: {issues:?}");
    }

    #[test]
    fn a_broken_patch_keeps_the_shipped_provider() {
        let (file, issues) =
            ProvidersFile::with_user_layer("[providers.openrouter]\nextra_headers = \"x\"\n");
        let builtin = ProvidersFile::builtin();
        let openrouter = file.get("openrouter").expect("kept");
        assert_eq!(
            openrouter.extra_headers,
            builtin.get("openrouter").unwrap().extra_headers
        );
        assert_eq!(issues.len(), 1, "{issues:?}");
        assert!(
            issues[0].starts_with("providers.openrouter: "),
            "{issues:?}"
        );
        assert!(issues[0].ends_with(" — kept the built-in"), "{issues:?}");
    }

    #[test]
    fn a_block_that_is_not_a_table_is_said() {
        let (file, issues) = ProvidersFile::with_user_layer("[providers]\ngroq = \"x\"\n");
        assert!(file.get("groq").is_none());
        assert_eq!(issues, ["providers.groq is not a table — skipped"]);
    }

    #[test]
    fn a_base_with_no_http_scheme_is_skipped_and_said() {
        let (file, issues) = ProvidersFile::with_user_layer(
            "[providers.groq]\nname = \"Groq\"\n[providers.groq.kwargs]\napi_base = \"api.groq.com/openai/v1\"\n",
        );
        assert!(file.get("groq").is_none());
        assert_eq!(
            issues,
            [
                "providers.groq.kwargs.api_base \"api.groq.com/openai/v1\" is not an http(s) URL — skipped"
            ]
        );
        let (file, issues) = ProvidersFile::with_user_layer(
            "[providers.groq]\nname = \"Groq\"\napi_model_base = \"models.example\"\n[providers.groq.kwargs]\napi_base = \"https://x/v1\"\n",
        );
        assert!(file.get("groq").is_none());
        assert_eq!(
            issues,
            ["providers.groq.api_model_base \"models.example\" is not an http(s) URL — skipped"]
        );
    }

    #[test]
    fn an_ollama_host_needs_no_scheme() {
        // Ollama's own grammar (`docs/ollama.md`): `gpu-box:11434` is a host.
        let (file, issues) = ProvidersFile::with_user_layer(
            "[providers.gpu]\nname = \"GPU box\"\nwire_api = \"ollama\"\nauth = \"optional_key\"\n[providers.gpu.kwargs]\napi_base = \"gpu-box:11434\"\n",
        );
        assert_eq!(issues, Vec::<String>::new());
        assert!(file.get("gpu").is_some());
    }

    #[test]
    fn an_unknown_auth_or_wire_format_in_a_user_block_is_said() {
        // Both still degrade to the default (a file from a newer build must
        // parse), but a user's own typo is said rather than silently becoming
        // a pasted-key chat-completions provider.
        let (file, issues) = ProvidersFile::with_user_layer(
            r#"
[providers.groq]
name = "Groq"
auth = "bearer"
wire_api = "chat_completions"

[providers.groq.kwargs]
api_base = "https://api.groq.com/openai/v1"
"#,
        );
        let groq = file.get("groq").expect("still loads, on the defaults");
        assert_eq!(groq.auth, AuthScheme::ApiKey);
        assert_eq!(groq.wire_api, WireApi::Chat);
        assert_eq!(
            issues,
            [
                "providers.groq.auth = \"bearer\" is not known — using a pasted key",
                "providers.groq.wire_api = \"chat_completions\" is not known — using chat completions",
            ]
        );
    }

    #[test]
    fn the_defaults_own_spellings_are_not_unknown() {
        let (file, issues) = ProvidersFile::with_user_layer(
            "[providers.groq]\nname = \"Groq\"\nauth = \"api_key\"\nwire_api = \"chat\"\n[providers.groq.kwargs]\napi_base = \"https://x/v1\"\n",
        );
        assert_eq!(issues, Vec::<String>::new());
        assert_eq!(file.get("groq").unwrap().auth, AuthScheme::ApiKey);
    }

    #[test]
    fn a_provider_written_without_the_providers_prefix_is_told_where_it_goes() {
        let (file, issues) = ProvidersFile::with_user_layer(
            "model = \"x\"\n[groq]\nname = \"Groq\"\n[groq.kwargs]\napi_base = \"https://api.groq.com/openai/v1\"\n",
        );
        assert!(file.get("groq").is_none());
        assert_eq!(
            issues,
            [
                "[groq] is not read — a provider goes under [providers.groq]",
                "model is not a known key — choose a model with /model",
            ]
        );
    }

    #[test]
    fn a_table_that_is_no_provider_block_is_told_where_providers_go() {
        // `[provider.groq]` (or codex's `[model_providers.groq]`) is a table
        // of providers under the wrong name, not a provider called
        // `provider` — so the row must not suggest `[providers.provider]`.
        for table in ["provider", "model_providers"] {
            let text = format!(
                "[{table}.groq]\nname = \"Groq\"\n[{table}.groq.kwargs]\napi_base = \"https://x/v1\"\n"
            );
            let (file, issues) = ProvidersFile::with_user_layer(&text);
            assert!(file.get("groq").is_none());
            assert_eq!(
                issues,
                [format!(
                    "[{table}] is not read — providers go under [providers.<id>]"
                )]
            );
        }
    }

    #[test]
    fn a_providers_key_that_is_not_a_table_leaves_the_builtins() {
        let (file, issues) = ProvidersFile::with_user_layer("providers = 3\n");
        assert_eq!(file.ids(), ProvidersFile::builtin().ids());
        assert_eq!(
            issues,
            ["providers is not a table — using the built-in providers"]
        );
    }

    #[test]
    fn a_standalone_file_is_the_whole_table() {
        // `ALTER_ZERO_PROVIDERS_FILE`'s "only these" — what keeps a smoke run
        // from listing every shipped provider beside its stub.
        let (file, issues) = ProvidersFile::standalone(GROQ);
        assert_eq!(issues, Vec::<String>::new());
        assert_eq!(file.ids(), ["groq"]);
        // Its blocks are checked like a layer's.
        let (file, issues) = ProvidersFile::standalone("[providers.groq]\nname = \"Groq\"\n");
        assert!(file.ids().is_empty());
        assert_eq!(
            issues,
            ["providers.groq has no api_base under [providers.groq.kwargs] — skipped"]
        );
    }

    #[test]
    fn a_standalone_file_that_does_not_parse_falls_back_to_the_builtins() {
        let (file, issues) = ProvidersFile::standalone("this is not = = toml");
        assert_eq!(file.ids(), ProvidersFile::builtin().ids());
        assert_eq!(issues.len(), 1, "{issues:?}");
        assert!(
            issues[0].ends_with(" — using the built-in providers"),
            "{issues:?}"
        );
    }

    #[test]
    fn a_keyless_provider_naming_no_host_variable_is_configured_by_its_definition() {
        // A local LM Studio / llama.cpp / vLLM server takes no key and has no
        // host variable to resolve: the file pointing at it is the
        // configuration, so `/model` lists it without a trip through `/login`.
        let (file, issues) = ProvidersFile::with_user_layer(
            r#"
[providers.lmstudio]
name = "LM Studio"
auth = "optional_key"

[providers.lmstudio.kwargs]
api_base = "http://localhost:1234/v1"
"#,
        );
        assert_eq!(issues, Vec::<String>::new());
        assert!(file.get("lmstudio").unwrap().configured_by_definition());
        // A pasted key is still needed for a keyed provider, and Ollama is
        // pointed at through `OLLAMA_HOST` (docs/ollama.md).
        assert!(!file.get("openrouter").unwrap().configured_by_definition());
        assert!(!file.get("ollama").unwrap().configured_by_definition());
    }

    #[test]
    fn no_shipped_provider_is_configured_by_its_definition() {
        // Otherwise every `/model` open would fetch from a server the user
        // never pointed the app at.
        let file = ProvidersFile::builtin();
        for id in file.ids() {
            assert!(
                !file.get(&id).unwrap().configured_by_definition(),
                "{id} would be fetched from unasked"
            );
        }
    }

    // --- request fields a strict server refuses (docs/user-providers.md) ---

    #[test]
    fn a_provider_can_name_the_request_fields_its_server_refuses() {
        // Every request carries `stream_options` and `prompt_cache_key`, and
        // a strict server answers a field it does not know with an error
        // (Mistral's "Extra inputs are not permitted"). TOML has no null, so
        // a kwarg can replace such a field but never remove it.
        let (file, issues) = ProvidersFile::with_user_layer(
            r#"
[providers.mistral]
name = "Mistral"
drop_params = ["prompt_cache_key", "stream_options"]

[providers.mistral.kwargs]
api_base = "https://api.mistral.ai/v1"
"#,
        );
        assert_eq!(issues, Vec::<String>::new());
        let cfg = file
            .model_config(&Selection {
                provider_id: "mistral".to_string(),
                model: "mistral-large-latest".to_string(),
                ..Selection::default()
            })
            .expect("resolves");
        assert_eq!(cfg.drop_params, ["prompt_cache_key", "stream_options"]);
        assert!(ModelConfig::fallback().drop_params.is_empty());
        // No shipped provider drops anything.
        let builtin = ProvidersFile::builtin();
        for id in builtin.ids() {
            assert!(builtin.get(&id).unwrap().drop_params.is_empty(), "{id}");
        }
    }

    #[test]
    fn dropping_takes_out_the_named_fields_and_never_the_ones_a_request_needs() {
        let mut fields = serde_json::Map::new();
        for key in ["model", "stream", "prompt_cache_key", "tools"] {
            fields.insert(key.to_string(), serde_json::json!(true));
        }
        let mut cfg = ModelConfig::fallback();
        cfg.drop_params = ["prompt_cache_key", "model", "stream", "absent"]
            .map(str::to_string)
            .to_vec();
        cfg.drop_refused(&mut fields);
        assert_eq!(
            fields.keys().collect::<Vec<_>>(),
            ["model", "stream", "tools"]
        );
    }

    #[test]
    fn dropping_a_field_every_request_needs_is_said() {
        let (file, issues) = ProvidersFile::with_user_layer(
            "[providers.x]\nname = \"X\"\ndrop_params = [\"stream\", \"prompt_cache_key\"]\n[providers.x.kwargs]\napi_base = \"https://x/v1\"\n",
        );
        assert!(
            file.get("x").is_some(),
            "the rest of the list still applies"
        );
        assert_eq!(
            issues,
            ["providers.x.drop_params cannot drop \"stream\" — every request needs it"]
        );
    }
}
