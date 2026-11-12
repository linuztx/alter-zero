//! The Ctrl+D context-debug overlay (`docs/architecture/context.md`).

use super::*;
use crate::ui::theme::{
    CONTEXT_VIEW_EMPTY, TOOL_VIEW_FOOTER_ROWS, TOOL_VIEW_TITLE_ROWS, context_assistant_color,
    context_chip_fg, context_system_color, context_text_color, context_tool_color,
    context_user_color, tool_dim_color,
};
use crate::ui::wrap::cols;

/// The window's rows as plain text, trailing blanks trimmed.
fn texts(app: &App, width: u16) -> Vec<String> {
    context_lines(app, width)
        .iter()
        .map(|l| plain(l).trim_end().to_string())
        .collect()
}

/// The header's token estimate, worded the way the view words it.
fn estimate(tokens: usize) -> String {
    if tokens == 1 {
        "≈1 token".to_string()
    } else {
        format!("≈{} tokens", format_token_count(tokens))
    }
}

/// The id of the first tool call `app`'s window carries — synthetic
/// (`call_N`) for a record that never had one, the provider's own otherwise.
fn first_call_id(app: &App) -> String {
    crate::context::context_messages(&app.history)
        .iter()
        .flat_map(|message| message.tool_calls.iter())
        .next()
        .expect("a tool call")
        .id
        .clone()
}

/// A body row's text without its `⎿` corner or the indent under it.
fn body_text(row: &str) -> Option<&str> {
    row.strip_prefix("  ⎿  ")
        .or_else(|| row.strip_prefix("     "))
}

