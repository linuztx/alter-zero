#!/usr/bin/env bash
# Phase 152 — the /settings MCP row stops every server and brings them back

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# The /settings **MCP** row (docs/tools/mcp.md, docs/state/settings.md): the session-wide
# switch over the servers /mcp turns on and off one by one. Driven against a
# SCRIPTED modern stdio server (Phase 80's fixture) that drops a marker as it
# starts and another once its stdin closes, so the phase sees the process
# itself, not only the rows:
#   1. on by default — the server connects and its instructions ride Ctrl+D;
#   2. cycled off — a `MCP: false` toast, the server STOPPED (its stdin
#      closed), Ctrl+D without its instructions, /mcp open with the red
#      off-note over a `◯ off` row whose page repeats the note and whose
#      only action is its own Disable, and
#      `"mcp": false` in this directory's settings.json entry;
#   3. relaunched — still off, and nothing launched at startup;
#   4. cycled back on — the server connects again;
#   5. ALTER_ZERO_MCP=0 — seeds the row off for the run, starts nothing, and
#      saves nothing;
#   6. a project server new to the startup review, MCP off at launch — the
#      review does not ask (nothing could run), /mcp lists it waiting, and
#      turning MCP on asks then, the Hooks row's rule (docs/state/config-review.md).
S152="${S}_mcpsetting"
MCP_DIR="$(mktemp -d)"
cat >"$MCP_DIR/server.sh" <<MCPSRV
#!/bin/sh
: > "$MCP_DIR/started.\$\$"
printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"resultType":"complete","supportedVersions":["2026-07-28"],"capabilities":{"tools":{}},"_meta":{"io.modelcontextprotocol/serverInfo":{"name":"fixture","version":"1.0"}},"instructions":"Echo only short text."}}'
printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"resultType":"complete","tools":[{"name":"echo_text","description":"Echo the text back.","inputSchema":{"type":"object","properties":{"text":{"type":"string"}},"required":["text"]}}]}}'
while IFS= read -r line; do :; done
: > "$MCP_DIR/stopped.\$\$"
MCPSRV
chmod +x "$MCP_DIR/server.sh"
cat >"$SMOKE_CFG/mcp.json" <<MCPJSON
{"mcpServers": {"fixture": {"type": "stdio", "command": "sh", "args": ["$MCP_DIR/server.sh"]}}}
MCPJSON
started() { find "$MCP_DIR" -name 'started.*' | wc -l | tr -d ' '; }
stopped() { [ -n "$(find "$MCP_DIR" -name 'stopped.*')" ]; }

# Open /settings narrowed to the MCP row, cycle it with Space and read the
# toast; the menu stays open over the row.
cycle_mcp_row() {
	local sess="$1" want="$2"
	submit "$sess" "/settings"
	wait_for 5 "$sess" -F "Type to search" || fail "/settings did not open"
	type_text "$sess" "mcp"
	wait_for 5 "$sess" -F "(1/1)" || fail "typing 'mcp' did not narrow /settings to its row"
	keys "$sess" Space
	wait_for 5 "$sess" -F "MCP: $want" || fail "no 'MCP: $want' toast after Space"
}
close_settings() {
	keys "$1" Escape
	sleep 0.2
	keys "$1" Escape
	wait_for 5 "$1" -E '^❯' || fail "Esc did not close /settings"
}
open_mcp() {
	submit "$1" "/mcp"
	wait_for 5 "$1" -F "Manage MCP servers" || fail "/mcp did not open"
}
close_mcp() {
	keys "$1" Escape
	wait_for 5 "$1" -E '^❯' || fail "Esc did not close /mcp"
}

