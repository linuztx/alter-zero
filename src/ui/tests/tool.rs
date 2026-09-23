//! Tool cells: headers, output peeks, and file-change rendering
//! (`docs/tools.md`, `docs/tool-streaming.md`).

use super::*;
use crate::ui::theme::{
    CODE_TAB_WIDTH, EXPAND_HINT, FILE_PEEK_LINES, TOOL_BULLET, TOOL_FOLD_ROWS,
    TOOL_HEADER_ELLIPSIS, TOOL_HEADER_MAX_COLS, TOOL_HEADER_MAX_LINES, TOOL_JSON_PRETTY_MAX_BYTES,
    TOOL_LINE_ELLIPSIS, TOOL_LINE_MAX_ROWS, TOOL_PULSE_PERIOD, TOOL_TRUNCATED_MARKER,
    tool_args_color, tool_diff_add_bg, tool_diff_add_color, tool_diff_add_mark_bg,
    tool_diff_del_bg, tool_diff_del_color, tool_diff_del_mark_bg, tool_dim_color, tool_fail_color,
    tool_ok_color, tool_output_color, tool_running_color, tool_waiting_color,
};
use crate::ui::tool::{live_tool_lines, running_command_lines, tool_full_lines};
use crate::ui::wrap::cols;

/// A theme RGB triple as the `Color` a rendered span carries.
/// The content of a `⎿` gutter row — the corner (or a continuation row's
/// matching indent) stripped, so a test can name the text a row carries.
fn gutter_content(row: &str) -> &str {
    row.trim_start().trim_start_matches('⎿').trim_start()
}

// --- tool_lines (collapsed, colour-by-status) ---

#[test]
fn tool_lines_header_shows_name_and_args() {
    let lines = tool_lines(
        &tool("Bash", "cargo test", ToolStatus::Ok, "a\nb\nc"),
        80,
        &PathDisplay::VERBATIM,
    );
    assert_eq!(plain(&lines[0]), "● Bash(cargo test)");
}

#[test]
fn tool_lines_header_omits_the_parens_when_args_are_empty() {
    // A `!` shell command is a tool with no args (name = the command), so
    // its header reads `● {command}`, not `● {command}()`.
    let lines = tool_lines(
        &tool("echo hi", "", ToolStatus::Ok, "hi"),
        80,
        &PathDisplay::VERBATIM,
    );
    assert_eq!(plain(&lines[0]), "● echo hi");
}

// --- the resolved AskUserQuestion cell (docs/ask.md) ---

#[test]
fn an_answered_ask_cell_promotes_the_headline_to_the_header() {
    // The reference transcript: `● User answered Alter Zero's questions:` over the
    // `⎿ · Q → A` rows — the tool name never shows on a resolved cell.
    let output = "User answered Alter Zero's questions:\n\
                  · What's your favorite way to drink coffee? → Black\n\
                  · Pick a snack → Chips, Fruit";
    let cell = tool("AskUserQuestion", "ignored", ToolStatus::Ok, output);
    let lines = tool_lines(&cell, 100, &PathDisplay::VERBATIM);
    assert_eq!(plain(&lines[0]), "● User answered Alter Zero's questions:");
    let bullet = &lines[0].spans[0];
    assert_eq!(
        bullet.style.fg,
        Some(tool_ok_color()),
        "a submission is green"
    );
    assert!(
        plain(&lines[1]).contains("· What's your favorite way to drink coffee? → Black"),
        "got {:?}",
        plain(&lines[1])
    );
    assert!(
        !lines
            .iter()
            .map(plain)
            .any(|l| l.contains("AskUserQuestion")),
        "the headline replaces the tool-name header"
    );
    // The Ctrl+O transcript renders the same header with the rows uncapped.
    let full = tool_full_lines(&cell, 100, &PathDisplay::VERBATIM);
    assert_eq!(plain(&full[0]), "● User answered Alter Zero's questions:");
    assert!(plain(&full[2]).contains("· Pick a snack → Chips, Fruit"));
}

#[test]
fn a_declined_ask_cell_is_red_and_lists_the_questions() {
    let output = "User declined to answer questions\n\
                  · Which code style do you prefer? (Arrow function / One-liner)";
    let cell = tool("AskUserQuestion", "ignored", ToolStatus::Failed, output);
    let lines = tool_lines(&cell, 100, &PathDisplay::VERBATIM);
    assert_eq!(plain(&lines[0]), "● User declined to answer questions");
    assert_eq!(lines[0].spans[0].style.fg, Some(tool_fail_color()));
    assert!(plain(&lines[1]).contains("(Arrow function / One-liner)"));
}

#[test]
fn a_running_ask_cell_keeps_the_generic_header() {
    // While the modal is up the call is Running — the ordinary
    // `● AskUserQuestion(…)` header stands (mostly hidden behind the modal;
    // the Ctrl+O transcript shows it).
    let cell = tool("AskUserQuestion", "Pick one?", ToolStatus::Running, "");
    let lines = tool_lines(&cell, 100, &PathDisplay::VERBATIM);
    assert!(
        plain(&lines[0]).starts_with("● AskUserQuestion(Pick one?)"),
        "got {:?}",
        plain(&lines[0])
    );
}

// --- the auto mode classifier's provenance note (docs/permissions.md) ---

/// [`tool`] with the classifier's allowed note riding it.
fn noted(name: &str, args: &str, status: ToolStatus, output: &str) -> ToolCall {
    let mut call = tool(name, args, status, output);
    call.approval_note = Some("Allowed by auto mode classifier".to_string());
    call
}

#[test]
fn a_classifier_allowed_bash_cell_appends_the_note_row() {
    // The reference transcript: the collapsed cell's output peek (and its
    // `… +N lines` hint) first, then a fresh dim `⎿` row with the note.
    let output = "Exit code: 0\ntotal 40\na\nb\nc\nd\ne\nf";
    let lines = tool_lines(
        &noted("Bash", "ls -la", ToolStatus::Ok, output),
        80,
        &PathDisplay::VERBATIM,
    );
    let texts: Vec<String> = lines.iter().map(plain).collect();
    assert_eq!(
        texts.last().map(String::as_str),
        Some("  ⎿  Allowed by auto mode classifier"),
        "the note is the cell's last row: {texts:?}"
    );
    assert!(
        texts[texts.len() - 2].contains(EXPAND_HINT),
        "the note sits under the hint, not inside the output: {texts:?}"
    );
    let note = lines.last().unwrap();
    assert_eq!(
        note.spans.last().unwrap().style.fg,
        Some(tool_dim_color()),
        "the note is meta, so it renders dim"
    );
}

#[test]
fn the_note_shows_in_the_expanded_transcript_view_too() {
    let lines = tool_full_lines(
        &noted("Bash", "ls -la", ToolStatus::Ok, "Exit code: 0\ntotal 40"),
        80,
        &PathDisplay::VERBATIM,
    );
    let texts: Vec<String> = lines.iter().map(plain).collect();
    assert_eq!(
        texts.last().map(String::as_str),
        Some("  ⎿  Allowed by auto mode classifier"),
        "got {texts:?}"
    );
}

#[test]
fn the_note_waits_for_the_call_to_resolve() {
    // The note is set right after ToolStart, but the user's example (and the
    // request) show it once the command has finished — a running cell keeps
    // its live look untouched.
    for status in [ToolStatus::Waiting, ToolStatus::Running] {
        let lines = tool_lines(
            &noted("Bash", "ls -la", status, ""),
            80,
            &PathDisplay::VERBATIM,
        );
        assert!(
            !lines.iter().map(plain).any(|t| t.contains("Allowed by")),
            "no note while {status:?}"
        );
    }
    // …and a failed run still shows it: the classifier did allow the call.
    let lines = tool_lines(
        &noted("Bash", "ls /gone", ToolStatus::Failed, "Exit code: 2"),
        80,
        &PathDisplay::VERBATIM,
    );
    assert!(
        lines
            .iter()
            .map(plain)
            .any(|t| t.ends_with("Allowed by auto mode classifier"))
    );
}

#[test]
fn a_noted_backgrounded_cell_keeps_its_fixed_row_over_the_note() {
    let lines = tool_lines(
        &noted(
            "Bash",
            "ping x.com",
            ToolStatus::Backgrounded,
            "launch text",
        ),
        80,
        &PathDisplay::VERBATIM,
    );
    let texts: Vec<String> = lines.iter().map(plain).collect();
    assert!(
        texts
            .iter()
            .any(|t| t.contains("Running in the background")),
        "got {texts:?}"
    );
    assert_eq!(
        texts.last().map(String::as_str),
        Some("  ⎿  Allowed by auto mode classifier"),
        "got {texts:?}"
    );
}

#[test]
fn an_unnoted_cell_is_byte_identical_to_before_the_feature() {
    let plain_cell = tool_lines(
        &tool("Bash", "ls", ToolStatus::Ok, "Exit code: 0\nout"),
        80,
        &PathDisplay::VERBATIM,
    );
    assert!(
        !plain_cell
            .iter()
            .map(plain)
            .any(|t| t.contains("classifier")),
        "no note row without a note"
    );
}

#[test]
fn tool_lines_colours_the_bullet_by_status() {
    for (status, color) in [
        (ToolStatus::Running, tool_running_color()),
        (ToolStatus::Ok, tool_ok_color()),
        (ToolStatus::Failed, tool_fail_color()),
    ] {
        let lines = tool_lines(&tool("X", "y", status, "out"), 80, &PathDisplay::VERBATIM);
        assert_eq!(
            lines[0].spans[0].style.fg,
            Some(color),
            "bullet colour tracks status {status:?}"
        );
    }
}

#[test]
fn a_running_bullet_is_the_permission_prompts_grey_never_blue() {
    // The running `●` used to be blue. It now reads like the grey bullet the
    // permission prompt shows over its pending call — one muted palette for
    // "in flight", the green/red resolution the only colour that lands.
    let lines = tool_lines(
        &tool("Bash", "cargo test", ToolStatus::Running, ""),
        80,
        &PathDisplay::VERBATIM,
    );
    assert_eq!(lines[0].spans[0].style.fg, Some(tool_dim_color()));
}

#[test]
fn a_running_bullet_blinks_across_the_pulse_period() {
    // …and in the live region it blinks, Claude Code's running dot: shown
    // for the first half of every TOOL_PULSE_PERIOD, hidden for the second —
    // a pure function of the boundary-injected frame clock. Hidden is the
    // same width of blanks, so the header text never shifts; shown is the
    // one resting grey, never a blend of two (docs/tool-pulse.md).
    let call = tool("Bash", "cargo test", ToolStatus::Running, "");
    let bullet = |at: Duration| {
        let mut lines = live_tool_lines(&call, 80, at, &PathDisplay::VERBATIM);
        let line = lines.swap_remove(0);
        (
            line.spans[0].content.to_string(),
            line.spans[0].style.fg,
            plain(&line),
        )
    };
    let blanks = " ".repeat(cols(TOOL_BULLET));
    let half = TOOL_PULSE_PERIOD / 2;
    let (glyph, color, text) = bullet(Duration::ZERO);
    assert_eq!(glyph, TOOL_BULLET, "shown at the top of the cycle");
    assert_eq!(
        color,
        Some(tool_running_color()),
        "one colour — the resting grey"
    );
    assert_eq!(text, "● Bash(cargo test)");
    let (glyph, _, text) = bullet(half);
    assert_eq!(
        glyph, blanks,
        "hidden at the half: blanks of the same width"
    );
    assert_eq!(
        text, "  Bash(cargo test)",
        "the header text keeps its column"
    );
    // Inside each half it holds — no in-between frame, never a blend.
    assert_eq!(bullet(TOOL_PULSE_PERIOD / 4).0, TOOL_BULLET);
    assert_eq!(bullet(TOOL_PULSE_PERIOD * 3 / 4).0, blanks);
    // A full period later the cycle loops.
    assert_eq!(bullet(TOOL_PULSE_PERIOD).0, TOOL_BULLET);
    assert_eq!(bullet(TOOL_PULSE_PERIOD + half).0, blanks);
}

#[test]
fn only_a_running_bullet_pulses() {
    // The pulse means "this is happening now". A queued sibling stays its flat
    // waiting grey and a resolved call keeps its green/red, whatever the frame
    // clock says — otherwise the animation would say nothing.
    for (status, color) in [
        (ToolStatus::Waiting, tool_waiting_color()),
        (ToolStatus::Ok, tool_ok_color()),
        (ToolStatus::Failed, tool_fail_color()),
    ] {
        let call = tool("X", "y", status, "out");
        for at in [Duration::ZERO, TOOL_PULSE_PERIOD / 2] {
            let lines = live_tool_lines(&call, 80, at, &PathDisplay::VERBATIM);
            assert_eq!(
                lines[0].spans[0].style.fg,
                Some(color),
                "{status:?} never animates"
            );
            assert_eq!(
                lines[0].spans[0].content, TOOL_BULLET,
                "{status:?} never hides its bullet"
            );
        }
    }
}

#[test]
fn a_committed_cell_never_carries_a_pulse_frame() {
    // `tool_lines` feeds scrollback, where a row is frozen forever. It
    // renders a running bullet **at rest** — shown, in the flat grey — so a
    // cell can never be committed on the blink's hidden half, headless.
    let call = tool("Bash", "cargo test", ToolStatus::Running, "");
    let lines = tool_lines(&call, 80, &PathDisplay::VERBATIM);
    assert_eq!(lines[0].spans[0].style.fg, Some(tool_running_color()));
    assert_eq!(
        lines[0].spans[0].content, TOOL_BULLET,
        "at rest the bullet is always drawn"
    );
}

#[test]
fn tool_lines_collapses_a_command_output_to_a_multiline_peek_plus_hint() {
    // A finished command-style backend tool (bash) shows the first
    // TOOL_FOLD_ROWS rows of its output — the head, Claude-Code style — then a
    // `… +N lines (ctrl+o to expand)` hint (docs/tool-streaming.md); the `!`
    // shell cell alone shows its output whole. (This is the mock's finished
    // state.)
    let out = "l1\nl2\nl3\nl4\nl5\nl6";
    let lines = tool_lines(
        &tool("Bash", "seq 6", ToolStatus::Ok, out),
        80,
        &PathDisplay::VERBATIM,
    );
    assert_eq!(
        lines.len(),
        TOOL_FOLD_ROWS + 2,
        "header + {TOOL_FOLD_ROWS} peek rows + hint: {:?}",
        lines.iter().map(plain).collect::<Vec<_>>()
    );
    assert!(
        plain(&lines[1]).contains("l1"),
        "peek opens at the first line"
    );
    assert!(
        plain(&lines[TOOL_FOLD_ROWS]).contains("l3"),
        "peek shows up to the {TOOL_FOLD_ROWS}th line"
    );
    let hint = plain(&lines[TOOL_FOLD_ROWS + 1]);
    assert!(
        hint.contains("+3 lines"),
        "hint counts hidden lines: {hint:?}"
    );
    assert!(
        hint.contains("ctrl+o to expand"),
        "hint mentions ctrl+o: {hint:?}"
    );
}

#[test]
fn a_finished_peek_shows_the_first_lines_fully_wrapped() {
    // Inside the budget a line is shown **wrapped**, never clipped at the
    // terminal width — a long first line's tail stays readable and its
    // siblings still show. Three lines here, the first wrapping to 2 rows →
    // all three visible in the 4-row block, no hint.
    let out = format!("{}\nbee\nsea", "a".repeat(60)); // 35 content cols → 2 rows
    let lines: Vec<String> = tool_lines(
        &tool("Bash", "cat log", ToolStatus::Ok, &out),
        40,
        &PathDisplay::VERBATIM,
    )
    .iter()
    .map(plain)
    .collect();
    let joined = lines.join("\n");
    assert!(
        joined.contains("bee") && joined.contains("sea"),
        "every source line within the budget shows: {lines:?}"
    );
    assert!(
        !joined.contains("ctrl+o to expand"),
        "nothing is hidden — no hint: {lines:?}"
    );
    assert_eq!(lines.len(), 1 + 4, "header + 2+1+1 wrapped rows: {lines:?}");
}

#[test]
fn a_finished_peek_bounds_rows_and_hints_when_one_line_overflows_the_budget() {
    // The fold: FOUR pathological lines (a minified bundle each) spend
    // TOOL_FOLD_ROWS rows between them — the cell is bounded in display rows,
    // so it can never balloon past what four short lines would cost. The hint
    // counts every display row hidden underneath, across all four
    // (docs/long-lines.md).
    let long = "x".repeat(600); // 35 content cols → 18 rows uncapped
    let out = vec![long; 4].join("\n");
    let lines: Vec<String> = tool_lines(
        &tool("Bash", "cat big", ToolStatus::Ok, &out),
        40,
        &PathDisplay::VERBATIM,
    )
    .iter()
    .map(plain)
    .collect();
    assert_eq!(
        lines.len(),
        1 + TOOL_FOLD_ROWS + 1,
        "header + the {TOOL_FOLD_ROWS}-row fold + hint: {lines:?}"
    );
    let hint = lines.last().unwrap();
    assert!(
        hint.contains("ctrl+o to expand"),
        "the hint signals more: {hint:?}"
    );
    assert!(
        hint.contains(&format!("+{} lines", 4 * 18 - TOOL_FOLD_ROWS)),
        "every row hidden under the fold is counted: {hint:?}"
    );
}

#[test]
fn a_finished_command_peek_skips_the_output_s_leading_blank_lines() {
    // A command whose output opens on a blank line (a `\n` before the real
    // first row — an `echo` with a leading newline, a formatter's spacer)
    // used to spend the cell's first row on nothing. The peek opens at the
    // first line that has something on it (docs/long-lines.md).
    let out = "\nl1\nl2\nl3\nl4\nl5";
    let lines: Vec<String> = tool_lines(
        &tool("Bash", "seq", ToolStatus::Ok, out),
        80,
        &PathDisplay::VERBATIM,
    )
    .iter()
    .map(plain)
    .collect();
    assert_eq!(
        lines.len(),
        1 + TOOL_FOLD_ROWS + 1,
        "header + {TOOL_FOLD_ROWS} content rows + hint — no blank row: {lines:?}"
    );
    assert_eq!(
        gutter_content(&lines[1]),
        "l1",
        "the peek opens at the first non-blank line: {lines:?}"
    );
    let hint = lines.last().unwrap();
    assert!(
        hint.contains("+3 lines"),
        "the skipped blank is still counted as a hidden row, with l4 and l5: {hint:?}"
    );
}

#[test]
fn a_finished_command_peek_stops_at_the_first_blank_line() {
    // The peek is the output's first **block**: once a blank line arrives the
    // cell stops rather than spending a row on it (and rather than hopping the
    // gap, which would read as one run of lines that isn't one).
    let out = "l1\nl2\n\nl3\nl4\nl5";
    let lines: Vec<String> = tool_lines(
        &tool("Bash", "seq", ToolStatus::Ok, out),
        80,
        &PathDisplay::VERBATIM,
    )
    .iter()
    .map(plain)
    .collect();
    assert_eq!(
        lines.len(),
        1 + 2 + 1,
        "header + the two rows before the blank + hint: {lines:?}"
    );
    assert_eq!(gutter_content(&lines[1]), "l1", "{lines:?}");
    assert_eq!(gutter_content(&lines[2]), "l2", "{lines:?}");
    let hint = lines.last().unwrap();
    assert!(
        hint.contains("+4 lines"),
        "the blank and everything under it are hidden rows: {hint:?}"
    );
}

