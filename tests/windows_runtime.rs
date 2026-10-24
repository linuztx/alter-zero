//! Native Windows proofs: Git for Windows Bash inside ConPTY, using the real
//! executable's helper (and therefore its pre-spawn Job ownership).
//! Cross-compilation checks the boundary's shape; these tests must run on
//! Windows to establish terminal behavior and descendant cancellation.
#![cfg(windows)]

use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use alter_zero::background::{BackgroundRegistry, BgEvent};
use alter_zero::pty::keys::{encode, parse_input};
use alter_zero::pty::report::{Status, Waiting};
use alter_zero::pty::session::WaitEnd;
use alter_zero::pty::settle::{Settle, WaitKind};
use alter_zero::pty::spawn::spawn;

struct Runtime {
    registry: BackgroundRegistry,
    events: tokio::sync::mpsc::UnboundedReceiver<BgEvent>,
    dir: tempfile::TempDir,
}

impl Runtime {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let (tx, events) = tokio::sync::mpsc::unbounded_channel();
        let registry = BackgroundRegistry::new(tx, dir.path().join("tasks"))
            .with_detach_helper(Some(helper()));
        Self {
            registry,
            events,
            dir,
        }
    }

    fn exited(&mut self, id: &str) -> (Option<i32>, bool) {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if let Ok(BgEvent::Exited {
                id: got,
                code,
                killed,
                ..
            }) = self.events.try_recv()
                && got == id
            {
                return (code, killed);
            }
            assert!(Instant::now() < deadline, "session {id} never exited");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        self.registry.kill_all();
    }
}

fn bash_quote(path: &std::path::Path) -> String {
    format!(
        "'{}'",
        path.to_string_lossy()
            .replace('\\', "/")
            .replace('\'', "'\\''")
    )
}

fn heartbeat_command(path: &std::path::Path) -> String {
    // A Bash subshell and its sleep are descendants, not the tracked helper.
    // The finite loop is a backstop if an assertion fails before cleanup.
    format!(
        "(for ((n=0; n<300; n++)); do printf x >> {}; sleep 0.1; done) & wait",
        bash_quote(path)
    )
}

fn wait_for_heartbeat(path: &std::path::Path) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while std::fs::metadata(path).map_or(0, |m| m.len()) < 3 {
        assert!(
            Instant::now() < deadline,
            "descendant never started: {path:?}"
        );
        std::thread::sleep(Duration::from_millis(30));
    }
}

fn assert_heartbeat_stopped(path: &std::path::Path) {
    std::thread::sleep(Duration::from_millis(250));
    let size = std::fs::metadata(path).unwrap().len();
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(
        std::fs::metadata(path).unwrap().len(),
        size,
        "descendant survived the session"
    );
}

fn never() -> bool {
    false
}

#[test]
fn registry_prompt_keys_and_unknown_termios_use_the_shared_session_engine() {
    let runtime = Runtime::new();
    let launch = runtime.registry.launch_tty(
        "printf 'Choice λ? '; IFS= read -r answer; printf 'accepted <%s>\\n' \"$answer\"; exit 9",
        None,
        None,
        false,
    ).expect("ConPTY launch");
    let io = launch.io;
    let id = launch.task.id;
    assert_eq!(
        runtime.registry.line_mode(&id),
        None,
        "ConPTY cannot report termios"
    );
    assert_eq!(
        runtime.registry.reader(&id),
        alter_zero::pty::keys::Reader::Unknown
    );
    assert_eq!(
        io.wait(
            WaitKind::Launch,
            io.origin_mark(),
            Duration::from_secs(15),
            &never,
            &never,
            &mut |_, _| {}
        ),
        WaitEnd::Settled(Settle::Prompt)
    );
    let prompt = io.look(
        &id,
        Status::Running {
            waiting: Waiting::Input,
        },
    );
    assert!(prompt.contains("Choice λ?"), "{prompt}");
    assert_eq!(io.end_wait(), None);
    let since = io.begin_wait();
    runtime
        .registry
        .send_input(&id, encode(&parse_input("東京<Enter>"), io.modes()))
        .unwrap();
    assert_eq!(
        io.wait(
            WaitKind::Input,
            since,
            Duration::from_secs(15),
            &never,
            &never,
            &mut |_, _| {}
        ),
        WaitEnd::Settled(Settle::Exited)
    );
    assert_eq!(io.exit(), Some(Some(9)));
    let output = io.look(&id, Status::Exited(Some(9)));
    assert!(output.contains("accepted <東京>"), "{output}");
    runtime.registry.finalize(&id, io.end_wait().unwrap());
}

