//! The operators' view of billing: who is subscribed, what they paid, and the
//! few things an operator does about it — charging a declined renewal again,
//! exempting an account, refunding outside the customer's seven days.
//!
//! Nothing here returns a billing key, a customer key or a secret: an account's
//! card is its company and the number as TossPayments masked it.

use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{ApiError, Shared, admin_user, billing, billing_sync};

#[derive(Serialize, sqlx::FromRow)]
pub(crate) struct Summary {
    active: i64,
    past_due: i64,
    canceling: i64,
    ended: i64,
    /// What the active and retried TossPayments subscriptions bring in each
    /// month, in won. Polar's are in `revenue`, by currency.
    monthly_revenue: i64,
    /// Accounts running instances with no subscription and no exemption, such
    /// as customers from before billing.
    unbilled: i64,
    refunds_open: i64,
    /// Retired billing keys TossPayments has not confirmed deleting after
    /// several tries.
    keys_waiting: i64,
    mismatches: i64,
}

#[derive(Serialize, sqlx::FromRow)]
pub(crate) struct Account {
    user_id: Uuid,
    email: Option<String>,
    is_admin: bool,
    billing_exempt: bool,
    /// Null for an account that never subscribed.
    status: Option<String>,
    amount: Option<i64>,
    provider: Option<String>,
    currency: Option<String>,
    card_company: Option<String>,
    card_number: Option<String>,
    has_card: bool,
    #[serde(with = "time::serde::rfc3339::option")]
    current_period_end: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    next_attempt_at: Option<OffsetDateTime>,
    cancel_at_period_end: bool,
    failed_attempts: i32,
    last_error: Option<String>,
    /// Instances counting against the account's limit.
    instances: i64,
    /// Instances an ended subscription took offline.
    suspended_instances: i64,
    refund_open: bool,
    #[serde(with = "time::serde::rfc3339::option")]
    last_paid_at: Option<OffsetDateTime>,
}

#[derive(Serialize, sqlx::FromRow)]
pub(crate) struct Plan {
    id: Uuid,
    name_en: String,
    name_ko: String,
    amount: i64,
    instance_limit: Option<i64>,
    user_limit: Option<i64>,
    media_gb: Option<i64>,
    database_gb: Option<i64>,
    monthly_activities: Option<i64>,
    /// The Polar product it is sold as to international cards, and its price.
    polar_product_id: Option<String>,
    polar_amount: Option<i64>,
    polar_currency: Option<String>,
    active: bool,
    /// Running subscriptions on it, or moving to it at their next renewal.
    subscribers: i64,
    /// Whether nothing refers to it, so it can be deleted rather than retired.
    deletable: bool,
}

const PLAN_COLUMNS: &str = "id,name_en,name_ko,amount,instance_limit,user_limit,media_gb,database_gb,monthly_activities,polar_product_id,polar_amount,polar_currency,active,(SELECT count(*) FROM subscriptions s WHERE s.status IN ('active','past_due') AND plans.id IN (s.plan_id,s.pending_plan_id)) subscribers,NOT EXISTS(SELECT 1 FROM subscriptions s WHERE plans.id IN (s.plan_id,s.pending_plan_id)) AND NOT EXISTS(SELECT 1 FROM subscription_payments p WHERE plans.id IN (p.plan_id,p.from_plan_id)) deletable";

#[derive(Serialize)]
pub(crate) struct Overview {
    enabled: bool,
    toss: bool,
    polar: bool,
    plans: Vec<Plan>,
    summary: Summary,
    /// What the running subscriptions bring in each month, by currency.
    revenue: Vec<Revenue>,
    accounts: Vec<Account>,
    mismatches: Vec<Mismatch>,
    /// The last day compared with TossPayments, and any failure since.
    last_check: Option<LastCheck>,
    webhooks: Vec<Webhook>,
    polar_webhooks: Vec<PolarWebhook>,
}

#[derive(Serialize, sqlx::FromRow)]
pub(crate) struct Revenue {
    currency: String,
    amount: i64,
    subscriptions: i64,
}

