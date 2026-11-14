//! The banner mascot's **motion**: the stills each mascot can strike, the
//! acts that string them into a hop here and a wink there, the schedule that
//! deals those acts out, and the session state that decides whether the
//! banner moves at all. See `docs/mascot-motion.md`.
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
//! mascot never moves. A pose is a **body** — a frame of art — with the
//! **eyes** laid over it. Eyes are holes, half a cell of background punched
//! in a solid face, so an eye's states are the block glyphs that reshape the
//! hole (`▄` open, `▆` half-lidded, `█` shut, `▟`/`▙` looking left/right,
//! `▂` wide): one choreography of blinks, winks and glances serves every
//! mascot, and each mascot's own tables hold only what its body does.

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

/// How much of a bar's end is always at rest, in milliseconds: every act is
/// over by then, so the next bar's can never run into it.
const ACT_CLEAR_MS: u64 = 900;

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
/// what spaces consecutive acts 3.0–6.6 s apart. A long act starts sooner
/// than this, early enough to be over before its bar's closing rest.
pub const MOTION_ACT_LATEST: Duration = Duration::from_millis(ACT_LATEST_MS);

/// How far either side of its centre a glint lights a glyph on one row, in
/// display columns: fully under the centre, half as much a column off, not
/// at all two off — a band three cells wide.
const GLINT_HALF_WIDTH: f32 = 2.0;

/// How long each still of a glint holds, in milliseconds — one column a
/// still, so the band crosses the nine-column block in about half a second.
const GLINT_STEP_MS: u64 = 40;

/// Which way a sided gesture goes, as the screen has it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

impl Side {
    /// The other side.
    const fn other(self) -> Self {
        match self {
            Self::Left => Self::Right,
            Self::Right => Self::Left,
        }
    }

    /// An eye looking this way.
    const fn look(self) -> Eye {
        match self {
            Self::Left => Eye::Left,
            Self::Right => Eye::Right,
        }
    }
}

/// The moves a mascot makes, each drawn in its own anatomy
/// ([`Mascot::stills`]). The first seven are **looks** — the eyes and
/// nothing else; the rest are **moves** of the body or of the light on it.
/// Every mascot has a signature and a flourish of its own, a hop, and the
/// light; not every one has a body for the rest — the gem has a single
/// facet for an eye and no limbs, the twin has ears where the others have
/// arms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gesture {
    /// The eyes close and open — the gem's centre facet, which is its eye.
    Blink,
    /// One eye closes, the one on this side, and is held shut a moment.
    Wink(Side),
    /// The eyes look this way, then the other — the gem's facet sliding a
    /// cell each way, the gem turning in the light.
    Glance(Side),
    /// The eyes flick from side to side, twice, this way first.
    Dart(Side),
    /// The eyes cross: each looks at the other.
    Cross,
    /// The eyes go wide — the gem's facet opening.
    Widen,
    /// It nods off — the frill half down, the head to one side, the leaf
    /// bent, an ear flat — sleeps a second, and starts awake wide-eyed.
    Doze,
    /// The move the mascot's description names: the crest flares its frill,
    /// the bloom swells, the sprout's leaf sways, the twin flicks its ears,
    /// the skitterer skitters, the gem catches the light — two flashes of
    /// it, colour only.
    Signature,
    /// A second move of the mascot's own, this way first: the crest's frill
    /// ripples, the bloom's head wobbles, the sprout unfurls two leaves, the
    /// twin's ears splay, the skitterer scuttles half a cell each way, the
    /// gem spins edge-on and back.
    Flourish(Side),
    /// The whole mascot bobs half a cell and back — the sprout, which stands
    /// on the block's floor, springs up; the gem floats, slowly.
    Hop,
    /// A hello: the crest and the sprout wave the arm on this side, the
    /// bloom flaps both, the skitterer raises its claws.
    Wave(Side),
    /// A hop with the arms thrown up at the top of it — the twin's ears, the
    /// skitterer's claws.
    Cheer,
    /// The feet patter on the spot, two at a time.
    Patter,
    /// The whole mascot swells with light and fades: colour only.
    Glow,
    /// A slanted band of light crosses the resting art, entering from this
    /// side: colour only.
    Glint(Side),
}

impl Gesture {
    /// Every gesture, each sided one both ways.
    pub const ALL: [Self; 21] = [
        Self::Blink,
        Self::Wink(Side::Left),
        Self::Wink(Side::Right),
        Self::Glance(Side::Left),
        Self::Glance(Side::Right),
        Self::Dart(Side::Left),
        Self::Dart(Side::Right),
        Self::Cross,
        Self::Widen,
        Self::Doze,
        Self::Signature,
        Self::Flourish(Side::Left),
        Self::Flourish(Side::Right),
        Self::Hop,
        Self::Wave(Side::Left),
        Self::Wave(Side::Right),
        Self::Cheer,
        Self::Patter,
        Self::Glow,
        Self::Glint(Side::Left),
        Self::Glint(Side::Right),
    ];

    /// This gesture the other way round — a sided one on its other side,
    /// any other unchanged. Every mascot is the same either side of its
    /// middle, so this is the gesture seen in a mirror.
    #[must_use]
    pub const fn mirrored(self) -> Self {
        match self {
            Self::Wink(side) => Self::Wink(side.other()),
            Self::Glance(side) => Self::Glance(side.other()),
            Self::Dart(side) => Self::Dart(side.other()),
            Self::Flourish(side) => Self::Flourish(side.other()),
            Self::Wave(side) => Self::Wave(side.other()),
            Self::Glint(side) => Self::Glint(side.other()),
            other => other,
        }
    }
}

/// What one eye is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Eye {
    Open,
    /// Half-lidded — only an eye in its cell's upper half has the glyph.
    Lid,
    Shut,
    Left,
    Right,
    Wide,
}

impl Eye {
    /// Both eyes doing this.
    const fn both(self) -> [Self; 2] {
        [self, self]
    }

    /// The glyph of an eye in this state whose open hole is drawn `open`: a
    /// hole in the cell's upper half (`▄`) or its lower half (`▀`). Any
    /// other glyph is not an eye and is left as it is, as is a state the
    /// block elements have no glyph for (a lower-half eye has no lid).
    const fn glyph(self, open: char) -> char {
        match (open, self) {
            ('▄' | '▀', Self::Shut) => '█',
            ('▄', Self::Lid) => '▆',
            ('▄', Self::Left) => '▟',
            ('▄', Self::Right) => '▙',
            ('▄', Self::Wide) => '▂',
            ('▀', Self::Left) => '▜',
            ('▀', Self::Right) => '▛',
            ('▀', Self::Wide) => '▔',
            _ => open,
        }
    }
}

