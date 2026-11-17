//! The frame tables as the binary carries them: **integers only**.
//!
//! A mascot's moves are written as stills, clips of stills and a deck of
//! clips (the parent module) — tables of `&'static` references, which is
//! what makes them readable. It is also what made them cost a process
//! memory of its own: every reference in a table is an address the loader
//! fills in at startup, so the pages that hold one are private to the
//! process from its first instruction, whether the animation is on or not.
//! The timeline read those tables directly once, and they came to 2,300
//! references in 52 kB (`docs/mascot-animation.md` *What it costs in
//! memory*).
//!
//! So the tables as written are read **at compile time** — and by the
//! tests, which hold the result to them — and never while the app runs.
//! [`build`] folds one mascot's [`Motions`] into a [`Built`] and
//! [`Built::fit`] cuts that to size as a [`Packed`]: the rows its stills are
//! made of, each once, end to end in one run of bytes — and every step,
//! clip and card as indices into it. That is the one thing the binary
//! keeps. There is no address in it, so there is nothing to relocate: it is
//! read-only data like any string, shared with every other process running
//! the binary and paged in only where it is read.
//!
//! The timeline reads a mascot's [`Packed`] through a [`Table`], and plays a
//! clip of it as a [`Reel`]. A test holds every mascot's table, step for
//! step, to the tables it was written as.

use super::{
    Art, DECK_KINDS, Deck, MASCOT_SHINE_FRAME, MascotPose, MascotShine, Motions, ShineBand, turn,
};

/// Rows in a still: every mascot's art is three.
pub(super) const STILL_ROWS: usize = 3;

/// The most rows one mascot's stills may be made of: a row is named by a
/// `u8`.
const MAX_ROWS: usize = 256;
/// The most bytes those rows may come to: a row's end is a `u16`.
const MAX_TEXT: usize = 16_384;
/// The most steps one mascot's clips may come to.
const MAX_STEPS: usize = 2_048;
/// The most clips one mascot may have: a clip is named by a `u8`, and
/// [`NO_CLIP`] names none.
const MAX_CLIPS: usize = 255;
/// The clip a mascot does not have — the gem's blink while it watches.
const NO_CLIP: u8 = u8::MAX;

/// One step of a clip: the still it shows, as the rows it is made of; how
/// brightly the mascot glows meanwhile, in percent; and how long it stays
/// up, in milliseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct PackedStep {
    rows: [u8; STILL_ROWS],
    glow: u8,
    ms: u16,
}

/// One clip: where its steps start among the mascot's, how many they are,
/// and the band of light it plays under ([`band_code`]).
#[derive(Debug, Clone, Copy)]
pub(super) struct PackedClip {
    first: u16,
    steps: u8,
    band: u8,
}

/// One card of the deck: how many copies a round deals, and the clips they
/// play — `moves` of them, from clip `first` on.
#[derive(Debug, Clone, Copy)]
pub(super) struct PackedCard {
    copies: u8,
    moves: u8,
    first: u8,
}

