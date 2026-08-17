//! The inline `/model` picker (`docs/llm.md`).

use super::*;
use crate::ui::theme::{ERROR_COLOR, MODEL_MENU_MAX_ROWS, MODEL_SEARCH_ROW, MODEL_SELECTED_COLOR};

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
    assert_eq!(buf[(2, MODEL_SEARCH_ROW)].fg, MODEL_SELECTED_COLOR);
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
    assert_eq!(buf[(2, 4)].fg, ERROR_COLOR);
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
    let lines = model_view_lines(&picker, 40);
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
    let lines = model_view_lines(&picker, 30);
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
    let lines = model_view_lines(&picker, 30);
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
    let lines = model_view_lines(&picker, 30);
    let name = lines
        .iter()
        .map(plain)
        .find(|l| l.contains("Model Name:"))
        .expect("the name row");
    assert!(name.trim_end().ends_with('…'), "{name:?}");
}
