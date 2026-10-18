#!/usr/bin/env bash
# Phase 139 — the pickers on a short terminal

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# The composer-replacing pickers on a terminal too short for their whole page
# (docs/view-flow.md *A page that sheds rows before it flows*). Bottom-
# anchoring such a page kept its tail — a description, a preview, the hint —
# and flowed its search line and its highlighted row into scrollback, so the
# row Enter would act on was off screen. Each picker now sheds what its
# choices don't need and keeps the search line, three rows between the ↑/↓
# overflow slots, and the hint — /donate and /secrets their highlighted entry,
# and the AskUserQuestion modal its question (docs/ask.md) — or, like
# /mascot laid beside its preview, fits whole. Drive each one on a 12-row
# pane. Counts are matched loosely where a catalog grows with each
# new feature, which must not fail this phase instead of that feature's own.
S139="${S}_shortpages"
# Eight secrets for /secrets to window (docs/secrets.md).
{
	printf '{\n  "secrets": [\n'
	for i in 0 1 2 3 4 5 6 7; do
		[ "$i" -gt 0 ] && printf ',\n'
		printf '    { "name": "SMOKE_%s", "value": "smoke-value-%s", "context": "Smoke secret %s" }' "$i" "$i" "$i"
	done
	printf '\n  ]\n}\n'
} >"$SMOKE_CFG/secrets.json"
chmod 600 "$SMOKE_CFG/secrets.json"
launch "$S139" 90 12
# /settings: three settings between the slots, the highlighted one on screen,
# no counter and no description.
submit "$S139" "/settings"
short_settings="$(wait_pane 5 "$S139" -F "Enter/Space to change")"
echo "==== Phase 139: /settings on 12 rows ===="
printf '%s\n' "$short_settings"
expect_has "$short_settings" -E '^  ❯ *$' "/settings lost its search line on 12 rows"
expect_has "$short_settings" -E '^→ Hide thinking +' "/settings lost its highlighted row on 12 rows"
expect_has "$short_settings" -E '^  ↓ [0-9]+ more below$' "/settings does not name the rows under its window"
expect_lacks "$short_settings" -E '\([0-9]+/[0-9]+\)' "/settings still shows its counter on 12 rows"
expect_lacks "$short_settings" -F "chain-of-thought" "/settings still shows its description on 12 rows"
# ↓ walks the window down the list: it rides the selection, and both slots
# name what they hide.
tmux send-keys -t "$S139" Down Down Down Down Down Down
sleep 0.4
short_settings_walk="$(pane "$S139")"
echo "==== Phase 139: /settings after six ↓ ===="
printf '%s\n' "$short_settings_walk"
expect_has "$short_settings_walk" -E '^→ Permission mode +' "the /settings window did not follow the selection"
expect_has "$short_settings_walk" -E '^  ↑ [0-9]+ more above$' "/settings does not name the rows over its window"
expect_has "$short_settings_walk" -E '^  ↓ [0-9]+ more below$' "/settings lost the row naming what is under its window"
tmux send-keys -t "$S139" Escape
sleep 0.4
expect_has "$(pane "$S139")" -F "dummy_model_name" "Esc did not close /settings back to the composer"
# /mascot: laid beside the list, its whole page is twelve rows, so a 12-row
# pane shows all of it — every mascot, the caption and the banner preview —
# and nothing is windowed (docs/mascot.md).
submit "$S139" "/mascot"
short_mascot="$(wait_pane 5 "$S139" -F "Enter to choose")"
echo "==== Phase 139: /mascot on 12 rows ===="
printf '%s\n' "$short_mascot"
expect_has "$short_mascot" -E '^  ❯ *$' "/mascot lost its search line on 12 rows"
expect_has "$short_mascot" -E '^  ❯ crest ✓ +Crest *$' "/mascot lost its highlighted row, or the caption beside it, on 12 rows"
expect_has "$short_mascot" -E '^    gem ' "/mascot does not list every mascot on 12 rows"
expect_has "$short_mascot" -F "crested hatchling" "/mascot lost its caption on 12 rows"
expect_has "$short_mascot" -F "▙▄▙▄▟▄▟" "/mascot lost its banner preview on 12 rows"
expect_lacks "$short_mascot" -F " more below" "/mascot windows a list that fits on 12 rows"
tmux send-keys -t "$S139" Escape
sleep 0.4
# /spinner: laid beside the list its whole page is fifteen rows, so on 12 the
# live preview yields, and three styles stay between the slots.
submit "$S139" "/spinner"
short_spinner="$(wait_pane 5 "$S139" -F "Enter to choose")"
echo "==== Phase 139: /spinner on 12 rows ===="
printf '%s\n' "$short_spinner"
expect_has "$short_spinner" -E '^  ❯ *$' "/spinner lost its search line on 12 rows"
expect_has "$short_spinner" -E '^  ❯ gravity' "/spinner lost its highlighted row on 12 rows"
expect_has "$short_spinner" -E '^  ↓ [0-9]+ more below$' "/spinner does not name the rows under its window"
expect_lacks "$short_spinner" -F "Working" "/spinner still shows its live preview on 12 rows"
tmux send-keys -t "$S139" Escape
sleep 0.4
expect_has "$(pane "$S139")" -F "dummy_model_name" "Esc did not close /spinner back to the composer"
# /login's API-key list: the counter and the where-keys-are-saved note yield.
submit "$S139" "/login"
wait_pane 5 "$S139" -F "Use an API key" >/dev/null
tmux send-keys -t "$S139" Down Enter
short_login="$(wait_pane 5 "$S139" -F "esc back")"
echo "==== Phase 139: /login's provider list on 12 rows ===="
printf '%s\n' "$short_login"
expect_has "$short_login" -E '^  ❯ *$' "/login lost its filter line on 12 rows"
expect_has "$short_login" -E '^→ .+ · (✔|◯) ' "/login lost its highlighted provider on 12 rows"
expect_has "$short_login" -E '^  ↓ [0-9]+ more below$' "/login does not name the providers under its window"
expect_lacks "$short_login" -F "Keys are saved to" "/login still shows its note on 12 rows"
# One Esc back to the sign-in fork, one more to close it — sent apart, since
# two ESC bytes in one write read as a single Alt+Esc.
tmux send-keys -t "$S139" Escape
wait_pane 5 "$S139" -F "Use an API key" >/dev/null
tmux send-keys -t "$S139" Escape
wait_pane 5 "$S139" -F "dummy_model_name" >/dev/null ||
	fail "Esc did not close /login back to the composer"
