//! What a subscription's own server says about its limits — the `/usage`
//! page's data. See `docs/usage.md`.
//!
//! Every subscription this app signs in to meters its use **server-side**,
//! against windows the client cannot see: ChatGPT's 5-hour and weekly
//! limits, Copilot's monthly premium requests, ClinePass's rolling 5-hour,
//! weekly and monthly windows. Counting tokens app-side would only ever
//! guess at them — a request's cost against a plan is the plan's to say —
//! so the page asks each provider instead, and this module is the one
//! place that knows how: the pure parses of each provider's answer into
//! one provider-neutral [`PlanUsage`], unit-tested, and the boundary
//! fetch that makes the requests.
//!
//! The neutral shape is deliberately small. A **limit** is a label and an
//! [`Allowance`] — a share used with its reset, or unlimited; a **balance**
//! is a label and a value (credits left); a **note** is a sentence the
//! provider's answer implies (a limit reached, a plan with nothing metered).
//! The page renders those three lists and nothing provider-specific, so a
//! fifth provider is a parse and a fetch here and no change to the page.

use std::time::Duration;

use serde::Deserialize;

use super::config::ModelConfig;
use super::{AuthScheme, LlmError, Result};
use crate::stream::CancelToken;

/// One subscription's limits, as its own server reported them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PlanUsage {
    /// The plan's name — `Pro`, `Copilot Pro`, `ClinePass` — when the
    /// answer named one.
    pub plan: Option<String>,
    /// The account the answer is about (an email, a login), when it said:
    /// what tells a user with two accounts which one is signed in here.
    pub account: Option<String>,
    /// The metered windows and the unlimited allowances, in the order the
    /// page lists them.
    pub limits: Vec<Limit>,
    /// What the account holds besides its windows — credits left.
    pub balances: Vec<Balance>,
    /// Sentences the answer implies: a limit reached, a spend cap hit.
    pub notes: Vec<Note>,
}

/// One limit: what it is called and how much of it is left.
#[derive(Debug, Clone, PartialEq)]
pub struct Limit {
    /// What the limit applies to when it is not the whole plan — a model
    /// with an allowance of its own (`GPT-5.3-Codex-Spark`). The page shows
    /// it once, as a sub-heading over the rows that share it.
    pub scope: Option<String>,
    /// The row's label — `5-hour limit`, `Weekly limit`, `Premium requests`.
    pub label: String,
    /// How much of it is used.
    pub allowance: Allowance,
}

impl Limit {
    /// A limit on the whole plan.
    #[must_use]
    pub fn new(label: impl Into<String>, allowance: Allowance) -> Self {
        Self {
            scope: None,
            label: label.into(),
            allowance,
        }
    }
}

/// How much of a limit is used.
#[derive(Debug, Clone, PartialEq)]
pub enum Allowance {
    /// A metered allowance: a share used, and when it starts over.
    Metered(Meter),
    /// No cap on this plan — Copilot's paid plans answer chat this way.
    Unlimited,
}

/// A metered allowance's state.
#[derive(Debug, Clone, PartialEq)]
pub struct Meter {
    /// The share used, `0.0..=100.0` — clamped, so an overage a server
    /// reports as `104` reads as full rather than past the bar's end.
    pub used_percent: f64,
    /// When the window starts over, when the answer said.
    pub resets: Option<Reset>,
    /// A count to go beside the share, in the provider's own unit —
    /// `177 of 300 left`, `8,000 of 25,000 credits used`.
    pub detail: Option<String>,
}

impl Meter {
    /// A meter at `used_percent`, clamped into `0..=100` (a `NaN` reads as
    /// nothing used rather than poisoning the bar's arithmetic) and kept to
    /// a hundredth of a percent — what a server's `100 - 31.17` or `1930 /
    /// 2000` leaves past that is float noise, and a share that compares
    /// equal to the one written down is one a test can state.
    #[must_use]
    pub fn new(used_percent: f64, resets: Option<Reset>, detail: Option<String>) -> Self {
        let used_percent = if used_percent.is_nan() {
            0.0
        } else {
            (used_percent.clamp(0.0, 100.0) * 100.0).round() / 100.0
        };
        Self {
            used_percent,
            resets,
            detail,
        }
    }

    /// Is the allowance used up?
    #[must_use]
    pub fn is_spent(&self) -> bool {
        self.used_percent >= 100.0
    }
}

/// When a window starts over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reset {
    /// An instant, in Unix seconds — shown as a local wall-clock time.
    At(i64),
    /// A calendar day with no time of day — Copilot's monthly allowance,
    /// which GitHub states as a date (`2026-11-01`) and shows as one.
    On(chrono::NaiveDate),
}

/// Something the account holds besides its windows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Balance {
    /// The row's label — `Credits`.
    pub label: String,
    /// What it holds, already worded — `62,500`, `Unlimited`, `$4.32`.
    pub value: String,
    /// A dim clause after it — `≈15,625–81,250 messages`.
    pub detail: Option<String>,
}

/// A sentence the answer implies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    /// What to say.
    pub text: String,
    /// Whether it is a warning (amber) rather than a plain fact (dim).
    pub warning: bool,
}

impl Note {
    /// A plain fact about the plan.
    #[must_use]
    pub fn info(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            warning: false,
        }
    }

    /// Something that stops or will stop requests.
    #[must_use]
    pub fn warning(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            warning: true,
        }
    }
}

// ---------------------------------------------------------------------------
// Shared pieces (pure)
// ---------------------------------------------------------------------------

/// A field read when it has the expected shape and ignored when it doesn't
/// — `null`, a string where a number was, an object a newer backend
/// reshaped. Every field of every usage answer is one of these, so one
/// surprising value costs only itself and never the page: these bodies are
/// undocumented and change without notice, and a parse that refused the
/// whole answer over one of them would show nothing where it could show
/// everything else.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum Lenient<T> {
    Value(T),
    Other(serde::de::IgnoredAny),
}

impl<T> Default for Lenient<T> {
    fn default() -> Self {
        Self::Other(serde::de::IgnoredAny)
    }
}

impl<T> Lenient<T> {
    fn value(self) -> Option<T> {
        match self {
            Self::Value(value) => Some(value),
            Self::Other(_) => None,
        }
    }
}

impl Lenient<String> {
    /// The string, trimmed, when it says anything.
    fn text(self) -> Option<String> {
        self.value()
            .map(|text| text.trim().to_string())
            .filter(|text| !text.is_empty())
    }
}

/// Parse a usage answer's top-level object. A body that is not one — an
/// HTML error page from an intercepting proxy, an array — is a decode
/// error; the object check is explicit because a derived struct also
/// accepts a JSON *array* of its fields, and every field here is lenient,
/// so `[1, 2]` would otherwise parse as an answer with nothing in it.
fn parse_object<'a, T: Deserialize<'a>>(body: &'a str) -> Result<T> {
    if !body.trim_start().starts_with('{') {
        return Err(LlmError::Decode(
            "the usage answer is not a JSON object".to_string(),
        ));
    }
    serde_json::from_str(body).map_err(|e| LlmError::Decode(e.to_string()))
}

/// `n` with its thousands grouped — `62,500` — the way codex words a
/// credit count (`format_with_separators`).
#[must_use]
pub fn group_thousands(n: i64) -> String {
    let digits = n.unsigned_abs().to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3 + 1);
    if n < 0 {
        grouped.push('-');
    }
    for (i, digit) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped
}

/// A whole number read off a decimal string — `"62500.0000000000"` →
/// `62500` — rounded, or `None` for anything that is not a finite number.
fn whole_number(text: &str) -> Option<i64> {
    let value: f64 = text.trim().parse().ok()?;
    // Saturating: a count past i64 is no count a person reads anyway.
    value.is_finite().then(|| value.round() as i64)
}

/// The first letter upper-cased — `weekly` → `Weekly` — for a label that
/// opens a row.
fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// The lengths a window is called by name, and the names: codex's
/// `get_limits_duration` set, spelled out.
const NAMED_WINDOWS: [(i64, &str); 5] = [
    (5 * 3_600, "5-hour"),
    (86_400, "daily"),
    (7 * 86_400, "weekly"),
    (30 * 86_400, "monthly"),
    (365 * 86_400, "annual"),
];

/// What a window `seconds` long is called — `5-hour`, `weekly` — lower-case,
/// for the caller to place in a label. A length within 5% of a named one
/// takes its name (codex's `is_approximate_window`, since a server rounds);
/// any other is said as a length (`2-hour`, `30-minute`, `3-day`) rather
/// than codex's generic "usage", which says nothing about when it ends.
/// `None` for a length that is no length at all.
#[must_use]
pub fn window_name(seconds: i64) -> Option<String> {
    if seconds <= 0 {
        return None;
    }
    let near = |expected: i64| {
        let (seconds, expected) = (seconds as f64, expected as f64);
        seconds >= expected * 0.95 && seconds <= expected * 1.05
    };
    if let Some((_, name)) = NAMED_WINDOWS.iter().find(|(length, _)| near(*length)) {
        return Some((*name).to_string());
    }
    let minutes = (seconds + 59) / 60;
    Some(if minutes < 60 {
        format!("{minutes}-minute")
    } else if seconds < 2 * 86_400 {
        format!("{}-hour", (seconds + 1_800) / 3_600)
    } else {
        format!("{}-day", (seconds + 43_200) / 86_400)
    })
}

// ---------------------------------------------------------------------------
// ChatGPT (docs/chatgpt.md): GET {…}/backend-api/wham/usage
// ---------------------------------------------------------------------------

