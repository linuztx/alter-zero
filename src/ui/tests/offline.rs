//! The lost-connection row under the status line (`docs/offline.md`): the
//! ripple, the host, the count, and the strip reserving exactly the rows it
//! paints.

use super::*;
use crate::app::OfflineInfo;
use crate::ui::offline::ripple_spans;
use crate::ui::theme::{
    OFFLINE_FRAME_INTERVAL, OFFLINE_FRAMES, TOOL_RESULT_PREFIX, status_offline_color,
    tool_dim_color,
};
use crate::ui::wrap::cols;

fn outage(host: &str, attempts: u32) -> OfflineInfo {
    OfflineInfo {
        host: host.into(),
        attempts,
        began: Duration::ZERO,
    }
}

/// The conversation view's live height for `app` at `width`, the way the
/// boundary asks it.
fn height(app: &App, width: u16) -> u16 {
    live_height(
        &app.input,
        width,
        24,
        strip_has_status(app),
        0,
        hang_rows(app, width),
        0,
        0,
        0,
        0,
        0,
    )
}

/// The RGB distance between two colours — how far a ring has faded.
fn distance(a: Color, b: Color) -> i32 {
    match (a, b) {
        (Color::Rgb(r0, g0, b0), Color::Rgb(r1, g1, b1)) => {
            (i32::from(r0) - i32::from(r1)).abs()
                + (i32::from(g0) - i32::from(g1)).abs()
                + (i32::from(b0) - i32::from(b1)).abs()
        }
        other => panic!("expected two RGB colours, got {other:?}"),
    }
}

#[test]
fn the_row_hangs_in_the_gutter_with_the_ripple_the_host_and_the_count() {
    let lines = offline_lines(&outage("api.venice.ai", 14), Duration::ZERO, 120);
    let texts: Vec<String> = lines.iter().map(plain).collect();
    assert_eq!(
        texts,
        vec![format!(
            "{TOOL_RESULT_PREFIX}{}  No connection to api.venice.ai — trying again · 14 attempts",
            OFFLINE_FRAMES[0]
        )]
    );
    let one = offline_lines(&outage("127.0.0.1:11434", 1), Duration::ZERO, 120);
    assert!(
        plain(&one[0]).ends_with("No connection to 127.0.0.1:11434 — trying again · 1 attempt"),
        "one attempt reads singular: {:?}",
        plain(&one[0])
    );
}

#[test]
fn every_ripple_frame_is_the_same_width_so_the_text_never_jitters() {
    let width = cols(OFFLINE_FRAMES[0]);
    assert!(width >= 5, "wide enough for three rings around the dot");
    for frame in OFFLINE_FRAMES {
        assert_eq!(cols(frame), width, "{frame:?}");
    }
}

#[test]
fn the_ripple_spreads_out_from_the_dot_and_back_one_frame_per_interval() {
    let frame = |i: u32| plain(&Line::from(ripple_spans(OFFLINE_FRAME_INTERVAL * i)));
    assert_eq!(frame(0).trim(), "·");
    assert_eq!(frame(1).trim(), "(·)");
    assert_eq!(frame(2).trim(), "((·))");
    assert_eq!(frame(3).trim(), "(((·)))");
    assert_eq!(frame(4).trim(), "((·))");
    assert_eq!(frame(5).trim(), "(·)");
    let n = u32::try_from(OFFLINE_FRAMES.len()).unwrap();
    assert_eq!(frame(n), frame(0), "the ripple loops");
}

#[test]
fn the_ripple_fades_from_the_amber_dot_outward() {
    // The dot is the warning amber whole; each ring out is a step further
    // toward the dim, so the widest frame reads as a fading signal rather
    // than a flat bracket soup.
    let spans = ripple_spans(OFFLINE_FRAME_INTERVAL * 3);
    assert_eq!(plain(&Line::from(spans.clone())).trim(), "(((·)))");
    let dot = spans
        .iter()
        .find(|s| s.content.contains('·'))
        .expect("the dot");
    assert_eq!(dot.style.fg, Some(status_offline_color()));
    let fade = |index: usize| distance(spans[index].style.fg.unwrap(), status_offline_color());
    assert!(
        fade(2) < fade(1) && fade(1) < fade(0),
        "rings fade with distance"
    );
    assert!(
        fade(0) < distance(tool_dim_color(), status_offline_color()),
        "even the outer ring is not plain dim"
    );
}

