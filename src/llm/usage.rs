//! A subscription's usage against its plan — what the `/usage` page shows
//! (`docs/usage.md`): the pure parse of each provider's own answer into one
//! provider-neutral [`UsageReport`], and the boundary [`fetch_usage`] that
//! asks for it.
//!
//! Only the **subscription** sign-ins have a plan to measure against
//! (`AuthScheme::is_subscription`): a pasted key bills per token and has no
//! window to fill. Each of them reports its usage its own way, and every
//! answer becomes the same report — a plan, who is signed in, and a list of
//! [`UsageMeter`]s — so the page draws one shape whatever asked.

use serde::Deserialize;
use serde_json::value::RawValue;

use super::config::{AuthScheme, ModelConfig};
use super::{LlmError, Result};
use crate::stream::CancelToken;

// ---------------------------------------------------------------------------
// The report (pure)
// ---------------------------------------------------------------------------

/// What one subscription reported about its usage, worded for the page.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct UsageReport {
    /// The plan's name as the page shows it — `Pro`, `Copilot Pro+`.
    pub plan: Option<String>,
    /// Who is signed in — an email address, or an organisation's name.
    pub account: Option<String>,
    /// The meters, in the order the page lists them.
    pub meters: Vec<UsageMeter>,
    /// What the meters can't say: a limit reached, a plan with no limits.
    pub notes: Vec<UsageNote>,
    /// The provider's own usage page, linked under the report.
    pub dashboard: Option<String>,
}

/// One row of a report: a label and what it measures.
#[derive(Debug, Clone, PartialEq)]
pub struct UsageMeter {
    /// The model family a per-model limit applies to; `None` for the plan's
    /// own limits.
    pub group: Option<String>,
    /// What the row measures — `5-hour limit`, `Premium requests`.
    pub label: String,
    pub value: MeterValue,
}

/// What a meter shows.
#[derive(Debug, Clone, PartialEq)]
pub enum MeterValue {
    /// A share of a limit, drawn as a bar.
    Limit(Limit),
    /// No limit applies.
    Unlimited,
    /// A balance or a count, already worded — `62,500 credits` — with a dim
    /// detail the page drops first when the row runs out of room.
    Text {
        value: String,
        detail: Option<String>,
    },
}

/// A share of a limit used.
#[derive(Debug, Clone, PartialEq)]
pub struct Limit {
    /// How much of the limit is used, in percent: 0 when none is, 100 when
    /// it is all gone.
    pub used_percent: f64,
    /// The units behind the share, when the provider counts them.
    pub counts: Option<Counts>,
    /// When the window starts over, in Unix seconds.
    pub resets_at: Option<i64>,
}

/// The units behind a [`Limit`]'s share: `used` of `total`, in `unit`.
#[derive(Debug, Clone, PartialEq)]
pub struct Counts {
    pub used: f64,
    pub total: f64,
    /// What is counted, when the label doesn't already say — `credits`.
    pub unit: Option<&'static str>,
}

/// A sentence the page shows under a report's meters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageNote {
    pub tone: NoteTone,
    pub text: String,
}

/// How a [`UsageNote`] is coloured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoteTone {
    /// Dim: a fact about the plan.
    Info,
    /// Amber: something to act on soon.
    Warning,
    /// Red: a limit that is stopping requests now.
    Error,
}

impl UsageNote {
    fn new(tone: NoteTone, text: impl Into<String>) -> Self {
        Self {
            tone,
            text: text.into(),
        }
    }
}

// ---------------------------------------------------------------------------
// Wording shared by every provider (pure)
// ---------------------------------------------------------------------------

/// A rolling window's row label from its length in seconds: `5-hour limit`,
/// `Daily limit`, `Weekly limit`, `Monthly limit`, `Annual limit` for a
/// window within 5% of one (codex's tolerance — a backend's month is not
/// always thirty days), else the length itself (`2-hour limit`,
/// `30-minute limit`), and `Usage limit` for a window that names none.
#[must_use]
pub fn window_label(seconds: i64) -> String {
    const HOUR: i64 = 3_600;
    const DAY: i64 = 24 * HOUR;
    const NAMED: [(i64, &str); 5] = [
        (5 * HOUR, "5-hour limit"),
        (DAY, "Daily limit"),
        (7 * DAY, "Weekly limit"),
        (30 * DAY, "Monthly limit"),
        (365 * DAY, "Annual limit"),
    ];
    if seconds <= 0 {
        return "Usage limit".to_string();
    }
    let near = |length: i64| {
        let (seconds, length) = (seconds as f64, length as f64);
        seconds >= length * 0.95 && seconds <= length * 1.05
    };
    if let Some((_, label)) = NAMED.iter().find(|(length, _)| near(*length)) {
        return (*label).to_string();
    }
    // Rounded up to whole minutes, as codex reads a window's length, then
    // named in the largest unit that measures it exactly.
    let minutes = (seconds + 59) / 60;
    if minutes % (24 * 60) == 0 {
        format!("{}-day limit", minutes / (24 * 60))
    } else if minutes % 60 == 0 {
        format!("{}-hour limit", minutes / 60)
    } else {
        format!("{minutes}-minute limit")
    }
}

/// `n` with its thousands grouped by commas: `62,500`. Rounded to a whole
/// number first.
#[must_use]
pub fn group_thousands(n: f64) -> String {
    let rounded = n.round();
    if !rounded.is_finite() {
        return n.to_string();
    }
    let digits = format!("{:.0}", rounded.abs());
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3 + 1);
    if rounded < 0.0 {
        grouped.push('-');
    }
    for (i, digit) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped
}

/// A large count shortened for a detail: bare under a thousand, one decimal
/// of thousands or millions past it (`15.6k`, `1.2M`) with a trailing `.0`
/// dropped — the footer's token count's rule.
fn compact_count(n: f64) -> String {
    let scaled = |scale: f64, suffix: &str| {
        let value = (n / scale * 10.0).round() / 10.0;
        if value.fract() == 0.0 {
            format!("{value:.0}{suffix}")
        } else {
            format!("{value:.1}{suffix}")
        }
    };
    if n < 1_000.0 {
        format!("{n:.0}")
    } else if n < 1_000_000.0 {
        scaled(1_000.0, "k")
    } else {
        scaled(1_000_000.0, "M")
    }
}

/// `raw` read as `T`, or `None` when it is absent, `null`, or a shape `T`
/// doesn't read — the leniency every section of a usage answer gets, since
/// these endpoints are the provider's own and change without notice.
fn lenient<'a, T: Deserialize<'a>>(raw: Option<&'a RawValue>) -> Option<T> {
    raw.and_then(|raw| serde_json::from_str(raw.get()).ok())
}

/// A number the wire may send as a JSON number or as a string of one
/// (`"62500.0000000000"`, Copilot's `"1000"`).
fn number(raw: Option<&RawValue>) -> Option<f64> {
    lenient::<f64>(raw)
        .or_else(|| lenient::<String>(raw).and_then(|text| text.trim().parse().ok()))
        .filter(|n| n.is_finite())
}

/// The body's top level, refusing anything but an object: a captive portal's
/// HTML or a JSON array is not a usage answer, however leniently read.
fn object<'a, T: Deserialize<'a>>(body: &'a str) -> Result<T> {
    if !body.trim_start().starts_with('{') {
        return Err(LlmError::Decode(
            "the usage answer was not a JSON object".to_string(),
        ));
    }
    serde_json::from_str(body).map_err(|e| LlmError::Decode(e.to_string()))
}

// ---------------------------------------------------------------------------
// ChatGPT (pure)
// ---------------------------------------------------------------------------

/// Where ChatGPT's own usage page is.
pub const CHATGPT_DASHBOARD: &str = "https://chatgpt.com/settings/usage";

/// The usage endpoint for a ChatGPT provider whose chat base is `base`.
///
/// The usage lives on the backend's **root** (`…/backend-api/wham/usage`),
/// not under the chat base's `/codex`. A base on a ChatGPT host that names no
/// `/backend-api` gains it, and a base that isn't one (a self-hosted backend,
/// the smoke suite's stub) is asked at `/api/codex/usage` — codex's own two
/// path styles (`backend-client`'s `PathStyle`).
#[must_use]
pub fn chatgpt_usage_url(base: &str) -> String {
    const BACKEND: &str = "/backend-api";
    let base = base.trim().trim_end_matches('/');
    if let Some(at) = base.find(BACKEND) {
        return format!("{}/wham/usage", &base[..at + BACKEND.len()]);
    }
    if base.starts_with("https://chatgpt.com") || base.starts_with("https://chat.openai.com") {
        let origin = base.splitn(4, '/').take(3).collect::<Vec<_>>().join("/");
        return format!("{origin}{BACKEND}/wham/usage");
    }
    let root = base
        .strip_suffix("/codex")
        .unwrap_or(base)
        .trim_end_matches('/');
    format!("{root}/api/codex/usage")
}

/// The `wham/usage` answer's top level. Every section is kept as the raw
/// slice it arrived as and read on its own ([`lenient`]), so one this parse
/// no longer understands costs only itself.
#[derive(Deserialize)]
struct WhamUsage<'a> {
    #[serde(default, borrow)]
    plan_type: Option<&'a RawValue>,
    #[serde(default, borrow)]
    email: Option<&'a RawValue>,
    #[serde(default, borrow)]
    rate_limit: Option<&'a RawValue>,
    #[serde(default, borrow)]
    additional_rate_limits: Option<&'a RawValue>,
    #[serde(default, borrow)]
    code_review_rate_limit: Option<&'a RawValue>,
    #[serde(default, borrow)]
    credits: Option<&'a RawValue>,
    #[serde(default, borrow)]
    spend_control: Option<&'a RawValue>,
    #[serde(default, borrow)]
    rate_limit_reached_type: Option<&'a RawValue>,
    #[serde(default, borrow)]
    rate_limit_reset_credits: Option<&'a RawValue>,
}

