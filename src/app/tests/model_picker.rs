//! The inline `/model` picker and its multi-provider load (`docs/llm.md`).

use super::*;
use crate::llm::models_cache::CachedListing;

#[test]
fn backtab_with_the_model_picker_open_is_inert() {
    // The picker owns every key while open — Shift+Tab must not cycle the
    // permission mode out from under it, nor Ctrl+T the thinking mode
    // (docs/permissions.md, docs/reasoning.md).
    let mut app = model_app(&sample_models());
    app.set_permission_mode(Some(PermissionMode::Manual));
    app.set_thinking(Some((
        trio_support(),
        ThinkingMode::Effort(ReasoningEffort::Medium),
    )));
    assert_eq!(app.on_key(backtab()), Action::None);
    assert_eq!(
        app.permission_mode(),
        Some(PermissionMode::Manual),
        "mode unchanged"
    );
    assert_eq!(app.on_key(ctrl('t')), Action::None);
    assert_eq!(
        app.thinking.as_ref().unwrap().mode,
        ThinkingMode::Effort(ReasoningEffort::Medium),
        "thinking unchanged"
    );
}

#[test]
fn selecting_a_model_carries_its_reasoning_support() {
    // Enter on a picker row hands the loop the entry's parsed support, so
    // a successful switch can seed the cycle without refetching /models.
    let mut with_support = model("thinker", "openrouter", "Thinker");
    with_support.reasoning = Some(trio_support());
    let mut app = model_app(&[
        model("anthropic/claude-3.5-haiku", "openrouter", "Haiku"),
        with_support,
    ]);
    type_chars(&mut app, "thinker");
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::SelectModel {
            provider: "openrouter".into(),
            id: "thinker".into(),
            reasoning: Some(trio_support()),
            vision: None,
            context: None,
            service_tiers: Vec::new(),
        }
    );
}

#[test]
fn selecting_a_model_carries_its_vision_support() {
    // Enter also hands the loop the entry's image-input support, so the
    // rebuilt backend gates attachments without refetching /models
    // (docs/tools.md).
    let mut blind = model("openai/gpt-oss-120b", "openrouter", "GPT OSS");
    blind.vision = Some(false);
    let mut app = model_app(&[blind]);
    let action = app.on_key(key(KeyCode::Enter));
    assert!(
        matches!(
            action,
            Action::SelectModel {
                vision: Some(false),
                ..
            }
        ),
        "the entry's vision rides the action: {action:?}"
    );
}

#[test]
fn paste_into_the_model_filter_extends_the_query() {
    // A model id is copied from a docs page or the provider's dashboard far
    // more often than it is typed out, so the picker's search takes pastes
    // like the /login provider step and the /resume picker (they used to be
    // swallowed outright — nothing happened at all).
    let mut app = model_app(&sample_models());
    type_chars(&mut app, "clau");
    app.paste_into_model_filter("de-3.5-haiku");
    let picker = app.model_picker.as_ref().expect("the picker is open");
    assert_eq!(picker.query, "claude-3.5-haiku");
    assert_eq!(
        picker.selected, 0,
        "a narrowed filter re-seats the highlight"
    );
    assert!(
        picker.matches().iter().any(|m| m.id.contains("haiku")),
        "the pasted id filters the list: {:?}",
        picker.matches()
    );
}

#[test]
fn paste_into_the_model_filter_flattens_newlines_and_ignores_blanks() {
    // A copied id usually drags a trailing newline; a multi-line paste
    // flattens to spaces rather than smuggling control characters into the
    // one-line filter. An all-whitespace paste is a no-op.
    let mut app = model_app(&sample_models());
    app.paste_into_model_filter("  openai/gpt-4o-mini \n");
    let query = app.model_picker.as_ref().unwrap().query.clone();
    assert_eq!(query, "openai/gpt-4o-mini");
    app.paste_into_model_filter("   \n\t ");
    assert_eq!(
        app.model_picker.as_ref().unwrap().query,
        query,
        "a blank paste changes nothing"
    );
    assert!(
        !app.model_picker.as_ref().unwrap().query.contains('\n'),
        "no newline reaches the filter"
    );
}

