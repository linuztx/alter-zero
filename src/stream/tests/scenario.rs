//! The scenario registry: how a prompt picks the demo the dummy plays
//! (`docs/dummy-backend.md`).

use super::super::dummy::scenario::{Cue, Play, SCENARIOS, select};
use super::super::dummy::script::HANDOFF;
use super::*;

/// A prompt that must select each [`SCENARIOS`] entry, in the same order.
///
/// This is the registry's coupling to its tests: adding a scenario without an
/// example fails [`every_scenario_is_reachable_by_its_example`] on the length
/// check, and an example that lands on an *earlier* entry — the shadowing a
/// hand-written `if`/`else` chain used to hide — fails it on the name.
const EXAMPLES: &[&str] = &[
    "ask me some questions",
    "auto permission demo",
    "parallel permission demo",
    "staggered permission demo",
    "permission demo",
    crate::context::SUMMARIZATION_PROMPT,
    "show me a table",
    "call agents for weather",
    "run three pings in parallel",
    "show me a diff",
    "hello there",
];

/// Select for `prompt` as the dummy would with both gates attached (the
/// permission gate and the ask gate — the app's default posture).
fn gated(prompt: &str) -> &'static str {
    select(&Cue::new(prompt, 0), true, true).name
}

/// Select for `prompt` as the dummy would with no gate — the `turn_events`
/// path, where the gated and asked demos are unreachable.
fn ungated(prompt: &str) -> &'static str {
    select(&Cue::new(prompt, 0), false, false).name
}

#[test]
fn every_scenario_is_reachable_by_its_example() {
    // The registry is ordered, so an entry can be shadowed by an earlier one
    // whose cue is a superset — "staggered permission demo" also mentions
    // "permission". Each entry's example must select *it*, which is what
    // makes adding one safe: a shadowed scenario fails here instead of
    // silently never playing again.
    assert_eq!(
        EXAMPLES.len(),
        SCENARIOS.len(),
        "every scenario needs an example prompt that selects it"
    );
    for (example, scenario) in EXAMPLES.iter().zip(SCENARIOS) {
        assert_eq!(
            gated(example),
            scenario.name,
            "{}'s example prompt selects a different scenario",
            scenario.name,
        );
    }
}

#[test]
fn scenario_names_are_unique() {
    // The name is the registry's key — in the docs, in these tests, and in
    // the smoke suite's prompts. A duplicate makes the reachability test
    // above pass while the wrong entry plays.
    let mut names: Vec<&str> = SCENARIOS.iter().map(|s| s.name).collect();
    names.sort_unstable();
    let count = names.len();
    names.dedup();
    assert_eq!(count, names.len(), "duplicate scenario name: {names:?}");
}

#[test]
fn selection_always_resolves_to_a_scenario() {
    // `select` is total: the last entry matches anything, so no prompt can
    // fall off the end of the registry. (The empty prompt is the extreme.)
    for prompt in ["", "hello there", "!@#$", "tell me about compaction"] {
        assert!(
            SCENARIOS.iter().any(|s| s.name == ungated(prompt)),
            "{prompt:?} resolved outside the registry"
        );
    }
}

#[test]
fn the_last_scenario_really_catches_everything() {
    // `select` ends in `unwrap_or(last)`, and `turn_events` documents its
    // gated arm as unreachable. Both rest on two properties of the last
    // entry that nothing at the call site can see: it matches **any** cue,
    // and it is a `Script`, so a gate-free session can always play it.
    //
    // The fallback actively hides the first one — narrow that entry's cue and
    // `select` keeps returning it, now for prompts it just rejected, with
    // every other test still green. So assert both here, where breaking
    // either names the reason.
    let last = SCENARIOS.last().expect("the registry is never empty");
    for prompt in [
        "",
        "hello there",
        "!@#$",
        crate::context::SUMMARIZATION_PROMPT,
    ] {
        assert!(
            (last.selects)(&Cue::new(prompt, 0)),
            "the catch-all rejected {prompt:?} — `select`'s fallback would \
             return it anyway, silently answering a cue it does not match"
        );
    }
    assert!(
        matches!(last.play, Play::Script(_)),
        "the catch-all must be playable with no gate attached"
    );
}

#[test]
fn a_gated_scenario_is_only_selected_when_a_gate_is_attached() {
    // The permission demos block on the gate, so they are unreachable
    // without one: the same prompt then falls through to a scripted turn.
    // This is what keeps `turn_events` — which has no gate — able to answer
    // *any* prompt (docs/permissions.md).
    for prompt in [
        "auto permission demo",
        "parallel permission demo",
        "staggered permission demo",
        "permission demo",
    ] {
        assert!(
            matches!(
                select(&Cue::new(prompt, 0), true, true).play,
                Play::Gated(_)
            ),
            "{prompt:?} should reach a gated demo when a gate is attached"
        );
        assert!(
            matches!(
                select(&Cue::new(prompt, 0), false, false).play,
                Play::Script(_)
            ),
            "{prompt:?} must fall through to a script with no gate attached"
        );
    }
}