# /donate: the highlighted address alone, its numbered label straight under
# the top rule, over the caution and the hint — no overflow rows, which would
# hug that rule blank at the first address (docs/donate.md).
submit "$S139" "/donate"
short_donate="$(wait_pane 5 "$S139" -F "enter/c copy address")"
echo "==== Phase 139: /donate on 12 rows ===="
printf '%s\n' "$short_donate"
expect_has "$short_donate" -E '^  ❯ 1\. BTC' "/donate lost its highlighted label on 12 rows"
expect_has "$short_donate" -F "Network: Bitcoin" "/donate lost the highlighted address's networks on 12 rows"
expect_lacks "$short_donate" -F " more below" "/donate still counts the other addresses on 12 rows"
expect_has "$short_donate" -F "cannot be recovered" "/donate lost its caution on 12 rows"
expect_lacks "$short_donate" -E '^ +2\. ETH' "/donate still lists the other addresses on 12 rows"
expect_lacks "$short_donate" -F "Support" "/donate still shows its title on 12 rows"
tmux send-keys -t "$S139" Down
sleep 0.4
short_donate_eth="$(pane "$S139")"
echo "==== Phase 139: /donate after ↓ ===="
printf '%s\n' "$short_donate_eth"
expect_has "$short_donate_eth" -E '^  ❯ 2\. ETH' "↓ did not move /donate to the next address"
expect_has "$short_donate_eth" -F "Networks: Ethereum" "/donate lost the second address's networks"
tmux send-keys -t "$S139" Escape
sleep 0.4
expect_has "$(pane "$S139")" -F "dummy_model_name" "Esc did not close /donate back to the composer"
# /secrets: eight secrets on 12 rows — three between the slots under the
# title and the blurb, the masks still lined up (docs/secrets.md).
submit "$S139" "/secrets"
short_secrets="$(wait_pane 5 "$S139" -F "c copy placeholder")"
echo "==== Phase 139: /secrets on 12 rows ===="
printf '%s\n' "$short_secrets"
expect_has "$short_secrets" -E '^  ❯ <secret:SMOKE_0> ' "/secrets lost its highlighted secret on 12 rows"
expect_has "$short_secrets" -F "<secret:SMOKE_2>  ••••••••  Smoke secret 2" "/secrets lost its mask column on 12 rows"
expect_has "$short_secrets" -E '^  ↓ 6 more below$' "/secrets does not count the secrets under its window"
expect_has "$short_secrets" -E '^  Secrets$' "/secrets lost its title on 12 rows"
expect_lacks "$short_secrets" -F "<secret:SMOKE_3>" "/secrets shows more than its window on 12 rows"
tmux send-keys -t "$S139" Down Down Down Down Down
sleep 0.4
short_secrets_walk="$(pane "$S139")"
echo "==== Phase 139: /secrets after five ↓ ===="
printf '%s\n' "$short_secrets_walk"
expect_has "$short_secrets_walk" -E '^  ❯ <secret:SMOKE_5>' "the /secrets window did not follow the selection"
expect_has "$short_secrets_walk" -E '^  ↑ 4 more above$' "/secrets does not count the secrets over its window"
expect_has "$short_secrets_walk" -E '^  ↓ 2 more below$' "/secrets lost the row counting the secrets under its window"
tmux send-keys -t "$S139" Escape
sleep 0.4
expect_has "$(pane "$S139")" -F "dummy_model_name" "Esc did not close /secrets back to the composer"
# The AskUserQuestion modal (docs/ask.md): the coffee question sheds its
# descriptions and two of its five rows, and keeps its chip strip — the last
# thing to go — the question, the highlighted option and the hint.
submit "$S139" "ask me some questions"
short_ask="$(wait_pane 10 "$S139" -F "What's your favorite way to drink coffee?")" ||
	fail "the dummy's ask modal never opened"
