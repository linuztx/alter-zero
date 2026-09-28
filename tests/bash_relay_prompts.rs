//! Real executor checks for prompts behind a terminal relay. The fixtures
//! reproduce the cursor hiding, menu collapse and erased rows emitted by
//! Inquirer, Enquirer and prompts, without installing Node packages in CI.

#![cfg(target_os = "linux")]

use std::fs;
use std::process::Command;
use std::sync::mpsc;
use std::time::Duration;

use alter_zero::background::{BackgroundRegistry, BgEvent};
use alter_zero::llm::exec::{RealToolExecutor, ToolExecutor, ToolProgress};
use alter_zero::llm::tools::{ToolCallRequest, ToolOutcome};
use alter_zero::pty::report::{Frame, Waiting, parse_frame};
use alter_zero::stream::CancelToken;
use serde_json::{Value, json};
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

struct Relay {
    root: tempfile::TempDir,
    registry: BackgroundRegistry,
    executor: RealToolExecutor,
    _events: UnboundedReceiver<BgEvent>,
}

impl Relay {
    fn new() -> Option<Self> {
        for program in ["python3", "script"] {
            if !Command::new(program)
                .arg("--version")
                .output()
                .is_ok_and(|output| output.status.success())
            {
                eprintln!("{program} unavailable; skipping relay fixture");
                return None;
            }
        }
        let root = tempfile::tempdir().expect("fixture directory");
        let (events, rx) = unbounded_channel();
        let registry = BackgroundRegistry::new(events, root.path().join("tasks"));
        let executor = RealToolExecutor::new().with_background(registry.clone());
        Some(Self {
            root,
            registry,
            executor,
            _events: rx,
        })
    }

    fn call(&self, name: &str, arguments: Value) -> ToolOutcome {
        self.executor.execute(
            &ToolCallRequest {
                id: name.into(),
                name: name.into(),
                arguments: arguments.to_string(),
            },
            &CancelToken::new(),
            &mut |_| {},
        )
    }

    fn launch(&self, program: &str) -> ToolOutcome {
        let path = self.root.path().join("fixture.py");
        fs::write(&path, program).expect("fixture program");
        let command = format!("python3 {}", quote(&path.to_string_lossy()));
        self.call(
            "bash",
            json!({"command": format!("script -qfec {} /dev/null", quote(&command)), "wait": 2}),
        )
    }

    fn send(&self, session: &str, input: &str) -> ToolOutcome {
        self.call("bashsend", json!({"session_id": session, "input": input}))
    }
}

impl Drop for Relay {
    fn drop(&mut self) {
        self.registry.kill_all();
    }
}

fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn session(output: &ToolOutcome, waiting: Waiting) -> String {
    assert!(output.ok, "{}", output.output);
    match output.output.lines().next().and_then(parse_frame) {
        Some(Frame::Running {
            session,
            waiting: actual,
        }) => {
            assert_eq!(actual, waiting, "{}", output.output);
            session.to_string()
        }
        frame => panic!(
            "expected a running session, got {frame:?}: {}",
            output.output
        ),
    }
}

fn finished(output: &ToolOutcome) {
    assert!(output.ok, "{}", output.output);
    assert!(
        output.output.starts_with("Exit code: 0\n"),
        "{}",
        output.output
    );
    assert!(output.output.contains("DONE"), "{}", output.output);
}

const READ_KEYS: &str = r#"import os, termios
mode = termios.tcgetattr(0)
mode[3] &= ~(termios.ECHO | termios.ICANON)
termios.tcsetattr(0, termios.TCSANOW, mode)
"#;

#[test]
fn a_hidden_menu_behind_a_relay_waits_for_input() {
    let Some(relay) = Relay::new() else { return };
    let program = format!(
        "{READ_KEYS}{}",
        r#"os.write(1, b'\x1b[?25l? Choose flavor\r\n> Vanilla\r\n  Chocolate\x1b[2A\x1b[20G')
os.read(0, 1)
os.write(1, b'\r\nDONE\r\n')
"#
    );
    let launched = relay.launch(&program);
    let id = session(&launched, Waiting::Input);
    assert!(launched.output.contains("Vanilla"), "{}", launched.output);
    finished(&relay.send(&id, "<Enter>"));
}

