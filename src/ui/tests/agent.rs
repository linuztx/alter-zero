//! Subagent rendering (`docs/agent-tool.md`).

use super::*;
use crate::ui::agent::{AgentCellView, agent_cell_lines, agent_group_full_lines};
use crate::ui::live::preview_lines;
use crate::ui::theme::{
    TOOL_BULLET, TOOL_PULSE_PERIOD, tool_dim_color, tool_fail_color, tool_ok_color,
    tool_output_color, tool_running_color,
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
        batch: None,
    };
    let lines = agent_group_lines(&group, 80);
    let texts: Vec<String> = lines.iter().map(plain).collect();
    assert_eq!(texts[0], "● 2 agents finished (ctrl+o to expand)");
    assert_eq!(texts[1], "   ├ Fetch Warsaw · 2 tool uses · 16.1k tokens");
    assert_eq!(texts[2], "   │ ⎿  Done");
    assert_eq!(texts[3], "   └ Fetch Manila · 2 tool uses · 16.1k tokens");
    assert_eq!(texts[4], "     ⎿  Done");
    // All clean → green bullet; one interrupted → red.
    assert_eq!(lines[0].spans[0].style.fg, Some(tool_ok_color()));
    let mut stopped = group.clone();
    stopped.agents[1].status = AgentStatus::Interrupted;
    let lines = agent_group_lines(&stopped, 80);
    assert_eq!(lines[0].spans[0].style.fg, Some(tool_fail_color()));
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
        batch: None,
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
            .all(|s| s.style.fg == Some(tool_dim_color())),
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
    // A description-less call wears the clean `Name: args` shape, and a long
    // one **clips at the width** rather than wrapping the cell open under a
    // live counter — all of it dim (`docs/agent-tool.md`).
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
        texts[1].starts_with("  ⎿  Bash: sleep 10 && curl"),
        "{}",
        texts[1]
    );
    assert!(texts[1].ends_with('…'), "the cut is marked: {}", texts[1]);
    assert!(cols(&texts[1]) <= 44, "clipped at the width: {}", texts[1]);
    assert!(
        lines[1]
            .spans
            .iter()
            .all(|s| s.style.fg == Some(tool_dim_color())),
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
        batch: None,
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
        batch: None,
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
    // No description on the call → the clean `Name: args` shape.
    assert!(texts[4].ends_with("⎿  Write: game.py"), "{}", texts[4]);
}

#[test]
fn a_live_agent_groups_bullet_blinks_like_a_running_tool() {
    // The tree header is the round's "this is happening now" bullet, so it
    // blinks on the same clock as a running tool cell — the two are on screen
    // together in a mixed round and must not blink out of step
    // (`docs/tool-pulse.md`): shown in the resting grey, then hidden behind
    // blanks of its own width so the header text never shifts.
    let mut app = App::new();
    app.begin_stream();
    app.start_agent_group(
        false,
        &[
            spec("a1", "Fetch Warsaw", false),
            spec("a2", "Fetch Oslo", false),
        ],
    );
    let bullet = |app: &App| {
        let line = live_agent_group_lines(app, 90).swap_remove(0);
        (
            line.spans[0].content.to_string(),
            line.spans[0].style.fg,
            plain(&line),
        )
    };
    let (glyph, color, text) = bullet(&app);
    assert_eq!(glyph, TOOL_BULLET);
    assert_eq!(color, Some(tool_running_color()));
    assert!(text.starts_with("● Running 2 agents…"), "{text:?}");
    app.set_pulse(TOOL_PULSE_PERIOD / 2);
    let (glyph, _, text) = bullet(&app);
    assert_eq!(glyph, " ".repeat(cols(TOOL_BULLET)), "hidden at the half");
    assert!(text.starts_with("  Running 2 agents…"), "{text:?}");
}

