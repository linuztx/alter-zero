#!/usr/bin/env bash
# Phase 163 — the `/plugins` page on the real store: add, install, review, toggle, uninstall, remove, cancel

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# The `/plugins` page's boundary (docs/plugins-menu.md *The boundary*),
# offline: a git marketplace in a local repository, offered through
# ALTER_ZERO_PLUGIN_MARKETPLACES so the `+ Add` tab of an empty install
# offers it, and a folder marketplace typed into the add field. Asserted:
# the offer and its add with Enter; `hello`'s page previews its contents
# from the clone's tree; installing it (a skill, a hook, an MCP server)
# opens the review over the page, and nothing of the plugin runs before it
# is answered; answering returns to the plugin's page, now showing the
# installed copy; the skill is in the `$` picker; Space turns the plugin off
# here and on again; a failure is said in red on the hint's row and the page
# does not move; the folder marketplace's plugin installs, and removing the
# marketplace takes it with it; Esc cancels a fetch in flight (a `git` on
# PATH that sleeps) and kills it; uninstall; a reopened page shows the same
# state; and `alter-zero plugin list` agrees with it.
S163="${S}_plugpage"
PP_CFG="$SMOKE_TMP/ppcfg"
PP_REPO="$SMOKE_TMP/pprepo"
PP_FOLDER="$SMOKE_TMP/ppfolder"
PP_BIN="$SMOKE_TMP/ppbin"
PP_WORK="$(readlink -f "$(work_dir ppwork)")"
mkdir -p "$PP_CFG" "$PP_BIN"

# The git marketplace `acme`: `hello` brings a skill, a hook and a server.
mkdir -p "$PP_REPO/.claude-plugin" "$PP_REPO/plugins/hello/.claude-plugin" \
	"$PP_REPO/plugins/hello/skills/greet" "$PP_REPO/plugins/hello/hooks"
cat >"$PP_REPO/.claude-plugin/marketplace.json" <<'JSON'
{ "name": "acme", "description": "Acme's tools", "owner": { "name": "Acme" }, "plugins": [
  { "name": "hello", "source": "./plugins/hello", "description": "Greets people", "version": "1.0.0" } ] }
JSON
printf '{ "name": "hello", "version": "1.0.0" }\n' >"$PP_REPO/plugins/hello/.claude-plugin/plugin.json"
printf -- '---\nname: greet\ndescription: Greet the user in their language\n---\nSay hello.\n' \
	>"$PP_REPO/plugins/hello/skills/greet/SKILL.md"
printf '#!/bin/sh\ncat >/dev/null\necho ran >> "$CLAUDE_PLUGIN_DATA/ran"\n' >"$PP_REPO/plugins/hello/hooks/mark.sh"
cat >"$PP_REPO/plugins/hello/hooks/hooks.json" <<'JSON'
{ "hooks": { "UserPromptSubmit": [ { "hooks": [ { "type": "command", "command": "sh \"${CLAUDE_PLUGIN_ROOT}/hooks/mark.sh\"" } ] } ] } }
JSON
# A scripted stdio MCP server (Phase 162's).
cat >"$PP_REPO/plugins/hello/server.sh" <<'MCPSRV'
#!/bin/sh
cat > /dev/null &
printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"resultType":"complete","supportedVersions":["2026-07-28"],"capabilities":{"tools":{}},"ttlMs":60000,"cacheScope":"public","_meta":{"io.modelcontextprotocol/serverInfo":{"name":"probe","version":"1.0"}}}}'
printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"resultType":"complete","tools":[{"name":"echo_text","description":"Echo the text back.","inputSchema":{"type":"object","properties":{"text":{"type":"string"}},"required":["text"]}}],"ttlMs":60000,"cacheScope":"public"}}'
sleep 60
MCPSRV
printf '{ "mcpServers": { "probe": { "command": "sh", "args": ["${CLAUDE_PLUGIN_ROOT}/server.sh"] } } }\n' \
	>"$PP_REPO/plugins/hello/.mcp.json"
(
	cd "$PP_REPO" &&
		git -c init.defaultBranch=main init -q &&
		git add -A &&
		git -c user.name=t -c user.email=t@t -c commit.gpgsign=false commit -qm one
) || fail "the marketplace repository could not be made"

