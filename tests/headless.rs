//! `alter-zero -p` against the **real built binary** with no terminal
//! anywhere (`docs/headless.md`): every stream a pipe, the way a cron job, a
//! CI step or an n8n node runs it — the reported failure was the binary
//! dying on exactly that (`Device not configured`) before it read a prompt.
//!
//! Two backends. The offline demo (`ALTER_ZERO_DUMMY=1`) drives the output
//! formats, the prompt's sources and `--continue`. A stand-in provider on the
//! loopback — a few lines of `std::net` answering Chat Completions — drives
//! what only a real backend has: a `bash` call the permission mode decides,
//! and the `--max-turns` budget. Hermetic: every run gets its own config
//! home, sessions dir, `HOME` and working directory, and nothing reaches the
//! network beyond the loopback.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The binary this test run built.
fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_alter-zero"))
}

/// A run's private world: config home, sessions, `HOME`, working directory.
struct World {
    root: tempdir::Dir,
}

/// A scratch directory removed with the test — `std` only.
mod tempdir {
    use std::path::{Path, PathBuf};

    pub struct Dir(PathBuf);

    impl Dir {
        pub fn new(tag: &str) -> Self {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos());
            let path = std::env::temp_dir().join(format!(
                "alter-zero-headless-{tag}-{}-{nanos}",
                std::process::id()
            ));
            std::fs::create_dir_all(&path).expect("a scratch dir");
            Self(path)
        }

        pub fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

impl World {
    fn new(tag: &str) -> Self {
        let root = tempdir::Dir::new(tag);
        for dir in ["cfg", "sessions", "home", "skills", "agents", "work"] {
            std::fs::create_dir_all(root.path().join(dir)).expect("a world dir");
        }
        Self { root }
    }

    fn dir(&self, name: &str) -> PathBuf {
        self.root.path().join(name)
    }

    fn work(&self) -> PathBuf {
        self.dir("work")
    }

    /// `alter-zero …` in this world, its environment scrubbed of anything
    /// that would reach the developer's own config, a real provider, or a
    /// proxy that cannot see the loopback.
    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(bin());
        command
            .args(args)
            .current_dir(self.work())
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", self.dir("home"))
            .env("ALTER_ZERO_CONFIG_DIR", self.dir("cfg"))
            .env("ALTER_ZERO_SESSIONS_DIR", self.dir("sessions"))
            .env("ALTER_ZERO_SKILLS_DIR", self.dir("skills"))
            .env("ALTER_ZERO_AGENTS_DIR", self.dir("agents"))
            .env("ALTER_ZERO_HISTORY_FILE", "/dev/null")
            .env("ALTER_ZERO_PROJECT_CONFIG", "0")
            .env("ALTER_ZERO_CHECKPOINTS", "0")
            .env("ALTER_ZERO_TELEMETRY", "0")
            .env("ALTER_ZERO_UPDATE_CHECK", "0")
            .env("ALTER_ZERO_STARTUP_DELAY_MS", "0")
            .env("ALTER_ZERO_CHUNK_DELAY_MS", "0")
            .env("NO_PROXY", "127.0.0.1,localhost")
            .env("no_proxy", "127.0.0.1,localhost");
        command
    }

    /// A run of the offline demo, stdin closed.
    fn demo(&self, args: &[&str]) -> Output {
        self.command(args)
            .env("ALTER_ZERO_DUMMY", "1")
            .stdin(Stdio::null())
            .output()
            .expect("the binary runs")
    }
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).to_string()
}

fn events(out: &Output) -> Vec<serde_json::Value> {
    stdout(out)
        .lines()
        .map(|line| serde_json::from_str(line).unwrap_or_else(|err| panic!("{err}: {line}")))
        .collect()
}

/// The closing sentence every demo reply ends on (`docs/dummy-backend.md`).
const DEMO_HANDOFF: &str = "`/login` signs you in";

// ===== the offline demo: formats, prompts, sessions =====

#[test]
fn text_prints_the_answer_alone_and_nothing_on_stderr() {
    let world = World::new("text");
    let out = world.demo(&["-p", "hello there"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let answer = stdout(&out);
    assert!(answer.contains(DEMO_HANDOFF), "{answer}");
    assert!(
        !answer.contains('\x1b'),
        "no escape ever reaches stdout: {answer:?}"
    );
    // Quiet unless asked: nothing went wrong, so stderr says nothing.
    assert_eq!(stderr(&out), "");
}

#[test]
fn verbose_logs_the_run_to_stderr_and_leaves_stdout_alone() {
    let world = World::new("verbose");
    let quiet = world.demo(&["-p", "hello there"]);
    let out = world.demo(&["-p", "--verbose", "hello there"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out), stdout(&quiet));
    let log = stderr(&out);
    assert!(log.starts_with("alter-zero "), "{log}");
    assert!(log.contains("● Read(about.py)"), "{log}");
    assert!(log.contains("● Bash(python3 about.py)"), "{log}");
    assert!(log.contains("\nDone in "), "{log}");
    assert!(!log.contains('\x1b'), "nor stderr: {log:?}");
}

#[test]
fn stream_json_is_one_event_per_line_from_session_to_result() {
    let world = World::new("stream");
    let out = world.demo(&["-p", "--output-format", "stream-json", "hello there"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let events = events(&out);
    assert_eq!(events[0]["type"], "session.started");
    let session = events[0]["session_id"].as_str().expect("a session id");
    assert_eq!(events[1]["type"], "item.completed");
    assert_eq!(events[1]["item"]["type"], "user_message");
    assert_eq!(events[1]["item"]["text"], "hello there");
    // Every tool call opens and completes under one id.
    let started: Vec<&str> = events
        .iter()
        .filter(|e| e["type"] == "item.started")
        .map(|e| e["item"]["id"].as_str().unwrap())
        .collect();
    assert!(!started.is_empty(), "{events:?}");
    for id in &started {
        assert!(
            events
                .iter()
                .any(|e| e["type"] == "item.completed" && e["item"]["id"] == *id),
            "{id} never completed"
        );
    }
    let result = events.last().expect("a result");
    assert_eq!(result["type"], "result");
    assert_eq!(result["subtype"], "success");
    assert_eq!(result["session_id"], session);
    assert!(result["result"].as_str().unwrap().contains(DEMO_HANDOFF));
}

#[test]
fn json_prints_one_result_object() {
    let world = World::new("json");
    let out = world.demo(&["-p", "--output-format", "json", "hello there"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let events = events(&out);
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0]["type"], "result");
    assert_eq!(events[0]["is_error"], false);
    assert_eq!(events[0]["permission_denials"], serde_json::json!([]));
}

#[test]
fn a_piped_prompt_needs_no_argument() {
    let world = World::new("stdin");
    let mut child = world
        .command(&["-p", "--output-format", "stream-json"])
        .env("ALTER_ZERO_DUMMY", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the binary runs");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(b"hello there\n")
        .expect("the prompt goes in");
    let out = child.wait_with_output().expect("the run ends");
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(events(&out)[1]["item"]["text"], "hello there");
}

#[test]
fn a_file_on_stdin_rides_beside_the_argument_in_a_tagged_block() {
    let world = World::new("stdinfile");
    let input = world.dir("diff.txt");
    std::fs::write(&input, "+added line\n").expect("the input file");
    let out = world
        .command(&["-p", "--output-format", "stream-json", "Review this"])
        .env("ALTER_ZERO_DUMMY", "1")
        .stdin(std::fs::File::open(&input).expect("the input opens"))
        .output()
        .expect("the binary runs");
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        events(&out)[1]["item"]["text"],
        "Review this\n\n<stdin>\n+added line\n</stdin>"
    );
}

#[test]
fn an_open_socket_that_says_nothing_is_left_alone() {
    // n8n's command node: Node starts a command with a socket on its stdin,
    // and a parent that never writes to it or closes it.
    let world = World::new("opensocket");
    let (held_open, theirs) = std::os::unix::net::UnixStream::pair().expect("a socket pair");
    let began = Instant::now();
    let out = world
        .command(&["-p", "--output-format", "json", "hello there"])
        .env("ALTER_ZERO_DUMMY", "1")
        .stdin(Stdio::from(std::os::fd::OwnedFd::from(theirs)))
        .output()
        .expect("the binary runs");
    drop(held_open);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("stdin said nothing"),
        "{}",
        stderr(&out)
    );
    assert!(began.elapsed() < Duration::from_secs(60));
}

#[test]
fn a_slow_pipe_is_waited_for_and_what_it_sends_is_used() {
    // `slow-command | alter-zero -p "summarize"`: a shell pipe ends when
    // its writer does, so the run waits for it rather than lose its data.
    let world = World::new("slowpipe");
    let mut child = world
        .command(&["-p", "--output-format", "stream-json", "summarize"])
        .env("ALTER_ZERO_DUMMY", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the binary runs");
    let mut pipe = child.stdin.take().expect("the pipe");
    // Silent past the cut a socket would get, then the data, then the end.
    std::thread::sleep(Duration::from_secs(4));
    pipe.write_all(b"LATE_DATA\n").expect("written");
    drop(pipe);
    let out = child.wait_with_output().expect("the run ends");
    assert!(out.status.success(), "{}", stderr(&out));
    let prompt = events(&out)
        .into_iter()
        .find(|event| event["item"]["type"] == "user_message")
        .expect("the prompt's item");
    assert_eq!(
        prompt["item"]["text"],
        "summarize\n\n<stdin>\nLATE_DATA\n</stdin>"
    );
    assert_eq!(stderr(&out), "", "nothing went wrong");
}

#[test]
fn nothing_to_answer_is_a_usage_error() {
    let world = World::new("empty");
    let out = world.demo(&["-p"]);
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("the prompt is empty"),
        "{}",
        stderr(&out)
    );
    assert!(stdout(&out).is_empty());
}

#[test]
fn a_headless_flag_without_print_is_a_usage_error() {
    let world = World::new("usage");
    let out = world.demo(&["--output-format", "json", "x"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr(&out).contains("needs --print"), "{}", stderr(&out));
}

#[test]
fn continue_sends_the_prompt_into_the_last_run() {
    let world = World::new("continue");
    let first = world.demo(&["-p", "--output-format", "json", "hello there"]);
    assert!(first.status.success(), "{}", stderr(&first));
    let first_id = events(&first)[0]["session_id"].clone();
    let second = world.demo(&["-p", "-c", "--output-format", "json", "hello again"]);
    assert!(second.status.success(), "{}", stderr(&second));
    assert_eq!(events(&second)[0]["session_id"], first_id);
}

#[test]
fn cwd_runs_the_session_in_that_directory() {
    let world = World::new("cwd");
    let elsewhere = world.dir("elsewhere");
    std::fs::create_dir_all(&elsewhere).expect("the other dir");
    let out = world.demo(&[
        "-p",
        "--output-format",
        "stream-json",
        "-C",
        elsewhere.to_str().unwrap(),
        "hello there",
    ]);
    assert!(out.status.success(), "{}", stderr(&out));
    let real = std::fs::canonicalize(&elsewhere).expect("the dir resolves");
    assert_eq!(events(&out)[0]["cwd"], real.display().to_string());
}

#[test]
fn no_model_configured_is_refused_rather_than_answered_by_the_demo() {
    let world = World::new("nomodel");
    let out = world
        .command(&["-p", "hello there"])
        .stdin(Stdio::null())
        .output()
        .expect("the binary runs");
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("no model is configured"),
        "{}",
        stderr(&out)
    );
    assert!(stdout(&out).is_empty(), "{}", stdout(&out));
}

#[test]
fn a_run_that_cannot_start_still_hands_a_json_reader_its_result() {
    let world = World::new("notstarted");
    let cases: [(&[&str], &str); 3] = [
        (
            &["-p", "--output-format", "json", "hello"],
            "no model is configured",
        ),
        (
            &["-p", "-c", "--output-format", "stream-json", "hello"],
            "No conversation found to continue",
        ),
        (
            &[
                "-p",
                "--resume",
                "no-such-session",
                "--output-format",
                "json",
                "hello",
            ],
            "No session found for id: no-such-session",
        ),
    ];
    for (args, error) in cases {
        let out = world
            .command(args)
            .stdin(Stdio::null())
            .output()
            .expect("the binary runs");
        assert_eq!(out.status.code(), Some(1), "{args:?}: {}", stderr(&out));
        let lines = events(&out);
        assert_eq!(lines.len(), 1, "{args:?}: {lines:?}");
        let result = &lines[0];
        assert_eq!(result["type"], "result", "{args:?}");
        assert_eq!(result["subtype"], "error_during_execution", "{args:?}");
        assert_eq!(result["is_error"], true, "{args:?}");
        assert!(
            result["error"]
                .as_str()
                .is_some_and(|text| text.contains(error)),
            "{args:?}: {result}"
        );
        // …and why, on stderr, as every failure is.
        assert!(stderr(&out).contains(error), "{args:?}: {}", stderr(&out));
    }
}

// ===== a real backend: permissions and the reply budget =====

/// The command the stand-in's model asks to run: it leaves a marker file,
/// the proof it ran.
const MARKER: &str = "headless-marker";

/// A stand-in Chat Completions provider: its "model" answers a user message
/// with one `bash` call making the marker, and anything after a tool result
/// with `Done.` It keeps every chat request's body.
struct StandIn {
    base: String,
    chats: Arc<Mutex<Vec<String>>>,
}

impl StandIn {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
        let base = format!("http://{}/v1", listener.local_addr().expect("an address"));
        let chats = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&chats);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { return };
                // A connection of its own: a held reply (`STALL`) must not
                // hold up the lead's next request.
                let log = Arc::clone(&log);
                std::thread::spawn(move || serve(stream, &log));
            }
        });
        Self { base, chats }
    }

    fn chat_requests(&self) -> Vec<String> {
        self.chats.lock().expect("the log").clone()
    }

    /// Point a world's runs at this provider.
    fn configure(&self, world: &World, command: &mut Command) {
        let file = world.dir("providers.toml");
        std::fs::write(
            &file,
            format!(
                "[providers.stub]\nname = \"Stub\"\n\n[providers.stub.kwargs]\napi_base = \"{}\"\n",
                self.base
            ),
        )
        .expect("the providers file");
        command
            .env("ALTER_ZERO_PROVIDERS_FILE", file)
            .env("ALTER_ZERO_PROVIDER", "stub")
            .env("ALTER_ZERO_MODEL", "stub-model")
            .env("STUB_API_KEY", "stand-in");
    }
}

