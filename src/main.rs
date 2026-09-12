use anyhow::{Context, Result};
use argon2::{
    Argon2, PasswordHash, PasswordHasher, PasswordVerifier,
    password_hash::{
        SaltString,
        rand_core::{OsRng, RngCore},
    },
};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use clap::{Parser, Subcommand};
use p256::{
    ecdsa::SigningKey,
    pkcs8::{EncodePrivateKey, LineEnding},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, postgres::PgPoolOptions, types::Json as SqlJson};
use std::{
    path::{Component, Path as FsPath, PathBuf},
    process::Stdio,
    sync::Arc,
    time::Duration,
};
use time::OffsetDateTime;
use tower_http::{
    services::{ServeDir, ServeFile},
    trace::TraceLayer,
};
use uuid::Uuid;

// SQLx embeds the complete migration set in this binary at compile time.

#[derive(Parser)]
struct Args {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Create or replace an operator password from EUNHA_SPACE_ADMIN_PASSWORD.
    SetOperator { username: String },
}

#[derive(Clone)]
struct AppState {
    db: PgPool,
    client: reqwest::Client,
    provision_database_url: String,
}
type Shared = Arc<AppState>;

#[derive(Debug)]
struct ApiError(StatusCode, String);
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(serde_json::json!({ "error": self.1 }))).into_response()
    }
}
impl<E: std::fmt::Display> From<E> for ApiError {
    fn from(value: E) -> Self {
        Self(StatusCode::INTERNAL_SERVER_ERROR, value.to_string())
    }
}

#[derive(Serialize, sqlx::FromRow)]
struct TenantSummary {
    id: Uuid,
    domain: String,
    title: String,
    lifecycle_state: String,
    desired_state: String,
    observed_state: String,
    config_revision: i64,
    rendered_revision: Option<i64>,
    runtime_name: String,
    bind_address: String,
    #[serde(with = "time::serde::rfc3339")]
    updated_at: OffsetDateTime,
}

#[derive(Serialize, sqlx::FromRow)]
struct OperationSummary {
    id: Uuid,
    domain: Option<String>,
    runtime_name: Option<String>,
    kind: String,
    status: String,
    error: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
}

#[derive(Serialize, sqlx::FromRow)]
struct RuntimeSummary {
    id: Uuid,
    name: String,
    instance_name: String,
    instance_address: String,
    bind_address: String,
    service_label: String,
    tenant_directory: String,
    tenant_count: i64,
    auto_deploy_enabled: bool,
    deployed_ref: Option<String>,
    release_operation_id: Option<Uuid>,
    release_status: Option<String>,
    release_message: Option<String>,
}

#[derive(Serialize, sqlx::FromRow)]
struct OperationEvent {
    id: i64,
    message: String,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
}

#[derive(Deserialize)]
struct Login {
    username: String,
    password: String,
}

#[derive(Deserialize)]
struct AutomaticRelease {
    enabled: bool,
}

#[derive(Deserialize)]
struct NewTenant {
    domain: String,
    title: String,
    contact_email: Option<String>,
    runtime_id: Uuid,
    #[serde(default = "default_true")]
    registrations_open: bool,
    #[serde(default)]
    approval_required: bool,
}

#[derive(Deserialize, Serialize)]
struct TenantToml {
    database_url: String,
    #[serde(default)]
    database_pool: DatabasePoolToml,
    redis_url: String,
    #[serde(default)]
    redis_coordination_url: Option<String>,
    #[serde(default)]
    redis_key_prefix: String,
    #[serde(default = "default_true")]
    redis_process_metrics: bool,
    bind_address: String,
    media_storage: MediaStorageToml,
    #[serde(default)]
    smtp: Option<SmtpToml>,
    resend: ResendToml,
    instance: InstanceToml,
    #[serde(default)]
    active_record_encryption: Option<EncryptionToml>,
    #[serde(default)]
    software_update_url: Option<String>,
    #[serde(default)]
    allowed_private_networks: Vec<String>,
    #[serde(default)]
    sign_integrity_proofs: bool,
    #[serde(default)]
    workers: WorkersToml,
    #[serde(default)]
    limits: LimitsToml,
}
fn default_true() -> bool {
    true
}
#[derive(Deserialize, Serialize)]
struct DatabasePoolToml {
    #[serde(default = "d20")]
    max_connections: i32,
    #[serde(default)]
    min_connections: i32,
    #[serde(default = "d30")]
    acquire_timeout_seconds: i64,
    #[serde(default = "d600")]
    idle_timeout_seconds: i64,
}
fn d20() -> i32 {
    20
}
fn d30() -> i64 {
    30
}
fn d600() -> i64 {
    600
}
impl Default for DatabasePoolToml {
    fn default() -> Self {
        Self {
            max_connections: 20,
            min_connections: 0,
            acquire_timeout_seconds: 30,
            idle_timeout_seconds: 600,
        }
    }
}
#[derive(Deserialize, Serialize)]
struct MediaStorageToml {
    #[serde(skip_serializing_if = "Option::is_none")]
    bucket: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    key_prefix: String,
    region: String,
    endpoint: Option<String>,
    access_key_id: String,
    secret_access_key: String,
    base_url: String,
}
#[derive(Deserialize, Serialize)]
struct ResendToml {
    api_key: String,
    from: String,
}
#[derive(Deserialize, Serialize)]
struct SmtpToml {
    host: String,
    port: i32,
    username: String,
    password: String,
    from: String,
}
#[derive(Deserialize, Serialize)]
struct EncryptionToml {
    primary_key: String,
    key_derivation_salt: String,
}
#[derive(Deserialize, Serialize)]
struct InstanceToml {
    domain: String,
    title: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    short_description: String,
    contact_email: Option<String>,
    #[serde(default = "default_true")]
    registrations_open: bool,
    #[serde(default)]
    approval_required: bool,
    vapid_private_key: String,
    vapid_public_key: String,
    icon_url: Option<String>,
    #[serde(default)]
    privacy_policy: String,
    #[serde(default)]
    terms_of_service: String,
}
#[derive(Deserialize, Serialize)]
struct WorkersToml {
    #[serde(default = "one")]
    delivery_workers: i32,
    #[serde(default = "d50")]
    delivery_batch: i64,
    #[serde(default = "d16")]
    delivery_concurrency: i32,
    #[serde(default = "one")]
    inbox_workers: i32,
    #[serde(default = "d20i")]
    inbox_batch: i64,
    #[serde(default = "d4")]
    inbox_concurrency: i32,
    #[serde(default = "d30")]
    queue_idle_poll_seconds: i64,
    #[serde(default = "d256")]
    process_delivery_concurrency: i32,
}
fn one() -> i32 {
    1
}
fn d50() -> i64 {
    50
}
fn d16() -> i32 {
    16
}
fn d20i() -> i64 {
    20
}
fn d4() -> i32 {
    4
}
fn d256() -> i32 {
    256
}
impl Default for WorkersToml {
    fn default() -> Self {
        Self {
            delivery_workers: 1,
            delivery_batch: 50,
            delivery_concurrency: 16,
            inbox_workers: 1,
            inbox_batch: 20,
            inbox_concurrency: 4,
            queue_idle_poll_seconds: 30,
            process_delivery_concurrency: 256,
        }
    }
}
#[derive(Deserialize, Serialize, Default)]
struct LimitsToml {
    max_concurrent_requests: Option<i32>,
    process_max_tenants: Option<i32>,
    process_database_connections: Option<i64>,
}

