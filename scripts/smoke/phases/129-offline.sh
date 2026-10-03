#!/usr/bin/env bash
# Phase 129 — a provider that cannot be reached is waited for, and the turn resumes when it is back

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# The lost-connection wait (docs/offline.md). A provider file points the real
# backend at a loopback port nothing listens on, so every connect is refused
# at once — the shape a provider gives from a machine with no route to it.
# The turn must not fail: the status line wears the amber `Waiting for
# internet…` verb with its `offline for Ns` clause, and the `No connection to
# 127.0.0.1:{port} — trying again · N attempts` row hangs under it, the count
# ticking as the backend keeps trying. Then a stub chat-completions server
# starts on that port — the network "comes back" — and the very next attempt
# gets through: the reply streams and the turn settles like any other, every
# trace of the wait gone. A second half interrupts a wait with Esc: nothing
# streamed, so the submission is undone and the message is back in the
# composer with no notice (docs/interrupt.md).
S129="${S}_offline"
OFF_CFG="$(mktemp -d "$SMOKE_TMP/offline-cfg.XXXXXX")"
OFF_DIR="$(work_dir offline)"
# A port that is free now — and so refuses connects until the stub binds it.
OFF_PORT="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
cat >"$OFF_CFG/providers.toml" <<PROVIDERS
[providers.flaky]
name = "Flaky"
api_model_base = "http://127.0.0.1:$OFF_PORT/v1"

[providers.flaky.kwargs]
api_base = "http://127.0.0.1:$OFF_PORT/v1"
PROVIDERS
# A key resolves for the provider and a selection is saved (Phase 118's
# trick), so the launch activates the real backend on the stub's model.
cat >"$OFF_CFG/config.json" <<'CONFIG'
{ "provider": "flaky", "model": "stub-model" }
CONFIG
# The loopback request must not be sent to a developer's HTTPS proxy.
OFF_ENV="-u HTTPS_PROXY -u https_proxy -u HTTP_PROXY -u http_proxy -u ALL_PROXY -u all_proxy ALTER_ZERO_TELEMETRY=0 ALTER_ZERO_UPDATE_CHECK=0 ALTER_ZERO_PROJECT_CONFIG=0 ALTER_ZERO_CONFIG_DIR=$OFF_CFG ALTER_ZERO_SESSIONS_DIR=$OFF_CFG/sessions ALTER_ZERO_HISTORY_FILE=/dev/null ALTER_ZERO_CHECKPOINTS=0 ALTER_ZERO_SKILLS_DIR=$SMOKE_SKILLS ALTER_ZERO_AGENTS_DIR=$SMOKE_AGENTS ALTER_ZERO_PROVIDERS_FILE=$OFF_CFG/providers.toml ALTER_ZERO_API_KEY=not-a-real-key ALTER_ZERO_TIPS=0"
APP_OFF="env $OFF_ENV $BIN_ABS"

# ---- half one: the wait, then the provider comes back ----
launch -c "$OFF_DIR" "$S129" 100 30 "$APP_OFF"
up="$(wait_pane 5 "$S129" -F "stub-model")" ||
	fail "the footer never named the stub model (the real backend did not activate)"
submit "$S129" "$USER_MSG"
waiting="$(wait_pane 10 "$S129" -F "Waiting for internet…")" ||
	fail "the status line never showed the wait (did the refused connect fail the turn instead?)"
dump "the wait, first announcement" "$waiting"
# The second attempt lands after the 1s backoff: the count ticks on while
# the start of the outage holds.
ticking="$(wait_pane 10 "$S129" -E "· [2-9][0-9]* attempts")" ||
	fail "the attempt count never moved on from 1"
dump "the wait, a later attempt" "$ticking"
note "the stub provider comes up on port $OFF_PORT"
python3 - "$OFF_PORT" <<'PY' &
import json, sys
from http.server import BaseHTTPRequestHandler, HTTPServer

port = int(sys.argv[1])

def chunk(text, finish=None):
    return "data: " + json.dumps({
        "id": "stub", "object": "chat.completion.chunk", "model": "stub-model",
        "choices": [{"index": 0, "delta": {"role": "assistant", "content": text}, "finish_reason": finish}],
    }) + "\n\n"

