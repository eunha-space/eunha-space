//! Customer subscriptions, charged through TossPayments automatic payments.
//!
//! The customer registers a card in TossPayments' billing window, which sends
//! them back with an `authKey`. That is exchanged here for a billing key, which
//! charges the card each month without the customer. TossPayments schedules
//! nothing itself: a renewal is a row in `subscriptions` whose `next_attempt_at`
//! has come, and the renewer charges it.
//!
//! A charge is written to `subscription_payments` before TossPayments is asked,
//! and its order id is also the request's idempotency key. TossPayments keeps
//! a key's first answer for 15 days, errors included, so a charge whose answer
//! was lost is never sent again under its key: the order is looked up instead.
//! One TossPayments approved is settled as paid; one it never recorded is
//! given up 45 minutes after it was sent, and a new order takes its place.
//!
//! The billing key and the secret key never leave this process: no API returns
//! them, and no log or operation event formats a request that carries them.
//! *docs/billing.md* describes what a customer sees and what an ended
//! subscription does to their instances.

use std::time::Duration;

use anyhow::Result;
use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use time::{OffsetDateTime, UtcOffset};
use uuid::Uuid;

use crate::{
    ApiError, Shared, begin_deprovisioning, customer, customer_instance_count,
    customer_instance_limit, jobs, locale::Locale, queue_restore,
};

/// How long after each declined renewal to try the card again. One more decline
/// than there are entries ends the subscription: about a week after the period
/// it was paying for ended.
const RETRY_AFTER_DECLINE: [&str; 3] = ["1 day", "2 days", "4 days"];

/// How long to wait before asking about a charge whose outcome is not known.
const RETRY_UNKNOWN: &str = "5 minutes";

/// How long after its last charge request an order TossPayments has no record
/// of is given up. TossPayments takes up to a minute to approve a charge, and
/// a request that timed out may still be approved for a while after.
const ABANDON_UNRECORDED_ORDER: &str = "45 minutes";

/// How soon a subscription held back by an MID mismatch may be looked at again,
/// which the renewer does at the next charge time after it.
const MID_MISMATCH_RECHECK: &str = "1 hour";

/// How long a charge in flight holds its subscription. Longer than the request's
/// own timeout, so a slow approval is not sent again while it is still running.
const LEASE: &str = "3 minutes";

/// The most recent 09:00 in Korea, when renewals are charged. A renewal due later
/// than this waits for the next one. SQL.
const LATEST_CHARGE_TIME: &str = "((date_trunc('day',now() AT TIME ZONE 'Asia/Seoul'-interval '9 hours')+interval '9 hours') AT TIME ZONE 'Asia/Seoul')";

/// The first 09:00 in Korea after a time given in SQL.
fn charge_time_after(at: &str) -> String {
    format!(
        "((date_trunc('day',({at}) AT TIME ZONE 'Asia/Seoul'-interval '9 hours')+interval '33 hours') AT TIME ZONE 'Asia/Seoul')"
    )
}

/// When a renewal at the end of a period given in SQL is charged: the period's
/// end if that is 09:00 in Korea, or else the next 09:00.
pub(crate) fn renewal_time(period_end: &str) -> String {
    charge_time_after(&format!("{period_end}-interval '1 microsecond'"))
}

/// The longest `customerEmail` TossPayments takes.
const TOSS_EMAIL_MAX: usize = 100;

/// How long a request to TossPayments may take. An approval can take 60 seconds
/// by its documentation, which asks for at least that much.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(90);

/// How long connecting may take. Divided among the addresses a name resolves
/// to, so one that drops packets does not use up the request's whole timeout.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// How often the renewer looks for a subscription that is due.
const POLL_INTERVAL: Duration = Duration::from_secs(30);

/// The most charges one pass makes, so a backlog cannot starve the loop's tick.
const MAX_PER_PASS: usize = 50;

/// What TossPayments says about one charge.
#[derive(Debug, PartialEq)]
pub(crate) enum Charge {
    Paid {
        payment_key: String,
        receipt_url: Option<String>,
    },
    /// The card or TossPayments refused it. The message is TossPayments' own,
    /// written for the customer.
    Declined { code: String, message: String },
    /// No answer that says either way. The payment stays pending and is asked
    /// about again under the same idempotency key.
    Unknown,
}

/// What TossPayments recorded for an order.
#[derive(Debug, PartialEq)]
pub(crate) enum Lookup {
    Settled(Charge),
    /// Approved, then partly given back before the approval was recorded. It is
    /// paid, and the cancellation is recorded once it is.
    PartlyCanceled {
        paid: Charge,
        payment: crate::billing_sync::PaymentRecord,
    },
    /// TossPayments has no such order: nothing was charged under it.
    NotFound,
    Unknown,
}

pub(crate) struct Config {
    client_key: String,
    secret_key: String,
    /// `https://api.tosspayments.com` unless `TOSSPAYMENTS_API_BASE` points at
    /// a stand-in, which is how charging is tested.
    pub(crate) api_base: String,
    order_name: String,
    /// The merchant ID (MID) the secret key belongs to. A billing key can only
    /// be charged through the MID that issued it, so a subscription whose key
    /// another MID issued (test keys, before the switch to live) is not
    /// charged at all rather than declined. Required with a live key.
    pub(crate) mid: Option<String>,
    /// Card approval can take far longer than the four seconds the shared
    /// client allows.
    pub(crate) client: reqwest::Client,
}

impl Config {
    pub(crate) fn from_env() -> Result<Option<Self>> {
        let (Ok(client_key), Ok(secret_key)) = (
            std::env::var("TOSSPAYMENTS_CLIENT_KEY"),
            std::env::var("TOSSPAYMENTS_SECRET_KEY"),
        ) else {
            return Ok(None);
        };
        anyhow::ensure!(
            !client_key.trim().is_empty() && !secret_key.trim().is_empty(),
            "TossPayments keys must not be empty"
        );
        let mid = std::env::var("TOSSPAYMENTS_MID")
            .ok()
            .map(|mid| mid.trim().to_owned())
            .filter(|mid| !mid.is_empty());
        anyhow::ensure!(
            mid.is_some() || !secret_key.trim().starts_with("live_"),
            "TOSSPAYMENTS_MID is required with a live TossPayments secret key"
        );
        Ok(Some(Self::new(
            client_key.trim().into(),
            secret_key.trim().into(),
            std::env::var("TOSSPAYMENTS_API_BASE")
                .unwrap_or_else(|_| "https://api.tosspayments.com".into()),
            std::env::var("EUNHA_SPACE_SUBSCRIPTION_NAME")
                .unwrap_or_else(|_| "은하공간 월간 구독".into()),
            mid,
        )?))
    }

    pub(crate) fn new(
        client_key: String,
        secret_key: String,
        api_base: String,
        order_name: String,
        mid: Option<String>,
    ) -> Result<Self> {
        Ok(Self {
            client_key,
            secret_key,
            api_base: api_base.trim_end_matches('/').into(),
            order_name: order_name.chars().take(100).collect(),
            mid,
            client: reqwest::Client::builder()
                .timeout(REQUEST_TIMEOUT)
                .connect_timeout(CONNECT_TIMEOUT)
                // TossPayments moves between addresses and asks that connections
                // not outlive its DNS answers. Charges are too far apart for
                // reusing one to save anything.
                .pool_max_idle_per_host(0)
                .build()?,
        })
    }

    pub(crate) fn authorization(&self) -> String {
        format!("Basic {}", STANDARD.encode(format!("{}:", self.secret_key)))
    }

    /// Exchange the `authKey` the billing window returned for a billing key.
    async fn issue_billing_key(
        &self,
        auth_key: &str,
        customer_key: Uuid,
    ) -> Result<IssuedBillingKey, ApiError> {
        let response = self
            .client
            .post(format!("{}/v1/billing/authorizations/issue", self.api_base))
            .header("Authorization", self.authorization())
            .json(&serde_json::json!({
                "authKey": auth_key,
                "customerKey": customer_key.to_string(),
            }))
            .send()
            .await
            // The error would name the request; say only that it failed.
            .map_err(|_| unreachable_toss())?;
        let status = response.status();
        let body = response.bytes().await.map_err(|_| unreachable_toss())?;
        if status.is_success() {
            let issued: IssuedBillingKey =
                serde_json::from_slice(&body).map_err(|_| unreachable_toss())?;
            if issued.customer_key != customer_key.to_string() || issued.billing_key.is_empty() {
                return Err(unreachable_toss());
            }
            return Ok(issued);
        }
        Err(ApiError(
            StatusCode::BAD_REQUEST,
            serde_json::from_slice::<TossError>(&body)
                .map(|error| customer_message(&error.message))
                .unwrap_or_else(|_| "The card could not be registered.".into()),
        ))
    }

    /// Charge a billing key once for an order. A charge whose answer was lost
    /// is settled by `lookup`, never by sending it again.
    pub(crate) async fn charge(
        &self,
        billing_key: &str,
        customer_key: Uuid,
        order_id: &str,
        amount: i64,
        email: Option<&str>,
    ) -> Charge {
        let response = self
            .client
            .post(format!(
                "{}/v1/billing/{}",
                self.api_base,
                urlencode(billing_key)
            ))
            .header("Authorization", self.authorization())
            .header("Idempotency-Key", order_id)
            .json(&serde_json::json!({
                "customerKey": customer_key.to_string(),
                "amount": amount,
                "orderId": order_id,
                "orderName": self.order_name,
                "customerEmail": toss_email(email),
            }))
            .send()
            .await;
        let Ok(response) = response else {
            return Charge::Unknown;
        };
        let status = response.status();
        let Ok(body) = response.bytes().await else {
            return Charge::Unknown;
        };
        if let Some(code) = merchant_setup_error(status, &body) {
            tracing::error!(
                %code,
                "TossPayments refused a charge over this control plane's keys or merchant setup; nothing is counted against the card until it is fixed"
            );
        }
        match charge_outcome(status, &body, order_id, amount) {
            // The order was settled before, longer ago than the idempotency key
            // is kept: what happened to it is on the payment itself.
            Some(Charge::Declined { code, .. })
                if matches!(
                    code.as_str(),
                    "DUPLICATED_ORDER_ID" | "ALREADY_PROCESSED_PAYMENT"
                ) =>
            {
                self.payment_by_order(order_id, amount).await
            }
            Some(outcome) => outcome,
            None => Charge::Unknown,
        }
    }

