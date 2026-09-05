//! GFM table rendering (`docs/markdown.md`, `docs/table-streaming.md`).

use super::*;
use crate::ui::table::{
    allocate_column_widths, join_wrapped_table_row, table_cell_segments, table_column_widths,
    table_content_rows, table_record_block, table_record_separator, table_row_lines,
    table_should_use_records,
};
use crate::ui::theme::{
    TABLE_MIN_COL, TABLE_RECORD_SEPARATOR_WIDTH, inline_code_color, tool_dim_color,
    tool_fail_color, tool_ok_color,
};
use crate::ui::tool::tool_full_lines;
use crate::ui::wrap::cols;

#[test]
fn table_content_rows_draws_a_bordered_grid() {
    let lines: Vec<String> = ["| Name | Type |", "|------|------|", "| Alpha | X |"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert_eq!(
        rows_text(&table_content_rows(&lines, 80)),
        vec![
            "┌───────┬──────┐",
            "│ Name  │ Type │",
            "├───────┼──────┤",
            "│ Alpha │ X    │",
            "└───────┴──────┘",
        ]
    );
}

#[test]
fn table_header_cells_center_by_default() {
    // Claude Code's look: a header cell is CENTERED in its column, while the
    // data below it stays left-aligned. The delimiter here declares no
    // alignment (`|---|`), which is the common case a model emits.
    let lines: Vec<String> = [
        "| Check | Result |",
        "|-------|--------|",
        "| cargo fmt | ok |",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    assert_eq!(
        rows_text(&table_content_rows(&lines, 80)),
        vec![
            "┌───────────┬────────┐",
            "│   Check   │ Result │",
            "├───────────┼────────┤",
            "│ cargo fmt │ ok     │",
            "└───────────┴────────┘",
        ]
    );
}

#[test]
fn table_header_keeps_an_explicitly_declared_alignment() {
    // Centering is only the *default* (`Alignment::None`). When the author
    // declared an alignment with colons, the header honours it — a markdown
    // renderer must not throw away stated intent.
    let lines: Vec<String> = ["| head | x |", "| :--- | ---: |", "| a longer cell | y |"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let rows = rows_text(&table_content_rows(&lines, 80));
    assert_eq!(
        rows[1], "│ head          │ x │",
        "an explicit `:---` keeps the header left: {rows:#?}"
    );
}

#[test]
fn table_row_centers_a_short_cell_against_a_wrapped_one() {
    // Claude Code centres a row's cells VERTICALLY: beside a cell that wrapped
    // to three rows, a one-line cell sits on the middle row, not the top.
    let cells = vec![
        table_cell_segments("one two three", Style::default()),
        table_cell_segments("x", Style::default()),
    ];
    let rows = table_row_lines(
        &cells,
        &[5, 3],
        &[markdown::Alignment::None, markdown::Alignment::None],
    );
    assert_eq!(
        rows_text(&rows),
        vec!["│ one   │     │", "│ two   │ x   │", "│ three │     │"]
    );
}

#[test]
fn assistant_table_centers_headers_and_short_cells_like_claude_code() {
    // The reported screenshot, end to end: `Check`/`Result` centred in their
    // columns, and a one-line cell vertically centred beside its wrapped
    // neighbour — so `✅ pass (exit 0)` shares a row with `--all-targets`
    // (the middle of the three) rather than sitting at the top, and
    // `cargo test` shares a row with the middle line of its Result cell.
    let text = "| Check | Result |\n\
                |-------|--------|\n\
                | `cargo fmt --check` | ✅ clean |\n\
                | `cargo clippy --all-targets -- -D warnings` | ✅ pass (exit 0) |\n\
                | `cargo test` | ✅ all tests pass, 0 failures (23 live-API tests \
                skipped — need `OPENROUTER_API_KEY` / `A0_VENICE_API_KEY`) |";
    let rows: Vec<String> = message_lines(Role::Assistant, text, 80)
        .iter()
        .map(plain)
        .collect();
    let header = rows
        .iter()
        .find(|r| r.contains("Check") && r.contains("Result"))
        .expect("a header row");
    assert!(
        header.starts_with("● │   ") || header.starts_with("  │   "),
        "the header cell is centred, not flush left: {header:?}"
    );
    assert!(
        rows.iter()
            .any(|r| r.contains("--all-targets") && r.contains("✅ pass (exit 0)")),
        "the one-line Result cell sits on the middle row of its neighbour: {rows:#?}"
    );
    assert!(
        rows.iter()
            .any(|r| r.contains("cargo test") && r.contains("skipped — need")),
        "`cargo test` sits on the middle row of its Result cell: {rows:#?}"
    );
}

#[test]
fn table_content_rows_aligns_per_delimiter() {
    // Left, right, and center alignment from the delimiter colons.
    let lines: Vec<String> = ["| a | b | c |", "| :-- | --: | :-: |", "| x | yy | zzz |"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert_eq!(
        rows_text(&table_content_rows(&lines, 80)),
        vec![
            "┌───┬────┬─────┐",
            "│ a │  b │  c  │",
            "├───┼────┼─────┤",
            "│ x │ yy │ zzz │",
            "└───┴────┴─────┘",
        ]
    );
}

#[test]
fn table_content_rows_pads_short_rows_to_the_column_count() {
    // A data row with fewer cells than the header is padded with blanks;
    // extra cells beyond the delimiter's column count are dropped.
    let lines: Vec<String> = ["| a | b |", "|---|---|", "| x |"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert_eq!(
        rows_text(&table_content_rows(&lines, 80)),
        vec![
            "┌───┬───┐",
            "│ a │ b │",
            "├───┼───┤",
            "│ x │   │",
            "└───┴───┘",
        ]
    );
}

#[test]
fn table_content_rows_renders_inline_markdown_in_cells() {
    // Regression (the reported bug): a table cell's `` `code` `` and
    // `**bold**` markers rendered literally instead of being inline-parsed
    // like prose (docs/markdown.md). They must be styled — backticks and
    // asterisks gone — and the column sized to the *rendered* width
    // (`a.db`, not `` `a.db` ``).
    let lines: Vec<String> = ["| Name  | When |", "|-------|------|", "| `a.db` | **X** |"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let rows = table_content_rows(&lines, 80);
    assert_eq!(
        rows_text(&rows),
        vec![
            "┌──────┬──────┐",
            "│ Name │ When │",
            "├──────┼──────┤",
            "│ a.db │ X    │",
            "└──────┴──────┘",
        ]
    );
    // The code cell is cyan, the bold cell carries BOLD, and no raw markers
    // survive anywhere in the grid.
    let spans: Vec<(String, Modifier, Option<Color>)> = rows
        .iter()
        .flat_map(|r| r.iter())
        .map(|s| (s.content.to_string(), s.style.add_modifier, s.style.fg))
        .collect();
    assert!(
        spans
            .iter()
            .any(|(t, _, fg)| t == "a.db" && *fg == Some(inline_code_color())),
        "inline code cell is cyan: {spans:?}"
    );
    assert!(
        spans
            .iter()
            .any(|(t, m, _)| t == "X" && m.contains(Modifier::BOLD)),
        "bold cell carries BOLD: {spans:?}"
    );
    let joined: String = spans.iter().map(|(t, _, _)| t.as_str()).collect();
    assert!(
        !joined.contains("**") && !joined.contains('`'),
        "no raw markers leak into the grid: {joined:?}"
    );
}

#[test]
fn table_grid_draws_separators_between_every_row() {
    // Claude Code's grid look (the user's reference): every data row is
    // framed — a `├──┼──┤` rule between consecutive rows, not just under
    // the header (docs/table-streaming.md).
    let lines: Vec<String> = ["| a | b |", "|---|---|", "| 1 | 2 |", "| 3 | 4 |"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert_eq!(
        rows_text(&table_content_rows(&lines, 80)),
        vec![
            "┌───┬───┐",
            "│ a │ b │",
            "├───┼───┤",
            "│ 1 │ 2 │",
            "├───┼───┤",
            "│ 3 │ 4 │",
            "└───┴───┘",
        ]
    );
}

#[test]
fn allocate_column_widths_spends_the_surplus_where_the_content_is() {
    // The reported screenshot's shape: column 1's natural width is driven by
    // ONE long cell (`cargo clippy --all-targets -- -D warnings`, 40) while
    // every other cell in it is ≤ 17; column 2 holds a ~104-column value.
    // Levelling the two widths gave column 1 ~35 columns it never used and
    // starved column 2 into four wrapped rows. Word-safe floors [13, 18] fit
    // in the 71-column budget, so the 40-column surplus is split by unmet
    // demand (27:86) — the value column ends with roughly twice the room,
    // Claude Code's layout.
    let w = allocate_column_widths(&[40, 104], &[13, 18], 78);
    assert_eq!(w, vec![23, 48]);
    assert_eq!(
        w.iter().sum::<usize>(),
        71,
        "columns fill the content width"
    );
}

#[test]
fn allocate_column_widths_wraps_words_before_shattering_a_token() {
    // The ping-summary fit: Host(12) IP(33) Packets(7) Loss(4) RTT(21) must
    // fit avail 85 (content 69, 8 over). The IPv6 column is a single
    // unbreakable 33-column token; the RTT column wraps at spaces. Taking
    // the overflow from the *widest* column hard-broke the address
    // mid-token, so the overflow comes from the column that can wrap
    // cleanly instead — and the short cells still keep their natural width.
    assert_eq!(
        allocate_column_widths(&[12, 33, 7, 4, 21], &[12, 33, 7, 4, 11], 85),
        vec![12, 33, 7, 4, 13]
    );
}

#[test]
fn allocate_column_widths_levels_the_widest_when_no_floor_fits() {
    // When even the word-safe floors overflow the budget, some token has to
    // hard-break: fall back to levelling the widest column down one display
    // column at a time (ties leftmost-first), which keeps a short column
    // whole for as long as possible.
    let w = allocate_column_widths(&[8, 20], &[8, 20], 20);
    assert_eq!(
        w.iter().sum::<usize>(),
        13,
        "columns fill the content width"
    );
    assert!(
        w[1] > w[0],
        "the wider natural column keeps more room: {w:?}"
    );
    assert!(w.iter().all(|&c| c >= TABLE_MIN_COL), "floored: {w:?}");
}

#[test]
fn narrow_table_keeps_short_cells_whole() {
    // End-to-end (the reported screenshot): at a width where the natural
    // grid overflows, "facebook.com" and the "Packets"/"Loss" headers stay
    // on one line — the wrapping lands on the wide IP column instead.
    let text = "| Host | IP | Packets | Loss | RTT min/avg/max |\n\
                |------|----|---------|------|-----------------|\n\
                | google.com | 2404:6800:4017:809::200e | 10/10 | 0% | 86.8 / 89.8 / 96.4 ms |\n\
                | facebook.com | 2a03:2880:f372:1:face:b00c:0:25de | 10/10 | 0% | 15.0 / 16.4 / 17.9 ms |\n\
                | x.com | 162.159.140.229 | 10/10 | 0% | 14.4 / 15.5 / 16.2 ms |";
    let rows: Vec<String> = message_lines(Role::Assistant, text, 87)
        .iter()
        .map(plain)
        .collect();
    assert!(
        rows.iter().any(|r| r.contains(" facebook.com ")),
        "a short host cell never breaks mid-word: {rows:#?}"
    );
    assert!(
        rows.iter().any(|r| r.contains(" Packets ")),
        "a short header never breaks mid-word: {rows:#?}"
    );
}

#[test]
fn table_cells_wrap_across_rows_instead_of_truncating() {
    // A cell wider than its column WRAPS across rows (hard-breaking an
    // unbreakable token grapheme-by-grapheme, wide CJK included) — never
    // truncated with a `…`, so no cell content is ever lost.
    let cells = vec![table_cell_segments("世界世界", Style::default())]; // 8 cols
    let rows = table_row_lines(&cells, &[5], &[markdown::Alignment::None]);
    let text = rows_text(&rows);
    assert!(
        rows.len() >= 2,
        "a too-wide cell wraps into >1 row: {text:?}"
    );
    assert!(
        !text.join("").contains('…'),
        "cells wrap, never truncate: {text:?}"
    );
    let kept: String = text
        .iter()
        .flat_map(|r| r.chars())
        .filter(|c| matches!(c, '世' | '界'))
        .collect();
    assert_eq!(
        kept, "世界世界",
        "no cell content lost to wrapping: {text:?}"
    );
}

#[test]
fn allocate_column_widths_keeps_a_grid_that_fits_natural() {
    // Fits: the natural grid (2+3 content + 7 overhead = 12) is ≤ avail, so
    // columns keep their natural widths and the table stays as narrow as its
    // content — never stretched to the terminal.
    assert_eq!(allocate_column_widths(&[2, 3], &[2, 3], 40), vec![2, 3]);
}

#[test]
fn narrow_table_wraps_cells_into_taller_rows_no_ellipsis() {
    // The reported fix (img1 → img2): when the natural grid overflows the
    // width, cells WORD-WRAP into taller rows instead of truncating with `…`,
    // so no content is ever lost. Width 28 keeps this a grid — the email
    // column levels to 13 and wraps twice; any narrower and the records
    // fallback (its own tests) takes over.
    let lines: Vec<String> = [
        "| Name | Email |",
        "|------|-------|",
        "| John Doe | john.doe@example.com |",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let text = rows_text(&table_content_rows(&lines, 28));
    assert!(
        !text.join("").contains('…'),
        "cells wrap, never truncate: {text:?}"
    );
    let mid = text.iter().position(|r| r.starts_with('├')).unwrap();
    let bot = text.iter().position(|r| r.starts_with('└')).unwrap();
    let data = &text[mid + 1..bot];
    assert!(data.len() >= 2, "the data row wraps across rows: {text:?}");
    // The email column's content survives intact across the wrapped rows
    // (a spaceless token is only hard-broken, never dropped).
    let email: String = data
        .iter()
        .map(|r| {
            r.split('│')
                .nth(2)
                .map_or(String::new(), |c| c.trim().to_string())
        })
        .collect();
    assert_eq!(
        email, "john.doe@example.com",
        "wrapped cell content is preserved: {text:?}"
    );
}

#[test]
fn very_narrow_table_renders_as_key_value_records() {
    // At a very narrow width the grid is too cramped to scan, so it flips to
    // codex-style key/value records: no box-drawing, a `label value` field
    // per column, a `─` rule between rows, nothing truncated.
    let table = "| Name | Email | Role |\n|------|-------|------|\n\
                 | Alice Johnson | alice@example.com | Administrator |\n\
                 | Bob Smith | bob@example.com | Editor |";
    let lines = message_lines(Role::Assistant, table, 24);
    let text: Vec<String> = lines
        .iter()
        .map(|l| plain(l).trim_end().to_string())
        .collect();
    let joined = text.join("\n");
    // Records mode has NO grid box-drawing.
    assert!(
        !joined.contains('│') && !joined.contains('┌') && !joined.contains('┐'),
        "records mode has no grid borders: {text:?}"
    );
    // Every column label appears as a field key (once per record).
    for label in ["Name", "Email", "Role"] {
        assert!(
            text.iter().any(|r| r.contains(label)),
            "label {label}: {text:?}"
        );
    }
    // Nothing truncated; content survives even where a value wrapped.
    assert!(!joined.contains('…'), "records never truncate: {text:?}");
    let squished: String = joined.split_whitespace().collect();
    for needle in [
        "alice@example.com",
        "Administrator",
        "bob@example.com",
        "BobSmith",
    ] {
        assert!(
            squished.contains(needle),
            "content preserved: {needle} in {text:?}"
        );
    }
    // A `─` rule separates the two records (rows carry the bullet indent).
    assert!(
        text.iter().any(|r| {
            let t = r.trim();
            !t.is_empty() && t.chars().all(|c| c == '─')
        }),
        "a separator rule between records: {text:?}"
    );
}

#[test]
fn record_block_uses_colon_key_value_fields() {
    // Records render as Claude Code-style `label: value` fields — a colon and
    // a single space, no aligned label column, no trailing padding.
    let labels = vec!["Name".to_string(), "Role".to_string()];
    let block = table_record_block(&labels, "| Alice Johnson | Admin |", 40);
    let rows: Vec<String> = block
        .iter()
        .map(|r| r.iter().map(|s| s.content.to_string()).collect())
        .collect();
    assert_eq!(rows, vec!["Name: Alice Johnson", "Role: Admin"]);
}

#[test]
fn record_separator_caps_its_width() {
    // The `─` rule between records caps at TABLE_RECORD_SEPARATOR_WIDTH rather
    // than spanning the full content width (Claude Code's shorter separator),
    // but still shrinks to fit a narrow content width.
    let wide: String = table_record_separator(80)
        .iter()
        .map(|s| s.content.to_string())
        .collect();
    assert_eq!(cols(&wide), TABLE_RECORD_SEPARATOR_WIDTH);
    let narrow: String = table_record_separator(15)
        .iter()
        .map(|s| s.content.to_string())
        .collect();
    assert_eq!(cols(&narrow), 15);
}

#[test]
fn moderately_narrow_table_stays_a_wrapping_grid() {
    // Records must not over-trigger: a moderately narrow table still renders
    // as a box-drawing grid (cells wrap into taller rows). Records only kick
    // in when the grid is genuinely cramped (docs/table-streaming.md).
    let table = "| Name | Email | Role |\n|------|-------|------|\n\
                 | Alice Johnson | alice@example.com | Administrator |\n\
                 | Bob Smith | bob@example.com | Editor |";
    let joined: String = message_lines(Role::Assistant, table, 48)
        .iter()
        .map(plain)
        .collect();
    assert!(
        joined.contains('│') && joined.contains('┌'),
        "a moderately narrow table stays a grid: {joined:?}"
    );
}

#[test]
fn table_should_use_records_only_when_narrow_and_cramped() {
    let header = "| Name | Email | Role |";
    let rows = vec!["| Alice Johnson | alice@example.com | Administrator |".to_string()];
    // Wide terminal → scannable grid, not records.
    let wide = table_column_widths(header, &rows, 3, 80);
    assert!(
        !table_should_use_records(header, &rows, &wide),
        "wide stays a grid"
    );
    // Very narrow → columns starved and cells wrap tall → records.
    let narrow = table_column_widths(header, &rows, 3, 24);
    assert!(
        table_should_use_records(header, &rows, &narrow),
        "narrow flips to records"
    );
    // The decision sees EVERY row, not just the first: a cramped cell in a
    // later row flips the block too (full knowledge, like the widths).
    let later = vec![
        "| a | b | c |".to_string(),
        "| Alice Johnson | alice@example.com | Administrator |".to_string(),
    ];
    let later_w = table_column_widths(header, &later, 3, 24);
    assert!(
        table_should_use_records(header, &later, &later_w),
        "a cramped later row flips to records"
    );
    // A single-column table is just a list — never records.
    let one = vec!["| a very long value that wraps a lot here |".to_string()];
    let one_col = table_column_widths("| X |", &one, 1, 12);
    assert!(
        !table_should_use_records("| X |", &one, &one_col),
        "single-column never records"
    );
}

#[test]
fn table_preview_caps_to_its_newest_rows() {
    // A forming table taller than the strip's budget tail-follows: the cap
    // keeps the NEWEST rows (the frontier stays visible), dropping the top
    // (docs/table-streaming.md).
    let mut full = String::from("| A | B |\n|---|---|\n");
    for i in 0..30 {
        full.push_str(&format!("| r{i} | v{i} |\n"));
    }
    let width = 40;
    let mut render = StreamRender::new();
    let _ = render.commit(&full, width);
    let capped: Vec<String> = render.preview(&full, width, 6).iter().map(plain).collect();
    assert_eq!(capped.len(), 6, "capped to the budget: {capped:?}");
    assert!(
        capped.iter().any(|r| r.contains("r29")),
        "the newest row stays visible: {capped:?}"
    );
    assert!(
        !capped.iter().any(|r| r.contains('┌')),
        "the top border scrolled out of the capped window: {capped:?}"
    );
    // Uncapped, the whole forming block previews.
    assert!(render.preview(&full, width, usize::MAX).len() > 30);
}

#[test]
fn assistant_message_renders_a_table_inline() {
    // A table inside a reply: bullet on the first row, the grid indented under
    // it, an interior blank kept between the prose and the table. The header
    // labels are centred over their left-aligned data (`header_align`).
    let text = "Here:\n\n| Name | Type |\n|------|------|\n| Alpha | String |";
    let rows: Vec<String> = message_lines(Role::Assistant, text, 40)
        .iter()
        .map(plain)
        .collect();
    assert_eq!(
        rows,
        vec![
            "● Here:",
            "  ",
            "  ┌───────┬────────┐",
            "  │ Name  │  Type  │",
            "  ├───────┼────────┤",
            "  │ Alpha │ String │",
            "  └───────┴────────┘",
        ]
    );
}

#[test]
fn assistant_table_sizes_columns_from_all_rows() {
    // The reported issue: a key/value table whose header is EMPTY (`| | |`)
    // and whose first data row is much narrower than the later ones. The old
    // lock-at-first-data-row streaming sized column 1 from "Time" (4) and
    // column 2 from "03:00 (CEST, GMT+2)" (19), shattering every later row
    // into slivers ("Temp/erat/ure", "10.1 km/h from NNE/(28°)"). Widths
    // must fit ALL rows (docs/table-streaming.md).
    let text = "**Warsaw, Poland**\n\n\
                | | |\n\
                |---|---|\n\
                | **Time** | 03:00 (CEST, GMT+2) |\n\
                | **Temperature** | 20.2 °C |\n\
                | **Wind** | 10.1 km/h from NNE (28°) |\n\
                | **Condition** | Clear sky (WMO code 0) |\n\
                | **Day/Night** | Night |\n\n\
                Data from Open-Meteo.";
    let rows: Vec<String> = message_lines(Role::Assistant, text, 80)
        .iter()
        .map(plain)
        .collect();
    assert!(
        rows.iter().any(|r| r.contains("│ Temperature │ 20.2 °C")),
        "columns fit every row, not just the first: {rows:#?}"
    );
    assert!(
        rows.iter()
            .any(|r| r.contains("│ 10.1 km/h from NNE (28°)")),
        "the widest cell sits on one line: {rows:#?}"
    );
}

#[test]
fn assistant_table_gives_the_content_heavy_column_the_room() {
    // The reported screenshot (img1 → img2): one long cell in the Check
    // column pulled it to ~35 columns — width every *other* row wasted —
    // while the Result column, holding a ~100-column value, was starved into
    // four wrapped rows. Claude Code gives each column what its longest word
    // needs and spends the rest where the content is, so Result ends up the
    // wide one.
    let text = "| Check | Result |\n\
                |-------|--------|\n\
                | `cargo fmt --check` | ✅ clean |\n\
                | `cargo clippy --all-targets -- -D warnings` | ✅ pass (exit 0) |\n\
                | `cargo test` | ✅ all tests pass, 0 failures (23 live-API tests \
                skipped — need `OPENROUTER_API_KEY` / `A0_VENICE_API_KEY`) |\n\
                | tmux | ✅ installed — tmux 3.6b (`/usr/bin/tmux`) |";
    let rows: Vec<String> = message_lines(Role::Assistant, text, 80)
        .iter()
        .map(plain)
        .collect();
    let w = grid_column_widths(&rows);
    assert!(
        w[1] > w[0],
        "the content-heavy column gets the room: {w:?} in {rows:#?}"
    );
    assert!(
        rows.iter().any(|r| r.contains("│ cargo fmt --check ")),
        "a short cell still sits on one line: {rows:#?}"
    );
    assert!(
        rows.iter().any(|r| r.contains("0 failures (23 live-API")),
        "the wide value keeps a scannable run per row: {rows:#?}"
    );
}

#[test]
fn table_rows_with_mixed_emoji_clusters_render_the_same_width() {
    // Every rendered row — borders and content rows alike — spans the same
    // display columns whatever mix of clusters the cells hold, so the `│`
    // seams line up.
    let lines: Vec<String> = [
        "| Status | Name | Note |",
        "| --- | --- | --- |",
        "| ✅ | build | emoji cell |",
        "| ⚠\u{FE0F} | lint 🔥 | mixed 👍🏽 emoji |",
        "| 👨\u{200D}👩\u{200D}👧\u{200D}👦 | family | ZWJ cluster |",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let rows = table_content_rows(&lines, 60);
    assert!(rows.len() > 5, "a full grid renders: {rows:?}");
    let widths: Vec<usize> = rows
        .iter()
        .map(|r| r.iter().map(|s| cols(&s.content)).sum())
        .collect();
    assert!(
        widths.windows(2).all(|w| w[0] == w[1]),
        "every grid row spans the same columns: {widths:?}"
    );
}

#[test]
fn emoji_table_rows_all_end_at_the_same_column() {
    // A 2-column-wide grapheme must be measured as two columns everywhere —
    // the cell's natural width, its wrap, and its pad — so an emoji row is
    // exactly as wide as a border row. (The terminal-side half of this bug
    // was the wide-grapheme filler cell the draw paths used to print, see
    // `term::visible_cells`.)
    let text = "| Check | Result |\n\
                |-------|--------|\n\
                | fmt | ✅ clean |\n\
                | test | ❌ 3 failures |\n\
                | docs | 世界 ok |";
    for width in [24u16, 40, 60, 80, 120] {
        let rows: Vec<String> = message_lines(Role::Assistant, text, width)
            .iter()
            .map(plain)
            .collect();
        let grid: Vec<&String> = rows.iter().filter(|r| r.contains('│')).collect();
        assert!(!grid.is_empty(), "a grid at width {width}: {rows:#?}");
        let widths: Vec<usize> = rows
            .iter()
            .filter(|r| {
                let t = r.trim_start();
                t.starts_with('│') || t.starts_with('┌') || t.starts_with('├') || t.starts_with('└')
            })
            .map(|r| cols(r.trim_end()))
            .collect();
        assert!(
            widths.windows(2).all(|p| p[0] == p[1]),
            "every grid row is the same width at {width}: {widths:?} in {rows:#?}"
        );
        assert!(
            widths[0] <= width as usize,
            "the grid fits the terminal at {width}: {widths:?}"
        );
    }
}

#[test]
fn assistant_table_joins_hard_wrapped_rows() {
    // The reported bug: the model echoed a table whose source carries a
    // terminal line break MID-ROW — google's RTT cell split
    // (`… | 86.8 / 89.8 /` ␤ `96.4 ms |`, the first piece unterminated) and
    // facebook's last cell wholly on the next line (`… | 0% |` ␤
    // `15.0 / 16.4 / 17.9 ms |`). Strict GFM reads each line as a row, so
    // the fragments became phantom one-cell rows (`│ 96.4 ms │ │ │ …`).
    // Inside a leading-pipe table a genuine row starts with `|`; a
    // pipe-carrying line that doesn't is the previous row's tail and must
    // re-join it (docs/table-streaming.md).
    let text = "All three pings completed in parallel. Here's the summary:\n\n\
                | Host | IP | Packets | Loss | RTT min/avg/max |\n\
                |------|----|---------|------|-----------------|\n\
                | **google.com** | 2404:6800:4017:809::200e | 10/10 | 0% | 86.8 / 89.8 /\n\
                96.4 ms |\n\
                | **facebook.com** | 2a03:2880:f372:1:face:b00c:0:25de | 10/10 | 0% |\n\
                15.0 / 16.4 / 17.9 ms |\n\
                | **x.com** | 162.159.140.229 | 10/10 | 0% | 14.4 / 15.5 / 16.2 ms |\n\n\
                done";
    let rows: Vec<String> = message_lines(Role::Assistant, text, 120)
        .iter()
        .map(plain)
        .collect();
    assert!(
        rows.iter().any(|r| r.contains("86.8 / 89.8 / 96.4 ms")),
        "google's split RTT cell is rejoined: {rows:#?}"
    );
    assert!(
        rows.iter().any(|r| r.contains("│ 15.0 / 16.4 / 17.9 ms")),
        "facebook's wrapped last cell lands in the RTT column: {rows:#?}"
    );
    assert!(
        !rows.iter().any(|r| r.trim_start().starts_with("│ 96.4 ms")),
        "no phantom row from a wrapped fragment: {rows:#?}"
    );
    assert_eq!(
        rows.iter().filter(|r| r.contains('├')).count(),
        3,
        "three data rows → header rule + two inter-row rules: {rows:#?}"
    );
}

#[test]
fn wrapped_row_join_accepts_fragments_and_rejects_real_rows() {
    // A pipe-carrying fragment that doesn't start with `|` re-joins the
    // previous row — the mid-cell wrap (unterminated first piece)…
    assert_eq!(
        join_wrapped_table_row("| a | b | 86.8 /", "96.4 ms |", 3).as_deref(),
        Some("| a | b | 86.8 / 96.4 ms |")
    );
    // …and the at-cell-boundary wrap (first piece `|`-terminated but short).
    assert_eq!(
        join_wrapped_table_row("| a | b |", "c |", 3).as_deref(),
        Some("| a | b | c |")
    );
    // A line that DECLARES itself a row (leading `|`) never joins.
    assert_eq!(join_wrapped_table_row("| a | b | c /", "| d |", 3), None);
    // A join that would overflow the column count is a real row, not a tail.
    assert_eq!(join_wrapped_table_row("| a | b | c |", "d | e |", 3), None);
    // In the no-leading-pipe row style, rows legitimately don't start with
    // `|`, so a pipe-carrying follow-up is a row of its own — never joined
    // even when the merged cell count would fit.
    assert_eq!(join_wrapped_table_row("a | b", "c | d", 4), None);
}

#[test]
fn a_failed_command_cell_surfaces_its_exit_code() {
    // A red cell says WHY it failed: the display reframes the model-facing
    // `Exit code: N` line as an `Error: Exit code N` header above the body
    // (the raw frame stays in tool.output for the model / context replay).
    let lines = tool_lines(
        &tool("Bash", "false", ToolStatus::Failed, "Exit code: 3\nboom"),
        80,
        &PathDisplay::VERBATIM,
    );
    let body: Vec<String> = lines[1..].iter().map(plain).collect();
    assert!(
        body[0].contains("Error: Exit code 3"),
        "the failure header leads: {body:?}"
    );
    assert!(body[1].contains("boom"), "the body follows: {body:?}");
}

#[test]
fn a_failed_command_cell_with_no_body_still_says_why() {
    // A failure with empty output used to show "(no output)" — now the
    // exit code itself is the body.
    let lines = tool_lines(
        &tool("Bash", "false", ToolStatus::Failed, "Exit code: 3"),
        80,
        &PathDisplay::VERBATIM,
    );
    let body: Vec<String> = lines[1..].iter().map(plain).collect();
    assert_eq!(body.len(), 1, "one row: {body:?}");
    assert!(
        body[0].contains("Error: Exit code 3"),
        "the reason shows: {body:?}"
    );
}

#[test]
fn a_signal_killed_command_cell_says_so() {
    let lines = tool_lines(
        &tool(
            "Bash",
            "x",
            ToolStatus::Failed,
            "Exit code: killed by signal",
        ),
        80,
        &PathDisplay::VERBATIM,
    );
    assert!(
        plain(&lines[1]).contains("Error: killed by signal"),
        "got {:?}",
        plain(&lines[1])
    );
}

#[test]
fn bash_cell_output_aligns_under_the_two_space_corner() {
    // Claude-Code's bash cell: the corner is two spaces wide, so output
    // opens at `  ⎿  {line}` (col 5) and the `… +N lines` hint aligns under
    // it.
    let out = "l1\nl2\nl3\nl4\nl5\nl6";
    let lines = tool_lines(
        &tool("Bash", "seq 6", ToolStatus::Ok, out),
        80,
        &PathDisplay::VERBATIM,
    );
    assert_eq!(plain(&lines[0]), "● Bash(seq 6)");
    assert_eq!(plain(&lines[1]), "  ⎿  l1");
    assert_eq!(
        plain(lines.last().unwrap()),
        "     … +3 lines (ctrl+o to expand)"
    );
}

#[test]
fn read_cell_full_view_shows_every_row_uncapped() {
    let body: String = (1..=30)
        .map(|i| format!("{i:>2} row {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let lines = tool_full_lines(
        &tool("Read", "big.txt", ToolStatus::Ok, &body),
        80,
        &PathDisplay::VERBATIM,
    );
    assert_eq!(lines.len(), 2 + 30, "header + summary + every row");
    assert!(plain(lines.last().unwrap()).contains("30 row 30"));
}

#[test]
fn edit_cell_renders_the_hunk_gap_dim() {
    let output = "Updated a.rs (+2 -0)\n1 +first()\n  ⋮\n9 +second()";
    let lines = tool_full_lines(
        &tool("Edit", "a.rs", ToolStatus::Ok, output),
        80,
        &PathDisplay::VERBATIM,
    );
    let gap = lines
        .iter()
        .find(|l| plain(l).trim_end().ends_with('⋮'))
        .expect("the ⋮ gap row is rendered");
    assert_eq!(gap.spans.last().unwrap().style.fg, Some(tool_dim_color()));
}

#[test]
fn preview_rows_counts_the_running_backend_tool_cell() {
    // The strip's preview slot is sized by `preview_rows`: nothing to preview
    // → 0; a streaming reply → 1; a running backend tool → its whole collapsed
    // cell (header + `⎿ Running…`), so a long command isn't clipped live.
    let mut app = App::new();
    app.begin_stream();
    assert_eq!(
        preview_rows(&app, 40),
        0,
        "empty pre-stream pause: no preview"
    );
    app.push_chunk("hi");
    assert_eq!(
        preview_rows(&app, 40),
        1,
        "a streaming reply previews one row"
    );
    app.start_tool("Bash", "sleep 1", None);
    // Past the hint delay so the Ctrl+B hint row is part of the preview.
    app.set_command_elapsed(Some(Duration::from_secs(3)));
    assert_eq!(
        preview_rows(&app, 40),
        3,
        "a running backend tool previews header + ⎿ Running… + the Ctrl+B hint"
    );
}

#[test]
fn a_rejected_cell_shows_the_amended_instructions_and_never_the_model_text() {
    // The mirror image of the backgrounded cell: here the *display* is the
    // richer text (Tab's instructions under the rejection headline) and the
    // model-facing result is the one that never renders — it rides
    // `context_output` for the derived context (docs/permissions.md).
    let tool = ToolCall {
        name: "Write".to_string(),
        args: "hello.py".to_string(),
        status: ToolStatus::Failed,
        output: "User rejected write to hello.py\nInstructions: just print it instead".to_string(),
        timestamp: String::new(),
        shell: false,
        truncated: false,
        context_output: Some(
            "The user doesn't want to proceed with this tool use. STOP what you are doing."
                .to_string(),
        ),
        arguments: None,
        approval_note: None,
        batch: None,
    };
    for lines in [
        tool_lines(&tool, 80, &PathDisplay::VERBATIM),
        tool_full_lines(&tool, 80, &PathDisplay::VERBATIM),
    ] {
        let texts: Vec<String> = lines.iter().map(plain).collect();
        let joined = texts.join("\n");
        assert!(texts[0].contains("Write(hello.py)"), "{texts:?}");
        assert!(
            joined.contains("User rejected write to hello.py"),
            "the headline is the cell's first output row: {texts:?}"
        );
        assert!(
            joined.contains("Instructions: just print it instead"),
            "the typed instructions are recorded on the cell: {texts:?}"
        );
        assert!(
            !joined.contains("STOP what you are doing"),
            "the model-facing result never renders: {texts:?}"
        );
    }
    assert_eq!(
        tool_lines(&tool, 80, &PathDisplay::VERBATIM)[0].spans[0]
            .style
            .fg,
        Some(tool_fail_color()),
        "a refused call keeps the red bullet"
    );
}

#[test]
fn a_backgrounded_tool_cell_shows_the_fixed_row_not_its_output() {
    // The stored output is the model-facing launch text — the cell (inline
    // and expanded alike) shows the fixed backgrounded row instead, under
    // a green header bullet (the launch succeeded).
    let tool = ToolCall {
        name: "Bash".to_string(),
        args: "ping -c 50 google.com".to_string(),
        status: ToolStatus::Backgrounded,
        output:
            "Command running in the background. Output is streaming to /tmp/a0/s1/bash_1.output."
                .to_string(),
        timestamp: String::new(),
        shell: false,
        truncated: false,
        context_output: None,
        arguments: None,
        approval_note: None,
        batch: None,
    };
    for lines in [
        tool_lines(&tool, 60, &PathDisplay::VERBATIM),
        tool_full_lines(&tool, 60, &PathDisplay::VERBATIM),
    ] {
        let texts: Vec<String> = lines.iter().map(plain).collect();
        assert_eq!(texts.len(), 2, "header + the fixed row: {texts:?}");
        assert!(texts[0].contains("Bash(ping -c 50 google.com)"));
        assert_eq!(
            texts[1].trim(),
            "⎿  Running in the background (↓ to manage)"
        );
        assert!(
            !texts.join("\n").contains("Command running"),
            "the model-facing text never renders: {texts:?}"
        );
    }
    assert_eq!(
        tool_lines(&tool, 60, &PathDisplay::VERBATIM)[0].spans[0]
            .style
            .fg,
        Some(tool_ok_color()),
        "a backgrounded launch gets the green bullet"
    );
}

#[test]
fn a_backgrounded_shell_cell_is_the_headerless_fixed_row() {
    let tool = ToolCall {
        name: "ping x.com".to_string(),
        args: String::new(),
        status: ToolStatus::Backgrounded,
        output: "[moved to background; the final output will follow when it completes]".to_string(),
        timestamp: String::new(),
        shell: true,
        truncated: false,
        context_output: None,
        arguments: None,
        approval_note: None,
        batch: None,
    };
    let texts: Vec<String> = tool_lines(&tool, 60, &PathDisplay::VERBATIM)
        .iter()
        .map(plain)
        .collect();
    assert_eq!(texts.len(), 1);
    assert_eq!(
        texts[0].trim(),
        "⎿  Running in the background (↓ to manage)"
    );
}

// --- markdown tables (docs/markdown.md) ---

/// The plain text of each content row (span contents concatenated).
fn rows_text(rows: &[Vec<Span<'static>>]) -> Vec<String> {
    rows.iter()
        .map(|r| r.iter().map(|s| s.content.as_ref()).collect::<String>())
        .collect()
}

/// The rendered cell widths of a table's grid, read off its `┌──┬──┐` top
/// border — the widths the columns were actually allocated, minus the two
/// pad spaces each side of a cell.
fn grid_column_widths(rows: &[String]) -> Vec<usize> {
    // The border may carry the message bullet (`● ┌──…`) when the table is
    // the reply's first block, so slice from the corner itself.
    let top = rows
        .iter()
        .find_map(|r| r.find('┌').map(|i| &r[i..]))
        .expect("a grid top border");
    top.trim()
        .trim_start_matches('┌')
        .trim_end_matches('┐')
        .split('┬')
        .map(|seg| cols(seg).saturating_sub(2))
        .collect()
}
