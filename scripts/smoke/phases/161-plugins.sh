#!/usr/bin/env bash
# Phase 161 — the `/plugins` page: a marketplace added, plugins installed, reviewed and used

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# The `/plugins` page (docs/plugins.md), end to end against a REAL local
# marketplace this phase plants — a directory, read in place, so nothing is
# cloned: open the page from the palette on an empty config home (it lands
# on Marketplaces, whose add row is the way in), add the marketplace through
# the field, install `hello` (a skill and a command) from its page, install
# `guard` (a hook) and answer the review that opens for it, then prove the
# plugin's skill is live — its `$hello:` mention completes in the composer —
# that the install PERSISTED for a second process, and that the `plugin`
# subcommand reads the same store.
S161="${S}_plugins"
PL_CFG="$(mktemp -d)"
PL_SKILLS="$(mktemp -d)"
PL_MARKET="$(mktemp -d)"
mkdir -p "$PL_MARKET/.claude-plugin" "$PL_MARKET/plugins/hello/.claude-plugin" \
	"$PL_MARKET/plugins/hello/skills/greet" "$PL_MARKET/plugins/hello/commands" \
	"$PL_MARKET/plugins/guard/hooks"
cat >"$PL_MARKET/.claude-plugin/marketplace.json" <<'JSON'
{ "name": "demo", "owner": { "name": "Demo" }, "plugins": [
  { "name": "hello", "source": "./plugins/hello", "description": "Greets people in many languages" },
  { "name": "guard", "source": "./plugins/guard", "description": "Refuses dangerous shell commands" } ] }
JSON
cat >"$PL_MARKET/plugins/hello/.claude-plugin/plugin.json" <<'JSON'
{ "name": "hello", "version": "1.2.0" }
JSON
cat >"$PL_MARKET/plugins/hello/skills/greet/SKILL.md" <<'SKILL'
---
name: greet
description: Greet the user in the language they ask for
---
Say hello.
SKILL
printf 'Say hi.\n' >"$PL_MARKET/plugins/hello/commands/hi.md"
cat >"$PL_MARKET/plugins/guard/hooks/hooks.json" <<'JSON'
{ "hooks": { "Stop": [ { "hooks": [ { "type": "command", "command": "true" } ] } ] } }
JSON
PL_ENV="ALTER_ZERO_TELEMETRY=0 ALTER_ZERO_UPDATE_CHECK=0 ALTER_ZERO_TIPS=0 ALTER_ZERO_PROJECT_CONFIG=0 ALTER_ZERO_CONFIG_DIR=$PL_CFG ALTER_ZERO_CHECKPOINTS=0 ALTER_ZERO_HISTORY_FILE=/dev/null ALTER_ZERO_SKILLS_DIR=$PL_SKILLS ALTER_ZERO_MASCOT_ANIMATION=0"
APP_PL="env $PL_ENV ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN"
launch "$S161" 100 40 "$APP_PL"

# The palette lists it.
tmux send-keys -t "$S161" -l "/plugi"
sleep 0.4
pl_palette="$(tmux capture-pane -t "$S161" -p)"
expect_has "$pl_palette" -F "Browse, install and manage plugins" "/plugins is missing from the slash-command palette"
tmux send-keys -t "$S161" Enter
sleep 1
pl_empty="$(tmux capture-pane -t "$S161" -p)"
echo "==== Phase 161: the page on an empty config home ===="
printf '%s\n' "$pl_empty"
expect_has "$pl_empty" -E "^  Plugins *$" "the page does not open on its title"
expect_has "$pl_empty" -F "0 installed · no marketplace added" "the header does not count what there is"
expect_has "$pl_empty" -F " Installed (0)    Discover (0)    Marketplaces (0)" "the tab strip is missing"
expect_has "$pl_empty" -E "^  → \+ Add a marketplace" "an empty page does not land on the add row"

# Add the marketplace through the field.
tmux send-keys -t "$S161" Enter
sleep 0.4
expect_has "$(tmux capture-pane -t "$S161" -p)" -F "Add a marketplace" "Enter on the add row did not open the field"
tmux send-keys -t "$S161" -l "$PL_MARKET"
tmux send-keys -t "$S161" Enter
sleep 1.2
pl_added="$(tmux capture-pane -t "$S161" -p)"
echo "==== Phase 161: the marketplace added ===="
printf '%s\n' "$pl_added"
expect_has "$pl_added" -F "Added demo — 2 plugins" "the add was not confirmed with a toast"
expect_has "$pl_added" -E "^  ❯ demo$" "the page did not land on the new marketplace's plugins"
expect_has "$pl_added" -E "^  → guard +\+ available +Refuses dangerous shell commands" "guard is not listed as available"
expect_has "$pl_added" -E "^    hello +\+ available +Greets people" "hello is not listed as available"

