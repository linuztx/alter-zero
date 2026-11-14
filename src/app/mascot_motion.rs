//! The banner mascot's **motion**: the stills each mascot can strike, the
//! schedule that strings them into a blink here and a glance there, and the
//! session state that decides whether the banner moves at all. See
//! `docs/mascot-motion.md`.
//!
//! Everything is a pure function of time. [`mascot_motion`] takes how long
//! the banner has been up and answers with the pose to draw and how long it
//! holds — no state advances, so the boundary can sleep for exactly that
//! hold (an idle banner costs one frame per pose change, nothing between
//! acts) and the `/mascot` picker can run a second clock through the same
//! function for its preview.
//!
//! A pose is drawn **in the resting art's place**: every still keeps the art
//! block's shape (the rest's row count, never wider than its widest row,
//! single-width block glyphs), which is why the metadata column beside the
//! mascot never moves. Eyes are holes — half a cell of background punched in
//! a solid face — so an eye's states are the block glyphs that reshape the
//! hole: `▄` open, `▆` half-lidded, `█` shut, `▟`/`▙` looking left/right.

use super::*;

/// One mascot's art block: one `&str` per row, drawn verbatim.
type Art = &'static [&'static str];

/// One bar of the schedule, in milliseconds — see [`MOTION_BAR`].
const BAR_MS: u64 = 4_800;

/// How far into bar 0 the greeting starts, in milliseconds.
const GREETING_MS: u64 = 700;

/// The earliest and latest an ordinary bar's act starts, in milliseconds.
const ACT_EARLIEST_MS: u64 = 900;
const ACT_LATEST_MS: u64 = 2_700;

/// The schedule's unit: each bar plays one act — a gesture, or a short run
/// of them — starting somewhere in its middle and over well before it ends,
/// so two acts never run into each other and never keep time.
pub const MOTION_BAR: Duration = Duration::from_millis(BAR_MS);

/// How long a banner that has just appeared rests before its mascot greets:
/// long enough to be seen at rest first, short enough to read as a reaction
/// to the launch.
pub const MOTION_GREETING_DELAY: Duration = Duration::from_millis(GREETING_MS);

/// The earliest an ordinary bar's act starts, measured from the bar's start.
pub const MOTION_ACT_EARLIEST: Duration = Duration::from_millis(ACT_EARLIEST_MS);

/// The latest an ordinary bar's act starts — with [`MOTION_ACT_EARLIEST`],
/// what spaces consecutive acts 3.0–6.6 s apart.
pub const MOTION_ACT_LATEST: Duration = Duration::from_millis(ACT_LATEST_MS);

/// How far either side of its centre a glint lights a glyph on one row, in
/// display columns: fully under the centre, half as much a column off, not
/// at all two off — a band three cells wide.
const GLINT_HALF_WIDTH: f32 = 2.0;

/// How long each still of a glint holds, in milliseconds — one column a
/// still, so the band crosses the nine-column block in about half a second.
const GLINT_STEP_MS: u64 = 40;

/// The moves a mascot makes. Every mascot has all four, drawn in its own
/// anatomy ([`Mascot::stills`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gesture {
    /// The eyes close and open — the gem's centre facet, which is its eye.
    Blink,
    /// The eyes look left, then right — the gem's facet sliding a cell each
    /// way, the gem turning in the light.
    Glance,
    /// The move the mascot's description names: the crest flares its frill,
    /// the bloom swells, the sprout's leaf sways, the twin flicks its ears,
    /// the skitterer skitters, the gem catches the light again.
    Signature,
    /// A slanted band of light crosses the resting art: colour only.
    Glint,
}

impl Gesture {
    /// Every gesture.
    pub const ALL: [Self; 4] = [Self::Blink, Self::Glance, Self::Signature, Self::Glint];
}

