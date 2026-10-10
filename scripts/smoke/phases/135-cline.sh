#!/usr/bin/env bash
# Phase 135 — the Cline provider reaches `/login` and its key step links its own page

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# Cline as a pasted-key provider (providers.toml). The row must reach the
# API-key list — not the subscription half — and its key step must introduce
# the provider and link the page its keys are made on (`api_key_url`), like
# every other keyed provider.
S135="${S}_cline"
launch "$S135" 90 30
submit "$S135" "/login"
sleep 0.5
tmux send-keys -t "$S135" Down
sleep 0.3
tmux send-keys -t "$S135" Enter
sleep 0.4
login_keys="$(tmux capture-pane -t "$S135" -p)"
echo "==== Phase 135: the API-key list carries Cline ===="
printf '%s\n' "$login_keys"
expect_has "$login_keys" -F "Cline" "the API-key list did not show \"Cline\""
expect_lacks "$login_keys" -F "Cline extension" "a provider row still trails its description"

# Type-to-search narrows onto the row — by name and by what the description
# says it does (providers.toml) — and Enter opens the key step.
tmux send-keys -t "$S135" "cline"
sleep 0.4
login_filtered="$(tmux capture-pane -t "$S135" -p)"
echo "==== Phase 135: the filter narrows onto Cline ===="
printf '%s\n' "$login_filtered"
expect_has "$login_filtered" -F "Cline" "the filter did not keep the Cline row"
tmux send-keys -t "$S135" Enter
sleep 0.4
login_key_step="$(tmux capture-pane -t "$S135" -p)"
echo "==== Phase 135: the key step introduces Cline ===="
printf '%s\n' "$login_key_step"
for want in "Enter your Cline API key" "Cline extension" "Create a key at" "app.cline.bot"; do
	expect_has "$login_key_step" -F "$want" "the key step did not show \"$want\""
done

# Esc steps back to the provider list, as for every keyed provider.
tmux send-keys -t "$S135" Escape
sleep 0.4
if ! tmux capture-pane -t "$S135" -p | grep -qF "Keys are saved to"; then
	fail "Esc on the key step did not return to the provider list"
fi

tmux kill-session -t "$S135" 2>/dev/null
echo "==== Phase 135: Cline is a first-class keyed provider ===="
