//! The Ctrl+D view's classifier page (`docs/permissions.md`).

use super::*;
use crate::permission::PermissionMode;
use crate::ui::theme::CLASSIFIER_VIEW_EMPTY;

/// The rendered rows of the view's body, trimmed for comparison.
fn rows(app: &App) -> Vec<String> {
    classifier_lines(app, 80)
        .iter()
        .map(|l| plain(l).trim_end().to_string())
        .collect()
}

#[test]
fn the_injected_block_shows_verbatim_under_an_auto_mode_note() {
    // What the classifier reads, shown exactly as it is sent — never the
    // markdown renderer, so the `##` headers and the `> ` quoting stay.
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
    assert!(texts.iter().any(|t| t == "## Task context"), "{texts:?}");
    assert!(
        texts.iter().any(|t| t == "> Improve the project"),
        "{texts:?}"
    );
    assert!(texts.iter().any(|t| t == "- Read(/p/a.rs)"), "{texts:?}");
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
    let joined: String = texts.concat();
    assert!(
        joined.contains(&"x".repeat(200)),
        "and the whole line survives the wrap"
    );
}