    async fn payment_by_order(&self, order_id: &str, amount: i64) -> Charge {
        match self.lookup(order_id, amount).await {
            Lookup::Settled(outcome) => outcome,
            // The renewer's next look settles it and records the cancellation.
            Lookup::PartlyCanceled { .. } | Lookup::NotFound | Lookup::Unknown => Charge::Unknown,
        }
    }

    /// What TossPayments recorded for an order. Only its own `NOT_FOUND_PAYMENT`
    /// says the order was never recorded; any other failure says nothing.
    pub(crate) async fn lookup(&self, order_id: &str, amount: i64) -> Lookup {
        let Ok(response) = self
            .client
            .get(format!(
                "{}/v1/payments/orders/{}",
                self.api_base,
                urlencode(order_id)
            ))
            .header("Authorization", self.authorization())
            .send()
            .await
        else {
            return Lookup::Unknown;
        };
        let status = response.status();
        let Ok(body) = response.bytes().await else {
            return Lookup::Unknown;
        };
        if !status.is_success() {
            return match serde_json::from_slice::<TossError>(&body) {
                Ok(error) if error.code == "NOT_FOUND_PAYMENT" => Lookup::NotFound,
                _ => Lookup::Unknown,
            };
        }
        match serde_json::from_slice::<Payment>(&body) {
            Ok(payment) if payment.status == "DONE" => {
                match charge_outcome(status, &body, order_id, amount) {
                    Some(outcome) => Lookup::Settled(outcome),
                    None => Lookup::Unknown,
                }
            }
            Ok(payment)
                if payment.order_id == order_id
                    && matches!(payment.status.as_str(), "ABORTED" | "EXPIRED") =>
            {
                Lookup::Settled(Charge::Declined {
                    code: payment.status,
                    message: "The payment was not approved.".into(),
                })
            }
            // Cancelled before the approval was recorded: nothing is kept, so
            // the period is not paid for.
            Ok(payment) if payment.order_id == order_id && payment.status == "CANCELED" => {
                Lookup::Settled(Charge::Declined {
                    code: payment.status,
                    message: "The payment was canceled.".into(),
                })
            }
            Ok(payment)
                if payment.order_id == order_id
                    && payment.status == "PARTIAL_CANCELED"
                    && payment.total_amount == amount =>
            {
                match serde_json::from_slice(&body) {
                    Ok(record) => Lookup::PartlyCanceled {
                        paid: Charge::Paid {
                            payment_key: payment.payment_key,
                            receipt_url: payment.receipt.and_then(|receipt| receipt.url),
                        },
                        payment: record,
                    },
                    Err(_) => Lookup::Unknown,
                }
            }
            // IN_PROGRESS, READY, or a record for another order: not settled.
            _ => Lookup::Unknown,
        }
    }
}

fn unreachable_toss() -> ApiError {
    ApiError(
        StatusCode::BAD_GATEWAY,
        "TossPayments could not be reached. Try again shortly.".into(),
    )
}

