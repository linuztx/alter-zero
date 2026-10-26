#!/usr/bin/env bash
# Phase 145 — a reply whose connection drops mid-stream starts over, the partial withdrawn

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# A connection lost under a reply (docs/offline.md *A connection lost
# mid-reply*). The user's report was `request failed: request or response body
# error: error reading a body from connection: Connection timed out (os error
# 110)` — a turn killed half-way. A stub chat-completions server answers the
# first request with a thought and two finished lines of a reply, then closes
# the socket mid-body (a chunked response that never gets its terminating
# chunk — the body read fails the same way a timed-out or reset connection's
# does). The turn must not fail: the retry withdraws everything that attempt
# streamed — the settled `Thought for` cell and the committed lines, from the
# screen, the scrollback and the rollout alike — the status line says
# `retrying 1/3` and a toast says why the text went, and the second answer
# streams the whole reply exactly once.
S145="${S}_droppedreply"
DROP_CFG="$(mktemp -d "$SMOKE_TMP/droppedreply-cfg.XXXXXX")"
DROP_DIR="$(work_dir droppedreply)"
DROP_LOG="$SMOKE_TMP/droppedreply-requests.log"
DROP_PORT="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
cat >"$DROP_CFG/providers.toml" <<PROVIDERS
[providers.flaky]
name = "Flaky"
api_model_base = "http://127.0.0.1:$DROP_PORT/v1"

[providers.flaky.kwargs]
api_base = "http://127.0.0.1:$DROP_PORT/v1"
PROVIDERS
cat >"$DROP_CFG/config.json" <<'CONFIG'
{ "provider": "flaky", "model": "stub-model" }
CONFIG
python3 - "$DROP_PORT" "$DROP_LOG" <<'PY' &
import json, sys, threading, time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

port, log = int(sys.argv[1]), sys.argv[2]
lock = threading.Lock()
posts = 0

def frame(delta, finish=None):
    return ("data: " + json.dumps({
        "id": "stub", "object": "chat.completion.chunk", "model": "stub-model",
        "choices": [{"index": 0, "delta": delta, "finish_reason": finish}],
    }) + "\n\n").encode()

class Stub(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def chunk(self, data):
        self.wfile.write(b"%x\r\n%s\r\n" % (len(data), data))
        self.wfile.flush()

    def stream_headers(self):
        self.send_response(200)
        self.send_header("content-type", "text/event-stream")
        self.send_header("transfer-encoding", "chunked")
        self.end_headers()

    def do_POST(self):
        global posts
        self.rfile.read(int(self.headers.get("content-length") or 0))
        with lock:
            posts += 1
            n = posts
        with open(log, "a") as f:
            f.write(f"post {n}\n")
        self.close_connection = True
        if n == 1:
            # A thought, two finished lines and the start of a third — then
            # the connection dies under the reply: no terminating chunk.
            self.stream_headers()
            self.chunk(frame({"role": "assistant", "reasoning_content": "Pondering draft one."}))
            for piece in ["Draft one: the parser reads tokens.\n",
                          "Draft one: then it builds a tree.\n",
                          "Draft one: and then"]:
                self.chunk(frame({"content": piece}))
            time.sleep(3)
            return
        # The retry: a pause first, so the status line's retry shows.
        time.sleep(2)
        self.stream_headers()
        for piece in ["Second attempt: ", "the whole reply ", "streams once."]:
            self.chunk(frame({"content": piece}))
        self.chunk(frame({}, "stop"))
        self.chunk(b"data: [DONE]\n\n")
        self.wfile.write(b"0\r\n\r\n")
        self.wfile.flush()

    def do_GET(self):
        data = json.dumps({"data": [{"id": "stub-model"}]}).encode()
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, *_args):
        pass

ThreadingHTTPServer(("127.0.0.1", port), Stub).serve_forever()
PY
DROP_SERVER=$!
smoke_on_exit "kill $DROP_SERVER 2>/dev/null"
for _ in $(seq 1 30); do # wait for the stub to listen
	python3 -c "import socket, sys; s = socket.socket(); s.settimeout(0.2); sys.exit(0 if s.connect_ex(('127.0.0.1', $DROP_PORT)) == 0 else 1)" && break
	sleep 0.1
