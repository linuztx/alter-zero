//! The Ctrl+D view's classifier page (`docs/state/permissions.md`).

use super::*;
use crate::llm::classifier::{ACTION_TO_REVIEW_HEADER, CLASSIFIER_SYSTEM_PROMPT};
use crate::permission::PermissionMode;
use crate::ui::classifier_view::{PromptRow, abridge_prompt};
use crate::ui::theme::{
    CLASSIFIER_ACTION_PLACEHOLDER, CLASSIFIER_PROMPT_PEEK_COLS, CLASSIFIER_VIEW_EMPTY,
    CLASSIFIER_VIEW_NOTE_INACTIVE,
};

/// The rendered rows of the view's body, trimmed for comparison.
fn rows(app: &App) -> Vec<String> {
    classifier_lines(app, 80)
        .iter()
        .map(|l| plain(l).trim_end().to_string())
        .collect()
}

/// A body row's text without its `⎿` corner or the indent under it.
fn body(row: &str) -> Option<&str> {
    row.strip_prefix("  ⎿  ")
        .or_else(|| row.strip_prefix("     "))
}

/// Whether some body row reads exactly `text`.
fn shows(texts: &[String], text: &str) -> bool {
    texts.iter().any(|t| body(t) == Some(text))
}

/// Where the entry whose chip reads `label` opens.
fn chip_at(texts: &[String], label: &str) -> Option<usize> {
    texts
        .iter()
        .position(|t| t.starts_with(&format!(" {label} ")))
}

#[test]
fn the_injected_block_shows_verbatim_under_an_auto_mode_note() {
    // What the classifier reads, shown exactly as it is sent — never the
    // markdown renderer, so the `##` headers and the `> ` quoting stay —
    // as the USER entry its sibling page gives a user message, since that is
    // what the block is.
    let mut app = App::new();
    app.set_permission_mode(Some(PermissionMode::Auto));
    app.set_classifier_context(Some(
        "## Task context\nUser request:\n> Improve the project\n\nActions taken this turn:\n- Read(/p/a.rs)"
            .to_string(),
    ));
    let texts = rows(&app);
    assert!(
        texts.iter().any(|t| t.contains("Auto mode")),
        "the note says the log is live: {texts:?}"
    );
    assert!(chip_at(&texts, "USER").is_some(), "{texts:?}");
    assert!(shows(&texts, "## Task context"), "{texts:?}");
    assert!(shows(&texts, "> Improve the project"), "{texts:?}");
    assert!(shows(&texts, "- Read(/p/a.rs)"), "{texts:?}");
}

#[test]
fn the_page_opens_each_message_on_its_role_chip() {
    // The sibling page's entries, so the two read as one view: the rubric
    // on a SYSTEM chip that says it is abridged, the task context on a USER
    // chip — and the rows ←/→ step between.
    let mut app = App::new();
    app.set_permission_mode(Some(PermissionMode::Auto));
    app.set_classifier_system_prompt(Some(shaped_prompt()));
    app.set_classifier_context(Some("## Task context\n- Read(/p/a.rs)".to_string()));
    let page = classifier_body(&app, 80);
    let texts: Vec<String> = page
        .lines
        .iter()
        .map(|l| plain(l).trim_end().to_string())
        .collect();
    let system = chip_at(&texts, "SYSTEM").expect("the rubric's entry");
    let user = chip_at(&texts, "USER").expect("the task context's entry");
    assert_eq!(
        texts[system], " SYSTEM  #1 · classifier prompt · abridged",
        "{texts:?}"
    );
    assert_eq!(texts[user], " USER  #2 · task context", "{texts:?}");
    assert_eq!(page.headers, vec![system, user]);
    assert_eq!(
        page.summary, None,
        "an abridged prompt's size is not what is sent"
    );
}