/// TossPayments' path segments are its own identifiers, but nothing here
/// should be able to change which path is asked for.
pub(crate) fn urlencode(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

/// TossPayments' messages are written for customers, but they are still text
/// from outside: keep them short and on one line.
pub(crate) fn customer_message(message: &str) -> String {
    let message: String = message
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(200)
        .collect();
    let message = message.trim();
    if message.is_empty() {
        "The payment was not approved.".into()
    } else {
        message.into()
    }
}

/// Codes that say TossPayments, not the card, could not finish: the charge may
/// or may not have happened, so it is asked about again rather than retried
/// as new.
const UNSETTLED_CODES: [&str; 8] = [
    "IDEMPOTENT_REQUEST_PROCESSING",
    "PROVIDER_ERROR",
    "FAILED_INTERNAL_SYSTEM_PROCESSING",
    "FAILED_PAYMENT_INTERNAL_SYSTEM_PROCESSING",
    "UNKNOWN_PAYMENT_ERROR",
    "ALREADY_PROCESSING_REQUEST",
    "DUPLICATED_REQUEST",
    "FORBIDDEN_CONSECUTIVE_REQUEST",
];

/// Codes that are about this control plane's keys, requests or merchant setup,
/// never about the card. Counted as declines, one bad key or an unfinished
/// contract would decline every renewal at once and, days later, take every
/// subscriber's instances offline. They are left unsettled instead, and logged
/// for an operator: subscriptions keep running until it is fixed.
/// https://docs.tosspayments.com/reference/error-codes
const MERCHANT_SETUP_CODES: [&str; 12] = [
    "UNAUTHORIZED_KEY",
    "INVALID_API_KEY",
    "INCORRECT_BASIC_AUTH_FORMAT",
    "FORBIDDEN_REQUEST",
    "NOT_SUPPORTED_METHOD",
    "NOT_FOUND_TERMINAL_ID",
    "NOT_FOUND_MERCHANT",
    "NOT_FOUND_MERCHANT_INTEGRATION",
    "API_VERSION_UPDATE_NEEDED",
    "INVALID_UNREGISTERED_SUBMALL",
    "INVALID_REQUEST",
    "NOT_MATCHES_CUSTOMER_KEY",
];

/// Whether a failed charge's answer is about this control plane's setup rather
/// than the card. The code is TossPayments' own and safe to log.
pub(crate) fn merchant_setup_error(status: reqwest::StatusCode, body: &[u8]) -> Option<String> {
    let code = serde_json::from_slice::<TossError>(body)
        .map(|error| error.code)
        .unwrap_or_default();
    if status == reqwest::StatusCode::UNAUTHORIZED || MERCHANT_SETUP_CODES.contains(&code.as_str())
    {
        Some(if code.is_empty() {
            status.as_u16().to_string()
        } else {
            code.chars().take(64).collect()
        })
    } else {
        None
    }
}

/// Read a charge's response. `None` when it does not say what happened.
/// The address to send TossPayments with a charge, which it refuses if it is too
/// long. It only uses it to mail its own receipt, and ours goes out regardless.
fn toss_email(email: Option<&str>) -> Option<&str> {
    email.filter(|email| email.chars().count() <= TOSS_EMAIL_MAX)
}

pub(crate) fn charge_outcome(
    status: reqwest::StatusCode,
    body: &[u8],
    order_id: &str,
    amount: i64,
) -> Option<Charge> {
    if status.is_success() {
        let payment = serde_json::from_slice::<Payment>(body).ok()?;
        // An approval for anything but this order and amount is not one we
        // can count.
        if payment.status != "DONE"
            || payment.order_id != order_id
            || payment.total_amount != amount
        {
            return None;
        }
        return Some(Charge::Paid {
            payment_key: payment.payment_key,
            receipt_url: payment.receipt.and_then(|receipt| receipt.url),
        });
    }
    if status.is_server_error()
        || status == reqwest::StatusCode::TOO_MANY_REQUESTS
        || status == reqwest::StatusCode::CONFLICT
        || merchant_setup_error(status, body).is_some()
    {
        return None;
    }
    let error = serde_json::from_slice::<TossError>(body).ok()?;
    if UNSETTLED_CODES.contains(&error.code.as_str()) {
        return None;
    }
    Some(Charge::Declined {
        message: customer_message(&error.message),
        code: error.code.chars().take(64).collect(),
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct IssuedBillingKey {
    billing_key: String,
    customer_key: String,
    /// The merchant ID that issued the key, and the only one that can charge it.
    m_id: Option<String>,
    card_company: Option<String>,
    card_number: Option<String>,
    card: Option<IssuedCard>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct IssuedCard {
    number: Option<String>,
    /// TossPayments' two-character code for the issuer, which replaced
    /// `cardCompany` in API version 2024-06-01.
    issuer_code: Option<String>,
}

/// The card company an issuer code stands for. Only codes whose company is
/// certain are named; any other shows the masked number alone.
fn card_company_name(issuer_code: &str) -> Option<&'static str> {
    Some(match issuer_code {
        "11" => "KB국민",
        "15" => "카카오뱅크",
        "21" => "하나",
        "24" => "토스뱅크",
        "31" => "BC",
        "36" => "씨티",
        "3A" => "케이뱅크",
        "41" => "신한",
        "51" => "삼성",
        "61" => "현대",
        "71" => "롯데",
        "91" => "NH농협",
        "W1" => "우리",
        _ => return None,
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Payment {
    #[serde(default)]
    payment_key: String,
    #[serde(default)]
    order_id: String,
    status: String,
    #[serde(default)]
    total_amount: i64,
    receipt: Option<Receipt>,
}

#[derive(Deserialize)]
struct Receipt {
    url: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct TossError {
    pub(crate) code: String,
    pub(crate) message: String,
}

/// Whether a customer may run another instance: one whose renewal was declined
/// is still subscribed while it is retried. Without TossPayments
/// configured nobody is billed; administrators and accounts an operator
/// exempted are never billed.
pub(crate) async fn require_subscription(
    s: &Shared,
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: Uuid,
) -> Result<(), ApiError> {
    if !billing_on(s) {
        return Ok(());
    }
    let allowed = sqlx::query_scalar::<_, bool>(
        "SELECT u.is_admin OR u.billing_exempt OR COALESCE(s.status IN ('active','past_due'),false) FROM users u LEFT JOIN subscriptions s ON s.user_id=u.id WHERE u.id=$1",
    )
    .bind(user_id)
    .fetch_one(&mut **tx)
    .await?;
    if allowed {
        Ok(())
    } else {
        Err(ApiError(
            StatusCode::PAYMENT_REQUIRED,
            "Subscribe to run an instance.".into(),
        ))
    }
}

/// What the customer portal needs to know before showing instance actions.
pub(crate) async fn subscription_required(s: &Shared, user_id: Uuid) -> Result<bool, ApiError> {
    if !billing_on(s) {
        return Ok(false);
    }
    Ok(sqlx::query_scalar::<_, bool>(
        "SELECT NOT (u.is_admin OR u.billing_exempt OR COALESCE(s.status IN ('active','past_due'),false)) FROM users u LEFT JOIN subscriptions s ON s.user_id=u.id WHERE u.id=$1",
    )
    .bind(user_id)
    .fetch_one(&s.db)
    .await?)
}

/// Whether customers pay at all: with neither TossPayments nor Polar
/// configured, nobody is billed.
pub(crate) fn billing_on(s: &Shared) -> bool {
    s.billing.is_some() || s.polar.is_some()
}

fn billing_config(s: &Shared) -> Result<&Config, ApiError> {
    s.billing.as_deref().ok_or_else(|| {
        ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "Subscriptions are not available.".into(),
        )
    })
}

#[derive(Serialize, sqlx::FromRow)]
pub(crate) struct SubscriptionView {
    status: String,
    amount: i64,
    /// `toss` or `polar`: which of them bills it.
    provider: String,
    currency: String,
    plan_id: Option<Uuid>,
    /// The plan the next renewal moves to.
    pending_plan_id: Option<Uuid>,
    card_company: Option<String>,
    card_number: Option<String>,
    has_card: bool,
    #[serde(with = "time::serde::rfc3339::option")]
    current_period_end: Option<OffsetDateTime>,
    cancel_at_period_end: bool,
    #[serde(with = "time::serde::rfc3339::option")]
    next_attempt_at: Option<OffsetDateTime>,
    /// When the next renewal is charged, which is at 09:00 in Korea.
    #[serde(with = "time::serde::rfc3339::option")]
    renews_at: Option<OffsetDateTime>,
    last_error: Option<String>,
    /// When the free trial it is in ends, for a Polar subscription in one.
    #[serde(with = "time::serde::rfc3339::option")]
    trial_ends_at: Option<OffsetDateTime>,
    #[serde(skip)]
    period_start: Option<OffsetDateTime>,
}

#[derive(Serialize, sqlx::FromRow)]
pub(crate) struct PlanView {
    id: Uuid,
    name_en: String,
    name_ko: String,
    amount: i64,
    instance_limit: Option<i64>,
    user_limit: Option<i64>,
    media_gb: Option<i64>,
    database_gb: Option<i64>,
    monthly_activities: Option<i64>,
    /// What Polar charges for it, in `polar_currency`, if it is sold there.
    polar_amount: Option<i64>,
    polar_currency: Option<String>,
    /// The free trial its Polar product gives: `day`, `week`, `month` or
    /// `year`, and how many.
    polar_trial_interval: Option<String>,
    polar_trial_interval_count: Option<i32>,
    /// For a subscriber in a trial, when the trial would end on this plan:
    /// Polar recomputes it from the trial's start with this plan's length.
    /// Null if it would end at once, and the first period be charged.
    #[serde(with = "time::serde::rfc3339::option")]
    trial_until: Option<OffsetDateTime>,
    /// Whether it can be chosen. A retired plan is listed only for those on it.
    active: bool,
    /// What moving to this plan charges now, for an upgrade.
    #[sqlx(skip)]
    switch_charge: Option<i64>,
}

#[derive(Serialize, sqlx::FromRow)]
pub(crate) struct PaymentView {
    order_id: String,
    amount: i64,
    provider: String,
    currency: String,
    kind: String,
    status: String,
    #[serde(with = "time::serde::rfc3339")]
    period_start: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    period_end: OffsetDateTime,
    receipt_url: Option<String>,
    failure_message: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    requested_at: OffsetDateTime,
    /// Whether the customer may ask for this payment back now.
    refundable: bool,
    refund_pending: bool,
    refunded_amount: i64,
    /// TossPayments' reason for refusing the last refund asked for.
    refund_error: Option<String>,
}

#[derive(Serialize)]
pub(crate) struct BillingView {
    enabled: bool,
    /// Whether a Korean card can be added, through TossPayments.
    toss: bool,
    /// Whether an international card can pay, through Polar, and in what.
    polar: bool,
    polar_currency: Option<String>,
    /// What can be subscribed to, cheapest first. An existing subscription
    /// keeps the amount it was started at.
    plans: Vec<PlanView>,
    subscription: Option<SubscriptionView>,
    payments: Vec<PaymentView>,
}

pub(crate) async fn view(s: &Shared, user_id: Uuid) -> Result<BillingView, ApiError> {
    let subscription = sqlx::query_as::<_, SubscriptionView>(
        &format!("SELECT status,amount,provider,currency,plan_id,pending_plan_id,card_company,card_number,billing_key IS NOT NULL has_card,current_period_end,cancel_at_period_end,next_attempt_at,{} renews_at,last_error,CASE WHEN trial_ends_at>now() THEN trial_ends_at END trial_ends_at,anchor_at+make_interval(months => GREATEST(periods_paid-1,0)) period_start FROM subscriptions WHERE user_id=$1", renewal_time("current_period_end")),
    )
    .bind(user_id)
    .fetch_optional(&s.db)
    .await?;
    let payments = sqlx::query_as::<_, PaymentView>(
        &format!("SELECT order_id,amount,provider,currency,kind,status,period_start,period_end,receipt_url,failure_message,requested_at,(status='paid' AND refunded_amount=0 AND refund_requested_at IS NULL AND settled_at>now()-interval '{}' AND (payment_key IS NOT NULL OR provider='polar')) refundable,(refund_requested_at IS NOT NULL AND refunded_at IS NULL) refund_pending,refunded_amount,refund_error FROM subscription_payments WHERE user_id=$1 AND status<>'expired' ORDER BY requested_at DESC LIMIT 24", crate::billing_sync::REFUND_WINDOW),
    )
    .bind(user_id)
    .fetch_all(&s.db)
    .await?;
    let mut plans = sqlx::query_as::<_, PlanView>(
        "SELECT id,name_en,name_ko,amount,instance_limit,user_limit,media_gb,database_gb,monthly_activities,polar_amount,polar_currency,polar_trial_interval,polar_trial_interval_count,(SELECT CASE WHEN t.trial_ends_at>now() AND t.trial_started_at+(polar_trial_interval_count||' '||polar_trial_interval)::interval>now() THEN t.trial_started_at+(polar_trial_interval_count||' '||polar_trial_interval)::interval END FROM subscriptions t WHERE t.user_id=$1) trial_until,active FROM plans WHERE active OR id IN (SELECT plan_id FROM subscriptions WHERE user_id=$1 UNION SELECT pending_plan_id FROM subscriptions WHERE user_id=$1) ORDER BY amount,created_at",
    )
    .bind(user_id)
    .fetch_all(&s.db)
    .await?;
    // Polar works out its own proration, and charges it itself.
    if let Some(running) = subscription
        .as_ref()
        .filter(|running| running.status == "active" && running.provider == "toss")
        && let (Some(start), Some(end)) = (running.period_start, running.current_period_end)
    {
        let now = OffsetDateTime::now_utc();
        for plan in plans.iter_mut().filter(|plan| plan.amount > running.amount) {
            plan.switch_charge = Some(upgrade_charge(plan.amount, running.amount, start, end, now));
        }
    }
    Ok(BillingView {
        enabled: billing_on(s),
        toss: s.billing.is_some(),
        polar: s.polar.is_some(),
        polar_currency: s.polar.as_ref().map(|polar| polar.currency.clone()),
        plans,
        subscription,
        payments,
    })
}

pub(crate) async fn get(
    State(s): State<Shared>,
    headers: HeaderMap,
) -> Result<Json<BillingView>, ApiError> {
    let (user_id, _) = customer(&headers, &s.db).await?;
    Ok(Json(view(&s, user_id).await?))
}

#[derive(Serialize, sqlx::FromRow)]
pub(crate) struct PublicPlan {
    name_en: String,
    name_ko: String,
    amount: i64,
    instance_limit: Option<i64>,
    user_limit: Option<i64>,
    media_gb: Option<i64>,
    database_gb: Option<i64>,
    monthly_activities: Option<i64>,
    polar_amount: Option<i64>,
    polar_currency: Option<String>,
    polar_trial_interval: Option<String>,
    polar_trial_interval_count: Option<i32>,
}

/// The plans on offer, cheapest first, for visitors who have no account yet.
/// With billing off nobody pays, so there are no prices to show.
pub(crate) async fn public_plans(
    State(s): State<Shared>,
) -> Result<Json<Vec<PublicPlan>>, ApiError> {
    if !billing_on(&s) {
        return Ok(Json(vec![]));
    }
    let plans = sqlx::query_as::<_, PublicPlan>(
        "SELECT name_en,name_ko,amount,instance_limit,user_limit,media_gb,database_gb,monthly_activities,polar_amount,polar_currency,polar_trial_interval,polar_trial_interval_count FROM plans WHERE active ORDER BY amount,created_at",
    )
    .fetch_all(&s.db)
    .await?;
    Ok(Json(plans))
}

/// Below this TossPayments will not charge a card, and it is not worth doing:
/// an upgrade this close to renewal costs nothing.
const SMALLEST_CHARGE: i64 = 100;

/// An order id is read out over the phone and shown in one line of a narrow
/// payment history, so it is digits only: the payment's date in Korea, then
/// eight random digits, as in `2026-11-05-0421-7730`. TossPayments takes this
/// shape, and the hyphens stop a spreadsheet reading it as a number.
fn order_id_at(now: OffsetDateTime, random: u32) -> String {
    let day = now.to_offset(UtcOffset::from_whole_seconds(9 * 3600).expect("a valid offset"));
    format!(
        "{:04}-{:02}-{:02}-{:04}-{:04}",
        day.year(),
        u8::from(day.month()),
        day.day(),
        random / 10_000 % 10_000,
        random % 10_000,
    )
}

/// An order id no payment has used. TossPayments never takes one twice. Two
/// charges drawing the same number at the same moment are stopped by the
/// primary key before either reaches TossPayments, and the charge is tried
/// again.
async fn new_order_id(tx: &mut sqlx::PgConnection) -> sqlx::Result<String> {
    loop {
        // The folding bias is negligible with 122 random bits to fold.
        let random = (Uuid::new_v4().as_u128() % 100_000_000) as u32;
        let order_id = order_id_at(OffsetDateTime::now_utc(), random);
        let taken: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM subscription_payments WHERE order_id=$1)",
        )
        .bind(&order_id)
        .fetch_one(&mut *tx)
        .await?;
        if !taken {
            return Ok(order_id);
        }
    }
}

/// The difference between two monthly prices for what is left of a period,
/// rounded up to the won, or nothing when that is too little to charge.
pub(crate) fn upgrade_charge(
    new_amount: i64,
    old_amount: i64,
    start: OffsetDateTime,
    end: OffsetDateTime,
    now: OffsetDateTime,
) -> i64 {
    let total = (end - start).whole_seconds().max(1) as i128;
    let left = ((end - now).whole_seconds() as i128).clamp(0, total);
    let difference = (new_amount - old_amount).max(0) as i128;
    let charge = ((difference * left + total - 1) / total) as i64;
    if charge < SMALLEST_CHARGE { 0 } else { charge }
}

#[derive(Serialize)]
pub(crate) struct Checkout {
    client_key: String,
    customer_key: Uuid,
}

#[derive(Deserialize, Default)]
pub(crate) struct CheckoutChoice {
    plan_id: Option<Uuid>,
}

/// What the billing window needs to open: the public client key and the key
/// the card will be filed under. A subscription that is not running remembers
/// the plan chosen, for when the card comes back; changing the card of a
/// running one touches no plan.
pub(crate) async fn checkout(
    State(s): State<Shared>,
    headers: HeaderMap,
    choice: Option<Json<CheckoutChoice>>,
) -> Result<Json<Checkout>, ApiError> {
    let (user_id, _) = customer(&headers, &s.db).await?;
    let billing = billing_config(&s)?;
    polar_not_running(&s, user_id).await?;
    let running = sqlx::query_as::<_, (Uuid, bool)>(
        "SELECT customer_key,status IN ('active','past_due') OR EXISTS(SELECT 1 FROM subscription_payments p WHERE p.user_id=s.user_id AND p.status='pending') FROM subscriptions s WHERE user_id=$1",
    )
    .bind(user_id)
    .fetch_optional(&s.db)
    .await?;
    if let Some((customer_key, true)) = running {
        return Ok(Json(Checkout {
            client_key: billing.client_key.clone(),
            customer_key,
        }));
    }
    let chosen = choice.and_then(|Json(choice)| choice.plan_id);
    let plan = sqlx::query_as::<_, (Uuid, i64)>(
        "SELECT id,amount FROM plans WHERE active AND ($1::uuid IS NULL OR id=$1) ORDER BY amount,created_at LIMIT 1",
    )
    .bind(chosen)
    .fetch_optional(&s.db)
    .await?
    .ok_or_else(|| {
        ApiError(
            StatusCode::CONFLICT,
            "This plan is not available.".into(),
        )
    })?;
    let customer_key = sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO subscriptions(user_id,customer_key,amount,pending_plan_id) VALUES($1,$2,$3,$4) ON CONFLICT (user_id) DO UPDATE SET pending_plan_id=excluded.pending_plan_id RETURNING customer_key",
    )
    .bind(user_id)
    .bind(Uuid::new_v4())
    .bind(plan.1)
    .bind(plan.0)
    .fetch_one(&s.db)
    .await?;
    Ok(Json(Checkout {
        client_key: billing.client_key.clone(),
        customer_key,
    }))
}

#[derive(Deserialize)]
pub(crate) struct Authorization {
    auth_key: String,
    customer_key: Uuid,
}

/// The billing window's success redirect: register the card, and charge the
/// first month when the subscription is not already paid for.
pub(crate) async fn authorize(
    State(s): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<Authorization>,
) -> Result<Json<BillingView>, ApiError> {
    let (user_id, _) = customer(&headers, &s.db).await?;
    let billing = billing_config(&s)?;
    polar_not_running(&s, user_id).await?;
    let auth_key = body.auth_key.trim();
    if auth_key.is_empty() || auth_key.len() > 300 {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "The card could not be registered.".into(),
        ));
    }
    // The customer key in the redirect is the customer's to change, so it
    // has to be the one this account was given.
    let owned = sqlx::query_scalar::<_, bool>(
        "SELECT true FROM subscriptions WHERE user_id=$1 AND customer_key=$2",
    )
    .bind(user_id)
    .bind(body.customer_key)
    .fetch_optional(&s.db)
    .await?
    .is_some();
    if !owned {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "The card could not be registered.".into(),
        ));
    }
    let issued = billing
        .issue_billing_key(auth_key, body.customer_key)
        .await?;
    let card_number = issued
        .card_number
        .or(issued.card.as_ref().and_then(|card| card.number.clone()))
        .map(|number| number.chars().take(32).collect::<String>());
    let card_company = issued
        .card_company
        .or_else(|| {
            issued
                .card
                .as_ref()
                .and_then(|card| card.issuer_code.as_deref())
                .and_then(card_company_name)
                .map(str::to_owned)
        })
        .map(|company| company.chars().take(32).collect::<String>());
    let mut tx = s.db.begin().await?;
    let (status, had_key) = sqlx::query_as::<_, (String, bool)>(
        "SELECT status,billing_key IS NOT NULL FROM subscriptions WHERE user_id=$1 FOR UPDATE",
    )
    .bind(user_id)
    .fetch_one(&mut *tx)
    .await?;
    // The card this replaces is never charged again, so its key is deleted at
    // TossPayments.
    sqlx::query("INSERT INTO retired_billing_keys(user_id,billing_key,toss_mid) SELECT user_id,billing_key,toss_mid FROM subscriptions WHERE user_id=$1 AND billing_key IS NOT NULL AND billing_key<>$2")
        .bind(user_id)
        .bind(&issued.billing_key)
        .execute(&mut *tx)
        .await?;
    // A running subscription whose key TossPayments deleted was set to end
    // with its period; a new card keeps it going.
    if status == "active" && !had_key {
        sqlx::query("UPDATE subscriptions SET cancel_at_period_end=false WHERE user_id=$1")
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
    }
    sqlx::query("UPDATE subscriptions SET billing_key=$2,card_company=$3,card_number=$4,toss_mid=$5,last_error=NULL,updated_at=now() WHERE user_id=$1")
        .bind(user_id)
        .bind(&issued.billing_key)
        .bind(&card_company)
        .bind(&card_number)
        .bind(issued.m_id.as_deref().map(|mid| mid.chars().take(64).collect::<String>()))
        .execute(&mut *tx)
        .await?;
    let pending = sqlx::query_scalar::<_, bool>(
        "SELECT true FROM subscription_payments WHERE user_id=$1 AND status='pending'",
    )
    .bind(user_id)
    .fetch_optional(&mut *tx)
    .await?
    .is_some();
    let detail = match status.as_str() {
        // Nothing is paid for, so this starts a new subscription today, at
        // today's price. A charge already in flight keeps the period it was
        // for.
        "incomplete" | "canceled" | "lapsed" => {
            sqlx::query(if pending {
                "UPDATE subscriptions SET status='incomplete',failed_attempts=0,cancel_at_period_end=false,next_attempt_at=now() WHERE user_id=$1"
            } else {
                "UPDATE subscriptions s SET status='incomplete',plan_id=p.id,amount=p.amount,pending_plan_id=NULL,anchor_at=now(),periods_paid=0,current_period_end=NULL,failed_attempts=0,cancel_at_period_end=false,next_attempt_at=now() FROM plans p WHERE s.user_id=$1 AND p.id=COALESCE(s.pending_plan_id,s.plan_id)"
            })
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
            "subscribe"
        }
        // A new card for a renewal that was declined is tried at once, with
        // the full run of retries ahead of it.
        "past_due" => {
            sqlx::query(
                "UPDATE subscriptions SET failed_attempts=0,next_attempt_at=now() WHERE user_id=$1",
            )
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
            "card_change"
        }
        _ => "card_change",
    };
    record(
        &mut tx,
        user_id,
        "subscription_card",
        "succeeded",
        serde_json::json!({ "change": detail, "card_company": card_company }),
        None,
    )
    .await?;
    tx.commit().await?;
    // Charge now rather than leave the customer waiting on the renewer. If
    // another control plane holds the charge, it settles it instead.
    if let Err(_error) = charge_due(&s, Some(user_id)).await {
        tracing::warn!(%user_id, "could not charge a new subscription");
    }
    Ok(Json(view(&s, user_id).await?))
}

