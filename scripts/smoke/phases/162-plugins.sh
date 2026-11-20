#!/usr/bin/env bash
# Phase 162 — the `/plugins` page against marketplaces of its own

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# The `/plugins` page (docs/plugins.md) driven end to end against three
# marketplaces the phase builds in its own temp tree — never the network:
# `acme`, a Claude-format FOLDER marketplace (a plugin with a skill, a
# command and an agent; one with a hook and an MCP server; one whose npm
# source cannot be installed), `codex-demo`, a Codex-format marketplace in a
# GIT repository added by `file://` URL, and `extras`, a folder the Add page
# adds. The page must fetch the first two on its own, install, disable and
# uninstall with the result stated on the page and the record in
# `plugins.json`, add and remove a marketplace, reopen from disk with no
# fetch, and keep its search line, a row and its hint on a short terminal.
S162="${S}_plugins"
PL_CFG="$(mktemp -d)"
PL_ROOT="$(mktemp -d)"
A="$PL_ROOT/mk-a"
B="$PL_ROOT/mk-b"
C="$PL_ROOT/mk-c"

# --- mk-a: the Claude-format folder marketplace `acme` -------------------------
mkdir -p "$A/.claude-plugin" \
	"$A/plugins/formatter/.claude-plugin" "$A/plugins/formatter/skills/tidy" \
	"$A/plugins/formatter/commands" "$A/plugins/formatter/agents" \
	"$A/plugins/guard/.claude-plugin" "$A/plugins/guard/hooks"
cat >"$A/.claude-plugin/marketplace.json" <<'JSON'
{
  "name": "acme",
  "owner": { "name": "Acme" },
  "plugins": [
    { "name": "formatter", "source": "./plugins/formatter", "description": "Tidy code the house way", "version": "1.0.0", "category": "development" },
    { "name": "guard", "source": "./plugins/guard", "description": "Check commands before they run", "version": "0.3.0" },
    { "name": "blocked", "source": { "source": "npm", "package": "@acme/blocked" }, "description": "Ships through npm" }
  ]
}
JSON
cat >"$A/plugins/formatter/.claude-plugin/plugin.json" <<'JSON'
{ "name": "formatter", "version": "1.0.0", "description": "Tidy code the house way" }
JSON
cat >"$A/plugins/formatter/skills/tidy/SKILL.md" <<'MD'
---
name: tidy
description: Tidy a file the house way
---
Tidy it.
MD
cat >"$A/plugins/formatter/commands/format.md" <<'MD'
---
description: Format the files named
---
Format these files: $ARGUMENTS
MD
cat >"$A/plugins/formatter/agents/reviewer.md" <<'MD'
---
name: reviewer
description: Reviews formatting
---
You review formatting.
MD
cat >"$A/plugins/guard/.claude-plugin/plugin.json" <<'JSON'
{ "name": "guard", "version": "0.3.0", "description": "Check commands before they run" }
JSON
cat >"$A/plugins/guard/hooks/hooks.json" <<'JSON'
{ "hooks": { "PreToolUse": [ { "matcher": "Bash", "hooks": [ { "type": "command", "command": "${CLAUDE_PLUGIN_ROOT}/check.sh" } ] } ] } }
JSON
cat >"$A/plugins/guard/.mcp.json" <<'JSON'
{ "mcpServers": { "ledger": { "command": "node", "args": ["server.js"] } } }
JSON

# --- mk-b: the Codex-format git marketplace `codex-demo` -----------------------
mkdir -p "$B/.agents/plugins" "$B/plugins/notes/.codex-plugin" "$B/plugins/notes/skills/jot" \
	"$B/plugins/tasks/.codex-plugin" "$B/plugins/tasks/skills/plan"
cat >"$B/.agents/plugins/marketplace.json" <<'JSON'
{
  "name": "codex-demo",
  "plugins": [
    { "name": "notes", "source": { "source": "local", "path": "./plugins/notes" } },
    { "name": "tasks", "source": { "source": "local", "path": "./plugins/tasks" } }
  ]
}
JSON
for p in notes tasks; do
	cat >"$B/plugins/$p/.codex-plugin/plugin.json" <<JSON
{ "name": "$p", "version": "2.0.0", "interface": { "displayName": "Demo ${p^}", "shortDescription": "The $p demo plugin" } }
JSON
done
printf -- '---\nname: jot\ndescription: Jot a note\n---\nJot.\n' >"$B/plugins/notes/skills/jot/SKILL.md"
printf -- '---\nname: plan\ndescription: Plan a task\n---\nPlan.\n' >"$B/plugins/tasks/skills/plan/SKILL.md"
git -C "$B" init -q
git -C "$B" add -A
git -C "$B" -c user.name=smoke -c user.email=smoke@example.invalid commit -qm "codex demo"