/// The usage endpoint of the ChatGPT backend whose Codex base is `api_base`.
///
/// `/wham/usage` is a **sibling** of the Responses API's `/codex` under
/// `/backend-api`, not under it — codex's `PathStyle::ChatGptApi`, which it
/// picks whenever the base contains `/backend-api`; any other base is the
/// Codex API's own `/api/codex/usage` (`PathStyle::CodexApi`). A bare
/// `chatgpt.com` gets its `/backend-api` back, codex's normalization.
#[must_use]
pub fn chatgpt_usage_url(api_base: &str) -> String {
    let trimmed = api_base.trim().trim_end_matches('/');
    let mut base = trimmed
        .strip_suffix("/codex")
        .unwrap_or(trimmed)
        .to_string();
    let chatgpt_host = ["https://chatgpt.com", "https://chat.openai.com"]
        .iter()
        .any(|host| base.eq_ignore_ascii_case(host));
    if chatgpt_host {
        base.push_str("/backend-api");
    }
    if base.contains("/backend-api") {
        format!("{base}/wham/usage")
    } else {
        format!("{base}/api/codex/usage")
    }
}

/// `GET /wham/usage`'s answer, as far as the page reads it — codex's
/// `RateLimitStatusPayload` with every field lenient.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ChatGptUsageWire {
    email: Lenient<String>,
    plan_type: Lenient<String>,
    rate_limit: Lenient<ChatGptRateLimit>,
    additional_rate_limits: Lenient<Vec<ChatGptAdditionalLimit>>,
    credits: Lenient<ChatGptCredits>,
    spend_control: Lenient<ChatGptSpendControl>,
    rate_limit_reached_type: Lenient<ChatGptReachedType>,
}

/// A bucket's windows: the short one first (`primary`), then the long one.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ChatGptRateLimit {
    limit_reached: Lenient<bool>,
    primary_window: Lenient<ChatGptWindow>,
    secondary_window: Lenient<ChatGptWindow>,
}

/// One window. Numbers are read as `f64` whatever the wire sends — codex's
/// model says `i32`, its header path already parses `12.5`.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ChatGptWindow {
    used_percent: Lenient<f64>,
    limit_window_seconds: Lenient<f64>,
    reset_after_seconds: Lenient<f64>,
    reset_at: Lenient<f64>,
}

/// A bucket beyond the main one — a model with its own allowance.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ChatGptAdditionalLimit {
    limit_name: Lenient<String>,
    metered_feature: Lenient<String>,
    rate_limit: Lenient<ChatGptRateLimit>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ChatGptCredits {
    has_credits: Lenient<bool>,
    unlimited: Lenient<bool>,
    balance: Lenient<String>,
    approx_local_messages: Lenient<Vec<Lenient<f64>>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ChatGptSpendControl {
    reached: Lenient<bool>,
    individual_limit: Lenient<ChatGptSpendLimit>,
}

/// A workspace's monthly credit cap for this member.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ChatGptSpendLimit {
    limit: Lenient<String>,
    used: Lenient<String>,
    used_percent: Lenient<f64>,
    remaining_percent: Lenient<f64>,
    reset_after_seconds: Lenient<f64>,
    reset_at: Lenient<f64>,
}

/// `{"type": kind}` — why requests are being refused.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ChatGptReachedType {
    #[serde(rename = "type")]
    kind: Lenient<String>,
}

/// The reset a window names: its instant (`reset_at`, the field codex
/// reads), else the seconds left placed against `now`, the moment the
/// answer was fetched.
fn chatgpt_reset(reset_at: Option<f64>, reset_after: Option<f64>, now: i64) -> Option<Reset> {
    let positive = |value: Option<f64>| value.filter(|v| v.is_finite() && *v > 0.0);
    if let Some(at) = positive(reset_at) {
        return Some(Reset::At(at.round() as i64));
    }
    positive(reset_after).map(|after| Reset::At(now.saturating_add(after.round() as i64)))
}

/// One bucket's windows as limits, labelled by their lengths — `5-hour
/// limit`, `Weekly limit` — and scoped to the bucket's name when it has one
/// (a model's own allowance), so the label stays a length the column holds.
fn chatgpt_windows(
    limits: &mut Vec<Limit>,
    scope: Option<&str>,
    primary: Lenient<ChatGptWindow>,
    secondary: Lenient<ChatGptWindow>,
    now: i64,
) {
    let windows = [(primary.value(), "usage"), (secondary.value(), "secondary")];
    for (window, fallback) in windows {
        let Some(window) = window else { continue };
        let length = window
            .limit_window_seconds
            .value()
            .filter(|s| s.is_finite())
            .map_or(0, |s| s.round() as i64);
        let name = window_name(length).unwrap_or_else(|| fallback.to_string());
        let resets = chatgpt_reset(
            window.reset_at.value(),
            window.reset_after_seconds.value(),
            now,
        );
        let used = window.used_percent.value().unwrap_or(0.0);
        limits.push(Limit {
            scope: scope.map(str::to_string),
            label: format!("{} limit", capitalized(&name)),
            allowance: Allowance::Metered(Meter::new(used, resets, None)),
        });
    }
}

/// The `Credits` row, codex's rule: none without credits, `Unlimited`, a
/// whole count, or `Available` when there is no count to give. A counted
/// balance carries the backend's own estimate of what it buys.
fn chatgpt_credits(credits: ChatGptCredits) -> Option<Balance> {
    if !credits.has_credits.value().unwrap_or(false) {
        return None;
    }
    let row = |value: String, detail: Option<String>| Balance {
        label: "Credits".to_string(),
        value,
        detail,
    };
    if credits.unlimited.value().unwrap_or(false) {
        return Some(row("Unlimited".to_string(), None));
    }
    let Some(count) = credits
        .balance
        .text()
        .and_then(|balance| whole_number(&balance))
        .filter(|count| *count > 0)
    else {
        return Some(row("Available".to_string(), None));
    };
    let estimate: Vec<i64> = credits
        .approx_local_messages
        .value()
        .unwrap_or_default()
        .into_iter()
        .filter_map(Lenient::value)
        .filter(|n| n.is_finite() && *n > 0.0)
        .map(|n| n.round() as i64)
        .collect();
    let detail = match estimate.as_slice() {
        [low, high, ..] if low != high => Some(format!(
            "≈{}–{} messages",
            group_thousands(*low),
            group_thousands(*high)
        )),
        [one, ..] => Some(format!("≈{} messages", group_thousands(*one))),
        [] => None,
    };
    Some(row(group_thousands(count), detail))
}

/// A workspace's spend cap for this member as the `Monthly credit limit`
/// meter: its share used — or, when only the share left is given, the rest
/// (codex's `100 - remaining_percent`) — over `{used} of {limit} credits
/// used`.
fn chatgpt_spend_limit(limit: ChatGptSpendLimit, now: i64) -> Limit {
    let used = limit
        .used_percent
        .value()
        .or_else(|| limit.remaining_percent.value().map(|left| 100.0 - left))
        .unwrap_or(0.0);
    let count = |field: Lenient<String>| field.text().and_then(|text| whole_number(&text));
    let detail = match (count(limit.used), count(limit.limit)) {
        (Some(used), Some(cap)) => Some(format!(
            "{} of {} credits used",
            group_thousands(used),
            group_thousands(cap)
        )),
        _ => None,
    };
    let resets = chatgpt_reset(
        limit.reset_at.value(),
        limit.reset_after_seconds.value(),
        now,
    );
    Limit::new(
        "Monthly credit limit",
        Allowance::Metered(Meter::new(used, resets, detail)),
    )
}

/// Why requests are refused, in codex's own sentences
/// (`UsageLimitReachedError`) folded to one row; `None` for the generic
/// kind, which the caller words itself.
fn chatgpt_reached_note(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "workspace_owner_credits_depleted" => {
            "Your workspace is out of credits — add credits to continue."
        }
        "workspace_member_credits_depleted" => {
            "Your workspace is out of credits — ask its owner to refill them."
        }
        "workspace_owner_usage_limit_reached" => {
            "You hit your workspace's spend cap — raise it to continue."
        }
        "workspace_member_usage_limit_reached" => {
            "You hit the spend cap your workspace owner set — ask them to raise it."
        }
        _ => return None,
    })
}

/// Parse `GET /wham/usage`'s answer. `now` (Unix seconds) places a window
/// that says only how long it has left.
///
/// The plan comes from `plan_type` through [`super::chatgpt::plan_label`],
/// the sign-in confirmation's own wording — except a `guest`/`unknown`
/// seat, which codex calls Unknown and which is no plan to name. The main
/// bucket's windows come first, then a workspace's monthly credit cap — the
/// plan-wide rows — then each named bucket's under its own name; credits
/// are the one balance.
///
/// # Errors
/// A body that is not a JSON object is a decode error. Every field inside
/// one is optional: what is missing or malformed is simply not shown.
pub fn parse_chatgpt(body: &str, now: i64) -> Result<PlanUsage> {
    let wire: ChatGptUsageWire = parse_object(body)?;
    let plan = wire
        .plan_type
        .text()
        .filter(|plan| !matches!(plan.to_ascii_lowercase().as_str(), "guest" | "unknown"))
        .map(|plan| super::chatgpt::plan_label(&plan));
    let mut limits = Vec::new();
    let mut limit_reached = false;
    if let Some(rate) = wire.rate_limit.value() {
        limit_reached = rate.limit_reached.value().unwrap_or(false);
        chatgpt_windows(
            &mut limits,
            None,
            rate.primary_window,
            rate.secondary_window,
            now,
        );
    }
    // The workspace's credit cap is the whole plan's, so it lists with the
    // main windows, before any named bucket's rows.
    let spend = wire.spend_control.value().unwrap_or_default();
    let spend_reached = spend.reached.value().unwrap_or(false);
    if let Some(cap) = spend.individual_limit.value() {
        limits.push(chatgpt_spend_limit(cap, now));
    }
    for extra in wire.additional_rate_limits.value().unwrap_or_default() {
        let name = extra
            .limit_name
            .text()
            .or_else(|| extra.metered_feature.text());
        if let (Some(name), Some(rate)) = (name, extra.rate_limit.value()) {
            chatgpt_windows(
                &mut limits,
                Some(&name),
                rate.primary_window,
                rate.secondary_window,
                now,
            );
        }
    }
    let balances = wire
        .credits
        .value()
        .and_then(chatgpt_credits)
        .into_iter()
        .collect();
    let kind = wire
        .rate_limit_reached_type
        .value()
        .and_then(|reached| reached.kind.text())
        .unwrap_or_default();
    let mut notes = Vec::new();
    match chatgpt_reached_note(&kind) {
        Some(text) => notes.push(Note::warning(text)),
        None if limit_reached || kind == "rate_limit_reached" => notes.push(Note::warning(
            "A limit is reached — requests wait for it to reset.",
        )),
        None => {}
    }
    // The usage-limit kinds already name the spend cap.
    if spend_reached && !kind.ends_with("_usage_limit_reached") {
        notes.push(Note::warning("Your spend cap is reached."));
    }
    Ok(PlanUsage {
        plan,
        account: wire.email.text(),
        limits,
        balances,
        notes,
    })
}

