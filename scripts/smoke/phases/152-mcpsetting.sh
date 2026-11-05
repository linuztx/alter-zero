#!/usr/bin/env bash
# Phase 152 — the /settings MCP row turns every server off and on

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# The /settings MCP row (docs/mcp.md *The /settings MCP row*): the whole MCP
# feature switched off and back on from the menu, while each server's own
# on/off in /mcp stays where it was. Against Phase 80's scripted (modern)
# stdio server — here writing its pid on every launch — it proves:
#   - on by default: the server connects and its instructions ride the
#     context;
#   - turning the row off stops the server's process, lists it `◯ off` in
#     /mcp under the red note (its page offering only Disable), and takes its
#     instructions out of the context;
#   - the change persists as `"mcp": false` for the directory, and mcp.json
#     — where a server's own on/off lives — is not touched;
#   - a relaunch in that directory launches nothing;
#   - turning the row back on connects the server again and saves the
#     default by leaving the key out;
#   - ALTER_ZERO_MCP=0 seeds the row off for one run, never saved;
#   - a project's new server is not asked about while MCP is off, and turning
#     the row on opens the review's servers page there and then.
S152="${S}_mcprow"
WORK="$(work_dir mcprow)"
SRV="$SMOKE_TMP/srv152"
mkdir -p "$SRV"
cat >"$SRV/server.sh" <<'MCPSRV'
#!/bin/sh
echo "$$" >>"$(dirname "$0")/launches"
cat > /dev/null &
printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"resultType":"complete","supportedVersions":["2026-07-28"],"capabilities":{"tools":{}},"ttlMs":60000,"cacheScope":"public","_meta":{"io.modelcontextprotocol/serverInfo":{"name":"fixture","version":"1.0"}},"instructions":"Echo only short text."}}'
printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"resultType":"complete","tools":[{"name":"echo_text","description":"Echo the text back.","inputSchema":{"type":"object","properties":{"text":{"type":"string","description":"What to echo."}},"required":["text"]}}],"ttlMs":60000,"cacheScope":"public"}}'
sleep 60
MCPSRV
cat >"$SMOKE_CFG/mcp.json" <<MCPJSON
{"mcpServers": {"fixture": {"type": "stdio", "command": "sh", "args": ["$SRV/server.sh"]}}}
MCPJSON
cp "$SMOKE_CFG/mcp.json" "$SRV/mcp.json.before"
# Every fixture this phase launched goes with it, whatever a failed check
# left running.
smoke_on_exit 'while read -r p; do kill "$p"; done <"$SRV/launches"'

# How many times the server was launched, and whether a pid still runs.
launches() {
	if [ -f "$SRV/launches" ]; then wc -l <"$SRV/launches" | tr -d ' '; else echo 0; fi
}
gone() { ! kill -0 "$1" 2>/dev/null; }
# open_menu SESSION COMMAND — run a slash command from the composer.
open_menu() {
	type_text "$1" "$2"
	sleep 0.3
	keys "$1" Enter
	sleep 0.5
}
# cycle_mcp SESSION — /settings, search the row, Enter, close back out.
cycle_mcp() {
	open_menu "$1" "/settings"
	type_text "$1" "mcp"
	sleep 0.4
	keys "$1" Enter
	sleep 0.5
}
close_menu() {
	keys "$1" Escape
	sleep 0.3
	keys "$1" Escape
	sleep 0.5
}

# ---- on by default: connected, its instructions in the context ----
launch -c "$WORK" "$S152" 100 36 "$APP_ABS"
open_menu "$S152" "/mcp"
if ! wait_for 15 "$S152" -F "fixture · ✔ connected"; then
	dump "the server list" "$(pane "$S152")"
	fail "the server never connected with the MCP row on"
fi
keys "$S152" Escape
sleep 0.4
pid="$(head -n 1 "$SRV/launches" 2>/dev/null)"
if [ -z "$pid" ] || gone "$pid"; then
	fail "the connected server's process is not running"
fi
keys "$S152" C-d
sleep 0.6
context_on="$(pane "$S152" -S -200)"
expect_has "$context_on" -F "# MCP Server Instructions" "the connected server's instructions are not in the context"
keys "$S152" q
sleep 0.4

# ---- turned off: the process stops, /mcp says so, the context forgets ----
open_menu "$S152" "/settings"
type_text "$S152" "mcp"
sleep 0.4
searched="$(pane "$S152")"
dump "the MCP row, searched for" "$searched"
expect_has "$searched" -E "→ MCP +true" "the MCP row is missing or not on by default"
expect_has "$searched" -F "(1/1)" "the search did not narrow to the MCP row alone"
expect_has "$searched" -F "Connect the MCP servers" "the MCP row's description is missing"
keys "$S152" Enter
if ! wait_for 5 "$S152" -E "→ MCP +false"; then
	fail "Enter did not turn the MCP row off"
fi
close_menu "$S152"
expect_has "$(pane "$S152")" -F "MCP: false" "turning MCP off was not confirmed with a toast"
if ! poll 10 gone "$pid"; then
	fail "the server's process outlived turning MCP off"
