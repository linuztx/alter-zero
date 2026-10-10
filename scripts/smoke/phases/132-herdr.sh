#!/usr/bin/env bash
# Phase 132 — herdr pane state: idle, working, blocked, a failed turn's hold, the resume command, the release

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# herdr support (docs/herdr.md). Run inside a herdr pane — HERDR_ENV=1, a
# pane id, a socket path — the app tells herdr's socket what the agent is
# doing: one `pane.report_agent` request per change, `idle` at launch,
# `working` for a turn, `blocked` while a permission prompt waits on the user
# — and after a turn that failed, until the user moves past it — `idle`
# again when the turn ends, each carrying the command that resumes the
# session (`alter-zero` alone until the conversation has a file, and again
# after a `/clear`), and a `pane.release_agent` as the last request of a
# quit. A `!` command runs without the pane's id, so nothing the session
# starts can claim the pane. A Python stub stands in for herdr's socket: one
# request per connection, each line logged, answered `{"type":"ok"}` the way
# herdr answers. The fixture unsets every HERDR_* variable, so only these
# launches ever report.
S132="${S}_herdr"
HD_SOCK="$SMOKE_TMP/herdr.sock"
HD_LOG="$SMOKE_TMP/herdr-requests.jsonl"
python3 - "$HD_SOCK" "$HD_LOG" <<'PY' &
import json, socket, sys

path, log = sys.argv[1], sys.argv[2]
srv = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
srv.bind(path)
srv.listen(16)
while True:
    conn, _ = srv.accept()
    with conn:
        conn.settimeout(5)
        data = b""
        try:
            while not data.endswith(b"\n"):
                chunk = conn.recv(4096)
                if not chunk:
                    break
                data += chunk
        except OSError:
            continue
        if not data.endswith(b"\n"):
            continue  # herdr drops a request that never ends its line
        line = data.decode().rstrip("\n")
        with open(log, "a") as f:
            f.write(line + "\n")
        try:
            rid = json.loads(line).get("id", "")
        except ValueError:
            rid = ""
        conn.sendall((json.dumps({"id": rid, "result": {"type": "ok"}}) + "\n").encode())
PY
HD_SERVER=$!
smoke_on_exit kill "$HD_SERVER"
poll 5 test -S "$HD_SOCK" || fail "the herdr stub never bound its socket"
# `alter-zero` on PATH: the resume command's first word is the name herdr will
# type into the pane's shell, so the app only offers one it can find there.
mkdir -p "$SMOKE_TMP/bin"
ln -s "$BIN_ABS" "$SMOKE_TMP/bin/alter-zero"
HD_PANE="HERDR_ENV=1 HERDR_PANE_ID=w1:p3 HERDR_SOCKET_PATH=$HD_SOCK PATH=$SMOKE_TMP/bin:$PATH"
HD_APP="env $CFG_ENV_NOHIST $HD_PANE ALTER_ZERO_STARTUP_DELAY_MS=1500 $BIN"

hd_count() {
	if [ -f "$HD_LOG" ]; then wc -l <"$HD_LOG" | tr -d ' '; else echo 0; fi
}
# The log's requests from line FROM (1-based) on, one per line, as
# `{method} {state|-} {seq} {message|-} {resume argv joined by +|-}`.
hd_rows() {
	python3 - "$HD_LOG" "${1:-1}" <<'PY'
import json, sys
log, start = sys.argv[1], int(sys.argv[2])
try:
    lines = open(log).read().splitlines()
except FileNotFoundError:
    lines = []
for line in lines[start - 1:]:
    r = json.loads(line)
    p = r["params"]
    print(
        r["method"],
        p.get("state", "-"),
        p.get("seq", "-"),
        (p.get("message") or "-").replace(" ", "_"),
        "+".join(p.get("resume_argv") or []) or "-",
    )
PY
}
# The states reported from line FROM on, consecutive repeats collapsed (a
# 30 s keepalive resends the current state unchanged), the release as `release`.
hd_states() {
	hd_rows "${1:-1}" | awk '{ s = ($1 == "pane.release_agent") ? "release" : $2; if (s != last) { printf "%s%s", sep, s; sep = " " } last = s }'
}
hd_last_state() { hd_states "${1:-1}" | awk '{ print $NF }'; }
# Predicates `poll` re-runs, so each re-reads the log.
hd_has_state() { hd_states "${1:-1}" | grep -qw "$2"; }
hd_count_ge() { [ "$(hd_count)" -ge "$1" ]; }
hd_count_gt() { [ "$(hd_count)" -gt "$1" ]; }
hd_last_is() { [ "$(hd_last_state)" = "$1" ]; }

