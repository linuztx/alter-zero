//! `/resume` session files — the pure core (see `docs/resume.md`).
//!
//! Every conversation is recorded to a codex-style JSONL "rollout" file:
//! line 1 is a `session_meta` record, then one line per finished
//! [`HistoryItem`] as it lands in [`App::history`] — completed items only,
//! never streaming deltas (codex's persistence policy, `rollout/src/policy.rs`).
//! The `/resume` picker lists those files and loads one back.
//!
//! This module owns the on-disk format (serde on its **own** record types —
//! the app types stay serde-free), the parse-back (malformed/unknown lines are
//! skipped, codex's forward-compatible reader), and the picker's pure
//! ingredients: the first-user-message preview and the humanized age. All the
//! file/clock/pid I/O lives at the boundary in `main.rs` (`SessionRecorder`),
//! which injects timestamps and ids into these pure functions — the
//! `set_clock` pattern.
//!
//! [`App::history`]: crate::app::App::history

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::agents::AgentStatus;
use crate::app::{
    AgentGroup, AgentGroupEntry, AgentNotice, DONE_VERBS, HistoryItem, Message, Role, ToolCall,
    ToolStatus, TurnSummary,
};
use crate::checkpoint::Checkpoint;

/// The `session_meta` payload — the first line of every rollout file (codex's
/// `SessionMeta`: identity plus enough context to label the session later).
/// `timestamp` is the session start (UTC, boundary-supplied); `cwd`/`model`
/// mirror the footer's [`SessionInfo`] strings at recording time.
///
/// [`SessionInfo`]: crate::app::SessionInfo
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionMeta {
    /// Unique session id — also embedded in the filename. Never parsed back
    /// (resume goes by *path*); uniqueness is all that matters.
    pub id: String,
    /// Session-start stamp (UTC `YYYY-MM-DDTHH:MM:SS.mmmZ`, codex's shape).
    pub timestamp: String,
    /// The working directory the session was recorded in.
    pub cwd: String,
    /// The backend's model id ([`ReplySource::model_name`]).
    ///
    /// [`ReplySource::model_name`]: crate::stream::ReplySource::model_name
    pub model: String,
    /// Who wrote the file (`"alter-zero"`) — codex records an `originator`.
    pub originator: String,
    /// The recording crate version (`CARGO_PKG_VERSION`).
    pub version: String,
}

/// One row of the `/resume` picker: the rollout file to load, its two sort
/// keys as seconds-ago values (frozen when the picker opens — codex's
/// `relative_time_reference`; the displayed age is [`relative_age`] of the
/// active sort key's value), the cwd its meta recorded (the `Cwd` filter
/// compares it to the picker's own), and the first-user-message preview.
/// Built at the boundary from the head scan; held on [`App`] while the picker
/// is up.
///
/// [`App`]: crate::app::App
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionSummary {
    /// The rollout file this row resumes.
    pub path: PathBuf,
    /// Seconds since the file was last modified — the `Updated` sort key
    /// (codex's default sort) and its displayed age.
    pub updated_secs: u64,
    /// Seconds since the session started (the meta line's timestamp) — the
    /// `Created` sort key and its displayed age.
    pub created_secs: u64,
    /// The working directory the session's meta line recorded — matched
    /// verbatim against the picker's cwd by the `Cwd` filter (both sides are
    /// the same `Path::display` formatting; no normalization).
    pub cwd: String,
    /// The session's first user (or `!` shell) message, whitespace-flattened.
    pub preview: String,
}

/// One JSONL line: the write-time stamp plus the tagged record — codex's
/// `RolloutLine { timestamp, type, payload }` shape.
#[derive(Serialize, Deserialize)]
struct LineRecord {
    timestamp: String,
    #[serde(flatten)]
    item: ItemRecord,
}

/// The tagged per-line payload (codex's `RolloutItem`): a session-meta line or
/// one finished history item. Unknown `type`s fail to parse and are skipped by
/// the reader — the forward-compatibility contract.
#[derive(Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
enum ItemRecord {
    SessionMeta(SessionMeta),
    Message(MessageRecord),
    Tool(ToolRecord),
    Summary(SummaryRecord),
    Background(BackgroundRecord),
    /// A filesystem checkpoint (`docs/checkpoint.md`) — not a [`HistoryItem`]
    /// but recorded in the same file so a resume can reset the code. Parsed by
    /// [`parse_checkpoints`], skipped by [`parse_session`]. Old builds skip the
    /// unknown record type (the forward-compatibility contract).
    Checkpoint(CheckpointRecord),
    /// A `/compact` marker (`docs/compact.md`) — persists so a `/resume`
    /// stays compacted. Old builds skip the unknown record type (and see the
    /// full uncompacted context — the graceful degradation).
    Compaction(CompactionRecord),
    /// A resolved subagent group (`docs/agent-tool.md`). Old builds skip the
    /// unknown record type (the forward-compatibility contract).
    AgentGroup(AgentGroupRecord),
    /// A background agent's completion notice (`docs/agent-tool.md`).
    AgentNotice(AgentNoticeRecord),
}

/// An [`AgentGroup`] on disk (`docs/agent-tool.md`).
///
/// [`AgentGroup`]: crate::app::AgentGroup
#[derive(Serialize, Deserialize)]
struct AgentGroupRecord {
    background: bool,
    agents: Vec<AgentEntryRecord>,
    timestamp: String,
}

