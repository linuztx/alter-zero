//! The `/settings` pure model: the row inventory, value rendering, cycling,
//! availability, and the `settings.json` round-trip. See `docs/settings.md`.

use super::*;

/// The permission posture a session with permissions on would carry.
const MANUAL: Option<PermissionMode> = Some(PermissionMode::Manual);

/// Cycle `key` `times` times from `start`, returning every value it showed.
fn cycle_values(mut s: SessionSettings, key: SettingKey, times: usize) -> Vec<String> {
    let mut out = vec![s.value_text(key, MANUAL)];
    for _ in 0..times {
        assert!(s.cycle(key), "{key:?} should cycle");
        out.push(s.value_text(key, MANUAL));
    }
    out
}

#[test]
fn every_setting_has_a_label_and_a_description() {
    // The menu renders both columns for every row, and the description line
    // under the list is never blank — a row with an empty one would show a
    // stray gap the layout still reserves.
    for &key in SettingKey::ALL {
        assert!(!key.label().is_empty(), "{key:?} has no label");
        assert!(!key.description().is_empty(), "{key:?} has no description");
    }
}

#[test]
fn the_labels_are_unique() {
    // Type-to-search matches on the label, so two rows sharing one would be
    // indistinguishable in the filtered list.
    let mut labels: Vec<&str> = SettingKey::ALL.iter().map(|k| k.label()).collect();
    labels.sort_unstable();
    let before = labels.len();
    labels.dedup();
    assert_eq!(labels.len(), before, "duplicate setting labels");
}

#[test]
fn the_defaults_match_the_pre_feature_behaviour() {
    // /settings must change nothing until the user touches it: every default
    // is what the app did before the menu existed.
    let s = SessionSettings::default();
    assert!(!s.hide_thinking, "thinking showed by default");
    assert!(s.show_thinking());
    assert_eq!(s.error_retry, crate::llm::retry::MAX_RETRIES);
    assert!(s.tools, "tools were on by default");
    assert!(s.checkpoints);
    assert!(s.auto_compact);
    assert!(s.project_docs);
    assert_eq!(s.temperature, None, "no temperature is sent by default");
    // The one deliberate divergence: the app now ships UNCAPPED where the
    // library's backstop was 20 rounds — see the dedicated test below and
    // `docs/settings.md`.
    assert_eq!(s.max_tool_calls, 0);
}

#[test]
fn booleans_cycle_between_true_and_false() {
    for key in [
        SettingKey::HideThinking,
        SettingKey::Tools,
        SettingKey::Checkpoints,
        SettingKey::AutoCompact,
        SettingKey::ProjectDocs,
    ] {
        let seen = cycle_values(SessionSettings::default(), key, 2);
        assert_eq!(seen[0], seen[2], "{key:?} returns to where it started");
        assert_ne!(seen[0], seen[1], "{key:?} actually moved");
        assert!(
            seen.iter().all(|v| v == "true" || v == "false"),
            "{key:?} shows a boolean: {seen:?}"
        );
    }
}

#[test]
fn max_tool_calls_defaults_to_no_limit_and_cycles_the_offered_ceilings() {
    // 0 is the default *and* the "no limit" value: cutting a long agentic
    // task off part-way leaves its work half-done, and Esc is already the
    // stop button. The offered ceilings are for those who want a hard one.
    let s = SessionSettings::default();
    assert_eq!(s.max_tool_calls, 0, "uncapped by default");
    assert_eq!(s.value_text(SettingKey::MaxToolCalls, MANUAL), "0");

    let seen = cycle_values(
        SessionSettings::default(),
        SettingKey::MaxToolCalls,
        TOOL_CALL_CHOICES.len(),
    );
    assert_eq!(seen.first().unwrap(), "0");
    assert_eq!(
        seen.last().unwrap(),
        "0",
        "the cycle wraps back to no limit"
    );
    for choice in TOOL_CALL_CHOICES {
        assert!(
            seen.contains(&choice.to_string()),
            "{choice} never appeared in {seen:?}"
        );
    }
    assert!(
        TOOL_CALL_CHOICES.contains(&crate::llm::agent::MAX_TOOL_ITERATIONS),
        "the library's own backstop is one of the offered ceilings"
    );
}

