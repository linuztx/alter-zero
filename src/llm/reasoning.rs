//! Reasoning ("thinking") modes — the pure core behind the Ctrl+T cycle.
//!
//! A reasoning-capable model advertises what it supports in the provider's
//! `/v1/models` record ([`ReasoningSupport`], parsed in [`super::models`]);
//! the user cycles the active [`ThinkingMode`] with Ctrl+T and the choice
//! rides the request payload (`docs/reasoning.md`). Pure and unit-tested; no
//! HTTP here.

use serde_json::json;

/// One reasoning-effort level — the canonical ladder both OpenRouter's
/// `reasoning.effort` and Venice's accept (`"minimal"` … `"max"`). A model
/// offers a subset ([`ReasoningSupport::efforts`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReasoningEffort {
    Minimal,
    Low,
    Medium,
    High,
    XHigh,
    Max,
}

impl ReasoningEffort {
    /// Every level, lowest to highest — the cycle order and the sort key for a
    /// provider's unordered `supported_efforts` list.
    pub const LADDER: &'static [Self] = &[
        Self::Minimal,
        Self::Low,
        Self::Medium,
        Self::High,
        Self::XHigh,
        Self::Max,
    ];

    /// The wire label (`reasoning.effort`'s value) — also what the footer and
    /// toast show.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::XHigh => "xhigh",
            Self::Max => "max",
        }
    }

    /// Parse a wire/persisted label (case-insensitive). `None` for anything
    /// off the ladder (including `"none"` — that's [`ThinkingMode::Off`], not
    /// an effort).
    #[must_use]
    pub fn parse(label: &str) -> Option<Self> {
        let label = label.trim().to_ascii_lowercase();
        Self::LADDER.iter().copied().find(|e| e.as_str() == label)
    }
}

/// The active thinking mode of a reasoning-capable model — what Ctrl+T
/// cycles and the footer shows beside the model name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThinkingMode {
    /// Reasoning disabled (`reasoning: {"enabled": false}`, plus Venice's
    /// `venice_parameters.disable_thinking` — the toggle Venice honours).
    Off,
    /// Reasoning at the model's own default — no parameter sent. Only offered
    /// by models that reason but take no effort level (Venice's
    /// `supportsReasoning` without `supportsReasoningEffort`).
    On,
    /// Reasoning at an explicit effort (`reasoning: {"effort": …}`).
    Effort(ReasoningEffort),
}

impl ThinkingMode {
    /// The display/persistence label: `off`, `on`, or the effort's name.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::On => "on",
            Self::Effort(effort) => effort.as_str(),
        }
    }

    /// Parse a persisted label back into a mode.
    #[must_use]
    pub fn parse(label: &str) -> Option<Self> {
        match label.trim().to_ascii_lowercase().as_str() {
            "off" => Some(Self::Off),
            "on" => Some(Self::On),
            other => ReasoningEffort::parse(other).map(Self::Effort),
        }
    }
}

/// What a model's `/v1/models` record said about its reasoning: which effort
/// levels it accepts (empty = it reasons but takes no effort parameter) and
/// whether reasoning can be turned off at all (OpenRouter marks some models
/// `mandatory`). Parsed per provider shape in [`super::models`]; absent
/// entirely (`Option::None` on the entry) for a model with no reasoning.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReasoningSupport {
    /// The accepted effort levels, canonically ordered (a subset of
    /// [`ReasoningEffort::LADDER`]). Empty for an on/off-only reasoner.
    pub efforts: Vec<ReasoningEffort>,
    /// Whether [`ThinkingMode::Off`] is available (false for a `mandatory`
    /// reasoner).
    pub can_disable: bool,
    /// The provider-reported default effort, when named (OpenRouter's
    /// `default_effort`) — the fallback when `medium` isn't offered.
    pub default_effort: Option<ReasoningEffort>,
}

impl ReasoningSupport {
    /// Every mode Ctrl+T cycles through, in order: `Off` (when the model
    /// allows disabling) then the effort ladder — or plain `On` when the model
    /// takes no effort parameter.
    #[must_use]
    pub fn modes(&self) -> Vec<ThinkingMode> {
        let mut modes = Vec::new();
        if self.can_disable {
            modes.push(ThinkingMode::Off);
        }
        if self.efforts.is_empty() {
            modes.push(ThinkingMode::On);
        } else {
            modes.extend(self.efforts.iter().map(|&e| ThinkingMode::Effort(e)));
        }
        modes
    }

