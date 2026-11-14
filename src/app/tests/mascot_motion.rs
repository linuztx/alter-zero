//! The banner mascot's motion: its stills, its schedule, and when the
//! session lets it move (`docs/mascot-motion.md`).

use super::*;

use unicode_width::UnicodeWidthStr;

/// Seeds the schedule tests run over — the unseeded default among them.
const SEEDS: [u64; 6] = [0, 1, 7, 42, 0xDEAD_BEEF, u64::MAX];

/// How many bars a schedule test walks.
const BARS: u32 = 60;

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

/// Display columns of `s` (the `ui::wrap::cols` measure, private to `ui`).
fn cols(s: &str) -> usize {
    s.width()
}

/// The glyph at (`row`, `col`) of `art`, a space past a short row's end.
fn glyph(art: &[&str], row: usize, col: usize) -> char {
    art[row].chars().nth(col).unwrap_or(' ')
}

/// The cells where `a` and `b` differ, as (row, column).
fn changed_cells(a: &[&str], b: &[&str]) -> Vec<(usize, usize)> {
    let mut cells = Vec::new();
    for row in 0..a.len().max(b.len()) {
        let width = a[row].chars().count().max(b[row].chars().count());
        for col in 0..width {
            if glyph(a, row, col) != glyph(b, row, col) {
                cells.push((row, col));
            }
        }
    }
    cells
}

/// The cells a blink closes: the two eyes — or, for the gem, its one facet.
fn eyes(mascot: Mascot) -> Vec<(usize, usize)> {
    match mascot {
        Mascot::Crest | Mascot::Bloom | Mascot::Twin | Mascot::Sprout => vec![(1, 2), (1, 6)],
        Mascot::Skiter => vec![(0, 2), (0, 6)],
        Mascot::Gem => vec![(1, 4)],
    }
}

/// Every pose the timeline visits in `[from, from + span)`, with the moment
/// each begins — walked the way the boundary does, one `holds` at a time.
fn walk(mascot: Mascot, seed: u64, from: Duration, span: Duration) -> Vec<(Duration, MascotPose)> {
    let mut visited = Vec::new();
    let mut at = from;
    while at < from + span {
        let motion = mascot_motion(mascot, seed, at);
        assert!(motion.holds > Duration::ZERO, "a pose holds for a while");
        visited.push((at, motion.pose));
        at += motion.holds;
    }
    visited
}

/// The poses of `stills`, in order.
fn poses(stills: &[Still]) -> Vec<MascotPose> {
    stills.iter().map(|still| still.pose).collect()
}

/// `poses` with every run of equal neighbours folded into one.
fn folded(poses: Vec<MascotPose>) -> Vec<MascotPose> {
    let mut out: Vec<MascotPose> = Vec::new();
    for pose in poses {
        if out.last() != Some(&pose) {
            out.push(pose);
        }
    }
    out
}

/// When bar `bar`'s act begins, measured from the timeline's start: a bar
/// opens at rest, and that rest holds until the act.
fn act_start(mascot: Mascot, seed: u64, bar: u32) -> Duration {
    let opens = MOTION_BAR * bar;
    let motion = mascot_motion(mascot, seed, opens);
    assert_eq!(
        motion.pose,
        MascotPose::rest(mascot),
        "{} bar {bar} opens at rest",
        mascot.name()
    );
    opens + motion.holds
}

// ===== the stills =====