/// What a mascot looks like at one instant: a frame of its art with its
/// eyes laid over it, drawn in the resting art's place ([`rows`](Self::rows)),
/// and the light on it — a passing glint, a glow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MascotPose {
    /// Whose pose this is — whose eyes the pose's are.
    mascot: Mascot,
    /// The body: the resting art's row count, none wider than its widest.
    art: Art,
    /// What each eye is doing, left then right, over the body's own eye
    /// cells — the gem's one facet is the first.
    eyes: [Eye; 2],
    /// The centre of a passing glint, in display columns across the art
    /// block (off either edge while it enters or leaves); `None` when no
    /// band of light is on the mascot.
    pub glint: Option<i8>,
    /// How strongly the whole mascot glows, in percent: 0 not at all, 100
    /// the light's full strength.
    pub glow: u8,
}

impl MascotPose {
    /// `mascot` at rest: its catalog art, eyes open, no light on it — what
    /// the banner is committed as, and what every act returns to.
    #[must_use]
    pub const fn rest(mascot: Mascot) -> Self {
        Self::drawn(mascot, mascot.art(), Eye::Open.both())
    }

    /// `mascot` with its body drawn as `art` and its eyes doing `eyes`.
    const fn drawn(mascot: Mascot, art: Art, eyes: [Eye; 2]) -> Self {
        Self {
            mascot,
            art,
            eyes,
            glint: None,
            glow: 0,
        }
    }

    /// The art rows to draw: the body with each eye in its state — the
    /// resting art's row count, none wider than its widest row.
    #[must_use]
    pub fn rows(&self) -> Vec<String> {
        let eyes = self.mascot.eye_cells();
        self.art
            .iter()
            .enumerate()
            .map(|(row, glyphs)| {
                // The art is single-width, so a row's nth char is its nth
                // column.
                glyphs
                    .chars()
                    .enumerate()
                    .map(|(col, glyph)| {
                        eyes.iter()
                            .position(|&cell| cell == (row, col))
                            .map_or(glyph, |eye| self.eyes[eye].glyph(glyph))
                    })
                    .collect()
            })
            .collect()
    }

    /// How strongly the pose's light falls on the glyph at display column
    /// `col` of art row `row`: 0 (not at all) to 1 (full strength). Zero
    /// for a blank cell — light shows on ink — and with no light on the
    /// mascot. How a lit glyph is coloured is the renderer's (`ui::header`).
    ///
    /// A **glow** lights every glyph alike. A **glint** is a band three
    /// cells wide on a row that **leans a column a row**, like a `/`:
    /// further right above the block's middle row, further left below it.
    /// That lean is what makes it read as light glancing off the mascot
    /// rather than a bar wiping across it.
    #[must_use]
    pub fn light(&self, col: usize, row: usize) -> f32 {
        // An eye is ink in every state, so the body's own glyphs say where
        // the ink is.
        let inked = self
            .art
            .get(row)
            .and_then(|glyphs| glyphs.chars().nth(col))
            .is_some_and(|glyph| glyph != ' ');
        if !inked {
            return 0.0;
        }
        let glow = f32::from(self.glow) / 100.0;
        let Some(centre) = self.glint else {
            return glow;
        };
        let lean = row as f32 - self.art.len().saturating_sub(1) as f32 / 2.0;
        let distance = (col as f32 + lean - f32::from(centre)).abs();
        glow.max(1.0 - distance / GLINT_HALF_WIDTH)
    }

