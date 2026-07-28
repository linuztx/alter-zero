//! The Ctrl+O transcript overlay and its cache
//! (`docs/tool-view-performance.md`, `docs/backtrack.md`).

use super::*;
use crate::ui::theme::{
    QUEUED_INDENT, TIMESTAMP_COLOR, TOOL_VIEW_FOOTER_ROWS, TOOL_VIEW_TITLE, TOOL_VIEW_TITLE_ROWS,
};
use crate::ui::transcript::scroll_into_view;
use crate::ui::wrap::cols;

#[test]
fn transcript_lines_interleaves_messages_and_full_tool_output_in_order() {
    let app = transcript_fixture();
    let texts: Vec<String> = transcript_lines(&app, 80)
        .iter()
        .map(|l| plain(l).trim_end().to_string())
        .collect();
    // User + both assistant segments are present (the new behaviour).
    assert!(texts.iter().any(|t| t == "❯ hello"), "{texts:?}");
    assert!(texts.iter().any(|t| t == "● let me check"), "{texts:?}");
    assert!(texts.iter().any(|t| t == "● all done"), "{texts:?}");
    // The tool's FULL output is present — every line, not collapsed.
    assert!(texts.iter().any(|t| t == "● Read(f)"));
    for needle in ["L1", "L2", "L3"] {
        assert!(texts.iter().any(|t| t.contains(needle)), "missing {needle}");
    }
    // …in the exact order they happened: user, text, tool+output, text.
    let pos = |needle: &str| texts.iter().position(|t| t.contains(needle)).unwrap();
    assert!(pos("hello") < pos("let me check"));
    assert!(pos("let me check") < pos("Read(f)"));
    assert!(pos("L3") < pos("all done"));
}

#[test]
fn transcript_lines_shows_the_in_progress_reply_and_running_tool() {
    // The live tail (not yet in history) is included so the view updates live.
    let mut app = App::new();
    app.record_user_message("q");
    app.begin_stream();
    app.push_chunk("partial answer");
    let texts: Vec<String> = transcript_lines(&app, 80)
        .iter()
        .map(|l| plain(l).trim_end().to_string())
        .collect();
    assert!(texts.iter().any(|t| t.contains("partial answer")));

    // Now a tool starts running (text flushed); it shows as running.
    app.flush_streaming_segment();
    app.start_tool("Bash", "ls");
    let texts: Vec<String> = transcript_lines(&app, 80)
        .iter()
        .map(|l| plain(l).trim_end().to_string())
        .collect();
    assert!(texts.iter().any(|t| t == "● Bash(ls)"));
    assert!(texts.iter().any(|t| t.to_lowercase().contains("running")));
}

#[test]
fn transcript_lines_list_the_queued_backlog_after_the_live_tail() {
    // The overlay shows the full live picture: entries still waiting in
    // the queue render after the live tail, styled exactly like the inline
    // strip's queued rows (two-space inset, ❯ user / red ! shell, a blank
    // dividing entries), so Ctrl+O never hides a queued message
    // (docs/queue.md).
    let mut app = App::new();
    app.record_user_message("q");
    app.begin_stream();
    app.push_chunk("partial");
    app.queued.push_back(batch(&["world"]));
    app.queued.push_back(QueuedTurn::Shell("ls".into()));
    let texts: Vec<String> = transcript_lines(&app, 80)
        .iter()
        .map(|l| plain(l).trim_end().to_string())
        .collect();
    let pos = |needle: &str| {
        texts
            .iter()
            .position(|t| t.contains(needle))
            .unwrap_or_else(|| panic!("missing {needle}: {texts:?}"))
    };
    assert!(
        pos("partial") < pos("❯ world"),
        "queued after the live tail"
    );
    assert!(pos("❯ world") < pos("! ls"), "entries in queue order");
    assert_eq!(
        texts[pos("❯ world")],
        format!("{QUEUED_INDENT}❯ world"),
        "the inline strip's two-space inset user style"
    );
    assert_eq!(
        texts[pos("❯ world") + 1].trim(),
        "",
        "a blank divides the entries"
    );
}

