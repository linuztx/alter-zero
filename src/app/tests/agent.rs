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
            arguments: None,
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
    // Typing + Enter chats with the viewed agent. Whether the message joins
    // its queue or starts a continuation is the registry's call, so the key
    // arm only hands the boundary the text (docs/queue.md) — the composer is
    // consumed and nothing is claimed on the transcript yet.
    app.input = TextArea::from_text("and humidity?");
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::AgentChat {
            id: "a1".to_string(),
            text: "and humidity?".to_string()
        }
    );
    assert_eq!(app.input.text(), "");
    let run = app.agent("a1").unwrap();
    assert!(
        !run.history.iter().any(|item| matches!(
            item,
            HistoryItem::Message(m) if m.text == "and humidity?"
        )),
        "the boundary records it once the registry says how it landed"
    );
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
    // ↓ inside the view opens the roster selection on the **viewed** agent's
    // row (entering it was a pick — `docs/agent-tool.md`); ↑ steps up to
    // `● main`, whose Enter leaves the view.
    app.on_key(key(KeyCode::Down));
    assert_eq!(app.agent_selection(), Some(1));
    app.on_key(key(KeyCode::Up));
    assert_eq!(app.agent_selection(), Some(0));
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::LeaveAgentView);
    assert!(app.agent_view.is_none());
    assert_eq!(app.agent_selection(), None);
}

#[test]
fn down_reopens_the_roster_on_the_last_picked_agent() {
    // The roster remembers where the user was: ↓ walks onto an agent, Enter
    // opens its session — and the next ↓ comes back to *that* row rather
    // than restarting at `● main` (the user-reported flow: stepping between
    // several agents shouldn't cost two keys every time).
    let mut app = App::new();
    app.begin_stream();
    app.start_agent_group(false, &agent_specs(false));
    app.on_key(key(KeyCode::Down));
    assert_eq!(app.agent_selection(), Some(0), "the first ↓ opens on main");
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Down));
    assert_eq!(
        app.agent_selection(),
        Some(2),
        "…and walks to the 2nd agent"
    );
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::ViewAgent("a2".to_string())
    );
    assert_eq!(app.agent_selection(), None, "Enter hands back the keys");
    // The remembered row is where ↓ resumes.
    app.on_key(key(KeyCode::Down));
    assert_eq!(app.agent_selection(), Some(2));
    // Esc leaves it remembered too — the selection is a cursor, not a mode.
    app.on_key(key(KeyCode::Esc));
    app.on_key(key(KeyCode::Down));
    assert_eq!(app.agent_selection(), Some(2));
    // Walking back onto `● main` forgets it: ↓ opens on main again.
    app.on_key(key(KeyCode::Up));
    app.on_key(key(KeyCode::Up));
    assert_eq!(app.agent_selection(), Some(0));
    app.on_key(key(KeyCode::Esc));
    app.on_key(key(KeyCode::Down));
    assert_eq!(app.agent_selection(), Some(0));
}

#[test]
fn the_remembered_row_falls_back_to_main_when_its_agent_is_gone() {
    // A remembered agent that has been swept off the roster can't be
    // selected — ↓ falls back to the `● main` row instead of a stale index.
    let mut app = App::new();
    app.begin_stream();
    app.start_agent_group(false, &agent_specs(false));
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Down));
    assert_eq!(app.agent_selection(), Some(2));
    app.on_key(key(KeyCode::Esc));
    app.remove_agent("a2");
    app.on_key(key(KeyCode::Down));
    assert_eq!(app.agent_selection(), Some(0));
}