/// The rows met so far, while a mascot's tables are being folded: each
/// once, in the order first met.
struct Interner {
    seen: [&'static str; MAX_ROWS],
    hashes: [u64; MAX_ROWS],
    count: usize,
}

/// FNV-1a over `row` — what lets [`intern`] pass over the rows it is not
/// with one comparison each.
const fn hash(row: &str) -> u64 {
    let bytes = row.as_bytes();
    let mut hash: u64 = 0xCBF2_9CE4_8422_2325;
    let mut i = 0;
    while i < bytes.len() {
        hash = (hash ^ bytes[i] as u64).wrapping_mul(0x0000_0100_0000_01B3);
        i += 1;
    }
    hash
}

/// Whether two rows are the same text.
const fn same(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0;
    while i < a.len() {
        if a[i] != b[i] {
            return false;
        }
        i += 1;
    }
    true
}

/// The name of `row` among the rows met so far — a new one if it is new.
const fn intern(rows: &mut Interner, row: &'static str) -> u8 {
    let wanted = hash(row);
    let mut i = 0;
    while i < rows.count {
        if rows.hashes[i] == wanted && same(rows.seen[i], row) {
            return i as u8;
        }
        i += 1;
    }
    assert!(
        rows.count < MAX_ROWS,
        "a mascot's stills are made of more rows than a u8 names"
    );
    rows.seen[rows.count] = row;
    rows.hashes[rows.count] = wanted;
    rows.count += 1;
    (rows.count - 1) as u8
}

/// A still as the names of its rows.
const fn still(rows: &mut Interner, art: Art) -> [u8; STILL_ROWS] {
    assert!(art.len() == STILL_ROWS, "a still is three rows");
    [
        intern(rows, art[0]),
        intern(rows, art[1]),
        intern(rows, art[2]),
    ]
}

/// A band of light as one byte: `0` for none.
const fn band_code(band: Option<ShineBand>) -> u8 {
    match band {
        None => 0,
        Some(ShineBand::Sweep) => 1,
        Some(ShineBand::GlintFromLeft) => 2,
        Some(ShineBand::GlintFromRight) => 3,
        Some(ShineBand::Glitter) => 4,
    }
}

/// The band of light [`band_code`] wrote as `code`.
const fn band_of(code: u8) -> Option<ShineBand> {
    match code {
        1 => Some(ShineBand::Sweep),
        2 => Some(ShineBand::GlintFromLeft),
        3 => Some(ShineBand::GlintFromRight),
        4 => Some(ShineBand::Glitter),
        _ => None,
    }
}

/// How many clips `motions` comes to, each listing counted: the greeting's
/// three, every card's moves, the blink of its watch if it has one, and the
/// nap's five.
const fn clip_count(motions: &Motions) -> usize {
    let mut count = motions.greeting.len();
    let mut kind = 0;
    while kind < motions.deck.len() {
        count += motions.deck[kind].moves.len();
        kind += 1;
    }
    if motions.watch_blink.is_some() {
        count += 1;
    }
    count + 5
}

/// Clip `id` of `motions`, in the order [`clip_count`] counts them — the
/// order a [`Table`] names them in.
const fn clip_at(motions: &Motions, id: usize) -> &'static super::Clip {
    if id < motions.greeting.len() {
        return motions.greeting[id];
    }
    let mut first = motions.greeting.len();
    let mut kind = 0;
    while kind < motions.deck.len() {
        let moves = motions.deck[kind].moves;
        if id < first + moves.len() {
            return moves[id - first];
        }
        first += moves.len();
        kind += 1;
    }
    if let Some(blink) = motions.watch_blink {
        if id == first {
            return blink;
        }
        first += 1;
    }
    let nap = &motions.nap;
    [nap.nod_off, nap.breath, nap.wake_up, nap.jolt, nap.caught][id - first]
}

/// One mascot's tables folded, in arrays large enough for any of them: the
/// counts say how much of each is used, and [`fit`](Self::fit) cuts them to
/// it. Compile-time only — the binary keeps the [`Packed`].
pub(super) struct Built {
    /// How many bytes of `text_bytes`, ends of `row_ends`, steps of
    /// `step_table` and clips of `clip_table` are used.
    pub(super) text: usize,
    pub(super) rows: usize,
    pub(super) steps: usize,
    pub(super) clips: usize,
    text_bytes: [u8; MAX_TEXT],
    row_ends: [u16; MAX_ROWS],
    step_table: [PackedStep; MAX_STEPS],
    clip_table: [PackedClip; MAX_CLIPS],
    cards: [PackedCard; DECK_KINDS],
    kinds: u8,
    rests: u8,
    gaze: [[u8; STILL_ROWS]; 2],
    watch_blink: u8,
    nap: u8,
}