/// One [`AgentGroupEntry`] on disk. `status` is the lowercase status name; an
/// unknown one parses as `interrupted` (the conservative red).
///
/// [`AgentGroupEntry`]: crate::app::AgentGroupEntry
#[derive(Serialize, Deserialize)]
struct AgentEntryRecord {
    id: String,
    description: String,
    agent_type: String,
    prompt: String,
    status: String,
    tool_uses: usize,
    tokens: u64,
    secs: u64,
    result: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    tool_headers: Vec<String>,
    output: String,
}

/// An [`AgentNotice`] on disk (`docs/agent-tool.md`).
///
/// [`AgentNotice`]: crate::app::AgentNotice
#[derive(Serialize, Deserialize)]
struct AgentNoticeRecord {
    id: String,
    description: String,
    status: String,
    secs: u64,
    result: String,
    timestamp: String,
}

/// The lowercase on-disk name of an agent status.
const fn agent_status_name(status: AgentStatus) -> &'static str {
    match status {
        AgentStatus::Pending => "pending",
        AgentStatus::Running => "running",
        AgentStatus::Done => "done",
        AgentStatus::Failed => "failed",
        AgentStatus::Interrupted => "interrupted",
    }
}

/// The agent status a recorded name maps back to. Unknown names — and the
/// transient `pending`/`running` of a rollout cut short mid-run (a resumed
/// session's agents are gone) — read back as `interrupted`, the conservative
/// "this never finished" red.
fn agent_status_from(name: &str) -> AgentStatus {
    match name {
        "done" => AgentStatus::Done,
        "failed" => AgentStatus::Failed,
        _ => AgentStatus::Interrupted,
    }
}

/// A [`Compaction`] marker on disk (`docs/compact.md`): the model-written
/// handoff summary the context derivation bridges from, plus the gauge's
/// before/after token counts and the auto tag — `serde(default)`ed (and kept
/// off the wire at their defaults) so files from before the gauge still parse
/// and unchanged lines keep their shape.
///
/// [`Compaction`]: crate::app::Compaction
#[derive(Serialize, Deserialize)]
struct CompactionRecord {
    summary: String,
    timestamp: String,
    #[serde(default, skip_serializing_if = "is_zero")]
    before: u64,
    #[serde(default, skip_serializing_if = "is_zero")]
    after: u64,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    auto: bool,
}

/// `skip_serializing_if` helper for the gauge counts' 0-means-unknown default.
#[allow(clippy::trivially_copy_pass_by_ref)] // the signature serde requires
const fn is_zero(count: &u64) -> bool {
    *count == 0
}

/// A [`Message`] on disk. `role` is the lowercase role name; an unknown role
/// (a future variant) skips the line on parse instead of failing the file.
#[derive(Serialize, Deserialize)]
struct MessageRecord {
    role: String,
    text: String,
    /// The display stamp the item carried (`hh:mm AM/PM`, possibly empty) —
    /// round-tripped verbatim; see `docs/timestamps.md`.
    timestamp: String,
    /// The Ctrl+V attachment paths a user message carried (`docs/context.md`).
    /// Omitted when empty, so imageless lines keep the pre-images shape and
    /// files written before the field still parse. Stored as plain strings
    /// (lossy-converted on write): serializing a non-UTF8 `PathBuf` is a
    /// serde_json *error*, which would break [`line`]'s infallibility and
    /// panic the recorder — a mangled path merely fails to open later and
    /// surfaces as the backend's `[image unavailable]` note.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    images: Vec<String>,
}

/// A finished [`ToolCall`] on disk. Only finished statuses exist in history,
/// so the status collapses to `ok: bool` — plus `backgrounded` for a call
/// resolved by moving to the background ([`ToolStatus::Backgrounded`]);
/// omitted when false, so files written before the field keep their shape and
/// still parse (`docs/background.md`).
#[derive(Serialize, Deserialize)]
struct ToolRecord {
    name: String,
    args: String,
    ok: bool,
    output: String,
    timestamp: String,
    shell: bool,
    truncated: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    backgrounded: bool,
}

/// A [`BackgroundNotice`] on disk — a background shell's completion notice
/// (`docs/background.md`). Old builds skip the unknown record type (the
/// forward-compatibility contract).
///
/// [`BackgroundNotice`]: crate::app::BackgroundNotice
#[derive(Serialize, Deserialize)]
struct BackgroundRecord {
    description: String,
    id: String,
    code: Option<i32>,
    killed: bool,
    output_tail: String,
    timestamp: String,
}

/// A [`Checkpoint`] on disk (`docs/checkpoint.md`): the isolated-store commit
/// SHA and the history length it captures the code state at.
#[derive(Serialize, Deserialize)]
struct CheckpointRecord {
    commit: String,
    after: usize,
}

/// A [`TurnSummary`] on disk. `verb` maps back to its [`DONE_VERBS`] static on
/// load (falling back to `Done` for a verb this build doesn't know) because
/// `TurnSummary::verb` is `&'static str`. The real usage (`tokens`/`cached`,
/// `docs/prompt-caching.md`) is persisted — a fact of the turn, unlike the
/// transient `shells` count — and `serde(default)`ed so rollouts recorded
/// before the fields existed still parse.
#[derive(Serialize, Deserialize)]
struct SummaryRecord {
    verb: String,
    secs: u64,
    timestamp: String,
    #[serde(default)]
    tokens: usize,
    #[serde(default)]
    cached: usize,
}

/// One serialized JSONL line for `item`, stamped `stamp`. Serializing these
/// record types cannot fail (strings, bools, integers — no fallible impls, no
/// non-string map keys), hence the `expect`.
fn line(stamp: &str, item: ItemRecord) -> String {
    serde_json::to_string(&LineRecord {
        timestamp: stamp.to_string(),
        item,
    })
    .expect("session record types serialize infallibly")
}

