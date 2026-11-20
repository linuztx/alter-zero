#!/usr/bin/env bash
# Phase 163 — a plugin's slash commands reach the session the way a built-in's do

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# How an installed plugin's commands reach a running session
# (docs/plugins.md *What an enabled plugin adds to a session*), driven
# against a folder marketplace the phase builds, installed with the CLI —
# never the network. A plugin `dup` ships `quit.md`, `empty.md` (nothing but
# frontmatter) and `echo.md` (`Echo: $ARGUMENTS`):
#   - `/q` and `/quit` still quit: every built-in lists ahead of the plugin's
#     rows, so Enter never runs `/dup:quit`;
#   - `/dup:empty` is refused with a toast naming it and starts no turn;
#   - the `[PROMPT]` shortcut and a Tab-queued follow-up run the command,
#     rendered — never the literal invocation;
#   - `-p` does the same headless;
#   - a plugin disabled from another terminal stops running at once.
S163="${S}_plugincmds"
PC_CFG="$(mktemp -d)"
PC_SESSIONS="$(mktemp -d)"
PC_MK="$(mktemp -d)"
SMOKE_EXIT_HOOKS+=("rm -rf '$PC_CFG' '$PC_SESSIONS' '$PC_MK'")

mkdir -p "$PC_MK/.claude-plugin" "$PC_MK/plugins/dup/.claude-plugin" "$PC_MK/plugins/dup/commands"
cat >"$PC_MK/.claude-plugin/marketplace.json" <<'JSON'
{ "name": "acme163", "plugins": [ { "name": "dup", "source": "./plugins/dup", "description": "Commands named like built-ins" } ] }
JSON
printf '{ "name": "dup", "version": "1.0.0" }\n' >"$PC_MK/plugins/dup/.claude-plugin/plugin.json"
printf -- '---\ndescription: Not the real quit\n---\nPLUGIN QUIT PROMPT\n' >"$PC_MK/plugins/dup/commands/quit.md"
printf -- '---\ndescription: Nothing to say\n---\n' >"$PC_MK/plugins/dup/commands/empty.md"
printf -- '---\ndescription: Echo it\n---\nEcho: $ARGUMENTS\n' >"$PC_MK/plugins/dup/commands/echo.md"

PC_ENV="$CFG_ENV_NOHIST ALTER_ZERO_CONFIG_DIR=$PC_CFG ALTER_ZERO_SESSIONS_DIR=$PC_SESSIONS ALTER_ZERO_PLUGIN_MARKETPLACES=$PC_MK"
PC_APP="env $PC_ENV ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN"
pc_install="$(env $PC_ENV $BIN plugin install dup@acme163 2>&1)" ||
	fail "plugin install dup@acme163 failed: $pc_install"
dump "plugin install dup@acme163" "$pc_install"

# /q: the built-in is the highlighted row and Enter quits.
launch "$S163" 100 30 "$PC_APP"
type_text "$S163" "/q"
pc_q="$(wait_pane 5 "$S163" -F "/dup:quit")"
dump "typed /q" "$pc_q"
expect_has "$pc_q" -E "^→ /quit " "/quit is not the highlighted row under /q"
keys "$S163" Enter
wait_gone 10 "$S163" || fail "Enter on /q did not quit"
tmux kill-session -t "$S163" 2>/dev/null

# /dup:empty: a toast naming it, no turn.
launch "$S163" 100 30 "$PC_APP"
submit "$S163" "/dup:empty"
pc_empty="$(wait_pane 5 "$S163" -F "/dup:empty has no prompt to submit")"
dump "/dup:empty" "$pc_empty"
expect_has "$pc_empty" -F "/dup:empty has no prompt to submit" "a blank command was not refused with a toast"
expect_eq "$(count_bare_prompts "$pc_empty")" "1" "a blank command left an empty ❯ message"
expect_lacks "$pc_empty" -E "$SUMMARY_RE" "a blank command started a turn"

# Tab mid-turn queues /dup:echo; its turn sends the rendered prompt.
submit "$S163" "$USER_MSG"
wait_for 5 "$S163" -F "esc to interrupt" || fail "the first turn never started"
type_text "$S163" "/dup:echo queued"
sleep "$SMOKE_TYPE_SETTLE"
keys "$S163" Tab
pc_queued="$(wait_pane 30 "$S163" -F "❯ Echo: queued")"
dump "the Tab-queued /dup:echo, dispatched" "$pc_queued"
expect_has "$pc_queued" -F "❯ Echo: queued" "the queued invocation was not rendered"
expect_lacks "$pc_queued" -F "❯ /dup:echo queued" "the queued invocation reached the model literally"
wait_summaries 60 "$S163" 2 -S -400 >/dev/null || fail "the queued plugin turn never finished"

# Disabled from another terminal: the next invocation is refused at once.
pc_off="$(env $PC_ENV $BIN plugin disable dup@acme163 2>&1)" || fail "plugin disable failed: $pc_off"
submit "$S163" "/dup:echo late"
pc_late="$(wait_pane 5 "$S163" -F "/dup:echo is no longer installed")"
dump "/dup:echo after disabling dup elsewhere" "$pc_late"
expect_has "$pc_late" -F "/dup:echo is no longer installed" "a plugin disabled elsewhere still ran its command"
expect_lacks "$pc_late" -F "Echo: late" "a plugin disabled elsewhere still ran its command"
keys "$S163" C-c
tmux kill-session -t "$S163" 2>/dev/null
pc_on="$(env $PC_ENV $BIN plugin enable dup@acme163 2>&1)" || fail "plugin enable failed: $pc_on"

# The [PROMPT] shortcut runs the command.
launch "$S163" 100 30 "$PC_APP '/dup:echo from the command line'"
pc_cli="$(wait_pane 15 "$S163" -F "❯ Echo: from the command line")"
dump "alter-zero '/dup:echo from the command line'" "$pc_cli"
expect_has "$pc_cli" -F "❯ Echo: from the command line" "the [PROMPT] invocation was not rendered"
keys "$S163" C-c
tmux kill-session -t "$S163" 2>/dev/null

# -p runs it headless; the rollout records the rendered prompt.
pc_hl="$(env $PC_ENV ALTER_ZERO_DUMMY=1 ALTER_ZERO_STARTUP_DELAY_MS=0 ALTER_ZERO_CHUNK_DELAY_MS=0 \
	$BIN -p '/dup:echo headless' </dev/null 2>&1)"
pc_hl_code=$?
dump "alter-zero -p '/dup:echo headless'" "$pc_hl"
expect_eq "$pc_hl_code" "0" "the headless plugin command did not succeed"
grep -rqF "Echo: headless" "$PC_SESSIONS" || fail "the headless run did not send the rendered prompt"
grep -rqF '"/dup:echo headless"' "$PC_SESSIONS" && fail "the headless run sent the invocation literally"

smoke_finish
