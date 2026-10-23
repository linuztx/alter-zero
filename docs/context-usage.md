# `/context` — what fills the context window

Date: 2026-10-22

## Goal

A **`/context`** slash command that answers the two questions a long session
keeps raising: *how full is the context window*, and *what is filling it*. The
footer's gauge (`docs/footer.md`) already says how full; nothing said what. A
system prompt, two dozen tool schemas, a stack of MCP tools, the project's
`AGENTS.md`, the skills listing and the conversation itself all ride every
request, and they are invisible until a provider refuses one.

The page breaks the window into those parts, shows each one's share, lets the
user drill into any of them, and reports what the provider said about the last
request — tokens in and out, and how much of the input came from the prompt
cache (`docs/prompt-caching.md`).

## The page

```
──────────────────────────────────────────────────────────────────────────────
  Context usage
  gpt-5.5 · ≈36.4k of 200k tokens (18.2%)

  ██████████████░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░╱╱╱╱╱╱╱

    ■ System prompt     161   0.1%   Messages · 48 messages
    ■ System tools     3.2k   1.6%   Tool results   17.2k  ━━━━━━━━━━━━
    ■ MCP tools        1.1k   0.6%   Tool calls      6.1k  ━━━━
    ■ Project docs     1.6k   0.8%   Replies         4.3k  ━━━
    ■ Skills            433   0.2%   Your messages   2.2k  ━━
    ■ Agent types       122   0.1%
  ❯ ■ Messages        29.8k  14.9%
    □ Free space     143.6k  71.8%
    ╱ Auto-compact      20k  10.0%

  Last request   38.1k in · 1.2k out
  Cache hit      ████████████████████░░░░  83%  31.6k read
  Since launch   14 requests · 402k in · 9.8k out · 79% cached

  ↑↓ select  ctrl+d raw context  esc close
──────────────────────────────────────────────────────────────────────────────
```

It is a composer-replacing inline page — the `/donate` and `/export` pages'
family (`docs/donate.md`, `docs/export.md`): the same rules, the same
two-column inset, the `❯` on the highlighted row, the banner-gradient title,
no text entry and so no hardware cursor (its seat tracks the `❯`), every key
owned while it is open, the composer back on Esc. It works **mid-turn**: the
streaming strip keeps its rows above it, exactly as above every page.

Why a page and not a scrollback report (Claude Code's `/context` prints one
into the transcript): a report is a snapshot, and the question is usually
asked *while* the window is filling — mid-turn, with tool output landing. The
page is live (below), it can be explored, and it leaves nothing in the
conversation; the alternate screen (Ctrl+O, Ctrl+D, `/diff`) would be the
heavier choice for a page whose whole content fits a quarter of the terminal.

### The header

The title wears the pages' one title dress (`ui::header::page_title_line`).
The subtitle names the model of the conversation on screen and the window's
fill: `{model} · ≈{total} of {window} tokens ({pct}%)`, every count through
`ui::format_token_count` — the footer's and the receipt's humanizer. The `≈`
is deliberate (*Estimated and reported*, below). The share turns amber past
75% and red past the auto-compact threshold, the one colour on the line, so
a window about to compact says so before anything is read. With no window
known the line reads `{model} · ≈{total} tokens · window unknown` and every
percentage on the page is a share of the total instead.

### The window bar

One row, the page's width less the inset on both sides, standing for the
whole window: each category's cells in its own colour, in the order the
request carries them (system prompt, tools, the reminder's sections,
messages), then the free space (`░`, dim), then the **auto-compact headroom**
— the last 10% the session never fills, because at 90% the loop compacts on
its own (`docs/compact.md`) — hatched `╱`. Cells are allotted by rounding
each segment's **cumulative** boundary rather than its own size
(`allot_cells`), so the segments always sum to the bar, the hatching starts
at 90% of it to the cell, and no run of small categories rounds itself away;
a category with any tokens at all gets at least one cell, so the 161-token
system prompt is a visible sliver rather than nothing. A conversation already
past the threshold fills the hatched cells; one past the window fills the
bar. With no window known the bar is the total's own proportions.

### The legend and the detail

One row per category the window actually holds — a category with no tokens is
left out, except **Messages**, which is always there (an empty conversation is
a fact worth stating) — then **Free space** and **Auto-compact** when a window
is known (the latter only while the `/settings` **Auto compact** knob is on).
Each row is a swatch in the category's colour (`■`, the free space's `□`, the
headroom's `╱`), the label, the tokens and the share, in aligned columns — a
share too small to print as a tenth reading `<0.1%` rather than a `0.0%` that
would say the category is free. The colours are theme roles
(`ui::theme::context_usage_color`): the system prompt and the tools keep the
Ctrl+D view's own two, the messages the accent, the project docs the link
blue, the skills the success green, the secrets the error red, and the two
categories with no role of their own are blends — the MCP tools between the
warning and the error hues, the agent types between the purple and the red —
so every row stays distinct in every theme. The terminal's own sixteen
colours (`ansi`) have no components to mix, and a blend of two named colours
would step to one of its ends — the MCP tools and the agent types both
landing on the secrets' red — so there each takes the bright variant of its
first hue instead (bright yellow, bright magenta; `palette::bright`).