#[derive(Deserialize)]
struct StoredConfig {
    database_url: String,
    database_pool_max_connections: i32,
    database_pool_min_connections: i32,
    database_pool_acquire_timeout_seconds: i64,
    database_pool_idle_timeout_seconds: i64,
    redis_url: String,
    redis_coordination_url: Option<String>,
    redis_key_prefix: String,
    redis_process_metrics: bool,
    resend_api_key: String,
    resend_from: String,
    smtp_host: Option<String>,
    smtp_port: Option<i32>,
    smtp_username: Option<String>,
    smtp_password: Option<String>,
    smtp_from: Option<String>,
    active_record_encryption_primary_key: Option<String>,
    active_record_encryption_key_derivation_salt: Option<String>,
    media_storage_region: String,
    media_storage_endpoint: Option<String>,
    media_storage_access_key_id: String,
    media_storage_secret_access_key: String,
    media_storage_base_url: String,
    media_storage_key_prefix: String,
    software_update_url: Option<String>,
    allowed_private_networks: Vec<String>,
    sign_integrity_proofs: bool,
    delivery_workers: i32,
    delivery_batch: i64,
    delivery_concurrency: i32,
    inbox_workers: i32,
    inbox_batch: i64,
    inbox_concurrency: i32,
    queue_idle_poll_seconds: i64,
    process_delivery_concurrency: i32,
    max_concurrent_requests: Option<i32>,
    process_max_tenants: Option<i32>,
    process_database_connections: Option<i64>,
}
#[derive(Deserialize)]
struct StoredTenant {
    domain: String,
    title: String,
    description: String,
    short_description: String,
    contact_email: Option<String>,
    registrations_open: bool,
    approval_required: bool,
    vapid_private_key: Option<String>,
    vapid_public_key: Option<String>,
    icon_url: Option<String>,
    privacy_policy: String,
    terms_of_service: String,
}
fn required(value: Option<String>, name: &str) -> Result<String> {
    value.with_context(|| format!("typed configuration is missing {name}"))
}
fn typed_toml(
    tenant: StoredTenant,
    config: StoredConfig,
    bucket: String,
    bind_address: String,
) -> Result<TenantToml> {
    let smtp = match (
        config.smtp_host,
        config.smtp_port,
        config.smtp_username,
        config.smtp_password,
        config.smtp_from,
    ) {
        (None, None, None, None, None) => None,
        (h, p, u, pw, f) => Some(SmtpToml {
            host: required(h, "smtp_host")?,
            port: p.context("typed configuration is missing smtp_port")?,
            username: required(u, "smtp_username")?,
            password: required(pw, "smtp_password")?,
            from: required(f, "smtp_from")?,
        }),
    };
    let encryption = match (
        config.active_record_encryption_primary_key,
        config.active_record_encryption_key_derivation_salt,
    ) {
        (None, None) => None,
        (p, s) => Some(EncryptionToml {
            primary_key: required(p, "active_record_encryption_primary_key")?,
            key_derivation_salt: required(s, "active_record_encryption_key_derivation_salt")?,
        }),
    };
    Ok(TenantToml {
        database_url: config.database_url,
        database_pool: DatabasePoolToml {
            max_connections: config.database_pool_max_connections,
            min_connections: config.database_pool_min_connections,
            acquire_timeout_seconds: config.database_pool_acquire_timeout_seconds,
            idle_timeout_seconds: config.database_pool_idle_timeout_seconds,
        },
        redis_url: config.redis_url,
        redis_coordination_url: config.redis_coordination_url,
        redis_key_prefix: config.redis_key_prefix,
        redis_process_metrics: config.redis_process_metrics,
        bind_address,
        media_storage: MediaStorageToml {
            bucket: Some(bucket),
            key_prefix: config.media_storage_key_prefix,
            region: config.media_storage_region,
            endpoint: config.media_storage_endpoint,
            access_key_id: config.media_storage_access_key_id,
            secret_access_key: config.media_storage_secret_access_key,
            base_url: config.media_storage_base_url,
        },
        smtp,
        resend: ResendToml {
            api_key: config.resend_api_key,
            from: config.resend_from,
        },
        instance: InstanceToml {
            domain: tenant.domain,
            title: tenant.title,
            description: tenant.description,
            short_description: tenant.short_description,
            contact_email: tenant.contact_email,
            registrations_open: tenant.registrations_open,
            approval_required: tenant.approval_required,
            vapid_private_key: required(tenant.vapid_private_key, "vapid_private_key")?,
            vapid_public_key: required(tenant.vapid_public_key, "vapid_public_key")?,
            icon_url: tenant.icon_url,
            privacy_policy: tenant.privacy_policy,
            terms_of_service: tenant.terms_of_service,
        },
        active_record_encryption: encryption,
        software_update_url: config.software_update_url,
        allowed_private_networks: config.allowed_private_networks,
        sign_integrity_proofs: config.sign_integrity_proofs,
        workers: WorkersToml {
            delivery_workers: config.delivery_workers,
            delivery_batch: config.delivery_batch,
            delivery_concurrency: config.delivery_concurrency,
            inbox_workers: config.inbox_workers,
            inbox_batch: config.inbox_batch,
            inbox_concurrency: config.inbox_concurrency,
            queue_idle_poll_seconds: config.queue_idle_poll_seconds,
            process_delivery_concurrency: config.process_delivery_concurrency,
        },
        limits: LimitsToml {
            max_concurrent_requests: config.max_concurrent_requests,
            process_max_tenants: config.process_max_tenants,
            process_database_connections: config.process_database_connections,
        },
    })
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "eunha_space=info,tower_http=info".into()),
        )
        .init();
    let args = Args::parse();
    let database_url =
        std::env::var("CONTROL_DATABASE_URL").context("CONTROL_DATABASE_URL is required")?;
    let db = PgPoolOptions::new()
        .max_connections(8)
        .connect(&database_url)
        .await?;
    sqlx::migrate!().run(&db).await?;

    match args.command {
        Some(Command::SetOperator { username }) => return set_operator(&db, &username).await,
        None => {}
    }

    let state = Arc::new(AppState {
        db,
        provision_database_url: std::env::var("CONTROL_PROVISION_DATABASE_URL")
            .unwrap_or(database_url),
        client: reqwest::Client::builder()
            .timeout(Duration::from_secs(4))
            .build()?,
    });
    tokio::spawn(health_reconciler(state.clone()));
    tokio::spawn(automatic_release_reconciler(state.clone()));
    tokio::spawn(provisioning_reconciler(state.clone()));
    let api = Router::new()
        .route("/session", post(login).delete(logout))
        .route("/session/me", get(me))
        .route("/tenants", get(tenants).post(create_tenant))
        .route("/tenants/{id}/check", post(check_tenant))
        .route("/tenants/{id}/retry", post(retry_provisioning))
        .route("/tenants/{id}/configuration", get(tenant_configuration))
        .route("/tenants/{id}/render", post(render_tenant))
        .route("/runtimes", get(runtimes))
        .route("/runtimes/{id}/release", post(release_runtime))
        .route(
            "/runtimes/{id}/automatic-release",
            post(set_automatic_release),
        )
        .route("/operations/{id}/events", get(operation_events))
        .route("/operations", get(operations));
    // The console owns /admin/<section>; each of those paths has to serve the
    // shell with a 200 so a deep link or a refresh lands on the right section.
    // ServeDir::not_found_service would serve the shell but force a 404, and a
    // blanket fallback would answer for genuinely missing assets too, so the
    // console routes are explicit and everything else still 404s.
    let index = ServeFile::new("frontend/dist/index.html");
    let app = Router::new()
        .nest("/api", api)
        .route_service("/", index.clone())
        .route_service("/admin", index.clone())
        .route_service("/admin/{*section}", index)
        .fallback_service(ServeDir::new("frontend/dist"))
        .layer(TraceLayer::new_for_http())
        .with_state(state);
    let bind = std::env::var("BIND_ADDRESS").unwrap_or_else(|_| "127.0.0.1:60001".into());
    let listener = tokio::net::TcpListener::bind(&bind).await?;
    tracing::info!(%bind, "eunha.space control plane listening");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn set_operator(db: &PgPool, username: &str) -> Result<()> {
    let password = std::env::var("EUNHA_SPACE_ADMIN_PASSWORD")
        .context("EUNHA_SPACE_ADMIN_PASSWORD is required")?;
    anyhow::ensure!(
        password.len() >= 14,
        "administrator password must be at least 14 characters"
    );
    let salt = SaltString::generate(&mut OsRng);
    let hash = Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map_err(|e| anyhow::anyhow!("hashing operator password: {e}"))?
        .to_string();
    sqlx::query("INSERT INTO operators(id, username, password_hash) VALUES($1,$2,$3) ON CONFLICT(username) DO UPDATE SET password_hash=excluded.password_hash")
        .bind(Uuid::new_v4()).bind(username).bind(hash).execute(db).await?;
    println!("Operator {username} is ready.");
    Ok(())
}

fn token_hash(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}
fn cookie_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .map(str::trim)
        .find_map(|v| v.strip_prefix("eunha_space_session="))
}
async fn operator(headers: &HeaderMap, db: &PgPool) -> Result<(Uuid, String), ApiError> {
    let token = cookie_token(headers)
        .ok_or_else(|| ApiError(StatusCode::UNAUTHORIZED, "Sign in required".into()))?;
    sqlx::query_as::<_, (Uuid,String)>("SELECT o.id,o.username FROM sessions s JOIN operators o ON o.id=s.operator_id WHERE s.token_hash=$1 AND s.expires_at>now()")
        .bind(token_hash(token)).fetch_optional(db).await?.ok_or_else(|| ApiError(StatusCode::UNAUTHORIZED, "Session expired".into()))
}
async fn login(State(s): State<Shared>, Json(body): Json<Login>) -> Result<Response, ApiError> {
    let row = sqlx::query_as::<_, (Uuid, String)>(
        "SELECT id,password_hash FROM operators WHERE username=$1",
    )
    .bind(&body.username)
    .fetch_optional(&s.db)
    .await?;
    let Some((id, hash)) = row else {
        return Err(ApiError(
            StatusCode::UNAUTHORIZED,
            "Invalid credentials".into(),
        ));
    };
    let parsed = PasswordHash::new(&hash).map_err(|_| {
        ApiError(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Stored credential is invalid".into(),
        )
    })?;
    if Argon2::default()
        .verify_password(body.password.as_bytes(), &parsed)
        .is_err()
    {
        return Err(ApiError(
            StatusCode::UNAUTHORIZED,
            "Invalid credentials".into(),
        ));
    }
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    let token = bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
    sqlx::query("INSERT INTO sessions(token_hash,operator_id,expires_at) VALUES($1,$2,now()+interval '12 hours')")
        .bind(token_hash(&token)).bind(id).execute(&s.db).await?;
    Ok(([(header::SET_COOKIE, format!("eunha_space_session={token}; Path=/; HttpOnly; Secure; SameSite=Strict; Max-Age=43200"))], Json(serde_json::json!({"username": body.username}))).into_response())
}
async fn logout(State(s): State<Shared>, headers: HeaderMap) -> Result<Response, ApiError> {
    if let Some(token) = cookie_token(&headers) {
        sqlx::query("DELETE FROM sessions WHERE token_hash=$1")
            .bind(token_hash(token))
            .execute(&s.db)
            .await?;
    }
    Ok((
        [(
            header::SET_COOKIE,
            "eunha_space_session=; Path=/; HttpOnly; Secure; SameSite=Strict; Max-Age=0",
        )],
        StatusCode::NO_CONTENT,
    )
        .into_response())
}
async fn me(
    State(s): State<Shared>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    let (_, username) = operator(&headers, &s.db).await?;
    Ok(Json(serde_json::json!({"username": username})))
}
async fn tenants(
    State(s): State<Shared>,
    headers: HeaderMap,
) -> Result<Json<Vec<TenantSummary>>, ApiError> {
    operator(&headers, &s.db).await?;
    Ok(Json(sqlx::query_as("SELECT t.id,t.domain,t.title,t.lifecycle_state,t.desired_state,CASE WHEN t.observed_at IS NULL OR t.observed_at < now()-interval '90 seconds' THEN 'unknown' ELSE t.observed_state END observed_state,t.config_revision,t.rendered_revision,r.name runtime_name,r.bind_address,t.updated_at FROM tenants t JOIN runtimes r ON r.id=t.runtime_id ORDER BY t.domain").fetch_all(&s.db).await?))
}

