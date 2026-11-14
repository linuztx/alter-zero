#!/usr/bin/env bash
# Phase 155 — /model's list cache: kept across a restart, Ctrl+R, an aged list refreshed behind its rows, a failed refresh, fetches outliving the picker, /login retiring one, an open picker re-dating its list

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# The `/model` picker keeps each provider's listing on disk
# (`{config_home}/model-cache/{provider}.json`, docs/model-cache.md) and
# opens on it at once instead of on `Loading models…`. A stub provider whose
# list, answer delays and status are files this phase rewrites stands in for
# a real one, and its log records every listing request as it arrives and
# again once its answer has gone. Driven:
#   (a) the first open fetches the list once, and keeps it;
#   (b) after a restart the kept list is the page — no request at all, and
#       a list a moment old carries no note — and the kept file names
#       neither the server it was read from nor the key it was read with;
#   (c) Ctrl+R asks again whatever the cache says — once, however often it
#       is pressed while that fetch is out — and keeps the answer;
#   (d) a list two hours old shows at once under `updated 2h ago ·
#       refreshing…` while the provider is asked again behind it, and the
#       answer replaces its rows;
#   (e) a refresh that fails keeps the rows it could not refresh, noted
#       `Stub not refreshed` with the way to try again — never `unavailable`;
#   (f) a fetch still out when the picker closes is kept when it lands: what
#       it read is no less true for the picker having closed;
#   (g) reopening while that fetch is out waits on it rather than asking
#       again, its answer landing in the reopened page;
#   (h) a key saved for the provider through /login drops its kept list,
#       and a fetch still out with the key the save replaced is kept
#       nowhere when it lands — it must not pass for fresh after the save;
#   (i) an open picker re-dates its list as it ages with no key pressed:
#       `updated 1m ago · ctrl+r to refresh` arrives on the idle redraw.
S155="${S}_modelcache"
STATE="$SMOKE_TMP/stub155"
mkdir -p "$STATE"
printf 'stub/alpha stub/beta\n' >"$STATE/models"
PORT="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
python3 - "$PORT" "$STATE" <<'PY' &
import json, os, sys, time
from http.server import BaseHTTPRequestHandler, HTTPServer
from socketserver import ThreadingMixIn

port, state = int(sys.argv[1]), sys.argv[2]

def read(name, default):
    try:
        with open(os.path.join(state, name)) as f:
            return f.read().strip()
    except FileNotFoundError:
        return default

def log(what, path):
    with open(os.path.join(state, "log"), "a") as f:
        f.write(f"{what} {path}\n")

class Stub(BaseHTTPRequestHandler):
    def do_GET(self):
        if not self.path.startswith("/v1/models"):
            self.send_response(404)
            self.end_headers()
            return
        # Logged as the request arrives, so a delayed answer is counted
        # while it is still being held back.
        log("GET", self.path)
        try:
            time.sleep(float(read("delay", "0")))
            status = int(read("status", "200"))
            if status == 200:
                ids = read("models", "").split()
                body = {"data": [{"id": i, "name": f"Stub: {i}"} for i in ids]}
            else:
                body = {"error": {"message": "the stub is down"}}
            data = json.dumps(body).encode()
            self.send_response(status)
            self.send_header("content-type", "application/json")
            self.send_header("content-length", str(len(data)))
            self.end_headers()
            # `body_delay` holds the body back after the headers have gone: a
            # big catalog on a slow link, still arriving when the picker that
            # asked for it closes, or when /login replaces the key it used.
            self.wfile.flush()
            time.sleep(float(read("body_delay", "0")))
            self.wfile.write(data)
            self.wfile.flush()
        except (BrokenPipeError, ConnectionResetError):
            pass # the client stopped reading
        finally:
            # Logged once the answer has gone, so the phase can tell an
            # answer that has landed from one still held back.
            log("ANSWERED", self.path)

    def log_message(self, *_args):
        pass

# Threaded, so an answer held back never holds back a request behind it:
# the log counts requests as they are made, not as the stub gets to them.
class Server(ThreadingMixIn, HTTPServer):
    daemon_threads = True

