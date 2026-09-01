//! What a request authenticates with, and where it goes — the one seam every
//! outbound call resolves through, whatever the provider's [`AuthScheme`].
//!
//! Three providers here are *sign-ins* rather than pasted keys, and all three
//! keep the same two-layer split: a long-lived token in the `.env` key store,
//! exchanged (and cached in memory) for the short-lived credential the API
//! actually takes. GitHub Copilot's exchange additionally names the account's
//! own host; OpenAI's additionally names the account the backend routes on,
//! and Anthropic's the beta its bearer must carry — all as headers. So the
//! seam answers three things, not one, and the third is what a
//! `(bearer, base)` pair could not carry.
//!
//! It answers a fourth by implication: **where** the credential goes. Every
//! provider but one puts it on `Authorization: Bearer`; Anthropic's Messages
//! API puts a pasted key on `x-api-key` and refuses a request carrying both.
//! That is a property of the (scheme, wire format) *pair*, which is why it is
//! decided here rather than by either alone.
//!
//! See `docs/copilot.md`, `docs/chatgpt.md` and `docs/claude.md`.

use super::config::{AuthScheme, ModelConfig, WireApi};
use super::{Result, chatgpt, claude, copilot};

/// How one request authenticates itself.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RequestAuth {
    /// The `Authorization: Bearer` value, when there is one. `None` for a
    /// provider that was never signed in to — the `/model` picker builds
    /// configs for those, and a request is simply not made.
    pub bearer: Option<String>,
    /// A base URL that **outranks** the configured one: a Copilot
    /// Business/Enterprise seat is served from a host `providers.toml` cannot
    /// know. `None` leaves the file's base in force.
    pub base: Option<String>,
    /// Headers the credential itself implies — the account the request is
    /// made on behalf of, and the client identity that account expects.
    /// Merged on top of the provider's static `extra_headers`.
    pub headers: Vec<(String, String)>,
}

impl RequestAuth {
    /// The plain case: a stored key, the configured base, nothing implied.
    fn key(api_key: Option<String>) -> Self {
        Self {
            bearer: api_key,
            base: None,
            headers: Vec::new(),
        }
    }

    /// A pasted key that rides a **header of its own** rather than
    /// `Authorization: Bearer`. Anthropic's Messages API takes an API key as
    /// `x-api-key` and reserves the bearer for OAuth — sending both is
    /// refused outright, so the two are alternatives, not a fallback pair.
    fn keyed_header(name: &str, api_key: Option<String>) -> Self {
        let headers = api_key
            .filter(|k| !k.is_empty())
            .map(|key| vec![(name.to_string(), key)])
            .unwrap_or_default();
        Self {
            bearer: None,
            base: None,
            headers,
        }
    }
}

