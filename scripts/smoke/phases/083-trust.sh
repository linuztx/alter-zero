#!/usr/bin/env bash
# Phase 83 — the project-level .alter-zero config layer behind the /trust gate

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# the project-level .alter-zero config layer behind the /trust
# gate (docs/project-config.md). A temp project (no .git — the root falls
# back to the cwd) carries .alter-zero/hooks.json and .mcp.json (the Phase
# 80 scripted stdio fixture). First launch, the layer ON over its own fresh
# config home: the startup toast points at /trust, /mcp lists the project
# server '⚠ untrusted' (default-deny — never launched), /trust reviews the
# hook command and the server target VERBATIM over the approve option, and
# approving activates LIVE — the server connects with no restart and the
# /hooks browser shows the merged Stop hook. A relaunch on the same config
# home starts already-trusted: /trust reads 'Status: trusted' offering only
# the revoke, and the server connects unprompted.
S83="${S}_trust"
TR_CFG="$(mktemp -d)"
TR_WORK="$(mktemp -d)"
mkdir -p "$TR_WORK/.alter-zero"
cat >"$TR_WORK/server.sh" <<'TRSRV'
#!/bin/sh
cat > /dev/null &
printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"resultType":"complete","supportedVersions":["2026-07-28"],"capabilities":{"tools":{}},"ttlMs":60000,"cacheScope":"public","_meta":{"io.modelcontextprotocol/serverInfo":{"name":"projfix","version":"1.0"}}}}'
printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"resultType":"complete","tools":[{"name":"echo_text","description":"Echo the text back.","inputSchema":{"type":"object","properties":{"text":{"type":"string","description":"What to echo."}},"required":["text"]}}],"ttlMs":60000,"cacheScope":"public"}}'
sleep 60
TRSRV
chmod +x "$TR_WORK/server.sh"
cat >"$TR_WORK/.mcp.json" <<TRJSON
{"mcpServers": {"projfix": {"type": "stdio", "command": "sh", "args": ["$TR_WORK/server.sh"]}}}
TRJSON
cat >"$TR_WORK/.alter-zero/hooks.json" <<'TRHOOKS'
{"hooks": {"Stop": [{"hooks": [{"type": "command", "command": "./fmt.sh"}]}]}}
TRHOOKS
# The phase runs in a temp cwd (-c "$TR_WORK"), so the binary path must be
# absolute — the Phase 46 BIN_ABS rule; a relative $BIN would resolve inside
# the temp project and never launch.
TR_BIN="$(readlink -f "$BIN")"
APP_TR="env ALTER_ZERO_TELEMETRY=0 ALTER_ZERO_PROJECT_CONFIG=1 ALTER_ZERO_CONFIG_DIR=$TR_CFG ALTER_ZERO_CHECKPOINTS=0 ALTER_ZERO_HOOKS=1 ALTER_ZERO_HISTORY_FILE=/dev/null ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $TR_BIN"
tmux new-session -d -s "$S83" -x 100 -y 36 -c "$TR_WORK" "$APP_TR"
# The pending toast rides the first frames and self-clears — poll for it.
tr_toast="$(wait_pane 5 "$S83" -F "/trust to review")"
echo "==== Phase 83: the pending-config startup toast ===="
printf '%s\n' "$tr_toast"
expect_has "$tr_toast" -F "Project .alter-zero config found — /trust to review" "the pending project config raised no startup toast"
# Default-deny: /mcp lists the project server untrusted, never connected.
tmux send-keys -t "$S83" -l "/mcp"
sleep 0.4
tmux send-keys -t "$S83" Enter
sleep 0.6
tr_mcp="$(tmux capture-pane -t "$S83" -p)"
echo "==== Phase 83: /mcp holds the untrusted project server ===="
printf '%s\n' "$tr_mcp"
for expect in "Project MCPs" "projfix · ⚠ untrusted"; do
	expect_has "$tr_mcp" -F "$expect" "/mcp is missing '$expect' before approval"
done
tmux send-keys -t "$S83" Escape
sleep 0.4
# The /trust review names the root, both files, and what would run.
tmux send-keys -t "$S83" -l "/trust"
sleep 0.4
tmux send-keys -t "$S83" Enter
sleep 0.5
tr_review="$(tmux capture-pane -t "$S83" -p)"
echo "==== Phase 83: the /trust review ===="
printf '%s\n' "$tr_review"
for expect in "Project trust —" "Status: not trusted" \
	"Hooks — " "pending approval" "Stop: ./fmt.sh" \
	"MCP servers — " "projfix: sh $TR_WORK/server.sh" \
	"1. Trust this project's config"; do
	expect_has "$tr_review" -F "$expect" "the /trust review is missing '$expect'"
