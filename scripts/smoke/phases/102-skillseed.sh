#!/usr/bin/env bash
# Phase 102 — the built-in SKILL is seeded as a real, editable directory

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# the built-in SKILL is seeded as a real, editable directory
# (docs/skills.md). `skill-creator` is a `skills/<name>/SKILL.md` under the user
# config home, not a string in the binary, so the same three things are
# filesystem behaviour no unit test can see: the first launch has to WRITE it
# — the whole directory, its `reference.md` included, since the body sends the
# model there for the substitution tokens a body cannot spell out — a later
# launch must never clobber a copy the user edited, and a deleted file has to
# come back. The fourth is the difference from the agent definitions: an
# ALTER_ZERO_SKILLS_DIR override is NEVER seeded into (it says "these are the
# skills, and only these" — and this suite's own hermetic sessions depend on
# that dir staying exactly as empty as it was made).
S102="${S}_skillseed"
SK_CFG="$(mktemp -d)"
SK_HOME="$(mktemp -d)"
# No ALTER_ZERO_SKILLS_DIR here — the override is what this phase must see
# switched off. HOME is redirected instead so the walk's `~/.claude/skills` row
# finds nothing and the run stays hermetic on a machine that has real skills.
APP_SK="env ALTER_ZERO_TELEMETRY=0 ALTER_ZERO_PROJECT_CONFIG=0 HOME=$SK_HOME ALTER_ZERO_CONFIG_DIR=$SK_CFG ALTER_ZERO_CHECKPOINTS=0 ALTER_ZERO_AGENTS_DIR=$SMOKE_AGENTS ALTER_ZERO_HISTORY_FILE=/dev/null ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN"
tmux kill-session -t "$S102" 2>/dev/null
tmux new-session -d -s "$S102" -x 100 -y 24 "$APP_SK"
sleep 1.2
# The seed runs before the walk, so the session that installed it can use it —
# a skill that appeared only on the *second* launch would be missing from
# exactly the session that just installed the app.
tmux send-keys -t "$S102" -l "/skills"
sleep 0.3
tmux send-keys -t "$S102" Enter
sleep 0.7
skillseed_menu="$(tmux capture-pane -t "$S102" -p)"
echo "==== Phase 102: /skills on the first launch ===="
printf '%s\n' "$skillseed_menu"
expect_has "$skillseed_menu" -F "skill-creator" "the first launch's /skills does not list the built-in"
tmux send-keys -t "$S102" Escape
sleep 0.3
submit "$S102" "/quit"
sleep 0.6
tmux kill-session -t "$S102" 2>/dev/null
echo "==== Phase 102: seeded skill directory ===="
ls -1 "$SK_CFG/skills/skill-creator" 2>&1
for seeded in SKILL.md reference.md; do
	if [ ! -f "$SK_CFG/skills/skill-creator/$seeded" ]; then
		fail "the first launch did not seed skill-creator/$seeded"
	fi
done
if ! grep -q "^name: skill-creator" "$SK_CFG/skills/skill-creator/SKILL.md" 2>/dev/null; then
	fail "the seeded SKILL.md has no name field"
fi
# Edit one file, delete the other, relaunch: the edit survives and the deletion
# is repaired — a seed that overwrote would discard the user's own copy on
# every restart.
printf '%s\n' "---" "name: skill-creator" "description: MINE-NOT-YOURS." "---" >"$SK_CFG/skills/skill-creator/SKILL.md"
rm -f "$SK_CFG/skills/skill-creator/reference.md"
tmux new-session -d -s "$S102" -x 100 -y 24 "$APP_SK"
sleep 1.2
submit "$S102" "/quit"
sleep 0.6
tmux kill-session -t "$S102" 2>/dev/null
if ! grep -q "MINE-NOT-YOURS" "$SK_CFG/skills/skill-creator/SKILL.md" 2>/dev/null; then
	fail "the relaunch clobbered an edited SKILL.md"
fi
if [ ! -f "$SK_CFG/skills/skill-creator/reference.md" ]; then
	fail "a deleted built-in file did not come back on the next launch"
fi
# And the override is left alone: an explicit skills root is the user's set,
# and every other phase in this suite runs against one that must stay empty.
SK_OVERRIDE="$(mktemp -d)"
SK_CFG2="$(mktemp -d)"
tmux new-session -d -s "$S102" -x 100 -y 24 "env ALTER_ZERO_TELEMETRY=0 ALTER_ZERO_PROJECT_CONFIG=0 HOME=$SK_HOME ALTER_ZERO_CONFIG_DIR=$SK_CFG2 ALTER_ZERO_CHECKPOINTS=0 ALTER_ZERO_SKILLS_DIR=$SK_OVERRIDE ALTER_ZERO_AGENTS_DIR=$SMOKE_AGENTS ALTER_ZERO_HISTORY_FILE=/dev/null ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN"
sleep 1.2
submit "$S102" "/quit"
sleep 0.6
tmux kill-session -t "$S102" 2>/dev/null
if [ -n "$(ls -A "$SK_OVERRIDE" 2>/dev/null)" ]; then
	fail "the seed wrote into an ALTER_ZERO_SKILLS_DIR override"
	ls -1 "$SK_OVERRIDE" >&2
fi
if [ -e "$SK_CFG2/skills" ]; then
	fail "the override run seeded the config home behind its back"
fi
rm -rf "$SK_CFG" "$SK_CFG2" "$SK_HOME" "$SK_OVERRIDE"
echo "==== Phase 102: the built-in skill seeds once, survives an edit, comes back if deleted, and never touches an override root ===="
