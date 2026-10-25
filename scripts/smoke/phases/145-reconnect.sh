#!/usr/bin/env bash
# Phase 145 — a reply whose connection dies mid-stream is carried on, not failed

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# The reported failure (docs/reconnect.md): `request failed: request or
# response body error: error reading a body from connection: Connection timed
# out (os error 110)` — the connection under a streaming reply died and the
# turn ended on a red error with the reply half written. A stub provider
# plays that link: its first answer streams half a reply and then the
# connection dies in the middle of the body (an HTTP chunk that never ends,
# the deterministic stand-in for a keepalive giving up on a dead network).
# The turn must not fail: the half that streamed stays, the amber `● Lost the
# connection to 127.0.0.1:{port} · reconnecting` marker commits under it with
# the socket's own words in its `⎿` gutter, and the backend sends one more
# request — which the stub records — carrying the cut-off text as the
# assistant's message and the `[system]` note after it, the status line
# counting it `retrying 1/3` meanwhile. The continuation streams as a message
# of its own and the turn settles like any other; Ctrl+O shows the note the
# model read. A second turn's connection dies while the model is only
# thinking: nothing it said is on screen to go on from, so the thought settles
# into its cell, the marker commits, and the round is sent again exactly as
# it was — no note.
S145="${S}_reconnect"
RC_CFG="$(mktemp -d "$SMOKE_TMP/reconnect-cfg.XXXXXX")"
RC_DIR="$(work_dir reconnect)"
RC_PORT="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
RC_LOG="$RC_CFG/requests.jsonl"
cat >"$RC_CFG/providers.toml" <<PROVIDERS
[providers.flaky]
name = "Flaky"
api_model_base = "http://127.0.0.1:$RC_PORT/v1"

[providers.flaky.kwargs]
api_base = "http://127.0.0.1:$RC_PORT/v1"
PROVIDERS
cat >"$RC_CFG/config.json" <<'CONFIG'
{ "provider": "flaky", "model": "stub-model" }
CONFIG

python3 - "$RC_PORT" "$RC_LOG" <<'PY' &
import json, sys, time
from http.server import BaseHTTPRequestHandler, HTTPServer

port, log = int(sys.argv[1]), sys.argv[2]
answered = 0

def frame(text, finish=None, field="content"):
    return "data: " + json.dumps({
        "id": "stub", "object": "chat.completion.chunk", "model": "stub-model",
        "choices": [{"index": 0, "delta": {field: text}, "finish_reason": finish}],
    }) + "\n\n"

class Stub(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def chunk(self, text):
        data = text.encode()
        self.wfile.write(b"%x\r\n" % len(data) + data + b"\r\n")
        self.wfile.flush()

    def do_POST(self):
        global answered
        body = self.rfile.read(int(self.headers.get("content-length") or 0))
        with open(log, "a") as out:
            out.write(json.dumps(json.loads(body)) + "\n")
        answered += 1
        if answered in (2, 4):
            # The reconnect takes a moment, long enough to see the status
            # line count it.
            time.sleep(1.5)
        self.send_response(200)
        self.send_header("content-type", "text/event-stream")
        self.send_header("transfer-encoding", "chunked")
        self.end_headers()
        if answered in (1, 3):
            # Half a reply — or, the second time, only thinking — a pause
            # long enough to see it on screen, then a chunk that never ends:
            # the connection dies mid-body.
            if answered == 1:
                for piece in ["Two steps. ", "First, split ", "the lex"]:
                    self.chunk(frame(piece))
            else:
                for piece in ["Weighing ", "the test ", "layout"]:
                    self.chunk(frame(piece, field="reasoning_content"))
            time.sleep(1.5)
            self.wfile.write(b"40\r\ndata: {\"choi")
            self.wfile.flush()
            self.close_connection = True
            return
        reply = (
            ["First, split ", "the lexer. ", "Then test it."]
            if answered == 2
            else ["Unit tests ", "beside each module."]
        )
        for piece in reply:
            self.chunk(frame(piece))
        self.chunk(frame("", "stop"))
        self.chunk("data: [DONE]\n\n")
        self.wfile.write(b"0\r\n\r\n")
        self.wfile.flush()

    def do_GET(self):
        data = json.dumps({"data": [{"id": "stub-model"}]}).encode()
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, *_args):
        pass

HTTPServer(("127.0.0.1", port), Stub).serve_forever()
PY
RC_SERVER=$!
smoke_on_exit "kill $RC_SERVER 2>/dev/null"
for _ in $(seq 1 30); do # wait for the stub to listen
	python3 -c "import socket, sys; s = socket.socket(); s.settimeout(0.2); sys.exit(0 if s.connect_ex(('127.0.0.1', $RC_PORT)) == 0 else 1)" && break
	sleep 0.1
done

# The loopback request must not be sent to a developer's HTTPS proxy.
RC_ENV="-u HTTPS_PROXY -u https_proxy -u HTTP_PROXY -u http_proxy -u ALL_PROXY -u all_proxy ALTER_ZERO_TELEMETRY=0 ALTER_ZERO_UPDATE_CHECK=0 ALTER_ZERO_PROJECT_CONFIG=0 ALTER_ZERO_CONFIG_DIR=$RC_CFG ALTER_ZERO_SESSIONS_DIR=$RC_CFG/sessions ALTER_ZERO_HISTORY_FILE=/dev/null ALTER_ZERO_CHECKPOINTS=0 ALTER_ZERO_SKILLS_DIR=$SMOKE_SKILLS ALTER_ZERO_AGENTS_DIR=$SMOKE_AGENTS ALTER_ZERO_PROVIDERS_FILE=$RC_CFG/providers.toml ALTER_ZERO_API_KEY=not-a-real-key ALTER_ZERO_TIPS=0"
launch -c "$RC_DIR" "$S145" 100 30 "env $RC_ENV $BIN_ABS"
wait_for 5 "$S145" -F "stub-model" ||
	fail "the footer never named the stub model (the real backend did not activate)"
