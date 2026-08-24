//! Subagent rendering (`docs/agent-tool.md`).

use super::*;
use crate::ui::agent::agent_group_full_lines;
use crate::ui::theme::{
    TOOL_DIM_COLOR, TOOL_FAIL_COLOR, TOOL_OK_COLOR, TOOL_OUTPUT_COLOR, TOOL_PULSE_BRIGHT,
    TOOL_PULSE_DIM, TOOL_PULSE_PERIOD,
};
use crate::ui::wrap::cols;

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
    assert_eq!(
        texts[0],
        "● 2 background agents launched (↓ to manage · ctrl+o to expand)"
    );
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
    // A running tool shows the **same one dim activity row** the tree rows do
    // — the model's own description when it gave one — never a white wrapped
    // `Bash(...)` header over a `Running…` row (`docs/agent-tool.md`).
    app.apply_agent_event(
        "a1",
        &crate::stream::StreamEvent::ToolStart {
            name: "Bash".into(),
            args: "sleep 10 && curl -s https://api.open-meteo.com/v1/forecast".into(),
            detail: Some("Fetching Warsaw weather".into()),
            arguments: None,
        },
    );
    let lines = live_agent_group_lines(&app, 44);
    let texts: Vec<String> = lines.iter().map(plain).collect();
    assert_eq!(texts[0], "● Agent(Fetch Warsaw)");
    assert_eq!(texts[1], "  ⎿  Bash: Fetching Warsaw weather");
    assert_eq!(texts.len(), 2, "one row for the state: {texts:?}");
    assert!(
        lines[1]
            .spans
            .iter()
            .all(|s| s.style.fg == Some(TOOL_DIM_COLOR)),
        "the whole row is dim: {:?}",
        lines[1]
    );
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
fn a_lone_live_agents_tool_row_clips_instead_of_wrapping() {
    // A description-less call wears the tool cell's own `Name(args)` shape,
    // and a long one **clips at the width** rather than wrapping the cell
    // open under a live counter — all of it dim (`docs/agent-tool.md`).
    let mut app = App::new();
    app.begin_stream();
    app.start_agent_group(false, &[spec("a1", "Fetch Warsaw", false)]);
    app.apply_agent_event(
        "a1",
        &crate::stream::StreamEvent::ToolStart {
            name: "Bash".into(),
            args: "sleep 10 && curl -s https://api.open-meteo.com/v1/forecast?latitude=52".into(),
            detail: None,
            arguments: None,
        },
    );
    let lines = live_agent_group_lines(&app, 44);
    let texts: Vec<String> = lines.iter().map(plain).collect();
    assert_eq!(texts.len(), 2, "the header + one clipped row: {texts:?}");
    assert!(
        texts[1].starts_with("  ⎿  Bash(sleep 10 && curl"),
        "{}",
        texts[1]
    );
    assert!(texts[1].ends_with('…'), "the cut is marked: {}", texts[1]);
    assert!(cols(&texts[1]) <= 44, "clipped at the width: {}", texts[1]);
    assert!(
        lines[1]
            .spans
            .iter()
            .all(|s| s.style.fg == Some(TOOL_DIM_COLOR)),
        "the whole row is dim: {:?}",
        lines[1]
    );
    assert_eq!(usize::from(preview_rows(&app, 44)), texts.len());
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
    // A lone background launch keeps the manage row instead — its trailing
    // ctrl+o hint teaches that the launch cell expands in the transcript too.
    let mut launched = group;
    launched.background = true;
    launched.agents[0].status = AgentStatus::Running;
    let texts: Vec<String> = agent_group_lines(&launched, 80).iter().map(plain).collect();
    assert_eq!(texts[0], "● Agent(Fetch current weather in Warsaw)");
    assert_eq!(
        texts[1],
        "  ⎿  Running in the background (↓ to manage · ctrl+o to expand)"
    );
    assert_eq!(
        texts.len(),
        2,
        "no separate expand row on the backgrounded cell"
    );
}

#[test]
fn agent_durations_humanize_past_a_minute() {
    // The done clause and the completion notice both read `6m 2s`, never a
    // bare `362s` — the status line's format_elapsed everywhere a runtime
    // shows (the user-report fix).
    use crate::agents::AgentStatus;
    let mut entry = agent_entry("a1", "Count 1-100 with sleep", AgentStatus::Done);
    entry.secs = 362;
    let group = crate::app::AgentGroup {
        background: false,
        agents: vec![entry],
        timestamp: String::new(),
    };
    let texts: Vec<String> = agent_group_lines(&group, 80).iter().map(plain).collect();
    assert_eq!(texts[1], "  ⎿  Done (2 tool uses · 16.1k tokens · 6m 2s)");
    let notice = crate::app::AgentNotice {
        id: "a1".into(),
        description: "Count 1-100 with sleep".into(),
        status: AgentStatus::Done,
        secs: 362,
        result: String::new(),
        timestamp: String::new(),
    };
    let texts: Vec<String> = agent_notice_lines(&notice, 100).iter().map(plain).collect();
    assert_eq!(
        texts[0],
        "● Agent \"Count 1-100 with sleep\" finished · 6m 2s"
    );
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
            arguments: None,
        },
    );
    for event in [
        crate::stream::StreamEvent::ToolStart {
            name: "Write".into(),
            args: "game.py".into(),
            detail: None,
            arguments: None,
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
    // No description on the call → the tool cell's own `Name(args)` shape.
    assert!(texts[4].ends_with("⎿  Write(game.py)"), "{}", texts[4]);
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
            arguments: None,
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
        all.contains("  ● general-purpose"),
        "the roster's filled bullet marks the viewed agent: {all}"
    );
    assert!(
        all.contains("  ◯ main"),
        "main demotes to the dim ring while an agent view is open: {all}"
    );
}

