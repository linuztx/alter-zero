---
name: general-purpose
description: General-purpose agent for researching complex questions, searching for code, and running multi-step tasks. Use it when a job is open-ended and you want the conclusion rather than every intermediate step.
# `model:` — any model id this session's provider serves (`kimi-k3`,
#   `claude-sonnet-5`, …), or `inherit` for the session's own model. `inherit`
#   is the default, written out so the knob is visible rather than remembered.
model: inherit
# `tools:` — a comma-separated allowlist of what this type may call. OMIT IT
#   for every tool the session has, which is what this file does.
#
#     tools: Bash, Read, Edit, Skill, mcp__deepwiki__*
#
#   Built-in names are capitalized (`Bash` matches the `bash` tool — the match
#   folds case); MCP tools keep their wire spelling, and a trailing `*` globs,
#   so `mcp__deepwiki__*` takes one server and `mcp__*` takes them all. Leaving
#   `Write` and `Edit` out is how a type is made read-only — the allowlist is
#   enforced when a call runs, not only when the tools are offered. The `Agent`
#   tool is never available whatever this says: subagents do not nest.
#
# Anything written BELOW the closing `---` becomes this type's system prompt,
# replacing the Alter Zero persona for agents of this type (the environment and
# scratchpad blocks still ride along). There is nothing below it here, so this
# type keeps the session's own persona.
---
