//! Finding your way in the Ctrl+O transcript: the entries ←/→ step between
//! and the header pinned over a window that opens inside one
//! (`docs/transcript-navigation.md`).

use super::*;
use crate::app::{AgentGroup, AgentGroupEntry, Reasoning, View};
use crate::ui::transcript::{Pin, ellipsized, pinned_rows};

/// A page's rows, plain and right-trimmed.
fn texts(page: &TranscriptPage) -> Vec<String> {
    page.lines
        .iter()
        .map(|line| plain(line).trim_end().to_string())
        .collect()
}

/// The row reading exactly `needle`.
#[track_caller]
fn row_of(texts: &[String], needle: &str) -> usize {
    texts
        .iter()
        .position(|text| text == needle)
        .unwrap_or_else(|| panic!("no row {needle:?} in {texts:?}"))
}

fn entry(start: usize, body: usize, end: usize) -> TranscriptEntry {
    TranscriptEntry { start, body, end }
}

/// `n` numbered lines, `{prefix}0` to `{prefix}{n-1}`.
fn numbered(prefix: &str, n: usize) -> String {
    (0..n)
        .map(|i| format!("{prefix}{i}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// A long reply, so a window anywhere above it can open at its top rather
/// than being clamped to the page's end.
fn long_reply() -> HistoryItem {
    let text = (0..40)
        .map(|i| format!("reply line {i}"))
        .collect::<Vec<_>>()
        .join("\n\n");
    msg(Role::Assistant, &text)
}

/// A finished `Bash({command})` call printing `rows` lines, then a long reply.
fn command_app(command: &str, rows: usize) -> App {
    let mut app = App::new();
    app.start_tool("Bash", command, None);
    app.end_tool(&numbered("line", rows), true);
    app.history.push(long_reply());
    app
}

/// Render the pager over `page` at `width` × `height`, the window at `scroll`.
fn pager(app: &mut App, page: &TranscriptPage, scroll: usize, width: u16, height: u16) -> Buffer {
    app.tool_scroll = scroll;
    let mut buf = buffer(width, height);
    render_tool_view(buf.area, &mut buf, app, page);
    buf
}

fn agent_entry(id: &str, description: &str) -> AgentGroupEntry {
    AgentGroupEntry {
        id: id.to_string(),
        description: description.to_string(),
        agent_type: "general-purpose".to_string(),
        prompt: "look it up".to_string(),
        status: crate::agents::AgentStatus::Done,
        tool_uses: 1,
        tokens: 900,
        secs: 4,
        result: "+19°C".to_string(),
        tool_headers: vec!["Bash(curl wttr.in)".to_string()],
        output: String::new(),
        call_id: None,
        arguments: None,
        position: None,
    }
}

// --- the entries a page records ---

#[test]
fn every_cell_is_an_entry_and_a_tools_header_ends_where_its_output_starts() {
    let app = transcript_fixture();
    let page = transcript_page(&app, 80);
    let texts = texts(&page);
    let user = row_of(&texts, "❯ hello");
    let reply = row_of(&texts, "● let me check");
    let read = row_of(&texts, "● Read(f)");
    let last = texts
        .iter()
        .position(|text| text.ends_with("L3"))
        .expect("the read's output renders");
    let done = row_of(&texts, "● all done");
    assert_eq!(
        page.entries,
        vec![
            entry(user, user, user + 1),
            entry(reply, reply, reply + 1),
            entry(read, read + 1, last + 1),
            entry(done, done, done + 1),
        ]
    );
}

#[test]
fn a_header_that_wraps_ends_on_its_last_row() {
    // `● Bash(` leaves 33 columns of a 40-column row: the command wraps onto
    // a second header row, and the output opens under both.
    let app = command_app("echo aaaaaaaaaa bbbbbbbbbb cccccccccc dddddddddd", 3);
    let page = transcript_page(&app, 40);
    let texts = texts(&page);
    let header = row_of(&texts, "● Bash(echo aaaaaaaaaa bbbbbbbbbb");
    assert_eq!(texts[header + 1].trim(), "cccccccccc dddddddddd)");
    assert_eq!(page.entries[0], entry(header, header + 2, header + 5));
}

#[test]
fn a_turn_receipt_is_no_entry() {
    // `Done for 3s` is a fact about the turn, not a cell to step to: → from
    // a turn's last cell goes on to the next prompt.
    let mut app = App::new();
    app.record_user_message("hi");
    app.begin_stream();
    app.push_chunk("hello");
    app.finish_stream();
    app.end_turn(3);
    app.record_user_message("again");
    let page = transcript_page(&app, 60);
    let texts = texts(&page);
    let receipt = texts
        .iter()
        .position(|text| text.contains(" for 3s"))
        .expect("the receipt renders");
    assert!(
        page.entries.iter().all(|e| e.start != receipt),
        "{:?}",
        page.entries
    );
    let starts: Vec<usize> = page.entries.iter().map(|e| e.start).collect();
    assert_eq!(
        starts,
        vec![
            row_of(&texts, "❯ hi"),
            row_of(&texts, "● hello"),
            row_of(&texts, "❯ again")
        ]
    );
}

#[test]
fn a_shell_commands_output_is_one_entry_under_its_command() {
    // The `! ls` header and its `⎿` output are two history items but one
    // exec cell, and the command is the header the output pins.
    let mut t = tool("ls", "", ToolStatus::Ok, "a\nb\nc");
    t.shell = true;
    let mut app = App::new();
    app.history = vec![msg(Role::Shell, "ls"), HistoryItem::Tool(t)];
    let page = transcript_page(&app, 40);
    let header = row_of(&texts(&page), "! ls");
    assert_eq!(page.entries, vec![entry(header, header + 1, header + 4)]);
}

#[test]
fn a_running_shell_command_joins_its_committed_command() {
    // While it runs, the command is history and its output the live tail;
    // the entry spans both.
    let mut app = App::new();
    app.begin_shell("sleep 5");
    let page = transcript_page(&app, 40);
    let header = row_of(&texts(&page), "! sleep 5");
    assert_eq!(page.entries, vec![entry(header, header + 1, header + 2)]);
}

#[test]
fn a_command_header_with_no_output_after_it_is_an_entry_of_its_own() {
    let mut app = App::new();
    app.history = vec![msg(Role::Shell, "ls"), msg(Role::Assistant, "next")];
    let page = transcript_page(&app, 40);
    let texts = texts(&page);
    let header = row_of(&texts, "! ls");
    let next = row_of(&texts, "● next");
    assert_eq!(
        page.entries,
        vec![
            entry(header, header, header + 1),
            entry(next, next, next + 1)
        ]
    );
}

#[test]
fn each_agent_of_a_group_is_an_entry_under_its_own_header() {
    let mut app = App::new();
    app.history.push(HistoryItem::AgentGroup(AgentGroup {
        background: false,
        agents: vec![
            agent_entry("a1", "Fetch Warsaw"),
            agent_entry("a2", "Fetch Paris"),
        ],
        batch: None,
        timestamp: String::new(),
    }));
    let page = transcript_page(&app, 60);
    let texts = texts(&page);
    let first = texts
        .iter()
        .position(|text| text.starts_with("● Agent(Fetch Warsaw)"))
        .expect("the first agent renders");
    let second = texts
        .iter()
        .position(|text| text.starts_with("● Agent(Fetch Paris)"))
        .expect("the second agent renders");
    // A blank divides the agents, and the group's spacer closes the page.
    assert_eq!(
        page.entries,
        vec![
            entry(first, first + 1, second - 1),
            entry(second, second + 1, texts.len() - 1),
        ]
    );
}

#[test]
fn a_thought_is_an_entry_under_its_label() {
    let mut app = App::new();
    app.history.push(HistoryItem::Reasoning(Reasoning {
        text: "first\nsecond\nthird".to_string(),
        secs: 5,
        tokens: 0,
        timestamp: String::new(),
    }));
    let page = transcript_page(&app, 60);
    let texts = texts(&page);
    let label = texts
        .iter()
        .position(|text| text.starts_with("Thought for 5s"))
        .expect("the thought renders");
    assert_eq!(page.entries, vec![entry(label, label + 1, label + 4)]);
}

#[test]
fn the_live_tail_records_its_cells_too() {
    // A reply streaming and a call running are entries like committed ones,
    // so ←/→ and the pin reach the frontier the pager follows.
    let mut app = App::new();
    app.record_user_message("go");
    app.begin_stream();
    app.push_chunk("checking");
    app.flush_streaming_segment();
    app.start_tool("Bash", "ls", None);
    app.push_tool_output("a\nb\n");
    let page = transcript_page(&app, 60);
    let texts = texts(&page);
    let header = row_of(&texts, "● Bash(ls)");
    let entries = &page.entries;
    assert_eq!(
        entries.last().map(|e| (e.start, e.body)),
        Some((header, header + 1))
    );
    let last = entries.last().expect("the running call is an entry");
    assert_eq!(texts[last.end], "", "the cell ends at its spacer");
    assert!(texts[last.end - 1].ends_with('b'), "{texts:?}");
}

#[test]
fn the_cache_records_the_entries_a_fresh_build_does() {
    // The incremental build appends committed items and rebuilds the live
    // tail; its entries must stay exactly a from-scratch build's at every
    // step — including a `!` command whose output moves from the tail into
    // history.
    let mut app = App::new();
    let mut cache = TranscriptCache::new();
    let check = |cache: &mut TranscriptCache, app: &App, step: &str| {
        assert_eq!(
            cache.entries(app, 50).to_vec(),
            transcript_page(app, 50).entries,
            "{step}"
        );
    };
    check(&mut cache, &app, "empty");
    app.record_user_message("hello");
    check(&mut cache, &app, "a prompt");
    app.begin_shell("ls");
    check(&mut cache, &app, "a running command");
    app.push_tool_output("a\nb\n");
    check(&mut cache, &app, "its output streaming");
    app.end_tool("a\nb\nc", true);
    check(&mut cache, &app, "its output committed");
    app.finish_stream();
    app.end_turn(1);
    app.record_user_message("again");
    app.begin_stream();
    app.start_tool("Read", "f", None);
    check(&mut cache, &app, "a call running");
    app.end_tool("L1\nL2", true);
    app.push_chunk("done");
    check(&mut cache, &app, "a reply streaming");
}

// --- the pinned header ---

#[test]
fn the_pin_is_the_header_of_the_entry_the_window_opens_inside() {
    let entries = [entry(0, 1, 5), entry(6, 7, 30)];
    assert_eq!(
        pinned_rows(&entries, 8, 10),
        Some(Pin {
            header: 6..7,
            cut: false
        })
    );
    assert_eq!(
        pinned_rows(&entries, 2, 10),
        Some(Pin {
            header: 0..1,
            cut: false
        })
    );
    assert_eq!(pinned_rows(&entries, 6, 10), None, "the header is on top");
    assert_eq!(pinned_rows(&entries, 5, 10), None, "a spacer between cells");
    assert_eq!(pinned_rows(&entries, 30, 10), None, "past every entry");
}

#[test]
fn an_entry_with_no_header_or_nothing_under_it_never_pins() {
    // A message has no header; a header alone has no body to pin it over.
    let entries = [entry(0, 0, 20), entry(21, 24, 24)];
    for scroll in [1, 10, 19, 22, 23] {
        assert_eq!(pinned_rows(&entries, scroll, 10), None, "scroll {scroll}");
    }
}

#[test]
fn the_pin_gives_way_on_the_entrys_last_row() {
    // Pinned there it would hide the cell's last row and butt against the
    // next cell's header.
    let entries = [entry(10, 11, 31)];
    assert!(pinned_rows(&entries, 29, 10).is_some());
    assert_eq!(pinned_rows(&entries, 30, 10), None);
}

#[test]
fn a_wrapped_header_pins_whole_and_shrinks_toward_the_entrys_end() {
    let entries = [entry(10, 12, 32)];
    assert_eq!(
        pinned_rows(&entries, 11, 10),
        Some(Pin {
            header: 10..12,
            cut: false
        }),
        "past its first row, the whole header"
    );
    assert_eq!(
        pinned_rows(&entries, 30, 10),
        Some(Pin {
            header: 10..11,
            cut: true
        }),
        "one row left under it: the first row, marked cut"
    );
}

#[test]
fn a_tall_header_pins_its_first_rows_once_the_rest_has_scrolled_off() {
    // A pin over a window still showing the header's own later rows would
    // print the header twice with a gap in it.
    let entries = [entry(10, 16, 40)];
    assert_eq!(pinned_rows(&entries, 13, 10), None);
    assert_eq!(
        pinned_rows(&entries, 14, 10),
        Some(Pin {
            header: 10..12,
            cut: true
        })
    );
}

#[test]
fn a_short_window_pins_less_and_a_tiny_one_nothing() {
    let entries = [entry(10, 12, 40)];
    assert_eq!(
        pinned_rows(&entries, 20, 3),
        Some(Pin {
            header: 10..11,
            cut: true
        }),
        "three rows spare one"
    );
    assert_eq!(pinned_rows(&entries, 20, 2), None, "two spare none");
}

#[test]
fn scrolled_inside_a_tools_output_the_pager_pins_its_header() {
    // The reported want: deep in a long output, the top row still says which
    // command printed it.
    let mut app = command_app("ls -la", 20);
    let page = transcript_page(&app, 40);
    let header = row_of(&texts(&page), "● Bash(ls -la)");
    let buf = pager(&mut app, &page, header + 6, 40, 12);
    assert_eq!(row(&buf, 1, 40).trim_end(), "● Bash(ls -la)");
    assert_eq!(
        row(&buf, 2, 40).trim_end(),
        "     line6",
        "the output carries on under it"
    );
}

#[test]
fn the_pager_pins_nothing_on_the_header_row_or_the_cells_last_row() {
    let mut app = command_app("ls -la", 20);
    let page = transcript_page(&app, 40);
    let header = row_of(&texts(&page), "● Bash(ls -la)");
    let buf = pager(&mut app, &page, header, 40, 12);
    assert_eq!(row(&buf, 2, 40).trim_end(), "  ⎿  line0", "nothing covered");
    let buf = pager(&mut app, &page, header + 20, 40, 12);
    assert_eq!(row(&buf, 1, 40).trim_end(), "     line19");
}

#[test]
fn a_short_pager_body_pins_nothing() {
    let mut app = command_app("ls -la", 20);
    let page = transcript_page(&app, 40);
    let header = row_of(&texts(&page), "● Bash(ls -la)");
    let buf = pager(&mut app, &page, header + 6, 40, 7); // two body rows
    assert_eq!(row(&buf, 1, 40).trim_end(), "     line5");
}

#[test]
fn a_wrapped_header_is_pinned_whole() {
    let mut app = command_app("echo aaaaaaaaaa bbbbbbbbbb cccccccccc dddddddddd", 20);
    let page = transcript_page(&app, 40);
    let header = row_of(&texts(&page), "● Bash(echo aaaaaaaaaa bbbbbbbbbb");
    let buf = pager(&mut app, &page, header + 5, 40, 12);
    assert_eq!(
        row(&buf, 1, 40).trim_end(),
        "● Bash(echo aaaaaaaaaa bbbbbbbbbb"
    );
    assert_eq!(row(&buf, 2, 40).trim(), "cccccccccc dddddddddd)");
    assert_eq!(row(&buf, 3, 40).trim_end(), "     line5");
}

#[test]
fn a_header_taller_than_the_pin_is_cut_with_an_ellipsis() {
    // A four-line script: the pin keeps its first two rows and says the
    // header goes on.
    let mut app = command_app("first\nsecond\nthird\nfourth", 20);
    let page = transcript_page(&app, 40);
    let texts = texts(&page);
    let header = row_of(&texts, "● Bash(first");
    assert_eq!(texts[header + 3].trim(), "fourth)");
    let buf = pager(&mut app, &page, header + 6, 40, 12);
    assert_eq!(row(&buf, 1, 40).trim_end(), "● Bash(first");
    assert_eq!(row(&buf, 2, 40).trim(), "second…");
    assert_eq!(row(&buf, 3, 40).trim_end(), texts[header + 8]);
}

#[test]
fn a_shell_commands_output_pins_the_command() {
    let mut t = tool("ls", "", ToolStatus::Ok, &numbered("file", 20));
    t.shell = true;
    let mut app = App::new();
    app.history = vec![msg(Role::Shell, "ls"), HistoryItem::Tool(t), long_reply()];
    let page = transcript_page(&app, 40);
    let header = row_of(&texts(&page), "! ls");
    let buf = pager(&mut app, &page, header + 5, 40, 12);
    assert_eq!(row(&buf, 1, 40).trim_end(), "! ls");
    assert_eq!(row(&buf, 2, 40).trim_end(), "     file5");
}

#[test]
fn a_reply_scrolled_into_pins_nothing() {
    // Prose has no header — its rows read as what they are.
    let mut app = App::new();
    app.history = vec![long_reply(), long_reply()];
    let page = transcript_page(&app, 40);
    let texts = texts(&page);
    let first = row_of(&texts, "● reply line 0");
    let buf = pager(&mut app, &page, first + 4, 40, 12);
    assert_eq!(row(&buf, 1, 40).trim_end(), texts[first + 4]);
}

#[test]
fn a_running_commands_live_output_pins_its_header_while_followed() {
    // Tail-following a long streaming command, the header has scrolled off
    // the top — the pin keeps naming it.
    let mut app = App::new();
    app.record_user_message("go");
    app.begin_stream();
    app.start_tool("Bash", "ping -c 30 example.com", None);
    for i in 0..30 {
        app.push_tool_output(&format!("64 bytes: icmp_seq={i}\n"));
    }
    let page = transcript_page(&app, 60);
    let buf = pager(&mut app, &page, usize::MAX, 60, 12);
    assert_eq!(
        row(&buf, 1, 60).trim_end(),
        "● Bash(ping -c 30 example.com)"
    );
    let rows: String = (2..8).map(|y| row(&buf, y, 60)).collect();
    assert!(rows.contains("icmp_seq=29"), "the frontier: {rows:?}");
}

#[test]
fn the_cached_pager_pins_exactly_like_a_fresh_page() {
    let mut app = command_app("first\nsecond\nthird\nfourth", 20);
    app.view = View::ToolOutput;
    let mut cache = TranscriptCache::new();
    let total = transcript_page(&app, 40).lines.len();
    for scroll in 0..total {
        app.tool_scroll = scroll;
        let mut cached = buffer(40, 12);
        cache.render(cached.area, &mut cached, &app);
        let page = transcript_page(&app, 40);
        let fresh = pager(&mut app, &page, scroll, 40, 12);
        assert_eq!(cached, fresh, "scroll {scroll}");
    }
}

// --- the cut pin's last row ---

/// A line's text.
fn text(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

#[test]
fn a_cut_row_ends_on_an_ellipsis_right_after_its_text() {
    let line = Line::from(vec![
        Span::raw("● "),
        Span::raw("Bash("),
        Span::raw("first"),
    ]);
    assert_eq!(text(&ellipsized(line, 40)), "● Bash(first…");
}

#[test]
fn a_padded_row_keeps_its_width_and_its_ground() {
    // A `!` command's dark header pads to the terminal's width; cut, the mark
    // takes a column of the padding, so the dark row still reaches the edge.
    let ground = Style::new().bg(Color::Rgb(1, 2, 3));
    let line = Line::from(vec![
        Span::raw("! "),
        Span::raw(format!("ls{}", " ".repeat(10))),
    ])
    .style(ground);
    let cut = ellipsized(line, 40);
    assert_eq!(text(&cut), format!("! ls…{}", " ".repeat(9)));
    assert_eq!(cut.style, ground, "the row keeps its ground");
}

#[test]
fn a_row_too_wide_for_the_mark_is_cut_first_by_display_columns() {
    assert_eq!(text(&ellipsized(Line::from("abcdefghij"), 5)), "abcd…");
    // A wide glyph that would straddle the mark's column goes whole.
    assert_eq!(text(&ellipsized(Line::from("ab你好"), 5)), "ab你…");
    assert_eq!(text(&ellipsized(Line::from("abc你好"), 5)), "abc…");
}

#[test]
fn the_mark_wears_the_texts_ink_but_never_its_link() {
    // A file tool's path is a link to the file; the mark after it is not
    // part of the path.
    let args = Style::new()
        .fg(Color::Rgb(9, 9, 9))
        .add_modifier(Modifier::BOLD);
    let path = crate::links::linked(args, "file:///tmp/a.py");
    let line = Line::from(vec![Span::raw("● Read("), Span::styled("/tmp/a.py", path)]);
    let cut = ellipsized(line, 40);
    let mark = cut.spans.last().expect("the mark");
    assert_eq!(mark.content, "…");
    assert_eq!(mark.style.fg, args.fg);
    assert!(mark.style.add_modifier.contains(Modifier::BOLD));
    assert_eq!(mark.style.underline_color, None, "no link carrier");
    assert!(
        crate::links::style_link(&cut.spans[1].style).is_some(),
        "the path keeps its own link"
    );
}

// --- the chrome ---

#[test]
fn the_first_hint_row_names_the_entry_step_outside_a_preview() {
    let mut app = backtrack_app();
    let page = transcript_page(&app, 80);
    let buf = pager(&mut app, &page, 0, 80, 16);
    let keys = row(&buf, 13, 80);
    assert!(keys.contains("←/→ prev/next entry"), "{keys:?}");
    // In a preview ← and → step the highlight, which the preview's own hint
    // row says; the first row stops promising the entry step.
    app.backtrack.selected = Some(1);
    let buf = pager(&mut app, &page, 0, 80, 16);
    let keys = row(&buf, 13, 80);
    assert!(!keys.contains("←/→"), "{keys:?}");
    assert!(keys.contains("↑/↓ to scroll"), "{keys:?}");
}

#[test]
fn an_agent_session_views_transcript_title_names_the_agent() {
    // Ctrl+D's rule: the lead's transcript and an agent's look alike, so the
    // title says whose it is.
    let mut app = App::new();
    app.begin_stream();
    app.start_agent_group(false, &[spec("a1", "Fetch Warsaw", false)]);
    app.open_agent_view("a1");
    let page = agent_transcript(&app, 80).expect("the agent view has its own transcript");
    let buf = pager(&mut app, &page, 0, 80, 12);
    let title = row(&buf, 0, 80);
    assert!(
        title.starts_with("/ T R A N S C R I P T · general-purpose agent · Fetch Warsaw / "),
        "{title:?}"
    );
}

#[test]
fn an_agents_transcript_records_its_cells_and_pins_their_headers() {
    let mut app = App::new();
    app.begin_stream();
    app.start_agent_group(false, &[spec("a1", "Fetch Warsaw", false)]);
    for event in [
        crate::stream::StreamEvent::ToolStart {
            name: "Bash".into(),
            args: "curl wttr.in".into(),
            detail: None,
            arguments: None,
        },
        crate::stream::StreamEvent::ToolEnd {
            output: numbered("row", 20),
            ok: true,
            truncated: false,
            run: crate::stream::ToolRun::default(),
        },
    ] {
        app.apply_agent_event("a1", &event);
    }
    app.open_agent_view("a1");
    let page = agent_transcript(&app, 60).expect("the agent view has its own transcript");
    let texts = texts(&page);
    let header = row_of(&texts, "● Bash(curl wttr.in)");
    assert!(
        page.entries
            .contains(&entry(header, header + 1, header + 21)),
        "{:?}",
        page.entries
    );
    let buf = pager(&mut app, &page, header + 4, 60, 12);
    assert_eq!(row(&buf, 1, 60).trim_end(), "● Bash(curl wttr.in)");
}
