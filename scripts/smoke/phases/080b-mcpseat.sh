#!/usr/bin/env bash
# Phase 80b — the /mcp MANAGER's cursor seat with NOTHING configured

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# An unconfigured /mcp lists no server rows, so there is no highlighted ❯ for
# the hidden cursor to rest on, and the seat used to fall to the far corner of
# the bottom rule — where a terminal with a cursor-move animation (kitty) flew
# to nowhere on every open (the reported bug). It rests just past the closing
# hint now, the Ctrl+O / Ctrl+D / /resume overlays' rule (docs/mcp.md,
# docs/view-flow.md). tmux reports the pane's cursor cell whether or not the
# cursor is shown, so the seat is checked the way Phase 55 checks the
# permission prompt's: `#{cursor_y}` is 0-based, grep -n 1-based.
S80B="${S}_mcpseat"
MCP_CFG="$(mktemp -d)"
APP_MCP="env ALTER_ZERO_TELEMETRY=0 ALTER_ZERO_PROJECT_CONFIG=0 ALTER_ZERO_CONFIG_DIR=$MCP_CFG ALTER_ZERO_CHECKPOINTS=0 ALTER_ZERO_HISTORY_FILE=/dev/null ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN"
tmux new-session -d -s "$S80B" -x 100 -y 36 "$APP_MCP"
sleep 0.8
tmux send-keys -t "$S80B" -l "/mcp"
sleep 0.4
tmux send-keys -t "$S80B" Enter
wait_for 10 "$S80B" -F "No MCP servers configured"
mcp_empty="$(tmux capture-pane -t "$S80B" -p)"
mcp_cursor="$(tmux display-message -p -t "$S80B" '#{cursor_x} #{cursor_y}')"
dump "the unconfigured list (cursor at $mcp_cursor)" "$mcp_empty"
expect_has "$mcp_empty" -F "0 servers" "the empty list is missing its count"
expect_has "$mcp_empty" -F "~/.alter-zero/mcp.json" "the empty list does not name the config file to add"
mcp_hint="↑/↓ to navigate · Enter to confirm · Esc to cancel"
mcp_hint_row="$(printf '%s\n' "$mcp_empty" | grep -nF "$mcp_hint" | head -1 | cut -d: -f1)"
[ -n "$mcp_hint_row" ] || fail "the closing hint is missing from the empty list"
# The cell just past the hint: the two-column inset plus the hint's 50 columns.
expect_eq "$mcp_cursor" "52 $((${mcp_hint_row:-1} - 1))" "the cursor rests elsewhere than just past the closing hint (row ${mcp_hint_row:-none})"
# …and never on the bottom rule, the old far corner.
mcp_rule_row="$(printf '%s\n' "$mcp_empty" | grep -nE '^(─)+$' | tail -1 | cut -d: -f1)"
expect_ne "${mcp_cursor#* }" "$((${mcp_rule_row:-0} - 1))" "the cursor rests on the bottom rule"
tmux send-keys -t "$S80B" Escape
sleep 0.5
mcp_closed="$(tmux capture-pane -t "$S80B" -p)"
dump "closed back to the composer" "$mcp_closed"
expect_lacks "$mcp_closed" -F "Manage MCP servers" "Esc did not close the /mcp manager"
expect_has "$mcp_closed" -F "dummy_model_name" "the session footer did not come back after closing /mcp"
tmux kill-session -t "$S80B" 2>/dev/null
rm -rf "$MCP_CFG"