#[test]
fn every_still_keeps_the_art_blocks_shape() {
    // A pose is drawn in the resting art's place, so it must fit there: the
    // same rows, never wider than the block, single-width block glyphs only
    // (a wide glyph shears the row — docs/table-streaming.md "Wide glyphs").
    for mascot in Mascot::ALL {
        let rest = mascot.art();
        for gesture in Gesture::ALL {
            let stills = mascot.stills(gesture);
            assert!(!stills.is_empty(), "{} has no {gesture:?}", mascot.name());
            for still in &stills {
                assert!(still.hold > Duration::ZERO);
                assert_eq!(
                    still.pose.art.len(),
                    rest.len(),
                    "{} {gesture:?}: a still's row count",
                    mascot.name()
                );
                for row in still.pose.art {
                    assert!(
                        cols(row) <= mascot.art_width(),
                        "{} {gesture:?}: {row:?} overflows the art block",
                        mascot.name()
                    );
                    assert_eq!(
                        cols(row),
                        row.chars().count(),
                        "{} {gesture:?}: {row:?} holds a wide glyph",
                        mascot.name()
                    );
                    assert!(
                        row.chars()
                            .all(|c| c == ' ' || ('\u{2580}'..='\u{259F}').contains(&c)),
                        "{} {gesture:?}: {row:?} leaves the block elements",
                        mascot.name()
                    );
                }
            }
        }
    }
}

#[test]
fn a_gesture_never_ends_on_a_still_that_is_the_resting_art() {
    // The timeline returns to rest after a gesture by itself; a trailing
    // resting still would only pad the act.
    for mascot in Mascot::ALL {
        for gesture in [Gesture::Blink, Gesture::Glance] {
            let stills = mascot.stills(gesture);
            let last = stills.last().expect("stills");
            assert_ne!(
                last.pose,
                MascotPose::rest(mascot),
                "{} {gesture:?}",
                mascot.name()
            );
            assert_ne!(
                stills[0].pose,
                MascotPose::rest(mascot),
                "{} {gesture:?} opens on a move",
                mascot.name()
            );
        }
    }
}

#[test]
fn a_blink_closes_every_eye_and_touches_nothing_else() {
    for mascot in Mascot::ALL {
        let rest = mascot.art();
        let stills = mascot.stills(Gesture::Blink);
        for still in &stills {
            assert_eq!(still.pose.glint, None);
            assert_eq!(
                changed_cells(rest, still.pose.art),
                eyes(mascot),
                "{}: a blink moves only the eyes",
                mascot.name()
            );
        }
        assert!(
            stills
                .iter()
                .any(|still| eyes(mascot).iter().all(|&(row, col)| glyph(
                    still.pose.art,
                    row,
                    col
                ) == '█')),
            "{}: the eyes never close",
            mascot.name()
        );
        let whole: Duration = stills.iter().map(|still| still.hold).sum();
        assert!(
            whole <= ms(250),
            "{}: a blink is quick, not {whole:?}",
            mascot.name()
        );
    }
}

#[test]
fn an_upper_half_eye_closes_through_a_lid() {
    // Where the eye is the cell's upper half, the lid rises through `▆`
    // before it shuts and after — an eyelid, not a cut.
    for mascot in [Mascot::Crest, Mascot::Bloom, Mascot::Twin, Mascot::Gem] {
        let (row, col) = eyes(mascot)[0];
        let seen: Vec<char> = mascot
            .stills(Gesture::Blink)
            .iter()
            .map(|still| glyph(still.pose.art, row, col))
            .collect();
        assert_eq!(seen, ['▆', '█', '▆'], "{}", mascot.name());
    }
}

#[test]
fn a_glance_looks_one_way_then_the_other() {
    for mascot in Mascot::ALL {
        let rest = mascot.art();
        let stills = mascot.stills(Gesture::Glance);
        let looks: Vec<&Still> = stills
            .iter()
            .filter(|still| still.pose != MascotPose::rest(mascot))
            .collect();
        assert_eq!(looks.len(), 2, "{}: two looks", mascot.name());
        assert_ne!(looks[0].pose, looks[1].pose, "{}", mascot.name());
        for look in &looks {
            assert!(
                look.hold >= ms(150),
                "{}: a look is held long enough to read",
                mascot.name()
            );
            // The eyes move, and — the gem's facet slides a cell — nothing
            // outside the eyes' own row.
            let row = eyes(mascot)[0].0;
            assert!(
                changed_cells(rest, look.pose.art)
                    .iter()
                    .all(|&(r, _)| r == row),
                "{}: a glance moves only the eye row",
                mascot.name()
            );
        }
        assert!(
            stills
                .iter()
                .any(|still| still.pose == MascotPose::rest(mascot)),
            "{}: the eyes pass through centre between the two looks",
            mascot.name()
        );
    }
}

