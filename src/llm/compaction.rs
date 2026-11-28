//! A subagent's background compaction (`docs/background-compaction.md`
//! *Subagents*): the main session's lines and lifecycle, one level down, run
//! on the agent's own thread.
//!
//! The lead's conversation of record is its history, so its turn hands the
//! swap to the boundary and goes on with the context re-derived from it. A
//! subagent's conversation of record is its own message list — what its
//! thread sends every round, and what the registry stores for a chat
//! continuation — so its compaction **splices that list**: the summary is
//! written from the list's head on a thread of its own while the agent keeps
//! working, held until the agent's context reaches its ceiling, and then
//! takes the head's place, everything recorded since the summary started
//! kept as it was. The events it sends ride the agent's own stream, so its
//! transcript records the marker its session view, gauge and Ctrl+D derive
//! from, in order with everything else the agent did.
//!
//! The decision is the lead's [`decide`]; the rest is pure but for the two
//! closures [`AgentCompaction::round_top`] is handed — one that starts a
//! summary on a thread and one that waits a moment — so every path is tested
//! without a thread or a network anywhere.

use std::sync::{Arc, Mutex};

use tokio::sync::mpsc::UnboundedSender;

use super::{ChatMessage, MessageContent};
use crate::compaction::{Decision, Job, MAX_FAILURES, MAX_PAST_LANDINGS, ceiling, decide};
use crate::context::{
    COMPACT_SYSTEM_PROMPT, SUMMARIZATION_PROMPT, SUMMARY_PREFIX, kept_user_texts, summary_bridge,
};
use crate::stream::{CancelToken, StreamEvent, TokenUsage};

/// What a finished summary left: the summary or why there is none, the
/// provider's usage frames — billed whatever became of it — and how long it
/// took to write, in whole seconds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WrittenSummary {
    pub outcome: Result<String, String>,
    pub usage: Vec<TokenUsage>,
    pub secs: u64,
}

/// A summary being written on a thread of its own: the cell the thread fills
/// once it is done, and the token that stops it.
#[derive(Debug, Clone, Default)]
pub struct SummaryJob {
    pub cell: Arc<Mutex<Option<WrittenSummary>>>,
    pub cancel: CancelToken,
}

impl SummaryJob {
    /// What the summary left, once — `None` while it is still being written.
    fn take(&self) -> Option<WrittenSummary> {
        self.cell.lock().ok().and_then(|mut cell| cell.take())
    }
}

/// The text a wire message carries, when it is plain text.
fn text_of(message: &ChatMessage) -> Option<&str> {
    match &message.content {
        MessageContent::Text(text) => Some(text),
        MessageContent::Parts(_) => None,
    }
}

/// Whether `message` is a `<system-reminder>` the session put on the wire —
/// the briefing a launch opens on, a hook's added context.
fn is_reminder(message: &ChatMessage) -> bool {
    message.role == "user" && text_of(message).is_some_and(crate::reminder::is_reminder)
}

/// How many of `messages` lead the list and stay whatever is summarized: the
/// system prompt, and the briefing right behind it.
fn lead_len(messages: &[ChatMessage]) -> usize {
    let system = messages
        .iter()
        .take_while(|message| message.role == "system")
        .count();
    system + usize::from(messages.get(system).is_some_and(is_reminder))
}

/// What the summarizer is sent for the head `messages[..cut]` of a
/// subagent's list (`docs/background-compaction.md` *What the summarizer
/// reads*): its own system prompt, the conversation — never the agent's
/// system prompt nor any `<system-reminder>` — and the compact prompt last.
#[must_use]
pub fn summarizer_input(messages: &[ChatMessage], cut: usize) -> Vec<ChatMessage> {
    let head = &messages[..cut.min(messages.len())];
    std::iter::once(ChatMessage::system(COMPACT_SYSTEM_PROMPT))
        .chain(
            head.iter()
                .filter(|message| message.role != "system" && !is_reminder(message))
                .cloned(),
        )
        .chain(std::iter::once(ChatMessage::user(SUMMARIZATION_PROMPT)))
        .collect()
}

/// Whether `message` is one the user wrote: a plain user text that is no
/// reminder, no note the app speaks (`[system]`-tagged, like the continuation
/// a cut-off reply gets) and no earlier compaction's bridge — the lead's
/// typed-only rule, read off the wire.
fn is_typed(message: &ChatMessage) -> bool {
    message.role == "user"
        && text_of(message).is_some_and(|text| {
            !crate::reminder::is_reminder(text)
                && !text.starts_with("[system]")
                && !text.starts_with(SUMMARY_PREFIX)
        })
}

