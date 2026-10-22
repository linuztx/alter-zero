#!/usr/bin/env bash
# Phase 144 — the `/context` page: what fills the context window

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# The `/context` page (docs/context-usage.md): a read-only, composer-replacing
# page that breaks the context window down by what fills it — the system
# prompt, the built-in tools' schemas, an MCP server's tools, the project's
# AGENTS.md, the skills and agent-type listings, the conversation — over the
# free space and the auto-compact reserve, with the provider's own usage
# (tokens in/out and the cache-hit share) under it.
#
# Against the real backend over a local OpenAI-compatible stub (one model,
# a 200k window; each reply carries a usage frame — no cache reads on the
# first request, 86% of the prompt read from cache after it), with a skill,
# an AGENTS.md and a scripted stdio MCP server planted so every category
# has something in it:
#   (a) the palette lists /context, and it opens before any turn: every
#       category row, the free space and the reserve, the largest slice
#       selected with its parts beside it, and no usage reported yet;
#   (b) the page is live — the MCP server's tools appear once its startup
#       connect lands, without reopening — and ↓ walks the rows, each
#       selection's parts naming what costs what;
#   (c) Ctrl+D opens the raw context over the page and Esc comes back to
#       it; Esc closes the page back to the composer and its footer;
#   (d) after two turns the usage rows report the last request's tokens in
#       and out, the cache-hit share the provider answered, and the running
#       total since launch;
# Then against the offline dummy:
#   (e) /context opens mid-turn with the status line still above it, and
#       the page keeps counting while the turn streams underneath.
S144="${S}_context"
C_PORT="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
python3 - "$C_PORT" <<'PY' &
import json, sys
from http.server import BaseHTTPRequestHandler, HTTPServer

port = int(sys.argv[1])
seen = {"requests": 0}

class Stub(BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path.startswith("/v1/models"):
            data = json.dumps({"data": [{"id": "stub-model", "context_length": 200000}]}).encode()
            self.send_response(200)
            self.send_header("content-type", "application/json")
            self.send_header("content-length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)
        else:
            self.send_response(404)
            self.end_headers()

    def do_POST(self):
        raw = self.rfile.read(int(self.headers.get("content-length") or 0))
        seen["requests"] += 1
        prompt = max(1, len(raw) // 4)
        cached = 0 if seen["requests"] == 1 else int(prompt * 0.86)
        self.send_response(200)
        self.send_header("content-type", "text/event-stream")
        self.end_headers()
        for frame in (
            {"choices": [{"index": 0, "delta": {"content": "A short reply from the stub."}}]},
            {"choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]},
            {"choices": [], "usage": {"prompt_tokens": prompt, "completion_tokens": 42,
                                      "prompt_tokens_details": {"cached_tokens": cached}}},
        ):
            self.wfile.write(f"data: {json.dumps(frame)}\n\n".encode())
        self.wfile.write(b"data: [DONE]\n\n")

    def log_message(self, *_args):
        pass

HTTPServer(("127.0.0.1", port), Stub).serve_forever()
PY
C_SERVER=$!
smoke_on_exit kill "$C_SERVER"
for _ in $(seq 1 30); do # wait for the stub to listen
	if python3 -c "import socket, sys; s = socket.socket(); s.settimeout(0.2); sys.exit(0 if s.connect_ex(('127.0.0.1', $C_PORT)) == 0 else 1)" 2>/dev/null; then
		break
	fi
	sleep 0.1
done
C_PROVIDERS="$SMOKE_TMP/providers.toml"
cat >"$C_PROVIDERS" <<TOML
[providers.stub]
name = "Stub"

[providers.stub.kwargs]
api_base = "http://127.0.0.1:$C_PORT/v1"
TOML

# Something in every category: a skill in the suite's (otherwise empty)
# skills root, an AGENTS.md in the working directory — the suite turns
# project docs off, so this launch turns them back on — and a stdio MCP
# server in the config home, the /mcp phase's scripted modern server — held
# five seconds before it answers, so the page is open before its tools are
# there to count.
mkdir -p "$SMOKE_SKILLS/haiku-writer"
cat >"$SMOKE_SKILLS/haiku-writer/SKILL.md" <<'SKILL'
---
name: haiku-writer
description: Write a haiku about the code you just changed.
---
Write three lines of five, seven and five syllables.
SKILL
C_DIR="$(work_dir ctx)"
printf '# Contributor guide\n\nRun the tests before every commit.\n' >"$C_DIR/AGENTS.md"
cat >"$SMOKE_TMP/server.sh" <<'MCPSRV'
#!/bin/sh
cat > /dev/null &
sleep 5
printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"resultType":"complete","supportedVersions":["2026-07-28"],"capabilities":{"tools":{}},"ttlMs":60000,"cacheScope":"public","_meta":{"io.modelcontextprotocol/serverInfo":{"name":"fixture","version":"1.0"}}}}'
printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"resultType":"complete","tools":[{"name":"echo_text","description":"Echo the text back.","inputSchema":{"type":"object","properties":{"text":{"type":"string","description":"What to echo."}},"required":["text"]}}],"ttlMs":60000,"cacheScope":"public"}}'
sleep 60
MCPSRV
cat >"$SMOKE_CFG/mcp.json" <<MCPJSON
{"mcpServers": {"fixture": {"type": "stdio", "command": "sh", "args": ["$SMOKE_TMP/server.sh"]}}}
MCPJSON