#[test]
fn transcript_lines_when_empty_is_a_placeholder() {
    // Even an empty transcript opens with the header banner (the overlay
    // mirrors the inline conversation — docs/header.md); the placeholder
    // sits under it, not lost.
    let app = App::new();
    let lines = transcript_lines(&app, 80);
    let chrome = header_lines(&app, 80).len() + 1; // banner + spacer
    assert!(
        plain(&lines[chrome]).to_lowercase().contains("nothing"),
        "{:?}",
        plain(&lines[chrome])
    );
}

#[test]
fn transcript_cache_reuses_the_build_while_scrolling_but_refreshes_on_change() {
    // The Ctrl+O scroll-perf fix: repeated draws with the same content (a
    // scroll only moves the viewport) must reuse the cached build, while any
    // real change — new history, a width change — rebuilds. Correctness is
    // that the cache always equals a fresh `transcript_lines`.
    let mut app = transcript_fixture();
    let mut cache = TranscriptCache::new();

    assert_eq!(cache.lines(&app, 80), transcript_lines(&app, 80).as_slice());
    assert_eq!(cache.builds, 1, "first access builds");

    // Repeated accesses with unchanged state (scrolling) are cache hits.
    cache.line_count(&app, 80);
    cache.lines(&app, 80);
    cache.selection(&app, 80);
    assert_eq!(cache.builds, 1, "scrolling is a cache hit, not a rebuild");

    // A new message changes the content → one rebuild, still correct.
    app.record_user_message("a brand new question");
    assert_eq!(cache.lines(&app, 80), transcript_lines(&app, 80).as_slice());
    assert_eq!(cache.builds, 2, "a history change rebuilds");

    // A width change (resize) also refreshes.
    assert_eq!(cache.lines(&app, 40), transcript_lines(&app, 40).as_slice());
    assert_eq!(cache.builds, 3, "a width change rebuilds");

    // clear() drops the cache so the next access rebuilds.
    cache.clear();
    cache.lines(&app, 40);
    assert_eq!(cache.builds, 1, "clear resets the build state");
}

#[test]
fn transcript_cache_rebuilds_as_a_running_bash_streams_output() {
    // The Ctrl+O overlay must show a running `bash` tool's output LIVE, not a
    // frozen snapshot: as the tool streams (`App::push_tool_output`), the
    // cache signature has to notice the front call's output growing so the
    // overlay rebuilds — and, since it tail-follows, scrolls the new output
    // into view (docs/tool-streaming.md). Without the output length in the
    // signature the overlay would stay static: a single running call's queue
    // length and status don't change while it streams.
    let mut app = App::new();
    app.begin_stream();
    app.start_tool("Bash", "make");
    let mut cache = TranscriptCache::new();
    let _ = cache.lines(&app, 80); // first build — the running cell, no output yet
    let builds = cache.builds;
    app.push_tool_output("compiling module_a\n");
    let lines = cache.lines(&app, 80).to_vec();
    assert!(
        cache.builds > builds,
        "the cache rebuilds when the running tool streams output"
    );
    assert!(
        lines
            .iter()
            .any(|l| plain(l).contains("compiling module_a")),
        "the overlay shows the newly streamed output: {:?}",
        lines.iter().map(plain).collect::<Vec<_>>()
    );
    // Each further chunk keeps it live and matches a fresh build.
    let builds = cache.builds;
    app.push_tool_output("compiling module_b\n");
    assert_eq!(cache.lines(&app, 80), transcript_lines(&app, 80).as_slice());
    assert!(
        cache.builds > builds,
        "each streamed chunk refreshes the overlay"
    );
}

#[test]
fn transcript_cache_appends_new_items_without_rerendering_frozen_ones() {
    // The slow-Ctrl+O fix: the transcript build is O(history) with real
    // grammar highlighting (~hundreds of ms on a resumed session), so the
    // cache must render each committed item ONCE and only append — a new
    // item, a streamed chunk, or a reopen must never re-render the frozen
    // prefix. Correctness stays "equals a fresh full build".
    let mut app = transcript_fixture();
    let mut cache = TranscriptCache::new();
    let _ = cache.lines(&app, 80);
    let rendered = cache.item_renders;
    assert_eq!(
        rendered,
        app.history.len(),
        "the first build renders every item exactly once"
    );

    app.record_user_message("appended later");
    assert_eq!(cache.lines(&app, 80), transcript_lines(&app, 80).as_slice());
    assert_eq!(
        cache.item_renders,
        rendered + 1,
        "a committed item renders once; the frozen prefix is reused"
    );

    let _ = cache.lines(&app, 80);
    assert_eq!(cache.item_renders, rendered + 1, "a scroll renders nothing");
}

