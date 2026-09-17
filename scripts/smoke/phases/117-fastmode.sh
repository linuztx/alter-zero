#!/usr/bin/env bash
# Phase 117 — the per-tier speed commands: none on a model listing none, one each on a model that lists some

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# Fast mode is a per-tier palette command (docs/fast-mode.md): every speed
# tier the active model's listing names becomes a row of its own — `/fast`,
# `/ultrafast`, whatever a record lists — spliced in after /model, codex's
# own placement, and nothing about a tier is hardcoded. Two halves prove the
# two sides of that. The offline dummy lists no tier, so it must show NO
# such row: typing `/fast` matches nothing, Enter is swallowed rather than
# sent as a message, and nothing claims a tier that was never listed — no
# footer word, no `Speed:` toast. Then a ChatGPT Codex model whose listing
# names TWO tiers, served by a local stub standing in for both the token
# mint (`ALTER_ZERO_OPENAI_ISSUER`) and the `/models` catalog (a smoke
# provider file pointing `api_base` at the stub), must show a row per tier
# with the backend's own description, toggle each — a tier's command
# selects it, the same command again is the way back to standard, another
# tier's command switches straight over — with the toast repeating the
# record's cost statement and the footer wearing the selection's name. The
# refresh token rides in under the variable's OLD name, which is the rename's
# compatibility shim under test at the same time (docs/chatgpt.md): the
# provider still activates, and the rotation the stub answers with is
# written back under the NEW name.

# ---- half one: a model listing no tier lists no tier command ----
S117="${S}_fast"
launch "$S117" 80 24
type_text "$S117" "/fast"
sleep 0.4
none_palette="$(tmux capture-pane -t "$S117" -p)"
dump "the dummy's palette on /fast" "$none_palette"
expect_has "$none_palette" -F "No matching commands" "the dummy's palette offered something for /fast"
expect_lacks "$none_palette" -E '^/fast' "the palette lists a /fast row on a model that lists no tier"
expect_lacks "$none_palette" -F "increased usage" "the palette describes a tier the model never listed"
keys "$S117" Enter
sleep 0.6
none_enter="$(tmux capture-pane -t "$S117" -p)"
dump "Enter on the unmatched /fast" "$none_enter"
expect_has "$none_enter" -F "❯ /fast" "the unmatched draft did not stay in the composer"
expect_lacks "$none_enter" -F "esc to interrupt" "/fast was sent to the model as a message"
expect_lacks "$none_enter" -F "Speed:" "a tier was reported switched on a model that lists none"
expect_lacks "$none_enter" -F "dummy_model_name fast" "the footer wears a tier the model does not list"
# The draft stays, so the band stays open (`No matching commands`) and, as
# every band does, displaces the footer; Esc closes it and the footer is
# back, wearing no tier word.
keys "$S117" Escape
sleep 0.4
none_footer="$(tmux capture-pane -t "$S117" -p)"
dump "the footer after the band closes" "$none_footer"
expect_has "$none_footer" -F "dummy_model_name ·" "the footer is gone"
expect_lacks "$none_footer" -F "dummy_model_name fast" "the footer wears a tier the model does not list"
tmux kill-session -t "$S117" 2>/dev/null

# ---- half two: a listing naming two tiers is two commands ----
S117B="${S}_tiers"
T_LOG="$SMOKE_TMP/tiers.log"
T_PORT="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
python3 - "$T_PORT" "$T_LOG" <<'PY' &
import base64, json, sys
from http.server import BaseHTTPRequestHandler, HTTPServer

port, log = int(sys.argv[1]), sys.argv[2]

def b64url(raw):
    return base64.urlsafe_b64encode(raw).rstrip(b"=").decode()

# An access token whose claims name the seat; the app never verifies the
# signature (docs/chatgpt.md), only reads the payload.
claims = {
    "https://api.openai.com/auth": {"chatgpt_account_id": "acct-117", "chatgpt_plan_type": "pro"},
    "exp": 4102444800,
}
jwt = ".".join([b64url(b'{"alg":"none"}'), b64url(json.dumps(claims).encode()), b64url(b"sig")])

