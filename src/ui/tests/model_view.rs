//! The inline `/model` picker (`docs/llm.md`).

use super::*;
use crate::ui::theme::{
    MODEL_ERROR_MAX_ROWS, MODEL_MENU_MAX_ROWS, MODEL_SEARCH_ROW, error_color, model_selected_color,
};

#[test]
fn model_picker_frames_with_rules_and_no_header() {
    let picker = model_picker(three_models(), 0, "anthropic/claude-3-haiku");
    let mut buf = buffer(60, 20);
    render_model_picker(buf.area, &mut buf, &picker);
    // Top rule, then a blank gap where the old "Showing models…" banner was.
    assert!(row(&buf, 0, 60).starts_with('─'), "top rule");
    assert!(row(&buf, 1, 60).trim().is_empty(), "no header banner");
    // The search line moved up to row 2.
    assert!(row(&buf, MODEL_SEARCH_ROW, 60).contains('❯'), "search line");
}

#[test]
fn model_picker_shows_the_search_prompt_and_query() {
    let mut picker = model_picker(three_models(), 0, "x");
    picker.query = "haiku".into();
    let mut buf = buffer(60, 20);
    render_model_picker(buf.area, &mut buf, &picker);
    let search = row(&buf, MODEL_SEARCH_ROW, 60);
    assert!(search.contains("❯ haiku"), "{search:?}");
    // The `❯` prompt is cyan.
    assert_eq!(buf[(2, MODEL_SEARCH_ROW)].fg, model_selected_color());
}

#[test]
fn model_picker_counter_and_name_reflect_the_selection() {
    let picker = model_picker(three_models(), 2, "x");
    // Size the buffer to the picker's natural height (9 chrome + 3 list),
    // like the boundary does — otherwise the Min(0) list would expand and
    // push the counter/name rows down.
    let mut buf = buffer(60, 12);
    render_model_picker(buf.area, &mut buf, &picker);
    // Counter row = top(0) gap(1) search(2) gap(3) list(4,5,6) → 7.
    let counter = row(&buf, 7, 60);
    assert!(counter.contains("(3/3)"), "{counter:?}");
    // Model-name row = counter(7) + gap(8) + 1 = 9.
    let name = row(&buf, 9, 60);
    assert!(
        name.contains("Model Name: MoonshotAI: Kimi K2.6"),
        "{name:?}"
    );
    // A trailing blank gap (the user's mock), then the bottom rule last.
    assert!(row(&buf, 10, 60).trim().is_empty(), "trailing gap");
    assert!(row(&buf, 11, 60).starts_with('─'), "bottom rule");
}

#[test]
fn model_list_keeps_the_selection_centered_not_pinned_to_an_edge() {
    // A long list (30 models) with a deep-interior selection (15) that has
    // plenty of room on both sides — the case the old bottom-anchored window
    // got wrong (it pinned the highlight to the last visible row).
    let models: Vec<ModelEntry> = (0..30)
        .map(|i| {
            model_entry(
                &format!("openrouter/model-{i:02}"),
                "openrouter",
                &format!("Model {i}"),
            )
        })
        .collect();
    let picker = model_picker(models, 15, "x");
    let mut buf = buffer(60, 20);
    render_model_picker(buf.area, &mut buf, &picker);
    // The list spans rows 4..14 — top(0) gap(1) search(2) gap(3) then 10 rows.
    let list: Vec<String> = (4..14).map(|y| row(&buf, y, 60)).collect();
    // The highlight lands on the middle row of the window (max/2), carrying
    // the `→` marker — centered, not jammed against the bottom edge.
    let middle = &list[MODEL_MENU_MAX_ROWS as usize / 2];
    assert!(
        middle.contains("model-15") && middle.contains('→'),
        "selection sits centered: {middle:?}"
    );
    let joined = list.join("\n");
    // Models both above *and* below the selection are on screen — the broad
    // view the fix restores.
    assert!(joined.contains("model-11"), "rows above show: {joined:?}");
    assert!(
        joined.contains("model-19"),
        "rows below show (the old window hid these): {joined:?}"
    );
}

#[test]
fn model_picker_shows_a_loading_placeholder() {
    let picker = ModelPicker {
        active_id: "x".into(),
        ..ModelPicker::default()
    };
    assert_eq!(picker.status, ModelLoad::Loading);
    let mut buf = buffer(60, 20);
    render_model_picker(buf.area, &mut buf, &picker);
    let list = row(&buf, 4, 60);
    assert!(list.contains("Loading models…"), "{list:?}");
}

