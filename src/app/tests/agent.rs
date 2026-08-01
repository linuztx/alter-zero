//! The `Agent` tool's roster, groups, and notices (`docs/agent-tool.md`).

use super::*;

#[test]
fn the_init_prompt_is_codexs_agents_md_generator() {
    // The canned prompt *is* the feature — everything after the submit is
    // the normal turn machinery. Codex's prompt_for_init_command.md asks
    // the model to generate AGENTS.md and to leave an existing one alone.
    assert!(INIT_PROMPT.contains("AGENTS.md"));
    assert!(
        INIT_PROMPT.contains("do not overwrite"),
        "the prompt guards an existing AGENTS.md"
    );
}

#[test]
fn set_user_instructions_stores_the_agents_md_fragment() {
    // The boundary injects the rendered AGENTS.md instructions (the
    // set_system_prompt pattern); everyone deriving context reads them.
    // See docs/project-doc.md.
    let mut app = App::new();
    assert_eq!(app.user_instructions, None);
    app.set_user_instructions(Some("<INSTRUCTIONS>\nguide\n</INSTRUCTIONS>".to_string()));
    assert_eq!(
        app.user_instructions.as_deref(),
        Some("<INSTRUCTIONS>\nguide\n</INSTRUCTIONS>")
    );
    app.set_user_instructions(None);
    assert_eq!(app.user_instructions, None);
}

#[test]
fn agent_batch_seeds_the_roster_and_the_live_group() {
    let mut app = App::new();
    app.begin_stream();
    app.start_agent_group(false, &agent_specs(false));
    assert_eq!(app.agents().len(), 2);
    assert_eq!(app.agents()[0].description, "Fetch Warsaw weather");
    assert_eq!(app.visible_agents().len(), 2);
    let live = app.agent_group().expect("group live");
    assert_eq!(live.ids, vec!["a1", "a2"]);
    assert!(!live.background);
}

#[test]
fn finish_agent_group_snapshots_the_roster_around_the_outputs() {
    let mut app = App::new();
    app.begin_stream();
    app.start_agent_group(false, &agent_specs(false));
    // a1 streams a tool round + a final reply on the agent channel.
    for event in [
        StreamEvent::ToolStart {
            name: "Bash".into(),
            args: "curl wttr.in".into(),
            detail: None,
        },
        StreamEvent::ToolEnd {
            output: "+19°C".into(),
            ok: true,
            truncated: false,
        },
        StreamEvent::Chunk("Warsaw is 19°C.".into()),
        StreamEvent::StreamDone,
    ] {
        app.apply_agent_event("a1", &event);
    }
    let group = app.finish_agent_group(
        false,
        &[
            AgentCallDone {
                id: "a1".into(),
                output: "Warsaw is 19°C.".into(),
                ok: true,
            },
            AgentCallDone {
                id: "a2".into(),
                output: AGENT_STOPPED_OUTPUT.into(),
                ok: false,
            },
        ],
    );
    assert!(app.agent_group().is_none(), "live cell cleared");
    assert!(matches!(
        app.history.last(),
        Some(HistoryItem::AgentGroup(_))
    ));
    assert_eq!(group.agents.len(), 2);
    assert_eq!(group.agents[0].status, crate::agents::AgentStatus::Done);
    assert_eq!(group.agents[0].tool_uses, 1);
    assert_eq!(group.agents[0].result, "Warsaw is 19°C.");
    assert_eq!(group.agents[0].tool_headers, vec!["Bash(curl wttr.in)"]);
    assert_eq!(group.agents[0].output, "Warsaw is 19°C.");
    assert!(!group.ok(), "a2 never finished — the cell is red");
}

#[test]
fn interrupt_resolves_a_live_agent_group_locally() {
    let mut app = App::new();
    app.record_user_message("go");
    app.begin_stream();
    app.push_chunk("spawning ");
    app.flush_streaming_segment();
    app.start_agent_group(false, &agent_specs(false));
    let Some(InterruptedTurn::Kept { agents, notice, .. }) = app.interrupt_turn() else {
        panic!("a live group forces the keep path");
    };
    let group = agents.expect("the group resolved with the turn");
    assert!(group.agents.iter().all(|entry| {
        entry.status == crate::agents::AgentStatus::Interrupted
            && entry.output == AGENT_STOPPED_OUTPUT
    }));
    assert_eq!(notice, Some(INTERRUPT_NOTICE));
    assert!(app.agent_group().is_none());
    // The roster entries settled too (they linger until swept).
    assert!(app.agents().iter().all(|run| run.status.is_final()));
}

#[test]
fn a_live_agent_group_blocks_the_interrupt_undo() {
    let mut app = App::new();
    app.record_user_message("go");
    app.begin_stream();
    // Nothing streamed, but a group is live: undo would orphan it.
    app.start_agent_group(false, &agent_specs(false));
    assert!(matches!(
        app.interrupt_turn(),
        Some(InterruptedTurn::Kept { .. })
    ));
}

