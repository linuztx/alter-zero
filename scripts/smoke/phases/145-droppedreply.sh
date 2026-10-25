#!/usr/bin/env bash
# Phase 145 — a reply the connection drops mid-stream carries on where it stopped

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# A dropped reply (docs/dropped-reply.md). A stub chat-completions server
# streams the first half of a sentence, holds the connection until the phase
# has seen that half on screen, then resets it — the way a link that died
# mid-reply ends (`request or response body error: error reading a body from
# connection: …`). The turn must not fail: the status line shows the bounded
# `retrying 1/3`, the next request carries the reply so far as the
# assistant's own message with a note asking for the rest, and the stub's
# second half streams on after the first — one sentence on screen, shown
# once, and the turn settles like any other. A second half turns `Error
# retry` off: then the drop surfaces as the red error with the partial kept,
# and nothing is sent again. A third drops the connection while a `write` is
# streaming into its cell: the half-made call never ran, its preview goes
# with the attempt that streamed it, and the call made again runs once,
# whole, in one cell.
S145="${S}_dropped"
DR_CFG="$(mktemp -d "$SMOKE_TMP/dropped-cfg.XXXXXX")"
DR_DIR="$(work_dir dropped)"
DR_STUB="$SMOKE_TMP/dropped-stub.py"
cat >"$DR_STUB" <<'PY'
import json, os, socket, struct, sys, time
from http.server import BaseHTTPRequestHandler, HTTPServer

port, state, mode = int(sys.argv[1]), sys.argv[2], sys.argv[3]
CONTENT = "".join("row %02d = %d\n" % (i, i * i) for i in range(1, 31))
ARGUMENTS = json.dumps({"path": "squares.py", "content": CONTENT})

def frame(delta, finish=None):
    return ("data: " + json.dumps({
        "id": "stub", "object": "chat.completion.chunk", "model": "stub-model",
        "choices": [{"index": 0, "delta": delta, "finish_reason": finish}],
    }) + "\n\n").encode()

def text(piece, finish=None):
    return frame({"role": "assistant", "content": piece}, finish)

def call(arguments, named=False):
    function = {"arguments": arguments}
    piece = {"index": 0, "function": function}
    if named:
        function["name"] = "write"
        piece.update({"id": "call_145", "type": "function"})
    return frame({"tool_calls": [piece]})

def pieces(arguments):
    return [call(arguments[at:at + 8]) for at in range(0, len(arguments), 8)]

def frames(post):
    """The frames a POST streams, and whether its connection then drops."""
    if mode == "text":
        if post == 1:
            return [text("The connection "), text("drops in "), text("the mid")], True
        return [text("dle of this "), text("sentence, and "), text("the reply "),
                text("carries on."), text("", "stop")], False
    if post == 1:
        half = ARGUMENTS[:len(ARGUMENTS) // 2]
        return [text("Writing it now.\n\n"), call("", named=True)] + pieces(half), True
    if post == 2:
        return [call("", named=True)] + pieces(ARGUMENTS) + [frame({}, "tool_calls")], False
    return [text("Done."), text("", "stop")], False

class Stub(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    posts = 0

    def chunk(self, data):
        self.wfile.write(b"%x\r\n%s\r\n" % (len(data), data))
        self.wfile.flush()

    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("content-length") or 0))
        Stub.posts += 1
        with open(os.path.join(state, "request-%d.json" % Stub.posts), "wb") as out:
            out.write(body)
        self.send_response(200)
        self.send_header("content-type", "text/event-stream")
        self.send_header("transfer-encoding", "chunked")
        self.end_headers()
        streamed, drop = frames(Stub.posts)
        if Stub.posts == 2:
            time.sleep(1.5)  # the continuation, held so the retry clause can be read
        # The call made again streams slowly enough for its preview to be seen.
        pace = 0.03 if mode == "write" and Stub.posts == 2 else 0
        for data in streamed:
            self.chunk(data)
            time.sleep(pace)
        if drop:
            # Hold the connection until the phase has seen the half on
            # screen, then reset it: no closing chunk, no [DONE] — an RST.
            deadline = time.time() + 30
            while not os.path.exists(os.path.join(state, "drop")) and time.time() < deadline:
                time.sleep(0.05)
            self.connection.setsockopt(socket.SOL_SOCKET, socket.SO_LINGER, struct.pack("ii", 1, 0))
            self.rfile.close()
            self.connection.close()
            self.close_connection = True
            return
        self.chunk(b"data: [DONE]\n\n")
        self.wfile.write(b"0\r\n\r\n")
        self.wfile.flush()
        self.close_connection = True

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
# A provider file pointing the real backend at the stub's port, with a key
# that resolves and a saved selection (Phase 129's set-up).
dropped_provider() {
	cat >"$DR_CFG/providers.toml" <<PROVIDERS
[providers.cutoff]
name = "Cutoff"
api_model_base = "http://127.0.0.1:$1/v1"

[providers.cutoff.kwargs]
api_base = "http://127.0.0.1:$1/v1"
PROVIDERS
	cat >"$DR_CFG/config.json" <<'CONFIG'
{ "provider": "cutoff", "model": "stub-model" }
CONFIG
}
# Start a stub on a free port, recording into STATE and streaming MODE's
# replies, and point the provider file at it. Not in a command substitution:
# the stub would hold its pipe open.
dropped_stub() {
	local state="$1" mode="$2" port
	port="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
	python3 "$DR_STUB" "$port" "$state" "$mode" >>"$SMOKE_TMP/dropped-stub.log" 2>&1 &
	smoke_on_exit "kill $! 2>/dev/null"
	for _ in $(seq 1 30); do # wait for the stub to listen
		python3 -c "import socket, sys; s = socket.socket(); s.settimeout(0.2); sys.exit(0 if s.connect_ex(('127.0.0.1', $port)) == 0 else 1)" && break
		sleep 0.1
	done
	dropped_provider "$port"
}
# The loopback request must not be sent to a developer's HTTPS proxy.
DR_ENV="-u HTTPS_PROXY -u https_proxy -u HTTP_PROXY -u http_proxy -u ALL_PROXY -u all_proxy ALTER_ZERO_TELEMETRY=0 ALTER_ZERO_UPDATE_CHECK=0 ALTER_ZERO_PROJECT_CONFIG=0 ALTER_ZERO_CONFIG_DIR=$DR_CFG ALTER_ZERO_SESSIONS_DIR=$DR_CFG/sessions ALTER_ZERO_HISTORY_FILE=/dev/null ALTER_ZERO_CHECKPOINTS=0 ALTER_ZERO_SKILLS_DIR=$SMOKE_SKILLS ALTER_ZERO_AGENTS_DIR=$SMOKE_AGENTS ALTER_ZERO_PROVIDERS_FILE=$DR_CFG/providers.toml ALTER_ZERO_API_KEY=not-a-real-key ALTER_ZERO_TIPS=0"
APP_DR="env $DR_ENV $BIN_ABS"

