# `/usage` — what is left of your subscription

Date: 2026-10-25

## Goal

A **`/usage`** slash command that answers the question a subscription keeps
raising: *how much of my plan have I used, and when does it come back?* The
footer's gauge says how full the context window is (`docs/footer.md`) and
`/context` what fills it (`docs/context-usage.md`); neither says anything about
the account. A ChatGPT seat has a weekly window (and on some plans a 5-hour
one), a GitHub Copilot seat a monthly allowance, ClinePass three rolling
windows — and until now the first a user heard of any of them was a refused
turn.

The page shows every **subscription** the app is signed in to — the ones
`/login` lists under *Use a subscription* (`docs/llm.md`) — each with its plan,
a meter per limit, when each one resets, and anything the provider says is
stopping requests now. A pasted API key bills per token and has no plan to
measure against, so API-key providers are not on the page. With no
subscription signed in at all there is nothing to show, and `/usage` opens no
page: a toast says so and points at `/login` → *Use a subscription* — the
`/export` empty rule.

## The page

```
────────────────────────────────────────────────────────────────────────────────────────────────────
  Usage limits
  3 subscriptions · updated just now

  ❯ ChatGPT Codex  Pro · dev@example.com                                                      in use
      Weekly limit   █░░░░░░░░░░░░░░░░░░░   7% used  resets in 6d · Sat 3:58 PM
      Credits        62,500 credits · ≈ 15.6k–81.3k local messages
      Limit resets   2 available

    ClinePass  Monthly
      5-hour limit   ░░░░░░░░░░░░░░░░░░░░   0% used
      Weekly limit   ████████████████████ 100% used  limit reached · resets in 1d 23h · Tue 2:36 PM
      Monthly limit  ████████████████░░░░  81% used  resets in 17d · Nov 12, 1:26 PM
      ⚠ Paused until the weekly limit resets — switch with /model meanwhile.

    GitHub Copilot  Pro
      AI credits     ████████░░░░░░░░░░░░  42% used  420 of 1,000 credits · resets in 6d 8h
      Chat           Unlimited

  Details at https://chatgpt.com/settings/usage
  ↑↓ select  r refresh  c copy link  esc close
────────────────────────────────────────────────────────────────────────────────────────────────────
```

It is a composer-replacing inline page — `/context`'s and `/donate`'s family:
the same rules, the two-column inset, the `❯` on the highlighted section, the
pages' title dress, no text entry and so no hardware cursor (its seat tracks
the `❯`), the composer back on Esc. It works **mid-turn**: the streaming strip
keeps its rows above it, so a long turn can be watched eating into a window.

Why a page and not a scrollback report: the numbers move. The page is live —
it re-reads every subscription each minute while it is open (*Live*, below) —
and it leaves nothing in the conversation.

The subtitle counts the subscriptions and says how fresh their numbers are:
`updated just now`, `updated 3m ago`, `2h ago`, `3d ago` — relative, like
every other age the app shows, since a clock time leaves the reader to do the
subtraction. While a read is in flight it adds `loading…` (the first round,
still landing) or `refreshing…` (over the numbers an earlier round left).

### A section per subscription

The subscription in use comes first and says so (`in use`, in the success
green, flush right); the others follow in `/login`'s order. A section's head
is the provider's name (bold; the accent on the highlighted section), the plan
(`Pro`, `Monthly`) and, dim, the account when the answer names one. Its rows
are indented under the name, so each section reads as one block.

One row per **meter**, labels in an aligned column across the page:

| Meter | Drawn as |
| --- | --- |
| a limit | a bar of the share **used**, the share (`42% used`), `limit reached` in red when the provider says it is spent, the counts behind it with their unit when the provider counts (`420 of 1,000 credits`), and when it resets |
| unlimited | `Unlimited`, in the success green |
| a balance | its value bright and a dim detail after it (`62,500 credits · ≈ 15.6k–81.3k local messages`) |

Every limit comes before every balance — the plan's windows, then any
per-model and code-review limits, then the credit balance and the limit
resets the account can redeem — so the bars form one column.

