#!/usr/bin/env bash
# Phase 162 — a plugin on in this directory, taken into the session, and what it costs when none is

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# The plugin runtime (docs/plugins.md *Loading into a session*), against a
# config home seeded by hand: a marketplace kept in a directory, so its
# plugin loads in place, and a `plugins.json` naming it. The plugin brings a
# skill, a command, an agent, a hook and a stdio MCP server. Asserted on
# screen: the startup review lists the hook's real command and the server
# under the plugin's name, and neither runs before it is answered; trusted,
# the hook fires and `/mcp` shows `plugin:hello:probe` connected; the `$`
# picker completes `hello:greet` and `hello:hi`, Ctrl+D's listing names them
# and the agent, `/skills` lists them; `ALTER_ZERO_PLUGINS=0` has none of
# it, nor a directory that turned the plugin off. Then the memory gate: an
# idle session over an empty config home, one over two recorded
# marketplaces with large catalogs and nothing installed, and one with
# `ALTER_ZERO_PLUGINS=0` stay within 64 kB of each other (`RssAnon`).
S162="${S}_plugrt"
PR_CFG="$SMOKE_TMP/prcfg"
PR_MARKET="$SMOKE_TMP/market"
PR_PLUGIN="$PR_MARKET/plugins/hello"
PR_WORK="$(work_dir prwork)"
PR_OFF="$(work_dir proff)"
PR_WORK="$(readlink -f "$PR_WORK")"
PR_OFF="$(readlink -f "$PR_OFF")"
mkdir -p "$PR_CFG" "$PR_MARKET/.claude-plugin" "$PR_PLUGIN/.claude-plugin" \
	"$PR_PLUGIN/skills/greet" "$PR_PLUGIN/commands" "$PR_PLUGIN/agents" "$PR_PLUGIN/hooks"
cat >"$PR_MARKET/.claude-plugin/marketplace.json" <<'JSON'
{ "name": "demo", "owner": { "name": "Demo" }, "plugins": [
  { "name": "hello", "source": "./plugins/hello", "description": "Greets people" } ] }
JSON
cat >"$PR_PLUGIN/.claude-plugin/plugin.json" <<'JSON'
{ "name": "hello", "version": "1.0.0" }
JSON
cat >"$PR_PLUGIN/skills/greet/SKILL.md" <<'SKILL'
---
name: greet
description: Greet the user in the language they ask for
---
Say hello.
SKILL
printf -- '---\ndescription: Say hi to everyone\n---\nSay hi.\n' >"$PR_PLUGIN/commands/hi.md"
printf -- '---\nname: reviewer\ndescription: Reviews greetings for warmth\ntools: Read, Grep\n---\nReview it.\n' >"$PR_PLUGIN/agents/reviewer.md"
# The hook: SessionStart, writing into the plugin's data directory through
# the variable it is exported under — its command named through the
# placeholder the review must show substituted.
cat >"$PR_PLUGIN/hooks/start.sh" <<'SH'
#!/bin/sh
cat > /dev/null
echo started >> "$CLAUDE_PLUGIN_DATA/started"
SH
# And UserPromptSubmit, whose script a reinstall from the command line
# changes under the same command line.
printf '#!/bin/sh\ncat > /dev/null\necho one >> "$CLAUDE_PLUGIN_DATA/prompted"\n' >"$PR_PLUGIN/hooks/prompt.sh"
cat >"$PR_PLUGIN/hooks/hooks.json" <<'JSON'
{ "hooks": {
  "SessionStart": [ { "hooks": [ { "type": "command", "command": "sh \"${CLAUDE_PLUGIN_ROOT}/hooks/start.sh\"" } ] } ],
  "UserPromptSubmit": [ { "hooks": [ { "type": "command", "command": "sh \"${CLAUDE_PLUGIN_ROOT}/hooks/prompt.sh\"" } ] } ] } }
