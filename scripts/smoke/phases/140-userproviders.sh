#!/usr/bin/env bash
# Phase 140 — a provider of the user's own, from the config home's providers.toml

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# A user's own providers.toml (docs/user-providers.md). The file in the
# config home is laid over the built-in providers: a new id is a provider
# beside the shipped ones, never instead of them. A local stub stands in for
# an OpenAI-compatible API — it lists its models as a bare JSON array, the
# shape Together AI answers with, streams one reply, and logs every request's
# Authorization header, its body's keys and its temperature. The config home
# here is the default one, `~/.alter-zero` under the phase's own HOME, so the
# toasts name the file the way a user sees it. Against the real binary:
#   (a) /login lists the user's provider beside the shipped ones, configured
#       by `ACME_API_KEY` (the id's default variable), and its key step shows
#       the file's description and key page;
#   (b) /model lists the stub's models under `[acme]`, a pick switches to one,
#       and a turn streams from the stub with the key riding as a bearer and
#       without the field the block's `drop_params` names;
#   (c) the file is re-read when /login opens: a provider added mid-session
#       is a row at once, a block the file gets wrong is a red toast naming
#       the fix — and the toast comes back at the next open, since one said
#       once is easily lost under the next;
#   (d) a file saved half-edited under the running model keeps the session's
#       providers, and a /settings change still reaches the request — first
#       for a file that does not parse, then for a block of the provider in
#       use that can't be used;
#   (e) a file that does not parse at launch is a startup toast naming its
#       line, and the shipped providers stay;
#   (f) ALTER_ZERO_CONFIG_DIR moves the file with the rest of the state:
#       `~/.alter-zero/providers.toml` is not read under it;
#   (g) a providers.toml in the working directory is never read — a checkout
#       must not be able to point a stored key at a host of its choosing.
S140="${S}_userprov"
P_LOG="$SMOKE_TMP/requests.log"
P_PORT="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
python3 - "$P_PORT" "$P_LOG" <<'PY' &
import json, sys
from http.server import BaseHTTPRequestHandler, HTTPServer

port, log = int(sys.argv[1]), sys.argv[2]
models = [
    {"id": "acme/coder-large", "object": "model", "display_name": "Acme Coder Large",
     "context_length": 65536},
    {"id": "acme/coder-small", "object": "model"},
]

