//! Keeping the subscription ledger in step with TossPayments: refunds, what
//! TossPayments tells us through webhooks, and deleting billing keys nothing
//! uses any more.
//!
//! A refund is recorded before TossPayments is asked and carried out by the
//! renewer, so a control plane that dies mid-request finishes it later. Every
//! pass looks the payment up first: a cancel TossPayments already made is
//! settled without asking again, and one it refused on the merits withdraws
//! the request, so it does not hold the subscription's renewals back forever.
//!
//! Webhooks are notifications, never facts: a payment event is acted on only
//! after the payment is looked up. *docs/billing.md* has the rules.

use axum::{
    Json,
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
};
use serde::Deserialize;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    ApiError, Shared,
    billing::{
        self, BillingView, TossError, customer_message, merchant_setup_error, mid_problem, notify,
        record, urlencode, view,
    },
    customer,
};

/// How long after it was paid a customer may ask for a payment back.
pub(crate) const REFUND_WINDOW: &str = "7 days";

/// Which Polar payments, `p`, a customer may ask back themselves: only an
/// account's first Polar payment, and only if the account has never had a
/// refund of its own. Polar keeps its fee on every refund, and a free trial
/// is where an international card tries the service; this is for one who
/// forgot to cancel. Anything else is an operator's call. TossPayments
/// payments are not limited this way.
/// Which upgrades, `p`, a customer may ask back themselves: the latest one of
/// the period being served, which can be taken back by returning the plan it
/// moved from. An earlier one is refunded after the later ones, and one from a
/// period already over is an operator's call.
pub(crate) const UPGRADE_SELF_REFUND: &str = "(p.kind<>'upgrade' OR (p.from_plan_id IS NOT NULL AND p.from_amount IS NOT NULL AND EXISTS(SELECT 1 FROM subscriptions s WHERE s.user_id=p.user_id AND s.status IN ('active','past_due') AND s.plan_id=p.plan_id AND s.current_period_end=p.period_end)))";

pub(crate) const POLAR_SELF_REFUND: &str = "(p.provider<>'polar' OR (p.kind='renewal' AND NOT EXISTS(SELECT 1 FROM subscription_payments e WHERE e.user_id=p.user_id AND e.provider='polar' AND e.kind='renewal' AND e.status IN ('paid','refunded') AND (e.settled_at,e.order_id)<(p.settled_at,p.order_id)) AND NOT EXISTS(SELECT 1 FROM subscription_payments r WHERE r.user_id=p.user_id AND r.order_id<>p.order_id AND r.refund_requested_by=r.user_id)))";

/// TossPayments' events are a few hundred bytes.
const WEBHOOK_BODY_LIMIT: usize = 64 * 1024;

/// The signature headers TossPayments documents.
const WEBHOOK_SIGNATURE_HEADERS: [&str; 2] = ["tosspayments-webhook-signature", "toss-signature"];

/// How long to wait before asking about a refund whose outcome is not known.
const REFUND_RETRY: &str = "5 minutes";

/// How long a webhook may spend looking a payment up. TossPayments waits ten
/// seconds for an answer and sends the event again if it gets none.
const WEBHOOK_LOOKUP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(7);

/// A payment as TossPayments records it, as far as refunds need it.
#[derive(Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PaymentRecord {
    #[serde(default)]
    order_id: String,
    status: String,
    #[serde(default)]
    total_amount: i64,
    /// What is left after cancellations.
    balance_amount: Option<i64>,
}

impl PaymentRecord {
    /// A Polar order, in the terms TossPayments' record is read in: what was
    /// paid and what has been given back of it.
    pub(crate) fn refunded_by(order_id: &str, total_amount: i64, refunded: i64) -> Self {
        Self {
            order_id: order_id.into(),
            status: if refunded > 0 {
                "PARTIAL_CANCELED".into()
            } else {
                "DONE".into()
            },
            total_amount,
            balance_amount: Some(total_amount - refunded),
        }
    }

    /// Whether TossPayments is holding money for this payment.
    pub(crate) fn keeps_money(&self) -> bool {
        matches!(self.status.as_str(), "DONE" | "PARTIAL_CANCELED")
    }

    pub(crate) fn status(&self) -> &str {
        &self.status
    }