The highlighted row's **detail** sits beside the legend on a terminal wide
enough for both (`CONTEXT_USAGE_SIDE_BY_SIDE_MIN_COLS`), and under it
otherwise: the category's own breakdown, largest first, each part a label, its
tokens and a bar measured against the largest part, the tail folded into one
`… N more` row carrying their sum. The highlight opens on the **largest**
category, so the first thing on screen is the answer to "what is using my
context?" — and **follows** the largest until the user moves it, since the
page is counted as it opens and the tool set the boundary hands over right
after it (an MCP server connecting later, a conversation outgrowing the tool
schemas mid-turn) can change which category that is. Once the user has picked
a row, a re-count leaves the highlight on it; only a row that disappears
sends it back to the largest.

| category | what it counts | its parts |
| --- | --- | --- |
| System prompt | `App::system_prompt` — persona → environment → scratchpad (`docs/environment.md`) | one per markdown heading block (`System prompt`, `Environment`, `Scratchpad`) |
| System tools | every built-in tool schema the next request offers | one per tool, by wire name |
| MCP tools | every `mcp__server__tool` schema (`docs/mcp.md`) | one per server, its tool count in the label |
| Project docs | the reminder's `AGENTS.md` section (`docs/project-doc.md`) | one per file, by path |
| Skills | the reminder's skills section (`docs/skills.md`) | one per listed skill |
| Agent types | the reminder's subagent-type section (`docs/subagents.md`) | one per type |
| Secrets | the reminder's secrets section (`docs/secrets.md`) — names and context only, never a value | one per secret |
| Messages | the derived conversation (`context::context_messages`, `docs/context.md`) | your messages, shell commands, app notices, the compaction summary, replies, tool calls, tool results, images |

The reminder's wrapper — the tags and its one-line preamble, a few tokens —
counts with its first section, so the reminder's sections add up to the
reminder. **Free space** and **Auto-compact** have no parts; their detail says
how far the conversation is from the threshold and the window, and what runs
there.

Every row's tokens come from the derivation the request is built from, never
a parallel model of it: the tool schemas are the very JSON the client sends,
the reminder is the block the derived context leads with, and the messages
are `context::context_messages` — what Ctrl+D shows and the next request
carries. So the page cannot describe a context the model does not get. The
tools are counted in the chat-completions shape the client keeps them in; a
wire that translates them (the Anthropic, Responses and Ollama requests,
`docs/claude.md`, `docs/chatgpt.md`, `docs/ollama.md`) sends the same names,
descriptions and parameters in its own framing, a few tokens either way.

The reminder is cut in two ways. Its instructions section is the text the
session stored (`App::user_instructions`), found whole where the reminder
carries it: an `AGENTS.md` is the user's own text and may quote any of the
headers below, so no header inside it may cut it. The listings after it are
the session's own rendering, in a fixed order, and each is found by the
header its contributor writes (`skills::SKILL_LISTING_HEADER`,
`subagents::AGENT_LISTING_HEADER`, `secrets::SECRET_LISTING_HEADER`), from
the end, where the session writes it. A subagent's briefing carries no
instructions, so it has no **Project docs** row.

The **Messages** parts split the user-role text by where it came from rather
than by its role, because much of what rides as a user message is not
something the user typed: the derivation notes each text's origin as it
lands (`context::context_messages_with_origins` — the same messages, with a
`TextSegment` per user-role text naming its `TextOrigin`), so **Your
messages** is only what was typed, **Shell commands** the `!` transcripts,
**App notices** the `[error]`/`[system]` notices, the background and agent
notices and the hook notes, **Summary** the newest compaction's bridge, and
an image read's attachment note counts with the **Tool results** it
belongs to.

### Usage and the cache

Under the legend, what the **provider** reported — real numbers, never
estimated (`docs/prompt-caching.md` *Real or estimated*):

- **Last request** — the newest usage frame's input and output tokens.
- **Cache hit** — the share of that request's input served from the prompt
  cache, as a meter in the success green, beside the read and written counts
  (each omitted when zero). A provider that reports no cache accounting
  (Ollama) reads `0%`.
- **Since launch** — every request this process has made, the subagents'
  included: their count, input and output totals, and the share of all input
  that was a cache read.

Before the first frame (the offline dummy never sends one) the section is one
dim row saying so. The last request describes the conversation it was sent
for, so whatever replaces that conversation — a `/clear`, a `/resume`, an
Esc-Esc rewind, a compaction (whose newest frame is the summarizer's own
request) — takes it too, and the row says so again until the next frame. The
ledger is the session's spend and keeps every frame.