#[test]
fn transcript_cache_streams_a_reply_without_rerendering_history() {
    // While a reply streams under the open overlay the signature changes
    // every chunk — the live tail must rebuild (the overlay follows the
    // stream) without paying the frozen prefix again.
    let mut app = transcript_fixture();
    let mut cache = TranscriptCache::new();
    let _ = cache.lines(&app, 80);
    let rendered = cache.item_renders;
    app.begin_stream();
    for chunk in ["stream", "ing 1", " and 2"] {
        app.push_chunk(chunk);
        assert_eq!(cache.lines(&app, 80), transcript_lines(&app, 80).as_slice());
    }
    assert_eq!(
        cache.item_renders, rendered,
        "streamed chunks re-render only the live tail, never the history"
    );
}

#[test]
fn transcript_cache_backtrack_selection_restyles_without_rerendering() {
    // The Esc-Esc preview reverses the highlighted user message's rows —
    // rows in the *frozen* prefix. Stepping the selection must restyle in
    // place (and un-restyle exactly, byte-for-byte) without re-rendering.
    let mut app = transcript_fixture();
    app.record_user_message("second question");
    let mut cache = TranscriptCache::new();
    let _ = cache.lines(&app, 80);
    let rendered = cache.item_renders;

    for selected in [Some(1), Some(0), None, Some(1)] {
        app.backtrack.selected = selected;
        assert_eq!(
            cache.lines(&app, 80),
            transcript_lines(&app, 80).as_slice(),
            "selection {selected:?} matches a fresh build"
        );
        assert_eq!(
            cache.selection(&app, 80),
            transcript_selection(&app, 80),
            "selection range {selected:?} matches a fresh build"
        );
    }
    assert_eq!(
        cache.item_renders, rendered,
        "restyling the selection never re-renders items"
    );
}

#[test]
fn transcript_cache_rebuilds_when_history_is_replaced_at_the_same_length() {
    // A pop + re-push (the interrupt-undo, then a new submission) can land
    // history back on the SAME length with different content — the length
    // signature alone would serve the stale build; the generation catches it.
    let mut app = App::new();
    app.record_user_message("first try");
    let mut cache = TranscriptCache::new();
    let _ = cache.lines(&app, 80);

    app.begin_stream();
    assert_eq!(
        app.interrupt_turn(),
        Some(crate::app::InterruptedTurn::Undone),
        "the no-output interrupt undoes the submission"
    );
    app.record_user_message("second try");
    let lines = cache.lines(&app, 80).to_vec();
    assert_eq!(lines, transcript_lines(&app, 80), "no stale frozen rows");
    let all: String = lines.iter().map(plain).collect::<Vec<_>>().join("\n");
    assert!(all.contains("second try"), "{all:?}");
    assert!(!all.contains("first try"), "{all:?}");
}

#[test]
fn transcript_cache_warm_prebuilds_so_the_open_renders_nothing() {
    // The loop warms the cache at the boundary (after a /resume load, after
    // each commit) so pressing Ctrl+O finds every item already rendered —
    // the open then only assembles the live tail.
    let mut app = transcript_fixture();
    let mut cache = TranscriptCache::new();
    cache.warm(&app, 80);
    let rendered = cache.item_renders;
    assert_eq!(rendered, app.history.len(), "warm renders every item");
    assert_eq!(cache.lines(&app, 80), transcript_lines(&app, 80).as_slice());
    assert_eq!(cache.item_renders, rendered, "the open renders no items");

    // A /resume swaps the whole history: the next warm rebuilds it.
    let loaded: Vec<HistoryItem> = transcript_fixture().history.clone();
    app.load_session(loaded);
    cache.warm(&app, 80);
    assert_eq!(cache.lines(&app, 80), transcript_lines(&app, 80).as_slice());

    // A width-only mismatch (a resize while the overlay is closed) defers:
    // warm must NOT burn a full rebuild per resize event…
    let rendered = cache.item_renders;
    cache.warm(&app, 40);
    assert_eq!(cache.item_renders, rendered, "warm skips a width change");
    // …the next real access (the overlay opening at the new width) pays it.
    assert_eq!(cache.lines(&app, 40), transcript_lines(&app, 40).as_slice());
}

