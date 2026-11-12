//! The pure core of the `@` file-path picker: extracting the `@token` under the
//! cursor, and fuzzy-matching/ranking workspace file paths against the query.
//!
//! A focused port of openai/codex's `file_search.rs` + the `@`-token detection
//! in `bottom_pane/chat_composer.rs` — deliberately *without* codex's nucleo
//! engine, async session machinery, `.gitignore` walk, or tool/skill mentions.
//! The actual filesystem walk + the async request/response plumbing live at the
//! `main.rs` boundary (it calls [`rank_files`] off-thread); everything here is
//! pure, so it is unit-tested directly. See `docs/input/file-search.md`.

use std::cmp::Ordering;
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

/// One ranked file match: the relative `path` and its `score` (higher is
/// better). Which characters the query matched — what the picker bolds — is
/// not kept: the picker works that out with [`fuzzy_match`] for the few rows
/// it draws, since a list holds every match the walk found and almost none
/// of them is ever on screen. See `docs/input/file-search.md`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileMatch {
    /// The relative file path (directories carry a trailing `/`).
    pub path: String,
    /// Match score; higher ranks earlier.
    pub score: i32,
}

impl FileMatch {
    /// Is this match a directory? The boundary's walk lists directories with a
    /// trailing `/` (see `docs/input/file-search.md`), so the kind is carried by the
    /// path itself.
    #[must_use]
    pub fn is_dir(&self) -> bool {
        self.path.ends_with('/')
    }

    /// Byte offset in [`path`](Self::path) where the display [`name`](Self::name)
    /// begins — what remaps the matched characters' byte offsets in the path
    /// onto the picker's name column.
    #[must_use]
    pub fn name_start(&self) -> usize {
        let trimmed = self.path.strip_suffix('/').unwrap_or(&self.path);
        trimmed.rfind('/').map_or(0, |i| i + 1)
    }

    /// The final path component (a directory's without its trailing `/`) — the
    /// picker's name column.
    #[must_use]
    pub fn name(&self) -> &str {
        let trimmed = self.path.strip_suffix('/').unwrap_or(&self.path);
        &trimmed[self.name_start()..]
    }

    /// The leading directories through the last interior `/` — empty for a
    /// root-level entry (the picker renders that as `./`).
    #[must_use]
    pub fn parent(&self) -> &str {
        &self.path[..self.name_start()]
    }
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

/// ASCII-case-insensitive **subsequence** fuzzy match of `query` against
/// `candidate`: `Some((score, indices))` when every character of `query` appears
/// in order (boundary, contiguous, and basename hits score higher), else `None`.
/// The case fold is ASCII-only (`eq_ignore_ascii_case` — `a` ≡ `A`, but `é` ≢
/// `É`; non-ASCII characters must match exactly). An empty query matches
/// everything with score 0 (the lone-`@` "list files" case). `indices` are byte
/// offsets of the matched characters in `candidate`.
#[must_use]
pub fn fuzzy_match(query: &str, candidate: &str) -> Option<(i32, Vec<usize>)> {
    let qchars: Vec<char> = query.chars().collect();
    let mut indices = Vec::with_capacity(qchars.len());
    let score = fuzzy_score(&qchars, candidate, |byte_off| indices.push(byte_off))?;
    Some((score, indices))
}

/// The scoring pass behind [`fuzzy_match`], over the query's characters
/// (`qchars`), handing `matched` the byte offset of each character of
/// `candidate` the query matches. The ranking hands it nothing to do and
/// gathers the query's characters once, so ranking a whole walk allocates
/// nothing per path; the picker asks [`fuzzy_match`] for the offsets of
/// the few rows it draws.
fn fuzzy_score(qchars: &[char], candidate: &str, mut matched: impl FnMut(usize)) -> Option<i32> {
    if qchars.is_empty() {
        return Some(0);
    }
    // A walked directory carries a trailing `/`; its basename is the component
    // *before* that slash, or every dir would lose the basename bonus.
    let trimmed = candidate.strip_suffix('/').unwrap_or(candidate);
    let basename_start = trimmed.rfind('/').map_or(0, |i| i + 1);
    let mut qi = 0;
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
            matched(byte_off);
            prev_match_char = Some(char_pos);
            qi += 1;
            if qi == qchars.len() {
                break; // nothing past the last match moves the score
            }
        }
    }
    if qi != qchars.len() {
        return None;
    }
    // Mildly prefer shorter candidates so a tight match isn't buried in a long path.
    score -= candidate.chars().count() as i32 / 8;
    Some(score)
}