#[test]
fn the_pages_own_sentences_break_between_words() {
    // The mode note and the placeholders are prose; a narrow terminal used
    // to cut them mid-word (`… (` / `shift+tab …`, `fill` / ` it.`).
    let mut app = App::new();
    app.set_permission_mode(Some(PermissionMode::Manual));
    for (context, sentences) in [
        (
            None,
            vec![CLASSIFIER_VIEW_NOTE_INACTIVE, CLASSIFIER_VIEW_EMPTY],
        ),
        (
            Some("## Task context".to_string()),
            vec![CLASSIFIER_VIEW_NOTE_INACTIVE, CLASSIFIER_ACTION_PLACEHOLDER],
        ),
    ] {
        app.set_classifier_context(context);
        let words: Vec<String> = classifier_lines(&app, 50)
            .iter()
            .flat_map(|l| {
                let text = plain(l);
                let text = body(&text).map_or(text.clone(), str::to_string);
                text.split_whitespace()
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .collect();
        for sentence in sentences {
            for word in sentence.split_whitespace() {
                assert!(
                    words.iter().any(|w| w == word),
                    "{word:?} of {sentence:?} survives whole: {words:?}"
                );
            }
        }
    }
}

#[test]
fn nothing_recorded_yet_shows_the_placeholder() {
    let app = App::new();
    assert!(
        rows(&app).iter().any(|t| t == CLASSIFIER_VIEW_EMPTY),
        "an empty log says so rather than painting a blank page"
    );
    // An injected-but-empty block reads the same way.
    let mut app = App::new();
    app.set_classifier_context(Some("   ".to_string()));
    assert!(rows(&app).iter().any(|t| t == CLASSIFIER_VIEW_EMPTY));
}

#[test]
fn a_non_auto_mode_says_the_log_is_recorded_but_unconsulted() {
    // The log accumulates in every mode — only auto mode consults it. A view
    // that didn't say so would read as "the classifier is deciding this".
    for mode in [
        PermissionMode::Manual,
        PermissionMode::Edit,
        PermissionMode::Master,
    ] {
        let mut app = App::new();
        app.set_permission_mode(Some(mode));
        app.set_classifier_context(Some("## Task context".to_string()));
        let texts = rows(&app);
        assert!(
            texts.iter().any(|t| t.contains("only in auto mode")),
            "{mode:?}: {texts:?}"
        );
        assert!(
            !texts.iter().any(|t| t.contains("Auto mode —")),
            "{mode:?} is not auto: {texts:?}"
        );
    }
}

#[test]
fn permissions_disabled_says_no_classifier_runs() {
    let mut app = App::new();
    app.set_permission_mode(None);
    app.set_classifier_context(Some("## Task context".to_string()));
    assert!(
        rows(&app).iter().any(|t| t.contains("disabled")),
        "with no gate there is no classifier at all"
    );
}

#[test]
fn a_long_action_line_wraps_instead_of_clipping() {
    // The block's own truncation bounds what the model reads; the view still
    // must not drop columns off the right edge of a narrow terminal.
    let mut app = App::new();
    app.set_permission_mode(Some(PermissionMode::Auto));
    let long = format!("- Bash({})", "x".repeat(200));
    app.set_classifier_context(Some(format!("## Task context\n{long}")));
    let texts: Vec<String> = classifier_lines(&app, 40)
        .iter()
        .map(|l| plain(l).trim_end().to_string())
        .collect();
    assert!(
        texts.iter().all(|t| crate::ui::wrap::cols(t) <= 40),
        "every row fits the width: {texts:?}"
    );
    let joined: String = texts.iter().map(|t| t.trim_start()).collect();
    assert!(
        joined.contains(&"x".repeat(200)),
        "and the whole line survives the wrap"
    );
}

// ===== the system prompt, abridged to its structure =====

/// A prompt shaped like the real one: a preamble, then headed sections whose
/// opening paragraphs run long.
fn shaped_prompt() -> String {
    format!(
        "You are the reviewer. {}\n\nMore preamble.\n- a rule\n- another rule\n\n## First section\n\nOpening line of the first section. {}\n\nBlock:\n- one\n- two\n\n## Second section\n\nShort opener.\n\n## Output Format\n\nIf blocked:\n<block>yes</block>\n\nIf allowed:\n<block>no</block>",
        "intro ".repeat(60),
        "detail ".repeat(60),
    )
}

#[test]
fn the_system_prompt_leads_the_page_abridged_under_its_own_tag() {
    // The rubric every verdict is judged by shows above the task context,
    // tagged like the sibling page's system prompt — but abridged: the
    // headings whole, each section's opening paragraph cut at the budget,
    // the rest folded into a counted `… +N lines` row, so the page shows
    // what the classifier is told without a wall of prose above the live
    // block it exists to show.
    let mut app = App::new();
    app.set_permission_mode(Some(PermissionMode::Auto));
    app.set_classifier_system_prompt(Some(shaped_prompt()));
    app.set_classifier_context(Some("## Task context\n- Read(/p/a.rs)".to_string()));
    let texts = rows(&app);
    let tag_at = chip_at(&texts, "SYSTEM").expect("the system prompt leads");
    let user_at = chip_at(&texts, "USER").expect("the task context follows as the user message");
    assert!(tag_at < user_at, "prompt before context: {texts:?}");
    // Every heading survives whole, hung on the entry's gutter.
    for heading in ["## First section", "## Second section", "## Output Format"] {
        assert!(shows(&texts, heading), "{heading}: {texts:?}");
    }
    // A long opening paragraph is cut and says so — the wrapped rows of the
    // cut line reassemble to its head closed with `…`, never the whole
    // paragraph — and the section's remaining lines are counted, not shown.
    let joined: String = texts.iter().map(|t| body(t).unwrap_or(t)).collect();
    assert!(
        joined.contains("Opening line of the first section. detail"),
        "the opener shows: {texts:?}"
    );
    let opener_at = texts
        .iter()
        .position(|t| body(t).is_some_and(|b| b.starts_with("Opening line of the first section.")))
        .unwrap();
    let marker_at = opener_at
        + texts[opener_at..]
            .iter()
            .position(|t| body(t) == Some("… +3 lines"))
            .expect("the section's fold follows its opener");
    assert!(
        texts[opener_at..marker_at].iter().any(|t| t.ends_with('…')),
        "and is cut: {texts:?}"
    );
    assert!(
        !joined.contains(&"detail ".repeat(60)),
        "never whole: {texts:?}"
    );
    assert!(!texts.iter().any(|t| t.contains("- one")), "{texts:?}");
    // Three sections fold lines — the preamble's three, the first section's
    // three, the output format's two — each behind its own counted row.
    let folds = |marker: &str| texts.iter().filter(|t| body(t) == Some(marker)).count();
    assert_eq!(folds("… +3 lines"), 2, "{texts:?}");
    assert_eq!(folds("… +2 lines"), 1, "{texts:?}");
    assert!(
        !texts.iter().any(|t| t.contains("More preamble")),
        "{texts:?}"
    );
    // …while a short section shows its opener with nothing to fold.
    assert!(shows(&texts, "Short opener."), "{texts:?}");
    // The block itself is untouched in its own entry.
    assert!(shows(&texts, "- Read(/p/a.rs)"), "{texts:?}");
}

#[test]
fn the_user_message_closes_on_the_action_slot() {
    // The classifier's user message is the context block and then the one
    // action under `## Action to review`; the action is only known when a
    // verdict is asked, so the page shows the header over a dim placeholder
    // — the request's real shape, with the one part that varies named.
    let mut app = App::new();
    app.set_permission_mode(Some(PermissionMode::Auto));
    app.set_classifier_system_prompt(Some(shaped_prompt()));
    app.set_classifier_context(Some("## Task context\n- Read(/p/a.rs)".to_string()));
    let texts = rows(&app);
    let block_at = texts
        .iter()
        .position(|t| body(t) == Some("- Read(/p/a.rs)"))
        .unwrap();
    let header_at = texts
        .iter()
        .position(|t| body(t) == Some(ACTION_TO_REVIEW_HEADER))
        .expect("the action header closes the user message");
    assert!(block_at < header_at, "{texts:?}");
    assert_eq!(texts[header_at - 1], "", "a blank divides them: {texts:?}");
    assert_eq!(
        body(&texts[header_at + 1]),
        Some(CLASSIFIER_ACTION_PLACEHOLDER),
        "{texts:?}"
    );
    // The last thing on the page — only the entry's spacer trails the
    // placeholder.
    assert_eq!(texts[header_at + 2], "", "{texts:?}");
    assert_eq!(texts.len(), header_at + 3, "{texts:?}");
}

#[test]
fn no_prompt_injected_shows_no_prompt_section() {
    // The dummy keeps no classifier at all (its auto-mode demo answers from a
    // heuristic), so it injects no prompt — and the page must not invent
    // one: no SYSTEM entry, straight from the note to the placeholder.
    let mut app = App::new();
    app.set_permission_mode(Some(PermissionMode::Auto));
    let texts = rows(&app);
    assert!(chip_at(&texts, "SYSTEM").is_none(), "{texts:?}");
    assert!(
        texts.iter().any(|t| t == CLASSIFIER_VIEW_EMPTY),
        "{texts:?}"
    );
    // A blank prompt reads as none.
    app.set_classifier_system_prompt(Some("  \n".to_string()));
    assert!(chip_at(&rows(&app), "SYSTEM").is_none(), "{texts:?}");
}

#[test]
fn a_prompt_without_a_context_still_shows_above_the_placeholder() {
    // An agent the registry no longer holds has no block to show, but the
    // rubric it was judged by is still a fact about the backend.
    let mut app = App::new();
    app.set_permission_mode(Some(PermissionMode::Auto));
    app.set_classifier_system_prompt(Some("## Only section\n\nA rule.".to_string()));
    let texts = rows(&app);
    let tag_at = chip_at(&texts, "SYSTEM").expect("the prompt shows");
    let empty_at = texts
        .iter()
        .position(|t| t == CLASSIFIER_VIEW_EMPTY)
        .expect("the placeholder stands in for the block");
    assert!(tag_at < empty_at, "{texts:?}");
    assert!(chip_at(&texts, "USER").is_none(), "{texts:?}");
}

#[test]
fn the_abridged_prompt_wraps_to_the_width() {
    let mut app = App::new();
    app.set_classifier_system_prompt(Some(shaped_prompt()));
    app.set_classifier_context(Some("## Task context".to_string()));
    let texts: Vec<String> = classifier_lines(&app, 40)
        .iter()
        .map(|l| plain(l).trim_end().to_string())
        .collect();
    assert!(
        texts.iter().all(|t| crate::ui::wrap::cols(t) <= 40),
        "every row fits the width: {texts:?}"
    );
}

#[test]
fn abridge_keeps_headings_opening_paragraphs_and_counts_the_rest() {
    let prompt = "Intro line.\nStill intro.\n\nSecond paragraph.\n\n## Section\n\nOpener.\n\n- a\n- b\n\n## Bare\n\nOnly line.";
    let rows = abridge_prompt(prompt, 200);
    assert_eq!(
        rows,
        vec![
            PromptRow::Text("Intro line.".to_string()),
            PromptRow::Text("Still intro.".to_string()),
            PromptRow::Elided(1),
            PromptRow::Blank,
            PromptRow::Text("## Section".to_string()),
            PromptRow::Text("Opener.".to_string()),
            PromptRow::Elided(2),
            PromptRow::Blank,
            PromptRow::Text("## Bare".to_string()),
            PromptRow::Text("Only line.".to_string()),
        ]
    );
}

#[test]
fn abridge_cuts_an_opening_paragraph_at_the_budget() {
    // The budget spans the paragraph: whole lines while they fit, the line
    // that overflows cut and closed with `…`, and anything after it in the
    // paragraph counted with the section's remainder.
    let prompt =
        "## S\n\nshort line\nthis line is far too long for what is left\nthird line\n\nafter";
    let rows = abridge_prompt(prompt, 20);
    assert_eq!(
        rows,
        vec![
            PromptRow::Text("## S".to_string()),
            PromptRow::Text("short line".to_string()),
            PromptRow::Text("this line…".to_string()),
            PromptRow::Elided(2),
        ]
    );
    // A single over-long line is the same cut, on its own.
    let rows = abridge_prompt("abcdefghijklmnopqrstuvwxyz", 10);
    assert_eq!(rows, vec![PromptRow::Text("abcdefghi…".to_string())]);
}

#[test]
fn abridge_skips_blank_lead_ins_and_an_empty_prompt() {
    assert_eq!(abridge_prompt("", 100), Vec::<PromptRow>::new());
    assert_eq!(abridge_prompt("\n\n  \n", 100), Vec::<PromptRow>::new());
    // Blanks between a heading and its first paragraph are not content.
    assert_eq!(
        abridge_prompt("\n\n## H\n\n\nfirst\n", 100),
        vec![
            PromptRow::Text("## H".to_string()),
            PromptRow::Text("first".to_string()),
        ]
    );
    // Any heading depth opens a section; a `#` that isn't one does not.
    let rows = abridge_prompt("# Top\n\n### Deep\n\n#hashtag not a heading", 100);
    assert_eq!(
        rows,
        vec![
            PromptRow::Text("# Top".to_string()),
            PromptRow::Blank,
            PromptRow::Text("### Deep".to_string()),
            PromptRow::Text("#hashtag not a heading".to_string()),
        ]
    );
}

#[test]
fn the_real_prompt_abridges_to_its_four_sections() {
    // The feature's promise against the prompt it exists for: the page shows
    // the rubric's structure — its opening sentence, the three headed
    // sections, the output contract's first shape — in a fraction of the
    // rows, with every fold counted.
    let rows = abridge_prompt(CLASSIFIER_SYSTEM_PROMPT.trim(), CLASSIFIER_PROMPT_PEEK_COLS);
    let texts: Vec<String> = rows
        .iter()
        .filter_map(|r| match r {
            PromptRow::Text(t) => Some(t.clone()),
            _ => None,
        })
        .collect();
    assert!(
        texts[0].starts_with("You are the safety classifier"),
        "{texts:?}"
    );
    for heading in ["## Shell commands", "## MCP tool calls", "## Output Format"] {
        assert!(texts.iter().any(|t| t == heading), "{heading}: {texts:?}");
    }
    assert!(
        texts.iter().any(|t| t.contains("<block>yes</block>")),
        "the output contract's shape survives: {texts:?}"
    );
    let folded: usize = rows
        .iter()
        .map(|r| match r {
            PromptRow::Elided(n) => *n,
            _ => 0,
        })
        .sum();
    let shown = texts.len();
    let content_lines = CLASSIFIER_SYSTEM_PROMPT
        .lines()
        .filter(|l| !l.trim().is_empty())
        .count();
    assert!(
        folded > shown,
        "most of the prompt folds away: {shown} shown, {folded} folded"
    );
    // Every content line is either shown or counted — nothing vanishes
    // silently (a cut line still counts as shown).
    assert_eq!(shown + folded, content_lines, "{texts:?}");
}

#[test]
fn every_classifier_row_fits_even_a_tiny_terminal() {
    let mut app = App::new();
    app.set_classifier_system_prompt(Some(shaped_prompt()));
    for context in [
        None,
        Some("## Task context\n> Check the project".to_string()),
    ] {
        app.set_classifier_context(context);
        assert!(classifier_lines(&app, 0).is_empty());
        for width in [1, 2, 3, 4, 8, 12, 20, 40, 80] {
            for line in classifier_lines(&app, width) {
                let text = plain(&line);
                assert!(
                    crate::ui::wrap::cols(&text) <= usize::from(width),
                    "row exceeds {width} columns: {text:?}"
                );
            }
        }
    }
}

#[test]
fn abridging_counts_unicode_display_columns() {
    assert_eq!(
        abridge_prompt("## Rules\n\n界界界界\nnext line\n\nhidden", 5),
        vec![
            PromptRow::Text("## Rules".to_string()),
            PromptRow::Text("界界…".to_string()),
            PromptRow::Elided(2),
        ]
    );
}
