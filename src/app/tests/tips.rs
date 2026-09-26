//! The usage tips under the status line: the walk through the catalog, its
//! timing, what it skips, and the file that carries it across launches
//! (`docs/tips.md`).

use super::*;
use crate::app::{TIP_DELAY, TIP_ROTATION, TIPS, Tip, TipFeature, TipsFile, tip_at};
use unicode_width::UnicodeWidthStr;

/// Display columns of `text` — the width rule every row is measured by.
fn cols(text: &str) -> usize {
    text.width()
}

/// Inject `elapsed` the way the boundary does before every draw, and read
/// the tip the strip then shows.
fn tip_at_elapsed(app: &mut App, elapsed: Duration) -> Option<&'static Tip> {
    app.set_status_times(elapsed, None);
    app.tip()
}

/// The catalog index of the tip with `id`.
fn index_of(id: &str) -> usize {
    TIPS.iter()
        .position(|tip| tip.id == id)
        .unwrap_or_else(|| panic!("no tip {id:?} in the catalog"))
}

#[test]
fn no_tip_shows_until_the_delay_has_passed() {
    // A quick answer never shows one: the row would flash under a status
    // line about to vanish, and the tip is background reading for a wait.
    let mut app = App::new();
    app.begin_stream();
    assert_eq!(app.tip(), None, "nothing before the first frame");
    assert_eq!(
        tip_at_elapsed(&mut app, TIP_DELAY - Duration::from_millis(1)),
        None,
        "still nothing a hair short of the delay"
    );
    let first = tip_at_elapsed(&mut app, TIP_DELAY).expect("a tip once the delay passes");
    assert_eq!(
        first.id, TIPS[0].id,
        "a fresh session opens on the first tip"
    );
}

#[test]
fn the_tip_moves_on_every_rotation() {
    let mut app = App::new();
    app.begin_stream();
    let first = tip_at_elapsed(&mut app, TIP_DELAY).unwrap();
    assert_eq!(
        tip_at_elapsed(
            &mut app,
            TIP_DELAY + TIP_ROTATION - Duration::from_millis(1)
        )
        .unwrap(),
        first,
        "the first tip holds for the whole rotation"
    );
    let second = tip_at_elapsed(&mut app, TIP_DELAY + TIP_ROTATION).unwrap();
    assert_eq!(second.id, TIPS[1].id, "then the next one takes over");
    let third = tip_at_elapsed(&mut app, TIP_DELAY + TIP_ROTATION * 2).unwrap();
    assert_eq!(third.id, TIPS[2].id);
}

#[test]
fn the_walk_wraps_around_the_catalog() {
    // A lap is over the tips that *apply* here: the unit-test app has no
    // permission gate, no thinking model and no skills, so the walk steps
    // over their tips (`a_tip_whose_feature_is_off_is_skipped`).
    let mut app = App::new();
    app.begin_stream();
    let applicable = TIPS.iter().filter(|tip| app.tip_applies(tip)).count();
    assert!(applicable < TIPS.len(), "some tips are gated here");
    let lap = TIP_ROTATION * u32::try_from(applicable).expect("a short catalog");
    assert_eq!(
        tip_at_elapsed(&mut app, TIP_DELAY + lap - TIP_ROTATION)
            .unwrap()
            .id,
        TIPS[TIPS.len() - 1].id,
        "the lap's last step is the catalog's last tip"
    );
    assert_eq!(
        tip_at_elapsed(&mut app, TIP_DELAY + lap).unwrap().id,
        TIPS[0].id,
        "one lap later the first tip is back"
    );
}

#[test]
fn the_next_turn_opens_on_the_tip_after_the_last_shown() {
    // The walk continues across turns, exactly like the status verbs: no
    // turn repeats the tip the previous one ended on.
    let mut app = App::new();
    app.begin_stream();
    tip_at_elapsed(&mut app, TIP_DELAY + TIP_ROTATION);
    app.finish_stream();
    app.end_turn(200);
    app.begin_stream();
    assert_eq!(app.tip(), None, "a new turn waits out the delay again");
    assert_eq!(
        tip_at_elapsed(&mut app, TIP_DELAY).unwrap().id,
        TIPS[2].id,
        "the turn opens on the tip after the one the last turn ended on"
    );
}

