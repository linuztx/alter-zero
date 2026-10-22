#!/usr/bin/env bash
# Phase 143 — a wait that reports the user's next run still hands the lead the first run's answer

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# docs/agent-tools.md *A report covers one run*, driven through the REAL
# backend against a stub provider. The lead launches a background agent to
# create the frontend and waits on it with `agentoutput`. While the agent
# writes its final reply, the user messages it from its session view; the
# message lands after the run's last drain, so the agent's own thread starts
# the next run the instant this one ends (docs/queue.md). The wait — seeing
# the agent at work again — reports that second run, and the first run's
# end goes out as an ordinary notice. That report used to take the notice
# back unread, so the lead never saw the first answer — the answer to the
# task it gave. Now:
#
# 1. the wait's result carries the second run's answer, then the first's,
#    introduced as the earlier one;
# 2. no lead request carries the first answer as a separate notice, nor the
#    note that the user resumed the agent (the report covers both);
# 3. the main conversation shows neither notice's cell, and Ctrl+D shows the
#    earlier answer inside the report.
#
# The stub holds the agent's first reply until the user's message is queued
# in its session view (a file the script touches), so the message always
# lands after the run's last drain and before its end.
S143="${S}_agentearlier"
WORK="$(work_dir earlier)"
STUB_LOG="$SMOKE_TMP/stub143.log"
HELD="$SMOKE_TMP/held143"
TYPED="$SMOKE_TMP/typed143"
STUB_PORT="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
python3 - "$STUB_PORT" "$STUB_LOG" "$HELD" "$TYPED" <<'PY' &
import json, os, re, sys, threading, time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

port, log, held, typed = int(sys.argv[1]), sys.argv[2], sys.argv[3], sys.argv[4]
lock = threading.Lock()

def sse(frames):
    body = "".join("data: %s\n\n" % json.dumps(f) for f in frames) + "data: [DONE]\n\n"
    return body.encode()

def text(message):
    content = message.get("content")
    if isinstance(content, list):
        return " ".join(part.get("text", "") for part in content if isinstance(part, dict))
    return content or ""

def answer(words):
    return [{"choices": [{"delta": {"content": words}}]},
            {"choices": [{"delta": {}, "finish_reason": "stop"}]}]

def calling(name, args, n):
    call = {"index": 0, "id": "call_143_%d" % n, "type": "function",
            "function": {"name": name, "arguments": json.dumps(args)}}
    return [{"choices": [{"delta": {"tool_calls": [call]}}]},
            {"choices": [{"delta": {}, "finish_reason": "tool_calls"}]}]

