//! The banner mascot's idle animation: what each mascot does while it has
//! nothing to do, and when. See `docs/mascot-animation.md`.
//!
//! A mascot is three rows of block glyphs ([`Mascot::art`]). Its animation is
//! a handful of **frames** — the same three rows with one part moved: the
//! eyes shut, a crest flared, a leaf unfurled, an ear folded, the legs
//! stepped, a spark opened — played as short **clips** on a **timeline**
//! that is mostly stillness. The eyes do most of the work: every mascot that
//! has them blinks every few seconds and glances aside now and then, and the
//! gem, which has none, twinkles where the others blink. Each mascot also has
//! one move of its own, the thing its catalog description says it does.
//!
//! The timeline is a **pure function of a seed and a clock** — no state, no
//! thread, nothing to advance: [`Mascot::pose`] says what the mascot looks
//! like at an instant and [`Mascot::pose_holds`] how long that lasts, which
//! is all the loop needs to draw a frame and sleep until the next one. It
//! opens on a scripted **greeting** (a light sweeps the mascot, it shows its
//! move, it blinks) and then runs in **beats** of [`BEAT_MS`]: each beat
//! draws one clip by lot from the mascot's table, or none, and starts it at
//! a drawn offset — so the rhythm never settles into a pattern, two sessions
//! never move in step, and the pose three hours in costs the same to compute
//! as the pose three seconds in.
//!
//! Only the *motion* lives here, beside the art it moves. What a pose looks
//! like on screen — the gradient, the sweep's light — is `ui::header`'s, and
//! where it may be drawn at all is the boundary's: the banner is repainted
//! only while the conversation under it is empty ([`App::banner_is_live`]),
//! and what reaches scrollback is always the rest pose.

use super::*;

/// How often a light sweep repaints while it crosses the mascot — 25 frames
/// a second, a step of under a column for the band, and a whole number of
/// frames to a sweep so its last one is not a sliver.
pub const MASCOT_SHINE_FRAME: Duration = Duration::from_millis(40);

/// How long the scripted greeting runs, in milliseconds, before the beats
/// drawn by lot begin.
pub const MASCOT_GREETING_MS: u64 = 3_600;

/// One beat of the timeline, in milliseconds: at most one clip starts in
/// each. Three seconds gives a move every three and a half on average, a
/// blink about every five — the pace of something calm and awake.
const BEAT_MS: u64 = 3_000;

/// The stillness every beat keeps at its end, so a clip that ends a beat and
/// the one that opens the next never run together.
const BEAT_BREATH_MS: u64 = 200;

/// How many beats ahead [`Mascot::pose_holds`] looks for the next clip
/// before it settles for waking at a beat boundary to look again.
const BEAT_LOOKAHEAD: u64 = 8;

/// When each of the greeting's three clips starts, in milliseconds after the
/// banner appears: a moment to be seen standing still, then the sweep, the
/// mascot's own move, and a blink.
const GREETING_STARTS: [u64; 3] = [400, 1_500, 2_600];

/// Where each of the greeting's three slots begins — the first holds the
/// stillness before the sweep as well.
const GREETING_SLOTS: [u64; 3] = [0, 1_500, 2_600];

// --- The clips' timing. A blink is quick and asymmetric, the way a real one
// is: the lid drops faster than it lifts. ---

/// An eyelid on its way down.
const LID_DOWN_MS: u64 = 35;
/// Eyes shut, between the two lids.
const SHUT_MS: u64 = 90;
/// An eyelid on its way back up.
const LID_UP_MS: u64 = 50;
/// Eyes shut where the glyphs offer no lid: the whole blink in one frame.
const BLINK_MS: u64 = 120;
/// Eyes open between the two halves of a double blink.
const BLINK_APART_MS: u64 = 150;
/// A glance to one side.
const GLANCE_MS: u64 = 640;
/// Each side of a look around, and the beat at the middle between them.
const LOOK_MS: u64 = 460;
const LOOK_APART_MS: u64 = 150;
/// One sweep of light across the mascot — nineteen [`MASCOT_SHINE_FRAME`]s.
const SWEEP_MS: u64 = 760;
/// The three stills of a spark opening (and, played back, closing).
const SPARK_MS: u64 = 70;
const SPARK_OPEN_MS: u64 = 120;

/// One still of a mascot — its rows, drawn verbatim like [`Mascot::art`].
type Art = &'static [&'static str];

/// One step of a [`Clip`]: a still, and how long it stays up.
#[derive(Debug, Clone, Copy)]
struct Step {
    art: Art,
    ms: u64,
}