A share is a whole percent that never claims all or none it is not: a
99.6% hit reads `>99%` and a 0.3% one `<1%`.

## Estimated and reported

The categories are counted with the app's tokenizer — `o200k_base`, the seam
every live tally goes through (`docs/tokenizer.md`): exact for OpenAI models,
close for the rest, and blind to what a provider adds on its own side (a chat
template's framing, an image's real cost, its own rendering of a tool
schema). A request's real size is the provider's to say, and it says it once
per round. So the page shows both, and labels which is which: the breakdown
and the bar are estimates (the `≈`), the usage section is the provider's
accounting. The footer's gauge is the provider's number after a round
(`docs/compact.md`), so the two can differ by the overhead the estimate
cannot see — and the page's total, unlike the gauge, counts the system
prompt, the tools and the reminder even before the first message, because
they are what the first request will cost.

## Live

The page never ticks — nothing on it animates — but it changes as the
conversation does: a tool round lands, an MCP server connects, a skill is
rescanned at a turn start. So the boundary refreshes it on the open and at
every loop bottom while it is open (`tui::context_usage`,
`Session::sync_context_usage`): the tool set is pulled from the backend
(`ReplySource::tool_definitions`, a shared handle the backend rebuilds with
its tool set, so the pull costs a pointer copy) and the
pure core re-counts only when its **signature** moves —
`(history generation, history length)`, the viewed agent and its transcript
length, and a hash of the system prompt, the reminder inputs and the tool
set — so it never counts per frame, only on a change. And a re-count reads
each text's tokens from the page's **memo** (`context_usage::CountMemo`):
every text the last pass counted is known by its length and hash, so only
what is new goes through the tokenizer, and whatever a pass no longer
counts is forgotten, which bounds the memo to two windows' entries. That is
what keeps a busy turn smooth under an open page: measured on a 288k-token
conversation, a whole-window count takes 45 ms and the re-count after a
commit under a millisecond, where the unmemoized re-count stalled 55 ms on
every commit. Opening the page pays the whole count once. The usage section
is read live from `App` at every paint.

The page signs its view flow on its rows (`docs/view-flow.md`), like every
page that changes on its own schedule rather than per frame — though it
never flows: every terminal gets a page that fits (below).

## Inside an agent session view

The page describes the conversation on screen, the rule `/copy`, `/export`,
Ctrl+D and the footer gauge already follow (`docs/agent-view-streaming.md`,
`docs/agent-context-gauge.md`): inside a subagent's session view it is that
agent's window — its own system prompt (`App::agent_system_prompt`), the tool
set its type is offered (`ReplySource::agent_tool_definitions`, the same
allowlisted set its launch builds — kept by the backend until the type or
its allowlist moves, since the page asks at every loop bottom), its briefing
(`App::agent_system_reminder`) and its transcript — against the window of the
model it runs on, with its own last request and a **This agent** ledger in
place of the session's. A subagent never auto-compacts, so its bar has no
hatched headroom.

## Keys

| key            | does                                                           |
| -------------- | -------------------------------------------------------------- |
| ↑ / ↓          | move the highlight, wrapping at the ends                       |
| Home / End     | the first / last row                                           |
| Ctrl+D         | the raw context window (`docs/context.md`) — the page stays open behind it |
| Ctrl+O         | the transcript, likewise                                       |
| Esc / Ctrl+C   | close — the composer returns                                   |

The two read-only overlays pass through for the permission prompt's reason:
they show what the page measures, and they only scroll. An overlay opened
from the page is a look, so its Esc goes back to the page and never arms the
Esc-Esc rewind (`App::overlay_esc_backtracks`): a rewind begun there would
land back on a page still open over the draft it recalled. Everything else
is swallowed, pastes included.

## A terminal too short for the page

The page builds the first of its pages that fits the rows it may have
(`model_view::first_page_that_fits`, `docs/view-flow.md` *A page that sheds
rows before it flows*), shedding in this order: the detail's tail (cut to
three parts, then — where it sits under the legend — gone, while a detail
beside the legend costs no rows and stays), the **Since launch** row, the
cache meter (its share riding the **Last request** row instead), the bar and
the subtitle together — the title folding into one row with the window's
fill, since how full the window is outlasts both the bar and the model's
name — and the usage row. Past those the legend itself is windowed: three
rows riding the highlight between reserved `↑`/`↓` slots, under the title
row and over the hint, the compact pages' shape (`model_view::compact_page`,
the `/theme` and `/skills` pickers' own), with the detail beside the window
where the terminal is wide enough. That ladder sheds the overflow slots, the
top rule, the rows down to the highlighted one, the bottom rule and the hint,
leaving the title row alone — so no terminal is too short for a page, and
nothing flows into scrollback.

