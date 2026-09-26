#!/usr/bin/env bash
# Phase 76 — the `Skill` tool loads an authored SKILL.md

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# the `Skill` tool loads an authored SKILL.md (docs/skills.md).
# The cell is the WHOLE visible surface — `● Skill(dataviz)` over one green
# `⎿  Successfully loaded skill` row — while the model reads the skill's entire
# body. That split is the feature: a 400-line skill would otherwise dump itself
# into the transcript every time it is used. Ctrl+D must show the body (it is
# what was really sent) and Ctrl+O must expand it, while the inline conversation
# shows neither.
S76="${S}_skills"
tmux new-session -d -s "$S76" -x 100 -y 30 "$APP; echo CLI_APP_EXITED; sleep 60"
sleep 0.5
submit "$S76" "load a skill for me"
wait_for 24 "$S76" -S -80 -- -E "$SUMMARY_RE"
skills_pane="$(tmux capture-pane -t "$S76" -p -S -80)"
echo "==== Phase 76: the skill cell ===="
printf '%s\n' "$skills_pane"
expect_has "$skills_pane" -E "● Skill\(dataviz\)" "no '● Skill(dataviz)' cell header"
expect_has "$skills_pane" -F "Successfully loaded skill" "the cell is missing its 'Successfully loaded skill' row"
# The body is the MODEL's, not the transcript's: none of it may reach the
# inline conversation.
expect_lacks "$skills_pane" -F "Base directory for this skill" "the skill body leaked into the inline conversation"
expect_lacks "$skills_pane" -F "references/palette.md" "the skill body's text leaked into the inline conversation"
# Ctrl+D: the derived context must carry the body, because that IS what the
# model was sent (the ToolOutcome::context split, docs/skills.md).
tmux send-keys -t "$S76" C-d
sleep 0.6
skills_ctx="$(tmux capture-pane -t "$S76" -p -S -400)"
echo "==== Phase 76: the Ctrl+D context ===="
printf '%s\n' "$skills_ctx"
for expect in "Base directory for this skill" "references/palette.md"; do
	expect_has "$skills_ctx" -F "$expect" "the context view is missing '$expect'"
done
tmux send-keys -t "$S76" q
sleep 0.5
# Ctrl+O: the transcript keeps the CELL, not the body. The two-text split's
# rule everywhere here (a rejected call's transcript shows its red display
# line, not the model-facing instruction): the transcript is what happened,
# Ctrl+D is what was sent. It is also the reference's behaviour, and it keeps
# a 100 KiB skill out of the render cache.
tmux send-keys -t "$S76" C-o
sleep 0.6
skills_tx="$(tmux capture-pane -t "$S76" -p -S -400)"
echo "==== Phase 76: the Ctrl+O transcript ===="
printf '%s\n' "$skills_tx"
expect_has "$skills_tx" -E "● Skill\(dataviz\)" "the transcript lost the skill cell"
expect_has "$skills_tx" -F "Successfully loaded skill" "the transcript lost the cell's resolved row"
expect_lacks "$skills_tx" -F "Base directory for this skill" "the transcript dumped the skill body (that belongs to ctrl+d)"
tmux send-keys -t "$S76" q
sleep 0.5
skills_back="$(tmux capture-pane -t "$S76" -p)"
expect_has "$skills_back" -F "dummy_model_name" "the composer did not come back after the overlays"
tmux kill-session -t "$S76" 2>/dev/null || true
