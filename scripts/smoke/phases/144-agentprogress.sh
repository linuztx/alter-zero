#!/usr/bin/env bash
# Phase 144 — a subagent's progress updates: on its live row, and in the lead's agentoutput report

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# docs/agent-progress.md, driven through the REAL backend against a stub
# provider. The lead launches one FOREGROUND agent. The agent's first reply
# is held mid-stream after its first sentence, so the lead's live cell can be
# read while the agent talks: `⎿  Agent: …`, where the row used to name only
# calls. Released, the agent runs a command, says what it found, runs another
# and answers. The lead then asks `agentoutput` about it, and the report —
# the lead's AgentOutput cell, and the tool result in the request the stub
# sees next — lists both of the agent's updates where it said them, among its
# calls. Before this, the forwarder cleared the agent's words at every call
# and the report listed the calls alone.
S144="${S}_agentprogress"
WORK="$(work_dir progress)"
STUB_LOG="$SMOKE_TMP/stub144.log"
HELD="$SMOKE_TMP/held144"
GO="$SMOKE_TMP/go144"
STUB_PORT="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
python3 - "$STUB_PORT" "$STUB_LOG" "$HELD" "$GO" <<'PY' &
import json, os, re, sys, threading, time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

port, log, held, go = int(sys.argv[1]), sys.argv[2], sys.argv[3], sys.argv[4]
lock = threading.Lock()

def text(message):
    content = message.get("content")
    if isinstance(content, list):
        return " ".join(part.get("text", "") for part in content if isinstance(part, dict))
    return content or ""

def said(words):
    return [{"choices": [{"delta": {"content": words}}]}]

def finish(reason):
    return [{"choices": [{"delta": {}, "finish_reason": reason}]}]

def calling(name, args, n):
    call = {"index": 0, "id": "call_144_%d" % n, "type": "function",
            "function": {"name": name, "arguments": json.dumps(args)}}
    return [{"choices": [{"delta": {"tool_calls": [call]}}]}] + finish("tool_calls")

class Stub(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def do_GET(self):
        body = b'{"object":"list","data":[{"id":"stub-model"}]}'
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers.get("content-length") or 0)).decode())
        messages = body.get("messages", [])
        # Only the lead is offered `agent` — subagents never nest.
        lead = any((tool.get("function") or {}).get("name") == "agent"
                   for tool in body.get("tools") or [])
        with lock, open(log, "a") as f:
            f.write(json.dumps({"lead": lead, "messages": messages}) + "\n")
        last, n = messages[-1], len(messages)
        self.send_response(200)
        self.send_header("content-type", "text/event-stream")
        self.send_header("connection", "close")
        self.end_headers()
        if not lead:
            # The agent talks, runs a command, talks again, runs another,
            # and answers — one round per request.
            results = sum(1 for m in messages if m.get("role") == "tool")
            if results == 0:
                self.out(said("I'll list the work directory first."))
                # Held mid-reply until the script has read the live row.
                open(held, "w").close()
                deadline = time.time() + 60
                while not os.path.exists(go) and time.time() < deadline:
                    time.sleep(0.05)
                self.out(calling("bash", {"command": "echo listing",
                                          "description": "List the work directory"}, n))
            elif results == 1:
                self.out(said("The directory is empty.\n\nChecking the date next."))
                self.out(calling("bash", {"command": "echo dated",
                                          "description": "Print the date"}, n))
            else:
                self.out(said("Surveyed: the directory is empty."))
                self.out(finish("stop"))
        elif last.get("role") == "user" and "smoke144 survey" in text(last):
            self.out(calling("agent", {"description": "Survey the work directory",
                                       "prompt": "Survey the work directory.",
                                       "run_in_background": False}, n))
        elif last.get("role") == "tool" and "agentsend continues this conversation" in text(last):
            agent = re.search(r"\[agent (\S+) — agentsend continues", text(last)).group(1)
            self.out(calling("agentoutput", {"agent_id": agent}, n))
        else:
            self.out(said("All done."))
            self.out(finish("stop"))
        self.wfile.write(b"data: [DONE]\n\n")
        self.wfile.flush()

    def out(self, frames):
        for frame in frames:
            self.wfile.write(("data: %s\n\n" % json.dumps(frame)).encode())
        self.wfile.flush()

    def log_message(self, *_args):
        pass