#[test]
fn a_finished_command_peek_reads_a_blank_only_output_as_no_output() {
    // Trailing blank lines are dropped from the display, so an output that is
    // nothing but newlines has no row left to show: the cell says so with the
    // `(no output)` placeholder rather than painting three rows of nothing
    // (and the transcript agrees).
    let out = "\n\n\n";
    let lines: Vec<String> = tool_lines(
        &tool("Bash", "printf", ToolStatus::Ok, out),
        80,
        &PathDisplay::VERBATIM,
    )
    .iter()
    .map(plain)
    .collect();
    assert_eq!(lines[1..], ["  ⎿  (no output)"], "{lines:?}");
    let full: Vec<String> = tool_full_lines(
        &tool("Bash", "printf", ToolStatus::Ok, out),
        80,
        &PathDisplay::VERBATIM,
    )
    .iter()
    .map(plain)
    .collect();
    assert_eq!(full[1..], ["  ⎿  (no output)"], "{full:?}");
}

#[test]
fn a_shell_cell_shows_its_whole_output_inline() {
    // The `!` shell cell does not fold (docs/shell-command.md): the user ran
    // the command to read its output, so every display line shows inline —
    // the same rows the Ctrl+O view paints, the leading blanks the command
    // printed included — with no `… +N lines (ctrl+o to expand)` hint. Only
    // the backend `bash` cell keeps Claude Code's fold.
    let mut t = tool(
        "printf '\\n\\nout\\n'",
        "",
        ToolStatus::Ok,
        "\n\nout\ntail\nmore",
    );
    t.shell = true;
    let lines: Vec<String> = tool_lines(&t, 80, &PathDisplay::VERBATIM)
        .iter()
        .map(plain)
        .collect();
    assert_eq!(
        lines.len(),
        5,
        "every display line, nothing folded: {lines:?}"
    );
    assert_eq!(gutter_content(&lines[0]), "", "{lines:?}");
    assert_eq!(gutter_content(&lines[1]), "", "{lines:?}");
    assert_eq!(gutter_content(&lines[2]), "out", "{lines:?}");
    assert_eq!(gutter_content(&lines[3]), "tail", "{lines:?}");
    assert_eq!(gutter_content(&lines[4]), "more", "{lines:?}");
    assert!(
        !lines.iter().any(|l| l.contains("ctrl+o")),
        "nothing is hidden, so nothing to expand: {lines:?}"
    );
}

#[test]
fn a_shell_cell_keeps_its_interior_blank_lines() {
    // With no fold there is no block to prefer: an interior blank line is a
    // row the command printed, shown as one — exactly as Ctrl+O shows it.
    let mut t = tool(
        "git status",
        "",
        ToolStatus::Ok,
        "On branch main\n\nChanges not staged:\n  modified: a\n\nUntracked:\n  b",
    );
    t.shell = true;
    let lines: Vec<String> = tool_lines(&t, 80, &PathDisplay::VERBATIM)
        .iter()
        .map(|l| plain(l).trim_end().to_string())
        .collect();
    assert_eq!(
        lines,
        [
            "  ⎿  On branch main",
            "",
            "     Changes not staged:",
            "       modified: a",
            "",
            "     Untracked:",
            "       b",
        ],
        "{lines:?}"
    );
}

#[test]
fn a_truncated_shell_cell_marks_the_cut_inline() {
    // Output over the in-memory cap used to rely on the `… +N lines` hint to
    // say more followed; with the whole retained output inline, the dim `…`
    // marker the Ctrl+O view appends closes the inline cell too, so the cut
    // is still visible (docs/shell-command.md).
    let mut t = tool("seq 1 50000", "", ToolStatus::Ok, "1\n2\n3\n4\n5\n6");
    t.shell = true;
    t.truncated = true;
    let lines: Vec<String> = tool_lines(&t, 80, &PathDisplay::VERBATIM)
        .iter()
        .map(|l| plain(l).trim_end().to_string())
        .collect();
    assert_eq!(lines.len(), 7, "six retained rows + the marker: {lines:?}");
    assert_eq!(lines[5], "     6", "{lines:?}");
    assert_eq!(lines[6].trim(), TOOL_TRUNCATED_MARKER, "{lines:?}");
    // A complete output appends nothing.
    t.truncated = false;
    let complete = tool_lines(&t, 80, &PathDisplay::VERBATIM);
    assert_eq!(complete.len(), 6, "no marker on a complete output");
}

#[test]
fn a_blank_line_inside_a_wrapped_first_block_still_closes_the_peek() {
    // The block rule is applied to **source** lines before wrapping, so a
    // wrapping line inside the block still shows whole and the blank after it
    // still ends the cell.
    let out = format!("{}\n\nafter\nmore\nstill", "a".repeat(60)); // 35 cols → 2 rows
    let lines: Vec<String> = tool_lines(
        &tool("Bash", "cat", ToolStatus::Ok, &out),
        40,
        &PathDisplay::VERBATIM,
    )
    .iter()
    .map(plain)
    .collect();
    assert_eq!(
        lines.len(),
        1 + 2 + 1,
        "header + the long line's two wrapped rows + hint: {lines:?}"
    );
    let hint = lines.last().unwrap();
    assert!(
        hint.contains("+4 lines"),
        "the blank and the three lines after it: {hint:?}"
    );
}

#[test]
fn a_finished_peek_wraps_the_sudo_error_at_word_boundaries() {
    // The user-visible polish over the plain hard-break: "askpass" must
    // never render as "as / kpass" across rows — the peek word-wraps like
    // prose while still preserving the line's own spaces.
    let out = "sudo: a terminal is required to read the password; either use \
               the -S option to read from standard input or configure an \
               askpass helper";
    let mut t = tool("sudo pacman -Rns steam", "", ToolStatus::Failed, out);
    t.shell = true;
    let rows: Vec<String> = tool_lines(&t, 66, &PathDisplay::VERBATIM)
        .iter()
        .map(plain)
        .collect();
    assert!(
        rows.iter().any(|r| r.contains("askpass")),
        "askpass stays intact on one row: {rows:?}"
    );
    for r in &rows[1..] {
        let content: String = r.chars().skip(5).collect(); // drop the gutter
        assert!(
            !content.starts_with(' '),
            "continuations start at a word: {rows:?}"
        );
    }
}

#[test]
fn the_full_view_surfaces_the_exit_code_on_failure_too() {
    let lines = tool_full_lines(
        &tool("Bash", "false", ToolStatus::Failed, "Exit code: 2\nnope"),
        80,
        &PathDisplay::VERBATIM,
    );
    let all = lines.iter().map(plain).collect::<Vec<_>>().join("\n");
    assert!(
        all.contains("Error: Exit code 2") && all.contains("nope"),
        "got {all:?}"
    );
}

#[test]
fn a_diff_peek_continuation_row_keeps_the_source_line_colour() {
    // The legacy (unparseable-output) diff peek: a long `+` line wraps; the
    // continuation rows must keep the SOURCE line's green, not fall to dim
    // because their own first char has no marker — the Ctrl+O view already
    // colours by source line (diff_line_color before wrapping).
    let long = format!("+{}", "a".repeat(60));
    let lines = tool_lines(
        &tool("Edit", "f", ToolStatus::Ok, &long),
        40,
        &PathDisplay::VERBATIM,
    );
    let rows: Vec<_> = lines[1..].iter().collect();
    assert!(
        rows.len() >= 2,
        "the long + line wrapped: {:?}",
        rows.iter().map(|l| plain(l)).collect::<Vec<_>>()
    );
    for r in &rows {
        assert_eq!(
            r.spans[1].style.fg,
            Some(tool_diff_add_color()),
            "every wrapped row keeps the + colour: {:?}",
            plain(r)
        );
    }
}

#[test]
fn a_non_command_tool_with_raw_multiline_output_shares_the_folded_peek() {
    // A generic backend tool whose output isn't the numbered file-cell format
    // (e.g. the dummy's canned `Read`, or an unknown tool) shows the same
    // folded peek the exec cells do — Claude Code's one tool-result surface:
    // TOOL_FOLD_ROWS rows, then the hint (docs/tool-streaming.md).
    let lines = tool_lines(
        &tool("Read", "f", ToolStatus::Ok, "one\ntwo\nthree\nfour\nfive"),
        80,
        &PathDisplay::VERBATIM,
    );
    assert_eq!(
        lines.len(),
        1 + TOOL_FOLD_ROWS + 1,
        "header + the fold + hint: {:?}",
        lines.iter().map(plain).collect::<Vec<_>>()
    );
    assert!(
        plain(&lines[1]).contains("one"),
        "peek shows the first line"
    );
    assert!(
        !lines.iter().any(|l| plain(l).contains("four")),
        "the rest stays hidden inline"
    );
    assert!(
        plain(lines.last().unwrap()).contains("+2 lines"),
        "the hint counts the rest"
    );
}

#[test]
fn tool_lines_strips_the_leading_exit_code_frame_from_a_bash_cell() {
    // `tool.output` stays framed (`Exit code: N\n…`) for the model / context
    // replay, but the display drops that first line so the cell reads like
    // the real command output (docs/tool-streaming.md).
    let out = "Exit code: 0\nhello\nworld";
    let lines = tool_lines(
        &tool("Bash", "echo", ToolStatus::Ok, out),
        80,
        &PathDisplay::VERBATIM,
    );
    let all: String = lines.iter().map(plain).collect::<Vec<_>>().join("\n");
    assert!(
        !all.contains("Exit code"),
        "the frame line is hidden: {all:?}"
    );
    assert!(
        all.contains("hello") && all.contains("world"),
        "the body shows: {all:?}"
    );
}

