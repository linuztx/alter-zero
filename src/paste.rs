//! Paste-burst detection — a pure state machine ported from openai/codex.
//!
//! Characters that keep arriving within [`BURST_CHAR_INTERVAL`] of each other are
//! a *burst* (a paste, or very fast typing) once [`BURST_MIN_CHARS`] pile up. The
//! event loop uses this to **defer** the redraw to the tail of the burst — one
//! coalesced paint instead of one per character — while a lone keystroke still
//! paints at once. Decisions come from injected `Instant`s, so it is unit-tested
//! with no clock.

use std::time::{Duration, Instant};

/// The longest gap between two characters that still counts them as part of the
/// same burst. Matches codex's `PASTE_BURST_CHAR_INTERVAL`.
pub const BURST_CHAR_INTERVAL: Duration = Duration::from_millis(8);

/// How many fast characters in a row constitute a burst.
pub const BURST_MIN_CHARS: u16 = 3;

/// Tracks the run of consecutive fast characters to tell a paste / fast-type
/// burst from ordinary typing.
#[derive(Debug, Default)]
pub struct PasteBurst {
    /// When the previous character arrived; `None` before the first (or after a
    /// [`reset`]).
    ///
    /// [`reset`]: PasteBurst::reset
    last_at: Option<Instant>,
    /// Length of the current run of fast (within-interval) characters.
    run: u16,
}

impl PasteBurst {
    /// A fresh detector with no characters seen yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Note a plain character typed at `now`, returning whether that puts us in a
    /// burst (so the caller can coalesce the redraw). A character within
    /// [`BURST_CHAR_INTERVAL`] of the previous one extends the run; a slower one
    /// starts a new run of length 1.
    pub fn note_char(&mut self, now: Instant) -> bool {
        let fast = self
            .last_at
            .is_some_and(|prev| now.duration_since(prev) <= BURST_CHAR_INTERVAL);
        self.run = if fast { self.run.saturating_add(1) } else { 1 };
        self.last_at = Some(now);
        self.is_burst()
    }

    /// Are we currently in a burst — at least [`BURST_MIN_CHARS`] fast characters
    /// in a row?
    #[must_use]
    pub fn is_burst(&self) -> bool {
        self.run >= BURST_MIN_CHARS
    }

    /// End any burst. Called on a non-character event (Enter, a navigation key, a
    /// resize) so the next character starts a fresh run.
    pub fn reset(&mut self) {
        self.last_at = None;
        self.run = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `n` characters spaced `gap` apart starting at `base`, fed in order; returns
    /// the burst flag after each.
    fn feed(burst: &mut PasteBurst, base: Instant, gaps: &[Duration]) -> Vec<bool> {
        let mut at = base;
        let mut out = vec![burst.note_char(at)];
        for gap in gaps {
            at += *gap;
            out.push(burst.note_char(at));
        }
        out
    }

    #[test]
    fn a_single_character_is_not_a_burst() {
        let mut burst = PasteBurst::new();
        assert!(!burst.note_char(Instant::now()));
        assert!(!burst.is_burst());
    }

    #[test]
    fn two_fast_characters_are_not_yet_a_burst() {
        let mut burst = PasteBurst::new();
        let base = Instant::now();
        let flags = feed(&mut burst, base, &[Duration::from_millis(2)]);
        assert_eq!(flags, vec![false, false]);
    }

    #[test]
    fn three_fast_characters_make_a_burst() {
        let mut burst = PasteBurst::new();
        let base = Instant::now();
        // 0ms, +2ms, +4ms — all within the 8ms interval.
        let flags = feed(
            &mut burst,
            base,
            &[Duration::from_millis(2), Duration::from_millis(2)],
        );
        assert_eq!(flags, vec![false, false, true], "the 3rd fast char bursts");
        assert!(burst.is_burst());
    }

    #[test]
    fn a_slow_character_breaks_the_run() {
        let mut burst = PasteBurst::new();
        let base = Instant::now();
        // Three fast (burst), then a long pause resets the run to 1.
        let flags = feed(
            &mut burst,
            base,
            &[
                Duration::from_millis(2),
                Duration::from_millis(2),
                Duration::from_millis(50),
            ],
        );
        assert_eq!(
            flags,
            vec![false, false, true, false],
            "the slow 4th char ends the burst"
        );
        assert!(!burst.is_burst());
    }

    #[test]
    fn reset_ends_the_burst() {
        let mut burst = PasteBurst::new();
        let base = Instant::now();
        feed(
            &mut burst,
            base,
            &[Duration::from_millis(2), Duration::from_millis(2)],
        );
        assert!(burst.is_burst());
        burst.reset();
        assert!(!burst.is_burst(), "reset clears the run");
        // After a reset the next char starts a fresh run of 1.
        assert!(!burst.note_char(base + Duration::from_millis(4)));
    }
}
