#!/usr/bin/env bash
# Phase 31 — /resume SESSION RECORDING + PICKER

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# /resume SESSION RECORDING + PICKER (docs/resume.md). Every
# conversation records to a rollout JSONL file under ALTER_ZERO_SESSIONS_DIR
# (created lazily on the first user message — the session_meta line first). A
# second launch's /resume opens the full-screen session picker (the
# slash-tiled R E S U M E title, a "Type to search" line, the saved session's
# `❯ {age} {preview}` row); Enter loads the conversation back inline — the old
# exchange repainted from the file — and a follow-up turn APPENDS to the SAME
# file (still exactly one rollout). /clear then starts a FRESH file: the next
# message must land in a second rollout, the resumed one untouched.
S31="${S}_resume"
RESUME_DIR="$(mktemp -d "$SMOKE_TMP/sessions.XXXXXX")"
RAPP="env $CFG_ENV ALTER_ZERO_SESSIONS_DIR=$RESUME_DIR ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN"
launch "$S31" 80 24 "$RAPP"
submit "$S31" "$USER_MSG"
wait_for 20.1 "$S31" -S -40 -- -F "$SUMMARY_TURN1" # instance 1, turn 1
tmux send-keys -t "$S31" C-c # quit instance 1 (empty composer)
sleep 0.4
tmux kill-session -t "$S31" 2>/dev/null
resume_files_after_one="$(find "$RESUME_DIR" -type f -name 'rollout-*.jsonl' | wc -l | tr -d ' ')"
resume_first_file="$(find "$RESUME_DIR" -type f -name 'rollout-*.jsonl' | head -1)"
resume_file_head="$(head -c 300 "$resume_first_file" 2>/dev/null)"
echo "==== recorded rollout head (instance 1's session file) ===="
printf '%s\n' "$resume_file_head"
launch "$S31" 80 24 "$RAPP"
tmux send-keys -t "$S31" -l "/resume"
sleep 0.3
tmux send-keys -t "$S31" Enter # run the palette's highlighted /resume
sleep 0.6
resume_picker="$(tmux capture-pane -t "$S31" -p)"
echo "==== captured pane (/resume — the session picker on the alt screen) ===="
printf '%s\n' "$resume_picker"
tmux send-keys -t "$S31" Enter # resume the highlighted session
sleep 0.8
resume_loaded="$(tmux capture-pane -t "$S31" -p -S -60)"
echo "==== captured pane (Enter — the saved conversation repainted inline) ===="
printf '%s\n' "$resume_loaded"
submit "$S31" "again please"
resume_appended=""
for _ in $(seq 1 134); do # the follow-up turn (this process's turn 1 → the second $SUMMARY_TURN1)
	resume_appended="$(tmux capture-pane -t "$S31" -p -S -200)"
	if [ "$(printf '%s' "$resume_appended" | grep -cF "$SUMMARY_TURN1")" -ge 2 ]; then
		break
	fi
	sleep 0.15
done
echo "==== captured pane (follow-up turn on the resumed session) ===="
printf '%s\n' "$resume_appended"
resume_files_after_append="$(find "$RESUME_DIR" -type f -name 'rollout-*.jsonl' | wc -l | tr -d ' ')"
# 20 KB, not 2: one turn's records now carry a numbered read and a diff, so a
# small tail no longer reaches back to the user message that opened it.
resume_appended_tail="$(tail -c 20000 "$resume_first_file" 2>/dev/null)"
tmux send-keys -t "$S31" -l "/clear"
sleep 0.3
tmux send-keys -t "$S31" Enter
sleep 0.4
submit "$S31" "fresh session"
wait_for 20.1 "$S31" -S -40 -- -F "$SUMMARY_TURN2" # post-/clear turn (this process's turn 2)
sleep 0.3
resume_files_after_clear="$(find "$RESUME_DIR" -type f -name 'rollout-*.jsonl' | wc -l | tr -d ' ')"
tmux kill-session -t "$S31" 2>/dev/null

# Phase 31: /resume — record a session, pick it, load it, append to it.
if [ "$resume_files_after_one" != "1" ]; then
	fail "instance 1 should have recorded exactly one rollout file, found $resume_files_after_one"
fi
expect_has "$resume_file_head" -F '"type":"session_meta"' "the rollout file does not open with a session_meta line"
expect_has "$resume_picker" -F "R E S U M E" "/resume did not open the session picker (no slash-tiled R E S U M E title)"
expect_has "$resume_picker" -F "Type to search" "the picker's search line placeholder is missing"
expect_has "$resume_picker" -F "Filter: [Cwd] All" "the picker's Filter toolbar tab pair is missing (or not defaulting to Cwd)"
expect_has "$resume_picker" -F "Sort: [Updated] Created" "the picker's Sort toolbar tab pair is missing (or not defaulting to Updated)"
expect_has "$resume_picker" -E "ago|now" "the picker shows no humanized session age (neither '…s ago' nor 'now')"
if ! printf '%s' "$resume_picker" | grep -qF "❯" || ! printf '%s' "$resume_picker" | grep -qF "$USER_MSG"; then
	fail "the saved session's preview row ('❯ {age} $USER_MSG') is not listed"
fi
expect_has "$resume_loaded" -F "❯ $USER_MSG" "resuming did not repaint the saved user message inline"
expect_has "$resume_loaded" -F "$EXPECT_REPLY" "resuming did not repaint the saved assistant reply inline"
expect_has "$resume_appended" -F "❯ again please" "the follow-up turn on the resumed session never ran"
if [ "$resume_files_after_append" != "1" ]; then
	fail "the follow-up turn should append to the SAME rollout file, found $resume_files_after_append files"
fi
expect_has "$resume_appended_tail" -F "again please" "the resumed rollout file did not gain the follow-up message"
if [ "$resume_files_after_clear" != "2" ]; then
	fail "/clear should start a fresh rollout file (expected 2 files, found $resume_files_after_clear)"
fi
