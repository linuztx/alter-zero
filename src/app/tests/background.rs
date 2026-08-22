//! Background shells and the ↓ manager band (`docs/background.md`).

use super::*;

// --- `!` shell commands (docs/shell-command.md — codex's bang shell mode,
// with the absorbed `!` prefix: the bang becomes the prompt, not text) ---

#[test]
fn shell_query_strips_a_leading_bang_keeping_the_rest_verbatim() {
    assert_eq!(shell_query("!ls -la"), Some("ls -la"));
    assert_eq!(shell_query("!"), Some(""), "a lone ! is empty shell mode");
    assert_eq!(shell_query("! echo hi"), Some(" echo hi"), "spaces kept");
    assert_eq!(shell_query("!/usr/bin/env"), Some("/usr/bin/env"));
}

#[test]
fn shell_query_is_none_without_a_leading_bang() {
    assert_eq!(shell_query(""), None);
    assert_eq!(shell_query("ls"), None);
    assert_eq!(shell_query("ask !ls"), None, "the ! must lead");
    assert_eq!(shell_query("/help"), None);
}

#[test]
fn esc_on_an_empty_shell_composer_exits_the_mode_instead_of_quitting() {
    let mut app = App::new();
    type_query(&mut app, "!");
    assert_eq!(app.on_key(key(KeyCode::Esc)), Action::None);
    assert!(!app.shell_mode, "Esc exits shell mode (codex)");
    // A second Esc, now out of the mode, quits as usual.
    assert_eq!(app.on_key(key(KeyCode::Esc)), Action::Quit);
}

#[test]
fn question_mark_types_into_shell_mode_not_the_band() {
    let mut app = App::new();
    type_query(&mut app, "!");
    type_query(&mut app, "?");
    assert!(!app.shortcuts_open, "`?` is a shell character here");
    assert_eq!(app.input.text(), "?");
}

#[test]
fn the_shell_command_is_trimmed_before_running() {
    let mut app = App::new();
    type_query(&mut app, "!  ls -la  ");
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::RunShell("ls -la".to_string())
    );
}

#[test]
fn searching_from_shell_mode_previews_plainly_and_esc_restores_the_mode() {
    let mut app = App::new();
    submit(&mut app, "git status");
    type_query(&mut app, "!pw");
    app.on_key(ctrl('r'));
    assert!(!app.shell_mode, "previews show raw text, outside the mode");
    type_query(&mut app, "git");
    assert_eq!(app.input.text(), "git status");
    app.on_key(key(KeyCode::Esc));
    assert!(app.shell_mode, "cancel restores the shell-mode draft");
    assert_eq!(app.input.text(), "pw");
}

#[test]
fn accepting_a_bang_match_restores_shell_mode() {
    let mut app = App::new();
    type_query(&mut app, "!echo hello");
    app.on_key(key(KeyCode::Enter));
    app.on_key(ctrl('r'));
    type_query(&mut app, "echo");
    app.on_key(key(KeyCode::Enter)); // accept "!echo hello"
    assert!(app.shell_mode, "the accepted !entry re-enters the mode");
    assert_eq!(app.input.text(), "echo hello");
}

#[test]
fn bg_started_lists_the_shell_for_the_footer_and_the_manager() {
    let mut app = App::new();
    assert!(app.background().is_empty());
    app.bg_started("bash_1", "ping x.com", Some("Ping x".into()), true, None);
    assert_eq!(app.background().len(), 1);
    let shell = &app.background()[0];
    assert_eq!(shell.id, "bash_1");
    assert_eq!(shell.command, "ping x.com");
    assert_eq!(shell.description.as_deref(), Some("Ping x"));
    assert!(shell.from_model);
}