#[test]
fn agent_cell_lines_expand_prompt_response_and_done() {
    use crate::agents::AgentStatus;
    let group = crate::app::AgentGroup {
        background: false,
        agents: vec![agent_entry("a1", "Fetch Warsaw", AgentStatus::Done)],
        timestamp: String::new(),
        batch: None,
    };
    let texts: Vec<String> = agent_group_full_lines(&group, 100, &PathDisplay::VERBATIM)
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
    let texts: Vec<String> = agent_group_full_lines(&stopped, 100, &PathDisplay::VERBATIM)
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
            batch: None,
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
            call_id: None,
            arguments: None,
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
            call_id: None,
            arguments: None,
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
    // The label is *embedded in* the top rule — `─ Fetch Warsaw ─`, the rule
    // resuming for one cell after the text so it reads as part of the frame
    // rather than dangling off its end (docs/agent-tool.md).
    let rule_row = (0..24)
        .map(|y| row(&buf, y, 80))
        .find(|r| r.contains("Fetch Warsaw") && r.contains('─'))
        .expect("the box rule carries the label");
    assert!(
        rule_row.trim_end().ends_with("─ Fetch Warsaw ─"),
        "the rule closes after the label: {rule_row}"
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
    assert_eq!(lines[1].spans[1].style.fg, Some(tool_dim_color()));
    assert_eq!(lines[2].spans[1].style.fg, Some(tool_output_color()));
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
        Some(tool_ok_color()),
        "a done agent's bullet turns green"
    );
    // Selecting the row keeps the verdict colour on the bullet.
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    app.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    app.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    let lines = agent_list_lines(&app, 100);
    assert!(plain(&lines[2]).starts_with("❯ ◯ general-purpose"));
    assert_eq!(lines[2].spans[1].style.fg, Some(tool_ok_color()));
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
        Some(tool_fail_color()),
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
        call_id: None,
        position: None,
        arguments: None,
    }
}

// ===== The agent session view's streaming strip (docs/agent-view-streaming.md) =====

/// An app with agent `a1` in its own session view, mid-reply on `text`.
fn streaming_agent_view(text: &str) -> App {
    let mut app = App::new();
    app.begin_stream();
    app.start_agent_group(false, &[spec("a1", "Table demo", false)]);
    app.apply_agent_event("a1", &crate::stream::StreamEvent::Chunk(text.to_string()));
    app.open_agent_view("a1");
    app
}

/// A markdown table caught mid-stream — the reported case.
const FORMING_TABLE: &str = "\
| Language | Year | Typing |
|----------|------|--------|
| Python | 1991 | Dynamic |
| Rust | 2010 | Static |
| Go";

#[test]
fn an_agent_views_strip_previews_the_rows_its_commits_withheld() {
    // The reported bug (docs/agent-view-streaming.md): the view's commits go
    // through a `StreamRender`, which withholds a forming table WHOLE — so
    // the strip must show that whole block, not the one row a batch render
    // happened to end on. `committed ++ preview` is the reply, in this view
    // exactly as in the main one (CLAUDE.md invariant 2).
    let width = 44;
    let mut app = streaming_agent_view(FORMING_TABLE);
    let text = app
        .agent("a1")
        .and_then(|run| run.streaming.clone())
        .expect("the agent is mid-reply");

    let mut render = StreamRender::new();
    let committed = render.commit(&text, width);
    let preview = render.preview(&text, width, 20);
    assert!(
        committed.is_empty(),
        "the open table is withheld whole: {committed:?}"
    );
    assert!(preview.len() > 1, "the forming grid is many rows");

    // What the boundary injects each frame, so the strip reserves the rows it
    // is about to draw (`App::set_stream_preview_rows`).
    app.set_stream_preview_rows(u16::try_from(preview.len()).unwrap());
    let lines = preview_lines(&app, width, Some(&preview), preview_rows(&app, width));
    let texts: Vec<String> = lines.iter().map(plain).collect();
    assert_eq!(lines.len(), preview.len(), "the whole frontier: {texts:?}");
    // What shipped before, pinned as the contrast: the batch-render fallback
    // keeps ONE row — the block's closing border — so every row above it was
    // on screen nowhere.
    assert_eq!(
        preview_lines(&app, width, None, preview_rows(&app, width)).len(),
        1,
        "the fallback is the old one-row render"
    );
    assert!(
        texts.iter().any(|t| t.contains("Python")),
        "the grid's rows show, not just its border: {texts:?}"
    );
    // …and the strip reserves exactly what it draws (the debug_assert).
    app_with_injected_rows(app, preview.len(), width);
}

/// `preview_rows` must report the injected count for a viewed agent, the way
/// it does for the main session — split out so the assertion reads once.
fn app_with_injected_rows(mut app: App, rows: usize, width: u16) {
    app.set_stream_preview_rows(u16::try_from(rows).unwrap());
    assert_eq!(usize::from(preview_rows(&app, width)), rows);
}

