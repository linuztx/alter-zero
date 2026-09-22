//! The `/export` page and the plain-text export it produces
//! (`docs/export.md`).

use super::*;
use crate::app::ExportTarget;
use crate::ui::export_view::{export_menu_rows, export_view_lines};
use crate::ui::theme::{model_meta_color, model_selected_color};
use crate::ui::wrap::cols;

/// An app with session info, a conversation, and the page open (the
/// ordinary shape).
fn open_app() -> App {
    let mut app = with_session();
    app.record_user_message("hello");
    app.open_export_picker();
    app
}

/// The page with the highlight on row `selected`.
fn open_at(selected: usize) -> App {
    let mut app = open_app();
    app.export_picker.as_mut().expect("open").selected = selected;
    app
}

fn texts(app: &App, width: u16) -> Vec<String> {
    export_view_lines(app, width).iter().map(plain).collect()
}

fn is_rule(t: &str) -> bool {
    let t = t.trim();
    !t.is_empty() && t.chars().all(|c| c == '─')
}

// ===== the page =====

#[test]
fn closed_builds_nothing() {
    assert!(export_view_lines(&with_session(), 80).is_empty());
    assert_eq!(export_picker_height(&with_session(), 80, 200), None);
}

#[test]
fn the_page_is_framed_with_title_blurb_rows_description_and_hint() {
    let texts = texts(&open_app(), 80);
    assert!(is_rule(texts.first().expect("a top rule")), "top rule");
    assert!(is_rule(texts.last().expect("a bottom rule")), "bottom rule");
    for expect in [
        "Export conversation",
        "as plain text",
        "❯ 1. Copy to clipboard",
        "  2. Save to file",
        "Copies the transcript to the system clipboard",
        "↑↓ navigate  enter select  esc close",
    ] {
        assert!(
            texts.iter().any(|t| t.contains(expect)),
            "{expect:?} is on the page: {texts:?}"
        );
    }
}

#[test]
fn each_row_reads_its_label_and_nothing_after() {
    // The rows are the two answers, one glance wide; what each one does is
    // the description under the list, not a clause on the row.
    let texts = texts(&open_app(), 80);
    for expect in ["❯ 1. Copy to clipboard", "2. Save to file"] {
        assert!(
            texts.iter().any(|t| t.trim() == expect),
            "the row reads {expect:?} and nothing more: {texts:?}"
        );
    }
}

#[test]
fn the_description_follows_the_highlight_and_names_the_file_and_the_directory() {
    // Under the list sits the highlighted row's description (the `/settings`
    // shape): the clipboard row says where the text goes; the file row names
    // the file's shape and the directory it lands in — the session's own cwd
    // as the footer shows it, so the user knows where to look before the
    // file exists.
    let clipboard = texts(&open_at(0), 100);
    assert!(
        clipboard
            .iter()
            .any(|t| t.contains("Copies the transcript to the system clipboard")),
        "{clipboard:?}"
    );
    assert!(
        !clipboard.iter().any(|t| t.contains("conversation-")),
        "the clipboard row says nothing about a file: {clipboard:?}"
    );
    let file = texts(&open_at(1), 100);
    assert!(
        file.iter()
            .any(|t| t.contains("conversation-YYYY-MM-DD-HHMMSS.txt")),
        "the file row names the file's shape: {file:?}"
    );
    assert!(
        file.iter().any(|t| t.contains("~/alter-zero")),
        "the file row names the directory: {file:?}"
    );
    assert!(
        !file
            .iter()
            .any(|t| t.contains("Copies the transcript to the system clipboard")),
        "only the highlighted row's description shows: {file:?}"
    );
    // Before the boundary injects the session info the directory has no
    // display form yet — the page says what it can.
    let mut app = App::new();
    app.record_user_message("hello");
    app.open_export_picker();
    app.export_picker.as_mut().expect("open").selected = 1;
    let bare: Vec<String> = export_view_lines(&app, 100).iter().map(plain).collect();
    assert!(
        bare.iter().any(|t| t.contains("the working directory")),
        "{bare:?}"
    );
}

