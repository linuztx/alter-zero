#!/usr/bin/env bash
# Phase 161 — the `/plugins` page: a marketplace added, plugins installed, reviewed and used

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# The `/plugins` page (docs/plugins.md), end to end against a REAL local
# marketplace this phase plants — a directory, whose plugins load in place,
# so nothing is cloned or copied: open the page from the palette on an
# empty config home (it lands on the + Add tab, the official catalogs one
# Enter away over the row that adds any other), add the marketplace by
# typing its folder into that row's field — the page lands on its tab and
# says nothing more — install `hello` (a skill and a command) from its page,
# install `guard` (a hook and an MCP server) and answer the review that
# opens for it — no line saying either went right: the page shows it — then
# prove the plugin's
# server runs under Claude Code's scoped name — `/mcp` shows its tool as
# `mcp__plugin_guard_probe__…` — and its skill is live (its `$hello:`
# mention completes), that the install PERSISTED for a second process, that
# neither moving through the list (guard's description is three rows long,
# hello's one) nor toggling a plugin MOVES THE PAGE (the flicker: no clear,
# no toast row over the page, the bottom rule on the same row throughout),
# and that the `plugin` subcommand reads the same store.
S161="${S}_plugins"
PL_CFG="$(mktemp -d)"
PL_SKILLS="$(mktemp -d)"
PL_MARKET="$(mktemp -d)"
PL_RAW="$(mktemp)"
mkdir -p "$PL_MARKET/.claude-plugin" "$PL_MARKET/plugins/hello/.claude-plugin" \
	"$PL_MARKET/plugins/hello/skills/greet" "$PL_MARKET/plugins/hello/commands" \
	"$PL_MARKET/plugins/guard/hooks"
cat >"$PL_MARKET/.claude-plugin/marketplace.json" <<'JSON'
{ "name": "demo", "owner": { "name": "Demo" }, "plugins": [
  { "name": "hello", "source": "./plugins/hello", "description": "Greets people in many languages" },
  { "name": "guard", "source": "./plugins/guard", "description": "Refuses dangerous shell commands before they run: rm -rf on the home directory, force pushes to a main branch, and anything that pipes a download straight into a shell. The commands it refuses are listed in its hook, ready to change." } ] }
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
{ "hooks": { "Stop": [ { "hooks": [ { "type": "command", "command": "${CLAUDE_PLUGIN_ROOT}/hooks/stop.sh" } ] } ] } }
JSON
# A scripted stdio MCP server (Phase 80's): 1 = the discover probe, 2 =
# tools/list.
cat >"$PL_MARKET/plugins/guard/server.sh" <<'MCPSRV'
#!/bin/sh
cat > /dev/null &
printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"resultType":"complete","supportedVersions":["2026-07-28"],"capabilities":{"tools":{}},"ttlMs":60000,"cacheScope":"public","_meta":{"io.modelcontextprotocol/serverInfo":{"name":"probe","version":"1.0"}}}}'
printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"resultType":"complete","tools":[{"name":"echo_text","description":"Echo the text back.","inputSchema":{"type":"object","properties":{"text":{"type":"string"}},"required":["text"]}}],"ttlMs":60000,"cacheScope":"public"}}'
sleep 60
MCPSRV
cat >"$PL_MARKET/plugins/guard/.mcp.json" <<'JSON'
{ "mcpServers": { "probe": { "command": "sh", "args": ["${CLAUDE_PLUGIN_ROOT}/server.sh"] } } }
JSON
PL_ENV="ALTER_ZERO_TELEMETRY=0 ALTER_ZERO_UPDATE_CHECK=0 ALTER_ZERO_TIPS=0 ALTER_ZERO_PROJECT_CONFIG=0 ALTER_ZERO_CONFIG_DIR=$PL_CFG ALTER_ZERO_CHECKPOINTS=0 ALTER_ZERO_HISTORY_FILE=/dev/null ALTER_ZERO_SKILLS_DIR=$PL_SKILLS ALTER_ZERO_MASCOT_ANIMATION=0"
APP_PL="env $PL_ENV ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN"
launch "$S161" 100 40 "$APP_PL"

# The palette lists it.
type_text "$S161" "/plugi"
sleep 0.4
expect_has "$(pane "$S161")" -F "Browse, install and manage plugins" "/plugins is missing from the slash-command palette"
keys "$S161" Enter
pl_empty="$(wait_pane 5 "$S161" -F "anthropics/claude-plugins-official")"
dump "the page on an empty config home" "$pl_empty"
expect_has "$pl_empty" -E "^  Plugins *$" "the page does not open on its title"
expect_has "$pl_empty" -F " All (0)    Installed (0)    + Add" "the tab strip is not All / Installed / + Add"
expect_has "$pl_empty" -E "^  → anthropics/claude-plugins-official +Claude Code's official plugin directory" "an empty page does not offer Claude Code's catalog first"
expect_has "$pl_empty" -E "^    openai/plugins +Codex's official plugin directory" "an empty page does not offer Codex's catalog"
expect_has "$pl_empty" -E "^    Add a marketplace… +owner/repo, git URL or path *$" "an empty page offers no way to add any other marketplace"