/// The lowercase on-disk name of `role`.
const fn role_name(role: Role) -> &'static str {
    match role {
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Error => "error",
        Role::System => "system",
        Role::Shell => "shell",
    }
}

/// The role a recorded name maps back to; `None` for a name this build
/// doesn't know (the line is skipped — forward compatibility).
fn role_from(name: &str) -> Option<Role> {
    match name {
        "user" => Some(Role::User),
        "assistant" => Some(Role::Assistant),
        "error" => Some(Role::Error),
        "system" => Some(Role::System),
        "shell" => Some(Role::Shell),
        _ => None,
    }
}

/// The [`DONE_VERBS`] static matching a recorded verb, or `Done` (the first)
/// for one this build doesn't know — [`TurnSummary::verb`] is `&'static str`.
fn done_verb(name: &str) -> &'static str {
    DONE_VERBS
        .iter()
        .copied()
        .find(|verb| *verb == name)
        .unwrap_or(DONE_VERBS[0])
}

/// Serialize the `session_meta` line (the first line of a new rollout file).
/// `stamp` is the write-time line stamp, boundary-supplied like the meta's own
/// session-start timestamp.
#[must_use]
pub fn meta_line(meta: &SessionMeta, stamp: &str) -> String {
    line(stamp, ItemRecord::SessionMeta(meta.clone()))
}

/// Serialize one finished history item as a rollout line. `stamp` is the
/// write-time line stamp (UTC, boundary-supplied).
#[must_use]
pub fn item_line(item: &HistoryItem, stamp: &str) -> String {
    let record = match item {
        HistoryItem::Message(message) => ItemRecord::Message(MessageRecord {
            role: role_name(message.role).to_string(),
            text: message.text.clone(),
            timestamp: message.timestamp.clone(),
            images: message
                .images
                .iter()
                .map(|path| path.to_string_lossy().into_owned())
                .collect(),
        }),
        // Only finished tools reach history, so status collapses to ok/failed
        // (a Running status — impossible here — would record as failed), plus
        // the backgrounded marker (recorded ok: a launch that handed off).
        HistoryItem::Tool(tool) => ItemRecord::Tool(ToolRecord {
            name: tool.name.clone(),
            args: tool.args.clone(),
            ok: matches!(tool.status, ToolStatus::Ok | ToolStatus::Backgrounded),
            output: tool.output.clone(),
            timestamp: tool.timestamp.clone(),
            shell: tool.shell,
            truncated: tool.truncated,
            backgrounded: matches!(tool.status, ToolStatus::Backgrounded),
        }),
        HistoryItem::Summary(summary) => ItemRecord::Summary(SummaryRecord {
            verb: summary.verb.to_string(),
            secs: summary.secs,
            timestamp: summary.timestamp.clone(),
            tokens: summary.tokens,
            cached: summary.cached,
        }),
        HistoryItem::Background(notice) => ItemRecord::Background(BackgroundRecord {
            description: notice.description.clone(),
            id: notice.id.clone(),
            code: notice.code,
            killed: notice.killed,
            output_tail: notice.output_tail.clone(),
            timestamp: notice.timestamp.clone(),
        }),
        HistoryItem::Compaction(compaction) => ItemRecord::Compaction(CompactionRecord {
            summary: compaction.summary.clone(),
            timestamp: compaction.timestamp.clone(),
            before: compaction.before,
            after: compaction.after,
            auto: compaction.auto,
        }),
        // The recorder mirrors history append-only, so a background entry's
        // later completion update (`App::settle_agent_completion`) never
        // rewrites this line — the AgentNotice line recorded after it carries
        // the outcome, and a resumed still-`running` entry reads back as
        // interrupted (agents don't survive a session).
        HistoryItem::AgentGroup(group) => ItemRecord::AgentGroup(AgentGroupRecord {
            background: group.background,
            agents: group
                .agents
                .iter()
                .map(|entry| AgentEntryRecord {
                    id: entry.id.clone(),
                    description: entry.description.clone(),
                    agent_type: entry.agent_type.clone(),
                    prompt: entry.prompt.clone(),
                    status: agent_status_name(entry.status).to_string(),
                    tool_uses: entry.tool_uses,
                    tokens: entry.tokens,
                    secs: entry.secs,
                    result: entry.result.clone(),
                    tool_headers: entry.tool_headers.clone(),
                    output: entry.output.clone(),
                })
                .collect(),
            timestamp: group.timestamp.clone(),
        }),
        HistoryItem::AgentNotice(notice) => ItemRecord::AgentNotice(AgentNoticeRecord {
            id: notice.id.clone(),
            description: notice.description.clone(),
            status: agent_status_name(notice.status).to_string(),
            secs: notice.secs,
            result: notice.result.clone(),
            timestamp: notice.timestamp.clone(),
        }),
    };
    line(stamp, record)
}

/// Serialize one filesystem checkpoint as a rollout line (`docs/checkpoint.md`).
/// Interleaved with the item lines in the same file; extracted by
/// [`parse_checkpoints`] and ignored by [`parse_session`].
#[must_use]
pub fn checkpoint_line(checkpoint: &Checkpoint, stamp: &str) -> String {
    line(
        stamp,
        ItemRecord::Checkpoint(CheckpointRecord {
            commit: checkpoint.commit.clone(),
            after: checkpoint.after,
        }),
    )
}