#[test]
fn paste_with_the_model_picker_open_never_reaches_the_composer() {
    // The picker only replaces the composer visually — the draft underneath
    // must stay untouched (the reason pastes were swallowed in the first
    // place; now they route to the filter instead).
    let mut app = App::new();
    type_chars(&mut app, "a draft");
    app.open_model_picker("openai/gpt-4o-mini");
    app.paste_into_model_filter("pasted");
    assert_eq!(
        app.input.text(),
        "a draft",
        "the composer draft is untouched"
    );
    assert_eq!(app.model_picker.as_ref().unwrap().query, "pasted");
}

#[test]
fn slash_model_runs_to_open_the_picker_when_idle() {
    let mut app = App::new();
    type_chars(&mut app, "/model");
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::OpenModelPicker);
    assert!(app.input.is_empty(), "running a command clears the draft");
}

#[test]
fn open_model_picker_starts_loading_and_stays_in_the_conversation() {
    let mut app = App::new();
    app.open_model_picker("m1");
    let picker = app.model_picker.as_ref().expect("picker open");
    assert_eq!(picker.status, ModelLoad::Loading);
    assert_eq!(picker.active_id, "m1");
    assert_eq!(
        app.view,
        View::Conversation,
        "the picker is inline, not an overlay"
    );
}

#[test]
fn set_models_marks_ready_and_seats_on_the_active_model() {
    let mut app = App::new();
    app.open_model_picker("anthropic/claude-fable-5");
    app.set_models(sample_models());
    let picker = app.model_picker.as_ref().unwrap();
    assert_eq!(picker.status, ModelLoad::Ready);
    // claude-fable-5 is index 1 in the alphabetical list.
    assert_eq!(picker.selected, 1);
    assert_eq!(picker.highlighted().unwrap().id, "anthropic/claude-fable-5");
}

#[test]
fn set_models_seats_on_top_when_active_is_absent() {
    let mut app = App::new();
    app.open_model_picker("not/present");
    app.set_models(sample_models());
    assert_eq!(app.model_picker.as_ref().unwrap().selected, 0);
}

#[test]
fn set_models_error_shows_the_message() {
    let mut app = App::new();
    app.open_model_picker("m");
    app.set_models_error("boom");
    assert_eq!(
        app.model_picker.as_ref().unwrap().status,
        ModelLoad::Error("boom".into())
    );
}

#[test]
fn set_models_is_a_noop_when_the_picker_is_closed() {
    let mut app = App::new();
    app.set_models(sample_models()); // no panic, no state
    assert!(app.model_picker.is_none());
}

#[test]
fn set_models_needs_login_clears_the_list_and_points_at_login() {
    let mut app = App::new();
    app.open_model_picker("m");
    app.set_models(sample_models()); // pretend a stale list was there
    app.set_models_needs_login();
    let picker = app.model_picker.as_ref().unwrap();
    assert_eq!(picker.status, ModelLoad::NeedsLogin);
    assert!(picker.models.is_empty(), "no models offered without a key");
    assert!(picker.matches().is_empty());
    assert_eq!(picker.selected, 0);
}

// --- Multi-provider parallel `/model` load (docs/llm.md), and the kept
// lists behind it (docs/model-cache.md). ---

/// A provider the picker waits on.
fn target(provider: &str, name: &str) -> ModelFetchTarget {
    ModelFetchTarget {
        provider: provider.into(),
        name: name.into(),
    }
}

/// A kept list read back from `model-cache.json`.
fn kept(provider: &str, fetched_at: i64, models: Vec<ModelEntry>) -> CachedListing {
    CachedListing {
        provider: provider.into(),
        fetched_at,
        models,
    }
}

const NOW: i64 = 1_763_110_000;

fn ids(app: &App) -> Vec<String> {
    app.model_picker
        .as_ref()
        .unwrap()
        .models
        .iter()
        .map(|m| format!("{}@{}", m.id, m.provider))
        .collect()
}

