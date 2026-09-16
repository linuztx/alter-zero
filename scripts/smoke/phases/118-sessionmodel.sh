#!/usr/bin/env bash
# Phase 118 — PER-SESSION model: a conversation resumes on the model it ran on

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# PER-SESSION model (docs/session-model.md). config.json's directory entry is
# what a NEW session starts on; every conversation's rollout records the
# model it actually runs on (a `model` sidecar line, the newest winning), and
# a resume brings THAT back. Two instances in one directory: (a) the first
# runs the directory's entry and records it; (b) the second, pinned to
# another model by ALTER_ZERO_MODEL, records its own — and the pin never
# reaches config.json. (c) The entry then moves, as a /model pick in a third
# instance would move it — but first, (b2) `--resume {path}` of the first
# conversation while the entry still IS its model restores nothing and
# appends no redundant line. (d) `--resume {path}` comes back on the first
# session's model, not the entry's, and writes nothing to config.json;
# (e) `--continue` — the newest conversation here — on the second's;
# (f) a fresh launch on the moved entry, and the /resume picker inside it
# on the first's; (g) an env pin over `--resume` keeps the pinned model;
# (h) a record naming a provider this machine cannot reach keeps the launch's
# model under the red `Can't resume on …` toast; (i) a rollout with no
# record at all — a file from before sessions kept their model — resumes on
# the directory's entry, silently, and then records it.
S118="${S}_sessmodel"
SM_CFG="$(mktemp -d "$SMOKE_TMP/sessmodel-cfg.XXXXXX")"
SM_DIR="$(work_dir sessmodel)"
# The path as the app keys config.json — the kernel's cwd, symlinks resolved.
SM_KEY="$(cd "$SM_DIR" && pwd -P)"
cat >"$SM_CFG/providers.toml" <<'PROVIDERS'
[providers.deadend]
name = "Dead End"
api_model_base = "http://127.0.0.1:9/v1"

[providers.deadend.kwargs]
api_base = "http://127.0.0.1:9/v1"
PROVIDERS
# A key resolves for the provider (Phase 65's trick), so a saved selection
# activates a real backend whose model name the footer shows. No request is
# ever answered: the turns below are `!` shell commands, which run locally,
# and the capability probe fails fast on the dead port.
cat >"$SM_CFG/config.json" <<'CONFIG'
{ "provider": "deadend", "model": "model-a" }
CONFIG
SM_ENV="ALTER_ZERO_TELEMETRY=0 ALTER_ZERO_UPDATE_CHECK=0 ALTER_ZERO_PROJECT_CONFIG=0 ALTER_ZERO_CONFIG_DIR=$SM_CFG ALTER_ZERO_SESSIONS_DIR=$SM_CFG/sessions ALTER_ZERO_HISTORY_FILE=/dev/null ALTER_ZERO_CHECKPOINTS=0 ALTER_ZERO_SKILLS_DIR=$SMOKE_SKILLS ALTER_ZERO_AGENTS_DIR=$SMOKE_AGENTS ALTER_ZERO_PROVIDERS_FILE=$SM_CFG/providers.toml ALTER_ZERO_API_KEY=not-a-real-key ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS"
APP_SM="env $SM_ENV $BIN_ABS"

# sm_quit SESSION — /quit the instance and drop its tmux session.
sm_quit() {
	tmux send-keys -t "$1" -l "/quit"
	sleep 0.3
	tmux send-keys -t "$1" Enter
	sleep 0.6
	tmux kill-session -t "$1" 2>/dev/null
}
# sm_rollouts — every rollout recorded so far, one path per line.
sm_rollouts() { find "$SM_CFG/sessions" -type f -name 'rollout-*.jsonl' 2>/dev/null | sort; }
# sm_model_lines FILE — the file's `model` sidecar lines.
sm_model_lines() { grep -F '"type":"model"' "$1" || true; }

# (a) The first instance: the directory's entry, recorded with the file.
launch -c "$SM_DIR" "$S118" 100 30 "$APP_SM"
sm_a="$(wait_pane 5 "$S118" -F "model-a")"
note "instance 1 on the directory's entry"
printf '%s\n' "$sm_a"
expect_has "$sm_a" -F "model-a" "instance 1 did not start on the directory's config.json entry"
submit "$S118" "!echo first"
sm_a_turn="$(wait_pane 10 "$S118" -E "⎿ +first")"
expect_has "$sm_a_turn" -E "⎿ +first" "instance 1's shell turn never ran (no rollout to record the model into)"
sm_quit "$S118"
SM_F1="$(sm_rollouts | head -1)"
expect_eq "$(sm_rollouts | wc -l | tr -d ' ')" "1" "instance 1 should have recorded exactly one rollout"
note "instance 1's model lines"
sm_model_lines "$SM_F1"
expect_has "$(sm_model_lines "$SM_F1")" -F '"provider":"deadend","model":"model-a"' "instance 1's rollout does not record its model"

