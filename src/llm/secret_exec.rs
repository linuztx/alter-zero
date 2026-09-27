//! Secrets at the tool boundary (`docs/secrets.md`): the seam every model
//! tool call passes through on its way to the executor — the main turn's
//! and a subagent's alike — where `<secrete:NAME>` placeholders become
//! values and values become placeholders again.
//!
//! [`run_with_secrets`] wraps one call: the arguments of a tool that acts
//! are expanded ([`expand_call`]) for the executor alone, every progress
//! report the tool streams is redacted on its way out, and so is the
//! outcome ([`redact_outcome`]) — the cell, the model's `tool` message, the
//! `PostToolUse` payload and the rollout all read the redacted text. It sits
//! **after** the classifier's action log and the `ToolStart` event, so both
//! keep the model's own placeholders.

use std::borrow::Cow;

use super::exec::ToolProgress;
use super::tools::{BASH_SEND_TOOL, BASH_SESSION_TOOL_NAME, ToolCallRequest, ToolOutcome};
use crate::secrets::{SecretRegistry, StreamRedactor, expands_placeholders};

/// `call` with its placeholders expanded — `None` when the tool does not
/// act ([`crate::secrets::expands_placeholders`]) or nothing changed, so the
/// executor runs the model's own call.
#[must_use]
pub fn expand_call(secrets: &SecretRegistry, call: &ToolCallRequest) -> Option<ToolCallRequest> {
    // Keys typed into a session are the one input read for notation —
    // `<Enter>`, `\n`, `&lt;` — so its placeholders are expanded where it is
    // typed, inside the text that notation leaves (`llm::exec`): expanded
    // here, a value holding `\n` or `<Up>` would be read as keys.
    if !expands_placeholders(&call.name)
        || call.name == BASH_SEND_TOOL
        || call.name == BASH_SESSION_TOOL_NAME
    {
        return None;
    }
    let arguments = secrets.expand_arguments(&call.arguments)?;
    Some(ToolCallRequest {
        id: call.id.clone(),
        name: call.name.clone(),
        arguments,
    })
}

/// `outcome` with every value in its texts redacted: the displayed `output`
/// (as a cut tail when the tool truncated it — a cut can leave a value's
/// first half behind) and the model-facing `context`.
#[must_use]
pub fn redact_outcome(secrets: &SecretRegistry, mut outcome: ToolOutcome) -> ToolOutcome {
    secrets.with(|store| {
        let output = if outcome.truncated {
            store.redact_cut_tail(&outcome.output)
        } else {
            store.redact(&outcome.output)
        };
        if let Cow::Owned(output) = output {
            outcome.output = output;
        }
        let context = outcome
            .context
            .as_deref()
            .and_then(|context| match store.redact(context) {
                Cow::Borrowed(_) => None,
                Cow::Owned(context) => Some(context),
            });
        if context.is_some() {
            outcome.context = context;
        }
    });
    outcome
}

/// A permission request built from an expanded call, redacted before
/// anything reads it: the target, the preview body and the detail line —
/// so the prompt, the allowlist and the auto-mode classifier see the
/// placeholders the user approves, never the values
/// (`llm::approval::approve_call`).
pub fn redact_request(
    secrets: &SecretRegistry,
    request: &mut crate::permission::PermissionRequest,
) {
    secrets.with(|store| {
        for text in [&mut request.target, &mut request.body]
            .into_iter()
            .chain(request.detail.as_mut())
        {
            if let Cow::Owned(redacted) = store.redact(text) {
                *text = redacted;
            }
        }
    });
}

