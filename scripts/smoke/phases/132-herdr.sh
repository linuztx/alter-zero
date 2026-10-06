#!/usr/bin/env bash
# Phase 132 — herdr pane state: idle, working, blocked, the resume command, the release

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# herdr support (docs/herdr.md). Run inside a herdr pane — HERDR_ENV=1, a
# pane id, a socket path — the app tells herdr's socket what the agent is
# doing: one `pane.report_agent` request per change, `idle` at launch,
# `working` for a turn, `blocked` while a permission prompt waits on the user,
# `idle` again when the turn ends, each carrying the recorded session's resume
# command once the conversation has a file, and a `pane.release_agent` as the
# last request of a quit. A Python stub stands in for herdr's socket: one
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
# `{method} {state|-} {seq} {message|-} {session|-} {resume argv joined by +|-}`.
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
        p.get("agent_session_id", "-"),
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
# request's id — and no resume command, since nothing is recorded yet.
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
for key in ("message", "resume_argv", "agent_session_id"):
    assert key not in p, (key, p)
PY
	fail "the launch's report is not idle for pane w1:p3 from alter-zero with a microsecond seq"
fi

# (b) A turn whose write waits on a permission prompt: `working`, then
# `blocked` naming what the prompt asks, then — answered — `working`, and
# `idle` when it ends. From the first message on, every report names the
# recorded session and the command that resumes it.
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
hd_named="$(hd_rows 2 | awk -v id="$HD_ID" '$5 == id && $6 == "alter-zero+--resume+" id' | wc -l | tr -d ' ')"
expect_eq "$hd_named" "$(($(hd_count) - 1))" "every report after the first message names session $HD_ID and resumes it with alter-zero --resume $HD_ID"
if ! hd_rows | awk '{ if (NR > 1 && $3 <= last) exit 1; last = $3 }'; then
	fail "the seqs do not strictly increase — herdr drops a report that does not outrank the last"
fi

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
expect_eq "$(printf '%s' "$hd_first" | awk '{ print $2, $5, $6 }')" "idle $HD_ID alter-zero+--resume+$HD_ID" "the resumed launch's first report: idle, its session, its resume command"
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