/// Fold `motions` — one mascot's tables as written — into integers.
///
/// Clips are numbered in the order they are listed: the greeting's three,
/// each card's moves in turn (so a card's are consecutive), the blink of
/// its watch, the nap's five. A clip listed twice is two clips; a row of
/// art is kept once however many stills are made of it.
pub(super) const fn build(motions: &Motions) -> Built {
    let mut rows = Interner {
        seen: [""; MAX_ROWS],
        hashes: [0; MAX_ROWS],
        count: 0,
    };
    let mut step_table = [PackedStep {
        rows: [0; STILL_ROWS],
        glow: 0,
        ms: 0,
    }; MAX_STEPS];
    let mut clip_table = [PackedClip {
        first: 0,
        steps: 0,
        band: 0,
    }; MAX_CLIPS];
    let clips = clip_count(motions);
    assert!(clips <= MAX_CLIPS, "more clips than a u8 names");
    let mut steps = 0;
    let mut id = 0;
    while id < clips {
        let clip = clip_at(motions, id);
        assert!(
            clip.steps.len() <= u8::MAX as usize,
            "a clip of too many steps"
        );
        clip_table[id] = PackedClip {
            first: steps as u16,
            steps: clip.steps.len() as u8,
            band: band_code(clip.band),
        };
        let mut i = 0;
        while i < clip.steps.len() {
            let step = clip.steps[i];
            assert!(steps < MAX_STEPS, "more steps than the table holds");
            assert!(step.ms <= u16::MAX as u64, "a step held for over a minute");
            step_table[steps] = PackedStep {
                rows: still(&mut rows, step.art),
                glow: step.glow,
                ms: step.ms as u16,
            };
            steps += 1;
            i += 1;
        }
        id += 1;
    }

    assert!(motions.deck.len() <= DECK_KINDS, "a deck of too many kinds");
    let mut cards = [PackedCard {
        copies: 0,
        moves: 0,
        first: 0,
    }; DECK_KINDS];
    let mut first = motions.greeting.len();
    let mut kind = 0;
    while kind < motions.deck.len() {
        let card = motions.deck[kind];
        cards[kind] = PackedCard {
            copies: card.copies,
            moves: card.moves.len() as u8,
            first: first as u8,
        };
        first += card.moves.len();
        kind += 1;
    }
    let (watch_blink, nap) = if motions.watch_blink.is_some() {
        (first as u8, first + 1)
    } else {
        (NO_CLIP, first)
    };
    let gaze = [
        still(&mut rows, motions.gaze[0]),
        still(&mut rows, motions.gaze[1]),
    ];

    let mut text_bytes = [0; MAX_TEXT];
    let mut row_ends = [0; MAX_ROWS];
    let mut text = 0;
    let mut row = 0;
    while row < rows.count {
        let bytes = rows.seen[row].as_bytes();
        assert!(
            text + bytes.len() <= MAX_TEXT,
            "more art than the table holds"
        );
        let mut i = 0;
        while i < bytes.len() {
            text_bytes[text] = bytes[i];
            text += 1;
            i += 1;
        }
        row_ends[row] = text as u16;
        row += 1;
    }

    Built {
        text,
        rows: rows.count,
        steps,
        clips,
        text_bytes,
        row_ends,
        step_table,
        clip_table,
        cards,
        kinds: motions.deck.len() as u8,
        rests: motions.rests,
        gaze,
        watch_blink,
        nap: nap as u8,
    }
}

impl Built {
    /// The tables cut to size — the sizes being this [`Built`]'s own counts,
    /// which a caller names as the const arguments.
    pub(super) const fn fit<
        const TEXT: usize,
        const ROWS: usize,
        const STEPS: usize,
        const CLIPS: usize,
    >(
        &self,
    ) -> Packed<TEXT, ROWS, STEPS, CLIPS> {
        assert!(
            TEXT == self.text && ROWS == self.rows && STEPS == self.steps && CLIPS == self.clips,
            "the table is cut to another's size"
        );
        let mut text = [0; TEXT];
        let mut i = 0;
        while i < TEXT {
            text[i] = self.text_bytes[i];
            i += 1;
        }
        let mut ends = [0; ROWS];
        let mut i = 0;
        while i < ROWS {
            ends[i] = self.row_ends[i];
            i += 1;
        }
        let mut steps = [self.step_table[0]; STEPS];
        let mut i = 0;
        while i < STEPS {
            steps[i] = self.step_table[i];
            i += 1;
        }
        let mut clips = [self.clip_table[0]; CLIPS];
        let mut i = 0;
        while i < CLIPS {
            clips[i] = self.clip_table[i];
            i += 1;
        }
        Packed {
            text,
            ends,
            steps,
            clips,
            cards: self.cards,
            kinds: self.kinds,
            rests: self.rests,
            gaze: self.gaze,
            watch_blink: self.watch_blink,
            nap: self.nap,
        }
    }
}

