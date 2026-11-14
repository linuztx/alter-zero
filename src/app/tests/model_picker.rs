//! The inline `/model` picker and its multi-provider load (`docs/llm.md`).

use super::*;

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

// --- The per-provider rounds: kept lists, fetches, refreshes
// (docs/llm.md, docs/model-cache.md). ---

/// A configured provider with nothing kept, fetched this round.
fn fetched(id: &str, name: &str) -> ProviderLoad {
    ProviderLoad {
        id: id.into(),
        name: name.into(),
        listed_at: None,
        models: Vec::new(),
        fetch: true,
    }
}

/// A configured provider whose kept `models` were listed at `at`, fetched
/// again this round when `fetch`.
fn kept(id: &str, name: &str, at: i64, models: Vec<ModelEntry>, fetch: bool) -> ProviderLoad {
    ProviderLoad {
        id: id.into(),
        name: name.into(),
        listed_at: Some(at),
        models,
        fetch,
    }
}

fn picker(app: &App) -> &ModelPicker {
    app.model_picker.as_ref().expect("the picker is open")
}

fn ids(app: &App) -> Vec<String> {
    picker(app).models.iter().map(|m| m.id.clone()).collect()
}

#[test]
fn a_round_with_nothing_kept_loads() {
    let mut app = App::new();
    app.open_model_picker("x");
    let round = app.begin_model_round(vec![fetched("openrouter", "OpenRouter")]);
    assert!(round.is_some(), "an open picker hands out a round");
    assert_eq!(picker(&app).status, ModelLoad::Loading);
    assert!(
        picker(&app).loading(),
        "a provider with nothing shown is loading"
    );
    assert!(!picker(&app).refreshing());
}

#[test]
fn a_round_shows_the_kept_rows_at_once() {
    // The cache's whole point: the picker opens on the rows it kept, sorted
    // and merged across providers, the highlight on the model in use — no
    // `Loading models…` while a provider is asked again behind them.
    let mut app = App::new();
    app.open_model_picker("b/y");
    app.begin_model_round(vec![
        kept(
            "openrouter",
            "OpenRouter",
            1_000,
            vec![
                model("c/z", "openrouter", "C Z"),
                model("a/x", "openrouter", "A X"),
            ],
            true,
        ),
        kept(
            "a0_venice",
            "Agent Zero API",
            2_000,
            vec![model("b/y", "a0_venice", "B Y")],
            false,
        ),
    ]);
    let shown = picker(&app);
    assert_eq!(shown.status, ModelLoad::Ready);
    assert_eq!(ids(&app), ["a/x", "b/y", "c/z"]);
    assert_eq!(
        shown.highlighted().unwrap().id,
        "b/y",
        "seated on the model in use"
    );
    assert!(
        shown.refreshing(),
        "a kept list fetched again is refreshing"
    );
    assert!(!shown.loading(), "nothing is missing from the page");
    assert_eq!(
        shown.listed_at(),
        Some(1_000),
        "the oldest list dates the page"
    );
}

#[test]
fn a_round_with_every_list_fresh_fetches_nothing() {
    let mut app = App::new();
    app.open_model_picker("a/x");
    app.begin_model_round(vec![kept(
        "openrouter",
        "OpenRouter",
        1_000,
        vec![model("a/x", "openrouter", "A X")],
        false,
    )]);
    assert!(!picker(&app).fetching(), "nothing is out");
    assert_eq!(picker(&app).status, ModelLoad::Ready);
}

#[test]
fn a_round_is_refused_when_the_picker_is_closed() {
    let mut app = App::new();
    assert_eq!(app.begin_model_round(vec![fetched("p", "P")]), None);
}

