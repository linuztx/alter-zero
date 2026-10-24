#![cfg(windows)]

use std::io::{Read, Write};
use std::process::Command;
use std::time::{Duration, Instant};

fn digest(path: &std::path::Path) -> String {
    use sha2::{Digest, Sha256};
    let mut file = std::fs::File::open(path).unwrap();
    let mut hash = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        let n = file.read(&mut buffer).unwrap();
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
    }
    format!("{:x}", hash.finalize())
}

#[test]
fn native_cli_reports_its_version_and_help() {
    let exe = env!("CARGO_BIN_EXE_alter-zero");
    let version = Command::new(exe).arg("--version").output().unwrap();
    assert!(version.status.success());
    assert_eq!(
        String::from_utf8_lossy(&version.stdout).trim(),
        format!("alter-zero {}", env!("CARGO_PKG_VERSION"))
    );
    assert!(
        Command::new(exe)
            .arg("--help")
            .output()
            .unwrap()
            .status
            .success()
    );
}

#[test]
fn captured_bash_preserves_syntax_unicode_and_exit_codes() {
    let mut child = alter_zero::subprocess::spawn_detached_shell(
        None,
        "[[ 1 == 1 ]] && printf 'hello 世界'; exit 7",
    )
    .unwrap();
    let mut text = String::new();
    child
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut text)
        .unwrap();
    assert_eq!(text, "hello 世界");
    assert_eq!(child.wait().unwrap().code(), Some(7));
}

#[test]
fn conpty_supports_input_output_and_exit_status() {
    let mut session = alter_zero::pty::spawn::spawn(
        None,
        "printf 'name? '; read -r name; printf 'hello %s\\n' \"$name\"; exit 7",
    )
    .unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let writer = std::sync::Arc::new(std::sync::Mutex::new(session.writer));
    let replies = std::sync::Arc::clone(&writer);
    std::thread::spawn(move || {
        let mut screen = alter_zero::pty::screen::Screen::default();
        let mut bytes = [0; 4096];
        while let Ok(n) = session.reader.read(&mut bytes) {
            if n == 0 {
                break;
            }
            let answer = screen.feed(&bytes[..n]);
            if !answer.is_empty() {
                replies.lock().unwrap().write_all(&answer).unwrap();
            }
            if tx.send(bytes[..n].to_vec()).is_err() {
                break;
            }
        }
    });
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut output = Vec::new();
    let mut answered = false;
    let status = loop {
        while let Ok(chunk) = rx.try_recv() {
            output.extend(chunk);
        }
        if !answered && String::from_utf8_lossy(&output).contains("name?") {
            writer.lock().unwrap().write_all(b"Ada Lovelace\r").unwrap();
            answered = true;
        }
        if let Some(status) = session.child.try_wait().unwrap() {
            break status;
        }
        assert!(
            Instant::now() < deadline,
            "ConPTY hung: {}",
            String::from_utf8_lossy(&output)
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    while let Ok(chunk) = rx.recv_timeout(Duration::from_millis(100)) {
        output.extend(chunk);
    }
    assert_eq!(status.code(), Some(7));
    assert!(String::from_utf8_lossy(&output).contains("hello Ada Lovelace"));
}

#[test]
fn windows_state_and_package_manager_paths_work() {
    assert!(alter_zero::platform::home_dir().unwrap().is_absolute());
    assert_eq!(
        alter_zero::update::package_manager(std::path::Path::new(
            r"C:\Users\Ada Lovelace\AppData\Roaming\npm\node_modules\@linuztx\alter-zero-win32-x64\bin\alter-zero.exe"
        )),
        Some(alter_zero::update::PackageManager::Npm)
    );
    let shell = alter_zero::subprocess::tool_shell();
    assert!(
        shell.is_absolute() && shell.is_file(),
        "install Git for Windows: {}",
        shell.display()
    );
    assert!(
        !shell
            .to_string_lossy()
            .to_ascii_lowercase()
            .contains("system32")
    );
}

#[test]
fn powershell_installer_checks_the_archive_before_replacing_an_install() {
    use std::io::{BufRead, BufReader};
    let root = tempfile::tempdir().unwrap();
    let version = env!("CARGO_PKG_VERSION");
    let target = if cfg!(target_arch = "aarch64") {
        "aarch64-pc-windows-msvc"
    } else {
        "x86_64-pc-windows-msvc"
    };
    let stem = format!("alter-zero-v{version}-{target}");
    let dist = root.path().join("dist");
    let stage = root.path().join(&stem);
    std::fs::create_dir_all(&stage).unwrap();
    std::fs::create_dir_all(&dist).unwrap();
    std::fs::copy(
        env!("CARGO_BIN_EXE_alter-zero"),
        stage.join("alter-zero.exe"),
    )
    .unwrap();
    for file in ["LICENSE", "README.md", "CHANGELOG.md"] {
        std::fs::write(stage.join(file), "fixture").unwrap();
    }
    let archive = dist.join(format!("{stem}.tar.gz"));
    assert!(
        Command::new("tar.exe")
            .arg("-czf")
            .arg(&archive)
            .arg("-C")
            .arg(root.path())
            .arg(&stem)
            .status()
            .unwrap()
            .success()
    );
    let archive_digest = digest(&archive);
    std::fs::write(
        dist.join("SHA256SUMS"),
        format!("{archive_digest}  {stem}.tar.gz\n"),
    )
    .unwrap();
    let mut server = Command::new("python")
        .arg("-u")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/scripts/release/release_server.py"
        ))
        .arg(&dist)
        .arg(format!("v{version}"))
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    // Ensure fixture servers are reaped even when an assertion fails.
    struct Server(std::process::Child);
    impl Drop for Server {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let mut port = String::new();
    BufReader::new(server.stdout.take().unwrap())
        .read_line(&mut port)
        .unwrap();
    let _server = Server(server);
    let install = root.path().join("install with spaces");
    let run = || {
        Command::new("powershell.exe")
            .args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
            ])
            .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/install.ps1"))
            .arg("-Version")
            .arg(version)
            .arg("-InstallDir")
            .arg(&install)
            .arg("-BaseUrl")
            .arg(format!("http://127.0.0.1:{}", port.trim()))
            .output()
            .unwrap()
    };
    let installed = run();
    assert!(
        installed.status.success(),
        "{}",
        String::from_utf8_lossy(&installed.stderr)
    );
    assert_eq!(
        digest(&install.join("alter-zero.exe")),
        digest(std::path::Path::new(env!("CARGO_BIN_EXE_alter-zero")))
    );
    // A checksum failure must preserve the existing install byte for byte.
    std::fs::write(&archive, "tampered archive").unwrap();
    let refused = run();
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("SHA-256 mismatch"));
    assert_eq!(
        digest(&install.join("alter-zero.exe")),
        digest(std::path::Path::new(env!("CARGO_BIN_EXE_alter-zero")))
    );
}