/// A card cannot be added through TossPayments while Polar bills the
/// subscription: there would be two charging it.
async fn polar_not_running(s: &Shared, user_id: Uuid) -> Result<(), ApiError> {
    let running = sqlx::query_scalar::<_, bool>(
        "SELECT true FROM subscriptions WHERE user_id=$1 AND provider='polar' AND status IN ('active','past_due')",
    )
    .bind(user_id)
    .fetch_optional(&s.db)
    .await?
    .is_some();
    if running {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "This subscription is paid by an international card. Manage it in the billing portal."
                .into(),
        ));
    }
    Ok(())
}

/// Stop renewing. The instances keep running until the paid period ends.
pub(crate) async fn cancel(
    State(s): State<Shared>,
    headers: HeaderMap,
) -> Result<Json<BillingView>, ApiError> {
    let (user_id, _) = customer(&headers, &s.db).await?;
    if let Some(subscription_id) = crate::billing_polar::billed_by_polar(&s, user_id).await? {
        crate::billing_polar::cancel(&s, user_id, &subscription_id).await?;
        return Ok(Json(view(&s, user_id).await?));
    }
    let mut tx = s.db.begin().await?;
    // A declined renewal's period is already over, so cancelling it ends the
    // subscription now instead of after the retries.
    let cancelled = sqlx::query_scalar::<_, bool>(
        "UPDATE subscriptions SET cancel_at_period_end=true,next_attempt_at=CASE WHEN status='past_due' THEN now() ELSE next_attempt_at END,updated_at=now() WHERE user_id=$1 AND status IN ('active','past_due') RETURNING true",
    )
    .bind(user_id)
    .fetch_optional(&mut *tx)
    .await?
    .is_some();
    if !cancelled {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "There is no subscription to cancel.".into(),
        ));
    }
    record(
        &mut tx,
        user_id,
        "subscription_cancel",
        "succeeded",
        serde_json::json!({}),
        None,
    )
    .await?;
    tx.commit().await?;
    Ok(Json(view(&s, user_id).await?))
}

