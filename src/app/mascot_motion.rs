//! The banner mascot's idle animation: what each mascot does while it has
//! nothing to do, and when. See `docs/mascot-animation.md`.
//!
//! A mascot is three rows of block glyphs ([`Mascot::art`]). Its animation is
//! a set of **frames** — the same three rows with one part moved (the eyes
//! shut, a crest flared, a leaf unfurled, an ear folded, a foot lifted, a
//! spark opened), or the whole mascot moved half a cell (a dip, a lean) —
//! played as short **clips** on a **timeline** that is mostly stillness.
//! Every mascot has a **repertoire** of some twenty clips: the looks its
//! eyes give (a blink, a glance, a wink, a stare), the things its whole body
//! does (a bounce, a wiggle, a little dance, a peek to one side), the moves
//! that are its own (the thing its catalog description says it does, and a
//! few more), a sweep of light, and — for the four that can — a doze. The
//! gem, which has no eyes, twinkles at its four corners and turns its facet
//! in the light instead.
//!
//! The timeline is a **pure function of a seed and a clock** — no state, no
//! thread, nothing to advance: [`Mascot::pose`] says what the mascot looks
//! like at an instant and [`Mascot::pose_holds`] how long that lasts, which
//! is all the loop needs to draw a frame and sleep until the next one. It
//! opens on a scripted **greeting** (a light sweeps the mascot, it shows its
//! move, it blinks) and then runs in **beats** of [`BEAT_MS`], at most one
//! clip to a beat, started at a drawn offset. What a beat plays is **dealt,
//! not rolled**: the repertoire is a deck, shuffled afresh for every
//! **round** of some thirty beats and dealt a card a beat, so a minute and a
//! half shows everything the mascot knows exactly as often as the deck holds
//! it, never the same move twice running, and never two rests in a row. Two
//! sessions never move in step, and the pose three hours in costs the same
//! to compute as the pose three seconds in.
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
/// dealt from the deck begin.
pub const MASCOT_GREETING_MS: u64 = 3_600;

/// One beat of the timeline, in milliseconds: at most one clip starts in
/// each. Three seconds gives a move every three and a half on average — the
/// pace of something calm and awake.
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
/// Each dart of a pair of shifty eyes.
const DART_MS: u64 = 170;
/// One eye shut — longer than a blink, so it reads as meant.
const WINK_MS: u64 = 320;
/// Eyes held wide, half shut, one of each, and crossed.
const WIDE_MS: u64 = 520;
const SQUINT_MS: u64 = 760;
const HUH_MS: u64 = 680;
const CROSS_MS: u64 = 560;
/// One sweep of light across the mascot — nineteen [`MASCOT_SHINE_FRAME`]s.
const SWEEP_MS: u64 = 760;
/// The three stills of a spark opening (and, played back, closing).
const SPARK_MS: u64 = 70;
const SPARK_OPEN_MS: u64 = 120;

// --- The whole mascot, moved half a cell and back. ---

/// A dip, and the stand between two of them: a bounce lands and is off
/// again.
const DIP_MS: u64 = 110;
const BOUNCE_APART_MS: u64 = 90;
/// Each side of a wiggle — straight from one lean into the other.
const STEP_MS: u64 = 80;
/// Each swing of a dance, and the stand between two.
const SWING_MS: u64 = 150;
const SWING_APART_MS: u64 = 70;
/// A peek to one side: the glance that leads it and closes it, and the lean
/// between them.
const PEEK_LEAD_MS: u64 = 140;
const PEEK_MS: u64 = 480;
/// A foot off the ground, and both feet down between two taps.
const TAP_MS: u64 = 110;
const TAP_APART_MS: u64 = 80;
/// A doze: the eyes grow heavy, close, the head nods and stays down, it
/// stirs, and it starts awake.
const DROWSE_MS: u64 = 320;
const NOD_OFF_MS: u64 = 480;
const ASLEEP_MS: u64 = 880;
const STIR_MS: u64 = 110;
const WAKE_MS: u64 = 320;

/// One still of a mascot — its rows, drawn verbatim like [`Mascot::art`].
type Art = &'static [&'static str];

/// One step of a [`Clip`]: a still, and how long it stays up.
#[derive(Debug, Clone, Copy)]
struct Step {
    art: Art,
    ms: u64,
}

/// A short run of [`Step`]s — a blink, a glance, a bounce, a mascot's own
/// move — optionally under a sweep of light that crosses the art once over
/// the clip's whole length.
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

// --- The moves the mascots share. Each is written once, here, over the
// stills of whichever mascot's module asks for it: a macro's body names
// `REST`, `SHUT`, `DIPPED` and the rest where it is used, so the crest's
// blink and the bloom's are one clip over two sets of frames. A module asks
// only for what its mascot can do — a sprout has no lid and cannot dip, a
// bloom fills its block from edge to edge and cannot lean. ---

/// The blink of eyes that have a lid: down, shut, up — and twice over.
macro_rules! lidded_blinks {
    () => {
const BLINK: Clip = clip![LID for LID_DOWN_MS, SHUT for SHUT_MS, LID for LID_UP_MS];
        const BLINK_TWICE: Clip = clip![
            LID for LID_DOWN_MS, SHUT for SHUT_MS, LID for LID_UP_MS,
            REST for BLINK_APART_MS,
            LID for LID_DOWN_MS, SHUT for SHUT_MS, LID for LID_UP_MS,
        ];
    };
}

/// The blink of eyes that have none: shut in one frame — and twice over.
macro_rules! lidless_blinks {
    () => {
const BLINK: Clip = clip![SHUT for BLINK_MS];
        const BLINK_TWICE: Clip = clip![
            SHUT for BLINK_MS, REST for BLINK_APART_MS, SHUT for BLINK_MS,
        ];
    };
}

/// What any pair of eyes does besides blink: a glance to either side, a
/// look around, a shifty dart from side to side, a wink of either eye, and
/// a cross-eyed moment — beside the sweep of light every eyed mascot's
/// greeting opens on.
macro_rules! looks {
    () => {
const SWEEP: Clip = lit_clip![REST for SWEEP_MS];
const GLANCE_LEFT: Clip = clip![LOOK_LEFT for GLANCE_MS];
const GLANCE_RIGHT: Clip = clip![LOOK_RIGHT for GLANCE_MS];
        const LOOK_AROUND: Clip = clip![
            LOOK_LEFT for LOOK_MS, REST for LOOK_APART_MS, LOOK_RIGHT for LOOK_MS,
        ];
        const SHIFTY: Clip = clip![
            LOOK_LEFT for DART_MS, LOOK_RIGHT for DART_MS,
            LOOK_LEFT for DART_MS, LOOK_RIGHT for DART_MS,
        ];
const WINK_LEFT: Clip = clip![LEFT_SHUT for WINK_MS];
const WINK_RIGHT: Clip = clip![RIGHT_SHUT for WINK_MS];
const CROSS_EYES: Clip = clip![CROSSED for CROSS_MS];
    };
}

/// The looks only lidded eyes can give — the eighth-block ramp that draws a
/// lid draws an eye opened wide too: a startled stare, a contented squint,
/// and one of each, the look of something that is not so sure.
macro_rules! stares {
    () => {
const STARTLE: Clip = clip![WIDE for WIDE_MS];
const SQUINT: Clip = clip![LID for SQUINT_MS];
const HUH: Clip = clip![ONE_WIDE for HUH_MS];
    };
}

/// A bounce: the whole mascot dips half a cell and is up again, twice.
macro_rules! bob {
    () => {
        const BOB: Clip = clip![
            DIPPED for DIP_MS, REST for BOUNCE_APART_MS, DIPPED for DIP_MS,
        ];
    };
}

/// A happy bounce: three of them, eyes squeezed shut on every landing.
macro_rules! happy {
    () => {
        const HAPPY: Clip = clip![
            DIPPED_SHUT for DIP_MS, REST for BOUNCE_APART_MS,
            DIPPED_SHUT for DIP_MS, REST for BOUNCE_APART_MS,
            DIPPED_SHUT for DIP_MS,
        ];
    };
}

/// A wiggle: half a cell to the left, half a cell to the right, twice,
/// without a stop in the middle.
macro_rules! wiggle {
    () => {
        const WIGGLE: Clip = clip![
            LEANT_LEFT for STEP_MS, LEANT_RIGHT for STEP_MS,
            LEANT_LEFT for STEP_MS, LEANT_RIGHT for STEP_MS,
        ];
    };
}

