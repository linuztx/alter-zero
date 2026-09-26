//! When a call waiting on an interactive session returns
//! (`docs/interactive-shell.md`).
//!
//! Codex's `exec_command`/`write_stdin` return after a fixed wait the model
//! picks, and models pick badly: too short and they poll in a loop, too long
//! and every keystroke into a REPL waits out the clock. A call here returns
//! when the program **settles** — pure over what the session observed, so
//! the rule is tested without a process:
//!
//! 1. it **exited** — and nothing else settles it while keys the call sent
//!    are still being typed (`pty::keys` writes them a moment apart);
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
//!    command busy, and counts what was printed since the model last looked,
//!    so a question asked while the model was deciding to wait ends the wait
//!    too — after the call has looked for [`PROMPT_QUIET`] itself, which
//!    gives the probe its word;
//! 4. it is a **full-screen program that never stops drawing** — a clock, a
//!    meter, an animation — and has had [`SCREEN_BUSY`] since the call's
//!    last key: it never goes quiet, and it takes keys all the while, so what
//!    is on screen is the answer (a pure wait still waits; a display that
//!    leaves the terminal reading whole lines takes no keys, and is not one —
//!    `pty::session`);
//! 5. it has been silent for [`LINE_QUIET`] after an input, or
//!    [`LAUNCH_QUIET`] after a launch — and the kernel does not see it **at
//!    work** ([`Observation::busy`]): every command launches in a terminal
//!    (`docs/bash-tools.md`), and a quiet command is not a waiting one, so a
//!    `curl`, a link step or a `sleep` runs on to its exit or the call's
//!    timeout. Never for a pure [`WaitKind::Wait`] either: a build that
//!    pauses between lines is still working, so a wait returns only on an
//!    exit, a prompt, or its timeout, which is what lets the model wait for a
//!    long command in one call. A call that submitted a password gives the
//!    program [`CHECK_QUIET`] to answer it: sudo takes seconds to check one
//!    and says nothing meanwhile — as does a full-screen program that
//!    switched screens and has drawn nothing yet (btop gathering its first
//!    frame);
//! 6. a pure wait on a program already **reading key by key** at a prompt,
//!    with nothing printed since the model last looked, settles as soon as
//!    the probe has had its moment ([`PROMPT_QUIET`]): a REPL waits for the
//!    model, not the other way round;
//! 7. the call's timeout passed.

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
/// silent before an input call returns anyway: it may be waiting on a
/// line-terminated question, or on nothing it has said.
pub const LINE_QUIET: Duration = Duration::from_secs(2);

/// [`LINE_QUIET`] for a **launch**: every command starts in a terminal
/// (`docs/bash-tools.md`), and a batch command that is quiet for two seconds
/// — a download, a link step, a `terraform plan` — is not asking anything.
/// What the kernel cannot see at work (a network wait, `sudo`, no `/proc`)
/// returns after this long; what it sees at work never does.
pub const LAUNCH_QUIET: Duration = Duration::from_secs(10);

/// How long a launch or an input call watches a full-screen program that
/// keeps redrawing before it returns the screen anyway, counted from the
/// call's last key — as long as a silent program gets ([`LINE_QUIET`]).
pub const SCREEN_BUSY: Duration = Duration::from_secs(2);

