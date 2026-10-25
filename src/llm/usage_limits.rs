//! Subscription usage limits — what `/usage` shows. See
//! `docs/usage-limits.md`.
//!
//! A pasted key is billed per token; a subscription is a set of **limits**
//! — a 5-hour window, a week, a month, a monthly allowance of premium
//! requests — each with a share used and a moment it resets. Every
//! subscription provider answers that question on an endpoint of its own,
//! in a shape of its own, and this module turns each answer into one
//! [`PlanLimits`] the page can lay out without knowing whose it was.
//!
//! The pure half is the parsers ([`parse_chatgpt_usage`],
//! [`parse_copilot_usage`], [`parse_clinepass_usage`]), the window naming
//! ([`window_label`]) and the URL rules; the boundary half is
//! [`fetch_plan_limits`], which authenticates through the seam every request
//! already uses (`auth::request_auth`) and reads the answer.
//!
//! Every answer is a few kilobytes of fixed shape, so each is read capped
//! ([`USAGE_BODY_MAX_BYTES`]) straight into typed structs — never a `Value`
//! tree (`docs/memory.md`) — and every number and string goes through a
//! lenient reader (`Num`, `Text`, `Moment`): a provider that starts
//! sending a count as a string, or a share as `null`, costs that one value
//! rather than the whole answer.

use std::fmt;
use std::io::Read;
use std::time::Duration;

use serde::Deserialize;
use serde::de::{self, Deserializer, IgnoredAny, MapAccess, SeqAccess, Visitor};

use super::config::{AuthScheme, ModelConfig};
use super::{LlmError, Result, auth, chatgpt, cline, copilot};
use crate::stream::CancelToken;

/// The most a usage answer is read. Every one is a few kilobytes; the cap
/// bounds what an intercepting proxy's HTML page can put on the screen as a
/// "reason" (the `copilot::BODY_MAX_BYTES` posture).
pub const USAGE_BODY_MAX_BYTES: u64 = 64 * 1024;

/// Per-operation deadline for a usage read — a small JSON round trip the
/// user is watching a page for (the sign-in flows' own thirty seconds, so
/// the two share one cached client).
const USAGE_TIMEOUT: Duration = Duration::from_secs(30);

/// One signed-in subscription's limits, translated from its provider's own
/// answer.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PlanLimits {
    /// The plan the account is on, as a label: `Pro`, `Pro+`, `Monthly`.
    pub plan: Option<String>,
    /// The metered windows, in the order the page lists them.
    pub windows: Vec<LimitWindow>,
    /// What the plan holds that is not a metered window: a credit balance,
    /// an allowance the plan does not count.
    pub notes: Vec<LimitNote>,
    /// A refusal in force that no window explains — a workspace out of
    /// credits, an organization's budget spent. Shown in red.
    pub alert: Option<String>,
    /// A sentence in place of windows when there is nothing to meter — a
    /// pay-as-you-go sign-in. Shown dim.
    pub remark: Option<String>,
}

/// One metered window: how much of it is used and when it comes back.
#[derive(Debug, Clone, PartialEq)]
pub struct LimitWindow {
    /// What the window meters: `5-hour limit`, `Weekly limit`, `Premium
    /// requests`.
    pub label: String,
    /// The share used, as a percentage — `0.0` up, as the provider
    /// reported it (a share past the allowance can exceed 100).
    pub used_percent: f64,
    /// When the window resets, as Unix seconds — `None` when the provider
    /// names no reset (an unused rolling window).
    pub resets_at: Option<i64>,
    /// The counts behind the share, when the provider gives them: `148 of
    /// 300 requests`.
    pub detail: Option<String>,
    /// Is the window used up — requests refused (or billed) until it
    /// resets?
    pub reached: bool,
}

/// What a plan holds that is not a window: `Credits · 62,500 credits`,
/// `Chat · Included`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LimitNote {
    /// What the note is about.
    pub label: String,
    /// What it says.
    pub value: String,
    /// A quieter clause after the value: what a balance buys.
    pub detail: Option<String>,
}

// ---------------------------------------------------------------------------
// Window names (pure)
// ---------------------------------------------------------------------------

const HOUR: i64 = 3600;
const DAY: i64 = 24 * HOUR;

/// The name a window of `seconds` goes by — Codex's rule, the length read off
/// the window rather than assumed: within 5% of five hours is the `5-hour
/// limit`, of a day the `Daily limit`, of a week the `Weekly limit`, of a
/// year the `Annual limit`, and any calendar month's length (28 to 31 days)
/// the `Monthly limit`. Any other length names itself — `30-minute limit`,
/// `3-day limit` — a part minute rounding up rather than reading as a
/// shorter window. A window of no length is a `Usage limit`.
#[must_use]
pub fn window_label(seconds: i64) -> String {
    if seconds <= 0 {
        return "Usage limit".to_string();
    }
    let near = |nominal: i64| (seconds - nominal).abs() * 20 <= nominal;
    if near(5 * HOUR) {
        return "5-hour limit".to_string();
    }
    if near(DAY) {
        return "Daily limit".to_string();
    }
    if near(7 * DAY) {
        return "Weekly limit".to_string();
    }
    if (28 * DAY..=31 * DAY).contains(&seconds) {
        return "Monthly limit".to_string();
    }
    if near(365 * DAY) {
        return "Annual limit".to_string();
    }
    let minutes = (seconds + 59) / 60;
    if minutes % (24 * 60) == 0 {
        format!("{}-day limit", minutes / (24 * 60))
    } else if minutes % 60 == 0 {
        format!("{}-hour limit", minutes / 60)
    } else {
        format!("{minutes}-minute limit")
    }
}

// ---------------------------------------------------------------------------
// The lenient readers (pure)
// ---------------------------------------------------------------------------

/// A JSON number — or a numeric string — and anything else read as absent.
#[derive(Debug, Clone, Copy, Default)]
struct Num(Option<f64>);

impl<'de> Deserialize<'de> for Num {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        struct NumVisitor;
        impl<'de> Visitor<'de> for NumVisitor {
            type Value = Num;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a number")
            }
            fn visit_f64<E: de::Error>(self, v: f64) -> std::result::Result<Num, E> {
                Ok(Num(v.is_finite().then_some(v)))
            }
            #[allow(clippy::cast_precision_loss)] // a count or a share, never near 2^53
            fn visit_i64<E: de::Error>(self, v: i64) -> std::result::Result<Num, E> {
                Ok(Num(Some(v as f64)))
            }
            #[allow(clippy::cast_precision_loss)] // as above
            fn visit_u64<E: de::Error>(self, v: u64) -> std::result::Result<Num, E> {
                Ok(Num(Some(v as f64)))
            }
            fn visit_str<E: de::Error>(self, v: &str) -> std::result::Result<Num, E> {
                Ok(Num(v.trim().parse::<f64>().ok().filter(|n| n.is_finite())))
            }
            fn visit_bool<E: de::Error>(self, _: bool) -> std::result::Result<Num, E> {
                Ok(Num(None))
            }
            fn visit_unit<E: de::Error>(self) -> std::result::Result<Num, E> {
                Ok(Num(None))
            }
            fn visit_none<E: de::Error>(self) -> std::result::Result<Num, E> {
                Ok(Num(None))
            }
            fn visit_some<D2: Deserializer<'de>>(
                self,
                deserializer: D2,
            ) -> std::result::Result<Num, D2::Error> {
                deserializer.deserialize_any(NumVisitor)
            }
            fn visit_seq<A: SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> std::result::Result<Num, A::Error> {
                while seq.next_element::<IgnoredAny>()?.is_some() {}
                Ok(Num(None))
            }
            fn visit_map<A: MapAccess<'de>>(
                self,
                mut map: A,
            ) -> std::result::Result<Num, A::Error> {
                while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
                Ok(Num(None))
            }
        }
        deserializer.deserialize_any(NumVisitor)
    }
}

/// A JSON string, trimmed and non-empty — anything else read as absent.
#[derive(Debug, Clone, Default)]
struct Text(Option<String>);

impl Text {
    fn as_deref(&self) -> Option<&str> {
        self.0.as_deref()
    }
}

