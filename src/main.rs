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
use clap::{Parser, Subcommand};
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
    bind_address: String,
    auto_deploy_enabled: bool,
    deployed_ref: Option<String>,
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
        client: reqwest::Client::builder()
            .timeout(Duration::from_secs(4))
            .build()?,
    });
    tokio::spawn(health_reconciler(state.clone()));
    tokio::spawn(automatic_release_reconciler(state.clone()));
    let api = Router::new()
        .route("/session", post(login).delete(logout))
        .route("/session/me", get(me))
        .route("/tenants", get(tenants))
        .route("/tenants/{id}/check", post(check_tenant))
        .route("/tenants/{id}/render", post(render_tenant))
        .route("/runtimes", get(runtimes))
        .route("/runtimes/{id}/release", post(release_runtime))
        .route(
            "/runtimes/{id}/automatic-release",
            post(set_automatic_release),
        )
        .route("/operations/{id}/events", get(operation_events))
        .route("/operations", get(operations));
    let frontend = ServeDir::new("frontend/dist")
        .not_found_service(ServeFile::new("frontend/dist/index.html"));
    let app = Router::new()
        .nest("/api", api)
        .fallback_service(frontend)
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
    Ok(Json(sqlx::query_as("SELECT t.id,t.domain,t.title,t.desired_state,CASE WHEN t.observed_at IS NULL OR t.observed_at < now()-interval '90 seconds' THEN 'unknown' ELSE t.observed_state END observed_state,t.config_revision,t.rendered_revision,r.name runtime_name,r.bind_address,t.updated_at FROM tenants t JOIN runtimes r ON r.id=t.runtime_id ORDER BY t.domain").fetch_all(&s.db).await?))
}
async fn operations(
    State(s): State<Shared>,
    headers: HeaderMap,
) -> Result<Json<Vec<OperationSummary>>, ApiError> {
    operator(&headers, &s.db).await?;
    Ok(Json(sqlx::query_as("SELECT o.id,t.domain,r.name runtime_name,o.kind,o.status,o.error,o.created_at FROM operations o LEFT JOIN tenants t ON t.id=o.tenant_id LEFT JOIN runtimes r ON r.id=o.runtime_id ORDER BY o.created_at DESC LIMIT 50").fetch_all(&s.db).await?))
}

async fn operation_events(
    State(s): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<OperationEvent>>, ApiError> {
    operator(&headers, &s.db).await?;
    Ok(Json(sqlx::query_as("SELECT id,message,created_at FROM operation_events WHERE operation_id=$1 ORDER BY id")
        .bind(id).fetch_all(&s.db).await?))
}

async fn runtimes(
    State(s): State<Shared>,
    headers: HeaderMap,
) -> Result<Json<Vec<RuntimeSummary>>, ApiError> {
    operator(&headers, &s.db).await?;
    Ok(Json(sqlx::query_as("SELECT id,name,bind_address,auto_deploy_enabled,deployed_ref FROM runtimes ORDER BY name").fetch_all(&s.db).await?))
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
        "SELECT name,host,tenant_directory,service_label,bind_address FROM runtimes WHERE id=$1",
    )
    .bind(runtime_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "Runtime not found.".into()))?;
    if !is_safe_runtime_name(&row.0) || !is_safe_remote_host(&row.1)
        || !is_safe_absolute_path(FsPath::new(&row.2)) || !is_safe_launchd_label(&row.3) {
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
    tokio::spawn(run_runtime_release(state, operation_id, runtime_id, row));
    Ok(operation_id)
}

