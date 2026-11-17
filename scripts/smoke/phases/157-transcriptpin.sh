#!/usr/bin/env bash
# Phase 157 — Ctrl+O pins the header of the cell you scroll into, and ←/→ step between cells

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# Finding your way in the Ctrl+O transcript (docs/transcript-navigation.md).
# Two `!` commands print more than a screen each. Ctrl+O opens tail-following
# inside the second one's output, its `! seq 100 160` header long scrolled
# off — so the pager pins that header over the body's top row. ← opens the
# entry being read at its own first row (the real header on top, the output
# under it from its first line), ← again the command before it; ↓ from there
# scrolls the output under its pinned header; → steps on to the next
# command. The first hint row names the step.
S157="${S}_transcriptpin"
launch "$S157" 80 24
submit "$S157" "!seq 1 60"
wait_for 10 "$S157" -E '^ +60 *$' ||
	fail "the first command's output never finished"
submit "$S157" "!seq 100 160"
wait_for 10 "$S157" -E '^ +160 *$' ||
	fail "the second command's output never finished"
keys "$S157" C-o
pin_open="$(wait_pane 5 "$S157" -F "T R A N S C R I P T")"
note "Ctrl+O opened at the bottom — the second command's header pinned on top"
printf '%s\n' "$pin_open"
# Row 1 is the title, row 2 the body's first row.
pin_top="$(printf '%s\n' "$pin_open" | sed -n 2p | sed 's/ *$//')"
expect_eq "$pin_top" "! seq 100 160" "the window opened inside the second command's output without its header pinned on top"
expect_has "$pin_open" -E '^ +160 *$' "the tail-following window does not end on the newest output"
expect_has "$pin_open" -F "←/→ prev/next entry" "the first hint row does not name the entry step"

# ← opens the entry being read at its own first row: the real header, the
# output from its first line under it.
keys "$S157" Left
pin_own="$(wait_pane 5 "$S157" -E '^  ⎿  100 *$')"
note "← — the second command at the window's top"
printf '%s\n' "$pin_own"
expect_eq "$(printf '%s\n' "$pin_own" | sed -n 2p | sed 's/ *$//')" "! seq 100 160" "← did not open the second command at its first row"
expect_eq "$(printf '%s\n' "$pin_own" | sed -n 3p | sed 's/ *$//')" "  ⎿  100" "the output does not start under its header"

# ← again: the command before it.
keys "$S157" Left
pin_prev="$(wait_pane 5 "$S157" -E '^  ⎿  1 *$')"
note "← again — the first command at the window's top"
printf '%s\n' "$pin_prev"
expect_eq "$(printf '%s\n' "$pin_prev" | sed -n 2p | sed 's/ *$//')" "! seq 1 60" "the second ← did not open the first command"

# ↓ five rows: the output scrolls under the pinned header — the window
# opens on `5`, the pin covers it, and `6` sits right under the header.
# The wait is on that row: `23` at the bottom is on screen after four of
# the five, and a read then would race the fifth.
for _ in 1 2 3 4 5; do keys "$S157" Down; done
pin_scrolled="$(wait_row 5 "$S157" 3 "     6")"
note "↓×5 — the first command's output scrolled under its pinned header"
printf '%s\n' "$pin_scrolled"
expect_eq "$(printf '%s\n' "$pin_scrolled" | sed -n 2p | sed 's/ *$//')" "! seq 1 60" "the header was not pinned over the scrolled output"
expect_eq "$(printf '%s\n' "$pin_scrolled" | sed -n 3p | sed 's/ *$//')" "     6" "the output under the pin is not the next rows of the command's output"
expect_lacks "$pin_scrolled" -E '^ +⎿ +1 *$' "the output's first row is still on screen after scrolling past it"

# → steps on to the next command.
keys "$S157" Right
pin_next="$(wait_pane 5 "$S157" -E '^  ⎿  100 *$')"
note "→ — on to the second command"
printf '%s\n' "$pin_next"
expect_eq "$(printf '%s\n' "$pin_next" | sed -n 2p | sed 's/ *$//')" "! seq 100 160" "→ did not open the next command"

keys "$S157" q
wait_for 5 "$S157" -F "dummy_model_name" ||
	fail "q did not bring the conversation back"
tmux kill-session -t "$S157" 2>/dev/null