/// Extract the [`Checkpoint`]s recorded in a rollout file, in file order —
/// the code-reset side channel `/resume` reads alongside [`parse_session`]'s
/// transcript. Malformed and non-checkpoint lines are skipped; an empty vec
/// means the session recorded no checkpoints (an older rollout, or a run with
/// the feature disabled), in which case a resume leaves the code untouched.
#[must_use]
pub fn parse_checkpoints(text: &str) -> Vec<Checkpoint> {
    text.lines()
        .filter_map(|line| serde_json::from_str::<LineRecord>(line.trim()).ok())
        .filter_map(|record| match record.item {
            ItemRecord::Checkpoint(cp) => Some(Checkpoint {
                after: cp.after,
                commit: cp.commit,
            }),
            _ => None,
        })
        .collect()
}

/// Parse a rollout file's text back into its meta + history items, in file
/// order. Malformed lines, unknown record types, and unknown roles are
/// **skipped** (codex's forward-compatible reader); `None` when no valid
/// `session_meta` line exists (an empty or foreign file). Items are collected
/// whether they precede or follow the meta line — in practice the meta is
/// line 1, but the reader doesn't insist.
#[must_use]
pub fn parse_session(text: &str) -> Option<(SessionMeta, Vec<HistoryItem>)> {
    let mut meta: Option<SessionMeta> = None;
    let mut items = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(record) = serde_json::from_str::<LineRecord>(line) else {
            continue; // malformed or unknown-type line: skip, don't fail
        };
        match record.item {
            // The first meta wins (codex: the file's own meta is line 1).
            ItemRecord::SessionMeta(parsed) => {
                meta.get_or_insert(parsed);
            }
            ItemRecord::Message(message) => {
                if let Some(role) = role_from(&message.role) {
                    items.push(HistoryItem::Message(Message {
                        role,
                        text: message.text,
                        timestamp: message.timestamp,
                        images: message.images.into_iter().map(PathBuf::from).collect(),
                    }));
                }
            }
            ItemRecord::Tool(tool) => items.push(HistoryItem::Tool(ToolCall {
                name: tool.name,
                args: tool.args,
                status: if tool.backgrounded {
                    ToolStatus::Backgrounded
                } else if tool.ok {
                    ToolStatus::Ok
                } else {
                    ToolStatus::Failed
                },
                output: tool.output,
                timestamp: tool.timestamp,
                shell: tool.shell,
                truncated: tool.truncated,
            })),
            ItemRecord::Summary(summary) => items.push(HistoryItem::Summary(TurnSummary {
                verb: done_verb(&summary.verb),
                secs: summary.secs,
                timestamp: summary.timestamp,
                // Not persisted: a resumed session's shells are gone, so the
                // suffix must not claim they still run (docs/background.md).
                shells: 0,
                tokens: summary.tokens,
                cached: summary.cached,
            })),
            ItemRecord::Background(notice) => {
                items.push(HistoryItem::Background(crate::app::BackgroundNotice {
                    description: notice.description,
                    id: notice.id,
                    code: notice.code,
                    killed: notice.killed,
                    output_tail: notice.output_tail,
                    timestamp: notice.timestamp,
                }));
            }
            ItemRecord::AgentGroup(group) => {
                items.push(HistoryItem::AgentGroup(AgentGroup {
                    background: group.background,
                    agents: group
                        .agents
                        .into_iter()
                        .map(|entry| AgentGroupEntry {
                            id: entry.id,
                            description: entry.description,
                            agent_type: entry.agent_type,
                            prompt: entry.prompt,
                            status: agent_status_from(&entry.status),
                            tool_uses: entry.tool_uses,
                            tokens: entry.tokens,
                            secs: entry.secs,
                            result: entry.result,
                            tool_headers: entry.tool_headers,
                            output: entry.output,
                        })
                        .collect(),
                    timestamp: group.timestamp,
                }));
            }
            ItemRecord::AgentNotice(notice) => {
                items.push(HistoryItem::AgentNotice(AgentNotice {
                    id: notice.id,
                    description: notice.description,
                    status: agent_status_from(&notice.status),
                    secs: notice.secs,
                    result: notice.result,
                    timestamp: notice.timestamp,
                }));
            }
            ItemRecord::Compaction(compaction) => {
                items.push(HistoryItem::Compaction(crate::app::Compaction {
                    summary: compaction.summary,
                    timestamp: compaction.timestamp,
                    before: compaction.before,
                    after: compaction.after,
                    auto: compaction.auto,
                }));
            }
            // Checkpoints ride the same file but aren't transcript items —
            // `parse_checkpoints` reads them for the code reset. Skip here so
            // the loaded history matches what the user actually said.
            ItemRecord::Checkpoint(_) => {}
        }
    }
    meta.map(|meta| (meta, items))
}

/// The picker preview: the first user input in `items` — a [`Role::User`]
/// message, or a [`Role::Shell`] header shown as `! {command}` (our `!` cells
/// are user input too; see `docs/resume.md`'s divergences) — with all
/// whitespace runs flattened to single spaces so a multiline message stays one
/// row. A whitespace-only message (a pasted blank can reach the record) is
/// skipped rather than claiming the preview as an empty — and unsearchable —
/// string. `None` when the session has no non-blank user input (such files
/// never list).
#[must_use]
pub fn preview_of(items: &[HistoryItem]) -> Option<String> {
    items.iter().find_map(|item| {
        let HistoryItem::Message(message) = item else {
            return None;
        };
        let flat = message
            .text
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        if flat.is_empty() {
            return None; // blank input: keep hunting for a real message
        }
        match message.role {
            Role::User => Some(flat),
            Role::Shell => Some(format!("! {flat}")),
            Role::Assistant | Role::Error | Role::System => None,
        }
    })
}