#[test]
fn every_signature_is_the_mascots_own() {
    // Each mascot's signature moves its art — the gem's is light instead.
    let mut seen: Vec<Vec<MascotPose>> = Vec::new();
    for mascot in Mascot::ALL {
        let stills = mascot.stills(Gesture::Signature);
        if mascot == Mascot::Gem {
            assert!(stills.iter().all(|still| still.pose.art == mascot.art()));
            assert!(stills.iter().all(|still| still.pose.glint.is_some()));
        } else {
            assert!(
                stills.iter().all(|still| still.pose.glint.is_none()),
                "{}",
                mascot.name()
            );
            assert!(
                stills
                    .iter()
                    .any(|still| still.pose != MascotPose::rest(mascot)),
                "{}: the signature moves nothing",
                mascot.name()
            );
            // …and not the eyes: that is what a blink and a glance are for.
            for still in &stills {
                assert!(
                    changed_cells(mascot.art(), still.pose.art)
                        .iter()
                        .all(|cell| !eyes(mascot).contains(cell)),
                    "{}: the signature moves an eye",
                    mascot.name()
                );
            }
        }
        let poses = poses(&stills);
        assert!(!seen.contains(&poses), "{}: a borrowed move", mascot.name());
        seen.push(poses);
    }
}

/// Whether `pose`'s glint lights any glyph of its art.
fn lights_something(pose: &MascotPose) -> bool {
    (0..pose.art.len()).any(|row| (0..9).any(|col| pose.light(col, row) > 0.0))
}

#[test]
fn a_glint_is_a_leaning_band_three_cells_wide() {
    // The skitterer's upper rows are solid, so every cell under the band
    // shows it.
    let pose = MascotPose {
        art: Mascot::Skiter.art(),
        glint: Some(4),
    };
    // On the middle row the band is centred on the pose's column: full
    // under it, half a column off, nothing two off.
    assert_eq!(pose.light(4, 1), 1.0);
    assert_eq!(pose.light(3, 1), 0.5);
    assert_eq!(pose.light(5, 1), 0.5);
    assert_eq!(pose.light(2, 1), 0.0);
    assert_eq!(pose.light(6, 1), 0.0);
    // It leans like a `/`: a column further right on the row above, a
    // column further left on the row below.
    assert_eq!(pose.light(5, 0), 1.0);
    assert_eq!(pose.light(3, 2), 1.0);
    // Light shows on ink: a blank cell under the band is not lit…
    assert_eq!(glyph(pose.art, 2, 4), ' ');
    assert_eq!(pose.light(4, 2), 0.0);
    // …nor is anything past the art, or with no glint passing.
    assert_eq!(pose.light(9, 1), 0.0);
    assert_eq!(pose.light(4, 3), 0.0);
    assert_eq!(MascotPose::rest(Mascot::Skiter).light(4, 1), 0.0);
}