#[test]
fn ctrl_c_reaches_conpty_as_terminal_input_not_a_noop_signal() {
    for signal_request in [false, true] {
        let runtime = Runtime::new();
        let launch = runtime.registry.launch_tty(
            "trap 'printf \"INTERRUPTED\\n\"; exit 17' INT; printf 'Interrupt? '; IFS= read -r answer; exit 93",
            None,
            None,
            false,
        ).unwrap();
        let io = launch.io;
        let id = launch.task.id;
        assert_eq!(
            io.wait(
                WaitKind::Launch,
                io.origin_mark(),
                Duration::from_secs(15),
                &never,
                &never,
                &mut |_, _| {}
            ),
            WaitEnd::Settled(Settle::Prompt)
        );
        let _ = io.look(
            &id,
            Status::Running {
                waiting: Waiting::Input,
            },
        );
        assert_eq!(io.end_wait(), None);
        let since = io.begin_wait();
        if signal_request {
            runtime.registry.signal(&id, "INT");
        } else {
            runtime
                .registry
                .send_input(&id, encode(&parse_input("<C-c>"), io.modes()))
                .unwrap();
        }
        assert_eq!(
            io.wait(
                WaitKind::Input,
                since,
                Duration::from_secs(10),
                &never,
                &never,
                &mut |_, _| {}
            ),
            WaitEnd::Settled(Settle::Exited)
        );
        assert_eq!(io.exit(), Some(Some(17)));
        let output = io.look(&id, Status::Exited(Some(17)));
        assert!(output.contains("INTERRUPTED"), "{output}");
        runtime.registry.finalize(&id, io.end_wait().unwrap());
    }
}

#[test]
fn kill_stops_pipe_and_conpty_heartbeat_descendants() {
    for tty in [false, true] {
        let mut runtime = Runtime::new();
        let path = runtime.dir.path().join("heartbeat.txt");
        let command = heartbeat_command(&path);
        let task = if tty {
            runtime
                .registry
                .launch_tty(&command, None, None, true)
                .unwrap()
                .task
        } else {
            runtime.registry.launch(&command, None, true).unwrap()
        };
        wait_for_heartbeat(&path);
        let began = Instant::now();
        runtime.registry.kill(&task.id);
        assert!(runtime.exited(&task.id).1, "a requested kill is reported");
        assert!(began.elapsed() < Duration::from_secs(10));
        assert_heartbeat_stopped(&path);
    }
}

#[test]
fn adopting_a_std_child_retains_its_helper_owned_descendants() {
    use alter_zero::subprocess::{spawn_detached_in, tool_shell};
    let mut runtime = Runtime::new();
    let path = runtime.dir.path().join("adopted-heartbeat.txt");
    let command = heartbeat_command(&path);
    let mut child = spawn_detached_in(Some(&helper()), tool_shell(), &command).unwrap();
    let (tx, rx) = mpsc::channel();
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    for mut pipe in [Box::new(stdout) as Box<dyn Read + Send>, Box::new(stderr)] {
        let tx = tx.clone();
        std::thread::spawn(move || {
            let mut chunk = [0u8; 8192];
            while let Ok(n) = pipe.read(&mut chunk) {
                if n == 0 || tx.send(chunk[..n].to_vec()).is_err() {
                    break;
                }
            }
        });
    }
    drop(tx);
    let task = runtime
        .registry
        .adopt(&command, None, true, child, rx, Vec::new());
    wait_for_heartbeat(&path);
    runtime.registry.kill(&task.id);
    assert!(runtime.exited(&task.id).1);
    assert_heartbeat_stopped(&path);
}