# The folder marketplace `team`: `lint` brings a skill.
mkdir -p "$PP_FOLDER/.claude-plugin" "$PP_FOLDER/plugins/lint/skills/check"
cat >"$PP_FOLDER/.claude-plugin/marketplace.json" <<'JSON'
{ "name": "team", "owner": { "name": "Team" }, "plugins": [
  { "name": "lint", "source": "./plugins/lint", "description": "Lints the code" } ] }
JSON
printf -- '---\nname: check\ndescription: Check the code\n---\nCheck.\n' >"$PP_FOLDER/plugins/lint/skills/check/SKILL.md"

# A `git` that sleeps on a fetch, for the cancel.
PP_GIT="$(command -v git)"
cat >"$PP_BIN/git" <<SH
#!/bin/bash
for arg in "\$@"; do
	case "\$arg" in fetch) exec -a smoke163sleep sleep 30 ;; esac
done
exec "$PP_GIT" "\$@"
SH
chmod +x "$PP_BIN/git"

PP_DATA="$PP_CFG/plugins/data/acme/hello"
PP_SERVER="$PP_CFG/plugins/installed/acme/hello/server.sh"
PP_ENV="ALTER_ZERO_TELEMETRY=0 ALTER_ZERO_UPDATE_CHECK=0 ALTER_ZERO_TIPS=0 ALTER_ZERO_PROJECT_CONFIG=0 ALTER_ZERO_CONFIG_DIR=$PP_CFG ALTER_ZERO_CHECKPOINTS=0 ALTER_ZERO_HOOKS=1 ALTER_ZERO_HISTORY_FILE=/dev/null ALTER_ZERO_SKILLS_DIR=$SMOKE_SKILLS ALTER_ZERO_AGENTS_DIR=$SMOKE_AGENTS ALTER_ZERO_MASCOT_ANIMATION=0 ALTER_ZERO_PLUGIN_MARKETPLACES=acme=file://$PP_REPO"
PP_APP="env $PP_ENV ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN_ABS"
# A killed tmux session leaves its stdio server's `sleep` behind.
smoke_on_exit pkill -f "$PP_SERVER"

pp_server_running() { pgrep -f "$PP_SERVER" >/dev/null 2>&1; }
pp_open() {
	type_text "$S163" "/plugins"
	sleep 0.4
	keys "$S163" Enter
}
pp_rule_row() { pane "$S163" | grep -nE '^(─)+$' | tail -1 | cut -d: -f1; }
pp_closed() { wait_for 3 "$S163" -E '^❯ *$' || fail "the page did not close"; }

launch -c "$PP_WORK" "$S163" 110 40 "$PP_APP"

# --- an empty install: the + Add tab offers the marketplace
pp_open
pp_empty="$(wait_pane 5 "$S163" -F "file://$PP_REPO")" || fail "the + Add tab does not offer the configured marketplace"
dump "the page on an empty install" "$pp_empty"
expect_has "$pp_empty" -F " All (0)    Installed (0)    + Add" "an empty install does not open on All / Installed / + Add"
keys "$S163" Enter
pp_added="$(wait_pane 15 "$S163" -F " acme (1) ")" || fail "Enter on the offer did not add the marketplace"
dump "acme added" "$pp_added"
expect_has "$pp_added" -E "^  → hello +\+ available +Greets people" "hello is not listed as available"
expect_has "$pp_added" -F "Acme's tools" "the marketplace's tab does not show its description"

# --- hello's page: a preview from the clone's tree
keys "$S163" Enter
pp_page="$(wait_pane 5 "$S163" -E "Skills: +hello:greet")" || fail "hello's page never showed its preview"
dump "hello's page, not installed" "$pp_page"
expect_has "$pp_page" -E "Hooks: +included · reviewed before they run" "the preview does not say hello has hooks"
expect_has "$pp_page" -E "MCP servers: +included" "the preview does not say hello has a server"
expect_has "$pp_page" -F "❯ 1. Install" "hello's page does not offer Install first"