/// A short run of [`Step`]s — a blink, a glance, a mascot's own move —
/// optionally under a sweep of light that crosses the art once over the
/// clip's whole length.
#[derive(Debug, Clone, Copy)]
struct Clip {
    steps: &'static [Step],
    shine: bool,
}

/// A [`Clip`]: its steps in order, each `STILL for MILLISECONDS`.
macro_rules! clip {
    ($($art:ident for $ms:expr),+ $(,)?) => {
        Clip {
            steps: &[$(Step { art: $art, ms: $ms }),+],
            shine: false,
        }
    };
}

/// A [`Clip`] played under a sweep of light.
macro_rules! lit_clip {
    ($($art:ident for $ms:expr),+ $(,)?) => {
        Clip {
            steps: &[$(Step { art: $art, ms: $ms }),+],
            shine: true,
        }
    };
}

impl Clip {
    /// The clip's whole length, in milliseconds.
    fn len(&self) -> u64 {
        self.steps.iter().map(|step| step.ms).sum()
    }

    /// The still on screen `at` milliseconds into the clip and how long it
    /// has left, or `None` once the clip is over.
    fn step_at(&self, at: u64) -> Option<(Art, u64)> {
        let mut end = 0;
        self.steps.iter().find_map(|step| {
            end += step.ms;
            (at < end).then(|| (step.art, end - at))
        })
    }

    /// The pose `at` milliseconds into the clip, or `None` once it is over.
    fn pose_at(&self, at: u64) -> Option<MascotPose> {
        let (art, _) = self.step_at(at)?;
        Some(MascotPose {
            art,
            // Millisecond counts of a clip well under a minute: exact in f32.
            shine: self.shine.then(|| at as f32 / self.len() as f32),
        })
    }

    /// How long the pose `at` milliseconds into the clip holds: to the end
    /// of its step, or — under a sweep, which moves on every frame — one
    /// [`MASCOT_SHINE_FRAME`] at most.
    fn holds_at(&self, at: u64) -> Option<u64> {
        let (_, left) = self.step_at(at)?;
        let frame = u64::try_from(MASCOT_SHINE_FRAME.as_millis()).unwrap_or(u64::MAX);
        Some(if self.shine { left.min(frame) } else { left })
    }
}

/// Everything one mascot does: its greeting, and the clips its beats draw
/// from.
struct Motions {
    /// The greeting's three clips, in order — the sweep, the mascot's own
    /// move, and a blink (the gem's: a glint and two twinkles).
    greeting: [&'static Clip; 3],
    /// The clips a beat draws from, each with its share of a hundred beats.
    /// What the shares leave over are the beats the mascot stands still —
    /// and the **first** entry is its everyday move, the one a second still
    /// beat in a row plays instead of staying frozen.
    beats: &'static [(u64, &'static Clip)],
}

impl Motions {
    /// The clip a `roll` of `0..100` draws, or `None` for a still beat.
    fn drawn(&self, roll: u64) -> Option<&'static Clip> {
        let mut share = 0;
        self.beats.iter().find_map(|&(weight, clip)| {
            share += weight;
            (roll < share).then_some(clip)
        })
    }

    /// Every clip the mascot can play.
    fn clips(&self) -> impl Iterator<Item = &'static Clip> {
        self.greeting
            .into_iter()
            .chain(self.beats.iter().map(|&(_, clip)| clip))
    }
}

/// The crested hatchling: lidded eyes, and a frill it flares — the spikes
/// pull in for an instant, fan out to six, and settle.
mod crest {
    use super::*;

    const REST: Art = Mascot::Crest.art();
    const LID: Art = &[" ▙▄▙▄▟▄▟", "▝▜▆███▆▛▘", "  ▘▘ ▝▝"];
    const SHUT: Art = &[" ▙▄▙▄▟▄▟", "▝▜█████▛▘", "  ▘▘ ▝▝"];
    const LOOK_LEFT: Art = &[" ▙▄▙▄▟▄▟", "▝▜▟███▟▛▘", "  ▘▘ ▝▝"];
    const LOOK_RIGHT: Art = &[" ▙▄▙▄▟▄▟", "▝▜▙███▙▛▘", "  ▘▘ ▝▝"];
    const RUFFLE: Art = &[" ▟▄▟▄▙▄▙", "▝▜▄███▄▛▘", "  ▘▘ ▝▝"];
    const FLARE: Art = &[" ▙▙▙▄▟▟▟", "▝▜▄███▄▛▘", "  ▘▘ ▝▝"];

