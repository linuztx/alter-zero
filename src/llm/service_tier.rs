//! Service tiers: the **speed/priority lane** a request runs in, and the
//! `fast` one the `/fast` command switches on. See `docs/fast-mode.md`.
//!
//! A tier is a fact about a *model*, published by its own `/models` record
//! (`llm::models`' `service_tier_support_of` sniff) rather than configured here,
//! so a lane the backend adds tomorrow needs no new code to be selectable —
//! the constants below name only the *fast* lane and the standard-lane
//! sentinel, and only because the UI and the persisted choice have to agree
//! on them.

/// The **fast** lane's wire id. Note the mismatch: the tier is *called* `Fast`
/// and written `fast` in config, but what the API takes is `priority` — so the
/// id is what a request and a saved choice carry, and the name is what the
/// user reads.
pub const FAST_ID: &str = "priority";

/// The deprecated `additional_speed_tiers` spelling of the same lane, kept
/// because a record may still name it there instead of in `service_tiers`.
pub const FAST_NAME: &str = "fast";

/// The **explicit standard lane**: not a tier id at all, but the sentinel for
/// "the user deliberately chose no tier". It is never sent — its whole job is
/// to outrank a catalog default that would otherwise apply
/// ([`ServiceTierSupport::for_request`]).
pub const DEFAULT_ID: &str = "default";

/// One tier a model offers, exactly as its `/models` record names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceTier {
    /// The wire id — what rides the request (`priority`).
    pub id: String,
    /// The label the record gives it (`Fast`).
    pub name: String,
    /// The record's own one-line description (`2x speed, increased usage`).
    pub description: String,
}

/// What one model's record says about tiers: the lanes it offers, and the one
/// the backend applies when the request names none.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ServiceTierSupport {
    /// Every lane the model offers, in the record's own order.
    pub tiers: Vec<ServiceTier>,
    /// The lane the backend uses when the request carries no `service_tier`.
    /// Honoured only when the model actually lists it.
    pub default_tier: Option<String>,
}

impl ServiceTierSupport {
    /// Does the model offer this lane?
    #[must_use]
    pub fn supports(&self, id: &str) -> bool {
        self.tiers.iter().any(|tier| tier.id == id)
    }

    /// The model's **fast** lane, when it has one — by wire id, else by the
    /// deprecated `fast` spelling either field may carry.
    #[must_use]
    pub fn fast(&self) -> Option<&ServiceTier> {
        self.tiers.iter().find(|tier| {
            tier.id == FAST_ID
                || tier.id.eq_ignore_ascii_case(FAST_NAME)
                || tier.name.eq_ignore_ascii_case(FAST_NAME)
        })
    }

    /// What `selected` should send as the request's `service_tier`, or `None`
    /// for the standard lane.
    ///
    /// Three rules, each one a way a naive pass-through breaks: the
    /// [`DEFAULT_ID`] sentinel is a *choice*, never a tier, so it sends
    /// nothing **and** suppresses the catalog default; a stale choice the
    /// model no longer offers is dropped rather than 400ing the turn; and an
    /// unchosen tier falls back to the catalog's default only when the model
    /// really lists it.
    #[must_use]
    pub fn for_request(&self, selected: Option<&str>) -> Option<String> {
        match selected {
            Some(DEFAULT_ID) => None,
            Some(id) => self.supports(id).then(|| id.to_string()),
            None => self
                .default_tier
                .as_deref()
                .filter(|id| self.supports(id))
                .map(str::to_string),
        }
    }

    /// The choice `/fast` moves to from `selected`: the fast lane when it is
    /// not already running, else the explicit standard one. `None` when the
    /// model has no fast lane to offer — the caller explains instead.
    #[must_use]
    pub fn toggled_fast(&self, selected: Option<&str>) -> Option<String> {
        let fast = self.fast()?;
        if self.for_request(selected).as_deref() == Some(fast.id.as_str()) {
            Some(DEFAULT_ID.to_string())
        } else {
            Some(fast.id.clone())
        }
    }

