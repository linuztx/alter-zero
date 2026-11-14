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

/// The sixteen quadrant glyphs by the quarters of a cell they fill: bit 0
/// the upper left, bit 1 the upper right, bit 2 the lower left, bit 3 the
/// lower right.
const QUADRANTS: [char; 16] = [
    ' ', '▘', '▝', '▀', '▖', '▌', '▞', '▛', '▗', '▚', '▐', '▜', '▄', '▙', '▟', '█',
];

/// `art` on the half-cell grid it is drawn on — one `bool` a quarter cell,
/// two rows of them to a row of glyphs, two to a column — or `None` when a
/// glyph of it is not a quadrant (an eyelid, a spark).
fn quarters(art: &[&str], width: usize) -> Option<Vec<Vec<bool>>> {
    let mut grid = vec![vec![false; width * 2]; art.len() * 2];
    for (r, row) in art.iter().enumerate() {
        for (c, glyph) in row.chars().enumerate() {
            let bits = QUADRANTS.iter().position(|&quad| quad == glyph)?;
            for (bit, (dy, dx)) in [(0, 0), (0, 1), (1, 0), (1, 1)].into_iter().enumerate() {
                grid[r * 2 + dy][c * 2 + dx] = bits & (1 << bit) != 0;
            }
        }
    }
    Some(grid)
}

/// The rows of glyphs a half-cell `grid` folds back into, right-trimmed
/// like the art.
fn glyphs(grid: &[Vec<bool>]) -> Vec<String> {
    grid.chunks(2)
        .map(|pair| {
            let row: String = (0..pair[0].len() / 2)
                .map(|c| {
                    let bit = |dy: usize, dx: usize| usize::from(pair[dy][c * 2 + dx]);
                    QUADRANTS[bit(0, 0) | bit(0, 1) << 1 | bit(1, 0) << 2 | bit(1, 1) << 3]
                })
                .collect();
            row.trim_end().to_string()
        })
        .collect()
}

/// `grid` moved `dx` quarter-columns and `dy` quarter-rows, or `None` when
/// that would carry ink off it.
fn moved(grid: &[Vec<bool>], dx: isize, dy: isize) -> Option<Vec<Vec<bool>>> {
    let mut out = vec![vec![false; grid[0].len()]; grid.len()];
    for (y, row) in grid.iter().enumerate() {
        for (x, _) in row.iter().enumerate().filter(|(_, ink)| **ink) {
            let to_y = usize::try_from(y as isize + dy).ok()?;
            let to_x = usize::try_from(x as isize + dx).ok()?;
            *out.get_mut(to_y)?.get_mut(to_x)? = true;
        }
    }
    Some(out)
}

/// How many rows of `art` differ from `rest`.
fn rows_redrawn(art: &[String], rest: &[&str]) -> usize {
    art.iter().zip(rest).filter(|(a, b)| a != b).count()
}

/// The half-cell steps a whole mascot is ever moved by: a dip, a lean to
/// either side, and the two at once.
const HALF_CELL_MOVES: [(isize, isize); 5] = [(0, 1), (-1, 0), (1, 0), (-1, 1), (1, 1)];

/// Whether `frame` is `mascot`'s rest art — with at most one row of it
/// redrawn — moved half a cell.
fn is_moved_half_a_cell(mascot: Mascot, frame: &[&str]) -> bool {
    let Some(grid) = quarters(frame, mascot.art_width()) else {
        return false;
    };
    HALF_CELL_MOVES.into_iter().any(|(dx, dy)| {
        moved(&grid, -dx, -dy).is_some_and(|back| rows_redrawn(&glyphs(&back), mascot.art()) <= 1)
    })
}