#[test]
fn the_first_provider_shows_and_the_rest_merge_as_they_land() {
    let mut app = App::new();
    app.open_model_picker("x");
    let round = app
        .begin_model_round(vec![
            fetched("openrouter", "OpenRouter"),
            fetched("a0_venice", "Agent Zero API"),
        ])
        .unwrap();
    app.finish_model_fetch(
        round,
        "openrouter",
        Ok(vec![
            model("c/z", "openrouter", "C Z"),
            model("a/x", "openrouter", "A X"),
        ]),
        5_000,
    );
    assert_eq!(
        picker(&app).status,
        ModelLoad::Ready,
        "shown before the rest arrive"
    );
    assert!(picker(&app).loading(), "one provider is still on its way");
    app.finish_model_fetch(
        round,
        "a0_venice",
        Ok(vec![model("b/y", "a0_venice", "B Y")]),
        5_001,
    );
    assert!(!picker(&app).fetching());
    assert_eq!(
        ids(&app),
        ["a/x", "b/y", "c/z"],
        "merged + sorted across providers"
    );
    assert_eq!(picker(&app).listed_at(), Some(5_000));
}

#[test]
fn exact_duplicates_are_dropped_from_a_providers_rows() {
    let mut app = App::new();
    app.open_model_picker("x");
    let round = app
        .begin_model_round(vec![fetched("openrouter", "OpenRouter")])
        .unwrap();
    app.finish_model_fetch(
        round,
        "openrouter",
        Ok(vec![
            model("a/x", "openrouter", "A X"),
            model("a/x", "openrouter", "A X"),
        ]),
        5_000,
    );
    assert_eq!(ids(&app), ["a/x"]);
}

#[test]
fn a_fetch_replaces_its_providers_kept_rows() {
    // A refreshed listing is the provider's list now: a model it no longer
    // lists goes, a new one comes, and the other providers' rows stay.
    let mut app = App::new();
    app.open_model_picker("x");
    let round = app
        .begin_model_round(vec![
            kept(
                "openrouter",
                "OpenRouter",
                1_000,
                vec![
                    model("a/old", "openrouter", "Old"),
                    model("a/kept", "openrouter", "Kept"),
                ],
                true,
            ),
            kept(
                "a0_venice",
                "Agent Zero API",
                1_000,
                vec![model("b/y", "a0_venice", "B Y")],
                false,
            ),
        ])
        .unwrap();
    app.finish_model_fetch(
        round,
        "openrouter",
        Ok(vec![
            model("a/kept", "openrouter", "Kept"),
            model("a/new", "openrouter", "New"),
        ]),
        9_000,
    );
    assert_eq!(ids(&app), ["a/kept", "a/new", "b/y"]);
    let list = picker(&app)
        .lists
        .iter()
        .find(|l| l.id == "openrouter")
        .unwrap();
    assert_eq!(list.listed_at, Some(9_000), "dated by the fetch now");
    assert!(!list.fetching);
    assert!(!picker(&app).refreshing());
}

#[test]
fn a_failed_refresh_keeps_the_rows_it_could_not_refresh() {
    let mut app = App::new();
    app.open_model_picker("x");
    let round = app
        .begin_model_round(vec![kept(
            "openrouter",
            "OpenRouter",
            1_000,
            vec![model("a/x", "openrouter", "A X")],
            true,
        )])
        .unwrap();
    app.finish_model_fetch(round, "openrouter", Err("HTTP 503".into()), 9_000);
    let shown = picker(&app);
    assert_eq!(ids(&app), ["a/x"], "the kept rows stay usable");
    assert_eq!(shown.status, ModelLoad::Ready);
    let list = &shown.lists[0];
    assert_eq!(list.error.as_deref(), Some("HTTP 503"));
    assert_eq!(list.listed_at, Some(1_000), "still dated by the cache");
}

#[test]
fn a_failure_beside_a_partial_list_is_noted_not_fatal() {
    let mut app = App::new();
    app.open_model_picker("x");
    let round = app
        .begin_model_round(vec![
            fetched("openrouter", "OpenRouter"),
            fetched("a0_venice", "Agent Zero API"),
        ])
        .unwrap();
    app.finish_model_fetch(
        round,
        "openrouter",
        Ok(vec![model("a/x", "openrouter", "A X")]),
        5_000,
    );
    app.finish_model_fetch(
        round,
        "a0_venice",
        Err("HTTP 401: unauthorized".into()),
        5_000,
    );
    let shown = picker(&app);
    assert_eq!(shown.status, ModelLoad::Ready);
    assert_eq!(ids(&app), ["a/x"]);
    let failed = shown.lists.iter().find(|l| l.id == "a0_venice").unwrap();
    assert_eq!(failed.error.as_deref(), Some("HTTP 401: unauthorized"));
    assert_eq!(failed.listed_at, None, "nothing of it was ever shown");
}

