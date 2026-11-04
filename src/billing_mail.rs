//! Receipts and renewal reminders, sent through an outbox.
//!
//! An email is a row in `billing_emails`, written in the same transaction as
//! what it is about: a receipt exists exactly when its charge was recorded as
//! paid, and its dedupe key keeps a second one from being written. The renewer
//! sends what is due, with backoff, and gives up on an email it could not send
//! for three days rather than send a stale one.

use anyhow::Result;
use uuid::Uuid;

use crate::Shared;

/// How long before a renewal its reminder goes out.
pub(crate) const RENEWAL_NOTICE: &str = "7 days";

/// A reminder this close to the renewal is not worth sending.
const RENEWAL_NOTICE_TOO_LATE: &str = "1 day";

/// How long an email that could not be sent keeps being tried.
const GIVE_UP_AFTER: &str = "3 days";

/// `₩9,900`.
pub(crate) fn won(amount: i64) -> String {
    let digits = amount.unsigned_abs().to_string();
    let mut grouped = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    format!("{}₩{grouped}", if amount < 0 { "-" } else { "" })
}

/// "the 현대 card ending 123*", or "your card" when nothing is known of it.
fn card(company: Option<&str>, number: Option<&str>) -> String {
    let ending = number
        .map(|number| number.chars().rev().take(4).collect::<Vec<_>>())
        .map(|last| last.into_iter().rev().collect::<String>());
    match (company, ending) {
        (Some(company), Some(ending)) => format!("your {company} card ending {ending}"),
        (Some(company), None) => format!("your {company} card"),
        (None, Some(ending)) => format!("your card ending {ending}"),
        (None, None) => "your card".into(),
    }
}