# Add the marketplace by typing its folder into the add row's field: the
# page lands on its tab, and says nothing more.
keys "$S161" End
sleep 0.2
keys "$S161" Enter
pl_field="$(wait_pane 3 "$S161" -E "^  ❯ owner/repo *$")"
dump "the add field" "$pl_field"
expect_has "$pl_field" -E "^  Add a marketplace *$" "the add row did not open its field"
type_text "$S161" "$PL_MARKET"
sleep 0.3
keys "$S161" Enter
pl_added="$(wait_pane 10 "$S161" -F " demo (2) ")"
dump "the marketplace added" "$pl_added"
expect_has "$pl_added" -F " All (2)    Installed (0)    demo (2)    + Add" "the new marketplace has no tab"
expect_has "$pl_added" -E "^  → guard +\+ available +Refuses dangerous shell commands" "guard is not listed as available"
expect_has "$pl_added" -E "^    hello +\+ available +Greets people" "hello is not listed as available"
expect_has "$pl_added" -F "Ctrl+R remove · ←/→ tabs · Enter to open" "a marketplace's tab does not offer its own keys in the hint's row"
expect_lacks "$pl_added" -F "Added demo" "the page said what the add did"

# Install hello from its page: it loads in place.
keys "$S161" Down
sleep 0.2
keys "$S161" Enter
pl_hello="$(wait_pane 3 "$S161" -F "❯ 1. Install")"
dump "hello's page" "$pl_hello"
expect_has "$pl_hello" -E "^  \+ not installed · hello@demo · v1.2.0 *$" "hello's page does not lead with its state"
expect_has "$pl_hello" -E "Skills: +hello:greet" "hello's page does not name its skill"
keys "$S161" Enter
pl_installed="$(wait_pane 10 "$S161" -E "^  ✔ enabled in this directory · hello@demo")"
dump "hello installed" "$pl_installed"
expect_lacks "$pl_installed" -F "Installed hello" "the page said what the install did"
expect_has "$pl_installed" -E "Loads from: " "a directory's plugin does not say it loads in place"
expect_lacks "$pl_installed" -F "Update" "a plugin loaded in place offers an update"

# Install guard: its hook and its server wait for the review.
keys "$S161" Escape
sleep 0.3
keys "$S161" Up
sleep 0.2
keys "$S161" Enter
sleep 0.4
keys "$S161" Enter
pl_review="$(wait_pane 10 "$S161" -F "1 new hook found")"
dump "the review over guard's hook" "$pl_review"
expect_has "$pl_review" -E "Plugin guard@demo · " "the review does not name the plugin its hook comes from"
expect_has "$pl_review" -F "/plugins/guard/hooks/stop.sh" "the hook's command is not shown as it runs"
expect_lacks "$pl_review" -F '${CLAUDE_PLUGIN_ROOT}' "the hook's placeholder was not substituted"
keys "$S161" Enter
pl_servers="$(wait_pane 5 "$S161" -F "found in your plugins")"
dump "the review over guard's server" "$pl_servers"
expect_has "$pl_servers" -F "plugin:guard:probe · stdio" "the server is not under its scoped name"
keys "$S161" Enter
pl_trusted="$(wait_pane 5 "$S161" -E "✔ enabled in this directory · guard@demo")"
dump "guard's page after the review" "$pl_trusted"
expect_has "$pl_trusted" -E "Hooks: +Stop" "guard's page does not name its hook's event"
expect_has "$pl_trusted" -E "MCP servers: +plugin:guard:probe" "guard's page does not name its server"

# Out to the composer (two Escapes: the page, then the list).
keys "$S161" Escape
sleep 0.3
keys "$S161" Escape
wait_for 3 "$S161" -E "^❯ *$" || fail "the page did not close"

# The server runs under Claude Code's scoped name.
type_text "$S161" "/mcp"
sleep 0.3
keys "$S161" Enter
pl_mcp="$(wait_pane 15 "$S161" -F "plugin:guard:probe · ✔ connected")"
dump "/mcp" "$pl_mcp"
expect_has "$pl_mcp" -F "plugin:guard:probe · ✔ connected · 1 tool" "the plugin's server did not connect"
keys "$S161" Enter
sleep 0.4
keys "$S161" Enter
sleep 0.4
keys "$S161" Enter
pl_tool="$(wait_pane 3 "$S161" -F "Full name:")"
dump "the plugin server's tool" "$pl_tool"
expect_has "$pl_tool" -F "Full name: mcp__plugin_guard_probe__echo_text" "the tool is not named as Claude Code names it"
for _ in 1 2 3 4; do
	keys "$S161" Escape
	sleep 0.3