#[test]
fn render_tool_view_shows_the_title_messages_and_full_output() {
    let app = transcript_fixture();
    let mut buf = buffer(40, 24);
    let tv_lines = transcript_lines(&app, buf.area.width);
    render_tool_view(buf.area, &mut buf, &app, &tv_lines);
    let all: String = (0..24)
        .map(|y| row(&buf, y, 40))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(all.contains(TOOL_VIEW_TITLE), "title present: {all:?}");
    assert!(
        all.contains(env!("CARGO_PKG_VERSION")),
        "the header banner opens the transcript: {all:?}"
    );
    assert!(all.contains("hello"), "user message shown: {all:?}");
    assert!(
        all.contains("let me check") && all.contains("all done"),
        "{all:?}"
    );
    assert!(all.contains("Read(f)"), "{all:?}");
    assert!(
        all.contains("L1") && all.contains("L2"),
        "full output: {all:?}"
    );
}

#[test]
fn render_tool_view_paints_the_codex_pager_chrome() {
    // The overlay is codex's Ctrl+T transcript pager: a slash-tiled dim
    // title row, the scrolling body, a `─` separator carrying the scroll
    // percentage right-aligned one dash in from the edge, two dim key-hint
    // rows, and a final blank row.
    let app = transcript_fixture();
    let mut buf = buffer(40, 24);
    let tv_lines = transcript_lines(&app, buf.area.width);
    render_tool_view(buf.area, &mut buf, &app, &tv_lines);
    let header = row(&buf, 0, 40);
    assert!(
        header.starts_with("/ T R A N S C R I P T / / "),
        "the title overlays the slash tiling: {header:?}"
    );
    // body rows 1..=19, then the separator at 24 - 4.
    let sep = row(&buf, 20, 40);
    assert!(sep.starts_with('─'), "{sep:?}");
    assert!(
        sep.contains(" 100% "),
        "everything fits → pinned at 100%: {sep:?}"
    );
    assert!(sep.ends_with('─'), "one dash right of the percent: {sep:?}");
    let hints = row(&buf, 21, 40);
    assert!(
        hints.contains("to scroll") && hints.contains("pgup/pgdn"),
        "{hints:?}"
    );
    assert!(
        row(&buf, 22, 40).contains("q/esc/ctrl+o to quit"),
        "{:?}",
        row(&buf, 22, 40)
    );
    assert_eq!(row(&buf, 23, 40).trim(), "", "a blank final row");
}

#[test]
fn render_tool_view_fills_rows_below_the_content_with_tildes() {
    // Body rows past the transcript's end read `~` (codex's pager, vi-style).
    let mut app = App::new();
    app.record_user_message("hi");
    let mut buf = buffer(40, 24);
    let tv_lines = transcript_lines(&app, buf.area.width);
    render_tool_view(buf.area, &mut buf, &app, &tv_lines);
    // Content is the banner chrome + two lines (message + spacer) in a
    // 19-row body: the rows after it are filler.
    let chrome = header_lines(&app, 40).len() + 1;
    let msg_row = 1 + chrome as u16;
    assert!(
        row(&buf, msg_row, 40).contains("❯ hi"),
        "{:?}",
        row(&buf, msg_row, 40)
    );
    for y in (msg_row + 2)..=19 {
        assert_eq!(row(&buf, y, 40).trim_end(), "~", "row {y} is filler");
    }
}

#[test]
fn render_tool_view_separator_tracks_the_scroll_position() {
    // 0% at the top, 100% at the bottom — codex's pager percentage.
    let mut app = App::new();
    app.start_tool("Read", "f");
    let output = (0..20)
        .map(|i| format!("line{i}"))
        .collect::<Vec<_>>()
        .join("\n");
    app.end_tool(&output, true);
    let mut buf = buffer(40, 12);
    let tv_lines = transcript_lines(&app, buf.area.width);
    render_tool_view(buf.area, &mut buf, &app, &tv_lines);
    assert!(row(&buf, 8, 40).contains(" 0% "), "{:?}", row(&buf, 8, 40));

    app.tool_scroll = usize::MAX; // pinned to the bottom (clamped)
    let mut buf = buffer(40, 12);
    let tv_lines = transcript_lines(&app, buf.area.width);
    render_tool_view(buf.area, &mut buf, &app, &tv_lines);
    assert!(
        row(&buf, 8, 40).contains(" 100% "),
        "{:?}",
        row(&buf, 8, 40)
    );
}