done
# The loopback request must not be sent to a developer's HTTPS proxy; the
# thinking display is on, so the first attempt's thought settles into a cell.
DROP_ENV="-u HTTPS_PROXY -u https_proxy -u HTTP_PROXY -u http_proxy -u ALL_PROXY -u all_proxy ALTER_ZERO_TELEMETRY=0 ALTER_ZERO_UPDATE_CHECK=0 ALTER_ZERO_PROJECT_CONFIG=0 ALTER_ZERO_CONFIG_DIR=$DROP_CFG ALTER_ZERO_SESSIONS_DIR=$DROP_CFG/sessions ALTER_ZERO_HISTORY_FILE=/dev/null ALTER_ZERO_CHECKPOINTS=0 ALTER_ZERO_SKILLS_DIR=$SMOKE_SKILLS ALTER_ZERO_AGENTS_DIR=$SMOKE_AGENTS ALTER_ZERO_PROVIDERS_FILE=$DROP_CFG/providers.toml ALTER_ZERO_API_KEY=not-a-real-key ALTER_ZERO_TIPS=0 ALTER_ZERO_SHOW_THINKING=1"

launch -c "$DROP_DIR" "$S145" 100 30 "env $DROP_ENV $BIN_ABS"
wait_for 5 "$S145" -F "stub-model" ||
	fail "the footer never named the stub model (the real backend did not activate)"
submit "$S145" "explain the parser"
partial="$(wait_pane 10 "$S145" -F "Draft one: then it builds a tree.")" ||
	fail "the first attempt's reply never reached the screen"
dump "the first attempt, streaming" "$partial"
retrying="$(wait_pane 15 "$S145" -F "retrying 1/3")" ||
	fail "the dropped reply was never retried (did it fail the turn instead?)"
dump "the connection lost: the partial withdrawn, the request sent again" "$retrying"
settled="$(wait_pane 15 "$S145" -E "^$SUMMARY_RE")" ||
	fail "the turn never settled after the retry"
dump "the second attempt's reply, the turn settled" "$settled"
history="$(pane "$S145" -S -400)"

note "a reply whose connection drops mid-stream starts over"
expect_has "$partial" -F "Thought for" "the first attempt's thought never settled into its cell"
expect_has "$partial" -F "Draft one: the parser reads tokens." "the first attempt's finished line never committed"
expect_lacks "$retrying" -F "Draft one" "the withdrawn reply stayed on screen"
expect_lacks "$retrying" -F "Thought for" "the withdrawn attempt's thought stayed on screen"
expect_has "$retrying" -F "Connection lost mid-reply — starting it over" "no toast said why the reply went"
expect_lacks "$retrying" -F "request failed" "the dropped connection failed the turn"
expect_has "$settled" -F "Second attempt: the whole reply streams once." "the retried reply did not stream whole"
expect_lacks "$settled" -F "retrying" "the retry clause outlived the request that got through"
expect_lacks "$settled" -F "Conversation interrupted" "the restart was reported as an interrupt"
expect_lacks "$history" -F "Draft one" "the withdrawn reply survived in scrollback"
expect_lacks "$history" -F "Thought for" "the withdrawn thought survived in scrollback"
expect_lacks "$history" -F "request failed" "an error reached scrollback"
expect_eq "$(grep -cF "Second attempt: the whole reply streams once." <<<"$history")" "1" \
	"the reply should appear exactly once"
expect_eq "$(grep -c "^post" "$DROP_LOG")" "2" "the request should be sent exactly twice"
# The rollout follows history: the thought the withdrawal popped is rewritten
# out of the file, the reply that got through is in it.
poll 5 grep -rqF "streams once." "$DROP_CFG/sessions" ||
	fail "the reply never reached the rollout"
grep -rqF "Pondering draft one" "$DROP_CFG/sessions" &&
	fail "the withdrawn thought survived in the rollout"