#[test]
fn a_glint_crosses_the_resting_art_from_edge_to_edge() {
    for mascot in Mascot::ALL {
        let stills = mascot.stills(Gesture::Glint);
        let centres: Vec<i8> = stills
            .iter()
            .map(|still| {
                assert_eq!(still.pose.art, mascot.art(), "a glint moves no glyph");
                assert!(
                    lights_something(&still.pose),
                    "{}: a still that lights nothing is a frame drawn for nothing",
                    mascot.name()
                );
                still.pose.glint.expect("a glint still carries its centre")
            })
            .collect();
        // One column a still, never stalling, from the first position that
        // lights a glyph to the last.
        assert!(
            centres.windows(2).all(|pair| pair[1] == pair[0] + 1),
            "{centres:?}"
        );
        for outside in [centres[0] - 1, centres[centres.len() - 1] + 1] {
            assert!(
                !lights_something(&MascotPose {
                    art: mascot.art(),
                    glint: Some(outside),
                }),
                "{}: the sweep stops short of {outside}",
                mascot.name()
            );
        }
        // It enters at the left edge and leaves at the right.
        let width = i8::try_from(mascot.art_width()).expect("a narrow block");
        assert!(centres[0] < 0, "{}: {centres:?}", mascot.name());
        assert!(
            centres[centres.len() - 1] >= width - 1,
            "{}: {centres:?}",
            mascot.name()
        );
        assert!(
            stills.windows(2).all(|pair| pair[0].hold == pair[1].hold),
            "a steady sweep"
        );
        let whole: Duration = stills.iter().map(|still| still.hold).sum();
        assert!(
            whole >= ms(350) && whole <= ms(650),
            "{}: the sweep takes {whole:?}",
            mascot.name()
        );
    }
}

#[test]
fn the_gems_signature_is_the_glint_travelling_back() {
    let mut there = poses(&Mascot::Gem.stills(Gesture::Glint));
    let back = poses(&Mascot::Gem.stills(Gesture::Signature));
    there.reverse();
    assert_eq!(back, there);
}

// ===== the schedule =====

#[test]
fn a_mascot_rests_until_its_greeting() {
    for mascot in Mascot::ALL {
        for seed in SEEDS {
            let opening = mascot_motion(mascot, seed, Duration::ZERO);
            assert_eq!(opening.pose, MascotPose::rest(mascot));
            assert_eq!(opening.holds, MOTION_GREETING_DELAY);
            let nearly = mascot_motion(mascot, seed, MOTION_GREETING_DELAY - ms(1));
            assert_eq!(nearly.pose, MascotPose::rest(mascot));
            assert_eq!(nearly.holds, ms(1), "the hold is what is left of it");
            let begun = mascot_motion(mascot, seed, MOTION_GREETING_DELAY);
            assert_ne!(begun.pose, MascotPose::rest(mascot), "{}", mascot.name());
        }
    }
}

#[test]
fn the_greeting_is_a_glint_then_the_signature_then_a_blink() {
    for mascot in Mascot::ALL {
        let rest = MascotPose::rest(mascot);
        let mut expected = vec![rest];
        for gesture in [Gesture::Glint, Gesture::Signature, Gesture::Blink] {
            expected.extend(poses(&mascot.stills(gesture)));
            expected.push(rest);
        }
        for seed in SEEDS {
            let visited: Vec<MascotPose> = walk(mascot, seed, Duration::ZERO, MOTION_BAR)
                .into_iter()
                .map(|(_, pose)| pose)
                .collect();
            assert_eq!(
                visited,
                folded(expected.clone()),
                "{} seed {seed}",
                mascot.name()
            );
        }
    }
}

#[test]
fn the_greeting_does_not_depend_on_the_seed() {
    // The first thing a mascot does is the same at every launch.
    for mascot in Mascot::ALL {
        let first = walk(mascot, SEEDS[0], Duration::ZERO, MOTION_BAR);
        for seed in SEEDS {
            assert_eq!(walk(mascot, seed, Duration::ZERO, MOTION_BAR), first);
        }
    }
}

#[test]
fn every_bar_opens_and_closes_at_rest() {
    // An act starts in the bar's middle and is over well before the bar is,
    // so two acts never run into each other.
    for mascot in Mascot::ALL {
        let rest = MascotPose::rest(mascot);
        for seed in SEEDS {
            for bar in 0..BARS {
                let opens = MOTION_BAR * bar;
                for into in (0..=600).step_by(50) {
                    assert_eq!(
                        mascot_motion(mascot, seed, opens + ms(into)).pose,
                        rest,
                        "{} seed {seed} bar {bar}: {into}ms in",
                        mascot.name()
                    );
                }
                let closes = opens + MOTION_BAR;
                for before in (1..=900).step_by(50) {
                    assert_eq!(
                        mascot_motion(mascot, seed, closes - ms(before)).pose,
                        rest,
                        "{} seed {seed} bar {bar}: {before}ms before its end",
                        mascot.name()
                    );
                }
            }
        }
    }
}

