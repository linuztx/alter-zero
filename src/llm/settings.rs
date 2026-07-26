//! Persisted UI settings — currently just the last provider/model chosen via
//! `/model`, so the selection survives a restart. Written to
//! `~/.alter-zero/config.json` by the boundary; the parse/serialize here is
//! **pure and unit-tested** (the file read/write lives in `main.rs`, like the
//! `.env` keystore). See `docs/llm.md`.

use serde::{Deserialize, Serialize};

use super::reasoning::{ReasoningEffort, ReasoningSupport, ThinkingMode};

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
    /// The saved model's reasoning capability + chosen mode, when it supports
    /// thinking — restored at startup so the Shift+Tab cycle needs no refetch.
    /// See `docs/reasoning.md`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<ThinkingSettings>,
    /// The saved model's image-input support, when its `/v1/models` record
    /// said either way — restored at startup so the backend gates image
    /// attachments without a re-probe. Absent = unknown (a legacy file, or a
    /// provider whose records don't say) — the probe finds out. See
    /// `docs/tools.md`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vision: Option<bool>,
    /// The saved model's context window in tokens, when its `/v1/models`
    /// record reported one — restored at startup so the footer's
    /// `{used}/{window} ({pct}%)` gauge (and the auto-compact trigger) work
    /// without a re-probe. Absent = unknown. See `docs/compact.md`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<u64>,
}

/// The persisted reasoning state, as plain labels so a hand-edited or
/// future-versioned file still loads ([`ThinkingSettings::to_state`] skips
/// what it can't parse). See `docs/reasoning.md`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThinkingSettings {
    /// `Some(false)` records "this model has no thinking" so startup doesn't
    /// re-probe `/v1/models` for it; absent/`Some(true)` means the rest of the
    /// blob describes a real reasoning state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supported: Option<bool>,
    /// The chosen [`ThinkingMode`] label (`off`/`on`/an effort name).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    /// The model's accepted effort labels (empty = an on/off-only reasoner).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub efforts: Option<Vec<String>>,
    /// Whether the model's reasoning can be turned off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub can_disable: Option<bool>,
    /// The provider-reported default effort, when named.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_effort: Option<String>,
}

impl ThinkingSettings {
    /// The blob recording a live reasoning state.
    #[must_use]
    pub fn from_state(support: &ReasoningSupport, mode: ThinkingMode) -> Self {
        Self {
            supported: Some(true),
            mode: Some(mode.label().to_string()),
            efforts: Some(
                support
                    .efforts
                    .iter()
                    .map(|e| e.as_str().to_string())
                    .collect(),
            ),
            can_disable: Some(support.can_disable),
            default_effort: support.default_effort.map(|e| e.as_str().to_string()),
        }
    }

    /// The marker blob for a model known to have no thinking at all — startup
    /// then skips the support probe entirely.
    #[must_use]
    pub fn unsupported() -> Self {
        Self {
            supported: Some(false),
            ..Self::default()
        }
    }

    /// What to seed [`crate::app::App::set_thinking`] with: `None` for the
    /// [`unsupported`](Self::unsupported) marker, else the restored state.
    #[must_use]
    pub fn to_seed(&self) -> Option<(ReasoningSupport, ThinkingMode)> {
        if self.supported == Some(false) {
            return None;
        }
        Some(self.to_state())
    }

