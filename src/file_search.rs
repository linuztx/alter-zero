//! The pure core of the `@` file-path picker: extracting the `@token` under the
//! cursor, and fuzzy-matching/ranking workspace file paths against the query.
//!
//! A focused port of openai/codex's `file_search.rs` + the `@`-token detection
//! in `bottom_pane/chat_composer.rs` — deliberately *without* codex's nucleo
//! engine, async session machinery, `.gitignore` walk, or tool/skill mentions.
//! The actual filesystem walk + the async request/response plumbing live at the
//! `main.rs` boundary (it calls [`rank_files`] off-thread); everything here is
//! pure, so it is unit-tested directly. See `docs/file-search.md`.

use std::ops::Range;

/// The `@`-mention token under the cursor: the byte `range` of the whole token
/// (the `@` through the end of the run, what [`crate::textarea::TextArea::replace_range`]
/// swaps for the chosen path) and the `query` (the text after the `@`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AtToken {
    /// Byte range of the whole `@token` in the input text.
    pub range: Range<usize>,
    /// The token text after the `@` (the fuzzy-search query).
    pub query: String,
}

/// One ranked file match: the relative `path`, its `score` (higher is better),
/// and the byte offsets of the query's matched characters in `path` (for popup
/// highlighting). Mirrors the fields codex's `FileMatch` carries that we use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileMatch {
    /// The relative file path (directories carry a trailing `/`).
    pub path: String,
    /// Match score; higher ranks earlier.
    pub score: i32,
    /// Byte offsets in `path` of the characters matched by the query.
    pub indices: Vec<usize>,
}

/// The `@`-mention token at `cursor` in `text`, if one is active: scan back over
/// non-whitespace to the token start (just after the previous whitespace, or the
/// start of the text) — it is an `@`-token only when that first character is `@`
/// (so the start-of-line/after-whitespace boundary holds by construction, and
/// `email@host` never triggers). The token extends forward to the next
/// whitespace, so editing inside it still targets the whole run. `None` when the
/// cursor is not in such a token. A port of codex's `current_prefixed_token_range`.
#[must_use]
pub fn at_token(text: &str, cursor: usize) -> Option<AtToken> {
    let cursor = cursor.min(text.len());
    // Token start: just after the last whitespace before the cursor (or the very
    // start). By construction the char before `start` is whitespace (or none), so
    // the `@`-at-a-boundary rule holds automatically.
    let start = text[..cursor]
        .char_indices()
        .rev()
        .find(|&(_, c)| c.is_whitespace())
        .map_or(0, |(i, c)| i + c.len_utf8());
    if !text[start..].starts_with('@') {
        return None;
    }
    // Token end: the first whitespace at or after the cursor (or the end).
    let end = text[cursor..]
        .char_indices()
        .find(|&(_, c)| c.is_whitespace())
        .map_or(text.len(), |(i, _)| cursor + i);
    let query = text[start + '@'.len_utf8()..end].to_string();
    Some(AtToken {
        range: start..end,
        query,
    })
}

/// Case-insensitive **subsequence** fuzzy match of `query` against `candidate`:
/// `Some((score, indices))` when every character of `query` appears in order
/// (boundary, contiguous, and basename hits score higher), else `None`. An empty
/// query matches everything with score 0 (the lone-`@` "list files" case).
/// `indices` are byte offsets of the matched characters in `candidate`.
#[must_use]
pub fn fuzzy_match(query: &str, candidate: &str) -> Option<(i32, Vec<usize>)> {
    if query.is_empty() {
        return Some((0, Vec::new()));
    }
    let qchars: Vec<char> = query.chars().collect();
    let basename_start = candidate.rfind('/').map_or(0, |i| i + 1);
    let mut qi = 0;
    let mut indices = Vec::with_capacity(qchars.len());
    let mut score = 0i32;
    let mut prev_match_char: Option<usize> = None;
    for (char_pos, (byte_off, ch)) in candidate.char_indices().enumerate() {
        if qi < qchars.len() && ch.eq_ignore_ascii_case(&qchars[qi]) {
            // Boundary bonus: at the start, or right after a path separator.
            let at_boundary = byte_off == 0
                || matches!(
                    candidate[..byte_off].chars().next_back(),
                    Some('/' | '_' | '-' | '.' | ' ')
                );
            score += if at_boundary { 16 } else { 1 };
            if byte_off >= basename_start {
                score += 4;
            }
            if prev_match_char == Some(char_pos.wrapping_sub(1)) {
                score += 12; // contiguous run
            }
            indices.push(byte_off);
            prev_match_char = Some(char_pos);
            qi += 1;
        }
    }
    if qi != qchars.len() {
        return None;
    }
    // Mildly prefer shorter candidates so a tight match isn't buried in a long path.
    score -= candidate.chars().count() as i32 / 8;
    Some((score, indices))
}