#[test]
fn acts_begin_in_the_middle_of_their_bars_and_are_spaced() {
    for mascot in [Mascot::Crest, Mascot::Gem] {
        for seed in SEEDS {
            let mut last = act_start(mascot, seed, 1);
            for bar in 1..BARS {
                let start = act_start(mascot, seed, bar);
                let into = start - MOTION_BAR * bar;
                assert!(
                    (MOTION_ACT_EARLIEST..=MOTION_ACT_LATEST).contains(&into),
                    "seed {seed} bar {bar}: the act begins {into:?} in"
                );
                if bar > 1 {
                    let gap = start - last;
                    assert!(
                        gap >= ms(3_000) && gap <= ms(6_600),
                        "seed {seed} bar {bar}: {gap:?} after the last act"
                    );
                }
                last = start;
            }
        }
    }
}

#[test]
fn acts_do_not_keep_time() {
    // A blink every N seconds on the dot reads as a machine. The offsets
    // must actually vary from bar to bar.
    for seed in SEEDS {
        let offsets: Vec<Duration> = (1..BARS)
            .map(|bar| act_start(Mascot::Crest, seed, bar) - MOTION_BAR * bar)
            .collect();
        let distinct: std::collections::BTreeSet<Duration> = offsets.iter().copied().collect();
        assert!(
            distinct.len() > offsets.len() / 2,
            "seed {seed}: {} distinct offsets in {} bars",
            distinct.len(),
            offsets.len()
        );
    }
}

#[test]
fn a_hold_runs_exactly_to_the_next_pose_that_differs() {
    // The boundary sleeps for `holds` and draws again, so every wake must
    // have something new to draw — and nothing may change before it.
    for mascot in Mascot::ALL {
        for seed in SEEDS {
            let visited = walk(mascot, seed, Duration::ZERO, MOTION_BAR * 12);
            for pair in visited.windows(2) {
                let ((at, pose), (next_at, next)) = (pair[0], pair[1]);
                assert_ne!(
                    pose,
                    next,
                    "{} seed {seed}: woke at {next_at:?} to the pose it already had",
                    mascot.name()
                );
                assert_eq!(
                    mascot_motion(mascot, seed, next_at - ms(1)).pose,
                    pose,
                    "{} seed {seed}: the pose from {at:?} changed early",
                    mascot.name()
                );
            }
        }
    }
}

#[test]
fn a_resting_mascot_sleeps_across_the_bar_boundary() {
    // Between acts nothing is drawn at all: the rest after one act holds
    // straight through to the next bar's act, never waking at the boundary.
    for seed in SEEDS {
        for bar in 1..BARS {
            let closing = MOTION_BAR * (bar + 1) - ms(200);
            let motion = mascot_motion(Mascot::Crest, seed, closing);
            assert_eq!(motion.pose, MascotPose::rest(Mascot::Crest));
            assert_eq!(
                closing + motion.holds,
                act_start(Mascot::Crest, seed, bar + 1),
                "seed {seed} bar {bar}"
            );
        }
    }
}

#[test]
fn the_schedule_is_the_seeds() {
    let starts = |seed| -> Vec<Duration> {
        (1..BARS)
            .map(|bar| act_start(Mascot::Crest, seed, bar))
            .collect()
    };
    assert_eq!(starts(7), starts(7), "one seed, one schedule");
    assert_ne!(starts(7), starts(8), "another seed, another schedule");
    // …and what plays is the seed's too, not only when.
    let acts = |seed| walk(Mascot::Crest, seed, MOTION_BAR, MOTION_BAR * 30);
    assert_eq!(acts(7), acts(7));
}