JSON
# A scripted stdio MCP server (Phase 83's).
cat >"$PR_PLUGIN/server.sh" <<'MCPSRV'
#!/bin/sh
cat > /dev/null &
printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"resultType":"complete","supportedVersions":["2026-07-28"],"capabilities":{"tools":{}},"ttlMs":60000,"cacheScope":"public","_meta":{"io.modelcontextprotocol/serverInfo":{"name":"probe","version":"1.0"}}}}'
printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"resultType":"complete","tools":[{"name":"echo_text","description":"Echo the text back.","inputSchema":{"type":"object","properties":{"text":{"type":"string"}},"required":["text"]}}],"ttlMs":60000,"cacheScope":"public"}}'
sleep 60
MCPSRV
cat >"$PR_PLUGIN/.mcp.json" <<'JSON'
{ "mcpServers": { "probe": { "command": "sh", "args": ["${CLAUDE_PLUGIN_ROOT}/server.sh"] } } }
JSON
cat >"$PR_CFG/plugins.json" <<JSON
{ "marketplaces": { "demo": { "source": "$PR_MARKET" } },
  "plugins": { "hello@demo": { "version": "1.0.0", "root": "$PR_PLUGIN" } },
  "projects": { "$PR_OFF": { "disabled": ["hello@demo"] } } }
JSON
PR_DATA="$PR_CFG/plugins/data/demo/hello"

# A stub provider refusing every request (Phase 133's): hooks run only on a
# real backend.
PR_PORT="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
python3 - "$PR_PORT" <<'PY' &
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer

class Stub(BaseHTTPRequestHandler):
    def do_GET(self):
        self.send(200, b'{"object":"list","data":[{"id":"stub-model"}]}')

    def do_POST(self):
        self.rfile.read(int(self.headers.get("content-length") or 0))
        self.send(400, b'{"error":{"message":"smoke162 refused"}}')

    def send(self, status, data):
        self.send_response(status)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, *_args):
        pass

HTTPServer(("127.0.0.1", int(sys.argv[1])), Stub).serve_forever()
PY
PR_STUB=$!
smoke_on_exit kill "$PR_STUB"
PR_PROVIDERS="$SMOKE_TMP/providers162.toml"
printf '[providers.stub]\nname = "Stub"\n\n[providers.stub.kwargs]\napi_base = "http://127.0.0.1:%s/v1"\n' "$PR_PORT" >"$PR_PROVIDERS"
PR_ENV="ALTER_ZERO_TELEMETRY=0 ALTER_ZERO_UPDATE_CHECK=0 ALTER_ZERO_TIPS=0 ALTER_ZERO_PROJECT_CONFIG=0 ALTER_ZERO_CONFIG_DIR=$PR_CFG ALTER_ZERO_CHECKPOINTS=0 ALTER_ZERO_HOOKS=1 ALTER_ZERO_HISTORY_FILE=/dev/null ALTER_ZERO_SKILLS_DIR=$SMOKE_SKILLS ALTER_ZERO_AGENTS_DIR=$SMOKE_AGENTS ALTER_ZERO_MASCOT_ANIMATION=0"
PR_APP="env NO_PROXY=127.0.0.1 no_proxy=127.0.0.1 $PR_ENV ALTER_ZERO_PROVIDERS_FILE=$PR_PROVIDERS ALTER_ZERO_PROVIDER=stub ALTER_ZERO_MODEL=stub-model STUB_API_KEY=stand-in $BIN_ABS"

pr_server_running() { pgrep -f "$PR_PLUGIN/server.sh" >/dev/null 2>&1; }
pr_open() {
	type_text "$S162" "$1"
	sleep 0.4
	keys "$S162" Enter
	sleep 0.6
}

# --- the review, before anything of the plugin has run
launch -c "$PR_WORK" -w "Esc to continue without trusting" "$S162" 140 45 "$PR_APP"
pr_hooks="$(pane "$S162")"
dump "the hooks page" "$pr_hooks"
expect_has "$pr_hooks" -F "Plugin hello@demo · " "the review does not name the plugin its hook comes from"
expect_has "$pr_hooks" -F "/plugins/hello/hooks/start.sh" "the hook's real command is not on the review"
expect_lacks "$pr_hooks" -F '${CLAUDE_PLUGIN_ROOT}' "the hook's placeholder was not substituted"
sleep 1
[ -e "$PR_DATA/started" ] && fail "the plugin's hook ran before the review was answered"
pr_server_running && fail "the plugin's MCP server started before the review was answered"
[ -d "$PR_DATA" ] || fail "the plugin's data directory was not created before its hooks were handed over"
keys "$S162" Enter
pr_servers="$(wait_pane 5 "$S162" -F "found in your plugins")" || fail "the servers page never followed the hooks page"
dump "the servers page" "$pr_servers"
expect_has "$pr_servers" -F "plugin:hello:probe · stdio" "the server is not on the review under its scoped name"
expect_has "$pr_servers" -F "Plugin hello@demo · " "the servers page does not name the plugin"
pr_server_running && fail "the plugin's MCP server started before its page was answered"
keys "$S162" Enter
wait_for 10 "$S162" -E '^❯' || fail "the composer never came back after the review"