#[test]
fn a_background_agent_completion_returns_its_notice() {
    let mut app = App::new();
    app.begin_stream();
    app.start_agent_group(true, &agent_specs(true));
    app.finish_agent_group(
        true,
        &[
            AgentCallDone {
                id: "a1".into(),
                output: "launched a1".into(),
                ok: true,
            },
            AgentCallDone {
                id: "a2".into(),
                output: "launched a2".into(),
                ok: true,
            },
        ],
    );
    // The cell is green and description-only; the roster keeps running.
    assert!(app.agents().iter().all(|run| !run.status.is_final()));
    // a1 finishes later, on the agent channel.
    app.apply_agent_event("a1", &StreamEvent::Chunk("28°C".into()));
    let notice = app
        .apply_agent_event("a1", &StreamEvent::StreamDone)
        .expect("a background settle owes a notice");
    assert_eq!(notice.id, "a1");
    assert!(notice.ok());
    assert_eq!(notice.result, "28°C");
    assert!(
        notice
            .headline()
            .starts_with("Agent \"Fetch Warsaw weather\" finished")
    );
    // Settling updates the recorded group entry for the Ctrl+O cell.
    let generation = app.history_generation();
    app.record_agent_notice(&notice);
    app.settle_agent_completion(&notice);
    assert!(app.history_generation() > generation);
    let Some(HistoryItem::AgentGroup(group)) = app
        .history
        .iter()
        .find(|item| matches!(item, HistoryItem::AgentGroup(_)))
    else {
        panic!("group recorded");
    };
    assert_eq!(group.agents[0].status, crate::agents::AgentStatus::Done);
    assert_eq!(group.agents[0].result, "28°C");
    assert_eq!(
        group.agents[0].output, "launched a1",
        "the wire result never changes"
    );
}

#[test]
fn enter_views_an_agent_and_the_composer_chats_with_it() {
    let mut app = App::new();
    app.begin_stream();
    app.start_agent_group(false, &agent_specs(false));
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Down));
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::ViewAgent("a1".to_string())
    );
    assert_eq!(app.agent_view.as_deref(), Some("a1"));
    // Typing + Enter chats with the viewed agent, recording into its
    // transcript.
    app.input = TextArea::from_text("and humidity?");
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::AgentChat {
            id: "a1".to_string(),
            text: "and humidity?".to_string()
        }
    );
    let run = app.agent("a1").unwrap();
    assert!(matches!(
        run.history.last(),
        Some(HistoryItem::Message(m)) if m.text == "and humidity?" && m.role == Role::User
    ));
    // Esc with an empty composer leaves the view.
    assert_eq!(app.on_key(key(KeyCode::Esc)), Action::LeaveAgentView);
    assert!(app.agent_view.is_none());
}

#[test]
fn enter_on_main_from_an_agent_view_returns_to_the_main_session() {
    let mut app = App::new();
    app.begin_stream();
    app.start_agent_group(false, &agent_specs(false));
    app.open_agent_view("a1");
    // ↓ inside the view opens the roster selection on the `● main` row.
    app.on_key(key(KeyCode::Down));
    assert_eq!(app.agent_selection(), Some(0));
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::LeaveAgentView);
    assert!(app.agent_view.is_none());
    assert_eq!(app.agent_selection(), None);
}

#[test]
fn stop_agent_hides_the_row_and_a_background_stop_owes_a_notice() {
    let mut app = App::new();
    app.begin_stream();
    app.start_agent_group(true, &agent_specs(true));
    app.finish_agent_group(
        true,
        &[
            AgentCallDone {
                id: "a1".into(),
                output: "launched".into(),
                ok: true,
            },
            AgentCallDone {
                id: "a2".into(),
                output: "launched".into(),
                ok: true,
            },
        ],
    );
    let notice = app
        .stop_agent("a1")
        .expect("a live agent was stopped")
        .expect("a background stop owes a notice");
    assert_eq!(notice.status, crate::agents::AgentStatus::Interrupted);
    assert!(notice.headline().contains("was stopped by user"));
    assert_eq!(app.visible_agents().len(), 1, "the row left at once");
}