#[test]
fn an_agent_views_running_command_tails_its_streamed_output() {
    // Main parity (docs/tool-streaming.md): a subagent's running `bash` call
    // tails its output in the strip — the newest lines plus the
    // `+N lines (Ns)` footer — instead of the plain `⎿ Running…` peek.
    use crate::stream::StreamEvent;
    let mut app = App::new();
    app.begin_stream();
    app.start_agent_group(false, &[spec("a1", "Ping", false)]);
    app.apply_agent_event(
        "a1",
        &StreamEvent::ToolStart {
            name: "Bash".into(),
            args: "ping -c 10 x".into(),
            detail: None,
            arguments: None,
        },
    );
    for i in 1..=9 {
        app.apply_agent_event("a1", &StreamEvent::ToolOutput(format!("line {i}\n")));
    }
    // The footer's `(Ns)` is the **command's** own clock — injected per
    // agent like the thinking phase's (`set_agent_command_elapsed`) — never
    // the agent's whole runtime, which its status line shows: a call an
    // agent starts a minute in used to open on `+N lines (60s)`.
    app.set_agent_runtime("a1", Duration::from_secs(60));
    app.set_agent_command_elapsed("a1", Duration::from_secs(9));
    app.open_agent_view("a1");
    let lines = preview_lines(&app, 60, None, preview_rows(&app, 60));
    let all: String = lines.iter().map(|l| plain(l) + "\n").collect();
    assert!(all.contains("line 9"), "the newest line tails: {all:?}");
    assert!(!all.contains("line 4"), "older lines are hidden: {all:?}");
    assert!(
        all.contains("+5 lines (9s · wait 2m)"),
        "the footer counts the command's own runtime, against its timeout: {all:?}"
    );
    assert_eq!(usize::from(preview_rows(&app, 60)), lines.len());
    let run = app.agent("a1").expect("listed");
    assert_eq!(
        agent_view_status(run).elapsed,
        Duration::from_secs(60),
        "the status line still carries the agent's runtime"
    );
}

/// The verb pair the agent session view's status line wears for `id`.
fn agent_view_verbs(app: &App, id: &str) -> (&'static str, &'static str) {
    let status = agent_view_status(app.agent(id).expect("listed"));
    (status.verb, status.done_verb)
}

/// The summary the agent's last settle recorded on its own transcript.
fn agent_summary(app: &App, id: &str) -> crate::app::TurnSummary {
    match app.agent(id).expect("listed").history.last() {
        Some(HistoryItem::Summary(summary)) => summary.clone(),
        other => panic!("the settle records a summary: {other:?}"),
    }
}

#[test]
fn an_agent_views_status_verb_rotates_with_its_runtime() {
    // The session view's line walks the status verbs on the agent's own
    // clock — the main line's rule (docs/status-indicator.md).
    let mut app = App::new();
    app.begin_stream();
    app.start_agent_group(false, &[spec("a1", "Rotate", false)]);
    assert_eq!(
        agent_view_verbs(&app, "a1"),
        ("Working", "Worked"),
        "an agent opens on the first verb"
    );
    app.set_agent_runtime("a1", Duration::from_millis(29_999));
    assert_eq!(agent_view_verbs(&app, "a1").0, "Working");
    app.set_agent_runtime("a1", Duration::from_secs(30));
    assert_eq!(agent_view_verbs(&app, "a1"), ("Generating", "Generated"));
}

#[test]
fn an_agent_turns_summary_names_the_verb_its_view_wore() {
    use crate::stream::StreamEvent;
    let mut app = App::new();
    app.begin_stream();
    app.start_agent_group(false, &[spec("a1", "Rotate", false)]);
    app.set_agent_runtime("a1", Duration::from_secs(45)); // `Generating…`
    app.apply_agent_event("a1", &StreamEvent::Chunk("It is 19°C.".into()));
    app.apply_agent_event("a1", &StreamEvent::StreamDone);
    let summary = agent_summary(&app, "a1");
    assert_eq!(
        summary.verb, "Generated",
        "the line last said `Generating…`"
    );
    assert_eq!(summary.secs, 45);
}

