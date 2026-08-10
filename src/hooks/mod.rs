//! Lifecycle hooks — the user's own commands wedged into the agent's
//! lifecycle (`docs/hooks.md`).
//!
//! This module is the **pure** half: the `hooks.json` format, which handlers
//! an event selects, the JSON payload each event writes to a handler's stdin,
//! and the verdict each handler's stdout is parsed back into (plus the rules
//! for merging several handlers' verdicts into one). It knows nothing about
//! processes — a handler's captured stdout arrives as a `String` — so every
//! rule here is unit-testable from fixtures, the same split `session` and
//! `history` use for their JSONL formats. Spawning is the boundary's job
//! ([`crate::llm::hooks`]).
//!
//! We implement **Claude Code's** wire contract rather than a novel one, which
//! is also the contract codex's `hooks` crate targets (its engine type is
//! literally named `ClaudeHooksEngine`), so a hook script written for either
//! tool works here unchanged. The two halves of that contract are spelled
//! differently and both references agree on the asymmetry, so it is copied
//! exactly: the **payload is `snake_case`**, the **verdict is `camelCase`**.
//!
//! - `event`   — the eleven [`HookEvent`]s and their wire names.
//! - `config`  — the `hooks.json` file format and handler selection.
//! - `matcher` — which handlers an event's match query selects.
//! - `payload` — the per-event stdin JSON.
//! - `verdict` — parsing one handler's answer and merging several.

mod config;
mod event;
mod matcher;
mod payload;
mod verdict;

pub use self::config::{CommandHook, HandlerKind, HookHandler, HooksFile, MatcherGroup, Selection};
pub use self::event::HookEvent;
pub use self::matcher::{invalid_regex, matches};
pub use self::payload::{
    HookContext, permission_request_payload, post_compact_payload, post_tool_use_payload,
    pre_compact_payload, pre_tool_use_payload, session_end_payload, session_start_payload,
    stop_payload, subagent_start_payload, subagent_stop_payload, user_prompt_submit_payload,
};
pub use self::verdict::{
    HOOK_OUTPUT_MAX_BYTES, HookOutcome, HookPermission, HookRun, ParsedHook, merge, parse_run,
    truncate_output,
};
