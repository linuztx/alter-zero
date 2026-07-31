//! Subagent rendering (`docs/agent-tool.md`).

use super::*;
use crate::ui::agent::agent_group_full_lines;
use crate::ui::theme::{
    TOOL_FAIL_COLOR, TOOL_OK_COLOR, TOOL_PULSE_BRIGHT, TOOL_PULSE_DIM, TOOL_PULSE_PERIOD,
};

#[test]
fn agent_group_lines_render_the_finished_tree() {
    use crate::agents::AgentStatus;
    let group = crate::app::AgentGroup {
        background: false,
        agents: vec![
            agent_entry("a1", "Fetch Warsaw", AgentStatus::Done),
            agent_entry("a2", "Fetch Manila", AgentStatus::Done),
        ],
        timestamp: String::new(),
    };
    let lines = agent_group_lines(&group, 80);
    let texts: Vec<String> = lines.iter().map(plain).collect();
    assert_eq!(texts[0], "● 2 agents finished (ctrl+o to expand)");
    assert_eq!(texts[1], "   ├ Fetch Warsaw · 2 tool uses · 16.1k tokens");
    assert_eq!(texts[2], "   │ ⎿  Done");
    assert_eq!(texts[3], "   └ Fetch Manila · 2 tool uses · 16.1k tokens");
    assert_eq!(texts[4], "     ⎿  Done");
    // All clean → green bullet; one interrupted → red.
    assert_eq!(lines[0].spans[0].style.fg, Some(TOOL_OK_COLOR));
    let mut stopped = group.clone();
    stopped.agents[1].status = AgentStatus::Interrupted;
    let lines = agent_group_lines(&stopped, 80);
    assert_eq!(lines[0].spans[0].style.fg, Some(TOOL_FAIL_COLOR));
    assert_eq!(plain(&lines[4]), "     ⎿  Interrupted");
}

#[test]
fn agent_group_lines_render_the_background_launch() {
    use crate::agents::AgentStatus;
    let group = crate::app::AgentGroup {
        background: true,
        agents: vec![
            agent_entry("a1", "Fetch Warsaw", AgentStatus::Running),
            agent_entry("a2", "Fetch Manila", AgentStatus::Running),
        ],
        timestamp: String::new(),
    };
    let texts: Vec<String> = agent_group_lines(&group, 80).iter().map(plain).collect();
    assert_eq!(texts[0], "● 2 background agents launched (↓ to manage)");
    assert_eq!(texts[1], "   ├ Fetch Warsaw");
    assert_eq!(texts[2], "   └ Fetch Manila");
    assert_eq!(texts.len(), 3, "description-only rows, no status");
}

#[test]
fn a_lone_live_agent_renders_the_tool_cell_shape() {
    let mut app = App::new();
    app.begin_stream();
    app.start_agent_group(false, &[spec("a1", "Fetch Warsaw", false)]);
    // Announced, no event yet: the Agent cell over `⎿ Initializing…`.
    let texts: Vec<String> = live_agent_group_lines(&app, 80).iter().map(plain).collect();
    assert_eq!(texts[0], "● Agent(Fetch Warsaw)");
    assert_eq!(texts[1], "  ⎿  Initializing…");
    // The strip sizes from the same walk (the box/cursor geometry contract).
    assert_eq!(usize::from(preview_rows(&app, 80)), texts.len());
    // A running tool shows its wrapped header + a dim Running… row.
    app.apply_agent_event(
        "a1",
        &crate::stream::StreamEvent::ToolStart {
            name: "Bash".into(),
            args: "sleep 10 && curl -s https://api.open-meteo.com/v1/forecast".into(),
            detail: Some("Fetching Warsaw weather".into()),
        },
    );
    let texts: Vec<String> = live_agent_group_lines(&app, 44).iter().map(plain).collect();
    assert_eq!(texts[0], "● Agent(Fetch Warsaw)");
    assert!(
        texts[1].starts_with("  ⎿  Bash(sleep 10 && curl"),
        "{}",
        texts[1]
    );
    assert!(
        texts[2].starts_with("         "),
        "continuations align under the (: {}",
        texts[2]
    );
    assert!(texts.iter().any(|t| t.trim() == "Running…"));
    assert_eq!(usize::from(preview_rows(&app, 44)), texts.len());
    // Between calls the sticky activity line holds — never `Working…`.
    app.apply_agent_event(
        "a1",
        &crate::stream::StreamEvent::ToolEnd {
            output: "+19°C".into(),
            ok: true,
            truncated: false,
        },
    );
    let texts: Vec<String> = live_agent_group_lines(&app, 80).iter().map(plain).collect();
    assert_eq!(texts[1], "  ⎿  Bash: Fetching Warsaw weather");
    // …and rendering the live region upholds the debug_assert.
    let area = Rect::new(0, 0, 80, 24);
    let mut buf = Buffer::empty(area);
    render_live(area, &mut buf, &app);
}