# ---- half one: the drop, then the reply carries on ----
DR_STATE="$(mktemp -d "$SMOKE_TMP/dropped-state.XXXXXX")"
dropped_stub "$DR_STATE" text
launch -c "$DR_DIR" "$S145" 100 30 "$APP_DR"
wait_for 5 "$S145" -F "stub-model" ||
	fail "the footer never named the stub model (the real backend did not activate)"
submit "$S145" "$USER_MSG"
partial="$(wait_pane 10 "$S145" -F "The connection drops in the mid")" ||
	fail "the first half of the reply never streamed"
dump "the reply's first half, before the drop" "$partial"
note "the stub resets the connection mid-reply"
touch "$DR_STATE/drop"
retrying="$(wait_pane 10 "$S145" -F "retrying 1/3")" ||
	fail "the status line never showed the retry (did the drop fail the turn instead?)"
dump "the drop, retried" "$retrying"
settled="$(wait_pane 20 "$S145" -E "^$SUMMARY_RE")" ||
	fail "the turn never settled after the reply carried on"
dump "the reply carried on; the turn settled" "$settled"

note "the drop — retried, continued where it stopped, shown once"
expect_has "$retrying" -E "\([0-9]+s · ↓ [0-9]+ tokens · retrying 1/3 · esc to interrupt\)" \
	"the status line does not read as the bounded retry"
expect_has "$retrying" -F "The connection drops in the mid" "the partial reply left the screen while it was retried"
expect_lacks "$retrying" -F "request failed" "the drop failed the turn"
expect_has "$settled" -F "The connection drops in the middle of this sentence, and the reply carries on." \
	"the reply did not carry on from exactly where it stopped"
expect_eq "$(grep -cF "The connection drops" <<<"$settled")" "1" "the reply's opening should show exactly once"
expect_lacks "$settled" -F "request failed" "the drop surfaced as an error"
expect_lacks "$settled" -F "retrying" "the retry clause outlived the reply"
expect_lacks "$settled" -F "Conversation interrupted" "the recovery was reported as an interrupt"
# The continuation request: the round's own messages, then the reply so far as
# the assistant's, then the note asking for the rest.
continuation="$(python3 - "$DR_STATE/request-1.json" "$DR_STATE/request-2.json" <<'PY' 2>&1
import json, sys
first = json.load(open(sys.argv[1]))["messages"]
second = json.load(open(sys.argv[2]))["messages"]
assert second[:len(first)] == first, "the continuation does not open with the round's own messages"
assert len(second) == len(first) + 2, "the continuation adds %d messages, not two" % (len(second) - len(first))
reply, ask = second[-2], second[-1]
assert reply == {"role": "assistant", "content": "The connection drops in the mid"}, reply
assert ask["role"] == "user" and "cut off" in ask["content"], ask
print("ok")
PY
)"
dump "the continuation request" "$continuation"
expect_eq "$continuation" "ok" "the second request did not continue the reply"
tmux kill-session -t "$S145" 2>/dev/null