/// One mascot's frame tables as the binary keeps them. See the module docs.
pub(super) struct Packed<
    const TEXT: usize,
    const ROWS: usize,
    const STEPS: usize,
    const CLIPS: usize,
> {
    /// The rows its stills are made of, each once, end to end.
    text: [u8; TEXT],
    /// Where each of those rows ends in `text` — and so where the next
    /// starts.
    ends: [u16; ROWS],
    /// Every clip's steps, clip after clip.
    steps: [PackedStep; STEPS],
    /// The clips, in the order [`build`] numbers them.
    clips: [PackedClip; CLIPS],
    /// The deck: its first `kinds` cards.
    cards: [PackedCard; DECK_KINDS],
    kinds: u8,
    /// How many beats of a round the mascot stands still.
    rests: u8,
    /// The stills it watches the composer with: eyes left, eyes right.
    gaze: [[u8; STILL_ROWS]; 2],
    /// The blink it gives while it watches, or [`NO_CLIP`].
    watch_blink: u8,
    /// The first of its nap's five clips: nodding off, a breath, waking,
    /// the start of one found asleep, the start of one caught nodding off.
    nap: u8,
}

impl<const TEXT: usize, const ROWS: usize, const STEPS: usize, const CLIPS: usize>
    Packed<TEXT, ROWS, STEPS, CLIPS>
{
    /// These tables as the timeline reads them.
    pub(super) fn table(&'static self) -> Table {
        let kinds = usize::from(self.kinds).min(DECK_KINDS);
        Table {
            rows: Rows {
                text: &self.text,
                ends: &self.ends,
            },
            steps: &self.steps,
            clips: &self.clips,
            cards: &self.cards[..kinds],
            rests: self.rests,
            gaze: self.gaze,
            watch_blink: self.watch_blink,
            nap: self.nap,
        }
    }
}

/// The rows a mascot's stills are made of.
#[derive(Debug, Clone, Copy)]
struct Rows {
    text: &'static [u8],
    ends: &'static [u16],
}

impl Rows {
    /// Row `row`, as text.
    fn row(self, row: u8) -> &'static str {
        let row = usize::from(row);
        let start = row
            .checked_sub(1)
            .and_then(|before| self.ends.get(before))
            .map_or(0, |&end| usize::from(end));
        let end = self.ends.get(row).map_or(start, |&end| usize::from(end));
        self.text
            .get(start..end)
            .and_then(|bytes| std::str::from_utf8(bytes).ok())
            .unwrap_or_default()
    }

    /// The still made of `rows`.
    fn art(self, rows: [u8; STILL_ROWS]) -> [&'static str; STILL_ROWS] {
        rows.map(|row| self.row(row))
    }
}

/// One mascot's frame tables, as the timeline reads them: everything
/// [`Motions`] says, looked up in a [`Packed`].
#[derive(Debug, Clone, Copy)]
pub(super) struct Table {
    rows: Rows,
    steps: &'static [PackedStep],
    clips: &'static [PackedClip],
    cards: &'static [PackedCard],
    rests: u8,
    gaze: [[u8; STILL_ROWS]; 2],
    watch_blink: u8,
    nap: u8,
}

