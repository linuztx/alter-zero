//! Memory probe for what a **turn** costs the process while a pasted picture
//! stays in the conversation (NOT part of the app; `docs/memory.md`).
//!
//! Every request re-sends the whole context, so an attachment pasted once
//! rides every later turn: the backend thread used to encode it into a
//! `data:` URL, the payload builder to copy that into the request, and the
//! body to be serialized whole and handed to the transport thread — several
//! picture-sized allocations per round, on a fresh thread per turn. This
//! probe runs the request path with no network, one turn at a time, and
//! prints the resident set after each, so a per-turn residue shows as a
//! climbing `RSS` column and a transient one as a flat `RSS` under a high
//! `peak`.
//!
//! Run:  cargo run --release --example image_turn_probe [WxH] [turns] [rounds]
//!
//! `WxH` is the screenshot-shaped PNG generated for the run (1920x1200 by
//! default, a gradient with per-pixel noise so it compresses like a real
//! one), `turns` how many follow-up turns to send after the one that carries
//! the paste, and `rounds` how many requests each turn makes (a turn that
//! calls tools makes one per round). `--stage encode|messages|payload|body`
//! stops the chain early to attribute the cost (`payload` builds the JSON
//! tree the tests read instead of the body, to show what that tree costs).
//!
//! Linux-only reporting (reads `/proc/self/status`); elsewhere it prints n/a.

use std::io::Read;
use std::path::{Path, PathBuf};

use alter_zero::context::{ContextMessage, ContextRole};
use alter_zero::images::attachment_data_url;
use alter_zero::llm::ModelConfig;
use alter_zero::llm::backend::build_messages_for;
use alter_zero::llm::openai::OpenAiClient;

/// A `/proc/self/status` field in KiB.
fn vm_kb(field: &str) -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|l| l.starts_with(field))?;
    line.split_whitespace().nth(1)?.parse().ok()
}

fn mb(kb: u64) -> f64 {
    kb as f64 / 1024.0
}

fn report(label: &str) {
    let rss = vm_kb("VmRSS:").unwrap_or(0);
    let hwm = vm_kb("VmHWM:").unwrap_or(0);
    println!(
        "{label:<20} RSS {:>6.1} MB   peak {:>6.1} MB",
        mb(rss),
        mb(hwm)
    );
}

/// The picture `clipboard_owner` serves: a gradient with per-pixel noise,
/// PNG-encoded fast, so it weighs what a screenshot weighs.
fn screenshot_png(width: u32, height: u32) -> Vec<u8> {
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    let mut seed: u32 = 0x9E37_79B9;
    for y in 0..height {
        for x in 0..width {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            let noise = (seed & 0x1F) as u8;
            let fx = (x * 255 / width.max(1)) as u8;
            let fy = (y * 255 / height.max(1)) as u8;
            rgba.extend_from_slice(&[
                fx.saturating_add(noise),
                fy.saturating_add(noise),
                200u8.saturating_sub(fx).saturating_add(noise),
                255,
            ]);
        }
    }
    let mut png = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(png::Compression::Fast);
        let mut writer = encoder.write_header().expect("a PNG header");
        writer.write_image_data(&rgba).expect("the picture");
    }
    png
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Stage {
    Encode,
    Messages,
    Payload,
    Body,
    All,
}

/// One request as the backend makes it: the messages built on the turn's
/// thread, the round's copy handed to the client, and the body streamed out
/// on a transport thread in the small reads `reqwest` makes.
fn round(client: &OpenAiClient, context: &[ContextMessage], stage: Stage) {
    if stage == Stage::Encode {
        for message in context {
            for image in &message.images {
                std::hint::black_box(attachment_data_url(image));
            }
        }
        return;
    }
    let messages = build_messages_for(
        None,
        Some("You are a terminal agent."),
        "",
        context,
        attachment_data_url,
    );
    if stage == Stage::Messages {
        return;
    }
    // `stream_chat` takes the messages by value: the agent loop hands each
    // round its own copy.
    let copy = messages.to_vec();
    if stage == Stage::Payload {
        // The tree form the tests read — never what the wire sends now.
        std::hint::black_box(client.build_payload(&copy));
        return;
    }
    let (mut body, _) = client.request_stream(copy).expect("streams");
    let mut pump = move || {
        let mut buf = [0u8; 8192];
        while let Ok(n) = body.read(&mut buf) {
            if n == 0 {
                break;
            }
        }
    };
    if stage == Stage::Body {
        pump();
        return;
    }
    std::thread::spawn(pump).join().expect("transport");
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let stage = match args.iter().position(|a| a == "--stage") {
        Some(at) => {
            args.remove(at);
            match args.remove(at).as_str() {
                "encode" => Stage::Encode,
                "messages" => Stage::Messages,
                "payload" => Stage::Payload,
                "body" => Stage::Body,
                other => panic!("unknown stage {other}"),
            }
        }
        None => Stage::All,
    };
    let size = args
        .first()
        .cloned()
        .unwrap_or_else(|| "1920x1200".to_string());
    let (w, h) = size.split_once('x').expect("WxH");
    let (width, height): (u32, u32) = (w.parse().expect("width"), h.parse().expect("height"));
    let turns: usize = args.get(1).and_then(|a| a.parse().ok()).unwrap_or(6);
    let rounds: usize = args.get(2).and_then(|a| a.parse().ok()).unwrap_or(1);

    let dir = tempfile::tempdir().expect("tempdir");
    let path: PathBuf = dir.path().join("1.png");
    std::fs::write(&path, screenshot_png(width, height)).expect("write");
    let bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    println!(
        "picture {width}x{height} ({:.1} MB on disk), {turns} follow-up turns × {rounds} round(s)",
        bytes as f64 / (1024.0 * 1024.0)
    );
    let mut cfg = ModelConfig::fallback();
    cfg.model = "probe-model".to_string();
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
    report("startup");
    for turn in 0..=turns {
        let snapshot = context.clone();
        let client = client.clone();
        // A turn runs on its own thread, like the backend's.
        std::thread::spawn(move || {
            for _ in 0..rounds {
                round(&client, &snapshot, stage);
            }
        })
        .join()
        .expect("turn");
        report(&if turn == 0 {
            "send image".to_string()
        } else {
            format!("followup {turn}")
        });
        context.push(ContextMessage::new(ContextRole::Assistant, "OK"));
        context.push(ContextMessage::new(
            ContextRole::User,
            format!("Reply with the single word OK ({}).", turn + 1),
        ));
    }
    let _ = Path::new(&path);
}
