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
    // is what the app did before the menu existed — with the deliberate
    // divergences pinned by their own tests below.
    let s = SessionSettings::default();
    assert!(!s.hide_thinking, "thinking showed by default");
    assert!(s.show_thinking());
    assert_eq!(s.error_retry, crate::llm::retry::MAX_RETRIES);
    assert!(s.tools, "tools were on by default");
    assert!(s.auto_compact);
    assert!(s.project_docs);
    assert_eq!(s.temperature, None, "no temperature is sent by default");
    // The app ships UNCAPPED where the library's backstop was 20 rounds —
    // see the dedicated test below and `docs/settings.md`.
    assert_eq!(s.max_tool_calls, 0);
}

#[test]
fn hooks_and_checkpoints_are_off_until_a_directory_turns_them_on() {
    // Both run *code* on the user's behalf — a hooks.json handler, a whole-cwd
    // `git add -A` before the first frame — so a directory opts in to each
    // rather than out (docs/per-directory-state.md). Turning one on is then
    // exactly what its entry records, and the defaults stay off the wire.
    let s = SessionSettings::default();
    assert!(!s.hooks, "hooks are off until turned on");
    assert!(!s.checkpoints, "checkpoints too");
    assert!(!s.hooks_active() && !s.checkpoints_active());
    assert_eq!(s.to_json().trim(), "{}");
    let mut on = s;
    assert!(on.cycle(SettingKey::Hooks));
    assert!(on.cycle(SettingKey::Checkpoints));
    assert!(on.hooks_active() && on.checkpoints_active());
    let json = on.to_json();
    assert!(json.contains("\"hooks\": true"), "{json}");
    assert!(json.contains("\"checkpoints\": true"), "{json}");
    assert_eq!(SessionSettings::parse(&json), on);
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
    // The posture lives on `App` (Shift+Tab owns it) — the menu is a second door
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
        checkpoints: true,
        availability: SettingAvailability {
            checkpoints: false,
            hooks: true,
            skills: true,
            images: true,
            telemetry: true,
        },
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
        checkpoints: true,
        availability: SettingAvailability {
            checkpoints: false,
            hooks: true,
            skills: true,
            images: true,
            telemetry: true,
        },
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
    assert!(!s.checkpoints_active(), "off until turned on");
    s.cycle(SettingKey::Checkpoints);
    assert!(s.checkpoints_active(), "the user turned them on");
    s.availability.checkpoints = false;
    assert!(!s.checkpoints_active(), "…but the host has the last word");
}

