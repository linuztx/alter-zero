#!/usr/bin/env bash
# Phase 171 — a turn several context windows long compacts as it runs and is never refused for its size

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# The reported bug (docs/compact.md *Compacting while the agent works*): auto
# compaction ran only between turns, so one long tool-calling turn grew past
# its model's window and died there. A stub chat-completions server plays a
# model whose window is small and ENFORCED through the real backend:
#
# - an agent request larger than the window is refused with a provider's own
#   words (`context_length_exceeded`) — the phase fails if that ever happens;
# - an agent request is answered with one more `read` of a 4.8 kB file, 34
#   times over — about fifty thousand tokens of reads against a window of
#   thirty thousand — and then with a closing line;
# - a request offering no tools is a summarization: answered `SUMMARY n`,
#   after a second and a half, long enough that the agent has to keep working
#   meanwhile for the turn to finish as the log shows it.
#
# What must hold: the turn reaches its closing line; nothing was refused;
# more than one summary was written, each from a transcript rather than the
# conversation's own messages; the agent was served at least one request
# WHILE a summary was being written; and each `● Context compacted` cell sits
# among the turn's tool cells, where the swap happened.
S171="${S}_compactwindow"
WORK="$(work_dir compactwindow)"
LOG="$SMOKE_TMP/compactwindow.log"
python3 -c 'print("the quick brown fox jumps over the lazy dog and runs on\n" * 86, end="")' >"$WORK/notes.txt"
PORT="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
python3 - "$PORT" "$LOG" "$WORK/notes.txt" <<'PY' &
import json, sys, threading, time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

port, log, path = int(sys.argv[1]), sys.argv[2], sys.argv[3]
WINDOW = 30000          # tokens — what ALTER_ZERO_CONTEXT_WINDOW tells the session
BYTES_PER_TOKEN = 4     # how this stand-in counts them
ROUNDS = 34
SUMMARY_DELAY = 1.5
lock = threading.Lock()
state = {"asked": 0, "summaries": 0}

def record(row):
    with lock, open(log, "a") as f:
        f.write(json.dumps(row) + "\n")

def text(message):
    content = message.get("content")
    if isinstance(content, list):
        return " ".join(part.get("text", "") for part in content if isinstance(part, dict))
    return content or ""

def said(words):
    return {"choices": [{"delta": {"content": words}}]}

def finish(reason):
    return {"choices": [{"delta": {}, "finish_reason": reason}]}

def read_call(number):
    return {"choices": [{"delta": {"tool_calls": [{
        "index": 0, "id": "call_%d" % number, "type": "function",
        "function": {"name": "read", "arguments": json.dumps({"path": path})},
    }]}}]}