#[test]
fn error_retry_cycles_the_offered_counts_and_wraps() {
    let seen = cycle_values(
        SessionSettings::default(),
        SettingKey::ErrorRetry,
        RETRY_CHOICES.len(),
    );
    // Starts at the default (3), walks the rest, and comes back round.
    assert_eq!(seen.first().unwrap(), "3");
    assert_eq!(seen.last().unwrap(), "3", "the cycle wraps");
    for choice in RETRY_CHOICES {
        assert!(
            seen.contains(&choice.to_string()),
            "{choice} never appeared in {seen:?}"
        );
    }
}

#[test]
fn temperature_cycles_from_default_through_the_offered_values() {
    let seen = cycle_values(
        SessionSettings::default(),
        SettingKey::Temperature,
        TEMPERATURE_CHOICES.len(),
    );
    assert_eq!(
        seen,
        vec!["default", "0.0", "0.3", "0.5", "0.7", "1.0", "default"],
        "one decimal place, and `default` sends none"
    );
}

#[test]
fn a_value_the_menu_does_not_offer_cycles_back_into_the_list() {
    // An ALTER_ZERO_TEMPERATURE=0.42 override (or a hand-edited file) is shown
    // faithfully, but one press rejoins the offered cycle rather than wedging.
    let mut s = SessionSettings {
        temperature: Some(0.42),
        error_retry: 7,
        ..SessionSettings::default()
    };
    assert_eq!(s.value_text(SettingKey::Temperature, MANUAL), "0.4");
    assert_eq!(s.value_text(SettingKey::ErrorRetry, MANUAL), "7");
    s.cycle(SettingKey::Temperature);
    s.cycle(SettingKey::ErrorRetry);
    assert_eq!(s.temperature, TEMPERATURE_CHOICES[0]);
    assert_eq!(s.error_retry, RETRY_CHOICES[0]);
}

#[test]
fn the_permission_row_reads_the_mode_it_is_given_and_never_stores_one() {
    // The posture lives on `App` (Ctrl+A owns it) — the menu is a second door
    // onto that one state, so cycling here is a no-op and the caller routes
    // the change through Action::SetPermissionMode instead.
    let mut s = SessionSettings::default();
    assert_eq!(
        s.value_text(SettingKey::PermissionMode, Some(PermissionMode::Auto)),
        "auto"
    );
    assert!(
        !s.cycle(SettingKey::PermissionMode),
        "the pure blob has no mode to move"
    );
}

#[test]
fn the_permission_row_reads_disabled_and_unavailable_with_no_gate() {
    // ALTER_ZERO_PERMISSIONS=0 runs with no gate at all: App carries no mode,
    // so the row says so rather than pretending `manual` is in force.
    let s = SessionSettings::default();
    assert!(!s.is_available(SettingKey::PermissionMode, None));
    assert_eq!(
        s.value_text(SettingKey::PermissionMode, None),
        format!("disabled{UNAVAILABLE_SUFFIX}")
    );
    assert!(
        s.is_available(SettingKey::PermissionMode, MANUAL),
        "with a gate it is an ordinary row"
    );
}

#[test]
fn an_unavailable_checkpoint_row_shows_its_effective_value_not_the_stored_one() {
    // The stored preference stays `true` (it applies again in a project the
    // feature can serve), but this session's *effective* value is false.
    let s = SessionSettings {
        availability: SettingAvailability { checkpoints: false },
        ..SessionSettings::default()
    };
    assert!(s.checkpoints, "the preference survives");
    assert_eq!(
        s.value_text(SettingKey::Checkpoints, MANUAL),
        format!("false{UNAVAILABLE_SUFFIX}")
    );
}

