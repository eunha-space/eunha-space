//! The console's list of user accounts, apart from billing: who has signed
//! up, who administers, and what they run. Signing in as one of them is in
//! *masquerade.rs*.
use super::*;

#[derive(Debug, Serialize, sqlx::FromRow)]
pub(crate) struct User {
    id: Uuid,
    email: String,
    is_admin: bool,
    verified: bool,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
    /// Instances counting against the account's limit.
    instances: i64,
    /// The subscription's status, null for an account that never subscribed.
    subscription: Option<String>,
    /// When the user last signed in themself, while that session is kept:
    /// sessions are deleted once they expire, after 30 days. Masquerades do
    /// not count.
    #[serde(with = "time::serde::rfc3339::option")]
    last_signed_in_at: Option<OffsetDateTime>,
}

/// `GET /api/users`: every account that can sign in, newest first. Audit-only
/// identities from before accounts had emails are left out.
pub(crate) async fn list(
    State(s): State<Shared>,
    headers: HeaderMap,
) -> Result<Json<Vec<User>>, ApiError> {
    admin_user(&headers, &s.db).await?;
    Ok(Json(
        sqlx::query_as(
            "SELECT u.id,u.email,u.is_admin,u.email_verified_at IS NOT NULL verified,u.created_at,
                (SELECT count(*) FROM tenants t WHERE t.user_id=u.id AND t.lifecycle_state IN ('provisioning','ready','failed')) instances,
                (SELECT s.status FROM subscriptions s WHERE s.user_id=u.id) subscription,
                (SELECT max(us.created_at) FROM user_sessions us WHERE us.user_id=u.id AND us.masquerading_admin_id IS NULL) last_signed_in_at
            FROM users u WHERE u.email IS NOT NULL
            ORDER BY u.created_at DESC
            LIMIT 1000",
        )
        .fetch_all(&s.db)
        .await?,
    ))
}
