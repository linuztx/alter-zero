# `/usage` — what is left of each subscription

Date: 2026-10-26

## Goal

A **`/usage`** slash command that answers the question a subscription
raises and a pasted key never does: *how much of my plan is left, and when
does it come back?* ChatGPT meters a Pro or Plus seat against a 5-hour and a
weekly window; GitHub Copilot counts premium requests (AI Credits since June
2026) against a monthly allowance; ClinePass caps a rolling 5-hour, a weekly
and a monthly window. Each of those runs out mid-task with a refusal, and
until now the first sign of a limit was the refusal.

The page lists every **subscription this machine is signed in to** — the
`/login` → *Use a subscription* list, the providers whose `auth` is a
sign-in (`AuthScheme::is_subscription`) — with each limit's share used, its
reset, and what the account holds besides (credits). A provider reached with
a pasted key or a local server has no plan to show and is not listed.

## Why the server's numbers

The obvious way to "count usage" — tally the tokens this app sends — cannot
answer the question, so the page never tries. A plan's meter is the
provider's own and is not a token count:

- **ChatGPT** meters a share of each window, weighted by model and by the
  work the backend does, and spends credits past it. The share is what codex
  shows, read from the same endpoint.
- **Copilot** charges a premium request per *user-initiated* turn times the
  model's multiplier (the `X-Initiator` header is billing-relevant for this
  reason, `docs/copilot.md`), and since June 2026 meters AI Credits.
- **ClinePass** caps each window in dollars of inference cost at the plan's
  own prices.

And all three windows are **per account**, shared with every other client
signed in to it: the ChatGPT app, VS Code, the Cline extension. A client-side
count would miss everything they spent. So each row is the provider's own
answer, fetched when the page opens, from the endpoint the provider's own
clients read — the source table below — and the tally the footer and
`/context` show stays what it is: this session's tokens.

## The sources

| provider | request | what it reads |
| --- | --- | --- |
| ChatGPT Codex | `GET {backend-api}/wham/usage`, the access token as a bearer plus `chatgpt-account-id` | `plan_type`, `email`, the main bucket's `primary_window`/`secondary_window` (`used_percent`, `limit_window_seconds`, `reset_at`), each `additional_rate_limits` bucket, `credits` (`has_credits`, `unlimited`, `balance`, `approx_local_messages`), `spend_control`, `rate_limit_reached_type` |
| GitHub Copilot | `GET https://api.github.com/copilot_internal/user`, the stored GitHub OAuth token as `Authorization: token …` | `access_type_sku`/`copilot_plan`, `quota_snapshots` (`premium_interactions`, `chat`, `completions`: `percent_remaining`, `entitlement`, `remaining`, `unlimited`, `overage_count`, `quota_reset_at`), `quota_reset_date_utc`/`quota_reset_date`, the Free plan's `limited_user_quotas`/`monthly_quotas` |
| ClinePass | `GET {api}/users/me/plan/usage-limits` and `GET {api}/users/me/plan`, the minted `workos:` bearer | each window's `type` (`five_hour`, `weekly`, `monthly`), `percentUsed`, `resetsAt`; the plan's `displayName` and its per-window dollar caps (`inferenceCapThreshold`, in 1e-8 USD) |
| Anthropic Console | none | — |

