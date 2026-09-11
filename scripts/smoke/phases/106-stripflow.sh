#!/usr/bin/env bash
# Phase 106 — the STRIP FLOW

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# the STRIP FLOW (docs/strip-flow.md). The streaming strip is
# the live region's only elastic content, and the rows it could not afford
# were thrown away: on a short terminal a running command lost its
# `+N lines (Ns)` footer and its ctrl+b hint first, then its output rows, then
# its header, and past that the spinner status line — none of it in any
# buffer. The strip bottom-anchors now and the rows it cannot paint FLOW into
# the terminal's real scrollback, frozen: the strip keeps its newest rows and
# its head stays readable by scrolling up. The turn's end purges the frozen
# rows and commits the real cells exactly once.
S106="${S}_stripflow"
# 12 rows: room for the composer, the footer and the status line, but not for
# a parallel batch's three cells — the regime the bug report was taken in.
launch "$S106" 80 12
submit "$S106" "run three pings in parallel"
sf_pane=""
sf_full=""
for _ in $(seq 1 400); do # the batch is announced a couple of seconds in
	sf_pane="$(tmux capture-pane -t "$S106" -p)"
	if printf '%s' "$sf_pane" | grep -qF "esc to interrupt" \
		&& printf '%s' "$sf_pane" | grep -qF "Waiting…"; then
		sf_full="$(tmux capture-pane -t "$S106" -p -S -80)"
		break
	fi
	sleep 0.05
done
echo "==== Phase 106: the squeezed strip mid-batch (visible pane) ===="
printf '%s\n' "$sf_pane"
# The anchor keeps the strip's TAIL — the last queued sibling and, below it,
# the spinner status line, which a starved strip used to clip away.
for expect in "Bash(ping -c 20 x.invalid)" "esc to interrupt"; do
	expect_has "$sf_pane" -F "$expect" "'$expect' is not on the squeezed screen"
done
# …and the strip genuinely overflowed: the running call at its head is NOT on
# the visible screen…
expect_lacks "$sf_pane" -F "Bash(ping -c 20 google.com)" "the strip fits the pane; the fixture must overflow for this phase to test the flow"
# …but IS in the terminal's real scrollback, running row and all, with the
# conversation still above it. Exactly once: a flow re-committed per frame
# would be a purge rebuild at 30fps.
echo "==== Phase 106: the flowed strip head in scrollback ===="
printf '%s\n' "$sf_full" | tail -18
for expect in "Bash(ping -c 20 google.com)" "run three pings in parallel"; do
	if ! printf '%s' "$sf_full" | grep -qF "$expect"; then
		fail "the flowed strip head is missing '$expect' from scrollback+screen"
		printf '%s\n' "$sf_full" >&2
	fi
done
# Its running row too — matched as a whole cell row (`⎿  Running…` alone),
# never as the substring the demo's own narration also contains.
if ! printf '%s\n' "$sf_full" | grep -qE '^[[:space:]]*⎿[[:space:]]+Running…[[:space:]]*$'; then
	fail "the flowed head lost the running call's '⎿ Running…' row"
	printf '%s\n' "$sf_full" >&2
fi
sf_heads="$(printf '%s\n' "$sf_full" | grep -cF "Bash(ping -c 20 google.com)" || true)"
if [ "$sf_heads" != "1" ]; then
	fail "the flowed head is in scrollback $sf_heads times, expected exactly 1"
fi
# The turn ends: the flow clears, the frozen rows are purged, and the three
# cells commit — once each — with nothing of the strip left behind.
sf_done="$(wait_pane 30 "$S106" -S -120 -- -E 'Done for [0-9]+s')"
echo "==== Phase 106: the turn resolved, the frozen rows purged ===="
printf '%s\n' "$sf_done" | tail -16
expect_lacks "$sf_done" -F "esc to interrupt" "the frozen status line survived the turn (the purge should have wiped the strip)"
# The live cell rows, as whole rows: this demo's closing narration *quotes*
# `⎿ Waiting…` mid-sentence, so a substring match would fail on the prose.
for stale in "Waiting…" "Running…"; do
	expect_lacks "$sf_done" -E "^[[:space:]]*⎿[[:space:]]+${stale}[[:space:]]*\$" "a frozen '⎿ $stale' row survived the turn (the purge should have wiped the strip)"
done
for cell in "Bash(ping -c 20 google.com)" "Bash(ping -c 20 facebook.com)" "Bash(ping -c 20 x.invalid)"; do
	sf_n="$(printf '%s\n' "$sf_done" | grep -cF "$cell" || true)"
	if [ "$sf_n" != "1" ]; then
		fail "'$cell' committed $sf_n times, expected exactly 1"
	fi
done
expect_has "$sf_done" -F "run three pings in parallel" "the conversation did not survive the flow-exit rebuild"
tmux kill-session -t "$S106" 2>/dev/null