#[test]
fn render_tool_view_scrolls_past_the_top() {
    let mut app = App::new();
    app.start_tool("Read", "f");
    let output = (0..20)
        .map(|i| format!("line{i}"))
        .collect::<Vec<_>>()
        .join("\n");
    app.end_tool(&output, true);
    app.view = crate::app::View::ToolOutput;
    app.tool_scroll = 9;
    let mut buf = buffer(40, 8);
    let tv_lines = transcript_lines(&app, buf.area.width);
    render_tool_view(buf.area, &mut buf, &app, &tv_lines);
    let all: String = (0..8)
        .map(|y| row(&buf, y, 40))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !all.contains("line0"),
        "scrolled past the first line: {all:?}"
    );
    assert!(all.contains("line"), "still shows some output: {all:?}");
}

#[test]
fn tool_view_max_scroll_is_total_lines_minus_the_body() {
    let app = transcript_fixture();
    let total = transcript_lines(&app, 40).len();
    let screen_h = 10u16;
    let body = (screen_h - TOOL_VIEW_TITLE_ROWS - TOOL_VIEW_FOOTER_ROWS) as usize;
    assert_eq!(
        tool_view_max_scroll(&app, 40, screen_h),
        total.saturating_sub(body)
    );
}

#[test]
fn transcript_puts_the_user_stamp_on_its_own_line_bottom_right() {
    let mut app = App::new();
    app.history = stamped_history();
    let width = 60u16;
    let texts: Vec<String> = transcript_lines(&app, width).iter().map(plain).collect();

    let header = texts
        .iter()
        .position(|t| t.trim_end() == "❯ hi")
        .expect("the user header carries no stamp");
    assert_eq!(
        texts[header + 1].trim_end(),
        "",
        "a blank row separates the message from its stamp"
    );
    let stamp = &texts[header + 2];
    assert_eq!(stamp.trim(), STAMP, "the stamp sits alone on its own line");
    assert_eq!(
        cols(stamp),
        width as usize,
        "the stamp is flush to the right edge: {stamp:?}"
    );
}

#[test]
fn transcript_styles_the_user_stamp_dim() {
    let mut app = App::new();
    app.history = stamped_history();
    let stamp_line = transcript_lines(&app, 60)
        .into_iter()
        .find(|l| plain(l).trim() == STAMP)
        .expect("the user stamp line");
    let stamp_span = stamp_line
        .spans
        .iter()
        .find(|s| s.content.contains(STAMP))
        .expect("the stamp span");
    assert_eq!(stamp_span.style.fg, Some(TIMESTAMP_COLOR));
}

#[test]
fn transcript_omits_the_stamp_line_for_an_empty_user_stamp() {
    // No clock injected (the unit-test default) → no stamp line, no extra
    // blank under the user message.
    let mut app = App::new();
    app.history = vec![msg(Role::User, "hi"), msg(Role::Assistant, "hello")];
    assert_eq!(transcript_body(&app, 60), vec!["❯ hi", "", "● hello", ""]);
}