# (a) The launch claims the pane: one `idle` report, the pane id verbatim,
# `alter-zero` as source and agent, a microsecond seq that is also the
# request's id — and `alter-zero` alone as the resume command, since nothing
# is recorded yet: a herdr restart brings back the fresh session it was.
launch "$S132" 100 44 "$HD_APP"
poll 10 hd_count_ge 1 || fail "no report reached the herdr socket after launch"
note "the launch's report"
cat "$HD_LOG" 2>/dev/null
if ! python3 - "$HD_LOG" <<'PY'; then
import json, sys
r = json.loads(open(sys.argv[1]).readline())
p = r["params"]
assert r["method"] == "pane.report_agent", r
assert p["pane_id"] == "w1:p3", p
assert p["source"] == "alter-zero" and p["agent"] == "alter-zero", p
assert p["state"] == "idle", p
assert isinstance(p["seq"], int) and p["seq"] > 10**15, p  # microseconds
assert r["id"] == "alter-zero:%d" % p["seq"], r
assert p["resume_argv"] == ["alter-zero"], p
for key in ("message", "agent_session_id"):
    assert key not in p, (key, p)
PY
	fail "the launch's report is not idle for pane w1:p3 from alter-zero with a microsecond seq and a bare resume command"
fi

# (b) A turn whose write waits on a permission prompt: `working`, then
# `blocked` naming what the prompt asks, then — answered — `working`, and
# `idle` when it ends. From the first message on, every report carries the
# command that resumes the recorded session.
submit "$S132" "permission demo please"
poll 15 hd_has_state 1 blocked || fail "the permission prompt never reported blocked"
hd_prompt="$(wait_pane 10 "$S132" -F "Do you want to create hello.py?")"
note "the permission prompt"
printf '%s\n' "$hd_prompt"
tmux send-keys -t "$S132" 1
poll 15 hd_last_is idle || fail "the turn's end never reported idle"
hd_summary="$(wait_summaries 10 "$S132" 1)"
note "the settled turn + every request so far"
printf '%s\n' "$hd_summary"
hd_rows
expect_eq "$(hd_states)" "idle working blocked working idle" "the states one turn with a permission prompt reports"
hd_blocked="$(hd_rows | awk '$2 == "blocked" { print $4; exit }')"
expect_eq "$hd_blocked" "Create_file:_hello.py" "the blocked report's message (spaces shown as _)"
HD_ID="$(find "$SMOKE_SESSIONS" -name 'rollout-*.jsonl' 2>/dev/null | head -1 | sed -E 's/.*rollout-[0-9-]{10}T[0-9-]{8}-(.*)\.jsonl/\1/')"
[ -n "$HD_ID" ] || fail "the turn recorded no rollout file"
hd_named="$(hd_rows 2 | awk -v id="$HD_ID" '$5 == "alter-zero+--resume+" id' | wc -l | tr -d ' ')"
expect_eq "$hd_named" "$(($(hd_count) - 1))" "every report after the first message resumes session $HD_ID with alter-zero --resume $HD_ID"
if ! hd_rows | awk '{ if (NR > 1 && $3 <= last) exit 1; last = $3 }'; then
	fail "the seqs do not strictly increase — herdr drops a report that does not outrank the last"
fi