#[test]
fn bg_output_appends_and_caps_the_tail_on_line_boundaries() {
    let mut app = app_with_shells(&["cmd"]);
    app.bg_output("bash_1", "hello\n");
    app.bg_output("bash_1", "world\n");
    assert_eq!(app.background()[0].output, "hello\nworld\n");
    // Overflow the cap: the retained tail starts on a line boundary.
    let long = "x".repeat(1024);
    for _ in 0..32 {
        app.bg_output("bash_1", &format!("{long}\n"));
    }
    let tail = &app.background()[0].output;
    assert!(tail.len() <= 16 * 1024, "tail stays capped: {}", tail.len());
    assert!(
        !tail.starts_with('x') || tail.split('\n').next().unwrap().len() == 1024,
        "the tail opens on a whole line"
    );
    // Output for an unknown id is dropped, not panicking.
    app.bg_output("nope", "zzz");
}

#[test]
fn bg_exited_removes_the_shell_and_returns_its_completion() {
    let mut app = App::new();
    app.bg_started("bash_1", "ping x.com", Some("Ping x".into()), true, None);
    app.bg_output("bash_1", "64 bytes\n");
    let completion = app.bg_exited("bash_1", Some(0), false).expect("completes");
    assert!(
        app.background().is_empty(),
        "the exited shell leaves the list"
    );
    assert_eq!(completion.id, "bash_1");
    assert_eq!(completion.code, Some(0));
    assert!(!completion.killed);
    assert!(completion.from_model);
    assert_eq!(completion.output_tail, "64 bytes");
    assert_eq!(completion.display_description(), "Ping x");
    // An unknown id (already swept by /clear) owes nothing.
    assert!(app.bg_exited("bash_1", Some(0), false).is_none());
}

#[test]
fn completion_notice_headline_covers_every_outcome() {
    let mut notice = BackgroundNotice {
        description: "Ping x".into(),
        id: "bash_1".into(),
        code: Some(0),
        killed: false,
        output_tail: String::new(),
        origin: None,
        timestamp: String::new(),
    };
    assert!(notice.ok());
    assert_eq!(
        notice.headline(),
        "Background command \"Ping x\" completed (exit code 0)"
    );
    notice.code = Some(2);
    assert!(!notice.ok());
    assert_eq!(
        notice.headline(),
        "Background command \"Ping x\" failed (exit code 2)"
    );
    notice.killed = true;
    assert_eq!(
        notice.headline(),
        "Background command \"Ping x\" was stopped by the user"
    );
    notice.killed = false;
    notice.code = None;
    assert_eq!(
        notice.headline(),
        "Background command \"Ping x\" was terminated by a signal"
    );
}

#[test]
fn completion_notice_context_text_carries_the_tail() {
    let notice = BackgroundNotice {
        description: "Ping x".into(),
        id: "bash_1".into(),
        code: Some(0),
        killed: false,
        output_tail: "line1\nline2".into(),
        origin: None,
        timestamp: String::new(),
    };
    assert_eq!(
        notice.context_text(),
        "[background] Background command \"Ping x\" completed (exit code 0).\n\
         Final output (tail):\nline1\nline2"
    );
    let silent = BackgroundNotice {
        output_tail: String::new(),
        ..notice
    };
    assert!(silent.context_text().ends_with("(no output)"));
}

#[test]
fn a_subagent_launched_shell_is_attributed_end_to_end() {
    // The origin flows launch → shell list → completion → notice: the
    // details page knows whose shell it is, the headline reads
    // `· from the {type} agent`, and the context note says who launched it
    // (docs/agent-tool.md).
    let mut app = App::new();
    let origin = crate::background::BgOrigin {
        agent_id: "a7k2m9x4q".into(),
        agent_type: "general-purpose".into(),
    };
    app.bg_started(
        "b1",
        "for i in $(seq 1 100); do echo $i; sleep 1; done",
        Some("Count 1-100".into()),
        true,
        Some(origin.clone()),
    );
    assert_eq!(app.background()[0].origin.as_ref(), Some(&origin));
    let completion = app.bg_exited("b1", Some(0), false).expect("completes");
    assert_eq!(
        completion.origin.as_ref(),
        Some(&origin),
        "the completion carries the launcher for the boundary's routing"
    );
    let notice = app.record_background_notice(&completion);
    assert_eq!(notice.origin.as_deref(), Some("general-purpose"));
    assert_eq!(
        notice.headline(),
        "Background command \"Count 1-100\" completed (exit code 0) \
         · from the general-purpose agent"
    );
    assert!(
        notice
            .context_text()
            .contains("(launched by the general-purpose agent)"),
        "{}",
        notice.context_text()
    );
    assert_eq!(
        completion.context_text(),
        notice.context_text(),
        "the board note and the recorded notice never diverge"
    );
}

