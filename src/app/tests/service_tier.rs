//! The `/fast` command: the model's speed lanes, the toggle, and the footer
//! marker. See `docs/fast-mode.md`.

use super::*;
use crate::llm::service_tier::{DEFAULT_ID, FAST_ID, ServiceTier, ServiceTierSupport};

fn fast_support() -> ServiceTierSupport {
    ServiceTierSupport {
        tiers: vec![ServiceTier {
            id: FAST_ID.to_string(),
            name: "Fast".to_string(),
            description: "2x speed, increased usage".to_string(),
        }],
        default_tier: None,
    }
}

/// An app on a model that offers the fast lane, standard by default.
fn fast_app() -> App {
    let mut app = App::new();
    app.set_session_info("gpt-5.6-sol", "~/repo");
    app.set_service_tier(Some((fast_support(), None)));
    app
}

#[test]
fn a_model_with_no_lanes_explains_instead_of_toggling() {
    let mut app = App::new();
    app.set_session_info("some-model", "~/repo");
    // The Ctrl+T-on-a-non-reasoner rule: the command is never silently dead.
    let action = app.toggle_service_tier();
    match action {
        Action::Toast(text) => assert!(
            text.contains("some-model") && text.contains("fast"),
            "names the model and what it lacks: {text}"
        ),
        other => panic!("expected an explanatory toast, got {other:?}"),
    }
    assert!(app.service_tier_label().is_none());
}

#[test]
fn toggling_walks_standard_to_fast_and_back() {
    let mut app = fast_app();
    assert_eq!(app.toggle_service_tier(), Action::SetServiceTier);
    assert_eq!(app.service_tier_label(), Some("fast"));
    assert_eq!(app.service_tier_for_request(), Some(FAST_ID.to_string()));

    assert_eq!(app.toggle_service_tier(), Action::SetServiceTier);
    assert_eq!(app.service_tier_label(), None);
    // …and back to standard is an *explicit* choice, not "never chose": it is
    // what outranks a catalog default the next model may carry.
    assert_eq!(
        app.service_tier.as_ref().and_then(|s| s.selected.clone()),
        Some(DEFAULT_ID.to_string())
    );
}

#[test]
fn the_request_carries_only_a_lane_the_model_offers() {
    let mut app = fast_app();
    assert_eq!(app.service_tier_for_request(), None, "standard by default");
    app.toggle_service_tier();
    assert_eq!(app.service_tier_for_request(), Some(FAST_ID.to_string()));
}

#[test]
fn switching_models_drops_a_lane_the_new_one_does_not_offer() {
    let mut app = fast_app();
    app.toggle_service_tier();
    assert_eq!(app.service_tier_label(), Some("fast"));
    // A `/model` switch to a model with no lanes blanks the marker outright,
    // exactly as it blanks the thinking mode.
    app.set_service_tier(None);
    assert_eq!(app.service_tier_label(), None);
    assert_eq!(app.service_tier_for_request(), None);
}

/// Type `text` into the composer key by key, as the user would.
fn type_str(app: &mut App, text: &str) {
    for c in text.chars() {
        app.on_key(key(KeyCode::Char(c)));
    }
}

#[test]
fn the_fast_command_toggles_the_lane() {
    let mut app = fast_app();
    type_str(&mut app, "/fast");
    let action = app.on_key(key(KeyCode::Enter));
    assert_eq!(action, Action::SetServiceTier);
    assert_eq!(app.service_tier_label(), Some("fast"));
    assert!(app.input.text().is_empty(), "the command is consumed");
}

#[test]
fn the_fast_command_works_mid_turn() {
    let mut app = fast_app();
    app.begin_stream();
    type_str(&mut app, "/fast");
    // Like /model it only rebinds the *next* turn — never a busy rejection.
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::SetServiceTier);
    assert_eq!(app.service_tier_label(), Some("fast"));
}

#[test]
fn the_footer_names_the_lane_beside_the_thinking_mode() {
    let mut app = fast_app();
    let plain = crate::ui::footer_line(&app, 80);
    let text: String = plain.spans.iter().map(|s| s.content.as_ref()).collect();
    assert!(!text.contains("fast"), "standard lane is unmarked: {text}");

    app.toggle_service_tier();
    let lit = crate::ui::footer_line(&app, 80);
    let text: String = lit.spans.iter().map(|s| s.content.as_ref()).collect();
    assert!(
        text.contains("gpt-5.6-sol fast"),
        "the lane rides the model segment: {text}"
    );
}