kill "$DROP_SERVER" 2>/dev/null
tmux kill-session -t "$S145" 2>/dev/null

# ---- half two: a subagent's reply drops while its session view is open ----
# The lead launches one foreground agent; the agent's first request streams
# two finished lines and is held there while the script opens the agent's
# session view, then loses its connection the same way. The view withdraws
# the partial and says why, the retry streams the agent's whole reply once —
# and the lead's next request carries exactly that reply as the agent's
# answer: the forwarder's record is withdrawn too, never the partial glued
# to the reply that replaced it.
S145B="${S}_droppedagent"
AGENT_WORK="$(work_dir droppedagent)"
AGENT_LOG="$SMOKE_TMP/droppedagent.log"
HELD="$SMOKE_TMP/held145"
GO="$SMOKE_TMP/go145"
AGENT_PORT="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
python3 - "$AGENT_PORT" "$AGENT_LOG" "$HELD" "$GO" <<'PY' &
import json, os, sys, threading, time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

port, log, held, go = int(sys.argv[1]), sys.argv[2], sys.argv[3], sys.argv[4]
lock = threading.Lock()
agent_posts = 0

def text(message):
    content = message.get("content")
    if isinstance(content, list):
        return " ".join(part.get("text", "") for part in content if isinstance(part, dict))
    return content or ""

def said(words):
    return {"choices": [{"delta": {"content": words}}]}

def finish(reason):
    return {"choices": [{"delta": {}, "finish_reason": reason}]}

