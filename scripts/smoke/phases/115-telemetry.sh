#!/usr/bin/env bash
# Phase 115 — telemetry

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# telemetry — one anonymous ping a day, disclosed once, off with one setting (docs/telemetry.md)
# A local Python stub stands in for the collector: it appends every POST it
# receives (path, user-agent, body) to a log and answers 204. The launches
# below UNSET the suite-wide ALTER_ZERO_TELEMETRY=0 (and a developer's own
# DO_NOT_TRACK) and point the app at the stub, each against a fresh config
# home — so each is that "install"'s first launch ever. NO_PROXY keeps a
# developer's HTTP proxy out of the loopback request.
S115="${S}_telemetry"
TM_CFG="$(mktemp -d "$SMOKE_TMP/telemetry.XXXXXX")"
TM_LOG="$TM_CFG/pings.log"
TM_PORT="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
python3 - "$TM_PORT" "$TM_LOG" <<'PY' &
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer

port, log = int(sys.argv[1]), sys.argv[2]

class Stub(BaseHTTPRequestHandler):
    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("content-length") or 0)).decode()
        with open(log, "a") as f:
            f.write(f"{self.path} {self.headers.get('user-agent') or '-'} {body}\n")
        self.send_response(204)
        self.end_headers()

    def log_message(self, *_args):
        pass

HTTPServer(("127.0.0.1", port), Stub).serve_forever()
PY
TM_SERVER=$!
for _ in $(seq 1 30); do # wait for the stub to listen
	if python3 -c "import socket, sys; s = socket.socket(); s.settimeout(0.2); sys.exit(0 if s.connect_ex(('127.0.0.1', $TM_PORT)) == 0 else 1)" 2>/dev/null; then
		break
	fi
	sleep 0.1
done
TM_BASE="env -u ALTER_ZERO_TELEMETRY -u DO_NOT_TRACK NO_PROXY=127.0.0.1 no_proxy=127.0.0.1 ALTER_ZERO_TELEMETRY_URL=http://127.0.0.1:$TM_PORT/v1/ping ALTER_ZERO_PROJECT_CONFIG=0 ALTER_ZERO_CHECKPOINTS=0 ALTER_ZERO_HISTORY_FILE=/dev/null ALTER_ZERO_SKILLS_DIR=$SMOKE_SKILLS ALTER_ZERO_AGENTS_DIR=$SMOKE_AGENTS ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS"
TM_NOTICE="sends one anonymous ping a day"
TM_VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
tm_quit() {
	submit "$S115" "/quit"
	sleep 0.6
	tmux kill-session -t "$S115" 2>/dev/null
}
# A top-level field of telemetry.json (pretty JSON, one field per line),
# quotes stripped: `tm_field FILE KEY`.
tm_field() {
	sed -n -E "s/^ *\"$2\": *(.*)$/\1/p" "$1" 2>/dev/null | sed -E 's/,$//; s/^"//; s/"$//' | head -1
}
tm_pings() {
	if [ -f "$TM_LOG" ]; then wc -l <"$TM_LOG" | tr -d ' '; else echo 0; fi
}
tm_fail() {
	fail "$1"
}
# Open /settings filtered to the Telemetry row: `tm_open_row`.
tm_open_row() {
	tmux send-keys -t "$S115" -l "/settings"
	sleep 0.3
	tmux send-keys -t "$S115" Enter
	sleep 0.5
	tmux send-keys -t "$S115" -l "telem"
	sleep 0.4
}

# (a) The first launch ever for this config home: the disclosure under the
# banner, an install id minted, exactly one five-field ping posted with the
# alter-zero user-agent, and the day recorded once the stub answered 204.
tmux new-session -d -s "$S115" -x 100 -y 30 "$TM_BASE ALTER_ZERO_CONFIG_DIR=$TM_CFG/home $BIN"
tm_first=""
for _ in $(seq 1 60); do # up to ~6s: the ping is a real (loopback) request
	tm_first="$(tmux capture-pane -t "$S115" -p -S -40)"
	if [ "$(tm_field "$TM_CFG/home/telemetry.json" last_ping_day)" = "$(date -u +%Y-%m-%d)" ]; then
		break
	fi
	sleep 0.1
