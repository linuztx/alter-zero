#!/usr/bin/env bash
# Phase 151 — a subagent's SubagentStart hooks leave their cell on its own transcript

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# docs/tools/hooks.md *The hook-run cell*, through the REAL backend against a stub
# provider. The lead launches one foreground agent; two SubagentStart hooks
# run on the agent's own thread before its first round — one briefs it (plain
# stdout is that event's context), one exits 2, which has nothing to block
# there. What it proves:
#   - the agent's first request carries the briefing, wrapped and labelled,
#     and not the exit-2 reason, which is the user's alone;
#   - the agent's session view shows the `● Ran 2 SubagentStart hooks` cell
#     under the task it opens on: the context row, then the reason as a
#     message — the cell an agent used to start without, its hooks' work
#     visible nowhere;
#   - the cell is the agent's: the lead's conversation never shows it.
S151="${S}_subagenthookcell"
WORK="$(work_dir subagenthook)"
STUB_LOG="$SMOKE_TMP/stub151.log"
HELD="$SMOKE_TMP/held151"
GO="$SMOKE_TMP/go151"
STUB_PORT="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
python3 - "$STUB_PORT" "$STUB_LOG" "$HELD" "$GO" <<'PY' &
import json, os, sys, threading, time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

port, log, held, go = int(sys.argv[1]), sys.argv[2], sys.argv[3], sys.argv[4]
lock = threading.Lock()

def text(message):
    content = message.get("content")
    if isinstance(content, list):
        return " ".join(part.get("text", "") for part in content if isinstance(part, dict))
    return content or ""

def said(words):
    return [{"choices": [{"delta": {"content": words}}]}]

def finish(reason):
    return [{"choices": [{"delta": {}, "finish_reason": reason}]}]

def calling(name, args):
    call = {"index": 0, "id": "call_151", "type": "function",
            "function": {"name": name, "arguments": json.dumps(args)}}
    return [{"choices": [{"delta": {"tool_calls": [call]}}]}] + finish("tool_calls")

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
        # Only the lead is offered `agent` — subagents never nest.
        lead = any((tool.get("function") or {}).get("name") == "agent"
                   for tool in body.get("tools") or [])
        with lock, open(log, "a") as f:
            f.write(json.dumps({"lead": lead, "messages": messages}) + "\n")
        last = messages[-1]
        self.send_response(200)
        self.send_header("content-type", "text/event-stream")
        self.send_header("connection", "close")
        self.end_headers()
        if not lead:
            # Held until the script has read the agent's session view.
            open(held, "w").close()
            deadline = time.time() + 60
            while not os.path.exists(go) and time.time() < deadline:
                time.sleep(0.05)
            self.out(said("Surveyed: nothing to report."))
            self.out(finish("stop"))
        elif last.get("role") == "user" and "smoke151 brief" in text(last):
            self.out(calling("agent", {"description": "Survey the repo",
                                       "prompt": "Survey the repo.",
                                       "run_in_background": False}))
        else:
            self.out(said("All done."))
            self.out(finish("stop"))
        self.wfile.write(b"data: [DONE]\n\n")
        self.wfile.flush()

    def out(self, frames):
        for frame in frames:
            self.wfile.write(("data: %s\n\n" % json.dumps(frame)).encode())
        self.wfile.flush()

    def log_message(self, *_args):
        pass

ThreadingHTTPServer(("127.0.0.1", port), Stub).serve_forever()
PY
STUB_PID=$!
smoke_on_exit kill "$STUB_PID"
PROVIDERS="$SMOKE_TMP/providers151.toml"
cat >"$PROVIDERS" <<EOF
[providers.stub]
name = "Stub"