# 1. On by default: the startup connect.
launch "$S152" 100 36
open_mcp "$S152"
on_list="$(wait_pane 15 "$S152" -F "fixture · ✔ connected · 1 tool")"
dump "/mcp with MCP on" "$on_list"
expect_has "$on_list" -F "fixture · ✔ connected · 1 tool" "the server did not connect with MCP on by default"
expect_lacks "$on_list" -F "MCP is off for this session" "the off-note shows while MCP is on"
close_mcp "$S152"
expect_eq "$(started)" "1" "the server was not launched exactly once at startup"
keys "$S152" C-d
on_context="$(wait_pane 5 "$S152" -S -200 -- -F "Echo only short text.")"
expect_has "$on_context" -F "Echo only short text." "the connected server's instructions are missing from Ctrl+D"
keys "$S152" q
wait_for 5 "$S152" -E '^❯' || fail "q did not close Ctrl+D"

# 2. Cycled off.
cycle_mcp_row "$S152" "false"
off_row="$(pane "$S152")"
dump "/settings after Space on the MCP row" "$off_row"
expect_has "$off_row" -E "MCP +false" "the MCP row did not read false after Space"
poll 5 stopped || fail "turning MCP off did not stop the server (its stdin never closed)"
close_settings "$S152"
keys "$S152" C-d
sleep 0.6
off_context="$(pane "$S152" -S -200)"
dump "Ctrl+D with MCP off" "$off_context"
expect_lacks "$off_context" -F "Echo only short text." "the server's instructions still ride the context with MCP off"
keys "$S152" q
wait_for 5 "$S152" -E '^❯' || fail "q did not close Ctrl+D"
open_mcp "$S152"
off_list="$(wait_pane 5 "$S152" -F "fixture · ◯ off")"
dump "/mcp with MCP off" "$off_list"
for expect in "MCP is off for this session — turn it on in /settings" "fixture · ◯ off"; do
	expect_has "$off_list" -F "$expect" "/mcp with MCP off is missing '$expect'"
done
keys "$S152" Enter
off_detail="$(wait_pane 5 "$S152" -F "Fixture MCP Server")"
dump "the server's page with MCP off" "$off_detail"
expect_has "$off_detail" -F "◯ off" "the server page does not say the server is off"
expect_has "$off_detail" -F "1. Disable" "the server page does not offer the server's own switch"
expect_has "$off_detail" -F "MCP is off for this session — turn it on in /settings" "the off server's page does not say why it is off"
expect_lacks "$off_detail" -F "Reconnect" "the server page offers a Reconnect while MCP is off"
keys "$S152" Escape
sleep 0.2
close_mcp "$S152"
wait_file 5 "$SMOKE_CFG/settings.json" -E '"mcp": *false' ||
	fail "settings.json does not record MCP off for this directory: $(cat "$SMOKE_CFG/settings.json" 2>/dev/null)"
submit "$S152" "/quit"
wait_gone 10 "$S152" || fail "/quit did not exit"
launched="$(started)"

# 3. Relaunched in the same directory: still off, nothing started.
launch "$S152" 100 36
open_mcp "$S152"
relaunch_list="$(wait_pane 5 "$S152" -F "fixture · ◯ off")"
dump "/mcp after a relaunch" "$relaunch_list"
expect_has "$relaunch_list" -F "MCP is off for this session" "a relaunch forgot this directory's MCP off"
sleep 1
expect_eq "$(started)" "$launched" "a session with MCP off launched the server at startup"
close_mcp "$S152"

# 4. Cycled back on: the server connects again.
cycle_mcp_row "$S152" "true"
close_settings "$S152"
open_mcp "$S152"
back_list="$(wait_pane 15 "$S152" -F "fixture · ✔ connected · 1 tool")"
dump "/mcp after MCP was turned back on" "$back_list"
expect_has "$back_list" -F "fixture · ✔ connected · 1 tool" "turning MCP back on did not reconnect the server"
expect_lacks "$back_list" -F "MCP is off for this session" "the off-note outlived MCP being turned back on"
expect_eq "$(started)" "$((launched + 1))" "turning MCP back on did not launch the server once"
close_mcp "$S152"
submit "$S152" "/quit"
wait_gone 10 "$S152" || fail "/quit did not exit"
launched="$(started)"