echo "==== Phase 139: the ask modal on 12 rows ===="
printf '%s\n' "$short_ask"
expect_has "$short_ask" -E '^ ❯ 1\. Black' "the ask modal lost its highlighted option on 12 rows"
expect_has "$short_ask" -E '^ ↓ 2 more below$' "the ask modal does not count the rows under its window"
expect_has "$short_ask" -F "Esc to cancel" "the ask modal lost its hint on 12 rows"
expect_has "$short_ask" -F "Coffee style" "the ask modal lost its chip strip on 12 rows"
expect_lacks "$short_ask" -F "No milk" "the ask modal still shows its descriptions on 12 rows"
# The preview question: the notes line yields and the panel is cut to the
# options' height, so the question and every option stay on screen.
tmux send-keys -t "$S139" Tab
sleep 0.3
tmux send-keys -t "$S139" Tab
short_preview="$(wait_pane 5 "$S139" -F "Which code style do you prefer")" ||
	fail "Tab did not reach the preview question"
echo "==== Phase 139: the ask modal's preview page on 12 rows ===="
printf '%s\n' "$short_preview"
expect_has "$short_preview" -E '^ ❯ 1\. Arrow function' "the preview page lost its highlighted option on 12 rows"
expect_has "$short_preview" -F "3. One-liner" "the preview page lost an option on 12 rows"
expect_has "$short_preview" -F "const greet = (name) => {" "the preview page lost its panel on 12 rows"
expect_has "$short_preview" -F "n to add notes" "the preview page's hint no longer offers notes"
expect_lacks "$short_preview" -F "Notes: press n" "the preview page still shows its notes line on 12 rows"
tmux send-keys -t "$S139" Escape
wait_pane 10 "$S139" -S -60 -- -F "User declined to answer questions" >/dev/null ||
	fail "Esc did not decline the questions"