fn serve(mut stream: TcpStream, log: &Mutex<Vec<String>>) {
    let Some((head, body)) = read_request(&mut stream) else {
        return;
    };
    let (kind, reply) = if head.starts_with("GET") {
        (
            "application/json",
            r#"{"object":"list","data":[{"id":"stub-model"}]}"#.to_string(),
        )
    } else {
        let dropped = {
            let mut log = log.lock().expect("the log");
            log.push(body.clone());
            dropped_deltas(&body, &log)
        };
        if let Some(deltas) = dropped {
            drop_mid_reply(&mut stream, deltas);
            return;
        }
        if body.contains(COMPACT) {
            ("text/event-stream", compact_sse(&body, log))
        } else {
            ("text/event-stream", chat_reply(&body))
        }
    };
    let response = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: {kind}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{reply}",
        reply.len()
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

/// What the stand-in streams of a request's reply before it drops it
/// ([`drop_mid_reply`]), or `None` to answer it whole. It drops the first
/// sending of two requests and answers each when it is sent again: a
/// [`DROP`] prompt's ([`dropped_attempt`]) and the continuation of a
/// [`CUT_SUMMARY`] summary ([`SUMMARY_DRAFT`]). `log` holds every chat
/// request so far, this one included.
fn dropped_deltas(body: &str, log: &[String]) -> Option<Vec<serde_json::Value>> {
    let first = |is: fn(&str) -> bool| is(body) && log.iter().filter(|chat| is(chat)).count() == 1;
    if first(|chat| chat.contains(DROP)) {
        Some(dropped_attempt())
    } else if first(continues_a_cut_summary) {
        Some(vec![serde_json::json!({"content": SUMMARY_DRAFT})])
    } else {
        None
    }
}

/// A reply the stand-in drops: a chunked stream carrying `deltas` — then
/// the connection closes with no terminating chunk, so the body read fails
/// the way a reset or timed-out connection's does.
fn drop_mid_reply(stream: &mut TcpStream, deltas: Vec<serde_json::Value>) {
    let mut response = String::from(
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\nconnection: close\r\n\r\n",
    );
    for delta in deltas {
        let data = format!(
            "data: {}\n\n",
            serde_json::json!({"choices": [{"index": 0, "delta": delta}]})
        );
        response.push_str(&format!("{:x}\r\n{data}\r\n", data.len()));
    }
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

/// Whether a chat request is the continuation of a [`CUT_SUMMARY`]
/// summary: a summarization (it offers no tools) whose last message is the
/// note asking for the rest of a reply the output cap cut off.
fn continues_a_cut_summary(body: &str) -> bool {
    if !body.contains(CUT_SUMMARY) {
        return false;
    }
    let request: serde_json::Value = serde_json::from_str(body).expect("a JSON request");
    let offers_tools = request["tools"]
        .as_array()
        .is_some_and(|tools| !tools.is_empty());
    let last = request["messages"]
        .as_array()
        .and_then(|messages| messages.last())
        .map(|message| match &message["content"] {
            serde_json::Value::String(text) => text.clone(),
            serde_json::Value::Array(parts) => parts
                .iter()
                .filter_map(|part| part["text"].as_str())
                .collect(),
            _ => String::new(),
        })
        .unwrap_or_default();
    !offers_tools && last == alter_zero::llm::agent::CUT_OFF_NOTE
}

/// What a prompt says to make the stand-in delegate, or start a command
/// that outlives the run — words no notice the app writes can contain.
/// Beside `DELEGATE`, `STALL` gives the subagent a task the stand-in never
/// answers in time: an agent that never finishes.
const DELEGATE: &str = "HEADLESS_DELEGATE";
const OUTLIVE: &str = "HEADLESS_OUTLIVE";
const STALL: &str = "HEADLESS_STALL";
/// What a prompt says to make the stand-in run a command that fails, and
/// report usage on every reply of that conversation — what the rollout's
/// `tool` and `usage` lines record (docs/rollout-records.md).
const RECORD: &str = "HEADLESS_RECORD";
/// What a prompt says to make the stand-in's first reply die under it:
/// [`dropped_attempt`] is streamed, then the connection closes mid-body
/// ([`drop_mid_reply`]). The request sent again is answered whole with
/// [`RETRIED_REPLY`] (docs/offline.md *A connection lost mid-reply*).
const DROP: &str = "HEADLESS_DROP";

/// What the stand-in's first reply to a [`DROP`] prompt streams before its
/// connection dies, one delta each: a thought, a finished line, a second
/// thought and the start of another line. With the thinking display on, the
/// second thought finalises the line before it into the history, so by the
/// time it fails the attempt has recorded a reply and two thoughts, and the
/// retry withdraws all of them. Every piece says `draft one`, which nothing
/// else the stand-in or the app writes does ([`has_dropped_text`]).
fn dropped_attempt() -> Vec<serde_json::Value> {
    vec![
        serde_json::json!({"role": "assistant", "reasoning_content": "Pondering draft one."}),
        serde_json::json!({"content": "Draft one: the parser reads tokens.\n"}),
        serde_json::json!({"reasoning_content": "Reconsidering draft one."}),
        serde_json::json!({"content": "Draft one: and then"}),
    ]
}

/// Whether `text` carries anything of [`dropped_attempt`].
fn has_dropped_text(text: &str) -> bool {
    text.to_lowercase().contains("draft one")
}

/// The whole reply the retried request streams.
const RETRIED_REPLY: &str = "Second attempt: the whole reply streams once.";
/// What a conversation says to make its summary stop at the output cap:
/// the summarization answers [`SUMMARY_HEAD`] with a `length` finish, and
/// the continuation request that follows (docs/unfinished-replies.md) is
/// dropped once, after [`SUMMARY_DRAFT`], then answered with
/// [`SUMMARY_TAIL`].
const CUT_SUMMARY: &str = "HEADLESS_CUT_SUMMARY";
/// The summary's first round, cut off at the output cap.
const SUMMARY_HEAD: &str = "The user ran a command that failed. ";
/// What the continuation's dropped attempt streamed.
const SUMMARY_DRAFT: &str = "It exited wi";
/// The continuation that got through.
const SUMMARY_TAIL: &str = "It exited with code 3.";

/// What a prompt says to make the stand-in work through [`ROUNDS`] tool
/// rounds of bulky output — the long agentic turn background compaction
/// exists for (docs/compact.md). Every reply of that conversation reports
/// its prompt as costing a quarter of its body's bytes, the provider count
/// the agent measures itself by.
const COMPACT: &str = "HEADLESS_COMPACT";
/// How many tool rounds a [`COMPACT`] turn runs before it answers.
const ROUNDS: usize = 30;
/// How many lines of prose each round prints — words, so the estimate the
/// agent measures its own appends with agrees with the stand-in's count. A
/// round (~190 tokens) stays well under the twentieth of the test's window
/// between the threshold and the hard limit, so a context that crosses the
/// threshold is checkpointed there and the agent goes on working while the
/// summary is written — rather than every crossing landing past the hard
/// limit, where it waits.
const ROUND_LINES: usize = 12;
/// What a [`COMPACT`] prompt adds to make every round print so much that one
/// round takes the request from under the threshold to past the hard limit
/// — more than the twentieth of the window between them, less than what is
/// left under the threshold after a compaction.
const BIG_ROUNDS: &str = "HEADLESS_BIG_ROUNDS";
/// How many lines of prose a [`BIG_ROUNDS`] round prints.
const BIG_ROUND_LINES: usize = 140;
/// What each round's command prints, ahead of its bulk: which round it was.
const ROUND_MARK: &str = "ROUND-";
/// What a [`COMPACT`] conversation's summarizer answers, ahead of the
/// newest round its snapshot held.
const SUMMARY_OF: &str = "SUMMARY-UP-TO-";

/// The text of a request message — its `content` a string, or text parts.
fn content_text(message: &serde_json::Value) -> String {
    match &message["content"] {
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Array(parts) => parts
            .iter()
            .filter_map(|part| part["text"].as_str())
            .collect(),
        _ => String::new(),
    }
}

/// The numbers that follow `mark` in `text`.
fn marks(text: &str, mark: &str) -> Vec<usize> {
    text.match_indices(mark)
        .filter_map(|(at, _)| {
            text[at + mark.len()..]
                .split(|c: char| !c.is_ascii_digit())
                .next()
                .and_then(|digits| digits.parse().ok())
        })
        .collect()
}

/// The numbers that follow `mark` in a request's messages, sorted, once
/// each.
fn marks_in(body: &str, mark: &str) -> Vec<usize> {
    let mut found: Vec<usize> = request_messages(body)
        .iter()
        .flat_map(|message| marks(&content_text(message), mark))
        .collect();
    found.sort_unstable();
    found.dedup();
    found
}

/// The rounds a request's messages show the output of, by their marks.
fn rounds_in(body: &str) -> Vec<usize> {
    marks_in(body, ROUND_MARK)
}

/// The newest round a summarization request covers: the newest round
/// output it holds, or the one the earlier summary it folds in covered.
fn covered_by(body: &str) -> usize {
    let rounds = rounds_in(body).last().copied().unwrap_or(0);
    let summarized = marks_in(body, SUMMARY_OF).last().copied().unwrap_or(0);
    rounds.max(summarized)
}

/// The bridge text the newest summary the stand-in wrote opens with, by the
/// summarization requests in `log` — `None` before any.
fn newest_bridge(log: &Mutex<Vec<String>>) -> Option<String> {
    let log = log.lock().expect("the log").clone();
    log.iter()
        .filter(|chat| chat.contains(COMPACT) && !offers_tools(chat))
        .map(|chat| covered_by(chat))
        .max()
        .map(|covered| format!("{SUMMARY_OF}{covered}:"))
}

/// A [`COMPACT`] conversation's reply, as a stream with its usage frame:
/// the summarizer names the newest round its snapshot held; the agent runs
/// the round after the one its request ends on — the newest result, or,
/// right after a compaction, the round the summary the request closes on
/// covered — and answers once [`ROUNDS`] have run, but only from a request
/// that carries the newest summary, so no compaction is still on its way
/// when the turn ends (one that is lands at the history's end instead,
/// which the test's last check is not about). A prompt that is not the
/// rounds' is answered at once.
fn compact_sse(body: &str, log: &Mutex<Vec<String>>) -> String {
    let request: serde_json::Value = serde_json::from_str(body).expect("a JSON request");
    let say = |text: String| {
        vec![
            serde_json::json!({"choices": [{"delta": {"content": text}}]}),
            serde_json::json!({"choices": [{"delta": {}, "finish_reason": "stop"}]}),
        ]
    };
    let run = |round: usize, lines: usize| {
        let command = format!(
            "echo {ROUND_MARK}{round}; yes 'the quick brown fox jumps over the lazy dog' | head -n {lines}"
        );
        vec![
            serde_json::json!({"choices": [{"delta": {"tool_calls": [{
                "index": 0, "id": format!("call_round_{round}"), "type": "function",
                "function": {"name": "bash", "arguments": serde_json::json!({
                    "command": command,
                }).to_string()},
            }]}}]}),
            serde_json::json!({"choices": [{"delta": {}, "finish_reason": "tool_calls"}]}),
        ]
    };
    let last = request["messages"]
        .as_array()
        .and_then(|messages| messages.last())
        .cloned()
        .unwrap_or_default();
    let said = content_text(&last);
    let frames = if !offers_tools(body) {
        let covered = covered_by(body);
        say(format!(
            "{SUMMARY_OF}{covered}: the rounds so far printed prose."
        ))
    } else if last["role"] == "user" && !said.contains(COMPACT) {
        say("Done.".to_string())
    } else {
        // A tool result names its round; a user message is the prompt, or
        // — right after a compaction — the head closing on the bridge,
        // whose summary says how far the rounds got.
        let mark = if last["role"] == "tool" {
            ROUND_MARK
        } else {
            SUMMARY_OF
        };
        let done = marks(&said, mark).into_iter().max().unwrap_or(0);
        let lines = if body.contains(BIG_ROUNDS) {
            BIG_ROUND_LINES
        } else {
            ROUND_LINES
        };
        if done < ROUNDS {
            run(done + 1, lines)
        } else {
            // A checkpoint taken at this very round top sends its summary
            // request beside this one: give it the moment to arrive.
            std::thread::sleep(Duration::from_millis(300));
            match newest_bridge(log) {
                Some(bridge) if !body.contains(&bridge) => run(done + 1, 3),
                _ => say(format!("All {ROUNDS} rounds done.")),
            }
        }
    };
    let mut sse: String = frames
        .iter()
        .map(|frame| format!("data: {frame}\n\n"))
        .collect();
    let usage = serde_json::json!({"choices": [], "usage": {
        "prompt_tokens": body.len() / 4, "completion_tokens": 20,
    }});
    sse.push_str(&format!("data: {usage}\n\ndata: [DONE]\n\n"));
    sse
}

/// The stand-in model's turn. After a tool result it answers `Done.`; to a
/// user message it makes one call, chosen by what the message says:
/// [`DELEGATE`] launches a background subagent with the task, [`OUTLIVE`]
/// starts a command that outlives the run, anything else makes the marker.
fn chat_reply(body: &str) -> String {
    let request: serde_json::Value = serde_json::from_str(body).expect("a JSON request");
    let last = request["messages"]
        .as_array()
        .and_then(|messages| messages.last())
        .cloned()
        .unwrap_or_default();
    let said = last["content"].to_string();
    // The stalled subagent's task: answered long after any run waits for it.
    if last["role"] == "user" && said.contains(STALL) && !said.contains(DELEGATE) {
        std::thread::sleep(Duration::from_secs(30));
    }
    let call = |name: &str, arguments: serde_json::Value| {
        vec![
            serde_json::json!({"choices": [{"delta": {"tool_calls": [{
                "index": 0, "id": "call_1", "type": "function",
                "function": {"name": name, "arguments": arguments.to_string()},
            }]}}]}),
            serde_json::json!({"choices": [{"delta": {}, "finish_reason": "tool_calls"}]}),
        ]
    };
    // A request offering no tools is a `/compact` summarization: it is
    // answered in text, never with a call it could not make.
    let offers_tools = request["tools"]
        .as_array()
        .is_some_and(|tools| !tools.is_empty());
    let frames = if continues_a_cut_summary(body) {
        vec![
            serde_json::json!({"choices": [{"delta": {"content": SUMMARY_TAIL}}]}),
            serde_json::json!({"choices": [{"delta": {}, "finish_reason": "stop"}]}),
        ]
    } else if !offers_tools && body.contains(CUT_SUMMARY) {
        // The summary's first round stops at the output cap.
        vec![
            serde_json::json!({"choices": [{"delta": {"content": SUMMARY_HEAD}}]}),
            serde_json::json!({"choices": [{"delta": {}, "finish_reason": "length"}]}),
        ]
    } else if !offers_tools {
        vec![
            serde_json::json!({"choices": [{"delta": {"content": "The user ran a command."}}]}),
            serde_json::json!({"choices": [{"delta": {}, "finish_reason": "stop"}]}),
        ]
    } else if last["role"] == "tool" || said.contains(GENERATED_NOTE) {
        // A round that ends on a generated picture's attachment ends on a
        // user message, the picture itself (docs/imagegen.md).
        vec![
            serde_json::json!({"choices": [{"delta": {"content": "Done."}}]}),
            serde_json::json!({"choices": [{"delta": {}, "finish_reason": "stop"}]}),
        ]
    } else if said.contains(DROP) {
        // The request sent again after the dropped reply: answered whole.
        vec![
            serde_json::json!({"choices": [{"delta": {"content": RETRIED_REPLY}}]}),
            serde_json::json!({"choices": [{"delta": {}, "finish_reason": "stop"}]}),
        ]
    } else if said.contains(DRAW) {
        call("imagegen", serde_json::json!({"prompt": "a fox mascot"}))
    } else if said.contains(DELEGATE) {
        let task = if said.contains(STALL) {
            format!("{STALL} make the marker")
        } else if said.contains(RECORD) {
            // A recorded conversation's subagent is recorded too.
            format!("{RECORD} make the marker")
        } else {
            "make the marker".to_string()
        };
        call(
            "agent",
            serde_json::json!({
                "description": "Make the marker",
                "prompt": task,
                "subagent_type": "general-purpose",
                "run_in_background": true,
            }),
        )
    } else if said.contains(OUTLIVE) {
        call(
            "bash",
            serde_json::json!({"command": "sleep 300", "wait": 0}),
        )
    } else if said.contains(RECORD) {
        call(
            "bash",
            serde_json::json!({"command": "sleep 0.2; echo recorded; exit 3"}),
        )
    } else {
        call(
            "bash",
            serde_json::json!({
                "command": format!("touch {MARKER} && echo made"),
                "description": "Make the marker",
            }),
        )
    };
    let mut sse: String = frames
        .iter()
        .map(|frame| format!("data: {frame}\n\n"))
        .collect();
    // A recorded conversation's every reply ends on its usage frame — the
    // one that read the tool result told apart by its counts.
    if body.contains(RECORD) {
        let usage = if last["role"] == "tool" {
            serde_json::json!({"prompt_tokens": 2400, "completion_tokens": 12,
                "prompt_tokens_details": {"cached_tokens": 2000}})
        } else {
            serde_json::json!({"prompt_tokens": 1800, "completion_tokens": 40,
                "completion_tokens_details": {"reasoning_tokens": 16}})
        };
        let frame = serde_json::json!({"choices": [], "usage": usage});
        sse.push_str(&format!("data: {frame}\n\n"));
    }
    sse.push_str("data: [DONE]\n\n");
    sse
}

/// One request: its head and its sized body. `None` for a connection that
/// closed without one.
fn read_request(stream: &mut TcpStream) -> Option<(String, String)> {
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 8192];
    let header_end = loop {
        let n = stream.read(&mut chunk).ok()?;
        if n == 0 {
            return None;
        }
        bytes.extend_from_slice(&chunk[..n]);
        if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
            break end + 4;
        }
    };
    let head = String::from_utf8_lossy(&bytes[..header_end]).to_string();
    let length: usize = head
        .to_ascii_lowercase()
        .lines()
        .find_map(|line| line.strip_prefix("content-length:"))
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(0);
    while bytes.len() < header_end + length {
        let n = stream.read(&mut chunk).ok()?;
        if n == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..n]);
    }
    let body = String::from_utf8_lossy(&bytes[header_end..]).to_string();
    Some((head, body))
}