fn random_hex(bytes: usize) -> String {
    let mut value = vec![0_u8; bytes];
    OsRng.fill_bytes(&mut value);
    value.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn tenant_database_url(admin_url: &str, tenant_id: Uuid, password: &str) -> Result<String> {
    let compact = tenant_id.simple().to_string();
    let mut url = url::Url::parse(admin_url).context("invalid provisioning database URL")?;
    url.set_username(&format!("eunha_r_{compact}"))
        .map_err(|_| anyhow::anyhow!("invalid provisioning database username"))?;
    url.set_password(Some(password))
        .map_err(|_| anyhow::anyhow!("invalid provisioning database password"))?;
    url.set_path(&format!("/eunha_t_{compact}"));
    Ok(url.to_string())
}

fn generate_vapid_keypair() -> Result<(String, String)> {
    let signing_key = SigningKey::random(&mut OsRng);
    let pem = signing_key
        .to_pkcs8_pem(LineEnding::LF)
        .map_err(|error| anyhow::anyhow!("encoding VAPID key: {error}"))?;
    let public = signing_key.verifying_key().to_encoded_point(false);
    Ok((pem.to_string(), URL_SAFE_NO_PAD.encode(public.as_bytes())))
}

async fn create_tenant(
    State(s): State<Shared>,
    headers: HeaderMap,
    Json(mut body): Json<NewTenant>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let (operator_id, _) = operator(&headers, &s.db).await?;
    body.domain = body
        .domain
        .trim()
        .trim_end_matches('.')
        .to_ascii_lowercase();
    body.title = body.title.trim().to_owned();
    body.contact_email = body
        .contact_email
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());
    if !is_safe_domain(&body.domain)
        || body.domain.starts_with(['.', '-'])
        || body.domain.ends_with('-')
        || !body.domain.contains('.')
        || body.title.is_empty()
        || body.title.len() > 200
    {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "Enter a valid domain and title.".into(),
        ));
    }

    let template = sqlx::query_scalar::<_, Uuid>(
        "SELECT t.id FROM tenants t JOIN tenant_configuration c ON c.tenant_id=t.id JOIN r2_buckets b ON b.tenant_id=t.id WHERE t.runtime_id=$1 AND t.lifecycle_state='ready' ORDER BY t.created_at LIMIT 1",
    ).bind(body.runtime_id).fetch_optional(&s.db).await?
        .ok_or_else(|| ApiError(StatusCode::CONFLICT, "This runtime has no ready tenant to use as its provider template.".into()))?;

    let tenant_id = Uuid::new_v4();
    let operation_id = Uuid::new_v4();
    let password = random_hex(24);
    let database_url = tenant_database_url(&s.provision_database_url, tenant_id, &password)
        .map_err(|_| {
            ApiError(
                StatusCode::INTERNAL_SERVER_ERROR,
                "The provisioning database is not configured.".into(),
            )
        })?;
    let (vapid_private, vapid_public) = generate_vapid_keypair().map_err(|_| {
        ApiError(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Could not generate tenant keys.".into(),
        )
    })?;
    let encryption_primary = random_hex(32);
    let encryption_salt = random_hex(32);
    let prefix = format!("t/{tenant_id}");
    let redis_prefix = format!("t:{}:", tenant_id.simple());

    let mut tx = s.db.begin().await?;
    sqlx::query("INSERT INTO tenants(id,domain,title,runtime_id,desired_state,observed_state,lifecycle_state,contact_email,registrations_open,approval_required,vapid_private_key,vapid_public_key) VALUES($1,$2,$3,$4,'running','unknown','provisioning',$5,$6,$7,$8,$9)")
        .bind(tenant_id).bind(&body.domain).bind(&body.title).bind(body.runtime_id)
        .bind(&body.contact_email).bind(body.registrations_open).bind(body.approval_required)
        .bind(vapid_private).bind(vapid_public).execute(&mut *tx).await
        .map_err(|error| if error.as_database_error().and_then(|e| e.constraint()) == Some("tenants_domain_key") { ApiError(StatusCode::CONFLICT, "That domain is already managed.".into()) } else { ApiError::from(error) })?;
    sqlx::query(r#"INSERT INTO tenant_configuration(
        tenant_id,database_url,database_pool_max_connections,database_pool_min_connections,database_pool_acquire_timeout_seconds,database_pool_idle_timeout_seconds,
        redis_url,redis_coordination_url,redis_key_prefix,redis_process_metrics,smtp_host,smtp_port,smtp_username,smtp_password,smtp_from,resend_api_key,resend_from,
        active_record_encryption_primary_key,active_record_encryption_key_derivation_salt,software_update_url,allowed_private_networks,sign_integrity_proofs,
        media_storage_region,media_storage_endpoint,media_storage_access_key_id,media_storage_secret_access_key,media_storage_base_url,media_storage_key_prefix,
        delivery_workers,delivery_batch,delivery_concurrency,inbox_workers,inbox_batch,inbox_concurrency,queue_idle_poll_seconds,process_delivery_concurrency,
        max_concurrent_requests,process_max_tenants,process_database_connections)
      SELECT $1,$2,database_pool_max_connections,database_pool_min_connections,database_pool_acquire_timeout_seconds,database_pool_idle_timeout_seconds,
        redis_url,redis_coordination_url,$3,redis_process_metrics,smtp_host,smtp_port,smtp_username,smtp_password,smtp_from,resend_api_key,resend_from,
        $4,$5,software_update_url,allowed_private_networks,sign_integrity_proofs,
        media_storage_region,media_storage_endpoint,media_storage_access_key_id,media_storage_secret_access_key,media_storage_base_url,$6,
        delivery_workers,delivery_batch,delivery_concurrency,inbox_workers,inbox_batch,inbox_concurrency,queue_idle_poll_seconds,process_delivery_concurrency,
        max_concurrent_requests,process_max_tenants,process_database_connections FROM tenant_configuration WHERE tenant_id=$7"#)
        .bind(tenant_id).bind(database_url).bind(redis_prefix).bind(encryption_primary).bind(encryption_salt).bind(prefix).bind(template)
        .execute(&mut *tx).await?;
    sqlx::query("INSERT INTO r2_buckets(tenant_id,bucket_name,lifecycle_state) SELECT $1,bucket_name,'provisioning' FROM r2_buckets WHERE tenant_id=$2")
        .bind(tenant_id).bind(template).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO operations(id,tenant_id,runtime_id,kind,status,requested_by,detail,started_at) VALUES($1,$2,$3,'tenant_provision','running',$4,jsonb_build_object('domain',$5),now())")
        .bind(operation_id).bind(tenant_id).bind(body.runtime_id).bind(operator_id).bind(&body.domain).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO provisioning_steps(tenant_id,step) SELECT $1,unnest(ARRAY['database','migrate','render','reload','health'])")
        .bind(tenant_id).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO operation_events(operation_id,message) VALUES($1,'Tenant reserved; provisioning queued.')")
        .bind(operation_id).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(serde_json::json!({"id": tenant_id, "operation_id": operation_id})),
    ))
}

async fn retry_provisioning(
    State(s): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let (operator_id, _) = operator(&headers, &s.db).await?;
    let operation_id = Uuid::new_v4();
    let mut tx = s.db.begin().await?;
    let changed = sqlx::query("UPDATE tenants SET lifecycle_state='provisioning',updated_at=now() WHERE id=$1 AND lifecycle_state='failed'")
        .bind(id).execute(&mut *tx).await?.rows_affected();
    if changed == 0 {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "Only failed provisioning can be retried.".into(),
        ));
    }
    sqlx::query("UPDATE provisioning_steps SET status='pending',error=NULL,started_at=NULL,finished_at=NULL,updated_at=now() WHERE tenant_id=$1 AND status='failed'")
        .bind(id).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO operations(id,tenant_id,runtime_id,kind,status,requested_by,started_at) SELECT $1,t.id,t.runtime_id,'tenant_provision','running',$2,now() FROM tenants t WHERE t.id=$3")
        .bind(operation_id).bind(operator_id).bind(id).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO operation_events(operation_id,message) VALUES($1,'Provisioning retry queued.')")
        .bind(operation_id).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(StatusCode::ACCEPTED)
}

