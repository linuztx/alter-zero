#!/usr/bin/env bash
# Phase 142 — the lead's agentkill takes back the note that the user resumed the run

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# docs/agent-tools.md *A resume is news when it starts*: the note that a run
# is going is news only until the lead hears that run end. Phase 141's case,
# driven through the REAL backend against a stub provider, with one change:
# the round the lead is in when the user brings the failed agent back answers
# `agentkill` on it — before the lead has read the note. The kill's report is
# the run's end, and no notice follows a stop the lead made, so the note —
# "running again, you will be notified" — used to reach the lead right after
# the kill's result, contradicting it. Now the kill takes it back:
#
# 1. the lead's request after the kill carries the kill's report, which
#    names the user's message, and no request ever carries the note;
# 2. the main conversation shows no `was resumed by user` cell — a note
#    nobody read owes none — and Ctrl+D carries no note.
#
# The stub holds the lead's post-failure round until the revived agent's
# request reaches it, and holds that request until the lead's request after
# the kill, so the kill always lands on the revived run while it is live.
S142="${S}_agentkillresume"
WORK="$(work_dir killresume)"
STUB_LOG="$SMOKE_TMP/stub142.log"
STUB_PORT="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
python3 - "$STUB_PORT" "$STUB_LOG" <<'PY' &
import json, re, sys, threading, time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

port, log = int(sys.argv[1]), sys.argv[2]
lock = threading.Lock()
revived = threading.Event()
killed = threading.Event()

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
    call = {"index": 0, "id": "call_142_%d" % n, "type": "function",
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
            if any("continue with React" in text(m) for m in messages):
                revived.set()
                # Still at work when the lead's kill lands.
                killed.wait(60)
                return self.send(200, sse(answer("Frontend created with React.")), "text/event-stream")
            return self.send(400, b'{"error":{"message":"smoke142 refused the request"}}',
                             "application/json")
        if last.get("role") == "user" and "smoke142 build" in text(last):
            frames = calling("agent", {"description": "Create the frontend",
                                       "prompt": "Write the frontend.",
                                       "run_in_background": False}, n)
        elif last.get("role") == "tool" and "[agent failed:" in text(last):
            # The lead goes on with its turn, and stops the agent the user is
            # bringing back before it has read the note saying so.
            agent = re.search(r"\[agent (\S+) — agentsend", text(last)).group(1)
            revived.wait(60)
            time.sleep(1.0)
            frames = calling("agentkill", {"agent_id": agent}, n)
        else:
            killed.set()
            frames = answer("All done.")
        self.send(200, sse(frames), "text/event-stream")

    def send(self, status, data, kind):
        try:
            self.send_response(status)
            self.send_header("content-type", kind)
            self.send_header("content-length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)
        except (BrokenPipeError, ConnectionResetError):
            pass  # the stopped agent's run let go of its request

    def log_message(self, *_args):
        pass

ThreadingHTTPServer(("127.0.0.1", port), Stub).serve_forever()
PY
STUB_PID=$!
smoke_on_exit kill "$STUB_PID"
PROVIDERS="$SMOKE_TMP/providers142.toml"
cat >"$PROVIDERS" <<EOF
[providers.stub]
name = "Stub"

[providers.stub.kwargs]
api_base = "http://127.0.0.1:$STUB_PORT/v1"
EOF
# No permission gate: nothing here should wait on a person.
# NO_PROXY keeps a developer's proxy off the loopback.
REAL_APP="env NO_PROXY=127.0.0.1 no_proxy=127.0.0.1 $CFG_ENV_NOHIST ALTER_ZERO_PERMISSIONS=0 ALTER_ZERO_PROVIDERS_FILE=$PROVIDERS ALTER_ZERO_PROVIDER=stub ALTER_ZERO_MODEL=stub-model STUB_API_KEY=stand-in $BIN_ABS"
launch -c "$WORK" "$S142" 120 60 "$REAL_APP"

# ---- the subagent fails, and the lead goes on ----
submit "$S142" "smoke142 build"
failed="$(wait_pane 30 "$S142" -S -200 -- -E 'Failed')" ||
	fail "the foreground subagent never resolved as failed"
dump "the lead read the failure and went on" "$failed"

# ---- the user messages the failed agent back to work ----
# ↓ opens the roster on `● main`, a second ↓ steps onto the agent, Enter
# opens its session view (Phase 96's walk).
keys "$S142" Down
sleep 0.15
keys "$S142" Down
sleep 0.15
keys "$S142" Enter
wait_for 10 "$S142" -E '^❯ Write the frontend\.' ||
	fail "the failed agent's session view did not open"
submit "$S142" "continue with React"
view="$(wait_pane 10 "$S142" -S -200 -- -E '^❯ continue with React')" ||
	fail "the message never started the agent again"
dump "the revived agent's own session" "$view"

# ---- back to the main conversation, where the lead stops it ----
keys "$S142" Escape
main="$(wait_settled 40 "$S142" -S -300 -- -F 'All done.')" ||
	fail "the lead's turn never ended"
dump "the main conversation" "$main"
expect_has "$main" -F 'AgentKill' "the lead's stop has no cell"
expect_lacks "$main" -F 'was resumed by user' \
	"a resume cell committed for a note the lead's stop took back unread"

# Ctrl+D: the lead's context has the stop's report and no note.
keys "$S142" C-d
wait_for 5 "$S142" -F "C O N T E X T" || fail "Ctrl+D did not open the context view"
context="$(wait_pane 5 "$S142" -F "Stopped (agent")" ||
	fail "Ctrl+D does not show the stop's report"
dump "Ctrl+D" "$context"
expect_lacks "$context" -F "is running again" "Ctrl+D shows the note the stop took back"
keys "$S142" q
wait_for 5 "$S142" -E '^❯' || fail "the context view did not close"

python3 - "$STUB_LOG" <<'PY' || fail "the requests the stub saw are wrong (see above)"
import json, sys

requests = [json.loads(line) for line in open(sys.argv[1]) if line.strip()]

def text(message):
    content = message.get("content")
    if isinstance(content, list):
        return " ".join(part.get("text", "") for part in content if isinstance(part, dict))
    return content or ""

def stop_report(msg):
    return msg.get("role") == "tool" and "Stopped (agent" in text(msg)

problems = []
lead = [r["messages"] for r in requests if r["lead"]]
after = next((m for m in lead if any(stop_report(msg) for msg in m)), None)
if after is None:
    problems.append("no lead request carried the kill's report")
else:
    report = next(text(msg) for msg in after if stop_report(msg))
    if "— message from the user: continue with React —" not in report:
        problems.append("the kill's report does not name the user's message: %r" % report)
for i, messages in enumerate(lead):
    for msg in messages:
        if "is running again" in text(msg):
            problems.append("lead request %d carried the note the stop took back: %r" % (i, text(msg)))
for problem in problems:
    print("FAIL:", problem)
sys.exit(1 if problems else 0)
PY
tmux kill-session -t "$S142" 2>/dev/null
note "the lead's agentkill takes back the note that the user resumed the run"
