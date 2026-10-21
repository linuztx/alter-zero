//! The inline `/model` picker (`docs/llm.md`).

use super::*;
use crate::ui::theme::{
    MODEL_ERROR_MAX_ROWS, MODEL_MENU_MAX_ROWS, error_color, model_selected_color,
};

#[test]
fn model_picker_frames_with_rules_and_no_header() {
    let picker = model_picker(three_models(), 0, "anthropic/claude-3-haiku");
    let mut buf = buffer(60, 20);
    render_model_picker(buf.area, &mut buf, &picker);
    // Top rule, then the search line straight under it: no header banner
    // (the old "Showing models…" was dropped) and no padding row either.
    assert!(row(&buf, 0, 60).starts_with('─'), "top rule");
    assert!(
        row(&buf, SEARCH_ROW, 60).contains('❯'),
        "the search line hugs the rule"
    );
    assert_eq!(SEARCH_ROW, 1, "straight under the rule");
}

#[test]
fn the_picker_frame_hugs_its_content_in_every_state() {
    // The search line sits straight under the opening rule and the last
    // row of content straight over the closing one — a real list's
    // `Model Name:` line, else the placeholder — never a padding row.
    use crate::ui::model_view::model_view_lines;
    let ready = model_picker(three_models(), 0, "x");
    let mut no_match = model_picker(three_models(), 0, "x");
    no_match.query = "zzzz".into();
    let mut partial = model_picker(three_models(), 0, "x");
    partial.errors.push(ModelFetchError {
        provider: "Agent Zero API".into(),
        message: "HTTP 401".into(),
    });
    let loading = ModelPicker {
        active_id: "x".into(),
        ..ModelPicker::default()
    };
    let error = ModelPicker {
        status: ModelLoad::Error("401 bad key".into()),
        active_id: "x".into(),
        ..ModelPicker::default()
    };
    let needs_login = ModelPicker {
        status: ModelLoad::NeedsLogin,
        active_id: "x".into(),
        ..ModelPicker::default()
    };
    for (what, picker) in [
        ("a ready list", &ready),
        ("a search matching nothing", &no_match),
        ("a list with a failed provider", &partial),
        ("the loading placeholder", &loading),
        ("the error placeholder", &error),
        ("the login hint", &needs_login),
    ] {
        assert_frame_hugs_content(what, &model_view_lines(picker, 60, u16::MAX));
    }
}

#[test]
fn model_picker_shows_the_search_prompt_and_query() {
    let mut picker = model_picker(three_models(), 0, "x");
    picker.query = "haiku".into();
    let mut buf = buffer(60, 20);
    render_model_picker(buf.area, &mut buf, &picker);
    let search = row(&buf, SEARCH_ROW, 60);
    assert!(search.contains("❯ haiku"), "{search:?}");
    // The `❯` prompt is cyan.
    assert_eq!(buf[(2, SEARCH_ROW)].fg, model_selected_color());
}

#[test]
fn model_picker_counter_and_name_reflect_the_selection() {
    let picker = model_picker(three_models(), 2, "x");
    // Size the buffer to the picker's natural height (7 chrome + 3 list),
    // like the boundary does — otherwise the Min(0) list would expand and
    // push the counter/name rows down.
    let mut buf = buffer(60, 10);
    render_model_picker(buf.area, &mut buf, &picker);
    // Counter row = top(0) search(1) gap(2) list(3,4,5) → 6.
    let counter = row(&buf, 6, 60);
    assert!(counter.contains("(3/3)"), "{counter:?}");
    // Model-name row = counter(6) + gap(7) + 1 = 8.
    let name = row(&buf, 8, 60);
    assert!(
        name.contains("Model Name: MoonshotAI: Kimi K2.6"),
        "{name:?}"
    );
    // The bottom rule straight under it — the user's mock, no trailing gap.
    assert!(row(&buf, 9, 60).starts_with('─'), "bottom rule");
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
    // The list spans rows 3..13 — top(0) search(1) gap(2) then 10 rows.
    let list: Vec<String> = (3..13).map(|y| row(&buf, y, 60)).collect();
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
    let list = row(&buf, 3, 60);
    assert!(list.contains("Loading models…"), "{list:?}");
}