# 5. ALTER_ZERO_MCP=0 seeds the row off for one run and saves nothing.
launch "$S152" 100 36 "env ALTER_ZERO_MCP=0 $CFG_ENV_NOHIST ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN"
submit "$S152" "/settings"
wait_for 5 "$S152" -F "Type to search" || fail "/settings did not open"
type_text "$S152" "mcp"
env_row="$(wait_pane 5 "$S152" -F "(1/1)")"
dump "/settings under ALTER_ZERO_MCP=0" "$env_row"
expect_has "$env_row" -E "MCP +false" "ALTER_ZERO_MCP=0 did not seed the MCP row off"
close_settings "$S152"
sleep 1
expect_eq "$(started)" "$launched" "ALTER_ZERO_MCP=0 still launched the server"
submit "$S152" "/quit"
wait_gone 10 "$S152" || fail "/quit did not exit"
expect_file_lacks "$SMOKE_CFG/settings.json" -E '"mcp": *false' "ALTER_ZERO_MCP=0 was saved into settings.json"

# 6. A project server waiting for the review, with MCP off at launch.
PROJ="$(mktemp -d)"
PROJ_DIR="$(mktemp -d)"
cat >"$PROJ_DIR/server.sh" <<PRJSRV
#!/bin/sh
: > "$PROJ_DIR/started.\$\$"
printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"resultType":"complete","supportedVersions":["2026-07-28"],"capabilities":{"tools":{}},"_meta":{"io.modelcontextprotocol/serverInfo":{"name":"projsrv","version":"1.0"}}}}'
printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"resultType":"complete","tools":[{"name":"echo_text","description":"Echo the text back.","inputSchema":{"type":"object","properties":{"text":{"type":"string"}},"required":["text"]}}]}}'
while IFS= read -r line; do :; done
PRJSRV
chmod +x "$PROJ_DIR/server.sh"
cat >"$PROJ/.mcp.json" <<PRJJSON
{"mcpServers": {"projsrv": {"type": "stdio", "command": "sh", "args": ["$PROJ_DIR/server.sh"]}}}
PRJJSON
project_started() { find "$PROJ_DIR" -name 'started.*' | wc -l | tr -d ' '; }
PROJ_APP="env ALTER_ZERO_MCP=0 ${CFG_ENV_NOHIST/ALTER_ZERO_PROJECT_CONFIG=0/ALTER_ZERO_PROJECT_CONFIG=1} ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN_ABS"
launch -c "$PROJ" "$S152" 100 40 "$PROJ_APP"
first="$(pane "$S152")"
dump "the first frame with MCP off over a project server" "$first"
expect_lacks "$first" -F "Select any you wish to enable." "the startup review asked about a server with MCP off"
expect_has "$first" -F "dummy_model_name" "the composer did not come up with MCP off"
open_mcp "$S152"
waiting="$(wait_pane 5 "$S152" -F "projsrv · ⚠ awaiting review")"
dump "/mcp over the waiting project server" "$waiting"
expect_has "$waiting" -F "projsrv · ⚠ awaiting review" "the project server is not listed waiting for the review"
expect_has "$waiting" -F "MCP is off for this session" "/mcp did not say MCP is off"
close_mcp "$S152"
cycle_mcp_row "$S152" "true"
review="$(wait_pane 5 "$S152" -F "Select any you wish to enable.")" ||
	fail "turning MCP on did not ask about the server waiting for the review"
dump "the review MCP asked for" "$review"
expect_has "$review" -F "projsrv · stdio" "the review does not list the waiting server"
expect_eq "$(project_started)" "0" "the project server ran before the review answered"
keys "$S152" Enter
wait_for 5 "$S152" -F "Type to search" || fail "the review did not hand back to /settings"
close_settings "$S152"
open_mcp "$S152"
enabled="$(wait_pane 15 "$S152" -F "projsrv · ✔ connected · 1 tool")"
dump "/mcp after the review enabled the server" "$enabled"
expect_has "$enabled" -F "projsrv · ✔ connected · 1 tool" "the server the review enabled did not connect"
close_mcp "$S152"
