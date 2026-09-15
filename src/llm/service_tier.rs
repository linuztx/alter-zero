//! Speed tiers — the pure core behind `/fast` (`docs/fast-mode.md`).
//!
//! Codex's *fast mode* is a **service tier** on the request: a model whose
//! `/models` record lists a tier with the wire id `priority` can be asked
//! for priority processing, and the ChatGPT backend answers ~1.5–2× faster
//! at increased plan usage. This module holds what the record said
//! ([`ServiceTier`]) and the session's choice over it ([`SpeedState`]) with
//! the cycle `/fast` steps through; the parse is in [`super::models`], the
//! wire field in [`super::responses`] / [`super::openai`], and the boundary
//! wiring in `tui::models`. Pure and unit-tested; no HTTP here.

use serde::{Deserialize, Serialize};

/// What the footer and the `Speed: …` toast say when no tier is selected —
/// the request carries no `service_tier` at all, so the backend routes it
/// as it routes every other request.
pub const STANDARD_LABEL: &str = "standard";

/// One speed tier a model's `/models` record lists — codex's
/// `ModelServiceTier`, verbatim: the **id** is the wire value the request
/// carries (`priority`), the **name** what a catalog shows (`Fast`), and the
/// **description** the backend's own one-line cost statement (`1.5x speed,
/// increased usage`), which the `/fast` toast repeats so the price of the
/// speed is said where it is bought. Serializable because `config.json`
/// keeps the listed tiers beside the model selection, the way it keeps the
/// reasoning ladder (`docs/per-directory-state.md`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceTier {
    /// The wire value (`service_tier: "priority"`).
    pub id: String,
    /// The catalog's name for it (`Fast`); its lowercase is the [`label`].
    ///
    /// [`label`]: ServiceTier::label
    pub name: String,
    /// The backend's own description, empty when the record gave none.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
}

impl ServiceTier {
    /// The fast tier's wire id — codex's `ServiceTier::Fast.request_value()`.
    pub const FAST_ID: &'static str = "priority";
    /// The fast tier's catalog name.
    pub const FAST_NAME: &'static str = "Fast";

    #[must_use]
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        description: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            description: description.into(),
        }
    }

    /// The fast tier as the **legacy** catalog marker names it — a record
    /// carrying `additional_speed_tiers: ["fast"]` and no `service_tiers`
    /// list (codex's `SPEED_TIER_FAST`, which its own parse still honours).
    /// No description: the marker carries none.
    #[must_use]
    pub fn fast() -> Self {
        Self::new(Self::FAST_ID, Self::FAST_NAME, "")
    }

    /// Is this codex's *fast mode* tier? Decided by either half — the wire
    /// id `priority` or a name of `fast` — since a catalog may spell one and
    /// not the other; `ultrafast` is a tier of its own.
    #[must_use]
    pub fn is_fast(&self) -> bool {
        self.id == Self::FAST_ID || self.name.eq_ignore_ascii_case("fast")
    }

    /// The word the footer and toast wear: the name lowercased (`fast`,
    /// `ultrafast`), falling back to the id when the record named nothing —
    /// never the bare id otherwise, since `priority` says nothing to a user
    /// who pressed `/fast`.
    #[must_use]
    pub fn label(&self) -> String {
        if self.name.is_empty() {
            self.id.to_ascii_lowercase()
        } else {
            self.name.to_ascii_lowercase()
        }
    }
}

/// The active model's speed state: the tiers its record listed and the one
/// selected (`None` = standard, no `service_tier` on the wire). Lives on
/// `App::speed`; `/fast` steps it through [`SpeedState::advance`], the footer
/// shows [`SpeedState::label`] beside the model name, and the boundary
/// threads [`SpeedState::tier`] into each request (`docs/fast-mode.md`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpeedState {
    /// The tiers the model offers, in the record's order — the cycle's order.
    pub tiers: Vec<ServiceTier>,
    /// The selected tier's id, when one is; `None` is standard.
    pub tier: Option<String>,
}

impl SpeedState {
    /// The state for a model listing `tiers`, with `tier` selected when the
    /// list offers it — a saved choice the model no longer lists falls back
    /// to standard rather than riding a request that would be refused.
    /// `None` when the model lists no tier at all: there is nothing to
    /// select, the footer shows no speed, and `/fast` explains instead.
    #[must_use]
    pub fn new(tiers: Vec<ServiceTier>, tier: Option<String>) -> Option<Self> {
        if tiers.is_empty() {
            return None;
        }
        let tier = tier.filter(|id| tiers.iter().any(|t| t.id == *id));
        Some(Self { tiers, tier })
    }

    /// The selected tier, when one is.
    #[must_use]
    pub fn selected(&self) -> Option<&ServiceTier> {
        let id = self.tier.as_deref()?;
        self.tiers.iter().find(|t| t.id == id)
    }

    /// Whether the selection is the fast tier.
    #[must_use]
    pub fn is_fast(&self) -> bool {
        self.selected().is_some_and(ServiceTier::is_fast)
    }

    /// The footer/toast word for the selection: the tier's [`label`], or
    /// [`STANDARD_LABEL`] with none selected.
    ///
    /// [`label`]: ServiceTier::label
    #[must_use]
    pub fn label(&self) -> String {
        self.selected()
            .map_or_else(|| STANDARD_LABEL.to_string(), ServiceTier::label)
    }

