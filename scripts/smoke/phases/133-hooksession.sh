#!/usr/bin/env bash
# Phase 133 — the session's lifecycle-hook events through a real backend

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# The session events (docs/hooks.md) need a real backend, so the app runs
# against a stub provider that refuses every request with a 400 — the turn
# fails, never retried. Every hooked event appends its payload (one JSON line)
# to a log. What it proves, in Claude Code's terms:
#   - SessionStart fires as the session opens, before any prompt, in the
#     background: a handler that sleeps does not hold up the first frame,
#     and a turn sent while it runs waits for it — its UserPromptSubmit
#     lands after it, and its context reaches the request the provider got;
#   - session_id is the conversation's own id, the one `--resume` takes: the
#     rollout file is named by it, the first prompt's payload already names
#     the transcript, and a /clear moves it on (SessionEnd names the old one,
#     SessionStart the new one);
#   - a turn that ends on an error fires StopFailure, typed by the error;
#   - a quit fires SessionEnd last, and the offline demo fires neither end;
#   - a bare --resume fires nothing while its picker is up: dismissed, the
#     fresh conversation begins (`startup`); picked, the resumed one does
#     (`resume`, under its own id) and the fresh one, never begun, gets no
#     SessionEnd.
HS_PORT="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
HS_BODIES="$SMOKE_TMP/bodies133.jsonl"
python3 - "$HS_PORT" "$HS_BODIES" <<'PY' &
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer

bodies = sys.argv[2]

class Stub(BaseHTTPRequestHandler):
    def do_GET(self):
        self.send(200, b'{"object":"list","data":[{"id":"stub-model"}]}')

    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("content-length") or 0))
        with open(bodies, "ab") as out:
            out.write(body.replace(b"\n", b" ") + b"\n")
        self.send(400, b'{"error":{"message":"smoke133 refused"}}')

    def send(self, status, data):
        self.send_response(status)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, *_args):
        pass

HTTPServer(("127.0.0.1", int(sys.argv[1])), Stub).serve_forever()
PY
HS_STUB=$!
smoke_on_exit kill "$HS_STUB"
HS_PROVIDERS="$SMOKE_TMP/providers133.toml"
printf '[providers.stub]\nname = "Stub"\n\n[providers.stub.kwargs]\napi_base = "http://127.0.0.1:%s/v1"\n' "$HS_PORT" >"$HS_PROVIDERS"

HS_LOG="$SMOKE_TMP/events133.jsonl"
HS_HOOKS="$SMOKE_TMP/hooks133.json"
hs_record="cat >> $HS_LOG; echo >> $HS_LOG"
cat >"$HS_HOOKS" <<HOOKS
{"hooks": {
  "SessionStart": [{"hooks": [{"type": "command",
    "command": "sleep 2; $hs_record; echo smoke133-session-context"}]}],
  "UserPromptSubmit": [{"hooks": [{"type": "command", "command": "$hs_record"}]}],
  "StopFailure": [{"hooks": [{"type": "command", "command": "$hs_record"}]}],
  "SessionEnd": [{"hooks": [{"type": "command", "command": "$hs_record"}]}]
}}
HOOKS
HS_ENV="$CFG_ENV_NOHIST ALTER_ZERO_HOOKS=1 ALTER_ZERO_HOOKS_FILE=$HS_HOOKS"
HS_REAL="env NO_PROXY=127.0.0.1 no_proxy=127.0.0.1 $HS_ENV ALTER_ZERO_PROVIDERS_FILE=$HS_PROVIDERS ALTER_ZERO_PROVIDER=stub ALTER_ZERO_MODEL=stub-model STUB_API_KEY=stand-in $BIN_ABS"

