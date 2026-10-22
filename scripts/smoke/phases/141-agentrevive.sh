#!/usr/bin/env bash
# Phase 141 — a failed subagent the user messages back to work is news the lead hears at once

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# The reported bug (docs/agent-tools.md *A resume is news when it starts*),
# driven through the REAL backend against a stub provider. The lead launches
# a foreground subagent to create the frontend; the subagent's request is
# refused, so the lead reads `[agent failed: …]` and goes on with its turn.
# The user opens the failed agent's session view and tells it to continue —
# and it does. The lead used to hear nothing of that until the new run ENDED,
# so for as long as it took the lead went on believing the agent had failed
# (and could start doing its work itself). Now the revive is told to the
# lead at once:
#
# 1. its turn, still in flight, reads the note at the very next round
#    boundary — right after the call it was making when the user spoke —
#    naming the failure, quoting the user's message and saying it came from
#    the user, not the lead;
# 2. the main conversation shows `● Agent "Create the frontend" was
#    resumed by user` where the lead read it, below that call's cell;
# 3. Ctrl+D carries the note, and the new run's own completion still
#    arrives as its notice when it ends.
#
# The stub holds the lead's post-failure round until the revived agent's
# request reaches it, so the revive always lands while the lead's turn runs.
S141="${S}_agentrevive"
WORK="$(work_dir revive)"
STUB_LOG="$SMOKE_TMP/stub141.log"
STUB_PORT="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
python3 - "$STUB_PORT" "$STUB_LOG" <<'PY' &
import json, sys, threading, time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

port, log = int(sys.argv[1]), sys.argv[2]
lock = threading.Lock()
revived = threading.Event()

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
    call = {"index": 0, "id": "call_141_%d" % n, "type": "function",
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
                time.sleep(0.5)
                return self.send(200, sse(answer("Frontend created with React.")), "text/event-stream")
            return self.send(400, b'{"error":{"message":"smoke141 refused the request"}}',
                             "application/json")
        if last.get("role") == "user" and "smoke141 build" in text(last):
            frames = calling("agent", {"description": "Create the frontend",
                                       "prompt": "Write the frontend.",
                                       "run_in_background": False}, n)
        elif last.get("role") == "tool" and "[agent failed:" in text(last):
            # The lead goes on with its turn — and is still at it when the
            # user messages the failed agent.
            revived.wait(60)
            time.sleep(1.0)
            frames = calling("bash", {"command": "echo checked",
                                      "description": "Check the tree"}, n)
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
PROVIDERS="$SMOKE_TMP/providers141.toml"
cat >"$PROVIDERS" <<EOF
[providers.stub]
name = "Stub"

[providers.stub.kwargs]
api_base = "http://127.0.0.1:$STUB_PORT/v1"
EOF
# No permission gate: the lead's `echo` would otherwise wait on a person.
# NO_PROXY keeps a developer's proxy off the loopback.
REAL_APP="env NO_PROXY=127.0.0.1 no_proxy=127.0.0.1 $CFG_ENV_NOHIST ALTER_ZERO_PERMISSIONS=0 ALTER_ZERO_PROVIDERS_FILE=$PROVIDERS ALTER_ZERO_PROVIDER=stub ALTER_ZERO_MODEL=stub-model STUB_API_KEY=stand-in $BIN_ABS"
launch -c "$WORK" "$S141" 120 60 "$REAL_APP"

# ---- the subagent fails, and the lead goes on ----
submit "$S141" "smoke141 build"
failed="$(wait_pane 30 "$S141" -S -200 -- -E 'Failed')" ||
	fail "the foreground subagent never resolved as failed"
dump "the lead read the failure and went on" "$failed"

# ---- the user messages the failed agent back to work ----
# ↓ opens the roster on `● main`, a second ↓ steps onto the agent, Enter
# opens its session view (Phase 96's walk).
keys "$S141" Down
sleep 0.15
keys "$S141" Down
sleep 0.15
keys "$S141" Enter
wait_for 10 "$S141" -E '^❯ Write the frontend\.' ||
	fail "the failed agent's session view did not open"
submit "$S141" "continue with React"
view="$(wait_pane 30 "$S141" -S -200 -- -F "Frontend created with React.")" ||
	fail "the revived agent never answered in its session view"
dump "the revived agent's own session" "$view"

# ---- back to the main conversation ----
keys "$S141" Escape
main="$(wait_settled 40 "$S141" -S -300 -- -F 'Agent "Create the frontend" finished')" ||
	fail "the revived run's completion notice never committed"
dump "the main conversation" "$main"
expect_has "$main" -F '● Agent "Create the frontend" was resumed by user' \
	"the main conversation shows no resume cell"
call_row="$(printf '%s\n' "$main" | grep -nF 'echo checked' | head -1 | cut -d: -f1)"
resume_row="$(printf '%s\n' "$main" | grep -nF 'was resumed by user' | head -1 | cut -d: -f1)"
if [ -z "$call_row" ] || [ -z "$resume_row" ] || [ "$resume_row" -le "$call_row" ]; then
	fail "the resume cell is not below the call the lead made before reading it (call row '$call_row', resume row '$resume_row')"
fi
expect_lacks "$main" -F "continue with React" \
	"the user's message to the agent was rendered in the main conversation (the note is context, not a cell)"

# Ctrl+D: the lead's own context carries the note (the view opens on its
# tail, where the note sits).
keys "$S141" C-d
wait_for 5 "$S141" -F "C O N T E X T" || fail "Ctrl+D did not open the context view"
context="$(wait_pane 5 "$S141" -F "is running again")" ||
	fail "Ctrl+D does not show the resume note in the lead's context"
dump "Ctrl+D" "$context"
keys "$S141" q
wait_for 5 "$S141" -E '^❯' || fail "the context view did not close"

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
first = next((m for m in lead if any("is running again" in text(msg) for msg in m)), None)
if first is None:
    problems.append("no lead request ever carried the resume note")
else:
    note_at = next(i for i, msg in enumerate(first) if "is running again" in text(msg))
    note = first[note_at]
    for needed in ('[background agent] Agent "Create the frontend"', "after it failed",
                   "these came from the user, not from you", "- continue with React"):
        if needed not in text(note):
            problems.append("the resume note lacks %r: %r" % (needed, text(note)))
    if note.get("role") != "user":
        problems.append("the resume note is not a user-role message: %r" % note.get("role"))
    # Heard mid-turn: right after the result of the call the lead was making
    # when the user spoke, in the turn that had read the failure.
    checked = [i for i, msg in enumerate(first) if msg.get("role") == "tool" and "checked" in text(msg)]
    failed = [i for i, msg in enumerate(first) if msg.get("role") == "tool" and "[agent failed:" in text(msg)]
    if not checked or note_at != checked[-1] + 1:
        problems.append("the note did not follow the echo call's result (note %d, results %s)" % (note_at, checked))
    if not failed or failed[-1] > note_at:
        problems.append("the note did not come after the failure the lead read (note %d, failure %s)" % (note_at, failed))
    if any("is running again" in text(msg) for m in lead[:lead.index(first)] for msg in m):
        problems.append("an earlier request carried the note")
for problem in problems:
    print("FAIL:", problem)
sys.exit(1 if problems else 0)
PY
tmux kill-session -t "$S141" 2>/dev/null
note "a failed subagent the user messages back to work is news the lead hears at once"