    /// How much of the payment has been given back.
    pub(crate) fn refunded(&self) -> i64 {
        match self.status.as_str() {
            "CANCELED" | "PARTIAL_CANCELED" => {
                (self.total_amount - self.balance_amount.unwrap_or(0)).clamp(0, self.total_amount)
            }
            _ => 0,
        }
    }
}

/// What TossPayments said about an order.
pub(crate) enum Found {
    Payment(PaymentRecord),
    NotFound,
    Unknown,
}

pub(crate) async fn find_order(s: &Shared, order_id: &str) -> Found {
    let Some(billing) = s.billing.as_deref() else {
        return Found::Unknown;
    };
    let Ok(response) = billing
        .client
        .get(format!(
            "{}/v1/payments/orders/{}",
            billing.api_base,
            urlencode(order_id)
        ))
        .header("Authorization", billing.authorization())
        .send()
        .await
    else {
        return Found::Unknown;
    };
    let status = response.status();
    let Ok(body) = response.bytes().await else {
        return Found::Unknown;
    };
    if status.is_success() {
        return match serde_json::from_slice::<PaymentRecord>(&body) {
            Ok(payment) if payment.order_id == order_id => Found::Payment(payment),
            _ => Found::Unknown,
        };
    }
    match serde_json::from_slice::<TossError>(&body) {
        Ok(error) if error.code == "NOT_FOUND_PAYMENT" => Found::NotFound,
        _ => Found::Unknown,
    }
}

/// What a cancel request came to.
#[derive(Debug, PartialEq)]
pub(crate) enum Cancel {
    Done,
    /// TossPayments refused it on its merits; asking again would be refused too.
    Refused(String),
    /// No answer that says either way. The next pass looks the payment up.
    Unknown,
}

/// Read a cancel's response. No idempotency key is sent with a cancel:
/// TossPayments would replay a refusal under it for 15 days, and a full cancel
/// is safe to repeat because TossPayments refuses to cancel a payment twice.
pub(crate) fn cancel_outcome(status: reqwest::StatusCode, body: &[u8]) -> Cancel {
    if status.is_success() {
        return Cancel::Done;
    }
    if status.is_server_error()
        || status == reqwest::StatusCode::TOO_MANY_REQUESTS
        || status == reqwest::StatusCode::CONFLICT
        || merchant_setup_error(status, body).is_some()
    {
        return Cancel::Unknown;
    }
    match serde_json::from_slice::<TossError>(body) {
        // Already given back, or on its way: the lookup settles it.
        Ok(error)
            if matches!(
                error.code.as_str(),
                "ALREADY_CANCELED_PAYMENT"
                    | "ALREADY_REFUNDING_PAYMENT"
                    | "PROVIDER_ERROR"
                    | "FAILED_INTERNAL_SYSTEM_PROCESSING"
                    | "FAILED_REFUND_PROCESS"
            ) =>
        {
            Cancel::Unknown
        }
        Ok(error) => Cancel::Refused(customer_message(&error.message)),
        Err(_) => Cancel::Unknown,
    }
}

async fn cancel_payment(s: &Shared, payment_key: &str) -> Cancel {
    let Some(billing) = s.billing.as_deref() else {
        return Cancel::Unknown;
    };
    let Ok(response) = billing
        .client
        .post(format!(
            "{}/v1/payments/{}/cancel",
            billing.api_base,
            urlencode(payment_key)
        ))
        .header("Authorization", billing.authorization())
        .json(&serde_json::json!({ "cancelReason": "고객 환불 요청" }))
        .send()
        .await
    else {
        return Cancel::Unknown;
    };
    let status = response.status();
    let Ok(body) = response.bytes().await else {
        return Cancel::Unknown;
    };
    if let Some(code) = merchant_setup_error(status, &body) {
        tracing::error!(%code, "TossPayments refused a refund over this control plane's keys or merchant setup");
    }
    cancel_outcome(status, &body)
}

/// Ask for a payment back. Allowed within `REFUND_WINDOW` of paying; once it is
/// given back, the subscription ends and its instances go offline.
pub(crate) async fn request_refund(
    State(s): State<Shared>,
    headers: HeaderMap,
    Path(order_id): Path<String>,
) -> Result<Json<BillingView>, ApiError> {
    let (user_id, _) = customer(&headers, &s.db).await?;
    start_refund(&s, &order_id, Some(user_id), user_id, true).await?;
    Ok(Json(view(&s, user_id).await?))
}