    /// The selection after this one in the cycle: standard → the first
    /// listed tier → each next one → standard again. With one listed tier
    /// (the usual catalog) that is codex's own `/fast` toggle.
    #[must_use]
    pub fn next(&self) -> Option<ServiceTier> {
        let at = self
            .tier
            .as_deref()
            .and_then(|id| self.tiers.iter().position(|t| t.id == id));
        match at {
            None => self.tiers.first().cloned(),
            Some(i) => self.tiers.get(i + 1).cloned(),
        }
    }

    /// Step the cycle ([`next`]) and return the new selection.
    ///
    /// [`next`]: SpeedState::next
    pub fn advance(&mut self) -> Option<ServiceTier> {
        let next = self.next();
        self.tier = next.as_ref().map(|t| t.id.clone());
        next
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fast() -> ServiceTier {
        ServiceTier::new("priority", "Fast", "1.5x speed, increased usage")
    }

    fn ultrafast() -> ServiceTier {
        ServiceTier::new("ultrafast", "Ultrafast", "The fastest available responses.")
    }

    #[test]
    fn the_priority_tier_is_fast_mode_whatever_it_is_called() {
        // The wire id is what the request carries; the name is what the
        // catalog shows. Either spelling of "fast" is the fast tier.
        assert!(fast().is_fast());
        assert!(ServiceTier::new("priority", "Priority", "").is_fast());
        assert!(ServiceTier::new("other", "fast", "").is_fast());
        assert!(!ultrafast().is_fast(), "ultrafast is a tier of its own");
        assert!(ServiceTier::fast().is_fast());
        assert_eq!(ServiceTier::fast().id, ServiceTier::FAST_ID);
    }

    #[test]
    fn a_tiers_label_is_its_lowercased_name() {
        // The footer and the toast wear the tier's name as codex's session
        // header does — `fast` — never the wire id (`priority` says nothing
        // to a user who pressed /fast).
        assert_eq!(fast().label(), "fast");
        assert_eq!(ultrafast().label(), "ultrafast");
        assert_eq!(ServiceTier::new("priority", "", "").label(), "priority");
    }

    #[test]
    fn a_model_that_lists_no_tier_has_no_speed_state() {
        assert_eq!(SpeedState::new(Vec::new(), None), None);
        assert_eq!(
            SpeedState::new(Vec::new(), Some("priority".to_string())),
            None,
            "a saved choice over an empty catalog is nothing to select"
        );
    }

    #[test]
    fn a_saved_tier_the_model_offers_is_kept_and_one_it_does_not_is_dropped() {
        let kept = SpeedState::new(vec![fast()], Some("priority".to_string())).unwrap();
        assert_eq!(kept.selected(), Some(&fast()));
        assert!(kept.is_fast());
        let dropped = SpeedState::new(vec![fast()], Some("ultrafast".to_string())).unwrap();
        assert_eq!(dropped.selected(), None, "back to standard");
        assert!(!dropped.is_fast());
        assert_eq!(dropped.tier, None, "the stale id is not kept around");
    }

    #[test]
    fn the_label_names_the_selection_or_standard() {
        let mut state = SpeedState::new(vec![fast()], None).unwrap();
        assert_eq!(state.label(), STANDARD_LABEL);
        state.advance();
        assert_eq!(state.label(), "fast");
    }

    #[test]
    fn a_single_tier_cycles_as_a_toggle() {
        // The common catalog: one `priority` tier, so /fast is codex's own
        // on/off toggle.
        let mut state = SpeedState::new(vec![fast()], None).unwrap();
        assert_eq!(state.next(), Some(fast()));
        assert_eq!(state.advance(), Some(fast()), "standard → fast");
        assert_eq!(state.tier.as_deref(), Some("priority"));
        assert_eq!(state.advance(), None, "fast → standard");
        assert_eq!(state.tier, None);
    }

    #[test]
    fn several_tiers_cycle_in_catalog_order_then_wrap_to_standard() {
        // A model listing more than one tier steps through each in the order
        // its record listed them — `/fast` reaches ultrafast on the second
        // press — and wraps back to standard.
        let mut state = SpeedState::new(vec![fast(), ultrafast()], None).unwrap();
        assert_eq!(state.advance(), Some(fast()));
        assert_eq!(state.advance(), Some(ultrafast()));
        assert!(!state.is_fast(), "ultrafast is not the fast tier");
        assert_eq!(state.label(), "ultrafast");
        assert_eq!(state.advance(), None);
        assert_eq!(state.advance(), Some(fast()), "and round again");
    }

    #[test]
    fn a_tier_round_trips_through_json_and_omits_an_empty_description() {
        // The blob `config.json` keeps (`docs/per-directory-state.md`): the
        // legacy marker's tier has no description to write.
        let json = serde_json::to_string(&fast()).unwrap();
        assert_eq!(serde_json::from_str::<ServiceTier>(&json).unwrap(), fast());
        let bare = serde_json::to_string(&ServiceTier::fast()).unwrap();
        assert!(!bare.contains("description"), "{bare}");
        let parsed: ServiceTier =
            serde_json::from_str(r#"{"id":"priority","name":"Fast"}"#).unwrap();
        assert_eq!(parsed, ServiceTier::fast());
    }
}
