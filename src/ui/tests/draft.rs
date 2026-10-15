//! The live `write` cell — a call the model is still generating, its file
//! streaming into the strip (`docs/write-streaming.md`).

use super::*;
use crate::stream::ToolCallFragment;
use crate::ui::draft::{draft_full_lines, draft_lines_highlighted, draft_write_lines};
use crate::ui::live::{preview_lines, strip_flow_key};
use crate::ui::theme::{EXPAND_HINT, FILE_PEEK_LINES, TOOL_LINE_ELLIPSIS, TOOL_LINE_MAX_ROWS};

/// The JSON a model streams for a `write` of `content` to `path` — cut off
/// before the content's closing quote unless `complete`.
fn write_arguments(path: Option<&str>, content: &str, complete: bool) -> String {
    let mut json = String::from("{");
    if let Some(path) = path {
        json.push_str(&format!(r#""path":{},"#, serde_json::json!(path)));
    }
    json.push_str(r#""content":"#);
    let encoded = serde_json::json!(content).to_string();
    if complete {
        json.push_str(&encoded);
        json.push('}');
    } else {
        json.push_str(&encoded[..encoded.len() - 1]);
    }
    json
}

/// A turn in flight with one `write` draft streaming.
fn drafting(path: Option<&str>, content: &str, complete: bool) -> App {
    let mut app = App::new();
    app.set_session_info("dummy_model_name", "~/repo");
    app.begin_stream();
    app.push_tool_call_fragment(&ToolCallFragment {
        index: 0,
        name: Some("write".to_string()),
        arguments: write_arguments(path, content, complete),
    });
    app
}

/// The rows the app's one draft renders as, at rest.
fn cell(app: &App, width: u16) -> Vec<String> {
    let draft = app.drafts().writes().next().expect("a write draft");
    draft_write_lines(draft, None, width, &PathDisplay::VERBATIM)
        .iter()
        .map(plain)
        .collect()
}

/// `count` numbered lines of plain text: `line 1` … `line {count}`.
fn numbered(count: usize) -> String {
    (1..=count)
        .map(|i| format!("line {i}"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn a_write_being_generated_shows_its_header_over_a_writing_corner() {
    let rows = cell(&drafting(Some("a.py"), "", false), 80);
    assert_eq!(rows, vec!["● Write(a.py)", "  ⎿  Writing…"]);
}

#[test]
fn the_header_names_no_path_until_its_string_has_closed() {
    let mut app = App::new();
    app.begin_stream();
    app.push_tool_call_fragment(&ToolCallFragment {
        index: 0,
        name: Some("write".to_string()),
        arguments: r#"{"path":"src/ga"#.to_string(),
    });
    assert_eq!(cell(&app, 80)[0], "● Write", "never a half-typed path");
}

#[test]
fn the_corner_counts_the_lines_written_so_far() {
    assert_eq!(
        cell(&drafting(Some("a"), "x", false), 80)[1],
        "  ⎿  Writing 1 line…"
    );
    assert_eq!(
        cell(&drafting(Some("a"), "x\ny\nz", false), 80)[1],
        "  ⎿  Writing 3 lines…"
    );
}

#[test]
fn while_the_content_streams_the_cell_shows_its_newest_lines() {
    let rows = cell(&drafting(Some("big.txt"), &numbered(25), false), 80);
    assert_eq!(rows[1], "  ⎿  Writing 25 lines…");
    let body = &rows[2..];
    assert_eq!(
        body.len(),
        FILE_PEEK_LINES,
        "a window of ten rows: {rows:?}"
    );
    assert_eq!(body[0], "      16 line 16", "{rows:?}");
    assert_eq!(
        body[FILE_PEEK_LINES - 1],
        "      25 line 25",
        "the line being typed is the last row"
    );
    assert!(
        !rows.iter().any(|r| r.contains("line 15")),
        "older lines have scrolled out of the window: {rows:?}"
    );
    assert!(
        !rows.iter().any(|r| r.contains(EXPAND_HINT)),
        "the tail carries no hint — its line numbers say where it is"
    );
}

#[test]
fn the_line_still_being_typed_shows_as_it_grows() {
    let rows = cell(&drafting(Some("a.py"), "import os\nprin", false), 80);
    assert_eq!(rows.last().map(String::as_str), Some("      2 prin"));
}

#[test]
fn a_newline_just_typed_opens_no_empty_row_yet() {
    // `str::lines`' rule, the corner's count and the finished cell's alike: a
    // trailing newline closes its line rather than starting a blank one.
    let rows = cell(&drafting(Some("a.py"), "one\ntwo\n", false), 80);
    assert_eq!(rows[1], "  ⎿  Writing 2 lines…");
    assert_eq!(rows.last().map(String::as_str), Some("      2 two"));
}

#[test]
fn once_the_content_closes_the_cell_shows_its_first_lines_over_the_hint() {
    // After the file has streamed: the head — the shape the committed
    // `Wrote N lines` cell takes, so nothing jumps when the call resolves.
    let rows = cell(&drafting(Some("big.txt"), &numbered(25), true), 80);
    assert_eq!(rows[1], "  ⎿  Writing 25 lines…");
    assert_eq!(rows[2], "       1 line 1", "{rows:?}");
    assert_eq!(
        rows[2 + FILE_PEEK_LINES - 1],
        "      10 line 10",
        "{rows:?}"
    );
    assert_eq!(
        rows.last().map(String::as_str),
        Some(format!("     … +15 lines{EXPAND_HINT}").as_str())
    );
    assert_eq!(rows.len(), 2 + FILE_PEEK_LINES + 1);
}

#[test]
fn a_short_file_shows_whole_once_complete_with_no_hint() {
    let rows = cell(&drafting(Some("a.txt"), "a\nb\nc", true), 80);
    assert_eq!(
        rows,
        vec![
            "● Write(a.txt)",
            "  ⎿  Writing 3 lines…",
            "      1 a",
            "      2 b",
            "      3 c",
        ]
    );
}

#[test]
fn the_window_is_ten_display_rows_not_ten_lines() {
    // At 30 columns a 40-character line wraps: the window counts the rows it
    // paints, like the finished cell's peek (docs/long-lines.md).
    let content: String = (1..=12)
        .map(|i| format!("{i:02} {}", "x".repeat(37)))
        .collect::<Vec<_>>()
        .join("\n");
    let rows = cell(&drafting(Some("w.txt"), &content, false), 30);
    let body = &rows[2..];
    assert!(body.len() <= FILE_PEEK_LINES, "{rows:?}");
    assert!(
        body.last().is_some_and(|r| r.contains('x')),
        "the newest line still ends the window: {rows:?}"
    );
}

#[test]
fn a_huge_line_is_clipped_to_its_row_budget_and_marked() {
    let content = format!("ok\n{}", "z".repeat(5000));
    let rows = cell(&drafting(Some("min.js"), &content, false), 40);
    let body = &rows[2..];
    assert!(
        body.len() <= 1 + TOOL_LINE_MAX_ROWS,
        "one short line and the clipped one: {rows:?}"
    );
    assert!(
        body.last().is_some_and(|r| r.ends_with(TOOL_LINE_ELLIPSIS)),
        "the cut is marked: {rows:?}"
    );
}

#[test]
fn line_numbers_align_to_the_widest_one() {
    let rows = cell(&drafting(Some("a"), &numbered(12), false), 80);
    assert_eq!(rows[2], "       3 line 3");
    assert_eq!(rows[11], "      12 line 12");
}

#[test]
fn the_tail_is_coloured_with_the_files_own_lexer_state() {
    // A docstring opened above the window: highlighting from the window's
    // first line would colour its body as code. The draft carries the state
    // over every line it has already seen, so the row matches what a
    // whole-file highlight says (docs/write-streaming.md).
    let mut source = vec!["def f():".to_string(), "    \"\"\"".to_string()];
    source.extend((1..=20).map(|i| format!("    doc line {i}")));
    source.push("    \"\"\"".to_string());
    source.push("    return 1".to_string());
    let content = source.join("\n");
    let app = drafting(Some("doc.py"), &content, false);
    let draft = app.drafts().writes().next().unwrap();
    let rows = draft_write_lines(draft, None, 80, &PathDisplay::VERBATIM);
    let row = rows
        .iter()
        .find(|r| plain(r).ends_with("doc line 17"))
        .expect("the row is in the window");
    let lines: Vec<&str> = source.iter().map(String::as_str).collect();
    let whole = crate::highlight::highlight(&lines, Some("py"), crate::ui::palette::palette().code);
    let index = source.iter().position(|l| l == "    doc line 17").unwrap();
    let want = whole[index]
        .iter()
        .find(|seg| seg.text.contains("doc"))
        .expect("the doc text segment")
        .style
        .fg;
    let got = row
        .spans
        .iter()
        .find(|span| span.content.contains("doc"))
        .expect("the doc text span")
        .style
        .fg;
    assert_eq!(got, want, "the docstring's body keeps the string colour");
}

#[test]
fn a_new_piece_highlights_only_the_lines_it_completed() {
    // The strip is rebuilt every animation frame, several times per frame:
    // the memo highlights each complete line once (the `QUEUED_MEMO`
    // pattern, ui::footer) and answers repeat builds from the cache.
    let mut app = drafting(Some("a.py"), &numbered(30), false);
    let render = |app: &App| {
        let draft = app.drafts().writes().next().unwrap();
        draft_write_lines(draft, None, 80, &PathDisplay::VERBATIM)
    };
    let before = draft_lines_highlighted();
    render(&app);
    assert_eq!(
        draft_lines_highlighted() - before,
        29,
        "every complete line, once — the 30th is still being typed"
    );
    let before = draft_lines_highlighted();
    render(&app);
    render(&app);
    assert_eq!(
        draft_lines_highlighted() - before,
        0,
        "nothing new, nothing redone"
    );
    app.push_tool_call_fragment(&ToolCallFragment {
        index: 0,
        name: None,
        arguments: "!\\nline 31".to_string(),
    });
    let before = draft_lines_highlighted();
    let rows: Vec<String> = render(&app).iter().map(plain).collect();
    assert_eq!(
        draft_lines_highlighted() - before,
        1,
        "only the line it closed"
    );
    assert_eq!(rows.last().map(String::as_str), Some("      31 line 31"));
    assert!(rows.iter().any(|r| r.ends_with("line 30!")), "{rows:?}");
}

#[test]
fn the_strip_reserves_exactly_the_rows_the_draft_paints() {
    let app = drafting(Some("big.txt"), &numbered(25), false);
    let rows = preview_rows(&app, 80);
    assert_eq!(rows, 2 + FILE_PEEK_LINES as u16);
    assert_eq!(preview_lines(&app, 80, None, rows).len(), usize::from(rows));
}

#[test]
fn render_live_streams_the_write_above_the_box() {
    let app = drafting(Some("big.txt"), &numbered(25), false);
    let pv = preview_rows(&app, 80);
    let footer = footer_rows(&app, 0);
    let h = live_height(&app.input, 80, 40, true, pv, 0, 0, 0, 0, footer, 0);
    let mut buf = buffer(80, h);
    render_live(buf.area, &mut buf, &app);
    let all: String = (0..h).map(|y| row(&buf, y, 80) + "\n").collect();
    assert!(all.contains("Write(big.txt)"), "{all}");
    assert!(all.contains("Writing 25 lines…"), "{all}");
    assert!(all.contains("25 line 25"), "{all}");
}

#[test]
fn a_call_that_is_not_a_write_leaves_the_strip_alone() {
    let mut app = App::new();
    app.begin_stream();
    app.push_tool_call_fragment(&ToolCallFragment {
        index: 0,
        name: Some("bash".to_string()),
        arguments: r#"{"command":"ls"}"#.to_string(),
    });
    assert_eq!(preview_rows(&app, 80), 0, "nothing to preview");
}

#[test]
fn every_write_being_generated_gets_its_own_cell() {
    let mut app = drafting(Some("a.txt"), "first", true);
    app.push_tool_call_fragment(&ToolCallFragment {
        index: 1,
        name: Some("write".to_string()),
        arguments: write_arguments(Some("b.txt"), "second", false),
    });
    let rows = preview_tool_rows(&app);
    let headers: Vec<&String> = rows.iter().filter(|r| r.starts_with('●')).collect();
    assert_eq!(
        headers,
        vec!["● Write(a.txt)", "● Write(b.txt)"],
        "{rows:?}"
    );
    assert!(rows.contains(&String::new()), "blank-separated: {rows:?}");
}

/// The strip's preview rows for `app`, as text.
fn preview_tool_rows(app: &App) -> Vec<String> {
    let n = preview_rows(app, 80);
    preview_lines(app, 80, None, n).iter().map(plain).collect()
}

#[test]
fn the_full_view_numbers_every_line_written_so_far() {
    let app = drafting(Some("big.txt"), &numbered(25), false);
    let draft = app.drafts().writes().next().unwrap();
    let rows: Vec<String> = draft_full_lines(draft, 80, &PathDisplay::VERBATIM)
        .iter()
        .map(plain)
        .collect();
    assert_eq!(rows[0], "● Write(big.txt)");
    assert_eq!(rows[1], "  ⎿  Writing 25 lines…");
    assert_eq!(rows[2], "       1 line 1");
    assert_eq!(rows.last().map(String::as_str), Some("      25 line 25"));
    assert_eq!(rows.len(), 2 + 25, "nothing folded away");
}

#[test]
fn the_transcript_shows_the_whole_draft_and_follows_it_as_it_streams() {
    let mut app = drafting(Some("big.txt"), &numbered(25), false);
    let mut cache = TranscriptCache::new();
    let lines: Vec<String> = cache.lines(&app, 80).iter().map(plain).collect();
    assert!(lines.iter().any(|l| l == "       1 line 1"), "{lines:?}");
    assert!(lines.iter().any(|l| l == "      25 line 25"), "{lines:?}");
    app.push_tool_call_fragment(&ToolCallFragment {
        index: 0,
        name: None,
        arguments: "\\nline 26".to_string(),
    });
    assert_eq!(cache.lines(&app, 80), transcript_lines(&app, 80).as_slice());
    assert!(
        cache
            .lines(&app, 80)
            .iter()
            .any(|l| plain(l) == "      26 line 26"),
        "the overlay follows the streaming file"
    );
}

#[test]
fn an_agent_session_view_streams_the_agents_own_write() {
    let mut app = App::new();
    app.set_session_info("dummy_model_name", "~/repo");
    app.begin_stream();
    app.start_agent_group(
        false,
        &[crate::stream::AgentSpec {
            id: "a1".into(),
            description: "Write the game".into(),
            agent_type: "general-purpose".into(),
            prompt: "write it".into(),
            background: false,
            call_id: None,
            arguments: None,
        }],
    );
    app.apply_agent_event(
        "a1",
        &crate::stream::StreamEvent::ToolCallDelta(ToolCallFragment {
            index: 0,
            name: Some("write".to_string()),
            arguments: write_arguments(Some("game.py"), &numbered(14), false),
        }),
    );
    app.open_agent_view("a1");
    let rows = preview_tool_rows(&app);
    assert_eq!(
        rows.first().map(String::as_str),
        Some("● Write(game.py)"),
        "{rows:?}"
    );
    assert!(rows.iter().any(|r| r == "      14 line 14"), "{rows:?}");
    let transcript: Vec<String> = agent_transcript_lines(&app, 80)
        .expect("an agent view is up")
        .iter()
        .map(plain)
        .collect();
    assert!(
        transcript.iter().any(|r| r == "       1 line 1"),
        "its Ctrl+O shows the whole draft: {transcript:?}"
    );
}

#[test]
fn the_strip_flow_is_resigned_when_the_file_closes_not_as_it_grows() {
    // The flowed rows freeze in scrollback: a streamed line must not re-sign
    // them (that would purge-rebuild per piece), while the switch from the
    // tail to the head reshapes the cell and does (docs/strip-flow.md).
    let mut app = drafting(Some("a.txt"), &numbered(20), false);
    let streaming = strip_flow_key(&app);
    app.push_tool_call_fragment(&ToolCallFragment {
        index: 0,
        name: None,
        arguments: "\\nline 21".to_string(),
    });
    assert_eq!(
        strip_flow_key(&app),
        streaming,
        "a new line is not a new shape"
    );
    app.push_tool_call_fragment(&ToolCallFragment {
        index: 0,
        name: None,
        arguments: "\"}".to_string(),
    });
    assert_ne!(
        strip_flow_key(&app),
        streaming,
        "the head replaced the tail"
    );
}
