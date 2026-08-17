//! The inline `/login` onboarding (`docs/llm.md`).

use super::*;

/// The key step's whole page height (see `key_onboarding_lines`).
const LOGIN_KEY_ROWS: u16 = 9;
use crate::ui::login_view::login_key_prompt;
use crate::ui::theme::{
    ERROR_COLOR, LOGIN_KEY_INPUT_ROW, LOGIN_KEY_PROMPT_COLOR, LOGIN_SEARCH_ROW,
    MODEL_SELECTED_COLOR,
};

#[test]
fn model_rows_show_marker_provider_tag_and_active_check() {
    // Selected = row 0, active = row 1 (claude-fable-5).
    let picker = model_picker(three_models(), 0, "anthropic/claude-fable-5");
    let mut buf = buffer(60, 20);
    render_model_picker(buf.area, &mut buf, &picker);
    // First list row (y = MODEL_SEARCH_ROW + 2 = 4).
    let first = row(&buf, 4, 60);
    assert!(first.starts_with("→ anthropic/claude-3-haiku"), "{first:?}");
    assert!(first.contains("[openrouter]"), "provider tag: {first:?}");
    // The selected marker is cyan.
    assert_eq!(buf[(0, 4)].fg, MODEL_SELECTED_COLOR);
    // The active model (row 1, y=5) carries the ✓.
    let second = row(&buf, 5, 60);
    assert!(second.contains('✓'), "active model has a check: {second:?}");
}

#[test]
fn counter_notes_more_providers_still_loading() {
    // A partial list (one provider in, another still fetching) shows the
    // list now with a dim "loading more…" hint beside the counter.
    let mut picker = model_picker(three_models(), 0, "x");
    picker.pending = 1;
    let mut buf = buffer(60, 12);
    render_model_picker(buf.area, &mut buf, &picker);
    let counter = row(&buf, 7, 60);
    assert!(counter.contains("(1/3)"), "{counter:?}");
    assert!(counter.contains("loading more"), "{counter:?}");
}

#[test]
fn counter_notes_a_provider_that_failed() {
    let mut picker = model_picker(three_models(), 0, "x");
    picker.errors.push(ModelFetchError {
        provider: "Agent Zero API".into(),
        message: "HTTP 401".into(),
    });
    let mut buf = buffer(60, 12);
    render_model_picker(buf.area, &mut buf, &picker);
    let counter = row(&buf, 7, 60);
    assert!(counter.contains("Agent Zero API"), "{counter:?}");
    assert!(counter.contains("unavailable"), "{counter:?}");
}

#[test]
fn all_failed_picker_shows_one_red_row_per_provider() {
    let mut picker = model_picker(vec![], 0, "x");
    picker.status = ModelLoad::Error(String::new());
    picker.errors = vec![
        ModelFetchError {
            provider: "OpenRouter".into(),
            message: "HTTP 500".into(),
        },
        ModelFetchError {
            provider: "Agent Zero API".into(),
            message: "HTTP 401".into(),
        },
    ];
    // Natural height: collapsed chrome (6) + 2 error rows = 8.
    let mut buf = buffer(60, 8);
    render_model_picker(buf.area, &mut buf, &picker);
    let r0 = row(&buf, 4, 60);
    let r1 = row(&buf, 5, 60);
    assert!(r0.contains("OpenRouter"), "{r0:?}");
    assert!(r0.contains("HTTP 500"), "{r0:?}");
    assert!(r1.contains("Agent Zero API"), "{r1:?}");
    assert_eq!(buf[(2, 4)].fg, ERROR_COLOR, "error rows are red");
}

#[test]
fn render_login_provider_step_frames_lists_and_marks_configured() {
    let app = login_app_provider();
    let onboarding = app.key_onboarding.as_ref().unwrap();
    // Natural height: 9 chrome + 2 provider rows = 11.
    let mut buf = buffer(60, 11);
    render_key_onboarding(buf.area, &mut buf, onboarding);
    assert!(row(&buf, 0, 60).starts_with('─'), "top rule");
    // Headerless (like /model): row 1 is a blank gap, not a banner.
    assert!(row(&buf, 1, 60).trim().is_empty(), "no header banner");
    // The `❯` filter line.
    assert!(row(&buf, LOGIN_SEARCH_ROW, 60).contains('❯'));
    // First provider row (y = LOGIN_SEARCH_ROW + 2 = 4): selected →, env tag,
    // and a green ✓ because OpenRouter is configured.
    let first = row(&buf, 4, 60);
    assert!(first.starts_with("→ OpenRouter"), "{first:?}");
    assert!(first.contains("[OPENROUTER_API_KEY]"), "{first:?}");
    assert!(
        first.contains('✓'),
        "configured provider has a check: {first:?}"
    );
    // Together AI (row 1, y=5) is not configured → no check.
    assert!(!row(&buf, 5, 60).contains('✓'));
    // The hint (row 8: counter(6) gap(7) hint(8)) names the real .env path.
    assert!(
        row(&buf, 8, 60).contains("Keys are saved to ~/.alter-zero/.env"),
        "hint: {:?}",
        row(&buf, 8, 60)
    );
}