/// A Polar webhook as received. Its payload is Polar's subscription or order,
/// which holds no card details.
#[derive(Serialize, sqlx::FromRow)]
pub(crate) struct PolarWebhook {
    id: Uuid,
    #[serde(with = "time::serde::rfc3339")]
    received_at: OffsetDateTime,
    webhook_id: Option<String>,
    event_type: Option<String>,
    verified: bool,
    payload: Option<String>,
    http_status: i32,
}

/// What the daily comparison found that an operator has to decide.
#[derive(Serialize, sqlx::FromRow)]
pub(crate) struct Mismatch {
    id: Uuid,
    order_id: String,
    kind: String,
    detail: String,
    #[serde(with = "time::serde::rfc3339")]
    found_at: OffsetDateTime,
}

#[derive(Serialize, sqlx::FromRow)]
pub(crate) struct LastCheck {
    day: String,
    transactions: Option<i32>,
    error: Option<String>,
}

/// A webhook as received. Billing keys were removed before it was kept.
#[derive(Serialize, sqlx::FromRow)]
pub(crate) struct Webhook {
    id: Uuid,
    #[serde(with = "time::serde::rfc3339")]
    received_at: OffsetDateTime,
    event_type: Option<String>,
    order_id: Option<String>,
    signed: bool,
    headers: serde_json::Value,
    payload: Option<String>,
    http_status: i32,
}

