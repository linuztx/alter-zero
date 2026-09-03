//! A turn that re-sends a pasted picture must cost the process **nothing
//! picture-sized** beyond the request body itself (`docs/memory.md`, *Every
//! turn re-sent the picture*).
//!
//! An attachment rides every later request, and each of those used to read
//! the file again, base64 it again, copy the string into a JSON tree and
//! serialize it into a body grown by doubling — five picture-sized blocks a
//! round, on a fresh thread. glibc's dynamic `mmap` threshold puts such
//! blocks on a thread arena's heap, which never shrinks, so the resident set
//! stepped up by the picture's weight whenever a turn landed on a new arena.
//! Now the encoding is built once and shared, and the body is serialized by
//! reference into a buffer sized exactly once — this test is what keeps it
//! that way — and the body itself is streamed through a pipe of small chunks
//! rather than built, so not even the request is a picture-sized block.
//!
//! Its own binary because the reading is process-wide
//! (`tests/model_parse_memory.rs`'s pattern); skips where `/proc` is absent.

use std::io::{Cursor, Read};
use std::path::PathBuf;

use alter_zero::context::{ContextMessage, ContextRole};
use alter_zero::images::attachment_data_url;
use alter_zero::llm::ModelConfig;
use alter_zero::llm::backend::build_messages_for;
use alter_zero::llm::openai::OpenAiClient;

fn vm_kb(field: &str) -> Option<usize> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find_map(|l| l.strip_prefix(field))?;
    line.trim().trim_end_matches(" kB").trim().parse().ok()
}

/// A screenshot-shaped PNG: a gradient with noise, so it weighs what a real
/// one weighs.
fn screenshot_png(width: u32, height: u32) -> Vec<u8> {
    let mut seed: u32 = 0x9E37_79B9;
    let image = image::RgbaImage::from_fn(width, height, |x, y| {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let noise = (seed >> 24) as u8 & 0x0f;
        image::Rgba([
            ((x * 255) / width) as u8 ^ noise,
            ((y * 255) / height) as u8 ^ noise,
            (((x + y) * 255) / (width + height)) as u8,
            255,
        ])
    });
    let mut out = Cursor::new(Vec::new());
    image
        .write_to(&mut out, image::ImageFormat::Png)
        .expect("encode");
    out.into_inner()
}

/// One request as the backend makes it: the messages built on the turn's
/// thread, the round's copy handed to the client, and the body streamed out
/// on a transport thread in the small reads `reqwest` makes.
fn turn(client: &OpenAiClient, context: &[ContextMessage]) -> usize {
    let messages = build_messages_for(
        None,
        Some("You are a terminal agent."),
        "",
        context,
        attachment_data_url,
    );
    let copy = messages.to_vec();
    let (mut body, len) = client.request_stream(copy).expect("streams");
    std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        while let Ok(n) = body.read(&mut buf) {
            if n == 0 {
                break;
            }
        }
    })
    .join()
    .expect("transport");
    len as usize
}

#[test]
fn later_turns_with_a_picture_in_context_cost_nothing_picture_sized() {
    if vm_kb("VmRSS:").is_none() {
        eprintln!("no /proc — skipping the resident-memory probe");
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let path: PathBuf = dir.path().join("1.png");
    std::fs::write(&path, screenshot_png(1920, 1200)).expect("write");
    let picture = std::fs::metadata(&path).expect("stat").len() as usize;

    let mut cfg = ModelConfig::fallback();
    cfg.model = "gate-model".to_string();
    let client = OpenAiClient::new(cfg);
    let mut context = vec![ContextMessage {
        role: ContextRole::User,
        text: format!(
            "[Image #1: {}] Reply with the single word OK.",
            path.display()
        ),
        images: vec![path.clone()],
        tool_calls: Vec::new(),
        tool_call_id: None,
    }];

    // The first turn pays for the one encoding the session keeps.
    let body = {
        let client = client.clone();
        let snapshot = context.clone();
        std::thread::spawn(move || turn(&client, &snapshot))
            .join()
            .expect("turn")
    };
    assert!(body > picture, "the body carries the picture: {body} bytes");
    let rss_after_first = vm_kb("VmRSS:").expect("rss");
    let peak_after_first = vm_kb("VmHWM:").expect("hwm");

    for n in 1..=6 {
        context.push(ContextMessage::new(ContextRole::Assistant, "OK"));
        context.push(ContextMessage::new(
            ContextRole::User,
            format!("Reply with the single word OK ({n})."),
        ));
        let client = client.clone();
        let snapshot = context.clone();
        std::thread::spawn(move || turn(&client, &snapshot))
            .join()
            .expect("turn");
    }
    let rss_growth = vm_kb("VmRSS:")
        .expect("rss")
        .saturating_sub(rss_after_first)
        * 1024;
    let peak_growth = vm_kb("VmHWM:")
        .expect("hwm")
        .saturating_sub(peak_after_first)
        * 1024;
    let slack = 2 * 1024 * 1024;
    assert!(
        rss_growth < slack,
        "six follow-up turns grew the resident set by {rss_growth} bytes against a \
         {picture}-byte picture — a turn is allocating picture-sized blocks again"
    );
    assert!(
        peak_growth < slack,
        "six follow-up turns raised the peak by {peak_growth} bytes: a later turn is \
         costing more than the first"
    );
}