#[test]
fn completion_context_text_matches_the_recorded_notices() {
    // The Exited arm posts the completion's context note to the registry
    // board the moment it lands, so the in-flight agent can inject it into
    // its next round (docs/background.md); the settle later records a
    // BackgroundNotice for the same completion. Both must read
    // identically, or the model would see one text mid-turn and a
    // different one in every later turn's derived context.
    let mut app = App::new();
    app.bg_started(
        "bash_1",
        "python3 server.py",
        Some("Start the API".into()),
        true,
        None,
    );
    app.bg_output("bash_1", "listening on 8888\n");
    let completion = app.bg_exited("bash_1", None, false).unwrap();
    let notice = app.record_background_notice(&completion);
    assert_eq!(completion.context_text(), notice.context_text());
    assert!(
        completion
            .context_text()
            .contains("was terminated by a signal"),
        "a signal death reads as terminated: {}",
        completion.context_text()
    );
}

#[test]
fn record_background_notice_lands_in_history_stamped() {
    let mut app = App::new();
    app.set_clock(|| "01:02 PM".to_string());
    app.bg_started("bash_1", "ping x.com", None, true, None);
    let completion = app.bg_exited("bash_1", Some(0), false).unwrap();
    let notice = app.record_background_notice(&completion);
    assert_eq!(
        notice.description, "ping x.com",
        "falls back to the command"
    );
    assert_eq!(notice.timestamp, "01:02 PM");
    assert_eq!(
        app.history.last(),
        Some(&HistoryItem::Background(notice)),
        "the notice is a history item — it repaints and rides the context"
    );
}

#[test]
fn completions_defer_and_drain_in_arrival_order() {
    let mut app = app_with_shells(&["a", "b"]);
    let first = app.bg_exited("bash_1", Some(0), false).unwrap();
    let second = app.bg_exited("bash_2", Some(1), false).unwrap();
    app.defer_bg_completion(first.clone());
    app.defer_bg_completion(second.clone());
    assert_eq!(app.take_pending_bg_completions(), vec![first, second]);
    assert!(app.take_pending_bg_completions().is_empty(), "drained once");
}

#[test]
fn down_reaches_the_manager_only_while_a_shell_is_running() {
    let mut app = App::new();
    // Before any shell: ↓ keeps its old meaning (a cursor no-op here).
    app.on_key(key(KeyCode::Down));
    assert!(!app.background_focused());
    assert!(app.background_view.is_none());
    app.bg_started("bash_1", "ping x.com", None, true, None);
    // ↓ highlights the footer's indicator first; Enter opens the band.
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Enter));
    assert_eq!(
        app.background_view,
        Some(BackgroundView::List { selected: 0 })
    );
    // Once every shell has finished the footer carries no count, so ↓ has
    // nothing to light up — and no hidden way into the manager either.
    app.close_background_view();
    app.bg_exited("bash_1", Some(0), false);
    app.on_key(key(KeyCode::Down));
    assert!(!app.background_focused());
    assert!(
        app.background_view.is_none(),
        "no indicator on screen, no keybinding"
    );
}