#[test]
fn the_selection_lights_its_row() {
    // The palette's rule: the whole selected row lights up in the accent
    // while the other keeps the muted ink. Read straight off the spans, for
    // both highlights.
    for selected in 0..2 {
        let lines = export_view_lines(&open_at(selected), 80);
        for (i, label) in ["Copy to clipboard", "Save to file"].iter().enumerate() {
            let line = lines
                .iter()
                .find(|l| plain(l).contains(label))
                .unwrap_or_else(|| panic!("{label} row"));
            let has_marker = plain(line).contains('❯');
            assert_eq!(
                has_marker,
                i == selected,
                "{label}: the ❯ marks the selection"
            );
            let span = line
                .spans
                .iter()
                .find(|s| s.content.contains(label))
                .expect("the label span");
            if i == selected {
                assert_eq!(span.style.fg, Some(model_selected_color()));
                assert!(span.style.add_modifier.contains(Modifier::BOLD));
            } else {
                assert_ne!(span.style.fg, Some(model_selected_color()));
            }
        }
        // The description under the list is dim on either highlight.
        let description = lines
            .iter()
            .find(|l| plain(l).contains("Copies the transcript") || plain(l).contains("Writes "))
            .expect("the description row");
        for span in description
            .spans
            .iter()
            .filter(|s| !s.content.trim().is_empty())
        {
            assert_eq!(span.style.fg, Some(model_meta_color()));
        }
    }
}

#[test]
fn the_page_never_exceeds_the_width() {
    for width in [20u16, 24, 40, 60, 80, 120] {
        for selected in 0..2 {
            for line in export_view_lines(&open_at(selected), width) {
                assert!(
                    cols(&plain(&line)) <= width as usize,
                    "width {width} selection {selected}: {:?} overflows",
                    plain(&line)
                );
            }
        }
    }
}

#[test]
fn no_page_ever_stacks_two_blank_rows() {
    for width in [30u16, 80] {
        for selected in 0..2 {
            let texts: Vec<String> = texts(&open_at(selected), width)
                .iter()
                .map(|t| t.trim_end().to_string())
                .collect();
            for pair in texts.windows(2) {
                assert!(
                    !(pair[0].is_empty() && pair[1].is_empty()),
                    "width {width} selection {selected} stacked two blank rows: {texts:?}"
                );
            }
            assert_eq!(texts[1], "", "one gap under the top rule: {texts:?}");
            assert_eq!(
                texts[texts.len() - 2],
                "",
                "one gap over the bottom rule: {texts:?}"
            );
        }
    }
}

#[test]
fn height_is_the_line_count_clamped_to_the_terminal() {
    let app = open_app();
    let lines = export_view_lines(&app, 80).len() as u16;
    assert_eq!(export_menu_rows(&app, 80), lines);
    assert_eq!(
        export_picker_height(&app, 80, 200),
        Some(lines),
        "an idle app has no strip above the page"
    );
    assert_eq!(
        export_picker_height(&app, 80, 6),
        Some(6),
        "clamped to a short terminal"
    );
}

#[test]
fn the_page_flows_like_its_family() {
    // A short terminal bottom-anchors the page and flows the skipped top
    // into scrollback (docs/view-flow.md) — the page must be flow-eligible
    // like /donate, or its title would silently clip.
    let app = open_app();
    let page = export_view_lines(&app, 80).len();
    let flow = crate::ui::view_flow(&app, 80, 6, 1000).expect("flows on a 6-row terminal");
    assert_eq!(flow.lines.len(), page - 6, "the skipped top flows");
}