#[test]
fn model_picker_placeholder_closes_the_page() {
    // A placeholder state (loading / error / no match) has no counter or
    // model-name to show, so those detail rows collapse away: the
    // placeholder sits straight over the bottom rule — not over the band of
    // blanks the counter/gap/name layout leaves for a real model. Size the
    // buffer to the picker's natural height so the Min(0) list can't expand
    // into the gap.
    let picker = ModelPicker {
        active_id: "x".into(),
        ..ModelPicker::default()
    };
    assert_eq!(picker.status, ModelLoad::Loading);
    // 4 collapsed chrome rows + 1 placeholder list row = 5.
    let mut buf = buffer(60, 5);
    render_model_picker(buf.area, &mut buf, &picker);
    assert!(row(&buf, 0, 60).starts_with('─'), "top rule");
    assert!(row(&buf, 3, 60).contains("Loading models…"), "list row");
    assert!(row(&buf, 4, 60).starts_with('─'), "bottom rule");
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
    let list = row(&buf, 3, 60);
    assert!(list.contains("Error: 401 bad key"), "{list:?}");
    assert_eq!(buf[(2, 3)].fg, error_color());
}

#[test]
fn model_picker_shows_no_match_when_the_query_filters_everything() {
    let mut picker = model_picker(three_models(), 0, "x");
    picker.query = "zzzz".into();
    let mut buf = buffer(60, 20);
    render_model_picker(buf.area, &mut buf, &picker);
    assert!(row(&buf, 3, 60).contains("No matching models"));
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
    let loading = row(&buf, 3, 60);
    assert!(loading.contains("Loading models…"), "{loading:?}");
    assert!(!loading.contains("No matching models"), "{loading:?}");

    // Every provider has answered: the query genuinely matches nothing now.
    picker.pending = 0;
    let mut buf = buffer(60, 20);
    render_model_picker(buf.area, &mut buf, &picker);
    let settled = row(&buf, 3, 60);
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
    assert_eq!((x, y), (7, SEARCH_ROW));
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
    // "No provider set up yet — run /login to add one": the tail IS the fix;
    // a 30-column terminal keeps it by wrapping.
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
fn the_needs_login_hint_does_not_presume_an_api_key() {
    // `/login` has three ways in — a subscription, a key, a local server —
    // and a hint naming only a key sends someone with a local model looking
    // for a key they will never have (docs/local-models.md).
    use crate::ui::model_view::model_view_lines;
    let mut picker = model_picker(vec![], 0, "x");
    picker.status = ModelLoad::NeedsLogin;
    let text = joined(&model_view_lines(&picker, 80, TALL));
    assert!(text.contains("No provider set up yet"), "{text}");
    assert!(!text.contains("API key"), "{text}");
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
    let first = row(&buf, 3, 40);
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

// --- the compact page's rungs: the search line never yields ---

/// A `len`-row list named `row 0`, `row 1`, … with `selected` marked `→` —
/// a compact page's window, riding the selection like every picker's.
fn named_rows(len: usize, selected: usize) -> impl Fn(usize) -> (usize, Vec<Line<'static>>, usize) {
    move |max| {
        let (above, range, below) = crate::ui::model_view::list_window(len, selected, max);
        let rows = range
            .map(|i| {
                Line::from(if i == selected {
                    format!("→ row {i}")
                } else {
                    format!("  row {i}")
                })
            })
            .collect();
        (above, rows, below)
    }
}

/// The compact page on a terminal `height` rows tall over a `len`-row list
/// with `selected` highlighted and `tail` under it, as trimmed plain rows.
fn compact_at(height: u16, len: usize, selected: usize, tail: &[&str]) -> Vec<String> {
    use crate::ui::model_view::compact_page;
    let search = Line::from("  ❯ query");
    let tail: Vec<Line<'static>> = tail.iter().map(|t| Line::from(t.to_string())).collect();
    compact_page(height, &search, &named_rows(len, selected), &tail, 20)
        .iter()
        .map(|l| plain(l).trim_end().to_string())
        .collect()
}

#[test]
fn the_compact_page_sheds_a_row_at_a_time_and_never_its_search_line() {
    // Bottom-anchored, a compact page taller than the terminal flowed its
    // search line into scrollback — and the caret with it — and, two rows
    // shorter, the highlighted row too. codex's rule instead: the search
    // line and the highlighted row stay; what is optional yields, one row at
    // a time, so every height has a page that fits it.
    let rule = "─".repeat(20);
    let rule = rule.as_str();
    let expect = |rows: &[&str]| rows.iter().map(|r| r.to_string()).collect::<Vec<_>>();
    // The compact page hugs its rules like every page: the search line
    // straight under the top one, the hint straight over the bottom one.
    let compact = expect(&[
        rule,
        "  ❯ query",
        "  ↑ 4 more above",
        "  row 4",
        "→ row 5",
        "  row 6",
        "  ↓ 4 more below",
        "  hint",
        rule,
    ]);
    assert_eq!(
        compact_at(9, 11, 5, &["  hint"]),
        compact,
        "the compact page"
    );
    // The overflow slots go first, a pair: codex reserves them only beside
    // three rows.
    let unslotted = expect(&[
        rule,
        "  ❯ query",
        "  row 4",
        "→ row 5",
        "  row 6",
        "  hint",
        rule,
    ]);
    assert_eq!(
        compact_at(8, 11, 5, &["  hint"]),
        unslotted,
        "the pair is two rows"
    );
    assert_eq!(compact_at(7, 11, 5, &["  hint"]), unslotted);
    // Then the top rule — on a terminal this short nothing is above it.
    assert_eq!(
        compact_at(6, 11, 5, &["  hint"]),
        expect(&["  ❯ query", "  row 4", "→ row 5", "  row 6", "  hint", rule])
    );
    // Then the rows, down to the highlighted one.
    assert_eq!(
        compact_at(5, 11, 5, &["  hint"]),
        expect(&["  ❯ query", "  row 4", "→ row 5", "  hint", rule])
    );
    assert_eq!(
        compact_at(4, 11, 5, &["  hint"]),
        expect(&["  ❯ query", "→ row 5", "  hint", rule])
    );
    // Then the bottom rule, the hint, and last the highlighted row.
    assert_eq!(
        compact_at(3, 11, 5, &["  hint"]),
        expect(&["  ❯ query", "→ row 5", "  hint"])
    );
    assert_eq!(
        compact_at(2, 11, 5, &["  hint"]),
        expect(&["  ❯ query", "→ row 5"])
    );
    assert_eq!(compact_at(1, 11, 5, &["  hint"]), expect(&["  ❯ query"]));
}

#[test]
fn every_compact_page_fits_keeps_its_search_line_and_holds_one_height() {
    for len in [1_usize, 2, 3, 4, 11] {
        for height in 1_u16..=12 {
            let mut heights = Vec::new();
            for selected in 0..len {
                let texts = compact_at(height, len, selected, &["  hint"]);
                let context = format!("len {len} height {height} at {selected}");
                assert!(texts.len() <= usize::from(height), "{context}: {texts:#?}");
                assert!(
                    texts.iter().any(|t| t == "  ❯ query"),
                    "{context}: the search line stays: {texts:#?}"
                );
                if height >= 2 {
                    let mark = format!("→ row {selected}");
                    assert!(texts.contains(&mark), "{context}: {texts:#?}");
                }
                if height >= 3 {
                    assert!(texts.iter().any(|t| t == "  hint"), "{context}: {texts:#?}");
                }
                assert!(
                    texts
                        .windows(2)
                        .all(|pair| !(pair[0].is_empty() && pair[1].is_empty())),
                    "{context}: two blank rows stack: {texts:#?}"
                );
                let rule = "─".repeat(20);
                assert!(
                    !(texts.first() == Some(&rule) && texts.get(1).is_some_and(String::is_empty)),
                    "{context}: a blank row hugs the top rule: {texts:#?}"
                );
                assert!(
                    !(texts.last() == Some(&rule)
                        && texts.iter().rev().nth(1).is_some_and(String::is_empty)),
                    "{context}: a blank row hugs the bottom rule: {texts:#?}"
                );
                heights.push(texts.len());
            }
            assert!(
                heights.windows(2).all(|pair| pair[0] == pair[1]),
                "len {len} height {height}: one height at every selection: {heights:?}"
            );
        }
    }
}

#[test]
fn a_page_with_no_tail_closes_on_its_list() {
    // `/model`'s placeholder pages have nothing under the list, so the page
    // closes on it: a blank `↓` slot there would hug the bottom rule
    // (`docs/view-flow.md` *The frame hugs its content*). The `↑` slot under
    // the search line is the gap the page keeps there anyway.
    let rule = "─".repeat(20);
    assert_eq!(
        compact_at(20, 2, 0, &[]),
        [
            rule.as_str(),
            "  ❯ query",
            "",
            "→ row 0",
            "  row 1",
            rule.as_str()
        ]
        .map(str::to_string)
    );
    assert_eq!(
        compact_at(4, 2, 0, &[]),
        ["  ❯ query", "→ row 0", "  row 1", rule.as_str()].map(str::to_string)
    );
}

#[test]
fn a_placeholder_window_is_whole_until_the_list_would_shrink() {
    use crate::ui::model_view::placeholder_window;
    let block: Vec<Line<'static>> = (0..4).map(|i| Line::from(format!("line {i}"))).collect();
    let rows = |max| {
        placeholder_window(&block, max)
            .1
            .iter()
            .map(plain)
            .collect::<Vec<_>>()
    };
    assert_eq!(placeholder_window(&block, 3).0, 0, "nothing hidden above");
    assert_eq!(placeholder_window(&block, 3).2, 0, "nothing hidden below");
    assert_eq!(rows(3), ["line 0", "line 1", "line 2", "line 3"], "whole");
    assert_eq!(rows(2), ["line 0", "line 1"], "its head");
    assert_eq!(rows(1), ["line 0"]);
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
    // is 17 rows; on 12, bottom-anchoring it flowed the search line and the
    // highlighted model into scrollback. The counter and the error rows
    // yield and three models sit between the reserved slots; with no hint
    // row of its own, the page keeps the highlighted model's name line in
    // the hint's place, so it has its siblings' shape.
    let picker = twelve_models();
    assert_eq!(page_at(&picker, 70, TALL).len(), 17, "the whole page");
    let texts = page_at(&picker, 70, 12);
    assert_eq!(texts.len(), 9, "{texts:#?}");
    assert!(is_rule(&texts[0]), "{texts:#?}");
    assert_eq!(texts[1], "  ❯", "the search line: {texts:#?}");
    assert_eq!(texts[2], "", "nothing above the first row: {texts:#?}");
    assert!(texts[3].starts_with("→ vendor/model-00"), "{texts:#?}");
    assert!(texts[4].starts_with("  vendor/model-01"), "{texts:#?}");
    assert!(texts[5].starts_with("  vendor/model-02"), "{texts:#?}");
    assert_eq!(texts[6], "  ↓ 9 more below", "{texts:#?}");
    assert!(
        texts[7].contains("Vendor: Model 0"),
        "the name line: {texts:#?}"
    );
    assert!(is_rule(&texts[8]), "{texts:#?}");
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
    assert_eq!(texts[2], "  ↑ 5 more above", "{texts:#?}");
    assert!(texts[4].starts_with("→ vendor/model-06"), "{texts:#?}");
    assert_eq!(texts[6], "  ↓ 4 more below", "{texts:#?}");
    for i in 0..12 {
        picker.selected = i;
        assert_eq!(
            page_at(&picker, 70, 12).len(),
            9,
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
fn a_placeholder_page_sheds_like_the_list_page() {
    // Loading, no key, every provider failed, nothing matched: the
    // placeholder stands in the list's place and the page sheds the same
    // rungs, keeping the search line — what was typed — over it. Tall, it
    // is the page it always was: the placeholder one gap under the search
    // line, closing the page against its bottom rule.
    let mut picker = twelve_models();
    picker.query = "zzz".into();
    let rule = "─".repeat(70);
    assert_eq!(
        page_at(&picker, 70, TALL),
        [
            rule.as_str(),
            "  ❯ zzz",
            "",
            "  No matching models",
            rule.as_str()
        ]
        .map(str::to_string)
    );
    assert_eq!(
        page_at(&picker, 70, 4),
        [
            rule.as_str(),
            "  ❯ zzz",
            "  No matching models",
            rule.as_str()
        ]
        .map(str::to_string)
    );
    assert_eq!(
        page_at(&picker, 70, 3),
        ["  ❯ zzz", "  No matching models", rule.as_str()].map(str::to_string)
    );
}

#[test]
fn the_cursor_sits_on_the_compact_model_search_line() {
    let mut app = App::new();
    app.open_model_picker("vendor/model-00");
    app.set_models(twelve_models().models);
    let height = model_picker_height(&app, 70, 12).expect("open");
    assert_eq!(height, 9, "the compact page");
    let area = Rect::new(0, 1, 70, height);
    let (_, y) = cursor_position(area, &app);
    assert_eq!(y, 1 + SEARCH_ROW, "on the search line");
    assert!(
        crate::ui::view_flow(&app, 70, 12, 1000).is_none(),
        "nothing flows"
    );
}

// --- a terminal shorter than the compact page keeps the search line ---

fn model_page_at(picker: &ModelPicker, width: u16, height: u16) -> Vec<String> {
    crate::ui::model_view::model_view_lines(picker, width, height)
        .iter()
        .map(|l| plain(l).trim_end().to_string())
        .collect()
}

#[test]
fn a_tiny_terminal_keeps_the_model_search_line() {
    // `/model` has no hint row: its tail is the highlighted model's name.
    let picker = twelve_models();
    assert_sheds_to_its_search_line(
        |height| model_page_at(&picker, 70, height),
        "",
        "→ vendor/model-00",
        "Vendor: Model 0",
    );
    let mut app = App::new();
    app.open_model_picker("vendor/model-00");
    app.set_models(twelve_models().models);
    for height in 1..=10 {
        let page = model_page_at(app.model_picker.as_ref().expect("open"), 70, height);
        assert_caret_on_search_line(&app, &page, 70, "");
    }
}

#[test]
fn a_tiny_terminal_keeps_the_model_search_line_over_a_placeholder() {
    // Loading, an error, no match: no tail, the placeholder in the list's
    // place.
    let mut picker = model_picker(three_models(), 0, "x");
    picker.query = "zzzz".into();
    for height in 1..=10_u16 {
        let texts = model_page_at(&picker, 70, height);
        assert!(texts.len() <= usize::from(height), "{height}: {texts:#?}");
        assert!(
            texts.iter().any(|t| t == "  ❯ zzzz"),
            "{height}: {texts:#?}"
        );
        if height >= 2 {
            assert!(
                texts.iter().any(|t| t.contains("No matching models")),
                "{height}: {texts:#?}"
            );
        }
    }
}