class Stub(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def do_GET(self):
        body = b'{"object":"list","data":[{"id":"stub-model"}]}'
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def chunk(self, data):
        self.wfile.write(b"%x\r\n%s\r\n" % (len(data), data))
        self.wfile.flush()

    def frames(self, frames, cut=False):
        self.send_response(200)
        self.send_header("content-type", "text/event-stream")
        self.send_header("transfer-encoding", "chunked")
        self.end_headers()
        for frame in frames:
            self.chunk(("data: %s\n\n" % json.dumps(frame)).encode())
        if cut:
            return
        self.chunk(b"data: [DONE]\n\n")
        self.wfile.write(b"0\r\n\r\n")
        self.wfile.flush()

    def do_POST(self):
        global agent_posts
        body = json.loads(self.rfile.read(int(self.headers.get("content-length") or 0)).decode())
        messages = body.get("messages", [])
        # Only the lead is offered `agent` — subagents never nest.
        lead = any((tool.get("function") or {}).get("name") == "agent"
                   for tool in body.get("tools") or [])
        with lock, open(log, "a") as f:
            f.write(json.dumps({"lead": lead, "messages": messages}) + "\n")
        self.close_connection = True
        if not lead:
            with lock:
                agent_posts += 1
                n = agent_posts
            if n == 1:
                # Two finished lines and the start of a third, held until
                # the script has read them in the session view — then the
                # connection dies under the reply.
                self.send_response(200)
                self.send_header("content-type", "text/event-stream")
                self.send_header("transfer-encoding", "chunked")
                self.end_headers()
                for words in ["Agent draft one: line A.\n", "Agent draft one: line B.\n",
                              "Agent draft one: and"]:
                    self.chunk(("data: %s\n\n" % json.dumps(said(words))).encode())
                open(held, "w").close()
                deadline = time.time() + 60
                while not os.path.exists(go) and time.time() < deadline:
                    time.sleep(0.05)
                return
            time.sleep(1.5)
            self.frames([said("The agent's whole reply."), finish("stop")])
        elif "smoke145 agent" in text(messages[-1]):
            call = {"index": 0, "id": "call_145", "type": "function",
                    "function": {"name": "agent", "arguments": json.dumps({
                        "description": "Survey the parser",
                        "prompt": "Survey the parser.",
                        "run_in_background": False})}}
            self.frames([{"choices": [{"delta": {"tool_calls": [call]}}]},
                         finish("tool_calls")])
        else:
            self.frames([said("All done."), finish("stop")])

    def log_message(self, *_args):
        pass

ThreadingHTTPServer(("127.0.0.1", port), Stub).serve_forever()
PY
AGENT_SERVER=$!
smoke_on_exit "kill $AGENT_SERVER 2>/dev/null"
AGENT_PROVIDERS="$SMOKE_TMP/providers145.toml"
cat >"$AGENT_PROVIDERS" <<EOF
[providers.stub]
name = "Stub"

[providers.stub.kwargs]
api_base = "http://127.0.0.1:$AGENT_PORT/v1"
EOF
AGENT_APP="env NO_PROXY=127.0.0.1 no_proxy=127.0.0.1 $CFG_ENV_NOHIST ALTER_ZERO_PERMISSIONS=0 ALTER_ZERO_PROVIDERS_FILE=$AGENT_PROVIDERS ALTER_ZERO_PROVIDER=stub ALTER_ZERO_MODEL=stub-model STUB_API_KEY=stand-in $BIN_ABS"
launch -c "$AGENT_WORK" "$S145B" 120 40 "$AGENT_APP"
submit "$S145B" "smoke145 agent"
poll 30 test -e "$HELD" || fail "the agent's first request never reached the stub"
# ↓ opens the roster on `● main`, a second ↓ steps onto the agent, Enter
# opens its session view (Phase 141's walk).
keys "$S145B" Down
sleep 0.15
keys "$S145B" Down
sleep 0.15
keys "$S145B" Enter
viewing="$(wait_pane 10 "$S145B" -F "Agent draft one: line B.")" ||
	fail "the agent's session view never showed its partial reply"
dump "the agent's session view, its reply streaming" "$viewing"
expect_has "$viewing" -E '^❯ Survey the parser\.' "the agent's session view did not open"
touch "$GO"
agent_retrying="$(wait_pane 15 "$S145B" -F "retrying 1/3")" ||
	fail "the agent's dropped reply was never retried"
dump "the agent's connection lost: its partial withdrawn" "$agent_retrying"
answered="$(wait_pane 15 "$S145B" -F "The agent's whole reply.")" ||
	fail "the agent's retried reply never reached its session view"
dump "the agent's whole reply" "$answered"
agent_history="$(pane "$S145B" -S -400)"
keys "$S145B" Escape
lead_done="$(wait_settled 30 "$S145B" -S -300 -- -F 'All done.')" ||
	fail "the lead's turn never ended"
dump "the lead, after the agent answered" "$lead_done"

note "a subagent's reply whose connection drops mid-stream starts over"
expect_lacks "$agent_retrying" -F "Agent draft one" "the withdrawn reply stayed in the agent's session view"
expect_has "$agent_retrying" -F "Connection lost mid-reply — starting it over" \
	"no toast said why the agent's reply went"
expect_lacks "$agent_history" -F "Agent draft one" "the withdrawn reply survived in the session view's scrollback"
expect_eq "$(grep -cF "The agent's whole reply." <<<"$agent_history")" "1" \
	"the agent's reply should appear exactly once in its session view"
expect_lacks "$lead_done" -F "request failed" "an error reached the lead's conversation"
python3 - "$AGENT_LOG" <<'PY' || fail "the agent's answer the lead read is wrong (see above)"
import json, sys

requests = [json.loads(line) for line in open(sys.argv[1]) if line.strip()]

def text(message):
    content = message.get("content")
    if isinstance(content, list):
        return " ".join(part.get("text", "") for part in content if isinstance(part, dict))
    return content or ""

agent_requests = [r for r in requests if not r["lead"]]
if len(agent_requests) != 2:
    print("FAIL: the agent's request should be sent exactly twice, got %d" % len(agent_requests))
    sys.exit(1)
answers = [text(m) for r in requests if r["lead"] for m in r["messages"]
           if m.get("role") == "tool" and "The agent's whole reply." in text(m)]
if not answers:
    print("FAIL: no lead request carried the agent's answer")
    sys.exit(1)
if "draft one" in answers[-1]:
    print("FAIL: the lead was handed the withdrawn partial: %r" % answers[-1])
    sys.exit(1)
PY
tmux kill-session -t "$S145B" 2>/dev/null
