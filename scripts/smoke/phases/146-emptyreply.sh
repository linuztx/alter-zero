#!/usr/bin/env bash
# Phase 146 — an empty reply is answered with a continue prompt; one that stays empty ends the turn on an error

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# The model thinks and then sends back nothing — no text, no tool call
# (docs/empty-reply.md). The turn used to end right there, its summary under
# a reply that never came, until the user typed `continue`. A stub
# chat-completions server answers the first request with a thought and a
# `stop` and nothing else. The app must ask again in the same turn — the
# continue prompt the request's last message — and the turn ends on the
# second answer, with no error and no interrupt. Ctrl+O shows the prompt
# under its `Empty reply` heading, and Ctrl+D shows it where the model read
# it.
#
# Half two, same session: a conversation whose every round runs out of
# output tokens mid-thought. The app asks twice, in the output-limit
# wording, then ends the turn on the error that says what happened and
# points at Ctrl+T.
S146="${S}_emptyreply"
EMPTY_WORK="$(work_dir emptyreply)"
EMPTY_LOG="$SMOKE_TMP/emptyreply.log"
EMPTY_PORT="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
python3 - "$EMPTY_PORT" "$EMPTY_LOG" <<'PY' &
import json, sys, threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

port, log = int(sys.argv[1]), sys.argv[2]
lock = threading.Lock()

def text(message):
    content = message.get("content")
    if isinstance(content, list):
        return " ".join(part.get("text", "") for part in content if isinstance(part, dict))
    return content or ""

def frame(delta, finish=None):
    return {"choices": [{"index": 0, "delta": delta, "finish_reason": finish}]}

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

    def stream(self, frames):
        self.send_response(200)
        self.send_header("content-type", "text/event-stream")
        self.send_header("transfer-encoding", "chunked")
        self.end_headers()
        for each in frames:
            self.chunk(("data: %s\n\n" % json.dumps(each)).encode())
        self.chunk(b"data: [DONE]\n\n")
        self.wfile.write(b"0\r\n\r\n")
        self.wfile.flush()

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers.get("content-length") or 0)).decode())
        messages = body.get("messages", [])
        with lock, open(log, "a") as f:
            f.write(json.dumps(messages) + "\n")
        self.close_connection = True
        said = [text(m) for m in messages if m.get("role") == "user"]
        if any("smoke146 never" in words for words in said):
            # Every round of this conversation thinks until the output
            # limit cuts it off, before a word of the reply.
            self.stream([frame({"reasoning_content": "Still weighing the model."}),
                         frame({}, "length")])
        elif said and said[-1].startswith("[system]"):
            # Asked to continue: this time the reply comes.
            self.stream([frame({"content": "The model is fine: smoke146 answered."}),
                         frame({}, "stop")])
        else:
            # The reported shape: a thought, then the round ends on `stop`
            # with no text and no call.
            self.stream([frame({"reasoning_content": "Weighing what is wrong with the model."}),
                         frame({}, "stop")])

    def log_message(self, *_args):
        pass

ThreadingHTTPServer(("127.0.0.1", port), Stub).serve_forever()
PY
EMPTY_SERVER=$!
smoke_on_exit "kill $EMPTY_SERVER 2>/dev/null"
for _ in $(seq 1 30); do # wait for the stub to listen
	python3 -c "import socket, sys; s = socket.socket(); s.settimeout(0.2); sys.exit(0 if s.connect_ex(('127.0.0.1', $EMPTY_PORT)) == 0 else 1)" && break
	sleep 0.1
done
EMPTY_PROVIDERS="$SMOKE_TMP/providers146.toml"
cat >"$EMPTY_PROVIDERS" <<EOF
[providers.stub]
name = "Stub"

[providers.stub.kwargs]
api_base = "http://127.0.0.1:$EMPTY_PORT/v1"
EOF
# The thinking display is on, so the empty round's thought settles into its
# cell exactly as in the report.
EMPTY_APP="env NO_PROXY=127.0.0.1 no_proxy=127.0.0.1 $CFG_ENV_NOHIST ALTER_ZERO_PROVIDERS_FILE=$EMPTY_PROVIDERS ALTER_ZERO_PROVIDER=stub ALTER_ZERO_MODEL=stub-model STUB_API_KEY=stand-in ALTER_ZERO_SHOW_THINKING=1 $BIN_ABS"
launch -c "$EMPTY_WORK" "$S146" 120 40 "$EMPTY_APP"
wait_for 5 "$S146" -F "stub-model" ||
	fail "the footer never named the stub model (the real backend did not activate)"