    const SWEEP: Clip = lit_clip![REST for SWEEP_MS];
    const BLINK: Clip = clip![LID for LID_DOWN_MS, SHUT for SHUT_MS, LID for LID_UP_MS];
    const BLINK_TWICE: Clip = clip![
        LID for LID_DOWN_MS, SHUT for SHUT_MS, LID for LID_UP_MS,
        REST for BLINK_APART_MS,
        LID for LID_DOWN_MS, SHUT for SHUT_MS, LID for LID_UP_MS,
    ];
    const GLANCE_LEFT: Clip = clip![LOOK_LEFT for GLANCE_MS];
    const GLANCE_RIGHT: Clip = clip![LOOK_RIGHT for GLANCE_MS];
    const LOOK_AROUND: Clip = clip![
        LOOK_LEFT for LOOK_MS, REST for LOOK_APART_MS, LOOK_RIGHT for LOOK_MS,
    ];
    const FLARE_FRILL: Clip = clip![RUFFLE for 70, FLARE for 460, RUFFLE for 70];

    pub(super) const MOTIONS: Motions = Motions {
        greeting: [&SWEEP, &FLARE_FRILL, &BLINK],
        beats: &[
            (48, &BLINK),
            (12, &BLINK_TWICE),
            (6, &GLANCE_LEFT),
            (6, &GLANCE_RIGHT),
            (4, &LOOK_AROUND),
            (9, &FLARE_FRILL),
        ],
    };
}

/// The round creature in bloom: lidded eyes, and petals at its sides that
/// it flutters — down and up, three times, like something about to lift off.
mod bloom {
    use super::*;

    const REST: Art = Mascot::Bloom.art();
    const LID: Art = &[" ▄█████▄", "▀█▆███▆█▀", " ▝▝   ▘▘"];
    const SHUT: Art = &[" ▄█████▄", "▀███████▀", " ▝▝   ▘▘"];
    const LOOK_LEFT: Art = &[" ▄█████▄", "▀█▟███▟█▀", " ▝▝   ▘▘"];
    const LOOK_RIGHT: Art = &[" ▄█████▄", "▀█▙███▙█▀", " ▝▝   ▘▘"];
    const DOWN: Art = &[" ▄█████▄", "▄█▄███▄█▄", " ▝▝   ▘▘"];

    const SWEEP: Clip = lit_clip![REST for SWEEP_MS];
    const BLINK: Clip = clip![LID for LID_DOWN_MS, SHUT for SHUT_MS, LID for LID_UP_MS];
    const BLINK_TWICE: Clip = clip![
        LID for LID_DOWN_MS, SHUT for SHUT_MS, LID for LID_UP_MS,
        REST for BLINK_APART_MS,
        LID for LID_DOWN_MS, SHUT for SHUT_MS, LID for LID_UP_MS,
    ];
    const GLANCE_LEFT: Clip = clip![LOOK_LEFT for GLANCE_MS];
    const GLANCE_RIGHT: Clip = clip![LOOK_RIGHT for GLANCE_MS];
    const LOOK_AROUND: Clip = clip![
        LOOK_LEFT for LOOK_MS, REST for LOOK_APART_MS, LOOK_RIGHT for LOOK_MS,
    ];
    const FLUTTER: Clip = clip![
        DOWN for 110, REST for 110, DOWN for 110, REST for 110, DOWN for 110,
    ];

    pub(super) const MOTIONS: Motions = Motions {
        greeting: [&SWEEP, &FLUTTER, &BLINK],
        beats: &[
            (48, &BLINK),
            (12, &BLINK_TWICE),
            (6, &GLANCE_LEFT),
            (6, &GLANCE_RIGHT),
            (4, &LOOK_AROUND),
            (9, &FLUTTER),
        ],
    };
}

/// The seedling: eyes in the lower half of their cells, where the font-safe
/// glyphs offer no lid, so it blinks in one frame — and a shoot that sways
/// in a draught or unfurls its first leaf.
mod sprout {
    use super::*;

    const REST: Art = Mascot::Sprout.art();
    const SHUT: Art = &["    █", "▗▟█████▙▖", " ▝▛▛▀▜▜▘"];
    const LOOK_LEFT: Art = &["    █", "▗▟▜███▜▙▖", " ▝▛▛▀▜▜▘"];
    const LOOK_RIGHT: Art = &["    █", "▗▟▛███▛▙▖", " ▝▛▛▀▜▜▘"];
    const TIP_RIGHT: Art = &["    ▟▘", "▗▟▀███▀▙▖", " ▝▛▛▀▜▜▘"];
    const TIP_LEFT: Art = &["   ▝▙", "▗▟▀███▀▙▖", " ▝▛▛▀▜▜▘"];
    const LEAF: Art = &["    █▀", "▗▟▀███▀▙▖", " ▝▛▛▀▜▜▘"];