/// A peek: the eyes go to one side and the whole mascot leans after them.
macro_rules! peeks {
    () => {
        const PEEK_LEFT: Clip = clip![
            LOOK_LEFT for PEEK_LEAD_MS, PEEKING_LEFT for PEEK_MS, LOOK_LEFT for PEEK_LEAD_MS,
        ];
        const PEEK_RIGHT: Clip = clip![
            LOOK_RIGHT for PEEK_LEAD_MS, PEEKING_RIGHT for PEEK_MS, LOOK_RIGHT for PEEK_LEAD_MS,
        ];
    };
}

/// A little dance: down to the left, down to the right, twice.
macro_rules! dance {
    () => {
        const DANCE: Clip = clip![
            DIPPED_LEFT for SWING_MS, REST for SWING_APART_MS,
            DIPPED_RIGHT for SWING_MS, REST for SWING_APART_MS,
            DIPPED_LEFT for SWING_MS, REST for SWING_APART_MS,
            DIPPED_RIGHT for SWING_MS,
        ];
    };
}

/// Feet tapping: the left, the right, twice.
macro_rules! foot_taps {
    () => {
        const TAP: Clip = clip![
            LEFT_FOOT_UP for TAP_MS, REST for TAP_APART_MS,
            RIGHT_FOOT_UP for TAP_MS, REST for TAP_APART_MS,
            LEFT_FOOT_UP for TAP_MS, REST for TAP_APART_MS,
            RIGHT_FOOT_UP for TAP_MS,
        ];
    };
}

/// A doze, for eyes with a lid: they grow heavy, close, the head nods and
/// stays down — and it starts awake, wide-eyed.
macro_rules! lidded_doze {
    () => {
        const DOZE: Clip = clip![
            LID for DROWSE_MS, SHUT for NOD_OFF_MS, DIPPED_SHUT for ASLEEP_MS,
            SHUT for STIR_MS, WIDE for WAKE_MS,
        ];
    };
}

/// One kind of card in a mascot's deck: a move, how many copies of it a
/// round deals, and whether it is held back from the first round.
#[derive(Debug, Clone, Copy)]
struct Card {
    /// How many beats of a round play this card.
    copies: u8,
    /// The clips its copies play, taking turns: two copies of a card with a
    /// left and a right play one of each, in an order the round draws.
    moves: &'static [&'static Clip],
    /// Held back from the first round — a doze comes to a mascot that has
    /// been left alone a while, not to one the app has just opened.
    late: bool,
}

/// A card dealt `copies` times a round.
const fn card(copies: u8, moves: &'static [&'static Clip]) -> Card {
    Card {
        copies,
        moves,
        late: false,
    }
}

/// A card dealt once a round, from the second round on.
const fn late(moves: &'static [&'static Clip]) -> Card {
    Card {
        copies: 1,
        moves,
        late: true,
    }
}

/// The most kinds of card a deck holds — the size of the hand a deal works
/// in.
const DECK_KINDS: usize = 24;

// What tells the three draws a round makes from one seed apart.
const DEAL: u64 = 0xA076_1D64_78BD_642F;
const CLOSER: u64 = 0xE703_7ED1_A0B4_28DB;
const TURN: u64 = 0x8EBC_6AF0_9C88_C6E3;

/// Everything one mascot does: its greeting, and the deck its beats are
/// dealt from.
struct Motions {
    /// The greeting's three clips, in order — the sweep, the mascot's own
    /// move, and a blink (the gem's: a glint and two twinkles).
    greeting: [&'static Clip; 3],
    /// How many beats of a round the mascot stands still.
    rests: u8,
    /// The moves a round deals. The **first** card is the mascot's everyday
    /// move — the blink its greeting closes on (the gem's twinkle), which
    /// the first round therefore never opens with.
    deck: &'static [Card],
}

impl Motions {
    /// How many copies of each kind `round` deals: the deck's, less the
    /// cards held back from the first round.
    fn copies(&self, round: u64) -> [u8; DECK_KINDS] {
        let mut copies = [0; DECK_KINDS];
        for (kind, card) in self.deck.iter().enumerate().take(DECK_KINDS) {
            if round > 0 || !card.late {
                copies[kind] = card.copies;
            }
        }
        copies
    }

    /// How many beats `round` lasts: one a card, and its rests.
    fn beats(&self, round: u64) -> u64 {
        let moves: u64 = self.copies(round).iter().map(|&n| u64::from(n)).sum();
        moves + u64::from(self.rests)
    }

    /// The round `beat` falls in, and how far into it.
    fn round_at(&self, beat: u64) -> (u64, u64) {
        match beat.checked_sub(self.beats(0)) {
            None => (0, beat),
            Some(past) => {
                let each = self.beats(1).max(1);
                (1 + past / each, past % each)
            }
        }
    }

    /// The kind `round` closes on: one of the moves it holds a single copy
    /// of, drawn by the round. Fixed before the deal, it is what lets a
    /// round promise the next one a move to differ from without either
    /// being dealt first.
    fn closer(&self, seed: u64, round: u64) -> usize {
        let copies = self.copies(round);
        let kinds = self.deck.len().min(DECK_KINDS);
        let singles = (0..kinds).filter(|&kind| copies[kind] == 1);
        let count = u64::try_from(singles.clone().count()).unwrap_or(1).max(1);
        let pick = usize::try_from(lot(seed ^ CLOSER, round) % count).unwrap_or(0);
        singles.clone().nth(pick).unwrap_or(0)
    }

    /// The card dealt `at` beats into `round` — its kind, and how many of
    /// that kind the round dealt before it — or `None` for a rest.
    ///
    /// A round is its deck shuffled, with two rules a shuffle alone would
    /// not keep: **no move twice running** (not across a rest, not across
    /// the round's two ends) and **no two rests in a row**. So the moves
    /// are dealt one at a time, each drawn from every card left but the kind
    /// just played — and a kind that has come to make up more than half of
    /// what is left goes at once, the one way its copies can still be kept
    /// apart; the round's last move was drawn before the deal
    /// ([`closer`](Self::closer)); and each rest is slipped in ahead of a
    /// move of its own, drawn from the moves still to come.
    fn dealt(&self, seed: u64, round: u64, at: u64) -> Option<(usize, u8)> {
        let kinds = self.deck.len().min(DECK_KINDS);
        let mut copies = self.copies(round);
        let closer = self.closer(seed, round);
        // The closer's one copy is the round's last card.
        copies[closer] = 0;
        let mut moves_left: u64 = copies.iter().map(|&n| u64::from(n)).sum();
        let mut rests_left = u64::from(self.rests);
        let mut played = [0u8; DECK_KINDS];
        // The move before this round's first: the last round's closer — or,
        // ahead of the first round, the everyday move the greeting ends on.
        let mut last = round
            .checked_sub(1)
            .map_or(0, |before| self.closer(seed, before));
        let mut draw = lot(seed ^ DEAL, round);
        let mut beat = 0;
        loop {
            // A rest ahead of this move? Each rest left takes one of the
            // moves still to come, the closer among them.
            draw = lot(draw, beat);
            if draw % (moves_left + 1) < rests_left {
                if beat == at {
                    return None;
                }
                rests_left -= 1;
                beat += 1;
            }
            if moves_left == 0 {
                return Some((closer, 0));
            }
            let crowded = (0..kinds).find(|&kind| u64::from(copies[kind]) * 2 > moves_left);
            let kind = crowded.unwrap_or_else(|| {
                draw = lot(draw, beat);
                let mut pick = draw % (moves_left - u64::from(copies[last])).max(1);
                (0..kinds)
                    .filter(|&kind| kind != last)
                    .find(|&kind| {
                        let here = u64::from(copies[kind]);
                        let found = pick < here;
                        pick = pick.saturating_sub(here);
                        found
                    })
                    .unwrap_or(last)
            });
            if beat == at {
                return Some((kind, played[kind]));
            }
            copies[kind] = copies[kind].saturating_sub(1);
            played[kind] += 1;
            moves_left -= 1;
            last = kind;
            beat += 1;
        }
    }

    /// The clip the `nth` copy of card `kind` plays in `round`: the card's
    /// moves take turns, from one the round draws.
    fn played(&self, seed: u64, round: u64, kind: usize, nth: u8) -> Option<&'static Clip> {
        let moves = self.deck.get(kind)?.moves;
        let count = u64::try_from(moves.len()).ok().filter(|&count| count > 0)?;
        let card = u64::try_from(kind).unwrap_or(0);
        let turn = lot(
            seed ^ TURN,
            round.wrapping_mul(DECK_KINDS as u64).wrapping_add(card),
        );
        let index = (turn % count + u64::from(nth)) % count;
        moves.get(usize::try_from(index).ok()?).copied()
    }

