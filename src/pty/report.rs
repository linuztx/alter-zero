//! What an interactive-session call tells the model
//! (`docs/interactive-shell.md`): one **frame line** saying what state the
//! session is in, over the output since the model's previous look — or, for a
//! full-screen program, its screen.
//!
//! ```text
//! Running (session b7x2k9m1q, waiting for input)
//! >>> import math; print(math.pi)
//! 3.141592653589793
//! >>>
//! ```
//!
//! At a password prompt — the terminal reading a line with echo off — the
//! frame says so instead (`waiting for a password — typed input is hidden`),
//! telling the model what is asked and why its answer will not show.
//!
//! An exited session reports exactly what a plain `bash` call does
//! (`Exit code: N` over the output — [`crate::llm::tools::format_exec_output`]),
//! so the two read alike and the cell renders both the same way; a session the
//! model stopped says so rather than wearing a signal's red
//! (`Stopped (session …)`). The frames are also what the cell's display
//! reframe parses ([`parse_frame`]), so the wording lives in one place.

use super::screen::{MAX_HIGHLIGHTS, Snapshot};

/// The most a session report carries. Past it the **tail** is kept — the
/// newest lines and the prompt are what an interactive step is about — and
/// the report says how many lines it left out.
pub const SESSION_OUTPUT_MAX_BYTES: usize = 32 * 1024;

/// Where the session stands at the end of the call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Still running; `waiting` says what for, when it sits at a prompt.
    Running { waiting: Waiting },
    /// Exited by itself — `None` when a signal ended it.
    Exited(Option<i32>),
    /// Ended by the call's own `kill`.
    Stopped,
}

/// What a running session waits for at the end of a call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Waiting {
    /// Nothing: it is at work, or quiet between lines.
    No,
    /// Input, at a prompt — the terminal awaiting keys, quiet.
    Input,
    /// A password: the terminal reads a line with echo off, so what is typed
    /// there does not show (`pty::spawn::LineMode::hides_input`).
    Password,
}

impl Waiting {
    /// What a session waits for when it sits at a prompt (`at_prompt`) —
    /// a password when its terminal hides what it reads (`password`).
    #[must_use]
    pub fn of(at_prompt: bool, password: bool) -> Self {
        match (at_prompt, password) {
            (false, _) => Self::No,
            (true, false) => Self::Input,
            (true, true) => Self::Password,
        }
    }
}

/// What the session showed since the previous look.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum View {
    /// The transcript: new lines, how many unread ones were dropped, and
    /// the line the terminal's cursor is on (`at` — empty for a pipe, or at
    /// the start of a fresh line) — what a look that found nothing new
    /// re-reads to the model, so a prompt it is still waiting at is named.
    Lines {
        text: String,
        omitted: usize,
        at: String,
    },
    /// A full-screen program's current screen, under the lines printed
    /// before it took over (`before` — empty when there were none).
    Screen { before: String, snapshot: Snapshot },
}

