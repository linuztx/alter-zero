#!/usr/bin/env bash
# Phase 63 — STAGGERED back-to-back prompts

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# STAGGERED back-to-back prompts — the still-open prompt stays
# flush at the screen bottom (docs/permissions.md). The dummy's "staggered
# permission" turn scripts two gated Writes whose prompts differ wildly in
# height: the first's body caps (the prompt fills the terminal), the second's
# is one line — and answering the first commits its cell and opens the second
# in the same frame gap. The pinned modal region shrinks hard, and the one-way
# scrolls that pinned it cannot refill the bottom: the flush used to seat the
# short prompt high and blank everything below, stranding it above a band of
# empty rows for as long as it asked (the reported empty-newlines bug; the
# separate-frame ordering repin-shrinks into the same band). The loop now
# purge-rebuilds the moment a pinned modal frame would seat short of the
# bottom, so the open prompt lands flush with the resolved cell above it.
S63="${S}_staggered"
# The prompt's frame rule, as a fixed-string needle (a `─{60}` ERE would bind
# the repeat to the glyph's final UTF-8 byte and never match).
STAG_RULE="$(printf '─%.0s' $(seq 1 60))"
launch "$S63" 80 44
# Fill the screen so the region sits pinned at the bottom (a short
# conversation's floating prompt shrinks over blank rows and shows nothing).
for stagperm_msg in "hello there" "tell me more about it" "and a little more"; do
	submit "$S63" "$stagperm_msg"
	sleep 0.6
	for _ in $(seq 1 300); do
		cap="$(tmux capture-pane -t "$S63" -p)"
		if ! printf '%s' "$cap" | grep -qF "esc to interrupt"; then
			break
		fi
		sleep 0.1
	done
	sleep 0.3
done
submit "$S63" "staggered permission demo"
stagperm_prompt1=""
for _ in $(seq 1 300); do
	cap="$(tmux capture-pane -t "$S63" -p)"
	if printf '%s' "$cap" | grep -qF "Do you want to create big_module.py?"; then
		stagperm_prompt1="$cap"
		break
	fi
	sleep 0.05
done
sleep 0.3
stagperm_prompt1="$(tmux capture-pane -t "$S63" -p)"
echo "==== Phase 63: the tall (body-capped) first prompt open ===="
printf '%s\n' "$stagperm_prompt1"
# The tall prompt's closing rule sits on the pane's last row (body capped →
# the prompt fills the terminal) — the precondition that pins the region.
stagperm_rule1=$(printf '%s\n' "$stagperm_prompt1" | grep -nF "$STAG_RULE" | tail -1 | cut -d: -f1)
# Approve the tall write: its cell resolves and the tiny prompt lands in the
# same frame gap (the dummy scripts no pause between them).
tmux send-keys -t "$S63" -l "1"
stagperm_prompt2=""
for _ in $(seq 1 300); do
	cap="$(tmux capture-pane -t "$S63" -p)"
	if printf '%s' "$cap" | grep -qF "Do you want to create tiny_note.py?"; then
		stagperm_prompt2="$cap"
		break
	fi
	sleep 0.05
done
sleep 0.3
stagperm_prompt2="$(tmux capture-pane -t "$S63" -p)"
echo "==== Phase 63: the tiny second prompt open (must be flush at the bottom) ===="
printf '%s\n' "$stagperm_prompt2"
# THE point of this phase: the open prompt's closing rule is the pane's LAST
# row — no band of blank rows underneath while it asks.
stagperm_rule2=$(printf '%s\n' "$stagperm_prompt2" | grep -nF "$STAG_RULE" | tail -1 | cut -d: -f1)
tmux send-keys -t "$S63" -l "1"
for _ in $(seq 1 300); do
	cap="$(tmux capture-pane -t "$S63" -p)"
	if printf '%s' "$cap" | grep -qF "Both files are written" &&
		! printf '%s' "$cap" | grep -qF "esc to interrupt"; then
		break
	fi
	sleep 0.05
done
sleep 0.5
stagperm_final="$(tmux capture-pane -t "$S63" -p)"
echo "==== Phase 63: final pane (the box must be back flush at the bottom) ===="
printf '%s\n' "$stagperm_final"
stagperm_footer=$(printf '%s\n' "$stagperm_final" | grep -nF 'dummy_model_name ·' | tail -1 | cut -d: -f1)
stagperm_hist="$(tmux capture-pane -t "$S63" -p -S -300)"
stagperm_dupes=$(printf '%s\n' "$stagperm_hist" | grep -cF '❯ staggered permission demo')
stagperm_cell_dupes=$(printf '%s\n' "$stagperm_hist" | grep -cF 'Wrote 60 lines to big_module.py')
tmux kill-session -t "$S63" 2>/dev/null
echo "==== Phase 63: staggered prompts keep the open prompt flush at the bottom ===="
if [ -z "$stagperm_prompt1" ]; then
	fail "the tall first prompt never showed"
fi
if [ "${stagperm_rule1:-0}" != "44" ]; then
	fail "the tall prompt's closing rule sits on row ${stagperm_rule1:-none} of the 44-row pane (the body cap should fill the terminal), so the shrink check proves nothing"
fi
if [ -z "$stagperm_prompt2" ]; then
	fail "the tiny second prompt never showed"
fi
if [ "${stagperm_rule2:-0}" != "44" ]; then
	fail "while the tiny prompt is open its closing rule sits on row ${stagperm_rule2:-none} of the 44-row pane: the region shrank in place and stranded the prompt above a band of blank rows (the empty-newlines-under-the-prompt bug)"
fi
# The resolved tall cell committed above the still-open tiny prompt (visible
# at once, Phase 59's continuity), and the conversation stays whole after.
expect_has "$stagperm_prompt2" -F "Wrote 60 lines to big_module.py" "the tall write's finished cell is hidden while the tiny prompt is up"
if [ "${stagperm_footer:-0}" != "44" ]; then
	fail "after the prompts the footer sits on row ${stagperm_footer:-none} of 44 (the box is not back flush at the bottom)"
fi
if [ "${stagperm_dupes:-0}" != "1" ]; then
	fail "'❯ staggered permission demo' appears $stagperm_dupes times in scrollback+screen (a rebuild lost or duplicated a row)"
fi
if [ "${stagperm_cell_dupes:-0}" != "1" ]; then
	fail "the tall write's cell appears $stagperm_cell_dupes times in scrollback+screen (the mid-prompt rebuild lost or duplicated it)"
fi
