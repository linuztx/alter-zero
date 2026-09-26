#!/usr/bin/env bash
# Phase 108 — the [PROMPT] CLI shortcut and the --help page

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# the [PROMPT] CLI shortcut and the --help page (docs/cli.md).
# `alter-zero "hello there"` boots STRAIGHT into that turn — the ❯ bubble, the
# streamed reply and the Done summary appear with no key pressed — and its
# quit still prints the resume hint; `--resume {id} "again please"` reloads
# the transcript and runs the prompt as the next turn in the SAME rollout
# file; `-c "…"` does the same for the newest session here. And the help
# page: on the pane's tty it opens on 'Alter Zero' in the bold-cyan heading
# escape and carries the [PROMPT] argument, through a pipe the same page
# holds no escape at all, and a grammar error prints the clap-shaped trailer
# with exit 2. The pane must OUTLIVE the app to capture what it prints after
# restore, so each launch is wrapped in a shell that holds the pane open.
S108="${S}_cliprompt"
CLIP_DIR="$(mktemp -d "$SMOKE_TMP/cliprompt.XXXXXX")"
CLIPAPP="env $CFG_ENV_NOHIST ALTER_ZERO_SESSIONS_DIR=$CLIP_DIR ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN"
tmux new-session -d -s "$S108" -x 80 -y 24 "$CLIPAPP \"$USER_MSG\"; echo CLI_APP_EXITED; sleep 60"
clip_first_pane="$(wait_pane 20.1 "$S108" -S -60 -- -F "$SUMMARY_TURN1")" # the shortcut's turn settled, no key pressed
echo "==== Phase 108: alter-zero \"$USER_MSG\" — the turn ran from the command line ===="
printf '%s\n' "$clip_first_pane"
tmux send-keys -t "$S108" C-c # quit (empty composer)
wait_for 4 "$S108" -F "CLI_APP_EXITED"
clip_quit_pane="$(tmux capture-pane -t "$S108" -p -S -80)"
clip_hint_id="$(printf '%s\n' "$clip_quit_pane" | sed -n 's/.*--resume \([a-f0-9-]*\).*/\1/p' | tail -1)"
tmux kill-session -t "$S108" 2>/dev/null
# --resume {id} "prompt": the transcript reloads and the prompt runs next.
tmux new-session -d -s "$S108" -x 80 -y 24 "$CLIPAPP --resume $clip_hint_id \"again please\"; echo CLI_APP_EXITED; sleep 60"
clip_resume_pane=""
for _ in $(seq 1 134); do # the loaded turn's summary + the new turn's → 2× $SUMMARY_TURN1
	clip_resume_pane="$(tmux capture-pane -t "$S108" -p -S -100)"
	if [ "$(printf '%s' "$clip_resume_pane" | grep -cF "$SUMMARY_TURN1")" -ge 2 ]; then
		break
	fi
	sleep 0.15
done
sleep 0.3
echo "==== Phase 108: --resume {id} \"again please\" (reloaded, then the prompt's turn) ===="
printf '%s\n' "$clip_resume_pane"
tmux kill-session -t "$S108" 2>/dev/null
# -c "prompt": the newest session here, plus a third turn.
tmux new-session -d -s "$S108" -x 80 -y 24 "$CLIPAPP -c \"and once more\"; echo CLI_APP_EXITED; sleep 60"
clip_continue_pane=""
for _ in $(seq 1 134); do
	clip_continue_pane="$(tmux capture-pane -t "$S108" -p -S -140)"
	if [ "$(printf '%s' "$clip_continue_pane" | grep -cF "$SUMMARY_TURN1")" -ge 3 ]; then
		break
	fi
	sleep 0.15
