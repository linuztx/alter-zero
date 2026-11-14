#!/usr/bin/env bash
# Phase 154 — the context gauge across a resume: a reopened conversation reads what it read before it was closed

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# The footer's context gauge is kept in the provider's own count: the last
# usage frame's `input`, plus the estimate of what joined since
# (docs/token-estimate.md). That count used to live only in memory, so a
# conversation that read `8.1k/475k` read `8.8k/475k` once it was reopened —
# the whole context estimated again, tool schemas the provider renders its
# own way included. The rollout now keeps the meter (a `context_meter`
# sidecar line) and a resume brings it back.
#
# Against the real backend over a local OpenAI-compatible stub (one model, a
# 200k window) whose usage frames count a request at a seventh of its bytes —
# well under the app's own estimate of the same context, so a reading that
# fell back to the estimate is a different number on the footer:
#   (a) a turn's footer reads the stub's count; the rollout keeps the meter,
#       its `tokens` the count the stub last reported;
#   (b) `--continue` reads the same number, to the character, and leaves the
#       file byte for byte as it was — a resume appends no line of its own;
#   (c) a fresh launch reads zero, and the `/resume` picker inside it brings
#       the conversation back on that same number;
#   (d) with the `context_meter` lines taken out — a rollout from before the
#       meter was kept — `--continue` still reads it: the file's last `usage`
#       line is the provider's count of the context it sits behind;
#   (e) two long answers later the meter has learned how this provider counts
#       (the change between two frames against the estimate of it), and the
#       next `--continue` reads the number that ratio made — which the
#       uncorrected estimate of the last answer would miss by a kilotoken;
#   (f) with every count taken out — a rollout from before 0.19.0, or one
#       whose provider reports no usage — `--continue` reads the estimate,
#       and so does the `/resume` picker inside a session that has just
#       counted a conversation of its own: that count describes the
#       conversation that went, and is not carried into the one reopened.
# And with a stdio MCP server's tools in the request:
#   (g) a session recorded with the server connected reads, reopened, the
#       recorded number once the server is back — its tools were in the
#       provider's count, and a tool set that changes moves the reading
#       without moving the meter, so nothing is appended here either.
S154="${S}_gauge"
G_LOG="$SMOKE_TMP/stub.log"
G_PORT="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
python3 - "$G_PORT" "$G_LOG" <<'PY' &
import json, sys
from http.server import BaseHTTPRequestHandler, HTTPServer