#[test]
fn a_frame_redraws_one_part_of_the_mascot_or_moves_all_of_it_half_a_cell() {
    // Eyes, a crest, a leaf, an ear, the legs, a spark: each lives on one
    // row, and a frame that moves it leaves the other two rows as drawn.
    // The one other thing a frame may do is move the whole mascot half a
    // cell — a dip, a lean — which redraws every row and changes nothing:
    // moved back, it is the rest art again, with one part moved at most.
    for mascot in Mascot::ALL {
        let rest = mascot.art();
        let mut whole = 0;
        for frame in mascot.motion_frames() {
            let redrawn = frame.iter().zip(rest).filter(|(a, b)| a != b).count();
            if redrawn == 1 {
                continue;
            }
            assert!(
                is_moved_half_a_cell(mascot, frame),
                "{}: {frame:?} is neither one row redrawn nor the mascot moved half a cell",
                mascot.name()
            );
            whole += 1;
        }
        assert!(whole >= 2, "{}: never moves as a whole", mascot.name());
    }
}

#[test]
fn the_half_cell_grid_folds_back_into_the_art_it_came_from() {
    // The helper the rule above stands on.
    for mascot in Mascot::ALL {
        let grid = quarters(mascot.art(), mascot.art_width()).expect("quadrant glyphs");
        assert_eq!(glyphs(&grid), mascot.art(), "{}", mascot.name());
        let dipped = moved(&grid, 0, 1);
        if let Some(dipped) = dipped {
            assert_eq!(
                glyphs(&moved(&dipped, 0, -1).expect("and back")),
                mascot.art(),
                "{}",
                mascot.name()
            );
        }
    }
    // A sprout cannot dip: its feet stand on the last quarter-row.
    let sprout = quarters(Mascot::Sprout.art(), Mascot::Sprout.art_width()).unwrap();
    assert!(moved(&sprout, 0, 1).is_none());
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
    // Were the rhythm the seed's alone, every mascot would play one
    // sequence: walking the `/mascot` list, each would go on from its
    // greeting blink for blink like the one before it, like one machine.
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
                moving * 100 < (window - MASCOT_GREETING_MS) * 25,
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
    // clip outlasts the beat it plays in.
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
                to - from <= 2_800,
                "{}: a {}ms clip",
                mascot.name(),
                to - from
            );
        }
    }
}

// ===== the repertoire =====

/// What a clip showed, with its timing left out: what tells one move from
/// another on a timeline that names none.
fn shown(clip: &(u64, u64, Vec<MascotPose>)) -> Vec<Vec<&'static str>> {
    clip.2.iter().map(|pose| pose.art.to_vec()).collect()
}

/// Whether a clip is a blink (or two): nothing but the eyes closing and
/// opening, through a lid where the glyphs have one.
fn is_blink(mascot: Mascot, clip: &(u64, u64, Vec<MascotPose>)) -> bool {
    let rest = mascot.art();
    let (open, _) = eyes(mascot, rest);
    let eye_row = usize::from(mascot != Mascot::Skiter);
    clip.2.iter().all(|pose| {
        let elsewhere = (0..rest.len()).all(|row| row == eye_row || pose.art[row] == rest[row]);
        let look = eyes(mascot, pose.art);
        elsewhere && [(open, open), ('▆', '▆'), ('█', '█')].contains(&look)
    }) && clip
        .2
        .iter()
        .any(|pose| eyes(mascot, pose.art) == ('█', '█'))
        && clip.1 - clip.0 < 600
}

/// The seeds the repertoire is checked under.
const SEEDS: [u64; 6] = [SEED, 0, 1, 0xDEAD_BEEF, 0x0123_4567_89AB_CDEF, u64::MAX];

#[test]
fn a_mascot_never_makes_the_same_move_twice_running() {
    for mascot in Mascot::ALL {
        for seed in SEEDS {
            let clips = clips(mascot, seed, 0, 900_000);
            for pair in clips.windows(2) {
                assert_ne!(
                    shown(&pair[0]),
                    shown(&pair[1]),
                    "{} (seed {seed:#x}): the same move at {} and {}",
                    mascot.name(),
                    pair[0].0,
                    pair[1].0
                );
            }
        }
    }
}