# ---- Part B: backend tool cells on a resumed session ----
# A handcrafted rollout (the Phase 48 shape): a prompt, two `bash` calls
# taller than the pane — the second's command wider than it, so its header
# wraps onto two rows — and the replies between them.
#   (a) Ctrl+O opens tail-following deep in the wide command's output: both
#       rows of its header are pinned, the whole command on screen;
#   (b) ← opens that cell at its own first row, ↓ scrolls its output under
#       the two-row pin;
#   (c) from the top, → walks entry by entry to the first call, and paging
#       into its output pins `● Bash(seq 1 60)` — and the same five keys
#       sent in ONE write land in the same place, every press a move of its
#       own in the order it came;
#   (d) → and Esc sent in ONE write — a laggy link batching two keys into one
#       frame — still open the backtrack preview with its highlight on
#       screen: a step pending when the preview begins is dropped, where it
#       used to land after the preview's scroll and push the highlighted
#       message off the top;
#   (e) the preview's first hint row stops promising the entry step, since
#       ←/→ step the highlight there, and q cancels it.
S157B="${S}_transcriptpin_tools"
TP_DIR="$(work_dir transcriptpin)"
TP_SESSIONS="$(mktemp -d "$SMOKE_TMP/transcriptpin-sessions.XXXXXX")"
mkdir -p "$TP_SESSIONS/2026/07/23"
python3 - "$TP_SESSIONS/2026/07/23/rollout-2026-07-23T10-00-00-15715716.jsonl" "$TP_DIR" <<'PY'
import json, sys

path, cwd = sys.argv[1], sys.argv[2]
stamp = "2026-07-23T10:00:00.000Z"
wide = ("for n in $(seq 1 30); do printf 'row %s\\n' \"$n\"; done "
        "# a deliberately long command line, wider than the pane, to wrap its header")


def record(kind, payload):
    return json.dumps({"timestamp": stamp, "type": kind, "payload": payload})


def message(role, text):
    return record("message", {"role": role, "text": text, "timestamp": "10:00 AM"})


def bash(command, output):
    return record("tool", {
        "name": "Bash", "args": command, "ok": True,
        "output": "Exit code: 0\n" + output,
        "timestamp": "10:00 AM", "shell": False, "truncated": False,
    })


lines = [
    record("session_meta", {
        "id": "smoke-157b", "timestamp": stamp, "cwd": cwd,
        "model": "dummy_model_name", "originator": "alter-zero", "version": "0.1.0",
    }),
    message("user", "count to sixty"),
    message("assistant", "Counting with seq."),
    bash("seq 1 60", "\n".join(str(n) for n in range(1, 61))),
    message("assistant", "Counted to sixty."),
    bash(wide, "\n".join(f"row {n}" for n in range(1, 31))),
    message("assistant", "Both commands ran."),
]
with open(path, "w") as out:
    out.write("\n".join(lines) + "\n")
PY
TP_APP="env $CFG_ENV_NOHIST ALTER_ZERO_SESSIONS_DIR=$TP_SESSIONS ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN_ABS --continue"
# tp_row N — pane row N (1-based: row 1 is the title, row 2 the body's top)
tp_row() { pane "$S157B" | sed -n "${1}p" | sed 's/[[:space:]]*$//'; }
TP_HEAD1="● Bash(for n in \$(seq 1 30); do printf 'row %s\\n' \"\$n\"; done # a deliberately"
TP_HEAD2="      long command line, wider than the pane, to wrap its header)"
launch -c "$TP_DIR" -w "Both commands ran" "$S157B" 80 24 "$TP_APP"
wait_for 5 "$S157B" -F "dummy_model_name" || fail "the resumed session never reached its composer"

# (a) opening deep in the wide command's output: its whole header pinned.
keys "$S157B" C-o
tp_open="$(wait_pane 5 "$S157B" -E '^ +row 30 *$')" || fail "Ctrl+O did not open on the end of the wide command's output"
note "Part B (a): Ctrl+O tail-following inside the wide command — both header rows pinned"
printf '%s\n' "$tp_open"
expect_eq "$(tp_row 2)" "$TP_HEAD1" "the wide command's first header row is not pinned on the top row"
expect_eq "$(tp_row 3)" "$TP_HEAD2" "the wrapped command's second row is not pinned under its first — the command is not on screen whole"