    const SWEEP: Clip = lit_clip![REST for SWEEP_MS];
    const BLINK: Clip = clip![SHUT for BLINK_MS];
    const BLINK_TWICE: Clip = clip![
        SHUT for BLINK_MS, REST for BLINK_APART_MS, SHUT for BLINK_MS,
    ];
    const GLANCE_LEFT: Clip = clip![LOOK_LEFT for GLANCE_MS];
    const GLANCE_RIGHT: Clip = clip![LOOK_RIGHT for GLANCE_MS];
    const LOOK_AROUND: Clip = clip![
        LOOK_LEFT for LOOK_MS, REST for LOOK_APART_MS, LOOK_RIGHT for LOOK_MS,
    ];
    const SWAY: Clip = clip![TIP_RIGHT for 170, REST for 110, TIP_LEFT for 170];
    const UNFURL: Clip = clip![TIP_RIGHT for 90, LEAF for 520, TIP_RIGHT for 90];

    pub(super) const MOTIONS: Motions = Motions {
        greeting: [&SWEEP, &UNFURL, &BLINK],
        beats: &[
            (48, &BLINK),
            (12, &BLINK_TWICE),
            (6, &GLANCE_LEFT),
            (6, &GLANCE_RIGHT),
            (4, &LOOK_AROUND),
            (5, &SWAY),
            (4, &UNFURL),
        ],
    };
}

/// The two-eared one: lidded eyes, and an ear that flicks — folds flat and
/// springs back, twice — now the right, now the left.
mod twin {
    use super::*;

    const REST: Art = Mascot::Twin.art();
    const LID: Art = &[" ██▄▄▄██", "▝▜▆███▆▛▘", "  ▘▘ ▝▝"];
    const SHUT: Art = &[" ██▄▄▄██", "▝▜█████▛▘", "  ▘▘ ▝▝"];
    const LOOK_LEFT: Art = &[" ██▄▄▄██", "▝▜▟███▟▛▘", "  ▘▘ ▝▝"];
    const LOOK_RIGHT: Art = &[" ██▄▄▄██", "▝▜▙███▙▛▘", "  ▘▘ ▝▝"];
    const LEFT_HALF: Art = &[" █▄▄▄▄██", "▝▜▄███▄▛▘", "  ▘▘ ▝▝"];
    const LEFT_FLAT: Art = &[" ▄▄▄▄▄██", "▝▜▄███▄▛▘", "  ▘▘ ▝▝"];
    const RIGHT_HALF: Art = &[" ██▄▄▄▄█", "▝▜▄███▄▛▘", "  ▘▘ ▝▝"];
    const RIGHT_FLAT: Art = &[" ██▄▄▄▄▄", "▝▜▄███▄▛▘", "  ▘▘ ▝▝"];

    const SWEEP: Clip = lit_clip![REST for SWEEP_MS];
    const BLINK: Clip = clip![LID for LID_DOWN_MS, SHUT for SHUT_MS, LID for LID_UP_MS];
    const BLINK_TWICE: Clip = clip![
        LID for LID_DOWN_MS, SHUT for SHUT_MS, LID for LID_UP_MS,
        REST for BLINK_APART_MS,
        LID for LID_DOWN_MS, SHUT for SHUT_MS, LID for LID_UP_MS,
    ];
    const GLANCE_LEFT: Clip = clip![LOOK_LEFT for GLANCE_MS];
    const GLANCE_RIGHT: Clip = clip![LOOK_RIGHT for GLANCE_MS];
    const LOOK_AROUND: Clip = clip![
        LOOK_LEFT for LOOK_MS, REST for LOOK_APART_MS, LOOK_RIGHT for LOOK_MS,
    ];
    const FLICK_RIGHT: Clip = clip![
        RIGHT_HALF for 60, RIGHT_FLAT for 110, RIGHT_HALF for 60,
        REST for 100,
        RIGHT_HALF for 60, RIGHT_FLAT for 110, RIGHT_HALF for 60,
    ];
    const FLICK_LEFT: Clip = clip![
        LEFT_HALF for 60, LEFT_FLAT for 110, LEFT_HALF for 60,
        REST for 100,
        LEFT_HALF for 60, LEFT_FLAT for 110, LEFT_HALF for 60,
    ];