#[test]
fn blinking_is_a_small_part_of_what_a_mascot_does() {
    // It blinks the way anything with eyes does — and that is nearly all
    // the first version did: seven moves in ten. A blink is one move in
    // nine now.
    for mascot in EYED {
        for seed in SEEDS {
            let clips = clips(mascot, seed, MASCOT_GREETING_MS, 600_000);
            let blinks = clips.iter().filter(|clip| is_blink(mascot, clip)).count();
            assert!(
                blinks >= 10,
                "{}: {blinks} blinks in ten minutes",
                mascot.name()
            );
            assert!(
                blinks * 100 <= clips.len() * 15,
                "{} (seed {seed:#x}): {blinks} of {} moves are blinks",
                mascot.name(),
                clips.len()
            );
        }
    }
}

#[test]
fn two_minutes_show_nearly_everything_a_mascot_knows() {
    // The moves are dealt like a deck, not drawn like dice: whatever two
    // minutes one looks at — they need not be a round's own — the mascot
    // does fifteen different things or more in them (the gem, which has no
    // eyes to make faces with, ten), and none of them more than a handful
    // of times.
    for mascot in Mascot::ALL {
        let wanted = if mascot == Mascot::Gem { 10 } else { 15 };
        for seed in SEEDS {
            let clips = clips(mascot, seed, MASCOT_GREETING_MS, 900_000);
            for from in (MASCOT_GREETING_MS..780_000).step_by(30_000) {
                let window: Vec<_> = clips
                    .iter()
                    .filter(|clip| clip.0 >= from && clip.1 < from + 120_000)
                    .collect();
                let kinds: BTreeSet<_> = window.iter().map(|clip| shown(clip)).collect();
                assert!(
                    kinds.len() >= wanted,
                    "{} (seed {seed:#x}): {} different moves in the two minutes from {from}",
                    mascot.name(),
                    kinds.len()
                );
                for kind in &kinds {
                    let times = window.iter().filter(|clip| &shown(clip) == kind).count();
                    assert!(
                        times <= 6,
                        "{} (seed {seed:#x}): one move {times} times in two minutes",
                        mascot.name()
                    );
                }
            }
        }
    }
}

#[test]
fn every_mascot_moves_its_whole_body_as_well_as_its_parts() {
    // Not only an eye or a leaf: every one of them bounces, leans or
    // wiggles — the whole creature, half a cell — four times a round or
    // more, so whatever two hundred seconds one looks at (long enough to
    // hold a whole round) show it at least three times.
    for mascot in Mascot::ALL {
        for seed in SEEDS {
            let clips = clips(mascot, seed, MASCOT_GREETING_MS, 700_000);
            for from in (MASCOT_GREETING_MS..480_000).step_by(30_000) {
                let whole = clips
                    .iter()
                    .filter(|clip| clip.0 >= from && clip.1 < from + 200_000)
                    .filter(|clip| {
                        clip.2
                            .iter()
                            .any(|pose| is_moved_half_a_cell(mascot, pose.art))
                    })
                    .count();
                assert!(
                    whole >= 3,
                    "{} (seed {seed:#x}): {whole} whole-body moves in the 200s from {from}",
                    mascot.name()
                );
            }
        }
    }
}

