//! The dummy's **gated** turns: the offline permission demos
//! (`docs/permissions.md`) and the `AskUserQuestion` demo (`docs/ask.md`).
//!
//! Unlike [`super::turns`] these can't be a pure event list — they *ask*, and
//! then block on the shared [`PermissionGate`](crate::permission::PermissionGate)
//! (or the [`AskGate`](crate::ask::AskGate)) exactly as a real backend's tool
//! thread does, so `scripts/smoke.sh` can drive the whole round trip (draft
//! stash, options, Tab's amend, the restore) with no provider attached.
//!
//! All the permission demos run the same three steps per call — ask, block,
//! resolve — so those live once on [`Stage`] and each demo is just its own
//! script of calls.

use crate::permission::{PermissionDecision, PermissionKind, PermissionRequest};

use super::super::{StreamEvent, ToolCallSummary};
use super::scenario::{AskStage, Stage};
use super::script::{chunks, handoff};
use super::{CHUNK_DELAY, nap};
use crate::llm::tools::write_report as created;

/// How a gated call resolved: `Ok(output)` ran, `Err((display, result))` was
/// refused. The two texts of a refusal are deliberately different — `display`
/// is the short red cell the user reads, `result` the longer stop-and-wait
/// text the *model* receives — which is the real backend's shape, so the
/// offline demo exercises Tab's amend feedback all the way into the derived
/// context (`docs/permissions.md`).
type Resolution = Result<String, (String, String)>;

impl Stage<'_> {
    /// Announce a batch of calls up front, so the ones not yet running show
    /// as dim `⎿ Waiting…` cells (`docs/parallel-tools.md`).
    fn announce(&self, kind: &str, targets: impl IntoIterator<Item = String>) {
        let _ = self.tx.send(StreamEvent::ToolBatch(
            targets
                .into_iter()
                .map(|args| ToolCallSummary {
                    name: kind.to_string(),
                    args,
                })
                .collect(),
        ));
    }

    /// The approve seam's disposition for one call, offline: a standing
    /// approval runs it unasked, otherwise raise the request and **block on
    /// the gate** until the user answers. `output` is what an approval
    /// resolves with (built lazily — a rejection never needs it).
    ///
    /// `None` means the turn was cancelled out from under us: the channel is
    /// already abandoned, so the caller must simply stop.
    fn ask(
        &self,
        request: &mut PermissionRequest,
        output: impl FnOnce() -> String,
    ) -> Option<Resolution> {
        if self.gate.allows(request) {
            return Some(Ok(output()));
        }
        let _ = self.tx.send(StreamEvent::Permission(request.clone()));
        let decision = self.gate.wait(&request.id, &|| self.cancel.is_cancelled());
        self.judge(request, decision, output)
    }

    /// Map one gate answer onto a [`Resolution`], remembering an
    /// "allow always" as a session rule on the way through.
    fn judge(
        &self,
        request: &mut PermissionRequest,
        decision: Option<PermissionDecision>,
        output: impl FnOnce() -> String,
    ) -> Option<Resolution> {
        Some(match &decision {
            Some(PermissionDecision::Approve | PermissionDecision::ApproveAlways) => {
                if matches!(decision, Some(PermissionDecision::ApproveAlways)) {
                    request.id.clear();
                    self.gate.remember(request);
                }
                Ok(output())
            }
            Some(PermissionDecision::Deny(feedback)) => Err((
                crate::permission::denied_display(request, feedback.as_deref()),
                crate::permission::denial_result(feedback.as_deref()),
            )),
            Some(PermissionDecision::Explain) => Err((
                crate::permission::explain_display(),
                crate::permission::explain_result(request),
            )),
            // Cancelled out from under us — the channel is already abandoned.
            None => return None,
        })
    }

    /// Resolve the announced call: flip its cell to running, append the
    /// provenance `note` when something other than the user cleared it (auto
    /// mode's classifier), then commit it green via `ToolEnd` or red via
    /// `ToolRejected`.
    fn resolve(
        &self,
        kind: &str,
        args: &str,
        note: Option<String>,
        resolved: Resolution,
        ok: bool,
    ) {
        let _ = self.tx.send(StreamEvent::ToolStart {
            name: kind.to_string(),
            args: args.to_string(),
            detail: None,
        });
        if let Some(note) = note {
            let _ = self.tx.send(StreamEvent::ToolNote(note));
        }
        match resolved {
            Ok(output) => {
                let _ = self.tx.send(StreamEvent::ToolEnd {
                    output,
                    ok,
                    truncated: false,
                });
            }
            Err((display, result)) => {
                let _ = self.tx.send(StreamEvent::ToolRejected {
                    display,
                    result,
                    truncated: false,
                });
            }
        }
    }

    /// Close the turn: stream `text` word-by-word, then `StreamDone`. Stops
    /// early if the turn is cancelled or the receiver has hung up.
    fn close(&self, text: &str) {
        for chunk in chunks(text) {
            if self.cancel.is_cancelled() {
                return;
            }
            if self.tx.send(StreamEvent::Chunk(chunk)).is_err() {
                return;
            }
            nap(CHUNK_DELAY, self.cancel);
        }
        let _ = self.tx.send(StreamEvent::StreamDone);
    }

    /// A fresh request for one gated call.
    fn request(
        &self,
        kind: PermissionKind,
        target: &str,
        body: String,
        detail: Option<&str>,
    ) -> PermissionRequest {
        PermissionRequest {
            id: self.gate.next_id(),
            kind,
            target: target.to_string(),
            body,
            detail: detail.map(str::to_string),
            agent: None,
        }
    }
}

