#!/usr/bin/env bash
# Phase 120 — full-screen Git change review

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# Keep every Git operation and file mutation in disposable fixtures. The
# viewer itself is read-only; compare the fixture's index, patches, and file
# contents before and after the review to guard that boundary too.
DIFF_DIR="$(work_dir review)"
git -C "$DIFF_DIR" init -q
git -C "$DIFF_DIR" config user.name "Smoke Test"
git -C "$DIFF_DIR" config user.email "smoke@example.invalid"
python3 - "$DIFF_DIR" <<'PY'
from pathlib import Path
import sys

root = Path(sys.argv[1])
root.joinpath("alpha-worktree.txt").write_text(
    "".join(f"unchanged line {i:02d}\n" for i in range(1, 71))
)
root.joinpath("beta-staged.txt").write_text("STAGED BEFORE\n")
PY
git -C "$DIFF_DIR" add alpha-worktree.txt beta-staged.txt
git -C "$DIFF_DIR" commit -qm "Initial review fixture"
python3 - "$DIFF_DIR" <<'PY'
from pathlib import Path
import sys

root = Path(sys.argv[1])
file = root / "alpha-worktree.txt"
lines = file.read_text().splitlines()
lines[1] = "WORKTREE AFTER first hunk"
for i in range(2, 7):
    lines[i] = f"WORKTREE changed companion {i:02d}"
lines[59] = "WORKTREE AFTER final hunk"
file.write_text("\n".join(lines) + "\n")
root.joinpath("beta-staged.txt").write_text(
    "STAGED AFTER\nLONG " + "x" * 100 + " HORIZONTAL END\n"
)
root.joinpath("gamma-untracked.txt").write_text(
    "".join(f"UNTRACKED review line {i:02d}\n" for i in range(1, 81))
)
PY
git -C "$DIFF_DIR" add beta-staged.txt

review_snapshot() {
	git -C "$DIFF_DIR" status --porcelain=v1
	git -C "$DIFF_DIR" diff --cached --binary
	git -C "$DIFF_DIR" diff --binary
	sha256sum "$DIFF_DIR/alpha-worktree.txt" "$DIFF_DIR/beta-staged.txt" "$DIFF_DIR/gamma-untracked.txt"
}
review_before="$(review_snapshot)"

review_pane_lacks() { ! pane_has "$1" -F "$2"; }

S120="${S}_diff"
launch -c "$DIFF_DIR" "$S120" 120 34 "$APP_ABS"
type_text "$S120" "/diff"
sleep "$SMOKE_TYPE_SETTLE"
diff_palette="$(wait_pane 3 "$S120" -F "Review Git changes")"
dump "/diff in the palette" "$diff_palette"
expect_has "$diff_palette" -F "/diff" "the slash palette does not list /diff"
keys "$S120" Enter
diff_open="$(wait_pane 8 "$S120" -F "gamma-untracked.txt")"
dump "all changes" "$diff_open"
for marker in "Git changes" "Files" "Patch" "alpha-worktree.txt" "beta-staged.txt" "gamma-untracked.txt"; do
	expect_has "$diff_open" -F "$marker" "the review is missing '$marker'"
done
expect_eq "$(tmux display-message -p -t "$S120" '#{alternate_on}')" "1" "/diff did not use the alternate screen"
expect_lacks "$diff_open" -F "dummy_model_name" "the conversation footer leaked into the full-screen review"