/// A subagent's list once `summary` takes the place of its head
/// `messages[..cut]`: the system prompt and the briefing as they were, what
/// the budget keeps of the user's texts and of the context a hook added (a
/// `SubagentStart` hook's note, which the summarizer is never sent and no
/// round makes again), the summary's bridge, then everything from `cut` on
/// verbatim — the shape the lead's derivation gives a marker.
#[must_use]
pub fn spliced(messages: &[ChatMessage], cut: usize, summary: &str) -> Vec<ChatMessage> {
    let cut = cut.min(messages.len());
    let lead = lead_len(messages).min(cut);
    let kept = kept_user_texts(
        messages[lead..cut]
            .iter()
            .filter(|message| is_typed(message) || is_reminder(message))
            .filter_map(text_of)
            .collect::<Vec<_>>(),
    );
    messages[..lead]
        .iter()
        .cloned()
        .chain(kept.into_iter().map(ChatMessage::user))
        .chain(std::iter::once(ChatMessage::user(summary_bridge(summary))))
        .chain(messages[cut..].iter().cloned())
        .collect()
}

/// What a round top measures: the context's size, and the window it runs in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Measure {
    pub used: u64,
    pub window: u64,
}

/// Where the agent's one summary stands.
#[derive(Debug)]
enum JobState {
    Running(SummaryJob),
    Ready { summary: String, secs: u64 },
}

#[derive(Debug)]
struct AgentJob {
    id: u64,
    /// The list's length when it started: everything before it is what the
    /// summary covers.
    cut: usize,
    state: JobState,
    /// Whether the agent has said it is waiting for it.
    waited: bool,
}

/// One subagent run's background compaction — the lead's
/// [`crate::compaction::CompactionSlot`] state, on the agent's own thread
/// with nobody to share it with: the one job, the failures this cycle, the
/// cycle's start, and how many landings this run left the context at or past
/// its ceiling.
#[derive(Debug, Default)]
pub struct AgentCompaction {
    job: Option<AgentJob>,
    failures: u32,
    cycle_start: u64,
    past_landings: u32,
    landed_unmeasured: bool,
    next_id: u64,
}

impl AgentCompaction {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The compaction of a run over `messages`: a launch starts its cycle at
    /// nothing, while a chat continuation over a list a summary was spliced
    /// into resumes a cycle — the lead reads its start off its last marker,
    /// and the agent takes it at its first round top, so a list compacted
    /// moments ago is not summarized again at once.
    #[must_use]
    pub fn for_list(messages: &[ChatMessage]) -> Self {
        let mut compaction = Self::new();
        compaction.landed_unmeasured = messages.iter().any(|message| {
            message.role == "user"
                && text_of(message).is_some_and(|text| text.starts_with(SUMMARY_PREFIX))
        });
        compaction
    }

    fn job_state(&self) -> Job {
        match &self.job {
            None => Job::None,
            Some(AgentJob {
                id,
                state: JobState::Running(_),
                ..
            }) => Job::Running(*id),
            Some(AgentJob {
                id,
                state: JobState::Ready { .. },
                ..
            }) => Job::Ready(*id),
        }
    }

    /// The agent's round top, before its request goes out, with
    /// `measure.used` tokens in a `measure.window`-token window
    /// (`docs/background-compaction.md` *The lifecycle*): past the start
    /// line a summary of `messages` is
    /// started with `start` and the round goes on; past the ceiling a held
    /// summary is spliced in, one still being written waited for — `pause`
    /// between looks — and with none one is started and waited for. Returns
    /// the list to go on with when a summary was spliced in. A cancelled run
    /// starts nothing and stops waiting.
    pub fn round_top(
        &mut self,
        messages: &[ChatMessage],
        measure: Measure,
        tx: &UnboundedSender<StreamEvent>,
        cancel: &CancelToken,
        start: &mut dyn FnMut(Vec<ChatMessage>) -> SummaryJob,
        pause: &mut dyn FnMut(),
    ) -> Option<Vec<ChatMessage>> {
        if cancel.is_cancelled() {
            return None;
        }
        let Measure { used, window } = measure;
        // The first round top after a splice measures what it left: the new
        // cycle's start, and — at or past the ceiling — a landing that could
        // not help.
        if std::mem::take(&mut self.landed_unmeasured) {
            self.cycle_start = used;
            if used >= ceiling(window) {
                self.past_landings += 1;
            }
        }
        loop {
            self.collect(tx);
            // Two landings past the ceiling this run — the second after
            // summarizing again, the tail included — start nothing more: what
            // rides every request alone passes it.
            let failures = if self.past_landings >= MAX_PAST_LANDINGS {
                MAX_FAILURES
            } else {
                self.failures
            };
            match decide(used, window, self.cycle_start, self.job_state(), failures) {
                Decision::Proceed => return None,
                Decision::Start => {
                    self.begin(messages, tx, start);
                    return None;
                }
                Decision::StartAndWait => self.begin(messages, tx, start),
                Decision::Wait(id) => {
                    if let Some(job) = self.job.as_mut().filter(|job| !job.waited) {
                        job.waited = true;
                        let _ = tx.send(StreamEvent::CompactionWaiting { id });
                    }
                    if cancel.is_cancelled() {
                        return None;
                    }
                    pause();
                    if cancel.is_cancelled() {
                        return None;
                    }
                }
                Decision::Swap(_) => return self.swap(messages, tx),
            }
        }
    }