#[test]
fn a_turn_that_showed_no_tip_leaves_the_walk_where_it_was() {
    let mut app = App::new();
    app.begin_stream();
    tip_at_elapsed(&mut app, Duration::from_secs(1)); // under the delay
    app.finish_stream();
    app.end_turn(1);
    app.begin_stream();
    assert_eq!(
        tip_at_elapsed(&mut app, TIP_DELAY).unwrap().id,
        TIPS[0].id,
        "nothing was shown, so nothing moved"
    );
}

#[test]
fn the_tip_dies_with_the_turn() {
    let mut app = App::new();
    app.begin_stream();
    assert!(tip_at_elapsed(&mut app, TIP_DELAY).is_some());
    app.finish_stream();
    app.end_turn(10);
    assert_eq!(app.tip(), None, "the status is gone, and the tip with it");
}

#[test]
fn a_shell_turn_shows_no_tip() {
    // A `!` shell turn has no status line to hang a tip from
    // (docs/shell-command.md).
    let mut app = App::new();
    app.begin_shell("ls");
    assert_eq!(tip_at_elapsed(&mut app, TIP_DELAY + TIP_ROTATION), None);
}

#[test]
fn a_seeded_last_tip_puts_the_walk_after_it() {
    // The boundary seeds the walk from `tips.json` at launch, so a relaunch
    // never repeats the tip the previous session ended on.
    let mut app = App::new();
    let last = TIPS[4].id;
    app.seed_tips(Some(last));
    app.begin_stream();
    assert_eq!(tip_at_elapsed(&mut app, TIP_DELAY).unwrap().id, TIPS[5].id);
    // The catalog's last tip hands the walk back to its first.
    let mut app = App::new();
    app.seed_tips(Some(TIPS[TIPS.len() - 1].id));
    app.begin_stream();
    assert_eq!(tip_at_elapsed(&mut app, TIP_DELAY).unwrap().id, TIPS[0].id);
}

#[test]
fn an_unknown_or_absent_seed_starts_at_the_first_tip() {
    for seed in [None, Some("a-tip-this-build-never-had")] {
        let mut app = App::new();
        app.seed_tips(seed);
        app.begin_stream();
        assert_eq!(
            tip_at_elapsed(&mut app, TIP_DELAY).unwrap().id,
            TIPS[0].id,
            "seed {seed:?}"
        );
    }
}

#[test]
fn every_shown_tip_is_reported_once_for_the_boundary_to_record() {
    let mut app = App::new();
    app.begin_stream();
    assert_eq!(app.take_tip_record(), None, "nothing shown yet");
    tip_at_elapsed(&mut app, TIP_DELAY);
    assert_eq!(app.take_tip_record(), Some(TIPS[0].id));
    assert_eq!(app.take_tip_record(), None, "taken once");
    // A frame that keeps the same tip reports nothing; the rotation does.
    tip_at_elapsed(&mut app, TIP_DELAY + Duration::from_secs(1));
    assert_eq!(app.take_tip_record(), None);
    tip_at_elapsed(&mut app, TIP_DELAY + TIP_ROTATION);
    assert_eq!(app.take_tip_record(), Some(TIPS[1].id));
}

#[test]
fn a_tip_whose_feature_is_off_is_skipped() {
    // The Shift+Tab tip is about the permission gate; a session running with
    // `ALTER_ZERO_PERMISSIONS=0` has no gate to cycle, so the walk steps over
    // it (Claude Code's `isRelevant`).
    let shift_tab = index_of("permission-mode");
    let mut app = App::new();
    assert_eq!(app.permission_mode(), None, "the unit-test app has no gate");
    app.seed_tips(Some(TIPS[shift_tab - 1].id));
    app.begin_stream();
    let shown = tip_at_elapsed(&mut app, TIP_DELAY).unwrap();
    assert_ne!(shown.id, "permission-mode");
    assert_eq!(shown.id, TIPS[shift_tab + 1].id, "the next applicable tip");
    // With a gate the same seed lands on it.
    let mut app = App::new();
    app.set_permission_mode(Some(crate::permission::PermissionMode::Manual));
    app.seed_tips(Some(TIPS[shift_tab - 1].id));
    app.begin_stream();
    assert_eq!(
        tip_at_elapsed(&mut app, TIP_DELAY).unwrap().id,
        "permission-mode"
    );
}

