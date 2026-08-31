//! Tests for the character-level refinement of a diff cell's `-`/`+` line pairs
//! ([`crate::ui::inline_diff`], `docs/inline-diff.md`).

use super::super::inline_diff::{RefineRow, refine_pair, refine_rows};

/// The `changed` byte ranges of `text`, read back as the substrings they mark
/// — what the cell actually paints on the bright tint.
fn marked(text: &str, ranges: &[std::ops::Range<usize>]) -> Vec<String> {
    ranges.iter().map(|r| text[r.clone()].to_string()).collect()
}

#[test]
fn an_edited_line_marks_only_the_characters_that_differ() {
    // The reported case: one letter of a surname changed. Only that letter is
    // marked — `Bruce River` is unchanged on both sides, and marking it would
    // point at text the edit never touched.
    let old = "Bruce Rivera";
    let new = "Bruce Rivero";
    let d = refine_pair(old, new).expect("a shared prefix makes these a pair");
    assert_eq!(marked(old, &d.removed), ["a"]);
    assert_eq!(marked(new, &d.added), ["o"]);
}

#[test]
fn an_edit_in_the_middle_of_code_marks_the_changed_token() {
    let old = "let x = 1;";
    let new = "let x = 2;";
    let d = refine_pair(old, new).unwrap();
    assert_eq!(marked(old, &d.removed), ["1"]);
    assert_eq!(marked(new, &d.added), ["2"]);
}

#[test]
fn an_insertion_marks_only_the_inserted_run_and_nothing_on_the_removed_side() {
    // Adding an argument: the old line has nothing to mark, the new line
    // marks exactly what appeared.
    let old = "fn run(a) {";
    let new = "fn run(a, b) {";
    let d = refine_pair(old, new).unwrap();
    assert!(d.removed.is_empty(), "nothing was removed: {:?}", d.removed);
    assert_eq!(marked(new, &d.added), [", b"]);
}

#[test]
fn consecutive_changed_characters_merge_into_one_run() {
    // Marking is per character, but a *run* of changed characters paints as
    // one block — otherwise `z(1)` would be four separate flecks of colour.
    let old = "call(foo.bar)";
    let new = "call(foo.baz(1))";
    let d = refine_pair(old, new).unwrap();
    assert_eq!(marked(old, &d.removed), ["r"]);
    assert_eq!(marked(new, &d.added), ["z(1)"]);
}

#[test]
fn unchanged_text_between_two_edits_is_never_marked() {
    // The rule the marking exists to serve: only what changed is coloured.
    // Two values changed at opposite ends of the line; the `and beta = ` that
    // sits between them did not, so it keeps the plain row tint.
    let old = "alpha = 1 and beta = 2";
    let new = "alpha = 9 and beta = 8";
    let d = refine_pair(old, new).unwrap();
    assert_eq!(marked(old, &d.removed), ["1", "2"]);
    assert_eq!(marked(new, &d.added), ["9", "8"]);
}

#[test]
fn an_unrelated_line_pair_is_not_refined() {
    // A wholesale replacement has no "what changed" to point at; marking all
    // of it is noisier than marking none of it, so the row keeps its plain
    // tint (the pre-existing rendering).
    assert!(refine_pair("import os", "def main(argv, env):").is_none());
}

#[test]
fn a_short_line_whose_substance_mostly_changed_stays_flat() {
    // `old = 1` -> `new = 2` leaves only the `=` in common: four fifths of the
    // line's substance changed, so "the whole line changed" — what the flat
    // row tint already says — is the honest summary, and marking all but one
    // character adds nothing. The guard is a ratio, so it is the *proportion*
    // that decides, not the absolute size of the edit.
    assert!(refine_pair("    old = 1", "    new = 2").is_none());
}

#[test]
fn a_shared_indent_alone_does_not_make_two_lines_similar() {
    // Whitespace is excluded from the similarity count — otherwise every pair
    // of same-indented lines in a file would read as "related".
    assert!(refine_pair("        alpha()", "        zulu_bravo(charlie)").is_none());
}

#[test]
fn an_indent_only_change_is_refined_because_the_code_is_common() {
    // The one change that is otherwise invisible: the code is identical, so
    // the guard passes and the leading whitespace itself is marked.
    let old = "    value = 1";
    let new = "        value = 1";
    let d = refine_pair(old, new).unwrap();
    assert_eq!(marked(new, &d.added), ["    "]);
    assert!(d.removed.is_empty());
}

#[test]
fn identical_lines_are_not_refined() {
    assert!(refine_pair("same", "same").is_none());
}

