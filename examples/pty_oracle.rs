//! Differential check of the session screen (`docs/interactive-shell.md`):
//! run a program in a pseudo-terminal of the session's size, type a script
//! of keys into it, and **record every byte** it wrote — then print the
//! session emulator's final screen, so `scripts/pty_oracle.sh` can replay the
//! same bytes into tmux and diff the two. What differs is an escape sequence
//! one emulator reads and the other does not. NOT run by `cargo test`.
//!
//! ```bash
//! cargo run --example pty_oracle -- 'btop' steps.jsonl raw.bin > ours.txt
//! ```
//!
//! A step is `{"keys": "<Down>q"}` (the `bashsend` notation, written
//! the way the session writes it) or `{"sleep": ms}`, one per line; with no
//! steps file the program runs for two seconds. The program is left to exit
//! on its own for a second after the last step, then killed.
//! `ORACLE_TRACE=1` logs every write, every read and every query reply to
//! stderr.

use std::io::{BufRead, Read, Write};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use alter_zero::pty::keys::{ESC_PAUSE, KEY_PAUSE, Pace, encode, parse_input};
use alter_zero::pty::screen::Screen;
use alter_zero::pty::spawn::spawn;
use serde_json::Value;

fn main() {
    let mut args = std::env::args().skip(1);
    let command = args
        .next()
        .expect("usage: pty_oracle <command> [steps] [raw]");
    let steps = args.next().filter(|path| path != "-");
    let raw_path = args.next().unwrap_or_else(|| "pty_oracle.raw".to_string());

    let mut process = spawn(None, &command).expect("the program starts");
    let screen = Arc::new(Mutex::new(Screen::default()));
    let raw = Arc::new(Mutex::new(Vec::<u8>::new()));
    #[cfg(unix)]
    let mut reader = process.master.try_clone().expect("a reader");
    #[cfg(unix)]
    let mut writer = process.master.try_clone().expect("a writer");
    #[cfg(windows)]
    let mut reader = process.reader;
    #[cfg(windows)]
    let writer = Arc::new(Mutex::new(process.writer));
    let pump = {
        let screen = Arc::clone(&screen);
        let raw = Arc::clone(&raw);
        #[cfg(unix)]
        let mut replies = process.master.try_clone().expect("a reply writer");
        #[cfg(windows)]
        let replies = Arc::clone(&writer);
        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        raw.lock().unwrap().extend_from_slice(&buf[..n]);
                        let reply = screen.lock().unwrap().feed(&buf[..n]);
                        if std::env::var_os("ORACLE_TRACE").is_some() {
                            eprintln!(
                                "OUT {:?}",
                                String::from_utf8_lossy(&buf[..n])
                                    .chars()
                                    .take(300)
                                    .collect::<String>()
                            );
                            if !reply.is_empty() {
                                eprintln!("REPLY {:?}", String::from_utf8_lossy(&reply));
                            }
                        }
                        if !reply.is_empty() {
                            #[cfg(unix)]
                            let _ = replies.write_all(&reply);
                            #[cfg(windows)]
                            let _ = replies.lock().unwrap().write_all(&reply);
                        }
                    }
                }
            }
        })
    };

    let script: Vec<Value> = match steps {
        Some(path) => std::io::BufReader::new(std::fs::File::open(path).expect("steps"))
            .lines()
            .map(|line| line.expect("a line"))
            .filter(|line| !line.trim().is_empty() && !line.trim_start().starts_with('#'))
            .map(|line| serde_json::from_str(&line).expect("a step"))
            .collect(),
        None => vec![serde_json::json!({"sleep": 2000})],
    };
    for step in script {
        if let Some(ms) = step.get("sleep").and_then(Value::as_u64) {
            std::thread::sleep(Duration::from_millis(ms));
        } else if let Some(keys) = step.get("keys").and_then(Value::as_str) {
            let modes = screen.lock().unwrap().modes();
            for chunk in encode(&parse_input(keys), modes) {
                #[cfg(unix)]
                writer.write_all(&chunk.bytes).expect("typed");
                #[cfg(windows)]
                writer
                    .lock()
                    .unwrap()
                    .write_all(&chunk.bytes)
                    .expect("typed");
                if std::env::var_os("ORACLE_TRACE").is_some() {
                    eprintln!("IN {:?}", String::from_utf8_lossy(&chunk.bytes));
                }
                // The session's writer waits for each read; a fixed gap is
                // enough to record a program's answer to its keys.
                match chunk.then {
                    Pace::Last => {}
                    Pace::Read => std::thread::sleep(KEY_PAUSE),
                    Pace::Esc => std::thread::sleep(KEY_PAUSE + ESC_PAUSE),
                }
            }
        }
    }
    let deadline = Instant::now() + Duration::from_secs(1);
    while Instant::now() < deadline {
        if process.child.try_wait().ok().flatten().is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    // The whole session, not just its leader: a program under `sh -c`.
    #[cfg(unix)]
    let _ = std::process::Command::new("kill")
        .args(["-KILL", "--", &format!("-{}", process.child.id())])
        .stderr(std::process::Stdio::null())
        .status();
    #[cfg(windows)]
    {
        // The oracle is a library-only caller: no real helper, so taskkill
        // is a fallback, not a guarantee of race-free Job ownership.
        if let Some(pid) = process.child.process_id() {
            let _ = alter_zero::subprocess::kill_windows_tree(pid);
        }
        let _ = process.child.kill();
    }
    let _ = process.child.wait();
    drop(writer);
    #[cfg(windows)]
    {
        std::thread::sleep(Duration::from_millis(150));
        // Closing the console releases the reader. Do not join a read that
        // needs the still-live pseudoconsole to close first.
        drop(process.master);
    }
    let _ = pump.join();

    std::fs::write(&raw_path, &*raw.lock().unwrap()).expect("the recording");
    let snapshot = screen.lock().unwrap().snapshot();
    for row in &snapshot.rows {
        println!("{row}");
    }
    eprintln!(
        "cursor {:?}, alternate {}, {} bytes recorded to {raw_path}",
        snapshot.cursor,
        snapshot.alternate,
        raw.lock().unwrap().len()
    );
}
