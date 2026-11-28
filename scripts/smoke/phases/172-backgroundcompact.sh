#!/usr/bin/env bash
# Phase 172 — background compaction: a summary written while the turn runs, waited for at the ceiling, landed at idle, discarded by /clear

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# Background compaction (docs/background-compaction.md), driven through the
# REAL backend against a stub provider whose usage frames set the measure:
# a 100k window puts the ceiling at 84k and a cold cycle's start at 42k.
#
# 1. `smoke172 wait`: the rounds report 30k, 45k, 60k, 90k. The summary starts
#    at the third round top and takes the stub six seconds, so the ceiling at
#    the fifth comes first: the status line reads `Compacting…` while the turn
#    waits, then the summary is swapped in — its `· background` cell committed
#    mid-turn, under the rounds and above the reply — and the request that
#    follows carries it with the rounds since it started kept as they were.
# 2. `smoke172 idle`: 40k, 50k, then the reply. The summary starts at the
#    third round top and the turn ends while it is written: it lands on its
#    own once it is done, no key pressed — the summarizer wakes the loop.
# 3. `smoke172 clear`: the same, and a `/clear` while the summary is written
#    discards it, so nothing lands on the cleared screen.
S172="${S}_bgcompact"
S172b="${S}_bgcompact_idle"
STUB_LOG="$SMOKE_TMP/stub172.log"
STUB_PORT="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
python3 - "$STUB_PORT" "$STUB_LOG" <<'PY' &
import json, re, sys, time
from http.server import BaseHTTPRequestHandler, HTTPServer
from socketserver import ThreadingMixIn

port, log = int(sys.argv[1]), sys.argv[2]
# Each round's usage, by scenario: the measure the round tops decide with.
USAGE = {"wait": [30000, 45000, 60000, 90000], "idle": [40000, 50000], "clear": [40000, 50000]}

def text(message):
    content = message.get("content")
    if isinstance(content, list):
        return " ".join(part.get("text", "") for part in content if isinstance(part, dict))
    return content or ""

def scenario(messages):
    for message in reversed(messages):
        found = re.findall(r"smoke172 (wait|idle|clear)", text(message))
        if message.get("role") == "user" and found:
            return found[-1]
    return ""

def sse(frames, prompt_tokens):
    frames = frames + [{"choices": [], "usage": {"prompt_tokens": prompt_tokens, "completion_tokens": 20}}]
    return ("".join("data: %s\n\n" % json.dumps(f) for f in frames) + "data: [DONE]\n\n").encode()

def answer(words):
    return [{"choices": [{"delta": {"content": words}}]},
            {"choices": [{"delta": {}, "finish_reason": "stop"}]}]