Server(("127.0.0.1", port), Stub).serve_forever()
PY
SERVER=$!
smoke_on_exit kill "$SERVER"
for _ in $(seq 1 30); do # wait for the stub to listen
	if python3 -c "import socket, sys; s = socket.socket(); s.settimeout(0.2); sys.exit(0 if s.connect_ex(('127.0.0.1', $PORT)) == 0 else 1)" 2>/dev/null; then
		break
	fi
	sleep 0.1
done
PROVIDERS="$SMOKE_TMP/providers155.toml"
cat >"$PROVIDERS" <<TOML
[providers.stub]
name = "Stub"
api_key_env = "STUB_API_KEY"

[providers.stub.kwargs]
api_base = "http://127.0.0.1:$PORT/v1"
TOML
APP155="env NO_PROXY=127.0.0.1 no_proxy=127.0.0.1 ALTER_ZERO_PROVIDERS_FILE=$PROVIDERS STUB_API_KEY=smoke-155 $APP"
CACHE="$SMOKE_CFG/model-cache/stub.json"

# requests — how many listings the stub has been asked for.
requests() {
	local n
	n="$(grep -c '^GET /v1/models' "$STATE/log" 2>/dev/null)"
	echo "${n:-0}"
}
# answered — how many of those the stub has finished answering.
answered() {
	local n
	n="$(grep -c '^ANSWERED /v1/models' "$STATE/log" 2>/dev/null)"
	echo "${n:-0}"
}
# asked_since N — has the stub been asked for its list more than N times?
asked_since() { [ "$(requests)" -gt "$1" ]; }
# answered_since N — has it finished more than N answers?
answered_since() { [ "$(answered)" -gt "$1" ]; }
# age_cache [SECONDS] — date the kept list SECONDS back (two hours by
# default), as though it had been fetched then.
age_cache() {
	python3 -I - "$CACHE" "${1:-7200}" <<'PY' || fail "could not age the kept list"
import json, sys, time
path, seconds = sys.argv[1], int(sys.argv[2])
with open(path) as f:
    data = json.load(f)
data["fetched_at"] = int(time.time()) - seconds
with open(path, "w") as f:
    json.dump(data, f)
PY
}
# close_model — Esc the picker shut, back to the composer.
close_model() {
	keys "$S155" Escape
	wait_for 3 "$S155" -E '^❯' || fail "Esc did not close /model"
}

# ---- (a) the first open fetches the list once, and keeps it ----
launch "$S155" 100 30 "$APP155"
submit "$S155" "/model"
first="$(wait_pane 10 "$S155" -F "stub/beta")" || fail "/model did not list the stub's models"
dump "the first /model open" "$first"
expect_eq "$(requests)" 1 "the first open did not ask for the list exactly once"
wait_file 5 "$CACHE" -F "stub/beta" || fail "the fetched list was not kept in $CACHE"
close_model

# ---- (b) after a restart, the kept list is the page — and nothing is asked ----
tmux kill-session -t "$S155" 2>/dev/null
launch "$S155" 100 30 "$APP155"
submit "$S155" "/model"
wait_for 5 "$S155" -F "stub/beta" || fail "the kept list was not shown after a restart"
sleep 1 # the window a fetch would land in
kept="$(pane "$S155")"
dump "/model after a restart, over the kept list" "$kept"
expect_eq "$(requests)" 1 "a list kept a moment ago was asked for again"
expect_has "$kept" -E '^    \(1/2\) *$' "a list kept a moment ago carries a note beside its counter"
expect_lacks "$kept" -F "Loading models…" "the kept list waited on a fetch"
# What is kept says which server it came from only as a fingerprint.
kept_file="$(cat "$CACHE")"
expect_lacks "$kept_file" -F "127.0.0.1" "the kept list names the server it was read from"
expect_lacks "$kept_file" -F "http" "the kept list names the server it was read from"
expect_lacks "$kept_file" -F "smoke-155" "the kept list holds the key it was read with"