async fn provisioning_reconciler(state: Shared) {
    loop {
        if let Ok(ids) = sqlx::query_scalar::<_, Uuid>("SELECT id FROM tenants WHERE lifecycle_state='provisioning' ORDER BY created_at LIMIT 10")
            .fetch_all(&state.db).await {
            for id in ids { provision_tenant(&state, id).await; }
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}

async fn provisioning_step(state: &Shared, tenant_id: Uuid, step: &str) -> Result<bool> {
    let status = sqlx::query_scalar::<_, String>(
        "SELECT status FROM provisioning_steps WHERE tenant_id=$1 AND step=$2",
    )
    .bind(tenant_id)
    .bind(step)
    .fetch_one(&state.db)
    .await?;
    if status == "succeeded" {
        return Ok(false);
    }
    sqlx::query("UPDATE provisioning_steps SET status='running',attempts=attempts+1,error=NULL,started_at=now(),finished_at=NULL,updated_at=now() WHERE tenant_id=$1 AND step=$2")
        .bind(tenant_id).bind(step).execute(&state.db).await?;
    Ok(true)
}

async fn finish_provisioning_step(db: &PgPool, tenant_id: Uuid, step: &str) -> Result<()> {
    sqlx::query("UPDATE provisioning_steps SET status='succeeded',finished_at=now(),updated_at=now() WHERE tenant_id=$1 AND step=$2")
        .bind(tenant_id).bind(step).execute(db).await?;
    if let Some(operation_id) = sqlx::query_scalar::<_, Uuid>("SELECT id FROM operations WHERE tenant_id=$1 AND kind='tenant_provision' AND status='running' ORDER BY created_at DESC LIMIT 1")
        .bind(tenant_id).fetch_optional(db).await? {
        sqlx::query("INSERT INTO operation_events(operation_id,message) VALUES($1,$2)")
            .bind(operation_id).bind(format!("{step} completed.")).execute(db).await?;
    }
    Ok(())
}

async fn fail_provisioning(db: &PgPool, tenant_id: Uuid, step: &str) {
    let message = format!("{step} failed; retry is safe.");
    let _ = sqlx::query("UPDATE provisioning_steps SET status='failed',error=$1,finished_at=now(),updated_at=now() WHERE tenant_id=$2 AND step=$3")
        .bind(&message).bind(tenant_id).bind(step).execute(db).await;
    let _ = sqlx::query("UPDATE tenants SET lifecycle_state='failed',observed_state='unhealthy',observation_error=$1,observed_at=now(),updated_at=now() WHERE id=$2")
        .bind(&message).bind(tenant_id).execute(db).await;
    let _ = sqlx::query("UPDATE operations SET status='failed',error=$1,finished_at=now() WHERE id=(SELECT id FROM operations WHERE tenant_id=$2 AND kind='tenant_provision' AND status='running' ORDER BY created_at DESC LIMIT 1)")
        .bind(&message).bind(tenant_id).execute(db).await;
}

async fn provision_tenant(state: &Shared, tenant_id: Uuid) {
    for step in ["database", "migrate", "render", "reload", "health"] {
        let should_run = match provisioning_step(state, tenant_id, step).await {
            Ok(value) => value,
            Err(_) => return,
        };
        if !should_run {
            continue;
        }
        let result = match step {
            "database" => create_tenant_database(state, tenant_id).await,
            "migrate" => migrate_tenant_database(state, tenant_id).await,
            "render" => render_provisioned_tenant(state, tenant_id)
                .await
                .map(|_| ()),
            "reload" => reload_provisioned_runtime(state, tenant_id).await,
            "health" => healthcheck_provisioned_tenant(state, tenant_id).await,
            _ => unreachable!(),
        };
        if result.is_err() {
            fail_provisioning(&state.db, tenant_id, step).await;
            return;
        }
        if finish_provisioning_step(&state.db, tenant_id, step)
            .await
            .is_err()
        {
            return;
        }
    }
    let _ = sqlx::query("UPDATE tenants SET lifecycle_state='ready',observed_state='healthy',observed_at=now(),observation_error=NULL,updated_at=now() WHERE id=$1")
        .bind(tenant_id).execute(&state.db).await;
    let _ = sqlx::query(
        "UPDATE r2_buckets SET lifecycle_state='ready',updated_at=now() WHERE tenant_id=$1",
    )
    .bind(tenant_id)
    .execute(&state.db)
    .await;
    let _ = sqlx::query("UPDATE operations SET status='succeeded',finished_at=now() WHERE id=(SELECT id FROM operations WHERE tenant_id=$1 AND kind='tenant_provision' AND status='running' ORDER BY created_at DESC LIMIT 1)")
        .bind(tenant_id).execute(&state.db).await;
}

async fn create_tenant_database(state: &Shared, tenant_id: Uuid) -> Result<()> {
    let database_url = sqlx::query_scalar::<_, String>(
        "SELECT database_url FROM tenant_configuration WHERE tenant_id=$1",
    )
    .bind(tenant_id)
    .fetch_one(&state.db)
    .await?;
    let tenant_url = url::Url::parse(&database_url)?;
    let role = tenant_url.username();
    let password = tenant_url
        .password()
        .context("tenant database password missing")?;
    let database = tenant_url.path().trim_start_matches('/');
    anyhow::ensure!(
        role.starts_with("eunha_r_") && database.starts_with("eunha_t_"),
        "unsafe generated database name"
    );
    let mut admin_url = url::Url::parse(&state.provision_database_url)?;
    admin_url.set_path("/postgres");
    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect(admin_url.as_str())
        .await?;
    if !sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM pg_roles WHERE rolname=$1)")
        .bind(role)
        .fetch_one(&admin)
        .await?
    {
        sqlx::query(&format!("CREATE ROLE {role} LOGIN PASSWORD '{password}'"))
            .execute(&admin)
            .await?;
    } else {
        sqlx::query(&format!("ALTER ROLE {role} PASSWORD '{password}'"))
            .execute(&admin)
            .await?;
    }
    if !sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM pg_database WHERE datname=$1)")
        .bind(database)
        .fetch_one(&admin)
        .await?
    {
        sqlx::query(&format!("CREATE DATABASE {database} OWNER {role}"))
            .execute(&admin)
            .await?;
    }
    admin.close().await;
    Ok(())
}

async fn migrate_tenant_database(state: &Shared, tenant_id: Uuid) -> Result<()> {
    let (database_url, tenant_directory) = sqlx::query_as::<_, (String, String)>("SELECT c.database_url,r.tenant_directory FROM tenant_configuration c JOIN tenants t ON t.id=c.tenant_id JOIN runtimes r ON r.id=t.runtime_id WHERE c.tenant_id=$1")
        .bind(tenant_id).fetch_one(&state.db).await?;
    let runtime_root = PathBuf::from(tenant_directory)
        .parent()
        .context("runtime directory has no parent")?
        .to_path_buf();
    let binary = runtime_root.join("current/eunha");
    let status = tokio::process::Command::new(binary)
        .arg("migrate")
        .env("DATABASE_URL", database_url)
        .current_dir(runtime_root.join("current"))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await?;
    anyhow::ensure!(status.success(), "tenant migration failed");
    Ok(())
}

async fn render_provisioned_tenant(state: &Shared, tenant_id: Uuid) -> Result<()> {
    let row = sqlx::query_as::<_, (SqlJson<StoredTenant>, SqlJson<StoredConfig>, String, String, String, String, i64)>(
        "SELECT to_jsonb(t),to_jsonb(c),b.bucket_name,i.address,r.tenant_directory,r.bind_address,t.config_revision FROM tenants t JOIN tenant_configuration c ON c.tenant_id=t.id JOIN runtimes r ON r.id=t.runtime_id JOIN instances i ON i.id=r.instance_id JOIN r2_buckets b ON b.tenant_id=t.id WHERE t.id=$1",
    ).bind(tenant_id).fetch_one(&state.db).await?;
    let (SqlJson(tenant), SqlJson(config), bucket, runtime_host, directory, bind_address, revision) =
        row;
    let configured_host = std::env::var("CONTROL_RUNTIME_HOST").unwrap_or_default();
    anyhow::ensure!(
        !configured_host.is_empty() && configured_host == runtime_host,
        "runtime is managed elsewhere"
    );
    let domain = tenant.domain.clone();
    anyhow::ensure!(is_safe_domain(&domain), "invalid tenant domain");
    let rendered = toml::to_string_pretty(&typed_toml(tenant, config, bucket, bind_address)?)?;
    toml::from_str::<TenantToml>(&rendered)?;
    let directory = PathBuf::from(directory);
    anyhow::ensure!(
        tokio::fs::metadata(&directory).await?.is_dir(),
        "tenant directory unavailable"
    );
    let file_name = tenant_file_name(&domain);
    let target = directory.join(&file_name);
    let temporary = directory.join(format!(".{file_name}.{}", Uuid::new_v4()));
    tokio::fs::write(&temporary, rendered.as_bytes()).await?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o600)).await?;
    }
    if let Err(error) = tokio::fs::rename(&temporary, &target).await {
        let _ = tokio::fs::remove_file(&temporary).await;
        return Err(error.into());
    }
    let checksum = format!("{:x}", Sha256::digest(rendered.as_bytes()));
    sqlx::query(
        "UPDATE tenants SET rendered_revision=$1,rendered_checksum=$2,updated_at=now() WHERE id=$3",
    )
    .bind(revision)
    .bind(checksum)
    .bind(tenant_id)
    .execute(&state.db)
    .await?;
    Ok(())
}