    pub(super) const MOTIONS: Motions = Motions {
        greeting: [&SWEEP, &FLICK_RIGHT, &BLINK],
        beats: &[
            (48, &BLINK),
            (12, &BLINK_TWICE),
            (6, &GLANCE_LEFT),
            (6, &GLANCE_RIGHT),
            (4, &LOOK_AROUND),
            (5, &FLICK_RIGHT),
            (4, &FLICK_LEFT),
        ],
    };
}

/// The skitterer: eyes in the lower half of their cells (a one-frame blink,
/// like the seedling's), and six legs that scuttle on the spot.
mod skiter {
    use super::*;

    const REST: Art = Mascot::Skiter.art();
    const SHUT: Art = &["▗▟█████▙▖", "▝▜█████▛▘", " ▘▘▝ ▘▝▝"];
    const LOOK_LEFT: Art = &["▗▟▜███▜▙▖", "▝▜█████▛▘", " ▘▘▝ ▘▝▝"];
    const LOOK_RIGHT: Art = &["▗▟▛███▛▙▖", "▝▜█████▛▘", " ▘▘▝ ▘▝▝"];
    const LEGS_RIGHT: Art = &["▗▟▀███▀▙▖", "▝▜█████▛▘", " ▝▝ ▘▝ ▘▘"];
    const LEGS_LEFT: Art = &["▗▟▀███▀▙▖", "▝▜█████▛▘", "▝▝ ▘▝ ▘▘"];

    const SWEEP: Clip = lit_clip![REST for SWEEP_MS];
    const BLINK: Clip = clip![SHUT for BLINK_MS];
    const BLINK_TWICE: Clip = clip![
        SHUT for BLINK_MS, REST for BLINK_APART_MS, SHUT for BLINK_MS,
    ];
    const GLANCE_LEFT: Clip = clip![LOOK_LEFT for GLANCE_MS];
    const GLANCE_RIGHT: Clip = clip![LOOK_RIGHT for GLANCE_MS];
    const LOOK_AROUND: Clip = clip![
        LOOK_LEFT for LOOK_MS, REST for LOOK_APART_MS, LOOK_RIGHT for LOOK_MS,
    ];
    const SKITTER: Clip = clip![
        LEGS_RIGHT for 75, LEGS_LEFT for 75, LEGS_RIGHT for 75,
        LEGS_LEFT for 75, LEGS_RIGHT for 75, LEGS_LEFT for 75,
    ];

    pub(super) const MOTIONS: Motions = Motions {
        greeting: [&SWEEP, &SKITTER, &BLINK],
        beats: &[
            (48, &BLINK),
            (12, &BLINK_TWICE),
            (6, &GLANCE_LEFT),
            (6, &GLANCE_RIGHT),
            (4, &LOOK_AROUND),
            (9, &SKITTER),
        ],
    };
}

/// The cut gem: no eyes to blink, so it catches the light instead — a spark
/// opens beside it, high on the right or low on the left, where the others
/// blink, and now and then a glint crosses the whole stone.
mod gem {
    use super::*;

    const REST: Art = Mascot::Gem.art();
    const HIGH_1: Art = &["  ▄▟█▙▄ ·", "▝▀██▄██▀▘", "   ▝▀▘"];
    const HIGH_2: Art = &["  ▄▟█▙▄ ✢", "▝▀██▄██▀▘", "   ▝▀▘"];
    const HIGH_3: Art = &["  ▄▟█▙▄ ✶", "▝▀██▄██▀▘", "   ▝▀▘"];
    const LOW_1: Art = &["  ▄▟█▙▄", "▝▀██▄██▀▘", " · ▝▀▘"];
    const LOW_2: Art = &["  ▄▟█▙▄", "▝▀██▄██▀▘", " ✢ ▝▀▘"];
    const LOW_3: Art = &["  ▄▟█▙▄", "▝▀██▄██▀▘", " ✶ ▝▀▘"];