# The catalog: one model, two tiers, in the record's own shape
# (docs/fast-mode.md). No reasoning levels, so the footer wears no mode word
# and the tier follows the model name directly.
catalog = {"models": [{
    "slug": "gpt-5.6-sol",
    "display_name": "GPT-5.6 Sol",
    "service_tiers": [
        {"id": "priority", "name": "Fast", "description": "1.5x speed, increased usage"},
        {"id": "ultrafast", "name": "Ultrafast", "description": "The fastest available responses."},
    ],
}]}

class Stub(BaseHTTPRequestHandler):
    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("content-length") or 0)).decode()
        with open(log, "a") as f:
            f.write(f"POST {self.path} {body}\n")
        if self.path == "/oauth/token":
            # The mint rotates the refresh token, as OpenAI may.
            self.reply(200, {"access_token": jwt, "refresh_token": "rt-117-rotated"})
        else:
            self.reply(404, {})

    def do_GET(self):
        with open(log, "a") as f:
            f.write(f"GET {self.path}\n")
        if self.path.startswith("/models"):
            self.reply(200, catalog)
        else:
            self.reply(404, {})

    def reply(self, status, obj):
        data = json.dumps(obj).encode()
        self.send_response(status)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, *_args):
        pass

HTTPServer(("127.0.0.1", port), Stub).serve_forever()
PY
T_SERVER=$!
for _ in $(seq 1 30); do # wait for the stub to listen
	if python3 -c "import socket, sys; s = socket.socket(); s.settimeout(0.2); sys.exit(0 if s.connect_ex(('127.0.0.1', $T_PORT)) == 0 else 1)" 2>/dev/null; then
		break
	fi
	sleep 0.1
done
T_BASE="http://127.0.0.1:$T_PORT"
# The provider file: the real one's ChatGPT Codex block with its base pointed
# at the stub, so the listing (and any request) goes there and nowhere else.
T_PROVIDERS="$SMOKE_TMP/providers.toml"
cat >"$T_PROVIDERS" <<TOML
[providers.chatgpt_codex]
name = "ChatGPT Codex"
auth = "chatgpt_codex"
wire_api = "responses"
description = "Sign in with your ChatGPT Plus/Pro account"
api_key_env = "CHATGPT_CODEX_REFRESH_TOKEN"

[providers.chatgpt_codex.kwargs]
api_base = "$T_BASE"
TOML
# NO_PROXY keeps a developer's HTTP proxy out of the loopback requests. The
# token rides in under the variable's OLD name (the shim under test); the
# provider and model are pinned so the real backend activates with nothing
# saved, which is exactly what makes the startup probe fetch the listing.
T_DIR="$(work_dir)"
launch -c "$T_DIR" "$S117B" 100 30 "env NO_PROXY=127.0.0.1 no_proxy=127.0.0.1 ALTER_ZERO_OPENAI_ISSUER=$T_BASE ALTER_ZERO_PROVIDERS_FILE=$T_PROVIDERS ALTER_ZERO_PROVIDER=chatgpt_codex ALTER_ZERO_MODEL=gpt-5.6-sol OPENAI_CHATGPT_REFRESH_TOKEN=rt-117-legacy $APP_ABS"

# The palette: the listed tiers each a row, right after /model, wearing the
# record's own description. The probe answers in the background, and the
# open palette re-derives its rows as it lands.
type_text "$S117B" "/"
tiers_palette="$(wait_pane 10 "$S117B" -F "1.5x speed, increased usage")"
dump "the palette with the listed tiers" "$tiers_palette"
expect_has "$tiers_palette" -E '^/model +Switch the active model' "the palette lost /model"
expect_has "$tiers_palette" -E '^/fast +1\.5x speed, increased usage' "/fast is not a row wearing the record's description"
model_row="$(printf '%s\n' "$tiers_palette" | grep -nE '^/model ' | cut -d: -f1)"
fast_row="$(printf '%s\n' "$tiers_palette" | grep -nE '^/fast ' | cut -d: -f1)"
if [ -z "$model_row" ] || [ -z "$fast_row" ] || [ "$fast_row" -ne $((model_row + 1)) ]; then
	fail "/fast (row ${fast_row:-none}) does not sit right after /model (row ${model_row:-none})"
