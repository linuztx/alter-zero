//! The backend seam: the reply protocol, the [`ReplySource`] trait, and the
//! built-in offline backends.
//!
//! The event loop depends on **this module's seam only** — never on a concrete
//! backend — so swapping in a real model is one `let backend = …;` line (see
//! `tui::event_loop::run` and `docs/design.md`). Everything a backend must speak
//! is here; everything a backend *is* lives behind the trait.
//!
//! One module per area, like `app/` and `ui/` (see `docs/module-layout.md`):
//!
//! - `event`  — [`StreamEvent`] and its payloads: the whole wire format.
//! - `source` — [`ReplySource`], the trait a backend implements.
//! - `cancel` — [`CancelToken`], the shared stop flag a backend polls.
//! - `dummy`  — [`DummyAi`], the canned offline demo backend
//!   (`docs/dummy-backend.md`).
//! - `stall`  — [`StallAi`], the wedged-backend test double
//!   (`docs/interrupt.md`).
//!
//! The real OpenAI-compatible backend is the separate [`crate::llm`] module; it
//! implements the same trait and speaks the same events.

mod cancel;
mod dummy;
mod event;
mod source;
mod stall;

#[cfg(test)]
mod tests;

pub use self::cancel::CancelToken;
pub use self::dummy::{
    AGENT_DELAY, CHUNK_DELAY, DummyAi, STARTUP_DELAY, THINK_CHUNK_DELAY, TOOL_DELAY, chunks,
    dummy_response, image_ack, turn_events,
};
pub use self::event::{AgentCallDone, AgentSpec, StreamEvent, TokenUsage, ToolCallSummary};
pub use self::source::{AgentChatDelivery, ReplySource};
pub use self::stall::StallAi;