#[test]
fn the_gated_tips_name_the_feature_they_need() {
    // Each gate is a fact about the session the pure core already holds.
    let needs = |id: &str| TIPS[index_of(id)].needs;
    assert_eq!(needs("permission-mode"), TipFeature::Permissions);
    assert_eq!(needs("thinking-mode"), TipFeature::Thinking);
    assert_eq!(needs("skill-mentions"), TipFeature::Skills);
    assert_eq!(needs("skills-menu"), TipFeature::Skills);
    assert_eq!(needs("compact"), TipFeature::Always);
    // A thinking model unlocks the Ctrl+T tip; skills on the `$` picker's
    // snapshot unlock theirs.
    let mut app = App::new();
    assert!(!app.tip_applies(&TIPS[index_of("thinking-mode")]));
    app.set_thinking(Some((
        trio_support(),
        crate::llm::ThinkingMode::Effort(crate::llm::ReasoningEffort::Medium),
    )));
    assert!(app.tip_applies(&TIPS[index_of("thinking-mode")]));
    assert!(!app.tip_applies(&TIPS[index_of("skill-mentions")]));
    app.set_skills(vec![crate::skills::SkillMetadata {
        name: "dataviz".to_string(),
        description: "charts".to_string(),
        dir: std::path::PathBuf::from("/skills/dataviz"),
        path: std::path::PathBuf::from("/skills/dataviz/SKILL.md"),
    }]);
    assert!(app.tip_applies(&TIPS[index_of("skill-mentions")]));
}

#[test]
fn tips_off_shows_none_and_moves_nothing() {
    // The `/settings` **Tips** row (and `ALTER_ZERO_TIPS=0`): no row, and
    // the walk stands still so turning it back on resumes where it was.
    let mut app = App::new();
    app.settings_mut().tips = false;
    app.begin_stream();
    assert_eq!(tip_at_elapsed(&mut app, TIP_DELAY + TIP_ROTATION), None);
    assert_eq!(app.take_tip_record(), None);
    app.settings_mut().tips = true;
    assert_eq!(
        tip_at_elapsed(&mut app, TIP_DELAY).unwrap().id,
        TIPS[0].id,
        "the walk had not moved: this turn still opens on the first tip"
    );
    assert_eq!(app.take_tip_record(), Some(TIPS[0].id));
}

#[test]
fn a_compact_turn_shows_tips_too() {
    // A summarization turn is a wait like any other.
    let mut app = App::new();
    app.begin_compact(false);
    assert!(tip_at_elapsed(&mut app, TIP_DELAY).is_some());
}

#[test]
fn every_tip_has_a_unique_id_and_fits_one_row_at_eighty_columns() {
    let mut ids = std::collections::HashSet::new();
    for tip in TIPS {
        assert!(ids.insert(tip.id), "duplicate tip id {:?}", tip.id);
        assert!(
            tip.id
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
            "tip id {:?} is not a slug",
            tip.id
        );
        // `  ⎿  Tip: ` is ten columns; the text fits beside it on an
        // 80-column terminal without wrapping.
        assert!(
            cols(tip.text) <= 70,
            "tip {:?} is {} columns wide: {:?}",
            tip.id,
            cols(tip.text),
            tip.text
        );
        assert!(
            !tip.text.ends_with('.'),
            "no closing period: {:?}",
            tip.text
        );
        assert!(
            !tip.text.starts_with("Tip"),
            "the prefix is added on render: {:?}",
            tip.text
        );
    }
}

#[test]
fn tip_at_is_the_catalog_wrapped() {
    assert_eq!(tip_at(0).id, TIPS[0].id);
    assert_eq!(tip_at(TIPS.len()).id, TIPS[0].id);
    assert_eq!(tip_at(TIPS.len() + 2).id, TIPS[2].id);
}

#[test]
fn the_tips_file_round_trips_and_reads_a_bad_file_as_empty() {
    let file = TipsFile {
        last: Some("compact".to_string()),
    };
    let json = file.to_json();
    assert!(json.contains("\"last\": \"compact\""), "{json}");
    assert_eq!(TipsFile::parse(&json), file);
    assert_eq!(TipsFile::parse(""), TipsFile::default());
    assert_eq!(TipsFile::parse("{not json"), TipsFile::default());
    assert_eq!(
        TipsFile::parse(r#"{"last": 7}"#),
        TipsFile::default(),
        "a wrong type reads as nothing known"
    );
    assert_eq!(
        TipsFile::default().to_json(),
        "{}",
        "nothing known writes an empty object"
    );
}