#[test]
fn a_lone_committed_agent_renders_done_with_the_expand_hint() {
    use crate::agents::AgentStatus;
    let group = crate::app::AgentGroup {
        background: false,
        agents: vec![agent_entry(
            "a1",
            "Fetch current weather in Warsaw",
            AgentStatus::Done,
        )],
        timestamp: String::new(),
    };
    let texts: Vec<String> = agent_group_lines(&group, 80).iter().map(plain).collect();
    assert_eq!(texts[0], "● Agent(Fetch current weather in Warsaw)");
    assert_eq!(texts[1], "  ⎿  Done (2 tool uses · 16.1k tokens · 39s)");
    assert_eq!(texts[2], "  (ctrl+o to expand)");
    // Interrupted: the red footer, no counters.
    let mut stopped = group.clone();
    stopped.agents[0].status = AgentStatus::Interrupted;
    let texts: Vec<String> = agent_group_lines(&stopped, 80).iter().map(plain).collect();
    assert_eq!(texts[1], "  ⎿  Interrupted");
    // A lone background launch keeps the manage row instead.
    let mut launched = group;
    launched.background = true;
    launched.agents[0].status = AgentStatus::Running;
    let texts: Vec<String> = agent_group_lines(&launched, 80).iter().map(plain).collect();
    assert_eq!(texts[0], "● Agent(Fetch current weather in Warsaw)");
    assert_eq!(texts[1], "  ⎿  Running in the background (↓ to manage)");
    assert_eq!(texts.len(), 2, "no expand hint on the backgrounded cell");
}

#[test]
fn a_multi_agent_tree_keeps_the_sticky_tool_activity() {
    let mut app = App::new();
    app.begin_stream();
    app.start_agent_group(
        false,
        &[
            spec("a1", "Fetch Warsaw", false),
            spec("a2", "Write a game", false),
        ],
    );
    app.apply_agent_event(
        "a1",
        &crate::stream::StreamEvent::ToolStart {
            name: "Bash".into(),
            args: "curl wttr.in/Warsaw".into(),
            detail: Some("Fetching Warsaw weather".into()),
        },
    );
    for event in [
        crate::stream::StreamEvent::ToolStart {
            name: "Write".into(),
            args: "game.py".into(),
            detail: None,
        },
        // The call resolves — the activity line stays (sticky, no
        // `Working…` between calls).
        crate::stream::StreamEvent::ToolEnd {
            output: "Created game.py (10 lines)".into(),
            ok: true,
            truncated: false,
        },
    ] {
        app.apply_agent_event("a2", &event);
    }
    let texts: Vec<String> = live_agent_group_lines(&app, 90).iter().map(plain).collect();
    assert_eq!(texts[0], "● Running 2 agents… (ctrl+o to expand)");
    assert!(
        texts[2].ends_with("⎿  Bash: Fetching Warsaw weather"),
        "{}",
        texts[2]
    );
    assert!(texts[4].ends_with("⎿  Write: game.py"), "{}", texts[4]);
}

#[test]
fn a_live_agent_groups_bullet_breathes_like_a_running_tool() {
    // The tree header is the round's "this is happening now" bullet, so it
    // pulses on the same clock as a running tool cell — the two are on screen
    // together in a mixed round and must not blink out of step
    // (`docs/tool-pulse.md`).
    let mut app = App::new();
    app.begin_stream();
    app.start_agent_group(
        false,
        &[
            spec("a1", "Fetch Warsaw", false),
            spec("a2", "Fetch Oslo", false),
        ],
    );
    let bullet = |app: &App| live_agent_group_lines(app, 90)[0].spans[0].style.fg;
    assert_eq!(
        bullet(&app),
        Some(Color::Rgb(
            TOOL_PULSE_DIM.0,
            TOOL_PULSE_DIM.1,
            TOOL_PULSE_DIM.2
        ))
    );
    app.set_pulse(TOOL_PULSE_PERIOD / 2);
    assert_eq!(
        bullet(&app),
        Some(Color::Rgb(
            TOOL_PULSE_BRIGHT.0,
            TOOL_PULSE_BRIGHT.1,
            TOOL_PULSE_BRIGHT.2
        ))
    );
}

#[test]
fn agent_cell_lines_expand_prompt_response_and_done() {
    use crate::agents::AgentStatus;
    let group = crate::app::AgentGroup {
        background: false,
        agents: vec![agent_entry("a1", "Fetch Warsaw", AgentStatus::Done)],
        timestamp: String::new(),
    };
    let texts: Vec<String> = agent_group_full_lines(&group, 100)
        .iter()
        .map(plain)
        .collect();
    assert_eq!(texts[0], "● Agent(Fetch Warsaw)");
    assert_eq!(texts[1], "  ⎿  Prompt:");
    assert_eq!(texts[2], "       What is the weather in Fetch Warsaw?");
    assert!(texts.contains(&"     Bash(curl wttr.in)".to_string()));
    assert!(texts.contains(&"  ⎿  Response:".to_string()));
    assert!(texts.contains(&"       It is 19°C.".to_string()));
    assert!(
        texts
            .last()
            .unwrap()
            .contains("Done (2 tool uses · 16.1k tokens · 39s)")
    );
    // An interrupted agent ends with the bare Interrupted footer instead.
    let mut stopped = group;
    stopped.agents[0].status = AgentStatus::Interrupted;
    let texts: Vec<String> = agent_group_full_lines(&stopped, 100)
        .iter()
        .map(plain)
        .collect();
    assert_eq!(texts.last().unwrap(), "  ⎿  Interrupted");
}