// ---------------------------------------------------------------------------
// GitHub Copilot (docs/copilot.md): GET api.github.com/copilot_internal/user
// ---------------------------------------------------------------------------

/// `GET /copilot_internal/user`'s answer, as far as the page reads it — VS
/// Code's `IEntitlementsData`, every field lenient.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct CopilotUserWire {
    access_type_sku: Lenient<String>,
    copilot_plan: Lenient<String>,
    chat_enabled: Lenient<bool>,
    can_signup_for_limited: Lenient<bool>,
    token_based_billing: Lenient<bool>,
    quota_reset_date: Lenient<String>,
    quota_reset_date_utc: Lenient<String>,
    limited_user_reset_date: Lenient<String>,
    quota_snapshots: Lenient<CopilotSnapshots>,
    limited_user_quotas: Lenient<CopilotLegacyQuotas>,
    monthly_quotas: Lenient<CopilotLegacyQuotas>,
}

/// The three allowances, keyed by what they meter.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct CopilotSnapshots {
    premium_interactions: Lenient<CopilotSnapshot>,
    chat: Lenient<CopilotSnapshot>,
    completions: Lenient<CopilotSnapshot>,
}

/// One allowance (VS Code's `IQuotaSnapshotData`).
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct CopilotSnapshot {
    entitlement: Lenient<Number>,
    quota_remaining: Lenient<Number>,
    remaining: Lenient<Number>,
    percent_remaining: Lenient<f64>,
    unlimited: Lenient<bool>,
    has_quota: Lenient<bool>,
    overage_count: Lenient<Number>,
    quota_reset_at: Lenient<f64>,
    credits_used: Lenient<Number>,
}

/// The Free plan's older shape: what is left, and the month's allowance.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct CopilotLegacyQuotas {
    chat: Lenient<f64>,
    completions: Lenient<f64>,
}

/// A number, or a string holding one — VS Code's types say `entitlement:
/// string` while real answers send a number, and its client parses both.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum Number {
    Value(f64),
    Text(String),
}

/// A lenient number field's finite value.
fn number(field: Lenient<Number>) -> Option<f64> {
    let value = match field.value()? {
        Number::Value(value) => value,
        Number::Text(text) => text.trim().parse().ok()?,
    };
    value.is_finite().then_some(value)
}

/// A reset given as text: an RFC 3339 instant (`2026-11-01T00:00:00.000Z`),
/// or a calendar day alone (`2026-11-01`), which is shown as the day.
fn reset_from_text(text: &str) -> Option<Reset> {
    let text = text.trim();
    if let Ok(at) = chrono::DateTime::parse_from_rfc3339(text) {
        return Some(Reset::At(at.timestamp()));
    }
    chrono::NaiveDate::parse_from_str(text, "%Y-%m-%d")
        .ok()
        .map(Reset::On)
}

/// Words joined by `_`/`-`, title-cased — a plan slug this build does not
/// know still reads like a name (`team_trial` → `Team Trial`).
fn title_words(raw: &str) -> String {
    raw.split(['_', '-'])
        .filter(|word| !word.is_empty())
        .map(capitalized)
        .collect::<Vec<_>>()
        .join(" ")
}

/// The plan's name, VS Code's `resolveEntitlements` order: the SKU first,
/// because a Free or student seat reports `copilot_plan: "individual"`
/// too, then the plan.
fn copilot_plan(sku: Option<&str>, plan: Option<&str>) -> Option<String> {
    match sku {
        Some("free_limited_copilot") => return Some("Free".to_string()),
        Some("free_educational_quota") => return Some("Student".to_string()),
        _ => {}
    }
    let plan = plan?;
    Some(
        match plan {
            "individual" => "Pro",
            "individual_pro" => "Pro+",
            "individual_max" => "Max",
            "individual_edu" => "Student",
            "business" => "Business",
            "enterprise" => "Enterprise",
            other => return Some(title_words(other)),
        }
        .to_string(),
    )
}

/// A count to read — one decimal only when it has one (a premium request
/// at a model's 0.33× multiplier leaves a fraction), grouped, never below
/// zero.
#[must_use]
pub fn count_text(value: f64) -> String {
    let tenths = if value.is_finite() {
        (value.max(0.0) * 10.0).round() as i64
    } else {
        0
    };
    if tenths % 10 == 0 {
        group_thousands(tenths / 10)
    } else {
        format!("{}.{}", group_thousands(tenths / 10), tenths % 10)
    }
}

/// What one allowance is on the page: a row of its own, or — an unlimited
/// one that counts what it spent, with no denominator — a balance.
enum CopilotRow {
    Limit(Limit),
    Balance(Balance),
}

/// One allowance by VS Code's rules (`parseQuotas`, `getQuotaUsage`,
/// `getQuotaReset`): skipped when it is metered but entitles nothing (the
/// Free plan's premium requests); unlimited, a depleted organization pool,
/// or credits spent when unlimited; else the share used is `100 −
/// percent_remaining`, over what is left of the entitlement. The
/// snapshot's own reset governs it, else the account's.
fn copilot_row(
    label: &str,
    snapshot: CopilotSnapshot,
    account_reset: Option<Reset>,
) -> Option<CopilotRow> {
    let unlimited = snapshot.unlimited.value().unwrap_or(false);
    let entitlement = number(snapshot.entitlement).filter(|e| *e >= 0.0);
    if !unlimited && entitlement == Some(0.0) {
        return None;
    }
    let resets = snapshot
        .quota_reset_at
        .value()
        .filter(|at| at.is_finite() && *at > 0.0)
        .map(|at| Reset::At(at.round() as i64))
        .or(account_reset);
    if unlimited {
        if snapshot.has_quota.value() == Some(false) {
            return Some(CopilotRow::Limit(Limit::new(
                label,
                Allowance::Metered(Meter::new(
                    100.0,
                    resets,
                    Some("organization limit reached".to_string()),
                )),
            )));
        }
        if let Some(spent) = number(snapshot.credits_used).filter(|n| *n >= 0.0) {
            return Some(CopilotRow::Balance(Balance {
                label: label.to_string(),
                value: format!("{} credits used", count_text(spent)),
                detail: None,
            }));
        }
        return Some(CopilotRow::Limit(Limit::new(label, Allowance::Unlimited)));
    }
    let left = snapshot
        .percent_remaining
        .value()
        .unwrap_or(100.0)
        .clamp(0.0, 100.0);
    let total = entitlement.filter(|e| *e > 0.0);
    let detail = total.map(|total| {
        let remaining = number(snapshot.quota_remaining)
            .or_else(|| number(snapshot.remaining))
            .unwrap_or(total * left / 100.0);
        let mut text = format!("{} of {} left", count_text(remaining), count_text(total));
        if let Some(over) = number(snapshot.overage_count).filter(|n| *n > 0.0) {
            text.push_str(&format!(" · {} over", count_text(over)));
        }
        text
    });
    Some(CopilotRow::Limit(Limit::new(
        label,
        Allowance::Metered(Meter::new(100.0 - left, resets, detail)),
    )))
}

/// The Free plan's older allowance: what is left of the month's.
fn copilot_legacy_row(
    label: &str,
    left: Option<f64>,
    total: Option<f64>,
    resets: Option<Reset>,
) -> Option<Limit> {
    let total = total.filter(|t| t.is_finite() && *t > 0.0)?;
    let left = left.filter(|l| l.is_finite())?.max(0.0);
    let used = 100.0 - (left / total * 100.0).clamp(0.0, 100.0);
    Some(Limit::new(
        label,
        Allowance::Metered(Meter::new(
            used,
            resets,
            Some(format!(
                "{} of {} left",
                count_text(left),
                count_text(total)
            )),
        )),
    ))
}

