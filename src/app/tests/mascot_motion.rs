//! The banner mascot's motion: its stills, its schedule, and when the
//! session lets it move (`docs/mascot-motion.md`).

use super::*;
use crate::app::mascot_motion::{Beat, GREETING, Kind, dealt};

use unicode_width::UnicodeWidthStr;

/// Seeds the schedule tests run over — the unseeded default among them.
const SEEDS: [u64; 6] = [0, 1, 7, 42, 0xDEAD_BEEF, u64::MAX];

/// How many bars a schedule test walks.
const BARS: u32 = 60;

/// The art block's width in half cells: nine columns of two.
const HALF_COLS: usize = 18;

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

/// Display columns of `s` (the `ui::wrap::cols` measure, private to `ui`).
fn cols(s: &str) -> usize {
    s.width()
}

/// `mascot`'s resting art, as the owned rows a pose answers with.
fn rest_rows(mascot: Mascot) -> Vec<String> {
    mascot.art().iter().map(|row| (*row).to_string()).collect()
}

/// The glyph at (`row`, `col`) of `art`, a space past a short row's end.
fn glyph(art: &[String], row: usize, col: usize) -> char {
    art[row].chars().nth(col).unwrap_or(' ')
}

/// The cells where `a` and `b` differ, as (row, column).
fn changed_cells(a: &[String], b: &[String]) -> Vec<(usize, usize)> {
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

/// The cells a blink closes: the two eyes, left then right — or, for the
/// gem, its one facet.
fn eyes(mascot: Mascot) -> Vec<(usize, usize)> {
    match mascot {
        Mascot::Crest | Mascot::Bloom | Mascot::Twin | Mascot::Sprout => vec![(1, 2), (1, 6)],
        Mascot::Skiter => vec![(0, 2), (0, 6)],
        Mascot::Gem => vec![(1, 4)],
    }
}

/// The mascots with a pair of eyes — every one but the gem.
const TWO_EYED: [Mascot; 5] = [
    Mascot::Crest,
    Mascot::Bloom,
    Mascot::Sprout,
    Mascot::Twin,
    Mascot::Skiter,
];

/// How an eye that rests as `open` — a hole in the cell's upper half (`▄`)
/// or its lower half (`▀`) — looks shut, looking left, looking right, and
/// wide.
fn eye_states(open: char) -> [char; 4] {
    match open {
        '▄' => ['█', '▟', '▙', '▂'],
        '▀' => ['█', '▜', '▛', '▔'],
        other => panic!("{other:?} is not an eye"),
    }
}

/// The quadrants a block glyph fills: upper left, upper right, lower left,
/// lower right.
fn quadrants(glyph: char) -> [bool; 4] {
    match glyph {
        ' ' => [false, false, false, false],
        '▘' => [true, false, false, false],
        '▝' => [false, true, false, false],
        '▖' => [false, false, true, false],
        '▗' => [false, false, false, true],
        '▀' => [true, true, false, false],
        '▄' => [false, false, true, true],
        '▌' => [true, false, true, false],
        '▐' => [false, true, false, true],
        '▚' => [true, false, false, true],
        '▞' => [false, true, true, false],
        '▛' => [true, true, true, false],
        '▜' => [true, true, false, true],
        '▙' => [true, false, true, true],
        '▟' => [false, true, true, true],
        '█' => [true, true, true, true],
        other => panic!("{other:?} is not a quadrant glyph"),
    }
}

/// `art` as a grid of half cells — two rows and two columns to a cell —
/// over the whole nine-column block: the resolution a quadrant glyph draws
/// at, and the one a hop or a scuttle moves the mascot by.
fn half_cells(art: &[String]) -> Vec<Vec<bool>> {
    let mut grid = vec![vec![false; HALF_COLS]; art.len() * 2];
    for (row, glyphs) in art.iter().enumerate() {
        for (col, glyph) in glyphs.chars().enumerate() {
            let [ul, ur, ll, lr] = quadrants(glyph);
            grid[row * 2][col * 2] = ul;
            grid[row * 2][col * 2 + 1] = ur;
            grid[row * 2 + 1][col * 2] = ll;
            grid[row * 2 + 1][col * 2 + 1] = lr;
        }
    }
    grid
}

/// `grid` moved `down` half rows and `right` half columns. Ink pushed off
/// the block is dropped, and counted.
fn moved(grid: &[Vec<bool>], down: isize, right: isize) -> (Vec<Vec<bool>>, usize) {
    let mut out = vec![vec![false; HALF_COLS]; grid.len()];
    let mut lost = 0;
    for (y, row) in grid.iter().enumerate() {
        for (x, &inked) in row.iter().enumerate() {
            if !inked {
                continue;
            }
            let (to_y, to_x) = (y as isize + down, x as isize + right);
            if (0..grid.len() as isize).contains(&to_y) && (0..HALF_COLS as isize).contains(&to_x) {
                out[to_y as usize][to_x as usize] = true;
            } else {
                lost += 1;
            }
        }
    }
    (out, lost)
}

/// `art` in a mirror: its columns reversed across the block and every glyph
/// swapped for its mirror image.
fn mirrored(art: &[String], width: usize) -> Vec<String> {
    let flip = |glyph: char| match glyph {
        '▘' => '▝',
        '▝' => '▘',
        '▖' => '▗',
        '▗' => '▖',
        '▌' => '▐',
        '▐' => '▌',
        '▚' => '▞',
        '▞' => '▚',
        '▛' => '▜',
        '▜' => '▛',
        '▙' => '▟',
        '▟' => '▙',
        other => other,
    };
    art.iter()
        .map(|row| {
            let padded: Vec<char> = row
                .chars()
                .chain(std::iter::repeat(' '))
                .take(width)
                .collect();
            let turned: String = padded.into_iter().rev().map(flip).collect();
            turned.trim_end().to_string()
        })
        .collect()
}

/// Every gesture a mascot's acts call for — the greeting's, its moves' and
/// its looks', each sided one both ways.
fn repertoire(mascot: Mascot) -> Vec<Gesture> {
    let mut gestures: Vec<Gesture> = Vec::new();
    let acts = std::iter::once(GREETING)
        .chain(mascot.moves().iter().copied())
        .chain(mascot.looks().iter().copied());
    for act in acts {
        for beat in act {
            if let Beat::Do(gesture) = *beat {
                for gesture in [gesture, gesture.mirrored()] {
                    if !gestures.contains(&gesture) {
                        gestures.push(gesture);
                    }
                }
            }
        }
    }
    gestures
}

/// What a pose paints: its rows, and the glint and the glow on them.
type Painted = (Vec<String>, Option<i8>, u8);

/// An act as it plays: every pose it goes through and how long each is held.
type Played = Vec<(MascotPose, Duration)>;

/// Which act: its kind, its place in the mascot's list of that kind, and
/// whether it is played flipped.
type ActKey = (Kind, usize, bool);

/// What a pose paints. Two poses that paint the same are the same frame,
/// however they were put together.
fn painted(pose: &MascotPose) -> Painted {
    (pose.rows(), pose.glint, pose.glow)
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

/// `act` as `mascot` plays it — every pose and how long it is held, equal
/// neighbours folded together — `flipped` or as written.
fn played(mascot: Mascot, act: &[Beat], flipped: bool) -> Played {
    let rest = MascotPose::rest(mascot);
    let mut spans: Played = Vec::new();
    let mut push = |pose: MascotPose, hold: Duration| match spans.last_mut() {
        Some((last, held)) if *last == pose => *held += hold,
        _ => spans.push((pose, hold)),
    };
    for beat in act {
        match *beat {
            Beat::Rest(millis) => push(rest, ms(millis)),
            Beat::Do(gesture) => {
                let gesture = if flipped { gesture.mirrored() } else { gesture };
                for still in mascot.stills(gesture) {
                    push(still.pose, still.hold);
                }
            }
        }
    }
    spans
}

/// How long `act` takes `mascot`.
fn length(mascot: Mascot, act: &[Beat]) -> Duration {
    played(mascot, act, false)
        .iter()
        .map(|&(_, hold)| hold)
        .sum()
}

/// What bar `bar` plays, read back off the timeline: every pose from the
/// act's first to its last, and how long each is held.
fn heard(mascot: Mascot, seed: u64, bar: u32) -> Played {
    let rest = MascotPose::rest(mascot);
    let closes = MOTION_BAR * (bar + 1);
    let mut spans = Vec::new();
    let mut at = act_start(mascot, seed, bar);
    while at < closes {
        let motion = mascot_motion(mascot, seed, at);
        spans.push((motion.pose, motion.holds));
        at += motion.holds;
    }
    // The bar closes at rest, and that rest is not part of the act.
    assert_eq!(spans.pop().map(|(pose, _)| pose), Some(rest));
    spans
}

/// The acts of `kind` in `mascot`'s repertoire.
fn acts(mascot: Mascot, kind: Kind) -> &'static [&'static [Beat]] {
    match kind {
        Kind::Move => mascot.moves(),
        Kind::Look => mascot.looks(),
    }
}

/// Every act `mascot` knows, as it plays — what the timeline is read back
/// against. An act that reads the same both ways is listed once, as written.
struct Book {
    mascot: Mascot,
    entries: Vec<(ActKey, Played)>,
}

impl Book {
    fn of(mascot: Mascot) -> Self {
        let mut entries = Vec::new();
        for kind in [Kind::Move, Kind::Look] {
            for (index, act) in acts(mascot, kind).iter().enumerate() {
                let as_written = played(mascot, act, false);
                let flipped = played(mascot, act, true);
                if flipped != as_written {
                    entries.push(((kind, index, true), flipped));
                }
                entries.push(((kind, index, false), as_written));
            }
        }
        Self { mascot, entries }
    }

    /// The act bar `bar` plays.
    fn act_of(&self, seed: u64, bar: u32) -> ActKey {
        let heard = heard(self.mascot, seed, bar);
        self.entries
            .iter()
            .find(|(_, spans)| *spans == heard)
            .map(|&(key, _)| key)
            .unwrap_or_else(|| {
                panic!(
                    "{} seed {seed} bar {bar} plays nothing the mascot knows",
                    self.mascot.name()
                )
            })
    }

    /// Whether the act at `key` plays differently flipped.
    fn is_sided(&self, (kind, index, _): ActKey) -> bool {
        self.entries
            .iter()
            .any(|&(key, _)| key == (kind, index, true))
    }
}

/// Which kind of act bar `bar` plays and which turn of that kind it is:
/// after the greeting the bars alternate, a move first.
fn turn_of(bar: u32) -> (Kind, u64) {
    assert!(bar >= 1, "bar 0 is the greeting");
    if bar % 2 == 1 {
        (Kind::Move, u64::from(bar - 1) / 2)
    } else {
        (Kind::Look, u64::from(bar) / 2 - 1)
    }
}

// ===== the stills =====

#[test]
fn every_still_keeps_the_art_blocks_shape() {
    // A pose is drawn in the resting art's place, so it must fit there: the
    // same rows, never wider than the block, single-width block glyphs only
    // (a wide glyph shears the row — docs/table-streaming.md "Wide glyphs").
    for mascot in Mascot::ALL {
        let rest = mascot.art();
        for gesture in repertoire(mascot) {
            let stills = mascot.stills(gesture);
            assert!(!stills.is_empty(), "{} has no {gesture:?}", mascot.name());
            for still in &stills {
                assert!(still.hold > Duration::ZERO);
                let rows = still.pose.rows();
                assert_eq!(
                    rows.len(),
                    rest.len(),
                    "{} {gesture:?}: a still's row count",
                    mascot.name()
                );
                for row in &rows {
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
                    assert_eq!(
                        row.trim_end(),
                        row,
                        "{} {gesture:?}: {row:?} trails blanks the resting art never has",
                        mascot.name()
                    );
                }
            }
        }
    }
}

#[test]
fn the_resting_pose_is_the_catalogs_art() {
    for mascot in Mascot::ALL {
        let rest = MascotPose::rest(mascot);
        assert_eq!(rest.rows(), rest_rows(mascot));
        assert_eq!(rest.glint, None);
        assert_eq!(rest.glow, 0);
    }
}

#[test]
fn a_gesture_opens_and_ends_on_a_move() {
    // The timeline rests before a gesture and after it by itself; a still
    // that paints the resting art at either end would only pad the act.
    for mascot in Mascot::ALL {
        let rest = painted(&MascotPose::rest(mascot));
        for gesture in repertoire(mascot) {
            let stills = mascot.stills(gesture);
            for (end, still) in [("opens", &stills[0]), ("ends", &stills[stills.len() - 1])] {
                assert_ne!(
                    painted(&still.pose),
                    rest,
                    "{} {gesture:?} {end} at rest",
                    mascot.name()
                );
            }
        }
    }
}

#[test]
fn no_still_repeats_the_one_before_it() {
    // Two stills that paint the same cells are one still held longer.
    for mascot in Mascot::ALL {
        for gesture in repertoire(mascot) {
            let stills = mascot.stills(gesture);
            for pair in stills.windows(2) {
                assert_ne!(
                    painted(&pair[0].pose),
                    painted(&pair[1].pose),
                    "{} {gesture:?}",
                    mascot.name()
                );
            }
        }
    }
}

#[test]
fn a_mascot_only_makes_the_moves_its_body_has() {
    // The gem has one facet for an eye and no limbs; the twin has ears where
    // the others have arms; the skitterer's legs are its signature.
    for side in [Side::Left, Side::Right] {
        for gesture in [Gesture::Wink(side), Gesture::Wave(side)] {
            assert!(Mascot::Gem.stills(gesture).is_empty(), "gem {gesture:?}");
        }
        assert!(Mascot::Twin.stills(Gesture::Wave(side)).is_empty());
    }
    for gesture in [
        Gesture::Cross,
        Gesture::Doze,
        Gesture::Cheer,
        Gesture::Patter,
    ] {
        assert!(Mascot::Gem.stills(gesture).is_empty(), "gem {gesture:?}");
    }
    assert!(Mascot::Sprout.stills(Gesture::Cheer).is_empty());
    assert!(Mascot::Skiter.stills(Gesture::Patter).is_empty());
    // …and what a mascot cannot make, no act of its asks for.
    for mascot in Mascot::ALL {
        for gesture in repertoire(mascot) {
            assert!(
                !mascot.stills(gesture).is_empty(),
                "{}: an act asks for {gesture:?}",
                mascot.name()
            );
        }
    }
}

#[test]
fn every_mascot_has_the_moves_every_body_can_make() {
    // A signature and a flourish of its own, a hop, and the light.
    for mascot in Mascot::ALL {
        for gesture in [
            Gesture::Blink,
            Gesture::Glance(Side::Left),
            Gesture::Dart(Side::Left),
            Gesture::Widen,
            Gesture::Signature,
            Gesture::Flourish(Side::Left),
            Gesture::Hop,
            Gesture::Glow,
            Gesture::Glint(Side::Left),
        ] {
            assert!(
                repertoire(mascot).contains(&gesture),
                "{}: no act of its makes {gesture:?}",
                mascot.name()
            );
        }
    }
}

#[test]
fn a_blink_closes_every_eye_and_touches_nothing_else() {
    for mascot in Mascot::ALL {
        let rest = rest_rows(mascot);
        let stills = mascot.stills(Gesture::Blink);
        for still in &stills {
            assert_eq!(still.pose.glint, None);
            assert_eq!(
                changed_cells(&rest, &still.pose.rows()),
                eyes(mascot),
                "{}: a blink moves only the eyes",
                mascot.name()
            );
        }
        assert!(
            stills
                .iter()
                .any(|still| eyes(mascot).iter().all(|&(row, col)| glyph(
                    &still.pose.rows(),
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
            .map(|still| glyph(&still.pose.rows(), row, col))
            .collect();
        assert_eq!(seen, ['▆', '█', '▆'], "{}", mascot.name());
    }
}

#[test]
fn a_wink_closes_one_eye_and_leaves_the_other_open() {
    for mascot in TWO_EYED {
        let rest = rest_rows(mascot);
        for (side, eye) in [(Side::Left, 0), (Side::Right, 1)] {
            let cell = eyes(mascot)[eye];
            let stills = mascot.stills(Gesture::Wink(side));
            for still in &stills {
                assert_eq!(
                    changed_cells(&rest, &still.pose.rows()),
                    [cell],
                    "{} {side:?}: a wink moves one eye",
                    mascot.name()
                );
            }
            assert!(
                stills
                    .iter()
                    .any(|still| glyph(&still.pose.rows(), cell.0, cell.1) == '█'),
                "{} {side:?}: the eye never closes",
                mascot.name()
            );
            // A wink is meant, so it is held longer than a blink.
            let whole: Duration = stills.iter().map(|still| still.hold).sum();
            let blink: Duration = mascot
                .stills(Gesture::Blink)
                .iter()
                .map(|still| still.hold)
                .sum();
            assert!(whole > blink, "{} {side:?}: {whole:?}", mascot.name());
        }
    }
}

#[test]
fn a_glance_looks_one_way_then_the_other() {
    for mascot in Mascot::ALL {
        let rest = rest_rows(mascot);
        let resting = MascotPose::rest(mascot);
        let stills = mascot.stills(Gesture::Glance(Side::Left));
        let looks: Vec<&Still> = stills
            .iter()
            .filter(|still| still.pose != resting)
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
                changed_cells(&rest, &look.pose.rows())
                    .iter()
                    .all(|&(r, _)| r == row),
                "{}: a glance moves only the eye row",
                mascot.name()
            );
        }
        assert!(
            stills.iter().any(|still| still.pose == resting),
            "{}: the eyes pass through centre between the two looks",
            mascot.name()
        );
    }
}

#[test]
fn a_glance_to_the_left_looks_left_first() {
    // An eye is a hole, and it looks where the hole goes.
    for mascot in TWO_EYED {
        let rest = rest_rows(mascot);
        let first = mascot.stills(Gesture::Glance(Side::Left))[0].pose.rows();
        for (row, col) in eyes(mascot) {
            let [_, left, _, _] = eye_states(glyph(&rest, row, col));
            assert_eq!(glyph(&first, row, col), left, "{}", mascot.name());
        }
    }
    // The gem's facet slides a cell to the left.
    let first = Mascot::Gem.stills(Gesture::Glance(Side::Left))[0]
        .pose
        .rows();
    assert_eq!(glyph(&first, 1, 3), '▄');
    assert_eq!(glyph(&first, 1, 4), '█');
}

#[test]
fn darting_eyes_flick_from_side_to_side() {
    for mascot in Mascot::ALL {
        let rest = rest_rows(mascot);
        let stills = mascot.stills(Gesture::Dart(Side::Left));
        assert_eq!(stills.len(), 4, "{}", mascot.name());
        // Two looks, traded: left, right, left, right…
        assert_eq!(stills[0].pose, stills[2].pose, "{}", mascot.name());
        assert_eq!(stills[1].pose, stills[3].pose, "{}", mascot.name());
        assert_ne!(stills[0].pose, stills[1].pose, "{}", mascot.name());
        for (still, state) in stills.iter().zip([1, 2, 1, 2]) {
            assert!(
                still.hold >= ms(80) && still.hold <= ms(250),
                "{}: a dart is quick but seen",
                mascot.name()
            );
            for (row, col) in eyes(mascot) {
                let looks = eye_states(glyph(&rest, row, col));
                assert_eq!(
                    glyph(&still.pose.rows(), row, col),
                    looks[state],
                    "{}",
                    mascot.name()
                );
            }
            assert_eq!(
                changed_cells(&rest, &still.pose.rows()),
                eyes(mascot),
                "{}: only the eyes dart",
                mascot.name()
            );
        }
        // …or right first.
        let other_way = mascot.stills(Gesture::Dart(Side::Right));
        assert_eq!(other_way[0].pose, stills[1].pose, "{}", mascot.name());
        assert_eq!(other_way[1].pose, stills[0].pose, "{}", mascot.name());
    }
}

#[test]
fn crossed_eyes_look_at_each_other() {
    for mascot in TWO_EYED {
        let rest = rest_rows(mascot);
        let stills = mascot.stills(Gesture::Cross);
        assert_eq!(stills.len(), 1, "{}", mascot.name());
        let rows = stills[0].pose.rows();
        let cells = eyes(mascot);
        let [_, left, right, _] = eye_states(glyph(&rest, cells[0].0, cells[0].1));
        // The left eye looks right and the right eye looks left.
        assert_eq!(
            glyph(&rows, cells[0].0, cells[0].1),
            right,
            "{}",
            mascot.name()
        );
        assert_eq!(
            glyph(&rows, cells[1].0, cells[1].1),
            left,
            "{}",
            mascot.name()
        );
        assert_eq!(changed_cells(&rest, &rows), cells, "{}", mascot.name());
        assert!(
            stills[0].hold >= ms(400),
            "{}: held to be seen",
            mascot.name()
        );
    }
}

#[test]
fn wide_eyes_open_the_hole_further() {
    for mascot in Mascot::ALL {
        let rest = rest_rows(mascot);
        let stills = mascot.stills(Gesture::Widen);
        assert_eq!(stills.len(), 1, "{}", mascot.name());
        let rows = stills[0].pose.rows();
        for (row, col) in eyes(mascot) {
            let [_, _, _, wide] = eye_states(glyph(&rest, row, col));
            assert_eq!(glyph(&rows, row, col), wide, "{}", mascot.name());
        }
        assert_eq!(
            changed_cells(&rest, &rows),
            eyes(mascot),
            "{}",
            mascot.name()
        );
    }
}

#[test]
fn a_doze_nods_off_sleeps_and_starts_awake() {
    for mascot in TWO_EYED {
        let rest = rest_rows(mascot);
        let cells = eyes(mascot);
        let stills = mascot.stills(Gesture::Doze);
        let shut = |still: &Still| {
            cells
                .iter()
                .all(|&(row, col)| glyph(&still.pose.rows(), row, col) == '█')
        };
        // It sleeps for a good second…
        let asleep: Vec<&Still> = stills.iter().filter(|still| shut(still)).collect();
        assert_eq!(asleep.len(), 1, "{}: one sleep", mascot.name());
        assert!(asleep[0].hold >= ms(1_000), "{}", mascot.name());
        // …and wakes with a start: the last still is wide-eyed, and the body
        // is back exactly as it rests.
        let last = stills[stills.len() - 1].pose.rows();
        for &(row, col) in &cells {
            let [_, _, _, wide] = eye_states(glyph(&rest, row, col));
            assert_eq!(glyph(&last, row, col), wide, "{}", mascot.name());
        }
        assert_eq!(changed_cells(&rest, &last), cells, "{}", mascot.name());
        assert!(
            shut(&stills[stills.len() - 2]),
            "{}: woken from sleep",
            mascot.name()
        );
        let whole: Duration = stills.iter().map(|still| still.hold).sum();
        assert!(whole <= ms(2_000), "{}: {whole:?}", mascot.name());
    }
    // All but the skitterer sag as they go: the frill half down, the head to
    // one side, the leaf bent, an ear flat — before the eyes have closed.
    for mascot in [Mascot::Crest, Mascot::Bloom, Mascot::Sprout, Mascot::Twin] {
        let rest = rest_rows(mascot);
        let first = mascot.stills(Gesture::Doze)[0].pose.rows();
        assert!(
            changed_cells(&rest, &first)
                .iter()
                .any(|cell| !eyes(mascot).contains(cell)),
            "{}: nothing sags",
            mascot.name()
        );
        assert!(
            eyes(mascot)
                .iter()
                .any(|&(row, col)| glyph(&first, row, col) != '█'),
            "{}: it nods off before it sleeps",
            mascot.name()
        );
    }
}

#[test]
fn every_signature_is_the_mascots_own() {
    // Each mascot's signature is the move its description names, and no two
    // share one. Five move their art; the gem's is light, since what a cut
    // gem does is catch it.
    let mut seen: Vec<Vec<Painted>> = Vec::new();
    for mascot in Mascot::ALL {
        let rest = rest_rows(mascot);
        let stills = mascot.stills(Gesture::Signature);
        if mascot == Mascot::Gem {
            assert!(stills.iter().all(|still| still.pose.rows() == rest));
            assert!(stills.iter().any(|still| still.pose.glow > 0));
        } else {
            assert!(
                stills
                    .iter()
                    .all(|still| still.pose.glint.is_none() && still.pose.glow == 0),
                "{}",
                mascot.name()
            );
            assert!(
                stills.iter().any(|still| still.pose.rows() != rest),
                "{}: the signature moves nothing",
                mascot.name()
            );
            // …and not the eyes: that is what a blink and a glance are for.
            for still in &stills {
                assert!(
                    changed_cells(&rest, &still.pose.rows())
                        .iter()
                        .all(|cell| !eyes(mascot).contains(cell)),
                    "{}: the signature moves an eye",
                    mascot.name()
                );
            }
        }
        let drawn: Vec<_> = stills.iter().map(|still| painted(&still.pose)).collect();
        assert!(!seen.contains(&drawn), "{}: a borrowed move", mascot.name());
        seen.push(drawn);
    }
}

#[test]
fn the_gem_catches_the_light_twice() {
    // Its signature: two flashes to full strength, dark between them.
    let stills = Mascot::Gem.stills(Gesture::Signature);
    let peaks = stills.iter().filter(|still| still.pose.glow == 100).count();
    assert_eq!(peaks, 2);
    assert!(
        stills
            .iter()
            .any(|still| still.pose == MascotPose::rest(Mascot::Gem)),
        "dark between the two"
    );
    for still in &stills {
        assert_eq!(still.pose.glint, None);
        assert!(still.hold <= ms(150), "a flash is quick");
    }
}

#[test]
fn the_gem_spins_edge_on_and_back() {
    // Its flourish: the gem narrows to a sliver — its facet gone, edge-on —
    // and widens again, the same stills coming back as went.
    let stills = Mascot::Gem.stills(Gesture::Flourish(Side::Left));
    let widths: Vec<usize> = stills
        .iter()
        .map(|still| {
            let rows = still.pose.rows();
            let inked = |row: &String| row.chars().filter(|&glyph| glyph != ' ').count();
            rows.iter().map(inked).max().unwrap_or(0)
        })
        .collect();
    let narrowest = widths.iter().position(|&w| w == 1).expect("edge-on");
    assert_eq!(narrowest * 2 + 1, widths.len(), "{widths:?}");
    assert!(
        widths[..=narrowest]
            .windows(2)
            .all(|pair| pair[1] < pair[0])
    );
    let mut back = poses(&stills);
    back.reverse();
    assert_eq!(back, poses(&stills), "it comes back the way it went");
}

#[test]
fn every_flourish_is_a_second_move_of_the_mascots_own() {
    for mascot in Mascot::ALL {
        let rest = rest_rows(mascot);
        let stills = mascot.stills(Gesture::Flourish(Side::Left));
        assert!(
            stills.iter().any(|still| still.pose.rows() != rest),
            "{}",
            mascot.name()
        );
        // Not the signature over again: it strikes no pose the signature does.
        let signature = poses(&mascot.stills(Gesture::Signature));
        for still in &stills {
            assert!(
                still.pose == MascotPose::rest(mascot) || !signature.contains(&still.pose),
                "{}: the flourish borrows from the signature",
                mascot.name()
            );
            assert_eq!(still.pose.glint, None);
            assert_eq!(still.pose.glow, 0);
        }
    }
}

#[test]
fn the_crests_frill_ripples_one_spike_at_a_time() {
    let rest = rest_rows(Mascot::Crest);
    let stills = Mascot::Crest.stills(Gesture::Flourish(Side::Left));
    assert_eq!(stills.len(), 4, "four spikes");
    let mut dipped = Vec::new();
    for still in &stills {
        let changed = changed_cells(&rest, &still.pose.rows());
        assert_eq!(changed.len(), 1, "one spike down at a time");
        assert_eq!(changed[0].0, 0, "the frill is the top row");
        dipped.push(changed[0].1);
    }
    assert_eq!(dipped, [1, 3, 5, 7], "left to right");
}

#[test]
fn the_skitterer_scuttles_half_a_cell_each_way() {
    // The whole creature steps sideways — every half cell of it, legs and
    // eyes too — and back through where it rests.
    let rest = half_cells(&rest_rows(Mascot::Skiter));
    let stills = Mascot::Skiter.stills(Gesture::Flourish(Side::Left));
    let grids: Vec<Vec<Vec<bool>>> = stills
        .iter()
        .map(|still| half_cells(&still.pose.rows()))
        .collect();
    let (left, lost_left) = moved(&rest, 0, -1);
    let (right, lost_right) = moved(&rest, 0, 1);
    assert_eq!((lost_left, lost_right), (0, 0), "it has the room");
    assert_eq!(
        grids,
        [
            left.clone(),
            rest,
            right,
            half_cells(&rest_rows(Mascot::Skiter)),
            left
        ]
    );
}

#[test]
fn a_hop_moves_the_whole_mascot_half_a_cell() {
    // Every half cell of the mascot drops half a row and comes back: the
    // creature bobs, and nothing about it changes shape.
    for mascot in [
        Mascot::Crest,
        Mascot::Bloom,
        Mascot::Twin,
        Mascot::Skiter,
        Mascot::Gem,
    ] {
        let rest = half_cells(&rest_rows(mascot));
        let (down, lost) = moved(&rest, 1, 0);
        assert_eq!(lost, 0, "{}: there is room under it", mascot.name());
        for still in mascot.stills(Gesture::Hop) {
            let grid = half_cells(&still.pose.rows());
            assert!(
                grid == down || grid == rest,
                "{}: a hop is the resting art, lower",
                mascot.name()
            );
            assert_eq!(still.pose.glint, None);
        }
    }
    // The sprout stands on the block's floor, so it springs up instead, its
    // leaf's tip leaving the block for the moment.
    let rest = half_cells(&rest_rows(Mascot::Sprout));
    let (up, lost) = moved(&rest, -1, 0);
    assert_eq!(lost, 2, "only the leaf's tip is cut");
    let stills = Mascot::Sprout.stills(Gesture::Hop);
    assert_eq!(stills.len(), 1);
    assert_eq!(half_cells(&stills[0].pose.rows()), up);
}

#[test]
fn the_gem_floats_where_the_others_hop() {
    // A hop is over in a tenth of a second; the gem hangs in the air.
    let quick: Duration = Mascot::Crest
        .stills(Gesture::Hop)
        .iter()
        .map(|still| still.hold)
        .sum();
    let slow: Duration = Mascot::Gem
        .stills(Gesture::Hop)
        .iter()
        .map(|still| still.hold)
        .sum();
    assert!(quick <= ms(150), "{quick:?}");
    assert!(slow >= ms(600), "{slow:?}");
}

#[test]
fn a_wave_moves_an_arm_and_leaves_the_face_alone() {
    for mascot in [Mascot::Crest, Mascot::Bloom, Mascot::Sprout, Mascot::Skiter] {
        let rest = rest_rows(mascot);
        for side in [Side::Left, Side::Right] {
            let stills = mascot.stills(Gesture::Wave(side));
            assert!(
                stills.len() >= 3,
                "{}: a wave goes back and forth",
                mascot.name()
            );
            for still in &stills {
                assert!(
                    changed_cells(&rest, &still.pose.rows())
                        .iter()
                        .all(|cell| !eyes(mascot).contains(cell)),
                    "{} {side:?}: a wave moves an eye",
                    mascot.name()
                );
            }
        }
    }
    // The crest and the sprout wave one arm, on the side asked for.
    for mascot in [Mascot::Crest, Mascot::Sprout] {
        let rest = rest_rows(mascot);
        for (side, half) in [(Side::Left, 0..4), (Side::Right, 5..9)] {
            for still in mascot.stills(Gesture::Wave(side)) {
                assert!(
                    changed_cells(&rest, &still.pose.rows())
                        .iter()
                        .all(|&(_, col)| half.contains(&col)),
                    "{} {side:?}: the other arm moves",
                    mascot.name()
                );
            }
        }
    }
}

#[test]
fn a_cheer_is_a_hop_with_the_arms_thrown_up() {
    for mascot in [Mascot::Crest, Mascot::Bloom, Mascot::Twin, Mascot::Skiter] {
        let rest = MascotPose::rest(mascot);
        let hop = mascot.stills(Gesture::Hop)[0].pose;
        let stills = mascot.stills(Gesture::Cheer);
        // Down, up with the arms out, down, up again.
        assert_eq!(stills[0].pose, hop, "{}: it crouches first", mascot.name());
        assert_eq!(stills[2].pose, hop, "{}", mascot.name());
        assert_eq!(stills[1].pose, stills[3].pose, "{}", mascot.name());
        assert_ne!(stills[1].pose, rest, "{}: the arms go up", mascot.name());
        assert_ne!(stills[1].pose, hop, "{}", mascot.name());
        assert!(
            stills[1].hold > stills[0].hold,
            "{}: it hangs at the top",
            mascot.name()
        );
    }
}

#[test]
fn pattering_feet_trade_places() {
    for mascot in [Mascot::Crest, Mascot::Bloom, Mascot::Sprout, Mascot::Twin] {
        let rest_art = rest_rows(mascot);
        let feet = rest_art.len() - 1;
        let stills = mascot.stills(Gesture::Patter);
        assert!(stills.len() >= 4, "{}: several steps", mascot.name());
        for (index, still) in stills.iter().enumerate() {
            assert_eq!(
                still.pose,
                stills[index % 2].pose,
                "{}: two steps, traded",
                mascot.name()
            );
            assert!(
                changed_cells(&rest_art, &still.pose.rows())
                    .iter()
                    .all(|&(row, _)| row == feet),
                "{}: only the feet patter",
                mascot.name()
            );
        }
        // Each step lifts half the feet: between them the two steps hold
        // every foot once, and neither lifts ink that is not a foot.
        let rest = half_cells(&rest_art);
        let one = half_cells(&stills[0].pose.rows());
        let other = half_cells(&stills[1].pose.rows());
        let mut lifted = (0, 0);
        for y in 0..rest.len() {
            for x in 0..HALF_COLS {
                assert!(
                    !(one[y][x] || other[y][x]) || rest[y][x],
                    "{}: a step adds ink at {y},{x}",
                    mascot.name()
                );
                assert!(
                    !rest[y][x] || one[y][x] || other[y][x],
                    "{}: both steps lift {y},{x}",
                    mascot.name()
                );
                lifted.0 += usize::from(rest[y][x] && !one[y][x]);
                lifted.1 += usize::from(rest[y][x] && !other[y][x]);
            }
        }
        assert_eq!(lifted, (2, 2), "{}: two feet a step", mascot.name());
    }
}

#[test]
fn a_sided_gesture_the_other_way_is_its_mirror_image() {
    // Every mascot is the same either side of its middle, so "left" and
    // "right" are one move seen in a mirror: the same stills, turned.
    for mascot in Mascot::ALL {
        let width = mascot.art_width();
        for gesture in repertoire(mascot) {
            // A glint is light, not art, and has a rule of its own.
            if gesture.mirrored() == gesture || matches!(gesture, Gesture::Glint(_)) {
                continue;
            }
            let turned: Vec<(Vec<String>, Duration)> = mascot
                .stills(gesture)
                .iter()
                .map(|still| (mirrored(&still.pose.rows(), width), still.hold))
                .collect();
            let other: Vec<(Vec<String>, Duration)> = mascot
                .stills(gesture.mirrored())
                .iter()
                .map(|still| (still.pose.rows(), still.hold))
                .collect();
            assert_eq!(other, turned, "{} {gesture:?}", mascot.name());
        }
    }
}

#[test]
fn every_mascot_rests_the_same_in_a_mirror() {
    // What the rule above stands on.
    for mascot in Mascot::ALL {
        let rest = rest_rows(mascot);
        assert_eq!(
            mirrored(&rest, mascot.art_width()),
            rest,
            "{}",
            mascot.name()
        );
    }
}

#[test]
fn only_the_sided_gestures_change_in_a_mirror() {
    for gesture in [
        Gesture::Blink,
        Gesture::Cross,
        Gesture::Widen,
        Gesture::Doze,
        Gesture::Signature,
        Gesture::Hop,
        Gesture::Cheer,
        Gesture::Patter,
        Gesture::Glow,
    ] {
        assert_eq!(gesture.mirrored(), gesture);
    }
    for (left, right) in [
        (Gesture::Wink(Side::Left), Gesture::Wink(Side::Right)),
        (Gesture::Glance(Side::Left), Gesture::Glance(Side::Right)),
        (Gesture::Dart(Side::Left), Gesture::Dart(Side::Right)),
        (
            Gesture::Flourish(Side::Left),
            Gesture::Flourish(Side::Right),
        ),
        (Gesture::Wave(Side::Left), Gesture::Wave(Side::Right)),
        (Gesture::Glint(Side::Left), Gesture::Glint(Side::Right)),
    ] {
        assert_eq!(left.mirrored(), right);
        assert_eq!(right.mirrored(), left);
    }
}

/// Whether `pose`'s light falls on any glyph of its art.
fn lights_something(pose: &MascotPose) -> bool {
    (0..pose.rows().len()).any(|row| (0..9).any(|col| pose.light(col, row) > 0.0))
}

/// `mascot` at rest with a glint centred on `centre` — the still of its
/// sweep that sits there.
fn glint_at(mascot: Mascot, centre: i8) -> MascotPose {
    mascot
        .stills(Gesture::Glint(Side::Left))
        .iter()
        .map(|still| still.pose)
        .find(|pose| pose.glint == Some(centre))
        .expect("the sweep passes there")
}

#[test]
fn a_glint_is_a_leaning_band_three_cells_wide() {
    // The skitterer's upper rows are solid, so every cell under the band
    // shows it.
    let pose = glint_at(Mascot::Skiter, 4);
    let rows = pose.rows();
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
    assert_eq!(glyph(&rows, 2, 4), ' ');
    assert_eq!(pose.light(4, 2), 0.0);
    // …nor is anything past the art, or with no light on the mascot.
    assert_eq!(pose.light(9, 1), 0.0);
    assert_eq!(pose.light(4, 3), 0.0);
    assert_eq!(MascotPose::rest(Mascot::Skiter).light(4, 1), 0.0);
}

#[test]
fn a_glint_crosses_the_resting_art_from_edge_to_edge() {
    for mascot in Mascot::ALL {
        let stills = mascot.stills(Gesture::Glint(Side::Left));
        let centres: Vec<i8> = stills
            .iter()
            .map(|still| {
                assert_eq!(
                    still.pose.rows(),
                    rest_rows(mascot),
                    "a glint moves no glyph"
                );
                assert_eq!(still.pose.glow, 0);
                assert!(
                    lights_something(&still.pose),
                    "{}: a still that lights nothing is a frame drawn for nothing",
                    mascot.name()
                );
                still.pose.glint.expect("a glint still carries its centre")
            })
            .collect();
        // One column a still, never stalling, from the first position that
        // lights a glyph to the last: it enters at the left edge and leaves
        // at the right.
        assert!(
            centres.windows(2).all(|pair| pair[1] == pair[0] + 1),
            "{centres:?}"
        );
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
fn a_glint_from_the_right_is_the_same_sweep_backwards() {
    for mascot in Mascot::ALL {
        let mut there = poses(&mascot.stills(Gesture::Glint(Side::Left)));
        let back = poses(&mascot.stills(Gesture::Glint(Side::Right)));
        there.reverse();
        assert_eq!(back, there, "{}", mascot.name());
    }
}

#[test]
fn a_glow_swells_and_fades_over_the_whole_mascot() {
    for mascot in Mascot::ALL {
        let stills = mascot.stills(Gesture::Glow);
        let levels: Vec<u8> = stills
            .iter()
            .map(|still| {
                assert_eq!(
                    still.pose.rows(),
                    rest_rows(mascot),
                    "a glow moves no glyph"
                );
                assert_eq!(still.pose.glint, None);
                still.pose.glow
            })
            .collect();
        // Up to one peak and down again, the same way it came.
        let peak = levels.len() / 2;
        assert!(
            levels[..=peak].windows(2).all(|pair| pair[1] > pair[0]),
            "{levels:?}"
        );
        let mut fading = levels.clone();
        fading.reverse();
        assert_eq!(fading, levels, "{levels:?}");
        assert!(levels[0] > 0 && levels[peak] <= 100, "{levels:?}");
        // Every glyph is lit alike, and no blank cell is lit at all.
        let pose = stills[peak].pose;
        let rows = pose.rows();
        for (row, glyphs) in rows.iter().enumerate() {
            for col in 0..9 {
                let inked = glyphs.chars().nth(col).is_some_and(|glyph| glyph != ' ');
                let expected = if inked {
                    f32::from(levels[peak]) / 100.0
                } else {
                    0.0
                };
                assert_eq!(
                    pose.light(col, row),
                    expected,
                    "{} {row},{col}",
                    mascot.name()
                );
            }
        }
        let whole: Duration = stills.iter().map(|still| still.hold).sum();
        assert!(
            whole >= ms(300) && whole <= ms(700),
            "{}: the glow takes {whole:?}",
            mascot.name()
        );
    }
}

// ===== the acts =====

#[test]
fn every_mascot_has_a_handful_of_moves_and_of_looks() {
    for mascot in Mascot::ALL {
        for (kind, least, acts) in [("moves", 5, mascot.moves()), ("looks", 4, mascot.looks())] {
            assert!(
                acts.len() >= least,
                "{}: only {} {kind}",
                mascot.name(),
                acts.len()
            );
            for (index, act) in acts.iter().enumerate() {
                assert!(
                    matches!(act.first(), Some(Beat::Do(_)))
                        && matches!(act.last(), Some(Beat::Do(_))),
                    "{} {kind} {index}: an act opens and ends on a gesture",
                    mascot.name()
                );
            }
        }
    }
}

#[test]
fn only_one_look_is_a_bare_blink() {
    // The rest of a mascot's blinking closes some other act.
    for mascot in Mascot::ALL {
        let bare = mascot
            .looks()
            .iter()
            .filter(|act| {
                act.iter()
                    .all(|beat| matches!(beat, Beat::Rest(_) | Beat::Do(Gesture::Blink)))
            })
            .count();
        assert_eq!(bare, 1, "{}", mascot.name());
    }
}

#[test]
fn a_mascot_blinks_twice_as_it_wakes() {
    for mascot in TWO_EYED {
        let doze = mascot
            .looks()
            .iter()
            .find(|act| act.contains(&Beat::Do(Gesture::Doze)))
            .expect("it dozes");
        let after: Vec<&Beat> = doze
            .iter()
            .skip_while(|beat| **beat != Beat::Do(Gesture::Doze))
            .skip(1)
            .filter(|beat| matches!(beat, Beat::Do(_)))
            .collect();
        assert_eq!(
            after,
            [&Beat::Do(Gesture::Blink), &Beat::Do(Gesture::Blink)]
        );
    }
}

#[test]
fn no_two_acts_of_a_mascot_play_alike() {
    // The schedule's variety is only as good as its list: an act that plays
    // the same as another is one act listed twice.
    for mascot in Mascot::ALL {
        let mut seen: Vec<Played> = Vec::new();
        for act in mascot.moves().iter().chain(mascot.looks()) {
            for flipped in [false, true] {
                let spans = played(mascot, act, flipped);
                if flipped && spans == played(mascot, act, false) {
                    continue;
                }
                assert!(!seen.contains(&spans), "{}: {act:?}", mascot.name());
                seen.push(spans);
            }
        }
    }
}

#[test]
fn a_look_moves_the_eyes_and_a_move_moves_more() {
    // What the schedule alternates between: a look is the eyes (the gem's
    // facet) and nothing else; a move is the body or the light.
    let is_look = |gesture: Gesture| {
        matches!(
            gesture,
            Gesture::Blink
                | Gesture::Wink(_)
                | Gesture::Glance(_)
                | Gesture::Dart(_)
                | Gesture::Cross
                | Gesture::Widen
                | Gesture::Doze
        )
    };
    for mascot in Mascot::ALL {
        for act in mascot.looks() {
            for beat in *act {
                if let Beat::Do(gesture) = *beat {
                    assert!(
                        is_look(gesture),
                        "{}: {gesture:?} among the looks",
                        mascot.name()
                    );
                }
            }
        }
        for act in mascot.moves() {
            assert!(
                act.iter()
                    .any(|beat| matches!(*beat, Beat::Do(gesture) if !is_look(gesture))),
                "{}: {act:?} among the moves",
                mascot.name()
            );
        }
    }
}

#[test]
fn every_act_fits_its_bar() {
    // An act starts no earlier than `MOTION_ACT_EARLIEST` into its bar and
    // the bar's last 900 ms are at rest, so that is all the room it has.
    let room = MOTION_BAR - ms(900) - MOTION_ACT_EARLIEST;
    for mascot in Mascot::ALL {
        for act in mascot.moves().iter().chain(mascot.looks()) {
            assert!(
                length(mascot, act) <= room,
                "{}: {act:?} takes {:?}",
                mascot.name(),
                length(mascot, act)
            );
        }
        assert!(
            length(mascot, GREETING) <= MOTION_BAR - ms(900) - MOTION_GREETING_DELAY,
            "{}: the greeting takes {:?}",
            mascot.name(),
            length(mascot, GREETING)
        );
    }
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
        for gesture in [
            Gesture::Glint(Side::Left),
            Gesture::Signature,
            Gesture::Blink,
        ] {
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
    for mascot in Mascot::ALL {
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
    // An act every N seconds on the dot reads as a machine. The offsets
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
fn a_long_act_starts_early_enough_to_finish() {
    // The start is scattered over what the act leaves of its bar: a doze
    // cannot begin as late as a blink can, and still it never begins on the
    // dot.
    let book = Book::of(Mascot::Crest);
    let doze = Mascot::Crest
        .looks()
        .iter()
        .position(|act| act.contains(&Beat::Do(Gesture::Doze)))
        .expect("the crest dozes");
    let room = MOTION_BAR - ms(900) - length(Mascot::Crest, Mascot::Crest.looks()[doze]);
    assert!(room < MOTION_ACT_LATEST, "the doze is the long one");
    let mut offsets = std::collections::BTreeSet::new();
    for seed in 0..40 {
        for bar in (2..BARS).step_by(2) {
            let (_, turn) = turn_of(bar);
            if dealt(seed, Kind::Look, turn, Mascot::Crest.looks().len()) != doze {
                continue;
            }
            assert_eq!(book.act_of(seed, bar), (Kind::Look, doze, false));
            offsets.insert(act_start(Mascot::Crest, seed, bar) - MOTION_BAR * bar);
        }
    }
    assert!(offsets.len() > 20, "{} starts", offsets.len());
    assert!(offsets.iter().all(|&into| into <= room), "{offsets:?}");
    assert!(
        offsets.iter().any(|&into| into > room - ms(200)),
        "it uses the room it has"
    );
}

#[test]
fn a_hold_runs_exactly_to_the_next_pose_that_differs() {
    // The boundary sleeps for `holds` and draws again, so every wake must
    // have something new to draw — and nothing may change before it.
    for mascot in Mascot::ALL {
        for seed in SEEDS {
            let visited = walk(mascot, seed, Duration::ZERO, MOTION_BAR * 40);
            for pair in visited.windows(2) {
                let ((at, pose), (next_at, next)) = (pair[0], pair[1]);
                assert_ne!(
                    painted(&pose),
                    painted(&next),
                    "{} seed {seed}: woke at {next_at:?} to what was already drawn",
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
fn a_deal_plays_every_act_once_a_cycle() {
    // The acts of a kind are dealt like a shuffled deck: all of them go by
    // before any comes round again.
    for kind in [Kind::Move, Kind::Look] {
        for count in 3..=9usize {
            for seed in 0..50 {
                for cycle in 0..12 {
                    let mut hand: Vec<usize> = (0..count)
                        .map(|slot| dealt(seed, kind, (cycle * count + slot) as u64, count))
                        .collect();
                    hand.sort_unstable();
                    assert_eq!(
                        hand,
                        (0..count).collect::<Vec<_>>(),
                        "seed {seed} {kind:?} cycle {cycle} of {count}"
                    );
                }
            }
        }
    }
}

#[test]
fn no_act_is_dealt_twice_in_a_row() {
    // Not within a cycle — every act is dealt once — and not across the join
    // between two, where the last of one deal could meet itself.
    for kind in [Kind::Move, Kind::Look] {
        for count in 3..=9usize {
            for seed in 0..300 {
                let turns: Vec<usize> = (0..count as u64 * 12)
                    .map(|turn| dealt(seed, kind, turn, count))
                    .collect();
                assert!(
                    turns.windows(2).all(|pair| pair[0] != pair[1]),
                    "seed {seed} {kind:?} of {count}: {turns:?}"
                );
            }
        }
    }
}

#[test]
fn each_cycle_is_dealt_afresh() {
    // A fixed order would be a loop the eye learns. Each cycle's order is
    // its own, another seed's is another, and the moves are not dealt in the
    // looks' order.
    let count = 8;
    let hand = |seed, kind, cycle: u64| -> Vec<usize> {
        (0..count as u64)
            .map(|slot| dealt(seed, kind, cycle * count as u64 + slot, count))
            .collect()
    };
    let cycles: std::collections::BTreeSet<Vec<usize>> =
        (0..40).map(|cycle| hand(7, Kind::Move, cycle)).collect();
    assert!(cycles.len() >= 38, "{} orders in 40 cycles", cycles.len());
    let seeds: std::collections::BTreeSet<Vec<usize>> =
        (0..40).map(|seed| hand(seed, Kind::Move, 0)).collect();
    assert!(seeds.len() >= 38, "{} orders over 40 seeds", seeds.len());
    assert_ne!(hand(7, Kind::Move, 0), hand(7, Kind::Look, 0));
    assert_eq!(
        hand(7, Kind::Move, 3),
        hand(7, Kind::Move, 3),
        "one seed, one order"
    );
    // Every act leads a cycle sooner or later: the deal is not lopsided.
    let leaders: std::collections::BTreeSet<usize> = (0..200)
        .map(|seed| dealt(seed, Kind::Move, count as u64, count))
        .collect();
    assert_eq!(leaders.len(), count);
}

#[test]
fn the_first_deal_does_not_open_on_what_the_greeting_showed() {
    // The greeting has just crossed the mascot with a glint, made the
    // signature and closed on a blink. The signature leads the moves and the
    // glint closes them, the bare blink leads the looks — and the first
    // cycle opens on none of the three. Any other act can come first.
    for mascot in Mascot::ALL {
        let moves = mascot.moves();
        assert_eq!(moves[0], [Beat::Do(Gesture::Signature)]);
        assert_eq!(
            moves[moves.len() - 1],
            [Beat::Do(Gesture::Glint(Side::Left))]
        );
        assert_eq!(mascot.looks()[0], [Beat::Do(Gesture::Blink)]);
    }
    for count in 3..=9usize {
        let firsts = |kind| -> std::collections::BTreeSet<usize> {
            (0..600).map(|seed| dealt(seed, kind, 0, count)).collect()
        };
        assert_eq!(
            firsts(Kind::Move),
            (1..count - 1).collect(),
            "moves of {count}: any but the first and the last can open"
        );
        assert_eq!(
            firsts(Kind::Look),
            (1..count).collect(),
            "looks of {count}: any but the first can open"
        );
    }
}

#[test]
fn the_first_idle_act_is_new_to_the_banner() {
    // …and so on the timeline: whatever a mascot does first after its
    // greeting, it is not the glint or the signature over again.
    for mascot in Mascot::ALL {
        let book = Book::of(mascot);
        let moves = mascot.moves();
        for seed in 0..60 {
            let (kind, index, _) = book.act_of(seed, 1);
            assert_eq!(kind, Kind::Move, "{} seed {seed}", mascot.name());
            assert!(
                index != 0 && index != moves.len() - 1,
                "{} seed {seed}: {:?} straight after the greeting",
                mascot.name(),
                moves[index]
            );
            let (kind, index, _) = book.act_of(seed, 2);
            assert_eq!(kind, Kind::Look, "{} seed {seed}", mascot.name());
            assert_ne!(index, 0, "{} seed {seed}: a bare blink", mascot.name());
        }
    }
}

#[test]
fn moves_and_looks_take_turns_and_play_what_is_dealt() {
    // After the greeting the mascot makes a move, then gives a look, then a
    // move again — so no stretch of an idle banner is all eyes — and each is
    // the next one off its deck.
    for mascot in Mascot::ALL {
        let book = Book::of(mascot);
        for seed in SEEDS {
            for bar in 1..40 {
                let (kind, turn) = turn_of(bar);
                let expected = dealt(seed, kind, turn, acts(mascot, kind).len());
                let (played_kind, index, _) = book.act_of(seed, bar);
                assert_eq!(
                    (played_kind, index),
                    (kind, expected),
                    "{} seed {seed} bar {bar}",
                    mascot.name()
                );
            }
        }
    }
}

#[test]
fn a_sided_act_plays_both_ways() {
    // Which eye winks, which way it glances first, which arm waves: a coin
    // flipped for every bar.
    for mascot in Mascot::ALL {
        let book = Book::of(mascot);
        let (mut as_written, mut flipped) = (0, 0);
        for bar in 1..200 {
            let key = book.act_of(5, bar);
            if !book.is_sided(key) {
                continue;
            }
            if key.2 {
                flipped += 1;
            } else {
                as_written += 1;
            }
        }
        assert!(
            as_written >= 10 && flipped >= 10,
            "{}: {as_written} as written, {flipped} flipped",
            mascot.name()
        );
    }
}

#[test]
fn a_bare_blink_is_a_small_share_of_what_a_mascot_does() {
    // A face at rest blinks, but a mascot that only ever blinks reads as a
    // loop. A blink is mostly punctuation — the end of a cheer, of a doze —
    // and over a long idle a bar that is nothing but blinking is at most one
    // in eight, while the mascot is seen doing at least nine different
    // things.
    for mascot in Mascot::ALL {
        let book = Book::of(mascot);
        let bars = 200;
        let mut blinks = 0;
        let mut seen = std::collections::BTreeSet::new();
        for bar in 1..=bars {
            let (kind, index, _) = book.act_of(3, bar);
            let only_blinks = acts(mascot, kind)[index]
                .iter()
                .all(|beat| matches!(beat, Beat::Rest(_) | Beat::Do(Gesture::Blink)));
            blinks += u32::from(only_blinks);
            seen.insert((kind == Kind::Move, index));
        }
        assert!(
            blinks * 8 <= bars,
            "{}: {blinks} of {bars} bars only blink",
            mascot.name()
        );
        assert!(
            seen.len() >= 9,
            "{}: {} different acts",
            mascot.name(),
            seen.len()
        );
    }
}

#[test]
fn half_a_minute_of_idling_shows_six_different_things() {
    // What the banner is for: someone who watches it for thirty seconds
    // after the greeting sees six acts, and no two of them are the same.
    for mascot in Mascot::ALL {
        let book = Book::of(mascot);
        for seed in 0..40 {
            let seen: std::collections::BTreeSet<(bool, usize)> = (1..=6)
                .map(|bar| {
                    let (kind, index, _) = book.act_of(seed, bar);
                    (kind == Kind::Move, index)
                })
                .collect();
            assert_eq!(seen.len(), 6, "{} seed {seed}", mascot.name());
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
fn the_timeline_runs_for_as_long_as_the_session_idles() {
    // A banner left up for a day still answers at once and still rests.
    let day = Duration::from_secs(24 * 60 * 60);
    for mascot in Mascot::ALL {
        let motion = mascot_motion(mascot, 9, day);
        assert!(motion.holds > Duration::ZERO);
        assert!(motion.holds <= MOTION_BAR * 2);
        assert_eq!(motion.pose.rows().len(), mascot.art().len());
    }
    // …and at the end of time it does not overflow.
    let motion = mascot_motion(Mascot::Crest, u64::MAX, Duration::MAX);
    assert!(motion.holds > Duration::ZERO);
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
        Mascot::Crest.stills(Gesture::Glint(Side::Left))[0].pose,
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
        Mascot::Crest.stills(Gesture::Glint(Side::Left))[0].pose
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
