//! The mascots' idle animation: the frames, the clips, and the timeline that
//! plays them (`docs/mascot-animation.md`).

use std::collections::BTreeSet;

use super::*;

use unicode_width::UnicodeWidthStr;

/// The seed the timeline tests run on.
const SEED: u64 = 0x5EED;

/// `n` milliseconds.
fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

/// Display columns of `s` (the `ui::wrap::cols` measure, private to `ui`).
fn cols(s: &str) -> usize {
    s.width()
}

/// One stretch of the timeline: the pose it holds and the milliseconds it
/// holds it over, `from..to`.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Span {
    from: u64,
    to: u64,
    pose: MascotPose,
}

/// The timeline of `mascot` over `from..to`, walked the way the loop walks
/// it: draw a pose, wait as long as it says it holds, draw the next.
fn spans(mascot: Mascot, seed: u64, from: u64, to: u64) -> Vec<Span> {
    let mut out = Vec::new();
    let mut at = from;
    while at < to {
        let pose = mascot.pose(seed, ms(at));
        let hold = u64::try_from(mascot.pose_holds(seed, ms(at)).as_millis()).unwrap();
        assert!(hold > 0, "{}: a pose that holds for no time", mascot.name());
        out.push(Span {
            from: at,
            to: at + hold,
            pose,
        });
        at += hold;
    }
    out
}

/// The longest pause a clip takes inside itself (between the two halves of
/// a double blink, the flaps of a flutter) is shorter than this, and the
/// stillness between two clips never is — what tells one clip from the next
/// on a timeline that says nothing about clips.
const BREATH: u64 = 200;

/// The timeline's **clips**: each run of spans off the rest pose — pauses
/// shorter than a [`BREATH`] included — as its start, its end and the poses
/// it showed in order.
fn clips(mascot: Mascot, seed: u64, from: u64, to: u64) -> Vec<(u64, u64, Vec<MascotPose>)> {
    let rest = MascotPose::rest(mascot);
    let mut out: Vec<(u64, u64, Vec<MascotPose>)> = Vec::new();
    for span in spans(mascot, seed, from, to) {
        if span.pose == rest {
            continue;
        }
        match out.last_mut() {
            Some(clip) if span.from - clip.1 < BREATH => {
                if span.from > clip.1 {
                    clip.2.push(rest);
                }
                clip.1 = span.to;
                clip.2.push(span.pose);
            }
            _ => out.push((span.from, span.to, vec![span.pose])),
        }
    }
    out
}

/// The eye cells of an art's eye row: the two glyphs the mascot looks out
/// of, by the row and the two columns its rest art keeps them in.
fn eyes(mascot: Mascot, art: &[&str]) -> (char, char) {
    let row = match mascot {
        Mascot::Skiter => 0,
        _ => 1,
    };
    let cells: Vec<char> = art[row].chars().collect();
    (cells[2], cells[6])
}

/// The mascots that look out of two eyes — every one but the gem.
const EYED: [Mascot; 5] = [
    Mascot::Crest,
    Mascot::Bloom,
    Mascot::Sprout,
    Mascot::Twin,
    Mascot::Skiter,
];

// ===== the frames =====

#[test]
fn every_frame_keeps_the_rest_arts_rows_and_never_outgrows_its_width() {
    // The banner seats its metadata column at the rest art's width and its
    // rows at the rest art's height, so a frame that grew either would shove
    // the title sideways or the composer down for the length of a blink.
    for mascot in Mascot::ALL {
        let rest = mascot.art();
        let frames = mascot.motion_frames();
        assert!(!frames.is_empty(), "{}: no frames", mascot.name());
        for frame in frames {
            assert_eq!(frame.len(), rest.len(), "{}: {frame:?}", mascot.name());
            for row in frame {
                assert!(
                    cols(row) <= mascot.art_width(),
                    "{}: {row:?} is wider than the art",
                    mascot.name()
                );
            }
        }
    }
}