#[test]
fn an_unavailable_setting_says_so_and_refuses_to_cycle() {
    // No git / no config home / a cwd checkpoints refuse: the row is honest
    // about it rather than pretending the toggle does something.
    let mut s = SessionSettings {
        availability: SettingAvailability { checkpoints: false },
        ..SessionSettings::default()
    };
    assert!(!s.is_available(SettingKey::Checkpoints, MANUAL));
    assert_eq!(
        s.value_text(SettingKey::Checkpoints, MANUAL),
        format!("false{UNAVAILABLE_SUFFIX}")
    );
    assert!(!s.cycle(SettingKey::Checkpoints));
    assert!(s.checkpoints, "the stored preference is left untouched");
    assert!(
        !s.checkpoints_active(),
        "the host's verdict wins over the preference"
    );
    // Every other row is unaffected.
    assert!(s.is_available(SettingKey::Tools, MANUAL));
    assert!(s.cycle(SettingKey::Tools));
}

#[test]
fn checkpoints_are_active_only_when_both_the_knob_and_the_host_agree() {
    let mut s = SessionSettings::default();
    assert!(s.checkpoints_active());
    s.cycle(SettingKey::Checkpoints);
    assert!(!s.checkpoints_active(), "the user turned them off");
}

#[test]
fn json_round_trips_every_changed_value() {
    let mut s = SessionSettings::default();
    for key in SettingKey::ALL {
        s.cycle(*key);
    }
    let restored = SessionSettings::parse(&s.to_json());
    assert_eq!(restored, s);
}

#[test]
fn defaults_stay_off_the_wire_and_a_partial_file_still_loads() {
    // Only what the user changed is written — the file reads as a diff from
    // the defaults, and an old/partial one fills the rest in.
    let json = SessionSettings::default().to_json();
    assert_eq!(
        json.trim(),
        "{}",
        "an untouched session writes nothing: {json}"
    );

    let one = SessionSettings {
        hide_thinking: true,
        ..SessionSettings::default()
    };
    let json = one.to_json();
    assert!(json.contains("hide_thinking"), "{json}");
    assert!(
        !json.contains("tools"),
        "unchanged fields are skipped: {json}"
    );

    let restored = SessionSettings::parse(r#"{"error_retry":10}"#);
    assert_eq!(restored.error_retry, 10);
    assert!(restored.tools, "the rest take their defaults");
}

#[test]
fn parse_of_garbage_or_unknown_keys_degrades_to_the_defaults() {
    // A corrupt or future file must never block startup (llm::Settings' posture).
    assert_eq!(SessionSettings::parse(""), SessionSettings::default());
    assert_eq!(
        SessionSettings::parse("not json at all"),
        SessionSettings::default()
    );
    let forward = SessionSettings::parse(r#"{"tools":false,"warp_drive":true}"#);
    assert!(!forward.tools, "the keys it knows still apply");
}

#[test]
fn copy_value_moves_exactly_one_setting_and_never_the_environment() {
    // The write path's whole point: an `ALTER_ZERO_*` override merged over the
    // file at startup must not be persisted when the user changes something
    // else. The boundary keeps the file's own blob and moves across only the
    // key that was cycled.
    let saved = SessionSettings::default();
    // What the session is running: the user cycled the retry count, and the
    // environment forced tools + checkpoints off for this run only.
    let live = SessionSettings {
        error_retry: 10,
        tools: false,
        checkpoints: false,
        ..SessionSettings::default()
    };
    let mut file = saved;
    file.copy_value(SettingKey::ErrorRetry, &live);
    assert_eq!(file.error_retry, 10, "the cycled key is saved");
    assert!(file.tools, "the env override did not stick");
    assert!(file.checkpoints, "…nor this one");
    // Cycling that row too *does* persist it — an explicit choice outranks the
    // override for later runs.
    file.copy_value(SettingKey::Tools, &live);
    assert!(!file.tools);
    // The posture isn't ours; it lives in permissions.json.
    let mut untouched = SessionSettings::default();
    untouched.copy_value(SettingKey::PermissionMode, &live);
    assert_eq!(untouched, SessionSettings::default());
}

#[test]
fn availability_is_never_persisted() {
    // It is a fact about the host, re-derived every run — writing it would let
    // one bad session teach the file a lie.
    let s = SessionSettings {
        availability: SettingAvailability { checkpoints: false },
        ..SessionSettings::default()
    };
    assert!(!s.to_json().contains("availability"), "{}", s.to_json());
    assert!(
        SessionSettings::parse(r#"{"availability":{"checkpoints":false}}"#)
            .availability
            .checkpoints,
        "a stale file can't disable a capable host"
    );
}