done
echo "==== Phase 115: the first launch (notice + ping) ===="
printf '%s\n' "$tm_first"
echo "==== Phase 115: telemetry.json + the stub's log ===="
cat "$TM_CFG/home/telemetry.json" 2>/dev/null
cat "$TM_LOG" 2>/dev/null
if ! printf '%s' "$tm_first" | grep -qF "$TM_NOTICE"; then
	tm_fail "the first launch did not show the disclosure under the banner"
fi
if ! printf '%s' "$tm_first" | grep -qF "ALTER_ZERO_TELEMETRY=0"; then
	tm_fail "the notice lost its ending — clamped instead of wrapped?"
fi
tm_id="$(tm_field "$TM_CFG/home/telemetry.json" install_id)"
if ! printf '%s' "$tm_id" | grep -qE '^[0-9a-f]{32}$'; then
	tm_fail "no 32-hex install id in telemetry.json (got '$tm_id')"
fi
if [ "$(tm_field "$TM_CFG/home/telemetry.json" last_ping_day)" != "$(date -u +%Y-%m-%d)" ]; then
	tm_fail "last_ping_day was not recorded as today after the stub's 204"
fi
if [ "$(tm_field "$TM_CFG/home/telemetry.json" notice_shown)" != "true" ]; then
	tm_fail "notice_shown was not recorded"
fi
if [ "$(tm_field "$TM_CFG/home/telemetry.json" enabled)" != "true" ]; then
	tm_fail "enabled should read true after a default first launch"
fi
if [ "$(tm_pings)" != "1" ]; then
	tm_fail "expected exactly one ping in the stub's log, got $(tm_pings)"
fi
tm_line="$(head -1 "$TM_LOG" 2>/dev/null)"
case "$tm_line" in
"/v1/ping alter-zero/$TM_VERSION "*) ;;
*) tm_fail "the ping did not hit /v1/ping with the alter-zero/$TM_VERSION user-agent: '$tm_line'" ;;
esac
tm_body="${tm_line#/v1/ping alter-zero/$TM_VERSION }"
# The body is the seven fields and nothing else — the shape docs/telemetry.md
# promises — with the id the file holds and the crate's own version. The suite
# runs on Linux, so `distro` must be there; `os_version` is checked only if
# present, because a rolling-release host (Arch, Void) legitimately names none.
if ! python3 - "$tm_body" "$tm_id" "$TM_VERSION" <<'PY'; then
import json, re, sys
body, install_id, version = json.loads(sys.argv[1]), sys.argv[2], sys.argv[3]
required = {"arch", "distro", "id", "os", "v", "version"}
assert required <= set(body) <= required | {"os_version"}, body
assert body["v"] == 2, body
assert body["id"] == install_id, body
assert body["version"] == version, body
assert re.fullmatch(r"[a-z0-9_]{1,16}", body["os"]), body
assert re.fullmatch(r"[a-z0-9_]{1,16}", body["arch"]), body
assert re.fullmatch(r"[a-z0-9._-]{1,32}", body["distro"]), body
if "os_version" in body:
    assert re.fullmatch(r"[a-z0-9][a-z0-9._-]{0,15}", body["os_version"]), body
PY
	tm_fail "the ping body is not the promised seven-field shape: '$tm_body'"
fi
tm_quit

# (b) The relaunch on the same config home: no notice a second time, no
# second ping today — and the /settings row toggles the switch into
# telemetry.json (never settings.json), turning it back on re-pinging nothing
# because today's ping is already recorded.
tmux new-session -d -s "$S115" -x 100 -y 30 "$TM_BASE ALTER_ZERO_CONFIG_DIR=$TM_CFG/home $BIN"
sleep 1.5
tm_second="$(tmux capture-pane -t "$S115" -p -S -40)"
echo "==== Phase 115: the relaunch (no notice, no second ping) ===="
printf '%s\n' "$tm_second"
if printf '%s' "$tm_second" | grep -qF "$TM_NOTICE"; then
	tm_fail "the relaunch repeated the one-time notice"
fi
if [ "$(tm_pings)" != "1" ]; then
	tm_fail "the relaunch pinged again on the same day ($(tm_pings) pings)"