    const TWINKLE_HIGH: Clip = clip![
        HIGH_1 for SPARK_MS, HIGH_2 for SPARK_MS, HIGH_3 for SPARK_OPEN_MS,
        HIGH_2 for SPARK_MS, HIGH_1 for SPARK_MS,
    ];
    const TWINKLE_LOW: Clip = clip![
        LOW_1 for SPARK_MS, LOW_2 for SPARK_MS, LOW_3 for SPARK_OPEN_MS,
        LOW_2 for SPARK_MS, LOW_1 for SPARK_MS,
    ];
    const TWINKLE_BOTH: Clip = clip![
        HIGH_1 for SPARK_MS, HIGH_2 for SPARK_MS, HIGH_3 for SPARK_OPEN_MS,
        HIGH_2 for SPARK_MS, HIGH_1 for SPARK_MS,
        REST for 160,
        LOW_1 for SPARK_MS, LOW_2 for SPARK_MS, LOW_3 for SPARK_OPEN_MS,
        LOW_2 for SPARK_MS, LOW_1 for SPARK_MS,
    ];
    /// The sweep, with the high spark opening as the light passes it.
    const GLINT: Clip = lit_clip![
        REST for 190,
        HIGH_1 for SPARK_MS, HIGH_2 for SPARK_MS, HIGH_3 for SPARK_OPEN_MS,
        HIGH_2 for SPARK_MS, HIGH_1 for SPARK_MS,
        REST for 170,
    ];

    pub(super) const MOTIONS: Motions = Motions {
        greeting: [&GLINT, &TWINKLE_LOW, &TWINKLE_HIGH],
        beats: &[
            (28, &TWINKLE_HIGH),
            (22, &TWINKLE_LOW),
            (8, &TWINKLE_BOTH),
            (10, &GLINT),
        ],
    };
}

/// What a mascot looks like at one instant: the rows to draw, and how far a
/// sweep of light has crossed them. `ui::header` turns one into banner rows;
/// [`MascotPose::rest`] is the pose every banner outside the live one wears —
/// the one committed to scrollback, the Ctrl+O transcript's, an export's.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MascotPose {
    /// The art's rows — the rest art's own count, none wider than
    /// [`Mascot::art_width`], so a pose never moves the banner's metadata
    /// column or its height.
    pub art: &'static [&'static str],
    /// How far a sweep of light has crossed the art, `0.0..1.0` — `None`
    /// when no sweep is passing, which is nearly always.
    pub shine: Option<f32>,
}

impl MascotPose {
    /// `mascot` standing still: its catalog art, unlit.
    #[must_use]
    pub const fn rest(mascot: Mascot) -> Self {
        Self {
            art: mascot.art(),
            shine: None,
        }
    }
}

/// Whole milliseconds of `clock`, saturating — the timeline's resolution.
fn millis(clock: Duration) -> u64 {
    u64::try_from(clock.as_millis()).unwrap_or(u64::MAX)
}

/// The timeline slot `ms` falls in: the greeting's three, then one per beat.
fn slot_at(ms: u64) -> u64 {
    if ms >= MASCOT_GREETING_MS {
        return 3 + (ms - MASCOT_GREETING_MS) / BEAT_MS;
    }
    // The last greeting slot that has begun.
    GREETING_SLOTS
        .iter()
        .rposition(|&start| ms >= start)
        .map_or(0, |slot| slot as u64)
}

/// The millisecond `slot` begins at.
fn slot_start(slot: u64) -> u64 {
    usize::try_from(slot)
        .ok()
        .and_then(|slot| GREETING_SLOTS.get(slot).copied())
        .unwrap_or_else(|| {
            MASCOT_GREETING_MS.saturating_add(slot.saturating_sub(3).saturating_mul(BEAT_MS))
        })
}