submit "$S145" "plan the refactor"
half="$(wait_pane 10 "$S145" -F "First, split the lex")" ||
	fail "the first half of the reply never streamed"
dump "half the reply, before the connection dies" "$half"
counting="$(wait_pane 10 "$S145" -F "retrying 1/3")" ||
	fail "the status line never counted the reconnect"
dump "the marker committed, the status line counting the reconnect" "$counting"
settled="$(wait_pane 20 "$S145" -E "^$SUMMARY_RE")" ||
	fail "the turn never settled after the connection died"
dump "the drop, the reconnect, the continuation" "$settled"

note "the drop is carried on — the half kept, the marker under it, the continuation after"
expect_has "$counting" -F "● Lost the connection to 127.0.0.1:$RC_PORT · reconnecting" \
	"the marker was not on screen while the turn reconnected"
expect_has "$counting" -E "Working… \([0-9]+s · ↓ [0-9]+ tokens · retrying 1/3 · esc to interrupt\)" \
	"the status line does not count the reconnect against the retry budget"
expect_has "$settled" -F "● Two steps. First, split the lex" "the half that streamed was not kept as its own message"
expect_has "$settled" -F "● Lost the connection to 127.0.0.1:$RC_PORT · reconnecting" \
	"no marker names the host the reply was cut off from"
expect_has "$settled" -E "^  ⎿  .+" "the marker has no cause under it"
expect_has "$settled" -F "● First, split the lexer. Then test it." "the continuation did not stream as a message of its own"
expect_lacks "$settled" -F "request failed" "the drop failed the turn"
expect_lacks "$settled" -F "Conversation interrupted" "the drop was reported as an interrupt"
expect_lacks "$settled" -F "retrying" "the reconnect's clause outlived the continuation"
order="$(printf '%s\n' "$settled" | grep -nE "First, split the lex$|Lost the connection|First, split the lexer\." | cut -d: -f1 | tr '\n' ' ')"
expect_eq "$(printf '%s\n' "$order" | awk '{ print ($1 < $2 && $2 < $3) ? "ordered" : "out of order" }')" \
	"ordered" "the half, the marker and the continuation are not in that order ($order)"

note "the continuing request carried the cut-off text and the note"
expect_eq "$(wc -l <"$RC_LOG" | tr -d ' ')" "2" "the stub was not asked exactly twice"
continued="$(python3 - "$RC_LOG" <<'PY'
import json, sys

def text(content):
    if isinstance(content, list):
        return "".join(part.get("text", "") for part in content)
    return content

second = [json.loads(line) for line in open(sys.argv[1])][1]["messages"]
for message in second[-2:]:
    print(message["role"], "|", text(message["content"])[:80])
PY
)"
dump "the continuing request's last two messages" "$continued"
expect_has "$continued" -F "assistant | Two steps. First, split the lex" \
	"the continuation was not sent the cut-off text as the assistant's message"
expect_has "$continued" -F "user | [system] The connection dropped while you were replying" \
	"the continuation was not sent the note"

note "Ctrl+O shows the note the model read"
keys "$S145" C-o
transcript="$(wait_pane 5 "$S145" -F "[system] The connection dropped")" ||
	fail "the transcript does not show the note under the marker"
dump "the Ctrl+O transcript" "$transcript"
keys "$S145" q
wait_for 5 "$S145" -E '^❯' || fail "Ctrl+O never closed"

# A drop while the model was only thinking: nothing it said is on screen to
# go on from, so the round is sent again exactly as it was — the thought it
# had settles into its own cell, the marker commits under it, and the
# request carries no note.
submit "$S145" "and the tests?"
second="$(wait_summaries 25 "$S145" 2)" ||
	fail "the second turn never settled after its connection died mid-thought"
dump "a drop mid-thought: the round sent again" "$second"
note "a drop with nothing visible kept — no note, the round sent again"
expect_eq "$(printf '%s\n' "$second" | grep -cF "Lost the connection to 127.0.0.1:$RC_PORT")" "2" \
	"the second drop left no marker"
expect_has "$second" -E "^Thought for [0-9]+s" "the thought before the drop did not settle into its cell"
expect_has "$second" -F "● Unit tests beside each module." "the re-sent round's reply did not stream"
expect_lacks "$second" -F "request failed" "the drop mid-thought failed the turn"
resent="$(python3 - "$RC_LOG" <<'PY'
import json, sys

def text(content):
    if isinstance(content, list):
        return "".join(part.get("text", "") for part in content)
    return content

requests = [json.loads(line)["messages"] for line in open(sys.argv[1])]
third, fourth = requests[2], requests[3]
print("same request:", third == fourth)
print("last:", fourth[-1]["role"], "|", text(fourth[-1]["content"])[:60])
PY
)"
dump "the re-sent round" "$resent"
expect_has "$resent" -F "same request: True" "the round after a drop mid-thought was not sent again as it was"
expect_has "$resent" -F "last: user | and the tests?" "a note was sent after a drop that kept nothing"
tmux kill-session -t "$S145" 2>/dev/null
kill "$RC_SERVER" 2>/dev/null