#[test]
fn every_provider_failing_settles_into_an_error() {
    let mut app = App::new();
    app.open_model_picker("x");
    let round = app
        .begin_model_round(vec![
            fetched("openrouter", "OpenRouter"),
            fetched("a0_venice", "Agent Zero API"),
        ])
        .unwrap();
    app.finish_model_fetch(round, "openrouter", Err("HTTP 500".into()), 5_000);
    assert_eq!(
        picker(&app).status,
        ModelLoad::Loading,
        "one fetch is still out"
    );
    app.finish_model_fetch(round, "a0_venice", Err("HTTP 401".into()), 5_000);
    let shown = picker(&app);
    assert!(matches!(shown.status, ModelLoad::Error(_)));
    assert!(shown.models.is_empty());
    assert_eq!(shown.lists.iter().filter(|l| l.error.is_some()).count(), 2);
}

#[test]
fn a_fetch_of_an_earlier_round_is_dropped() {
    // A refresh begun over a round still landing: the old round's answers
    // are not this round's, so they change nothing — not the rows, and not
    // whether the page still says it is refreshing.
    let mut app = App::new();
    app.open_model_picker("x");
    let first = app
        .begin_model_round(vec![fetched("openrouter", "OpenRouter")])
        .unwrap();
    let second = app
        .begin_model_round(vec![fetched("openrouter", "OpenRouter")])
        .unwrap();
    assert_ne!(first, second);
    app.finish_model_fetch(
        first,
        "openrouter",
        Ok(vec![model("a/old", "openrouter", "Old")]),
        5_000,
    );
    assert!(
        picker(&app).models.is_empty(),
        "an earlier round's rows were merged"
    );
    assert!(
        picker(&app).loading(),
        "an earlier round's answer settled this one"
    );
    app.finish_model_fetch(
        second,
        "openrouter",
        Ok(vec![model("a/new", "openrouter", "New")]),
        5_001,
    );
    assert_eq!(ids(&app), ["a/new"]);
}

#[test]
fn a_fetch_for_a_closed_and_reopened_picker_is_dropped() {
    // The round counter outlives the picker, so a late answer to the last
    // open can never pass for the new one's.
    let mut app = App::new();
    app.open_model_picker("x");
    let old = app
        .begin_model_round(vec![fetched("openrouter", "OpenRouter")])
        .unwrap();
    app.close_model_picker();
    app.open_model_picker("x");
    let new = app
        .begin_model_round(vec![fetched("openrouter", "OpenRouter")])
        .unwrap();
    app.finish_model_fetch(
        old,
        "openrouter",
        Ok(vec![model("a/old", "openrouter", "Old")]),
        5_000,
    );
    assert!(picker(&app).models.is_empty());
    assert!(picker(&app).loading());
    app.finish_model_fetch(
        new,
        "openrouter",
        Ok(vec![model("a/new", "openrouter", "New")]),
        5_000,
    );
    assert_eq!(ids(&app), ["a/new"]);
}

#[test]
fn a_fetch_after_the_picker_closed_changes_nothing() {
    let mut app = App::new();
    app.open_model_picker("x");
    let round = app
        .begin_model_round(vec![fetched("openrouter", "OpenRouter")])
        .unwrap();
    app.close_model_picker();
    app.finish_model_fetch(
        round,
        "openrouter",
        Ok(vec![model("a/x", "openrouter", "A X")]),
        5_000,
    );
    assert!(app.model_picker.is_none());
}