pub(crate) async fn overview(
    State(s): State<Shared>,
    headers: HeaderMap,
) -> Result<Json<Overview>, ApiError> {
    admin_user(&headers, &s.db).await?;
    let summary = sqlx::query_as::<_, Summary>(
        "SELECT
            (SELECT count(*) FROM subscriptions WHERE status='active' AND NOT cancel_at_period_end) active,
            (SELECT count(*) FROM subscriptions WHERE status='past_due') past_due,
            (SELECT count(*) FROM subscriptions WHERE status='active' AND cancel_at_period_end) canceling,
            (SELECT count(*) FROM subscriptions WHERE status IN ('canceled','lapsed')) ended,
            (SELECT coalesce(sum(amount),0)::bigint FROM subscriptions WHERE status IN ('active','past_due') AND NOT cancel_at_period_end AND currency='KRW') monthly_revenue,
            (SELECT count(*) FROM users u WHERE NOT u.is_admin AND NOT u.billing_exempt
                AND EXISTS(SELECT 1 FROM tenants t WHERE t.user_id=u.id AND t.lifecycle_state IN ('provisioning','ready','failed'))
                AND NOT EXISTS(SELECT 1 FROM subscriptions s WHERE s.user_id=u.id AND s.status IN ('active','past_due'))) unbilled,
            (SELECT count(*) FROM subscription_payments WHERE refund_requested_at IS NOT NULL AND refunded_at IS NULL) refunds_open,
            (SELECT count(*) FROM retired_billing_keys WHERE deleted_at IS NULL AND attempts>=3) keys_waiting,
            (SELECT count(*) FROM toss_mismatches WHERE resolved_at IS NULL) mismatches",
    )
    .fetch_one(&s.db)
    .await?;
    let accounts = sqlx::query_as::<_, Account>(
        "SELECT u.id user_id,u.email,u.is_admin,u.billing_exempt,s.status,s.amount,s.provider,s.currency,s.card_company,s.card_number,
            coalesce(s.billing_key IS NOT NULL,false) has_card,s.current_period_end,s.next_attempt_at,
            coalesce(s.cancel_at_period_end,false) cancel_at_period_end,coalesce(s.failed_attempts,0) failed_attempts,s.last_error,
            (SELECT count(*) FROM tenants t WHERE t.user_id=u.id AND t.lifecycle_state IN ('provisioning','ready','failed')) instances,
            (SELECT count(*) FROM tenants t WHERE t.user_id=u.id AND t.billing_suspended_at IS NOT NULL AND t.lifecycle_state IN ('deprovisioning','retained')) suspended_instances,
            EXISTS(SELECT 1 FROM subscription_payments p WHERE p.user_id=u.id AND p.refund_requested_at IS NOT NULL AND p.refunded_at IS NULL) refund_open,
            (SELECT max(settled_at) FROM subscription_payments p WHERE p.user_id=u.id AND p.status IN ('paid','refunded')) last_paid_at
        FROM users u LEFT JOIN subscriptions s ON s.user_id=u.id
        WHERE u.email IS NOT NULL AND (
            (s.user_id IS NOT NULL AND (s.status<>'incomplete' OR s.billing_key IS NOT NULL
                OR EXISTS(SELECT 1 FROM subscription_payments p WHERE p.user_id=u.id)))
            OR u.billing_exempt
            OR (NOT u.is_admin AND EXISTS(SELECT 1 FROM tenants t WHERE t.user_id=u.id AND t.lifecycle_state IN ('provisioning','ready','failed'))))
        ORDER BY CASE s.status WHEN 'past_due' THEN 0 WHEN 'incomplete' THEN 1 WHEN 'active' THEN 2 ELSE 3 END, u.email
        LIMIT 500",
    )
    .fetch_all(&s.db)
    .await?;
    let plans = sqlx::query_as::<_, Plan>(&format!(
        "SELECT {PLAN_COLUMNS} FROM plans ORDER BY active DESC,amount,created_at"
    ))
    .fetch_all(&s.db)
    .await?;
    let mismatches = sqlx::query_as::<_, Mismatch>(
        "SELECT id,order_id,kind,detail,found_at FROM toss_mismatches WHERE resolved_at IS NULL ORDER BY found_at DESC LIMIT 100",
    )
    .fetch_all(&s.db)
    .await?;
    let last_check = sqlx::query_as::<_, LastCheck>(
        "SELECT to_char(day,'YYYY-MM-DD') AS day,transactions,error FROM toss_transaction_checks ORDER BY day DESC LIMIT 1",
    )
    .fetch_optional(&s.db)
    .await?;
    let revenue = sqlx::query_as::<_, Revenue>(
        "SELECT currency,sum(amount)::bigint amount,count(*) subscriptions FROM subscriptions WHERE status IN ('active','past_due') AND NOT cancel_at_period_end GROUP BY currency ORDER BY currency",
    )
    .fetch_all(&s.db)
    .await?;
    let polar_webhooks = sqlx::query_as::<_, PolarWebhook>(
        "SELECT id,received_at,webhook_id,event_type,verified,payload,http_status FROM polar_webhook_deliveries ORDER BY received_at DESC LIMIT 20",
    )
    .fetch_all(&s.db)
    .await?;
    let webhooks = sqlx::query_as::<_, Webhook>(
        "SELECT id,received_at,event_type,order_id,signed,headers,payload,http_status FROM toss_webhook_deliveries ORDER BY received_at DESC LIMIT 20",
    )
    .fetch_all(&s.db)
    .await?;
    Ok(Json(Overview {
        enabled: billing::billing_on(&s),
        toss: s.billing.is_some(),
        polar: s.polar.is_some(),
        plans,
        summary,
        revenue,
        accounts,
        mismatches,
        last_check,
        webhooks,
        polar_webhooks,
    }))
}

#[derive(Serialize, sqlx::FromRow)]
pub(crate) struct Payment {
    order_id: String,
    amount: i64,
    provider: String,
    currency: String,
    status: String,
    #[serde(with = "time::serde::rfc3339")]
    period_start: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    period_end: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    requested_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    settled_at: Option<OffsetDateTime>,
    receipt_url: Option<String>,
    failure_code: Option<String>,
    failure_message: Option<String>,
    refunded_amount: i64,
    refund_pending: bool,
    refund_error: Option<String>,
    /// An operator may refund any paid payment not refunded yet.
    refundable: bool,
}

#[derive(Serialize, sqlx::FromRow)]
pub(crate) struct RetiredKey {
    #[serde(with = "time::serde::rfc3339")]
    retired_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    deleted_at: Option<OffsetDateTime>,
    attempts: i32,
    last_error: Option<String>,
}