/// One limit's state: whether it is spent, and its (up to two) windows.
#[derive(Deserialize)]
struct WhamRateLimit<'a> {
    #[serde(default)]
    limit_reached: Option<bool>,
    #[serde(default, borrow)]
    primary_window: Option<&'a RawValue>,
    #[serde(default, borrow)]
    secondary_window: Option<&'a RawValue>,
}

/// One rolling window. The numbers are read as `f64`: the generated model
/// says integers, the response headers carry fractions, and a share that
/// arrives as `12.5` should not cost the row.
#[derive(Deserialize)]
struct WhamWindow {
    used_percent: f64,
    #[serde(default)]
    limit_window_seconds: Option<f64>,
    #[serde(default)]
    reset_after_seconds: Option<f64>,
    #[serde(default)]
    reset_at: Option<f64>,
}

/// A per-model limit, named by the model family it meters.
#[derive(Deserialize)]
struct WhamAdditional<'a> {
    #[serde(default)]
    limit_name: Option<String>,
    #[serde(default)]
    metered_feature: Option<String>,
    #[serde(default, borrow)]
    rate_limit: Option<&'a RawValue>,
}

#[derive(Deserialize)]
struct WhamCredits<'a> {
    #[serde(default)]
    has_credits: bool,
    #[serde(default)]
    unlimited: bool,
    #[serde(default, borrow)]
    balance: Option<&'a RawValue>,
    #[serde(default, borrow)]
    approx_local_messages: Option<&'a RawValue>,
}

/// A workspace's spend controls: whether its cap is reached, and the
/// member's own monthly credit limit when the workspace sets one.
#[derive(Deserialize)]
struct WhamSpendControl<'a> {
    #[serde(default)]
    reached: bool,
    #[serde(default, borrow)]
    individual_limit: Option<&'a RawValue>,
}

#[derive(Deserialize)]
struct WhamIndividualLimit<'a> {
    #[serde(borrow)]
    limit: &'a RawValue,
    #[serde(borrow)]
    used: &'a RawValue,
    #[serde(default)]
    used_percent: Option<f64>,
    #[serde(default)]
    remaining_percent: Option<f64>,
    #[serde(default)]
    reset_at: Option<f64>,
    #[serde(default)]
    reset_after_seconds: Option<f64>,
}

#[derive(Deserialize)]
struct WhamReached {
    #[serde(rename = "type")]
    kind: String,
}

#[derive(Deserialize)]
struct WhamResetCredits {
    #[serde(default)]
    available_count: i64,
}

/// When a window starts over: its own `reset_at`, else `now` plus the
/// countdown it names.
fn reset_time(reset_at: Option<f64>, reset_after: Option<f64>, now: i64) -> Option<i64> {
    reset_at
        .filter(|at| *at > 0.0)
        .map(|at| at.round() as i64)
        .or_else(|| reset_after.map(|after| now + after.round() as i64))
}

/// One window's meter, labelled by its length.
fn wham_window_meter(group: Option<&str>, window: &WhamWindow, now: i64) -> UsageMeter {
    UsageMeter {
        group: group.map(str::to_string),
        label: window_label(window.limit_window_seconds.map_or(0, |s| s.round() as i64)),
        value: MeterValue::Limit(Limit {
            used_percent: window.used_percent,
            counts: None,
            resets_at: reset_time(window.reset_at, window.reset_after_seconds, now),
        }),
    }
}

/// A limit's windows, primary then secondary — each one read on its own.
fn wham_limit_meters(limit: &WhamRateLimit<'_>, group: Option<&str>, now: i64) -> Vec<UsageMeter> {
    [limit.primary_window, limit.secondary_window]
        .into_iter()
        .filter_map(lenient::<WhamWindow>)
        .map(|window| wham_window_meter(group, &window, now))
        .collect()
}

/// The credits row: unlimited, a balance (with what it buys, roughly, when
/// the backend says), `Available` when it names none, and no row at all for
/// a seat with no credits — codex's `credit_status_row`.
fn wham_credits_meter(credits: &WhamCredits<'_>) -> Option<UsageMeter> {
    let value = if credits.unlimited {
        MeterValue::Unlimited
    } else if !credits.has_credits {
        return None;
    } else {
        match number(credits.balance).filter(|balance| *balance > 0.0) {
            Some(balance) => MeterValue::Text {
                value: format!("{} credits", group_thousands(balance)),
                detail: lenient::<Vec<f64>>(credits.approx_local_messages).and_then(|range| {
                    match range.as_slice() {
                        [low, high] if *low > 0.0 && high > low => Some(format!(
                            "≈ {}–{} local messages",
                            compact_count(*low),
                            compact_count(*high)
                        )),
                        [count] | [count, _] if *count > 0.0 => {
                            Some(format!("≈ {} local messages", compact_count(*count)))
                        }
                        _ => None,
                    }
                }),
            },
            None => MeterValue::Text {
                value: "Available".to_string(),
                detail: None,
            },
        }
    };
    Some(UsageMeter {
        group: None,
        label: "Credits".to_string(),
        value,
    })
}

/// The member's monthly credit limit a workspace's spend controls set:
/// credits used of the limit, codex's `Monthly credit limit` row.
fn wham_credit_limit_meter(limit: &WhamIndividualLimit<'_>, now: i64) -> Option<UsageMeter> {
    let used = number(Some(limit.used))?;
    let total = number(Some(limit.limit))?;
    let used_percent = limit
        .used_percent
        .or_else(|| limit.remaining_percent.map(|remaining| 100.0 - remaining))
        .or_else(|| (total > 0.0).then(|| used / total * 100.0))?;
    Some(UsageMeter {
        group: None,
        label: "Monthly credit limit".to_string(),
        value: MeterValue::Limit(Limit {
            used_percent,
            counts: Some(Counts {
                used,
                total,
                unit: Some("credits"),
            }),
            resets_at: reset_time(limit.reset_at, limit.reset_after_seconds, now),
        }),
    })
}

/// Why requests are stopped, in codex's words for who can lift it.
fn wham_reached_note(
    kind: Option<&str>,
    limit_reached: bool,
    cap_reached: bool,
) -> Option<UsageNote> {
    let text = match kind {
        Some("workspace_owner_credits_depleted") => {
            "Your workspace is out of credits. Add credits to continue."
        }
        Some("workspace_member_credits_depleted") => {
            "Your workspace is out of credits. Ask your workspace owner to refill in order to continue."
        }
        Some("workspace_owner_usage_limit_reached") => {
            "You hit your spend cap set in your workspace. Increase your spend cap to continue."
        }
        Some("workspace_member_usage_limit_reached") => {
            "You hit your spend cap set by the owner of your workspace. Ask an owner to increase your spend cap to continue."
        }
        Some(_) => "You've hit your usage limit.",
        None if limit_reached => "You've hit your usage limit.",
        None if cap_reached => "You've reached your workspace's spend cap.",
        None => return None,
    };
    Some(UsageNote::new(NoteTone::Error, text))
}

/// Parse ChatGPT's `wham/usage` answer. `now` (Unix seconds) places a window
/// that names only how long until it resets.
///
/// The rows come in codex's order — the plan's own windows, its credits and
/// the member's credit limit, then each per-model limit under its model's
/// name, then the code-review limit — and the reset credits the account can
/// redeem last.
///
/// # Errors
/// [`LlmError::Decode`] when the body is not a JSON object. Everything inside
/// it is read leniently: a section this parse no longer understands is left
/// out rather than costing the whole report.
pub fn parse_chatgpt_usage(body: &str, now: i64) -> Result<UsageReport> {
    let usage: WhamUsage<'_> = object(body)?;
    let plan = lenient::<String>(usage.plan_type)
        .filter(|plan| !plan.trim().is_empty())
        .map(|plan| super::chatgpt::plan_label(&plan));
    let account = lenient::<String>(usage.email).filter(|email| !email.trim().is_empty());
    let rate_limit = lenient::<WhamRateLimit<'_>>(usage.rate_limit);
    let spend = lenient::<WhamSpendControl<'_>>(usage.spend_control);

    let mut meters = Vec::new();
    if let Some(limit) = &rate_limit {
        meters.extend(wham_limit_meters(limit, None, now));
    }
    if let Some(credits) = lenient::<WhamCredits<'_>>(usage.credits) {
        meters.extend(wham_credits_meter(&credits));
    }
    if let Some(limit) = spend
        .as_ref()
        .and_then(|spend| lenient::<WhamIndividualLimit<'_>>(spend.individual_limit))
    {
        meters.extend(wham_credit_limit_meter(&limit, now));
    }
    for raw in lenient::<Vec<&RawValue>>(usage.additional_rate_limits).unwrap_or_default() {
        let Some(additional) = lenient::<WhamAdditional<'_>>(Some(raw)) else {
            continue;
        };
        let name = [&additional.limit_name, &additional.metered_feature]
            .into_iter()
            .flatten()
            .find(|name| !name.trim().is_empty())
            .map_or("Other models", String::as_str);
        if let Some(limit) = lenient::<WhamRateLimit<'_>>(additional.rate_limit) {
            meters.extend(wham_limit_meters(&limit, Some(name), now));
        }
    }
    if let Some(limit) = lenient::<WhamRateLimit<'_>>(usage.code_review_rate_limit) {
        meters.extend(wham_limit_meters(&limit, Some("Code review"), now));
    }
    if let Some(resets) = lenient::<WhamResetCredits>(usage.rate_limit_reset_credits)
        .filter(|resets| resets.available_count > 0)
    {
        meters.push(UsageMeter {
            group: None,
            label: "Limit resets".to_string(),
            value: MeterValue::Text {
                value: format!("{} available", resets.available_count),
                detail: None,
            },
        });
    }

    let notes = wham_reached_note(
        lenient::<WhamReached>(usage.rate_limit_reached_type)
            .map(|reached| reached.kind)
            .as_deref(),
        rate_limit
            .as_ref()
            .is_some_and(|limit| limit.limit_reached == Some(true)),
        spend.as_ref().is_some_and(|spend| spend.reached),
    )
    .into_iter()
    .collect();

    Ok(UsageReport {
        plan,
        account,
        meters,
        notes,
        dashboard: Some(CHATGPT_DASHBOARD.to_string()),
    })
}