#[test]
fn a_landed_list_shows_at_once_and_merges_with_the_rest() {
    let mut app = App::new();
    app.open_model_picker("x");
    app.begin_model_load(
        Vec::new(),
        vec![
            target("openrouter", "OpenRouter"),
            target("a0_venice", "Agent Zero API"),
        ],
    );
    assert_eq!(
        app.model_picker.as_ref().unwrap().status,
        ModelLoad::Loading
    );
    // OpenRouter lands first — its list shows immediately, one fetch still out.
    app.finish_model_fetch(
        "openrouter",
        Ok(vec![
            model("c/z", "openrouter", "C Z"),
            model("a/x", "openrouter", "A X"),
        ]),
        NOW,
    );
    let picker = app.model_picker.as_ref().unwrap();
    assert_eq!(
        picker.status,
        ModelLoad::Ready,
        "shown before the rest arrive"
    );
    assert_eq!(picker.pending(), 1);
    assert_eq!(picker.models.len(), 2);
    // Agent Zero lands second — merged into one sorted list, no fetches left.
    app.finish_model_fetch("a0_venice", Ok(vec![model("b/y", "a0_venice", "B Y")]), NOW);
    assert_eq!(app.model_picker.as_ref().unwrap().pending(), 0);
    assert_eq!(
        ids(&app),
        ["a/x@openrouter", "b/y@a0_venice", "c/z@openrouter"],
        "merged + sorted across providers"
    );
}

#[test]
fn a_failed_provider_keeps_a_partial_list_and_notes_the_failure_by_name() {
    let mut app = App::new();
    app.open_model_picker("x");
    app.begin_model_load(
        Vec::new(),
        vec![
            target("openrouter", "OpenRouter"),
            target("a0_venice", "Agent Zero API"),
        ],
    );
    app.finish_model_fetch(
        "openrouter",
        Ok(vec![model("a/x", "openrouter", "A X")]),
        NOW,
    );
    app.finish_model_fetch("a0_venice", Err("HTTP 401: unauthorized".into()), NOW);
    let picker = app.model_picker.as_ref().unwrap();
    // The good provider's list stays; the failure is recorded, not fatal.
    assert_eq!(picker.status, ModelLoad::Ready);
    assert_eq!(picker.pending(), 0);
    assert_eq!(picker.models.len(), 1);
    assert_eq!(
        picker.errors,
        [ModelFetchError {
            provider: "Agent Zero API".into(),
            message: "HTTP 401: unauthorized".into(),
        }]
    );
    assert!(picker.stale.is_empty(), "nothing was on screen to keep");
}

#[test]
fn all_providers_failing_settles_into_an_error() {
    let mut app = App::new();
    app.open_model_picker("x");
    app.begin_model_load(
        Vec::new(),
        vec![
            target("openrouter", "OpenRouter"),
            target("a0_venice", "Agent Zero API"),
        ],
    );
    app.finish_model_fetch("openrouter", Err("HTTP 500".into()), NOW);
    // Still loading — one fetch outstanding, nothing to show yet.
    assert_eq!(
        app.model_picker.as_ref().unwrap().status,
        ModelLoad::Loading
    );
    app.finish_model_fetch("a0_venice", Err("HTTP 401".into()), NOW);
    let picker = app.model_picker.as_ref().unwrap();
    assert!(matches!(picker.status, ModelLoad::Error(_)));
    assert!(picker.models.is_empty());
    assert_eq!(picker.errors.len(), 2);
}

#[test]
fn a_landed_list_reseats_on_the_active_model_once_its_provider_lands() {
    let mut app = App::new();
    app.open_model_picker("b/y"); // active lives in the second provider
    app.begin_model_load(
        Vec::new(),
        vec![
            target("openrouter", "OpenRouter"),
            target("a0_venice", "A0"),
        ],
    );
    app.finish_model_fetch(
        "openrouter",
        Ok(vec![model("a/x", "openrouter", "A X")]),
        NOW,
    );
    // Active isn't here yet — seated on top.
    assert_eq!(app.model_picker.as_ref().unwrap().selected, 0);
    app.finish_model_fetch("a0_venice", Ok(vec![model("b/y", "a0_venice", "B Y")]), NOW);
    // Its provider landed and the user hadn't moved — jump to the active row.
    let picker = app.model_picker.as_ref().unwrap();
    assert_eq!(picker.highlighted().unwrap().id, "b/y");
}

