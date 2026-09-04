//! Committed conversation lines and the repaint helpers.

use super::*;
use crate::ui::theme::{
    AI_COLOR, ERROR_COLOR, LIST_MARKER_COLOR, SYSTEM_COLOR, THEMATIC_BREAK, USER_BG_COLOR,
};
use crate::ui::wrap::cols;

#[test]
fn ordered_list_number_is_colored() {
    let lines = message_lines(Role::Assistant, "1. item", 40);
    let num = lines[0]
        .spans
        .iter()
        .find(|s| s.content.contains("1."))
        .expect("the ordered marker span");
    assert_eq!(num.style.fg, Some(LIST_MARKER_COLOR));
}

// --- message_lines ---

#[test]
fn message_lines_prefixes_assistant_bullet() {
    let lines = message_lines(Role::Assistant, "hello", 80);
    assert_eq!(lines.len(), 1);
    assert_eq!(plain(&lines[0]), "● hello");
}

#[test]
fn message_lines_prefixes_user_bullet() {
    let lines = message_lines(Role::User, "hello", 80);
    assert_eq!(plain(&lines[0]).trim_end(), "❯ hello");
}

#[test]
fn message_lines_indents_wrapped_continuation_lines() {
    // width 8 → content width 6 → "hello"/"world" on separate lines.
    let lines = message_lines(Role::Assistant, "hello world", 8);
    assert!(lines.len() >= 2);
    assert_eq!(plain(&lines[0]), "● hello");
    assert_eq!(plain(&lines[1]), "  world");
}

#[test]
fn message_lines_colours_the_bullet() {
    let lines = message_lines(Role::Assistant, "hi", 80);
    assert_eq!(lines[0].spans[0].style.fg, Some(AI_COLOR));
}

#[test]
fn message_lines_renders_errors_with_a_red_bullet() {
    let lines = message_lines(Role::Error, "stream failed", 80);
    assert_eq!(lines.len(), 1);
    assert!(plain(&lines[0]).contains("stream failed"));
    assert_eq!(
        lines[0].spans[0].style.fg,
        Some(ERROR_COLOR),
        "the error bullet is red, not white"
    );
    assert_ne!(ERROR_COLOR, AI_COLOR, "error colour differs from assistant");
}

#[test]
fn message_lines_applies_background_to_user_lines() {
    let lines = message_lines(Role::User, "hi there long enough to wrap", 10);
    for line in &lines {
        assert_eq!(
            line.style.bg,
            Some(USER_BG_COLOR),
            "every user line has the background"
        );
    }
}

#[test]
fn message_lines_user_spans_fill_the_full_width() {
    let width = 20u16;
    let lines = message_lines(Role::User, "hi", width);
    for line in &lines {
        let span_chars: usize = line.spans.iter().map(|s| s.content.chars().count()).sum();
        assert_eq!(
            span_chars as u16, width,
            "spans cover full width so background extends edge-to-edge"
        );
    }
}

#[test]
fn message_lines_pads_user_lines_to_full_display_width() {
    // Padding must count terminal columns, not chars: a CJK user line still
    // fills the row edge-to-edge so its dark background does not fall short.
    let width = 20u16;
    let lines = message_lines(Role::User, "你好", width);
    for line in &lines {
        let total: usize = line.spans.iter().map(|s| cols(s.content.as_ref())).sum();
        assert_eq!(total as u16, width, "user line fills full display width");
    }
}

#[test]
fn a_dash_rule_without_a_preceding_blank_stays_literal() {
    // `text\n---` is a setext H2 underline in codex, which we can't render;
    // rather than fabricate a rule we leave `---` as literal prose.
    let joined: Vec<String> = message_lines(Role::Assistant, "some text\n---", 80)
        .iter()
        .map(plain)
        .collect();
    assert!(
        joined.iter().any(|l| l.contains("---")),
        "literal --- kept: {joined:?}"
    );
    assert!(
        !joined.iter().any(|l| l.contains(THEMATIC_BREAK)),
        "no fabricated rule: {joined:?}"
    );
}

#[test]
fn inline_conversation_never_shows_the_timestamp() {
    // The "only in Ctrl+O" invariant: the inline repaint path must never
    // carry a stamp, even though its history items hold one.
    let history = stamped_history();
    let inline: String = conversation_lines(&history, 80)
        .iter()
        .map(plain)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !inline.contains(STAMP),
        "timestamps never leak into the inline view: {inline:?}"
    );
}

