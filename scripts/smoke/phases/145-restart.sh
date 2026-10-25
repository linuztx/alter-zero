#!/usr/bin/env bash
# Phase 145 — a reply whose connection drops mid-stream is restarted, and lands once

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# A connection lost mid-reply (docs/offline.md). A peer that stops answering
# ends in `Connection timed out (os error 110)` on the next read, once the
# kernel's keepalive gives up on it — and such a drop used to end the turn
# with a red `request failed: …` under half a reply. Now the request goes out
# again from the start, and what the dropped attempt streamed is taken back
# first, so the reply lands exactly once.
#
# A stub chat-completions server follows a plan, one file per request: what
# to stream, how long to hold the request first, and whether to drop it. A
# dropping request waits, after streaming, until this phase has seen its rows
# on screen — committed to scrollback, which the restart must purge, not
# just a preview row — and then resets the connection: the same failed body
# read as the timeout, without the minute of keepalive probes. Holding the
# re-sent request a few seconds is what lets the retry clause be read.
#
# Half one drops twice in one turn, once over committed reply rows and once
# over a settled `Thought for …` cell (taken back out of history), then
# lands the reply. Half two drops under the Ctrl+O overlay, where the purge
# must wait for the overlay to close. The checks read the scrollback too.
RS_CFG="$(mktemp -d "$SMOKE_TMP/restart-cfg.XXXXXX")"
RS_DIR="$(work_dir restart)"
RS_STUB="$SMOKE_TMP/restart-stub.py"
cat >"$RS_STUB" <<'PY'
import json, os, socket, struct, sys, time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

port, plan = int(sys.argv[1]), sys.argv[2]
posts = 0

def frame(delta, finish=None):
    return ("data: " + json.dumps({
        "id": "stub", "object": "chat.completion.chunk", "model": "stub-model",
        "choices": [{"index": 0, "delta": dict(role="assistant", **delta), "finish_reason": finish}],
    }) + "\n\n").encode()

class Stub(BaseHTTPRequestHandler):
    def do_POST(self):
        global posts
        posts += 1
        self.rfile.read(int(self.headers.get("content-length") or 0))
        with open(os.path.join(plan, f"post{posts}.json")) as f:
            step = json.load(f)
        time.sleep(step.get("hold", 0))
        self.send_response(200)
        self.send_header("content-type", "text/event-stream")
        self.end_headers()
        for piece in step.get("reasoning", []):
            self.wfile.write(frame({"reasoning_content": piece}))
            self.wfile.flush()
        for piece in step.get("content", []):
            self.wfile.write(frame({"content": piece}))
            self.wfile.flush()
        drop = step.get("drop")
        if drop:
            while not os.path.exists(drop):
                time.sleep(0.05)
            # Reset, not close: the response is HTTP/1.0, so a close would
            # end its body cleanly; a reset fails the read mid-body.
            self.connection.setsockopt(socket.SOL_SOCKET, socket.SO_LINGER, struct.pack("ii", 1, 0))
            self.connection.close()
            return
        self.wfile.write(frame({"content": ""}, "stop"))
        self.wfile.write(b"data: [DONE]\n\n")
        self.wfile.flush()

    def do_GET(self):
        data = json.dumps({"data": [{"id": "stub-model"}]}).encode()
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def finish(self):
        # A reset connection has nothing left to flush.
        try:
            super().finish()
        except (OSError, ValueError):
            pass

    def log_message(self, *_args):
        pass

ThreadingHTTPServer(("127.0.0.1", port), Stub).serve_forever()
PY