#[test]
fn render_paints_the_lines_and_the_cursor_hides_seated_on_the_marker() {
    let app = open_app();
    let height = export_picker_height(&app, 80, 200).unwrap();
    let mut buf = buffer(80, height);
    render_export_picker(buf.area, &mut buf, &app);
    assert!(row(&buf, 0, 80).starts_with('─'));
    let painted: Vec<String> = (0..height).map(|y| row(&buf, y, 80)).collect();
    for expect in ["Copy to clipboard", "Save to file"] {
        assert!(
            painted.iter().any(|r| r.contains(expect)),
            "{expect} painted: {painted:?}"
        );
    }
    // No text entry anywhere on the page — the /donate rule: no hardware
    // cursor at all, while the *seat* still tracks the highlighted ❯ row.
    assert!(
        !cursor_visible(&app),
        "a read-only page has nothing for a cursor to point at"
    );
    let area = Rect::new(0, 0, 80, height);
    let marker_row = (0..height)
        .find(|&y| row(&buf, y, 80).trim_start().starts_with('❯'))
        .expect("the selected row wears the marker");
    assert_eq!(
        cursor_position(area, &app),
        (2, marker_row),
        "the seat is the highlighted row's marker"
    );
    // …and it follows the highlight.
    let app = open_at(1);
    let mut buf = buffer(80, height);
    render_export_picker(buf.area, &mut buf, &app);
    let marker_row_2 = (0..height)
        .find(|&y| row(&buf, y, 80).trim_start().starts_with('❯'))
        .expect("the marker moved");
    assert_eq!(marker_row_2, marker_row + 1);
    assert_eq!(cursor_position(area, &app), (2, marker_row_2));
}

// ===== the export text =====

/// The export's lines, the final newline dropped.
fn export_lines(app: &App, width: u16) -> Vec<String> {
    let text = export_text(app, width);
    assert!(
        text.ends_with('\n'),
        "the export ends in a newline: {text:?}"
    );
    assert!(
        !text.ends_with("\n\n"),
        "…exactly one, no trailing blank rows: {text:?}"
    );
    text.lines().map(str::to_string).collect()
}

#[test]
fn export_text_is_the_transcript_as_plain_text_in_order() {
    // What Ctrl+O shows, as text: every message and every tool call's FULL
    // output, in the order they happened — the styles dropped, nothing else.
    let app = transcript_fixture();
    let lines = export_lines(&app, 80);
    let pos = |needle: &str| {
        lines
            .iter()
            .position(|t| t.contains(needle))
            .unwrap_or_else(|| panic!("{needle:?} is in the export: {lines:?}"))
    };
    assert!(lines.iter().any(|t| t == "❯ hello"), "{lines:?}");
    assert!(lines.iter().any(|t| t == "● let me check"), "{lines:?}");
    assert!(lines.iter().any(|t| t == "● Read(f)"), "{lines:?}");
    assert!(lines.iter().any(|t| t == "● all done"), "{lines:?}");
    assert!(pos("hello") < pos("let me check"));
    assert!(pos("let me check") < pos("Read(f)"));
    assert!(pos("Read(f)") < pos("L1"));
    assert!(pos("L1") < pos("L2") && pos("L2") < pos("L3"));
    assert!(pos("L3") < pos("all done"));
    // The transcript's own text, exactly: every row Ctrl+O builds at this
    // width — the banner included — each trimmed of the padding a cell
    // carries, the trailing blank rows dropped.
    let mut expected: Vec<String> = transcript_lines(&app, 80)
        .iter()
        .map(|l| plain(l).trim_end().to_string())
        .collect();
    while expected.last().is_some_and(String::is_empty) {
        expected.pop();
    }
    assert_eq!(
        lines, expected,
        "the export is the transcript's rows verbatim"
    );
    // …and the banner leads: its title row sits inside the opening block,
    // ahead of the first blank and the first message. (Which art row the
    // title shares depends on how many metadata rows the banner has — two
    // without session info, over a three-row mascot — so it is "before the
    // conversation", not "row 0".)
    let title = pos("Alter Zero (v");
    let first_blank = lines
        .iter()
        .position(String::is_empty)
        .expect("a blank under the banner");
    assert!(
        title < first_blank && first_blank < pos("❯ hello"),
        "{lines:?}"
    );
}

