# `/usage` — what your subscriptions have left

Date: 2026-10-25

## Goal

A **`/usage`** slash command that answers the question every subscription
sign-in leaves open: *how much of my plan have I used, and when does it come
back?* A pasted API key is billed per token and has a dashboard of its own;
a subscription is a set of **limits** — a 5-hour window, a week, a month, a
monthly allowance of premium requests — and the first a user hears of one is
the refusal that says it ran out. `/usage` shows every signed-in
subscription's limits on one page: how much of each is used, when it
resets, and what else the plan holds (a credit balance, the allowances it
does not count).

It covers the four sign-ins `/login` offers under **Use a subscription**
(`AuthScheme::is_subscription`), each read from the provider's own usage
endpoint — the one its own client reads:

| sign-in | endpoint | what it says |
| --- | --- | --- |
| ChatGPT Codex | `GET {backend-api}/wham/usage` — Codex's `/status` source | the plan; a primary and a secondary window (5-hour, weekly, …) as a used percentage and a reset time; per-model and code-review limits; a workspace spend cap; the credit balance |
| GitHub Copilot | `GET https://api.github.com/copilot_internal/user` — VS Code's quota source | the plan; each quota bucket (premium requests or AI credits, chat, completions) as an entitlement and what remains; the monthly reset |
| ClinePass | `GET {api}/api/v1/users/me/plan/usage-limits` — Cline's dashboard's source — then `/api/v1/users/me/plan` | the 5-hour, weekly and monthly windows as a used percentage and a reset time; the plan's name |
| Anthropic Console | *(none)* | nothing to meter: the sign-in bills per token to the account's API organisation (`docs/claude.md`) |

The Anthropic row is deliberate. Claude.ai's plan windows (`/api/oauth/usage`)
describe a Claude Pro/Max subscription, which a Console sign-in is not and
must not pretend to be: its tokens are minted for an API organisation, so
the windows do not apply to anything it does — Claude Code's own schema says
so of every API-key session — and reading them would be the subscription
credential use `docs/claude.md` rules out. So the row says what is true
(*billed per token, no plan limits*) and links the Console's usage page,
with no request at all.

## The page

```
────────────────────────────────────────────────────────────────────────────────────────────────────
  Usage limits
  4 subscriptions · updated just now

    Anthropic Console  Pay as you go
      Billed per token to your API organisation — no plan limits

  ❯ ChatGPT Codex  Pro                                                                      in use
      Weekly limit      ███░░░░░░░░░░░░░░░░░░   12% used  resets in 6d 18h · Sun 8:00 AM
      Credits           62,500 credits · ≈15.6k–81.3k local messages

    ClinePass  Monthly
      5-hour limit      ░░░░░░░░░░░░░░░░░░░░░    0% used
      Weekly limit      █████████████████████  100% used  limit reached · resets in 2d · Tue 2:00 PM
      Monthly limit     █████████████████░░░░   81% used  resets in 18d · Nov 12

    GitHub Copilot  Pro
      Premium requests  ██████████░░░░░░░░░░░   49% used  resets in 18d · Nov 12
                        148 of 300 requests
      Chat              Included
      Code completions  Included

  Full usage: https://chatgpt.com/settings/usage
  ↑↓ select  r refresh  c copy link  esc close
────────────────────────────────────────────────────────────────────────────────────────────────────
```

A composer-replacing inline page — the `/context` and `/donate` pages'
family (`docs/context-usage.md`, `docs/donate.md`): the same rules, the same
two-column inset, the pages' title dress, the `❯` on the highlighted
section, no text entry and so no hardware cursor (its seat tracks the `❯`),
every key owned while it is open, the composer back on Esc. It works
**mid-turn**, the streaming strip keeping its rows above it as it does above
every page — the moment a limit matters is often the moment a turn is
running into it.

Why a page and not a report in the transcript: the numbers move (a turn
spends them, a window resets), a report would be stale the moment it
committed, and a page leaves nothing in the conversation. Codex shows the
same numbers inside its `/status` card; its own `/usage` is a menu of
analytics and resets. Claude Code's `/usage` is a settings tab. A page of its
own fits this app's family, opens in one keystroke, and holds every
subscription at once.