#[test]
fn an_events_runtime_freeze_never_moves_the_agents_verb() {
    // The boundary pins the runtime as each event lands, so a settle records
    // its exact elapsed — but only a drawn frame moves the verb: a turn that
    // ends a few milliseconds past a rotation no frame drew still names the
    // verb its view showed.
    use crate::stream::StreamEvent;
    let mut app = App::new();
    app.begin_stream();
    app.start_agent_group(false, &[spec("a1", "Rotate", false)]);
    app.set_agent_runtime("a1", Duration::from_millis(29_990)); // the last frame
    app.freeze_agent_runtime("a1", Duration::from_millis(30_010)); // StreamDone lands
    app.apply_agent_event("a1", &StreamEvent::StreamDone);
    let summary = agent_summary(&app, "a1");
    assert_eq!(
        summary.verb, "Worked",
        "the view never showed `Generating…`"
    );
    assert_eq!(
        summary.secs, 30,
        "the settle still records its exact elapsed"
    );
}

#[test]
fn an_agents_chat_continuation_opens_on_the_verb_after_the_last_one_shown() {
    use crate::stream::StreamEvent;
    let mut app = App::new();
    app.begin_stream();
    app.start_agent_group(false, &[spec("a1", "Rotate", false)]);
    app.set_agent_runtime("a1", Duration::from_secs(45));
    app.apply_agent_event("a1", &StreamEvent::StreamDone);
    app.agent_chat("a1", "and in Manila?");
    assert_eq!(
        agent_view_verbs(&app, "a1").0,
        "Pondering",
        "never the verb the summary above it just named"
    );
}

#[test]
fn an_agent_views_strip_previews_its_thinking_block() {
    // A reasoning subagent shows the same live `● Thinking…` block the main
    // view does, and its status line says `Thinking for Ns`
    // (docs/thinking-stream.md, docs/agent-view-streaming.md).
    use crate::stream::StreamEvent;
    let mut app = App::new();
    app.begin_stream();
    app.start_agent_group(false, &[spec("a1", "Think", false)]);
    app.begin_agent_reasoning("a1");
    app.apply_agent_event("a1", &StreamEvent::ThinkingChunk("weighing options".into()));
    app.set_agent_thinking("a1", Duration::from_secs(3));
    app.open_agent_view("a1");
    let lines = preview_lines(&app, 60, None, preview_rows(&app, 60));
    let texts: Vec<String> = lines.iter().map(plain).collect();
    assert!(texts[0].starts_with("● Thinking…"), "{texts:?}");
    assert!(
        texts.iter().any(|t| t.contains("weighing options")),
        "the chain-of-thought tails: {texts:?}"
    );
    assert_eq!(usize::from(preview_rows(&app, 60)), lines.len());
    let run = app.agent("a1").expect("listed");
    assert_eq!(
        agent_view_status(run).thinking,
        Some(Duration::from_secs(3)),
        "the strip's status carries the phase's elapsed"
    );
}

#[test]
fn an_agent_views_strip_shows_every_waiting_sibling_of_a_parallel_batch() {
    // The main strip renders a parallel batch as the running call over each
    // dim `⎿ Waiting…` sibling, blank-separated (docs/parallel-tools.md).
    // The agent session view is the same picture over the agent's own queue:
    // its `ToolBatch` fills `AgentRun::tool_queue` exactly as the main turn's
    // fills `App::tool_queue`, and `agent_view_preview_lines` walks it through
    // the shared `live_call_lines`.
    let mut app = App::new();
    app.begin_stream();
    app.start_agent_group(false, &[spec("a1", "Parallel demo", false)]);
    app.apply_agent_event(
        "a1",
        &crate::stream::StreamEvent::ToolBatch(vec![
            crate::stream::ToolCallSummary {
                name: "Bash".into(),
                args: "echo AAA".into(),
            },
            crate::stream::ToolCallSummary {
                name: "Bash".into(),
                args: "echo BBB".into(),
            },
            crate::stream::ToolCallSummary {
                name: "Bash".into(),
                args: "echo CCC".into(),
            },
        ]),
    );
    app.open_agent_view("a1");
    let width = 60;
    let lines: Vec<String> = preview_lines(&app, width, None, preview_rows(&app, width))
        .iter()
        .map(plain)
        .collect();
    assert_eq!(
        lines,
        vec![
            "● Bash(echo AAA)".to_string(),
            "  ⎿  Waiting…".to_string(),
            String::new(),
            "● Bash(echo BBB)".to_string(),
            "  ⎿  Waiting…".to_string(),
            String::new(),
            "● Bash(echo CCC)".to_string(),
            "  ⎿  Waiting…".to_string(),
        ],
        "the whole batch previews, blank-separated"
    );
    // The reserved rows and the painted rows must agree (the strip's
    // `debug_assert`), for the agent branch as for the main one.
    assert_eq!(
        usize::from(preview_rows(&app, width)),
        lines.len(),
        "preview_rows sizes from the same walk"
    );
}

