//! The `$` skill-mention picker (`docs/skill-mentions.md`).

use super::*;

use crate::skills::SkillMetadata;

/// A minimal discovered skill for the picker tests.
fn skill(name: &str, description: &str) -> SkillMetadata {
    SkillMetadata {
        name: name.to_string(),
        description: description.to_string(),
        dir: std::path::PathBuf::from("/skills").join(name),
        path: std::path::PathBuf::from("/skills")
            .join(name)
            .join(crate::skills::SKILL_FILE_NAME),
    }
}

/// An app that knows about the two demo skills (the boundary's
/// `set_skills` injection).
fn skilled_app() -> App {
    let mut app = App::new();
    app.set_skills(vec![
        skill("dataviz", "Charts and dashboards"),
        skill("skill-creator", "Create or update a skill"),
    ]);
    app
}

// ===== opening =====

#[test]
fn typing_dollar_opens_the_skill_picker() {
    let mut app = skilled_app();
    type_str(&mut app, "use $da");
    assert!(app.skill_picker.is_some());
    let names: Vec<String> = app.skill_matches().into_iter().map(|m| m.name).collect();
    assert_eq!(names, vec!["dataviz".to_string()]);
}

#[test]
fn a_bare_dollar_lists_every_skill() {
    let mut app = skilled_app();
    type_str(&mut app, "$");
    assert!(app.skill_picker.is_some());
    assert_eq!(app.skill_matches().len(), 2);
}

#[test]
fn the_picker_stays_closed_with_no_skills_installed() {
    // With nothing to offer, `$` is just a dollar sign — prose about prices
    // must not raise an empty band.
    let mut app = App::new();
    type_str(&mut app, "$");
    assert!(app.skill_picker.is_none());
    assert!(app.skill_matches().is_empty());
}

#[test]
fn a_mid_word_dollar_never_opens_the_picker() {
    let mut app = skilled_app();
    type_str(&mut app, "US$5");
    assert!(app.skill_picker.is_none());
}

#[test]
fn shell_flavored_queries_keep_the_picker_closed() {
    // `$1` is a positional parameter and `$PATH` an environment variable —
    // codex's classification; the band closes the moment the query turns
    // into shell talk.
    let mut app = skilled_app();
    type_str(&mut app, "costs $1");
    assert!(app.skill_picker.is_none());

    let mut app = skilled_app();
    type_str(&mut app, "echo $PATH");
    assert!(app.skill_picker.is_none());
}

#[test]
fn the_picker_is_suppressed_in_shell_mode() {
    // A `!command`'s `$VAR` is real shell syntax, never a mention.
    let mut app = skilled_app();
    type_str(&mut app, "!echo $da");
    assert!(app.shell_mode);
    assert!(app.skill_picker.is_none());
}

// ===== dismissal =====

#[test]
fn esc_dismisses_the_picker_and_stays_dismissed_in_the_mention() {
    let mut app = skilled_app();
    type_str(&mut app, "$da");
    assert!(app.skill_picker.is_some());
    assert_eq!(app.on_key(key(KeyCode::Esc)), Action::None);
    assert!(app.skill_picker.is_none());
    // Editing within the same mention does not reopen it (sticky, like the
    // `@` picker); leaving and re-entering does.
    type_str(&mut app, "t");
    assert!(app.skill_picker.is_none());
    type_str(&mut app, " $d");
    assert!(app.skill_picker.is_some());
}

#[test]
fn leaving_the_mention_closes_the_picker() {
    let mut app = skilled_app();
    type_str(&mut app, "$dataviz");
    assert!(app.skill_picker.is_some());
    // The terminator ends the mention — codex's trailing-space close.
    type_str(&mut app, " ");
    assert!(app.skill_picker.is_none());
}

#[test]
fn ctrl_c_clearing_the_draft_closes_the_picker() {
    let mut app = skilled_app();
    type_str(&mut app, "$da");
    assert!(app.skill_picker.is_some());
    assert_eq!(app.on_key(ctrl('c')), Action::None);
    assert!(app.input.is_empty());
    assert!(app.skill_picker.is_none());
}

#[test]
fn a_composer_replacing_picker_abandons_the_band() {
    let mut app = skilled_app();
    type_str(&mut app, "$da");
    assert!(app.skill_picker.is_some());
    app.open_settings();
    assert!(app.skill_picker.is_none());
}

#[test]
fn clear_conversation_closes_the_picker() {
    // `/clear` nulls the `@` picker as belt-and-braces; the `$` picker
    // follows the same posture.
    let mut app = skilled_app();
    type_str(&mut app, "$da");
    assert!(app.skill_picker.is_some());
    app.clear_conversation();
    assert!(app.skill_picker.is_none());
}

// ===== selection =====