#[test]
fn every_frame_is_drawn_in_single_width_glyphs_the_art_already_uses() {
    // The rest art's rule (docs/table-streaming.md "Wide glyphs"), and its
    // alphabet: the Block Elements the catalog is drawn in, plus the three
    // sparks the `sparkle` spinner already puts on screen.
    for mascot in Mascot::ALL {
        for frame in mascot.motion_frames() {
            for row in frame {
                assert_eq!(cols(row), row.chars().count(), "{row:?}");
                for glyph in row.chars() {
                    assert!(
                        glyph == ' '
                            || ('\u{2580}'..='\u{259F}').contains(&glyph)
                            || "·✢✶".contains(glyph),
                        "{}: {glyph:?} in {row:?}",
                        mascot.name()
                    );
                }
            }
        }
    }
}

#[test]
fn every_frame_moves_something_and_no_row_trails_a_blank() {
    for mascot in Mascot::ALL {
        for frame in mascot.motion_frames() {
            assert_ne!(frame, mascot.art(), "{}: a frame at rest", mascot.name());
            for row in frame {
                // A trailing blank would paint a cell the rest art leaves
                // alone — one the banner's metadata column may sit in.
                assert_eq!(row.trim_end(), *row, "{}: {row:?}", mascot.name());
            }
        }
    }
}

#[test]
fn a_frame_changes_one_part_of_the_mascot_at_a_time() {
    // Eyes, a crest, a leaf, an ear, the legs, a spark: each lives on one
    // row, and a frame that moves it leaves the other two rows as drawn —
    // the bloom's petals alone span a single row too (its arms sit beside
    // its eyes).
    for mascot in Mascot::ALL {
        let rest = mascot.art();
        for frame in mascot.motion_frames() {
            let moved = frame.iter().zip(rest).filter(|(a, b)| a != b).count();
            assert_eq!(moved, 1, "{}: {frame:?}", mascot.name());
        }
    }
}

// ===== the pose and how long it holds =====

#[test]
fn the_animation_opens_at_rest() {
    for mascot in Mascot::ALL {
        let pose = mascot.pose(SEED, Duration::ZERO);
        assert_eq!(pose, MascotPose::rest(mascot));
        assert_eq!(pose.art, mascot.art());
        assert_eq!(pose.shine, None);
    }
}

#[test]
fn a_pose_holds_exactly_as_long_as_it_says() {
    // The loop draws a pose and sleeps for `pose_holds`: too long and a
    // frame is skipped, too short and it wakes to repaint nothing. Checked
    // against the timeline itself, a millisecond at a time.
    for mascot in Mascot::ALL {
        let end = 45_000u64;
        let poses: Vec<MascotPose> = (0..=end).map(|at| mascot.pose(SEED, ms(at))).collect();
        // The millisecond each stretch of one **art** ends at.
        let mut art_ends = vec![end; poses.len()];
        for at in (0..end as usize).rev() {
            art_ends[at] = if poses[at].art == poses[at + 1].art {
                art_ends[at + 1]
            } else {
                at as u64 + 1
            };
        }
        for at in (0..end - 5_000).step_by(7) {
            let pose = poses[at as usize];
            let hold = u64::try_from(mascot.pose_holds(SEED, ms(at)).as_millis()).unwrap();
            assert!(hold > 0, "{} at {at}", mascot.name());
            let art_end = art_ends[at as usize];
            if pose.shine.is_some() {
                // A sweep moves every frame: it never sleeps past its
                // cadence, nor past the art under it.
                assert!(
                    hold <= u64::try_from(MASCOT_SHINE_FRAME.as_millis()).unwrap(),
                    "{} at {at}: a sweep held {hold}ms",
                    mascot.name()
                );
                assert!(at + hold <= art_end, "{} at {at}", mascot.name());
            } else {
                assert_eq!(
                    at + hold,
                    // …or the sweep that starts over this very art.
                    (at..art_end)
                        .find(|&t| poses[t as usize] != pose)
                        .unwrap_or(art_end),
                    "{} at {at}: {pose:?}",
                    mascot.name()
                );
            }
        }
    }
}

#[test]
fn the_same_seed_plays_the_same_timeline_and_another_seed_another() {
    for mascot in Mascot::ALL {
        let a = clips(mascot, SEED, 0, 120_000);
        assert_eq!(a, clips(mascot, SEED, 0, 120_000), "{}", mascot.name());
        let b = clips(mascot, SEED + 1, 0, 120_000);
        let starts = |clips: &[(u64, u64, Vec<MascotPose>)]| -> Vec<u64> {
            clips.iter().map(|clip| clip.0).collect()
        };
        assert_ne!(
            starts(&a),
            starts(&b),
            "{}: two sessions in step",
            mascot.name()
        );
    }
}