/// Rank `candidates` against `query`: keep **every** fuzzy match, sorted by
/// score (desc), then shorter path, then lexicographically — or, for an empty
/// query, which has nothing to rank by, in `listing_order`, the way a
/// directory listing reads. The pure ranking the boundary's file-search
/// worker runs over the walked file list; the picker scrolls through and
/// counts all of it, so nothing is capped here — the walk's own bound is what
/// keeps the list finite. The list is taken by value so each matched path
/// *moves* into its [`FileMatch`]: ranking a whole walk never copies it, and
/// scoring a path allocates nothing (`fuzzy_score`, the query's characters
/// gathered once for the whole walk). See `docs/input/file-search.md`.
#[must_use]
pub fn rank_files(query: &str, candidates: Vec<String>) -> Vec<FileMatch> {
    let qchars: Vec<char> = query.chars().collect();
    if qchars.is_empty() {
        let mut listed: Vec<FileMatch> = candidates
            .into_iter()
            .map(|path| FileMatch { path, score: 0 })
            .collect();
        listed.sort_by(listing_order);
        return listed;
    }
    // Each match beside its length in characters, counted once here rather
    // than twice in every comparison the sort makes between two paths that
    // score the same.
    let mut scored: Vec<(usize, FileMatch)> = candidates
        .into_iter()
        .filter_map(|path| {
            let score = fuzzy_score(&qchars, &path, |_| {})?;
            Some((path.chars().count(), FileMatch { path, score }))
        })
        .collect();
    scored.sort_by(|(a_len, a), (b_len, b)| {
        b.score
            .cmp(&a.score)
            .then_with(|| a_len.cmp(b_len))
            .then_with(|| a.path.cmp(&b.path))
    });
    scored.into_iter().map(|(_, m)| m).collect()
}

/// The order a bare `@` lists the workspace in — a directory listing read a
/// level at a time: the cwd's own entries first, then each deeper level, an
/// entry's siblings grouped under their parent in the order the parents were
/// themselves listed (their paths compared component by component), and
/// within a parent the directories before the files, each name compared
/// case-insensitively. A tie in case alone falls to the spelling, so the
/// order is total: the filesystem hands back a directory's entries in no
/// promised order, and the list must not reshuffle between two keystrokes
/// over the same files. See `docs/input/file-search.md`.
///
/// It runs for every comparison of a sort over up to `FILE_INDEX_CAP`
/// entries, so the common cases are cheap: siblings share a parent spelled
/// byte for byte the same, which skips the component walk, and a name in
/// plain ASCII folds a byte at a time rather than through `char`'s Unicode
/// tables.
fn listing_order(a: &FileMatch, b: &FileMatch) -> Ordering {
    let (a_parent, b_parent) = (a.parent(), b.parent());
    a_parent
        .matches('/')
        .count()
        .cmp(&b_parent.matches('/').count())
        .then_with(|| {
            if a_parent == b_parent {
                return Ordering::Equal;
            }
            // Equal depths split into equally many components.
            a_parent
                .split('/')
                .zip(b_parent.split('/'))
                .map(|(x, y)| name_order(x, y))
                .find(|order| order.is_ne())
                .unwrap_or(Ordering::Equal)
        })
        .then_with(|| sibling_order(a.name(), a.is_dir(), b.name(), b.is_dir()))
}

/// Two entries of one folder in the order a bare `@` lists them: folders
/// before files, then the names compared case-insensitively, a tie in case
/// alone falling to the spelling — `apple/`, `Zed/`, `a.md`, `B.txt`,
/// `b.txt`. A name is the entry's own, without a folder's closing `/`. The
/// last word of the listing order, and the order the boundary's walk sorts
/// each folder into as it reads it, which is what hands the listing's sort a
/// list already in order (`docs/input/file-search.md`).
#[must_use]
pub fn sibling_order(a_name: &str, a_is_dir: bool, b_name: &str, b_is_dir: bool) -> Ordering {
    b_is_dir
        .cmp(&a_is_dir)
        .then_with(|| name_order(a_name, b_name))
}