#[test]
fn registry_uses_conpty_and_can_cancel_it() {
    use alter_zero::background::{BackgroundRegistry, BgEvent};
    let dir = tempfile::tempdir().unwrap();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let registry = BackgroundRegistry::new(tx, dir.path().to_path_buf());
    let session = registry
        .launch_tty("printf 'ready\\n'; sleep 60", None, None, true)
        .unwrap();
    registry.kill(&session.task.id);
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Ok(BgEvent::Exited { killed, .. }) = rx.try_recv() {
            assert!(killed);
            break;
        }
        if Instant::now() >= deadline {
            registry.kill_all();
            panic!("ConPTY cancellation did not terminate the session");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn native_tui_paints_a_prompt_and_exits_cleanly_in_conpty() {
    let state = tempfile::tempdir().unwrap();
    let pty = portable_pty::native_pty_system()
        .openpty(portable_pty::PtySize {
            rows: 40,
            cols: 120,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    let mut command = portable_pty::CommandBuilder::new(env!("CARGO_BIN_EXE_alter-zero"));
    command.cwd(state.path());
    command.env("ALTER_ZERO_CONFIG_DIR", state.path());
    command.env("ALTER_ZERO_DUMMY", "1");
    command.env("ALTER_ZERO_PROJECT_CONFIG", "0");
    command.env("ALTER_ZERO_CHECKPOINTS", "0");
    command.env("ALTER_ZERO_TELEMETRY", "0");
    command.env("ALTER_ZERO_UPDATE_CHECK", "0");
    let mut child = pty.slave.spawn_command(command).unwrap();
    drop(pty.slave);
    let mut reader = pty.master.try_clone_reader().unwrap();
    let writer = std::sync::Arc::new(std::sync::Mutex::new(pty.master.take_writer().unwrap()));
    let reply_writer = std::sync::Arc::clone(&writer);
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut screen = alter_zero::pty::screen::Screen::default();
        let mut bytes = [0; 4096];
        while let Ok(n) = reader.read(&mut bytes) {
            if n == 0 {
                break;
            }
            let reply = screen.feed(&bytes[..n]);
            if !reply.is_empty() {
                let _ = reply_writer.lock().unwrap().write_all(&reply);
            }
            if tx.send(bytes[..n].to_vec()).is_err() {
                break;
            }
        }
    });
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut output = Vec::new();
    let mut quit = false;
    loop {
        while let Ok(chunk) = rx.try_recv() {
            output.extend(chunk);
        }
        if !quit && String::from_utf8_lossy(&output).contains('❯') {
            writer.lock().unwrap().write_all(b"\x03").unwrap();
            quit = true;
        }
        if let Some(status) = child.try_wait().unwrap() {
            assert!(
                quit && status.success(),
                "TUI did not start and exit cleanly: {}",
                String::from_utf8_lossy(&output)
            );
            break;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            panic!(
                "TUI did not paint a prompt and exit: {}",
                String::from_utf8_lossy(&output)
            );
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}