impl<'de> Deserialize<'de> for Text {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        struct TextVisitor;
        impl<'de> Visitor<'de> for TextVisitor {
            type Value = Text;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a string")
            }
            fn visit_str<E: de::Error>(self, v: &str) -> std::result::Result<Text, E> {
                let v = v.trim();
                Ok(Text((!v.is_empty()).then(|| v.to_string())))
            }
            fn visit_bool<E: de::Error>(self, _: bool) -> std::result::Result<Text, E> {
                Ok(Text(None))
            }
            fn visit_i64<E: de::Error>(self, _: i64) -> std::result::Result<Text, E> {
                Ok(Text(None))
            }
            fn visit_u64<E: de::Error>(self, _: u64) -> std::result::Result<Text, E> {
                Ok(Text(None))
            }
            fn visit_f64<E: de::Error>(self, _: f64) -> std::result::Result<Text, E> {
                Ok(Text(None))
            }
            fn visit_unit<E: de::Error>(self) -> std::result::Result<Text, E> {
                Ok(Text(None))
            }
            fn visit_none<E: de::Error>(self) -> std::result::Result<Text, E> {
                Ok(Text(None))
            }
            fn visit_some<D2: Deserializer<'de>>(
                self,
                deserializer: D2,
            ) -> std::result::Result<Text, D2::Error> {
                deserializer.deserialize_any(TextVisitor)
            }
            fn visit_seq<A: SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> std::result::Result<Text, A::Error> {
                while seq.next_element::<IgnoredAny>()?.is_some() {}
                Ok(Text(None))
            }
            fn visit_map<A: MapAccess<'de>>(
                self,
                mut map: A,
            ) -> std::result::Result<Text, A::Error> {
                while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
                Ok(Text(None))
            }
        }
        deserializer.deserialize_any(TextVisitor)
    }
}

/// A moment, as Unix seconds — sent as a number (`reset_at`), an RFC 3339
/// stamp (`resetsAt`, `quota_reset_date_utc`) or a bare date
/// (`quota_reset_date`, the start of that day, UTC). Zero, a negative, or
/// anything unparseable is no moment at all.
#[derive(Debug, Clone, Copy, Default)]
struct Moment(Option<i64>);

impl<'de> Deserialize<'de> for Moment {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        // A number is a Unix time already; a string is parsed.
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Number(f64),
            Text(String),
            Other(IgnoredAny),
        }
        #[allow(clippy::cast_possible_truncation)] // whole seconds, in range below
        let moment = match Raw::deserialize(deserializer)? {
            Raw::Number(n) if n.is_finite() && n > 0.0 && n < 1e13 => Some(n.trunc() as i64),
            Raw::Text(text) => parse_moment(&text),
            _ => None,
        };
        Ok(Moment(moment))
    }
}

/// An RFC 3339 stamp or a `YYYY-MM-DD` date (its midnight, UTC) as Unix
/// seconds.
fn parse_moment(text: &str) -> Option<i64> {
    let text = text.trim();
    if let Ok(stamp) = chrono::DateTime::parse_from_rfc3339(text) {
        return Some(stamp.timestamp()).filter(|t| *t > 0);
    }
    chrono::NaiveDate::parse_from_str(text, "%Y-%m-%d")
        .ok()
        .and_then(|date| date.and_hms_opt(0, 0, 0))
        .map(|midnight| midnight.and_utc().timestamp())
        .filter(|t| *t > 0)
}

/// Parse a body that must be a JSON object — serde would otherwise read an
/// array positionally into a struct whose every field defaults, and call a
/// `[]` an answer.
fn decode<T: for<'de> Deserialize<'de>>(body: &str) -> Result<T> {
    if !body.trim_start().starts_with('{') {
        return Err(LlmError::Decode(
            "the answer is not a JSON object".to_string(),
        ));
    }
    serde_json::from_str(body).map_err(|e| LlmError::Decode(e.to_string()))
}

// ---------------------------------------------------------------------------
// Number wording (pure)
// ---------------------------------------------------------------------------

/// `n` with its thousands grouped and at most one decimal — `62,500`,
/// `3,739.5`, `172.3` — the trailing `.0` of a whole number dropped.
fn amount(n: f64) -> String {
    let tenths = (n.abs() * 10.0).round();
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // display only
    let tenths = tenths as u64;
    let (whole, fraction) = (tenths / 10, tenths % 10);
    let digits = whole.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, digit) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    let sign = if n < 0.0 && tenths > 0 { "-" } else { "" };
    if fraction == 0 {
        format!("{sign}{grouped}")
    } else {
        format!("{sign}{grouped}.{fraction}")
    }
}