/// What a mascot looks like at one instant: the art to draw in the resting
/// art's place, and where a glint sits on it if one is passing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MascotPose {
    /// The art rows — the resting art's row count, none wider than its
    /// widest row.
    pub art: &'static [&'static str],
    /// The centre of a passing glint, in display columns across the art
    /// block (off either edge while it enters or leaves); `None` when no
    /// light is on the mascot.
    pub glint: Option<i8>,
}

impl MascotPose {
    /// `mascot` at rest: its catalog art, no glint — what the banner is
    /// committed as, and what every act returns to.
    #[must_use]
    pub const fn rest(mascot: Mascot) -> Self {
        Self {
            art: mascot.art(),
            glint: None,
        }
    }

    /// How strongly the passing glint lights the glyph at display column
    /// `col` of art row `row`: 0 (not at all) to 1 (under the band's crest).
    /// Zero with no glint, and for a blank cell — light shows on ink.
    ///
    /// The band is three cells wide on a row and **leans a column a row**,
    /// like a `/`: further right above the block's middle row, further left
    /// below it. That lean is what makes it read as light glancing off the
    /// mascot rather than a bar wiping across it. How a lit glyph is
    /// coloured is the renderer's (`ui::header`).
    #[must_use]
    pub fn light(&self, col: usize, row: usize) -> f32 {
        let Some(centre) = self.glint else {
            return 0.0;
        };
        // The art is single-width, so a row's nth char is its nth column.
        let inked = self
            .art
            .get(row)
            .and_then(|glyphs| glyphs.chars().nth(col))
            .is_some_and(|glyph| glyph != ' ');
        if !inked {
            return 0.0;
        }
        let lean = row as f32 - self.art.len().saturating_sub(1) as f32 / 2.0;
        let distance = (col as f32 + lean - f32::from(centre)).abs();
        (1.0 - distance / GLINT_HALF_WIDTH).max(0.0)
    }

    /// Whether the glint lights any glyph of the art at all.
    fn is_lit(&self) -> bool {
        self.art
            .iter()
            .enumerate()
            .any(|(row, glyphs)| (0..glyphs.chars().count()).any(|col| self.light(col, row) > 0.0))
    }
}

/// One still of a gesture: a pose and how long it is held.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Still {
    /// What is drawn.
    pub pose: MascotPose,
    /// For how long.
    pub hold: Duration,
}

/// Where a mascot's motion stands at one moment ([`mascot_motion`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MascotMotion {
    /// The pose to draw now.
    pub pose: MascotPose,
    /// How long until the pose next **differs** — never zero. A redraw
    /// before then would paint the same cells.
    pub holds: Duration,
}

/// One drawn still of an art gesture: the art and its hold in milliseconds.
type Frame = (Art, u64);

// --- The stills. Each mascot's resting rows come from the catalog
// (`Mascot::art`), so a still can only ever differ from rest in the row it
// redraws. ---

const CREST: Art = Mascot::Crest.art();
const BLOOM: Art = Mascot::Bloom.art();
const SPROUT: Art = Mascot::Sprout.art();
const TWIN: Art = Mascot::Twin.art();
const SKITER: Art = Mascot::Skiter.art();
const GEM: Art = Mascot::Gem.art();

/// An upper-half eye's blink: the lid up, shut, and down again.
const LID_MS: u64 = 45;
const SHUT_MS: u64 = 95;
/// A lower-half eye has no lid glyph, so its blink is the one cut.
const CUT_MS: u64 = 120;
/// A glance: each look held long enough to read, centre passed through.
const LOOK_MS: u64 = 460;
const CENTRE_MS: u64 = 80;

const CREST_BLINK: &[Frame] = &[
    (&[CREST[0], "▝▜▆███▆▛▘", CREST[2]], LID_MS),
    (&[CREST[0], "▝▜█████▛▘", CREST[2]], SHUT_MS),
    (&[CREST[0], "▝▜▆███▆▛▘", CREST[2]], LID_MS),
];
const CREST_GLANCE: &[Frame] = &[
    (&[CREST[0], "▝▜▟███▟▛▘", CREST[2]], LOOK_MS),
    (CREST, CENTRE_MS),
    (&[CREST[0], "▝▜▙███▙▛▘", CREST[2]], LOOK_MS),
];
/// The frill drops flat, then flares back up from the middle.
const CREST_FLARE: &[Frame] = &[
    (&[" ▄▄▄▄▄▄▄", CREST[1], CREST[2]], 130),
    (&[" ▄▄▙▄▟▄▄", CREST[1], CREST[2]], 70),
];