class Stub(BaseHTTPRequestHandler):
    def do_GET(self):
        self.send(200, b'{"object":"list","data":[{"id":"stub-model"}]}', "application/json")

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers.get("content-length") or 0)).decode())
        messages = body.get("messages", [])
        # Only the lead is offered `agent` — subagents never nest.
        lead = any((tool.get("function") or {}).get("name") == "agent"
                   for tool in body.get("tools") or [])
        with lock, open(log, "a") as f:
            f.write(json.dumps({"lead": lead, "messages": messages}) + "\n")
        last, n = messages[-1], len(messages)
        if not lead:
            if any("also add tests" in text(m) for m in messages):
                return self.send(200, sse(answer("Added tests in tests/.")), "text/event-stream")
            # Its first and last round: held until the user's message is
            # queued behind it.
            open(held, "w").close()
            deadline = time.time() + 60
            while not os.path.exists(typed) and time.time() < deadline:
                time.sleep(0.05)
            return self.send(200, sse(answer("Frontend created with React.")), "text/event-stream")
        if last.get("role") == "user" and "smoke143 build" in text(last):
            frames = calling("agent", {"description": "Create the frontend",
                                       "prompt": "Write the frontend."}, n)
        elif last.get("role") == "tool" and "launched as" in text(last):
            agent = re.search(r"launched as (\S+) and working", text(last)).group(1)
            frames = calling("agentoutput", {"agent_id": agent, "wait": 120}, n)
        else:
            frames = answer("All done.")
        self.send(200, sse(frames), "text/event-stream")

    def send(self, status, data, kind):
        self.send_response(status)
        self.send_header("content-type", kind)
        self.send_header("content-length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, *_args):
        pass

ThreadingHTTPServer(("127.0.0.1", port), Stub).serve_forever()
PY
STUB_PID=$!
smoke_on_exit kill "$STUB_PID"
PROVIDERS="$SMOKE_TMP/providers143.toml"
cat >"$PROVIDERS" <<EOF
[providers.stub]
name = "Stub"

[providers.stub.kwargs]
api_base = "http://127.0.0.1:$STUB_PORT/v1"
EOF
# No permission gate: nothing here should wait on a person.
# NO_PROXY keeps a developer's proxy off the loopback.
REAL_APP="env NO_PROXY=127.0.0.1 no_proxy=127.0.0.1 $CFG_ENV_NOHIST ALTER_ZERO_PERMISSIONS=0 ALTER_ZERO_PROVIDERS_FILE=$PROVIDERS ALTER_ZERO_PROVIDER=stub ALTER_ZERO_MODEL=stub-model STUB_API_KEY=stand-in $BIN_ABS"
launch -c "$WORK" "$S143" 120 60 "$REAL_APP"

# ---- the lead launches the agent and waits on it ----
submit "$S143" "smoke143 build"
waited=0
until [ -e "$HELD" ]; do
	[ "$waited" -ge 300 ] && fail "the agent's first reply never reached the stub"
	sleep 0.1
	waited=$((waited + 1))
done
launched="$(wait_pane 20 "$S143" -S -200 -- -F 'AgentOutput')" ||
	fail "the lead never started waiting on the agent"
dump "the lead waits on its agent" "$launched"

# ---- the user messages the agent while it writes its last reply ----
# ↓ opens the roster on `● main`, a second ↓ steps onto the agent, Enter
# opens its session view (Phase 96's walk).
keys "$S143" Down
sleep 0.15
keys "$S143" Down
sleep 0.15
keys "$S143" Enter
wait_for 10 "$S143" -E '^❯ Write the frontend\.' ||
	fail "the agent's session view did not open"
submit "$S143" "also add tests"
queued="$(wait_pane 10 "$S143" -S -200 -- -F '  ❯ also add tests')" ||
	fail "the message never showed queued behind the agent's running reply"
dump "the message waits behind the agent's last reply" "$queued"
touch "$TYPED"
view="$(wait_pane 30 "$S143" -S -300 -- -F 'Added tests in tests/.')" ||
	fail "the agent never ran the user's message as its next run"
dump "the agent's own session" "$view"

# ---- back to the main conversation ----
keys "$S143" Escape
main="$(wait_settled 40 "$S143" -S -300 -- -F 'All done.')" ||
	fail "the lead's turn never ended"
dump "the main conversation" "$main"
expect_lacks "$main" -F 'Agent "Create the frontend" finished' \
	"the first run's answer committed a notice cell of its own, after the report that carries it"
expect_lacks "$main" -F 'was resumed by user' \
	"the resume note committed a cell though the report covered the run it announced"

# Ctrl+D: the lead's own context has the earlier answer inside the report.
keys "$S143" C-d
wait_for 5 "$S143" -F "C O N T E X T" || fail "Ctrl+D did not open the context view"
context="$(wait_pane 5 "$S143" -F "Before this run it had ended an earlier one")" ||
	fail "Ctrl+D does not show the earlier answer the report carries"
dump "Ctrl+D" "$context"
keys "$S143" q
wait_for 5 "$S143" -E '^❯' || fail "the context view did not close"

python3 - "$STUB_LOG" <<'PY' || fail "the requests the stub saw are wrong (see above)"
import json, sys

requests = [json.loads(line) for line in open(sys.argv[1]) if line.strip()]

def text(message):
    content = message.get("content")
    if isinstance(content, list):
        return " ".join(part.get("text", "") for part in content if isinstance(part, dict))
    return content or ""

problems = []
lead = [r["messages"] for r in requests if r["lead"]]
reports = [msg for m in lead for msg in m
           if msg.get("role") == "tool" and "Final response:\nAdded tests in tests/." in text(msg)]
if not reports:
    problems.append("no lead request carried the wait's report of the second run")
else:
    report = text(reports[-1])
    order = [report.find(part) for part in (
        "Final response:\nAdded tests in tests/.",
        "[Before this run it had ended an earlier one, whose notice had not reached you:]",
        "Final response:\nFrontend created with React.",
    )]
    if -1 in order or order != sorted(order):
        problems.append("the report does not carry the first answer after its own: %r" % report)
for i, messages in enumerate(lead):
    for msg in messages:
        if msg.get("role") == "user" and "[background agent]" in text(msg):
            problems.append("lead request %d carried a notice the report covered: %r" % (i, text(msg)))
for problem in problems:
    print("FAIL:", problem)
sys.exit(1 if problems else 0)
PY
tmux kill-session -t "$S143" 2>/dev/null
note "a wait that reports the user's next run still hands the lead the first run's answer"