# (b) A second instance in the SAME directory, pinned to another model.
launch -c "$SM_DIR" "$S118" 100 30 "env ALTER_ZERO_MODEL=model-b $APP_SM"
sm_b="$(wait_pane 5 "$S118" -F "model-b")"
note "instance 2 pinned to model-b in the same directory"
printf '%s\n' "$sm_b"
expect_has "$sm_b" -F "model-b" "instance 2 did not start on the ALTER_ZERO_MODEL pin"
submit "$S118" "!echo second"
sm_b_turn="$(wait_pane 10 "$S118" -E "⎿ +second")"
expect_has "$sm_b_turn" -E "⎿ +second" "instance 2's shell turn never ran"
sm_quit "$S118"
expect_eq "$(sm_rollouts | wc -l | tr -d ' ')" "2" "instance 2 should have recorded a second rollout"
SM_F2="$(sm_rollouts | grep -vF "$SM_F1" | head -1)"
note "instance 2's model lines"
sm_model_lines "$SM_F2"
expect_has "$(sm_model_lines "$SM_F2")" -F '"model":"model-b"' "instance 2's rollout does not record its own model"
expect_has "$(sm_model_lines "$SM_F1")" -F '"model":"model-a"' "instance 2 rewrote instance 1's record"
expect_lacks "$(cat "$SM_CFG/config.json")" -F "model-b" "an environment pin reached config.json (env wins for the run, never sticks)"

# (b2) Resuming the first conversation while the entry still IS its model:
# already running exactly the record, so nothing is rebuilt and no line is
# appended.
launch -c "$SM_DIR" "$S118" 100 30 "$APP_SM --resume $SM_F1"
sm_b2="$(wait_pane 5 "$S118" -F "model-a")"
note "--resume of instance 1's conversation on the entry's own model"
printf '%s\n' "$sm_b2"
expect_has "$sm_b2" -F "model-a" "--resume onto the model already running did not keep it"
expect_has "$sm_b2" -F "! echo first" "--resume did not repaint the first conversation"
sm_quit "$S118"
expect_eq "$(sm_model_lines "$SM_F1" | wc -l | tr -d ' ')" "1" "a resume onto the model already running appended a redundant model line"

# (c) The directory's entry moves on — as a /model pick in another instance
# would move it — to a model neither conversation ever ran.
cat >"$SM_CFG/config.json" <<CONFIG
{
  "provider": "deadend",
  "model": "model-c",
  "projects": {
    "$SM_KEY": { "provider": "deadend", "model": "model-c" }
  }
}
CONFIG

# (d) --resume {path}: the first conversation, on ITS model.
launch -c "$SM_DIR" "$S118" 100 30 "$APP_SM --resume $SM_F1"
sm_d="$(wait_pane 5 "$S118" -F "model-a")"
note "--resume of instance 1's conversation"
printf '%s\n' "$sm_d"
expect_has "$sm_d" -F "model-a" "--resume did not bring the conversation back on its own model (the directory's entry is model-c)"
expect_has "$sm_d" -F "! echo first" "--resume did not repaint the first conversation"
expect_lacks "$sm_d" -F "model-c" "--resume ran the directory's current entry instead of the session's model"
sm_quit "$S118"
expect_lacks "$(cat "$SM_CFG/config.json")" -F "model-a" "a resume wrote the session's model over the directory's entry (a resume chooses nothing)"
expect_eq "$(sm_model_lines "$SM_F1" | wc -l | tr -d ' ')" "1" "a resume onto the recorded model appended a redundant model line"

# (e) --continue: the newest conversation recorded here — the second — on ITS model.
launch -c "$SM_DIR" "$S118" 100 30 "$APP_SM --continue"
sm_e="$(wait_pane 5 "$S118" -F "model-b")"
note "--continue (the newest conversation in this directory)"
printf '%s\n' "$sm_e"
expect_has "$sm_e" -F "model-b" "--continue did not bring the second conversation back on its own model"
expect_has "$sm_e" -F "! echo second" "--continue did not reopen the newest conversation"
sm_quit "$S118"