/// The scripted `AskUserQuestion` arguments for the ask demo (`docs/ask.md`):
/// three questions exercising every surface — a single-select, a multi-select
/// (checkboxes + its own `Submit` row), and a preview question (the
/// side-by-side panel, the `n` notes field). The reference transcript's
/// coffee/demo-topics/code-style trio.
const DUMMY_ASK_ARGS: &str = r#"{"questions":[
  {
    "question": "What's your favorite way to drink coffee?",
    "header": "Coffee style",
    "options": [
      {"label": "Black", "description": "No milk, no sugar — just coffee"},
      {"label": "Latte", "description": "Espresso with steamed milk"},
      {"label": "Cold brew", "description": "Slow-steeped, served cold"}
    ],
    "multiSelect": false
  },
  {
    "question": "Which of these tool features would you like to see demoed next? (pick any number)",
    "header": "Demo topics",
    "options": [
      {"label": "Preview panel", "description": "Side-by-side layout for comparing code/mockups/configs"},
      {"label": "Custom 'Other' input", "description": "Every question auto-includes a free-text Type something. row"},
      {"label": "4-option question", "description": "Questions can offer up to 4 choices each"}
    ],
    "multiSelect": true
  },
  {
    "question": "Which code style do you prefer for a simple greeting function?",
    "header": "Code style",
    "options": [
      {"label": "Arrow function", "description": "Modern and terse", "preview": "const greet = (name) => {\n  return `Hello, ${name}!`;\n};"},
      {"label": "Function declaration", "description": "Classic and hoisted", "preview": "function greet(name) {\n  return `Hello, ${name}!`;\n}"},
      {"label": "One-liner", "description": "As short as it gets", "preview": "const greet = (name) => `Hello, ${name}!`;"}
    ],
    "multiSelect": false
  }
]}"#;

