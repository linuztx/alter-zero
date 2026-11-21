#!/usr/bin/env bash
# Phase 162 — --system-prompt-file and --append-system-prompt reach the wire and Ctrl+D

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# The system prompt flags (docs/system-prompt.md): launched with a persona
# read from a file and a line appended to it, the request on the wire opens
# on the file's text, then the appended line, then `## Environment` — the
# built-in persona nowhere — and Ctrl+D shows the appended line too. The
# dummy sends no system prompt, so Phase 127's stub stands in for a
# provider and logs every chat request body.
S162="${S}_systemprompt"
P162_LOG="$SMOKE_TMP/requests.jsonl"
P162_PORT="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
python3 - "$P162_PORT" "$P162_LOG" <<'PY' &
import json, sys
from http.server import BaseHTTPRequestHandler, HTTPServer

port, log = int(sys.argv[1]), sys.argv[2]

class Stub(BaseHTTPRequestHandler):
    def do_GET(self):
        # The startup probe's listing: one model, with a window.
        if self.path.startswith("/v1/models"):
            data = json.dumps({"data": [{"id": "stub-model", "context_length": 32000}]}).encode()
            self.send_response(200)
            self.send_header("content-type", "application/json")
            self.send_header("content-length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)
        else:
            self.send_response(404)
            self.end_headers()

    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("content-length") or 0))
        with open(log, "ab") as f:
            f.write(body + b"\n")
        self.send_response(200)
        self.send_header("content-type", "text/event-stream")
        self.end_headers()
        for frame in (
            {"choices": [{"index": 0, "delta": {"content": "Stub reply."}}]},
            {"choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]},
        ):
            self.wfile.write(f"data: {json.dumps(frame)}\n\n".encode())
        self.wfile.write(b"data: [DONE]\n\n")

    def log_message(self, *_args):
        pass

HTTPServer(("127.0.0.1", port), Stub).serve_forever()
PY
P162_SERVER=$!
smoke_on_exit kill "$P162_SERVER"
for _ in $(seq 1 30); do # wait for the stub to listen
	if python3 -c "import socket, sys; s = socket.socket(); s.settimeout(0.2); sys.exit(0 if s.connect_ex(('127.0.0.1', $P162_PORT)) == 0 else 1)" 2>/dev/null; then
		break
	fi
	sleep 0.1
done
P162_PROVIDERS="$SMOKE_TMP/providers.toml"
cat >"$P162_PROVIDERS" <<TOML
[providers.stub]
name = "Stub"
api_key_env = "STUB_API_KEY"

[providers.stub.kwargs]
api_base = "http://127.0.0.1:$P162_PORT/v1"
TOML

P162_DIR="$(work_dir)"
P162_FILE="$SMOKE_TMP/persona.md"
printf 'SMOKE162_PERSONA from the file.\n' >"$P162_FILE"

launch -c "$P162_DIR" "$S162" 100 40 "env NO_PROXY=127.0.0.1 no_proxy=127.0.0.1 ALTER_ZERO_PROVIDERS_FILE=$P162_PROVIDERS ALTER_ZERO_PROVIDER=stub ALTER_ZERO_MODEL=stub-model STUB_API_KEY=smoke-162 $APP_ABS --system-prompt-file $P162_FILE --append-system-prompt=SMOKE162_APPENDED"
submit "$S162" "$USER_MSG"
replied="$(wait_pane 10 "$S162" -F "Stub reply.")" || fail "the stub's reply never streamed in"
dump "the turn against the stub" "$replied"

# The persona block as it went over the wire, up to `## Environment`.
verdict="$(python3 - "$P162_LOG" 2>&1 <<'PY'
import json, sys
bodies = [line for line in open(sys.argv[1]) if line.strip()]
if not bodies:
    sys.exit("no chat request was logged")
system = next(m for m in json.loads(bodies[-1])["messages"] if m["role"] == "system")["content"]
if isinstance(system, list):
    system = "".join(part.get("text", "") for part in system)
if not system.startswith("SMOKE162_PERSONA from the file."):
    sys.exit("the system prompt does not open on the file's text:\n" + system[:400])
appended, env = system.find("SMOKE162_APPENDED"), system.find("## Environment")
if not 0 < appended < env:
    sys.exit("the appended line is not between the persona and ## Environment:\n" + system[:600])
if "You are Alter Zero" in system:
    sys.exit("the built-in persona is still there")
print(system[:env])
PY
)" || fail "the system prompt on the wire is wrong: $verdict"
dump "the persona block on the wire" "$verdict"

# Ctrl+D: the same prompt in the context the user can inspect — at the top
# of the system message, so Home, since the view opens on the newest. The
# file's line, then the appended one, and no built-in persona.
keys "$S162" C-d
wait_pane 5 "$S162" -F "C O N T E X T" >/dev/null || fail "Ctrl+D never opened"
keys "$S162" Home
debug="$(wait_pane 5 "$S162" -F "SMOKE162_APPENDED")" || fail "Ctrl+D does not show the appended line"
dump "Ctrl+D" "$debug"
expect_lacks "$debug" -F "You are Alter Zero" "Ctrl+D still shows the built-in persona"
persona_row="$(printf '%s\n' "$debug" | grep -n -F "SMOKE162_PERSONA from the file." | head -1 | cut -d: -f1)"
appended_row="$(printf '%s\n' "$debug" | grep -n -F "SMOKE162_APPENDED" | head -1 | cut -d: -f1)"
if [ -z "$persona_row" ] || [ "$persona_row" -ge "$appended_row" ]; then
	fail "Ctrl+D does not show the file's persona above the appended line (rows '$persona_row' and '$appended_row')"
fi
keys "$S162" Escape
