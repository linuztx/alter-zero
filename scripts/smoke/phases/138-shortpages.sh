#!/usr/bin/env bash
# Phase 138 — the pickers on a short terminal

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# The composer-replacing pickers on a terminal too short for their whole page
# (docs/view-flow.md *A page that sheds rows before it flows*). Bottom-
# anchoring such a page kept its tail — a description, a preview, the hint —
# and flowed its search line and its highlighted row into scrollback, so the
# row Enter would act on was off screen. Each picker now sheds what its
# choices don't need and keeps the search line, three rows between the ↑/↓
# overflow slots, and the hint. Drive each one on a 12-row pane. Counts are
# matched loosely: a catalog that grows with each new feature must not fail
# this phase instead of that feature's own.
S138="${S}_shortpages"
launch "$S138" 90 12
# /settings: three settings between the slots, the highlighted one on screen,
# no counter and no description.
submit "$S138" "/settings"
short_settings="$(wait_pane 5 "$S138" -F "Enter/Space to change")"
echo "==== Phase 138: /settings on 12 rows ===="
printf '%s\n' "$short_settings"
expect_has "$short_settings" -E '^  ❯ *$' "/settings lost its search line on 12 rows"
expect_has "$short_settings" -E '^→ Hide thinking +' "/settings lost its highlighted row on 12 rows"
expect_has "$short_settings" -E '^  ↓ [0-9]+ more below$' "/settings does not name the rows under its window"
expect_lacks "$short_settings" -E '\([0-9]+/[0-9]+\)' "/settings still shows its counter on 12 rows"
expect_lacks "$short_settings" -F "chain-of-thought" "/settings still shows its description on 12 rows"
# ↓ walks the window down the list: it rides the selection, and both slots
# name what they hide.
tmux send-keys -t "$S138" Down Down Down Down Down Down
sleep 0.4
short_settings_walk="$(pane "$S138")"
echo "==== Phase 138: /settings after six ↓ ===="
printf '%s\n' "$short_settings_walk"
expect_has "$short_settings_walk" -E '^→ Permission mode +' "the /settings window did not follow the selection"
expect_has "$short_settings_walk" -E '^  ↑ [0-9]+ more above$' "/settings does not name the rows over its window"
expect_has "$short_settings_walk" -E '^  ↓ [0-9]+ more below$' "/settings lost the row naming what is under its window"
tmux send-keys -t "$S138" Escape
sleep 0.4
expect_has "$(pane "$S138")" -F "dummy_model_name" "Esc did not close /settings back to the composer"
# /mascot and /spinner: their live previews yield with the counter and the
# description, and three rows stay between the slots.
submit "$S138" "/mascot"
short_mascot="$(wait_pane 5 "$S138" -F "Enter to choose")"
echo "==== Phase 138: /mascot on 12 rows ===="
printf '%s\n' "$short_mascot"
expect_has "$short_mascot" -E '^  ❯ *$' "/mascot lost its search line on 12 rows"
expect_has "$short_mascot" -E '^→ crest' "/mascot lost its highlighted row on 12 rows"
expect_has "$short_mascot" -E '^  ↓ [0-9]+ more below$' "/mascot does not name the rows under its window"
expect_lacks "$short_mascot" -E '\([0-9]+/[0-9]+\)' "/mascot still shows its counter on 12 rows"
expect_lacks "$short_mascot" -F "crested hatchling" "/mascot still shows its description on 12 rows"
tmux send-keys -t "$S138" Escape
sleep 0.4
submit "$S138" "/spinner"
short_spinner="$(wait_pane 5 "$S138" -F "Enter to choose")"
echo "==== Phase 138: /spinner on 12 rows ===="
printf '%s\n' "$short_spinner"
expect_has "$short_spinner" -E '^  ❯ *$' "/spinner lost its search line on 12 rows"
expect_has "$short_spinner" -E '^→ gravity' "/spinner lost its highlighted row on 12 rows"
expect_has "$short_spinner" -E '^  ↓ [0-9]+ more below$' "/spinner does not name the rows under its window"
expect_lacks "$short_spinner" -F "Working" "/spinner still shows its live preview on 12 rows"
tmux send-keys -t "$S138" Escape
sleep 0.4
expect_has "$(pane "$S138")" -F "dummy_model_name" "Esc did not close /spinner back to the composer"
# /login's API-key list: the counter and the where-keys-are-saved note yield.
submit "$S138" "/login"
wait_pane 5 "$S138" -F "Use an API key" >/dev/null
tmux send-keys -t "$S138" Down Enter
short_login="$(wait_pane 5 "$S138" -F "esc back")"
echo "==== Phase 138: /login's provider list on 12 rows ===="
printf '%s\n' "$short_login"
expect_has "$short_login" -E '^  ❯ *$' "/login lost its filter line on 12 rows"
expect_has "$short_login" -E '^→ .+ · (✔|◯) ' "/login lost its highlighted provider on 12 rows"
expect_has "$short_login" -E '^  ↓ [0-9]+ more below$' "/login does not name the providers under its window"
expect_lacks "$short_login" -F "Keys are saved to" "/login still shows its note on 12 rows"
tmux send-keys -t "$S138" Escape Escape
sleep 0.4
tmux kill-session -t "$S138" 2>/dev/null