/// A headless run against the stand-in, stdin closed.
fn real_run(world: &World, stand_in: &StandIn, args: &[&str]) -> Output {
    let mut command = world.command(args);
    stand_in.configure(world, &mut command);
    command
        .stdin(Stdio::null())
        .output()
        .expect("the binary runs")
}

fn marker_made(world: &World) -> bool {
    Path::new(&world.work()).join(MARKER).exists()
}

#[test]
fn a_call_that_needs_approval_is_refused_and_the_model_is_told() {
    let world = World::new("manual");
    let stand_in = StandIn::start();
    let out = real_run(
        &world,
        &stand_in,
        &[
            "-p",
            "--permission-mode",
            "manual",
            "--output-format",
            "json",
            "make it",
        ],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(!marker_made(&world), "the refused command never ran");
    let result = &events(&out)[0];
    assert_eq!(result["result"], "Done.");
    assert_eq!(result["num_turns"], 2);
    // Claude Code's entry: the tool as the model called it, and its input.
    let denial = &result["permission_denials"][0];
    assert_eq!(denial["tool_name"], "bash");
    assert_eq!(denial["tool_use_id"], "call_1");
    assert_eq!(
        denial["tool_input"]["command"],
        format!("touch {MARKER} && echo made")
    );
    // The model read why, in the tool result of its second request.
    let chats = stand_in.chat_requests();
    assert_eq!(chats.len(), 2, "{chats:?}");
    assert!(chats[1].contains("non-interactive"), "{}", chats[1]);
    // …and it is said on stderr, beside the flag that decides it.
    assert!(stderr(&out).contains("needed approval"), "{}", stderr(&out));
    // AskUserQuestion is never offered to a run nobody can answer.
    assert!(!chats[0].contains("askuserquestion"), "{}", chats[0]);
}

#[test]
fn master_mode_runs_the_call_and_reports_success() {
    let world = World::new("master");
    let stand_in = StandIn::start();
    let out = real_run(
        &world,
        &stand_in,
        &["-p", "--verbose", "--permission-mode", "master", "make it"],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(marker_made(&world), "the command ran");
    assert_eq!(stdout(&out), "Done.\n");
    assert!(
        stderr(&out).contains("● Bash(touch headless-marker && echo made)"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn max_turns_stops_before_the_reply_it_cannot_pay_for() {
    let world = World::new("maxturns");
    let stand_in = StandIn::start();
    let out = real_run(
        &world,
        &stand_in,
        &[
            "-p",
            "--permission-mode",
            "master",
            "--max-turns",
            "1",
            "--output-format",
            "json",
            "make it",
        ],
    );
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let result = &events(&out)[0];
    assert_eq!(result["subtype"], "error_max_turns");
    assert_eq!(result["is_error"], true);
    assert_eq!(result["num_turns"], 1);
    assert_eq!(result["result"], "");
    assert!(
        result["error"].as_str().unwrap().contains("--max-turns"),
        "{result}"
    );
    assert_eq!(
        stand_in.chat_requests().len(),
        1,
        "the second reply was never asked for"
    );
}

#[test]
fn the_system_prompt_tells_the_model_it_runs_headless() {
    let world = World::new("prompt");
    let stand_in = StandIn::start();
    let out = real_run(
        &world,
        &stand_in,
        &["-p", "--permission-mode", "master", "make it"],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    let chats = stand_in.chat_requests();
    assert!(chats[0].contains("## Headless"), "{}", chats[0]);
}

// ===== a connection lost mid-reply (docs/offline.md) =====

/// A reply whose connection dies under it is sent again, and the run
/// reports the reply the retry streamed — never the dropped attempt's draft
/// glued to it — in every format, quiet or logged, and in the rollout a
/// `--continue` reads back (docs/headless.md *Output*: nothing is printed
/// that the session later takes back). The attempt has recorded a reply and
/// two thoughts by the time it fails ([`dropped_attempt`]), so the
/// `stream-json` runs also prove that its items wait for the settled history.
#[test]
fn a_reply_whose_connection_drops_reports_only_what_the_retry_streamed() {
    for format in ["text", "json", "stream-json"] {
        for verbose in [false, true] {
            let run = format!("{format}{}", if verbose { " --verbose" } else { "" });
            let world = World::new("dropped");
            let stand_in = StandIn::start();
            let prompt = format!("{DROP} explain the parser");
            let mut args = vec!["-p", "--output-format", format, prompt.as_str()];
            if verbose {
                args.insert(1, "--verbose");
            }
            let mut command = world.command(&args);
            stand_in.configure(&world, &mut command);
            // The thinking display on: a thought that leaked would be an
            // item of its own.
            let out = command
                .env("ALTER_ZERO_SHOW_THINKING", "1")
                .stdin(Stdio::null())
                .output()
                .expect("the binary runs");
            assert!(out.status.success(), "{run}: {}", stderr(&out));
            assert_eq!(stand_in.chat_requests().len(), 2, "{run}: sent again once");
            let (printed, log) = (stdout(&out), stderr(&out));
            assert!(!has_dropped_text(&printed), "{run}: {printed}");
            assert!(!has_dropped_text(&log), "{run}: {log}");
            // A retry that worked needs nothing from anyone: the step log's
            // news, and quiet otherwise.
            if verbose {
                assert!(log.contains("retrying (1/3)"), "{run}: {log}");
            } else {
                assert_eq!(log, "", "{run}");
            }
            if format == "text" {
                assert_eq!(printed, format!("{RETRIED_REPLY}\n"), "{run}");
            } else {
                let events = events(&out);
                let result = events.last().expect("a result");
                assert_eq!(result["subtype"], "success", "{run}: {result}");
                assert_eq!(result["is_error"], false, "{run}: {result}");
                assert_eq!(result["result"], RETRIED_REPLY, "{run}: {result}");
                if format == "stream-json" {
                    let items: Vec<&serde_json::Value> = events
                        .iter()
                        .filter(|event| event["type"] == "item.completed")
                        .map(|event| &event["item"])
                        .collect();
                    let kinds: Vec<&str> = items
                        .iter()
                        .filter_map(|item| item["type"].as_str())
                        .collect();
                    assert_eq!(kinds, ["user_message", "agent_message"], "{run}: {items:?}");
                    assert_eq!(items[1]["text"], RETRIED_REPLY, "{run}");
                }
            }
            let recorded: String = rollout(&world)
                .iter()
                .map(serde_json::Value::to_string)
                .collect();
            assert!(!has_dropped_text(&recorded), "{run}: {recorded}");
            assert_eq!(
                recorded.matches(RETRIED_REPLY).count(),
                1,
                "{run}: {recorded}"
            );
        }
    }
}

/// The messages of a logged chat request.
fn request_messages(body: &str) -> Vec<serde_json::Value> {
    let request: serde_json::Value = serde_json::from_str(body).expect("a JSON request");
    request["messages"].as_array().cloned().unwrap_or_default()
}

/// Whether a logged chat request is the agent's — it offers tools — rather
/// than a summarizer's.
fn offers_tools(body: &str) -> bool {
    let request: serde_json::Value = serde_json::from_str(body).expect("a JSON request");
    request["tools"]
        .as_array()
        .is_some_and(|tools| !tools.is_empty())
}

/// A long agentic turn on a small window compacts **while it runs**, more
/// than once (docs/compact.md *Background compaction*): each summary covers
/// a snapshot the agent took at a round boundary, the request after it
/// carries the summary bridge and the rounds since verbatim — never the
/// rounds it summarized — and the turn completes. The history then derives
/// exactly what the agent held: a `--continue` run's first request opens on
/// the last request's messages, under another window — the recent user
/// messages a compaction keeps are budgeted by the window it was made in,
/// which its marker records.
#[test]
fn a_long_turn_compacts_in_the_background_and_its_history_derives_what_the_agent_held() {
    let world = World::new("background-compaction");
    let stand_in = StandIn::start();
    // The fixed overhead — system prompt, tool schemas — measured off a
    // first request, so the window leaves room for a handful of rounds.
    let probe = real_run(
        &world,
        &stand_in,
        &["-p", "--permission-mode", "master", "measure"],
    );
    assert!(probe.status.success(), "{}", stderr(&probe));
    let overhead = stand_in.chat_requests()[0].len() / 4;
    let window = overhead + 3_000;
    let _ = std::fs::remove_dir_all(world.dir("sessions"));
    std::fs::create_dir_all(world.dir("sessions")).expect("a sessions dir");
    let before = stand_in.chat_requests().len();
    let run = |window: usize, args: &[&str]| {
        let mut command = world.command(args);
        stand_in.configure(&world, &mut command);
        command
            .env("ALTER_ZERO_CONTEXT_WINDOW", window.to_string())
            .stdin(Stdio::null())
            .output()
            .expect("the binary runs")
    };
    // A prompt longer than the share of the window a compaction keeps of
    // the user's own words, so the head carries it cut down.
    let prompt = format!(
        "{COMPACT} run the rounds. {}",
        "Keep every round's output in mind. ".repeat(120)
    );
    let out = run(
        window,
        &[
            "-p",
            "--verbose",
            "--permission-mode",
            "master",
            "--output-format",
            "stream-json",
            &prompt,
        ],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    let events = events(&out);
    let result = events.last().expect("a result");
    assert_eq!(result["result"], format!("All {ROUNDS} rounds done."));
    let chats: Vec<String> = stand_in.chat_requests()[before..].to_vec();

    // Two compactions or more, each completed mid-turn — before the turn's
    // own end — and each announced as it started and as it was summarized.
    let completed: Vec<&serde_json::Value> = events
        .iter()
        .filter(|e| e["type"] == "item.completed" && e["item"]["type"] == "compaction")
        .collect();
    assert!(completed.len() >= 2, "{events:#?}");
    let turn_end = events
        .iter()
        .position(|e| e["type"] == "turn.completed")
        .expect("the turn completed");
    for compaction in &completed {
        assert_eq!(compaction["item"]["auto"], true);
        let at = events
            .iter()
            .position(|e| std::ptr::eq(e, *compaction))
            .expect("listed");
        assert!(at < turn_end, "compacted mid-turn, not after it");
    }
    assert!(
        events
            .iter()
            .any(|e| e["type"] == "item.started" && e["item"]["type"] == "compaction"),
        "{events:#?}"
    );
    let log = stderr(&out);
    assert!(
        log.contains("Compacting the context in the background"),
        "{log}"
    );
    assert!(log.contains("Compaction summary ready"), "{log}");
    assert!(log.contains("● Context compacted"), "{log}");

    // Each summary covered the rounds its snapshot held; the agent's next
    // request carries its bridge and the rounds since, never those rounds —
    // and at least one was written while the agent went on working.
    let summarizations: Vec<&String> = chats.iter().filter(|chat| !offers_tools(chat)).collect();
    assert!(summarizations.len() >= 2, "{summarizations:#?}");
    let mut worked_on = 0;
    for summarization in &summarizations {
        let covered = covered_by(summarization);
        assert!(covered > 0, "a summary covers rounds: {summarization}");
        let bridge = format!("{SUMMARY_OF}{covered}:");
        let spliced = chats
            .iter()
            .find(|chat| offers_tools(chat) && chat.contains(&bridge))
            .unwrap_or_else(|| panic!("no request carries {bridge}"));
        let rounds = rounds_in(spliced);
        assert!(
            rounds.iter().all(|round| *round > covered),
            "the summarized rounds are gone: {covered} {rounds:?}"
        );
        let newest = rounds.last().copied().unwrap_or(covered);
        assert_eq!(
            rounds,
            (covered + 1..=newest).collect::<Vec<_>>(),
            "every round since the checkpoint, verbatim"
        );
        worked_on += usize::from(newest > covered);
        assert!(
            spliced.contains("Another language model started to solve this problem"),
            "the bridge opens on codex's prefix"
        );
    }
    assert!(
        worked_on >= 1,
        "no summary was written while the agent worked"
    );
    // From the first compaction on, no request the agent sent is past the
    // window by the count it reports.
    let first_spliced = chats
        .iter()
        .position(|chat| offers_tools(chat) && chat.contains(SUMMARY_OF))
        .expect("a spliced request");
    for chat in chats[first_spliced..]
        .iter()
        .filter(|chat| offers_tools(chat))
    {
        assert!(chat.len() / 4 <= window, "{} > {window}", chat.len() / 4);
    }
    // The rollout keeps each summarizer's usage — tagged a compaction's,
    // between its checkpoint and the marker it made.
    let lines = rollout(&world);
    let cut = lines
        .iter()
        .position(|line| line["type"] == "compaction_cut")
        .unwrap_or_else(|| panic!("a checkpoint: {lines:#?}"));
    let marker = lines
        .iter()
        .position(|line| line["type"] == "compaction")
        .unwrap_or_else(|| panic!("a marker: {lines:#?}"));
    assert_eq!(
        lines[marker]["payload"]["cut"], lines[cut]["payload"]["id"],
        "the marker names its checkpoint"
    );
    assert!(
        lines[cut..marker]
            .iter()
            .any(|line| line["type"] == "usage" && line["payload"]["compact"] == true),
        "{lines:#?}"
    );

    // The history derives what the agent held: a new run continuing the
    // conversation opens on the last request's messages, then the answer and
    // the new prompt — under a window big enough that nothing compacts
    // before it goes out. What each request renders afresh for its own
    // process and window is left out of the comparison: the system prompt
    // names the process's scratchpad, and the `<system-reminder>`'s agent
    // listing is budgeted by the window.
    let last_turn_request = chats
        .iter()
        .rev()
        .find(|chat| offers_tools(chat))
        .expect("the agent's last request")
        .clone();
    let held = conversation_part(&request_messages(&last_turn_request));
    assert!(
        !last_turn_request.contains(&prompt),
        "the head keeps the prompt cut down to the window's share"
    );
    let resumed = run(
        1_000_000,
        &[
            "-p",
            "-c",
            "--permission-mode",
            "master",
            "and now say done",
        ],
    );
    assert!(resumed.status.success(), "{}", stderr(&resumed));
    let after: Vec<String> = stand_in.chat_requests()[before + chats.len()..].to_vec();
    let next = after
        .iter()
        .find(|chat| offers_tools(chat))
        .expect("the continued run's request");
    let derived = conversation_part(&request_messages(next));
    assert!(derived.len() > held.len(), "{derived:#?}");
    assert_eq!(
        derived[..held.len()],
        held[..],
        "the history derives exactly what the agent held"
    );
}

/// A round that takes the request from under the threshold to past the hard
/// limit at once is the newest thing the agent has: the checkpoint the agent
/// forces there reaches back to where the last request went out from, so
/// that round reaches the model word for word rather than only through a
/// summary (docs/compact.md *The round that crossed the limit*).
#[test]
fn a_round_that_crosses_the_hard_limit_reaches_the_model_word_for_word() {
    let world = World::new("crossing-round");
    let stand_in = StandIn::start();
    let probe = real_run(
        &world,
        &stand_in,
        &["-p", "--permission-mode", "master", "measure"],
    );
    assert!(probe.status.success(), "{}", stderr(&probe));
    let overhead = stand_in.chat_requests()[0].len() / 4;
    let window = overhead + 3_000;
    let _ = std::fs::remove_dir_all(world.dir("sessions"));
    std::fs::create_dir_all(world.dir("sessions")).expect("a sessions dir");
    let before = stand_in.chat_requests().len();
    let mut command = world.command(&[
        "-p",
        "--permission-mode",
        "master",
        &format!("{COMPACT} {BIG_ROUNDS} run the rounds"),
    ]);
    stand_in.configure(&world, &mut command);
    let out = command
        .env("ALTER_ZERO_CONTEXT_WINDOW", window.to_string())
        .stdin(Stdio::null())
        .output()
        .expect("the binary runs");
    assert!(out.status.success(), "{}", stderr(&out));
    let chats: Vec<String> = stand_in.chat_requests()[before..].to_vec();
    let agent: Vec<&String> = chats.iter().filter(|chat| offers_tools(chat)).collect();
    for round in 1..=ROUNDS {
        let mark = format!("{ROUND_MARK}{round}\n");
        assert!(
            agent.iter().any(|chat| {
                request_messages(chat).iter().any(|message| {
                    message["role"] == "tool" && content_text(message).contains(&mark)
                })
            }),
            "round {round}'s output never reached the model itself"
        );
    }
    for chat in &agent {
        assert!(chat.len() / 4 <= window, "{} > {window}", chat.len() / 4);
    }
    let lines = rollout(&world);
    assert!(
        lines.iter().any(|line| line["type"] == "compaction_cut"
            && line["payload"]["back"]
                .as_u64()
                .is_some_and(|back| back > 0)),
        "a checkpoint reached back over the round that crossed: {lines:#?}"
    );
}

/// A request's messages past its system prompt, each `<system-reminder>`
/// block taken out of their text.
fn conversation_part(messages: &[serde_json::Value]) -> Vec<serde_json::Value> {
    fn without_reminder(text: &str) -> String {
        let mut text = text.to_string();
        while let Some(start) = text.find("<system-reminder>") {
            let Some(end) = text[start..].find("</system-reminder>") else {
                break;
            };
            let mut end = start + end + "</system-reminder>".len();
            if text[end..].starts_with("\n\n") {
                end += 2;
            }
            text.replace_range(start..end, "");
        }
        text
    }
    messages
        .iter()
        .skip(1)
        .map(|message| {
            let mut message = message.clone();
            match &mut message["content"] {
                serde_json::Value::String(text) => *text = without_reminder(text),
                serde_json::Value::Array(parts) => {
                    for part in parts {
                        if let Some(text) = part["text"].as_str() {
                            part["text"] = serde_json::Value::String(without_reminder(text));
                        }
                    }
                }
                _ => {}
            }
            message
        })
        .collect()
}

/// A summary the output cap cut off goes on in another round
/// (docs/unfinished-replies.md). A connection lost in that round takes back
/// what its own attempt streamed and nothing the round before it wrote, so
/// the compaction a `-c` run makes before its prompt keeps the whole summary
/// (docs/compact.md). The first run has room to spare, so nothing compacts
/// while it works.
#[test]
fn a_retry_in_a_summarys_second_round_keeps_its_first() {
    let world = World::new("cut-summary");
    let stand_in = StandIn::start();
    let run = |window: &str, args: &[&str]| {
        let mut command = world.command(args);
        stand_in.configure(&world, &mut command);
        command
            .env("ALTER_ZERO_CONTEXT_WINDOW", window)
            .stdin(Stdio::null())
            .output()
            .expect("the binary runs")
    };
    let first = run(
        "1000000",
        &[
            "-p",
            "--permission-mode",
            "master",
            &format!("{RECORD} {CUT_SUMMARY} run it"),
        ],
    );
    assert!(first.status.success(), "{}", stderr(&first));
    let second = run(
        "2000",
        &[
            "-p",
            "-c",
            "--permission-mode",
            "master",
            &format!("{RECORD} again"),
        ],
    );
    assert!(second.status.success(), "{}", stderr(&second));
    assert_cut_summary_kept(&world, &stand_in);
}

/// The same summary written in the background, while the turn runs: the
/// turn ends before it does, the compaction goes on in the turn's stead —
/// and the retry still takes back only what its own attempt streamed
/// (docs/compact.md *A turn that ends mid-summary*).
#[test]
fn a_retry_in_a_background_summarys_second_round_keeps_its_first() {
    let world = World::new("cut-background-summary");
    let stand_in = StandIn::start();
    let mut command = world.command(&[
        "-p",
        "--permission-mode",
        "master",
        &format!("{RECORD} {CUT_SUMMARY} run it"),
    ]);
    stand_in.configure(&world, &mut command);
    let out = command
        .env("ALTER_ZERO_CONTEXT_WINDOW", "2000")
        .stdin(Stdio::null())
        .output()
        .expect("the binary runs");
    assert!(out.status.success(), "{}", stderr(&out));
    assert_cut_summary_kept(&world, &stand_in);
    let lines = rollout(&world);
    let cut = lines
        .iter()
        .position(|line| line["type"] == "compaction_cut")
        .unwrap_or_else(|| panic!("the agent took a checkpoint: {lines:#?}"));
    let marker = lines
        .iter()
        .position(|line| line["type"] == "compaction")
        .expect("a marker");
    assert!(cut < marker, "{lines:#?}");
    assert_eq!(lines[marker]["payload"]["cut"], lines[cut]["payload"]["id"]);
}

/// The cut summary's continuation was dropped once and sent again, and the
/// conversation's first compaction kept the whole summary.
fn assert_cut_summary_kept(world: &World, stand_in: &StandIn) {
    let continuations = stand_in
        .chat_requests()
        .iter()
        .filter(|chat| continues_a_cut_summary(chat))
        .count();
    assert_eq!(
        continuations, 2,
        "the continuation was dropped, then sent again"
    );
    let lines = rollout(world);
    let summary = lines
        .iter()
        .find(|line| line["type"] == "compaction")
        .unwrap_or_else(|| panic!("the conversation compacted: {lines:#?}"))["payload"]["summary"]
        .as_str()
        .expect("a summary")
        .to_string();
    assert_eq!(summary, format!("{SUMMARY_HEAD}{SUMMARY_TAIL}"));
}

// ===== the system prompt flags (docs/system-prompt.md) =====

/// The system message's text of a logged chat request — its `content` a
/// string, or an array of text parts.
fn system_text(body: &str) -> String {
    let request: serde_json::Value = serde_json::from_str(body).expect("a JSON request");
    let system = request["messages"]
        .as_array()
        .and_then(|messages| messages.iter().find(|m| m["role"] == "system"))
        .unwrap_or_else(|| panic!("no system message: {body}"));
    match &system["content"] {
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Array(parts) => parts
            .iter()
            .filter_map(|part| part["text"].as_str())
            .collect(),
        other => panic!("a system content of {other}"),
    }
}

/// Where `needle` sits in `text`, which must contain it.
fn at(text: &str, needle: &str) -> usize {
    text.find(needle)
        .unwrap_or_else(|| panic!("{needle:?} is missing from:\n{text}"))
}

/// The first chat request's system text, after a run that succeeded.
fn first_system_text(out: &Output, stand_in: &StandIn) -> String {
    assert!(out.status.success(), "{}", stderr(out));
    let chats = stand_in.chat_requests();
    system_text(chats.first().expect("a chat request"))
}

#[test]
fn appended_text_sits_between_the_persona_and_the_environment() {
    let world = World::new("sysappend");
    let stand_in = StandIn::start();
    let out = real_run(
        &world,
        &stand_in,
        &[
            "-p",
            "--permission-mode",
            "master",
            "--append-system-prompt",
            "APPENDED_MARKER rules",
            "make it",
        ],
    );
    let text = first_system_text(&out, &stand_in);
    let persona = at(&text, "You are Alter Zero");
    let appended = at(&text, "APPENDED_MARKER rules");
    let environment = at(&text, "## Environment");
    let headless = at(&text, "## Headless");
    assert!(
        persona < appended && appended < environment && environment < headless,
        "{text}"
    );
}

#[test]
fn a_replaced_persona_keeps_the_blocks_after_it() {
    let world = World::new("sysreplace");
    let stand_in = StandIn::start();
    let out = real_run(
        &world,
        &stand_in,
        &[
            "-p",
            "--permission-mode",
            "master",
            "--system-prompt",
            "REPLACED_MARKER persona",
            "make it",
        ],
    );
    let text = first_system_text(&out, &stand_in);
    assert!(text.starts_with("REPLACED_MARKER persona"), "{text}");
    assert!(!text.contains("You are Alter Zero"), "{text}");
    at(&text, "## Environment");
    at(&text, "## Headless");
}

#[test]
fn the_flag_outranks_the_variable_and_an_append_adds_to_the_variables_persona() {
    let world = World::new("sysenv");
    let stand_in = StandIn::start();
    let run = |args: &[&str]| {
        let mut command = world.command(args);
        stand_in.configure(&world, &mut command);
        command
            .env("ALTER_ZERO_SYSTEM_PROMPT", "ENV_MARKER persona")
            .stdin(Stdio::null())
            .output()
            .expect("the binary runs")
    };
    let out = run(&[
        "-p",
        "--permission-mode",
        "master",
        "--system-prompt",
        "FLAG_MARKER persona",
        "make it",
    ]);
    assert!(out.status.success(), "{}", stderr(&out));
    let text = system_text(&stand_in.chat_requests()[0]);
    assert!(text.starts_with("FLAG_MARKER persona"), "{text}");
    assert!(!text.contains("ENV_MARKER"), "{text}");

    let seen = stand_in.chat_requests().len();
    let out = run(&[
        "-p",
        "--permission-mode",
        "master",
        "--append-system-prompt",
        "APPENDED_MARKER",
        "make it",
    ]);
    assert!(out.status.success(), "{}", stderr(&out));
    let text = system_text(&stand_in.chat_requests()[seen]);
    assert!(text.starts_with("ENV_MARKER persona"), "{text}");
    assert!(
        at(&text, "ENV_MARKER") < at(&text, "APPENDED_MARKER")
            && at(&text, "APPENDED_MARKER") < at(&text, "## Environment"),
        "{text}"
    );
    assert!(!text.contains("You are Alter Zero"), "{text}");
}

#[test]
fn the_file_flags_read_relative_to_cwd() {
    let world = World::new("sysfiles");
    let stand_in = StandIn::start();
    let prompts = world.dir("prompts");
    std::fs::create_dir_all(&prompts).expect("the prompts dir");
    std::fs::write(prompts.join("persona.md"), "FILE_PERSONA_MARKER\n").expect("a file");
    std::fs::write(prompts.join("extra.md"), "FILE_APPEND_MARKER\n").expect("a file");
    let out = real_run(
        &world,
        &stand_in,
        &[
            "-p",
            "--permission-mode",
            "master",
            "-C",
            prompts.to_str().unwrap(),
            "--system-prompt-file",
            "persona.md",
            "--append-system-prompt-file=extra.md",
            "make it",
        ],
    );
    let text = first_system_text(&out, &stand_in);
    assert!(text.starts_with("FILE_PERSONA_MARKER"), "{text}");
    assert!(
        at(&text, "FILE_APPEND_MARKER") < at(&text, "## Environment"),
        "{text}"
    );
    assert!(!text.contains("You are Alter Zero"), "{text}");
}

#[test]
fn an_unreadable_system_prompt_file_sends_nothing_and_is_one_error_result() {
    let world = World::new("sysunreadable");
    let stand_in = StandIn::start();
    let out = real_run(
        &world,
        &stand_in,
        &[
            "-p",
            "--output-format",
            "json",
            "--append-system-prompt-file",
            "missing.md",
            "make it",
        ],
    );
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(stand_in.chat_requests().is_empty(), "no request was sent");
    let lines = events(&out);
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert_eq!(lines[0]["type"], "result");
    assert_eq!(lines[0]["is_error"], true);
    assert!(
        lines[0]["error"]
            .as_str()
            .is_some_and(|text| text.contains("Cannot read --append-system-prompt-file missing.md")),
        "{}",
        lines[0]
    );
}

#[test]
fn the_wait_on_a_background_agent_that_never_finishes_is_bounded() {
    let world = World::new("stuckagent");
    let stand_in = StandIn::start();
    let mut command = world.command(&[
        "-p",
        "--permission-mode",
        "master",
        &format!("{DELEGATE} {STALL}"),
    ]);
    stand_in.configure(&world, &mut command);
    let began = Instant::now();
    let out = command
        .env("ALTER_ZERO_HEADLESS_AGENT_WAIT_SECS", "1")
        .stdin(Stdio::null())
        .output()
        .expect("the binary runs");
    assert!(
        began.elapsed() < Duration::from_secs(20),
        "the run waited out the stalled agent: {:?}",
        began.elapsed()
    );
    // It ends on the lead's last reply, and says what it stopped.
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out), "Done.\n");
    assert!(
        stderr(&out).contains("stopped 1 background agent still running: Make the marker"),
        "{}",
        stderr(&out)
    );
    assert!(
        !marker_made(&world),
        "the stalled agent never ran its command"
    );
}

#[test]
fn the_run_waits_for_a_background_agent_and_answers_on_its_result() {
    // "Launch the critic agent, write a report": the lead's turn ends with
    // the agent still working in the background; the run waits for it and
    // the turn its completion starts is the one that answers.
    let world = World::new("agent");
    let stand_in = StandIn::start();
    let out = real_run(
        &world,
        &stand_in,
        &[
            "-p",
            "--permission-mode",
            "master",
            "--output-format",
            "stream-json",
            &format!("{DELEGATE} the marker"),
        ],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(marker_made(&world), "the subagent ran its command");
    let events = events(&out);
    let kinds: Vec<&str> = events
        .iter()
        .filter_map(|e| e["item"]["type"].as_str())
        .collect();
    assert!(kinds.contains(&"subagents"), "{kinds:?}");
    assert!(kinds.contains(&"agent_notice"), "{kinds:?}");
    // The lead's turn, then the one the agent's completion started.
    let turns = events
        .iter()
        .filter(|e| e["type"] == "turn.completed")
        .count();
    assert_eq!(turns, 2, "{events:?}");
    let result = events.last().expect("a result");
    assert_eq!(result["subtype"], "success");
    assert_eq!(result["result"], "Done.");
    assert!(!stderr(&out).contains("stopped"), "{}", stderr(&out));
}

#[test]
fn a_command_still_running_at_the_end_is_stopped_and_named() {
    let world = World::new("shell");
    let stand_in = StandIn::start();
    let began = Instant::now();
    let out = real_run(
        &world,
        &stand_in,
        &[
            "-p",
            "--permission-mode",
            "master",
            &format!("{OUTLIVE} please"),
        ],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        began.elapsed() < Duration::from_secs(60),
        "the run never waits on a command that does not end"
    );
    assert!(
        stderr(&out).contains("stopped 1 background command still running: sleep 300"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn a_signal_stops_the_run_and_still_reports() {
    let world = World::new("signal");
    let child = world
        .command(&["-p", "--output-format", "stream-json", "hello there"])
        .env("ALTER_ZERO_DUMMY", "1")
        // The demo waits before its first word: long enough to be stopped.
        .env("ALTER_ZERO_STARTUP_DELAY_MS", "30000")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the binary runs");
    // Stopped once the run has announced itself.
    std::thread::sleep(Duration::from_secs(1));
    let sent = Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .expect("kill runs");
    assert!(sent.success());
    let out = child.wait_with_output().expect("the run ends");
    assert_eq!(out.status.code(), Some(143), "{}", stderr(&out));
    let events = events(&out);
    let result = events.last().expect("a result, even interrupted");
    assert_eq!(result["type"], "result");
    assert_eq!(result["subtype"], "error_interrupted");
    assert_eq!(result["error"], "interrupted by SIGTERM");
}

// ===== what the rollout records (docs/rollout-records.md) =====

/// Every line of the one rollout file a world's runs wrote, parsed.
fn rollout(world: &World) -> Vec<serde_json::Value> {
    fn files(dir: &Path, found: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                files(&path, found);
            } else if path.extension().is_some_and(|ext| ext == "jsonl") {
                found.push(path);
            }
        }
    }
    let mut found = Vec::new();
    files(&world.dir("sessions"), &mut found);
    let [file] = found.as_slice() else {
        panic!("one rollout: {found:?}");
    };
    std::fs::read_to_string(file)
        .expect("the rollout reads")
        .lines()
        .map(|line| serde_json::from_str(line).expect("a JSON line"))
        .collect()
}

#[test]
fn the_rollout_records_each_request_and_how_each_call_ran() {
    let world = World::new("records");
    let stand_in = StandIn::start();
    let out = real_run(
        &world,
        &stand_in,
        &[
            "-p",
            "--permission-mode",
            "master",
            &format!("{RECORD} run it"),
        ],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stand_in.chat_requests().len(), 2);
    let lines = rollout(&world);
    let kinds: Vec<&str> = lines
        .iter()
        .filter_map(|line| line["type"].as_str())
        .filter(|kind| ["message", "tool", "usage", "summary"].contains(kind))
        .collect();
    // The request that asked for the call heads it, the one that read its
    // result follows it: a step's cost reads off file order.
    assert_eq!(
        kinds,
        ["message", "usage", "tool", "usage", "message", "summary"],
        "{lines:#?}"
    );
    let usage: Vec<&serde_json::Value> = lines
        .iter()
        .filter(|line| line["type"] == "usage")
        .map(|line| &line["payload"])
        .collect();
    assert_eq!(
        (
            &usage[0]["input"],
            &usage[0]["output"],
            &usage[0]["reasoning"]
        ),
        (&1800.into(), &40.into(), &16.into())
    );
    assert_eq!(
        (&usage[1]["input"], &usage[1]["output"], &usage[1]["cached"]),
        (&2400.into(), &12.into(), &2000.into())
    );
    assert!(usage.iter().all(|line| line.get("agent").is_none()));
    // The call ran and failed: its exit code, its runtime and why, as data.
    let tool = &lines
        .iter()
        .find(|line| line["type"] == "tool")
        .expect("the call")["payload"];
    assert_eq!(tool["ok"], false);
    assert_eq!(tool["exit_code"], 3);
    assert_eq!(tool["error_kind"], "tool_error");
    assert!(
        tool["duration_ms"].as_u64().is_some_and(|ms| ms >= 200),
        "{tool}"
    );
    assert!(
        tool["output"]
            .as_str()
            .is_some_and(|out| out.contains("Exit code: 3")),
        "the text the model read is unchanged: {tool}"
    );
    // …and the turn's summary is still the sum of its requests.
    let summary = &lines
        .iter()
        .find(|line| line["type"] == "summary")
        .expect("the summary")["payload"];
    assert_eq!(summary["tokens"], 1800 + 40 + 2400 + 12);
    assert_eq!(summary["cached"], 2000);

    // A continued conversation keeps the file's lines where they were and
    // appends its own the same way.
    let out = real_run(
        &world,
        &stand_in,
        &[
            "-p",
            "-c",
            "--permission-mode",
            "master",
            &format!("{RECORD} again"),
        ],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    let kinds: Vec<String> = rollout(&world)
        .iter()
        .filter_map(|line| line["type"].as_str().map(str::to_string))
        .filter(|kind| ["message", "tool", "usage", "summary"].contains(&kind.as_str()))
        .collect();
    let turn = ["message", "usage", "tool", "usage", "message", "summary"];
    assert_eq!(kinds, [turn, turn].concat());
}

#[test]
fn a_subagents_requests_are_recorded_under_its_id() {
    // A subagent's requests are paid for by the conversation that launched
    // it: their lines land in its rollout, tagged with the id its
    // `agent_group` entry carries (docs/rollout-records.md).
    let world = World::new("agent-usage");
    let stand_in = StandIn::start();
    let out = real_run(
        &world,
        &stand_in,
        &[
            "-p",
            "--permission-mode",
            "master",
            &format!("{DELEGATE} {RECORD} go"),
        ],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    let lines = rollout(&world);
    let group: Vec<&str> = lines
        .iter()
        .filter(|line| line["type"] == "agent_group")
        .flat_map(|line| line["payload"]["agents"].as_array().expect("agents").iter())
        .filter_map(|agent| agent["id"].as_str())
        .collect();
    let tagged: Vec<&str> = lines
        .iter()
        .filter(|line| line["type"] == "usage")
        .filter_map(|line| line["payload"]["agent"].as_str())
        .collect();
    // The subagent's call and its answer: two requests, both its own.
    assert_eq!(tagged.len(), 2, "{lines:#?}");
    assert!(
        tagged.iter().all(|id| group == [*id]),
        "{tagged:?} vs {group:?}"
    );
}

#[test]
fn a_compactions_request_is_recorded_as_one() {
    // A conversation continued past its window compacts before the prompt
    // goes out; that summarization's usage line says so and sits right
    // ahead of the compaction it made (docs/rollout-records.md).
    let world = World::new("compact-usage");
    let stand_in = StandIn::start();
    // The first run has room to spare; the second's window is past.
    let run = |window: &str, args: &[&str]| {
        let mut command = world.command(args);
        stand_in.configure(&world, &mut command);
        command
            .env("ALTER_ZERO_CONTEXT_WINDOW", window)
            .stdin(Stdio::null())
            .output()
            .expect("the binary runs")
    };
    let first = run(
        "1000000",
        &[
            "-p",
            "--permission-mode",
            "master",
            &format!("{RECORD} run it"),
        ],
    );
    assert!(first.status.success(), "{}", stderr(&first));
    let second = run(
        "2000",
        &[
            "-p",
            "-c",
            "--permission-mode",
            "master",
            &format!("{RECORD} again"),
        ],
    );
    assert!(second.status.success(), "{}", stderr(&second));
    let lines = rollout(&world);
    let compaction = lines
        .iter()
        .position(|line| line["type"] == "compaction")
        .unwrap_or_else(|| panic!("the conversation compacted: {lines:#?}"));
    assert_eq!(lines[compaction - 1]["type"], "usage", "{lines:#?}");
    assert_eq!(lines[compaction - 1]["payload"]["compact"], true);
    // Every other request is the conversation's own, untagged.
    let tagged = lines
        .iter()
        .filter(|line| line["type"] == "usage" && line["payload"].get("compact").is_some())
        .count();
    assert_eq!(tagged, 1, "{lines:#?}");
}

#[test]
fn the_rollout_says_why_a_call_never_ran() {
    // A headless run refuses what needs an approval nobody can give: the
    // call's line says so, with no exit status and no time — it never ran
    // (docs/rollout-records.md).
    let world = World::new("refusal-kind");
    let stand_in = StandIn::start();
    let out = real_run(
        &world,
        &stand_in,
        &["-p", "--permission-mode", "manual", "make it"],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    let lines = rollout(&world);
    let call = &lines
        .iter()
        .find(|line| line["type"] == "tool")
        .expect("the refused call is recorded")["payload"];
    assert_eq!(call["ok"], false);
    assert_eq!(call["error_kind"], "approval_required", "{call}");
    assert!(call.get("exit_code").is_none(), "{call}");
    assert!(call.get("duration_ms").is_none(), "{call}");
}

// ===== image generation through a ChatGPT sign-in (docs/imagegen.md) =====

/// What a prompt says to make the stand-in model draw.
const DRAW: &str = "HEADLESS_DRAW";

/// What the note a generated picture rides under says, which tells the
/// stand-in model its round is over.
const GENERATED_NOTE: &str = "from the imagegen tool call above";

/// What the stand-in's OpenAI mints for a refresh: an unsigned token whose
/// claims name the account. The app reads them for the `chatgpt-account-id`
/// header and never checks the signature — the server does.
const CHATGPT_ACCESS: &str = "eyJhbGciOiJub25lIiwidHlwIjoiSldUIn0.eyJodHRwczovL2FwaS5vcGVuYWkuY29tL2F1dGgiOnsiY2hhdGdwdF9hY2NvdW50X2lkIjoiYWNjdC1oZWFkbGVzcyIsImNoYXRncHRfcGxhbl90eXBlIjoicGx1cyJ9fQ.c2ln";

/// A 2x2 PNG, as the Images API's `b64_json` carries it — 74 bytes decoded.
const PIXEL_PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAYAAABytg0kAAAAEUlEQVR4nGP4X6HxH4QZYAwAXuoKeUwHsUUAAAAASUVORK5CYII=";

/// A stand-in for OpenAI's auth server and ChatGPT's backend on one port:
/// `POST /oauth/token` mints [`CHATGPT_ACCESS`], anything else is answered
/// as the Images API with [`PIXEL_PNG`]. Every request's head and body is
/// kept.
struct ChatGpt {
    base: String,
    requests: Arc<Mutex<Vec<(String, String)>>>,
}

impl ChatGpt {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
        let base = format!("http://{}", listener.local_addr().expect("an address"));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&requests);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let Some((head, body)) = read_request(&mut stream) else {
                    continue;
                };
                let reply = if head.starts_with("POST /oauth/token ") {
                    format!(r#"{{"access_token":"{CHATGPT_ACCESS}","expires_in":3600}}"#)
                } else {
                    format!(r#"{{"created":1,"data":[{{"b64_json":"{PIXEL_PNG}"}}]}}"#)
                };
                log.lock().expect("the log").push((head, body));
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{reply}",
                    reply.len()
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
            }
        });
        Self { base, requests }
    }

    fn requests(&self) -> Vec<(String, String)> {
        self.requests.lock().expect("the log").clone()
    }
}

/// A headless run asking the stand-in chat provider's model to draw, with
/// the ChatGPT provider pointed at `chatgpt` — and, when `signed_in`, a
/// ChatGPT sign-in stored for it.
fn draw_run(world: &World, chat: &StandIn, chatgpt: &ChatGpt, signed_in: bool) -> Output {
    draw_command(world, chat, chatgpt, signed_in)
        .stdin(Stdio::null())
        .output()
        .expect("the binary runs")
}

/// [`draw_run`]'s command, for a run that adds to its environment.
fn draw_command(world: &World, chat: &StandIn, chatgpt: &ChatGpt, signed_in: bool) -> Command {
    let file = world.dir("providers.toml");
    std::fs::write(
        &file,
        format!(
            "[providers.stub]\nname = \"Stub\"\n\n[providers.stub.kwargs]\napi_base = \"{}\"\n\n\
             [providers.chatgpt_codex]\nname = \"ChatGPT Codex\"\nauth = \"chatgpt_codex\"\n\
             wire_api = \"responses\"\napi_key_env = \"CHATGPT_CODEX_REFRESH_TOKEN\"\n\n\
             [providers.chatgpt_codex.extra_headers]\noriginator = \"codex_cli_rs\"\n\n\
             [providers.chatgpt_codex.kwargs]\napi_base = \"{}/backend-api/codex\"\n",
            chat.base, chatgpt.base
        ),
    )
    .expect("the providers file");
    let prompt = format!("{DRAW} draw a fox");
    let mut command = world.command(&["-p", &prompt]);
    command
        .env("ALTER_ZERO_PROVIDERS_FILE", file)
        .env("ALTER_ZERO_PROVIDER", "stub")
        .env("ALTER_ZERO_MODEL", "stub-model")
        .env("STUB_API_KEY", "stand-in")
        .env("ALTER_ZERO_OPENAI_ISSUER", &chatgpt.base);
    if signed_in {
        command.env("CHATGPT_CODEX_REFRESH_TOKEN", "rt-headless");
    }
    command
}

/// Every generated picture saved under the world's config home.
fn generated_pictures(world: &World) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let Ok(sessions) = std::fs::read_dir(world.dir("cfg").join("generated_images")) else {
        return found;
    };
    for session in sessions.flatten() {
        for file in std::fs::read_dir(session.path())
            .into_iter()
            .flatten()
            .flatten()
        {
            if file.path().is_file() {
                found.push(file.path());
            }
        }
    }
    found
}

/// The tools a chat request offered, by name.
fn offered_tools(request: &str) -> Vec<String> {
    let request: serde_json::Value = serde_json::from_str(request).expect("a JSON request");
    request["tools"]
        .as_array()
        .map(|tools| {
            tools
                .iter()
                .filter_map(|tool| tool["function"]["name"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn a_chatgpt_sign_in_lets_another_providers_model_draw() {
    let world = World::new("draw");
    let chat = StandIn::start();
    let chatgpt = ChatGpt::start();
    let out = draw_run(&world, &chat, &chatgpt, true);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "Done.", "{}", stderr(&out));
    let chats = chat.chat_requests();
    assert_eq!(chats.len(), 2, "{chats:?}");
    // The tool rode the stand-in provider's own request: the model that
    // drew is not ChatGPT's.
    assert!(
        offered_tools(&chats[0]).contains(&"imagegen".to_string()),
        "{:?}",
        offered_tools(&chats[0])
    );
    // The sign-in was minted, and the picture asked for as Codex asks.
    let requests = chatgpt.requests();
    assert_eq!(requests.len(), 2, "{requests:?}");
    let (head, body) = &requests[0];
    assert!(head.starts_with("POST /oauth/token "), "{head}");
    assert!(body.contains("rt-headless"), "{body}");
    let (head, body) = &requests[1];
    assert!(
        head.starts_with("POST /backend-api/codex/images/generations "),
        "{head}"
    );
    let head = head.to_ascii_lowercase();
    assert!(
        head.contains(&format!(
            "authorization: bearer {}",
            CHATGPT_ACCESS.to_ascii_lowercase()
        )),
        "{head}"
    );
    assert!(head.contains("chatgpt-account-id: acct-headless"), "{head}");
    assert!(head.contains("originator: codex_cli_rs"), "{head}");
    assert!(head.contains("x-codex-image-turn-id: "), "{head}");
    let sent: serde_json::Value = serde_json::from_str(body).expect("a JSON body");
    assert_eq!(
        sent,
        serde_json::json!({"prompt": "a fox mascot", "background": "opaque",
            "model": "gpt-image-2", "quality": "auto", "size": "auto"})
    );
    // The picture landed in the session's generated-images folder…
    let pictures = generated_pictures(&world);
    assert_eq!(pictures.len(), 1, "{pictures:?}");
    assert!(pictures[0].ends_with("1.png"), "{pictures:?}");
    assert_eq!(std::fs::metadata(&pictures[0]).expect("saved").len(), 74);
    // …and the model read where, then saw it.
    let second: serde_json::Value = serde_json::from_str(&chats[1]).expect("a JSON request");
    let messages = second["messages"].as_array().expect("messages");
    let result = messages
        .iter()
        .find(|message| message["role"] == "tool")
        .and_then(|message| message["content"].as_str())
        .expect("the tool result");
    assert!(result.starts_with("Generated image (PNG, 2x2"), "{result}");
    assert!(
        result.contains(&pictures[0].display().to_string()),
        "{result}"
    );
    let attachment = messages.last().expect("the attachment");
    assert_eq!(attachment["role"], "user");
    assert!(
        attachment["content"][0]["text"]
            .as_str()
            .is_some_and(|text| text.contains(GENERATED_NOTE)),
        "{attachment}"
    );
    assert!(
        attachment["content"][1]["image_url"]["url"]
            .as_str()
            .is_some_and(|url| url.starts_with("data:image/png;base64,")),
        "{attachment}"
    );
}

#[test]
fn without_a_chatgpt_sign_in_no_model_is_offered_imagegen() {
    let world = World::new("nodraw");
    let chat = StandIn::start();
    let chatgpt = ChatGpt::start();
    let out = draw_run(&world, &chat, &chatgpt, false);
    assert!(out.status.success(), "{}", stderr(&out));
    let chats = chat.chat_requests();
    assert!(
        !offered_tools(&chats[0]).contains(&"imagegen".to_string()),
        "{:?}",
        offered_tools(&chats[0])
    );
    // A model that names it anyway is told it isn't here, and nothing is
    // asked of ChatGPT.
    assert!(
        chats[1].contains("image generation is not available here"),
        "{}",
        chats[1]
    );
    assert!(chatgpt.requests().is_empty(), "{:?}", chatgpt.requests());
    assert!(generated_pictures(&world).is_empty());
}

#[test]
fn the_image_generation_row_off_withdraws_the_tool_for_the_run() {
    // `ALTER_ZERO_IMAGEGEN=0` seeds the /settings row off: signed in, yet
    // nothing offered and nothing asked of ChatGPT (docs/imagegen.md).
    let world = World::new("rowoff");
    let chat = StandIn::start();
    let chatgpt = ChatGpt::start();
    let out = draw_command(&world, &chat, &chatgpt, true)
        .env("ALTER_ZERO_IMAGEGEN", "0")
        .stdin(Stdio::null())
        .output()
        .expect("the binary runs");
    assert!(out.status.success(), "{}", stderr(&out));
    let chats = chat.chat_requests();
    assert!(
        !offered_tools(&chats[0]).contains(&"imagegen".to_string()),
        "{:?}",
        offered_tools(&chats[0])
    );
    assert!(chatgpt.requests().is_empty(), "{:?}", chatgpt.requests());
}