#[test]
fn the_tool_view_renders_a_shell_cell_headerless_with_full_output() {
    // In the Ctrl+O transcript the shell cell shows its `! ls` dark header
    // (the Role::Shell message) and the FULL output under `⎿` — no `● ls`
    // bullet, and uncapped (this is the expand view).
    let mut app = App::new();
    let output = ('a'..='f')
        .map(|c| c.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    let mut t = tool("ls", "", ToolStatus::Ok, &output);
    t.shell = true;
    app.history = vec![
        HistoryItem::Message(Message {
            role: Role::Shell,
            text: "ls".to_string(),
            timestamp: String::new(),
            images: Vec::new(),
        }),
        HistoryItem::Tool(t),
    ];
    let texts: Vec<String> = transcript_lines(&app, 60)
        .iter()
        .map(|l| plain(l).trim_end().to_string())
        .collect();
    assert!(
        texts.contains(&"! ls".to_string()),
        "dark header: {texts:?}"
    );
    assert!(
        !texts.iter().any(|t| t.starts_with("● ls")),
        "no bullet header in the overlay: {texts:?}"
    );
    assert!(texts.contains(&"  ⎿  a".to_string()), "{texts:?}");
    assert!(
        texts.contains(&"     f".to_string()),
        "every output line, uncapped: {texts:?}"
    );
    assert!(
        !texts.iter().any(|t| t.contains("ctrl+o to expand")),
        "no truncation hint in the expand view: {texts:?}"
    );
}

#[test]
fn transcript_lines_keep_the_shell_cell_flush() {
    // The Ctrl+O transcript renders the same shell cell as the inline view
    // (docs/shell-command.md): no blank spacer between the `! pwd` header
    // and its `⎿` output either.
    let mut t = tool("pwd", "", ToolStatus::Ok, "/home");
    t.shell = true;
    let mut app = App::new();
    app.history = vec![
        HistoryItem::Message(Message {
            role: Role::Shell,
            text: "pwd".to_string(),
            timestamp: String::new(),
            images: Vec::new(),
        }),
        HistoryItem::Tool(t),
    ];
    assert_eq!(transcript_body(&app, 40), vec!["! pwd", "  ⎿  /home", ""]);
}

#[test]
fn the_running_shell_transcript_sits_flush_under_its_header() {
    // The live tail too: while a `!` command runs, its committed header is
    // the last history item and the running tool renders flush below it.
    let mut app = App::new();
    app.begin_shell("sleep 5");
    let texts: Vec<String> = transcript_lines(&app, 40)
        .iter()
        .map(|l| plain(l).trim_end().to_string())
        .collect();
    let header = texts
        .iter()
        .position(|t| t == "! sleep 5")
        .expect("the shell header is in the transcript");
    assert_eq!(
        texts[header + 1],
        "  ⎿  Running…",
        "no blank between the header and the running peek: {texts:?}"
    );
}

#[test]
fn a_backend_tool_keeps_its_transcript_spacing() {
    // Only the shell cell is flush — a backend tool still gets the blank
    // spacer after the preceding message, as today.
    let app = transcript_fixture();
    let texts: Vec<String> = transcript_lines(&app, 80)
        .iter()
        .map(|l| plain(l).trim_end().to_string())
        .collect();
    let msg = texts.iter().position(|t| t == "● let me check").unwrap();
    assert_eq!(texts[msg + 1], "", "blank spacer after the message");
    assert_eq!(texts[msg + 2], "● Read(f)", "then the tool header");
}

#[test]
fn transcript_reverses_the_selected_user_message() {
    let mut app = backtrack_app();
    app.backtrack.selected = Some(0);
    let lines = transcript_lines(&app, 40);
    let range = transcript_selection(&app, 40).expect("a selection range");
    assert!(
        plain(&lines[range.start]).contains("first"),
        "the range points at the selected message"
    );
    for (i, line) in lines.iter().enumerate() {
        let reversed = line.style.add_modifier.contains(Modifier::REVERSED);
        assert_eq!(
            reversed,
            range.contains(&i),
            "only the highlighted rows are reversed (row {i})"
        );
    }
}

#[test]
fn transcript_selection_follows_the_stepped_ordinal() {
    let mut app = backtrack_app();
    app.backtrack.selected = Some(1);
    let lines = transcript_lines(&app, 40);
    let range = transcript_selection(&app, 40).expect("a selection range");
    assert!(plain(&lines[range.start]).contains("second"));
}

#[test]
fn transcript_without_a_selection_reverses_nothing() {
    let app = backtrack_app();
    assert!(transcript_selection(&app, 40).is_none());
    for line in transcript_lines(&app, 40) {
        assert!(!line.style.add_modifier.contains(Modifier::REVERSED));
    }
}

#[test]
fn scroll_into_view_moves_only_when_the_target_is_off_screen() {
    // Above the window: scroll up to its top. Below: down just enough.
    // Visible: stay put. Taller than the window: show its top.
    assert_eq!(scroll_into_view(10, &(2..4), 5), 2, "above → its top");
    assert_eq!(scroll_into_view(0, &(8..10), 5), 5, "below → just enough");
    assert_eq!(scroll_into_view(2, &(3..6), 5), 2, "visible → unchanged");
    assert_eq!(scroll_into_view(0, &(4..20), 5), 4, "tall → its top");
}

#[test]
fn backtrack_scroll_targets_the_highlight() {
    let mut app = backtrack_app();
    app.backtrack.selected = Some(0);
    app.tool_scroll = 50; // scrolled far past the first message
    let scroll = backtrack_scroll(&app, 40, 24).expect("a scroll decision");
    let range = transcript_selection(&app, 40).unwrap();
    assert_eq!(scroll, range.start, "scrolls back up to the highlight");
    assert!(
        backtrack_scroll(&App::new(), 40, 24).is_none(),
        "no selection, no decision"
    );
}

#[test]
fn tool_view_hints_swap_while_previewing() {
    let mut app = backtrack_app();
    let mut buf = buffer(80, 16);
    let tv_lines = transcript_lines(&app, buf.area.width);
    render_tool_view(buf.area, &mut buf, &app, &tv_lines);
    let idle: String = (0..16).map(|y| row(&buf, y, 80)).collect();
    assert!(idle.contains("q/esc/ctrl+o to quit"), "normal pager hints");

    app.backtrack.selected = Some(1);
    let mut buf = buffer(80, 16);
    let tv_lines = transcript_lines(&app, buf.area.width);
    render_tool_view(buf.area, &mut buf, &app, &tv_lines);
    let preview: String = (0..16)
        .map(|y| row(&buf, y, 80))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        preview.contains("enter to edit message"),
        "backtrack hints while previewing: {preview:?}"
    );
    assert!(
        !preview.contains("q/esc/ctrl+o to quit"),
        "the quit hint made way: {preview:?}"
    );
}

