//! Tool cells: headers, output peeks, and file-change rendering
//! (`docs/tools.md`, `docs/tool-streaming.md`).

use super::*;
use crate::ui::theme::{
    CODE_TAB_WIDTH, EXPAND_HINT, FILE_PEEK_LINES, TOOL_ARGS_COLOR, TOOL_DIFF_ADD_BG,
    TOOL_DIFF_ADD_COLOR, TOOL_DIFF_ADD_MARK_BG, TOOL_DIFF_DEL_BG, TOOL_DIFF_DEL_COLOR,
    TOOL_DIFF_DEL_MARK_BG, TOOL_DIM_COLOR, TOOL_FAIL_COLOR, TOOL_HEADER_MAX_ROWS,
    TOOL_LINE_ELLIPSIS, TOOL_LINE_MAX_ROWS, TOOL_OK_COLOR, TOOL_OUTPUT_COLOR, TOOL_PEEK_LINES,
    TOOL_PEEK_ROWS, TOOL_PULSE_BRIGHT, TOOL_PULSE_DIM, TOOL_PULSE_PERIOD, TOOL_RUNNING_COLOR,
    TOOL_WAITING_COLOR,
};
use crate::ui::tool::{live_tool_lines, running_command_lines, tool_full_lines};
use crate::ui::wrap::cols;

/// A theme RGB triple as the `Color` a rendered span carries.
fn rgb((r, g, b): (u8, u8, u8)) -> Color {
    Color::Rgb(r, g, b)
}

/// The content of a `⎿` gutter row — the corner (or a continuation row's
/// matching indent) stripped, so a test can name the text a row carries.
fn gutter_content(row: &str) -> &str {
    row.trim_start().trim_start_matches('⎿').trim_start()
}

// --- tool_lines (collapsed, colour-by-status) ---

#[test]
fn tool_lines_header_shows_name_and_args() {
    let lines = tool_lines(&tool("Bash", "cargo test", ToolStatus::Ok, "a\nb\nc"), 80);
    assert_eq!(plain(&lines[0]), "● Bash(cargo test)");
}

#[test]
fn tool_lines_header_omits_the_parens_when_args_are_empty() {
    // A `!` shell command is a tool with no args (name = the command), so
    // its header reads `● {command}`, not `● {command}()`.
    let lines = tool_lines(&tool("echo hi", "", ToolStatus::Ok, "hi"), 80);
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
    let lines = tool_lines(&cell, 100);
    assert_eq!(plain(&lines[0]), "● User answered Alter Zero's questions:");
    let bullet = &lines[0].spans[0];
    assert_eq!(
        bullet.style.fg,
        Some(TOOL_OK_COLOR),
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
    let full = tool_full_lines(&cell, 100);
    assert_eq!(plain(&full[0]), "● User answered Alter Zero's questions:");
    assert!(plain(&full[2]).contains("· Pick a snack → Chips, Fruit"));
}

#[test]
fn a_declined_ask_cell_is_red_and_lists_the_questions() {
    let output = "User declined to answer questions\n\
                  · Which code style do you prefer? (Arrow function / One-liner)";
    let cell = tool("AskUserQuestion", "ignored", ToolStatus::Failed, output);
    let lines = tool_lines(&cell, 100);
    assert_eq!(plain(&lines[0]), "● User declined to answer questions");
    assert_eq!(lines[0].spans[0].style.fg, Some(TOOL_FAIL_COLOR));
    assert!(plain(&lines[1]).contains("(Arrow function / One-liner)"));
}

#[test]
fn a_running_ask_cell_keeps_the_generic_header() {
    // While the modal is up the call is Running — the ordinary
    // `● AskUserQuestion(…)` header stands (mostly hidden behind the modal;
    // the Ctrl+O transcript shows it).
    let cell = tool("AskUserQuestion", "Pick one?", ToolStatus::Running, "");
    let lines = tool_lines(&cell, 100);
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
    let lines = tool_lines(&noted("Bash", "ls -la", ToolStatus::Ok, output), 80);
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
        Some(TOOL_DIM_COLOR),
        "the note is meta, so it renders dim"
    );
}