/// Parse `GET /copilot_internal/user`'s answer: the plan, then the premium
/// requests (**AI Credits** on a `token_based_billing` account — the June
/// 2026 billing), chat messages and inline suggestions, by VS Code's rules.
/// An account with no plan, or with chat turned off, says so as a note.
///
/// # Errors
/// A body that is not a JSON object is a decode error; every field inside
/// one is optional.
pub fn parse_copilot(body: &str) -> Result<PlanUsage> {
    let wire: CopilotUserWire = parse_object(body)?;
    let plan = copilot_plan(
        wire.access_type_sku.text().as_deref(),
        wire.copilot_plan.text().as_deref(),
    );
    let account_reset = wire
        .quota_reset_date_utc
        .text()
        .and_then(|text| reset_from_text(&text))
        .or_else(|| {
            wire.quota_reset_date
                .text()
                .and_then(|text| reset_from_text(&text))
        })
        .or_else(|| {
            wire.limited_user_reset_date
                .text()
                .and_then(|text| reset_from_text(&text))
        });
    let premium = if wire.token_based_billing.value().unwrap_or(false) {
        "AI Credits"
    } else {
        "Premium requests"
    };
    let mut limits = Vec::new();
    let mut balances = Vec::new();
    match wire.quota_snapshots.value() {
        Some(snapshots) => {
            let rows = [
                (premium, snapshots.premium_interactions.value()),
                ("Chat messages", snapshots.chat.value()),
                ("Inline suggestions", snapshots.completions.value()),
            ];
            for (label, snapshot) in rows {
                match snapshot.and_then(|s| copilot_row(label, s, account_reset)) {
                    Some(CopilotRow::Limit(limit)) => limits.push(limit),
                    Some(CopilotRow::Balance(balance)) => balances.push(balance),
                    None => {}
                }
            }
        }
        None => {
            let left = wire.limited_user_quotas.value().unwrap_or_default();
            let total = wire.monthly_quotas.value().unwrap_or_default();
            limits.extend(copilot_legacy_row(
                "Chat messages",
                left.chat.value(),
                total.chat.value(),
                account_reset,
            ));
            limits.extend(copilot_legacy_row(
                "Inline suggestions",
                left.completions.value(),
                total.completions.value(),
                account_reset,
            ));
        }
    }
    let mut notes = Vec::new();
    if plan.is_none() && wire.can_signup_for_limited.value().unwrap_or(false) {
        notes.push(Note::warning(
            "This GitHub account has no Copilot plan yet.",
        ));
    } else if wire.chat_enabled.value() == Some(false) {
        notes.push(Note::warning(
            "Copilot chat is turned off for this account.",
        ));
    }
    Ok(PlanUsage {
        plan,
        account: None,
        limits,
        balances,
        notes,
    })
}

// ---------------------------------------------------------------------------
// ClinePass (docs/cline.md): GET {api}/users/me/plan/usage-limits
// ---------------------------------------------------------------------------

/// The `{success, data}` envelope every Cline answer comes in.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ClineEnvelope<T> {
    success: Lenient<bool>,
    data: Lenient<T>,
    error: Lenient<String>,
    message: Lenient<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ClineLimitsData {
    limits: Lenient<Vec<Lenient<ClineWindow>>>,
}