#[test]
fn the_roster_highlight_follows_the_viewed_session_and_the_marker_the_selection() {
    // The user-report fix (docs/agent-tool.md): the filled `●` + bright row
    // mark the SESSION IN VIEW — inside an agent's session view the highlight
    // sits on that agent, not on `main` — and the `❯` belongs to the active
    // ↑/↓ selection alone, leaving with the focus when Enter/Esc hand the
    // keys back to the composer.
    let mut app = App::new();
    app.begin_stream();
    app.start_agent_group(
        false,
        &[spec("a1", "Count 1 to 100 with 1 second sleep", false)],
    );
    // Main session in view: `● main` white-bold, the agent row a dim `◯`,
    // and — with no selection active — no `❯` anywhere.
    let texts: Vec<String> = agent_list_lines(&app, 100).iter().map(plain).collect();
    assert_eq!(texts[1], "  ● main");
    assert!(texts[2].starts_with("  ◯ general-purpose"), "{}", texts[2]);
    assert!(
        !texts.iter().any(|t| t.contains('❯')),
        "no selection marker at rest: {texts:?}"
    );
    // Inside the agent's session view the highlight moves with it.
    app.open_agent_view("a1");
    let lines = agent_list_lines(&app, 100);
    let texts: Vec<String> = lines.iter().map(plain).collect();
    assert_eq!(texts[1], "  ◯ main", "main loses the filled bullet");
    assert!(
        texts[2].starts_with("  ● general-purpose"),
        "the viewed agent gains it: {}",
        texts[2]
    );
    assert!(
        !texts.iter().any(|t| t.contains('❯')),
        "entering the view returns the keys to the composer — the ❯ leaves \
         with the selection: {texts:?}"
    );
    // The viewed row is the bright one; main is dim.
    assert_eq!(lines[1].spans[1].style.fg, Some(TOOL_DIM_COLOR));
    assert_eq!(lines[2].spans[1].style.fg, Some(TOOL_OUTPUT_COLOR));
    // An active ↓ selection still shows its ❯ (and cyan) on the selected row
    // — inside the view the first ↓ lands straight on the viewed agent (the
    // remembered pick), and a second ↓ has nowhere further to go.
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    app.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    app.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(app.agent_selection(), Some(1));
    let texts: Vec<String> = agent_list_lines(&app, 100).iter().map(plain).collect();
    assert!(texts[2].starts_with("❯ ● general-purpose"), "{}", texts[2]);
}

#[test]
fn a_finished_roster_rows_bullet_is_green_for_its_linger() {
    // The linger window (docs/agent-tool.md) exists so the row can be *read*
    // — a finished agent's `◯` wears the tool green (red for a stop) while
    // it waits out the sweep, and the green wins even over the selection.
    let mut app = App::new();
    app.begin_stream();
    app.start_agent_group(false, &[spec("a1", "Fetch Warsaw", false)]);
    app.apply_agent_event("a1", &crate::stream::StreamEvent::Chunk("19°C".into()));
    app.apply_agent_event("a1", &crate::stream::StreamEvent::StreamDone);
    let lines = agent_list_lines(&app, 100);
    assert!(plain(&lines[2]).starts_with("  ◯ general-purpose"));
    assert_eq!(
        lines[2].spans[1].style.fg,
        Some(TOOL_OK_COLOR),
        "a done agent's bullet turns green"
    );
    // Selecting the row keeps the verdict colour on the bullet.
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    app.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    app.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    let lines = agent_list_lines(&app, 100);
    assert!(plain(&lines[2]).starts_with("❯ ◯ general-purpose"));
    assert_eq!(lines[2].spans[1].style.fg, Some(TOOL_OK_COLOR));
    // An x-stopped sibling wears the fail red instead.
    let mut app = App::new();
    app.begin_stream();
    app.start_agent_group(false, &[spec("a1", "Fetch Warsaw", false)]);
    assert!(matches!(
        app.stop_agent("a1"),
        Some(crate::app::AgentStop::Stopped(_))
    ));
    let lines = agent_list_lines(&app, 100);
    assert_eq!(
        lines[2].spans[1].style.fg,
        Some(TOOL_FAIL_COLOR),
        "a stopped agent's bullet turns red"
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
