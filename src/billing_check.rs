//! Comparing the books with the money: once a day, every transaction
//! TossPayments recorded for the day before, in Korea, against
//! `subscription_payments`.
//!
//! Everything else asks TossPayments about orders we already know are open.
//! This finds what those paths cannot: an order closed here that was approved
//! after all, an order nobody recorded, a dashboard cancel whose webhook never
//! came. A missed cancel is recorded as usual. Anything else is written to
//! `toss_mismatches` for an operator, because only a person can tell whether a
//! charge should be honoured or given back. *docs/billing.md* has the rules.

use std::collections::{BTreeMap, HashSet};

use anyhow::{Result, bail};
use serde::Deserialize;

use crate::{
    Shared,
    billing::{self, urlencode},
    billing_sync::{Found, apply_cancellation, find_order},
};

/// A day is compared this long after it ended in Korea, so what was being
/// settled at midnight has finished.
const CHECK_FROM: &str = "04:00";
/// How far back a control plane that was down catches up.
const CATCH_UP: i32 = 7;
/// How long before a check that failed is tried again.
const RETRY: &str = "1 hour";
const PAGE: usize = 1000;
const MAX_PAGES: usize = 100;
const KEEP_WEBHOOKS: &str = "5 years";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Transaction {
    transaction_key: String,
    #[serde(default)]
    order_id: String,
    status: String,
    #[serde(default)]
    amount: i64,
}

/// Compare the next day that is due, if any. Returns whether one was attempted.
pub(crate) async fn check_due(s: &Shared) -> Result<bool> {
    let Some(day) = sqlx::query_scalar::<_, String>(&format!(
        "WITH today AS (SELECT (now() AT TIME ZONE 'Asia/Seoul') AS kst)
         SELECT to_char(d,'YYYY-MM-DD') FROM (
             SELECT GREATEST(
                 COALESCE((SELECT max(day)+1 FROM toss_transaction_checks WHERE finished_at IS NOT NULL),kst::date-1),
                 kst::date-{CATCH_UP}
             ) d,kst FROM today
         ) due WHERE d<kst::date AND kst::time>='{CHECK_FROM}'"
    ))
    .fetch_optional(&s.db)
    .await?
    else {
        return Ok(false);
    };
    let claimed = sqlx::query(&format!(
        "INSERT INTO toss_transaction_checks(day) VALUES($1::date) ON CONFLICT(day) DO UPDATE SET attempted_at=now() WHERE toss_transaction_checks.finished_at IS NULL AND toss_transaction_checks.attempted_at<now()-interval '{RETRY}'"
    ))
    .bind(&day)
    .execute(&s.db)
    .await?
    .rows_affected();
    if claimed == 0 {
        return Ok(false);
    }
    match check_day(s, &day).await {
        Ok(transactions) => {
            sqlx::query("UPDATE toss_transaction_checks SET finished_at=now(),transactions=$2,error=NULL WHERE day=$1::date")
                .bind(&day)
                .bind(transactions as i32)
                .execute(&s.db)
                .await?;
        }
        Err(error) => {
            tracing::error!(%day, %error, "could not compare a day with TossPayments' transactions");
            sqlx::query("UPDATE toss_transaction_checks SET error=$2 WHERE day=$1::date")
                .bind(&day)
                .bind(error.to_string())
                .execute(&s.db)
                .await?;
        }
    }
    sqlx::query(&format!(
        "DELETE FROM toss_webhook_deliveries WHERE received_at<now()-interval '{KEEP_WEBHOOKS}'"
    ))
    .execute(&s.db)
    .await?;
    Ok(true)
}