const BLOOM_BLINK: &[Frame] = &[
    (&[BLOOM[0], "▀█▆███▆█▀", BLOOM[2]], LID_MS),
    (&[BLOOM[0], "▀███████▀", BLOOM[2]], SHUT_MS),
    (&[BLOOM[0], "▀█▆███▆█▀", BLOOM[2]], LID_MS),
];
const BLOOM_GLANCE: &[Frame] = &[
    (&[BLOOM[0], "▀█▟███▟█▀", BLOOM[2]], LOOK_MS),
    (BLOOM, CENTRE_MS),
    (&[BLOOM[0], "▀█▙███▙█▀", BLOOM[2]], LOOK_MS),
];
/// The head swells to the block's full width and eases back.
const BLOOM_SWELL: &[Frame] = &[
    (&["▗▄█████▄▖", BLOOM[1], BLOOM[2]], 80),
    (&["▄███████▄", BLOOM[1], BLOOM[2]], 220),
    (&["▗▄█████▄▖", BLOOM[1], BLOOM[2]], 80),
];

const SPROUT_BLINK: &[Frame] = &[(&[SPROUT[0], "▗▟█████▙▖", SPROUT[2]], CUT_MS)];
const SPROUT_GLANCE: &[Frame] = &[
    (&[SPROUT[0], "▗▟▜███▜▙▖", SPROUT[2]], LOOK_MS),
    (SPROUT, CENTRE_MS),
    (&[SPROUT[0], "▗▟▛███▛▙▖", SPROUT[2]], LOOK_MS),
];
/// The leaf's tip leans left, stands, and leans right.
const SPROUT_SWAY: &[Frame] = &[
    (&["   ▝▙", SPROUT[1], SPROUT[2]], 140),
    (SPROUT, 80),
    (&["    ▟▘", SPROUT[1], SPROUT[2]], 140),
];

const TWIN_BLINK: &[Frame] = &[
    (&[TWIN[0], "▝▜▆███▆▛▘", TWIN[2]], LID_MS),
    (&[TWIN[0], "▝▜█████▛▘", TWIN[2]], SHUT_MS),
    (&[TWIN[0], "▝▜▆███▆▛▘", TWIN[2]], LID_MS),
];
const TWIN_GLANCE: &[Frame] = &[
    (&[TWIN[0], "▝▜▟███▟▛▘", TWIN[2]], LOOK_MS),
    (TWIN, CENTRE_MS),
    (&[TWIN[0], "▝▜▙███▙▛▘", TWIN[2]], LOOK_MS),
];
/// One ear flicks flat, then the other.
const TWIN_FLICK: &[Frame] = &[
    (&[" ▄▄▄▄▄██", TWIN[1], TWIN[2]], 90),
    (TWIN, 70),
    (&[" ██▄▄▄▄▄", TWIN[1], TWIN[2]], 90),
];

const SKITER_BLINK: &[Frame] = &[(&["▗▟█████▙▖", SKITER[1], SKITER[2]], CUT_MS)];
const SKITER_GLANCE: &[Frame] = &[
    (&["▗▟▜███▜▙▖", SKITER[1], SKITER[2]], LOOK_MS),
    (SKITER, CENTRE_MS),
    (&["▗▟▛███▛▙▖", SKITER[1], SKITER[2]], LOOK_MS),
];
/// The legs trade positions three times — a skitter on the spot.
const SKITER_STEP: Art = &[SKITER[0], SKITER[1], " ▝▝▘ ▝▘▘"];
const SKITER_SKITTER: &[Frame] = &[
    (SKITER_STEP, 70),
    (SKITER, 70),
    (SKITER_STEP, 70),
    (SKITER, 70),
    (SKITER_STEP, 70),
];

