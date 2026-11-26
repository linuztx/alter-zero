#!/usr/bin/env bash
# Phase 171 — background compaction inside a running turn

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# A running turn compacts without ending (docs/background-compaction.md
# *The lifecycle*). A local stub stands in for an OpenAI-compatible provider:
# every turn request answers with a `read` of a 740-line file — some 14k
# tokens by the estimate, under the read tool's cap — and a usage frame whose
# prompt count grows 30k a round, on a 200k window: the ceiling 170k, the
# start line 110k. The turn measures each round top as the last count plus
# the estimate of the read since, so for any read under 20k tokens the round
# top before the fifth request starts a summary in the background and the
# turn goes on; the summarizer's request (no tools, the compact prompt last)
# sleeps four seconds, so the round top before the seventh finds the turn
# past its ceiling with the summary unwritten and waits on it; once written
# it is swapped in, the turn's seventh request goes out compacted — the
# summary's bridge, then the last two rounds verbatim — and the stub ends the
# turn there. Every request body is logged and read back.
S171="${S}_midturncompact"
M_LOG="$SMOKE_TMP/requests171.jsonl"
M_PORT="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
M_PROJ="$SMOKE_TMP/proj171"
mkdir -p "$M_PROJ"
for i in $(seq -w 1 740); do
	printf 'smoke171 line %s: the payments module moves to the ledger service next week.\n' "$i"
done >"$M_PROJ/notes.txt"
python3 - "$M_PORT" "$M_LOG" "$M_PROJ/notes.txt" <<'PY' &
import json, sys, threading, time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

port, log, notes = int(sys.argv[1]), sys.argv[2], sys.argv[3]
lock = threading.Lock()
turn_requests = [0]

def text_of(content):
    if isinstance(content, list):
        return "".join(part.get("text", "") for part in content if isinstance(part, dict))
    return content or ""

def sse(frames):
    return ("".join("data: %s\n\n" % json.dumps(f) for f in frames) + "data: [DONE]\n\n").encode()

def usage(prompt):
    return {"choices": [], "usage": {"prompt_tokens": prompt, "completion_tokens": 20,
                                     "total_tokens": prompt + 20}}

class Stub(BaseHTTPRequestHandler):
    def do_GET(self):
        data = json.dumps({"data": [{"id": "stub-model", "context_length": 200000}]}).encode()
        self.reply(data, "application/json")

    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("content-length") or 0))
        with lock:
            with open(log, "ab") as f:
                f.write(body + b"\n")
        messages = json.loads(body)["messages"]
        texts = [text_of(m.get("content")) for m in messages]
        if "CONTEXT CHECKPOINT COMPACTION" in texts[-1]:
            time.sleep(4)  # writing the summary while the turn goes on
            frames = [{"choices": [{"delta": {"content": "Stub summary: the user is reading notes."}}]},
                      {"choices": [{"delta": {}, "finish_reason": "stop"}]}, usage(6000)]
        elif any("Stub summary" in t for t in texts):
            frames = [{"choices": [{"delta": {"content": "All done."}}]},
                      {"choices": [{"delta": {}, "finish_reason": "stop"}]}, usage(90000)]
        else:
            with lock:
                turn_requests[0] += 1
                n = turn_requests[0]
            if n > 9:  # never compacted: end the turn rather than hang
                frames = [{"choices": [{"delta": {"content": "Gave up."}}]},
                          {"choices": [{"delta": {}, "finish_reason": "stop"}]}, usage(30000 * n)]
            else:
                call = {"index": 0, "id": "call_171_%d" % n, "type": "function",
                        "function": {"name": "read", "arguments": json.dumps({"path": notes})}}
                frames = [{"choices": [{"delta": {"tool_calls": [call]}}]},
                          {"choices": [{"delta": {}, "finish_reason": "tool_calls"}]},
                          usage(30000 * n)]
        self.reply(sse(frames), "text/event-stream")

    def reply(self, data, kind):
        self.send_response(200)
        self.send_header("content-type", kind)
        self.send_header("content-length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, *_args):
        pass

ThreadingHTTPServer(("127.0.0.1", port), Stub).serve_forever()
PY
M_SERVER=$!
smoke_on_exit kill "$M_SERVER"
for _ in $(seq 1 30); do # wait for the stub to listen
	if python3 -c "import socket, sys; s = socket.socket(); s.settimeout(0.2); sys.exit(0 if s.connect_ex(('127.0.0.1', $M_PORT)) == 0 else 1)" 2>/dev/null; then
		break
	fi
	sleep 0.1
done
M_PROVIDERS="$SMOKE_TMP/providers171.toml"
cat >"$M_PROVIDERS" <<TOML
[providers.stub]
name = "Stub"
api_key_env = "STUB_API_KEY"

[providers.stub.kwargs]
api_base = "http://127.0.0.1:$M_PORT/v1"
TOML

launch -c "$M_PROJ" "$S171" 100 40 "env NO_PROXY=127.0.0.1 no_proxy=127.0.0.1 ALTER_ZERO_CONTEXT_WINDOW=200000 ALTER_ZERO_PROVIDERS_FILE=$M_PROVIDERS ALTER_ZERO_PROVIDER=stub ALTER_ZERO_MODEL=stub-model STUB_API_KEY=smoke-171 $APP_ABS"
submit "$S171" "smoke171 go"
# Past the start line the summary is written in the background, the row under
# the status line saying so; past the ceiling the turn waits on it.
writing="$(wait_pane 20 "$S171" -F "Compacting in the background")" || fail "no background summary row while the turn ran"
dump "the summary being written" "$writing"
waiting="$(wait_pane 10 "$S171" -F "Compacting…")" || fail "the turn never waited on the summary at its ceiling"
dump "the turn waiting at its ceiling" "$waiting"
done_pane="$(wait_pane 20 "$S171" -S -300 -- -F "All done.")" || fail "the turn never finished after the swap"
dump "the turn after the swap" "$done_pane"

report="$(python3 - "$M_LOG" <<'PY'
import json, sys

def text_of(content):
    if isinstance(content, list):
        return "".join(part.get("text", "") for part in content if isinstance(part, dict))
    return content or ""

bodies = [json.loads(line) for line in open(sys.argv[1]) if line.strip()]
def last_text(body):
    return text_of(body["messages"][-1].get("content"))
summaries = [b for b in bodies if "CONTEXT CHECKPOINT COMPACTION" in last_text(b)]
compacted = [b for b in bodies
             if not "CONTEXT CHECKPOINT COMPACTION" in last_text(b)
             and any("Stub summary" in text_of(m.get("content")) for m in b["messages"])]
print(f"summaries {len(summaries)}")
for name, body in (("summary", summaries[0] if summaries else None),
                   ("compacted", compacted[0] if compacted else None)):
    if body is None:
        continue
    messages = body["messages"]
    texts = [text_of(m.get("content")) for m in messages]
    print(f"{name} tools {bool(body.get('tools'))}")
    print(f"{name} tool-results {sum(1 for m in messages if m['role'] == 'tool')}")
    print(f"{name} asks {any('smoke171 go' in t for t in texts)}")
    # The kept request and the bridge go out as one user message with the
    # reminder: consecutive user-role entries merge on the wire.
    print(f"{name} bridge {any('Another language model started to solve this problem' in t for t in texts)}")
PY
)" || fail "the request log could not be read: $report"
dump "what the requests carried" "$report"
tmux kill-session -t "$S171" 2>/dev/null

