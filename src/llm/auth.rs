//! What a request authenticates with, and where it goes — the one seam every
//! outbound call resolves through, whatever the provider's [`AuthScheme`].
//!
//! Two providers here are *subscriptions* rather than pasted keys, and both
//! keep the same two-layer split: a long-lived token in the `.env` key store,
//! exchanged (and cached in memory) for the short-lived credential the API
//! actually takes. GitHub Copilot's exchange additionally names the account's
//! own host; OpenAI's additionally names the account the backend routes on,
//! as a header. So the seam answers three things, not one — and the third is
//! what a `(bearer, base)` pair could not carry.
//!
//! See `docs/copilot.md` and `docs/chatgpt.md`.

use super::config::{AuthScheme, ModelConfig};
use super::{Result, chatgpt, copilot};

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
        AuthScheme::ApiKey => Ok(RequestAuth::key(cfg.api_key.clone())),
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
    fn a_subscription_with_no_stored_token_resolves_to_nothing_rather_than_calling_out() {
        // The `/model` picker builds a config per provider, signed in or not.
        // An exchange attempt on an unsigned-in one would hang the fetch on a
        // request that can only fail.
        for scheme in [AuthScheme::GithubCopilot, AuthScheme::OpenAiChatGpt] {
            let mut cfg = ModelConfig::fallback();
            cfg.auth = scheme;
            cfg.api_key = None;
            assert_eq!(request_auth(&cfg).unwrap(), RequestAuth::default());
            cfg.api_key = Some(String::new());
            assert_eq!(request_auth(&cfg).unwrap(), RequestAuth::default());
        }
    }
}
