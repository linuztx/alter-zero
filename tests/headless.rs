//! `alter-zero -p` against the **real built binary** with no terminal
//! anywhere (`docs/cli/headless.md`): every stream a pipe, the way a cron job, a
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

/// The closing sentence every demo reply ends on (`docs/architecture/dummy-backend.md`).
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
        log.lock().expect("the log").push(body.clone());
        ("text/event-stream", chat_reply(&body))
    };
    let response = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: {kind}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{reply}",
        reply.len()
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
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
/// `tool` and `usage` lines record (docs/state/rollout-records.md).
const RECORD: &str = "HEADLESS_RECORD";

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
    let frames = if !offers_tools {
        vec![
            serde_json::json!({"choices": [{"delta": {"content": "The user ran a command."}}]}),
            serde_json::json!({"choices": [{"delta": {}, "finish_reason": "stop"}]}),
        ]
    } else if last["role"] == "tool" {
        vec![
            serde_json::json!({"choices": [{"delta": {"content": "Done."}}]}),
            serde_json::json!({"choices": [{"delta": {}, "finish_reason": "stop"}]}),
        ]
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

// ===== what the rollout records (docs/state/rollout-records.md) =====

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
    // `agent_group` entry carries (docs/state/rollout-records.md).
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
    // ahead of the compaction it made (docs/state/rollout-records.md).
    let world = World::new("compact-usage");
    let stand_in = StandIn::start();
    let run = |args: &[&str]| {
        let mut command = world.command(args);
        stand_in.configure(&world, &mut command);
        command
            .env("ALTER_ZERO_CONTEXT_WINDOW", "2000")
            .stdin(Stdio::null())
            .output()
            .expect("the binary runs")
    };
    let first = run(&[
        "-p",
        "--permission-mode",
        "master",
        &format!("{RECORD} run it"),
    ]);
    assert!(first.status.success(), "{}", stderr(&first));
    let second = run(&[
        "-p",
        "-c",
        "--permission-mode",
        "master",
        &format!("{RECORD} again"),
    ]);
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
    // (docs/state/rollout-records.md).
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
