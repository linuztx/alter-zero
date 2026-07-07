//! Persisted UI settings — currently just the last provider/model chosen via
//! `/model`, so the selection survives a restart. Written to
//! `~/.inline-tui/config.json` by the boundary; the parse/serialize here is
//! **pure and unit-tested** (the file read/write lives in `main.rs`, like the
//! `.env` keystore). See `docs/llm.md`.

use serde::{Deserialize, Serialize};

/// The persisted settings blob. Every field is optional so an old, partial, or
/// future file still loads — a missing key just leaves that setting unset, and
/// unknown keys are ignored.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    /// The last provider id chosen via `/model` (e.g. `openrouter`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// The last model id chosen via `/model`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

impl Settings {
    /// Parse a `config.json` body, best-effort: malformed or empty JSON yields
    /// the default (all-unset) settings rather than an error, so a corrupt file
    /// never blocks startup.
    #[must_use]
    pub fn parse(text: &str) -> Self {
        serde_json::from_str(text).unwrap_or_default()
    }

    /// Serialize to pretty JSON for writing back to `config.json`. A plain
    /// struct of strings can't really fail to serialize; an impossible failure
    /// falls back to `{}`.
    #[must_use]
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".to_string())
    }

    /// The settings recording a `/model` selection.
    #[must_use]
    pub fn for_selection(provider: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            provider: Some(provider.into()),
            model: Some(model.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_reads_provider_and_model() {
        let s =
            Settings::parse(r#"{"provider":"openrouter","model":"anthropic/claude-3.5-haiku"}"#);
        assert_eq!(s.provider.as_deref(), Some("openrouter"));
        assert_eq!(s.model.as_deref(), Some("anthropic/claude-3.5-haiku"));
    }

    #[test]
    fn parse_of_garbage_or_empty_is_the_default() {
        assert_eq!(Settings::parse(""), Settings::default());
        assert_eq!(Settings::parse("not json at all"), Settings::default());
        assert_eq!(Settings::parse("{}"), Settings::default());
    }

    #[test]
    fn parse_tolerates_missing_and_unknown_fields() {
        // Only `model` set; an unknown key is ignored (forward compatibility).
        let s = Settings::parse(r#"{"model":"gpt-4o-mini","theme":"dark"}"#);
        assert_eq!(s.provider, None);
        assert_eq!(s.model.as_deref(), Some("gpt-4o-mini"));
    }

    #[test]
    fn json_round_trips() {
        let s = Settings::for_selection("openrouter", "vendor/model-1");
        assert_eq!(Settings::parse(&s.to_json()), s);
    }

    #[test]
    fn to_json_omits_unset_fields() {
        let json = Settings::default().to_json();
        assert!(
            !json.contains("provider"),
            "unset fields are skipped: {json}"
        );
        assert!(!json.contains("model"), "unset fields are skipped: {json}");
    }
}