# restart_provider PORT — point the config home at a stub on PORT: a key
# resolves for the provider and a selection is saved (Phase 118's trick), so
# the launch activates the real backend on the stub's model.
restart_provider() {
	cat >"$RS_CFG/providers.toml" <<PROVIDERS
[providers.flaky]
name = "Flaky"
api_model_base = "http://127.0.0.1:$1/v1"

[providers.flaky.kwargs]
api_base = "http://127.0.0.1:$1/v1"
PROVIDERS
	cat >"$RS_CFG/config.json" <<'CONFIG'
{ "provider": "flaky", "model": "stub-model" }
CONFIG
}
# restart_stub PORT PLAN — start the stub on PORT serving PLAN/post{N}.json to
# the Nth request, and wait for it to listen.
restart_stub() {
	python3 "$RS_STUB" "$1" "$2" &
	smoke_on_exit "kill $! 2>/dev/null"
	for _ in $(seq 1 30); do
		python3 -c "import socket, sys; s = socket.socket(); s.settimeout(0.2); sys.exit(0 if s.connect_ex(('127.0.0.1', $1)) == 0 else 1)" && break
		sleep 0.1
	done
}
free_port() { python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])'; }

# The loopback request must not be sent to a developer's HTTPS proxy.
RS_ENV="-u HTTPS_PROXY -u https_proxy -u HTTP_PROXY -u http_proxy -u ALL_PROXY -u all_proxy ALTER_ZERO_TELEMETRY=0 ALTER_ZERO_UPDATE_CHECK=0 ALTER_ZERO_PROJECT_CONFIG=0 ALTER_ZERO_CONFIG_DIR=$RS_CFG ALTER_ZERO_SESSIONS_DIR=$RS_CFG/sessions ALTER_ZERO_HISTORY_FILE=/dev/null ALTER_ZERO_CHECKPOINTS=0 ALTER_ZERO_SKILLS_DIR=$SMOKE_SKILLS ALTER_ZERO_AGENTS_DIR=$SMOKE_AGENTS ALTER_ZERO_PROVIDERS_FILE=$RS_CFG/providers.toml ALTER_ZERO_API_KEY=not-a-real-key ALTER_ZERO_TIPS=0"
APP_RS="env $RS_ENV $BIN_ABS"

# ---- half one: two drops in one turn, then the reply ----
S145="${S}_restart"
PLAN1="$(mktemp -d "$SMOKE_TMP/restart-plan.XXXXXX")"
cat >"$PLAN1/post1.json" <<JSON
{"content": ["Dropped draft ", "line one.\n", "Dropped draft line two.\n", "Dropped half"],
 "drop": "$PLAN1/drop1"}
JSON
cat >"$PLAN1/post2.json" <<JSON
{"hold": 3, "reasoning": ["Dropped thought, ", "weighed at length."],
 "content": ["Second draft line.\n", "Second half"], "drop": "$PLAN1/drop2"}
JSON
cat >"$PLAN1/post3.json" <<'JSON'
{"hold": 3, "content": ["The restarted ", "reply streams ", "once."]}
JSON
PORT1="$(free_port)"
restart_provider "$PORT1"
restart_stub "$PORT1" "$PLAN1"
launch -c "$RS_DIR" "$S145" 100 30 "$APP_RS"
wait_for 5 "$S145" -F "stub-model" ||
	fail "the footer never named the stub model (the real backend did not activate)"
submit "$S145" "$USER_MSG"
# The third line's start means the first two are committed rows.
first="$(wait_pane 10 "$S145" -F "Dropped half")" ||
	fail "the first attempt's partial reply never streamed"
dump "the first attempt, mid-reply" "$first"
note "the stub resets the first attempt's connection"
touch "$PLAN1/drop1"
restart1="$(wait_pane 10 "$S145" -S -200 -- -F "retrying 1/3")" ||
	fail "the status line never said the request was being retried (did the drop fail the turn?)"
dump "the first restart, screen and scrollback" "$restart1"
second="$(wait_pane 10 "$S145" -F "Second half")" ||
	fail "the second attempt's partial reply never streamed"
dump "the second attempt: a settled thought and a reply, mid-reply" "$second"
note "the stub resets the second attempt's connection"
touch "$PLAN1/drop2"
restart2="$(wait_pane 10 "$S145" -S -200 -- -F "retrying 2/3")" ||
	fail "the second drop was never retried"
dump "the second restart, screen and scrollback" "$restart2"
settled="$(wait_pane 20 "$S145" -S -200 -- -E "^$SUMMARY_RE")" ||
	fail "the turn never settled after the restarted reply"
