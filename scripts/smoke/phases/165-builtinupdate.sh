#!/usr/bin/env bash
# Phase 165 — a release reaches the built-ins an older build seeded

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# docs/builtin-updates.md: an install an older build made holds built-in files
# this release changed. The config home is planted the way that build left
# it. The unedited explore.md must be replaced with this release's version —
# prompts/shipped.sha256 lists the older bytes, so the launch knows nobody
# edited them — and a toast must say so. The edited skill-creator/SKILL.md
# must be kept, this release's version written beside it as SKILL.md.new,
# and an `Edited built-ins` card committed under the banner — once: a second
# launch, the .new deleted after merging, is quiet and writes nothing back.
# Filesystem behaviour on the real binary, past what the unit tests' temp
# directories can see.
S165="${S}_builtinupdate"
BU_HOME="$(mktemp -d)"
BU_CFG="$BU_HOME/.alter-zero"
mkdir -p "$BU_CFG/agents" "$BU_CFG/skills/skill-creator"
# explore.md byte for byte as a development build of 2026-08-27 seeded it —
# one of the versions prompts/shipped.sha256 lists for it.
cat >"$BU_CFG/agents/explore.md" <<'EOF'
---
name: explore
description: Read-only search agent for broad fan-out searches. Use it when answering means sweeping many files, directories or naming conventions and you want the conclusion, not the file dumps — it locates code and reports what it found, and never edits.
# `model:` — a model id to run this type on, or `inherit` for the session's
#   own. See general-purpose.md for the full grammar of every key here.
model: inherit
# `tools:` — the read-only set: no `Write`, no `Edit`. Drop the key entirely to
#   offer every tool instead.
tools: Bash, Read, Skill, mcp__*
---

You are a read-only search agent. Find what the task asks about and report it —
never modify a file, and never suggest that you did.

- Search widely before you read deeply: `rg`/`grep`/`find` to locate candidates,
  then `read` only the parts that decide the question.
- Report file paths with line numbers (`src/app/mod.rs:412`), quote only the
  lines that matter, and say plainly when something does not exist.
- Your final message is the whole result the caller sees: lead with the answer,
  then the evidence. No preamble, no "I will now…".
EOF
cp "$SMOKE_ROOT/prompts/agents/general-purpose.md" "$BU_CFG/agents/general-purpose.md"
# skill-creator's SKILL.md as the user rewrote it, over a record saying the
# edit started from an older version, which this release has since changed.
printf '%s\n' "---" "name: skill-creator" "description: MINE-NOT-YOURS." "---" \
	>"$BU_CFG/skills/skill-creator/SKILL.md"
printf '{"files": {"skill-creator/SKILL.md": "%s"}}\n' \
	d6b0ec10ef9db48a98e16e572bfa017437bc4470d535fd08cc4498439ee7695a \
	>"$BU_CFG/skills/.builtins.json"
APP_BU="env ALTER_ZERO_TELEMETRY=0 ALTER_ZERO_UPDATE_CHECK=0 ALTER_ZERO_TIPS=0 ALTER_ZERO_MASCOT_ANIMATION=0 ALTER_ZERO_PROJECT_CONFIG=0 HOME=$BU_HOME ALTER_ZERO_CONFIG_DIR=$BU_CFG ALTER_ZERO_CHECKPOINTS=0 ALTER_ZERO_HISTORY_FILE=/dev/null ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN"

tmux kill-session -t "$S165" 2>/dev/null
launch "$S165" 100 30 "$APP_BU"
bu_first="$(wait_pane 5 "$S165" -F "Updated the built-in explore agent")"
dump "the first launch of the newer release" "$bu_first"
expect_has "$bu_first" -F "Updated the built-in explore agent" \
	"no toast for the agent brought up to date"
expect_has "$bu_first" -F "Edited built-ins" "no card for the edited built-in"
expect_has "$bu_first" -F "~/.alter-zero/skills/skill-creator/SKILL.md.new" \
	"the card does not name the .new it wrote"
submit "$S165" "/quit"
sleep 0.6
tmux kill-session -t "$S165" 2>/dev/null

if ! cmp -s "$SMOKE_ROOT/prompts/agents/explore.md" "$BU_CFG/agents/explore.md"; then
	fail "the unedited explore.md an older build wrote was not replaced with this release's"
fi
if ! cmp -s "$SMOKE_ROOT/prompts/agents/general-purpose.md" "$BU_CFG/agents/general-purpose.md"; then
	fail "general-purpose.md, already this release's, was changed"
fi
if ! grep -q "MINE-NOT-YOURS" "$BU_CFG/skills/skill-creator/SKILL.md" 2>/dev/null; then
	fail "the launch wrote over an edited SKILL.md"
fi
if ! cmp -s "$SMOKE_ROOT/prompts/skills/skill-creator/SKILL.md" "$BU_CFG/skills/skill-creator/SKILL.md.new"; then
	fail "this release's SKILL.md was not written beside the edited one"
fi
# Every other built-in was missing, so it was seeded as before.
if ! cmp -s "$SMOKE_ROOT/prompts/skills/skill-creator/reference.md" "$BU_CFG/skills/skill-creator/reference.md"; then
	fail "a missing built-in file was not seeded beside the edited one"
fi
for record in "$BU_CFG/skills/.builtins.json" "$BU_CFG/agents/.builtins.json"; do
	expect_file_has "$record" -F '"files"' "the launch wrote no record at $record"
done

# The user merges and deletes the .new: the next launch has nothing to say,
# and does not write it again.
rm -f "$BU_CFG/skills/skill-creator/SKILL.md.new"
launch "$S165" 100 30 "$APP_BU"
bu_second="$(pane "$S165")"
dump "the second launch" "$bu_second"
expect_lacks "$bu_second" -F "Edited built-ins" "the card came back on the second launch"
expect_lacks "$bu_second" -F "Updated the built-in" "the second launch announced an update again"
submit "$S165" "/quit"
sleep 0.6
tmux kill-session -t "$S165" 2>/dev/null
if [ -e "$BU_CFG/skills/skill-creator/SKILL.md.new" ]; then
	fail "the .new the user deleted was written again"
fi
if ! grep -q "MINE-NOT-YOURS" "$BU_CFG/skills/skill-creator/SKILL.md" 2>/dev/null; then
	fail "the second launch wrote over the edited SKILL.md"
fi
rm -rf "$BU_HOME"
note "an unedited built-in is replaced, an edited one kept with this release's version beside it, once"