    /// Whether the pose's light falls on any glyph of the art at all.
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

/// One drawn still of a body gesture: the art and its hold in milliseconds.
type Frame = (Art, u64);

// --- The eyes' timing, shared by every mascot. ---

/// An upper-half eye's blink: the lid up, shut, and down again.
const LID_MS: u64 = 45;
const SHUT_MS: u64 = 95;
/// A lower-half eye has no lid glyph, so its blink is the one cut.
const CUT_MS: u64 = 120;
/// A wink is meant: the eye is held shut far longer than a blink holds it.
const WINK_MS: u64 = 240;
const WINK_CUT_MS: u64 = 300;
/// A glance: each look held long enough to read, centre passed through.
const LOOK_MS: u64 = 460;
const CENTRE_MS: u64 = 80;
/// Darting eyes: quick enough to read as a flick, slow enough to be seen.
const DART_MS: u64 = 170;
/// Crossed and wide eyes are faces pulled, held to be seen.
const CROSS_MS: u64 = 560;
const WIDE_MS: u64 = 420;
/// A doze: the nod (lidded) or the sag (no lid to droop), the sleep — a
/// little longer where nothing led into it — and the wide-eyed start.
const NOD_MS: u64 = 420;
const SAG_MS: u64 = 320;
const SLEEP_MS: u64 = 1_100;
const SOUND_SLEEP_MS: u64 = 1_300;
const START_MS: u64 = 260;

/// A hop's one still: down (the sprout: up) and straight back.
const HOP_MS: u64 = 100;
/// A step of pattering feet.
const STEP_MS: u64 = 70;

// --- The bodies. Each mascot's resting rows come from the catalog
// (`Mascot::art`), so a frame can only ever differ from rest in the rows it
// redraws. A frame that moves the whole creature half a cell — a hop, the
// skitterer's scuttle — redraws every row: it is the resting art's half
// cells, each moved one over (`a_hop_moves_the_whole_mascot_half_a_cell`). ---

const CREST: Art = Mascot::Crest.art();
const BLOOM: Art = Mascot::Bloom.art();
const SPROUT: Art = Mascot::Sprout.art();
const TWIN: Art = Mascot::Twin.art();
const SKITER: Art = Mascot::Skiter.art();
const GEM: Art = Mascot::Gem.art();

/// The crest's frill: flat, and half up — the two middle spikes.
const CREST_FLAT: Art = &[" ▄▄▄▄▄▄▄", CREST[1], CREST[2]];
const CREST_HALF: Art = &[" ▄▄▙▄▟▄▄", CREST[1], CREST[2]];
/// The frill drops flat, then flares back up from the middle.
const CREST_FLARE: &[Frame] = &[(CREST_FLAT, 130), (CREST_HALF, 70)];
/// A ripple runs along the frill: each spike dips in turn, left to right.
const CREST_RIPPLE: &[Frame] = &[
    (&[" ▄▄▙▄▟▄▟", CREST[1], CREST[2]], 70),
    (&[" ▙▄▄▄▟▄▟", CREST[1], CREST[2]], 70),
    (&[" ▙▄▙▄▄▄▟", CREST[1], CREST[2]], 70),
    (&[" ▙▄▙▄▟▄▄", CREST[1], CREST[2]], 70),
];
const CREST_LOW: Art = &[" ▖ ▖ ▗ ▗", "▗█▀███▀█▖", " ▝▛▛▀▜▜▘"];
const CREST_HOP: &[Frame] = &[(CREST_LOW, HOP_MS)];
/// An arm leaves the body's side for the head's: half up, then high.
const CREST_RIGHT_UP: Art = &[" ▙▄▙▄▟▄▟▖", "▝▜▄███▄▛", CREST[2]];
const CREST_RIGHT_HIGH: Art = &[" ▙▄▙▄▟▄▟▘", "▝▜▄███▄▛", CREST[2]];
const CREST_LEFT_UP: Art = &["▗▙▄▙▄▟▄▟", " ▜▄███▄▛▘", CREST[2]];
const CREST_LEFT_HIGH: Art = &["▝▙▄▙▄▟▄▟", " ▜▄███▄▛▘", CREST[2]];
const CREST_BOTH_UP: Art = &["▗▙▄▙▄▟▄▟▖", " ▜▄███▄▛", CREST[2]];
const CREST_BOTH_HIGH: Art = &["▝▙▄▙▄▟▄▟▘", " ▜▄███▄▛", CREST[2]];
const CREST_WAVE_RIGHT: &[Frame] = &[
    (CREST_RIGHT_UP, 70),
    (CREST_RIGHT_HIGH, 120),
    (CREST_RIGHT_UP, 80),
    (CREST_RIGHT_HIGH, 120),
    (CREST_RIGHT_UP, 70),
];
const CREST_WAVE_LEFT: &[Frame] = &[
    (CREST_LEFT_UP, 70),
    (CREST_LEFT_HIGH, 120),
    (CREST_LEFT_UP, 80),
    (CREST_LEFT_HIGH, 120),
    (CREST_LEFT_UP, 70),
];
const CREST_CHEER: &[Frame] = &[
    (CREST_LOW, 90),
    (CREST_BOTH_HIGH, 160),
    (CREST_LOW, 90),
    (CREST_BOTH_HIGH, 160),
    (CREST_BOTH_UP, 70),
];
const CREST_STEP: Art = &[CREST[0], CREST[1], "  ▘  ▝"];
const CREST_OTHER_STEP: Art = &[CREST[0], CREST[1], "   ▘  ▝"];
const CREST_PATTER: &[Frame] = &[
    (CREST_STEP, STEP_MS),
    (CREST_OTHER_STEP, STEP_MS),
    (CREST_STEP, STEP_MS),
    (CREST_OTHER_STEP, STEP_MS),
    (CREST_STEP, STEP_MS),
    (CREST_OTHER_STEP, STEP_MS),
];

/// The head swells to the block's full width and eases back.
const BLOOM_SWELL: &[Frame] = &[
    (&["▗▄█████▄▖", BLOOM[1], BLOOM[2]], 80),
    (&["▄███████▄", BLOOM[1], BLOOM[2]], 220),
    (&["▗▄█████▄▖", BLOOM[1], BLOOM[2]], 80),
];
/// The crown of the head, half a cell to one side.
const BLOOM_TILT_LEFT: Art = &[" ▟████▙▄", BLOOM[1], BLOOM[2]];
const BLOOM_TILT_RIGHT: Art = &[" ▄▟████▙", BLOOM[1], BLOOM[2]];
const BLOOM_WOBBLE_LEFT: &[Frame] = &[
    (BLOOM_TILT_LEFT, 150),
    (BLOOM, 60),
    (BLOOM_TILT_RIGHT, 150),
    (BLOOM, 60),
    (BLOOM_TILT_LEFT, 150),
];
const BLOOM_WOBBLE_RIGHT: &[Frame] = &[
    (BLOOM_TILT_RIGHT, 150),
    (BLOOM, 60),
    (BLOOM_TILT_LEFT, 150),
    (BLOOM, 60),
    (BLOOM_TILT_RIGHT, 150),
];
const BLOOM_LOW: Art = &["  ▄▄▄▄▄", "▄█▀███▀█▄", " ▜▜▀▀▀▛▛"];
const BLOOM_HOP: &[Frame] = &[(BLOOM_LOW, HOP_MS)];
/// Its two arms, up at the head and down at the hem.
const BLOOM_ARMS_UP: Art = &["▄▄█████▄▄", " █▄███▄█", BLOOM[2]];
const BLOOM_ARMS_DOWN: Art = &[BLOOM[0], "▄█▄███▄█▄", BLOOM[2]];
const BLOOM_FLAP: &[Frame] = &[
    (BLOOM_ARMS_UP, 80),
    (BLOOM, 50),
    (BLOOM_ARMS_DOWN, 80),
    (BLOOM, 50),
    (BLOOM_ARMS_UP, 80),
    (BLOOM, 50),
    (BLOOM_ARMS_DOWN, 80),
];
const BLOOM_CHEER: &[Frame] = &[
    (BLOOM_LOW, 90),
    (BLOOM_ARMS_UP, 160),
    (BLOOM_LOW, 90),
    (BLOOM_ARMS_UP, 160),
];
const BLOOM_STEP: Art = &[BLOOM[0], BLOOM[1], " ▝    ▘"];
const BLOOM_OTHER_STEP: Art = &[BLOOM[0], BLOOM[1], "  ▝    ▘"];
const BLOOM_PATTER: &[Frame] = &[
    (BLOOM_STEP, STEP_MS),
    (BLOOM_OTHER_STEP, STEP_MS),
    (BLOOM_STEP, STEP_MS),
    (BLOOM_OTHER_STEP, STEP_MS),
    (BLOOM_STEP, STEP_MS),
    (BLOOM_OTHER_STEP, STEP_MS),
];

/// The leaf's tip, leaning.
const SPROUT_LEAN_LEFT: Art = &["   ▝▙", SPROUT[1], SPROUT[2]];
const SPROUT_LEAN_RIGHT: Art = &["    ▟▘", SPROUT[1], SPROUT[2]];
/// The leaf's tip leans left, stands, and leans right.
const SPROUT_SWAY: &[Frame] = &[
    (SPROUT_LEAN_LEFT, 140),
    (SPROUT, 80),
    (SPROUT_LEAN_RIGHT, 140),
];
/// Two leaves open from the tip, are held spread, and fold away again.
const SPROUT_UNFURL: &[Frame] = &[
    (&["   ▝█▘", SPROUT[1], SPROUT[2]], 90),
    (&["   ▀█▀", SPROUT[1], SPROUT[2]], 90),
    (&["  ▝▀█▀▘", SPROUT[1], SPROUT[2]], 520),
    (&["   ▀█▀", SPROUT[1], SPROUT[2]], 90),
    (&["   ▝█▘", SPROUT[1], SPROUT[2]], 90),
];
/// The sprout stands on the block's floor, so its hop is up: the leaf's tip
/// leaves the block for the moment.
const SPROUT_HIGH: Art = &[" ▗▄▄█▄▄▖", "▝▜▄███▄▛▘", "  ▘▘ ▝▝"];
const SPROUT_HOP: &[Frame] = &[(SPROUT_HIGH, HOP_MS)];
/// An arm lifted from the waist to the shoulder.
const SPROUT_RIGHT_UP: Art = &[SPROUT[0], "▗▟▀███▀█▘", SPROUT[2]];
const SPROUT_LEFT_UP: Art = &[SPROUT[0], "▝█▀███▀▙▖", SPROUT[2]];
const SPROUT_WAVE_RIGHT: &[Frame] = &[
    (SPROUT_RIGHT_UP, 110),
    (SPROUT, 80),
    (SPROUT_RIGHT_UP, 110),
    (SPROUT, 80),
    (SPROUT_RIGHT_UP, 110),
];
const SPROUT_WAVE_LEFT: &[Frame] = &[
    (SPROUT_LEFT_UP, 110),
    (SPROUT, 80),
    (SPROUT_LEFT_UP, 110),
    (SPROUT, 80),
    (SPROUT_LEFT_UP, 110),
];
const SPROUT_STEP: Art = &[SPROUT[0], SPROUT[1], " ▝▛▀▀▜▀▘"];
const SPROUT_OTHER_STEP: Art = &[SPROUT[0], SPROUT[1], " ▝▀▛▀▀▜▘"];
const SPROUT_PATTER: &[Frame] = &[
    (SPROUT_STEP, STEP_MS),
    (SPROUT_OTHER_STEP, STEP_MS),
    (SPROUT_STEP, STEP_MS),
    (SPROUT_OTHER_STEP, STEP_MS),
    (SPROUT_STEP, STEP_MS),
    (SPROUT_OTHER_STEP, STEP_MS),
];

/// One of the twin's ears, flat.
const TWIN_LEFT_FLAT: Art = &[" ▄▄▄▄▄██", TWIN[1], TWIN[2]];
const TWIN_RIGHT_FLAT: Art = &[" ██▄▄▄▄▄", TWIN[1], TWIN[2]];
/// One ear flicks flat, then the other.
const TWIN_FLICK: &[Frame] = &[(TWIN_LEFT_FLAT, 90), (TWIN, 70), (TWIN_RIGHT_FLAT, 90)];
/// Both ears splayed to the block's corners.
const TWIN_SPLAYED: Art = &["▐█▄▄▄▄▄█▌", TWIN[1], TWIN[2]];
const TWIN_WIGGLE: &[Frame] = &[(TWIN_SPLAYED, 90), (TWIN, 70), (TWIN_SPLAYED, 90)];
const TWIN_LOW: Art = &[" ▄▄   ▄▄", "▗█▀███▀█▖", " ▝▛▛▀▜▜▘"];
const TWIN_HOP: &[Frame] = &[(TWIN_LOW, HOP_MS)];
const TWIN_CHEER: &[Frame] = &[
    (TWIN_LOW, 90),
    (TWIN_SPLAYED, 160),
    (TWIN_LOW, 90),
    (TWIN_SPLAYED, 160),
];
const TWIN_STEP: Art = &[TWIN[0], TWIN[1], "  ▘  ▝"];
const TWIN_OTHER_STEP: Art = &[TWIN[0], TWIN[1], "   ▘  ▝"];
const TWIN_PATTER: &[Frame] = &[
    (TWIN_STEP, STEP_MS),
    (TWIN_OTHER_STEP, STEP_MS),
    (TWIN_STEP, STEP_MS),
    (TWIN_OTHER_STEP, STEP_MS),
    (TWIN_STEP, STEP_MS),
    (TWIN_OTHER_STEP, STEP_MS),
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
/// The whole skitterer, half a cell to one side.
const SKITER_LEFT: Art = &["▄▛▜██▛▜▄", "▀██████▀", "▝▝ ▘▝ ▘▘"];
const SKITER_RIGHT: Art = &[" ▄▛▜██▛▜▄", " ▀██████▀", " ▝▝ ▘▝ ▘▘"];
const SKITER_SCUTTLE_LEFT: &[Frame] = &[
    (SKITER_LEFT, 100),
    (SKITER, 60),
    (SKITER_RIGHT, 100),
    (SKITER, 60),
    (SKITER_LEFT, 100),
];
const SKITER_SCUTTLE_RIGHT: &[Frame] = &[
    (SKITER_RIGHT, 100),
    (SKITER, 60),
    (SKITER_LEFT, 100),
    (SKITER, 60),
    (SKITER_RIGHT, 100),
];
const SKITER_LOW: Art = &[" ▗▄▄▄▄▄▖", "▐█▄███▄█▌", " ▞▛▜▀▛▜▚"];
const SKITER_HOP: &[Frame] = &[(SKITER_LOW, HOP_MS)];
/// Both claws raised from the body's sides to its shoulders.
const SKITER_CLAWS_UP: Art = &["▐▟▀███▀▙▌", " ▜█████▛", SKITER[2]];
const SKITER_CLAWS: &[Frame] = &[(SKITER_CLAWS_UP, 130), (SKITER, 80), (SKITER_CLAWS_UP, 130)];
const SKITER_CHEER: &[Frame] = &[
    (SKITER_LOW, 90),
    (SKITER_CLAWS_UP, 160),
    (SKITER_LOW, 90),
    (SKITER_CLAWS_UP, 160),
];

/// The gem's facet slides a cell each way as the gem turns in the light.
const GEM_TURN: &[Frame] = &[
    (&[GEM[0], "▝▀█▄███▀▘", GEM[2]], 200),
    (GEM, CENTRE_MS),
    (&[GEM[0], "▝▀███▄█▀▘", GEM[2]], 200),
];
/// The gem turning away: narrower, thin, and edge-on — its facet out of
/// sight from the moment it thins.
const GEM_NARROW: Art = &["  ▗▟█▙▖", " ▝▜█▄█▛▘", GEM[2]];
const GEM_THIN: Art = &["   ▗█▖", "   ▜█▛", "    ▀"];
const GEM_EDGE: Art = &["    █", "    █", "    ▀"];
/// Its flourish — it spins: round to edge-on and back the way it went.
const GEM_SPIN: &[Frame] = &[
    (GEM_NARROW, 90),
    (GEM_THIN, 80),
    (GEM_EDGE, 70),
    (GEM_THIN, 80),
    (GEM_NARROW, 90),
];
const GEM_LOW: Art = &["   ▗▄▖", "▗▄██▀██▄▖", "  ▀▜█▛▀"];
/// A gem does not hop; it hangs in the air, dips, and dips again.
const GEM_FLOAT: &[Frame] = &[(GEM_LOW, 280), (GEM, 240), (GEM_LOW, 280)];

// --- The light. Each step is a glow level in percent and its hold. ---

/// A glow swells over the whole mascot and fades the way it came.
const GLOW: &[(u8, u64)] = &[
    (15, 60),
    (30, 60),
    (45, 60),
    (60, 60),
    (45, 60),
    (30, 60),
    (15, 60),
];
/// The gem's signature: it catches the light — a flash to full strength,
/// dark, and a flash again.
const FLASH: &[(u8, u64)] = &[
    (50, 40),
    (100, 70),
    (50, 40),
    (0, 110),
    (50, 40),
    (100, 70),
    (50, 40),
];

impl Mascot {
    /// Where this mascot's eyes are, as (art row, column), left then right —
    /// the cells its resting art punches a half-cell hole in. The gem has no
    /// eyes: its one centre facet stands in for one.
    const fn eye_cells(self) -> &'static [(usize, usize)] {
        match self {
            Self::Crest | Self::Bloom | Self::Sprout | Self::Twin => &[(1, 2), (1, 6)],
            Self::Skiter => &[(0, 2), (0, 6)],
            Self::Gem => &[(1, 4)],
        }
    }

    /// Whether this mascot has a pair of eyes — to wink one of, to cross.
    const fn has_two_eyes(self) -> bool {
        self.eye_cells().len() == 2
    }

    /// Whether its eyes can be half-lidded: an eye in its cell's upper half
    /// closes from below through `▆`; one in the lower half would close from
    /// above, and the block elements have no glyph for that.
    const fn has_lids(self) -> bool {
        !matches!(self, Self::Sprout | Self::Skiter)
    }

    /// The body it sleeps in ([`Gesture::Doze`]): the crest's frill half
    /// down, the bloom's head to one side, the sprout's leaf bent, one of
    /// the twin's ears flat. The skitterer sleeps as it stands.
    const fn asleep(self) -> Art {
        match self {
            Self::Crest => CREST_HALF,
            Self::Bloom => BLOOM_TILT_LEFT,
            Self::Sprout => SPROUT_LEAN_LEFT,
            Self::Twin => TWIN_LEFT_FLAT,
            Self::Skiter | Self::Gem => self.art(),
        }
    }

    /// The stills of `gesture` as this mascot makes it, in order — none at
    /// all for a gesture its body has no way to make. A gesture starts from
    /// the resting art and the schedule returns to it after, so the stills
    /// hold only the moves themselves.
    #[must_use]
    pub fn stills(self, gesture: Gesture) -> Vec<Still> {
        match gesture {
            Gesture::Glint(side) => self.glint_stills(side),
            Gesture::Glow => self.light_stills(GLOW),
            // What a cut gem does is catch the light: its signature is that.
            Gesture::Signature if matches!(self, Self::Gem) => self.light_stills(FLASH),
            // The gem has no eyes to look about with: its facet slides.
            Gesture::Glance(side) if !self.has_two_eyes() => {
                self.body_stills(GEM_TURN, matches!(side, Side::Right))
            }
            Gesture::Blink
            | Gesture::Wink(_)
            | Gesture::Glance(_)
            | Gesture::Dart(_)
            | Gesture::Cross
            | Gesture::Widen
            | Gesture::Doze => self.look_stills(gesture),
            Gesture::Signature
            | Gesture::Flourish(_)
            | Gesture::Hop
            | Gesture::Wave(_)
            | Gesture::Cheer
            | Gesture::Patter => {
                let (frames, backwards) = self.body_frames(gesture);
                self.body_stills(frames, backwards)
            }
        }
    }

    /// The frames of a move of the body as this mascot makes it, and whether
    /// they play backwards — how a move that runs across the mascot is made
    /// the other way. No frames for a move this body cannot make.
    const fn body_frames(self, gesture: Gesture) -> (&'static [Frame], bool) {
        let frames: &'static [Frame] = match (self, gesture) {
            (Self::Crest, Gesture::Signature) => CREST_FLARE,
            (Self::Crest, Gesture::Flourish(side)) => {
                return (CREST_RIPPLE, matches!(side, Side::Right));
            }
            (Self::Crest, Gesture::Hop) => CREST_HOP,
            (Self::Crest, Gesture::Wave(Side::Left)) => CREST_WAVE_LEFT,
            (Self::Crest, Gesture::Wave(Side::Right)) => CREST_WAVE_RIGHT,
            (Self::Crest, Gesture::Cheer) => CREST_CHEER,
            (Self::Crest, Gesture::Patter) => CREST_PATTER,
            (Self::Bloom, Gesture::Signature) => BLOOM_SWELL,
            (Self::Bloom, Gesture::Flourish(Side::Left)) => BLOOM_WOBBLE_LEFT,
            (Self::Bloom, Gesture::Flourish(Side::Right)) => BLOOM_WOBBLE_RIGHT,
            (Self::Bloom, Gesture::Hop) => BLOOM_HOP,
            (Self::Bloom, Gesture::Wave(_)) => BLOOM_FLAP,
            (Self::Bloom, Gesture::Cheer) => BLOOM_CHEER,
            (Self::Bloom, Gesture::Patter) => BLOOM_PATTER,
            (Self::Sprout, Gesture::Signature) => SPROUT_SWAY,
            (Self::Sprout, Gesture::Flourish(_)) => SPROUT_UNFURL,
            (Self::Sprout, Gesture::Hop) => SPROUT_HOP,
            (Self::Sprout, Gesture::Wave(Side::Left)) => SPROUT_WAVE_LEFT,
            (Self::Sprout, Gesture::Wave(Side::Right)) => SPROUT_WAVE_RIGHT,
            (Self::Sprout, Gesture::Patter) => SPROUT_PATTER,
            (Self::Twin, Gesture::Signature) => TWIN_FLICK,
            (Self::Twin, Gesture::Flourish(_)) => TWIN_WIGGLE,
            (Self::Twin, Gesture::Hop) => TWIN_HOP,
            (Self::Twin, Gesture::Cheer) => TWIN_CHEER,
            (Self::Twin, Gesture::Patter) => TWIN_PATTER,
            (Self::Skiter, Gesture::Signature) => SKITER_SKITTER,
            (Self::Skiter, Gesture::Flourish(Side::Left)) => SKITER_SCUTTLE_LEFT,
            (Self::Skiter, Gesture::Flourish(Side::Right)) => SKITER_SCUTTLE_RIGHT,
            (Self::Skiter, Gesture::Hop) => SKITER_HOP,
            (Self::Skiter, Gesture::Wave(_)) => SKITER_CLAWS,
            (Self::Skiter, Gesture::Cheer) => SKITER_CHEER,
            (Self::Gem, Gesture::Flourish(_)) => GEM_SPIN,
            (Self::Gem, Gesture::Hop) => GEM_FLOAT,
            _ => &[],
        };
        (frames, false)
    }

