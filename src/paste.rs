//! Paste-burst detection — a pure state machine ported from openai/codex.
//!
//! Characters that keep arriving within [`BURST_CHAR_INTERVAL`] of each other are
//! a *burst* (a paste, or very fast typing) once [`BURST_MIN_CHARS`] pile up. The
//! event loop uses this to stop asking for an **immediate** frame per burst
//! character (`schedule_frame_in` requests one a beat out instead), while a lone
//! keystroke still paints at once. The actual coalescing — one paint per
//! [`crate::frame::MIN_FRAME_INTERVAL`] no matter how many requests — is the
//! frame scheduler's rate limiter; the scheduler keeps the *soonest* pending
//! deadline, so the burst request never postpones a frame, it only avoids
//! demanding extra ones (codex's scheduler folds the same way). Decisions come
//! from injected `Instant`s, so it is unit-tested with no clock.

use std::ops::Range;
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

// ===== large-paste placeholders =====
//
// A *different* job from the burst detector above (which only coalesces
// redraws): when a real bracketed paste arrives, anything longer than
// [`LARGE_PASTE_CHAR_THRESHOLD`] is shown in the composer as a compact
// `[Pasted Content N chars]` placeholder, with the real text remembered and
// spliced back in on send. See `docs/paste.md`.

/// A paste of more than this many characters is replaced by a placeholder in the
/// composer instead of inserted verbatim. Matches codex's
/// `LARGE_PASTE_CHAR_THRESHOLD`.
pub const LARGE_PASTE_CHAR_THRESHOLD: usize = 1000;

/// The placeholder string for a paste of `char_count` characters, disambiguated
/// against the placeholders already pending in `existing` (a slice of
/// `(placeholder, real_text)` pairs). A port of codex's
/// `next_large_paste_placeholder`: the base form is `[Pasted Content N chars]`,
/// and a same-base collision gets a ` #2`, ` #3`, … suffix (max existing + 1) so
/// two same-size pastes never share a placeholder.
#[must_use]
pub fn next_paste_placeholder(char_count: usize, existing: &[(String, String)]) -> String {
    let base = format!("[Pasted Content {char_count} chars]");
    let prefix = format!("{base} #");
    let mut max_suffix = 0usize;
    for (placeholder, _) in existing {
        if placeholder == &base {
            max_suffix = max_suffix.max(1);
        } else if let Some(suffix) = placeholder.strip_prefix(&prefix)
            && let Ok(value) = suffix.parse::<usize>()
        {
            max_suffix = max_suffix.max(value);
        }
    }
    if max_suffix == 0 {
        base
    } else {
        format!("{base} #{}", max_suffix + 1)
    }
}

/// The placeholder for the next pasted **image**, given the image attachments
/// already in the composer (a slice of `(placeholder, path)` pairs — generic
/// over the payload, only the placeholder strings are read). The form is codex's
/// `[Image #N]`, with `N` the **max existing `#k` + 1** (or 1 for the first).
///
/// Numbering by max-existing rather than count keeps labels unique even after a
/// deletion (delete `[Image #2]` of three and the next is `[Image #4]`, not a
/// duplicate `[Image #3]`): unlike codex's element-id model, deletion and the
/// send-time channel match images **by placeholder string**, so a collision
/// would be ambiguous. Gaps in the numbers are harmless. See `docs/image-paste.md`.
#[must_use]
pub fn next_image_placeholder<T>(existing: &[(String, T)]) -> String {
    let mut max = 0usize;
    for (placeholder, _) in existing {
        if let Some(n) = placeholder
            .strip_prefix("[Image #")
            .and_then(|rest| rest.strip_suffix(']'))
            .and_then(|digits| digits.parse::<usize>().ok())
        {
            max = max.max(n);
        }
    }
    format!("[Image #{}]", max + 1)
}

