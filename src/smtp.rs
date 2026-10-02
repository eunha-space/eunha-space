use super::*;

pub(crate) fn customer_email_config() -> Result<Option<CustomerEmailConfig>> {
    use lettre::{
        AsyncSmtpTransport, Tokio1Executor, transport::smtp::authentication::Credentials,
    };
    let Ok(host) = std::env::var("EUNHA_SPACE_SMTP_HOST") else {
        return Ok(None);
    };
    let port = std::env::var("EUNHA_SPACE_SMTP_PORT").unwrap_or_else(|_| "465".into());
    let transport = match port.as_str() {
        "465" => AsyncSmtpTransport::<Tokio1Executor>::relay(&host),
        "587" => AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&host),
        _ => anyhow::bail!("Control-plane SMTP requires port 465 or 587"),
    }
    .map_err(|_| anyhow::anyhow!("Invalid control-plane SMTP hostname"))?;
    let username = std::env::var("EUNHA_SPACE_SMTP_USERNAME")
        .context("Control-plane SMTP username is required")?;
    let password = std::env::var("EUNHA_SPACE_SMTP_PASSWORD")
        .context("Control-plane SMTP password is required")?;
    anyhow::ensure!(
        !username.is_empty() && !password.is_empty(),
        "Control-plane SMTP credentials are required"
    );
    let from = std::env::var("EUNHA_SPACE_EMAIL_FROM")
        .unwrap_or_else(|_| "eunha space <noreply@eunha.space>".into())
        .parse()
        .map_err(|_| anyhow::anyhow!("Invalid control-plane email sender"))?;
    Ok(Some(CustomerEmailConfig {
        transport: transport
            .credentials(Credentials::new(username, password))
            .timeout(Some(Duration::from_secs(30)))
            .build(),
        from,
        public_url: std::env::var("EUNHA_SPACE_PUBLIC_URL")
            .unwrap_or_else(|_| "https://eunha.space".into()),
    }))
}

#[derive(Deserialize)]
pub(crate) struct Settings {
    host: String,
    port: i32,
    username: String,
    password: String,
    from: String,
}

fn validate(settings: &Settings) -> Result<(), ApiError> {
    if !is_valid_domain(&settings.host)
        || !matches!(settings.port, 465 | 587)
        || settings.username.trim().is_empty()
        || settings.password.is_empty()
        || settings.username.len() > 1024
        || settings.password.len() > 4096
        || settings.username.contains(['\r', '\n'])
        || settings.from.contains(['\r', '\n'])
        || settings.from.len() > 254
        || !settings
            .from
            .split_once('@')
            .is_some_and(|(local, domain)| {
                !local.is_empty() && !local.contains(char::is_whitespace) && is_valid_domain(domain)
            })
    {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "Enter an SMTP hostname, port 465 or 587, credentials, and a sender email address."
                .into(),
        ));
    }
    Ok(())
}

async fn authorize(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    id: Uuid,
    owner: Uuid,
) -> Result<(), ApiError> {
    let eligible = sqlx::query_scalar::<_, bool>("SELECT t.lifecycle_state='ready' AND EXISTS(SELECT 1 FROM tenant_domains d WHERE d.tenant_id=t.id AND d.kind='custom' AND d.status='active' AND d.is_primary) FROM tenants t WHERE t.id=$1 AND t.customer_id=$2 FOR UPDATE")
        .bind(id).bind(owner).fetch_optional(&mut **tx).await?
        .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "Instance not found".into()))?;
    if !eligible {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "SMTP settings require a ready instance with an active custom primary domain.".into(),
        ));
    }
    Ok(())
}

pub(crate) async fn get(
    State(s): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let (owner, _) = customer(&headers, &s.db).await?;
    let mut tx = s.db.begin().await?;
    authorize(&mut tx, id, owner).await?;
    // No credential values leave PostgreSQL, including the login name.
    let (host, port, from) = sqlx::query_as::<_, (Option<String>, Option<i32>, Option<String>)>(
        "SELECT smtp_host,smtp_port,smtp_from FROM tenant_configuration WHERE tenant_id=$1",
    )
    .bind(id)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(
        serde_json::json!({"configured":host.is_some(), "host":host, "port":port, "from":from}),
    ))
}

pub(crate) async fn save(
    State(s): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(mut settings): Json<Settings>,
) -> Result<StatusCode, ApiError> {
    let (owner, _) = customer(&headers, &s.db).await?;
    settings.host = settings.host.trim().to_ascii_lowercase();
    settings.from = settings.from.trim().to_owned();
    validate(&settings)?;
    let mut tx = s.db.begin().await?;
    authorize(&mut tx, id, owner).await?;
    sqlx::query("UPDATE tenant_configuration SET smtp_host=$2,smtp_port=$3,smtp_username=$4,smtp_password=$5,smtp_from=$6 WHERE tenant_id=$1")
        .bind(id).bind(settings.host).bind(settings.port).bind(settings.username).bind(settings.password).bind(settings.from).execute(&mut *tx).await?;
    sqlx::query("UPDATE tenants SET reconcile_failures=0,reconcile_error=NULL WHERE id=$1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO operations(id,tenant_id,kind,status,requested_by_customer,started_at,finished_at) VALUES($1,$2,'smtp_update','succeeded',$3,now(),now())")
        .bind(Uuid::new_v4()).bind(id).bind(owner).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_plaintext_ports_and_header_injection() {
        let mut settings = Settings {
            host: "smtp.example.com".into(),
            port: 587,
            username: "login".into(),
            password: "secret".into(),
            from: "mail@example.com".into(),
        };
        assert!(validate(&settings).is_ok());
        settings.port = 25;
        assert!(validate(&settings).is_err());
        settings.port = 465;
        settings.from = "mail@example.com\r\nBcc: other@example.com".into();
        assert!(validate(&settings).is_err());
    }
}