A bar fills as the limit is spent — green, amber from 75%, red from 90% (the
`/context` meter's thresholds), and red whatever the share once the provider
says the limit is reached — and the share beside it takes the bar's colour
once it warns. A share is a whole percent that never claims all or none it
isn't: `<1%` for a sliver, `>99%` short of the whole, `100%` only when the
provider says so (Copilot's and ClinePass's answers can run past it; the bar
stops at full). The bar keeps the same promise: any use at all fills at least
one cell, and a share short of the whole leaves at least one empty.

**One bar length for the whole page**, so the column of meters lines up —
set by the terminal's width alone, never by the text beside a bar: 20 cells
(the segments codex's `/status` draws) wherever a row has room for that and
a countdown beside it, shorter only on a terminal too narrow for both, and
never below 8. The bar is the one thing on a row that keeps its size, the
text beside it giving way instead: at 20 cells a bar moves in 5% steps, so
81% and 86% look different; a long row costs only itself (Copilot's, above,
gave up its reset's clock time and no other row lost a cell); and the bars
hold their length while the countdowns tick. Sizing every bar to the widest
row's text was the first version's rule, and it drew 81% and 86% the same at
100 columns.

**Used, not left.** Codex's `/status` bars show what is *left*; GitHub's,
Cline's and Claude Code's own usage pages show what is *used*. A page called
*Usage limits* reads one way, and a bar that fills as the week goes is the
one that needs no legend.

**When it resets** is both relative and absolute: `resets in 2h 14m` says how
long to wait, `· Tue 2:36 PM` (local time — `3:58 PM` today, a weekday within
the week, `Nov 12, 1:26 PM` past it) says when to come back. A narrow row
gives up the clock time first; then the counts move to a row of their own
under the bar, at the bar's column; and with no room beside the bar at all,
the whole trailer moves under it. Whether a limit is spent, how much of it,
and how long until it resets are never lost to a narrow terminal. A window
that names no reset (ClinePass's 5-hour window before it starts) shows none.

A per-model limit (ChatGPT's `additional_rate_limits`, its code-review limit)
sits under a dim sub-heading naming the model family, after the plan's own
rows — codex's order — so a long model name never has to be cut to fit the
label column.

**Notes** follow the meters: what the meters can't say. Red for something
stopping requests that no row explains (a workspace's spend cap, a
`limit_reached` with no spent window, a Copilot organisation budget used up),
amber for something to act on (ClinePass's *Paused until the weekly limit
resets — switch with /model meanwhile.*, a Cline account with no ClinePass
plan), dim for a fact (usage billed past a Copilot allowance; the Console
sign-in's per-token billing). A spent window says `limit reached` on its own
row, so it needs no second red sentence.

### One link

Over the hint, **the highlighted subscription's** usage page, as a clickable
link (`docs/links.md`): `Details at https://chatgpt.com/settings/usage`. One
link rather than one per section, because it follows the highlight — the row
the user moved to is the page they want — and the rows it would otherwise
spend are the rows a short terminal needs most. `c` copies it, through
`/copy`'s clipboard path (`Copied the usage link to clipboard`), the page
staying open; the hint names `c` only when there is a link to copy.

### States

| What | The section shows |
| --- | --- |
| first read in flight | `Loading usage…`, dim |
| read failed, nothing shown yet | the error in red, `Couldn't load usage: …` |
| a refresh failed | the last report, and an amber `Couldn't refresh: …` under it |
| a refresh in flight | the last report; the subtitle says `refreshing…` |

A page left with no subscription at all — every one signed out while it was
open — says so and points at `/login` → *Use a subscription*, in the accent
`/model` uses for the same advice.

## Keys

| Key | Does |
| --- | --- |
| ↑ / ↓ | move between subscriptions (wrapping) |
| Home / End | the first / last subscription |
| `r` | re-read every subscription now (ignored while a read is in flight) |
| `c` | copy the highlighted subscription's usage page link |
| Ctrl+O, Ctrl+D | the transcript and the context view over the page, as `/context` lets them; the page is still there on the way back |
| Esc, Ctrl+C | close the page |

Every other key is swallowed while the page is open — the `/donate` rule —
and an overlay opened from the page never arms the Esc-Esc rewind
(`App::overlay_esc_backtracks`). The highlight matters on a short terminal
(below), where it decides which section stays open.

## A terminal too short

The family's rule (`docs/view-flow.md` *A page that sheds rows before it
flows*), the first page that fits:

1. the whole page;
2. without the link;
3. an **accordion**: the highlighted section open, every other one folded to
   its head row with a summary of its fullest limit
   (`GitHub Copilot  Free · Chat 86% used`) — `/donate`'s accordion;
4. that page with the title and subtitle folded into **one** row
   (`Usage limits · updated just now`) — a page never loses its name;
5. the compact page: that title row, then one folded row per subscription,
   windowed around the highlight between the `↑`/`↓` slots, then the hint —
   the family's compact ladder (`model_view::compact_page`), shedding down to
   the title row alone, so the page never flows.

## Live

The page is fetched when it opens, and re-read **every minute** while it is
open and not already reading — the cadence codex polls the same ChatGPT
endpoint at — or at once with `r`. A refresh keeps every report on screen
until its replacement lands, and lays the page over the subscriptions signed
in now: one signed in since is added, `Loading usage…`; one signed out is
dropped. Closing the page cancels whatever is in flight; a result for a closed
page, or for a round a newer one replaced, is dropped
(`App::finish_usage_fetch` keys every result on its round).

**A closed page keeps its last reports** (`App::usage_kept`): `/usage` again
shows them at once, `refreshing…`, instead of opening on a column of
`Loading usage…` rows for the second it takes to read them again. The
highlight reopens on the first section, the one in use.

The relative times tick: the boundary injects the wall clock into the open
page every draw (`App::set_usage_now`), and keeps a slow frame (every 5 s)
scheduled while the page is on screen — never under an overlay, whose return
re-seeds it — so a countdown and the freshness move with no other event. The
absolute times are local: the boundary hands the app, once, a function giving
the local UTC offset at an instant (`App::set_local_offset`), so a reset
across a daylight-saving change still reads the right hour — the
`App::set_clock` pattern, and the page itself never reads a clock or a time
zone.

## Where each number comes from

Every read runs on a worker thread of its own (`tui::workers::spawn_usage_fetch`
→ `llm::usage::fetch_usage`), one per subscription, so a slow provider never
holds the others back. Each one reuses the credential seam its chat requests
use (`llm::auth::request_auth`): the stored token minted and cached the same
way, a rotated refresh token written back to the `.env` store the same way
(`docs/chatgpt.md`, `docs/cline.md`). Each section's link is the provider's
own usage page (`llm::usage::dashboard_url`, by sign-in scheme).

### ChatGPT

`GET https://chatgpt.com/backend-api/wham/usage` — Codex's own read
(`backend-client`), on the backend's root rather than under the chat base's
`/codex` (`llm::usage::chatgpt_usage_url`; a base that is not the ChatGPT
backend is asked at `/api/codex/usage`, codex's other path style). The
request carries the minted bearer, `chatgpt-account-id`, Codex's user agent
and `originator` — the chat request's own identity. The answer names the plan,
the email, a `rate_limit` with up to two windows (`used_percent`,
`limit_window_seconds`, `reset_at`), `additional_rate_limits` per model
family, a `code_review_rate_limit`, the `credits` balance (a string —
`"62500.0000000000"` — with an `approx_local_messages` range), a workspace's
`spend_control` (its monthly credit limit, a meter of its own), the
`rate_limit_reached_type` when requests are stopped, and
`rate_limit_reset_credits.available_count`, the limit resets the account can
redeem. A window is named by its length within 5% (`5-hour`, `Daily`,
`Weekly`, `Monthly`, `Annual` — codex's tolerance), else by the length itself
(`2-hour limit`), and is reached at 100% used. A workspace's reason for
stopping is worded for who can lift it (codex's sentences); a bare
`limit_reached` is worded only when no window on the page already says so.
Verified live on a Pro seat.

### GitHub Copilot

`GET https://api.github.com/copilot_internal/user` — what VS Code's own
Copilot dashboard reads — with the stored GitHub token itself
(`Authorization: token ghu_…`), **not** the Copilot bearer the chat request
exchanges it for, plus `X-GitHub-Api-Version: 2025-04-01` and the editor
identity the exchange sends (`copilot::user_headers`).

The plan comes off `access_type_sku` first: `free_limited_copilot` is
**Free**, whatever `copilot_plan` says — a Free seat reports `individual`
there, Pro's own slug, and only the SKU tells them apart (VS Code's rule).
Otherwise `copilot_plan` names it (`individual` → Pro, `individual_pro` →
Pro+, `individual_max` → Max, `business` → Business, …).

`quota_snapshots` holds one bucket per feature: `premium_interactions`
(**AI credits** on a seat billed in GitHub's AI Credits —
`token_based_billing` — else **Premium requests**), `chat` (**Chat**) and
`completions` (**Code completions**), counted in credits on a credit-billed
seat, else in requests, messages and completions. A bucket's share is its own
`percent_remaining`, as VS Code reads it; the counts are the `entitlement`
less what remains (`quota_remaining`, else `remaining`); every number may
arrive as a number, a numeric string or `null`. A bucket is **reached** when
nothing is left — its share, its remainder or `has_quota: false` say so —
unless the seat may run past it (`overage_permitted`), and usage billed past
the allowance is a dim note (`12 credits billed past the allowance.`). An
`unlimited` bucket says so, unless `has_quota: false` marks the organisation's
budget spent — a red note, once. A zero allowance that isn't unlimited is a
placeholder (a pooled Business seat) and is left out. Everything resets at
midnight UTC on the 1st (`quota_reset_at`, else `quota_reset_date_utc`, else
`quota_reset_date`). A seat on the old Free plan reports `limited_user_quotas`
against `monthly_quotas` instead. `ALTER_ZERO_GITHUB_API_BASE` points this read
— and the token exchange — at another host (`copilot::set_github_api`, read at
the boundary), which is how the smoke suite stands in for GitHub.

### ClinePass

Two reads, the Cline dashboard's own order: `GET /api/v1/users/me/plan` (the
plan, named by its billing `interval` — `Monthly` — else its `displayName`;
`402`, `404` or a `null` plan mean the account has no ClinePass subscription,
and the section says so instead of reading windows), then `GET
/api/v1/users/me/plan/usage-limits` — `limits[]` of `{type, percentUsed,
resetsAt?}` for the `five_hour`, `weekly` and `monthly` windows. Both carry
the minted `Bearer workos:<jwt>` and go to the API host in force
(`ALTER_ZERO_CLINE_API_BASE`, `docs/cline.md`). A spent window is reached on
its own row, and one amber line says what to do about it, naming the window
that resets **last** — how long the pause lasts — the dashboard's banner.
Verified live.

### Console sign-in

A Console sign-in bills per token to the account's API organisation — there
is no plan and no window (`docs/claude.md` says why this is deliberately not a
subscription sign-in). The section asks **nobody**: it says `Pay as you go`,
that the organisation is billed per token with no plan limits, and links the
Console's usage page. Minting a token only to name the organisation would
rotate the sign-in's refresh token for a fact the page doesn't need.

### Failures

A refused read is worded for what to do (`llm::usage::usage_advice`): a `401`
asks for `/login`, a `429` to wait a minute, a server error to try again
shortly — with the provider's own words after it when they are showable (a
JSON `message`, never an intercepting proxy's HTML page). A sign-in that no
longer mints fails with the sign-in module's own sentence.

## The seams

| Piece | Where | Kind |
| --- | --- | --- |
| the report model, every parse, the advice, the links | `src/llm/usage.rs` | pure, unit-tested |
| the reads | `llm::usage::fetch_usage` | boundary (HTTP), stub-tested |
| the page state, keys, rounds, the kept reports | `src/app/usage.rs` | pure, unit-tested |
| the page | `src/ui/usage_view.rs` | pure, unit-tested |
| workers, the clock, the minute refresh, the clipboard | `src/tui/usage.rs` | boundary, smoke Phase 145 |

Every parse reads the provider's answer leniently: each section of it is kept
as the raw slice it arrived as and read on its own, so a section this parse no
longer understands costs only itself — these are undocumented endpoints, and
they change without notice. Nothing is parsed into a `serde_json::Value` tree
(`docs/memory.md`).

## Tests

- `llm::usage::tests` — each provider's answer (the live shapes, with the
  identifiers replaced), the window labels, reached limits and the notes that
  explain them, the Copilot SKUs and units, the leniency, the URLs and links,
  the advice, and the reads against a loopback stub.
- `app::tests::usage` — the command, the empty toast, the keys, rounds and
  stale results, the reports kept across opens, `c`, the overlays.
- `ui::tests::usage_view` — the sections, the meters, `limit reached`, the
  reset wording and what a narrow row gives up, the one link, the freshness,
  the frame hugging its content at every width and height, the ladder.
- `tests/live_usage.rs` — every provider against its real endpoint,
  `#[ignore]`d (it needs credentials; the rotating ones write back to
  `ALTER_ZERO_LIVE_TOKEN_STORE`).
- `scripts/smoke/phases/145-usage.sh` — the page against stub endpoints for
  ChatGPT, Copilot (a Free seat on credit billing) and ClinePass: the
  sections, `limit reached` in red, `r`, a refused refresh, ↓ moving the
  link, `c`, Ctrl+O over the page, a reopen showing the last numbers, a short
  terminal, the page mid-turn, the toast with nothing signed in.