/// The `[Image #N]` placeholders present in `text`, deduplicated and sorted
/// ascending by `N` — which **is** attach order, because
/// [`next_image_placeholder`] numbers by max-existing + 1 (a later attachment
/// always gets a larger `N`, even across deletions). The interrupt-undo path
/// zips this with the turn's image paths (also attach-ordered) to rebuild the
/// composer's `(placeholder, path)` pairs. See `docs/image-paste.md`.
#[must_use]
pub fn image_placeholders_in(text: &str) -> Vec<String> {
    let mut numbers: Vec<usize> = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("[Image #") {
        rest = &rest[start + "[Image #".len()..];
        if let Some(end) = rest.find(']')
            && let Ok(n) = rest[..end].parse::<usize>()
            && !numbers.contains(&n)
        {
            numbers.push(n);
        }
    }
    numbers.sort_unstable();
    numbers
        .into_iter()
        .map(|n| format!("[Image #{n}]"))
        .collect()
}

/// The longest placeholder in `pastes` that `text[i..]` starts with, if any.
/// Longest-match so a base `[Pasted Content N chars]` can't shadow its `… #N`
/// extension (of which it is a prefix). Shared by the left-to-right walk in
/// [`expand_pastes`] and [`placeholder_to_delete`]. Generic over the payload
/// `T` (text `String` or an image `PathBuf`) — only the placeholder string
/// (`.0`) is read here.
fn longest_placeholder_at<'a, T>(
    text: &str,
    i: usize,
    pastes: &'a [(String, T)],
) -> Option<&'a (String, T)> {
    pastes
        .iter()
        .filter(|(placeholder, _)| text[i..].starts_with(placeholder.as_str()))
        .max_by_key(|(placeholder, _)| placeholder.len())
}

