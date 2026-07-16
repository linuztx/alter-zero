//! Performance probe for the streaming-render hot path (NOT part of the app).
//!
//! Contrasts the OLD whole-reply re-render (what `stable_commit`/the preview did
//! every chunk/frame — O(reply) each, so O(reply²) over a stream) against the NEW
//! incremental `ui::StreamRender` (O(new text) per call). Part B drives a real
//! OpenRouter stream when `OPENROUTER_API_KEY` is set.
//!
//! Run:  cargo run --release --example perf_probe

use std::time::{Duration, Instant};

use inline_tui::app::Role;
use inline_tui::ui::{self, StreamRender};

const WIDTH: u16 = 100;

/// A realistic large fenced code block — the worst case for the render path
/// (markdown block parse + per-line syntax highlight + verbatim wrap).
fn gen_code_reply(code_lines: usize) -> String {
    let mut s = String::from("Here is the program you asked for:\n\n```python\n");
    for i in 0..code_lines {
        s.push_str(&format!("def function_{i}(a, b, c):  # step {i}\n"));
        s.push_str(&format!(
            "    result = a * {i} + b - c   # arithmetic on inputs\n"
        ));
        s.push_str("    values = [x for x in range(10) if x % 2 == 0]\n");
        s.push_str(&format!("    return result + sum(values) + {i}\n"));
    }
    s.push_str("```\n\nThat's the code you asked for. Let me know if you need changes.\n");
    s
}

/// OLD per-chunk / per-frame cost: re-render the whole reply from scratch
/// (`message_lines` = what `stable_commit` and the strip preview both did).
fn time_naive_whole_render(buf: &str) -> Duration {
    let start = Instant::now();
    let _ = ui::message_lines(Role::Assistant, buf, WIDTH);
    start.elapsed()
}

fn avg<F: FnMut() -> Duration>(mut f: F, reps: u32) -> Duration {
    let mut t = Duration::ZERO;
    for _ in 0..reps {
        t += f();
    }
    t / reps
}

fn part_a() {
    println!("=== Part A: synthetic large code reply (offline) ===");
    println!("width = {WIDTH} cols\n");
    println!(
        "{:>6}  {:>10}  {:>16}  {:>16}",
        "code", "reply", "OLD whole-render", "NEW incremental"
    );
    println!(
        "{:>6}  {:>10}  {:>16}  {:>16}",
        "lines", "bytes", "(per chunk/frame)", "commit + preview"
    );
    println!("{}", "-".repeat(56));

    for &lines in &[50usize, 100, 200, 400, 800, 1200, 1600] {
        let reply = gen_code_reply(lines);
        let old = avg(|| time_naive_whole_render(&reply), 5);
        // NEW: one incremental commit + one preview at this size (the renderer is
        // primed to the buffer, so this measures the steady-state per-call cost).
        let new = avg(
            || {
                let mut r = StreamRender::new();
                // Prime with everything but the last 40 bytes, then time the
                // marginal commit + preview the loop actually pays per step.
                let split = reply.len().saturating_sub(40);
                let split = (0..=split)
                    .rev()
                    .find(|&i| reply.is_char_boundary(i))
                    .unwrap();
                let _ = r.commit(&reply[..split], WIDTH);
                let start = Instant::now();
                let _ = r.commit(&reply, WIDTH);
                let _ = r.preview(&reply, WIDTH, usize::MAX);
                start.elapsed()
            },
            5,
        );
        println!(
            "{:>6}  {:>10}  {:>13.2} ms  {:>13.3} ms",
            lines,
            reply.len(),
            old.as_secs_f64() * 1e3,
            new.as_secs_f64() * 1e3,
        );
    }

    // Cumulative cost of streaming a whole reply, OLD vs NEW.
    let cum_lines = 400usize;
    let reply = gen_code_reply(cum_lines);
    let delta = 20; // chars per streamed chunk (a realistic multi-token SSE delta)
    let chars: Vec<char> = reply.chars().collect();

    // NEW: StreamRender across the whole stream.
    let mut render = StreamRender::new();
    let mut buf = String::new();
    let mut new_total = Duration::ZERO;
    let mut chunks = 0u64;
    let mut i = 0;
    while i < chars.len() {
        let end = (i + delta).min(chars.len());
        for c in &chars[i..end] {
            buf.push(*c);
        }
        i = end;
        let t = Instant::now();
        let _ = render.commit(&buf, WIDTH);
        let _ = render.preview(&buf, WIDTH, usize::MAX);
        new_total += t.elapsed();
        chunks += 1;
    }

    // OLD: re-render the whole buffer each chunk (commit + preview).
    let mut buf = String::new();
    let mut old_total = Duration::ZERO;
    let mut i = 0;
    while i < chars.len() {
        let end = (i + delta).min(chars.len());
        for c in &chars[i..end] {
            buf.push(*c);
        }
        i = end;
        let t = Instant::now();
        let _ = ui::message_lines(Role::Assistant, &buf, WIDTH); // commit re-render
        let _ = ui::message_lines(Role::Assistant, &buf, WIDTH); // preview re-render
        old_total += t.elapsed();
    }

    println!("\n-- cumulative cost of streaming one {cum_lines}-line reply ({chunks} chunks) --");
    println!(
        "OLD (whole-render each chunk):  {:>9.1} ms  of blocking main-loop CPU",
        old_total.as_secs_f64() * 1e3
    );
    println!(
        "NEW (incremental StreamRender): {:>9.1} ms  ({:.0}x less)",
        new_total.as_secs_f64() * 1e3,
        old_total.as_secs_f64() / new_total.as_secs_f64().max(1e-9),
    );

    // Per-frame preview budget: NEW must stay well under the 32ms status cadence.
    println!("\n-- single preview render vs the 32ms status-frame budget --");
    for &lines in &[200usize, 400, 800, 1200, 1600] {
        let reply = gen_code_reply(lines);
        let old = avg(|| time_naive_whole_render(&reply), 5);
        let new = avg(
            || {
                let mut r = StreamRender::new();
                let split = reply.len().saturating_sub(40);
                let split = (0..=split)
                    .rev()
                    .find(|&i| reply.is_char_boundary(i))
                    .unwrap();
                let _ = r.commit(&reply[..split], WIDTH);
                let start = Instant::now();
                let _ = r.preview(&reply, WIDTH, usize::MAX);
                start.elapsed()
            },
            5,
        );
        let flag = |d: Duration| {
            if d > Duration::from_millis(32) {
                " OVER 32ms"
            } else {
                ""
            }
        };
        println!(
            "{lines:>5} lines: OLD {:>7.2} ms{:<11}  NEW {:>7.3} ms{}",
            old.as_secs_f64() * 1e3,
            flag(old),
            new.as_secs_f64() * 1e3,
            flag(new),
        );
    }
}