/// Two names compared case-insensitively, the exact spelling breaking a tie
/// — `a.txt`, `B.txt`, `b.txt` — so no two different names compare equal.
/// Two ASCII names fold byte by byte, which is what `char::to_lowercase`
/// does to ASCII anyway.
fn name_order(a: &str, b: &str) -> Ordering {
    let folded = if a.is_ascii() && b.is_ascii() {
        a.bytes()
            .map(|c| c.to_ascii_lowercase())
            .cmp(b.bytes().map(|c| c.to_ascii_lowercase()))
    } else {
        a.chars()
            .flat_map(char::to_lowercase)
            .cmp(b.chars().flat_map(char::to_lowercase))
    };
    folded.then_with(|| a.cmp(b))
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

    // ===== FileMatch name/parent/kind split (the columned picker rows) =====

    #[test]
    fn file_match_splits_name_parent_and_kind() {
        let m = |path: &str| FileMatch {
            path: path.to_string(),
            score: 0,
        };
        let file = m("public/assets/cv.pdf");
        assert!(!file.is_dir());
        assert_eq!(file.name(), "cv.pdf");
        assert_eq!(file.name_start(), 14);
        assert_eq!(file.parent(), "public/assets/");

        let dir = m("public/assets/");
        assert!(dir.is_dir());
        assert_eq!(dir.name(), "assets");
        assert_eq!(dir.name_start(), 7);
        assert_eq!(dir.parent(), "public/");

        let root_dir = m("public/");
        assert!(root_dir.is_dir());
        assert_eq!(root_dir.name(), "public");
        assert_eq!(root_dir.name_start(), 0);
        assert_eq!(root_dir.parent(), "", "root-level: no parent prefix");

        let root_file = m("README.md");
        assert!(!root_file.is_dir());
        assert_eq!(root_file.name(), "README.md");
        assert_eq!(root_file.name_start(), 0);
        assert_eq!(root_file.parent(), "");
    }

    // ===== rank_files =====

    #[test]
    fn rank_files_gives_a_dir_its_basename_bonus() {
        // A directory literally named `beta` must outrank a file that merely
        // contains the letters: the trailing `/` a walked dir carries must not
        // defeat the basename bonus (`rfind('/')` finding the trailing slash
        // used to leave every dir with no basename at all).
        let files = vec!["docs/beta/".to_string(), "xbeta.txt".to_string()];
        let r = rank_files("beta", files);
        assert_eq!(r[0].path, "docs/beta/", "{r:?}");
    }

    #[test]
    fn rank_files_prefers_basename_contiguous_matches() {
        let files = vec![
            "src/my_admin_index.rs".to_string(),
            "src/main.rs".to_string(),
        ];
        let r = rank_files("main", files);
        assert_eq!(r[0].path, "src/main.rs");
    }

    #[test]
    fn rank_files_drops_non_matches_and_keeps_every_match() {
        let files = vec![
            "a.rs".to_string(),
            "b.rs".to_string(),
            "zzz.txt".to_string(),
        ];
        let r = rank_files("rs", files);
        let paths: Vec<&str> = r.iter().map(|m| m.path.as_str()).collect();
        assert_eq!(paths, ["a.rs", "b.rs"]);
    }

    #[test]
    fn rank_files_keeps_every_match() {
        // The picker scrolls through every match and counts them all
        // (`docs/input/file-search.md`), so the ranking caps nothing: thirty
        // matches in, thirty out, in rank order. They all score the same
        // here, so the shorter `src/mod.rs` leads and the rest follow by
        // name; `README.md` has no `o` after its `M` and is dropped.
        let mut files: Vec<String> = (0..29).map(|i| format!("src/f{i:02}_mod.rs")).collect();
        files.push("src/mod.rs".to_string());
        files.push("README.md".to_string());
        let r = rank_files("mod", files);
        assert_eq!(r.len(), 30, "every match, no cap: {r:?}");
        assert_eq!(r[0].path, "src/mod.rs", "{r:?}");
        assert_eq!(r[1].path, "src/f00_mod.rs", "{r:?}");
        assert_eq!(r[29].path, "src/f28_mod.rs", "{r:?}");
        assert!(r.iter().all(|m| m.path != "README.md"), "{r:?}");
    }

    /// The paths an empty query lists `walked` in.
    fn listed(walked: &[&str]) -> Vec<String> {
        let walked = walked.iter().map(|p| (*p).to_string()).collect();
        rank_files("", walked).into_iter().map(|m| m.path).collect()
    }

    #[test]
    fn a_bare_at_lists_the_workspace_like_a_directory_listing() {
        // An empty query has nothing to rank by, so it lists the walk the way
        // a directory listing reads: the cwd's own entries first —
        // directories, then files, case-insensitively — then each deeper
        // level, siblings grouped under their parent in the order the
        // parents were themselves listed. Shortest-path-first, the order this
        // replaced, put `a/b.rs` ahead of `README.md`.
        let walked = [
            "src/ui/menu.rs",
            "README.md",
            "src/",
            "a/",
            "src/main.rs",
            "Cargo.toml",
            "src/ui/",
            "a/b.rs",
            "docs/",
            "docs/x.md",
            "src/app/",
            "src/app/mod.rs",
            "zeta.txt",
            "Alpha.txt",
        ];
        assert_eq!(
            listed(&walked),
            [
                // The cwd's own entries: directories, then files.
                "a/",
                "docs/",
                "src/",
                "Alpha.txt",
                "Cargo.toml",
                "README.md",
                "zeta.txt",
                // One level down, under a/, docs/ and src/ in that order.
                "a/b.rs",
                "docs/x.md",
                "src/app/",
                "src/ui/",
                "src/main.rs",
                // Two levels down: src/app/ before src/ui/, as listed above.
                "src/app/mod.rs",
                "src/ui/menu.rs",
            ]
        );
    }

    #[test]
    fn the_bare_listing_is_the_same_whatever_order_the_walk_found_it_in() {
        // A directory's entries come back from the filesystem in no promised
        // order, so the listing must be a total order of its own — a case-only
        // tie broken by the spelling — or the list could reshuffle between
        // two keystrokes over the same files.
        let walked = ["b.txt", "B.txt", "a.txt", "dir/", "Dir/", "dir/x", "Dir/x"];
        let forward = listed(&walked);
        let mut reversed = walked;
        reversed.reverse();
        assert_eq!(listed(&reversed), forward);
        assert_eq!(
            forward,
            ["Dir/", "dir/", "a.txt", "B.txt", "b.txt", "Dir/x", "dir/x"]
        );
    }

    #[test]
    fn the_bare_listing_folds_case_beyond_ascii_too() {
        // A name outside ASCII folds its case like any other — `Éclair` and
        // `éclair` tie and fall to the spelling — and sorts by code point
        // after the folding, so `é` comes after `z`, however a pair mixes
        // ASCII names with the rest.
        let walked = ["éclair.txt", "Zed.txt", "Éclair.txt", "eclair.txt"];
        assert_eq!(
            listed(&walked),
            ["eclair.txt", "Zed.txt", "Éclair.txt", "éclair.txt"]
        );
    }

    #[test]
    fn a_folders_entries_list_folders_first_then_names_without_regard_to_case() {
        // The order within one folder — what the walk sorts each folder into
        // as it reads it, and the last word of the bare listing's own order,
        // so the walk hands that sort a list already in order.
        let mut entries = [
            ("b.txt", false),
            ("Zed", true),
            ("B.txt", false),
            ("a.md", false),
            ("apple", true),
        ];
        entries.sort_by(|a, b| sibling_order(a.0, a.1, b.0, b.1));
        assert_eq!(
            entries,
            [
                ("apple", true),
                ("Zed", true),
                ("a.md", false),
                ("B.txt", false),
                ("b.txt", false),
            ]
        );
    }
}