#[derive(Serialize)]
pub(crate) struct AccountDetail {
    payments: Vec<Payment>,
    retired_keys: Vec<RetiredKey>,
}

pub(crate) async fn account(
    State(s): State<Shared>,
    headers: HeaderMap,
    Path(user_id): Path<Uuid>,
) -> Result<Json<AccountDetail>, ApiError> {
    admin_user(&headers, &s.db).await?;
    let payments = sqlx::query_as::<_, Payment>(
        "SELECT order_id,amount,provider,currency,status,period_start,period_end,requested_at,settled_at,receipt_url,failure_code,failure_message,refunded_amount,
            (refund_requested_at IS NOT NULL AND refunded_at IS NULL) refund_pending,refund_error,
            (status='paid' AND refunded_amount=0 AND refund_requested_at IS NULL AND (payment_key IS NOT NULL OR provider='polar')) refundable
        FROM subscription_payments WHERE user_id=$1 ORDER BY requested_at DESC LIMIT 100",
    )
    .bind(user_id)
    .fetch_all(&s.db)
    .await?;
    let retired_keys = sqlx::query_as::<_, RetiredKey>(
        "SELECT retired_at,deleted_at,attempts,last_error FROM retired_billing_keys WHERE user_id=$1 ORDER BY retired_at DESC LIMIT 20",
    )
    .bind(user_id)
    .fetch_all(&s.db)
    .await?;
    Ok(Json(AccountDetail {
        payments,
        retired_keys,
    }))
}

/// Charge a declined renewal, or a first charge that failed, again now, on
/// the card on file. It counts as one of the retries.
/// An operator has dealt with a mismatch. It is kept, and not raised again.
pub(crate) async fn resolve_mismatch(
    State(s): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    admin_user(&headers, &s.db).await?;
    sqlx::query("UPDATE toss_mismatches SET resolved_at=now() WHERE id=$1 AND resolved_at IS NULL")
        .bind(id)
        .execute(&s.db)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn retry(
    State(s): State<Shared>,
    headers: HeaderMap,
    Path(user_id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let (admin_id, _) = admin_user(&headers, &s.db).await?;
    if s.billing.is_none() {
        return Err(ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "Subscriptions are not available.".into(),
        ));
    }
    let mut tx = s.db.begin().await?;
    let queued = sqlx::query("UPDATE subscriptions SET next_attempt_at=now(),updated_at=now() WHERE user_id=$1 AND status IN ('past_due','incomplete') AND billing_key IS NOT NULL AND (lease_until IS NULL OR lease_until<now())")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    if queued.rows_affected() == 0 {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "Only a declined subscription with a card on file can be charged again.".into(),
        ));
    }
    billing::record(
        &mut tx,
        admin_id,
        "subscription_retry",
        "succeeded",
        serde_json::json!({ "customer": user_id }),
        None,
    )
    .await?;
    tx.commit().await?;
    if billing::charge_due(&s, Some(user_id)).await.is_err() {
        tracing::warn!(%user_id, "could not charge a subscription an operator retried");
    }
    Ok(StatusCode::NO_CONTENT)
}

fn plan_name(name: &str) -> Result<&str, ApiError> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 60 {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "A plan's name must be 1 to 60 characters.".into(),
        ));
    }
    Ok(name)
}

#[derive(Deserialize)]
pub(crate) struct NewPlan {
    name_en: String,
    name_ko: String,
    amount: i64,
    instance_limit: Option<i64>,
    user_limit: Option<i64>,
    media_gb: Option<i64>,
    database_gb: Option<i64>,
    monthly_activities: Option<i64>,
}

