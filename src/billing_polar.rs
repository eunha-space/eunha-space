//! Customer subscriptions billed through Polar, for cards TossPayments cannot
//! bill: those issued outside Korea.
//!
//! Polar is the merchant of record. It sells the subscription in its own name,
//! keeps the renewal schedule, retries a failed charge and ends the
//! subscription when the retries run out, and collects and remits the tax. So
//! unlike a TossPayments subscription, nothing here charges a Polar one: the
//! row in `subscriptions` mirrors what Polar holds, kept in step from Polar's
//! webhooks and a periodic look, and what it says decides whether the
//! customer's instances run, exactly as for TossPayments.
//!
//! A webhook is a notification, never a fact: its signature is checked, and the
//! subscription or order it names is then read from Polar's API, so events that
//! arrive late or out of order settle on Polar's current record.
//!
//! The access token and the webhook secret never leave this process.
//! *docs/billing.md* describes what a customer sees.

use std::time::Duration;

use anyhow::Result;
use axum::{
    Json,
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    ApiError, Shared,
    billing::{self, BillingView, Notice, notify, record, view},
    customer, customer_instance_count,
};

/// How long a request to Polar may take. Polar answers in well under a second;
/// a webhook is answered within its own ten seconds even after one lookup.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(8);

/// How far a webhook's timestamp may be from now, either way, before it is
/// refused as a replay. Standard Webhooks' own default.
const WEBHOOK_TOLERANCE_SECONDS: i64 = 5 * 60;

/// Polar's events carry a whole subscription or order: a few kilobytes.
const WEBHOOK_BODY_LIMIT: usize = 256 * 1024;

/// How often a running subscription is read from Polar without being told to,
/// in case a webhook never arrived.
const SYNC_EVERY: &str = "1 hour";

/// How long after a refund was asked of Polar, with no refund on record for the
/// order, it may be asked again.
const REFUND_RESEND_AFTER: &str = "1 hour";

/// The API version every request is pinned to. Without one Polar answers in
/// whatever is current, which changes every quarter; a version Polar has
/// removed answers 404, which is told apart from a missing object below.
const API_VERSION: &str = "2026-10";

pub(crate) struct Config {
    access_token: String,
    webhook_secret: String,
    /// `https://api.polar.sh` unless `POLAR_API_BASE` points at Polar's sandbox
    /// or a stand-in, which is how this is tested.
    pub(crate) api_base: String,
    /// Where Polar's checkout and portal send the customer back.
    public_url: String,
    /// The one currency Polar plans are sold in, upper case. A subscription can
    /// only change to a product priced in its own currency, so every plan has
    /// to share it.
    pub(crate) currency: String,
    client: reqwest::Client,
}

impl Config {
    pub(crate) fn from_env() -> Result<Option<Self>> {
        let (Ok(access_token), Ok(webhook_secret)) = (
            std::env::var("POLAR_ACCESS_TOKEN"),
            std::env::var("POLAR_WEBHOOK_SECRET"),
        ) else {
            return Ok(None);
        };
        anyhow::ensure!(
            !access_token.trim().is_empty() && !webhook_secret.trim().is_empty(),
            "Polar's access token and webhook secret must not be empty"
        );
        Ok(Some(Self::new(
            access_token.trim().into(),
            webhook_secret.trim().into(),
            std::env::var("POLAR_API_BASE").unwrap_or_else(|_| "https://api.polar.sh".into()),
            std::env::var("EUNHA_SPACE_PUBLIC_URL")
                .unwrap_or_else(|_| "https://eunha.space".into()),
            std::env::var("POLAR_CURRENCY").unwrap_or_else(|_| "USD".into()),
        )?))
    }

    pub(crate) fn new(
        access_token: String,
        webhook_secret: String,
        api_base: String,
        public_url: String,
        currency: String,
    ) -> Result<Self> {
        let currency = currency.trim().to_ascii_uppercase();
        anyhow::ensure!(
            currency.len() == 3 && currency.chars().all(|c| c.is_ascii_uppercase()),
            "POLAR_CURRENCY must be a three-letter currency code"
        );
        Ok(Self {
            access_token,
            webhook_secret,
            api_base: api_base.trim_end_matches('/').into(),
            public_url: public_url.trim_end_matches('/').into(),
            currency,
            client: reqwest::Client::builder()
                .timeout(REQUEST_TIMEOUT)
                .build()?,
        })
    }

    async fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<Vec<u8>, PolarError> {
        let mut request = self
            .client
            .request(method, format!("{}{path}", self.api_base))
            .bearer_auth(&self.access_token)
            .header("Polar-Version", API_VERSION)
            .header("Accept", "application/json");
        if let Some(body) = body {
            request = request.json(&body);
        }
        // The error would name the request; say only that it failed.
        let response = request.send().await.map_err(|_| PolarError::Unreachable)?;
        let status = response.status();
        // Polar names the version it answered in. A 404 without one is not
        // about the object asked for: the version, or the address, is wrong.
        let versioned = response.headers().contains_key("polar-version");
        let body = response
            .bytes()
            .await
            .map_err(|_| PolarError::Unreachable)?;
        if status.is_success() {
            return Ok(body.to_vec());
        }
        if status == reqwest::StatusCode::NOT_FOUND {
            if !versioned {
                tracing::error!(
                    version = API_VERSION,
                    "Polar answered 404 without naming an API version; the pinned version may have been removed"
                );
                return Err(PolarError::Unreachable);
            }
            return Err(PolarError::NotFound);
        }
        if status.is_server_error() || status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(PolarError::Unreachable);
        }
        if status == reqwest::StatusCode::UNAUTHORIZED {
            tracing::error!("Polar refused this control plane's access token");
            return Err(PolarError::Unreachable);
        }
        Err(PolarError::Refused(polar_detail(&body)))
    }

    async fn get<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T, PolarError> {
        let body = self.request(reqwest::Method::GET, path, None).await?;
        serde_json::from_slice(&body).map_err(|_| PolarError::Unreachable)
    }

    async fn send<T: serde::de::DeserializeOwned>(
        &self,
        method: reqwest::Method,
        path: &str,
        body: serde_json::Value,
    ) -> Result<T, PolarError> {
        let body = self.request(method, path, Some(body)).await?;
        serde_json::from_slice(&body).map_err(|_| PolarError::Unreachable)
    }

    pub(crate) async fn subscription(&self, id: &str) -> Result<Subscription, PolarError> {
        self.get(&format!("/v1/subscriptions/{}", billing::urlencode(id)))
            .await
    }

    pub(crate) async fn order(&self, id: &str) -> Result<Order, PolarError> {
        self.get(&format!("/v1/orders/{}", billing::urlencode(id)))
            .await
    }

    pub(crate) async fn product(&self, id: &str) -> Result<Product, PolarError> {
        self.get(&format!("/v1/products/{}", billing::urlencode(id)))
            .await
    }

    async fn update_subscription(
        &self,
        id: &str,
        body: serde_json::Value,
    ) -> Result<Subscription, PolarError> {
        self.send(
            reqwest::Method::PATCH,
            &format!("/v1/subscriptions/{}", billing::urlencode(id)),
            body,
        )
        .await
    }

    /// End a subscription at Polar now: no more charges, and no refund.
    async fn revoke(&self, id: &str) -> Result<Subscription, PolarError> {
        let body = self
            .request(
                reqwest::Method::DELETE,
                &format!("/v1/subscriptions/{}", billing::urlencode(id)),
                None,
            )
            .await?;
        serde_json::from_slice(&body).map_err(|_| PolarError::Unreachable)
    }

    async fn customer_subscriptions(&self, user_id: Uuid) -> Result<Vec<Subscription>, PolarError> {
        let page: Page<Subscription> = self
            .get(&format!(
                "/v1/subscriptions/?external_customer_id={user_id}&limit=20&sorting=-started_at"
            ))
            .await?;
        Ok(page.items)
    }

    async fn customer_orders(&self, user_id: Uuid) -> Result<Vec<Order>, PolarError> {
        let page: Page<Order> = self
            .get(&format!(
                "/v1/orders/?external_customer_id={user_id}&limit=20&sorting=-created_at"
            ))
            .await?;
        Ok(page.items)
    }

    async fn refunds(&self, order_id: &str) -> Result<Vec<Refund>, PolarError> {
        let page: Page<Refund> = self
            .get(&format!(
                "/v1/refunds/?order_id={}&limit=20",
                billing::urlencode(order_id)
            ))
            .await?;
        Ok(page.items)
    }
}