#[test]
fn a_doze_comes_only_to_a_mascot_left_alone() {
    // The four that can nod off — eyes shut, head dipping — do it now and
    // then, and never in the first minute: a mascot that fell asleep as the
    // app opened would be saying something about the app.
    for mascot in [Mascot::Crest, Mascot::Bloom, Mascot::Twin, Mascot::Skiter] {
        for seed in SEEDS {
            let dozes: Vec<u64> = clips(mascot, seed, 0, 600_000)
                .iter()
                .filter(|clip| clip.1 - clip.0 >= 1_400)
                .map(|clip| clip.0)
                .collect();
            assert!(
                dozes.len() >= 3,
                "{} (seed {seed:#x}): {} dozes in ten minutes",
                mascot.name(),
                dozes.len()
            );
            assert!(
                dozes[0] >= 80_000,
                "{} (seed {seed:#x}): dozed off {}ms in",
                mascot.name(),
                dozes[0]
            );
        }
    }
    // The seedling and the gem never do.
    for mascot in [Mascot::Sprout, Mascot::Gem] {
        assert!(
            clips(mascot, SEED, 0, 600_000)
                .iter()
                .all(|clip| clip.1 - clip.0 < 1_400),
            "{}",
            mascot.name()
        );
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
        for clip in clips(mascot, SEED, 0, 600_000) {
            if !is_blink(mascot, &clip) {
                continue;
            }
            blinks += 1;
            let looks: Vec<(char, char)> = clip.2.iter().map(|p| eyes(mascot, p.art)).collect();
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
            blinks >= 10,
            "{}: {blinks} blinks in ten minutes",
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

/// Every look `mascot`'s eyes take over ten minutes, standing where it
/// stands — the frames that move the whole mascot leave them as they were.
fn looks(mascot: Mascot) -> BTreeSet<(char, char)> {
    let rest = mascot.art();
    let eye_row = usize::from(mascot != Mascot::Skiter);
    spans(mascot, SEED, 0, 600_000)
        .iter()
        .filter(|span| (0..rest.len()).all(|row| row == eye_row || span.pose.art[row] == rest[row]))
        .map(|span| eyes(mascot, span.pose.art))
        .collect()
}

/// The glyphs an eye narrowed to its left and to its right is drawn with.
fn narrowed(mascot: Mascot) -> (char, char) {
    if eyes(mascot, mascot.art()).0 == '▄' {
        ('▟', '▙')
    } else {
        ('▜', '▛')
    }
}

#[test]
fn eyes_glance_to_one_side_together() {
    // A glance narrows both eyes to the same side, so the pair reads as
    // looking somewhere. Narrowed towards each other they cross — the one
    // other way they are ever drawn — and never apart.
    for mascot in EYED {
        let (left, right) = narrowed(mascot);
        let seen = looks(mascot);
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
        assert!(
            seen.contains(&(right, left)),
            "{}: never crosses its eyes",
            mascot.name()
        );
        assert!(
            !seen.contains(&(left, right)),
            "{}: eyes apart",
            mascot.name()
        );
    }
}

#[test]
fn a_wink_shuts_one_eye_and_leaves_the_other_open() {
    for mascot in EYED {
        let (open, _) = eyes(mascot, mascot.art());
        let seen = looks(mascot);
        assert!(
            seen.contains(&('█', open)),
            "{}: never winks its left eye",
            mascot.name()
        );
        assert!(
            seen.contains(&(open, '█')),
            "{}: never winks its right eye",
            mascot.name()
        );
    }
}

#[test]
fn only_eyes_with_a_lid_widen_and_squint() {
    // The eighth-block ramp that draws a lid also draws an eye opened wide
    // (`▂`) — for the three whose eyes sit in a cell's upper half. Wide,
    // squinting, and one of each: the looks the other two cannot give.
    for mascot in EYED {
        let seen = looks(mascot);
        let ramp = |look: &(char, char)| "▂▆".contains(look.0) || "▂▆".contains(look.1);
        if eyes(mascot, mascot.art()).0 == '▄' {
            for look in [('▂', '▂'), ('▆', '▆'), ('▂', '▆')] {
                assert!(seen.contains(&look), "{}: never {look:?}", mascot.name());
            }
        } else {
            assert!(!seen.iter().any(ramp), "{}: {seen:?}", mascot.name());
        }
    }
}

#[test]
fn every_look_is_one_the_eyes_are_known_to_give() {
    for mascot in EYED {
        let (open, _) = eyes(mascot, mascot.art());
        let (left, right) = narrowed(mascot);
        let mut known = vec![
            (open, open),
            ('█', '█'),
            (left, left),
            (right, right),
            (right, left),
            ('█', open),
            (open, '█'),
        ];
        if open == '▄' {
            known.extend([('▆', '▆'), ('▂', '▂'), ('▂', '▆')]);
        }
        for look in looks(mascot) {
            assert!(known.contains(&look), "{}: {look:?}", mascot.name());
        }
    }
}

#[test]
fn the_gem_twinkles_and_turns_and_never_shuts_its_facet() {
    // A cut gem has no eyes to blink: sparks open around it instead, and
    // the notch in its middle — its one facet — slides and gleams as the
    // stone turns, and is never closed.
    let gem = Mascot::Gem;
    let rest = gem.art();
    let mut corners = BTreeSet::new();
    let mut facets = BTreeSet::new();
    for frame in gem.motion_frames() {
        let sparks: Vec<(usize, usize)> = frame
            .iter()
            .enumerate()
            .flat_map(|(r, row)| {
                row.chars()
                    .enumerate()
                    .filter(|(_, glyph)| "·✢✶".contains(*glyph))
                    .map(move |(c, _)| (r, c))
            })
            .collect();
        assert!(sparks.len() <= 1, "{frame:?}");
        corners.extend(sparks.iter().copied());
        if frame[0] == rest[0] && frame[2] == rest[2] && frame[1] != rest[1] {
            assert_ne!(frame[1], "▝▀█████▀▘", "the facet shut");
            assert!(
                frame[1].starts_with("▝▀") && frame[1].ends_with("▀▘"),
                "{frame:?}"
            );
            facets.insert(frame[1]);
        }
    }
    assert_eq!(corners.len(), 4, "a spark at each corner: {corners:?}");
    assert!(facets.len() >= 5, "the facet turns: {facets:?}");
    // …and no other mascot carries a spark.
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

/// The `/mascot` picker open over an app whose own clock reads
/// `banner_at`, the highlight on `mascot`, and that mascot's preview
/// `preview_at` into its timeline — the frame that first shows a mascot's
/// preview is the one its clock starts from.
fn previewing(mascot: Mascot, banner_at: u64, preview_at: u64) -> App {
    let mut app = animated(banner_at);
    app.open_mascot_picker();
    let steps = Mascot::ALL.iter().position(|&m| m == mascot).unwrap();
    for _ in 0..steps {
        app.on_key(key(KeyCode::Down));
    }
    assert_eq!(app.highlighted_mascot(), Some(mascot));
    app.set_mascot_clock(ms(banner_at));
    app.set_mascot_clock(ms(banner_at + preview_at));
    app
}

#[test]
fn the_open_picker_previews_the_highlighted_mascot_in_motion() {
    // The `/mascot` picker shows the banner each mascot would give — moves
    // included, so a mascot can be chosen for how it behaves. The preview
    // keeps a clock of its own: the banner may be a minute into its rhythm
    // while the mascot under the highlight is a second and a half into its
    // greeting, showing its own move.
    let banner_at = 60_000;
    let own_move = moving_at(Mascot::Bloom);
    let app = previewing(Mascot::Bloom, banner_at, own_move);
    assert_eq!(
        app.mascot_pose_for(Mascot::Bloom),
        Mascot::Bloom.pose(SEED, ms(own_move))
    );
    assert_ne!(
        app.mascot_pose_for(Mascot::Bloom),
        MascotPose::rest(Mascot::Bloom)
    );
    // The banner is where the session's clock says it is.
    assert_eq!(
        app.banner_pose(),
        Mascot::Crest.pose(SEED, ms(banner_at + own_move))
    );
    // A frame is due for the preview even with the banner itself off
    // screen, at the previewed mascot's own pace.
    assert_eq!(
        app.mascot_frame_in(false),
        Some(Mascot::Bloom.pose_holds(SEED, ms(own_move)))
    );
    // …and with the banner on screen too, whichever changes first.
    let both = Mascot::Bloom
        .pose_holds(SEED, ms(own_move))
        .min(Mascot::Crest.pose_holds(SEED, ms(banner_at + own_move)));
    assert_eq!(app.mascot_frame_in(true), Some(both));
}

#[test]
fn a_mascot_greets_as_the_highlight_lands_on_it() {
    // Walking the list shows what each mascot does: every one the highlight
    // lands on starts over — a light, its own move, a blink — instead of
    // standing in whichever beat the session's clock happens to be in.
    let mut app = previewing(Mascot::Crest, 60_000, 2_000);
    app.on_key(key(KeyCode::Down));
    assert_eq!(app.highlighted_mascot(), Some(Mascot::Bloom));
    // Until a frame has shown it, it stands at the top of its greeting…
    assert_eq!(
        app.mascot_pose_for(Mascot::Bloom),
        MascotPose::rest(Mascot::Bloom)
    );
    assert_eq!(
        app.mascot_frame_in(false),
        Some(Mascot::Bloom.pose_holds(SEED, ms(0)))
    );
    // …which the frame that shows it starts.
    app.set_mascot_clock(ms(70_000));
    assert_eq!(
        app.mascot_pose_for(Mascot::Bloom),
        Mascot::Bloom.pose(SEED, ms(0))
    );
    let sweep = 600;
    app.set_mascot_clock(ms(70_000 + sweep));
    let pose = app.mascot_pose_for(Mascot::Bloom);
    assert_eq!(pose, Mascot::Bloom.pose(SEED, ms(sweep)));
    assert!(pose.shine.is_some(), "the light is crossing it");
    // Back on a mascot it has already shown, the greeting plays again.
    app.on_key(key(KeyCode::Up));
    assert_eq!(app.highlighted_mascot(), Some(Mascot::Crest));
    app.set_mascot_clock(ms(80_000));
    app.set_mascot_clock(ms(80_000 + sweep));
    assert_eq!(
        app.mascot_pose_for(Mascot::Crest),
        Mascot::Crest.pose(SEED, ms(sweep))
    );
}

#[test]
fn a_search_that_lands_on_another_mascot_starts_its_greeting() {
    // The preview's clock follows the mascot under the highlight, however
    // the highlight got there.
    let mut app = previewing(Mascot::Crest, 60_000, 2_000);
    for c in "gem".chars() {
        app.on_key(key(KeyCode::Char(c)));
    }
    assert_eq!(app.highlighted_mascot(), Some(Mascot::Gem));
    app.set_mascot_clock(ms(70_000));
    app.set_mascot_clock(ms(70_500));
    assert_eq!(
        app.mascot_pose_for(Mascot::Gem),
        Mascot::Gem.pose(SEED, ms(500))
    );
}

#[test]
fn a_picker_opened_again_greets_again() {
    let mut app = previewing(Mascot::Crest, 60_000, 2_000);
    app.close_mascot_picker();
    app.set_mascot_clock(ms(63_000));
    assert_eq!(app.mascot_frame_in(false), None);
    app.open_mascot_picker();
    app.set_mascot_clock(ms(64_000));
    app.set_mascot_clock(ms(64_600));
    assert_eq!(
        app.mascot_pose_for(Mascot::Crest),
        Mascot::Crest.pose(SEED, ms(600))
    );
}

#[test]
fn a_preview_starts_over_with_a_clock_that_does() {
    // The session's clock starts over when a fresh banner opens. A preview
    // counted from a reading that is now in the future starts over with it
    // rather than standing still until the clock catches up.
    let mut app = previewing(Mascot::Crest, 60_000, 2_000);
    app.set_mascot_clock(ms(1_000));
    assert_eq!(
        app.mascot_pose_for(Mascot::Crest),
        Mascot::Crest.pose(SEED, ms(0))
    );
    app.set_mascot_clock(ms(1_600));
    assert_eq!(
        app.mascot_pose_for(Mascot::Crest),
        Mascot::Crest.pose(SEED, ms(600))
    );
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
