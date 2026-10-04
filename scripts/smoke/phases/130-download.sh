#!/usr/bin/env bash
# Phase 130 — a running command's window follows the bars still moving, not the finished ones below them

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# docs/tool-streaming.md *The window follows what is still moving*. The dummy's
# `download` demo plays pacman's parallel database refresh the way a terminal
# sees it: four bars redrawn in place, the two small ones finishing in the
# first second and sitting BELOW the two still downloading, every row within
# the terminal's reach and so streamed live. A window pinned to the output's
# end showed exactly those finished bars while the motion hid in `+N lines` —
# the reported "the inline cell only shows the finished lines". The cell must
# instead move its window onto the bars still moving once the finished ones
# have sat still for the active span, counting them into the footer — and the
# settled cell must still read like any `bash` cell: the head peek over the
# expand hint, no frame line.
S130="${S}_download"
launch "$S130" 100 40 "$APP"
submit "$S130" "refresh the package databases with pacman"
followed=0
for _ in $(seq 1 600); do
	cap="$(pane "$S130")"
	if has "$cap" -F "$SETTLED_REPLY"; then break; fi
	# The report's own picture, inverted: each bar wraps to two rows at this
	# width, and the sample shows core's and extra's percentage rows both
	# still below 100% while the finished multilib bar is hidden below the
	# window, under a footer counting the heading above and the two bars
	# below — the five rows the old tail showed instead.
	if has "$cap" -F " core " && has "$cap" -F " extra " &&
		[ "$(printf '%s\n' "$cap" | grep -cE '\] +[0-9]{1,2}%$')" = "2" ] &&
		lacks "$cap" -F " multilib" && lacks "$cap" -F "100%" &&
		has "$cap" -E '\+5 lines \([0-9]+s · wait 3m\)'; then
		followed=1
		followed_cap="$cap"
	fi
	sleep 0.05
done
if [ "$followed" = "1" ]; then
	echo "==== Phase 130: the running cell, its window on the two bars still moving ===="
	printf '%s\n' "$followed_cap"
fi
expect_eq "$followed" "1" "the running cell never moved its window onto the bars still downloading"
expect_has "${followed_cap:-}" -F "Bash(sudo pacman -Syy)" "the followed sample lost the cell's header"
wait_settled 30 "$S130" -F "$SETTLED_REPLY" >/dev/null
download="$(pane "$S130" -S -200)"
echo "==== Phase 130: the download demo, settled (with scrollback) ===="
printf '%s\n' "$download"
expect_has "$download" -F "● Bash(sudo pacman -Syy)" "the launch cell is missing"
expect_has "$download" -F "⎿  :: Synchronizing package databases..." "the settled cell does not open on the command's first line"
expect_has "$download" -F "… +6 lines (ctrl+o to expand)" "the settled cell should fold the rest of the two-row bars behind the hint"
expect_lacks "$download" -F "Exit code: 0" "the exited command's frame leaked onto the screen"
tmux kill-session -t "$S130" 2>/dev/null
echo "==== Phase 130: the running cell follows the moving bars; the settled cell is an ordinary bash cell ===="
