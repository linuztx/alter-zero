#!/usr/bin/env bash
# Phase 53 — the Agent tool

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# the Agent tool (docs/agent-tool.md) — the dummy's scripted
# two-agent demo. A prompt mentioning "agents" announces a foreground group:
# while it "runs" (AGENT_DELAY) the strip shows the blue `● Running 2 agents…`
# tree with `⎿ Initializing…` per agent AND the footer roster lists `● main` +
# two `◯ general-purpose …` rows; at resolution the committed
# `● 2 agents finished (ctrl+o to expand)` tree lands with `⎿ Done` rows; the
# Ctrl+O transcript expands each as `● Agent({description})` with its
# `⎿ Prompt:` block and `⎿ Done (…)` footer; and after the linger the roster
# rows sweep away.
S53="${S}_agents"
launch "$S53" 100 44
submit "$S53" "call agents for the weather"
agents_live=""
for _ in $(seq 1 120); do # the group "runs" for AGENT_DELAY (1.6s)
	cap="$(tmux capture-pane -t "$S53" -p)"
	if printf '%s' "$cap" | grep -qF "Running 2 agents…" &&
		printf '%s' "$cap" | grep -qF "● main" &&
		printf '%s' "$cap" | grep -qF "Initializing…"; then
		agents_live="$cap"
		break
	fi
	sleep 0.05
done
echo "==== Phase 53: captured pane (live agent group tree + footer roster) ===="
printf '%s\n' "$agents_live"
agents_done=""
for _ in $(seq 1 200); do # the resolution + the closing text
	cap="$(tmux capture-pane -t "$S53" -p)"
	if printf '%s' "$cap" | grep -qF "2 agents finished (ctrl+o to expand)"; then
		agents_done="$cap"
		break
	fi
	sleep 0.05
done
echo "==== Phase 53: captured pane (committed agent group cell) ===="
printf '%s\n' "$agents_done"
# The Ctrl+O transcript expands each agent with its prompt + Done footer.
tmux send-keys -t "$S53" C-o
sleep 0.5
agents_overlay="$(tmux capture-pane -t "$S53" -p)"
echo "==== Phase 53: captured pane (Ctrl+O agent cells) ===="
printf '%s\n' "$agents_overlay"
tmux send-keys -t "$S53" q
sleep 0.4
# The roster lingers after the group settles, then sweeps.
agents_swept=""
for _ in $(seq 1 900); do # AGENT_LINGER is 30s
	cap="$(tmux capture-pane -t "$S53" -p)"
	if ! printf '%s' "$cap" | grep -qF "● main"; then
		agents_swept="$cap"
		break
	fi
	sleep 0.05
done
tmux kill-session -t "$S53" 2>/dev/null
echo "==== Phase 53: the Agent tool — live tree, committed cell, Ctrl+O expansion, roster sweep ===="
if [ -z "$agents_live" ]; then
	fail "the live agent group tree + footer roster never showed"
fi
expect_has "$agents_done" -F "├ Fetch current weather and time in Warsaw" "the committed group cell lacks the Warsaw tree row"
expect_has "$agents_done" -F "⎿  Done" "the committed group cell lacks the ⎿ Done status rows"
expect_has "$agents_overlay" -F "● Agent(Fetch current weather and time in Warsaw)" "the Ctrl+O transcript lacks the expanded Agent cell"
expect_has "$agents_overlay" -F "⎿  Prompt:" "the Ctrl+O Agent cell lacks its Prompt: block"
if [ -z "$agents_swept" ]; then
	fail "the finished agents never swept off the roster"
fi
