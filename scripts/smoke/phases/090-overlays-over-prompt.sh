#!/usr/bin/env bash
# Phase 90 — the READ-ONLY OVERLAYS stay reachable from a permission prompt

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# the READ-ONLY OVERLAYS stay reachable from a permission prompt
# (docs/permissions.md). The prompt is modal — it owns every key so the blocked
# tool thread can't be answered by accident — but Ctrl+O (the transcript) and
# Ctrl+D (the raw context) are exactly how you decide what to answer, so they
# are the two keys it lets through. Both must open over the open prompt, and
# both round trips must be a **no-op on the terminal**: the region is the one
# inline view that can be as tall as the screen and its growth scrolls chat
# one-way into real scrollback, so a return that repainted at the wrong seat
# would strand the prompt above a blank band or double the flowed rows. Driven
# at three heights — a region floating with rows to spare below it, one seated
# flush at the screen bottom, and one flush whose page FLOWS its top into real
# scrollback (docs/view-flow.md); each asserts the seat it is named for, so a
# case can never quietly degenerate into a copy of another.
# The screen AND the scrollback are compared byte for byte before and after —
# and where the region is meant to sit flush at the screen bottom the runner
# ASSERTS that first, since a stranding bug is invisible on a half-empty screen
# and the comparison would then prove nothing. Both exits are driven: the
# transcript is left by **Esc** — the key that would otherwise arm the
# edit-previous backtrack, so the trip exercises the guard keeping a parked
# tool thread's history un-rewindable (docs/backtrack.md) — and the context
# view by its own Ctrl+D. Phase 62 makes the same check in the other
# direction, with the overlay already up when the request lands.
po_round_trip() { # session, rows, cue, label, seat(flush | floating)
	local S90="$1" rows="$2" cue="$3" label="$4" seat="$5"
	tmux new-session -d -s "$S90" -x 100 -y "$rows" "$APP_PERM"
	sleep 0.4
	submit "$S90" "$cue"
	local po_open=""
	for _ in $(seq 1 250); do
		if tmux capture-pane -t "$S90" -p | grep -qF "Do you want to create"; then
			po_open=1
			break
		fi
		sleep 0.05
	done
	sleep 0.4
	if [ -z "$po_open" ]; then
		fail "($label) the permission prompt never showed"
		tmux kill-session -t "$S90" 2>/dev/null
		return
	fi
	local before before_sb in_o back_o in_d back_o_sb after_sb
	local before_rule back_rule
	before="$(tmux capture-pane -t "$S90" -p)"
	before_sb="$(tmux capture-pane -t "$S90" -p -S -100)"
	# Where the prompt's closing rule sits before the trip. Each case asserts
	# the seat it is *named* for — a flush case that had drifted off the
	# bottom would make "it came back flush" vacuous, and a floating case that
	# had grown flush would silently become a third copy of the flush one.
	before_rule=$(printf '%s\n' "$before" | grep -nF '────' | tail -1 | cut -d: -f1)
	if [ "$seat" = flush ] && [ "${before_rule:-0}" != "$rows" ]; then
		fail "($label) the prompt is not flush at the bottom before the trip (closing rule on row ${before_rule:-none} of $rows), so the return check proves nothing"
	fi
	if [ "$seat" = floating ] && [ "${before_rule:-0}" = "$rows" ]; then
		fail "($label) the prompt already reaches row $rows, so this is not the floating geometry it is meant to cover"
	fi
	# Ctrl+O — the transcript, on the alternate screen, over the open prompt.
	tmux send-keys -t "$S90" C-o
	sleep 0.7
	in_o="$(tmux capture-pane -t "$S90" -p)"
	# …left with **Esc**, the key that would otherwise arm the edit-previous
	# preview: under an open prompt it must simply close the overlay.
	tmux send-keys -t "$S90" Escape
	sleep 0.9
	back_o="$(tmux capture-pane -t "$S90" -p)"
	back_o_sb="$(tmux capture-pane -t "$S90" -p -S -100)"
	back_rule=$(printf '%s\n' "$back_o" | grep -nF '────' | tail -1 | cut -d: -f1)
	if [ "$seat" = flush ] && [ "${back_rule:-0}" != "$rows" ]; then
		fail "($label) after the return the prompt's closing rule sits on row ${back_rule:-none} of $rows: it is stranded above a band of blank rows"
	fi
	# Ctrl+D — the raw LLM context, the same dance.
	tmux send-keys -t "$S90" C-d
	sleep 0.7
	in_d="$(tmux capture-pane -t "$S90" -p)"
	tmux send-keys -t "$S90" C-d
	sleep 0.9
	after_sb="$(tmux capture-pane -t "$S90" -p -S -100)"
	local back_d
	back_d="$(tmux capture-pane -t "$S90" -p)"
	echo "==== Phase 90 ($label): captured pane (the transcript over the prompt) ===="
	printf '%s\n' "$in_o"
	expect_has "$in_o" -F "T R A N S C R I P T" "($label) Ctrl+O did not open the transcript over the prompt"
	# The call being asked about is what the transcript must show — the whole
	# point of looking is reading what you are approving.
	expect_has "$in_o" -F "Waiting…" "($label) the transcript hides the call the prompt is asking about"
	# The Esc that left it must NOT have armed the backtrack preview — a rewind
	# would truncate the history a parked tool thread is still waiting on — and
	# the hint row promises exactly that, off the same `modal_open` predicate.
	expect_has "$in_o" -F "q/esc/ctrl+o to quit" "($label) the overlay offers 'esc to edit prev' under an open prompt: Esc would arm a rewind of the history a parked tool thread is waiting on"
	expect_has "$back_o" -F "Do you want to create" "($label) Esc from the overlay did not land back on the still-open prompt"
	expect_has "$in_d" -F "C O N T E X T" "($label) Ctrl+D did not open the context view over the prompt"
	if [ "$before" != "$back_o" ]; then
		fail "($label) the Ctrl+O round trip changed the screen under the prompt"
		diff <(printf '%s\n' "$before") <(printf '%s\n' "$back_o") >&2
	fi
	if [ "$back_o" != "$back_d" ]; then
		fail "($label) the Ctrl+D round trip changed the screen under the prompt"
		diff <(printf '%s\n' "$back_o") <(printf '%s\n' "$back_d") >&2
	fi
	if [ "$before_sb" != "$back_o_sb" ] || [ "$before_sb" != "$after_sb" ]; then
		fail "($label) the round trips lost or doubled rows in the scrollback"
		diff <(printf '%s\n' "$before_sb") <(printf '%s\n' "$after_sb") >&2
	fi
	# …and the prompt is still answerable afterwards, draft restore included.
	tmux send-keys -t "$S90" -l "1"
	local po_done=""
	for _ in $(seq 1 250); do
		if tmux capture-pane -t "$S90" -p -S -80 | grep -qF "Wrote 8 lines to hello.py"; then
			po_done=1
			break
		fi
		sleep 0.05
	done
	if [ -z "$po_done" ]; then
		fail "($label) the prompt no longer resolves after the overlay round trips"
	fi
	tmux kill-session -t "$S90" 2>/dev/null
}
# 44 rows: the whole conversation and the prompt fit with rows to spare, so the
# region floats — nothing has scrolled one-way, and the return must not start.
po_round_trip "${S}_permoverlay" 44 "permission demo please" "region floating" floating
# 30 rows: everything still fits the page, but the region now reaches the
# screen bottom — the seat a mis-timed return strands above a blank band.
po_round_trip "${S}_permoverlayfit" 30 "permission demo please" "page fits, flush" flush
# 18 rows: the page is taller than the terminal, so its top FLOWS into real
# scrollback (docs/view-flow.md) — the geometry the close's purge rebuild
# exists for, and the only one that exercises the flow signature across the
# overlay round trip.
po_round_trip "${S}_permoverlaytall" 18 "permission demo please" "page flows" flush
echo "==== Phase 90: Ctrl+O / Ctrl+D open over a permission prompt and give the screen back unchanged ===="
