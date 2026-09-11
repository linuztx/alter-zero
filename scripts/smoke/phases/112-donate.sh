#!/usr/bin/env bash
# Phase 112 — the `/donate` page

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# the `/donate` page (docs/donate.md). The `/hooks` browser's
# sibling over the project's crypto donation addresses: a gradient
# `♥ Support Alter Zero` title over a dim blurb, each address as a numbered
# `❯ 1. BTC  Bitcoin` row (the ticker and the coin — no network clause)
# over its rounded box, with the networks it is reachable on captioned
# UNDER that box (`Network: Solana`, and the seven EVM chains on the one
# ETH address — the label agreeing with the count), an amber
# wrong-network caution pointing at those captions and a key hint; ↓
# moves the `❯`, on to the third (SOL) row and back; `c` copies the
# highlighted address — the toast names the coin, and headless here (the
# suite's own unset of the display variables) arboard has no clipboard server
# so the OSC 52 fallback lands the address VERBATIM in tmux's paste buffer
# (Phase 28's trick) while the page stays open; Esc
# brings the composer and its footer back.
S112="${S}_donate"
tmux new-session -d -s "$S112" -x 90 -y 40 "$APP"
# tmux must capture OSC 52 from the app into its own buffer (Phase 28).
tmux set-option -g set-clipboard on
sleep 0.5
tmux send-keys -t "$S112" -l "/donate"
sleep 0.4
donate_palette="$(tmux capture-pane -t "$S112" -p)"
echo "==== Phase 112: the palette filtered to /donate ===="
printf '%s\n' "$donate_palette"
expect_has "$donate_palette" -F "Support the project with a crypto donation" "/donate is missing from the slash-command palette"
tmux send-keys -t "$S112" Enter
sleep 0.5
donate_open="$(tmux capture-pane -t "$S112" -p)"
echo "==== Phase 112: the page open (BTC highlighted) ===="
printf '%s\n' "$donate_open"
for expect in "♥ Support Alter Zero" "Free and open source" \
	"❯ 1. BTC  Bitcoin" "bc1qhwamfrwuhz64pk00l75ykfff2ang22ns64chf7" "Network: Bitcoin (Native SegWit)" \
	"  2. ETH  Ethereum" "0xEAf6fbabB9DBE7a23BfE22A7A6c4aCe02063524b" \
	"Networks: Ethereum, Linea, Base, Arbitrum, BNB Chain, OP, Polygon" \
	"  3. SOL  Solana" "Gwhv5c6uAa6aAz1MjwzV9QJpbm7CJWy2kuCeZ75mFc94" "Network: Solana" \
	"↑↓ navigate  enter/c copy address  esc close"; do
	expect_has "$donate_open" -F "$expect" "the open page is missing '$expect'"
done
# The caution is prose wrapped to the pane's width, so a phrase of it can
# straddle a row break (at these 90 columns it breaks after `cannot be`):
# read it whole off the rows joined back into one line, and read the whole
# sentence — the one line on the page that must say exactly what it says.
donate_open_prose="$(printf '%s\n' "$donate_open" | sed 's/^[[:space:]]*//; s/[[:space:]]*$//' | paste -sd' ')"
if ! printf '%s' "$donate_open_prose" | grep -qF \
	"Send each coin only over a network listed under its address — a transfer on any other network cannot be recovered."; then
	fail "the open page is missing the wrong-network caution, whole"
fi
# A row is the ticker and the coin's name and nothing more: no network
# clause after the coin (read as the whole trimmed row, not a substring).
for label in "❯ 1. BTC  Bitcoin" "2. ETH  Ethereum" "3. SOL  Solana"; do
	if ! printf '%s\n' "$donate_open" | sed 's/^[[:space:]]*//; s/[[:space:]]*$//' | grep -qxF "$label"; then
		fail "the row '$label' carries something after the coin"
	fi
done
# Each address sits in its rounded box: the row over it opens with ╭ and
# the row under it with ╰.
for addr in "bc1qhwamfrwuhz64pk00l75ykfff2ang22ns64chf7" "0xEAf6fbabB9DBE7a23BfE22A7A6c4aCe02063524b" \
	"Gwhv5c6uAa6aAz1MjwzV9QJpbm7CJWy2kuCeZ75mFc94"; do
	if ! printf '%s\n' "$donate_open" | grep -B1 -F "$addr" | head -1 | grep -qF "╭"; then
		fail "no rounded top over $addr"
	fi
	if ! printf '%s\n' "$donate_open" | grep -A1 -F "$addr" | tail -1 | grep -qF "╰"; then
		fail "no rounded bottom under $addr"
	fi
