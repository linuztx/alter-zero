//! Cross-session input-history file — the pure core (see `docs/history-persistence.md`).
//!
//! The composer's ↑/↓ recall and Ctrl+R reverse search read
//! [`InputHistory::entries`]; to make them span sessions we persist every
//! submitted input to an append-only JSONL file and seed `entries` from it at
//! startup. This module owns *only* the on-disk format — one codex-shaped JSON
//! object per line:
//!
//! ```text
//! {"session_id":"<id>","ts":<unix_seconds>,"text":"<message>"}
//! ```
//!
//! serde lives on this module's **own** record type; the `app` types stay
//! serde-free (the `session` module's convention). Every impurity — the path,
//! the clock for `ts`, the reads/appends/compaction — lives at the boundary in
//! `main.rs` (`InputHistoryStore`), which injects ids/timestamps into these
//! pure functions (the `set_clock` pattern).
//!
//! [`InputHistory::entries`]: crate::app::InputHistory

use serde::{Deserialize, Serialize};

/// One history line on disk — codex's `HistoryEntry` schema. `session_id`/`ts`
/// are recorded for the file's self-description and codex interop; the app never
/// reads them back (only `text` seeds `entries`). Unknown extra fields are
/// ignored on parse (serde's default), so a file a future build wrote still
/// loads — the forward-compatibility contract, like `session`'s reader.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct HistoryRecord {
    session_id: String,
    ts: u64,
    text: String,
}

/// Serialize one entry as a single JSONL line (no trailing newline — the caller
/// adds it). serde_json escapes embedded newlines/quotes, so a multi-line draft
/// (Shift+Enter) or a `"`-containing message is still **one physical line**,
/// keeping the file valid JSON-Lines. Infallible: the record is plain owned
/// data, so serialization cannot fail (mirrors `session::meta_line`).
#[must_use]
pub fn history_line(session_id: &str, ts: u64, text: &str) -> String {
    let record = HistoryRecord {
        session_id: session_id.to_string(),
        ts,
        text: text.to_string(),
    };
    // A struct of owned String/u64 always serializes; fall back to an empty
    // object on the impossible error rather than panicking the recorder.
    serde_json::to_string(&record).unwrap_or_else(|_| "{}".to_string())
}

/// Parse a whole file's contents into the recorded texts, **oldest-first** (the
/// order [`InputHistory::seed`] wants). Blank lines, lines that don't parse as a
/// [`HistoryRecord`], and records whose `text` is empty are skipped — a torn
/// last line, a future-version line, or a hand-edited empty entry never breaks
/// the load or seeds a blank recall entry (codex's forward-compatible reader;
/// `record` never writes an empty text, but a hand-edited/future file could).
///
/// [`InputHistory::seed`]: crate::app::InputHistory::seed
#[must_use]
pub fn parse_history(contents: &str) -> Vec<String> {
    contents
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str::<HistoryRecord>(line).ok())
        .map(|record| record.text)
        .filter(|text| !text.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_line_round_trips_through_parse() {
        let line = history_line("sess-1", 1_700_000_000, "cargo test");
        let texts = parse_history(&line);
        assert_eq!(texts, ["cargo test"]);
    }

    #[test]
    fn a_multiline_text_stays_one_physical_line_and_round_trips() {
        let text = "first line\nsecond line";
        let line = history_line("s", 1, text);
        assert!(
            !line.contains('\n'),
            "the serialized line must have no raw newline (JSONL stays one line per record)"
        );
        assert_eq!(parse_history(&line), [text]);
    }

    #[test]
    fn quotes_and_unicode_round_trip() {
        let text = r#"say "hi" — café 日本語"#;
        let line = history_line("s", 1, text);
        assert!(!line.contains('\n'));
        assert_eq!(parse_history(&line), [text]);
    }

    #[test]
    fn parse_preserves_order_oldest_first() {
        let mut file = String::new();
        for (i, t) in ["one", "two", "three"].iter().enumerate() {
            file.push_str(&history_line("s", i as u64, t));
            file.push('\n');
        }
        assert_eq!(parse_history(&file), ["one", "two", "three"]);
    }

    #[test]
    fn blank_and_malformed_lines_are_skipped() {
        let file = format!(
            "{}\n\n   \nnot json at all\n{{\"missing\":\"fields\"}}\n{}\n",
            history_line("s", 1, "kept-a"),
            history_line("s", 2, "kept-b"),
        );
        assert_eq!(parse_history(&file), ["kept-a", "kept-b"]);
    }

    #[test]
    fn an_empty_text_record_is_dropped() {
        // `record` never writes an empty text, but a hand-edited or future file
        // could carry one — it is not a recallable entry (a blank ↑ recall).
        let file = format!(
            "{}\n{}\n{}\n",
            history_line("s", 1, "kept"),
            r#"{"session_id":"s","ts":2,"text":""}"#,
            history_line("s", 3, "also"),
        );
        assert_eq!(parse_history(&file), ["kept", "also"]);
    }

    #[test]
    fn unknown_extra_fields_still_parse() {
        // A future build may add fields; serde ignores unknown keys by default.
        let line = r#"{"session_id":"s","ts":1,"text":"hello","future":42}"#;
        assert_eq!(parse_history(line), ["hello"]);
    }

    #[test]
    fn empty_input_is_empty() {
        assert!(parse_history("").is_empty());
        assert!(parse_history("\n\n  \n").is_empty());
    }

    #[test]
    fn a_torn_last_line_without_a_newline_does_not_break_earlier_entries() {
        // The newest append lost its trailing bytes (a torn write): the good
        // earlier line still loads, the torn tail is skipped.
        let file = format!(
            "{}\n{{\"session_id\":\"s\",\"ts\":2,\"te",
            history_line("s", 1, "good")
        );
        assert_eq!(parse_history(&file), ["good"]);
    }
}
