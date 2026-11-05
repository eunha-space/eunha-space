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

/// How long to wait before asking about a refund whose outcome is not known.
const REFUND_RETRY: &str = "5 minutes";

/// How long a webhook may spend looking a payment up. TossPayments waits ten
/// seconds for an answer and sends the event again if it gets none.
const WEBHOOK_LOOKUP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(7);

/// A payment as TossPayments records it, as far as refunds need it.
#[derive(Debug, Deserialize)]
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
    /// How much of the payment has been given back.
    fn refunded(&self) -> i64 {
        match self.status.as_str() {
            "CANCELED" | "PARTIAL_CANCELED" => {
                (self.total_amount - self.balance_amount.unwrap_or(0)).clamp(0, self.total_amount)
            }
            _ => 0,
        }
    }
}

/// What TossPayments said about an order.
enum Found {
    Payment(PaymentRecord),
    NotFound,
    Unknown,
}

async fn find_order(s: &Shared, order_id: &str) -> Found {
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

/// Ask for a payment back. Allowed within `REFUND_WINDOW` of paying; it stops
/// the subscription renewing.
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
    if s.billing.is_none() {
        return Err(ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "Subscriptions are not available.".into(),
        ));
    }
    let mut tx = s.db.begin().await?;
    let (user_id, refundable) = sqlx::query_as::<_, (Uuid, bool)>(&format!(
        "SELECT user_id,status='paid' AND refunded_amount=0 AND refund_requested_at IS NULL AND (NOT $3 OR settled_at>now()-interval '{REFUND_WINDOW}') AND payment_key IS NOT NULL FROM subscription_payments WHERE order_id=$1 AND ($2::uuid IS NULL OR user_id=$2) FOR UPDATE"
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
    payment_key: Option<String>,
    toss_mid: Option<String>,
}

/// Carry one requested refund forward, or the given order's. Returns whether
/// there was one.
pub(crate) async fn refund_due(s: &Shared, order_id: Option<&str>) -> anyhow::Result<bool> {
    let Some(billing) = s.billing.as_deref() else {
        return Ok(false);
    };
    let mut tx = s.db.begin().await?;
    // The subscription's lease keeps a refund and a charge for the same
    // account from running at once.
    let Some(due) = sqlx::query_as::<_, DueRefund>(
        "SELECT p.order_id,p.user_id,p.payment_key,s.toss_mid FROM subscription_payments p JOIN subscriptions s ON s.user_id=p.user_id WHERE p.refund_requested_at IS NOT NULL AND p.refunded_at IS NULL AND p.refund_next_attempt_at<=now() AND (s.lease_until IS NULL OR s.lease_until<now()) AND ($1::text IS NULL OR p.order_id=$1) ORDER BY p.refund_next_attempt_at LIMIT 1 FOR UPDATE OF p,s SKIP LOCKED",
    )
    .bind(order_id)
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

    let outcome = refund(s, billing, &due).await;
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
/// a refund asked for here, or one made in the TossPayments dashboard. A full
/// refund stops the subscription renewing, and one of the period being served
/// also takes back the paid time.
async fn apply_cancellation(
    s: &Shared,
    order_id: &str,
    payment: &PaymentRecord,
) -> anyhow::Result<()> {
    let refunded = payment.refunded();
    let mut tx = s.db.begin().await?;
    let Some((user_id, amount, previously, current_period, running)) = sqlx::query_as::<_, (Uuid, i64, i64, bool, bool)>(
        "SELECT p.user_id,p.amount,p.refunded_amount,p.period_end=s.current_period_end,s.status IN ('active','past_due') FROM subscription_payments p JOIN subscriptions s ON s.user_id=p.user_id JOIN users u ON u.id=p.user_id WHERE p.order_id=$1 AND p.status IN ('paid','refunded') FOR UPDATE OF p,s",
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
    let ends_on = if running {
        Some(billing::stop_renewing_after_refund(&mut tx, user_id, full && current_period).await?)
    } else {
        None
    };
    crate::billing_mail::refund(&mut tx, user_id, order_id, refunded, ends_on.as_deref()).await?;
    tx.commit().await?;
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
pub(crate) async fn webhook(State(s): State<Shared>, body: Bytes) -> StatusCode {
    if s.billing.is_none() || body.len() > 64 * 1024 {
        return StatusCode::OK;
    }
    let Ok(event) = serde_json::from_slice::<WebhookEvent>(&body) else {
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
                tokio::time::timeout(WEBHOOK_LOOKUP_TIMEOUT, payment_changed(&s, &order_id))
                    .await
                    .unwrap_or(Ok(false))
            }
            None => Ok(true),
        },
        Some("BILLING_DELETED") => match field("billingKey") {
            Some(billing_key) => billing_key_deleted(&s, &billing_key).await.map(|_| true),
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
    if payment.refunded() > 0 {
        apply_cancellation(s, order_id, &payment).await?;
    } else if status == "pending" {
        // Settling a charge belongs to the renewer, which looks it up again;
        // this only brings that forward.
        sqlx::query("UPDATE subscriptions SET next_attempt_at=now() WHERE user_id=$1 AND status IN ('incomplete','active','past_due')")
            .bind(user_id)
            .execute(&s.db)
            .await?;
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