impl Table {
    /// Clip `id` — one of no steps for an `id` the table does not hold,
    /// which plays as the rest pose.
    fn reel(&self, id: usize) -> Reel {
        let steps = self.clips.get(id).map(|clip| {
            let first = usize::from(clip.first);
            (first..first + usize::from(clip.steps), band_of(clip.band))
        });
        let (steps, band) = match steps {
            Some((steps, band)) => (self.steps.get(steps).unwrap_or_default(), band),
            None => (&[][..], None),
        };
        Reel {
            rows: self.rows,
            steps,
            band,
        }
    }

    /// How many clips the table holds.
    #[cfg(test)]
    pub(super) const fn clips(&self) -> usize {
        self.clips.len()
    }

    /// Clip `id`, in the order [`build`] numbers them.
    #[cfg(test)]
    pub(super) fn clip(&self, id: usize) -> Reel {
        self.reel(id)
    }

    /// The greeting's clip for `slot`: the sweep, the mascot's own move, a
    /// blink.
    pub(super) fn greeting(&self, slot: usize) -> Reel {
        self.reel(slot)
    }

    /// The deck as a round deals it: how many copies of each kind, and the
    /// rests.
    pub(super) fn deck(&self) -> Deck {
        let mut copies = [0; DECK_KINDS];
        for (kind, card) in self.cards.iter().enumerate() {
            copies[kind] = card.copies;
        }
        Deck {
            copies,
            kinds: self.cards.len(),
            rests: self.rests,
        }
    }

    /// The clip the `nth` copy of card `kind` plays in `round`: the card's
    /// moves take turns, from one the round draws ([`turn`]).
    pub(super) fn played(&self, seed: u64, round: u64, kind: usize, nth: u8) -> Option<Reel> {
        let card = self.cards.get(kind)?;
        let index = turn(seed, round, kind, nth, usize::from(card.moves))?;
        Some(self.reel(usize::from(card.first) + index))
    }