    /// Every clip the mascot can play.
    fn clips(&self) -> impl Iterator<Item = &'static Clip> {
        self.greeting
            .into_iter()
            .chain(self.deck.iter().flat_map(|card| card.moves.iter().copied()))
    }
}

/// The crested hatchling: lidded eyes, and a frill it flares — the spikes
/// pull in for an instant, fan out to six, and settle — ripples from end to
/// end, and flattens when it ducks.
mod crest {
    use super::*;

    const REST: Art = Mascot::Crest.art();
    // The eyes.
    const LID: Art = &[" ▙▄▙▄▟▄▟", "▝▜▆███▆▛▘", "  ▘▘ ▝▝"];
    const SHUT: Art = &[" ▙▄▙▄▟▄▟", "▝▜█████▛▘", "  ▘▘ ▝▝"];
    const LOOK_LEFT: Art = &[" ▙▄▙▄▟▄▟", "▝▜▟███▟▛▘", "  ▘▘ ▝▝"];
    const LOOK_RIGHT: Art = &[" ▙▄▙▄▟▄▟", "▝▜▙███▙▛▘", "  ▘▘ ▝▝"];
    const LEFT_SHUT: Art = &[" ▙▄▙▄▟▄▟", "▝▜████▄▛▘", "  ▘▘ ▝▝"];
    const RIGHT_SHUT: Art = &[" ▙▄▙▄▟▄▟", "▝▜▄████▛▘", "  ▘▘ ▝▝"];
    const WIDE: Art = &[" ▙▄▙▄▟▄▟", "▝▜▂███▂▛▘", "  ▘▘ ▝▝"];
    const ONE_WIDE: Art = &[" ▙▄▙▄▟▄▟", "▝▜▂███▆▛▘", "  ▘▘ ▝▝"];
    const CROSSED: Art = &[" ▙▄▙▄▟▄▟", "▝▜▙███▟▛▘", "  ▘▘ ▝▝"];
    // The frill: pulled in, flared, each spike dropped in turn, and flat.
    const RUFFLE: Art = &[" ▟▄▟▄▙▄▙", "▝▜▄███▄▛▘", "  ▘▘ ▝▝"];
    const FLARE: Art = &[" ▙▙▙▄▟▟▟", "▝▜▄███▄▛▘", "  ▘▘ ▝▝"];
    const DROP_1: Art = &[" ▄▄▙▄▟▄▟", "▝▜▄███▄▛▘", "  ▘▘ ▝▝"];
    const DROP_2: Art = &[" ▙▄▄▄▟▄▟", "▝▜▄███▄▛▘", "  ▘▘ ▝▝"];
    const DROP_3: Art = &[" ▙▄▙▄▄▄▟", "▝▜▄███▄▛▘", "  ▘▘ ▝▝"];
    const DROP_4: Art = &[" ▙▄▙▄▟▄▄", "▝▜▄███▄▛▘", "  ▘▘ ▝▝"];
    const FLAT: Art = &[" ▄▄▄▄▄▄▄", "▝▜▄███▄▛▘", "  ▘▘ ▝▝"];
    // The feet.
    const LEFT_FOOT_UP: Art = &[" ▙▄▙▄▟▄▟", "▝▜▄███▄▛▘", "   ▘ ▝▝"];
    const RIGHT_FOOT_UP: Art = &[" ▙▄▙▄▟▄▟", "▝▜▄███▄▛▘", "  ▘▘ ▝"];
    // The whole hatchling, half a cell off.
    const DIPPED: Art = &[" ▖ ▖ ▗ ▗", "▗█▀███▀█▖", " ▝▛▛▀▜▜▘"];
    const DIPPED_SHUT: Art = &[" ▖ ▖ ▗ ▗", "▗███████▖", " ▝▛▛▀▜▜▘"];
    const DIPPED_LEFT: Art = &["▗ ▗  ▖ ▖", "▟▛▜██▛▜▙", " ▜▜▀▀▛▛"];
    const DIPPED_RIGHT: Art = &[" ▗ ▗  ▖ ▖", " ▟▛▜██▛▜▙", "  ▜▜▀▀▛▛"];
    const LEANT_LEFT: Art = &["▐▄▟▄▄▙▄▌", "▀▙▟██▙▟▀", " ▝▝  ▘▘"];
    const LEANT_RIGHT: Art = &[" ▐▄▟▄▄▙▄▌", " ▀▙▟██▙▟▀", "  ▝▝  ▘▘"];
    const PEEKING_LEFT: Art = &["▐▄▟▄▄▙▄▌", "▀▙███▙█▀", " ▝▝  ▘▘"];
    const PEEKING_RIGHT: Art = &[" ▐▄▟▄▄▙▄▌", " ▀█▟███▟▀", "  ▝▝  ▘▘"];

    lidded_blinks!();
    looks!();
    stares!();
    bob!();
    happy!();
    wiggle!();
    peeks!();
    dance!();
    foot_taps!();
    lidded_doze!();
    const FLARE_FRILL: Clip = clip![RUFFLE for 70, FLARE for 460, RUFFLE for 70];
    const RIPPLE: Clip = clip![
        DROP_1 for 70, DROP_2 for 70, DROP_3 for 70, DROP_4 for 70,
        DROP_3 for 70, DROP_2 for 70, DROP_1 for 70,
    ];
    const DUCK: Clip = clip![FLAT for 340, RUFFLE for 60, FLARE for 260, RUFFLE for 60];

    pub(super) const MOTIONS: Motions = Motions {
        greeting: [&SWEEP, &FLARE_FRILL, &BLINK],
        rests: 4,
        deck: &[
            card(3, &[&BLINK, &BLINK_TWICE, &BLINK]),
            card(2, &[&GLANCE_LEFT, &GLANCE_RIGHT]),
            card(2, &[&WINK_LEFT, &WINK_RIGHT]),
            card(4, &[&STARTLE, &SQUINT, &HUH, &CROSS_EYES]),
            card(2, &[&PEEK_LEFT, &PEEK_RIGHT]),
            card(3, &[&BOB, &HAPPY, &BOB]),
            card(2, &[&FLARE_FRILL]),
            card(1, &[&LOOK_AROUND]),
            card(1, &[&SHIFTY]),
            card(1, &[&WIGGLE]),
            card(1, &[&DANCE]),
            card(1, &[&RIPPLE]),
            card(1, &[&DUCK]),
            card(1, &[&TAP]),
            card(1, &[&SWEEP]),
            late(&[&DOZE]),
        ],
    };
}

/// The round creature in bloom: lidded eyes, and petals at its sides that
/// it flutters — down and up, three times, like something about to lift
/// off — seesaws, and tucks in; and a head it puffs up to its full width.
/// It fills its block from edge to edge, so it bounces but never leans.
mod bloom {
    use super::*;

    const REST: Art = Mascot::Bloom.art();
    // The eyes.
    const LID: Art = &[" ▄█████▄", "▀█▆███▆█▀", " ▝▝   ▘▘"];
    const SHUT: Art = &[" ▄█████▄", "▀███████▀", " ▝▝   ▘▘"];
    const LOOK_LEFT: Art = &[" ▄█████▄", "▀█▟███▟█▀", " ▝▝   ▘▘"];
    const LOOK_RIGHT: Art = &[" ▄█████▄", "▀█▙███▙█▀", " ▝▝   ▘▘"];
    const LEFT_SHUT: Art = &[" ▄█████▄", "▀█████▄█▀", " ▝▝   ▘▘"];
    const RIGHT_SHUT: Art = &[" ▄█████▄", "▀█▄█████▀", " ▝▝   ▘▘"];
    const WIDE: Art = &[" ▄█████▄", "▀█▂███▂█▀", " ▝▝   ▘▘"];
    const ONE_WIDE: Art = &[" ▄█████▄", "▀█▂███▆█▀", " ▝▝   ▘▘"];
    const CROSSED: Art = &[" ▄█████▄", "▀█▙███▟█▀", " ▝▝   ▘▘"];
    // The petals: both down, one down, and tucked in.
    const DOWN: Art = &[" ▄█████▄", "▄█▄███▄█▄", " ▝▝   ▘▘"];
    const LEFT_DOWN: Art = &[" ▄█████▄", "▄█▄███▄█▀", " ▝▝   ▘▘"];
    const RIGHT_DOWN: Art = &[" ▄█████▄", "▀█▄███▄█▄", " ▝▝   ▘▘"];
    const TUCKED: Art = &[" ▄█████▄", " █▄███▄█", " ▝▝   ▘▘"];
    // The head, puffing up.
    const PUFFING: Art = &[" ▟█████▙", "▀█▄███▄█▀", " ▝▝   ▘▘"];
    const PUFFED: Art = &[" ███████", "▀█▄███▄█▀", " ▝▝   ▘▘"];
    // The feet.
    const LEFT_FOOT_UP: Art = &[" ▄█████▄", "▀█▄███▄█▀", "  ▝   ▘▘"];
    const RIGHT_FOOT_UP: Art = &[" ▄█████▄", "▀█▄███▄█▀", " ▝▝   ▘"];
    // The whole creature, half a cell down.
    const DIPPED: Art = &["  ▄▄▄▄▄", "▄█▀███▀█▄", " ▜▜▀▀▀▛▛"];
    const DIPPED_SHUT: Art = &["  ▄▄▄▄▄", "▄███████▄", " ▜▜▀▀▀▛▛"];