fn part_b() {
    use inline_tui::llm::ModelConfig;
    use inline_tui::llm::openai::OpenAiClient;
    use inline_tui::stream::CancelToken;

    let Ok(key) = std::env::var("OPENROUTER_API_KEY") else {
        println!("\n=== Part B skipped (set OPENROUTER_API_KEY to run the live test) ===");
        return;
    };
    let model = std::env::var("PROBE_MODEL").unwrap_or_else(|_| "openai/gpt-4o-mini".to_string());
    println!("\n=== Part B: live OpenRouter stream ({model}) ===");

    let mut cfg = ModelConfig::fallback();
    cfg.provider_id = "openrouter".into();
    cfg.provider_name = "OpenRouter".into();
    cfg.model = model;
    cfg.api_base = "https://openrouter.ai/api/v1".into();
    cfg.api_model_base = "https://openrouter.ai/api/v1".into();
    cfg.api_key = Some(key);
    cfg.temperature = Some(0.3);

    let client = OpenAiClient::new(cfg);
    let cancel = CancelToken::new();
    let messages = vec![
        inline_tui::llm::ChatMessage::system(
            "You are a coding assistant. Reply with a single large fenced code block and minimal prose.",
        ),
        inline_tui::llm::ChatMessage::user(
            "Write a complete Python implementation of a terminal Snake game using curses, \
             with about 250 lines of well-commented code. Output only one ```python code block.",
        ),
    ];

    // Replicate the loop with the NEW StreamRender: commit per delta + preview.
    let mut render = StreamRender::new();
    let mut buf = String::new();
    let mut commit_total = Duration::ZERO;
    let mut preview_total = Duration::ZERO;
    let mut n_deltas = 0u64;
    let mut first10 = Duration::ZERO;
    let mut last10: Vec<Duration> = Vec::new();

    let wall_start = Instant::now();
    let result = client.stream_chat(messages, &cancel, |d| {
        if d.response.is_empty() {
            return;
        }
        buf.push_str(&d.response);
        n_deltas += 1;

        let t0 = Instant::now();
        let _ = render.commit(&buf, WIDTH);
        let commit = t0.elapsed();

        let t1 = Instant::now();
        let _ = render.preview(&buf, WIDTH, usize::MAX);
        let preview = t1.elapsed();

        commit_total += commit;
        preview_total += preview;
        if n_deltas <= 10 {
            first10 += commit + preview;
        }
        last10.push(commit + preview);
        if last10.len() > 10 {
            last10.remove(0);
        }
    });
    let wall = wall_start.elapsed();

    if let Err(e) = result {
        println!("stream error: {e}");
        return;
    }

    let render_total = commit_total + preview_total;
    let last10_sum: Duration = last10.iter().sum();
    println!("reply bytes:             {}", buf.len());
    println!("reply lines:             {}", buf.lines().count());
    println!("deltas:                  {n_deltas}");
    println!("wall clock:              {:.2} s", wall.as_secs_f64());
    println!(
        "render CPU (commit+preview): {:.1} ms  ({:.2}% of wall)",
        render_total.as_secs_f64() * 1e3,
        render_total.as_secs_f64() / wall.as_secs_f64() * 100.0
    );
    if n_deltas >= 20 {
        println!(
            "per-delta render: first 10 avg {:.4} ms  vs  last 10 avg {:.4} ms  (growth = {:.2}x)",
            first10.as_secs_f64() / 10.0 * 1e3,
            last10_sum.as_secs_f64() / last10.len() as f64 * 1e3,
            (last10_sum.as_secs_f64() / last10.len() as f64)
                / (first10.as_secs_f64() / 10.0).max(1e-9)
        );
    }
}

fn main() {
    part_a();
    part_b();
}
