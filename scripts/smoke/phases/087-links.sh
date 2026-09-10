#!/usr/bin/env bash
# Phase 87 — CLICKABLE LINKS

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# CLICKABLE LINKS (docs/links.md). A URL wider than its row
# hard-breaks across display rows — this 26-col pane splits the reply's
# https://github.com/linuztx exactly like the report — and the terminal's own
# per-row URL detection then opened only the first fragment on click. Every
# painted fragment must ride an OSC 8 hyperlink carrying the FULL target. The
# escape renders as nothing, so the assertion reads the RAW byte stream
# (pipe-pane, captured before tmux interprets it); the link-id carrier must
# never leak as a real underline colour (SGR 58); and a falsy
# ALTER_ZERO_HYPERLINKS must emit no escape while the visible wrap stays
# identical.
S87="${S}_links"
LINKS_RAW="$(mktemp "$SMOKE_TMP/links.XXXXXX")"
tmux new-session -d -s "$S87" -x 26 -y 40 "$APP"
tmux pipe-pane -t "$S87" -o "cat >> $LINKS_RAW"
sleep 0.4
submit "$S87" "$USER_MSG"
# Poll the raw stream for the REPLY's hyperlink open — the one carrying the
# github URL, which lands when the reply's URL commits — then give the rest of
# the turn a beat to settle. Any OSC 8 open won't do: the demo's
# `Read/Edit(about.py)` headers are file:// links now (docs/links.md), and
# those land with the tool cells, well before the reply streams — a wait that
# broke on the first open captured the pane mid-tool-loop.
for _ in $(seq 1 120); do
	if grep -aqE ']8;id=az[0-9]+;https://github\.com/linuztx' "$LINKS_RAW"; then
		break
	fi
	sleep 0.1
done
sleep 1.0
links_pane="$(tmux capture-pane -t "$S87" -p -S -200)"
echo "==== Phase 87: the 26-col pane wraps the URL (tail) ===="
printf '%s\n' "$links_pane" | tail -30
# The visible text is what it always was: the URL hard-breaks at the 24-col
# content width, so NO single row holds it whole — the split prefix row is
# there and the unbroken URL is not.
expect_has "$links_pane" -F "https://github.com/linuz" "the reply's wrapped URL prefix is missing from the pane"
expect_lacks "$links_pane" -F "https://github.com/linuztx" "the URL fits one row; narrow the pane so this phase tests the split"
# The raw stream carries what the screen cannot show: an OSC 8 open whose URI
# is the WHOLE URL (ESC ] 8 ; id=azN ; url ESC \), plus its close.
if ! grep -aqE $']8;id=az[0-9]+;https://github\\.com/linuztx\x1b' "$LINKS_RAW"; then
	fail "no OSC 8 open carries the full URL in the raw stream"
fi
if ! grep -aqF $'\x1b]8;;\x1b' "$LINKS_RAW"; then
	fail "the OSC 8 close is missing from the raw stream"
fi
# The id carrier (an RGB underline colour) is stripped at the paint boundary —
# a leaked SGR 58 would draw garbage underlines on supporting terminals.
if grep -aqF $'\x1b[58;' "$LINKS_RAW"; then
	fail "the link-id carrier leaked as an SGR 58 underline colour"
fi
tmux kill-session -t "$S87" 2>/dev/null

# The env gate: ALTER_ZERO_HYPERLINKS=0 emits no escapes — and the visible
# wrap is identical, so turning links off can never change the layout.
LINKS_RAW_OFF="$(mktemp "$SMOKE_TMP/links.XXXXXX")"
tmux new-session -d -s "$S87" -x 26 -y 40 "env ALTER_ZERO_HYPERLINKS=0 $CFG_ENV_NOHIST ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN"
tmux pipe-pane -t "$S87" -o "cat >> $LINKS_RAW_OFF"
sleep 0.4
submit "$S87" "$USER_MSG"
links_off_pane="$(wait_pane 12 "$S87" -S -200 -- -F "https://github.com/linuz")"
echo "==== Phase 87: gate off — same wrap, no escapes ===="
printf '%s\n' "$links_off_pane" | tail -12
expect_has "$links_off_pane" -F "https://github.com/linuz" "the wrapped URL is missing with the gate off"
if grep -aqF "]8;" "$LINKS_RAW_OFF"; then
	fail "ALTER_ZERO_HYPERLINKS=0 still emitted OSC 8"
fi
if grep -aqF $'\x1b[58;' "$LINKS_RAW_OFF"; then
	fail "the carrier leaked as SGR 58 with the gate off"
fi
tmux kill-session -t "$S87" 2>/dev/null
rm -f "$LINKS_RAW" "$LINKS_RAW_OFF"