#[test]
fn two_mascots_under_one_seed_keep_their_own_rhythms() {
    // The `/mascot` picker shows the highlighted mascot's preview under the
    // session's own banner, on one seed and one clock: were the rhythm the
    // seed's alone, the two would blink in unison, like one machine.
    let starts = |mascot: Mascot| -> Vec<u64> {
        clips(mascot, SEED, MASCOT_GREETING_MS, 120_000)
            .iter()
            .map(|clip| clip.0)
            .collect()
    };
    for (i, a) in Mascot::ALL.into_iter().enumerate() {
        for b in Mascot::ALL.into_iter().skip(i + 1) {
            let (a_starts, b_starts) = (starts(a), starts(b));
            let together = a_starts
                .iter()
                .filter(|start| b_starts.contains(start))
                .count();
            assert!(
                together * 10 < a_starts.len(),
                "{} and {} start {together} of {} clips on the same millisecond",
                a.name(),
                b.name(),
                a_starts.len()
            );
        }
    }
}

// ===== the greeting =====

#[test]
fn every_session_opens_on_the_same_greeting() {
    // The first seconds are scripted, not drawn by lot: a light sweeps the
    // mascot, it shows its own move, and it blinks.
    for mascot in Mascot::ALL {
        let greeting = clips(mascot, SEED, 0, MASCOT_GREETING_MS);
        assert_eq!(
            greeting,
            clips(mascot, 0xFACE, 0, MASCOT_GREETING_MS),
            "{}: the greeting follows the seed",
            mascot.name()
        );
        assert_eq!(greeting.len(), 3, "{}: {greeting:?}", mascot.name());
        // It waits for the banner to be seen before it moves…
        assert!(greeting[0].0 >= 300, "{}", mascot.name());
        // …opens on the sweep…
        assert!(
            greeting[0].2.iter().all(|pose| pose.shine.is_some()),
            "{}",
            mascot.name()
        );
        // …and is back at rest when the drawn-by-lot beats begin.
        assert!(greeting[2].1 < MASCOT_GREETING_MS, "{}", mascot.name());
        assert_eq!(
            mascot.pose(SEED, ms(MASCOT_GREETING_MS)),
            MascotPose::rest(mascot),
            "{}",
            mascot.name()
        );
    }
}

#[test]
fn the_greeting_shows_each_mascots_own_move_then_a_blink() {
    for mascot in EYED {
        let greeting = clips(mascot, SEED, 0, MASCOT_GREETING_MS);
        let (open, _) = eyes(mascot, mascot.art());
        // The move: eyes open throughout, something else in motion.
        assert!(
            greeting[1]
                .2
                .iter()
                .all(|pose| pose.shine.is_none() && eyes(mascot, pose.art).0 == open),
            "{}: {:?}",
            mascot.name(),
            greeting[1]
        );
        // The blink: the eyes shut at its middle.
        assert!(
            greeting[2]
                .2
                .iter()
                .any(|pose| eyes(mascot, pose.art) == ('█', '█')),
            "{}: {:?}",
            mascot.name(),
            greeting[2]
        );
    }
}

#[test]
fn a_sweep_runs_once_from_edge_to_edge() {
    for mascot in Mascot::ALL {
        let sweep = &clips(mascot, SEED, 0, MASCOT_GREETING_MS)[0];
        let progress: Vec<f32> = sweep.2.iter().filter_map(|pose| pose.shine).collect();
        assert!(
            progress.len() >= 15,
            "{}: a {}-frame sweep",
            mascot.name(),
            progress.len()
        );
        assert!(
            progress[0] < 0.05,
            "{}: starts at {}",
            mascot.name(),
            progress[0]
        );
        assert!(
            progress.windows(2).all(|pair| pair[0] < pair[1]),
            "{}: the sweep stalled or turned back",
            mascot.name()
        );
        let last = progress[progress.len() - 1];
        assert!(
            (0.9..1.0).contains(&last),
            "{}: ends at {last}",
            mascot.name()
        );
        let length = sweep.1 - sweep.0;
        assert!(
            (500..=1_000).contains(&length),
            "{}: {length}ms",
            mascot.name()
        );
    }
}