[providers.stub.kwargs]
api_base = "http://127.0.0.1:$STUB_PORT/v1"
EOF
HOOKS="$SMOKE_TMP/hooks151.json"
cat >"$HOOKS" <<'HOOKS'
{"hooks": {
  "SubagentStart": [{"hooks": [
    {"type": "command", "command": "echo 'smoke151 the repo root is /srv/app'"},
    {"type": "command", "command": "echo 'smoke151 no deploys on Fridays' >&2; exit 2"}]}]
}}
HOOKS
# No permission gate; NO_PROXY keeps a developer's proxy off the loopback.
APP151="env NO_PROXY=127.0.0.1 no_proxy=127.0.0.1 $CFG_ENV_NOHIST ALTER_ZERO_PERMISSIONS=0 ALTER_ZERO_HOOKS=1 ALTER_ZERO_HOOKS_FILE=$HOOKS ALTER_ZERO_PROVIDERS_FILE=$PROVIDERS ALTER_ZERO_PROVIDER=stub ALTER_ZERO_MODEL=stub-model STUB_API_KEY=stand-in $BIN_ABS"

# sa_under PANE HEADER N — the N lines right under the first line of PANE that
# contains HEADER (fixed string).
sa_under() {
	printf '%s\n' "$1" | awk -v header="$2" -v n="$3" 'index($0, header) {for (i = 0; i < n && (getline line) > 0; i++) print line; exit}'
}

launch -n -c "$WORK" "$S151" 120 50 "$APP151"
trust_hooks "$S151"
submit "$S151" "smoke151 brief"
poll 30 test -e "$HELD" || fail "the agent's first request never reached the stub"

# What the agent read: the briefing, and not the reason meant for the user.
expect_file_has "$STUB_LOG" -F "SubagentStart hook additional context: smoke151 the repo root is /srv/app" \
	"the agent never read its SubagentStart hook's context"
grep -q -F "smoke151 no deploys on Fridays" "$STUB_LOG" &&
	fail "a SubagentStart hook's exit-2 reason reached the agent"

# ---- the agent's session view: the hooks' cell under its task ----
# ↓ opens the roster on `● main`, a second ↓ steps onto the agent, Enter
# opens its session view (Phase 96's walk).
keys "$S151" Down
sleep 0.15
keys "$S151" Down
sleep 0.15
keys "$S151" Enter
wait_for 10 "$S151" -E '^❯ Survey the repo\.' || fail "the agent's session view did not open"
sa_view="$(wait_pane 10 "$S151" -S -200 -- -F "● Ran 2 SubagentStart hooks (ctrl+o to expand)")" ||
	fail "the agent's session view shows no SubagentStart cell"
dump "the agent's session view, its hooks' cell under its task" "$sa_view"
sa_rows="$(sa_under "$sa_view" "● Ran 2 SubagentStart hooks (ctrl+o to expand)" 2)"
expect_has "$(printf '%s\n' "$sa_rows" | sed -n 1p)" -F "⎿  Context added by SubagentStart hook" \
	"the cell lost its context row (saw: $sa_rows)"
expect_has "$(printf '%s\n' "$sa_rows" | sed -n 2p)" -F "⎿  smoke151 no deploys on Fridays" \
	"the exit-2 reason is not a message row under the cell (saw: $sa_rows)"
sa_order="$(printf '%s\n' "$sa_view" | grep -n -E "^❯ Survey the repo\.|● Ran 2 SubagentStart hooks" | cut -d: -f1 | tr '\n' ' ')"
read -r sa_task_at sa_cell_at _ <<<"$sa_order"
[ -n "$sa_task_at" ] && [ -n "$sa_cell_at" ] && [ "$sa_task_at" -lt "$sa_cell_at" ] ||
	fail "the SubagentStart cell is not under the agent's task (lines: $sa_order)"

touch "$GO"
wait_pane 30 "$S151" -S -300 -- -F "Surveyed: nothing to report." >/dev/null ||
	fail "the agent never answered in its session view"

# ---- back to the lead's conversation: the cell is the agent's alone ----
keys "$S151" Escape
sa_main="$(wait_settled 40 "$S151" -S -300 -- -F 'All done.')" || fail "the lead's turn never ended"
dump "the lead's conversation" "$sa_main"
expect_lacks "$sa_main" -F "SubagentStart hook" "the agent's hook cell showed on the lead's conversation"

tmux kill-session -t "$S151" 2>/dev/null || true