echo "==== Phase 171: background compaction inside a running turn ===="
expect_has "$report" -xF "summaries 1" "not exactly one summary was written"
# The summary: tools-free, covering the four rounds before its start.
expect_has "$report" -xF "summary tools False" "the summarizer was offered tools"
expect_has "$report" -xF "summary tool-results 4" "the summary did not cover exactly the rounds before the round top that started it"
expect_has "$report" -xF "summary asks True" "the summarizer was not sent the user's request"
# The turn's next request: the bridge, the request, the last two rounds verbatim.
expect_has "$report" -xF "compacted tools True" "the turn lost its tools after the swap"
expect_has "$report" -xF "compacted bridge True" "the compacted request carries no summary bridge"
expect_has "$report" -xF "compacted asks True" "the compacted request lost the user's request"
expect_has "$report" -xF "compacted tool-results 2" "the compacted request did not keep the rounds after the summary's start verbatim"
# On screen: the cell in place, inside the turn — after its reads, before its answer.
expect_has "$done_pane" -E "Context compacted · [0-9.]+k? → [0-9.]+k? tokens( · [0-9]+[hms][0-9ms ]*)? · background" "the swap committed no '· background' compaction cell"
last_read="$(grep -n -F "● Read(" <<<"$done_pane" | tail -1 | cut -d: -f1)"
cell="$(grep -n -F "● Context compacted" <<<"$done_pane" | tail -1 | cut -d: -f1)"
answer="$(grep -n -F "● All done." <<<"$done_pane" | tail -1 | cut -d: -f1)"
[ -n "$last_read" ] && [ -n "$cell" ] && [ -n "$answer" ] && [ "$last_read" -lt "$cell" ] && [ "$cell" -lt "$answer" ] ||
	fail "the compaction cell is not between the turn's last read and its answer (lines: read $last_read, cell $cell, answer $answer)"
# The swap freed room: the context after it is smaller than the one it
# replaced.
freed="$(sed -n 's/.*Context compacted · \([0-9.]*\)k → \([0-9.]*\)k tokens.*/\1 \2/p' <<<"$done_pane" | head -1)"
python3 -c "import sys; before, after = map(float, sys.argv[1:3]); sys.exit(0 if after < before else 1)" $freed 2>/dev/null ||
	fail "the compaction cell does not show a smaller context after the swap (got '$freed')"
expect_eq "$(grep -c "Context compacted" <<<"$done_pane")" "1" "the turn compacted more than once"
