#!/usr/bin/env bash
# Phase 155 — /model's list cache: kept across a restart, Ctrl+R, an aged list refreshed behind its rows, a failed refresh, /login dropping it

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# The `/model` picker keeps each provider's listing on disk
# (`{config_home}/model-cache/{provider}.json`, docs/model-cache.md) and
# opens on it at once instead of on `Loading models…`. A stub provider whose
# list, answer delay and status are files this phase rewrites stands in for
# a real one, and its log counts every listing request. Driven:
#   (a) the first open fetches the list once, and keeps it;
#   (b) after a restart the kept list is the page — no request at all, and
#       a list a moment old carries no note;
#   (c) Ctrl+R asks again whatever the cache says, and keeps the answer;
#   (d) a list two hours old shows at once under `updated 2h ago ·
#       refreshing…` while the provider is asked again behind it, and the
#       answer replaces its rows;
#   (e) a refresh that fails keeps the rows it could not refresh, noted
#       `Stub not refreshed` with the way to try again — never `unavailable`;
#   (f) a key saved for the provider through /login drops its kept list.
S155="${S}_modelcache"
STATE="$SMOKE_TMP/stub155"
mkdir -p "$STATE"
printf 'stub/alpha stub/beta\n' >"$STATE/models"
PORT="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
python3 - "$PORT" "$STATE" <<'PY' &
import json, os, sys, time
from http.server import BaseHTTPRequestHandler, HTTPServer

port, state = int(sys.argv[1]), sys.argv[2]

def read(name, default):
    try:
        with open(os.path.join(state, name)) as f:
            return f.read().strip()
    except FileNotFoundError:
        return default

class Stub(BaseHTTPRequestHandler):
    def do_GET(self):
        if not self.path.startswith("/v1/models"):
            self.send_response(404)
            self.end_headers()
            return
        # Logged as the request arrives, so a delayed answer is counted
        # while it is still being held back.
        with open(os.path.join(state, "log"), "a") as log:
            log.write(f"GET {self.path}\n")
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
        self.wfile.write(data)

    def log_message(self, *_args):
        pass

HTTPServer(("127.0.0.1", port), Stub).serve_forever()
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
# age_cache — date the kept list two hours back, as though it had been
# fetched then.
age_cache() {
	python3 -I - "$CACHE" <<'PY' || fail "could not age the kept list"
import json, sys, time
path = sys.argv[1]
with open(path) as f:
    data = json.load(f)
data["fetched_at"] = int(time.time()) - 2 * 3600
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

# ---- (c) Ctrl+R asks again whatever the cache says, and keeps the answer ----
printf 'stub/alpha stub/beta stub/gamma\n' >"$STATE/models"
keys "$S155" C-r
refreshed="$(wait_pane 5 "$S155" -F "stub/gamma")" || fail "Ctrl+R did not bring the list's new model"
dump "/model after Ctrl+R" "$refreshed"
expect_eq "$(requests)" 2 "Ctrl+R did not ask for the list exactly once more"
expect_lacks "$refreshed" -F "❯ r" "Ctrl+R typed into the search"
wait_file 5 "$CACHE" -F "stub/gamma" || fail "the refreshed list was not kept"
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

# ---- (f) a key saved through /login drops the provider's kept list ----
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
[ -e "$CACHE" ] && fail "saving the stub's key left its kept list behind ($CACHE)"

tmux kill-session -t "$S155" 2>/dev/null
echo "==== Phase 155: /model opens on its kept lists, refreshes them with Ctrl+R or once they age, keeps them through a failed refresh, and forgets one /login re-keys ===="
