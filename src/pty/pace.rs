//! When a running command's cell is sent its next update
//! (`docs/interactive-shell.md` *Streaming the running cell*).
//!
//! A program redrawing a line writes each frame in pieces — npm's spinner
//! sends a column move, an erase and the glyph as three writes — and the
//! reader can take them apart. An update sampled between the pieces shows
//! the line the erase has just blanked: the running cell empties, falls back
//! to `⎿ Running…` and moves the box a row each way. Worse, the sampler used
//! to lock onto that moment: the read that woke it was the first piece of a
//! frame, so update after update fell due exactly there and the spinner
//! hardly reached the screen.
//!
//! So an update is sampled at a **frame boundary**. It falls due every
//! interval and is taken once the output has been still for
//! [`FRAME_SETTLE`] — the pieces of one frame land microseconds apart, the
//! frames tens of milliseconds — or, for output that never pauses, once it
//! has waited [`MAX_HOLD`].

use std::time::{Duration, Instant};

/// How long the output must be still before an update is sampled: far
/// longer than the gaps between the pieces of one frame, shorter than the
/// gaps between frames of anything that animates (npm's spinner draws every
/// 80 ms, a 30 fps display every 33).
pub const FRAME_SETTLE: Duration = Duration::from_millis(10);

/// The longest a due update waits for the output to pause: output that
/// never stops still reaches the cell, an interval plus this apart at worst.
pub const MAX_HOLD: Duration = Duration::from_millis(100);

/// See the module docs.
#[derive(Debug, Clone)]
pub struct Pace {
    /// The least time between two updates.
    interval: Duration,
    /// When the last update was sampled.
    last: Option<Instant>,
    /// When the update now due fell due, while it waits for a pause.
    held: Option<Instant>,
}

impl Pace {
    /// Updates at most every `interval`.
    #[must_use]
    pub const fn new(interval: Duration) -> Self {
        Self {
            interval,
            last: None,
            held: None,
        }
    }

    /// Sample an update at `now`, the output last written at `output`
    /// (`None` before any)? Due once `interval` has passed since the last
    /// one, then held until the output has been still for [`FRAME_SETTLE`]
    /// or the hold reaches [`MAX_HOLD`]. Saying yes starts the next interval.
    pub fn due(&mut self, now: Instant, output: Option<Instant>) -> bool {
        if self
            .last
            .is_some_and(|last| now.saturating_duration_since(last) < self.interval)
        {
            return false;
        }
        let held = *self.held.get_or_insert(now);
        let still = output.is_none_or(|at| now.saturating_duration_since(at) >= FRAME_SETTLE);
        if !still && now.saturating_duration_since(held) < MAX_HOLD {
            return false;
        }
        self.last = Some(now);
        self.held = None;
        true
    }

    /// How long until an update held for a pause may be sampled, at `now`
    /// with the output last written at `output` — `None` when none is held.
    /// What a waiting loop sleeps at most, so the sample lands in the pause
    /// rather than at the loop's next poll, which a steady stream of frames
    /// might never leave quiet.
    #[must_use]
    pub fn hold_left(&self, now: Instant, output: Option<Instant>) -> Option<Duration> {
        let held = self.held?;
        let quiet = output.map_or(FRAME_SETTLE, |at| now.saturating_duration_since(at));
        let settle = FRAME_SETTLE.saturating_sub(quiet);
        let cap = MAX_HOLD.saturating_sub(now.saturating_duration_since(held));
        Some(settle.min(cap))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const INTERVAL: Duration = Duration::from_millis(50);

    #[test]
    fn an_update_is_due_an_interval_after_the_last() {
        let t0 = Instant::now();
        let mut pace = Pace::new(INTERVAL);
        assert!(pace.due(t0, None), "the first is due at once");
        assert!(!pace.due(t0 + INTERVAL / 2, None));
        assert!(pace.due(t0 + INTERVAL, None));
    }

    #[test]
    fn a_due_update_waits_for_the_output_to_pause() {
        // Output just landed: a frame may be half written — its erase in,
        // its text not yet.
        let t0 = Instant::now();
        let mut pace = Pace::new(INTERVAL);
        assert!(!pace.due(t0, Some(t0)));
        assert_eq!(pace.hold_left(t0, Some(t0)), Some(FRAME_SETTLE));
        // The frame's next piece restarts the pause.
        let piece = t0 + Duration::from_millis(1);
        assert!(!pace.due(piece, Some(piece)));
        assert_eq!(
            pace.hold_left(piece + Duration::from_millis(4), Some(piece)),
            Some(FRAME_SETTLE - Duration::from_millis(4))
        );
        // Still for long enough: the frame is whole.
        assert!(pace.due(piece + FRAME_SETTLE, Some(piece)));
        assert_eq!(pace.hold_left(piece + FRAME_SETTLE, Some(piece)), None);
    }

    #[test]
    fn output_that_never_pauses_is_sampled_anyway() {
        let t0 = Instant::now();
        let mut pace = Pace::new(INTERVAL);
        let mut now = t0;
        while now < t0 + MAX_HOLD {
            assert!(!pace.due(now, Some(now)), "held at {:?}", now - t0);
            now += Duration::from_millis(5);
        }
        assert!(pace.due(now, Some(now)), "the hold is capped");
        assert_eq!(
            pace.hold_left(now, Some(now)),
            None,
            "the next is not due yet"
        );
    }

    #[test]
    fn the_hold_never_outlasts_its_cap() {
        let t0 = Instant::now();
        let mut pace = Pace::new(INTERVAL);
        assert!(!pace.due(t0, Some(t0)));
        let late = t0 + MAX_HOLD - Duration::from_millis(3);
        assert_eq!(
            pace.hold_left(late, Some(late)),
            Some(Duration::from_millis(3)),
            "a waiter wakes when the cap lets the update go, not a whole settle later"
        );
    }

    #[test]
    fn an_update_due_while_nothing_was_written_goes_at_once() {
        let t0 = Instant::now();
        let mut pace = Pace::new(INTERVAL);
        assert!(pace.due(t0, Some(t0 - FRAME_SETTLE)));
        assert!(
            pace.due(t0 + INTERVAL, Some(t0)),
            "the output went still long ago"
        );
    }
}
