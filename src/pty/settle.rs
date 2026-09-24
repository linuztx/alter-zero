//! When a call waiting on an interactive session returns
//! (`docs/interactive-shell.md`).
//!
//! Codex's `exec_command`/`write_stdin` return after a fixed wait the model
//! picks, and models pick badly: too short and they poll in a loop, too long
//! and every keystroke into a REPL waits out the clock. A call here returns
//! when the program **settles** — pure over what the session observed, so
//! the rule is tested without a process:
//!
//! 1. it **exited**;
//! 2. it has been quiet for [`PROMPT_QUIET`] with a thread **seen reading
//!    its terminal** — the kernel's word, from Linux's `/proc`
//!    (`pty::probe`), which needs no prompt and no output: a bare `read x`
//!    or a `cat` waits too — or with the terminal **reading a line with echo
//!    off**, a password prompt, told by the terminal's own mode even where
//!    `/proc` is blind (sudo runs as root);
//! 3. it printed something and went quiet for [`PROMPT_QUIET`] with the
//!    screen **awaiting keys** — a prompt, or a full-screen program done
//!    drawing (`pty::session` decides, from the screen, the transcript and
//!    the terminal's mode); a pure [`WaitKind::Wait`] needs
//!    [`WAIT_PROMPT_QUIET`], since the model waits because it believes the
//!    command busy;
//! 4. it has been silent for [`LINE_QUIET`] — except for a pure
//!    [`WaitKind::Wait`]: a build that pauses between lines is still working,
//!    so a wait returns only on an exit, a prompt, or its timeout, which is
//!    what lets the model wait for a long command in one call;
//! 5. the call's timeout passed.

use std::time::Duration;

/// How long a program sitting at a prompt must stay quiet before the call
/// says it is waiting for input — long enough that output arriving in two
/// writes is not cut in half, short enough that a REPL answers briskly.
pub const PROMPT_QUIET: Duration = Duration::from_millis(500);

/// How long a **pure wait** needs a prompt that appeared during it to stay
/// quiet — longer, since a wait is the model saying the command is busy, and
/// a line a busy command leaves open while it works (`Reading package
/// lists... `) looks just like one. A question still ends the wait in
/// seconds.
pub const WAIT_PROMPT_QUIET: Duration = Duration::from_secs(3);

/// How long a prompt must stay quiet to settle — or be reported as waiting
/// at the end of — a call of `kind`, `output` saying whether the program
/// printed anything since the call began: [`WAIT_PROMPT_QUIET`] for a pure
/// wait that saw it print, else [`PROMPT_QUIET`] (a wait that saw nothing
/// new found the program sitting where it was throughout).
#[must_use]
pub fn prompt_quiet(kind: WaitKind, output: bool) -> Duration {
    if kind == WaitKind::Wait && output {
        WAIT_PROMPT_QUIET
    } else {
        PROMPT_QUIET
    }
}

/// How long a program that left the cursor at the start of a line must stay
/// silent before a launch or an input call returns anyway: it may be waiting
/// on a line-terminated question, or on nothing it has said.
pub const LINE_QUIET: Duration = Duration::from_secs(2);

/// What the waiting call did before it began to wait.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitKind {
    /// Started the command (`bash` with `tty`).
    Launch,
    /// Typed into it (`bash_session` with `input`).
    Input,
    /// Nothing — a `bash_session` call that only waits.
    Wait,
}

/// What the session looked like at one poll.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Observation {
    /// Since the call began to wait.
    pub elapsed: Duration,
    /// Since the last output arrived — or since the call began, when none has.
    pub quiet: Duration,
    /// Did any output arrive since the call began?
    pub output: bool,
    /// Does the screen look like it is waiting for keys?
    pub awaiting_keys: bool,
    /// Did the probe see a thread blocked reading the terminal
    /// (`pty::probe`)?
    pub reading: bool,
    /// Is the terminal reading a line with echo off — a password prompt —
    /// that the program put up since it was last typed into
    /// (`pty::session`)?
    pub secret: bool,
    /// Has the program exited?
    pub exited: bool,
}

/// Why the call returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Settle {
    Exited,
    /// Quiet at a prompt — the frame says `waiting for input`, or `waiting
    /// for a password` at one that reads with echo off.
    Prompt,
    /// Silent for [`LINE_QUIET`].
    Quiet,
    Timeout,
}

