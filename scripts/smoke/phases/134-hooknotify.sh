#!/usr/bin/env bash
# Phase 134 — a Notification hook fires as the permission prompt reaches the screen, and keeps its place

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# A real backend — a stub provider whose "model" asks for one `bash` call —
# raises the permission prompt, and a hooks.json records PreToolUse,
# Notification and PostToolUse. What it proves (docs/hooks.md):
#   - Notification(permission_prompt) fires for the prompt on screen;
#   - it never holds the prompt up: answered at once, the command runs at
#     once, while the notifier is still asleep;
#   - it keeps its place: the call's PostToolUse waits for it, so the hooks
#     see PreToolUse, Notification, PostToolUse — the order things happened.
#     Run beside the conversation with no order, the slow notifier used to
#     land last, after the call it announced had finished.
S134="${S}_hooknotify"
WORK="$(work_dir notify)"
HN_LOG="$SMOKE_TMP/events134.jsonl"
HN_RAN="$SMOKE_TMP/ran134"
HN_PORT="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
python3 - "$HN_PORT" "$HN_RAN" <<'PY' &
import json, sys
from http.server import BaseHTTPRequestHandler, HTTPServer

port, ran = int(sys.argv[1]), sys.argv[2]

def sse(frames):
    body = "".join("data: %s\n\n" % json.dumps(f) for f in frames) + "data: [DONE]\n\n"
    return body.encode()

class Stub(BaseHTTPRequestHandler):
    def do_GET(self):
        self.send(b'{"object":"list","data":[{"id":"stub-model"}]}', "application/json")

    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("content-length") or 0)).decode()
        last = json.loads(body).get("messages", [{}])[-1]
        if last.get("role") == "user":
            args = {"command": "date +%%s%%N > %s; echo smoke134-ran" % ran,
                    "description": "Mark the run"}
            call = {"index": 0, "id": "call_134", "type": "function",
                    "function": {"name": "bash", "arguments": json.dumps(args)}}
            frames = [{"choices": [{"delta": {"tool_calls": [call]}}]},
                      {"choices": [{"delta": {}, "finish_reason": "tool_calls"}]}]
        else:
            frames = [{"choices": [{"delta": {"content": "Done."}}]},
                      {"choices": [{"delta": {}, "finish_reason": "stop"}]}]
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
HN_STUB=$!
smoke_on_exit kill "$HN_STUB"
HN_PROVIDERS="$SMOKE_TMP/providers134.toml"
printf '[providers.stub]\nname = "Stub"\n\n[providers.stub.kwargs]\napi_base = "http://127.0.0.1:%s/v1"\n' "$HN_PORT" >"$HN_PROVIDERS"

HN_HOOKS="$SMOKE_TMP/hooks134.json"
hn_record="cat >> $HN_LOG; echo >> $HN_LOG"
cat >"$HN_HOOKS" <<HOOKS
{"hooks": {
  "PreToolUse": [{"hooks": [{"type": "command", "command": "$hn_record"}]}],
  "Notification": [{"matcher": "permission_prompt",
    "hooks": [{"type": "command", "command": "sleep 4; $hn_record"}]}],
  "PostToolUse": [{"hooks": [{"type": "command", "command": "$hn_record"}]}]
}}
HOOKS
HN_APP="env NO_PROXY=127.0.0.1 no_proxy=127.0.0.1 $CFG_ENV_NOHIST ALTER_ZERO_HOOKS=1 ALTER_ZERO_HOOKS_FILE=$HN_HOOKS ALTER_ZERO_PROVIDERS_FILE=$HN_PROVIDERS ALTER_ZERO_PROVIDER=stub ALTER_ZERO_MODEL=stub-model STUB_API_KEY=stand-in $BIN_ABS"

# hn_events — the logged events' names, in the order they were recorded.
hn_events() {
	[ -f "$HN_LOG" ] || return 0
	python3 - "$HN_LOG" <<'PY'
import json, sys
for line in open(sys.argv[1], encoding="utf-8"):
    if line.strip():
        print(json.loads(line)["hook_event_name"])
PY
}
# hn_at_least N — N or more events logged; a function, so a poll counts afresh
# each try (an inline "$(…)" would be expanded once, before it).
hn_at_least() { [ "$(hn_events | wc -l | tr -d ' ')" -ge "$1" ]; }
# hn_field EVENT FIELD — FIELD of EVENT's first logged payload.
hn_field() {
	python3 - "$HN_LOG" "$1" "$2" <<'PY'
import json, sys
for line in open(sys.argv[1], encoding="utf-8"):
    if line.strip():
        row = json.loads(line)
        if row["hook_event_name"] == sys.argv[2]:
            print(row.get(sys.argv[3]))
            break
PY
}

launch -c "$WORK" "$S134" 120 40 "$HN_APP"
submit "$S134" "smoke134 notify"
hn_prompt="$(wait_pane 20 "$S134" -F "Bash command")" || fail "the permission prompt never opened"
dump "the permission prompt" "$hn_prompt"
# Answer at once — option 1, Yes — while the notifier still sleeps.
hn_answered="$(date +%s%N)"
keys "$S134" Enter
hn_settled="$(wait_settled 30 "$S134" -S -200 -- -F "Done.")" || fail "the turn never settled"
dump "the settled turn" "$hn_settled"
poll 10 hn_at_least 3 || fail "the hooks never all recorded"
note "events"
hn_events

expect_eq "$(hn_events | tr '\n' ' ')" "PreToolUse Notification PostToolUse " \
	"the call's hooks out of order — the notification must keep its place"
expect_eq "$(hn_field Notification notification_type)" "permission_prompt" "the notification's type"
expect_eq "$(hn_field Notification message)" "Alter Zero needs your permission to use Bash" \
	"the notification's message"
[ -s "$HN_RAN" ] || fail "the approved command never ran"
hn_ran="$(cat "$HN_RAN" 2>/dev/null || echo 0)"
hn_delay_ms=$(((hn_ran - hn_answered) / 1000000))
note "the command ran ${hn_delay_ms} ms after the answer (the notifier sleeps 4 s)"
if [ "$hn_delay_ms" -ge 2000 ]; then
	fail "the notifier held the prompt up: the command ran ${hn_delay_ms} ms after the answer"
fi