    /// Start a summary of everything so far.
    fn begin(
        &mut self,
        messages: &[ChatMessage],
        tx: &UnboundedSender<StreamEvent>,
        start: &mut dyn FnMut(Vec<ChatMessage>) -> SummaryJob,
    ) {
        let id = self.next_id;
        self.next_id += 1;
        let cut = messages.len();
        let job = start(summarizer_input(messages, cut));
        self.job = Some(AgentJob {
            id,
            cut,
            state: JobState::Running(job),
            waited: false,
        });
        let _ = tx.send(StreamEvent::CompactionStart { id });
    }

    /// Take in a summary that has finished: billed either way, held when it
    /// was written, a failure counted when it was not.
    fn collect(&mut self, tx: &UnboundedSender<StreamEvent>) {
        let Some(job) = &mut self.job else {
            return;
        };
        let JobState::Running(running) = &job.state else {
            return;
        };
        let Some(written) = running.take() else {
            return;
        };
        for usage in written.usage {
            let _ = tx.send(StreamEvent::CompactionUsage(usage));
        }
        match written.outcome {
            Ok(summary) => {
                job.state = JobState::Ready {
                    summary: summary.trim().to_string(),
                    secs: written.secs,
                };
            }
            Err(_) => {
                self.job = None;
                self.failures += 1;
            }
        }
    }

    /// Splice the held summary in, and say so on the agent's stream.
    fn swap(
        &mut self,
        messages: &[ChatMessage],
        tx: &UnboundedSender<StreamEvent>,
    ) -> Option<Vec<ChatMessage>> {
        let job = self.job.take()?;
        let JobState::Ready { summary, secs } = job.state else {
            return None;
        };
        let rebuilt = spliced(messages, job.cut, &summary);
        let _ = tx.send(StreamEvent::CompactionLanded {
            id: job.id,
            summary,
            secs,
        });
        self.failures = 0;
        self.landed_unmeasured = true;
        Some(rebuilt)
    }

    /// The run ended: a summary that finished is billed, one still being
    /// written stopped — nothing will swap either in.
    pub fn finish(&mut self, tx: &UnboundedSender<StreamEvent>) {
        self.collect(tx);
        self.stop();
    }

    fn stop(&mut self) {
        if let Some(AgentJob {
            state: JobState::Running(job),
            ..
        }) = self.job.take()
        {
            job.cancel.cancel();
        }
    }
}

impl Drop for AgentCompaction {
    /// However the run ended, its summary thread does not outlive it.
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compaction::start_line;
    use crate::llm::ToolCallSpec;
    use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

    const WINDOW: u64 = 200_000;

    fn briefing() -> ChatMessage {
        ChatMessage::user("<system-reminder>\nSkills you can load: dataviz\n</system-reminder>")
    }

    /// One tool round: the call and its result.
    fn round(n: usize) -> [ChatMessage; 2] {
        let id = format!("call_{n}");
        [
            ChatMessage::assistant_tool_calls(
                "",
                vec![ToolCallSpec::function(
                    id.clone(),
                    "read",
                    format!("{{\"path\":\"/src/{n}.rs\"}}"),
                )],
            ),
            ChatMessage::tool_result(id, format!("fn f{n}() {{}}")),
        ]
    }