    /// `frames` as stills, eyes as the art draws them — `backwards`, last
    /// frame first.
    fn body_stills(self, frames: &[Frame], backwards: bool) -> Vec<Still> {
        let mut stills: Vec<Still> = frames
            .iter()
            .map(|&(art, ms)| Still {
                pose: MascotPose::drawn(self, art, Eye::Open.both()),
                hold: Duration::from_millis(ms),
            })
            .collect();
        if backwards {
            stills.reverse();
        }
        stills
    }

    /// The stills of a look: the eyes' states laid over the resting art —
    /// one choreography for every mascot, the glyphs each eye's own
    /// ([`Eye::glyph`]). A doze alone moves the body too, which sags while
    /// it sleeps. Nothing for a look that takes two eyes from a mascot with
    /// one.
    fn look_stills(self, gesture: Gesture) -> Vec<Still> {
        use Eye::{Lid, Open, Shut, Wide};
        let rest = self.art();
        let two = self.has_two_eyes();
        let steps: Vec<(Art, [Eye; 2], u64)> = match gesture {
            Gesture::Blink if self.has_lids() => vec![
                (rest, Lid.both(), LID_MS),
                (rest, Shut.both(), SHUT_MS),
                (rest, Lid.both(), LID_MS),
            ],
            Gesture::Blink => vec![(rest, Shut.both(), CUT_MS)],
            Gesture::Wink(side) if two => {
                let one = |eye: Eye| match side {
                    Side::Left => [eye, Open],
                    Side::Right => [Open, eye],
                };
                if self.has_lids() {
                    vec![
                        (rest, one(Lid), LID_MS),
                        (rest, one(Shut), WINK_MS),
                        (rest, one(Lid), LID_MS),
                    ]
                } else {
                    vec![(rest, one(Shut), WINK_CUT_MS)]
                }
            }
            Gesture::Glance(side) if two => vec![
                (rest, side.look().both(), LOOK_MS),
                (rest, Open.both(), CENTRE_MS),
                (rest, side.other().look().both(), LOOK_MS),
            ],
            Gesture::Dart(side) => vec![
                (rest, side.look().both(), DART_MS),
                (rest, side.other().look().both(), DART_MS),
                (rest, side.look().both(), DART_MS),
                (rest, side.other().look().both(), DART_MS),
            ],
            Gesture::Cross if two => vec![(rest, [Eye::Right, Eye::Left], CROSS_MS)],
            Gesture::Widen => vec![(rest, Wide.both(), WIDE_MS)],
            Gesture::Doze if two => {
                let asleep = self.asleep();
                if self.has_lids() {
                    vec![
                        (asleep, Lid.both(), NOD_MS),
                        (asleep, Shut.both(), SLEEP_MS),
                        (rest, Wide.both(), START_MS),
                    ]
                } else if asleep == rest {
                    vec![
                        (rest, Shut.both(), SOUND_SLEEP_MS),
                        (rest, Wide.both(), START_MS),
                    ]
                } else {
                    vec![
                        (asleep, Open.both(), SAG_MS),
                        (asleep, Shut.both(), SLEEP_MS),
                        (rest, Wide.both(), START_MS),
                    ]
                }
            }
            _ => Vec::new(),
        };
        steps
            .into_iter()
            .map(|(art, eyes, ms)| Still {
                pose: MascotPose::drawn(self, art, eyes),
                hold: Duration::from_millis(ms),
            })
            .collect()
    }