# hs_events — the logged events, one `Event:qualifier:session_id` per line.
hs_events() {
	[ -f "$HS_LOG" ] || return 0
	python3 - "$HS_LOG" <<'PY'
import json, sys
for line in open(sys.argv[1], encoding="utf-8"):
    line = line.strip()
    if not line:
        continue
    event = json.loads(line)
    name = event["hook_event_name"]
    qualifier = event.get("source") or event.get("reason") or event.get("error") or ""
    print(f"{name}:{qualifier}:{event['session_id']}")
PY
}
# hs_field N FIELD — FIELD of the Nth logged payload (1-based), "null" when null.
hs_field() {
	python3 - "$HS_LOG" "$1" "$2" <<'PY'
import json, sys
rows = [json.loads(l) for l in open(sys.argv[1], encoding="utf-8") if l.strip()]
value = rows[int(sys.argv[2]) - 1].get(sys.argv[3])
print("null" if value is None else value)
PY
}
hs_has() { hs_events | grep -q "^$1"; }
# hs_at_least EVENT N — N or more EVENTs logged; a function, so a poll counts
# afresh each try (an inline "$(…)" would be expanded once, before it).
hs_at_least() { [ "$(hs_events | grep -c "^$1")" -ge "$2" ]; }

S133="${S}_hooksession"
hs_launch_started="$(date +%s%N)"
launch "$S133" 100 30 "$HS_REAL"
hs_first_frame_ms=$((($(date +%s%N) - hs_launch_started) / 1000000))
note "first frame after ${hs_first_frame_ms} ms (SessionStart sleeps 2 s)"
if [ "$hs_first_frame_ms" -ge 1900 ]; then
	fail "the first frame waited on the SessionStart hook (${hs_first_frame_ms} ms)"
fi
# SessionStart fires as the session opens — no prompt sent yet.
poll 8 hs_has "SessionStart:startup" || fail "SessionStart never fired at launch (no prompt was sent)"
hs_startup="$(hs_events | sed -n 1p)"
hs_first_id="${hs_startup##*:}"
expect_ne "$(hs_field 1 scratchpad_dir)" "null" "the payload carries no scratchpad_dir"
submit "$S133" "smoke133 hello"
wait_pane 20 "$S133" -F "smoke133 refused" >/dev/null || fail "the refused turn never showed its error"
poll 10 hs_has "StopFailure" || fail "the failed turn fired no StopFailure"
note "events after the failed turn"
hs_events
expect_eq "$(hs_events | sed -n 2p)" "UserPromptSubmit::$hs_first_id" "the first turn's prompt event: on the conversation SessionStart announced"
expect_eq "$(hs_events | sed -n 3p)" "StopFailure:invalid_request:$hs_first_id" "the failed turn's StopFailure, typed by the 400"
hs_transcript="$(hs_field 2 transcript_path)"
case "$hs_transcript" in
*"$hs_first_id"*.jsonl) ;;
*) fail "the first prompt's payload does not name its transcript, by its id (got '$hs_transcript')" ;;
esac
[ -f "$hs_transcript" ] || fail "the transcript the first prompt names does not exist: $hs_transcript"
expect_file_has "$HS_BODIES" -F "smoke133-session-context" "the SessionStart context never reached the first request"

# /clear ends the conversation and begins another, with an id of its own —
# and a prompt sent while its SessionStart still sleeps waits for it.
: >"$HS_BODIES"
submit "$S133" "/clear"
sleep 0.3
submit "$S133" "smoke133 again"
wait_pane 20 "$S133" -F "smoke133 refused" >/dev/null || fail "the second refused turn never showed its error"
poll 10 hs_at_least StopFailure 2 || fail "the second failed turn fired no StopFailure"
note "events after /clear and a second turn"
hs_events
expect_eq "$(hs_events | sed -n 4p)" "SessionEnd:clear:$hs_first_id" "/clear's SessionEnd names the conversation that ended"
hs_clear="$(hs_events | sed -n 5p)"
hs_second_id="${hs_clear##*:}"
case "$hs_clear" in
SessionStart:clear:*) ;;
*) fail "the event after /clear's SessionEnd is not SessionStart(clear) (got '$hs_clear')" ;;
esac
expect_ne "$hs_second_id" "$hs_first_id" "the conversation /clear began kept the old id"
expect_eq "$(hs_events | sed -n 6p)" "UserPromptSubmit::$hs_second_id" "the prompt sent during SessionStart(clear) waited for it"
expect_file_has "$HS_BODIES" -F "smoke133-session-context" "SessionStart(clear)'s context never reached the turn that waited for it"

