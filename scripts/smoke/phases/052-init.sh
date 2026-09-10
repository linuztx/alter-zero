#!/usr/bin/env bash
# Phase 52 — /init + the AGENTS.md instructions in the context

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# /init + the AGENTS.md instructions in the context
# (docs/init.md, docs/project-doc.md). In a temp project with a planted
# AGENTS.md: Ctrl+D shows codex's `# AGENTS.md instructions … <INSTRUCTIONS>`
# fragment BEFORE any turn (the startup seed — dummy backend, so this proves
# the App-side injection is backend-independent), /init submits the bundled
# AGENTS.md-authoring prompt as a normal user turn (codex's one-line
# dispatch), and a mid-turn /init is rejected with the busy toast (codex's
# available_during_task=false). Launched with a LONG pre-stream pause so the
# mid-turn press deterministically lands while the first turn is still
# active (the Phase 20 pattern).
S52="${S}_init"
WORK52="$(mktemp -d "$SMOKE_TMP/init.XXXXXX")"
mkdir -p "$WORK52/.git"
printf '# Contributor guide\n\nSmoke sentinel: Umbral-Kite-77.\n' >"$WORK52/AGENTS.md"
# The app runs in the temp cwd (-c), so the binary must be an absolute path.
BIN_ABS52="$(readlink -f "$BIN")"
tmux new-session -d -s "$S52" -x 100 -y 30 -c "$WORK52" \
	"env $CFG_ENV_NOHIST ALTER_ZERO_STARTUP_DELAY_MS=2000 $BIN_ABS52"
sleep 0.5
tmux send-keys -t "$S52" C-d
sleep 0.4
tmux send-keys -t "$S52" Home # the view opens at the bottom; jump to the top
sleep 0.3
init_ctx="$(tmux capture-pane -t "$S52" -p)"
echo "==== captured pane (Ctrl+D shows the AGENTS.md instructions fragment) ===="
printf '%s\n' "$init_ctx"
tmux send-keys -t "$S52" q # back to the conversation
sleep 0.4
tmux send-keys -t "$S52" -l "/init"
sleep 0.3
tmux send-keys -t "$S52" Enter
init_pane="$(wait_pane 1 "$S52" -S -60 -- -F "Generate a file named AGENTS.md")" # the prompt commits as the user message immediately
echo "==== captured pane (/init submitted the bundled prompt) ===="
printf '%s\n' "$init_pane"
# Still inside the 2s pre-stream pause: /init again -> the busy toast.
submit "$S52" "/init"
init_busy="$(wait_pane 1.5 "$S52" -S -20 -- -F "/init is disabled")" # up to ~1.5s, inside the pause
echo "==== captured pane (mid-turn /init busy toast) ===="
printf '%s\n' "$init_busy"
tmux kill-session -t "$S52" 2>/dev/null
echo "==== Phase 52: /init + the AGENTS.md instructions in the Ctrl+D context ===="
expect_has "$init_ctx" -F "# AGENTS.md instructions" "Ctrl+D lacks the AGENTS.md instructions fragment"
expect_has "$init_ctx" -F "Umbral-Kite-77" "the planted AGENTS.md content is missing from the context view"
expect_has "$init_pane" -F "Generate a file named AGENTS.md" "/init did not submit the bundled AGENTS.md prompt as the user message"
expect_has "$init_busy" -F "/init is disabled while a task is in progress" "mid-turn /init was not rejected with the busy toast"
rm -rf "$WORK52" 2>/dev/null