    /// A glint's stills over the resting art: the band's centre stepping one
    /// column a still across every position that lights a glyph
    /// ([`MascotPose::light`]) — from the first to light one to the last, so
    /// no still is a frame drawn for nothing. It enters from `side`: from
    /// the left it travels right, from the right the same sweep backwards.
    fn glint_stills(self, side: Side) -> Vec<Still> {
        let lit = |centre: i8| MascotPose {
            glint: Some(centre),
            ..MascotPose::rest(self)
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
        if matches!(side, Side::Right) {
            stills.reverse();
        }
        stills
    }

    /// The resting art glowing through `levels` — each a strength in percent
    /// and its hold in milliseconds, 0 the mascot dark between two flashes.
    fn light_stills(self, levels: &[(u8, u64)]) -> Vec<Still> {
        levels
            .iter()
            .map(|&(glow, ms)| Still {
                pose: MascotPose {
                    glow,
                    ..MascotPose::rest(self)
                },
                hold: Duration::from_millis(ms),
            })
            .collect()
    }
}

/// One beat of an act: a gesture, or a rest of so many milliseconds between
/// two of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Beat {
    Do(Gesture),
    Rest(u64),
}

/// One thing a mascot does in a bar: a gesture, or a short phrase of them.
/// A sided gesture is written one way round and played either
/// ([`Gesture::mirrored`]) on the flip of a coin.
type Act = &'static [Beat];