#[test]
fn running_command_lines_tails_recent_output_with_the_elapsed() {
    // The mock's running state: the header, the last TOOL_PEEK_ROWS output
    // lines (the *tail* — what just happened), and a `+N lines (Ns)` footer
    // counting the lines hidden above plus the elapsed.
    let out = (1..=9)
        .map(|i| format!("line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let t = tool("Bash", "ping -c 10 x", ToolStatus::Running, &out);
    let lines = running_command_lines(
        &t,
        Duration::from_secs(9),
        Duration::ZERO,
        80,
        &PathDisplay::VERBATIM,
    );
    assert_eq!(plain(&lines[0]), "● Bash(ping -c 10 x)");
    let body: Vec<String> = lines[1..].iter().map(plain).collect();
    assert!(
        body[0].contains("line 6"),
        "the tail starts at line 6: {body:?}"
    );
    assert!(
        body[3].contains("line 9"),
        "the tail ends at the newest line: {body:?}"
    );
    assert!(
        !body.iter().any(|l| l.contains("line 4")),
        "older lines are hidden above the tail: {body:?}"
    );
    assert_eq!(
        body.last().unwrap().trim(),
        "+5 lines (9s · timeout 2m)",
        "the footer counts hidden lines, the elapsed and the timeout: {body:?}"
    );
}

#[test]
fn the_running_footer_names_the_timeout_the_call_runs_under() {
    // The reported ask: beside the elapsed, the footer says how long the
    // command *may* run — the model's own `timeout`, read off the call's
    // verbatim arguments (`ToolCall::arguments`) and humanized as a limit:
    // `+18 lines (22s · timeout 1m 50s)` (docs/tool-streaming.md).
    let command = "for i in $(seq 1 100); do echo $i; sleep 1; done";
    let out = (1..=22)
        .map(|i| i.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    let mut t = tool("Bash", command, ToolStatus::Running, &out);
    t.arguments = Some(format!(r#"{{"command":{command:?},"timeout":110000}}"#));
    let lines: Vec<String> = running_command_lines(
        &t,
        Duration::from_secs(22),
        Duration::ZERO,
        80,
        &PathDisplay::VERBATIM,
    )
    .iter()
    .map(plain)
    .collect();
    assert_eq!(
        lines[1..],
        [
            "  ⎿  19",
            "     20",
            "     21",
            "     22",
            "     +18 lines (22s · timeout 1m 50s)",
        ],
        "{lines:?}"
    );
}

#[test]
fn a_running_command_with_no_output_counts_on_its_running_row() {
    // A silent command used to sit on a bare `⎿ Running…` for as long as it
    // ran. The row carries the clock clause now — `Running… (10s · timeout
    // 2m)` — so the user can see it is alive, and for how much longer at
    // most (docs/tool-streaming.md).
    let t = tool(
        "Bash",
        r#"python3 -c "import time; time.sleep(100)""#,
        ToolStatus::Running,
        "",
    );
    let lines: Vec<String> = running_command_lines(
        &t,
        Duration::from_secs(10),
        Duration::ZERO,
        80,
        &PathDisplay::VERBATIM,
    )
    .iter()
    .map(plain)
    .collect();
    assert_eq!(
        lines,
        [
            r#"● Bash(python3 -c "import time; time.sleep(100)")"#,
            "  ⎿  Running… (10s · timeout 2m)",
        ]
    );
}

#[test]
fn running_footers_humanize_the_elapsed_past_a_minute() {
    // The user-report fix: every running counter reads `2m 3s`, never a bare
    // `123s` — the streaming footer and the `!` shell's Running row alike
    // (format_elapsed everywhere a runtime shows).
    let out = (1..=9)
        .map(|i| format!("line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let t = tool("Bash", "ping -c 200 x", ToolStatus::Running, &out);
    let lines = running_command_lines(
        &t,
        Duration::from_secs(123),
        Duration::ZERO,
        80,
        &PathDisplay::VERBATIM,
    );
    assert_eq!(
        plain(lines.last().unwrap()).trim(),
        "+5 lines (2m 3s · timeout 2m)",
        "the streaming footer humanizes"
    );
    let row = plain(&crate::ui::tool::shell_running_line(Duration::from_secs(
        75,
    )));
    assert!(
        row.ends_with("Running… (1m 15s)"),
        "the shell running row humanizes: {row:?}"
    );
}

#[test]
fn running_command_lines_without_overflow_shows_the_clock_row_alone() {
    // Fewer lines than the window: show them all, then the clock row with
    // no `+N lines` count in front of it — `(1s · timeout 2m)` — so a
    // command whose output fits still says how long it has run and how long
    // it may (docs/tool-streaming.md).
    let t = tool("Bash", "echo", ToolStatus::Running, "a\nb");
    let lines = running_command_lines(
        &t,
        Duration::from_secs(1),
        Duration::ZERO,
        80,
        &PathDisplay::VERBATIM,
    );
    let body: Vec<String> = lines[1..].iter().map(plain).collect();
    assert_eq!(body.len(), 3, "2 output rows + the clock row: {body:?}");
    assert!(
        !body.iter().any(|l| l.contains("lines (")),
        "no hidden count when nothing is hidden: {body:?}"
    );
    assert_eq!(body[2].trim(), "(1s · timeout 2m)", "{body:?}");
}

#[test]
fn running_command_lines_tail_window_counts_display_rows_when_lines_wrap() {
    // The TOOL_PEEK_ROWS window is counted in *display rows*, so a wrapping tail
    // can't grow the strip past its budget — a long newest line
    // tail-follows its own newest rows. The `+N lines` footer keeps
    // counting source lines, and only the ones *fully* hidden above the
    // window (a partially shown wrapped line is on screen, not hidden).
    let long = "x".repeat(70); // 35 content cols → exactly 2 rows
    let out = format!("alpha\nbeta\ngamma\n{long}");
    let t = tool("Bash", "cat log", ToolStatus::Running, &out);
    let lines = running_command_lines(
        &t,
        Duration::from_secs(7),
        Duration::ZERO,
        40,
        &PathDisplay::VERBATIM,
    );
    let body: Vec<String> = lines[1..].iter().map(plain).collect();
    assert_eq!(body.len(), 5, "4 tail rows + the footer: {body:?}");
    assert!(
        !body.iter().any(|l| l.contains("alpha")),
        "alpha is fully hidden above the window: {body:?}"
    );
    assert!(
        body[0].contains("beta") && body[1].contains("gamma"),
        "the window opens on the still-visible lines: {body:?}"
    );
    assert_eq!(body[2].trim(), "x".repeat(35), "the long line wraps…");
    assert_eq!(body[3].trim(), "x".repeat(35), "…across the window's rows");
    assert_eq!(
        body.last().unwrap().trim(),
        "+1 lines (7s · timeout 2m)",
        "the footer counts the one fully hidden line: {body:?}"
    );
}

#[test]
fn tool_full_lines_strips_the_exit_code_frame_from_a_bash_cell() {
    // The Ctrl+O full view shows the whole body but, like the inline cell,
    // drops the `Exit code: N` frame line (docs/tool-streaming.md).
    let out = "Exit code: 0\nalpha\nbeta";
    let lines = tool_full_lines(
        &tool("Bash", "echo", ToolStatus::Ok, out),
        80,
        &PathDisplay::VERBATIM,
    );
    let all: String = lines.iter().map(plain).collect::<Vec<_>>().join("\n");
    assert!(
        !all.contains("Exit code"),
        "the frame is hidden in the full view too: {all:?}"
    );
    assert!(
        all.contains("alpha") && all.contains("beta"),
        "the whole body shows: {all:?}"
    );
}

#[test]
fn tool_lines_single_line_output_has_no_expand_hint() {
    let lines = tool_lines(
        &tool("Bash", "echo hi", ToolStatus::Ok, "hi"),
        80,
        &PathDisplay::VERBATIM,
    );
    assert_eq!(lines.len(), 2, "header + peek only, nothing hidden");
    assert!(plain(&lines[1]).contains("hi"));
}

#[test]
fn tool_lines_running_shows_a_running_peek() {
    let lines = tool_lines(
        &tool("Bash", "sleep 1", ToolStatus::Running, ""),
        80,
        &PathDisplay::VERBATIM,
    );
    assert_eq!(lines.len(), 2);
    assert!(
        plain(&lines[1]).to_lowercase().contains("running"),
        "a running tool peeks as running: {:?}",
        plain(&lines[1])
    );
}

#[test]
fn tool_lines_running_backend_peek_reads_running_capitalized() {
    // The running `⎿` row reads `Running…` (capital, like the shell cell) so
    // the live preview under the header matches Claude-Code's look.
    let lines = tool_lines(
        &tool("Bash", "sleep 1", ToolStatus::Running, ""),
        80,
        &PathDisplay::VERBATIM,
    );
    assert!(
        plain(&lines[1]).contains("Running…"),
        "backend running peek is capitalised: {:?}",
        plain(&lines[1])
    );
}

#[test]
fn tool_lines_waiting_shows_a_waiting_peek() {
    // A not-yet-started call in a parallel batch renders `● name(args)` over
    // a dim `⎿ Waiting…` row — the queued-but-not-running state. See
    // `docs/parallel-tools.md`.
    let lines = tool_lines(
        &tool("Bash", "ping x.com", ToolStatus::Waiting, ""),
        80,
        &PathDisplay::VERBATIM,
    );
    assert_eq!(lines.len(), 2, "header + waiting peek");
    assert!(
        plain(&lines[0]).contains("Bash(ping x.com)"),
        "the waiting call still shows its header: {:?}",
        plain(&lines[0])
    );
    assert!(
        plain(&lines[1]).contains("Waiting…"),
        "a waiting call peeks as Waiting…: {:?}",
        plain(&lines[1])
    );
}

#[test]
fn tool_lines_colours_a_waiting_bullet_dim_and_still() {
    // The waiting bullet is dim grey — the same hue a running bullet rests on;
    // what tells them apart in the live region is that the running one moves
    // (`docs/tool-pulse.md`),
    // since the call hasn't started.
    let lines = tool_lines(
        &tool("Bash", "ping x.com", ToolStatus::Waiting, ""),
        80,
        &PathDisplay::VERBATIM,
    );
    let bullet = lines[0].spans.first().expect("a bullet span");
    assert_eq!(
        bullet.style.fg,
        Some(tool_waiting_color()),
        "the waiting bullet is the dim grey"
    );
}

#[test]
fn tool_header_body_is_bold_white_parens_included() {
    // The whole `(...)` header body — the command text AND its framing parens
    // — reads like a normal reply (bold + the white assistant colour), so a
    // bash command and its brackets are all noticeable rather than dim.
    let lines = tool_lines(
        &tool("Bash", "cargo test", ToolStatus::Ok, "out"),
        80,
        &PathDisplay::VERBATIM,
    );
    let arg = lines[0]
        .spans
        .iter()
        .find(|s| s.content.contains("cargo"))
        .expect("an args span");
    assert_eq!(
        arg.style.fg,
        Some(tool_args_color()),
        "args use the normal reply colour"
    );
    assert!(
        arg.style.add_modifier.contains(Modifier::BOLD),
        "args are bold"
    );
    // The opening `(` and closing `)` are the same bold white, not dim.
    let open = lines[0]
        .spans
        .iter()
        .find(|s| s.content.contains('('))
        .expect("a span carrying the open paren");
    assert_eq!(
        open.style.fg,
        Some(tool_args_color()),
        "the opening paren is bold white too, not a dim delimiter"
    );
    let close = lines[0]
        .spans
        .iter()
        .find(|s| s.content.contains(')'))
        .expect("a span carrying the close paren");
    assert_eq!(
        close.style.fg,
        Some(tool_args_color()),
        "the closing paren is bold white too"
    );
}

#[test]
fn tool_full_lines_keeps_the_whole_header_untruncated() {
    // The Ctrl+O transcript view shows the entire command — no `…)` cap —
    // however many rows it wraps to.
    let cmd = "for i in {1..5}; do echo \"=== Iteration $i ===\" \
               && echo \"Current time: $(date)\" \
               && echo \"System uptime: $(uptime)\" \
               && echo \"Memory usage: $(free -h | grep Mem)\"; done";
    let lines = tool_full_lines(
        &tool("Bash", cmd, ToolStatus::Ok, "out"),
        50,
        &PathDisplay::VERBATIM,
    );
    let header: Vec<String> = lines
        .iter()
        .take_while(|l| !plain(l).contains('⎿'))
        .map(plain)
        .collect();
    assert!(
        cols(cmd) > TOOL_HEADER_MAX_COLS,
        "the fixture is over the collapsed budget"
    );
    let joined: String = header
        .iter()
        .map(|r| r.trim())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        !joined.contains('…'),
        "no ellipsis truncation in the full view: {joined:?}"
    );
    assert!(
        joined.trim_end().ends_with(')') && joined.contains("done"),
        "keeps the command tail and closing paren: {joined:?}"
    );
}

#[test]
fn edit_tool_inline_peek_colours_the_diff_rows() {
    // An `edit` cell shows its diff inline (codex's trick): a `+` row is
    // green, a `-` row is red, the summary/context dim.
    let output = "Updated a.rs (+1 -1)\n keep\n-old\n+new";
    let lines = tool_lines(
        &tool("Edit", "a.rs", ToolStatus::Ok, output),
        80,
        &PathDisplay::VERBATIM,
    );
    assert_eq!(
        plain(&lines[0]),
        "● Edit(a.rs)",
        "keeps the coloured header"
    );
    // Rows: header, summary, ` keep`, `-old`, `+new`.
    let del = lines.iter().find(|l| plain(l).contains("-old")).unwrap();
    let add = lines.iter().find(|l| plain(l).contains("+new")).unwrap();
    // The content span (after the gutter) carries the diff colour.
    assert_eq!(
        del.spans.last().unwrap().style.fg,
        Some(tool_diff_del_color())
    );
    assert_eq!(
        add.spans.last().unwrap().style.fg,
        Some(tool_diff_add_color())
    );
}

#[test]
fn a_non_diff_tool_peek_is_not_diff_coloured() {
    // A `bash` cell whose output happens to start with `+`/`-` is NOT a diff
    // tool, so its rows render as plain (white) output, never green/red.
    let lines = tool_lines(
        &tool("Bash", "diff a b", ToolStatus::Ok, "-removed\n+added"),
        80,
        &PathDisplay::VERBATIM,
    );
    let fg = lines[1].spans.last().unwrap().style.fg;
    assert_eq!(
        fg,
        Some(tool_output_color()),
        "bash output is the plain white output colour"
    );
    assert_ne!(fg, Some(tool_diff_add_color()), "never diff-coloured");
    assert_ne!(fg, Some(tool_diff_del_color()), "never diff-coloured");
}

#[test]
fn tool_output_content_is_white_the_corner_stays_dim() {
    // A finished tool's output under the ⎿ gutter is the noticeable white
    // output colour, while the ⎿ corner glyph itself stays a dim delimiter.
    let lines = tool_lines(
        &tool("Bash", "echo hi", ToolStatus::Ok, "hello world"),
        80,
        &PathDisplay::VERBATIM,
    );
    let out = lines
        .iter()
        .find(|l| plain(l).contains("hello world"))
        .expect("an output row");
    let content = out
        .spans
        .iter()
        .find(|s| s.content.contains("hello"))
        .expect("the content span");
    assert_eq!(
        content.style.fg,
        Some(tool_output_color()),
        "output content is the white output colour"
    );
    let corner = out
        .spans
        .iter()
        .find(|s| s.content.contains('⎿'))
        .expect("the ⎿ corner span");
    assert_eq!(
        corner.style.fg,
        Some(tool_dim_color()),
        "the ⎿ corner stays a dim delimiter"
    );
}

#[test]
fn tool_running_and_waiting_placeholders_stay_dim() {
    // The `Running…`/`Waiting…` placeholders are meta, not output, so they
    // keep the dim colour even though real output is now white.
    for status in [ToolStatus::Running, ToolStatus::Waiting] {
        let lines = tool_lines(
            &tool("Bash", "sleep 1", status, ""),
            80,
            &PathDisplay::VERBATIM,
        );
        let row = lines
            .iter()
            .find(|l| {
                let p = plain(l);
                p.contains("Running…") || p.contains("Waiting…")
            })
            .expect("a placeholder row");
        let content = row.spans.last().unwrap();
        assert_eq!(
            content.style.fg,
            Some(tool_dim_color()),
            "the {status:?} placeholder stays dim"
        );
    }
}

#[test]
fn write_tool_full_view_colours_the_diff() {
    let output = "Updated a.rs (+1 -0)\n keep\n+added";
    let lines = tool_full_lines(
        &tool("Write", "a.rs", ToolStatus::Ok, output),
        80,
        &PathDisplay::VERBATIM,
    );
    let add = lines.iter().find(|l| plain(l).contains("+added")).unwrap();
    assert_eq!(
        add.spans.last().unwrap().style.fg,
        Some(tool_diff_add_color())
    );
}

#[test]
fn write_cell_shows_numbered_syntax_highlighted_rows() {
    // A `Created …` head — the pre-rename spelling old rollouts still carry —
    // keeps parsing as a numbered body (llm::tools::render_numbered_content)
    // and renders as Claude-Code's Write preview: dim right-aligned line
    // numbers, the content syntax-highlighted by the path's extension.
    let output = "Created hello.py (2 lines)\n1 def main():\n2     x = \"hi\"";
    let lines = tool_lines(
        &tool("Write", "hello.py", ToolStatus::Ok, output),
        80,
        &PathDisplay::VERBATIM,
    );
    assert_eq!(plain(&lines[0]), "● Write(hello.py)");
    assert_eq!(plain(&lines[1]), "  ⎿  Created hello.py (2 lines)");
    let row1 = &lines[2];
    assert_eq!(plain(row1), "      1 def main():");
    let num = &row1.spans[1];
    assert_eq!(num.content.as_ref(), "1 ");
    assert_eq!(num.style.fg, Some(tool_dim_color()), "line number is dim");
    let kw = row1
        .spans
        .iter()
        .find(|s| s.content.as_ref() == "def")
        .expect("keyword segment");
    assert!(kw.style.fg.is_some(), "keyword coloured");
    let s = lines[3]
        .spans
        .iter()
        .find(|s| s.content.as_ref() == "\"hi\"")
        .expect("string segment");
    assert!(s.style.fg.is_some(), "string coloured");
    assert_ne!(
        kw.style.fg, s.style.fg,
        "keyword and string are distinct colours"
    );
}

#[test]
fn write_cell_parses_the_live_wrote_head() {
    // The live executor's head (`llm::tools::write_report`): `Wrote {N} lines
    // to {path}` — the path cwd-relative — over the same unsigned numbered
    // body, rendered exactly like the legacy `Created` cells.
    let output = "Wrote 2 lines to nested/hello.py\n1 def main():\n2     x = \"hi\"";
    let lines = tool_lines(
        &tool("Write", "/repo/nested/hello.py", ToolStatus::Ok, output),
        80,
        &PathDisplay::VERBATIM,
    );
    assert_eq!(plain(&lines[0]), "● Write(/repo/nested/hello.py)");
    assert_eq!(plain(&lines[1]), "  ⎿  Wrote 2 lines to nested/hello.py");
    let row1 = &lines[2];
    assert_eq!(plain(row1), "      1 def main():");
    assert_eq!(
        row1.spans[1].style.fg,
        Some(tool_dim_color()),
        "line number is dim"
    );
    assert!(
        row1.spans
            .iter()
            .find(|s| s.content.as_ref() == "def")
            .is_some_and(|s| s.style.fg.is_some()),
        "the body stays syntax-highlighted under the new head"
    );
}

#[test]
fn file_summary_head_wraps_under_the_corner_instead_of_clipping() {
    // A long `Wrote {N} lines to {deep/../path}` head used to clip at the
    // terminal edge; it now word-wraps with continuation rows indented under
    // the corner content, so the whole path stays readable.
    let head = "Wrote 12 lines to ../../nested/chain/readme.md";
    let output = format!("{head}\n1 a\n2 b");
    let width: u16 = 40;
    let lines = tool_lines(
        &tool("Write", "/x/readme.md", ToolStatus::Ok, &output),
        width,
        &PathDisplay::VERBATIM,
    );
    let head_rows: Vec<String> = lines[1..]
        .iter()
        .map(plain)
        .take_while(|row| !row.trim_start().starts_with(|c: char| c.is_ascii_digit()))
        .collect();
    assert!(head_rows.len() > 1, "the head wrapped: {head_rows:?}");
    assert_eq!(head_rows[0], "  ⎿  Wrote 12 lines to");
    assert_eq!(
        head_rows[1], "     ../../nested/chain/readme.md",
        "the continuation indents under the corner content"
    );
    for line in &lines {
        assert!(
            cols(&plain(line)) <= width as usize,
            "every row fits the width: {:?}",
            plain(line)
        );
    }
}

#[test]
fn an_updated_head_keeps_its_count_colours_when_it_wraps() {
    // The `(+A -D)` dress survives the head wrap: wherever the counts land,
    // they stay green/red.
    let output = "Updated ../../some/long/path/chain/into/the/tree/main.rs (+3 -1)\n1 +x";
    let lines = tool_lines(
        &tool("Edit", "/x/main.rs", ToolStatus::Ok, output),
        40,
        &PathDisplay::VERBATIM,
    );
    let add = lines
        .iter()
        .flat_map(|l| l.spans.iter())
        .find(|s| s.content.as_ref() == "+3")
        .expect("the added count span survives the wrap");
    assert_eq!(add.style.fg, Some(tool_diff_add_color()));
    let del = lines
        .iter()
        .flat_map(|l| l.spans.iter())
        .find(|s| s.content.as_ref() == "-1")
        .expect("the removed count span survives the wrap");
    assert_eq!(del.style.fg, Some(tool_diff_del_color()));
}

#[test]
fn an_image_read_cell_is_one_concise_fact_row() {
    // `⎿ Read image (PNG, 512x512, 17 KB)` — the executor's whole output, so
    // the cell is the header plus exactly one output row, no hint.
    let output = "Read image (PNG, 512x512, 17 KB)";
    let lines = tool_lines(
        &tool("Read", "flower.png", ToolStatus::Ok, output),
        80,
        &PathDisplay::VERBATIM,
    );
    assert_eq!(plain(&lines[0]), "● Read(flower.png)");
    assert_eq!(plain(&lines[1]), "  ⎿  Read image (PNG, 512x512, 17 KB)");
    assert_eq!(lines.len(), 2, "no hint, no second row: {lines:?}");
}

#[test]
fn a_generic_cell_peek_line_wraps_instead_of_clipping() {
    // The single collapsed peek line (an image read's fact row, an error
    // body, an unknown tool) word-wraps to the width like the command peek —
    // the old render clipped its tail at the terminal edge. A line within the
    // per-line row budget survives whole; the pathological one is clipped and
    // marked instead (`a_generic_backend_cell_clips_its_single_peeked_line`,
    // docs/long-lines.md).
    let long = "could not read /home/u/missing/file.txt: no such file";
    let width: u16 = 40;
    let lines = tool_lines(
        &tool("Read", "file.txt", ToolStatus::Failed, long),
        width,
        &PathDisplay::VERBATIM,
    );
    let body: Vec<String> = lines[1..].iter().map(plain).collect();
    assert!(body.len() > 1, "the peek wrapped: {body:?}");
    // `wrap_output` keeps each boundary space at the end of its row, so
    // stripping the gutter and concatenating reconstructs the line exactly.
    let joined: String = body
        .iter()
        .map(|row| {
            let bare = row.trim_start_matches(' ');
            bare.strip_prefix('⎿')
                .map_or(bare, |rest| rest.trim_start_matches(' '))
                .to_string()
        })
        .collect();
    assert_eq!(joined, long, "no character of the line is lost");
    for line in &lines[1..] {
        assert!(
            cols(&plain(line)) <= width as usize,
            "every row fits the width: {:?}",
            plain(line)
        );
    }
}

#[test]
fn a_generic_cell_still_hints_the_lines_behind_the_wrapped_peek() {
    // The fold is counted in rows: a first line wrapping to three rows fills
    // it by itself, and the lines after it stay behind the accurate
    // `… +N lines` hint.
    let output = "first line of the body that is long enough to wrap to three rows at this width\nsecond\nthird";
    let lines = tool_lines(
        &tool("Teleport", "x", ToolStatus::Ok, output),
        40,
        &PathDisplay::VERBATIM,
    );
    let hint = plain(lines.last().unwrap());
    assert!(
        hint.contains("+2 lines") && hint.contains("ctrl+o"),
        "got {hint:?}"
    );
    assert!(
        lines.len() > 3,
        "the first line wrapped into several rows: {:?}",
        lines.iter().map(plain).collect::<Vec<_>>()
    );
}

#[test]
fn file_cell_uses_claude_code_gutter_spacing() {
    // Claude-Code's file-change look: the `⎿` corner is two spaces wide
    // (`  ⎿  Created…`, content at col 5), and the numbered body sits one
    // column further in so the gutter reads like Claude Code — for a
    // two-digit file line `1` lands under the corner word's 3rd letter and
    // its content under the 5th (number at col 7, content at col 9). The
    // `… +N lines` hint keeps the corner-content column (col 5).
    let body: String = (1..=12)
        .map(|i| format!("{i:>2} line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let output = format!("Created f.txt (12 lines)\n{body}");
    let lines = tool_lines(
        &tool("Write", "f.txt", ToolStatus::Ok, &output),
        80,
        &PathDisplay::VERBATIM,
    );
    assert_eq!(plain(&lines[1]), "  ⎿  Created f.txt (12 lines)");
    assert_eq!(plain(&lines[2]), "       1 line 1");
    assert_eq!(
        plain(lines.last().unwrap()),
        "     … +2 lines (ctrl+o to expand)"
    );
}

#[test]
fn read_cell_shows_numbered_syntax_highlighted_rows() {
    // A `read` cell renders like a `write`: the `Read N lines` summary on
    // the corner, then dim right-aligned line numbers with the content
    // syntax-highlighted by the path's extension — no diff sign or tint.
    let output = "1 def main():\n2     return 42";
    let lines = tool_lines(
        &tool("Read", "app.py", ToolStatus::Ok, output),
        80,
        &PathDisplay::VERBATIM,
    );
    assert_eq!(plain(&lines[0]), "● Read(app.py)");
    assert_eq!(plain(&lines[1]), "  ⎿  Read 2 lines");
    let row = &lines[2];
    assert_eq!(plain(row), "      1 def main():");
    let num = &row.spans[1];
    assert_eq!(num.content.as_ref(), "1 ");
    assert_eq!(num.style.fg, Some(tool_dim_color()), "line number is dim");
    let kw = row
        .spans
        .iter()
        .find(|s| s.content.as_ref() == "def")
        .expect("keyword segment");
    assert!(kw.style.fg.is_some(), "keyword coloured");
    let lit = lines[3]
        .spans
        .iter()
        .find(|s| s.content.as_ref() == "42")
        .expect("number literal");
    assert!(lit.style.fg.is_some(), "number coloured");
    assert_ne!(
        kw.style.fg, lit.style.fg,
        "keyword and number are distinct colours"
    );
    assert!(
        lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .all(|s| s.style.bg.is_none()),
        "a read cell carries no diff background tint"
    );
}

#[test]
fn read_cell_peek_caps_at_file_peek_lines_with_the_expand_hint() {
    let body: String = (1..=30)
        .map(|i| format!("{i:>2} row {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let lines = tool_lines(
        &tool("Read", "big.txt", ToolStatus::Ok, &body),
        80,
        &PathDisplay::VERBATIM,
    );
    // header + summary + FILE_PEEK_LINES rows + the hint.
    assert_eq!(lines.len(), 2 + FILE_PEEK_LINES + 1);
    assert!(
        plain(lines.last().unwrap())
            .contains(&format!("+{} lines{EXPAND_HINT}", 30 - FILE_PEEK_LINES))
    );
}

#[test]
fn read_cell_placeholder_output_falls_back_to_a_plain_peek() {
    // The `(file is empty)` / offset-past-end placeholders aren't numbered,
    // so the cell keeps the plain output peek (no numbering, no crash).
    let lines = tool_lines(
        &tool("Read", "x.txt", ToolStatus::Ok, "(file x.txt is empty)"),
        80,
        &PathDisplay::VERBATIM,
    );
    assert_eq!(plain(&lines[0]), "● Read(x.txt)");
    assert!(plain(&lines[1]).contains("(file x.txt is empty)"));
    assert!(
        lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .all(|s| s.style.bg.is_none())
    );
}

#[test]
fn edit_cell_shows_numbered_hunks_with_diff_tints() {
    // An `Updated …` body (llm::tools::render_numbered_diff) renders as
    // codex's diff cell: numbered rows, `+` rows on the dark-green tint,
    // `-` rows dimmed on the dark-red tint, context syntax-highlighted.
    let output = "Updated a.rs (+1 -1)\n 9  before()\n10 -let x = 1;\n10 +let x = 2;\n11  after()";
    let lines = tool_lines(
        &tool("Edit", "a.rs", ToolStatus::Ok, output),
        80,
        &PathDisplay::VERBATIM,
    );
    assert_eq!(plain(&lines[1]), "  ⎿  Updated a.rs (+1 -1)");
    let del = lines.iter().find(|l| plain(l).contains("-let")).unwrap();
    let add = lines.iter().find(|l| plain(l).contains("+let")).unwrap();
    let ctx = lines
        .iter()
        .find(|l| plain(l).contains("before()"))
        .unwrap();
    // The sign spans carry the diff colours…
    let del_sign = del
        .spans
        .iter()
        .find(|s| s.content.as_ref() == "-")
        .unwrap();
    assert_eq!(del_sign.style.fg, Some(tool_diff_del_color()));
    let add_sign = add
        .spans
        .iter()
        .find(|s| s.content.as_ref() == "+")
        .unwrap();
    assert_eq!(add_sign.style.fg, Some(tool_diff_add_color()));
    // …every span past the `⎿` indent is tinted end to end — the row's own
    // tint, or the brighter mark tint where the line actually changed
    // (`docs/inline-diff.md`; here the `1` -> `2`).
    assert!(
        del.spans
            .iter()
            .skip(1)
            .all(|s| s.style.bg == Some(tool_diff_del_bg())
                || s.style.bg == Some(tool_diff_del_mark_bg())),
        "removed row is tinted red"
    );
    assert!(
        add.spans
            .iter()
            .skip(1)
            .all(|s| s.style.bg == Some(tool_diff_add_bg())
                || s.style.bg == Some(tool_diff_add_mark_bg())),
        "added row is tinted green"
    );
    assert_eq!(
        on_bg(del, tool_diff_del_mark_bg()),
        "1",
        "only the `1` changed"
    );
    assert_eq!(
        on_bg(add, tool_diff_add_mark_bg()),
        "2",
        "only the `2` changed"
    );
    // …the added text keeps its syntax colour, the removed text is dimmed,
    // and context rows are highlighted with no tint.
    let add_kw = add
        .spans
        .iter()
        .find(|s| s.content.as_ref() == "let")
        .unwrap();
    assert!(
        add_kw.style.fg.is_some(),
        "added keyword keeps its syntax colour"
    );
    assert!(!add_kw.style.add_modifier.contains(Modifier::DIM));
    let del_kw = del
        .spans
        .iter()
        .find(|s| s.content.as_ref() == "let")
        .unwrap();
    assert!(del_kw.style.add_modifier.contains(Modifier::DIM));
    let ctx_fn = ctx
        .spans
        .iter()
        .find(|s| s.content.as_ref() == "before")
        .unwrap();
    assert!(
        ctx_fn.style.fg.is_some(),
        "context row is syntax-highlighted"
    );
    assert!(ctx.spans.iter().all(|s| s.style.bg.is_none()));
}

#[test]
fn file_cell_summary_head_is_white_not_dim() {
    // The `Created …`/`Updated …`/`Read N lines` summary head reads in the
    // white output colour (noticeable) rather than dim grey — the `(+A -D)`
    // counts still show green/red.
    let write = tool_lines(
        &tool(
            "Write",
            "f.txt",
            ToolStatus::Ok,
            "Created f.txt (2 lines)\n1 a\n2 b",
        ),
        80,
        &PathDisplay::VERBATIM,
    );
    let w_head = write[1]
        .spans
        .iter()
        .find(|s| s.content.contains("Created"))
        .expect("the Created summary span");
    assert_eq!(
        w_head.style.fg,
        Some(tool_output_color()),
        "a write summary head is white"
    );

    let read = tool_lines(
        &tool("Read", "f.txt", ToolStatus::Ok, "1 a\n2 b"),
        80,
        &PathDisplay::VERBATIM,
    );
    let r_head = read[1]
        .spans
        .iter()
        .find(|s| s.content.contains("Read"))
        .expect("the Read summary span");
    assert_eq!(
        r_head.style.fg,
        Some(tool_output_color()),
        "a read summary head is white"
    );

    let edit = tool_lines(
        &tool(
            "Edit",
            "a.rs",
            ToolStatus::Ok,
            "Updated a.rs (+1 -1)\n1 +x\n2 -y",
        ),
        80,
        &PathDisplay::VERBATIM,
    );
    let e_head = edit[1]
        .spans
        .iter()
        .find(|s| s.content.contains("Updated"))
        .expect("the Updated summary span");
    assert_eq!(
        e_head.style.fg,
        Some(tool_output_color()),
        "an edit summary path is white"
    );
    // The counts still stand out green/red.
    let plus = edit[1].spans.iter().find(|s| s.content == "+1").unwrap();
    assert_eq!(
        plus.style.fg,
        Some(tool_diff_add_color()),
        "counts stay green"
    );
}

#[test]
fn write_cell_peek_caps_at_file_peek_lines_with_the_expand_hint() {
    let body: String = (1..=30)
        .map(|i| format!("{i:>2} line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let output = format!("Created big.txt (30 lines)\n{body}");
    let lines = tool_lines(
        &tool("Write", "big.txt", ToolStatus::Ok, &output),
        80,
        &PathDisplay::VERBATIM,
    );
    // header + summary + FILE_PEEK_LINES numbered rows + the hint.
    assert_eq!(lines.len(), 2 + FILE_PEEK_LINES + 1);
    let hint = plain(lines.last().unwrap());
    assert!(
        hint.contains(&format!("+{} lines{EXPAND_HINT}", 30 - FILE_PEEK_LINES)),
        "got {hint}"
    );
}

#[test]
fn write_cell_full_view_shows_every_numbered_row() {
    let body: String = (1..=30)
        .map(|i| format!("{i:>2} line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let output = format!("Created big.txt (30 lines)\n{body}");
    let lines = tool_full_lines(
        &tool("Write", "big.txt", ToolStatus::Ok, &output),
        80,
        &PathDisplay::VERBATIM,
    );
    assert_eq!(lines.len(), 2 + 30, "header + summary + every row");
    assert!(plain(lines.last().unwrap()).contains("30 line 30"));
}

#[test]
fn a_bash_cell_with_a_created_looking_output_gets_no_diff_tint() {
    // Only Write/Edit cells opt into the numbered rendering — a bash
    // command whose output mimics the format keeps the plain output peek
    // (white content, no diff background tint).
    let lines = tool_lines(
        &tool("Bash", "gen", ToolStatus::Ok, "Created x (1 line)\n1 hi"),
        80,
        &PathDisplay::VERBATIM,
    );
    assert!(
        lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .all(|s| s.style.bg.is_none()),
        "no diff tint on a bash cell"
    );
}

#[test]
fn tool_full_lines_colours_the_header_by_status() {
    let lines = tool_full_lines(
        &tool("Read", "f", ToolStatus::Ok, "x"),
        80,
        &PathDisplay::VERBATIM,
    );
    assert_eq!(lines[0].spans[0].style.fg, Some(tool_ok_color()));
}

#[test]
fn a_backend_tools_full_output_hangs_under_the_gutter_verbatim() {
    // The expanded (Ctrl+O) view opens a backend tool's output with the same
    // `⎿` gutter as its inline peek (and as a shell cell), continuation rows
    // aligned under the corner — Claude-Code's exec-cell style — with the
    // output's own space runs preserved (`ls -l` columns must survive).
    let lines: Vec<String> = tool_full_lines(
        &tool(
            "Bash",
            "ls -l",
            ToolStatus::Ok,
            "total 8\n-rw-  1 user   42 a",
        ),
        80,
        &PathDisplay::VERBATIM,
    )
    .iter()
    .map(plain)
    .collect();
    assert_eq!(
        lines,
        vec!["● Bash(ls -l)", "  ⎿  total 8", "     -rw-  1 user   42 a"],
        "the gutter opens the body, continuation rows aligned, space runs kept"
    );
}

#[test]
fn render_live_grows_the_preview_to_fit_a_long_running_command() {
    // A running backend tool with a *long* command previews its whole cell:
    // the header wraps across rows (never clipped at the edge) AND the
    // `⎿ Running…` row sits beneath the last header row — so the live preview
    // grows past the usual two rows. Guards the header-wrap + running-row
    // composition in the real render (not just `tool_lines` in isolation).
    let cmd = "curl -s \"wttr.in/Warsaw?format=%C+%t+%w+%h\" 2>/dev/null \
               || echo \"wttr.in unavailable, trying alternative...\"";
    let mut app = App::new();
    app.begin_stream();
    app.start_tool("Bash", cmd, None);
    let width = 40;
    let pv = preview_rows(&app, width);
    assert!(
        pv > 2,
        "a wrapped header + ⎿ Running… is more than two rows: {pv}"
    );
    let h = live_height(&app.input, width, 24, true, pv, 0, 0, 0, 0, 0, 0);
    let mut buf = buffer(width, h);
    render_live(buf.area, &mut buf, &app);
    let rows: Vec<String> = (0..h).map(|y| row(&buf, y, width)).collect();
    // The header wraps: row 0 opens it, and at least one later row is an
    // indented continuation (aligned under the opening `(`), before the ⎿ row.
    assert!(
        rows[0].starts_with("● Bash("),
        "row 0 opens the header: {:?}",
        rows[0]
    );
    let running_y = rows
        .iter()
        .position(|r| r.contains('⎿') && r.contains("Running…"))
        .expect("a ⎿ Running… row is drawn");
    assert!(running_y >= 2, "the header took ≥2 rows before ⎿: {rows:?}");
    assert!(
        rows[1].starts_with(&" ".repeat(cols("● Bash"))),
        "the header's second row aligns under the (: {:?}",
        rows[1]
    );
    // Nothing clipped: no drawn row exceeds the width.
    for r in &rows {
        assert!(
            cols(r.trim_end()) <= width as usize,
            "row within width: {r:?}"
        );
    }
}

#[test]
fn a_shell_tool_renders_headerless_output_only() {
    // The Role::Shell header message is the cell's first line; the tool
    // itself contributes only the `⎿` output lines, flush below it.
    let mut t = tool("pwd", "", ToolStatus::Ok, "/home/user/alter-zero");
    t.shell = true;
    let lines = tool_lines(&t, 60, &PathDisplay::VERBATIM);
    assert_eq!(plain(&lines[0]), "  ⎿  /home/user/alter-zero");
    assert!(
        !plain(&lines[0]).contains("pwd"),
        "no `● pwd` header — the Shell message above is the header"
    );
}

#[test]
fn a_running_shell_tool_peeks_running() {
    let mut t = tool("sleep 5", "", ToolStatus::Running, "");
    t.shell = true;
    let lines = tool_lines(&t, 60, &PathDisplay::VERBATIM);
    assert_eq!(plain(&lines[0]), "  ⎿  Running…", "the mock's running cell");
}

#[test]
fn a_short_shell_output_shows_every_line_aligned_under_the_corner() {
    // Up to TOOL_PEEK_LINES lines all show, the continuation lines indented
    // to align under the first (Claude-Code exec style), no truncation hint.
    let mut t = tool(
        "ls",
        "",
        ToolStatus::Ok,
        "index.html\nscript.js\nstyles.css",
    );
    t.shell = true;
    let lines: Vec<String> = tool_lines(&t, 60, &PathDisplay::VERBATIM)
        .iter()
        .map(|l| plain(l).trim_end().to_string())
        .collect();
    assert_eq!(
        lines,
        vec!["  ⎿  index.html", "     script.js", "     styles.css"],
        "every line shown, continuation aligned under the first"
    );
}

#[test]
fn tool_peek_expands_tabs_for_display() {
    // A '\t' paints as zero cells (ratatui filters control-char
    // graphemes), gluing tab-separated fields together: `! printf
    // 'name\tsize'` showed "namesize". Tool output expands tabs on the
    // render path exactly like code blocks (expand_code_tabs); the
    // stored output stays byte-exact.
    let mut t = tool("pwd", "", ToolStatus::Ok, "name\tsize");
    t.shell = true;
    let texts: Vec<String> = tool_lines(&t, 80, &PathDisplay::VERBATIM)
        .iter()
        .map(plain)
        .collect();
    assert!(
        !texts.iter().any(|l| l.contains('\t')),
        "no raw tab reaches a painted row: {texts:?}"
    );
    let tab = " ".repeat(CODE_TAB_WIDTH);
    assert!(
        texts.iter().any(|l| l.contains(&format!("name{tab}size"))),
        "the separator survives as spaces: {texts:?}"
    );
}

#[test]
fn tool_full_lines_expand_tabs_for_display() {
    let t = tool("bash", "cat Makefile", ToolStatus::Ok, "target:\n\tcc -o x");
    let texts: Vec<String> = tool_full_lines(&t, 80, &PathDisplay::VERBATIM)
        .iter()
        .map(plain)
        .collect();
    assert!(
        !texts.iter().any(|l| l.contains('\t')),
        "no raw tab reaches the expanded view: {texts:?}"
    );
    let tab = " ".repeat(CODE_TAB_WIDTH);
    assert!(
        texts.iter().any(|l| l.contains(&format!("{tab}cc -o x"))),
        "the recipe keeps its indentation: {texts:?}"
    );
}

// --- MCP cells (docs/mcp.md) ---

use crate::ui::theme::{MCP_CALLED_PREFIX, MCP_CALLING_PREFIX, reasoning_label_color};
use crate::ui::tool::{mcp_batch_lines, tool_commit_lines};

/// An MCP cell fixture: the display name + raw-JSON args the loop records.
fn mcp_tool(status: ToolStatus, output: &str) -> ToolCall {
    tool(
        "deepwiki - ask_question (MCP)",
        r#"{"repoName":"linuztx/flaredantic","question":"What is this?"}"#,
        status,
        output,
    )
}

#[test]
fn a_running_mcp_cell_collapses_to_calling_server() {
    // One line and nothing else: the arguments (and the result) are Ctrl+O's
    // story, so the inline cell doesn't echo a peek of the question
    // (`docs/mcp.md`).
    let lines = tool_lines(
        &mcp_tool(ToolStatus::Running, ""),
        100,
        &PathDisplay::VERBATIM,
    );
    assert_eq!(lines.len(), 1);
    assert_eq!(
        plain(&lines[0]),
        format!("● {MCP_CALLING_PREFIX}Deepwiki…{EXPAND_HINT}")
    );
}

#[test]
fn a_waiting_mcp_sibling_shows_the_waiting_row() {
    let lines = tool_lines(
        &mcp_tool(ToolStatus::Waiting, ""),
        100,
        &PathDisplay::VERBATIM,
    );
    assert!(plain(&lines[0]).starts_with(&format!("● {MCP_CALLING_PREFIX}Deepwiki…")));
    assert!(plain(&lines[1]).contains("Waiting…"));
}

#[test]
fn a_resolved_mcp_cell_is_the_bullet_less_called_line() {
    // The settled thinking line's shape: no bullet, dim throughout, the
    // result text never inline (docs/mcp.md).
    let lines = tool_lines(
        &mcp_tool(ToolStatus::Ok, "{\"result\": \"long json…\"}"),
        100,
        &PathDisplay::VERBATIM,
    );
    assert_eq!(lines.len(), 1);
    assert_eq!(
        plain(&lines[0]),
        format!("{MCP_CALLED_PREFIX}Deepwiki{EXPAND_HINT}")
    );
    assert_eq!(lines[0].spans[0].style.fg, Some(reasoning_label_color()));
    assert!(
        !plain(&lines[0]).contains("long json"),
        "the result stays out of inline scrollback"
    );
}

#[test]
fn a_resolved_mcp_cell_stays_quiet_without_the_classifier_note() {
    // The reported noise: in auto mode every resolved MCP call's quiet
    // `Called {server} (ctrl+o to expand)` line grew a
    // `⎿ Allowed by auto mode classifier` row under it — doubling a cell
    // that is deliberately one line (and that a parallel run's aggregated
    // `Called Deepwiki 2 times` line never showed anyway). Inline the note
    // stays off the quiet cell; the Ctrl+O transcript keeps the record.
    let mut cell = mcp_tool(ToolStatus::Ok, "done");
    cell.approval_note = Some("Allowed by auto mode classifier".to_string());
    let texts: Vec<String> = tool_lines(&cell, 100, &PathDisplay::VERBATIM)
        .iter()
        .map(plain)
        .collect();
    assert_eq!(
        texts,
        vec![format!("{MCP_CALLED_PREFIX}Deepwiki{EXPAND_HINT}")],
        "the quiet line is the whole inline cell"
    );
    // The expanded transcript still closes with the note — the record that
    // no human approved this call survives where the full story lives.
    let full: Vec<String> = tool_full_lines(&cell, 100, &PathDisplay::VERBATIM)
        .iter()
        .map(plain)
        .collect();
    assert_eq!(
        full.last().map(String::as_str),
        Some("  ⎿  Allowed by auto mode classifier"),
        "got {full:?}"
    );
}

#[test]
fn a_failed_noted_mcp_cell_keeps_the_note() {
    // A failed MCP call falls to the loud generic cell, where the note still
    // explains why the call ran at all — only the quiet Ok line drops it.
    let mut cell = mcp_tool(ToolStatus::Failed, "server exploded");
    cell.approval_note = Some("Allowed by auto mode classifier".to_string());
    let texts: Vec<String> = tool_lines(&cell, 120, &PathDisplay::VERBATIM)
        .iter()
        .map(plain)
        .collect();
    assert_eq!(
        texts.last().map(String::as_str),
        Some("  ⎿  Allowed by auto mode classifier"),
        "got {texts:?}"
    );
}

#[test]
fn a_failed_mcp_cell_keeps_the_loud_generic_form() {
    let lines = tool_lines(
        &mcp_tool(ToolStatus::Failed, "server exploded"),
        120,
        &PathDisplay::VERBATIM,
    );
    // The full header — pretty-printed args, not the raw JSON — over the
    // error peek, red bullet.
    let head = plain(&lines[0]);
    assert!(
        head.starts_with("● Deepwiki - ask_question (MCP)("),
        "got {head:?}"
    );
    assert!(head.contains("question: \"What is this?\""), "got {head:?}");
    assert!(!head.contains("{\"repoName\""), "raw JSON never renders");
    assert_eq!(lines[0].spans[0].style.fg, Some(tool_fail_color()));
    assert!(lines.iter().any(|l| plain(l).contains("server exploded")));
}

#[test]
fn a_wide_mcp_header_wraps_to_the_bullets_own_hanging_indent() {
    // `● Deepwiki - ask_question (MCP)` is 31 columns: aligning the wrapped
    // arguments under the opening `(` would spend 40% of the terminal on
    // indent, so a header that wide falls back to the bullet's two columns
    // and the args get the whole width (`docs/mcp.md`).
    let mut cell = mcp_tool(ToolStatus::Ok, "");
    cell.args = r#"{"repoName":"linuztx/flaredantic","question":"What is this project about? What does it do, what problem does it solve, and how is it typically used?"}"#.to_string();
    let lines = tool_full_lines(&cell, 76, &PathDisplay::VERBATIM);
    assert_eq!(
        plain(&lines[0]),
        "● Deepwiki - ask_question (MCP)(repoName: \"linuztx/flaredantic\", question:"
    );
    assert_eq!(
        plain(&lines[1]),
        "  \"What is this project about? What does it do, what problem does it solve,"
    );
    assert_eq!(plain(&lines[2]), "  and how is it typically used?\")");
}

#[test]
fn a_narrow_header_still_aligns_under_its_opening_paren() {
    // The hanging indent is for headers whose name eats the width; an
    // ordinary `● Bash(…)` keeps the alignment it always had.
    let cell = tool(
        "Bash",
        "echo one two three four five six seven eight nine ten eleven twelve",
        ToolStatus::Ok,
        "",
    );
    let lines = tool_full_lines(&cell, 40, &PathDisplay::VERBATIM);
    assert_eq!(plain(&lines[0]), "● Bash(echo one two three four five six");
    assert_eq!(plain(&lines[1]), "      seven eight nine ten eleven");
}

#[test]
fn the_ctrl_o_view_shows_the_full_mcp_story() {
    let cell = mcp_tool(ToolStatus::Ok, "{\n  \"result\": \"Flaredantic is…\"\n}");
    let lines = tool_full_lines(&cell, 120, &PathDisplay::VERBATIM);
    let head = plain(&lines[0]);
    assert!(
        head.starts_with("● Deepwiki - ask_question (MCP)("),
        "got {head:?}"
    );
    assert!(head.contains("repoName: \"linuztx/flaredantic\""));
    // The complete output sits in the ⎿ gutter.
    assert!(lines.iter().any(|l| plain(l).contains("Flaredantic is…")));
}

/// An app whose live queue is the announced batch `names` (display names),
/// the front call running — the shape the strip and the permission prompt
/// render from.
fn app_calling(names: &[&str]) -> App {
    let mut app = App::new();
    let items: Vec<crate::stream::ToolCallSummary> = names
        .iter()
        .map(|name| crate::stream::ToolCallSummary {
            name: (*name).to_string(),
            args: r#"{"repoName":"linuztx/flaredantic"}"#.to_string(),
        })
        .collect();
    app.start_tool_batch(&items);
    app.start_tool(names[0], "", None);
    app
}

#[test]
fn an_all_mcp_batch_collapses_the_strip_to_one_aggregated_cell() {
    let app = app_calling(&[
        "deepwiki - ask_question (MCP)",
        "deepwiki - read_wiki_structure (MCP)",
        "plugin:context7:context7 - resolve-library-id (MCP)",
    ]);
    let lines = mcp_batch_lines(&app, Some(Duration::ZERO), 120).expect("all-MCP batch");
    // One line, no peek: the servers in call order and the batch's own count.
    assert_eq!(lines.len(), 1);
    assert_eq!(
        plain(&lines[0]),
        format!("● {MCP_CALLING_PREFIX}Deepwiki, Plugin:context7:context7 3 times…{EXPAND_HINT}")
    );
}

#[test]
fn the_aggregated_label_counts_the_whole_batch_not_what_is_left_of_it() {
    // The first of two parallel calls resolved: the strip still says `2
    // times` — the label describes the batch, not the remaining queue.
    let mut app = app_calling(&[
        "deepwiki - ask_question (MCP)",
        "deepwiki - read_wiki_structure (MCP)",
    ]);
    app.end_tool("{\"answer\":\"…\"}", true);
    let lines = mcp_batch_lines(&app, Some(Duration::ZERO), 120).expect("still an all-MCP batch");
    assert_eq!(
        plain(&lines[0]),
        format!("● {MCP_CALLING_PREFIX}Deepwiki 2 times…{EXPAND_HINT}")
    );
}

#[test]
fn a_mixed_batch_keeps_the_ordinary_per_cell_strip() {
    let app = app_calling(&["deepwiki - ask_question (MCP)", "Bash"]);
    assert!(mcp_batch_lines(&app, Some(Duration::ZERO), 120).is_none());
    // A lone MCP call aggregates to the same single line it renders anyway.
    let lone = app_calling(&["deepwiki - ask_question (MCP)"]);
    let lines = mcp_batch_lines(&lone, Some(Duration::ZERO), 120).expect("a lone MCP call");
    assert_eq!(
        plain(&lines[0]),
        format!("● {MCP_CALLING_PREFIX}Deepwiki…{EXPAND_HINT}")
    );
    // Nothing in flight is nothing to aggregate.
    assert!(mcp_batch_lines(&App::new(), Some(Duration::ZERO), 120).is_none());
}

#[test]
fn a_mixed_batchs_mcp_cell_commits_exactly_once() {
    // The user-reported duplicate: a mixed batch — one deepwiki call, one
    // bash call — printed `Called Deepwiki` twice. The MCP cell was never
    // held (its batch continued with a NON-MCP sibling, so it committed its
    // own line at its own ToolEnd), but the bash sibling's commit still
    // counted it into "the run this call ends" and re-emitted it. The hold
    // decision and the flush must agree on what was held (`docs/mcp.md`).
    let mut app = app_calling(&["deepwiki - ask_question (MCP)", "Bash"]);
    app.end_tool("{\"answer\":\"…\"}", true);
    let first = tool_commit_lines(&app.history, app.tool_queue(), 100, &PathDisplay::VERBATIM)
        .expect("a mixed batch's MCP cell is never held");
    assert_eq!(
        first.iter().map(plain).collect::<Vec<_>>(),
        vec![format!("{MCP_CALLED_PREFIX}Deepwiki{EXPAND_HINT}")]
    );
    app.start_tool("Bash", "ls", None);
    app.end_tool("ok", true);
    let second: Vec<String> =
        tool_commit_lines(&app.history, app.tool_queue(), 100, &PathDisplay::VERBATIM)
            .expect("the bash cell commits")
            .iter()
            .map(plain)
            .collect();
    assert!(
        !second.iter().any(|row| row.contains(MCP_CALLED_PREFIX)),
        "the bash commit re-emitted the already-committed MCP line: {second:?}"
    );
    assert!(second[0].starts_with("● Bash("), "{second:?}");
    // And the repaint from history shows the line exactly once.
    let repaint: Vec<String> =
        crate::ui::conversation_lines(&app.history, 100, &PathDisplay::VERBATIM)
            .iter()
            .map(plain)
            .collect();
    assert_eq!(
        repaint
            .iter()
            .filter(|row| row.contains(MCP_CALLED_PREFIX))
            .count(),
        1,
        "{repaint:?}"
    );
}

#[test]
fn a_non_mcp_sibling_never_reflushes_the_committed_run() {
    // The same disagreement, one deeper: [mcp, mcp, bash]. The two MCP calls
    // are a genuine run — the first held, the pair committing as one
    // aggregated line when the SECOND resolves (the batch's next call being
    // bash, the run is over there) — and the bash sibling's own commit must
    // then be the bash cell alone, not a re-flush of the run before it.
    let mut app = app_calling(&[
        "deepwiki - ask_question (MCP)",
        "deepwiki - read_wiki_structure (MCP)",
        "Bash",
    ]);
    app.end_tool("{\"answer\":\"…\"}", true);
    assert_eq!(
        tool_commit_lines(&app.history, app.tool_queue(), 100, &PathDisplay::VERBATIM),
        None,
        "the first call holds: the batch's next call is still MCP"
    );
    app.start_tool("deepwiki - read_wiki_structure (MCP)", "", None);
    app.end_tool("{\"topics\":[]}", true);
    let run: Vec<String> =
        tool_commit_lines(&app.history, app.tool_queue(), 100, &PathDisplay::VERBATIM)
            .expect("the MCP run ends here — the batch continues non-MCP")
            .iter()
            .map(plain)
            .collect();
    assert_eq!(
        run,
        vec![format!("{MCP_CALLED_PREFIX}Deepwiki 2 times{EXPAND_HINT}")]
    );
    app.start_tool("Bash", "ls", None);
    app.end_tool("ok", true);
    let second: Vec<String> =
        tool_commit_lines(&app.history, app.tool_queue(), 100, &PathDisplay::VERBATIM)
            .expect("the bash cell commits")
            .iter()
            .map(plain)
            .collect();
    assert!(
        !second.iter().any(|row| row.contains(MCP_CALLED_PREFIX)),
        "the bash commit re-flushed the committed run: {second:?}"
    );
    assert!(second[0].starts_with("● Bash("), "{second:?}");
}

#[test]
fn a_finished_parallel_mcp_run_commits_one_aggregated_line() {
    // Two calls of one batch resolve: the first holds its line (the run is
    // still going), the second commits both as one `Called … 2 times` line —
    // the same line the history repaint renders (`docs/mcp.md`).
    let mut app = app_calling(&[
        "deepwiki - ask_question (MCP)",
        "deepwiki - read_wiki_structure (MCP)",
    ]);
    app.end_tool("{\"answer\":\"…\"}", true);
    assert_eq!(
        tool_commit_lines(&app.history, app.tool_queue(), 100, &PathDisplay::VERBATIM),
        None,
        "the run's first call holds its commit"
    );
    app.start_tool("deepwiki - read_wiki_structure (MCP)", "", None);
    app.end_tool("{\"topics\":[]}", true);
    let lines = tool_commit_lines(&app.history, app.tool_queue(), 100, &PathDisplay::VERBATIM)
        .expect("the run committed");
    assert_eq!(lines.len(), 1);
    assert_eq!(
        plain(&lines[0]),
        format!("{MCP_CALLED_PREFIX}Deepwiki 2 times{EXPAND_HINT}")
    );
    // And the repaint from history agrees, line for line.
    let repaint = crate::ui::conversation_lines(&app.history, 100, &PathDisplay::VERBATIM);
    assert_eq!(plain(&repaint[0]), plain(&lines[0]));
    assert_eq!(repaint.len(), 2, "one line + its spacer: {repaint:?}");
}

#[test]
fn a_rebuild_mid_run_leaves_the_held_cell_to_the_strip() {
    // The bug a live run caught: the first call of a parallel batch resolves,
    // its line is held (the run isn't over) — and then the permission prompt
    // for the *second* call closes, which purge-rebuilds the screen from
    // history. Painting the held cell there wrote a `Called Deepwiki` line
    // that the run's own `Called Deepwiki 2 times` then followed. Until the
    // run ends the strip's `● Calling Deepwiki 2 times…` speaks for it, so a
    // rebuild must skip exactly the held cells (`docs/mcp.md`).
    let mut app = app_calling(&[
        "deepwiki - ask_question (MCP)",
        "deepwiki - read_wiki_structure (MCP)",
    ]);
    app.end_tool("{\"answer\":\"…\"}", true);
    assert_eq!(app.history.len(), 1, "the call is recorded…");
    let committed = crate::ui::committed_history(&app.history, app.tool_queue());
    assert!(
        committed.is_empty(),
        "…but not yet committed: {committed:?}"
    );
    assert!(crate::ui::conversation_lines(committed, 100, &PathDisplay::VERBATIM).is_empty());
    // Once the run ends, the rebuild has the whole run — as its one line.
    app.start_tool("deepwiki - read_wiki_structure (MCP)", "", None);
    app.end_tool("{\"topics\":[]}", true);
    let committed = crate::ui::committed_history(&app.history, app.tool_queue());
    assert_eq!(committed.len(), 2);
    let rebuilt: Vec<String> =
        crate::ui::conversation_lines(committed, 100, &PathDisplay::VERBATIM)
            .iter()
            .map(plain)
            .collect();
    assert_eq!(
        rebuilt,
        vec![
            format!("{MCP_CALLED_PREFIX}Deepwiki 2 times{EXPAND_HINT}"),
            String::new()
        ]
    );
}

#[test]
fn a_failed_call_still_flushes_the_run_it_ends() {
    // The batch's first call succeeded (its line held), the second failed:
    // the failure commits loudly, and the held line comes with it rather
    // than being lost.
    let mut app = app_calling(&[
        "deepwiki - ask_question (MCP)",
        "deepwiki - read_wiki_structure (MCP)",
    ]);
    app.end_tool("{\"answer\":\"…\"}", true);
    app.start_tool("deepwiki - read_wiki_structure (MCP)", "", None);
    app.end_tool("server exploded", false);
    let lines = tool_commit_lines(&app.history, app.tool_queue(), 100, &PathDisplay::VERBATIM)
        .expect("the run committed");
    let texts: Vec<String> = lines.iter().map(plain).collect();
    assert_eq!(
        texts[0],
        format!("{MCP_CALLED_PREFIX}Deepwiki{EXPAND_HINT}")
    );
    assert_eq!(texts[1], "");
    assert!(texts[2].starts_with("● Deepwiki - read_wiki_structure (MCP)("));
    assert!(texts.iter().any(|t| t.contains("server exploded")));
    let repaint: Vec<String> =
        crate::ui::conversation_lines(&app.history, 100, &PathDisplay::VERBATIM)
            .iter()
            .map(plain)
            .collect();
    assert_eq!(repaint[..texts.len()], texts[..]);
}

#[test]
fn two_sequential_mcp_calls_are_not_one_parallel_run() {
    // Adjacent in history, but each its own round: they were never parallel,
    // so they keep their own lines (the batch id is what tells them apart).
    let mut app = App::new();
    for _ in 0..2 {
        app.start_tool("deepwiki - ask_question (MCP)", "{}", None);
        app.end_tool("{}", true);
    }
    let repaint: Vec<String> =
        crate::ui::conversation_lines(&app.history, 100, &PathDisplay::VERBATIM)
            .iter()
            .map(plain)
            .collect();
    let called = format!("{MCP_CALLED_PREFIX}Deepwiki{EXPAND_HINT}");
    assert_eq!(
        repaint,
        vec![called.clone(), String::new(), called, String::new()]
    );
}

#[test]
fn a_narrow_terminal_drops_the_mcp_hint_before_the_label() {
    let lines = tool_lines(
        &mcp_tool(ToolStatus::Running, ""),
        20,
        &PathDisplay::VERBATIM,
    );
    let head = plain(&lines[0]);
    assert!(head.starts_with("● Calling Deepwiki…"), "got {head:?}");
    assert!(!head.contains("ctrl+o"), "no room for the hint at 20 cols");
}

// --- very long output lines: bounded rows, honest counts (docs/long-lines.md) ---

#[test]
fn a_finished_peek_hint_counts_rows_hidden_inside_a_long_line() {
    // The example verbatim: a short line, then a huge one. The old hint said
    // `+1 lines` while hiding 15 rows of JSON — the number the user reads is
    // what expanding actually adds: the blob's 18 rows less the two the fold
    // had room for after the short line.
    let out = format!("/home/u/.local/bin/yt-dlp\n{}", "x".repeat(600));
    let lines: Vec<String> = tool_lines(
        &tool("Bash", "curl -s …", ToolStatus::Ok, &out),
        40,
        &PathDisplay::VERBATIM,
    )
    .iter()
    .map(plain)
    .collect();
    assert_eq!(
        lines.len(),
        1 + TOOL_FOLD_ROWS + 1,
        "header + the short line + two rows of the blob + hint: {lines:?}"
    );
    assert!(
        lines.last().unwrap().contains("+16 lines"),
        "18 wrapped rows less the 2 shown: {lines:?}"
    );
}

#[test]
fn the_fold_spends_its_rows_on_a_blob_and_counts_the_lines_after_it() {
    // A blob first: the fold's three rows are all blob (Claude Code shows the
    // first three rows of whatever the output is), and the hint counts the
    // blob's remaining rows plus every line under it.
    let out = format!("{}\nbee\nsea", "x".repeat(600));
    let lines: Vec<String> = tool_lines(
        &tool("Bash", "cat log", ToolStatus::Ok, &out),
        40,
        &PathDisplay::VERBATIM,
    )
    .iter()
    .map(plain)
    .collect();
    assert_eq!(
        lines.len(),
        1 + TOOL_FOLD_ROWS + 1,
        "header + three blob rows + hint: {lines:?}"
    );
    assert!(
        lines.last().unwrap().contains("+17 lines"),
        "the blob's 15 hidden rows plus `bee` and `sea`: {lines:?}"
    );
}

#[test]
fn a_finished_peek_hint_counts_whole_hidden_lines_in_rows_too() {
    // Past the fold the hint counts the hidden lines' ROWS: three hidden
    // lines, one of which wraps to three rows, reads `+5 lines`.
    let out = format!("a\nb\nc\nd\ne\n{}", "x".repeat(100)); // 100 cols → 3 rows
    let lines: Vec<String> = tool_lines(
        &tool("Bash", "cat log", ToolStatus::Ok, &out),
        40,
        &PathDisplay::VERBATIM,
    )
    .iter()
    .map(plain)
    .collect();
    assert!(
        lines.last().unwrap().contains("+5 lines"),
        "`d`, `e` (1 row each) + the 3-row line: {lines:?}"
    );
}

#[test]
fn everyday_short_output_counts_the_same_as_before() {
    // Rows and source lines are the same number when nothing wraps — the
    // change is invisible for ordinary output.
    let lines: Vec<String> = tool_lines(
        &tool("Bash", "seq 6", ToolStatus::Ok, "l1\nl2\nl3\nl4\nl5\nl6"),
        80,
        &PathDisplay::VERBATIM,
    )
    .iter()
    .map(plain)
    .collect();
    assert!(lines.last().unwrap().contains("+3 lines"), "{lines:?}");
}

#[test]
fn a_shell_cell_shows_a_pathological_line_whole() {
    // The headerless `!` exec cell is not folded: a 600-column line wraps to
    // every row it needs (35 content columns at width 40 → 18 rows) and the
    // last row carries the tail of the line, never a hint.
    let mut t = tool("curl -s api", "", ToolStatus::Ok, &"x".repeat(600));
    t.shell = true;
    let lines: Vec<String> = tool_lines(&t, 40, &PathDisplay::VERBATIM)
        .iter()
        .map(plain)
        .collect();
    assert_eq!(lines.len(), 18, "every wrapped row shows: {lines:?}");
    assert_eq!(
        lines.last().unwrap().trim(),
        "x".repeat(600 - 17 * 35),
        "the line's tail, not a hint: {lines:?}"
    );
}

#[test]
fn a_generic_backend_cell_folds_its_output_too() {
    // A non-command backend tool (an image read's fact line, an error body)
    // is bounded by the same fold.
    let long = "x".repeat(600);
    let lines: Vec<String> = tool_lines(
        &tool("WebFetch", "https://x", ToolStatus::Ok, &long),
        40,
        &PathDisplay::VERBATIM,
    )
    .iter()
    .map(plain)
    .collect();
    assert_eq!(
        lines.len(),
        1 + TOOL_FOLD_ROWS + 1,
        "header + the fold + hint: {lines:?}"
    );
    assert!(lines.last().unwrap().contains("+15 lines"), "{lines:?}");
}

#[test]
fn a_read_cell_clips_a_very_long_file_line() {
    // The same class of bug in the numbered file cell: its first body row was
    // exempt from the FILE_PEEK_LINES budget, so ONE minified line painted
    // dozens of rows inline. It is clipped and marked like any other line —
    // but the hint keeps counting FILE lines, the unit the gutter numbers.
    let body = format!(" 1 {}\n 2 short\n 3 tail", "x".repeat(400));
    let lines: Vec<String> = tool_lines(
        &tool("Read", "min.json", ToolStatus::Ok, &body),
        80,
        &PathDisplay::VERBATIM,
    )
    .iter()
    .map(plain)
    .collect();
    assert_eq!(
        lines.len(),
        2 + TOOL_LINE_MAX_ROWS + 2,
        "header + `Read 3 lines` + the clipped line + rows 2 and 3: {lines:?}"
    );
    assert!(
        lines[1 + TOOL_LINE_MAX_ROWS].ends_with(TOOL_LINE_ELLIPSIS),
        "the cut is marked: {lines:?}"
    );
    assert!(
        lines.iter().any(|l| l.contains("short")) && lines.iter().any(|l| l.contains("tail")),
        "the lines after it still show: {lines:?}"
    );
    assert!(
        !lines.join("\n").contains("ctrl+o to expand"),
        "every file line is represented — no hint: {lines:?}"
    );
}

#[test]
fn the_ctrl_o_view_never_clips_a_long_line() {
    // The expansion is where the whole line lives; clipping it would leave
    // the text nowhere.
    let long = "x".repeat(600);
    let lines = tool_full_lines(
        &tool("Bash", "cat big", ToolStatus::Ok, &long),
        40,
        &PathDisplay::VERBATIM,
    );
    let joined: String = lines[1..]
        .iter()
        .map(|l| plain(l).chars().skip(5).collect::<String>())
        .collect();
    assert_eq!(joined, long, "every column survives: {}", lines.len());
    assert!(
        !plain(lines.last().unwrap()).contains(TOOL_LINE_ELLIPSIS),
        "no cut marker in the expansion"
    );
}

#[test]
fn the_running_tail_footer_counts_hidden_rows() {
    // The live tail's `+N lines ({secs}s)` footer measures what scrolled off
    // the top the same way the finished cell's hint does: a 600-char line
    // above the window is 18 rows, not "1 line".
    let out = format!("{}\nw\nx\ny\nz", "x".repeat(600));
    let t = tool("Bash", "curl -s api", ToolStatus::Running, &out);
    let lines: Vec<String> = running_command_lines(
        &t,
        Duration::from_secs(3),
        Duration::ZERO,
        40,
        &PathDisplay::VERBATIM,
    )
    .iter()
    .map(plain)
    .collect();
    let footer = lines.last().unwrap();
    assert!(
        footer.contains("+18 lines (3s · timeout 2m)"),
        "the 18 wrapped rows above the window: {lines:?}"
    );
}

#[test]
fn a_write_cell_clips_a_very_long_line_under_its_summary() {
    // The `Wrote …` head keeps its row; the pathological content line under it
    // is bounded like any other, and the short line after it still shows.
    let body = format!("Wrote 2 lines to min.js\n 1 {}\n 2 ok", "z".repeat(400));
    let lines: Vec<String> = tool_lines(
        &tool("Write", "min.js", ToolStatus::Ok, &body),
        80,
        &PathDisplay::VERBATIM,
    )
    .iter()
    .map(plain)
    .collect();
    assert_eq!(
        lines.len(),
        2 + TOOL_LINE_MAX_ROWS + 1,
        "header + `Wrote …` + the clipped line + row 2: {lines:?}"
    );
    assert!(lines[1 + TOOL_LINE_MAX_ROWS].ends_with(TOOL_LINE_ELLIPSIS));
    assert!(lines.last().unwrap().contains("ok"), "{lines:?}");
}

#[test]
fn a_clipped_diff_row_keeps_its_tint_across_the_marker() {
    // An added row's dark-green tint pads the whole row; the `…` that marks
    // the cut sits inside it, so the band doesn't break one column early.
    let body = format!("Updated min.js (+1 -0)\n 1 +{}\n 2  ctx", "z".repeat(400));
    let lines = tool_lines(
        &tool("Edit", "min.js", ToolStatus::Ok, &body),
        80,
        &PathDisplay::VERBATIM,
    );
    let cut_row = &lines[1 + TOOL_LINE_MAX_ROWS];
    assert!(
        plain(cut_row).ends_with(TOOL_LINE_ELLIPSIS),
        "the cut is marked: {:?}",
        plain(cut_row)
    );
    let marker = cut_row.spans.last().unwrap();
    assert_eq!(
        marker.style.bg,
        Some(tool_diff_add_bg()),
        "the marker carries the added-row tint: {marker:?}"
    );
    assert_eq!(
        cols(&plain(cut_row)),
        80,
        "the tint still pads the full width: {:?}",
        plain(cut_row)
    );
}

#[test]
fn a_finished_peek_never_spends_more_rows_than_the_row_ceiling() {
    // The reported mess (`docs/long-lines.md`, "Rows, not lines"): a `curl` of
    // a web page — a short `<title>` then minified `<script>` lines — spent
    // FOUR source lines' worth of per-line budget, ten wrapped rows of noise
    // inline. The cell is bounded in the unit the user reads it in: at most
    // TOOL_FOLD_ROWS display rows over the hint, whatever shape the output has.
    let out = format!(
        "<title>World Chess Championship - Wikipedia</title>\n{}\n{}\n{}",
        "<script>".to_string() + &"a".repeat(200),
        "RLSTATE=".to_string() + &"b".repeat(200),
        "<script>".to_string() + &"c".repeat(200),
    );
    let lines: Vec<String> = tool_lines(
        &tool("Bash", "curl -s …", ToolStatus::Ok, &out),
        60,
        &PathDisplay::VERBATIM,
    )
    .iter()
    .map(plain)
    .collect();
    assert_eq!(
        lines.len(),
        1 + TOOL_FOLD_ROWS + 1,
        "header + the {TOOL_FOLD_ROWS}-row fold + hint: {lines:?}"
    );
    assert!(
        lines.last().unwrap().contains("ctrl+o to expand"),
        "the rest is behind the hint: {lines:?}"
    );
}

// --- character-level diff marking (docs/inline-diff.md) ---

/// The spans of the one rendered row containing `needle`, past the `⎿` indent.
fn diff_row<'a>(lines: &'a [Line<'a>], needle: &str) -> &'a Line<'a> {
    lines
        .iter()
        .find(|l| plain(l).contains(needle))
        .unwrap_or_else(|| panic!("no row containing {needle:?}"))
}

/// The text of every span on `line` carrying background `bg`.
fn on_bg(line: &Line, bg: Color) -> String {
    line.spans
        .iter()
        .filter(|s| s.style.bg == Some(bg))
        .map(|s| s.content.as_ref())
        .collect()
}

#[test]
fn an_edited_line_lifts_only_the_changed_characters_onto_the_bright_tint() {
    // The reported case: `Rivera` -> `Rivero`. The row still carries its muted
    // tint, but *only the letter that changed* sits on the brighter one —
    // `Bruce River` is untouched text, and marking it would point at an edit
    // that never happened.
    let output = "Updated hello.txt (+1 -1)\n1 -Bruce Rivera\n1 +Bruce Rivero";
    let lines = tool_lines(
        &tool("Edit", "hello.txt", ToolStatus::Ok, output),
        80,
        &PathDisplay::VERBATIM,
    );
    let del = diff_row(&lines, "Rivera");
    let add = diff_row(&lines, "Rivero");

    assert_eq!(on_bg(del, tool_diff_del_mark_bg()), "a");
    assert_eq!(on_bg(add, tool_diff_add_mark_bg()), "o");
    // Everything the edit did not touch keeps the plain row tint — the two
    // tints together are what say "this line changed, and *here*".
    assert!(on_bg(del, tool_diff_del_bg()).contains("Bruce River"));
    assert!(on_bg(add, tool_diff_add_bg()).contains("Bruce River"));
}

#[test]
fn the_changed_run_is_bold_and_the_removed_one_escapes_the_row_dim() {
    // A dimmed highlight would defeat its own purpose: the removed row dims
    // its *unchanged* text (codex's look) and leaves the changed run bright.
    let output = "Updated hello.txt (+1 -1)\n1 -Bruce Rivera\n1 +Bruce Rivero";
    let lines = tool_lines(
        &tool("Edit", "hello.txt", ToolStatus::Ok, output),
        80,
        &PathDisplay::VERBATIM,
    );

    for (needle, mark_bg) in [
        ("Rivera", tool_diff_del_mark_bg()),
        ("Rivero", tool_diff_add_mark_bg()),
    ] {
        let row = diff_row(&lines, needle);
        let marked: Vec<_> = row
            .spans
            .iter()
            .filter(|s| s.style.bg == Some(mark_bg))
            .collect();
        assert!(!marked.is_empty(), "{needle} is marked");
        assert!(
            marked
                .iter()
                .all(|s| s.style.add_modifier.contains(Modifier::BOLD)),
            "{needle} renders bold"
        );
        assert!(
            marked
                .iter()
                .all(|s| !s.style.add_modifier.contains(Modifier::DIM)),
            "{needle} is never dimmed — it is the thing to find"
        );
    }
    // …while the removed row's unchanged *content* still dims (the gutter
    // number and the `-` sign are chrome, and were never dimmed).
    let del = diff_row(&lines, "Rivera");
    let unchanged = del
        .spans
        .iter()
        .find(|s| s.content.contains("Bruce"))
        .expect("the unchanged half renders as its own span");
    assert_eq!(unchanged.style.bg, Some(tool_diff_del_bg()));
    assert!(
        unchanged.style.add_modifier.contains(Modifier::DIM),
        "the removed row's unchanged text keeps codex's dim"
    );
}

#[test]
fn a_wholly_replaced_line_keeps_the_flat_row_tint() {
    // No refinement when the two lines aren't related — the cell renders
    // exactly as it did before this feature.
    let output = "Updated a.py (+1 -1)\n1 -import os\n1 +def main(argv, env):";
    let lines = tool_lines(
        &tool("Edit", "a.py", ToolStatus::Ok, output),
        80,
        &PathDisplay::VERBATIM,
    );
    let del = diff_row(&lines, "import os");
    let add = diff_row(&lines, "def main");
    assert_eq!(on_bg(del, tool_diff_del_mark_bg()), "");
    assert_eq!(on_bg(add, tool_diff_add_mark_bg()), "");
    assert!(
        del.spans
            .iter()
            .skip(1)
            .all(|s| s.style.bg == Some(tool_diff_del_bg()))
    );
    assert!(
        add.spans
            .iter()
            .skip(1)
            .all(|s| s.style.bg == Some(tool_diff_add_bg()))
    );
}

#[test]
fn context_and_write_rows_never_carry_a_mark_tint() {
    // A `Wrote …` body is brand-new content: no pairs, nothing to refine.
    let output = "Wrote 2 lines to a.txt\n1 alpha\n2 beta";
    let lines = tool_lines(
        &tool("Write", "a.txt", ToolStatus::Ok, output),
        80,
        &PathDisplay::VERBATIM,
    );
    assert!(
        lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .all(|s| s.style.bg != Some(tool_diff_add_mark_bg())
                && s.style.bg != Some(tool_diff_del_mark_bg()))
    );
}

#[test]
fn the_mark_tint_survives_a_wrap_onto_the_continuation_row() {
    // A changed run too long for one display row keeps its tint on both, so a
    // long line's highlight isn't silently lost at the wrap point.
    let common = "value common_filler_text_that_is_quite_long_here";
    let output = format!(
        "Updated a.txt (+1 -1)\n1 -{common} {}\n1 +{common} {}",
        "o".repeat(50),
        "n".repeat(50)
    );
    // The Ctrl+O expansion, so the assertion is about the wrap and not about
    // the collapsed cell's per-line row budget (`docs/long-lines.md`).
    let lines = tool_full_lines(
        &tool("Edit", "a.txt", ToolStatus::Ok, &output),
        40,
        &PathDisplay::VERBATIM,
    );
    let rows: Vec<&Line> = lines
        .iter()
        .filter(|l| {
            l.spans
                .iter()
                .any(|s| s.style.bg == Some(tool_diff_add_mark_bg()))
        })
        .collect();
    assert!(rows.len() >= 2, "the changed run wraps onto a second row");
    let tinted: String = rows
        .iter()
        .flat_map(|l| l.spans.iter())
        .filter(|s| s.style.bg == Some(tool_diff_add_mark_bg()))
        .map(|s| s.content.as_ref())
        .collect();
    assert_eq!(tinted, "n".repeat(50));
}

#[test]
fn the_trailing_pad_keeps_the_row_tint_not_the_mark_tint() {
    // The bright block must end where the changed text ends — otherwise it
    // bleeds to the terminal's right edge and stops meaning "here".
    let output = "Updated hello.txt (+1 -1)\n1 -Bruce Rivera\n1 +Bruce Rivero";
    let lines = tool_lines(
        &tool("Edit", "hello.txt", ToolStatus::Ok, output),
        80,
        &PathDisplay::VERBATIM,
    );
    let add = diff_row(&lines, "Rivero");
    let last = add.spans.last().unwrap();
    assert!(last.content.ends_with(' '), "the row pads to full width");
    assert_eq!(last.style.bg, Some(tool_diff_add_bg()));
}

// --- the header keeps the command's own spacing (docs/tools.md) ---

#[test]
fn tool_header_keeps_the_commands_space_runs() {
    // A quoted run of spaces is part of the command: `echo "a    b"` must
    // not read as `echo "a b"` — the permission prompt already shows the
    // command byte-exact, so the cell it becomes has to agree with it.
    let lines = tool_lines(
        &tool("Bash", "echo \"a    b\"", ToolStatus::Ok, "out"),
        80,
        &PathDisplay::VERBATIM,
    );
    assert_eq!(plain(&lines[0]), "● Bash(echo \"a    b\")");
}

#[test]
fn tool_header_renders_a_multiline_command_line_by_line() {
    // A newline is a statement boundary in a shell script: `cd foo\nls -la`
    // flattened to `cd foo ls -la` reads as a different command. Each line
    // of the command takes a header row of its own, aligned under the `(`,
    // the closing paren riding the last one (Claude Code's multi-line
    // `Bash(…)` header).
    let lines = tool_lines(
        &tool("Bash", "cd foo\nls -la", ToolStatus::Ok, "out"),
        80,
        &PathDisplay::VERBATIM,
    );
    assert_eq!(plain(&lines[0]), "● Bash(cd foo");
    assert_eq!(plain(&lines[1]), "      ls -la)");
    assert!(
        plain(&lines[2]).contains('⎿'),
        "the output follows the header"
    );
}

#[test]
fn tool_header_spills_a_first_word_that_fits_the_continuation_row_whole() {
    // At 40 columns `● Deepwiki - ask_question (MCP)(` leaves eight columns on
    // its row: `repoName:` used to be hard-broken across the rows as
    // `repoName` / `: "linuztx/…"`. A first word that fits a continuation row
    // moves down whole; the `(` stays with the name, so the header reads as
    // a call whose arguments spill onto the next row.
    let call = tool(
        "deepwiki - ask_question (MCP)",
        r#"{"repoName":"linuztx/flaredantic","question":"What is it?"}"#,
        ToolStatus::Failed,
        "err",
    );
    let lines = tool_lines(&call, 40, &PathDisplay::VERBATIM);
    assert_eq!(plain(&lines[0]), "● Deepwiki - ask_question (MCP)(");
    assert!(
        plain(&lines[1]).starts_with("  repoName: \"linuztx/flaredantic\","),
        "the first argument moves down whole: {:?}",
        plain(&lines[1])
    );
    for l in &lines {
        assert!(
            cols(&plain(l)) <= 40,
            "row stays within the width: {:?}",
            plain(l)
        );
    }
}

#[test]
fn a_spilled_header_still_shows_the_whole_call_within_its_budget() {
    // The cut is a budget on the text, not on rows: a spilled header (nothing
    // fits beside a long name, so row 0 carries only the name and its `(`)
    // still shows every argument row of a call within TOOL_HEADER_MAX_COLS —
    // the readable spill never costs the content it was meant to make
    // readable.
    let call = tool(
        "deepwiki - ask_question (MCP)",
        r#"{"repoName":"linuztx/flaredantic","question":"How does the tunnel client work?"}"#,
        ToolStatus::Failed,
        "err",
    );
    let lines = tool_lines(&call, 40, &PathDisplay::VERBATIM);
    let header: Vec<String> = lines
        .iter()
        .map(plain)
        .take_while(|row| !row.contains('⎿'))
        .collect();
    assert_eq!(header[0], "● Deepwiki - ask_question (MCP)(");
    assert!(
        header.len() >= 3,
        "the name's own row over the wrapped argument rows: {header:?}"
    );
    let joined = header.join("");
    assert!(
        joined.ends_with("work?\")") && !joined.contains(TOOL_HEADER_ELLIPSIS),
        "the whole call fits its budget now: {joined:?}"
    );
}

#[test]
fn tool_header_truncation_never_leaves_a_space_before_the_ellipsis() {
    // The `…)` cut lands wherever the row budget runs out — which can be
    // right after a space. The marker attaches to the last kept word.
    let cmd = "word ".repeat(100);
    for width in 20..60u16 {
        let lines = tool_lines(
            &tool("Bash", cmd.trim_end(), ToolStatus::Ok, "out"),
            width,
            &PathDisplay::VERBATIM,
        );
        let cut = lines
            .iter()
            .map(plain)
            .find(|row| row.contains(&format!("{TOOL_HEADER_ELLIPSIS})")))
            .unwrap_or_else(|| panic!("a capped header at {width}"));
        assert!(
            !cut.contains(&format!(" {TOOL_HEADER_ELLIPSIS}")),
            "no space before the ellipsis at {width}: {cut:?}"
        );
        assert!(cols(&cut) <= usize::from(width), "fits at {width}: {cut:?}");
    }
}

#[test]
fn tool_header_expands_tabs_in_a_command_for_display() {
    // A tab paints as zero cells (ratatui drops control characters), which
    // would glue `cut` to `-f1` on screen; the header expands it like the
    // code-block and output paths do, byte-exact in the record.
    let lines = tool_lines(
        &tool("Bash", "cut\t-f1", ToolStatus::Ok, "out"),
        80,
        &PathDisplay::VERBATIM,
    );
    assert_eq!(plain(&lines[0]), "● Bash(cut    -f1)");
}

// --- the path a file tool's header shows (docs/tools.md "Path display") ---

/// The worked example's session: launched in `~/Codes/tests`, home
/// `/home/linuztx`. The **header** shortens through the policy; the corner
/// head is the executor's own record (`tools::display_path` — `hello.py`
/// under the cwd, a `../../hello.py` climb outside it) and is never rewritten.
fn session_paths() -> PathDisplay {
    PathDisplay::new(
        "/home/linuztx/Codes/tests",
        Some(std::path::PathBuf::from("/home/linuztx")),
    )
}

#[test]
fn a_file_tool_header_shows_the_path_relative_to_the_cwd() {
    let paths = session_paths();
    // The executor's head for a file under the cwd is already relative.
    let output = "Wrote 1 line to hello.py\n1 print(\"Hello, world!\")";
    let cell = tool(
        "Write",
        "/home/linuztx/Codes/tests/hello.py",
        ToolStatus::Ok,
        output,
    );
    let lines = tool_lines(&cell, 80, &paths);
    assert_eq!(plain(&lines[0]), "● Write(hello.py)");
    assert_eq!(plain(&lines[1]), "  ⎿  Wrote 1 line to hello.py");
    assert_eq!(plain(&lines[2]), "      1 print(\"Hello, world!\")");
    let output = "Wrote 1 line to hello/hello.py\n1 print()";
    let nested = tool(
        "Write",
        "/home/linuztx/Codes/tests/hello/hello.py",
        ToolStatus::Ok,
        output,
    );
    let lines = tool_lines(&nested, 80, &paths);
    assert_eq!(plain(&lines[0]), "● Write(hello/hello.py)");
    assert_eq!(plain(&lines[1]), "  ⎿  Wrote 1 line to hello/hello.py");
}

#[test]
fn a_file_under_home_but_outside_the_cwd_shows_tilde_relative() {
    // The header reads `~/hello.py`; the corner head keeps the executor's
    // `../../hello.py` climb exactly as recorded — the reference look.
    let paths = session_paths();
    let output = "Wrote 1 line to ../../hello.py\n1 print(\"Hello, World!\")";
    let cell = tool("Write", "/home/linuztx/hello.py", ToolStatus::Ok, output);
    let lines = tool_lines(&cell, 80, &paths);
    assert_eq!(plain(&lines[0]), "● Write(~/hello.py)");
    assert_eq!(plain(&lines[1]), "  ⎿  Wrote 1 line to ../../hello.py");
    assert_eq!(plain(&lines[2]), "      1 print(\"Hello, World!\")");
}

#[test]
fn a_file_outside_home_keeps_its_absolute_path() {
    let paths = session_paths();
    let output = "Updated ../../../../tmp/notes.txt (+1 -1)\n1 -a\n1 +b";
    let cell = tool("Edit", "/tmp/notes.txt", ToolStatus::Ok, output);
    let lines = tool_lines(&cell, 80, &paths);
    assert_eq!(plain(&lines[0]), "● Edit(/tmp/notes.txt)");
    assert_eq!(
        plain(&lines[1]),
        "  ⎿  Updated ../../../../tmp/notes.txt (+1 -1)"
    );
}

#[test]
fn the_read_header_shortens_and_its_synthesized_head_is_unchanged() {
    let paths = session_paths();
    let cell = tool(
        "Read",
        "/home/linuztx/Codes/tests/src/app.rs",
        ToolStatus::Ok,
        "1 fn main() {}\n2 // end",
    );
    let lines = tool_lines(&cell, 80, &paths);
    assert_eq!(plain(&lines[0]), "● Read(src/app.rs)");
    assert_eq!(plain(&lines[1]), "  ⎿  Read 2 lines");
}

#[test]
fn the_edit_head_keeps_its_counts_coloured_beside_a_shortened_header() {
    let paths = session_paths();
    let output = "Updated a.rs (+1 -1)\n1 -old\n1 +new";
    let cell = tool(
        "Edit",
        "/home/linuztx/Codes/tests/a.rs",
        ToolStatus::Ok,
        output,
    );
    let lines = tool_lines(&cell, 80, &paths);
    assert_eq!(plain(&lines[0]), "● Edit(a.rs)");
    assert_eq!(plain(&lines[1]), "  ⎿  Updated a.rs (+1 -1)");
    let head = &lines[1];
    assert!(
        head.spans
            .iter()
            .any(|s| s.content.as_ref() == "+1" && s.style.fg == Some(tool_diff_add_color())),
        "the added count stays green: {:?}",
        head.spans
    );
    assert!(
        head.spans
            .iter()
            .any(|s| s.content.as_ref() == "-1" && s.style.fg == Some(tool_diff_del_color())),
        "the removed count stays red: {:?}",
        head.spans
    );
}

#[test]
fn the_corner_head_is_the_record_and_is_never_rewritten() {
    // Whatever the executor put in the head stays — an absolute path in a
    // hand-written or old record, the legacy `Created` head, a failure body.
    // Only the header derives from the policy.
    let paths = session_paths();
    let output = "Wrote 1 line to /home/linuztx/hello.py\n1 print()";
    let cell = tool("Write", "/home/linuztx/hello.py", ToolStatus::Ok, output);
    let lines = tool_lines(&cell, 80, &paths);
    assert_eq!(plain(&lines[0]), "● Write(~/hello.py)");
    assert_eq!(
        plain(&lines[1]),
        "  ⎿  Wrote 1 line to /home/linuztx/hello.py"
    );
    let output = "Created /home/linuztx/x.py (1 lines)\n1 print()";
    let cell = tool("Write", "/home/linuztx/x.py", ToolStatus::Ok, output);
    let lines = tool_lines(&cell, 80, &paths);
    assert_eq!(plain(&lines[0]), "● Write(~/x.py)");
    assert_eq!(
        plain(&lines[1]),
        "  ⎿  Created /home/linuztx/x.py (1 lines)"
    );
    let output = "could not write /home/linuztx/x.py: permission denied";
    let cell = tool("Write", "/home/linuztx/x.py", ToolStatus::Failed, output);
    let lines = tool_lines(&cell, 80, &paths);
    assert_eq!(plain(&lines[0]), "● Write(~/x.py)");
    assert!(
        plain(&lines[1]).contains("could not write /home/linuztx/x.py"),
        "the body is the record: {:?}",
        plain(&lines[1])
    );
}

#[test]
fn the_transcript_expansion_shortens_the_header_alike() {
    let paths = session_paths();
    let output = "Wrote 1 line to ../../hello.py\n1 print()";
    let cell = tool("Write", "/home/linuztx/hello.py", ToolStatus::Ok, output);
    let lines = tool_full_lines(&cell, 80, &paths);
    assert_eq!(plain(&lines[0]), "● Write(~/hello.py)");
    assert_eq!(plain(&lines[1]), "  ⎿  Wrote 1 line to ../../hello.py");
}

#[test]
fn only_a_file_tools_arguments_are_shortened() {
    // A `bash` command is not a path, however many it embeds; an agent's
    // description is prose. Only `Read`/`Write`/`Edit` name a file.
    let paths = session_paths();
    let cmd = "python3 /home/linuztx/Codes/tests/hello.py";
    let cell = tool("Bash", cmd, ToolStatus::Ok, "Exit code: 0\nhi");
    assert_eq!(
        plain(&tool_lines(&cell, 80, &paths)[0]),
        format!("● Bash({cmd})")
    );
    let cell = tool(
        "Agent",
        "/home/linuztx/Codes/tests/x",
        ToolStatus::Ok,
        "done",
    );
    assert_eq!(
        plain(&tool_lines(&cell, 80, &paths)[0]),
        "● Agent(/home/linuztx/Codes/tests/x)"
    );
}

#[test]
fn the_verbatim_policy_renders_the_recorded_path_exactly() {
    // The unit-test default (and a session whose cwd is unreadable): nothing
    // shortens, so every other test in this file describes the rows it
    // always did.
    let output = "Wrote 1 line to ../../hello.py\n1 print()";
    let cell = tool("Write", "/home/linuztx/hello.py", ToolStatus::Ok, output);
    let lines = tool_lines(&cell, 80, &PathDisplay::VERBATIM);
    assert_eq!(plain(&lines[0]), "● Write(/home/linuztx/hello.py)");
    assert_eq!(plain(&lines[1]), "  ⎿  Wrote 1 line to ../../hello.py");
}

#[test]
fn the_live_cell_shortens_the_header_too() {
    // The strip's live cell — a `Write` waiting on the permission gate wears
    // the header it will commit with (`docs/permissions.md`).
    let paths = session_paths();
    let cell = tool("Write", "/home/linuztx/hello.py", ToolStatus::Running, "");
    let lines = live_tool_lines(&cell, 80, Duration::ZERO, &paths);
    assert_eq!(plain(&lines[0]), "● Write(~/hello.py)");
}

#[test]
fn the_commit_and_the_rebuild_shorten_alike() {
    // The scrollback commit and a resize's rebuild come from the same
    // history through the same policy, so the two can never disagree.
    let paths = session_paths();
    let output = "Wrote 1 line to ../../hello.py\n1 print()";
    let history = vec![HistoryItem::Tool(tool(
        "Write",
        "/home/linuztx/hello.py",
        ToolStatus::Ok,
        output,
    ))];
    let committed =
        tool_commit_lines(&history, &VecDeque::new(), 80, &paths).expect("a resolved cell commits");
    assert_eq!(plain(&committed[0]), "● Write(~/hello.py)");
    assert_eq!(plain(&committed[1]), "  ⎿  Wrote 1 line to ../../hello.py");
    let rebuilt = conversation_lines(&history, 80, &paths);
    assert_eq!(plain(&rebuilt[0]), "● Write(~/hello.py)");
    assert_eq!(plain(&rebuilt[1]), "  ⎿  Wrote 1 line to ../../hello.py");
}

#[test]
fn the_transcript_reads_the_apps_policy() {
    // The Ctrl+O transcript renders through `App`, whose policy the boundary
    // injects once at startup (`App::set_path_display`).
    let mut app = App::new();
    app.set_path_display(session_paths());
    app.history.push(HistoryItem::Tool(tool(
        "Read",
        "/home/linuztx/hello.py",
        ToolStatus::Ok,
        "1 x",
    )));
    let lines: Vec<String> = transcript_lines(&app, 80).iter().map(plain).collect();
    assert!(
        lines.iter().any(|l| l == "● Read(~/hello.py)"),
        "the transcript shortens the header: {lines:?}"
    );
    assert!(
        !lines.iter().any(|l| l.contains("/home/linuztx")),
        "…and shows the absolute path nowhere: {lines:?}"
    );
}

// --- the header wraps and cuts like Claude Code's `Bash(…)` (docs/tools.md) ---

/// The command from the report, byte for byte.
const WIKI_CURL: &str = "curl -s --max-time 15 \"https://en.wikipedia.org/w/api.php?action=query&prop=extracts&exintro&explaintext&format=json&titles=World%20Chess%20Championship%202026\" | head -c 2000";

/// The `● name(args)` rows of a rendered cell — everything above the `⎿`.
fn header_rows(lines: &[Line]) -> Vec<String> {
    lines
        .iter()
        .take_while(|l| !plain(l).contains('⎿'))
        .map(plain)
        .collect()
}

/// A header's rows joined back into one string, the continuation indent
/// dropped — the text the header showed, in order.
fn header_text(lines: &[Line]) -> String {
    header_rows(lines)
        .iter()
        .map(|row| row.trim_start())
        .collect::<Vec<_>>()
        .concat()
}

#[test]
fn the_header_fills_every_row_and_cuts_at_160_columns_like_claude_code() {
    // The report, at the 72 columns it was captured in. Claude Code shows
    //
    //   ● Bash(curl -s --max-time 15 "https://en.wikipedia.org/w/api.php?action=
    //         query&prop=extracts&exintro&explaintext&format=json&titles=World%2
    //         0Chess%20Championship%202026"…)
    //
    // — every row filled to the edge, the command cut after 160 characters.
    // Ours left the first row at `curl -s --max-time 15` because the URL
    // token, which fits no row, was moved down to the next one before being
    // broken, and then cut the command at a row budget instead.
    let lines = tool_lines(
        &tool("Bash", WIKI_CURL, ToolStatus::Ok, "{}"),
        72,
        &PathDisplay::VERBATIM,
    );
    assert_eq!(
        header_rows(&lines),
        vec![
            "● Bash(curl -s --max-time 15 \"https://en.wikipedia.org/w/api.php?action=",
            "      query&prop=extracts&exintro&explaintext&format=json&titles=World%2",
            "      0Chess%20Championship%202026\"…)",
        ]
    );
}

#[test]
fn the_collapsed_header_cuts_a_long_command_at_160_columns_not_at_a_row_count() {
    // The cut is a budget on the text (Claude Code's 160 characters), not on
    // rows: the same command is cut at the same character in every width.
    let cmd: String = (0..200u8).map(|i| char::from(b'a' + (i % 26))).collect();
    let expected = format!(
        "● Bash({}{TOOL_HEADER_ELLIPSIS})",
        &cmd[..TOOL_HEADER_MAX_COLS]
    );
    for width in [40u16, 72, 120, 200] {
        let lines = tool_lines(
            &tool("Bash", &cmd, ToolStatus::Ok, "x"),
            width,
            &PathDisplay::VERBATIM,
        );
        assert_eq!(header_text(&lines), expected, "at {width} columns");
        for l in &lines {
            assert!(cols(&plain(l)) <= usize::from(width), "fits {width}: {l:?}");
        }
    }
    // The Ctrl+O transcript shows the whole command.
    let full = tool_full_lines(
        &tool("Bash", &cmd, ToolStatus::Ok, "x"),
        72,
        &PathDisplay::VERBATIM,
    );
    assert_eq!(header_text(&full), format!("● Bash({cmd})"));
}

#[test]
fn a_command_within_the_budget_is_never_cut() {
    // Exactly 160 columns: shown whole, no marker.
    let cmd = "x".repeat(TOOL_HEADER_MAX_COLS);
    let lines = tool_lines(
        &tool("Bash", &cmd, ToolStatus::Ok, "x"),
        80,
        &PathDisplay::VERBATIM,
    );
    assert_eq!(header_text(&lines), format!("● Bash({cmd})"));
}

#[test]
fn the_collapsed_header_keeps_two_lines_of_a_multiline_command() {
    // Claude Code keeps the first two lines of a multi-line command and
    // marks the cut; the transcript shows every line.
    let lines = tool_lines(
        &tool("Bash", "cd foo\nls -la\npwd", ToolStatus::Ok, "x"),
        80,
        &PathDisplay::VERBATIM,
    );
    assert_eq!(header_rows(&lines), vec!["● Bash(cd foo", "      ls -la…)"]);
    assert_eq!(header_rows(&lines).len(), TOOL_HEADER_MAX_LINES);
    let full = tool_full_lines(
        &tool("Bash", "cd foo\nls -la\npwd", ToolStatus::Ok, "x"),
        80,
        &PathDisplay::VERBATIM,
    );
    assert_eq!(
        header_rows(&full),
        vec!["● Bash(cd foo", "      ls -la", "      pwd)"]
    );
}

#[test]
fn the_header_collapses_a_heredoc_commit_message_like_claude_code() {
    // `git commit -m "$(cat <<'EOF' … EOF)"` is how a commit message with a
    // body is passed; Claude Code's header shows it as the quoted message
    // itself — `git commit -m "Add the fold…)` — since the `cat` scaffolding
    // says nothing about what the command does.
    let cmd = "git commit -m \"$(cat <<'EOF'\nAdd the fold\n\nCo-Authored-By: X <x@y>\nEOF\n)\"";
    let lines = tool_lines(
        &tool("Bash", cmd, ToolStatus::Ok, "x"),
        80,
        &PathDisplay::VERBATIM,
    );
    assert_eq!(
        header_rows(&lines),
        vec!["● Bash(git commit -m \"Add the fold…)"]
    );
    // The transcript shows the collapsed quote whole, line by line.
    let full = header_rows(&tool_full_lines(
        &tool("Bash", cmd, ToolStatus::Ok, "x"),
        80,
        &PathDisplay::VERBATIM,
    ));
    assert_eq!(full[0], "● Bash(git commit -m \"Add the fold");
    assert_eq!(full.last().unwrap(), "      Co-Authored-By: X <x@y>\")");
    assert!(
        !full.iter().any(|r| r.contains("cat <<")),
        "the scaffolding is gone: {full:?}"
    );
}

#[test]
fn a_heredoc_that_is_not_the_commit_idiom_stays_verbatim() {
    // A plain heredoc (no `"$(cat <<'EOF'` … `)"`) is a different command
    // and is shown as written — cut to its first two lines like any other.
    let lines = tool_lines(
        &tool("Bash", "cat > f <<'EOF'\nhi\nEOF", ToolStatus::Ok, "x"),
        80,
        &PathDisplay::VERBATIM,
    );
    assert_eq!(
        header_rows(&lines),
        vec!["● Bash(cat > f <<'EOF'", "      hi…)"]
    );
}

// --- the output folds like Claude Code's tool result (docs/long-lines.md) ---

/// The report's JSON body — one compact line, as `curl` prints it.
const WIKI_JSON: &str = r#"{"batchcomplete":"","query":{"pages":{"75936044":{"pageid":75936044,"ns":0,"title":"World Chess Championship 2026"}}}}"#;

#[test]
fn an_exec_cell_pretty_prints_a_json_line_like_claude_code() {
    // The report's other half: Claude Code showed
    //
    //   ⎿  {
    //        "batchcomplete": "",
    //        "query": {
    //      … +16 lines (ctrl+o to expand)
    //
    // where ours painted the compact line wrapped. A line that is a JSON
    // document is reshaped, for display only, with two-space indentation;
    // the peek is then the first rows of *that*. The record stays byte-exact.
    let lines: Vec<String> = tool_lines(
        &tool(
            "Bash",
            "curl …",
            ToolStatus::Ok,
            &format!("Exit code: 0\n{WIKI_JSON}"),
        ),
        72,
        &PathDisplay::VERBATIM,
    )
    .iter()
    .map(plain)
    .collect();
    assert_eq!(lines[1], "  ⎿  {");
    assert_eq!(lines[2], "       \"batchcomplete\": \"\",");
    assert_eq!(lines[3], "       \"query\": {");
    // Twelve pretty rows: three shown, nine behind the hint.
    assert_eq!(lines.len(), 1 + TOOL_FOLD_ROWS + 1, "{lines:?}");
    assert!(
        lines[4].contains("+9 lines (ctrl+o to expand)"),
        "{lines:?}"
    );
    // The transcript shows the pretty document whole.
    let full: Vec<String> = tool_full_lines(
        &tool(
            "Bash",
            "curl …",
            ToolStatus::Ok,
            &format!("Exit code: 0\n{WIKI_JSON}"),
        ),
        72,
        &PathDisplay::VERBATIM,
    )
    .iter()
    .map(plain)
    .collect();
    assert_eq!(full.len(), 1 + 12, "{full:?}");
    assert_eq!(full[6], "             \"pageid\": 75936044,");
    assert_eq!(full[12], "     }");
}

#[test]
fn a_shell_cell_pretty_prints_json_too() {
    // The `!` exec cell reshapes a JSON line the same way — and, unfolded,
    // shows the whole reshaped document inline (docs/shell-command.md).
    let mut t = tool("curl -s api", "", ToolStatus::Ok, WIKI_JSON);
    t.shell = true;
    let lines: Vec<String> = tool_lines(&t, 72, &PathDisplay::VERBATIM)
        .iter()
        .map(plain)
        .collect();
    assert_eq!(lines[0], "  ⎿  {");
    assert_eq!(lines[1], "       \"batchcomplete\": \"\",");
    assert_eq!(lines.len(), 12, "the whole reshaped document: {lines:?}");
    assert_eq!(lines.last().unwrap().trim(), "}", "{lines:?}");
}

#[test]
fn only_a_line_that_round_trips_as_json_is_reshaped() {
    // Duplicate keys parse (last wins) but re-serialize differently — the
    // guard leaves such a line alone rather than showing a document the
    // command never printed. Prose and bare scalars are untouched too.
    let out = "{\"a\":1,\"a\":2}\nnot json {\n42\n{\"ok\":true}";
    let lines: Vec<String> = tool_full_lines(
        &tool("Bash", "x", ToolStatus::Ok, out),
        80,
        &PathDisplay::VERBATIM,
    )
    .iter()
    .map(plain)
    .collect();
    assert_eq!(
        &lines[1..],
        &[
            "  ⎿  {\"a\":1,\"a\":2}",
            "     not json {",
            "     42",
            "     {",
            "       \"ok\": true",
            "     }",
        ]
    );
}

#[test]
fn output_past_the_prettify_cap_is_shown_as_it_came() {
    // Claude Code's guard: past 10 000 characters no line is reshaped, even
    // a JSON one — the bound on what a render may parse.
    let out = format!("{{\"a\":1}}\n{}", "x".repeat(TOOL_JSON_PRETTY_MAX_BYTES));
    let lines: Vec<String> = tool_lines(
        &tool("Bash", "x", ToolStatus::Ok, &out),
        80,
        &PathDisplay::VERBATIM,
    )
    .iter()
    .map(plain)
    .collect();
    assert_eq!(lines[1], "  ⎿  {\"a\":1}", "{lines:?}");
}

#[test]
fn the_running_tail_never_reshapes_what_is_still_streaming() {
    // A running command's output is partial and arrives line by line: the
    // tail shows it as printed — reshaping happens when the cell settles.
    let t = tool("Bash", "curl", ToolStatus::Running, "{\"a\":1,\"b\":2}\n");
    let lines: Vec<String> = running_command_lines(
        &t,
        Duration::from_secs(1),
        Duration::ZERO,
        60,
        &PathDisplay::VERBATIM,
    )
    .iter()
    .map(plain)
    .collect();
    assert_eq!(
        lines[1..],
        ["  ⎿  {\"a\":1,\"b\":2}", "     (1s · timeout 2m)"],
        "{lines:?}"
    );
}

#[test]
fn a_four_row_output_shows_whole_instead_of_hiding_one_row() {
    // Claude Code's fold: three rows above it — but an output of exactly four
    // shows all four, since a hint hiding one row costs the row it hides.
    let four: Vec<String> = tool_lines(
        &tool("Bash", "seq 4", ToolStatus::Ok, "a\nb\nc\nd"),
        80,
        &PathDisplay::VERBATIM,
    )
    .iter()
    .map(plain)
    .collect();
    assert_eq!(four.len(), 1 + 4, "{four:?}");
    assert!(!four.iter().any(|l| l.contains("ctrl+o")), "{four:?}");
    let five: Vec<String> = tool_lines(
        &tool("Bash", "seq 5", ToolStatus::Ok, "a\nb\nc\nd\ne"),
        80,
        &PathDisplay::VERBATIM,
    )
    .iter()
    .map(plain)
    .collect();
    assert_eq!(five.len(), 1 + TOOL_FOLD_ROWS + 1, "{five:?}");
    assert!(five[3].contains('c'), "{five:?}");
    assert!(five[4].contains("+2 lines (ctrl+o to expand)"), "{five:?}");
}

#[test]
fn trailing_blank_lines_never_count_as_hidden_rows() {
    // `echo; echo` tails, a formatter's closing newlines: nothing to see and
    // nothing to hide — the cell and the transcript both end at the last row
    // that has something on it.
    let lines: Vec<String> = tool_lines(
        &tool("Bash", "x", ToolStatus::Ok, "a\nb\n\n\n"),
        80,
        &PathDisplay::VERBATIM,
    )
    .iter()
    .map(plain)
    .collect();
    assert_eq!(lines[1..], ["  ⎿  a", "     b"], "{lines:?}");
    let full: Vec<String> = tool_full_lines(
        &tool("Bash", "x", ToolStatus::Ok, "a\nb\n\n\n"),
        80,
        &PathDisplay::VERBATIM,
    )
    .iter()
    .map(plain)
    .collect();
    assert_eq!(full[1..], ["  ⎿  a", "     b"], "{full:?}");
}

#[test]
fn the_fold_shows_three_rows_of_one_long_line_unmarked() {
    // A 600-char blob at 35 content columns is 18 rows: the first three show
    // as they are — the hint right under them is what says it continues —
    // and the count is the fifteen rows the expansion adds.
    let long = "x".repeat(600);
    let lines: Vec<String> = tool_lines(
        &tool("Bash", "cat big", ToolStatus::Ok, &long),
        40,
        &PathDisplay::VERBATIM,
    )
    .iter()
    .map(plain)
    .collect();
    assert_eq!(lines.len(), 1 + TOOL_FOLD_ROWS + 1, "{lines:?}");
    for row in &lines[1..=TOOL_FOLD_ROWS] {
        assert_eq!(
            row.trim_start_matches([' ', '⎿']),
            "x".repeat(35),
            "{row:?}"
        );
    }
    assert!(
        lines[4].contains("+15 lines (ctrl+o to expand)"),
        "{lines:?}"
    );
}

// --- the header's path is a `file://` link (docs/links.md) ---

/// The `(text, target)` pairs of a line's link-carrying spans.
fn linked_spans(line: &Line) -> Vec<(String, String)> {
    line.spans
        .iter()
        .filter_map(|s| {
            crate::links::style_link(&s.style).map(|u| (s.content.to_string(), u.to_string()))
        })
        .collect()
}

/// The `● name(args)` rows of a rendered cell — everything above the `⎿`.
fn header_only<'a>(lines: &'a [Line<'a>]) -> Vec<&'a Line<'a>> {
    lines
        .iter()
        .take_while(|l| !plain(l).contains('⎿'))
        .collect()
}

#[test]
fn a_file_tool_header_links_its_path_to_the_file() {
    // `● Write(cat_poem.txt)`: the shown path carries the file's absolute
    // `file://` target — the whole file, not the shortened text — while the
    // bullet, the name, the parens and every corner row carry none.
    let paths = session_paths();
    let output = "Wrote 2 lines to cat_poem.txt\n1 The Cat in the Box\n2 ";
    let cell = tool(
        "Write",
        "/home/linuztx/Codes/tests/cat_poem.txt",
        ToolStatus::Ok,
        output,
    );
    let lines = tool_lines(&cell, 80, &paths);
    assert_eq!(
        plain(&lines[0]),
        "● Write(cat_poem.txt)",
        "the visible row is what it was"
    );
    assert_eq!(
        linked_spans(&lines[0]),
        vec![(
            "cat_poem.txt".to_string(),
            "file:///home/linuztx/Codes/tests/cat_poem.txt".to_string()
        )]
    );
    let close = lines[0].spans.last().expect("the closing paren");
    assert_eq!(close.content.as_ref(), ")");
    assert_eq!(
        close.style.fg,
        Some(tool_args_color()),
        "the `)` keeps the args dress, unlinked"
    );
    assert_eq!(plain(&lines[1]), "  ⎿  Wrote 2 lines to cat_poem.txt");
    for line in &lines[1..] {
        assert!(
            linked_spans(line).is_empty(),
            "the corner rows are not links: {:?}",
            plain(line)
        );
    }
}

#[test]
fn read_and_edit_headers_link_alike_whatever_form_the_row_shows() {
    let paths = session_paths();
    // `~/hello.py` and an absolute path outside home both link the file.
    let read = tool("Read", "/home/linuztx/hello.py", ToolStatus::Ok, "1 x");
    let lines = tool_lines(&read, 80, &paths);
    assert_eq!(plain(&lines[0]), "● Read(~/hello.py)");
    assert_eq!(
        linked_spans(&lines[0]),
        vec![(
            "~/hello.py".to_string(),
            "file:///home/linuztx/hello.py".to_string()
        )]
    );
    let edit = tool(
        "Edit",
        "/tmp/notes.txt",
        ToolStatus::Ok,
        "Updated ../../../../tmp/notes.txt (+1 -1)\n1 -a\n1 +b",
    );
    let lines = tool_lines(&edit, 80, &paths);
    assert_eq!(plain(&lines[0]), "● Edit(/tmp/notes.txt)");
    assert_eq!(
        linked_spans(&lines[0]),
        vec![(
            "/tmp/notes.txt".to_string(),
            "file:///tmp/notes.txt".to_string()
        )]
    );
    assert!(
        linked_spans(&lines[1]).is_empty(),
        "the `Updated …` head is not a link"
    );
    // An image read links the same way — the header is the header.
    let image = tool(
        "Read",
        "/tmp/alter-zero-1000/18d27aab55500771-4c329/scratchpad/cute_cat.jpg",
        ToolStatus::Ok,
        "Read image (PNG, 784x562, 1.1 MB)",
    );
    let lines = tool_lines(&image, 100, &paths);
    assert_eq!(
        linked_spans(&lines[0]),
        vec![(
            "/tmp/alter-zero-1000/18d27aab55500771-4c329/scratchpad/cute_cat.jpg".to_string(),
            "file:///tmp/alter-zero-1000/18d27aab55500771-4c329/scratchpad/cute_cat.jpg"
                .to_string()
        )]
    );
    assert_eq!(plain(&lines[1]), "  ⎿  Read image (PNG, 784x562, 1.1 MB)");
    assert!(linked_spans(&lines[1]).is_empty());
}

#[test]
fn a_wrapped_or_cut_header_path_links_every_fragment_to_the_whole_file() {
    // The point of the carrier (docs/links.md): a path hard-broken across
    // rows — or cut to the header's column budget — opens the whole file
    // from any fragment, where a terminal's own detection sees row text.
    let paths = session_paths();
    let file = "/home/linuztx/Codes/tests/some/deeply/nested/directory/tree/file.txt";
    let cell = tool(
        "Write",
        file,
        ToolStatus::Ok,
        "Wrote 1 line to some/deeply/nested/directory/tree/file.txt\n1 x",
    );
    let lines = tool_lines(&cell, 30, &paths);
    let header = header_only(&lines);
    let rows: Vec<String> = header.iter().map(|l| plain(l)).collect();
    assert!(header.len() >= 2, "the path wraps at 30 columns: {rows:?}");
    let linked: Vec<(String, String)> = header.iter().flat_map(|l| linked_spans(l)).collect();
    assert!(
        linked.len() >= 2,
        "every fragment is its own linked span: {linked:?}"
    );
    let shown: String = linked.iter().map(|(t, _)| t.as_str()).collect();
    assert_eq!(
        shown, "some/deeply/nested/directory/tree/file.txt",
        "the fragments are the shown path and nothing else: {rows:?}"
    );
    for (_, url) in &linked {
        assert_eq!(
            url,
            "file:///home/linuztx/Codes/tests/some/deeply/nested/directory/tree/file.txt"
        );
    }
    // A path past the header's column budget is cut with `…` in the
    // collapsed cell; the fragment still carries the whole file.
    let long = format!("/tmp/{}/file.txt", "d".repeat(TOOL_HEADER_MAX_COLS));
    let cell = tool("Read", &long, ToolStatus::Ok, "1 x");
    let lines = tool_lines(&cell, 400, &paths);
    let row = plain(&lines[0]);
    assert!(
        row.ends_with(&format!("{TOOL_HEADER_ELLIPSIS})")),
        "cut: {row}"
    );
    let linked = linked_spans(&lines[0]);
    assert_eq!(linked.len(), 1, "{linked:?}");
    assert_eq!(linked[0].1, format!("file://{long}"));
}

#[test]
fn only_a_file_tools_header_is_a_link() {
    // A `Bash` command embeds paths the shell resolves, an `Agent`'s summary
    // is prose — neither is a file to open.
    let paths = session_paths();
    let cell = tool(
        "Bash",
        "python3 /home/linuztx/Codes/tests/hello.py",
        ToolStatus::Ok,
        "Exit code: 0\nhi",
    );
    for line in tool_lines(&cell, 80, &paths) {
        assert!(linked_spans(&line).is_empty(), "{:?}", plain(&line));
    }
    let cell = tool(
        "Agent",
        "/home/linuztx/Codes/tests/x",
        ToolStatus::Ok,
        "done",
    );
    assert!(linked_spans(&tool_lines(&cell, 80, &paths)[0]).is_empty());
    // The `(`/`)` framing a linked path is not part of it.
    let cell = tool(
        "Write",
        "/home/linuztx/Codes/tests/hello.py",
        ToolStatus::Ok,
        "Wrote 1 line to hello.py\n1 x",
    );
    let header = &tool_lines(&cell, 80, &paths)[0];
    for span in header
        .spans
        .iter()
        .filter(|s| s.content.as_ref() != "hello.py")
    {
        assert_eq!(
            crate::links::style_link(&span.style),
            None,
            "not a link: {:?}",
            span.content
        );
    }
}

#[test]
fn the_verbatim_policy_links_an_absolute_path_and_leaves_a_relative_one() {
    // With no cwd to resolve against, an absolute argument is still a place
    // — the schema asks for absolute paths — while a relative one is not.
    let cell = tool(
        "Write",
        "/home/linuztx/hello.py",
        ToolStatus::Ok,
        "Wrote 1 line to hello.py\n1 x",
    );
    let lines = tool_lines(&cell, 80, &PathDisplay::VERBATIM);
    assert_eq!(plain(&lines[0]), "● Write(/home/linuztx/hello.py)");
    assert_eq!(
        linked_spans(&lines[0]),
        vec![(
            "/home/linuztx/hello.py".to_string(),
            "file:///home/linuztx/hello.py".to_string()
        )]
    );
    let cell = tool(
        "Write",
        "hello.py",
        ToolStatus::Ok,
        "Wrote 1 line to hello.py\n1 x",
    );
    let lines = tool_lines(&cell, 80, &PathDisplay::VERBATIM);
    assert_eq!(plain(&lines[0]), "● Write(hello.py)");
    assert!(
        linked_spans(&lines[0]).is_empty(),
        "nowhere to resolve `hello.py` to"
    );
}

#[test]
fn the_live_cell_and_the_transcript_link_the_header_alike() {
    // The strip's running cell (a `Write` waiting on the permission gate) and
    // the Ctrl+O expansion share the header builder, so the link rides both.
    let paths = session_paths();
    let cell = tool("Write", "/home/linuztx/hello.py", ToolStatus::Running, "");
    let live = live_tool_lines(&cell, 80, Duration::ZERO, &paths);
    assert_eq!(plain(&live[0]), "● Write(~/hello.py)");
    assert_eq!(
        linked_spans(&live[0]),
        vec![(
            "~/hello.py".to_string(),
            "file:///home/linuztx/hello.py".to_string()
        )]
    );
    let done = tool(
        "Write",
        "/home/linuztx/hello.py",
        ToolStatus::Ok,
        "Wrote 1 line to ../../hello.py\n1 x",
    );
    let full = tool_full_lines(&done, 80, &paths);
    assert_eq!(plain(&full[0]), "● Write(~/hello.py)");
    assert_eq!(
        linked_spans(&full[0]),
        vec![(
            "~/hello.py".to_string(),
            "file:///home/linuztx/hello.py".to_string()
        )]
    );
    assert!(linked_spans(&full[1]).is_empty());
}

// --- interactive sessions (docs/interactive-shell.md) ---

#[test]
fn a_session_cell_shows_its_output_over_a_dim_state_row() {
    // The report's frame line is for the model; the cell shows what the
    // program printed, then — like the classifier's provenance row — a
    // fresh dim corner saying where the session stands.
    let cell = tool(
        "Bash",
        "./signup.py",
        ToolStatus::Ok,
        "Running (session b7x2k9m1q, waiting for input)\nFull name:",
    );
    let lines = tool_lines(&cell, 80, &PathDisplay::VERBATIM);
    let rows: Vec<String> = lines.iter().map(plain).collect();
    assert_eq!(
        rows,
        [
            "● Bash(./signup.py)",
            "  ⎿  Full name:",
            "  ⎿  Waiting for input · session b7x2k9m1q",
        ]
    );
    assert_eq!(
        lines.last().unwrap().spans.last().unwrap().style.fg,
        Some(tool_dim_color()),
        "the state row is meta, so it renders dim"
    );
}

#[test]
fn a_session_still_busy_or_stopped_says_which() {
    let busy = tool(
        "BashSession",
        "b1",
        ToolStatus::Ok,
        "Running (session b1)\nCompiling…",
    );
    let rows: Vec<String> = tool_lines(&busy, 80, &PathDisplay::VERBATIM)
        .iter()
        .map(plain)
        .collect();
    assert_eq!(
        rows,
        [
            "● BashSession(b1)",
            "  ⎿  Compiling…",
            "  ⎿  Still running · session b1",
        ]
    );
    let stopped = tool(
        "BashSession",
        "b1 · kill",
        ToolStatus::Ok,
        "Stopped (session b1)\n(no new output)",
    );
    let rows: Vec<String> = tool_lines(&stopped, 80, &PathDisplay::VERBATIM)
        .iter()
        .map(plain)
        .collect();
    assert_eq!(
        rows,
        [
            "● BashSession(b1 · kill)",
            "  ⎿  (no new output)",
            "  ⎿  Stopped · session b1",
        ]
    );
}

#[test]
fn a_session_that_exited_reads_like_a_bash_cell() {
    let done = tool(
        "BashSession",
        "b1 ← y⏎",
        ToolStatus::Ok,
        "Exit code: 0\nSure? y\nbye",
    );
    let rows: Vec<String> = tool_lines(&done, 80, &PathDisplay::VERBATIM)
        .iter()
        .map(plain)
        .collect();
    assert_eq!(rows, ["● BashSession(b1 ← y⏎)", "  ⎿  Sure? y", "     bye"]);
}

#[test]
fn the_expanded_session_cell_keeps_the_state_row() {
    let cell = tool(
        "Bash",
        "python3",
        ToolStatus::Ok,
        "Running (session b1, waiting for input)\nPython 3.12\n>>>",
    );
    let rows: Vec<String> = tool_full_lines(&cell, 80, &PathDisplay::VERBATIM)
        .iter()
        .map(plain)
        .collect();
    assert_eq!(
        rows,
        [
            "● Bash(python3)",
            "  ⎿  Python 3.12",
            "     >>>",
            "  ⎿  Waiting for input · session b1",
        ]
    );
}

#[test]
fn a_running_session_call_shows_the_wait_it_runs_under() {
    // `bash_session` waits 10 s by default, not `bash`'s two minutes.
    let t = tool("BashSession", "b1", ToolStatus::Running, "");
    let lines: Vec<String> = running_command_lines(
        &t,
        Duration::from_secs(3),
        Duration::ZERO,
        80,
        &PathDisplay::VERBATIM,
    )
    .iter()
    .map(plain)
    .collect();
    assert_eq!(
        lines,
        ["● BashSession(b1)", "  ⎿  Running… (3s · timeout 10s)"]
    );
}