/// The lot beat `beat` of the timeline `seed` draws — `SplitMix64`'s
/// finalizer over the pair, so neighbouring beats and neighbouring seeds
/// share nothing.
const fn lot(seed: u64, beat: u64) -> u64 {
    let mut z = seed.wrapping_add(beat.wrapping_add(1).wrapping_mul(0x9E37_79B9_7F4A_7C15));
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

impl Mascot {
    /// Everything this mascot does (`docs/mascot-animation.md`).
    const fn motions(self) -> &'static Motions {
        match self {
            Self::Crest => &crest::MOTIONS,
            Self::Bloom => &bloom::MOTIONS,
            Self::Sprout => &sprout::MOTIONS,
            Self::Twin => &twin::MOTIONS,
            Self::Skiter => &skiter::MOTIONS,
            Self::Gem => &gem::MOTIONS,
        }
    }

    /// What tells this mascot's timeline from another's under the same seed
    /// — its place in the catalog, spread across the word.
    const fn salt(self) -> u64 {
        (self as u64 + 1).wrapping_mul(0xD6E8_FEB8_6659_FD93)
    }

    /// Every frame the animation can draw besides the rest art, each once —
    /// the catalog the tests hold to the art's own rules (three rows, no
    /// wider than the art, single-width glyphs).
    #[must_use]
    pub fn motion_frames(self) -> Vec<&'static [&'static str]> {
        let mut frames: Vec<Art> = Vec::new();
        for step in self.motions().clips().flat_map(|clip| clip.steps) {
            if step.art != self.art() && !frames.contains(&step.art) {
                frames.push(step.art);
            }
        }
        frames
    }

    /// The clip that plays in `slot` of the timeline `seed`, and the
    /// millisecond it starts at — `None` for a beat the mascot stands still.
    ///
    /// The greeting's three slots are scripted. A beat draws its clip by
    /// [`lot`] — over the seed **and the mascot**, so no two mascots play
    /// one sequence (walking the `/mascot` list, each goes on from its
    /// greeting in a rhythm of its own) — and
    /// starts it anywhere that leaves it whole inside the beat with
    /// [`BEAT_BREATH_MS`] to spare. A still beat that follows a still
    /// beat plays the mascot's everyday move instead, so the stillness is
    /// never longer than a beat and the two clips either side of it — about
    /// eight seconds — however the lots fall.
    fn cue(self, seed: u64, slot: u64) -> Option<(u64, &'static Clip)> {
        let motions = self.motions();
        let Some(beat) = slot.checked_sub(3) else {
            let slot = usize::try_from(slot).ok()?;
            return Some((GREETING_STARTS[slot], motions.greeting[slot]));
        };
        let seed = seed ^ self.salt();
        let drawn = lot(seed, beat);
        let clip = motions.drawn(drawn % 100).or_else(|| {
            let before = lot(seed, beat.checked_sub(1)?) % 100;
            motions
                .drawn(before)
                .is_none()
                .then(|| motions.beats.first().map(|&(_, clip)| clip))?
        })?;
        let room = BEAT_MS.saturating_sub(BEAT_BREATH_MS + clip.len());
        Some((slot_start(slot) + (drawn >> 32) % (room + 1), clip))
    }

    /// What this mascot looks like `clock` into the timeline `seed` plays —
    /// the rest art unless a clip is on at that instant.
    ///
    /// `clock` runs from the moment the banner first reached the screen, so
    /// the scripted greeting plays once, at launch; `seed` is the session's,
    /// so no two sessions keep the same rhythm. Pure and stateless: the pose
    /// at any instant costs one hash and a table walk, however long the
    /// session has idled.
    #[must_use]
    pub fn pose(self, seed: u64, clock: Duration) -> MascotPose {
        let ms = millis(clock);
        self.cue(seed, slot_at(ms))
            .and_then(|(start, clip)| clip.pose_at(ms.checked_sub(start)?))
            .unwrap_or_else(|| MascotPose::rest(self))
    }

    /// How long the pose at `clock` holds — when the loop should draw next.
    ///
    /// A still mascot holds until its next clip starts, which is seconds
    /// away: the loop sleeps the whole time and an idle session stays idle.
    /// A step of a clip holds to its end, and a sweep of light for one
    /// [`MASCOT_SHINE_FRAME`]. Never zero.
    #[must_use]
    pub fn pose_holds(self, seed: u64, clock: Duration) -> Duration {
        let ms = millis(clock);
        let slot = slot_at(ms);
        let this = self.cue(seed, slot);
        let held = match this {
            // Still, ahead of this slot's clip.
            Some((start, _)) if ms < start => Some(start - ms),
            // Inside it (`None` once it is over).
            Some((start, clip)) => clip.holds_at(ms - start),
            None => None,
        };
        let wait = held.unwrap_or_else(|| {
            // Still, with this slot's clip behind: until the next one ahead
            // starts — or, past a run of still beats, a beat boundary.
            (slot + 1..=slot + BEAT_LOOKAHEAD)
                .find_map(|ahead| self.cue(seed, ahead))
                .map_or_else(|| slot_start(slot + BEAT_LOOKAHEAD + 1), |(start, _)| start)
                .saturating_sub(ms)
        });
        Duration::from_millis(wait.max(1))
    }
}

impl App {
    /// Seed the mascot's idle animation — the boundary's entropy, handed
    /// over once at bootstrap (the [`seed_tips`](App::seed_tips) pattern).
    /// An `App` nobody seeded never animates, which is every unit test's and
    /// a headless run's.
    pub const fn seed_mascot_motion(&mut self, seed: u64) {
        self.mascot_motion = Some(seed);
    }