    /// Restore the live state: unknown effort labels are skipped, and a mode
    /// the restored support doesn't offer (or that doesn't parse) falls back
    /// to its default — a stale file degrades, never wedges.
    #[must_use]
    pub fn to_state(&self) -> (ReasoningSupport, ThinkingMode) {
        let support = ReasoningSupport {
            efforts: self
                .efforts
                .as_deref()
                .unwrap_or_default()
                .iter()
                .filter_map(|label| ReasoningEffort::parse(label))
                .collect(),
            can_disable: self.can_disable.unwrap_or(true),
            default_effort: self
                .default_effort
                .as_deref()
                .and_then(ReasoningEffort::parse),
        };
        let mode = self
            .mode
            .as_deref()
            .and_then(ThinkingMode::parse)
            .filter(|m| support.modes().contains(m))
            .unwrap_or_else(|| support.default_mode());
        (support, mode)
    }
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
            thinking: None,
            vision: None,
            context: None,
        }
    }

    /// The same settings with the saved model's reasoning state attached
    /// (`None` for a model with no thinking support).
    #[must_use]
    pub fn with_thinking(mut self, thinking: Option<ThinkingSettings>) -> Self {
        self.thinking = thinking;
        self
    }

    /// The same settings with the saved model's image-input support attached
    /// (`None` = the record didn't say).
    #[must_use]
    pub fn with_vision(mut self, vision: Option<bool>) -> Self {
        self.vision = vision;
        self
    }

    /// The same settings with the saved model's context window attached
    /// (`None` = the record didn't report one). See `docs/compact.md`.
    #[must_use]
    pub fn with_context(mut self, context: Option<u64>) -> Self {
        self.context = context;
        self
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
    fn the_context_window_round_trips_and_omits_when_absent() {
        // The saved model's context window (docs/compact.md) — restored at
        // startup so the footer gauge shows without a re-probe.
        let s = Settings::for_selection("p", "m").with_context(Some(128_000));
        assert_eq!(Settings::parse(&s.to_json()).context, Some(128_000));
        assert!(
            !Settings::for_selection("p", "m")
                .to_json()
                .contains("context"),
            "absent stays off the wire (old-shape compat)"
        );
    }

    fn trio_support() -> ReasoningSupport {
        ReasoningSupport {
            efforts: vec![
                ReasoningEffort::Low,
                ReasoningEffort::Medium,
                ReasoningEffort::High,
            ],
            can_disable: true,
            default_effort: Some(ReasoningEffort::Medium),
        }
    }

    #[test]
    fn thinking_round_trips_through_json() {
        let s = Settings::for_selection("openrouter", "vendor/model-1").with_thinking(Some(
            ThinkingSettings::from_state(
                &trio_support(),
                ThinkingMode::Effort(ReasoningEffort::High),
            ),
        ));
        let restored = Settings::parse(&s.to_json());
        assert_eq!(restored, s);
        let (support, mode) = restored.thinking.unwrap().to_state();
        assert_eq!(support, trio_support());
        assert_eq!(mode, ThinkingMode::Effort(ReasoningEffort::High));
    }

    #[test]
    fn an_effortless_reasoner_round_trips() {
        // efforts = [] (Venice's on/off-only shape) must survive the file —
        // it is a real state, distinct from "no thinking support at all".
        let support = ReasoningSupport {
            efforts: vec![],
            can_disable: true,
            default_effort: None,
        };
        let blob = ThinkingSettings::from_state(&support, ThinkingMode::On);
        let (restored, mode) = blob.to_state();
        assert_eq!(restored, support);
        assert_eq!(mode, ThinkingMode::On);
    }

    #[test]
    fn a_legacy_file_without_thinking_loads_with_none() {
        let s = Settings::parse(r#"{"provider":"openrouter","model":"m"}"#);
        assert_eq!(s.thinking, None);
        assert!(
            !Settings::for_selection("p", "m")
                .to_json()
                .contains("thinking"),
            "unset thinking is skipped in the output"
        );
    }

    #[test]
    fn a_known_non_reasoner_is_recorded_and_seeds_nothing() {
        // "This model has no thinking" is knowledge worth keeping — without
        // it every startup re-probes /v1/models for nothing. The marker blob
        // seeds None; a real state seeds Some; and a legacy/absent blob stays
        // "unknown" at the caller (which probes).
        let marker = ThinkingSettings::unsupported();
        assert_eq!(marker.to_seed(), None);
        let s = Settings::for_selection("p", "m").with_thinking(Some(marker));
        let restored = Settings::parse(&s.to_json());
        assert_eq!(restored.thinking.unwrap().to_seed(), None);

        let real = ThinkingSettings::from_state(&trio_support(), ThinkingMode::Off);
        assert_eq!(real.to_seed(), Some((trio_support(), ThinkingMode::Off)));
    }

    #[test]
    fn a_stale_or_garbled_thinking_blob_degrades_gracefully() {
        // Unknown effort labels are skipped; an unparseable mode falls back to
        // the support's default. A hand-edited file can never wedge startup.
        let s = Settings::parse(
            r#"{"thinking":{"mode":"turbo","efforts":["low","warp","high"],"can_disable":false}}"#,
        );
        let (support, mode) = s.thinking.unwrap().to_state();
        assert_eq!(
            support.efforts,
            vec![ReasoningEffort::Low, ReasoningEffort::High]
        );
        assert!(!support.can_disable);
        assert_eq!(mode, support.default_mode());
    }

    #[test]
    fn vision_round_trips_and_legacy_files_load_with_none() {
        // The saved model's image-input support is persisted beside the
        // thinking blob so startup needs no re-probe; a legacy file simply
        // has it unknown (the probe finds out). See docs/tools.md.
        let s =
            Settings::for_selection("openrouter", "openai/gpt-oss-120b").with_vision(Some(false));
        let restored = Settings::parse(&s.to_json());
        assert_eq!(restored.vision, Some(false));
        let known_vision = Settings::for_selection("p", "m").with_vision(Some(true));
        assert_eq!(Settings::parse(&known_vision.to_json()).vision, Some(true));
        let legacy = Settings::parse(r#"{"provider":"p","model":"m"}"#);
        assert_eq!(legacy.vision, None);
        assert!(
            !Settings::for_selection("p", "m")
                .to_json()
                .contains("vision"),
            "unset vision is skipped in the output"
        );
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
