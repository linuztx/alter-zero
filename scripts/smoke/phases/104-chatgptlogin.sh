#!/usr/bin/env bash
# Phase 104 — the BROWSER sign-in page

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# the BROWSER sign-in page (docs/chatgpt.md). The second
# subscription's sign-in is not a device code — it is a link the user opens,
# and the browser redirects back to a loopback listener. That page is drivable
# offline (building the URL and binding 127.0.0.1:1455 touch no network at
# all), so unlike Copilot's device page it can be walked here. What this
# phase pins is that the page is worded for a LINK rather than for a code: the
# `Open …` verb, the authorize URL itself, the row saying the window continues
# by itself, and a hint offering `c copy link`. A page that silently fell back
# to the device wording would tell the user to type a one-time code that does
# not exist.
S104="${S}_chatgptlogin"
launch "$S104" 100 30
submit "$S104" "/login"
sleep 0.5
# Enter opens the subscription list, then the row is picked by NAME rather
# than by counting Downs: the list is alphabetical by provider id, so every
# subscription added ahead of `openai_chatgpt` used to shift this phase onto
# the wrong sign-in page — and the failure reads as "the browser page fell
# back to the device wording", which blames the code under test rather than
# the walk. `chatgpt` matches this row's id, name and description, and no
# other row's anything.
tmux send-keys -t "$S104" Enter
sleep 0.4
tmux send-keys -t "$S104" -l "chatgpt"
sleep 0.3
tmux send-keys -t "$S104" Enter
sleep 1.2
chatgpt_page="$(tmux capture-pane -t "$S104" -p)"
echo "==== Phase 104: the ChatGPT browser sign-in page ===="
printf '%s\n' "$chatgpt_page"
for want in "Sign in to OpenAI (ChatGPT)" "this window continues by itself" \
	"c copy link" "Waiting for the browser"; do
	expect_has "$chatgpt_page" -F "$want" "the browser sign-in page did not show \"$want\""
done
# The URL is one unbreakable word, so it wraps across rows at any width — the
# parameter assertions read the pane with the wrapping squeezed out (a URL
# contains no spaces, so nothing real is lost). These are the parameters
# OpenAI refuses the flow without, plus the allow-listed redirect and the
# `offline_access` scope that is what earns a refresh token at all.
chatgpt_url="$(printf '%s' "$chatgpt_page" | tr -d ' \n')"
for want in "https://auth.openai.com/oauth/authorize?response_type=code" \
	"client_id=app_EMoamEEZ73f0CkXaXp7hrann" \
	"redirect_uri=http%3A%2F%2Flocalhost%3A1455%2Fauth%2Fcallback" \
	"offline_access" "code_challenge_method=S256" "originator=codex_cli_rs"; do
	expect_has "$chatgpt_url" -F "$want" "the authorize URL is missing \"$want\""
done
# The URL opens its own row: it is a clickable OSC 8 hyperlink
# (docs/links.md), and a verb in front of one only pushes the target off the
# start of the row it should begin.
expect_has "$chatgpt_page" -E '^ *https://auth\.openai\.com/oauth/authorize' "the authorize URL does not start its own row"
# And it must NOT wear the device page's clothes, or a verb the link replaced.
for unwanted in "enter this one-time code" "c copy code" "Open https://"; do
	expect_lacks "$chatgpt_page" -F "$unwanted" "the browser page fell back to the device-code wording (\"$unwanted\")"
done
# Esc cancels the sign-in back to the subscription list, releasing the port.
tmux send-keys -t "$S104" Escape
sleep 0.5
chatgpt_back="$(tmux capture-pane -t "$S104" -p)"
if ! printf '%s' "$chatgpt_back" | grep -qF "OpenAI (ChatGPT)"; then
	fail "Esc on the sign-in page did not return to the subscription list"
	printf '%s\n' "$chatgpt_back" >&2
fi
expect_lacks "$chatgpt_back" -F "Waiting for the browser" "Esc left the sign-in page up"
tmux kill-session -t "$S104" 2>/dev/null
echo "==== Phase 104: the browser sign-in page is worded for a link, not a code ===="