/// Play the offline `AskUserQuestion` round trip (`docs/ask.md`): stream the
/// intro, announce the call, raise the question modal and **block on the ask
/// gate** exactly as the real tool thread does, then resolve the cell —
/// green with the answers, red for a decline or a `Chat about this` — and
/// close on a reply that reports what happened. The whole resolution mapping
/// is the real one ([`crate::llm::ask::ask_user`]), so the offline demo and a
/// live backend produce byte-identical cells.
pub(in crate::stream) fn ask_questions_turn(stage: &AskStage<'_>) {
    let intro = "Happy to ask — I'll walk you through the question tool: a single-select, \
                 a multi-select with checkboxes, and a preview question with notes. \
                 Tab moves between them; the last page reviews and submits.\n\n";
    for chunk in chunks(intro) {
        if stage.cancel.is_cancelled() {
            return;
        }
        if stage.tx.send(StreamEvent::Chunk(chunk)).is_err() {
            return;
        }
        nap(CHUNK_DELAY, stage.cancel);
    }
    let call = crate::llm::tools::ToolCallRequest {
        id: "ask_demo".to_string(),
        name: crate::llm::tools::ASK_TOOL_NAME.to_string(),
        arguments: DUMMY_ASK_ARGS.to_string(),
    };
    let name = crate::llm::tools::display_name(&call.name);
    let args = crate::llm::tools::summarize_call(&call.name, &call.arguments);
    let _ = stage.tx.send(StreamEvent::ToolBatch(vec![ToolCallSummary {
        name: name.clone(),
        args: args.clone(),
    }]));
    // The real executor path: parse, raise `AskUser`, block on the gate, map
    // the decision onto the split outcome (docs/ask.md).
    let outcome = crate::llm::ask::ask_user(stage.gate, stage.tx, stage.cancel, &call);
    if stage.cancel.is_cancelled() {
        return;
    }
    let _ = stage.tx.send(StreamEvent::ToolStart {
        name,
        args,
        detail: None,
    });
    let answered = outcome.ok;
    let chatting = outcome.output.starts_with(crate::ask::CHAT_HEADLINE);
    match outcome.context {
        Some(result) if answered => {
            let _ = stage.tx.send(StreamEvent::ToolAnswered {
                truncated: false,
                display: outcome.output,
                result,
            });
        }
        Some(result) => {
            let _ = stage.tx.send(StreamEvent::ToolRejected {
                truncated: false,
                display: outcome.output,
                result,
            });
        }
        // An argument failure can't happen (the script is valid); resolve
        // plainly so the mapping stays total.
        None => {
            let _ = stage.tx.send(StreamEvent::ToolEnd {
                output: outcome.output,
                ok: outcome.ok,
                truncated: false,
            });
        }
    }
    let closing: &str = if answered {
        concat!(
            "Noted — the cell above records each answer exactly the way a real model \
             would read them (the JSON result carries the same pairs, plus any notes \
             you typed on the preview question).\n\n",
            handoff!()
        )
    } else if chatting {
        concat!(
            "Sure — let's talk it through. I'm only the demo backend, but a real model \
             would now wait for your message and discuss the options before deciding \
             anything.\n\n",
            handoff!()
        )
    } else {
        concat!(
            "No problem — the questions can wait. A real model would stop here and let \
             you steer; ask for the ask question demo again anytime.\n\n",
            handoff!()
        )
    };
    for chunk in chunks(closing) {
        if stage.cancel.is_cancelled() {
            return;
        }
        if stage.tx.send(StreamEvent::Chunk(chunk)).is_err() {
            return;
        }
        nap(CHUNK_DELAY, stage.cancel);
    }
    let _ = stage.tx.send(StreamEvent::StreamDone);
}

/// The dummy's scripted `write` for the permission demo (`docs/permissions.md`).
const DUMMY_PERMISSION_PATH: &str = "hello.py";
const DUMMY_PERMISSION_CONTENT: &str = "#!/usr/bin/env python3\n\n\
    def main():\n    \
        name = input(\"What's your name? \")\n    \
        print(f\"Hello, {name}! Welcome to Python.\")\n\n\
    if __name__ == \"__main__\":\n    \
        main()";

/// Play the offline approval round trip: announce the `Write` call, raise the
/// request, **block on the gate** exactly as a real backend's tool thread does,
/// then resolve the cell green (approved) or red (rejected). Lets `smoke.sh`
/// drive the whole prompt — draft stash, options, restore — with no provider.
pub(in crate::stream) fn write_permission_turn(stage: &Stage<'_>) {
    stage.announce("Write", [DUMMY_PERMISSION_PATH.to_string()]);
    let mut request = stage.request(
        PermissionKind::Write,
        DUMMY_PERMISSION_PATH,
        crate::llm::tools::render_numbered_content(DUMMY_PERMISSION_CONTENT),
        None,
    );
    let Some(resolved) = stage.ask(&mut request, || {
        created(DUMMY_PERMISSION_PATH, DUMMY_PERMISSION_CONTENT)
    }) else {
        return;
    };
    let ok = resolved.is_ok();
    stage.resolve("Write", DUMMY_PERMISSION_PATH, None, resolved, true);
    stage.close(if ok {
        "Done — the file is written."
    } else {
        "Understood, I have left the file alone."
    });
}

/// The scripted **staggered** permission demo's files: two gated `write`s in
/// one batch whose prompts differ wildly in height — the first's body is
/// [`staggered_big_content`] (long enough to cap the prompt on any sane
/// terminal, so it fills the screen), the second's a single line. The offline
/// mirror of the reported gap-under-the-prompt shape: answering the tall
/// prompt commits its cell and opens the short prompt in the same frame gap,
/// shrinking the pinned modal region hard (`docs/permissions.md`, the smoke
/// suite's staggered-prompt phase).
const DUMMY_STAGGERED_BIG_PATH: &str = "big_module.py";
const DUMMY_STAGGERED_TINY_PATH: &str = "tiny_note.py";
const DUMMY_STAGGERED_TINY_CONTENT: &str = "# Reserved for a later chapter of the demo.";

