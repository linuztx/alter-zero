//! The inline `/login` onboarding (`docs/llm.md`).

use super::*;

/// The key step's whole page height for a provider the file describes in no
/// words (see `key_onboarding_lines`); a description block grows it.
const LOGIN_KEY_ROWS: u16 = 9;

/// The row every list step's `❯` filter sits on — top(0) gap(1) search(2).
/// Pinned *here* rather than in the theme: it is a claim about the page's
/// shape, which is a thing to test, while the paint finds its own caret seat
/// in the page it just built (`login_prompt_row`).
const LOGIN_SEARCH_ROW: u16 = 2;

/// The row the key step's `❯` field sits on when nothing stands between it
/// and the title — top(0) gap(1) title(2) gap(3) field(4). A described
/// provider pushes it down, which is the whole reason the seat is found and
/// not counted.
const LOGIN_KEY_INPUT_ROW: u16 = 4;
use crate::ui::login_view::{login_host_prompt, login_key_prompt};
use crate::ui::theme::{
    DEVICE_CURSOR_ROW, error_color, login_title_color, model_meta_color, model_selected_color,
};

/// The row the code box's top border lands on, found by content — the page's
/// shape changes with what GitHub has answered, so a pinned row number would
/// test the layout rather than the behaviour.
fn code_box_row(buf: &Buffer, width: u16) -> u16 {
    (0..buf.area.height)
        .find(|y| row(buf, *y, width).contains('╭'))
        .expect("the code box")
}

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
    assert_eq!(buf[(0, 4)].fg, model_selected_color());
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
    // Found by content, not by row index: the page grew a reason row under the
    // list (`model_error_lines`) and it is bottom-anchored, so a fixed row
    // number pins the assertion to a layout rather than to the behaviour.
    let lines = crate::ui::model_view::model_view_lines(&picker, 60);
    let counter = lines
        .iter()
        .map(plain)
        .find(|l| l.contains("unavailable"))
        .expect("the counter's failure note");
    assert!(counter.contains("Agent Zero API"), "{counter:?}");
    // …and the reason itself now sits below it, so the note is not a dead end.
    let text = lines.iter().map(plain).collect::<Vec<_>>().join("\n");
    assert!(text.contains("HTTP 401"), "{text}");
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
    assert_eq!(buf[(2, 4)].fg, error_color(), "error rows are red");
}

