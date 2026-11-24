#!/usr/bin/env bash
# Phase 170 — a long turn compacts in the background, its cell committed mid-turn

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# Background compaction (docs/compact.md *Background compaction*): a turn
# that grows past 90% of its window compacts WHILE it runs — the agent takes
# a checkpoint at a round boundary, keeps working while the summary is
# written beside it, and splices it in at a later round boundary — so the
# `● Context compacted … · auto` cell commits in the middle of the turn,
# above the turn's own `… for Ns` summary, and the turn completes.
#
# A stub chat-completions server plays the model through the real backend:
# `smoke170 rounds` makes it run ten bash rounds of bulky prose, then answer;
# a request offering no tools is a summarization, answered with
# `SUMMARY-UP-TO-{n}` (the newest round its snapshot held). Every reply
# reports its prompt as a quarter of its body's bytes — the count the agent
# measures itself by, which prose keeps close to its own estimate. Right
# after a compaction the request ends on the summary's bridge, whose mark
# says how far the rounds got. The window is sized off a first measuring
# turn, so the fixed overhead leaves room for a few rounds whatever it is.
S170="${S}_bgcompact"
WORK="$(work_dir bgcompact)"
LOG="$SMOKE_TMP/bgcompact.log"
PORT="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
python3 - "$PORT" "$LOG" <<'PY' &
import json, re, sys, threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

port, log = int(sys.argv[1]), sys.argv[2]
lock = threading.Lock()
ROUNDS = 10

def text(message):
    content = message.get("content")
    if isinstance(content, list):
        return " ".join(part.get("text", "") for part in content if isinstance(part, dict))
    return content or ""

def marks(messages, mark):
    found = set()
    for message in messages:
        found.update(int(n) for n in re.findall(mark + r"(\d+)", text(message)))
    return sorted(found)

def rounds(messages):
    return marks(messages, "ROUND-")

def said(words):
    return [{"choices": [{"delta": {"content": words}}]},
            {"choices": [{"delta": {}, "finish_reason": "stop"}]}]