/// Resolve `cfg` into what its next request needs.
///
/// - **[`AuthScheme::ApiKey`]** → the stored key and nothing else, with **no
///   I/O at all**. Every ordinary provider's path is byte-identical to what
///   it was before the subscriptions existed.
/// - **[`AuthScheme::GithubCopilot`]** → the exchanged Copilot bearer and the
///   account's own host (`docs/copilot.md`).
/// - **[`AuthScheme::OpenAiChatGpt`]** → an access token minted from the
///   stored refresh token, plus the `chatgpt-account-id` the backend routes
///   on (`docs/chatgpt.md`).
/// - **[`AuthScheme::AnthropicConsole`]** → an access token minted from the
///   stored refresh token, plus the `anthropic-beta` an OAuth bearer must
///   carry on the Messages API (`docs/claude.md`).
///
/// A subscription config with no stored token resolves to an empty
/// [`RequestAuth`] rather than calling out: the `/model` picker builds configs
/// for providers that were never signed in to, and an exchange attempt there
/// would hang the fetch on a request that can only fail.
///
/// # Errors
/// An exchange or refresh the provider refused becomes an
/// [`LlmError`](super::LlmError); [`AuthScheme::ApiKey`] is infallible.
pub(crate) fn request_auth(cfg: &ModelConfig) -> Result<RequestAuth> {
    let stored = cfg.api_key.as_deref().filter(|k| !k.is_empty());
    match cfg.auth {
        // The Messages API is the one wire format whose key is not a bearer.
        // It is the *pair* that decides, not either half: an Anthropic key
        // reaching a chat-completions shim is still a bearer, and an OpenAI
        // key never becomes an `x-api-key`.
        AuthScheme::ApiKey if cfg.wire_api == WireApi::Anthropic => {
            Ok(RequestAuth::keyed_header("x-api-key", cfg.api_key.clone()))
        }
        AuthScheme::ApiKey => Ok(RequestAuth::key(cfg.api_key.clone())),
        AuthScheme::AnthropicConsole => {
            let Some(refresh) = stored else {
                return Ok(RequestAuth::default());
            };
            let access = claude::authorize(refresh)?;
            Ok(RequestAuth {
                bearer: Some(access.bearer),
                base: None,
                // An OAuth bearer is refused on `/v1/messages` without this;
                // a pasted key is refused *with* it. It belongs to the
                // credential, which is why it is here and not in the file.
                headers: vec![("anthropic-beta".to_string(), claude::OAUTH_BETA.to_string())],
            })
        }
        AuthScheme::GithubCopilot => {
            let Some(oauth) = stored else {
                return Ok(RequestAuth::default());
            };
            let auth = copilot::authorize(oauth)?;
            Ok(RequestAuth {
                bearer: Some(auth.bearer),
                base: Some(auth.api_base),
                headers: Vec::new(),
            })
        }
        AuthScheme::OpenAiChatGpt => {
            let Some(refresh) = stored else {
                return Ok(RequestAuth::default());
            };
            let access = chatgpt::authorize(refresh)?;
            let mut headers = vec![("user-agent".to_string(), chatgpt::user_agent().to_string())];
            // Absent on a token that carries no auth claims — attach the
            // header conditionally rather than sending an empty account id,
            // which the backend reads as a *different* (invalid) routing hint
            // than sending none.
            if let Some(account) = &access.account_id {
                headers.push(("chatgpt-account-id".to_string(), account.clone()));
            }
            if access.fedramp {
                headers.push(("x-openai-fedramp".to_string(), "true".to_string()));
            }
            Ok(RequestAuth {
                bearer: Some(access.bearer),
                base: None,
                headers,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_api_key_provider_resolves_to_its_stored_key_with_no_io() {
        let mut cfg = ModelConfig::fallback();
        cfg.api_key = Some("sk-test".to_string());
        let auth = request_auth(&cfg).unwrap();
        assert_eq!(auth.bearer.as_deref(), Some("sk-test"));
        assert_eq!(auth.base, None, "the configured base stands");
        assert!(auth.headers.is_empty(), "a key implies no identity");
    }

    #[test]
    fn an_anthropic_key_rides_x_api_key_rather_than_a_bearer() {
        // The Messages API reserves `Authorization` for OAuth and refuses a
        // request that sends both.
        let mut cfg = ModelConfig::fallback();
        cfg.wire_api = super::super::WireApi::Anthropic;
        cfg.api_key = Some("sk-ant-test".to_string());
        let auth = request_auth(&cfg).unwrap();
        assert_eq!(auth.bearer, None, "never both");
        assert_eq!(
            auth.headers,
            vec![("x-api-key".to_string(), "sk-ant-test".to_string())]
        );
        // The pair decides: the same key on a chat-completions provider is a
        // bearer exactly as it always was.
        cfg.wire_api = super::super::WireApi::Chat;
        let auth = request_auth(&cfg).unwrap();
        assert_eq!(auth.bearer.as_deref(), Some("sk-ant-test"));
        assert!(auth.headers.is_empty());
    }

    #[test]
    fn an_anthropic_provider_with_no_key_sends_no_header_at_all() {
        // An empty `x-api-key` is a *different* (invalid) credential than
        // none — the `/model` picker builds configs for unkeyed providers.
        let mut cfg = ModelConfig::fallback();
        cfg.wire_api = super::super::WireApi::Anthropic;
        cfg.api_key = None;
        assert_eq!(request_auth(&cfg).unwrap(), RequestAuth::default());
        cfg.api_key = Some(String::new());
        assert_eq!(request_auth(&cfg).unwrap(), RequestAuth::default());
    }

    #[test]
    fn a_subscription_with_no_stored_token_resolves_to_nothing_rather_than_calling_out() {
        // The `/model` picker builds a config per provider, signed in or not.
        // An exchange attempt on an unsigned-in one would hang the fetch on a
        // request that can only fail.
        for scheme in [
            AuthScheme::GithubCopilot,
            AuthScheme::OpenAiChatGpt,
            AuthScheme::AnthropicConsole,
        ] {
            let mut cfg = ModelConfig::fallback();
            cfg.auth = scheme;
            cfg.api_key = None;
            assert_eq!(request_auth(&cfg).unwrap(), RequestAuth::default());
            cfg.api_key = Some(String::new());
            assert_eq!(request_auth(&cfg).unwrap(), RequestAuth::default());
        }
    }
}
