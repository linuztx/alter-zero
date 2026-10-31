//! The facts inside dim meta text, picked out in the theme's emphasis colour
//! (`docs/emphasis.md`).

use super::*;
use crate::ui::emphasis::{emphasized, fact_ranges, meta_row, meta_spans};
use crate::ui::theme::*;

/// The facts `fact_ranges` finds in `text`, as the text they cover.
fn facts(text: &str) -> Vec<&str> {
    fact_ranges(text)
        .into_iter()
        .map(|range| &text[range])
        .collect()
}

#[test]
fn a_hint_names_its_key() {
    assert_eq!(facts(" (ctrl+o to expand)"), ["ctrl+o"]);
    assert_eq!(facts("(ctrl+b to run in background)"), ["ctrl+b"]);
    assert_eq!(facts(" · esc to interrupt)"), ["esc"]);
    assert_eq!(facts("shift+tab to switch"), ["shift+tab"]);
    assert_eq!(facts("press alt+↑ to pull it back"), ["alt+↑"]);
    assert_eq!(facts("tab for classifier context"), ["tab"]);
    assert_eq!(facts("enter to accept"), ["enter"]);
}

#[test]
fn an_arrow_is_a_key_only_where_a_hint_says_what_it_does() {
    // `(↓ to manage)` names the key that opens the manager…
    assert_eq!(
        facts("Running in the background (↓ to manage)"),
        ["↓"],
        "the key the hint teaches"
    );
    assert_eq!(facts(" (↓ to manage · ctrl+o to expand)"), ["↓", "ctrl+o"]);
    // …while the status line's `↓ 1.2k tokens` is the tally's direction,
    // which is no key at all: only the count is a fact there.
    assert_eq!(facts(" · ↓ 1.2k tokens"), ["1.2k"]);
    assert_eq!(facts(" · ↑ 842 tokens"), ["842"]);
}

#[test]
fn every_number_a_summary_reports_is_a_fact() {
    assert_eq!(
        facts("Worked for 12s · 6.1k tokens (2.8k cached · 1.2k written) · 1 shell still running"),
        ["12s", "6.1k", "2.8k", "1.2k", "1"]
    );
    assert_eq!(
        facts("Thought for 1m 5s · 1.5k tokens"),
        ["1m", "5s", "1.5k"]
    );
    assert_eq!(
        facts("     … +42 lines (ctrl+o to expand)"),
        ["+42", "ctrl+o"]
    );
    assert_eq!(
        facts("+12 lines (22s · wait 1m 50s)"),
        ["+12", "22s", "1m", "50s"]
    );
    assert_eq!(
        facts("Done (3 tool uses · 12.3k tokens · 45s)"),
        ["3", "12.3k", "45s"]
    );
    assert_eq!(facts("Worked for 8s · ≈24.3k tokens"), ["8s", "≈24.3k"]);
    assert_eq!(facts("context at 4.5%"), ["4.5%"]);
    assert_eq!(facts("120k → 8k tokens · 1.2M"), ["120k", "8k", "1.2M"]);
}

#[test]
fn a_digit_inside_a_word_is_not_a_number() {
    // A server named `context7`, a model `gpt-5.5`, a version `v0.17.0`, a
    // task `#1` — a name that happens to hold a digit is no count, and a
    // half-built number (`1.`, `3a1`) is no number either.
    for text in [
        "Called context7",
        "gpt-5.5",
        "Alter Zero (v0.17.0)",
        "blocked by #1",
        "1.",
        "3a1",
        "escape the table, entered it",
        "control+o",
    ] {
        assert!(
            facts(text).is_empty(),
            "no fact in {text:?}: {:?}",
            facts(text)
        );
    }
}

#[test]
fn meta_spans_emphasize_the_facts_and_keep_the_rest_dim() {
    let dim = Style::new().fg(tool_dim_color());
    let spans = meta_spans("Thought for 3s · 120 tokens (ctrl+o to expand)", dim);
    let text: String = spans.iter().map(|span| span.content.as_ref()).collect();
    assert_eq!(
        text, "Thought for 3s · 120 tokens (ctrl+o to expand)",
        "the text is untouched — only its colours change"
    );
    assert_eq!(emphasized_texts(&spans), ["3s", "120", "ctrl+o"]);
    for span in &spans {
        let fg = span.style.fg;
        assert!(
            fg == Some(tool_dim_color()) || fg == Some(emphasis_color()),
            "dim or emphasized, nothing else: {span:?}"
        );
    }
    // A line with no fact stays one span, exactly as it was.
    assert_eq!(
        meta_spans("Interrupted by user", dim),
        vec![Span::styled("Interrupted by user", dim)]
    );
}

#[test]
fn an_emphasized_fact_keeps_the_modifiers_of_its_line() {
    // The settled group line bolds its counts (Claude Code's look); the
    // emphasis recolours them without dropping the bold.
    let bold_dim = Style::new()
        .fg(tool_dim_color())
        .add_modifier(Modifier::BOLD);
    assert_eq!(
        emphasized(bold_dim),
        Style::new()
            .fg(emphasis_color())
            .add_modifier(Modifier::BOLD)
    );
    let spans = meta_spans("Read 2 files", bold_dim);
    assert_eq!(emphasized_texts(&spans), ["2"]);
    assert!(
        spans
            .iter()
            .all(|span| span.style.add_modifier.contains(Modifier::BOLD))
    );
}

#[test]
fn a_meta_row_is_a_gutter_row_whose_facts_are_emphasized() {
    let row = meta_row(0, "Running… (10s · wait 2m)".to_string());
    assert_eq!(plain(&row), "  ⎿  Running… (10s · wait 2m)");
    assert_eq!(
        row.spans[0].style.fg,
        Some(tool_dim_color()),
        "the corner stays dim"
    );
    assert_eq!(emphasized_texts(&row.spans), ["10s", "2m"]);
    // A continuation row indents under the corner, like `result_row`'s.
    let hint = meta_row(1, "(ctrl+b to run in background)".to_string());
    assert_eq!(plain(&hint), "     (ctrl+b to run in background)");
    assert_eq!(emphasized_texts(&hint.spans), ["ctrl+b"]);
}

#[test]
fn the_emphasis_follows_the_active_theme() {
    for theme in crate::app::Theme::ALL {
        with_theme(theme, || {
            let spans = meta_spans("Worked for 12s", Style::new().fg(tool_dim_color()));
            assert_eq!(
                spans.last().and_then(|span| span.style.fg),
                Some(emphasis_color()),
                "{theme:?}"
            );
        });
    }
}