### Sections

One section per signed-in subscription, in the order `/login` lists them,
the one the session runs on marked `in use` at the right edge — only while a
real model runs on it: the offline dummy still names a provider, but runs on
none. A section's header is the provider's name in bold over its **plan** in
the dim ink — `Pro`, `Pro+`, `Monthly`, `Pay as you go` — read from the
provider's own answer (ChatGPT's `plan_type` through `chatgpt::plan_label`,
Copilot's `access_type_sku`/`copilot_plan`, ClinePass's plan `interval`).

Under it, one row per **window**, in columns every section shares: the
label, the bar, the share used, and when it resets.

- **The label** names the window by its length — Codex's rule, the length
  read off the window rather than assumed: within 5% of five hours is the
  `5-hour limit`, of a day the `Daily limit`, of a week the `Weekly limit`,
  of a year the `Annual limit`, any calendar month's length the `Monthly
  limit`; any other length names itself (`30-minute limit`, `3-day limit`).
  A per-model limit carries its model first (`codex_other · 30-minute
  limit`), Codex's per-bucket rows. A Copilot bucket is named for what it
  counts — `Premium requests` (`AI credits` on credit billing), `Chat`,
  `Code completions`.
- **The bar** fills with what is **used** — the page is about usage, and an
  emptying bar would read as the opposite of its percentage. Its cells are
  the accent while the window has room, the caution amber from 75% used and
  the error red from 90%, the thresholds Copilot's own status menu warns at
  and the `/context` subtitle already uses. A share above nothing keeps at
  least one cell, and a share short of the whole never fills the bar, so a
  window at 99.6% is never drawn spent. It is as long as the row leaves it
  (8 to 32 cells), and every bar on the page starts in one column.
- **The share** reads `27% used`; a sliver reads `<1%` rather than a `0%`
  that would say nothing is used, and a share short of whole reads `99%`,
  never `100%`. It takes the bar's amber and red too.
- **The reset** reads relative first — `resets in 3h 12m`, `in 6d 18h`, `in
  18d` — because that is the question, then the local wall-clock moment
  after a dim `·`: the time within a day (`6:58 PM`), the weekday and time
  within a week (`Sat 6:58 PM`), the date beyond (`Nov 12`). A window whose
  provider names no reset (an unused rolling window) says nothing, rather
  than guess. A window that is used up says `limit reached` in red first.
  Where the row has no room for both parts, every row drops the moment; where
  it has room for neither, the clause sits on its own row under the bar.

Under the windows, the plan's **notes**, in the same columns: what it holds
that is not a window. ChatGPT's credit balance (`62,500 credits`,
`Unlimited`), with the message counts it buys when the backend says
(`≈15.6k–81.3k local messages`, under the value when the row cannot hold
it); Copilot's buckets the plan does not count (`Included`), with the
credits an unlimited bucket has used when it reports any. A window's own
numbers sit under its bar when the provider gives them — `148 of 300
requests`, `8,000 of 25,000 credits used` for a workspace's spend cap.

A section still loading says `Loading…`; one whose fetch failed says why in
red, wrapped, never cut — a sign-in the provider refused names `/login`, a
host that cannot be reached names it — and keeps whatever it last showed
above the error when a refresh failed after a success. A refusal the
provider states that no window explains (a workspace out of credits, an
organization's Copilot budget spent) is a red sentence of its own.

### The header and the link

The subtitle counts the subscriptions and says how fresh the page is:
`updated just now`, `updated 3m ago`, or `refreshing…` while a fetch is out.
Under the sections, the **highlighted** section's own usage page is linked
— `Full usage: https://…` as an OSC 8 hyperlink (`docs/links.md`) — the
provider's dashboard, which has the history and the controls this page does
not: `chatgpt.com/settings/usage` (Codex's own link),
`github.com/settings/copilot`, ClinePass's
`app.cline.bot/dashboard/subscription`, and the Console's
`platform.claude.com/usage` (`usage_limits::dashboard_url`). `c` copies it.

## Live

The page fetches every signed-in subscription **on open**, in parallel, one
worker thread each (`tui::workers::spawn_limits_fetch`, the `/model` listing's
pattern), and paints each section as its answer lands. `r` fetches them all
again, and the page refreshes itself **once a minute** while it stays open —
Codex's own polling cadence for a signed-in account — so a page left open
beside a long turn watches the window fill. What was fetched is kept for the
session (`App::subscription_limits`): reopening shows the last answer at
once, marked `refreshing…` while the new one is out, so the page never opens
blank twice; the store follows the sign-ins — a subscription signed out
meanwhile is dropped, one signed in meanwhile starts empty.

Every refresh has a **generation** (`App::begin_usage_limits_refresh`), which
each worker echoes back: an answer to an older refresh is dropped
(`App::set_plan_limits`), and closing the page bumps it too, so the fetches
the close cancels can report nothing late. A refresh is never doubled — `r`
does nothing while one is out, and the minute tick waits for it
(`App::usage_limits_refresh_due`).

The page's clock is the boundary's: every draw while it is open injects the
wall clock (`App::set_usage_limits_now`), and the local UTC offset at a given
instant comes from an injected function (`App::set_local_offset` —
`chrono::Local` at the boundary, UTC in a test), the `App::set_clock`
pattern — so the pure page formats `resets in 3h 12m · 6:58 PM` with no clock
of its own, and a reset across a DST change still lands on its own hour. Its
countdowns move by the minute, so it keeps its own frame chain — one a
second, the `/login` device page's, never under an overlay — rather than the
30 fps status chain. It signs its view flow on its rows, and never flows:
every terminal gets a page that fits (below).

## Keys

| key            | does                                                       |
| -------------- | ---------------------------------------------------------- |
| ↑ / ↓          | move the highlight between sections, wrapping at the ends  |
| Home / End     | the first / last section                                   |
| r              | fetch every subscription's usage again                     |
| c              | copy the highlighted section's usage link                  |
| Ctrl+O / Ctrl+D| the transcript / the raw context — the page stays open behind it |
| Esc / Ctrl+C   | close — the composer returns                               |

Everything else is swallowed, pastes included. An overlay opened from the
page never arms the Esc-Esc rewind (`App::overlay_esc_backtracks`), the
`/context` page's rule. With a lone subscription the hint drops `↑↓ select`.

With no subscription signed in there is nothing to show, so `/usage` does
not open a page: a toast says `No subscription signed in — use /login → Use
a subscription`, the empty rule `/export` and `/compact` follow.

## A terminal too short for the page

The page builds the first of its pages that fits the rows it may have
(`model_view::first_page_that_fits`, `docs/view-flow.md` *A page that sheds
rows before it flows*), shedding in this order: the windows' counts, the
link, the **other** sections' bodies — an accordion, the highlighted section
keeping its windows while every other folds to its header with its tightest
window's share beside it (`ClinePass  Monthly · Weekly limit 100%`) — then
the subtitle, the title folding into one row with the freshness (`Usage
limits · updated just now`): a page never loses its name. Past those the
sections are a compact list — one folded row each, three riding the
highlight between the reserved `↑`/`↓` slots, under that title row and over
the hint, the compact pages' ladder (`model_view::compact_page`) shedding on
down to the title row alone — so no terminal is too short, and nothing flows
into scrollback.

```
────────────────────────────────────────────────────────────────────────────────
  Usage limits · updated just now
  ↑ 1 more above
    ClinePass  Monthly · Weekly limit 100%
  ❯ GitHub Copilot  Pro · Premium requests 49%
    Anthropic Console  Pay as you go

  ↑↓ select  r refresh  c copy link  esc close
────────────────────────────────────────────────────────────────────────────────
```

## Fetching

`llm::usage_limits::fetch_plan_limits(cfg, cancel)` is the one entry point,
a library `pub fn` beside `llm::models::fetch_models` (the shared HTTP client
is crate-private). It dispatches on the provider's **auth scheme**, never its
id — a user's `providers.toml` can name a row anything — and authenticates
through the seam every request already uses:

- **ChatGPT Codex** — `auth::request_auth` mints (or reuses) the access token
  and adds `chatgpt-account-id`, the user agent and the FedRAMP flag; the URL
  is the provider's base cut after `/backend-api` with `/wham/usage`
  appended, Codex's path-style rule (`/api/codex/usage` for a base without
  `/backend-api`, `usage_limits::chatgpt_usage_url`). Nothing is minted per
  page open: the bearer cache that serves every turn serves this, and a
  rotated refresh token is written back exactly as a turn's is.