submit "$S146" "smoke146 what is wrong with the model?"
answered="$(wait_settled 30 "$S146" -E "^$SUMMARY_RE")" ||
	fail "the turn never settled"
dump "an empty round, a continue prompt, then the reply" "$answered"
keys "$S146" C-o
transcript="$(wait_pane 5 "$S146" -F "Empty reply")"
dump "Ctrl+O — the continue prompt between the two rounds" "$transcript"
keys "$S146" C-o
wait_settled 5 "$S146" -E "^$SUMMARY_RE" >/dev/null ||
	fail "Ctrl+O did not close"
keys "$S146" C-d
context="$(wait_pane 5 "$S146" -F "Your last response was")"
dump "Ctrl+D — the prompt where the model read it" "$context"
# The page hard-wraps mid-word under a two-space indent: join each wrapped
# row back onto the one before it.
context_flat="$(printf '%s' "$context" | sed -e ':a' -e 'N' -e '$!ba' -e 's/\n  //g')"
keys "$S146" C-d
wait_settled 5 "$S146" -E "^$SUMMARY_RE" >/dev/null ||
	fail "Ctrl+D did not close"
recovered_posts="$(grep -c . "$EMPTY_LOG")"

note "an empty reply is answered with a continue prompt and the turn goes on"
expect_has "$answered" -F "Thought for" "the empty round's thought never settled into its cell"
expect_has "$answered" -F "The model is fine: smoke146 answered." "the reply after the prompt never came"
expect_lacks "$answered" -F "Conversation interrupted" "the recovery was reported as an interrupt"
expect_lacks "$answered" -F "empty response" "a recovered turn showed the give-up error"
expect_has "$transcript" -F "Empty reply" "Ctrl+O did not show the continue prompt's heading"
expect_has "$transcript" -F "[system] Your last response was empty" "Ctrl+O did not show the prompt itself"
expect_has "$context_flat" -F "[system] Your last response was empty: no text and no tool call" \
	"the derived context does not carry the prompt the model read"
expect_eq "$recovered_posts" "2" "the empty round should cost exactly one more request"
python3 - "$EMPTY_LOG" <<'PY' || fail "the continue prompt did not go out as the request's last message (see above)"
import json, sys

requests = [json.loads(line) for line in open(sys.argv[1]) if line.strip()]
last = requests[1][-1]
content = last.get("content")
if last.get("role") != "user" or not str(content).startswith("[system] Your last response was empty"):
    print("FAIL: the second request ends on %r" % (last,))
    sys.exit(1)
if any(m.get("role") == "assistant" for m in requests[1]):
    print("FAIL: the empty reply left an assistant message behind: %r" % (requests[1],))
    sys.exit(1)
PY

# ---- half two: a model that never answers ----------------------------------
submit "$S146" "smoke146 never mind, look again"
gave_up="$(wait_settled 30 "$S146" -F "returned an empty response 3 times in a row")" ||
	fail "a model that kept answering with nothing never ended the turn on the error"
dump "three empty rounds, then the error" "$gave_up"
history="$(pane "$S146" -S -400)"
tmux kill-session -t "$S146" 2>/dev/null

note "a model that keeps replying with nothing ends the turn on an error"
expect_has "$gave_up" -F "ran out of output tokens" "the error did not say the output limit cut the round off"
expect_has "$gave_up" -F "Ctrl+T" "the error did not point at the thinking level"
expect_eq "$(grep -c . "$EMPTY_LOG")" "$((recovered_posts + 3))" \
	"the first round and two continue prompts, then no more requests"
expect_eq "$(grep -cF "returned an empty response" <<<"$history")" "1" \
	"the error should appear exactly once"
python3 - "$EMPTY_LOG" <<'PY' || fail "the output-limit prompts went out wrong (see above)"
import json, sys

requests = [json.loads(line) for line in open(sys.argv[1]) if line.strip()]
for n in (3, 4):
    last = requests[n][-1]
    if last.get("role") != "user" or "hit the output token limit" not in str(last.get("content")):
        print("FAIL: request %d ends on %r" % (n + 1, last))
        sys.exit(1)
PY