async fn reload_provisioned_runtime(state: &Shared, tenant_id: Uuid) -> Result<()> {
    let service_label = sqlx::query_scalar::<_, String>(
        "SELECT r.service_label FROM tenants t JOIN runtimes r ON r.id=t.runtime_id WHERE t.id=$1",
    )
    .bind(tenant_id)
    .fetch_one(&state.db)
    .await?;
    anyhow::ensure!(
        is_safe_launchd_label(&service_label),
        "invalid service label"
    );
    let uid = tokio::process::Command::new("id")
        .arg("-u")
        .output()
        .await?;
    anyhow::ensure!(uid.status.success(), "cannot identify service account");
    let target = format!(
        "gui/{}/{}",
        String::from_utf8_lossy(&uid.stdout).trim(),
        service_label
    );
    let output = tokio::process::Command::new("launchctl")
        .args(["kill", "HUP", &target])
        .output()
        .await?;
    anyhow::ensure!(output.status.success(), "launchd reload failed");
    Ok(())
}

async fn healthcheck_provisioned_tenant(state: &Shared, tenant_id: Uuid) -> Result<()> {
    let (domain, bind_address) = sqlx::query_as::<_, (String, String)>("SELECT t.domain,r.bind_address FROM tenants t JOIN runtimes r ON r.id=t.runtime_id WHERE t.id=$1")
        .bind(tenant_id).fetch_one(&state.db).await?;
    let url = format!("http://{bind_address}/api/v2/instance");
    for _ in 0..30 {
        if state
            .client
            .get(&url)
            .header("Host", &domain)
            .send()
            .await
            .map(|response| response.status().is_success())
            .unwrap_or(false)
        {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    anyhow::bail!("tenant did not become healthy")
}
async fn operations(
    State(s): State<Shared>,
    headers: HeaderMap,
) -> Result<Json<Vec<OperationSummary>>, ApiError> {
    operator(&headers, &s.db).await?;
    Ok(Json(sqlx::query_as("SELECT o.id,t.domain,r.name runtime_name,o.kind,o.status,o.error,o.created_at FROM operations o LEFT JOIN tenants t ON t.id=o.tenant_id LEFT JOIN runtimes r ON r.id=o.runtime_id ORDER BY o.created_at DESC LIMIT 50").fetch_all(&s.db).await?))
}

/// Secrets are replaced before a configuration ever leaves the control plane.
const REDACTED: &str = "<redacted>";

/// Keeps the shape an operator needs to recognise a connection while masking
/// the password: `postgresql://eunha:<redacted>@127.0.0.1/eunha_seoul_earth`.
///
/// Anything this cannot parse with confidence is withheld outright rather than
/// echoed back, so an unencoded delimiter inside a password cannot leak it.
fn redact_connection_url(value: &str) -> String {
    let Some((scheme, rest)) = value.split_once("://") else {
        return REDACTED.to_owned();
    };
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(authority_end);
    // A credential may also ride in the query string; that shape is not parsed.
    if tail.to_ascii_lowercase().contains("password=") {
        return REDACTED.to_owned();
    }
    match authority.rsplit_once('@') {
        Some((userinfo, host)) => {
            let userinfo = match userinfo.split_once(':') {
                Some((user, _)) => format!("{user}:{REDACTED}"),
                None => userinfo.to_owned(),
            };
            format!("{scheme}://{userinfo}@{host}{tail}")
        }
        // An '@' past the authority means an unencoded delimiter split the
        // credentials; the parse is untrustworthy, so withhold the value.
        None if tail.contains('@') => REDACTED.to_owned(),
        None => format!("{scheme}://{authority}{tail}"),
    }
}

fn redact_tenant_toml(config: &mut TenantToml) {
    config.database_url = redact_connection_url(&config.database_url);
    config.redis_url = redact_connection_url(&config.redis_url);
    config.redis_coordination_url = config
        .redis_coordination_url
        .as_deref()
        .map(redact_connection_url);
    config.media_storage.access_key_id = REDACTED.to_owned();
    config.media_storage.secret_access_key = REDACTED.to_owned();
    config.resend.api_key = REDACTED.to_owned();
    if let Some(smtp) = config.smtp.as_mut() {
        smtp.password = REDACTED.to_owned();
    }
    if let Some(encryption) = config.active_record_encryption.as_mut() {
        encryption.primary_key = REDACTED.to_owned();
        encryption.key_derivation_salt = REDACTED.to_owned();
    }
    // The public half of the VAPID pair stays readable; the private half never leaves.
    config.instance.vapid_private_key = REDACTED.to_owned();
}

#[derive(Serialize)]
struct TenantConfiguration {
    domain: String,
    file_name: String,
    config_revision: i64,
    rendered_revision: Option<i64>,
    toml: String,
}

/// Renders exactly what a reconciliation would write, minus every secret, so
/// operators can read a tenant's effective configuration from the console.
async fn tenant_configuration(
    State(s): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<TenantConfiguration>, ApiError> {
    operator(&headers, &s.db).await?;
    let row = sqlx::query_as::<_, (SqlJson<StoredTenant>, SqlJson<StoredConfig>, String, String, i64, Option<i64>)>(
        "SELECT to_jsonb(t),to_jsonb(c),b.bucket_name,r.bind_address,t.config_revision,t.rendered_revision FROM tenants t JOIN tenant_configuration c ON c.tenant_id=t.id JOIN runtimes r ON r.id=t.runtime_id JOIN r2_buckets b ON b.tenant_id=t.id WHERE t.id=$1",
    )
    .bind(id)
    .fetch_optional(&s.db)
    .await?
    .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "Tenant not found".into()))?;
    let (
        SqlJson(tenant),
        SqlJson(config),
        bucket_name,
        bind_address,
        config_revision,
        rendered_revision,
    ) = row;
    let domain = tenant.domain.clone();
    let mut typed =
        typed_toml(tenant, config, bucket_name, bind_address).map_err(ApiError::from)?;
    redact_tenant_toml(&mut typed);
    let rendered = toml::to_string_pretty(&typed).map_err(ApiError::from)?;
    Ok(Json(TenantConfiguration {
        file_name: tenant_file_name(&domain),
        domain,
        config_revision,
        rendered_revision,
        toml: rendered,
    }))
}

async fn operation_events(
    State(s): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<OperationEvent>>, ApiError> {
    operator(&headers, &s.db).await?;
    Ok(Json(
        sqlx::query_as(
            "SELECT id,message,created_at FROM operation_events WHERE operation_id=$1 ORDER BY id",
        )
        .bind(id)
        .fetch_all(&s.db)
        .await?,
    ))
}

async fn runtimes(
    State(s): State<Shared>,
    headers: HeaderMap,
) -> Result<Json<Vec<RuntimeSummary>>, ApiError> {
    operator(&headers, &s.db).await?;
    Ok(Json(sqlx::query_as("SELECT r.id,r.name,i.name instance_name,i.address instance_address,r.bind_address,r.service_label,r.tenant_directory,(SELECT count(*) FROM tenants WHERE runtime_id=r.id) tenant_count,r.auto_deploy_enabled,r.deployed_ref,o.id release_operation_id,o.status release_status,e.message release_message FROM runtimes r JOIN instances i ON i.id=r.instance_id LEFT JOIN LATERAL (SELECT id,status FROM operations WHERE runtime_id=r.id AND kind='runtime_release' ORDER BY created_at DESC LIMIT 1) o ON true LEFT JOIN LATERAL (SELECT string_agg(message,E'\\n' ORDER BY id) message FROM operation_events WHERE operation_id=o.id) e ON true ORDER BY i.name,r.name").fetch_all(&s.db).await?))
}

async fn set_automatic_release(
    State(s): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(body): Json<AutomaticRelease>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let (operator_id, _) = operator(&headers, &s.db).await?;
    let changed = sqlx::query("UPDATE runtimes SET auto_deploy_enabled=$1 WHERE id=$2")
        .bind(body.enabled)
        .bind(id)
        .execute(&s.db)
        .await?
        .rows_affected();
    if changed == 0 {
        return Err(ApiError(StatusCode::NOT_FOUND, "Runtime not found.".into()));
    }
    sqlx::query("INSERT INTO operations(id,runtime_id,kind,status,requested_by,detail,finished_at) VALUES($1,$2,'release_policy','succeeded',$3,jsonb_build_object('auto_deploy_enabled',$4),now())")
        .bind(Uuid::new_v4()).bind(id).bind(operator_id).bind(body.enabled).execute(&s.db).await?;
    Ok(Json(
        serde_json::json!({"auto_deploy_enabled": body.enabled}),
    ))
}

async fn release_runtime(
    State(s): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let (operator_id, _) = operator(&headers, &s.db).await?;
    let operation_id = queue_runtime_release(s, id, Some(operator_id), "manual").await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(serde_json::json!({"operation_id": operation_id})),
    ))
}