class Stub(BaseHTTPRequestHandler):
    def do_POST(self):
        self.rfile.read(int(self.headers.get("content-length") or 0))
        self.send_response(200)
        self.send_header("content-type", "text/event-stream")
        self.end_headers()
        for piece in ["Back ", "online — ", "the reply ", "streams ", "as before."]:
            self.wfile.write(chunk(piece).encode())
            self.wfile.flush()
        self.wfile.write(chunk("", "stop").encode())
        self.wfile.write(b"data: [DONE]\n\n")
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
OFF_SERVER=$!
smoke_on_exit "kill $OFF_SERVER 2>/dev/null"
for _ in $(seq 1 30); do # wait for the stub to listen
	python3 -c "import socket, sys; s = socket.socket(); s.settimeout(0.2); sys.exit(0 if s.connect_ex(('127.0.0.1', $OFF_PORT)) == 0 else 1)" && break
	sleep 0.1
done
# The next attempt gets through (at most the 5s cap away): the reply streams
# and the turn settles as any other.
resumed="$(wait_pane 20 "$S129" -F "streams as before.")" ||
	fail "the reply never streamed once the provider was back"
settled="$(wait_pane 10 "$S129" -E "^$SUMMARY_RE")" ||
	fail "the turn never settled after the reply"
dump "the provider is back; the turn settled" "$settled"

note "the wait — the verb, the clause, the row, the count, and the recovery"
expect_has "$waiting" -E "Waiting for internet… \([0-9]+s · ↑ [0-9]+ tokens · offline for [0-9]+s · esc to interrupt\)" \
	"the status line does not read as the wait with its offline clause"
expect_lacks "$waiting" -F "Working…" "the turn's own verb showed beside the wait"
expect_has "$waiting" -E "⎿ +\(*·\)* +No connection to 127\.0\.0\.1:$OFF_PORT — trying again · 1 attempt$" \
	"the row under the line does not name the host with its first attempt"
expect_lacks "$waiting" -F "retrying" "a lost connection was counted as a bounded retry"
expect_lacks "$waiting" -F "request failed" "the refused connect failed the turn"
expect_has "$ticking" -E "No connection to 127\.0\.0\.1:$OFF_PORT — trying again · [2-9][0-9]* attempts" \
	"the attempt count did not tick"
expect_has "$resumed" -F "Back online — the reply streams as before." "the reply did not stream whole"
expect_lacks "$settled" -F "Waiting for internet" "the wait outlived the request that got through"
expect_lacks "$settled" -F "No connection to" "the row outlived the request that got through"
expect_lacks "$settled" -F "Conversation interrupted" "the recovery was reported as an interrupt"
expect_lacks "$settled" -F "could not reach" "the wait surfaced as an error"
kill "$OFF_SERVER" 2>/dev/null
tmux kill-session -t "$S129" 2>/dev/null

# ---- half two: Esc while waiting undoes the submission ----
S129B="${S}_offline_esc"
OFF_PORT_B="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
cat >"$OFF_CFG/providers.toml" <<PROVIDERS
[providers.flaky]
name = "Flaky"
api_model_base = "http://127.0.0.1:$OFF_PORT_B/v1"

[providers.flaky.kwargs]
api_base = "http://127.0.0.1:$OFF_PORT_B/v1"
PROVIDERS
launch -c "$OFF_DIR" "$S129B" 100 30 "$APP_OFF"
wait_for 5 "$S129B" -F "stub-model" || fail "the second launch never named the stub model"
submit "$S129B" "is anyone there"
waiting_b="$(wait_pane 10 "$S129B" -F "Waiting for internet…")" ||
	fail "the second wait never showed"
keys "$S129B" Escape
undone="$(wait_settled 10 "$S129B" -F "❯ is anyone there")" ||
	fail "Esc did not hand the unsent message back to the composer"
dump "Esc mid-wait: the submission undone" "$undone"
note "Esc while waiting — the undo, no notice, no trace of the wait"
expect_has "$undone" -F "❯ is anyone there" "the message is not back in the composer"
expect_lacks "$undone" -F "Conversation interrupted" "a notice was committed for a turn that streamed nothing"
expect_lacks "$undone" -F "Waiting for internet" "the status line outlived the interrupt"
expect_lacks "$undone" -F "No connection to" "the row outlived the interrupt"
expect_lacks "$undone" -F "esc to interrupt" "the turn is still in flight after Esc"
expect_eq "$(count_msg_lines "$undone" "is anyone there")" "1" "the message should appear exactly once: in the composer, not as a committed bubble too"