done
sleep 0.3
clip_files="$(find "$CLIP_DIR" -type f -name 'rollout-*.jsonl' | wc -l | tr -d ' ')"
echo "==== Phase 108: -c \"and once more\" (rollout files: $clip_files) ===="
printf '%s\n' "$clip_continue_pane" | tail -20
tmux kill-session -t "$S108" 2>/dev/null
# The help page on a real tty: the raw pane (escapes kept — tmux re-encodes
# the binary's `ESC[1;36m` as its own `ESC[1m ESC[36m`, so the check is on
# the cyan `36m` landing right before the word) opens on the bold-cyan title.
tmux new-session -d -s "$S108" -x 100 -y 40 "$BIN --help; echo CLI_APP_EXITED; sleep 60"
wait_for 4 "$S108" -F "CLI_APP_EXITED"
clip_help_raw="$(tmux capture-pane -t "$S108" -p -e)"
clip_help_text="$(tmux capture-pane -t "$S108" -p)"
echo "==== Phase 108: --help on the pane's tty ===="
printf '%s\n' "$clip_help_text" | head -24
tmux kill-session -t "$S108" 2>/dev/null
# The same tty, with NO_COLOR set: the suite unsets that variable for its own
# fixtures (colour is what nine assertions read), which leaves the behaviour it
# is honouring untested — so pin it here, where it is ours rather than
# crossterm's. `cli::colour_enabled` must fall to `HelpStyle::Plain`: the words
# stay, every escape goes.
tmux new-session -d -s "$S108" -x 100 -y 40 "env NO_COLOR=1 $BIN --help; echo CLI_APP_EXITED; sleep 60"
wait_for 4 "$S108" -F "CLI_APP_EXITED"
clip_help_nocolor_raw="$(tmux capture-pane -t "$S108" -p -e)"
clip_help_nocolor="$(tmux capture-pane -t "$S108" -p)"
echo "==== Phase 108: --help on a tty under NO_COLOR=1 ===="
printf '%s\n' "$clip_help_nocolor" | head -6
tmux kill-session -t "$S108" 2>/dev/null
# …and through a pipe: the same words, no escape anywhere.
clip_help_piped="$("$BIN" --help 2>&1)"
clip_help_piped_exit=$?
clip_usage_err="$("$BIN" fix "the bug" 2>&1)"
clip_usage_err_exit=$?
echo "==== Phase 108: a grammar error → exit $clip_usage_err_exit ===="
printf '%s\n' "$clip_usage_err"
rm -rf "$CLIP_DIR" 2>/dev/null

# Phase 108: the [PROMPT] shortcut and the --help page.
expect_has "$clip_first_pane" -F "❯ $USER_MSG" "the command-line prompt was not committed as the user bubble"
expect_has "$clip_first_pane" -F "$EXPECT_REPLY" "the command-line prompt's turn never streamed its reply"
expect_has "$clip_first_pane" -F "$SUMMARY_TURN1" "the command-line prompt's turn never settled"
expect_has "$clip_quit_pane" -F "Resume this session with:" "quitting the shortcut's session printed no resume hint"
if [ -z "$clip_hint_id" ]; then
	fail "no '--resume {id}' line under the hint"
fi
expect_has "$clip_resume_pane" -F "❯ $USER_MSG" "--resume {id} \"prompt\" did not reload the first turn"
expect_has "$clip_resume_pane" -F "❯ again please" "--resume {id} \"prompt\" did not run the prompt as the next turn"
if [ "$(printf '%s' "$clip_resume_pane" | grep -cF "$SUMMARY_TURN1")" -lt 2 ]; then
	fail "the resumed session's prompt turn never settled"
fi
expect_has "$clip_continue_pane" -F "❯ and once more" "-c \"prompt\" did not run the prompt as the next turn"
if [ "$clip_files" != "1" ]; then
	fail "the prompt turns should append to the SAME rollout file, found $clip_files files"
fi
if [ "$(printf '%s\n' "$clip_help_text" | head -1)" != "Alter Zero" ]; then
	fail "--help on a tty does not open on 'Alter Zero' (got '$(printf '%s\n' "$clip_help_text" | head -1)')"
fi
expect_has "$clip_help_raw" -F "36mAlter Zero" "--help on a tty does not wear the bold-cyan heading escape on its title"
expect_has "$clip_help_raw" -F "36mUsage:" "--help on a tty does not wear the heading escape on 'Usage:'"
# …and the same page under NO_COLOR=1 keeps every word and drops every escape
# (`cli::colour_enabled`) — the production behaviour the suite's own `unset
# NO_COLOR` relies on being real.
if [ "$(printf '%s\n' "$clip_help_nocolor" | head -1)" != "Alter Zero" ]; then
	fail "--help under NO_COLOR=1 does not open on 'Alter Zero'"
fi
expect_has "$clip_help_nocolor" -F "Usage:" "--help under NO_COLOR=1 lost its 'Usage:' section"
if printf '%s' "$clip_help_nocolor_raw" | grep -qE "3[0-9]m|1m"; then
	fail "--help under NO_COLOR=1 still wears an SGR escape"
	printf '%s\n' "$clip_help_nocolor_raw" | head -4 | cat -v >&2
fi
expect_has "$clip_help_text" -F "[PROMPT]" "--help does not name the [PROMPT] argument"
if [ "$clip_help_piped_exit" != "0" ] || [ "$(printf '%s\n' "$clip_help_piped" | head -1)" != "Alter Zero" ]; then
	fail "piped --help should exit 0 opening on 'Alter Zero' (exit $clip_help_piped_exit)"
fi
expect_lacks "$clip_help_piped" -F "$(printf '\033')" "piped --help carries an escape sequence"
if [ "$clip_usage_err_exit" != "2" ]; then
	fail "a grammar error should exit 2, got $clip_usage_err_exit"
fi
expect_has "$clip_usage_err" -F "error: unexpected argument: the bug" "the grammar error does not lead with clap's 'error:' line naming the culprit"
expect_has "$clip_usage_err" -F "For more information, try '--help'." "the grammar error does not close on the --help pointer"