#[test]
fn a_refresh_keeps_the_rows_on_screen_and_drops_a_provider_gone_since() {
    // Ctrl+R lays a new round over the open page: a provider still
    // configured keeps the rows it shows through the round (whatever the
    // boundary kept for it), one no longer configured leaves with its rows.
    let mut app = App::new();
    app.open_model_picker("x");
    let round = app
        .begin_model_round(vec![
            fetched("openrouter", "OpenRouter"),
            fetched("a0_venice", "Agent Zero API"),
        ])
        .unwrap();
    app.finish_model_fetch(
        round,
        "openrouter",
        Ok(vec![model("a/x", "openrouter", "A X")]),
        5_000,
    );
    app.finish_model_fetch(
        round,
        "a0_venice",
        Ok(vec![model("b/y", "a0_venice", "B Y")]),
        5_000,
    );
    app.begin_model_round(vec![fetched("openrouter", "OpenRouter")]);
    let shown = picker(&app);
    assert_eq!(ids(&app), ["a/x"], "the gone provider's rows left with it");
    assert_eq!(shown.lists.len(), 1);
    assert_eq!(
        shown.lists[0].listed_at,
        Some(5_000),
        "the rows on screen keep their date"
    );
    assert!(shown.refreshing(), "fetched again over the rows it shows");
    assert_eq!(shown.status, ModelLoad::Ready);
}

#[test]
fn a_refresh_clears_the_last_rounds_failures() {
    let mut app = App::new();
    app.open_model_picker("x");
    let round = app
        .begin_model_round(vec![fetched("openrouter", "OpenRouter")])
        .unwrap();
    app.finish_model_fetch(round, "openrouter", Err("HTTP 500".into()), 5_000);
    assert!(matches!(picker(&app).status, ModelLoad::Error(_)));
    app.begin_model_round(vec![fetched("openrouter", "OpenRouter")]);
    assert_eq!(picker(&app).lists[0].error, None, "a retry starts clean");
    assert_eq!(picker(&app).status, ModelLoad::Loading);
}

#[test]
fn an_untouched_picker_reseats_on_the_active_model_once_its_provider_lands() {
    let mut app = App::new();
    app.open_model_picker("b/y"); // active lives in the second provider
    let round = app
        .begin_model_round(vec![
            fetched("openrouter", "OpenRouter"),
            fetched("a0_venice", "Agent Zero API"),
        ])
        .unwrap();
    app.finish_model_fetch(
        round,
        "openrouter",
        Ok(vec![model("a/x", "openrouter", "A X")]),
        5_000,
    );
    assert_eq!(picker(&app).selected, 0, "the active model isn't here yet");
    app.finish_model_fetch(
        round,
        "a0_venice",
        Ok(vec![model("b/y", "a0_venice", "B Y")]),
        5_000,
    );
    assert_eq!(picker(&app).highlighted().unwrap().id, "b/y");
}

#[test]
fn a_merge_keeps_the_highlight_after_the_user_navigates() {
    let mut app = App::new();
    app.open_model_picker("x");
    let round = app
        .begin_model_round(vec![
            fetched("openrouter", "OpenRouter"),
            fetched("a0_venice", "Agent Zero API"),
        ])
        .unwrap();
    app.finish_model_fetch(
        round,
        "openrouter",
        Ok(vec![
            model("a/x", "openrouter", "A X"),
            model("c/z", "openrouter", "C Z"),
        ]),
        5_000,
    );
    app.on_key(key(KeyCode::Down)); // highlight c/z (index 1)
    assert_eq!(picker(&app).highlighted().unwrap().id, "c/z");
    // A merge pushes a row in between — the highlight rides c/z, not the index.
    app.finish_model_fetch(
        round,
        "a0_venice",
        Ok(vec![model("b/y", "a0_venice", "B Y")]),
        5_000,
    );
    assert_eq!(picker(&app).highlighted().unwrap().id, "c/z");
}