# --- trusted: the hook fires (SessionStart, released by the review) and
# the server connects under Claude Code's name.
wait_file 10 "$PR_DATA/started" -F started || fail "the trusted plugin hook never fired"
pr_open "/mcp"
pr_mcp="$(wait_pane 15 "$S162" -F "plugin:hello:probe · ✔ connected")" || fail "the plugin's server did not connect"
dump "/mcp" "$pr_mcp"
expect_has "$pr_mcp" -F "Plugin MCPs" "/mcp does not list the plugin's server under its scope"
keys "$S162" Escape
sleep 0.4

# --- the `$` picker completes the skill and the command
type_text "$S162" '$hello'
pr_picker="$(wait_pane 5 "$S162" -F "hello:greet")" || fail "the \$ picker does not offer the plugin's skill"
dump "the \$ picker" "$pr_picker"
expect_has "$pr_picker" -F "hello:hi" "the \$ picker does not offer the plugin's command"
keys "$S162" C-c
sleep 0.3

# --- a turn (refused by the stub), then Ctrl+D's listing
submit "$S162" "hi there"
wait_for 10 "$S162" -F "smoke162 refused" || fail "the stub's refusal never showed"
keys "$S162" C-d
sleep 0.8
keys "$S162" Home
sleep 0.4
pr_ctx="$(pane "$S162")"
for _ in 1 2 3; do
	keys "$S162" PageDown
	sleep 0.3
	pr_ctx="$pr_ctx
$(pane "$S162")"
done
dump "Ctrl+D" "$pr_ctx"
for expect in "hello:greet" "hello:hi" "hello:reviewer"; do
	expect_has "$pr_ctx" -F "$expect" "the derived context's listing does not name '$expect'"
done
keys "$S162" C-d
sleep 0.4

# --- reinstalled by another process, with a changed hook script under the
# same command line: the open session forgets its approval at the next turn
# start (docs/plugins.md *Nothing runs unseen*) — the hook does not run, and
# the review asks again.
wait_file 10 "$PR_DATA/prompted" -F one || fail "the trusted UserPromptSubmit hook never fired"
printf '#!/bin/sh\ncat > /dev/null\necho two >> "$CLAUDE_PLUGIN_DATA/prompted"\n' >"$PR_PLUGIN/hooks/prompt.sh"
(cd "$PR_WORK" && env $PR_ENV "$BIN_ABS" plugin install hello@demo >/dev/null 2>&1) ||
	fail "the command-line reinstall failed"
submit "$S162" "after the reinstall"
pr_again="$(wait_pane 10 "$S162" -F "Esc to continue without trusting")" ||
	fail "the reinstalled plugin's hook was not taken back to the review"
dump "the review after a reinstall elsewhere" "$pr_again"
expect_has "$pr_again" -F "/plugins/hello/hooks/prompt.sh" "the review does not ask about the reinstalled plugin's hook"
sleep 1
grep -q two "$PR_DATA/prompted" && fail "the reinstalled plugin's changed hook ran under its old approval"
keys "$S162" Escape
sleep 0.6
# Its server was forgotten with it: its page follows, and is declined too.
if pane "$S162" | grep -qF "found in your plugins"; then
	keys "$S162" Escape
fi
wait_for 10 "$S162" -E '^❯' || fail "the composer never came back after the second review"

# --- /skills lists them
pr_open "/skills"
pr_skills="$(wait_pane 5 "$S162" -F "hello:greet")" || fail "/skills does not list the plugin's skill"
dump "/skills" "$pr_skills"
expect_has "$pr_skills" -F "hello:hi" "/skills does not list the plugin's command"
keys "$S162" Escape
tmux kill-session -t "$S162" 2>/dev/null

# --- ALTER_ZERO_PLUGINS=0: none of it
launch -c "$PR_WORK" "$S162" 140 45 "env ALTER_ZERO_PLUGINS=0 $PR_APP"
expect_lacks "$(pane "$S162")" -F "Plugin hello@demo" "ALTER_ZERO_PLUGINS=0 still reviewed the plugin"
pr_open "/skills"
pr_none="$(wait_pane 5 "$S162" -F "No skills found")" || fail "with ALTER_ZERO_PLUGINS=0, /skills did not open on an empty list"
expect_lacks "$pr_none" -F "hello:greet" "ALTER_ZERO_PLUGINS=0 still loaded the plugin's skill"
tmux kill-session -t "$S162" 2>/dev/null