    /// A subagent's list as a launch lays it out: its system prompt, its
    /// briefing, its task — then `rounds` tool rounds.
    fn agent_list(rounds: usize) -> Vec<ChatMessage> {
        let mut messages = vec![
            ChatMessage::system("You are a subagent."),
            briefing(),
            ChatMessage::user("Port the payments module."),
        ];
        for n in 0..rounds {
            messages.extend(round(n));
        }
        messages
    }

    fn texts(messages: &[ChatMessage]) -> Vec<(String, String)> {
        messages
            .iter()
            .map(|message| {
                (
                    message.role.clone(),
                    text_of(message).unwrap_or_default().to_string(),
                )
            })
            .collect()
    }

    fn drain(rx: &mut UnboundedReceiver<StreamEvent>) -> Vec<StreamEvent> {
        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }
        events
    }

    fn written(summary: &str) -> WrittenSummary {
        WrittenSummary {
            outcome: Ok(summary.to_string()),
            usage: vec![TokenUsage {
                input: 9_000,
                output: 400,
                ..TokenUsage::default()
            }],
            secs: 31,
        }
    }

    /// A start closure that records each summary started and hands back
    /// its job, so the test can finish it.
    struct Starts {
        jobs: Vec<SummaryJob>,
        inputs: Vec<Vec<ChatMessage>>,
    }

    impl Starts {
        fn new() -> Self {
            Self {
                jobs: Vec::new(),
                inputs: Vec::new(),
            }
        }

        fn start(&mut self) -> impl FnMut(Vec<ChatMessage>) -> SummaryJob + '_ {
            move |input| {
                let job = SummaryJob::default();
                self.inputs.push(input);
                self.jobs.push(job.clone());
                job
            }
        }

        fn finish(&self, n: usize, with: WrittenSummary) {
            *self.jobs[n].cell.lock().unwrap() = Some(with);
        }
    }

    fn start_line_of(window: u64) -> u64 {
        start_line(ceiling(window), 0)
    }

    // ── What the summarizer reads ────────────────────────────────────────

    #[test]
    fn the_summarizer_reads_the_conversation_alone() {
        let mut messages = agent_list(2);
        messages.push(ChatMessage::user(
            "<system-reminder>\nSubagentStart hook additional context: the repo is /srv\n</system-reminder>",
        ));
        let input = summarizer_input(&messages, messages.len());
        assert_eq!(input[0], ChatMessage::system(COMPACT_SYSTEM_PROMPT));
        assert_eq!(input.last(), Some(&ChatMessage::user(SUMMARIZATION_PROMPT)));
        assert!(
            !input[1..]
                .iter()
                .any(|m| m.role == "system" || is_reminder(m)),
            "never the agent's system prompt nor a reminder: {input:#?}"
        );
        assert_eq!(
            input[1],
            ChatMessage::user("Port the payments module."),
            "the task leads the conversation"
        );
        assert_eq!(
            input.len(),
            1 + 1 + 4 + 1,
            "the task, two rounds, the prompt"
        );
    }

    #[test]
    fn the_summarizer_reads_only_the_head_before_the_cut() {
        let messages = agent_list(3);
        let input = summarizer_input(&messages, 5);
        assert_eq!(input.len(), 1 + 3 + 1, "the task and one round");
        assert_eq!(input[3], round(0)[1]);
    }

    // ── The splice ───────────────────────────────────────────────────────

    #[test]
    fn a_splice_keeps_the_lead_then_the_task_and_the_bridge_then_the_tail() {
        let messages = agent_list(3);
        let rebuilt = spliced(&messages, 5, "Read 0.rs.");
        assert_eq!(rebuilt[0], messages[0], "the system prompt as it was");
        assert_eq!(rebuilt[1], messages[1], "the briefing as it was");
        assert_eq!(rebuilt[2], ChatMessage::user("Port the payments module."));
        assert_eq!(rebuilt[3], ChatMessage::user(summary_bridge("Read 0.rs.")));
        assert_eq!(
            &rebuilt[4..],
            &messages[5..],
            "everything from the cut verbatim"
        );
    }

    /// The note a launch opens on when a `SubagentStart` hook added context.
    fn start_note() -> ChatMessage {
        ChatMessage::user(
            "<system-reminder>\nSubagentStart hook additional context: the repo is /srv\n</system-reminder>",
        )
    }

    #[test]
    fn a_splice_keeps_what_the_user_wrote_and_hooks_added_and_nothing_the_app_or_a_summary_said() {
        // A hook's added context is never sent to the summarizer, and no
        // round makes it again: the splice keeps it as it was, after the task
        // it came with.
        let mut messages = agent_list(0);
        messages.push(start_note());
        messages.extend(round(0));
        messages.push(ChatMessage::user(crate::llm::agent::CUT_OFF_NOTE));
        messages.push(ChatMessage::user("also the refunds"));
        messages.push(ChatMessage::user(summary_bridge("an earlier summary")));
        let cut = messages.len();
        messages.extend(round(9));
        let rebuilt = spliced(&messages, cut, "now");
        let users: Vec<String> = texts(&rebuilt[2..rebuilt.len() - 2])
            .into_iter()
            .filter(|(role, _)| role == "user")
            .map(|(_, text)| text)
            .collect();
        assert_eq!(
            users,
            vec![
                "Port the payments module.".to_string(),
                text_of(&start_note()).unwrap().to_string(),
                "also the refunds".to_string(),
                summary_bridge("now"),
            ]
        );
    }

    #[test]
    fn hook_context_survives_every_splice_and_never_reaches_a_summarizer() {
        let mut messages = agent_list(0);
        messages.push(start_note());
        messages.extend(round(0));
        let cut = messages.len();
        messages.extend(round(1));
        let once = spliced(&messages, cut, "read 0.rs");
        assert!(once.contains(&start_note()), "{once:#?}");
        let mut grown = once;
        let cut = grown.len();
        grown.extend(round(2));
        assert!(
            !summarizer_input(&grown, cut).iter().any(is_reminder),
            "the carried note is still kept from the summarizer"
        );
        let twice = spliced(&grown, cut, "read 1.rs");
        assert!(twice.contains(&start_note()), "{twice:#?}");
    }

    #[test]
    fn a_splice_at_the_end_of_the_list_ends_on_the_bridge() {
        let messages = agent_list(2);
        let rebuilt = spliced(&messages, messages.len(), "all of it");
        assert_eq!(rebuilt.len(), 4);
        assert_eq!(rebuilt[3], ChatMessage::user(summary_bridge("all of it")));
    }

    #[test]
    fn a_list_with_no_system_prompt_or_briefing_keeps_no_lead() {
        let mut messages = vec![ChatMessage::user("Port it.")];
        messages.extend(round(0));
        let rebuilt = spliced(&messages, messages.len(), "done");
        assert_eq!(
            texts(&rebuilt),
            vec![
                ("user".to_string(), "Port it.".to_string()),
                ("user".to_string(), summary_bridge("done")),
            ]
        );
    }

    // ── The lifecycle ────────────────────────────────────────────────────

    #[test]
    fn below_the_start_line_nothing_starts() {
        let (tx, mut rx) = unbounded_channel();
        let mut starts = Starts::new();
        let mut compaction = AgentCompaction::new();
        let messages = agent_list(1);
        let used = start_line_of(WINDOW) - 1;
        let swapped = compaction.round_top(
            &messages,
            Measure {
                used,
                window: WINDOW,
            },
            &tx,
            &CancelToken::new(),
            &mut starts.start(),
            &mut || panic!("nothing to wait for"),
        );
        assert_eq!(swapped, None);
        assert!(starts.inputs.is_empty());
        assert!(drain(&mut rx).is_empty());
    }

    #[test]
    fn past_the_start_line_a_summary_starts_and_the_round_goes_on() {
        let (tx, mut rx) = unbounded_channel();
        let mut starts = Starts::new();
        let mut compaction = AgentCompaction::new();
        let messages = agent_list(2);
        let swapped = compaction.round_top(
            &messages,
            Measure {
                used: start_line_of(WINDOW),
                window: WINDOW,
            },
            &tx,
            &CancelToken::new(),
            &mut starts.start(),
            &mut || panic!("the round goes on"),
        );
        assert_eq!(swapped, None);
        assert_eq!(
            starts.inputs,
            vec![summarizer_input(&messages, messages.len())]
        );
        assert_eq!(drain(&mut rx), vec![StreamEvent::CompactionStart { id: 0 }]);
    }

    /// A compaction whose summary of `agent_list(2)` was started, at the
    /// start line.
    fn started(starts: &mut Starts, tx: &UnboundedSender<StreamEvent>) -> AgentCompaction {
        let mut compaction = AgentCompaction::new();
        compaction.round_top(
            &agent_list(2),
            Measure {
                used: start_line_of(WINDOW),
                window: WINDOW,
            },
            tx,
            &CancelToken::new(),
            &mut starts.start(),
            &mut || {},
        );
        compaction
    }

    #[test]
    fn a_written_summary_is_held_until_the_ceiling() {
        let (tx, mut rx) = unbounded_channel();
        let mut starts = Starts::new();
        let mut compaction = started(&mut starts, &tx);
        starts.finish(0, written("Read 0.rs and 1.rs."));
        drain(&mut rx);
        let messages = agent_list(3);
        let swapped = compaction.round_top(
            &messages,
            Measure {
                used: ceiling(WINDOW) - 1,
                window: WINDOW,
            },
            &tx,
            &CancelToken::new(),
            &mut starts.start(),
            &mut || panic!("nothing to wait for"),
        );
        assert_eq!(swapped, None, "held below the ceiling");
        assert_eq!(starts.inputs.len(), 1, "no second summary");
        assert_eq!(
            drain(&mut rx),
            vec![StreamEvent::CompactionUsage(written("").usage[0])],
            "billed when it lands on the agent's side, held all the same"
        );
    }

    #[test]
    fn at_the_ceiling_the_held_summary_is_spliced_in() {
        let (tx, mut rx) = unbounded_channel();
        let mut starts = Starts::new();
        let mut compaction = started(&mut starts, &tx);
        starts.finish(0, written("  Read 0.rs and 1.rs.\n"));
        drain(&mut rx);
        let messages = agent_list(4);
        let swapped = compaction.round_top(
            &messages,
            Measure {
                used: ceiling(WINDOW),
                window: WINDOW,
            },
            &tx,
            &CancelToken::new(),
            &mut starts.start(),
            &mut || panic!("nothing to wait for"),
        );
        let cut = agent_list(2).len();
        assert_eq!(
            swapped,
            Some(spliced(&messages, cut, "Read 0.rs and 1.rs.")),
            "the head before the summary started, replaced; the rounds since kept"
        );
        assert_eq!(
            drain(&mut rx),
            vec![
                StreamEvent::CompactionUsage(written("").usage[0]),
                StreamEvent::CompactionLanded {
                    id: 0,
                    summary: "Read 0.rs and 1.rs.".into(),
                    secs: 31,
                },
            ]
        );
    }

    #[test]
    fn at_the_ceiling_an_unwritten_summary_is_waited_for() {
        let (tx, mut rx) = unbounded_channel();
        let mut starts = Starts::new();
        let mut compaction = started(&mut starts, &tx);
        drain(&mut rx);
        let messages = agent_list(4);
        let mut pauses = 0;
        let job = starts.jobs[0].clone();
        let swapped = compaction.round_top(
            &messages,
            Measure {
                used: ceiling(WINDOW),
                window: WINDOW,
            },
            &tx,
            &CancelToken::new(),
            &mut |_| panic!("one summary at a time"),
            &mut || {
                pauses += 1;
                if pauses == 3 {
                    *job.cell.lock().unwrap() = Some(written("Read them."));
                }
            },
        );
        assert_eq!(pauses, 3);
        assert_eq!(
            swapped,
            Some(spliced(&messages, agent_list(2).len(), "Read them."))
        );
        let events = drain(&mut rx);
        assert_eq!(
            events[0],
            StreamEvent::CompactionWaiting { id: 0 },
            "said once: {events:?}"
        );
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e, StreamEvent::CompactionWaiting { .. }))
                .count(),
            1
        );
        assert!(matches!(
            events.last(),
            Some(StreamEvent::CompactionLanded { .. })
        ));
    }

    #[test]
    fn past_the_ceiling_with_nothing_started_one_is_started_and_waited_for() {
        let (tx, mut rx) = unbounded_channel();
        let mut compaction = AgentCompaction::new();
        let messages = agent_list(3);
        let jobs: Arc<Mutex<Vec<SummaryJob>>> = Arc::default();
        let swapped = compaction.round_top(
            &messages,
            Measure {
                used: ceiling(WINDOW) + 5_000,
                window: WINDOW,
            },
            &tx,
            &CancelToken::new(),
            &mut |_| {
                let job = SummaryJob::default();
                jobs.lock().unwrap().push(job.clone());
                job
            },
            &mut || {
                if let Some(job) = jobs.lock().unwrap().first() {
                    *job.cell.lock().unwrap() = Some(written("All of it."));
                }
            },
        );
        assert_eq!(
            swapped,
            Some(spliced(&messages, messages.len(), "All of it.")),
            "everything so far summarized, nothing kept after it"
        );
        let events = drain(&mut rx);
        assert_eq!(events[0], StreamEvent::CompactionStart { id: 0 });
        assert_eq!(events[1], StreamEvent::CompactionWaiting { id: 0 });
    }

    #[test]
    fn a_failed_summary_is_counted_and_two_stop_starts_until_a_landing() {
        let (tx, mut rx) = unbounded_channel();
        let mut starts = Starts::new();
        let mut compaction = AgentCompaction::new();
        let messages = agent_list(2);
        let failed = || WrittenSummary {
            outcome: Err("HTTP 529".into()),
            usage: Vec::new(),
            secs: 2,
        };
        for n in 0..2 {
            compaction.round_top(
                &messages,
                Measure {
                    used: start_line_of(WINDOW),
                    window: WINDOW,
                },
                &tx,
                &CancelToken::new(),
                &mut starts.start(),
                &mut || {},
            );
            assert_eq!(starts.inputs.len(), n + 1, "a fresh start after a failure");
            starts.finish(n, failed());
        }
        compaction.round_top(
            &messages,
            Measure {
                used: start_line_of(WINDOW),
                window: WINDOW,
            },
            &tx,
            &CancelToken::new(),
            &mut starts.start(),
            &mut || {},
        );
        assert_eq!(starts.inputs.len(), 2, "two failures stop starts");
        let swapped = compaction.round_top(
            &messages,
            Measure {
                used: ceiling(WINDOW),
                window: WINDOW,
            },
            &tx,
            &CancelToken::new(),
            &mut starts.start(),
            &mut || panic!("nothing to wait for"),
        );
        assert_eq!(
            swapped, None,
            "at the ceiling the request goes out as it is"
        );
        assert!(
            !drain(&mut rx)
                .iter()
                .any(|e| matches!(e, StreamEvent::CompactionLanded { .. }))
        );
    }

    #[test]
    fn a_cancelled_run_starts_nothing_and_stops_waiting() {
        let (tx, mut rx) = unbounded_channel();
        let mut starts = Starts::new();
        let mut compaction = AgentCompaction::new();
        let cancel = CancelToken::new();
        cancel.cancel();
        let swapped = compaction.round_top(
            &agent_list(2),
            Measure {
                used: ceiling(WINDOW),
                window: WINDOW,
            },
            &tx,
            &cancel,
            &mut starts.start(),
            &mut || panic!("a cancelled run does not wait"),
        );
        assert_eq!(swapped, None);
        assert!(starts.inputs.is_empty());
        assert!(drain(&mut rx).is_empty());

        // …and one cancelled while it waits stops at the next look.
        let mut compaction = started(&mut starts, &tx);
        let cancel = CancelToken::new();
        let swapped = compaction.round_top(
            &agent_list(4),
            Measure {
                used: ceiling(WINDOW),
                window: WINDOW,
            },
            &tx,
            &cancel,
            &mut starts.start(),
            &mut || cancel.cancel(),
        );
        assert_eq!(swapped, None);
    }

    /// A compaction whose first summary was swapped in.
    fn landed(starts: &mut Starts, tx: &UnboundedSender<StreamEvent>) -> AgentCompaction {
        let mut compaction = started(starts, tx);
        starts.finish(0, written("first"));
        let swapped = compaction.round_top(
            &agent_list(4),
            Measure {
                used: ceiling(WINDOW),
                window: WINDOW,
            },
            tx,
            &CancelToken::new(),
            &mut starts.start(),
            &mut || {},
        );
        assert!(swapped.is_some());
        compaction
    }

    #[test]
    fn the_cycle_starts_where_the_landing_left_the_context() {
        let (tx, _rx) = unbounded_channel();
        let mut starts = Starts::new();
        let mut compaction = landed(&mut starts, &tx);
        // The first round top after the splice measures 150k: the start line
        // is now half the room left, 160k, so 150k starts nothing.
        let after = 150_000;
        compaction.round_top(
            &agent_list(5),
            Measure {
                used: after,
                window: WINDOW,
            },
            &tx,
            &CancelToken::new(),
            &mut starts.start(),
            &mut || {},
        );
        assert_eq!(starts.inputs.len(), 1);
        let line = start_line(ceiling(WINDOW), after);
        assert!(line > after, "{line}");
        compaction.round_top(
            &agent_list(6),
            Measure {
                used: line,
                window: WINDOW,
            },
            &tx,
            &CancelToken::new(),
            &mut starts.start(),
            &mut || {},
        );
        assert_eq!(
            starts.inputs.len(),
            2,
            "the next summary starts at the new line"
        );
    }

    #[test]
    fn a_landing_past_the_ceiling_is_summarized_once_more_then_the_run_compacts_no_more() {
        let (tx, mut rx) = unbounded_channel();
        let mut starts = Starts::new();
        let mut compaction = landed(&mut starts, &tx);
        drain(&mut rx);
        // The splice left the context past the ceiling: the next round top
        // summarizes again at once, the tail included, and waits for it.
        let mut refold = |_input: Vec<ChatMessage>| {
            let job = SummaryJob::default();
            *job.cell.lock().unwrap() = Some(written("second"));
            job
        };
        let swapped = compaction.round_top(
            &agent_list(5),
            Measure {
                used: ceiling(WINDOW) + 1_000,
                window: WINDOW,
            },
            &tx,
            &CancelToken::new(),
            &mut refold,
            &mut || {},
        );
        assert!(swapped.is_some(), "refolded once");
        // That one left it past the ceiling too: nothing starts again.
        let swapped = compaction.round_top(
            &agent_list(6),
            Measure {
                used: ceiling(WINDOW) + 1_000,
                window: WINDOW,
            },
            &tx,
            &CancelToken::new(),
            &mut |_| panic!("spent for the run"),
            &mut || panic!("nothing to wait for"),
        );
        assert_eq!(swapped, None);
    }

    #[test]
    fn a_launch_starts_its_cycle_at_nothing() {
        let (tx, _rx) = unbounded_channel();
        let mut starts = Starts::new();
        let messages = agent_list(2);
        let mut compaction = AgentCompaction::for_list(&messages);
        compaction.round_top(
            &messages,
            Measure {
                used: start_line_of(WINDOW),
                window: WINDOW,
            },
            &tx,
            &CancelToken::new(),
            &mut starts.start(),
            &mut || {},
        );
        assert_eq!(starts.inputs.len(), 1);
    }

    #[test]
    fn a_continuation_of_a_compacted_list_starts_its_cycle_where_it_resumes() {
        // A chat continuation runs on the list its last run settled with:
        // one that carries a summary's bridge resumes a cycle, which the lead
        // reads off its last marker and the agent off its first round top —
        // so it does not summarize a list it compacted moments ago.
        let (tx, _rx) = unbounded_channel();
        let mut starts = Starts::new();
        let mut messages = spliced(&agent_list(4), 5, "earlier");
        messages.extend(round(7));
        let mut compaction = AgentCompaction::for_list(&messages);
        let resumed_at = start_line_of(WINDOW) + 10_000;
        compaction.round_top(
            &messages,
            Measure {
                used: resumed_at,
                window: WINDOW,
            },
            &tx,
            &CancelToken::new(),
            &mut starts.start(),
            &mut || {},
        );
        assert!(
            starts.inputs.is_empty(),
            "past the first cycle's line, inside this one"
        );
        compaction.round_top(
            &messages,
            Measure {
                used: start_line(ceiling(WINDOW), resumed_at),
                window: WINDOW,
            },
            &tx,
            &CancelToken::new(),
            &mut starts.start(),
            &mut || {},
        );
        assert_eq!(starts.inputs.len(), 1, "half the room left, and it starts");
    }

    #[test]
    fn finishing_a_run_bills_a_summary_that_never_landed_and_stops_one_being_written() {
        let (tx, mut rx) = unbounded_channel();
        let mut starts = Starts::new();
        let mut compaction = started(&mut starts, &tx);
        drain(&mut rx);
        compaction.finish(&tx);
        assert!(
            starts.jobs[0].cancel.is_cancelled(),
            "its thread is stopped"
        );
        assert!(drain(&mut rx).is_empty());

        let mut compaction = started(&mut starts, &tx);
        starts.finish(1, written("never swapped"));
        drain(&mut rx);
        compaction.finish(&tx);
        assert_eq!(
            drain(&mut rx),
            vec![StreamEvent::CompactionUsage(written("").usage[0])],
            "billed though it never landed"
        );
    }

    #[test]
    fn a_compaction_dropped_mid_summary_stops_its_thread() {
        let (tx, _rx) = unbounded_channel();
        let mut starts = Starts::new();
        let compaction = started(&mut starts, &tx);
        drop(compaction);
        assert!(starts.jobs[0].cancel.is_cancelled());
    }
}