tmux kill-session -t "$S139" 2>/dev/null

# The search-line pickers on 7 rows (docs/view-flow.md *Below the compact
# page*). Bottom-anchoring the compact page flowed its search line — and the
# caret, clamped onto a list row — into scrollback, and then the highlighted
# row. The page sheds its ↑/↓ rows, its rules and list rows instead, so the
# search line stays on screen with the caret on it, the highlighted row under
# it, the hint under that; on seven rows both rules still fit, since the
# frame pads neither: the search line straight under the top one.
for skill in alpha beta; do
	mkdir -p "$SMOKE_SKILLS/$skill"
	printf -- '---\nname: %s\ndescription: The %s skill.\n---\nBody.\n' "$skill" "$skill" >"$SMOKE_SKILLS/$skill/SKILL.md"
done
S139T="${S}_tiny"
launch "$S139T" 90 7
caret_on_search_line() { # $1 label, $2 the pane, $3 the query
	local row cur
	row="$(printf '%s\n' "$2" | grep -n -x -F "  ❯${3:+ $3}" | head -1 | cut -d: -f1)"
	if [ -z "$row" ]; then
		fail "$1: the search line is not on screen"
		return
	fi
	cur="$(tmux display-message -p -t "$S139T" '#{cursor_x},#{cursor_y}')"
	expect_eq "$cur" "$((4 + ${#3})),$((row - 1))" "$1: the caret is not on the search line"
}
close_picker() { # $1 label
	tmux send-keys -t "$S139T" Escape
	wait_pane 5 "$S139T" -F "dummy_model_name" >/dev/null ||
		fail "Esc did not close $1 back to the composer"
}
submit "$S139T" "/theme"
wait_pane 5 "$S139T" -F "Enter to choose" >/dev/null
tmux send-keys -t "$S139T" Down Down Down Down Down
tiny_theme="$(wait_pane 5 "$S139T" -E '^  ❯ dracula')"
echo "==== Phase 139: /theme on 7 rows ===="
printf '%s\n' "$tiny_theme"
expect_has "$(printf '%s\n' "$tiny_theme" | head -1)" -F "──────────" "/theme on 7 rows lost its top rule"
expect_eq "$(printf '%s\n' "$tiny_theme" | sed -n 2p)" "  ❯" "/theme on 7 rows does not put its search line under the rule"
expect_has "$tiny_theme" -E '^    onedark ' "/theme on 7 rows lost the theme above the highlighted one"
expect_has "$tiny_theme" -E '^    nord ' "/theme on 7 rows lost the theme below the highlighted one"
expect_has "$tiny_theme" -F "↑/↓ to navigate" "/theme on 7 rows lost its hint"
caret_on_search_line "/theme on 7 rows" "$tiny_theme" ""
# What was typed stays over a search that matches nothing.
tmux send-keys -t "$S139T" -l "zzz"
tiny_nomatch="$(wait_pane 5 "$S139T" -F "No matching themes")"
echo "==== Phase 139: /theme matching nothing on 7 rows ===="
printf '%s\n' "$tiny_nomatch"
caret_on_search_line "/theme matching nothing on 7 rows" "$tiny_nomatch" "zzz"
tmux send-keys -t "$S139T" Escape # clears the search
sleep 0.3
close_picker /theme
for picker in "/settings:→ Hide thinking:Enter/Space to change" "/mascot:❯ crest:Enter to choose" \
	"/spinner:❯ gravity:Enter to choose" "/skills:→ alpha:Enter/Space to enable"; do
	cmd="${picker%%:*}"
	rest="${picker#*:}"
	row="${rest%%:*}"
	hint="${rest#*:}"
	submit "$S139T" "$cmd"
	tiny="$(wait_pane 5 "$S139T" -F "$hint")"
	echo "==== Phase 139: $cmd on 7 rows ===="
	printf '%s\n' "$tiny"
	expect_has "$tiny" -F "$row" "$cmd on 7 rows lost its highlighted row"
	caret_on_search_line "$cmd on 7 rows" "$tiny" ""
	close_picker "$cmd"