// ---------------------------------------------------------------------------
// ClinePass (pure)
// ---------------------------------------------------------------------------

/// Where a ClinePass subscription is managed and its usage shown.
pub const CLINEPASS_DASHBOARD: &str = "https://app.cline.bot/dashboard/subscription?personal=true";

/// The plan a `GET /api/v1/users/me/plan` answer names — its display name,
/// `Cline Pass (Monthly)` (its internal name when it has none, `ClinePass`
/// when it has neither) — or `None` when the account has no plan: `plan` is
/// `null`, which is how Cline says the account is not subscribed.
///
/// # Errors
/// [`LlmError::Decode`] when the body is not Cline's `{success, data}`
/// envelope — an answer this parse cannot read is not an answer that says
/// "no plan".
pub fn parse_clinepass_plan(body: &str) -> Result<Option<String>> {
    #[derive(Deserialize)]
    struct PlanData<'a> {
        #[serde(default, borrow)]
        plan: Option<&'a RawValue>,
    }
    #[derive(Deserialize)]
    struct Plan {
        #[serde(rename = "displayName", default)]
        display_name: Option<String>,
        #[serde(default)]
        name: Option<String>,
    }
    let data = cline_data(body)?;
    let Some(plan) = lenient::<PlanData<'_>>(Some(data)).and_then(|data| data.plan) else {
        return Ok(None);
    };
    let Some(plan) = lenient::<Plan>(Some(plan)) else {
        return Ok(None);
    };
    Ok(Some(
        [plan.display_name, plan.name]
            .into_iter()
            .flatten()
            .map(|name| name.trim().to_string())
            .find(|name| !name.is_empty())
            .unwrap_or_else(|| "ClinePass".to_string()),
    ))
}

/// Every Cline answer's `{success, data, error?}` envelope, opened: its
/// `data`, or why there is none.
fn cline_data(body: &str) -> Result<&RawValue> {
    #[derive(Deserialize)]
    struct Envelope<'a> {
        #[serde(default)]
        success: Option<bool>,
        #[serde(default, borrow)]
        data: Option<&'a RawValue>,
        #[serde(default)]
        error: Option<String>,
    }
    let envelope: Envelope<'_> = object(body)?;
    if envelope.success == Some(false) {
        return Err(LlmError::Decode(
            envelope
                .error
                .unwrap_or_else(|| "Cline answered without success".to_string()),
        ));
    }
    envelope
        .data
        .filter(|data| data.get() != "null")
        .ok_or_else(|| LlmError::Decode("Cline's answer carried no data".to_string()))
}

/// One ClinePass window as the usage-limits answer names it.
#[derive(Deserialize)]
struct ClineLimit {
    #[serde(rename = "type", default)]
    kind: String,
    #[serde(rename = "percentUsed")]
    percent_used: f64,
    #[serde(rename = "resetsAt", default)]
    resets_at: Option<String>,
}

/// A ClinePass window's row label from its `type`: the three the dashboard
/// shows by name, and any other read off its own words (`two_day` →
/// `Two day limit`), so a window Cline adds is a row the day it is listed.
fn clinepass_window_label(kind: &str) -> String {
    match kind {
        "five_hour" => "5-hour limit".to_string(),
        "daily" => "Daily limit".to_string(),
        "weekly" => "Weekly limit".to_string(),
        "monthly" => "Monthly limit".to_string(),
        other => {
            let words = other.replace(['_', '-'], " ");
            let words = words.trim();
            let mut chars = words.chars();
            match chars.next() {
                Some(first) => format!("{}{} limit", first.to_uppercase(), chars.as_str()),
                None => "Usage limit".to_string(),
            }
        }
    }
}

/// Parse `GET /api/v1/users/me/plan/usage-limits`: the plan's rolling
/// windows (`five_hour`, `weekly`, `monthly`), each a share used and when it
/// starts over — the Cline dashboard's own *Usage Limits* card. A window that
/// is spent is a red note too, naming — like the dashboard's banner — the
/// spent window that resets **last**, since that one is how long the pause
/// lasts.
///
/// # Errors
/// [`LlmError::Decode`] when the body is not a `{success, data: {limits}}`
/// envelope.
pub fn parse_clinepass_limits(body: &str) -> Result<UsageReport> {
    #[derive(Deserialize)]
    struct Limits<'a> {
        #[serde(borrow)]
        limits: Vec<&'a RawValue>,
    }
    let data = cline_data(body)?;
    let limits: Limits<'_> = serde_json::from_str(data.get())
        .map_err(|e| LlmError::Decode(format!("ClinePass usage limits: {e}")))?;
    let mut meters = Vec::new();
    let mut spent: Option<(Option<i64>, String)> = None;
    for limit in limits
        .limits
        .into_iter()
        .filter_map(|raw| lenient::<ClineLimit>(Some(raw)))
    {
        let label = clinepass_window_label(&limit.kind);
        let resets_at = limit.resets_at.as_deref().and_then(timestamp_seconds);
        if limit.percent_used >= 100.0 && spent.as_ref().is_none_or(|(at, _)| resets_at > *at) {
            spent = Some((resets_at, label.clone()));
        }
        meters.push(UsageMeter {
            group: None,
            label,
            value: MeterValue::Limit(Limit {
                used_percent: limit.percent_used,
                counts: None,
                resets_at,
            }),
        });
    }
    let notes = spent
        .map(|(_, label)| {
            UsageNote::new(
                NoteTone::Error,
                format!(
                    "Your {} has been reached. ClinePass requests are paused until it resets — \
                     switch with /model to the usage-billed Cline row or another provider \
                     meanwhile.",
                    label.to_lowercase()
                ),
            )
        })
        .into_iter()
        .collect();
    Ok(UsageReport {
        plan: None,
        account: None,
        meters,
        notes,
        dashboard: Some(CLINEPASS_DASHBOARD.to_string()),
    })
}

/// An RFC 3339 instant (`2026-10-27T14:36:37.287853835Z`) or a bare date
/// (`2026-11-01`, read as midnight UTC) as Unix seconds.
fn timestamp_seconds(text: &str) -> Option<i64> {
    let text = text.trim();
    chrono::DateTime::parse_from_rfc3339(text)
        .map(|at| at.timestamp())
        .ok()
        .or_else(|| {
            chrono::NaiveDate::parse_from_str(text, "%Y-%m-%d")
                .ok()
                .and_then(|day| day.and_hms_opt(0, 0, 0))
                .map(|midnight| midnight.and_utc().timestamp())
        })
}

// ---------------------------------------------------------------------------
// GitHub Copilot (pure)
// ---------------------------------------------------------------------------

/// Where a Copilot seat's allowance and budget are managed.
pub const COPILOT_DASHBOARD: &str = "https://github.com/settings/copilot";

/// Parse `GET https://api.github.com/copilot_internal/user`, the endpoint
/// VS Code's own Copilot dashboard reads: the plan, and one meter per quota
/// snapshot — the premium allowance (`Credits` on a token-billed seat), chat
/// messages, inline suggestions — or, for a seat on the older Free plan, its
/// `limited_user_quotas`.
///
/// # Errors
/// [`LlmError::Decode`] when the body is not a JSON object.
pub fn parse_copilot_user(body: &str) -> Result<UsageReport> {
    let user: CopilotUser<'_> = object(body)?;
    let plan = lenient::<String>(user.copilot_plan)
        .filter(|plan| !plan.trim().is_empty())
        .map(|plan| copilot_plan_label(&plan));
    let account = lenient::<String>(user.login).filter(|login| !login.trim().is_empty());
    // The allowance's own reset when a snapshot names none: every paid SKU
    // resets at midnight UTC on the 1st (`quota_reset_date_utc` carrying the
    // time, `quota_reset_date` the day), the old Free plan on its own date.
    let fallback_reset = [
        user.quota_reset_date_utc,
        user.quota_reset_date,
        user.limited_user_reset_date,
    ]
    .into_iter()
    .find_map(|raw| {
        lenient::<String>(raw)
            .as_deref()
            .and_then(timestamp_seconds)
    });
    let credits = lenient::<bool>(user.token_based_billing).unwrap_or(false);

    let mut meters = Vec::new();
    let mut notes = Vec::new();
    if let Some(snapshots) = lenient::<CopilotSnapshots<'_>>(user.quota_snapshots) {
        let premium_label = if credits {
            "Credits"
        } else {
            "Premium requests"
        };
        for (label, raw) in [
            (
                premium_label,
                snapshots.premium_interactions.or(snapshots.premium_models),
            ),
            ("Chat messages", snapshots.chat),
            ("Inline suggestions", snapshots.completions),
        ] {
            if let Some(snapshot) = lenient::<CopilotSnapshot<'_>>(raw) {
                meters.extend(copilot_meter(label, &snapshot, fallback_reset, &mut notes));
            }
        }
    }
    if meters.is_empty()
        && let (Some(left), Some(monthly)) = (
            lenient::<CopilotFreeQuotas>(user.limited_user_quotas),
            lenient::<CopilotFreeQuotas>(user.monthly_quotas),
        )
    {
        for (label, left, total) in [
            ("Chat messages", left.chat, monthly.chat),
            ("Inline suggestions", left.completions, monthly.completions),
        ] {
            if let (Some(left), Some(total)) = (left, total)
                && total > 0.0
            {
                let used = (total - left).clamp(0.0, total);
                meters.push(UsageMeter {
                    group: None,
                    label: label.to_string(),
                    value: MeterValue::Limit(Limit {
                        used_percent: used / total * 100.0,
                        counts: Some(Counts {
                            used,
                            total,
                            unit: None,
                        }),
                        resets_at: fallback_reset,
                    }),
                });
            }
        }
    }
    Ok(UsageReport {
        plan,
        account,
        meters,
        notes,
        dashboard: Some(COPILOT_DASHBOARD.to_string()),
    })
}

