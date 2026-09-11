#!/usr/bin/env bash
# Phase 42 — BACKGROUND SHELLS

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# BACKGROUND SHELLS (docs/background.md). A long `!` command is
# moved to the background with Ctrl+B: its cell resolves to the fixed
# `⎿ Running in the background (↓ to manage)` row, the footer counts
# `· 1 shell`, ↓ LIGHTS THAT COUNT on cyan (the rest of the footer intact, no
# band yet) and Enter opens the manager band (the list page, then Enter opens
# the details page whose output box tails the STILL-STREAMING live output),
# `x` stops the shell — committing the red `was stopped by the user` notice —
# and, that being the last shell, the manager CLOSES itself: the composer and
# the shell-less footer come back with no empty `No tasks currently running`
# page left asking to be dismissed (docs/background.md).
S42="${S}_background"
# A short command line (the notice headline must fit one row at 80 cols), with
# the slow streamer in a script file.
BG_SCRIPT="$SMOKE_TMP/bg.sh" # at the tree root: the headline must still fit one row
cat >"$BG_SCRIPT" <<'EOS'
i=0
while [ $i -lt 400 ]; do
	echo bgline$i
	i=$((i + 1))
	sleep 0.05
done
EOS
# In a short `~/work` cwd: this phase asserts the footer TAIL (`· 1 shell`),
# which the cwd pushes rightward off an 80-column row in a deep checkout.
launch -c "$(work_dir)" "$S42" 80 24 "$APP_ABS"
submit "$S42" "!sh $BG_SCRIPT"
# The hint is DELAYED (TOOL_BACKGROUND_HINT_DELAY, 3s): early in the run the
# command is clearly executing (its `⎿ Running…` row shows) but the Ctrl+B hint
# must NOT be there yet — a command that finishes right away never flashes it.
sleep 0.6
bg_early_pane="$(tmux capture-pane -t "$S42" -p)"
echo "==== Phase 42: early in the run — Running… but no Ctrl+B hint yet ===="
printf '%s\n' "$bg_early_pane"
# The running `!` cell shows the live-only Ctrl+B hint under its Running row —
# but only after the command has run a few seconds, Claude-Code-style, so a
# fast command never flashes it. Poll well past the delay.
bg_hint_pane="$(wait_pane 8 "$S42" -F "(ctrl+b to run in background)")"
echo "==== Phase 42: running ! command with the Ctrl+B hint ===="
printf '%s\n' "$bg_hint_pane"
# Ctrl+B: the runner hands the child to the registry; the cell resolves to the
# fixed backgrounded row and the footer gains the shell count.
tmux send-keys -t "$S42" C-b
bg_cell_pane=""
for _ in $(seq 1 30); do
	bg_cell_pane="$(tmux capture-pane -t "$S42" -p)"
	if printf '%s' "$bg_cell_pane" | grep -qF "Running in the background (↓ to manage)" \
		&& printf '%s' "$bg_cell_pane" | grep -qF "· 1 shell"; then
		break
	fi
	sleep 0.1
done
echo "==== Phase 42: cell resolved backgrounded + footer count ===="
printf '%s\n' "$bg_cell_pane"
# ↓ from the empty composer FOCUSES the footer's shell count first — it lights
# up on the default theme's accent — Catppuccin Mocha's sky, `#89DCEB`,
# SGR `48;2;137;220;235`, captured with -e (docs/theme.md; `ui/palette.rs`) — while the
# model/cwd segments stay put and no band opens. Enter is what opens it.
tmux send-keys -t "$S42" Down
sleep 0.3
bg_focus_pane="$(tmux capture-pane -t "$S42" -p)"
bg_focus_ansi="$(tmux capture-pane -t "$S42" -p -e)"
echo "==== Phase 42: ↓ lights the footer's shell indicator (no band yet) ===="
printf '%s\n' "$bg_focus_pane"
# Enter steps into the manager band on the list page.
tmux send-keys -t "$S42" Enter
sleep 0.3
bg_list_pane="$(tmux capture-pane -t "$S42" -p)"
echo "==== Phase 42: the ↓ manager list ===="
printf '%s\n' "$bg_list_pane"
# Enter opens the details page; its output box tails the live output.
tmux send-keys -t "$S42" Enter
bg_details_pane=""
for _ in $(seq 1 30); do
	bg_details_pane="$(tmux capture-pane -t "$S42" -p)"
	if printf '%s' "$bg_details_pane" | grep -qF "Shell details" \
		&& printf '%s' "$bg_details_pane" | grep -qF "bgline"; then
		break
	fi
	sleep 0.1