#[test]
fn down_highlights_the_footer_shell_indicator_before_opening_the_manager() {
    let mut app = app_with_shells(&["a"]);
    app.on_key(key(KeyCode::Down));
    assert!(
        app.background_focused(),
        "↓ lights up the footer's shell count first"
    );
    assert!(
        app.background_view.is_none(),
        "…and opens no band until Enter"
    );
    // A second ↓ keeps the highlight — there is only the one indicator.
    app.on_key(key(KeyCode::Down));
    assert!(app.background_focused());
    assert!(app.background_view.is_none());
    // Enter opens the manager band and drops the highlight.
    app.on_key(key(KeyCode::Enter));
    assert_eq!(
        app.background_view,
        Some(BackgroundView::List { selected: 0 })
    );
    assert!(!app.background_focused(), "the band replaces the highlight");
}

#[test]
fn the_overlay_globals_clear_the_shell_highlight_on_their_way_past() {
    // Ctrl+O and Ctrl+D are dispatched *above* the composer, so the
    // highlight has to clear as they pass — otherwise the transcript /
    // context overlay is returned from onto a stale lit indicator (the
    // footer isn't even painted under the overlay) whose Enter would then
    // open the band. This pins the routing: moving the focus handler below
    // those globals breaks it.
    for global in [ctrl('o'), ctrl('d')] {
        let mut app = app_with_shells(&["a"]);
        app.on_key(key(KeyCode::Down));
        assert!(app.background_focused(), "precondition: lit");
        app.on_key(global);
        assert!(
            !app.background_focused(),
            "{global:?} returned from the overlay onto a stale highlight"
        );
        assert_ne!(app.view, View::Conversation, "…and it still opened");
    }
}

#[test]
fn any_other_key_clears_the_shell_highlight_and_still_acts() {
    let mut app = app_with_shells(&["a"]);
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Char('h')));
    assert!(!app.background_focused(), "typing dismisses the highlight");
    assert_eq!(app.input.text(), "h", "…and the key still types");
}

#[test]
fn the_last_shell_exiting_clears_the_shell_highlight() {
    let mut app = app_with_shells(&["a", "b"]);
    app.on_key(key(KeyCode::Down));
    app.bg_exited("bash_1", Some(0), false);
    assert!(
        app.background_focused(),
        "one shell still runs — the indicator stays"
    );
    app.bg_exited("bash_2", Some(0), false);
    assert!(
        !app.background_focused(),
        "the indicator went with the last shell"
    );
}

#[test]
fn down_with_a_draft_or_in_shell_mode_never_opens_the_manager() {
    let mut app = App::new();
    app.bg_started("bash_1", "ping x.com", None, true, None);
    app.input = TextArea::from_text("draft");
    app.on_key(key(KeyCode::Down));
    assert!(
        app.background_view.is_none() && !app.background_focused(),
        "a draft keeps ↓ for the cursor"
    );
    app.input.clear();
    app.on_key(key(KeyCode::Char('!')));
    app.on_key(key(KeyCode::Down));
    assert!(
        app.background_view.is_none() && !app.background_focused(),
        "shell mode keeps ↓ too"
    );
}

#[test]
fn a_details_view_falls_back_to_the_list_when_its_shell_exits() {
    let mut app = app_with_shells(&["a", "b"]);
    app.open_background_view();
    app.on_key(key(KeyCode::Enter)); // details of bash_1
    app.bg_exited("bash_1", Some(0), false);
    assert_eq!(
        app.background_view,
        Some(BackgroundView::List { selected: 0 }),
        "the watched shell exited — back to the (re-clamped) list"
    );
    // …and the last shell exiting closes the band: there is no empty list to
    // fall back to.
    app.bg_exited("bash_2", Some(0), false);
    assert!(app.background_view.is_none());
}