# --- install: the review takes the screen; nothing runs before it is answered
keys "$S163" Enter
pp_review="$(wait_pane 20 "$S163" -F "1 new hook found")" || fail "installing hello did not open the review"
dump "the review over the page" "$pp_review"
expect_has "$pp_review" -F "Plugin hello@acme · " "the review does not name the plugin"
expect_has "$pp_review" -F "/plugins/installed/acme/hello/hooks/mark.sh" "the hook's real command is not on the review"
sleep 1
pp_server_running && fail "hello's MCP server started before the review was answered"
[ -e "$PP_DATA/ran" ] && fail "hello's hook ran before the review was answered"
keys "$S163" Enter
pp_servers="$(wait_pane 5 "$S163" -F "found in your plugins")" || fail "the servers page never followed the hooks page"
expect_has "$pp_servers" -F "plugin:hello:probe · stdio" "the server is not on the review under its scoped name"
pp_server_running && fail "hello's MCP server started before its page was answered"
keys "$S163" Enter
pp_installed="$(wait_pane 10 "$S163" -E "^  ✔ enabled in this directory · hello@acme")" || fail "answering the review did not return to hello's page"
dump "hello's page after the review" "$pp_installed"
expect_has "$pp_installed" -E "Format: +Claude Code" "hello's page does not show the installed copy"
expect_has "$pp_installed" -E "Hooks: +UserPromptSubmit" "hello's page does not name its hook's event"
expect_has "$pp_installed" -E "MCP servers: +plugin:hello:probe" "hello's page does not name its server"
for _ in $(seq 1 50); do pp_server_running && break; sleep 0.2; done
pp_server_running || fail "hello's MCP server did not start once enabled on the review"

# --- the skill is live
keys "$S163" Escape
sleep 0.3
keys "$S163" Escape
pp_closed
type_text "$S163" '$hel'
pp_picker="$(wait_pane 5 "$S163" -F "hello:greet")" || fail "the \$ picker does not offer hello's skill"
dump "the \$ picker" "$pp_picker"
keys "$S163" C-c
sleep 0.3

# --- Space: off in this directory, and on again
pp_open
wait_for 5 "$S163" -E "^  → hello +✔ enabled" || fail "the page did not reopen on the installed plugin"
pp_rule="$(pp_rule_row)"
keys "$S163" " "
pp_off="$(wait_pane 5 "$S163" -E "^  → hello +◯ disabled")" || fail "Space did not turn hello off"
dump "hello off" "$pp_off"
expect_eq "$(pp_rule_row)" "$pp_rule" "the page moved as the toggle ran"
sed -n '/"projects"/,$p' "$PP_CFG/plugins.json" | grep -qF '"hello@acme"' || fail "plugins.json does not record hello off in this directory"
for _ in $(seq 1 25); do pp_server_running || break; sleep 0.2; done
pp_server_running && fail "hello's server still runs with hello off"
keys "$S163" Escape
pp_closed
type_text "$S163" '$hel'
sleep 0.8
expect_lacks "$(pane "$S163")" -F "hello:greet" "hello's skill is still offered with hello off"
keys "$S163" C-c
sleep 0.3
pp_open
wait_for 5 "$S163" -E "^  → hello +◯ disabled" || fail "the page did not reopen on hello off"
keys "$S163" " "
wait_for 5 "$S163" -E "^  → hello +✔ enabled" || fail "Space did not turn hello back on"
sleep 0.5
expect_lacks "$(pane "$S163")" -F "new hook found" "turning a trusted plugin back on asked the review again"

# --- a failure: said in red on the hint's row, the page not moving
keys "$S163" Left
sleep 0.2
keys "$S163" Left
wait_for 3 "$S163" -F "Add a marketplace…" || fail "Left did not reach the + Add tab"
type_text "$S163" "$SMOKE_TMP/no-such-folder"
wait_for 3 "$S163" -F "Press Enter to add it" || fail "the typed source is not offered"
pp_rule="$(pp_rule_row)"
keys "$S163" Enter
sleep 1
pp_failed="$(pane "$S163")"
dump "a failed add" "$pp_failed"
expect_eq "$(pp_rule_row)" "$pp_rule" "the page moved for the failure"
pp_hint="$(pane "$S163" | sed -n "$((pp_rule - 1))p")"
case "$pp_hint" in
*no-such-folder*) ;;
*) fail "the failure is not on the hint's row: $pp_hint" ;;
esac
# Red: the row carries a colour escape ahead of the failure.
pane "$S163" -e | sed -n "$((pp_rule - 1))p" | grep -qE $'\e\\[[0-9;]*m[^\e]*no-such-folder' || fail "the failure is not coloured"
expect_lacks "$pp_failed" -F " team (" "a folder that does not exist was added"

