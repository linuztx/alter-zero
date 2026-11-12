//! The hook-run cell — `● Ran 1 Stop hook` collapsed inline and expanded in
//! Ctrl+O (`docs/tools/hooks.md` *The hook-run cell*).

use super::*;
use crate::hooks::{HookReport, HookSaid, RanHook};
use crate::ui::hook_cell::{hook_duration, hook_report_full_lines};
use crate::ui::theme::{tool_dim_color, tool_fail_color, tool_ok_color, tool_output_color};
use crate::ui::wrap::cols;

const FEEDBACK: &str = "Stop hook feedback: There are uncommitted Git changes.";

fn ran(command: &str, millis: u64, said: Vec<HookSaid>) -> RanHook {
    RanHook {
        command: command.to_string(),
        config: Some("~/.alter-zero/hooks.json".to_string()),
        millis,
        said,
    }
}

fn stop(hooks: Vec<RanHook>) -> HookReport {
    HookReport {
        event: "Stop".to_string(),
        hooks,
        warnings: Vec::new(),
    }
}

fn check_git(said: Vec<HookSaid>) -> RanHook {
    ran("bash \"$HOME/.alter-zero/hooks/check-git.sh\"", 412, said)
}

fn texts(lines: &[Line]) -> Vec<String> {
    lines
        .iter()
        .map(|line| plain(line).trim_end().to_string())
        .collect()
}

/// The style of the span holding `needle` on `line`.
fn style_of(line: &Line, needle: &str) -> Style {
    line.spans
        .iter()
        .find(|span| span.content.contains(needle))
        .unwrap_or_else(|| panic!("no span holds {needle:?}: {line:?}"))
        .style
}

#[test]
fn a_stop_block_reads_ran_one_stop_hook_over_its_feedback() {
    // The reported gap: a Stop hook that kept the agent going showed nothing
    // inline. Its cell is Claude Code's shape — the count under a bullet,
    // the hint, and the feedback the agent continues on under the corner.
    let report = stop(vec![check_git(vec![HookSaid::Feedback(FEEDBACK.into())])]);
    assert_eq!(
        texts(&hook_report_lines(&report, 80)),
        vec![
            "● Ran 1 Stop hook (ctrl+o to expand)",
            "  ⎿  Stop hook feedback: There are uncommitted Git changes.",
        ]
    );
}

#[test]
fn the_header_wraps_under_its_bullet_where_one_row_cannot_hold_it() {
    // A committed row wider than the terminal is cut at its edge, so on a
    // narrow one the header wraps instead — the hint, still dim, taking a
    // row of its own when the last has no room for it.
    let report = HookReport {
        event: "UserPromptSubmit".to_string(),
        hooks: vec![
            ran("./a.sh", 1, Vec::new()),
            ran("./b.sh", 1, vec![HookSaid::Message("ok".into())]),
        ],
        warnings: Vec::new(),
    };
    let lines = hook_report_lines(&report, 32);
    assert_eq!(
        texts(&lines),
        vec![
            "● Ran 2 UserPromptSubmit hooks",
            "  (ctrl+o to expand)",
            "  ⎿  ok",
        ]
    );
    assert_eq!(
        style_of(&lines[1], "(ctrl+o to expand)").fg,
        Some(tool_dim_color())
    );
    assert_eq!(style_of(&lines[0], "●").fg, Some(tool_ok_color()));
    for width in [12u16, 20, 24, 40] {
        for row in texts(&hook_report_lines(&report, width)) {
            assert!(cols(&row) <= usize::from(width), "{row:?} at {width}");
        }
        for row in texts(&hook_report_full_lines(&report, width)) {
            assert!(cols(&row) <= usize::from(width), "{row:?} at {width}");
        }
    }
}

#[test]
fn the_header_counts_every_hook_that_ran_and_each_row_opens_its_own_corner() {
    let report = stop(vec![
        ran("./notify.sh", 9, Vec::new()),
        check_git(vec![
            HookSaid::Feedback(FEEDBACK.into()),
            HookSaid::Message("Checked 4 files".into()),
        ]),
    ]);
    assert_eq!(
        texts(&hook_report_lines(&report, 80)),
        vec![
            "● Ran 2 Stop hooks (ctrl+o to expand)",
            "  ⎿  Stop hook feedback: There are uncommitted Git changes.",
            "  ⎿  Checked 4 files",
        ],
        "a silent hook is counted, never given a row"
    );
}