#[test]
fn active_mark_matches_the_provider_not_just_the_id() {
    // The merged list can carry the same id under two providers; only the
    // row from the active provider is the ✓ row.
    let mut app = App::new();
    app.open_model_picker("shared/id");
    app.set_active_provider("a0_venice");
    app.begin_model_load(
        Vec::new(),
        vec![
            target("openrouter", "OpenRouter"),
            target("a0_venice", "A0"),
        ],
    );
    app.finish_model_fetch(
        "openrouter",
        Ok(vec![model("shared/id", "openrouter", "OR")]),
        NOW,
    );
    app.finish_model_fetch(
        "a0_venice",
        Ok(vec![model("shared/id", "a0_venice", "A0")]),
        NOW,
    );
    let picker = app.model_picker.as_ref().unwrap();
    let or = picker
        .models
        .iter()
        .find(|m| m.provider == "openrouter")
        .unwrap();
    let a0 = picker
        .models
        .iter()
        .find(|m| m.provider == "a0_venice")
        .unwrap();
    assert!(
        !picker.is_active(or),
        "same id, other provider is not active"
    );
    assert!(picker.is_active(a0), "the active provider's row is marked");
    assert_eq!(
        picker.highlighted(),
        Some(a0),
        "the highlight seats on the active provider's row, not the first id"
    );
}

#[test]
fn a_landed_list_preserves_the_highlight_after_the_user_navigates() {
    let mut app = App::new();
    app.open_model_picker("x");
    app.begin_model_load(
        Vec::new(),
        vec![
            target("openrouter", "OpenRouter"),
            target("a0_venice", "A0"),
        ],
    );
    app.finish_model_fetch(
        "openrouter",
        Ok(vec![
            model("a/x", "openrouter", "A X"),
            model("c/z", "openrouter", "C Z"),
        ]),
        NOW,
    );
    app.on_key(key(KeyCode::Down)); // highlight c/z (index 1)
    assert_eq!(
        app.model_picker.as_ref().unwrap().highlighted().unwrap().id,
        "c/z"
    );
    // A merge pushes a row in between — the highlight rides c/z, not the index.
    app.finish_model_fetch("a0_venice", Ok(vec![model("b/y", "a0_venice", "B Y")]), NOW);
    assert_eq!(
        app.model_picker.as_ref().unwrap().highlighted().unwrap().id,
        "c/z"
    );
}

#[test]
fn kept_lists_show_at_once_seated_on_the_active_model() {
    // The point of the cache (docs/model-cache.md): no `Loading models…` for
    // a list read before — the rows are there the moment the picker opens.
    let mut app = App::new();
    app.open_model_picker("b/y");
    app.begin_model_load(
        vec![
            kept(
                "openrouter",
                NOW - 60,
                vec![
                    model("a/x", "openrouter", "A X"),
                    model("c/z", "openrouter", "C Z"),
                ],
            ),
            kept(
                "a0_venice",
                NOW - 120,
                vec![model("b/y", "a0_venice", "B Y")],
            ),
        ],
        Vec::new(),
    );
    let picker = app.model_picker.as_ref().unwrap();
    assert_eq!(picker.status, ModelLoad::Ready);
    assert_eq!(picker.pending(), 0);
    assert_eq!(picker.highlighted().unwrap().id, "b/y");
    assert_eq!(
        ids(&app),
        ["a/x@openrouter", "b/y@a0_venice", "c/z@openrouter"]
    );
    let picker = app.model_picker.as_ref().unwrap();
    assert_eq!(picker.fetched_at.get("openrouter"), Some(&(NOW - 60)));
    assert_eq!(picker.oldest_fetch(), Some(NOW - 120), "the oldest list's");
}