async fn queue_runtime_release(
    state: Shared,
    runtime_id: Uuid,
    requested_by: Option<Uuid>,
    trigger: &str,
) -> Result<Uuid, ApiError> {
    let row = sqlx::query_as::<_, (String, String, String, String, String)>(
        "SELECT r.name,i.address,r.tenant_directory,r.service_label,r.bind_address FROM runtimes r JOIN instances i ON i.id=r.instance_id WHERE r.id=$1",
    )
    .bind(runtime_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "Runtime not found.".into()))?;
    if !is_safe_runtime_name(&row.0)
        || !is_safe_remote_host(&row.1)
        || !is_safe_absolute_path(FsPath::new(&row.2))
        || !is_safe_launchd_label(&row.3)
    {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "The stored runtime release target is invalid.".into(),
        ));
    }
    let operation_id = Uuid::new_v4();
    let inserted: Option<Uuid> = sqlx::query_scalar("INSERT INTO operations(id,runtime_id,kind,status,requested_by,detail,started_at) VALUES($1,$2,'runtime_release','running',$3,jsonb_build_object('trigger',$4),now()) ON CONFLICT DO NOTHING RETURNING id")
        .bind(operation_id).bind(runtime_id).bind(requested_by).bind(trigger).fetch_optional(&state.db).await?;
    let Some(operation_id) = inserted else {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "Another runtime release is already in progress.".into(),
        ));
    };
    let deployment_id = Uuid::new_v4();
    sqlx::query("INSERT INTO runtime_deployments(id,runtime_id,operation_id,trigger,status,started_at) VALUES($1,$2,$3,$4,'queued',now())")
        .bind(deployment_id).bind(runtime_id).bind(operation_id).bind(trigger).execute(&state.db).await?;
    tokio::spawn(run_runtime_release(
        state,
        operation_id,
        deployment_id,
        runtime_id,
        row,
    ));
    Ok(operation_id)
}

async fn run_runtime_release(
    state: Shared,
    operation_id: Uuid,
    deployment_id: Uuid,
    runtime_id: Uuid,
    runtime: (String, String, String, String, String),
) {
    let (_, host, tenant_directory, service_label, bind_address) = runtime;
    release_event(
        &state.db,
        operation_id,
        "Release queued; validating the central source checkout.",
    )
    .await;
    let _ = sqlx::query("UPDATE runtime_deployments SET status='building' WHERE id=$1")
        .bind(deployment_id)
        .execute(&state.db)
        .await;
    let result = match build_release(&state.db, operation_id).await {
        Ok(release) => {
            let release_id = Uuid::new_v4();
            let artifact_path = release.1.to_string_lossy().to_string();
            let release_id: Uuid = sqlx::query_scalar("INSERT INTO eunha_releases(id,git_sha,artifact_path,build_status,built_at) VALUES($1,$2,$3,'ready',now()) ON CONFLICT(git_sha) DO UPDATE SET artifact_path=excluded.artifact_path,build_status='ready',built_at=now() RETURNING id")
                .bind(release_id).bind(&release.0).bind(artifact_path).fetch_one(&state.db).await.unwrap_or(release_id);
            let _ = sqlx::query(
                "UPDATE runtime_deployments SET release_id=$1,status='transferring' WHERE id=$2",
            )
            .bind(release_id)
            .bind(deployment_id)
            .execute(&state.db)
            .await;
            async_deploy_release(
                &state.db,
                operation_id,
                &host,
                &tenant_directory,
                &service_label,
                &bind_address,
                release,
            )
            .await
        }
        Err(message) => Err(message),
    };
    match result {
        Ok(reference) => {
            release_event(
                &state.db,
                operation_id,
                "Runtime is healthy; recording the deployed release.",
            )
            .await;
            let _ = sqlx::query("UPDATE runtimes SET deployed_ref=COALESCE($1,deployed_ref),release_checked_at=now() WHERE id=$2")
                .bind(reference).bind(runtime_id).execute(&state.db).await;
            let _ = sqlx::query(
                "UPDATE operations SET status='succeeded',finished_at=now() WHERE id=$1",
            )
            .bind(operation_id)
            .execute(&state.db)
            .await;
            let _ = sqlx::query(
                "UPDATE runtime_deployments SET status='healthy',finished_at=now() WHERE id=$1",
            )
            .bind(deployment_id)
            .execute(&state.db)
            .await;
            release_event(&state.db, operation_id, "Release completed successfully.").await;
        }
        Err(message) => {
            release_event(
                &state.db,
                operation_id,
                "Release failed; the previous release was retained or restored.",
            )
            .await;
            let _ = sqlx::query("UPDATE runtime_deployments SET status='failed',error=$1,finished_at=now() WHERE id=$2").bind(&message).bind(deployment_id).execute(&state.db).await;
            let _ = fail_release(&state.db, operation_id, &message).await;
        }
    }
}

async fn fail_release(db: &PgPool, operation_id: Uuid, message: &str) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE operations SET status='failed',error=$1,finished_at=now() WHERE id=$2")
        .bind(message)
        .bind(operation_id)
        .execute(db)
        .await?;
    Ok(())
}

async fn release_event(db: &PgPool, operation_id: Uuid, message: &str) {
    let _ = sqlx::query("INSERT INTO operation_events(operation_id,message) VALUES($1,$2)")
        .bind(operation_id)
        .bind(message)
        .execute(db)
        .await;
}

fn central_paths() -> Result<(PathBuf, PathBuf), String> {
    let exe = std::env::current_exe()
        .map_err(|_| "Control-plane release directory is unavailable.".to_string())?;
    let root = exe
        .parent()
        .and_then(FsPath::parent)
        .and_then(FsPath::parent)
        .ok_or_else(|| "Control-plane release directory is unavailable.".to_string())?;
    let source = root
        .parent()
        .ok_or_else(|| "Central Eunha source directory is unavailable.".to_string())?
        .join("eunha");
    if !source.join("Cargo.toml").is_file() {
        return Err("Central Eunha source checkout is unavailable.".into());
    }
    Ok((root.to_path_buf(), source))
}

async fn build_release(db: &PgPool, operation_id: Uuid) -> Result<(String, PathBuf), String> {
    let (root, source) = central_paths()?;
    let source_text = source.to_string_lossy().to_string();
    let status = tokio::process::Command::new("git")
        .args([
            "-C",
            &source_text,
            "status",
            "--porcelain",
            "--untracked-files=no",
        ])
        .stdin(Stdio::null())
        .output()
        .await
        .map_err(|_| "Could not inspect the central Eunha checkout.".to_string())?;
    if !status.status.success() || !status.stdout.is_empty() {
        return Err(
            "Central Eunha checkout has uncommitted changes; releases require a commit.".into(),
        );
    }
    release_event(
        db,
        operation_id,
        "Fetching the latest central Eunha source.",
    )
    .await;
    command_status(
        "git",
        &["-C", &source_text, "fetch", "--prune", "--tags", "origin"],
        None,
    )
    .await?;
    let output = tokio::process::Command::new("git")
        .args(["-C", &source_text, "rev-parse", "origin/main"])
        .stdin(Stdio::null())
        .output()
        .await
        .map_err(|_| "Could not resolve the central Eunha release.".to_string())?;
    if !output.status.success() {
        return Err("Could not resolve the central Eunha release.".into());
    }
    let reference = String::from_utf8(output.stdout)
        .map_err(|_| "Could not read the central Eunha release.".to_string())?
        .trim()
        .to_owned();
    if !is_git_sha(&reference) {
        return Err("Central Eunha release is invalid.".into());
    }
    let artifact = root.join("releases").join(&reference);
    if artifact.join("eunha").is_file() && artifact.join("frontend/dist/index.html").is_file() {
        release_event(
            db,
            operation_id,
            "A verified central build already exists; reusing it.",
        )
        .await;
        return Ok((reference, artifact));
    }
    release_event(
        db,
        operation_id,
        "Building the Eunha frontend and release binary centrally.",
    )
    .await;
    command_status(
        "git",
        &[
            "-C",
            &source_text,
            "checkout",
            "--detach",
            "--quiet",
            &reference,
        ],
        None,
    )
    .await?;
    command_status(
        "mise",
        &[
            "exec",
            "--",
            "pnpm",
            "--dir",
            "frontend",
            "install",
            "--frozen-lockfile",
        ],
        Some(&source),
    )
    .await?;
    command_status(
        "mise",
        &["exec", "--", "pnpm", "--dir", "frontend", "run", "build"],
        Some(&source),
    )
    .await?;
    command_status(
        "mise",
        &[
            "exec",
            "--",
            "cargo",
            "build",
            "--release",
            "--bin",
            "eunha",
        ],
        Some(&source),
    )
    .await?;
    let staging = root.join("releases").join(format!(".{reference}.staging"));
    let _ = tokio::fs::remove_dir_all(&staging).await;
    tokio::fs::create_dir_all(staging.join("frontend"))
        .await
        .map_err(|_| "Could not stage the central Eunha release.".to_string())?;
    tokio::fs::copy(source.join("target/release/eunha"), staging.join("eunha"))
        .await
        .map_err(|_| "Could not stage the central Eunha binary.".to_string())?;
    command_status(
        "cp",
        &[
            "-R",
            "frontend/dist",
            &staging.join("frontend").to_string_lossy(),
        ],
        Some(&source),
    )
    .await?;
    tokio::fs::rename(&staging, &artifact)
        .await
        .map_err(|_| "Could not publish the central Eunha release.".to_string())?;
    Ok((reference, artifact))
}