#[test]
fn arrows_move_the_selection_and_wrap_at_the_ends() {
    let mut app = skilled_app();
    type_str(&mut app, "$");
    assert_eq!(app.skill_picker.as_ref().unwrap().selected, 0);
    app.on_key(key(KeyCode::Down));
    assert_eq!(app.skill_picker.as_ref().unwrap().selected, 1);
    app.on_key(key(KeyCode::Down));
    assert_eq!(
        app.skill_picker.as_ref().unwrap().selected,
        0,
        "Down from the last match wraps to the first"
    );
    app.on_key(key(KeyCode::Up));
    assert_eq!(
        app.skill_picker.as_ref().unwrap().selected,
        1,
        "Up from the first wraps back to the last"
    );
    app.on_key(key(KeyCode::Up));
    assert_eq!(app.skill_picker.as_ref().unwrap().selected, 0);
}

#[test]
fn narrowing_the_query_resets_the_selection() {
    let mut app = skilled_app();
    type_str(&mut app, "$");
    app.on_key(key(KeyCode::Down));
    assert_eq!(app.skill_picker.as_ref().unwrap().selected, 1);
    type_str(&mut app, "da");
    assert_eq!(app.skill_picker.as_ref().unwrap().selected, 0);
    assert_eq!(
        app.highlighted_skill_match().map(|m| m.name),
        Some("dataviz".to_string())
    );
}

// ===== accepting =====

#[test]
fn tab_accepts_the_mention_keeping_the_dollar_prefix() {
    let mut app = skilled_app();
    type_str(&mut app, "please $da");
    assert_eq!(app.on_key(key(KeyCode::Tab)), Action::None);
    assert_eq!(app.input.text(), "please $dataviz ");
    assert_eq!(app.input.cursor(), app.input.text().len());
    assert!(app.skill_picker.is_none(), "accept closes the picker");
}

#[test]
fn enter_accepts_instead_of_submitting_while_the_picker_is_open() {
    let mut app = skilled_app();
    type_str(&mut app, "$da");
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::None);
    assert_eq!(app.input.text(), "$dataviz ");
    // The next Enter submits the completed draft as usual.
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::Submit("$dataviz ".to_string())
    );
}

#[test]
fn accepting_mid_text_reuses_the_following_space() {
    // "Hello world $skill …generate": a mention typed in front of existing
    // text. codex's advance_past_completion_separator — the accept reuses
    // the space already following the mention rather than doubling it, and
    // the cursor lands past it, ready to keep typing.
    let mut app = skilled_app();
    type_str(&mut app, "see  now");
    // Park the cursor between the two spaces and type the mention there.
    for _ in 0..4 {
        app.on_key(key(KeyCode::Left));
    }
    type_str(&mut app, "$da");
    assert_eq!(app.input.text(), "see $da now");
    assert!(app.skill_picker.is_some());
    app.on_key(key(KeyCode::Tab));
    assert_eq!(app.input.text(), "see $dataviz now");
    assert_eq!(app.input.cursor(), "see $dataviz ".len());
}

#[test]
fn enter_with_no_match_falls_through_to_submit() {
    // An unmatched `$typo` is still sendable text — the band shows its
    // placeholder but never swallows the send.
    let mut app = skilled_app();
    type_str(&mut app, "$zzz");
    assert!(app.skill_picker.is_some());
    assert!(app.highlighted_skill_match().is_none());
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::Submit("$zzz".to_string())
    );
}

// ===== exclusivity =====

#[test]
fn the_file_and_skill_pickers_are_mutually_exclusive() {
    // Typing into a `$mention` (past an earlier `@token`) opens the skill
    // band alone…
    let mut app = skilled_app();
    type_str(&mut app, "see @src stuff $da");
    assert!(app.skill_picker.is_some());
    assert!(app.file_search.is_none());

    // …and typing an `@token` opens the file band alone.
    let mut app = skilled_app();
    type_str(&mut app, "@src");
    assert!(app.file_search.is_some());
    assert!(app.skill_picker.is_none());
}

#[test]
fn moving_the_cursor_out_of_the_mention_hides_the_band() {
    // The rows derive from the *live* token, so ← out of the mention drops
    // the band (and frees ↑/↓ for history/cursor work) even before an edit
    // closes the state.
    let mut app = skilled_app();
    type_str(&mut app, "$da");
    assert!(app.skill_band_active());
    app.on_key(key(KeyCode::Home));
    assert!(!app.skill_band_active(), "cursor before the `$`");
    assert!(app.skill_matches().is_empty());
}

#[test]
fn the_palette_wins_over_the_skill_picker() {
    // A bare `/token` is the palette's; a `$` inside it is not at a word
    // boundary anyway.
    let mut app = skilled_app();
    type_str(&mut app, "/qu");
    assert!(app.command_menu.is_some());
    assert!(app.skill_picker.is_none());
}