/// Keep renewing a subscription that was cancelled but has not ended yet.
pub(crate) async fn resume(
    State(s): State<Shared>,
    headers: HeaderMap,
) -> Result<Json<BillingView>, ApiError> {
    let (user_id, _) = customer(&headers, &s.db).await?;
    if let Some(subscription_id) = crate::billing_polar::billed_by_polar(&s, user_id).await? {
        crate::billing_polar::resume(&s, user_id, &subscription_id).await?;
        return Ok(Json(view(&s, user_id).await?));
    }
    let mut tx = s.db.begin().await?;
    let resumed = sqlx::query_scalar::<_, bool>(
        "UPDATE subscriptions SET cancel_at_period_end=false,updated_at=now() WHERE user_id=$1 AND status='active' AND cancel_at_period_end AND billing_key IS NOT NULL RETURNING true",
    )
    .bind(user_id)
    .fetch_optional(&mut *tx)
    .await?
    .is_some();
    if !resumed {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "This subscription has ended. Subscribe again to continue.".into(),
        ));
    }
    record(
        &mut tx,
        user_id,
        "subscription_resume",
        "succeeded",
        serde_json::json!({}),
        None,
    )
    .await?;
    tx.commit().await?;
    Ok(Json(view(&s, user_id).await?))
}

#[derive(Deserialize)]
pub(crate) struct PlanChoice {
    plan_id: Uuid,
}

/// Move a running subscription to another plan. A dearer one applies now, for
/// the difference over what is left of the period; if that is declined, nothing
/// changes. Any other applies at the next renewal, and choosing the current
/// plan takes that back.
pub(crate) async fn change_plan(
    State(s): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<PlanChoice>,
) -> Result<Json<BillingView>, ApiError> {
    let (user_id, _) = customer(&headers, &s.db).await?;
    if let Some(subscription_id) = crate::billing_polar::billed_by_polar(&s, user_id).await? {
        crate::billing_polar::change_plan(&s, user_id, &subscription_id, body.plan_id).await?;
        return Ok(Json(view(&s, user_id).await?));
    }
    billing_config(&s)?;
    let mut tx = s.db.begin().await?;
    let current = sqlx::query_as::<_, (String, i64, Option<Uuid>, bool, bool, Option<OffsetDateTime>, Option<OffsetDateTime>)>(
        "SELECT status,amount,plan_id,cancel_at_period_end,billing_key IS NOT NULL,anchor_at+make_interval(months => GREATEST(periods_paid-1,0)),current_period_end FROM subscriptions WHERE user_id=$1 FOR UPDATE",
    )
    .bind(user_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((status, amount, plan_id, canceling, has_card, Some(start), Some(end))) = current
    else {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "Only an active subscription can change plans.".into(),
        ));
    };
    if status != "active" {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "Only an active subscription can change plans.".into(),
        ));
    }
    if canceling {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "Keep your subscription before changing plans.".into(),
        ));
    }
    let busy = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM subscription_payments WHERE user_id=$1 AND (status='pending' OR (refund_requested_at IS NOT NULL AND refunded_at IS NULL)))",
    )
    .bind(user_id)
    .fetch_one(&mut *tx)
    .await?;
    if busy {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "A payment is being confirmed. Try again shortly.".into(),
        ));
    }
    if plan_id == Some(body.plan_id) {
        sqlx::query(
            "UPDATE subscriptions SET pending_plan_id=NULL,updated_at=now() WHERE user_id=$1",
        )
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
        record(
            &mut tx,
            user_id,
            "subscription_plan",
            "succeeded",
            serde_json::json!({ "change": "keep", "plan_id": body.plan_id }),
            None,
        )
        .await?;
        tx.commit().await?;
        return Ok(Json(view(&s, user_id).await?));
    }
    let (new_amount, limit) = sqlx::query_as::<_, (i64, Option<i64>)>(
        "SELECT amount,instance_limit FROM plans WHERE id=$1 AND active",
    )
    .bind(body.plan_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| ApiError(StatusCode::CONFLICT, "This plan is not available.".into()))?;
    if new_amount <= amount {
        let count = customer_instance_count(&mut tx, user_id).await?;
        if limit.is_some_and(|limit| count > limit) {
            return Err(ApiError(
                StatusCode::CONFLICT,
                "Delete instances until you are within this plan's limit first.".into(),
            ));
        }
        sqlx::query(
            "UPDATE subscriptions SET pending_plan_id=$2,updated_at=now() WHERE user_id=$1",
        )
        .bind(user_id)
        .bind(body.plan_id)
        .execute(&mut *tx)
        .await?;
        record(
            &mut tx,
            user_id,
            "subscription_plan",
            "succeeded",
            serde_json::json!({ "change": "at_renewal", "plan_id": body.plan_id }),
            None,
        )
        .await?;
        tx.commit().await?;
        return Ok(Json(view(&s, user_id).await?));
    }
    let charge = upgrade_charge(new_amount, amount, start, end, OffsetDateTime::now_utc());
    if charge == 0 {
        sqlx::query("UPDATE subscriptions SET plan_id=$2,amount=$3,pending_plan_id=NULL,updated_at=now() WHERE user_id=$1")
            .bind(user_id)
            .bind(body.plan_id)
            .bind(new_amount)
            .execute(&mut *tx)
            .await?;
        record(
            &mut tx,
            user_id,
            "subscription_plan",
            "succeeded",
            serde_json::json!({ "change": "now", "plan_id": body.plan_id, "charge": 0 }),
            None,
        )
        .await?;
        tx.commit().await?;
        return Ok(Json(view(&s, user_id).await?));
    }
    if !has_card {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "Add a card before upgrading.".into(),
        ));
    }
    let order_id = new_order_id(&mut tx).await?;
    sqlx::query("INSERT INTO subscription_payments(order_id,user_id,amount,period,period_start,period_end,kind,plan_id,from_plan_id,from_amount,toss_mid) SELECT $1,user_id,$2,GREATEST(periods_paid-1,0),now(),current_period_end,'upgrade',$3,plan_id,amount,toss_mid FROM subscriptions WHERE user_id=$4")
        .bind(&order_id)
        .bind(charge)
        .bind(body.plan_id)
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE subscriptions SET next_attempt_at=now(),updated_at=now() WHERE user_id=$1")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    record(&mut tx, user_id, "subscription_plan", "succeeded", serde_json::json!({ "change": "now", "plan_id": body.plan_id, "charge": charge, "order_id": order_id }), None).await?;
    tx.commit().await?;
    if charge_due(&s, Some(user_id)).await.is_err() {
        tracing::warn!(%user_id, "could not charge an upgrade at once");
    }
    let declined = sqlx::query_scalar::<_, Option<String>>(
        "SELECT failure_message FROM subscription_payments WHERE order_id=$1 AND status='failed'",
    )
    .bind(&order_id)
    .fetch_optional(&s.db)
    .await?;
    if let Some(message) = declined {
        return Err(ApiError(
            StatusCode::PAYMENT_REQUIRED,
            message.unwrap_or_else(|| "The card was declined.".into()),
        ));
    }
    Ok(Json(view(&s, user_id).await?))
}