#[test]
fn a_hook_note_is_invisible_inline_and_expanded_in_the_transcript() {
    // Claude Code hides these from the normal view too (docs/hooks.md): the
    // inline repaint skips the item entirely, the Ctrl+O transcript shows
    // the label over the wire text.
    let note = HistoryItem::HookNote(crate::app::HookNote {
        label: "Stop hook".to_string(),
        text: "Stop hook feedback:\ntests are red".to_string(),
        timestamp: String::new(),
    });
    let history = [msg(Role::User, "hi"), note.clone()];
    let inline: String = conversation_lines(&history, 80)
        .iter()
        .map(plain)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !inline.contains("Stop hook"),
        "cell-less inline: {inline:?}"
    );
    let mut app = crate::app::App::new();
    app.history.extend(history);
    let transcript: String = transcript_lines(&app, 80)
        .iter()
        .map(plain)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        transcript.contains("● Stop hook") && transcript.contains("tests are red"),
        "the transcript is the record: {transcript:?}"
    );
}

#[test]
fn conversation_lines_lays_out_a_turn_with_a_trailing_blank() {
    let history = [msg(Role::User, "hi"), msg(Role::Assistant, "hello")];
    let texts: Vec<String> = conversation_lines(&history, 80)
        .iter()
        .map(|l| plain(l).trim_end().to_string())
        .collect();
    // User line, blank, assistant line, blank spacer after the reply.
    assert_eq!(texts, vec!["❯ hi", "", "● hello", ""]);
}

#[test]
fn conversation_lines_puts_one_blank_between_trailing_break_text_and_a_tool() {
    // The reported bug: an assistant segment ending with a paragraph break
    // (`…\n\n`) before a tool call must show exactly ONE blank row between
    // them on a repaint — not three (the trailing blanks plus the spacer).
    let history = [
        msg(Role::User, "go"),
        msg(Role::Assistant, "I'll do it.\n\n"),
        HistoryItem::Tool(tool("Bash", "ls", ToolStatus::Ok, "out")),
    ];
    let texts: Vec<String> = conversation_lines(&history, 80)
        .iter()
        .map(|l| plain(l).trim_end().to_string())
        .collect();
    let text_idx = texts
        .iter()
        .position(|t| t == "● I'll do it.")
        .unwrap_or_else(|| panic!("assistant text present: {texts:?}"));
    let tool_idx = texts
        .iter()
        .position(|t| t == "● Bash(ls)")
        .unwrap_or_else(|| panic!("tool header present: {texts:?}"));
    assert_eq!(
        tool_idx - text_idx,
        2,
        "exactly one blank row between text and tool: {texts:?}"
    );
    assert_eq!(texts[text_idx + 1], "", "the single separator is blank");
}

#[test]
fn conversation_lines_renders_a_tool_call_between_messages() {
    let history = [
        msg(Role::User, "hi"),
        HistoryItem::Tool(tool("Bash", "ls", ToolStatus::Ok, "a\nb")),
        msg(Role::Assistant, "done"),
    ];
    let texts: Vec<String> = conversation_lines(&history, 80)
        .iter()
        .map(|l| plain(l).trim_end().to_string())
        .collect();
    // user, blank, tool header, tool peek, tool hint, blank, assistant, blank
    assert_eq!(texts.first().map(String::as_str), Some("❯ hi"));
    assert!(
        texts.iter().any(|t| t == "● Bash(ls)"),
        "tool header is present in order: {texts:?}"
    );
    assert!(texts.iter().any(|t| t == "● done"));
}

#[test]
fn repaint_lines_keeps_only_the_last_max_rows() {
    // Lines are: "❯ one", "● two", "" → keep the last 2.
    let history = [msg(Role::User, "one"), msg(Role::Assistant, "two")];
    let texts: Vec<String> = repaint_lines(&history, 80, 2).iter().map(plain).collect();
    assert_eq!(texts, vec!["● two", ""]);
}

#[test]
fn repaint_lines_returns_everything_when_it_fits() {
    let history = [msg(Role::User, "hi")];
    assert_eq!(repaint_lines(&history, 80, 100).len(), 2); // message + blank
}

#[test]
fn repaint_lines_of_empty_history_is_empty() {
    assert!(repaint_lines(&[], 80, 10).is_empty());
}

#[test]
fn repaint_tail_repaints_the_partial_reply_rows_already_committed() {
    // Mid-stream repaint (the Ctrl+O overlay return): the tail must carry
    // the rows the stream had already committed to scrollback — repainting
    // from history alone blanks the partial reply until the next chunk
    // arrives (the disappear-then-flicker bug).
    let width = 30;
    let history = [msg(Role::User, "hi")];
    let partial = "first line of the reply\nsecond line still growing";
    let mut render = StreamRender::new();
    let committed: Vec<String> = render.commit(partial, width).iter().map(plain).collect();
    assert!(!committed.is_empty(), "the completed first line is stable");

    let tail: Vec<String> = repaint_tail(&history, Some(partial), &mut render, width, 100)
        .iter()
        .map(plain)
        .collect();
    let mut expected: Vec<String> = repaint_lines(&history, width, 100)
        .iter()
        .map(plain)
        .collect();
    expected.extend(committed);
    assert_eq!(tail, expected);
}