- **GitHub Copilot** — the stored GitHub OAuth token itself, as `Authorization:
  token …` with the editor identity headers and `X-GitHub-Api-Version:
  2025-04-01`, the set VS Code sends; no Copilot bearer is exchanged for it.
  `ALTER_ZERO_GITHUB_API_BASE` (read at the boundary, handed in once through
  `copilot::set_github_api_base`) moves both GitHub calls — this one and the
  token exchange — for the smoke stub.
- **ClinePass** — the `workos:` bearer from `auth::request_auth`, on the API
  host in force (`ALTER_ZERO_CLINE_API_BASE`); the plan is fetched second
  and only names the plan — its failure never fails the section.
- **Anthropic Console** — no request; `usage_limits::pay_as_you_go`.

Every body is small and fixed-shape, so each is read capped
(`USAGE_BODY_MAX_BYTES`, 64 KiB) and deserialized straight into typed
structs — no `Value` tree (`docs/memory.md`). A refusal becomes a sentence
the page can show (`usage_limits::refusal`): `401` says the provider refused
the stored sign-in and names `/login`; ClinePass's `404` says the account has
no active plan and its `402` that an unpaid invoice has paused it; `429`
says to try again shortly; anything else quotes a short body, never an HTML
page. Failures before the request — a refresh the provider refused, a host
that cannot be reached — are worded by the auth seam and
`usage_limits::error_text` (`No connection to chatgpt.com`).