#[test]
fn ctrl_b_moves_only_a_running_command_to_the_background() {
    let mut app = App::new();
    assert_eq!(app.on_key(ctrl('b')), Action::None, "idle: nothing to move");
    // A running model bash call can move.
    app.begin_stream();
    app.start_tool("Bash", "ping x.com");
    assert!(app.can_move_to_background());
    assert_eq!(app.on_key(ctrl('b')), Action::MoveToBackground);
    app.end_tool("done", true);
    // A running non-command tool can't.
    app.start_tool("Read", "src/main.rs");
    assert!(!app.can_move_to_background());
    assert_eq!(app.on_key(ctrl('b')), Action::None);
}

#[test]
fn ctrl_b_moves_a_running_shell_turn_too() {
    let mut app = App::new();
    app.begin_shell("ping x.com");
    assert!(app.can_move_to_background());
    assert_eq!(app.on_key(ctrl('b')), Action::MoveToBackground);
}

#[test]
fn the_open_manager_band_suppresses_the_ctrl_b_hint_clock() {
    // The band owns every key while open, so Ctrl+B does nothing there — the
    // delayed `(ctrl+b to run in background)` hint this clock gates must not
    // advertise it (the permission-prompt rule; docs/background.md). The
    // running cell itself stays visible above the band, hintless.
    let mut app = App::new();
    app.begin_stream();
    app.start_tool("Bash", "sleep 100");
    app.set_command_elapsed(Some(Duration::from_secs(5)));
    assert!(app.command_elapsed().is_some());
    app.bg_started("bash_1", "sleep 200", None, true, None);
    app.open_background_view();
    assert_eq!(app.command_elapsed(), None, "the band swallows Ctrl+B");
    app.close_background_view();
    assert!(
        app.command_elapsed().is_some(),
        "the hint clock returns when the band closes"
    );
}

#[test]
fn an_open_inline_picker_suppresses_the_ctrl_b_hint_clock() {
    // The `/model`, `/login`, `/settings`, `/hooks` and `/skills` pickers own
    // every key while open too (`on_key`'s dispatch), so the running cell they
    // now keep visible above themselves must not advertise a Ctrl+B they would
    // swallow — the band's rule (docs/background.md). Every composer-replacing
    // picker belongs on this list; the two newest were the ones that grew it.
    let hint_clock_off = |open: fn(&mut App)| {
        let mut app = App::new();
        app.begin_stream();
        app.start_tool("Bash", "sleep 100");
        app.set_command_elapsed(Some(Duration::from_secs(5)));
        assert!(app.command_elapsed().is_some());
        open(&mut app);
        assert_eq!(app.command_elapsed(), None, "the picker swallows Ctrl+B");
    };
    hint_clock_off(|app| app.open_model_picker("a"));
    hint_clock_off(|app| app.open_key_onboarding(Vec::new(), "~/.alter-zero/.env"));
    hint_clock_off(App::open_settings);
    hint_clock_off(|app| {
        app.open_hooks_menu(crate::hooks::HooksOverview::default(), None, true);
    });
    hint_clock_off(|app| {
        app.open_skills_menu(Vec::new(), Default::default(), true, Vec::new());
    });
}

#[test]
fn clear_conversation_wipes_the_background_state() {
    let mut app = app_with_shells(&["a"]);
    let completion = app.bg_exited("bash_1", Some(0), false).unwrap();
    app.bg_started("bash_2", "b", None, false, None);
    app.defer_bg_completion(completion);
    // Run /clear the real way: type it (the palette opens) and Enter.
    for c in "/clear".chars() {
        app.on_key(key(KeyCode::Char(c)));
    }
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::Clear);
    assert!(app.background().is_empty());
    assert!(app.background_view.is_none());
    assert!(app.take_pending_bg_completions().is_empty());
    assert!(
        !app.background_focused(),
        "no indicator left to light after the slate is wiped"
    );
}

#[test]
fn set_background_runtime_targets_one_shell() {
    let mut app = app_with_shells(&["a", "b"]);
    app.set_background_runtime("bash_2", Duration::from_secs(7));
    assert_eq!(app.background()[0].runtime, Duration::ZERO);
    assert_eq!(app.background()[1].runtime, Duration::from_secs(7));
    app.set_background_runtime("nope", Duration::from_secs(9)); // no panic
}

