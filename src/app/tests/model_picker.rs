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

// --- Multi-provider parallel `/model` load (docs/llm.md). ---

#[test]
fn add_models_shows_the_first_provider_and_merges_the_rest() {
    let mut app = App::new();
    app.open_model_picker("x");
    app.begin_model_load(2);
    assert_eq!(
        app.model_picker.as_ref().unwrap().status,
        ModelLoad::Loading
    );
    // OpenRouter lands first — its list shows immediately, one fetch still out.
    app.add_models(vec![
        model("c/z", "openrouter", "C Z"),
        model("a/x", "openrouter", "A X"),
    ]);
    let picker = app.model_picker.as_ref().unwrap();
    assert_eq!(
        picker.status,
        ModelLoad::Ready,
        "shown before the rest arrive"
    );
    assert_eq!(picker.pending, 1);
    assert_eq!(picker.models.len(), 2);
    // Agent Zero lands second — merged into one sorted list, no fetches left.
    app.add_models(vec![model("b/y", "a0_venice", "B Y")]);
    let picker = app.model_picker.as_ref().unwrap();
    assert_eq!(picker.pending, 0);
    let ids: Vec<&str> = picker.models.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(
        ids,
        ["a/x", "b/y", "c/z"],
        "merged + sorted across providers"
    );
}

#[test]
fn add_model_error_keeps_a_partial_list_and_notes_the_failure() {
    let mut app = App::new();
    app.open_model_picker("x");
    app.begin_model_load(2);
    app.add_models(vec![model("a/x", "openrouter", "A X")]);
    app.add_model_error("Agent Zero API", "HTTP 401: unauthorized");
    let picker = app.model_picker.as_ref().unwrap();
    // The good provider's list stays; the failure is recorded, not fatal.
    assert_eq!(picker.status, ModelLoad::Ready);
    assert_eq!(picker.pending, 0);
    assert_eq!(picker.models.len(), 1);
    assert_eq!(picker.errors.len(), 1);
    assert_eq!(picker.errors[0].provider, "Agent Zero API");
}

#[test]
fn all_providers_failing_settles_into_an_error() {
    let mut app = App::new();
    app.open_model_picker("x");
    app.begin_model_load(2);
    app.add_model_error("OpenRouter", "HTTP 500");
    // Still loading — one fetch outstanding, nothing to show yet.
    assert_eq!(
        app.model_picker.as_ref().unwrap().status,
        ModelLoad::Loading
    );
    app.add_model_error("Agent Zero API", "HTTP 401");
    let picker = app.model_picker.as_ref().unwrap();
    assert!(matches!(picker.status, ModelLoad::Error(_)));
    assert!(picker.models.is_empty());
    assert_eq!(picker.errors.len(), 2);
}

#[test]
fn add_models_reseats_on_the_active_model_once_its_provider_lands() {
    let mut app = App::new();
    app.open_model_picker("b/y"); // active lives in the second provider
    app.begin_model_load(2);
    app.add_models(vec![model("a/x", "openrouter", "A X")]);
    // Active isn't here yet — seated on top.
    assert_eq!(app.model_picker.as_ref().unwrap().selected, 0);
    app.add_models(vec![model("b/y", "a0_venice", "B Y")]);
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
    app.begin_model_load(2);
    app.add_models(vec![model("shared/id", "openrouter", "OR")]);
    app.add_models(vec![model("shared/id", "a0_venice", "A0")]);
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
}

#[test]
fn add_models_preserves_the_highlight_after_the_user_navigates() {
    let mut app = App::new();
    app.open_model_picker("x");
    app.begin_model_load(2);
    app.add_models(vec![
        model("a/x", "openrouter", "A X"),
        model("c/z", "openrouter", "C Z"),
    ]);
    app.on_key(key(KeyCode::Down)); // highlight c/z (index 1)
    assert_eq!(
        app.model_picker.as_ref().unwrap().highlighted().unwrap().id,
        "c/z"
    );
    // A merge pushes a row in between — the highlight rides c/z, not the index.
    app.add_models(vec![model("b/y", "a0_venice", "B Y")]);
    assert_eq!(
        app.model_picker.as_ref().unwrap().highlighted().unwrap().id,
        "c/z"
    );
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
fn esc_on_the_provider_step_clears_the_query_then_closes() {
    let mut app = login_app();
    type_chars(&mut app, "open");
    assert_eq!(app.on_key(key(KeyCode::Esc)), Action::None);
    assert!(app.key_onboarding.as_ref().unwrap().query.is_empty());
    assert!(app.key_onboarding.is_some(), "first Esc only clears");
    assert_eq!(app.on_key(key(KeyCode::Esc)), Action::CloseKeyOnboarding);
    assert!(app.key_onboarding.is_none());
}

#[test]
fn selecting_a_model_carries_its_speed_tiers() {
    // Enter on a picker row hands the loop the entry's listed tiers, so a
    // successful switch can seed /fast without refetching /models
    // (docs/fast-mode.md).
    let mut quick = model("gpt-5.5", "openai_chatgpt", "GPT-5.5");
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