fi
tm_open_row
tm_menu="$(tmux capture-pane -t "$S115" -p)"
echo "==== Phase 115: /settings filtered to the Telemetry row ===="
printf '%s\n' "$tm_menu"
if ! printf '%s' "$tm_menu" | grep -qE "Telemetry +true"; then
	tm_fail "the Telemetry row did not read true"
fi
tmux send-keys -t "$S115" Space
sleep 0.5
tm_toggled="$(tmux capture-pane -t "$S115" -p)"
echo "==== Phase 115: after Space on the Telemetry row ===="
printf '%s\n' "$tm_toggled"
if ! printf '%s' "$tm_toggled" | grep -qF "Telemetry: false"; then
	tm_fail "no 'Telemetry: false' toast after Space"
fi
if [ "$(tm_field "$TM_CFG/home/telemetry.json" enabled)" != "false" ]; then
	tm_fail "telemetry.json did not record enabled: false"
fi
if grep -q telemetry "$TM_CFG/home/settings.json" 2>/dev/null; then
	tm_fail "the toggle leaked into settings.json (it is a per-user knob)"
fi
tmux send-keys -t "$S115" Space
sleep 0.5
if [ "$(tm_field "$TM_CFG/home/telemetry.json" enabled)" != "true" ]; then
	tm_fail "a second Space did not record enabled: true"
fi
if [ "$(tm_pings)" != "1" ]; then
	tm_fail "turning telemetry back on re-pinged although today's ping was already recorded"
fi
tmux send-keys -t "$S115" Escape
sleep 0.3
tm_quit

# (c) DO_NOT_TRACK=1 against a fresh config home: nothing is minted, shown or
# sent, and the row reads false for the run.
tmux new-session -d -s "$S115" -x 100 -y 30 "$TM_BASE DO_NOT_TRACK=1 ALTER_ZERO_CONFIG_DIR=$TM_CFG/dnt $BIN"
sleep 1.5
tm_dnt="$(tmux capture-pane -t "$S115" -p -S -40)"
echo "==== Phase 115: DO_NOT_TRACK=1 on a fresh config home ===="
printf '%s\n' "$tm_dnt"
if printf '%s' "$tm_dnt" | grep -qF "$TM_NOTICE"; then
	tm_fail "DO_NOT_TRACK=1 still showed the notice"
fi
if [ -e "$TM_CFG/dnt/telemetry.json" ]; then
	tm_fail "DO_NOT_TRACK=1 still wrote telemetry.json"
fi
tm_open_row
tm_dnt_row="$(tmux capture-pane -t "$S115" -p)"
echo "==== Phase 115: the row under DO_NOT_TRACK=1 ===="
printf '%s\n' "$tm_dnt_row"
# UNAVAILABLE, not merely false: an opt-out a keystroke could undo is not an
# opt-out. The row used to be seeded false but stay cyclable, so Space wrote
# `enabled: true` and sent a ping under DO_NOT_TRACK=1 — with the disclosure
# never shown, since only the launch path committed it (docs/telemetry.md).
if ! printf '%s' "$tm_dnt_row" | grep -qE "Telemetry +false \\(unavailable\\)"; then
	tm_fail "the Telemetry row is not unavailable under DO_NOT_TRACK=1"
fi
tmux send-keys -t "$S115" Space
sleep 1.2
tm_dnt_after="$(tmux capture-pane -t "$S115" -p)"
echo "==== Phase 115: after Space under DO_NOT_TRACK=1 ===="
printf '%s\n' "$tm_dnt_after"
if ! printf '%s' "$tm_dnt_after" | grep -qF "Can't change Telemetry"; then
	tm_fail "Space on the forbidden row did not refuse with a toast"
fi
if printf '%s' "$tm_dnt_after" | grep -qE "Telemetry +true"; then
	tm_fail "DO_NOT_TRACK=1 was cycled around from inside the app"
fi
if [ -e "$TM_CFG/dnt/telemetry.json" ]; then
	tm_fail "the refused cycle still wrote telemetry.json"
fi
if [ "$(tm_pings)" != "1" ]; then
	tm_fail "a ping left the machine under DO_NOT_TRACK=1 ($(tm_pings) in the log)"