    lidded_blinks!();
    looks!();
    stares!();
    bob!();
    happy!();
    foot_taps!();
    lidded_doze!();
    const FLUTTER: Clip = clip![
        DOWN for 110, REST for 110, DOWN for 110, REST for 110, DOWN for 110,
    ];
    const SEESAW: Clip = clip![
        LEFT_DOWN for 150, RIGHT_DOWN for 150, LEFT_DOWN for 150, RIGHT_DOWN for 150,
    ];
    const PUFF: Clip = clip![PUFFING for 80, PUFFED for 420, PUFFING for 80];
    const SHY: Clip = clip![TUCKED for 560];

    pub(super) const MOTIONS: Motions = Motions {
        greeting: [&SWEEP, &FLUTTER, &BLINK],
        rests: 4,
        deck: &[
            card(3, &[&BLINK, &BLINK_TWICE, &BLINK]),
            card(2, &[&GLANCE_LEFT, &GLANCE_RIGHT]),
            card(2, &[&WINK_LEFT, &WINK_RIGHT]),
            card(4, &[&STARTLE, &SQUINT, &HUH, &CROSS_EYES]),
            card(4, &[&BOB, &HAPPY, &BOB, &HAPPY]),
            card(2, &[&FLUTTER]),
            card(2, &[&SEESAW]),
            card(1, &[&LOOK_AROUND]),
            card(1, &[&SHIFTY]),
            card(1, &[&PUFF]),
            card(1, &[&SHY]),
            card(1, &[&TAP]),
            card(1, &[&SWEEP]),
            late(&[&DOZE]),
        ],
    };
}

/// The seedling: eyes in the lower half of their cells, where the font-safe
/// glyphs offer no lid, so it blinks in one frame — and a shoot that sways
/// in a draught, unfurls a leaf to either side or both, and sinks to a bud
/// to push up again; and arms it throws up, or waves. Its feet stand on
/// the block's last half-row, so it leans but never bounces — and never
/// dozes.
mod sprout {
    use super::*;

    const REST: Art = Mascot::Sprout.art();
    // The eyes.
    const SHUT: Art = &["    █", "▗▟█████▙▖", " ▝▛▛▀▜▜▘"];
    const LOOK_LEFT: Art = &["    █", "▗▟▜███▜▙▖", " ▝▛▛▀▜▜▘"];
    const LOOK_RIGHT: Art = &["    █", "▗▟▛███▛▙▖", " ▝▛▛▀▜▜▘"];
    const LEFT_SHUT: Art = &["    █", "▗▟████▀▙▖", " ▝▛▛▀▜▜▘"];
    const RIGHT_SHUT: Art = &["    █", "▗▟▀████▙▖", " ▝▛▛▀▜▜▘"];
    const CROSSED: Art = &["    █", "▗▟▛███▜▙▖", " ▝▛▛▀▜▜▘"];
    // The shoot: its tip bent, a leaf out, both out, and sunk to a bud.
    const TIP_RIGHT: Art = &["    ▟▘", "▗▟▀███▀▙▖", " ▝▛▛▀▜▜▘"];
    const TIP_LEFT: Art = &["   ▝▙", "▗▟▀███▀▙▖", " ▝▛▛▀▜▜▘"];
    const LEAF: Art = &["    █▀", "▗▟▀███▀▙▖", " ▝▛▛▀▜▜▘"];
    const LEAF_LEFT: Art = &["   ▀█", "▗▟▀███▀▙▖", " ▝▛▛▀▜▜▘"];
    const LEAVES: Art = &["   ▀█▀", "▗▟▀███▀▙▖", " ▝▛▛▀▜▜▘"];
    const BUD: Art = &["    ▄", "▗▟▀███▀▙▖", " ▝▛▛▀▜▜▘"];
    // The arms.
    const ARMS_UP: Art = &["    █", "▝▜▀███▀▛▘", " ▝▛▛▀▜▜▘"];
    const LEFT_ARM_UP: Art = &["    █", "▝▜▀███▀▙▖", " ▝▛▛▀▜▜▘"];
    const RIGHT_ARM_UP: Art = &["    █", "▗▟▀███▀▛▘", " ▝▛▛▀▜▜▘"];
    // The whole seedling, half a cell off.
    const LEANT_LEFT: Art = &["   ▐▌", "▄▛▜██▛▜▄", " ▜▜▀▀▛▛"];
    const LEANT_RIGHT: Art = &["    ▐▌", " ▄▛▜██▛▜▄", "  ▜▜▀▀▛▛"];
    const PEEKING_LEFT: Art = &["   ▐▌", "▄▛███▛█▄", " ▜▜▀▀▛▛"];
    const PEEKING_RIGHT: Art = &["    ▐▌", " ▄█▜███▜▄", "  ▜▜▀▀▛▛"];

    lidless_blinks!();
    looks!();
    wiggle!();
    peeks!();
    const SWAY: Clip = clip![TIP_RIGHT for 170, REST for 110, TIP_LEFT for 170];
    const UNFURL: Clip = clip![TIP_RIGHT for 90, LEAF for 520, TIP_RIGHT for 90];
    const UNFURL_LEFT: Clip = clip![TIP_LEFT for 90, LEAF_LEFT for 520, TIP_LEFT for 90];
    const SPREAD: Clip = clip![
        TIP_RIGHT for 80, LEAF for 90, LEAVES for 520, LEAF for 90, TIP_RIGHT for 80,
    ];
    const GROW: Clip = clip![
        BUD for 320, REST for 140, TIP_RIGHT for 90, LEAF for 420, TIP_RIGHT for 90,
    ];
    const CHEER: Clip = clip![
        ARMS_UP for 150, REST for 90, ARMS_UP for 150, REST for 90, ARMS_UP for 150,
    ];
    const WAVE_RIGHT: Clip = clip![
        RIGHT_ARM_UP for 140, REST for 90, RIGHT_ARM_UP for 140, REST for 90,
        RIGHT_ARM_UP for 140,
    ];
    const WAVE_LEFT: Clip = clip![
        LEFT_ARM_UP for 140, REST for 90, LEFT_ARM_UP for 140, REST for 90,
        LEFT_ARM_UP for 140,
    ];

    pub(super) const MOTIONS: Motions = Motions {
        greeting: [&SWEEP, &UNFURL, &BLINK],
        rests: 4,
        deck: &[
            card(3, &[&BLINK, &BLINK_TWICE, &BLINK]),
            card(2, &[&GLANCE_LEFT, &GLANCE_RIGHT]),
            card(2, &[&WINK_LEFT, &WINK_RIGHT]),
            card(2, &[&PEEK_LEFT, &PEEK_RIGHT]),
            card(2, &[&WIGGLE]),
            card(2, &[&UNFURL, &UNFURL_LEFT]),
            card(2, &[&SWAY]),
            card(2, &[&WAVE_RIGHT, &WAVE_LEFT]),
            card(1, &[&LOOK_AROUND]),
            card(1, &[&SHIFTY]),
            card(1, &[&CROSS_EYES]),
            card(1, &[&SPREAD]),
            card(1, &[&GROW]),
            card(1, &[&CHEER]),
            card(1, &[&SWEEP]),
        ],
    };
}

/// The two-eared one: lidded eyes, and ears that flick — fold flat and
/// spring back, twice, now the right, now the left — wiggle out and in,
/// sway together, and droop only to spring up again.
mod twin {
    use super::*;