async fn command_status(program: &str, args: &[&str], cwd: Option<&FsPath>) -> Result<(), String> {
    let executable = match program {
        "mise" => "/opt/homebrew/bin/mise",
        "git" => "/usr/bin/git",
        "cp" => "/bin/cp",
        "ssh" | "scp" => "/usr/bin/ssh",
        _ => program,
    };
    let executable = if program == "scp" {
        "/usr/bin/scp"
    } else {
        executable
    };
    let mut command = tokio::process::Command::new(executable);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if program == "mise" && args.contains(&"cargo") {
        command.env("SQLX_OFFLINE", "true");
    }
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    match command.status().await {
        Ok(status) if status.success() => Ok(()),
        _ => Err("Central release build failed.".into()),
    }
}

async fn async_deploy_release(
    db: &PgPool,
    operation_id: Uuid,
    host: &str,
    tenant_directory: &str,
    service_label: &str,
    bind_address: &str,
    release: (String, PathBuf),
) -> Result<String, String> {
    let (reference, artifact) = release;
    let tenant = FsPath::new(tenant_directory);
    let runtime_root = tenant
        .parent()
        .ok_or_else(|| "Runtime release target is invalid.".to_string())?;
    let releases = runtime_root.join("releases");
    let staging = releases.join(format!(".{reference}.staging"));
    let release_path = releases.join(&reference);
    let artifact_text = artifact.to_string_lossy().to_string();
    let remote_staging = shell_quote(&staging.to_string_lossy());
    let local_runtime = matches!(host, "localhost" | "127.0.0.1" | "::1");
    release_event(db, operation_id, "Preparing the runtime release directory.").await;
    if local_runtime {
        tokio::fs::create_dir_all(&releases)
            .await
            .map_err(|_| "Could not prepare the runtime release directory.".to_string())?;
    } else {
        command_status(
            "ssh",
            &[
                host,
                &format!("mkdir -p {}", shell_quote(&releases.to_string_lossy())),
            ],
            None,
        )
        .await
        .map_err(|_| "Could not prepare the runtime release directory.".to_string())?;
    }
    release_event(
        db,
        operation_id,
        "Transferring the immutable release bundle to the runtime.",
    )
    .await;
    if local_runtime {
        command_status(
            "cp",
            &["-R", &artifact_text, &staging.to_string_lossy()],
            None,
        )
        .await
        .map_err(|_| "Could not transfer the runtime release bundle.".to_string())?;
    } else {
        command_status(
            "scp",
            &[
                "-r",
                &artifact_text,
                &format!("{}:{}", host, remote_staging),
            ],
            None,
        )
        .await
        .map_err(|_| "Could not transfer the runtime release bundle.".to_string())?;
    }
    release_event(
        db,
        operation_id,
        "Applying migrations, switching the runtime, and waiting for health.",
    )
    .await;
    let script = format!(
        "set -eu; rel={}; stage={}; root={}; cd \"$root\"; \"$stage/eunha\" --tenants {} migrate; mv \"$stage\" \"$rel\"; previous=$(readlink current 2>/dev/null || true); ln -sfn \"$rel\" .current.new; mv -h .current.new current; launchctl kickstart -k gui/$(id -u)/{}; for i in $(seq 1 45); do curl -sf -m 5 -o /dev/null http://127.0.0.1:{}/api/v2/instance && exit 0; sleep 1; done; if [ -n \"$previous\" ]; then ln -sfn \"$previous\" .current.new; mv -h .current.new current; launchctl kickstart -k gui/$(id -u)/{}; fi; exit 1",
        shell_quote(&release_path.to_string_lossy()),
        remote_staging,
        shell_quote(&runtime_root.to_string_lossy()),
        shell_quote(tenant_directory),
        service_label,
        bind_address.rsplit(':').next().unwrap_or(""),
        service_label
    );
    if local_runtime {
        command_status("/bin/sh", &["-c", &script], None).await.map_err(|_| "Runtime migration, switch, or health check failed; the prior release was restored when available.".to_string())?;
    } else {
        command_status("ssh", &[host, &script], None).await.map_err(|_| "Runtime migration, switch, or health check failed; the prior release was restored when available.".to_string())?;
    }
    Ok(reference)
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\\"'\\\"'"))
}

async fn automatic_release_reconciler(state: Shared) {
    let interval = 300;
    loop {
        if let Some(reference) = latest_remote_ref().await {
            let rows = sqlx::query_scalar::<_, Uuid>("SELECT id FROM runtimes WHERE auto_deploy_enabled AND deployed_ref IS DISTINCT FROM $1 ORDER BY name LIMIT 1")
                .bind(&reference).fetch_all(&state.db).await;
            if let Ok(rows) = rows
                && let Some(runtime_id) = rows.into_iter().next()
            {
                let _ = queue_runtime_release(state.clone(), runtime_id, None, "automatic").await;
            }
        }
        tokio::time::sleep(Duration::from_secs(interval)).await;
    }
}

async fn latest_remote_ref() -> Option<String> {
    let (_, source) = central_paths().ok()?;
    let source = source.to_string_lossy().to_string();
    let output = tokio::process::Command::new("git")
        .args(["-C", &source, "ls-remote", "origin", "refs/heads/main"])
        .stdin(Stdio::null())
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let reference = String::from_utf8(output.stdout)
        .ok()?
        .split_whitespace()
        .next()?
        .to_owned();
    is_git_sha(&reference).then_some(reference)
}
async fn check_tenant(
    State(s): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let (operator_id, _) = operator(&headers, &s.db).await?;
    let row = sqlx::query_as::<_, (String,String)>("SELECT t.domain,r.bind_address FROM tenants t JOIN runtimes r ON r.id=t.runtime_id WHERE t.id=$1").bind(id).fetch_optional(&s.db).await?.ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "Tenant not found".into()))?;
    let op = Uuid::new_v4();
    sqlx::query("INSERT INTO operations(id,tenant_id,kind,status,requested_by,started_at) VALUES($1,$2,'health_check','running',$3,now())").bind(op).bind(id).bind(operator_id).execute(&s.db).await?;
    let url = format!("http://{}/api/v2/instance", row.1);
    let healthy = s
        .client
        .get(url)
        .header("Host", &row.0)
        .send()
        .await
        .map(|r| r.status().is_success())
        .unwrap_or(false);
    let (observed, error) = if healthy {
        ("healthy", None)
    } else {
        (
            "unhealthy",
            Some("Health endpoint was unreachable or returned an unsuccessful status."),
        )
    };
    sqlx::query(
        "UPDATE tenants SET observed_state=$1,observed_at=now(),observation_error=$2 WHERE id=$3",
    )
    .bind(observed)
    .bind(error)
    .bind(id)
    .execute(&s.db)
    .await?;
    sqlx::query("UPDATE operations SET status='succeeded',detail=jsonb_build_object('healthy',$1),finished_at=now() WHERE id=$2").bind(healthy).bind(op).execute(&s.db).await?;
    Ok(Json(serde_json::json!({"healthy": healthy})))
}

async fn health_reconciler(state: Shared) {
    loop {
        let _ = sqlx::query("DELETE FROM sessions WHERE expires_at <= now()")
            .execute(&state.db)
            .await;
        let _ = sqlx::query("UPDATE tenants SET observed_state='disabled',observed_at=now(),observation_error=NULL WHERE desired_state='disabled' AND observed_state IS DISTINCT FROM 'disabled'")
            .execute(&state.db)
            .await;
        let rows = sqlx::query_as::<_, (Uuid, String, String)>(
            "SELECT t.id,t.domain,r.bind_address FROM tenants t JOIN runtimes r ON r.id=t.runtime_id WHERE t.desired_state='running'",
        )
        .fetch_all(&state.db)
        .await;
        if let Ok(rows) = rows {
            for (id, domain, bind_address) in rows {
                let result = state
                    .client
                    .get(format!("http://{bind_address}/api/v2/instance"))
                    .header("Host", domain)
                    .send()
                    .await;
                let (observed, error) = match result {
                    Ok(response) if response.status().is_success() => ("healthy", None),
                    Ok(response) => (
                        "unhealthy",
                        Some(format!("Health endpoint returned {}", response.status())),
                    ),
                    Err(_) => (
                        "unhealthy",
                        Some("Health endpoint was unreachable.".to_string()),
                    ),
                };
                let _ = sqlx::query("UPDATE tenants SET observed_state=$1,observed_at=now(),observation_error=$2 WHERE id=$3")
                    .bind(observed).bind(error).bind(id).execute(&state.db).await;
            }
        }
        tokio::time::sleep(Duration::from_secs(60)).await;
    }
}