/// A parsed frame line — what the cell's display reframe needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Frame<'a> {
    Running { session: &'a str, waiting: Waiting },
    Stopped { session: &'a str },
}

/// The frame line's opening words for a session still running, and for one
/// the call stopped — [`parse_frame`] reads the same constants back.
const RUNNING_PREFIX: &str = "Running (session ";
const STOPPED_PREFIX: &str = "Stopped (session ";
/// The clause a settled prompt adds to the running frame.
const WAITING_CLAUSE: &str = ", waiting for input";
/// The clause a password prompt adds instead: what is asked for, and that
/// the answer will not show when typed.
const PASSWORD_CLAUSE: &str = ", waiting for a password — typed input is hidden";

/// Appended to a report when the call typed text a line-reading prompt has
/// not received — no Enter after it (`pty::keys::leaves_line_open`).
pub const UNSUBMITTED_NOTE: &str = "[Typed but not submitted: this prompt reads whole lines. \
     Send <Enter> to submit it — and end each answer with <Enter> so it goes in one call.]";

/// Appended when a call's `kill` came with input the program answered by
/// asking for more: the program is left running, since it is waiting on the
/// model rather than done.
pub const NOT_KILLED_NOTE: &str = "[Not killed: it is waiting for more input. Answer it, or \
     send kill without input to end it anyway.]";

/// Appended when the typed text held codes a terminal writes to a program —
/// colour, mode switches, erases — which no keyboard sends and which were
/// left out (`pty::keys::strip_output_codes`): what they were, and how keys
/// are named instead.
#[must_use]
pub fn dropped_codes_note(codes: &[String]) -> String {
    format!(
        "[Not typed: {} — codes a terminal sends to a program, not keys. Name keys in \
         angle brackets: <Enter>, <Esc>, <C-o>.]",
        codes.join(" ")
    )
}

/// Appended when the input was read as **HTML-escaped** — `&lt;Esc&gt;` for
/// `<Esc>` (`pty::keys::html_escaped`): what it was read as, and how to
/// write it, since text sent escaped in an earlier call went in as written.
pub const HTML_ESCAPED_NOTE: &str = "[Read as HTML-escaped: &lt; as <, &gt; as >, &amp; as &. \
     Write them as themselves — escaped text in other calls was typed as written.]";

/// Appended when the user pressed Ctrl+B on a call waiting on its session:
/// the wait ended early, the command did not.
pub const WAIT_ENDED_NOTE: &str = "[The user ended this wait early; the command keeps \
     running. Carry on, and check on it later.]";

/// The model-facing result of a session call (see the module docs).
#[must_use]
pub fn report(session: &str, status: Status, view: &View) -> String {
    let body = body(view);
    match status {
        Status::Exited(code) => crate::llm::tools::format_exec_output(code, &body),
        Status::Running { waiting } => {
            let clause = match waiting {
                Waiting::No => "",
                Waiting::Input => WAITING_CLAUSE,
                Waiting::Password => PASSWORD_CLAUSE,
            };
            format!(
                "{RUNNING_PREFIX}{session}{clause})\n{}",
                or_nothing_new(body, view)
            )
        }
        Status::Stopped => format!("{STOPPED_PREFIX}{session})\n{}", or_nothing_new(body, view)),
    }
}

/// A running or stopped session's body, never blank: a look that found
/// nothing says so rather than ending on its frame line — naming the line
/// the terminal is still at, when it is on one (a prompt the model may have
/// lost track of across its polls).
fn or_nothing_new(body: String, view: &View) -> String {
    if !body.trim().is_empty() {
        return body;
    }
    match view {
        View::Lines { at, .. } if !at.trim().is_empty() => {
            format!("(no new output — still at: {})", at.trim())
        }
        _ => "(no new output)".to_string(),
    }
}

/// The text under the frame: the new lines (tail-capped, dropped lines
/// counted on a marker line at the top) or the screen.
fn body(view: &View) -> String {
    match view {
        View::Lines { text, omitted, .. } => {
            let (kept, dropped) = keep_tail(text, SESSION_OUTPUT_MAX_BYTES);
            let omitted = omitted + dropped;
            if omitted == 0 {
                kept.to_string()
            } else {
                format!("[… {omitted} earlier lines not shown]\n{kept}")
            }
        }
        View::Screen { before, snapshot } => {
            let (rows, columns) = snapshot.size;
            let (line, column) = snapshot.cursor;
            // Where typing lands, quoted — or that nothing shows it.
            let cursor = match &snapshot.cursor_text {
                _ if snapshot.cursor_hidden => "cursor hidden".to_string(),
                Some(text) => format!("cursor at line {line}, column {column} \u{2014} \"{text}\""),
                None => format!("cursor at line {line}, column {column}"),
            };
            let head = format!("Screen ({rows}x{columns}, {cursor}):");
            let mut screen = if snapshot.rows.is_empty() {
                format!("{head} (blank)")
            } else {
                format!("{head}\n{}", snapshot.rows.join("\n"))
            };
            if !snapshot.highlights.is_empty() {
                screen.push('\n');
                screen.push_str(&highlights_note(&snapshot.highlights));
            }
            let (before, _) = keep_tail(before, SESSION_OUTPUT_MAX_BYTES / 2);
            if before.trim().is_empty() {
                screen
            } else {
                format!("{before}\n{screen}")
            }
        }
    }
}

/// What a screen highlights, under it: `[Highlighted: line 2 "banana"]` —
/// which item a curses menu has selected, which button has the focus, what
/// the rows' text cannot say (`pty::screen`). Past [`MAX_HIGHLIGHTS`], an
/// ellipsis.
fn highlights_note(highlights: &[(u16, String)]) -> String {
    let mut named: Vec<String> = highlights
        .iter()
        .take(MAX_HIGHLIGHTS)
        .map(|(line, text)| format!("line {line} \"{text}\""))
        .collect();
    if highlights.len() > MAX_HIGHLIGHTS {
        named.push("…".to_string());
    }
    format!("[Highlighted: {}]", named.join("; "))
}

/// The last whole lines of `text` that fit in `max_bytes`, and how many lines
/// were left out in front of them. A single last line longer than the cap
/// keeps its own tail from a character boundary.
fn keep_tail(text: &str, max_bytes: usize) -> (&str, usize) {
    if text.len() <= max_bytes {
        return (text, 0);
    }
    let cut = text.len() - max_bytes;
    let start = match text[cut..].find('\n') {
        Some(newline) => cut + newline + 1,
        None => (cut..text.len())
            .find(|&at| text.is_char_boundary(at))
            .unwrap_or(text.len()),
    };
    let dropped = text[..start].matches('\n').count();
    (&text[start..], dropped)
}

/// Parse a report's first line back into its [`Frame`] — `None` for anything
/// else, an `Exit code: N` frame included (the plain `bash` reframe handles
/// that one).
#[must_use]
pub fn parse_frame(line: &str) -> Option<Frame<'_>> {
    if let Some(rest) = line.strip_prefix(RUNNING_PREFIX) {
        let inner = rest.strip_suffix(')')?;
        let (session, waiting) = if let Some(session) = inner.strip_suffix(WAITING_CLAUSE) {
            (session, Waiting::Input)
        } else if let Some(session) = inner.strip_suffix(PASSWORD_CLAUSE) {
            (session, Waiting::Password)
        } else {
            (inner, Waiting::No)
        };
        return is_session_id(session).then_some(Frame::Running { session, waiting });
    }
    let session = line.strip_prefix(STOPPED_PREFIX)?.strip_suffix(')')?;
    is_session_id(session).then_some(Frame::Stopped { session })
}

