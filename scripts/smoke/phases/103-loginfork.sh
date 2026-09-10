#!/usr/bin/env bash
# Phase 103 — the `/login` SIGN-IN FORK

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# the `/login` SIGN-IN FORK (docs/copilot.md). `/login` no
# longer opens on the provider list — it asks how you sign in first, because
# GitHub Copilot is a subscription rather than a key you paste. Walk the whole
# tree in one session: the method root, down into the subscription list and
# back, down into the API-key list and back, and out. The two lists must be
# disjoint — a subscription offered a key field, or Copilot missing from the
# subscriptions, is the bug this phase exists for — and Esc must step *back*
# from either list rather than closing the flow, with only the root's Esc
# closing it. The device page itself is not driven here (it would talk to
# github.com); Phase 33 already covers `/login` opening mid-turn.
S103="${S}_loginfork"
launch "$S103" 90 30
submit "$S103" "/login"
sleep 0.5
login_root="$(tmux capture-pane -t "$S103" -p)"
echo "==== Phase 103: /login opens on the method root ===="
printf '%s\n' "$login_root"
for want in "Use a subscription" "Use an API key" "escape/ctrl+c cancel"; do
	expect_has "$login_root" -F "$want" "the /login root did not show \"$want\""
done
expect_lacks "$login_root" -F "Keys are saved to" "the root opened straight onto the provider list"

# Enter on the highlighted first row opens the subscription list.
tmux send-keys -t "$S103" Enter
sleep 0.4
login_subs="$(tmux capture-pane -t "$S103" -p)"
echo "==== Phase 103: the subscription list ===="
printf '%s\n' "$login_subs"
# The rows are names and their ✓, not sentences: the one-line descriptions
# used to trail each name and were dropped so the three lists read as one
# shape. What the list must still show is every subscription, by name.
for want in "Anthropic Console" "GitHub Copilot" "OpenAI (ChatGPT)" "enter sign in"; do
	expect_has "$login_subs" -F "$want" "the subscription list did not show \"$want\""
done
expect_lacks "$login_subs" -F "Sign in with your" "a row still trails its description"
# Every row says whether it is reachable. The suite scrubs the key store, so
# each one must read as unconfigured — and say so, rather than leaving the
# reader to notice a missing mark.
expect_has "$login_subs" -F "◯ unconfigured" "a subscription row does not report its configured status"
# …and the list opens straight onto its filter: the heading that used to
# repeat the row that opened it is gone.
expect_lacks "$login_subs" -F "Use a subscription" "the subscription list still carries a heading"

# Esc steps BACK to the root, not out of the flow.
tmux send-keys -t "$S103" Escape
sleep 0.4
login_back="$(tmux capture-pane -t "$S103" -p)"
if ! printf '%s' "$login_back" | grep -qF "Use a subscription"; then
	fail "Esc on the subscription list left the flow instead of stepping back"
	printf '%s\n' "$login_back" >&2
fi

# Down + Enter takes the other branch: the API-key providers, and ONLY those.
tmux send-keys -t "$S103" Down
sleep 0.2
tmux send-keys -t "$S103" Enter
sleep 0.4
login_keys="$(tmux capture-pane -t "$S103" -p)"
echo "==== Phase 103: the API-key provider list ===="
printf '%s\n' "$login_keys"
for want in "Agent Zero API" "OpenRouter" "Keys are saved to" "esc back"; do
	expect_has "$login_keys" -F "$want" "the API-key list did not show \"$want\""
done
expect_lacks "$login_keys" -F "GitHub Copilot" "a subscription provider was offered a key field"
# Nor does a row trail its env var. The step's own hint names the file every
# key lands in, and the save toast names the variable — on the row it only
# pushed the names apart.
expect_lacks "$login_keys" -F "_API_KEY]" "a provider row still trails its env var"
expect_has "$login_keys" -F "◯ unconfigured" "a provider row does not report its configured status"
expect_lacks "$login_keys" -F "Use an API key" "the provider list still carries a heading"

# Enter on the highlighted provider opens the key step — which introduces the
# provider it is asking a secret for and links the page that secret is made
# on, read from `providers.toml` (docs/llm.md). A page that asks for a key and
# says nothing about where to get one is the thing this replaces.
tmux send-keys -t "$S103" Enter
sleep 0.4
login_key_step="$(tmux capture-pane -t "$S103" -p)"
echo "==== Phase 103: the key step introduces its provider ===="
printf '%s\n' "$login_key_step"
for want in "Enter your Agent Zero API key" "Venice.ai" "Create a key at" "agent-zero.ai"; do
	expect_has "$login_key_step" -F "$want" "the key step did not show \"$want\""
done
# The block wraps rather than clipping: the description's own last word has to
# survive, or the sentence stops mid-thought.
expect_has "$login_key_step" -F "holders." "the key step clipped its description"
# Esc steps back to the provider list, as it always did.
tmux send-keys -t "$S103" Escape
sleep 0.3
if ! tmux capture-pane -t "$S103" -p | grep -qF "Keys are saved to"; then
	fail "Esc on the key step did not return to the provider list"
fi

# Esc back to the root, then Esc again closes the flow and the composer returns.
tmux send-keys -t "$S103" Escape
sleep 0.3
tmux send-keys -t "$S103" Escape
sleep 0.4
login_closed="$(tmux capture-pane -t "$S103" -p)"
if printf '%s' "$login_closed" | grep -qF "Use a subscription"; then
	fail "Esc on the root did not close the flow"
	printf '%s\n' "$login_closed" >&2
fi
tmux kill-session -t "$S103" 2>/dev/null
echo "==== Phase 103: the /login sign-in fork walks both halves and closes at its root ===="