// ===== the rhythm =====

#[test]
fn a_mascot_rests_far_more_than_it_moves_and_never_for_long() {
    // Alive, not restless: a move every few seconds, the rest of the time
    // still — and never so still that it reads as frozen.
    for mascot in Mascot::ALL {
        for seed in [SEED, 1, 0xDEAD_BEEF, u64::MAX] {
            let window = 600_000u64;
            let clips = clips(mascot, seed, MASCOT_GREETING_MS, window);
            let moving: u64 = clips.iter().map(|clip| clip.1 - clip.0).sum();
            assert!(
                moving * 100 < (window - MASCOT_GREETING_MS) * 15,
                "{} moves {}% of the time",
                mascot.name(),
                moving * 100 / window
            );
            let mean = (window - MASCOT_GREETING_MS) / clips.len() as u64;
            assert!(
                (2_500..=5_000).contains(&mean),
                "{}: a move every {mean}ms",
                mascot.name()
            );
            let longest = clips
                .windows(2)
                .map(|pair| pair[1].0 - pair[0].1)
                .max()
                .unwrap();
            assert!(longest <= 9_000, "{}: still for {longest}ms", mascot.name());
        }
    }
}

#[test]
fn a_clip_never_runs_into_the_next() {
    // Each returns to rest before another starts, with a breath between —
    // so no stillness falls between a clip's own pause and a breath, and no
    // clip is as long as two.
    for mascot in Mascot::ALL {
        let rest = MascotPose::rest(mascot);
        for span in spans(mascot, SEED, 0, 300_000) {
            let length = span.to - span.from;
            assert!(
                span.pose != rest || length <= 160 || length >= BREATH,
                "{}: still for {length}ms at {}",
                mascot.name(),
                span.from
            );
        }
        for (from, to, _) in clips(mascot, SEED, 0, 300_000) {
            assert!(
                to - from <= 1_100,
                "{}: a {}ms clip",
                mascot.name(),
                to - from
            );
        }
    }
}

#[test]
fn every_frame_reaches_the_screen_sooner_or_later() {
    // A frame no clip plays is art nobody will ever see.
    for mascot in Mascot::ALL {
        let shown: BTreeSet<Vec<&str>> = spans(mascot, SEED, 0, 1_800_000)
            .iter()
            .map(|span| span.pose.art.to_vec())
            .collect();
        for frame in mascot.motion_frames() {
            assert!(shown.contains(frame), "{}: {frame:?}", mascot.name());
        }
    }
}

// ===== the moves =====

#[test]
fn eyes_close_and_open_through_a_lid_where_the_glyphs_have_one() {
    // The three mascots whose eyes sit in a cell's upper half have a
    // half-way glyph (`▆`) and blink through it; the two whose eyes sit in
    // the lower half have none in the font-safe range and blink at once.
    for mascot in EYED {
        let (open, _) = eyes(mascot, mascot.art());
        let lidded = open == '▄';
        let mut blinks = 0;
        for (_, _, poses) in clips(mascot, SEED, 0, 300_000) {
            let looks: Vec<(char, char)> = poses.iter().map(|p| eyes(mascot, p.art)).collect();
            if !looks.contains(&('█', '█')) {
                continue;
            }
            blinks += 1;
            for (i, look) in looks.iter().enumerate() {
                if *look != ('█', '█') {
                    continue;
                }
                let before = if i == 0 { (open, open) } else { looks[i - 1] };
                let after = looks.get(i + 1).copied().unwrap_or((open, open));
                let step = if lidded { ('▆', '▆') } else { (open, open) };
                assert_eq!(before, step, "{}: into the blink", mascot.name());
                assert_eq!(after, step, "{}: out of the blink", mascot.name());
            }
        }
        assert!(
            blinks >= 30,
            "{}: {blinks} blinks in five minutes",
            mascot.name()
        );
    }
}