/// Run one tool call with the session's secrets: `run` receives the
/// expanded call and a progress sink that redacts what it is handed —
/// `settled` text through a [`crate::secrets::StreamRedactor`], so a value
/// split across two reports is still caught, the `live` rows with the held
/// tail in front of them, a refined title whole — and the outcome is
/// redacted on the way back. With no secrets it is `run` itself.
pub fn run_with_secrets(
    secrets: Option<&SecretRegistry>,
    call: &ToolCallRequest,
    on_output: &mut dyn FnMut(ToolProgress<'_>),
    run: impl FnOnce(&ToolCallRequest, &mut dyn FnMut(ToolProgress<'_>)) -> ToolOutcome,
) -> ToolOutcome {
    let Some(secrets) = secrets.filter(|secrets| !secrets.is_empty()) else {
        return run(call, on_output);
    };
    let expanded = expand_call(secrets, call);
    let call = expanded.as_ref().unwrap_or(call);
    let mut stream = StreamRedactor::default();
    let mut redacting = |progress: ToolProgress<'_>| match progress {
        ToolProgress::Screen { settled, live } => {
            // The held tail is text already reported as settled but not yet
            // decided: it shows ahead of the live rows, masked when it is a
            // long enough piece of a value, until the next report decides it.
            let (settled, live) = secrets.with(|store| {
                let settled = stream.push(store, settled);
                let live = store
                    .redact_cut_tail(&format!("{}{live}", stream.held()))
                    .into_owned();
                (settled, live)
            });
            on_output(ToolProgress::Screen {
                settled: &settled,
                live: &live,
            });
        }
        ToolProgress::Title(title) => on_output(ToolProgress::Title(&secrets.redact(title))),
    };
    let outcome = run(call, &mut redacting);
    redact_outcome(secrets, outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::{SecretDraft, SecretStore, SecretValue};

    const PASSWORD: &str = "hunter22";
    const TOKEN: &str = "sk-live-0123456789";

    fn secrets() -> SecretRegistry {
        let mut store = SecretStore::new();
        for (name, value) in [("ROOT_PASSWORD", PASSWORD), ("TOKEN", TOKEN)] {
            store
                .apply(&SecretDraft {
                    original: None,
                    name: name.into(),
                    value: Some(SecretValue::new(value)),
                    context: String::new(),
                })
                .unwrap();
        }
        SecretRegistry::new(store)
    }

    fn call(name: &str, arguments: &str) -> ToolCallRequest {
        ToolCallRequest {
            id: "call_1".into(),
            name: name.into(),
            arguments: arguments.into(),
        }
    }

    /// Every progress report `run_with_secrets` passed on, owned.
    #[derive(Debug, Default, PartialEq)]
    struct Seen {
        screens: Vec<(String, String)>,
        titles: Vec<String>,
    }

    fn sink(seen: &mut Seen) -> impl FnMut(ToolProgress<'_>) + '_ {
        move |progress| match progress {
            ToolProgress::Screen { settled, live } => {
                seen.screens.push((settled.to_string(), live.to_string()));
            }
            ToolProgress::Title(title) => seen.titles.push(title.to_string()),
        }
    }

    #[test]
    fn an_acting_tool_runs_on_the_expanded_arguments() {
        let secrets = secrets();
        let mut ran = None;
        let mut seen = Seen::default();
        run_with_secrets(
            Some(&secrets),
            &call(
                "bash",
                r#"{"command":"echo <secrete:ROOT_PASSWORD> | sudo -S id"}"#,
            ),
            &mut sink(&mut seen),
            |call, _| {
                ran = Some(call.clone());
                ToolOutcome::ok("")
            },
        );
        let ran = ran.unwrap();
        let args: serde_json::Value = serde_json::from_str(&ran.arguments).unwrap();
        assert_eq!(args["command"], "echo hunter22 | sudo -S id");
        assert_eq!(ran.id, "call_1");
        assert_eq!(ran.name, "bash");
    }

    #[test]
    fn a_tool_that_shows_its_arguments_is_never_expanded() {
        let secrets = secrets();
        for name in ["askuserquestion", "agent", "skill", "taskcreate"] {
            let original = call(name, r#"{"prompt":"use <secrete:TOKEN>"}"#);
            let mut ran = None;
            let mut seen = Seen::default();
            run_with_secrets(
                Some(&secrets),
                &original,
                &mut sink(&mut seen),
                |call, _| {
                    ran = Some(call.clone());
                    ToolOutcome::ok("")
                },
            );
            assert_eq!(ran.unwrap(), original, "{name}");
        }
    }

    #[test]
    fn every_way_output_leaves_the_tool_is_redacted() {
        let secrets = secrets();
        let mut seen = Seen::default();
        let outcome = run_with_secrets(
            Some(&secrets),
            &call(
                "bashsend",
                r#"{"session_id":"b1","input":"<secrete:ROOT_PASSWORD>"}"#,
            ),
            &mut sink(&mut seen),
            |_, on_output| {
                on_output(ToolProgress::Title("sudo ← hunter22⏎"));
                on_output(ToolProgress::Screen {
                    settled: "echoed hunter22\n",
                    live: "token sk-live-0123456789 $ ",
                });
                ToolOutcome::error("wrong password hunter22")
                    .with_context(format!("model reads {TOKEN}"))
            },
        );
        assert_eq!(seen.titles, ["sudo ← <secrete:ROOT_PASSWORD>⏎"]);
        assert_eq!(
            seen.screens,
            [(
                "echoed <secrete:ROOT_PASSWORD>\n".to_string(),
                "token <secrete:TOKEN> $ ".to_string()
            )]
        );
        assert_eq!(outcome.output, "wrong password <secrete:ROOT_PASSWORD>");
        assert_eq!(
            outcome.context.as_deref(),
            Some("model reads <secrete:TOKEN>")
        );
        assert!(!outcome.ok);
    }

    #[test]
    fn a_value_split_across_two_reports_is_still_redacted() {
        let secrets = secrets();
        let mut seen = Seen::default();
        run_with_secrets(
            Some(&secrets),
            &call("bash", r#"{"command":"cat .env"}"#),
            &mut sink(&mut seen),
            |_, on_output| {
                on_output(ToolProgress::Screen {
                    settled: "TOKEN=sk-live-01",
                    live: "",
                });
                on_output(ToolProgress::Screen {
                    settled: "23456789\n",
                    live: "",
                });
                ToolOutcome::ok(format!("TOKEN={TOKEN}"))
            },
        );
        let settled: String = seen.screens.iter().map(|(s, _)| s.as_str()).collect();
        assert_eq!(settled, "TOKEN=<secrete:TOKEN>\n");
        for (settled, live) in &seen.screens {
            assert!(!settled.contains("sk-live-01"), "{settled}");
            // The held piece rides the live rows — masked, never shown.
            assert!(!live.contains("sk-live-01"), "{live}");
        }
    }

    #[test]
    fn a_truncated_output_masks_the_piece_of_a_value_it_cut() {
        let secrets = secrets();
        let mut seen = Seen::default();
        let outcome = run_with_secrets(
            Some(&secrets),
            &call("read", r#"{"path":"/tmp/big"}"#),
            &mut sink(&mut seen),
            |_, _| ToolOutcome::ok("…TOKEN=sk-live-012").with_truncated(true),
        );
        assert_eq!(outcome.output, "…TOKEN=<secrete:TOKEN>");
        assert!(outcome.truncated);
    }

    #[test]
    fn without_secrets_the_call_passes_straight_through() {
        let original = call("bash", r#"{"command":"echo <secrete:TOKEN>"}"#);
        for secrets in [None, Some(SecretRegistry::default())] {
            let mut ran = None;
            let mut seen = Seen::default();
            let outcome = run_with_secrets(
                secrets.as_ref(),
                &original,
                &mut sink(&mut seen),
                |call, on_output| {
                    ran = Some(call.clone());
                    on_output(ToolProgress::Title("t"));
                    ToolOutcome::ok("out")
                },
            );
            assert_eq!(ran.unwrap(), original);
            assert_eq!(outcome, ToolOutcome::ok("out"));
            assert_eq!(seen.titles, ["t"]);
        }
    }

    #[test]
    fn expand_call_only_rewrites_what_it_must() {
        let secrets = secrets();
        let expanded = expand_call(
            &secrets,
            &call(
                "write",
                r#"{"path":"/x/.env","content":"KEY=<secrete:TOKEN>\n"}"#,
            ),
        )
        .unwrap();
        let args: serde_json::Value = serde_json::from_str(&expanded.arguments).unwrap();
        assert_eq!(args["content"], format!("KEY={TOKEN}\n"));
        assert_eq!(
            expand_call(
                &secrets,
                &call("write", r#"{"path":"/x","content":"plain"}"#)
            ),
            None
        );
        assert_eq!(
            expand_call(&secrets, &call("agent", r#"{"prompt":"<secrete:TOKEN>"}"#)),
            None
        );
        // Typed input is expanded where it is typed — after its key notation
        // is read — so a value is always text (`llm::exec::run_session`).
        assert_eq!(
            expand_call(
                &secrets,
                &call(
                    "bashsend",
                    r#"{"session_id":"b1","input":"<secrete:TOKEN><Enter>"}"#
                )
            ),
            None
        );
    }

    #[test]
    fn an_mcp_call_is_expanded_too() {
        let secrets = secrets();
        let expanded = expand_call(
            &secrets,
            &call(
                "mcp__github__create_issue",
                r#"{"token":"<secrete:TOKEN>"}"#,
            ),
        )
        .unwrap();
        assert!(expanded.arguments.contains(TOKEN));
    }
}