/// The gem has no eyes: its centre facet closes like one…
const GEM_BLINK: &[Frame] = &[
    (&[GEM[0], "▝▀██▆██▀▘", GEM[2]], LID_MS),
    (&[GEM[0], "▝▀█████▀▘", GEM[2]], SHUT_MS),
    (&[GEM[0], "▝▀██▆██▀▘", GEM[2]], LID_MS),
];
/// …and slides a cell each way as the gem turns in the light.
const GEM_TURN: &[Frame] = &[
    (&[GEM[0], "▝▀█▄███▀▘", GEM[2]], 200),
    (GEM, CENTRE_MS),
    (&[GEM[0], "▝▀███▄█▀▘", GEM[2]], 200),
];

impl Mascot {
    /// The stills of `gesture` as this mascot makes it, in order. A gesture
    /// starts from the resting art and the schedule returns to it after, so
    /// the stills hold only the moves themselves.
    #[must_use]
    pub fn stills(self, gesture: Gesture) -> Vec<Still> {
        let frames = match (self, gesture) {
            (_, Gesture::Glint) => return self.glint_stills(false),
            // The gem's signature is light: the glint, travelling back.
            (Self::Gem, Gesture::Signature) => return self.glint_stills(true),
            (Self::Crest, Gesture::Blink) => CREST_BLINK,
            (Self::Crest, Gesture::Glance) => CREST_GLANCE,
            (Self::Crest, Gesture::Signature) => CREST_FLARE,
            (Self::Bloom, Gesture::Blink) => BLOOM_BLINK,
            (Self::Bloom, Gesture::Glance) => BLOOM_GLANCE,
            (Self::Bloom, Gesture::Signature) => BLOOM_SWELL,
            (Self::Sprout, Gesture::Blink) => SPROUT_BLINK,
            (Self::Sprout, Gesture::Glance) => SPROUT_GLANCE,
            (Self::Sprout, Gesture::Signature) => SPROUT_SWAY,
            (Self::Twin, Gesture::Blink) => TWIN_BLINK,
            (Self::Twin, Gesture::Glance) => TWIN_GLANCE,
            (Self::Twin, Gesture::Signature) => TWIN_FLICK,
            (Self::Skiter, Gesture::Blink) => SKITER_BLINK,
            (Self::Skiter, Gesture::Glance) => SKITER_GLANCE,
            (Self::Skiter, Gesture::Signature) => SKITER_SKITTER,
            (Self::Gem, Gesture::Blink) => GEM_BLINK,
            (Self::Gem, Gesture::Glance) => GEM_TURN,
        };
        frames
            .iter()
            .map(|&(art, ms)| Still {
                pose: MascotPose { art, glint: None },
                hold: Duration::from_millis(ms),
            })
            .collect()
    }

    /// A glint's stills over the resting art: the band's centre stepping one
    /// column a still across every position that lights a glyph
    /// ([`MascotPose::light`]) — from the first to light one to the last, so
    /// no still is a frame drawn for nothing — or the same sweep `back`wards.
    fn glint_stills(self, back: bool) -> Vec<Still> {
        let lit = |centre: i8| MascotPose {
            art: self.art(),
            glint: Some(centre),
        };
        // The band reaches a few columns past either edge of the block (its
        // half width, plus the lean); twice the block either way is clear of
        // it for any art this small.
        let width = i8::try_from(self.art_width()).unwrap_or(i8::MAX / 4);
        let mut stills: Vec<Still> = (-width..=width.saturating_mul(2))
            .map(lit)
            .filter(MascotPose::is_lit)
            .map(|pose| Still {
                pose,
                hold: Duration::from_millis(GLINT_STEP_MS),
            })
            .collect();
        if back {
            stills.reverse();
        }
        stills
    }
}