/// Offer a new plan. Its price is fixed from here on.
pub(crate) async fn create_plan(
    State(s): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<NewPlan>,
) -> Result<Json<Plan>, ApiError> {
    let (admin_id, _) = admin_user(&headers, &s.db).await?;
    let limits = [
        body.instance_limit,
        body.user_limit,
        body.media_gb,
        body.database_gb,
        body.monthly_activities,
    ];
    if body.amount < 100 || limits.iter().flatten().any(|limit| *limit < 0) {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "A plan costs at least 100 won and cannot have a negative limit.".into(),
        ));
    }
    let mut tx = s.db.begin().await?;
    let plan = sqlx::query_as::<_, Plan>(&format!(
        "INSERT INTO plans(name_en,name_ko,amount,instance_limit,user_limit,media_gb,database_gb,monthly_activities) VALUES($1,$2,$3,$4,$5,$6,$7,$8) RETURNING {PLAN_COLUMNS}"
    ))
    .bind(plan_name(&body.name_en)?)
    .bind(plan_name(&body.name_ko)?)
    .bind(body.amount)
    .bind(body.instance_limit)
    .bind(body.user_limit)
    .bind(body.media_gb)
    .bind(body.database_gb)
    .bind(body.monthly_activities)
    .fetch_one(&mut *tx)
    .await?;
    billing::record(
        &mut tx,
        admin_id,
        "subscription_plan_create",
        "succeeded",
        serde_json::json!({ "plan_id": plan.id, "amount": plan.amount }),
        None,
    )
    .await?;
    tx.commit().await?;
    Ok(Json(plan))
}

/// A field that may be left out (`None`), sent as null to clear it
/// (`Some(None)`), or sent with a value.
fn present<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Option<i64>>, D::Error> {
    Option::<i64>::deserialize(deserializer).map(Some)
}

#[derive(Deserialize)]
pub(crate) struct PlanUpdate {
    name_en: String,
    name_ko: String,
    instance_limit: Option<i64>,
    #[serde(default, deserialize_with = "present")]
    user_limit: Option<Option<i64>>,
    #[serde(default, deserialize_with = "present")]
    media_gb: Option<Option<i64>>,
    #[serde(default, deserialize_with = "present")]
    database_gb: Option<Option<i64>>,
    #[serde(default, deserialize_with = "present")]
    monthly_activities: Option<Option<i64>>,
    active: bool,
}

/// Rename a plan, change its limits, or retire or bring it back. A displayed
/// limit left out of the request stays as it is, and one sent as null is
/// cleared, shown as no limit. Its price never changes. Retiring leaves
/// those on it where they are; it only stops anyone new choosing it.
pub(crate) async fn update_plan(
    State(s): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(body): Json<PlanUpdate>,
) -> Result<Json<Plan>, ApiError> {
    let (admin_id, _) = admin_user(&headers, &s.db).await?;
    let shown = [
        body.user_limit,
        body.media_gb,
        body.database_gb,
        body.monthly_activities,
    ];
    if body.instance_limit.is_some_and(|limit| limit < 0)
        || shown.iter().flatten().flatten().any(|limit| *limit < 0)
    {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "A plan cannot have a negative limit.".into(),
        ));
    }
    let given = |field: Option<Option<i64>>| (field.is_some(), field.flatten());
    let mut tx = s.db.begin().await?;
    let plan = sqlx::query_as::<_, Plan>(&format!(
        "UPDATE plans SET name_en=$2,name_ko=$3,instance_limit=$4,active=$5,user_limit=CASE WHEN $6 THEN $7 ELSE user_limit END,media_gb=CASE WHEN $8 THEN $9 ELSE media_gb END,database_gb=CASE WHEN $10 THEN $11 ELSE database_gb END,monthly_activities=CASE WHEN $12 THEN $13 ELSE monthly_activities END WHERE id=$1 RETURNING {PLAN_COLUMNS}"
    ))
    .bind(id)
    .bind(plan_name(&body.name_en)?)
    .bind(plan_name(&body.name_ko)?)
    .bind(body.instance_limit)
    .bind(body.active)
    .bind(given(body.user_limit).0)
    .bind(given(body.user_limit).1)
    .bind(given(body.media_gb).0)
    .bind(given(body.media_gb).1)
    .bind(given(body.database_gb).0)
    .bind(given(body.database_gb).1)
    .bind(given(body.monthly_activities).0)
    .bind(given(body.monthly_activities).1)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "Plan not found.".into()))?;
    billing::record(
        &mut tx,
        admin_id,
        "subscription_plan_update",
        "succeeded",
        serde_json::json!({ "plan_id": id, "instance_limit": body.instance_limit, "active": body.active, "user_limit": body.user_limit, "media_gb": body.media_gb, "database_gb": body.database_gb, "monthly_activities": body.monthly_activities }),
        None,
    )
    .await?;
    tx.commit().await?;
    Ok(Json(plan))
}