/// An audit record of a billing change. Billing belongs to an account, not an
/// instance, so it names no tenant. `detail` must never carry a key.
pub(crate) async fn record(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: Uuid,
    kind: &str,
    status: &str,
    detail: serde_json::Value,
    error: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO operations(id,kind,status,requested_by,detail,error,started_at,finished_at) VALUES($1,$2,$3,$4,$5,$6,now(),now())")
        .bind(Uuid::new_v4())
        .bind(kind)
        .bind(status)
        .bind(user_id)
        .bind(detail)
        .bind(error)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

pub async fn renewer(state: Shared) {
    loop {
        if billing_on(&state) {
            for _ in 0..MAX_PER_PASS {
                match crate::billing_sync::refund_due(&state, None).await {
                    Ok(true) => continue,
                    Ok(false) => break,
                    Err(_error) => {
                        tracing::warn!("could not carry out the refunds that are due");
                        break;
                    }
                }
            }
        }
        for _ in 0..MAX_PER_PASS {
            match crate::billing_polar::sync_due(&state).await {
                Ok(true) => continue,
                Ok(false) => break,
                Err(_error) => {
                    tracing::warn!("could not read subscriptions from Polar");
                    break;
                }
            }
        }
        if state.billing.is_some() {
            for _ in 0..MAX_PER_PASS {
                match charge_due(&state, None).await {
                    Ok(true) => continue,
                    Ok(false) => break,
                    Err(_error) => {
                        tracing::warn!("could not renew the subscriptions that are due");
                        break;
                    }
                }
            }
            if crate::billing_mail::schedule_renewal_notices(&state)
                .await
                .is_err()
            {
                tracing::warn!("could not schedule renewal reminders");
            }
            for _ in 0..MAX_PER_PASS {
                match crate::billing_mail::send_due(&state).await {
                    Ok(true) => continue,
                    Ok(false) => break,
                    Err(_error) => {
                        tracing::warn!("could not send billing emails");
                        break;
                    }
                }
            }
            if crate::billing_check::check_due(&state).await.is_err() {
                tracing::warn!("could not compare a day with TossPayments' transactions");
            }
            for _ in 0..MAX_PER_PASS {
                match crate::billing_sync::delete_retired_key(&state).await {
                    Ok(true) => continue,
                    Ok(false) => break,
                    Err(_error) => {
                        tracing::warn!("could not delete retired billing keys");
                        break;
                    }
                }
            }
        }
        state.health.tick("billing");
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

#[derive(sqlx::FromRow)]
struct Due {
    user_id: Uuid,
    customer_key: Uuid,
    status: String,
    cancel_at_period_end: bool,
    billing_key: Option<String>,
    toss_mid: Option<String>,
    email: Option<String>,
}

#[derive(sqlx::FromRow)]
struct PendingPayment {
    order_id: String,
    amount: i64,
    kind: String,
    plan_id: Option<Uuid>,
    period: i32,
    period_end: OffsetDateTime,
    /// When a charge for this order last went out; null if none ever did.
    charge_attempted_at: Option<OffsetDateTime>,
    /// Whether that was long enough ago to give up an unrecorded order.
    abandonable: bool,
}

/// Why a billing key cannot be charged through the configured MID, or `None`
/// when it can. Without a configured MID (test keys) nothing is held back.
pub(crate) fn mid_problem(configured: Option<&str>, key_mid: Option<&str>) -> Option<String> {
    let configured = configured?;
    if key_mid == Some(configured) {
        return None;
    }
    Some(format!(
        "key issued by MID {}, current key is MID {configured}",
        key_mid.unwrap_or("(unrecorded)")
    ))
}

/// Settle one subscription that is due, or the given user's if it is.
/// Returns whether there was one.
pub(crate) async fn charge_due(s: &Shared, user_id: Option<Uuid>) -> Result<bool> {
    let Some(billing) = s.billing.as_deref() else {
        return Ok(false);
    };
    let mut tx = s.db.begin().await?;
    let Some(due) = sqlx::query_as::<_, Due>(
        &format!("SELECT s.user_id,s.customer_key,s.status,s.cancel_at_period_end,s.billing_key,s.toss_mid,u.email FROM subscriptions s JOIN users u ON u.id=s.user_id WHERE s.provider='toss' AND s.next_attempt_at<=now() AND (s.lease_until IS NULL OR s.lease_until<now()) AND s.status IN ('incomplete','active','past_due') AND NOT EXISTS(SELECT 1 FROM subscription_payments p WHERE p.user_id=s.user_id AND p.refund_requested_at IS NOT NULL AND p.refunded_at IS NULL) AND ($1::uuid IS NULL OR s.user_id=$1) AND ($1::uuid IS NOT NULL OR s.status='incomplete' OR s.cancel_at_period_end OR s.billing_key IS NULL OR s.next_attempt_at<={LATEST_CHARGE_TIME} OR EXISTS(SELECT 1 FROM subscription_payments p WHERE p.user_id=s.user_id AND p.status='pending')) ORDER BY s.next_attempt_at LIMIT 1 FOR UPDATE OF s SKIP LOCKED"),
    )
    .bind(user_id)
    .fetch_optional(&mut *tx)
    .await?
    else {
        return Ok(false);
    };
    // A charge still in flight is settled before anything ends: TossPayments
    // may have approved it.
    let in_flight = sqlx::query_scalar::<_, bool>(
        "SELECT true FROM subscription_payments WHERE user_id=$1 AND status='pending'",
    )
    .bind(due.user_id)
    .fetch_optional(&mut *tx)
    .await?
    .is_some();
    if due.cancel_at_period_end && due.status != "incomplete" && !in_flight {
        let tenants = end_subscription(&mut tx, due.user_id, "canceled").await?;
        tx.commit().await?;
        wake(s, &tenants).await;
        notify(s, due.user_id, Notice::Canceled).await;
        return Ok(true);
    }
    let Some(billing_key) = due.billing_key.clone() else {
        // No card to charge: a running subscription ends with its period
        // rather than staying active unpaid; an incomplete one waits for a
        // card.
        sqlx::query("UPDATE subscriptions SET next_attempt_at=CASE WHEN status='incomplete' THEN NULL ELSE now() END,cancel_at_period_end=(status<>'incomplete') WHERE user_id=$1")
            .bind(due.user_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        return Ok(true);
    };
    if let Some(problem) = mid_problem(billing.mid.as_deref(), due.toss_mid.as_deref()) {
        // Charging would only be refused, and counting that against the card
        // would end the subscription over a key the customer did nothing to.
        // The subscription stays as it is and is looked at again in an hour.
        sqlx::query(&format!("UPDATE subscriptions SET next_attempt_at=now()+interval '{MID_MISMATCH_RECHECK}' WHERE user_id=$1"))
            .bind(due.user_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        tracing::error!(user_id = %due.user_id, %problem, "subscription not charged: its billing key belongs to another TossPayments MID");
        return Ok(true);
    }
    let pending = match sqlx::query_as::<_, PendingPayment>(
        &format!("SELECT order_id,amount,kind,plan_id,period,period_end,charge_attempted_at,coalesce(charge_attempted_at<now()-interval '{ABANDON_UNRECORDED_ORDER}',false) abandonable FROM subscription_payments WHERE user_id=$1 AND status='pending'"),
    )
    .bind(due.user_id)
    .fetch_optional(&mut *tx)
    .await?
    {
        Some(pending) => pending,
        None => {
            // A move to another plan waited for this renewal.
            sqlx::query("UPDATE subscriptions s SET plan_id=p.id,amount=p.amount,pending_plan_id=NULL FROM plans p WHERE s.user_id=$1 AND p.id=s.pending_plan_id")
                .bind(due.user_id)
                .execute(&mut *tx)
                .await?;
            let order_id = new_order_id(&mut tx).await?;
            sqlx::query_as::<_, PendingPayment>(
                "INSERT INTO subscription_payments(order_id,user_id,amount,period,period_start,period_end,plan_id,toss_mid) SELECT $1,user_id,amount,periods_paid,anchor_at+make_interval(months => periods_paid),anchor_at+make_interval(months => periods_paid+1),plan_id,toss_mid FROM subscriptions WHERE user_id=$2 RETURNING order_id,amount,kind,plan_id,period,period_end,charge_attempted_at,false abandonable",
            )
            .bind(order_id)
            .bind(due.user_id)
            .fetch_one(&mut *tx)
            .await?
        }
    };
    sqlx::query(&format!(
        "UPDATE subscriptions SET lease_until=now()+interval '{LEASE}' WHERE user_id=$1"
    ))
    .bind(due.user_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    // A charge that already went out is not sent again under its key, which
    // would only replay its first answer: the order is looked up instead.
    let mut pending = pending;
    if pending.charge_attempted_at.is_some() {
        match billing.lookup(&pending.order_id, pending.amount).await {
            Lookup::Settled(outcome) => {
                settle(s, &due, &pending, outcome).await?;
                return Ok(true);
            }
            Lookup::PartlyCanceled { paid, payment } => {
                settle(s, &due, &pending, paid).await?;
                crate::billing_sync::apply_cancellation(s, &pending.order_id, &payment).await?;
                return Ok(true);
            }
            // Never recorded, and long enough ago that it will not be: nothing
            // was charged, so a new order takes its place now. Not a decline.
            Lookup::NotFound if pending.abandonable => {
                pending = replace_unrecorded_order(s, due.user_id, &pending.order_id).await?;
            }
            Lookup::NotFound | Lookup::Unknown => {
                settle(s, &due, &pending, Charge::Unknown).await?;
                return Ok(true);
            }
        }
    }
    sqlx::query("UPDATE subscription_payments SET charge_attempted_at=now() WHERE order_id=$1")
        .bind(&pending.order_id)
        .execute(&s.db)
        .await?;
    let outcome = billing
        .charge(
            &billing_key,
            due.customer_key,
            &pending.order_id,
            pending.amount,
            due.email.as_deref(),
        )
        .await;
    settle(s, &due, &pending, outcome).await?;
    Ok(true)
}

/// Expire an order TossPayments never recorded and request a new one for the
/// same period, under the lease the caller holds.
async fn replace_unrecorded_order(
    s: &Shared,
    user_id: Uuid,
    order_id: &str,
) -> Result<PendingPayment> {
    let mut tx = s.db.begin().await?;
    sqlx::query("UPDATE subscription_payments SET status='expired',settled_at=now() WHERE order_id=$1 AND status='pending'")
        .bind(order_id)
        .execute(&mut *tx)
        .await?;
    let next_order_id = new_order_id(&mut tx).await?;
    let pending = sqlx::query_as::<_, PendingPayment>(
        "INSERT INTO subscription_payments(order_id,user_id,amount,period,period_start,period_end,kind,plan_id,from_plan_id,from_amount,toss_mid) SELECT $1,user_id,amount,period,period_start,period_end,kind,plan_id,from_plan_id,from_amount,toss_mid FROM subscription_payments WHERE order_id=$2 RETURNING order_id,amount,kind,plan_id,period,period_end,charge_attempted_at,false abandonable",
    )
    .bind(next_order_id)
    .bind(order_id)
    .fetch_one(&mut *tx)
    .await?;
    record(
        &mut tx,
        user_id,
        "subscription_charge",
        "failed",
        serde_json::json!({ "order_id": order_id, "replaced_by": pending.order_id }),
        Some("TossPayments has no record of this charge; a new order replaces it."),
    )
    .await?;
    tx.commit().await?;
    Ok(pending)
}

async fn settle(s: &Shared, due: &Due, pending: &PendingPayment, outcome: Charge) -> Result<()> {
    let mut tx = s.db.begin().await?;
    match outcome {
        Charge::Paid {
            payment_key,
            receipt_url,
        } => {
            sqlx::query("UPDATE subscription_payments SET status='paid',payment_key=$2,receipt_url=$3,settled_at=now() WHERE order_id=$1")
                .bind(&pending.order_id)
                .bind(payment_key)
                .bind(receipt_url)
                .execute(&mut *tx)
                .await?;
            if pending.kind == "upgrade" {
                // The new plan starts now. The period and its renewal date are
                // the ones the customer already had.
                sqlx::query("UPDATE subscriptions s SET plan_id=p.id,amount=p.amount,pending_plan_id=NULL,next_attempt_at=s.current_period_end,lease_until=NULL,updated_at=now() FROM plans p WHERE s.user_id=$1 AND p.id=$2")
                    .bind(due.user_id)
                    .bind(pending.plan_id)
                    .execute(&mut *tx)
                    .await?;
                crate::billing_mail::receipt(&mut tx, &pending.order_id).await?;
                record(
                    &mut tx,
                    due.user_id,
                    "subscription_charge",
                    "succeeded",
                    serde_json::json!({ "order_id": pending.order_id, "amount": pending.amount, "kind": "upgrade", "plan_id": pending.plan_id }),
                    None,
                )
                .await?;
                tx.commit().await?;
                return Ok(());
            }
            // A subscription cancelled while this charge was in flight is not
            // renewed again, but the period it paid for is still served.
            sqlx::query("UPDATE subscriptions SET status='active',periods_paid=$2+1,current_period_end=$3,next_attempt_at=$3,failed_attempts=0,last_error=NULL,lease_until=NULL,updated_at=now() WHERE user_id=$1")
                .bind(due.user_id)
                .bind(pending.period)
                .bind(pending.period_end)
                .execute(&mut *tx)
                .await?;
            crate::billing_mail::receipt(&mut tx, &pending.order_id).await?;
            record(
                &mut tx,
                due.user_id,
                "subscription_charge",
                "succeeded",
                serde_json::json!({ "order_id": pending.order_id, "amount": pending.amount, "period": pending.period }),
                None,
            )
            .await?;
            restore_suspended(s, &mut tx, due.user_id).await?;
            tx.commit().await?;
        }
        Charge::Declined { code, message } => {
            sqlx::query("UPDATE subscription_payments SET status='failed',failure_code=$2,failure_message=$3,settled_at=now() WHERE order_id=$1")
                .bind(&pending.order_id)
                .bind(&code)
                .bind(&message)
                .execute(&mut *tx)
                .await?;
            record(
                &mut tx,
                due.user_id,
                "subscription_charge",
                "failed",
                serde_json::json!({ "order_id": pending.order_id, "amount": pending.amount, "period": pending.period, "code": code }),
                Some(&message),
            )
            .await?;
            if pending.kind == "upgrade" {
                // The plan they had is still paid for, so a declined upgrade
                // changes nothing and is not retried or counted.
                sqlx::query("UPDATE subscriptions SET next_attempt_at=current_period_end,lease_until=NULL,updated_at=now() WHERE user_id=$1")
                    .bind(due.user_id)
                    .execute(&mut *tx)
                    .await?;
                tx.commit().await?;
                return Ok(());
            }
            if due.status == "incomplete" {
                // Nothing was ever paid for, so there is nothing to retry: the
                // customer is in the billing window and can try another card.
                sqlx::query("UPDATE subscriptions SET next_attempt_at=NULL,last_error=$2,lease_until=NULL,updated_at=now() WHERE user_id=$1")
                    .bind(due.user_id)
                    .bind(&message)
                    .execute(&mut *tx)
                    .await?;
                tx.commit().await?;
                return Ok(());
            }
            let failed_attempts = sqlx::query_scalar::<_, i32>(
                "UPDATE subscriptions SET failed_attempts=failed_attempts+1,last_error=$2,lease_until=NULL,updated_at=now() WHERE user_id=$1 RETURNING failed_attempts",
            )
            .bind(due.user_id)
            .bind(&message)
            .fetch_one(&mut *tx)
            .await?;
            match RETRY_AFTER_DECLINE.get(failed_attempts as usize - 1) {
                Some(wait) => {
                    // Half a day short, so a decline just after 09:00 is tried
                    // again at 09:00 that many days on, not the day after.
                    let next =
                        charge_time_after(&format!("now()+interval '{wait}'-interval '12 hours'"));
                    sqlx::query(&format!("UPDATE subscriptions SET status='past_due',next_attempt_at={next} WHERE user_id=$1"))
                        .bind(due.user_id)
                        .execute(&mut *tx)
                        .await?;
                    tx.commit().await?;
                    notify(s, due.user_id, Notice::Declined(&message)).await;
                }
                None => {
                    let tenants = end_subscription(&mut tx, due.user_id, "lapsed").await?;
                    tx.commit().await?;
                    wake(s, &tenants).await;
                    notify(s, due.user_id, Notice::Lapsed).await;
                }
            }
        }
        Charge::Unknown => {
            sqlx::query(&format!("UPDATE subscriptions SET lease_until=NULL,next_attempt_at=now()+interval '{RETRY_UNKNOWN}' WHERE user_id=$1"))
                .bind(due.user_id)
                .execute(&mut *tx)
                .await?;
            tx.commit().await?;
        }
    }
    Ok(())
}

/// End a subscription and take its instances offline. Their data is kept and
/// no purge is scheduled: nobody confirmed a deletion. Returns the tenants
/// whose provisioning has to be woken to notice.
pub(crate) async fn end_subscription(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: Uuid,
    status: &str,
) -> Result<Vec<Uuid>, sqlx::Error> {
    sqlx::query("INSERT INTO retired_billing_keys(user_id,billing_key,toss_mid) SELECT user_id,billing_key,toss_mid FROM subscriptions WHERE user_id=$1 AND billing_key IS NOT NULL")
        .bind(user_id)
        .execute(&mut **tx)
        .await?;
    sqlx::query("UPDATE subscriptions SET status=$2,billing_key=NULL,toss_mid=NULL,next_attempt_at=NULL,cancel_at_period_end=false,lease_until=NULL,pending_plan_id=NULL,trial_started_at=NULL,trial_ends_at=NULL,updated_at=now() WHERE user_id=$1")
        .bind(user_id)
        .bind(status)
        .execute(&mut **tx)
        .await?;
    // An account that needs no subscription keeps its instances whatever
    // becomes of one it had.
    let tenants = sqlx::query_scalar::<_, Uuid>(
        "SELECT t.id FROM tenants t JOIN users u ON u.id=t.user_id WHERE t.user_id=$1 AND t.lifecycle_state IN ('provisioning','ready','failed') AND NOT (u.is_admin OR u.billing_exempt) ORDER BY t.created_at FOR UPDATE OF t",
    )
    .bind(user_id)
    .fetch_all(&mut **tx)
    .await?;
    for &tenant_id in &tenants {
        begin_deprovisioning(tx, tenant_id, None, None).await?;
        sqlx::query(
            "UPDATE tenants SET purge_scheduled_for=NULL,billing_suspended_at=now() WHERE id=$1",
        )
        .bind(tenant_id)
        .execute(&mut **tx)
        .await?;
        sqlx::query("INSERT INTO operation_events(operation_id,message) SELECT id,'Taken offline because the subscription ended; no purge is scheduled.' FROM operations WHERE tenant_id=$1 AND kind='tenant_deprovision' ORDER BY created_at DESC LIMIT 1")
            .bind(tenant_id)
            .execute(&mut **tx)
            .await?;
    }
    record(
        tx,
        user_id,
        "subscription_end",
        "succeeded",
        serde_json::json!({ "status": status, "suspended_tenants": tenants.len() }),
        None,
    )
    .await?;
    Ok(tenants)
}

/// Serve again the instances an ended subscription took offline, as far as the
/// customer's instance limit allows. One that cannot come back, such as one
/// whose custom domain another instance took meanwhile, stays retained for the
/// customer to see.
pub(crate) async fn restore_suspended(
    s: &Shared,
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: Uuid,
) -> Result<()> {
    let suspended = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM tenants WHERE user_id=$1 AND lifecycle_state='retained' AND billing_suspended_at IS NOT NULL ORDER BY created_at FOR UPDATE",
    )
    .bind(user_id)
    .fetch_all(&mut **tx)
    .await?;
    for tenant_id in suspended {
        let limit = customer_instance_limit(tx, user_id).await?;
        let count = customer_instance_count(tx, user_id).await?;
        if limit.is_some_and(|limit| count >= limit) {
            break;
        }
        let mut savepoint = sqlx::Acquire::begin(&mut **tx).await?;
        match queue_restore(s, &mut savepoint, tenant_id, None, None).await {
            Ok(()) => savepoint.commit().await?,
            Err(_) => savepoint.rollback().await?,
        }
    }
    Ok(())
}

pub(crate) async fn wake(s: &Shared, tenants: &[Uuid]) {
    for &tenant_id in tenants {
        jobs::wake_provisioning(s, tenant_id).await;
    }
}

/// Something about a subscription the customer is told at once.
pub(crate) enum Notice<'a> {
    Canceled,
    Lapsed,
    /// The card company's or TossPayments' reason.
    Declined(&'a str),
    CardRemoved,
}

impl Notice<'_> {
    fn text(&self, language: Locale) -> (String, String) {
        let (subject, body) = match (self, language) {
            (Notice::Canceled, Locale::En) => (
                "Your eunha space subscription has ended".into(),
                "Your subscription was cancelled and its paid period is over, so your instances are offline. Their data is kept: subscribe again from your account page to serve them again.".into(),
            ),
            (Notice::Canceled, Locale::Ko) => (
                "은하공간 구독이 종료되었습니다".into(),
                "구독이 해지되었고 결제한 기간이 끝나 인스턴스가 오프라인으로 전환되었습니다. 데이터는 보관되며, 계정 페이지에서 다시 구독하면 다시 서비스됩니다.".into(),
            ),
            (Notice::Lapsed, Locale::En) => (
                "Your eunha space subscription has ended".into(),
                "We could not renew your subscription after several attempts, so your instances are offline. Their data is kept: subscribe again from your account page to serve them again.".into(),
            ),
            (Notice::Lapsed, Locale::Ko) => (
                "은하공간 구독이 종료되었습니다".into(),
                "여러 번 시도했지만 구독을 갱신하지 못해 인스턴스가 오프라인으로 전환되었습니다. 데이터는 보관되며, 계정 페이지에서 다시 구독하면 다시 서비스됩니다.".into(),
            ),
            (Notice::Declined(reason), Locale::En) => (
                "Your eunha space payment was declined".into(),
                format!("We could not renew your subscription: {reason}\n\nWe will try again, and your instances keep running meanwhile. To use another card, open your account page."),
            ),
            (Notice::Declined(reason), Locale::Ko) => (
                "은하공간 결제가 거절되었습니다".into(),
                format!("구독을 갱신하지 못했습니다: {reason}\n\n다시 결제를 시도하며, 그동안 인스턴스는 계속 운영됩니다. 다른 카드를 쓰려면 계정 페이지에서 변경하세요."),
            ),
            (Notice::CardRemoved, Locale::En) => (
                "Your eunha space card was removed".into(),
                "The card for your subscription was removed at TossPayments, so it will not renew. Your instances keep running until the paid period ends. To keep them running after that, register a card on your account page.".into(),
            ),
            (Notice::CardRemoved, Locale::Ko) => (
                "은하공간 결제 카드가 삭제되었습니다".into(),
                "구독에 등록된 카드가 토스페이먼츠에서 삭제되어 구독이 갱신되지 않습니다. 결제한 기간이 끝날 때까지 인스턴스는 계속 운영됩니다. 그 뒤에도 계속 이용하려면 계정 페이지에서 카드를 등록하세요.".into(),
            ),
        };
        (subject, body)
    }
}

/// Tell the customer about their subscription in their language, if mail is
/// configured. Best effort: the account page says the same.
pub(crate) async fn notify(s: &Shared, user_id: Uuid, notice: Notice<'_>) {
    let Some(mail) = s.customer_email.as_ref() else {
        return;
    };
    let Ok(Some((email, code))) = sqlx::query_as::<_, (Option<String>, Option<String>)>(
        "SELECT email,locale FROM users WHERE id=$1",
    )
    .bind(user_id)
    .fetch_optional(&s.db)
    .await
    else {
        return;
    };
    let Some(Ok(to)) = email.map(|email| email.parse()) else {
        return;
    };
    let language = code
        .as_deref()
        .and_then(Locale::parse)
        .unwrap_or(Locale::En);
    let (subject, body) = notice.text(language);
    use lettre::AsyncTransport;
    let Ok(message) = lettre::Message::builder()
        .from(crate::locale::sender(mail, language))
        .to(to)
        .subject(subject)
        .body(format!(
            "{body}\n\n{}/account",
            mail.public_url.trim_end_matches('/')
        ))
    else {
        return;
    };
    if mail.transport.send(message).await.is_err() {
        tracing::warn!("could not send a subscription email");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_address_tosspayments_would_refuse_is_left_out() {
        let longest = format!("{}@example.com", "a".repeat(88));
        assert_eq!(longest.len(), TOSS_EMAIL_MAX);
        assert_eq!(toss_email(Some(&longest)), Some(longest.as_str()));
        assert_eq!(toss_email(Some(&format!("a{longest}"))), None);
        assert_eq!(toss_email(None), None);
    }

    #[test]
    fn order_ids_read_as_the_day_in_korea_and_eight_digits() {
        let at = |day, hour| {
            time::Date::from_calendar_date(2026, time::Month::November, day)
                .unwrap()
                .with_hms(hour, 30, 0)
                .unwrap()
                .assume_utc()
        };
        // Past midnight in Korea, still the day before in UTC.
        assert_eq!(order_id_at(at(4, 15), 4_217_730), "2026-11-05-0421-7730");
        assert_eq!(order_id_at(at(4, 14), 99_999_999), "2026-11-04-9999-9999");
        assert_eq!(order_id_at(at(4, 14), 0), "2026-11-04-0000-0000");
    }

    #[test]
    fn an_approval_counts_only_for_its_own_order_and_amount() {
        let ok = reqwest::StatusCode::OK;
        let body = br#"{"paymentKey":"pk","orderId":"eunha-1","status":"DONE","totalAmount":9900,"receipt":{"url":"https://receipt.example/1"}}"#;
        assert_eq!(
            charge_outcome(ok, body, "eunha-1", 9900),
            Some(Charge::Paid {
                payment_key: "pk".into(),
                receipt_url: Some("https://receipt.example/1".into())
            })
        );
        assert_eq!(charge_outcome(ok, body, "eunha-2", 9900), None);
        assert_eq!(charge_outcome(ok, body, "eunha-1", 100), None);
        let waiting =
            br#"{"paymentKey":"pk","orderId":"eunha-1","status":"IN_PROGRESS","totalAmount":9900}"#;
        assert_eq!(charge_outcome(ok, waiting, "eunha-1", 9900), None);
    }

    #[test]
    fn a_decline_keeps_its_reason_and_an_outage_is_asked_about_again() {
        let declined = charge_outcome(
            reqwest::StatusCode::FORBIDDEN,
            "{\"code\":\"REJECT_CARD_COMPANY\",\"message\":\"잔액이 부족합니다.\\n\"}".as_bytes(),
            "eunha-1",
            9900,
        );
        assert_eq!(
            declined,
            Some(Charge::Declined {
                code: "REJECT_CARD_COMPANY".into(),
                message: "잔액이 부족합니다.".into()
            })
        );
        assert_eq!(
            charge_outcome(
                reqwest::StatusCode::CONFLICT,
                br#"{"code":"IDEMPOTENT_REQUEST_PROCESSING","message":"x"}"#,
                "eunha-1",
                9900
            ),
            None
        );
        assert_eq!(
            charge_outcome(
                reqwest::StatusCode::INTERNAL_SERVER_ERROR,
                br#"{"code":"REJECT_CARD_COMPANY","message":"x"}"#,
                "eunha-1",
                9900
            ),
            None
        );
        assert_eq!(
            charge_outcome(reqwest::StatusCode::BAD_REQUEST, b"<html>", "eunha-1", 9900),
            None
        );
    }

    #[test]
    fn the_secret_key_is_sent_as_basic_credentials() {
        let config = Config::new(
            "ck".into(),
            "test_sk_secret".into(),
            "https://toss.example/".into(),
            "plan".into(),
            None,
        )
        .unwrap();
        assert_eq!(config.authorization(), "Basic dGVzdF9za19zZWNyZXQ6");
        assert_eq!(config.api_base, "https://toss.example");
    }

    #[test]
    fn merchant_setup_errors_are_never_declines() {
        for (status, code) in [
            (reqwest::StatusCode::UNAUTHORIZED, "UNAUTHORIZED_KEY"),
            (reqwest::StatusCode::BAD_REQUEST, "INVALID_API_KEY"),
            (reqwest::StatusCode::NOT_FOUND, "NOT_FOUND_MERCHANT"),
            (reqwest::StatusCode::BAD_REQUEST, "NOT_FOUND_TERMINAL_ID"),
            (reqwest::StatusCode::FORBIDDEN, "FORBIDDEN_REQUEST"),
            (reqwest::StatusCode::BAD_REQUEST, "NOT_MATCHES_CUSTOMER_KEY"),
            (reqwest::StatusCode::CONFLICT, "ALREADY_PROCESSING_REQUEST"),
        ] {
            let body = format!("{{\"code\":\"{code}\",\"message\":\"x\"}}");
            assert_eq!(
                charge_outcome(status, body.as_bytes(), "eunha-1", 9900),
                None,
                "{code}"
            );
        }
        // A 401 is about the key whatever its body says.
        assert_eq!(
            charge_outcome(reqwest::StatusCode::UNAUTHORIZED, b"", "eunha-1", 9900),
            None
        );
        assert_eq!(
            merchant_setup_error(
                reqwest::StatusCode::NOT_FOUND,
                br#"{"code":"NOT_FOUND_MERCHANT","message":"x"}"#
            ),
            Some("NOT_FOUND_MERCHANT".into())
        );
        assert_eq!(
            merchant_setup_error(
                reqwest::StatusCode::FORBIDDEN,
                br#"{"code":"REJECT_CARD_COMPANY","message":"x"}"#
            ),
            None
        );
    }

    #[test]
    fn a_key_is_charged_only_through_the_mid_that_issued_it() {
        assert_eq!(mid_problem(None, None), None);
        assert_eq!(mid_problem(None, Some("tosstest")), None);
        assert_eq!(mid_problem(Some("live1"), Some("live1")), None);
        assert!(mid_problem(Some("live1"), Some("tosstest")).is_some());
        assert!(mid_problem(Some("live1"), None).is_some());
    }

    #[test]
    fn path_segments_cannot_escape_their_place() {
        assert_eq!(urlencode("a/../b?c"), "a%2F..%2Fb%3Fc");
    }
}