/// Record a refund request and carry it out as far as it goes now; the renewer
/// finishes it otherwise. `owner` limits it to one customer's payments, and
/// `within_window` to `REFUND_WINDOW` after paying: an operator is bound by
/// neither. Returns the customer whose payment it is.
pub(crate) async fn start_refund(
    s: &Shared,
    order_id: &str,
    owner: Option<Uuid>,
    requested_by: Uuid,
    within_window: bool,
) -> Result<Uuid, ApiError> {
    if !billing::billing_on(s) {
        return Err(ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "Subscriptions are not available.".into(),
        ));
    }
    let mut tx = s.db.begin().await?;
    let (user_id, refundable, allowed) = sqlx::query_as::<_, (Uuid, bool, bool)>(&format!(
        "SELECT p.user_id,p.status='paid' AND p.refunded_amount=0 AND p.refund_requested_at IS NULL AND (NOT $3 OR p.settled_at>now()-interval '{REFUND_WINDOW}') AND (p.payment_key IS NOT NULL OR p.provider='polar') AND (NOT $3 OR {UPGRADE_SELF_REFUND}),NOT $3 OR {POLAR_SELF_REFUND} FROM subscription_payments p WHERE p.order_id=$1 AND ($2::uuid IS NULL OR p.user_id=$2) FOR UPDATE OF p"
    ))
    .bind(order_id)
    .bind(owner)
    .bind(within_window)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "Payment not found.".into()))?;
    if !refundable {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "This payment can no longer be refunded.".into(),
        ));
    }
    // Taking an upgrade back returns its old plan, which has to hold the
    // instances.
    if within_window {
        let crowded = sqlx::query_scalar::<_, bool>(
            "SELECT pl.instance_limit IS NOT NULL AND (SELECT count(*) FROM tenants t WHERE t.user_id=p.user_id AND t.lifecycle_state NOT IN ('deprovisioning','retained','purging','purged'))>pl.instance_limit FROM subscription_payments p JOIN plans pl ON pl.id=p.from_plan_id JOIN users u ON u.id=p.user_id WHERE p.order_id=$1 AND p.kind='upgrade' AND NOT (u.is_admin OR u.billing_exempt)",
        )
        .bind(order_id)
        .fetch_optional(&mut *tx)
        .await?
        .unwrap_or(false);
        if crowded {
            return Err(ApiError(
                StatusCode::CONFLICT,
                "Refunding this upgrade returns your previous plan. Delete instances until you are within its limit first.".into(),
            ));
        }
    }
    if !allowed {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "Only an account's first payment by international card can be refunded here. Contact us about this one.".into(),
        ));
    }
    // A charge still being confirmed could otherwise be approved after the
    // refund ended the subscription it was for.
    let charging = sqlx::query_scalar::<_, bool>(
        "SELECT true FROM subscription_payments WHERE user_id=$1 AND status='pending'",
    )
    .bind(user_id)
    .fetch_optional(&mut *tx)
    .await?
    .is_some();
    if charging {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "A payment is being confirmed. Try again shortly.".into(),
        ));
    }
    sqlx::query("UPDATE subscription_payments SET refund_requested_at=now(),refund_requested_by=$2,refund_next_attempt_at=now(),refund_error=NULL WHERE order_id=$1")
        .bind(order_id)
        .bind(requested_by)
        .execute(&mut *tx)
        .await?;
    record(
        &mut tx,
        requested_by,
        "subscription_refund_request",
        "succeeded",
        serde_json::json!({ "order_id": order_id, "customer": user_id }),
        None,
    )
    .await?;
    tx.commit().await?;
    if refund_due(s, Some(order_id)).await.is_err() {
        tracing::warn!(%user_id, "could not carry out a refund at once");
    }
    Ok(user_id)
}

#[derive(sqlx::FromRow)]
struct DueRefund {
    order_id: String,
    user_id: Uuid,
    provider: String,
    payment_key: Option<String>,
    toss_mid: Option<String>,
}