#[test]
fn every_gesture_turns_up_and_blinks_lead() {
    // The weights are alive: over a long idle every gesture is seen, and a
    // blink far more often than anything else.
    for mascot in Mascot::ALL {
        let visited: Vec<MascotPose> = walk(mascot, 3, MOTION_BAR, MOTION_BAR * 400)
            .into_iter()
            .map(|(_, pose)| pose)
            .collect();
        let count = |gesture: Gesture| {
            let first = mascot.stills(gesture)[0].pose;
            visited.iter().filter(|pose| **pose == first).count()
        };
        for gesture in Gesture::ALL {
            assert!(
                count(gesture) > 0,
                "{}: no {gesture:?} in 400 bars",
                mascot.name()
            );
        }
        for other in [Gesture::Glance, Gesture::Signature, Gesture::Glint] {
            assert!(
                count(Gesture::Blink) > count(other),
                "{}: {other:?} outnumbers the blink",
                mascot.name()
            );
        }
    }
}

#[test]
fn the_timeline_runs_for_as_long_as_the_session_idles() {
    // A banner left up for a day still answers at once and still rests.
    let day = Duration::from_secs(24 * 60 * 60);
    for mascot in Mascot::ALL {
        let motion = mascot_motion(mascot, 9, day);
        assert!(motion.holds > Duration::ZERO);
        assert!(motion.holds <= MOTION_BAR * 2);
        assert_eq!(motion.pose.art.len(), mascot.art().len());
    }
}

// ===== when the session lets it move =====

#[test]
fn a_fresh_session_has_its_banner_in_motion() {
    let app = App::new();
    assert!(app.banner_in_motion());
    assert_eq!(
        app.banner_motion().expect("in motion").pose,
        MascotPose::rest(Mascot::Crest),
        "before any frame the mascot rests"
    );
}

#[test]
fn the_banner_stops_at_the_first_cell() {
    let mut app = App::new();
    app.record_user_message("hello");
    assert!(!app.banner_in_motion());
    assert_eq!(app.banner_motion(), None);
}

#[test]
fn the_setting_stills_the_banner() {
    let mut app = App::new();
    app.settings_mut().mascot_animation = false;
    assert!(!app.banner_in_motion());
    assert_eq!(app.banner_motion(), None);
}

#[test]
fn an_agent_session_view_shows_no_motion() {
    // Its banner tops another conversation's transcript.
    let mut app = App::new();
    app.agent_view = Some("agent-1".to_string());
    assert!(!app.banner_in_motion());
}

#[test]
fn the_motion_counts_from_the_first_frame_not_from_launch() {
    // The frame clock has been running since the loop began; the mascot's
    // greeting is timed from the first frame it is actually on screen for.
    let mut app = App::new();
    app.set_pulse(ms(5_000));
    let opening = app.banner_motion().expect("in motion");
    assert_eq!(opening.pose, MascotPose::rest(Mascot::Crest));
    assert_eq!(opening.holds, MOTION_GREETING_DELAY);
    app.set_pulse(ms(5_000) + MOTION_GREETING_DELAY);
    assert_eq!(
        app.banner_motion().expect("in motion").pose,
        Mascot::Crest.stills(Gesture::Glint)[0].pose,
        "the greeting opens on its glint"
    );
}

#[test]
fn the_banner_wears_the_sessions_mascot_and_seed() {
    let mut app = App::new();
    app.set_mascot(Mascot::Skiter);
    app.seed_mascot_motion(11);
    app.set_pulse(Duration::ZERO);
    let at = MOTION_BAR * 3 + ms(1_500);
    app.set_pulse(at);
    assert_eq!(
        app.banner_motion(),
        Some(mascot_motion(Mascot::Skiter, 11, at))
    );
}