/// How long a program may stay silent after a password the call submitted
/// ([`Observation::answer_pending`]) — or on a screen it switched to and has
/// not drawn on ([`Observation::undrawn`]) — before the call returns anyway.
/// sudo takes about two seconds to refuse a password and a network login
/// longer, and btop seconds to gather its first frame, while a command that
/// runs on silently once let in is just a quiet line.
pub const CHECK_QUIET: Duration = Duration::from_secs(10);

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
    /// Did the program print anything new to the call — since it began, or
    /// for a pure wait since the model last looked (`pty::session`)?
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
    /// Did the call submit a password the program has not answered yet —
    /// no visible text since the Enter (`pty::session`)? Its silence is the
    /// program checking it, for up to [`CHECK_QUIET`].
    pub answer_pending: bool,
    /// Are keys the call sent still being typed — written a moment apart
    /// (`pty::keys`)? Until the last is, the screen answers only some.
    pub typing: bool,
    /// Is a full-screen program that takes keys up — the alternate screen,
    /// the terminal out of line mode (`pty::session`)?
    pub full_screen: bool,
    /// Has the program switched to the alternate screen and drawn nothing on
    /// it (`pty::screen::Screen::undrawn`)? It is getting ready: its silence
    /// is no quiet line for up to [`CHECK_QUIET`], and its blank screen is
    /// neither a prompt nor a screen being drawn (`pty::session`).
    pub undrawn: bool,
    /// Since the call's last key was typed — or since it began, when it
    /// typed none — or since the first frame of a screen switched to
    /// meanwhile, when that came later (`pty::session`).
    pub since_keys: Duration,
    /// Has the program exited?
    pub exited: bool,
    /// Did the probe see every thread of the program **at work**
    /// (`pty::probe::Probe::Idle`)? Its silence then settles nothing: the
    /// command runs on to its exit or the call's timeout.
    pub busy: bool,
    /// Does the program read its terminal **key by key** — a REPL, an
    /// editor, a menu (`pty::spawn::LineMode::reads_keys`)?
    pub key_by_key: bool,
}

/// Why the call returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Settle {
    Exited,
    /// Quiet at a prompt — the frame says `waiting for input`, or `waiting
    /// for a password` at one that reads with echo off.
    Prompt,
    /// Silent for [`LINE_QUIET`] — or a full-screen program still drawing
    /// after [`SCREEN_BUSY`] with nothing seen to read its keys.
    Quiet,
    Timeout,
}