#[test]
fn the_transcript_expands_agent_groups_and_notices() {
    use crate::agents::AgentStatus;
    let mut app = App::new();
    app.history
        .push(HistoryItem::AgentGroup(crate::app::AgentGroup {
            background: false,
            agents: vec![agent_entry("a1", "Fetch Warsaw", AgentStatus::Done)],
            timestamp: String::new(),
        }));
    app.history
        .push(HistoryItem::AgentNotice(crate::app::AgentNotice {
            id: "a1".into(),
            description: "Fetch Warsaw".into(),
            status: AgentStatus::Done,
            secs: 35,
            result: "19°C".into(),
            timestamp: String::new(),
        }));
    let texts: Vec<String> = transcript_lines(&app, 100).iter().map(plain).collect();
    assert!(texts.contains(&"● Agent(Fetch Warsaw)".to_string()));
    assert!(
        texts
            .iter()
            .any(|t| t == "● Agent \"Fetch Warsaw\" finished · 35s")
    );
}

#[test]
fn the_footer_roster_lists_main_and_the_agents() {
    let mut app = App::new();
    app.begin_stream();
    app.start_agent_group(
        false,
        &[crate::stream::AgentSpec {
            id: "a1".into(),
            description: "Fetch current weather and time in Warsaw".into(),
            agent_type: "general-purpose".into(),
            prompt: "warsaw?".into(),
            background: false,
        }],
    );
    app.set_agent_runtime("a1", Duration::from_secs(48));
    assert_eq!(agent_list_rows(&app), 3, "blank + main + one agent");
    let texts: Vec<String> = agent_list_lines(&app, 100).iter().map(plain).collect();
    assert_eq!(texts[0], "");
    assert_eq!(texts[1], "  ● main");
    assert!(
        texts[2].starts_with("  ◯ general-purpose  Fetch current weather and time in Warsaw"),
        "{}",
        texts[2]
    );
    assert!(texts[2].ends_with(" 48s"), "{}", texts[2]);
    // A narrow width truncates the description, never the suffix.
    let narrow: Vec<String> = agent_list_lines(&app, 46).iter().map(plain).collect();
    assert!(narrow[2].contains('…'), "{}", narrow[2]);
    assert!(narrow[2].ends_with(" 48s"), "{}", narrow[2]);
}

#[test]
fn the_agent_view_swaps_the_strip_to_the_agents_stream() {
    let mut app = App::new();
    app.set_session_info("dummy_model_name", "~/repo");
    app.begin_stream();
    app.start_agent_group(
        false,
        &[crate::stream::AgentSpec {
            id: "a1".into(),
            description: "Fetch Warsaw".into(),
            agent_type: "general-purpose".into(),
            prompt: "warsaw?".into(),
            background: false,
        }],
    );
    app.apply_agent_event(
        "a1",
        &crate::stream::StreamEvent::ToolStart {
            name: "Bash".into(),
            args: "curl wttr.in".into(),
            detail: None,
        },
    );
    app.open_agent_view("a1");
    // The preview previews the AGENT's running tool, not the main group.
    let area = Rect::new(0, 0, 80, 24);
    let mut buf = Buffer::empty(area);
    render_live(area, &mut buf, &app);
    let all: String = (0..24).map(|y| row(&buf, y, 80) + "\n").collect();
    assert!(all.contains("Bash(curl wttr.in)"), "{all}");
    assert!(
        all.contains(" Fetch Warsaw "),
        "the box rule carries the label: {all}"
    );
    assert!(
        all.contains("❯ ◯ "),
        "the roster marks the viewed agent: {all}"
    );
}

// ===== The Agent tool's cells + roster (docs/agent-tool.md) =====

fn agent_entry(
    id: &str,
    desc: &str,
    status: crate::agents::AgentStatus,
) -> crate::app::AgentGroupEntry {
    crate::app::AgentGroupEntry {
        id: id.to_string(),
        description: desc.to_string(),
        agent_type: "general-purpose".to_string(),
        prompt: format!("What is the weather in {desc}?"),
        status,
        tool_uses: 2,
        tokens: 16_100,
        secs: 39,
        result: "It is 19°C.".to_string(),
        tool_headers: vec!["Bash(curl wttr.in)".to_string()],
        output: "It is 19°C.".to_string(),
    }
}
