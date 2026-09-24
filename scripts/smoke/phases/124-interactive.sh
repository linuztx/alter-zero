#!/usr/bin/env bash
# Phase 124 — an interactive session's cells, offline, a progress bar redrawn in place

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# an interactive session's cells (docs/interactive-shell.md). The dummy's
# `interactive` demo plays the shape a live model produces: `bash` with `tty`
# stopping at a prompt, then `bash_session` answering one question a round
# until the program exits — every result the real report formatter's. What
# only the real binary shows is how those reports land on screen: the frame
# line (`Running (session …, waiting for input)`) is the MODEL's, so it must
# never reach a cell; a session still at its prompt closes its cell on a dim
# `⎿ Waiting for input · session …` row instead; an answer's header names the
# command it types into and what was typed (`BashSession(./configure.sh ←
# demo⏎)`); and the last answer, whose program exited, reads like a plain
# `bash` cell — no state row, no `Exit code: 0` frame.
S124="${S}_interactive"
launch "$S124" 100 40 "$APP"
submit "$S124" "run an interactive installer"
# The last answer's install bar streams as the running cell's live screen:
# each frame REPLACES the last, so no sample of the screen may ever hold two
# of its rows — a bar forwarded frame by frame would stack them.
max_bar=0
for _ in $(seq 1 300); do
	cap="$(pane "$S124")"
	n=$(printf '%s\n' "$cap" | grep -cF "Installing  [")
	[ "$n" -gt "$max_bar" ] && max_bar=$n
	if has "$cap" -F "$SETTLED_REPLY"; then break; fi
	sleep 0.05
done
expect_eq "$max_bar" "1" "the install bar showed more than one frame at once (or never showed)"
wait_settled 20 "$S124" -F "$SETTLED_REPLY" >/dev/null
interactive="$(pane "$S124" -S -200)"
echo "==== Phase 124: the interactive demo, settled (with scrollback) ===="
printf '%s\n' "$interactive"
expect_has "$interactive" -F "● Bash(./configure.sh)" "the tty launch cell is missing"
expect_has "$interactive" -F "⎿  Project name:" "the launch cell does not show the prompt it stopped at"
expect_has "$interactive" -F "⎿  Waiting for input · session b7x2k9m1q" "a session still at its prompt has no dim state row"
expect_has "$interactive" -F "● BashSession(./configure.sh ← demo⏎)" "the answer cell does not name the command and the input"
expect_has "$interactive" -F "Install into ./demo? [Y/n]" "the answer cell does not show the next question"
expect_has "$interactive" -F "● BashSession(./configure.sh ← y⏎)" "the last answer's cell is missing"
expect_has "$interactive" -F "Created ./demo (3 files)." "the exited program's last output is missing"
expect_lacks "$interactive" -F "Running (session" "the model's frame line leaked onto the screen"
expect_lacks "$interactive" -F "Exit code: 0" "the exited session's frame leaked onto the screen"
bar_rows="$(printf '%s\n' "$interactive" | grep -cF "Installing  [")"
expect_eq "$bar_rows" "1" "the settled conversation should hold the install bar exactly once, in its final state"
expect_has "$interactive" -F "Installing  [##########] 100%" "the install bar did not settle at 100%"
waiting_rows="$(printf '%s\n' "$interactive" | grep -cF "Waiting for input · session b7x2k9m1q")"
expect_eq "$waiting_rows" "2" "the two cells whose session was still at a prompt should each end on the state row — and the exited one on none"
tmux kill-session -t "$S124" 2>/dev/null
echo "==== Phase 124: session cells show the program's output under a dim state row, never the model's frame ===="