    const REST: Art = Mascot::Twin.art();
    // The eyes.
    const LID: Art = &[" ██▄▄▄██", "▝▜▆███▆▛▘", "  ▘▘ ▝▝"];
    const SHUT: Art = &[" ██▄▄▄██", "▝▜█████▛▘", "  ▘▘ ▝▝"];
    const LOOK_LEFT: Art = &[" ██▄▄▄██", "▝▜▟███▟▛▘", "  ▘▘ ▝▝"];
    const LOOK_RIGHT: Art = &[" ██▄▄▄██", "▝▜▙███▙▛▘", "  ▘▘ ▝▝"];
    const LEFT_SHUT: Art = &[" ██▄▄▄██", "▝▜████▄▛▘", "  ▘▘ ▝▝"];
    const RIGHT_SHUT: Art = &[" ██▄▄▄██", "▝▜▄████▛▘", "  ▘▘ ▝▝"];
    const WIDE: Art = &[" ██▄▄▄██", "▝▜▂███▂▛▘", "  ▘▘ ▝▝"];
    const ONE_WIDE: Art = &[" ██▄▄▄██", "▝▜▂███▆▛▘", "  ▘▘ ▝▝"];
    const CROSSED: Art = &[" ██▄▄▄██", "▝▜▙███▟▛▘", "  ▘▘ ▝▝"];
    // The ears: one folding, both apart, together, to a side, flat, pricked.
    const LEFT_HALF: Art = &[" █▄▄▄▄██", "▝▜▄███▄▛▘", "  ▘▘ ▝▝"];
    const LEFT_FLAT: Art = &[" ▄▄▄▄▄██", "▝▜▄███▄▛▘", "  ▘▘ ▝▝"];
    const RIGHT_HALF: Art = &[" ██▄▄▄▄█", "▝▜▄███▄▛▘", "  ▘▘ ▝▝"];
    const RIGHT_FLAT: Art = &[" ██▄▄▄▄▄", "▝▜▄███▄▛▘", "  ▘▘ ▝▝"];
    const EARS_OUT: Art = &["▝█▙▄▄▄▟█▘", "▝▜▄███▄▛▘", "  ▘▘ ▝▝"];
    const EARS_IN: Art = &[" ▟█▙▄▟█▙", "▝▜▄███▄▛▘", "  ▘▘ ▝▝"];
    const EARS_LEFT: Art = &["▝█▙▄▄▟█▙", "▝▜▄███▄▛▘", "  ▘▘ ▝▝"];
    const EARS_RIGHT: Art = &[" ▟█▙▄▄▟█▘", "▝▜▄███▄▛▘", "  ▘▘ ▝▝"];
    const EARS_FLAT: Art = &[" ▄▄▄▄▄▄▄", "▝▜▄███▄▛▘", "  ▘▘ ▝▝"];
    const EARS_PRICKED: Art = &[" ▟▙▄▄▄▟▙", "▝▜▄███▄▛▘", "  ▘▘ ▝▝"];
    // The feet.
    const LEFT_FOOT_UP: Art = &[" ██▄▄▄██", "▝▜▄███▄▛▘", "   ▘ ▝▝"];
    const RIGHT_FOOT_UP: Art = &[" ██▄▄▄██", "▝▜▄███▄▛▘", "  ▘▘ ▝"];
    // The whole creature, half a cell off.
    const DIPPED: Art = &[" ▄▄   ▄▄", "▗█▀███▀█▖", " ▝▛▛▀▜▜▘"];
    const DIPPED_SHUT: Art = &[" ▄▄   ▄▄", "▗███████▖", " ▝▛▛▀▜▜▘"];
    const DIPPED_LEFT: Art = &["▗▄▖  ▗▄▖", "▟▛▜██▛▜▙", " ▜▜▀▀▛▛"];
    const DIPPED_RIGHT: Art = &[" ▗▄▖  ▗▄▖", " ▟▛▜██▛▜▙", "  ▜▜▀▀▛▛"];
    const LEANT_LEFT: Art = &["▐█▙▄▄▟█▌", "▀▙▟██▙▟▀", " ▝▝  ▘▘"];
    const LEANT_RIGHT: Art = &[" ▐█▙▄▄▟█▌", " ▀▙▟██▙▟▀", "  ▝▝  ▘▘"];
    const PEEKING_LEFT: Art = &["▐█▙▄▄▟█▌", "▀▙███▙█▀", " ▝▝  ▘▘"];
    const PEEKING_RIGHT: Art = &[" ▐█▙▄▄▟█▌", " ▀█▟███▟▀", "  ▝▝  ▘▘"];

    lidded_blinks!();
    looks!();
    stares!();
    bob!();
    happy!();
    wiggle!();
    peeks!();
    dance!();
    foot_taps!();
    lidded_doze!();
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
    const EAR_WIGGLE: Clip = clip![
        EARS_OUT for 110, EARS_IN for 110, EARS_OUT for 110, EARS_IN for 110,
    ];
    const EAR_SWAY: Clip = clip![EARS_LEFT for 170, REST for 80, EARS_RIGHT for 170];
    const DROOP: Clip = clip![EARS_FLAT for 380, EARS_PRICKED for 160];

    pub(super) const MOTIONS: Motions = Motions {
        greeting: [&SWEEP, &FLICK_RIGHT, &BLINK],
        rests: 4,
        deck: &[
            card(3, &[&BLINK, &BLINK_TWICE, &BLINK]),
            card(2, &[&GLANCE_LEFT, &GLANCE_RIGHT]),
            card(2, &[&WINK_LEFT, &WINK_RIGHT]),
            card(4, &[&STARTLE, &SQUINT, &HUH, &CROSS_EYES]),
            card(2, &[&PEEK_LEFT, &PEEK_RIGHT]),
            card(3, &[&BOB, &HAPPY, &BOB]),
            card(2, &[&FLICK_RIGHT, &FLICK_LEFT]),
            card(1, &[&LOOK_AROUND]),
            card(1, &[&SHIFTY]),
            card(1, &[&WIGGLE]),
            card(1, &[&DANCE]),
            card(1, &[&EAR_WIGGLE]),
            card(1, &[&EAR_SWAY]),
            card(1, &[&DROOP]),
            card(1, &[&TAP]),
            card(1, &[&SWEEP]),
            late(&[&DOZE]),
        ],
    };
}

/// The skitterer: eyes in the lower half of their cells (a one-frame blink,
/// like the seedling's), and six legs that scuttle on the spot, ripple one
/// after another, and stretch.
mod skiter {
    use super::*;

    const REST: Art = Mascot::Skiter.art();
    // The eyes.
    const SHUT: Art = &["▗▟█████▙▖", "▝▜█████▛▘", " ▘▘▝ ▘▝▝"];
    const LOOK_LEFT: Art = &["▗▟▜███▜▙▖", "▝▜█████▛▘", " ▘▘▝ ▘▝▝"];
    const LOOK_RIGHT: Art = &["▗▟▛███▛▙▖", "▝▜█████▛▘", " ▘▘▝ ▘▝▝"];
    const LEFT_SHUT: Art = &["▗▟████▀▙▖", "▝▜█████▛▘", " ▘▘▝ ▘▝▝"];
    const RIGHT_SHUT: Art = &["▗▟▀████▙▖", "▝▜█████▛▘", " ▘▘▝ ▘▝▝"];
    const CROSSED: Art = &["▗▟▛███▜▙▖", "▝▜█████▛▘", " ▘▘▝ ▘▝▝"];
    // The legs: a step to each side, each lifted in turn, drawn in, spread.
    const LEGS_RIGHT: Art = &["▗▟▀███▀▙▖", "▝▜█████▛▘", " ▝▝ ▘▝ ▘▘"];
    const LEGS_LEFT: Art = &["▗▟▀███▀▙▖", "▝▜█████▛▘", "▝▝ ▘▝ ▘▘"];
    const LEG_1_UP: Art = &["▗▟▀███▀▙▖", "▝▜█████▛▘", "  ▘▝ ▘▝▝"];
    const LEG_2_UP: Art = &["▗▟▀███▀▙▖", "▝▜█████▛▘", " ▘ ▝ ▘▝▝"];
    const LEG_3_UP: Art = &["▗▟▀███▀▙▖", "▝▜█████▛▘", " ▘▘  ▘▝▝"];
    const LEG_4_UP: Art = &["▗▟▀███▀▙▖", "▝▜█████▛▘", " ▘▘▝  ▝▝"];
    const LEG_5_UP: Art = &["▗▟▀███▀▙▖", "▝▜█████▛▘", " ▘▘▝ ▘ ▝"];
    const LEG_6_UP: Art = &["▗▟▀███▀▙▖", "▝▜█████▛▘", " ▘▘▝ ▘▝"];
    const TUCKED: Art = &["▗▟▀███▀▙▖", "▝▜█████▛▘", " ▝▝▝ ▘▘▘"];
    const SPLAYED: Art = &["▗▟▀███▀▙▖", "▝▜█████▛▘", "▝ ▘▝ ▘▝ ▘"];
    // The whole skitterer, half a cell off.
    const DIPPED: Art = &[" ▗▄▄▄▄▄▖", "▐█▄███▄█▌", " ▞▛▜▀▛▜▚"];
    const DIPPED_SHUT: Art = &[" ▗▄▄▄▄▄▖", "▐███████▌", " ▞▛▜▀▛▜▚"];
    const DIPPED_LEFT: Art = &[" ▄▄▄▄▄▄", "█▙▟██▙▟█", "▗▜▀▛▜▀▛▖"];
    const DIPPED_RIGHT: Art = &["  ▄▄▄▄▄▄", " █▙▟██▙▟█", " ▗▜▀▛▜▀▛▖"];
    const LEANT_LEFT: Art = &["▄▛▜██▛▜▄", "▀██████▀", "▝▝ ▘▝ ▘▘"];
    const LEANT_RIGHT: Art = &[" ▄▛▜██▛▜▄", " ▀██████▀", " ▝▝ ▘▝ ▘▘"];
    const PEEKING_LEFT: Art = &["▄▛███▛█▄", "▀██████▀", "▝▝ ▘▝ ▘▘"];
    const PEEKING_RIGHT: Art = &[" ▄█▜███▜▄", " ▀██████▀", " ▝▝ ▘▝ ▘▘"];