#[test]
fn json_round_trips_every_changed_value() {
    // Every knob settings.json owns. Telemetry is the one that lives
    // elsewhere (telemetry.json, docs/telemetry.md) — its own test pins that
    // it never reaches this file.
    let mut s = SessionSettings::default();
    for key in SettingKey::ALL
        .iter()
        .filter(|k| **k != SettingKey::Telemetry)
    {
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
    // environment forced tools off and checkpoints on for this run only.
    let live = SessionSettings {
        error_retry: 10,
        tools: false,
        checkpoints: true,
        ..SessionSettings::default()
    };
    let mut file = saved;
    file.copy_value(SettingKey::ErrorRetry, &live);
    assert_eq!(file.error_retry, 10, "the cycled key is saved");
    assert!(file.tools, "the env override did not stick");
    assert!(!file.checkpoints, "…nor this one");
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
        availability: SettingAvailability {
            checkpoints: false,
            hooks: true,
            skills: true,
            images: true,
            telemetry: true,
        },
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

#[test]
fn skills_are_not_offered_with_tools_off() {
    // The `skill` tool rides the tool set, so `/settings` → Tools = false
    // withdraws it — and the `<system-reminder>` listing has to go with it.
    // A context that advertises a tool the request never carries is a dead
    // end the model will spend a round hunting for (`docs/skills.md`).
    let mut s = SessionSettings {
        availability: SettingAvailability {
            checkpoints: true,
            hooks: true,
            skills: true,
            images: true,
            telemetry: true,
        },
        ..SessionSettings::default()
    };
    assert!(
        s.skills_active() && s.skills_offered(),
        "both on by default"
    );
    s.tools = false;
    assert!(
        s.skills_active(),
        "the Skills row keeps its own value — Tools is a separate knob"
    );
    assert!(!s.skills_offered(), "…but nothing is offered");
    // And with no skill installed there is nothing to offer either way.
    s.tools = true;
    s.availability.skills = false;
    assert!(!s.skills_offered());
}

// ===== per-directory settings (docs/per-directory-state.md) =====

#[test]
fn a_pre_directory_file_seeds_every_directory_that_has_no_entry() {
    // The old flat file is the new file's top level: what it says applies to
    // every directory until that directory changes something of its own.
    let file = SettingsFile::parse(r#"{"error_retry": 5, "hide_thinking": true}"#);
    let a = file.settings_for("/a");
    assert_eq!(a.error_retry, 5);
    assert!(a.hide_thinking);
    assert!(a.tools, "the rest take their defaults");
    assert!(file.projects.is_empty());
    assert_eq!(file.seed, a);
}

#[test]
fn a_directory_entry_outranks_the_seed_and_is_a_whole_blob() {
    // An entry is a diff from the DEFAULTS like the seed itself — never a layer
    // over the seed, which the diff format could not express (a `true` left
    // at its default is indistinguishable from one deliberately chosen).
    let file = SettingsFile::parse(r#"{"error_retry": 5, "projects": {"/a": {"tools": false}}}"#);
    let a = file.settings_for("/a");
    assert!(!a.tools);
    assert_eq!(
        a.error_retry, 3,
        "the entry says nothing about retries: the default"
    );
    let b = file.settings_for("/b");
    assert!(b.tools);
    assert_eq!(b.error_retry, 5, "no entry: the seed");
}

#[test]
fn record_value_moves_one_key_onto_the_directory_entry_seeded_from_the_file() {
    // The write path: take this directory's entry (else the seed), move across
    // only the key the user cycled — never the environment's overrides — and
    // keep it as the directory's own. The seed is never rewritten.
    let mut file = SettingsFile::parse(r#"{"error_retry": 5}"#);
    let live = SessionSettings {
        error_retry: 5,
        hide_thinking: true,
        tools: false, // an ALTER_ZERO_TOOLS=0 override, this run only
        ..SessionSettings::default()
    };
    file.record_value("/a", SettingKey::HideThinking, &live);
    let a = file.settings_for("/a");
    assert!(a.hide_thinking, "the cycled key is saved");
    assert_eq!(
        a.error_retry, 5,
        "the seed's value came along into the entry"
    );
    assert!(a.tools, "the env override did not stick");
    assert_eq!(file.seed.error_retry, 5);
    assert!(!file.seed.hide_thinking, "the seed is never rewritten");
    assert!(
        !file.settings_for("/b").hide_thinking,
        "another directory is untouched"
    );
    // A second cycle in the same directory builds on its own entry.
    let live = SessionSettings {
        error_retry: 10,
        hide_thinking: true,
        ..SessionSettings::default()
    };
    file.record_value("/a", SettingKey::ErrorRetry, &live);
    let a = file.settings_for("/a");
    assert_eq!(a.error_retry, 10);
    assert!(a.hide_thinking);
}

#[test]
fn an_entry_is_kept_even_when_it_returns_to_the_defaults() {
    // Dropping it would let the seed back in: a directory that cycled Error
    // retry from the file's 5 back to 3 has chosen 3.
    let mut file = SettingsFile::parse(r#"{"error_retry": 5}"#);
    file.record_value("/a", SettingKey::ErrorRetry, &SessionSettings::default());
    assert_eq!(file.settings_for("/a").error_retry, 3);
    let json = file.to_json();
    assert!(json.contains("\"/a\""), "{json}");
    assert_eq!(SettingsFile::parse(&json).settings_for("/a").error_retry, 3);
}

#[test]
fn settings_file_round_trips_and_an_empty_one_is_the_old_shape() {
    let mut file = SettingsFile::default();
    assert_eq!(file.to_json().trim(), "{}");
    file.record_value(
        "/home/u/a",
        SettingKey::Tools,
        &SessionSettings {
            tools: false,
            ..SessionSettings::default()
        },
    );
    file.record_value(
        "/home/u/b",
        SettingKey::Checkpoints,
        &SessionSettings {
            checkpoints: true,
            ..SessionSettings::default()
        },
    );
    let json = file.to_json();
    assert!(json.contains("\"projects\""), "{json}");
    let back = SettingsFile::parse(&json);
    assert_eq!(back, file);
    assert!(!back.settings_for("/home/u/a").tools);
    assert!(back.settings_for("/home/u/b").checkpoints);
    assert!(
        back.settings_for("/home/u/c").tools,
        "a third directory: the defaults"
    );
    // A corrupt or empty file must never block startup.
    assert_eq!(SettingsFile::parse(""), SettingsFile::default());
    assert_eq!(SettingsFile::parse("not json"), SettingsFile::default());
    // The seed survives beside the entries, byte-for-byte in meaning.
    let legacy = SettingsFile::parse(r#"{"auto_compact": false}"#);
    let mut legacy_plus = legacy.clone();
    legacy_plus.record_value("/x", SettingKey::Tools, &SessionSettings::default());
    let back = SettingsFile::parse(&legacy_plus.to_json());
    assert!(!back.seed.auto_compact);
    assert!(
        !back.settings_for("/y").auto_compact,
        "still seeds the next directory"
    );
}

// ===== the Telemetry row (docs/telemetry.md) =====

#[test]
fn the_telemetry_row_is_last_cycles_and_needs_a_config_home() {
    // Opt-out: on until turned off, and the last row so nothing above it
    // moves. Without a config home there is nowhere to keep an install id, so
    // the row reads unavailable like every knob the host can't serve.
    let mut s = SessionSettings::default();
    assert!(s.telemetry, "on by default");
    assert!(s.telemetry_active());
    assert_eq!(SettingKey::ALL.last(), Some(&SettingKey::Telemetry));
    assert_eq!(s.value_text(SettingKey::Telemetry, MANUAL), "true");
    assert!(s.cycle(SettingKey::Telemetry));
    assert!(!s.telemetry && !s.telemetry_active());
    assert_eq!(s.value_text(SettingKey::Telemetry, MANUAL), "false");
    assert!(s.cycle(SettingKey::Telemetry));
    assert!(s.telemetry, "a boolean: back on");
    s.availability.telemetry = false;
    assert!(!s.is_available(SettingKey::Telemetry, MANUAL));
    assert!(!s.telemetry_active(), "a stored yes the host can't honour");
    assert_eq!(
        s.value_text(SettingKey::Telemetry, MANUAL),
        format!("false{UNAVAILABLE_SUFFIX}"),
        "the effective value, not the stored one"
    );
    assert!(!s.cycle(SettingKey::Telemetry), "unavailable rows refuse");
    assert!(s.telemetry, "…and leave the value alone");
}

#[test]
fn telemetry_never_reaches_settings_json() {
    // It is a user preference, not a project's (docs/per-directory-state.md):
    // it lives in telemetry.json beside the install id. So the blob never
    // serializes it, a settings.json that names it is ignored, and
    // `copy_value` never moves it — the PermissionMode pattern.
    let off = SessionSettings {
        telemetry: false,
        ..SessionSettings::default()
    };
    assert_eq!(off.to_json().trim(), "{}", "not written");
    assert!(
        SessionSettings::parse(r#"{"telemetry": false}"#).telemetry,
        "not read"
    );
    let mut file = SessionSettings::default();
    file.copy_value(SettingKey::Telemetry, &off);
    assert!(file.telemetry, "not this file's to record");
    assert_eq!(file, SessionSettings::default());
}