    /// The stills it watches the composer with: its eyes turned left, and
    /// turned right.
    pub(super) fn gaze(&self) -> [[&'static str; STILL_ROWS]; 2] {
        self.gaze.map(|rows| self.rows.art(rows))
    }

    /// The blink it gives while it watches — `None` for the gem, which has
    /// no eyes to blink.
    pub(super) fn watch_blink(&self) -> Option<Reel> {
        (self.watch_blink != NO_CLIP).then(|| self.reel(usize::from(self.watch_blink)))
    }

    /// How it naps.
    pub(super) fn nap(&self) -> Naps {
        let clip = |nth: usize| self.reel(usize::from(self.nap) + nth);
        Naps {
            nod_off: clip(0),
            breath: clip(1),
            wake_up: clip(2),
            jolt: clip(3),
            caught: clip(4),
        }
    }
}

/// How a mascot naps, as clips to play ([`Nap`](super::Nap)): nodding off,
/// one breath asleep, waking by itself — and the start a key gives one
/// found sound asleep, and one caught still nodding off.
#[derive(Debug, Clone, Copy)]
pub(super) struct Naps {
    pub(super) nod_off: Reel,
    pub(super) breath: Reel,
    pub(super) wake_up: Reel,
    pub(super) jolt: Reel,
    pub(super) caught: Reel,
}

/// One step of a clip as it was written: its still, how long it stays up in
/// milliseconds, and how brightly the mascot glows meanwhile.
#[cfg(test)]
pub(super) type WrittenStep = ([&'static str; STILL_ROWS], u64, u8);

/// A clip, as the timeline plays it: its steps out of a [`Table`], and the
/// band of light it plays under.
#[derive(Debug, Clone, Copy)]
pub(super) struct Reel {
    rows: Rows,
    steps: &'static [PackedStep],
    band: Option<ShineBand>,
}

impl Reel {
    /// The clip's whole length, in milliseconds.
    pub(super) fn len(&self) -> u64 {
        self.steps.iter().map(|step| u64::from(step.ms)).sum()
    }

    /// The step on screen `at` milliseconds into the clip and how long it
    /// has left, or `None` once the clip is over.
    fn step_at(&self, at: u64) -> Option<(PackedStep, u64)> {
        let mut end = 0;
        self.steps.iter().find_map(|step| {
            end += u64::from(step.ms);
            (at < end).then(|| (*step, end - at))
        })
    }

    /// The pose `at` milliseconds into the clip, or `None` once it is over.
    pub(super) fn pose_at(&self, at: u64) -> Option<MascotPose> {
        let (step, _) = self.step_at(at)?;
        Some(MascotPose {
            art: self.rows.art(step.rows),
            shine: self.band.map(|band| MascotShine {
                band,
                // Millisecond counts of a clip well under a minute: exact in
                // f32.
                progress: at as f32 / self.len() as f32,
            }),
            glow: step.glow,
        })
    }

    /// How long the pose `at` milliseconds into the clip holds: to the end
    /// of its step, or — under a band of light, which moves on every frame
    /// — one [`MASCOT_SHINE_FRAME`] at most.
    pub(super) fn holds_at(&self, at: u64) -> Option<u64> {
        let (_, left) = self.step_at(at)?;
        let frame = u64::try_from(MASCOT_SHINE_FRAME.as_millis()).unwrap_or(u64::MAX);
        Some(if self.band.is_some() {
            left.min(frame)
        } else {
            left
        })
    }

    /// The clip's steps, each as it was written ([`WrittenStep`]).
    #[cfg(test)]
    pub(super) fn steps(&self) -> Vec<WrittenStep> {
        self.steps
            .iter()
            .map(|step| (self.rows.art(step.rows), u64::from(step.ms), step.glow))
            .collect()
    }

    /// The band of light the clip plays under.
    #[cfg(test)]
    pub(super) const fn band(&self) -> Option<ShineBand> {
        self.band
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::super::Mascot;
    use super::*;

    #[test]
    fn a_row_of_art_is_kept_once_however_many_stills_are_made_of_it() {
        // What makes the tables small: a blink redraws one row of three,
        // so most stills share two rows with the art at rest.
        for mascot in Mascot::ALL {
            let table = mascot.table();
            let count = table.rows.ends.len();
            let rows: BTreeSet<&str> = (0..count)
                .map(|row| table.rows.row(u8::try_from(row).expect("a u8 names a row")))
                .collect();
            // (An empty row is a row like any other — a mascot sunk below
            // the floor of its block has one — and kept once too.)
            assert_eq!(rows.len(), count, "{}", mascot.name());
            // …and every one of them is drawn by some step.
            let drawn: BTreeSet<u8> = table
                .steps
                .iter()
                .flat_map(|step| step.rows)
                .chain(table.gaze.into_iter().flatten())
                .collect();
            assert_eq!(drawn.len(), count, "{}", mascot.name());
            assert_eq!(
                usize::from(*table.rows.ends.last().expect("rows")),
                table.rows.text.len(),
                "{}: the rows are the whole of the text",
                mascot.name()
            );
        }
    }

    #[test]
    fn what_a_table_does_not_hold_is_blank_and_plays_as_the_rest_pose() {
        // Nothing in the tables can name a row or a clip that is not
        // there — they are folded from the ones as written — but a lookup
        // that misses must still never panic a frame.
        let table = Mascot::Crest.table();
        assert_eq!(table.rows.row(u8::MAX), "");
        let none = table.clip(table.clips());
        assert_eq!(none.len(), 0);
        assert_eq!(none.pose_at(0), None);
        assert_eq!(none.holds_at(0), None);
        assert!(table.played(0, 0, DECK_KINDS, 0).is_none());
    }

    #[test]
    fn a_band_of_light_reads_back_as_it_was_written() {
        let bands = [
            None,
            Some(ShineBand::Sweep),
            Some(ShineBand::GlintFromLeft),
            Some(ShineBand::GlintFromRight),
            Some(ShineBand::Glitter),
        ];
        for band in bands {
            assert_eq!(band_of(band_code(band)), band);
        }
    }
}