/// `n` humanized — `842`, `15.6k`, `1.2M` — the footer gauge's shape, for
/// counts the page only needs a feel for.
fn compact(n: f64) -> String {
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

// ---------------------------------------------------------------------------
// ChatGPT — `GET {backend-api}/wham/usage` (pure)
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ChatGptUsage {
    plan_type: Text,
    rate_limit: Option<ChatGptRateLimit>,
    code_review_rate_limit: Option<ChatGptRateLimit>,
    additional_rate_limits: Option<Vec<ChatGptAdditional>>,
    credits: Option<ChatGptCredits>,
    spend_control: Option<ChatGptSpendControl>,
    rate_limit_reached_type: Option<ChatGptReachedType>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ChatGptRateLimit {
    limit_reached: Option<bool>,
    primary_window: Option<ChatGptWindow>,
    secondary_window: Option<ChatGptWindow>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ChatGptWindow {
    used_percent: Num,
    limit_window_seconds: Num,
    reset_at: Moment,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ChatGptAdditional {
    limit_name: Text,
    metered_feature: Text,
    rate_limit: Option<ChatGptRateLimit>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ChatGptCredits {
    has_credits: Option<bool>,
    unlimited: Option<bool>,
    balance: Num,
    approx_local_messages: Option<Vec<Num>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ChatGptSpendControl {
    reached: Option<bool>,
    individual_limit: Option<ChatGptSpendLimit>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ChatGptSpendLimit {
    limit: Num,
    used: Num,
    used_percent: Num,
    remaining_percent: Num,
    reset_at: Moment,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ChatGptReachedType {
    #[serde(rename = "type")]
    kind: Text,
}

/// The alert when a request is refused right now and no window says why.
const LIMIT_REACHED_ALERT: &str = "Usage limit reached — requests are paused until it resets";

/// A ChatGPT seat's `/wham/usage` answer as limits — Codex's `/status`
/// source, read the way Codex reads it: the plan (`plan_type`, through
/// [`chatgpt::plan_label`]); the primary and secondary windows, a window at
/// 100% marked reached; each per-model limit and the code-review limit as a
/// named window; the workspace spend cap as a `Monthly credit limit`; the
/// credit balance as a note; a `rate_limit_reached_type` as the alert it
/// names. Every other field is ignored.
///
/// # Errors
/// A body that is not the usage object.
pub fn parse_chatgpt_usage(body: &str) -> Result<PlanLimits> {
    let usage: ChatGptUsage = decode(body)?;
    let mut limits = PlanLimits {
        plan: usage.plan_type.as_deref().map(chatgpt::plan_label),
        ..PlanLimits::default()
    };
    let mut unexplained = false;
    if let Some(rate) = &usage.rate_limit {
        unexplained = push_chatgpt_windows(&mut limits.windows, None, rate);
    }
    for extra in usage.additional_rate_limits.iter().flatten() {
        let name = extra
            .limit_name
            .as_deref()
            .or(extra.metered_feature.as_deref())
            .unwrap_or("Model");
        if let Some(rate) = &extra.rate_limit {
            push_chatgpt_windows(&mut limits.windows, Some(name), rate);
        }
    }
    if let Some(rate) = &usage.code_review_rate_limit {
        push_chatgpt_windows(&mut limits.windows, Some("Code review"), rate);
    }
    if let Some(spend) = &usage.spend_control
        && let Some(cap) = &spend.individual_limit
    {
        limits
            .windows
            .push(spend_window(cap, spend.reached == Some(true)));
    }
    if let Some(note) = usage.credits.as_ref().and_then(credits_note) {
        limits.notes.push(note);
    }
    limits.alert = usage
        .rate_limit_reached_type
        .as_ref()
        .and_then(|reached| reached.kind.as_deref())
        .map(reached_type_alert)
        .or_else(|| unexplained.then(|| LIMIT_REACHED_ALERT.to_string()));
    Ok(limits)
}

/// Push `rate`'s primary and secondary windows, named under `scope` when
/// there is one. Returns whether the rate limit says it is reached while no
/// window it carries is — a refusal the windows cannot explain.
fn push_chatgpt_windows(
    out: &mut Vec<LimitWindow>,
    scope: Option<&str>,
    rate: &ChatGptRateLimit,
) -> bool {
    let mut explained = false;
    for window in [&rate.primary_window, &rate.secondary_window]
        .into_iter()
        .flatten()
    {
        #[allow(clippy::cast_possible_truncation)] // a window's seconds
        let seconds = window.limit_window_seconds.0.unwrap_or(0.0).trunc() as i64;
        let label = window_label(seconds);
        let used = window.used_percent.0.unwrap_or(0.0).max(0.0);
        let reached = used >= 100.0;
        explained |= reached;
        out.push(LimitWindow {
            label: match scope {
                Some(scope) => format!("{scope} · {label}"),
                None => label,
            },
            used_percent: used,
            resets_at: window.reset_at.0,
            detail: None,
            reached,
        });
    }
    rate.limit_reached == Some(true) && !explained
}

/// The workspace spend cap as a window — Codex's `Monthly credit limit`: its
/// share is what the remainder leaves (`100 − remaining_percent`), else the
/// share used, else the credits' own ratio.
fn spend_window(cap: &ChatGptSpendLimit, reached: bool) -> LimitWindow {
    let ratio = match (cap.used.0, cap.limit.0) {
        (Some(used), Some(limit)) if limit > 0.0 => Some(used / limit * 100.0),
        _ => None,
    };
    let used_percent = cap
        .remaining_percent
        .0
        .map(|left| 100.0 - left)
        .or(cap.used_percent.0)
        .or(ratio)
        .unwrap_or(0.0)
        .max(0.0);
    let detail = match (cap.used.0, cap.limit.0) {
        (Some(used), Some(limit)) => Some(format!(
            "{} of {} credits used",
            amount(used),
            amount(limit)
        )),
        _ => None,
    };
    LimitWindow {
        label: "Monthly credit limit".to_string(),
        used_percent,
        resets_at: cap.reset_at.0,
        detail,
        reached: reached || used_percent >= 100.0,
    }
}

/// The credit balance as Codex words it: `Unlimited`, `{n} credits` for a
/// positive balance (rounded), `Available` when credits are on but no
/// balance is named — and no note at all without credits. What the balance
/// buys, when the backend says, rides as its detail.
fn credits_note(credits: &ChatGptCredits) -> Option<LimitNote> {
    let value = if credits.unlimited == Some(true) {
        "Unlimited".to_string()
    } else if credits.has_credits != Some(true) {
        return None;
    } else {
        match credits.balance.0 {
            Some(balance) if balance > 0.0 => format!("{} credits", amount(balance.round())),
            _ => "Available".to_string(),
        }
    };
    let range: Vec<f64> = credits
        .approx_local_messages
        .iter()
        .flatten()
        .filter_map(|n| n.0)
        .filter(|n| *n > 0.0)
        .collect();
    let detail = (credits.unlimited != Some(true))
        .then(|| match range.as_slice() {
            [low, high, ..] => Some(format!(
                "≈{}–{} local messages",
                compact(*low),
                compact(*high)
            )),
            [one] => Some(format!("≈{} local messages", compact(*one))),
            [] => None,
        })
        .flatten();
    Some(LimitNote {
        label: "Credits".to_string(),
        value,
        detail,
    })
}

/// What a `rate_limit_reached_type` says, in words.
fn reached_type_alert(kind: &str) -> String {
    match kind {
        "workspace_owner_credits_depleted" | "workspace_member_credits_depleted" => {
            "Your workspace is out of credits".to_string()
        }
        "workspace_owner_usage_limit_reached" | "workspace_member_usage_limit_reached" => {
            "Your workspace's usage limit is reached".to_string()
        }
        _ => LIMIT_REACHED_ALERT.to_string(),
    }
}

// ---------------------------------------------------------------------------
// GitHub Copilot — `GET https://api.github.com/copilot_internal/user` (pure)
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct CopilotUser {
    access_type_sku: Text,
    copilot_plan: Text,
    token_based_billing: Option<bool>,
    quota_reset_date_utc: Moment,
    quota_reset_date: Moment,
    quota_snapshots: Option<CopilotSnapshots>,
    limited_user_quotas: Option<CopilotLegacy>,
    monthly_quotas: Option<CopilotLegacy>,
    limited_user_reset_date: Moment,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct CopilotSnapshots {
    premium_interactions: Option<CopilotBucket>,
    premium_models: Option<CopilotBucket>,
    chat: Option<CopilotBucket>,
    completions: Option<CopilotBucket>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct CopilotBucket {
    entitlement: Num,
    remaining: Num,
    quota_remaining: Num,
    percent_remaining: Num,
    unlimited: Option<bool>,
    has_quota: Option<bool>,
    overage_permitted: Option<bool>,
    overage_count: Num,
    credits_used: Num,
    quota_reset_at: Moment,
    token_based_billing: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct CopilotLegacy {
    chat: Num,
    completions: Num,
}

/// Which of Copilot's buckets a row is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Bucket {
    Premium,
    Chat,
    Completions,
}

impl Bucket {
    /// The row's name — the premium bucket counts **AI credits** on credit
    /// billing, VS Code's own relabelling.
    fn label(self, credits: bool) -> &'static str {
        match self {
            Self::Premium if credits => "AI credits",
            Self::Premium => "Premium requests",
            Self::Chat => "Chat",
            Self::Completions => "Code completions",
        }
    }

    /// What one unit of the bucket is.
    fn unit(self, credits: bool) -> &'static str {
        match self {
            _ if credits => "credits",
            Self::Premium => "requests",
            Self::Chat => "messages",
            Self::Completions => "completions",
        }
    }
}

/// The alert an unlimited bucket with no quota left raises: a pooled
/// organization budget, spent.
const ORG_BUDGET_ALERT: &str = "Your organization's Copilot budget is used up";

/// A Copilot seat's `copilot_internal/user` answer as limits — VS Code's
/// quota source, read the way its status menu reads it: the plan from the
/// SKU (Free, Student) else `copilot_plan`; each bucket the plan has
/// (premium requests — AI credits on credit billing — then chat, then
/// completions): an unlimited one an `Included` note, a metered one a window
/// whose share is `100 − percent_remaining` and whose detail is what it has
/// used of its entitlement, a bucket outside the plan (`!unlimited &&
/// entitlement == 0`) left out. The legacy free plan's two maps become the
/// same chat and completions windows.
///
/// # Errors
/// A body that is not the user object.
pub fn parse_copilot_usage(body: &str) -> Result<PlanLimits> {
    let user: CopilotUser = decode(body)?;
    let mut limits = PlanLimits {
        plan: copilot_plan_label(
            user.access_type_sku.as_deref(),
            user.copilot_plan.as_deref(),
        ),
        ..PlanLimits::default()
    };
    let reset = user.quota_reset_date_utc.0.or(user.quota_reset_date.0);
    let billing = user.token_based_billing == Some(true);
    if let Some(snapshots) = &user.quota_snapshots {
        let premium = snapshots
            .premium_interactions
            .as_ref()
            .or(snapshots.premium_models.as_ref());
        for (bucket, kind) in [
            (premium, Bucket::Premium),
            (snapshots.chat.as_ref(), Bucket::Chat),
            (snapshots.completions.as_ref(), Bucket::Completions),
        ] {
            if let Some(bucket) = bucket {
                push_copilot_bucket(&mut limits, kind, bucket, billing, reset);
            }
        }
    } else if let (Some(left), Some(allowance)) = (&user.limited_user_quotas, &user.monthly_quotas)
    {
        let reset = user.limited_user_reset_date.0.or(reset);
        for (kind, left, allowance) in [
            (Bucket::Chat, left.chat.0, allowance.chat.0),
            (
                Bucket::Completions,
                left.completions.0,
                allowance.completions.0,
            ),
        ] {
            let (Some(left), Some(allowance)) = (left, allowance) else {
                continue;
            };
            if allowance <= 0.0 {
                continue;
            }
            let used = allowance - left;
            limits.windows.push(LimitWindow {
                label: kind.label(false).to_string(),
                used_percent: (used / allowance * 100.0).max(0.0),
                resets_at: reset,
                detail: Some(format!(
                    "{} of {} {}",
                    amount(used.max(0.0)),
                    amount(allowance),
                    kind.unit(false)
                )),
                reached: left <= 0.0,
            });
        }
    }
    Ok(limits)
}

/// One snapshot bucket as a window, a note, or nothing.
fn push_copilot_bucket(
    limits: &mut PlanLimits,
    kind: Bucket,
    bucket: &CopilotBucket,
    billing: bool,
    reset: Option<i64>,
) {
    let credits = billing || bucket.token_based_billing == Some(true);
    let label = kind.label(credits).to_string();
    if bucket.unlimited == Some(true) {
        let detail = bucket
            .credits_used
            .0
            .filter(|used| *used > 0.0)
            .map(|used| format!("{} credits used", amount(used)));
        limits.notes.push(LimitNote {
            label,
            value: "Included".to_string(),
            detail,
        });
        if bucket.has_quota == Some(false) && limits.alert.is_none() {
            limits.alert = Some(ORG_BUDGET_ALERT.to_string());
        }
        return;
    }
    let entitlement = bucket.entitlement.0.unwrap_or(0.0);
    if entitlement <= 0.0 {
        return; // not in the plan
    }
    let remaining = bucket.quota_remaining.0.or(bucket.remaining.0);
    let used_percent = match bucket.percent_remaining.0 {
        Some(left) => 100.0 - left.clamp(0.0, 100.0),
        None => remaining.map_or(0.0, |left| (entitlement - left) / entitlement * 100.0),
    }
    .max(0.0);
    let used = remaining.map_or(entitlement * used_percent / 100.0, |left| {
        entitlement - left
    });
    let mut detail = format!(
        "{} of {} {}",
        amount(used),
        amount(entitlement),
        kind.unit(credits)
    );
    if let Some(left) = remaining
        && left < 0.0
    {
        detail.push_str(&format!(" · over by {}", amount(-left)));
    }
    let overage = bucket.overage_permitted == Some(true);
    if overage && let Some(count) = bucket.overage_count.0.filter(|c| *c > 0.0) {
        detail.push_str(&format!(" · {} more billed", amount(count)));
    }
    let exhausted = bucket.percent_remaining.0.is_some_and(|left| left <= 0.0)
        || remaining.is_some_and(|left| left <= 0.0)
        || bucket.has_quota == Some(false);
    limits.windows.push(LimitWindow {
        label,
        used_percent,
        resets_at: bucket.quota_reset_at.0.or(reset),
        detail: Some(detail),
        reached: exhausted && !overage,
    });
}

/// The plan a Copilot answer names: the SKU first for the two plans its
/// `copilot_plan` does not tell apart (Free and Student both report
/// `individual`), then the plan itself; an unknown one title-cased, so a
/// plan invented after this build still reads like a name.
fn copilot_plan_label(sku: Option<&str>, plan: Option<&str>) -> Option<String> {
    match sku {
        Some("free_limited_copilot") => return Some("Free".to_string()),
        Some("free_educational_quota") => return Some("Student".to_string()),
        _ => {}
    }
    let label = match plan? {
        "individual" => "Pro",
        "individual_pro" => "Pro+",
        "individual_max" => "Max",
        "individual_edu" => "Student",
        "free" => "Free",
        "business" => "Business",
        "enterprise" => "Enterprise",
        other => return Some(title_case(other)),
    };
    Some(label.to_string())
}

/// `some_slug-name` as `Some Slug Name`.
fn title_case(slug: &str) -> String {
    slug.split(['_', '-'])
        .filter(|word| !word.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

// ---------------------------------------------------------------------------
// ClinePass — `GET {api}/api/v1/users/me/plan/usage-limits` (pure)
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ClineEnvelope<T: Default> {
    success: Option<bool>,
    error: Text,
    data: Option<T>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ClineLimits {
    limits: Option<Vec<ClineLimit>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ClineLimit {
    #[serde(rename = "type")]
    kind: Text,
    #[serde(rename = "percentUsed")]
    percent_used: Num,
    #[serde(rename = "resetsAt")]
    resets_at: Moment,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ClinePlanData {
    plan: Option<ClinePlan>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ClinePlan {
    #[serde(rename = "displayName")]
    display_name: Text,
    name: Text,
    interval: Text,
}

/// ClinePass's `usage-limits` answer — and, when it came, the `plan` one —
/// as limits, the way Cline's own dashboard reads them: each listed window
/// by its type (`five_hour`, `weekly`, `monthly`, any other named from
/// itself), a missing share read as unused, a reset that is not RFC 3339
/// dropped, a share of 100 reached. The plan answer only names the plan —
/// its `interval` (`Monthly`), else its display name — and costs nothing
/// but that name when it is missing or unreadable.
///
/// # Errors
/// A limits body that is not the envelope, or one that is a refusal
/// (`success: false`, worded by its own `error`).
pub fn parse_clinepass_usage(limits: &str, plan: Option<&str>) -> Result<PlanLimits> {
    let envelope: ClineEnvelope<ClineLimits> = decode(limits)?;
    if envelope.success == Some(false) {
        return Err(LlmError::Api {
            status: 0,
            body: envelope
                .error
                .0
                .unwrap_or_else(|| "Cline refused the usage request".to_string()),
        });
    }
    let windows: Vec<LimitWindow> = envelope
        .data
        .and_then(|data| data.limits)
        .unwrap_or_default()
        .into_iter()
        .map(|limit| {
            let used = limit.percent_used.0.unwrap_or(0.0).max(0.0);
            LimitWindow {
                label: clinepass_label(limit.kind.as_deref().unwrap_or("")),
                used_percent: used,
                resets_at: limit.resets_at.0,
                detail: None,
                reached: used >= 100.0,
            }
        })
        .collect();
    Ok(PlanLimits {
        plan: plan.and_then(clinepass_plan_label),
        remark: windows.is_empty().then(|| "No limits reported".to_string()),
        windows,
        ..PlanLimits::default()
    })
}

/// A ClinePass window's name, from its type.
fn clinepass_label(kind: &str) -> String {
    match kind {
        "five_hour" => "5-hour limit".to_string(),
        "daily" => "Daily limit".to_string(),
        "weekly" => "Weekly limit".to_string(),
        "monthly" => "Monthly limit".to_string(),
        "" => "Usage limit".to_string(),
        other => {
            let words = other.replace(['_', '-'], " ");
            let mut chars = words.trim().chars();
            match chars.next() {
                Some(first) => format!("{}{} limit", first.to_uppercase(), chars.as_str()),
                None => "Usage limit".to_string(),
            }
        }
    }
}

/// The plan a ClinePass `plan` answer names, if it is readable.
fn clinepass_plan_label(body: &str) -> Option<String> {
    let envelope: ClineEnvelope<ClinePlanData> = decode(body).ok()?;
    let plan = envelope.data?.plan?;
    plan.interval.0.or(plan.display_name.0).or(plan.name.0)
}

// ---------------------------------------------------------------------------
// Anthropic Console, the links, the URLs (pure)
// ---------------------------------------------------------------------------

/// What a pay-as-you-go sign-in has instead of limits: the Anthropic Console
/// one bills per token to the account's API organisation, and Claude.ai's
/// plan windows describe a subscription it is not (`docs/usage-limits.md`).
#[must_use]
pub fn pay_as_you_go() -> PlanLimits {
    PlanLimits {
        plan: Some("Pay as you go".to_string()),
        remark: Some("Billed per token to your API organisation — no plan limits".to_string()),
        ..PlanLimits::default()
    }
}

/// Where a subscription's own usage page is — the history and the controls
/// the page does not have. `None` for a scheme that is not a sign-in.
#[must_use]
pub fn dashboard_url(auth: AuthScheme) -> Option<&'static str> {
    match auth {
        // Codex's own link (`tui/src/status`).
        AuthScheme::ChatGptCodex => Some("https://chatgpt.com/settings/usage"),
        AuthScheme::GithubCopilot => Some("https://github.com/settings/copilot"),
        // The Cline extension's *Manage Subscription* button.
        AuthScheme::ClineAccount => {
            Some("https://app.cline.bot/dashboard/subscription?personal=true")
        }
        AuthScheme::AnthropicConsole => Some("https://platform.claude.com/usage"),
        AuthScheme::ApiKey | AuthScheme::OptionalKey => None,
    }
}

/// The ChatGPT usage endpoint for a provider's chat base — Codex's
/// path-style rule: a base under `/backend-api` is cut after it and reads
/// `/wham/usage` (the shipped `https://chatgpt.com/backend-api/codex` gives
/// `https://chatgpt.com/backend-api/wham/usage`); any other base is the
/// Codex API's own and reads `/api/codex/usage`.
#[must_use]
pub fn chatgpt_usage_url(api_base: &str) -> String {
    const BACKEND: &str = "/backend-api";
    let base = api_base.trim().trim_end_matches('/');
    match base.find(BACKEND) {
        Some(at) => format!("{}/wham/usage", &base[..at + BACKEND.len()]),
        None => format!("{base}/api/codex/usage"),
    }
}

// ---------------------------------------------------------------------------
// The fetch (boundary)
// ---------------------------------------------------------------------------

/// Read one signed-in subscription's limits. Boundary code — one round trip
/// (two for ClinePass, whose plan is a second, optional read), none for the
/// pay-as-you-go Console sign-in. It dispatches on the provider's **auth
/// scheme**, never its id, and authenticates the way every request does:
///
/// - ChatGPT through `auth::request_auth` — the access token every turn
///   already mints and caches, the account id and the user agent;
/// - Copilot with the stored GitHub OAuth token itself (`Authorization:
///   token …`) and the editor identity — no Copilot bearer is exchanged;
/// - ClinePass with the `workos:` bearer, on the API host in force.
///
/// `cancel` is checked before each request and after it: a page closed
/// meanwhile gets nothing back.
///
/// # Errors
/// A sign-in the provider refused (worded by the auth seam), a usage read it
/// refused (worded by [`refusal`]), a transport failure, an answer that is
/// not the expected shape, or a cancel.
pub fn fetch_plan_limits(cfg: &ModelConfig, cancel: &CancelToken) -> Result<PlanLimits> {
    match cfg.auth {
        AuthScheme::AnthropicConsole => Ok(pay_as_you_go()),
        AuthScheme::ChatGptCodex => {
            let body = get_json(cfg, cancel, &chatgpt_usage_url(&cfg.api_base), |req| {
                let auth = auth::request_auth(cfg)?;
                let bearer = auth.bearer.ok_or_else(not_signed_in)?;
                let mut req = req.header("authorization", format!("Bearer {bearer}"));
                for (name, value) in auth.headers {
                    req = req.header(name, value);
                }
                Ok(req)
            })?;
            parse_chatgpt_usage(&body)
        }
        AuthScheme::GithubCopilot => {
            let token = cfg
                .api_key
                .clone()
                .filter(|t| !t.is_empty())
                .ok_or_else(not_signed_in)?;
            let body = get_json(cfg, cancel, &copilot::user_info_url(), |req| {
                Ok(req
                    .header("authorization", format!("token {token}"))
                    .header("editor-version", copilot::EDITOR_VERSION)
                    .header("editor-plugin-version", copilot::EDITOR_PLUGIN_VERSION)
                    .header("user-agent", copilot::USER_AGENT)
                    .header("x-github-api-version", copilot::GITHUB_API_VERSION))
            })?;
            parse_copilot_usage(&body)
        }
        AuthScheme::ClineAccount => {
            let bearer = auth::request_auth(cfg)?.bearer.ok_or_else(not_signed_in)?;
            let with_bearer = |req: reqwest::blocking::RequestBuilder| {
                Ok(req.header("authorization", format!("Bearer {bearer}")))
            };
            let limits = get_json(cfg, cancel, &cline::usage_limits_url(), with_bearer)?;
            // The plan only names the plan: a refusal of it costs the label.
            let plan = get_json(cfg, cancel, &cline::plan_url(), with_bearer).ok();
            parse_clinepass_usage(&limits, plan.as_deref())
        }
        AuthScheme::ApiKey | AuthScheme::OptionalKey => Err(LlmError::Decode(
            "this provider is billed per key, not a subscription".to_string(),
        )),
    }
}

/// The error for a subscription with no stored sign-in.
fn not_signed_in() -> LlmError {
    LlmError::Api {
        status: 0,
        body: "Not signed in — run /login to sign in".to_string(),
    }
}

/// `GET url` with JSON accepted and `authorize` adding the credential,
/// cancel-checked on both sides of the request, the body read capped. A
/// non-2xx answer is the [`refusal`] it words.
fn get_json(
    cfg: &ModelConfig,
    cancel: &CancelToken,
    url: &str,
    authorize: impl FnOnce(
        reqwest::blocking::RequestBuilder,
    ) -> Result<reqwest::blocking::RequestBuilder>,
) -> Result<String> {
    if cancel.is_cancelled() {
        return Err(LlmError::Cancelled);
    }
    let client = super::http_client(USAGE_TIMEOUT)?;
    let request = authorize(client.get(url).header("accept", "application/json"))?;
    let resp = request.send().map_err(|e| LlmError::transport(&e))?;
    let status = resp.status().as_u16();
    let mut body = String::new();
    resp.take(USAGE_BODY_MAX_BYTES)
        .read_to_string(&mut body)
        .map_err(|e| LlmError::body_read(&e))?;
    if cancel.is_cancelled() {
        return Err(LlmError::Cancelled);
    }
    if !(200..300).contains(&status) {
        return Err(LlmError::Api {
            status,
            body: refusal(cfg.auth, status, &body),
        });
    }
    Ok(body)
}

/// A usage read the provider refused, in words the page can show.
#[must_use]
pub fn refusal(auth: AuthScheme, status: u16, body: &str) -> String {
    let provider = match auth {
        AuthScheme::ChatGptCodex => "ChatGPT",
        AuthScheme::GithubCopilot => "GitHub",
        AuthScheme::ClineAccount => "Cline",
        AuthScheme::AnthropicConsole => "Anthropic",
        AuthScheme::ApiKey | AuthScheme::OptionalKey => "The provider",
    };
    match (auth, status) {
        (_, 401) => format!("{provider} refused the stored sign-in — run /login to sign in again"),
        (AuthScheme::ClineAccount, 404) => "No active ClinePass plan on this account".to_string(),
        (AuthScheme::ClineAccount, 402) => {
            "ClinePass is paused until the subscription's unpaid invoice is paid".to_string()
        }
        (_, 403) => {
            format!("{provider} refused the usage request (HTTP 403) — run /login if it persists")
        }
        (_, 429) => format!("{provider} is rate limiting usage checks — try again in a minute"),
        _ => {
            let flat: String = body.split_whitespace().collect::<Vec<_>>().join(" ");
            let flat: String = flat.chars().take(160).collect();
            if flat.is_empty() || flat.starts_with('<') {
                format!("{provider} answered HTTP {status}")
            } else {
                format!("{provider} answered HTTP {status}: {flat}")
            }
        }
    }
}

/// A failed fetch as the one line the page shows: a refusal or advice as
/// worded, a host that cannot be reached named, anything else as itself.
#[must_use]
pub fn error_text(error: &LlmError) -> String {
    match error {
        LlmError::Api { status, body } => {
            let body = body.trim();
            if body.is_empty() {
                format!("The provider answered HTTP {status}")
            } else {
                body.to_string()
            }
        }
        LlmError::Unreachable { host, .. } => format!("No connection to {host}"),
        LlmError::Decode(e) => format!("Couldn't read the answer: {e}"),
        LlmError::Http(e) => format!("The request failed: {e}"),
        LlmError::Cancelled => "Cancelled".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ----- fixtures -----

    /// What a ChatGPT Pro seat's `/wham/usage` answered live on 2026-10-25
    /// (identities redacted): one weekly window, a credit balance, and the
    /// fields this module does not read.
    const CHATGPT_LIVE: &str = r#"{"user_id":"user-redacted","account_id":"account-redacted","email":"dev@example.com","plan_type":"pro","rate_limit":{"allowed":true,"limit_reached":false,"primary_window":{"used_percent":0,"limit_window_seconds":604800,"reset_after_seconds":583887,"reset_at":1793521139},"secondary_window":null},"code_review_rate_limit":null,"additional_rate_limits":null,"model_usage":{"gpt-6-astra":{"available":true,"available_at":null,"credits_would_enable":false}},"credits":{"has_credits":true,"unlimited":false,"overage_limit_reached":false,"balance":"62500.0000000000","approx_local_messages":[15625,81250],"approx_cloud_messages":[2500,15625]},"spend_control":{"reached":false,"individual_limit":null},"rate_limit_reached_type":null,"promo":null,"rate_limit_reset_credits":{"available_count":2,"applicable_available_count":0}}"#;

    /// Codex's own fixture (`app-server/tests/suite/v2/rate_limits.rs`),
    /// its primary window a five-hour one: two windows, a per-model limit, a
    /// workspace spend cap and a reached type.
    const CHATGPT_FULL: &str = r#"{"plan_type":"plus","rate_limit":{"allowed":true,"limit_reached":false,"primary_window":{"used_percent":42,"limit_window_seconds":18000,"reset_after_seconds":120,"reset_at":1735689720},"secondary_window":{"used_percent":5,"limit_window_seconds":604800,"reset_after_seconds":43200,"reset_at":1735693200}},"rate_limit_reached_type":{"type":"workspace_member_usage_limit_reached"},"spend_control":{"reached":false,"individual_limit":{"source":"workspace_spend_controls","limit":"25000","used":"8000","remaining":"17000","used_percent":32,"remaining_percent":68,"reset_after_seconds":43200,"reset_at":1735693200}},"additional_rate_limits":[{"limit_name":"codex_other","metered_feature":"codex_other","rate_limit":{"allowed":true,"limit_reached":false,"primary_window":{"used_percent":88,"limit_window_seconds":1800,"reset_after_seconds":600,"reset_at":1735693200}}}]}"#;

    /// ClinePass's `usage-limits`, live on 2026-10-25: an unused rolling
    /// window with no reset, a used-up week, and a month at 81%.
    const CLINE_LIVE: &str = r#"{"data":{"limits":[{"type":"five_hour","percentUsed":0},{"type":"weekly","percentUsed":100,"resetsAt":"2026-10-27T14:36:38.072840224Z"},{"type":"monthly","percentUsed":81,"resetsAt":"2026-11-12T13:26:58.074857294Z"}]},"success":true}"#;

    /// ClinePass's `plan`, live on 2026-10-25, trimmed to what is read.
    const CLINE_PLAN_LIVE: &str = r#"{"data":{"plan":{"name":"Cline Pass (Monthly)[Internal]","displayName":"Cline Pass (Monthly)","type":"individual","interval":"Monthly","pricePerSeatCents":999,"isActive":true},"currentPeriodStart":"2026-10-13T13:24:06Z","currentPeriodEnd":"2026-11-13T13:24:06Z"},"success":true}"#;

    /// A Copilot Pro seat on premium-request billing (a capture published
    /// by AIUsageTracker): premium requests metered, chat and completions
    /// unlimited.
    const COPILOT_PRO: &str = r#"{"access_type_sku":"monthly_subscriber_quota","copilot_plan":"individual","quota_reset_date":"2026-04-01","quota_reset_date_utc":"2026-04-01T00:00:00.000Z","quota_snapshots":{"chat":{"entitlement":0,"overage_count":0,"overage_permitted":false,"percent_remaining":100.0,"quota_id":"chat","quota_remaining":0.0,"remaining":0,"unlimited":true,"timestamp_utc":"2026-03-12T16:24:00.879Z"},"completions":{"entitlement":0,"overage_count":0,"overage_permitted":false,"percent_remaining":100.0,"quota_id":"completions","quota_remaining":0.0,"remaining":0,"unlimited":true,"timestamp_utc":"2026-03-12T16:24:00.879Z"},"premium_interactions":{"entitlement":300,"overage_count":0,"overage_permitted":false,"percent_remaining":50.6,"quota_id":"premium_interactions","quota_remaining":152.0,"remaining":152,"unlimited":false,"timestamp_utc":"2026-03-12T16:24:00.879Z"}}}"#;

    /// Copilot Free on credit billing (a capture published by switchboard):
    /// chat and completions metered in credits, premium outside the plan.
    const COPILOT_FREE: &str = r#"{"access_type_sku":"free_limited_copilot","copilot_plan":"individual","token_based_billing":true,"quota_reset_date":"2026-11-01","quota_reset_date_utc":"2026-11-01T00:00:00.000Z","quota_snapshots":{"chat":{"overage_count":0,"overage_permitted":false,"percent_remaining":13.8,"quota_id":"chat","quota_remaining":27.7,"unlimited":false,"has_quota":true,"quota_reset_at":0,"token_based_billing":true,"credits_used":0,"remaining":27,"entitlement":200},"completions":{"overage_count":0,"overage_permitted":false,"percent_remaining":100.0,"quota_id":"completions","quota_remaining":2000.0,"unlimited":false,"has_quota":true,"quota_reset_at":0,"token_based_billing":true,"credits_used":0,"remaining":2000,"entitlement":2000},"premium_interactions":{"overage_count":0,"overage_permitted":false,"percent_remaining":0.0,"quota_id":"premium_interactions","quota_remaining":0.0,"unlimited":false,"has_quota":false,"quota_reset_at":0,"token_based_billing":true,"credits_used":0,"remaining":0,"entitlement":0}}}"#;

    /// Copilot Max on credit billing (a capture published by token-torch):
    /// 20,000 AI credits with paid overage on.
    const COPILOT_MAX: &str = r#"{"access_type_sku":"max_monthly_subscriber_quota","copilot_plan":"individual_max","token_based_billing":true,"quota_reset_date":"2026-11-01","quota_reset_date_utc":"2026-11-01T00:00:00.000Z","quota_snapshots":{"chat":{"overage_count":0,"overage_permitted":false,"percent_remaining":100.0,"quota_remaining":0.0,"unlimited":true,"has_quota":true,"quota_reset_at":0,"credits_used":0,"remaining":0,"entitlement":0},"completions":{"overage_count":0,"overage_permitted":false,"percent_remaining":100.0,"quota_remaining":0.0,"unlimited":true,"has_quota":true,"quota_reset_at":0,"credits_used":0,"remaining":0,"entitlement":0},"premium_interactions":{"overage_count":0,"overage_permitted":true,"percent_remaining":100.0,"quota_remaining":20000.0,"unlimited":false,"has_quota":true,"quota_reset_at":0,"token_based_billing":true,"credits_used":0,"overage_entitlement":10000,"remaining":20000,"entitlement":20000}}}"#;

    /// A Business seat past its allowance on credit billing (opencode-quota
    /// issue #300): the remainder negative, nothing left to share.
    const COPILOT_OVER: &str = r#"{"access_type_sku":"copilot_standalone_seat_quota","copilot_plan":"business","token_based_billing":true,"quota_reset_date_utc":"2026-11-01T00:00:00.000Z","quota_snapshots":{"premium_interactions":{"entitlement":3000,"quota_remaining":-739.5,"remaining":-740,"credits_used":3739,"percent_remaining":0,"has_quota":false,"unlimited":false,"overage_permitted":false}}}"#;

    /// The legacy free plan (a capture published by AIMeter): no snapshots,
    /// the allowance and what remains of it in two maps.
    const COPILOT_LEGACY_FREE: &str = r#"{"access_type_sku":"free_limited_copilot","copilot_plan":"individual","limited_user_quotas":{"chat":500,"completions":3876},"limited_user_subscribed_day":26,"limited_user_reset_date":"2026-03-26","monthly_quotas":{"chat":500,"completions":4000}}"#;

    /// Rounds a share for comparison: the parsers do float arithmetic.
    fn share(window: &LimitWindow) -> f64 {
        (window.used_percent * 10.0).round() / 10.0
    }

    // ----- window names -----

    #[test]
    fn a_window_is_named_by_its_length() {
        assert_eq!(window_label(5 * 3600), "5-hour limit");
        assert_eq!(window_label(86_400), "Daily limit");
        assert_eq!(window_label(604_800), "Weekly limit");
        assert_eq!(window_label(30 * 86_400), "Monthly limit");
        assert_eq!(window_label(365 * 86_400), "Annual limit");
    }

    #[test]
    fn a_window_near_a_named_length_takes_its_name() {
        // Codex's ±5% rule — a window a few minutes off its nominal length
        // is still that window.
        assert_eq!(window_label(5 * 3600 - 600), "5-hour limit");
        assert_eq!(window_label(604_800 + 3600), "Weekly limit");
        // A month is any calendar month's length, February included.
        assert_eq!(window_label(28 * 86_400), "Monthly limit");
        assert_eq!(window_label(31 * 86_400), "Monthly limit");
        assert_eq!(window_label(366 * 86_400), "Annual limit");
    }

    #[test]
    fn any_other_window_names_its_own_length() {
        assert_eq!(window_label(1800), "30-minute limit");
        assert_eq!(window_label(3600), "1-hour limit");
        assert_eq!(window_label(3 * 86_400), "3-day limit");
        // A part minute rounds up rather than reading as a shorter window.
        assert_eq!(window_label(90), "2-minute limit");
    }

    #[test]
    fn a_window_of_no_length_is_a_usage_limit() {
        assert_eq!(window_label(0), "Usage limit");
        assert_eq!(window_label(-5), "Usage limit");
    }

    // ----- ChatGPT -----

    #[test]
    fn a_live_chatgpt_answer_is_its_plan_its_window_and_its_credits() {
        let limits = parse_chatgpt_usage(CHATGPT_LIVE).unwrap();
        assert_eq!(limits.plan.as_deref(), Some("Pro"));
        assert_eq!(
            limits.windows,
            vec![LimitWindow {
                label: "Weekly limit".to_string(),
                used_percent: 0.0,
                resets_at: Some(1_793_521_139),
                detail: None,
                reached: false,
            }]
        );
        assert_eq!(
            limits.notes,
            vec![LimitNote {
                label: "Credits".to_string(),
                value: "62,500 credits".to_string(),
                detail: Some("≈15.6k–81.3k local messages".to_string()),
            }]
        );
        assert_eq!(limits.alert, None);
        assert_eq!(limits.remark, None);
    }

    #[test]
    fn chatgpt_lists_both_windows_then_the_per_model_limits_then_the_spend_cap() {
        let limits = parse_chatgpt_usage(CHATGPT_FULL).unwrap();
        assert_eq!(limits.plan.as_deref(), Some("Plus"));
        let labels: Vec<&str> = limits.windows.iter().map(|w| w.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "5-hour limit",
                "Weekly limit",
                "codex_other · 30-minute limit",
                "Monthly credit limit",
            ]
        );
        assert_eq!(share(&limits.windows[0]), 42.0);
        assert_eq!(limits.windows[0].resets_at, Some(1_735_689_720));
        assert_eq!(share(&limits.windows[1]), 5.0);
        assert_eq!(share(&limits.windows[2]), 88.0);
        // The spend cap's share is what its remainder leaves, and its detail
        // the credits behind it.
        let cap = &limits.windows[3];
        assert_eq!(share(cap), 32.0);
        assert_eq!(cap.detail.as_deref(), Some("8,000 of 25,000 credits used"));
        assert_eq!(cap.resets_at, Some(1_735_693_200));
        assert!(limits.windows.iter().all(|w| !w.reached));
    }

    #[test]
    fn a_reached_type_is_the_alert_it_names() {
        let limits = parse_chatgpt_usage(CHATGPT_FULL).unwrap();
        assert_eq!(
            limits.alert.as_deref(),
            Some("Your workspace's usage limit is reached")
        );
    }

    #[test]
    fn a_used_up_chatgpt_window_is_reached() {
        let body = r#"{"plan_type":"plus","rate_limit":{"allowed":false,"limit_reached":true,"primary_window":{"used_percent":100,"limit_window_seconds":18000,"reset_at":1735689720},"secondary_window":{"used_percent":64,"limit_window_seconds":604800,"reset_at":1735693200}}}"#;
        let limits = parse_chatgpt_usage(body).unwrap();
        assert!(limits.windows[0].reached);
        assert!(!limits.windows[1].reached);
        // The window says it; no alert repeats it.
        assert_eq!(limits.alert, None);
    }

    #[test]
    fn a_reached_limit_no_window_explains_is_an_alert() {
        let body = r#"{"plan_type":"plus","rate_limit":{"allowed":false,"limit_reached":true,"primary_window":{"used_percent":64,"limit_window_seconds":18000,"reset_at":1735689720}}}"#;
        let limits = parse_chatgpt_usage(body).unwrap();
        assert_eq!(
            limits.alert.as_deref(),
            Some("Usage limit reached — requests are paused until it resets")
        );
    }

    #[test]
    fn chatgpt_credits_read_as_codex_reads_them() {
        let note = |credits: &str| {
            let body = format!(r#"{{"plan_type":"plus","credits":{credits}}}"#);
            parse_chatgpt_usage(&body).unwrap().notes
        };
        assert_eq!(
            note(r#"{"has_credits":true,"unlimited":true,"balance":null}"#)[0].value,
            "Unlimited"
        );
        assert_eq!(
            note(r#"{"has_credits":true,"unlimited":false,"balance":"9.99"}"#)[0].value,
            "10 credits"
        );
        assert_eq!(
            note(r#"{"has_credits":true,"unlimited":false,"balance":null}"#)[0].value,
            "Available"
        );
        assert!(note(r#"{"has_credits":false,"unlimited":false,"balance":"0"}"#).is_empty());
    }

    #[test]
    fn a_code_review_limit_is_a_named_window() {
        let body = r#"{"plan_type":"pro","code_review_rate_limit":{"allowed":true,"limit_reached":false,"primary_window":{"used_percent":12,"limit_window_seconds":604800,"reset_at":1735693200}}}"#;
        let limits = parse_chatgpt_usage(body).unwrap();
        assert_eq!(limits.windows[0].label, "Code review · Weekly limit");
    }

    #[test]
    fn a_chatgpt_answer_that_names_nothing_is_still_an_answer() {
        let limits = parse_chatgpt_usage("{}").unwrap();
        assert_eq!(limits, PlanLimits::default());
    }

    #[test]
    fn a_chatgpt_body_that_is_not_the_object_is_a_decode_error() {
        assert!(parse_chatgpt_usage("<html>").is_err());
        assert!(parse_chatgpt_usage("[]").is_err());
    }

    #[test]
    fn a_fractional_chatgpt_share_is_kept() {
        let body = r#"{"plan_type":"plus","rate_limit":{"primary_window":{"used_percent":12.5,"limit_window_seconds":18000,"reset_at":1735689720.0}}}"#;
        let limits = parse_chatgpt_usage(body).unwrap();
        assert_eq!(share(&limits.windows[0]), 12.5);
        assert_eq!(limits.windows[0].resets_at, Some(1_735_689_720));
    }

    #[test]
    fn a_reset_of_zero_is_no_reset() {
        let body = r#"{"plan_type":"plus","rate_limit":{"primary_window":{"used_percent":3,"limit_window_seconds":18000,"reset_at":0}}}"#;
        assert_eq!(
            parse_chatgpt_usage(body).unwrap().windows[0].resets_at,
            None
        );
    }

    // ----- Copilot -----

    #[test]
    fn a_copilot_pro_seat_meters_its_premium_requests_and_includes_the_rest() {
        let limits = parse_copilot_usage(COPILOT_PRO).unwrap();
        assert_eq!(limits.plan.as_deref(), Some("Pro"));
        assert_eq!(limits.windows.len(), 1);
        let premium = &limits.windows[0];
        assert_eq!(premium.label, "Premium requests");
        assert_eq!(share(premium), 49.4);
        assert_eq!(premium.detail.as_deref(), Some("148 of 300 requests"));
        assert_eq!(premium.resets_at, Some(1_775_001_600));
        assert!(!premium.reached);
        assert_eq!(
            limits.notes,
            vec![
                LimitNote {
                    label: "Chat".to_string(),
                    value: "Included".to_string(),
                    detail: None,
                },
                LimitNote {
                    label: "Code completions".to_string(),
                    value: "Included".to_string(),
                    detail: None,
                },
            ]
        );
    }

    #[test]
    fn copilot_free_on_credit_billing_meters_chat_and_completions_in_credits() {
        let limits = parse_copilot_usage(COPILOT_FREE).unwrap();
        assert_eq!(limits.plan.as_deref(), Some("Free"));
        // Premium is outside the plan (no entitlement), so it is not a row.
        let labels: Vec<&str> = limits.windows.iter().map(|w| w.label.as_str()).collect();
        assert_eq!(labels, ["Chat", "Code completions"]);
        assert_eq!(share(&limits.windows[0]), 86.2);
        assert_eq!(
            limits.windows[0].detail.as_deref(),
            Some("172.3 of 200 credits")
        );
        assert_eq!(share(&limits.windows[1]), 0.0);
        assert_eq!(
            limits.windows[1].detail.as_deref(),
            Some("0 of 2,000 credits")
        );
        assert_eq!(limits.windows[0].resets_at, Some(1_793_491_200));
        assert!(limits.notes.is_empty());
    }

    #[test]
    fn copilot_on_credit_billing_names_its_premium_bucket_for_credits() {
        let limits = parse_copilot_usage(COPILOT_MAX).unwrap();
        assert_eq!(limits.plan.as_deref(), Some("Max"));
        let credits = &limits.windows[0];
        assert_eq!(credits.label, "AI credits");
        assert_eq!(credits.detail.as_deref(), Some("0 of 20,000 credits"));
        assert_eq!(share(credits), 0.0);
    }

    #[test]
    fn a_copilot_seat_past_its_allowance_says_by_how_much() {
        let limits = parse_copilot_usage(COPILOT_OVER).unwrap();
        assert_eq!(limits.plan.as_deref(), Some("Business"));
        let credits = &limits.windows[0];
        assert_eq!(share(credits), 100.0);
        assert!(credits.reached);
        assert_eq!(
            credits.detail.as_deref(),
            Some("3,739.5 of 3,000 credits · over by 739.5")
        );
    }

    #[test]
    fn paid_overage_keeps_a_spent_copilot_bucket_unreached() {
        let body = r#"{"copilot_plan":"individual_pro","quota_reset_date":"2026-11-01","quota_snapshots":{"premium_interactions":{"entitlement":1500,"quota_remaining":0,"percent_remaining":0,"unlimited":false,"overage_permitted":true,"overage_count":42}}}"#;
        let limits = parse_copilot_usage(body).unwrap();
        assert_eq!(limits.plan.as_deref(), Some("Pro+"));
        let premium = &limits.windows[0];
        assert!(!premium.reached, "overage billing keeps requests going");
        assert_eq!(
            premium.detail.as_deref(),
            Some("1,500 of 1,500 requests · 42 more billed")
        );
        // A date with no time is the start of that day, UTC.
        assert_eq!(premium.resets_at, Some(1_793_491_200));
    }

    #[test]
    fn the_legacy_free_plan_meters_from_its_two_maps() {
        let limits = parse_copilot_usage(COPILOT_LEGACY_FREE).unwrap();
        assert_eq!(limits.plan.as_deref(), Some("Free"));
        let labels: Vec<&str> = limits.windows.iter().map(|w| w.label.as_str()).collect();
        assert_eq!(labels, ["Chat", "Code completions"]);
        assert_eq!(share(&limits.windows[0]), 0.0);
        assert_eq!(
            limits.windows[0].detail.as_deref(),
            Some("0 of 500 messages")
        );
        assert_eq!(share(&limits.windows[1]), 3.1);
        assert_eq!(
            limits.windows[1].detail.as_deref(),
            Some("124 of 4,000 completions")
        );
        assert_eq!(limits.windows[1].resets_at, Some(1_774_483_200));
    }

    #[test]
    fn an_unlimited_bucket_with_no_quota_left_is_the_organizations_limit() {
        let body = r#"{"copilot_plan":"enterprise","quota_snapshots":{"premium_interactions":{"entitlement":0,"unlimited":true,"has_quota":false,"credits_used":1250}}}"#;
        let limits = parse_copilot_usage(body).unwrap();
        assert_eq!(limits.plan.as_deref(), Some("Enterprise"));
        assert_eq!(
            limits.alert.as_deref(),
            Some("Your organization's Copilot budget is used up")
        );
        assert_eq!(
            limits.notes,
            vec![LimitNote {
                label: "Premium requests".to_string(),
                value: "Included".to_string(),
                detail: Some("1,250 credits used".to_string()),
            }]
        );
    }

    #[test]
    fn an_unknown_copilot_plan_reads_as_a_name() {
        let body = r#"{"copilot_plan":"individual_ultra_seat"}"#;
        assert_eq!(
            parse_copilot_usage(body).unwrap().plan.as_deref(),
            Some("Individual Ultra Seat")
        );
        // The student SKU outranks the plan that rides with it.
        let student = r#"{"access_type_sku":"free_educational_quota","copilot_plan":"individual"}"#;
        assert_eq!(
            parse_copilot_usage(student).unwrap().plan.as_deref(),
            Some("Student")
        );
    }

    #[test]
    fn a_copilot_body_that_is_not_the_object_is_a_decode_error() {
        assert!(parse_copilot_usage("Not Found").is_err());
    }

    // ----- ClinePass -----

    #[test]
    fn a_live_clinepass_answer_is_its_three_windows_and_its_plan() {
        let limits = parse_clinepass_usage(CLINE_LIVE, Some(CLINE_PLAN_LIVE)).unwrap();
        assert_eq!(limits.plan.as_deref(), Some("Monthly"));
        assert_eq!(
            limits.windows,
            vec![
                LimitWindow {
                    label: "5-hour limit".to_string(),
                    used_percent: 0.0,
                    resets_at: None,
                    detail: None,
                    reached: false,
                },
                LimitWindow {
                    label: "Weekly limit".to_string(),
                    used_percent: 100.0,
                    resets_at: Some(1_793_111_798),
                    detail: None,
                    reached: true,
                },
                LimitWindow {
                    label: "Monthly limit".to_string(),
                    used_percent: 81.0,
                    resets_at: Some(1_794_490_018),
                    detail: None,
                    reached: false,
                },
            ]
        );
    }

    #[test]
    fn a_clinepass_answer_without_its_plan_still_has_its_windows() {
        let limits = parse_clinepass_usage(CLINE_LIVE, None).unwrap();
        assert_eq!(limits.plan, None);
        assert_eq!(limits.windows.len(), 3);
        // A plan body that is not the envelope costs only the plan's name.
        let limits = parse_clinepass_usage(CLINE_LIVE, Some("<html>")).unwrap();
        assert_eq!(limits.plan, None);
        assert_eq!(limits.windows.len(), 3);
    }

    #[test]
    fn a_plan_with_no_interval_is_named_by_its_display_name() {
        let plan = r#"{"data":{"plan":{"displayName":"ClinePass Annual"}},"success":true}"#;
        let limits = parse_clinepass_usage(CLINE_LIVE, Some(plan)).unwrap();
        assert_eq!(limits.plan.as_deref(), Some("ClinePass Annual"));
    }

    #[test]
    fn an_unknown_clinepass_window_is_named_from_its_type() {
        let body = r#"{"data":{"limits":[{"type":"daily","percentUsed":12.5},{"type":"burst_window","percentUsed":3}]},"success":true}"#;
        let limits = parse_clinepass_usage(body, None).unwrap();
        let labels: Vec<&str> = limits.windows.iter().map(|w| w.label.as_str()).collect();
        assert_eq!(labels, ["Daily limit", "Burst window limit"]);
        assert_eq!(share(&limits.windows[0]), 12.5);
    }

    #[test]
    fn a_clinepass_window_missing_its_share_is_unused_and_a_bad_reset_is_none() {
        let body = r#"{"data":{"limits":[{"type":"weekly","resetsAt":"soon"}]},"success":true}"#;
        let window = &parse_clinepass_usage(body, None).unwrap().windows[0];
        assert_eq!(window.used_percent, 0.0);
        assert_eq!(window.resets_at, None);
    }

    #[test]
    fn a_clinepass_refusal_is_its_own_error() {
        let body = r#"{"success":false,"error":"no plan found for user"}"#;
        let error = parse_clinepass_usage(body, None).unwrap_err();
        assert!(
            error.to_string().contains("no plan found for user"),
            "{error}"
        );
    }

    #[test]
    fn clinepass_with_no_limits_listed_says_so() {
        let body = r#"{"data":{"limits":[]},"success":true}"#;
        let limits = parse_clinepass_usage(body, None).unwrap();
        assert!(limits.windows.is_empty());
        assert_eq!(limits.remark.as_deref(), Some("No limits reported"));
    }

    // ----- the rest -----

    #[test]
    fn a_pay_as_you_go_sign_in_has_a_plan_and_a_remark_and_nothing_to_meter() {
        let limits = pay_as_you_go();
        assert_eq!(limits.plan.as_deref(), Some("Pay as you go"));
        assert!(limits.windows.is_empty());
        assert!(limits.notes.is_empty());
        assert_eq!(
            limits.remark.as_deref(),
            Some("Billed per token to your API organisation — no plan limits")
        );
    }

    #[test]
    fn every_subscription_links_its_own_usage_page_and_a_key_none() {
        assert_eq!(
            dashboard_url(AuthScheme::ChatGptCodex),
            Some("https://chatgpt.com/settings/usage")
        );
        assert_eq!(
            dashboard_url(AuthScheme::GithubCopilot),
            Some("https://github.com/settings/copilot")
        );
        assert_eq!(
            dashboard_url(AuthScheme::ClineAccount),
            Some("https://app.cline.bot/dashboard/subscription?personal=true")
        );
        assert_eq!(
            dashboard_url(AuthScheme::AnthropicConsole),
            Some("https://platform.claude.com/usage")
        );
        assert_eq!(dashboard_url(AuthScheme::ApiKey), None);
        assert_eq!(dashboard_url(AuthScheme::OptionalKey), None);
    }

    // ----- refusals -----

    #[test]
    fn a_refused_sign_in_names_login() {
        assert_eq!(
            refusal(AuthScheme::ChatGptCodex, 401, "{}"),
            "ChatGPT refused the stored sign-in — run /login to sign in again"
        );
        assert_eq!(
            refusal(AuthScheme::GithubCopilot, 401, ""),
            "GitHub refused the stored sign-in — run /login to sign in again"
        );
    }

    #[test]
    fn a_clinepass_account_with_no_plan_says_so() {
        assert_eq!(
            refusal(
                AuthScheme::ClineAccount,
                404,
                r#"{"error":"no plan found"}"#
            ),
            "No active ClinePass plan on this account"
        );
        assert_eq!(
            refusal(AuthScheme::ClineAccount, 402, ""),
            "ClinePass is paused until the subscription's unpaid invoice is paid"
        );
    }

    #[test]
    fn a_rate_limited_usage_read_says_to_wait() {
        assert_eq!(
            refusal(AuthScheme::GithubCopilot, 429, ""),
            "GitHub is rate limiting usage checks — try again in a minute"
        );
    }

    #[test]
    fn any_other_refusal_quotes_a_short_body_but_never_a_page() {
        assert_eq!(
            refusal(AuthScheme::ChatGptCodex, 500, "  internal\n  error "),
            "ChatGPT answered HTTP 500: internal error"
        );
        // An intercepting proxy's HTML is not a reason.
        assert_eq!(
            refusal(
                AuthScheme::ChatGptCodex,
                502,
                "<html><body>Bad gateway</body></html>"
            ),
            "ChatGPT answered HTTP 502"
        );
        let long = "x".repeat(400);
        assert_eq!(
            refusal(AuthScheme::ClineAccount, 500, &long).len(),
            "Cline answered HTTP 500: ".len() + 160
        );
    }

    #[test]
    fn a_failed_fetch_is_one_line_the_page_can_show() {
        let api = LlmError::Api {
            status: 400,
            body: "Your ChatGPT sign-in has expired. Run /login and sign in again.".to_string(),
        };
        // The auth seam's advice is shown as worded, with no `HTTP 400:`.
        assert_eq!(
            error_text(&api),
            "Your ChatGPT sign-in has expired. Run /login and sign in again."
        );
        let bare = LlmError::Api {
            status: 503,
            body: String::new(),
        };
        assert_eq!(error_text(&bare), "The provider answered HTTP 503");
        let offline = LlmError::Unreachable {
            host: "chatgpt.com".to_string(),
            message: "dns error: failed to lookup address information".to_string(),
        };
        assert_eq!(error_text(&offline), "No connection to chatgpt.com");
        assert_eq!(
            error_text(&LlmError::Decode("expected value".to_string())),
            "Couldn't read the answer: expected value"
        );
    }

    #[test]
    fn the_clinepass_endpoints_sit_on_clines_api_host() {
        assert_eq!(
            super::super::cline::usage_limits_url(),
            "https://api.cline.bot/api/v1/users/me/plan/usage-limits"
        );
        assert_eq!(
            super::super::cline::plan_url(),
            "https://api.cline.bot/api/v1/users/me/plan"
        );
    }

    #[test]
    fn the_chatgpt_usage_endpoint_follows_codexs_path_style() {
        // The shipped chat base: cut after `/backend-api`.
        assert_eq!(
            chatgpt_usage_url("https://chatgpt.com/backend-api/codex"),
            "https://chatgpt.com/backend-api/wham/usage"
        );
        assert_eq!(
            chatgpt_usage_url("http://127.0.0.1:9000/backend-api/codex/"),
            "http://127.0.0.1:9000/backend-api/wham/usage"
        );
        // A base with no `/backend-api` is the Codex API's own style.
        assert_eq!(
            chatgpt_usage_url("https://codex.example.com/v1"),
            "https://codex.example.com/v1/api/codex/usage"
        );
    }
}
