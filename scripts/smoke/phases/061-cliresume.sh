#!/usr/bin/env bash
# Phase 61 — CLI resume

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# CLI resume — --continue / --resume {id} / bare --resume, and
# the exit hint (docs/cli.md). Instance 1 records a turn; quitting prints
# 'Resume this session with: {bin} --resume {id}' below the restored terminal,
# the id naming the rollout file (only a session WITH history hints — an empty
# quit prints nothing). '--continue' relaunches straight into the old
# conversation (no picker, repainted inline) and appends to the SAME file;
# '--resume {id}' does the same by id; bare '--resume' boots into the picker;
# and '--continue' over an empty sessions dir fails fast on stderr (exit 1,
# no TUI). The pane must OUTLIVE the app to capture what it prints after
# restore, so each launch is wrapped in a shell that holds the pane open.
S61="${S}_cliresume"
CLIR_DIR="$(mktemp -d "$SMOKE_TMP/cliresume.XXXXXX")"
CLIAPP="env $CFG_ENV_NOHIST ALTER_ZERO_SESSIONS_DIR=$CLIR_DIR ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN"
tmux new-session -d -s "$S61" -x 80 -y 24 "$CLIAPP; echo CLI_APP_EXITED; sleep 60"
sleep 0.4
submit "$S61" "$USER_MSG"
wait_summaries 20.1 "$S61" 1 -S -40 >/dev/null # instance 1, turn 1 settles
tmux send-keys -t "$S61" C-c # quit instance 1 (empty composer)
wait_for 4 "$S61" -F "CLI_APP_EXITED"
cli_quit_pane="$(tmux capture-pane -t "$S61" -p -S -80)"
echo "==== Phase 61: pane after quitting a recorded session (the exit hint) ===="
printf '%s\n' "$cli_quit_pane"
# The advertised id must name the recorded rollout file.
cli_hint_id="$(printf '%s\n' "$cli_quit_pane" | sed -n 's/.*--resume \([a-f0-9-]*\).*/\1/p' | tail -1)"
cli_rollout="$(find "$CLIR_DIR" -type f -name 'rollout-*.jsonl' | head -1)"
tmux kill-session -t "$S61" 2>/dev/null
# --continue: straight into the old conversation, appending to the same file.
tmux new-session -d -s "$S61" -x 80 -y 24 "$CLIAPP --continue; echo CLI_APP_EXITED; sleep 60"
cli_continue_pane="$(wait_pane 3 "$S61" -S -80 -- -F "$EXPECT_REPLY")" # the loaded conversation repaints at startup
echo "==== Phase 61: --continue relaunch (old conversation repainted, no picker) ===="
printf '%s\n' "$cli_continue_pane"
submit "$S61" "again please"
# The follow-up: the loaded summary plus its own make two.
cli_continue_appended="$(wait_summaries 20.1 "$S61" 2 -S -60)"
sleep 0.3
cli_files_after_continue="$(find "$CLIR_DIR" -type f -name 'rollout-*.jsonl' | wc -l | tr -d ' ')"
tmux kill-session -t "$S61" 2>/dev/null
# --resume {id}: the hint's id resolves to the same session.
tmux new-session -d -s "$S61" -x 80 -y 24 "$CLIAPP --resume $cli_hint_id; echo CLI_APP_EXITED; sleep 60"
cli_resume_id_pane="$(wait_pane 3 "$S61" -S -100 -- -F "again please")"
echo "==== Phase 61: --resume {id} relaunch ===="
printf '%s\n' "$cli_resume_id_pane"
tmux kill-session -t "$S61" 2>/dev/null
# Bare --resume: the /resume picker as the first screen.
tmux new-session -d -s "$S61" -x 80 -y 24 "$CLIAPP --resume; echo CLI_APP_EXITED; sleep 60"
cli_picker_pane="$(wait_pane 3 "$S61" -F "R E S U M E")"
echo "==== Phase 61: bare --resume (the picker as the first screen) ===="
printf '%s\n' "$cli_picker_pane"
tmux kill-session -t "$S61" 2>/dev/null
# An EMPTY session (no history) must print no hint on quit.
tmux new-session -d -s "$S61" -x 80 -y 24 "$CLIAPP; echo CLI_APP_EXITED; sleep 60"
sleep 0.6
tmux send-keys -t "$S61" C-c
wait_for 4 "$S61" -F "CLI_APP_EXITED"
cli_empty_quit_pane="$(tmux capture-pane -t "$S61" -p -S -40)"
echo "==== Phase 61: pane after quitting an EMPTY session (no hint expected) ===="
printf '%s\n' "$cli_empty_quit_pane"
tmux kill-session -t "$S61" 2>/dev/null
# --continue with nothing to continue: fail fast on stderr, exit 1, no TUI.
CLIR_EMPTY="$(mktemp -d "$SMOKE_TMP/cliempty.XXXXXX")"
cli_nothing_msg="$(env $CFG_ENV_NOHIST ALTER_ZERO_SESSIONS_DIR="$CLIR_EMPTY" "$BIN" --continue 2>&1)"
cli_nothing_exit=$?
rm -rf "$CLIR_EMPTY" 2>/dev/null
echo "==== Phase 61: --continue with no sessions → '$cli_nothing_msg' (exit $cli_nothing_exit) ===="

# Phase 61: CLI resume — the exit hint, --continue, --resume {id}, bare --resume.
expect_has "$cli_quit_pane" -F "Resume this session with:" "quitting a recorded session printed no 'Resume this session with:' hint"
if [ -z "$cli_hint_id" ]; then
	fail "no '--resume {id}' command line found under the hint"
fi
case "$(basename "${cli_rollout:-none}")" in
*"$cli_hint_id"*) : ;;
*)
	fail "the hinted id '$cli_hint_id' does not name the rollout file '$(basename "${cli_rollout:-none}")'"
	;;
esac
expect_has "$cli_continue_pane" -F "❯ $USER_MSG" "--continue did not repaint the saved user message inline"
expect_has "$cli_continue_pane" -F "$EXPECT_REPLY" "--continue did not repaint the saved assistant reply inline"
expect_lacks "$cli_continue_pane" -F "R E S U M E" "--continue opened the picker instead of loading directly"
expect_has "$cli_continue_appended" -F "❯ again please" "the follow-up turn on the --continue'd session never ran"
if [ "$cli_files_after_continue" != "1" ]; then
	fail "--continue should append to the SAME rollout file, found $cli_files_after_continue files"
fi
expect_has "$cli_resume_id_pane" -F "❯ $USER_MSG" "--resume {id} did not reload the session the hint advertised"
expect_has "$cli_picker_pane" -F "R E S U M E" "bare --resume did not boot into the session picker"
expect_has "$cli_picker_pane" -F "$USER_MSG" "the startup picker does not list the saved session's preview"
expect_lacks "$cli_empty_quit_pane" -F "Resume this session with:" "quitting an EMPTY session must not print the resume hint"
if [ "$cli_nothing_exit" != "1" ]; then
	fail "--continue with no sessions should exit 1, got $cli_nothing_exit"
fi
expect_has "$cli_nothing_msg" -F "No conversation found to continue" "--continue with no sessions printed '$cli_nothing_msg' instead of the fail-fast message"