port, log = int(sys.argv[1]), sys.argv[2]

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
        # A count no estimate of the same bytes would land on.
        prompt = max(1, len(raw) // 7)
        with open(log, "a") as f:
            f.write(f"{prompt} mcp={int(b'mcp__fixture__echo_text' in raw)}\n")
        reply = "A short reply from the stub."
        if b"a long answer please" in raw:
            reply = "The context window fills as the conversation grows. " * 700
        self.send_response(200)
        self.send_header("content-type", "text/event-stream")
        self.end_headers()
        for frame in (
            {"choices": [{"index": 0, "delta": {"content": reply}}]},
            {"choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]},
            {"choices": [], "usage": {"prompt_tokens": prompt, "completion_tokens": 42}},
        ):
            self.wfile.write(f"data: {json.dumps(frame)}\n\n".encode())
        self.wfile.write(b"data: [DONE]\n\n")

    def log_message(self, *_args):
        pass

HTTPServer(("127.0.0.1", port), Stub).serve_forever()
PY
G_SERVER=$!
smoke_on_exit kill "$G_SERVER"
for _ in $(seq 1 30); do # wait for the stub to listen
	if python3 -c "import socket, sys; s = socket.socket(); s.settimeout(0.2); sys.exit(0 if s.connect_ex(('127.0.0.1', $G_PORT)) == 0 else 1)" 2>/dev/null; then
		break
	fi
	sleep 0.1
done
G_PROVIDERS="$SMOKE_TMP/providers.toml"
cat >"$G_PROVIDERS" <<TOML
[providers.stub]
name = "Stub"

[providers.stub.kwargs]
api_base = "http://127.0.0.1:$G_PORT/v1"
TOML

G_DIR="$(work_dir gauge)"
G_APP="env NO_PROXY=127.0.0.1 no_proxy=127.0.0.1 $CFG_ENV_NOHIST ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS ALTER_ZERO_PROVIDERS_FILE=$G_PROVIDERS ALTER_ZERO_PROVIDER=stub ALTER_ZERO_MODEL=stub-model STUB_API_KEY=smoke-154 $BIN_ABS"

# g_gauge PAGE — the footer's `{used}/{window} ({pct}%)` on a captured pane.
g_gauge() { grep -oE '[0-9.]+k?/200k \([0-9.]+%\)' <<<"$1" | tail -n 1; }
# g_quit SESSION — /quit the instance and wait for it to be gone.
g_quit() {
	submit "$1" "/quit"
	wait_gone 10 "$1" || {
		fail "session $1 did not quit"
		tmux kill-session -t "$1" 2>/dev/null
	}
}
# g_rollout DIR — the one rollout recorded in working directory DIR.
g_rollout() { grep -lF "\"cwd\":\"$(cd "$1" && pwd -P)\"" $(find "$SMOKE_SESSIONS" -type f -name 'rollout-*.jsonl' | sort) | head -n 1; }
# g_meters FILE — the file's `context_meter` lines.
g_meters() { grep -F '"type":"context_meter"' "$1" || true; }
# g_reopened SESSION WANT WHAT — wait for the reopened session's footer to
# read WANT, printing the pane.
g_reopened() {
	local page
	page="$(wait_pane 10 "$1" -F "$2")" ||
		fail "$3 reads '$(g_gauge "$page")', not the '$2' it read before it was closed"
	dump "$3" "$page"
}

# ---- (a) a turn: the footer reads the provider's count, the rollout keeps it ----
launch -c "$G_DIR" "$S154" 110 30 "$G_APP"
wait_for 10 "$S154" -F "stub-model · ~/gauge" || fail "the stub provider's model never reached the footer"
submit "$S154" "first question"
turn1="$(wait_summaries 20 "$S154" 1 -S -200)" || fail "the first turn against the stub never settled"
turn1="$(wait_settled 5 "$S154" -F "/200k")"
dump "the footer after the first turn" "$turn1"
G1="$(g_gauge "$turn1")"
[ -n "$G1" ] || fail "the footer shows no context gauge after the turn"
counted="$(tail -n 1 "$G_LOG" | cut -d' ' -f1)"
note "the stub counted $counted tokens for the request; the footer reads $G1"
g_quit "$S154"
G_FILE="$(g_rollout "$G_DIR")"
[ -n "$G_FILE" ] || fail "the first session recorded no rollout"
note "the rollout's context meter"
g_meters "$G_FILE"
expect_eq "$(g_meters "$G_FILE" | wc -l | tr -d ' ')" "1" "one request should leave one context_meter line"
expect_has "$(g_meters "$G_FILE")" -F "\"tokens\":$counted," "the kept meter's count is not the one the provider reported"
expect_has "$(g_meters "$G_FILE")" -F '"model":"stub-model"' "the kept meter does not name the model it counted in"
# The meter sits behind the request that moved it.
expect_eq "$(grep -nE '"type":"(usage|context_meter)"' "$G_FILE" | tail -n 1 | grep -c context_meter)" "1" \
	"the context_meter line is not written behind its request's usage line"

# ---- (b) --continue: the same number, and nothing appended ----
before_sum="$(cksum <"$G_FILE")"
launch -c "$G_DIR" "$S154" 110 30 "$G_APP --continue"
g_reopened "$S154" "$G1" "--continue"
expect_has "$(pane "$S154" -S -50)" -F "first question" "--continue did not reopen the conversation"
g_quit "$S154"
expect_eq "$(cksum <"$G_FILE")" "$before_sum" "a resume that changed nothing rewrote or appended to the rollout"

# ---- (c) a fresh launch reads zero; the /resume picker brings the number back ----
launch -c "$G_DIR" "$S154" 110 30 "$G_APP"
fresh="$(wait_pane 10 "$S154" -F "/200k")" || fail "the fresh session's footer has no gauge"
expect_eq "$(g_gauge "$fresh")" "0/200k (0.0%)" "a fresh session's gauge is not zero"
submit "$S154" "/resume"
wait_for 5 "$S154" -F "first question" || fail "the /resume picker does not list the recorded conversation"
keys "$S154" Enter
g_reopened "$S154" "$G1" "the /resume picker's load"
g_quit "$S154"
expect_eq "$(cksum <"$G_FILE")" "$before_sum" "the picker's resume rewrote or appended to the rollout"

# ---- (d) a rollout from before the meter was kept: its last usage line ----
sed -i.bak '/"type":"context_meter"/d' "$G_FILE" && rm -f "$G_FILE.bak"
expect_eq "$(g_meters "$G_FILE" | wc -l | tr -d ' ')" "0" "the context_meter lines were not taken out"
expect_file_has "$G_FILE" -F '"type":"usage"' "the rollout has no usage line to fall back on"
launch -c "$G_DIR" "$S154" 110 30 "$G_APP --continue"
g_reopened "$S154" "$G1" "--continue of a rollout with no kept meter"

# ---- (e) two long answers: the learned ratio survives the next resume ----
submit "$S154" "a long answer please"
wait_summaries 40 "$S154" 2 -S -3000 >/dev/null || fail "the first long answer never settled"
submit "$S154" "and one more"
wait_summaries 40 "$S154" 3 -S -6000 >/dev/null || fail "the second long answer never settled"
turn3="$(wait_settled 5 "$S154" -F "/200k")"
G3="$(g_gauge "$turn3")"
note "after two long answers the footer reads $G3"
expect_ne "$G3" "$G1" "the gauge did not move over two long answers"
g_quit "$S154"
learned="$(g_meters "$G_FILE" | tail -n 1)"
note "the newest context meter"
printf '%s\n' "$learned"
expect_lacks "$learned" -F '"learned":{"provider":0,"estimated":0}' \
	"two frames a long answer apart taught the meter nothing"
launch -c "$G_DIR" "$S154" 110 30 "$G_APP --continue"
g_reopened "$S154" "$G3" "--continue after the meter learned the provider's ratio"
g_quit "$S154"

# ---- (f) no count kept: the picker reads the estimate --continue reads ----
# Both readings are estimates of the whole context taken in two processes,
# so the scratchpad is off for this step: its directory names each launch's
# own session, which moves such an estimate by a few tokens between launches.
sed -i.bak -e '/"type":"context_meter"/d' -e '/"type":"usage"/d' "$G_FILE" && rm -f "$G_FILE.bak"
expect_file_lacks "$G_FILE" -F '"type":"usage"' "the usage lines were not taken out"
G_BARE="env ALTER_ZERO_SCRATCHPAD=0 $G_APP"
launch -c "$G_DIR" "$S154" 110 30 "$G_BARE --continue"
wait_for 10 "$S154" -S -6000 -- -F "a long answer please" || fail "--continue did not reopen the stripped conversation"
bare="$(wait_settled 10 "$S154" -F "/200k")"
GE="$(g_gauge "$bare")"
note "with no count kept, --continue reads the estimate: $GE"
expect_ne "$GE" "$G3" "a rollout with every count taken out still read the count"
g_quit "$S154"
launch -c "$G_DIR" "$S154" 110 30 "$G_BARE"
wait_for 10 "$S154" -F "stub-model · ~/gauge" || fail "the session that counts another conversation never came up"
submit "$S154" "a fresh question"
wait_summaries 20 "$S154" 1 -S -200 >/dev/null || fail "the fresh conversation's turn never settled"
fresh="$(wait_settled 5 "$S154" -F "/200k")"
note "the fresh conversation, counted by the stub, reads $(g_gauge "$fresh")"
# The picker never lists the conversation it is opened in, so its one row
# here is the stripped conversation's.
submit "$S154" "/resume"
wait_for 5 "$S154" -F "first question" || fail "the /resume picker does not list the stripped conversation"
keys "$S154" Enter
g_reopened "$S154" "$GE" "the /resume picker's load into a session that counted another conversation"
g_quit "$S154"

# ---- (g) with an MCP server's tools in the count ----
# The /mcp phase's scripted stdio server, answering at once — its one tool
# described at length, so its schema is a share of the context the footer's
# rounding shows: a reading that lost the tool, or counted it twice, is
# another number.
python3 - "$SMOKE_TMP" <<'PY'
import json, sys

root = sys.argv[1]
init = {"jsonrpc": "2.0", "id": 1, "result": {
    "resultType": "complete", "supportedVersions": ["2026-07-28"],
    "capabilities": {"tools": {}}, "ttlMs": 60000, "cacheScope": "public",
    "_meta": {"io.modelcontextprotocol/serverInfo": {"name": "fixture", "version": "1.0"}}}}
tools = {"jsonrpc": "2.0", "id": 2, "result": {
    "resultType": "complete", "ttlMs": 60000, "cacheScope": "public",
    "tools": [{
        "name": "echo_text",
        "description": "Echo the text back exactly as it was given, so a caller can check it. " * 40,
        "inputSchema": {"type": "object", "required": ["text"],
                        "properties": {"text": {"type": "string", "description": "What to echo."}}}}]}}
for name, message in (("mcp-init.json", init), ("mcp-tools.json", tools)):
    with open(f"{root}/{name}", "w") as f:
        f.write(json.dumps(message) + "\n")
PY
cat >"$SMOKE_TMP/server.sh" <<MCPSRV
#!/bin/sh
cat > /dev/null &
cat "$SMOKE_TMP/mcp-init.json" "$SMOKE_TMP/mcp-tools.json"
sleep 120
MCPSRV
cat >"$SMOKE_CFG/mcp.json" <<MCPJSON
{"mcpServers": {"fixture": {"type": "stdio", "command": "sh", "args": ["$SMOKE_TMP/server.sh"]}}}
MCPJSON
M_DIR="$(work_dir gaugemcp)"
S154M="${S}_gaugemcp"
launch -c "$M_DIR" "$S154M" 110 40 "$G_APP"
wait_for 10 "$S154M" -F "stub-model · ~/gaugemcp" || fail "the MCP session's footer never came up"
# The server's tools must be in the request the provider counts.
submit "$S154M" "/context"
wait_for 15 "$S154M" -E "■ MCP tools +[0-9.]+k?" || fail "the MCP server's tools never reached the session"
keys "$S154M" Escape
wait_for 5 "$S154M" -F "stub-model · ~/gaugemcp" || fail "Esc did not close the /context page"
submit "$S154M" "first question"
wait_summaries 20 "$S154M" 1 -S -200 >/dev/null || fail "the MCP session's turn never settled"
mcp1="$(wait_settled 5 "$S154M" -F "/200k")"
GM="$(g_gauge "$mcp1")"
expect_has "$(tail -n 1 "$G_LOG")" -F "mcp=1" "the counted request did not carry the MCP server's tool"
note "with the MCP server's tools the footer reads $GM"
expect_ne "$GM" "$G1" "the MCP server's tools made no difference the footer shows"
g_quit "$S154M"
M_FILE="$(g_rollout "$M_DIR")"
[ -n "$M_FILE" ] || fail "the MCP session recorded no rollout"
mcp_sum="$(cksum <"$M_FILE")"
launch -c "$M_DIR" "$S154M" 110 40 "$G_APP --continue"
g_reopened "$S154M" "$GM" "--continue with the MCP server back"
# …and it holds there: the reading has stopped moving on the recorded number.
held="$(wait_settled 5 "$S154M" -F "/200k")"
expect_eq "$(g_gauge "$held")" "$GM" "the reopened MCP session's gauge did not hold the recorded number"
g_quit "$S154M"
expect_eq "$(cksum <"$M_FILE")" "$mcp_sum" "the MCP server connecting after a resume appended to the rollout"