#[test]
fn the_ask_demo_is_only_selected_when_an_ask_gate_is_attached() {
    // The ask demo blocks on the ask gate (`docs/ask.md`) — without one it
    // would park forever, so selection skips it and the prompt falls through
    // to a scripted turn, exactly like the permission demos above.
    let prompt = "ask me some questions";
    assert!(
        matches!(
            select(&Cue::new(prompt, 0), false, true).play,
            Play::Asked(_)
        ),
        "{prompt:?} should reach the ask demo when the ask gate is attached \
         (the permission gate is irrelevant to it)"
    );
    assert!(
        matches!(
            select(&Cue::new(prompt, 0), true, false).play,
            Play::Script(_)
        ),
        "{prompt:?} must fall through to a script with no ask gate attached"
    );
}

#[test]
fn the_cue_matches_words_case_insensitively_but_markers_exactly() {
    // Word cues ("table", "agents") are matched against the lowercased
    // prompt, so the user can type however they like; the `/compact`
    // request is recognized by an exact leading marker, because it is
    // generated text and a fuzzy match there would hijack a real question
    // about the same subject.
    let cue = Cue::new("Show Me A TABLE", 0);
    assert!(cue.mentions("table"));
    assert!(!cue.starts_with("Show me"), "the marker match is exact");
    assert!(cue.starts_with("Show Me"));
    assert_eq!(gated("SHOW ME A TABLE"), "table");
}

#[test]
fn every_user_facing_script_acknowledges_attached_images() {
    // The dummy has no vision, so a turn carrying Ctrl+V images opens by
    // acknowledging them — visible proof the typed image channel reached
    // the backend (docs/image-paste.md). Every scripted scenario the user
    // can talk to must do it; the one exception is the `/compact`
    // summarization request, which the loop builds itself and never
    // attaches images to. A new scenario that forgets fails here.
    let mut checked = 0;
    for (example, scenario) in EXAMPLES.iter().zip(SCENARIOS) {
        let Play::Script(script) = scenario.play else {
            continue; // a gated demo streams live; it has no script to inspect
        };
        if scenario.name == "compact" {
            continue;
        }
        let events = script(&Cue::new(example, 2));
        assert!(
            chunk_text(&events).starts_with("Looking at your 2 images. "),
            "{} does not acknowledge attached images",
            scenario.name,
        );
        checked += 1;
    }
    assert!(checked >= 3, "the walk found the scripted scenarios");
}

#[test]
fn every_user_facing_script_hands_the_user_off_to_a_real_model() {
    // A session on the dummy has no model behind it, and the two commands
    // that fix that are `/login` and `/model`. Whichever demo the user
    // stumbles into must say so — otherwise the offline first run is a dead
    // end that never explains itself (`docs/dummy-backend.md`). The
    // `/compact` request is exempt for the same reason as the image
    // acknowledgement: the loop builds it, and its summary is never rendered.
    for (example, scenario) in EXAMPLES.iter().zip(SCENARIOS) {
        let Play::Script(script) = scenario.play else {
            continue; // a gated demo streams live; it has no script to inspect
        };
        if scenario.name == "compact" {
            continue;
        }
        let text = chunk_text(&script(&Cue::new(example, 0)));
        assert!(
            text.trim_end().ends_with(HANDOFF),
            "{} doesn't close on the /login → /model hand-off: {text}",
            scenario.name,
        );
    }
}

#[test]
fn the_init_prompt_never_selects_the_file_change_demo() {
    // `/init` submits a canned prompt about authoring AGENTS.md as an
    // ordinary user turn (`docs/init.md`). The file-change demo's cue is the
    // vocabulary that prompt lives in, so — like the `agents.md` guard above
    // it — this pins that the prompt still falls through to the default turn
    // instead of being answered with a scripted `Write` of fizzbuzz.
    assert_eq!(gated(crate::app::INIT_PROMPT), "tools");
}

#[test]
fn turn_events_plays_the_scenario_the_registry_selects() {
    // `turn_events` is the ungated entry point, so it must agree with the
    // registry rather than keep a second copy of the dispatch — the drift
    // this refactor removes.
    for prompt in [
        "hello there",
        "show me a table",
        "run three pings in parallel",
        "call agents for weather",
        crate::context::SUMMARIZATION_PROMPT,
    ] {
        let cue = Cue::new(prompt, 0);
        let Play::Script(script) = select(&cue, false, false).play else {
            panic!("{prompt:?} selected a gated demo with no gate attached");
        };
        assert_eq!(
            turn_events(prompt, 0),
            script(&cue),
            "turn_events disagrees with the registry for {prompt:?}"
        );
    }
}
