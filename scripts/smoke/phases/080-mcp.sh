#!/usr/bin/env bash
# Phase 80 — the /mcp MANAGER

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# the /mcp MANAGER (docs/mcp.md). Claude Code's MCP surface
# driven offline end to end against a SCRIPTED stdio server (a sh script
# answering the deterministic ids of a MODERN 2026-07-28 server: 1 = the
# server/discover probe, 2 = tools/list — no initialize anywhere): the
# palette lists /mcp, the manager opens on the grouped server list, the
# startup connect resolves the row to '✔ connected · 1 tool', Enter walks
# list → server detail (facts + actions, the negotiated Protocol row
# included) → tools → the tool detail naming the wire name the model calls,
# and Esc walks all the way back out with the composer restored.
S80="${S}_mcp"
MCP_CFG="$(mktemp -d)"
MCP_DIR="$(mktemp -d)"
cat >"$MCP_DIR/server.sh" <<'MCPSRV'
#!/bin/sh
cat > /dev/null &
printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"resultType":"complete","supportedVersions":["2026-07-28"],"capabilities":{"tools":{}},"ttlMs":60000,"cacheScope":"public","_meta":{"io.modelcontextprotocol/serverInfo":{"name":"fixture","version":"1.0"}}}}'
printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"resultType":"complete","tools":[{"name":"echo_text","description":"Echo the text back.","inputSchema":{"type":"object","properties":{"text":{"type":"string","description":"What to echo."}},"required":["text"]}}],"ttlMs":60000,"cacheScope":"public"}}'
sleep 60
MCPSRV
chmod +x "$MCP_DIR/server.sh"
cat >"$MCP_CFG/mcp.json" <<MCPJSON
{"mcpServers": {"fixture": {"type": "stdio", "command": "sh", "args": ["$MCP_DIR/server.sh"]}}}
MCPJSON
APP_MCP="env ALTER_ZERO_TELEMETRY=0 ALTER_ZERO_PROJECT_CONFIG=0 ALTER_ZERO_CONFIG_DIR=$MCP_CFG ALTER_ZERO_CHECKPOINTS=0 ALTER_ZERO_HISTORY_FILE=/dev/null ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN"
tmux new-session -d -s "$S80" -x 100 -y 36 "$APP_MCP"
sleep 0.8
tmux send-keys -t "$S80" -l "/mcp"
sleep 0.4
mcp_palette="$(tmux capture-pane -t "$S80" -p)"
echo "==== Phase 80: the palette filtered to /mcp ===="
printf '%s\n' "$mcp_palette"
expect_has "$mcp_palette" -F "Manage MCP servers" "/mcp is missing from the slash-command palette"
tmux send-keys -t "$S80" Enter
# The startup connect runs on a worker thread; poll for the resolved row.
wait_for 15 "$S80" -F "✔ connected"
mcp_list="$(tmux capture-pane -t "$S80" -p)"
echo "==== Phase 80: the server list ===="
printf '%s\n' "$mcp_list"
for expect in "Manage MCP servers" "1 server" "User MCPs" \
	"fixture · ✔ connected · 1 tool" \
	"↑/↓ to navigate · Enter to confirm · Esc to cancel"; do
	expect_has "$mcp_list" -F "$expect" "the server list is missing '$expect'"
done
tmux send-keys -t "$S80" Enter
sleep 0.5
mcp_detail="$(tmux capture-pane -t "$S80" -p)"
echo "==== Phase 80: the server detail ===="
printf '%s\n' "$mcp_detail"
for expect in "Fixture MCP Server" "Status:" "✔ connected" \
	"Protocol:" "2026-07-28" \
	"Command:" "server.sh" "Config location:" "Capabilities:" "tools" \
	"Tools:" "1 tool" \
	"1. View tools" "2. Reconnect" "3. Disable"; do
	expect_has "$mcp_detail" -F "$expect" "the server detail is missing '$expect'"
done
# The count belongs to the LIST row; the detail page has a `Tools:` row of
# its own, so saying it on the Status row too is a duplicate (docs/mcp.md).
expect_lacks "$mcp_detail" -F "· 1 tool" "the detail's Status row repeats the tool count"
# A stdio server has no auth story — and a modern one that never asked for
# credentials must not wear a '✘ not authenticated' row (the deepwiki bug).
expect_lacks "$mcp_detail" -F "Auth:" "the detail page shows an Auth row for a server with no auth story"
tmux send-keys -t "$S80" Enter
sleep 0.5
mcp_tools="$(tmux capture-pane -t "$S80" -p)"
echo "==== Phase 80: the tools list ===="
printf '%s\n' "$mcp_tools"
for expect in "Tools for fixture" "1 tool" "1. echo_text"; do
	expect_has "$mcp_tools" -F "$expect" "the tools list is missing '$expect'"
done
tmux send-keys -t "$S80" Enter
sleep 0.5
mcp_tool="$(tmux capture-pane -t "$S80" -p)"
echo "==== Phase 80: the tool detail ===="
printf '%s\n' "$mcp_tool"
for expect in "Tool name:" "echo_text" "Full name:" "mcp__fixture__echo_text" \
	"Description:" "Echo the text back." "Parameters:" \
	"text (required): string - What to echo." "Esc to go back"; do
	expect_has "$mcp_tool" -F "$expect" "the tool detail is missing '$expect'"
done
# Esc walks back out: tool → tools → server → list → closed (footer back).
tmux send-keys -t "$S80" Escape; sleep 0.3
tmux send-keys -t "$S80" Escape; sleep 0.3
tmux send-keys -t "$S80" Escape; sleep 0.3
tmux send-keys -t "$S80" Escape; sleep 0.5
mcp_closed="$(tmux capture-pane -t "$S80" -p)"
echo "==== Phase 80: closed back to the composer ===="
printf '%s\n' "$mcp_closed"
expect_lacks "$mcp_closed" -F "Manage MCP servers" "Esc did not close the /mcp manager"
expect_has "$mcp_closed" -F "dummy_model_name" "the session footer did not come back after closing /mcp"
tmux kill-session -t "$S80" 2>/dev/null
rm -rf "$MCP_CFG" "$MCP_DIR"