def call(n):
    arguments = json.dumps({"command": "echo ROUND-%d; yes 'the quick brown fox jumps over the lazy dog' | head -n 30" % n})
    return [{"choices": [{"delta": {"tool_calls": [{"index": 0, "id": "call_%d" % n,
             "type": "function", "function": {"name": "bash", "arguments": arguments}}]}}]},
            {"choices": [{"delta": {}, "finish_reason": "tool_calls"}]}]

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
        raw = self.rfile.read(int(self.headers.get("content-length") or 0))
        body = json.loads(raw.decode())
        messages = body.get("messages", [])
        tools = bool(body.get("tools"))
        last = messages[-1] if messages else {}
        with lock, open(log, "a") as f:
            f.write(json.dumps({"bytes": len(raw), "tools": tools,
                                "rounds": rounds(messages)}) + "\n")
        if not tools:
            covered = max((rounds(messages) or [0])[-1],
                          (marks(messages, "SUMMARY-UP-TO-") or [0])[-1])
            frames = said("SUMMARY-UP-TO-%d: the rounds printed prose." % covered)
        elif "smoke170 rounds" not in "\n".join(text(m) for m in messages):
            frames = said("Measured.")
        else:
            mark = "ROUND-" if last.get("role") == "tool" else "SUMMARY-UP-TO-"
            done = (marks([last], mark) or [0])[-1]
            frames = call(done + 1) if done < ROUNDS else said("All the rounds are done.")
        frames.append({"choices": [], "usage": {"prompt_tokens": len(raw) // 4,
                                                "completion_tokens": 20}})
        data = ("".join("data: %s\n\n" % json.dumps(f) for f in frames)
                + "data: [DONE]\n\n").encode()
        self.send_response(200)
        self.send_header("content-type", "text/event-stream")
        self.send_header("content-length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, *_args):
        pass

ThreadingHTTPServer(("127.0.0.1", port), Stub).serve_forever()
PY
SERVER=$!
smoke_on_exit "kill $SERVER 2>/dev/null"
for _ in $(seq 1 30); do # wait for the stub to listen
	python3 -c "import socket, sys; s = socket.socket(); s.settimeout(0.2); sys.exit(0 if s.connect_ex(('127.0.0.1', $PORT)) == 0 else 1)" && break
	sleep 0.1
done
PROVIDERS="$SMOKE_TMP/providers170.toml"
cat >"$PROVIDERS" <<EOF
[providers.stub]
name = "Stub"

[providers.stub.kwargs]
api_base = "http://127.0.0.1:$PORT/v1"
EOF
stub_app() { # WINDOW → the app's command line
	printf '%s' "env NO_PROXY=127.0.0.1 no_proxy=127.0.0.1 $CFG_ENV_NOHIST ALTER_ZERO_PERMISSIONS=0 ALTER_ZERO_CONTEXT_WINDOW=$1 ALTER_ZERO_PROVIDERS_FILE=$PROVIDERS ALTER_ZERO_PROVIDER=stub ALTER_ZERO_MODEL=stub-model STUB_API_KEY=stand-in $BIN_ABS"
}

# ---- the window: the fixed overhead, measured ----
launch -c "$WORK" "${S170}_measure" 100 30 "$(stub_app 1000000)"
submit "${S170}_measure" "measure"
wait_for 20 "${S170}_measure" -F "Measured." || fail "the measuring turn never answered"
tmux kill-session -t "${S170}_measure" 2>/dev/null
overhead="$(python3 -c "import json, sys; print(json.loads(open(sys.argv[1]).readline())['bytes'] // 4)" "$LOG")"
window=$((overhead + 2500))
note "the fixed overhead is ~$overhead tokens; the window $window"

# ---- the long turn ----
launch -c "$WORK" "$S170" 100 40 "$(stub_app "$window")"
submit "$S170" "smoke170 rounds"
done_pane="$(wait_pane 90 "$S170" -S -400 -- -F "All the rounds are done.")" ||
	fail "the long turn never finished"
done_pane="$(wait_summaries 20 "$S170" 1 -S -400)" || fail "the long turn never settled"
dump "a long turn that compacted while it ran" "$done_pane"

note "the context compacted mid-turn, above the turn's own summary"
expect_has "$done_pane" -E "Context compacted · .* tokens .*· auto" \
	"no '● Context compacted … · auto' cell was committed"
cells="$(printf '%s\n' "$done_pane" | grep -cF "Context compacted")"
[ "$cells" -ge 1 ] || fail "expected a compaction cell, found $cells"
first_cell="$(printf '%s\n' "$done_pane" | grep -nF "Context compacted" | head -1 | cut -d: -f1)"
summary_row="$(printf '%s\n' "$done_pane" | grep -nE "^$SUMMARY_RE" | tail -1 | cut -d: -f1)"
[ -n "$first_cell" ] && [ -n "$summary_row" ] && [ "$first_cell" -lt "$summary_row" ] ||
	fail "the compaction cell did not land before the turn's summary (cell $first_cell, summary $summary_row)"
last_round_row="$(printf '%s\n' "$done_pane" | grep -nF "ROUND-10" | tail -1 | cut -d: -f1)"
[ -n "$last_round_row" ] && [ "$first_cell" -lt "$last_round_row" ] ||
	fail "the compaction cell did not land before the turn's last rounds (cell $first_cell, round 10 at $last_round_row)"

note "the summarizer ran beside the turn and the agent read its summary"
summaries="$(grep -c '"tools": false' "$LOG")"
[ "$summaries" -ge 1 ] || fail "no summarization request reached the stub"
spliced="$(python3 - "$LOG" <<'PY'
import json, sys
rows = [json.loads(line) for line in open(sys.argv[1]) if line.strip()]
agent = [r for r in rows if r["tools"]]
# A request after the splice no longer holds round 1's output.
print(sum(1 for r in agent if r["rounds"] and 1 not in r["rounds"]))
PY
)"
[ "$spliced" -ge 1 ] || fail "no request after the compaction dropped the summarized rounds"

tmux kill-session -t "$S170" 2>/dev/null