/// Has the call settled? `None` keeps waiting (see the module docs).
#[must_use]
pub fn settle(kind: WaitKind, timeout: Duration, seen: &Observation) -> Option<Settle> {
    if seen.exited {
        return Some(Settle::Exited);
    }
    if seen.typing {
        return (seen.elapsed >= timeout).then_some(Settle::Timeout);
    }
    if (seen.reading || seen.secret) && seen.quiet >= PROMPT_QUIET {
        return Some(Settle::Prompt);
    }
    // A program already waiting on keys, and nothing new since the model
    // last looked: nothing will come until it is typed into.
    if kind == WaitKind::Wait
        && !seen.output
        && seen.awaiting_keys
        && seen.key_by_key
        && seen.elapsed >= PROMPT_QUIET
    {
        return Some(Settle::Prompt);
    }
    // Output from before the call — a question asked while the model was
    // deciding to wait — can be quiet long enough the moment the call
    // begins: the call still looks for PROMPT_QUIET first, which is what
    // gives the probe its word (output during the call can be no quieter
    // than the call is old, so for it this changes nothing).
    if seen.output
        && seen.awaiting_keys
        && seen.quiet >= prompt_quiet(kind, true)
        && seen.elapsed >= PROMPT_QUIET
    {
        return Some(Settle::Prompt);
    }
    if kind != WaitKind::Wait && seen.full_screen && seen.output && seen.since_keys >= SCREEN_BUSY {
        return Some(if seen.awaiting_keys {
            Settle::Prompt
        } else {
            Settle::Quiet
        });
    }
    let line_quiet = if kind == WaitKind::Launch {
        LAUNCH_QUIET
    } else {
        LINE_QUIET
    };
    let line_quiet = if seen.answer_pending || seen.undrawn {
        line_quiet.max(CHECK_QUIET)
    } else {
        line_quiet
    };
    if kind != WaitKind::Wait && !seen.busy && seen.quiet >= line_quiet {
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
            answer_pending: false,
            typing: false,
            full_screen: false,
            undrawn: false,
            since_keys: ms(elapsed),
            exited: false,
            busy: false,
            key_by_key: false,
        }
    }

    const TIMEOUT: Duration = Duration::from_secs(10);

    #[test]
    fn a_full_screen_program_that_never_stops_drawing_still_answers() {
        // Seen driving `watch -n 0.1`: a screen redrawn every tenth of a
        // second never goes quiet, and the launch sat out its whole
        // two-minute timeout. A full-screen program takes keys while it
        // draws, so once it has had SCREEN_BUSY since the call's last key,
        // what is on screen is the answer.
        let drawing = |since_keys| Observation {
            full_screen: true,
            since_keys: ms(since_keys),
            ..seen(since_keys + 400, 50, true, true)
        };
        assert_eq!(settle(WaitKind::Input, TIMEOUT, &drawing(1_500)), None);
        for kind in [WaitKind::Launch, WaitKind::Input] {
            assert_eq!(
                settle(kind, TIMEOUT, &drawing(2_000)),
                Some(Settle::Prompt),
                "{kind:?}"
            );
        }
        // The probe saw nothing reading keys: the screen, not a prompt.
        let busy = Observation {
            awaiting_keys: false,
            ..drawing(2_000)
        };
        assert_eq!(settle(WaitKind::Input, TIMEOUT, &busy), Some(Settle::Quiet));
        // A pure wait is the model asking to watch it change.
        assert_eq!(settle(WaitKind::Wait, TIMEOUT, &drawing(5_000)), None);
        // On the main screen output that never stops is a busy command.
        let lines = Observation {
            full_screen: false,
            ..drawing(5_000)
        };
        assert_eq!(settle(WaitKind::Input, TIMEOUT, &lines), None);
    }

    #[test]
    fn a_screen_switched_to_and_not_yet_drawn_on_is_waited_on() {
        // btop 1.4 switches to the alternate screen, then probes its GPU for
        // seconds before it draws a thing: its silence is no quiet line
        // until CHECK_QUIET, so the launch shows the first frame, not a
        // blank.
        let undrawn = |quiet| Observation {
            undrawn: true,
            ..seen(quiet, quiet, true, false)
        };
        for kind in [WaitKind::Launch, WaitKind::Input] {
            assert_eq!(settle(kind, TIMEOUT, &undrawn(5_000)), None, "{kind:?}");
            assert_eq!(
                settle(kind, TIMEOUT, &undrawn(CHECK_QUIET.as_millis() as u64)),
                Some(Settle::Quiet),
                "{kind:?}"
            );
        }
    }

    #[test]
    fn nothing_settles_while_the_keys_are_still_being_typed() {
        // Keys go a moment apart (`pty::keys`): a call that looked before the
        // last of them was typed would report a screen the rest are still
        // changing — a quiet menu at its last item is no answer yet.
        let typing = Observation {
            typing: true,
            reading: true,
            ..seen(3_000, 3_000, true, true)
        };
        for kind in [WaitKind::Launch, WaitKind::Input, WaitKind::Wait] {
            assert_eq!(settle(kind, TIMEOUT, &typing), None, "{kind:?}");
        }
        // An exit, or the call's own timeout, still ends it.
        let exited = Observation {
            exited: true,
            ..typing
        };
        assert_eq!(
            settle(WaitKind::Input, TIMEOUT, &exited),
            Some(Settle::Exited)
        );
        assert_eq!(
            settle(WaitKind::Input, ms(2_000), &typing),
            Some(Settle::Timeout)
        );
    }

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
    fn a_password_being_checked_is_no_quiet_line() {
        // Submitted, and the program has not said a word about it: its
        // silence is the check — for as long as a check takes, no longer.
        let checking = |quiet| Observation {
            answer_pending: true,
            ..seen(quiet, quiet, true, false)
        };
        let long = ms(60_000);
        assert_eq!(settle(WaitKind::Input, long, &checking(5_000)), None);
        assert_eq!(
            settle(
                WaitKind::Input,
                long,
                &checking(CHECK_QUIET.as_millis() as u64)
            ),
            Some(Settle::Quiet),
            "a command that runs on silently after its password"
        );
        assert_eq!(
            settle(WaitKind::Input, ms(4_000), &checking(5_000)),
            Some(Settle::Timeout)
        );
        assert_eq!(
            settle(WaitKind::Input, long, &seen(5_000, 5_000, true, false)),
            Some(Settle::Quiet),
            "answered: the usual quiet line"
        );
    }

    #[test]
    fn a_question_from_before_the_wait_is_weighed_after_a_moment() {
        // A wait can begin on a question already quiet for seconds: it
        // looks for PROMPT_QUIET first, so the probe can say it is busy.
        let long = ms(10_000);
        assert_eq!(
            settle(WaitKind::Wait, long, &seen(100, 5_000, true, true)),
            None
        );
        assert_eq!(
            settle(WaitKind::Wait, long, &seen(500, 5_000, true, true)),
            Some(Settle::Prompt)
        );
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
    fn an_input_returns_after_a_silence_at_the_start_of_a_line() {
        let silent = seen(2100, LINE_QUIET.as_millis() as u64, true, false);
        assert_eq!(
            settle(WaitKind::Input, TIMEOUT, &silent),
            Some(Settle::Quiet)
        );
        let never_spoke = seen(2100, 2100, false, false);
        assert_eq!(
            settle(WaitKind::Input, TIMEOUT, &never_spoke),
            Some(Settle::Quiet),
            "a program waiting silently still hands control back"
        );
    }

    #[test]
    fn a_quiet_launch_the_kernel_cannot_see_returns_after_a_longer_silence() {
        // A quiet command is not a waiting one (docs/bash-tools.md): every
        // command launches in a terminal now, and two seconds of silence
        // handed back a `curl`, a link step or a `terraform plan` as
        // `Running` — then a completion notice nobody asked for.
        let long = ms(60_000);
        let silent = |quiet| seen(quiet, quiet, true, false);
        assert_eq!(
            settle(
                WaitKind::Launch,
                long,
                &silent(LINE_QUIET.as_millis() as u64)
            ),
            None
        );
        assert_eq!(
            settle(
                WaitKind::Launch,
                long,
                &silent(LAUNCH_QUIET.as_millis() as u64)
            ),
            Some(Settle::Quiet)
        );
        let never_spoke = seen(10_100, 10_100, false, false);
        assert_eq!(
            settle(WaitKind::Launch, long, &never_spoke),
            Some(Settle::Quiet),
            "a program waiting silently still hands control back"
        );
    }

    #[test]
    fn a_program_the_kernel_sees_at_work_is_never_settled_by_silence() {
        // `sleep 4; echo done` came back `Running` at 2.02 s and then exited
        // with nobody watching: the probe saw every thread at work the
        // whole time (`pty::probe::Probe::Idle`).
        let working = Observation {
            busy: true,
            ..seen(120_000, 120_000, true, false)
        };
        for kind in [WaitKind::Launch, WaitKind::Input] {
            assert_eq!(settle(kind, ms(600_000), &working), None, "{kind:?}");
            assert_eq!(
                settle(kind, ms(120_000), &working),
                Some(Settle::Timeout),
                "{kind:?}"
            );
        }
    }

    #[test]
    fn a_wait_on_a_program_reading_keys_with_nothing_new_returns_at_once() {
        // A REPL at the prompt the model already saw: nothing comes until it
        // is typed into, and a wait holding its budget over it only costs
        // the user the wait (docs/bash-tools.md).
        let at_prompt = |elapsed| Observation {
            key_by_key: true,
            ..seen(elapsed, 5_000, false, true)
        };
        let long = ms(120_000);
        assert_eq!(
            settle(WaitKind::Wait, long, &at_prompt(300)),
            None,
            "the probe gets its moment"
        );
        assert_eq!(
            settle(WaitKind::Wait, long, &at_prompt(600)),
            Some(Settle::Prompt)
        );
        // A line prompt behind a relay, or a busy line shaped like one, is
        // the model choosing to wait: it still waits.
        let line_prompt = Observation {
            key_by_key: false,
            ..at_prompt(600)
        };
        assert_eq!(settle(WaitKind::Wait, long, &line_prompt), None);
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