#[test]
fn every_entry_opens_on_a_role_chip_over_a_body_hung_on_the_gutter() {
    // The request, entry by entry: a role chip that stands out from the
    // text, the text hung on the tool cell's `⎿` gutter, a blank between
    // entries. The system prompt leads, then the conversation in wire order.
    let app = context_fixture();
    let id = first_call_id(&app);
    let texts = texts(&app, 80);
    assert!(texts[0].starts_with(" SYSTEM  #1"), "{texts:?}");
    assert_eq!(texts[1], "  ⎿  be nice", "{texts:?}");
    assert_eq!(texts[2], "", "{texts:?}");
    assert!(texts[3].starts_with(" USER  #2"), "{texts:?}");
    assert_eq!(texts[4], "  ⎿  hello", "{texts:?}");
    assert!(texts[6].starts_with(" ASSISTANT  #3"), "{texts:?}");
    assert_eq!(texts[7], "  ⎿  let me check", "{texts:?}");
    // The tool call is the native request in its raw wire form — not the old
    // `[tool …]` bracket, and not the TUI's bullet rendering — closed by the
    // id its result answers to.
    assert_eq!(
        texts[8],
        format!(r#"     → read({{"path":"f"}})  {id}"#),
        "{texts:?}"
    );
    assert!(!texts.iter().any(|t| t.contains("[tool")), "{texts:?}");
    // The result rides its own entry, in full, every line aligned under the
    // corner.
    assert!(texts[10].starts_with(" TOOL  #4"), "{texts:?}");
    assert_eq!(
        texts[11..14],
        ["  ⎿  L1", "     L2", "     L3"],
        "{texts:?}"
    );
    assert!(texts[15].starts_with(" ASSISTANT  #5"), "{texts:?}");
    assert_eq!(texts[16], "  ⎿  all done", "{texts:?}");
    assert_eq!(texts.len(), 18, "{texts:?}");
    // Turn summaries are TUI chrome; they never reach the context.
    assert!(!texts.iter().any(|t| t.contains("Done")), "{texts:?}");
}

#[test]
fn the_role_chip_is_lit_in_the_role_colour_under_the_on_accent_ink() {
    // The chip is the row the eye hunts for, so it is filled — the agent
    // label's and the ask countdown's dress — in each role's own hue.
    let lines = context_lines(&context_fixture(), 80);
    for (at, label, colour) in [
        (0, "SYSTEM", context_system_color()),
        (3, "USER", context_user_color()),
        (6, "ASSISTANT", context_assistant_color()),
        (10, "TOOL", context_tool_color()),
    ] {
        let chip = &lines[at].spans[0];
        assert_eq!(chip.content, format!(" {label} "), "row {at}");
        assert_eq!(chip.style.bg, Some(colour), "{label}'s fill");
        assert_eq!(chip.style.fg, Some(context_chip_fg()), "{label}'s ink");
        assert!(
            chip.style.add_modifier.contains(Modifier::BOLD),
            "{label} is bold"
        );
    }
}

#[test]
fn a_header_numbers_its_entry_and_estimates_its_size() {
    // Beside the chip, dim: where the entry sits in the request and roughly
    // what it costs — the question a raw context window is opened to answer
    // ("what is filling it?"). An assistant entry's calls ride the same
    // message, so their names and arguments count toward it, and the TOOL
    // header names the call it answers — the tool, then the call's id —
    // which the bare result never did.
    let app = context_fixture();
    let id = first_call_id(&app);
    let texts = texts(&app, 80);
    let count = crate::tokens::estimate;
    assert_eq!(
        texts[0],
        format!(" SYSTEM  #1 · {}", estimate(count("be nice")))
    );
    assert_eq!(
        texts[3],
        format!(" USER  #2 · {}", estimate(count("hello")))
    );
    let call = count("read") + count(r#"{"path":"f"}"#);
    assert_eq!(
        texts[6],
        format!(
            " ASSISTANT  #3 · {} · 1 tool call",
            estimate(count("let me check") + call)
        )
    );
    assert_eq!(
        texts[10],
        format!(
            " TOOL  #4 · read · {id} · {}",
            estimate(count("L1\nL2\nL3"))
        )
    );
}

#[test]
fn a_tool_call_row_lifts_the_tool_name_and_closes_on_its_id() {
    let app = context_fixture();
    let id = first_call_id(&app);
    let lines = context_lines(&app, 80);
    let call = &lines[8];
    assert_eq!(plain(call), format!(r#"     → read({{"path":"f"}})  {id}"#));
    let name = call
        .spans
        .iter()
        .find(|s| s.content.contains('→'))
        .expect("a name span");
    assert_eq!(name.content, "→ read", "the arrow leads the name");
    assert_eq!(name.style.fg, Some(context_tool_color()));
    assert!(name.style.add_modifier.contains(Modifier::BOLD));
    let args = call
        .spans
        .iter()
        .find(|s| s.content.starts_with('('))
        .expect("an arguments span");
    assert_eq!(args.content, r#"({"path":"f"})"#);
    assert_eq!(
        args.style.fg,
        Some(context_text_color()),
        "the arguments read in the body's own ink"
    );
    // The id the TOOL entry answering it names too, so a parallel batch's
    // results pair with their calls by eye — dim, since it is bookkeeping.
    let tail = call.spans.last().expect("an id span");
    assert_eq!(tail.content, format!("  {id}"));
    assert_eq!(tail.style.fg, Some(tool_dim_color()));
}

#[test]
fn a_call_id_with_no_room_beside_its_call_takes_a_row_of_its_own() {
    let app = context_fixture();
    let id = first_call_id(&app);
    // 25 columns of body: the call's 20 fit, the id beside them does not.
    let texts = texts(&app, 30);
    let call = texts
        .iter()
        .position(|t| t.contains("→ read("))
        .unwrap_or_else(|| panic!("{texts:?}"));
    assert_eq!(texts[call], r#"     → read({"path":"f"})"#, "{texts:?}");
    assert_eq!(texts[call + 1], format!("     {id}"), "{texts:?}");
    assert!(texts.iter().all(|t| cols(t) <= 30), "{texts:?}");
}

#[test]
fn a_long_tool_call_keeps_the_name_on_its_first_row() {
    // Arguments are JSON with few spaces, so the call wraps verbatim: the
    // `→ name(` head never strands alone on a row of its own.
    let mut app = App::new();
    app.record_user_message("go");
    app.begin_stream();
    app.start_tool("Write", "/tmp/a.txt", None);
    app.end_tool("ok", true);
    app.finish_stream();
    let lines = context_lines(&app, 24);
    let first = lines
        .iter()
        .position(|l| plain(l).contains("→ write("))
        .expect("the call's first row");
    assert!(plain(&lines[first]).starts_with("  ⎿  → write("));
    let next = plain(&lines[first + 1]);
    assert!(
        next.starts_with("     ") && !next.contains('→'),
        "the arguments carry on under the corner: {next:?}"
    );
    assert!(lines.iter().all(|l| cols(&plain(l)) <= 24));
}

#[test]
fn a_tool_calls_arguments_wrap_between_words_under_its_name() {
    // Arguments carry prose and code as often as not — a command, a file's
    // contents — so they break between words like the text above them
    // instead of mid-word, hung under the `→ name` lead.
    let arguments = r#"{"command":"echo the quick brown fox jumps over the lazy dog"}"#;
    let mut app = App::new();
    app.record_user_message("go");
    app.begin_stream();
    app.start_tool("Bash", "echo", Some(arguments));
    app.end_tool("ok", true);
    app.finish_stream();
    let id = first_call_id(&app);
    let mut rows: Vec<String> = texts(&app, 40)
        .into_iter()
        .skip_while(|t| !t.contains("→ bash("))
        .take_while(|t| !t.is_empty())
        .collect();
    assert!(rows.len() > 1, "it wrapped: {rows:?}");
    assert!(rows.iter().all(|t| cols(t) <= 40), "{rows:?}");
    // The id closes the call, beside its last row or on a row of its own.
    let last = rows.pop().expect("a last row");
    match last.strip_suffix(&format!("  {id}")) {
        Some(rest) => rows.push(rest.to_string()),
        None => assert_eq!(last, format!("     {id}"), "{rows:?}"),
    }
    let first = rows[0]
        .strip_prefix("  ⎿  → bash")
        .unwrap_or_else(|| panic!("the lead opens the call: {rows:?}"));
    let joined: Vec<&str> = std::iter::once(first)
        .chain(rows[1..].iter().map(|r| r.trim_start()))
        .collect();
    assert_eq!(
        joined.join(" "),
        format!("({arguments})"),
        "every row is whole words: {rows:?}"
    );
}

#[test]
fn a_header_counts_a_users_images_and_charges_for_them() {
    // The estimate is the footer gauge's own rule for one message
    // (`app::message_tokens`): the pictures cost their flat charge each,
    // which a count of the text alone would leave out.
    let mut app = App::new();
    app.record_user_message_with_images(
        "[Image #1] and [Image #2]",
        vec!["/tmp/a.png".into(), "/tmp/b.png".into()],
    );
    let message = &crate::context::context_messages(&app.history)[0];
    let tokens = crate::app::message_tokens(message, &mut crate::tokens::estimate);
    assert!(
        tokens > crate::tokens::estimate(&message.text),
        "the pictures are charged"
    );
    let texts = texts(&app, 80);
    assert_eq!(
        texts[0],
        format!(" USER  #1 · {} · 2 images", estimate(tokens)),
        "{texts:?}"
    );
}

#[test]
fn prose_wraps_at_word_boundaries_under_the_gutter() {
    // Breaking mid-word (`a s` / `cripted`) made every long message hard to
    // read; the text still keeps its own spacing, only the breaks move.
    let text = "the quick brown fox jumps over the lazy dog again and again";
    let mut app = App::new();
    app.record_user_message(text);
    let texts = texts(&app, 30);
    let body: Vec<&str> = texts.iter().filter_map(|t| body_text(t)).collect();
    assert!(body.len() > 1, "it wrapped: {texts:?}");
    assert_eq!(body.join(" "), text, "every row is whole words: {texts:?}");
    assert!(texts.iter().all(|t| cols(t) <= 30), "{texts:?}");
}

#[test]
fn trailing_blank_lines_never_pad_an_entry() {
    // A model's `…\n\n` before its tool call used to paint two blank rows
    // between the text and the call it was introducing.
    let mut app = App::new();
    app.record_user_message("go");
    app.begin_stream();
    app.push_chunk("Let me look.\n\n");
    app.flush_streaming_segment();
    app.start_tool("Read", "f", None);
    app.end_tool("x", true);
    app.finish_stream();
    let texts = texts(&app, 80);
    let at = texts
        .iter()
        .position(|t| t == "  ⎿  Let me look.")
        .unwrap_or_else(|| panic!("{texts:?}"));
    assert!(
        texts[at + 1].starts_with("     → read("),
        "the call follows its text directly: {texts:?}"
    );
}

#[test]
fn an_entry_with_nothing_in_it_says_so() {
    // A result that came back empty still went over the wire; a header over
    // nothing would read as a rendering bug.
    let mut app = App::new();
    app.record_user_message("go");
    app.begin_stream();
    app.start_tool("Read", "f", None);
    app.end_tool("", true);
    app.finish_stream();
    let texts = texts(&app, 80);
    let tool = texts
        .iter()
        .position(|t| t.starts_with(" TOOL "))
        .unwrap_or_else(|| panic!("{texts:?}"));
    assert_eq!(texts[tool + 1], "  ⎿  (empty)", "{texts:?}");
}

#[test]
fn the_system_reminder_markers_recede_from_what_they_wrap() {
    let mut app = context_fixture();
    app.set_user_instructions(Some("guide".to_string()));
    let lines = context_lines(&app, 100);
    let last_style = |needle: &str| {
        lines
            .iter()
            .find(|l| plain(l).trim_end().ends_with(needle))
            .and_then(|l| l.spans.last())
            .unwrap_or_else(|| panic!("a row ending {needle:?}"))
            .style
    };
    for marker in ["<system-reminder>", "</system-reminder>"] {
        assert_eq!(last_style(marker).fg, Some(tool_dim_color()), "{marker}");
    }
    assert_eq!(
        last_style("guide").fg,
        Some(context_text_color()),
        "what the block says keeps the body's ink"
    );
}

#[test]
fn context_lines_show_the_user_instructions_first() {
    // The AGENTS.md section (docs/state/project-doc.md) opens the window's first
    // user entry inside the one `<system-reminder>` — right after the system
    // prompt, in front of the conversation, exactly what the request carries.
    let mut app = context_fixture();
    app.set_user_instructions(Some(
        "Contents of /r/AGENTS.md (project instructions, checked into the codebase):\n\nguide"
            .to_string(),
    ));
    let texts = texts(&app, 100);
    assert!(texts[0].starts_with(" SYSTEM "), "{texts:?}");
    let first_user = texts.iter().position(|t| t.starts_with(" USER ")).unwrap();
    assert_eq!(texts[first_user + 1], "  ⎿  <system-reminder>", "{texts:?}");
    assert_eq!(
        texts[first_user + 2],
        "     Use the following contexts and instructions:",
        "{texts:?}"
    );
    assert_eq!(texts[first_user + 3], "", "{texts:?}");
    assert_eq!(
        texts[first_user + 4],
        "     Contents of /r/AGENTS.md (project instructions, checked into the codebase):",
        "{texts:?}"
    );
    assert_eq!(texts[first_user + 6], "     guide", "{texts:?}");
    assert_eq!(
        texts[first_user + 7],
        "     </system-reminder>",
        "{texts:?}"
    );
    assert!(
        !texts.iter().any(|t| t.contains("<INSTRUCTIONS>")),
        "the codex markers are gone: {texts:?}"
    );
}

#[test]
fn context_lines_show_the_listings_behind_the_instructions_in_one_block() {
    // The skills/agent-type sections (`App::listings`) share the block with
    // the instructions rather than opening a second one.
    let mut app = context_fixture();
    app.set_user_instructions(Some("guide".to_string()));
    app.set_listings(Some(
        "Available agent types for the Agent tool:\n\n- explore: Searches. (Tools: Read)"
            .to_string(),
    ));
    let texts = texts(&app, 100);
    assert_eq!(
        texts
            .iter()
            .filter(|t| body_text(t) == Some("<system-reminder>"))
            .count(),
        1,
        "one block: {texts:?}"
    );
    let guide = texts.iter().position(|t| t == "     guide").unwrap();
    assert_eq!(
        texts[guide + 2],
        "     Available agent types for the Agent tool:",
        "{texts:?}"
    );
    assert_eq!(texts[guide + 5], "     </system-reminder>", "{texts:?}");
}

#[test]
fn context_lines_list_image_attachments_under_their_message() {
    let mut app = App::new();
    app.record_user_message_with_images(
        "[Image #1] what is this?",
        vec![std::path::PathBuf::from("/tmp/shot.png")],
    );
    let texts = texts(&app, 80);
    assert!(
        texts
            .iter()
            .any(|t| t == "  ⎿  [Image #1: /tmp/shot.png] what is this?"),
        "the placeholder names the saved file, exactly as the wire carries \
         it — this view is what was sent (`docs/input/image-paste.md`): {texts:?}"
    );
    assert!(
        texts.iter().any(|t| t == "     image: /tmp/shot.png"),
        "and the attachment still lists beneath, as the picture that rides \
         beside the text: {texts:?}"
    );
}

#[test]
fn context_lines_wrap_a_long_image_path_instead_of_clipping() {
    let mut app = App::new();
    app.record_user_message_with_images(
        "[Image #1]",
        vec![std::path::PathBuf::from(
            "/tmp/a-very-long-temp-directory-name/alter-zero-clipboard-0123456789.png",
        )],
    );
    let texts = texts(&app, 30);
    assert!(
        texts.iter().any(|t| t.starts_with("     image: /tmp")),
        "the label row starts the path: {texts:?}"
    );
    assert!(
        texts
            .iter()
            .any(|t| t.ends_with(".png") && !t.contains("image:")),
        "the path's tail wraps onto a continuation row: {texts:?}"
    );
    assert!(
        texts.iter().all(|t| cols(t) <= 30),
        "no row exceeds the width: {texts:?}"
    );
}

#[test]
fn no_row_is_ever_wider_than_the_terminal() {
    // Down to a single column: the chip and its detail cut with `…`, and the
    // gutter gives its five columns back once the body would have none left.
    let mut app = context_fixture();
    app.record_user_message_with_images(
        "[Image #1] look at this",
        vec![std::path::PathBuf::from("/tmp/a-long/path/shot.png")],
    );
    for width in 1..=60u16 {
        for line in context_lines(&app, width) {
            let text = plain(&line);
            assert!(
                cols(&text) <= usize::from(width),
                "{width} columns: {text:?}"
            );
        }
    }
}

#[test]
fn an_empty_context_shows_the_placeholder() {
    let lines = context_lines(&App::new(), 80);
    assert_eq!(lines.len(), 1);
    assert_eq!(plain(&lines[0]), CONTEXT_VIEW_EMPTY);
}

// ===== the body's entry map and summary =====

#[test]
fn the_body_marks_the_row_every_entry_header_sits_on() {
    // What ←/→ step between and what the pinned header is read from.
    let body = context_body(&context_fixture(), 80);
    assert_eq!(body.headers, vec![0, 3, 6, 10, 15]);
    for &at in &body.headers {
        let chip = body.lines[at].spans[0].content.trim().to_string();
        assert!(
            ["SYSTEM", "USER", "ASSISTANT", "TOOL"].contains(&chip.as_str()),
            "row {at} opens an entry: {chip:?}"
        );
    }
}

#[test]
fn the_summary_adds_up_to_the_footer_gauges_estimate() {
    // The entries' estimates follow the gauge's rule, so the page's total is
    // the very number the footer gauge reads offline — the view and the
    // footer never disagree about one window.
    let mut app = context_fixture();
    app.set_user_instructions(Some("guide".to_string()));
    app.record_user_message_with_images("[Image #1] look", vec!["/tmp/a.png".into()]);
    let gauge = crate::tokens::estimate("be nice")
        + crate::app::estimate_messages_tokens(&crate::context::context_messages_full(
            app.user_instructions.as_deref(),
            app.listings.as_deref(),
            &app.history,
        ));
    let body = context_body(&app, 80);
    assert_eq!(
        body.summary.as_deref(),
        Some(format!("6 messages · {}", estimate(gauge)).as_str())
    );
}

#[test]
fn an_empty_context_has_no_entries_and_no_summary() {
    let body = context_body(&App::new(), 80);
    assert!(body.headers.is_empty());
    assert_eq!(body.summary, None);
}

// ===== rendering =====

#[test]
fn render_context_view_paints_the_pager_chrome_with_its_own_title_and_keys() {
    let app = context_fixture();
    let mut buf = buffer(50, 14);
    render_context_view(buf.area, &mut buf, &app, &context_body(&app, 50));
    let header = row(&buf, 0, 50);
    assert!(
        header.starts_with("/ C O N T E X T / "),
        "the title overlays the slash tiling: {header:?}"
    );
    let sep = row(&buf, 10, 50);
    assert!(sep.starts_with('─'), "{sep:?}");
    assert!(sep.contains('%'), "the scroll percentage rides it: {sep:?}");
    assert!(
        row(&buf, 11, 50).contains("to scroll"),
        "{:?}",
        row(&buf, 11, 50)
    );
    assert!(
        row(&buf, 12, 50).contains("q/esc/ctrl+d to quit"),
        "{:?}",
        row(&buf, 12, 50)
    );
    assert_eq!(row(&buf, 13, 50).trim(), "", "a blank final row");
}

#[test]
fn the_key_hints_teach_the_message_jump() {
    let app = context_fixture();
    let mut buf = buffer(90, 14);
    render_context_view(buf.area, &mut buf, &app, &context_body(&app, 90));
    assert!(
        row(&buf, 11, 90).contains("←/→ prev/next message"),
        "{:?}",
        row(&buf, 11, 90)
    );
}

#[test]
fn the_separator_carries_the_summary_beside_the_scroll_percentage() {
    let app = context_fixture();
    let body = context_body(&app, 60);
    let mut buf = buffer(60, 14);
    render_context_view(buf.area, &mut buf, &app, &body);
    let sep = row(&buf, 10, 60);
    let summary = body.summary.as_deref().expect("a summary");
    assert!(
        sep.starts_with(&format!("─ {summary} ─")),
        "the summary leads the rule: {sep:?}"
    );
    assert!(
        sep.trim_end().ends_with("% ─"),
        "the percentage keeps its place: {sep:?}"
    );
}

#[test]
fn a_rule_too_narrow_for_the_summary_keeps_only_the_percentage() {
    let app = context_fixture();
    let mut buf = buffer(20, 14);
    render_context_view(buf.area, &mut buf, &app, &context_body(&app, 20));
    let sep = row(&buf, 10, 20);
    assert!(!sep.contains("messages"), "{sep:?}");
    assert!(sep.contains('%'), "{sep:?}");
}

#[test]
fn render_context_view_fills_rows_below_the_content_with_tildes() {
    let mut app = App::new();
    app.record_user_message("hi");
    let mut buf = buffer(30, 16);
    render_context_view(buf.area, &mut buf, &app, &context_body(&app, 30));
    assert!(
        row(&buf, 9, 30).starts_with('~'),
        "vi-style filler past the end: {:?}",
        row(&buf, 9, 30)
    );
}

#[test]
fn the_context_views_scroll_clamp_is_the_shared_pager_formula() {
    // The Ctrl+D view shares the transcript pager's chrome, so its scroll
    // clamp is the same `tool_view_max_scroll_for` over its own line count —
    // no context-specific sibling to drift.
    let app = context_fixture();
    let total = context_lines(&app, 40).len();
    let screen_h = 10u16;
    let body = (screen_h - TOOL_VIEW_TITLE_ROWS - TOOL_VIEW_FOOTER_ROWS) as usize;
    assert_eq!(
        tool_view_max_scroll_for(total, screen_h),
        total.saturating_sub(body)
    );
}

#[test]
fn render_context_view_windows_by_the_debug_scroll() {
    let mut app = context_fixture();
    app.debug_scroll = 3; // past the system prompt and its spacer
    let mut buf = buffer(50, 14);
    render_context_view(buf.area, &mut buf, &app, &context_body(&app, 50));
    assert!(
        row(&buf, 1, 50).starts_with(" USER "),
        "the window opens on the next entry: {:?}",
        row(&buf, 1, 50)
    );
}

#[test]
fn scrolled_inside_an_entry_the_view_pins_its_header_on_top() {
    // Deep in a long entry — a system prompt, a big tool result — the top
    // row says whose text it is.
    let mut app = context_fixture();
    app.debug_scroll = 12; // the tool result's second body row
    let mut buf = buffer(50, 9); // four body rows, so row 12 is in reach
    render_context_view(buf.area, &mut buf, &app, &context_body(&app, 50));
    assert!(
        row(&buf, 1, 50).starts_with(" TOOL  #4 · read"),
        "{:?}",
        row(&buf, 1, 50)
    );
    assert_eq!(
        row(&buf, 2, 50).trim_end(),
        "     L3",
        "the body carries on under it"
    );
}

#[test]
fn the_pinned_header_gives_way_on_its_own_row_and_on_the_spacer() {
    // On the header row nothing is pinned (it is already on top); on the
    // entry's trailing blank the next header is one row down, and pinning
    // the old one there would stack two headers.
    for (scroll, top) in [(10, " TOOL  #4"), (14, "")] {
        let mut app = context_fixture();
        app.debug_scroll = scroll;
        let mut buf = buffer(50, 9);
        render_context_view(buf.area, &mut buf, &app, &context_body(&app, 50));
        let first = row(&buf, 1, 50);
        if top.is_empty() {
            assert_eq!(first.trim(), "", "scroll {scroll}: {first:?}");
        } else {
            assert!(first.starts_with(top), "scroll {scroll}: {first:?}");
        }
    }
}

#[test]
fn a_body_too_short_to_spare_a_row_pins_nothing() {
    let mut app = context_fixture();
    app.debug_scroll = 12;
    let mut buf = buffer(50, 7); // two body rows
    render_context_view(buf.area, &mut buf, &app, &context_body(&app, 50));
    assert_eq!(row(&buf, 1, 50).trim_end(), "     L2");
}

#[test]
fn an_agent_session_view_shows_the_subagents_own_system_prompt() {
    // The wire hands a subagent the MAIN prompt with the subagent note
    // appended (`prompts/subagent.md`, via `LlmBackend::subagent_config` —
    // docs/agents/agent-tool.md); the agent session view's Ctrl+D must show that
    // prompt, or the note can never be verified. The body derives from the
    // agent's own transcript, and the AGENTS.md section stays main-only
    // (subagents get none).
    let mut app = context_fixture();
    app.set_user_instructions(Some(
        "Contents of /r/AGENTS.md (project instructions, checked into the codebase):\n\nguide"
            .to_string(),
    ));
    app.set_agent_system_prompt(Some(
        "be nice\n\nYou are running as a subagent.".to_string(),
    ));
    app.begin_stream();
    app.start_agent_group(false, &[spec("a1", "Fetch Warsaw weather", false)]);
    app.open_agent_view("a1");
    let texts = texts(&app, 80);
    assert!(texts[0].starts_with(" SYSTEM "), "{texts:?}");
    assert_eq!(texts[1], "  ⎿  be nice", "{texts:?}");
    assert!(
        texts
            .iter()
            .any(|t| t == "     You are running as a subagent."),
        "the subagent note is visible: {texts:?}"
    );
    // The agent's own transcript derives — its task prompt, not main history.
    assert!(texts.iter().any(|t| t == "  ⎿  task?"), "{texts:?}");
    assert!(
        !texts.iter().any(|t| t == "  ⎿  hello"),
        "main history stays out: {texts:?}"
    );
    assert!(
        !texts.iter().any(|t| t.contains("AGENTS.md")),
        "subagents get no AGENTS.md fragment: {texts:?}"
    );
}

#[test]
fn an_agent_session_views_title_names_the_agent() {
    // Nothing else on the page says whose window this is, and the lead's and
    // an agent's look alike; the title names the agent the way its roster
    // row does — its type, then the task.
    let mut app = context_fixture();
    app.begin_stream();
    app.start_agent_group(false, &[spec("a1", "Fetch Warsaw weather", false)]);
    let mut buf = buffer(80, 12);
    render_context_view(buf.area, &mut buf, &app, &context_body(&app, 80));
    assert!(
        row(&buf, 0, 80).starts_with("/ C O N T E X T / "),
        "the lead's window names no agent: {:?}",
        row(&buf, 0, 80)
    );
    app.open_agent_view("a1");
    let mut buf = buffer(80, 12);
    render_context_view(buf.area, &mut buf, &app, &context_body(&app, 80));
    assert!(
        row(&buf, 0, 80)
            .starts_with("/ C O N T E X T · general-purpose agent · Fetch Warsaw weather "),
        "{:?}",
        row(&buf, 0, 80)
    );
}

#[test]
fn a_wide_title_is_cut_by_columns_not_characters() {
    // An agent's description is whatever the model wrote — CJK included —
    // and a title cut at `width` *characters* is twice the row wide.
    let header = crate::ui::transcript::overlay_header("天气预报天气预报天气预报", 9);
    assert!(
        cols(&plain(&header)) <= 9,
        "the title fits its row: {:?}",
        plain(&header)
    );
}

#[test]
fn the_tiling_never_glues_a_slash_onto_a_title() {
    // Every spaced-caps title ends on the same parity, which hid that the
    // tiling followed the absolute column: an agent's description can end
    // on either, and its last letter then wore a `/`.
    for title in ["C O N T E X T", "C O N T E X T · explore agent · Even"] {
        let header = plain(&crate::ui::transcript::overlay_header(title, 60));
        let rest = header
            .strip_prefix(&format!("/ {title}"))
            .unwrap_or_else(|| panic!("{header:?}"));
        assert!(rest.starts_with(" /"), "{title:?}: {header:?}");
    }
}

#[test]
fn an_agents_context_view_leads_with_the_briefing_it_opened_on() {
    // A launched subagent opens on the skills `<system-reminder>` and reads
    // its task behind it (`docs/agents/subagents.md`), so its Ctrl+D has to lead
    // with the reminder: a view that showed the task alone claimed the agent
    // was sent less than it was, and one that showed the reminder *after* the
    // task would misreport the order the agent read them in.
    let mut app = context_fixture();
    app.set_agent_system_prompt(Some("be nice\n\nsubagent note".to_string()));
    app.set_agent_system_reminder(Some(
        "<system-reminder>\nskills: dataviz\n</system-reminder>".to_string(),
    ));
    app.begin_stream();
    app.start_agent_group(false, &[spec("a1", "Fetch Warsaw weather", false)]);
    app.open_agent_view("a1");
    let texts = texts(&app, 80);
    let user = texts
        .iter()
        .position(|t| t.starts_with(" USER "))
        .unwrap_or_else(|| panic!("a user entry: {texts:?}"));
    // The reminder opens that entry, a blank row divides it from the task.
    assert_eq!(texts[user + 1], "  ⎿  <system-reminder>", "{texts:?}");
    assert_eq!(texts[user + 2], "     skills: dataviz", "{texts:?}");
    assert_eq!(texts[user + 3], "     </system-reminder>", "{texts:?}");
    assert_eq!(texts[user + 4], "", "{texts:?}");
    assert_eq!(texts[user + 5], "     task?", "{texts:?}");
}

#[test]
fn the_main_view_leaves_an_agents_briefing_out_of_its_own_context() {
    // The subagent briefing is stored on `App` like the subagent prompt is;
    // neither may leak into the main window, which carries its own reminder.
    let mut app = context_fixture();
    app.set_agent_system_reminder(Some(
        "<system-reminder>\nagent-only\n</system-reminder>".into(),
    ));
    let texts = texts(&app, 80);
    assert!(
        !texts.iter().any(|t| t.contains("agent-only")),
        "the agent's briefing stays out of the main window: {texts:?}"
    );
}

#[test]
fn the_main_view_keeps_the_main_prompt_beside_a_stored_agent_prompt() {
    // Storing the subagent prompt must not leak it into the main Ctrl+D —
    // the main request never carries the note.
    let mut app = context_fixture();
    app.set_agent_system_prompt(Some("be nice\n\nsubagent note".to_string()));
    let texts = texts(&app, 80);
    assert!(texts[0].starts_with(" SYSTEM "), "{texts:?}");
    assert_eq!(texts[1], "  ⎿  be nice", "{texts:?}");
    assert!(
        !texts.iter().any(|t| t.contains("subagent note")),
        "the note stays out of the main window: {texts:?}"
    );
}

// ===== the ContextCache (docs/architecture/context.md) =====
//
// The Ctrl+D view redraws every animation frame while a turn runs, and the
// derived-context walk + wrap is O(conversation) — rebuilt per frame it froze
// the pager on a big context (a long skill loaded). The cache rebuilds only
// when its signature changes, so a scroll key or a status tick is a hit. The
// TranscriptCache's test shape (`builds` is test-only).

#[test]
fn context_cache_matches_a_fresh_build_and_reuses_it_unchanged() {
    let app = context_fixture();
    let mut cache = ContextCache::new();
    let fresh = context_body(&app, 60);
    let cached = cache.body(&app, 60).clone();
    assert_eq!(cached, fresh, "the cache serves exactly a fresh build");
    assert_eq!(cache.builds, 1, "first access builds");
    assert_eq!(cache.line_count(&app, 60), fresh.lines.len());
    let _ = cache.body(&app, 60);
    assert_eq!(cache.builds, 1, "an unchanged conversation is a cache hit");
}

#[test]
fn context_cache_rebuilds_on_new_history_a_width_change_and_a_rewind() {
    let mut app = context_fixture();
    let mut cache = ContextCache::new();
    let _ = cache.body(&app, 60);
    assert_eq!(cache.builds, 1);
    app.record_user_message("fresh turn");
    let grown: Vec<String> = cache.body(&app, 60).lines.iter().map(plain).collect();
    assert_eq!(cache.builds, 2, "a history append rebuilds");
    assert!(
        grown.iter().any(|t| t.trim_end() == "  ⎿  fresh turn"),
        "{grown:?}"
    );
    let _ = cache.body(&app, 50);
    assert_eq!(cache.builds, 3, "a width change rebuilds");
    app.load_session(Vec::new()); // a rewind: same-or-shorter history, new generation
    let cleared: Vec<String> = cache.body(&app, 50).lines.iter().map(plain).collect();
    assert_eq!(cache.builds, 4, "a generation bump rebuilds");
    assert!(
        !cleared.iter().any(|t| t.contains("fresh turn")),
        "{cleared:?}"
    );
}

#[test]
fn context_cache_counts_only_what_is_new() {
    // The header estimates put every entry through the estimator; the cache
    // keeps what it counted, so a rebuild for one new message counts that
    // message rather than the whole conversation again.
    let mut app = context_fixture();
    let mut cache = ContextCache::new();
    let _ = cache.body(&app, 60);
    let first = cache.counted;
    assert!(first > 0, "the first build counts every entry");
    app.record_user_message("fresh turn");
    let _ = cache.body(&app, 60);
    assert_eq!(
        cache.counted - first,
        1,
        "the rebuild tokenized the one new entry"
    );
}

#[test]
fn context_cache_release_frees_the_window_and_the_next_open_rebuilds() {
    // Ctrl+D's close calls `release` so a large rendered window (a second
    // full copy of the conversation as owned rows) doesn't sit resident for
    // the rest of the session after one peek at the debug view. The next
    // open simply rebuilds — same rows, one build.
    let app = context_fixture();
    let mut cache = ContextCache::new();
    let fresh = cache.body(&app, 60).clone();
    assert_eq!(cache.builds, 1);
    cache.release();
    assert_eq!(
        cache.retained_rows(),
        0,
        "release drops the rendered window"
    );
    let reopened = cache.body(&app, 60).clone();
    assert_eq!(reopened, fresh, "a reopened view serves the same rows");
    assert_eq!(cache.builds, 2, "the reopen is one rebuild");
}

#[test]
fn context_cache_rebuilds_when_the_leading_fragments_change() {
    // The AGENTS.md section reloads at turn starts and the `/settings`
    // Project-docs knob drops it with no history append — the signature must
    // catch the direct edit.
    let mut app = context_fixture();
    let mut cache = ContextCache::new();
    let _ = cache.body(&app, 60);
    app.set_user_instructions(Some(
        "Contents of /r/AGENTS.md (project instructions, checked into the codebase):\n\nguide"
            .to_string(),
    ));
    let cached: Vec<String> = cache.body(&app, 60).lines.iter().map(plain).collect();
    assert_eq!(cache.builds, 2, "an instructions change rebuilds");
    assert!(cached.iter().any(|t| t.contains("AGENTS.md")), "{cached:?}");
}

#[test]
fn context_cache_tracks_the_viewed_agents_window() {
    // An open agent session view swaps the whole window to the agent's own
    // derivation (its prompt, its transcript) and back — the cache must key
    // on which window it cached.
    let mut app = context_fixture();
    app.set_agent_system_prompt(Some(
        "be nice\n\nYou are running as a subagent.".to_string(),
    ));
    app.begin_stream();
    app.start_agent_group(false, &[spec("a1", "Fetch Warsaw weather", false)]);
    let mut cache = ContextCache::new();
    let rows = |cache: &mut ContextCache, app: &App| -> Vec<String> {
        cache
            .body(app, 80)
            .lines
            .iter()
            .map(|l| plain(l).trim_end().to_string())
            .collect()
    };
    let main_window = rows(&mut cache, &app);
    assert!(main_window.iter().any(|t| t == "  ⎿  hello"));
    app.open_agent_view("a1");
    let agent_window = rows(&mut cache, &app);
    assert_eq!(cache.builds, 2, "entering the agent view rebuilds");
    assert!(
        agent_window.iter().any(|t| t == "  ⎿  task?"),
        "{agent_window:?}"
    );
    app.close_agent_view();
    let back = rows(&mut cache, &app);
    assert_eq!(cache.builds, 3, "leaving it rebuilds the main window");
    assert!(back.iter().any(|t| t == "  ⎿  hello"), "{back:?}");
}

// ===== Ctrl+D context-debug view (docs/architecture/context.md) =====

/// A conversation with a system prompt, a user turn, a raw tool record,
/// and a summary — everything the context window derives from.
fn context_fixture() -> App {
    let mut app = transcript_fixture();
    app.set_system_prompt(Some("be nice".to_string()));
    app.end_turn(2);
    app
}

#[test]
fn the_context_cache_rebuilds_when_the_theme_changes() {
    // The role chips are coloured, so the cached window is pinned on the
    // active theme: a `/theme` switch rebuilds it in the new palette
    // (`docs/ui/theme.md`).
    use crate::app::Theme;
    use crate::ui::palette::{palette_of, with_theme};
    let mut app = App::new();
    app.record_user_message("hello");
    let mut cache = ContextCache::new();
    let wears = |lines: &[Line<'static>], color: Color| {
        lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .any(|s| s.style.bg == Some(color))
    };
    let mocha = palette_of(Theme::Mocha).link;
    let dracula = palette_of(Theme::Dracula).link;
    assert!(
        wears(&cache.body(&app, 60).lines, mocha),
        "the user chip in Mocha's link blue"
    );
    assert_eq!(cache.builds, 1);
    let switched = with_theme(Theme::Dracula, || {
        let lines = cache.body(&app, 60).lines.clone();
        (wears(&lines, dracula), wears(&lines, mocha))
    });
    assert_eq!(
        switched,
        (true, false),
        "rebuilt in Dracula's palette, nothing left of Mocha's"
    );
    assert_eq!(cache.builds, 2, "the switch cost one rebuild");
}

/// The whole context `app` sends, estimated the way its gauge estimates it.
fn whole_estimate(app: &App) -> u64 {
    let prompt = crate::tokens::estimate(app.system_prompt.as_deref().unwrap_or_default());
    let messages = crate::app::estimate_messages_tokens(&crate::context::context_messages_full(
        app.user_instructions.as_deref(),
        app.listings.as_deref(),
        &app.history,
    ));
    (prompt + messages) as u64
}

/// A usage frame counting `input` tokens.
fn frame(input: u64) -> crate::stream::TokenUsage {
    crate::stream::TokenUsage {
        input,
        ..crate::stream::TokenUsage::default()
    }
}

#[test]
fn the_estimates_count_in_what_the_providers_reports_taught() {
    // Two usage reports that counted the conversation's growth at three
    // halves of the estimate: the view's figures follow, so they and the
    // footer's gauge speak the provider's tokens (docs/architecture/token-estimate.md).
    let mut app = context_fixture();
    app.begin_stream();
    let first = whole_estimate(&app);
    app.apply_usage(&frame(first + 1_000));
    app.record_user_message(&"word ".repeat(600));
    let second = whole_estimate(&app);
    app.apply_usage(&frame(first + 1_000 + (second - first) * 3 / 2));
    let calibration = app.context_calibration();
    assert!(calibration.ratio_permille() > 1_400, "{calibration:?}");
    let hello = calibration.correct(crate::tokens::estimate("hello") as u64);
    let texts = texts(&app, 80);
    assert_eq!(
        texts[3],
        format!(" USER  #2 · {}", estimate(hello as usize))
    );
    // The cached view rebuilds on a lesson alone.
    let mut cache = ContextCache::new();
    let mut fresh = context_fixture();
    let before = cache.body(&fresh, 80).summary.clone();
    fresh.begin_stream();
    let first = whole_estimate(&fresh);
    fresh.apply_usage(&frame(first + 1_000));
    assert_eq!(
        cache.body(&fresh, 80).summary,
        before,
        "nothing to learn from yet"
    );
    fresh.record_user_message(&"word ".repeat(600));
    let after_message = cache.body(&fresh, 80).summary.clone();
    let second = whole_estimate(&fresh);
    fresh.apply_usage(&frame(first + 1_000 + (second - first) * 3 / 2));
    assert_ne!(cache.body(&fresh, 80).summary, after_message);
}