/// The `copilot_internal/user` answer's top level, each field read on its
/// own: the shape has grown since it first shipped, and the numbers arrive
/// as numbers in older answers and as numeric strings in newer ones.
#[derive(Deserialize)]
struct CopilotUser<'a> {
    #[serde(default, borrow)]
    copilot_plan: Option<&'a RawValue>,
    #[serde(default, borrow)]
    login: Option<&'a RawValue>,
    #[serde(default, borrow)]
    token_based_billing: Option<&'a RawValue>,
    #[serde(default, borrow)]
    quota_reset_date_utc: Option<&'a RawValue>,
    #[serde(default, borrow)]
    quota_reset_date: Option<&'a RawValue>,
    #[serde(default, borrow)]
    limited_user_reset_date: Option<&'a RawValue>,
    #[serde(default, borrow)]
    quota_snapshots: Option<&'a RawValue>,
    #[serde(default, borrow)]
    limited_user_quotas: Option<&'a RawValue>,
    #[serde(default, borrow)]
    monthly_quotas: Option<&'a RawValue>,
}

#[derive(Deserialize)]
struct CopilotSnapshots<'a> {
    #[serde(default, borrow)]
    premium_interactions: Option<&'a RawValue>,
    /// The same bucket under the name some answers give it.
    #[serde(default, borrow)]
    premium_models: Option<&'a RawValue>,
    #[serde(default, borrow)]
    chat: Option<&'a RawValue>,
    #[serde(default, borrow)]
    completions: Option<&'a RawValue>,
}

/// One quota bucket.
#[derive(Deserialize)]
struct CopilotSnapshot<'a> {
    #[serde(default, borrow)]
    entitlement: Option<&'a RawValue>,
    #[serde(default, borrow)]
    remaining: Option<&'a RawValue>,
    #[serde(default, borrow)]
    quota_remaining: Option<&'a RawValue>,
    #[serde(default)]
    percent_remaining: Option<f64>,
    #[serde(default)]
    unlimited: bool,
    #[serde(default)]
    overage_count: Option<f64>,
    #[serde(default)]
    overage_permitted: bool,
    #[serde(default)]
    quota_reset_at: Option<f64>,
}

/// The old Free plan's per-feature counts: what is left
/// (`limited_user_quotas`) and the month's allowance (`monthly_quotas`).
#[derive(Deserialize)]
struct CopilotFreeQuotas {
    #[serde(default)]
    chat: Option<f64>,
    #[serde(default)]
    completions: Option<f64>,
}

/// A plan slug as VS Code names it, without the `Copilot` every one of them
/// would repeat beside the provider's own name.
fn copilot_plan_label(slug: &str) -> String {
    match slug.trim().to_ascii_lowercase().as_str() {
        "free" => "Free".to_string(),
        "individual" => "Pro".to_string(),
        "individual_pro" => "Pro+".to_string(),
        "individual_max" => "Max".to_string(),
        "individual_edu" => "Student".to_string(),
        "business" => "Business".to_string(),
        "enterprise" => "Enterprise".to_string(),
        other => other
            .split(['_', '-'])
            .filter(|word| !word.is_empty())
            .map(|word| {
                let mut chars = word.chars();
                chars.next().map_or_else(String::new, |first| {
                    first.to_uppercase().collect::<String>() + chars.as_str()
                })
            })
            .collect::<Vec<_>>()
            .join(" "),
    }
}

/// One quota bucket's meter — `None` for a bucket that measures nothing (a
/// zero allowance that isn't unlimited: a token-billed Business seat's
/// placeholder, VS Code's `parseQuotas` rule). The share is the answer's own
/// `percent_remaining`, which VS Code trusts over the counts; the counts are
/// the allowance less what remains. A spent allowance the plan won't extend
/// is a red note, and usage billed past the allowance a dim one.
fn copilot_meter(
    label: &str,
    snapshot: &CopilotSnapshot<'_>,
    fallback_reset: Option<i64>,
    notes: &mut Vec<UsageNote>,
) -> Option<UsageMeter> {
    let meter = |value| UsageMeter {
        group: None,
        label: label.to_string(),
        value,
    };
    if snapshot.unlimited {
        return Some(meter(MeterValue::Unlimited));
    }
    let total = number(snapshot.entitlement).filter(|total| *total > 0.0)?;
    let remaining = number(snapshot.quota_remaining).or_else(|| number(snapshot.remaining));
    let used_percent = match (snapshot.percent_remaining, remaining) {
        (Some(percent), _) => 100.0 - percent.clamp(0.0, 100.0),
        (None, Some(remaining)) => ((total - remaining) / total * 100.0).clamp(0.0, 100.0),
        (None, None) => return None,
    };
    let used = remaining.map_or(total * used_percent / 100.0, |remaining| {
        (total - remaining).clamp(0.0, total)
    });
    let lower = label.to_lowercase();
    if used_percent >= 100.0 && !snapshot.overage_permitted {
        notes.push(UsageNote::new(
            NoteTone::Error,
            format!("Your {lower} are used up until the allowance resets."),
        ));
    }
    if let Some(over) = snapshot.overage_count.filter(|over| *over > 0.0) {
        notes.push(UsageNote::new(
            NoteTone::Info,
            format!(
                "{} {lower} billed past the allowance.",
                group_thousands(over)
            ),
        ));
    }
    Some(meter(MeterValue::Limit(Limit {
        used_percent,
        counts: Some(Counts {
            used,
            total,
            unit: None,
        }),
        resets_at: snapshot
            .quota_reset_at
            .filter(|at| *at > 0.0)
            .map(|at| at.round() as i64)
            .or(fallback_reset),
    })))
}

// ---------------------------------------------------------------------------
// Anthropic Console (pure)
// ---------------------------------------------------------------------------

/// Where a Console organisation's usage and spend are shown.
pub const CONSOLE_DASHBOARD: &str = "https://platform.claude.com/usage";

/// What the page says for an Anthropic Console sign-in. A Console account
/// bills per token to its API organisation — there is no plan, so there is
/// no window to fill — and the honest report is the organisation it bills
/// and where its spend is shown.
#[must_use]
pub fn console_report(organization: Option<String>) -> UsageReport {
    UsageReport {
        plan: None,
        account: organization.filter(|name| !name.trim().is_empty()),
        meters: Vec::new(),
        notes: vec![UsageNote::new(
            NoteTone::Info,
            "Billed per token to your API organisation — a Console sign-in has no plan \
             limits to fill.",
        )],
        dashboard: Some(CONSOLE_DASHBOARD.to_string()),
    }
}

// ---------------------------------------------------------------------------
// The fetch (boundary)
// ---------------------------------------------------------------------------

/// The most a usage answer may buffer. Every real one is a few KB; the bound
/// keeps an intercepting proxy's HTML page from becoming the "reason" a
/// failure shows.
const USAGE_BODY_MAX_BYTES: u64 = 256 * 1024;

/// The `User-Agent` a read carries when the credential implies none — GitHub's
/// REST API refuses a request without one, and a server's logs should be able
/// to tell who asked.
fn default_user_agent() -> String {
    format!("alter-zero/{}", env!("CARGO_PKG_VERSION"))
}

/// The sentence for a usage read the provider refused: what to do about it,
/// then the provider's own words when it said something showable (a JSON
/// `message`/`error`, or a short line of text — never an HTML page).
#[must_use]
pub fn usage_advice(status: u16, body: &str) -> String {
    let advice = match status {
        401 => Some("the sign-in was refused — run /login to sign in again"),
        403 => Some("the provider refused this read — run /login to sign in again if it persists"),
        404 => Some("no usage is reported for this account"),
        429 => Some("rate limited — try again in a minute"),
        500..=599 => Some("the provider's usage service is having trouble — try again shortly"),
        _ => None,
    };
    match (advice, provider_said(body)) {
        (Some(advice), Some(said)) => format!("{advice} ({said})"),
        (Some(advice), None) => advice.to_string(),
        (None, Some(said)) => said,
        (None, None) => String::new(),
    }
}

/// What a refusal's body says, on one line: a JSON body's `message`,
/// `error_description`, `detail` or `error` (itself a string or an object
/// with a `message`), or a body that is plain text. An HTML page — an
/// intercepting proxy's — says nothing showable.
fn provider_said(body: &str) -> Option<String> {
    #[derive(Deserialize)]
    struct Said<'a> {
        #[serde(default, borrow)]
        message: Option<&'a RawValue>,
        #[serde(default, borrow)]
        error_description: Option<&'a RawValue>,
        #[serde(default, borrow)]
        detail: Option<&'a RawValue>,
        #[serde(default, borrow)]
        error: Option<&'a RawValue>,
    }
    #[derive(Deserialize)]
    struct Nested {
        #[serde(default)]
        message: Option<String>,
    }
    const MAX_CHARS: usize = 160;
    let text = body.trim();
    let said = if text.starts_with('{') {
        let said: Said<'_> = serde_json::from_str(text).ok()?;
        [
            said.message,
            said.error_description,
            said.detail,
            said.error,
        ]
        .into_iter()
        .find_map(|raw| {
            lenient::<String>(raw).or_else(|| lenient::<Nested>(raw).and_then(|n| n.message))
        })?
    } else if text.is_empty() || text.starts_with('<') {
        return None;
    } else {
        text.to_string()
    };
    let flat = said.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.is_empty() {
        return None;
    }
    if flat.chars().count() <= MAX_CHARS {
        return Some(flat);
    }
    let mut cut: String = flat.chars().take(MAX_CHARS).collect();
    cut.push('…');
    Some(cut)
}