    lidless_blinks!();
    looks!();
    bob!();
    happy!();
    wiggle!();
    peeks!();
    dance!();
    /// A doze without a lid to lower: the eyes close, the body sinks and
    /// stays down, and it is up again.
    const DOZE: Clip = clip![
        SHUT for NOD_OFF_MS, DIPPED_SHUT for ASLEEP_MS, SHUT for STIR_MS + 40,
    ];
    const SKITTER: Clip = clip![
        LEGS_RIGHT for 75, LEGS_LEFT for 75, LEGS_RIGHT for 75,
        LEGS_LEFT for 75, LEGS_RIGHT for 75, LEGS_LEFT for 75,
    ];
    const LEG_WAVE: Clip = clip![
        LEG_1_UP for 55, LEG_2_UP for 55, LEG_3_UP for 55,
        LEG_4_UP for 55, LEG_5_UP for 55, LEG_6_UP for 55,
        LEG_1_UP for 55, LEG_2_UP for 55, LEG_3_UP for 55,
        LEG_4_UP for 55, LEG_5_UP for 55, LEG_6_UP for 55,
    ];
    const STRETCH: Clip = clip![TUCKED for 140, SPLAYED for 380, TUCKED for 140];

    pub(super) const MOTIONS: Motions = Motions {
        greeting: [&SWEEP, &SKITTER, &BLINK],
        rests: 4,
        deck: &[
            card(3, &[&BLINK, &BLINK_TWICE, &BLINK]),
            card(2, &[&GLANCE_LEFT, &GLANCE_RIGHT]),
            card(2, &[&WINK_LEFT, &WINK_RIGHT]),
            card(2, &[&PEEK_LEFT, &PEEK_RIGHT]),
            card(3, &[&BOB, &HAPPY, &BOB]),
            card(2, &[&SKITTER]),
            card(2, &[&WIGGLE]),
            card(2, &[&LEG_WAVE]),
            card(1, &[&LOOK_AROUND]),
            card(1, &[&SHIFTY]),
            card(1, &[&CROSS_EYES]),
            card(1, &[&DANCE]),
            card(1, &[&STRETCH]),
            card(1, &[&SWEEP]),
            late(&[&DOZE]),
        ],
    };
}

/// The cut gem: no eyes to blink, so it catches the light instead — a spark
/// opens at one of its four corners where the others blink, or at each in
/// turn; its one facet slides from side to side as the stone turns, and
/// stands tall when it gleams; and now and then a glint crosses the whole
/// of it. The facet is never shut: that would be an eye.
mod gem {
    use super::*;

    const REST: Art = Mascot::Gem.art();
    // The sparks: high on the right, low on the left, far up on the left,
    // near down on the right — each opening in three stills.
    const HIGH_1: Art = &["  ▄▟█▙▄ ·", "▝▀██▄██▀▘", "   ▝▀▘"];
    const HIGH_2: Art = &["  ▄▟█▙▄ ✢", "▝▀██▄██▀▘", "   ▝▀▘"];
    const HIGH_3: Art = &["  ▄▟█▙▄ ✶", "▝▀██▄██▀▘", "   ▝▀▘"];
    const LOW_1: Art = &["  ▄▟█▙▄", "▝▀██▄██▀▘", " · ▝▀▘"];
    const LOW_2: Art = &["  ▄▟█▙▄", "▝▀██▄██▀▘", " ✢ ▝▀▘"];
    const LOW_3: Art = &["  ▄▟█▙▄", "▝▀██▄██▀▘", " ✶ ▝▀▘"];
    const FAR_1: Art = &["· ▄▟█▙▄", "▝▀██▄██▀▘", "   ▝▀▘"];
    const FAR_2: Art = &["✢ ▄▟█▙▄", "▝▀██▄██▀▘", "   ▝▀▘"];
    const FAR_3: Art = &["✶ ▄▟█▙▄", "▝▀██▄██▀▘", "   ▝▀▘"];
    const NEAR_1: Art = &["  ▄▟█▙▄", "▝▀██▄██▀▘", "   ▝▀▘ ·"];
    const NEAR_2: Art = &["  ▄▟█▙▄", "▝▀██▄██▀▘", "   ▝▀▘ ✢"];
    const NEAR_3: Art = &["  ▄▟█▙▄", "▝▀██▄██▀▘", "   ▝▀▘ ✶"];
    // The facet: half a cell and a whole one to each side, and stood tall.
    const FACET_HALF_LEFT: Art = &["  ▄▟█▙▄", "▝▀█▙▟██▀▘", "   ▝▀▘"];
    const FACET_LEFT: Art = &["  ▄▟█▙▄", "▝▀█▄███▀▘", "   ▝▀▘"];
    const FACET_HALF_RIGHT: Art = &["  ▄▟█▙▄", "▝▀██▙▟█▀▘", "   ▝▀▘"];
    const FACET_RIGHT: Art = &["  ▄▟█▙▄", "▝▀███▄█▀▘", "   ▝▀▘"];
    const FACET_TALL: Art = &["  ▄▟█▙▄", "▝▀██▂██▀▘", "   ▝▀▘"];
    // The whole stone, half a cell off.
    const DIPPED: Art = &["   ▗▄▖", "▗▄██▀██▄▖", "  ▀▜█▛▀"];
    const DIPPED_LEFT: Art = &["   ▄▄", "▄▟█▛▜█▙▄", " ▝▀██▀▘"];
    const DIPPED_RIGHT: Art = &["    ▄▄", " ▄▟█▛▜█▙▄", "  ▝▀██▀▘"];
    const LEANT_LEFT: Art = &[" ▗▄██▄▖", "▀▜█▙▟█▛▀", "   ▀▀"];
    const LEANT_RIGHT: Art = &["  ▗▄██▄▖", " ▀▜█▙▟█▛▀", "    ▀▀"];

    /// A spark opening and closing through its three stills.
    macro_rules! twinkle {
        ($one:ident, $two:ident, $three:ident) => {
            clip![
                $one for SPARK_MS, $two for SPARK_MS, $three for SPARK_OPEN_MS,
                $two for SPARK_MS, $one for SPARK_MS,
            ]
        };
    }

