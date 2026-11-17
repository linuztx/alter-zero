//! Keystroke-to-paint latency of the real binary while a reply streams (NOT
//! part of the app, NOT run by `cargo test`).
//!
//! The probe is the terminal: it starts `alter-zero` in a pseudo-terminal of
//! its own, answers its startup questions through the session emulator
//! (`pty::screen::Screen`), and plays a provider on the loopback that streams
//! a reply of a chosen shape at a chosen pace. While that reply streams it
//! types into the composer at a typist's cadence and times each key from the
//! write that sent it to the frame that shows it — the cursor back on the
//! composer, right after the character.
//!
//! ```bash
//! cargo build --release && cargo run --release --example latency_probe
//! cargo run --release --example latency_probe -- --scenario code --rate 0
//! cargo run --release --example latency_probe -- --scenario all --json out.json
//! ```
//!
//! Per scenario it prints the key latency's distribution, how long the
//! stream took to reach the screen after the provider finished sending it,
//! the CPU the app's loop thread and the whole process spent, the frames and
//! bytes it wrote, and the process's resident memory at the end. `--rate 0`
//! is a provider that never pauses: more events than any model sends, which
//! is what shows whether the loop keeps its promises under load.
//!
//! Beyond the composer: `--overlay` times the Ctrl+O pager's scroll keys
//! under the stream instead, `--warmup N` streams N turns first so the
//! measured one has a history behind it, and `--draft N` (with the `idle`
//! scenario) types into a composer already holding N characters; the `burst`
//! scenario types `--draft` characters in one write and times the frame that
//! shows the last of them — or reports that none did. The `bash`
//! scenario runs a command that prints `--bytes` of output at `--rate` lines
//! a second, `write` streams a file of that size into a `write` call, and
//! `thought` thinks in one paragraph of that size, with no line break in it.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use alter_zero::pty::screen::Screen;
use alter_zero::pty::spawn::{PtyProcess, spawn};

/// The sentence every scenario's reply closes on — what "the whole stream is
/// on screen" is read off.
const END_MARK: &str = "END-OF-PROBE-STREAM";

/// The characters the probe types, in order. Neighbours always differ, so a
/// two-character suffix names one keystroke and no earlier state.
const KEYS: &str = "Q7Z3K9X5V1J8W4Y6";

/// What a draft and a burst are made of.
const FILLER: &str = "lorem ipsum dolor sit amet ";

/// What a burst closes on — the frame that shows it has shown all of it.
const BURST_END: &str = "ENDBURST";

/// How long any one wait may take before the probe gives up on the run.
const STEP_TIMEOUT: Duration = Duration::from_secs(60);

struct Options {
    bin: PathBuf,
    scenarios: Vec<String>,
    /// Stream frames per second; 0 never pauses.
    rate: u64,
    /// Characters per stream frame.
    chars: usize,
    /// Size of the streamed reply, in bytes.
    bytes: usize,
    /// Pause between typed keys.
    key_every: Duration,
    rows: u16,
    cols: u16,
    /// Characters already in the composer before the timed keys.
    draft: usize,
    /// Unpaced turns streamed before the measured one, to give it a history.
    warmup: usize,
    /// Time the Ctrl+O pager's scroll keys instead of the composer's.
    overlay: bool,
    json: Option<PathBuf>,
    keep: bool,
    env: Vec<(String, String)>,
}

fn usage() -> ! {
    eprintln!(
        "usage: latency_probe [--bin PATH] [--scenario NAME[,NAME…]|all] [--rate N] \
         [--chars N] [--bytes N] [--key-every MS] [--rows N] [--cols N] [--draft N] \
         [--warmup TURNS] [--overlay] [--env K=V] [--json PATH] [--keep]\n\
         scenarios: idle burst prose code table oneline mixed reasoning thought write bash"
    );
    std::process::exit(2);
}

fn parse_options() -> Options {
    let mut options = Options {
        bin: PathBuf::from("target/release/alter-zero"),
        scenarios: vec!["idle".into(), "prose".into(), "code".into()],
        rate: 400,
        chars: 4,
        bytes: 60_000,
        key_every: Duration::from_millis(60),
        rows: 40,
        cols: 120,
        draft: 0,
        warmup: 0,
        overlay: false,
        json: None,
        keep: false,
        env: Vec::new(),
    };
    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        let mut value = || args.next().unwrap_or_else(|| usage());
        match flag.as_str() {
            "--bin" => options.bin = PathBuf::from(value()),
            "--scenario" => {
                let names = value();
                options.scenarios = if names == "all" {
                    [
                        "idle",
                        "prose",
                        "code",
                        "table",
                        "oneline",
                        "mixed",
                        "reasoning",
                        "thought",
                        "write",
                        "bash",
                    ]
                    .iter()
                    .map(|name| (*name).to_string())
                    .collect()
                } else {
                    names.split(',').map(str::to_string).collect()
                };
            }
            "--rate" => options.rate = value().parse().unwrap_or_else(|_| usage()),
            "--chars" => options.chars = value().parse().unwrap_or_else(|_| usage()),
            "--bytes" => options.bytes = value().parse().unwrap_or_else(|_| usage()),
            "--key-every" => {
                options.key_every =
                    Duration::from_millis(value().parse().unwrap_or_else(|_| usage()));
            }
            "--rows" => options.rows = value().parse().unwrap_or_else(|_| usage()),
            "--cols" => options.cols = value().parse().unwrap_or_else(|_| usage()),
            "--draft" => options.draft = value().parse().unwrap_or_else(|_| usage()),
            "--warmup" => options.warmup = value().parse().unwrap_or_else(|_| usage()),
            "--overlay" => options.overlay = true,
            "--json" => options.json = Some(PathBuf::from(value())),
            "--keep" => options.keep = true,
            "--env" => {
                let pair = value();
                let (key, val) = pair.split_once('=').unwrap_or_else(|| usage());
                options.env.push((key.to_string(), val.to_string()));
            }
            _ => usage(),
        }
    }
    options
}