    /// The mode a freshly-selected model starts in: `medium` when offered
    /// (the requested default), else the provider's own `default_effort`,
    /// else the middle of the ladder — and `On` for an effort-less reasoner.
    /// Never `Off`: a reasoning model defaults to reasoning.
    #[must_use]
    pub fn default_mode(&self) -> ThinkingMode {
        if self.efforts.contains(&ReasoningEffort::Medium) {
            return ThinkingMode::Effort(ReasoningEffort::Medium);
        }
        if let Some(default) = self.default_effort
            && self.efforts.contains(&default)
        {
            return ThinkingMode::Effort(default);
        }
        match self.efforts.get(self.efforts.len() / 2) {
            Some(&middle) => ThinkingMode::Effort(middle),
            None => ThinkingMode::On,
        }
    }

    /// The mode after `current` in the cycle, wrapping at the end. A `current`
    /// the model doesn't offer (a stale persisted value) steps to the default.
    #[must_use]
    pub fn next_mode(&self, current: ThinkingMode) -> ThinkingMode {
        let modes = self.modes();
        match modes.iter().position(|&m| m == current) {
            Some(i) => modes[(i + 1) % modes.len()],
            None => self.default_mode(),
        }
    }
}

/// The request's `reasoning` object for a mode: an explicit effort, a disable,
/// or nothing at all for [`ThinkingMode::On`] (the model's own default). The
/// Venice-side `disable_thinking` companion is applied by the payload builder
/// (`super::openai`), which knows whether the provider carries a
/// `venice_parameters` table.
#[must_use]
pub fn reasoning_body(mode: ThinkingMode) -> Option<serde_json::Value> {
    match mode {
        ThinkingMode::Off => Some(json!({"enabled": false})),
        ThinkingMode::On => None,
        ThinkingMode::Effort(effort) => Some(json!({"effort": effort.as_str()})),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effort_labels_round_trip() {
        for effort in ReasoningEffort::LADDER {
            assert_eq!(ReasoningEffort::parse(effort.as_str()), Some(*effort));
        }
        assert_eq!(
            ReasoningEffort::parse("medium"),
            Some(ReasoningEffort::Medium)
        );
        assert_eq!(
            ReasoningEffort::parse("XHigh"),
            Some(ReasoningEffort::XHigh)
        );
        assert_eq!(ReasoningEffort::parse("nope"), None);
        assert_eq!(ReasoningEffort::parse(""), None);
    }

    #[test]
    fn ladder_orders_minimal_to_max() {
        let labels: Vec<&str> = ReasoningEffort::LADDER.iter().map(|e| e.as_str()).collect();
        assert_eq!(labels, ["minimal", "low", "medium", "high", "xhigh", "max"]);
    }

    #[test]
    fn mode_labels_round_trip() {
        assert_eq!(ThinkingMode::Off.label(), "off");
        assert_eq!(ThinkingMode::On.label(), "on");
        assert_eq!(
            ThinkingMode::Effort(ReasoningEffort::Medium).label(),
            "medium"
        );
        for mode in [
            ThinkingMode::Off,
            ThinkingMode::On,
            ThinkingMode::Effort(ReasoningEffort::XHigh),
        ] {
            assert_eq!(ThinkingMode::parse(mode.label()), Some(mode));
        }
        assert_eq!(ThinkingMode::parse("bogus"), None);
    }

    /// The support shape most models advertise: a full effort ladder that can
    /// be disabled.
    fn trio() -> ReasoningSupport {
        ReasoningSupport {
            efforts: vec![
                ReasoningEffort::Low,
                ReasoningEffort::Medium,
                ReasoningEffort::High,
            ],
            can_disable: true,
            default_effort: None,
        }
    }

    #[test]
    fn modes_cycle_off_then_the_effort_ladder() {
        let modes = trio().modes();
        assert_eq!(
            modes,
            vec![
                ThinkingMode::Off,
                ThinkingMode::Effort(ReasoningEffort::Low),
                ThinkingMode::Effort(ReasoningEffort::Medium),
                ThinkingMode::Effort(ReasoningEffort::High),
            ]
        );
    }

    #[test]
    fn a_mandatory_reasoner_has_no_off_mode() {
        // OpenRouter's kimi-k3 shape: mandatory, only "max" accepted.
        let support = ReasoningSupport {
            efforts: vec![ReasoningEffort::Max],
            can_disable: false,
            default_effort: None,
        };
        assert_eq!(
            support.modes(),
            vec![ThinkingMode::Effort(ReasoningEffort::Max)]
        );
    }

    #[test]
    fn an_effortless_reasoner_toggles_on_off() {
        // Venice's supportsReasoning-without-supportsReasoningEffort shape
        // (e.g. Claude through the Agent Zero proxy): no effort parameter, so
        // the cycle is just off ↔ on (the model's own default thinking).
        let support = ReasoningSupport {
            efforts: vec![],
            can_disable: true,
            default_effort: None,
        };
        assert_eq!(support.modes(), vec![ThinkingMode::Off, ThinkingMode::On]);
    }

    #[test]
    fn default_mode_prefers_medium() {
        assert_eq!(
            trio().default_mode(),
            ThinkingMode::Effort(ReasoningEffort::Medium)
        );
    }

    #[test]
    fn default_mode_falls_back_to_the_models_default_effort() {
        // OpenRouter's deepseek-v4-flash shape: only xhigh/high, default high.
        let support = ReasoningSupport {
            efforts: vec![ReasoningEffort::High, ReasoningEffort::XHigh],
            can_disable: true,
            default_effort: Some(ReasoningEffort::High),
        };
        assert_eq!(
            support.default_mode(),
            ThinkingMode::Effort(ReasoningEffort::High)
        );
    }

    #[test]
    fn default_mode_without_medium_or_a_default_takes_the_middle() {
        let support = ReasoningSupport {
            efforts: vec![
                ReasoningEffort::Low,
                ReasoningEffort::High,
                ReasoningEffort::XHigh,
                ReasoningEffort::Max,
            ],
            can_disable: true,
            default_effort: None,
        };
        // Four levels, middle = index 4/2 = xhigh — never Off, never a panic.
        assert_eq!(
            support.default_mode(),
            ThinkingMode::Effort(ReasoningEffort::XHigh)
        );
    }

    #[test]
    fn default_mode_of_an_effortless_reasoner_is_on() {
        let support = ReasoningSupport {
            efforts: vec![],
            can_disable: true,
            default_effort: None,
        };
        assert_eq!(support.default_mode(), ThinkingMode::On);
    }

    #[test]
    fn a_stale_default_effort_outside_the_ladder_is_ignored() {
        // The record's default_effort must actually be offered; otherwise the
        // medium-then-middle fallbacks apply.
        let support = ReasoningSupport {
            efforts: vec![ReasoningEffort::Low, ReasoningEffort::High],
            can_disable: true,
            default_effort: Some(ReasoningEffort::Max),
        };
        assert_eq!(
            support.default_mode(),
            ThinkingMode::Effort(ReasoningEffort::High)
        );
    }

    #[test]
    fn next_mode_cycles_and_wraps() {
        let support = trio();
        let low = ThinkingMode::Effort(ReasoningEffort::Low);
        let medium = ThinkingMode::Effort(ReasoningEffort::Medium);
        let high = ThinkingMode::Effort(ReasoningEffort::High);
        assert_eq!(support.next_mode(ThinkingMode::Off), low);
        assert_eq!(support.next_mode(low), medium);
        assert_eq!(support.next_mode(medium), high);
        assert_eq!(support.next_mode(high), ThinkingMode::Off, "wraps to Off");
    }

    #[test]
    fn next_mode_from_an_unknown_current_lands_on_the_default() {
        // A stale persisted mode the model no longer offers must not wedge the
        // cycle — step to the default rather than panicking or sticking.
        let support = trio();
        assert_eq!(
            support.next_mode(ThinkingMode::Effort(ReasoningEffort::Max)),
            support.default_mode()
        );
    }

    #[test]
    fn single_mode_support_cycles_in_place() {
        let support = ReasoningSupport {
            efforts: vec![ReasoningEffort::Max],
            can_disable: false,
            default_effort: None,
        };
        let max = ThinkingMode::Effort(ReasoningEffort::Max);
        assert_eq!(support.next_mode(max), max);
    }

    #[test]
    fn reasoning_body_maps_the_modes() {
        use serde_json::json;
        assert_eq!(
            reasoning_body(ThinkingMode::Effort(ReasoningEffort::Medium)),
            Some(json!({"effort": "medium"}))
        );
        assert_eq!(
            reasoning_body(ThinkingMode::Off),
            Some(json!({"enabled": false}))
        );
        // On = the model's own default — no reasoning key at all.
        assert_eq!(reasoning_body(ThinkingMode::On), None);
    }
}