#[test]
fn render_login_key_step_masks_the_entered_key() {
    let mut app = login_app_key();
    // Chose the highlighted provider (OpenRouter) and typed a key.
    app.key_onboarding.as_mut().unwrap().key_input = "sk-secret-1234".into();
    let onboarding = app.key_onboarding.as_ref().unwrap();
    assert_eq!(onboarding.step, KeyStep::Key);
    let mut buf = buffer(60, LOGIN_KEY_ROWS);
    render_key_onboarding(buf.area, &mut buf, onboarding);
    // Periwinkle prompt (row 2, after the top rule + gap) names the provider.
    assert!(row(&buf, 2, 60).contains("Enter your OpenRouter API key"));
    assert_eq!(
        buf[(2, 2)].fg,
        LOGIN_KEY_PROMPT_COLOR,
        "prompt is periwinkle"
    );
    // The field is masked: dots, never the plaintext key.
    let field = row(&buf, LOGIN_KEY_INPUT_ROW, 60);
    assert!(field.contains('•'), "masked: {field:?}");
    assert!(
        !field.contains("sk-secret"),
        "no plaintext leaks: {field:?}"
    );
}

#[test]
fn login_key_prompt_avoids_a_doubled_api() {
    // A provider whose name already ends in "API" doesn't gain a second one.
    assert_eq!(
        login_key_prompt("Agent Zero API"),
        "Enter your Agent Zero API key"
    );
    // A normal name still gets the "API key" suffix.
    assert_eq!(
        login_key_prompt("OpenRouter"),
        "Enter your OpenRouter API key"
    );
}

#[test]
fn render_login_key_step_shows_a_placeholder_when_empty() {
    let app = login_app_key();
    let onboarding = app.key_onboarding.as_ref().unwrap();
    let mut buf = buffer(60, LOGIN_KEY_ROWS);
    render_key_onboarding(buf.area, &mut buf, onboarding);
    assert!(row(&buf, LOGIN_KEY_INPUT_ROW, 60).contains("paste your API key"));
}

#[test]
fn render_live_shows_the_onboarding_when_open() {
    let app = login_app_provider();
    // Its natural height, like the boundary paints it (the flow is pinned at
    // the region's bottom — the strip above it owns any slack).
    let mut buf = buffer(60, key_onboarding_height(&app, 60, 40).unwrap());
    render_live(buf.area, &mut buf, &app);
    // The onboarding stands in for the composer: top rule, `❯` filter, and
    // the provider list (headerless — no banner on row 1).
    assert!(row(&buf, 0, 60).starts_with('─'), "top rule");
    assert!(row(&buf, LOGIN_SEARCH_ROW, 60).contains('❯'), "filter line");
    assert!(row(&buf, 4, 60).contains("OpenRouter"), "a provider row");
}

#[test]
fn cursor_tracks_the_login_filter_then_the_masked_key() {
    let mut app = login_app_provider();
    app.key_onboarding.as_mut().unwrap().query = "tog".into();
    let area = Rect::new(0, 0, 60, key_onboarding_height(&app, 60, 40).unwrap());
    let (x, y) = cursor_position(area, &app);
    // indent(2) + "❯ "(2) + "tog"(3) = 7 on the filter row.
    assert_eq!((x, y), (7, LOGIN_SEARCH_ROW));
    // Advance to the key step and type: the cursor tracks the mask length.
    {
        let onboarding = app.key_onboarding.as_mut().unwrap();
        onboarding.step = KeyStep::Key;
        onboarding.chosen = Some(0);
        onboarding.key_input = "abcd".into();
    }
    let area = Rect::new(0, 0, 60, key_onboarding_height(&app, 60, 40).unwrap());
    let (x, y) = cursor_position(area, &app);
    // indent(2) + "❯ "(2) + 4 mask glyphs = 8 on the key row.
    assert_eq!((x, y), (8, LOGIN_KEY_INPUT_ROW));
}

#[test]
fn model_picker_shows_a_login_hint_when_no_provider_is_configured() {
    let mut app = App::new();
    app.open_model_picker("x");
    app.set_models_needs_login();
    let picker = app.model_picker.as_ref().unwrap();
    let mut buf = buffer(60, 12);
    render_model_picker(buf.area, &mut buf, picker);
    // The list area (row 4) points at /login, in cyan (not a red error).
    let list = row(&buf, 4, 60);
    assert!(list.contains("run /login"), "{list:?}");
    assert_eq!(buf[(2, 4)].fg, MODEL_SELECTED_COLOR);
    // No counter or model-name line when there's nothing selectable.
    assert!(row(&buf, 7, 60).trim().is_empty(), "no counter");
    assert!(row(&buf, 9, 60).trim().is_empty(), "no model name");
}