#[test]
fn model_picker_placeholder_collapses_to_a_single_trailing_gap() {
    // A placeholder state (loading / error / no match) has no counter or
    // model-name to show, so those detail rows collapse: exactly one blank
    // gap sits between the placeholder and the bottom rule — not the four
    // trailing blanks the counter/gap/name/gap layout leaves for a real
    // model. Size the buffer to the picker's natural height so the Min(0)
    // list can't expand into the gap.
    let picker = ModelPicker {
        active_id: "x".into(),
        ..ModelPicker::default()
    };
    assert_eq!(picker.status, ModelLoad::Loading);
    // 6 collapsed chrome rows + 1 placeholder list row = 7.
    let mut buf = buffer(60, 7);
    render_model_picker(buf.area, &mut buf, &picker);
    assert!(row(&buf, 0, 60).starts_with('─'), "top rule");
    assert!(row(&buf, 4, 60).contains("Loading models…"), "list row");
    assert!(row(&buf, 5, 60).trim().is_empty(), "single trailing gap");
    assert!(row(&buf, 6, 60).starts_with('─'), "bottom rule");
}

#[test]
fn model_picker_shows_an_error_placeholder_in_red() {
    let picker = ModelPicker {
        status: ModelLoad::Error("401 bad key".into()),
        active_id: "x".into(),
        ..ModelPicker::default()
    };
    let mut buf = buffer(60, 20);
    render_model_picker(buf.area, &mut buf, &picker);
    let list = row(&buf, 4, 60);
    assert!(list.contains("Error: 401 bad key"), "{list:?}");
    assert_eq!(buf[(2, 4)].fg, error_color());
}

#[test]
fn model_picker_shows_no_match_when_the_query_filters_everything() {
    let mut picker = model_picker(three_models(), 0, "x");
    picker.query = "zzzz".into();
    let mut buf = buffer(60, 20);
    render_model_picker(buf.area, &mut buf, &picker);
    assert!(row(&buf, 4, 60).contains("No matching models"));
}

#[test]
fn model_picker_waits_for_every_provider_before_showing_no_match() {
    // A partial multi-provider list cannot prove a query has no match — the
    // provider still fetching may hold the very model being searched for. The
    // placeholder stays `Loading models…` until the count settles at zero.
    let mut picker = model_picker(three_models(), 0, "x");
    picker.query = "chatgpt".into();
    picker.pending = 1;
    let mut buf = buffer(60, 20);
    render_model_picker(buf.area, &mut buf, &picker);
    let loading = row(&buf, 4, 60);
    assert!(loading.contains("Loading models…"), "{loading:?}");
    assert!(!loading.contains("No matching models"), "{loading:?}");

    // Every provider has answered: the query genuinely matches nothing now.
    picker.pending = 0;
    let mut buf = buffer(60, 20);
    render_model_picker(buf.area, &mut buf, &picker);
    let settled = row(&buf, 4, 60);
    assert!(settled.contains("No matching models"), "{settled:?}");
}

#[test]
fn cursor_sits_at_the_end_of_the_model_search_query() {
    let mut app = App::new();
    app.open_model_picker("x");
    app.set_models(three_models());
    app.model_picker.as_mut().unwrap().query = "hai".into();
    let area = Rect::new(0, 0, 60, model_picker_height(&app, 60, 40).unwrap());
    let (x, y) = cursor_position(area, &app);
    // indent(2) + prompt("❯ " = 2) + "hai"(3) = 7.
    assert_eq!((x, y), (7, MODEL_SEARCH_ROW));
}