// ---------------------------------------------------------------------------
// The reply shapes.
// ---------------------------------------------------------------------------

/// What one request is answered with.
enum Reply {
    /// Reply text, streamed a few characters a frame.
    Text(String),
    /// Reasoning first, then reply text.
    Reasoned { thought: String, text: String },
    /// One tool call whose arguments stream a few characters a frame.
    Call {
        name: &'static str,
        arguments: String,
    },
}

const WORDS: &[&str] = &[
    "the", "stream", "keeps", "arriving", "while", "you", "type", "into", "a", "composer", "that",
    "must", "answer", "at", "once", "whatever", "the", "model", "is", "doing", "behind", "it",
    "and", "however", "fast", "its", "tokens", "land",
];

fn prose(bytes: usize) -> String {
    let mut text = String::new();
    let mut n = 0usize;
    while text.len() < bytes {
        for sentence in 0..5 {
            for word in 0..(9 + (n + sentence) % 7) {
                let pick = WORDS[(n * 7 + sentence * 3 + word * 5) % WORDS.len()];
                if word == 3 && sentence == 1 {
                    text.push_str(&format!("**{pick}** "));
                } else if word == 5 && sentence == 3 {
                    text.push_str(&format!("`{pick}_{n}` "));
                } else {
                    text.push_str(pick);
                    text.push(' ');
                }
            }
            text.pop();
            text.push_str(". ");
        }
        text.push_str("\n\n");
        if n % 4 == 3 {
            for item in 0..3 {
                text.push_str(&format!(
                    "- point {item} of paragraph {n} about the stream\n"
                ));
            }
            text.push('\n');
        }
        n += 1;
    }
    text
}

fn code(bytes: usize) -> String {
    let mut text = String::from("Here is the module:\n\n```rust\n");
    let mut n = 0usize;
    while text.len() < bytes {
        text.push_str(&format!(
            "/// Step {n} of the pipeline.\n\
             pub fn step_{n}(input: &[u8], scale: usize) -> Result<Vec<u8>, String> {{\n    \
             let mut out = Vec::with_capacity(input.len() * scale);\n    \
             for (index, byte) in input.iter().enumerate() {{\n        \
             if index % {m} == 0 {{ out.push(byte.wrapping_add({n} as u8)); }}\n        \
             else {{ out.extend(std::iter::repeat(*byte).take(scale)); }}\n    \
             }}\n    \
             if out.is_empty() {{ return Err(\"step {n} produced nothing\".to_string()); }}\n    \
             Ok(out)\n}}\n\n",
            m = n % 7 + 2,
        ));
        n += 1;
    }
    text.push_str("```\n\n");
    text
}

fn table(bytes: usize) -> String {
    let mut text = String::from(
        "The measurements:\n\n| run | scenario | p50 ms | p99 ms | note |\n|---:|:---|---:|---:|:---|\n",
    );
    let mut n = 0usize;
    while text.len() < bytes {
        text.push_str(&format!(
            "| {n} | stream {} | {}.{} | {}.{} | row {n} of the table with a cell that wraps |\n",
            n % 9,
            n % 13,
            n % 10,
            n % 41,
            n % 10
        ));
        n += 1;
        // A table per forty rows, so each one closes and commits.
        if n.is_multiple_of(40) {
            text.push_str(
                "\nAnd the next batch:\n\n| run | scenario | p50 ms | p99 ms | note |\n|---:|:---|---:|---:|:---|\n",
            );
        }
    }
    text.push('\n');
    text
}

/// One paragraph of thought with no line break in it — how some providers
/// stream reasoning, and the shape that costs the live thinking block most:
/// its window is the last rows of the newest paragraph.
fn paragraph(bytes: usize) -> String {
    let mut text = String::new();
    let mut n = 0usize;
    while text.len() < bytes {
        text.push_str(&format!(
            "then option {n} is weighed against the one before it, "
        ));
        n += 1;
    }
    text
}

fn oneline(bytes: usize) -> String {
    let mut text = String::from("The payload: ");
    let mut n = 0usize;
    while text.len() < bytes {
        text.push_str(&format!(
            "{{\"id\":{n},\"name\":\"item-{n}\",\"tags\":[\"a\",\"b\"]}},"
        ));
        n += 1;
    }
    text.push_str("\n\n");
    text
}

fn mixed(bytes: usize) -> String {
    let mut text = String::new();
    let part = (bytes / 6).max(400);
    while text.len() < bytes {
        text.push_str("## A section\n\n");
        text.push_str(&prose(part));
        text.push_str(&code(part));
        text.push_str(&table(part));
        text.push_str("1. first\n2. second\n   - nested\n3. third\n\n> a quoted line\n\n");
    }
    text
}

