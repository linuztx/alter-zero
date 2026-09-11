#!/usr/bin/env bash
# Phase 64 — auto + master permission modes

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# auto + master permission modes (docs/permissions.md). Shift+Tab
# cycles manual → edit → auto → master; in auto the dummy's "auto permission"
# demo is decided by the offline heuristic classifier — the read-only listing
# runs with a dim '⎿ Allowed by auto mode classifier' row on its finished
# cell, the delete is rejected red 'Denied by auto mode classifier', and NO
# prompt ever opens; the mode persists as "auto" in permissions.json; then in
# master the same demo runs both commands silently (still no prompt, no new
# denial). A fresh config dir so earlier phases' rules can't cover anything.
S64="${S}_automode"
SMOKE_CFG64="$(mktemp -d)"
APP_AUTO="env ALTER_ZERO_TELEMETRY=0 ALTER_ZERO_PROJECT_CONFIG=0 ALTER_ZERO_CONFIG_DIR=$SMOKE_CFG64 ALTER_ZERO_CHECKPOINTS=0 ALTER_ZERO_HISTORY_FILE=/dev/null ALTER_ZERO_STARTUP_DELAY_MS=300 $BIN"
launch "$S64" 100 44 "$APP_AUTO"
# Two Shift+Tab steps: manual → edit → auto. The footer's right-edge segment and
# the confirming toast must both say so.
tmux send-keys -t "$S64" BTab
sleep 0.2
tmux send-keys -t "$S64" BTab
sleep 0.3
auto_footer="$(tmux capture-pane -t "$S64" -p)"
echo "==== Phase 64: captured pane (mode cycled to auto) ===="
printf '%s\n' "$auto_footer"
submit "$S64" "auto permission demo"
auto_done=""
for _ in $(seq 1 250); do
	cap="$(tmux capture-pane -t "$S64" -p)"
	if printf '%s' "$cap" | grep -qF "Allowed by auto mode classifier" &&
		printf '%s' "$cap" | grep -qF "Denied by auto mode classifier" &&
		printf '%s' "$cap" | grep -qF "the delete was blocked"; then
		auto_done="$cap"
		break
	fi
	sleep 0.05
done
echo "==== Phase 64: captured pane (auto mode — classifier decided, no prompt) ===="
printf '%s\n' "$auto_done"
# Snapshot the persisted mode NOW — the master switch below overwrites it.
auto_json="$(cat "$SMOKE_CFG64/permissions.json" 2>/dev/null)"
# One more Shift+Tab step: auto → master.
tmux send-keys -t "$S64" BTab
sleep 0.3
master_footer="$(tmux capture-pane -t "$S64" -p)"
submit "$S64" "auto permission demo again"
master_done=""
for _ in $(seq 1 250); do
	cap="$(tmux capture-pane -t "$S64" -p)"
	if printf '%s' "$cap" | grep -qF "both commands ran to completion"; then
		master_done="$cap"
		break
	fi
	sleep 0.05
done
echo "==== Phase 64: captured pane (master mode — everything ran silently) ===="
printf '%s\n' "$master_done"
auto_scroll="$(tmux capture-pane -t "$S64" -p -S -400)"
tmux kill-session -t "$S64" 2>/dev/null
echo "==== Phase 64: auto mode classifies, master mode bypasses ===="
if ! printf '%s' "$auto_footer" | grep -qE "auto\s*$" ||
	! printf '%s' "$auto_footer" | grep -qF "Mode: auto"; then
	fail "two Shift+Tab steps did not land the footer/toast on auto"
fi
if [ -z "$auto_done" ]; then
	fail "the auto-mode demo never resolved both classifier verdicts"
fi
expect_lacks "$auto_scroll" -F "Do you want to proceed?" "a permission prompt opened in auto/master mode"
expect_has "$auto_done" -F "● Bash(ls -la)" "the allowed command's cell is missing"
expect_has "$auto_done" -F "⎿  Allowed by auto mode classifier" "the allowed cell lacks its classifier note row"
expect_has "$auto_done" -F "Reason:" "the denied cell lacks the classifier's reason line"
expect_has "$auto_json" -F '"mode": "auto"' "the auto mode did not persist to permissions.json"
if ! grep -qF '"mode": "master"' "$SMOKE_CFG64/permissions.json" 2>/dev/null; then
	fail "the master mode did not persist to permissions.json"
fi
expect_has "$master_footer" -E "master\s*$" "the third Shift+Tab step did not land the footer on master"
if [ -z "$master_done" ]; then
	fail "the master-mode demo never completed"
fi
if [ "$(printf '%s' "$auto_scroll" | grep -cF "Denied by auto mode classifier")" != "1" ]; then
	fail "master mode denied (or re-denied) a command; only auto's single denial may exist"
fi
if [ "$(printf '%s' "$auto_scroll" | grep -cF "Allowed by auto mode classifier")" != "1" ]; then
	fail "master mode noted a classifier approval; only auto's single note may exist"
fi
rm -rf "$SMOKE_CFG64"