#[test]
fn a_refresh_replaces_its_providers_rows() {
    // Models the provider no longer lists go, new ones appear — a merge
    // that only appended would keep a model the provider has dropped.
    let mut app = App::new();
    app.open_model_picker("a/x");
    app.begin_model_load(
        vec![kept(
            "openrouter",
            NOW - 7_200,
            vec![
                model("a/x", "openrouter", "A X"),
                model("gone/m", "openrouter", "Gone"),
            ],
        )],
        vec![target("openrouter", "OpenRouter")],
    );
    app.finish_model_fetch(
        "openrouter",
        Ok(vec![
            model("a/x", "openrouter", "A X"),
            model("new/m", "openrouter", "New"),
        ]),
        NOW,
    );
    assert_eq!(ids(&app), ["a/x@openrouter", "new/m@openrouter"]);
    let picker = app.model_picker.as_ref().unwrap();
    assert_eq!(
        picker.fetched_at.get("openrouter"),
        Some(&NOW),
        "stamped now"
    );
    assert_eq!(picker.pending(), 0);
}

#[test]
fn a_refresh_leaves_the_other_providers_rows_alone() {
    let mut app = App::new();
    app.open_model_picker("x");
    app.begin_model_load(
        vec![
            kept(
                "openrouter",
                NOW - 7_200,
                vec![model("a/x", "openrouter", "A X")],
            ),
            kept(
                "a0_venice",
                NOW - 60,
                vec![model("b/y", "a0_venice", "B Y")],
            ),
        ],
        vec![target("openrouter", "OpenRouter")],
    );
    app.finish_model_fetch(
        "openrouter",
        Ok(vec![model("c/z", "openrouter", "C Z")]),
        NOW,
    );
    assert_eq!(ids(&app), ["b/y@a0_venice", "c/z@openrouter"]);
    assert_eq!(
        app.model_picker
            .as_ref()
            .unwrap()
            .fetched_at
            .get("a0_venice"),
        Some(&(NOW - 60)),
        "an untouched list keeps its age"
    );
}

#[test]
fn a_failed_refresh_keeps_the_kept_rows_and_is_noted_apart() {
    // A list an hour old beats no list: the rows stay, and the failure is
    // recorded as a refresh that didn't happen, not a provider that is down.
    let mut app = App::new();
    app.open_model_picker("a/x");
    app.begin_model_load(
        vec![kept(
            "openrouter",
            NOW - 7_200,
            vec![model("a/x", "openrouter", "A X")],
        )],
        vec![target("openrouter", "OpenRouter")],
    );
    app.finish_model_fetch("openrouter", Err("HTTP 503".into()), NOW);
    let picker = app.model_picker.as_ref().unwrap();
    assert_eq!(picker.status, ModelLoad::Ready);
    assert_eq!(ids(&app), ["a/x@openrouter"]);
    let picker = app.model_picker.as_ref().unwrap();
    assert!(picker.errors.is_empty(), "{:?}", picker.errors);
    assert_eq!(
        picker.stale,
        [ModelFetchError {
            provider: "OpenRouter".into(),
            message: "HTTP 503".into(),
        }]
    );
    assert_eq!(
        picker.fetched_at.get("openrouter"),
        Some(&(NOW - 7_200)),
        "a failed fetch doesn't make the old list new"
    );
}

#[test]
fn a_list_the_picker_is_not_waiting_on_changes_nothing() {
    let mut app = App::new();
    app.open_model_picker("a/x");
    app.begin_model_load(
        vec![kept(
            "openrouter",
            NOW - 60,
            vec![model("a/x", "openrouter", "A X")],
        )],
        Vec::new(),
    );
    let before = app.model_picker.clone();
    app.finish_model_fetch("openrouter", Ok(vec![model("z/z", "openrouter", "Z")]), NOW);
    app.finish_model_fetch("elsewhere", Err("boom".into()), NOW);
    assert_eq!(app.model_picker, before);
}

#[test]
fn a_finished_fetch_is_a_noop_when_the_picker_is_closed() {
    let mut app = App::new();
    app.finish_model_fetch(
        "openrouter",
        Ok(vec![model("a/x", "openrouter", "A X")]),
        NOW,
    );
    assert!(app.model_picker.is_none());
}