fn is_safe_domain(domain: &str) -> bool {
    !domain.is_empty()
        && domain.len() <= 253
        && domain
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-'))
}

fn is_safe_launchd_label(label: &str) -> bool {
    !label.is_empty()
        && label
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
}

fn is_safe_runtime_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
}

fn is_safe_remote_host(host: &str) -> bool {
    !host.is_empty()
        && host.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_' | b':' | b'@')
        })
}

fn is_safe_absolute_path(path: &FsPath) -> bool {
    path.is_absolute()
        && path
            .components()
            .all(|component| !matches!(component, Component::ParentDir))
}

fn is_git_sha(value: &str) -> bool {
    value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn is_safe_tenant_file_name(file_name: &str) -> bool {
    file_name.ends_with(".toml")
        && file_name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-'))
}

fn tenant_file_name(domain: &str) -> String {
    format!("{}.toml", domain.replace('.', "-"))
}

async fn mark_operation_failed(db: &PgPool, id: Uuid, message: &str) {
    let _ =
        sqlx::query("UPDATE operations SET status='failed',error=$1,finished_at=now() WHERE id=$2")
            .bind(message)
            .bind(id)
            .execute(db)
            .await;
}

async fn render_tenant(
    State(s): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let (operator_id, _) = operator(&headers, &s.db).await?;
    let row = sqlx::query_as::<_, (SqlJson<StoredTenant>, SqlJson<StoredConfig>, String, String, String, String, String, i64)>(
        "SELECT to_jsonb(t),to_jsonb(c),b.bucket_name,i.address,r.tenant_directory,r.service_label,r.bind_address,t.config_revision FROM tenants t JOIN tenant_configuration c ON c.tenant_id=t.id JOIN runtimes r ON r.id=t.runtime_id JOIN instances i ON i.id=r.instance_id JOIN r2_buckets b ON b.tenant_id=t.id WHERE t.id=$1",
    )
    .bind(id)
    .fetch_optional(&s.db)
    .await?
    .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "Tenant not found".into()))?;
    let (
        SqlJson(tenant),
        SqlJson(config),
        bucket_name,
        runtime_host,
        tenant_directory,
        service_label,
        bind_address,
        revision,
    ) = row;
    let domain = tenant.domain.clone();
    let tenant_file_name = tenant_file_name(&domain);
    let op = Uuid::new_v4();
    sqlx::query("INSERT INTO operations(id,tenant_id,kind,status,requested_by,started_at) VALUES($1,$2,'render','running',$3,now())")
        .bind(op).bind(id).bind(operator_id).execute(&s.db).await?;

    let configured_host = std::env::var("CONTROL_RUNTIME_HOST").unwrap_or_default();
    if configured_host.is_empty() || configured_host != runtime_host {
        mark_operation_failed(
            &s.db,
            op,
            "This control plane is not the configured runtime host.",
        )
        .await;
        return Err(ApiError(
            StatusCode::CONFLICT,
            "This runtime is managed by a different control-plane host.".into(),
        ));
    }
    if !is_safe_domain(&domain)
        || !is_safe_launchd_label(&service_label)
        || !is_safe_tenant_file_name(&tenant_file_name)
    {
        mark_operation_failed(&s.db, op, "The stored runtime target is invalid.").await;
        return Err(ApiError(
            StatusCode::CONFLICT,
            "The stored runtime target is invalid.".into(),
        ));
    }
    let typed =
        typed_toml(tenant, config, bucket_name, bind_address.clone()).map_err(ApiError::from)?;
    let rendered_config = toml::to_string_pretty(&typed).map_err(ApiError::from)?;
    toml::from_str::<TenantToml>(&rendered_config).map_err(ApiError::from)?;

    let directory = PathBuf::from(tenant_directory);
    if tokio::fs::metadata(&directory)
        .await
        .map(|meta| !meta.is_dir())
        .unwrap_or(true)
    {
        mark_operation_failed(&s.db, op, "The tenant directory is unavailable.").await;
        return Err(ApiError(
            StatusCode::CONFLICT,
            "The tenant directory is unavailable.".into(),
        ));
    }
    let target = directory.join(&tenant_file_name);
    let temporary = directory.join(format!(".{tenant_file_name}.{}", Uuid::new_v4()));
    if tokio::fs::write(&temporary, rendered_config.as_bytes())
        .await
        .is_err()
    {
        mark_operation_failed(&s.db, op, "The tenant configuration could not be written.").await;
        return Err(ApiError(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Could not render the tenant configuration.".into(),
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if tokio::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o600))
            .await
            .is_err()
        {
            let _ = tokio::fs::remove_file(&temporary).await;
            mark_operation_failed(
                &s.db,
                op,
                "The tenant configuration permissions could not be set.",
            )
            .await;
            return Err(ApiError(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Could not secure the rendered configuration.".into(),
            ));
        }
    }
    if tokio::fs::rename(&temporary, &target).await.is_err() {
        let _ = tokio::fs::remove_file(&temporary).await;
        mark_operation_failed(
            &s.db,
            op,
            "The tenant configuration could not be activated.",
        )
        .await;
        return Err(ApiError(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Could not activate the rendered configuration.".into(),
        ));
    }
    let uid = match tokio::process::Command::new("id").arg("-u").output().await {
        Ok(output) if output.status.success() => {
            String::from_utf8_lossy(&output.stdout).trim().to_owned()
        }
        _ => {
            mark_operation_failed(
                &s.db,
                op,
                "The service account could not be identified after rendering.",
            )
            .await;
            return Err(ApiError(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Configuration rendered, but the service could not be restarted.".into(),
            ));
        }
    };
    let launch_target = format!("gui/{uid}/{service_label}");
    if !matches!(tokio::process::Command::new("launchctl").args(["kickstart", "-k", &launch_target]).output().await, Ok(output) if output.status.success())
    {
        mark_operation_failed(
            &s.db,
            op,
            "Configuration rendered, but launchd could not restart the instance.",
        )
        .await;
        return Err(ApiError(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Configuration rendered, but the instance could not be restarted.".into(),
        ));
    }
    let health_url = format!("http://{bind_address}/api/v2/instance");
    let mut healthy = false;
    for _ in 0..20 {
        if s.client
            .get(&health_url)
            .header("Host", &domain)
            .send()
            .await
            .map(|response| response.status().is_success())
            .unwrap_or(false)
        {
            healthy = true;
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    if !healthy {
        sqlx::query("UPDATE tenants SET observed_state='unhealthy',observed_at=now(),observation_error='Tenant did not become healthy after restart.',updated_at=now() WHERE id=$1")
            .bind(id).execute(&s.db).await?;
        mark_operation_failed(
            &s.db,
            op,
            "Configuration rendered and restarted, but the tenant did not become healthy.",
        )
        .await;
        return Err(ApiError(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Configuration rendered, but the tenant did not become healthy after restart.".into(),
        ));
    }
    let checksum = format!("{:x}", Sha256::digest(rendered_config.as_bytes()));
    sqlx::query("UPDATE tenants SET rendered_revision=$1,rendered_checksum=$2,observed_state='healthy',observed_at=now(),observation_error=NULL,updated_at=now() WHERE id=$3")
        .bind(revision).bind(checksum).bind(id).execute(&s.db).await?;
    sqlx::query("UPDATE operations SET status='succeeded',detail=jsonb_build_object('revision',$1),finished_at=now() WHERE id=$2")
        .bind(revision).bind(op).execute(&s.db).await?;
    Ok(Json(
        serde_json::json!({"rendered": true, "revision": revision}),
    ))
}

#[cfg(test)]
mod tests {
    use super::{REDACTED, redact_connection_url};

    #[test]
    fn masks_a_password_but_keeps_the_connection_recognisable() {
        assert_eq!(
            redact_connection_url("postgresql://eunha:s3cret@127.0.0.1/eunha_seoul_earth"),
            format!("postgresql://eunha:{REDACTED}@127.0.0.1/eunha_seoul_earth")
        );
        assert_eq!(
            redact_connection_url("postgresql://eunha:s3cret@db:5432/eunha?sslmode=require"),
            format!("postgresql://eunha:{REDACTED}@db:5432/eunha?sslmode=require")
        );
    }

    #[test]
    fn leaves_credential_free_urls_alone() {
        for url in [
            "postgresql://limeburst@localhost/eunha_seoul_earth",
            "redis://127.0.0.1:6379/3",
            "redis://127.0.0.1:6379",
        ] {
            assert_eq!(redact_connection_url(url), url);
        }
    }

    #[test]
    fn withholds_anything_it_cannot_parse_confidently() {
        // An unencoded delimiter inside the password ends the authority early.
        assert_eq!(
            redact_connection_url("postgresql://eunha:pa/ss@127.0.0.1/eunha"),
            REDACTED
        );
        assert_eq!(
            redact_connection_url("postgresql://eunha:pa?ss@127.0.0.1/eunha"),
            REDACTED
        );
        assert_eq!(
            redact_connection_url("redis://127.0.0.1:6379?password=s3cret"),
            REDACTED
        );
        assert_eq!(redact_connection_url("not-a-url"), REDACTED);
    }
}