/// Rejoin built rows word-by-word so a wrapped needle matches across rows.
fn joined(lines: &[Line<'_>]) -> String {
    lines
        .iter()
        .map(plain)
        .collect::<Vec<_>>()
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[test]
fn provider_errors_wrap_instead_of_clipping() {
    // The error text is the only diagnostic the user gets — at a narrow
    // width it wraps whole instead of losing its tail to a silent clip.
    use crate::ui::model_view::model_view_lines;
    let mut picker = model_picker(vec![], 0, "x");
    picker.status = ModelLoad::Error("legacy".into());
    picker.errors.push(ModelFetchError {
        provider: "openrouter".into(),
        message: "HTTP 429 too many requests — retry after sixty seconds".into(),
    });
    let lines = model_view_lines(&picker, 40, TALL);
    for line in &lines {
        assert!(
            crate::ui::wrap::cols(plain(line).trim_end()) <= 40,
            "no row leaks past the width: {:?}",
            plain(line)
        );
    }
    assert!(
        joined(&lines).contains("retry after sixty seconds"),
        "the error's tail survives: {lines:#?}"
    );
}

#[test]
fn the_needs_login_hint_wraps_keeping_its_actionable_tail() {
    // "No API key yet — run /login to add one": the tail IS the fix; a
    // 30-column terminal keeps it by wrapping.
    use crate::ui::model_view::model_view_lines;
    let mut picker = model_picker(vec![], 0, "x");
    picker.status = ModelLoad::NeedsLogin;
    let lines = model_view_lines(&picker, 30, TALL);
    assert!(
        joined(&lines).contains("run /login to add one"),
        "{lines:#?}"
    );
}

#[test]
fn a_cut_model_id_ends_with_an_ellipsis() {
    // One row per entry is the window's shape, so a long id can't wrap — but
    // the cut must be visible before the user picks between two clipped twins.
    let models = vec![model_entry(
        "anthropic/claude-3.5-sonnet-20241022-preview-long",
        "openrouter",
        "Sonnet",
    )];
    let picker = model_picker(models, 0, "x");
    let mut buf = buffer(40, 14);
    render_model_picker(buf.area, &mut buf, &picker);
    let first = row(&buf, 4, 40);
    assert!(
        first.contains('…') && first.contains("[openrouter]"),
        "the id marks its cut and the tag keeps its seat: {first:?}"
    );
}

#[test]
fn the_counter_suffix_clamps_to_the_width() {
    // The red "{provider} unavailable" note used to paint-clip past the
    // buffer edge; the assembled counter line now clamps with a dim `…`.
    use crate::ui::model_view::model_view_lines;
    let mut picker = model_picker(three_models(), 0, "x");
    picker.errors.push(ModelFetchError {
        provider: "a-very-long-provider-name-indeed".into(),
        message: "boom".into(),
    });
    let lines = model_view_lines(&picker, 30, TALL);
    for line in &lines {
        assert!(
            crate::ui::wrap::cols(plain(line).trim_end()) <= 30,
            "no row leaks past the width: {:?}",
            plain(line)
        );
    }
    let counter = lines
        .iter()
        .map(plain)
        .find(|l| l.contains("(1/3)"))
        .expect("the counter row");
    assert!(counter.trim_end().ends_with('…'), "{counter:?}");
}

#[test]
fn a_cut_model_name_detail_ends_with_an_ellipsis() {
    let models = vec![model_entry(
        "m/id",
        "openrouter",
        "A Very Long Friendly Display Name For A Model",
    )];
    let picker = model_picker(models, 0, "x");
    use crate::ui::model_view::model_view_lines;
    let lines = model_view_lines(&picker, 30, TALL);
    let name = lines
        .iter()
        .map(plain)
        .find(|l| l.contains("Model Name:"))
        .expect("the name row");
    assert!(name.trim_end().ends_with('…'), "{name:?}");
}

// --- a failed provider says WHY, not just that it failed (docs/llm.md) ---

#[test]
fn a_failed_provider_shows_its_reason_beneath_the_list() {
    // The collapsed `{provider} unavailable` counter suffix is a dead end: it
    // names the provider and nothing else, and the reason — the one thing that
    // tells the user whether to re-run /login, wait, or check their seat — was
    // recorded and never rendered anywhere.
    use crate::ui::model_view::model_view_lines;
    let mut picker = model_picker(three_models(), 0, "x");
    picker.errors.push(ModelFetchError {
        provider: "GitHub Copilot".into(),
        message: "HTTP 403: no Copilot subscription on this account".into(),
    });
    let text = model_view_lines(&picker, 70, TALL)
        .iter()
        .map(plain)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("GitHub Copilot unavailable"), "{text}");
    assert!(
        text.contains("no Copilot subscription on this account"),
        "the reason is on the page: {text}"
    );
}

#[test]
fn every_failed_providers_reason_is_shown() {
    use crate::ui::model_view::model_view_lines;
    let mut picker = model_picker(three_models(), 0, "x");
    for (provider, message) in [("GitHub Copilot", "boom"), ("OpenRouter", "kaboom")] {
        picker.errors.push(ModelFetchError {
            provider: provider.into(),
            message: message.into(),
        });
    }
    let text = model_view_lines(&picker, 70, TALL)
        .iter()
        .map(plain)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("2 providers unavailable"), "{text}");
    assert!(text.contains("boom") && text.contains("kaboom"), "{text}");
}

