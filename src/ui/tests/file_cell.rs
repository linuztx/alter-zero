//! The streaming `write` cell: a file written into its cell while the model
//! writes it (`docs/write-streaming.md`).

use super::*;
use crate::app::{ToolDraft, ToolDrafts};
use crate::stream::ToolCallFragment;
use crate::ui::file_cell::draft_cell_lines;
use crate::ui::theme::FILE_PEEK_LINES;

const WIDTH: u16 = 80;

/// `n` source lines, `line 1` to `line n`, newline-joined with no trailing
/// newline — so the last one is still "arriving".
fn numbered_text(n: usize) -> String {
    (1..=n)
        .map(|i| format!("line {i}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Drafts holding one streamed `write` of `path` whose `content` has arrived
/// this far — the content string `closed` or still open.
fn drafts_with(path: &str, content: &str, closed: bool) -> ToolDrafts {
    let encoded = serde_json::to_string(content).expect("a string encodes");
    let content = if closed {
        format!("{encoded}}}")
    } else {
        encoded[..encoded.len() - 1].to_string()
    };
    let path = serde_json::to_string(path).expect("a string encodes");
    let mut drafts = ToolDrafts::default();
    drafts.push(&ToolCallFragment {
        index: 0,
        name: "write".to_string(),
        arguments: format!(r#"{{"path":{path},"content":{content}"#),
    });
    drafts
}

/// The one preview `drafts` holds.
fn draft(drafts: &ToolDrafts) -> &ToolDraft {
    drafts.previews().next().expect("a preview")
}

/// The strip's cell for a call still being generated.
fn streaming_cell(draft: &ToolDraft) -> Vec<Line<'static>> {
    draft_cell_lines(
        draft,
        None,
        Some(Duration::ZERO),
        WIDTH,
        &PathDisplay::VERBATIM,
        false,
    )
}

fn texts(lines: &[Line]) -> Vec<String> {
    lines.iter().map(plain).collect()
}

/// The finished `write` of `content` to `path` as it commits — the executor's
/// own report, through the ordinary inline cell.
fn committed_call(path: &str, content: &str) -> ToolCall {
    ToolCall {
        name: "Write".to_string(),
        args: path.to_string(),
        status: ToolStatus::Ok,
        output: crate::llm::tools::write_report(path, content),
        timestamp: String::new(),
        shell: false,
        truncated: false,
        context_output: None,
        arguments: None,
        approval_note: None,
        batch: None,
        call_id: None,
        position: None,
    }
}

/// The leading line number of a numbered body row.
fn number_of(row: &str) -> Option<usize> {
    row.split_whitespace().next()?.parse().ok()
}

#[test]
fn a_streaming_write_shows_its_newest_ten_rows_under_a_running_count() {
    let drafts = drafts_with("a.py", &numbered_text(17), false);
    let lines = texts(&streaming_cell(draft(&drafts)));
    assert_eq!(lines[0], "● Write(a.py)");
    assert_eq!(lines[1], "  ⎿  Writing 17 lines…");
    let numbers: Vec<Option<usize>> = lines[2..].iter().map(|row| number_of(row)).collect();
    assert_eq!(
        numbers,
        (8..=17).map(Some).collect::<Vec<_>>(),
        "the window ends at the line still arriving: {lines:#?}"
    );
    assert!(lines[lines.len() - 1].ends_with("line 17"));
}

#[test]
fn the_window_moves_down_the_file_as_lines_arrive() {
    let first_number = |n: usize| {
        let drafts = drafts_with("a.py", &numbered_text(n), false);
        let lines = texts(&streaming_cell(draft(&drafts)));
        (number_of(&lines[2]), lines.len())
    };
    assert_eq!(first_number(3), (Some(1), 5), "a short file shows whole");
    assert_eq!(first_number(10), (Some(1), 12));
    assert_eq!(first_number(11), (Some(2), 12), "the oldest row gives way");
    assert_eq!(first_number(40), (Some(31), 12));
}

#[test]
fn once_the_content_has_arrived_the_cell_shows_the_head_the_committed_cell_will() {
    let content = format!("{}\n", numbered_text(17));
    let drafts = drafts_with("a.py", &content, true);
    let lines = streaming_cell(draft(&drafts));
    let committed = tool_lines(
        &committed_call("a.py", &content),
        WIDTH,
        &PathDisplay::VERBATIM,
    );
    assert_eq!(plain(&lines[0]), plain(&committed[0]), "the same header");
    assert_eq!(plain(&lines[1]), "  ⎿  Writing 17 lines…");
    assert_eq!(
        plain(&committed[1]),
        "  ⎿  Wrote 17 lines to a.py",
        "only the corner row changes when it commits"
    );
    assert_eq!(
        lines[2..],
        committed[2..],
        "the body is the committed peek, row for row and style for style"
    );
    assert!(plain(&lines[lines.len() - 1]).contains("+7 lines (ctrl+o to expand)"));
}

#[test]
fn the_tail_is_coloured_as_if_the_whole_file_had_been_read() {
    // A docstring opened on line 1 and closed on line 9 makes lines 6 to 8
    // string text; a highlighter restarted at the window's top would colour
    // them as code.
    let content = "\"\"\"A module whose docstring\nruns long\n\nso that\nits middle\nfalls\ninside\nthe window\n\"\"\"\n\n\nimport os\n\n\ndef f():\n    return os.sep\n";
    let drafts = drafts_with("m.py", content, false);
    let lines = streaming_cell(draft(&drafts));
    let whole = crate::ui::tool::tool_full_lines(
        &committed_call("m.py", content),
        WIDTH,
        &PathDisplay::VERBATIM,
    );
    // The committed expansion's rows for the same lines, by their numbers.
    let body: Vec<&Line> = lines[2..].iter().collect();
    for row in body {
        let n = number_of(&plain(row)).expect("a numbered row");
        let same = whole
            .iter()
            .find(|line| number_of(&plain(line)) == Some(n))
            .expect("the committed expansion numbers every line");
        assert_eq!(
            row, same,
            "line {n} wears the colours the whole file gives it"
        );
    }
}

#[test]
fn a_long_line_spends_at_most_three_rows_in_the_window() {
    let content = format!("short\n{}\nend", "x".repeat(500));
    let drafts = drafts_with("a.txt", &content, false);
    let lines = texts(&streaming_cell(draft(&drafts)));
    let long_rows = lines.iter().filter(|row| row.contains("xxxx")).count();
    assert_eq!(
        long_rows, 3,
        "the collapsed cell's per-line budget: {lines:#?}"
    );
    assert!(
        lines.iter().any(|row| row.ends_with('…')),
        "the cut is marked"
    );
}

#[test]
fn before_its_first_line_the_corner_says_writing_alone() {
    let mut drafts = ToolDrafts::default();
    drafts.push(&ToolCallFragment {
        index: 0,
        name: "write".to_string(),
        arguments: r#"{"path":"a.py","#.to_string(),
    });
    let lines = texts(&streaming_cell(draft(&drafts)));
    assert_eq!(lines, ["● Write(a.py)", "  ⎿  Writing…"]);
}

#[test]
fn the_header_names_the_path_only_once_it_has_closed() {
    let mut drafts = ToolDrafts::default();
    drafts.push(&ToolCallFragment {
        index: 0,
        name: "write".to_string(),
        arguments: r#"{"path":"/tmp/half"#.to_string(),
    });
    let lines = texts(&streaming_cell(draft(&drafts)));
    assert_eq!(lines[0], "● Write");
}

#[test]
fn the_header_shows_the_path_the_way_the_session_reads_paths() {
    let drafts = drafts_with("/home/u/proj/src/a.py", "x = 1\n", false);
    let paths = PathDisplay::new("/home/u/proj", Some("/home/u".into()));
    let lines = texts(&draft_cell_lines(
        draft(&drafts),
        None,
        None,
        WIDTH,
        &paths,
        false,
    ));
    assert_eq!(lines[0], "● Write(src/a.py)");
}

#[test]
fn a_queued_write_keeps_its_head_under_waiting() {
    let content = format!("{}\n", numbered_text(12));
    let mut drafts = drafts_with("a.py", &content, true);
    drafts.adopt(["Write"]);
    let queued = drafts.queued().next().expect("an adopted draft");
    let tool = ToolCall {
        status: ToolStatus::Waiting,
        output: String::new(),
        ..committed_call("a.py", &content)
    };
    let lines = draft_cell_lines(
        queued,
        Some(&tool),
        None,
        WIDTH,
        &PathDisplay::VERBATIM,
        false,
    );
    let committed = tool_lines(
        &committed_call("a.py", &content),
        WIDTH,
        &PathDisplay::VERBATIM,
    );
    assert_eq!(plain(&lines[1]), "  ⎿  Waiting…");
    assert_eq!(lines[2..], committed[2..]);
}

/// A long docstring and then code: a peek that lost the grammar's state
/// between its first and newest lines would colour the code as a string.
fn docstring_then_code() -> String {
    let mut text = String::from("def f():\n    \"\"\"Doc\n");
    for i in 0..60 {
        text.push_str(&format!("    still doc {i}\n"));
    }
    text.push_str("    \"\"\"\n");
    for i in 0..20 {
        text.push_str(&format!("    value_{i} = compute({i})  # step {i}\n"));
    }
    text
}

#[test]
fn the_cells_rows_are_the_ctrl_o_views_rows() {
    // The strip colours only the lines its cell shows (docs/memory.md); the
    // rows it shows must be exactly the Ctrl+O view's — which colours every
    // line — for the same lines, styles and all.
    let body = |lines: &[Line<'static>]| lines[2..].to_vec();
    let content = docstring_then_code();
    let open = drafts_with("a.py", &content, false);
    let whole = body(&draft_cell_lines(
        draft(&open),
        None,
        None,
        WIDTH,
        &PathDisplay::VERBATIM,
        true,
    ));
    let tail = body(&streaming_cell(draft(&open)));
    assert_eq!(tail.len(), FILE_PEEK_LINES);
    assert_eq!(tail, whole[whole.len() - tail.len()..], "the newest rows");

    let closed = drafts_with("a.py", &content, true);
    let whole = body(&draft_cell_lines(
        draft(&closed),
        None,
        None,
        WIDTH,
        &PathDisplay::VERBATIM,
        true,
    ));
    let mut head = body(&streaming_cell(draft(&closed)));
    assert!(
        plain(&head.pop().expect("the hint")).contains("ctrl+o to expand"),
        "the head closes on the hint"
    );
    assert_eq!(head, whole[..head.len()], "the first rows");
}

#[test]
fn ctrl_o_shows_the_whole_file_as_it_streams() {
    let drafts = drafts_with("a.py", &numbered_text(17), false);
    let lines = texts(&draft_cell_lines(
        draft(&drafts),
        None,
        None,
        WIDTH,
        &PathDisplay::VERBATIM,
        true,
    ));
    let numbers: Vec<Option<usize>> = lines[2..].iter().map(|row| number_of(row)).collect();
    assert_eq!(numbers, (1..=17).map(Some).collect::<Vec<_>>());
    assert!(!lines.iter().any(|row| row.contains("ctrl+o to expand")));
}