#[test]
fn render_login_provider_step_frames_lists_and_marks_configured() {
    let app = login_app_provider();
    let onboarding = app.key_onboarding.as_ref().unwrap();
    // rule(0) gap(1) ❯(2) gap(3) rows(4,5) counter(6) gap(7) path(8) nav(9)
    // gap(10) rule(11).
    let mut buf = buffer(60, 12);
    render_key_onboarding(buf.area, &mut buf, onboarding);
    assert!(row(&buf, 0, 60).starts_with('─'), "top rule");
    assert!(row(&buf, 1, 60).trim().is_empty(), "gap under the rule");
    // The `❯` filter line, straight under the rule — no heading over it.
    assert!(row(&buf, LOGIN_SEARCH_ROW, 60).contains('❯'));
    // First provider row (y = LOGIN_SEARCH_ROW + 2 = 4): selected →, the name,
    // and its configured status — and nothing else. The env var it saves under
    // is named by the hint below and by the save toast; repeating it on every
    // row only crowds the list.
    let first = row(&buf, 4, 60);
    assert!(first.starts_with("→ OpenRouter"), "{first:?}");
    assert!(
        !first.contains('[') && !first.contains("OPENROUTER_API_KEY"),
        "the env var does not ride the row: {first:?}"
    );
    assert!(
        first.contains("✔ configured"),
        "a configured provider says so: {first:?}"
    );
    // Together AI (row 1, y=5) has no key — and says *that*, rather than
    // leaving the reader to notice a missing mark.
    assert!(
        row(&buf, 5, 60).contains("◯ unconfigured"),
        "{:?}",
        row(&buf, 5, 60)
    );
    // The hints name the real .env path and the step's own key grammar.
    assert!(
        row(&buf, 8, 60).contains("Keys are saved to ~/.alter-zero/.env"),
        "hint: {:?}",
        row(&buf, 8, 60)
    );
    assert!(
        row(&buf, 9, 60).contains("esc back"),
        "{:?}",
        row(&buf, 9, 60)
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
    // The cyan title (row 2, after the top rule + gap) names the provider.
    assert!(row(&buf, 2, 60).contains("Enter your OpenRouter API key"));
    assert_eq!(buf[(2, 2)].fg, login_title_color(), "titles are cyan");
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

/// The flow parked on the **host** field — Ollama's row, added as a third
/// choice for these tests alone.
fn login_app_host() -> App {
    let mut app = App::new();
    let mut choices = login_choices();
    choices.push(host_choice());
    app.open_key_onboarding(choices, login_subscriptions(), "~/.alter-zero/.env");
    let onboarding = app.key_onboarding.as_mut().unwrap();
    onboarding.step = KeyStep::Key;
    onboarding.chosen = Some(2);
    assert!(onboarding.chosen_provider().unwrap().key_kind.is_host());
    app
}

#[test]
fn render_login_host_step_asks_for_the_host_and_shows_it_unmasked() {
    // A host is not a secret: the title says what it wants, the field shows
    // what was typed (a URL typed blind is a URL typed wrong), and the hint
    // says an empty Enter takes the default (docs/ollama.md).
    let mut app = login_app_host();
    app.key_onboarding.as_mut().unwrap().key_input = "myhost:11434".into();
    let onboarding = app.key_onboarding.as_ref().unwrap();
    let mut buf = buffer(60, LOGIN_KEY_ROWS);
    render_key_onboarding(buf.area, &mut buf, onboarding);
    assert!(
        row(&buf, 2, 60).contains("Enter your Ollama host"),
        "{:?}",
        row(&buf, 2, 60)
    );
    assert!(
        !row(&buf, 2, 60).contains("API key"),
        "not a key: {:?}",
        row(&buf, 2, 60)
    );
    let field = row(&buf, LOGIN_KEY_INPUT_ROW, 60);
    assert!(field.contains("myhost:11434"), "unmasked: {field:?}");
    assert!(!field.contains('•'), "no dots: {field:?}");
    let hint = row(&buf, LOGIN_KEY_INPUT_ROW + 2, 60);
    assert!(
        hint.contains("Enter to save") && hint.contains("empty"),
        "{hint:?}"
    );
}

#[test]
fn render_login_host_step_offers_the_default_when_empty() {
    let app = login_app_host();
    let onboarding = app.key_onboarding.as_ref().unwrap();
    let mut buf = buffer(60, LOGIN_KEY_ROWS);
    render_key_onboarding(buf.area, &mut buf, onboarding);
    let field = row(&buf, LOGIN_KEY_INPUT_ROW, 60);
    assert!(
        field.contains("http://127.0.0.1:11434"),
        "the default is the placeholder: {field:?}"
    );
    assert!(!field.contains("paste your API key"), "{field:?}");
}

#[test]
fn login_host_prompt_names_the_provider() {
    assert_eq!(login_host_prompt("Ollama"), "Enter your Ollama host");
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
    // the provider list.
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
    assert_eq!(buf[(2, 4)].fg, model_selected_color());
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
    assert_eq!(texts.len(), 10, "the collapsed page is 10 rows: {texts:?}");
    assert!(is_rule(&texts[0]), "{texts:?}");
    assert_eq!(texts[1], "", "{texts:?}");
    assert!(texts[2].contains('❯'), "{texts:?}");
    assert_eq!(texts[3], "", "{texts:?}");
    assert!(texts[4].contains("No matching providers"), "{texts:?}");
    assert_eq!(texts[5], "", "one gap under the placeholder: {texts:?}");
    assert!(texts[6].contains("Keys are saved"), "the hint: {texts:?}");
    assert!(texts[7].contains("esc back"), "the key hint: {texts:?}");
    assert_eq!(texts[8], "", "{texts:?}");
    assert!(is_rule(&texts[9]), "{texts:?}");
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
    // 38 columns of name into a 30-column row: the marker and the ✓ keep
    // their seats and the name shows its cut.
    let row = lines
        .iter()
        .map(plain)
        .find(|l| l.contains("An E"))
        .expect("the provider row");
    assert!(row.contains('…'), "the name marks its cut: {row:?}");
    assert!(
        crate::ui::wrap::cols(row.trim_end()) <= 30,
        "the row still fits the width: {row:?}"
    );
}

// --- the method root and the subscription half (docs/copilot.md) ---

#[test]
fn the_method_step_offers_the_two_ways_in_with_no_counter() {
    // The root question: rule(0) gap(1) ❯(2) gap(3) rows(4,5) gap(6) hint(7)
    // gap(8) rule(9). No title — the two rows *are* the question — and no
    // `(1/2)` counter, which would say nothing they don't.
    let app = login_app();
    let onboarding = app.key_onboarding.as_ref().unwrap();
    let texts: Vec<String> = crate::ui::login_view::key_onboarding_lines(onboarding, 70)
        .iter()
        .map(|l| plain(l).trim_end().to_string())
        .collect();
    assert_eq!(texts.len(), 10, "{texts:?}");
    assert!(texts[2].contains('❯'), "{texts:?}");
    assert_eq!(texts[4], "→ Use a subscription", "{texts:?}");
    assert_eq!(texts[5], "  Use an API key", "{texts:?}");
    assert!(texts[7].contains("↑↓ navigate"), "{texts:?}");
    assert!(texts[7].contains("escape/ctrl+c cancel"), "{texts:?}");
    assert!(
        !texts.iter().any(|t| t.contains("(1/2)")),
        "no counter on the root: {texts:?}"
    );
}

#[test]
fn the_subscription_step_lists_each_row_under_its_filter() {
    let app = login_app_subscription();
    let onboarding = app.key_onboarding.as_ref().unwrap();
    let mut buf = buffer(70, 10);
    render_key_onboarding(buf.area, &mut buf, onboarding);
    assert!(row(&buf, LOGIN_SEARCH_ROW, 70).contains('❯'), "the filter");
    let entry = row(&buf, 4, 70);
    assert!(entry.starts_with("→ GitHub Copilot"), "{entry:?}");
    assert!(
        !entry.contains("Sign in with your GitHub account"),
        "the row is the name and its status, not a sentence: {entry:?}"
    );
    assert!(
        entry.contains("✔ configured"),
        "already signed in: {entry:?}"
    );
    assert!(row(&buf, 5, 70).contains("(1/1)"), "the counter");
    assert!(row(&buf, 7, 70).contains("enter sign in"), "the hint");
}

#[test]
fn a_subscription_is_still_findable_by_words_the_row_no_longer_shows() {
    // The one-line description left the *display*, not the data: it still
    // steers the type-to-search, so a query matching only the description
    // finds the row. Locking it here because a reader who greps the view for
    // `.description` now finds nothing and could delete the field as dead.
    let mut app = login_app_subscription();
    let onboarding = app.key_onboarding.as_mut().unwrap();
    let described = onboarding.subscriptions[0].description.clone();
    assert!(
        described.contains("GitHub"),
        "the fixture's description: {described:?}"
    );
    onboarding.query = "sign in with your github".to_string();
    assert_eq!(
        onboarding.subscription_matches().len(),
        1,
        "a description-only query still finds its row"
    );
    // And the row it finds still shows none of those words.
    let mut buf = buffer(70, 10);
    render_key_onboarding(buf.area, &mut buf, app.key_onboarding.as_ref().unwrap());
    let entry = row(&buf, 4, 70);
    assert!(entry.contains("GitHub Copilot"), "{entry:?}");
    assert!(!entry.contains("Sign in with your"), "{entry:?}");
}

// --- the device-code page (docs/copilot.md) ---

#[test]
fn the_device_page_shows_the_url_and_the_code_in_a_box() {
    let app = login_app_device();
    let onboarding = app.key_onboarding.as_ref().unwrap();
    let mut buf = buffer(72, 16);
    render_key_onboarding(buf.area, &mut buf, onboarding);
    assert!(
        row(&buf, 2, 72).contains("Sign in to GitHub Copilot"),
        "title"
    );
    assert_eq!(buf[(2, 2)].fg, login_title_color(), "titles are cyan");
    assert!(
        row(&buf, 4, 72).contains("Visit https://github.com/login/device"),
        "{:?}",
        row(&buf, 4, 72)
    );
    assert!(row(&buf, 5, 72).contains("and enter this one-time code"));
    // The rounded box, sized to the code (9 glyphs + 2 pad each side = 13).
    let box_row = code_box_row(&buf, 72);
    let top = row(&buf, box_row, 72);
    assert!(top.contains("╭─────────────╮"), "{top:?}");
    let middle = row(&buf, box_row + 1, 72);
    assert!(middle.contains("│  C363-262E  │"), "{middle:?}");
    assert!(row(&buf, box_row + 2, 72).contains("╰─────────────╯"));
    assert!(
        row(&buf, 11, 72).contains("Waiting for approval"),
        "the wait"
    );
    assert!(
        row(&buf, 13, 72).contains("c copy code  esc cancel"),
        "the hint"
    );
}

#[test]
fn the_device_page_counts_the_code_down() {
    // The remaining time is a boundary clock read, injected per draw — so the
    // page shows whatever the loop last measured, and nothing else.
    let mut app = login_app_device();
    app.set_device_remaining(Some(std::time::Duration::from_secs(851)));
    let onboarding = app.key_onboarding.as_ref().unwrap();
    let lines = crate::ui::login_view::key_onboarding_lines(onboarding, 72);
    let wait = lines
        .iter()
        .map(plain)
        .find(|l| l.contains("Waiting for approval"))
        .expect("the wait row");
    assert!(wait.contains("expires in 14:11"), "{wait:?}");
}

#[test]
fn an_expired_code_says_so_rather_than_counting_zero() {
    let mut app = login_app_device();
    app.set_device_remaining(Some(std::time::Duration::ZERO));
    let onboarding = app.key_onboarding.as_ref().unwrap();
    let wait = crate::ui::login_view::key_onboarding_lines(onboarding, 72)
        .iter()
        .map(plain)
        .find(|l| l.contains("Waiting for approval"))
        .expect("the wait row");
    assert!(wait.contains("code expired"), "{wait:?}");
    assert!(!wait.contains("0:00"), "{wait:?}");
}

#[test]
fn the_countdown_pads_its_seconds_but_not_its_minutes() {
    use crate::ui::login_view::countdown;
    use std::time::Duration;
    assert_eq!(countdown(Duration::from_secs(851)), "14:11");
    assert_eq!(countdown(Duration::from_secs(65)), "1:05");
    assert_eq!(countdown(Duration::from_secs(9)), "0:09");
}

#[test]
fn a_failed_sign_in_replaces_the_wait_with_the_reason_in_red() {
    // The page stays up wearing the failure: closing it on the error would
    // take the explanation away with it.
    let mut app = login_app_device();
    app.fail_device_login("The code expired — press Esc and sign in again.");
    let onboarding = app.key_onboarding.as_ref().unwrap();
    let mut buf = buffer(72, 16);
    render_key_onboarding(buf.area, &mut buf, onboarding);
    let all: String = (0..16)
        .map(|y| row(&buf, y, 72))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(all.contains("The code expired"), "{all}");
    assert!(!all.contains("Waiting for approval"), "{all}");
    // …and the code box is still there to compare against GitHub's page.
    assert!(all.contains("C363-262E"), "{all}");
    let reason_row = (0..16)
        .find(|y| row(&buf, *y, 72).contains("The code expired"))
        .unwrap();
    assert_eq!(buf[(2, reason_row)].fg, error_color(), "failures are red");
}

#[test]
fn the_device_page_hides_the_hardware_cursor() {
    // It is a wait, not a field — and a kitty cursor trail would streak
    // across it on every countdown tick (the permission prompt's rule).
    let app = login_app_device();
    assert!(!cursor_visible(&app));
    assert!(cursor_visible(&login_app()), "a list still has a filter");
}

#[test]
fn a_device_page_with_no_code_yet_still_frames_itself() {
    // Between Enter and GitHub's answer there is no URL and no code; the page
    // must keep its shape rather than collapsing and re-growing under the user.
    let mut app = login_app_subscription();
    {
        let onboarding = app.key_onboarding.as_mut().unwrap();
        onboarding.step = KeyStep::Device;
        onboarding.device = Some(crate::app::DeviceLogin {
            provider_name: "GitHub Copilot".into(),
            ..Default::default()
        });
    }
    let onboarding = app.key_onboarding.as_ref().unwrap();
    let texts: Vec<String> = crate::ui::login_view::key_onboarding_lines(onboarding, 72)
        .iter()
        .map(|l| plain(l).trim_end().to_string())
        .collect();
    assert!(texts[2].contains("Sign in to GitHub Copilot"), "{texts:?}");
    assert!(
        texts.iter().any(|t| t.contains("Requesting a code")),
        "{texts:?}"
    );
    assert!(
        !texts.iter().any(|t| t.contains('╭')),
        "no empty box: {texts:?}"
    );
}

// --- clickable sign-in links (docs/links.md) ---

/// The lines of a sign-in page for `kind` carrying `uri`.
fn signin_page(kind: crate::app::SigninKind, uri: &str, width: u16) -> Vec<Line<'static>> {
    let mut app = login_app_subscription();
    {
        let onboarding = app.key_onboarding.as_mut().unwrap();
        onboarding.step = KeyStep::Device;
        onboarding.device = Some(crate::app::DeviceLogin {
            provider_name: "P".into(),
            verification_uri: uri.into(),
            status: crate::app::DeviceStatus::Waiting,
            kind,
            ..Default::default()
        });
    }
    let onboarding = app.key_onboarding.as_ref().unwrap();
    crate::ui::login_view::key_onboarding_lines(onboarding, width)
}

/// Every link target carried by any span of `lines`.
fn link_targets(lines: &[Line<'static>]) -> Vec<String> {
    lines
        .iter()
        .flat_map(|l| l.spans.iter())
        .filter_map(|s| crate::links::style_link(&s.style).map(|u| u.to_string()))
        .collect()
}

#[test]
fn the_browser_sign_in_shows_the_bare_url_with_no_verb_in_front_of_it() {
    // The link is the affordance: an "Open" before a clickable URL is a word
    // doing nothing, and it pushed the URL off its own line's start.
    let lines = signin_page(
        crate::app::SigninKind::BrowserLink,
        "https://auth.openai.com/oauth/authorize?client_id=abc",
        72,
    );
    let texts: Vec<String> = lines.iter().map(|l| plain(l).trim().to_string()).collect();
    assert!(
        texts
            .iter()
            .any(|t| t.starts_with("https://auth.openai.com/oauth/authorize")),
        "the URL opens its own row: {texts:?}"
    );
    assert!(
        !texts.iter().any(|t| t.starts_with("Open ")),
        "no verb in front of the link: {texts:?}"
    );
}

#[test]
fn a_hard_broken_sign_in_url_opens_the_whole_target_from_every_fragment() {
    // The reason `links` exists at all: a URL wider than the row breaks across
    // display rows, and a terminal's own detection sees only row text — so
    // without the carrier, clicking the second half opens a truncated URL.
    // The page is narrow enough here that the URL must break.
    let url = "https://auth.openai.com/oauth/authorize?response_type=code&client_id=app_EMoamEEZ73f0CkXaXp7hrann&state=xyz";
    let lines = signin_page(crate::app::SigninKind::BrowserLink, url, 48);
    let fragments: Vec<&Span<'static>> = lines
        .iter()
        .flat_map(|l| l.spans.iter())
        .filter(|s| s.content.contains("openai.com") || s.content.contains("client_id"))
        .collect();
    assert!(
        fragments.len() >= 2,
        "the URL must actually wrap for this test to mean anything: {}",
        fragments.len()
    );
    for span in fragments {
        assert_eq!(
            crate::links::style_link(&span.style).as_deref(),
            Some(url),
            "every fragment carries the whole target, not its own row text: {:?}",
            span.content
        );
    }
}

#[test]
fn the_device_pages_url_is_clickable_too_and_keeps_its_dim_dress() {
    // Both sign-in pages get the carrier. The device page's URL stays DIM on
    // purpose (docs/copilot.md: the code in its box is what the eye should
    // land on), so linking it must not repaint it in the chat link colour —
    // the underline is the affordance that it is clickable.
    let lines = signin_page(
        crate::app::SigninKind::DeviceCode,
        "https://github.com/login/device",
        72,
    );
    assert_eq!(
        link_targets(&lines),
        vec!["https://github.com/login/device".to_string()],
        "exactly the one URL on the page is a link"
    );
    let url_span = lines
        .iter()
        .flat_map(|l| l.spans.iter())
        .find(|s| s.content.contains("github.com"))
        .expect("the URL span");
    assert_eq!(url_span.style.fg, Some(model_meta_color()), "still dim");
    assert!(
        url_span.style.add_modifier.contains(Modifier::UNDERLINED),
        "underlined, as a link is"
    );
    // The prose around it is not swept into the link.
    let visit = lines
        .iter()
        .flat_map(|l| l.spans.iter())
        .find(|s| s.content.contains("Visit"))
        .expect("the Visit lead");
    assert!(crate::links::style_link(&visit.style).is_none());
}

#[test]
fn no_device_page_ever_stacks_two_blank_rows() {
    // The suite's own convention (`no_login_page_ever_stacks_two_blank_rows`),
    // applied to the page whose content genuinely comes and goes: before the
    // code lands there is no URL and no box, and reserving their rows anyway
    // left a band of blanks under the title.
    use crate::ui::login_view::key_onboarding_lines;
    let mut app = login_app_subscription();
    {
        let onboarding = app.key_onboarding.as_mut().unwrap();
        onboarding.step = KeyStep::Device;
        onboarding.device = Some(crate::app::DeviceLogin {
            provider_name: "GitHub Copilot".into(),
            ..Default::default()
        });
    }
    // Every state the page passes through, in the order it passes through
    // them: requesting the code, showing it, then failing.
    for state in 0..3 {
        match state {
            1 => app.set_device_code("https://github.com/login/device", "C363-262E"),
            2 => app.fail_device_login("it went wrong"),
            _ => {}
        }
        let onboarding = app.key_onboarding.as_ref().unwrap();
        let texts: Vec<String> = key_onboarding_lines(onboarding, 72)
            .iter()
            .map(|l| plain(l).trim_end().to_string())
            .collect();
        for pair in texts.windows(2) {
            assert!(
                !(pair[0].is_empty() && pair[1].is_empty()),
                "state {state} stacked two blank rows: {texts:?}"
            );
        }
    }
}

#[test]
fn the_waiting_page_is_title_status_hint_and_nothing_else() {
    // The exact shape asked for: rule, gap, title, gap, status, gap, hint,
    // gap, rule — no rows held open for a URL and a code box that do not
    // exist yet.
    use crate::ui::login_view::key_onboarding_lines;
    let mut app = login_app_subscription();
    {
        let onboarding = app.key_onboarding.as_mut().unwrap();
        onboarding.step = KeyStep::Device;
        onboarding.device = Some(crate::app::DeviceLogin {
            provider_name: "GitHub Copilot".into(),
            ..Default::default()
        });
    }
    let onboarding = app.key_onboarding.as_ref().unwrap();
    let texts: Vec<String> = key_onboarding_lines(onboarding, 72)
        .iter()
        .map(|l| plain(l).trim_end().to_string())
        .collect();
    let is_rule = |t: &str| !t.is_empty() && t.chars().all(|c| c == '─');
    assert_eq!(texts.len(), 9, "{texts:?}");
    assert!(is_rule(&texts[0]), "{texts:?}");
    assert_eq!(texts[1], "", "{texts:?}");
    assert!(texts[2].contains("Sign in to GitHub Copilot"), "{texts:?}");
    assert_eq!(texts[3], "", "{texts:?}");
    assert!(texts[4].contains("Requesting a code"), "{texts:?}");
    assert_eq!(texts[5], "", "{texts:?}");
    assert!(texts[6].contains("c copy code"), "{texts:?}");
    assert_eq!(texts[7], "", "{texts:?}");
    assert!(is_rule(&texts[8]), "{texts:?}");
}

#[test]
fn the_verification_url_is_dim_so_the_code_is_the_bright_thing() {
    // The code in its box is what gets transcribed; a cyan URL competed with
    // it for the eye. Everything around the box reads as instruction now.
    let app = login_app_device();
    let onboarding = app.key_onboarding.as_ref().unwrap();
    let mut buf = buffer(72, 16);
    render_key_onboarding(buf.area, &mut buf, onboarding);
    let visit_row = (0..16)
        .find(|y| row(&buf, *y, 72).contains("Visit https://"))
        .expect("the visit row");
    assert_eq!(buf[(2, visit_row)].fg, model_meta_color(), "the URL is dim");
    // …and the code inside the box stays bright.
    let code_row = code_box_row(&buf, 72) + 1;
    let code_col = row(&buf, code_row, 72).find('C').expect("the code") as u16;
    assert_ne!(
        buf[(code_col, code_row)].fg,
        model_meta_color(),
        "the code itself is not dim"
    );
}

#[test]
fn the_device_pages_hidden_cursor_parks_off_the_code() {
    // The caret is hidden here, but a terminal with a cursor-trail animation
    // (kitty and kin) still animates toward wherever it is *seated* — so the
    // seat must not be the code box. Anything the emulator paints at the
    // cursor would land on the one thing the page exists to be read from.
    // It parks on the frame's first content row instead, where every other
    // `/login` step already puts it.
    let app = login_app_device();
    assert!(!cursor_visible(&app), "the page is a wait, not a field");
    let onboarding = app.key_onboarding.as_ref().unwrap();
    let rows = crate::ui::login_view::key_onboarding_lines(onboarding, 72).len() as u16;
    let area = Rect::new(0, 0, 72, rows);
    let (_, y) = cursor_position(area, &app);
    assert_eq!(y, DEVICE_CURSOR_ROW, "the frame's first content row");
    let mut buf = buffer(72, rows);
    render_key_onboarding(buf.area, &mut buf, onboarding);
    assert_ne!(y, code_box_row(&buf, 72) + 1, "never on the code itself");
}

#[test]
fn the_device_seat_does_not_move_when_the_code_arrives() {
    // The page grows from 9 rows to 16 when GitHub answers. A seat measured
    // from the frame's top is the same screen row across that change, so the
    // trail fires once at most — a seat further down jumps with the growth,
    // and the countdown re-arms a frame every 32ms behind it.
    let mut app = login_app_subscription();
    {
        let onboarding = app.key_onboarding.as_mut().unwrap();
        onboarding.step = KeyStep::Device;
        onboarding.device = Some(crate::app::DeviceLogin {
            provider_name: "GitHub Copilot".into(),
            ..Default::default()
        });
    }
    let seat_of = |app: &App| {
        let onboarding = app.key_onboarding.as_ref().unwrap();
        let rows = crate::ui::login_view::key_onboarding_lines(onboarding, 72).len() as u16;
        cursor_position(Rect::new(0, 0, 72, rows.max(1)), app)
    };
    let waiting = seat_of(&app);
    app.set_device_code("https://github.com/login/device", "C363-262E");
    assert_eq!(
        waiting,
        seat_of(&app),
        "the seat holds still across the growth"
    );
}

// --- every list row says whether it is configured (docs/llm.md) ---

/// The foreground of the first span of `line` containing `needle`.
fn span_fg(line: &Line, needle: &str) -> Option<ratatui::style::Color> {
    line.spans
        .iter()
        .find(|s| s.content.contains(needle))
        .and_then(|s| s.style.fg)
}

/// The row of a built `/login` page whose text contains `needle`.
fn page_row(onboarding: &KeyOnboarding, width: u16, needle: &str) -> Line<'static> {
    crate::ui::login_view::key_onboarding_lines(onboarding, width)
        .into_iter()
        .find(|l| plain(l).contains(needle))
        .unwrap_or_else(|| panic!("no row matched {needle:?}"))
}

#[test]
fn a_provider_row_says_whether_its_key_is_configured() {
    // A bare row used to mean "no key yet" — a fact the reader could only get
    // from the *absence* of a ✓ two columns further right. Both states are
    // now spelled out, so the list answers "which of these can I use?" at a
    // glance.
    //
    // **Only the ✔ is coloured.** The mark is the thing worth finding down a
    // list — it wears the green the `/model` picker's ✓ does — while the word
    // beside it is a plain fact and stays dim, so a list of statuses reads as
    // statuses rather than as a column of alerts.
    use crate::ui::theme::{
        LOGIN_CONFIGURED_LABEL, LOGIN_CONFIGURED_MARK, LOGIN_UNCONFIGURED_LABEL,
        LOGIN_UNCONFIGURED_MARK, model_active_color, model_meta_color,
    };
    let app = login_app_provider();
    let onboarding = app.key_onboarding.as_ref().unwrap();
    let keyed = page_row(onboarding, 60, "OpenRouter");
    assert_eq!(
        plain(&keyed).trim_end(),
        format!("→ OpenRouter · {LOGIN_CONFIGURED_MARK}{LOGIN_CONFIGURED_LABEL}")
    );
    assert_eq!(
        span_fg(&keyed, LOGIN_CONFIGURED_MARK),
        Some(model_active_color()),
        "the ✔ is green"
    );
    assert_eq!(
        span_fg(&keyed, LOGIN_CONFIGURED_LABEL),
        Some(model_meta_color()),
        "the word beside it is not"
    );

    let bare = page_row(onboarding, 60, "Together AI");
    assert_eq!(
        plain(&bare).trim_end(),
        format!("  Together AI · {LOGIN_UNCONFIGURED_MARK}{LOGIN_UNCONFIGURED_LABEL}")
    );
    // Nothing to find on a row with no key: the ◯ is dim like its word, so the
    // green marks are the only thing standing out down the list.
    assert_eq!(
        span_fg(&bare, LOGIN_UNCONFIGURED_MARK),
        Some(model_meta_color())
    );
    assert_eq!(
        span_fg(&bare, LOGIN_UNCONFIGURED_LABEL),
        Some(model_meta_color())
    );
}

#[test]
fn a_subscription_row_says_whether_it_is_signed_in() {
    use crate::ui::theme::{LOGIN_CONFIGURED_LABEL, LOGIN_CONFIGURED_MARK};
    let app = login_app_subscription();
    let onboarding = app.key_onboarding.as_ref().unwrap();
    let row = page_row(onboarding, 70, "GitHub Copilot");
    assert_eq!(
        plain(&row).trim_end(),
        format!("→ GitHub Copilot · {LOGIN_CONFIGURED_MARK}{LOGIN_CONFIGURED_LABEL}")
    );
}

#[test]
fn the_method_root_rows_carry_no_configured_status() {
    // The two ways in are the *question*, not an answer: neither is a thing
    // that can be configured, so neither wears a status.
    let app = login_app();
    let onboarding = app.key_onboarding.as_ref().unwrap();
    for name in ["Use a subscription", "Use an API key"] {
        let row = plain(&page_row(onboarding, 70, name));
        assert!(!row.contains("configured"), "{row:?}");
    }
}

// --- the two list steps carry no heading (docs/llm.md) ---

/// A built `/login` page as trailing-trimmed row texts.
fn page_texts(onboarding: &KeyOnboarding, width: u16) -> Vec<String> {
    crate::ui::login_view::key_onboarding_lines(onboarding, width)
        .iter()
        .map(|l| plain(l).trim_end().to_string())
        .collect()
}

#[test]
fn the_two_list_steps_open_straight_onto_their_filter() {
    // The subscription and provider pages used to repeat the method row that
    // opened them as a cyan heading. It says nothing the hint under the list
    // and the rows themselves don't, and it pushed every row two lines down —
    // so all three lists are one shape now: rule, gap, filter.
    for app in [login_app_subscription(), login_app_provider()] {
        let onboarding = app.key_onboarding.as_ref().unwrap();
        let texts = page_texts(onboarding, 70);
        assert!(
            texts[2].contains('❯'),
            "the filter is the first content row: {texts:?}"
        );
        for heading in ["Use a subscription", "Use an API key"] {
            assert!(
                !texts.iter().any(|t| t.contains(heading)),
                "no {heading:?} heading: {texts:?}"
            );
        }
    }
    // The root still asks its question — there its two rows *are* the answer
    // set, not a heading over one.
    let app = login_app();
    let texts = page_texts(app.key_onboarding.as_ref().unwrap(), 70);
    assert!(texts[2].contains('❯'), "{texts:?}");
    assert!(texts[4].contains("Use a subscription"), "{texts:?}");
}

// --- the key step introduces the provider it is asking for (docs/llm.md) ---

/// The flow parked on the key step of a provider that carries a description
/// and the page its keys are created on.
fn login_app_described(key_kind: KeyKind) -> App {
    let mut app = App::new();
    let choice = ProviderChoice {
        id: "openrouter".into(),
        name: "OpenRouter".into(),
        env_var: "OPENROUTER_API_KEY".into(),
        configured: false,
        key_kind,
        description: "One key for models from every major lab, billed from a single balance."
            .into(),
        key_url: "https://openrouter.ai/workspaces/default/keys".into(),
    };
    app.open_key_onboarding(vec![choice], Vec::new(), "~/.alter-zero/.env");
    let onboarding = app.key_onboarding.as_mut().unwrap();
    onboarding.step = KeyStep::Key;
    onboarding.chosen = Some(0);
    app
}

#[test]
fn the_key_step_describes_the_provider_and_links_its_key_page() {
    // "Enter your OpenRouter API key" tells a reader who already knows what
    // OpenRouter is nothing, and tells everyone else nothing at all — least
    // of all *where* the key they are being asked to paste comes from.
    let app = login_app_described(KeyKind::Secret);
    let onboarding = app.key_onboarding.as_ref().unwrap();
    let lines = crate::ui::login_view::key_onboarding_lines(onboarding, 70);
    let text = lines.iter().map(plain).collect::<Vec<_>>().join("\n");
    assert!(text.contains("every major lab"), "the description: {text}");
    assert!(
        text.contains("Create a key at https://openrouter.ai/workspaces/default/keys"),
        "the key page: {text}"
    );
    // …and the URL is a real hyperlink, like every other URL this flow shows.
    assert_eq!(
        link_targets(&lines),
        vec!["https://openrouter.ai/workspaces/default/keys".to_string()]
    );
    // The description sits between the title and the field, so the field is
    // still the last thing above the hint.
    let texts = page_texts(onboarding, 70);
    let title = texts.iter().position(|t| t.contains("Enter your")).unwrap();
    let about = texts.iter().position(|t| t.contains("major lab")).unwrap();
    let field = texts.iter().position(|t| t.contains('❯')).unwrap();
    assert!(title < about && about < field, "{texts:?}");
}

#[test]
fn a_narrow_key_page_wraps_the_description_rather_than_cutting_it() {
    // Every row of the block must survive: a description clipped at the width
    // is a sentence that stops mid-word, and a clipped URL is a dead link.
    let app = login_app_described(KeyKind::Secret);
    let onboarding = app.key_onboarding.as_ref().unwrap();
    let lines = crate::ui::login_view::key_onboarding_lines(onboarding, 40);
    for line in &lines {
        assert!(
            crate::ui::wrap::cols(plain(line).trim_end()) <= 40,
            "no row leaks past the width: {:?}",
            plain(line)
        );
    }
    let joined = lines.iter().map(plain).collect::<Vec<_>>().join(" ");
    assert!(
        joined.contains("single balance."),
        "the description's tail survives: {joined}"
    );
    // A URL is one long word, so the wrap hard-breaks it; stripping spaces
    // reassembles it wherever it broke — and each fragment still carries the
    // whole target (docs/links.md).
    assert!(
        joined
            .replace(' ', "")
            .contains("openrouter.ai/workspaces/default/keys"),
        "the URL's tail survives: {joined}"
    );
    assert!(
        link_targets(&lines)
            .iter()
            .all(|u| u == "https://openrouter.ai/workspaces/default/keys"),
        "every fragment opens the whole page"
    );
}

#[test]
fn a_host_field_points_at_the_software_rather_than_a_key_page() {
    // A keyless provider has no key to create: what its link is for is the
    // server the host field is asking about (docs/ollama.md).
    let app = login_app_described(KeyKind::Host {
        default: "http://127.0.0.1:11434".into(),
    });
    let onboarding = app.key_onboarding.as_ref().unwrap();
    let text = page_texts(onboarding, 70).join("\n");
    assert!(text.contains("Install it from https://"), "{text}");
    assert!(!text.contains("Create a key at"), "{text}");
}

#[test]
fn a_provider_with_nothing_to_say_keeps_the_bare_key_page() {
    // The block is omitted whole — never a blank band under the title.
    let app = login_app_key();
    let onboarding = app.key_onboarding.as_ref().unwrap();
    let texts = page_texts(onboarding, 70);
    assert_eq!(texts.len(), usize::from(LOGIN_KEY_ROWS), "{texts:?}");
    for pair in texts.windows(2) {
        assert!(
            !(pair[0].is_empty() && pair[1].is_empty()),
            "no stacked blanks: {texts:?}"
        );
    }
}

#[test]
fn the_cursor_follows_the_key_field_below_a_description_block() {
    // The field's row is no longer a constant — a description of any length
    // sits above it — so the seat is found in the page the paint builds.
    let mut app = login_app_described(KeyKind::Secret);
    app.key_onboarding.as_mut().unwrap().key_input = "abcd".into();
    let area = Rect::new(0, 0, 70, key_onboarding_height(&app, 70, 40).unwrap());
    let (x, y) = cursor_position(area, &app);
    let onboarding = app.key_onboarding.as_ref().unwrap();
    let field = page_texts(onboarding, 70)
        .iter()
        .position(|t| t.contains('❯'))
        .unwrap() as u16;
    assert_eq!(y, field, "the caret sits on the field row");
    // indent(2) + "❯ "(2) + 4 mask glyphs = 8.
    assert_eq!(x, 8);
}

#[test]
fn a_provider_is_findable_by_the_words_of_its_description() {
    // The provider list filters on the description for the same reason the
    // subscription list does: the row shows a name, and a user hunting for
    // "claude" or "local" is describing what they want, not naming it.
    let app = login_app_described(KeyKind::Secret);
    let mut onboarding = app.key_onboarding.clone().unwrap();
    onboarding.query = "major lab".into();
    assert_eq!(onboarding.matches().len(), 1);
}
