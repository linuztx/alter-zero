#!/usr/bin/env bash
# Phase 155 — /model opens on the lists it kept, and Ctrl+R fetches them again

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# The /model picker keeps every provider's list in model-cache.json and opens
# on it at once, fetching again only a list past its hour — or on Ctrl+R
# (docs/model-cache.md). A stub provider lists three models and logs every
# listing request; a flag file makes it list a fourth, one makes it answer
# slowly, and one makes it refuse with a 503. Against the real binary:
#   (a) the first /model fetches the list and keeps it, keyed by the
#       provider, the rows without the provider's URL, and the counter says
#       how fresh it is and which key refreshes it;
#   (b) a second /model, inside the hour, lists the models without asking
#       the stub at all;
#   (c) a restart reads the list back from the file — still no request;
#   (d) Ctrl+R asks again, once however often it is pressed while the
#       answer is out, says `refreshing…` over the kept rows meanwhile, and
#       the stub's new model appears when it lands;
#   (e) a refresh the stub refuses keeps the rows, under `couldn't refresh
#       Stub` and the refusal's reason;
#   (f) /login saving the provider's key drops its kept list, and the next
#       /model asks again.
S155="${S}_modelcache"
C_LOG="$SMOKE_TMP/stub.log"
C_FLAGS="$SMOKE_TMP/flags"
mkdir -p "$C_FLAGS"
: >"$C_LOG"
C_PORT="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
python3 - "$C_PORT" "$C_LOG" "$C_FLAGS" <<'PY' &
import json, os, sys, time
from http.server import BaseHTTPRequestHandler, HTTPServer

port, log, flags = int(sys.argv[1]), sys.argv[2], sys.argv[3]
base = [{"id": f"stub/model-{i:02}", "name": f"Stub: Model {i}"} for i in range(3)]

def flag(name):
    return os.path.exists(os.path.join(flags, name))

class Stub(BaseHTTPRequestHandler):
    def reply(self, status, body, kind):
        self.send_response(status)
        self.send_header("content-type", kind)
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        with open(log, "a") as f:
            f.write(f"GET {self.path}\n")
        if not self.path.startswith("/v1/models"):
            self.reply(404, b"", "text/plain")
            return
        if flag("slow"):
            time.sleep(3)
        if flag("refuse"):
            self.reply(503, b"upstream busy", "text/plain")
            return
        extra = [{"id": "stub/model-new", "name": "Stub: New Model"}] if flag("extra") else []
        self.reply(200, json.dumps({"data": base + extra}).encode(), "application/json")

    def log_message(self, *_args):
        pass

HTTPServer(("127.0.0.1", port), Stub).serve_forever()
PY
C_SERVER=$!
smoke_on_exit kill "$C_SERVER"
for _ in $(seq 1 30); do # wait for the stub to listen
	if python3 -c "import socket, sys; s = socket.socket(); s.settimeout(0.2); sys.exit(0 if s.connect_ex(('127.0.0.1', $C_PORT)) == 0 else 1)" 2>/dev/null; then
		break
	fi
	sleep 0.1
done
C_PROVIDERS="$SMOKE_TMP/providers.toml"
cat >"$C_PROVIDERS" <<TOML
[providers.stub]
name = "Stub"
api_key_env = "STUB_API_KEY"

[providers.stub.kwargs]
api_base = "http://127.0.0.1:$C_PORT/v1"
TOML
C_APP="env NO_PROXY=127.0.0.1 no_proxy=127.0.0.1 ALTER_ZERO_PROVIDERS_FILE=$C_PROVIDERS STUB_API_KEY=smoke-155 $APP"
C_CACHE="$SMOKE_CFG/model-cache.json"

# How many times the stub has been asked for its list.
c_lists() { grep -c '^GET /v1/models' "$C_LOG" || true; }

# ---- (a) the first /model fetches the list and keeps it ----
launch "$S155" 100 30 "$C_APP"
submit "$S155" "/model"
first="$(wait_pane 10 "$S155" -F "updated just now")" || fail "the first /model never settled on a fetched list"
dump "the first /model" "$first"
expect_has "$first" -E '^  → stub/model-00 \[stub\]' "the first /model did not list the stub's models"
expect_has "$first" -F "(1/3)   ·   updated just now   ·   ctrl+r to refresh" \
	"the counter does not say how fresh the list is and which key refreshes it"
expect_eq "$(c_lists)" "1" "the first /model did not ask the stub for its list exactly once"
expect_file_has "$C_CACHE" -F '"stub":{' "model-cache.json does not keep the list under its provider"
expect_file_has "$C_CACHE" -F '"id":"stub/model-02"' "model-cache.json does not hold the rows"
expect_file_lacks "$C_CACHE" -F "127.0.0.1" "model-cache.json carries the provider's address"
keys "$S155" Escape
wait_for 5 "$S155" -F "dummy_model_name" || fail "Esc did not close /model"