#[test]
fn export_text_opens_with_the_startup_banner() {
    // The export is the Ctrl+O page as text, top to bottom — and that page
    // opens with the startup banner the terminal did: the mascot beside
    // `Alter Zero (v…)`, the cwd, the `/login   /model   /resume` hint, one
    // blank, then the conversation.
    let mut app = with_session();
    app.record_user_message("hello");
    let lines = export_lines(&app, 80);
    let pos = |needle: &str| {
        lines
            .iter()
            .position(|t| t.contains(needle))
            .unwrap_or_else(|| panic!("{needle:?} is in the export: {lines:?}"))
    };
    assert!(
        !lines[0].trim().is_empty(),
        "the export opens on the banner's first row: {lines:?}"
    );
    assert!(pos("Alter Zero (v") < pos("~/alter-zero"), "{lines:?}");
    assert!(pos("~/alter-zero") < pos("/login   /model   /resume"));
    let hello = pos("❯ hello");
    assert!(pos("/login   /model   /resume") < hello);
    assert_eq!(
        lines[hello - 1],
        "",
        "one blank between the banner and the conversation"
    );
    assert!(
        lines[..hello].iter().all(|t| t == t.trim_end()),
        "the banner rows lose their screen padding too: {lines:?}"
    );
}

#[test]
fn export_text_carries_no_trailing_whitespace_on_any_line() {
    // A user bubble is padded to the full width on screen and a stamp is
    // right-aligned; in a text file that padding is noise a diff would show.
    let app = transcript_fixture();
    for line in export_lines(&app, 80) {
        assert_eq!(line, line.trim_end(), "{line:?}");
    }
}

#[test]
fn export_text_includes_the_live_tail() {
    // Mid-turn the export carries what Ctrl+O shows: the in-progress reply
    // and the running tool.
    let mut app = App::new();
    app.record_user_message("q");
    app.begin_stream();
    app.push_chunk("partial answer");
    app.flush_streaming_segment();
    app.start_tool("Bash", "ls", None);
    let lines = export_lines(&app, 80);
    assert!(
        lines.iter().any(|t| t.contains("partial answer")),
        "{lines:?}"
    );
    assert!(lines.iter().any(|t| t == "● Bash(ls)"), "{lines:?}");
}

#[test]
fn export_text_wraps_at_the_width_it_is_asked_for() {
    // The text is the transcript at the terminal's width — what the user
    // sees is what they get — so a long reply wraps there and no line
    // overflows it.
    let mut app = App::new();
    app.record_user_message("q");
    app.begin_stream();
    app.push_chunk(&"word ".repeat(60));
    app.finish_stream();
    for width in [40u16, 80] {
        let lines = export_lines(&app, width);
        assert!(
            lines.iter().all(|t| cols(t) <= width as usize),
            "width {width}: {lines:?}"
        );
        assert!(
            lines.iter().filter(|t| t.contains("word")).count() > 1,
            "the reply wrapped at {width}: {lines:?}"
        );
    }
}

#[test]
fn export_text_inside_an_agent_view_is_that_agents_transcript() {
    // The export is of the conversation on screen: inside a subagent's
    // session view that is the agent's own transcript, not the lead's (the
    // `/copy` rule, `docs/agent-view-streaming.md`).
    let mut app = with_session();
    app.record_user_message("lead question");
    app.begin_stream();
    app.start_agent_group(
        false,
        &[crate::stream::AgentSpec {
            id: "a1".into(),
            description: "Fetch Warsaw".into(),
            agent_type: "general-purpose".into(),
            prompt: "warsaw?".into(),
            background: false,
            call_id: None,
            arguments: None,
        }],
    );
    app.apply_agent_event(
        "a1",
        &crate::stream::StreamEvent::Chunk("agent answer".to_string()),
    );
    app.open_agent_view("a1");
    let text = export_text(&app, 80);
    assert!(text.contains("warsaw?"), "the agent's prompt: {text:?}");
    assert!(text.contains("agent answer"), "the agent's reply: {text:?}");
    assert!(
        !text.contains("lead question"),
        "never the lead's conversation: {text:?}"
    );
    assert!(
        text.contains("Alter Zero (v") && text.find("Alter Zero (v") < text.find("warsaw?"),
        "the agent's page opens with the banner too: {text:?}"
    );
    let _ = ExportTarget::File;
}
