//! Receipts and renewal reminders, sent through an outbox.
//!
//! An email is a row in `billing_emails`, written in the same transaction as
//! what it is about: a receipt exists exactly when its charge was recorded as
//! paid, and its dedupe key keeps a second one from being written. The renewer
//! sends what is due, with backoff, and gives up on an email it could not send
//! for three days rather than send a stale one.

use anyhow::Result;
use uuid::Uuid;

use crate::{Shared, locale::Locale};

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

/// The card a charge goes to, by what is known of it: "your 현대 card ending
/// 123*", "현대 카드(끝자리 123*)".
fn card(language: Locale, company: Option<&str>, number: Option<&str>) -> String {
    let ending = number
        .map(|number| number.chars().rev().take(4).collect::<Vec<_>>())
        .map(|last| last.into_iter().rev().collect::<String>());
    match (language, company, ending) {
        (Locale::En, Some(company), Some(ending)) => format!("your {company} card ending {ending}"),
        (Locale::En, Some(company), None) => format!("your {company} card"),
        (Locale::En, None, Some(ending)) => format!("your card ending {ending}"),
        (Locale::En, None, None) => "your card".into(),
        (Locale::Ko, Some(company), Some(ending)) => format!("{company} 카드(끝자리 {ending})"),
        (Locale::Ko, Some(company), None) => format!("{company} 카드"),
        (Locale::Ko, None, Some(ending)) => format!("카드(끝자리 {ending})"),
        (Locale::Ko, None, None) => "등록된 카드".into(),
    }
}

/// An account's language, read in the transaction that writes its email.
async fn language(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: Uuid,
) -> Result<Locale, sqlx::Error> {
    Ok(
        sqlx::query_scalar::<_, Option<String>>("SELECT locale FROM users WHERE id=$1")
            .bind(user_id)
            .fetch_one(&mut **tx)
            .await?
            .as_deref()
            .and_then(Locale::parse)
            .unwrap_or(Locale::En),
    )
}

struct Email {
    kind: &'static str,
    dedupe_key: String,
    language: Locale,
    subject: String,
    body: String,
}

async fn enqueue(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: Uuid,
    email: Email,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO billing_emails(user_id,kind,dedupe_key,locale,subject,body) VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT (dedupe_key) DO NOTHING")
        .bind(user_id)
        .bind(email.kind)
        .bind(email.dedupe_key)
        .bind(email.language.code())
        .bind(email.subject)
        .bind(email.body)
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
    let language = language(tx, user_id).await?;
    let amount = won(amount);
    let card = card(language, company.as_deref(), number.as_deref());
    let (subject, mut body) = match language {
        Locale::En => (
            format!("Your eunha space receipt: {amount}"),
            format!(
                "Thank you for using eunha space.\n\nWe charged {amount} to {card} for {start} to {end}.\n\nOrder: {order_id}"
            ),
        ),
        Locale::Ko => (
            format!("은하공간 결제 영수증: {amount}"),
            format!(
                "은하공간을 이용해 주셔서 감사합니다.\n\n{start}부터 {end}까지의 이용 요금 {amount}을 {card}로 결제했습니다.\n\n주문 번호: {order_id}"
            ),
        ),
    };
    if let Some(url) = receipt_url {
        body.push_str(&match language {
            Locale::En => format!("\nReceipt: {url}"),
            Locale::Ko => format!("\n영수증: {url}"),
        });
    }
    body.push_str(match language {
        Locale::En => {
            "\n\nWithin seven days of a payment you can ask for it back on your account page."
        }
        Locale::Ko => "\n\n결제일로부터 7일 안에는 계정 페이지에서 환불을 요청할 수 있습니다.",
    });
    enqueue(
        tx,
        user_id,
        Email {
            kind: "receipt",
            dedupe_key: format!("receipt:{order_id}"),
            language,
            subject,
            body,
        },
    )
    .await
}