/// Why a request to Polar did not succeed.
#[derive(Debug, PartialEq)]
pub(crate) enum PolarError {
    /// No answer that says either way: a network failure, a timeout, Polar's own
    /// error, or this control plane's token. Worth asking again later.
    Unreachable,
    NotFound,
    /// Polar refused the request on its merits, with its reason.
    Refused(String),
}

impl PolarError {
    fn into_api(self, what: &str) -> ApiError {
        match self {
            PolarError::Unreachable => ApiError(
                StatusCode::BAD_GATEWAY,
                "Polar could not be reached. Try again shortly.".into(),
            ),
            PolarError::NotFound => ApiError(
                StatusCode::CONFLICT,
                format!("{what} is not known to Polar."),
            ),
            PolarError::Refused(reason) => {
                tracing::warn!(%reason, "Polar refused a request: {what}");
                ApiError(
                    StatusCode::CONFLICT,
                    format!("Polar refused this: {reason}"),
                )
            }
        }
    }
}

/// Polar's reason for refusing a request, short and on one line. It is written
/// for developers, but it is the most useful thing to show.
fn polar_detail(body: &[u8]) -> String {
    let detail = serde_json::from_slice::<serde_json::Value>(body)
        .ok()
        .and_then(|value| match value.get("detail") {
            Some(serde_json::Value::String(detail)) => Some(detail.clone()),
            Some(serde_json::Value::Array(errors)) => errors
                .first()
                .and_then(|error| error.get("msg"))
                .and_then(|msg| msg.as_str())
                .map(str::to_owned),
            _ => None,
        })
        .unwrap_or_default();
    let detail = billing::customer_message(&detail);
    if detail == "The payment was not approved." {
        "the request was refused.".into()
    } else {
        detail
    }
}

#[derive(Deserialize)]
struct Page<T> {
    items: Vec<T>,
}