#[test]
fn a_page_length_failure_reason_is_capped_not_left_to_own_the_picker() {
    // A provider that answers a blocked request with an HTML page would
    // otherwise push the list off the top of the frame.
    use crate::ui::model_view::model_view_lines;
    let mut picker = model_picker(three_models(), 0, "x");
    picker.errors.push(ModelFetchError {
        provider: "GitHub Copilot".into(),
        message: "wordy ".repeat(400),
    });
    let lines = model_view_lines(&picker, 70, TALL);
    let reason_rows = lines
        .iter()
        .map(plain)
        .filter(|l| l.contains("wordy"))
        .count();
    assert!(
        reason_rows <= MODEL_ERROR_MAX_ROWS as usize,
        "the reason block is bounded, got {reason_rows} rows"
    );
    // …and the cut shows, or a reason clipped mid-sentence reads as one that
    // simply ended there.
    let last = lines
        .iter()
        .map(plain)
        .rfind(|l| l.contains("wordy"))
        .expect("a reason row");
    assert!(last.trim_end().ends_with('…'), "{last:?}");
    for line in &lines {
        assert!(
            crate::ui::wrap::cols(plain(line).trim_end()) <= 70,
            "no row leaks past the width: {:?}",
            plain(line)
        );
    }
}

#[test]
fn a_clean_load_adds_no_reason_rows_at_all() {
    use crate::ui::model_view::model_view_lines;
    let picker = model_picker(three_models(), 0, "x");
    let before = model_view_lines(&picker, 70, TALL).len();
    let mut failed = model_picker(three_models(), 0, "x");
    failed.errors.push(ModelFetchError {
        provider: "GitHub Copilot".into(),
        message: "boom".into(),
    });
    assert!(
        model_view_lines(&failed, 70, TALL).len() > before,
        "the block only exists when something failed"
    );
}

// --- the compact page's shared pieces (docs/view-flow.md) ---

#[test]
fn the_first_page_that_fits_wins_and_the_pages_after_it_are_never_built() {
    use crate::ui::model_view::first_page_that_fits;
    let built = std::cell::Cell::new(0);
    let page = |rows: usize| {
        built.set(built.get() + 1);
        vec![Line::default(); rows]
    };
    let whole = || page(20);
    let compact = || page(11);
    let smaller = || page(5);
    let chosen = first_page_that_fits(15, &[&whole, &compact, &smaller]);
    assert_eq!(
        chosen.len(),
        11,
        "the whole page is too tall, the compact fits"
    );
    assert_eq!(
        built.get(),
        2,
        "the page after the one that fits is never built"
    );
    assert_eq!(first_page_that_fits(20, &[&whole, &compact]).len(), 20);
}

#[test]
fn when_no_page_fits_the_last_is_used_and_bottom_anchors() {
    use crate::ui::model_view::first_page_that_fits;
    let whole = || vec![Line::default(); 20];
    let compact = || vec![Line::default(); 11];
    assert_eq!(first_page_that_fits(6, &[&whole, &compact]).len(), 11);
}

#[test]
fn the_list_window_names_what_it_hides_on_either_side() {
    use crate::ui::model_view::list_window;
    assert_eq!(list_window(8, 0, 3), (0, 0..3, 5), "pinned to the top");
    assert_eq!(
        list_window(8, 3, 3),
        (2, 2..5, 3),
        "the selection in the middle"
    );
    assert_eq!(list_window(8, 7, 3), (5, 5..8, 0), "pinned to the end");
    assert_eq!(
        list_window(2, 1, 3),
        (0, 0..2, 0),
        "a short list shows whole"
    );
}

#[test]
fn the_compact_list_reserves_a_slot_on_either_side_of_its_window() {
    use crate::ui::model_view::model_compact_list;
    let window = vec![Line::from("a"), Line::from("b"), Line::from("c")];
    let texts = |lines: Vec<Line<'static>>| -> Vec<String> {
        lines
            .iter()
            .map(|l| plain(l).trim_end().to_string())
            .collect()
    };
    assert_eq!(
        texts(model_compact_list(2, window.clone(), 4, 40)),
        ["  ↑ 2 more above", "a", "b", "c", "  ↓ 4 more below"]
    );
    // Nothing hidden that way: the slot stays, blank — one height at every
    // selection, and the gap the page wants there anyway.
    assert_eq!(
        texts(model_compact_list(0, window, 0, 40)),
        ["", "a", "b", "c", ""]
    );
}

// --- the compact page: a short terminal keeps the choices on screen ---

/// A terminal tall enough for the whole page.
const TALL: u16 = 200;

