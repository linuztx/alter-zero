//! Performance probe for link rendering (NOT part of the app) — `docs/links.md`.
//!
//! Renders a link-heavy reply (labelled links, bare URLs, a table of links)
//! the two ways the app does — whole, as a resize or the Ctrl+O transcript
//! renders a finished message, and streamed, a commit and a preview per
//! chunk — in both presentations of a labelled link (`label (url)`, and the
//! label alone), beside the same reply with its links taken out. It also
//! counts what the paint boundary is then handed: the rows, the cells inside
//! a hyperlink, the hyperlink runs and the escape bytes those runs cost on
//! the wire.
//!
//! Run:  cargo run --release --example link_probe

use std::time::{Duration, Instant};

use alter_zero::app::Role;
use alter_zero::links::{self, TargetDisplay};
use alter_zero::ui::{self, StreamRender};
use ratatui::text::Line;
use unicode_width::UnicodeWidthStr;

/// Paragraphs (and table rows) in the generated reply.
const ITEMS: usize = 300;

/// Characters per streamed chunk — a realistic multi-token delta.
const CHUNK: usize = 20;

/// Timed repetitions; the best one is reported, which is the run the machine
/// interfered with least.
const REPS: u32 = 15;

/// A reply dense with links: every paragraph holds two labelled links and a
/// bare URL, and a table of labelled links closes it.
fn link_reply(items: usize) -> String {
    let mut s = String::new();
    for i in 0..items {
        s.push_str(&format!(
            "Source {i}: [FIDE world championship match {i}]\
             (https://www.fide.com/geneva-to-host-fide-world-championship-match-2026/?item={i}) \
             and the [official documentation](https://example.com/docs/section/{i}/index.html#anchor), \
             see also https://example.org/bare/{i}.\n\n"
        ));
    }
    s.push_str("| Name | Link |\n| --- | --- |\n");
    for i in 0..items {
        s.push_str(&format!(
            "| item {i} | [entry {i}](https://example.com/table/entry/{i}?ref=probe) |\n"
        ));
    }
    s
}

/// The same reply with nothing to link: the labels as plain words, no URLs.
fn plain_reply(items: usize) -> String {
    let mut s = String::new();
    for i in 0..items {
        s.push_str(&format!(
            "Source {i}: FIDE world championship match {i} \
             and the official documentation, see also the bare item {i}.\n\n"
        ));
    }
    s.push_str("| Name | Link |\n| --- | --- |\n");
    for i in 0..items {
        s.push_str(&format!("| item {i} | entry {i} |\n"));
    }
    s
}

/// The best of [`REPS`] runs of `f`.
fn best(mut f: impl FnMut() -> Duration) -> Duration {
    (0..REPS).map(|_| f()).min().unwrap_or_default()
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1e3
}

/// One whole-message render.
fn whole(reply: &str, width: u16) -> Duration {
    let start = Instant::now();
    let lines = ui::message_lines(Role::Assistant, reply, width);
    let elapsed = start.elapsed();
    std::hint::black_box(lines);
    elapsed
}

/// One streamed render — a commit and a preview per chunk — as the total and
/// the slowest single step.
fn streamed(reply: &str, width: u16) -> (Duration, Duration) {
    let chars: Vec<char> = reply.chars().collect();
    let mut render = StreamRender::new();
    let mut buf = String::with_capacity(reply.len());
    let (mut total, mut worst) = (Duration::ZERO, Duration::ZERO);
    for chunk in chars.chunks(CHUNK) {
        buf.extend(chunk);
        let start = Instant::now();
        std::hint::black_box(render.commit(&buf, width));
        std::hint::black_box(render.preview(&buf, width, usize::MAX));
        let step = start.elapsed();
        total += step;
        worst = worst.max(step);
    }
    (total, worst)
}

/// What the paint boundary is handed for `lines`: the rows, the cells that
/// sit inside a hyperlink, the hyperlink runs (a run is what one escape pair
/// brackets — consecutive cells of one target on one row) and the bytes of
/// escape those runs cost.
struct Painted {
    rows: usize,
    cells: usize,
    runs: usize,
    escape_bytes: usize,
}

fn painted(lines: &[Line<'static>]) -> Painted {
    let mut out = Painted {
        rows: lines.len(),
        cells: 0,
        runs: 0,
        escape_bytes: 0,
    };
    for line in lines {
        let mut open: Option<std::sync::Arc<str>> = None;
        for span in &line.spans {
            let target = links::style_link(&span.style);
            if let Some(url) = &target {
                out.cells += span.content.width();
                if open.as_ref() != Some(url) {
                    out.runs += 1;
                    out.escape_bytes += links::osc8_open(1, url).len() + links::OSC8_CLOSE.len();
                }
            }
            open = target;
        }
    }
    out
}

/// Measure `reply` under `display` — the per-thread choice the app makes
/// once at startup from the terminal it finds itself in.
fn report(name: &str, reply: &str, display: TargetDisplay) {
    links::with_target_display(display, || measure(name, reply));
}

fn measure(name: &str, reply: &str) {
    println!("--- {name} ({} bytes) ---", reply.len());
    println!(
        "{:>5}  {:>10}  {:>11}  {:>10}  {:>7}  {:>8}  {:>6}  {:>9}",
        "width", "whole", "streamed", "worst step", "rows", "linked", "runs", "escapes"
    );
    for width in [40u16, 80, 120] {
        // One untimed pass first: the first sighting of each URL interns it.
        let lines = ui::message_lines(Role::Assistant, reply, width);
        let p = painted(&lines);
        let whole = best(|| whole(reply, width));
        let mut worst = Duration::MAX;
        let total = best(|| {
            let (total, step) = streamed(reply, width);
            worst = worst.min(step);
            total
        });
        println!(
            "{width:>5}  {:>7.2} ms  {:>8.2} ms  {:>7.3} ms  {:>7}  {:>8}  {:>6}  {:>7} B",
            ms(whole),
            ms(total),
            ms(worst),
            p.rows,
            p.cells,
            p.runs,
            p.escape_bytes,
        );
    }
    println!();
}

fn main() {
    println!(
        "link_probe: {ITEMS} paragraphs + a {ITEMS}-row table, best of {REPS}; \
         streamed in {CHUNK}-character chunks\n"
    );
    let links = link_reply(ITEMS);
    report("links, address shown", &links, TargetDisplay::Shown);
    report("links, label only", &links, TargetDisplay::Hidden);
    report(
        "no links (control)",
        &plain_reply(ITEMS),
        TargetDisplay::Shown,
    );
}