#[derive(Debug, Deserialize, Clone)]
pub(crate) struct PolarCustomer {
    #[serde(default)]
    pub(crate) id: String,
    /// The user id a checkout was created for.
    pub(crate) external_id: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub(crate) struct PendingUpdate {
    pub(crate) product_id: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub(crate) struct Subscription {
    pub(crate) id: String,
    pub(crate) status: String,
    #[serde(default)]
    pub(crate) amount: i64,
    pub(crate) currency: String,
    #[serde(with = "time::serde::rfc3339")]
    pub(crate) current_period_start: OffsetDateTime,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub(crate) current_period_end: Option<OffsetDateTime>,
    #[serde(default)]
    pub(crate) cancel_at_period_end: bool,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub(crate) ended_at: Option<OffsetDateTime>,
    pub(crate) product_id: String,
    pub(crate) customer: PolarCustomer,
    #[serde(default)]
    pub(crate) pending_update: Option<PendingUpdate>,
}

/// Where a Polar subscription stands, as far as instances are concerned.
#[derive(Debug, PartialEq)]
pub(crate) enum Standing {
    /// Paid for, or being retried: `active` or `past_due` here.
    Running(&'static str),
    /// Over: no more charges. `lapsed` when it ended unpaid.
    Ended(&'static str),
    /// Not paid for yet: a checkout still in progress.
    NotStarted,
}

impl Subscription {
    /// `Ended("lapsed")` only for `unpaid`, which Polar gives up on. A
    /// subscription Polar revoked after its retries is plain `canceled` with
    /// `ended_at`, so whether it lapsed is decided by whether it was past due
    /// here when it ended: `past_due_at` stays set after a recovery.
    pub(crate) fn standing(&self) -> Standing {
        match self.status.as_str() {
            "unpaid" => Standing::Ended("lapsed"),
            _ if self.ended_at.is_some() => Standing::Ended("canceled"),
            "active" | "trialing" => Standing::Running("active"),
            "past_due" => Standing::Running("past_due"),
            "canceled" | "paused" | "incomplete_expired" => Standing::Ended("canceled"),
            _ => Standing::NotStarted,
        }
    }

    fn user_id(&self) -> Option<Uuid> {
        self.customer
            .external_id
            .as_deref()
            .and_then(|id| id.parse().ok())
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct Order {
    pub(crate) id: String,
    pub(crate) status: String,
    pub(crate) total_amount: i64,
    #[serde(default)]
    pub(crate) refunded_amount: i64,
    #[serde(default)]
    pub(crate) refunded_tax_amount: i64,
    /// What can still be refunded, tax excluded: Polar refunds the tax with it.
    #[serde(default)]
    pub(crate) refundable_amount: i64,
    pub(crate) currency: String,
    pub(crate) billing_reason: String,
    #[serde(with = "time::serde::rfc3339")]
    pub(crate) created_at: OffsetDateTime,
    pub(crate) customer: PolarCustomer,
    pub(crate) product_id: Option<String>,
}

impl Order {
    /// What has been given back of what the customer paid, tax included.
    pub(crate) fn refunded(&self) -> i64 {
        (self.refunded_amount + self.refunded_tax_amount).clamp(0, self.total_amount)
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct Refund {
    pub(crate) status: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Product {
    #[serde(default)]
    pub(crate) is_recurring: bool,
    #[serde(default)]
    pub(crate) is_archived: bool,
    pub(crate) recurring_interval: Option<String>,
    #[serde(default)]
    pub(crate) prices: Vec<ProductPrice>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ProductPrice {
    pub(crate) amount_type: String,
    #[serde(default)]
    pub(crate) is_archived: bool,
    pub(crate) price_currency: Option<String>,
    pub(crate) price_amount: Option<i64>,
}

impl Product {
    /// The monthly price this product charges in `currency`, if it is one a
    /// plan can be sold as: recurring every month at a fixed price.
    pub(crate) fn monthly_price(&self, currency: &str) -> Result<i64, String> {
        if self.is_archived {
            return Err("The Polar product is archived.".into());
        }
        if !self.is_recurring || self.recurring_interval.as_deref() != Some("month") {
            return Err("The Polar product must renew every month.".into());
        }
        self.prices
            .iter()
            .filter(|price| !price.is_archived && price.amount_type == "fixed")
            .find(|price| {
                price
                    .price_currency
                    .as_deref()
                    .is_some_and(|c| c.eq_ignore_ascii_case(currency))
            })
            .and_then(|price| price.price_amount)
            .filter(|amount| *amount > 0)
            .ok_or_else(|| format!("The Polar product needs a fixed price in {currency}."))
    }
}

pub(crate) fn config(s: &Shared) -> Result<&Config, ApiError> {
    s.polar.as_deref().ok_or_else(|| {
        ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "International cards are not available.".into(),
        )
    })
}

// --- Webhook signatures ----------------------------------------------------

fn hmac_sha256(key: &[u8], message: &[&[u8]]) -> [u8; 32] {
    let mut block = [0u8; 64];
    if key.len() > 64 {
        block[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let mut inner = Sha256::new();
    inner.update(block.map(|b| b ^ 0x36));
    for part in message {
        inner.update(part);
    }
    let mut outer = Sha256::new();
    outer.update(block.map(|b| b ^ 0x5c));
    outer.update(inner.finalize());
    outer.finalize().into()
}

fn same_bytes(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Whether a webhook carries a valid signature from Polar, by the Standard
/// Webhooks scheme. Polar's secrets made before 8 September 2026 sign with the
/// secret's own bytes rather than the key it encodes, so both are tried.
pub(crate) fn verify(
    secret: &str,
    id: &str,
    timestamp: &str,
    signatures: &str,
    body: &[u8],
    now: i64,
) -> bool {
    let Ok(sent) = timestamp.parse::<i64>() else {
        return false;
    };
    if (now - sent).abs() > WEBHOOK_TOLERANCE_SECONDS || id.is_empty() {
        return false;
    }
    let mut keys = vec![secret.as_bytes().to_vec()];
    if let Ok(key) = STANDARD.decode(secret.strip_prefix("whsec_").unwrap_or(secret)) {
        keys.insert(0, key);
    }
    let expected: Vec<[u8; 32]> = keys
        .iter()
        .map(|key| {
            hmac_sha256(
                key,
                &[id.as_bytes(), b".", timestamp.as_bytes(), b".", body],
            )
        })
        .collect();
    signatures.split_whitespace().any(|signature| {
        let Some(encoded) = signature.strip_prefix("v1,") else {
            return false;
        };
        let Ok(given) = STANDARD.decode(encoded) else {
            return false;
        };
        expected.iter().any(|mac| same_bytes(mac, &given))
    })
}

/// A `webhook-signature` header for a delivery, as Polar signs one with a
/// Standard Webhooks secret.
#[cfg(test)]
pub(crate) fn sign(secret: &str, id: &str, timestamp: &str, body: &[u8]) -> String {
    let key = STANDARD
        .decode(secret.strip_prefix("whsec_").unwrap_or(secret))
        .unwrap();
    let mac = hmac_sha256(
        &key,
        &[id.as_bytes(), b".", timestamp.as_bytes(), b".", body],
    );
    format!("v1,{}", STANDARD.encode(mac))
}

fn header<'a>(headers: &'a HeaderMap, name: &str) -> &'a str {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
}

#[derive(Deserialize)]
struct WebhookEvent {
    #[serde(rename = "type")]
    kind: String,
    data: serde_json::Value,
}

/// Polar's webhook. A delivery is kept whatever it says; one whose signature
/// verifies is acted on by reading what it names from Polar.
pub(crate) async fn webhook(
    State(s): State<Shared>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    let Some(polar) = s.polar.as_deref() else {
        // Nothing here to act on it; Polar disables an endpoint that keeps
        // failing, so it is not told to send it again.
        return StatusCode::ACCEPTED;
    };
    let id = header(&headers, "webhook-id");
    let verified = body.len() <= WEBHOOK_BODY_LIMIT
        && verify(
            &polar.webhook_secret,
            id,
            header(&headers, "webhook-timestamp"),
            header(&headers, "webhook-signature"),
            &body,
            OffsetDateTime::now_utc().unix_timestamp(),
        );
    let event = verified
        .then(|| serde_json::from_slice::<WebhookEvent>(&body).ok())
        .flatten();
    let status = match &event {
        None if !verified => StatusCode::UNAUTHORIZED,
        None => StatusCode::ACCEPTED,
        Some(event) => match handle_event(&s, polar, event).await {
            Ok(()) => StatusCode::ACCEPTED,
            // Polar sends it again, with backoff.
            Err(_) => StatusCode::SERVICE_UNAVAILABLE,
        },
    };
    let kept = sqlx::query("INSERT INTO polar_webhook_deliveries(webhook_id,event_type,verified,payload,http_status) VALUES($1,$2,$3,$4,$5)")
        .bind((!id.is_empty()).then(|| id.chars().take(200).collect::<String>()))
        .bind(event.as_ref().map(|event| event.kind.chars().take(100).collect::<String>()))
        .bind(verified)
        .bind((body.len() <= WEBHOOK_BODY_LIMIT).then(|| String::from_utf8_lossy(&body).into_owned()))
        .bind(i32::from(status.as_u16()))
        .execute(&s.db)
        .await;
    if kept.is_err() {
        tracing::warn!("could not keep a Polar webhook");
    }
    status
}

async fn handle_event(s: &Shared, polar: &Config, event: &WebhookEvent) -> Result<()> {
    let id = |field: &str| {
        event
            .data
            .get(field)
            .and_then(|value| value.as_str())
            .map(str::to_owned)
    };
    if event.kind.starts_with("subscription.") {
        let Some(subscription_id) = id("id") else {
            return Ok(());
        };
        match polar.subscription(&subscription_id).await {
            Ok(subscription) => sync_subscription(s, &subscription).await,
            Err(PolarError::NotFound) => Ok(()),
            Err(_) => anyhow::bail!("could not read a subscription from Polar"),
        }
    } else if event.kind.starts_with("order.") {
        let Some(order_id) = id("id") else {
            return Ok(());
        };
        read_order(s, polar, &order_id).await
    } else if event.kind.starts_with("refund.") {
        let Some(order_id) = id("order_id") else {
            return Ok(());
        };
        read_order(s, polar, &order_id).await
    } else {
        Ok(())
    }
}

async fn read_order(s: &Shared, polar: &Config, order_id: &str) -> Result<()> {
    match polar.order(order_id).await {
        Ok(order) => sync_order(s, &order).await,
        Err(PolarError::NotFound) => Ok(()),
        Err(_) => anyhow::bail!("could not read an order from Polar"),
    }
}

// --- Keeping the mirror in step ---------------------------------------------

/// Bring a customer's subscription row in step with a Polar subscription.
///
/// A subscription that is running at Polar starts or updates the customer's
/// row, unless the row is running another subscription: that is never
/// replaced, and an operator is told. One that ended at Polar ends the row if
/// it is the same subscription. One that ended here but runs at Polar, as when
/// a refund ended it, is revoked at Polar rather than started again.
pub(crate) async fn sync_subscription(s: &Shared, subscription: &Subscription) -> Result<()> {
    let Some(user_id) = subscription.user_id() else {
        return Ok(());
    };
    let standing = subscription.standing();
    let mut tx = s.db.begin().await?;
    let known = sqlx::query_scalar::<_, bool>("SELECT true FROM users WHERE id=$1")
        .bind(user_id)
        .fetch_optional(&mut *tx)
        .await?
        .is_some();
    if !known {
        return Ok(());
    }
    if let Standing::Running(_) = standing {
        // A placeholder to lock, filled in below.
        sqlx::query("INSERT INTO subscriptions(user_id,customer_key,amount,status,provider,currency) VALUES($1,$2,1,'incomplete','polar',$3) ON CONFLICT (user_id) DO NOTHING")
            .bind(user_id)
            .bind(Uuid::new_v4())
            .bind(subscription.currency.to_ascii_uppercase())
            .execute(&mut *tx)
            .await?;
    }
    let Some((status, polar_id)) = sqlx::query_as::<_, (String, Option<String>)>(
        "SELECT status,polar_subscription_id FROM subscriptions WHERE user_id=$1 FOR UPDATE",
    )
    .bind(user_id)
    .fetch_optional(&mut *tx)
    .await?
    else {
        return Ok(());
    };
    let same = polar_id.as_deref() == Some(subscription.id.as_str());
    let running_here = matches!(status.as_str(), "active" | "past_due");
    let ended_here = matches!(status.as_str(), "canceled" | "lapsed");
    match standing {
        Standing::NotStarted => {}
        Standing::Ended(kind) => {
            if same {
                sqlx::query("UPDATE subscriptions SET polar_live=false,polar_synced_at=now() WHERE user_id=$1")
                    .bind(user_id)
                    .execute(&mut *tx)
                    .await?;
                if running_here {
                    let kind = if status == "past_due" { "lapsed" } else { kind };
                    let tenants = billing::end_subscription(&mut tx, user_id, kind).await?;
                    tx.commit().await?;
                    billing::wake(s, &tenants).await;
                    notify(
                        s,
                        user_id,
                        if kind == "lapsed" {
                            Notice::Lapsed
                        } else {
                            Notice::Canceled
                        },
                    )
                    .await;
                    return Ok(());
                }
            }
        }
        Standing::Running(_) if same && ended_here => {
            // Ended here first. Polar is told again until it agrees.
            sqlx::query(
                "UPDATE subscriptions SET polar_live=true,polar_synced_at=now() WHERE user_id=$1",
            )
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
            tx.commit().await?;
            if let Some(polar) = s.polar.as_deref() {
                revoke_at_polar(s, polar, user_id, &subscription.id).await;
            }
            return Ok(());
        }
        Standing::Running(_) if !same && running_here => {
            tracing::error!(%user_id, subscription = %subscription.id, "a customer has a second running subscription at Polar; it was left alone");
            record(
                &mut tx,
                user_id,
                "subscription_conflict",
                "failed",
                serde_json::json!({ "polar_subscription_id": subscription.id }),
                Some("This account already has a running subscription. Refund and revoke one of them in Polar."),
            )
            .await?;
        }
        Standing::Running(state) => {
            let started = !running_here;
            mirror(&mut tx, user_id, subscription, state).await?;
            if started {
                record(
                    &mut tx,
                    user_id,
                    "subscription_start",
                    "succeeded",
                    serde_json::json!({ "provider": "polar", "polar_subscription_id": subscription.id }),
                    None,
                )
                .await?;
                billing::restore_suspended(s, &mut tx, user_id).await?;
            }
        }
    }
    tx.commit().await?;
    Ok(())
}

/// The plan sold as a Polar product, and its Polar price.
async fn plan_for(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    product: Option<&str>,
) -> sqlx::Result<Option<(Uuid, i64)>> {
    let Some(product) = product else {
        return Ok(None);
    };
    sqlx::query_as::<_, (Uuid, i64)>("SELECT id,polar_amount FROM plans WHERE polar_product_id=$1")
        .bind(product)
        .fetch_optional(&mut **tx)
        .await
}

/// Write a running Polar subscription into the customer's row, taking it over
/// from whatever ended there before.
async fn mirror(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: Uuid,
    subscription: &Subscription,
    state: &str,
) -> Result<()> {
    let current = plan_for(tx, Some(&subscription.product_id)).await?;
    let pending = plan_for(
        tx,
        subscription
            .pending_update
            .as_ref()
            .and_then(|update| update.product_id.as_deref())
            .filter(|product| *product != subscription.product_id),
    )
    .await?;
    if current.is_none() {
        tracing::warn!(product = %subscription.product_id, "a Polar subscription is for a product no plan is linked to");
    }
    // A full discount leaves nothing charged; the amount is kept positive.
    let amount = if subscription.amount > 0 {
        subscription.amount
    } else {
        current.map(|(_, amount)| amount).unwrap_or(1)
    };
    // A card TossPayments held is not charged again once Polar bills.
    sqlx::query("INSERT INTO retired_billing_keys(user_id,billing_key,toss_mid) SELECT user_id,billing_key,toss_mid FROM subscriptions WHERE user_id=$1 AND billing_key IS NOT NULL")
        .bind(user_id)
        .execute(&mut **tx)
        .await?;
    sqlx::query("UPDATE subscriptions SET provider='polar',currency=$2,polar_subscription_id=$3,polar_customer_id=$4,status=$5,amount=$6,plan_id=COALESCE($7,plan_id),pending_plan_id=$8,anchor_at=$9,periods_paid=1,current_period_end=$10,cancel_at_period_end=$11,last_error=$12,billing_key=NULL,toss_mid=NULL,card_company=NULL,card_number=NULL,next_attempt_at=NULL,failed_attempts=0,lease_until=NULL,polar_live=true,polar_synced_at=now(),updated_at=now() WHERE user_id=$1")
        .bind(user_id)
        .bind(subscription.currency.to_ascii_uppercase())
        .bind(&subscription.id)
        .bind(&subscription.customer.id)
        .bind(state)
        .bind(amount)
        .bind(current.map(|(id, _)| id))
        .bind(pending.map(|(id, _)| id))
        .bind(subscription.current_period_start)
        .bind(subscription.current_period_end)
        .bind(subscription.cancel_at_period_end)
        .bind((state == "past_due").then_some("The last payment failed. Update your card in the billing portal."))
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// Record a paid Polar order as a payment, and what has been refunded of it.
/// Polar mails its own receipts, so none is written here.
pub(crate) async fn sync_order(s: &Shared, order: &Order) -> Result<()> {
    if !matches!(
        order.status.as_str(),
        "paid" | "refunded" | "partially_refunded"
    ) || order.total_amount <= 0
    {
        return Ok(());
    }
    let Some(user_id) = order
        .customer
        .external_id
        .as_deref()
        .and_then(|id| id.parse::<Uuid>().ok())
    else {
        return Ok(());
    };
    let kind = if order.billing_reason == "subscription_update" {
        "upgrade"
    } else {
        "renewal"
    };
    let mut tx = s.db.begin().await?;
    let inserted = sqlx::query("INSERT INTO subscription_payments(order_id,user_id,amount,period,period_start,period_end,status,kind,plan_id,provider,currency,requested_at,charge_attempted_at,settled_at) SELECT $1,u.id,$3,0,$4,CASE WHEN $5='upgrade' THEN COALESCE(s.current_period_end,$4+interval '1 month') ELSE $4+interval '1 month' END,'paid',$5,(SELECT id FROM plans WHERE polar_product_id=$6),'polar',$7,$4,$4,$4 FROM users u LEFT JOIN subscriptions s ON s.user_id=u.id WHERE u.id=$2 ON CONFLICT DO NOTHING")
        .bind(&order.id)
        .bind(user_id)
        .bind(order.total_amount)
        .bind(order.created_at)
        .bind(kind)
        .bind(order.product_id.as_deref())
        .bind(order.currency.to_ascii_uppercase())
        .execute(&mut *tx)
        .await?;
    if inserted.rows_affected() > 0 {
        record(
            &mut tx,
            user_id,
            "subscription_charge",
            "succeeded",
            serde_json::json!({ "provider": "polar", "order_id": order.id, "amount": order.total_amount, "currency": order.currency.to_ascii_uppercase(), "kind": kind }),
            None,
        )
        .await?;
    }
    tx.commit().await?;
    if order.refunded() > 0 {
        crate::billing_sync::apply_cancellation(
            s,
            &order.id,
            &crate::billing_sync::PaymentRecord::refunded_by(
                &order.id,
                order.total_amount,
                order.refunded(),
            ),
        )
        .await?;
        end_at_polar_if_ended(s, user_id).await;
    }
    Ok(())
}

/// Revoke a subscription at Polar, best effort: the periodic look tries again
/// while Polar still bills it.
async fn revoke_at_polar(s: &Shared, polar: &Config, user_id: Uuid, subscription_id: &str) {
    match polar.revoke(subscription_id).await {
        Ok(_) | Err(PolarError::NotFound) => {
            let _ = sqlx::query("UPDATE subscriptions SET polar_live=false,polar_synced_at=now() WHERE user_id=$1 AND polar_subscription_id=$2")
                .bind(user_id)
                .bind(subscription_id)
                .execute(&s.db)
                .await;
        }
        // Already over at Polar: it refuses to revoke what is not running.
        Err(PolarError::Refused(_)) => {
            if let Ok(subscription) = polar.subscription(subscription_id).await
                && !matches!(subscription.standing(), Standing::Running(_))
            {
                let _ = sqlx::query("UPDATE subscriptions SET polar_live=false,polar_synced_at=now() WHERE user_id=$1 AND polar_subscription_id=$2")
                    .bind(user_id)
                    .bind(subscription_id)
                    .execute(&s.db)
                    .await;
            }
        }
        Err(PolarError::Unreachable) => {
            tracing::warn!(%user_id, "could not revoke a subscription at Polar; it is tried again");
        }
    }
}

/// After a refund ended a Polar subscription here, stop Polar billing it.
async fn end_at_polar_if_ended(s: &Shared, user_id: Uuid) {
    let Some(polar) = s.polar.as_deref() else {
        return;
    };
    let ended = sqlx::query_scalar::<_, Option<String>>(
        "SELECT polar_subscription_id FROM subscriptions WHERE user_id=$1 AND provider='polar' AND status IN ('canceled','lapsed') AND polar_live",
    )
    .bind(user_id)
    .fetch_optional(&s.db)
    .await
    .ok()
    .flatten()
    .flatten();
    if let Some(subscription_id) = ended {
        revoke_at_polar(s, polar, user_id, &subscription_id).await;
    }
}

/// Read one Polar subscription that has not been read for `SYNC_EVERY`, or one
/// that ended here while Polar still bills it. Returns whether there was one.
pub(crate) async fn sync_due(s: &Shared) -> Result<bool> {
    let Some(polar) = s.polar.as_deref() else {
        return Ok(false);
    };
    let Some((user_id, subscription_id)) = sqlx::query_as::<_, (Uuid, String)>(&format!(
        "UPDATE subscriptions SET polar_synced_at=now() WHERE user_id=(SELECT user_id FROM subscriptions WHERE provider='polar' AND polar_subscription_id IS NOT NULL AND (status IN ('active','past_due') OR polar_live) AND (polar_synced_at IS NULL OR polar_synced_at<now()-interval '{SYNC_EVERY}') ORDER BY polar_synced_at NULLS FIRST LIMIT 1 FOR UPDATE SKIP LOCKED) RETURNING user_id,polar_subscription_id"
    ))
    .fetch_optional(&s.db)
    .await?
    else {
        return Ok(false);
    };
    match polar.subscription(&subscription_id).await {
        Ok(subscription) => sync_subscription(s, &subscription).await?,
        Err(PolarError::NotFound) => {
            tracing::error!(%user_id, subscription = %subscription_id, "a subscription mirrored from Polar is not known to Polar");
        }
        Err(_) => tracing::warn!(%user_id, "could not read a subscription from Polar"),
    }
    Ok(true)
}

/// Read everything Polar holds for one customer: after a checkout, before its
/// webhooks may have arrived.
async fn sync_customer(s: &Shared, polar: &Config, user_id: Uuid) -> Result<(), ApiError> {
    let mut subscriptions = polar
        .customer_subscriptions(user_id)
        .await
        .map_err(|error| error.into_api("This account"))?;
    // Ended ones first, so a running one is what is left in the row.
    subscriptions
        .sort_by_key(|subscription| matches!(subscription.standing(), Standing::Running(_)));
    for subscription in &subscriptions {
        sync_subscription(s, subscription).await?;
    }
    let orders = polar
        .customer_orders(user_id)
        .await
        .map_err(|error| error.into_api("This account"))?;
    for order in &orders {
        sync_order(s, order).await?;
    }
    Ok(())
}

// --- What the customer does ------------------------------------------------

#[derive(Deserialize, Default)]
pub(crate) struct CheckoutChoice {
    plan_id: Option<Uuid>,
}

#[derive(Serialize)]
pub(crate) struct CheckoutPage {
    url: String,
}

#[derive(Deserialize)]
struct CreatedCheckout {
    url: String,
}

/// Start paying through Polar: a checkout page for the chosen plan, which sends
/// the customer back to their account when it is done.
pub(crate) async fn checkout(
    State(s): State<Shared>,
    headers: HeaderMap,
    choice: Option<Json<CheckoutChoice>>,
) -> Result<Json<CheckoutPage>, ApiError> {
    let (user_id, email) = customer(&headers, &s.db).await?;
    let polar = config(&s)?;
    let busy = sqlx::query_scalar::<_, bool>(
        "SELECT status IN ('active','past_due') OR EXISTS(SELECT 1 FROM subscription_payments p WHERE p.user_id=s.user_id AND p.status='pending') FROM subscriptions s WHERE user_id=$1",
    )
    .bind(user_id)
    .fetch_optional(&s.db)
    .await?
    .unwrap_or(false);
    if busy {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "You already have a subscription. Change plans instead.".into(),
        ));
    }
    let chosen = choice.and_then(|Json(choice)| choice.plan_id);
    let (plan_id, product_id) = sqlx::query_as::<_, (Uuid, String)>(
        "SELECT id,polar_product_id FROM plans WHERE active AND polar_product_id IS NOT NULL AND polar_currency=$2 AND ($1::uuid IS NULL OR id=$1) ORDER BY polar_amount,created_at LIMIT 1",
    )
    .bind(chosen)
    .bind(&polar.currency)
    .fetch_optional(&s.db)
    .await?
    .ok_or_else(|| {
        ApiError(
            StatusCode::CONFLICT,
            "This plan is not available with an international card.".into(),
        )
    })?;
    let locale = sqlx::query_scalar::<_, Option<String>>("SELECT locale FROM users WHERE id=$1")
        .bind(user_id)
        .fetch_one(&s.db)
        .await?;
    let mut body = serde_json::json!({
        "products": [product_id],
        "external_customer_id": user_id.to_string(),
        "customer_email": email,
        "currency": polar.currency.to_ascii_lowercase(),
        "success_url": format!("{}/account/billing?polar_checkout={{CHECKOUT_ID}}", polar.public_url),
        "return_url": format!("{}/account", polar.public_url),
        "metadata": { "plan_id": plan_id.to_string() },
    });
    if let Some(locale) = locale {
        body["locale"] = locale.into();
    }
    // Polar fills in the billing country from it, which its tax starts from.
    if let Some(address) = headers
        .get("cf-connecting-ip")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<std::net::IpAddr>().ok())
    {
        body["customer_ip_address"] = address.to_string().into();
    }
    let created: CreatedCheckout = polar
        .send(reqwest::Method::POST, "/v1/checkouts/", body)
        .await
        .map_err(|error| error.into_api("This plan"))?;
    Ok(Json(CheckoutPage { url: created.url }))
}

/// Read the customer's Polar subscription and payments now, rather than wait
/// for Polar's webhooks: what the account page does when a checkout returns.
pub(crate) async fn refresh(
    State(s): State<Shared>,
    headers: HeaderMap,
) -> Result<Json<BillingView>, ApiError> {
    let (user_id, _) = customer(&headers, &s.db).await?;
    let polar = config(&s)?;
    sync_customer(&s, polar, user_id).await?;
    Ok(Json(view(&s, user_id).await?))
}

#[derive(Serialize)]
pub(crate) struct PortalPage {
    url: String,
}

#[derive(Deserialize)]
struct CustomerSession {
    customer_portal_url: String,
}

/// Polar's customer portal, where a customer paying through Polar changes their
/// card and finds their invoices.
pub(crate) async fn portal(
    State(s): State<Shared>,
    headers: HeaderMap,
) -> Result<Json<PortalPage>, ApiError> {
    let (user_id, _) = customer(&headers, &s.db).await?;
    let polar = config(&s)?;
    let known = sqlx::query_scalar::<_, bool>(
        "SELECT true FROM subscriptions WHERE user_id=$1 AND provider='polar' AND polar_customer_id IS NOT NULL",
    )
    .bind(user_id)
    .fetch_optional(&s.db)
    .await?
    .is_some();
    if !known {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "This account does not pay through Polar.".into(),
        ));
    }
    let session: CustomerSession = polar
        .send(
            reqwest::Method::POST,
            "/v1/customer-sessions/",
            serde_json::json!({
                "external_customer_id": user_id.to_string(),
                "return_url": format!("{}/account", polar.public_url),
            }),
        )
        .await
        .map_err(|error| error.into_api("This account"))?;
    Ok(Json(PortalPage {
        url: session.customer_portal_url,
    }))
}

/// The customer's Polar subscription id, if Polar bills them.
pub(crate) async fn billed_by_polar(
    s: &Shared,
    user_id: Uuid,
) -> Result<Option<String>, sqlx::Error> {
    Ok(sqlx::query_scalar::<_, Option<String>>(
        "SELECT polar_subscription_id FROM subscriptions WHERE user_id=$1 AND provider='polar'",
    )
    .bind(user_id)
    .fetch_optional(&s.db)
    .await?
    .flatten())
}

/// Stop renewing at the end of the period, or end a subscription whose last
/// charge failed now, as cancelling a TossPayments one does.
pub(crate) async fn cancel(
    s: &Shared,
    user_id: Uuid,
    subscription_id: &str,
) -> Result<(), ApiError> {
    let polar = config(s)?;
    let status =
        sqlx::query_scalar::<_, String>("SELECT status FROM subscriptions WHERE user_id=$1")
            .bind(user_id)
            .fetch_one(&s.db)
            .await?;
    if !matches!(status.as_str(), "active" | "past_due") {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "There is no subscription to cancel.".into(),
        ));
    }
    let subscription = if status == "past_due" {
        polar.revoke(subscription_id).await
    } else {
        polar
            .update_subscription(
                subscription_id,
                serde_json::json!({ "cancel_at_period_end": true }),
            )
            .await
    }
    .map_err(|error| error.into_api("This subscription"))?;
    let mut tx = s.db.begin().await?;
    record(
        &mut tx,
        user_id,
        "subscription_cancel",
        "succeeded",
        serde_json::json!({ "provider": "polar" }),
        None,
    )
    .await?;
    tx.commit().await?;
    sync_subscription(s, &subscription).await?;
    Ok(())
}

/// Keep renewing a subscription that was cancelled but has not ended yet.
pub(crate) async fn resume(
    s: &Shared,
    user_id: Uuid,
    subscription_id: &str,
) -> Result<(), ApiError> {
    let polar = config(s)?;
    let canceling = sqlx::query_scalar::<_, bool>(
        "SELECT status='active' AND cancel_at_period_end FROM subscriptions WHERE user_id=$1",
    )
    .bind(user_id)
    .fetch_one(&s.db)
    .await?;
    if !canceling {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "This subscription has ended. Subscribe again to continue.".into(),
        ));
    }
    let subscription = polar
        .update_subscription(
            subscription_id,
            serde_json::json!({ "cancel_at_period_end": false }),
        )
        .await
        .map_err(|error| error.into_api("This subscription"))?;
    let mut tx = s.db.begin().await?;
    record(
        &mut tx,
        user_id,
        "subscription_resume",
        "succeeded",
        serde_json::json!({ "provider": "polar" }),
        None,
    )
    .await?;
    tx.commit().await?;
    sync_subscription(s, &subscription).await?;
    Ok(())
}

/// Move a Polar subscription to another plan. A dearer one applies now, and
/// Polar charges the difference for the rest of the period; any other applies at
/// the next renewal, and choosing the current plan takes that back.
pub(crate) async fn change_plan(
    s: &Shared,
    user_id: Uuid,
    subscription_id: &str,
    plan_id: Uuid,
) -> Result<(), ApiError> {
    let polar = config(s)?;
    let (status, canceling, amount, currency, current_plan) =
        sqlx::query_as::<_, (String, bool, i64, String, Option<Uuid>)>(
            "SELECT status,cancel_at_period_end,amount,currency,plan_id FROM subscriptions WHERE user_id=$1",
        )
        .bind(user_id)
        .fetch_one(&s.db)
        .await?;
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
    let refunding = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM subscription_payments WHERE user_id=$1 AND refund_requested_at IS NOT NULL AND refunded_at IS NULL)",
    )
    .bind(user_id)
    .fetch_one(&s.db)
    .await?;
    if refunding {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "A payment is being confirmed. Try again shortly.".into(),
        ));
    }
    let (change, body) = if current_plan == Some(plan_id) {
        ("keep", serde_json::json!({ "pending_update": null }))
    } else {
        let (product_id, new_amount, limit) = sqlx::query_as::<_, (String, i64, Option<i64>)>(
            "SELECT polar_product_id,polar_amount,instance_limit FROM plans WHERE id=$1 AND active AND polar_product_id IS NOT NULL AND polar_currency=$2",
        )
        .bind(plan_id)
        .bind(&currency)
        .fetch_optional(&s.db)
        .await?
        .ok_or_else(|| ApiError(StatusCode::CONFLICT, "This plan is not available.".into()))?;
        if new_amount > amount {
            (
                "now",
                serde_json::json!({ "product_id": product_id, "proration_behavior": "invoice" }),
            )
        } else {
            let mut tx = s.db.begin().await?;
            let count = customer_instance_count(&mut tx, user_id).await?;
            tx.rollback().await?;
            if limit.is_some_and(|limit| count > limit) {
                return Err(ApiError(
                    StatusCode::CONFLICT,
                    "Delete instances until you are within this plan's limit first.".into(),
                ));
            }
            (
                "at_renewal",
                serde_json::json!({ "product_id": product_id, "proration_behavior": "next_period" }),
            )
        }
    };
    let subscription = polar
        .update_subscription(subscription_id, body)
        .await
        .map_err(|error| error.into_api("This plan"))?;
    let mut tx = s.db.begin().await?;
    record(
        &mut tx,
        user_id,
        "subscription_plan",
        "succeeded",
        serde_json::json!({ "provider": "polar", "change": change, "plan_id": plan_id }),
        None,
    )
    .await?;
    tx.commit().await?;
    sync_subscription(s, &subscription).await?;
    Ok(())
}