/// One beat of an act: a gesture, or a rest between two of them.
#[derive(Clone, Copy)]
enum Beat {
    Do(Gesture),
    Rest(u64),
}

/// Bar 0's act: a glint crosses the mascot, it makes its signature move, and
/// it blinks — about two seconds of hello, the same at every launch.
const GREETING: &[Beat] = &[
    Beat::Do(Gesture::Glint),
    Beat::Rest(240),
    Beat::Do(Gesture::Signature),
    Beat::Rest(420),
    Beat::Do(Gesture::Blink),
];

/// What an ordinary bar plays, by weight out of [`ACT_WEIGHTS`]: mostly a
/// blink, the way a face at rest mostly blinks.
const ACTS: &[(u64, &[Beat])] = &[
    (52, &[Beat::Do(Gesture::Blink)]),
    (
        12,
        &[
            Beat::Do(Gesture::Blink),
            Beat::Rest(150),
            Beat::Do(Gesture::Blink),
        ],
    ),
    (16, &[Beat::Do(Gesture::Glance)]),
    (14, &[Beat::Do(Gesture::Signature)]),
    (6, &[Beat::Do(Gesture::Glint)]),
];

/// The sum of [`ACTS`]' weights.
const ACT_WEIGHTS: u64 = 100;

/// A well-mixed 64 bits for bar `bar` of the schedule `seed` names
/// (splitmix64's finaliser over the pair) — what scatters the acts' starts
/// and picks, with no state to carry from one bar to the next.
const fn mix(seed: u64, bar: u64) -> u64 {
    let mut z = seed ^ bar.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// When bar `bar`'s act starts (milliseconds into the bar) and what it is.
fn plan(seed: u64, bar: u64) -> (u64, &'static [Beat]) {
    if bar == 0 {
        return (GREETING_MS, GREETING);
    }
    let hash = mix(seed, bar);
    let start = ACT_EARLIEST_MS + hash % (ACT_LATEST_MS - ACT_EARLIEST_MS + 1);
    let mut roll = (hash >> 32) % ACT_WEIGHTS;
    for &(weight, act) in ACTS {
        if roll < weight {
            return (start, act);
        }
        roll -= weight;
    }
    (start, ACTS[0].1)
}

/// Bar `bar` as spans — each the millisecond it ends on (within the bar) and
/// its pose — covering the whole bar, equal neighbours folded into one. It
/// opens and closes at rest.
fn bar_spans(mascot: Mascot, seed: u64, bar: u64) -> Vec<(u64, MascotPose)> {
    let rest = MascotPose::rest(mascot);
    let (start, act) = plan(seed, bar);
    let mut spans: Vec<(u64, MascotPose)> = Vec::new();
    let mut push = |end: u64, pose: MascotPose| match spans.last_mut() {
        Some((last_end, last)) if *last == pose => *last_end = end,
        _ => spans.push((end, pose)),
    };
    let mut at = start;
    push(at, rest);
    for beat in act {
        match *beat {
            Beat::Rest(ms) => {
                at += ms;
                push(at, rest);
            }
            Beat::Do(gesture) => {
                for still in mascot.stills(gesture) {
                    at += u64::try_from(still.hold.as_millis()).unwrap_or(u64::MAX);
                    push(at, still.pose);
                }
            }
        }
    }
    debug_assert!(at < BAR_MS, "an act outruns its bar");
    push(BAR_MS, rest);
    spans
}

/// Where `mascot`'s motion stands `elapsed` after its banner appeared, on
/// the schedule `seed` names: the pose to draw and how long it holds
/// (`docs/mascot-motion.md`).
///
/// Time is cut into bars of [`MOTION_BAR`]. Bar 0 plays the greeting
/// [`MOTION_GREETING_DELAY`] in; every later bar starts one act between
/// [`MOTION_ACT_EARLIEST`] and [`MOTION_ACT_LATEST`] in, the start and the
/// act both hashed from the seed and the bar's index. A bar opens and closes
/// at rest, and the hold of a closing rest runs on through the next bar's
/// opening one — so hopping by [`MascotMotion::holds`] always lands on a
/// pose that differs, and a resting mascot asks for nothing until its next
/// act.
#[must_use]
pub fn mascot_motion(mascot: Mascot, seed: u64, elapsed: Duration) -> MascotMotion {
    let at = u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX);
    let (bar, into) = (at / BAR_MS, at % BAR_MS);
    let spans = bar_spans(mascot, seed, bar);
    let index = spans
        .iter()
        .position(|&(end, _)| into < end)
        .unwrap_or(spans.len() - 1);
    let (end, pose) = spans[index];
    let mut holds = end - into;
    if index + 1 == spans.len() {
        // The bar's closing rest: it lasts through the next bar's opening.
        holds += bar_spans(mascot, seed, bar.saturating_add(1))[0].0;
    }
    MascotMotion {
        pose,
        holds: Duration::from_millis(holds),
    }
}