**ChatGPT's endpoint is a sibling of the Codex base, not under it**:
`https://chatgpt.com/backend-api/codex` is where turns go, and
`https://chatgpt.com/backend-api/wham/usage` is where codex's `/status` reads
the limits (`PathStyle::ChatGptApi`; a base with no `/backend-api` is the
Codex API's `/api/codex/usage`). The window's label comes from its length —
`5-hour`, `daily`, `weekly`, `monthly`, `annual` within 5%, codex's
`is_approximate_window`, and any other length said as one (`2-hour`,
`30-minute`) rather than codex's generic "usage". A named bucket (a model with
an allowance of its own, `GPT-5.3-Codex-Spark`) is a sub-heading over its own
windows. Credits follow codex's rule: no row without credits, `Unlimited`, a
whole count, or `Available` when there is no count to give — and a count
carries the backend's own estimate of what it buys (`≈15,625–81,250
messages`). A workspace spend cap is the **Monthly credit limit** meter, and
`rate_limit_reached_type` becomes codex's own sentence for why requests are
refused (a workspace out of credits, a spend cap hit), folded to one row.

**Copilot's endpoint takes the long-lived GitHub token, not the exchanged
bearer** — it is GitHub's API, not Copilot's — so no exchange is spent on it.
The rules are VS Code's own (`chatEntitlementService`): the share used is
`100 − percent_remaining`; a snapshot that is not unlimited and entitles
nothing (the Free plan's premium requests) is skipped; the reset is the
snapshot's `quota_reset_at`, else the account's `quota_reset_date_utc`, else
its `quota_reset_date` (a date with no time, shown as one); and the plan is
named from `access_type_sku` first, because a Free account reports
`copilot_plan: "individual"`. A `token_based_billing` account's premium row is
**AI Credits** rather than **Premium requests**. Copilot's newer session and
weekly limits travel only in chat-response headers and are not in this
answer, so the page does not show them.

**ClinePass answers 404 on an account without the plan** — a session that is
signed in but not subscribed — which the page shows as a sentence under the
row rather than an error. The plan request is best-effort: without it the
windows still show, only the plan's name and the caps go.

**Anthropic Console has no row to fetch.** It is deliberately *not* the
Claude Pro/Max sign-in (`docs/claude.md`): usage bills per token to the
account's API organisation, whose limits are per-minute rate limits and a
monthly spend cap set in the Console, readable only with an organisation
admin's key. The endpoint that reports Claude subscription windows is the
Claude.ai sign-in's, which this app does not offer and does not call. So the
row says what the sign-in is — billed per token, no plan limits — and links
the Console's usage page, with no request at all.

## The page

```
────────────────────────────────────────────────────────────────────────────────
  Subscription usage
  3 subscriptions · as of 2:25 AM

  ❯ ChatGPT Codex  Pro · dev@example.com                                  in use
      Weekly limit      ███░░░░░░░   34% used  resets Thu 10:38 AM
      Credits           62,500  ≈15,625–81,250 messages

    GitHub Copilot  Pro
      Premium requests  ███████░░░   68% used  93.5 of 300 left · resets Nov 1
      Chat messages     Unlimited

    ClinePass  Cline Pass (Monthly)
      5-hour limit      ░░░░░░░░░░    0% used  $10 cap
      Weekly limit      ██████████  100% used  $25 cap · resets tomorrow 2:36 PM
      Monthly limit     ████████░░   81% used  $50 cap · resets Nov 12

  Details: https://chatgpt.com/settings/usage

  r refresh  ↑↓ select  esc close