class Stub(BaseHTTPRequestHandler):
    def note(self, fields="-", temperature="-"):
        with open(log, "a") as f:
            f.write(f"{self.command} {self.path} {self.headers.get('authorization')} "
                    f"{fields} temperature={temperature}\n")

    def do_GET(self):
        self.note()
        if self.path.startswith("/v1/models"):
            data = json.dumps(models).encode()  # no {"data": …} envelope
            self.send_response(200)
            self.send_header("content-type", "application/json")
            self.send_header("content-length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)
        else:
            self.send_response(404)
            self.end_headers()

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers.get("content-length") or 0)))
        self.note(",".join(sorted(body)), body.get("temperature", "-"))
        self.send_response(200)
        self.send_header("content-type", "text/event-stream")
        self.end_headers()
        for frame in (
            {"choices": [{"index": 0, "delta": {"content": "Hello from Acme."}}]},
            {"choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]},
        ):
            self.wfile.write(f"data: {json.dumps(frame)}\n\n".encode())
        self.wfile.write(b"data: [DONE]\n\n")

    def log_message(self, *_args):
        pass

HTTPServer(("127.0.0.1", port), Stub).serve_forever()
PY
P_SERVER=$!
smoke_on_exit kill "$P_SERVER"
for _ in $(seq 1 30); do # wait for the stub to listen
	if python3 -c "import socket, sys; s = socket.socket(); s.settimeout(0.2); sys.exit(0 if s.connect_ex(('127.0.0.1', $P_PORT)) == 0 else 1)" 2>/dev/null; then
		break
	fi
	sleep 0.1
done

# The user layer lives in the default config home — no ALTER_ZERO_CONFIG_DIR
# (the launch below takes the suite's out) and no ALTER_ZERO_PROVIDERS_FILE,
# which would stand alone instead of layering.
P_HOME_CFG="$SMOKE_HOME/.alter-zero"
P_FILE="$P_HOME_CFG/providers.toml"
mkdir -p "$P_HOME_CFG"
P_ACME="# A provider of my own, beside the shipped ones.
[providers.acme]
name = \"Acme Cloud\"
description = \"Acme's hosted coding models, reached with a key from your Acme account.\"
api_key_url = \"https://acme.example/keys\"
drop_params = [\"prompt_cache_key\"]

[providers.acme.kwargs]
api_base = \"http://127.0.0.1:$P_PORT/v1\""
printf '%s\n' "$P_ACME" >"$P_FILE"

# (g)'s bait, in the working directory: a block that would add a row.
P_DIR="$(work_dir userprov)"
cat >"$P_DIR/providers.toml" <<'TOML'
[providers.planted]
name = "Planted Provider"

[providers.planted.kwargs]
api_base = "https://planted.example/v1"
TOML

# The suite's environment, then a second `env` taking its config home back
# out, so the app resolves the default `$HOME/.alter-zero` itself.
P_APP="env NO_PROXY=127.0.0.1 no_proxy=127.0.0.1 $CFG_ENV_NOHIST ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS ACME_API_KEY=smoke-140 env -u ALTER_ZERO_CONFIG_DIR $BIN_ABS"
P_SHOWN="~/.alter-zero/providers.toml"

# p_close SESSION — Esc out of whatever page is open, back to the composer.
p_close() {
	local _
	for _ in 1 2 3 4; do
		pane_has "$1" -F "dummy_model_name ·" && return 0
		pane_has "$1" -F "acme/coder-large ·" && return 0
		keys "$1" Escape
		sleep 0.3
	done
	fail "Esc did not close the page back to the composer"
}
# p_key_list SESSION — open /login's API-key list and print it.
p_key_list() {
	submit "$1" "/login"
	wait_for 5 "$1" -F "Use an API key" || fail "/login did not open"
	keys "$1" Down
	sleep 0.2
	keys "$1" Enter
	wait_pane 5 "$1" -F "Keys are saved to"
}
# p_temperature SESSION VALUE — cycle /settings' Temperature once, to VALUE
# (its row's label), and close the menu.
p_temperature() {
	submit "$1" "/settings"
	wait_for 5 "$1" -F "Enter/Space to change" || fail "/settings did not open"
	type_text "$1" "temperature"
	wait_for 5 "$1" -F "(1/1)" || fail "the search did not narrow /settings to Temperature"
	keys "$1" Enter
	wait_for 5 "$1" -E "Temperature +$2" || fail "Enter did not cycle Temperature to $2"
	p_close "$1"
}
# p_turn SESSION TEXT COUNT — send TEXT, wait for the COUNTth turn summary,
# and print the stub's last chat request; 1 when the turn never settled. (It
# runs in a command substitution, where a `fail` would not be counted.)
p_turn() {
	submit "$1" "$2"
	wait_summaries 20 "$1" "$3" -S -300 >/dev/null || return 1
	grep '^POST ' "$P_LOG" | tail -n 1
}

# ---- (a) /login: the user's provider beside the shipped ones ----
launch -c "$P_DIR" "$S140" 150 40 "$P_APP"
start="$(pane "$S140")"
dump "launched over the user's providers.toml" "$start"
expect_lacks "$start" -F "providers.toml:" "a valid providers.toml raised a toast"
keys_list="$(p_key_list "$S140")" || fail "/login's API-key list did not open"
dump "/login's API-key list" "$keys_list"
expect_has "$keys_list" -E "Acme Cloud · ✔ configured" \
	"the user's provider is not a configured row (ACME_API_KEY did not resolve it)"
for shipped in "Agent Zero API" "OpenRouter" "Venice" "Anthropic"; do
	expect_has "$keys_list" -F "$shipped" \
		"the shipped \"$shipped\" went missing — the file replaced the table instead of adding to it"
done
expect_lacks "$keys_list" -F "Planted Provider" "a providers.toml in the working directory was read"
type_text "$S140" "acme"
sleep 0.3
keys "$S140" Enter
key_step="$(wait_pane 5 "$S140" -F "acme.example/keys")" || fail "the key step did not link the file's api_key_url"
dump "the key step for the user's provider" "$key_step"
expect_has "$key_step" -F "Acme's hosted coding models" "the key step did not show the file's description"
p_close "$S140"

# ---- (b) /model lists the stub's models, and a turn streams from it ----
submit "$S140" "/model"
picker="$(wait_pane 10 "$S140" -F "acme/coder-small")" || fail "/model did not list the stub's bare-array models"
dump "/model over the user's provider" "$picker"
expect_has "$picker" -E "acme/coder-large +\[acme\]" "the model rows are not tagged with the user's provider"
expect_lacks "$picker" -F "unavailable" "a provider fetch failed"
keys "$S140" Enter
wait_for 5 "$S140" -F "acme/coder-large ·" || fail "picking a model did not switch the footer to it"
first="$(p_turn "$S140" "hello acme" 1)" || fail "the first turn never settled"
reply="$(pane "$S140")"
dump "a turn against the user's provider" "$reply"
expect_has "$reply" -F "Hello from Acme." "the turn never streamed from the user's provider"
dump "what the stub was sent" "$(cat "$P_LOG")"
expect_file_has "$P_LOG" -F "GET /v1/models Bearer smoke-140" "the listing did not carry the key as a bearer"
expect_has "$first" -F "POST /v1/chat/completions Bearer smoke-140" "the chat request did not carry the key as a bearer"
expect_has "$first" -E "[ ,]stream_options(,| )" "the chat request lost a field drop_params did not name"
expect_lacks "$first" -F "prompt_cache_key" "the chat request still carried the prompt_cache_key drop_params names"
expect_has "$first" -F "temperature=-" "a request sent a temperature nobody chose"

# ---- (c) the file is re-read when /login opens, and says what it gets wrong ----
cat >>"$P_FILE" <<'TOML'

[providers.later]
name = "Later Labs"

[providers.later.kwargs]
api_base = "https://later.example/v1"

[providers.zeta]
name = "Zeta"
base_url = "https://zeta.example/v1"
TOML
reread="$(p_key_list "$S140")" || fail "/login's API-key list did not open after the edit"
dump "/login after the file grew mid-session" "$reread"
expect_has "$reread" -F "Later Labs" "a provider added mid-session is not a row — /login did not re-read the file"
expect_has "$reread" -F "$P_SHOWN: providers.zeta.base_url is not a known key — set api_base under [providers.zeta.kwargs] (+1 more)" \
	"the misplaced base_url raised no toast naming the file and the fix"
expect_lacks "$reread" -F "Zeta ·" "a block with no chat base became a row"
expect_has "$reread" -F "Acme Cloud" "the edit cost the provider that was already there"
p_close "$S140"
# Said again at the next open — the first toast may have gone unseen.
sleep 4.5 # outlast the first toast
again="$(p_key_list "$S140")" || fail "/login did not reopen"
dump "/login reopened over the same file" "$again"
expect_has "$again" -F "providers.zeta.base_url is not a known key" "a file still wrong was not said again when /login reopened"
p_close "$S140"

# ---- (d) a file saved half-edited keeps the running model's provider ----
# Not TOML at all: the whole table stays as it was, and a /settings change
# made meanwhile is a rebuild that still finds its provider. A re-read that
# swapped in the built-ins instead skipped that rebuild without a word, and
# the temperature never reached the wire.
printf '[providers.acme\nname = "half an edit"\n' >"$P_FILE"
half="$(p_key_list "$S140")" || fail "/login did not open over the half-saved file"
dump "/login over a half-saved file" "$half"
expect_has "$half" -F "$P_SHOWN: line 1, column" "the half-saved file raised no toast naming the file and the line"
expect_has "$half" -F "— keeping the providers already loaded" "the toast did not say the session kept its providers"
expect_has "$half" -F "Acme Cloud" "a file saved half-edited dropped the providers it had defined"
p_close "$S140"
p_temperature "$S140" "0.0"
second="$(p_turn "$S140" "hello again" 2)" || fail "the turn over the half-saved file never settled"
dump "the request after a /settings change over the half-saved file" "$second"
expect_has "$second" -F "POST /v1/chat/completions Bearer smoke-140" "the turn did not reach the user's provider"
expect_has "$second" -E "temperature=0(\.0)?$" "the Temperature change never reached the request — the rebuild lost its provider"
# A block of the provider in use that can't be used: the definition the
# session runs on stays, so the next request still goes to the stub.
sed "s#http://127.0.0.1:$P_PORT/v1#127.0.0.1:$P_PORT#" <<<"$P_ACME" >"$P_FILE"
kept="$(p_key_list "$S140")" || fail "/login did not open over the broken block"
dump "/login over a broken block of the provider in use" "$kept"
expect_has "$kept" -F "$P_SHOWN: providers.acme.kwargs.api_base \"127.0.0.1:$P_PORT\" is not an http(s) URL — kept the one in use" \
	"the broken block of the provider in use was not said, or not kept"
expect_has "$kept" -F "Acme Cloud" "the provider in use left /login with its broken block"
p_close "$S140"
p_temperature "$S140" "0.3"
third="$(p_turn "$S140" "and once more" 3)" || fail "the turn over the broken block never settled"
dump "the request after a /settings change over the broken block" "$third"
expect_has "$third" -F "POST /v1/chat/completions Bearer smoke-140" "the kept definition did not reach the stub"
expect_has "$third" -E "temperature=0\.3" "the Temperature change never reached the request over the broken block"
# Fixed: nothing more to say.
printf '%s\n' "$P_ACME" >"$P_FILE"
fixed="$(p_key_list "$S140")" || fail "/login did not open over the fixed file"
dump "/login over the fixed file" "$fixed"
expect_lacks "$fixed" -F "providers.toml:" "a fixed file still raised a toast"
expect_has "$fixed" -F "Acme Cloud" "the fixed file lost its provider"
p_close "$S140"
submit "$S140" "/quit"
wait_gone 5 "$S140" || fail "/quit did not exit"
tmux kill-session -t "$S140" 2>/dev/null

# ---- (e) a file that does not parse at launch: a toast, the built-ins stay ----
printf '[providers.acme]\nname = Acme Cloud\n' >"$P_FILE"
launch -c "$P_DIR" "$S140" 150 40 "$P_APP"
broken="$(wait_pane 5 "$S140" -F "$P_SHOWN: line 2, column")" || fail "a providers.toml that does not parse raised no startup toast"
dump "launched over a providers.toml that does not parse" "$broken"
expect_has "$broken" -F "— using the built-in providers" "the startup toast did not say what the session fell back to"
fallback="$(p_key_list "$S140")" || fail "/login did not open over the broken file"
dump "/login over the broken file" "$fallback"
expect_has "$fallback" -F "$P_SHOWN: line 2, column" "/login did not say again what is wrong with the file"
expect_has "$fallback" -F "OpenRouter" "the shipped providers did not stand in for the broken file"
expect_lacks "$fallback" -F "Acme Cloud" "a provider from a file that does not parse was loaded"
p_close "$S140"
tmux kill-session -t "$S140" 2>/dev/null

# ---- (f) ALTER_ZERO_CONFIG_DIR moves the file with the rest of the state ----
# `~/.alter-zero/providers.toml` holds Acme again, and must go unread: the
# config home is elsewhere and has a file of its own. An
# ALTER_ZERO_PROVIDERS_FILE set to nothing names no file — the empty-is-unset
# rule every variable here keeps — so the config home's file still layers.
printf '%s\n' "$P_ACME" >"$P_FILE"
cat >"$SMOKE_CFG/providers.toml" <<'TOML'
[providers.moved]
name = "Moved Corp"

[providers.moved.kwargs]
api_base = "https://moved.example/v1"
TOML
launch -c "$P_DIR" "$S140" 150 40 "env $CFG_ENV_NOHIST ALTER_ZERO_PROVIDERS_FILE= ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN_ABS"
moved_start="$(pane "$S140")"
dump "launched under a moved config home" "$moved_start"
expect_lacks "$moved_start" -F "providers.toml:" "a toast was raised under the moved config home"
expect_lacks "$moved_start" -F "os error" "an empty ALTER_ZERO_PROVIDERS_FILE was read as a file name"
moved="$(p_key_list "$S140")" || fail "/login did not open under the moved config home"
dump "/login under a moved config home" "$moved"
expect_has "$moved" -F "Moved Corp" "the config home's own providers.toml was not read"
expect_lacks "$moved" -F "Acme Cloud" "~/.alter-zero/providers.toml was read with the config home elsewhere"
expect_lacks "$moved" -F "Planted Provider" "a providers.toml in the working directory was read"
p_close "$S140"
tmux kill-session -t "$S140" 2>/dev/null
echo "==== Phase 140: the user's providers.toml adds providers, keeps the one in use, and says what it gets wrong ===="