#[test]
fn conversation_and_transcript_walks_render_background_notices() {
    let mut app = App::new();
    app.history
        .push(HistoryItem::Background(bg_notice(Some(0), false)));
    let inline: Vec<String> = conversation_lines(&app.history, 80)
        .iter()
        .map(plain)
        .collect();
    assert!(
        inline.iter().any(|l| l.contains("completed (exit code 0)")),
        "the inline repaint shows the notice: {inline:?}"
    );
    let transcript: Vec<String> = transcript_lines(&app, 80).iter().map(plain).collect();
    assert!(
        transcript
            .iter()
            .any(|l| l.contains("completed (exit code 0)")),
        "the Ctrl+O transcript shows the notice: {transcript:?}"
    );
}

#[test]
fn the_agent_view_overlays_show_the_agents_transcript_and_context() {
    let mut app = App::new();
    app.begin_stream();
    app.start_agent_group(false, &[spec("a1", "Fetch Warsaw", false)]);
    for event in [
        crate::stream::StreamEvent::ToolStart {
            name: "Bash".into(),
            args: "curl wttr.in".into(),
            detail: None,
        },
        crate::stream::StreamEvent::ToolEnd {
            output: "+19°C".into(),
            ok: true,
            truncated: false,
        },
    ] {
        app.apply_agent_event("a1", &event);
    }
    assert!(
        agent_transcript_lines(&app, 80).is_none(),
        "no agent view — the main cache path renders"
    );
    app.open_agent_view("a1");
    let texts: Vec<String> = agent_transcript_lines(&app, 80)
        .expect("the agent view has its own transcript")
        .iter()
        .map(plain)
        .collect();
    assert!(
        texts.iter().any(|t| t.contains("task?")),
        "the prompt shows"
    );
    assert!(texts.iter().any(|t| t.starts_with("● Bash(curl wttr.in)")));
    assert!(texts.iter().any(|t| t.contains("+19°C")));
    // Ctrl+D derives the *agent's* context: its prompt is the user entry.
    let ctx: Vec<String> = context_lines(&app, 80).iter().map(plain).collect();
    assert!(ctx.iter().any(|t| t.contains("task?")), "{ctx:?}");
    assert!(
        ctx.iter().any(|t| t.contains("→ bash(")),
        "the agent's tool call replays: {ctx:?}"
    );
}

/// The transcript rows *after* the banner chrome (banner + spacer),
/// trimmed — for tests asserting on the conversation walk itself (the
/// banner atop it has its own tests).
fn transcript_body(app: &App, width: u16) -> Vec<String> {
    let chrome = header_lines(app, width).len() + 1;
    transcript_lines(app, width)
        .iter()
        .skip(chrome)
        .map(|l| plain(l).trim_end().to_string())
        .collect()
}