/// A ready picker over twelve models, the first highlighted and active.
fn twelve_models() -> ModelPicker {
    let models = (0..12)
        .map(|i| {
            model_entry(
                &format!("vendor/model-{i:02}"),
                "openrouter",
                &format!("Vendor: Model {i}"),
            )
        })
        .collect();
    model_picker(models, 0, "vendor/model-00")
}

fn page_at(picker: &ModelPicker, width: u16, height: u16) -> Vec<String> {
    crate::ui::model_view::model_view_lines(picker, width, height)
        .iter()
        .map(|l| plain(l).trim_end().to_string())
        .collect()
}

fn is_rule(text: &str) -> bool {
    !text.is_empty() && text.chars().all(|c| c == '─')
}

#[test]
fn a_short_terminal_keeps_the_model_choices_on_screen() {
    // The whole page — ten rows of the list, the counter and the name line —
    // is 19 rows; on 12, bottom-anchoring it flowed the search line and the
    // highlighted model into scrollback. The counter and the error rows
    // yield and three models sit between the reserved slots; with no hint
    // row of its own, the page keeps the highlighted model's name line in
    // the hint's place, so it has its siblings' shape.
    let picker = twelve_models();
    assert_eq!(page_at(&picker, 70, TALL).len(), 19, "the whole page");
    let texts = page_at(&picker, 70, 12);
    assert_eq!(texts.len(), 11, "{texts:#?}");
    assert!(is_rule(&texts[0]), "{texts:#?}");
    assert_eq!(texts[1], "", "{texts:#?}");
    assert_eq!(texts[2], "  ❯", "the search line: {texts:#?}");
    assert_eq!(texts[3], "", "nothing above the first row: {texts:#?}");
    assert!(texts[4].starts_with("→ vendor/model-00"), "{texts:#?}");
    assert!(texts[5].starts_with("  vendor/model-01"), "{texts:#?}");
    assert!(texts[6].starts_with("  vendor/model-02"), "{texts:#?}");
    assert_eq!(texts[7], "  ↓ 9 more below", "{texts:#?}");
    assert!(
        texts[8].contains("Vendor: Model 0"),
        "the name line: {texts:#?}"
    );
    assert_eq!(texts[9], "", "{texts:#?}");
    assert!(is_rule(&texts[10]), "{texts:#?}");
    assert!(
        !texts.iter().any(|t| t.contains("(1/12)")),
        "no counter: {texts:#?}"
    );
}

#[test]
fn the_compact_model_window_rides_the_selection_and_holds_still() {
    let mut picker = twelve_models();
    picker.selected = 6;
    let texts = page_at(&picker, 70, 12);
    assert_eq!(texts[3], "  ↑ 5 more above", "{texts:#?}");
    assert!(texts[5].starts_with("→ vendor/model-06"), "{texts:#?}");
    assert_eq!(texts[7], "  ↓ 4 more below", "{texts:#?}");
    for i in 0..12 {
        picker.selected = i;
        assert_eq!(
            page_at(&picker, 70, 12).len(),
            11,
            "selection {i} moved the frame"
        );
    }
}

#[test]
fn the_whole_model_page_returns_once_it_fits() {
    let picker = twelve_models();
    let whole = u16::try_from(page_at(&picker, 70, TALL).len()).expect("rows");
    let has_counter = |texts: &[String]| texts.iter().any(|t| t.contains("(1/12)"));
    assert!(
        has_counter(&page_at(&picker, 70, whole)),
        "whole at its own height"
    );
    assert!(
        !has_counter(&page_at(&picker, 70, whole - 1)),
        "compact a row short"
    );
}

#[test]
fn a_placeholder_page_has_no_compact_form() {
    // Loading, no key, every provider failed, nothing matched: no list to
    // compact, so the page is the same at every height.
    let mut picker = twelve_models();
    picker.query = "zzz".into();
    assert_eq!(page_at(&picker, 70, 6), page_at(&picker, 70, TALL));
}

#[test]
fn the_cursor_sits_on_the_compact_model_search_line() {
    let mut app = App::new();
    app.open_model_picker("vendor/model-00");
    app.set_models(twelve_models().models);
    let height = model_picker_height(&app, 70, 12).expect("open");
    assert_eq!(height, 11, "the compact page");
    let area = Rect::new(0, 1, 70, height);
    let (_, y) = cursor_position(area, &app);
    assert_eq!(y, 1 + MODEL_SEARCH_ROW, "on the search line");
    assert!(
        crate::ui::view_flow(&app, 70, 12, 1000).is_none(),
        "nothing flows"
    );
}