    const TWINKLE_HIGH: Clip = twinkle!(HIGH_1, HIGH_2, HIGH_3);
    const TWINKLE_LOW: Clip = twinkle!(LOW_1, LOW_2, LOW_3);
    const TWINKLE_FAR: Clip = twinkle!(FAR_1, FAR_2, FAR_3);
    const TWINKLE_NEAR: Clip = twinkle!(NEAR_1, NEAR_2, NEAR_3);
    const TWINKLE_BOTH: Clip = clip![
        HIGH_1 for SPARK_MS, HIGH_2 for SPARK_MS, HIGH_3 for SPARK_OPEN_MS,
        HIGH_2 for SPARK_MS, HIGH_1 for SPARK_MS,
        REST for 160,
        LOW_1 for SPARK_MS, LOW_2 for SPARK_MS, LOW_3 for SPARK_OPEN_MS,
        LOW_2 for SPARK_MS, LOW_1 for SPARK_MS,
    ];
    /// A spark at each corner in turn, round the stone.
    const SPARKLE_ROUND: Clip = clip![
        HIGH_2 for 60, HIGH_3 for 90, HIGH_2 for 60,
        NEAR_2 for 60, NEAR_3 for 90, NEAR_2 for 60,
        LOW_2 for 60, LOW_3 for 90, LOW_2 for 60,
        FAR_2 for 60, FAR_3 for 90, FAR_2 for 60,
    ];
    /// The sweep, with the high spark opening as the light passes it.
    const GLINT: Clip = lit_clip![
        REST for 190,
        HIGH_1 for SPARK_MS, HIGH_2 for SPARK_MS, HIGH_3 for SPARK_OPEN_MS,
        HIGH_2 for SPARK_MS, HIGH_1 for SPARK_MS,
        REST for 170,
    ];
    const TURN_FACET: Clip = clip![
        FACET_HALF_LEFT for 90, FACET_LEFT for 150, FACET_HALF_LEFT for 90,
        REST for 110,
        FACET_HALF_RIGHT for 90, FACET_RIGHT for 150, FACET_HALF_RIGHT for 90,
    ];
    const GLEAM: Clip = clip![FACET_TALL for 480];
    const PULSE: Clip = clip![FACET_TALL for 140, REST for 100, FACET_TALL for 140];
    bob!();
    wiggle!();
    dance!();
    /// A slow rock from side to side.
    const ROCK: Clip = clip![LEANT_LEFT for 260, REST for 80, LEANT_RIGHT for 260];