#[test]
fn the_manager_band_owns_every_key_while_open() {
    let mut app = app_with_shells(&["a"]);
    app.open_background_view();
    // Typing does not reach the composer.
    app.on_key(key(KeyCode::Char('h')));
    assert!(app.input.is_empty());
    // Enter does not submit — it navigates to the details page instead.
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::None);
    assert!(matches!(
        app.background_view,
        Some(BackgroundView::Details { .. })
    ));
}

#[test]
fn down_steps_onto_the_shell_indicator_then_into_the_roster() {
    let mut app = App::new();
    app.bg_started("b1", "sleep 99", None, true, None);
    app.begin_stream();
    app.start_agent_group(false, &agent_specs(false));
    assert_eq!(app.on_key(key(KeyCode::Down)), Action::None);
    assert!(app.background_focused(), "shells first");
    assert_eq!(app.on_key(key(KeyCode::Down)), Action::None);
    assert!(!app.background_focused());
    assert_eq!(app.agent_selection(), Some(0), "then the roster's main row");
    assert_eq!(app.on_key(key(KeyCode::Down)), Action::None);
    assert_eq!(app.agent_selection(), Some(1));
    // x on the selected agent stops it.
    assert_eq!(
        app.on_key(key(KeyCode::Char('x'))),
        Action::StopAgent("a1".to_string())
    );
    // Esc dismisses the selection.
    assert_eq!(app.on_key(key(KeyCode::Esc)), Action::None);
    assert_eq!(app.agent_selection(), None);
}

#[test]
fn down_opens_the_roster_directly_without_shells() {
    let mut app = App::new();
    app.begin_stream();
    app.start_agent_group(false, &agent_specs(false));
    assert_eq!(app.on_key(key(KeyCode::Down)), Action::None);
    assert_eq!(app.agent_selection(), Some(0));
    // ↑ from the main row exits the selection.
    assert_eq!(app.on_key(key(KeyCode::Up)), Action::None);
    assert_eq!(app.agent_selection(), None);
}

#[test]
fn a_live_foreground_group_enables_ctrl_b() {
    let mut app = App::new();
    app.begin_stream();
    assert!(!app.can_move_to_background(), "nothing runs yet");
    app.start_agent_group(false, &agent_specs(false));
    assert!(
        app.can_move_to_background(),
        "a foreground group polls the latch"
    );
    app.finish_agent_group(
        false,
        &[
            AgentCallDone {
                id: "a1".into(),
                output: "r1".into(),
                ok: true,
            },
            AgentCallDone {
                id: "a2".into(),
                output: "r2".into(),
                ok: true,
            },
        ],
    );
    assert!(
        !app.can_move_to_background(),
        "resolved — nothing to hand over"
    );
    // A background group never offers the hand-off (it is already there).
    app.start_agent_group(true, &agent_specs(true));
    assert!(!app.can_move_to_background());
}

#[test]
fn the_last_shell_exiting_closes_the_manager_band() {
    let mut app = app_with_shells(&["a", "b"]);
    app.open_background_view();
    app.bg_exited("bash_1", Some(0), false);
    assert!(
        app.background_view.is_some(),
        "one shell still runs — the band stays open on the (re-clamped) list"
    );
    app.bg_exited("bash_2", Some(0), false);
    assert!(
        app.background_view.is_none(),
        "nothing left to manage — the band closes and the composer returns"
    );
}

#[test]
fn the_details_page_of_the_last_shell_closes_the_band() {
    let mut app = app_with_shells(&["a"]);
    app.open_background_view();
    app.on_key(key(KeyCode::Enter)); // details of bash_1
    app.bg_exited("bash_1", None, true);
    assert!(
        app.background_view.is_none(),
        "the only shell it was watching is gone — no empty list to fall back to"
    );
}
