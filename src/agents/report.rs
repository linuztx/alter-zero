//! What the agent companions tell the model (`docs/agent-tools.md`): one
//! **frame line** saying where an agent stands, over the tool calls it has
//! made — the `bash` family's small grammar (`pty::report`) over an agent.
//!
//! ```text
//! Running (agent a7k2m9x4q) — general-purpose "Fetch weather in Manila" · 48s · 3 tool uses · 16.5k tokens
//! Bash(curl -s https://api.github.com/users/linuztx)
//! Bash(curl -s https://api.github.com/users/linuztx/events/public) ← running
//! ```
//!
//! Pure: every function renders an [`AgentSnapshot`] the registry handed
//! out, so the texts are unit-tested with no thread anywhere.

use std::time::Duration;

use super::{AgentSnapshot, AgentStatus};

/// How many of an agent's calls a report shows — the newest ones, over a
/// line counting the rest. Every report rides into the model's context, so
/// it is bounded below the record's own [`PROGRESS_MAX_CALLS`].
///
/// [`PROGRESS_MAX_CALLS`]: super::PROGRESS_MAX_CALLS
pub const REPORT_MAX_CALLS: usize = 50;

/// The marker a call still in flight wears on its row.
pub const RUNNING_MARKER: &str = " ← running";

/// The line a report shows for an agent that has made no call yet.
pub const NO_CALLS_LINE: &str = "(no tool calls yet)";

/// The heading over a finished agent's final message.
pub const RESPONSE_HEADING: &str = "Response:";

/// The frame line's opening word for each outcome — `Running (agent …)`,
/// `Done (agent …)`, `Failed (agent …)`, `Stopped (agent …)`.
#[must_use]
pub const fn frame_word(status: AgentStatus) -> &'static str {
    match status {
        AgentStatus::Pending | AgentStatus::Running => "Running",
        AgentStatus::Done => "Done",
        AgentStatus::Failed => "Failed",
        AgentStatus::Interrupted => "Stopped",
    }
}

/// The frame line: `{word} (agent {id}) — {type} "{description}" · {elapsed}
/// · {n} tool uses · {tokens} tokens` (the token clause only once a usage
/// frame has landed).
#[must_use]
pub fn frame(snapshot: &AgentSnapshot) -> String {
    format!(
        "{} (agent {}) — {}",
        frame_word(snapshot.status),
        snapshot.id,
        identity_and_counters(snapshot)
    )
}

/// `{type} "{description}" · {elapsed} · {n} tool uses[ · {tokens} tokens]`
/// — what the frame and the list row share.
fn identity_and_counters(snapshot: &AgentSnapshot) -> String {
    format!(
        "{} {:?} · {} · {}",
        snapshot.agent_type,
        snapshot.description,
        elapsed(snapshot.elapsed),
        counters(snapshot)
    )
}

/// `{n} tool uses[ · {tokens} tokens]`.
fn counters(snapshot: &AgentSnapshot) -> String {
    let uses = match snapshot.tool_uses {
        1 => "1 tool use".to_string(),
        n => format!("{n} tool uses"),
    };
    if snapshot.tokens == 0 {
        return uses;
    }
    format!("{uses} · {} tokens", token_count(snapshot.tokens))
}

/// The progress report (`agentoutput`, `agentwait`): the frame over the
/// calls, the newest [`REPORT_MAX_CALLS`] of them, the one still in flight
/// marked; a finished agent's final message under [`RESPONSE_HEADING`], a
/// failed one's error on an `Error:` line.
#[must_use]
pub fn progress(snapshot: &AgentSnapshot) -> String {
    let mut lines = vec![frame(snapshot)];
    let shown = snapshot.calls.len().min(REPORT_MAX_CALLS);
    let skipped = snapshot.omitted_calls + (snapshot.calls.len() - shown);
    if skipped > 0 {
        lines.push(format!(
            "[… {skipped} earlier {} not shown]",
            if skipped == 1 { "call" } else { "calls" }
        ));
    }
    if snapshot.calls.is_empty() {
        lines.push(NO_CALLS_LINE.to_string());
    }
    for call in &snapshot.calls[snapshot.calls.len() - shown..] {
        let mut row = call.header();
        if call.running {
            row.push_str(RUNNING_MARKER);
        }
        lines.push(row);
    }
    match snapshot.status {
        AgentStatus::Done => {
            let body = snapshot.result.as_deref().unwrap_or_default().trim();
            lines.push(RESPONSE_HEADING.to_string());
            lines.push(if body.is_empty() {
                "(no output)".to_string()
            } else {
                body.to_string()
            });
        }
        AgentStatus::Failed => {
            lines.push(format!(
                "Error: {}",
                snapshot.error.as_deref().unwrap_or("unknown").trim()
            ));
        }
        _ => {}
    }
    lines.join("\n")
}