#[test]
fn collapsed_menu_rows_do_not_hide_later_questions() {
    let Some(relay) = Relay::new() else { return };
    let program = format!(
        "{READ_KEYS}{}",
        r#"os.write(1, b'? Choose flavor\r\n> Vanilla\r\n  Chocolate\r\n  Mint\r\n  Coffee\x1b[4A\x1b[20G')
os.read(0, 1)
# Erase the expanded menu, leaving blank retained rows below the next prompt.
os.write(1, b'\x1b[4B\x1b[2K\x1b[F\x1b[2K\x1b[F\x1b[2K\x1b[F\x1b[2K\x1b[F\x1b[2K\x1b[1GSelected Vanilla\r\n\x1b[?25lProject name > ')
os.read(0, 1)
os.write(1, b'\r\nContinue? [y/N] ')
os.read(0, 1)
os.write(1, b'\r\nDONE\r\n')
"#
    );
    let id = session(&relay.launch(&program), Waiting::Input);
    let name = relay.send(&id, "<Enter>");
    assert_eq!(session(&name, Waiting::Input), id);
    assert!(name.output.contains("Project name"), "{}", name.output);
    let confirm = relay.send(&id, "A");
    assert_eq!(session(&confirm, Waiting::Input), id);
    assert!(confirm.output.contains("Continue?"), "{}", confirm.output);
    finished(&relay.send(&id, "y"));
}

#[test]
fn a_question_after_progress_can_keep_the_cursor_hidden() {
    let Some(relay) = Relay::new() else { return };
    let program = format!(
        "{READ_KEYS}{}",
        r#"os.write(1, b'\x1b[?25lDownload 100%\r\nContinue? [y/N] ')
os.read(0, 1)
os.write(1, b'\r\nDONE\r\n')
"#
    );
    let launched = relay.launch(&program);
    let id = session(&launched, Waiting::Input);
    assert!(launched.output.contains("Continue?"), "{}", launched.output);
    finished(&relay.send(&id, "y"));
}

#[test]
fn paused_progress_frames_behind_a_relay_are_not_questions() {
    for frame in [
        r"\x1b[?25lSyncing databases...\r\nrepo [Co  o  o] 0%",
        r"\x1b[?25lrepo1 100%\r\nrepo2 100%\r\n\x1b[2Arepo1 100%",
    ] {
        let Some(relay) = Relay::new() else { return };
        // A FIFO releases the paused command after its running report, so
        // readiness and completion do not depend on a guessed sleep length.
        let program = format!(
            r#"import os
from pathlib import Path
release = Path(__file__).with_name('release')
os.mkfifo(release)
os.write(1, b'{frame}')
with release.open('rb') as ready:
    ready.read(1)
os.write(1, b'\r\nDONE\r\n')
"#
        );
        let launched = relay.launch(&program);
        let id = session(&launched, Waiting::No);
        assert!(launched.output.contains("repo"), "{}", launched.output);
        fs::write(relay.root.path().join("release"), b"go").expect("release paused work");
        finished(&relay.call("bashwait", json!({"session_id": id, "wait": 5})));
    }
}

#[test]
fn an_answer_waits_past_two_seconds_of_paused_progress() {
    let Some(relay) = Relay::new() else { return };
    let program = format!(
        "{READ_KEYS}{}",
        r#"from pathlib import Path
release = Path(__file__).with_name('release')
os.mkfifo(release)
os.write(1, b'Continue? [y/N] ')
os.read(0, 1)
os.write(1, b'\r\n\x1b[?25lDownloading [Co  o] 0%')
with release.open('rb') as ready:
    ready.read(1)
os.write(1, b'\rDownloading [####] 100%\r\nDONE\r\n')
"#
    );
    let id = session(&relay.launch(&program), Waiting::Input);
    let (ready, progress) = mpsc::sync_channel(1);
    let release_path = relay.root.path().join("release");
    let release = std::thread::spawn(move || {
        progress
            .recv_timeout(Duration::from_secs(10))
            .expect("the program printed its first progress frame");
        // The old input quiet cutoff was two seconds. Start the necessary
        // three-second pause only after the actual progress frame arrives.
        std::thread::sleep(Duration::from_secs(3));
        fs::write(release_path, b"go").expect("release paused work");
    });
    let mut ready = Some(ready);
    let answered = relay.executor.execute(
        &ToolCallRequest {
            id: "answer".into(),
            name: "bashsend".into(),
            arguments: json!({"session_id": id, "input": "y"}).to_string(),
        },
        &CancelToken::new(),
        &mut |event| {
            if let ToolProgress::Screen { settled, live } = event
                && (settled.contains("Downloading") || live.contains("Downloading"))
                && let Some(ready) = ready.take()
            {
                ready.send(()).expect("release observer still waiting");
            }
        },
    );
    release.join().expect("release observer finished");
    finished(&answered);
}