# Filters must change both the file list and the selected patch. The keys
# follow the order printed in the toolbar: 1 All, 2 Unstaged, 3 Staged,
# 4 Untracked. Filtering also resets selection to a visible file.
keys "$S120" 2
diff_unstaged="$(wait_pane 3 "$S120" -F "WORKTREE AFTER first hunk")"
dump "unstaged changes" "$diff_unstaged"
expect_has "$diff_unstaged" -F "WORKTREE AFTER first hunk" "2 did not show the unstaged patch"
expect_lacks "$diff_unstaged" -F "beta-staged.txt" "the unstaged filter includes a staged-only file"
expect_lacks "$diff_unstaged" -F "gamma-untracked.txt" "the unstaged filter includes an untracked file"
keys "$S120" Enter
diff_patch_focus="$(wait_pane 3 "$S120" -F "Patch focus")"
expect_has "$diff_patch_focus" -F "Patch focus" "Enter did not focus the patch"
keys "$S120" n n
diff_next_hunk="$(wait_pane 3 "$S120" -F "WORKTREE AFTER final hunk")"
expect_has "$diff_next_hunk" -F "WORKTREE AFTER final hunk" "n did not reveal the next hunk"
keys "$S120" N
diff_previous_hunk="$(wait_pane 3 "$S120" -F "WORKTREE AFTER first hunk")"
expect_has "$diff_previous_hunk" -F "WORKTREE AFTER first hunk" "N did not return to the previous hunk"
keys "$S120" Tab
diff_files_focus="$(wait_pane 3 "$S120" -F "Files focus")"
expect_has "$diff_files_focus" -F "Files focus" "Tab did not return focus to the file list"

keys "$S120" 3
diff_staged="$(wait_pane 3 "$S120" -F "STAGED AFTER")"
dump "staged changes" "$diff_staged"
expect_has "$diff_staged" -F "STAGED BEFORE" "the staged patch omits the removed content"
expect_has "$diff_staged" -F "STAGED AFTER" "3 did not show the staged patch"
expect_lacks "$diff_staged" -F "alpha-worktree.txt" "the staged filter includes an unstaged-only file"
expect_lacks "$diff_staged" -F "gamma-untracked.txt" "the staged filter includes an untracked file"
for _ in $(seq 1 20); do keys "$S120" Right; done
diff_horizontal="$(wait_pane 3 "$S120" -F "HORIZONTAL END")"
expect_has "$diff_horizontal" -F "HORIZONTAL END" "Right did not reveal the end of a long patch line"
for _ in $(seq 1 20); do keys "$S120" Left; done
diff_horizontal_reset="$(wait_pane 3 "$S120" -F "STAGED AFTER")"
expect_has "$diff_horizontal_reset" -F "STAGED AFTER" "Left did not return to the start of patch lines"

keys "$S120" 4
diff_untracked="$(wait_pane 3 "$S120" -F "UNTRACKED review line 01")"
expect_has "$diff_untracked" -F "UNTRACKED review line 01" "4 did not show untracked file contents"
expect_lacks "$diff_untracked" -F "alpha-worktree.txt" "the untracked filter includes a tracked file"
keys "$S120" Enter End
diff_patch_bottom="$(wait_pane 3 "$S120" -F "UNTRACKED review line 80")"
expect_has "$diff_patch_bottom" -F "UNTRACKED review line 80" "End did not reach the end of the patch"
keys "$S120" Home NPage
poll 3 review_pane_lacks "$S120" "UNTRACKED review line 01" || true
diff_page_down="$(pane "$S120")"
expect_lacks "$diff_page_down" -F "UNTRACKED review line 01" "PgDn did not scroll the patch"
keys "$S120" PPage Home
diff_patch_top="$(wait_pane 3 "$S120" -F "UNTRACKED review line 01")"
expect_has "$diff_patch_top" -F "UNTRACKED review line 01" "PgUp/Home did not return to the start of the patch"