/// `agentlist`: every agent, one row each — `- {id}: {type} "{description}"
/// — {state} {elapsed} · {counters}` — so a model that lost an id to a
/// `/compact` or a `/resume` finds it without guessing.
#[must_use]
pub fn list(snapshots: &[AgentSnapshot]) -> String {
    if snapshots.is_empty() {
        return "No agents have been launched in this session.".to_string();
    }
    let rows: Vec<String> = snapshots
        .iter()
        .map(|snapshot| {
            let state = match snapshot.status {
                AgentStatus::Pending | AgentStatus::Running => "running",
                AgentStatus::Done => "done after",
                AgentStatus::Failed => "failed after",
                AgentStatus::Interrupted => "stopped after",
            };
            format!(
                "- {}: {} {:?} — {state} {} · {}",
                snapshot.id,
                snapshot.agent_type,
                snapshot.description,
                elapsed(snapshot.elapsed),
                counters(snapshot)
            )
        })
        .collect();
    let head = match snapshots.len() {
        1 => "1 agent:".to_string(),
        n => format!("{n} agents:"),
    };
    format!("{head}\n{}", rows.join("\n"))
}

/// The model-facing error for an agent id nothing answers to — naming the
/// agents that exist, so a model that lost track can find its way back
/// rather than guess.
#[must_use]
pub fn unknown_agent(id: &str, snapshots: &[AgentSnapshot]) -> String {
    // An id run together with more — a model's other arguments leaking into
    // the field — still holds the agent it names: say so.
    let mut held = snapshots.iter().filter(|snapshot| {
        id.split(|c: char| !c.is_ascii_alphanumeric())
            .any(|word| word == snapshot.id)
    });
    if let (Some(snapshot), None) = (held.next(), held.next()) {
        return format!(
            "`agent_id` takes the id alone, and {id:?} is not one. Agent {} ({:?}) exists: \
             call again with \"agent_id\": \"{}\".",
            snapshot.id, snapshot.description, snapshot.id
        );
    }
    let head = format!("No agent {id} — it was never launched, or the session was cleared.");
    if snapshots.is_empty() {
        return format!("{head} No agents have been launched.");
    }
    let known: Vec<String> = snapshots
        .iter()
        .map(|snapshot| {
            format!(
                "{} ({:?}, {})",
                snapshot.id,
                snapshot.description,
                frame_word(snapshot.status).to_ascii_lowercase()
            )
        })
        .collect();
    format!("{head} Agents in this session: {}.", known.join(", "))
}

/// `4m 12s`, `58s`, `1h 3m` — the runtime display every roster surface
/// shares (`app::format_elapsed`).
fn elapsed(duration: Duration) -> String {
    crate::app::format_elapsed(duration.as_secs())
}

