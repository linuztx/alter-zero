#!/usr/bin/env bash
# Phase 88 — the PROTOCOL REVISION IS THE SERVER'S OWN, re-derived at every launch

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# the PROTOCOL REVISION IS THE SERVER'S OWN, re-derived at
# every launch (docs/mcp.md). The reported bug: a DUAL-ERA server — one that
# serves the modern 2026-07-28 `server/discover` *and* answers the legacy
# `initialize` handshake, which is what every real dual-era server does —
# read `2025-11-25` on the /mcp detail page forever, because a remembered
# `legacy` verdict skipped the probe and the handshake it went to instead
# succeeded, so the wrong guess never failed and never corrected itself.
# The era cache is retired: this launches against a dual-era scripted server
# with a stale `mcp-era.json` sitting in the config dir claiming legacy, and
# the detail page must read the server's live revision — and the retired
# cache file must be swept away rather than left to mislead.
S88="${S}_mcpera"
MCP88_CFG="$(mktemp -d)"
MCP88_DIR="$(mktemp -d)"
cat >"$MCP88_DIR/server.sh" <<'MCPDUAL'
#!/bin/sh
# A DUAL-ERA server: it answers whichever era the client opens with.
while IFS= read -r line; do
	id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
	case "$line" in
	*'"method":"server/discover"'*)
		printf '{"jsonrpc":"2.0","id":%s,"result":{"resultType":"complete","supportedVersions":["2026-07-28"],"capabilities":{"tools":{}},"ttlMs":60000,"cacheScope":"public","_meta":{"io.modelcontextprotocol/serverInfo":{"name":"dual","version":"1.0"}}}}\n' "$id" ;;
	*'"method":"initialize"'*)
		printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2025-11-25","capabilities":{"tools":{}},"serverInfo":{"name":"dual","version":"1.0"}}}\n' "$id" ;;
	*'"method":"tools/list"'*)
		printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"echo_text","description":"Echo the text back.","inputSchema":{"type":"object","properties":{}}}]}}\n' "$id" ;;
	esac
done
MCPDUAL
chmod +x "$MCP88_DIR/server.sh"
cat >"$MCP88_CFG/mcp.json" <<MCP88JSON
{"mcpServers": {"dual": {"type": "stdio", "command": "sh", "args": ["$MCP88_DIR/server.sh"]}}}
MCP88JSON
# The stale verdict a previous build would have left behind, keyed exactly
# as it keyed it (the server's command line).
cat >"$MCP88_CFG/mcp-era.json" <<MCP88ERA
{"servers": {"sh $MCP88_DIR/server.sh": {"era": "legacy", "version": "2025-11-25"}}}
MCP88ERA
APP_MCP88="env ALTER_ZERO_TELEMETRY=0 ALTER_ZERO_PROJECT_CONFIG=0 ALTER_ZERO_CONFIG_DIR=$MCP88_CFG ALTER_ZERO_CHECKPOINTS=0 ALTER_ZERO_HISTORY_FILE=/dev/null ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN"
tmux new-session -d -s "$S88" -x 100 -y 36 "$APP_MCP88"
sleep 1.2
tmux send-keys -t "$S88" -l "/mcp"
sleep 0.4
tmux send-keys -t "$S88" Enter
sleep 1.2
tmux send-keys -t "$S88" Enter
sleep 0.6
mcp88_detail="$(tmux capture-pane -t "$S88" -p)"
echo "==== Phase 88: the dual-era server's detail page ===="
printf '%s\n' "$mcp88_detail"
for expect in "Dual MCP Server" "✔ connected" "Protocol:" "2026-07-28"; do
	expect_has "$mcp88_detail" -F "$expect" "the detail page is missing '$expect'"
done
expect_lacks "$mcp88_detail" -F "2025-11-25" "the stale cached revision is on the detail page"
if [ -e "$MCP88_CFG/mcp-era.json" ]; then
	fail "the retired era cache survived the launch"
fi
tmux kill-session -t "$S88" 2>/dev/null
rm -rf "$MCP88_CFG" "$MCP88_DIR"