done
submit "$S139T" "/login"
tiny_login="$(wait_pane 5 "$S139T" -F "Use an API key")"
echo "==== Phase 139: /login on 7 rows ===="
printf '%s\n' "$tiny_login"
expect_has "$tiny_login" -E '^→ Use a subscription' "/login on 7 rows lost its highlighted row"
caret_on_search_line "/login on 7 rows" "$tiny_login" ""
tmux send-keys -t "$S139T" Down Enter
tiny_providers="$(wait_pane 5 "$S139T" -F "esc back")"
echo "==== Phase 139: /login's provider list on 7 rows ===="
printf '%s\n' "$tiny_providers"
expect_eq "$(printf '%s\n' "$tiny_providers" | sed -n 2p)" "  ❯" "/login's provider list on 7 rows does not put its filter line under the rule"
expect_has "$tiny_providers" -E '^→ .+ · (✔|◯) ' "/login's provider list on 7 rows lost its highlighted provider"
caret_on_search_line "/login's provider list on 7 rows" "$tiny_providers" ""
tmux send-keys -t "$S139T" Escape
wait_pane 5 "$S139T" -F "Use an API key" >/dev/null
close_picker /login
tmux kill-session -t "$S139T" 2>/dev/null

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
launch "$S139" 90 12 "env NO_PROXY=127.0.0.1 no_proxy=127.0.0.1 ALTER_ZERO_PROVIDERS_FILE=$M_PROVIDERS STUB_API_KEY=smoke-138 $APP"
submit "$S139" "/model"
short_model="$(wait_pane 10 "$S139" -F "stub/model-01")"
echo "==== Phase 139: /model on 12 rows ===="
printf '%s\n' "$short_model"
expect_has "$short_model" -E '^  ❯ *$' "/model lost its search line on 12 rows"
expect_has "$short_model" -E '^→ stub/model-00 \[stub\]' "/model lost its highlighted model on 12 rows"
expect_has "$short_model" -E '^  ↓ 9 more below$' "/model does not name the models under its window"
expect_has "$short_model" -F "Stub: Model 0" "/model's compact page lost the highlighted model's name"
expect_lacks "$short_model" -E '\([0-9]+/[0-9]+\)' "/model still shows its counter on 12 rows"
tmux send-keys -t "$S139" Escape
sleep 0.4
tmux kill-session -t "$S139" 2>/dev/null
# …and on 7 rows the search line sits under the top rule, the name line in
# the hint's place.
launch "$S139" 90 7 "env NO_PROXY=127.0.0.1 no_proxy=127.0.0.1 ALTER_ZERO_PROVIDERS_FILE=$M_PROVIDERS STUB_API_KEY=smoke-138 $APP"
submit "$S139" "/model"
tiny_model="$(wait_pane 10 "$S139" -F "stub/model-01")"
echo "==== Phase 139: /model on 7 rows ===="
printf '%s\n' "$tiny_model"
expect_eq "$(printf '%s\n' "$tiny_model" | sed -n 2p)" "  ❯" "/model on 7 rows does not put its search line under the rule"
expect_has "$tiny_model" -E '^→ stub/model-00 \[stub\]' "/model on 7 rows lost its highlighted model"
expect_has "$tiny_model" -F "Stub: Model 0" "/model on 7 rows lost the highlighted model's name"
expect_eq "$(tmux display-message -p -t "$S139" '#{cursor_x},#{cursor_y}')" "4,1" "/model on 7 rows: the caret is not on the search line"
tmux send-keys -t "$S139" Escape
sleep 0.4
tmux kill-session -t "$S139" 2>/dev/null