fn login_app_key() -> App {
    let mut app = login_app_provider();
    // Advance to the masked key step for the highlighted provider (index 0).
    let onboarding = app.key_onboarding.as_mut().unwrap();
    onboarding.step = KeyStep::Key;
    onboarding.chosen = Some(0);
    app
}

// --- the no-match provider page collapses its empty counter (docs/llm.md) ---

#[test]
fn an_unmatched_provider_filter_collapses_to_placeholder_and_hint() {
    // No provider matched: the blank counter collapses into the single gap
    // that carries the placeholder to the hint (the `/model` picker's rule).
    let mut app = login_app_provider();
    app.key_onboarding.as_mut().expect("open").query = "zzz".to_string();
    let onboarding = app.key_onboarding.as_ref().expect("open");
    let texts: Vec<String> = crate::ui::login_view::key_onboarding_lines(onboarding, 78)
        .iter()
        .map(|l| plain(l).trim_end().to_string())
        .collect();
    let is_rule = |t: &str| !t.is_empty() && t.chars().all(|c| c == '─');
    assert_eq!(texts.len(), 9, "the collapsed page is 9 rows: {texts:?}");
    assert!(is_rule(&texts[0]), "{texts:?}");
    assert_eq!(texts[1], "", "{texts:?}");
    assert!(texts[2].contains('❯'), "{texts:?}");
    assert_eq!(texts[3], "", "{texts:?}");
    assert!(texts[4].contains("No matching providers"), "{texts:?}");
    assert_eq!(texts[5], "", "one gap under the placeholder: {texts:?}");
    assert!(texts[6].contains("Keys are saved"), "the hint: {texts:?}");
    assert_eq!(texts[7], "", "{texts:?}");
    assert!(is_rule(&texts[8]), "{texts:?}");
}

#[test]
fn no_login_page_ever_stacks_two_blank_rows() {
    let mut app = login_app_provider();
    for query in ["", "open", "zzz"] {
        app.key_onboarding.as_mut().expect("open").query = query.to_string();
        let onboarding = app.key_onboarding.as_ref().expect("open");
        let texts: Vec<String> = crate::ui::login_view::key_onboarding_lines(onboarding, 78)
            .iter()
            .map(|l| plain(l).trim_end().to_string())
            .collect();
        for pair in texts.windows(2) {
            assert!(
                !(pair[0].is_empty() && pair[1].is_empty()),
                "query {query:?} stacked two blank rows: {texts:?}"
            );
        }
    }
}

#[test]
fn the_env_path_hint_wraps_at_narrow_widths() {
    // "Keys are saved to {path}" names where the secret lives — the path is
    // the tail, so it was the first thing a narrow terminal lost. It sits
    // below the cursor's search row, so wrapping it moves nothing above.
    use crate::ui::login_view::key_onboarding_lines;
    let mut app = login_app_provider();
    let onboarding = app.key_onboarding.as_mut().unwrap();
    onboarding.env_path = "~/.config/alter-zero/deeply/nested/credentials.env".into();
    let lines = key_onboarding_lines(onboarding, 40);
    for line in &lines {
        assert!(
            crate::ui::wrap::cols(plain(line).trim_end()) <= 40,
            "no row leaks past the width: {:?}",
            plain(line)
        );
    }
    // A path is one long word, so the wrap hard-breaks it mid-token;
    // stripping spaces reassembles the pieces regardless of where they broke.
    let all = lines
        .iter()
        .map(plain)
        .collect::<Vec<_>>()
        .join("")
        .replace(' ', "");
    assert!(
        all.contains("credentials.env"),
        "the path's tail survives: {lines:#?}"
    );
}

#[test]
fn a_cut_provider_name_ends_with_an_ellipsis() {
    use crate::ui::login_view::key_onboarding_lines;
    let mut app = login_app_provider();
    let onboarding = app.key_onboarding.as_mut().unwrap();
    onboarding.providers[0].name = "An Extremely Long Provider Display Name".into();
    let lines = key_onboarding_lines(onboarding, 30);
    // The `[OPENROUTER_API_KEY]` tag reserves 21 columns, so at width 30 the
    // name keeps only its first few — the row must still mark the cut.
    let row = lines
        .iter()
        .map(plain)
        .find(|l| l.contains("An E"))
        .expect("the provider row");
    assert!(
        row.contains('…') && row.contains('['),
        "the name marks its cut and the tag keeps its seat: {row:?}"
    );
}
