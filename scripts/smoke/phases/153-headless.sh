#!/usr/bin/env bash
# Phase 153 — alter-zero -p on a terminal prints the run and leaves the terminal as it was

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# Headless mode on a real tty (docs/headless.md). Pipes are cargo test's
# (tests/headless.rs); what only a terminal can show is that a headless run
# never touches one. From a tmux pane, `alter-zero -p` prints the run's log
# and its answer and exits 0; the tty's modes are what they were before it
# (`stty -g` before and after); and the raw byte stream (pipe-pane) holds
# none of the escapes the TUI's viewport writes — no cursor query (DSR), no
# alternate screen, no hidden cursor, no bracketed paste, no synchronized
# update. The pane outlives the run so its last lines can be read. Then the
# two flags `-p` shares with the TUI, on the TUI: `--permission-mode` and
# `-C` show in its footer.
S153="${S}_headless"
HL_WORK="$(work_dir headless)"
HL_RAW="$SMOKE_TMP/raw153.txt"
HL_APP="env $CFG_ENV_NOHIST ALTER_ZERO_DUMMY=1 ALTER_ZERO_STARTUP_DELAY_MS=0 ALTER_ZERO_CHUNK_DELAY_MS=0 $BIN_ABS"
# The half-second lets pipe-pane attach before the run writes anything.
tmux new-session -d -s "$S153" -c "$HL_WORK" -x 100 -y 40 \
	"sleep 0.5; before=\$(stty -g); $HL_APP -p \"$USER_MSG\"; code=\$?; after=\$(stty -g); \
	if [ \"\$before\" = \"\$after\" ]; then echo HL_TTY_KEPT; else echo HL_TTY_CHANGED; fi; \
	echo HL_EXIT=\$code; sleep 60"
tmux pipe-pane -t "$S153" -o "cat > $HL_RAW"
hl_pane="$(wait_pane 30 "$S153" -S -80 -- -F "HL_EXIT=")" || fail "the headless run never exited"
tmux pipe-pane -t "$S153"
dump "alter-zero -p \"$USER_MSG\" on a tty" "$hl_pane"
tmux kill-session -t "$S153" 2>/dev/null

# The two flags `-p` shares with the TUI (docs/cli.md): `--permission-mode`
# seeds the session's mode — the footer's right edge says so, over the
# suite's exported `manual` — and `-C` runs it in that directory, the footer
# naming it. Launched from the work dir's parent so `-C` is what moves it.
S153T="${S}_headlesstui"
launch -c "$SMOKE_HOME" "$S153T" 100 30 "$APP_ABS --permission-mode master -C $HL_WORK"
hl_tui="$(wait_pane 10 "$S153T" -F "master")" || fail "the TUI never showed --permission-mode's mode"
dump "the TUI under --permission-mode master -C ~/headless" "$hl_tui"
keys "$S153T" C-c
tmux kill-session -t "$S153T" 2>/dev/null

expect_has "$hl_tui" -E "~/headless +master" "the footer does not show -C's directory beside --permission-mode's mode"
expect_has "$hl_pane" -F "HL_EXIT=0" "the headless run did not exit 0"
expect_has "$hl_pane" -F "HL_TTY_KEPT" "the headless run left the terminal's modes changed"
expect_has "$hl_pane" -F "● Read(about.py)" "the run's log does not name its tool calls"
expect_has "$hl_pane" -F "Done in " "the run's log does not close on its footer"
expect_has "$hl_pane" -F "/login" "the answer never reached the terminal"
expect_lacks "$hl_pane" -F "────" "the headless run drew the TUI's input box"
for hl_esc in '[6n' '[?1049h' '[?25l' '[?2004h' '[?2026h'; do
	if grep -qF "$(printf '\033')$hl_esc" "$HL_RAW"; then
		fail "the headless run wrote the terminal escape ESC$hl_esc"
	fi
done

smoke_finish
