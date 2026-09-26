#!/usr/bin/env bash
# Phase 5 — after a reply finishes, the input box stays flush at the BOTTOM

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# after a reply finishes, the input box stays flush at the BOTTOM —
# no blank rows creep in below it when the streaming strip (preview + gap, drawn
# *above* the box) clears. A short 40x12 terminal makes one exchange overflow the
# screen so the box is pushed to the bottom while streaming; the bug let the box
# rise by the strip's height once the reply finished, leaving blank rows beneath.
S2="${S}_bottom"
TMP5="$(mktemp)"
launch "$S2" 40 12
submit "$S2" "hello there"
# Wait until the reply has fully finished (its closing hand-off paragraph is
# committed) AND the screen has stopped changing — so we measure the *settled*
# layout, not a mid-stream frame (where the box legitimately sits at the bottom).
settled_prev=""
for _ in $(seq 1 60); do # up to ~12s
	tmux capture-pane -t "$S2" -p >"$TMP5"
	settled_cur="$(cat "$TMP5")"
	if printf '%s' "$settled_cur" | grep -qF "$SETTLED_REPLY" &&
		[ "$settled_cur" = "$settled_prev" ]; then
		break
	fi
	settled_prev="$settled_cur"
	sleep 0.2
done
echo "==== captured pane (box settled at the bottom after the reply) ===="
cat "$TMP5"
# Count blank rows below the box: walk up from the last pane row while it is blank
# (strip only ASCII space/tab so the multibyte box rule still counts as content).
trailing_blanks=$(awk '{a[NR]=$0} END{c=0; for(i=NR;i>=1;i--){t=a[i]; gsub(/[ \t]/,"",t); if(t==""){c++}else break} print c}' "$TMP5")
tmux kill-session -t "$S2" 2>/dev/null
rm -f "$TMP5"

if ! printf '%s' "$settled_cur" | grep -qF "$SETTLED_REPLY"; then
	fail "the reply never finished on the short terminal (Phase 5 could not settle)"
else
	# When the turn ends the live status line is replaced by a committed
	# summary: the past tense of the verb the line wore ("Worked for Ns").
	expect_has "$settled_cur" -E "^$SUMMARY_RE" "the committed turn summary ('Worked for Ns') was not shown after the reply finished"
	if [ "${trailing_blanks:-99}" -ne 0 ]; then
		fail "$trailing_blanks blank row(s) left below the input box after the reply settled — the box should stay flush at the bottom"
	fi
fi
