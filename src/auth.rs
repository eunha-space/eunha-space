//! Durable request budgets and bounded, blocking password work.
use super::*;
use tokio::sync::Semaphore;
static PASSWORD_WORK: Semaphore = Semaphore::const_new(4);

pub(crate) async fn limit(
    db: &PgPool,
    category: &str,
    identity: &str,
    budget: i32,
) -> Result<(), ApiError> {
    let key = token_hash(&format!("{category}:{identity}"));
    let attempts = sqlx::query_scalar::<_, i32>(
        "INSERT INTO auth_rate_limits(key_hash,window_started_at,attempts) VALUES($1,now(),1) ON CONFLICT(key_hash) DO UPDATE SET attempts=CASE WHEN auth_rate_limits.window_started_at <= now()-interval '15 minutes' THEN 1 ELSE auth_rate_limits.attempts+1 END,window_started_at=CASE WHEN auth_rate_limits.window_started_at <= now()-interval '15 minutes' THEN now() ELSE auth_rate_limits.window_started_at END RETURNING attempts",
    ).bind(key).fetch_one(db).await?;
    if attempts > budget {
        return Err(ApiError(
            StatusCode::TOO_MANY_REQUESTS,
            "Too many attempts. Try again in 15 minutes.".into(),
        ));
    }
    Ok(())
}

pub(crate) async fn verify(password: String, hash: String) -> Result<bool, ApiError> {
    if password.len() > 1024 {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "Password is too long.".into(),
        ));
    }
    let permit = PASSWORD_WORK.try_acquire().map_err(|_| {
        ApiError(
            StatusCode::TOO_MANY_REQUESTS,
            "Password service is busy. Try again shortly.".into(),
        )
    })?;
    let result = tokio::task::spawn_blocking(move || {
        // Hold the permit inside the blocking task, even if its caller disconnects.
        let _permit = permit;
        let parsed = PasswordHash::new(&hash)?;
        Ok::<_, argon2::password_hash::Error>(
            Argon2::default()
                .verify_password(password.as_bytes(), &parsed)
                .is_ok(),
        )
    })
    .await
    .map_err(ApiError::from)?;
    result.map_err(ApiError::from)
}

pub(crate) async fn hash(password: String) -> Result<String, ApiError> {
    if password.len() > 1024 {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "Password is too long.".into(),
        ));
    }
    let permit = PASSWORD_WORK.try_acquire().map_err(|_| {
        ApiError(
            StatusCode::TOO_MANY_REQUESTS,
            "Password service is busy. Try again shortly.".into(),
        )
    })?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let salt = SaltString::generate(&mut OsRng);
        Argon2::default()
            .hash_password(password.as_bytes(), &salt)
            .map(|hash| hash.to_string())
    })
    .await
    .map_err(ApiError::from)?
    .map_err(ApiError::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn password_work_verifies_and_rejects() {
        let hash = hash("a sufficiently long password".into()).await.unwrap();
        assert!(
            verify("a sufficiently long password".into(), hash.clone())
                .await
                .unwrap()
        );
        assert!(!verify("incorrect".into(), hash).await.unwrap());
        assert!(
            verify("anything".into(), "invalid hash".into())
                .await
                .is_err()
        );
    }
}

/// Trust Cloudflare's client address only from a loopback tunnel peer.
pub(crate) fn client_address(peer: std::net::IpAddr, headers: &HeaderMap) -> std::net::IpAddr {
    if peer.is_loopback() {
        if let Some(address) = headers
            .get("cf-connecting-ip")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse().ok())
        {
            return address;
        }
    }
    peer
}

pub(crate) async fn request_budget(
    State(s): State<Shared>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let path = request.uri().path();
    let path = path.strip_prefix("/api").unwrap_or(path);
    let limited = matches!(
        path,
        "/session"
            | "/customer/register"
            | "/customer/session"
            | "/customer/verify"
            | "/customer/password"
            | "/customer/dns-check"
    );
    if limited {
        let peer = request
            .extensions()
            .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
            .map(|info| info.0.ip());
        let Some(peer) = peer else {
            return ApiError(
                StatusCode::SERVICE_UNAVAILABLE,
                "Client address unavailable.".into(),
            )
            .into_response();
        };
        let address = client_address(peer, request.headers());
        if let Err(error) = limit(&s.db, "auth-ip", &address.to_string(), 200).await {
            return error.into_response();
        }
    }
    let mut response = next.run(request).await;
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-store"),
    );
    response
}

#[cfg(test)]
mod address_tests {
    use super::*;
    #[test]
    fn untrusted_peers_cannot_spoof_client_addresses() {
        let mut headers = HeaderMap::new();
        headers.insert("cf-connecting-ip", "192.0.2.10".parse().unwrap());
        assert_eq!(
            client_address("198.51.100.1".parse().unwrap(), &headers).to_string(),
            "198.51.100.1"
        );
        assert_eq!(
            client_address("127.0.0.1".parse().unwrap(), &headers).to_string(),
            "192.0.2.10"
        );
        headers.insert("cf-connecting-ip", "invalid".parse().unwrap());
        assert_eq!(
            client_address("127.0.0.1".parse().unwrap(), &headers).to_string(),
            "127.0.0.1"
        );
    }
}