#[test]
fn an_agent_views_running_call_leads_its_waiting_siblings() {
    // Sequential execution inside the agent, exactly as in the main view:
    // only the front call ever runs, the rest keep waiting.
    let mut app = App::new();
    app.begin_stream();
    app.start_agent_group(false, &[spec("a1", "Parallel demo", false)]);
    app.apply_agent_event(
        "a1",
        &crate::stream::StreamEvent::ToolBatch(vec![
            crate::stream::ToolCallSummary {
                name: "Bash".into(),
                args: "echo AAA".into(),
            },
            crate::stream::ToolCallSummary {
                name: "Read".into(),
                args: "notes.md".into(),
            },
        ]),
    );
    app.apply_agent_event(
        "a1",
        &crate::stream::StreamEvent::ToolStart {
            name: "Bash".into(),
            args: "echo AAA".into(),
            detail: None,
            arguments: None,
        },
    );
    app.open_agent_view("a1");
    let lines: Vec<String> = preview_lines(&app, 60, None, preview_rows(&app, 60))
        .iter()
        .map(plain)
        .collect();
    assert_eq!(lines[0], "● Bash(echo AAA)");
    // The agent's running command carries the same clock clause the main
    // strip's does — zero here, no per-agent command clock injected — over
    // the timeout it runs under (docs/tool-streaming.md).
    assert_eq!(lines[1], "  ⎿  Running… (0s · wait 2m)", "{lines:?}");
    assert_eq!(lines[3], "● Read(notes.md)");
    assert_eq!(lines[4], "  ⎿  Waiting…", "{lines:?}");
}

#[test]
fn an_agents_ctrl_o_cell_lists_only_the_calls_it_ran() {
    // The cell's `tool_headers` carry no status of their own
    // (`docs/agent-tool.md`: "the nested tool headers the agent ran"), so a
    // not-yet-started `⎿ Waiting…` sibling listed there reads as a call the
    // agent ran while it may yet never run. The running call still belongs:
    // what it is doing now is part of what it has done.
    let mut app = App::new();
    app.begin_stream();
    app.start_agent_group(false, &[spec("a1", "Batch demo", false)]);
    app.apply_agent_event(
        "a1",
        &crate::stream::StreamEvent::ToolBatch(vec![
            crate::stream::ToolCallSummary {
                name: "Bash".to_string(),
                args: "echo running".to_string(),
            },
            crate::stream::ToolCallSummary {
                name: "Bash".to_string(),
                args: "echo waiting".to_string(),
            },
        ]),
    );
    app.apply_agent_event(
        "a1",
        &crate::stream::StreamEvent::ToolStart {
            name: "Bash".to_string(),
            args: "echo running".to_string(),
            detail: None,
            arguments: None,
        },
    );
    let run = app.agent("a1").expect("the roster entry");
    let cell = crate::ui::agent::AgentCellView::of_run(run);
    assert_eq!(
        cell.tool_headers,
        vec!["Bash(echo running)".to_string()],
        "only the running call is listed"
    );
}