class Stub(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def answer(self, status, kind, data):
        self.send_response(status)
        self.send_header("content-type", kind)
        self.send_header("content-length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        self.answer(200, "application/json", b'{"object":"list","data":[{"id":"stub-model"}]}')

    def do_POST(self):
        raw = self.rfile.read(int(self.headers.get("content-length") or 0))
        body = json.loads(raw.decode())
        tokens = len(raw) // BYTES_PER_TOKEN
        began = time.time()
        messages = body.get("messages", [])
        if not body.get("tools"):
            # A summarization: the one kind of request that offers no tools.
            time.sleep(SUMMARY_DELAY)
            with lock:
                state["summaries"] += 1
                number, asked = state["summaries"], state["asked"]
            frames = [said("SUMMARY %d: notes.txt read %d times so far; keep going." % (number, asked)),
                      finish("stop")]
            record({"kind": "summary", "tokens": tokens, "began": began, "ended": time.time(),
                    "transcript": "<conversation>" in (text(messages[-1]) if messages else ""),
                    "roles": sorted({m.get("role", "") for m in messages})})
        elif tokens > WINDOW:
            record({"kind": "refused", "tokens": tokens, "at": began})
            self.answer(400, "application/json", json.dumps({"error": {
                "message": "This model's maximum context length is %d tokens. "
                           "Please reduce the length of the messages." % WINDOW,
                "code": "context_length_exceeded"}}).encode())
            return
        else:
            with lock:
                state["asked"] += 1
                number = state["asked"]
            record({"kind": "agent", "tokens": tokens, "at": began, "round": number})
            if number > ROUNDS:
                frames = [said("Every read is done. DONE-171"), finish("stop")]
            else:
                frames = [read_call(number), finish("tool_calls")]
        frames.append({"choices": [], "usage": {"prompt_tokens": tokens, "completion_tokens": 8}})
        self.answer(200, "text/event-stream",
                    ("".join("data: %s\n\n" % json.dumps(f) for f in frames) + "data: [DONE]\n\n").encode())

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
PROVIDERS="$SMOKE_TMP/providers171.toml"
cat >"$PROVIDERS" <<EOF
[providers.stub]
name = "Stub"

[providers.stub.kwargs]
api_base = "http://127.0.0.1:$PORT/v1"
EOF
STUB_APP="env NO_PROXY=127.0.0.1 no_proxy=127.0.0.1 $CFG_ENV_NOHIST ALTER_ZERO_PERMISSIONS=0 ALTER_ZERO_CONTEXT_WINDOW=30000 ALTER_ZERO_PROVIDERS_FILE=$PROVIDERS ALTER_ZERO_PROVIDER=stub ALTER_ZERO_MODEL=stub-model STUB_API_KEY=stand-in $BIN_ABS"

# count KIND — how many requests of that kind the stub logged.
count() { grep -c "\"kind\": \"$1\"" "$LOG" 2>/dev/null; }
# report — what the log says, one `name value` line each.
report() {
	python3 - "$LOG" <<'PY'
import json, sys
rows = [json.loads(line) for line in open(sys.argv[1]) if line.strip()]
agent = [r for r in rows if r["kind"] == "agent"]
summaries = [r for r in rows if r["kind"] == "summary"]
meanwhile = sum(1 for a in agent if any(s["began"] < a["at"] < s["ended"] for s in summaries))
print("largest", max((r["tokens"] for r in agent), default=0))
print("meanwhile", meanwhile)
print("transcripts", sum(1 for s in summaries if s["transcript"]))
print("native", sum(1 for s in summaries if "tool" in s["roles"] or "assistant" in s["roles"]))
PY
}
field() { printf '%s\n' "$1" | awk -v name="$2" '$1 == name { print $2 }'; }

launch -c "$WORK" "$S171" 100 40 "$STUB_APP"
submit "$S171" "smoke171 read the notes until you are told to stop"
busy="$(wait_pane 60 "$S171" -- -iF "compacting context")" ||
	fail "the status line never said a summary was being written"
dump "the status line while a summary is being written" "$busy"
wait_for 120 "$S171" -S -3000 -- -F "DONE-171" ||
	fail "the turn never reached its closing line (did it die at the window?)"
done_pane="$(wait_summaries 20 "$S171" 1)" || fail "the turn never settled"
whole="$(pane "$S171" -S -3000)"
dump "the end of the turn" "$done_pane"
facts="$(report)"
note "what the stub saw"
printf '%s\n' "$facts"

CELL="● Context compacted"
expect_eq "$(count refused)" "0" "a request was sent past the model's window — the turn was not compacted in time"
if [ "$(count summary)" -lt 2 ]; then
	fail "a turn several windows long should compact more than once (summaries: $(count summary))"
fi
if [ "$(field "$facts" largest)" -gt 30000 ]; then
	fail "the largest request was past the window ($(field "$facts" largest) tokens)"
fi
if [ "$(field "$facts" meanwhile)" -lt 1 ]; then
	fail "no agent request was served while a summary was being written — compaction is not in the background"
fi
expect_eq "$(field "$facts" transcripts)" "$(count summary)" "a summarization was not sent the conversation as a transcript"
expect_eq "$(field "$facts" native)" "0" "a summarization carried the conversation's own tool messages"
expect_eq "$(count agent)" "35" "the turn should make its 34 reads and one closing request"
expect_has "$busy" -F "esc to interrupt" "the turn was not running while its summary was being written"
cells="$(printf '%s\n' "$whole" | grep -cF "$CELL")"
expect_eq "$cells" "$(count summary)" "each summary that landed should leave one '● Context compacted' cell"
expect_eq "$(printf '%s\n' "$whole" | grep -F "$CELL" | grep -cF "· auto")" "$cells" "a mid-turn compaction's cell is not tagged auto"
first_cell="$(printf '%s\n' "$whole" | grep -nF "$CELL" | head -1 | cut -d: -f1)"
last_read="$(printf '%s\n' "$whole" | grep -nF "● Read(" | tail -1 | cut -d: -f1)"
if [ -z "$first_cell" ] || [ -z "$last_read" ] || [ "$first_cell" -ge "$last_read" ]; then
	fail "the first compaction cell is not among the turn's tool cells (cell row '$first_cell', last read row '$last_read')"
fi
expect_has "$whole" -F "DONE-171" "the closing line is not in the conversation"
expect_lacks "$whole" -F "maximum context length" "the provider's refusal reached the conversation"
tmux kill-session -t "$S171" 2>/dev/null