#[derive(Deserialize)]
pub(crate) struct Exemption {
    exempt: bool,
}

/// Let an account run instances without a subscription, or stop letting it.
/// Lifting an exemption takes nothing offline: the account just cannot create
/// or restore instances until it subscribes.
pub(crate) async fn exempt(
    State(s): State<Shared>,
    headers: HeaderMap,
    Path(user_id): Path<Uuid>,
    Json(body): Json<Exemption>,
) -> Result<StatusCode, ApiError> {
    let (admin_id, _) = admin_user(&headers, &s.db).await?;
    let mut tx = s.db.begin().await?;
    let changed = sqlx::query(
        "UPDATE users SET billing_exempt=$2 WHERE id=$1 AND email IS NOT NULL AND billing_exempt<>$2",
    )
    .bind(user_id)
    .bind(body.exempt)
    .execute(&mut *tx)
    .await?;
    if changed.rows_affected() > 0 {
        billing::record(
            &mut tx,
            admin_id,
            "billing_exemption",
            "succeeded",
            serde_json::json!({ "customer": user_id, "exempt": body.exempt }),
            None,
        )
        .await?;
    }
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Refund any paid payment, outside the customer's seven days as well.
pub(crate) async fn refund(
    State(s): State<Shared>,
    headers: HeaderMap,
    Path(order_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let (admin_id, _) = admin_user(&headers, &s.db).await?;
    billing_sync::start_refund(&s, &order_id, None, admin_id, false).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Delete a plan nothing refers to: no subscription, running or ended, and no
/// payment. One that was ever used is retired instead, so its history keeps
/// its name.
pub(crate) async fn delete_plan(
    State(s): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let (admin_id, _) = admin_user(&headers, &s.db).await?;
    let mut tx = s.db.begin().await?;
    let found = sqlx::query_scalar::<_, String>("SELECT name_en FROM plans WHERE id=$1 FOR UPDATE")
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "Plan not found.".into()))?;
    let deleted = sqlx::query("DELETE FROM plans WHERE id=$1 AND NOT EXISTS(SELECT 1 FROM subscriptions s WHERE $1 IN (s.plan_id,s.pending_plan_id)) AND NOT EXISTS(SELECT 1 FROM subscription_payments p WHERE $1 IN (p.plan_id,p.from_plan_id))")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    if deleted.rows_affected() == 0 {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "A subscription or payment refers to this plan. Retire it instead.".into(),
        ));
    }
    billing::record(
        &mut tx,
        admin_id,
        "subscription_plan_delete",
        "succeeded",
        serde_json::json!({ "plan_id": id, "name_en": found }),
        None,
    )
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub(crate) struct PolarLink {
    product_id: String,
}