fn reply_for(scenario: &str, bytes: usize, rate: u64, turn: u64, follow_up: bool) -> Reply {
    let closing = format!("{END_MARK}-{turn}\n");
    if follow_up {
        return Reply::Text(format!("The call finished.\n\n{closing}"));
    }
    match scenario {
        "idle" | "prose" => Reply::Text(prose(bytes) + &closing),
        "code" => Reply::Text(code(bytes) + &closing),
        "table" => Reply::Text(table(bytes) + &closing),
        "oneline" => Reply::Text(oneline(bytes) + &closing),
        "mixed" => Reply::Text(mixed(bytes) + &closing),
        "reasoning" => Reply::Reasoned {
            thought: prose(bytes / 2),
            text: prose(bytes / 2) + &closing,
        },
        "thought" => Reply::Reasoned {
            thought: paragraph(bytes),
            text: format!("Decided.\n\n{closing}"),
        },
        "write" => Reply::Call {
            name: "write",
            arguments: serde_json::json!({
                "path": "probe_output.rs",
                "content": code(bytes),
            })
            .to_string(),
        },
        "bash" => {
            // `rate` lines a second, in twentieths of a second; 0 never pauses.
            let pause = if rate > 0 {
                format!(
                    "if [ $((i % {})) -eq 0 ]; then sleep 0.05; fi; ",
                    (rate / 20).max(1)
                )
            } else {
                String::new()
            };
            Reply::Call {
                name: "bash",
                arguments: serde_json::json!({
                    "command": format!(
                        "i=0; while [ $i -lt {} ]; do echo \"line $i of the command's output, long enough to wrap now and then\"; i=$((i+1)); {pause}done",
                        bytes / 66
                    ),
                    "description": "Print many lines",
                    "wait": 600,
                })
                .to_string(),
            }
        }
        other => {
            eprintln!("unknown scenario `{other}`");
            usage();
        }
    }
}

// ---------------------------------------------------------------------------
// The provider: just enough of Chat Completions over SSE.
// ---------------------------------------------------------------------------

#[derive(Default)]
struct StubState {
    scenario: Mutex<String>,
    bytes: AtomicU64,
    rate: AtomicU64,
    chars: AtomicU64,
    /// When the stream's first frame was written.
    first_frame: Mutex<Option<Instant>>,
    /// When the round that carries [`END_MARK`] was written out whole.
    sent_all: Mutex<Option<Instant>>,
    frames_sent: AtomicU64,
    streaming: AtomicBool,
    /// The turn being answered, which its closing mark names.
    turn: AtomicU64,
}

fn sse(payload: &serde_json::Value) -> Vec<u8> {
    format!("data: {payload}\n\n").into_bytes()
}

/// Split `text` into pieces of `chars` characters.
fn pieces(text: &str, chars: usize) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0usize;
    let mut count = 0usize;
    for (at, ch) in text.char_indices() {
        if count == chars {
            out.push(&text[start..at]);
            start = at;
            count = 0;
        }
        count += 1;
        let _ = ch;
    }
    if start < text.len() {
        out.push(&text[start..]);
    }
    out
}

/// Write `frames` at `rate` a second (0: as fast as the socket takes them).
fn pace_out(stream: &mut TcpStream, frames: &[Vec<u8>], rate: u64, state: &StubState) -> bool {
    let started = Instant::now();
    for (index, frame) in frames.iter().enumerate() {
        if stream.write_all(frame).is_err() {
            return false;
        }
        if index == 0 {
            let mut first = state.first_frame.lock().unwrap();
            if first.is_none() {
                *first = Some(Instant::now());
            }
        }
        state.frames_sent.fetch_add(1, Ordering::Relaxed);
        if rate > 0 {
            let _ = stream.flush();
            let due = started + Duration::from_nanos(1_000_000_000 / rate * (index as u64 + 1));
            let now = Instant::now();
            if due > now {
                std::thread::sleep(due - now);
            }
        }
    }
    stream.flush().is_ok()
}