/// The confirmation of a refund just recorded, in the same transaction.
pub(crate) async fn refund(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: Uuid,
    order_id: &str,
    refunded: i64,
    ends_on: Option<&str>,
) -> Result<(), sqlx::Error> {
    let language = language(tx, user_id).await?;
    let amount = won(refunded);
    let (subject, body) = match (language, ends_on) {
        (Locale::En, Some(ends_on)) => (
            format!("Your eunha space refund: {amount}"),
            format!(
                "We refunded {amount} for order {order_id} to your card, and your subscription is cancelled, so you will not be charged again. Your instances keep running until {ends_on}, then go offline with their data kept: subscribe again from your account page to keep serving them.\n\nCard companies usually return the money within three to seven business days."
            ),
        ),
        (Locale::En, None) => (
            format!("Your eunha space refund: {amount}"),
            format!(
                "We refunded {amount} for order {order_id} to your card.\n\nCard companies usually return the money within three to seven business days."
            ),
        ),
        (Locale::Ko, Some(ends_on)) => (
            format!("은하공간 환불 안내: {amount}"),
            format!(
                "주문 {order_id}의 {amount}을 카드로 환불했습니다. 구독이 해지되어 더 이상 결제되지 않습니다. 인스턴스는 {ends_on}까지 계속 운영된 뒤 오프라인으로 전환되며, 데이터는 보관됩니다. 계정 페이지에서 다시 구독하면 계속 서비스됩니다.\n\n카드사에 따라 영업일 기준 3~7일 안에 환불됩니다."
            ),
        ),
        (Locale::Ko, None) => (
            format!("은하공간 환불 안내: {amount}"),
            format!(
                "주문 {order_id}의 {amount}을 카드로 환불했습니다.\n\n카드사에 따라 영업일 기준 3~7일 안에 환불됩니다."
            ),
        ),
    };
    enqueue(
        tx,
        user_id,
        Email {
            kind: "refund",
            dedupe_key: format!("refund:{order_id}:{refunded}"),
            language,
            subject,
            body,
        },
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
        let language = language(&mut tx, user_id).await?;
        let amount = won(amount);
        let card = card(language, company.as_deref(), number.as_deref());
        let (subject, body) = match language {
            Locale::En => (
                format!("Your eunha space subscription renews on {renews_on}"),
                format!(
                    "On {renews_on} we will charge {amount} to {card} for another month of eunha space.\n\nNothing needs doing to keep it. To stop it, cancel before then on your account page; your instances keep running until the paid period ends. To use another card, change it there too."
                ),
            ),
            Locale::Ko => (
                format!("은하공간 구독이 {renews_on}에 갱신됩니다"),
                format!(
                    "{renews_on}에 은하공간 한 달 이용 요금 {amount}을 {card}로 결제할 예정입니다.\n\n계속 이용하려면 따로 하실 일은 없습니다. 갱신을 원하지 않으면 그 전에 계정 페이지에서 구독을 해지하세요. 결제한 기간이 끝날 때까지 인스턴스는 계속 운영됩니다. 다른 카드를 쓰려면 같은 곳에서 카드를 변경할 수 있습니다."
                ),
            ),
        };
        enqueue(
            &mut tx,
            user_id,
            Email {
                kind: "renewal_notice",
                dedupe_key: format!("renewal:{user_id}:{period_key}"),
                language,
                subject,
                body,
            },
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
    locale: String,
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
        "SELECT e.id,e.locale,e.subject,e.body,u.email FROM billing_emails e JOIN users u ON u.id=e.user_id WHERE e.sent_at IS NULL AND e.abandoned_at IS NULL AND e.next_attempt_at<=now() ORDER BY e.next_attempt_at LIMIT 1 FOR UPDATE OF e SKIP LOCKED",
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
        .from(crate::locale::sender(
            mail,
            Locale::parse(&outgoing.locale).unwrap_or(Locale::En),
        ))
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
            card(Locale::En, Some("현대"), Some("43301234****123*")),
            "your 현대 card ending 123*"
        );
        assert_eq!(
            card(Locale::En, None, Some("43301234****1234")),
            "your card ending 1234"
        );
        assert_eq!(card(Locale::En, None, None), "your card");
        assert_eq!(
            card(Locale::Ko, Some("현대"), Some("43301234****123*")),
            "현대 카드(끝자리 123*)"
        );
        assert_eq!(card(Locale::Ko, None, None), "등록된 카드");
    }
}