dump "the restarted reply settled, screen and scrollback" "$settled"

note "two drops — each taken back, retried, and the reply lands once"
expect_has "$first" -F "Dropped draft line one." "the first attempt's reply never reached the screen"
expect_lacks "$restart1" -F "Dropped draft" "the first attempt's committed rows outlived its restart"
expect_lacks "$restart1" -F "request failed" "the first drop failed the turn"
expect_has "$second" -F "Thought for" "the second attempt's thought never settled into its cell"
expect_has "$second" -F "Second draft line." "the second attempt's reply never reached the screen"
expect_lacks "$restart2" -F "Thought for" "the second attempt's settled thought outlived its restart"
expect_lacks "$restart2" -F "Second draft" "the second attempt's committed rows outlived its restart"
expect_has "$settled" -F "The restarted reply streams once." "the restarted reply did not stream whole"
expect_eq "$(grep -cF "The restarted reply streams once." <<<"$settled")" "1" \
	"the restarted reply should land exactly once"
expect_lacks "$settled" -E "Dropped|Second draft|Thought for" "a dropped attempt came back with the reply"
expect_lacks "$settled" -F "request failed" "a drop surfaced as an error"
expect_lacks "$settled" -F "Conversation interrupted" "a restart was reported as an interrupt"
expect_lacks "$settled" -F "retrying" "the retry clause outlived the reply that got through"
expect_eq "$(count_msg_lines "$settled" "$USER_MSG")" "1" "the purge-repaints should keep the user's message exactly once"
tmux kill-session -t "$S145" 2>/dev/null

# ---- half two: a drop under the Ctrl+O overlay ----
S145B="${S}_restart_overlay"
PLAN2="$(mktemp -d "$SMOKE_TMP/restart-plan.XXXXXX")"
cat >"$PLAN2/post1.json" <<JSON
{"content": ["Overlaid draft ", "line one.\n", "Overlaid draft line two.\n", "Overlaid half"],
 "drop": "$PLAN2/drop"}
JSON
cat >"$PLAN2/post2.json" <<'JSON'
{"hold": 1, "content": ["Back from ", "the overlay ", "once."]}
JSON
PORT2="$(free_port)"
restart_provider "$PORT2"
restart_stub "$PORT2" "$PLAN2"
launch -c "$RS_DIR" "$S145B" 100 30 "$APP_RS"
wait_for 5 "$S145B" -F "stub-model" || fail "the second launch never named the stub model"
submit "$S145B" "$USER_MSG"
overlaid="$(wait_pane 10 "$S145B" -F "Overlaid half")" ||
	fail "the overlay half's partial reply never streamed"
dump "before the overlay, mid-reply" "$overlaid"
keys "$S145B" C-o
wait_for 5 "$S145B" -F "T R A N S C R I P T" || fail "the Ctrl+O overlay never opened"
note "the stub resets the connection under the overlay"
touch "$PLAN2/drop"
under="$(wait_pane 10 "$S145B" -F "Back from the overlay once.")" ||
	fail "the restarted reply never reached the overlay's transcript"
dump "the restarted reply, in the overlay" "$under"
keys "$S145B" C-o
returned="$(wait_pane 10 "$S145B" -S -200 -- -E "^$SUMMARY_RE")" ||
	fail "the inline view never showed the settled turn after the overlay closed"
dump "back from the overlay, screen and scrollback" "$returned"

note "a drop under the overlay — purged on its return"
expect_has "$overlaid" -F "Overlaid draft line one." "the overlay half's reply never reached the screen"
expect_lacks "$returned" -F "Overlaid" "the dropped attempt's rows outlived the overlay's return"
expect_eq "$(grep -cF "Back from the overlay once." <<<"$returned")" "1" \
	"the restarted reply should land exactly once after the overlay"
expect_lacks "$returned" -F "request failed" "the drop under the overlay surfaced as an error"
expect_eq "$(count_msg_lines "$returned" "$USER_MSG")" "1" "the overlay's return should keep the user's message exactly once"