#[test]
fn a_long_agent_description_is_clipped_so_the_composer_rule_survives() {
    // The reported bug: a wordy `description` filled the whole top rule, and
    // ratatui's right-aligned title skids an over-wide line off its LEFT end,
    // so the frame read as a sentence with a stray `─` — the head of the
    // description gone and no rule at all. The label is clipped to at most
    // half the rule now, closed with `…` (docs/agent-tool.md).
    let mut app = App::new();
    app.set_session_info("dummy_model_name", "~/repo");
    app.begin_stream();
    app.start_agent_group(
        false,
        &[crate::stream::AgentSpec {
            id: "a1".into(),
            description: "An agent tasked with confirming its status and \
                          acknowledging the requested description length"
                .into(),
            agent_type: "general-purpose".into(),
            prompt: "status?".into(),
            background: false,
            call_id: None,
            arguments: None,
        }],
    );
    app.open_agent_view("a1");
    let area = Rect::new(0, 0, 76, 24);
    let mut buf = Buffer::empty(area);
    render_live(area, &mut buf, &app);
    // The composer's top rule is the row directly above the `❯` prompt.
    let rows: Vec<String> = (0..24).map(|y| row(&buf, y, 76)).collect();
    let prompt = rows
        .iter()
        .position(|r| r.starts_with('❯'))
        .expect("the composer prompt");
    // Half the 76-column rule stays rule; the label keeps the description's
    // HEAD, closes with `…`, and the tail glyph still shuts the frame.
    assert_eq!(
        rows[prompt - 1].trim_end(),
        format!("{} An agent tasked with confirming it… ─", "─".repeat(38))
    );
}

#[test]
fn the_composer_label_keeps_a_short_description_whole_and_gives_up_on_a_narrow_rule() {
    use crate::ui::agent::agent_view_rule_label;
    let long = "An agent tasked with confirming its status and acknowledging \
                the requested description length";
    // Inside the budget nothing is cut — the ordinary case, padded for the rule.
    assert_eq!(
        agent_view_rule_label("Fetch Warsaw", 80).as_deref(),
        Some(" Fetch Warsaw ")
    );
    // A rule wide enough for the whole thing shows the whole thing.
    assert_eq!(
        agent_view_rule_label(long, 400).as_deref(),
        Some(format!(" {long} ").as_str())
    );
    // Past the budget the HEAD is kept (what the agent is for), closed with
    // `…`, and the whole label — its padding and the rule tail counted in —
    // stays inside half the rule at every width.
    for width in [12u16, 24, 40, 60, 76, 80] {
        let label = agent_view_rule_label(long, width).expect("wide enough for a label");
        assert!(
            long.starts_with(label.trim().trim_end_matches('…')),
            "clipped from the end at {width}: {label:?}"
        );
        assert!(
            label.ends_with("… "),
            "the cut is marked at {width}: {label:?}"
        );
        assert!(
            cols(&label) + cols("─") <= usize::from(width) / 2,
            "at most half the rule at {width}: {label:?}"
        );
    }
    // A rule with no room for the mark AND one real character beside it shows
    // no label at all: a lone ` … ─` names nothing, and the frame is worth
    // more than the hint.
    assert_eq!(agent_view_rule_label(long, 9), None);
    assert_eq!(agent_view_rule_label(long, 8), None);
    assert_eq!(agent_view_rule_label(long, 6), None);
    assert_eq!(agent_view_rule_label(long, 0), None);
    // …but a description that *fits* still rides at that width: the mark is
    // what needs the room, so there is nothing to spend it on when nothing is
    // cut (width 8 → a one-column budget).
    assert_eq!(agent_view_rule_label("x", 8).as_deref(), Some(" x "));
}