done
# Approve: Enter on option 1 records trust.json and activates live.
tmux send-keys -t "$S83" Enter
sleep 0.5
tr_after="$(tmux capture-pane -t "$S83" -p)"
echo "==== Phase 83: after the approval ===="
printf '%s\n' "$tr_after"
expect_has "$tr_after" -F "Trusted this project's config" "approving raised no confirmation toast"
# Live activation, no restart: the project server connects…
tmux send-keys -t "$S83" -l "/mcp"
sleep 0.4
tmux send-keys -t "$S83" Enter
wait_for 15 "$S83" -F "✔ connected"
tr_live="$(tmux capture-pane -t "$S83" -p)"
echo "==== Phase 83: the approved server connected live ===="
printf '%s\n' "$tr_live"
expect_has "$tr_live" -F "projfix · ✔ connected · 1 tool" "the approved project server did not connect live"
tmux send-keys -t "$S83" Escape
sleep 0.4
# …and the merged project hook shows in the /hooks browser.
tmux send-keys -t "$S83" -l "/hooks"
sleep 0.4
tmux send-keys -t "$S83" Enter
sleep 0.5
tr_hooks="$(tmux capture-pane -t "$S83" -p)"
echo "==== Phase 83: the merged project hook in /hooks ===="
printf '%s\n' "$tr_hooks"
expect_has "$tr_hooks" -F "1 hook configured" "/hooks does not count the merged project hook"
# Stop sits past the five-row event window — the digit jumps straight into
# its handler list, which names the project file's command.
tmux send-keys -t "$S83" -l "7"
sleep 0.5
tr_stop="$(tmux capture-pane -t "$S83" -p)"
echo "==== Phase 83: the Stop handler list ===="
printf '%s\n' "$tr_stop"
expect_has "$tr_stop" -F "./fmt.sh" "the merged Stop hook does not list ./fmt.sh"
tmux send-keys -t "$S83" Escape
sleep 0.3
tmux send-keys -t "$S83" Escape
sleep 0.3
tmux kill-session -t "$S83" 2>/dev/null
# Relaunch on the same config home: the trust persisted.
tmux new-session -d -s "$S83" -x 100 -y 36 -c "$TR_WORK" "$APP_TR"
sleep 0.8
tmux send-keys -t "$S83" -l "/trust"
sleep 0.4
tmux send-keys -t "$S83" Enter
sleep 0.5
tr_persist="$(tmux capture-pane -t "$S83" -p)"
echo "==== Phase 83: /trust after a relaunch ===="
printf '%s\n' "$tr_persist"
for expect in "Status: trusted" "1. Revoke trust"; do
	expect_has "$tr_persist" -F "$expect" "the relaunch lost the recorded trust ('$expect' missing)"
done
expect_lacks "$tr_persist" -F "Trust this project's config" "a trusted, unchanged project still offers the approval"
tmux send-keys -t "$S83" Escape
sleep 0.3
tmux send-keys -t "$S83" -l "/mcp"
sleep 0.4
tmux send-keys -t "$S83" Enter
wait_for 15 "$S83" -F "✔ connected"
tr_boot="$(tmux capture-pane -t "$S83" -p)"
echo "==== Phase 83: the trusted server connects unprompted at startup ===="
printf '%s\n' "$tr_boot"
expect_has "$tr_boot" -F "projfix · ✔ connected · 1 tool" "the trusted project server did not connect at the relaunch"
tmux kill-session -t "$S83" 2>/dev/null
# The home directory is never a project (docs/project-config.md): a cwd with
# no .git falls back to itself as the root, and launched in ~ that made
# {root}/.alter-zero the user's own config home — the layer rediscovered the
# user's files as pending "project config" and asked the user to trust
# themself (the reported bug). A fake HOME carrying user-level hooks must
# raise no pending toast, still load them as user hooks, and /trust explains.
TR_HOME="$(mktemp -d)"
mkdir -p "$TR_HOME/.alter-zero"
cat >"$TR_HOME/.alter-zero/hooks.json" <<'TRHOME'
{"hooks": {"Stop": [{"hooks": [{"type": "command", "command": "./fmt.sh"}]}]}}
TRHOME
tmux new-session -d -s "$S83" -x 100 -y 36 -c "$TR_HOME" \
	"env HOME=$TR_HOME ALTER_ZERO_TELEMETRY=0 ALTER_ZERO_PROJECT_CONFIG=1 ALTER_ZERO_CHECKPOINTS=0 ALTER_ZERO_HOOKS=1 ALTER_ZERO_HISTORY_FILE=/dev/null ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $TR_BIN"
sleep 1.5
tr_home_pane="$(tmux capture-pane -t "$S83" -p)"
echo "==== Phase 83: launched in the home directory ===="
printf '%s\n' "$tr_home_pane"
expect_lacks "$tr_home_pane" -F "/trust to review" "the user's own config home raised the project trust toast"
tmux send-keys -t "$S83" -l "/hooks"
sleep 0.4
tmux send-keys -t "$S83" Enter
sleep 0.5
if ! tmux capture-pane -t "$S83" -p | grep -qF "1 hook configured"; then
	fail "the home config no longer loads as user-level hooks"
fi
tmux send-keys -t "$S83" Escape
sleep 0.4
tmux send-keys -t "$S83" -l "/trust"
sleep 0.4
tmux send-keys -t "$S83" Enter
sleep 0.5
if ! tmux capture-pane -t "$S83" -p | grep -qF "The home directory is not a project"; then
	fail "/trust in the home directory does not explain itself"
fi
tmux kill-session -t "$S83" 2>/dev/null
rm -rf "$TR_CFG" "$TR_WORK" "$TR_HOME"