fn serve(mut stream: TcpStream, state: &StubState) {
    let mut reader = BufReader::new(stream.try_clone().expect("clone the socket"));
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).unwrap_or(0) == 0 {
        return;
    }
    let mut length = 0usize;
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).unwrap_or(0) == 0 || header == "\r\n" {
            break;
        }
        if let Some((name, value)) = header.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            length = value.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0u8; length];
    if reader.read_exact(&mut body).is_err() {
        return;
    }
    if request_line.starts_with("GET") {
        let listing = br#"{"object":"list","data":[{"id":"stub-model","context_length":1000000}]}"#;
        let _ = write!(
            stream,
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
            listing.len()
        );
        let _ = stream.write_all(listing);
        return;
    }
    let body = String::from_utf8_lossy(&body);
    // A request whose conversation already holds a tool result is the round
    // after the call: answer it with the closing text.
    let follow_up = body.contains("\"role\":\"tool\"");
    let scenario = state.scenario.lock().unwrap().clone();
    let bytes = state.bytes.load(Ordering::Relaxed) as usize;
    let chars = state.chars.load(Ordering::Relaxed).max(1) as usize;
    let rate = state.rate.load(Ordering::Relaxed);
    let turn = state.turn.load(Ordering::Relaxed);
    let reply = reply_for(&scenario, bytes, rate, turn, follow_up);
    let mut frames: Vec<Vec<u8>> = Vec::new();
    let mut finish = "stop";
    let closes = match &reply {
        Reply::Text(text) => {
            for piece in pieces(text, chars) {
                frames.push(sse(
                    &serde_json::json!({"choices":[{"delta":{"content":piece}}]}),
                ));
            }
            true
        }
        Reply::Reasoned { thought, text } => {
            for piece in pieces(thought, chars) {
                frames.push(sse(
                    &serde_json::json!({"choices":[{"delta":{"reasoning":piece}}]}),
                ));
            }
            for piece in pieces(text, chars) {
                frames.push(sse(
                    &serde_json::json!({"choices":[{"delta":{"content":piece}}]}),
                ));
            }
            true
        }
        Reply::Call { name, arguments } => {
            frames.push(sse(
                &serde_json::json!({"choices":[{"delta":{"content":"Running it now.\n\n"}}]}),
            ));
            frames.push(sse(&serde_json::json!({"choices":[{"delta":{"tool_calls":[
                {"index":0,"id":"call_probe","type":"function","function":{"name":name,"arguments":""}}
            ]}}]})));
            for piece in pieces(arguments, chars) {
                frames.push(sse(&serde_json::json!({"choices":[{"delta":{"tool_calls":[
                    {"index":0,"function":{"arguments":piece}}
                ]}}]})));
            }
            finish = "tool_calls";
            false
        }
    };
    frames.push(sse(&serde_json::json!({
        "choices":[{"delta":{},"finish_reason":finish}],
        "usage":{"prompt_tokens":1200,"completion_tokens":frames.len(),"total_tokens":1200+frames.len()}
    })));
    frames.push(b"data: [DONE]\n\n".to_vec());
    let _ = stream.write_all(
        b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n",
    );
    state.streaming.store(true, Ordering::Relaxed);
    let whole = pace_out(&mut stream, &frames, rate, state);
    if whole && closes {
        *state.sent_all.lock().unwrap() = Some(Instant::now());
    }
}

fn start_stub(state: Arc<StubState>) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind the stub provider");
    let port = listener.local_addr().expect("the stub's address").port();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let state = state.clone();
            std::thread::spawn(move || serve(stream, &state));
        }
    });
    port
}

// ---------------------------------------------------------------------------
// The terminal.
// ---------------------------------------------------------------------------

struct Terminal {
    process: PtyProcess,
    input: std::fs::File,
    output: Receiver<(Instant, Vec<u8>)>,
    screen: Screen,
    bytes: u64,
    frames: u64,
    /// The tail of the last chunk, so a frame marker cut by a read still counts.
    carry: Vec<u8>,
}

const FRAME_OPEN: &[u8] = b"\x1b[?2026h";

impl Terminal {
    fn start(command: &str, rows: u16, cols: u16) -> Self {
        let process = spawn(None, command).expect("start the app in a terminal");
        let input = process.master.try_clone().expect("clone the terminal");
        let mut reader = process.master.try_clone().expect("clone the terminal");
        let (tx, output) = channel();
        std::thread::spawn(move || {
            let mut buf = vec![0u8; 65_536];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if tx.send((Instant::now(), buf[..n].to_vec())).is_err() {
                            break;
                        }
                    }
                }
            }
        });
        Self {
            process,
            input,
            output,
            screen: Screen::new(rows, cols),
            bytes: 0,
            frames: 0,
            carry: Vec::new(),
        }
    }

    fn send(&mut self, bytes: &[u8]) -> Instant {
        let at = Instant::now();
        self.input.write_all(bytes).expect("type into the terminal");
        at
    }

    fn absorb(&mut self, chunk: &[u8]) {
        self.bytes += chunk.len() as u64;
        let mut joined = std::mem::take(&mut self.carry);
        joined.extend_from_slice(chunk);
        self.frames += joined
            .windows(FRAME_OPEN.len())
            .filter(|window| *window == FRAME_OPEN)
            .count() as u64;
        let keep = joined.len().min(FRAME_OPEN.len() - 1);
        self.carry = joined[joined.len() - keep..].to_vec();
        let replies = self.screen.feed(chunk);
        if !replies.is_empty() {
            let _ = self.input.write_all(&replies);
        }
    }

    /// Fold output in until `done` holds; the instant of the chunk that made
    /// it so, or `None` once `timeout` passes.
    fn until(
        &mut self,
        timeout: Duration,
        mut done: impl FnMut(&Screen) -> bool,
    ) -> Option<Instant> {
        let deadline = Instant::now() + timeout;
        if done(&self.screen) {
            return Some(Instant::now());
        }
        loop {
            let left = deadline.checked_duration_since(Instant::now())?;
            match self.output.recv_timeout(left) {
                Ok((at, chunk)) => {
                    self.absorb(&chunk);
                    if done(&self.screen) {
                        return Some(at);
                    }
                }
                Err(RecvTimeoutError::Timeout) => return None,
                Err(RecvTimeoutError::Disconnected) => return None,
            }
        }
    }

    /// Fold in whatever arrives for `span`.
    fn settle(&mut self, span: Duration) {
        let _ = self.until(span, |_| false);
    }

    fn shows(&self, needle: &str) -> bool {
        self.screen
            .snapshot()
            .rows
            .iter()
            .any(|row| row.contains(needle))
    }

    fn dump(&self) -> String {
        self.screen.snapshot().rows.join("\n")
    }
}