/// GET one usage answer: its status and its body, bounded. `headers` are sent
/// as given, with `Accept: application/json` and — when they name none — a
/// [`default_user_agent`].
fn get_body(url: &str, headers: &[(String, String)]) -> Result<(u16, String)> {
    use std::io::Read;
    // The `/model` listing's deadline, on purpose: `http_client` keeps one
    // client per timeout, and a usage-only one would pay a second client
    // stack for the life of the process (`openai::NET_OP_TIMEOUT`).
    let client = super::http_client(super::openai::NET_OP_TIMEOUT)?;
    let names = |name: &str| {
        headers
            .iter()
            .any(|(key, _)| key.eq_ignore_ascii_case(name))
    };
    let mut request = client.get(url);
    if !names("accept") {
        request = request.header("accept", "application/json");
    }
    if !names("user-agent") {
        request = request.header("user-agent", default_user_agent());
    }
    for (name, value) in headers {
        request = request.header(name, value);
    }
    let mut response = request.send().map_err(|e| LlmError::transport(&e))?;
    let status = response.status().as_u16();
    let mut body = String::new();
    (&mut response)
        .take(USAGE_BODY_MAX_BYTES)
        .read_to_string(&mut body)
        .map_err(|e| LlmError::body_read(&e))?;
    Ok((status, body))
}

/// [`get_body`] for a read that must succeed: a non-2xx answer is an
/// [`LlmError::Api`] carrying [`usage_advice`]'s sentence.
fn get_ok(url: &str, headers: &[(String, String)]) -> Result<String> {
    let (status, body) = get_body(url, headers)?;
    if (200..300).contains(&status) {
        Ok(body)
    } else {
        Err(LlmError::Api {
            status,
            body: usage_advice(status, &body),
        })
    }
}

/// The report for a Cline account with no ClinePass subscription.
fn clinepass_unsubscribed() -> UsageReport {
    UsageReport {
        notes: vec![UsageNote::new(
            NoteTone::Warning,
            "This Cline account has no ClinePass subscription — subscribe at \
             app.cline.bot/dashboard/subscription, or use Cline's usage-billed models through \
             an API key (/login → Use an API key → Cline).",
        )],
        dashboard: Some(CLINEPASS_DASHBOARD.to_string()),
        ..UsageReport::default()
    }
}

/// ChatGPT's usage, read from `url` with the credential's `headers`.
fn chatgpt_usage(url: &str, headers: &[(String, String)], now: i64) -> Result<UsageReport> {
    parse_chatgpt_usage(&get_ok(url, headers)?, now)
}

/// A Copilot seat's quota, read from `url` with the stored GitHub token.
fn copilot_usage(url: &str, oauth_token: &str) -> Result<UsageReport> {
    parse_copilot_user(&get_ok(url, &super::copilot::user_headers(oauth_token))?)
}

/// A ClinePass plan's windows: the plan first — the dashboard's own order,
/// since an account with no plan has no windows to read (Cline answers
/// `402`/`404`, or a plan of `null`) — then its usage limits.
fn clinepass_usage(
    plan_url: &str,
    limits_url: &str,
    headers: &[(String, String)],
) -> Result<UsageReport> {
    let (status, body) = get_body(plan_url, headers)?;
    if status == 402 || status == 404 {
        return Ok(clinepass_unsubscribed());
    }
    if !(200..300).contains(&status) {
        return Err(LlmError::Api {
            status,
            body: usage_advice(status, &body),
        });
    }
    let Some(plan) = parse_clinepass_plan(&body)? else {
        return Ok(clinepass_unsubscribed());
    };
    let mut report = parse_clinepass_limits(&get_ok(limits_url, headers)?)?;
    report.plan = Some(plan);
    Ok(report)
}

/// Read the usage of the subscription `cfg` signs in to (boundary — real
/// HTTP). Polls `cancel` before and after, so a page closed meanwhile leaves
/// nothing to deliver.
///
/// # Errors
/// A config that is not a subscription sign-in, a credential the provider
/// refuses, a transport failure, or an answer this parse cannot read.
pub fn fetch_usage(cfg: &ModelConfig, cancel: &CancelToken) -> Result<UsageReport> {
    if cancel.is_cancelled() {
        return Err(LlmError::Cancelled);
    }
    let report = match cfg.auth {
        // The same seam the chat request uses: the stored refresh token minted
        // (and cached) into a bearer, rotations written back, and the
        // account's own headers — `chatgpt-account-id`, the Codex user agent.
        AuthScheme::ChatGptCodex => {
            let auth = super::auth::request_auth(cfg)?;
            let mut headers = bearer(auth.bearer.as_deref())?;
            headers.extend(cfg.extra_headers.iter().cloned());
            headers.extend(auth.headers);
            let base = auth.base.as_deref().unwrap_or(&cfg.api_base);
            chatgpt_usage(&chatgpt_usage_url(base), &headers, unix_now())?
        }
        // The quota read takes the stored GitHub token itself, not the
        // Copilot bearer the seam would exchange it for.
        AuthScheme::GithubCopilot => copilot_usage(
            &super::copilot::user_url(),
            signed_in(cfg.api_key.as_deref())?,
        )?,
        AuthScheme::ClineAccount => {
            let auth = super::auth::request_auth(cfg)?;
            clinepass_usage(
                &super::cline::plan_url(),
                &super::cline::usage_limits_url(),
                &bearer(auth.bearer.as_deref())?,
            )?
        }
        // Nothing to read but who the usage bills to — which the mint names,
        // and which proves the sign-in still works.
        AuthScheme::AnthropicConsole => {
            let access = super::claude::authorize(signed_in(cfg.api_key.as_deref())?)?;
            console_report(access.organization)
        }
        AuthScheme::ApiKey | AuthScheme::OptionalKey => {
            return Err(refusal(format!(
                "{} bills per token through an API key — there is no subscription plan to \
                 report usage against",
                cfg.provider_name
            )));
        }
    };
    if cancel.is_cancelled() {
        return Err(LlmError::Cancelled);
    }
    Ok(report)
}

/// A failure this module words itself rather than one the provider answered —
/// an [`LlmError::Api`] with no status, whose message is the sentence alone.
fn refusal(message: impl Into<String>) -> LlmError {
    LlmError::Api {
        status: 0,
        body: message.into(),
    }
}

/// The stored credential, or the sentence for a provider never signed in to.
fn signed_in(key: Option<&str>) -> Result<&str> {
    key.map(str::trim)
        .filter(|key| !key.is_empty())
        .ok_or_else(|| refusal("not signed in — run /login and choose Use a subscription"))
}

/// The `Authorization` header for a minted bearer.
fn bearer(token: Option<&str>) -> Result<Vec<(String, String)>> {
    Ok(vec![(
        "authorization".to_string(),
        format!("Bearer {}", signed_in(token)?),
    )])
}