C_APP="env NO_PROXY=127.0.0.1 no_proxy=127.0.0.1 $CFG_ENV_NOHIST ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS ALTER_ZERO_PROJECT_DOC_MAX_BYTES=32768 ALTER_ZERO_PROVIDERS_FILE=$C_PROVIDERS ALTER_ZERO_PROVIDER=stub ALTER_ZERO_MODEL=stub-model STUB_API_KEY=smoke-144 $BIN_ABS"

# c_select SESSION LABEL — press ↓ until LABEL's row is the selected one
# (its detail heading `LABEL · …` beside the legend), printing the page.
c_select() {
	local _
	for _ in 1 2 3 4 5 6 7 8 9 10 11; do
		if pane_has "$1" -F "❯ ■ $2 "; then
			pane "$1"
			return 0
		fi
		keys "$1" Down
		sleep 0.2
	done
	pane "$1"
	return 1
}
# c_tokens PAGE LABEL — the token count on LABEL's legend row ("37", "3.2k").
c_tokens() {
	grep -oE "$2 +[0-9.]+k?" <<<"$1" | head -n 1 | grep -oE '[0-9.]+k?$'
}

# ---- (a) the palette lists /context, and it opens before any turn ----
launch -c "$C_DIR" "$S144" 110 40 "$C_APP"
wait_for 10 "$S144" -F "stub-model · ~/ctx" || fail "the stub provider's model never reached the footer"
type_text "$S144" "/cont"
palette="$(wait_pane 5 "$S144" -F "Show what fills the context window")" ||
	fail "/context is missing from the slash-command palette"
dump "the palette filtered to /cont" "$palette"
keys "$S144" Enter
first="$(wait_pane 5 "$S144" -F "Context usage")" || fail "/context did not open the page"
dump "the page before any turn" "$first"
expect_has "$first" -E "stub-model · ≈[0-9.]+k? of 200k tokens" \
	"the page's subtitle does not name the model, the estimate and the 200k window"
for row in "System prompt" "System tools" "Project docs" "Skills" "Agent types" "Messages" \
	"Free space" "Auto-compact"; do
	expect_has "$first" -E "[■□╱] $row +[0-9.]+k? +[<0-9.]+%" "the legend has no '$row' row with tokens and a share"
done
expect_has "$first" -E "Auto-compact +20k +10\.0%" \
	"the auto-compact reserve is not the last 10% of the window"
# The largest slice opens selected, its parts beside the legend.
expect_has "$first" -E "❯ ■ System tools" "the largest slice (the tool schemas) did not open selected"
expect_has "$first" -E "System tools · [0-9]+ tools" "the selected slice's detail heading is missing"
expect_has "$first" -E "bash +[0-9.]+k?" "the tool detail does not list bash's schema cost"
expect_has "$first" -F "Last request" "the usage rows are missing"
expect_has "$first" -F "no usage reported yet" "the usage rows claim a request before any was made"
expect_lacks "$first" -F "stub-model · ~/ctx" "the footer is still showing under the page"
expect_lacks "$first" -F "MCP tools" "the MCP server's tools were counted before it answered"

# ---- (b) live: the MCP server's tools land on the open page; ↓ walks it ----
mcp="$(wait_pane 15 "$S144" -E "■ MCP tools +[0-9.]+k?")" ||
	fail "the MCP server's tools never reached the open page"
dump "the page once the MCP server connected" "$mcp"
if picked="$(c_select "$S144" "MCP tools")"; then
	expect_has "$picked" -F "MCP tools · 1 tool" "the MCP detail does not count the server's one tool"
	expect_has "$picked" -E "fixture · 1 tool +[0-9]+" "the MCP detail does not name the server"