# A `!` command runs without the pane's id — a nested agent's herdr hook
# would claim the pane, and herdr drops every report alter-zero sends after
# that — while herdr's marker and socket stay, so its CLI still works.
submit "$S132" '!printf "pane=%s env=%s sock=%s\n" "${HERDR_PANE_ID-unset}" "${HERDR_ENV-unset}" "${HERDR_SOCKET_PATH:+set}"'
hd_env="$(wait_pane 10 "$S132" -F "pane=unset env=1 sock=set")" ||
	fail "a ! command inherited the herdr pane id (or lost herdr's marker or socket)"
note "a ! command's view of the herdr variables"
printf '%s\n' "$hd_env" | grep -F "pane=" | tail -1
poll 10 hd_last_is idle || fail "the ! command's end never reported idle"

# (c) The quit hands the pane back: a release, last, outranking every report.
HD_BEFORE_QUIT="$(hd_count)"
submit "$S132" "/quit"
wait_gone 5 "$S132" || fail "the app did not exit on /quit"
poll 5 hd_count_gt "$HD_BEFORE_QUIT" || fail "the quit sent nothing to herdr"
note "the quit's requests"
hd_rows "$((HD_BEFORE_QUIT + 1))"
expect_eq "$(hd_states "$((HD_BEFORE_QUIT + 1))")" "release" "the quit's one request is the release"
if ! hd_rows | awk '{ if (NR > 1 && $3 <= last) exit 1; last = $3 }'; then
	fail "the release does not outrank the reports before it"
fi
HD_RELEASE_SEQ="$(hd_rows | awk 'END { print $3 }')"

# (d) A relaunch resuming that session claims the pane with the resume
# command on its very first report — and outranks the process before it, or
# herdr would drop every report of the new one.
HD_BEFORE_RESUME="$(hd_count)"
launch "$S132" 100 44 "env $CFG_ENV_NOHIST $HD_PANE ALTER_ZERO_STARTUP_DELAY_MS=200 $BIN --resume $HD_ID"
poll 10 hd_count_gt "$HD_BEFORE_RESUME" || fail "the resumed session reported nothing"
hd_first="$(hd_rows "$((HD_BEFORE_RESUME + 1))" | head -1)"
note "the resumed launch's first report"
printf '%s\n' "$hd_first"
expect_eq "$(printf '%s' "$hd_first" | awk '{ print $2, $5 }')" "idle alter-zero+--resume+$HD_ID" "the resumed launch's first report: idle, with its resume command"
if [ "$(printf '%s' "$hd_first" | awk '{ print $3 }')" -le "$HD_RELEASE_SEQ" ]; then
	fail "the relaunch's seq does not outrank the previous process's release"
fi
submit "$S132" "/quit"
wait_gone 5 "$S132" || fail "the resumed app did not exit on /quit"

# (e) ALTER_ZERO_HERDR=0 turns it off for a run: a whole turn and a quit
# inside the pane, and nothing reaches the socket.
HD_BEFORE_OFF="$(hd_count)"
launch "$S132" 100 30 "env $CFG_ENV_NOHIST $HD_PANE ALTER_ZERO_HERDR=0 ALTER_ZERO_STARTUP_DELAY_MS=200 $BIN"
submit "$S132" "hello there"
wait_summaries 15 "$S132" 1 >/dev/null || fail "the turn with herdr support off never settled"
submit "$S132" "/quit"
wait_gone 5 "$S132" || fail "the app with herdr support off did not exit"
expect_eq "$(hd_count)" "$HD_BEFORE_OFF" "requests sent with ALTER_ZERO_HERDR=0"

# (f) A herdr that is not there costs nothing: against a socket nobody
# listens on, the turn runs and the quit is prompt.
launch "$S132" 100 30 "env $CFG_ENV_NOHIST HERDR_ENV=1 HERDR_PANE_ID=w1:p3 HERDR_SOCKET_PATH=$SMOKE_TMP/absent.sock ALTER_ZERO_STARTUP_DELAY_MS=200 $BIN"
submit "$S132" "hello there"
hd_dead="$(wait_summaries 15 "$S132" 1)" || fail "the turn against a dead herdr socket never settled"
note "a turn against a dead herdr socket"
printf '%s\n' "$hd_dead"
submit "$S132" "/quit"
wait_gone 3 "$S132" || fail "the quit hung on a dead herdr socket"