#[test]
fn a_refresh_landing_never_undoes_a_move_to_the_top() {
    // Kept rows open the picker seated on the model in use, so a refresh
    // can land while the user is already moving: Home to the first row is a
    // choice, not an untouched picker, and the highlight stays there.
    let mut app = App::new();
    app.open_model_picker("c/z");
    let round = app
        .begin_model_round(vec![kept(
            "openrouter",
            "OpenRouter",
            1_000,
            vec![
                model("a/x", "openrouter", "A X"),
                model("c/z", "openrouter", "C Z"),
            ],
            true,
        )])
        .unwrap();
    assert_eq!(picker(&app).highlighted().unwrap().id, "c/z");
    app.on_key(key(KeyCode::Home));
    assert_eq!(picker(&app).highlighted().unwrap().id, "a/x");
    app.finish_model_fetch(
        round,
        "openrouter",
        Ok(vec![
            model("a/x", "openrouter", "A X"),
            model("c/z", "openrouter", "C Z"),
        ]),
        5_000,
    );
    assert_eq!(
        picker(&app).highlighted().unwrap().id,
        "a/x",
        "the refresh moved the highlight back to the model in use"
    );
}

#[test]
fn active_mark_matches_the_provider_not_just_the_id() {
    // The merged list can carry the same id under two providers; only the
    // row from the active provider is the ✓ row.
    let mut app = App::new();
    app.open_model_picker("shared/id");
    app.set_active_provider("a0_venice");
    app.begin_model_round(vec![
        kept(
            "openrouter",
            "OpenRouter",
            1_000,
            vec![model("shared/id", "openrouter", "OR")],
            false,
        ),
        kept(
            "a0_venice",
            "Agent Zero API",
            1_000,
            vec![model("shared/id", "a0_venice", "A0")],
            false,
        ),
    ]);
    let shown = picker(&app);
    let or = shown
        .models
        .iter()
        .find(|m| m.provider == "openrouter")
        .unwrap();
    let a0 = shown
        .models
        .iter()
        .find(|m| m.provider == "a0_venice")
        .unwrap();
    assert!(
        !shown.is_active(or),
        "same id, other provider is not active"
    );
    assert!(shown.is_active(a0), "the active provider's row is marked");
    assert_eq!(
        shown.highlighted().unwrap().provider,
        "a0_venice",
        "seated on the exact row in use"
    );
}

#[test]
fn the_clock_reaches_the_open_picker() {
    let mut app = App::new();
    app.set_model_picker_now(42); // closed: nothing to set, no panic
    app.open_model_picker("x");
    app.set_model_picker_now(1_763_100_000);
    assert_eq!(picker(&app).now, 1_763_100_000);
}

#[test]
fn ctrl_r_asks_for_every_list_again() {
    let mut app = App::new();
    app.open_model_picker("a/x");
    app.begin_model_round(vec![kept(
        "openrouter",
        "OpenRouter",
        1_000,
        vec![model("a/x", "openrouter", "A X")],
        false,
    )]);
    assert_eq!(app.on_key(ctrl('r')), Action::RefreshModels);
    assert_eq!(
        picker(&app).query,
        "",
        "Ctrl+R types nothing into the search"
    );
}

#[test]
fn ctrl_r_while_a_fetch_is_out_asks_for_nothing_more() {
    // The /usage page's `r` rule: a round in flight already answers the
    // question, and a second one would only cancel it and ask again.
    let mut app = App::new();
    app.open_model_picker("a/x");
    app.begin_model_round(vec![fetched("openrouter", "OpenRouter")]);
    assert_eq!(app.on_key(ctrl('r')), Action::None);
}

#[test]
fn ctrl_r_retries_when_every_provider_failed() {
    let mut app = App::new();
    app.open_model_picker("x");
    let round = app
        .begin_model_round(vec![fetched("openrouter", "OpenRouter")])
        .unwrap();
    app.finish_model_fetch(round, "openrouter", Err("HTTP 500".into()), 5_000);
    assert_eq!(app.on_key(ctrl('r')), Action::RefreshModels);
}

#[test]
fn a_plain_r_still_types_into_the_search() {
    // The picker is type-to-search: `deepseek-r1` and `openrouter` both need
    // an `r`, which is why the refresh is Ctrl+R.
    let mut app = model_app(&sample_models());
    assert_eq!(app.on_key(key(KeyCode::Char('r'))), Action::None);
    assert_eq!(picker(&app).query, "r");
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
