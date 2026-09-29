#!/usr/bin/env bash
# Phase 127 — the environment block names the user

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# The environment block (docs/environment.md): the real backend's system
# prompt carries `## Environment` — date, os, user, cwd — read at the
# boundary (`tui::host`) and folded in once at startup. The dummy sends no
# system prompt at all, so a local stub stands in for an OpenAI-compatible
# provider: it lists one model, streams one reply, and appends every chat
# request body to a log. One turn, then the request is read back off that
# log: the block rides the wire, in order, its `User` line naming the account
# the process runs as — the answer `id -un` gives, `uid N` for an account
# with no name, and `(root)` on a uid-0 account named anything else — so the
# agent knows whether it is root. Ctrl+D shows the same line.
S127="${S}_environment"
E_LOG="$SMOKE_TMP/requests.jsonl"
E_PORT="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
python3 - "$E_PORT" "$E_LOG" <<'PY' &
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
E_SERVER=$!
smoke_on_exit kill "$E_SERVER"
for _ in $(seq 1 30); do # wait for the stub to listen
	if python3 -c "import socket, sys; s = socket.socket(); s.settimeout(0.2); sys.exit(0 if s.connect_ex(('127.0.0.1', $E_PORT)) == 0 else 1)" 2>/dev/null; then
		break
	fi
	sleep 0.1
done
E_PROVIDERS="$SMOKE_TMP/providers.toml"
cat >"$E_PROVIDERS" <<TOML
[providers.stub]
name = "Stub"
api_key_env = "STUB_API_KEY"

[providers.stub.kwargs]
api_base = "http://127.0.0.1:$E_PORT/v1"
TOML

# The user the block must name, from `id` rather than the app's own reads:
# uid 0 is root whatever it is called, any other uid goes by its name.
E_UID="$(id -u)"
E_NAME="$(id -un 2>/dev/null)" || E_NAME="${USER:-${LOGNAME:-}}"
if [ "$E_UID" = 0 ]; then
	if [ -z "$E_NAME" ] || [ "$E_NAME" = root ]; then E_USER="root"; else E_USER="$E_NAME (root)"; fi
elif [ -n "$E_NAME" ] && [ "$E_NAME" != root ]; then
	E_USER="$E_NAME"
else
	E_USER="uid $E_UID"
fi
E_DIR="$(work_dir)"
# The app reads its cwd physically (a /tmp symlink resolves), and so must we.
E_CWD="$(cd "$E_DIR" && pwd -P)"

# NO_PROXY keeps a developer's HTTP proxy out of the loopback requests; the
# provider and model are pinned so the real backend activates with nothing
# saved.
launch -c "$E_DIR" "$S127" 100 40 "env NO_PROXY=127.0.0.1 no_proxy=127.0.0.1 ALTER_ZERO_PROVIDERS_FILE=$E_PROVIDERS ALTER_ZERO_PROVIDER=stub ALTER_ZERO_MODEL=stub-model STUB_API_KEY=smoke-127 $APP_ABS"
submit "$S127" "$USER_MSG"
replied="$(wait_pane 10 "$S127" -F "Stub reply.")" || fail "the stub's reply never streamed in"
dump "the turn against the stub" "$replied"

# The block as it went over the wire: the system message's `## Environment`
# section, cut at the next section (the scratchpad's).
block="$(python3 - "$E_LOG" <<'PY'
import json, sys
bodies = [line for line in open(sys.argv[1]) if line.strip()]
if not bodies:
    sys.exit("no chat request was logged")
system = next(m for m in json.loads(bodies[-1])["messages"] if m["role"] == "system")["content"]
if isinstance(system, list):
    system = "".join(part.get("text", "") for part in system)
start = system.find("## Environment")
if start < 0:
    sys.exit("the system prompt has no environment block")
print(system[start:].split("\n\n## ")[0])
PY
)" || fail "no environment block reached the wire"
dump "the environment block on the wire" "$block"
expect_has "$block" -xF "User $E_USER" "the block does not name the user as 'User $E_USER'"
expect_has "$block" -xF "CWD $E_CWD" "the block does not name the cwd as 'CWD $E_CWD'"
expect_has "$block" -xE 'Date [A-Z][a-z]+day [0-9]{4}-[0-9]{2}-[0-9]{2}' "the block has no weekday-and-ISO date line"
order="$(printf '%s\n' "$block" | sed -n -E 's/^(Date|OS|User|CWD) .*/\1/p' | tr '\n' ' ')"
expect_eq "$order" "Date OS User CWD " "the block's lines are out of order"

# Ctrl+D: the same line in the context the user can inspect.
keys "$S127" C-d
debug="$(wait_pane 5 "$S127" -F "User $E_USER")" || fail "Ctrl+D does not show 'User $E_USER'"
dump "Ctrl+D" "$debug"
keys "$S127" Escape
