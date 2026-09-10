#!/usr/bin/env bash
# Phase 84 — the mcp CLI SUBCOMMAND

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# the mcp CLI SUBCOMMAND (docs/mcp-cli.md). The install path
# end to end, before any TUI: `mcp add fixture -- sh server.sh` writes the
# scripted stdio fixture into a temp USER mcp.json (exit 0, the Added line
# + the File: path), `mcp list` reports it, a duplicate add exits 1 naming
# the mcp remove escape hatch, a grammar error exits 2 with the mcp usage
# as its trailer — then the TUI boots against that SAME file and /mcp
# resolves the row '✔ connected · 1 tool': the file the CLI writes is the
# file the session reads.
S84="${S}_mcpcli"
MCP84_CFG="$(mktemp -d)"
MCP84_DIR="$(mktemp -d)"
cat >"$MCP84_DIR/server.sh" <<'MCPSRV84'
#!/bin/sh
cat > /dev/null &
printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"resultType":"complete","supportedVersions":["2026-07-28"],"capabilities":{"tools":{}},"ttlMs":60000,"cacheScope":"public","_meta":{"io.modelcontextprotocol/serverInfo":{"name":"fixture","version":"1.0"}}}}'
printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"resultType":"complete","tools":[{"name":"echo_text","description":"Echo the text back.","inputSchema":{"type":"object","properties":{"text":{"type":"string","description":"What to echo."}},"required":["text"]}}],"ttlMs":60000,"cacheScope":"public"}}'
sleep 60
MCPSRV84
chmod +x "$MCP84_DIR/server.sh"
mcp84_add="$(env ALTER_ZERO_MCP_FILE="$MCP84_CFG/mcp.json" "$BIN" mcp add fixture -- sh "$MCP84_DIR/server.sh" 2>&1)"
mcp84_add_exit=$?
echo "==== Phase 84: mcp add ===="
printf '%s\n' "$mcp84_add"
if [ "$mcp84_add_exit" -ne 0 ]; then
	fail "mcp add exited $mcp84_add_exit"
fi
for expect in 'Added stdio MCP server "fixture"' "File: $MCP84_CFG/mcp.json"; do
	expect_has "$mcp84_add" -F "$expect" "mcp add output is missing '$expect'"
done
mcp84_list="$(env ALTER_ZERO_MCP_FILE="$MCP84_CFG/mcp.json" "$BIN" mcp list 2>&1)"
echo "==== Phase 84: mcp list ===="
printf '%s\n' "$mcp84_list"
expect_has "$mcp84_list" -F "fixture: sh $MCP84_DIR/server.sh (stdio)" "mcp list does not report the installed server"
mcp84_dup="$(env ALTER_ZERO_MCP_FILE="$MCP84_CFG/mcp.json" "$BIN" mcp add fixture --url https://dup 2>&1)"
mcp84_dup_exit=$?
if [ "$mcp84_dup_exit" -ne 1 ]; then
	fail "a duplicate add exited $mcp84_dup_exit (want 1)"
fi
for expect in 'MCP server "fixture" already exists' "mcp remove fixture"; do
	expect_has "$mcp84_dup" -F "$expect" "the duplicate-add error is missing '$expect'"
done
mcp84_bad="$(env ALTER_ZERO_MCP_FILE="$MCP84_CFG/mcp.json" "$BIN" mcp add lonely 2>&1)"
mcp84_bad_exit=$?
if [ "$mcp84_bad_exit" -ne 2 ]; then
	fail "a grammar error exited $mcp84_bad_exit (want 2)"
fi
# The clap-shaped trailer (docs/cli.md): `error: {message}`, the mcp `Usage:`
# block, `For more information, try '--help'.` — never the whole page.
for expect in "error: mcp add needs a --url or a command" \
	"Usage: alter-zero mcp add <name>" \
	"For more information, try '--help'."; do
	expect_has "$mcp84_bad" -F "$expect" "the grammar error is missing '$expect'"
done
expect_lacks "$mcp84_bad" -F "alter-zero mcp — manage MCP servers" "a grammar error dumped the whole help page instead of the usage trailer"
APP_MCP84="env ALTER_ZERO_TELEMETRY=0 ALTER_ZERO_PROJECT_CONFIG=0 ALTER_ZERO_CONFIG_DIR=$MCP84_CFG ALTER_ZERO_CHECKPOINTS=0 ALTER_ZERO_HISTORY_FILE=/dev/null ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN"
tmux new-session -d -s "$S84" -x 100 -y 36 "$APP_MCP84"
sleep 0.8
tmux send-keys -t "$S84" -l "/mcp"
sleep 0.4
tmux send-keys -t "$S84" Enter
wait_for 15 "$S84" -F "✔ connected"
mcp84_tui="$(tmux capture-pane -t "$S84" -p)"
echo "==== Phase 84: the TUI reads the CLI-written file ===="
printf '%s\n' "$mcp84_tui"
expect_has "$mcp84_tui" -F "fixture · ✔ connected · 1 tool" "/mcp does not show the CLI-installed server connected"
tmux kill-session -t "$S84" 2>/dev/null
rm -rf "$MCP84_CFG" "$MCP84_DIR"