/// The tall `write`'s body — enough source lines that the first prompt's
/// numbered preview caps (and so pads itself to fill the terminal) at any
/// realistic height, maximising the height drop to the tiny second prompt.
fn staggered_big_content() -> String {
    let mut lines = vec![
        "\"\"\"A module long enough to cap the permission prompt's preview.\"\"\"".to_string(),
        String::new(),
    ];
    lines.extend((1..=58).map(|i| format!("VALUE_{i:02} = {i}")));
    lines.join("\n")
}

/// Play the staggered batch approval round trip ([`DUMMY_STAGGERED_BIG_PATH`]
/// / [`DUMMY_STAGGERED_TINY_PATH`]): announce both `Write` calls up front,
/// then — per call, exactly like [`parallel_permission_turn`] — ask, block on
/// the gate, and resolve the cell before moving on, with **no scripted pause
/// anywhere** so the tall prompt's answer and the tiny prompt's request land
/// in one frame gap.
pub(in crate::stream) fn staggered_permission_turn(stage: &Stage<'_>) {
    let files = [
        (DUMMY_STAGGERED_BIG_PATH, staggered_big_content()),
        (
            DUMMY_STAGGERED_TINY_PATH,
            DUMMY_STAGGERED_TINY_CONTENT.to_string(),
        ),
    ];
    stage.announce("Write", files.iter().map(|(path, _)| (*path).to_string()));
    for (path, content) in &files {
        let mut request = stage.request(
            PermissionKind::Write,
            path,
            crate::llm::tools::render_numbered_content(content),
            None,
        );
        let Some(resolved) = stage.ask(&mut request, || created(path, content)) else {
            return;
        };
        stage.resolve("Write", path, None, resolved, true);
    }
    stage.close("Both files are written — the big module and the tiny note.");
}

/// The scripted **parallel** permission demo: two gated `bash` commands in one
/// batch — `(command, description, output, ok)` each. The first fails the way
/// `sudo` does in a captured shell, the second succeeds; both ask.
const DUMMY_PARALLEL_COMMANDS: [(&str, &str, &str, bool); 2] = [
    (
        "sudo whoami",
        "Check root/sudo privileges",
        "Exit code: 1\nsudo: a terminal is required to read the password; either use the -S \
         option to read from standard input or configure an askpass helper\nsudo: a password \
         is required",
        false,
    ),
    (
        "ping -c 4 google.com",
        "Ping google.com 4 times to test connectivity",
        "Exit code: 0\nPING google.com (142.250.72.14) 56(84) bytes of data.\n64 bytes from \
         142.250.72.14: icmp_seq=1 ttl=115 time=12.3 ms\n\n--- google.com ping statistics \
         ---\n4 packets transmitted, 4 received, 0% packet loss",
        true,
    ),
];

/// Play the offline **batch** approval round trip ([`DUMMY_PARALLEL_COMMANDS`]):
/// announce both `Bash` calls up front, then — per call, exactly like
/// `llm::agent::run_agent`'s loop — ask, block on the gate, and resolve the
/// cell before moving to the next. Deliberately **no scripted pause
/// anywhere**: on an approval the cell's `ToolStart`/`ToolEnd` and the *next*
/// call's `Permission` land in the same instant, so the loop sees a resolved
/// commit and a reopening modal inside one frame gap — the covering
/// machinery's hardest timing (`docs/permissions.md`, the smoke suite's
/// back-to-back-prompt phase). A rejection resolves that cell red and still
/// asks for the next call, as the real loop does.
pub(in crate::stream) fn parallel_permission_turn(stage: &Stage<'_>) {
    stage.announce(
        "Bash",
        DUMMY_PARALLEL_COMMANDS
            .iter()
            .map(|(cmd, ..)| (*cmd).to_string()),
    );
    for (cmd, detail, output, ok) in DUMMY_PARALLEL_COMMANDS {
        let mut request = stage.request(PermissionKind::Bash, cmd, String::new(), Some(detail));
        let Some(resolved) = stage.ask(&mut request, || output.to_string()) else {
            return;
        };
        stage.resolve("Bash", cmd, None, resolved, ok);
    }
    stage.close(
        "Both commands are done — sudo whoami failed (no terminal here) and the ping to \
         google.com came back clean.",
    );
}