# ---- (b) a second /model, inside the hour, asks nobody ----
submit "$S155" "/model"
second="$(wait_pane 5 "$S155" -F "stub/model-02")" || fail "the second /model did not list the kept models"
dump "the second /model" "$second"
expect_lacks "$second" -F "Loading models…" "the second /model waited for a list it had kept"
sleep 0.5 # a window for a request that must not come
expect_eq "$(c_lists)" "1" "a second /model inside the hour asked the stub again"
keys "$S155" Escape
wait_for 5 "$S155" -F "dummy_model_name" || fail "Esc did not close the second /model"

# ---- (c) a restart reads the list back from the file ----
tmux kill-session -t "$S155" 2>/dev/null
launch "$S155" 100 30 "$C_APP"
submit "$S155" "/model"
restarted="$(wait_pane 5 "$S155" -F "stub/model-02")" || fail "a restarted /model did not list the kept models"
dump "/model after a restart" "$restarted"
expect_has "$restarted" -E "updated (just now|[0-9]+m ago)   ·   ctrl\+r to refresh" \
	"a restarted /model does not say how old the kept list is"
sleep 0.5 # a window for a request that must not come
expect_eq "$(c_lists)" "1" "a restarted session asked the stub instead of reading model-cache.json"

# ---- (d) Ctrl+R asks again — once — behind the kept rows ----
touch "$C_FLAGS/slow" "$C_FLAGS/extra"
keys "$S155" C-r
refreshing="$(wait_pane 3 "$S155" -F "refreshing…")" || fail "Ctrl+R did not say it is refreshing"
dump "/model refreshing" "$refreshing"
expect_has "$refreshing" -E '^  → stub/model-00 \[stub\]' "the kept rows left the screen while the refresh was out"
expect_lacks "$refreshing" -F "ctrl+r to refresh" "the key was offered while its refresh was out"
keys "$S155" C-r # pressed again while the answer is out: nothing more is asked
refreshed="$(wait_pane 10 "$S155" -F "stub/model-new")" || fail "the refreshed list never landed"
rm -f "$C_FLAGS/slow"
dump "/model refreshed" "$refreshed"
expect_has "$refreshed" -F "(1/4)   ·   updated just now   ·   ctrl+r to refresh" \
	"the refreshed list's counter is not fresh"
expect_eq "$(c_lists)" "2" "Ctrl+R did not ask the stub exactly once"
expect_file_has "$C_CACHE" -F '"id":"stub/model-new"' "the refreshed list was not kept"

# ---- (e) a refused refresh keeps the rows ----
touch "$C_FLAGS/refuse"
keys "$S155" C-r
refused="$(wait_pane 10 "$S155" -F "couldn't refresh Stub")" || fail "a refused refresh was not noted beside the counter"
rm -f "$C_FLAGS/refuse"
dump "/model after a refused refresh" "$refused"
expect_has "$refused" -F "stub/model-new" "a refused refresh took the kept rows away"
expect_has "$refused" -F "upstream busy" "the refusal's reason is not on the page"
expect_has "$refused" -F "couldn't refresh Stub   ·   ctrl+r to refresh" "the page does not say how to try again"
expect_file_has "$C_CACHE" -F '"id":"stub/model-new"' "a refused refresh emptied the kept list"
keys "$S155" Escape
wait_for 5 "$S155" -F "dummy_model_name" || fail "Esc did not close /model after the refusal"

# ---- (f) /login saving the key drops the kept list ----
submit "$S155" "/login"
wait_for 5 "$S155" -F "Sign in with an API key" || fail "/login did not open"
keys "$S155" Down
sleep 0.2
keys "$S155" Enter
wait_for 5 "$S155" -F "Keys are saved to" || fail "/login's API-key list did not open"
type_text "$S155" "stub"
sleep 0.3
keys "$S155" Enter
wait_for 5 "$S155" -F "paste your API key" || fail "/login did not ask for the stub's key"
type_text "$S155" "smoke-155-new"
sleep 0.2
keys "$S155" Enter
saved="$(wait_pane 5 "$S155" -F "Saved STUB_API_KEY")" || fail "/login did not save the stub's key"
dump "/login saved the key" "$saved"
expect_file_lacks "$C_CACHE" -F '"stub":{' "saving the provider's key kept the list read with the old one"
submit "$S155" "/model"
wait_for 10 "$S155" -F "updated just now" || fail "/model after the new key never settled"
expect_eq "$(c_lists)" "4" "/model after the new key did not ask the stub again"
keys "$S155" Escape