/// Compare one day, given as `YYYY-MM-DD` in Korea. Returns how many of its
/// transactions moved money.
pub(crate) async fn check_day(s: &Shared, day: &str) -> Result<usize> {
    let Some(config) = s.billing.as_deref() else {
        return Ok(0);
    };
    let transactions = transactions(config, day).await?;
    let count = transactions.len();
    let mut orders = BTreeMap::<String, Vec<Transaction>>::new();
    for transaction in transactions {
        orders
            .entry(transaction.order_id.clone())
            .or_default()
            .push(transaction);
    }

    for (order_id, moved) in &orders {
        let row = sqlx::query_as::<_, (String, i64)>(
            "SELECT status,amount FROM subscription_payments WHERE order_id=$1",
        )
        .bind(order_id)
        .fetch_optional(&s.db)
        .await?;
        let Some((status, amount)) = row else {
            let listed = moved
                .iter()
                .map(|t| format!("{} {}", t.status, t.amount))
                .collect::<Vec<_>>()
                .join(", ");
            flag(s, order_id, "unknown_order", &listed).await?;
            continue;
        };
        if let Some(approval) = moved
            .iter()
            .find(|t| t.status == "DONE" && t.amount != amount)
        {
            let detail = format!("TossPayments {}, recorded {amount}", approval.amount);
            flag(s, order_id, "amount_differs", &detail).await?;
        }
        match status.as_str() {
            // The renewer is settling it, and looks up whatever happened.
            "pending" => {}
            "paid" | "refunded" => {
                if moved.iter().any(|t| t.status != "DONE") {
                    match find_order(s, order_id).await {
                        Found::Payment(payment) => {
                            if payment.refunded() > 0 {
                                apply_cancellation(s, order_id, &payment).await?;
                            }
                        }
                        Found::NotFound => {
                            flag(s, order_id, "not_at_toss", "listed, but not found").await?;
                        }
                        Found::Unknown => bail!("could not look up a cancelled order"),
                    }
                }
            }
            // Closed here without being paid. Nothing is owed if it was
            // cancelled in full since.
            _ => match find_order(s, order_id).await {
                Found::Payment(payment) if payment.keeps_money() => {
                    let detail = format!("recorded {status}, TossPayments {}", payment.status());
                    flag(s, order_id, "approved_not_recorded", &detail).await?;
                }
                Found::Payment(_) | Found::NotFound => {}
                Found::Unknown => bail!("could not look up a closed order"),
            },
        }
    }

    // The other way round: what the books say was paid that day. An approval
    // can be settled here after midnight, so one not listed is looked up.
    let paid = sqlx::query_scalar::<_, String>(
        "SELECT order_id FROM subscription_payments
         WHERE status IN ('paid','refunded') AND payment_key IS NOT NULL
           AND (settled_at AT TIME ZONE 'Asia/Seoul')::date=$1::date
           AND ($2::text IS NULL OR toss_mid IS NULL OR toss_mid=$2)",
    )
    .bind(day)
    .bind(config.mid.as_deref())
    .fetch_all(&s.db)
    .await?;
    let listed: HashSet<&str> = orders.keys().map(String::as_str).collect();
    for order_id in paid {
        if listed.contains(order_id.as_str()) {
            continue;
        }
        match find_order(s, &order_id).await {
            Found::Payment(_) => {}
            Found::NotFound => {
                flag(
                    s,
                    &order_id,
                    "not_at_toss",
                    "recorded as paid, but not found",
                )
                .await?;
            }
            Found::Unknown => bail!("could not look up a paid order"),
        }
    }

    // A charge should settle within minutes; a day means something is stuck.
    let stuck = sqlx::query_scalar::<_, String>(
        "SELECT order_id FROM subscription_payments WHERE status='pending' AND charge_attempted_at<now()-interval '1 day'",
    )
    .fetch_all(&s.db)
    .await?;
    for order_id in stuck {
        flag(
            s,
            &order_id,
            "still_pending",
            "no outcome after a day of looking it up",
        )
        .await?;
    }
    sqlx::query("UPDATE toss_mismatches m SET resolved_at=now() FROM subscription_payments p WHERE m.kind='still_pending' AND m.resolved_at IS NULL AND p.order_id=m.order_id AND p.status<>'pending'")
        .execute(&s.db)
        .await?;
    Ok(count)
}

/// Every transaction of a day that moved money. `READY`, `IN_PROGRESS` and the
/// like did not.
async fn transactions(config: &billing::Config, day: &str) -> Result<Vec<Transaction>> {
    let mut all = Vec::new();
    let mut after: Option<String> = None;
    for _ in 0..MAX_PAGES {
        let mut url = format!(
            "{}/v1/transactions?startDate={day}T00:00:00&endDate={day}T23:59:59&limit={PAGE}",
            config.api_base
        );
        if let Some(after) = &after {
            url.push_str(&format!("&startingAfter={}", urlencode(after)));
        }
        let response = config
            .client
            .get(url)
            .header("Authorization", config.authorization())
            .send()
            .await
            // The error would name the request; say only that it failed.
            .map_err(|_| anyhow::anyhow!("TossPayments did not answer"))?;
        let status = response.status();
        if !status.is_success() {
            bail!("TossPayments answered {status}");
        }
        let page: Vec<Transaction> = response.json().await?;
        let full = page.len() == PAGE;
        after = page.last().map(|t| t.transaction_key.clone());
        all.extend(page.into_iter().filter(|t| {
            !t.order_id.is_empty()
                && matches!(t.status.as_str(), "DONE" | "CANCELED" | "PARTIAL_CANCELED")
        }));
        if !full {
            return Ok(all);
        }
    }
    bail!("more than {MAX_PAGES} pages of transactions")
}

/// Record a mismatch once, however many days it stays true.
async fn flag(s: &Shared, order_id: &str, kind: &str, detail: &str) -> Result<()> {
    let new = sqlx::query(
        "INSERT INTO toss_mismatches(order_id,kind,detail) VALUES($1,$2,$3) ON CONFLICT(order_id,kind) DO NOTHING",
    )
    .bind(order_id)
    .bind(kind)
    .bind(detail)
    .execute(&s.db)
    .await?
    .rows_affected();
    if new > 0 {
        tracing::error!(%order_id, %kind, %detail, "the books and TossPayments disagree");
    }
    Ok(())
}