fi
open_menu "$S152" "/mcp"
off_list="$(pane "$S152")"
dump "/mcp with MCP off" "$off_list"
expect_has "$off_list" -F "MCP is off for this session — turn it on in /settings" "/mcp does not say MCP is off"
expect_has "$off_list" -F "fixture · ◯ off" "the server does not read '◯ off'"
keys "$S152" Enter
sleep 0.5
off_page="$(pane "$S152")"
dump "an off server's page" "$off_page"
expect_has "$off_page" -F "1. Disable" "an off server's page does not offer Disable"
expect_lacks "$off_page" -F "Reconnect" "an off server's page offers Reconnect"
close_menu "$S152"
keys "$S152" C-d
sleep 0.6
context_off="$(pane "$S152" -S -200)"
expect_lacks "$context_off" -F "# MCP Server Instructions" "the context still carries the stopped server's instructions"
keys "$S152" q
sleep 0.4
open_menu "$S152" "/quit"
tmux kill-session -t "$S152" 2>/dev/null
expect_file_has "$SMOKE_CFG/settings.json" -F '"mcp": false' "settings.json does not record MCP off for the directory"
cmp -s "$SMOKE_CFG/mcp.json" "$SRV/mcp.json.before" ||
	fail "turning MCP off rewrote mcp.json — a server's own on/off moved"

# ---- a relaunch in that directory launches nothing; on connects again ----
before="$(launches)"
S152B="${S152}_again"
launch -c "$WORK" "$S152B" 100 36 "$APP_ABS"
open_menu "$S152B" "/mcp"
sleep 1
relaunched="$(pane "$S152B")"
dump "relaunched where MCP is off" "$relaunched"
expect_has "$relaunched" -F "fixture · ◯ off" "a relaunch did not start with MCP off"
expect_eq "$(launches)" "$before" "a relaunch launched the server with MCP off"
keys "$S152B" Escape
sleep 0.4
cycle_mcp "$S152B"
if ! wait_for 5 "$S152B" -E "→ MCP +true"; then
	fail "Enter did not turn the MCP row back on"
fi
close_menu "$S152B"
open_menu "$S152B" "/mcp"
if ! wait_for 15 "$S152B" -F "fixture · ✔ connected"; then
	dump "the server list" "$(pane "$S152B")"
	fail "turning MCP back on did not connect the server"
fi
expect_lacks "$(pane "$S152B")" -F "MCP is off" "the off note outlived turning MCP on"
keys "$S152B" Escape
sleep 0.4
open_menu "$S152B" "/quit"
tmux kill-session -t "$S152B" 2>/dev/null
expect_file_lacks "$SMOKE_CFG/settings.json" -F '"mcp"' "turning MCP back on did not save the default by leaving the key out"

# ---- ALTER_ZERO_MCP=0 seeds the row off for one run, never saved ----
before="$(launches)"
S152C="${S152}_env"
launch -c "$WORK" "$S152C" 100 36 "env ALTER_ZERO_MCP=0 $APP_ABS"
open_menu "$S152C" "/mcp"
sleep 1
seeded="$(pane "$S152C")"
dump "ALTER_ZERO_MCP=0" "$seeded"
expect_has "$seeded" -F "fixture · ◯ off" "ALTER_ZERO_MCP=0 did not seed the row off"
expect_eq "$(launches)" "$before" "ALTER_ZERO_MCP=0 launched the server"
keys "$S152C" Escape
sleep 0.4
open_menu "$S152C" "/quit"
tmux kill-session -t "$S152C" 2>/dev/null
expect_file_lacks "$SMOKE_CFG/settings.json" -F '"mcp"' "ALTER_ZERO_MCP=0 was saved into settings.json"

# ---- a project's servers: nothing asked while MCP is off, asked once on ----
# The startup review asks only about what would run (docs/config-review.md),
# so with the row off a project's new server waits unasked and unlaunched —
# and turning the row on opens the review's servers page there and then.
PROJ="$(work_dir mcpproj)"
cp "$SRV/server.sh" "$PROJ/server.sh"
cat >"$PROJ/.mcp.json" <<MCPJSON
{"mcpServers": {"projsrv": {"type": "stdio", "command": "sh", "args": ["$PROJ/server.sh"]}}}
MCPJSON
S152D="${S152}_review"
launch -c "$PROJ" "$S152D" 100 40 \
	"env ALTER_ZERO_MCP=0 ${APP_ABS/ALTER_ZERO_PROJECT_CONFIG=0/ALTER_ZERO_PROJECT_CONFIG=1}"
expect_lacks "$(pane "$S152D")" -F "Select any you wish to enable." "the review asked about a project server with MCP off"
cycle_mcp "$S152D"
asked="$(wait_pane 5 "$S152D" -F "Select any you wish to enable.")" ||
	fail "turning MCP on did not ask about the project's new server"
dump "the servers page, asked once MCP is on" "$asked"
expect_has "$asked" -F "projsrv" "the servers page does not name the project's server"
# Esc on the servers page disables them all — then the settings menu is
# back, its search still typed.
keys "$S152D" Escape
sleep 0.5
close_menu "$S152D"
open_menu "$S152D" "/mcp"
declined="$(pane "$S152D")"
dump "/mcp after declining" "$declined"
expect_has "$declined" -F "projsrv · ◯ disabled" "the declined project server is not disabled"
expect_lacks "$declined" -F "MCP is off" "the off note outlived turning MCP on"
[ -f "$PROJ/launches" ] && fail "the project's server launched before anyone enabled it"
keys "$S152D" Escape
sleep 0.4
open_menu "$S152D" "/quit"
tmux kill-session -t "$S152D" 2>/dev/null