```
──────────────────────────────────────────────────────────
  Context usage · ≈36.4k of 200k tokens (18.2%)
  ↑ 5 more above
    ■ Agent types       122   0.1%
  ❯ ■ Messages        29.8k  14.9%
    □ Free space     143.6k  71.8%
  ↓ 1 more below
  ↑↓ select  ctrl+d raw context  esc close
──────────────────────────────────────────────────────────
```

## The seam

```rust
fn tool_definitions(&self) -> Arc<[ToolDefinition]>;              // the next request's tools
fn agent_tool_definitions(&self, agent_type: &str) -> Arc<[ToolDefinition]>;
```

A `ToolDefinition` is a wire name and the schema serialized exactly as the
client sends it. Both default to empty — the dummy offers the model nothing.
`LlmBackend` keeps its own set beside the client's `tools` array, rebuilt by
the one function that rebuilds that array (`sync_tool_specs`), so the two
cannot drift; the subagent set is built by the same function its launch uses
(`subagent_tool_specs`, beside `spawn_subagent_run`) and kept with the type
and the allowlist it was built from, the one input a per-turn rescan
rewrites in place. The skills verdict and the MCP set are the backend's as
built — the boundary rebuilds the backend when either moves
(`refresh_skills`, `refresh_mcp`), which starts a fresh set — so, like the
lead's own, the agent's describes the backend as it stands.

## API

- `context_usage` (pure): `ToolDefinition`, `UsageLedger`, `Category`,
  `Part`, `Slice`, `Breakdown`, `Sources` (its `instructions` and `origins`
  what cut the reminder and the messages), `breakdown`, `CountMemo`.
- `app`: `ContextUsagePage`, `ContextUsageRow`; `App::open_context_usage`,
  `close_context_usage`, `refresh_context_usage`, `set_tool_definitions`,
  `set_agent_tool_definitions`, `last_usage`, `usage_since_launch`;
  `CommandEffect::Context`, `Action::OpenContextUsage` /
  `CloseContextUsage`. `AgentRun` gains `last_usage` and `usage`.
- `ui::context_usage_view`: `context_usage_lines` (the page builder; its
  length is the reserved height), `context_usage_height`,
  `render_context_usage`.
- `context::context_messages_with_origins`, `TextOrigin`, `TextSegment` —
  the derived conversation with each user-role text's origin noted, so the
  page can count what was typed apart from what the app said.
- `tui::context_usage` — `Session::sync_context_usage`, the refresh run on
  the open and at the loop bottom.

## Tests

- `context_usage` — every category's attribution over a fixture prompt,
  tool set, reminder and history (the counter injected, so a token is a byte
  and every sum is checkable by hand); the reminder's instructions counted
  as stored however they quote a header, its listings split by their
  headers, its wrapper counted with the first section; the system prompt
  split at headings outside its code fences; MCP tools grouped by server;
  messages split by kind and their user-role text by origin; the ledger's
  sums and hit rate; the memo counting each text once across passes,
  forgetting what a pass skipped, and never changing a count.
- `context` — each user-role text's origin noted where it lands, the
  compaction's texts and its bridge included, over messages identical to
  `context_messages`.
- `llm::backend` — the subagent tool set built once until its allowlist
  moves.
- `app/tests/context_usage.rs` — the palette row, the command opening the
  page idle and mid-turn and inside an agent view, the largest category
  opening highlighted and the highlight following it until the user moves
  it, the key grammar (wrapping, Home/End, Esc/Ctrl+C, the
  overlay pass-through, owns-every-key), an overlay over the page never
  arming a rewind, the re-count happening on a change and only then, the
  ledgers fed by lead and agent frames, the last request dropped with the
  conversation a `/clear`, `/resume`, rewind or compaction replaced, and the
  user's share naming what they did not type.
- `ui/tests/context_usage_view.rs` — the framed page and its hugging frame,
  the bar's allotment (every category visible, the hatched headroom, a
  window over the threshold), the legend's columns and swatches, the detail
  beside and under the legend, the usage rows and the meter, a share short
  of whole never reading as all or none, an unknown window, every category
  its own colour in every theme, the ladder at every height down to the
  windowed legend and the title row alone, width safety down to two
  columns, no flow at any height, the hidden cursor seated on the `❯` on
  every page.
- `scripts/smoke.sh` Phase 144 — the page end to end in a real terminal:
  against an OpenAI-compatible stub with a skill, an `AGENTS.md` and a
  scripted MCP server planted (every category on the legend, the MCP
  server's tools landing on the already-open page, ↓ walking each detail,
  Ctrl+D over the page and back, the provider's in/out and 86% cache hit
  after two turns), then the dummy mid-turn (the status line above the page,
  the conversation re-counted as the turn streams under it).