/// Has the call settled? `None` keeps waiting (see the module docs).
#[must_use]
pub fn settle(kind: WaitKind, timeout: Duration, seen: &Observation) -> Option<Settle> {
    if seen.exited {
        return Some(Settle::Exited);
    }
    if (seen.reading || seen.secret) && seen.quiet >= PROMPT_QUIET {
        return Some(Settle::Prompt);
    }
    if seen.output && seen.awaiting_keys && seen.quiet >= prompt_quiet(kind, true) {
        return Some(Settle::Prompt);
    }
    if kind != WaitKind::Wait && seen.quiet >= LINE_QUIET {
        return Some(Settle::Quiet);
    }
    (seen.elapsed >= timeout).then_some(Settle::Timeout)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    fn seen(elapsed: u64, quiet: u64, output: bool, awaiting_keys: bool) -> Observation {
        Observation {
            elapsed: ms(elapsed),
            quiet: ms(quiet),
            output,
            awaiting_keys,
            reading: false,
            secret: false,
            exited: false,
        }
    }

    const TIMEOUT: Duration = Duration::from_secs(10);

    #[test]
    fn an_exit_settles_at_once() {
        let exited = Observation {
            exited: true,
            ..seen(5, 0, false, false)
        };
        for kind in [WaitKind::Launch, WaitKind::Input, WaitKind::Wait] {
            assert_eq!(settle(kind, TIMEOUT, &exited), Some(Settle::Exited));
        }
    }

    #[test]
    fn a_program_seen_reading_its_terminal_settles_as_waiting() {
        // The probe's word (`pty::probe`): a read blocked on the terminal is
        // a program waiting for input — with no prompt, no output, in a
        // pure wait alike, after the ordinary prompt quiet.
        let reading = Observation {
            reading: true,
            ..seen(600, 600, false, false)
        };
        for kind in [WaitKind::Launch, WaitKind::Input, WaitKind::Wait] {
            assert_eq!(settle(kind, TIMEOUT, &reading), Some(Settle::Prompt));
        }
        let early = Observation {
            reading: true,
            ..seen(300, 300, false, false)
        };
        assert_eq!(settle(WaitKind::Launch, TIMEOUT, &early), None);
    }

    #[test]
    fn a_password_prompt_settles_as_waiting_whatever_the_call_saw() {
        // The terminal reading a hidden line — sudo, ssh, getpass: waiting,
        // in a pure wait too whose output all came before it began (sudo's
        // `Sorry, try again.` landing between two calls), and whatever the
        // screen looks like.
        let secret = Observation {
            secret: true,
            ..seen(600, 600, false, false)
        };
        for kind in [WaitKind::Launch, WaitKind::Input, WaitKind::Wait] {
            assert_eq!(settle(kind, TIMEOUT, &secret), Some(Settle::Prompt));
        }
        let early = Observation {
            secret: true,
            ..seen(300, 300, false, false)
        };
        assert_eq!(settle(WaitKind::Wait, TIMEOUT, &early), None);
    }

    #[test]
    fn a_quiet_prompt_settles_as_waiting_for_input() {
        let at_prompt = seen(900, PROMPT_QUIET.as_millis() as u64, true, true);
        for kind in [WaitKind::Launch, WaitKind::Input] {
            assert_eq!(settle(kind, TIMEOUT, &at_prompt), Some(Settle::Prompt));
        }
    }

    #[test]
    fn a_pure_wait_needs_a_longer_silence_at_a_prompt() {
        // A wait is the model saying the command is busy: a line left open
        // for a moment (`Reading package lists... `) is not enough to end it.
        let brief = seen(900, PROMPT_QUIET.as_millis() as u64, true, true);
        assert_eq!(settle(WaitKind::Wait, TIMEOUT, &brief), None);
        let long = seen(4000, WAIT_PROMPT_QUIET.as_millis() as u64, true, true);
        assert_eq!(settle(WaitKind::Wait, TIMEOUT, &long), Some(Settle::Prompt));
        assert_eq!(prompt_quiet(WaitKind::Wait, true), WAIT_PROMPT_QUIET);
        assert_eq!(
            prompt_quiet(WaitKind::Wait, false),
            PROMPT_QUIET,
            "a wait that saw nothing new: the program sat where it was throughout"
        );
        for output in [true, false] {
            assert_eq!(prompt_quiet(WaitKind::Launch, output), PROMPT_QUIET);
            assert_eq!(prompt_quiet(WaitKind::Input, output), PROMPT_QUIET);
        }
    }

    #[test]
    fn a_prompt_still_printing_is_left_to_finish() {
        assert_eq!(
            settle(WaitKind::Input, TIMEOUT, &seen(300, 100, true, true)),
            None
        );
    }

    #[test]
    fn a_prompt_needs_output_since_the_call_began() {
        // The old prompt is still on screen, but nothing has answered the
        // input yet — the program is still thinking.
        let unanswered = seen(900, 900, false, true);
        assert_eq!(settle(WaitKind::Input, TIMEOUT, &unanswered), None);
        assert_eq!(settle(WaitKind::Wait, TIMEOUT, &unanswered), None);
    }

    #[test]
    fn a_launch_or_input_returns_after_a_silence_at_the_start_of_a_line() {
        let silent = seen(2100, LINE_QUIET.as_millis() as u64, true, false);
        assert_eq!(
            settle(WaitKind::Launch, TIMEOUT, &silent),
            Some(Settle::Quiet)
        );
        assert_eq!(
            settle(WaitKind::Input, TIMEOUT, &silent),
            Some(Settle::Quiet)
        );
        let never_spoke = seen(2100, 2100, false, false);
        assert_eq!(
            settle(WaitKind::Launch, TIMEOUT, &never_spoke),
            Some(Settle::Quiet),
            "a program waiting silently still hands control back"
        );
    }

    #[test]
    fn a_wait_rides_out_a_silence_between_lines() {
        let between_lines = seen(5000, 4000, true, false);
        assert_eq!(settle(WaitKind::Wait, TIMEOUT, &between_lines), None);
    }

    #[test]
    fn the_timeout_ends_any_wait() {
        let busy = seen(10_000, 10, true, false);
        for kind in [WaitKind::Launch, WaitKind::Input, WaitKind::Wait] {
            assert_eq!(settle(kind, TIMEOUT, &busy), Some(Settle::Timeout));
        }
    }

    #[test]
    fn an_exit_outranks_a_timeout() {
        let late_exit = Observation {
            exited: true,
            ..seen(20_000, 0, true, false)
        };
        assert_eq!(
            settle(WaitKind::Wait, TIMEOUT, &late_exit),
            Some(Settle::Exited)
        );
    }
}
