#!/usr/bin/env bash
# Phase 64 — auto, the default permission mode, then master and manual

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# The permission modes end to end (docs/permissions.md). Every other phase
# runs with ALTER_ZERO_PERMISSION_MODE=manual exported; these launches unset
# it, so the session comes up in the DEFAULT — auto — with the footer saying
# so before any key is pressed. In auto the dummy's "auto permission" demo is
# decided without the user: the read-only listing takes the fast path past
# the classifier and carries no note, the test run is allowed by the offline
# heuristic classifier with a dim '⎿ Allowed by auto mode classifier' row on
# its finished cell, the delete is rejected red 'Denied by auto mode
# classifier', NO prompt ever opens, and nothing is written — the default is
# never saved. One Shift+Tab
# step is master (persisted as "master"), where the same demo runs both
# commands silently; a second is manual, which must be written out now that
# an absent mode means auto, so a relaunch in the same directory comes back
# in manual. Last, a relaunch seeded ALTER_ZERO_PERMISSION_MODE=edit starts in
# edit, and a command rule approved under that seed saves only the allowlist:
# the file still says manual.
S64="${S}_automode"
CFG64="$SMOKE_TMP/cfg64"
PERMS64="$CFG64/permissions.json"
mkdir -p "$CFG64"
ENV64="ALTER_ZERO_TELEMETRY=0 ALTER_ZERO_PROJECT_CONFIG=0 ALTER_ZERO_CONFIG_DIR=$CFG64 ALTER_ZERO_CHECKPOINTS=0 ALTER_ZERO_HISTORY_FILE=/dev/null ALTER_ZERO_STARTUP_DELAY_MS=300"
APP64="env -u ALTER_ZERO_PERMISSION_MODE $ENV64 $BIN"
APP64_EDIT="env ALTER_ZERO_PERMISSION_MODE=edit $ENV64 $BIN"

launch "$S64" 100 44 "$APP64"
start_pane="$(pane "$S64")"
dump "captured pane (a fresh session, no key pressed)" "$start_pane"
expect_has "$start_pane" -E "dummy_model_name · .*auto$" "a session with no saved mode and no seed did not start in auto, the default"

submit "$S64" "auto permission demo"
auto_done="$(wait_pane 15 "$S64" -F "the delete was blocked")" ||
	fail "the auto-mode demo never resolved both classifier verdicts"
dump "captured pane (auto — the classifier decided, no prompt)" "$auto_done"
expect_has "$auto_done" -F "● Bash(ls -la)" "the read-only command's cell is missing"
expect_has "$auto_done" -F "● Bash(pytest -q)" "the classifier-allowed command's cell is missing"
expect_has "$auto_done" -F "⎿  Allowed by auto mode classifier" "the allowed cell lacks its classifier note row"
# The listing never met the classifier: nothing between its cell and the
# test run's may carry the note (docs/permissions.md, the read-only fast path).
listing_cell="$(sed -n '/● Bash(ls -la)/,/● Bash(pytest -q)/p' <<<"$auto_done")"
expect_lacks "$listing_cell" -F "Allowed by auto mode classifier" "the read-only listing went through the classifier"
expect_has "$auto_done" -F "Denied by auto mode classifier" "the delete was not denied by the classifier"
expect_has "$auto_done" -F "Reason:" "the denied cell lacks the classifier's reason line"
if [ -f "$PERMS64" ] && grep -qF '"mode"' "$PERMS64"; then
	fail "starting in the default wrote a mode to permissions.json: $(cat "$PERMS64")"
fi

# One Shift+Tab step from the default: auto → master.
keys "$S64" BTab
master_footer="$(wait_pane 5 "$S64" -E "master\s*$")" ||
	fail "one Shift+Tab step from auto did not land the footer on master"
expect_has "$master_footer" -F "Mode: master" "the master toast is missing"
wait_file 5 "$PERMS64" -F '"mode": "master"' ||
	fail "the master mode did not persist to permissions.json"
submit "$S64" "auto permission demo again"
master_done="$(wait_pane 15 "$S64" -F "all three commands ran to completion")" ||
	fail "the master-mode demo never completed"
dump "captured pane (master — everything ran silently)" "$master_done"

# A second step: master → manual, which is written out explicitly.
keys "$S64" BTab
wait_for 5 "$S64" -E "manual\s*$" ||
	fail "a second Shift+Tab step did not land the footer on manual"
wait_file 5 "$PERMS64" -F '"mode": "manual"' ||
	fail "manual was not written to permissions.json — absent means auto now, so it would read back as auto"
modes_scroll="$(pane "$S64" -S -400)"
expect_lacks "$modes_scroll" -F "Do you want to proceed?" "a permission prompt opened in auto/master mode"
expect_eq "$(grep -cF "Denied by auto mode classifier" <<<"$modes_scroll")" "1" \
	"master mode denied (or re-denied) a command; only auto's single denial may exist"
expect_eq "$(grep -cF "Allowed by auto mode classifier" <<<"$modes_scroll")" "1" \
	"master mode noted a classifier approval; only auto's single note may exist"
keys "$S64" C-c
wait_gone 10 "$S64" || fail "the app did not quit on Ctrl+C"

# The saved manual survives a restart in the same directory.
launch "$S64" 100 44 "$APP64"
restart_pane="$(pane "$S64")"
dump "captured pane (relaunched in the same directory)" "$restart_pane"
expect_has "$restart_pane" -E "dummy_model_name · .*manual$" "a project saved in manual did not come back in manual after a restart"
keys "$S64" C-c
wait_gone 10 "$S64" || fail "the relaunched app did not quit on Ctrl+C"

# A seeded run: the seed outranks the saved manual for this launch alone.
launch "$S64" 100 44 "$APP64_EDIT"
seeded_pane="$(pane "$S64")"
expect_has "$seeded_pane" -E "dummy_model_name · .*edit$" "ALTER_ZERO_PERMISSION_MODE=edit did not seed the run over the saved manual"
# Edit mode covers file changes, not commands, so the demo's listing asks
# (the read-only fast path is auto mode's alone): option 2 saves a rule while
# the gate's live mode is the seed. The test run asks next and is allowed
# once, then the delete is refused.
submit "$S64" "auto permission demo"
wait_for 10 "$S64" -F "Do you want to proceed?" ||
	fail "edit mode did not ask about the demo's listing"
keys "$S64" 2
wait_file 5 "$PERMS64" -F '"ls *"' ||
	fail "option 2 on the listing did not save its rule"
wait_for 10 "$S64" -F "don't ask again for: pytest" ||
	fail "the test run's prompt never opened after the listing's rule"
keys "$S64" 1
wait_for 10 "$S64" -F "don't ask again for: rm" ||
	fail "the delete's prompt never opened after the test run's"
keys "$S64" 3
seeded_done="$(wait_pane 15 "$S64" -F "the delete was blocked")" ||
	fail "the seeded demo never finished"
dump "captured pane (seeded edit — a rule saved, the delete refused)" "$seeded_done"
keys "$S64" C-c
wait_gone 10 "$S64" || fail "the seeded app did not quit on Ctrl+C"
note "permissions.json after the seeded run"
cat "$PERMS64" 2>/dev/null
expect_file_has "$PERMS64" -F '"mode": "manual"' "a rule saved under the seed overwrote the saved mode — the seed reached the file"
if grep -qF '"mode": "edit"' "$PERMS64" 2>/dev/null; then
	fail "the ALTER_ZERO_PERMISSION_MODE seed was saved to permissions.json"
fi