────────────────────────────────────────────────────────────────────────────────
```

(80 columns — the bar yields from its 20 cells to 10 so the longest
trailer, ClinePass's weekly, still fits its row.)

- **The header**: the title in the pages' title dress, then the count and the
  time the numbers are from (`as of 2:25 AM`, in the local clock the reset
  times use) — or `fetching…` while a request is out, so a page left open
  says how old it is without ticking.
- **A block per subscription**, the one the session's model runs on first
  and marked `in use`, the rest in the `/login` list's order: the provider's
  name (bold, lit in the accent on the highlighted block with the family's
  `❯`), the plan, and the account when the answer named one.
- **A row per limit**: the label in a column aligned across the page; a
  bar of the share **used**, `█` over `░` — the cells the `/context` page's
  meter and codex's `/status` draw — up to 20 cells long, yielding down to 8
  so the longest trailer stays on its row; the share right-aligned so the
  column reads down the page; and a dim trailer of what the row adds (the
  count left, the cap) and when it resets. The fill is green, turning amber
  at 75% and red at 90% — the `/context` page's two thresholds — and the
  share takes the same colour, so a spent limit is a red `100% used`. An
  unlimited allowance is the word `Unlimited` and no bar. A trailer the row
  has no room for drops to the line under it, aligned with the bar; a bar the
  row has no room for goes, leaving the share.
- **Balances** after the limits, the value bright and its detail dim.
- **Notes** last: a warning in amber behind `⚠` (a limit reached, a spend cap
  hit), a plain fact dim.
- **The highlighted subscription's dashboard** under the blocks — the page
  where the provider shows the same numbers with history, as an OSC 8 link
  (`docs/links.md`) so a wrapped URL still opens whole.

Shares are shown the way a meter is read: `<1%` for a sliver that is not
nothing, a whole percent below that floored rather than rounded (so `99.6%`
is `99%` — the bar never claims a limit is spent before it is), `100%` only
when it is.

Reset times are **absolute**, never relative — codex's `/status` rule — so a
page left open never goes stale on screen: `resets 2:36 PM` the same day,
`resets tomorrow 2:36 PM`, `resets Tue 2:36 PM` within the week, `resets Nov
12` past it (with the year when it differs), and a date-only reset (Copilot's
monthly) as the date alone. The day is decided against the moment the
answer was fetched, in the local offset the boundary hands the page when it
opens.

### Fetching, failing, nothing signed in

- Each block shows `Fetching usage…` until its own answer lands; the blocks
  land independently, so a slow provider never holds up a quick one.
- A failed fetch is a red row in the block, behind `✗` — the provider's refusal worded
  the way the rest of the app words it (`/login` when the sign-in is no
  longer valid) — and `r` tries again.
- `r` re-fetches every block **keeping the numbers on screen** until each new
  answer lands, the subtitle saying `refreshing…` meanwhile.
- With **no subscription signed in**, `/usage` opens no page: a toast says so
  and names `/login` — the `/export` rule, a toast rather than an empty page.

## Keys

| key | does |
| --- | --- |
| ↑ / ↓ | move the highlight between subscriptions, wrapping |
| Home / End | the first / the last subscription |
| r | fetch every subscription's usage again |
| Esc, Ctrl+C | close the page |
| Ctrl+O, Ctrl+D | open the transcript / the raw context over the page, which waits behind them |

The page owns every other key while it is open. It works **mid-turn**, like
every page in its family: it replaces the composer and only the composer, so
the running turn's strip stays above it.

## A terminal too short for the page

The page sheds rows before it flows (`docs/view-flow.md` *A page that sheds
rows before it flows*), the `/donate` page's accordion:

1. the whole page;
2. the subscriptions the highlight is **not** on collapsed to their header
   row, which then names the tightest limit (`ClinePass  Cline Pass
   (Monthly) · Weekly limit 100% used`) — the highlighted one still whole,
   laid out on its own columns;
3. that page without the header;
4. that page without the dashboard link;
5. the highlighted subscription alone.

The hint stays on every page. Only a terminal shorter than the last page
bottom-anchors it and flows the rest into scrollback, like any still page.

## The fetch

`/usage` is pure up to the network: `App::run_selected_command` answers
`Action::OpenUsagePage`, and the boundary (`tui::usage`) builds the page's
rows from the providers table and the key store — every subscription with a
credential resolving, in the `/login` order, the active provider first —
opens it, and spawns **one worker thread per subscription**
(`workers::spawn_usage_fetch`, the `/model` fetch's shape). Each worker calls
the library's `llm::plan_usage::fetch`, which resolves the credential through
the same seams a turn does (`auth::request_auth` for ChatGPT and ClinePass —
so a rotated refresh token is written back exactly as a turn writes it — and
the stored token for Copilot) and parses the answer, and sends `(generation,
provider id, result)` on the loop's usage channel. Every open and every `r`
starts a new **generation**; an answer from an earlier one, or one landing
after the page closed, is dropped (`App::set_usage_result`), and closing the
page cancels the workers' token so a slow request reports nothing.

Requests are bounded: a 15-second timeout (codex's own for these reads) and
a 256 KiB body cap. The parses read **typed, lenient** fields — every field
of every answer is optional and a value of the wrong shape is skipped rather
than failing the answer — because these bodies are undocumented and change
without notice, and one surprising field must cost only itself. Nothing
builds a `Value` tree (`docs/memory.md`).

Copilot's API host is overridable with `ALTER_ZERO_GITHUB_API_BASE` (read at
the boundary, handed in once through `copilot::set_github_api_base`, which
the token exchange follows too), so the smoke suite can stand in for GitHub;
ChatGPT's and ClinePass's follow the provider's own base and the existing
`ALTER_ZERO_OPENAI_ISSUER` / `ALTER_ZERO_CLINE_API_BASE` overrides.

## What it does not do

- **No polling.** The numbers are fetched when the page opens and when `r`
  asks; the page says when they are from. Codex polls in the background to
  warn as a limit nears — a different feature, and one this page does not
  need to be right.
- **No warnings in the transcript.** Codex announces 75%/90%/95% as it
  crosses them; here a limit that runs out still surfaces as the provider's
  refusal, worded by each provider's advice (`docs/cline.md`,
  `docs/copilot.md`, `docs/chatgpt.md`).
- **No response-header reading.** ChatGPT (`x-codex-primary-used-percent`…)
  and Copilot (`x-quota-snapshot-…`, `x-usage-ratelimit-session`) also report
  limits on every chat response; reading them would keep a number live
  between opens, at the cost of a second source to keep consistent with the
  first.
- **No Claude.ai subscription usage** — see *Anthropic Console* above.

## API

- `llm::plan_usage` (pure parses, boundary fetch): `PlanUsage`, `Limit`,
  `Allowance`, `Meter`, `Reset`, `Balance`, `Note`; `parse_chatgpt`,
  `parse_copilot`, `parse_clinepass`, `clinepass_unsubscribed`,
  `console_usage`, `chatgpt_usage_url`, `window_name`, `refusal`,
  `group_thousands`, `count_text`, `dollars`, `dashboard_url`; `fetch`.
- `llm::copilot::set_github_api_base` / `GITHUB_API_ENV`
  (`ALTER_ZERO_GITHUB_API_BASE`), which the token exchange follows too.
- `app::UsagePage` / `UsageEntry` / `UsageState`, `App::usage_page`,
  `App::open_usage_page`, `App::begin_usage_refresh`, `App::set_usage_result`,
  `App::close_usage_page`; `Action::OpenUsagePage`, `Action::RefreshUsage`,
  `Action::CloseUsagePage`.
- `ui::usage_view_lines`, `ui::usage_page_height`, `ui::render_usage_page`.
- `tui::usage` (`Session::open_usage_page`, `refresh_usage`,
  `cancel_usage_fetch`, `on_usage_result`), `workers::spawn_usage_fetch`,
  `ModelSession::usage_subscriptions`.

## Tests

- `llm::plan_usage`: every parse against the live shapes (a Pro seat's
  `wham/usage`, a ClinePass account at its weekly limit) and the reference
  clients' fixtures, the window names, the URL rule, the lenient fields.
- `app::tests::usage`: the command's place in the palette, opening mid-turn,
  the generation guard, refresh keeping the numbers, the keys.
- `ui::tests::usage_view`: the layout, the bar's colours at the thresholds,
  the reset wording, every rung of the short-terminal ladder through
  `assert_frame_hugs_content`, the cursor seat.
- `smoke.sh` Phase 145: the page against a local stub standing in for
  ChatGPT, GitHub and Cline — every block, Copilot's refusal, the bars'
  colours off the raw SGR bytes, the highlight and its dashboard, `r` with
  the headers each read carried and the rotated token written back, a
  short terminal, Esc — and, in a config home of its own, the
  no-subscription toast.
- `tests/live_usage.rs` (`#[ignore]`d): each provider's real endpoint
  through `fetch`, printing what it answered — run on purpose, with the
  rotating sign-ins writing their new token to `ALTER_ZERO_LIVE_TOKEN_STORE`.