# ---- (c) Ctrl+R asks again whatever the cache says, once, and keeps the answer ----
printf 'stub/alpha stub/beta stub/gamma\n' >"$STATE/models"
printf '1.5' >"$STATE/delay"
keys "$S155" C-r
asking="$(wait_pane 3 "$S155" -F "refreshing…")" || fail "Ctrl+R did not say it was refreshing"
dump "/model while Ctrl+R's fetch is out" "$asking"
keys "$S155" C-r # again, while that fetch is out
refreshed="$(wait_pane 5 "$S155" -F "stub/gamma")" || fail "Ctrl+R did not bring the list's new model"
sleep 0.5 # the window a second request would arrive in
dump "/model after Ctrl+R" "$refreshed"
expect_eq "$(requests)" 2 "Ctrl+R pressed twice did not ask for the list exactly once more"
expect_lacks "$refreshed" -F "❯ r" "Ctrl+R typed into the search"
wait_file 5 "$CACHE" -F "stub/gamma" || fail "the refreshed list was not kept"
rm -f "$STATE/delay"
close_model

# ---- (d) an aged list shows at once, refreshed behind its rows ----
age_cache
printf '2' >"$STATE/delay"
printf 'stub/alpha stub/beta stub/gamma stub/delta\n' >"$STATE/models"
submit "$S155" "/model"
aged="$(wait_pane 3 "$S155" -F "refreshing…")" || fail "an aged list was not refreshed behind its rows"
dump "/model over a list two hours old" "$aged"
expect_has "$aged" -F "stub/gamma" "the aged list's rows were not shown at once"
expect_has "$aged" -F "updated 2h ago" "the aged list did not say how old it was"
expect_lacks "$aged" -F "stub/delta" "the refresh landed before the stub answered"
expect_lacks "$aged" -F "Loading models…" "an aged list waited on its refresh"
landed="$(wait_pane 8 "$S155" -F "stub/delta")" || fail "the refresh never replaced the aged rows"
dump "/model once the refresh landed" "$landed"
expect_lacks "$landed" -F "refreshing…" "the page still says it is refreshing"
expect_lacks "$landed" -F "updated 2h ago" "the page still dates itself by the aged list"
expect_eq "$(requests)" 3 "the aged list was not asked for exactly once"
rm -f "$STATE/delay"
close_model

# ---- (e) a failed refresh keeps the rows it could not refresh ----
age_cache
printf '503' >"$STATE/status"
submit "$S155" "/model"
failed="$(wait_pane 8 "$S155" -F "not refreshed")" || fail "a failed refresh was not noted"
dump "/model after a failed refresh" "$failed"
expect_has "$failed" -F "stub/delta" "a failed refresh took the kept rows away"
expect_has "$failed" -E '\(1/4\) +· +updated 2h ago +· +Stub not refreshed +· +ctrl\+r to refresh' \
	"the counter does not say the kept list could not be refreshed, and how to try again"
expect_lacks "$failed" -F "unavailable" "a provider whose kept rows stay was called unavailable"
rm -f "$STATE/status"
close_model

# ---- (f) a fetch still out when the picker closes is kept when it lands ----
# The answer's headers go at once and its body is held back, so the fetch is
# past every check it makes before reading when the picker closes on it.
printf 'stub/alpha stub/beta stub/epsilon\n' >"$STATE/models"
printf '2' >"$STATE/body_delay"
before="$(requests)"
done_before="$(answered)"
submit "$S155" "/model"
wait_for 3 "$S155" -F "refreshing…" || fail "the list (still aged) was not refreshed"
poll 3 asked_since "$before" || fail "/model did not ask the stub for its list"
close_model
answered_since "$done_before" && fail "the answer landed before the picker closed: the step proves nothing"
wait_file 6 "$CACHE" -F "stub/epsilon" || fail "a fetch that landed after the picker closed was not kept"
expect_eq "$(requests)" "$((before + 1))" "the closed picker's list was not asked for exactly once"