# (g) A turn that fails on a backend error holds the pane blocked, naming the
# failure — idle would have herdr announce the work finished — until the user
# moves past it: a `/clear` replaces the conversation, and its report names a
# fresh launch again rather than the session just cleared. The real backend,
# against a stub provider that refuses every request with a 400 (never
# retried).
HD_PROVIDER_PORT="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
python3 - "$HD_PROVIDER_PORT" <<'PY' &
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer

class Stub(BaseHTTPRequestHandler):
    def do_GET(self):
        self.send(200, b'{"object":"list","data":[{"id":"stub-model"}]}')

    def do_POST(self):
        self.rfile.read(int(self.headers.get("content-length") or 0))
        self.send(400, b'{"error":{"message":"smoke132 refused"}}')

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
HD_PROVIDER=$!
smoke_on_exit kill "$HD_PROVIDER"
HD_PROVIDERS="$SMOKE_TMP/providers132.toml"
printf '[providers.stub]\nname = "Stub"\n\n[providers.stub.kwargs]\napi_base = "http://127.0.0.1:%s/v1"\n' "$HD_PROVIDER_PORT" >"$HD_PROVIDERS"
HD_BEFORE_FAIL="$(hd_count)"
launch "$S132" 100 30 "env NO_PROXY=127.0.0.1 no_proxy=127.0.0.1 $CFG_ENV_NOHIST $HD_PANE ALTER_ZERO_PROVIDERS_FILE=$HD_PROVIDERS ALTER_ZERO_PROVIDER=stub ALTER_ZERO_MODEL=stub-model STUB_API_KEY=stand-in $BIN_ABS"
poll 10 hd_count_gt "$HD_BEFORE_FAIL" || fail "the real-backend launch reported nothing"
submit "$S132" "smoke132 fail"
hd_failed="$(wait_pane 15 "$S132" -F "smoke132 refused")" || fail "the refused turn never showed its error"
note "the failed turn"
printf '%s\n' "$hd_failed"
poll 10 hd_last_is blocked || fail "the failed turn did not report blocked"
hd_rows "$((HD_BEFORE_FAIL + 1))"
expect_eq "$(hd_states "$((HD_BEFORE_FAIL + 1))")" "idle working blocked" "the states a launch and one failed turn report"
hd_failure="$(hd_rows "$((HD_BEFORE_FAIL + 1))" | awk '$2 == "blocked" { print $4; exit }')"
case "$hd_failure" in
Turn_failed:*smoke132*) ;;
*) fail "the failed turn's blocked message does not name the failure (got '$hd_failure')" ;;
esac
# The hold outlives the turn: the loop keeps running, and nothing reports idle
# behind the user's back.
hd_held="$(hd_count)"
wait_settled 5 "$S132" -F "smoke132 refused" >/dev/null || true
expect_eq "$(hd_last_state "$((HD_BEFORE_FAIL + 1))")" "blocked" "the state once the failed turn settled"
expect_eq "$(hd_count)" "$hd_held" "reports sent while the failure stood"
submit "$S132" "/clear"
poll 10 hd_last_is idle || fail "/clear never released the failed turn's hold"
hd_cleared="$(hd_rows | tail -1)"
note "the report after /clear"
printf '%s\n' "$hd_cleared"
expect_eq "$(printf '%s' "$hd_cleared" | awk '{ print $2, $5 }')" "idle alter-zero" "the report after /clear: idle, resuming as a fresh launch"
submit "$S132" "/quit"
wait_gone 5 "$S132" || fail "the real-backend app did not exit on /quit"
poll 5 hd_last_is release || fail "the real-backend quit did not release the pane"
