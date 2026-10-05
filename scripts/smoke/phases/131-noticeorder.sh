#!/usr/bin/env bash
# Phase 131 — a background notice lands where the model read it, and a call into a session that just ended reports the exit

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# The reported race (docs/background.md *Where a notice lands*,
# docs/bash-tools.md *One notice per exit*), driven through the REAL backend
# against a stub provider whose "model" takes its time:
#
# 1. A `bash` call comes back at a prompt (`Password: `), the stub then spends
#    six seconds writing the `bashsend` that would answer it, and the command
#    finishes by itself meanwhile. The call must report the exit — `Exit
#    code: 0` over the output, the model told nothing was typed — and no
#    `[background] … completed` note may follow it, on the wire or on screen:
#    it used to answer "No running session … its final output was already
#    reported" while that note still waited unread, and the note arrived after.
# 2. A background job finishes while the stub is writing an unrelated call.
#    The model reads the job's notice only at the next round's top, AFTER that
#    call's result — and the transcript, Ctrl+D and the next turn's request
#    must all say so. The notice cell used to settle at the call's ToolStart,
#    in front of the call, so every later context claimed the model had read
#    it first and carried on regardless.
S131="${S}_noticeorder"
WORK="$(work_dir notices)"
STUB_LOG="$SMOKE_TMP/stub131.log"
STUB_PORT="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
python3 - "$STUB_PORT" "$STUB_LOG" <<'PY' &
import json, re, sys, time
from http.server import BaseHTTPRequestHandler, HTTPServer

port, log = int(sys.argv[1]), sys.argv[2]
PROMPTS = ("smoke131 remove", "smoke131 list", "smoke131 next")

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
    call = {"index": 0, "id": "call_131_%d" % n, "type": "function",
            "function": {"name": name, "arguments": json.dumps(args)}}
    return [{"choices": [{"delta": {"tool_calls": [call]}}]},
            {"choices": [{"delta": {}, "finish_reason": "tool_calls"}]}]