/// Nanoseconds of CPU a thread has run, off `/proc/{pid}/task/{tid}/schedstat`.
fn thread_cpu(pid: u32, tid: u32) -> u64 {
    std::fs::read_to_string(format!("/proc/{pid}/task/{tid}/schedstat"))
        .ok()
        .and_then(|text| text.split_whitespace().next().and_then(|n| n.parse().ok()))
        .unwrap_or(0)
}

/// Nanoseconds of CPU every thread of the process has run.
fn process_cpu(pid: u32) -> u64 {
    std::fs::read_dir(format!("/proc/{pid}/task"))
        .map(|tasks| {
            tasks
                .flatten()
                .filter_map(|task| task.file_name().to_str().and_then(|name| name.parse().ok()))
                .map(|tid: u32| thread_cpu(pid, tid))
                .sum()
        })
        .unwrap_or(0)
}

fn resident_kb(pid: u32) -> u64 {
    std::fs::read_to_string(format!("/proc/{pid}/status"))
        .ok()
        .and_then(|text| {
            text.lines()
                .find(|line| line.starts_with("VmRSS:"))
                .and_then(|line| line.split_whitespace().nth(1).and_then(|n| n.parse().ok()))
        })
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// One run.
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Outcome {
    scenario: String,
    latencies_us: Vec<u64>,
    missed: usize,
    stream_ms: f64,
    /// From the provider's last byte to the whole reply on screen.
    catch_up_ms: f64,
    loop_cpu_ms: f64,
    process_cpu_ms: f64,
    frames: u64,
    bytes: u64,
    frames_sent: u64,
    resident_kb: u64,
}

fn percentile(sorted: &[u64], pct: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    let rank = ((sorted.len() - 1) as f64 * pct / 100.0).round() as usize;
    sorted[rank] as f64 / 1000.0
}

fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

fn launch(options: &Options, root: &Path, port: u16) -> Terminal {
    let providers = root.join("providers.toml");
    std::fs::write(
        &providers,
        format!(
            "[providers.stub]\nname = \"Stub\"\n\n[providers.stub.kwargs]\napi_base = \"http://127.0.0.1:{port}/v1\"\n"
        ),
    )
    .expect("write the providers file");
    for dir in ["cfg", "sessions", "skills", "agents", "home", "work", "tmp"] {
        std::fs::create_dir_all(root.join(dir)).expect("create the run's directories");
    }
    let bin = std::fs::canonicalize(&options.bin).unwrap_or_else(|err| {
        eprintln!(
            "{}: {err} (run: cargo build --release)",
            options.bin.display()
        );
        std::process::exit(2);
    });
    let dir = |name: &str| root.join(name).display().to_string();
    let mut env: Vec<(String, String)> = vec![
        ("PATH".into(), std::env::var("PATH").unwrap_or_default()),
        ("TERM".into(), "xterm-256color".into()),
        ("LANG".into(), "C.UTF-8".into()),
        ("HOME".into(), dir("home")),
        ("TMPDIR".into(), dir("tmp")),
        ("ALTER_ZERO_CONFIG_DIR".into(), dir("cfg")),
        ("ALTER_ZERO_SESSIONS_DIR".into(), dir("sessions")),
        ("ALTER_ZERO_SKILLS_DIR".into(), dir("skills")),
        ("ALTER_ZERO_AGENTS_DIR".into(), dir("agents")),
        ("ALTER_ZERO_HISTORY_FILE".into(), "/dev/null".into()),
        ("ALTER_ZERO_PROJECT_CONFIG".into(), "0".into()),
        ("ALTER_ZERO_TELEMETRY".into(), "0".into()),
        ("ALTER_ZERO_UPDATE_CHECK".into(), "0".into()),
        ("ALTER_ZERO_TIPS".into(), "0".into()),
        ("ALTER_ZERO_CHECKPOINTS".into(), "0".into()),
        ("ALTER_ZERO_PERMISSIONS".into(), "0".into()),
        ("ALTER_ZERO_PROJECT_DOC_MAX_BYTES".into(), "0".into()),
        ("ALTER_ZERO_VERBOSE".into(), "1".into()),
        (
            "ALTER_ZERO_PROVIDERS_FILE".into(),
            providers.display().to_string(),
        ),
        ("ALTER_ZERO_PROVIDER".into(), "stub".into()),
        ("ALTER_ZERO_MODEL".into(), "stub-model".into()),
        ("STUB_API_KEY".into(), "stand-in".into()),
    ];
    env.extend(options.env.iter().cloned());
    let assignments: Vec<String> = env
        .iter()
        .map(|(key, value)| format!("{key}={}", shell_quote(value)))
        .collect();
    let command = format!(
        "stty rows {} cols {}; cd {} && exec env -i {} {}",
        options.rows,
        options.cols,
        shell_quote(&dir("work")),
        assignments.join(" "),
        shell_quote(&bin.display().to_string()),
    );
    let mut terminal = Terminal::start(&command, options.rows, options.cols);
    // The composer is up once the footer names the model.
    if terminal
        .until(STEP_TIMEOUT, |screen| {
            screen
                .snapshot()
                .rows
                .iter()
                .any(|row| row.contains("stub-model"))
        })
        .is_none()
    {
        eprintln!("the app never reached its composer:\n{}", terminal.dump());
        std::process::exit(1);
    }
    terminal.settle(Duration::from_millis(400));
    terminal
}

/// Type `count` keys `every` apart, timing each to the frame that shows it.
/// `stop` ends the run early (the stream it was to be typed under is over).
fn type_keys(
    terminal: &mut Terminal,
    count: usize,
    every: Duration,
    stop: impl Fn(&Terminal) -> bool,
    outcome: &mut Outcome,
) {
    let keys: Vec<char> = KEYS.chars().collect();
    let mut typed = String::new();
    for index in 0..count {
        if stop(terminal) {
            break;
        }
        // A row of the probe's own keys, then a clean line: the draft never
        // wraps, so the suffix the frame is recognised by stays on one row.
        if typed.chars().count() >= 24 {
            terminal.send(&[0x15]); // Ctrl+U
            typed.clear();
            terminal.settle(every);
        }
        let key = keys[index % keys.len()];
        typed.push(key);
        let tail: String = typed
            .chars()
            .rev()
            .take(2)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        let mut encoded = [0u8; 4];
        let sent = terminal.send(key.encode_utf8(&mut encoded).as_bytes());
        let shown = terminal.until(Duration::from_secs(10), |screen| {
            !screen.cursor_hidden() && screen.holds_at_cursor(&tail)
        });
        match shown {
            // The first key of a row has a one-character suffix, which an
            // earlier state can end on too — typed, never counted.
            Some(at) if tail.chars().count() == 2 => {
                outcome
                    .latencies_us
                    .push(at.saturating_duration_since(sent).as_micros() as u64);
            }
            Some(_) => {}
            None => outcome.missed += 1,
        }
        let next = sent + every;
        if let Some(left) = next.checked_duration_since(Instant::now()) {
            terminal.settle(left);
        }
    }
    terminal.send(&[0x15]);
    terminal.settle(Duration::from_millis(50));
}

/// The pager's rows a scroll key is recognised by: a band from the middle of
/// its body, clear of the title above and the hint rows below.
fn pager_band(screen: &Screen) -> Vec<String> {
    let rows = screen.snapshot().rows;
    rows.iter().skip(8).take(10).cloned().collect()
}

/// Time the Ctrl+O pager's scroll keys while the turn streams under it: open
/// it, step off the bottom so the page holds still, then press ↑ and ↓ in
/// turn, each timed to the frame whose rows have moved by one.
fn scroll_keys(
    terminal: &mut Terminal,
    every: Duration,
    stop: impl Fn(&Terminal) -> bool,
    outcome: &mut Outcome,
) {
    terminal.send(&[0x0f]); // Ctrl+O
    if terminal
        .until(Duration::from_secs(20), Screen::alternate)
        .is_none()
    {
        eprintln!("the pager never opened:\n{}", terminal.dump());
        std::process::exit(1);
    }
    for _ in 0..2 {
        terminal.send(b"\x1b[5~"); // PageUp: leave the tail it follows
        terminal.settle(Duration::from_millis(200));
    }
    let mut up = true;
    while !stop(terminal) {
        let before = pager_band(&terminal.screen);
        let distinct = before
            .iter()
            .filter(|row| !row.trim().is_empty())
            .collect::<std::collections::BTreeSet<_>>()
            .len();
        let sent = terminal.send(if up { b"\x1b[A" } else { b"\x1b[B" });
        let shown = terminal.until(Duration::from_secs(10), |screen| {
            let now = pager_band(screen);
            if now.len() != before.len() || before.len() < 2 {
                return false;
            }
            if up {
                now[1..] == before[..before.len() - 1]
            } else {
                now[..now.len() - 1] == before[1..]
            }
        });
        match shown {
            // A band of rows too alike to tell a one-row move apart is typed
            // through, never counted.
            Some(at) if distinct >= 4 => outcome
                .latencies_us
                .push(at.saturating_duration_since(sent).as_micros() as u64),
            Some(_) => {}
            None => outcome.missed += 1,
        }
        up = !up;
        if let Some(left) = (sent + every).checked_duration_since(Instant::now()) {
            terminal.settle(left);
        }
    }
    terminal.send(b"q");
    let _ = terminal.until(Duration::from_secs(20), |screen| !screen.alternate());
}

/// Submit a prompt and wait for the turn it starts to settle: its closing
/// mark on screen and the status line's interrupt hint gone. `typing` runs
/// once the provider's first frame is out.
fn turn(
    terminal: &mut Terminal,
    state: &Arc<StubState>,
    number: u64,
    mut typing: impl FnMut(&mut Terminal, &dyn Fn(&Terminal) -> bool),
) -> (Instant, Instant) {
    state.turn.store(number, Ordering::Relaxed);
    *state.first_frame.lock().unwrap() = None;
    *state.sent_all.lock().unwrap() = None;
    let mark = format!("{END_MARK}-{number}");
    terminal.send(b"go");
    terminal.settle(Duration::from_millis(250));
    let submitted = terminal.send(b"\r");
    let waiting = Instant::now();
    while state.first_frame.lock().unwrap().is_none() {
        terminal.settle(Duration::from_millis(5));
        if waiting.elapsed() > STEP_TIMEOUT {
            eprintln!("the provider was never asked:\n{}", terminal.dump());
            std::process::exit(1);
        }
    }
    let stub = state.clone();
    let shown = mark.clone();
    typing(terminal, &move |terminal: &Terminal| {
        stub.sent_all.lock().unwrap().is_some()
            && (terminal.screen.alternate() || terminal.shows(&shown))
    });
    let settled = terminal.until(STEP_TIMEOUT, |screen| {
        let rows = screen.snapshot().rows;
        rows.iter().any(|row| row.contains(&mark))
            && !rows.iter().any(|row| row.contains("esc to interrupt"))
    });
    let Some(settled) = settled else {
        eprintln!("turn {number} never settled:\n{}", terminal.dump());
        std::process::exit(1);
    };
    (submitted, settled)
}

fn run(options: &Options, scenario: &str, state: &Arc<StubState>, port: u16) -> Outcome {
    let root =
        std::env::temp_dir().join(format!("latency-probe-{}-{scenario}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("create the run's root");
    *state.scenario.lock().unwrap() = scenario.to_string();
    state.bytes.store(options.bytes as u64, Ordering::Relaxed);
    state.chars.store(options.chars as u64, Ordering::Relaxed);
    state.frames_sent.store(0, Ordering::Relaxed);
    state.streaming.store(false, Ordering::Relaxed);

    let mut terminal = launch(options, &root, port);
    let pid = terminal.process.child.id();
    let mut outcome = Outcome {
        scenario: scenario.to_string(),
        ..Outcome::default()
    };

    // The turns before the measured one: streamed unpaced, so the session
    // has a history by the time a key is timed.
    state.rate.store(0, Ordering::Relaxed);
    for number in 1..=options.warmup as u64 {
        turn(&mut terminal, state, number, |_, _| {});
        terminal.settle(Duration::from_millis(100));
    }
    state.rate.store(options.rate, Ordering::Relaxed);
    state.frames_sent.store(0, Ordering::Relaxed);

    // A draft already in the composer, so each timed key edits a long one.
    // Typed a slice at a time: the point is the cost of a key over a long
    // draft, and a burst is the `burst` scenario's to measure.
    if options.draft > 0 && scenario != "burst" {
        if scenario != "idle" {
            eprintln!("--draft is measured idle only; pass --scenario idle or burst");
            std::process::exit(2);
        }
        let filler: Vec<u8> = FILLER.bytes().cycle().take(options.draft).collect();
        for slice in filler.chunks(256) {
            terminal.send(slice);
            terminal.settle(Duration::from_millis(40));
        }
        terminal.send(b"\n"); // Ctrl+J: the timed keys get a row of their own
        terminal.settle(Duration::from_millis(1500));
    }

    let cpu0 = (thread_cpu(pid, pid), process_cpu(pid));
    let (frames0, bytes0) = (terminal.frames, terminal.bytes);
    if scenario == "burst" {
        // Everything at once — a paste the terminal did not bracket, a text
        // expander, `tmux send-keys`: timed to the frame that shows its end.
        let mut text: Vec<u8> = FILLER
            .bytes()
            .cycle()
            .take(if options.draft > 0 {
                options.draft
            } else {
                4000
            })
            .collect();
        text.extend_from_slice(BURST_END.as_bytes());
        let mut input = terminal.input.try_clone().expect("clone the terminal");
        let sent = Instant::now();
        // Written off this thread: a terminal's input queue is a few
        // kilobytes, and the write waits on the app reading it.
        std::thread::spawn(move || {
            let _ = input.write_all(&text);
        });
        let shown = terminal.until(Duration::from_secs(8), |screen| {
            !screen.cursor_hidden() && screen.holds_at_cursor(BURST_END)
        });
        match shown {
            Some(at) => outcome
                .latencies_us
                .push(at.saturating_duration_since(sent).as_micros() as u64),
            None => outcome.missed += 1,
        }
        outcome.stream_ms = sent.elapsed().as_secs_f64() * 1e3;
    } else if scenario == "idle" {
        let began = Instant::now();
        type_keys(
            &mut terminal,
            120,
            options.key_every,
            |_| false,
            &mut outcome,
        );
        outcome.stream_ms = began.elapsed().as_secs_f64() * 1e3;
    } else {
        let every = options.key_every;
        let overlay = options.overlay;
        let number = options.warmup as u64 + 1;
        let (submitted, settled) = turn(&mut terminal, state, number, |terminal, over| {
            if overlay {
                scroll_keys(terminal, every, over, &mut outcome);
            } else {
                type_keys(terminal, usize::MAX, every, over, &mut outcome);
            }
        });
        outcome.stream_ms = settled.saturating_duration_since(submitted).as_secs_f64() * 1e3;
        if let Some(sent_all) = *state.sent_all.lock().unwrap() {
            outcome.catch_up_ms = settled.saturating_duration_since(sent_all).as_secs_f64() * 1e3;
        }
        terminal.settle(Duration::from_millis(200));
    }
    outcome.loop_cpu_ms = (thread_cpu(pid, pid) - cpu0.0) as f64 / 1e6;
    outcome.process_cpu_ms = (process_cpu(pid) - cpu0.1) as f64 / 1e6;
    outcome.frames = terminal.frames - frames0;
    outcome.bytes = terminal.bytes - bytes0;
    outcome.frames_sent = state.frames_sent.load(Ordering::Relaxed);
    outcome.resident_kb = resident_kb(pid);

    // Ctrl+C on an empty composer quits.
    terminal.send(&[0x03]);
    terminal.settle(Duration::from_millis(300));
    let _ = terminal.process.child.kill();
    let _ = terminal.process.child.wait();
    if !options.keep {
        let _ = std::fs::remove_dir_all(&root);
    }
    outcome
}

fn main() {
    let options = parse_options();
    let state = Arc::new(StubState::default());
    let port = start_stub(state.clone());
    println!(
        "latency_probe: {} · {}x{} · {} frames/s × {} chars · {} KB replies · a key every {} ms{}{}{}",
        options.bin.display(),
        options.cols,
        options.rows,
        if options.rate == 0 {
            "unpaced".to_string()
        } else {
            options.rate.to_string()
        },
        options.chars,
        options.bytes / 1000,
        options.key_every.as_millis(),
        if options.warmup > 0 {
            format!(" · after {} turns", options.warmup)
        } else {
            String::new()
        },
        if options.draft > 0 {
            format!(" · over a {}-char draft", options.draft)
        } else {
            String::new()
        },
        if options.overlay {
            " · scrolling the Ctrl+O pager"
        } else {
            ""
        },
    );
    println!(
        "{:<10} {:>5} {:>7} {:>7} {:>7} {:>7} {:>8} {:>8} {:>8} {:>8} {:>7} {:>8} {:>7}",
        "scenario",
        "keys",
        "p50ms",
        "p90ms",
        "p99ms",
        "maxms",
        "stream_s",
        "catchup",
        "loopcpu",
        "proccpu",
        "frames",
        "out_kb",
        "rss_mb"
    );
    let mut report: Vec<BTreeMap<String, serde_json::Value>> = Vec::new();
    for scenario in &options.scenarios {
        let outcome = run(&options, scenario, &state, port);
        let mut sorted = outcome.latencies_us.clone();
        sorted.sort_unstable();
        println!(
            "{:<10} {:>5} {:>7.2} {:>7.2} {:>7.2} {:>7.2} {:>8.2} {:>8.0} {:>8.0} {:>8.0} {:>7} {:>8} {:>7.1}{}",
            outcome.scenario,
            sorted.len(),
            percentile(&sorted, 50.0),
            percentile(&sorted, 90.0),
            percentile(&sorted, 99.0),
            sorted.last().map_or(f64::NAN, |us| *us as f64 / 1000.0),
            outcome.stream_ms / 1e3,
            outcome.catch_up_ms,
            outcome.loop_cpu_ms,
            outcome.process_cpu_ms,
            outcome.frames,
            outcome.bytes / 1000,
            outcome.resident_kb as f64 / 1024.0,
            if outcome.missed > 0 {
                format!("  ({} keys never shown)", outcome.missed)
            } else {
                String::new()
            },
        );
        let mut row = BTreeMap::new();
        row.insert("scenario".to_string(), serde_json::json!(outcome.scenario));
        row.insert("keys".to_string(), serde_json::json!(sorted.len()));
        row.insert("missed".to_string(), serde_json::json!(outcome.missed));
        row.insert(
            "p50_ms".to_string(),
            serde_json::json!(percentile(&sorted, 50.0)),
        );
        row.insert(
            "p90_ms".to_string(),
            serde_json::json!(percentile(&sorted, 90.0)),
        );
        row.insert(
            "p99_ms".to_string(),
            serde_json::json!(percentile(&sorted, 99.0)),
        );
        row.insert(
            "max_ms".to_string(),
            serde_json::json!(sorted.last().map_or(0.0, |us| *us as f64 / 1000.0)),
        );
        // Which keys were the slow ones — their place in the run says what
        // the app was doing (a reply ending, a tip appearing, a verb turning).
        let mut slowest: Vec<(usize, u64)> =
            outcome.latencies_us.iter().copied().enumerate().collect();
        slowest.sort_unstable_by_key(|&(_, us)| std::cmp::Reverse(us));
        slowest.truncate(5);
        row.insert(
            "slowest".to_string(),
            serde_json::json!(
                slowest
                    .iter()
                    .map(|&(at, us)| (at, us as f64 / 1000.0))
                    .collect::<Vec<_>>()
            ),
        );
        row.insert(
            "stream_ms".to_string(),
            serde_json::json!(outcome.stream_ms),
        );
        row.insert(
            "catch_up_ms".to_string(),
            serde_json::json!(outcome.catch_up_ms),
        );
        row.insert(
            "loop_cpu_ms".to_string(),
            serde_json::json!(outcome.loop_cpu_ms),
        );
        row.insert(
            "process_cpu_ms".to_string(),
            serde_json::json!(outcome.process_cpu_ms),
        );
        row.insert("frames".to_string(), serde_json::json!(outcome.frames));
        row.insert("bytes".to_string(), serde_json::json!(outcome.bytes));
        row.insert(
            "frames_sent".to_string(),
            serde_json::json!(outcome.frames_sent),
        );
        row.insert(
            "resident_kb".to_string(),
            serde_json::json!(outcome.resident_kb),
        );
        report.push(row);
    }
    if let Some(path) = &options.json {
        std::fs::write(
            path,
            serde_json::to_string_pretty(&report).expect("serialize the report"),
        )
        .expect("write the report");
    }
}