/// Sell a plan to international cards as a Polar product, or move it to
/// another, as from Polar's sandbox to its live products. Its price is read
/// from Polar, and follows the product's from then on. A plan with a running
/// Polar subscription keeps its product: those subscriptions were sold as it.
pub(crate) async fn link_polar(
    State(s): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(body): Json<PolarLink>,
) -> Result<Json<Plan>, ApiError> {
    let (admin_id, _) = admin_user(&headers, &s.db).await?;
    let polar = crate::billing_polar::config(&s)?;
    let product_id = body.product_id.trim();
    if product_id.is_empty() || product_id.len() > 100 {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "Enter the Polar product's id.".into(),
        ));
    }
    let product = match polar.product(product_id).await {
        Ok(product) => product,
        Err(crate::billing_polar::PolarError::NotFound) => {
            return Err(ApiError(
                StatusCode::BAD_REQUEST,
                "Polar has no product with this id.".into(),
            ));
        }
        Err(_) => {
            return Err(ApiError(
                StatusCode::BAD_GATEWAY,
                "Polar could not be reached. Try again shortly.".into(),
            ));
        }
    };
    let amount = product
        .monthly_price(&polar.currency)
        .map_err(|reason| ApiError(StatusCode::BAD_REQUEST, reason))?;
    let mut tx = s.db.begin().await?;
    let linked = sqlx::query_scalar::<_, Option<String>>(
        "SELECT polar_product_id FROM plans WHERE id=$1 FOR UPDATE",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "Plan not found.".into()))?;
    if linked.as_deref() == Some(product_id) {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "This plan is already sold as this product.".into(),
        ));
    }
    if linked.is_some() {
        let running = sqlx::query_scalar::<_, bool>(
            "SELECT true FROM subscriptions WHERE provider='polar' AND status IN ('active','past_due') AND $1 IN (plan_id,pending_plan_id) LIMIT 1",
        )
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?
        .is_some();
        if running {
            return Err(ApiError(
                StatusCode::CONFLICT,
                "A running Polar subscription is on this plan, so its product cannot change."
                    .into(),
            ));
        }
    }
    let taken = sqlx::query_scalar::<_, bool>("SELECT true FROM plans WHERE polar_product_id=$1")
        .bind(product_id)
        .fetch_optional(&mut *tx)
        .await?
        .is_some();
    if taken {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "Another plan is already sold as this product.".into(),
        ));
    }
    let plan = sqlx::query_as::<_, Plan>(&format!(
        "UPDATE plans SET polar_product_id=$2,polar_amount=$3,polar_currency=$4 WHERE id=$1 RETURNING {PLAN_COLUMNS}"
    ))
    .bind(id)
    .bind(product_id)
    .bind(amount)
    .bind(&polar.currency)
    .fetch_one(&mut *tx)
    .await?;
    billing::record(
        &mut tx,
        admin_id,
        "subscription_plan_polar",
        "succeeded",
        serde_json::json!({ "plan_id": id, "polar_product_id": product_id, "previous_product_id": linked, "amount": amount, "currency": polar.currency }),
        None,
    )
    .await?;
    tx.commit().await?;
    Ok(Json(plan))
}

/// Read a plan's Polar price again now, rather than wait for Polar's
/// `product.updated` webhook.
pub(crate) async fn refresh_polar(
    State(s): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<Plan>, ApiError> {
    admin_user(&headers, &s.db).await?;
    let polar = crate::billing_polar::config(&s)?;
    let product_id =
        sqlx::query_scalar::<_, Option<String>>("SELECT polar_product_id FROM plans WHERE id=$1")
            .bind(id)
            .fetch_optional(&s.db)
            .await?
            .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "Plan not found.".into()))?
            .ok_or_else(|| {
                ApiError(
                    StatusCode::CONFLICT,
                    "This plan is not sold through Polar.".into(),
                )
            })?;
    match crate::billing_polar::refresh_product(&s, polar, &product_id).await {
        Ok(Some(Err(reason))) => return Err(ApiError(StatusCode::CONFLICT, reason)),
        Ok(_) => {}
        Err(crate::billing_polar::PolarError::NotFound) => {
            return Err(ApiError(
                StatusCode::CONFLICT,
                "Polar has no product with this plan's id. Link it to another.".into(),
            ));
        }
        Err(_) => {
            return Err(ApiError(
                StatusCode::BAD_GATEWAY,
                "Polar could not be reached. Try again shortly.".into(),
            ));
        }
    }
    let plan = sqlx::query_as::<_, Plan>(&format!("SELECT {PLAN_COLUMNS} FROM plans WHERE id=$1"))
        .bind(id)
        .fetch_one(&s.db)
        .await?;
    Ok(Json(plan))
}