# Filename search handles ordinary letters as text, including 'r', rather
# than interpreting them as review shortcuts. Esc leaves search editing and
# keeps the review open; selecting All first ensures it searches every kind.
keys "$S120" 1 /
type_text "$S120" "gamma"
poll 3 review_pane_lacks "$S120" "alpha-worktree.txt" || true
diff_search="$(wait_pane 3 "$S120" -F "UNTRACKED review line 01")"
expect_lacks "$diff_search" -F "alpha-worktree.txt" "filename search did not filter the file list"
expect_lacks "$diff_search" -F "beta-staged.txt" "filename search retained an unrelated path"
keys "$S120" Enter /
keys "$S120" Escape
expect_eq "$(tmux display-message -p -t "$S120" '#{alternate_on}')" "1" "Esc in filename search closed the review"
diff_search_cancel="$(wait_pane 3 "$S120" -F "alpha-worktree.txt")"
expect_has "$diff_search_cancel" -F "alpha-worktree.txt" "Esc did not clear the filename search"
keys "$S120" /
type_text "$S120" "missing-file"
diff_no_matches="$(wait_pane 3 "$S120" -F "No matching files")"
expect_has "$diff_no_matches" -F "No matching files" "an unmatched filename query has no empty state"
keys "$S120" Enter /
for _ in $(seq 1 12); do keys "$S120" BSpace; done
keys "$S120" Enter
diff_search_clear="$(wait_pane 3 "$S120" -F "alpha-worktree.txt")"
expect_has "$diff_search_clear" -F "beta-staged.txt" "clearing the filename query did not restore the file list"
keys "$S120" ']'
diff_next_file="$(wait_pane 3 "$S120" -F "STAGED AFTER")"
expect_has "$diff_next_file" -F "STAGED AFTER" "] did not select the next file"
keys "$S120" '['
diff_previous_file="$(wait_pane 3 "$S120" -F "WORKTREE AFTER first hunk")"
expect_has "$diff_previous_file" -F "WORKTREE AFTER first hunk" "[ did not select the previous file"

# Full-screen resizing is an I/O path, distinct from pure layout tests.
tmux resize-window -t "$S120" -x 64 -y 18
diff_narrow="$(wait_pane 3 "$S120" -F "Git changes")"
dump "resized review" "$diff_narrow"
expect_has "$diff_narrow" -F "Git changes" "the review disappeared on resize"
tmux resize-window -t "$S120" -x 80 -y 24
diff_standard="$(wait_pane 3 "$S120" -F "Files")"
dump "standard 80-column review" "$diff_standard"
expect_has "$diff_standard" -F "Files" "the standard-width review did not restore the file pane"
tmux resize-window -t "$S120" -x 120 -y 34
wait_for 3 "$S120" -F "gamma-untracked.txt" || fail "the review did not recover its wide layout"
keys "$S120" q
diff_closed="$(wait_pane 3 "$S120" -F "dummy_model_name")"
expect_has "$diff_closed" -F "dummy_model_name" "q did not restore the conversation"
expect_eq "$(tmux display-message -p -t "$S120" '#{alternate_on}')" "0" "closing /diff left the terminal on its alternate screen"
expect_eq "$(review_snapshot)" "$review_before" "reviewing changed the Git index or working files"

# The refresh sees a file created while the review is already open. Closing
# with Ctrl+C must restore chat rather than exiting the app.
submit "$S120" "/diff"
wait_for 8 "$S120" -F "gamma-untracked.txt" || fail "the review did not reopen"
printf 'REFRESH NEW FILE\n' >"$DIFF_DIR/delta-refresh.txt"
keys "$S120" r
diff_refreshed="$(wait_pane 8 "$S120" -F "delta-refresh.txt")"
expect_has "$diff_refreshed" -F "delta-refresh.txt" "r did not reload changes created after opening"
keys "$S120" C-c
diff_ctrl_c="$(wait_pane 3 "$S120" -F "dummy_model_name")"
expect_has "$diff_ctrl_c" -F "dummy_model_name" "Ctrl+C quit the app instead of closing the review"

# Streaming keeps advancing while the alternate screen owns the terminal.
# The recorder's completed-turn summary is the observable completion signal
# without leaving /diff or adding a fixed delay. No prior turn in this phase
# has produced a rollout summary.
submit "$S120" "$USER_MSG"
wait_for 6 "$S120" -F "Happy" || fail "the dummy turn never began streaming"
submit "$S120" "/diff"
diff_during_turn="$(wait_pane 8 "$S120" -F "Git changes")"
expect_has "$diff_during_turn" -F "Git changes" "/diff did not open during a running turn"
poll 30 grep -rqF --include='*.jsonl' '"type":"summary"' "$SMOKE_SESSIONS" || fail "the running turn did not finish while /diff was open"
expect_eq "$(tmux display-message -p -t "$S120" '#{alternate_on}')" "1" "finishing the turn unexpectedly closed the review"
keys "$S120" q
diff_stream_return="$(wait_pane 3 "$S120" -S -80 -- -F "$SETTLED_REPLY")"
dump "conversation restored after a turn finished under /diff" "$diff_stream_return"
expect_has "$diff_stream_return" -F "$SETTLED_REPLY" "the completed reply was not restored after closing /diff"
expect_has "$diff_stream_return" -E "$SUMMARY_RE" "the completed turn's summary was not restored after closing /diff"
expect_lacks "$diff_stream_return" -F "esc to interrupt" "closing /diff left a stale streaming status"
tmux kill-session -t "$S120" 2>/dev/null