else
	dump "↓ never reached MCP tools" "$picked"
	fail "↓ never selected the MCP tools row"
fi
if picked="$(c_select "$S144" "Project docs")"; then
	expect_has "$picked" -F "Project docs · 1 file" "the project-docs detail does not count AGENTS.md"
	expect_has "$picked" -E "AGENTS\.md +[0-9]+" "the project-docs detail does not name AGENTS.md"
else
	fail "↓ never selected the Project docs row"
fi
if picked="$(c_select "$S144" "Skills")"; then
	expect_has "$picked" -F "Skills · 1 skill" "the skills detail does not count the planted skill"
	expect_has "$picked" -E "haiku-writer +[0-9]+" "the skills detail does not name the planted skill"
else
	fail "↓ never selected the Skills row"
fi
if picked="$(c_select "$S144" "Agent types")"; then
	dump "the agent-types detail" "$picked"
	expect_has "$picked" -E "general-purpose +[0-9]+" "the agent-types detail does not list general-purpose"
else
	fail "↓ never selected the Agent types row"
fi

# ---- (c) Ctrl+D over the page and back; Esc closes it ----
keys "$S144" C-d
raw="$(wait_pane 5 "$S144" -F "C O N T E X T")" || fail "Ctrl+D did not open the raw context over the page"
keys "$S144" Escape
back="$(wait_pane 5 "$S144" -F "Context usage")" || fail "Esc from Ctrl+D did not come back to the page"
dump "back from Ctrl+D" "$back"
keys "$S144" Escape
closed="$(wait_pane 5 "$S144" -F "stub-model · ~/ctx")" || fail "Esc did not close the page back to the composer"
dump "the page closed" "$closed"
expect_lacks "$closed" -F "Context usage" "the page is still on screen after Esc"

# ---- (d) two turns: tokens in/out, the cache hit and the running total ----
submit "$S144" "first question"
wait_summaries 20 "$S144" 1 -S -200 >/dev/null || fail "the first turn against the stub never settled"
submit "$S144" "second question"
wait_summaries 20 "$S144" 2 -S -200 >/dev/null || fail "the second turn against the stub never settled"
submit "$S144" "/context"
usage="$(wait_pane 5 "$S144" -F "Since launch")" || fail "the reopened page has no Since launch row"
dump "the page after two turns" "$usage"
expect_has "$usage" -E "Last request +[0-9.]+k? in · 42 out" "the last request's tokens in/out are missing"
expect_has "$usage" -E "Cache hit +[█░]+ +86%" "the cache-hit meter does not show the provider's 86%"
expect_has "$usage" -E "Since launch +2 requests · [0-9.]+k? in · 84 out" "the running total does not add both requests"
expect_lacks "$usage" -F "no usage reported yet" "the usage rows still read as empty after two requests"
expect_has "$usage" -E "■ Messages +[0-9.]+k?" "the Messages row is missing after two turns"
keys "$S144" Escape
wait_for 5 "$S144" -F "stub-model · ~/ctx" || fail "Esc did not close the reopened page"
tmux kill-session -t "$S144" 2>/dev/null

# ---- (e) the dummy, mid-turn: the status line above, the page live ----
rm -f "$SMOKE_CFG/mcp.json"
S144D="${S}_contextdummy"
launch -c "$C_DIR" "$S144D" 100 40 "env $CFG_ENV_NOHIST ALTER_ZERO_STARTUP_DELAY_MS=2500 $BIN_ABS"
submit "$S144D" "$USER_MSG"
wait_for 5 "$S144D" -F "esc to interrupt" || fail "the dummy's turn never started"
submit "$S144D" "/context"
mid="$(wait_pane 5 "$S144D" -F "Context usage")" || fail "/context did not open mid-turn"
dump "the page mid-turn" "$mid"
expect_has "$mid" -F "esc to interrupt" "the running turn's status line is not above the page"
before="$(c_tokens "$mid" "Messages")"
expect_ne "${before:-}" "" "the mid-turn page has no Messages count"
wait_summaries 20 "$S144D" 1 -S -200 >/dev/null || fail "the dummy's turn never settled under the page"
after_page="$(wait_pane 5 "$S144D" -F "Context usage")" || fail "the page closed itself when the turn ended"
dump "the page after the turn settled" "$after_page"
after="$(c_tokens "$after_page" "Messages")"
expect_ne "$after" "$before" "the page did not re-count the conversation as the turn streamed"
keys "$S144D" Escape
wait_for 5 "$S144D" -F "dummy_model_name ·" || fail "Esc did not close the page after the dummy's turn"
tmux kill-session -t "$S144D" 2>/dev/null