/// A session id as the registry mints them: a short run of ASCII letters
/// and digits — which is what keeps a sentence that happens to open with
/// `Running (` from parsing as a frame.
fn is_session_id(id: &str) -> bool {
    !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str) -> View {
        View::Lines {
            text: text.to_string(),
            omitted: 0,
            at: String::new(),
        }
    }

    #[test]
    fn a_session_waiting_at_a_prompt_says_so_over_its_output() {
        assert_eq!(
            report(
                "b7x2k9m1q",
                Status::Running {
                    waiting: Waiting::Input
                },
                &lines(">>>")
            ),
            "Running (session b7x2k9m1q, waiting for input)\n>>>"
        );
    }

    #[test]
    fn a_busy_session_is_just_running() {
        assert_eq!(
            report(
                "b7x2k9m1q",
                Status::Running {
                    waiting: Waiting::No
                },
                &lines("Compiling…")
            ),
            "Running (session b7x2k9m1q)\nCompiling…"
        );
    }

    #[test]
    fn nothing_new_is_said_out_loud() {
        assert_eq!(
            report(
                "s1",
                Status::Running {
                    waiting: Waiting::No
                },
                &lines("")
            ),
            "Running (session s1)\n(no new output)"
        );
    }

    #[test]
    fn nothing_new_names_the_line_the_session_is_still_at() {
        // A model polling a program that waits on it should not have to
        // remember what it was asked: the prompt is re-read to it.
        let view = View::Lines {
            text: String::new(),
            omitted: 0,
            at: "Full name: ".to_string(),
        };
        assert_eq!(
            report(
                "s1",
                Status::Running {
                    waiting: Waiting::Input
                },
                &view
            ),
            "Running (session s1, waiting for input)\n(no new output — still at: Full name:)"
        );
        // New output needs no reminder.
        let view = View::Lines {
            text: "Age: ".to_string(),
            omitted: 0,
            at: "Age: ".to_string(),
        };
        assert_eq!(
            report(
                "s1",
                Status::Running {
                    waiting: Waiting::Input
                },
                &view
            ),
            "Running (session s1, waiting for input)\nAge: "
        );
    }

    #[test]
    fn an_exited_session_reports_like_a_plain_bash_call() {
        assert_eq!(
            report("s1", Status::Exited(Some(0)), &lines("bye")),
            "Exit code: 0\nbye"
        );
        assert_eq!(
            report("s1", Status::Exited(Some(3)), &lines("")),
            "Exit code: 3\n(no output)"
        );
        assert_eq!(
            report("s1", Status::Exited(None), &lines("")),
            "Exit code: killed by signal\n(no output)"
        );
    }

    #[test]
    fn a_stopped_session_says_it_was_stopped() {
        assert_eq!(
            report("s1", Status::Stopped, &lines("^C")),
            "Stopped (session s1)\n^C"
        );
        assert_eq!(
            report("s1", Status::Stopped, &lines("")),
            "Stopped (session s1)\n(no new output)"
        );
    }

    #[test]
    fn dropped_lines_are_counted_at_the_top() {
        let view = View::Lines {
            text: "tail".to_string(),
            omitted: 1500,
            at: String::new(),
        };
        assert_eq!(
            report(
                "s1",
                Status::Running {
                    waiting: Waiting::No
                },
                &view
            ),
            "Running (session s1)\n[… 1500 earlier lines not shown]\ntail"
        );
    }

    #[test]
    fn an_oversized_update_keeps_its_tail_and_counts_the_rest() {
        let text: String = (0..20_000).map(|i| format!("line {i}\n")).collect();
        let out = report(
            "s1",
            Status::Running {
                waiting: Waiting::Input,
            },
            &lines(text.trim_end()),
        );
        assert!(out.len() <= SESSION_OUTPUT_MAX_BYTES + 200, "{}", out.len());
        assert!(out.ends_with("line 19999"), "the newest line survives");
        let marker = out.lines().nth(1).expect("a marker line");
        assert!(
            marker.starts_with("[… ") && marker.ends_with(" earlier lines not shown]"),
            "{marker}"
        );
        let dropped: usize = marker
            .trim_start_matches("[… ")
            .split(' ')
            .next()
            .unwrap()
            .parse()
            .unwrap();
        let kept = out.lines().count() - 2;
        assert_eq!(dropped + kept, 20_000, "every line is kept or counted");
    }

    #[test]
    fn a_full_screen_program_shows_its_screen() {
        let view = View::Screen {
            before: String::new(),
            snapshot: Snapshot {
                rows: vec!["~".to_string(), String::new(), "-- INSERT --".to_string()],
                cursor: (1, 5),
                size: (40, 120),
                alternate: true,
                highlights: Vec::new(),
                cursor_text: None,
                cursor_hidden: false,
            },
        };
        assert_eq!(
            report(
                "s1",
                Status::Running {
                    waiting: Waiting::Input
                },
                &view
            ),
            "Running (session s1, waiting for input)\n\
             Screen (40x120, cursor at line 1, column 5):\n~\n\n-- INSERT --"
        );
    }

    #[test]
    fn the_heading_quotes_the_cursors_line_or_says_it_is_hidden() {
        let screen = |cursor_text: Option<&str>, cursor_hidden| View::Screen {
            before: String::new(),
            snapshot: Snapshot {
                rows: vec!["File Name to Write: notes.txt".to_string()],
                cursor: (1, 30),
                size: (40, 120),
                alternate: true,
                highlights: Vec::new(),
                cursor_text: cursor_text.map(str::to_string),
                cursor_hidden,
            },
        };
        let running = Status::Running {
            waiting: Waiting::Input,
        };
        assert!(
            report(
                "s1",
                running,
                &screen(Some("File Name to Write: notes.txt\u{2038}"), false)
            )
            .contains(
                "Screen (40x120, cursor at line 1, column 30 \u{2014} \
                 \"File Name to Write: notes.txt\u{2038}\"):"
            )
        );
        assert!(
            report("s1", running, &screen(None, true)).contains("Screen (40x120, cursor hidden):")
        );
    }

    fn menu(highlights: Vec<(u16, String)>) -> View {
        View::Screen {
            before: String::new(),
            snapshot: Snapshot {
                rows: vec!["  apple".into(), "  banana".into(), "  cherry".into()],
                cursor: (2, 3),
                size: (40, 120),
                alternate: true,
                highlights,
                cursor_text: None,
                cursor_hidden: false,
            },
        }
    }

    #[test]
    fn a_screen_names_what_it_highlights_under_it() {
        // Which item a curses menu has selected is colour alone: the rows'
        // text cannot say it, so the report does.
        let out = report(
            "s1",
            Status::Running {
                waiting: Waiting::Input,
            },
            &menu(vec![(2, "banana".into())]),
        );
        assert_eq!(
            out,
            "Running (session s1, waiting for input)\n\
             Screen (40x120, cursor at line 2, column 3):\n  apple\n  banana\n  cherry\n\
             [Highlighted: line 2 \"banana\"]"
        );
        let out = report(
            "s1",
            Status::Running {
                waiting: Waiting::Input,
            },
            &menu(vec![(1, "PID USER".into()), (3, "68 root".into())]),
        );
        assert!(
            out.ends_with("[Highlighted: line 1 \"PID USER\"; line 3 \"68 root\"]"),
            "{out}"
        );
        let plain = report(
            "s1",
            Status::Running {
                waiting: Waiting::Input,
            },
            &menu(Vec::new()),
        );
        assert!(!plain.contains("Highlighted"), "{plain}");
    }

    #[test]
    fn highlights_past_the_most_are_left_at_an_ellipsis() {
        let many = (1..=u16::try_from(MAX_HIGHLIGHTS).unwrap() + 1)
            .map(|line| (line, format!("row {line}")))
            .collect();
        let out = report(
            "s1",
            Status::Running {
                waiting: Waiting::Input,
            },
            &menu(many),
        );
        let note = out.lines().last().unwrap();
        assert_eq!(note.matches("line ").count(), MAX_HIGHLIGHTS, "{note}");
        assert!(note.ends_with("; …]"), "{note}");
    }

    #[test]
    fn an_empty_screen_says_so() {
        let view = View::Screen {
            before: String::new(),
            snapshot: Snapshot {
                rows: Vec::new(),
                cursor: (1, 1),
                size: (40, 120),
                alternate: true,
                highlights: Vec::new(),
                cursor_text: None,
                cursor_hidden: false,
            },
        };
        assert_eq!(
            report(
                "s1",
                Status::Running {
                    waiting: Waiting::Input
                },
                &view
            ),
            "Running (session s1, waiting for input)\n\
             Screen (40x120, cursor at line 1, column 1): (blank)"
        );
    }

    #[test]
    fn lines_printed_before_a_full_screen_program_took_over_come_first() {
        // `git commit` prints its hints, then opens the editor: the model sees
        // both, in the order they happened.
        let view = View::Screen {
            before: "hint: Waiting for your editor to close the file...".to_string(),
            snapshot: Snapshot {
                rows: vec!["~".to_string()],
                cursor: (1, 1),
                size: (40, 120),
                alternate: true,
                highlights: Vec::new(),
                cursor_text: None,
                cursor_hidden: false,
            },
        };
        assert_eq!(
            report(
                "s1",
                Status::Running {
                    waiting: Waiting::Input
                },
                &view
            ),
            "Running (session s1, waiting for input)\n\
             hint: Waiting for your editor to close the file...\n\
             Screen (40x120, cursor at line 1, column 1):\n~"
        );
    }

    #[test]
    fn a_password_prompt_is_named_as_one() {
        // What the model is asked for, and that its answer will not show
        // when typed — so it asks the user, never guesses, and does not take
        // a silent terminal for a lost keystroke (docs/interactive-shell.md).
        let out = report(
            "s1",
            Status::Running {
                waiting: Waiting::Password,
            },
            &lines("Sorry, try again.\n[sudo] password for u:"),
        );
        assert_eq!(
            out,
            "Running (session s1, waiting for a password — typed input is hidden)\n\
             Sorry, try again.\n[sudo] password for u:"
        );
        assert_eq!(
            parse_frame(out.lines().next().unwrap()),
            Some(Frame::Running {
                session: "s1",
                waiting: Waiting::Password
            })
        );
    }

    #[test]
    fn a_prompt_waits_for_a_password_when_its_terminal_hides_what_it_reads() {
        assert_eq!(Waiting::of(false, true), Waiting::No, "not at a prompt yet");
        assert_eq!(Waiting::of(true, false), Waiting::Input);
        assert_eq!(Waiting::of(true, true), Waiting::Password);
    }

    #[test]
    fn frames_parse_back() {
        assert_eq!(
            parse_frame("Running (session b7x2k9m1q, waiting for input)"),
            Some(Frame::Running {
                session: "b7x2k9m1q",
                waiting: Waiting::Input
            })
        );
        assert_eq!(
            parse_frame("Running (session s1)"),
            Some(Frame::Running {
                session: "s1",
                waiting: Waiting::No
            })
        );
        assert_eq!(
            parse_frame("Stopped (session s1)"),
            Some(Frame::Stopped { session: "s1" })
        );
        assert_eq!(parse_frame("Exit code: 0"), None);
        assert_eq!(parse_frame("Running (a marathon)"), None);
        assert_eq!(parse_frame("Running (session )"), None);
    }

    #[test]
    fn every_report_frame_round_trips() {
        for status in [
            Status::Running {
                waiting: Waiting::Input,
            },
            Status::Running {
                waiting: Waiting::Password,
            },
            Status::Running {
                waiting: Waiting::No,
            },
            Status::Stopped,
        ] {
            let out = report("b7x2k9m1q", status, &lines("x"));
            let first = out.lines().next().unwrap();
            assert!(parse_frame(first).is_some(), "{first}");
        }
    }
}