#[test]
fn x_interrupts_the_agent_and_a_second_x_clears_the_lingering_row() {
    // `x` on a running agent stops it — and the row **stays**, red, for the
    // stopped linger (30s) so the user sees what they stopped; a second `x`
    // is the clear that removes it. See `docs/agent-tool.md`.
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
    let Some(AgentStop::Stopped(notice)) = app.stop_agent("a1") else {
        panic!("a live agent was stopped");
    };
    let notice = notice.expect("a background stop owes a notice");
    assert_eq!(notice.status, crate::agents::AgentStatus::Interrupted);
    assert!(notice.headline().contains("was stopped by user"));
    let run = app.agent("a1").expect("the stopped row stays listed");
    assert_eq!(run.status, crate::agents::AgentStatus::Interrupted);
    assert_eq!(
        run.linger(),
        crate::agents::AGENT_STOPPED_LINGER,
        "a user stop keeps its own 30s window"
    );
    assert_eq!(app.visible_agents().len(), 2, "the red row stays");
    // The second `x` clears it.
    assert_eq!(app.stop_agent("a1"), Some(AgentStop::Cleared));
    assert_eq!(app.visible_agents().len(), 1, "the cleared row left");
    assert_eq!(app.stop_agent("a1"), None, "nothing left to stop or clear");
}

#[test]
fn clearing_the_viewed_agent_closes_its_session_view() {
    // The session view renders *from* the roster entry, so a clear that drops
    // the entry takes the screen with it (the boundary repaints the main
    // conversation) — leaving the agent's transcript up over a roster that no
    // longer lists it, and marks no session as in view, is the broken middle
    // state. The stop before it changes nothing: that row is still there.
    let mut app = App::new();
    app.begin_stream();
    app.start_agent_group(true, &agent_specs(true));
    app.open_agent_view("a1");
    assert!(matches!(app.stop_agent("a1"), Some(AgentStop::Stopped(_))));
    assert_eq!(
        app.agent_view.as_deref(),
        Some("a1"),
        "the stop leaves the view open on the red row"
    );
    assert_eq!(app.stop_agent("a1"), Some(AgentStop::Cleared));
    assert!(app.agent_view.is_none(), "the clear closed the view");
    assert_eq!(app.visible_agents().len(), 1);
}

#[test]
fn x_clears_a_naturally_finished_row_too() {
    // A done agent lingers green; `x` on it is the same clear (the hint
    // reads `x to clear` for every settled row).
    let mut app = App::new();
    app.begin_stream();
    app.start_agent_group(true, &agent_specs(true));
    app.apply_agent_event("a1", &StreamEvent::Chunk("done".into()));
    app.apply_agent_event("a1", &StreamEvent::StreamDone);
    let run = app.agent("a1").expect("listed");
    assert!(run.status.is_final());
    assert_eq!(
        run.linger(),
        crate::agents::AGENT_LINGER,
        "a natural finish wears its own linger window"
    );
    assert_eq!(app.stop_agent("a1"), Some(AgentStop::Cleared));
    assert_eq!(app.visible_agents().len(), 1);
}