# A clean initialized project still opens the review. A plain folder stays
# in chat throughout validation and receives a neutral, concise info toast.
CLEAN_DIR="$(work_dir clean)"
git -C "$CLEAN_DIR" init -q
S120C="${S}_diffclean"
launch -c "$CLEAN_DIR" "$S120C" 100 28 "$APP_ABS"
DIFF_CLEAN_RAW="$SMOKE_TMP/diff-clean.raw"
tmux pipe-pane -t "$S120C" -o "cat > $DIFF_CLEAN_RAW"
submit "$S120C" "/diff"
diff_clean="$(wait_pane 8 "$S120C" -F "Working tree clean")"
expect_has "$diff_clean" -F "Working tree clean" "a clean unborn repository did not show its empty state"
poll 3 grep -aqF 'Working tree clean' "$DIFF_CLEAN_RAW" || fail "the valid-repo terminal recording never reached the review"
tmux pipe-pane -t "$S120C"
# Positive control for the non-Git recording below: a valid repository emits
# the exact alternate-screen escape that the rejected command must not emit.
expect_file_has "$DIFF_CLEAN_RAW" -aF $'\033[?1049h' "a valid repository did not record its alternate-screen entry"
keys "$S120C" Escape
diff_clean_closed="$(wait_pane 3 "$S120C" -F "dummy_model_name")"
expect_has "$diff_clean_closed" -F "dummy_model_name" "Esc did not close the clean review"
tmux kill-session -t "$S120C" 2>/dev/null

PLAIN_DIR="$(work_dir plain)"
S120N="${S}_diffplain"
launch -c "$PLAIN_DIR" "$S120N" 100 28 "$APP_ABS"
DIFF_NONREPO_RAW="$SMOKE_TMP/diff-nonrepo.raw"
tmux pipe-pane -t "$S120N" -o "cat > $DIFF_NONREPO_RAW"
submit "$S120N" "/diff"
diff_nonrepo="$(wait_pane 8 "$S120N" -iE '^  (/diff|not a git repository)')"
diff_nonrepo_styled="$(pane "$S120N" -e)"
# Wait until the terminal recording reaches the toast before inspecting it:
# a final alternate_on=0 alone misses entering and immediately leaving the
# full-screen review, which visibly flashes even for an invalid directory.
poll 3 grep -aiqE 'git repository|git working tree' "$DIFF_NONREPO_RAW" || fail "the non-Git terminal recording never reached its toast"
tmux pipe-pane -t "$S120N"
dump "outside a Git worktree" "$diff_nonrepo"
expect_has "$diff_nonrepo" -E '^  Not a Git repository\.$' "a non-Git directory did not show only the concise Git info toast"
expect_lacks "$diff_nonrepo" -F "fatal:" "the non-Git toast exposed Git's raw fatal diagnostic"
# Mocha is the fixture's default theme. Info toasts use its dim foreground
# (#7f849c), the same neutral color as the footer; errors use red instead.
if ! printf '%s\n' "$diff_nonrepo_styled" | grep -F 'Not a Git repository.' | grep -qF '38;2;127;132;156'; then
	fail "the non-Git notice is not styled as a neutral info toast"
fi
if grep -aqF $'\033[?1049h' "$DIFF_NONREPO_RAW"; then
	fail "a non-Git /diff entered the alternate screen before rejecting the directory"
fi
expect_has "$diff_nonrepo" -F "dummy_model_name" "a rejected /diff did not leave the conversation usable"
expect_eq "$(tmux display-message -p -t "$S120N" '#{alternate_on}')" "0" "a non-Git directory entered the alternate screen"
[ ! -e "$PLAIN_DIR/.git" ] || fail "/diff initialized a Git repository"
tmux kill-session -t "$S120N" 2>/dev/null