impl App {
    /// Seed the mascot's schedule — the boundary's entropy at bootstrap, so
    /// two sessions do not blink in step. Unseeded (every unit test) it is
    /// schedule 0. The greeting is the same on every seed.
    pub const fn seed_mascot_motion(&mut self, seed: u64) {
        self.mascot_motion_seed = seed;
    }

    /// Whether the banner's mascot moves right now: the **Mascot animation**
    /// setting is on, the conversation holds no cell yet, and the screen is
    /// the main conversation's — an agent session view's banner only tops
    /// another conversation's transcript. Chrome committed under the banner
    /// (a notice card) is not a cell. See `docs/mascot-motion.md`.
    #[must_use]
    pub fn banner_in_motion(&self) -> bool {
        self.settings.mascot_animation && self.history.is_empty() && self.agent_view.is_none()
    }

    /// The banner mascot's motion at this frame — `None` when it does not
    /// move ([`banner_in_motion`](Self::banner_in_motion)), which is the
    /// resting art. Timed from the first frame the banner was up for, not
    /// from launch.
    #[must_use]
    pub fn banner_motion(&self) -> Option<MascotMotion> {
        self.banner_in_motion().then(|| {
            mascot_motion(
                self.mascot,
                self.mascot_motion_seed,
                self.since(self.banner_epoch),
            )
        })
    }

    /// The `/mascot` picker's highlighted mascot in motion — its preview
    /// plays the greeting from the moment the highlight lands on it, on a
    /// clock of its own. `None` with the picker closed, nothing highlighted,
    /// or the setting off.
    #[must_use]
    pub fn mascot_preview_motion(&self) -> Option<MascotMotion> {
        if !self.settings.mascot_animation {
            return None;
        }
        let shown_at = self.mascot_picker.as_ref()?.shown_at;
        let mascot = self.highlighted_mascot()?;
        Some(mascot_motion(
            mascot,
            self.mascot_motion_seed,
            self.since(shown_at),
        ))
    }

    /// How long ago `epoch` was on the frame clock — zero for one not
    /// latched yet, which is a motion that has not begun.
    fn since(&self, epoch: Option<Duration>) -> Duration {
        epoch.map_or(Duration::ZERO, |epoch| self.pulse.saturating_sub(epoch))
    }

    /// Start the motion clocks that are waiting for a frame: the banner's,
    /// and the open picker's preview. Called with every frame-clock reading
    /// ([`set_pulse`](Self::set_pulse)), so each counts from the first frame
    /// its mascot was actually on screen for — the checkpoint snapshot can
    /// hold the first frame for seconds, and a greeting must not have played
    /// to nobody.
    pub(super) fn latch_mascot_clocks(&mut self) {
        let now = self.pulse;
        self.banner_epoch.get_or_insert(now);
        if let Some(picker) = self.mascot_picker.as_mut() {
            picker.shown_at.get_or_insert(now);
        }
    }
}