class Stub(BaseHTTPRequestHandler):
    def do_GET(self):
        self.send(b'{"object":"list","data":[{"id":"stub-model"}]}', "application/json")

    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("content-length") or 0)).decode()
        request = json.loads(body)
        messages = request.get("messages", [])
        which = scenario(messages)
        summary = not request.get("tools")
        with open(log, "a") as f:
            f.write("%s\t%s\t%s\n" % ("summary" if summary else "round", which, body.replace("\n", " ")))
        if summary:
            time.sleep(6)  # written while the turn goes on
            self.send(sse(answer("SUMMARY-%s: the user grew the logs." % which), 50000), "text/event-stream")
            return
        rounds = USAGE.get(which, [])
        done = sum(1 for m in messages if m.get("role") == "tool")
        if any(("SUMMARY-%s" % which) in text(m) for m in messages) or done >= len(rounds):
            self.send(sse(answer("Done %s." % which), 52000), "text/event-stream")
            return
        call = {"index": 0, "id": "call_172_%s_%d" % (which, done), "type": "function",
                "function": {"name": "bash", "arguments": json.dumps({"command": "echo round %d" % (done + 1)})}}
        frames = [{"choices": [{"delta": {"tool_calls": [call]}}]},
                  {"choices": [{"delta": {}, "finish_reason": "tool_calls"}]}]
        self.send(sse(frames, rounds[done]), "text/event-stream")

    def send(self, data, kind):
        try:
            self.send_response(200)
            self.send_header("content-type", kind)
            self.send_header("content-length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)
        except (BrokenPipeError, ConnectionResetError):
            pass  # a summary the app discarded hung up on it

    def log_message(self, *_args):
        pass

class Server(ThreadingMixIn, HTTPServer):
    daemon_threads = True

Server(("127.0.0.1", port), Stub).serve_forever()
PY
STUB_PID=$!
smoke_on_exit kill "$STUB_PID"
PROVIDERS="$SMOKE_TMP/providers172.toml"
cat >"$PROVIDERS" <<EOF
[providers.stub]
name = "Stub"

[providers.stub.kwargs]
api_base = "http://127.0.0.1:$STUB_PORT/v1"
EOF
REAL_APP="env NO_PROXY=127.0.0.1 no_proxy=127.0.0.1 $CFG_ENV_NOHIST ALTER_ZERO_PERMISSIONS=0 ALTER_ZERO_CONTEXT_WINDOW=100000 ALTER_ZERO_PROVIDERS_FILE=$PROVIDERS ALTER_ZERO_PROVIDER=stub ALTER_ZERO_MODEL=stub-model STUB_API_KEY=stand-in $BIN_ABS"

# ---- 1. the ceiling waits for the summary, which is swapped in mid-turn ----
launch -c "$(work_dir wait)" "$S172" 120 50 "$REAL_APP"
submit "$S172" "smoke172 wait"
waiting="$(wait_pane 20 "$S172" -F "Compacting…")" || fail "the turn never waited at the ceiling with 'Compacting…'"
dump "the turn waiting on its summary at the ceiling" "$waiting"
swapped="$(wait_settled 30 "$S172" -S -200 -- -F "Done wait.")" || fail "the turn never finished after its summary was swapped in"
dump "the summary swapped in mid-turn" "$swapped"
expect_has "$swapped" -E "Context compacted · [0-9.]+k → [0-9.]+k? tokens · [0-9]+s · background" \
	"no '· background' compaction cell"
cell_row="$(printf '%s\n' "$swapped" | grep -nF "Context compacted" | tail -1 | cut -d: -f1)"
reply_row="$(printf '%s\n' "$swapped" | grep -nF "Done wait." | tail -1 | cut -d: -f1)"
round_row="$(printf '%s\n' "$swapped" | grep -nF "echo round 4" | tail -1 | cut -d: -f1)"
if [ -z "$cell_row" ] || [ -z "$reply_row" ] || [ -z "$round_row" ] ||
	[ "$cell_row" -le "$round_row" ] || [ "$cell_row" -ge "$reply_row" ]; then
	fail "the compaction cell is not between the last round and the reply (round $round_row, cell $cell_row, reply $reply_row)"
fi
tmux kill-session -t "$S172" 2>/dev/null

python3 - "$STUB_LOG" <<'PY' || fail "the requests the stub saw are wrong (see above)"
import json, sys

lines = [line.rstrip("\n").split("\t", 2) for line in open(sys.argv[1]) if line.strip()]
wait = [(kind, json.loads(body)["messages"]) for kind, which, body in lines if which == "wait"]
kinds = [kind for kind, _ in wait]

def text(message):
    content = message.get("content")
    if isinstance(content, list):
        return " ".join(part.get("text", "") for part in content if isinstance(part, dict))
    return content or ""

problems = []
if kinds.count("summary") != 1:
    problems.append("not one summary request: %s" % kinds)
else:
    asked = kinds.index("summary")
    after = kinds[asked + 1:]
    if not after or after.count("round") < 2:
        problems.append("the turn did not go on while the summary was written: %s" % kinds)
    summary = wait[asked][1]
    if summary[0].get("role") != "system" or "Alter Zero" in text(summary[0]):
        problems.append("the summarizer read the session's system prompt")
    if any("system-reminder" in text(m) for m in summary):
        problems.append("the summarizer read the <system-reminder>")
swapped = [m for kind, m in wait if kind == "round" and any("SUMMARY-wait" in text(x) for x in m)]
if not swapped:
    problems.append("no request carries the summary")
else:
    m = swapped[0]
    bridge = next(i for i, x in enumerate(m) if "SUMMARY-wait" in text(x))
    kept = [x for x in m[bridge + 1:] if x.get("role") == "tool"]
    if not kept:
        problems.append("the rounds since it started were not kept after the summary: %s" % [x.get("role") for x in m])
for problem in problems:
    print("FAIL:", problem)
sys.exit(1 if problems else 0)
PY

# ---- 2. a summary that outlives its turn lands on its own ----
launch -c "$(work_dir idle)" "$S172b" 120 50 "$REAL_APP"
submit "$S172b" "smoke172 idle"
ended="$(wait_settled 20 "$S172b" -S -200 -- -F "Done idle.")" || fail "the idle turn never finished"
dump "the turn ended while its summary is still being written" "$ended"
expect_lacks "$ended" -F "Context compacted" "the summary landed before it was written"
landed="$(wait_pane 20 "$S172b" -S -200 -- -F "Context compacted")" || fail "the summary never landed at idle"
dump "the summary landed at idle, no key pressed" "$landed"
expect_has "$landed" -E "Context compacted · [0-9.]+k → [0-9.]+k? tokens · [0-9]+s · background" \
	"the idle landing is not a '· background' compaction cell"

# ---- 3. /clear discards a summary still being written ----
submit "$S172b" "/clear"
wait_settled 5 "$S172b" -E '^❯' >/dev/null || fail "/clear never settled"
submit "$S172b" "smoke172 clear"
wait_settled 20 "$S172b" -S -200 -- -F "Done clear." >/dev/null || fail "the clear turn never finished"
wait_file 10 "$STUB_LOG" -E "^summary	clear	" || fail "the clear turn never started a summary"
submit "$S172b" "/clear"
wait_settled 5 "$S172b" -E '^❯' >/dev/null || fail "the second /clear never settled"
sleep 8 # past when the discarded summary would have landed
cleared="$(pane "$S172b" -S -200)"
dump "the cleared screen after the discarded summary's time" "$cleared"
expect_lacks "$cleared" -F "Context compacted" "a discarded summary landed on the cleared screen"
tmux kill-session -t "$S172b" 2>/dev/null