#[test]
fn the_composer_label_is_a_lit_chip_in_the_themes_accent() {
    // The user-requested fill: the agent session view's `── Greet user ─`
    // label rides the top rule as a **chip** — the theme's accent under the
    // on-accent ink, the ↓-focused footer chip's dress — so the one row that
    // says *which* conversation the composer feeds reads at a glance rather
    // than as dim text embedded in a dim rule. The fill covers the label's
    // own padding spaces and nothing else: the rule before it and the tail
    // glyph after it keep the border colour and no ground (docs/agent-tool.md).
    use crate::app::Theme;
    use crate::ui::palette::{palette_of, with_theme};
    use crate::ui::theme::{agent_view_label_bg, agent_view_label_fg, border_color};
    let mut app = App::new();
    app.set_session_info("dummy_model_name", "~/repo");
    app.begin_stream();
    app.start_agent_group(false, &[spec("a1", "Greet user", false)]);
    app.open_agent_view("a1");
    let area = Rect::new(0, 0, 76, 24);
    let render = |buf: &mut Buffer| {
        render_live(area, buf, &app);
        let rows: Vec<String> = (0..24).map(|y| row(buf, y, 76)).collect();
        let prompt = rows
            .iter()
            .position(|r| r.starts_with('❯'))
            .expect("the composer prompt");
        let y = u16::try_from(prompt - 1).expect("a small row");
        assert_eq!(
            rows[usize::from(y)].trim_end(),
            format!("{} Greet user ─", "─".repeat(63)),
            "the row's text is what it always was"
        );
        y
    };
    let mut buf = Buffer::empty(area);
    let y = render(&mut buf);
    // 63 rule cells, the 12-column ` Greet user ` label, the tail glyph.
    for x in 63..75u16 {
        let cell = &buf[(x, y)];
        assert_eq!(cell.bg, agent_view_label_bg(), "the fill at {x}: {cell:?}");
        assert_eq!(cell.fg, agent_view_label_fg(), "the ink at {x}: {cell:?}");
    }
    for x in (0..63u16).chain(75..76) {
        let cell = &buf[(x, y)];
        assert_eq!(cell.symbol(), "─", "a rule cell at {x}");
        assert_eq!(cell.fg, border_color(), "the rule's colour at {x}");
        assert_eq!(cell.bg, Color::Reset, "no fill outside the label at {x}");
    }
    // The chip is the *active* theme's — Mocha's sky under its crust here —
    // and every theme in the catalog paints it from its own table once it
    // is active, ANSI's named cyan and black included, with an ink that is
    // never the fill: a colour of the chip's own would be a colour that
    // ignores the theme.
    let mocha = palette_of(Theme::Mocha);
    assert_eq!(buf[(63, y)].bg, mocha.accent);
    assert_eq!(buf[(63, y)].fg, mocha.on_accent);
    for theme in Theme::ALL {
        with_theme(theme, || {
            let mut buf = Buffer::empty(area);
            let y = render(&mut buf);
            let p = palette_of(theme);
            let cell = &buf[(63, y)];
            assert_eq!(cell.bg, p.accent, "{theme:?}: the fill is the accent");
            assert_eq!(cell.fg, p.on_accent, "{theme:?}: the ink is on_accent");
            assert_ne!(cell.fg, cell.bg, "{theme:?}: the ink must show on the fill");
            assert_eq!(
                buf[(62, y)].bg,
                Color::Reset,
                "{theme:?}: the rule stays bare"
            );
        });
    }
}

// ===== The file tools' path display on the agent surfaces (docs/tools.md) =====

/// The worked example's session policy: launched in `~/Codes/tests`.
fn session_paths() -> PathDisplay {
    PathDisplay::new(
        "/home/linuztx/Codes/tests",
        Some(std::path::PathBuf::from("/home/linuztx")),
    )
}

#[test]
fn a_live_agents_file_activity_row_shortens_its_path() {
    // The sticky `Write: {path}` row reads by the same rule the cell's
    // header does — `~/hello.py`, never the record's `/home/linuztx/…` — on
    // the lone cell and on every tree row alike. The run's own
    // `last_activity` keeps the verbatim path.
    let mut app = App::new();
    app.set_path_display(session_paths());
    app.begin_stream();
    app.start_agent_group(false, &[spec("a1", "Write the notes", false)]);
    app.apply_agent_event(
        "a1",
        &crate::stream::StreamEvent::ToolStart {
            name: "Write".into(),
            args: "/home/linuztx/hello.py".into(),
            detail: None,
            arguments: None,
        },
    );
    let texts: Vec<String> = live_agent_group_lines(&app, 80).iter().map(plain).collect();
    assert_eq!(texts[1], "  ⎿  Write: ~/hello.py", "{texts:?}");
    assert_eq!(
        app.agent("a1").unwrap().activity(),
        "Write: /home/linuztx/hello.py",
        "the record keeps the absolute path"
    );

    let mut app = App::new();
    app.set_path_display(session_paths());
    app.begin_stream();
    app.start_agent_group(
        false,
        &[
            spec("a1", "Write the notes", false),
            spec("a2", "Check the tests", false),
        ],
    );
    app.apply_agent_event(
        "a1",
        &crate::stream::StreamEvent::ToolStart {
            name: "Read".into(),
            args: "/home/linuztx/Codes/tests/src/app.rs".into(),
            detail: None,
            arguments: None,
        },
    );
    // A bash description is prose, not a path — untouched even when it
    // names one.
    app.apply_agent_event(
        "a2",
        &crate::stream::StreamEvent::ToolStart {
            name: "Bash".into(),
            args: "cat /home/linuztx/Codes/tests/x".into(),
            detail: Some("Reading /home/linuztx/Codes/tests/x".into()),
            arguments: None,
        },
    );
    let texts: Vec<String> = live_agent_group_lines(&app, 100)
        .iter()
        .map(plain)
        .collect();
    assert!(
        texts.iter().any(|t| t.contains("Read: src/app.rs")),
        "the tree row shortens the read's path: {texts:?}"
    );
    assert!(
        texts
            .iter()
            .any(|t| t.contains("Bash: Reading /home/linuztx/Codes/tests/x")),
        "a description passes through: {texts:?}"
    );
}