#[test]
fn clear_greets_again() {
    let mut app = App::new();
    app.set_pulse(Duration::ZERO);
    app.set_pulse(MOTION_BAR * 4);
    app.record_user_message("hello");
    app.clear_conversation();
    // The next frame is the fresh banner's first.
    app.set_pulse(MOTION_BAR * 5 + ms(123));
    let opening = app.banner_motion().expect("in motion again");
    assert_eq!(opening.pose, MascotPose::rest(Mascot::Crest));
    assert_eq!(opening.holds, MOTION_GREETING_DELAY);
}

// ===== the /mascot picker's preview =====

#[test]
fn the_preview_greets_as_a_mascot_is_highlighted() {
    let mut app = App::new();
    app.set_pulse(ms(40_000));
    assert_eq!(app.mascot_preview_motion(), None, "no picker, no preview");
    app.open_mascot_picker();
    // The first frame after the open is the preview's own start.
    app.set_pulse(ms(41_000));
    let opening = app.mascot_preview_motion().expect("previewing");
    assert_eq!(opening.pose, MascotPose::rest(Mascot::Crest));
    assert_eq!(opening.holds, MOTION_GREETING_DELAY);
    app.set_pulse(ms(41_000) + MOTION_GREETING_DELAY);
    assert_eq!(
        app.mascot_preview_motion().expect("previewing").pose,
        Mascot::Crest.stills(Gesture::Glint)[0].pose
    );
}

#[test]
fn moving_the_highlight_starts_the_next_mascots_greeting() {
    let mut app = App::new();
    app.open_mascot_picker();
    app.set_pulse(ms(1_000));
    app.set_pulse(ms(1_000) + MOTION_GREETING_DELAY + ms(100));
    assert_ne!(
        app.mascot_preview_motion().expect("previewing").pose,
        MascotPose::rest(Mascot::Crest),
        "crest is mid-greeting"
    );
    app.on_key(key(KeyCode::Down));
    app.set_pulse(ms(3_000));
    let opening = app.mascot_preview_motion().expect("previewing");
    assert_eq!(
        opening.pose,
        MascotPose::rest(Mascot::Bloom),
        "bloom starts from rest"
    );
    assert_eq!(opening.holds, MOTION_GREETING_DELAY);
    // A key that leaves the highlight where it was restarts nothing.
    app.set_pulse(ms(3_000) + MOTION_GREETING_DELAY);
    app.on_key(key(KeyCode::Left));
    app.set_pulse(ms(3_000) + MOTION_GREETING_DELAY + ms(1));
    assert_ne!(
        app.mascot_preview_motion().expect("previewing").pose,
        MascotPose::rest(Mascot::Bloom)
    );
}

#[test]
fn a_search_that_changes_the_highlight_restarts_the_preview() {
    let mut app = App::new();
    app.open_mascot_picker();
    app.set_pulse(ms(1_000));
    app.set_pulse(ms(1_000) + MOTION_GREETING_DELAY + ms(100));
    type_str(&mut app, "gem");
    app.set_pulse(ms(9_000));
    let opening = app.mascot_preview_motion().expect("previewing");
    assert_eq!(opening.pose, MascotPose::rest(Mascot::Gem));
    assert_eq!(opening.holds, MOTION_GREETING_DELAY);
    // Nothing matches: nothing to preview.
    type_str(&mut app, "zzz");
    assert_eq!(app.mascot_preview_motion(), None);
}

#[test]
fn the_setting_stills_the_preview_too() {
    let mut app = App::new();
    app.settings_mut().mascot_animation = false;
    app.open_mascot_picker();
    app.set_pulse(ms(1_000));
    assert_eq!(app.mascot_preview_motion(), None);
}

#[test]
fn the_preview_moves_even_with_a_conversation_under_it() {
    // The banner above has settled, but the picker is still a showcase.
    let mut app = App::new();
    app.record_user_message("hello");
    app.open_mascot_picker();
    app.set_pulse(ms(1_000));
    assert!(app.mascot_preview_motion().is_some());
    assert!(!app.banner_in_motion());
}