#[test]
fn repaint_tail_then_commit_catches_up_without_duplicate_or_gap() {
    // Chunks that arrived while the overlay was up commit right after the
    // repaint: committed-before ++ committed-after ++ finish must
    // reconstruct the whole reply exactly — no row lost, none inserted
    // twice (the scrollback-duplication half of the bug).
    let width = 24;
    let before = "streamed before the overlay opened\nand a second line\n";
    let full = format!("{before}plus lines that arrived\nwhile the overlay was up\nstill going");
    let mut render = StreamRender::new();
    let mut inserted: Vec<String> = render.commit(before, width).iter().map(plain).collect();

    // The overlay round-trip: the tail repaints exactly what was already
    // committed (empty history keeps the comparison direct)…
    let tail: Vec<String> = repaint_tail(&[], Some(&full), &mut render, width, 100)
        .iter()
        .map(plain)
        .collect();
    assert_eq!(tail, inserted, "the tail repaints the committed rows only");
    // …and the follow-up commit emits just the overlay-time delta.
    inserted.extend(render.commit(&full, width).iter().map(plain));
    inserted.extend(render.finish(&full, width).iter().map(plain));

    let expected: Vec<String> = message_lines(Role::Assistant, &full, width)
        .iter()
        .map(plain)
        .collect();
    assert_eq!(inserted, expected);
}

#[test]
fn repaint_tail_without_a_stream_matches_repaint_lines() {
    let history = [msg(Role::User, "one"), msg(Role::Assistant, "two")];
    let mut render = StreamRender::new();
    let tail: Vec<String> = repaint_tail(&history, None, &mut render, 80, 2)
        .iter()
        .map(plain)
        .collect();
    assert_eq!(tail, vec!["● two", ""]);
}

#[test]
fn repaint_tail_cap_keeps_the_newest_rows_including_the_partial() {
    // The row budget applies to the combined tail — history and partial
    // together — keeping the newest rows, like a screen would.
    let width = 80;
    let history = [msg(Role::User, "one")];
    let partial = "alpha\nbeta\ngamma";
    let mut render = StreamRender::new();
    let _ = render.commit(partial, width);
    let tail: Vec<String> = repaint_tail(&history, Some(partial), &mut render, width, 2)
        .iter()
        .map(plain)
        .collect();
    assert_eq!(tail.len(), 2);
    assert!(
        tail[1].contains("beta"),
        "newest stream rows kept: {tail:?}"
    );
}

#[test]
fn repaint_tail_after_a_width_change_carries_no_stale_rows() {
    // A width change rebuilt the render's cache: nothing is "already
    // committed" at the new width, so the tail carries no stale-width rows
    // and the follow-up commit re-emits the reply wrapped fresh.
    let partial = "one two three four five six seven\nnext";
    let mut render = StreamRender::new();
    let _ = render.commit(partial, 20);
    let tail = repaint_tail(&[], Some(partial), &mut render, 40, 100);
    assert!(tail.is_empty(), "no stale-width rows repainted");
    let recommitted: Vec<String> = render.commit(partial, 40).iter().map(plain).collect();
    let expected: Vec<String> = message_lines(Role::Assistant, partial, 40)
        .iter()
        .map(plain)
        .collect();
    assert_eq!(recommitted[..], expected[..recommitted.len()]);
}

#[test]
fn message_lines_renders_a_system_notice_with_a_cyan_bullet() {
    let lines = message_lines(Role::System, "a notice", 80);
    assert_eq!(lines[0].spans[0].style.fg, Some(SYSTEM_COLOR));
    assert_ne!(SYSTEM_COLOR, AI_COLOR, "distinct from an AI reply");
}

#[test]
fn compaction_lines_render_the_cyan_marker_cell() {
    // The inline cell is codex's "Context compacted" info line, in our
    // system-notice dress (cyan ●). No token info → just the notice.
    let lines = compaction_lines(&bare_compaction("s"), 80);
    assert_eq!(lines.len(), 1);
    assert_eq!(plain(&lines[0]), format!("● {COMPACTED_NOTICE}"));
    assert_eq!(lines[0].spans[0].style.fg, Some(SYSTEM_COLOR));
}

#[test]
fn the_compaction_cell_appends_the_token_shrink_and_auto_tag() {
    // The gauge info rides the cell: `· {before} → {after} tokens`, plus
    // `· auto` when the compaction was auto-triggered (docs/compact.md).
    let compaction = crate::app::Compaction {
        summary: "s".into(),
        timestamp: String::new(),
        before: 88_000,
        after: 2_100,
        auto: true,
        secs: 0,
    };
    let text = plain(&compaction_lines(&compaction, 120)[0]);
    assert!(text.starts_with(&format!("● {COMPACTED_NOTICE}")), "{text}");
    assert!(text.contains("88k → 2.1k tokens"), "{text}");
    assert!(text.ends_with("· auto"), "{text}");
}