submit "$S133" "/quit"
wait_gone 5 "$S133" || fail "the app did not exit on /quit"
note "events after the quit"
hs_events
expect_eq "$(hs_events | tail -1)" "SessionEnd:prompt_input_exit:$hs_second_id" "the quit's SessionEnd, last, on the cleared-to conversation"
expect_eq "$(hs_events | wc -l | tr -d ' ')" "8" "events in the real-backend session"

# A bare --resume boots into its picker, and the pick is the conversation that
# begins — so nothing fires while the picker is up. Dismissed, the fresh
# conversation it lands in begins then.
rm -f "$HS_LOG"
S133P="${S}_hooksession_picker"
launch -w "R E S U M E" "$S133P" 100 30 "$HS_REAL --resume"
sleep 2.5
expect_eq "$(hs_events | wc -l | tr -d ' ')" "0" "session events while a bare --resume's picker was still up"
keys "$S133P" Escape
poll 8 hs_has "SessionStart:startup" || fail "dismissing a bare --resume's picker fired no SessionStart(startup)"
hs_fresh_id="$(hs_events | sed -n 1p)"
hs_fresh_id="${hs_fresh_id##*:}"
submit "$S133P" "/quit"
wait_gone 5 "$S133P" || fail "the app did not exit on /quit after the dismissed picker"
note "events after a dismissed picker and a quit"
hs_events
expect_eq "$(hs_events | tr '\n' ' ')" "SessionStart:startup:$hs_fresh_id SessionEnd:prompt_input_exit:$hs_fresh_id " "a dismissed picker's conversation: begun, then ended at the quit"

# Picked, the resumed conversation begins under the id `--resume` takes, and
# the fresh one the boot held never started, so no SessionEnd announces it.
rm -f "$HS_LOG"
S133R="${S}_hooksession_pick"
launch -w "R E S U M E" "$S133R" 100 30 "$HS_REAL --resume"
wait_pane 5 "$S133R" -F "smoke133 again" >/dev/null || fail "the picker never listed the recorded conversations"
keys "$S133R" Enter
poll 8 hs_has "SessionStart:resume" || fail "picking a conversation fired no SessionStart(resume)"
hs_picked="$(hs_events | sed -n 1p)"
hs_picked_id="${hs_picked##*:}"
case "$hs_picked_id" in
"$hs_first_id" | "$hs_second_id") ;;
*) fail "SessionStart(resume) names '$hs_picked_id', neither recorded conversation ($hs_first_id, $hs_second_id)" ;;
esac
submit "$S133R" "/quit"
wait_gone 5 "$S133R" || fail "the app did not exit on /quit after the pick"
note "events after a pick and a quit"
hs_events
expect_eq "$(hs_events | tr '\n' ' ')" "SessionStart:resume:$hs_picked_id SessionEnd:prompt_input_exit:$hs_picked_id " "a picked conversation: resumed under its own id, ended at the quit, no end for the fresh one"

# The offline demo runs no hooks, so neither session event fires — an end
# without its start was the old bug.
rm -f "$HS_LOG"
S133D="${S}_hooksession_dummy"
launch "$S133D" 100 30 "env $HS_ENV $BIN_ABS"
sleep 3
submit "$S133D" "/quit"
wait_gone 5 "$S133D" || fail "the offline app did not exit on /quit"
expect_eq "$(hs_events | wc -l | tr -d ' ')" "0" "session events the offline demo fired"