/// One window — `five_hour`, `weekly`, `monthly` — and how much is used.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ClineWindow {
    #[serde(rename = "type")]
    kind: Lenient<String>,
    #[serde(rename = "percentUsed")]
    percent_used: Lenient<f64>,
    #[serde(rename = "resetsAt")]
    resets_at: Lenient<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ClinePlanData {
    plan: Lenient<ClinePlan>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ClinePlan {
    #[serde(rename = "displayName")]
    display_name: Lenient<String>,
    name: Lenient<String>,
    entitlements: Lenient<ClineEntitlements>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ClineEntitlements {
    cline_pass: Lenient<ClinePassEntitlement>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ClinePassEntitlement {
    enabled: Lenient<bool>,
    #[serde(rename = "inferenceCapThreshold")]
    caps: Lenient<ClinePassCaps>,
}

/// Each window's cap, in units of 1e-8 USD (`1000000000` is $10).
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ClinePassCaps {
    #[serde(rename = "last5HoursUsageCostUSDPerUser")]
    five_hour: Lenient<f64>,
    #[serde(rename = "last7daysUsageCostUSDPerUser")]
    weekly: Lenient<f64>,
    #[serde(rename = "last30daysUsageCostUSDPerUser")]
    monthly: Lenient<f64>,
}

/// What a ClinePass cap is in dollars: the plan states it in 1e-8 USD.
const CLINEPASS_CAP_UNIT: f64 = 1e-8;

/// A dollar amount to read — `$10`, `$12.50`, `$1,500` — the cents dropped
/// only when the amount is whole (`$0.00` is a sliver, not nothing).
#[must_use]
pub fn dollars(amount: f64) -> String {
    let amount = if amount.is_finite() {
        amount.max(0.0)
    } else {
        0.0
    };
    if (amount - amount.round()).abs() < 1e-9 {
        return format!("${}", group_thousands(amount.round() as i64));
    }
    let cents = (amount * 100.0).round() as i64;
    format!("${}.{:02}", group_thousands(cents / 100), cents % 100)
}

/// The page for a signed-in Cline account that has no ClinePass — the
/// usage request's 404 — as the sentence that says what to do about it.
#[must_use]
pub fn clinepass_unsubscribed() -> PlanUsage {
    PlanUsage {
        notes: vec![Note::warning(
            "This Cline account has no ClinePass subscription — \
             app.cline.bot/dashboard/subscription subscribes.",
        )],
        ..PlanUsage::default()
    }
}

/// Parse ClinePass's windows (`limits_body`, `GET /users/me/plan/usage-
/// limits`) with the plan's name and per-window dollar caps
/// (`plan_body`, `GET /users/me/plan`, best-effort — without it the windows
/// still stand). The windows come in the answer's order, a type this build
/// does not know skipped (`experimental_pool`).
///
/// # Errors
/// A windows body that is not a JSON object is a decode error; an envelope
/// that says `success: false` is the server's own words.
pub fn parse_clinepass(limits_body: &str, plan_body: Option<&str>) -> Result<PlanUsage> {
    let envelope: ClineEnvelope<ClineLimitsData> = parse_object(limits_body)?;
    if envelope.success.value() == Some(false) {
        let said = envelope
            .error
            .text()
            .or_else(|| envelope.message.text())
            .unwrap_or_else(|| "Cline refused the usage request".to_string());
        return Err(LlmError::Api {
            status: 0,
            body: said,
        });
    }
    let plan = plan_body
        .and_then(|body| parse_object::<ClineEnvelope<ClinePlanData>>(body).ok())
        .and_then(|envelope| envelope.data.value())
        .and_then(|data| data.plan.value());
    let (name, entitlement) = match plan {
        Some(plan) => {
            let name = plan.display_name.text().or_else(|| plan.name.text());
            let entitlement = plan.entitlements.value().and_then(|e| e.cline_pass.value());
            (name, entitlement)
        }
        None => (None, None),
    };
    let (enabled, caps) = match entitlement {
        Some(entitlement) => (
            entitlement.enabled.value(),
            entitlement.caps.value().unwrap_or_default(),
        ),
        None => (None, ClinePassCaps::default()),
    };
    let cap = |field: Lenient<f64>| {
        field
            .value()
            .filter(|cap| cap.is_finite() && *cap > 0.0)
            .map(|cap| format!("{} cap", dollars(cap * CLINEPASS_CAP_UNIT)))
    };
    let (mut five_hour, mut weekly, mut monthly) =
        (cap(caps.five_hour), cap(caps.weekly), cap(caps.monthly));
    let mut limits = Vec::new();
    let windows = envelope
        .data
        .value()
        .and_then(|data| data.limits.value())
        .unwrap_or_default();
    for window in windows.into_iter().filter_map(Lenient::value) {
        let (label, detail) = match window.kind.text().as_deref() {
            Some("five_hour") => ("5-hour limit", five_hour.take()),
            Some("weekly") => ("Weekly limit", weekly.take()),
            Some("monthly") => ("Monthly limit", monthly.take()),
            _ => continue,
        };
        let resets = window
            .resets_at
            .text()
            .and_then(|text| reset_from_text(&text));
        let used = window.percent_used.value().unwrap_or(0.0);
        limits.push(Limit::new(
            label,
            Allowance::Metered(Meter::new(used, resets, detail)),
        ));
    }
    let mut notes = Vec::new();
    if enabled == Some(false) {
        notes.push(Note::warning("This Cline plan doesn't include ClinePass."));
    }
    Ok(PlanUsage {
        plan: name,
        account: None,
        limits,
        balances: Vec::new(),
        notes,
    })
}

// ---------------------------------------------------------------------------
// Anthropic Console (docs/claude.md): nothing to fetch
// ---------------------------------------------------------------------------

/// The Console sign-in's row. It bills per token to the account's API
/// organisation — per-minute rate limits and a monthly spend cap, readable
/// only with an organisation admin's key — and the endpoint that reports
/// Claude subscription windows belongs to the Claude.ai sign-in, which this
/// app does not offer (`docs/claude.md`). So there is no request: the row
/// says what the sign-in is.
#[must_use]
pub fn console_usage() -> PlanUsage {
    PlanUsage {
        notes: vec![Note::info(
            "Billed per token to your API organization — there's no plan limit to show.",
        )],
        ..PlanUsage::default()
    }
}

// ---------------------------------------------------------------------------
// The boundary: one request (or two) per subscription
// ---------------------------------------------------------------------------

/// Per-request deadline — codex's own for these reads. A usage answer is a
/// small JSON object; a request still out after this is a dead one.
const OP_TIMEOUT: Duration = Duration::from_secs(15);

/// The most a usage answer may buffer — far past any real one (a few KB),
/// and a bound on what a proxy's error page can make the page hold.
const BODY_MAX_BYTES: u64 = 256 * 1024;

/// The longest provider message a refusal row quotes.
const REFUSAL_MESSAGE_MAX_CHARS: usize = 160;

/// The message fields an error body may carry: GitHub's `message`, OpenAI's
/// `detail` and `error.message`, an OAuth `error_description`, Cline's
/// `error` string.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ErrorWire {
    message: Lenient<String>,
    detail: Lenient<String>,
    error_description: Lenient<String>,
    error: Lenient<ErrorField>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ErrorField {
    Text(String),
    Object {
        #[serde(default)]
        message: Lenient<String>,
    },
}

/// What a refusal's body says, on one line — `None` for a body that names
/// nothing (an HTML page from a proxy, an empty answer).
fn error_message(body: &str) -> Option<String> {
    let wire: ErrorWire = parse_object(body).ok()?;
    let nested = match wire.error.value() {
        Some(ErrorField::Text(text)) => Some(text),
        Some(ErrorField::Object { message }) => message.value(),
        None => None,
    };
    let text = wire
        .message
        .text()
        .or_else(|| wire.detail.text())
        .or_else(|| wire.error_description.text())
        .or(nested)?;
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.is_empty() {
        return None;
    }
    Some(match flat.char_indices().nth(REFUSAL_MESSAGE_MAX_CHARS) {
        Some((cut, _)) => format!("{}…", &flat[..cut]),
        None => flat,
    })
}

/// A refused usage request as the page's row: what the status means for
/// the user — a sign-in to redo, a pause before trying again — and the
/// provider's own reason when its body gave one. `provider` is the row's
/// name (`GitHub Copilot`).
#[must_use]
pub fn refusal(provider: &str, status: u16, body: &str) -> String {
    let said = error_message(body);
    match (status, said) {
        (401, _) => format!("{provider} refused the sign-in — /login signs in again."),
        (429, _) => {
            format!("{provider} is limiting how often usage is read — try again in a moment.")
        }
        (403, Some(said)) => format!("{provider} refused the request (HTTP 403): {said}"),
        (403, None) => format!("{provider} refused the request (HTTP 403)."),
        (status, Some(said)) => format!("{provider} answered HTTP {status}: {said}"),
        (status, None) => format!("{provider} answered HTTP {status}."),
    }
}

/// The wall clock in Unix seconds — what a window that says only how long
/// it has left is placed against. Boundary code.
fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

/// One GET, bounded, as `(status, body)`. Boundary code.
fn get(url: &str, headers: &[(String, String)]) -> Result<(u16, String)> {
    use std::io::Read;
    let client = super::http_client(OP_TIMEOUT)?;
    let mut request = client.get(url).header("accept", "application/json");
    for (name, value) in headers {
        request = request.header(name.as_str(), value.as_str());
    }
    let response = request.send().map_err(|e| LlmError::transport(&e))?;
    let status = response.status().as_u16();
    let mut body = String::new();
    response
        .take(BODY_MAX_BYTES)
        .read_to_string(&mut body)
        .map_err(|e| LlmError::Http(e.to_string()))?;
    Ok((status, body))
}

/// A 2xx answer's body, or the refusal worded for the page.
fn answered(provider: &str, (status, body): (u16, String)) -> Result<String> {
    if (200..300).contains(&status) {
        Ok(body)
    } else {
        Err(LlmError::Api {
            status: 0,
            body: refusal(provider, status, &body),
        })
    }
}

/// The headers a signed-in request carries — the bearer the auth seam
/// resolved, the provider's own, then what the credential implies — or the
/// sentence for a sign-in with nothing stored.
fn signed_in(cfg: &ModelConfig) -> Result<Vec<(String, String)>> {
    let auth = super::auth::request_auth(cfg)?;
    let bearer = auth.bearer.ok_or_else(|| not_signed_in(cfg))?;
    let mut headers = vec![("authorization".to_string(), format!("Bearer {bearer}"))];
    headers.extend(cfg.extra_headers.iter().cloned());
    headers.extend(auth.headers);
    Ok(headers)
}

/// The error for a subscription with no credential stored.
fn not_signed_in(cfg: &ModelConfig) -> LlmError {
    LlmError::Api {
        status: 0,
        body: format!("{} is not signed in — /login signs in.", cfg.provider_name),
    }
}

/// Read the plan usage of the subscription `cfg` is signed in to — the
/// `/usage` page's one entry point (`docs/usage.md`). Boundary code: one
/// request for ChatGPT and Copilot, two for ClinePass, none for the
/// Anthropic Console. The credential resolves through the seams a turn
/// uses, so a rotated refresh token is written back exactly as a turn
/// writes it. `cancel` is honoured between requests (the page closed).
///
/// # Errors
/// A sign-in that cannot be refreshed, a transport failure, a refusal
/// (worded by [`refusal`]), or an answer that is not the usage object.
pub fn fetch(cfg: &ModelConfig, cancel: &CancelToken) -> Result<PlanUsage> {
    if cancel.is_cancelled() {
        return Err(LlmError::Cancelled);
    }
    let provider = cfg.provider_name.as_str();
    match cfg.auth {
        AuthScheme::ChatGptCodex => {
            let headers = signed_in(cfg)?;
            let body = answered(provider, get(&chatgpt_usage_url(&cfg.api_base), &headers)?)?;
            parse_chatgpt(&body, unix_now())
        }
        AuthScheme::GithubCopilot => {
            // GitHub's API takes the stored OAuth token itself, never the
            // exchanged Copilot bearer — no exchange is spent on this.
            let token = cfg
                .api_key
                .as_deref()
                .filter(|token| !token.is_empty())
                .ok_or_else(|| not_signed_in(cfg))?;
            let headers = [
                ("authorization", format!("token {token}")),
                ("editor-version", super::copilot::EDITOR_VERSION.to_string()),
                (
                    "editor-plugin-version",
                    super::copilot::EDITOR_PLUGIN_VERSION.to_string(),
                ),
                ("user-agent", super::copilot::USER_AGENT.to_string()),
                ("x-github-api-version", "2025-04-01".to_string()),
            ]
            .map(|(name, value)| (name.to_string(), value));
            let url = super::copilot::github_api(super::copilot::USER_PATH);
            let body = answered(provider, get(&url, &headers)?)?;
            parse_copilot(&body)
        }
        AuthScheme::ClineAccount => {
            let headers = signed_in(cfg)?;
            let base = cfg.api_base.trim().trim_end_matches('/');
            let (status, body) = get(&format!("{base}/users/me/plan/usage-limits"), &headers)?;
            // Signed in, but no plan: the sentence, not an error.
            if status == 404 {
                return Ok(clinepass_unsubscribed());
            }
            let limits = answered(provider, (status, body))?;
            let plan = if cancel.is_cancelled() {
                None
            } else {
                get(&format!("{base}/users/me/plan"), &headers)
                    .ok()
                    .filter(|(status, _)| (200..300).contains(status))
                    .map(|(_, body)| body)
            };
            parse_clinepass(&limits, plan.as_deref())
        }
        AuthScheme::AnthropicConsole => Ok(console_usage()),
        AuthScheme::ApiKey | AuthScheme::OptionalKey => Err(LlmError::Api {
            status: 0,
            body: format!("{provider} is reached with a key — there is no plan to read."),
        }),
    }
}

/// Where the provider itself shows these numbers, with history — the link
/// under the highlighted subscription. `None` for a scheme with no plan.
#[must_use]
pub fn dashboard_url(scheme: AuthScheme) -> Option<&'static str> {
    match scheme {
        // codex's `/status` card's own link.
        AuthScheme::ChatGptCodex => Some("https://chatgpt.com/settings/usage"),
        AuthScheme::GithubCopilot => Some("https://github.com/settings/copilot"),
        // docs/cline.md: the subscription dashboard Cline's clients point at.
        AuthScheme::ClineAccount => {
            Some("https://app.cline.bot/dashboard/subscription?personal=true")
        }
        AuthScheme::AnthropicConsole => Some("https://platform.claude.com/usage"),
        AuthScheme::ApiKey | AuthScheme::OptionalKey => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The body `GET /backend-api/wham/usage` answered for a live Pro seat
    /// (October 2026), identifiers redacted: one weekly window, no second,
    /// and a credits balance with its message estimates.
    const CHATGPT_PRO: &str = r#"{
        "user_id": "user-1", "account_id": "acct-1", "email": "dev@example.com",
        "plan_type": "pro",
        "rate_limit": {"allowed": true, "limit_reached": false,
            "primary_window": {"used_percent": 0, "limit_window_seconds": 604800,
                "reset_after_seconds": 541264, "reset_at": 1793521139},
            "secondary_window": null},
        "code_review_rate_limit": null,
        "additional_rate_limits": null,
        "model_usage": {"gpt-6-astra": {"available": true, "available_at": null,
            "credits_would_enable": false}},
        "credits": {"has_credits": true, "unlimited": false, "overage_limit_reached": false,
            "balance": "62500.0000000000", "approx_local_messages": [15625, 81250],
            "approx_cloud_messages": [2500, 15625]},
        "spend_control": {"reached": false, "individual_limit": null},
        "rate_limit_reached_type": null,
        "promo": null,
        "rate_limit_reset_credits": {"available_count": 2, "applicable_available_count": 0}
    }"#;

    #[test]
    fn a_chatgpt_answer_names_its_plan_account_window_and_credits() {
        let usage = parse_chatgpt(CHATGPT_PRO, 0).unwrap();
        assert_eq!(usage.plan.as_deref(), Some("Pro"));
        assert_eq!(usage.account.as_deref(), Some("dev@example.com"));
        assert_eq!(
            usage.limits,
            vec![Limit::new(
                "Weekly limit",
                Allowance::Metered(Meter::new(0.0, Some(Reset::At(1_793_521_139)), None)),
            )]
        );
        assert_eq!(
            usage.balances,
            vec![Balance {
                label: "Credits".to_string(),
                value: "62,500".to_string(),
                detail: Some("≈15,625–81,250 messages".to_string()),
            }]
        );
        assert!(usage.notes.is_empty(), "{:?}", usage.notes);
    }

    #[test]
    fn both_windows_list_in_the_order_the_server_sent_them() {
        // Plus: the 5-hour window is the primary, the week the secondary —
        // the short one first, codex's order.
        let body = r#"{"plan_type": "plus", "rate_limit": {"allowed": true, "limit_reached": false,
            "primary_window": {"used_percent": 42, "limit_window_seconds": 18000,
                "reset_after_seconds": 120, "reset_at": 1735689720},
            "secondary_window": {"used_percent": 5, "limit_window_seconds": 604800,
                "reset_after_seconds": 43200, "reset_at": 1735693200}}}"#;
        let usage = parse_chatgpt(body, 0).unwrap();
        let labels: Vec<_> = usage.limits.iter().map(|l| l.label.as_str()).collect();
        assert_eq!(labels, ["5-hour limit", "Weekly limit"]);
        assert_eq!(
            usage.limits[0].allowance,
            Allowance::Metered(Meter::new(42.0, Some(Reset::At(1_735_689_720)), None))
        );
        assert_eq!(usage.plan.as_deref(), Some("Plus"));
        assert!(usage.balances.is_empty(), "no credits object, no row");
    }

    #[test]
    fn a_missing_reset_instant_falls_back_to_the_seconds_left() {
        // `reset_at` is the instant codex reads; a body that only says how
        // long is placed against the moment it was fetched.
        let body = r#"{"plan_type": "plus", "rate_limit": {
            "primary_window": {"used_percent": 10, "limit_window_seconds": 18000,
                "reset_after_seconds": 600}}}"#;
        let usage = parse_chatgpt(body, 1_000).unwrap();
        assert_eq!(
            usage.limits[0].allowance,
            Allowance::Metered(Meter::new(10.0, Some(Reset::At(1_600)), None))
        );
    }

    #[test]
    fn window_lengths_are_named_the_way_people_say_them() {
        // Codex's names within 5% of the length (`is_approximate_window`),
        // spelled out rather than abbreviated; any other length is still a
        // name, never a generic "usage limit".
        assert_eq!(window_name(18_000).as_deref(), Some("5-hour"));
        assert_eq!(window_name(17_400).as_deref(), Some("5-hour"), "within 5%");
        assert_eq!(window_name(86_400).as_deref(), Some("daily"));
        assert_eq!(window_name(604_800).as_deref(), Some("weekly"));
        assert_eq!(window_name(2_592_000).as_deref(), Some("monthly"));
        assert_eq!(window_name(31_536_000).as_deref(), Some("annual"));
        assert_eq!(window_name(7_200).as_deref(), Some("2-hour"));
        assert_eq!(window_name(1_800).as_deref(), Some("30-minute"));
        assert_eq!(window_name(259_200).as_deref(), Some("3-day"));
        assert_eq!(window_name(0), None);
        assert_eq!(window_name(-5), None);
    }

    #[test]
    fn a_window_of_no_known_length_keeps_a_generic_label() {
        let body = r#"{"plan_type": "team", "rate_limit": {
            "primary_window": {"used_percent": 1, "limit_window_seconds": 0, "reset_at": 5},
            "secondary_window": {"used_percent": 2, "limit_window_seconds": 0, "reset_at": 6}}}"#;
        let usage = parse_chatgpt(body, 0).unwrap();
        let labels: Vec<_> = usage.limits.iter().map(|l| l.label.as_str()).collect();
        assert_eq!(labels, ["Usage limit", "Secondary limit"]);
    }

    #[test]
    fn credits_read_as_unlimited_a_balance_or_available() {
        let credits = |json: &str| {
            parse_chatgpt(&format!(r#"{{"plan_type": "pro", "credits": {json}}}"#), 0)
                .unwrap()
                .balances
        };
        let row = |value: &str, detail: Option<&str>| Balance {
            label: "Credits".to_string(),
            value: value.to_string(),
            detail: detail.map(str::to_string),
        };
        assert_eq!(
            credits(r#"{"has_credits": true, "unlimited": true}"#),
            vec![row("Unlimited", None)]
        );
        assert_eq!(
            credits(r#"{"has_credits": true, "unlimited": false, "balance": "1234.6"}"#),
            vec![row("1,235", None)],
            "rounded to a whole credit"
        );
        assert_eq!(
            credits(r#"{"has_credits": true, "unlimited": false, "balance": null}"#),
            vec![row("Available", None)]
        );
        assert_eq!(
            credits(r#"{"has_credits": true, "unlimited": false, "balance": "0"}"#),
            vec![row("Available", None)],
            "codex shows no count for an empty balance"
        );
        assert!(credits(r#"{"has_credits": false, "unlimited": false}"#).is_empty());
    }

    #[test]
    fn a_workspace_spend_cap_is_a_monthly_credit_meter() {
        let body = r#"{"plan_type": "business", "spend_control": {"reached": false,
            "individual_limit": {"source": "workspace_spend_controls", "limit": "25000",
                "used": "8000", "remaining": "17000", "used_percent": 32,
                "remaining_percent": 68, "reset_after_seconds": 43200, "reset_at": 1735693200}}}"#;
        let usage = parse_chatgpt(body, 0).unwrap();
        assert_eq!(
            usage.limits,
            vec![Limit::new(
                "Monthly credit limit",
                Allowance::Metered(Meter::new(
                    32.0,
                    Some(Reset::At(1_735_693_200)),
                    Some("8,000 of 25,000 credits used".to_string()),
                )),
            )]
        );
    }

    #[test]
    fn an_additional_bucket_is_scoped_to_its_own_name() {
        // A model with an allowance of its own: its windows are labelled by
        // length like the main bucket's, under its name — codex's heading
        // row for a named bucket — rather than a label the column can't hold.
        let body = r#"{"plan_type": "pro",
            "rate_limit": {"primary_window": {"used_percent": 1, "limit_window_seconds": 18000, "reset_at": 8}},
            "additional_rate_limits": [
            {"limit_name": "GPT-5.3-Codex-Spark", "metered_feature": "codex_bengalfox",
             "rate_limit": {"allowed": true, "limit_reached": false,
               "primary_window": {"used_percent": 88, "limit_window_seconds": 18000, "reset_at": 9},
               "secondary_window": {"used_percent": 3, "limit_window_seconds": 604800, "reset_at": 10}}},
            {"metered_feature": "codex_other", "rate_limit": {
               "primary_window": {"used_percent": 4, "limit_window_seconds": 0, "reset_at": 11}}}]}"#;
        let usage = parse_chatgpt(body, 0).unwrap();
        let rows: Vec<_> = usage
            .limits
            .iter()
            .map(|l| (l.scope.as_deref(), l.label.as_str()))
            .collect();
        assert_eq!(
            rows,
            [
                (None, "5-hour limit"),
                (Some("GPT-5.3-Codex-Spark"), "5-hour limit"),
                (Some("GPT-5.3-Codex-Spark"), "Weekly limit"),
                (Some("codex_other"), "Usage limit"),
            ]
        );
    }

    #[test]
    fn the_plan_wide_rows_come_before_the_named_buckets() {
        // A workspace's credit cap is the whole plan's, so it lists with the
        // main windows — after a named bucket's indented rows it would read
        // as that model's.
        let body = r#"{"plan_type": "business",
            "rate_limit": {"primary_window": {"used_percent": 1, "limit_window_seconds": 18000, "reset_at": 8}},
            "additional_rate_limits": [{"limit_name": "GPT-5.3-Codex-Spark", "rate_limit": {
               "primary_window": {"used_percent": 2, "limit_window_seconds": 18000, "reset_at": 9}}}],
            "spend_control": {"reached": false, "individual_limit": {"limit": "100", "used": "5",
               "used_percent": 5, "reset_at": 10}}}"#;
        let usage = parse_chatgpt(body, 0).unwrap();
        let rows: Vec<_> = usage
            .limits
            .iter()
            .map(|l| (l.scope.as_deref(), l.label.as_str()))
            .collect();
        assert_eq!(
            rows,
            [
                (None, "5-hour limit"),
                (None, "Monthly credit limit"),
                (Some("GPT-5.3-Codex-Spark"), "5-hour limit"),
            ]
        );
    }

    #[test]
    fn a_limit_reached_or_a_spend_cap_hit_is_a_warning() {
        // Codex's own sentences for each kind (`UsageLimitReachedError`),
        // folded to one row: the kind says *why*, so it replaces the
        // generic "a limit is reached".
        let body = r#"{"plan_type": "plus",
            "rate_limit": {"allowed": false, "limit_reached": true,
              "primary_window": {"used_percent": 100, "limit_window_seconds": 18000, "reset_at": 9}},
            "spend_control": {"reached": true, "individual_limit": null},
            "rate_limit_reached_type": {"type": "workspace_member_credits_depleted"}}"#;
        let usage = parse_chatgpt(body, 0).unwrap();
        let notes: Vec<_> = usage
            .notes
            .iter()
            .map(|n| (n.warning, n.text.as_str()))
            .collect();
        assert_eq!(
            notes,
            [
                (
                    true,
                    "Your workspace is out of credits — ask its owner to refill them."
                ),
                (true, "Your spend cap is reached."),
            ]
        );
    }

    #[test]
    fn a_spend_cap_kind_is_said_once() {
        // The kind already names the spend cap; the `spend_control` flag
        // saying so again would be the same sentence twice.
        let body = r#"{"plan_type": "business",
            "rate_limit": {"allowed": false, "limit_reached": true},
            "spend_control": {"reached": true},
            "rate_limit_reached_type": {"type": "workspace_owner_usage_limit_reached"}}"#;
        let usage = parse_chatgpt(body, 0).unwrap();
        assert_eq!(
            usage.notes,
            vec![Note::warning(
                "You hit your workspace's spend cap — raise it to continue."
            )]
        );
        let member = body.replace("owner_usage", "member_usage");
        assert_eq!(
            parse_chatgpt(&member, 0).unwrap().notes,
            vec![Note::warning(
                "You hit the spend cap your workspace owner set — ask them to raise it."
            )]
        );
        let owner_credits = body.replace("owner_usage_limit_reached", "owner_credits_depleted");
        assert_eq!(
            parse_chatgpt(&owner_credits, 0).unwrap().notes[0],
            Note::warning("Your workspace is out of credits — add credits to continue.")
        );
    }

    #[test]
    fn a_plain_limit_reached_names_no_workspace() {
        let body = r#"{"plan_type": "plus",
            "rate_limit": {"allowed": false, "limit_reached": true,
              "primary_window": {"used_percent": 100, "limit_window_seconds": 18000, "reset_at": 9}}}"#;
        let usage = parse_chatgpt(body, 0).unwrap();
        assert_eq!(
            usage.notes,
            vec![Note::warning(
                "A limit is reached — requests wait for it to reset."
            )]
        );
    }

    #[test]
    fn an_answer_that_is_not_the_usage_object_is_a_decode_error() {
        assert!(matches!(
            parse_chatgpt("<html>", 0),
            Err(LlmError::Decode(_))
        ));
        assert!(matches!(
            parse_chatgpt("[1, 2]", 0),
            Err(LlmError::Decode(_))
        ));
    }

    #[test]
    fn an_unknown_plan_slug_still_reads_like_a_name() {
        let usage = parse_chatgpt(r#"{"plan_type": "self_serve_business_prolite"}"#, 0).unwrap();
        assert_eq!(usage.plan.as_deref(), Some("Self Serve Business Prolite"));
        assert!(usage.limits.is_empty() && usage.balances.is_empty());
        let usage = parse_chatgpt(r#"{"plan_type": "guest"}"#, 0).unwrap();
        assert_eq!(usage.plan, None, "codex calls a guest seat Unknown");
    }

    #[test]
    fn the_usage_url_sits_beside_the_codex_base_not_under_it() {
        // `/wham/usage` is a sibling of `/codex` under `/backend-api` —
        // codex's `PathStyle::ChatGptApi` — and a base without
        // `/backend-api` is the Codex API's own `/api/codex/usage`.
        assert_eq!(
            chatgpt_usage_url("https://chatgpt.com/backend-api/codex"),
            "https://chatgpt.com/backend-api/wham/usage"
        );
        assert_eq!(
            chatgpt_usage_url("https://chatgpt.com/backend-api/codex/"),
            "https://chatgpt.com/backend-api/wham/usage"
        );
        assert_eq!(
            chatgpt_usage_url("https://chatgpt.com/backend-api"),
            "https://chatgpt.com/backend-api/wham/usage"
        );
        assert_eq!(
            chatgpt_usage_url("http://127.0.0.1:4242/backend-api/codex"),
            "http://127.0.0.1:4242/backend-api/wham/usage"
        );
        assert_eq!(
            chatgpt_usage_url("https://chatgpt.com"),
            "https://chatgpt.com/backend-api/wham/usage"
        );
        assert_eq!(
            chatgpt_usage_url("https://codex.example.com/v1/codex"),
            "https://codex.example.com/v1/api/codex/usage"
        );
    }

    #[test]
    fn thousands_are_grouped_for_reading() {
        assert_eq!(group_thousands(0), "0");
        assert_eq!(group_thousands(999), "999");
        assert_eq!(group_thousands(1_000), "1,000");
        assert_eq!(group_thousands(62_500), "62,500");
        assert_eq!(group_thousands(1_234_567), "1,234,567");
        assert_eq!(group_thousands(-4_200), "-4,200");
    }

    /// A Pro seat's `GET /copilot_internal/user`, in the shape VS Code's
    /// `IEntitlementsData` types and a Business answer published in
    /// litellm#18242 carries: chat and completions unlimited, premium
    /// requests metered with a fractional remainder (model multipliers).
    const COPILOT_PRO: &str = r#"{
        "access_type_sku": "monthly_subscriber_quota", "copilot_plan": "individual",
        "chat_enabled": true, "can_signup_for_limited": false,
        "analytics_tracking_id": "x", "assigned_date": "2026-01-01T00:00:00Z",
        "organization_login_list": [],
        "quota_reset_date": "2026-11-01", "quota_reset_date_utc": "2026-11-01T00:00:00.000Z",
        "quota_snapshots": {
          "chat": {"entitlement": 0, "overage_count": 0, "overage_permitted": false,
                   "percent_remaining": 100.0, "quota_id": "chat", "quota_remaining": 0.0,
                   "remaining": 0, "unlimited": true, "timestamp_utc": "2026-10-26T02:00:00.000Z"},
          "completions": {"entitlement": 0, "overage_count": 0, "overage_permitted": false,
                   "percent_remaining": 100.0, "quota_id": "completions", "quota_remaining": 0.0,
                   "remaining": 0, "unlimited": true, "timestamp_utc": "2026-10-26T02:00:00.000Z"},
          "premium_interactions": {"entitlement": 300, "overage_count": 0, "overage_permitted": false,
                   "percent_remaining": 31.17, "quota_id": "premium_interactions",
                   "quota_remaining": 93.5, "remaining": 93, "unlimited": false,
                   "timestamp_utc": "2026-10-26T02:00:00.000Z"}}
    }"#;

    #[test]
    fn a_copilot_pro_seat_meters_premium_requests_and_nothing_else() {
        let usage = parse_copilot(COPILOT_PRO).unwrap();
        assert_eq!(usage.plan.as_deref(), Some("Pro"));
        assert_eq!(usage.account, None, "the answer names no login");
        assert_eq!(
            usage.limits,
            vec![
                Limit::new(
                    "Premium requests",
                    Allowance::Metered(Meter::new(
                        100.0 - 31.17,
                        // The account's reset with its time of day: the UTC
                        // spelling wins over the bare date.
                        Some(Reset::At(1_793_491_200)),
                        Some("93.5 of 300 left".to_string()),
                    )),
                ),
                Limit::new("Chat messages", Allowance::Unlimited),
                Limit::new("Inline suggestions", Allowance::Unlimited),
            ]
        );
        assert!(usage.balances.is_empty() && usage.notes.is_empty());
    }

    /// A Free seat: chat and completions metered, premium requests entitled
    /// to nothing — which VS Code skips rather than showing a full bar.
    const COPILOT_FREE: &str = r#"{
        "access_type_sku": "free_limited_copilot", "copilot_plan": "individual",
        "chat_enabled": true, "limited_user_reset_date": "2026-11-12",
        "quota_snapshots": {
          "chat": {"entitlement": "50", "percent_remaining": 84.0, "quota_remaining": 42,
                   "remaining": 42, "unlimited": false, "overage_count": 0},
          "completions": {"entitlement": 2000, "percent_remaining": 96.5,
                   "quota_remaining": 1930, "unlimited": false},
          "premium_interactions": {"entitlement": 0, "percent_remaining": 0,
                   "quota_remaining": 0, "unlimited": false}}
    }"#;

    fn free_limits() -> Vec<Limit> {
        let reset = Some(Reset::On(
            chrono::NaiveDate::from_ymd_opt(2026, 11, 12).unwrap(),
        ));
        vec![
            Limit::new(
                "Chat messages",
                Allowance::Metered(Meter::new(16.0, reset, Some("42 of 50 left".to_string()))),
            ),
            Limit::new(
                "Inline suggestions",
                Allowance::Metered(Meter::new(
                    3.5,
                    reset,
                    Some("1,930 of 2,000 left".to_string()),
                )),
            ),
        ]
    }

    #[test]
    fn a_copilot_free_seat_meters_chat_and_suggestions_and_skips_premium() {
        // `access_type_sku` names the plan first: a Free account reports
        // `copilot_plan: "individual"` too (VS Code's `resolveEntitlements`).
        // The entitlement arrives as a string here, as VS Code's types say.
        let usage = parse_copilot(COPILOT_FREE).unwrap();
        assert_eq!(usage.plan.as_deref(), Some("Free"));
        assert_eq!(usage.limits, free_limits());
    }

    #[test]
    fn the_legacy_free_quotas_meter_the_same_two_allowances() {
        let body = r#"{"access_type_sku": "free_limited_copilot", "copilot_plan": "individual",
            "limited_user_quotas": {"chat": 42, "completions": 1930},
            "monthly_quotas": {"chat": 50, "completions": 2000},
            "limited_user_reset_date": "2026-11-12"}"#;
        assert_eq!(parse_copilot(body).unwrap().limits, free_limits());
    }

    #[test]
    fn a_snapshot_s_own_reset_outranks_the_account_s() {
        // A snapshot's `quota_reset_at` governs that quota; `0` is absent.
        let with = COPILOT_PRO.replace(
            r#""quota_remaining": 93.5,"#,
            r#""quota_remaining": 93.5, "quota_reset_at": 1790000000,"#,
        );
        let Allowance::Metered(meter) = &parse_copilot(&with).unwrap().limits[0].allowance else {
            panic!("premium is metered");
        };
        assert_eq!(meter.resets, Some(Reset::At(1_790_000_000)));
        let zero = COPILOT_PRO.replace(
            r#""quota_remaining": 93.5,"#,
            r#""quota_remaining": 93.5, "quota_reset_at": 0,"#,
        );
        let Allowance::Metered(meter) = &parse_copilot(&zero).unwrap().limits[0].allowance else {
            panic!("premium is metered");
        };
        assert_eq!(meter.resets, Some(Reset::At(1_793_491_200)));
    }

    #[test]
    fn overage_past_the_allowance_is_counted() {
        let body = r#"{"copilot_plan": "individual_pro", "quota_reset_date": "2026-11-01",
            "quota_snapshots": {"premium_interactions": {"entitlement": 1500,
              "percent_remaining": 0, "quota_remaining": 0, "overage_count": 12,
              "overage_permitted": true, "unlimited": false}}}"#;
        let usage = parse_copilot(body).unwrap();
        assert_eq!(usage.plan.as_deref(), Some("Pro+"));
        assert_eq!(
            usage.limits,
            vec![Limit::new(
                "Premium requests",
                Allowance::Metered(Meter::new(
                    100.0,
                    Some(Reset::On(
                        chrono::NaiveDate::from_ymd_opt(2026, 11, 1).unwrap()
                    )),
                    Some("0 of 1,500 left · 12 over".to_string()),
                )),
            )]
        );
    }

    #[test]
    fn token_based_billing_meters_ai_credits() {
        // Since June 2026 premium requests are GitHub AI Credits; VS Code
        // labels the row Credits under `token_based_billing`.
        let body = r#"{"copilot_plan": "business", "token_based_billing": true,
            "quota_snapshots": {"premium_interactions": {"entitlement": 1900,
              "percent_remaining": 50, "quota_remaining": 950, "unlimited": false},
              "chat": {"unlimited": true, "has_quota": false},
              "completions": {"unlimited": true, "credits_used": 1234}}}"#;
        let usage = parse_copilot(body).unwrap();
        assert_eq!(usage.plan.as_deref(), Some("Business"));
        let labels: Vec<_> = usage.limits.iter().map(|l| l.label.as_str()).collect();
        assert_eq!(labels, ["AI Credits", "Chat messages"]);
        // An unlimited snapshot with no quota left is a depleted
        // organization pool — VS Code's "Organization limit reached".
        assert_eq!(
            usage.limits[1].allowance,
            Allowance::Metered(Meter::new(
                100.0,
                None,
                Some("organization limit reached".to_string())
            ))
        );
        // An unlimited allowance that counts what it spent has no
        // denominator — a balance, not a bar.
        assert_eq!(
            usage.balances,
            vec![Balance {
                label: "Inline suggestions".to_string(),
                value: "1,234 credits used".to_string(),
                detail: None,
            }]
        );
    }

    #[test]
    fn copilot_plans_are_named_the_way_github_names_them() {
        let plan = |json: &str| parse_copilot(json).unwrap().plan;
        let named = |copilot_plan: &str| plan(&format!(r#"{{"copilot_plan": "{copilot_plan}"}}"#));
        assert_eq!(named("individual").as_deref(), Some("Pro"));
        assert_eq!(named("individual_pro").as_deref(), Some("Pro+"));
        assert_eq!(named("individual_max").as_deref(), Some("Max"));
        assert_eq!(named("individual_edu").as_deref(), Some("Student"));
        assert_eq!(named("business").as_deref(), Some("Business"));
        assert_eq!(named("enterprise").as_deref(), Some("Enterprise"));
        assert_eq!(named("team_trial").as_deref(), Some("Team Trial"));
        assert_eq!(
            plan(r#"{"access_type_sku": "free_educational_quota", "copilot_plan": "individual"}"#)
                .as_deref(),
            Some("Student")
        );
        assert_eq!(plan("{}"), None);
    }

    #[test]
    fn an_account_without_copilot_says_so() {
        let usage =
            parse_copilot(r#"{"can_signup_for_limited": true, "chat_enabled": false}"#).unwrap();
        assert_eq!(usage.plan, None);
        assert_eq!(
            usage.notes,
            vec![Note::warning(
                "This GitHub account has no Copilot plan yet."
            )]
        );
        let disabled =
            parse_copilot(r#"{"copilot_plan": "business", "chat_enabled": false}"#).unwrap();
        assert_eq!(
            disabled.notes,
            vec![Note::warning(
                "Copilot chat is turned off for this account."
            )]
        );
        assert!(matches!(
            parse_copilot("not json"),
            Err(LlmError::Decode(_))
        ));
    }

    /// ClinePass's `GET /users/me/plan/usage-limits` for a live account at its
    /// weekly cap (October 2026): an unused 5-hour window carries no reset.
    const CLINEPASS_LIMITS: &str = r#"{"data": {"limits": [
        {"type": "five_hour", "percentUsed": 0},
        {"type": "weekly", "percentUsed": 100, "resetsAt": "2026-10-27T14:36:37.926555347Z"},
        {"type": "monthly", "percentUsed": 81, "resetsAt": "2026-11-12T13:26:57.928652807Z"},
        {"type": "experimental_pool", "percentUsed": 3}]}, "success": true}"#;

    /// The same account's `GET /users/me/plan`, trimmed: the plan's name and
    /// its per-window caps in units of 1e-8 USD.
    const CLINEPASS_PLAN: &str = r#"{"data": {"userId": "usr-1", "plan": {
        "name": "Cline Pass (Monthly)[Internal]", "displayName": "Cline Pass (Monthly)",
        "type": "individual", "interval": "Monthly", "pricePerSeatCents": 999,
        "entitlements": {"cline_pass": {"enabled": true, "inferenceCapThreshold": {
            "last5HoursUsageCostUSDPerUser": 1000000000,
            "last7daysUsageCostUSDPerUser": 2500000000,
            "last30daysUsageCostUSDPerUser": 5000000000}}}},
        "currentPeriodEnd": "2026-11-13T13:24:06Z"}, "success": true}"#;

    #[test]
    fn a_clinepass_answer_meters_its_three_windows_against_the_plan_s_caps() {
        let usage = parse_clinepass(CLINEPASS_LIMITS, Some(CLINEPASS_PLAN)).unwrap();
        assert_eq!(usage.plan.as_deref(), Some("Cline Pass (Monthly)"));
        assert_eq!(
            usage.limits,
            vec![
                Limit::new(
                    "5-hour limit",
                    Allowance::Metered(Meter::new(0.0, None, Some("$10 cap".to_string()))),
                ),
                Limit::new(
                    "Weekly limit",
                    Allowance::Metered(Meter::new(
                        100.0,
                        Some(Reset::At(1_793_111_797)),
                        Some("$25 cap".to_string()),
                    )),
                ),
                Limit::new(
                    "Monthly limit",
                    Allowance::Metered(Meter::new(
                        81.0,
                        Some(Reset::At(1_794_490_017)),
                        Some("$50 cap".to_string()),
                    )),
                ),
            ],
            "a window type this build doesn't know is not shown"
        );
        assert!(usage.notes.is_empty());
    }

    #[test]
    fn the_windows_stand_without_the_plan() {
        // The plan request is best-effort: without it only the name and the
        // caps go.
        let usage = parse_clinepass(CLINEPASS_LIMITS, None).unwrap();
        assert_eq!(usage.plan, None);
        assert_eq!(usage.limits.len(), 3);
        assert!(usage.limits.iter().all(|limit| matches!(
            &limit.allowance,
            Allowance::Metered(meter) if meter.detail.is_none()
        )));
        let garbled = parse_clinepass(CLINEPASS_LIMITS, Some("<html>")).unwrap();
        assert_eq!(
            garbled.limits.len(),
            3,
            "a plan page that won't parse costs only itself"
        );
    }

    #[test]
    fn a_plan_without_clinepass_says_so() {
        let plan = CLINEPASS_PLAN.replace(r#""enabled": true"#, r#""enabled": false"#);
        let usage =
            parse_clinepass(r#"{"success": true, "data": {"limits": []}}"#, Some(&plan)).unwrap();
        assert_eq!(
            usage.notes,
            vec![Note::warning("This Cline plan doesn't include ClinePass.")]
        );
        assert_eq!(
            clinepass_unsubscribed().notes,
            vec![Note::warning(
                "This Cline account has no ClinePass subscription — \
                 app.cline.bot/dashboard/subscription subscribes."
            )]
        );
    }

    #[test]
    fn a_refused_clinepass_envelope_is_the_server_s_own_words() {
        let err =
            parse_clinepass(r#"{"success": false, "error": "Unauthorized"}"#, None).unwrap_err();
        assert_eq!(err.to_string(), "Unauthorized");
        assert!(matches!(
            parse_clinepass("[]", None),
            Err(LlmError::Decode(_))
        ));
    }

    #[test]
    fn dollars_drop_the_cents_a_whole_amount_has_none_of() {
        assert_eq!(dollars(10.0), "$10");
        assert_eq!(dollars(12.5), "$12.50");
        assert_eq!(dollars(1_500.0), "$1,500");
        assert_eq!(dollars(0.004), "$0.00");
    }

    #[test]
    fn the_console_sign_in_has_no_plan_limit_to_show() {
        // It bills per token to an API organisation, and the endpoint that
        // reports Claude subscription windows belongs to a sign-in this app
        // does not offer (docs/usage.md) — so the row says so, with no request.
        let usage = console_usage();
        assert!(usage.limits.is_empty() && usage.balances.is_empty());
        assert_eq!(
            usage.notes,
            vec![Note::info(
                "Billed per token to your API organization — there's no plan limit to show."
            )]
        );
    }

    #[test]
    fn every_subscription_scheme_has_a_dashboard_and_no_key_does() {
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

    #[test]
    fn a_refused_usage_request_is_worded_for_the_page() {
        // The status says what to do; the provider's own message, when its
        // body has one, says why. An HTML error page says nothing worth a row.
        assert_eq!(
            refusal("GitHub Copilot", 401, ""),
            "GitHub Copilot refused the sign-in — /login signs in again."
        );
        assert_eq!(
            refusal(
                "GitHub Copilot",
                403,
                r#"{"message": "Resource not accessible by integration"}"#
            ),
            "GitHub Copilot refused the request (HTTP 403): Resource not accessible by integration"
        );
        assert_eq!(
            refusal("ChatGPT Codex", 429, ""),
            "ChatGPT Codex is limiting how often usage is read — try again in a moment."
        );
        assert_eq!(
            refusal("ClinePass", 500, "<html><body>Bad gateway</body></html>"),
            "ClinePass answered HTTP 500."
        );
        assert_eq!(
            refusal(
                "ClinePass",
                502,
                r#"{"error": {"message": "upstream\n  timed out"}}"#
            ),
            "ClinePass answered HTTP 502: upstream timed out"
        );
        assert_eq!(
            refusal("ChatGPT Codex", 400, r#"{"detail": "Unsupported client"}"#),
            "ChatGPT Codex answered HTTP 400: Unsupported client"
        );
        assert_eq!(
            refusal("ClinePass", 403, r#"{"error": "forbidden"}"#),
            "ClinePass refused the request (HTTP 403): forbidden"
        );
    }

    #[test]
    fn a_count_keeps_one_decimal_only_when_it_has_one() {
        assert_eq!(count_text(93.5), "93.5");
        assert_eq!(count_text(93.0), "93");
        assert_eq!(count_text(1_930.0), "1,930");
        assert_eq!(count_text(0.04), "0");
        assert_eq!(count_text(-2.0), "0", "a remainder is never negative");
    }

    #[test]
    fn a_meter_is_clamped_into_the_bar() {
        assert_eq!(Meter::new(104.0, None, None).used_percent, 100.0);
        assert_eq!(Meter::new(-3.0, None, None).used_percent, 0.0);
        assert_eq!(Meter::new(f64::NAN, None, None).used_percent, 0.0);
        assert!(Meter::new(100.0, None, None).is_spent());
        assert!(!Meter::new(99.9, None, None).is_spent());
    }
}