#[test]
fn loading_new_tells_a_first_load_from_a_refresh() {
    // `loading more…` while a provider with nothing on screen is fetched;
    // `refreshing…` once every fetch out is behind rows already shown.
    let mut app = App::new();
    app.open_model_picker("x");
    app.begin_model_load(
        vec![kept(
            "openrouter",
            NOW - 7_200,
            vec![model("a/x", "openrouter", "A X")],
        )],
        vec![
            target("openrouter", "OpenRouter"),
            target("a0_venice", "A0"),
        ],
    );
    assert!(app.model_picker.as_ref().unwrap().loading_new());
    app.finish_model_fetch("a0_venice", Ok(vec![model("b/y", "a0_venice", "B Y")]), NOW);
    let picker = app.model_picker.as_ref().unwrap();
    assert_eq!(picker.pending(), 1);
    assert!(!picker.loading_new(), "only a refresh is out now");
}

#[test]
fn ctrl_r_refreshes_the_lists_without_typing_into_the_search() {
    let mut app = model_app(&sample_models());
    type_chars(&mut app, "kim");
    assert_eq!(app.on_key(ctrl('r')), Action::RefreshModels);
    let picker = app.model_picker.as_ref().expect("the picker stays open");
    assert_eq!(picker.query, "kim", "Ctrl+R is not a search character");
}

#[test]
fn ctrl_r_does_nothing_while_a_fetch_is_out() {
    let mut app = App::new();
    app.open_model_picker("x");
    app.begin_model_load(Vec::new(), vec![target("openrouter", "OpenRouter")]);
    assert_eq!(app.on_key(ctrl('r')), Action::None);
}

#[test]
fn a_refresh_keeps_the_rows_and_waits_on_every_provider() {
    let mut app = App::new();
    app.open_model_picker("a/x");
    app.begin_model_load(
        vec![kept(
            "openrouter",
            NOW - 60,
            vec![model("a/x", "openrouter", "A X")],
        )],
        vec![target("a0_venice", "A0")],
    );
    app.finish_model_fetch("a0_venice", Err("HTTP 401".into()), NOW);
    app.begin_model_refresh(vec![
        target("openrouter", "OpenRouter"),
        target("a0_venice", "A0"),
    ]);
    let picker = app.model_picker.as_ref().unwrap();
    assert_eq!(picker.pending(), 2);
    assert_eq!(ids(&app), ["a/x@openrouter"], "the rows stay meanwhile");
    let picker = app.model_picker.as_ref().unwrap();
    assert!(picker.errors.is_empty(), "a retry clears the last failure");
    assert_eq!(picker.highlighted().unwrap().id, "a/x");
}

#[test]
fn a_refresh_drops_the_rows_of_a_provider_no_longer_configured() {
    let mut app = App::new();
    app.open_model_picker("a/x");
    app.begin_model_load(
        vec![
            kept(
                "openrouter",
                NOW - 60,
                vec![model("a/x", "openrouter", "A X")],
            ),
            kept(
                "a0_venice",
                NOW - 60,
                vec![model("b/y", "a0_venice", "B Y")],
            ),
        ],
        Vec::new(),
    );
    app.begin_model_refresh(vec![target("openrouter", "OpenRouter")]);
    assert_eq!(ids(&app), ["a/x@openrouter"]);
    assert!(
        !app.model_picker
            .as_ref()
            .unwrap()
            .fetched_at
            .contains_key("a0_venice")
    );
}

#[test]
fn a_refresh_after_the_login_hint_loads_the_lists() {
    // A provider file fixed while the picker was open: Ctrl+R finds it.
    let mut app = App::new();
    app.open_model_picker("x");
    app.set_models_needs_login();
    app.begin_model_refresh(vec![target("openrouter", "OpenRouter")]);
    assert_eq!(
        app.model_picker.as_ref().unwrap().status,
        ModelLoad::Loading
    );
    app.finish_model_fetch(
        "openrouter",
        Ok(vec![model("a/x", "openrouter", "A X")]),
        NOW,
    );
    assert_eq!(app.model_picker.as_ref().unwrap().status, ModelLoad::Ready);
}