#[test]
fn a_naturally_finished_agent_lingers_long_enough_to_be_read() {
    // The green row is the roster's only evidence the agent ran until its
    // group cell commits — swept in a few seconds it can vanish before the
    // user has read it (and, mid-group, before the group cell exists). A
    // natural finish keeps the same 30s window a user stop earns; only the
    // reason differs. See `docs/agent-tool.md`.
    let mut app = App::new();
    app.begin_stream();
    app.start_agent_group(true, &agent_specs(true));
    app.apply_agent_event("a1", &StreamEvent::Chunk("done".into()));
    app.apply_agent_event("a1", &StreamEvent::StreamDone);
    let run = app.agent("a1").expect("listed");
    assert!(run.status.is_final(), "the run settled");
    assert!(
        run.linger() >= std::time::Duration::from_secs(30),
        "a finished row must stay long enough to be read, got {:?}",
        run.linger()
    );
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
    // No id token: the model cannot address an agent by id anywhere, so the
    // description is the note's whole correlation key.
    assert!(
        !notice.context_text().contains("(id "),
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

// ===== The agent session view's `/copy` and thinking (docs/agent-view-streaming.md) =====

#[test]
fn last_assistant_text_follows_the_viewed_agent() {
    // `/copy` copies what the screen shows: inside an agent session view the
    // screen is that AGENT's conversation, so the last assistant message is
    // its own — not the lead's, which the user isn't looking at (the
    // reported bug, docs/agent-view-streaming.md).
    let mut app = App::new();
    app.begin_stream();
    app.push_chunk("the lead's answer");
    app.finish_stream();
    app.start_agent_group(false, &agent_specs(false));
    app.apply_agent_event("a1", &StreamEvent::Chunk("the agent's answer".into()));
    app.apply_agent_event("a1", &StreamEvent::StreamDone);
    assert_eq!(
        app.last_assistant_text().as_deref(),
        Some("the lead's answer"),
        "the main view copies the main conversation"
    );
    app.open_agent_view("a1");
    assert_eq!(
        app.last_assistant_text().as_deref(),
        Some("the agent's answer"),
        "the agent view copies the agent's"
    );
    // …and `/copy` through the palette carries it.
    for c in "/copy".chars() {
        app.on_key(key(KeyCode::Char(c)));
    }
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::Copy(Some("the agent's answer".to_string()))
    );
}

#[test]
fn the_context_gauge_follows_the_viewed_agent() {
    // The footer's gauge describes the conversation ON SCREEN
    // (docs/agent-context-gauge.md): the main session's `input + output`
    // while the main view is up, the viewed agent's own once its session view
    // opens — each against its own window.
    let mut app = App::new();
    app.set_context_window(Some(1_000_000));
    app.begin_stream();
    app.apply_usage(&usage_of(23_000, 700));
    app.start_agent_group(false, &agent_specs(false));
    app.apply_agent_event(
        "a1",
        &crate::stream::StreamEvent::Usage(usage_of(64_000, 900)),
    );
    assert_eq!(app.context_gauge(), Some((23_700, 1_000_000)));
    app.set_agent_context_window(Some(1_000_000));
    app.open_agent_view("a1");
    assert_eq!(
        app.context_gauge(),
        Some((64_900, 1_000_000)),
        "the agent's own, not the lead's"
    );
    app.close_agent_view();
    assert_eq!(app.context_gauge(), Some((23_700, 1_000_000)));
}

#[test]
fn a_viewed_agent_with_no_known_window_has_no_gauge() {
    // A type pinned to another model runs against a window this session may
    // not know: no gauge — exactly as the main footer hides its own without a
    // window — rather than the lead's denominator under the agent's count.
    let mut app = App::new();
    app.set_context_window(Some(1_000_000));
    app.begin_stream();
    app.start_agent_group(false, &agent_specs(false));
    app.apply_agent_event(
        "a1",
        &crate::stream::StreamEvent::Usage(usage_of(64_000, 900)),
    );
    app.set_agent_context_window(None);
    app.open_agent_view("a1");
    assert_eq!(app.context_gauge(), None);
}

#[test]
fn the_viewed_agents_pinned_model_shows_only_inside_its_view() {
    let mut app = App::new();
    app.begin_stream();
    app.start_agent_group(false, &agent_specs(false));
    app.set_agent_model(Some("kimi-k3".to_string()));
    assert_eq!(app.viewed_agent_model(), None, "no view, no agent model");
    app.open_agent_view("a1");
    assert_eq!(app.viewed_agent_model(), Some("kimi-k3"));
    app.set_agent_model(None);
    assert_eq!(
        app.viewed_agent_model(),
        None,
        "an inheriting type runs on the session's model"
    );
}

#[test]
fn an_agent_with_nothing_to_copy_reports_the_empty_case() {
    // A freshly launched agent has only its prompt (a *user* message) — the
    // empty path, not the lead's answer leaking through the view.
    let mut app = App::new();
    app.begin_stream();
    app.push_chunk("the lead's answer");
    app.finish_stream();
    app.start_agent_group(false, &agent_specs(false));
    app.open_agent_view("a1");
    assert!(app.last_assistant_text().is_none());
}

#[test]
fn an_agents_thinking_phase_settles_onto_its_own_transcript() {
    // The main session's shape (docs/thinking-stream.md), on the agent's
    // transcript: the buffer opens only when the boundary asks (the display
    // gate), the deltas accumulate, and the settle records the cell AHEAD of
    // the reply it preceded.
    let mut app = App::new();
    app.begin_stream();
    app.start_agent_group(false, &agent_specs(false));
    app.apply_agent_event("a1", &StreamEvent::ThinkingChunk("unseen".into()));
    assert!(
        app.agent_reasoning("a1").is_none(),
        "no phase open — the delta is counted and dropped"
    );
    app.begin_agent_reasoning("a1");
    app.apply_agent_event("a1", &StreamEvent::ThinkingChunk("weighing it".into()));
    assert_eq!(app.agent_reasoning("a1"), Some("weighing it"));
    let settled = app
        .finish_agent_reasoning("a1", 3)
        .expect("a phase with text settles");
    assert_eq!(settled.text, "weighing it");
    assert_eq!(settled.secs, 3);
    app.apply_agent_event("a1", &StreamEvent::Chunk("the answer".into()));
    app.apply_agent_event("a1", &StreamEvent::StreamDone);
    let history = &app.agent("a1").expect("listed").history;
    let kinds: Vec<&str> = history
        .iter()
        .map(|item| match item {
            HistoryItem::Message(m) if m.role == Role::User => "user",
            HistoryItem::Message(_) => "assistant",
            HistoryItem::Reasoning(_) => "reasoning",
            HistoryItem::Summary(_) => "summary",
            _ => "other",
        })
        .collect();
    assert_eq!(kinds, ["user", "reasoning", "assistant", "summary"]);
}

#[test]
fn an_empty_agent_thinking_phase_records_nothing() {
    // A provider that opens and closes a phase without a delta: no
    // `Thought for 0s` noise (the main session's rule).
    let mut app = App::new();
    app.begin_stream();
    app.start_agent_group(false, &agent_specs(false));
    app.begin_agent_reasoning("a1");
    assert!(app.finish_agent_reasoning("a1", 0).is_none());
    let history = &app.agent("a1").expect("listed").history;
    assert_eq!(history.len(), 1, "only the prompt: {history:?}");
}

// ===== chatting with a running agent: its own mid-turn queue (docs/queue.md) =====

#[test]
fn a_message_typed_into_a_running_agents_session_waits_on_its_queue() {
    // The boundary asks the registry, not the roster — only the registry
    // knows whether the loop is still running — and a queued message shows
    // above the box until that loop's next round boundary takes it.
    let mut app = App::new();
    app.begin_stream();
    app.start_agent_group(false, &agent_specs(false));
    let generation = app.agents_generation();
    app.queue_agent_chat("a1", "also check Manila");
    assert_eq!(
        app.agent("a1").expect("the row").queued,
        ["also check Manila"]
    );
    assert!(
        app.agents_generation() > generation,
        "the transcript cache is told the agent's view changed"
    );
}

#[test]
fn the_agents_round_boundary_delivers_its_queued_message() {
    let mut app = App::new();
    app.begin_stream();
    app.start_agent_group(false, &agent_specs(false));
    app.queue_agent_chat("a1", "also check Manila");
    app.apply_agent_event(
        "a1",
        &crate::stream::StreamEvent::Steered {
            text: "also check Manila".to_string(),
        },
    );
    let run = app.agent("a1").expect("the row");
    assert!(run.queued.is_empty());
    assert!(
        matches!(
            run.history.last(),
            Some(HistoryItem::Message(m))
                if m.role == Role::User && m.text == "also check Manila"
        ),
        "it lands on that agent's transcript, not the main one"
    );
    assert!(
        !app.history.iter().any(|item| matches!(
            item,
            HistoryItem::Message(m) if m.text == "also check Manila"
        )),
        "the main conversation is untouched"
    );
}

#[test]
fn a_message_reaches_an_agents_transcript_by_exactly_one_path() {
    // Two things can put a user-role message on an agent's transcript: the
    // continuation recorder (`agent_chat` — the run carries the message, so
    // nothing will announce it) and the delivery echo. A caller that uses
    // both records it twice, permanently — in history, the rollout, the
    // context replay and every rebuild. That is what routing a
    // subagent-launched shell's completion note through the seam *and*
    // `agent_chat` used to do (`docs/queue.md`), so the queue path must
    // record nothing until the echo arrives.
    let note = "[background] Background command \"npm test\" completed.";
    let mut app = App::new();
    app.begin_stream();
    app.start_agent_group(false, &agent_specs(false));
    app.queue_agent_chat("a1", note);
    app.apply_agent_event(
        "a1",
        &crate::stream::StreamEvent::Steered {
            text: note.to_string(),
        },
    );
    let landed = app
        .agent("a1")
        .expect("the row")
        .history
        .iter()
        .filter(|item| matches!(item, HistoryItem::Message(m) if m.text == note))
        .count();
    assert_eq!(landed, 1, "the echo records it, and only the echo");
}

#[test]
fn a_foreground_groups_resolution_settles_its_members_live_calls() {
    // A member the group settles from the call's outcome — its own terminal
    // event never arrived (a killed loop returns without one; the offline
    // dummy scripts none at all) — must settle the way every other settle
    // does: its running call resolved onto the transcript, its queue cleared.
    // Flipping the status alone left a *finished* agent still owning live
    // cells, so its session view previewed a `⎿ Running…` that could never
    // resolve and the next chat continuation's batch queued behind it.
    let mut app = App::new();
    app.begin_stream();
    app.start_agent_group(false, &agent_specs(false));
    for event in [
        StreamEvent::ToolBatch(vec![
            ToolCallSummary {
                name: "Bash".into(),
                args: "sleep 30".into(),
            },
            ToolCallSummary {
                name: "Read".into(),
                args: "main.rs".into(),
            },
        ]),
        StreamEvent::ToolStart {
            name: "Bash".into(),
            args: "sleep 30".into(),
            detail: None,
            arguments: None,
        },
    ] {
        app.apply_agent_event("a1", &event);
    }
    assert_eq!(app.agent("a1").unwrap().tool_queue.len(), 2, "mid-round");
    app.finish_agent_group(
        false,
        &[
            AgentCallDone {
                id: "a1".into(),
                output: AGENT_STOPPED_OUTPUT.into(),
                ok: false,
            },
            AgentCallDone {
                id: "a2".into(),
                output: AGENT_STOPPED_OUTPUT.into(),
                ok: false,
            },
        ],
    );
    let run = app.agent("a1").expect("the roster entry");
    assert_eq!(run.status, crate::agents::AgentStatus::Interrupted);
    assert!(
        run.tool_queue.is_empty(),
        "a settled agent owns no live cells: {:?}",
        run.tool_queue
    );
    assert!(
        matches!(run.history.last(), Some(HistoryItem::Tool(tool))
            if tool.name == "Bash" && tool.status == ToolStatus::Failed),
        "the running call resolved onto its transcript: {:?}",
        run.history.last()
    );
}

// ===== the agent view's follow-up queue: Tab, one level down (docs/queue.md) =====

#[test]
fn tab_in_an_agent_view_queues_a_follow_up_for_that_agent() {
    // The bug this pins: Tab read the *main* session's `is_streaming()` and
    // pushed onto the *main* session's `queued`, so a message typed into a
    // subagent's session ran as a follow-up turn of the lead conversation.
    let mut app = App::new();
    app.begin_stream();
    app.start_agent_group(false, &agent_specs(false));
    app.open_agent_view("a1");
    let generation = app.agents_generation();
    app.input = TextArea::from_text("also add Elixir");
    assert_eq!(app.on_key(key(KeyCode::Tab)), Action::None);
    assert_eq!(app.input.text(), "", "Tab consumes the composer like Enter");
    assert_eq!(
        app.agent("a1").expect("the row").followups,
        ["also add Elixir"],
        "it queues a follow-up turn for the viewed agent"
    );
    assert!(
        app.queued.is_empty() && app.steered.is_empty(),
        "the main session's queue is untouched"
    );
    assert!(
        app.agents_generation() > generation,
        "the transcript cache is told the agent's view changed"
    );
}

#[test]
fn each_tab_in_an_agent_view_opens_its_own_follow_up_turn() {
    let mut app = App::new();
    app.begin_stream();
    app.start_agent_group(false, &agent_specs(false));
    app.open_agent_view("a1");
    for text in ["first", "second"] {
        app.input = TextArea::from_text(text);
        app.on_key(key(KeyCode::Tab));
    }
    assert_eq!(
        app.agent("a1").expect("the row").followups,
        ["first", "second"],
        "one entry per Tab, in submission order"
    );
}

#[test]
fn tab_in_an_agent_view_whose_agent_settled_is_a_no_op() {
    // The main session's rule one level down: Tab only queues against a
    // *running* turn, and an idle Tab keeps the draft (docs/queue.md).
    let mut app = App::new();
    app.begin_stream();
    app.start_agent_group(false, &agent_specs(false));
    app.apply_agent_event("a1", &StreamEvent::StreamDone);
    app.open_agent_view("a1");
    app.input = TextArea::from_text("too late");
    assert_eq!(app.on_key(key(KeyCode::Tab)), Action::None);
    assert!(app.agent("a1").expect("the row").followups.is_empty());
    assert!(app.queued.is_empty(), "and never the main session's queue");
    assert_eq!(app.input.text(), "too late", "the draft survives");
}

#[test]
fn the_agent_view_dispatches_one_follow_up_per_settle() {
    // `take_agent_followup` is what the boundary drains at the agent's settle
    // — one entry per turn, exactly like `drain_next_batch` (docs/queue.md).
    let mut app = App::new();
    app.begin_stream();
    app.start_agent_group(false, &agent_specs(false));
    app.open_agent_view("a1");
    for text in ["first", "second"] {
        app.input = TextArea::from_text(text);
        app.on_key(key(KeyCode::Tab));
    }
    assert_eq!(app.take_agent_followup("a1").as_deref(), Some("first"));
    assert_eq!(app.take_agent_followup("a1").as_deref(), Some("second"));
    assert_eq!(app.take_agent_followup("a1"), None, "drained");
}

#[test]
fn alt_up_in_an_agent_view_pulls_back_that_agents_follow_up() {
    // …and never a main-session batch, which is what it used to reach.
    let mut app = App::new();
    app.begin_stream();
    app.start_agent_group(false, &agent_specs(false));
    app.queued.push_back(batch(&["a main follow-up"]));
    app.open_agent_view("a1");
    app.input = TextArea::from_text("also add Elixir");
    app.on_key(key(KeyCode::Tab));
    assert_eq!(app.on_key(alt(KeyCode::Up)), Action::None);
    assert_eq!(app.input.text(), "also add Elixir", "back in the composer");
    assert!(app.agent("a1").expect("the row").followups.is_empty());
    assert_eq!(
        app.queued.len(),
        1,
        "the main session's batch stayed queued"
    );
}

#[test]
fn alt_up_in_an_agent_view_never_reaches_the_main_queue() {
    let mut app = App::new();
    app.begin_stream();
    app.start_agent_group(false, &agent_specs(false));
    app.queued.push_back(batch(&["a main follow-up"]));
    app.open_agent_view("a1");
    assert_eq!(app.on_key(alt(KeyCode::Up)), Action::None);
    assert_eq!(app.input.text(), "", "nothing came back");
    assert_eq!(app.queued.len(), 1, "the main batch is untouched");
}

#[test]
fn stopping_an_agent_drops_its_follow_ups() {
    // Its loop is cancelled, so no continuation will ever run them — the rule
    // `interrupt` already applies to the steered rows (docs/queue.md).
    let mut app = App::new();
    app.begin_stream();
    app.start_agent_group(false, &agent_specs(false));
    app.open_agent_view("a1");
    app.input = TextArea::from_text("never mind");
    app.on_key(key(KeyCode::Tab));
    app.stop_agent("a1");
    assert!(app.agent("a1").expect("the row").followups.is_empty());
}

#[test]
fn alt_up_in_an_agent_view_reaches_its_unread_steered_message() {
    // With no follow-up left, Alt+Up asks for the message the agent's loop
    // has not read yet — the main session's fall-through to
    // `Action::ReclaimSteered`, one level down. Only the registry knows
    // whether it can still be taken back, so the key only asks.
    let mut app = App::new();
    app.begin_stream();
    app.start_agent_group(false, &agent_specs(false));
    app.open_agent_view("a1");
    app.queue_agent_chat("a1", "also check Manila");
    assert_eq!(
        app.on_key(alt(KeyCode::Up)),
        Action::ReclaimAgentChat {
            id: "a1".to_string()
        }
    );
    // The boundary's answer puts it back in the composer and drops the row.
    app.recall_agent_chat("a1", "also check Manila");
    assert_eq!(app.input.text(), "also check Manila");
    assert!(app.agent("a1").expect("the row").queued.is_empty());
}

#[test]
fn alt_up_in_an_agent_view_prefers_the_follow_up_queue() {
    // The deliberate backlog first, exactly as the main session prefers
    // `queued` over its steered messages.
    let mut app = App::new();
    app.begin_stream();
    app.start_agent_group(false, &agent_specs(false));
    app.open_agent_view("a1");
    app.queue_agent_chat("a1", "steered");
    app.input = TextArea::from_text("a follow up");
    app.on_key(key(KeyCode::Tab));
    assert_eq!(app.on_key(alt(KeyCode::Up)), Action::None);
    assert_eq!(app.input.text(), "a follow up");
    assert_eq!(
        app.agent("a1").expect("the row").queued,
        ["steered"],
        "the steered row is untouched"
    );
}

#[test]
fn a_nested_tool_header_splits_back_into_its_name_and_path() {
    // `tool_header_text` writes the Ctrl+O agent cell's nested `Name(args)`
    // one-liners; `file_tool_header` is its inverse for the three file tools,
    // so the cell can show a recorded header's path by the session's rule
    // (docs/tools.md "Path display") without ui re-parsing the grammar.
    for (name, path) in [
        ("Write", "/home/u/repo/a.py"),
        ("Read", "/home/u/x (copy).py"),
        ("Edit", ""),
    ] {
        let text = agent::tool_header_text(name, path);
        assert_eq!(text, format!("{name}({path})"));
        assert_eq!(agent::file_tool_header(&text), Some((name, path)), "{text}");
    }
    // Every other tool's header renders as recorded — including one whose
    // name or args carry parentheses of their own.
    assert_eq!(agent::file_tool_header("Bash(echo (hi))"), None);
    assert_eq!(
        agent::file_tool_header(r#"deepwiki - ask_question (MCP)({"q":"x"})"#),
        None
    );
    assert_eq!(agent::file_tool_header("Write(a.py"), None, "unclosed");
    assert_eq!(
        agent::file_tool_header("Writer(a.py)"),
        None,
        "a prefix is not the name"
    );
}