done
echo "==== Phase 42: the details page with live output ===="
printf '%s\n' "$bg_details_pane"
# The box follows the stream: the newest visible line number keeps advancing.
bg_seq_before="$(printf '%s\n' "$bg_details_pane" | grep -oE 'bgline[0-9]+' | tail -1)"
bg_seq_after="$bg_seq_before"
for _ in $(seq 1 30); do
	bg_details_later="$(tmux capture-pane -t "$S42" -p)"
	bg_seq_after="$(printf '%s\n' "$bg_details_later" | grep -oE 'bgline[0-9]+' | tail -1)"
	if [ -n "$bg_seq_after" ] && [ "$bg_seq_after" != "$bg_seq_before" ]; then
		break
	fi
	sleep 0.2
done
# `x` stops the shell: the red stopped notice commits and — every shell gone —
# the manager closes itself, the composer's prompt row returning in its place.
tmux send-keys -t "$S42" x
bg_stopped_pane=""
for _ in $(seq 1 40); do
	bg_stopped_pane="$(tmux capture-pane -t "$S42" -p -S -40)"
	if printf '%s' "$bg_stopped_pane" | grep -qF "was stopped by the user" \
		&& ! printf '%s' "$bg_stopped_pane" | grep -qF "Shell details"; then
		break
	fi
	sleep 0.1
done
echo "==== Phase 42: stopped notice + the band closed behind it ===="
printf '%s\n' "$bg_stopped_pane"
# The band is gone on its own — no Esc needed — and the footer stopped counting.
sleep 0.3
bg_closed_pane="$(tmux capture-pane -t "$S42" -p)"
tmux kill-session -t "$S42" 2>/dev/null

expect_lacks "$bg_early_pane" -F "(ctrl+b to run in background)" "the Ctrl+B hint showed immediately (it must wait a few seconds so a fast command never flashes it)"
expect_has "$bg_early_pane" -F "Running…" "the ! command was not visibly running early in its run (can't trust the no-hint check)"
expect_has "$bg_hint_pane" -F "(ctrl+b to run in background)" "the running ! command never showed the Ctrl+B hint (even after the delay)"
expect_has "$bg_cell_pane" -F "Running in the background (↓ to manage)" "Ctrl+B did not resolve the cell as backgrounded"
expect_has "$bg_cell_pane" -F "· 1 shell" "the footer never counted the running background shell"
# ↓ must FOCUS the footer's count, not open the band: the whole footer row
# survives (model · cwd · 1 shell), the count is painted on the cyan
# background, and the manager's list is nowhere on screen yet.
expect_has "$bg_focus_pane" -E 'dummy_model_name · .* · 1 shell' "↓ did not keep the whole footer row (model · cwd · 1 shell) while highlighting the indicator"
expect_lacks "$bg_focus_pane" -F "1 active shell" "↓ opened the manager band outright (it must light the footer indicator and wait for Enter)"
expect_has "$bg_focus_ansi" '48;2;137;220;235' "the focused shell indicator is not painted on the cyan background"
if ! printf '%s' "$bg_list_pane" | grep -qF "Background" \
	|| ! printf '%s' "$bg_list_pane" | grep -qF "1 active shell" \
	|| ! printf '%s' "$bg_list_pane" | grep -qF "(running)" \
	|| ! printf '%s' "$bg_list_pane" | grep -qF "↑/↓ to select · Enter to view · x to stop · Esc to close"; then
	fail "Enter on the lit indicator did not open the manager list (title/count/row/hints)"
fi
if ! printf '%s' "$bg_details_pane" | grep -qF "Shell details" \
	|| ! printf '%s' "$bg_details_pane" | grep -qF "Status:   running" \
	|| ! printf '%s' "$bg_details_pane" | grep -qF "Runtime:" \
	|| ! printf '%s' "$bg_details_pane" | grep -qF "Showing"; then
	fail "the details page is missing its fields/output box"
fi
if [ -z "$bg_seq_after" ] || [ "$bg_seq_after" = "$bg_seq_before" ]; then
	fail "the details output box did not stream (stuck at ${bg_seq_before:-nothing})"
fi
expect_has "$bg_stopped_pane" -F "was stopped by the user" "stopping the shell committed no notice"
if printf '%s' "$bg_closed_pane" | grep -qF "No tasks currently running" \
	|| printf '%s' "$bg_closed_pane" | grep -qF "Shell details" \
	|| printf '%s' "$bg_closed_pane" | grep -qF "active shell"; then
	fail "stopping the last shell left the manager band open (it must close itself, not sit on an empty page)"
fi
expect_has "$bg_closed_pane" -E 'dummy_model_name · ' "the composer/footer did not come back after the band closed"
expect_lacks "$bg_closed_pane" -F "· 1 shell" "the footer still counts a shell after the stop"