/// Humanize how long ago something happened — codex's single-unit
/// `format_relative_time`: `now`, `{N}s ago`, `{N}m ago`, `{N}h ago`,
/// `{N}d ago` (integer division at each step).
#[must_use]
pub fn relative_age(secs: u64) -> String {
    if secs == 0 {
        return "now".to_string();
    }
    if secs < 60 {
        return format!("{secs}s ago");
    }
    let minutes = secs / 60;
    if minutes < 60 {
        return format!("{minutes}m ago");
    }
    let hours = minutes / 60;
    if hours < 24 {
        return format!("{hours}h ago");
    }
    format!("{}d ago", hours / 24)
}

/// The rollout file's path relative to the sessions root:
/// `YYYY/MM/DD/rollout-YYYY-MM-DDThh-mm-ss-{id}.jsonl` — codex's layout, with
/// `-` for `:` in the time so the name stays filesystem-safe. `date`/`time`
/// come from the boundary's clock (local time, like codex).
#[must_use]
pub fn rollout_rel_path(date: (i32, u32, u32), time: (u32, u32, u32), id: &str) -> PathBuf {
    let (year, month, day) = date;
    let (hour, minute, second) = time;
    PathBuf::from(format!("{year:04}"))
        .join(format!("{month:02}"))
        .join(format!("{day:02}"))
        .join(format!(
            "rollout-{year:04}-{month:02}-{day:02}T{hour:02}-{minute:02}-{second:02}-{id}.jsonl"
        ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta() -> SessionMeta {
        SessionMeta {
            id: "abc123".into(),
            timestamp: "2026-07-06T10:00:00.000Z".into(),
            cwd: "/home/user/repo".into(),
            model: "dummy_model_name".into(),
            originator: "alter-zero".into(),
            version: "0.1.0".into(),
        }
    }

    fn message(role: Role, text: &str) -> HistoryItem {
        HistoryItem::Message(Message {
            role,
            text: text.into(),
            timestamp: "03:20 PM".into(),
            images: Vec::new(),
        })
    }

    // ===== the line format (docs/resume.md) =====

    #[test]
    fn meta_line_is_a_tagged_session_meta_json_line() {
        let line = meta_line(&meta(), "2026-07-06T10:00:00.000Z");
        let value: serde_json::Value = serde_json::from_str(&line).expect("valid JSON");
        assert_eq!(value["type"], "session_meta");
        assert_eq!(value["timestamp"], "2026-07-06T10:00:00.000Z");
        assert_eq!(value["payload"]["id"], "abc123");
        assert_eq!(value["payload"]["cwd"], "/home/user/repo");
        assert_eq!(value["payload"]["model"], "dummy_model_name");
        // One line of JSONL: no embedded newline.
        assert!(!line.contains('\n'));
    }

    #[test]
    fn message_line_records_role_text_and_stamp() {
        let line = item_line(&message(Role::User, "hello there"), "t1");
        let value: serde_json::Value = serde_json::from_str(&line).expect("valid JSON");
        assert_eq!(value["type"], "message");
        assert_eq!(value["timestamp"], "t1");
        assert_eq!(value["payload"]["role"], "user");
        assert_eq!(value["payload"]["text"], "hello there");
        assert_eq!(value["payload"]["timestamp"], "03:20 PM");
    }

    #[test]
    fn user_message_images_round_trip() {
        // The Ctrl+V attachment paths ride the message record so a /resume
        // restores them for the conversation context (docs/context.md).
        let item = HistoryItem::Message(Message {
            role: Role::User,
            text: "[Image #1] what is this?".into(),
            timestamp: "03:20 PM".into(),
            images: vec![PathBuf::from("/tmp/alter-zero-clipboard-a.png")],
        });
        let (_, parsed) = parse_session(&file_of(std::slice::from_ref(&item))).expect("parses");
        assert_eq!(parsed, vec![item]);
    }

    #[test]
    fn a_compaction_marker_round_trips() {
        // The /compact marker persists so a /resume stays compacted
        // (docs/compact.md); the summary — and the gauge's before/after token
        // counts + the auto tag — are the payload.
        let item = HistoryItem::Compaction(crate::app::Compaction {
            summary: "we did the thing".into(),
            timestamp: "03:20 PM".into(),
            before: 88_000,
            after: 2_100,
            auto: true,
        });
        let line = item_line(&item, "t");
        let value: serde_json::Value = serde_json::from_str(&line).expect("valid JSON");
        assert_eq!(value["type"], "compaction");
        assert_eq!(value["payload"]["summary"], "we did the thing");
        let (_, parsed) = parse_session(&file_of(std::slice::from_ref(&item))).expect("parses");
        assert_eq!(parsed, vec![item]);
    }

    #[test]
    fn a_compaction_line_without_token_info_parses_with_defaults() {
        // Rollouts written before the gauge fields still load (the
        // forward-compatibility contract) — counts default to 0, auto false.
        let old =
            r#"{"timestamp":"t","type":"compaction","payload":{"summary":"s","timestamp":""}}"#;
        let text = format!("{}\n{old}\n", meta_line(&meta(), "t0"));
        let (_, parsed) = parse_session(&text).expect("parses");
        let [HistoryItem::Compaction(compaction)] = parsed.as_slice() else {
            panic!("one compaction parses, got {parsed:?}");
        };
        assert_eq!(compaction.summary, "s");
        assert_eq!((compaction.before, compaction.after), (0, 0));
        assert!(!compaction.auto);
    }

    #[test]
    fn a_message_line_without_images_still_parses() {
        // Rollout files written before the images field omit it entirely —
        // they must keep loading (the forward-compatibility contract).
        let old = r#"{"timestamp":"t","type":"message","payload":{"role":"user","text":"hi","timestamp":""}}"#;
        let text = format!("{}\n{old}\n", meta_line(&meta(), "t0"));
        let (_, parsed) = parse_session(&text).expect("parses");
        let [HistoryItem::Message(message)] = parsed.as_slice() else {
            panic!("one message parses, got {parsed:?}");
        };
        assert_eq!(message.text, "hi");
        assert!(message.images.is_empty());
    }

    #[test]
    fn a_non_utf8_image_path_records_lossily_instead_of_panicking() {
        // Serializing a non-UTF8 PathBuf is a serde_json error — it must not
        // break `line`'s infallibility and panic the recorder mid-session.
        // The lossy path merely fails to open later ([image unavailable]).
        use std::os::unix::ffi::OsStrExt;
        let bad = PathBuf::from(std::ffi::OsStr::from_bytes(b"/tmp/bad-\xff.png"));
        let item = HistoryItem::Message(Message {
            role: Role::User,
            text: "[Image #1]".into(),
            timestamp: String::new(),
            images: vec![bad],
        });
        let line = item_line(&item, "t"); // must not panic
        let value: serde_json::Value = serde_json::from_str(&line).expect("valid JSON");
        let recorded = value["payload"]["images"][0].as_str().expect("a string");
        assert!(recorded.starts_with("/tmp/bad-"), "{recorded:?}");
    }

    #[test]
    fn an_imageless_message_line_keeps_the_old_shape() {
        // No `images` key on the wire when there are none, so files stay
        // byte-identical to the pre-images format in the common case.
        let line = item_line(&message(Role::User, "hi"), "t");
        assert!(!line.contains("images"), "no empty images key: {line}");
    }

    #[test]
    fn multiline_and_quoted_text_stays_one_jsonl_line() {
        // The whole reason serde_json is here: arbitrary user text — quotes,
        // newlines, unicode — must survive without hand-rolled escaping.
        let tricky = "line one\nline \"two\" — emoji 🎉, CJK 汉字";
        let line = item_line(&message(Role::Assistant, tricky), "t");
        assert!(!line.contains('\n'), "JSONL lines never embed newlines");
        let value: serde_json::Value = serde_json::from_str(&line).expect("valid JSON");
        assert_eq!(value["payload"]["text"], tricky);
    }

    // ===== round-trip through parse_session =====

    /// A session file built from `items` the way the recorder writes one.
    fn file_of(items: &[HistoryItem]) -> String {
        let mut text = meta_line(&meta(), "t0");
        text.push('\n');
        for item in items {
            text.push_str(&item_line(item, "t"));
            text.push('\n');
        }
        text
    }

    // ===== checkpoints (docs/checkpoint.md) =====

    #[test]
    fn checkpoint_line_is_a_tagged_json_line() {
        let cp = Checkpoint {
            after: 4,
            commit: "deadbeef".into(),
        };
        let line = checkpoint_line(&cp, "t9");
        let value: serde_json::Value = serde_json::from_str(&line).expect("valid JSON");
        assert_eq!(value["type"], "checkpoint");
        assert_eq!(value["timestamp"], "t9");
        assert_eq!(value["payload"]["commit"], "deadbeef");
        assert_eq!(value["payload"]["after"], 4);
        assert!(!line.contains('\n'));
    }

    #[test]
    fn parse_checkpoints_extracts_them_in_file_order() {
        // A realistic file: meta, a turn, its checkpoint, another checkpoint.
        let mut text = file_of(&[message(Role::User, "hi"), message(Role::Assistant, "yo")]);
        text.push_str(&checkpoint_line(
            &Checkpoint {
                after: 0,
                commit: "pristine".into(),
            },
            "t",
        ));
        text.push('\n');
        text.push_str(&checkpoint_line(
            &Checkpoint {
                after: 2,
                commit: "after-turn".into(),
            },
            "t",
        ));
        text.push('\n');
        assert_eq!(
            parse_checkpoints(&text),
            vec![
                Checkpoint {
                    after: 0,
                    commit: "pristine".into()
                },
                Checkpoint {
                    after: 2,
                    commit: "after-turn".into()
                },
            ]
        );
    }

    #[test]
    fn checkpoint_lines_are_invisible_to_the_transcript_parse() {
        // A checkpoint line among the items must not become a history item —
        // resume loads the same transcript whether or not checkpoints exist.
        let items = vec![message(Role::User, "hi"), message(Role::Assistant, "yo")];
        let mut text = file_of(&items);
        text.push_str(&checkpoint_line(
            &Checkpoint {
                after: 2,
                commit: "abc".into(),
            },
            "t",
        ));
        text.push('\n');
        let (_, parsed) = parse_session(&text).expect("parses");
        assert_eq!(parsed, items, "checkpoints don't leak into the transcript");
        // …but the sidecar sees them.
        assert_eq!(parse_checkpoints(&text).len(), 1);
    }

    #[test]
    fn every_role_round_trips() {
        let items = vec![
            message(Role::User, "hi"),
            message(Role::Assistant, "hello"),
            message(Role::System, "notice"),
            message(Role::Error, "boom"),
            message(Role::Shell, "pwd"),
        ];
        let (parsed_meta, parsed) = parse_session(&file_of(&items)).expect("parses");
        assert_eq!(parsed_meta, meta());
        assert_eq!(parsed, items);
    }

    #[test]
    fn finished_tools_round_trip_ok_failed_and_truncated() {
        let ok_tool = HistoryItem::Tool(ToolCall {
            name: "search".into(),
            args: "query".into(),
            status: ToolStatus::Ok,
            output: "row one\nrow two".into(),
            timestamp: "03:21 PM".into(),
            shell: false,
            truncated: false,
        });
        let failed_shell = HistoryItem::Tool(ToolCall {
            name: "tree ~/".into(),
            args: String::new(),
            status: ToolStatus::Failed,
            output: "huge\u{2026}".into(),
            timestamp: String::new(),
            shell: true,
            truncated: true,
        });
        let (_, parsed) =
            parse_session(&file_of(&[ok_tool.clone(), failed_shell.clone()])).expect("parses");
        assert_eq!(parsed, vec![ok_tool, failed_shell]);
    }

    #[test]
    fn summary_verb_maps_back_to_the_done_verbs_static() {
        let summary = HistoryItem::Summary(TurnSummary {
            verb: DONE_VERBS[2],
            secs: 7,
            timestamp: "03:22 PM".into(),
            shells: 0,
            tokens: 0,
            cached: 0,
        });
        let (_, parsed) = parse_session(&file_of(std::slice::from_ref(&summary))).expect("parses");
        assert_eq!(parsed, vec![summary]);
        // The parsed verb is one of the known statics, restored by value
        // (DONE_VERBS is a `const`, so pointer identity can't be asserted).
        let HistoryItem::Summary(s) = &parsed[0] else {
            panic!("expected a summary");
        };
        assert!(DONE_VERBS.contains(&s.verb));
    }

    #[test]
    fn an_unknown_summary_verb_falls_back_to_done() {
        // A file written by a build with different verbs still loads.
        let file = format!(
            "{}\n{}\n",
            meta_line(&meta(), "t0"),
            r#"{"timestamp":"t","type":"summary","payload":{"verb":"Vanished","secs":3,"timestamp":""}}"#,
        );
        let (_, parsed) = parse_session(&file).expect("parses");
        let HistoryItem::Summary(s) = &parsed[0] else {
            panic!("expected a summary");
        };
        assert_eq!(s.verb, "Done");
        assert_eq!(s.secs, 3);
    }

    #[test]
    fn malformed_and_unknown_lines_are_skipped_not_fatal() {
        // Codex's forward-compatible reader: garbage, blank lines, unknown
        // record types, and unknown roles all skip; the good items survive.
        let file = format!(
            "{}\n{{\n\n{}\nnot json at all\n{}\n{}\n",
            meta_line(&meta(), "t0"),
            r#"{"timestamp":"t","type":"world_state","payload":{"future":true}}"#,
            r#"{"timestamp":"t","type":"message","payload":{"role":"overseer","text":"?","timestamp":""}}"#,
            item_line(&message(Role::User, "kept"), "t"),
        );
        let (_, parsed) = parse_session(&file).expect("parses");
        assert_eq!(parsed, vec![message(Role::User, "kept")]);
    }

    #[test]
    fn a_file_without_a_meta_line_is_none() {
        let file = format!("{}\n", item_line(&message(Role::User, "hi"), "t"));
        assert_eq!(parse_session(&file), None);
        assert_eq!(parse_session(""), None);
        assert_eq!(parse_session("garbage\n"), None);
    }

    // ===== the picker preview (docs/resume.md) =====

    #[test]
    fn preview_is_the_first_user_message_flattened() {
        let items = vec![
            message(Role::System, "notice first"),
            message(Role::User, "fix the\n  wrap   bug"),
            message(Role::User, "second question"),
        ];
        assert_eq!(preview_of(&items).as_deref(), Some("fix the wrap bug"));
    }

    #[test]
    fn a_shell_header_previews_with_its_bang() {
        let items = vec![message(Role::Shell, "cargo test")];
        assert_eq!(preview_of(&items).as_deref(), Some("! cargo test"));
    }

    #[test]
    fn a_session_with_no_user_input_has_no_preview() {
        let items = vec![
            message(Role::Assistant, "unprompted?"),
            message(Role::Error, "boom"),
        ];
        assert_eq!(preview_of(&items), None);
        assert_eq!(preview_of(&[]), None);
    }

    #[test]
    fn a_whitespace_only_user_message_is_skipped_for_the_preview() {
        // A pure-whitespace first message (a whitespace paste can reach the
        // record) must not claim the preview as an empty — and unsearchable —
        // string; the hunt continues to the next real user input.
        let items = vec![
            message(Role::User, " \n \t "),
            message(Role::User, "real question"),
        ];
        assert_eq!(preview_of(&items).as_deref(), Some("real question"));
        // Only whitespace input in the whole session ⇒ no preview at all
        // (the session never lists, same as no user input).
        assert_eq!(preview_of(&[message(Role::User, "  ")]), None);
        assert_eq!(preview_of(&[message(Role::Shell, " \n")]), None);
    }

    // ===== relative_age (codex's format_relative_time) =====

    #[test]
    fn relative_age_buckets_match_codex() {
        assert_eq!(relative_age(0), "now");
        assert_eq!(relative_age(1), "1s ago");
        assert_eq!(relative_age(59), "59s ago");
        assert_eq!(relative_age(60), "1m ago");
        assert_eq!(relative_age(3_599), "59m ago");
        assert_eq!(relative_age(3_600), "1h ago");
        assert_eq!(relative_age(86_399), "23h ago");
        assert_eq!(relative_age(86_400), "1d ago");
        assert_eq!(relative_age(864_000), "10d ago");
    }

    // ===== the file path (codex's layout) =====

    #[test]
    fn rollout_rel_path_pads_and_dashes_the_stamp() {
        assert_eq!(
            rollout_rel_path((2026, 7, 6), (3, 4, 5), "1a2b-3c"),
            PathBuf::from("2026/07/06/rollout-2026-07-06T03-04-05-1a2b-3c.jsonl"),
        );
    }

    // ===== background shells (docs/background.md) =====

    #[test]
    fn a_backgrounded_tool_round_trips_its_status() {
        let tool = HistoryItem::Tool(ToolCall {
            name: "Bash".into(),
            args: "ping -c 200 x.com".into(),
            status: ToolStatus::Backgrounded,
            output: "Command running in background with ID: bash_1.".into(),
            timestamp: "03:20 PM".into(),
            shell: false,
            truncated: false,
        });
        let line = item_line(&tool, "t");
        let value: serde_json::Value = serde_json::from_str(&line).expect("valid JSON");
        assert_eq!(value["payload"]["backgrounded"], true);
        let (_, parsed) = parse_session(&file_of(std::slice::from_ref(&tool))).expect("parses");
        assert_eq!(parsed, vec![tool]);
    }

    #[test]
    fn a_normal_tool_line_omits_the_backgrounded_field() {
        // Files written before the feature keep their exact shape — and old
        // lines (without the field) still parse as not-backgrounded.
        let tool = HistoryItem::Tool(ToolCall {
            name: "Bash".into(),
            args: "ls".into(),
            status: ToolStatus::Ok,
            output: "Exit code: 0".into(),
            timestamp: String::new(),
            shell: false,
            truncated: false,
        });
        let line = item_line(&tool, "t");
        assert!(
            !line.contains("backgrounded"),
            "the field is skipped when false: {line}"
        );
        let old_line = r#"{"timestamp":"t","type":"tool","payload":{"name":"Bash","args":"ls","ok":true,"output":"Exit code: 0","timestamp":"","shell":false,"truncated":false}}"#;
        let text = format!("{}\n{old_line}\n", meta_line(&meta(), "t0"));
        let (_, parsed) = parse_session(&text).expect("parses");
        assert_eq!(parsed, vec![tool]);
    }

    #[test]
    fn a_background_notice_round_trips() {
        let notice = HistoryItem::Background(crate::app::BackgroundNotice {
            description: "Ping x.com 200 times".into(),
            id: "bash_1".into(),
            code: Some(0),
            killed: false,
            output_tail: "64 bytes from x.com\n200 packets transmitted".into(),
            timestamp: "03:21 PM".into(),
        });
        let line = item_line(&notice, "t");
        let value: serde_json::Value = serde_json::from_str(&line).expect("valid JSON");
        assert_eq!(value["type"], "background");
        assert_eq!(value["payload"]["id"], "bash_1");
        let (_, parsed) = parse_session(&file_of(std::slice::from_ref(&notice))).expect("parses");
        assert_eq!(parsed, vec![notice]);
    }

    #[test]
    fn a_killed_signal_notice_round_trips_the_none_code() {
        let notice = HistoryItem::Background(crate::app::BackgroundNotice {
            description: "sleep 100".into(),
            id: "bash_2".into(),
            code: None,
            killed: true,
            output_tail: String::new(),
            timestamp: String::new(),
        });
        let (_, parsed) = parse_session(&file_of(std::slice::from_ref(&notice))).expect("parses");
        assert_eq!(parsed, vec![notice]);
    }

    #[test]
    fn summary_shells_are_not_persisted() {
        // A resumed session's shells are gone — the parsed summary must not
        // claim they still run.
        let summary = HistoryItem::Summary(TurnSummary {
            verb: DONE_VERBS[0],
            secs: 22,
            timestamp: String::new(),
            shells: 3,
            tokens: 0,
            cached: 0,
        });
        let line = item_line(&summary, "t");
        assert!(!line.contains("shells"), "not recorded: {line}");
        let (_, parsed) = parse_session(&file_of(std::slice::from_ref(&summary))).expect("parses");
        let HistoryItem::Summary(parsed) = &parsed[0] else {
            panic!("a summary parses back");
        };
        assert_eq!(parsed.shells, 0);
    }

    #[test]
    fn summary_usage_round_trips() {
        // The real billed tokens are a fact of the turn (unlike the transient
        // shells count) — a resumed transcript keeps them
        // (docs/prompt-caching.md).
        let summary = HistoryItem::Summary(TurnSummary {
            verb: DONE_VERBS[1],
            secs: 9,
            timestamp: String::new(),
            shells: 0,
            tokens: 8_203,
            cached: 8_063,
        });
        let (_, parsed) = parse_session(&file_of(std::slice::from_ref(&summary))).expect("parses");
        assert_eq!(parsed, vec![summary]);
    }

    #[test]
    fn a_summary_line_recorded_before_usage_existed_still_parses() {
        // Rollouts written by older builds have no tokens/cached keys — they
        // load with zeroes, never an error.
        let mut text = format!("{}\n", meta_line(&meta(), "t"));
        text.push_str(
            "{\"timestamp\":\"t\",\"type\":\"summary\",\"payload\":{\"verb\":\"Done\",\"secs\":4,\"timestamp\":\"\"}}\n",
        );
        let (_, parsed) = parse_session(&text).expect("an old line parses");
        let HistoryItem::Summary(parsed) = &parsed[0] else {
            panic!("a summary parses back");
        };
        assert_eq!((parsed.tokens, parsed.cached), (0, 0));
        assert_eq!(parsed.secs, 4);
    }
}