/// Rank `candidates` against `query`: keep the fuzzy matches, sort by score
/// (desc), then shorter path, then lexicographically, and cap at `limit`. The
/// pure ranking the boundary's file-search worker runs over the walked file list.
#[must_use]
pub fn rank_files(query: &str, candidates: &[String], limit: usize) -> Vec<FileMatch> {
    let mut scored: Vec<FileMatch> = candidates
        .iter()
        .filter_map(|c| {
            fuzzy_match(query, c).map(|(score, indices)| FileMatch {
                path: c.clone(),
                score,
                indices,
            })
        })
        .collect();
    scored.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| a.path.chars().count().cmp(&b.path.chars().count()))
            .then_with(|| a.path.cmp(&b.path))
    });
    scored.truncate(limit);
    scored
}

#[cfg(test)]
mod tests {
    use super::*;

    // ===== at_token =====

    #[test]
    fn at_token_finds_the_token_after_whitespace() {
        let t = at_token("see @alp", 8).expect("token");
        assert_eq!(t.query, "alp");
        assert_eq!(t.range, 4..8);
    }

    #[test]
    fn at_token_is_none_inside_an_email() {
        assert!(at_token("mail@host.com", 13).is_none());
    }

    #[test]
    fn at_token_empty_query_for_a_lone_at() {
        let t = at_token("@", 1).expect("token");
        assert_eq!(t.query, "");
        assert_eq!(t.range, 0..1);
    }

    #[test]
    fn at_token_captures_the_whole_token_when_cursor_is_inside() {
        // Cursor right after "fo" in "@foo bar".
        let t = at_token("@foo bar", 3).expect("token");
        assert_eq!(t.query, "foo");
        assert_eq!(t.range, 0..4);
    }

    #[test]
    fn at_token_none_without_an_at() {
        assert!(at_token("hello world", 11).is_none());
    }

    // ===== fuzzy_match =====

    #[test]
    fn fuzzy_match_is_a_subsequence_with_indices() {
        let (_, idx) = fuzzy_match("ab", "xaxb").expect("match");
        assert_eq!(idx, vec![1, 3]);
    }

    #[test]
    fn fuzzy_match_is_case_insensitive() {
        assert!(fuzzy_match("AB", "ab").is_some());
    }

    #[test]
    fn fuzzy_match_none_when_not_a_subsequence() {
        assert!(fuzzy_match("ba", "ab").is_none());
        assert!(fuzzy_match("z", "abc").is_none());
    }

    #[test]
    fn fuzzy_match_empty_query_matches_everything() {
        assert_eq!(fuzzy_match("", "anything"), Some((0, vec![])));
    }

    // ===== rank_files =====

    #[test]
    fn rank_files_prefers_basename_contiguous_matches() {
        let files = vec![
            "src/my_admin_index.rs".to_string(),
            "src/main.rs".to_string(),
        ];
        let r = rank_files("main", &files, 10);
        assert_eq!(r[0].path, "src/main.rs");
    }

    #[test]
    fn rank_files_drops_non_matches_and_caps_at_limit() {
        let files = vec![
            "a.rs".to_string(),
            "b.rs".to_string(),
            "zzz.txt".to_string(),
        ];
        let r = rank_files("rs", &files, 1);
        assert_eq!(r.len(), 1);
        assert!(r.iter().all(|m| m.path.ends_with(".rs")));
    }

    #[test]
    fn rank_files_empty_query_lists_all_shortest_first() {
        let files = vec!["longer_name.rs".to_string(), "a.rs".to_string()];
        let r = rank_files("", &files, 10);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].path, "a.rs", "shorter path first on an empty query");
    }
}