// --- Refunds ---------------------------------------------------------------

/// What a refund pass for a Polar payment came to.
pub(crate) enum RefundStep {
    /// Polar has given money back; it is recorded.
    Refunded,
    /// Polar will not give it back, for this reason.
    Refused(String),
    /// Nothing settled yet. The next pass looks again.
    Waiting,
}

/// Carry a requested refund of a Polar payment one step forward. A refund is
/// asked of Polar once, written down first; after that Polar's record of the
/// order and its refunds decides, and it is only asked again when Polar has no
/// refund for the order an hour later.
pub(crate) async fn refund_step(s: &Shared, order_id: &str) -> Result<RefundStep> {
    let Some(polar) = s.polar.as_deref() else {
        return Ok(RefundStep::Waiting);
    };
    let order = match polar.order(order_id).await {
        Ok(order) => order,
        Err(PolarError::NotFound) => {
            return Ok(RefundStep::Refused(
                "This payment cannot be refunded here. Please contact us.".into(),
            ));
        }
        Err(_) => return Ok(RefundStep::Waiting),
    };
    if order.refunded() > 0 {
        sync_order(s, &order).await?;
        return Ok(RefundStep::Refunded);
    }
    let (sent, resendable) = sqlx::query_as::<_, (bool, bool)>(&format!(
        "SELECT refund_sent_at IS NOT NULL,coalesce(refund_sent_at<now()-interval '{REFUND_RESEND_AFTER}',false) FROM subscription_payments WHERE order_id=$1"
    ))
    .bind(order_id)
    .fetch_one(&s.db)
    .await?;
    if sent {
        let refunds = match polar.refunds(order_id).await {
            Ok(refunds) => refunds,
            Err(_) => return Ok(RefundStep::Waiting),
        };
        if refunds
            .iter()
            .any(|refund| matches!(refund.status.as_str(), "pending" | "succeeded"))
        {
            return Ok(RefundStep::Waiting);
        }
        if !refunds.is_empty() {
            return Ok(RefundStep::Refused(
                "Polar could not make this refund. Please contact us.".into(),
            ));
        }
        if !resendable {
            return Ok(RefundStep::Waiting);
        }
    }
    if order.refundable_amount <= 0 {
        return Ok(RefundStep::Refused(
            "This payment can no longer be refunded.".into(),
        ));
    }
    sqlx::query("UPDATE subscription_payments SET refund_sent_at=now() WHERE order_id=$1")
        .bind(order_id)
        .execute(&s.db)
        .await?;
    let created: Result<serde_json::Value, PolarError> = polar
        .send(
            reqwest::Method::POST,
            "/v1/refunds/",
            serde_json::json!({
                "order_id": order_id,
                "reason": "customer_request",
                "amount": order.refundable_amount,
                // Ending the subscription is decided here, not by Polar's
                // benefits.
                "revoke_benefits": false,
            }),
        )
        .await;
    match created {
        Ok(_) => {
            if let Ok(order) = polar.order(order_id).await
                && order.refunded() > 0
            {
                sync_order(s, &order).await?;
                return Ok(RefundStep::Refunded);
            }
            Ok(RefundStep::Waiting)
        }
        Err(PolarError::Refused(reason)) => {
            // Never made, so it may be asked again once the reason is dealt with.
            sqlx::query("UPDATE subscription_payments SET refund_sent_at=NULL WHERE order_id=$1")
                .bind(order_id)
                .execute(&s.db)
                .await?;
            Ok(RefundStep::Refused(reason))
        }
        Err(_) => Ok(RefundStep::Waiting),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Standard Webhooks' own example.
    const SECRET: &str = "whsec_MfKQ9r8GKYqrTwjUPD8ILPZIo2LaLaSw";
    const ID: &str = "msg_p5jXN8AQM9LWM0D4loKWxJek";
    const TIMESTAMP: &str = "1614265330";
    const BODY: &[u8] = br#"{"test": 2432232314}"#;
    const SIGNATURE: &str = "v1,g0hM9SsE+OTPJTGt/tmIKtSyZlE3uFJELVlNIOLJ1OE=";

    #[test]
    fn a_standard_webhooks_signature_verifies() {
        assert!(verify(SECRET, ID, TIMESTAMP, SIGNATURE, BODY, 1614265330));
        // Among others, as a rotated secret sends them.
        assert!(verify(
            SECRET,
            ID,
            TIMESTAMP,
            &format!("v1,bm90IGl0 {SIGNATURE}"),
            BODY,
            1614265330
        ));
    }

    #[test]
    fn a_changed_body_or_id_or_an_old_timestamp_does_not() {
        assert!(!verify(
            SECRET,
            ID,
            TIMESTAMP,
            SIGNATURE,
            br#"{"test": 1}"#,
            1614265330
        ));
        assert!(!verify(
            SECRET,
            "msg_other",
            TIMESTAMP,
            SIGNATURE,
            BODY,
            1614265330
        ));
        assert!(!verify(
            SECRET,
            ID,
            TIMESTAMP,
            SIGNATURE,
            BODY,
            1614265330 + 301
        ));
        assert!(!verify(SECRET, ID, "soon", SIGNATURE, BODY, 1614265330));
        assert!(!verify(SECRET, ID, TIMESTAMP, "", BODY, 1614265330));
        assert!(!verify(
            "whsec_other",
            ID,
            TIMESTAMP,
            SIGNATURE,
            BODY,
            1614265330
        ));
    }

    #[test]
    fn an_older_polar_secret_signs_with_its_own_bytes() {
        let secret = "whsec_older-polar-secret";
        let mac = hmac_sha256(
            secret.as_bytes(),
            &[ID.as_bytes(), b".", TIMESTAMP.as_bytes(), b".", BODY],
        );
        let signature = format!("v1,{}", STANDARD.encode(mac));
        assert!(verify(secret, ID, TIMESTAMP, &signature, BODY, 1614265330));
    }

    #[test]
    fn hmac_matches_rfc_4231() {
        // Test case 2.
        let mac = hmac_sha256(b"Jefe", &[b"what do ya want ", b"for nothing?"]);
        assert_eq!(
            mac.iter().map(|b| format!("{b:02x}")).collect::<String>(),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
        // Test case 6: a key longer than a block is hashed first.
        let mac = hmac_sha256(
            &[0xaa; 131],
            &[b"Test Using Larger Than Block-Size Key - Hash Key First"],
        );
        assert_eq!(
            mac.iter().map(|b| format!("{b:02x}")).collect::<String>(),
            "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
        );
    }

    fn subscription(status: &str, ended: bool, past_due: bool) -> Subscription {
        serde_json::from_value(serde_json::json!({
            "id": "sub_1",
            "status": status,
            "amount": 699,
            "currency": "usd",
            "current_period_start": "2026-11-01T00:00:00.123456Z",
            "current_period_end": "2026-12-01T00:00:00Z",
            "cancel_at_period_end": false,
            "ended_at": ended.then_some("2026-11-20T00:00:00Z"),
            "past_due_at": past_due.then_some("2026-11-15T00:00:00Z"),
            "product_id": "prod_1",
            "customer": {"id": "cus_1", "external_id": Uuid::nil().to_string()},
            "pending_update": null,
        }))
        .unwrap()
    }

    #[test]
    fn polar_statuses_say_whether_instances_run() {
        assert_eq!(
            subscription("active", false, false).standing(),
            Standing::Running("active")
        );
        assert_eq!(
            subscription("trialing", false, false).standing(),
            Standing::Running("active")
        );
        assert_eq!(
            subscription("past_due", false, true).standing(),
            Standing::Running("past_due")
        );
        // Given up on after the retries.
        assert_eq!(
            subscription("unpaid", false, true).standing(),
            Standing::Ended("lapsed")
        );
        assert_eq!(
            subscription("canceled", true, false).standing(),
            Standing::Ended("canceled")
        );
        // Revoked: whether it lapsed is up to the row it ends.
        assert_eq!(
            subscription("canceled", true, true).standing(),
            Standing::Ended("canceled")
        );
        assert_eq!(
            subscription("incomplete", false, false).standing(),
            Standing::NotStarted
        );
        // Whatever the status says, an end is an end.
        assert_eq!(
            subscription("active", true, false).standing(),
            Standing::Ended("canceled")
        );
    }

    #[test]
    fn a_plan_needs_a_monthly_fixed_price_in_the_currency() {
        let product = |recurring: bool, interval: &str, prices: serde_json::Value| -> Product {
            serde_json::from_value(serde_json::json!({
                "is_recurring": recurring,
                "is_archived": false,
                "recurring_interval": interval,
                "prices": prices,
            }))
            .unwrap()
        };
        let fixed = serde_json::json!([
            {"amount_type": "fixed", "is_archived": true, "price_currency": "usd", "price_amount": 499},
            {"amount_type": "fixed", "is_archived": false, "price_currency": "eur", "price_amount": 649},
            {"amount_type": "fixed", "is_archived": false, "price_currency": "usd", "price_amount": 699},
        ]);
        assert_eq!(
            product(true, "month", fixed.clone()).monthly_price("USD"),
            Ok(699)
        );
        assert!(
            product(true, "year", fixed.clone())
                .monthly_price("USD")
                .is_err()
        );
        assert!(product(false, "month", fixed).monthly_price("USD").is_err());
        assert!(
            product(
                true,
                "month",
                serde_json::json!([{"amount_type": "custom", "is_archived": false}])
            )
            .monthly_price("USD")
            .is_err()
        );
    }

    #[test]
    fn polar_reasons_are_kept_short() {
        assert_eq!(
            polar_detail(br#"{"detail":"Order is already fully refunded."}"#),
            "Order is already fully refunded."
        );
        assert_eq!(
            polar_detail(
                br#"{"detail":[{"msg":"Input should be greater than 0","loc":["body","amount"]}]}"#
            ),
            "Input should be greater than 0"
        );
        assert_eq!(polar_detail(b"<html>"), "the request was refused.");
    }
}
