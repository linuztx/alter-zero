#!/usr/bin/env bash
# Phase 75 — the read-only /hooks MENU

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# the read-only /hooks MENU (docs/hooks-menu.md). Claude Code's
# /hooks browser over a planted hooks.json: the palette lists the command, it
# opens the inline framed events list ('Hooks', '{N} hooks configured', the ℹ
# read-only banner, the eleven events with counts + summaries in an aligned
# column, a ↓ overflow marker past the five-row window), Enter drills into
# matchers → hooks → details (the aligned field block, the REAL command
# word-wrapped in a rounded box, the modify note), Esc walks back one level
# at a time, and closing restores the composer.
S75="${S}_hooksmenu"
HK_CFG="$(mktemp -d)"
cat >"$HK_CFG/hooks.json" <<'HOOKS75'
{
  "hooks": {
    "PreToolUse": [
      { "matcher": "Bash",
        "hooks": [ { "type": "command",
          "command": "jq -re '.tool_input.command | test(\"rm -rf\") | not' >/dev/null || { echo 'no recursive deletes' >&2; exit 2; }" } ] }
    ],
    "Stop": [
      { "hooks": [ { "type": "command", "command": "./notify.sh" } ] }
    ]
  }
}
HOOKS75
APP_HK="env ALTER_ZERO_TELEMETRY=0 ALTER_ZERO_PROJECT_CONFIG=0 ALTER_ZERO_CONFIG_DIR=$HK_CFG ALTER_ZERO_CHECKPOINTS=0 ALTER_ZERO_HOOKS=1 ALTER_ZERO_HISTORY_FILE=/dev/null ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN"
launch "$S75" 100 35 "$APP_HK"
tmux send-keys -t "$S75" -l "/hooks"
sleep 0.4
hooks_palette="$(tmux capture-pane -t "$S75" -p)"
echo "==== Phase 75: the palette filtered to /hooks ===="
printf '%s\n' "$hooks_palette"
expect_has "$hooks_palette" -F "Browse the configured lifecycle hooks" "/hooks is missing from the slash-command palette"
tmux send-keys -t "$S75" Enter
sleep 0.6
hooks_events="$(tmux capture-pane -t "$S75" -p)"
echo "==== Phase 75: the events level ===="
printf '%s\n' "$hooks_events"
for expect in "Hooks" "2 hooks configured" "This menu is read-only" \
	"PreToolUse (1)" "Before tool execution" "↓ 5." \
	"Enter to confirm · Esc to cancel"; do
	expect_has "$hooks_events" -F "$expect" "the events level is missing '$expect'"
done
expect_has "$hooks_events" -F "❯ 1." "the first event row is not marked selected"
# Enter drills into PreToolUse's matchers.
tmux send-keys -t "$S75" Enter
sleep 0.4
hooks_matchers="$(tmux capture-pane -t "$S75" -p)"
echo "==== Phase 75: the matchers level ===="
printf '%s\n' "$hooks_matchers"
for expect in "PreToolUse - Matchers" "Input to command is the tool call" \
	"[User] Bash" "1 hook"; do
	expect_has "$hooks_matchers" -F "$expect" "the matchers level is missing '$expect'"
done
# Enter again: the matcher's hooks.
tmux send-keys -t "$S75" Enter
sleep 0.4
hooks_list="$(tmux capture-pane -t "$S75" -p)"
echo "==== Phase 75: the hooks level ===="
printf '%s\n' "$hooks_list"
for expect in "PreToolUse - Matcher: Bash" "[command] jq -re" "User Settings"; do
	expect_has "$hooks_list" -F "$expect" "the hooks level is missing '$expect'"
done
# Enter once more: the read-only detail page, the command whole in its box.
tmux send-keys -t "$S75" Enter
sleep 0.4
hooks_detail="$(tmux capture-pane -t "$S75" -p)"
echo "==== Phase 75: the detail page ===="
printf '%s\n' "$hooks_detail"
for expect in "Hook details" "Event:    PreToolUse" "Matcher:  Bash" \
	"Type:     command" "Source:   User settings (" "Command:" \
	"│ jq -re" "To modify or remove this hook" \
	"Esc to go back"; do
	expect_has "$hooks_detail" -F "$expect" "the detail page is missing '$expect'"
done
# The command survives WHOLE in the box — but word-wrapped, so a phrase can
# split across box rows ('no recursive / deletes'). Strip the borders, join
# the rows, and assert on the reassembled text (the unit test's approach).
hooks_box="$(printf '%s\n' "$hooks_detail" | sed -n 's/^  │ \(.*\)│[[:space:]]*$/\1/p' \
	| sed 's/[[:space:]]*$//' | tr '\n' ' ')"
expect_has "$hooks_box" -F "no recursive deletes" "the boxed command lost 'no recursive deletes' (got: $hooks_box)"
expect_lacks "$hooks_detail" -F "Enter to confirm" "the detail page offers Enter (it has nothing to confirm)"
# Esc walks back one level at a time; a fourth Esc closes to the composer.
tmux send-keys -t "$S75" Escape
sleep 0.3
hooks_back="$(tmux capture-pane -t "$S75" -p)"
expect_has "$hooks_back" -F "PreToolUse - Matcher: Bash" "Esc from the details did not return to the hooks level"
tmux send-keys -t "$S75" Escape
sleep 0.2
tmux send-keys -t "$S75" Escape
sleep 0.2
tmux send-keys -t "$S75" Escape
sleep 0.5
hooks_closed="$(tmux capture-pane -t "$S75" -p)"
echo "==== Phase 75: closed back to the composer ===="
printf '%s\n' "$hooks_closed"
expect_lacks "$hooks_closed" -F "This menu is read-only" "Esc from the events level did not close the menu"
expect_has "$hooks_closed" -F "dummy_model_name" "the composer (and its footer) did not come back"
tmux kill-session -t "$S75" 2>/dev/null || true
rm -rf "$HK_CFG"