/// Bar 0's act: a glint crosses the mascot, it makes its signature move, and
/// it blinks — about two seconds of hello, the same at every launch.
pub(super) const GREETING: Act = &[
    Beat::Do(Gesture::Glint(Side::Left)),
    Beat::Rest(240),
    Beat::Do(Gesture::Signature),
    Beat::Rest(420),
    Beat::Do(Gesture::Blink),
];

const BLINK: Beat = Beat::Do(Gesture::Blink);
const HOP_TWICE: Act = &[
    Beat::Do(Gesture::Hop),
    Beat::Rest(110),
    Beat::Do(Gesture::Hop),
];
const CHEER: Act = &[Beat::Do(Gesture::Cheer), Beat::Rest(300), BLINK];
/// A wave from an arm, and a wink from the eye beside it.
const WAVE_AND_WINK: Act = &[
    Beat::Do(Gesture::Wave(Side::Right)),
    Beat::Rest(180),
    Beat::Do(Gesture::Wink(Side::Right)),
];
/// A wave of both arms has no side to wink on.
const WAVE: Act = &[Beat::Do(Gesture::Wave(Side::Right)), Beat::Rest(200), BLINK];
const SIGNATURE: Act = &[Beat::Do(Gesture::Signature)];
const FLOURISH: Act = &[Beat::Do(Gesture::Flourish(Side::Left))];
const PATTER: Act = &[Beat::Do(Gesture::Patter)];
const GLOW_ACT: Act = &[Beat::Do(Gesture::Glow)];
const GLINT: Act = &[Beat::Do(Gesture::Glint(Side::Left))];