#[test]
fn the_note_shows_in_the_expanded_transcript_view_too() {
    let lines = tool_full_lines(
        &noted("Bash", "ls -la", ToolStatus::Ok, "Exit code: 0\ntotal 40"),
        80,
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
        let lines = tool_lines(&noted("Bash", "ls -la", status, ""), 80);
        assert!(
            !lines.iter().map(plain).any(|t| t.contains("Allowed by")),
            "no note while {status:?}"
        );
    }
    // …and a failed run still shows it: the classifier did allow the call.
    let lines = tool_lines(
        &noted("Bash", "ls /gone", ToolStatus::Failed, "Exit code: 2"),
        80,
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
    let plain_cell = tool_lines(&tool("Bash", "ls", ToolStatus::Ok, "Exit code: 0\nout"), 80);
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
        (ToolStatus::Running, TOOL_RUNNING_COLOR),
        (ToolStatus::Ok, TOOL_OK_COLOR),
        (ToolStatus::Failed, TOOL_FAIL_COLOR),
    ] {
        let lines = tool_lines(&tool("X", "y", status, "out"), 80);
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
    let lines = tool_lines(&tool("Bash", "cargo test", ToolStatus::Running, ""), 80);
    assert_eq!(lines[0].spans[0].style.fg, Some(TOOL_DIM_COLOR));
}

#[test]
fn a_running_bullet_breathes_across_the_pulse_period() {
    // …and in the live region it pulses, Claude-Code's running dot: dim at the
    // top of the cycle, back up at the half, down again — a pure function of
    // the boundary-injected frame clock, like the status shimmer. The breath
    // only ever dips **below** the resting grey; its peak is that same grey, so
    // the bullet never brightens toward white.
    let call = tool("Bash", "cargo test", ToolStatus::Running, "");
    let bullet = |at: Duration| live_tool_lines(&call, 80, at)[0].spans[0].style.fg;
    let half = TOOL_PULSE_PERIOD / 2;
    assert_eq!(bullet(Duration::ZERO), Some(rgb(TOOL_PULSE_DIM)));
    assert_eq!(bullet(half), Some(rgb(TOOL_PULSE_BRIGHT)));
    // A full period later it is back where it started — the cycle loops.
    assert_eq!(bullet(TOOL_PULSE_PERIOD), Some(rgb(TOOL_PULSE_DIM)));
    assert_eq!(
        bullet(TOOL_PULSE_PERIOD + half),
        Some(rgb(TOOL_PULSE_BRIGHT))
    );
    // Between the extremes it is genuinely in between, not snapped to one end.
    let mid = bullet(TOOL_PULSE_PERIOD / 4);
    assert_ne!(mid, Some(rgb(TOOL_PULSE_DIM)));
    assert_ne!(mid, Some(rgb(TOOL_PULSE_BRIGHT)));
}

#[test]
fn only_a_running_bullet_pulses() {
    // The pulse means "this is happening now". A queued sibling stays its flat
    // waiting grey and a resolved call keeps its green/red, whatever the frame
    // clock says — otherwise the animation would say nothing.
    for (status, color) in [
        (ToolStatus::Waiting, TOOL_WAITING_COLOR),
        (ToolStatus::Ok, TOOL_OK_COLOR),
        (ToolStatus::Failed, TOOL_FAIL_COLOR),
    ] {
        let call = tool("X", "y", status, "out");
        for at in [Duration::ZERO, TOOL_PULSE_PERIOD / 2] {
            assert_eq!(
                live_tool_lines(&call, 80, at)[0].spans[0].style.fg,
                Some(color),
                "{status:?} never animates"
            );
        }
    }
}

#[test]
fn a_committed_cell_never_carries_a_pulse_frame() {
    // `tool_lines` feeds scrollback, where a colour is frozen forever. It
    // renders a running bullet **at rest** — the flat grey — so a cell can
    // never be committed mid-breath.
    let call = tool("Bash", "cargo test", ToolStatus::Running, "");
    assert_eq!(
        tool_lines(&call, 80)[0].spans[0].style.fg,
        Some(TOOL_RUNNING_COLOR)
    );
    // The peak of the breath *is* the resting grey — the pulse only dips below
    // it — so what a commit must never freeze is the **dip**. That is also the
    // value an un-injected clock would render (phase 0), which is exactly the
    // accident this renderer split exists to prevent.
    assert_ne!(TOOL_RUNNING_COLOR, rgb(TOOL_PULSE_DIM));
    assert_eq!(
        TOOL_RUNNING_COLOR,
        rgb(TOOL_PULSE_BRIGHT),
        "the breath tops out at the resting grey, never brighter"
    );
}

#[test]
fn tool_lines_collapses_a_command_output_to_a_multiline_peek_plus_hint() {
    // A finished command-style backend tool (bash) shows up to TOOL_PEEK_LINES
    // of its output — the head, Claude-Code style — then a
    // `… +N lines (ctrl+o to expand)` hint (docs/tool-streaming.md), like the
    // `!` shell cell. (This is the mock's finished state.)
    let out = "l1\nl2\nl3\nl4\nl5\nl6";
    let lines = tool_lines(&tool("Bash", "seq 6", ToolStatus::Ok, out), 80);
    assert_eq!(
        lines.len(),
        TOOL_PEEK_LINES + 2,
        "header + {TOOL_PEEK_LINES} peek rows + hint: {:?}",
        lines.iter().map(plain).collect::<Vec<_>>()
    );
    assert!(
        plain(&lines[1]).contains("l1"),
        "peek opens at the first line"
    );
    assert!(
        plain(&lines[TOOL_PEEK_LINES]).contains("l4"),
        "peek shows up to the {TOOL_PEEK_LINES}th line"
    );
    let hint = plain(&lines[TOOL_PEEK_LINES + 1]);
    assert!(
        hint.contains("+2 lines"),
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
    let lines: Vec<String> = tool_lines(&tool("Bash", "cat log", ToolStatus::Ok, &out), 40)
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
    // The block ceiling: FOUR pathological lines (a minified bundle each)
    // spend TOOL_PEEK_ROWS rows between them — the cell is bounded in display
    // rows, so it can never balloon past what four short lines would cost.
    // The hint counts every display row hidden underneath, across all four
    // (docs/long-lines.md).
    let long = "x".repeat(600); // 35 content cols → 18 rows uncapped
    let out = vec![long; 4].join("\n");
    let lines: Vec<String> = tool_lines(&tool("Bash", "cat big", ToolStatus::Ok, &out), 40)
        .iter()
        .map(plain)
        .collect();
    assert_eq!(
        lines.len(),
        1 + TOOL_PEEK_ROWS + 1,
        "header + the {TOOL_PEEK_ROWS}-row ceiling + hint: {lines:?}"
    );
    let hint = lines.last().unwrap();
    assert!(
        hint.contains("ctrl+o to expand"),
        "the hint signals more: {hint:?}"
    );
    assert!(
        hint.contains(&format!("+{} lines", 4 * 18 - TOOL_PEEK_ROWS)),
        "every row hidden under the ceiling is counted: {hint:?}"
    );
}

#[test]
fn a_finished_command_peek_skips_the_output_s_leading_blank_lines() {
    // A command whose output opens on a blank line (a `\n` before the real
    // first row — an `echo` with a leading newline, a formatter's spacer)
    // used to spend the cell's first row on nothing. The peek opens at the
    // first line that has something on it (docs/long-lines.md).
    let out = "\nl1\nl2\nl3\nl4\nl5";
    let lines: Vec<String> = tool_lines(&tool("Bash", "seq", ToolStatus::Ok, out), 80)
        .iter()
        .map(plain)
        .collect();
    assert_eq!(
        lines.len(),
        1 + TOOL_PEEK_LINES + 1,
        "header + {TOOL_PEEK_LINES} content rows + hint — no blank row: {lines:?}"
    );
    assert_eq!(
        gutter_content(&lines[1]),
        "l1",
        "the peek opens at the first non-blank line: {lines:?}"
    );
    let hint = lines.last().unwrap();
    assert!(
        hint.contains("+2 lines"),
        "the skipped blank is still counted as a hidden row, with l5: {hint:?}"
    );
}

#[test]
fn a_finished_command_peek_stops_at_the_first_blank_line() {
    // The peek is the output's first **block**: once a blank line arrives the
    // cell stops rather than spending a row on it (and rather than hopping the
    // gap, which would read as one run of lines that isn't one).
    let out = "l1\nl2\n\nl3\nl4\nl5";
    let lines: Vec<String> = tool_lines(&tool("Bash", "seq", ToolStatus::Ok, out), 80)
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
fn a_finished_command_peek_keeps_a_blank_only_output_as_it_is() {
    // Nothing to prefer when there is no non-blank line anywhere: the block
    // renders exactly as before rather than collapsing to an empty cell whose
    // hint has no `⎿` corner to hang from.
    let out = "\n\n\n";
    let lines: Vec<String> = tool_lines(&tool("Bash", "printf", ToolStatus::Ok, out), 80)
        .iter()
        .map(plain)
        .collect();
    assert_eq!(
        lines.len(),
        1 + 3,
        "header + the three blank rows: {lines:?}"
    );
    assert!(
        !lines.iter().any(|l| l.contains("ctrl+o to expand")),
        "nothing is hidden: {lines:?}"
    );
}

#[test]
fn a_shell_cell_peek_follows_the_same_first_block_rule() {
    // The `!` shell cell is the same exec cell as the backend `bash` one
    // (docs/shell-command.md) — headerless, same gutter, same budget — so its
    // peek skips the leading blanks and stops at the first interior one too.
    let mut t = tool("printf '\\n\\nout\\n'", "", ToolStatus::Ok, "\n\nout\ntail");
    t.shell = true;
    let lines: Vec<String> = tool_lines(&t, 80).iter().map(plain).collect();
    assert_eq!(lines.len(), 3, "the block's two rows + hint: {lines:?}");
    assert_eq!(gutter_content(&lines[0]), "out", "{lines:?}");
    assert_eq!(gutter_content(&lines[1]), "tail", "{lines:?}");
    assert!(
        lines[2].contains("+2 lines"),
        "the two skipped blanks are counted: {lines:?}"
    );
}

#[test]
fn a_blank_line_inside_a_wrapped_first_block_still_closes_the_peek() {
    // The block rule is applied to **source** lines before wrapping, so a
    // wrapping line inside the block still shows whole and the blank after it
    // still ends the cell.
    let out = format!("{}\n\nafter", "a".repeat(60)); // 35 content cols → 2 rows
    let lines: Vec<String> = tool_lines(&tool("Bash", "cat", ToolStatus::Ok, &out), 40)
        .iter()
        .map(plain)
        .collect();
    assert_eq!(
        lines.len(),
        1 + 2 + 1,
        "header + the long line's two wrapped rows + hint: {lines:?}"
    );
    let hint = lines.last().unwrap();
    assert!(hint.contains("+2 lines"), "the blank and `after`: {hint:?}");
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
    let rows: Vec<String> = tool_lines(&t, 66).iter().map(plain).collect();
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
    let lines = tool_lines(&tool("Edit", "f", ToolStatus::Ok, &long), 40);
    let rows: Vec<_> = lines[1..].iter().collect();
    assert!(
        rows.len() >= 2,
        "the long + line wrapped: {:?}",
        rows.iter().map(|l| plain(l)).collect::<Vec<_>>()
    );
    for r in &rows {
        assert_eq!(
            r.spans[1].style.fg,
            Some(TOOL_DIFF_ADD_COLOR),
            "every wrapped row keeps the + colour: {:?}",
            plain(r)
        );
    }
}

#[test]
fn a_non_command_tool_with_raw_multiline_output_keeps_a_single_peek_line() {
    // Only a command tool (bash) expands to a multi-line peek. A generic
    // backend tool whose output isn't the numbered file-cell format (e.g. the
    // dummy's canned `Read`, or an unknown tool) keeps the compact single
    // peek line + hint — so its committed footprint is unchanged
    // (docs/tool-streaming.md; guards the resize/reflow layout, smoke Phase 17).
    let lines = tool_lines(&tool("Read", "f", ToolStatus::Ok, "one\ntwo\nthree"), 80);
    assert_eq!(
        lines.len(),
        3,
        "header + one peek line + hint: {:?}",
        lines.iter().map(plain).collect::<Vec<_>>()
    );
    assert!(
        plain(&lines[1]).contains("one"),
        "peek shows the first line"
    );
    assert!(
        !plain(&lines[1]).contains("two"),
        "the rest stays hidden inline"
    );
    assert!(
        plain(&lines[2]).contains("+2 lines"),
        "the hint counts the rest"
    );
}

#[test]
fn tool_lines_strips_the_leading_exit_code_frame_from_a_bash_cell() {
    // `tool.output` stays framed (`Exit code: N\n…`) for the model / context
    // replay, but the display drops that first line so the cell reads like
    // the real command output (docs/tool-streaming.md).
    let out = "Exit code: 0\nhello\nworld";
    let lines = tool_lines(&tool("Bash", "echo", ToolStatus::Ok, out), 80);
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
    let lines = running_command_lines(&t, Duration::from_secs(9), Duration::ZERO, 80);
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
        "+5 lines (9s)",
        "the footer counts hidden lines and the elapsed: {body:?}"
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
    let lines = running_command_lines(&t, Duration::from_secs(123), Duration::ZERO, 80);
    assert_eq!(
        plain(lines.last().unwrap()).trim(),
        "+5 lines (2m 3s)",
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
fn running_command_lines_without_overflow_shows_no_footer() {
    // Fewer lines than the window: show them all, no `+N lines` footer (the
    // status line carries the timer).
    let t = tool("Bash", "echo", ToolStatus::Running, "a\nb");
    let lines = running_command_lines(&t, Duration::from_secs(1), Duration::ZERO, 80);
    assert_eq!(
        lines.len(),
        3,
        "header + 2 output rows, no footer: {:?}",
        lines.iter().map(plain).collect::<Vec<_>>()
    );
    assert!(
        !lines.iter().any(|l| plain(l).contains("lines (")),
        "no footer when nothing is hidden"
    );
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
    let lines = running_command_lines(&t, Duration::from_secs(7), Duration::ZERO, 40);
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
        "+1 lines (7s)",
        "the footer counts the one fully hidden line: {body:?}"
    );
}

#[test]
fn tool_full_lines_strips_the_exit_code_frame_from_a_bash_cell() {
    // The Ctrl+O full view shows the whole body but, like the inline cell,
    // drops the `Exit code: N` frame line (docs/tool-streaming.md).
    let out = "Exit code: 0\nalpha\nbeta";
    let lines = tool_full_lines(&tool("Bash", "echo", ToolStatus::Ok, out), 80);
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
    let lines = tool_lines(&tool("Bash", "echo hi", ToolStatus::Ok, "hi"), 80);
    assert_eq!(lines.len(), 2, "header + peek only, nothing hidden");
    assert!(plain(&lines[1]).contains("hi"));
}

#[test]
fn tool_lines_running_shows_a_running_peek() {
    let lines = tool_lines(&tool("Bash", "sleep 1", ToolStatus::Running, ""), 80);
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
    let lines = tool_lines(&tool("Bash", "sleep 1", ToolStatus::Running, ""), 80);
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
    let lines = tool_lines(&tool("Bash", "ping x.com", ToolStatus::Waiting, ""), 80);
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
    let lines = tool_lines(&tool("Bash", "ping x.com", ToolStatus::Waiting, ""), 80);
    let bullet = lines[0].spans.first().expect("a bullet span");
    assert_eq!(
        bullet.style.fg,
        Some(TOOL_WAITING_COLOR),
        "the waiting bullet is the dim grey"
    );
}

#[test]
fn tool_header_body_is_bold_white_parens_included() {
    // The whole `(...)` header body — the command text AND its framing parens
    // — reads like a normal reply (bold + the white assistant colour), so a
    // bash command and its brackets are all noticeable rather than dim.
    let lines = tool_lines(&tool("Bash", "cargo test", ToolStatus::Ok, "out"), 80);
    let arg = lines[0]
        .spans
        .iter()
        .find(|s| s.content.contains("cargo"))
        .expect("an args span");
    assert_eq!(
        arg.style.fg,
        Some(TOOL_ARGS_COLOR),
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
        Some(TOOL_ARGS_COLOR),
        "the opening paren is bold white too, not a dim delimiter"
    );
    let close = lines[0]
        .spans
        .iter()
        .find(|s| s.content.contains(')'))
        .expect("a span carrying the close paren");
    assert_eq!(
        close.style.fg,
        Some(TOOL_ARGS_COLOR),
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
    let lines = tool_full_lines(&tool("Bash", cmd, ToolStatus::Ok, "out"), 50);
    let header: Vec<String> = lines
        .iter()
        .take_while(|l| !plain(l).contains('⎿'))
        .map(plain)
        .collect();
    assert!(
        header.len() > TOOL_HEADER_MAX_ROWS,
        "full view shows every header row: {header:?}"
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
    let lines = tool_lines(&tool("Edit", "a.rs", ToolStatus::Ok, output), 80);
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
        Some(TOOL_DIFF_DEL_COLOR)
    );
    assert_eq!(
        add.spans.last().unwrap().style.fg,
        Some(TOOL_DIFF_ADD_COLOR)
    );
}

#[test]
fn a_non_diff_tool_peek_is_not_diff_coloured() {
    // A `bash` cell whose output happens to start with `+`/`-` is NOT a diff
    // tool, so its rows render as plain (white) output, never green/red.
    let lines = tool_lines(
        &tool("Bash", "diff a b", ToolStatus::Ok, "-removed\n+added"),
        80,
    );
    let fg = lines[1].spans.last().unwrap().style.fg;
    assert_eq!(
        fg,
        Some(TOOL_OUTPUT_COLOR),
        "bash output is the plain white output colour"
    );
    assert_ne!(fg, Some(TOOL_DIFF_ADD_COLOR), "never diff-coloured");
    assert_ne!(fg, Some(TOOL_DIFF_DEL_COLOR), "never diff-coloured");
}

#[test]
fn tool_output_content_is_white_the_corner_stays_dim() {
    // A finished tool's output under the ⎿ gutter is the noticeable white
    // output colour, while the ⎿ corner glyph itself stays a dim delimiter.
    let lines = tool_lines(&tool("Bash", "echo hi", ToolStatus::Ok, "hello world"), 80);
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
        Some(TOOL_OUTPUT_COLOR),
        "output content is the white output colour"
    );
    let corner = out
        .spans
        .iter()
        .find(|s| s.content.contains('⎿'))
        .expect("the ⎿ corner span");
    assert_eq!(
        corner.style.fg,
        Some(TOOL_DIM_COLOR),
        "the ⎿ corner stays a dim delimiter"
    );
}

#[test]
fn tool_running_and_waiting_placeholders_stay_dim() {
    // The `Running…`/`Waiting…` placeholders are meta, not output, so they
    // keep the dim colour even though real output is now white.
    for status in [ToolStatus::Running, ToolStatus::Waiting] {
        let lines = tool_lines(&tool("Bash", "sleep 1", status, ""), 80);
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
            Some(TOOL_DIM_COLOR),
            "the {status:?} placeholder stays dim"
        );
    }
}

#[test]
fn write_tool_full_view_colours_the_diff() {
    let output = "Updated a.rs (+1 -0)\n keep\n+added";
    let lines = tool_full_lines(&tool("Write", "a.rs", ToolStatus::Ok, output), 80);
    let add = lines.iter().find(|l| plain(l).contains("+added")).unwrap();
    assert_eq!(
        add.spans.last().unwrap().style.fg,
        Some(TOOL_DIFF_ADD_COLOR)
    );
}

#[test]
fn write_cell_shows_numbered_syntax_highlighted_rows() {
    // A `Created …` head — the pre-rename spelling old rollouts still carry —
    // keeps parsing as a numbered body (llm::tools::render_numbered_content)
    // and renders as Claude-Code's Write preview: dim right-aligned line
    // numbers, the content syntax-highlighted by the path's extension.
    let output = "Created hello.py (2 lines)\n1 def main():\n2     x = \"hi\"";
    let lines = tool_lines(&tool("Write", "hello.py", ToolStatus::Ok, output), 80);
    assert_eq!(plain(&lines[0]), "● Write(hello.py)");
    assert_eq!(plain(&lines[1]), "  ⎿  Created hello.py (2 lines)");
    let row1 = &lines[2];
    assert_eq!(plain(row1), "      1 def main():");
    let num = &row1.spans[1];
    assert_eq!(num.content.as_ref(), "1 ");
    assert_eq!(num.style.fg, Some(TOOL_DIM_COLOR), "line number is dim");
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
    );
    assert_eq!(plain(&lines[0]), "● Write(/repo/nested/hello.py)");
    assert_eq!(plain(&lines[1]), "  ⎿  Wrote 2 lines to nested/hello.py");
    let row1 = &lines[2];
    assert_eq!(plain(row1), "      1 def main():");
    assert_eq!(
        row1.spans[1].style.fg,
        Some(TOOL_DIM_COLOR),
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
    let lines = tool_lines(&tool("Edit", "/x/main.rs", ToolStatus::Ok, output), 40);
    let add = lines
        .iter()
        .flat_map(|l| l.spans.iter())
        .find(|s| s.content.as_ref() == "+3")
        .expect("the added count span survives the wrap");
    assert_eq!(add.style.fg, Some(TOOL_DIFF_ADD_COLOR));
    let del = lines
        .iter()
        .flat_map(|l| l.spans.iter())
        .find(|s| s.content.as_ref() == "-1")
        .expect("the removed count span survives the wrap");
    assert_eq!(del.style.fg, Some(TOOL_DIFF_DEL_COLOR));
}

#[test]
fn an_image_read_cell_is_one_concise_fact_row() {
    // `⎿ Read image (PNG, 512x512, 17 KB)` — the executor's whole output, so
    // the cell is the header plus exactly one output row, no hint.
    let output = "Read image (PNG, 512x512, 17 KB)";
    let lines = tool_lines(&tool("Read", "flower.png", ToolStatus::Ok, output), 80);
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
    let lines = tool_lines(&tool("Read", "file.txt", ToolStatus::Failed, long), width);
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
    // Only the first source line peeks; the rest stay behind the accurate
    // `… +N lines` hint, wrap or no wrap.
    let output = "first line of the body that is long enough to wrap at this width\nsecond\nthird";
    let lines = tool_lines(&tool("Teleport", "x", ToolStatus::Ok, output), 40);
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
    let lines = tool_lines(&tool("Write", "f.txt", ToolStatus::Ok, &output), 80);
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
    let lines = tool_lines(&tool("Read", "app.py", ToolStatus::Ok, output), 80);
    assert_eq!(plain(&lines[0]), "● Read(app.py)");
    assert_eq!(plain(&lines[1]), "  ⎿  Read 2 lines");
    let row = &lines[2];
    assert_eq!(plain(row), "      1 def main():");
    let num = &row.spans[1];
    assert_eq!(num.content.as_ref(), "1 ");
    assert_eq!(num.style.fg, Some(TOOL_DIM_COLOR), "line number is dim");
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
    let lines = tool_lines(&tool("Read", "big.txt", ToolStatus::Ok, &body), 80);
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
    let lines = tool_lines(&tool("Edit", "a.rs", ToolStatus::Ok, output), 80);
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
    assert_eq!(del_sign.style.fg, Some(TOOL_DIFF_DEL_COLOR));
    let add_sign = add
        .spans
        .iter()
        .find(|s| s.content.as_ref() == "+")
        .unwrap();
    assert_eq!(add_sign.style.fg, Some(TOOL_DIFF_ADD_COLOR));
    // …every span past the `⎿` indent is tinted end to end — the row's own
    // tint, or the brighter mark tint where the line actually changed
    // (`docs/inline-diff.md`; here the `1` -> `2`).
    assert!(
        del.spans
            .iter()
            .skip(1)
            .all(|s| s.style.bg == Some(TOOL_DIFF_DEL_BG)
                || s.style.bg == Some(TOOL_DIFF_DEL_MARK_BG)),
        "removed row is tinted red"
    );
    assert!(
        add.spans
            .iter()
            .skip(1)
            .all(|s| s.style.bg == Some(TOOL_DIFF_ADD_BG)
                || s.style.bg == Some(TOOL_DIFF_ADD_MARK_BG)),
        "added row is tinted green"
    );
    assert_eq!(
        on_bg(del, TOOL_DIFF_DEL_MARK_BG),
        "1",
        "only the `1` changed"
    );
    assert_eq!(
        on_bg(add, TOOL_DIFF_ADD_MARK_BG),
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
    );
    let w_head = write[1]
        .spans
        .iter()
        .find(|s| s.content.contains("Created"))
        .expect("the Created summary span");
    assert_eq!(
        w_head.style.fg,
        Some(TOOL_OUTPUT_COLOR),
        "a write summary head is white"
    );

    let read = tool_lines(&tool("Read", "f.txt", ToolStatus::Ok, "1 a\n2 b"), 80);
    let r_head = read[1]
        .spans
        .iter()
        .find(|s| s.content.contains("Read"))
        .expect("the Read summary span");
    assert_eq!(
        r_head.style.fg,
        Some(TOOL_OUTPUT_COLOR),
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
    );
    let e_head = edit[1]
        .spans
        .iter()
        .find(|s| s.content.contains("Updated"))
        .expect("the Updated summary span");
    assert_eq!(
        e_head.style.fg,
        Some(TOOL_OUTPUT_COLOR),
        "an edit summary path is white"
    );
    // The counts still stand out green/red.
    let plus = edit[1].spans.iter().find(|s| s.content == "+1").unwrap();
    assert_eq!(
        plus.style.fg,
        Some(TOOL_DIFF_ADD_COLOR),
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
    let lines = tool_lines(&tool("Write", "big.txt", ToolStatus::Ok, &output), 80);
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
    let lines = tool_full_lines(&tool("Write", "big.txt", ToolStatus::Ok, &output), 80);
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
    let lines = tool_full_lines(&tool("Read", "f", ToolStatus::Ok, "x"), 80);
    assert_eq!(lines[0].spans[0].style.fg, Some(TOOL_OK_COLOR));
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
    let lines = tool_lines(&t, 60);
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
    let lines = tool_lines(&t, 60);
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
    let lines: Vec<String> = tool_lines(&t, 60)
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
    let texts: Vec<String> = tool_lines(&t, 80).iter().map(plain).collect();
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
    let texts: Vec<String> = tool_full_lines(&t, 80).iter().map(plain).collect();
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

use crate::ui::theme::{MCP_CALLED_PREFIX, MCP_CALLING_PREFIX, REASONING_LABEL_COLOR};
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
    let lines = tool_lines(&mcp_tool(ToolStatus::Running, ""), 100);
    assert_eq!(lines.len(), 1);
    assert_eq!(
        plain(&lines[0]),
        format!("● {MCP_CALLING_PREFIX}Deepwiki…{EXPAND_HINT}")
    );
}

#[test]
fn a_waiting_mcp_sibling_shows_the_waiting_row() {
    let lines = tool_lines(&mcp_tool(ToolStatus::Waiting, ""), 100);
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
    );
    assert_eq!(lines.len(), 1);
    assert_eq!(
        plain(&lines[0]),
        format!("{MCP_CALLED_PREFIX}Deepwiki{EXPAND_HINT}")
    );
    assert_eq!(lines[0].spans[0].style.fg, Some(REASONING_LABEL_COLOR));
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
    let texts: Vec<String> = tool_lines(&cell, 100).iter().map(plain).collect();
    assert_eq!(
        texts,
        vec![format!("{MCP_CALLED_PREFIX}Deepwiki{EXPAND_HINT}")],
        "the quiet line is the whole inline cell"
    );
    // The expanded transcript still closes with the note — the record that
    // no human approved this call survives where the full story lives.
    let full: Vec<String> = tool_full_lines(&cell, 100).iter().map(plain).collect();
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
    let texts: Vec<String> = tool_lines(&cell, 120).iter().map(plain).collect();
    assert_eq!(
        texts.last().map(String::as_str),
        Some("  ⎿  Allowed by auto mode classifier"),
        "got {texts:?}"
    );
}

#[test]
fn a_failed_mcp_cell_keeps_the_loud_generic_form() {
    let lines = tool_lines(&mcp_tool(ToolStatus::Failed, "server exploded"), 120);
    // The full header — pretty-printed args, not the raw JSON — over the
    // error peek, red bullet.
    let head = plain(&lines[0]);
    assert!(
        head.starts_with("● Deepwiki - ask_question (MCP)("),
        "got {head:?}"
    );
    assert!(head.contains("question: \"What is this?\""), "got {head:?}");
    assert!(!head.contains("{\"repoName\""), "raw JSON never renders");
    assert_eq!(lines[0].spans[0].style.fg, Some(TOOL_FAIL_COLOR));
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
    let lines = tool_full_lines(&cell, 76);
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
    let lines = tool_full_lines(&cell, 40);
    assert_eq!(plain(&lines[0]), "● Bash(echo one two three four five six");
    assert_eq!(plain(&lines[1]), "      seven eight nine ten eleven");
}

#[test]
fn the_ctrl_o_view_shows_the_full_mcp_story() {
    let cell = mcp_tool(ToolStatus::Ok, "{\n  \"result\": \"Flaredantic is…\"\n}");
    let lines = tool_full_lines(&cell, 120);
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
    let first = tool_commit_lines(&app.history, app.tool_queue(), 100)
        .expect("a mixed batch's MCP cell is never held");
    assert_eq!(
        first.iter().map(plain).collect::<Vec<_>>(),
        vec![format!("{MCP_CALLED_PREFIX}Deepwiki{EXPAND_HINT}")]
    );
    app.start_tool("Bash", "ls", None);
    app.end_tool("ok", true);
    let second: Vec<String> = tool_commit_lines(&app.history, app.tool_queue(), 100)
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
    let repaint: Vec<String> = crate::ui::conversation_lines(&app.history, 100)
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
        tool_commit_lines(&app.history, app.tool_queue(), 100),
        None,
        "the first call holds: the batch's next call is still MCP"
    );
    app.start_tool("deepwiki - read_wiki_structure (MCP)", "", None);
    app.end_tool("{\"topics\":[]}", true);
    let run: Vec<String> = tool_commit_lines(&app.history, app.tool_queue(), 100)
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
    let second: Vec<String> = tool_commit_lines(&app.history, app.tool_queue(), 100)
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
        tool_commit_lines(&app.history, app.tool_queue(), 100),
        None,
        "the run's first call holds its commit"
    );
    app.start_tool("deepwiki - read_wiki_structure (MCP)", "", None);
    app.end_tool("{\"topics\":[]}", true);
    let lines = tool_commit_lines(&app.history, app.tool_queue(), 100).expect("the run committed");
    assert_eq!(lines.len(), 1);
    assert_eq!(
        plain(&lines[0]),
        format!("{MCP_CALLED_PREFIX}Deepwiki 2 times{EXPAND_HINT}")
    );
    // And the repaint from history agrees, line for line.
    let repaint = crate::ui::conversation_lines(&app.history, 100);
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
    assert!(crate::ui::conversation_lines(committed, 100).is_empty());
    // Once the run ends, the rebuild has the whole run — as its one line.
    app.start_tool("deepwiki - read_wiki_structure (MCP)", "", None);
    app.end_tool("{\"topics\":[]}", true);
    let committed = crate::ui::committed_history(&app.history, app.tool_queue());
    assert_eq!(committed.len(), 2);
    let rebuilt: Vec<String> = crate::ui::conversation_lines(committed, 100)
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
    let lines = tool_commit_lines(&app.history, app.tool_queue(), 100).expect("the run committed");
    let texts: Vec<String> = lines.iter().map(plain).collect();
    assert_eq!(
        texts[0],
        format!("{MCP_CALLED_PREFIX}Deepwiki{EXPAND_HINT}")
    );
    assert_eq!(texts[1], "");
    assert!(texts[2].starts_with("● Deepwiki - read_wiki_structure (MCP)("));
    assert!(texts.iter().any(|t| t.contains("server exploded")));
    let repaint: Vec<String> = crate::ui::conversation_lines(&app.history, 100)
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
    let repaint: Vec<String> = crate::ui::conversation_lines(&app.history, 100)
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
    let lines = tool_lines(&mcp_tool(ToolStatus::Running, ""), 20);
    let head = plain(&lines[0]);
    assert!(head.starts_with("● Calling Deepwiki…"), "got {head:?}");
    assert!(!head.contains("ctrl+o"), "no room for the hint at 20 cols");
}

// --- very long output lines: bounded rows, honest counts (docs/long-lines.md) ---

#[test]
fn a_finished_peek_clips_one_pathological_line_to_the_per_line_budget() {
    // The reported mess: a 2 KB `curl` body used to spend the WHOLE block
    // ceiling (12 rows) on one source line. It now shows its head —
    // TOOL_LINE_MAX_ROWS rows — marked with the `…` that says it continues.
    let long = "x".repeat(600); // 35 content cols → 18 rows uncapped
    let lines: Vec<String> = tool_lines(&tool("Bash", "cat big", ToolStatus::Ok, &long), 40)
        .iter()
        .map(plain)
        .collect();
    assert_eq!(
        lines.len(),
        1 + TOOL_LINE_MAX_ROWS + 1,
        "header + the {TOOL_LINE_MAX_ROWS}-row line budget + hint: {lines:?}"
    );
    let last_row = &lines[TOOL_LINE_MAX_ROWS];
    assert!(
        last_row.ends_with(TOOL_LINE_ELLIPSIS),
        "the cut is visible on the last kept row: {last_row:?}"
    );
    let hint = lines.last().unwrap();
    assert!(
        hint.contains(&format!("+{} lines", 18 - TOOL_LINE_MAX_ROWS)),
        "the hint counts the DISPLAY ROWS it hid, not `+1 lines`: {hint:?}"
    );
    assert!(hint.contains("ctrl+o to expand"), "{hint:?}");
    for l in &lines {
        assert!(cols(l) <= 40, "no row overflows the width: {l:?}");
    }
}

#[test]
fn a_finished_peek_hint_counts_rows_hidden_inside_a_long_line() {
    // The example verbatim: a short line, then a huge one. The old hint said
    // `+1 lines` while hiding 15 rows of JSON — the number the user reads is
    // what expanding actually adds.
    let out = format!("/home/u/.local/bin/yt-dlp\n{}", "x".repeat(600));
    let lines: Vec<String> = tool_lines(&tool("Bash", "curl -s …", ToolStatus::Ok, &out), 40)
        .iter()
        .map(plain)
        .collect();
    assert_eq!(
        lines.len(),
        1 + 1 + TOOL_LINE_MAX_ROWS + 1,
        "header + the short line + the clipped line + hint: {lines:?}"
    );
    assert!(
        lines.last().unwrap().contains("+15 lines"),
        "18 wrapped rows less the {TOOL_LINE_MAX_ROWS} shown: {lines:?}"
    );
}

#[test]
fn a_clipped_line_still_leaves_room_for_the_line_after_it() {
    // The per-line budget inside the block ceiling: clipping the blob at
    // TOOL_LINE_MAX_ROWS is what buys the line after it a row, so the peek
    // still shows that the output continues (before, one line ate the whole
    // block). What the ceiling then hides is counted in the hint.
    let out = format!("{}\nbee\nsea", "x".repeat(600));
    let lines: Vec<String> = tool_lines(&tool("Bash", "cat log", ToolStatus::Ok, &out), 40)
        .iter()
        .map(plain)
        .collect();
    assert!(
        lines.iter().any(|l| l.contains("bee")),
        "the line after the blob still shows: {lines:?}"
    );
    assert_eq!(
        lines.len(),
        1 + TOOL_LINE_MAX_ROWS + 1 + 1,
        "header + clipped line + bee + hint: {lines:?}"
    );
    assert!(
        lines.last().unwrap().contains("+16 lines"),
        "the blob's 15 hidden rows plus `sea`: {lines:?}"
    );
}

#[test]
fn a_finished_peek_hint_counts_whole_hidden_lines_in_rows_too() {
    // Past the source-line budget the hint counts the hidden lines' ROWS: two
    // hidden lines, one of which wraps to three rows, reads `+4 lines`.
    let out = format!("a\nb\nc\nd\ne\n{}", "x".repeat(100)); // 100 cols → 3 rows
    let lines: Vec<String> = tool_lines(&tool("Bash", "cat log", ToolStatus::Ok, &out), 40)
        .iter()
        .map(plain)
        .collect();
    assert!(
        lines.last().unwrap().contains("+4 lines"),
        "`e` (1 row) + the 3-row line: {lines:?}"
    );
}

#[test]
fn everyday_short_output_counts_the_same_as_before() {
    // Rows and source lines are the same number when nothing wraps — the
    // change is invisible for ordinary output.
    let lines: Vec<String> = tool_lines(
        &tool("Bash", "seq 6", ToolStatus::Ok, "l1\nl2\nl3\nl4\nl5\nl6"),
        80,
    )
    .iter()
    .map(plain)
    .collect();
    assert!(lines.last().unwrap().contains("+2 lines"), "{lines:?}");
}

#[test]
fn a_shell_cell_clips_a_pathological_line_too() {
    // The headerless `!` exec cell shares the peek, so it is bounded the same
    // way (a `! curl` of a JSON API used to paint twelve rows).
    let mut t = tool("curl -s api", "", ToolStatus::Ok, &"x".repeat(600));
    t.shell = true;
    let lines: Vec<String> = tool_lines(&t, 40).iter().map(plain).collect();
    assert_eq!(
        lines.len(),
        TOOL_LINE_MAX_ROWS + 1,
        "headerless: the clipped line + hint: {lines:?}"
    );
    assert!(lines[TOOL_LINE_MAX_ROWS - 1].ends_with(TOOL_LINE_ELLIPSIS));
}

#[test]
fn a_generic_backend_cell_clips_its_single_peeked_line() {
    // A non-command backend tool (an image read's fact line, an error body)
    // peeks ONE source line — bounded by the same per-line budget.
    let long = "x".repeat(600);
    let lines: Vec<String> = tool_lines(&tool("WebFetch", "https://x", ToolStatus::Ok, &long), 40)
        .iter()
        .map(plain)
        .collect();
    assert_eq!(
        lines.len(),
        1 + TOOL_LINE_MAX_ROWS + 1,
        "header + budget + hint: {lines:?}"
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
    let lines: Vec<String> = tool_lines(&tool("Read", "min.json", ToolStatus::Ok, &body), 80)
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
    let lines = tool_full_lines(&tool("Bash", "cat big", ToolStatus::Ok, &long), 40);
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
    let lines: Vec<String> = running_command_lines(&t, Duration::from_secs(3), Duration::ZERO, 40)
        .iter()
        .map(plain)
        .collect();
    let footer = lines.last().unwrap();
    assert!(
        footer.contains("+18 lines (3s)"),
        "the 18 wrapped rows above the window: {lines:?}"
    );
}

#[test]
fn a_write_cell_clips_a_very_long_line_under_its_summary() {
    // The `Wrote …` head keeps its row; the pathological content line under it
    // is bounded like any other, and the short line after it still shows.
    let body = format!("Wrote 2 lines to min.js\n 1 {}\n 2 ok", "z".repeat(400));
    let lines: Vec<String> = tool_lines(&tool("Write", "min.js", ToolStatus::Ok, &body), 80)
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
    let lines = tool_lines(&tool("Edit", "min.js", ToolStatus::Ok, &body), 80);
    let cut_row = &lines[1 + TOOL_LINE_MAX_ROWS];
    assert!(
        plain(cut_row).ends_with(TOOL_LINE_ELLIPSIS),
        "the cut is marked: {:?}",
        plain(cut_row)
    );
    let marker = cut_row.spans.last().unwrap();
    assert_eq!(
        marker.style.bg,
        Some(TOOL_DIFF_ADD_BG),
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
    // TOOL_PEEK_ROWS display rows, whatever shape the output has.
    let out = format!(
        "<title>World Chess Championship - Wikipedia</title>\n{}\n{}\n{}",
        "<script>".to_string() + &"a".repeat(200),
        "RLSTATE=".to_string() + &"b".repeat(200),
        "<script>".to_string() + &"c".repeat(200),
    );
    let lines: Vec<String> = tool_lines(&tool("Bash", "curl -s …", ToolStatus::Ok, &out), 60)
        .iter()
        .map(plain)
        .collect();
    assert_eq!(
        lines.len(),
        1 + TOOL_PEEK_ROWS + 1,
        "header + the {TOOL_PEEK_ROWS}-row ceiling + hint: {lines:?}"
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
    let lines = tool_lines(&tool("Edit", "hello.txt", ToolStatus::Ok, output), 80);
    let del = diff_row(&lines, "Rivera");
    let add = diff_row(&lines, "Rivero");

    assert_eq!(on_bg(del, TOOL_DIFF_DEL_MARK_BG), "a");
    assert_eq!(on_bg(add, TOOL_DIFF_ADD_MARK_BG), "o");
    // Everything the edit did not touch keeps the plain row tint — the two
    // tints together are what say "this line changed, and *here*".
    assert!(on_bg(del, TOOL_DIFF_DEL_BG).contains("Bruce River"));
    assert!(on_bg(add, TOOL_DIFF_ADD_BG).contains("Bruce River"));
}

#[test]
fn the_changed_run_is_bold_and_the_removed_one_escapes_the_row_dim() {
    // A dimmed highlight would defeat its own purpose: the removed row dims
    // its *unchanged* text (codex's look) and leaves the changed run bright.
    let output = "Updated hello.txt (+1 -1)\n1 -Bruce Rivera\n1 +Bruce Rivero";
    let lines = tool_lines(&tool("Edit", "hello.txt", ToolStatus::Ok, output), 80);

    for (needle, mark_bg) in [
        ("Rivera", TOOL_DIFF_DEL_MARK_BG),
        ("Rivero", TOOL_DIFF_ADD_MARK_BG),
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
    assert_eq!(unchanged.style.bg, Some(TOOL_DIFF_DEL_BG));
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
    let lines = tool_lines(&tool("Edit", "a.py", ToolStatus::Ok, output), 80);
    let del = diff_row(&lines, "import os");
    let add = diff_row(&lines, "def main");
    assert_eq!(on_bg(del, TOOL_DIFF_DEL_MARK_BG), "");
    assert_eq!(on_bg(add, TOOL_DIFF_ADD_MARK_BG), "");
    assert!(
        del.spans
            .iter()
            .skip(1)
            .all(|s| s.style.bg == Some(TOOL_DIFF_DEL_BG))
    );
    assert!(
        add.spans
            .iter()
            .skip(1)
            .all(|s| s.style.bg == Some(TOOL_DIFF_ADD_BG))
    );
}

#[test]
fn context_and_write_rows_never_carry_a_mark_tint() {
    // A `Wrote …` body is brand-new content: no pairs, nothing to refine.
    let output = "Wrote 2 lines to a.txt\n1 alpha\n2 beta";
    let lines = tool_lines(&tool("Write", "a.txt", ToolStatus::Ok, output), 80);
    assert!(
        lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .all(|s| s.style.bg != Some(TOOL_DIFF_ADD_MARK_BG)
                && s.style.bg != Some(TOOL_DIFF_DEL_MARK_BG))
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
    let lines = tool_full_lines(&tool("Edit", "a.txt", ToolStatus::Ok, &output), 40);
    let rows: Vec<&Line> = lines
        .iter()
        .filter(|l| {
            l.spans
                .iter()
                .any(|s| s.style.bg == Some(TOOL_DIFF_ADD_MARK_BG))
        })
        .collect();
    assert!(rows.len() >= 2, "the changed run wraps onto a second row");
    let tinted: String = rows
        .iter()
        .flat_map(|l| l.spans.iter())
        .filter(|s| s.style.bg == Some(TOOL_DIFF_ADD_MARK_BG))
        .map(|s| s.content.as_ref())
        .collect();
    assert_eq!(tinted, "n".repeat(50));
}

#[test]
fn the_trailing_pad_keeps_the_row_tint_not_the_mark_tint() {
    // The bright block must end where the changed text ends — otherwise it
    // bleeds to the terminal's right edge and stops meaning "here".
    let output = "Updated hello.txt (+1 -1)\n1 -Bruce Rivera\n1 +Bruce Rivero";
    let lines = tool_lines(&tool("Edit", "hello.txt", ToolStatus::Ok, output), 80);
    let add = diff_row(&lines, "Rivero");
    let last = add.spans.last().unwrap();
    assert!(last.content.ends_with(' '), "the row pads to full width");
    assert_eq!(last.style.bg, Some(TOOL_DIFF_ADD_BG));
}
