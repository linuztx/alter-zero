#!/usr/bin/env bash
# Phase 155 — the `/sponsor` page

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# the `/sponsor` page (docs/sponsor.md). The `/donate` page's twin over the
# developer's sponsor page and contact address: a gradient `♥ Sponsor Alter
# Zero` title over a dim blurb, each row a numbered `❯ 1. GitHub Sponsors`
# label over its value in a rounded box, and a key hint; ↓ moves the `❯`
# and wraps; Enter copies the highlighted value — the toast says which, and
# headless here the OSC 52 fallback lands it VERBATIM in tmux's paste buffer
# (Phase 28's trick) while the page stays open; Esc brings the composer and
# its footer back.
S155="${S}_sponsor"
tmux new-session -d -s "$S155" -x 90 -y 40 "$APP"
# tmux must capture OSC 52 from the app into its own buffer (Phase 28).
tmux set-option -g set-clipboard on
sleep 0.5
tmux send-keys -t "$S155" -l "/sponsor"
sponsor_palette="$(wait_pane 5 "$S155" -F "Sponsor the developer or get in touch")"
echo "==== Phase 155: the palette filtered to /sponsor ===="
printf '%s\n' "$sponsor_palette"
expect_has "$sponsor_palette" -F "Sponsor the developer or get in touch" "/sponsor is missing from the slash-command palette"
tmux send-keys -t "$S155" Enter
sponsor_open="$(wait_pane 5 "$S155" -F "enter/c copy  esc close")"
echo "==== Phase 155: the page open (GitHub Sponsors highlighted) ===="
printf '%s\n' "$sponsor_open"
for expect in "♥ Sponsor Alter Zero" "built and maintained by one developer" \
	"❯ 1. GitHub Sponsors  monthly or one-time" "│  https://github.com/sponsors/linuztx  │" \
	"  2. Email  company sponsorships and anything else" "│  linuztx@gmail.com  │" \
	"↑↓ navigate  enter/c copy  esc close"; do
	expect_has "$sponsor_open" -F "$expect" "the open page is missing '$expect'"
done
# The page replaces the composer: its footer is off screen while it is up.
expect_lacks "$sponsor_open" -F "dummy_model_name ·" "the footer is still on screen under the page"
# Enter copies the sponsor page's URL.
while tmux delete-buffer 2>/dev/null; do :; done
tmux send-keys -t "$S155" Enter
sponsor_copied="$(wait_pane 3 "$S155" -F "Copied the sponsor link to clipboard")"
echo "==== Phase 155: after Enter — the toast ===="
printf '%s\n' "$sponsor_copied"
expect_has "$sponsor_copied" -F "Copied the sponsor link to clipboard" "Enter did not confirm the copy with a toast"
expect_has "$sponsor_copied" -F "❯ 1. GitHub Sponsors" "copying closed the page"
sponsor_clip="$(tmux show-buffer 2>/dev/null)"
if [ "$sponsor_clip" != "https://github.com/sponsors/linuztx" ]; then
	fail "the clipboard does not hold the sponsor link verbatim: '$sponsor_clip'"
fi
# ↓ moves the ❯ to the email row; `c` copies the address.
while tmux delete-buffer 2>/dev/null; do :; done
tmux send-keys -t "$S155" Down
tmux send-keys -t "$S155" -l "c"
sponsor_email="$(wait_pane 3 "$S155" -F "Copied the email address to clipboard")"
echo "==== Phase 155: ↓ then c — the email row ===="
printf '%s\n' "$sponsor_email"
expect_has "$sponsor_email" -F "Copied the email address to clipboard" "c did not confirm the email copy"
expect_has "$sponsor_email" -F "❯ 2. Email" "↓ did not move the ❯ to the email row"
expect_lacks "$sponsor_email" -F "❯ 1. GitHub" "the first row still wears the ❯ after ↓"
sponsor_clip="$(tmux show-buffer 2>/dev/null)"
if [ "$sponsor_clip" != "linuztx@gmail.com" ]; then
	fail "the clipboard does not hold the email address verbatim: '$sponsor_clip'"
fi
# ↓ past the last row wraps to the first.
tmux send-keys -t "$S155" Down
sleep 0.4
expect_has "$(pane "$S155")" -F "❯ 1. GitHub Sponsors" "↓ past the last row did not wrap"
# Esc closes: the composer and its footer come back, the page is gone.
tmux send-keys -t "$S155" Escape
sponsor_closed="$(wait_pane 5 "$S155" -F "dummy_model_name ·")"
echo "==== Phase 155: after Esc ===="
printf '%s\n' "$sponsor_closed"
expect_has "$sponsor_closed" -F "dummy_model_name ·" "the composer and footer did not come back after Esc"
expect_lacks "$sponsor_closed" -F "github.com/sponsors" "the page is still on screen after Esc"
tmux kill-session -t "$S155" 2>/dev/null