#[test]
fn up_from_the_main_row_steps_back_onto_the_shell_indicator() {
    // The ↓ walk is composer → shell indicator → roster; ↑ must walk the
    // same path back up (the user-requested flow): from `● main` it lands
    // on the footer's lit `{n} shell` segment — not straight back in the
    // textarea — and a second ↑ dismisses the indicator to the composer.
    let mut app = App::new();
    app.begin_stream();
    app.start_agent_group(false, &agent_specs(false));
    app.bg_started("b1", "ping google.com", None, true, None);
    // ↓ lights the indicator; a second ↓ enters the roster.
    app.on_key(key(KeyCode::Down));
    assert!(app.background_focused(), "↓ lights the shell indicator");
    app.on_key(key(KeyCode::Down));
    assert_eq!(app.agent_selection(), Some(0));
    assert!(!app.background_focused());
    // ↑ from `● main` steps back onto the indicator…
    app.on_key(key(KeyCode::Up));
    assert_eq!(app.agent_selection(), None);
    assert!(
        app.background_focused(),
        "↑ from the main row lands on the shell indicator"
    );
    // …and a second ↑ returns to the composer.
    app.on_key(key(KeyCode::Up));
    assert!(!app.background_focused());
    assert_eq!(app.agent_selection(), None);
}

#[test]
fn up_from_the_main_row_without_shells_returns_to_the_composer() {
    // With no shell running there is no indicator to land on — ↑ from
    // `● main` exits the selection to the composer, as before.
    let mut app = App::new();
    app.begin_stream();
    app.start_agent_group(false, &agent_specs(false));
    app.on_key(key(KeyCode::Down));
    assert_eq!(
        app.agent_selection(),
        Some(0),
        "no shells: ↓ goes straight to the roster"
    );
    app.on_key(key(KeyCode::Up));
    assert_eq!(app.agent_selection(), None);
    assert!(
        !app.background_focused(),
        "nothing to land on — back to the composer"
    );
}

#[test]
fn agent_notice_texts_humanize_the_runtime() {
    // `362s` reads as `6m 2s` in both the rendered headline and the
    // model-facing context note (the format_elapsed contract — the
    // user-report fix); under a minute stays the bare seconds.
    let notice = crate::app::AgentNotice {
        id: "a1".into(),
        description: "Count 1-100 with sleep".into(),
        status: crate::agents::AgentStatus::Done,
        secs: 362,
        result: "done".into(),
        timestamp: String::new(),
    };
    assert_eq!(
        notice.headline(),
        "Agent \"Count 1-100 with sleep\" finished · 6m 2s"
    );
    assert!(
        notice.context_text().contains("completed in 6m 2s"),
        "{}",
        notice.context_text()
    );
    let quick = crate::app::AgentNotice { secs: 35, ..notice };
    assert_eq!(
        quick.headline(),
        "Agent \"Count 1-100 with sleep\" finished · 35s"
    );
}

#[test]
fn the_agent_view_keeps_the_full_composer() {
    let mut app = App::new();
    app.begin_stream();
    app.start_agent_group(false, &agent_specs(false));
    app.open_agent_view("a1");
    // The slash palette opens like in the main session…
    app.on_key(key(KeyCode::Char('/')));
    assert!(
        app.command_menu.is_some(),
        "the palette opens in an agent view"
    );
    app.on_key(key(KeyCode::Esc)); // dismiss the palette
    app.on_key(key(KeyCode::Backspace)); // empty the composer again
    // …the `?` shortcuts band toggles…
    app.on_key(key(KeyCode::Char('?')));
    assert!(app.shortcuts_open);
    app.on_key(key(KeyCode::Esc));
    assert!(!app.shortcuts_open);
    // …Ctrl+R opens the history search…
    app.on_key(ctrl('r'));
    assert!(app.history_search.is_some());
    app.on_key(key(KeyCode::Esc));
    // …and Ctrl+O / Ctrl+D toggle their overlays (showing the agent's
    // transcript/context — the view stays an agent view underneath).
    assert_eq!(app.on_key(ctrl('o')), Action::ToggleToolView);
    assert_eq!(app.view, View::ToolOutput);
    assert_eq!(app.on_key(ctrl('o')), Action::ToggleToolView);
    assert_eq!(app.on_key(ctrl('d')), Action::ToggleContextDebug);
    assert_eq!(app.view, View::ContextDebug);
    assert_eq!(app.on_key(ctrl('d')), Action::ToggleContextDebug);
    assert_eq!(app.agent_view.as_deref(), Some("a1"), "the view survived");
    // A leading `!` stays literal chat text — no shell mode in an agent
    // session (the draft chats with the agent).
    app.on_key(key(KeyCode::Char('!')));
    assert!(!app.shell_mode);
    assert_eq!(app.input.text(), "!");
}

#[test]
fn clear_wipes_the_roster_and_the_live_group() {
    let mut app = App::new();
    app.begin_stream();
    app.start_agent_group(false, &agent_specs(false));
    // Type the command so the palette opens (a direct set bypasses it);
    // mid-turn /clear is a kill, agents included.
    for c in "/clear".chars() {
        app.on_key(key(KeyCode::Char(c)));
    }
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::Clear);
    assert!(app.agents().is_empty());
    assert!(app.agent_group().is_none());
    assert!(app.agent_view.is_none());
}