/// The looks of a mascot with two eyes: what its face does while its body
/// is still. Only the first is a bare blink — the rest of its blinking is
/// punctuation, the close of a pulled face, of a doze, of a cheer.
const LOOKS: &[Act] = &[
    &[BLINK],
    &[Beat::Do(Gesture::Glance(Side::Left))],
    &[Beat::Do(Gesture::Wink(Side::Left))],
    &[Beat::Do(Gesture::Dart(Side::Left))],
    &[Beat::Do(Gesture::Cross), Beat::Rest(220), BLINK],
    &[Beat::Do(Gesture::Widen), Beat::Rest(180), BLINK],
    // Started awake, it blinks the sleep out of its eyes — twice.
    &[
        Beat::Do(Gesture::Doze),
        Beat::Rest(260),
        BLINK,
        Beat::Rest(150),
        BLINK,
    ],
];

/// The gem's looks: all its one facet can do.
const GEM_LOOKS: &[Act] = &[
    &[BLINK],
    &[Beat::Do(Gesture::Glance(Side::Left))],
    &[Beat::Do(Gesture::Dart(Side::Left))],
    &[Beat::Do(Gesture::Widen)],
];

const CREST_MOVES: &[Act] = &[
    SIGNATURE,
    FLOURISH,
    HOP_TWICE,
    CHEER,
    WAVE_AND_WINK,
    PATTER,
    GLOW_ACT,
    GLINT,
];
const BLOOM_MOVES: &[Act] = &[
    SIGNATURE, FLOURISH, HOP_TWICE, CHEER, WAVE, PATTER, GLOW_ACT, GLINT,
];
const SPROUT_MOVES: &[Act] = &[
    SIGNATURE,
    FLOURISH,
    HOP_TWICE,
    WAVE_AND_WINK,
    PATTER,
    GLOW_ACT,
    GLINT,
];
const TWIN_MOVES: &[Act] = &[
    SIGNATURE, FLOURISH, HOP_TWICE, CHEER, PATTER, GLOW_ACT, GLINT,
];
const SKITER_MOVES: &[Act] = &[SIGNATURE, FLOURISH, HOP_TWICE, CHEER, WAVE, GLOW_ACT, GLINT];
const GEM_MOVES: &[Act] = &[
    SIGNATURE,
    FLOURISH,
    &[Beat::Do(Gesture::Hop)],
    GLOW_ACT,
    GLINT,
];

impl Mascot {
    /// What this mascot's body does between looks: its signature, its
    /// flourish, a hop, and whatever else it has the limbs for — and the
    /// light on it. Every act here moves more than the eyes. The signature
    /// leads the list and the glint closes it: the two the greeting has
    /// just shown, which the first deal does not open on ([`greeted`]).
    pub(super) const fn moves(self) -> &'static [Act] {
        match self {
            Self::Crest => CREST_MOVES,
            Self::Bloom => BLOOM_MOVES,
            Self::Sprout => SPROUT_MOVES,
            Self::Twin => TWIN_MOVES,
            Self::Skiter => SKITER_MOVES,
            Self::Gem => GEM_MOVES,
        }
    }

    /// What this mascot's eyes do between moves. The bare blink leads the
    /// list: it is what the greeting has just closed on ([`greeted`]).
    pub(super) const fn looks(self) -> &'static [Act] {
        if self.has_two_eyes() {
            LOOKS
        } else {
            GEM_LOOKS
        }
    }
}

/// The two kinds of act the schedule alternates between
/// ([`Mascot::moves`], [`Mascot::looks`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    Move,
    Look,
}