#[test]
fn a_helpers_ordinary_exit_stops_heartbeat_descendants_it_left_behind() {
    for tty in [false, true] {
        let mut runtime = Runtime::new();
        let path = runtime.dir.path().join("orphan-heartbeat.txt");
        let quoted = bash_quote(&path);
        let command = format!(
            "(for ((n=0; n<300; n++)); do printf x >> {quoted}; sleep 0.1; done) & \
             while [[ ! -s {quoted} ]]; do sleep 0.05; done; exit 6"
        );
        let task = if tty {
            runtime
                .registry
                .launch_tty(&command, None, None, true)
                .unwrap()
                .task
        } else {
            runtime.registry.launch(&command, None, true).unwrap()
        };
        assert_eq!(runtime.exited(&task.id), (Some(6), false));
        assert_heartbeat_stopped(&path);
    }
}

#[test]
fn kill_all_stops_every_conpty_and_its_heartbeat_descendants() {
    let mut runtime = Runtime::new();
    let mut paths = Vec::new();
    let mut ids = std::collections::HashSet::new();
    for index in 0..2 {
        let path = runtime.dir.path().join(format!("heartbeat-{index}.txt"));
        let task = runtime
            .registry
            .launch_tty(&heartbeat_command(&path), None, None, true)
            .unwrap()
            .task;
        wait_for_heartbeat(&path);
        ids.insert(task.id);
        paths.push(path);
    }
    runtime.registry.kill_all();
    let deadline = Instant::now() + Duration::from_secs(15);
    while !ids.is_empty() {
        if let Ok(BgEvent::Exited { id, killed, .. }) = runtime.events.try_recv() {
            assert!(killed);
            ids.remove(&id);
        }
        assert!(Instant::now() < deadline, "kill_all left sessions: {ids:?}");
        std::thread::sleep(Duration::from_millis(20));
    }
    for path in paths {
        assert_heartbeat_stopped(&path);
    }
}

#[test]
fn conpty_output_is_drained_and_the_report_stays_bounded() {
    let mut runtime = Runtime::new();
    let launch = runtime.registry.launch_tty(
        "printf 'BEGIN-OUTPUT\\n'; for ((n=0; n<5000; n++)); do printf 'row-%05d abcdefghijklmnopqrstuvwxyz\\n' \"$n\"; done; printf 'END-OUTPUT 世界\\n'",
        None,
        None,
        true,
    ).unwrap();
    assert_eq!(runtime.exited(&launch.task.id), (Some(0), false));
    let output = launch.io.look(&launch.task.id, Status::Exited(Some(0)));
    let log = std::fs::read_to_string(&launch.task.output_path).unwrap();
    assert!(log.contains("BEGIN-OUTPUT"), "start was drained");
    assert!(log.contains("row-02500"), "middle was drained");
    assert!(log.contains("END-OUTPUT 世界"), "last bytes were drained");
    assert!(output.contains("END-OUTPUT 世界"), "{output}");
    assert!(
        output.len() < 80 * 1024,
        "report is bounded: {}",
        output.len()
    );
}

fn helper() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_alter-zero"))
}

/// Failure cleanup for tests using portable-pty directly. Unlike the registry,
/// dropping a portable child handle does not terminate its process or Job.
struct TestChild(Box<dyn portable_pty::Child + Send + Sync>);