# /model: a stub provider lists twelve models, and the compact page keeps
# three of them, the search line, and the highlighted model's name line.
M_PORT="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
python3 - "$M_PORT" <<'PY' &
import json, sys
from http.server import BaseHTTPRequestHandler, HTTPServer

port = int(sys.argv[1])
models = [{"id": f"stub/model-{i:02}", "name": f"Stub: Model {i}"} for i in range(12)]

class Stub(BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path.startswith("/v1/models"):
            data = json.dumps({"data": models}).encode()
            self.send_response(200)
            self.send_header("content-type", "application/json")
            self.send_header("content-length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)
        else:
            self.send_response(404)
            self.end_headers()

    def log_message(self, *_args):
        pass

HTTPServer(("127.0.0.1", port), Stub).serve_forever()
PY
M_SERVER=$!
smoke_on_exit kill "$M_SERVER"
for _ in $(seq 1 30); do # wait for the stub to listen
	if python3 -c "import socket, sys; s = socket.socket(); s.settimeout(0.2); sys.exit(0 if s.connect_ex(('127.0.0.1', $M_PORT)) == 0 else 1)" 2>/dev/null; then
		break
	fi
	sleep 0.1
done
M_PROVIDERS="$SMOKE_TMP/providers.toml"
cat >"$M_PROVIDERS" <<TOML
[providers.stub]
name = "Stub"
api_key_env = "STUB_API_KEY"

[providers.stub.kwargs]
api_base = "http://127.0.0.1:$M_PORT/v1"
TOML
launch "$S138" 90 12 "env NO_PROXY=127.0.0.1 no_proxy=127.0.0.1 ALTER_ZERO_PROVIDERS_FILE=$M_PROVIDERS STUB_API_KEY=smoke-138 $APP"
submit "$S138" "/model"
short_model="$(wait_pane 10 "$S138" -F "stub/model-01")"
echo "==== Phase 138: /model on 12 rows ===="
printf '%s\n' "$short_model"
expect_has "$short_model" -E '^  ❯ *$' "/model lost its search line on 12 rows"
expect_has "$short_model" -E '^→ stub/model-00 \[stub\]' "/model lost its highlighted model on 12 rows"
expect_has "$short_model" -E '^  ↓ 9 more below$' "/model does not name the models under its window"
expect_has "$short_model" -F "Stub: Model 0" "/model's compact page lost the highlighted model's name"
expect_lacks "$short_model" -E '\([0-9]+/[0-9]+\)' "/model still shows its counter on 12 rows"
tmux send-keys -t "$S138" Escape
sleep 0.4
tmux kill-session -t "$S138" 2>/dev/null