async fn run_runtime_release(
    state: Shared,
    operation_id: Uuid,
    runtime_id: Uuid,
    runtime: (String, String, String, String, String),
) {
    let (_, host, tenant_directory, service_label, bind_address) = runtime;
    release_event(&state.db, operation_id, "Release queued; validating the central source checkout.").await;
    let result = match build_release(&state.db, operation_id).await {
        Ok(release) => async_deploy_release(&state.db, operation_id, &host, &tenant_directory, &service_label, &bind_address, release).await,
        Err(message) => Err(message),
    };
    match result {
        Ok(reference) => {
            release_event(&state.db, operation_id, "Runtime is healthy; recording the deployed release.").await;
            let _ = sqlx::query("UPDATE runtimes SET deployed_ref=COALESCE($1,deployed_ref),release_checked_at=now() WHERE id=$2")
                .bind(reference).bind(runtime_id).execute(&state.db).await;
            let _ = sqlx::query(
                "UPDATE operations SET status='succeeded',finished_at=now() WHERE id=$1",
            )
            .bind(operation_id)
            .execute(&state.db)
            .await;
            release_event(&state.db, operation_id, "Release completed successfully.").await;
        }
        Err(message) => { release_event(&state.db, operation_id, "Release failed; the previous release was retained or restored.").await; let _ = fail_release(&state.db, operation_id, &message).await; }
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
        .bind(operation_id).bind(message).execute(db).await;
}

fn central_paths() -> Result<(PathBuf, PathBuf), String> {
    let exe = std::env::current_exe().map_err(|_| "Control-plane release directory is unavailable.".to_string())?;
    let root = exe.parent().and_then(FsPath::parent).and_then(FsPath::parent)
        .ok_or_else(|| "Control-plane release directory is unavailable.".to_string())?;
    let source = root.parent().ok_or_else(|| "Central Eunha source directory is unavailable.".to_string())?.join("eunha");
    if !source.join("Cargo.toml").is_file() { return Err("Central Eunha source checkout is unavailable.".into()); }
    Ok((root.to_path_buf(), source))
}

async fn build_release(db: &PgPool, operation_id: Uuid) -> Result<(String, PathBuf), String> {
    let (root, source) = central_paths()?;
    let source_text = source.to_string_lossy().to_string();
    let status = tokio::process::Command::new("git")
        .args(["-C", &source_text, "status", "--porcelain", "--untracked-files=no"])
        .stdin(Stdio::null()).output().await
        .map_err(|_| "Could not inspect the central Eunha checkout.".to_string())?;
    if !status.status.success() || !status.stdout.is_empty() {
        return Err("Central Eunha checkout has uncommitted changes; releases require a commit.".into());
    }
    release_event(db, operation_id, "Fetching the latest central Eunha source.").await;
    command_status("git", &["-C", &source_text, "fetch", "--prune", "--tags", "origin"], None).await?;
    let output = tokio::process::Command::new("git")
        .args(["-C", &source_text, "rev-parse", "origin/main"])
        .stdin(Stdio::null())
        .output()
        .await
        .map_err(|_| "Could not resolve the central Eunha release.".to_string())?;
    if !output.status.success() { return Err("Could not resolve the central Eunha release.".into()); }
    let reference = String::from_utf8(output.stdout).map_err(|_| "Could not read the central Eunha release.".to_string())?.trim().to_owned();
    if !is_git_sha(&reference) { return Err("Central Eunha release is invalid.".into()); }
    let artifact = root.join("releases").join(&reference);
    if artifact.join("eunha").is_file() && artifact.join("frontend/dist/index.html").is_file() { release_event(db, operation_id, "A verified central build already exists; reusing it.").await; return Ok((reference, artifact)); }
    release_event(db, operation_id, "Building the Eunha frontend and release binary centrally.").await;
    command_status("git", &["-C", &source_text, "checkout", "--detach", "--quiet", &reference], None).await?;
    command_status("mise", &["exec", "--", "pnpm", "--dir", "frontend", "install", "--frozen-lockfile"], Some(&source)).await?;
    command_status("mise", &["exec", "--", "pnpm", "--dir", "frontend", "run", "build"], Some(&source)).await?;
    command_status("mise", &["exec", "--", "cargo", "build", "--release", "--bin", "eunha"], Some(&source)).await?;
    let staging = root.join("releases").join(format!(".{reference}.staging"));
    let _ = tokio::fs::remove_dir_all(&staging).await;
    tokio::fs::create_dir_all(staging.join("frontend")).await.map_err(|_| "Could not stage the central Eunha release.".to_string())?;
    tokio::fs::copy(source.join("target/release/eunha"), staging.join("eunha")).await.map_err(|_| "Could not stage the central Eunha binary.".to_string())?;
    command_status("cp", &["-R", "frontend/dist", &staging.join("frontend").to_string_lossy()], Some(&source)).await?;
    tokio::fs::rename(&staging, &artifact).await.map_err(|_| "Could not publish the central Eunha release.".to_string())?;
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
    let executable = if program == "scp" { "/usr/bin/scp" } else { executable };
    let mut command = tokio::process::Command::new(executable);
    command.args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    if let Some(cwd) = cwd { command.current_dir(cwd); }
    match command.status().await { Ok(status) if status.success() => Ok(()), _ => Err("Central release build failed.".into()) }
}

async fn async_deploy_release(db: &PgPool, operation_id: Uuid, host: &str, tenant_directory: &str, service_label: &str, bind_address: &str, release: (String, PathBuf)) -> Result<String, String> {
    let (reference, artifact) = release;
    let tenant = FsPath::new(tenant_directory);
    let runtime_root = tenant.parent().ok_or_else(|| "Runtime release target is invalid.".to_string())?;
    let releases = runtime_root.join("releases");
    let staging = releases.join(format!(".{reference}.staging"));
    let release_path = releases.join(&reference);
    let artifact_text = artifact.to_string_lossy().to_string();
    let remote_staging = shell_quote(&staging.to_string_lossy());
    release_event(db, operation_id, "Preparing the runtime release directory.").await;
    command_status("ssh", &[host, &format!("mkdir -p {}", shell_quote(&releases.to_string_lossy()))], None).await.map_err(|_| "Could not prepare the runtime release directory.".to_string())?;
    release_event(db, operation_id, "Transferring the immutable release bundle to the runtime.").await;
    command_status("scp", &["-r", &artifact_text, &format!("{}:{}", host, remote_staging)], None).await.map_err(|_| "Could not transfer the runtime release bundle.".to_string())?;
    release_event(db, operation_id, "Applying migrations, switching the runtime, and waiting for health.").await;
    let script = format!("set -eu; rel={}; stage={}; root={}; cd \"$root\"; \"$stage/eunha\" --tenants {} migrate; mv \"$stage\" \"$rel\"; previous=$(readlink current 2>/dev/null || true); ln -sfn \"$rel\" .current.new; mv -h .current.new current; launchctl kickstart -k gui/$(id -u)/{}; for i in $(seq 1 45); do curl -sf -m 5 -o /dev/null http://127.0.0.1:{}/api/v2/instance && exit 0; sleep 1; done; if [ -n \"$previous\" ]; then ln -sfn \"$previous\" .current.new; mv -h .current.new current; launchctl kickstart -k gui/$(id -u)/{}; fi; exit 1", shell_quote(&release_path.to_string_lossy()), remote_staging, shell_quote(&runtime_root.to_string_lossy()), shell_quote(tenant_directory), service_label, bind_address.rsplit(':').next().unwrap_or(""), service_label);
    command_status("ssh", &[host, &script], None).await.map_err(|_| "Runtime migration, switch, or health check failed; the prior release was restored when available.".to_string())?;
    Ok(reference)
}

fn shell_quote(value: &str) -> String { format!("'{}'", value.replace('\'', "'\\\"'\\\"'")) }

async fn automatic_release_reconciler(state: Shared) {
    let interval = 300;
    loop {
        if let Some(reference) = latest_remote_ref().await {
            let rows = sqlx::query_scalar::<_, Uuid>("SELECT id FROM runtimes WHERE auto_deploy_enabled AND deployed_ref IS DISTINCT FROM $1 ORDER BY name LIMIT 1")
                .bind(&reference).fetch_all(&state.db).await;
            if let Ok(rows) = rows {
                if let Some(runtime_id) = rows.into_iter().next() {
                    let _ =
                        queue_runtime_release(state.clone(), runtime_id, None, "automatic").await;
                }
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
    !host.is_empty() && host.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_' | b':' | b'@'))
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
        "SELECT to_jsonb(t),to_jsonb(c),b.bucket_name,r.host,r.tenant_directory,r.service_label,r.bind_address,t.config_revision FROM tenants t JOIN tenant_configuration c ON c.tenant_id=t.id JOIN runtimes r ON r.id=t.runtime_id JOIN r2_buckets b ON b.tenant_id=t.id WHERE t.id=$1",
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
