//! The inline `/login` onboarding (`docs/llm.md`).

use super::*;

/// The key step's whole page height (see `key_onboarding_lines`).
const LOGIN_KEY_ROWS: u16 = 9;
use crate::ui::login_view::login_key_prompt;
use crate::ui::theme::{
    DEVICE_CODE_ROW, ERROR_COLOR, LOGIN_KEY_INPUT_ROW, LOGIN_SEARCH_ROW, LOGIN_TITLE_COLOR,
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
    // rule(0) gap(1) title(2) gap(3) ❯(4) gap(5) rows(6,7) counter(8) gap(9)
    // path(10) nav(11) gap(12) rule(13).
    let mut buf = buffer(60, 14);
    render_key_onboarding(buf.area, &mut buf, onboarding);
    assert!(row(&buf, 0, 60).starts_with('─'), "top rule");
    assert!(row(&buf, 1, 60).trim().is_empty(), "gap under the rule");
    // The cyan title says which half of `/login` you are in.
    assert!(
        row(&buf, 2, 60).contains("Use an API key"),
        "{:?}",
        row(&buf, 2, 60)
    );
    assert_eq!(buf[(2, 2)].fg, LOGIN_TITLE_COLOR, "titles are cyan");
    // The `❯` filter line.
    assert!(row(&buf, LOGIN_SEARCH_ROW, 60).contains('❯'));
    // First provider row (y = LOGIN_SEARCH_ROW + 2 = 6): selected →, env tag,
    // and a green ✓ because OpenRouter is configured.
    let first = row(&buf, 6, 60);
    assert!(first.starts_with("→ OpenRouter"), "{first:?}");
    assert!(first.contains("[OPENROUTER_API_KEY]"), "{first:?}");
    assert!(
        first.contains('✓'),
        "configured provider has a check: {first:?}"
    );
    // Together AI (row 1, y=7) is not configured → no check.
    assert!(!row(&buf, 7, 60).contains('✓'));
    // The hints name the real .env path and the step's own key grammar.
    assert!(
        row(&buf, 10, 60).contains("Keys are saved to ~/.alter-zero/.env"),
        "hint: {:?}",
        row(&buf, 10, 60)
    );
    assert!(
        row(&buf, 11, 60).contains("esc back"),
        "{:?}",
        row(&buf, 11, 60)
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
    assert_eq!(buf[(2, 2)].fg, LOGIN_TITLE_COLOR, "titles are cyan");
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
    // The onboarding stands in for the composer: top rule, cyan title, `❯`
    // filter, and the provider list.
    assert!(row(&buf, 0, 60).starts_with('─'), "top rule");
    assert!(row(&buf, 2, 60).contains("Use an API key"), "the title");
    assert!(row(&buf, LOGIN_SEARCH_ROW, 60).contains('❯'), "filter line");
    assert!(row(&buf, 6, 60).contains("OpenRouter"), "a provider row");
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
    assert_eq!(texts.len(), 12, "the collapsed page is 12 rows: {texts:?}");
    assert!(is_rule(&texts[0]), "{texts:?}");
    assert_eq!(texts[1], "", "{texts:?}");
    assert!(texts[2].contains("Use an API key"), "{texts:?}");
    assert_eq!(texts[3], "", "{texts:?}");
    assert!(texts[4].contains('❯'), "{texts:?}");
    assert_eq!(texts[5], "", "{texts:?}");
    assert!(texts[6].contains("No matching providers"), "{texts:?}");
    assert_eq!(texts[7], "", "one gap under the placeholder: {texts:?}");
    assert!(texts[8].contains("Keys are saved"), "the hint: {texts:?}");
    assert!(texts[9].contains("esc back"), "the key hint: {texts:?}");
    assert_eq!(texts[10], "", "{texts:?}");
    assert!(is_rule(&texts[11]), "{texts:?}");
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
fn the_subscription_step_titles_itself_and_describes_each_row() {
    let app = login_app_subscription();
    let onboarding = app.key_onboarding.as_ref().unwrap();
    let mut buf = buffer(70, 12);
    render_key_onboarding(buf.area, &mut buf, onboarding);
    assert!(row(&buf, 2, 70).contains("Use a subscription"), "the title");
    assert_eq!(buf[(2, 2)].fg, LOGIN_TITLE_COLOR, "titles are cyan");
    assert!(row(&buf, LOGIN_SEARCH_ROW, 70).contains('❯'), "the filter");
    let entry = row(&buf, 6, 70);
    assert!(entry.starts_with("→ GitHub Copilot"), "{entry:?}");
    assert!(
        entry.contains("Sign in with your GitHub account"),
        "the description sits beside the name: {entry:?}"
    );
    assert!(entry.contains('✓'), "already signed in: {entry:?}");
    assert!(row(&buf, 7, 70).contains("(1/1)"), "the counter");
    assert!(row(&buf, 9, 70).contains("enter sign in"), "the hint");
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
    assert_eq!(buf[(2, 2)].fg, LOGIN_TITLE_COLOR, "titles are cyan");
    assert!(
        row(&buf, 4, 72).contains("Visit https://github.com/login/device"),
        "{:?}",
        row(&buf, 4, 72)
    );
    assert!(row(&buf, 5, 72).contains("and enter this one-time code"));
    // The rounded box, sized to the code (9 glyphs + 2 pad each side = 13).
    let top = row(&buf, DEVICE_CODE_ROW, 72);
    assert!(top.contains("╭─────────────╮"), "{top:?}");
    let middle = row(&buf, DEVICE_CODE_ROW + 1, 72);
    assert!(middle.contains("│  C363-262E  │"), "{middle:?}");
    assert!(row(&buf, DEVICE_CODE_ROW + 2, 72).contains("╰─────────────╯"));
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
    assert_eq!(buf[(2, reason_row)].fg, ERROR_COLOR, "failures are red");
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
