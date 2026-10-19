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
expect_has "$skillseed_menu" -F "flaredantic" "the first launch's /skills does not list flaredantic"
expect_has "$skillseed_menu" -F "skill-creator" "the first launch's /skills does not list skill-creator"
expect_has "$skillseed_menu" -F "yt-dlp" "the first launch's /skills does not list yt-dlp"
expect_has "$skillseed_menu" -F "web-search" "the first launch's /skills does not list web-search"
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
if ! grep -q "^name: yt-dlp" "$SK_CFG/skills/yt-dlp/SKILL.md" 2>/dev/null; then
	fail "the first launch did not seed yt-dlp/SKILL.md"
fi
for seeded in SKILL.md reference.md; do
	if ! cmp -s "$SMOKE_ROOT/prompts/skills/flaredantic/$seeded" "$SK_CFG/skills/flaredantic/$seeded"; then
		fail "the first launch did not seed the authored flaredantic/$seeded"
	fi
done
# web-search's body runs the scripts seeded beside it, so they have to land
# too, and as programs that start from there: three of them import
# mcp_http.py from their own directory. `--help` touches no network.
SK_WEB="$SK_CFG/skills/web-search"
if ! grep -q "^name: web-search" "$SK_WEB/SKILL.md" 2>/dev/null; then
	fail "the first launch did not seed web-search/SKILL.md"
fi
for seeded in exa_search.py parallel_search.py tavily_search.py mcp_http.py jina_web_search.py; do
	if [ ! -f "$SK_WEB/scripts/$seeded" ]; then
		fail "the first launch did not seed web-search/scripts/$seeded"
	fi
done
if command -v python3 >/dev/null 2>&1; then
	# Run the way an agent would, bytecode writes allowed: importing
	# mcp_http.py must not leave a __pycache__ folder in the user's skill.
	for script in exa_search.py parallel_search.py tavily_search.py jina_web_search.py; do
		sk_help="$(env -u PYTHONDONTWRITEBYTECODE -u PYTHONPYCACHEPREFIX python3 "$SK_WEB/scripts/$script" --help 2>&1)" ||
			fail "the seeded web-search/scripts/$script --help failed: $sk_help"
		expect_has "$sk_help" -F "usage:" "the seeded web-search/scripts/$script --help printed no usage"
	done
	if [ -e "$SK_WEB/scripts/__pycache__" ]; then
		fail "running the seeded web-search scripts left a __pycache__ folder in the skill"
	fi
fi
# Edit one file, delete the other, relaunch: the edit survives and the deletion
# is repaired — a seed that overwrote would discard the user's own copy on
# every restart.
printf '%s\n' "---" "name: skill-creator" "description: MINE-NOT-YOURS." "---" >"$SK_CFG/skills/skill-creator/SKILL.md"
printf '%s\n' "---" "name: yt-dlp" "description: MY-MEDIA-WORKFLOW." "---" >"$SK_CFG/skills/yt-dlp/SKILL.md"
printf '%s\n' "---" "name: flaredantic" "description: MY-TUNNEL-WORKFLOW." "---" >"$SK_CFG/skills/flaredantic/SKILL.md"
rm -f "$SK_CFG/skills/flaredantic/reference.md"
rm -f "$SK_CFG/skills/skill-creator/reference.md"
rm -f "$SK_WEB/scripts/mcp_http.py"
tmux new-session -d -s "$S102" -x 100 -y 24 "$APP_SK"
sleep 1.2
submit "$S102" "/quit"
sleep 0.6
tmux kill-session -t "$S102" 2>/dev/null
if ! grep -q "MINE-NOT-YOURS" "$SK_CFG/skills/skill-creator/SKILL.md" 2>/dev/null; then
	fail "the relaunch clobbered an edited SKILL.md"
fi
if ! grep -q "MY-MEDIA-WORKFLOW" "$SK_CFG/skills/yt-dlp/SKILL.md" 2>/dev/null; then
	fail "the relaunch clobbered an edited yt-dlp/SKILL.md"
fi
if [ ! -f "$SK_CFG/skills/skill-creator/reference.md" ]; then
	fail "a deleted built-in file did not come back on the next launch"
fi
if ! grep -q "MY-TUNNEL-WORKFLOW" "$SK_CFG/skills/flaredantic/SKILL.md" 2>/dev/null; then
	fail "the relaunch clobbered an edited flaredantic/SKILL.md"
fi
if ! cmp -s "$SMOKE_ROOT/prompts/skills/flaredantic/reference.md" "$SK_CFG/skills/flaredantic/reference.md"; then
	fail "a deleted flaredantic/reference.md did not come back on the next launch"
fi
if [ ! -f "$SK_WEB/scripts/mcp_http.py" ]; then
	fail "a deleted web-search script did not come back on the next launch"
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