#[test]
fn a_long_row_wraps_under_its_own_text_not_under_the_corner() {
    // Claude Code's Ink box sends a wrapped row back under the `⎿`; the
    // cells here hang every continuation under the text, like a tool's
    // output.
    let reason = "Stop hook feedback: There are uncommitted Git changes. Review the changes \
                  and commit the relevant files with a descriptive commit message before \
                  finishing.";
    let report = stop(vec![check_git(vec![HookSaid::Feedback(reason.into())])]);
    let rows = texts(&hook_report_lines(&report, 60));
    assert_eq!(rows[0], "● Ran 1 Stop hook (ctrl+o to expand)");
    assert!(rows[1].starts_with("  ⎿  Stop hook feedback: "), "{rows:?}");
    assert!(rows.len() > 2, "the reason wraps at 60 columns: {rows:?}");
    for row in &rows[2..] {
        assert!(
            row.starts_with("     ") && !row.starts_with("      "),
            "a continuation hangs under the text: {row:?} in {rows:?}"
        );
    }
    assert!(rows.iter().all(|row| cols(row) <= 60), "{rows:?}");
}

/// `n` numbered lines as one message: one row of the cell's that wraps to
/// `n` display rows.
fn lines_of(n: usize) -> HookSaid {
    HookSaid::Message(
        (1..=n)
            .map(|n| format!("line {n}"))
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

#[test]
fn a_long_row_folds_like_a_tool_cells_output_with_the_hint_left_to_the_header() {
    // Three rows and a `… +N lines` row past four — the header already says
    // `ctrl+o to expand`, so the fold row does not say it again.
    let rows = texts(&hook_report_lines(
        &stop(vec![check_git(vec![lines_of(6)])]),
        80,
    ));
    assert_eq!(
        rows,
        vec![
            "● Ran 1 Stop hook (ctrl+o to expand)",
            "  ⎿  line 1",
            "     line 2",
            "     line 3",
            "     … +3 lines",
        ]
    );
    // Exactly four rows show whole: a hint hiding one row would cost it.
    let rows = texts(&hook_report_lines(
        &stop(vec![check_git(vec![lines_of(4)])]),
        80,
    ));
    assert_eq!(rows.len(), 5, "header and four rows: {rows:?}");
    assert_eq!(rows[4], "     line 4");
}

#[test]
fn each_row_folds_on_its_own_so_no_row_hides_behind_another() {
    // Folding the body as a whole let a long feedback row push the error
    // after it out of sight — a red bullet over no red row. Each row folds on
    // its own, so every row shows its head.
    let report = stop(vec![
        check_git(vec![lines_of(6)]),
        ran(
            "./notify.sh",
            9,
            vec![HookSaid::error(
                "Stop",
                "hook ./notify.sh failed (exit 1): offline",
            )],
        ),
    ]);
    let lines = hook_report_lines(&report, 80);
    assert_eq!(
        texts(&lines),
        vec![
            "● Ran 2 Stop hooks (ctrl+o to expand)",
            "  ⎿  line 1",
            "     line 2",
            "     line 3",
            "     … +3 lines",
            "  ⎿  Stop hook error: ./notify.sh failed (exit 1): offline",
        ]
    );
    assert_eq!(
        style_of(&lines[5], "Stop hook error").fg,
        Some(tool_fail_color())
    );
    // Six one-row messages are six rows: none is the fold's to hide.
    let said = (1..=6)
        .map(|n| HookSaid::Message(format!("message {n}")))
        .collect();
    let rows = texts(&hook_report_lines(&stop(vec![check_git(said)]), 80));
    assert_eq!(rows.len(), 7, "the header and every message: {rows:?}");
    assert_eq!(rows[6], "  ⎿  message 6");
}

#[test]
fn the_fold_counts_the_display_rows_a_wrapped_row_hides() {
    // One long feedback row past the fold: the hint counts the rows Ctrl+O
    // adds, not the one source row.
    let reason = format!("Stop hook feedback: {}", "word ".repeat(80));
    let rows = texts(&hook_report_lines(
        &stop(vec![check_git(vec![HookSaid::Feedback(
            reason.trim_end().into(),
        )])]),
        40,
    ));
    assert_eq!(rows.len(), 5, "header, three rows, the fold: {rows:?}");
    let hidden: usize = rows[4]
        .trim_start()
        .strip_prefix("… +")
        .and_then(|rest| rest.strip_suffix(" lines"))
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| panic!("a fold row: {:?}", rows[4]));
    assert!(
        hidden > 1,
        "the wrapped rows past the fold are counted: {rows:?}"
    );
}

#[test]
fn a_block_is_a_hook_working_so_the_bullet_is_green_and_the_feedback_reads_as_output() {
    let lines = hook_report_lines(
        &stop(vec![check_git(vec![HookSaid::Feedback(FEEDBACK.into())])]),
        80,
    );
    assert_eq!(style_of(&lines[0], "●").fg, Some(tool_ok_color()));
    assert_eq!(
        style_of(&lines[1], "Stop hook feedback").fg,
        Some(tool_output_color())
    );
    assert_eq!(
        style_of(&lines[0], "(ctrl+o to expand)").fg,
        Some(tool_dim_color()),
        "the hint is dim"
    );
}

#[test]
fn a_hook_that_broke_is_a_red_row_and_turns_the_bullet_red() {
    let report = stop(vec![check_git(vec![HookSaid::Error(
        "Stop hook error: failed (exit 1): git: not found".into(),
    )])]);
    let lines = hook_report_lines(&report, 80);
    assert_eq!(style_of(&lines[0], "●").fg, Some(tool_fail_color()));
    assert_eq!(
        style_of(&lines[1], "Stop hook error").fg,
        Some(tool_fail_color())
    );
}

#[test]
fn a_broken_hook_is_named_on_its_row_but_not_twice_under_its_own_line() {
    // Collapsed, the rows stand without their hooks, so an error row says
    // which hook broke; expanded, it sits under that hook's line, which says
    // it already.
    let report = stop(vec![
        ran(
            "./notify.sh",
            9,
            vec![HookSaid::error(
                "Stop",
                "hook ./notify.sh failed (exit 1): offline",
            )],
        ),
        check_git(vec![HookSaid::Feedback(FEEDBACK.into())]),
    ]);
    assert_eq!(
        texts(&hook_report_lines(&report, 80)),
        vec![
            "● Ran 2 Stop hooks (ctrl+o to expand)",
            "  ⎿  Stop hook feedback: There are uncommitted Git changes.",
            "  ⎿  Stop hook error: ./notify.sh failed (exit 1): offline",
        ]
    );
    let full = hook_report_full_lines(&report, 80);
    assert_eq!(
        texts(&full),
        vec![
            "● Ran 2 Stop hooks",
            "  ⎿  ./notify.sh (9ms)",
            "     Config: ~/.alter-zero/hooks.json",
            "  ⎿  Stop hook error: failed (exit 1): offline",
            "  ⎿  bash \"$HOME/.alter-zero/hooks/check-git.sh\" (412ms)",
            "     Config: ~/.alter-zero/hooks.json",
            "  ⎿  Stop hook feedback: There are uncommitted Git changes.",
        ]
    );
    assert_eq!(
        style_of(&full[3], "Stop hook error").fg,
        Some(tool_fail_color()),
        "still red under its hook"
    );
}

#[test]
fn context_is_a_dim_row_and_a_config_problem_a_red_one_after_the_hooks() {
    let report = HookReport {
        event: "SessionStart".to_string(),
        hooks: vec![ran(
            "./git-status.sh",
            34,
            vec![HookSaid::Context {
                row: "Context added by SessionStart hook".into(),
                text: "On branch main".into(),
            }],
        )],
        warnings: vec![
            "hooks.json: SessionStart matcher \"[\" is not a valid regex — skipped".into(),
        ],
    };
    let lines = hook_report_lines(&report, 100);
    assert_eq!(
        texts(&lines),
        vec![
            "● Ran 1 SessionStart hook (ctrl+o to expand)",
            "  ⎿  Context added by SessionStart hook",
            "  ⎿  hooks.json: SessionStart matcher \"[\" is not a valid regex — skipped",
        ],
        "the text added is Ctrl+O's alone"
    );
    assert_eq!(
        style_of(&lines[1], "Context added").fg,
        Some(tool_dim_color())
    );
    assert_eq!(
        style_of(&lines[2], "hooks.json").fg,
        Some(tool_fail_color())
    );
    assert_eq!(style_of(&lines[0], "●").fg, Some(tool_fail_color()));
}

#[test]
fn the_expanded_cell_lists_each_hook_with_its_time_and_file_then_what_it_said() {
    // The user's own example, in Ctrl+O: the command that ran, then what it
    // said — attributable when several hooks ran.
    let report = stop(vec![
        ran("./notify.sh", 9, Vec::new()),
        check_git(vec![HookSaid::Feedback(FEEDBACK.into())]),
    ]);
    assert_eq!(
        texts(&hook_report_full_lines(&report, 80)),
        vec![
            "● Ran 2 Stop hooks",
            "  ⎿  ./notify.sh (9ms)",
            "     Config: ~/.alter-zero/hooks.json",
            "  ⎿  bash \"$HOME/.alter-zero/hooks/check-git.sh\" (412ms)",
            "     Config: ~/.alter-zero/hooks.json",
            "  ⎿  Stop hook feedback: There are uncommitted Git changes.",
        ]
    );
}

#[test]
fn the_expanded_cell_shows_the_context_a_hook_added_and_never_folds() {
    let report = HookReport {
        event: "UserPromptSubmit".to_string(),
        hooks: vec![RanHook {
            config: None,
            ..ran(
                "./branch.sh",
                1_250,
                vec![HookSaid::Context {
                    row: "Context added by UserPromptSubmit hook".into(),
                    text: "branch: main\nahead by 2".into(),
                }],
            )
        }],
        warnings: Vec::new(),
    };
    let lines = hook_report_full_lines(&report, 80);
    assert_eq!(
        texts(&lines),
        vec![
            "● Ran 1 UserPromptSubmit hook",
            "  ⎿  ./branch.sh (1.2s)",
            "  ⎿  Context added by UserPromptSubmit hook",
            "     branch: main",
            "     ahead by 2",
        ],
        "a hook from no file has no Config row"
    );
    assert_eq!(
        style_of(&lines[3], "branch: main").fg,
        Some(tool_dim_color())
    );
}

#[test]
fn a_tab_in_what_a_hook_said_is_spaced_out_not_lost() {
    // Hook output is often a tool's (`git status` indents with tabs), and a
    // tab paints as nothing — `modified:src/a.rs` — so the cell spaces it
    // out the way a tool's output is, inline and in Ctrl+O alike.
    let report = stop(vec![check_git(vec![
        HookSaid::Feedback("Stop hook feedback: commit first:\n\tmodified:\tsrc/a.rs".into()),
        HookSaid::Context {
            row: "Context added by Stop hook".into(),
            text: "\tbranch\tmain".into(),
        },
    ])]);
    for rows in [
        texts(&hook_report_lines(&report, 80)),
        texts(&hook_report_full_lines(&report, 80)),
    ] {
        assert!(rows.iter().all(|row| !row.contains('\t')), "{rows:?}");
        assert!(
            rows.iter().any(|row| row.contains("modified:    src/a.rs")),
            "{rows:?}"
        );
    }
    assert!(
        texts(&hook_report_full_lines(&report, 80))
            .iter()
            .any(|row| row.contains("branch    main")),
        "the context's text too"
    );
}

#[test]
fn a_hooks_time_reads_in_the_unit_that_fits() {
    assert_eq!(hook_duration(0), "0ms");
    assert_eq!(hook_duration(999), "999ms");
    assert_eq!(hook_duration(1_000), "1.0s");
    assert_eq!(hook_duration(12_345), "12.3s");
    assert_eq!(hook_duration(59_999), "59.9s");
    assert_eq!(hook_duration(65_000), "1m 5s");
}

#[test]
fn a_cell_with_only_a_config_problem_counts_no_hooks() {
    let report = HookReport {
        event: "Stop".to_string(),
        hooks: Vec::new(),
        warnings: vec!["hooks.json: Stop handler of type \"prompt\" is skipped".into()],
    };
    assert_eq!(
        texts(&hook_report_lines(&report, 80)),
        vec![
            "● Ran 0 Stop hooks (ctrl+o to expand)",
            "  ⎿  hooks.json: Stop handler of type \"prompt\" is skipped",
        ]
    );
}