fi
tmux send-keys -t "$S115" Escape
sleep 0.3
tm_quit

# (d) ALTER_ZERO_TELEMETRY=0 — the switch every other phase runs with — on a
# fresh config home: the same silence.
tmux new-session -d -s "$S115" -x 100 -y 30 "$TM_BASE ALTER_ZERO_TELEMETRY=0 ALTER_ZERO_CONFIG_DIR=$TM_CFG/off $BIN"
sleep 1.5
tm_off="$(tmux capture-pane -t "$S115" -p -S -40)"
if printf '%s' "$tm_off" | grep -qF "$TM_NOTICE"; then
	tm_fail "ALTER_ZERO_TELEMETRY=0 still showed the notice"
fi
if [ -e "$TM_CFG/off/telemetry.json" ]; then
	tm_fail "ALTER_ZERO_TELEMETRY=0 still wrote telemetry.json"
fi
tm_quit
if [ "$(tm_pings)" != "1" ]; then
	tm_fail "the off launches pinged ($(tm_pings) pings in the log)"
fi
# (e) The day-rollover check runs at every turn start, so a collector that
# never succeeds must not be retried once per turn. The attempt is remembered
# in memory for the session — the file's last_ping_day is written only on a
# 2xx — so a refusing collector costs exactly one attempt per UTC day, and the
# next LAUNCH is what retries it (docs/telemetry.md).
TM_PORT5="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
TM_LOG5="$TM_CFG/refused.log"
python3 - "$TM_PORT5" "$TM_LOG5" <<'PY' &
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer

port, log = int(sys.argv[1]), sys.argv[2]

class Stub(BaseHTTPRequestHandler):
    def do_POST(self):
        self.rfile.read(int(self.headers.get("content-length") or 0))
        with open(log, "a") as f:
            f.write("attempt\n")
        self.send_response(500)
        self.end_headers()

    def log_message(self, *_args):
        pass

HTTPServer(("127.0.0.1", port), Stub).serve_forever()
PY
TM_SERVER5=$!
for _ in $(seq 1 30); do
	if python3 -c "import socket, sys; s = socket.socket(); s.settimeout(0.2); sys.exit(0 if s.connect_ex(('127.0.0.1', $TM_PORT5)) == 0 else 1)" 2>/dev/null; then
		break
	fi
	sleep 0.1
done
tmux new-session -d -s "$S115" -x 100 -y 30 "$TM_BASE ALTER_ZERO_TELEMETRY_URL=http://127.0.0.1:$TM_PORT5/v1/ping ALTER_ZERO_CONFIG_DIR=$TM_CFG/refused $BIN"
sleep 1.5
tm_refused_boot="$(wc -l <"$TM_LOG5" 2>/dev/null | tr -d ' ')"
# One real turn, so the turn-start day check actually runs.
submit "$S115" "$USER_MSG"
wait_for 12 "$S115" -S -60 -- -F "$SETTLED_REPLY"
sleep 1.0
tm_refused_after="$(wc -l <"$TM_LOG5" 2>/dev/null | tr -d ' ')"
echo "==== Phase 115: refusing collector — attempts at boot: ${tm_refused_boot:-0}, after a turn: ${tm_refused_after:-0} ===="
if [ "${tm_refused_boot:-0}" != "1" ]; then
	tm_fail "expected exactly one attempt at boot against a refusing collector, got ${tm_refused_boot:-0}"
fi
if [ "${tm_refused_after:-0}" != "1" ]; then
	tm_fail "the turn-start day check retried a failing collector (${tm_refused_after:-0} attempts; it must be one per day per session)"
fi
# The KEY is always written (every field, always — docs/telemetry.md); what
# must not appear is a date in it, which is what "delivered" means.
if grep -qE '"last_ping_day": *"[0-9]' "$TM_CFG/refused/telemetry.json" 2>/dev/null; then
	tm_fail "a refused ping recorded last_ping_day — the day is only recorded on a 2xx"
fi
tm_quit
kill "$TM_SERVER5" 2>/dev/null
wait "$TM_SERVER5" 2>/dev/null

kill "$TM_SERVER" 2>/dev/null
wait "$TM_SERVER" 2>/dev/null
rm -rf "$TM_CFG"