fi
expect_lacks "$tiers_palette" -F "Toggle fast mode" "the palette carries a hardcoded /fast description"
# The second tier is a command of its own — reached by its own name.
keys "$S117B" C-u
type_text "$S117B" "/ultra"
sleep 0.4
ultra_palette="$(tmux capture-pane -t "$S117B" -p)"
dump "the palette on /ultra" "$ultra_palette"
expect_has "$ultra_palette" -E '^/ultrafast +The fastest available responses\.' "/ultrafast is not a row wearing the record's description"
expect_lacks "$ultra_palette" -E '^/fast' "/ultra matched /fast"

# Running /ultrafast selects it: the toast repeats the record's own cost
# statement and the footer wears the tier's name after the model's.
keys "$S117B" Enter
ultra_on="$(wait_pane 8 "$S117B" -F "Speed: ultrafast — The fastest available responses.")"
dump "/ultrafast selected" "$ultra_on"
expect_has "$ultra_on" -F "gpt-5.6-sol ultrafast ·" "the footer does not wear the selected tier"
expect_lacks "$ultra_on" -F "❯ /ultra" "/ultrafast was sent as a message instead of run"

# The same command again is the way back to standard.
type_text "$S117B" "/ultrafast"
sleep 0.3
keys "$S117B" Enter
ultra_off="$(wait_pane 8 "$S117B" -F "Speed: standard")"
dump "/ultrafast again" "$ultra_off"
expect_lacks "$ultra_off" -F "gpt-5.6-sol ultrafast" "the footer kept the tier after the switch back to standard"
expect_has "$ultra_off" -F "gpt-5.6-sol ·" "the footer lost the model name"

# Each tier is its own switch: /fast selects fast, and /ultrafast on a fast
# session switches straight over, with no standard step between.
type_text "$S117B" "/fast"
sleep 0.3
keys "$S117B" Enter
fast_on="$(wait_pane 8 "$S117B" -F "Speed: fast — 1.5x speed, increased usage")"
dump "/fast selected" "$fast_on"
expect_has "$fast_on" -F "gpt-5.6-sol fast ·" "the footer does not wear fast"
type_text "$S117B" "/ultra"
sleep 0.3
keys "$S117B" Enter
switched="$(wait_pane 8 "$S117B" -F "gpt-5.6-sol ultrafast ·")"
dump "/ultrafast over fast" "$switched"
expect_has "$switched" -F "Speed: ultrafast" "the switch from fast to ultrafast did not toast"
expect_lacks "$switched" -F "Speed: standard" "the switch between two tiers went through standard"

# What went over the wire, off the stub's log: ONE listing fetch (the startup
# probe, carrying the client version the backend gates on) after the mint —
# a tier command rebinds the next turn's backend and never refetches.
t_log="$(cat "$T_LOG" 2>/dev/null)"
dump "the stub's log" "$t_log"
expect_has "$t_log" -E '^POST /oauth/token .*"grant_type":"refresh_token".*"refresh_token":"rt-117-legacy"' "the token stored under the old variable name was not presented to the mint"
t_lists="$(printf '%s\n' "$t_log" | grep -c '^GET /models?client_version=' || true)"
expect_eq "${t_lists:-0}" 1 "the listing was fetched other than once, by the startup probe"
# …and the rotation the mint answered with landed in the key store under the
# variable's NEW name — the shim reads the old name, the write-back uses the
# current one (docs/chatgpt.md).
t_env="$(cat "$SMOKE_CFG/.env" 2>/dev/null)"
dump "the key store" "$t_env"
expect_has "$t_env" -F "CHATGPT_CODEX_REFRESH_TOKEN=rt-117-rotated" "the rotated refresh token was not stored under the current variable name"
expect_lacks "$t_env" -F "OPENAI_CHATGPT_REFRESH_TOKEN" "the rotation was written under the old variable name"

kill "$T_SERVER" 2>/dev/null
tmux kill-session -t "$S117B" 2>/dev/null