# (b) ← opens the cell at its own first row; ↓ scrolls the output under the pin.
keys "$S157B" Left
wait_pane 5 "$S157B" -E '^  ⎿  row 1 *$' >/dev/null || fail "← did not open the wide command at its first row"
expect_eq "$(tp_row 2)" "$TP_HEAD1" "← did not put the wide command's header on top"
for _ in 1 2 3 4 5 6 7 8; do keys "$S157B" Down; done
tp_scrolled="$(wait_row 5 "$S157B" 4 "     row 9")"
note "Part B (b): eight rows into the wide command's output, under its two-row pin"
printf '%s\n' "$tp_scrolled"
expect_eq "$(tp_row 2)" "$TP_HEAD1" "scrolled into the output, the header's first row is not pinned"
expect_eq "$(tp_row 3)" "$TP_HEAD2" "scrolled into the output, the header's second row is not pinned"
expect_eq "$(tp_row 4)" "     row 9" "the output does not carry on under the two-row pin"
expect_lacks "$tp_scrolled" -E '⎿ +row 1 *$' "the output's first row is still on screen — nothing scrolled under the pin"

# (c) → from the top walks entry by entry; paging into the first call pins it.
keys "$S157B" Home
# (`.*`, not `─+`: a repeat binds to the last byte of a multibyte glyph.)
wait_for 5 "$S157B" -E '^─.* 0% ─$' || fail "Home did not reach the top of the transcript"
# Each step waits on the row it moves: the prompt, the reply and the call
# are all on screen from the top, so a wait for their text anywhere returned
# before the key had landed, and a busy CI runner read the pane a frame early.
keys "$S157B" Right
wait_row 5 "$S157B" 2 "❯ count to sixty" >/dev/null || fail "→ from the top did not open the prompt"
keys "$S157B" Right
wait_row 5 "$S157B" 2 "● Counting with seq." >/dev/null || fail "a second → did not open the reply"
keys "$S157B" Right
wait_row 5 "$S157B" 2 "● Bash(seq 1 60)" >/dev/null || fail "a third → did not put the first bash cell on top"
expect_eq "$(tp_row 3)" "  ⎿  1" "a third → did not open the first bash cell at its first row"
keys "$S157B" PageDown PageDown
tp_paged="$(wait_row 5 "$S157B" 3 "     21")"
note "Part B (c): two pages into the first call's output"
printf '%s\n' "$tp_paged"
expect_eq "$(tp_row 2)" "● Bash(seq 1 60)" "deep in the first call's output its header is not pinned"
expect_eq "$(tp_row 3)" "     21" "the first call's output does not carry on under its pin"

# The same five keys in ONE write land where they did one at a time: each
# press a move of its own, in the order it came. The newest step used to win
# (three presses, one step) and the pages to overtake it, landing on the
# reply after the call — the collapse that cost a CI run the third step.
keys "$S157B" Home
wait_for 5 "$S157B" -E '^─.* 0% ─$' || fail "Home did not return to the top"
keys "$S157B" Right Right Right PageDown PageDown
tp_batched="$(wait_row 5 "$S157B" 3 "     21")" ||
	fail "→ → → PageDown PageDown in one write did not land two pages into the first call"
note "Part B (c): the same keys in one write"
printf '%s\n' "$tp_batched"
expect_eq "$(tp_row 2)" "● Bash(seq 1 60)" "the keys in one write did not open the first call under its pin"

# (d) → and Esc in one write: the preview opens with its highlight on screen.
keys "$S157B" End
wait_for 5 "$S157B" -E '^ +row 30 *$' || fail "End did not return to the tail"
keys "$S157B" Right Escape
tp_preview="$(wait_pane 5 "$S157B" -F "enter to edit message")" || fail "→ and Esc in one write did not open the backtrack preview"
note "Part B (d): → and Esc in one write — the preview and its highlight"
printf '%s\n' "$tp_preview"
expect_has "$tp_preview" -E '^❯ count to sixty' "the preview's highlighted message is off screen — a pending step moved the page past it"

# (e) the preview's hint rows; q cancels.
expect_lacks "$tp_preview" -F "prev/next entry" "the preview's first hint row still promises the entry step — ←/→ step the highlight there"
expect_has "$tp_preview" -F "to scroll" "the preview lost its scroll hints"
keys "$S157B" q
wait_for 5 "$S157B" -F "dummy_model_name" || fail "q did not cancel the backtrack preview"
tmux kill-session -t "$S157B" 2>/dev/null
smoke_finish