# --- a directory that turned the plugin off: none of it
launch -c "$PR_OFF" "$S162" 140 45 "$PR_APP"
expect_lacks "$(pane "$S162")" -F "Plugin hello@demo" "a directory with the plugin off still reviewed it"
pr_open "/skills"
pr_none="$(wait_pane 5 "$S162" -F "No skills found")" || fail "with the plugin off here, /skills did not open on an empty list"
expect_lacks "$pr_none" -F "hello:greet" "a directory with the plugin off still loaded its skill"
tmux kill-session -t "$S162" 2>/dev/null

# --- the memory gate (Phase 159's method): with no plugin on, the feature
# costs an idle session nothing, whatever is recorded beside it.
pr_rss() {
	local pid exe want
	want="$(readlink -f "$BIN" 2>/dev/null)"
	pid="$(tmux display-message -p -t "$1" '#{pane_pid}')"
	for pid in "$pid" $(cat "/proc/$pid/task/$pid/children" 2>/dev/null); do
		exe="$(readlink -f "/proc/$pid/exe" 2>/dev/null)"
		if [ -n "$want" ] && [ "$exe" = "$want" ]; then
			awk '/^RssAnon:/ {print $2}' "/proc/$pid/status" 2>/dev/null
			return
		fi
	done
}
# Two marketplaces recorded, each with a large catalog on disk, nothing
# installed.
PR_BIG="$SMOKE_TMP/bigcfg"
for name in alpha beta; do
	mkdir -p "$PR_BIG/plugins/marketplaces/$name/.claude-plugin"
	python3 - "$PR_BIG/plugins/marketplaces/$name/.claude-plugin/marketplace.json" "$name" <<'PY'
import json, sys
plugins = [{"name": f"p{i}", "source": f"./plugins/p{i}", "description": "x" * 600} for i in range(400)]
json.dump({"name": sys.argv[2], "owner": {"name": "o"}, "plugins": plugins}, open(sys.argv[1], "w"))
PY
done
printf '{ "marketplaces": { "alpha": { "source": "o/alpha" }, "beta": { "source": "o/beta" } } }\n' >"$PR_BIG/plugins.json"
PR_EMPTY="$SMOKE_TMP/emptycfg"
mkdir -p "$PR_EMPTY"
pr_idle() { # CONFIG_HOME [EXTRA_ENV] → RssAnon kB of an idle session
	launch -c "$PR_WORK" "$S162" 120 30 "env ${2:-} ALTER_ZERO_TELEMETRY=0 ALTER_ZERO_UPDATE_CHECK=0 ALTER_ZERO_TIPS=0 ALTER_ZERO_PROJECT_CONFIG=0 ALTER_ZERO_CONFIG_DIR=$1 ALTER_ZERO_CHECKPOINTS=0 ALTER_ZERO_HISTORY_FILE=/dev/null ALTER_ZERO_SKILLS_DIR=$SMOKE_SKILLS ALTER_ZERO_AGENTS_DIR=$SMOKE_AGENTS ALTER_ZERO_MASCOT_ANIMATION=0 ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN_ABS" >/dev/null
	sleep 3
	pr_rss "$S162"
	tmux kill-session -t "$S162" 2>/dev/null
}
pr_base="$(pr_idle "$PR_EMPTY")"
pr_big="$(pr_idle "$PR_BIG")"
pr_off="$(pr_idle "$PR_CFG" ALTER_ZERO_PLUGINS=0)"
if [ -n "$pr_base" ] && [ -n "$pr_big" ] && [ -n "$pr_off" ]; then
	note "idle RssAnon: $pr_base kB empty config home, $pr_big kB two catalogs recorded, $pr_off kB ALTER_ZERO_PLUGINS=0"
	for pair in "$pr_big:two recorded marketplaces" "$pr_off:ALTER_ZERO_PLUGINS=0"; do
		value="${pair%%:*}"
		what="${pair#*:}"
		diff=$((value - pr_base))
		[ "${diff#-}" -gt 64 ] && fail "an idle session with $what is ${diff} kB from one over an empty config home (more than 64 kB)"
	done
else
	note "no /proc to read resident memory from; the memory gate was skipped"
fi
