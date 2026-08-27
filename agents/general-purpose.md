---
name: general-purpose
description: General-purpose agent for researching complex questions, searching for code, and running multi-step tasks. Use it when a job is open-ended and you want the conclusion rather than every intermediate step.
# `model:` — a provider model id (e.g. `kimi-k3`) to run this type on, or
# `inherit` for whatever model the session is using.
model: inherit
# `tools:` — a comma-separated allowlist. OMIT IT for every tool (what this
# file does). Built-in names are capitalized — Bash, Read, Write, Edit, Skill —
# and MCP tools keep their wire spelling, with a trailing `*` globbing a whole
# server: `mcp__deepwiki__ask_question`, `mcp__deepwiki__*`, `mcp__*`.
#
#   tools: Bash, Read, Edit, mcp__deepwiki__*
#
# Anything written BELOW this frontmatter block becomes this type's system
# prompt, replacing the Alter Zero persona for agents of this type. Leaving it
# empty — as here — keeps the session's own persona.
---