#[test]
fn a_blink_is_quick_and_opens_slower_than_it_closes() {
    for mascot in [Mascot::Crest, Mascot::Bloom, Mascot::Twin] {
        let blink = spans(mascot, SEED, 0, MASCOT_GREETING_MS)
            .into_iter()
            .filter(|span| eyes(mascot, span.pose.art) != ('▄', '▄'))
            .collect::<Vec<_>>();
        let lengths: Vec<u64> = blink.iter().map(|span| span.to - span.from).collect();
        assert_eq!(lengths.len(), 3, "{}: {blink:?}", mascot.name());
        assert!(lengths[0] < lengths[2], "{}: {lengths:?}", mascot.name());
        let whole: u64 = lengths.iter().sum();
        assert!(
            (120..=260).contains(&whole),
            "{}: a {whole}ms blink",
            mascot.name()
        );
    }
}

#[test]
fn eyes_glance_to_one_side_together() {
    // A glance narrows both eyes to the same side, so the pair reads as
    // looking somewhere rather than crossing.
    for mascot in EYED {
        let (open, _) = eyes(mascot, mascot.art());
        let (left, right) = if open == '▄' {
            ('▟', '▙')
        } else {
            ('▜', '▛')
        };
        let seen: BTreeSet<(char, char)> = spans(mascot, SEED, 0, 600_000)
            .iter()
            .map(|span| eyes(mascot, span.pose.art))
            .collect();
        assert!(
            seen.contains(&(left, left)),
            "{}: never looks left",
            mascot.name()
        );
        assert!(
            seen.contains(&(right, right)),
            "{}: never looks right",
            mascot.name()
        );
        for look in seen {
            assert_eq!(look.0, look.1, "{}: {look:?}", mascot.name());
        }
    }
}

#[test]
fn the_gem_twinkles_and_never_shuts_its_facet() {
    // A cut gem has no eyes to blink: a spark opens beside it instead, and
    // the notch in its middle stays cut.
    let rest = Mascot::Gem.art();
    for frame in Mascot::Gem.motion_frames() {
        assert_eq!(frame[1], rest[1], "the gem's girdle row");
        let sparks: Vec<char> = frame
            .iter()
            .flat_map(|row| row.chars())
            .filter(|glyph| "·✢✶".contains(*glyph))
            .collect();
        assert_eq!(sparks.len(), 1, "{frame:?}");
    }
    // …and no other mascot carries one.
    for mascot in EYED {
        for frame in mascot.motion_frames() {
            assert!(
                !frame.iter().any(|row| row.contains(['·', '✢', '✶'])),
                "{}: {frame:?}",
                mascot.name()
            );
        }
    }
}

#[test]
fn a_spark_opens_and_closes_through_the_same_glyphs() {
    let twinkle = clips(Mascot::Gem, SEED, 0, MASCOT_GREETING_MS)
        .into_iter()
        .nth(1)
        .unwrap();
    let sparks: String = twinkle
        .2
        .iter()
        .flat_map(|pose| pose.art.iter().flat_map(|row| row.chars()))
        .filter(|glyph| "·✢✶".contains(*glyph))
        .collect();
    assert_eq!(sparks, "·✢✶✢·");
}

// ===== the session's side =====

/// An app seeded to animate, `at` into its mascot's timeline.
fn animated(at: u64) -> App {
    let mut app = App::new();
    app.seed_mascot_motion(SEED);
    app.set_mascot_clock(ms(at));
    app
}

/// A millisecond at which `mascot`'s timeline is off its rest pose.
fn moving_at(mascot: Mascot) -> u64 {
    clips(mascot, SEED, 0, MASCOT_GREETING_MS)[1].0 + 1
}

#[test]
fn an_unseeded_app_keeps_its_mascot_still() {
    // The unit-test default, and a headless run's: no seed, no motion.
    let mut app = App::new();
    app.set_mascot_clock(ms(moving_at(Mascot::Crest)));
    assert!(!app.mascot_animates());
    assert_eq!(app.banner_pose(), MascotPose::rest(Mascot::Crest));
    assert_eq!(app.mascot_frame_in(true), None);
}

#[test]
fn a_seeded_app_poses_the_banner_of_an_empty_conversation() {
    let at = moving_at(Mascot::Crest);
    let app = animated(at);
    assert!(app.mascot_animates());
    assert!(app.banner_is_live());
    assert_eq!(app.banner_pose(), Mascot::Crest.pose(SEED, ms(at)));
    assert_ne!(app.banner_pose(), MascotPose::rest(Mascot::Crest));
    assert_eq!(
        app.mascot_frame_in(true),
        Some(Mascot::Crest.pose_holds(SEED, ms(at)))
    );
}