# ---- (g) reopening while a fetch is out waits on it rather than asking again ----
printf 'stub/alpha stub/beta stub/zeta\n' >"$STATE/models"
printf '3' >"$STATE/body_delay"
submit "$S155" "/model"
wait_for 3 "$S155" -F "stub/epsilon" || fail "the list kept a moment ago was not shown"
before="$(requests)"
keys "$S155" C-r
poll 3 asked_since "$before" || fail "Ctrl+R did not ask the stub for its list"
close_model
submit "$S155" "/model"
waiting="$(wait_pane 3 "$S155" -F "refreshing…")" || fail "the reopened picker did not wait on the fetch still out"
dump "/model reopened while its fetch was still out" "$waiting"
expect_has "$waiting" -F "stub/epsilon" "the reopened picker did not show the kept rows while it waited"
wait_for 6 "$S155" -F "stub/zeta" || fail "the fetch the reopened picker waited on never landed in it"
sleep 0.5 # the window a second request would arrive in
expect_eq "$(requests)" "$((before + 1))" "reopening while a fetch was out asked for the list again"
wait_file 5 "$CACHE" -F "stub/zeta" || fail "the list the reopened picker waited on was not kept"
close_model

# ---- (h) a key saved through /login drops the kept list, and a fetch out with the old key ----
printf 'stub/alpha stub/beta stub/eta\n' >"$STATE/models"
printf '6' >"$STATE/body_delay"
submit "$S155" "/model"
wait_for 3 "$S155" -F "stub/zeta" || fail "the list kept a moment ago was not shown"
before="$(requests)"
done_before="$(answered)"
keys "$S155" C-r
poll 3 asked_since "$before" || fail "Ctrl+R did not ask the stub for its list"
close_model
submit "$S155" "/login"
wait_for 5 "$S155" -F "Sign in with an API key" || fail "/login did not open"
keys "$S155" Down
sleep 0.2
keys "$S155" Enter
wait_for 5 "$S155" -F "Keys are saved to" || fail "/login's API-key list did not open"
keys "$S155" Enter
wait_for 5 "$S155" -F "paste your API key" || fail "the key step did not open for the stub"
type_text "$S155" "smoke-155-rotated"
sleep 0.2
keys "$S155" Enter
saved="$(wait_pane 5 "$S155" -F "Saved STUB_API_KEY")" || fail "the key was not saved"
dump "a key saved for the stub" "$saved"
answered_since "$done_before" && fail "the old key's answer landed before the save: the step proves nothing"
[ -e "$CACHE" ] && fail "saving the stub's key left its kept list behind ($CACHE)"
poll 10 answered_since "$done_before" || fail "the stub never answered the fetch made with the old key"
sleep 1 # what the app would do with that answer, it has done
[ -e "$CACHE" ] && fail "a list read with the key /login replaced was kept when it landed ($CACHE)"
rm -f "$STATE/body_delay"

# ---- (i) an open picker re-dates its list as it ages, with no key pressed ----
submit "$S155" "/model"
wait_for 10 "$S155" -F "stub/eta" || fail "/model did not list the stub's models after the key was saved"
wait_file 5 "$CACHE" -F "stub/eta" || fail "the list read with the new key was not kept"
close_model
age_cache 50
submit "$S155" "/model"
young="$(wait_pane 3 "$S155" -F "stub/eta")" || fail "the list kept 50 seconds ago was not shown"
dump "/model over a list 50 seconds old" "$young"
expect_lacks "$young" -F "updated" "a list under a minute old was dated"
expect_lacks "$young" -F "ctrl+r to refresh" "a list under a minute old offered a refresh"
before="$(requests)"
# No key is pressed from here on: only the open picker's idle redraw can
# show the list passing a minute.
dated="$(wait_pane 25 "$S155" -F "updated 1m ago")" || fail "the open picker never re-dated its list as it aged"
dump "/model left open as its list passed a minute" "$dated"
expect_has "$dated" -E '\(1/3\) +· +updated 1m ago +· +ctrl\+r to refresh' \
	"a list past a minute old did not offer a refresh beside its age"
expect_eq "$(requests)" "$before" "the idle picker asked for the list"
close_model

tmux kill-session -t "$S155" 2>/dev/null
echo "==== Phase 155: /model opens on its kept lists, refreshes them with Ctrl+R (once however often it is pressed) or once they age, keeps them through a failed refresh, keeps a fetch that outlives the picker and waits on it when reopened, keeps nothing from a fetch /login re-keyed, and re-dates an open list as it ages ===="