# Install hello from its page.
tmux send-keys -t "$S161" Down
sleep 0.2
tmux send-keys -t "$S161" Enter
sleep 0.5
pl_hello="$(tmux capture-pane -t "$S161" -p)"
echo "==== Phase 161: hello's page ===="
printf '%s\n' "$pl_hello"
expect_has "$pl_hello" -E "Status: +\+ not installed" "hello's page does not say it is not installed"
expect_has "$pl_hello" -E "Skills: +greet" "hello's page does not name its skill"
expect_has "$pl_hello" -E "Commands: +hi" "hello's page does not name its command"
expect_has "$pl_hello" -F "❯ 1. Install" "hello's page does not offer the install"
tmux send-keys -t "$S161" Enter
sleep 1.2
pl_installed="$(tmux capture-pane -t "$S161" -p)"
echo "==== Phase 161: hello installed ===="
printf '%s\n' "$pl_installed"
expect_has "$pl_installed" -F "Installed hello" "the install was not confirmed with a toast"
expect_has "$pl_installed" -E "Status: +✔ enabled in this directory" "hello is not enabled once installed"
expect_has "$pl_installed" -F "❯ 1. Disable in this directory" "an installed plugin's page does not offer to turn it off"

# Install guard: it brings a hook, so the review asks before it runs.
tmux send-keys -t "$S161" Escape
sleep 0.3
tmux send-keys -t "$S161" Up
sleep 0.2
tmux send-keys -t "$S161" Enter
sleep 0.4
tmux send-keys -t "$S161" Enter
sleep 1.2
pl_review="$(tmux capture-pane -t "$S161" -p)"
echo "==== Phase 161: the review over guard's hook ===="
printf '%s\n' "$pl_review"
expect_has "$pl_review" -F "1 new hook found" "installing a plugin with a hook did not open the review"
expect_has "$pl_review" -E "Plugin · .*guard/hooks/hooks.json" "the review does not name the plugin's hooks file"
tmux send-keys -t "$S161" Enter
sleep 0.8
pl_trusted="$(tmux capture-pane -t "$S161" -p)"
expect_has "$pl_trusted" -E "Status: +✔ enabled in this directory" "the review did not hand back to guard's page"
expect_has "$pl_trusted" -E "Hooks: +Stop" "guard's page does not name its hook's event"

# Close the page; the plugin's skill is live: its mention completes.
for _ in 1 2 3; do
	tmux send-keys -t "$S161" Escape
	sleep 0.3
done
tmux send-keys -t "$S161" -l '$hello:g'
sleep 0.6
pl_mention="$(tmux capture-pane -t "$S161" -p)"
echo "==== Phase 161: the plugin's skill in the \$ picker ===="
printf '%s\n' "$pl_mention"
expect_has "$pl_mention" -E "→ hello:greet +Greet the user" "the plugin's skill is not offered to a \$ mention"
tmux kill-session -t "$S161" 2>/dev/null

echo "==== Phase 161: plugins.json on disk ===="
cat "$PL_CFG/plugins.json" 2>/dev/null || echo "(no file)"
for key in '"demo"' '"hello@demo"' '"guard@demo"'; do
	grep -qF "$key" "$PL_CFG/plugins.json" 2>/dev/null || fail "plugins.json does not record $key"
done
[ -f "$PL_CFG/plugins/installed/demo/hello/skills/greet/SKILL.md" ] || fail "hello's copy is not under plugins/installed"

# A second process against the same config home opens on what is installed.
launch "$S161" 100 40 "$APP_PL"
tmux send-keys -t "$S161" -l "/plugins"
sleep 0.3
tmux send-keys -t "$S161" Enter
sleep 1
pl_again="$(tmux capture-pane -t "$S161" -p)"
echo "==== Phase 161: a fresh process ===="
printf '%s\n' "$pl_again"
expect_has "$pl_again" -F "2 installed · 2 plugins in 1 marketplace" "the installs did not survive a restart"
expect_has "$pl_again" -E "^  → guard +✔ enabled" "a fresh process does not open on the installed plugins"
# Space turns one off for this directory, at once.
tmux send-keys -t "$S161" " "
sleep 0.8
pl_off="$(tmux capture-pane -t "$S161" -p)"
expect_has "$pl_off" -E "^  → guard +◯ disabled" "Space did not turn the highlighted plugin off"
expect_has "$pl_off" -F "Disabled guard in this directory" "the toggle was not confirmed with a toast"
tmux kill-session -t "$S161" 2>/dev/null

# The subcommand reads the same store.
pl_cli="$(cd "$PL_SKILLS" && env $PL_ENV "$BIN_ABS" plugin list 2>&1)"
echo "==== Phase 161: alter-zero plugin list ===="
printf '%s\n' "$pl_cli"
expect_has "$pl_cli" -E "^hello@demo +v1.2.0 +enabled$" "plugin list does not show hello installed"
rm -rf "$PL_CFG" "$PL_SKILLS" "$PL_MARKET"