#[test]
fn the_banner_follows_the_sessions_mascot() {
    let at = moving_at(Mascot::Sprout);
    let mut app = animated(at);
    app.set_mascot(Mascot::Sprout);
    assert_eq!(app.banner_pose(), Mascot::Sprout.pose(SEED, ms(at)));
}

#[test]
fn the_first_cell_stills_the_banner_for_good() {
    // The banner animates while there is no cell under it. The first one —
    // a message, a `!` command, anything recorded — ends that: from then on
    // the banner is scrollback, and scrollback does not move.
    let at = moving_at(Mascot::Crest);
    let mut app = animated(at);
    app.record_user_message("hi");
    assert!(!app.banner_is_live());
    assert_eq!(app.banner_pose(), MascotPose::rest(Mascot::Crest));
    assert_eq!(app.mascot_frame_in(true), None);
    // …and a `/clear` brings the empty conversation, and the motion, back.
    app.clear_conversation();
    assert!(app.banner_is_live());
    assert_ne!(app.banner_pose(), MascotPose::rest(Mascot::Crest));
}

#[test]
fn a_turn_in_flight_stills_the_banner_before_it_records_anything() {
    let mut app = animated(moving_at(Mascot::Crest));
    app.begin_stream();
    assert!(!app.banner_is_live());
    assert_eq!(app.banner_pose(), MascotPose::rest(Mascot::Crest));
}

#[test]
fn the_setting_turns_the_motion_off_and_back_on() {
    let mut app = animated(moving_at(Mascot::Crest));
    app.settings_mut().animate_mascot = false;
    assert!(!app.mascot_animates());
    assert_eq!(app.banner_pose(), MascotPose::rest(Mascot::Crest));
    assert_eq!(app.mascot_frame_in(true), None);
    // The banner is still the banner of an empty conversation: it can be
    // repainted the moment the row is turned back on.
    assert!(app.banner_is_live());
    app.settings_mut().animate_mascot = true;
    assert_ne!(app.banner_pose(), MascotPose::rest(Mascot::Crest));
}

#[test]
fn no_frame_is_due_for_a_banner_that_is_not_on_screen() {
    // Scrolled off the top, or behind an alternate-screen overlay: a frame
    // there would wake the loop to repaint nothing.
    let mut app = animated(moving_at(Mascot::Crest));
    assert_eq!(app.mascot_frame_in(false), None);
    app.view = View::ToolOutput;
    assert_eq!(app.mascot_frame_in(true), None);
}

#[test]
fn the_open_picker_previews_the_highlighted_mascot_in_motion() {
    // The `/mascot` picker shows the banner each mascot would give — moves
    // included, so a mascot can be chosen for how it behaves.
    let at = moving_at(Mascot::Bloom);
    let mut app = animated(at);
    app.open_mascot_picker();
    app.on_key(key(KeyCode::Down));
    assert_eq!(app.highlighted_mascot(), Some(Mascot::Bloom));
    assert_eq!(
        app.mascot_pose_for(Mascot::Bloom),
        Mascot::Bloom.pose(SEED, ms(at))
    );
    // A frame is due for the preview even with the banner itself off
    // screen, at the highlighted mascot's own pace.
    assert_eq!(
        app.mascot_frame_in(false),
        Some(Mascot::Bloom.pose_holds(SEED, ms(at)))
    );
    // …and with the banner on screen too, whichever changes first.
    let both = Mascot::Bloom
        .pose_holds(SEED, ms(at))
        .min(Mascot::Crest.pose_holds(SEED, ms(at)));
    assert_eq!(app.mascot_frame_in(true), Some(both));
}

#[test]
fn a_preview_holds_still_when_the_motion_is_off() {
    let at = moving_at(Mascot::Bloom);
    let mut app = App::new();
    app.set_mascot_clock(ms(at));
    app.open_mascot_picker();
    assert_eq!(
        app.mascot_pose_for(Mascot::Bloom),
        MascotPose::rest(Mascot::Bloom)
    );
    assert_eq!(app.mascot_frame_in(true), None);
}