ThreadingHTTPServer(("127.0.0.1", port), Stub).serve_forever()
PY
STUB_PID=$!
smoke_on_exit kill "$STUB_PID"
PROVIDERS="$SMOKE_TMP/providers144.toml"
cat >"$PROVIDERS" <<EOF
[providers.stub]
name = "Stub"

[providers.stub.kwargs]
api_base = "http://127.0.0.1:$STUB_PORT/v1"
EOF
# No permission gate: the agent's commands run unasked.
# NO_PROXY keeps a developer's proxy off the loopback.
REAL_APP="env NO_PROXY=127.0.0.1 no_proxy=127.0.0.1 $CFG_ENV_NOHIST ALTER_ZERO_PERMISSIONS=0 ALTER_ZERO_PROVIDERS_FILE=$PROVIDERS ALTER_ZERO_PROVIDER=stub ALTER_ZERO_MODEL=stub-model STUB_API_KEY=stand-in $BIN_ABS"
launch -c "$WORK" "$S144" 120 40 "$REAL_APP"

# ---- the agent talks: the lead's live cell says what it is saying ----
submit "$S144" "smoke144 survey"
poll 30 test -e "$HELD" || fail "the agent's first request never reached the stub"
live="$(wait_pane 20 "$S144" -F "⎿  Agent: I'll list the work directory first.")" ||
	fail "the lead's live agent cell never showed what the agent was saying"
dump "the lead's live cell while the agent talks" "$live"
# Matched without its bullet: a running cell's `●` blinks (docs/tool-pulse.md).
expect_has "$live" -F "Agent(Survey the work directory)" "the lone agent's live cell is missing"
touch "$GO"

# ---- the lead asks agentoutput: the report carries the agent's updates ----
main="$(wait_settled 60 "$S144" -S -300 -- -F 'All done.')" ||
	fail "the lead's turn never ended"
dump "the lead's transcript" "$main"
expect_has "$main" -F "● AgentOutput(Survey the work directory)" \
	"the agentoutput cell is missing or not named by the agent's task"
expect_has "$main" -E "Done \(agent a[0-9a-z]{8} · Survey the work directory · [0-9]+s · 2 tool uses\)" \
	"the report's frame is missing"
expect_has "$main" -F "Agent: I'll list the work directory first." \
	"the AgentOutput cell does not show what the agent said before its first call"
expect_has "$main" -F "Bash(echo listing)" "the AgentOutput cell lost the agent's first call"

python3 - "$STUB_LOG" <<'PY' || fail "the report the lead read is wrong (see above)"
import json, sys

requests = [json.loads(line) for line in open(sys.argv[1]) if line.strip()]

def text(message):
    content = message.get("content")
    if isinstance(content, list):
        return " ".join(part.get("text", "") for part in content if isinstance(part, dict))
    return content or ""

lead = [r["messages"] for r in requests if r["lead"]]
reports = [text(msg) for m in lead for msg in m
           if msg.get("role") == "tool" and text(msg).startswith("Done (agent ")]
if not reports:
    print("FAIL: no lead request carried the agentoutput report")
    sys.exit(1)
report = reports[-1]
print(report)
parts = (
    "Agent: I'll list the work directory first.\n",
    "Bash(echo listing)\n",
    "Agent: The directory is empty.\n  Checking the date next.\n",
    "Bash(echo dated)\n",
    "Final response:\nSurveyed: the directory is empty.",
)
order = [report.find(part) for part in parts]
if -1 in order or order != sorted(order):
    print("FAIL: the report does not list the agent's updates among its calls: %r" % (order,))
    sys.exit(1)
PY
tmux kill-session -t "$S144" 2>/dev/null
note "a subagent's progress updates — its live row, the lead's agentoutput report"
smoke_finish
