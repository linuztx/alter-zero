#!/usr/bin/env bash
# Phase 167 — the `/discord` page

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# the `/discord` page (docs/discord.md). The `/sponsor` page's twin over the
# one const invite: a gradient `♥ Join the Alter Zero Discord` title over a
# dim blurb, the numbered `❯ 1. Discord` label over the invite in a rounded
# box, and a key hint with no arrows (there is no other row). Enter copies the
# invite — the toast says so, and tmux, handed it by the app inside its pane
# (`tmux load-buffer`), keeps it VERBATIM in its paste buffer (Phase 169,
# docs/copy.md) while the page stays open; `c` and `1` copy it too, ↓ moves
# nothing, and Esc brings the composer and its footer back.
S167="${S}_discord"
DISCORD_URL="https://discord.gg/EDBSpmq2fV"
# Whether tmux's paste buffer holds exactly the invite.
buffer_holds_invite() { [ "$(tmux show-buffer 2>/dev/null)" = "$DISCORD_URL" ]; }
tmux new-session -d -s "$S167" -x 90 -y 40 "$APP"
# For a tmux older than 3.2, which has no `load-buffer -w`: the copy falls
# back to the escape there, and only `on` keeps an application's escape.
tmux set-option -g set-clipboard on
sleep 0.5
tmux send-keys -t "$S167" -l "/discord"
discord_palette="$(wait_pane 5 "$S167" -F "Join the community on Discord")"
echo "==== Phase 167: the palette filtered to /discord ===="
printf '%s\n' "$discord_palette"
expect_has "$discord_palette" -F "Join the community on Discord" "/discord is missing from the slash-command palette"
tmux send-keys -t "$S167" Enter
discord_open="$(wait_pane 5 "$S167" -F "enter/c copy invite link  esc close")"
echo "==== Phase 167: the page open ===="
printf '%s\n' "$discord_open"
# The blurb is wrapped to the width, so each check is a phrase that stays on
# one row however the 90-column pane breaks it.
for expect in "♥ Join the Alter Zero Discord" "Ask for help, share what you have built" \
	"new features are welcome too." \
	"❯ 1. Discord  invite to the community server" "│  $DISCORD_URL  │" \
	"enter/c copy invite link  esc close"; do
	expect_has "$discord_open" -F "$expect" "the open page is missing '$expect'"
done
# One row: the hint offers no arrows to navigate with.
expect_lacks "$discord_open" -F "↑↓ navigate" "the hint offers navigation on a one-row page"
# The page replaces the composer: its footer is off screen while it is up.
expect_lacks "$discord_open" -F "dummy_model_name ·" "the footer is still on screen under the page"
# Enter copies the invite, and the page stays open.
while tmux delete-buffer 2>/dev/null; do :; done
tmux send-keys -t "$S167" Enter
discord_copied="$(wait_pane 3 "$S167" -F "Copied the Discord invite link to clipboard")"
echo "==== Phase 167: after Enter — the toast ===="
printf '%s\n' "$discord_copied"
expect_has "$discord_copied" -F "Copied the Discord invite link to clipboard" "Enter did not confirm the copy with a toast"
expect_has "$discord_copied" -F "❯ 1. Discord" "copying closed the page"
poll 3 buffer_holds_invite ||
	fail "Enter did not put the invite in the clipboard verbatim: '$(tmux show-buffer 2>/dev/null)'"
# ↓ has no other row to reach; `c` then copies the same invite.
while tmux delete-buffer 2>/dev/null; do :; done
tmux send-keys -t "$S167" Down
tmux send-keys -t "$S167" -l "c"
poll 3 buffer_holds_invite ||
	fail "c did not put the invite in the clipboard verbatim: '$(tmux show-buffer 2>/dev/null)'"
expect_has "$(pane "$S167")" -F "❯ 1. Discord" "↓ or c moved the ❯ off the one row"
# The row's digit copies it too.
while tmux delete-buffer 2>/dev/null; do :; done
tmux send-keys -t "$S167" -l "1"
poll 3 buffer_holds_invite ||
	fail "1 did not put the invite in the clipboard verbatim: '$(tmux show-buffer 2>/dev/null)'"
# Esc closes: the composer and its footer come back, the page is gone.
tmux send-keys -t "$S167" Escape
discord_closed="$(wait_pane 5 "$S167" -F "dummy_model_name ·")"
echo "==== Phase 167: after Esc ===="
printf '%s\n' "$discord_closed"
expect_has "$discord_closed" -F "dummy_model_name ·" "the composer and footer did not come back after Esc"
expect_lacks "$discord_closed" -F "discord.gg" "the page is still on screen after Esc"
tmux kill-session -t "$S167" 2>/dev/null