/// Splice every pasted placeholder in `text` back to its real content, given the
/// `(placeholder, real_text)` pairs. Scans `text` left-to-right, and at each
/// position substitutes the **longest** matching placeholder (so a base
/// `[Pasted Content N chars]` doesn't shadow its `… #2` extension, of which it is
/// a prefix). Substituted content is emitted straight to the output and never
/// re-scanned, so real content that happens to contain another placeholder
/// string can't be re-expanded. A placeholder the user has since deleted is
/// simply never matched, so its content is dropped.
#[must_use]
pub fn expand_pastes(text: &str, pastes: &[(String, String)]) -> String {
    if pastes.is_empty() {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < text.len() {
        if let Some((placeholder, content)) = longest_placeholder_at(text, i, pastes) {
            out.push_str(content);
            i += placeholder.len();
        } else {
            // No placeholder here — copy one whole character and advance.
            let ch = text[i..].chars().next().expect("i < text.len()");
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    out
}

/// The byte span of the pasted placeholder the cursor is "on", for an **atomic**
/// Backspace/Delete that removes the whole `[Pasted Content N chars]` placeholder
/// in one keystroke, or `None` when the cursor isn't on one. With `backward`
/// (Backspace) a cursor at the placeholder's end — or anywhere inside it —
/// removes the whole placeholder (a cursor at its *start* deletes the character
/// before it instead); with `!backward` (Delete) a cursor at the start or inside
/// removes it (a cursor at its end deletes the character after). Placeholders are
/// matched longest-first, like [`expand_pastes`]. See `docs/paste.md`.
#[must_use]
pub fn placeholder_to_delete<T>(
    text: &str,
    cursor: usize,
    pastes: &[(String, T)],
    backward: bool,
) -> Option<Range<usize>> {
    let mut i = 0;
    while i < text.len() {
        if let Some((placeholder, _)) = longest_placeholder_at(text, i, pastes) {
            let span = i..i + placeholder.len();
            let on_it = if backward {
                // Backspace: at the end, or anywhere strictly inside.
                span.start < cursor && cursor <= span.end
            } else {
                // Delete: at the start, or anywhere strictly inside.
                span.start <= cursor && cursor < span.end
            };
            if on_it {
                return Some(span);
            }
            i += placeholder.len();
        } else {
            let ch = text[i..].chars().next().expect("i < text.len()");
            i += ch.len_utf8();
        }
    }
    None
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

    // ===== large-paste placeholders =====

    /// Build a `(placeholder, content)` pair list from placeholder strings (the
    /// content is irrelevant to `next_paste_placeholder`).
    fn pairs(placeholders: &[&str]) -> Vec<(String, String)> {
        placeholders
            .iter()
            .map(|p| ((*p).to_string(), "…".to_string()))
            .collect()
    }

    #[test]
    fn placeholder_is_codex_format() {
        assert_eq!(
            next_paste_placeholder(3907, &[]),
            "[Pasted Content 3907 chars]"
        );
    }

    #[test]
    fn placeholder_disambiguates_same_size_pastes() {
        // A second paste of the same size gets ` #2`, a third ` #3`.
        let one = pairs(&["[Pasted Content 1500 chars]"]);
        assert_eq!(
            next_paste_placeholder(1500, &one),
            "[Pasted Content 1500 chars] #2"
        );
        let two = pairs(&[
            "[Pasted Content 1500 chars]",
            "[Pasted Content 1500 chars] #2",
        ]);
        assert_eq!(
            next_paste_placeholder(1500, &two),
            "[Pasted Content 1500 chars] #3"
        );
    }

    #[test]
    fn placeholder_ignores_other_sizes() {
        // A pending paste of a *different* size doesn't bump our suffix.
        let other = pairs(&["[Pasted Content 2000 chars]"]);
        assert_eq!(
            next_paste_placeholder(1500, &other),
            "[Pasted Content 1500 chars]"
        );
    }

    #[test]
    fn expand_substitutes_the_placeholder() {
        let pastes = pairs2(&[("[Pasted Content 1500 chars]", "REAL")]);
        assert_eq!(
            expand_pastes("see [Pasted Content 1500 chars] ok", &pastes),
            "see REAL ok"
        );
    }

    #[test]
    fn expand_handles_multiple_pastes_in_order() {
        let pastes = pairs2(&[
            ("[Pasted Content 1500 chars]", "AAA"),
            ("[Pasted Content 1500 chars] #2", "BBB"),
        ]);
        // Note the #2 placeholder precedes the base one in the text — expansion
        // is by position, so both land correctly regardless of pair order.
        assert_eq!(
            expand_pastes(
                "x [Pasted Content 1500 chars] #2 y [Pasted Content 1500 chars] z",
                &pastes
            ),
            "x BBB y AAA z"
        );
    }

    #[test]
    fn expand_drops_a_deleted_placeholder() {
        // The user pasted then deleted the placeholder: it isn't in the text, so
        // its content is simply not spliced in.
        let pastes = pairs2(&[("[Pasted Content 1500 chars]", "REAL")]);
        assert_eq!(expand_pastes("nothing here", &pastes), "nothing here");
    }

    #[test]
    fn expand_is_a_noop_without_pastes() {
        assert_eq!(expand_pastes("plain text", &[]), "plain text");
    }

    #[test]
    fn expand_does_not_re_expand_inside_pasted_content() {
        // The real content of the first paste literally contains the second
        // paste's placeholder; expansion must not re-expand it.
        let pastes = pairs2(&[
            (
                "[Pasted Content 1500 chars]",
                "literal [Pasted Content 9 chars]",
            ),
            ("[Pasted Content 9 chars]", "SHOULD-NOT-APPEAR"),
        ]);
        assert_eq!(
            expand_pastes("[Pasted Content 1500 chars]", &pastes),
            "literal [Pasted Content 9 chars]"
        );
    }

    /// `(placeholder, content)` pairs from explicit string pairs.
    fn pairs2(items: &[(&str, &str)]) -> Vec<(String, String)> {
        items
            .iter()
            .map(|(p, c)| ((*p).to_string(), (*c).to_string()))
            .collect()
    }

    // ===== atomic placeholder deletion =====

    const PH: &str = "[Pasted Content 1500 chars]"; // 27 bytes

    #[test]
    fn backspace_at_placeholder_end_targets_the_whole_placeholder() {
        let pastes = pairs(&[PH]);
        let text = PH; // composer holds just the placeholder
        // Cursor at the end (just typed/pasted) → remove the whole thing.
        assert_eq!(
            placeholder_to_delete(text, PH.len(), &pastes, true),
            Some(0..PH.len())
        );
    }

    #[test]
    fn backspace_inside_placeholder_targets_the_whole_placeholder() {
        let pastes = pairs(&[PH]);
        assert_eq!(
            placeholder_to_delete(PH, 5, &pastes, true),
            Some(0..PH.len()),
            "a cursor inside still removes the whole placeholder"
        );
    }

    #[test]
    fn backspace_at_placeholder_start_is_not_atomic() {
        let pastes = pairs(&[PH]);
        let text = format!("x{PH}");
        // Cursor right before the placeholder (after the 'x'): Backspace should
        // delete the 'x', not the placeholder.
        assert_eq!(placeholder_to_delete(&text, 1, &pastes, true), None);
    }

    #[test]
    fn delete_forward_at_placeholder_start_targets_it() {
        let pastes = pairs(&[PH]);
        assert_eq!(
            placeholder_to_delete(PH, 0, &pastes, false),
            Some(0..PH.len())
        );
    }

    #[test]
    fn delete_forward_at_placeholder_end_is_not_atomic() {
        let pastes = pairs(&[PH]);
        let text = format!("{PH}x");
        assert_eq!(placeholder_to_delete(&text, PH.len(), &pastes, false), None);
    }

    #[test]
    fn cursor_off_any_placeholder_targets_nothing() {
        let pastes = pairs(&[PH]);
        let text = format!("hi {PH}");
        // Cursor in the "hi " prefix.
        assert_eq!(placeholder_to_delete(&text, 1, &pastes, true), None);
    }

    #[test]
    fn atomic_delete_picks_the_right_one_among_prefix_duplicates() {
        // The base placeholder is a prefix of the `#2` one; the cursor sits at
        // the end of the *second* (longer) occurrence.
        let base = PH;
        let second = format!("{PH} #2"); // 30 bytes
        let pastes = pairs2(&[(base, "A"), (second.as_str(), "B")]);
        let text = format!("{second} {base}");
        // Cursor at the end of `second` (byte 30) → its full 0..30 span.
        assert_eq!(
            placeholder_to_delete(&text, second.len(), &pastes, true),
            Some(0..second.len())
        );
    }

    // ===== image placeholders (Ctrl+V image paste, docs/image-paste.md) =====

    /// `(placeholder, path)` pairs for an image-attachment list (the path is
    /// irrelevant to placeholder numbering / deletion span math).
    fn image_pairs(placeholders: &[&str]) -> Vec<(String, std::path::PathBuf)> {
        placeholders
            .iter()
            .map(|p| ((*p).to_string(), std::path::PathBuf::from("/tmp/x.png")))
            .collect()
    }

    #[test]
    fn image_placeholder_is_codex_format() {
        assert_eq!(
            next_image_placeholder::<std::path::PathBuf>(&[]),
            "[Image #1]"
        );
    }

    #[test]
    fn image_placeholder_increments_past_the_existing_ones() {
        let one = image_pairs(&["[Image #1]"]);
        assert_eq!(next_image_placeholder(&one), "[Image #2]");
        let two = image_pairs(&["[Image #1]", "[Image #2]"]);
        assert_eq!(next_image_placeholder(&two), "[Image #3]");
    }

    #[test]
    fn image_placeholder_uses_max_plus_one_after_a_deletion() {
        // #2 deleted from {#1,#2,#3} leaves {#1,#3}; the next is max(3)+1 = #4.
        // A gap is fine — labels must stay unique because we match by string.
        let gapped = image_pairs(&["[Image #1]", "[Image #3]"]);
        assert_eq!(next_image_placeholder(&gapped), "[Image #4]");
    }

    #[test]
    fn image_placeholders_in_finds_them_in_ascending_number_order() {
        // Ascending N is attach order (numbering is max+1), so the undo path
        // can zip these with the attach-ordered image paths.
        assert_eq!(
            image_placeholders_in("[Image #3] before [Image #1] and text"),
            vec!["[Image #1]".to_string(), "[Image #3]".to_string()]
        );
    }

    #[test]
    fn image_placeholders_in_ignores_duplicates_and_junk() {
        assert_eq!(
            image_placeholders_in("[Image #1] again [Image #1], [Image #x], [Image #"),
            vec!["[Image #1]".to_string()]
        );
        assert!(image_placeholders_in("no placeholders here").is_empty());
    }

    #[test]
    fn placeholder_to_delete_is_generic_over_an_image_path_payload() {
        // The deletion span math reads only the placeholder string, so the same
        // helper serves the `(String, PathBuf)` image list as the text one.
        let imgs = image_pairs(&["[Image #1]"]);
        let text = "[Image #1]";
        assert_eq!(
            placeholder_to_delete(text, text.len(), &imgs, true),
            Some(0..text.len())
        );
    }
}