/// A well-mixed 64 bits for bar `bar` of the schedule `seed` names
/// (splitmix64's finaliser over the pair) — what scatters the acts' starts
/// and deals their order, with no state to carry from one bar to the next.
const fn mix(seed: u64, bar: u64) -> u64 {
    let mut z = seed ^ bar.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// The order cycle `cycle` deals `count` acts of `kind` in: a shuffle
/// (Fisher–Yates) drawn from the seed, the kind and the cycle alone.
fn shuffled(seed: u64, kind: Kind, cycle: u64, count: usize) -> Vec<usize> {
    // Moves and looks are dealt from two decks; a salt keeps one seed from
    // shuffling both alike.
    let salt = match kind {
        Kind::Move => 0x6D6F_7665,
        Kind::Look => 0x6C6F_6F6B,
    };
    let key = mix(seed ^ salt, cycle);
    let mut order: Vec<usize> = (0..count).collect();
    for slot in (1..count).rev() {
        let pick = (mix(key, slot as u64) % (slot as u64 + 1)) as usize;
        order.swap(slot, pick);
    }
    order
}

/// The acts of `kind` the greeting has just shown, as places in a list of
/// `count`: it crossed the mascot with a glint, made the signature, and
/// closed on a blink. The signature leads every mascot's moves and the glint
/// closes them; the bare blink leads its looks.
const fn greeted(kind: Kind, count: usize) -> [usize; 2] {
    match kind {
        Kind::Move => [0, count.saturating_sub(1)],
        Kind::Look => [0, 0],
    }
}

/// `order` opening on its first act that is not `barred` — brought forward
/// in exchange for the act that led.
fn opening(mut order: Vec<usize>, barred: &[usize]) -> Vec<usize> {
    if let Some(open) = order.iter().position(|act| !barred.contains(act)) {
        order.swap(0, open);
    }
    order
}

/// Which of `count` acts of `kind` the schedule `seed` names plays on its
/// `turn`th turn of that kind, counted from the greeting.
///
/// The acts are dealt like a deck: every cycle of `count` turns is a fresh
/// shuffle that plays each act once, so a mascot goes through everything it
/// can do before any of it comes round again, and no act plays twice in a
/// row — within a cycle because each is dealt once, and across the join
/// between two because a cycle never opens on the act the last one closed
/// on: the first act it may open on is brought forward instead.
///
/// The first cycle has a join too, onto the **greeting**, and opens on
/// nothing the greeting has just shown ([`greeted`]) — so what follows the
/// greeting is always something new.
pub(super) fn dealt(seed: u64, kind: Kind, turn: u64, count: usize) -> usize {
    let turns = (count as u64).max(1);
    let (cycle, slot) = (turn / turns, (turn % turns) as usize);
    let first = || opening(shuffled(seed, kind, 0, count), &greeted(kind, count));
    let order = match cycle {
        0 => first(),
        _ => {
            // Bringing an act forward moves one of a cycle's first two
            // (three, in the first), so from the second cycle on its last
            // act is its shuffle's own; the first cycle is dealt whole.
            let last = match cycle {
                1 => first(),
                _ => shuffled(seed, kind, cycle - 1, count),
            }
            .pop();
            opening(shuffled(seed, kind, cycle, count), last.as_slice())
        }
    };
    order.get(slot).copied().unwrap_or(0)
}

/// What bar `bar` plays, and whether its sided gestures are flipped. Bar 0
/// is the greeting; after it the bars take turns, a move then a look, each
/// the next act off its deck ([`dealt`]).
fn act_of(mascot: Mascot, seed: u64, bar: u64) -> (Act, bool) {
    if bar == 0 {
        return (GREETING, false);
    }
    let (kind, turn, acts) = if bar % 2 == 1 {
        (Kind::Move, (bar - 1) / 2, mascot.moves())
    } else {
        (Kind::Look, bar / 2 - 1, mascot.looks())
    };
    let act = acts[dealt(seed, kind, turn, acts.len())];
    // A high bit of the bar's own hash; its low bits scatter the start.
    (act, (mix(seed, bar) >> 47) & 1 == 1)
}

/// Bar `bar` as spans — each the millisecond it ends on (within the bar) and
/// its pose — covering the whole bar, equal neighbours folded into one. It
/// opens and closes at rest.
fn bar_spans(mascot: Mascot, seed: u64, bar: u64) -> Vec<(u64, MascotPose)> {
    let rest = MascotPose::rest(mascot);
    let (act, flipped) = act_of(mascot, seed, bar);
    // The act as the poses it goes through and how long each is held.
    let mut steps: Vec<(MascotPose, u64)> = Vec::new();
    for beat in act {
        match *beat {
            Beat::Rest(ms) => steps.push((rest, ms)),
            Beat::Do(gesture) => {
                let gesture = if flipped { gesture.mirrored() } else { gesture };
                steps.extend(mascot.stills(gesture).into_iter().map(|still| {
                    let ms = u64::try_from(still.hold.as_millis()).unwrap_or(u64::MAX);
                    (still.pose, ms)
                }));
            }
        }
    }
    let length: u64 = steps.iter().map(|&(_, ms)| ms).sum();
    let start = if bar == 0 {
        GREETING_MS
    } else {
        // Scattered over what the act leaves of its bar: a long one cannot
        // start as late as a blink can and still be over in time.
        let latest = ACT_LATEST_MS
            .min((BAR_MS - ACT_CLEAR_MS).saturating_sub(length))
            .max(ACT_EARLIEST_MS);
        ACT_EARLIEST_MS + mix(seed, bar) % (latest - ACT_EARLIEST_MS + 1)
    };
    debug_assert!(
        start + length <= BAR_MS - ACT_CLEAR_MS,
        "an act outruns its bar"
    );
    let mut spans: Vec<(u64, MascotPose)> = Vec::with_capacity(steps.len() + 2);
    let mut push = |end: u64, pose: MascotPose| match spans.last_mut() {
        Some((last_end, last)) if *last == pose => *last_end = end,
        _ => spans.push((end, pose)),
    };
    let mut at = start;
    push(at, rest);
    for (pose, ms) in steps {
        at += ms;
        push(at, pose);
    }
    push(BAR_MS, rest);
    spans
}

/// Where `mascot`'s motion stands `elapsed` after its banner appeared, on
/// the schedule `seed` names: the pose to draw and how long it holds
/// (`docs/mascot-motion.md`).
///
/// Time is cut into bars of [`MOTION_BAR`]. Bar 0 plays the greeting
/// [`MOTION_GREETING_DELAY`] in; every later bar starts one act between
/// [`MOTION_ACT_EARLIEST`] and [`MOTION_ACT_LATEST`] in — a move and a look
/// by turns, each kind dealt in a freshly shuffled order every time the
/// mascot has been through all of it, the start and the side of a sided
/// gesture hashed from the seed and the bar's index. A bar opens and closes
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
    /// two sessions do not move in step. Unseeded (every unit test) it is
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