# --- mk-c: the folder marketplace `extras`, for the Add page --------------------
mkdir -p "$C/.claude-plugin" "$C/plugins/hello/.claude-plugin" "$C/plugins/hello/commands"
cat >"$C/.claude-plugin/marketplace.json" <<'JSON'
{ "name": "extras", "plugins": [ { "name": "hello", "source": "./plugins/hello", "description": "Say hello" } ] }
JSON
printf '{ "name": "hello", "version": "0.1.0" }\n' >"$C/plugins/hello/.claude-plugin/plugin.json"
printf 'Say hello to $ARGUMENTS\n' >"$C/plugins/hello/commands/hi.md"

APP_PL="env $CFG_ENV_NOHIST ALTER_ZERO_CONFIG_DIR=$PL_CFG ALTER_ZERO_PLUGIN_MARKETPLACES='$A file://$B' ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN"
STATE="$PL_CFG/plugins.json"

launch "$S162" 100 34 "$APP_PL"
type_text "$S162" "/plug"
pl_palette="$(wait_pane 5 "$S162" -F "Browse, install and manage plugins")"
dump "the palette filtered to /plugins" "$pl_palette"
expect_has "$pl_palette" -F "/plugins" "/plugins is missing from the slash-command palette"

# Both marketplaces are fetched without a key press.
keys "$S162" Enter
pl_open="$(wait_pane 15 "$S162" -F "codex-demo 2")"
dump "the page, both marketplaces fetched" "$pl_open"
expect_has "$pl_open" -E "^  Plugins *$" "the page does not open on its title"
expect_has "$pl_open" -F "acme 3" "the acme tab does not count its three plugins"
expect_has "$pl_open" -F "2 marketplaces · 5 plugins · 0 installed" "the subtitle does not count what is listed"
expect_has "$pl_open" -F "(1/5)" "the counter does not count the five plugins"

# Typing filters; Esc clears.
type_text "$S162" "form"
pl_search="$(wait_pane 5 "$S162" -F "(1/1)")"
dump "typed 'form'" "$pl_search"
expect_has "$pl_search" -E "^  →   formatter " "the search does not leave formatter highlighted"
expect_lacks "$pl_search" -F "guard" "the search did not filter guard out"
keys "$S162" Escape
pl_cleared="$(wait_pane 5 "$S162" -F "(1/5)")"
expect_has "$pl_cleared" -E '^  ❯ *$' "Esc did not clear the search"

# →: the Installed tab, empty; ←: back to All.
keys "$S162" Right
pl_installed="$(wait_pane 5 "$S162" -F "Nothing installed yet")"
dump "→ the Installed tab" "$pl_installed"
expect_has "$pl_installed" -F "Nothing installed yet" "→ did not move the chip onto the empty Installed tab"
keys "$S162" Left
wait_for 5 "$S162" -F "(1/5)" || fail "← did not come back to All"

# formatter's page names its skill, command and agent; Install states it.
type_text "$S162" "formatter"
wait_for 5 "$S162" -F "(1/1)"
keys "$S162" Enter
# The title comes at once, the detail a moment later: wait for the detail.
pl_formatter="$(wait_pane 5 "$S162" -F "formatter:reviewer")"
dump "formatter's page" "$pl_formatter"
expect_has "$pl_formatter" -F "formatter:tidy" "the page does not name formatter's skill"
expect_has "$pl_formatter" -F "/formatter:format" "the page does not name formatter's command"
expect_has "$pl_formatter" -F "formatter:reviewer" "the page does not name formatter's agent"
expect_has "$pl_formatter" -E "❯ 1\. Install" "Install is not the first action"
keys "$S162" Enter
pl_installed_ok="$(wait_pane 10 "$S162" -F "Installed formatter")"
dump "formatter installed" "$pl_installed_ok"
expect_has "$pl_installed_ok" -F "ready for your next message" "the result does not say when it takes effect"
expect_has "$pl_installed_ok" -F "✔ installed · enabled" "the status line did not turn installed"
wait_file 5 "$STATE" -F '"formatter@acme"' || fail "plugins.json does not record formatter@acme"
dump "plugins.json after the install" "$(cat "$STATE" 2>/dev/null)"
expect_file_has "$STATE" -F '"enabled": true' "formatter@acme is not recorded enabled"
expect_file_has "$STATE" -F '"skills"' "formatter@acme's component paths are not recorded"

# Esc: back to Browse, the row marked; Space disables it.
keys "$S162" Escape
pl_back="$(wait_pane 5 "$S162" -E "^  → ✔ formatter")"
dump "back on Browse" "$pl_back"
expect_has "$pl_back" -E "^  → ✔ formatter" "the installed row is not marked ✔"
keys "$S162" Space
pl_off="$(wait_pane 5 "$S162" -E "^  → ◯ formatter")"
dump "Space disabled it" "$pl_off"
expect_has "$pl_off" -E "^  → ◯ formatter" "Space did not flip the row to ◯"
wait_file 5 "$STATE" -F '"enabled": false' || fail "plugins.json does not record formatter@acme disabled"