impl Drop for TestChild {
    fn drop(&mut self) {
        if matches!(self.0.try_wait(), Ok(Some(_))) {
            return;
        }
        if let Some(pid) = self.0.process_id() {
            let _ = alter_zero::subprocess::kill_windows_tree(pid);
        }
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn read_until(rx: &mpsc::Receiver<Vec<u8>>, needle: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut bytes = Vec::new();
    loop {
        match rx.recv_timeout(Duration::from_millis(20)) {
            Ok(chunk) => bytes.extend_from_slice(&chunk),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        if String::from_utf8_lossy(&bytes).contains(needle) || Instant::now() >= deadline {
            break;
        }
    }
    String::from_utf8(bytes).expect("ConPTY speaks UTF-8")
}

#[test]
fn the_native_tui_starts_and_quits_cleanly_inside_conpty() {
    use portable_pty::{CommandBuilder, PtySize};
    let dir = tempfile::tempdir().unwrap();
    let work = dir.path().join("work");
    std::fs::create_dir(&work).unwrap();
    let mut command = CommandBuilder::new(helper());
    command.cwd(&work);
    command.env("ALTER_ZERO_DUMMY", "1");
    for name in [
        "ALTER_ZERO_TELEMETRY",
        "ALTER_ZERO_UPDATE_CHECK",
        "ALTER_ZERO_TOOLS",
        "ALTER_ZERO_HOOKS",
        "ALTER_ZERO_MCP",
        "ALTER_ZERO_CHECKPOINTS",
        "ALTER_ZERO_PROJECT_CONFIG",
        "ALTER_ZERO_HERDR",
        "ALTER_ZERO_TIPS",
        "ALTER_ZERO_PROJECT_DOC_MAX_BYTES",
    ] {
        command.env(name, "0");
    }
    for (name, leaf) in [
        ("ALTER_ZERO_CONFIG_DIR", "config"),
        ("ALTER_ZERO_SESSIONS_DIR", "sessions"),
        ("ALTER_ZERO_SKILLS_DIR", "skills"),
        ("ALTER_ZERO_AGENTS_DIR", "agents"),
    ] {
        let path = dir.path().join(leaf);
        std::fs::create_dir(&path).unwrap();
        command.env(name, &path);
    }
    command.env("ALTER_ZERO_HISTORY_FILE", dir.path().join("history.jsonl"));
    command.env("ALTER_ZERO_STARTUP_DELAY_MS", "0");
    let pair = portable_pty::native_pty_system()
        .openpty(PtySize {
            rows: 40,
            cols: 120,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("a native outer console for the real TUI");
    let mut reader = pair.master.try_clone_reader().unwrap();
    let writer = std::sync::Arc::new(std::sync::Mutex::new(pair.master.take_writer().unwrap()));
    let mut child = TestChild(
        pair.slave
            .spawn_command(command)
            .expect("the native TUI starts"),
    );
    drop(pair.slave);
    let (tx, rx) = mpsc::channel();
    let replies = std::sync::Arc::clone(&writer);
    let pump = std::thread::spawn(move || {
        let mut screen = alter_zero::pty::screen::Screen::default();
        let mut bytes = [0u8; 8192];
        while let Ok(n) = reader.read(&mut bytes) {
            if n == 0 {
                break;
            }
            let reply = screen.feed(&bytes[..n]);
            if !reply.is_empty() {
                let _ = replies.lock().unwrap().write_all(&reply);
            }
            // ConPTY may put styling or cursor movements between painted
            // characters. Detect the banner as visible text, not raw bytes.
            if tx
                .send(screen.snapshot().rows.join("\n").into_bytes())
                .is_err()
            {
                break;
            }
        }
    });
    let banner = read_until(&rx, "/resume");
    let ready = banner.contains("Alter Zero") && banner.contains("/resume");
    if ready {
        // Native crossterm creates its sole reader after the synchronous
        // cursor query. Console input buffers these keys until it is ready.
        writer.lock().unwrap().write_all(b"/quit\r").unwrap();
    }
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut exit = None;
    while Instant::now() < deadline {
        if let Some(status) = child.0.try_wait().unwrap() {
            exit = Some(status);
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    drop(child);
    drop(writer);
    drop(pair.master);
    let _ = pump.join();
    assert!(ready, "native banner was never drawn: {banner:?}");
    let status = exit.expect("/quit must exit without a cursor-query or stdin-reader deadlock");
    assert_eq!(status.exit_code(), 0, "native TUI teardown: {banner:?}");
}

#[test]
fn a_pipe_helper_has_no_terminal_to_read_or_write() {
    use alter_zero::subprocess::{spawn_detached_in, tool_shell};
    let mut child = spawn_detached_in(
        Some(&helper()),
        tool_shell(),
        "if [[ -t 0 || -t 1 || -t 2 ]]; then printf 'ATTACHED-STDIO\\n'; exit 91; fi; \
         if { : < /dev/tty; } 2>/dev/null; then \
         printf 'OPENED-TERMINAL\\n'; exit 92; fi; printf 'NO-TERMINAL\\n'",
    )
    .expect("the production pipe helper starts");
    let (tx, rx) = mpsc::channel();
    let mut stdout = child.stdout.take().unwrap();
    std::thread::spawn(move || {
        let mut output = String::new();
        let _ = stdout.read_to_string(&mut output);
        let _ = tx.send(output);
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            alter_zero::subprocess::kill_process_group(&mut child);
            let _ = child.wait();
            panic!("a pipe helper blocked on the user's terminal");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let output = rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(status.code(), Some(0), "{output}");
    assert!(output.contains("NO-TERMINAL"), "{output}");
    assert!(!output.contains("ATTACHED-STDIO"), "{output}");
    assert!(!output.contains("OPENED-TERMINAL"), "{output}");
}

#[test]
fn conpty_has_separate_streams_a_unicode_prompt_and_the_shells_exit_code() {
    let process = spawn(
        Some(&helper()),
        "[[ -t 0 && -t 1 && -t 2 ]] || exit 91; \
         printf 'Name λ: '; IFS= read -r name; printf 'hello <%s> 世界\\n' \"$name\"; exit 7",
    )
    .expect("Git for Windows Bash starts in a real ConPTY");
    let mut child = TestChild(process.child);
    let (tx, rx) = mpsc::channel();
    let mut reader = process.reader;
    let writer = std::sync::Arc::new(std::sync::Mutex::new(process.writer));
    let replies = std::sync::Arc::clone(&writer);
    std::thread::spawn(move || {
        let mut screen = alter_zero::pty::screen::Screen::default();
        let mut bytes = [0u8; 8192];
        while let Ok(n) = reader.read(&mut bytes) {
            if n == 0 {
                break;
            }
            // ConPTY's inherit-cursor handshake asks a cursor query too;
            // answer it through the same emulator as production sessions.
            let reply = screen.feed(&bytes[..n]);
            if !reply.is_empty() {
                let _ = replies.lock().unwrap().write_all(&reply);
            }
            if tx.send(bytes[..n].to_vec()).is_err() {
                break;
            }
        }
    });
    let prompt = read_until(&rx, "Name λ: ");
    assert!(prompt.contains("Name λ: "), "{prompt:?}");
    writer
        .lock()
        .unwrap()
        .write_all("Zoë 東京\r".as_bytes())
        .unwrap();
    writer.lock().unwrap().flush().unwrap();
    let output = read_until(&rx, "hello <Zoë 東京> 世界");
    assert!(output.contains("hello <Zoë 東京> 世界"), "{output:?}");
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = child.0.try_wait().expect("polls the helper") {
            break status;
        }
        assert!(
            Instant::now() < deadline,
            "Bash and its helper exit promptly"
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(status.exit_code(), 7);
    // The ConPTY control handle must outlive both the child and output drain.
    drop(writer);
    drop(process.master);
}
