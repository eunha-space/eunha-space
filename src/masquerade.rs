//! An administrator signing in as another user, to see the customer pages as
//! they do. The masquerade is a separate, hour-long session of that user,
//! marked with the administrator it came from; the administrator's own session
//! is set aside in a cookie only the customer API sees, and comes back when
//! the masquerade ends. A masquerade cannot reach the console, and cannot
//! change the user's password, billing or language.
use super::*;

const MASQUERADE_SECONDS: u32 = 3600;
const ADMIN_COOKIE: &str = "eunha_space_admin_session";

fn admin_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .map(str::trim)
        .find_map(|value| value.strip_prefix("eunha_space_admin_session="))
}

fn admin_cookie(token: &str, max_age: u32) -> String {
    // Only ending the masquerade, under /api/customer, ever reads it back.
    format!(
        "{ADMIN_COOKIE}={token}; Path=/api/customer; HttpOnly; Secure; SameSite=Strict; Max-Age={max_age}"
    )
}

fn customer_cookie(token: &str, max_age: u32) -> String {
    format!(
        "eunha_space_customer_session={token}; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age={max_age}"
    )
}

/// The administrator behind this session, when it is a masquerade.
pub(crate) async fn admin_of(
    headers: &HeaderMap,
    db: &PgPool,
) -> Result<Option<(Uuid, String)>, ApiError> {
    let Some(token) = customer_token(headers) else {
        return Ok(None);
    };
    Ok(sqlx::query_as(
        "SELECT a.id,a.email FROM user_sessions s JOIN users a ON a.id=s.masquerading_admin_id WHERE s.token_hash=$1 AND s.expires_at>now()",
    )
    .bind(token_hash(token))
    .fetch_optional(db)
    .await?)
}

/// Refuses what only the user themself may do: their password and billing.
pub(crate) async fn guard(
    State(s): State<Shared>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let path = request.uri().path();
    let path = path.strip_prefix("/api").unwrap_or(path);
    let personal = request.method() != axum::http::Method::GET
        && (path == "/customer/password" || path.starts_with("/customer/billing"));
    if personal {
        match admin_of(request.headers(), &s.db).await {
            Ok(None) => {}
            Ok(Some(_)) => {
                return ApiError(
                    StatusCode::FORBIDDEN,
                    "Only the account's owner can do that, not an administrator signed in as them."
                        .into(),
                )
                .into_response();
            }
            Err(error) => return error.into_response(),
        }
    }
    next.run(request).await
}

/// `POST /api/users/{id}/masquerade`: sign in as the user, keeping the
/// administrator's session to come back to.
pub(crate) async fn start(
    State(s): State<Shared>,
    headers: HeaderMap,
    Path(user_id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let (admin_id, _) = admin_user(&headers, &s.db).await?;
    let own_token = customer_token(&headers)
        .ok_or_else(|| ApiError(StatusCode::UNAUTHORIZED, "Sign in required".into()))?
        .to_owned();
    if user_id == admin_id {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "You are already signed in as yourself.".into(),
        ));
    }
    let (email, is_admin, verified) = sqlx::query_as::<_, (String, bool, bool)>(
        "SELECT email,is_admin,email_verified_at IS NOT NULL FROM users WHERE id=$1 AND email IS NOT NULL",
    )
    .bind(user_id)
    .fetch_optional(&s.db)
    .await?
    .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "No such account.".into()))?;
    // Another administrator's session would carry the console with it.
    if is_admin {
        return Err(ApiError(
            StatusCode::FORBIDDEN,
            "Administrators cannot be signed in as.".into(),
        ));
    }
    if !verified {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "That account has not verified its email yet.".into(),
        ));
    }
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    let token = bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
    let mut tx = s.db.begin().await?;
    sqlx::query("INSERT INTO user_sessions(token_hash,user_id,expires_at,masquerading_admin_id) VALUES($1,$2,now()+make_interval(secs=>$3),$4)")
        .bind(token_hash(&token))
        .bind(user_id)
        .bind(f64::from(MASQUERADE_SECONDS))
        .bind(admin_id)
        .execute(&mut *tx)
        .await?;
    billing::record(
        &mut tx,
        admin_id,
        "masquerade",
        "succeeded",
        serde_json::json!({"user_id": user_id, "email": email}),
        None,
    )
    .await?;
    tx.commit().await?;
    Ok((
        axum::response::AppendHeaders([
            (
                header::SET_COOKIE,
                customer_cookie(&token, MASQUERADE_SECONDS),
            ),
            (
                header::SET_COOKIE,
                admin_cookie(&own_token, MASQUERADE_SECONDS),
            ),
        ]),
        Json(serde_json::json!({ "email": email })),
    )
        .into_response())
}

/// `DELETE /api/customer/masquerade`: end the masquerade and, while it is
/// still good, go back to the administrator's own session.
pub(crate) async fn stop(
    State(s): State<Shared>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let token = customer_token(&headers)
        .ok_or_else(|| ApiError(StatusCode::UNAUTHORIZED, "Sign in required".into()))?;
    let admin_id = sqlx::query_scalar::<_, Uuid>(
        "DELETE FROM user_sessions WHERE token_hash=$1 AND masquerading_admin_id IS NOT NULL RETURNING masquerading_admin_id",
    )
    .bind(token_hash(token))
    .fetch_optional(&s.db)
    .await?
    .ok_or_else(|| {
        ApiError(
            StatusCode::CONFLICT,
            "You are not signed in as another user.".into(),
        )
    })?;
    let mut restored = None;
    if let Some(own) = admin_token(&headers) {
        let live = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM user_sessions s JOIN users u ON u.id=s.user_id WHERE s.token_hash=$1 AND s.user_id=$2 AND s.expires_at>now() AND s.masquerading_admin_id IS NULL AND u.is_admin)",
        )
        .bind(token_hash(own))
        .bind(admin_id)
        .fetch_one(&s.db)
        .await?;
        if live {
            restored = Some(own);
        }
    }
    let customer = match restored {
        Some(own) => customer_cookie(own, 2_592_000),
        None => customer_cookie("", 0),
    };
    Ok((
        axum::response::AppendHeaders([
            (header::SET_COOKIE, customer),
            (header::SET_COOKIE, admin_cookie("", 0)),
        ]),
        Json(serde_json::json!({ "restored": restored.is_some() })),
    )
        .into_response())
}
