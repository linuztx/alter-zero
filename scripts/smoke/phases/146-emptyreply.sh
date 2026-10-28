#!/usr/bin/env bash
# Phase 146 — a reply that comes back empty or cut off is continued, one that never comes ends red

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# The reported stall (docs/unfinished-replies.md): a round that only thought —
# no reply, no tool call — ended the turn on its summary, and the user had to
# type `continue`. A stub chat-completions server plays the model through the
# real backend:
#
# 1. `smoke146 empty` — the first request streams a thought and finishes with
#    `stop` and nothing said; the request after it must carry the loop's
#    continuation prompt, and its answer ends the turn.
# 2. `smoke146 cut` — the first request's thought runs into the output cap
#    (`finish_reason: "length"`); the continuation must say so.
# 3. `smoke146 never` — every request comes back empty: after its two
#    continuations the turn ends on the red notice, not on a summary that
#    looks finished.
#
# The stub logs, per request, which prompt it answers, how many continuation
# prompts follow that prompt, and the last message's text.
S146="${S}_emptyreply"
WORK="$(work_dir emptyreply)"
LOG="$SMOKE_TMP/emptyreply.log"
PORT="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
python3 - "$PORT" "$LOG" <<'PY' &
import json, sys, threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

port, log = int(sys.argv[1]), sys.argv[2]
lock = threading.Lock()
MARK = "smoke146 "
# Both continuation prompts open with this (`llm::agent`'s notes).
ASKED = "Your last response was"

def text(message):
    content = message.get("content")
    if isinstance(content, list):
        return " ".join(part.get("text", "") for part in content if isinstance(part, dict))
    return content or ""

def thought(words):
    return {"choices": [{"delta": {"reasoning_content": words}}]}

def said(words):
    return {"choices": [{"delta": {"content": words}}]}

def finish(reason):
    return {"choices": [{"delta": {}, "finish_reason": reason}]}

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
        body = json.loads(self.rfile.read(int(self.headers.get("content-length") or 0)).decode())
        messages = body.get("messages", [])
        transcript = "\n".join(text(m) for m in messages)
        # The newest prompt names the scenario; the prompts the loop added
        # after it say how far the turn has got.
        at = transcript.rfind(MARK)
        mode = transcript[at + len(MARK):].split()[0] if at >= 0 else ""
        asked = transcript.count(ASKED, at) if at >= 0 else 0
        with lock, open(log, "a") as f:
            f.write(json.dumps({"mode": mode, "asked": asked,
                                "last": text(messages[-1]) if messages else ""}) + "\n")
        if mode == "never" or (mode == "empty" and asked == 0):
            frames = [thought("Looking at the model first."), finish("stop")]
        elif mode == "cut" and asked == 0:
            frames = [thought("Weighing every option at great length."), finish("length")]
        elif mode == "empty":
            frames = [said("Recovered after an empty reply."), finish("stop")]
        else:
            frames = [said("Recovered after a cut-off reply."), finish("stop")]
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
PROVIDERS="$SMOKE_TMP/providers146.toml"
cat >"$PROVIDERS" <<EOF
[providers.stub]
name = "Stub"

[providers.stub.kwargs]
api_base = "http://127.0.0.1:$PORT/v1"
EOF
# The thinking display is on, so the empty round's thought settles into a cell
# the way it did in the report.
STUB_APP="env NO_PROXY=127.0.0.1 no_proxy=127.0.0.1 $CFG_ENV_NOHIST ALTER_ZERO_PERMISSIONS=0 ALTER_ZERO_SHOW_THINKING=1 ALTER_ZERO_PROVIDERS_FILE=$PROVIDERS ALTER_ZERO_PROVIDER=stub ALTER_ZERO_MODEL=stub-model STUB_API_KEY=stand-in $BIN_ABS"

# requests MODE — how many requests answered the `smoke146 MODE` prompt.
requests() { grep -c "\"mode\": \"$1\"" "$LOG" 2>/dev/null; }
# last_of MODE — the last message of the newest request for that prompt.
last_of() {
	python3 - "$LOG" "$1" <<'PY'
import json, sys
rows = [json.loads(line) for line in open(sys.argv[1]) if line.strip()]
rows = [r for r in rows if r["mode"] == sys.argv[2]]
print(rows[-1]["last"] if rows else "")
PY
}

launch -c "$WORK" "$S146" 100 34 "$STUB_APP"

# ---- 1. an empty reply ----
submit "$S146" "smoke146 empty"
first="$(wait_pane 20 "$S146" -F "Recovered after an empty reply.")" ||
	fail "the turn never got past the empty reply (did it end there?)"
first="$(wait_summaries 10 "$S146" 1)" || fail "the recovered turn never settled"
dump "an empty reply, continued" "$first"

note "an empty reply is continued, not taken for the answer"
expect_eq "$(requests empty)" "2" "the empty reply should be followed by exactly one more request"
expect_has "$(last_of empty)" -F "Your last response was empty" \
	"the request after the empty reply did not carry the continuation prompt"
expect_has "$first" -F "Thought for" "the empty round's thought never settled into its cell"
expect_lacks "$first" -F "stopped without replying" "a recovered turn showed the give-up notice"

# Ctrl+O shows why the turn kept going; the inline view stays clean.
expect_lacks "$first" -F "Empty reply" "the continuation leaked into the inline view"
keys "$S146" C-o
transcript="$(wait_pane 5 "$S146" -F "Empty reply")" ||
	fail "the Ctrl+O transcript does not show the continuation"
dump "the Ctrl+O transcript" "$transcript"
expect_has "$transcript" -F "Your last response was empty" \
	"the transcript does not show what the model was told"
keys "$S146" C-o
wait_for 5 "$S146" -F "stub-model" || fail "Ctrl+O did not return to the conversation"

# ---- 2. a reply cut off at the output cap ----
submit "$S146" "smoke146 cut"
second="$(wait_pane 20 "$S146" -F "Recovered after a cut-off reply.")" ||
	fail "the turn never got past the cut-off reply"
second="$(wait_summaries 10 "$S146" 2 -S -300)" || fail "the second turn never settled"
dump "a reply cut off at the output cap, continued" "$second"

note "a reply cut off at the output cap is continued"
expect_eq "$(requests cut)" "2" "the cut-off reply should be followed by exactly one more request"
expect_has "$(last_of cut)" -F "cut off at the output token limit" \
	"the request after the cut-off reply did not say why it was continued"

# ---- 3. a model that never replies ----
submit "$S146" "smoke146 never"
third="$(wait_pane 30 "$S146" -F "stopped without replying")" ||
	fail "a model that never replies did not end on the give-up notice"
dump "a model that never replies" "$third"

note "a model that never replies ends red after its continuations"
expect_eq "$(requests never)" "3" "the loop should try three times in all, then stop"
expect_eq "$(count_summaries "$(pane "$S146" -S -300)")" "2" \
	"the failed turn should end on its red notice, not on a summary"
expect_has "$third" -F "/model" "the give-up notice names no way out"

tmux kill-session -t "$S146" 2>/dev/null