#[test]
fn a_manual_compaction_cell_shows_the_shrink_without_the_auto_tag() {
    let compaction = crate::app::Compaction {
        summary: "s".into(),
        timestamp: String::new(),
        before: 1_000,
        after: 300,
        auto: false,
        secs: 0,
    };
    let text = plain(&compaction_lines(&compaction, 120)[0]);
    assert!(text.contains("1k → 300 tokens"), "{text}");
    assert!(!text.contains("auto"), "{text}");
}

#[test]
fn the_compaction_cell_appends_its_duration_between_the_shrink_and_the_auto_tag() {
    // `● Context compacted · 2.1k → 507 tokens · 36s · auto` — the
    // summarization turn's elapsed rides the cell (humanized past a minute),
    // 0 (an old rollout) hiding the clause (docs/compact.md).
    let compaction = crate::app::Compaction {
        summary: "s".into(),
        timestamp: String::new(),
        before: 2_100,
        after: 507,
        auto: true,
        secs: 36,
    };
    let text = plain(&compaction_lines(&compaction, 120)[0]);
    assert!(text.contains("2.1k → 507 tokens · 36s · auto"), "{text}");
    let mut long = compaction;
    long.secs = 96;
    long.auto = false;
    let text = plain(&compaction_lines(&long, 120)[0]);
    assert!(text.ends_with("tokens · 1m 36s"), "{text}");
}

#[test]
fn conversation_lines_keep_the_compaction_marker_collapsed() {
    // The inline repaint shows the one-line cell + the spacer — the summary
    // body is Ctrl+O-only.
    let history = vec![HistoryItem::Compaction(bare_compaction("kept the gist"))];
    let lines = conversation_lines(&history, 80);
    assert_eq!(lines.len(), 2, "marker + spacer: {:?}", lines.len());
    assert_eq!(plain(&lines[0]), format!("● {COMPACTED_NOTICE}"));
}

#[test]
fn conversation_lines_keep_the_shell_cell_flush() {
    // No blank spacer between the `! pwd` header and its `⎿` output —
    // they form one cell (and the running preview sits flush the same way).
    let mut t = tool("pwd", "", ToolStatus::Ok, "/home");
    t.shell = true;
    let history = vec![
        HistoryItem::Message(Message {
            role: Role::Shell,
            text: "pwd".to_string(),
            timestamp: String::new(),
            images: Vec::new(),
        }),
        HistoryItem::Tool(t),
    ];
    let texts: Vec<String> = conversation_lines(&history, 40)
        .iter()
        .map(|l| plain(l).trim_end().to_string())
        .collect();
    assert_eq!(texts, vec!["! pwd", "  ⎿  /home", ""]);
}

// --- literal roles echo their text's own spacing (docs/textarea.md) ---

#[test]
fn user_messages_keep_their_spacing() {
    // What was typed is what is echoed: the composer shows a run of spaces
    // and a line's indentation as typed, so the bubble it becomes must agree
    // instead of collapsing `a    b` to `a b` and dropping the indent.
    let lines = message_lines(Role::User, "a    b\n    indented", 40);
    assert_eq!(plain(&lines[0]).trim_end(), "❯ a    b");
    assert_eq!(plain(&lines[1]).trim_end(), "      indented");
}

#[test]
fn a_shell_header_keeps_the_commands_spacing() {
    // `! echo "a   b"` is a command — its spaces are data.
    let lines = message_lines(Role::Shell, "echo \"a   b\"", 40);
    assert_eq!(plain(&lines[0]).trim_end(), "! echo \"a   b\"");
}

#[test]
fn user_messages_expand_tabs_for_display() {
    // A tab paints as zero cells (ratatui drops control characters) but
    // measures as one, which would leave the dark row a column short of the
    // edge; it expands like a code block's, the record untouched.
    let lines = message_lines(Role::User, "a\tb", 20);
    assert_eq!(plain(&lines[0]).trim_end(), "❯ a    b");
    let total: usize = lines[0]
        .spans
        .iter()
        .map(|s| cols(s.content.as_ref()))
        .sum();
    assert_eq!(total, 20, "the background still fills the row");
}

#[test]
fn user_messages_still_wrap_at_word_boundaries() {
    // Width 12 → content 10: the message breaks at spaces and every
    // continuation starts at a word, as before.
    let lines = message_lines(Role::User, "hello world again", 12);
    let rows: Vec<String> = lines
        .iter()
        .map(|l| plain(l).trim_end().to_string())
        .collect();
    assert_eq!(rows, vec!["❯ hello", "  world", "  again"]);
}