    /// Inject the animation's clock before a draw (the
    /// [`set_pulse`](App::set_pulse) pattern): time since a fresh banner
    /// reached the screen, so the greeting plays as the screen opens.
    ///
    /// The same reading starts the `/mascot` picker's preview: the frame
    /// that first shows a mascot under the highlight is the one its preview
    /// counts from, so each mascot **greets** as the highlight lands on it
    /// — however it got there, a key or a search — and one already seen
    /// greets again. A reading older than the one a preview counts from
    /// (the clock started over) starts the preview over with it.
    pub fn set_mascot_clock(&mut self, clock: Duration) {
        self.mascot_clock = clock;
        self.mascot_preview = self
            .highlighted_mascot()
            .map(|mascot| match self.mascot_preview {
                Some((shown, since)) if shown == mascot && since <= clock => (shown, since),
                _ => (mascot, clock),
            });
    }

    /// The animation's seed, when the mascot animates at all in this
    /// session: one was handed over, and the `/settings` **Animate mascot**
    /// row is on.
    const fn mascot_seed(&self) -> Option<u64> {
        if self.settings.animate_mascot {
            self.mascot_motion
        } else {
            None
        }
    }

    /// Whether the mascot animates at all in this session — seeded, and the
    /// `/settings` **Animate mascot** row on.
    #[must_use]
    pub const fn mascot_animates(&self) -> bool {
        self.mascot_seed().is_some()
    }

    /// Whether the banner is still **the banner of an empty conversation** —
    /// no cell recorded under it and no turn in flight to record one — which
    /// is the only time it may be repainted. The first cell ends that for
    /// good: from then on the banner is scrollback, and scrollback does not
    /// move (`docs/mascot-animation.md`). An agent's session view shows
    /// another conversation under the same banner, so it never qualifies.
    #[must_use]
    pub fn banner_is_live(&self) -> bool {
        self.history.is_empty() && !self.turn_active() && self.agent_view.is_none()
    }

    /// The banner mascot's pose now: its animation's, while the banner is
    /// live and the mascot animates — the rest pose otherwise.
    #[must_use]
    pub fn banner_pose(&self) -> MascotPose {
        match self.mascot_seed() {
            Some(seed) if self.banner_is_live() => self.mascot.pose(seed, self.mascot_clock),
            _ => MascotPose::rest(self.mascot),
        }
    }

    /// How far into its own timeline the `/mascot` picker's preview of
    /// `mascot` is: time since the frame that first showed it under the
    /// highlight. Zero — the top of its greeting — for a mascot no frame has
    /// shown yet.
    fn preview_clock(&self, mascot: Mascot) -> Duration {
        match self.mascot_preview {
            Some((shown, since)) if shown == mascot => self.mascot_clock.saturating_sub(since),
            _ => Duration::ZERO,
        }
    }

    /// `mascot`'s pose in the `/mascot` picker's preview, which shows each
    /// mascot moving so one can be chosen for how it behaves — on a clock
    /// of its own, so the mascot under the highlight opens on its greeting
    /// whatever the banner is doing. The rest pose when the mascot does not
    /// animate.
    #[must_use]
    pub fn mascot_pose_for(&self, mascot: Mascot) -> MascotPose {
        self.mascot_seed().map_or_else(
            || MascotPose::rest(mascot),
            |seed| mascot.pose(seed, self.preview_clock(mascot)),
        )
    }

    /// How long until the next animation frame is due, or `None` when no
    /// mascot is moving on screen — the loop's one question, asked after
    /// every draw.
    ///
    /// Two things can be moving: the banner (`banner_on_screen` — the
    /// boundary's word that it is still above the composer where it can be
    /// repainted) and the open `/mascot` picker's preview, each on its own
    /// clock. Neither is on
    /// screen behind an alternate-screen overlay, and a frame there would
    /// wake the loop to repaint nothing.
    #[must_use]
    pub fn mascot_frame_in(&self, banner_on_screen: bool) -> Option<Duration> {
        let seed = self.mascot_seed()?;
        if self.view.is_overlay() {
            return None;
        }
        let banner = (banner_on_screen && self.banner_is_live())
            .then(|| self.mascot.pose_holds(seed, self.mascot_clock));
        let preview = self
            .highlighted_mascot()
            .map(|mascot| mascot.pose_holds(seed, self.preview_clock(mascot)));
        match (banner, preview) {
            (Some(banner), Some(preview)) => Some(banner.min(preview)),
            (banner, preview) => banner.or(preview),
        }
    }
}