done
# The networks caption sits DIRECTLY under its box — the row two below the
# address (the box's bottom wall is the one between them) — so a reader who
# has found an address has found where it may be sent without hunting.
donate_caption_under() { # $1 address, $2 caption
	printf '%s\n' "$donate_open" | grep -A2 -F "$1" | tail -1 |
		sed 's/^[[:space:]]*//; s/[[:space:]]*$//' | grep -qxF "$2"
}
if ! donate_caption_under "bc1qhwamfrwuhz64pk00l75ykfff2ang22ns64chf7" "Network: Bitcoin (Native SegWit)"; then
	fail "the BTC networks caption is not under its box"
fi
if ! donate_caption_under "0xEAf6fbabB9DBE7a23BfE22A7A6c4aCe02063524b" \
	"Networks: Ethereum, Linea, Base, Arbitrum, BNB Chain, OP, Polygon"; then
	fail "the ETH networks caption is not under its box"
fi
if ! donate_caption_under "Gwhv5c6uAa6aAz1MjwzV9QJpbm7CJWy2kuCeZ75mFc94" "Network: Solana"; then
	fail "the SOL networks caption is not under its box"
fi
# The page replaces the composer: its footer is off screen while it is up.
expect_lacks "$donate_open" -F "dummy_model_name ·" "the footer is still on screen under the page"
# ↓ moves the ❯ to ETH.
tmux send-keys -t "$S112" Down
sleep 0.4
donate_eth="$(tmux capture-pane -t "$S112" -p)"
echo "==== Phase 112: ↓ highlights ETH ===="
printf '%s\n' "$donate_eth"
expect_has "$donate_eth" -F "❯ 2. ETH" "↓ did not move the ❯ to ETH"
expect_lacks "$donate_eth" -F "❯ 1. BTC" "BTC still wears the ❯ after ↓"
# `c` copies the highlighted address: the toast names the coin, and the
# OSC 52 fallback lands the address verbatim in tmux's buffer.
while tmux delete-buffer 2>/dev/null; do :; done
tmux send-keys -t "$S112" -l "c"
donate_copied="$(wait_pane 3 "$S112" -F "Copied the ETH address to clipboard")" # up to ~3s
echo "==== Phase 112: after c — the toast ===="
printf '%s\n' "$donate_copied"
expect_has "$donate_copied" -F "Copied the ETH address to clipboard" "c did not confirm the copy with a toast naming the coin"
donate_clip="$(tmux show-buffer 2>/dev/null)"
echo "==== Phase 112: tmux clipboard buffer (the OSC 52 fallback landed here) ===="
printf '%s\n' "$donate_clip"
if [ "$donate_clip" != "0xEAf6fbabB9DBE7a23BfE22A7A6c4aCe02063524b" ]; then
	fail "the clipboard does not hold the ETH address verbatim"
fi
expect_has "$donate_copied" -F "❯ 2. ETH" "copying closed the page"
# ↓ once more reaches the third row, SOL; a fourth ↓ wraps back to BTC.
tmux send-keys -t "$S112" Down
sleep 0.4
donate_sol="$(tmux capture-pane -t "$S112" -p)"
echo "==== Phase 112: ↓ highlights SOL ===="
printf '%s\n' "$donate_sol"
expect_has "$donate_sol" -F "❯ 3. SOL  Solana" "↓ did not move the ❯ to SOL"
tmux send-keys -t "$S112" Down
sleep 0.4
if ! tmux capture-pane -t "$S112" -p | grep -qF "❯ 1. BTC"; then
	fail "↓ past the last row did not wrap to BTC"
fi
# Esc closes: the composer and its footer come back, the page is gone.
tmux send-keys -t "$S112" Escape
sleep 0.4
donate_closed="$(tmux capture-pane -t "$S112" -p)"
echo "==== Phase 112: after Esc ===="
printf '%s\n' "$donate_closed"
expect_has "$donate_closed" -F "dummy_model_name ·" "the composer and footer did not come back after Esc"
expect_lacks "$donate_closed" -F "bc1qhwamfrwuhz64pk00l75ykfff2ang22ns64chf7" "the page is still on screen after Esc"
tmux kill-session -t "$S112" 2>/dev/null
