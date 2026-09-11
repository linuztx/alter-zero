#!/usr/bin/env bash
# Phase 100 — the built-in AGENT DEFINITIONS are seeded as real, editable files

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# the built-in AGENT DEFINITIONS are seeded as real, editable
# files (docs/subagents.md). `general-purpose` and `explore` are `agents/*.md`
# under the user config home now, not a `match` in the binary — so the first
# launch has to WRITE them (with the comments that teach the `tools:`/`model:`
# keys), a later launch must never clobber one the user edited (seeding that
# overwrote would silently discard their own agent on every restart), and a
# deleted default must come back, since the `agent` schema's default type has
# to resolve. Boundary I/O, so it is checked here rather than in a unit test.
S100="${S}_agentseed"
AG_CFG="$(mktemp -d)"
# The seed lands in the config home's `agents/`; plant a file that will not
# parse there first, so the same launch has to seed AND report.
mkdir -p "$AG_CFG/agents"
printf 'just a body, no frontmatter at all\n' >"$AG_CFG/agents/broken.md"
APP_AG="env ALTER_ZERO_TELEMETRY=0 ALTER_ZERO_PROJECT_CONFIG=0 ALTER_ZERO_CONFIG_DIR=$AG_CFG ALTER_ZERO_CHECKPOINTS=0 ALTER_ZERO_SKILLS_DIR=$SMOKE_SKILLS ALTER_ZERO_HISTORY_FILE=/dev/null ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN"
tmux kill-session -t "$S100" 2>/dev/null
tmux new-session -d -s "$S100" -x 100 -y 24 "$APP_AG"
sleep 1.2
# A definition that will not parse says so, and says why: silence here is what
# makes "the model says my agent type is unknown" and "I typo'd the
# frontmatter" read as two unrelated problems.
agent_toast="$(tmux capture-pane -t "$S100" -p)"
echo "==== Phase 100: the startup toast names the definition that would not parse ===="
printf '%s\n' "$agent_toast"
expect_has "$agent_toast" -F "broken.md" "a definition that will not parse said nothing at startup"
expect_has "$agent_toast" -F "frontmatter" "the toast does not say WHY the definition was refused"
submit "$S100" "/quit"
sleep 0.6
echo "==== Phase 100: seeded agent definitions ===="
ls -1 "$AG_CFG/agents" 2>&1
for seeded in general-purpose explore; do
	if [ ! -f "$AG_CFG/agents/$seeded.md" ]; then
		fail "the first launch did not seed $seeded.md"
	fi
done
# The frontmatter the model reads, and the comments the *user* reads: a file
# that documents neither key is a file nobody can edit with confidence.
if ! grep -q "^name: general-purpose" "$AG_CFG/agents/general-purpose.md" 2>/dev/null; then
	fail "the seeded general-purpose.md has no name field"
fi
if ! grep -q "^# \`tools:\`" "$AG_CFG/agents/general-purpose.md" 2>/dev/null; then
	fail "the seeded default does not document the tools key"
fi
# explore is the read-only one: its allowlist leaves Write and Edit out.
if ! grep -q "^tools: Bash, Read, Skill, mcp__\*" "$AG_CFG/agents/explore.md" 2>/dev/null; then
	fail "explore.md is not the read-only allowlist"
fi

if ! grep -qF "no frontmatter at all" "$AG_CFG/agents/broken.md" 2>/dev/null; then
	fail "the seed overwrote a file that was already there"
fi
# Edit one, delete the other, relaunch: the edit survives and the deletion is
# repaired.
printf '%s\n' "---" "name: explore" "description: MINE-NOT-YOURS." "---" >"$AG_CFG/agents/explore.md"
rm -f "$AG_CFG/agents/general-purpose.md"
tmux kill-session -t "$S100" 2>/dev/null
tmux new-session -d -s "$S100" -x 100 -y 24 "$APP_AG"
sleep 1.2
submit "$S100" "/quit"
sleep 0.6
tmux kill-session -t "$S100" 2>/dev/null
if ! grep -q "MINE-NOT-YOURS" "$AG_CFG/agents/explore.md" 2>/dev/null; then
	fail "the relaunch clobbered an edited agent definition"
fi
if [ ! -f "$AG_CFG/agents/general-purpose.md" ]; then
	fail "a deleted default was not re-seeded on the next launch"
fi
rm -rf "$AG_CFG"
echo "==== Phase 100: the built-in agent definitions seed once, survive an edit, come back if deleted, and a broken one is reported ===="
