#!/usr/bin/env bash
# Phase 37 — the input history PERSISTS across sessions

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# the input history PERSISTS across sessions (docs/history-persistence.md).
# Submit a distinctive message in one process — written to an isolated
# ALTER_ZERO_HISTORY_FILE — then launch a SECOND process against the same file:
# ↑ recalls the previous session's message and Ctrl+R finds it. Proves both the
# ↑/↓ recall and the Ctrl+R search span sessions (the seed loads the file into
# InputHistory::entries, which both read).
HISTFILE="$(mktemp -u "$SMOKE_TMP/hist.XXXXXX").jsonl"
HAPP="env $CFG_ENV ALTER_ZERO_HISTORY_FILE=$HISTFILE ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN"
PMSG="persist_across_sessions_42"
S37="${S}_persist"
launch "$S37" 80 24 "$HAPP"
submit "$S37" "$PMSG"
# The submit is flushed to the history file at the next loop tick — poll the
# file (this file's own rule: poll, don't fixed-sleep) rather than guessing.
persist_written=""
for _ in $(seq 1 40); do # up to ~4s
	if [ -f "$HISTFILE" ] && grep -qF "$PMSG" "$HISTFILE"; then
		persist_written="yes"
		break
	fi
	sleep 0.1
done
persist_file_contents="$(cat "$HISTFILE" 2>/dev/null)"
echo "==== Phase 37: history file after session A ===="
printf '%s\n' "$persist_file_contents"
tmux send-keys -t "$S37" C-c # empty composer → quit session A
sleep 0.3
tmux kill-session -t "$S37" 2>/dev/null

# Corrupt the file's tail with an invalid-UTF-8 line (a torn/interleaved append
# leaves such bytes): the lossy load must skip ONLY this line, not discard the
# whole history — so session B's recall below still works. With the old
# read_to_string load this single bad byte wiped all persisted history.
printf '\377\376 torn-not-valid-utf8\n' >>"$HISTFILE"

# Session B: a fresh process against the SAME history file.
S37B="${S}_persist_b"
launch "$S37B" 80 24 "$HAPP"
tmux send-keys -t "$S37B" Up # recall from the persisted (cross-session) history
sleep 0.3
persist_recall="$(tmux capture-pane -t "$S37B" -p)"
echo "==== Phase 37: session B — Up recalls the persisted message ===="
printf '%s\n' "$persist_recall"
tmux send-keys -t "$S37B" C-c # clears the recalled draft (non-empty composer)
sleep 0.2
tmux send-keys -t "$S37B" C-r # open reverse-i-search
sleep 0.2
tmux send-keys -t "$S37B" -l "persist_across"
sleep 0.3
persist_search="$(tmux capture-pane -t "$S37B" -p)"
echo "==== Phase 37: session B — Ctrl+R finds the persisted message ===="
printf '%s\n' "$persist_search"
tmux send-keys -t "$S37B" Escape
sleep 0.2
tmux kill-session -t "$S37B" 2>/dev/null
rm -f "$HISTFILE"

# Phase 37: the input history persists across sessions (docs/history-persistence.md).
if [ "$persist_written" != "yes" ]; then
	fail "the submitted message was never written to the history file (append broken)"
fi
expect_has "$persist_file_contents" -F "\"text\":\"$PMSG\"" "the history file lacks the JSONL record for the submitted message"
expect_has "$persist_recall" -F "$PMSG" "Up in a FRESH session did not recall the previous session's message (seed/persistence broken)"
expect_has "$persist_search" -F "reverse-i-search:" "Ctrl+R did not open the reverse-i-search line in the fresh session"
expect_has "$persist_search" -F "$PMSG" "Ctrl+R search did not find the persisted message in the fresh session"
