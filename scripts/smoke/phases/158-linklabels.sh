#!/usr/bin/env bash
# Phase 158 — a labelled link is its label where the terminal opens hyperlinks, `label (url)` elsewhere

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# LABELLED LINKS (docs/links.md *Labelled links*). A reply's `[label](url)`
# used to print `label (url)` everywhere — `Source: FIDE (https://www.fide.com/
# geneva-to-host-…/)` — an address nobody reads, spending a row. Where the
# terminal is known to open an OSC 8 hyperlink the label IS the link now:
# dressed as one, the whole address behind it, the address off the screen.
# Anywhere else the address stays, since hidden there it would simply be
# gone.
#
# This pane is tmux's, so by the app's own rule it is a terminal that keeps
# the address — a multiplexer forwards a hyperlink only to a terminal its
# configuration names. That makes all three cases drivable here:
#   1. ALTER_ZERO_LINK_TARGETS=hide — the override for a terminal the app
#      cannot name: the tour's `[link](https://github.com/linuztx/alter-zero)`
#      shows `link` alone, and the RAW byte stream (pipe-pane, read before
#      tmux interprets it) opens a hyperlink to the whole address around
#      exactly that word;
#   2. nothing set — the address is spelled out, as it always was;
#   3. hide WITH hyperlinks off — nothing is written for the label to stand
#      in front of, so the address is spelled out and no escape is emitted.
# The bare URL beside it (https://ratatui.rs/) is on screen in all three: it
# has no label to hide behind.
LINK_URL="https://github.com/linuztx/alter-zero"
LINK_PROMPT="stream some markdown to me"
# The tour streams in token-sized pieces at the dummy's chunk pace; a few
# milliseconds keeps this phase about the settled rows, not the stream.
LINK_APP="env $CFG_ENV_NOHIST ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS ALTER_ZERO_CHUNK_DELAY_MS=2"

# run_tour SESSION RAWFILE EXTRA_ENV — play the tour in a 100x60 pane and
# print the settled screen with its scrollback.
run_tour() {
	local sess="$1" raw="$2" extra="$3"
	launch "$sess" 100 60 "$LINK_APP $extra $BIN"
	tmux pipe-pane -t "$sess" -o "cat >> $raw"
	submit "$sess" "$LINK_PROMPT"
	wait_for 20 "$sess" -F "esc to interrupt" || fail "the tour never started in $sess"
	wait_settled 60 "$sess" -S -300 -- -F "$SETTLED_REPLY"
}

# 1. The override: the label alone, the address behind it.
S158="${S}_linklabel"
RAW_HIDE="$SMOKE_TMP/hide.raw"
hide_pane="$(run_tour "$S158" "$RAW_HIDE" "ALTER_ZERO_LINK_TARGETS=hide")"
dump "label only (the link row)" "$(printf '%s\n' "$hide_pane" | grep -F -B1 -A2 "Prose with")"
expect_has "$hide_pane" -F "a link and a bare URL, https://ratatui.rs/" "the label is not followed straight by the prose after the link"
expect_lacks "$hide_pane" -F "$LINK_URL" "the hidden address is on screen"
expect_lacks "$hide_pane" -F "link (" "the label is still followed by a parenthesis"
# The address the screen no longer shows rides the hyperlink: an OSC 8 open
# carrying ALL of it, then nothing but cursor and style sequences before the
# label's own letters.
if ! grep -aqE $'\x1b]8;id=az[0-9]+;https://github\\.com/linuztx/alter-zero\x1b\\\\(\x1b\\[[0-9;:]*[A-Za-z])*link' "$RAW_HIDE"; then
	fail "no hyperlink to the whole address opens around the label in the raw stream"
fi
if ! grep -aqF $'\x1b]8;;\x1b' "$RAW_HIDE"; then
	fail "the hyperlink around the label is never closed"
fi
# The carrier that got the address there never reaches the terminal as the
# underline colour it is stored as (SGR 58 — matched whole, to its `m`: on a
# pane this tall `ESC[58;1H` is a cursor move to row 58).
if grep -aqE $'\x1b\\[58[;:][0-9;:]*m' "$RAW_HIDE"; then
	fail "the link-id carrier leaked as an SGR 58 underline colour"
fi
# The label wears the link dress: underlined on screen. A styled capture
# writes a cell's attributes as an SGR of their own — single-digit codes,
# `4` the underline — ahead of its colour (and, from tmux 3.4, its
# hyperlink), so the label must follow one that holds a 4.
hide_styled="$(pane "$S158" -e -S -300 | grep -F "Prose with")"
if ! printf '%s\n' "$hide_styled" | grep -qE $'\x1b\\[([0-9];)*4(;[0-9])*m(\x1b\\[[0-9;:]*m|\x1b]8;[^\x1b]*\x1b\\\\)*link'; then
	fail "the label is not underlined: $(printf '%s' "$hide_styled" | cat -v)"
fi
tmux kill-session -t "$S158" 2>/dev/null

# 2. A terminal the app cannot vouch for keeps the address on screen.
S158B="${S}_linkshown"
RAW_SHOWN="$SMOKE_TMP/shown.raw"
shown_pane="$(run_tour "$S158B" "$RAW_SHOWN" "")"
dump "address shown (the link row)" "$(printf '%s\n' "$shown_pane" | grep -F -B1 -A2 "Prose with")"
expect_has "$shown_pane" -F "($LINK_URL)" "the address is not spelled out after the label"
if ! grep -aqE $']8;id=az[0-9]+;https://github\\.com/linuztx/alter-zero\x1b' "$RAW_SHOWN"; then
	fail "the spelled-out address is not a hyperlink"
fi
tmux kill-session -t "$S158B" 2>/dev/null

# 3. With hyperlinks off nothing may hide behind one.
S158C="${S}_linkgate"
RAW_GATE="$SMOKE_TMP/gate.raw"
gate_pane="$(run_tour "$S158C" "$RAW_GATE" "ALTER_ZERO_LINK_TARGETS=hide ALTER_ZERO_HYPERLINKS=0")"
dump "hide + hyperlinks off (the link row)" "$(printf '%s\n' "$gate_pane" | grep -F -B1 -A2 "Prose with")"
expect_has "$gate_pane" -F "($LINK_URL)" "the address was hidden with no hyperlink to reach it by"
if grep -aqF "]8;" "$RAW_GATE"; then
	fail "ALTER_ZERO_HYPERLINKS=0 still emitted OSC 8"
fi
tmux kill-session -t "$S158C" 2>/dev/null