#[test]
fn the_text_wraps_under_its_own_column_and_never_past_the_width() {
    let lines = offline_lines(&outage("api.venice.ai", 2), Duration::ZERO, 52);
    assert!(
        lines.len() > 1,
        "the sentence does not fit one 52-column row"
    );
    assert_eq!(
        offline_lines(&outage("api.venice.ai", 2), Duration::ZERO, 80).len(),
        1,
        "and fits one row of an 80-column terminal beside an ordinary host"
    );
    let indent = cols(TOOL_RESULT_PREFIX) + cols(OFFLINE_FRAMES[0]) + 2;
    for line in &lines {
        assert!(cols(&plain(line)) <= 52, "{:?}", plain(line));
    }
    for line in &lines[1..] {
        let text = plain(line);
        assert!(
            text.starts_with(&" ".repeat(indent)) && !text[indent..].starts_with(' '),
            "a continuation row aligns under the text column: {text:?}"
        );
    }
}

#[test]
fn the_host_is_lit_in_amber_and_the_rest_of_the_row_is_dim() {
    let lines = offline_lines(&outage("api.venice.ai", 2), Duration::ZERO, 120);
    let host = lines[0]
        .spans
        .iter()
        .find(|s| s.content == "api.venice.ai")
        .expect("the host as its own span");
    assert_eq!(host.style.fg, Some(status_offline_color()));
    for span in lines[0]
        .spans
        .iter()
        .filter(|s| s.content.contains("No connection") || s.content.contains("trying again"))
    {
        assert_eq!(span.style.fg, Some(tool_dim_color()), "{:?}", span.content);
    }
}

#[test]
fn the_strip_reserves_the_row_and_paints_it_under_the_status_line() {
    let mut app = App::new();
    app.record_user_message("go");
    app.begin_stream();
    app.set_status_times(Duration::from_secs(5), None);
    assert_eq!(offline_rows(&app, 80), 0);
    let idle = height(&app, 80);
    app.set_offline("api.venice.ai", 3);
    assert_eq!(offline_rows(&app, 80), 1);
    assert_eq!(
        hang_rows(&app, 80),
        1,
        "the row is the strip's one hanging row"
    );
    let h = height(&app, 80);
    assert_eq!(h, idle + 1, "the region grows by the row it paints");
    let mut buf = buffer(80, h);
    render_live(buf.area, &mut buf, &app);
    let rows: Vec<String> = (0..h).map(|y| row(&buf, y, 80)).collect();
    assert!(
        rows[0].contains("Waiting for internet…") && rows[0].contains("offline for 0s"),
        "the status line waits: {:?}",
        rows[0]
    );
    assert!(
        rows[1].contains("No connection to api.venice.ai"),
        "the row hangs right under it: {:?}",
        rows[1]
    );
    assert!(
        rows[2].trim().is_empty(),
        "then the status gap: {:?}",
        rows[2]
    );
    // The request gets through: the row goes and the region settles back.
    app.push_chunk("back");
    assert_eq!(offline_rows(&app, 80), 0);
    assert_eq!(height(&app, 80), idle);
}

#[test]
fn an_agent_views_row_is_the_viewed_agents_not_the_main_turns() {
    let mut app = App::new();
    app.begin_stream();
    app.start_agent_group(
        false,
        &[
            super::spec("a1", "Fetch Warsaw", false),
            super::spec("a2", "Fetch Berlin", false),
        ],
    );
    app.set_offline("main.example", 1);
    app.apply_agent_event(
        "a1",
        &crate::stream::StreamEvent::Offline {
            host: "agent.example".into(),
            attempts: 2,
        },
    );
    app.open_agent_view("a1");
    assert_eq!(offline_rows(&app, 100), 1);
    let lines = crate::ui::offline::status_offline_lines(&app, 100);
    assert!(
        plain(&lines[0]).contains("No connection to agent.example"),
        "{:?}",
        plain(&lines[0])
    );
    app.open_agent_view("a2");
    assert_eq!(offline_rows(&app, 100), 0, "a2 is not waiting");
}
