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
    clips_of(mascot, &spans(mascot, seed, from, to))
}

/// The clips in a run of `spans` of `mascot`'s — see [`clips`].
fn clips_of(mascot: Mascot, spans: &[Span]) -> Vec<(u64, u64, Vec<MascotPose>)> {
    let rest = MascotPose::rest(mascot);
    let mut out: Vec<(u64, u64, Vec<MascotPose>)> = Vec::new();
    for span in spans {
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
    // A row sunk below the floor is blank: no eyes there to read.
    let cells: Vec<char> = art[row].chars().collect();
    let cell = |col: usize| cells.get(col).copied().unwrap_or(' ');
    (cell(2), cell(6))
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
    // sparks the `sparkle` spinner already puts on screen, the marks that
    // ride beside a sleeper — the `z` of a snore, the `!` of a start — and
    // the two lines a sleeper's closed eyes are drawn with.
    for mascot in Mascot::ALL {
        for frame in mascot.motion_frames() {
            for row in frame {
                assert_eq!(cols(row), row.chars().count(), "{row:?}");
                for glyph in row.chars() {
                    assert!(
                        glyph == ' '
                            || ('\u{2580}'..='\u{259F}').contains(&glyph)
                            || "·✢✶".contains(glyph)
                            || MARKS.contains(&glyph)
                            || MASCOT_LIDS.contains(&glyph),
                        "{}: {glyph:?} in {row:?}",
                        mascot.name()
                    );
                }
            }
        }
    }
}

#[test]
fn a_mark_rides_in_the_blocks_top_corner_and_nowhere_else() {
    // A snore and a start are drawn beside the mascot's head, in the top
    // right corner of its block: never in its body, never under it.
    let mut seen = BTreeSet::new();
    for mascot in Mascot::ALL {
        for frame in mascot.motion_frames() {
            for (row, text) in frame.iter().enumerate() {
                for (col, glyph) in text.chars().enumerate() {
                    if !MARKS.contains(&glyph) {
                        continue;
                    }
                    assert_eq!(
                        (row, col),
                        (0, mascot.art_width() - 1),
                        "{}: {glyph:?} in {frame:?}",
                        mascot.name()
                    );
                    seen.insert((mascot.name(), glyph));
                }
            }
        }
    }
    // Every one of the six snores, and starts awake.
    assert_eq!(seen.len(), Mascot::ALL.len() * MARKS.len(), "{seen:?}");
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

/// The **marks** that ride beside a mascot, in its block's top row: the
/// `z` and `Z` of a snore, the `!` of one woken with a start. Light, not
/// mascot — no part of the body the frame rules hold.
const MARKS: [char; 3] = ['z', 'Z', '!'];

/// `frame` with its marks taken off: the body alone — a sleeper's lids
/// filled in, so its closed eyes are the shut eyes the frame rules know.
fn unmarked(frame: &[&str]) -> Vec<String> {
    frame
        .iter()
        .map(|row| {
            row.replace(MARKS, " ")
                .replace(MASCOT_LIDS, "█")
                .trim_end()
                .to_string()
        })
        .collect()
}

/// How many lids `frame` draws — two on a sleeper, none on anything else.
fn lids(frame: &[&str]) -> usize {
    frame
        .iter()
        .flat_map(|row| row.chars())
        .filter(|glyph| MASCOT_LIDS.contains(glyph))
        .count()
}

/// `frame` with its marks taken off, leaked into the `&str` rows the eye
/// and frame helpers read — a test's few dozen bytes.
fn unmarked_rows(frame: &[&str]) -> Vec<&'static str> {
    unmarked(frame)
        .into_iter()
        .map(|row| &*Box::leak(row.into_boxed_str()))
        .collect()
}

/// Whether `frame` is `mascot`'s rest art — with at most one row of it
/// redrawn — moved half a cell.
fn is_moved_half_a_cell(mascot: Mascot, frame: &[&str]) -> bool {
    let body = unmarked(frame);
    let body: Vec<&str> = body.iter().map(String::as_str).collect();
    let Some(grid) = quarters(&body, mascot.art_width()) else {
        return false;
    };
    HALF_CELL_MOVES.into_iter().any(|(dx, dy)| {
        moved(&grid, -dx, -dy).is_some_and(|back| rows_redrawn(&glyphs(&back), mascot.art()) <= 1)
    })
}

/// `row` padded with blanks to `width` columns, as a row of cells.
fn padded(row: &str, width: usize) -> Vec<char> {
    let mut cells: Vec<char> = row.chars().collect();
    cells.resize(width.max(cells.len()), ' ');
    cells
}

/// Whether `frame` raises a limb from `mascot`'s side: two neighbouring
/// rows redrawn, and nothing in them but the art's two edge columns — an
/// arm, a petal, a claw lifted from the body's side to its shoulder.
fn raises_a_limb(mascot: Mascot, frame: &[&str]) -> bool {
    let (rest, width) = (mascot.art(), mascot.art_width());
    let changed: Vec<usize> = (0..rest.len())
        .filter(|&row| frame[row] != rest[row])
        .collect();
    changed.len() == 2
        && changed[1] == changed[0] + 1
        && changed.iter().all(|&row| {
            let (now, was) = (padded(frame[row], width), padded(rest[row], width));
            (1..width - 1).all(|col| now[col] == was[col])
        })
}

/// The sides `frame` **puffs out** on — `(left, right)` — when it is
/// `mascot` swollen toward the edges of its block: every quarter cell of
/// the rest art still inked, and all that is added lying outside the body,
/// past one end of a half-row of it or the other. Nothing is taken away
/// and nothing inside it is filled in — an eye closed is a blink, not a
/// breath taken — so it is the same creature, bigger. `None` for anything
/// else.
fn puffed_out(mascot: Mascot, frame: &[&str]) -> Option<(bool, bool)> {
    let width = mascot.art_width();
    let (now, was) = (quarters(frame, width)?, quarters(mascot.art(), width)?);
    let (mut left, mut right) = (false, false);
    for (now, was) in now.iter().zip(&was) {
        let first = was.iter().position(|ink| *ink);
        let last = was.iter().rposition(|ink| *ink);
        for (x, (&now, &was)) in now.iter().zip(was).enumerate() {
            match (was, now) {
                (true, false) => return None,
                (false, true) => match (first, last) {
                    (Some(first), _) if x < first => left = true,
                    (_, Some(last)) if x > last => right = true,
                    _ => return None,
                },
                _ => {}
            }
        }
    }
    (left || right).then_some((left, right))
}

/// Whether `frame` is `mascot` **turned** about its own middle, the way a
/// stone turns to show its edge: every half-row of ink as wide as the rest
/// art's or narrower, about the same middle, and at least one narrower.
fn turns(mascot: Mascot, frame: &[&str]) -> bool {
    let width = mascot.art_width();
    let (Some(now), Some(was)) = (quarters(frame, width), quarters(mascot.art(), width)) else {
        return false;
    };
    let span = |half_row: &Vec<bool>| {
        let inked: Vec<usize> = (0..half_row.len()).filter(|&x| half_row[x]).collect();
        inked.first().copied().zip(inked.last().copied())
    };
    let mut narrower = false;
    for (now, was) in now.iter().zip(&was) {
        match (span(now), span(was)) {
            (None, was) => narrower |= was.is_some(),
            (Some((a, b)), Some((c, d))) if a >= c && b <= d && a + b == c + d => {
                narrower |= a > c;
            }
            _ => return false,
        }
    }
    narrower
}

/// How many whole rows `frame` has **sunk** below the floor of its block:
/// the rest art moved down that many rows — the rows above it blank, what
/// is left of it above the floor as drawn, at most one row of that redrawn
/// (eyes turned to one side) — or `None` when it is not the mascot sunk.
fn sunk(mascot: Mascot, frame: &[&str]) -> Option<usize> {
    let rest = mascot.art();
    (1..rest.len()).find(|&rows| {
        frame[..rows].iter().all(|row| row.is_empty())
            && frame[rows..]
                .iter()
                .zip(rest)
                .filter(|(now, was)| now != was)
                .count()
                <= 1
    })
}

/// `body` with its eyes **opened** again where they are shut: asleep, a
/// mascot's eyes are shut whatever else it does, so a sleeper's frame is
/// held to the frame rule as the frame it would be awake.
fn eyes_opened(mascot: Mascot, body: &[&str]) -> Vec<String> {
    let mut rows: Vec<String> = body.iter().map(|row| (*row).to_string()).collect();
    if mascot == Mascot::Gem || eyes(mascot, body) != ('█', '█') {
        return rows;
    }
    let eye_row = usize::from(mascot != Mascot::Skiter);
    let open: Vec<char> = mascot.art()[eye_row].chars().collect();
    let mut cells: Vec<char> = rows[eye_row].chars().collect();
    for col in [2, 6] {
        cells[col] = open[col];
    }
    rows[eye_row] = cells.into_iter().collect();
    rows
}

/// `art` **on its feet**: moved back up half a cell when it is the mascot
/// dipped, as drawn otherwise — and its marks off either way.
fn risen(mascot: Mascot, art: &[&str]) -> Vec<String> {
    let body = unmarked(art);
    let rows: Vec<&str> = body.iter().map(String::as_str).collect();
    quarters(&rows, mascot.art_width())
        .and_then(|grid| moved(&grid, 0, -1))
        .map(|back| glyphs(&back))
        .filter(|back| rows_redrawn(back, mascot.art()) <= 1)
        .unwrap_or(body)
}

/// Whether `art` shows `mascot` **asleep**: its eyes shut, standing or
/// dipped — the gem, which has none, settled half a cell low.
fn asleep(mascot: Mascot, art: &[&str]) -> bool {
    let standing = risen(mascot, art);
    if mascot == Mascot::Gem {
        return standing
            .iter()
            .map(String::as_str)
            .eq(mascot.art().iter().copied())
            && unmarked(art)
                .iter()
                .map(String::as_str)
                .ne(mascot.art().iter().copied());
    }
    let rows: Vec<&str> = standing.iter().map(String::as_str).collect();
    eyes(mascot, &rows) == ('█', '█')
}

/// Whether the **whole** of `mascot` is on screen in `art`: every row of
/// its block drawn, none of it below the floor.
fn whole(mascot: Mascot, art: &[&str]) -> bool {
    sunk(mascot, art).is_none() && unmarked(art).iter().all(|row| !row.trim().is_empty())
}

/// Whether `art` shows `mascot` **on its feet**: neither dipped nor — the
/// seedling's way of going down — its shoot drooped to a bud.
fn on_its_feet(mascot: Mascot, art: &[&str]) -> bool {
    !is_moved_half_a_cell(mascot, art)
        && (mascot != Mascot::Sprout || unmarked(art)[0].trim() != "▄")
}

/// `mascot`'s rest art **upside down**: its half-cell grid turned top to
/// bottom, legs in the air.
fn upside_down(mascot: Mascot) -> Vec<String> {
    let mut grid = quarters(mascot.art(), mascot.art_width()).expect("quadrant glyphs");
    grid.reverse();
    glyphs(&grid)
}

/// Whether `frame` is `mascot` upside down.
fn is_upside_down(mascot: Mascot, frame: &[&str]) -> bool {
    frame
        .iter()
        .copied()
        .eq(upside_down(mascot).iter().map(String::as_str))
}

#[test]
fn a_frame_redraws_one_part_of_the_mascot_or_moves_all_of_it_half_a_cell() {
    // Eyes, a crest, a leaf, an ear, the legs, a spark: each lives on one
    // row, and a frame that moves it leaves the other two rows as drawn —
    // but a limb raised from the body's side reaches into the row above,
    // and redraws the two at the art's edges and nowhere else; and a body
    // puffed out swells past its own sides in every row it has, nothing of
    // it taken away and nothing inside it filled in. The other things a
    // frame may do are all the whole mascot, unchanged, somewhere else:
    // moved half a cell — a dip, a lean — which redraws every row and
    // changes nothing (moved back, it is the rest art again, with one part
    // moved at most); sunk whole rows below the floor of its block; turned
    // upside down; or turned about its middle, narrower, the way the gem
    // shows its edge. A sleeper's eyes are shut over any of them.
    for mascot in Mascot::ALL {
        let rest = mascot.art();
        let mut whole = 0;
        for frame in mascot.motion_frames() {
            // A mark beside the mascot is no part of it — and asleep its
            // eyes are shut whatever else it does.
            let body = unmarked(frame);
            let body: Vec<&str> = body.iter().map(String::as_str).collect();
            let awake = eyes_opened(mascot, &body);
            let redrawn = awake.iter().zip(rest).filter(|(a, b)| a != b).count();
            if redrawn <= 1
                || raises_a_limb(mascot, &body)
                || puffed_out(mascot, &body).is_some()
                || turns(mascot, &body)
                || sunk(mascot, &body).is_some()
                || is_upside_down(mascot, &body)
            {
                continue;
            }
            assert!(
                is_moved_half_a_cell(mascot, frame),
                "{}: {frame:?} is neither a part moved, the mascot moved half a cell nor it turned",
                mascot.name()
            );
            whole += 1;
        }
        assert!(whole >= 2, "{}: never moves as a whole", mascot.name());
    }
}

#[test]
fn the_mascots_with_limbs_at_their_sides_raise_them() {
    // The crest's arms and the bloom's petals: each is a quarter cell at
    // the body's side, and each comes up to the shoulder.
    for mascot in [Mascot::Crest, Mascot::Bloom] {
        assert!(
            mascot
                .motion_frames()
                .iter()
                .any(|frame| raises_a_limb(mascot, frame)),
            "{} never raises a limb",
            mascot.name()
        );
    }
}

#[test]
fn the_skitterer_puffs_itself_up_and_raises_nothing() {
    // It has no arm to throw up, and twice it was given one. The bumps at
    // its sides were claws it raised: a thin stick at each shoulder, which
    // read as a small hand stuck to its head. Then it stood tall on the
    // tips of its legs instead, which was liked no better. What it does is
    // puff itself up: its body swells to the edges of its block — both
    // sides, or one — and its eyes and its legs stay as drawn.
    let skiter = Mascot::Skiter;
    let rest = skiter.art();
    let frames = skiter.motion_frames();
    for frame in &frames {
        assert!(
            !raises_a_limb(skiter, frame),
            "something raised at its side in {frame:?}"
        );
        // A leg hangs half a cell under the body, and never reaches the
        // floor of its block while the body stands where it is drawn.
        if frame[..2] == rest[..2] {
            assert!(
                !frame[2].contains(['▌', '▐']),
                "a leg stretched to the floor in {frame:?}"
            );
        }
    }
    for (which, sides) in [
        ("both sides", (true, true)),
        ("its left side", (true, false)),
        ("its right side", (false, true)),
    ] {
        let puffs: Vec<_> = frames
            .iter()
            .filter(|frame| puffed_out(skiter, frame) == Some(sides))
            .collect();
        assert!(!puffs.is_empty(), "it never puffs out {which}");
        for frame in puffs {
            assert_eq!(frame[2], rest[2], "its legs moved in {frame:?}");
            assert_eq!(
                eyes(skiter, frame),
                eyes(skiter, rest),
                "its eyes moved in {frame:?}"
            );
        }
    }
}

#[test]
fn the_skitterer_puffs_up_where_the_others_throw_their_arms_up() {
    // A move of its own, its cheer, and the end of its start — the three
    // places another mascot's arms go up.
    let skiter = Mascot::Skiter;
    let rest = MascotPose::rest(skiter);
    let puffed = |art: &[&str]| puffed_out(skiter, art) == Some((true, true));
    let moves = clips(skiter, SEED, MASCOT_GREETING_MS, 900_000);
    // Its own move: puffed up, let out, puffed up again.
    assert!(
        moves.iter().any(|clip| {
            clip.2.len() == 3 && puffed(clip.2[0].art) && clip.2[1] == rest && puffed(clip.2[2].art)
        }),
        "it never puffs up twice"
    );
    // Its cheer: from a crouch it springs up puffed, twice, and blinks.
    assert!(
        moves.iter().any(|clip| {
            closes_on_a_blink(skiter, clip)
                && is_moved_half_a_cell(skiter, clip.2[0].art)
                && clip.2.iter().filter(|pose| puffed(pose.art)).count() == 2
        }),
        "it never cheers puffed up"
    );
    // Swollen on both sides it is held long enough to be seen, not to
    // flash: the whole silhouette changes, where a blink changes two cells.
    let held: Vec<u64> = spans(skiter, SEED, MASCOT_GREETING_MS, 900_000)
        .iter()
        .filter(|span| puffed(span.pose.art))
        .map(|span| span.to - span.from)
        .collect();
    assert!(held.iter().all(|&ms| ms >= 150), "{held:?}");
    // Woken with a start it opens its eyes where it lies — and is up,
    // puffed out to the edges of its block.
    let (mut app, at) = woken(skiter, |app| type_str(app, "h"));
    let start = banner_spans(&mut app, at, at + 600);
    let up = start
        .iter()
        .find(|span| on_its_feet(skiter, span.pose.art))
        .expect("it got up");
    assert!(puffed(up.pose.art), "{:?}", up.pose.art);
    assert!(up.to - up.from >= 200, "up for {}ms", up.to - up.from);
}

#[test]
fn the_gem_spins_edge_on_and_back() {
    // Narrower, thin, edge-on — a single column — and back the way it went:
    // the stone turning, not a frame of something else.
    let gem = Mascot::Gem;
    let edge_on = |art: &[&str]| art.iter().all(|row| row.trim().chars().count() <= 1);
    let spin = clips(gem, SEED, MASCOT_GREETING_MS, 900_000)
        .into_iter()
        .find(|clip| clip.2.iter().any(|pose| edge_on(pose.art)))
        .expect("the gem spins");
    let arts: Vec<&[&str]> = spin.2.iter().map(|pose| pose.art).collect();
    let mut back = arts.clone();
    back.reverse();
    assert_eq!(arts, back, "the way back retraces the way there");
    assert!(arts.iter().all(|art| turns(gem, art)), "{arts:?}");
    assert!(arts.len() >= 5, "{arts:?}");
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
        assert!(
            sweep.2.iter().all(|pose| pose
                .shine
                .is_some_and(|shine| shine.band == ShineBand::Sweep)),
            "{}: the greeting's light is the broad sweep",
            mascot.name()
        );
        let progress: Vec<f32> = sweep
            .2
            .iter()
            .filter_map(|pose| pose.shine)
            .map(|shine| shine.progress)
            .collect();
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

/// The clips over ten minutes of `mascot`'s timeline that are light and
/// nothing else: the art at rest throughout, something lit on it.
fn light_clips(mascot: Mascot) -> Vec<(u64, u64, Vec<MascotPose>)> {
    clips(mascot, SEED, MASCOT_GREETING_MS, 600_000)
        .into_iter()
        .filter(|clip| {
            clip.2.iter().all(|pose| pose.art == mascot.art())
                && clip
                    .2
                    .iter()
                    .any(|pose| pose.shine.is_some() || pose.glow > 0)
        })
        .collect()
}

#[test]
fn every_mascot_glows_and_fades() {
    // A breath of light: the whole mascot brightens by degrees and dims the
    // same way, its art unmoved — the light swelling through it rather than
    // crossing it.
    for mascot in Mascot::ALL {
        let glows: Vec<Vec<u8>> = light_clips(mascot)
            .iter()
            .filter(|clip| {
                clip.2
                    .iter()
                    .all(|pose| pose.shine.is_none() && pose.glow > 0)
            })
            .map(|clip| clip.2.iter().map(|pose| pose.glow).collect())
            .collect();
        assert!(
            glows.len() >= 3,
            "{}: {} glows in ten minutes",
            mascot.name(),
            glows.len()
        );
        for levels in glows {
            let peak = levels.iter().copied().max().unwrap();
            let top = levels.iter().position(|&level| level == peak).unwrap();
            assert!(levels.len() >= 5, "{}: {levels:?}", mascot.name());
            assert!(
                levels[..=top].windows(2).all(|pair| pair[0] < pair[1]),
                "{}: {levels:?} swells by degrees",
                mascot.name()
            );
            assert!(
                levels[top..].windows(2).all(|pair| pair[0] > pair[1]),
                "{}: {levels:?} fades by degrees",
                mascot.name()
            );
            assert!(
                (40..=70).contains(&peak),
                "{}: a glow, not a flash: {levels:?}",
                mascot.name()
            );
            assert!(
                levels[0] <= 20 && levels[levels.len() - 1] <= 20,
                "{}: {levels:?} starts and ends faint",
                mascot.name()
            );
        }
    }
}

#[test]
fn a_glint_crosses_from_either_side_quicker_than_the_sweep() {
    // A glint is a narrow catch of the light, gone in half a second, and it
    // comes from either side — not only the way the greeting's sweep runs.
    for mascot in Mascot::ALL {
        let mut sides = BTreeSet::new();
        for clip in light_clips(mascot) {
            let shines: Vec<MascotShine> = clip.2.iter().filter_map(|pose| pose.shine).collect();
            let Some(first) = shines.first() else {
                continue;
            };
            if matches!(first.band, ShineBand::Sweep | ShineBand::Glitter) {
                continue;
            }
            assert!(
                shines.iter().all(|shine| shine.band == first.band),
                "{}: a glint changed sides",
                mascot.name()
            );
            let progress: Vec<f32> = shines.iter().map(|shine| shine.progress).collect();
            assert!(
                progress.windows(2).all(|pair| pair[0] < pair[1]),
                "{}: {progress:?}",
                mascot.name()
            );
            assert!(
                progress[0] < 0.1 && progress[progress.len() - 1] > 0.9,
                "{}: {progress:?} edge to edge",
                mascot.name()
            );
            let length = clip.1 - clip.0;
            assert!(
                (300..=650).contains(&length),
                "{}: a {length}ms glint",
                mascot.name()
            );
            sides.insert(format!("{:?}", first.band));
        }
        assert_eq!(
            sides,
            BTreeSet::from(["GlintFromLeft".to_string(), "GlintFromRight".to_string()]),
            "{}",
            mascot.name()
        );
    }
}

#[test]
fn the_gem_catches_the_light_in_two_flashes() {
    // The cut gem's own way with light: a flash to full strength, dark, and
    // a flash again — over the stone at rest.
    let flashes: Vec<Vec<u8>> = light_clips(Mascot::Gem)
        .iter()
        .map(|clip| clip.2.iter().map(|pose| pose.glow).collect::<Vec<u8>>())
        .filter(|levels| levels.contains(&100))
        .collect();
    assert!(
        flashes.len() >= 3,
        "{} flashes in ten minutes",
        flashes.len()
    );
    for levels in flashes {
        let full = levels.iter().filter(|&&level| level == 100).count();
        assert_eq!(full, 2, "{levels:?}");
        let first = levels.iter().position(|&level| level == 100).unwrap();
        let second = levels.iter().rposition(|&level| level == 100).unwrap();
        assert!(
            levels[first..second].contains(&0),
            "{levels:?} goes dark between its flashes"
        );
    }
    // A flash is the gem's alone: the others glow, never at full strength.
    for mascot in EYED {
        assert!(
            light_clips(mascot)
                .iter()
                .all(|clip| clip.2.iter().all(|pose| pose.glow < 100)),
            "{}",
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
/// another on a timeline that names none — the art, and the light on it (a
/// glow from a glint, a glint from the left from one from the right).
fn shown(clip: &(u64, u64, Vec<MascotPose>)) -> Vec<(Vec<&'static str>, Option<ShineBand>, u8)> {
    clip.2
        .iter()
        .map(|pose| {
            (
                pose.art.to_vec(),
                pose.shine.map(|shine| shine.band),
                pose.glow,
            )
        })
        .collect()
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
fn every_frame_reaches_the_screen_sooner_or_later() {
    // A frame no clip plays is art nobody will ever see: every one is in
    // the mascot's repertoire, or in the nap of one left alone.
    for mascot in Mascot::ALL {
        let mut shown: BTreeSet<Vec<&str>> = spans(mascot, SEED, 0, 1_800_000)
            .iter()
            .map(|span| span.pose.art.to_vec())
            .collect();
        let mut app = left_alone(mascot, SEED);
        shown.extend(
            banner_spans(&mut app, 0, 320_000)
                .iter()
                .map(|span| span.pose.art.to_vec()),
        );
        // …or in the start of one woken from it.
        let (mut app, at) = woken(mascot, |app| type_str(app, "h"));
        shown.extend(
            banner_spans(&mut app, at, at + 2_000)
                .iter()
                .map(|span| span.pose.art.to_vec()),
        );
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
fn a_lidded_eye_winks_through_its_lid() {
    // The three with a lid close one eye the way they blink: through the
    // lid, held shut, and back.
    for mascot in [Mascot::Crest, Mascot::Bloom, Mascot::Twin] {
        let seen = looks(mascot);
        for look in [('▆', '▄'), ('▄', '▆')] {
            assert!(seen.contains(&look), "{}: never {look:?}", mascot.name());
        }
    }
}

/// Whether `clip` **closes on a blink**: having done something else, it
/// stands still a moment and blinks — the eyes' punctuation at the end of a
/// cheer, a pulled face.
fn closes_on_a_blink(mascot: Mascot, clip: &(u64, u64, Vec<MascotPose>)) -> bool {
    let rest = MascotPose::rest(mascot);
    let (open, _) = eyes(mascot, mascot.art());
    let blink: &[(char, char)] = if open == '▄' {
        &[('▆', '▆'), ('█', '█'), ('▆', '▆')]
    } else {
        &[('█', '█')]
    };
    let poses = &clip.2;
    let Some(before) = poses.len().checked_sub(blink.len() + 1) else {
        return false;
    };
    !is_blink(mascot, clip)
        && poses[before] == rest
        && poses[before + 1..]
            .iter()
            .zip(blink)
            .all(|(pose, look)| eyes(mascot, pose.art) == *look)
        && poses[..before].iter().any(|pose| *pose != rest)
}

#[test]
fn some_moves_close_on_a_blink() {
    // A blink is punctuation as well as a move: a cheer and a pulled face
    // each end on one, the way something alive collects itself.
    for mascot in EYED {
        let closing: BTreeSet<_> = clips(mascot, SEED, MASCOT_GREETING_MS, 900_000)
            .iter()
            .filter(|clip| closes_on_a_blink(mascot, clip))
            .map(shown)
            .collect();
        assert!(
            closing.len() >= 2,
            "{}: {} moves close on a blink",
            mascot.name(),
            closing.len()
        );
    }
}

/// The side of the art `art` changes, when it changes nothing but one
/// side's last two columns: an arm raised on that side.
fn arm_side(mascot: Mascot, art: &[&str]) -> Option<char> {
    let (rest, width) = (mascot.art(), mascot.art_width());
    let changed: BTreeSet<usize> = (0..rest.len())
        .flat_map(|row| {
            let (now, was) = (padded(art[row], width), padded(rest[row], width));
            (0..width).filter(move |&col| now[col] != was[col])
        })
        .collect();
    if changed.is_empty() {
        None
    } else if changed.iter().all(|&col| col < 2) {
        Some('L')
    } else if changed.iter().all(|&col| col >= width - 2) {
        Some('R')
    } else {
        None
    }
}

#[test]
fn a_wave_closes_on_a_wink_of_the_eye_beside_the_arm() {
    // The crest and the seedling wave an arm — and wink the eye on that
    // side when they are done: a hello.
    for mascot in [Mascot::Crest, Mascot::Sprout] {
        let (open, _) = eyes(mascot, mascot.art());
        let mut sides = BTreeSet::new();
        for clip in clips(mascot, SEED, MASCOT_GREETING_MS, 900_000) {
            let arms: BTreeSet<char> = clip
                .2
                .iter()
                .filter_map(|pose| arm_side(mascot, pose.art))
                .collect();
            let looks: Vec<(char, char)> = clip.2.iter().map(|p| eyes(mascot, p.art)).collect();
            let wink = match arms.iter().next() {
                Some('L') if arms.len() == 1 => ('█', open),
                Some('R') if arms.len() == 1 => (open, '█'),
                _ => continue,
            };
            if looks.contains(&wink) {
                sides.extend(arms);
                let other = (wink.1, wink.0);
                assert!(
                    !looks.contains(&other),
                    "{}: winked the other eye",
                    mascot.name()
                );
            }
        }
        assert_eq!(
            sides,
            BTreeSet::from(['L', 'R']),
            "{}: waves and winks on",
            mascot.name()
        );
    }
}

#[test]
fn feet_patter_on_the_spot() {
    // The four that stand on feet step from one pair to the other and back,
    // three times over, the rest of them still.
    for mascot in [Mascot::Crest, Mascot::Bloom, Mascot::Sprout, Mascot::Twin] {
        let rest = mascot.art();
        let pattered = clips(mascot, SEED, MASCOT_GREETING_MS, 900_000)
            .iter()
            .any(|clip| {
                let arts: Vec<&[&str]> = clip.2.iter().map(|pose| pose.art).collect();
                let steps: BTreeSet<&[&str]> = arts.iter().copied().collect();
                arts.len() >= 6
                    && steps.len() == 2
                    && arts.windows(2).all(|pair| pair[0] != pair[1])
                    && arts
                        .iter()
                        .all(|art| art[0] == rest[0] && art[1] == rest[1] && art[2] != rest[2])
            });
        assert!(pattered, "{} never patters", mascot.name());
    }
}

#[test]
fn eyes_wander_off_and_blink_where_they_went() {
    // A daydream: the eyes go to one side and stay there, blink while they
    // are away, and are still there after — something seen, and looked at.
    for mascot in EYED {
        let (left, right) = narrowed(mascot);
        for side in [left, right] {
            let wandered = clips(mascot, SEED, MASCOT_GREETING_MS, 900_000)
                .iter()
                .any(|clip| {
                    let looks: Vec<(char, char)> =
                        clip.2.iter().map(|pose| eyes(mascot, pose.art)).collect();
                    looks.first() == Some(&(side, side))
                        && looks.last() == Some(&(side, side))
                        && looks.contains(&('█', '█'))
                        && clip.1 - clip.0 >= 900
                });
            assert!(wandered, "{}: never wanders to {side:?}", mascot.name());
        }
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
            known.extend([('▆', '▆'), ('▂', '▂'), ('▂', '▆'), ('▆', open), (open, '▆')]);
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
    // …and no other mascot carries a spark beside it: theirs are in their
    // eyes, when those light up.
    for mascot in EYED {
        let eye_row = usize::from(mascot != Mascot::Skiter);
        for frame in mascot.motion_frames() {
            for (r, row) in frame.iter().enumerate() {
                for (c, glyph) in row.chars().enumerate() {
                    assert!(
                        !"·✢✶".contains(glyph) || (r == eye_row && (c == 2 || c == 6)),
                        "{}: a spark outside the eyes in {frame:?}",
                        mascot.name()
                    );
                }
            }
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
    // keeps a clock of its own: the banner may be half a minute into its
    // rhythm while the mascot under the highlight is a second and a half
    // into its greeting, showing its own move.
    let banner_at = 30_000;
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

// ===== the tricks =====

/// The eyes on a frame's bottom row — where they are once the mascot has
/// sunk to them.
fn floor_eyes(art: &[&str]) -> (char, char) {
    let cells: Vec<char> = art[art.len() - 1].chars().collect();
    let cell = |col: usize| cells.get(col).copied().unwrap_or(' ');
    (cell(2), cell(6))
}

#[test]
fn a_mascot_ducks_out_of_sight_and_peeks_back_over() {
    // It sinks below the floor of its block, comes back up to its eyes,
    // looks both ways over the edge, and pops up — every frame of it the
    // mascot as drawn, moved down whole rows.
    for mascot in EYED {
        let (left, right) = narrowed(mascot);
        let (open, _) = eyes(mascot, mascot.art());
        let hide = clips(mascot, SEED, MASCOT_GREETING_MS, 900_000)
            .into_iter()
            .find(|clip| clip.2.iter().all(|pose| sunk(mascot, pose.art).is_some()))
            .unwrap_or_else(|| panic!("{} never hides", mascot.name()));
        let depths: Vec<usize> = hide
            .2
            .iter()
            .filter_map(|pose| sunk(mascot, pose.art))
            .collect();
        // Down to its deepest, and back up the way it went.
        let deepest = *depths.iter().max().unwrap();
        let bottom = depths.iter().position(|&depth| depth == deepest).unwrap();
        assert_eq!(deepest, 2, "{}: {depths:?}", mascot.name());
        assert!(
            depths[..=bottom].windows(2).all(|pair| pair[0] <= pair[1])
                && depths[bottom..].windows(2).all(|pair| pair[0] >= pair[1]),
            "{}: {depths:?}",
            mascot.name()
        );
        assert_eq!(
            (depths[0], depths[depths.len() - 1]),
            (1, 1),
            "{}: {depths:?}",
            mascot.name()
        );
        // Its eyes at the floor, it looks one way and then the other.
        let looks: Vec<(char, char)> = hide.2.iter().map(|pose| floor_eyes(pose.art)).collect();
        for look in [(open, open), (left, left), (right, right)] {
            assert!(
                looks.contains(&look),
                "{}: never {look:?} over the edge: {looks:?}",
                mascot.name()
            );
        }
        let length = hide.1 - hide.0;
        assert!(
            (800..1_400).contains(&length),
            "{}: a {length}ms hide",
            mascot.name()
        );
    }
}

#[test]
fn the_gem_sets_like_a_sun_and_rises_glowing() {
    let gem = Mascot::Gem;
    let set = clips(gem, SEED, MASCOT_GREETING_MS, 900_000)
        .into_iter()
        .find(|clip| clip.2.iter().any(|pose| sunk(gem, pose.art).is_some()))
        .expect("the gem sets");
    let depths: Vec<usize> = set
        .2
        .iter()
        .filter_map(|pose| sunk(gem, pose.art))
        .collect();
    assert_eq!(depths, [1, 2, 1], "half down, down to its tip, half up");
    // Down it is dark; it comes up glowing — a sunrise — and the glow
    // fades once it is whole again.
    let glows: Vec<u8> = set.2.iter().map(|pose| pose.glow).collect();
    assert_eq!(&glows[..2], [0, 0], "{glows:?}");
    assert!(glows[2] > 0, "{glows:?}");
    let up: Vec<u8> = set
        .2
        .iter()
        .filter(|pose| pose.art == gem.art())
        .map(|pose| pose.glow)
        .collect();
    assert!(
        up.len() >= 2 && up.windows(2).all(|pair| pair[0] > pair[1]) && up[up.len() - 1] > 0,
        "{up:?}"
    );
}

#[test]
fn a_mascot_flips_head_over_heels() {
    // Upside down for an instant — the rest art turned top to bottom, legs
    // in the air — between a crouch and a landing (a somersault) or a lean
    // to one side and a lean to the other (a cartwheel).
    for mascot in EYED {
        let (mut somersaults, mut cartwheels) = (0, 0);
        for clip in clips(mascot, SEED, MASCOT_GREETING_MS, 900_000) {
            let Some(at) = clip
                .2
                .iter()
                .position(|pose| is_upside_down(mascot, pose.art))
            else {
                continue;
            };
            assert!(
                at > 0 && at + 1 < clip.2.len(),
                "{}: a flip with no take-off or no landing",
                mascot.name()
            );
            let (before, after) = (clip.2[at - 1].art, clip.2[at + 1].art);
            assert!(
                is_moved_half_a_cell(mascot, before) && is_moved_half_a_cell(mascot, after),
                "{}: {before:?} … {after:?}",
                mascot.name()
            );
            if before == after {
                somersaults += 1;
            } else {
                cartwheels += 1;
            }
            assert!(
                clip.1 - clip.0 <= 400,
                "{}: a {}ms flip",
                mascot.name(),
                clip.1 - clip.0
            );
        }
        // The bloom fills its block and cannot lean; the seedling stands on
        // the floor and cannot crouch.
        let wanted = match mascot {
            Mascot::Bloom => (true, false),
            Mascot::Sprout => (false, true),
            _ => (true, true),
        };
        assert_eq!(
            (somersaults > 0, cartwheels > 0),
            wanted,
            "{}: {somersaults} somersaults, {cartwheels} cartwheels",
            mascot.name()
        );
    }
    // The gem is the same either way up: upside down it is itself, dipped.
    assert!(
        is_moved_half_a_cell(
            Mascot::Gem,
            &upside_down(Mascot::Gem)
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
        ),
        "the gem upside down"
    );
}

/// Which half of the art `art` redraws, when it redraws only one: `'<'` for
/// cells left of the middle column, `'>'` for cells right of it.
fn half_moved(mascot: Mascot, art: &[&str]) -> Option<char> {
    let (rest, width) = (mascot.art(), mascot.art_width());
    let changed: Vec<usize> = (0..rest.len())
        .flat_map(|row| {
            let (now, was) = (padded(art[row], width), padded(rest[row], width));
            (0..width)
                .filter(move |&col| now[col] != was[col])
                .collect::<Vec<_>>()
        })
        .collect();
    let middle = width / 2;
    match changed.as_slice() {
        [] => None,
        cols if cols.iter().all(|&col| col < middle) => Some('<'),
        cols if cols.iter().all(|&col| col > middle) => Some('>'),
        _ => None,
    }
}

#[test]
fn every_eyed_mascot_pumps_its_limbs_one_after_the_other() {
    // A little victory dance: an arm, a petal, an ear goes up on one side,
    // then the other, and again — the skitterer puffs out one side of
    // itself and then the other — the face between them as drawn. (It
    // took the place of eyes that filled with stars: a glyph in a socket
    // reads as an eye put out, not one lit up.)
    for mascot in EYED {
        let pump = clips(mascot, SEED, MASCOT_GREETING_MS, 900_000)
            .into_iter()
            .find(|clip| {
                let sides: Vec<Option<char>> = clip
                    .2
                    .iter()
                    .map(|pose| half_moved(mascot, pose.art))
                    .collect();
                sides == [Some('<'), Some('>'), Some('<'), Some('>')]
            })
            .unwrap_or_else(|| panic!("{} never pumps its limbs", mascot.name()));
        assert!(
            pump.2
                .iter()
                .all(|pose| eyes(mascot, pose.art) == eyes(mascot, mascot.art())),
            "{}: its eyes moved",
            mascot.name()
        );
        assert!(
            (400..=600).contains(&(pump.1 - pump.0)),
            "{}: a {}ms pump",
            mascot.name(),
            pump.1 - pump.0
        );
    }
}

#[test]
fn no_light_is_ever_put_in_an_eye() {
    // An eye is a hole in the body: negative space. A spark or a mark drawn
    // in it fills the socket with the highlight — the eye reads as put out.
    // The one thing an eye may hold besides the body's own blocks is its
    // lid: the line of a closed eye, which is cut out of the ink like the
    // eye itself.
    for mascot in EYED {
        for frame in mascot.motion_frames() {
            let (left, right) = eyes(mascot, frame);
            for eye in [left, right] {
                assert!(
                    eye == ' '
                        || ('\u{2580}'..='\u{259F}').contains(&eye)
                        || MASCOT_LIDS.contains(&eye),
                    "{}: {eye:?} in an eye of {frame:?}",
                    mascot.name()
                );
            }
        }
    }
}

/// `mascot`'s rest art as it stands in the pose `frame` is drawn in: half a
/// cell lower where the frame is the mascot dipped, as drawn otherwise.
fn awake_in_the_pose_of(mascot: Mascot, frame: &[&str]) -> Vec<String> {
    let rest = mascot.art();
    if !is_moved_half_a_cell(mascot, frame) {
        return rest.iter().map(|row| (*row).to_string()).collect();
    }
    let grid = quarters(rest, mascot.art_width()).expect("quadrant glyphs");
    glyphs(&moved(&grid, 0, 1).expect("room to dip"))
}

#[test]
fn a_lid_is_the_line_along_the_bottom_of_its_socket() {
    // A closed eye is drawn where the eye was, and nowhere else: a thin
    // line along the bottom of the socket, where a lid comes to rest. A
    // socket is half a cell — the upper half of its cell or the lower — so
    // the line runs through the middle of the cell or along its bottom, and
    // it moves with the mascot when it dips.
    let mut seen = BTreeSet::new();
    for mascot in Mascot::ALL {
        for frame in mascot.motion_frames() {
            if lids(frame) == 0 {
                continue;
            }
            assert_ne!(mascot, Mascot::Gem, "the gem has no eyes to close");
            assert_eq!(lids(frame), 2, "{}: {frame:?}", mascot.name());
            let awake = awake_in_the_pose_of(mascot, frame);
            for (row, text) in frame.iter().enumerate() {
                for (col, glyph) in text.chars().enumerate() {
                    if !MASCOT_LIDS.contains(&glyph) {
                        continue;
                    }
                    let eye = awake[row].chars().nth(col);
                    let want = match eye {
                        // The socket is the cell's upper half.
                        Some('▄') => '━',
                        // …or its lower half.
                        Some('▀') => '▁',
                        _ => panic!(
                            "{}: a lid at row {row}, column {col} of {frame:?}, where it has no eye",
                            mascot.name()
                        ),
                    };
                    assert!(
                        col == 2 || col == 6,
                        "{}: a lid in column {col} of {frame:?}",
                        mascot.name()
                    );
                    assert_eq!(glyph, want, "{}: {frame:?}", mascot.name());
                    seen.insert((mascot.name(), glyph));
                }
            }
        }
    }
    // Every eyed mascot closes its eyes standing and with its head down —
    // but the seedling, which droops its shoot and stands where it is, and
    // the skitterer, which is on its belly before it closes them.
    for mascot in EYED {
        let both = MASCOT_LIDS
            .iter()
            .filter(|lid| seen.contains(&(mascot.name(), **lid)))
            .count();
        let want = if matches!(mascot, Mascot::Sprout | Mascot::Skiter) {
            1
        } else {
            2
        };
        assert_eq!(both, want, "{}: {seen:?}", mascot.name());
    }
}

#[test]
fn every_mascot_glitters() {
    // Scattered cells of it twinkle for most of a second: light only, the
    // art at rest under it.
    for mascot in Mascot::ALL {
        let glitters: Vec<_> = light_clips(mascot)
            .into_iter()
            .filter(|clip| {
                clip.2.iter().all(|pose| {
                    pose.shine
                        .is_some_and(|shine| shine.band == ShineBand::Glitter)
                })
            })
            .collect();
        assert!(
            glitters.len() >= 3,
            "{}: {} glitters in ten minutes",
            mascot.name(),
            glitters.len()
        );
        for clip in glitters {
            let length = clip.1 - clip.0;
            assert!(
                (600..=900).contains(&length),
                "{}: a {length}ms glitter",
                mascot.name()
            );
            let progress: Vec<f32> = clip
                .2
                .iter()
                .filter_map(|pose| pose.shine)
                .map(|shine| shine.progress)
                .collect();
            assert!(
                progress.windows(2).all(|pair| pair[0] < pair[1]),
                "{}: {progress:?}",
                mascot.name()
            );
            assert!(
                clip.2.iter().all(|pose| pose.glow == 0),
                "{}",
                mascot.name()
            );
        }
    }
}

// ===== the nap =====

/// An app animating `mascot` on `seed` that nobody touches, its first frame
/// at the top of the clock.
fn left_alone(mascot: Mascot, seed: u64) -> App {
    let mut app = App::new();
    app.seed_mascot_motion(seed);
    app.set_mascot(mascot);
    app.set_mascot_clock(ms(0));
    app
}

/// The banner's timeline on `app` over `from..to`, walked the way the loop
/// walks it: draw the pose, sleep for as long as a frame is not due, draw
/// the next.
fn banner_spans(app: &mut App, from: u64, to: u64) -> Vec<Span> {
    let mut out = Vec::new();
    let mut at = from;
    while at < to {
        app.set_mascot_clock(ms(at));
        let pose = app.banner_pose();
        let due = app.mascot_frame_in(true).expect("a frame is due");
        let hold = u64::try_from(due.as_millis()).unwrap();
        assert!(hold > 0, "a pose that holds for no time at {at}");
        out.push(Span {
            from: at,
            to: at + hold,
            pose,
        });
        at += hold;
    }
    out
}

/// The **naps** in the banner's timeline over `from..to`: each run off the
/// rest pose far too long to be a move.
fn naps(app: &mut App, from: u64, to: u64) -> Vec<(u64, u64, Vec<MascotPose>)> {
    let spans = banner_spans(app, from, to);
    clips_of(app.mascot(), &spans)
        .into_iter()
        .filter(|clip| clip.1 - clip.0 >= 20_000)
        .collect()
}

/// What rides in the top row of `art` beside the mascot: its marks, in
/// order.
fn marks(art: &[&str]) -> String {
    art[0]
        .chars()
        .filter(|glyph| MARKS.contains(glyph))
        .collect()
}

#[test]
fn a_mascot_left_alone_nods_off_within_the_minute() {
    // Nobody at the keyboard: it plays for three quarters of a minute and
    // then — every one of the six — it grows drowsy and falls asleep where
    // it stands. Not sooner: a mascot asleep as the app opened would be
    // saying something about the app.
    for mascot in Mascot::ALL {
        let rest = MascotPose::rest(mascot);
        for seed in SEEDS {
            let mut app = left_alone(mascot, seed);
            let naps = naps(&mut app, 0, 120_000);
            assert_eq!(naps.len(), 1, "{} (seed {seed:#x})", mascot.name());
            let (from, to, poses) = &naps[0];
            assert!(
                (45_000..=60_000).contains(from),
                "{} (seed {seed:#x}): nodded off {from}ms in",
                mascot.name()
            );
            assert!(
                (30_000..=55_000).contains(&(to - from)),
                "{} (seed {seed:#x}): a {}ms nap",
                mascot.name(),
                to - from
            );
            // Until then it is the mascot it always was, move for move —
            // but for a move the nap would have cut in two.
            for at in (0..*from).step_by(23) {
                app.set_mascot_clock(ms(at));
                let pose = app.banner_pose();
                assert!(
                    pose == mascot.pose(seed, ms(at)) || pose == rest,
                    "{} (seed {seed:#x}) at {at}",
                    mascot.name()
                );
            }
            // Four seconds on it is asleep — and all of it still there:
            // it sleeps where it stands, the whole of it on screen.
            assert!(
                poses.iter().all(|pose| whole(mascot, pose.art)),
                "{} (seed {seed:#x}): part of it left the screen",
                mascot.name()
            );
            app.set_mascot_clock(ms(from + 4_000));
            assert!(
                asleep(mascot, app.banner_pose().art),
                "{} (seed {seed:#x}): still awake four seconds on",
                mascot.name()
            );
        }
    }
}

/// What a mascot's eyes are doing in `art`, wherever it holds them:
/// `"closed"` — each a lid — `"heavy"`, a lid half way down, or `"open"`.
fn eyelids(art: &[&str]) -> &'static str {
    if lids(art) == 2 {
        "closed"
    } else if art.iter().any(|row| row.contains('▆')) {
        "heavy"
    } else {
        "open"
    }
}

#[test]
fn eyes_grow_heavy_before_they_close() {
    // Falling asleep is seen coming: the eyes droop — through their lid,
    // where they have one, shut where they have none — open again, and
    // close for good before the first snore.
    for mascot in EYED {
        let mut app = left_alone(mascot, SEED);
        let nap = naps(&mut app, 0, 120_000).remove(0);
        let (open, _) = eyes(mascot, mascot.art());
        let mut looks: Vec<&str> = nap
            .2
            .iter()
            .take_while(|pose| marks(pose.art).is_empty())
            .map(|pose| eyelids(pose.art))
            .collect();
        looks.dedup();
        // The skitterer settles onto its belly first, eyes still open.
        if mascot == Mascot::Skiter {
            assert_eq!(looks.remove(0), "open", "it settles first");
        }
        let drooping = if open == '▄' { "heavy" } else { "closed" };
        assert_eq!(
            looks[..3],
            [drooping, "open", drooping],
            "{}: {looks:?}",
            mascot.name()
        );
        assert_eq!(
            looks.last(),
            Some(&"closed"),
            "{}: {looks:?}",
            mascot.name()
        );
        assert!(
            looks[3..].iter().all(|look| *look == "closed"),
            "{}: its eyes opened again: {looks:?}",
            mascot.name()
        );
    }
}

#[test]
fn falling_asleep_it_goes_down_once_and_stays_there() {
    // It settles — its head down, the seedling's shoot drooped, the stone
    // half a cell low — once, and from there to the moment it stirs it does
    // not come back up: a sleeper bobbing up and down with each breath read
    // as one jumping on the spot. The skitterer and the stone, which settle
    // before anything else, are down from the nap's first frame.
    for mascot in Mascot::ALL {
        for seed in SEEDS {
            let mut app = left_alone(mascot, seed);
            let nap = naps(&mut app, 0, 120_000).remove(0);
            let mut postures: Vec<bool> = banner_spans(&mut app, nap.0, nap.1 - 4_000)
                .iter()
                .map(|span| on_its_feet(mascot, span.pose.art))
                .collect();
            postures.dedup();
            let settles_first = matches!(mascot, Mascot::Skiter | Mascot::Gem);
            let want: &[bool] = if settles_first {
                &[false]
            } else {
                &[true, false]
            };
            assert_eq!(
                postures,
                want,
                "{} (seed {seed:#x}): up and down as it sleeps",
                mascot.name()
            );
        }
    }
}

#[test]
fn asleep_it_holds_still_and_only_its_snore_moves() {
    // Asleep for the length of the nap, and all of it on screen: eyes
    // closed (where it has eyes), settled, and **still** — the one thing
    // of it that moves is its snore, a `z` in the corner of its block that
    // grows to a `Z` and is gone, over and over.
    for mascot in Mascot::ALL {
        let mut app = left_alone(mascot, SEED);
        let nap = naps(&mut app, 0, 120_000).remove(0);
        let sleeping = banner_spans(&mut app, nap.0 + 4_000, nap.1 - 4_000);
        let mut snores: Vec<String> = Vec::new();
        for span in &sleeping {
            let art = span.pose.art;
            assert!(
                asleep(mascot, art),
                "{}: {art:?} at {}",
                mascot.name(),
                span.from
            );
            assert!(
                whole(mascot, art),
                "{}: part of it is off screen in {art:?}",
                mascot.name()
            );
            let snore = marks(art);
            if snores.last() != Some(&snore) {
                snores.push(snore);
            }
            // Never so still that it reads as frozen.
            assert!(
                span.to - span.from <= 1_500,
                "{}: still for {}ms asleep",
                mascot.name(),
                span.to - span.from
            );
            assert_eq!(span.pose.shine, None, "{}", mascot.name());
        }
        let cycle = ["", "z", "Z"];
        let first = cycle
            .iter()
            .position(|snore| *snore == snores[0])
            .unwrap_or_else(|| panic!("{}: {:?}", mascot.name(), snores[0]));
        for (i, snore) in snores.iter().enumerate() {
            assert_eq!(
                snore,
                cycle[(first + i) % 3],
                "{}: {snores:?}",
                mascot.name()
            );
        }
        assert!(
            snores.len() >= 20,
            "{}: {} snores",
            mascot.name(),
            snores.len()
        );
        // A breath every three or four seconds.
        let breaths = snores.iter().filter(|snore| *snore == "z").count() as u64;
        let each = (nap.1 - nap.0 - 8_000) / breaths;
        assert!(
            (3_000..=4_200).contains(&each),
            "{}: a breath every {each}ms",
            mascot.name()
        );
        // Still: one body from its first breath to its last — it does not
        // rise and sink with each — and no light coming and going in it.
        let bodies: BTreeSet<Vec<String>> = sleeping
            .iter()
            .map(|span| unmarked(span.pose.art))
            .collect();
        assert_eq!(
            bodies.len(),
            1,
            "{}: it moves in its sleep: {bodies:#?}",
            mascot.name()
        );
        assert!(
            sleeping
                .iter()
                .all(|span| !on_its_feet(mascot, span.pose.art)),
            "{}: it sleeps standing up",
            mascot.name()
        );
        let glows: BTreeSet<u8> = sleeping.iter().map(|span| span.pose.glow).collect();
        assert_eq!(glows, BTreeSet::from([0]), "{}", mascot.name());
        // So a step of its sleep redraws one cell: the corner the snore
        // sits in.
        for pair in sleeping.windows(2) {
            let (was, now) = (pair[0].pose.art, pair[1].pose.art);
            let changed: Vec<(usize, usize)> = (0..was.len())
                .flat_map(|row| {
                    let width = mascot.art_width();
                    let (a, b) = (padded(was[row], width), padded(now[row], width));
                    (0..width)
                        .filter(move |&col| a[col] != b[col])
                        .map(move |col| (row, col))
                })
                .collect();
            assert_eq!(
                changed,
                [(0, mascot.art_width() - 1)],
                "{}: {was:?} to {now:?}",
                mascot.name()
            );
        }
    }
}

#[test]
fn asleep_its_eyes_are_closed_not_gone() {
    // A sleeper's eyes filled in solid leave it no face at all. Closed,
    // each shows its lid — from the moment they close for the nap to the
    // moment it stirs, standing and with its head down alike.
    for mascot in EYED {
        let mut app = left_alone(mascot, SEED);
        let nap = naps(&mut app, 0, 120_000).remove(0);
        let mut slept = 0;
        for span in banner_spans(&mut app, nap.0, nap.1 - 4_000) {
            let art = span.pose.art;
            if !asleep(mascot, art) {
                assert_eq!(
                    lids(art),
                    0,
                    "{}: a lid on open eyes in {art:?}",
                    mascot.name()
                );
                continue;
            }
            assert_eq!(
                lids(art),
                2,
                "{}: its eyes are blank in {art:?} at {}",
                mascot.name(),
                span.from
            );
            slept += span.to - span.from;
        }
        assert!(slept >= 20_000, "{}: asleep for {slept}ms", mascot.name());
    }
    // Awake, an eye that shuts is a blink: too quick to need a lid, and
    // drawn as it always was. No move of the repertoire draws one.
    for mascot in Mascot::ALL {
        for span in spans(mascot, SEED, 0, 300_000) {
            assert_eq!(
                lids(span.pose.art),
                0,
                "{}: {:?}",
                mascot.name(),
                span.pose.art
            );
        }
    }
    // The stone has no eyes to close.
    for frame in Mascot::Gem.motion_frames() {
        assert_eq!(lids(frame), 0, "{frame:?}");
    }
}

#[test]
fn a_snore_rises_in_the_corner_beside_the_sleepers_head() {
    // The `z`, and the `Z` it grows to, sit in the top right corner of the
    // block — the one cell beside the head every sleeper leaves free.
    for mascot in Mascot::ALL {
        let mut app = left_alone(mascot, SEED);
        let nap = naps(&mut app, 0, 120_000).remove(0);
        let mut seen = BTreeSet::new();
        for pose in &nap.2 {
            let snore = marks(pose.art);
            if snore.is_empty() {
                continue;
            }
            let top: Vec<char> = pose.art[0].chars().collect();
            assert_eq!(
                top.len(),
                mascot.art_width(),
                "{}: {:?}",
                mascot.name(),
                pose.art[0]
            );
            assert_eq!(
                top.last().map(char::to_string),
                Some(snore.clone()),
                "{}: {:?}",
                mascot.name(),
                pose.art[0]
            );
            seen.insert(snore);
        }
        assert_eq!(seen.len(), 2, "{}: {seen:?}", mascot.name());
    }
}

#[test]
fn a_nap_ends_by_itself_and_it_is_up_again() {
    // Three quarters of a minute, and it stirs: its eyes open, a stretch,
    // and — where it has eyes — two blinks to clear the sleep from them.
    // Then it is itself again.
    for mascot in Mascot::ALL {
        let rest = MascotPose::rest(mascot);
        let mut app = left_alone(mascot, SEED);
        let nap = naps(&mut app, 0, 120_000).remove(0);
        let waking = banner_spans(&mut app, nap.1 - 2_500, nap.1);
        assert!(
            waking.iter().all(|span| whole(mascot, span.pose.art)),
            "{}: part of it left the screen as it woke",
            mascot.name()
        );
        if mascot != Mascot::Gem {
            let (open, _) = eyes(mascot, mascot.art());
            // The stretch: on its feet, eyes open, something of it moved
            // that is not its eyes.
            let stretch = waking
                .iter()
                .position(|span| {
                    span.pose != rest
                        && on_its_feet(mascot, span.pose.art)
                        && eyes(mascot, span.pose.art) == (open, open)
                })
                .unwrap_or_else(|| panic!("{} never stretched", mascot.name()));
            assert!(
                waking[..stretch]
                    .iter()
                    .any(|span| asleep(mascot, span.pose.art)),
                "{}: it was not asleep before it stretched",
                mascot.name()
            );
            let after = &waking[stretch + 1..];
            let mut blinks = 0;
            let mut was_shut = false;
            for span in after {
                assert!(
                    on_its_feet(mascot, span.pose.art) && marks(span.pose.art).is_empty(),
                    "{}: {:?} once it is up",
                    mascot.name(),
                    span.pose.art
                );
                let shut = eyes(mascot, span.pose.art) == ('█', '█');
                blinks += usize::from(shut && !was_shut);
                was_shut = shut;
            }
            assert_eq!(blinks, 2, "{}", mascot.name());
        }
        // …and a breath later it is on its own timeline again, from the
        // first move that begins once it is up.
        let up = nap.1 + BREATH;
        let missed: Vec<(u64, u64)> = clips(mascot, SEED, nap.1 - 3_000, up + 20_000)
            .iter()
            .filter(|clip| clip.0 < up && clip.1 > nap.1)
            .map(|clip| (clip.0, clip.1))
            .collect();
        for at in (up..up + 20_000).step_by(13) {
            app.set_mascot_clock(ms(at));
            let want = if missed.iter().any(|&(from, to)| at >= from && at < to) {
                rest
            } else {
                mascot.pose(SEED, ms(at))
            };
            assert_eq!(app.banner_pose(), want, "{} at {at}", mascot.name());
        }
    }
}

#[test]
fn the_gem_settles_as_its_light_goes_out_and_rises_glowing() {
    // No eyes to close: the stone stops floating and settles half a cell
    // low, a last light fading out of it — and sleeps dark, as still as
    // the rest. It wakes the way a day does, rising in a glow that fades
    // once it is up.
    let gem = Mascot::Gem;
    let mut app = left_alone(gem, SEED);
    let nap = naps(&mut app, 0, 120_000).remove(0);
    let settling: Vec<u8> = banner_spans(&mut app, nap.0, nap.0 + 2_000)
        .iter()
        .map(|span| span.pose.glow)
        .collect();
    assert!(
        (20..=40).contains(&settling[0]),
        "a soft light as it settles: {settling:?}"
    );
    assert!(
        settling.windows(2).all(|pair| pair[1] <= pair[0]),
        "it only fades: {settling:?}"
    );
    assert_eq!(settling.last(), Some(&0), "{settling:?}");
    let sleeping = banner_spans(&mut app, nap.0 + 4_000, nap.1 - 4_000);
    assert!(
        sleeping.iter().all(|span| span.pose.glow == 0),
        "it sleeps in the dark"
    );
    let waking = banner_spans(&mut app, nap.1 - 2_500, nap.1);
    let glows: Vec<u8> = waking.iter().map(|span| span.pose.glow).collect();
    assert!(
        glows.iter().copied().max().unwrap() > settling[0],
        "{glows:?}"
    );
    let risen = waking
        .iter()
        .position(|span| !asleep(gem, span.pose.art))
        .expect("it rises");
    assert!(waking[risen].pose.glow > 0, "it is up in the glow");
    assert_eq!(waking.last().unwrap().pose.glow, 0, "and the glow is gone");
}

#[test]
fn no_move_is_cut_short_by_a_nap() {
    // Either side of a nap the mascot is on its own timeline, move for
    // move — and a move the nap would have cut in two is left out whole,
    // so it nods off standing still and wakes to a move with a start.
    for mascot in Mascot::ALL {
        let rest = MascotPose::rest(mascot);
        for seed in SEEDS {
            let mut app = left_alone(mascot, seed);
            let naps = naps(&mut app, 0, 240_000);
            assert_eq!(naps.len(), 2, "{} (seed {seed:#x})", mascot.name());
            // Nothing else is going on as it goes under: from the first
            // sign of it to its first snore is the nod and half a breath —
            // never the tail of a move run into it.
            for nap in &naps {
                let snore = banner_spans(&mut app, nap.0, nap.0 + 8_000)
                    .into_iter()
                    .find(|span| !marks(span.pose.art).is_empty())
                    .expect("it snores");
                assert!(
                    snore.from - nap.0 <= 4_300,
                    "{} (seed {seed:#x}): {}ms from the first sign of a nap to its first snore",
                    mascot.name(),
                    snore.from - nap.0
                );
            }
            for clip in clips(mascot, seed, 0, 240_000) {
                let cut = naps
                    .iter()
                    .any(|nap| clip.1 + BREATH > nap.0 && clip.0 < nap.1 + BREATH);
                for at in (clip.0..clip.1).step_by(11) {
                    if naps.iter().any(|nap| at >= nap.0 && at < nap.1) {
                        continue;
                    }
                    app.set_mascot_clock(ms(at));
                    let want = if cut { rest } else { mascot.pose(seed, ms(at)) };
                    assert_eq!(
                        app.banner_pose(),
                        want,
                        "{} (seed {seed:#x}) at {at}",
                        mascot.name()
                    );
                }
            }
        }
    }
}

#[test]
fn two_naps_and_then_it_sleeps_until_it_is_woken() {
    // It naps, plays, naps — two minutes apart — and the third time, five
    // minutes into being left alone, it stays asleep: whoever comes back to
    // the keyboard an hour later, or a week, finds it sleeping, still
    // breathing.
    for mascot in Mascot::ALL {
        for seed in [SEED, 1, u64::MAX] {
            let mut app = left_alone(mascot, seed);
            let naps = naps(&mut app, 0, 900_000);
            assert_eq!(naps.len(), 3, "{} (seed {seed:#x})", mascot.name());
            assert!(
                (165_000..=180_000).contains(&naps[1].0),
                "{}: its second nap at {}",
                mascot.name(),
                naps[1].0
            );
            assert!(
                (285_000..=300_000).contains(&naps[2].0),
                "{}: its last nap at {}",
                mascot.name(),
                naps[2].0
            );
            assert!(naps[1].1 < 240_000, "{}: a cat-nap ends", mascot.name());
            assert!(naps[2].1 >= 900_000, "{}: it woke by itself", mascot.name());
            for at in [3_600_000, 36_000_000, 604_800_000, u64::MAX / 2_000_000] {
                app.set_mascot_clock(ms(at));
                let art = app.banner_pose().art;
                assert!(
                    asleep(mascot, art) && whole(mascot, art),
                    "{} at {at}: {art:?}",
                    mascot.name()
                );
                let due = app.mascot_frame_in(true).expect("it is breathing");
                assert!(due <= ms(1_500), "{} at {at}: {due:?}", mascot.name());
            }
        }
    }
}

#[test]
fn no_two_naps_begin_at_the_same_moment() {
    // When it nods off is drawn afresh: from the session, from the mascot,
    // and from when it was last left alone.
    let first = |mascot: Mascot, seed: u64| naps(&mut left_alone(mascot, seed), 0, 100_000)[0].0;
    let starts: BTreeSet<u64> = SEEDS
        .iter()
        .map(|&seed| first(Mascot::Crest, seed))
        .collect();
    assert!(starts.len() >= 5, "{starts:?}");
    let starts: BTreeSet<u64> = Mascot::ALL.iter().map(|&m| first(m, SEED)).collect();
    assert!(starts.len() >= 5, "{starts:?}");
}

#[test]
fn a_frame_is_due_at_every_change_while_it_sleeps() {
    // The loop sleeps for as long as the pose holds, asleep as awake: it
    // wakes at each step of nodding off, each stage of a breath, each step
    // of waking — and the frame it wakes for always shows something new.
    for mascot in Mascot::ALL {
        let mut app = left_alone(mascot, SEED);
        let mut at = 40_000;
        while at < 125_000 {
            app.set_mascot_clock(ms(at));
            let pose = app.banner_pose();
            let due = app.mascot_frame_in(true).expect("a frame is due");
            let hold = u64::try_from(due.as_millis()).unwrap();
            assert!(hold > 0, "{} at {at}", mascot.name());
            let held = |pose: MascotPose| (pose.art, pose.shine.map(|shine| shine.band), pose.glow);
            for t in (at..at + hold).step_by(7) {
                app.set_mascot_clock(ms(t));
                assert_eq!(
                    held(app.banner_pose()),
                    held(pose),
                    "{} at {t}: changed inside a hold from {at}",
                    mascot.name()
                );
            }
            app.set_mascot_clock(ms(at + hold));
            if pose.shine.is_none() {
                assert_ne!(
                    app.banner_pose(),
                    pose,
                    "{} at {}: woke for nothing",
                    mascot.name(),
                    at + hold
                );
            }
            at += hold;
        }
    }
}

#[test]
fn the_picker_preview_shows_a_mascot_awake() {
    // The preview is there to show how a mascot moves: left under the
    // highlight for minutes it goes on moving, and never falls asleep.
    for mascot in Mascot::ALL {
        let mut app = previewing(mascot, 0, 0);
        for at in (40_000..320_000).step_by(997) {
            app.set_mascot_clock(ms(at));
            assert_eq!(
                app.mascot_pose_for(mascot),
                mascot.pose(SEED, ms(at)),
                "{} at {at}",
                mascot.name()
            );
        }
    }
}

#[test]
fn a_mascot_that_does_not_animate_never_sleeps() {
    // No seed, or the setting off: it stands at rest however long it is
    // left, and no frame is ever due for it.
    let mut app = App::new();
    app.set_mascot_clock(ms(52_000));
    assert_eq!(app.banner_pose(), MascotPose::rest(Mascot::Crest));
    let mut app = left_alone(Mascot::Crest, SEED);
    app.settings_mut().animate_mascot = false;
    for at in [52_000, 3_600_000] {
        app.set_mascot_clock(ms(at));
        assert_eq!(app.banner_pose(), MascotPose::rest(Mascot::Crest));
        assert_eq!(app.mascot_frame_in(true), None);
    }
}

// ===== woken by the user =====

/// An app whose `mascot` was left alone until it was sound asleep, and
/// then given a sign of the user — `stir`, a key or a paste — and the
/// millisecond of the first frame after it.
fn woken(mascot: Mascot, stir: impl FnOnce(&mut App)) -> (App, u64) {
    woken_on(mascot, SEED, stir)
}

/// [`woken`], in the session `seed` plays.
fn woken_on(mascot: Mascot, seed: u64, stir: impl FnOnce(&mut App)) -> (App, u64) {
    let mut app = left_alone(mascot, seed);
    let nap = naps(&mut app, 0, 120_000).remove(0);
    let napping = nap.0 + 12_345;
    app.set_mascot_clock(ms(napping));
    assert!(
        asleep(mascot, app.banner_pose().art),
        "{}: not asleep at {napping}",
        mascot.name()
    );
    stir(&mut app);
    // The key asks for a frame; this is it.
    app.set_mascot_clock(ms(napping + 8));
    (app, napping + 8)
}

/// Whether `spans` show `mascot` **up and awake** from end to end: on its
/// feet, no snore over it, and its eyes never shut for longer than a blink.
fn stays_up(mascot: Mascot, spans: &[Span]) -> bool {
    spans.iter().all(|span| {
        let art = span.pose.art;
        on_its_feet(mascot, art)
            && !art[0].contains(['z', 'Z'])
            && !(asleep(mascot, art) && span.to - span.from > 400)
    })
}

#[test]
fn a_keystroke_wakes_a_sleeping_mascot_with_a_start() {
    // Asleep, and the user types: its eyes snap open and it flinches, a `!`
    // in the corner where the snore was — and it is on its feet, wide
    // awake, inside half a second. Most of a second and the start has worn
    // off.
    for mascot in Mascot::ALL {
        let (mut app, at) = woken(mascot, |app| type_str(app, "h"));
        let start = banner_spans(&mut app, at, at + 1_500);
        let first = start[0].pose;
        assert_eq!(marks(first.art), "!", "{}: {:?}", mascot.name(), first.art);
        assert_eq!(
            first.art[0].chars().count(),
            mascot.art_width(),
            "{}: the `!` sits in the block's top right corner",
            mascot.name()
        );
        assert!(
            whole(mascot, first.art),
            "{}: {:?}",
            mascot.name(),
            first.art
        );
        if mascot == Mascot::Gem {
            assert!(first.glow >= 40, "the stone lights up: {}", first.glow);
        } else {
            let (open, _) = eyes(mascot, mascot.art());
            let flinch = risen(mascot, first.art);
            let flinch: Vec<&str> = flinch.iter().map(String::as_str).collect();
            assert_eq!(
                eyes(mascot, &flinch),
                (open, open),
                "{}: its eyes are open before it is up",
                mascot.name()
            );
        }
        let up = start
            .iter()
            .position(|span| on_its_feet(mascot, span.pose.art) && !asleep(mascot, span.pose.art))
            .unwrap_or_else(|| panic!("{} never got up", mascot.name()));
        assert!(
            start[up].from - at <= 500,
            "{}: up {}ms after the key",
            mascot.name(),
            start[up].from - at
        );
        // Up it stays, and the snoring is over.
        assert!(
            stays_up(mascot, &start[up..]),
            "{}: it went back to sleep",
            mascot.name()
        );
        let marked: u64 = start
            .iter()
            .filter(|span| marks(span.pose.art) == "!")
            .map(|span| span.to - span.from)
            .sum();
        assert!(
            (300..=700).contains(&marked),
            "{}: the `!` for {marked}ms",
            mascot.name()
        );
        assert!(
            start
                .iter()
                .skip_while(|span| span.from < at + 800)
                .all(|span| marks(span.pose.art).is_empty()),
            "{}: still startled most of a second on",
            mascot.name()
        );
    }
}

#[test]
fn a_start_is_the_same_wherever_in_its_sleep_the_key_finds_it() {
    // Snoring or between two snores: the key finds it asleep and the start
    // is the one start.
    for mascot in Mascot::ALL {
        let mut starts = BTreeSet::new();
        for into in [4_100, 5_000, 5_700, 6_300, 6_900, 7_500] {
            let mut app = left_alone(mascot, SEED);
            let nap = naps(&mut app, 0, 120_000).remove(0);
            let at = nap.0 + into;
            app.set_mascot_clock(ms(at));
            assert!(
                asleep(mascot, app.banner_pose().art),
                "{} at {at}",
                mascot.name()
            );
            app.on_key(key(KeyCode::Left));
            let start: Vec<(Vec<String>, u8, u64)> = banner_spans(&mut app, at + 8, at + 600)
                .iter()
                .map(|span| {
                    let art = span.pose.art.iter().map(|row| (*row).to_string()).collect();
                    (art, span.pose.glow, span.to - span.from)
                })
                .collect();
            starts.insert(format!("{start:?}"));
        }
        assert_eq!(starts.len(), 1, "{}: {starts:#?}", mascot.name());
    }
}

#[test]
fn caught_on_its_feet_it_is_never_taken_down() {
    // The flicker: a key that found it still on its feet — its eyes only
    // growing heavy — dropped it into the sleeper's crouch for an eighth
    // of a second and brought it straight back up, the whole block redrawn
    // twice. A start never takes a mascot down: caught standing it starts
    // where it stands, and one that is down comes up once and stays up.
    for mascot in Mascot::ALL {
        let nap = naps(&mut left_alone(mascot, SEED), 0, 120_000).remove(0);
        let mut caught_standing = 0;
        for into in (0..4_000).step_by(45) {
            let at = nap.0 + into;
            // Where the key's own frame finds it.
            let mut calm = left_alone(mascot, SEED);
            calm.set_mascot_clock(ms(at + 8));
            let standing = on_its_feet(mascot, calm.banner_pose().art);
            let mut app = left_alone(mascot, SEED);
            app.set_mascot_clock(ms(at));
            type_str(&mut app, "h");
            let start = banner_spans(&mut app, at + 8, at + 1_500);
            assert_eq!(
                marks(start[0].pose.art),
                "!",
                "{} {into}ms into its nap",
                mascot.name()
            );
            let mut postures: Vec<bool> = start
                .iter()
                .map(|span| on_its_feet(mascot, span.pose.art))
                .collect();
            postures.dedup();
            let want: &[bool] = if standing { &[true] } else { &[false, true] };
            assert_eq!(
                postures,
                want,
                "{}: a key {into}ms into its nap found it {} and moved it {postures:?}",
                mascot.name(),
                if standing { "on its feet" } else { "down" },
            );
            caught_standing += usize::from(standing);
        }
        // The four that droop where they stand can be caught doing it.
        let droops_standing = !matches!(mascot, Mascot::Skiter | Mascot::Gem);
        assert_eq!(
            caught_standing > 0,
            droops_standing,
            "{}: caught standing {caught_standing} times",
            mascot.name()
        );
    }
}

#[test]
fn woken_it_reads_a_while_before_it_blinks() {
    // The start is three looks in three fifths of a second. Its eyes then
    // turn to the text and stay there: the watch's first blink is counted
    // from the end of the start, not from the key — it used to come a
    // third of a second after the eyes had turned.
    for mascot in EYED {
        for seed in SEEDS {
            let (mut app, at) = woken_on(mascot, seed, |app| type_str(app, "hello"));
            let watching = MascotPose::rest(mascot);
            let mut blinked = None;
            let mut t = at;
            // A key every two seconds keeps it watching.
            while blinked.is_none() && t < at + 12_000 {
                blinked = banner_spans(&mut app, t.max(at + 600), t + 2_000)
                    .into_iter()
                    .find(|span| span.pose != watching)
                    .map(|span| span.from);
                t += 2_000;
                type_str(&mut app, "x");
            }
            let blinked = blinked
                .unwrap_or_else(|| panic!("{} (seed {seed:#x}) never blinked", mascot.name()));
            assert!(
                blinked >= at + 600 + 900,
                "{} (seed {seed:#x}): blinked {}ms after its eyes turned to the text",
                mascot.name(),
                blinked - at - 600
            );
        }
    }
}

#[test]
fn a_lidded_mascot_starts_awake_wide_eyed() {
    // Eyes that can open wide do, the moment it is on its feet.
    for mascot in [Mascot::Crest, Mascot::Bloom, Mascot::Twin] {
        let (mut app, at) = woken(mascot, |app| type_str(app, "h"));
        let start = banner_spans(&mut app, at, at + 450);
        let up = start
            .iter()
            .find(|span| on_its_feet(mascot, span.pose.art))
            .expect("it gets up");
        assert_eq!(
            eyes(mascot, &unmarked_rows(up.pose.art)),
            ('▂', '▂'),
            "{}",
            mascot.name()
        );
    }
}

#[test]
fn the_gem_starts_awake_in_a_flash() {
    // No eyes to open wide: the stone is up in a flash of light.
    let (mut app, at) = woken(Mascot::Gem, |app| type_str(app, "h"));
    let start = banner_spans(&mut app, at, at + 1_500);
    let brightest = start.iter().map(|span| span.pose.glow).max().unwrap();
    assert_eq!(brightest, 100);
    assert_eq!(start.last().unwrap().pose.glow, 0, "and the flash is over");
}

#[test]
fn woken_it_turns_to_what_is_being_typed() {
    // The start first — whatever the caret is doing — and then the watch:
    // its eyes toward the caret, as if it had been awake all along.
    for mascot in EYED {
        let (mut app, at) = woken(mascot, |app| type_str(app, "hello"));
        let (left, right) = narrowed(mascot);
        // Mid-start, the caret is nothing to it.
        app.set_mascot_clock(ms(at + 150));
        assert_eq!(
            app.banner_pose_watching(Gaze::Left),
            app.banner_pose_watching(Gaze::Right),
            "{}",
            mascot.name()
        );
        // The start over, it reads along — and blinks no sooner than it
        // would have, had it been awake.
        app.set_mascot_clock(ms(at + 760));
        let look = |gaze: Gaze| eyes(mascot, app.banner_pose_watching(gaze).art);
        assert_eq!(look(Gaze::Left), (left, left), "{}", mascot.name());
        assert_eq!(look(Gaze::Right), (right, right), "{}", mascot.name());
    }
    let (mut app, at) = woken(Mascot::Gem, |app| type_str(app, "hello"));
    app.set_mascot_clock(ms(at + 760));
    assert_eq!(app.banner_pose_watching(Gaze::Right).art[1], "▝▀███▄█▀▘");
}

#[test]
fn any_key_wakes_it_even_one_that_types_nothing() {
    // An arrow key in an empty composer leaves nothing to read — but
    // someone is there: it starts awake all the same, and is itself again
    // once the start has worn off.
    for mascot in Mascot::ALL {
        let (mut app, at) = woken(mascot, |app| {
            app.on_key(key(KeyCode::Left));
        });
        assert!(app.input.is_empty());
        assert_eq!(marks(app.banner_pose().art), "!", "{}", mascot.name());
        // …its own moves, each a couple of seconds at most: no snore, no
        // start, nothing as long as a nap.
        let after = banner_spans(&mut app, at + 800, at + 40_000);
        assert!(
            after.iter().all(|span| marks(span.pose.art).is_empty()),
            "{}",
            mascot.name()
        );
        let moves = clips_of(mascot, &after);
        assert!(moves.len() >= 5, "{}: {} moves", mascot.name(), moves.len());
        assert!(
            moves.iter().all(|clip| clip.1 - clip.0 <= 2_800),
            "{}: asleep again",
            mascot.name()
        );
    }
}

#[test]
fn a_paste_wakes_it_too() {
    let (app, _) = woken(Mascot::Twin, |app| app.on_paste("pasted text"));
    assert_eq!(marks(app.banner_pose().art), "!");
}

#[test]
fn a_paste_that_lands_outside_the_draft_wakes_it_all_the_same() {
    // Pasted into an open Ctrl+R search, the text extends the query and the
    // composer's draft is what it was — but somebody pasted it.
    let mascot = Mascot::Crest;
    let mut app = left_alone(mascot, SEED);
    app.on_key(ctrl('r'));
    assert!(app.history_search.is_some(), "the search is open");
    app.set_mascot_clock(ms(10));
    let nap = naps(&mut app, 10, 130_000).remove(0);
    app.set_mascot_clock(ms(nap.0 + 12_345));
    assert!(asleep(mascot, app.banner_pose().art));
    let draft = (app.input.text().to_string(), app.input.cursor());
    app.on_paste("needle");
    assert_eq!(
        (app.input.text().to_string(), app.input.cursor()),
        draft,
        "the paste went to the search, not the draft"
    );
    app.set_mascot_clock(ms(nap.0 + 12_353));
    assert_eq!(marks(app.banner_pose().art), "!");
}

#[test]
fn woken_it_stays_up_three_quarters_of_a_minute_before_it_naps_again() {
    // The nap is counted from the last sign of the user, not from the
    // clock: a mascot just woken does not nod straight off again — and one
    // left alone again does, in its own time.
    for mascot in Mascot::ALL {
        let (mut app, at) = woken(mascot, |app| {
            app.on_key(key(KeyCode::Left));
        });
        let naps = naps(&mut app, at, at + 125_000);
        assert_eq!(naps.len(), 1, "{}", mascot.name());
        assert!(
            (45_000..=61_000).contains(&(naps[0].0 - at)),
            "{}: asleep again {}ms after it was woken",
            mascot.name(),
            naps[0].0 - at
        );
    }
}

#[test]
fn it_naps_again_only_after_the_typing_stops() {
    // While a draft is being worked on it watches; the three quarters of a
    // minute run from when it looked away.
    let mascot = Mascot::Crest;
    let (mut app, at) = woken(mascot, |app| type_str(app, "a draft"));
    // A key every two seconds for a minute and a half: never asleep.
    let mut t = at;
    while t < at + 90_000 {
        t += 2_000;
        type_str(&mut app, "x");
        let typing = banner_spans(&mut app, t, t + 1_990);
        assert!(stays_up(mascot, &typing), "asleep after {t} while typed at");
    }
    let naps = naps(&mut app, t, t + 125_000);
    let looked_away = t + linger();
    assert!(
        (45_000..=61_000).contains(&(naps[0].0 - looked_away)),
        "asleep {}ms after it looked away",
        naps[0].0 - looked_away
    );
}

#[test]
fn a_key_keeps_a_wakeful_mascot_up_and_gives_it_no_start() {
    // Awake, a key is no surprise: no `!` — but the nap it was heading for
    // is put off.
    for mascot in Mascot::ALL {
        let mut app = left_alone(mascot, SEED);
        app.set_mascot_clock(ms(40_000));
        app.on_key(key(KeyCode::Left));
        app.set_mascot_clock(ms(40_008));
        let after = banner_spans(&mut app, 40_008, 84_000);
        assert!(
            after.iter().all(|span| marks(span.pose.art).is_empty()),
            "{}: a start, or a snore",
            mascot.name()
        );
        assert!(
            clips_of(mascot, &after)
                .iter()
                .all(|clip| clip.1 - clip.0 <= 2_800),
            "{}: it napped all the same",
            mascot.name()
        );
        // Left alone from there, it nods off in its own time.
        let naps = naps(&mut app, 84_000, 180_000);
        assert!(
            (85_000..=101_000).contains(&naps[0].0),
            "{}: asleep at {}",
            mascot.name(),
            naps[0].0
        );
    }
}

#[test]
fn a_key_that_types_nothing_leaves_a_move_under_way_alone() {
    // An arrow key in an empty composer says someone is there, so the nap
    // is put off — but it gives the mascot nothing to look at, and the
    // move it is in the middle of plays on. It used to stop dead at the
    // key: a glance cut off a twentieth of a second in, a flip left
    // upside down for a frame.
    for mascot in Mascot::ALL {
        let moves: Vec<_> = clips(mascot, SEED, MASCOT_GREETING_MS, 40_000)
            .into_iter()
            .filter(|clip| clip.1 - clip.0 >= 300)
            .collect();
        assert!(moves.len() >= 4, "{}", mascot.name());
        for clip in moves {
            let key_at = clip.0 + (clip.1 - clip.0) / 3;
            let mut app = left_alone(mascot, SEED);
            app.set_mascot_clock(ms(key_at));
            app.on_key(key(KeyCode::Left));
            for at in (key_at + 8..clip.1 + 400).step_by(9) {
                app.set_mascot_clock(ms(at));
                assert_eq!(
                    app.banner_pose(),
                    mascot.pose(SEED, ms(at)),
                    "{} at {at}: a key at {key_at} cut its move short",
                    mascot.name()
                );
            }
        }
    }
}

#[test]
fn a_key_as_a_nap_nears_never_joins_a_move_half_way() {
    // A move the coming nap left out — it would have been cut in two —
    // stays left out when a key puts the nap off half way through it: it
    // does not start from its middle.
    let mut tried = 0;
    for mascot in Mascot::ALL {
        let rest = MascotPose::rest(mascot);
        for seed in SEEDS {
            let nap = naps(&mut left_alone(mascot, seed), 0, 120_000).remove(0);
            let Some(left_out) = clips(mascot, seed, nap.0 - 5_000, nap.0 + 5_000)
                .into_iter()
                .find(|clip| clip.0 + 150 < nap.0 && clip.1 + BREATH > nap.0)
            else {
                continue;
            };
            let key_at = (left_out.0 + (left_out.1 - left_out.0) / 2).min(nap.0 - 30);
            if key_at <= left_out.0 {
                continue;
            }
            let mut app = left_alone(mascot, seed);
            app.set_mascot_clock(ms(key_at));
            assert_eq!(
                app.banner_pose(),
                rest,
                "{} (seed {seed:#x}): the nap left it out",
                mascot.name()
            );
            app.on_key(key(KeyCode::Left));
            for at in (key_at + 8..left_out.1).step_by(7) {
                app.set_mascot_clock(ms(at));
                assert_eq!(
                    app.banner_pose(),
                    rest,
                    "{} (seed {seed:#x}) at {at}: joined a move begun at {}",
                    mascot.name(),
                    left_out.0
                );
            }
            // …and it is up for another three quarters of a minute, its
            // next moves whole.
            let after = banner_spans(&mut app, left_out.1, left_out.1 + 30_000);
            assert!(
                after.iter().all(|span| marks(span.pose.art).is_empty()),
                "{} (seed {seed:#x})",
                mascot.name()
            );
            assert!(
                clips_of(mascot, &after).len() >= 4,
                "{} (seed {seed:#x})",
                mascot.name()
            );
            tried += 1;
        }
    }
    assert!(tried >= 5, "only {tried} naps had a move to leave out");
}

#[test]
fn caught_nodding_off_it_starts_too() {
    // Its eyes were closing: a key then is as much a surprise as one a
    // minute later.
    for mascot in Mascot::ALL {
        let mut app = left_alone(mascot, SEED);
        let nap = naps(&mut app, 0, 120_000).remove(0);
        app.set_mascot_clock(ms(nap.0 + 700));
        type_str(&mut app, "h");
        app.set_mascot_clock(ms(nap.0 + 708));
        assert_eq!(marks(app.banner_pose().art), "!", "{}", mascot.name());
    }
}

#[test]
fn a_key_as_it_wakes_by_itself_is_no_surprise() {
    // Already stirring — up, stretching, blinking — it needs no start: it
    // just turns to the text.
    for mascot in EYED {
        let mut app = left_alone(mascot, SEED);
        let nap = naps(&mut app, 0, 120_000).remove(0);
        let waking = nap.1 - 600;
        app.set_mascot_clock(ms(waking));
        type_str(&mut app, "hello");
        let (_, right) = narrowed(mascot);
        for t in (waking + 8..waking + 800).step_by(40) {
            app.set_mascot_clock(ms(t));
            let pose = app.banner_pose_watching(Gaze::Right);
            assert!(marks(pose.art).is_empty(), "{} at {t}", mascot.name());
            assert_eq!(
                eyes(mascot, pose.art),
                (right, right),
                "{} at {t}",
                mascot.name()
            );
        }
    }
}

#[test]
fn it_can_be_woken_from_the_sleep_that_has_no_end() {
    // Five minutes left alone and it sleeps until someone is back — an
    // hour later, say. Then it starts awake like any other time, and the
    // count begins again: two naps, and the long sleep.
    for mascot in Mascot::ALL {
        let mut app = left_alone(mascot, SEED);
        let hour = 3_600_000;
        app.set_mascot_clock(ms(hour));
        assert!(asleep(mascot, app.banner_pose().art));
        app.on_key(key(KeyCode::Left));
        app.set_mascot_clock(ms(hour + 8));
        assert_eq!(marks(app.banner_pose().art), "!", "{}", mascot.name());
        let naps = naps(&mut app, hour + 8, hour + 700_000);
        assert_eq!(naps.len(), 3, "{}", mascot.name());
        assert!(
            naps[0].1 < hour + 120_000,
            "{}: a cat-nap first",
            mascot.name()
        );
        assert!(naps[2].1 >= hour + 700_000, "{}", mascot.name());
    }
}

#[test]
fn a_frame_is_due_at_every_change_through_a_start() {
    // The loop is woken by the key, and from there for every step of the
    // start, the watch after it, and the timeline after that.
    for mascot in Mascot::ALL {
        let (mut app, woke) = woken(mascot, |app| type_str(app, "hi"));
        let mut at = woke;
        while at < woke + linger() + 8_000 {
            app.set_mascot_clock(ms(at));
            let pose = app.banner_pose_watching(Gaze::Left);
            let due = app.mascot_frame_in(true).expect("a frame is due");
            let hold = u64::try_from(due.as_millis()).unwrap();
            assert!(hold > 0, "{} at {at}", mascot.name());
            let held = |pose: MascotPose| (pose.art, pose.shine.map(|shine| shine.band), pose.glow);
            for t in (at..at + hold).step_by(7) {
                app.set_mascot_clock(ms(t));
                assert_eq!(
                    held(app.banner_pose_watching(Gaze::Left)),
                    held(pose),
                    "{} at {t}: changed inside a hold from {at}",
                    mascot.name()
                );
            }
            app.set_mascot_clock(ms(at + hold));
            if pose.shine.is_none() {
                assert_ne!(
                    app.banner_pose_watching(Gaze::Left),
                    pose,
                    "{} at {}: woke for nothing",
                    mascot.name(),
                    at + hold
                );
            }
            at += hold;
        }
    }
}

#[test]
fn a_clock_that_starts_over_forgets_it_was_woken() {
    // `/clear` and a `/mascot` choice start the clock over so the banner
    // greets: the start, and when it was last disturbed, belong to the old
    // clock.
    let mascot = Mascot::Crest;
    let (mut app, _) = woken(mascot, |app| {
        app.on_key(key(KeyCode::Left));
    });
    for at in [100, 700, 2_000, 30_000] {
        app.set_mascot_clock(ms(at));
        assert_eq!(app.banner_pose(), mascot.pose(SEED, ms(at)), "at {at}");
    }
    // …and it naps on the new clock's schedule, as if never disturbed.
    let fresh = naps(&mut left_alone(mascot, SEED), 0, 120_000);
    app.set_mascot_clock(ms(0));
    assert_eq!(naps(&mut app, 0, 120_000), fresh);
}

#[test]
fn a_key_before_the_first_frame_is_nobody_at_the_keyboard_yet() {
    // The first frame only looks: what was typed before the banner was up
    // — a prompt on the command line — is no sign of anyone watching it.
    let mascot = Mascot::Crest;
    let mut app = App::new();
    app.seed_mascot_motion(SEED);
    type_str(&mut app, "typed ahead");
    app.input.clear();
    app.set_mascot_clock(ms(0));
    for at in (0..20_000).step_by(37) {
        app.set_mascot_clock(ms(at));
        assert_eq!(app.banner_pose(), mascot.pose(SEED, ms(at)), "at {at}");
    }
}

// ===== watching the user type =====

/// An app animating `mascot`, its first frame at `at` — an empty composer.
fn watching_app(mascot: Mascot, at: u64) -> App {
    let mut app = animated(at);
    app.set_mascot(mascot);
    app
}

/// How long the mascot keeps its eyes on the composer after a keystroke.
fn linger() -> u64 {
    u64::try_from(MASCOT_WATCH_LINGER.as_millis()).unwrap()
}

#[test]
fn a_caret_is_read_from_where_the_mascot_stands() {
    // The banner's mascot stands at the screen's left edge, over the first
    // columns of the composer. A caret still under its near half has it
    // looking straight ahead; one past its middle — where the line is
    // heading, and where the rest of the screen is — has it looking right.
    let stands = 0..9;
    for caret in 0..=4 {
        assert_eq!(
            Gaze::toward(caret, stands.clone()),
            Gaze::Ahead,
            "a caret at {caret}"
        );
    }
    for caret in [5, 8, 9, 40, 200] {
        assert_eq!(
            Gaze::toward(caret, stands.clone()),
            Gaze::Right,
            "a caret at {caret}"
        );
    }
    // It looks left only at a caret that is off to its left — which, at the
    // edge of the screen, no caret is.
    assert_eq!(Gaze::toward(3, 10..19), Gaze::Left);
    assert_eq!(Gaze::toward(9, 10..19), Gaze::Left);
    assert_eq!(Gaze::toward(10, 10..19), Gaze::Ahead);
    assert_eq!(Gaze::toward(14, 10..19), Gaze::Ahead);
    assert_eq!(Gaze::toward(15, 10..19), Gaze::Right);
    // A mascot with no columns to stand over is no reason to panic.
    let _ = Gaze::toward(3, 0..0);
}

#[test]
fn the_mascot_watches_the_caret_while_the_user_types() {
    // While a message is being typed the mascot reads along: its eyes go to
    // the left while the caret is in the composer's left third, straight
    // ahead in the middle, to the right in the right third.
    for mascot in EYED {
        let mut app = watching_app(mascot, 10_000);
        type_str(&mut app, "hello");
        app.set_mascot_clock(ms(10_100));
        let (left, right) = narrowed(mascot);
        let look = |gaze: Gaze| eyes(mascot, app.banner_pose_watching(gaze).art);
        assert_eq!(look(Gaze::Left), (left, left), "{}", mascot.name());
        assert_eq!(look(Gaze::Right), (right, right), "{}", mascot.name());
        assert_eq!(
            app.banner_pose_watching(Gaze::Ahead),
            MascotPose::rest(mascot),
            "{}",
            mascot.name()
        );
    }
}

#[test]
fn the_gem_turns_its_facet_toward_the_caret() {
    // No eyes to follow it with: the stone turns, its facet sliding a cell
    // toward the caret's side.
    let mut app = watching_app(Mascot::Gem, 10_000);
    type_str(&mut app, "hello");
    app.set_mascot_clock(ms(10_100));
    assert_eq!(app.banner_pose_watching(Gaze::Left).art[1], "▝▀█▄███▀▘");
    assert_eq!(app.banner_pose_watching(Gaze::Right).art[1], "▝▀███▄█▀▘");
}

#[test]
fn a_watching_mascot_still_blinks_now_and_then() {
    // Its eyes on the text, it blinks every few seconds, the way anything
    // reading does — and between blinks they stay on the caret.
    for mascot in EYED {
        let mut app = watching_app(mascot, 5_000);
        let (_, right) = narrowed(mascot);
        let mut blinks = 0;
        let mut was_shut = false;
        for (i, at) in (5_100..15_100u64).step_by(50).enumerate() {
            if i % 6 == 0 {
                type_str(&mut app, "x");
            }
            app.set_mascot_clock(ms(at));
            let look = eyes(mascot, app.banner_pose_watching(Gaze::Right).art);
            let shut = look == ('█', '█');
            blinks += usize::from(shut && !was_shut);
            was_shut = shut;
            assert!(
                [(right, right), ('█', '█'), ('▆', '▆')].contains(&look),
                "{}: {look:?} at {at}",
                mascot.name()
            );
        }
        assert!(
            (2..=5).contains(&blinks),
            "{}: {blinks} blinks in ten seconds of typing",
            mascot.name()
        );
    }
}

#[test]
fn the_mascot_goes_back_to_its_own_moves_when_the_typing_stops() {
    // A few seconds after the last keystroke it looks away from the text
    // and is itself again — but a move its timeline started meanwhile is
    // not joined half way through: it waits for the next.
    for mascot in Mascot::ALL {
        let mut app = watching_app(mascot, 20_000);
        type_str(&mut app, "hi");
        app.set_mascot_clock(ms(20_000));
        let end = 20_000 + linger();
        app.set_mascot_clock(ms(end - 1));
        if mascot != Mascot::Gem {
            let (_, right) = narrowed(mascot);
            let look = eyes(mascot, app.banner_pose_watching(Gaze::Right).art);
            assert!(
                [(right, right), ('█', '█'), ('▆', '▆')].contains(&look),
                "{}: still watching just before the end, {look:?}",
                mascot.name()
            );
        }
        let under_way: Vec<(u64, u64)> = clips(mascot, SEED, 0, end + 12_000)
            .iter()
            .filter(|clip| clip.0 < end && clip.1 > end)
            .map(|clip| (clip.0, clip.1))
            .collect();
        for at in (end..end + 12_000).step_by(5) {
            app.set_mascot_clock(ms(at));
            let pose = app.banner_pose_watching(Gaze::Right);
            if under_way.iter().any(|&(from, to)| at >= from && at < to) {
                assert_eq!(pose, MascotPose::rest(mascot), "{} at {at}", mascot.name());
            } else {
                assert_eq!(pose, mascot.pose(SEED, ms(at)), "{} at {at}", mascot.name());
            }
        }
    }
}

#[test]
fn a_frame_is_due_at_every_change_while_it_watches() {
    // The loop sleeps for as long as the pose holds: while the mascot
    // watches, to its next blink or the end of the watch — and the frame it
    // wakes for always shows something new.
    for mascot in Mascot::ALL {
        let mut app = watching_app(mascot, 30_000);
        type_str(&mut app, "hi");
        app.set_mascot_clock(ms(30_000));
        let end = 30_000 + linger() + 6_000;
        let mut at = 30_000;
        while at < end {
            app.set_mascot_clock(ms(at));
            let pose = app.banner_pose_watching(Gaze::Left);
            let due = app.mascot_frame_in(true).expect("a frame is due");
            let hold = u64::try_from(due.as_millis()).unwrap();
            assert!(hold > 0, "{} at {at}", mascot.name());
            // A band of light moves on within its frame — that is what the
            // 40 ms cadence is for — so inside a hold it is the art, the
            // band and the glow that stand still, not the band's progress.
            let held = |pose: MascotPose| (pose.art, pose.shine.map(|shine| shine.band), pose.glow);
            for t in (at..at + hold).step_by(7) {
                app.set_mascot_clock(ms(t));
                assert_eq!(
                    held(app.banner_pose_watching(Gaze::Left)),
                    held(pose),
                    "{} at {t}: changed inside a hold from {at}",
                    mascot.name()
                );
            }
            app.set_mascot_clock(ms(at + hold));
            if pose.shine.is_none() {
                assert_ne!(
                    app.banner_pose_watching(Gaze::Left),
                    pose,
                    "{} at {}: woke for nothing",
                    mascot.name(),
                    at + hold
                );
            }
            at += hold;
        }
    }
}

#[test]
fn an_emptied_composer_ends_the_watch_at_once() {
    // Ctrl+U, a message sent, `/clear` typed and run: nothing is left to
    // read, so the mascot looks away now rather than stare at an empty box.
    let mascot = Mascot::Crest;
    let mut app = watching_app(mascot, 40_000);
    type_str(&mut app, "hi");
    app.set_mascot_clock(ms(40_000));
    app.input.clear();
    app.set_mascot_clock(ms(40_500));
    let (left, _) = narrowed(mascot);
    assert_ne!(
        eyes(mascot, app.banner_pose_watching(Gaze::Left).art),
        (left, left)
    );
}

#[test]
fn a_launch_watches_nothing() {
    // The first frame sees an empty composer and no keystroke: the mascot
    // goes about its own business from the start.
    for mascot in Mascot::ALL {
        let mut app = watching_app(mascot, 0);
        for at in (0..20_000).step_by(37) {
            app.set_mascot_clock(ms(at));
            assert_eq!(
                app.banner_pose_watching(Gaze::Left),
                mascot.pose(SEED, ms(at)),
                "{} at {at}",
                mascot.name()
            );
        }
    }
}

#[test]
fn a_clock_that_starts_over_forgets_what_it_watched() {
    // `/clear` and a `/mascot` choice start the clock over so the banner
    // greets; a watch from before belongs to the old clock.
    let mut app = watching_app(Mascot::Crest, 50_000);
    type_str(&mut app, "hi");
    app.set_mascot_clock(ms(50_000));
    app.set_mascot_clock(ms(100));
    assert_eq!(
        app.banner_pose_watching(Gaze::Left),
        Mascot::Crest.pose(SEED, ms(100))
    );
}
