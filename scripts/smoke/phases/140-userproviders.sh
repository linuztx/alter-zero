#!/usr/bin/env bash
# Phase 140 — a provider of the user's own, from the config home's providers.toml

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# A user's own providers.toml (docs/user-providers.md). The file in the
# config home is laid over the built-in providers: a new id is a provider
# beside the shipped ones, never instead of them. A local stub stands in for
# an OpenAI-compatible API — it lists its models as a bare JSON array, the
# shape Together AI answers with, streams one reply, and logs every request's
# Authorization header. Against the real binary:
#   (a) /login lists the user's provider beside the shipped ones, configured
#       by `ACME_API_KEY` (the id's default variable), and its key step shows
#       the file's description and key page;
#   (b) /model lists the stub's models under `[acme]`, a pick switches to one,
#       and a turn streams from the stub with the key riding as a bearer and
#       without the field the block's `drop_params` names;
#   (c) the file is re-read when /login opens: a provider added mid-session
#       is a row at once, and a block the file gets wrong is a red toast
#       naming the fix;
#   (d) a file that does not parse is a startup toast naming its line, and
#       the shipped providers stay;
#   (e) a providers.toml in the working directory is never read — a checkout
#       must not be able to point a stored key at a host of its choosing.
S140="${S}_userprov"
P_LOG="$SMOKE_TMP/auth.log"
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
    def note(self, fields="-"):
        with open(log, "a") as f:
            f.write(f"{self.command} {self.path} {self.headers.get('authorization')} {fields}\n")

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
        self.note(",".join(sorted(body)))
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

# The user layer lives in the config home the suite gives every phase — no
# ALTER_ZERO_PROVIDERS_FILE, which would stand alone instead of layering.
P_FILE="$SMOKE_CFG/providers.toml"
cat >"$P_FILE" <<TOML
# A provider of my own, beside the shipped ones.
[providers.acme]
name = "Acme Cloud"
description = "Acme's hosted coding models, reached with a key from your Acme account."
api_key_url = "https://acme.example/keys"
drop_params = ["prompt_cache_key"]

[providers.acme.kwargs]
api_base = "http://127.0.0.1:$P_PORT/v1"
TOML

# (e)'s bait, in the working directory: a block that would add a row.
P_DIR="$(work_dir userprov)"
cat >"$P_DIR/providers.toml" <<'TOML'
[providers.planted]
name = "Planted Provider"

[providers.planted.kwargs]
api_base = "https://planted.example/v1"
TOML

P_APP="env NO_PROXY=127.0.0.1 no_proxy=127.0.0.1 $CFG_ENV_NOHIST ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS ACME_API_KEY=smoke-140 $BIN_ABS"

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
submit "$S140" "hello acme"
reply="$(wait_settled 20 "$S140" -F "Hello from Acme.")" || fail "the turn never streamed from the user's provider"
dump "a turn against the user's provider" "$reply"
expect_file_has "$P_LOG" -F "GET /v1/models Bearer smoke-140" "the listing did not carry the key as a bearer"
expect_file_has "$P_LOG" -F "POST /v1/chat/completions Bearer smoke-140" "the chat request did not carry the key as a bearer"
dump "what the stub was sent" "$(cat "$P_LOG")"
expect_file_has "$P_LOG" -E "^POST .*[ ,]stream_options(,|$)" "the chat request lost a field drop_params did not name"
if grep -q "prompt_cache_key" "$P_LOG"; then
	fail "the chat request still carried the prompt_cache_key drop_params names"
fi

# ---- (c) the file is re-read when /login opens ----
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
expect_has "$reread" -F "providers.toml: providers.zeta.base_url is not a known key — set api_base under [providers.zeta.kwargs] (+1 more)" \
	"the misplaced base_url raised no toast naming the fix"
expect_lacks "$reread" -F "Zeta ·" "a block with no chat base became a row"
expect_has "$reread" -F "Acme Cloud" "the edit cost the provider that was already there"
p_close "$S140"
# The same broken file is said once, not at every open.
sleep 4.5 # outlast the first toast
again="$(p_key_list "$S140")" || fail "/login did not reopen"
dump "/login reopened over the same file" "$again"
expect_lacks "$again" -F "providers.zeta" "an unchanged file's issue was toasted twice"
p_close "$S140"
submit "$S140" "/quit"
wait_gone 5 "$S140" || fail "/quit did not exit"
tmux kill-session -t "$S140" 2>/dev/null

# ---- (d) a file that does not parse: a startup toast, the built-ins stay ----
printf '[providers.acme]\nname = Acme Cloud\n' >"$P_FILE"
launch -c "$P_DIR" "$S140" 150 40 "$P_APP"
broken="$(wait_pane 5 "$S140" -F "providers.toml: line 2, column")" || fail "a providers.toml that does not parse raised no startup toast"
dump "launched over a providers.toml that does not parse" "$broken"
expect_has "$broken" -F "— using the built-in providers" "the startup toast did not say what the session fell back to"
fallback="$(p_key_list "$S140")" || fail "/login did not open over the broken file"
dump "/login over the broken file" "$fallback"
expect_has "$fallback" -F "OpenRouter" "the shipped providers did not stand in for the broken file"
expect_lacks "$fallback" -F "Acme Cloud" "a provider from a file that does not parse was loaded"
p_close "$S140"
tmux kill-session -t "$S140" 2>/dev/null
echo "==== Phase 140: the user's providers.toml adds providers, and says what it gets wrong ===="