    pub(super) const MOTIONS: Motions = Motions {
        greeting: [&GLINT, &TWINKLE_LOW, &TWINKLE_HIGH],
        rests: 4,
        deck: &[
            card(
                4,
                &[&TWINKLE_HIGH, &TWINKLE_LOW, &TWINKLE_FAR, &TWINKLE_NEAR],
            ),
            card(2, &[&TWINKLE_BOTH, &SPARKLE_ROUND]),
            card(2, &[&TURN_FACET]),
            card(2, &[&BOB]),
            card(2, &[&GLINT]),
            card(1, &[&GLEAM]),
            card(1, &[&PULSE]),
            card(1, &[&WIGGLE]),
            card(1, &[&DANCE]),
            card(1, &[&ROCK]),
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

/// The lot `seed` draws for `beat` — `SplitMix64`'s finalizer over the pair,
/// so neighbouring beats and neighbouring seeds share nothing. A beat draws
/// its clip's start from it, and a round its deal: each draw of a deal is
/// the lot of the draw before.
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
    /// wider than the art, single-width glyphs) and to the animation's: one
    /// row redrawn, or the whole mascot moved half a cell.
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
    /// The greeting's three slots are scripted. A beat plays the card its
    /// round deals it ([`Motions::dealt`]) — a round being the mascot's deck
    /// shuffled over the seed **and the mascot**, so no two mascots play one
    /// sequence (walking the `/mascot` list, each goes on from its greeting
    /// in an order of its own) — and starts it anywhere that leaves it whole
    /// inside the beat with [`BEAT_BREATH_MS`] to spare. No round deals two
    /// rests in a row, so the stillness is never longer than a beat and the
    /// two clips either side of it — about eight seconds — however the
    /// cards fall.
    fn cue(self, seed: u64, slot: u64) -> Option<(u64, &'static Clip)> {
        let motions = self.motions();
        let Some(beat) = slot.checked_sub(3) else {
            let slot = usize::try_from(slot).ok()?;
            return Some((GREETING_STARTS[slot], motions.greeting[slot]));
        };
        let seed = seed ^ self.salt();
        let (round, at) = motions.round_at(beat);
        let (kind, nth) = motions.dealt(seed, round, at)?;
        let clip = motions.played(seed, round, kind, nth)?;
        let room = BEAT_MS.saturating_sub(BEAT_BREATH_MS + clip.len());
        Some((
            slot_start(slot) + (lot(seed, beat) >> 32) % (room + 1),
            clip,
        ))
    }

    /// What this mascot looks like `clock` into the timeline `seed` plays —
    /// the rest art unless a clip is on at that instant.
    ///
    /// `clock` runs from the moment the banner first reached the screen, so
    /// the scripted greeting plays once, at launch; `seed` is the session's,
    /// so no two sessions keep the same rhythm. Pure and stateless: the pose
    /// at any instant costs the deal of one round — some thirty cards —
    /// however long the session has idled.
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

/// The dealer's own promises, checked on the cards rather than on the
/// timeline they make — what the pose tests (`app/tests/mascot_motion.rs`)
/// can only sample. See `docs/mascot-animation.md`.
#[cfg(test)]
mod tests {
    use super::*;

    const SEEDS: [u64; 6] = [0, 1, 0x5EED, 0xDEAD_BEEF, 0x0123_4567_89AB_CDEF, u64::MAX];

    /// Enough rounds to meet every way a deal can fall out.
    const ROUNDS: u64 = 60;

    /// Whether two clips are one move: the same stills, for the same time,
    /// under the same light.
    fn same(a: &Clip, b: &Clip) -> bool {
        a.shine == b.shine
            && a.steps.len() == b.steps.len()
            && a.steps
                .iter()
                .zip(b.steps)
                .all(|(x, y)| x.art == y.art && x.ms == y.ms)
    }

    /// Every card `round` deals, in order — `None` a rest.
    fn round(motions: &Motions, seed: u64, round: u64) -> Vec<Option<(usize, u8)>> {
        (0..motions.beats(round))
            .map(|at| motions.dealt(seed, round, at))
            .collect()
    }

    #[test]
    fn every_deck_fits_the_dealers_hand() {
        for mascot in Mascot::ALL {
            let motions = mascot.motions();
            assert!(motions.deck.len() <= DECK_KINDS, "{}", mascot.name());
            for card in motions.deck {
                assert!(card.copies >= 1, "{}: a card never dealt", mascot.name());
                assert!(
                    !card.moves.is_empty(),
                    "{}: a card with no move",
                    mascot.name()
                );
                // Every move a card lists gets its turn within a round.
                assert!(
                    card.moves.len() <= usize::from(card.copies),
                    "{}: more moves than copies",
                    mascot.name()
                );
            }
        }
    }

    #[test]
    fn a_round_deals_every_card_it_holds_and_every_rest() {
        for mascot in Mascot::ALL {
            let motions = mascot.motions();
            for seed in SEEDS {
                for number in 0..ROUNDS {
                    let cards = round(motions, seed, number);
                    let rests = cards.iter().filter(|card| card.is_none()).count();
                    assert_eq!(rests, usize::from(motions.rests), "{}", mascot.name());
                    let mut dealt = [0u8; DECK_KINDS];
                    for (kind, nth) in cards.iter().flatten() {
                        // Its copies are numbered in the order they come.
                        assert_eq!(*nth, dealt[*kind], "{}", mascot.name());
                        dealt[*kind] += 1;
                    }
                    assert_eq!(
                        dealt,
                        motions.copies(number),
                        "{} round {number} (seed {seed:#x})",
                        mascot.name()
                    );
                }
            }
        }
    }

    #[test]
    fn no_move_is_dealt_twice_running_and_no_two_rests_meet() {
        // Not across a rest, and not across the two ends of a round.
        for mascot in Mascot::ALL {
            let motions = mascot.motions();
            for seed in SEEDS {
                // The greeting closes on the everyday move.
                let mut last_move = Some(0);
                let mut rested = false;
                for number in 0..ROUNDS {
                    for (at, card) in round(motions, seed, number).into_iter().enumerate() {
                        match card {
                            None => {
                                assert!(
                                    !rested,
                                    "{} round {number} beat {at} (seed {seed:#x}): two rests",
                                    mascot.name()
                                );
                                rested = true;
                            }
                            Some((kind, _)) => {
                                assert_ne!(
                                    Some(kind),
                                    last_move,
                                    "{} round {number} beat {at} (seed {seed:#x})",
                                    mascot.name()
                                );
                                last_move = Some(kind);
                                rested = false;
                            }
                        }
                    }
                    // A round closes on a move — the one drawn for it.
                    assert_eq!(
                        last_move,
                        Some(motions.closer(seed, number)),
                        "{}",
                        mascot.name()
                    );
                    assert!(!rested, "{}: a round closing on a rest", mascot.name());
                }
            }
        }
    }

    #[test]
    fn a_cards_moves_take_turns() {
        // Two copies of a card with a left and a right play one of each;
        // three of the blink card — a blink, a double, a blink — play two
        // blinks and a double: every move as often as the card lists it.
        for mascot in Mascot::ALL {
            let motions = mascot.motions();
            for seed in SEEDS {
                for number in 1..ROUNDS {
                    for (kind, card) in motions.deck.iter().enumerate() {
                        let played: Vec<&Clip> = (0..card.copies)
                            .map(|nth| motions.played(seed, number, kind, nth).expect("a move"))
                            .collect();
                        let whole_turns = usize::from(card.copies) / card.moves.len();
                        for listed in card.moves {
                            let times = |clips: &[&Clip]| {
                                clips.iter().filter(|clip| same(clip, listed)).count()
                            };
                            let (listings, plays) = (times(card.moves), times(&played));
                            assert!(
                                plays >= listings * whole_turns,
                                "{}: a move played {plays} times, listed {listings}",
                                mascot.name()
                            );
                            assert!(
                                plays <= listings * (whole_turns + 1),
                                "{}: a move played {plays} times, listed {listings}",
                                mascot.name()
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn a_card_takes_its_turns_from_a_different_move_round_by_round() {
        // Which of a card's moves comes first is the round's draw, so a
        // left does not always lead its right.
        for mascot in Mascot::ALL {
            let motions = mascot.motions();
            for (kind, card) in motions.deck.iter().enumerate() {
                let distinct = (1..card.moves.len())
                    .filter(|&i| !same(card.moves[i], card.moves[0]))
                    .count();
                if distinct == 0 {
                    continue;
                }
                let firsts: Vec<bool> = (0..ROUNDS)
                    .filter_map(|number| motions.played(SEEDS[2], number, kind, 0))
                    .map(|clip| same(clip, card.moves[0]))
                    .collect();
                assert!(
                    firsts.contains(&true) && firsts.contains(&false),
                    "{}: card {kind} always leads with the same move",
                    mascot.name()
                );
            }
        }
    }

    #[test]
    fn a_late_card_stays_out_of_the_first_round() {
        for mascot in Mascot::ALL {
            let motions = mascot.motions();
            let late: Vec<usize> = (0..motions.deck.len())
                .filter(|&kind| motions.deck[kind].late)
                .collect();
            assert_eq!(
                motions.beats(1) - motions.beats(0),
                late.len() as u64,
                "{}",
                mascot.name()
            );
            for seed in SEEDS {
                let first = round(motions, seed, 0);
                let second = round(motions, seed, 1);
                for &kind in &late {
                    let holds = |cards: &[Option<(usize, u8)>]| {
                        cards.iter().flatten().any(|card| card.0 == kind)
                    };
                    assert!(
                        !holds(&first),
                        "{}: dozes in its first round",
                        mascot.name()
                    );
                    assert!(holds(&second), "{}: never dozes", mascot.name());
                }
            }
        }
        // The four that can nod off do; the seedling and the gem have no
        // late card at all.
        let dozes = |mascot: Mascot| mascot.motions().deck.iter().any(|card| card.late);
        for mascot in Mascot::ALL {
            assert_eq!(
                dozes(mascot),
                !matches!(mascot, Mascot::Sprout | Mascot::Gem),
                "{}",
                mascot.name()
            );
        }
    }

    #[test]
    fn a_decks_first_card_is_the_move_its_greeting_closes_on() {
        // …which is what the first round is told not to open with.
        for mascot in Mascot::ALL {
            let motions = mascot.motions();
            let closing = motions.greeting[2];
            assert!(
                motions.deck[0].moves.iter().any(|clip| same(clip, closing)),
                "{}",
                mascot.name()
            );
            for seed in SEEDS {
                let first = round(motions, seed, 0).into_iter().flatten().next();
                assert_ne!(first.map(|card| card.0), Some(0), "{}", mascot.name());
            }
        }
    }

    #[test]
    fn every_clip_fits_a_beat_with_a_breath_to_spare() {
        for mascot in Mascot::ALL {
            for clip in mascot.motions().clips() {
                assert!(
                    clip.len() + BEAT_BREATH_MS <= BEAT_MS,
                    "{}: a {}ms clip",
                    mascot.name(),
                    clip.len()
                );
                assert!(
                    clip.steps.iter().all(|step| step.ms > 0),
                    "{}",
                    mascot.name()
                );
            }
        }
    }

    #[test]
    fn a_clip_stands_still_for_less_than_a_breath_and_never_at_its_ends() {
        // A pause inside a clip — between two bounces, the two halves of a
        // double blink — must stay shorter than the stillness that parts
        // two clips, or one move would read as two.
        for mascot in Mascot::ALL {
            let rest = mascot.art();
            for clip in mascot.motions().clips().filter(|clip| !clip.shine) {
                let still = |step: &Step| step.art == rest;
                assert!(
                    clip.steps
                        .iter()
                        .filter(|step| still(step))
                        .all(|step| step.ms <= 160),
                    "{}: a clip pauses as long as a breath",
                    mascot.name()
                );
                assert!(
                    clip.steps.first().is_some_and(|step| !still(step))
                        && clip.steps.last().is_some_and(|step| !still(step)),
                    "{}: a clip that opens or closes standing still",
                    mascot.name()
                );
            }
        }
    }

    #[test]
    fn no_card_crowds_its_round() {
        // The deal keeps a card's copies apart for certain only while no
        // card starts out as more than half of the moves — and can part
        // every rest only while the moves outnumber them.
        for mascot in Mascot::ALL {
            let motions = mascot.motions();
            for number in [0, 1] {
                let copies = motions.copies(number);
                let moves: u64 = copies.iter().map(|&n| u64::from(n)).sum();
                let most = u64::from(copies.iter().copied().max().unwrap_or(0));
                // One move is set aside to close the round.
                assert!(most * 2 < moves, "{}: round {number}", mascot.name());
                assert!(u64::from(motions.rests) < moves, "{}", mascot.name());
            }
        }
    }

    #[test]
    fn every_round_has_several_moves_to_close_on() {
        for mascot in Mascot::ALL {
            let motions = mascot.motions();
            for number in [0, 1] {
                let singles = motions.copies(number).iter().filter(|&&n| n == 1).count();
                assert!(singles >= 4, "{}: {singles} single cards", mascot.name());
            }
            for seed in SEEDS {
                let closers: std::collections::BTreeSet<usize> = (0..ROUNDS)
                    .map(|number| motions.closer(seed, number))
                    .collect();
                assert!(closers.len() >= 4, "{}: {closers:?}", mascot.name());
            }
        }
    }

    #[test]
    fn no_two_cards_play_the_same_move() {
        for mascot in Mascot::ALL {
            let deck = mascot.motions().deck;
            for (a, first) in deck.iter().enumerate() {
                for second in &deck[a + 1..] {
                    for x in first.moves {
                        for y in second.moves {
                            assert!(!same(x, y), "{}: one move on two cards", mascot.name());
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn every_round_is_shuffled_afresh() {
        for mascot in Mascot::ALL {
            let motions = mascot.motions();
            let kinds = |seed: u64, number: u64| -> Vec<Option<usize>> {
                round(motions, seed, number)
                    .into_iter()
                    .map(|card| card.map(|card| card.0))
                    .collect()
            };
            // Round to round, and session to session.
            assert_ne!(kinds(SEEDS[2], 1), kinds(SEEDS[2], 2), "{}", mascot.name());
            assert_ne!(kinds(SEEDS[2], 1), kinds(SEEDS[3], 1), "{}", mascot.name());
            // …and the same round of the same session is always the same.
            assert_eq!(kinds(SEEDS[2], 7), kinds(SEEDS[2], 7), "{}", mascot.name());
        }
    }

    #[test]
    fn every_mascot_knows_a_score_of_moves() {
        // The repertoire the timeline is dealt from: the gem, with no eyes
        // to make faces with, a dozen and more; the rest a score.
        for mascot in Mascot::ALL {
            let mut known: Vec<&Clip> = Vec::new();
            for clip in mascot.motions().deck.iter().flat_map(|card| card.moves) {
                if !known.iter().any(|seen| same(seen, clip)) {
                    known.push(clip);
                }
            }
            let wanted = if mascot == Mascot::Gem { 13 } else { 20 };
            assert!(
                known.len() >= wanted,
                "{} knows {} moves",
                mascot.name(),
                known.len()
            );
        }
    }
}
