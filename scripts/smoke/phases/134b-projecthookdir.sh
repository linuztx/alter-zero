#!/usr/bin/env bash
# Phase 134b — a trusted project hook reached through $ALTER_ZERO_PROJECT_DIR runs from a subdirectory

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# A project's own hooks.json reaches its scripts the documented way —
# "${ALTER_ZERO_PROJECT_DIR}/.alter-zero/hooks/guard.sh" — and the session is
# launched in a SUBDIRECTORY of the repository (docs/hooks.md). The variable
# used to be the cwd, so the command named a script that does not exist, the
# hook failed open and the refused command ran. What it proves:
#   - $ALTER_ZERO_PROJECT_DIR is the project root (the root the project layer
#     reads `.alter-zero/` from), while the hook still runs in the cwd;
#   - the guard's block holds: `rm -rf build/` never runs;
#   - what the model reads names the hook and the trusted project's own
#     hooks.json it is configured in, and so does the refusal's cell — the
#     file named from the phase's home, `~/projrepo/…` (docs/hooks.md *What
#     the user sees*).
# A stub provider asks for that one `bash` call and records the tool result
# the model is sent back.
S134B="${S}_projecthookdir"
# Canonical: the app resolves the cwd it is launched in, and this path is what
# the hook's own message quotes back (on macOS `$TMPDIR` is `/var/...` behind
# `/private/var/...`, so a name spelled the other way never matches the row).
REPO="$(smoke_real_path "$(work_dir projrepo)")"
git -C "$REPO" init -q
mkdir -p "$REPO/.alter-zero/hooks" "$REPO/src/build"
: >"$REPO/src/build/keep"
cat >"$REPO/.alter-zero/hooks/guard.sh" <<'SH'
#!/bin/sh
grep -q 'rm -rf' && { echo "no destructive deletes (ran in $PWD)" >&2; exit 2; }
exit 0
SH
chmod +x "$REPO/.alter-zero/hooks/guard.sh"
cat >"$REPO/.alter-zero/hooks.json" <<'JSON'
{"hooks": {"PreToolUse": [{"matcher": "Bash", "hooks": [
  {"type": "command", "command": "${ALTER_ZERO_PROJECT_DIR}/.alter-zero/hooks/guard.sh"}]}]}}
JSON

PH_BODIES="$SMOKE_TMP/tool-results134b.jsonl"
PH_PORT="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
python3 - "$PH_PORT" "$PH_BODIES" <<'PY' &
import json, sys
from http.server import BaseHTTPRequestHandler, HTTPServer

port, results = int(sys.argv[1]), sys.argv[2]

def sse(frames):
    body = "".join("data: %s\n\n" % json.dumps(f) for f in frames) + "data: [DONE]\n\n"
    return body.encode()

class Stub(BaseHTTPRequestHandler):
    def do_GET(self):
        self.send(b'{"object":"list","data":[{"id":"stub-model"}]}', "application/json")

    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("content-length") or 0)).decode()
        last = json.loads(body).get("messages", [{}])[-1]
        if last.get("role") == "tool":
            with open(results, "a", encoding="utf-8") as f:
                f.write(json.dumps({"content": last.get("content")}) + "\n")
            frames = [{"choices": [{"delta": {"content": "Done."}}]},
                      {"choices": [{"delta": {}, "finish_reason": "stop"}]}]
        else:
            call = {"index": 0, "id": "call_134b", "type": "function",
                    "function": {"name": "bash",
                                 "arguments": json.dumps({"command": "rm -rf build/"})}}
            frames = [{"choices": [{"delta": {"tool_calls": [call]}}]},
                      {"choices": [{"delta": {}, "finish_reason": "tool_calls"}]}]
        self.send(sse(frames), "text/event-stream")

    def send(self, data, kind):
        self.send_response(200)
        self.send_header("content-type", kind)
        self.send_header("content-length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, *_args):
        pass

HTTPServer(("127.0.0.1", port), Stub).serve_forever()
PY
PH_STUB=$!
smoke_on_exit kill "$PH_STUB"
PH_PROVIDERS="$SMOKE_TMP/providers134b.toml"
printf '[providers.stub]\nname = "Stub"\n\n[providers.stub.kwargs]\napi_base = "http://127.0.0.1:%s/v1"\n' "$PH_PORT" >"$PH_PROVIDERS"
# The project layer on (the base env turns it off for hermeticity), no user
# hooks file, and `master` so the command would run unasked if the hook let it.
PH_APP="env NO_PROXY=127.0.0.1 no_proxy=127.0.0.1 $CFG_ENV_NOHIST ALTER_ZERO_PROJECT_CONFIG=1 ALTER_ZERO_HOOKS=1 ALTER_ZERO_HOOKS_FILE=$SMOKE_TMP/no-user-hooks.json ALTER_ZERO_PROVIDERS_FILE=$PH_PROVIDERS ALTER_ZERO_PROVIDER=stub ALTER_ZERO_MODEL=stub-model STUB_API_KEY=stand-in ALTER_ZERO_PERMISSION_MODE=master $BIN_ABS"

# The project's hook is trusted at the startup review first
# (docs/config-review.md), launched in the subdirectory.
launch -n -c "$REPO/src" "$S134B" 120 40 "$PH_APP"
trust_hooks "$S134B"
submit "$S134B" "clean the build"
ph_pane="$(wait_settled 30 "$S134B" -S -200 -- -F "Done.")" || fail "the turn never settled"
dump "the turn, launched in $REPO/src" "$ph_pane"
# The cell wraps the message, so the path it quotes is asserted on its own:
# a single-line match only holds while the temp path is short enough.
expect_has "$ph_pane" -F "Blocked by PreToolUse hook: no destructive deletes (ran in" \
	"the project's guard never ran from the subdirectory, or did not run in the cwd"
expect_has "$ph_pane" -F "$REPO/src)" "the hook's cell does not name the cwd it ran in"
expect_has "$ph_pane" -F 'Hook: ${ALTER_ZERO_PROJECT_DIR}/.alter-zero/hooks/guard.sh' \
	"the refusal cell did not name the project's hook"
expect_has "$ph_pane" -F "Config: ~/projrepo/.alter-zero/hooks.json" \
	"the refusal cell did not name the project's hooks.json from the user's home"
[ -e "$REPO/src/build/keep" ] || fail "the refused rm -rf ran: the project hook did not guard it"
ph_result="$(python3 -c 'import json,sys; [print(json.loads(l)["content"]) for l in open(sys.argv[1])]' "$PH_BODIES" 2>/dev/null)"
dump "what the model read back" "$ph_result"
expect_has "$ph_result" -F "Blocked by PreToolUse hook: no destructive deletes" "the model was not told the hook refused"
expect_has "$ph_result" -F 'Hook: ${ALTER_ZERO_PROJECT_DIR}/.alter-zero/hooks/guard.sh' "the model was not told which hook refused"
expect_has "$ph_result" -F "Config: $REPO/.alter-zero/hooks.json" "the model was not told the project hooks.json the hook is configured in"
tmux kill-session -t "$S134B" 2>/dev/null || true