/// Carry one requested refund forward, or the given order's. Returns whether
/// there was one.
pub(crate) async fn refund_due(s: &Shared, order_id: Option<&str>) -> anyhow::Result<bool> {
    if !billing::billing_on(s) {
        return Ok(false);
    }
    let mut tx = s.db.begin().await?;
    // The subscription's lease keeps a refund and a charge for the same
    // account from running at once.
    let Some(due) = sqlx::query_as::<_, DueRefund>(
        "SELECT p.order_id,p.user_id,p.provider,p.payment_key,COALESCE(p.toss_mid,s.toss_mid) toss_mid FROM subscription_payments p JOIN subscriptions s ON s.user_id=p.user_id WHERE p.refund_requested_at IS NOT NULL AND p.refunded_at IS NULL AND p.refund_next_attempt_at<=now() AND (s.lease_until IS NULL OR s.lease_until<now()) AND ($1::text IS NULL OR p.order_id=$1) AND (p.provider<>'toss' OR $2) AND (p.provider<>'polar' OR $3) ORDER BY p.refund_next_attempt_at LIMIT 1 FOR UPDATE OF p,s SKIP LOCKED",
    )
    .bind(order_id)
    .bind(s.billing.is_some())
    .bind(s.polar.is_some())
    .fetch_optional(&mut *tx)
    .await?
    else {
        return Ok(false);
    };
    sqlx::query(&format!("UPDATE subscription_payments SET refund_next_attempt_at=now()+interval '{REFUND_RETRY}' WHERE order_id=$1"))
        .bind(&due.order_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE subscriptions SET lease_until=now()+interval '3 minutes' WHERE user_id=$1")
        .bind(due.user_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;

    let outcome = match (due.provider.as_str(), s.billing.as_deref()) {
        ("polar", _) => refund_through_polar(s, &due).await,
        (_, Some(billing)) => refund(s, billing, &due).await,
        _ => Ok(()),
    };
    sqlx::query("UPDATE subscriptions SET lease_until=NULL WHERE user_id=$1")
        .bind(due.user_id)
        .execute(&s.db)
        .await?;
    outcome?;
    Ok(true)
}

async fn refund(s: &Shared, billing: &billing::Config, due: &DueRefund) -> anyhow::Result<()> {
    // Only the MID that took a payment can give it back.
    let refused = if let Some(problem) =
        mid_problem(billing.mid.as_deref(), due.toss_mid.as_deref())
    {
        tracing::error!(order_id = %due.order_id, %problem, "refund refused: the payment belongs to another TossPayments MID");
        Some("This payment was taken under an earlier payment setup and cannot be refunded here. Please contact us.".to_owned())
    } else if due.payment_key.is_none() {
        Some("This payment cannot be refunded here. Please contact us.".to_owned())
    } else {
        None
    };
    if let Some(reason) = refused {
        return withdraw_refund(s, due, &reason).await;
    }
    match find_order(s, &due.order_id).await {
        Found::Payment(payment) if payment.refunded() > 0 => {
            return apply_cancellation(s, &due.order_id, &payment).await;
        }
        Found::Payment(payment) if payment.status == "DONE" => {}
        // Not known, or not a payment that can be cancelled yet: ask again.
        _ => return Ok(()),
    }
    match cancel_payment(s, due.payment_key.as_deref().unwrap_or_default()).await {
        // Settled from the record rather than the response, by the same rule
        // as every other path.
        Cancel::Done => {
            if let Found::Payment(payment) = find_order(s, &due.order_id).await {
                apply_cancellation(s, &due.order_id, &payment).await?;
            }
            Ok(())
        }
        Cancel::Refused(reason) => withdraw_refund(s, due, &reason).await,
        Cancel::Unknown => Ok(()),
    }
}

async fn refund_through_polar(s: &Shared, due: &DueRefund) -> anyhow::Result<()> {
    match crate::billing_polar::refund_step(s, &due.order_id).await? {
        crate::billing_polar::RefundStep::Refused(reason) => {
            withdraw_refund(s, due, &customer_message(&reason)).await
        }
        crate::billing_polar::RefundStep::Refunded | crate::billing_polar::RefundStep::Waiting => {
            Ok(())
        }
    }
}

/// TossPayments will not give this payment back. The request is withdrawn so
/// the customer can see why and ask again, and the subscription renews again.
async fn withdraw_refund(s: &Shared, due: &DueRefund, reason: &str) -> anyhow::Result<()> {
    let mut tx = s.db.begin().await?;
    let withdrawn = sqlx::query("UPDATE subscription_payments SET refund_requested_at=NULL,refund_requested_by=NULL,refund_next_attempt_at=NULL,refund_error=$2 WHERE order_id=$1 AND refunded_at IS NULL")
        .bind(&due.order_id)
        .bind(reason)
        .execute(&mut *tx)
        .await?;
    if withdrawn.rows_affected() > 0 {
        record(
            &mut tx,
            due.user_id,
            "subscription_refund",
            "failed",
            serde_json::json!({ "order_id": due.order_id }),
            Some(reason),
        )
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Record what TossPayments has given back of a payment, from its own record:
/// a refund asked for here, or one made in the TossPayments dashboard. A
/// customer's own full refund ends the subscription and takes its instances
/// offline at once, and one of the period being served takes that period's
/// upgrades back with it. What an operator gives back, here or in the dashboard,
/// returns the money and changes nothing else.
pub(crate) async fn apply_cancellation(
    s: &Shared,
    order_id: &str,
    payment: &PaymentRecord,
) -> anyhow::Result<()> {
    let refunded = payment.refunded();
    let mut tx = s.db.begin().await?;
    let Some((user_id, amount, previously, current_period, running, kind, requested_by, period_end, back_to)) = sqlx::query_as::<_, (Uuid, i64, i64, bool, bool, String, Option<Uuid>, OffsetDateTime, Option<(Uuid, i64)>)>(
        "SELECT p.user_id,p.amount,p.refunded_amount,p.period_end=s.current_period_end,s.status IN ('active','past_due'),p.kind,p.refund_requested_by,p.period_end,CASE WHEN p.from_plan_id IS NOT NULL AND p.from_amount IS NOT NULL AND s.plan_id=p.plan_id THEN (p.from_plan_id,p.from_amount) END FROM subscription_payments p JOIN subscriptions s ON s.user_id=p.user_id JOIN users u ON u.id=p.user_id WHERE p.order_id=$1 AND p.status IN ('paid','refunded') FOR UPDATE OF p,s",
    )
    .bind(order_id)
    .fetch_optional(&mut *tx)
    .await?
    else {
        return Ok(());
    };
    if payment.total_amount != amount || refunded <= previously {
        return Ok(());
    }
    let full = refunded >= amount;
    sqlx::query("UPDATE subscription_payments SET refunded_amount=$2,refunded_at=now(),status=CASE WHEN $3 THEN 'refunded' ELSE status END,refund_next_attempt_at=NULL,refund_error=NULL WHERE order_id=$1")
        .bind(order_id)
        .bind(refunded)
        .bind(full)
        .execute(&mut *tx)
        .await?;
    record(
        &mut tx,
        user_id,
        "subscription_refund",
        "succeeded",
        serde_json::json!({ "order_id": order_id, "refunded_amount": refunded }),
        None,
    )
    .await?;
    let own = full && running && requested_by == Some(user_id);
    // A customer's own refund of an upgrade for the period being served takes
    // the upgrade back: the plan and price it moved from return, and the
    // subscription goes on. Anything else of theirs ends it.
    let reverted = match back_to {
        Some((plan_id, price)) if own && kind == "upgrade" && current_period => {
            sqlx::query("UPDATE subscriptions SET plan_id=$2,amount=$3,pending_plan_id=CASE WHEN pending_plan_id=$2 THEN NULL ELSE pending_plan_id END,updated_at=now() WHERE user_id=$1")
                .bind(user_id)
                .bind(plan_id)
                .bind(price)
                .execute(&mut *tx)
                .await?;
            record(
                &mut tx,
                user_id,
                "subscription_plan",
                "succeeded",
                serde_json::json!({ "change": "upgrade_refunded", "plan_id": plan_id, "order_id": order_id }),
                None,
            )
            .await?;
            Some(plan_id)
        }
        _ => None,
    };
    let ends = own && reverted.is_none();
    let mut suspended = Vec::new();
    if ends {
        if current_period && kind != "upgrade" {
            // The renewer carries these through like any other refund.
            sqlx::query("UPDATE subscription_payments SET refund_requested_at=now(),refund_requested_by=$3,refund_next_attempt_at=now(),refund_error=NULL WHERE user_id=$1 AND kind='upgrade' AND period_end=$2 AND status='paid' AND refunded_amount=0 AND refund_requested_at IS NULL AND (payment_key IS NOT NULL OR provider='polar')")
                .bind(user_id)
                .bind(period_end)
                .bind(requested_by)
                .execute(&mut *tx)
                .await?;
        }
        suspended = billing::end_subscription(&mut tx, user_id, "canceled").await?;
    }
    crate::billing_mail::refund(
        &mut tx,
        user_id,
        order_id,
        refunded,
        ends,
        !suspended.is_empty(),
        reverted,
    )
    .await?;
    tx.commit().await?;
    billing::wake(s, &suspended).await;
    Ok(())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WebhookEvent {
    event_type: Option<String>,
    data: Option<serde_json::Value>,
}

/// TossPayments' webhook. Payment events are unsigned, so their content is
/// only a reason to look the payment up. `BILLING_DELETED` cannot be looked
/// up, but it names the key it is about, which only TossPayments and this
/// control plane know, and all it does is stop charging that key.
pub(crate) async fn webhook(
    State(s): State<Shared>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    if s.billing.is_none() {
        return StatusCode::OK;
    }
    let status = answer_webhook(&s, &body).await;
    keep_webhook(&s, &headers, &body, status).await;
    status
}

/// Keep what arrived. A failure here must not change the answer, or a database
/// problem would become a stream of resends.
async fn keep_webhook(s: &Shared, headers: &HeaderMap, body: &[u8], status: StatusCode) {
    let event = (body.len() <= WEBHOOK_BODY_LIMIT)
        .then(|| serde_json::from_slice::<serde_json::Value>(body).ok())
        .flatten()
        .map(|mut event| {
            remove_secrets(&mut event);
            event
        });
    let text = |value: Option<&serde_json::Value>| {
        value
            .and_then(|value| value.as_str())
            .map(|value| value.chars().take(200).collect::<String>())
    };
    let kept = sqlx::query("INSERT INTO toss_webhook_deliveries(event_type,order_id,signed,headers,payload,http_status) VALUES($1,$2,$3,$4,$5,$6)")
        .bind(text(event.as_ref().and_then(|event| event.get("eventType"))))
        .bind(text(event.as_ref().and_then(|event| event.pointer("/data/orderId"))))
        .bind(WEBHOOK_SIGNATURE_HEADERS.iter().any(|name| headers.contains_key(*name)))
        .bind(kept_headers(headers))
        .bind(event.map(|event| event.to_string()))
        .bind(i32::from(status.as_u16()))
        .execute(&s.db)
        .await;
    if kept.is_err() {
        tracing::warn!("could not keep a TossPayments webhook");
    }
}

fn kept_headers(headers: &HeaderMap) -> serde_json::Value {
    let kept = headers
        .iter()
        .filter(|(name, _)| !matches!(name.as_str(), "authorization" | "cookie"))
        .filter_map(|(name, value)| {
            let value: String = value.to_str().ok()?.chars().take(1000).collect();
            Some((name.as_str().to_owned(), serde_json::Value::String(value)))
        })
        .take(50)
        .collect::<serde_json::Map<_, _>>();
    serde_json::Value::Object(kept)
}

/// Billing keys are what charges a card, and a virtual account's `secret`
/// verifies its deposits; neither belongs in a log.
fn remove_secrets(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(fields) => {
            for (name, field) in fields.iter_mut() {
                if matches!(name.as_str(), "billingKey" | "secret") {
                    *field = "[removed]".into();
                } else {
                    remove_secrets(field);
                }
            }
        }
        serde_json::Value::Array(items) => items.iter_mut().for_each(remove_secrets),
        _ => {}
    }
}

async fn answer_webhook(s: &Shared, body: &[u8]) -> StatusCode {
    if body.len() > WEBHOOK_BODY_LIMIT {
        return StatusCode::OK;
    }
    let Ok(event) = serde_json::from_slice::<WebhookEvent>(body) else {
        return StatusCode::OK;
    };
    let field = |name: &str| {
        event
            .data
            .as_ref()
            .and_then(|data| data.get(name))
            .and_then(|value| value.as_str())
            .map(str::to_owned)
    };
    let handled = match event.event_type.as_deref() {
        Some("PAYMENT_STATUS_CHANGED") => match field("orderId") {
            Some(order_id) => {
                tokio::time::timeout(WEBHOOK_LOOKUP_TIMEOUT, payment_changed(s, &order_id))
                    .await
                    .unwrap_or(Ok(false))
            }
            None => Ok(true),
        },
        Some("BILLING_DELETED") => match field("billingKey") {
            Some(billing_key) => billing_key_deleted(s, &billing_key).await.map(|_| true),
            None => Ok(true),
        },
        _ => Ok(true),
    };
    match handled {
        Ok(true) => StatusCode::OK,
        // TossPayments sends it again.
        Ok(false) | Err(_) => StatusCode::SERVICE_UNAVAILABLE,
    }
}

/// A payment changed at TossPayments. Returns false when it could not be
/// looked up, so the event comes again.
async fn payment_changed(s: &Shared, order_id: &str) -> anyhow::Result<bool> {
    let Some((user_id, status)) = sqlx::query_as::<_, (Uuid, String)>(
        "SELECT user_id,status FROM subscription_payments WHERE order_id=$1",
    )
    .bind(order_id)
    .fetch_optional(&s.db)
    .await?
    else {
        return Ok(true);
    };
    let payment = match find_order(s, order_id).await {
        Found::Payment(payment) => payment,
        Found::NotFound => return Ok(true),
        Found::Unknown => return Ok(false),
    };
    if status == "pending" {
        // Settling a charge belongs to the renewer, which looks it up again and
        // records any cancellation; this only brings that forward.
        sqlx::query("UPDATE subscriptions SET next_attempt_at=now() WHERE user_id=$1 AND status IN ('incomplete','active','past_due')")
            .bind(user_id)
            .execute(&s.db)
            .await?;
    } else if payment.refunded() > 0 {
        apply_cancellation(s, order_id, &payment).await?;
    }
    Ok(true)
}

/// TossPayments deleted a billing key, as when the cardholder removed it. A
/// running subscription keeps its paid period and ends with it unless a new
/// card is registered.
async fn billing_key_deleted(s: &Shared, billing_key: &str) -> anyhow::Result<()> {
    let mut tx = s.db.begin().await?;
    sqlx::query("UPDATE retired_billing_keys SET billing_key=NULL,deleted_at=now() WHERE billing_key=$1 AND deleted_at IS NULL")
        .bind(billing_key)
        .execute(&mut *tx)
        .await?;
    let holder = sqlx::query_as::<_, (Uuid, Option<String>)>(
        "UPDATE subscriptions s SET billing_key=NULL,toss_mid=NULL,cancel_at_period_end=(s.status<>'incomplete'),updated_at=now() FROM users u WHERE u.id=s.user_id AND s.billing_key=$1 RETURNING s.user_id,u.email",
    )
    .bind(billing_key)
    .fetch_optional(&mut *tx)
    .await?;
    if let Some((user_id, _)) = &holder {
        record(
            &mut tx,
            *user_id,
            "subscription_card",
            "succeeded",
            serde_json::json!({ "change": "deleted_at_tosspayments" }),
            None,
        )
        .await?;
    }
    tx.commit().await?;
    if let Some((user_id, _)) = holder {
        notify(s, user_id, billing::Notice::CardRemoved).await;
    }
    Ok(())
}

#[derive(sqlx::FromRow)]
struct RetiredKey {
    id: Uuid,
    billing_key: String,
    toss_mid: Option<String>,
}

/// Delete one retired billing key at TossPayments, if one is due. Returns
/// whether there was one.
pub(crate) async fn delete_retired_key(s: &Shared) -> anyhow::Result<bool> {
    let Some(billing) = s.billing.as_deref() else {
        return Ok(false);
    };
    let mut tx = s.db.begin().await?;
    let Some(key) = sqlx::query_as::<_, RetiredKey>(
        "SELECT id,billing_key,toss_mid FROM retired_billing_keys WHERE deleted_at IS NULL AND next_attempt_at<=now() ORDER BY next_attempt_at LIMIT 1 FOR UPDATE SKIP LOCKED",
    )
    .fetch_optional(&mut *tx)
    .await?
    else {
        return Ok(false);
    };
    // Backing off before asking, so a dead control plane mid-request does not
    // make this one ask again at once: an hour, doubling, up to a day.
    sqlx::query("UPDATE retired_billing_keys SET attempts=attempts+1,next_attempt_at=now()+least(interval '1 hour'*power(2,attempts),interval '1 day') WHERE id=$1")
        .bind(key.id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;

    // A key of another MID cannot be deleted with this one's secret key; it
    // waits for an operator, who can delete it in the TossPayments dashboard.
    if let Some(problem) = mid_problem(billing.mid.as_deref(), key.toss_mid.as_deref()) {
        sqlx::query("UPDATE retired_billing_keys SET last_error=$2 WHERE id=$1")
            .bind(key.id)
            .bind(problem)
            .execute(&s.db)
            .await?;
        return Ok(true);
    }
    let response = billing
        .client
        .delete(format!(
            "{}/v1/billing/{}",
            billing.api_base,
            urlencode(&key.billing_key)
        ))
        .header("Authorization", billing.authorization())
        .send()
        .await;
    let error = match response {
        Ok(response) => {
            let status = response.status();
            let body = response.bytes().await.unwrap_or_default();
            let code = serde_json::from_slice::<TossError>(&body)
                .map(|error| error.code)
                .unwrap_or_default();
            // Gone already is as deleted as it will ever be.
            if status.is_success() || code == "NOT_FOUND_BILLING" {
                None
            } else if code.is_empty() {
                Some(status.as_u16().to_string())
            } else {
                Some(code.chars().take(64).collect::<String>())
            }
        }
        Err(_) => Some("no answer".to_owned()),
    };
    match error {
        None => {
            sqlx::query("UPDATE retired_billing_keys SET billing_key=NULL,deleted_at=now(),last_error=NULL WHERE id=$1")
                .bind(key.id)
                .execute(&s.db)
                .await?;
        }
        Some(error) => {
            sqlx::query("UPDATE retired_billing_keys SET last_error=$2 WHERE id=$1")
                .bind(key.id)
                .bind(error)
                .execute(&s.db)
                .await?;
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refusal_on_the_merits_withdraws_and_anything_else_is_asked_again() {
        let refused = cancel_outcome(
            reqwest::StatusCode::FORBIDDEN,
            "{\"code\":\"NOT_CANCELABLE_PAYMENT\",\"message\":\"취소 할 수 없는 결제 입니다.\"}"
                .as_bytes(),
        );
        assert_eq!(
            refused,
            Cancel::Refused("취소 할 수 없는 결제 입니다.".into())
        );
        for (status, code) in [
            (reqwest::StatusCode::BAD_REQUEST, "ALREADY_CANCELED_PAYMENT"),
            (reqwest::StatusCode::BAD_REQUEST, "PROVIDER_ERROR"),
            (reqwest::StatusCode::UNAUTHORIZED, "UNAUTHORIZED_KEY"),
            (reqwest::StatusCode::NOT_FOUND, "NOT_FOUND_MERCHANT"),
            (
                reqwest::StatusCode::INTERNAL_SERVER_ERROR,
                "FAILED_INTERNAL_SYSTEM_PROCESSING",
            ),
        ] {
            let body = format!("{{\"code\":\"{code}\",\"message\":\"x\"}}");
            assert_eq!(
                cancel_outcome(status, body.as_bytes()),
                Cancel::Unknown,
                "{code}"
            );
        }
        assert_eq!(cancel_outcome(reqwest::StatusCode::OK, b"{}"), Cancel::Done);
    }

    #[test]
    fn a_refund_is_what_the_cancellations_took_off_the_payment() {
        let record = |status: &str, balance: Option<i64>| PaymentRecord {
            order_id: "o".into(),
            status: status.into(),
            total_amount: 9900,
            balance_amount: balance,
        };
        assert_eq!(record("DONE", Some(9900)).refunded(), 0);
        assert_eq!(record("CANCELED", Some(0)).refunded(), 9900);
        assert_eq!(record("CANCELED", None).refunded(), 9900);
        assert_eq!(record("PARTIAL_CANCELED", Some(4900)).refunded(), 5000);
    }
}
