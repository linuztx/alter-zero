#!/usr/bin/env bash
# Phase 99 — a BAND OPENED UNDER A TALL STREAMING PREVIEW keeps the composer

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# a BAND OPENED UNDER A TALL STREAMING PREVIEW keeps the composer
# (docs/table-streaming.md "The preview slot is budgeted"). The strip's preview
# was capped against a FIXED chrome allowance — the box, the status line, a
# footer — so a forming table always spent the region's whole slack. Press `/`
# (or `?`, or `@`) mid-table on a small terminal and the band asked for rows
# the terminal did not have: the constraint solver spent them on the strip and
# the textarea disappeared until the turn ended — the reported bug. The budget
# now pays every other row the region owes first and the preview tail-follows
# into what is left, so the box, the band and the status line are all on screen
# together. Driven at the reported size with each of the three bands.
S99="${S}_bandpreview"
for band_key in "/" "?" "@s"; do
	tmux kill-session -t "$S99" 2>/dev/null
	tmux new-session -d -s "$S99" -x 51 -y 24 "$APP"
	sleep 0.7
	submit "$S99" "response again but now with table"
	# Wait until the block is genuinely taller than the region's slack: the
	# demo table's fifth record is well past it at 51 columns.
	band_ready=""
	for _ in $(seq 1 250); do
		if tmux capture-pane -t "$S99" -p | grep -qF "Shell Command"; then
			band_ready=1
			break
		fi
		sleep 0.1
	done
	if [ -z "$band_ready" ]; then
		fail "the table demo never streamed past the region's slack"
	fi
	tmux send-keys -t "$S99" -l "$band_key"
	sleep 0.5
	band_pane="$(tmux capture-pane -t "$S99" -p)"
	echo "==== Phase 99: '$band_key' opened mid-table on a 51x24 pane ===="
	printf '%s\n' "$band_pane"
	# The composer: its prompt row, and both of the box's rules around it.
	expect_has "$band_pane" -E '^❯' "'$band_key' mid-table squeezed the composer off the screen"
	# The rules are counted in awk's byte mode (the suite runs in a POSIX
	# locale, where `─+` would quantify the glyph's last byte): a rule row is
	# one that is nothing but `─`.
	band_rules="$(printf '%s\n' "$band_pane" |
		awk '{ bare = $0; gsub(/─/, "", bare); if (length($0) > 0 && length(bare) == 0) n++ } END { print n + 0 }')"
	if [ "$band_rules" -ne 2 ]; then
		fail "the box lost a rule under the '$band_key' band ($band_rules of 2)"
	fi
	# …and neither the band nor the turn's status line was traded away for it.
	expect_has "$band_pane" -F "Working…" "the status line went missing under the '$band_key' band"
	case "$band_key" in
	"/") band_marker="/help" ;;
	"?") band_marker="for commands" ;;
	*) band_marker="Dir" ;;
	esac
	expect_has "$band_pane" -F "$band_marker" "the '$band_key' band did not open"
	# The preview yielded rather than the composer: the forming block is still
	# there, tail-following into the rows that are left.
	expect_has "$band_pane" -E "(Code Snippet|│)" "the forming table vanished from the strip entirely"
done
tmux kill-session -t "$S99" 2>/dev/null
echo "==== Phase 99: a band opened under a tall streaming preview keeps the composer ===="
