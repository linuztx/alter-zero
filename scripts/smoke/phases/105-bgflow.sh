#!/usr/bin/env bash
# Phase 105 — the ↓ MANAGER BAND FLOWS ITS PAGE TOP

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# the ↓ MANAGER BAND FLOWS ITS PAGE TOP (docs/view-flow.md,
# docs/background.md). On a terminal shorter than the details page, the band
# bottom-anchors — and the rows the anchor skips used to be dropped into NO
# buffer at all: the conversation ran straight into a headless output box, and
# scrolling the terminal up never found the `Shell details` title, the status
# or the command (the reported bug). They flow into real scrollback now, like
# every other framed view — while the page itself keeps live-tailing, because
# this one flow is signed on the SHELL rather than on its ticking rows
# (`FlowSign::Frozen`), so the frozen top costs no purge rebuild per frame.
# Closing the band purges the flowed rows and leaves the conversation intact.
S105="${S}_bgflow"
BGF_SCRIPT="$SMOKE_CFG/bgflow.sh"
cat >"$BGF_SCRIPT" <<'EOS'
i=0
while [ $i -lt 900 ]; do
	echo flowline$i
	i=$((i + 1))
	sleep 0.05
done
EOS
# 18 rows: shorter than the details page (26 rows at this width), so the top
# rule, the title and the whole field block are what the anchor skips.
launch "$S105" 100 18
# A committed cell above the band, so the close can be checked to leave the
# conversation — the thing the user scrolls up for — untouched.
submit "$S105" "!echo FLOWCONV_MARKER_105"
sleep 0.6
submit "$S105" "!sh $BGF_SCRIPT"
# Wait past TOOL_BACKGROUND_HINT_DELAY, then hand the run to the registry.
wait_for 8 "$S105" -F "(ctrl+b to run in background)"
tmux send-keys -t "$S105" C-b
sleep 0.5
tmux send-keys -t "$S105" Down # focus the footer's shell indicator
sleep 0.3
tmux send-keys -t "$S105" Enter # open the list
sleep 0.4
tmux send-keys -t "$S105" Enter # open the details page
wait_for 4 "$S105" -F "flowline"
bgf_pane="$(wait_pane 4 "$S105" -F "to go back")"
bgf_full="$(tmux capture-pane -t "$S105" -p -S -200)"
echo "==== Phase 105: the screen-tall details page (visible pane) ===="
printf '%s\n' "$bgf_pane"
# The visible screen keeps the page's TAIL — the hints and the closing rule.
expect_has "$bgf_pane" -F "to go back" "the details tail (← to go back …) is not on screen"
if ! printf '%s\n' "$bgf_pane" | awk 'END { exit ($0 ~ /──/) ? 0 : 1 }'; then
	fail "the bottom rule is not the last screen row"
fi
# …and the page genuinely overflowed: its top is NOT on the visible screen…
expect_lacks "$bgf_pane" -F "Shell details" "the page fits the pane; the fixture must overflow for this phase to test the flow"
# …but IS in the terminal's real scrollback, whole — the bug this phase guards.
for expect in "Shell details" "Status:" "Runtime:" "Command:"; do
	if ! printf '%s' "$bgf_full" | grep -qF "$expect"; then
		fail "the flowed page top is missing '$expect' from scrollback+screen"
		printf '%s\n' "$bgf_full" >&2
	fi
done
# The flow FREEZES: the page keeps tailing live (the box advances) while the
# flowed rows stay put — committed once, never re-flowed per tick, and never
# purge-rebuilt out from under the conversation above them.
bgf_seq_before="$(printf '%s\n' "$bgf_pane" | grep -oE 'flowline[0-9]+' | tail -1)"
bgf_seq_after="$bgf_seq_before"
for _ in $(seq 1 40); do
	bgf_later="$(tmux capture-pane -t "$S105" -p)"
	bgf_seq_after="$(printf '%s\n' "$bgf_later" | grep -oE 'flowline[0-9]+' | tail -1)"
	if [ -n "$bgf_seq_after" ] && [ "$bgf_seq_after" != "$bgf_seq_before" ]; then
		break
	fi
	sleep 0.2
done
if [ "$bgf_seq_after" = "$bgf_seq_before" ]; then
	fail "the details box stopped tailing under the flow ($bgf_seq_before)"
fi
bgf_held="$(tmux capture-pane -t "$S105" -p -S -200)"
echo "==== Phase 105: the flowed top after the page has ticked on ===="
printf '%s\n' "$bgf_held" | grep -n "Shell details" || true
bgf_titles="$(printf '%s\n' "$bgf_held" | grep -cF "Shell details" || true)"
if [ "$bgf_titles" != "1" ]; then
	fail "the flowed title is in scrollback $bgf_titles times, expected exactly 1"
	printf '%s\n' "$bgf_held" >&2
fi
expect_has "$bgf_held" -F "FLOWCONV_MARKER_105" "the conversation above the flowed page was lost from scrollback"
# Esc closes the band: the flowed rows purge, the conversation stays.
tmux send-keys -t "$S105" Escape
sleep 0.8
bgf_closed="$(tmux capture-pane -t "$S105" -p -S -200)"
echo "==== Phase 105: closed back to the composer ===="
printf '%s\n' "$bgf_closed" | tail -14
expect_lacks "$bgf_closed" -F "Shell details" "stale flowed rows survived the close (the purge should have wiped them)"
expect_has "$bgf_closed" -F "FLOWCONV_MARKER_105" "the conversation did not survive the flow-exit rebuild"
expect_has "$bgf_closed" -F "Running in the background" "the backgrounded cell did not survive the flow-exit rebuild"
tmux kill-session -t "$S105" 2>/dev/null
rm -f "$BGF_SCRIPT"