#[test]
fn the_wall_clock_reaches_the_open_picker() {
    let mut app = App::new();
    app.set_model_picker_now(NOW); // closed: a no-op
    app.open_model_picker("x");
    app.set_model_picker_now(NOW);
    assert_eq!(app.model_picker.as_ref().unwrap().now, NOW);
}

#[test]
fn typing_filters_the_models_case_insensitively() {
    let mut app = model_app(&sample_models());
    type_chars(&mut app, "KIMI");
    let picker = app.model_picker.as_ref().unwrap();
    assert_eq!(picker.matches().len(), 1);
    assert_eq!(picker.matches()[0].id, "moonshotai/kimi-k2.6");
}

#[test]
fn filtering_matches_provider_and_display_name_too() {
    let mut app = model_app(&sample_models());
    type_chars(&mut app, "openrouter");
    assert_eq!(app.model_picker.as_ref().unwrap().matches().len(), 3);
    app.model_picker.as_mut().unwrap().query.clear();
    type_chars(&mut app, "MoonshotAI");
    assert_eq!(app.model_picker.as_ref().unwrap().matches().len(), 1);
}

#[test]
fn enter_selects_the_highlighted_model_and_closes() {
    let mut app = model_app(&sample_models());
    app.model_picker.as_mut().unwrap().selected = 2;
    let action = app.on_key(key(KeyCode::Enter));
    assert_eq!(
        action,
        Action::SelectModel {
            provider: "openrouter".into(),
            id: "moonshotai/kimi-k2.6".into(),
            reasoning: None,
            vision: None,
            context: None,
            service_tiers: Vec::new(),
        }
    );
    assert!(app.model_picker.is_none(), "selecting closes the picker");
}

#[test]
fn ctrl_c_closes_the_model_picker_not_the_app() {
    let mut app = model_app(&sample_models());
    assert_eq!(app.on_key(ctrl('c')), Action::CloseModelPicker);
    assert!(app.model_picker.is_none());
}

#[test]
fn backspace_pops_the_model_query() {
    let mut app = model_app(&sample_models());
    type_chars(&mut app, "kim");
    app.on_key(key(KeyCode::Backspace));
    assert_eq!(app.model_picker.as_ref().unwrap().query, "ki");
}

#[test]
fn esc_on_the_provider_step_clears_the_query_then_steps_back() {
    let mut app = provider_app();
    type_chars(&mut app, "open");
    assert_eq!(app.on_key(key(KeyCode::Esc)), Action::None);
    assert!(app.key_onboarding.as_ref().unwrap().query.is_empty());
    assert_eq!(
        app.key_onboarding.as_ref().unwrap().step,
        KeyStep::Provider,
        "first Esc only clears"
    );
    assert_eq!(app.on_key(key(KeyCode::Esc)), Action::None);
    assert_eq!(app.key_onboarding.as_ref().unwrap().step, KeyStep::Method);
    assert_eq!(app.on_key(key(KeyCode::Esc)), Action::CloseKeyOnboarding);
    assert!(app.key_onboarding.is_none());
}

#[test]
fn selecting_a_model_carries_its_speed_tiers() {
    // Enter on a picker row hands the loop the entry's listed tiers, so a
    // successful switch can seed /fast without refetching /models
    // (docs/fast-mode.md).
    let mut quick = model("gpt-5.5", "chatgpt_codex", "GPT-5.5");
    quick.service_tiers = vec![ServiceTier::new("priority", "Fast", "1.5x speed")];
    let mut app = model_app(&[model("plain", "openrouter", "Plain"), quick]);
    type_chars(&mut app, "gpt-5.5");
    let action = app.on_key(key(KeyCode::Enter));
    assert!(
        matches!(
            &action,
            Action::SelectModel { service_tiers, .. } if service_tiers.len() == 1
                && service_tiers[0].id == "priority"
        ),
        "the entry's tiers ride the action: {action:?}"
    );
}