class Stub(BaseHTTPRequestHandler):
    def do_GET(self):
        self.send(b'{"object":"list","data":[{"id":"stub-model"}]}', "application/json")

    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("content-length") or 0)).decode()
        with open(log, "a") as f:
            f.write(body.replace("\n", " ") + "\n")
        messages = json.loads(body).get("messages", [])
        last, n = messages[-1], len(messages)
        said = text(last)
        prompt = next((p for m in reversed(messages) if m.get("role") == "user"
                       for p in PROMPTS if p in text(m)), "")
        calls = json.dumps([m.get("tool_calls") for m in messages])
        fresh = last.get("role") == "user" and not said.startswith("[background]")
        if prompt == "smoke131 remove" and fresh:
            frames = calling("bash", {"command": "read -r -t 4 -p 'Password: ' pw; echo removed",
                                      "description": "Remove the package", "wait": 120}, n)
        elif prompt == "smoke131 remove" and "waiting for input" in said:
            session = re.search(r"session (b[0-9a-z]{8})", said).group(1)
            time.sleep(6)  # writing the answer — the command ends meanwhile
            frames = calling("bashsend", {"session_id": session, "input": "hunter2<Enter>"}, n)
        elif prompt == "smoke131 list" and fresh:
            frames = calling("bash", {"command": "sleep 2; echo finished",
                                      "description": "Slow job", "wait": 0}, n)
        elif prompt == "smoke131 list" and "echo hi" not in calls:
            time.sleep(3.5)  # writing the next call — the slow job ends meanwhile
            frames = calling("bash", {"command": "echo hi", "description": "Say hi"}, n)
        else:
            frames = answer("Done.")
        self.send(sse(frames), "text/event-stream")

    def send(self, data, kind):
        self.send_response(200)
        self.send_header("content-type", kind)
        self.send_header("content-length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, *_args):
        pass

HTTPServer(("127.0.0.1", port), Stub).serve_forever()
PY
STUB_PID=$!
smoke_on_exit kill "$STUB_PID"
PROVIDERS="$SMOKE_TMP/providers131.toml"
cat >"$PROVIDERS" <<EOF
[providers.stub]
name = "Stub"

[providers.stub.kwargs]
api_base = "http://127.0.0.1:$STUB_PORT/v1"
EOF
# No permission gate: the race is in the timing, and a prompt would be a
# person in the loop. NO_PROXY keeps a developer's proxy off the loopback.
REAL_APP="env NO_PROXY=127.0.0.1 no_proxy=127.0.0.1 $CFG_ENV_NOHIST ALTER_ZERO_PERMISSIONS=0 ALTER_ZERO_PROVIDERS_FILE=$PROVIDERS ALTER_ZERO_PROVIDER=stub ALTER_ZERO_MODEL=stub-model STUB_API_KEY=stand-in $BIN_ABS"
launch -c "$WORK" "$S131" 120 60 "$REAL_APP"

# ---- 1. a call into a session that ended while the model wrote it ----
submit "$S131" "smoke131 remove"
removed="$(wait_settled 40 "$S131" -S -200 -- -F "Done.")" || fail "the remove turn never settled"
dump "the call into the session that had just ended" "$removed"
expect_lacks "$removed" -F 'Background command "Remove the package"' \
	"a notice for the exit the call reported was committed too"

# ---- 2. a notice that lands while the model writes another call ----
submit "$S131" "smoke131 list"
listed="$(wait_settled 40 "$S131" -S -200 -- -F 'Background command "Slow job" completed')" ||
	fail "the slow job's notice never committed"
dump "the notice after the call the model made without it" "$listed"
order="$(printf '%s\n' "$listed" | grep -nE 'echo hi|Background command "Slow job" completed' | cut -d: -f1 | tr '\n' ' ')"
call_row="$(printf '%s\n' "$listed" | grep -nF 'echo hi' | tail -1 | cut -d: -f1)"
notice_row="$(printf '%s\n' "$listed" | grep -nF 'Background command "Slow job" completed' | tail -1 | cut -d: -f1)"
if [ -z "$call_row" ] || [ -z "$notice_row" ] || [ "$notice_row" -le "$call_row" ]; then
	fail "the notice is not below the call the model made before reading it (rows: $order)"
fi

# Ctrl+D shows the context the next request derives from history: the notice
# after the `echo hi` call and its result, as the wire carried it.
keys "$S131" C-d
context="$(wait_pane 5 "$S131" -F "C O N T E X T")" || fail "Ctrl+D did not open the context view"
dump "Ctrl+D" "$context"
ctx_call="$(printf '%s\n' "$context" | grep -nF '"echo hi"' | tail -1 | cut -d: -f1)"
ctx_notice="$(printf '%s\n' "$context" | grep -nF '[background] Background command "Slow job"' | tail -1 | cut -d: -f1)"
if [ -z "$ctx_call" ] || [ -z "$ctx_notice" ] || [ "$ctx_notice" -le "$ctx_call" ]; then
	fail "Ctrl+D does not show the notice after the call it never informed (call row '$ctx_call', notice row '$ctx_notice')"
fi
keys "$S131" q
wait_for 5 "$S131" -E '^❯' || fail "the context view did not close"

# ---- 3. the next turn replays what the wire carried ----
submit "$S131" "smoke131 next"
wait_file 20 "$STUB_LOG" -F "smoke131 next" || fail "the next turn's request never reached the stub"
wait_settled 20 "$S131" -E "Done\.[[:space:]]*$" >/dev/null || fail "the next turn never settled"

python3 - "$STUB_LOG" <<'PY' || fail "the requests the stub saw are wrong (see above)"
import json, sys

requests = [json.loads(line)["messages"] for line in open(sys.argv[1]) if line.strip()]

def text(message):
    content = message.get("content")
    if isinstance(content, list):
        return " ".join(part.get("text", "") for part in content if isinstance(part, dict))
    return content or ""

def calls(message):
    return json.dumps(message.get("tool_calls") or [])

problems = []
# 1: the bashsend's own result reported the exit, and no note about it went out.
reports = [m[-1] for m in requests if m[-1].get("role") == "tool" and "Nothing was typed" in text(m[-1])]
if not reports:
    problems.append("no bashsend result reported the exit")
elif not (text(reports[0]).startswith("Exit code: 0") and "removed" in text(reports[0])):
    problems.append("the bashsend result is not the exit report: %r" % text(reports[0]))
if any(text(msg).startswith('[background] Background command "Remove the package"')
       for m in requests for msg in m):
    problems.append("the claimed exit's note was sent too")
# 2: the slow job's note reached the model right after the `echo hi` result.
heard = [m for m in requests if text(m[-1]).startswith('[background] Background command "Slow job"')]
if not heard:
    problems.append("the slow job's note never reached the model mid-turn")
else:
    m = heard[0]
    if not (m[-2].get("role") == "tool" and "hi" in text(m[-2]) and "echo hi" in calls(m[-3])):
        problems.append("the note did not follow the echo-hi call's result: %r" % [msg.get("role") for msg in m[-4:]])
# 3: the next turn's request, derived from history, keeps that order.
following = [m for m in requests if "smoke131 next" in text(m[-1])]
if not following:
    problems.append("the next turn's request never arrived")
else:
    m = following[0]
    notice = [i for i, msg in enumerate(m) if text(msg).startswith('[background] Background command "Slow job"')]
    call = [i for i, msg in enumerate(m) if "echo hi" in calls(msg)]
    if not notice or not call or notice[0] < call[0]:
        problems.append("the next turn replays the notice before the call (notice %s, call %s)" % (notice, call))
for problem in problems:
    print("FAIL:", problem)
sys.exit(1 if problems else 0)
PY
tmux kill-session -t "$S131" 2>/dev/null