#[test]
fn a_huge_line_with_a_small_edit_refines_fast() {
    // The realistic minified-bundle case: a 24 KB line, one token changed.
    // The prefix/suffix trim is what makes this affordable — the live cell
    // re-renders it every animation frame.
    let head = "abcdefghij,".repeat(1000);
    let tail = "klmnopqrst,".repeat(1000);
    let old = format!("{head}aaaaaa,{tail}");
    let new = format!("{head}bbbbbb,{tail}");
    let start = std::time::Instant::now();
    let d = refine_pair(&old, &new).unwrap();
    assert!(
        start.elapsed() < std::time::Duration::from_millis(32),
        "a huge line with a small edit must stay inside a frame"
    );
    assert_eq!(marked(&old, &d.removed), ["aaaaaa"]);
    assert_eq!(marked(&new, &d.added), ["bbbbbb"]);
}

#[test]
fn a_pathological_line_pair_answers_within_the_bound() {
    // Both lines differ almost everywhere: the LCS is skipped past
    // INLINE_DIFF_MAX_CELLS, and the guard then rejects the pair outright —
    // that much change is a replacement, not an edit. What matters here is
    // that it decides *fast*, without filling an O(n×m) table.
    let old = format!("head {} tail", "q0,".repeat(4000));
    let new = format!("head {} tail", "q1,".repeat(4000));
    let start = std::time::Instant::now();
    let verdict = refine_pair(&old, &new);
    assert!(
        start.elapsed() < std::time::Duration::from_millis(32),
        "the bound must keep a pathological pair off the frame budget"
    );
    assert!(verdict.is_none(), "mostly-different lines stay flat");
}

#[test]
fn utf8_lines_split_on_character_boundaries() {
    // The ranges index the line's bytes, so a multi-byte edit must never cut
    // a code point (the render slices with them).
    let old = "let s = \"héllo wörld\";";
    let new = "let s = \"héllo wörld!\";";
    let d = refine_pair(old, new).unwrap();
    for r in d.removed.iter().chain(&d.added) {
        assert!(old.is_char_boundary(r.start) || new.is_char_boundary(r.start));
    }
    assert!(marked(new, &d.added).concat().contains('!'));
}

#[test]
fn rows_pair_a_removed_run_with_the_added_run_that_follows_it() {
    // Two modified lines in one hunk: `-a -b +a' +b'` pairs by position.
    let rows = [
        RefineRow::Line(' ', "context()"),
        RefineRow::Line('-', "let x = 1;"),
        RefineRow::Line('-', "let y = 2;"),
        RefineRow::Line('+', "let x = 7;"),
        RefineRow::Line('+', "let y = 8;"),
    ];
    let out = refine_rows(&rows);
    assert_eq!(out.len(), rows.len());
    assert!(out[0].is_empty(), "a context row is never refined");
    assert_eq!(marked("let x = 1;", &out[1]), ["1"]);
    assert_eq!(marked("let y = 2;", &out[2]), ["2"]);
    assert_eq!(marked("let x = 7;", &out[3]), ["7"]);
    assert_eq!(marked("let y = 8;", &out[4]), ["8"]);
}

#[test]
fn a_character_appended_inside_a_line_marks_nothing_on_the_removed_side() {
    // `1` -> `10` did not *change* the `1`, it added a `0` after it. Marking
    // the `1` red would claim an edit that never happened.
    let d = refine_pair("let x = 1;", "let x = 10;").unwrap();
    assert!(d.removed.is_empty(), "nothing was removed: {:?}", d.removed);
    assert_eq!(marked("let x = 10;", &d.added), ["0"]);
}

#[test]
fn a_break_row_ends_a_run_so_hunks_never_pair_across_a_gap() {
    // A `⋮` gap means the two sides are lines apart in the file; pairing them
    // would invent a relationship the diff never claimed.
    let rows = [
        RefineRow::Line('-', "let x = 1;"),
        RefineRow::Break,
        RefineRow::Line('+', "let x = 2;"),
    ];
    let out = refine_rows(&rows);
    assert!(out.iter().all(Vec::is_empty), "no pair spans the gap");
}

#[test]
fn an_added_run_with_no_removed_run_before_it_is_not_refined() {
    // A pure insertion: every line is new, so there is nothing to compare
    // against and the whole row is the change.
    let rows = [
        RefineRow::Line(' ', "context()"),
        RefineRow::Line('+', "brand_new();"),
    ];
    assert!(refine_rows(&rows).iter().all(Vec::is_empty));
}

#[test]
fn an_unequal_run_pairs_the_overlap_and_leaves_the_rest_plain() {
    // `-1 +2` is usually "the line was edited, and one was inserted": the
    // pair still refines, the unpaired addition stays plain.
    let rows = [
        RefineRow::Line('-', "total = 1"),
        RefineRow::Line('+', "total = 2"),
        RefineRow::Line('+', "extra_line_entirely_new()"),
    ];
    let out = refine_rows(&rows);
    assert_eq!(marked("total = 1", &out[0]), ["1"]);
    assert_eq!(marked("total = 2", &out[1]), ["2"]);
    assert!(out[2].is_empty(), "the unpaired addition stays plain");
}