    /// The footer's marker for `selected` — `Some("fast")` only while the
    /// request will actually carry the fast lane (a catalog default counts),
    /// `None` otherwise. It names what the wire does, not what was clicked.
    #[must_use]
    pub fn label(&self, selected: Option<&str>) -> Option<&'static str> {
        let fast = self.fast()?;
        (self.for_request(selected).as_deref() == Some(fast.id.as_str())).then_some(FAST_NAME)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fast_tier() -> ServiceTier {
        ServiceTier {
            id: "priority".to_string(),
            name: "Fast".to_string(),
            description: "2x speed, increased usage".to_string(),
        }
    }

    fn fast_support() -> ServiceTierSupport {
        ServiceTierSupport {
            tiers: vec![fast_tier()],
            default_tier: None,
        }
    }

    #[test]
    fn the_fast_tier_rides_the_wire_as_priority() {
        assert_eq!(FAST_ID, "priority");
        assert_eq!(DEFAULT_ID, "default");
    }

    #[test]
    fn a_support_finds_its_fast_tier_by_id_or_name() {
        assert_eq!(
            fast_support().fast().map(|t| t.id.clone()),
            Some(FAST_ID.to_string())
        );
        // Named `fast` rather than carrying the `priority` id — the
        // deprecated `additional_speed_tiers` spelling.
        let named = ServiceTierSupport {
            tiers: vec![ServiceTier {
                id: "fast".to_string(),
                name: "Fast".to_string(),
                description: String::new(),
            }],
            default_tier: None,
        };
        assert_eq!(named.fast().map(|t| t.id.clone()), Some("fast".to_string()));
        assert!(ServiceTierSupport::default().fast().is_none());
    }

    #[test]
    fn only_a_tier_the_model_offers_reaches_the_request() {
        let s = fast_support();
        assert_eq!(
            s.for_request(Some("priority")),
            Some("priority".to_string())
        );
        // The explicit "no tier" sentinel is a choice, not a tier: never sent.
        assert_eq!(s.for_request(Some(DEFAULT_ID)), None);
        // A stale choice the model does not offer is dropped rather than 400ing.
        assert_eq!(s.for_request(Some("flex")), None);
        assert_eq!(s.for_request(None), None);
    }

    #[test]
    fn an_unchosen_tier_falls_back_to_the_catalog_default() {
        let s = ServiceTierSupport {
            tiers: vec![fast_tier()],
            default_tier: Some("priority".to_string()),
        };
        assert_eq!(s.for_request(None), Some("priority".to_string()));
        // …but an explicit `default` outranks the catalog's own default: the
        // user said standard, and a catalog default must not undo that.
        assert_eq!(s.for_request(Some(DEFAULT_ID)), None);
        // A default the model does not actually list is ignored.
        let bogus = ServiceTierSupport {
            tiers: vec![fast_tier()],
            default_tier: Some("flex".to_string()),
        };
        assert_eq!(bogus.for_request(None), None);
    }

    #[test]
    fn toggling_walks_between_the_fast_tier_and_standard() {
        let s = fast_support();
        assert_eq!(s.toggled_fast(None), Some(FAST_ID.to_string()));
        assert_eq!(s.toggled_fast(Some(FAST_ID)), Some(DEFAULT_ID.to_string()));
        assert_eq!(s.toggled_fast(Some(DEFAULT_ID)), Some(FAST_ID.to_string()));
        // Nothing to toggle on a model with no fast tier.
        assert_eq!(ServiceTierSupport::default().toggled_fast(None), None);
    }

    #[test]
    fn the_footer_marker_shows_only_an_active_fast_tier() {
        let s = fast_support();
        assert_eq!(s.label(Some(FAST_ID)), Some("fast"));
        assert_eq!(s.label(Some(DEFAULT_ID)), None);
        assert_eq!(s.label(None), None);
        // A catalog default that is fast still reads as fast on the footer:
        // the marker names what the request will carry.
        let defaulted = ServiceTierSupport {
            tiers: vec![fast_tier()],
            default_tier: Some("priority".to_string()),
        };
        assert_eq!(defaulted.label(None), Some("fast"));
    }
}
