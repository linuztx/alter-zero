//! Tool cells: headers, output peeks, and file-change rendering
//! (`docs/tools.md`, `docs/tool-streaming.md`).

use super::*;
use crate::ui::theme::{
    CODE_TAB_WIDTH, EXPAND_HINT, FILE_PEEK_LINES, TOOL_ARGS_COLOR, TOOL_DIFF_ADD_BG,
    TOOL_DIFF_ADD_COLOR, TOOL_DIFF_DEL_BG, TOOL_DIFF_DEL_COLOR, TOOL_DIM_COLOR, TOOL_FAIL_COLOR,
    TOOL_HEADER_MAX_ROWS, TOOL_OK_COLOR, TOOL_OUTPUT_COLOR, TOOL_PEEK_LINES, TOOL_PEEK_MAX_ROWS,
    TOOL_PULSE_BRIGHT, TOOL_PULSE_DIM, TOOL_PULSE_PERIOD, TOOL_RUNNING_COLOR, TOOL_WAITING_COLOR,
};
use crate::ui::tool::{live_tool_lines, running_command_lines, tool_full_lines};
use crate::ui::wrap::cols;

/// A theme RGB triple as the `Color` a rendered span carries.
fn rgb((r, g, b): (u8, u8, u8)) -> Color {
    Color::Rgb(r, g, b)
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
    // The peek budget is SOURCE lines (`TOOL_PEEK_LINES` of them, each
    // fully wrapped) — "the first 4 lines of output", not "the first 4
    // display rows": a long first line must not push its siblings out of
    // the peek. Three lines here, the first wrapping to 3 rows → all
    // three lines visible (5 rows), no hint.
    let out = format!("{}\nbee\nsea", "a".repeat(80)); // 35 content cols → 3 rows
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
    assert_eq!(lines.len(), 1 + 5, "header + 3+1+1 wrapped rows: {lines:?}");
}

#[test]
fn a_finished_peek_bounds_rows_and_hints_when_one_line_overflows_the_budget() {
    // The safety ceiling: a single pathological line (a minified bundle)
    // wraps but is bounded to TOOL_PEEK_MAX_ROWS display rows, so it can't
    // balloon the committed cell into hundreds of rows; the expand hint
    // appears because content is hidden below the ceiling — even though it
    // is one source line (a partially-shown line counts as not-fully-shown).
    let long = "x".repeat(600); // 35 content cols → 18 rows uncapped
    let lines: Vec<String> = tool_lines(&tool("Bash", "cat big", ToolStatus::Ok, &long), 40)
        .iter()
        .map(plain)
        .collect();
    assert_eq!(
        lines.len(),
        1 + TOOL_PEEK_MAX_ROWS + 1,
        "header + the {TOOL_PEEK_MAX_ROWS}-row ceiling + hint: {lines:?}"
    );
    let hint = lines.last().unwrap();
    assert!(
        hint.contains("ctrl+o to expand"),
        "the hint signals more: {hint:?}"
    );
    assert!(
        hint.contains("+1 lines"),
        "one source line, partially hidden below the window: {hint:?}"
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
    // The mock's running state: the header, the last TOOL_PEEK_LINES output
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
    // The TOOL_PEEK_LINES cap bounds *display rows*, so a wrapping tail
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
    // A `Created …` body (llm::tools::render_numbered_content) renders as
    // Claude-Code's Write preview: dim right-aligned line numbers, the
    // content syntax-highlighted by the path's extension.
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
    // …every span past the `⎿` indent sits on the row's background tint…
    assert!(
        del.spans
            .iter()
            .skip(1)
            .all(|s| s.style.bg == Some(TOOL_DIFF_DEL_BG)),
        "removed row is tinted red"
    );
    assert!(
        add.spans
            .iter()
            .skip(1)
            .all(|s| s.style.bg == Some(TOOL_DIFF_ADD_BG)),
        "added row is tinted green"
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
    app.start_tool("Bash", cmd);
    let width = 40;
    let pv = preview_rows(&app, width);
    assert!(
        pv > 2,
        "a wrapped header + ⎿ Running… is more than two rows: {pv}"
    );
    let h = live_height(&app.input, width, 24, true, pv, 0, 0, 0, 0, 0);
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