# --- the folder marketplace: typed into the field, its plugin installed
keys "$S163" Escape
sleep 0.3
type_text "$S163" "$PP_FOLDER"
wait_for 3 "$S163" -F "Press Enter to add it" || fail "the folder is not offered"
keys "$S163" Enter
pp_team="$(wait_pane 10 "$S163" -F " team (1) ")" || fail "the folder marketplace was not added"
dump "team added" "$pp_team"
keys "$S163" Enter
wait_for 5 "$S163" -E "Skills: +lint:check" || fail "lint's page never showed its preview"
keys "$S163" Enter
wait_for 10 "$S163" -E "^  ✔ enabled in this directory · lint@team" || fail "lint did not install"
sleep 0.5
expect_lacks "$(pane "$S163")" -F "new hook found" "a plugin with only a skill opened the review"
keys "$S163" Escape
sleep 0.3

# --- removing the folder marketplace takes lint with it
keys "$S163" C-r
pp_remove="$(wait_pane 3 "$S163" -F "Remove the team marketplace?")" || fail "Ctrl+R on team's tab did not ask"
dump "the question" "$pp_remove"
expect_has "$pp_remove" -F "lint" "the question does not name the plugin it uninstalls"
keys "$S163" Enter
wait_for 10 "$S163" -F " All (1)    Installed (1)    acme (1)    + Add" || fail "removing team did not drop its tab"
grep -qF "lint@team" "$PP_CFG/plugins.json" && fail "plugins.json still records lint after its marketplace was removed"
[ -d "$PP_FOLDER/plugins/lint" ] || fail "removing a folder marketplace deleted the user's folder"
tmux kill-session -t "$S163" 2>/dev/null
pkill -f "$PP_SERVER" 2>/dev/null
sleep 0.3

# --- Esc cancels a job in flight, killing its git
launch -c "$PP_WORK" "$S163" 110 40 "env PATH=$PP_BIN:$PATH $PP_APP"
pp_open
wait_for 5 "$S163" -E "^  → hello +✔ enabled" || fail "a second process did not open on hello installed"
keys "$S163" Right
wait_for 3 "$S163" -F "Ctrl+U update" || fail "Right did not reach acme's tab"
pp_rule="$(pp_rule_row)"
keys "$S163" C-u
pp_job="$(wait_pane 5 "$S163" -F "Updating acme…")" || fail "a fetch in flight is not said on the page"
dump "a fetch in flight" "$pp_job"
expect_eq "$(pp_rule_row)" "$pp_rule" "the page moved for the job's line"
pgrep -f smoke163sleep >/dev/null || fail "the fetch never reached git"
keys "$S163" Escape
wait_for 3 "$S163" -F "Ctrl+U update · Ctrl+R remove" || fail "Esc did not let go of the job"
for _ in $(seq 1 25); do pgrep -f smoke163sleep >/dev/null || break; sleep 0.2; done
pgrep -f smoke163sleep >/dev/null && fail "Esc did not kill the git child"

# --- uninstall
keys "$S163" Enter
wait_for 5 "$S163" -E "^  ✔ enabled in this directory · hello@acme" || fail "hello's page did not open"
keys "$S163" 3
wait_for 3 "$S163" -F "Uninstall hello?" || fail "Uninstall did not ask"
keys "$S163" Enter
wait_for 10 "$S163" -E "^  \+ not installed · hello@acme" || fail "hello was not uninstalled"
for _ in $(seq 1 25); do pp_server_running || break; sleep 0.2; done
pp_server_running && fail "hello's server still runs after the uninstall"
keys "$S163" Escape
sleep 0.3
keys "$S163" Escape
pp_closed

# --- a reopened page shows the same state; the subcommand agrees
pp_open
pp_again="$(wait_pane 5 "$S163" -F " acme (1) ")" || fail "the reopened page lost the marketplace"
dump "reopened" "$pp_again"
expect_has "$pp_again" -F " All (1)    Installed (0)    acme (1)    + Add" "the reopened page does not show what is on disk"
keys "$S163" Escape
tmux kill-session -t "$S163" 2>/dev/null
pp_list="$(cd "$PP_WORK" && env $PP_ENV "$BIN_ABS" plugin list --available 2>&1)"
dump "plugin list --available" "$pp_list"
expect_has "$pp_list" -E "^acme — 1 plugin" "plugin list does not list acme, as the page does"
expect_has "$pp_list" -E "^  hello +1\.0\.0 +available" "plugin list does not list hello available, as the page does"
expect_lacks "$pp_list" -F "team" "plugin list still lists the removed marketplace"