## Parsing

Each provider's answer becomes one `PlanLimits` — the plan label, the
windows (`LimitWindow`: label, used percent, reset as Unix seconds, an
optional detail, reached), the notes (`LimitNote`), an alert and a remark —
and a parse never fails on a field it does not know or a value it cannot
read: every number, string and moment goes through a lenient reader (a
count sent as a string still counts, a share sent as `null` is skipped), so
a backend that adds a window tomorrow costs nothing and one that drops a
field shows what remains. Only a body that is not the expected object at
all is an error.

- **ChatGPT** — `plan_type`; `rate_limit.primary_window` then
  `secondary_window` (`used_percent`, `limit_window_seconds`, `reset_at`), a
  window at 100% marked reached and a `limit_reached` no window explains an
  alert; `additional_rate_limits[]` and `code_review_rate_limit` as named
  windows; `spend_control.individual_limit` as a `Monthly credit limit`
  window whose share is `100 − remaining_percent` and whose detail is
  `{used} of {limit} credits used`; `credits` as a note (`Unlimited`,
  `{balance} credits`, or `Available`), with `approx_local_messages` as its
  detail; `rate_limit_reached_type` as the alert it names (a workspace out
  of credits, its usage limit reached).
- **Copilot** — the plan from `access_type_sku` (Free, Student) else
  `copilot_plan` (`individual` → Pro, `individual_pro` → Pro+,
  `individual_max` → Max, `business`, `enterprise`, anything else
  title-cased); each of `premium_interactions` (or `premium_models`),
  `chat`, `completions`: a bucket outside the plan (`!unlimited &&
  entitlement == 0`) is left out, an unlimited one is an `Included` note (an
  unlimited one with no quota left the organization's budget alert), a
  metered one a window whose share is `100 − percent_remaining` and whose
  detail is `{used} of {entitlement} {unit}` — credits on token billing,
  else requests, messages or completions — usage past the allowance saying
  `over by …` and paid overage `… more billed` (which keeps a spent bucket
  unreached). The reset is the bucket's `quota_reset_at`, else
  `quota_reset_date_utc`, else `quota_reset_date` at 00:00 UTC; the legacy
  free plan's `limited_user_quotas`/`monthly_quotas` become the same two
  windows.
- **ClinePass** — the `{success, data: {limits: [{type, percentUsed,
  resetsAt}]}}` envelope: `five_hour`, `weekly` and `monthly` named as above,
  any other type named from itself, a missing share read as unused, a reset
  that is not RFC 3339 dropped; `success: false` is the envelope's own
  error, and an empty list says `No limits reported`.

## API

- `llm::usage_limits` — `PlanLimits`, `LimitWindow`, `LimitNote`,
  `window_label`, `parse_chatgpt_usage`, `parse_copilot_usage`,
  `parse_clinepass_usage`, `pay_as_you_go`, `dashboard_url`,
  `chatgpt_usage_url`, `refusal`, `error_text`, `fetch_plan_limits`,
  `USAGE_BODY_MAX_BYTES`. Beside it: `copilot::user_info_url`,
  `set_github_api_base`, `github_api_base`, `GITHUB_API_ENV`;
  `cline::usage_limits_url`, `plan_url`.
- `app` — `UsageLimitsPage`, `SubscriptionLimits`, `SubscriptionEntry`,
  `USAGE_LIMITS_EMPTY_NOTICE`, `USAGE_LIMITS_REFRESH_SECS`;
  `App::open_usage_limits`, `close_usage_limits`, `subscription_limits`,
  `begin_usage_limits_refresh`, `set_plan_limits`, `usage_limits_refreshing`,
  `usage_limits_refresh_due`, `set_usage_limits_now`, `usage_limits_now`,
  `set_local_offset`, `local_offset`, `highlighted_subscription_limits`;
  `CommandEffect::Usage`; `Action::OpenUsageLimits`, `CloseUsageLimits`,
  `RefreshUsageLimits`, `CopyUsageLink`.
- `ui::usage_limits_view` — `usage_limits_lines` (the page builder; its
  length is the reserved height), `usage_limits_height`,
  `render_usage_limits`.
- `tui::usage_limits` — the open, the refresh, each result, the minute tick
  and the copy at the boundary; `tui::workers::spawn_limits_fetch`;
  `tui::host::local_offset`.

## Tests

- `llm::usage_limits` — every provider's answer from real bodies: the
  ChatGPT and ClinePass answers read live on 2026-10-25, Codex's own
  fixture, and Copilot's from the captures VS Code-compatible clients
  publish (a Pro seat, Free on credit billing, Max, a Business seat past its
  allowance, paid overage, the legacy free plan, an organization's spent
  budget); the window names, reached limits, details and notes, unknown
  fields and types, malformed values skipped, refusals and failures worded;
  the URL rules (`llm::copilot` pins the GitHub API base both calls follow).
- `app/tests/usage_limits.rs` — the palette row, the command handing the
  open to the boundary idle and mid-turn, the store reconciled with the
  sign-ins and kept across opens, answers applied per subscription, a
  failure keeping the last answer, stale and unlisted answers dropped, the
  close dropping what it was owed, the minute cadence, the injected clock
  and offset, the key grammar (wrapping, Home/End, `r` never doubled, `c`,
  Esc/Ctrl+C, the overlay pass-through, owns-every-key), an overlay over the
  page never arming a rewind, the Ctrl+B hint masked.
- `ui/tests/usage_limits_view.rs` — the words (shares, countdowns, local
  moments, ages, the bar's cells), the framed page and its hugging frame at
  every width and height, aligned bars and values, the bar's and share's
  colours, a used-up window, counts under the bar, notes and their wrapped
  detail, loading, failed and pay-as-you-go sections, the link with its OSC 8
  target, the hint, the narrow-terminal reset fallbacks, the accordion, the
  compact page, a title on every page of the ladder, no flow and no
  overwide row at any size, the hidden cursor on the `❯`, the reserved
  height, the painted page.
- `scripts/smoke.sh` Phase 145 — the page end to end against a local stub
  answering ChatGPT's, Copilot's and Cline's wire shapes: the four sections,
  each request's credential off the stub's log, ChatGPT's rotated refresh
  token written back, a refused Copilot read worded and recovered by `r`,
  `c` copying the link, the page mid-turn under the status line and on 9
  rows, and the toast with nothing signed in.