done
wait_for 3 "$S161" -E "^❯ *$" || fail "/mcp did not close"

# The plugin's skill is live: its mention completes.
type_text "$S161" '$hello:g'
sleep 0.6
pl_mention="$(pane "$S161")"
dump "the plugin's skill in the \$ picker" "$pl_mention"
expect_has "$pl_mention" -E "→ hello:greet +Greet the user" "the plugin's skill is not offered to a \$ mention"
tmux kill-session -t "$S161" 2>/dev/null

note "plugins.json on disk"
cat "$PL_CFG/plugins.json" 2>/dev/null || echo "(no file)"
for key in '"demo"' '"hello@demo"' '"guard@demo"'; do
	grep -qF "$key" "$PL_CFG/plugins.json" 2>/dev/null || fail "plugins.json does not record $key"
done
[ ! -e "$PL_CFG/plugins/installed/demo/hello" ] || fail "a directory's plugin was copied, not loaded in place"

# A second process opens on what is installed — at once, on the catalog
# read behind its first frame.
launch "$S161" 100 40 "$APP_PL"
type_text "$S161" "/plugins"
sleep 0.3
keys "$S161" Enter
pl_again="$(wait_pane 5 "$S161" -F "2 installed · 2 on in this directory")"
dump "a fresh process" "$pl_again"
expect_has "$pl_again" -E "^  → guard +✔ enabled" "a fresh process does not open on the installed plugins"
expect_lacks "$pl_again" -F "Reading the marketplaces" "the page did not open on the catalog it read at startup"

# Moving through the list never resizes the page: guard's long description
# is one row under the list, like hello's short one.
rule_row() { pane "$S161" | grep -nE '^(─)+$' | tail -1 | cut -d: -f1; }
before_rule="$(rule_row)"
keys "$S161" Down
wait_for 3 "$S161" -E "^  → hello " || fail "Down did not move to hello"
expect_eq "$(rule_row)" "$before_rule" "the page moved as the selection did"
keys "$S161" Up
wait_for 3 "$S161" -E "^  → guard " || fail "Up did not move back to guard"
expect_eq "$(rule_row)" "$before_rule" "the page moved as the selection did"

# Space turns one off — and the page never moves (the flicker): the rows
# the toggle changes are rewritten in place, no clear, no toast row over
# the page, the bottom rule on one row from the key to the end.
tmux pipe-pane -t "$S161" -o "cat >> $PL_RAW"
sleep 0.2
keys "$S161" " "
for _ in 1 2 3 4 5 6 7 8 9 10; do
	expect_eq "$(rule_row)" "$before_rule" "the page moved while the toggle ran"
	sleep 0.05
done
pl_off="$(wait_pane 3 "$S161" -F "2 installed · 1 on in this directory")"
sleep 0.5
tmux pipe-pane -t "$S161"
pl_off="$(pane "$S161")"
dump "after Space" "$pl_off"
expect_has "$pl_off" -E "^  → guard +◯ disabled" "Space did not turn the highlighted plugin off"
expect_has "$pl_off" -F "←/→ tabs · Enter to open · Space to toggle" "the hint left its row"
expect_lacks "$pl_off" -F "Disabled guard" "the page said what the toggle did"
expect_eq "$(rule_row)" "$before_rule" "the page moved once the toggle was done"
python3 - "$PL_RAW" <<'PY' || fail "the toggle repainted the screen"
import sys
data = open(sys.argv[1], 'rb').read()
for clear in (b'\x1b[2J', b'\x1b[3J', b'\x1b[J', b'\x1b[0J'):
    if clear in data:
        sys.exit(f"the toggle wrote {clear!r}")
if len(data) > 4000:
    sys.exit(f"the toggle wrote {len(data)} bytes — more than its rows")
print(f"the toggle wrote {len(data)} bytes, no clear")
PY
tmux kill-session -t "$S161" 2>/dev/null

# The subcommand reads the same store.
pl_cli="$(cd "$PL_SKILLS" && env $PL_ENV "$BIN_ABS" plugin list 2>&1)"
note "alter-zero plugin list"
printf '%s\n' "$pl_cli"
expect_has "$pl_cli" -E "^hello@demo +v1.2.0 +enabled$" "plugin list does not show hello installed"
rm -rf "$PL_CFG" "$PL_SKILLS" "$PL_MARKET" "$PL_RAW"