/// `16.5k`, `1.2M`, `312` — the footer roster's token shape.
fn token_count(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{:.1}k", tokens as f64 / 1_000.0)
    } else {
        tokens.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::super::ProgressCall;
    use super::*;

    fn snapshot(status: AgentStatus) -> AgentSnapshot {
        AgentSnapshot {
            id: "a7k2m9x4q".to_string(),
            agent_type: "general-purpose".to_string(),
            description: "Fetch weather in Manila".to_string(),
            status,
            elapsed: Duration::from_secs(48),
            tool_uses: 3,
            tokens: 16_540,
            calls: vec![
                ProgressCall {
                    name: "Bash".to_string(),
                    args: "curl -s https://api.github.com/users/linuztx".to_string(),
                    running: false,
                },
                ProgressCall {
                    name: "Bash".to_string(),
                    args: "curl -s \"https://api.github.com/users/linuztx/repos?per_page=100&sort=updated\"".to_string(),
                    running: false,
                },
                ProgressCall {
                    name: "Bash".to_string(),
                    args: "curl -s https://api.github.com/users/linuztx/events/public".to_string(),
                    running: status == AgentStatus::Running,
                },
            ],
            omitted_calls: 0,
            result: (status == AgentStatus::Done).then(|| "Manila: 31°C, cloudy.".to_string()),
            error: (status == AgentStatus::Failed).then(|| "boom".to_string()),
        }
    }

    #[test]
    fn a_running_report_is_the_frame_over_the_call_headers() {
        // The user's own example: only the summary — the headers, the one
        // in flight marked — never the half-streamed reply.
        assert_eq!(
            progress(&snapshot(AgentStatus::Running)),
            "Running (agent a7k2m9x4q) — general-purpose \"Fetch weather in Manila\" · 48s · 3 tool uses · 16.5k tokens\n\
             Bash(curl -s https://api.github.com/users/linuztx)\n\
             Bash(curl -s \"https://api.github.com/users/linuztx/repos?per_page=100&sort=updated\")\n\
             Bash(curl -s https://api.github.com/users/linuztx/events/public) ← running"
        );
    }

    #[test]
    fn a_finished_report_closes_on_the_response() {
        let report = progress(&snapshot(AgentStatus::Done));
        assert!(
            report.starts_with("Done (agent a7k2m9x4q) — general-purpose"),
            "{report}"
        );
        assert!(!report.contains("← running"), "{report}");
        assert!(
            report.ends_with("Bash(curl -s https://api.github.com/users/linuztx/events/public)\nResponse:\nManila: 31°C, cloudy."),
            "{report}"
        );
        let mut empty = snapshot(AgentStatus::Done);
        empty.result = Some("  \n".to_string());
        assert!(progress(&empty).ends_with("Response:\n(no output)"));
    }

    #[test]
    fn a_failed_report_names_the_error_and_a_stopped_one_only_the_calls() {
        let failed = progress(&snapshot(AgentStatus::Failed));
        assert!(failed.starts_with("Failed (agent a7k2m9x4q)"), "{failed}");
        assert!(failed.ends_with("\nError: boom"), "{failed}");
        let stopped = progress(&snapshot(AgentStatus::Interrupted));
        assert!(
            stopped.starts_with("Stopped (agent a7k2m9x4q)"),
            "{stopped}"
        );
        assert!(stopped.ends_with("events/public)"), "{stopped}");
    }

    #[test]
    fn a_report_without_calls_says_so_and_without_usage_shows_no_tokens() {
        let mut fresh = snapshot(AgentStatus::Running);
        fresh.calls.clear();
        fresh.tool_uses = 0;
        fresh.tokens = 0;
        assert_eq!(
            progress(&fresh),
            "Running (agent a7k2m9x4q) — general-purpose \"Fetch weather in Manila\" · 48s · 0 tool uses\n\
             (no tool calls yet)"
        );
    }

    #[test]
    fn a_long_record_shows_the_newest_calls_and_counts_the_rest() {
        let mut long = snapshot(AgentStatus::Running);
        long.calls = (0..(REPORT_MAX_CALLS + 5))
            .map(|i| ProgressCall {
                name: "Bash".to_string(),
                args: format!("echo {i}"),
                running: false,
            })
            .collect();
        long.omitted_calls = 7;
        long.tool_uses = long.calls.len() + 7;
        let report = progress(&long);
        let lines: Vec<&str> = report.lines().collect();
        assert_eq!(lines[1], "[… 12 earlier calls not shown]");
        assert_eq!(lines[2], "Bash(echo 5)");
        assert_eq!(lines.len(), 2 + REPORT_MAX_CALLS);
    }

    #[test]
    fn the_list_names_every_agent_with_its_state() {
        let mut done = snapshot(AgentStatus::Done);
        done.id = "a3b4c5d6e".to_string();
        done.agent_type = "explore".to_string();
        done.description = "Find the config loader".to_string();
        done.elapsed = Duration::from_secs(62);
        done.tool_uses = 1;
        done.tokens = 0;
        assert_eq!(
            list(&[snapshot(AgentStatus::Running), done]),
            "2 agents:\n\
             - a7k2m9x4q: general-purpose \"Fetch weather in Manila\" — running 48s · 3 tool uses · 16.5k tokens\n\
             - a3b4c5d6e: explore \"Find the config loader\" — done after 1m 2s · 1 tool use"
        );
        assert!(list(&[snapshot(AgentStatus::Interrupted)]).contains("— stopped after 48s"));
        assert!(list(&[snapshot(AgentStatus::Failed)]).contains("— failed after 48s"));
        assert!(list(&[snapshot(AgentStatus::Failed)]).starts_with("1 agent:\n"));
        assert_eq!(list(&[]), "No agents have been launched in this session.");
    }

    #[test]
    fn an_unknown_id_names_the_agents_that_exist() {
        let known = [snapshot(AgentStatus::Running)];
        let text = unknown_agent("a0000000x", &known);
        assert!(text.starts_with("No agent a0000000x"), "{text}");
        assert!(
            text.contains("a7k2m9x4q (\"Fetch weather in Manila\", running)"),
            "{text}"
        );
        assert!(unknown_agent("a0000000x", &[]).ends_with("No agents have been launched."));
        // An id run together with more still names the agent it holds.
        let text = unknown_agent("agent a7k2m9x4q please", &known);
        assert!(
            text.contains("call again with \"agent_id\": \"a7k2m9x4q\""),
            "{text}"
        );
    }

    #[test]
    fn token_counts_read_like_the_roster() {
        assert_eq!(token_count(312), "312");
        assert_eq!(token_count(16_540), "16.5k");
        assert_eq!(token_count(1_234_567), "1.2M");
    }
}