# guard: the hook and the server wait for the review.
keys "$S162" Escape
wait_for 5 "$S162" -F "(1/5)"
type_text "$S162" "guard"
wait_for 5 "$S162" -F "(1/1)"
keys "$S162" Enter
pl_guard="$(wait_pane 5 "$S162" -F "ledger (stdio)")"
dump "guard's page" "$pl_guard"
expect_has "$pl_guard" -E "^  Hooks .*asks for your review at the next launch" "the Hooks row does not say it asks for review"
expect_has "$pl_guard" -E "^  MCP .*asks for your review at the next launch" "the MCP row does not say it asks for review"
keys "$S162" Enter
pl_guard_ok="$(wait_pane 10 "$S162" -F "Installed guard")"
dump "guard installed" "$pl_guard_ok"
expect_has "$pl_guard_ok" -F "for your review at the next launch." "the install result does not carry the review sentence"

# Uninstall asks once, then removes it.
keys "$S162" 3
wait_for 5 "$S162" -F "press Enter again to remove it" || fail "Uninstall did not ask once"
keys "$S162" Enter
pl_gone="$(wait_pane 10 "$S162" -F "Uninstalled guard")"
dump "guard uninstalled" "$pl_gone"
expect_has "$pl_gone" -E "^  Plugins *$" "Uninstall did not return to Browse"
expect_file_lacks "$STATE" -F '"guard@acme"' "plugins.json still records guard@acme"

# blocked: why, and only Back.
keys "$S162" Escape
wait_for 5 "$S162" -F "(1/5)"
type_text "$S162" "blocked"
wait_for 5 "$S162" -F "(1/1)"
keys "$S162" Enter
pl_blocked="$(wait_pane 5 "$S162" -F "Can't install")"
dump "blocked's page" "$pl_blocked"
expect_has "$pl_blocked" -F "❯ 1. Back" "a plugin that cannot be installed offers more than Back"
expect_lacks "$pl_blocked" -F "2. " "a plugin that cannot be installed offers a second action"
# Two Esc keys sent at once can read as one Alt+Esc: wait between them.
keys "$S162" Escape
wait_for 5 "$S162" -F "(1/1)" || fail "Esc did not go back from blocked's page"
keys "$S162" Escape
wait_for 5 "$S162" -F "(1/5)" || fail "Esc did not clear the search"

# The Marketplaces tab: both, then Add — `extras` lands on its tab.
keys "$S162" Left
pl_markets="$(wait_pane 5 "$S162" -F "+ Add a marketplace")"
dump "the Marketplaces tab" "$pl_markets"
expect_has "$pl_markets" -E "^  → acme " "the Marketplaces tab does not list acme first"
expect_has "$pl_markets" -F "codex-demo" "the Marketplaces tab does not list codex-demo"
keys "$S162" End
keys "$S162" Enter
wait_for 5 "$S162" -F "Plugins › Add a marketplace" || fail "+ Add a marketplace did not open the field"
type_text "$S162" "$C"
keys "$S162" Enter
pl_added="$(wait_pane 10 "$S162" -F "Added extras")"
dump "extras added" "$pl_added"
expect_has "$pl_added" -F "Added extras — 1 plugin. Pick one and press Enter." "the Add result is not stated"
expect_has "$pl_added" -F "Say hello" "the page did not land on the new marketplace's tab"

# Remove it: asked once, then gone.
keys "$S162" Right
wait_for 5 "$S162" -F "+ Add a marketplace"
keys "$S162" Home Down Down
keys "$S162" Enter
wait_for 5 "$S162" -F "Plugins › extras" || fail "Enter did not open the extras page"
keys "$S162" 3
wait_for 5 "$S162" -F "press Enter again" || fail "Remove did not ask once"
keys "$S162" Enter
pl_removed="$(wait_pane 10 "$S162" -F "Removed extras")"
dump "extras removed" "$pl_removed"
expect_lacks "$pl_removed" -E "^ .*extras 1" "the extras tab is still in the strip"
keys "$S162" C-c
tmux kill-session -t "$S162" 2>/dev/null

# A second launch reopens from disk, at once, with the same counts.
launch "$S162" 100 34 "$APP_PL"
type_text "$S162" "/plugins"
keys "$S162" Enter
pl_again="$(wait_pane 5 "$S162" -F "codex-demo 2")"
dump "the page reopened from disk" "$pl_again"
expect_has "$pl_again" -F "acme 3" "the reopened page lost acme's count"
expect_lacks "$pl_again" -F "Fetching" "the reopened page fetched again"
expect_has "$pl_again" -F "1 installed" "the reopened page lost the install"

# 14 rows: the search line, a selected row and the hint, and no flow.
tmux resize-window -t "$S162" -x 100 -y 14
sleep 0.6
pl_short="$(pane "$S162")"
dump "a 14-row terminal" "$pl_short"
expect_has "$pl_short" -E '^  ❯ *$' "the short page lost its search line"
expect_has "$pl_short" -E "^  → " "the short page lost its selected row"
expect_has "$pl_short" -F "Esc to close" "the short page lost its hint"
if [ "$(count_rules "$pl_short")" -gt 2 ]; then
	fail "the short page drew more than one frame (it flowed)"
fi
keys "$S162" C-c
tmux kill-session -t "$S162" 2>/dev/null
smoke_finish