# (f) A fresh launch starts on the moved entry; the /resume picker inside it
# brings the first conversation back on the first's model.
launch -c "$SM_DIR" "$S118" 100 30 "$APP_SM"
sm_f="$(wait_pane 5 "$S118" -F "model-c")"
note "a fresh launch after the entry moved"
printf '%s\n' "$sm_f"
expect_has "$sm_f" -F "model-c" "a fresh launch did not start on the directory's current entry"
tmux send-keys -t "$S118" -l "/resume"
sleep 0.3
tmux send-keys -t "$S118" Enter
sm_picker="$(wait_pane 5 "$S118" -F "R E S U M E")"
expect_has "$sm_picker" -F "R E S U M E" "/resume did not open the session picker"
tmux send-keys -t "$S118" -l "first"
sleep 0.4
tmux send-keys -t "$S118" Enter
sm_f2="$(wait_pane 5 "$S118" -F "model-a")"
note "the /resume picker's pick, mid-session"
printf '%s\n' "$sm_f2"
expect_has "$sm_f2" -F "model-a" "the /resume picker did not switch the session onto the resumed conversation's model"
expect_has "$sm_f2" -F "! echo first" "the /resume picker did not load the first conversation"
sm_quit "$S118"

# (g) An environment pin outranks the record — the startup precedence.
launch -c "$SM_DIR" "$S118" 100 30 "env ALTER_ZERO_MODEL=model-p $APP_SM --resume $SM_F1"
sm_g="$(wait_pane 5 "$S118" -F "model-p")"
note "--resume under an ALTER_ZERO_MODEL pin"
printf '%s\n' "$sm_g"
expect_has "$sm_g" -F "model-p" "an ALTER_ZERO_MODEL pin did not outrank the session's recorded model"
sm_quit "$S118"
expect_eq "$(sm_model_lines "$SM_F1" | wc -l | tr -d ' ')" "1" "a pinned run wrote the pin into the session's record (env never sticks)"

# (h) A record this machine cannot honour: the provider is not in the table.
SM_F3="$SM_CFG/ghost.jsonl"
sed 's/"provider":"deadend","model":"model-a"/"provider":"nowhere","model":"ghost"/' "$SM_F1" >"$SM_F3"
expect_has "$(sm_model_lines "$SM_F3")" -F '"model":"ghost"' "the ghost rollout was not rewritten (the phase would test nothing)"
launch -c "$SM_DIR" "$S118" 100 30 "$APP_SM --resume $SM_F3"
sm_h="$(wait_pane 4 "$S118" -F "Can't resume on ghost")"
note "--resume of a record naming an unreachable provider"
printf '%s\n' "$sm_h"
expect_has "$sm_h" -F "Can't resume on ghost" "no toast said the recorded model could not be restored"
expect_has "$sm_h" -F "model-c" "the session did not stay on the launch's own model when the record was unusable"
sm_quit "$S118"
expect_has "$(sm_model_lines "$SM_F3")" -F '"model":"ghost"' "an unusable record was overwritten with the fallback model (a forced fallback is not a choice)"
expect_eq "$(sm_model_lines "$SM_F3" | wc -l | tr -d ' ')" "1" "an unusable record gained a model line"

# (i) A rollout with no record — a file from before sessions kept their
# model: the session stays on the directory's entry, says nothing, and the
# file then records what it came back on so the NEXT resume has an answer.
SM_F4="$SM_CFG/stripped.jsonl"
grep -vF '"type":"model"' "$SM_F1" >"$SM_F4"
expect_eq "$(sm_model_lines "$SM_F4" | wc -l | tr -d ' ')" "0" "the stripped rollout still carries a model line (the phase would test nothing)"
launch -c "$SM_DIR" "$S118" 100 30 "$APP_SM --resume $SM_F4"
sm_i="$(wait_pane 5 "$S118" -F "model-c")"
note "--resume of a rollout recorded before sessions kept their model"
printf '%s\n' "$sm_i"
expect_has "$sm_i" -F "model-c" "a rollout with no record did not resume on the directory's entry"
expect_has "$sm_i" -F "! echo first" "the stripped rollout's conversation was not loaded"
expect_lacks "$sm_i" -F "Can't resume" "a rollout with no record raised a restore failure (there was nothing to restore)"
sm_quit "$S118"
note "the stripped rollout's model lines after the resume"
sm_model_lines "$SM_F4"
expect_eq "$(sm_model_lines "$SM_F4" | wc -l | tr -d ' ')" "1" "a rollout with no record did not record the model it came back on"
expect_has "$(sm_model_lines "$SM_F4")" -F '"model":"model-c"' "the stripped rollout recorded a model other than the one the session ran"