async fn enqueue(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: Uuid,
    kind: &str,
    dedupe_key: &str,
    subject: &str,
    body: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO billing_emails(user_id,kind,dedupe_key,subject,body) VALUES($1,$2,$3,$4,$5) ON CONFLICT (dedupe_key) DO NOTHING")
        .bind(user_id)
        .bind(kind)
        .bind(dedupe_key)
        .bind(subject)
        .bind(body)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// The receipt for a charge just recorded as paid, in the same transaction.
pub(crate) async fn receipt(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    order_id: &str,
) -> Result<(), sqlx::Error> {
    let Some((user_id, amount, start, end, receipt_url, company, number)) =
        sqlx::query_as::<_, (Uuid, i64, String, String, Option<String>, Option<String>, Option<String>)>(
            "SELECT p.user_id,p.amount,to_char(p.period_start AT TIME ZONE 'Asia/Seoul','YYYY-MM-DD'),to_char(p.period_end AT TIME ZONE 'Asia/Seoul','YYYY-MM-DD'),p.receipt_url,s.card_company,s.card_number FROM subscription_payments p JOIN subscriptions s ON s.user_id=p.user_id WHERE p.order_id=$1 AND p.status='paid'",
        )
        .bind(order_id)
        .fetch_optional(&mut **tx)
        .await?
    else {
        return Ok(());
    };
    let mut body = format!(
        "Thank you for using eunha space.\n\nWe charged {} to {} for {start} to {end}.\n\nOrder: {order_id}",
        won(amount),
        card(company.as_deref(), number.as_deref()),
    );
    if let Some(url) = receipt_url {
        body.push_str(&format!("\nReceipt: {url}"));
    }
    body.push_str(
        "\n\nWithin seven days of a payment you can ask for it back on your account page.",
    );
    enqueue(
        tx,
        user_id,
        "receipt",
        &format!("receipt:{order_id}"),
        &format!("Your eunha space receipt: {}", won(amount)),
        &body,
    )
    .await
}

/// The confirmation of a refund just recorded, in the same transaction.
pub(crate) async fn refund(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: Uuid,
    order_id: &str,
    refunded: i64,
    ended: bool,
) -> Result<(), sqlx::Error> {
    let body = if ended {
        format!(
            "We refunded {} for order {order_id} to your card, and your subscription has ended, so your instances are offline. Their data is kept: subscribe again from your account page to serve them again.\n\nCard companies usually return the money within three to seven business days.",
            won(refunded)
        )
    } else {
        format!(
            "We refunded {} for order {order_id} to your card.\n\nCard companies usually return the money within three to seven business days.",
            won(refunded)
        )
    };
    enqueue(
        tx,
        user_id,
        "refund",
        &format!("refund:{order_id}:{refunded}"),
        &format!("Your eunha space refund: {}", won(refunded)),
        &body,
    )
    .await
}

/// Write a reminder for every subscription renewing within `RENEWAL_NOTICE`
/// that has not had one for that renewal. Returns how many were written.
pub(crate) async fn schedule_renewal_notices(s: &Shared) -> Result<u64> {
    let due = sqlx::query_as::<_, (Uuid, i64, String, String, Option<String>, Option<String>)>(&format!(
        "SELECT s.user_id,s.amount,to_char(s.current_period_end AT TIME ZONE 'Asia/Seoul','YYYY-MM-DD'),to_char(s.current_period_end,'YYYYMMDDHH24MISS'),s.card_company,s.card_number
        FROM subscriptions s
        WHERE s.status='active' AND NOT s.cancel_at_period_end AND s.billing_key IS NOT NULL
          AND s.current_period_end > now()+interval '{RENEWAL_NOTICE_TOO_LATE}'
          AND s.current_period_end <= now()+interval '{RENEWAL_NOTICE}'
          AND NOT EXISTS(SELECT 1 FROM billing_emails e WHERE e.dedupe_key='renewal:'||s.user_id||':'||to_char(s.current_period_end,'YYYYMMDDHH24MISS'))
        LIMIT 200"
    ))
    .fetch_all(&s.db)
    .await?;
    let mut written = 0;
    for (user_id, amount, renews_on, period_key, company, number) in due {
        let mut tx = s.db.begin().await?;
        enqueue(
            &mut tx,
            user_id,
            "renewal_notice",
            &format!("renewal:{user_id}:{period_key}"),
            &format!("Your eunha space subscription renews on {renews_on}"),
            &format!(
                "On {renews_on} we will charge {} to {} for another month of eunha space.\n\nNothing needs doing to keep it. To stop it, cancel before then on your account page; your instances keep running until the paid period ends. To use another card, change it there too.",
                won(amount),
                card(company.as_deref(), number.as_deref()),
            ),
        )
        .await?;
        tx.commit().await?;
        written += 1;
    }
    Ok(written)
}

#[derive(sqlx::FromRow)]
struct Outgoing {
    id: Uuid,
    subject: String,
    body: String,
    email: Option<String>,
}

/// Send one email that is due. Returns whether there was one. Without
/// control-plane SMTP nothing is sent, and emails wait until they go stale.
pub(crate) async fn send_due(s: &Shared) -> Result<bool> {
    let Some(mail) = s.customer_email.as_ref() else {
        return Ok(false);
    };
    let mut tx = s.db.begin().await?;
    // An email that could not be sent for days would arrive about something
    // long past.
    sqlx::query(&format!("UPDATE billing_emails SET abandoned_at=now() WHERE sent_at IS NULL AND abandoned_at IS NULL AND created_at<now()-interval '{GIVE_UP_AFTER}'"))
        .execute(&mut *tx)
        .await?;
    let Some(outgoing) = sqlx::query_as::<_, Outgoing>(
        "SELECT e.id,e.subject,e.body,u.email FROM billing_emails e JOIN users u ON u.id=e.user_id WHERE e.sent_at IS NULL AND e.abandoned_at IS NULL AND e.next_attempt_at<=now() ORDER BY e.next_attempt_at LIMIT 1 FOR UPDATE OF e SKIP LOCKED",
    )
    .fetch_optional(&mut *tx)
    .await?
    else {
        tx.commit().await?;
        return Ok(false);
    };
    // Backed off before sending, so a control plane that dies mid-send does not
    // make another send it again at once: five minutes, doubling, up to six
    // hours.
    sqlx::query("UPDATE billing_emails SET attempts=attempts+1,next_attempt_at=now()+least(interval '5 minutes'*power(2,attempts),interval '6 hours') WHERE id=$1")
        .bind(outgoing.id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;

    let Some(to) = outgoing
        .email
        .as_deref()
        .and_then(|email| email.parse().ok())
    else {
        sqlx::query("UPDATE billing_emails SET abandoned_at=now(),last_error='no deliverable address' WHERE id=$1")
            .bind(outgoing.id)
            .execute(&s.db)
            .await?;
        return Ok(true);
    };
    use lettre::AsyncTransport;
    let message = lettre::Message::builder()
        .from(mail.from.clone())
        .to(to)
        .subject(&outgoing.subject)
        .body(format!(
            "{}\n\n{}/account",
            outgoing.body,
            mail.public_url.trim_end_matches('/')
        ));
    let sent = match message {
        Ok(message) => mail.transport.send(message).await.is_ok(),
        Err(_) => false,
    };
    if sent {
        sqlx::query("UPDATE billing_emails SET sent_at=now(),last_error=NULL WHERE id=$1")
            .bind(outgoing.id)
            .execute(&s.db)
            .await?;
    } else {
        // The transport's error can name the server and account; keep a fixed
        // reason.
        sqlx::query("UPDATE billing_emails SET last_error='not accepted by SMTP' WHERE id=$1")
            .bind(outgoing.id)
            .execute(&s.db)
            .await?;
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amounts_are_won_with_thousands_grouped() {
        assert_eq!(won(0), "₩0");
        assert_eq!(won(900), "₩900");
        assert_eq!(won(9900), "₩9,900");
        assert_eq!(won(1234567), "₩1,234,567");
    }

    #[test]
    fn a_card_is_named_by_what_is_known_of_it() {
        assert_eq!(
            card(Some("현대"), Some("43301234****123*")),
            "your 현대 card ending 123*"
        );
        assert_eq!(
            card(None, Some("43301234****1234")),
            "your card ending 1234"
        );
        assert_eq!(card(None, None), "your card");
    }
}