/// Now, in Unix seconds — where a window that names only its countdown
/// is placed.
fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| {
            i64::try_from(since.as_secs()).unwrap_or(i64::MAX)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stand-in for a provider's usage endpoint answering one request per
    /// entry of `answers`, in order, and handing back each request it was
    /// sent.
    fn serve(answers: Vec<(u16, &'static str)>) -> (String, std::thread::JoinHandle<Vec<String>>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let mut requests = Vec::new();
            for (status, body) in answers {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = Vec::new();
                let mut chunk = [0u8; 4096];
                while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                    let n = stream.read(&mut chunk).unwrap();
                    if n == 0 {
                        break;
                    }
                    request.extend_from_slice(&chunk[..n]);
                }
                let reply = format!(
                    "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(reply.as_bytes()).unwrap();
                requests.push(String::from_utf8_lossy(&request).to_ascii_lowercase());
            }
            requests
        });
        (base, server)
    }

    fn header(name: &str, value: &str) -> (String, String) {
        (name.to_string(), value.to_string())
    }

    #[test]
    fn a_usage_read_sends_its_headers_and_hands_back_the_answer() {
        let (base, server) = serve(vec![(200, r#"{"ok":true}"#)]);
        let (status, body) = get_body(
            &format!("{base}/wham/usage"),
            &[header("authorization", "Bearer t-1")],
        )
        .unwrap();
        assert_eq!((status, body.as_str()), (200, r#"{"ok":true}"#));
        let request = &server.join().unwrap()[0];
        assert!(request.starts_with("get /wham/usage "), "{request}");
        assert!(request.contains("authorization: bearer t-1"), "{request}");
        assert!(request.contains("accept: application/json"), "{request}");
        assert!(request.contains("user-agent: alter-zero/"), "{request}");
    }

    #[test]
    fn a_credential_s_own_user_agent_is_the_one_sent() {
        let (base, server) = serve(vec![(200, "{}")]);
        get_body(&base, &[header("User-Agent", "codex_cli_rs/1")]).unwrap();
        let request = &server.join().unwrap()[0];
        assert!(request.contains("user-agent: codex_cli_rs/1"), "{request}");
        assert!(!request.contains("alter-zero/"), "{request}");
    }

    #[test]
    fn a_refused_read_is_an_error_worded_for_what_to_do() {
        let (base, server) = serve(vec![(401, r#"{"message":"Bad credentials"}"#)]);
        let error = get_ok(&base, &[]).unwrap_err().to_string();
        assert!(error.contains("401"), "{error}");
        assert!(error.contains("/login"), "{error}");
        assert!(error.contains("Bad credentials"), "{error}");
        server.join().unwrap();
    }

    #[test]
    fn each_refusal_is_worded_and_an_html_page_never_becomes_the_reason() {
        assert!(usage_advice(429, "").contains("try again"));
        let html = usage_advice(503, "<html><body>Bad gateway</body></html>");
        assert!(!html.contains('<'), "{html}");
        assert!(!html.is_empty());
        assert_eq!(usage_advice(418, r#"{"error":"teapot"}"#), "teapot");
        assert!(usage_advice(404, r#"{"error":{"message":"nope"}}"#).contains("nope"));
        assert!(usage_advice(403, r#"{"detail":"Forbidden"}"#).contains("Forbidden"));
    }

    #[test]
    fn chatgpt_usage_reads_the_report_off_its_endpoint() {
        let (base, server) = serve(vec![(200, PRO_BODY)]);
        let report = chatgpt_usage(
            &format!("{base}/backend-api/wham/usage"),
            &[header("chatgpt-account-id", "acct-1")],
            1_793_000_000,
        )
        .unwrap();
        assert_eq!(report.plan.as_deref(), Some("Pro"));
        let request = &server.join().unwrap()[0];
        assert!(
            request.starts_with("get /backend-api/wham/usage "),
            "{request}"
        );
        assert!(request.contains("chatgpt-account-id: acct-1"), "{request}");
    }

    #[test]
    fn copilot_usage_presents_the_github_token_to_the_quota_read() {
        let (base, server) = serve(vec![(200, COPILOT_CREDITS)]);
        let report =
            copilot_usage(&super::super::copilot::user_url_for(&base), "ghu_test").unwrap();
        assert_eq!(report.plan.as_deref(), Some("Pro"));
        let request = &server.join().unwrap()[0];
        assert!(
            request.starts_with("get /copilot_internal/user "),
            "{request}"
        );
        assert!(
            request.contains("authorization: token ghu_test"),
            "{request}"
        );
    }

    #[test]
    fn clinepass_usage_reads_the_plan_then_its_windows() {
        let plan = r#"{"success":true,"data":{"plan":{"displayName":"Cline Pass (Monthly)"}}}"#;
        let (base, server) = serve(vec![(200, plan), (200, CLINEPASS_LIMITS)]);
        let report = clinepass_usage(
            &format!("{base}/api/v1/users/me/plan"),
            &format!("{base}/api/v1/users/me/plan/usage-limits"),
            &[header("authorization", "Bearer workos:jwt")],
        )
        .unwrap();
        assert_eq!(report.plan.as_deref(), Some("Cline Pass (Monthly)"));
        assert_eq!(report.meters.len(), 3);
        let requests = server.join().unwrap();
        assert!(
            requests[0].starts_with("get /api/v1/users/me/plan "),
            "{requests:?}"
        );
        assert!(
            requests[1].starts_with("get /api/v1/users/me/plan/usage-limits "),
            "{requests:?}"
        );
        assert!(requests[1].contains("authorization: bearer workos:jwt"));
    }

    #[test]
    fn a_cline_account_with_no_plan_is_told_so_without_reading_windows() {
        for answer in [
            (404, r#"{"success":false,"error":"no plan"}"#),
            (402, r#"{"success":false,"error":"payment required"}"#),
            (200, r#"{"success":true,"data":{"plan":null}}"#),
        ] {
            let (base, server) = serve(vec![answer]);
            let report = clinepass_usage(
                &format!("{base}/plan"),
                "http://127.0.0.1:1/never-asked",
                &[],
            )
            .unwrap();
            assert!(report.meters.is_empty(), "{answer:?}");
            assert_eq!(report.notes.len(), 1, "{answer:?}");
            assert!(report.notes[0].text.contains("no ClinePass subscription"));
            assert_eq!(report.dashboard.as_deref(), Some(CLINEPASS_DASHBOARD));
            assert_eq!(server.join().unwrap().len(), 1);
        }
    }

    #[test]
    fn a_pasted_key_has_no_plan_to_report() {
        let error = fetch_usage(&super::super::ModelConfig::fallback(), &CancelToken::new())
            .unwrap_err()
            .to_string();
        assert!(error.contains("subscription"), "{error}");
    }

    #[test]
    fn a_cancelled_read_never_starts() {
        let cancel = CancelToken::new();
        cancel.cancel();
        assert!(matches!(
            fetch_usage(&super::super::ModelConfig::fallback(), &cancel),
            Err(LlmError::Cancelled)
        ));
    }

    /// The live answer's shape for a Pro seat (identifiers replaced): one
    /// weekly window, a credit balance, two limit resets to redeem.
    const PRO_BODY: &str = r#"{
      "user_id": "user-1",
      "account_id": "acct-1",
      "email": "dev@example.com",
      "plan_type": "pro",
      "rate_limit": {
        "allowed": true,
        "limit_reached": false,
        "primary_window": {
          "used_percent": 7,
          "limit_window_seconds": 604800,
          "reset_after_seconds": 583983,
          "reset_at": 1793521139
        },
        "secondary_window": null
      },
      "code_review_rate_limit": null,
      "additional_rate_limits": null,
      "model_usage": {"gpt-6": {"available": true}},
      "credits": {
        "has_credits": true,
        "unlimited": false,
        "overage_limit_reached": false,
        "balance": "62500.0000000000",
        "approx_local_messages": [15625, 81250],
        "approx_cloud_messages": [2500, 15625]
      },
      "spend_control": {"reached": false, "individual_limit": null},
      "rate_limit_reached_type": null,
      "promo": null,
      "rate_limit_reset_credits": {"available_count": 2, "applicable_available_count": 0}
    }"#;

    fn limit(meter: &UsageMeter) -> &Limit {
        match &meter.value {
            MeterValue::Limit(limit) => limit,
            other => panic!("{} is not a limit: {other:?}", meter.label),
        }
    }

    fn text(meter: &UsageMeter) -> (&str, Option<&str>) {
        match &meter.value {
            MeterValue::Text { value, detail } => (value, detail.as_deref()),
            other => panic!("{} is not text: {other:?}", meter.label),
        }
    }

    #[test]
    fn a_window_within_five_percent_of_a_named_length_takes_its_name() {
        assert_eq!(window_label(18_000), "5-hour limit");
        assert_eq!(window_label(17_400), "5-hour limit");
        assert_eq!(window_label(86_400), "Daily limit");
        assert_eq!(window_label(604_800), "Weekly limit");
        assert_eq!(window_label(2_592_000), "Monthly limit");
        assert_eq!(window_label(31_536_000), "Annual limit");
    }

    #[test]
    fn any_other_window_is_named_by_its_own_length() {
        assert_eq!(window_label(3_600), "1-hour limit");
        assert_eq!(window_label(7_200), "2-hour limit");
        assert_eq!(window_label(1_800), "30-minute limit");
        assert_eq!(window_label(172_800), "2-day limit");
        assert_eq!(window_label(90), "2-minute limit");
    }

    #[test]
    fn a_window_with_no_length_is_a_usage_limit() {
        assert_eq!(window_label(0), "Usage limit");
        assert_eq!(window_label(-5), "Usage limit");
    }

    #[test]
    fn thousands_are_grouped_and_fractions_rounded() {
        assert_eq!(group_thousands(0.0), "0");
        assert_eq!(group_thousands(999.0), "999");
        assert_eq!(group_thousands(62_500.0), "62,500");
        assert_eq!(group_thousands(1_234_567.4), "1,234,567");
        assert_eq!(group_thousands(12.5), "13");
    }

    #[test]
    fn the_usage_url_sits_on_the_backend_root_not_the_chat_base() {
        assert_eq!(
            chatgpt_usage_url("https://chatgpt.com/backend-api/codex"),
            "https://chatgpt.com/backend-api/wham/usage"
        );
        assert_eq!(
            chatgpt_usage_url("https://chatgpt.com/backend-api/codex/"),
            "https://chatgpt.com/backend-api/wham/usage"
        );
        assert_eq!(
            chatgpt_usage_url("https://chatgpt.com"),
            "https://chatgpt.com/backend-api/wham/usage"
        );
    }

    #[test]
    fn a_base_that_is_not_the_chatgpt_backend_is_asked_codex_style() {
        assert_eq!(
            chatgpt_usage_url("http://127.0.0.1:4100"),
            "http://127.0.0.1:4100/api/codex/usage"
        );
        assert_eq!(
            chatgpt_usage_url("http://127.0.0.1:4100/codex/"),
            "http://127.0.0.1:4100/api/codex/usage"
        );
    }

    #[test]
    fn a_pro_seat_reports_its_plan_window_credits_and_resets() {
        let report = parse_chatgpt_usage(PRO_BODY, 1_793_000_000).unwrap();
        assert_eq!(report.plan.as_deref(), Some("Pro"));
        assert_eq!(report.account.as_deref(), Some("dev@example.com"));
        assert_eq!(report.dashboard.as_deref(), Some(CHATGPT_DASHBOARD));
        assert!(report.notes.is_empty(), "{:?}", report.notes);
        let labels: Vec<&str> = report.meters.iter().map(|m| m.label.as_str()).collect();
        assert_eq!(labels, ["Weekly limit", "Credits", "Limit resets"]);
        let weekly = limit(&report.meters[0]);
        assert_eq!(weekly.used_percent, 7.0);
        assert_eq!(weekly.resets_at, Some(1_793_521_139));
        assert_eq!(weekly.counts, None);
        assert_eq!(report.meters[0].group, None);
        assert_eq!(
            text(&report.meters[1]),
            ("62,500 credits", Some("≈ 15.6k–81.3k local messages"))
        );
        assert_eq!(text(&report.meters[2]), ("2 available", None));
    }

    #[test]
    fn two_windows_read_primary_then_secondary() {
        let body = r#"{"plan_type":"plus","rate_limit":{"allowed":true,"limit_reached":false,
            "primary_window":{"used_percent":42,"limit_window_seconds":18000,"reset_after_seconds":120,"reset_at":1000120},
            "secondary_window":{"used_percent":5.5,"limit_window_seconds":604800,"reset_after_seconds":43200,"reset_at":1043200}}}"#;
        let report = parse_chatgpt_usage(body, 1_000_000).unwrap();
        assert_eq!(report.plan.as_deref(), Some("Plus"));
        let labels: Vec<&str> = report.meters.iter().map(|m| m.label.as_str()).collect();
        assert_eq!(labels, ["5-hour limit", "Weekly limit"]);
        assert_eq!(limit(&report.meters[0]).used_percent, 42.0);
        assert_eq!(limit(&report.meters[1]).used_percent, 5.5);
        assert_eq!(limit(&report.meters[1]).resets_at, Some(1_043_200));
    }

    #[test]
    fn a_window_with_no_reset_time_is_placed_by_its_countdown() {
        let body = r#"{"plan_type":"plus","rate_limit":{"primary_window":
            {"used_percent":10,"limit_window_seconds":18000,"reset_after_seconds":600}}}"#;
        let report = parse_chatgpt_usage(body, 2_000).unwrap();
        assert_eq!(limit(&report.meters[0]).resets_at, Some(2_600));
    }

    #[test]
    fn per_model_limits_follow_the_plans_own_under_their_names() {
        let body = r#"{"plan_type":"pro","rate_limit":{"primary_window":
              {"used_percent":1,"limit_window_seconds":18000,"reset_at":5}},
            "additional_rate_limits":[{"limit_name":"GPT-5.3-Codex-Spark","metered_feature":"codex_spark",
              "rate_limit":{"allowed":true,"limit_reached":false,
                "primary_window":{"used_percent":88,"limit_window_seconds":18000,"reset_at":9},
                "secondary_window":{"used_percent":20,"limit_window_seconds":604800,"reset_at":99}}},
              {"metered_feature":"codex_other","rate_limit":{"primary_window":
                {"used_percent":3,"limit_window_seconds":1800,"reset_at":7}}}]}"#;
        let report = parse_chatgpt_usage(body, 0).unwrap();
        let rows: Vec<(Option<&str>, &str)> = report
            .meters
            .iter()
            .map(|m| (m.group.as_deref(), m.label.as_str()))
            .collect();
        assert_eq!(
            rows,
            [
                (None, "5-hour limit"),
                (Some("GPT-5.3-Codex-Spark"), "5-hour limit"),
                (Some("GPT-5.3-Codex-Spark"), "Weekly limit"),
                (Some("codex_other"), "30-minute limit"),
            ]
        );
        assert_eq!(limit(&report.meters[1]).used_percent, 88.0);
    }

    #[test]
    fn a_code_review_limit_is_its_own_group() {
        let body = r#"{"plan_type":"pro","code_review_rate_limit":{"allowed":true,"limit_reached":false,
            "primary_window":{"used_percent":12,"limit_window_seconds":604800,"reset_at":5}}}"#;
        let report = parse_chatgpt_usage(body, 0).unwrap();
        assert_eq!(report.meters.len(), 1);
        assert_eq!(report.meters[0].group.as_deref(), Some("Code review"));
        assert_eq!(report.meters[0].label, "Weekly limit");
    }

    #[test]
    fn a_workspace_credit_limit_counts_credits_used_of_the_limit() {
        let body = r#"{"plan_type":"team","spend_control":{"reached":false,"individual_limit":{
            "source":"workspace_spend_controls","limit":"25000","used":"8000","remaining":"17000",
            "used_percent":32,"remaining_percent":68,"reset_after_seconds":43200,"reset_at":1043200}}}"#;
        let report = parse_chatgpt_usage(body, 0).unwrap();
        assert_eq!(report.meters.len(), 1);
        assert_eq!(report.meters[0].label, "Monthly credit limit");
        let credit = limit(&report.meters[0]);
        assert_eq!(credit.used_percent, 32.0);
        assert_eq!(
            credit.counts,
            Some(Counts {
                used: 8_000.0,
                total: 25_000.0,
                unit: Some("credits"),
            })
        );
        assert_eq!(credit.resets_at, Some(1_043_200));
    }

    #[test]
    fn unlimited_credits_say_so_and_none_at_all_are_left_out() {
        let unlimited =
            r#"{"plan_type":"pro","credits":{"has_credits":true,"unlimited":true,"balance":null}}"#;
        let report = parse_chatgpt_usage(unlimited, 0).unwrap();
        assert_eq!(report.meters.len(), 1);
        assert_eq!(report.meters[0].label, "Credits");
        assert_eq!(report.meters[0].value, MeterValue::Unlimited);

        let none = r#"{"plan_type":"plus","credits":{"has_credits":false,"unlimited":false,"balance":"0"}}"#;
        assert!(parse_chatgpt_usage(none, 0).unwrap().meters.is_empty());

        let unknown_balance =
            r#"{"plan_type":"plus","credits":{"has_credits":true,"unlimited":false}}"#;
        let report = parse_chatgpt_usage(unknown_balance, 0).unwrap();
        assert_eq!(text(&report.meters[0]), ("Available", None));
    }

    #[test]
    fn no_resets_to_redeem_leaves_the_row_out() {
        let body = r#"{"plan_type":"plus","rate_limit_reset_credits":{"available_count":0}}"#;
        assert!(parse_chatgpt_usage(body, 0).unwrap().meters.is_empty());
        let one = r#"{"plan_type":"plus","rate_limit_reset_credits":{"available_count":1}}"#;
        let report = parse_chatgpt_usage(one, 0).unwrap();
        assert_eq!(text(&report.meters[0]), ("1 available", None));
    }

    #[test]
    fn a_reached_limit_is_a_red_note_worded_for_who_can_lift_it() {
        let personal = r#"{"plan_type":"plus","rate_limit":{"allowed":false,"limit_reached":true,
            "primary_window":{"used_percent":100,"limit_window_seconds":18000,"reset_at":9}}}"#;
        let report = parse_chatgpt_usage(personal, 0).unwrap();
        assert_eq!(
            report.notes,
            [UsageNote::new(
                NoteTone::Error,
                "You've hit your usage limit."
            )]
        );

        let member = r#"{"plan_type":"team","rate_limit":{"allowed":false,"limit_reached":true},
            "rate_limit_reached_type":{"type":"workspace_member_credits_depleted"}}"#;
        let report = parse_chatgpt_usage(member, 0).unwrap();
        assert_eq!(
            report.notes,
            [UsageNote::new(
                NoteTone::Error,
                "Your workspace is out of credits. Ask your workspace owner to refill in order to continue."
            )]
        );

        let cap =
            r#"{"plan_type":"team","spend_control":{"reached":true,"individual_limit":null}}"#;
        let report = parse_chatgpt_usage(cap, 0).unwrap();
        assert_eq!(report.notes.len(), 1);
        assert_eq!(report.notes[0].tone, NoteTone::Error);
        assert!(
            report.notes[0].text.contains("spend cap"),
            "{:?}",
            report.notes
        );
    }

    #[test]
    fn a_section_this_parse_cannot_read_costs_only_itself() {
        let body = r#"{"plan_type":"plus",
            "rate_limit":{"primary_window":"soon"},
            "credits":{"has_credits":true,"unlimited":false,"balance":"10"},
            "additional_rate_limits":{"not":"a list"}}"#;
        let report = parse_chatgpt_usage(body, 0).unwrap();
        let labels: Vec<&str> = report.meters.iter().map(|m| m.label.as_str()).collect();
        assert_eq!(labels, ["Credits"]);
        assert_eq!(text(&report.meters[0]), ("10 credits", None));
    }

    #[test]
    fn a_body_that_is_not_an_object_is_a_decode_error() {
        assert!(matches!(
            parse_chatgpt_usage("<html>blocked</html>", 0),
            Err(LlmError::Decode(_))
        ));
        assert!(matches!(
            parse_chatgpt_usage("[1, 2]", 0),
            Err(LlmError::Decode(_))
        ));
    }

    // --- ClinePass ---

    /// The live usage-limits answer's shape: no 5-hour window running, the
    /// week spent, most of the month used.
    const CLINEPASS_LIMITS: &str = r#"{"data":{"limits":[
        {"type":"five_hour","percentUsed":0},
        {"type":"weekly","percentUsed":100,"resetsAt":"2026-10-27T14:36:37.287853835Z"},
        {"type":"monthly","percentUsed":81,"resetsAt":"2026-11-12T13:26:58.291096578Z"}]},
      "success":true}"#;

    #[test]
    fn clinepass_reports_its_three_windows_in_the_dashboards_order() {
        let report = parse_clinepass_limits(CLINEPASS_LIMITS).unwrap();
        let labels: Vec<&str> = report.meters.iter().map(|m| m.label.as_str()).collect();
        assert_eq!(labels, ["5-hour limit", "Weekly limit", "Monthly limit"]);
        let five = limit(&report.meters[0]);
        assert_eq!((five.used_percent, five.resets_at), (0.0, None));
        let weekly = limit(&report.meters[1]);
        assert_eq!(
            (weekly.used_percent, weekly.resets_at),
            (100.0, Some(1_793_111_797))
        );
        let monthly = limit(&report.meters[2]);
        assert_eq!(
            (monthly.used_percent, monthly.resets_at),
            (81.0, Some(1_794_490_018))
        );
        assert_eq!(report.dashboard.as_deref(), Some(CLINEPASS_DASHBOARD));
    }

    #[test]
    fn a_spent_clinepass_window_says_requests_are_paused_until_it_resets() {
        let report = parse_clinepass_limits(CLINEPASS_LIMITS).unwrap();
        assert_eq!(report.notes.len(), 1, "{:?}", report.notes);
        assert_eq!(report.notes[0].tone, NoteTone::Error);
        assert!(
            report.notes[0]
                .text
                .starts_with("Your weekly limit has been reached."),
            "{:?}",
            report.notes
        );
    }

    #[test]
    fn of_two_spent_windows_the_note_names_the_one_that_resets_last() {
        let body = r#"{"success":true,"data":{"limits":[
            {"type":"five_hour","percentUsed":100,"resetsAt":"2026-10-25T19:00:00Z"},
            {"type":"weekly","percentUsed":104.5,"resetsAt":"2026-10-27T14:36:37Z"}]}}"#;
        let report = parse_clinepass_limits(body).unwrap();
        assert_eq!(report.notes.len(), 1);
        assert!(report.notes[0].text.starts_with("Your weekly limit"));
        assert_eq!(limit(&report.meters[1]).used_percent, 104.5);
    }

    #[test]
    fn an_unknown_clinepass_window_is_named_from_its_type() {
        let body = r#"{"success":true,"data":{"limits":[{"type":"two_day","percentUsed":3}]}}"#;
        let report = parse_clinepass_limits(body).unwrap();
        assert_eq!(report.meters[0].label, "Two day limit");
        assert!(report.notes.is_empty());
    }

    #[test]
    fn a_clinepass_answer_without_its_limits_is_a_decode_error() {
        for body in [
            "<html></html>",
            r#"{"success":false,"error":"unauthorized"}"#,
            r#"{"success":true,"data":{}}"#,
        ] {
            assert!(
                matches!(parse_clinepass_limits(body), Err(LlmError::Decode(_))),
                "{body}"
            );
        }
    }

    #[test]
    fn the_clinepass_plan_is_named_by_its_display_name() {
        let body = r#"{"data":{"planHistoryId":"iph-1","plan":{"id":"pln-1",
            "name":"Cline Pass (Monthly)[Internal]","displayName":"Cline Pass (Monthly)",
            "interval":"Monthly"},"currentPeriodEnd":"2026-11-13T13:24:06Z"},"success":true}"#;
        assert_eq!(
            parse_clinepass_plan(body).unwrap().as_deref(),
            Some("Cline Pass (Monthly)")
        );
        let internal = r#"{"success":true,"data":{"plan":{"name":"Cline Pass"}}}"#;
        assert_eq!(
            parse_clinepass_plan(internal).unwrap().as_deref(),
            Some("Cline Pass")
        );
        let nameless = r#"{"success":true,"data":{"plan":{"id":"pln-1"}}}"#;
        assert_eq!(
            parse_clinepass_plan(nameless).unwrap().as_deref(),
            Some("ClinePass")
        );
        assert_eq!(
            parse_clinepass_plan(r#"{"success":true,"data":{"plan":null}}"#).unwrap(),
            None
        );
        assert!(parse_clinepass_plan("not json").is_err());
    }

    // --- GitHub Copilot ---

    /// A Pro seat after GitHub's move to AI Credits: the premium allowance is
    /// credits, chat and completions unlimited.
    const COPILOT_CREDITS: &str = r#"{"access_type_sku":"monthly_subscriber_quota",
      "copilot_plan":"individual","chat_enabled":true,"token_based_billing":true,
      "quota_reset_date":"2026-11-01","quota_reset_date_utc":"2026-11-01T00:00:00.000Z",
      "quota_snapshots":{
        "chat":{"quota_id":"chat","entitlement":"0","remaining":0,"percent_remaining":100,
          "unlimited":true,"overage_count":0,"overage_entitlement":0,"overage_permitted":false},
        "completions":{"quota_id":"completions","entitlement":"0","remaining":0,
          "percent_remaining":100,"unlimited":true,"overage_count":0,"overage_permitted":false},
        "premium_interactions":{"quota_id":"premium_interactions","entitlement":"1000",
          "remaining":580,"quota_remaining":580,"percent_remaining":58.0,"credits_used":420,
          "unlimited":false,"overage_count":0,"overage_entitlement":0,"overage_permitted":false,
          "quota_reset_at":1793491200}}}"#;

    #[test]
    fn a_credit_billed_copilot_seat_meters_its_credits_first() {
        let report = parse_copilot_user(COPILOT_CREDITS).unwrap();
        assert_eq!(report.plan.as_deref(), Some("Pro"));
        assert_eq!(report.dashboard.as_deref(), Some(COPILOT_DASHBOARD));
        let labels: Vec<&str> = report.meters.iter().map(|m| m.label.as_str()).collect();
        assert_eq!(labels, ["Credits", "Chat messages", "Inline suggestions"]);
        let credits = limit(&report.meters[0]);
        assert_eq!(credits.used_percent, 42.0);
        assert_eq!(
            credits.counts,
            Some(Counts {
                used: 420.0,
                total: 1_000.0,
                unit: None,
            })
        );
        assert_eq!(credits.resets_at, Some(1_793_491_200));
        assert_eq!(report.meters[1].value, MeterValue::Unlimited);
        assert_eq!(report.meters[2].value, MeterValue::Unlimited);
        assert!(report.notes.is_empty(), "{:?}", report.notes);
    }

    #[test]
    fn a_premium_request_seat_counts_requests_and_resets_on_the_first() {
        let body = r#"{"copilot_plan":"individual_pro","quota_reset_date":"2026-11-01",
          "quota_snapshots":{"premium_interactions":{"entitlement":1500,"remaining":870,
            "percent_remaining":58.0,"unlimited":false,"overage_count":0,"overage_permitted":true},
            "chat":{"entitlement":0,"remaining":0,"percent_remaining":100,"unlimited":true}}}"#;
        let report = parse_copilot_user(body).unwrap();
        assert_eq!(report.plan.as_deref(), Some("Pro+"));
        let labels: Vec<&str> = report.meters.iter().map(|m| m.label.as_str()).collect();
        assert_eq!(labels, ["Premium requests", "Chat messages"]);
        let premium = limit(&report.meters[0]);
        assert_eq!(premium.used_percent, 42.0);
        assert_eq!(
            premium.counts.as_ref().map(|c| (c.used, c.total)),
            Some((630.0, 1_500.0))
        );
        assert_eq!(premium.resets_at, Some(1_793_491_200));
    }

    #[test]
    fn a_zero_entitlement_placeholder_is_not_a_meter() {
        let body = r#"{"copilot_plan":"business","token_based_billing":true,
          "quota_snapshots":{"premium_interactions":{"entitlement":"0","remaining":0,
            "percent_remaining":100,"unlimited":false,"overage_count":0,"overage_permitted":false},
            "chat":{"entitlement":"0","percent_remaining":100,"unlimited":true}}}"#;
        let report = parse_copilot_user(body).unwrap();
        assert_eq!(report.plan.as_deref(), Some("Business"));
        let labels: Vec<&str> = report.meters.iter().map(|m| m.label.as_str()).collect();
        assert_eq!(labels, ["Chat messages"]);
    }

    #[test]
    fn a_spent_allowance_is_red_unless_overage_is_allowed() {
        let spent = r#"{"copilot_plan":"individual","quota_reset_date":"2026-11-01",
          "quota_snapshots":{"premium_interactions":{"entitlement":300,"remaining":0,
            "percent_remaining":0,"unlimited":false,"overage_count":0,"overage_permitted":false}}}"#;
        let report = parse_copilot_user(spent).unwrap();
        assert_eq!(limit(&report.meters[0]).used_percent, 100.0);
        assert_eq!(report.notes.len(), 1);
        assert_eq!(report.notes[0].tone, NoteTone::Error);
        assert!(
            report.notes[0].text.contains("used up"),
            "{:?}",
            report.notes
        );

        let overage = r#"{"copilot_plan":"individual",
          "quota_snapshots":{"premium_interactions":{"entitlement":300,"remaining":0,
            "percent_remaining":0,"unlimited":false,"overage_count":42,"overage_permitted":true}}}"#;
        let report = parse_copilot_user(overage).unwrap();
        assert_eq!(report.notes.len(), 1);
        assert_eq!(report.notes[0].tone, NoteTone::Info);
        assert!(report.notes[0].text.contains("42"), "{:?}", report.notes);
    }

    #[test]
    fn a_free_seat_on_the_old_shape_meters_its_monthly_quotas() {
        let body = r#"{"copilot_plan":"free","access_type_sku":"free_limited_copilot",
          "limited_user_quotas":{"chat":42,"completions":1800},
          "monthly_quotas":{"chat":50,"completions":2000},
          "limited_user_reset_date":"2026-11-01"}"#;
        let report = parse_copilot_user(body).unwrap();
        assert_eq!(report.plan.as_deref(), Some("Free"));
        let labels: Vec<&str> = report.meters.iter().map(|m| m.label.as_str()).collect();
        assert_eq!(labels, ["Chat messages", "Inline suggestions"]);
        let chat = limit(&report.meters[0]);
        assert_eq!(chat.used_percent, 16.0);
        assert_eq!(
            chat.counts.as_ref().map(|c| (c.used, c.total)),
            Some((8.0, 50.0))
        );
        assert_eq!(chat.resets_at, Some(1_793_491_200));
        assert_eq!(limit(&report.meters[1]).used_percent, 10.0);
    }

    #[test]
    fn the_premium_models_alias_stands_in_for_premium_interactions() {
        let body = r#"{"copilot_plan":"individual","quota_snapshots":{"premium_models":
            {"entitlement":300,"remaining":150,"percent_remaining":50,"unlimited":false}}}"#;
        let report = parse_copilot_user(body).unwrap();
        assert_eq!(report.meters[0].label, "Premium requests");
        assert_eq!(limit(&report.meters[0]).used_percent, 50.0);
    }

    #[test]
    fn copilot_plans_read_like_names() {
        let plan = |slug: &str| {
            parse_copilot_user(&format!(r#"{{"copilot_plan":"{slug}"}}"#))
                .unwrap()
                .plan
        };
        assert_eq!(plan("free").as_deref(), Some("Free"));
        assert_eq!(plan("individual").as_deref(), Some("Pro"));
        assert_eq!(plan("individual_pro").as_deref(), Some("Pro+"));
        assert_eq!(plan("individual_max").as_deref(), Some("Max"));
        assert_eq!(plan("individual_edu").as_deref(), Some("Student"));
        assert_eq!(plan("business").as_deref(), Some("Business"));
        assert_eq!(plan("enterprise").as_deref(), Some("Enterprise"));
        assert_eq!(plan("team_plus").as_deref(), Some("Team Plus"));
    }

    #[test]
    fn a_copilot_answer_that_is_not_an_object_is_a_decode_error() {
        assert!(matches!(
            parse_copilot_user("Not Found"),
            Err(LlmError::Decode(_))
        ));
    }

    // --- Anthropic Console ---

    #[test]
    fn a_console_sign_in_reports_its_organisation_and_no_plan_limits() {
        let report = console_report(Some("Acme Labs".to_string()));
        assert_eq!(report.account.as_deref(), Some("Acme Labs"));
        assert_eq!(report.plan, None);
        assert!(report.meters.is_empty());
        assert_eq!(report.notes.len(), 1);
        assert_eq!(report.notes[0].tone, NoteTone::Info);
        assert!(
            report.notes[0].text.contains("per token"),
            "{:?}",
            report.notes
        );
        assert_eq!(report.dashboard.as_deref(), Some(CONSOLE_DASHBOARD));
    }

    #[test]
    fn a_seat_with_no_plan_or_email_still_reports_its_windows() {
        let body = r#"{"rate_limit":{"primary_window":{"used_percent":3,"limit_window_seconds":18000,"reset_at":5}}}"#;
        let report = parse_chatgpt_usage(body, 0).unwrap();
        assert_eq!(report.plan, None);
        assert_eq!(report.account, None);
        assert_eq!(report.meters.len(), 1);
    }
}