# ---- half two: with Error retry off, the drop surfaces as it always did ----
S145B="${S}_dropped_off"
DR_STATE_B="$(mktemp -d "$SMOKE_TMP/dropped-state.XXXXXX")"
dropped_stub "$DR_STATE_B" text
cat >"$DR_CFG/settings.json" <<'SETTINGS'
{ "error_retry": 0 }
SETTINGS
launch -c "$DR_DIR" "$S145B" 100 30 "$APP_DR"
wait_for 5 "$S145B" -F "stub-model" || fail "the second launch never named the stub model"
submit "$S145B" "$USER_MSG"
wait_for 10 "$S145B" -F "The connection drops in the mid" || fail "the second reply's first half never streamed"
touch "$DR_STATE_B/drop"
failed="$(wait_settled 10 "$S145B" -F "request failed")" ||
	fail "with retries off the drop did not surface as the error"
dump "Error retry 0: the drop surfaced" "$failed"
note "retries off — the error, the partial kept, nothing sent again"
expect_has "$failed" -F "The connection drops in the mid" "the partial reply was not kept"
expect_lacks "$failed" -F "retrying" "a retry was announced with retries off"
expect_lacks "$failed" -F "esc to interrupt" "the turn is still in flight after the error"
[ -e "$DR_STATE_B/request-2.json" ] && fail "a second request went out with retries off"
tmux kill-session -t "$S145B" 2>/dev/null

# ---- half three: a write cut off mid-call is made again, whole, once ----
S145C="${S}_dropped_write"
DR_STATE_C="$(mktemp -d "$SMOKE_TMP/dropped-state.XXXXXX")"
DR_WRITE_DIR="$(work_dir dropped-write)"
rm -f "$DR_CFG/settings.json"
dropped_stub "$DR_STATE_C" write
# No permission gate: the write runs unasked once its round ends.
launch -c "$DR_WRITE_DIR" "$S145C" 100 40 "env $DR_ENV ALTER_ZERO_PERMISSIONS=0 $BIN_ABS"
wait_for 5 "$S145C" -F "stub-model" || fail "the third launch never named the stub model"
submit "$S145C" "write the squares"
preview="$(wait_pane 10 "$S145C" -E "Writing [0-9]+ lines…")" ||
	fail "the write never streamed into its cell"
dump "the write streaming, before the drop" "$preview"
touch "$DR_STATE_C/drop"
# The call made again streams into a preview of its own: one that grows past
# where the dropped call stopped. Folded into the old draft, its pieces would
# land after the cut-off text — a string the decoder then closes, so the
# preview would sit at the old count until the call ran.
regrown="$(wait_pane 15 "$S145C" -E "Writing (1[6-9]|2[0-9]|30) lines…")" ||
	fail "the call made again never streamed past where the dropped one stopped"
dump "the call made again, streaming into its own preview" "$regrown"
written="$(wait_settled 30 "$S145C" -S -80 -- -F "Done.")" ||
	fail "the turn never settled after the call was made again"
dump "the call made again; the turn settled" "$written"
note "a write cut off mid-call — made again whole, run once, in one cell"
expect_has "$written" -F "Wrote 30 lines to squares.py" "the call made again did not run whole"
expect_eq "$(grep -cF "Write(squares.py)" <<<"$written")" "1" "the write should show as exactly one cell"
expect_eq "$(grep -cF "Writing it now." <<<"$written")" "1" "the text before the call should show exactly once"
expect_lacks "$written" -E "Writing [0-9]+ lines…" "the dropped attempt's preview outlived it"
expect_lacks "$written" -F "request failed" "the drop surfaced as an error"
expect_eq "$(wc -l <"$DR_WRITE_DIR/squares.py" | tr -d ' ')" "30" "the file on disk is not the whole write"
expect_eq "$(tail -n 1 "$DR_WRITE_DIR/squares.py")" "row 30 = 900" "the file on disk does not end where the call did"
# The continuation carried the text before the call, and the note said the
# call never went through.
remade="$(python3 - "$DR_STATE_C/request-2.json" <<'PY' 2>&1
import json, sys
messages = json.load(open(sys.argv[1]))["messages"]
reply, ask = messages[-2], messages[-1]
assert reply == {"role": "assistant", "content": "Writing it now.\n\n"}, reply
assert ask["role"] == "user" and "tool call" in ask["content"], ask
print("ok")
PY
)"
dump "the continuation request" "$remade"
expect_eq "$remade" "ok" "the second request did not carry on from the text before the call"