#[test]
fn the_transcripts_nested_agent_headers_shorten_their_paths() {
    // The Ctrl+O expansion lists the nested calls an agent ran as
    // `Name(args)` headers: a file tool's shortens like the main
    // transcript's cell header, every other tool's stays as recorded.
    let entry = crate::app::AgentGroupEntry {
        id: "a1".to_string(),
        description: "Write the notes".to_string(),
        agent_type: "general-purpose".to_string(),
        prompt: "Write the notes".to_string(),
        status: crate::agents::AgentStatus::Done,
        tool_uses: 3,
        tokens: 1_000,
        secs: 9,
        result: "Done.".to_string(),
        tool_headers: vec![
            "Write(/home/linuztx/Codes/tests/notes.md)".to_string(),
            "Edit(/home/linuztx/notes.md)".to_string(),
            "Bash(cat /home/linuztx/Codes/tests/notes.md)".to_string(),
        ],
        output: "Done.".to_string(),
        call_id: None,
        position: None,
        arguments: None,
    };
    let texts: Vec<String> = agent_cell_lines(
        &AgentCellView::of_entry(&entry, false),
        80,
        &session_paths(),
    )
    .iter()
    .map(plain)
    .collect();
    assert!(
        texts.iter().any(|t| t.trim() == "Write(notes.md)"),
        "{texts:?}"
    );
    assert!(
        texts.iter().any(|t| t.trim() == "Edit(~/notes.md)"),
        "{texts:?}"
    );
    assert!(
        texts
            .iter()
            .any(|t| t.trim() == "Bash(cat /home/linuztx/Codes/tests/notes.md)"),
        "{texts:?}"
    );
    // The verbatim policy renders the record exactly.
    let texts: Vec<String> = agent_cell_lines(
        &AgentCellView::of_entry(&entry, false),
        80,
        &PathDisplay::VERBATIM,
    )
    .iter()
    .map(plain)
    .collect();
    assert!(
        texts
            .iter()
            .any(|t| t.trim() == "Write(/home/linuztx/Codes/tests/notes.md)"),
        "{texts:?}"
    );
}

#[test]
fn a_live_agent_cells_nested_headers_shorten_from_the_record() {
    // The transcript tail builds a live agent's cell from its own tool
    // calls — the record, not a formatted string — and shows a file tool's
    // path by the session's rule like the committed entry does.
    let mut app = App::new();
    app.set_path_display(session_paths());
    app.begin_stream();
    app.start_agent_group(false, &[spec("a1", "Write the notes", false)]);
    app.apply_agent_event(
        "a1",
        &crate::stream::StreamEvent::ToolStart {
            name: "Write".into(),
            args: "/home/linuztx/notes.md".into(),
            detail: None,
            arguments: None,
        },
    );
    app.apply_agent_event(
        "a1",
        &crate::stream::StreamEvent::ToolEnd {
            output: "Wrote 1 line to ../../notes.md\n1 x".into(),
            ok: true,
            truncated: false,
        },
    );
    app.apply_agent_event(
        "a1",
        &crate::stream::StreamEvent::ToolStart {
            name: "Read".into(),
            args: "/home/linuztx/Codes/tests/a.rs".into(),
            detail: None,
            arguments: None,
        },
    );
    let run = app.agent("a1").expect("the run is on the roster");
    let texts: Vec<String> = agent_cell_lines(&AgentCellView::of_run(run), 80, app.path_display())
        .iter()
        .map(plain)
        .collect();
    assert!(
        texts.iter().any(|t| t.trim() == "Write(~/notes.md)"),
        "{texts:?}"
    );
    assert!(
        texts.iter().any(|t| t.trim() == "Read(a.rs)"),
        "the running call too: {texts:?}"
    );
}