/// The **auto-mode** demo's two commands: a read-only listing the offline
/// heuristic classifier allows (its output long enough for the collapsed
/// `… +N lines` hint, like the reference transcript) and a deletion it
/// denies. See [`auto_permission_turn`].
const DUMMY_AUTO_COMMANDS: &[(&str, &str, &str, bool)] = &[
    (
        "ls -la",
        "List the project files",
        "Exit code: 0\ntotal 40\ndrwxr-xr-x 1 user user  256 Aug  4 17:14 .\n\
         drwxr-xr-x 1 user user  126 Aug  1 14:35 ..\n\
         -rw-r--r-- 1 user user 1017 Aug  4 16:59 config.py\n\
         -rw-r--r-- 1 user user  188 Aug  4 16:59 README.md\n\
         -rw-r--r-- 1 user user 2044 Aug  4 17:01 game.py\n\
         -rw-r--r-- 1 user user  310 Aug  4 17:02 utils.py\n\
         -rw-r--r-- 1 user user  452 Aug  4 17:03 test_game.py\n\
         -rw-r--r-- 1 user user   96 Aug  4 17:05 Makefile\n\
         -rw-r--r-- 1 user user  120 Aug  4 17:08 .gitignore\n\
         -rw-r--r-- 1 user user  640 Aug  4 17:10 notes.md\n\
         -rw-r--r-- 1 user user  517 Aug  4 17:12 setup.py\n\
         drwxr-xr-x 1 user user  128 Aug  4 17:13 tests",
        true,
    ),
    (
        "rm -rf /tmp/scratch",
        "Clean up the scratch directory",
        // Only reachable when something other than the classifier cleared it
        // (master mode, an allowlist rule) — the classifier path rejects
        // before any output exists.
        "Exit code: 0",
        true,
    ),
];

/// Play the offline **auto mode** round trip (`docs/permissions.md`): the
/// [`DUMMY_AUTO_COMMANDS`] batch announced up front, then — per call, the
/// real approve seam's disposition — the allowlist first, then in
/// [`PermissionMode::Auto`] the offline heuristic classifier
/// ([`crate::permission::auto_verdict`]) in the user's stead: an allowed
/// command runs with the [`CLASSIFIER_ALLOWED_NOTE`] riding a `ToolNote`, a
/// denied one resolves red via `ToolRejected` with the classifier texts. In
/// any other mode the call asks like the ordinary demos, so the same prompt
/// exercises the Shift+Tab switch (smoke drives it in auto).
///
/// [`PermissionMode::Auto`]: crate::permission::PermissionMode::Auto
/// [`CLASSIFIER_ALLOWED_NOTE`]: crate::permission::CLASSIFIER_ALLOWED_NOTE
pub(in crate::stream) fn auto_permission_turn(stage: &Stage<'_>) {
    use crate::permission::{
        CLASSIFIER_ALLOWED_NOTE, PermissionMode, auto_verdict, classifier_denial_result,
        classifier_denied_display,
    };
    stage.announce(
        "Bash",
        DUMMY_AUTO_COMMANDS
            .iter()
            .map(|(cmd, ..)| (*cmd).to_string()),
    );
    let mut any_rejected = false;
    for (cmd, detail, output, ok) in DUMMY_AUTO_COMMANDS {
        let mut request = stage.request(PermissionKind::Bash, cmd, String::new(), Some(detail));
        // The approve seam's disposition, offline: standing approvals first,
        // then auto mode's classifier — here the deterministic heuristic —
        // and the prompt only outside auto mode.
        let mut note = None;
        let resolved = if stage.gate.allows(&request) {
            Ok(output.to_string())
        } else if stage.gate.mode() == PermissionMode::Auto {
            let verdict = auto_verdict(cmd);
            if verdict.allow {
                note = Some(CLASSIFIER_ALLOWED_NOTE.to_string());
                Ok(output.to_string())
            } else {
                let reason = Some(verdict.reason.as_str()).filter(|r| !r.trim().is_empty());
                Err((
                    classifier_denied_display(reason),
                    classifier_denial_result(reason),
                ))
            }
        } else {
            match stage.ask(&mut request, || output.to_string()) {
                Some(resolved) => resolved,
                // Cancelled out from under us — the channel is abandoned.
                None => return,
            }
        };
        any_rejected |= resolved.is_err();
        stage.resolve("Bash", cmd, note, resolved, *ok);
    }
    // The closing reply reports what actually happened — in auto mode the
    // delete is blocked, while master (or an allowlist) runs both.
    stage.close(if any_rejected {
        "Done — the listing ran and the delete was blocked, so nothing was removed."
    } else {
        "Done — both commands ran to completion."
    });
}
