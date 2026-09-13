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
    extract::{Path, Query, State},
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
    cloudflare: Option<CloudflareConfig>,
    provided_domain_suffixes: Vec<String>,
    customer_email: Option<CustomerEmailConfig>,
}
#[derive(Clone)]
struct CustomerEmailConfig {
    api_key: String,
    from: String,
    public_url: String,
}
#[derive(Clone)]
struct CloudflareConfig {
    zone_id: String,
    api_token: String,
    cname_target: String,
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
    #[serde(with = "time::serde::rfc3339::option")]
    deletion_scheduled_for: Option<OffsetDateTime>,
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
    origin_hostname: String,
    router_enabled: bool,
    blue_bind_address: String,
    green_bind_address: String,
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
struct CustomerCredentials {
    email: String,
    password: Option<String>,
}

#[derive(Deserialize)]
struct NewCustomerPassword {
    password: String,
}
#[derive(Deserialize)]
struct VerificationQuery {
    token: String,
}
#[derive(Deserialize)]
struct CustomerTenantRequest {
    subdomain: String,
    title: String,
    custom_domain: Option<String>,
    owner_username: String,
}

#[derive(Deserialize)]
struct AutomaticRelease {
    enabled: bool,
}

#[derive(Deserialize)]
struct NewTenant {
    subdomain: String,
    title: String,
    contact_email: Option<String>,
    runtime_id: Uuid,
    #[serde(default = "default_true")]
    registrations_open: bool,
    #[serde(default)]
    approval_required: bool,
}

#[derive(Deserialize)]
struct NewDomain {
    hostname: String,
}

#[derive(Serialize, sqlx::FromRow)]
struct TenantDomain {
    id: Uuid,
    hostname: String,
    kind: String,
    status: String,
    is_primary: bool,
    cloudflare_status: Option<String>,
    ssl_status: Option<String>,
    cname_target: String,
    ownership_name: Option<String>,
    ownership_value: Option<String>,
    error: Option<String>,
}

#[derive(Serialize, sqlx::FromRow)]
struct CustomerDomainSetup {
    hostname: String,
    kind: String,
    status: String,
    is_primary: bool,
    cname_target: String,
    ownership_name: Option<String>,
    ownership_value: Option<String>,
    ssl_status: Option<String>,
}

#[derive(Serialize, sqlx::FromRow)]
struct CustomerProvisioningStep {
    step: String,
    status: String,
    #[serde(with = "time::serde::rfc3339")]
    updated_at: OffsetDateTime,
}

#[derive(Serialize, sqlx::FromRow)]
struct CustomerTenantInfo {
    id: Uuid,
    domain: String,
    title: String,
    lifecycle_state: String,
    observed_state: String,
    #[serde(with = "time::serde::rfc3339")]
    updated_at: OffsetDateTime,
    owner_username: Option<String>,
    owner_ready: bool,
    #[serde(with = "time::serde::rfc3339::option")]
    owner_password_issued_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    deletion_scheduled_for: Option<OffsetDateTime>,
}

#[derive(Serialize)]
struct CustomerTenantDetail {
    tenant: CustomerTenantInfo,
    domains: Vec<CustomerDomainSetup>,
    provisioning_steps: Vec<CustomerProvisioningStep>,
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    aliases: Vec<String>,
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
    aliases: Vec<String>,
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
    lifecycle_state: String,
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
            aliases: tenant.aliases,
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
        cloudflare: match (
            std::env::var("CLOUDFLARE_ZONE_ID"),
            std::env::var("CLOUDFLARE_API_TOKEN"),
        ) {
            (Ok(zone_id), Ok(api_token)) => Some(CloudflareConfig {
                zone_id,
                api_token,
                cname_target: std::env::var("CLOUDFLARE_SAAS_CNAME_TARGET")
                    .unwrap_or_else(|_| "customers.eunha.space".into()),
            }),
            _ => None,
        },
        provided_domain_suffixes: std::env::var("EUNHA_PROVIDED_DOMAIN_SUFFIXES")
            .unwrap_or_else(|_| "eunha.space".into())
            .split(',')
            .map(|suffix| suffix.trim().trim_start_matches('.').to_ascii_lowercase())
            .filter(|suffix| is_valid_domain(suffix))
            .collect(),
        customer_email: std::env::var("EUNHA_SPACE_RESEND_API_KEY")
            .ok()
            .map(|api_key| CustomerEmailConfig {
                api_key,
                from: std::env::var("EUNHA_SPACE_EMAIL_FROM")
                    .unwrap_or_else(|_| "eunha space <noreply@eunha.space>".into()),
                public_url: std::env::var("EUNHA_SPACE_PUBLIC_URL")
                    .unwrap_or_else(|_| "https://eunha.space".into()),
            }),
        client: reqwest::Client::builder()
            .timeout(Duration::from_secs(4))
            .build()?,
    });
    tokio::spawn(health_reconciler(state.clone()));
    tokio::spawn(release_recovery_reconciler(state.clone()));
    tokio::spawn(automatic_release_reconciler(state.clone()));
    tokio::spawn(provisioning_reconciler(state.clone()));
    tokio::spawn(domain_reconciler(state.clone()));
    tokio::spawn(deprovisioning_reconciler(state.clone()));
    let api = Router::new()
        .route("/session", post(login).delete(logout))
        .route("/session/me", get(me))
        .route("/customer/register", post(customer_register))
        .route("/customer/verify", get(customer_verify))
        .route(
            "/customer/session",
            post(customer_login).delete(customer_logout),
        )
        .route("/customer/session/me", get(customer_me))
        .route("/customer/password", post(customer_set_password))
        .route(
            "/customer/tenants",
            get(customer_tenants).post(customer_create_tenant),
        )
        .route("/customer/tenants/{id}", get(customer_tenant_detail))
        .route(
            "/customer/tenants/{id}/owner/password",
            post(customer_reset_owner_password),
        )
        .route(
            "/customer/tenants/{id}/deletion",
            post(customer_schedule_deletion).delete(customer_cancel_deletion),
        )
        .route(
            "/customer/tenants/{id}/deletion/now",
            post(customer_delete_now),
        )
        .route("/customer/tenants/{id}/restore", post(customer_restore))
        .route("/tenants", get(tenants).post(create_tenant))
        .route("/tenants/{id}/check", post(check_tenant))
        .route("/tenants/{id}/retry", post(retry_provisioning))
        .route(
            "/tenants/{id}/domains",
            get(tenant_domains).post(attach_domain),
        )
        .route(
            "/tenants/{id}/domains/{domain_id}/cloudflare",
            post(adopt_legacy_domain),
        )
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
        .route_service("/admin/{*section}", index.clone())
        .route_service("/signup", index.clone())
        .route_service("/verify", index.clone())
        .route_service("/account", index)
        .route_service(
            "/instances/{id}",
            ServeFile::new("frontend/dist/index.html"),
        )
        .fallback_service(ServeDir::new("frontend/dist"))
        .layer(TraceLayer::new_for_http())
        .with_state(state);
    let bind = std::env::var("BIND_ADDRESS").unwrap_or_else(|_| "127.0.0.1:60001".into());
    let listener = tokio::net::TcpListener::bind(&bind).await?;
    tracing::info!(%bind, "eunha space control plane listening");
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

fn customer_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .map(str::trim)
        .find_map(|value| value.strip_prefix("eunha_space_customer_session="))
}

async fn customer(headers: &HeaderMap, db: &PgPool) -> Result<(Uuid, String), ApiError> {
    let token = customer_token(headers)
        .ok_or_else(|| ApiError(StatusCode::UNAUTHORIZED, "Sign in required".into()))?;
    sqlx::query_as("SELECT c.id,c.email FROM customer_sessions s JOIN customers c ON c.id=s.customer_id WHERE s.token_hash=$1 AND s.expires_at>now() AND c.email_verified_at IS NOT NULL")
        .bind(token_hash(token)).fetch_optional(db).await?.ok_or_else(|| ApiError(StatusCode::UNAUTHORIZED, "Session expired".into()))
}

async fn customer_register(
    State(s): State<Shared>,
    Json(mut body): Json<CustomerCredentials>,
) -> Result<StatusCode, ApiError> {
    body.email = body.email.trim().to_ascii_lowercase();
    if !body.email.contains('@') || body.email.len() > 254 {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "Enter a valid email address.".into(),
        ));
    }
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO customers(id,email,password_hash) VALUES($1,$2,NULL)")
        .bind(id)
        .bind(&body.email)
        .execute(&s.db)
        .await
        .map_err(|error| {
            if error.as_database_error().and_then(|e| e.constraint()) == Some("customers_email_key")
            {
                ApiError(
                    StatusCode::CONFLICT,
                    "An account with that email already exists.".into(),
                )
            } else {
                ApiError::from(error)
            }
        })?;
    if let Err(error) =
        send_customer_link(&s, id, &body.email, "Verify your eunha space email").await
    {
        sqlx::query("DELETE FROM customers WHERE id=$1")
            .bind(id)
            .execute(&s.db)
            .await?;
        return Err(error);
    }
    Ok(StatusCode::ACCEPTED)
}

async fn send_customer_link(
    s: &Shared,
    customer_id: Uuid,
    email: &str,
    subject: &str,
) -> Result<(), ApiError> {
    let mail = s.customer_email.as_ref().ok_or_else(|| {
        ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "Customer email verification is not configured.".into(),
        )
    })?;
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    let token = bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
    sqlx::query("DELETE FROM customer_email_verifications WHERE customer_id=$1")
        .bind(customer_id)
        .execute(&s.db)
        .await?;
    sqlx::query("INSERT INTO customer_email_verifications(token_hash,customer_id,expires_at) VALUES($1,$2,now()+interval '30 minutes')").bind(token_hash(&token)).bind(customer_id).execute(&s.db).await?;
    let url = format!(
        "{}/verify?token={}",
        mail.public_url.trim_end_matches('/'),
        token
    );
    let response=s.client.post("https://api.resend.com/emails").bearer_auth(&mail.api_key).json(&serde_json::json!({"from":mail.from,"to":[email],"subject":subject,"text":format!("Use this secure link to continue to eunha space:\n\n{url}\n\nThis link expires in 30 minutes.")})).send().await;
    if matches!(response,Ok(response) if response.status().is_success()) {
        Ok(())
    } else {
        Err(ApiError(
            StatusCode::BAD_GATEWAY,
            "Could not send the email link.".into(),
        ))
    }
}

async fn customer_verify(
    State(s): State<Shared>,
    Query(query): Query<VerificationQuery>,
) -> Result<Response, ApiError> {
    let customer_id=sqlx::query_scalar::<_,Uuid>("UPDATE customers c SET email_verified_at=COALESCE(email_verified_at,now()) FROM customer_email_verifications v WHERE v.customer_id=c.id AND v.token_hash=$1 AND v.expires_at>now() RETURNING c.id").bind(token_hash(&query.token)).fetch_optional(&s.db).await?;
    let Some(customer_id) = customer_id else {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "That verification link is invalid or expired.".into(),
        ));
    };
    sqlx::query("DELETE FROM customer_email_verifications WHERE token_hash=$1")
        .bind(token_hash(&query.token))
        .execute(&s.db)
        .await?;
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    let token = bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
    sqlx::query("INSERT INTO customer_sessions(token_hash,customer_id,expires_at) VALUES($1,$2,now()+interval '30 days')").bind(token_hash(&token)).bind(customer_id).execute(&s.db).await?;
    Ok(([(header::SET_COOKIE,format!("eunha_space_customer_session={token}; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age=2592000"))],StatusCode::NO_CONTENT).into_response())
}

async fn customer_login(
    State(s): State<Shared>,
    Json(mut body): Json<CustomerCredentials>,
) -> Result<Response, ApiError> {
    body.email = body.email.trim().to_ascii_lowercase();
    let row = sqlx::query_as::<_, (Uuid, Option<String>, bool)>(
        "SELECT id,password_hash,email_verified_at IS NOT NULL FROM customers WHERE email=$1",
    )
    .bind(&body.email)
    .fetch_optional(&s.db)
    .await?;
    let Some((id, hash, verified)) = row else {
        return Err(ApiError(
            StatusCode::UNAUTHORIZED,
            "Invalid credentials".into(),
        ));
    };
    if body.password.is_none() {
        send_customer_link(&s, id, &body.email, "Your eunha space sign-in link").await?;
        return Ok(StatusCode::ACCEPTED.into_response());
    }
    let hash = hash.ok_or_else(|| {
        ApiError(
            StatusCode::FORBIDDEN,
            "Set a password after signing in with an email link.".into(),
        )
    })?;
    let parsed = PasswordHash::new(&hash).map_err(|_| {
        ApiError(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Stored credential is invalid".into(),
        )
    })?;
    if Argon2::default()
        .verify_password(
            body.password.as_deref().unwrap_or_default().as_bytes(),
            &parsed,
        )
        .is_err()
    {
        return Err(ApiError(
            StatusCode::UNAUTHORIZED,
            "Invalid credentials".into(),
        ));
    }
    if !verified {
        return Err(ApiError(
            StatusCode::FORBIDDEN,
            "Verify your email before signing in.".into(),
        ));
    }
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    let token = bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
    sqlx::query("INSERT INTO customer_sessions(token_hash,customer_id,expires_at) VALUES($1,$2,now()+interval '30 days')").bind(token_hash(&token)).bind(id).execute(&s.db).await?;
    Ok(([(header::SET_COOKIE,format!("eunha_space_customer_session={token}; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age=2592000"))],Json(serde_json::json!({"email":body.email}))).into_response())
}

async fn customer_set_password(
    State(s): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<NewCustomerPassword>,
) -> Result<StatusCode, ApiError> {
    let (id, _) = customer(&headers, &s.db).await?;
    if body.password.len() < 14 {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "Use a password with at least 14 characters.".into(),
        ));
    }
    let salt = SaltString::generate(&mut OsRng);
    let hash = Argon2::default()
        .hash_password(body.password.as_bytes(), &salt)
        .map_err(|_| {
            ApiError(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Could not secure the password.".into(),
            )
        })?
        .to_string();
    sqlx::query("UPDATE customers SET password_hash=$1 WHERE id=$2")
        .bind(hash)
        .bind(id)
        .execute(&s.db)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn customer_logout(
    State(s): State<Shared>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    if let Some(token) = customer_token(&headers) {
        sqlx::query("DELETE FROM customer_sessions WHERE token_hash=$1")
            .bind(token_hash(token))
            .execute(&s.db)
            .await?;
    }
    Ok((
        [(
            header::SET_COOKIE,
            "eunha_space_customer_session=; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age=0",
        )],
        StatusCode::NO_CONTENT,
    )
        .into_response())
}
async fn customer_me(
    State(s): State<Shared>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    let (id, email) = customer(&headers, &s.db).await?;
    let has_password = sqlx::query_scalar::<_, bool>(
        "SELECT password_hash IS NOT NULL FROM customers WHERE id=$1",
    )
    .bind(id)
    .fetch_one(&s.db)
    .await?;
    Ok(Json(
        serde_json::json!({"email":email,"instance_limit":CUSTOMER_INSTANCE_LIMIT,"has_password":has_password}),
    ))
}
async fn customer_tenants(
    State(s): State<Shared>,
    headers: HeaderMap,
) -> Result<Json<Vec<TenantSummary>>, ApiError> {
    let (id, _) = customer(&headers, &s.db).await?;
    Ok(Json(sqlx::query_as("SELECT t.id,t.domain,t.title,t.lifecycle_state,t.desired_state,CASE WHEN t.observed_at IS NULL OR t.observed_at < now()-interval '90 seconds' THEN 'unknown' ELSE t.observed_state END observed_state,t.config_revision,t.rendered_revision,r.name runtime_name,r.bind_address,t.updated_at,t.deletion_scheduled_for FROM tenants t JOIN runtimes r ON r.id=t.runtime_id WHERE t.customer_id=$1 AND t.lifecycle_state <> 'purged' ORDER BY t.created_at").bind(id).fetch_all(&s.db).await?))
}

async fn customer_tenant_detail(
    State(s): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<CustomerTenantDetail>, ApiError> {
    let (customer_id, _) = customer(&headers, &s.db).await?;
    let tenant = sqlx::query_as("SELECT t.id,t.domain,t.title,t.lifecycle_state,CASE WHEN t.observed_at IS NULL OR t.observed_at < now()-interval '90 seconds' THEN 'unknown' ELSE t.observed_state END observed_state,t.updated_at,t.owner_username,EXISTS(SELECT 1 FROM provisioning_steps p WHERE p.tenant_id=t.id AND p.step='owner' AND p.status='succeeded') owner_ready,t.owner_password_issued_at,t.deletion_scheduled_for FROM tenants t WHERE t.id=$1 AND t.customer_id=$2 AND t.lifecycle_state <> 'purged'")
        .bind(id)
        .bind(customer_id)
        .fetch_optional(&s.db)
        .await?
        .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "Instance not found".into()))?;
    let domains = sqlx::query_as("SELECT hostname,kind,status,is_primary,cname_target,ownership_name,ownership_value,ssl_status FROM tenant_domains WHERE tenant_id=$1 AND kind IN ('provided','custom') ORDER BY is_primary DESC,created_at")
        .bind(id)
        .fetch_all(&s.db)
        .await?;
    let provisioning_steps = sqlx::query_as("SELECT step,status,updated_at FROM provisioning_steps WHERE tenant_id=$1 ORDER BY array_position(ARRAY['database','migrate','render','reload','health','routing','owner'],step)")
        .bind(id)
        .fetch_all(&s.db)
        .await?;
    Ok(Json(CustomerTenantDetail {
        tenant,
        domains,
        provisioning_steps,
    }))
}
async fn tenants(
    State(s): State<Shared>,
    headers: HeaderMap,
) -> Result<Json<Vec<TenantSummary>>, ApiError> {
    operator(&headers, &s.db).await?;
    Ok(Json(sqlx::query_as("SELECT t.id,t.domain,t.title,t.lifecycle_state,t.desired_state,CASE WHEN t.observed_at IS NULL OR t.observed_at < now()-interval '90 seconds' THEN 'unknown' ELSE t.observed_state END observed_state,t.config_revision,t.rendered_revision,r.name runtime_name,r.bind_address,t.updated_at,t.deletion_scheduled_for FROM tenants t JOIN runtimes r ON r.id=t.runtime_id ORDER BY t.domain").fetch_all(&s.db).await?))
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

fn tenant_redis_key_prefix(tenant_id: Uuid) -> String {
    format!("t_{}_", tenant_id.simple())
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
    Json(body): Json<NewTenant>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let (operator_id, _) = operator(&headers, &s.db).await?;
    create_tenant_inner(&s, body, Some(operator_id), None, None, None).await
}

async fn customer_create_tenant(
    State(s): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<CustomerTenantRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let (customer_id, email) = customer(&headers, &s.db).await?;
    let has_password = sqlx::query_scalar::<_, bool>(
        "SELECT password_hash IS NOT NULL FROM customers WHERE id=$1",
    )
    .bind(customer_id)
    .fetch_one(&s.db)
    .await?;
    if !has_password {
        return Err(ApiError(
            StatusCode::FORBIDDEN,
            "Set a password before creating an instance.".into(),
        ));
    }
    let owner_username = body.owner_username.trim().to_owned();
    if !is_valid_owner_username(&owner_username) {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "Use up to 30 letters, numbers, and underscores for the admin username.".into(),
        ));
    }
    let runtime_id=sqlx::query_scalar::<_,Uuid>("SELECT r.id FROM runtimes r WHERE EXISTS(SELECT 1 FROM tenants t JOIN tenant_configuration c ON c.tenant_id=t.id JOIN r2_buckets b ON b.tenant_id=t.id WHERE t.runtime_id=r.id AND t.lifecycle_state='ready') ORDER BY (SELECT count(*) FROM tenants t WHERE t.runtime_id=r.id AND t.lifecycle_state <> 'purged'),r.created_at LIMIT 1").fetch_optional(&s.db).await?.ok_or_else(||ApiError(StatusCode::SERVICE_UNAVAILABLE,"No runtime is currently available.".into()))?;
    create_tenant_inner(
        &s,
        NewTenant {
            subdomain: body.subdomain,
            title: body.title,
            contact_email: Some(email),
            runtime_id,
            // Mastodon's default `registrations_mode` is `none`: a new server
            // is closed until its Owner decides otherwise, so nobody can sign
            // up ahead of the account provisioning creates for the customer.
            registrations_open: false,
            approval_required: false,
        },
        None,
        Some(customer_id),
        body.custom_domain,
        Some(owner_username),
    )
    .await
}

async fn create_tenant_inner(
    s: &Shared,
    mut body: NewTenant,
    operator_id: Option<Uuid>,
    customer_id: Option<Uuid>,
    custom_domain: Option<String>,
    owner_username: Option<String>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    body.subdomain = body.subdomain.trim().to_ascii_lowercase();
    let suffix = s.provided_domain_suffixes.first().ok_or_else(|| {
        ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "No provided domain suffix is configured.".into(),
        )
    })?;
    let domain = format!("{}.{}", body.subdomain, suffix);
    let provided_domains: Vec<String> = s
        .provided_domain_suffixes
        .iter()
        .map(|suffix| format!("{}.{}", body.subdomain, suffix))
        .collect();
    let aliases: Vec<String> = provided_domains.iter().skip(1).cloned().collect();
    body.title = body.title.trim().to_owned();
    body.contact_email = body
        .contact_email
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());
    if body.subdomain.is_empty()
        || body.subdomain.len() > 63
        || body.subdomain.starts_with('-')
        || body.subdomain.ends_with('-')
        || !body
            .subdomain
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        || body.title.is_empty()
        || body.title.len() > 200
    {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "Enter a valid domain and title.".into(),
        ));
    }
    if sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM reserved_subdomains WHERE label=$1)",
    )
    .bind(&body.subdomain)
    .fetch_one(&s.db)
    .await?
    {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "That subdomain is reserved.".into(),
        ));
    }

    let origin_hostname =
        sqlx::query_scalar::<_, String>("SELECT origin_hostname FROM runtimes WHERE id=$1")
            .bind(body.runtime_id)
            .fetch_optional(&s.db)
            .await?
            .ok_or_else(|| ApiError(StatusCode::BAD_REQUEST, "Select a valid runtime.".into()))?;
    if !is_valid_domain(&origin_hostname) {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "The selected runtime has an invalid Cloudflare origin hostname.".into(),
        ));
    }
    let cname_target = s
        .cloudflare
        .as_ref()
        .ok_or_else(|| {
            ApiError(
                StatusCode::SERVICE_UNAVAILABLE,
                "Cloudflare for SaaS is not configured.".into(),
            )
        })?
        .cname_target
        .clone();

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
    // Redis keyspaces are separated internally with `:`, so tenant-controlled
    // prefixes themselves are restricted to Eunha's safe `[A-Za-z0-9_-]`
    // alphabet.
    let redis_prefix = tenant_redis_key_prefix(tenant_id);

    let custom_domain = custom_domain
        .map(|value| value.trim().trim_end_matches('.').to_ascii_lowercase())
        .filter(|value| !value.is_empty());
    if custom_domain
        .as_deref()
        .is_some_and(|value| !is_valid_domain(value) || value.ends_with(".eunha.space"))
    {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "Enter a valid externally owned custom domain.".into(),
        ));
    }
    let mut tx = s.db.begin().await?;
    if let Some(customer_id) = customer_id {
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1::text,0))")
            .bind(customer_id)
            .execute(&mut *tx)
            .await?;
        if customer_instance_count(&mut tx, customer_id).await? >= CUSTOMER_INSTANCE_LIMIT {
            return Err(ApiError(
                StatusCode::CONFLICT,
                "Each customer may create up to two instances.".into(),
            ));
        }
    }
    sqlx::query("INSERT INTO tenants(id,domain,aliases,title,runtime_id,desired_state,observed_state,lifecycle_state,contact_email,registrations_open,approval_required,vapid_private_key,vapid_public_key,customer_id,owner_username) VALUES($1,$2,$3,$4,$5,'running','unknown','provisioning',$6,$7,$8,$9,$10,$11,$12)")
        .bind(tenant_id).bind(&domain).bind(&aliases).bind(&body.title).bind(body.runtime_id)
        .bind(&body.contact_email).bind(body.registrations_open).bind(body.approval_required)
        .bind(vapid_private).bind(vapid_public).bind(customer_id).bind(&owner_username).execute(&mut *tx).await
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
    for (index, hostname) in provided_domains.iter().enumerate() {
        sqlx::query("INSERT INTO tenant_domains(id,tenant_id,hostname,kind,status,is_primary,cname_target) VALUES($1,$2,$3,'provided','pending',$4,$5)")
            .bind(Uuid::new_v4()).bind(tenant_id).bind(hostname).bind(index == 0).bind(&cname_target).execute(&mut *tx).await?;
    }
    if let Some(hostname) = custom_domain {
        sqlx::query("INSERT INTO tenant_domains(id,tenant_id,hostname,kind,status,is_primary,cname_target) VALUES($1,$2,$3,'custom','pending',false,$4)").bind(Uuid::new_v4()).bind(tenant_id).bind(hostname).bind(&cname_target).execute(&mut *tx).await?;
    }
    sqlx::query("INSERT INTO operations(id,tenant_id,runtime_id,kind,status,requested_by,requested_by_customer,detail,started_at) VALUES($1,$2,$3,'tenant_provision','running',$4,$5,jsonb_build_object('domain',$6),now())")
        .bind(operation_id).bind(tenant_id).bind(body.runtime_id).bind(operator_id).bind(customer_id).bind(&domain).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO provisioning_steps(tenant_id,step) SELECT $1,unnest(ARRAY['database','migrate','render','reload','health','routing'] || CASE WHEN $2 THEN ARRAY['owner'] ELSE ARRAY[]::text[] END)")
        .bind(tenant_id).bind(owner_username.is_some()).execute(&mut *tx).await?;
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

async fn tenant_domains(
    State(s): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<TenantDomain>>, ApiError> {
    operator(&headers, &s.db).await?;
    Ok(Json(sqlx::query_as("SELECT id,hostname,kind,status,is_primary,cloudflare_status,ssl_status,cname_target,ownership_name,ownership_value,error FROM tenant_domains WHERE tenant_id=$1 ORDER BY is_primary DESC,created_at")
        .bind(id).fetch_all(&s.db).await?))
}

async fn attach_domain(
    State(s): State<Shared>,
    headers: HeaderMap,
    Path(tenant_id): Path<Uuid>,
    Json(mut body): Json<NewDomain>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let (operator_id, _) = operator(&headers, &s.db).await?;
    if s.cloudflare.is_none() {
        return Err(ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "Cloudflare for SaaS is not configured.".into(),
        ));
    }
    body.hostname = body
        .hostname
        .trim()
        .trim_end_matches('.')
        .to_ascii_lowercase();
    if !is_valid_domain(&body.hostname)
        || body.hostname == "eunha.space"
        || body.hostname.ends_with(".eunha.space")
    {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "Enter an externally owned custom domain.".into(),
        ));
    }
    let cloudflare = s.cloudflare.as_ref().expect("checked above");
    let domain_id = Uuid::new_v4();
    let operation_id = Uuid::new_v4();
    let mut tx = s.db.begin().await?;
    let changed = sqlx::query("INSERT INTO tenant_domains(id,tenant_id,hostname,kind,status,cname_target) SELECT $1,id,$2,'custom','pending',$3 FROM tenants WHERE id=$4 AND lifecycle_state='ready' AND NOT EXISTS(SELECT 1 FROM tenant_domains WHERE tenant_id=$4 AND kind='custom' AND status='pending')")
        .bind(domain_id).bind(&body.hostname).bind(&cloudflare.cname_target).bind(tenant_id).execute(&mut *tx).await
        .map_err(|error| if error.as_database_error().and_then(|e| e.constraint()) == Some("tenant_domains_hostname_key") { ApiError(StatusCode::CONFLICT, "That hostname is already attached.".into()) } else { ApiError::from(error) })?.rows_affected();
    if changed == 0 {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "The tenant must be ready and have no other pending custom domain.".into(),
        ));
    }
    sqlx::query("INSERT INTO operations(id,tenant_id,runtime_id,kind,status,requested_by,detail,started_at) SELECT $1,t.id,t.runtime_id,'custom_domain_attach','running',$2,jsonb_build_object('hostname',$3),now() FROM tenants t WHERE t.id=$4")
        .bind(operation_id).bind(operator_id).bind(&body.hostname).bind(tenant_id).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO operation_events(operation_id,message) VALUES($1,'Custom hostname registration queued.')")
        .bind(operation_id).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(serde_json::json!({"id": domain_id, "operation_id": operation_id})),
    ))
}

async fn adopt_legacy_domain(
    State(s): State<Shared>,
    headers: HeaderMap,
    Path((tenant_id, domain_id)): Path<(Uuid, Uuid)>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let (operator_id, _) = operator(&headers, &s.db).await?;
    let cloudflare = s.cloudflare.as_ref().ok_or_else(|| {
        ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "Cloudflare for SaaS is not configured.".into(),
        )
    })?;
    let operation_id = Uuid::new_v4();
    let mut tx = s.db.begin().await?;
    let hostname = sqlx::query_scalar::<_, String>(
        "UPDATE tenant_domains d SET kind='custom',status='pending',cname_target=$1,cloudflare_id=NULL,cloudflare_status=NULL,ssl_status=NULL,ownership_name=NULL,ownership_value=NULL,error=NULL,attempts=0,updated_at=now() FROM tenants t WHERE d.id=$2 AND d.tenant_id=$3 AND d.kind='legacy' AND d.is_primary AND t.id=d.tenant_id AND t.lifecycle_state='ready' RETURNING d.hostname",
    )
    .bind(&cloudflare.cname_target)
    .bind(domain_id)
    .bind(tenant_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| {
        ApiError(
            StatusCode::CONFLICT,
            "Only a ready tenant's legacy primary domain can be adopted.".into(),
        )
    })?;
    sqlx::query("INSERT INTO operations(id,tenant_id,runtime_id,kind,status,requested_by,detail,started_at) SELECT $1,t.id,t.runtime_id,'custom_domain_attach','running',$2,jsonb_build_object('hostname',$3,'mode','adopt_legacy_primary'),now() FROM tenants t WHERE t.id=$4")
        .bind(operation_id).bind(operator_id).bind(&hostname).bind(tenant_id).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO operation_events(operation_id,message) VALUES($1,'Legacy primary domain queued for Cloudflare for SaaS adoption; ActivityPub identity is unchanged.')")
        .bind(operation_id).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(serde_json::json!({"id": domain_id, "operation_id": operation_id})),
    ))
}

fn is_valid_domain(domain: &str) -> bool {
    is_safe_domain(domain)
        && domain.contains('.')
        && domain.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
        })
}

async fn domain_reconciler(state: Shared) {
    loop {
        if let Ok(ids) = sqlx::query_scalar::<_, Uuid>("SELECT d.id FROM tenant_domains d JOIN tenants t ON t.id=d.tenant_id JOIN runtimes r ON r.id=t.runtime_id WHERE d.kind IN ('provided','custom') AND t.lifecycle_state IN ('provisioning','ready','failed') AND ((d.status='pending' AND d.updated_at < now()-interval '15 seconds') OR d.routed_origin_hostname IS DISTINCT FROM r.origin_hostname) ORDER BY d.created_at LIMIT 10")
            .fetch_all(&state.db).await {
            for id in ids {
                if let Err(error) = reconcile_domain(&state, id).await {
                    let message = domain_reconcile_error_message(&error);
                    record_domain_error(&state.db, id, &message).await;
                }
            }
        }
        tokio::time::sleep(Duration::from_secs(15)).await;
    }
}

#[derive(Debug)]
struct DomainDnsTargetError {
    cname_target: String,
}

impl std::fmt::Display for DomainDnsTargetError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "hostname does not point to {}",
            self.cname_target
        )
    }
}

impl std::error::Error for DomainDnsTargetError {}

fn domain_reconcile_error_message(error: &anyhow::Error) -> String {
    if let Some(error) = error.downcast_ref::<DomainDnsTargetError>() {
        return format!(
            "Cloudflare cannot validate this hostname because its DNS does not point to {}. Update the CNAME record; existing routing remains unchanged.",
            error.cname_target
        );
    }
    "Hostname routing could not be reconciled; existing routing remains unchanged.".into()
}

async fn record_domain_error(db: &PgPool, id: Uuid, message: &str) {
    let failed = sqlx::query_scalar::<_, bool>("UPDATE tenant_domains SET attempts=attempts+1,status=CASE WHEN status='pending' AND attempts+1 >= 5 THEN 'failed' ELSE status END,error=$1,updated_at=now() WHERE id=$2 RETURNING status='failed'")
        .bind(message).bind(id).fetch_optional(db).await;
    let is_failed = failed.ok().flatten().unwrap_or(false);
    if !is_failed {
        return;
    }
    let _ = sqlx::query("UPDATE operations SET status='failed',error=$1,finished_at=now() WHERE id=(SELECT o.id FROM operations o JOIN tenant_domains d ON d.tenant_id=o.tenant_id WHERE d.id=$2 AND o.kind='custom_domain_attach' AND o.status='running' ORDER BY o.created_at DESC LIMIT 1)")
        .bind(message).bind(id).execute(db).await;
}

async fn reconcile_domain(state: &Shared, id: Uuid) -> Result<()> {
    let cloudflare = state
        .cloudflare
        .as_ref()
        .context("Cloudflare is not configured")?;
    let (tenant_id, hostname, kind, cloudflare_id, origin_hostname, routed_origin_hostname) = sqlx::query_as::<_, (Uuid, String, String, Option<String>, String, Option<String>)>(
        "SELECT d.tenant_id,d.hostname,d.kind,d.cloudflare_id,r.origin_hostname,d.routed_origin_hostname FROM tenant_domains d JOIN tenants t ON t.id=d.tenant_id JOIN runtimes r ON r.id=t.runtime_id WHERE d.id=$1",
    )
    .bind(id)
    .fetch_one(&state.db)
    .await?;
    let endpoint = format!(
        "https://api.cloudflare.com/client/v4/zones/{}/custom_hostnames",
        cloudflare.zone_id
    );
    let origin_changed = routed_origin_hostname.as_deref() != Some(origin_hostname.as_str());
    let response: serde_json::Value = if let Some(cloudflare_id) = &cloudflare_id {
        let request = if origin_changed {
            state
                .client
                .patch(format!("{endpoint}/{cloudflare_id}"))
                .json(&serde_json::json!({
                    "custom_origin_server": origin_hostname,
                    "ssl": { "method": "http", "type": "dv", "settings": { "min_tls_version": "1.2" } }
                }))
        } else {
            state.client.get(format!("{endpoint}/{cloudflare_id}"))
        };
        request
            .bearer_auth(&cloudflare.api_token)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?
    } else {
        state
            .client
            .post(&endpoint)
            .bearer_auth(&cloudflare.api_token)
            .json(&serde_json::json!({
                "hostname": hostname,
                "custom_origin_server": origin_hostname,
                "ssl": { "method": "http", "type": "dv", "settings": { "min_tls_version": "1.2" } }
            }))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?
    };
    anyhow::ensure!(
        response.get("success").and_then(|v| v.as_bool()) == Some(true),
        "Cloudflare rejected hostname"
    );
    let result = response
        .get("result")
        .context("Cloudflare response has no result")?;
    let cf_id = result
        .get("id")
        .and_then(|v| v.as_str())
        .context("Cloudflare response has no id")?;
    let cf_status = result
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("pending");
    let ssl_status = result
        .pointer("/ssl/status")
        .and_then(|v| v.as_str())
        .unwrap_or("pending");
    let ownership_name = result
        .pointer("/ownership_verification/name")
        .and_then(|v| v.as_str());
    let ownership_value = result
        .pointer("/ownership_verification/value")
        .and_then(|v| v.as_str());
    let cname_mismatch = result
        .get("verification_errors")
        .and_then(|value| value.as_array())
        .is_some_and(|errors| {
            errors.iter().any(|error| {
                error
                    .as_str()
                    .is_some_and(|message| message.contains("does not CNAME to this zone"))
            })
        });
    sqlx::query("UPDATE tenant_domains SET cloudflare_id=$1,cloudflare_status=$2,ssl_status=$3,ownership_name=$4,ownership_value=$5,routed_origin_hostname=$6,updated_at=now() WHERE id=$7")
        .bind(cf_id).bind(cf_status).bind(ssl_status).bind(ownership_name).bind(ownership_value).bind(&origin_hostname).bind(id).execute(&state.db).await?;
    if cname_mismatch {
        return Err(DomainDnsTargetError {
            cname_target: cloudflare.cname_target.clone(),
        }
        .into());
    }
    anyhow::ensure!(
        !matches!(
            cf_status,
            "blocked" | "deleted" | "pending_blocked" | "test_blocked" | "test_failed"
        ),
        "Cloudflare hostname is terminal"
    );
    anyhow::ensure!(
        !ssl_status.ends_with("_timed_out")
            && !matches!(ssl_status, "expired" | "inactive" | "deleted"),
        "Cloudflare certificate is terminal"
    );
    if cf_status != "active" || ssl_status != "active" {
        state
            .client
            .patch(format!("{endpoint}/{cf_id}"))
            .bearer_auth(&cloudflare.api_token)
            .json(&serde_json::json!({
                "custom_origin_server": origin_hostname,
                "ssl": {"method": "http", "type": "dv", "settings": {"min_tls_version": "1.2"}}
            }))
            .send()
            .await?
            .error_for_status()?;
        return Ok(());
    }

    let (primary_id, primary_kind) = sqlx::query_as::<_, (Uuid, String)>(
        "SELECT id,kind FROM tenant_domains WHERE tenant_id=$1 AND is_primary",
    )
    .bind(tenant_id)
    .fetch_one(&state.db)
    .await?;
    // An imported tenant can adopt its existing primary hostname into
    // Cloudflare for SaaS. Its ActivityPub identity and runtime configuration
    // are already correct, so activation only completes the durable operation.
    if primary_id == id {
        healthcheck_provisioned_tenant(state, tenant_id).await?;
        finish_domain(&state.db, id).await?;
        return Ok(());
    }
    if kind == "provided" {
        healthcheck_host(state, tenant_id, &hostname).await?;
        finish_domain(&state.db, id).await?;
        return Ok(());
    }
    let has_identity = if primary_kind == "custom" {
        true
    } else {
        let database_url = sqlx::query_scalar::<_, String>(
            "SELECT database_url FROM tenant_configuration WHERE tenant_id=$1",
        )
        .bind(tenant_id)
        .fetch_one(&state.db)
        .await?;
        let tenant_db = PgPoolOptions::new()
            .max_connections(1)
            .connect(&database_url)
            .await?;
        let has_identity = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM public.accounts WHERE domain IS NULL)",
        )
        .fetch_one(&tenant_db)
        .await;
        tenant_db.close().await;
        has_identity?
    };
    // Local accounts' ActivityPub identities are bound to the primary domain,
    // so once any exist a verified custom domain can only be an alias.
    if has_identity {
        sqlx::query("UPDATE tenants SET aliases=CASE WHEN $1=ANY(aliases) THEN aliases ELSE array_append(aliases,$1) END,config_revision=config_revision+1,updated_at=now() WHERE id=$2")
            .bind(&hostname).bind(tenant_id).execute(&state.db).await?;
        render_provisioned_tenant(state, tenant_id).await?;
        reload_provisioned_runtime(state, tenant_id).await?;
        healthcheck_host(state, tenant_id, &hostname).await?;
        finish_domain(&state.db, id).await?;
        return Ok(());
    }

    let mut tx = state.db.begin().await?;
    sqlx::query("UPDATE tenant_domains SET is_primary=false,updated_at=now() WHERE tenant_id=$1 AND is_primary")
        .bind(tenant_id).execute(&mut *tx).await?;
    sqlx::query("UPDATE tenant_domains SET is_primary=true WHERE id=$1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE tenants SET domain=$1,aliases=ARRAY(SELECT hostname FROM tenant_domains WHERE tenant_id=$2 AND id<>$3 AND (kind IN ('provided','legacy') OR status='active') ORDER BY created_at),config_revision=config_revision+1,updated_at=now() WHERE id=$2")
        .bind(&hostname).bind(tenant_id).bind(id).execute(&mut *tx).await?;
    tx.commit().await?;
    render_provisioned_tenant(state, tenant_id).await?;
    let (directory, aliases) = sqlx::query_as::<_, (String, Vec<String>)>("SELECT r.tenant_directory,t.aliases FROM tenants t JOIN runtimes r ON r.id=t.runtime_id WHERE t.id=$1")
        .bind(tenant_id).fetch_one(&state.db).await?;
    for alias in aliases {
        let old_file = PathBuf::from(&directory).join(tenant_file_name(&alias));
        if tokio::fs::try_exists(&old_file).await? {
            tokio::fs::remove_file(old_file).await?;
        }
    }
    reload_provisioned_runtime(state, tenant_id).await?;
    healthcheck_provisioned_tenant(state, tenant_id).await?;
    finish_domain(&state.db, id).await?;
    Ok(())
}

async fn finish_domain(db: &PgPool, id: Uuid) -> Result<()> {
    sqlx::query(
        "UPDATE tenant_domains SET status='active',error=NULL,updated_at=now() WHERE id=$1",
    )
    .bind(id)
    .execute(db)
    .await?;
    sqlx::query("UPDATE operations SET status='succeeded',finished_at=now() WHERE id=(SELECT o.id FROM operations o JOIN tenant_domains d ON d.tenant_id=o.tenant_id WHERE d.id=$1 AND o.kind='custom_domain_attach' AND o.status='running' ORDER BY o.created_at DESC LIMIT 1)")
        .bind(id).execute(db).await?;
    Ok(())
}

const CUSTOMER_INSTANCE_LIMIT: i64 = 2;

/// How long a customer's deleted instance keeps running before it is
/// deprovisioned, unless they cancel or bring it forward.
const DELETION_GRACE: &str = "7 days";

/// Instances counting against a customer's limit. A deprovisioned instance
/// frees its slot while its data is retained.
async fn customer_instance_count(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    customer_id: Uuid,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM tenants WHERE customer_id=$1 AND lifecycle_state NOT IN ('deprovisioning','retained','purged')",
    )
    .bind(customer_id)
    .fetch_one(&mut **tx)
    .await
}

async fn customer_schedule_deletion(
    State(s): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let (customer_id, _) = customer(&headers, &s.db).await?;
    let mut tx = s.db.begin().await?;
    let scheduled = sqlx::query_scalar::<_, OffsetDateTime>(&format!(
        "UPDATE tenants SET deletion_scheduled_for=now()+interval '{DELETION_GRACE}',updated_at=now() WHERE id=$1 AND customer_id=$2 AND lifecycle_state IN ('ready','failed') AND deletion_scheduled_for IS NULL RETURNING deletion_scheduled_for"
    ))
    .bind(id)
    .bind(customer_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| {
        ApiError(
            StatusCode::CONFLICT,
            "This instance cannot be scheduled for deletion.".into(),
        )
    })?;
    record_customer_operation(
        &mut tx,
        id,
        customer_id,
        "tenant_deletion_scheduled",
        "Deletion scheduled; the instance keeps running until then.",
    )
    .await?;
    tx.commit().await?;
    Ok(Json(serde_json::json!({
        "deletion_scheduled_for": scheduled.format(&time::format_description::well_known::Rfc3339)?,
    })))
}

async fn customer_cancel_deletion(
    State(s): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let (customer_id, _) = customer(&headers, &s.db).await?;
    let mut tx = s.db.begin().await?;
    let changed = sqlx::query("UPDATE tenants SET deletion_scheduled_for=NULL,updated_at=now() WHERE id=$1 AND customer_id=$2 AND lifecycle_state IN ('ready','failed') AND deletion_scheduled_for IS NOT NULL")
        .bind(id).bind(customer_id).execute(&mut *tx).await?.rows_affected();
    if changed == 0 {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "This instance has no deletion to cancel.".into(),
        ));
    }
    record_customer_operation(
        &mut tx,
        id,
        customer_id,
        "tenant_deletion_cancelled",
        "Scheduled deletion cancelled.",
    )
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Deprovision an instance already scheduled for deletion without waiting for
/// the rest of its grace period.
async fn customer_delete_now(
    State(s): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let (customer_id, _) = customer(&headers, &s.db).await?;
    let mut tx = s.db.begin().await?;
    let scheduled = sqlx::query_scalar::<_, bool>("SELECT true FROM tenants WHERE id=$1 AND customer_id=$2 AND lifecycle_state IN ('ready','failed') AND deletion_scheduled_for IS NOT NULL FOR UPDATE")
        .bind(id).bind(customer_id).fetch_optional(&mut *tx).await?.is_some();
    if !scheduled {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "Schedule this instance for deletion first.".into(),
        ));
    }
    begin_deprovisioning(&mut tx, id, Some(customer_id)).await?;
    tx.commit().await?;
    Ok(StatusCode::ACCEPTED)
}

/// Bring a retained instance back on its hostnames, database and accounts.
async fn customer_restore(
    State(s): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let (customer_id, _) = customer(&headers, &s.db).await?;
    let mut tx = s.db.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1::text,0))")
        .bind(customer_id)
        .execute(&mut *tx)
        .await?;
    let retained = sqlx::query_scalar::<_, bool>("SELECT true FROM tenants WHERE id=$1 AND customer_id=$2 AND lifecycle_state='retained' FOR UPDATE")
        .bind(id).bind(customer_id).fetch_optional(&mut *tx).await?.is_some();
    if !retained {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "Only a deleted instance can be restored.".into(),
        ));
    }
    if customer_instance_count(&mut tx, customer_id).await? >= CUSTOMER_INSTANCE_LIMIT {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "Delete another instance before restoring this one.".into(),
        ));
    }
    sqlx::query("UPDATE tenants SET lifecycle_state='provisioning',desired_state='running',observed_state='unknown',observed_at=NULL,observation_error=NULL,deletion_scheduled_for=NULL,updated_at=now() WHERE id=$1")
        .bind(id).execute(&mut *tx).await?;
    // Its database, owner and routing survive; the instance only needs its
    // schema brought up to the runtime's release and its configuration served.
    sqlx::query("INSERT INTO provisioning_steps(tenant_id,step) SELECT $1,unnest(ARRAY['migrate','render','reload','health']) ON CONFLICT (tenant_id,step) DO UPDATE SET status='pending',error=NULL,started_at=NULL,finished_at=NULL,updated_at=now()")
        .bind(id).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM deprovisioning_steps WHERE tenant_id=$1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    let operation_id = Uuid::new_v4();
    sqlx::query("INSERT INTO operations(id,tenant_id,runtime_id,kind,status,requested_by_customer,detail,started_at) SELECT $1,t.id,t.runtime_id,'tenant_provision','running',$2,jsonb_build_object('mode','restore'),now() FROM tenants t WHERE t.id=$3")
        .bind(operation_id).bind(customer_id).bind(id).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO operation_events(operation_id,message) VALUES($1,'Restore queued; retained data is served again once the instance is healthy.')")
        .bind(operation_id).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(StatusCode::ACCEPTED)
}

async fn record_customer_operation(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: Uuid,
    customer_id: Uuid,
    kind: &str,
    message: &str,
) -> Result<(), sqlx::Error> {
    let operation_id = Uuid::new_v4();
    sqlx::query("INSERT INTO operations(id,tenant_id,runtime_id,kind,status,requested_by_customer,started_at,finished_at) SELECT $1,t.id,t.runtime_id,$2,'succeeded',$3,now(),now() FROM tenants t WHERE t.id=$4")
        .bind(operation_id).bind(kind).bind(customer_id).bind(tenant_id).execute(&mut **tx).await?;
    sqlx::query("INSERT INTO operation_events(operation_id,message) VALUES($1,$2)")
        .bind(operation_id)
        .bind(message)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// Start taking a tenant offline. Nothing is destroyed: the database, media,
/// configuration and hostnames stay, and only an operator's purge removes them.
async fn begin_deprovisioning(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: Uuid,
    customer_id: Option<Uuid>,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE tenants SET lifecycle_state='deprovisioning',desired_state='deprovisioning',observed_state='disabled',observed_at=now(),observation_error=NULL,deletion_scheduled_for=NULL,updated_at=now() WHERE id=$1")
        .bind(tenant_id).execute(&mut **tx).await?;
    sqlx::query("INSERT INTO deprovisioning_steps(tenant_id,step) SELECT $1,unnest(ARRAY['unrender','reload','offline']) ON CONFLICT (tenant_id,step) DO UPDATE SET status='pending',attempts=0,error=NULL,started_at=NULL,finished_at=NULL,updated_at=now()")
        .bind(tenant_id).execute(&mut **tx).await?;
    let operation_id = Uuid::new_v4();
    sqlx::query("INSERT INTO operations(id,tenant_id,runtime_id,kind,status,requested_by_customer,started_at) SELECT $1,t.id,t.runtime_id,'tenant_deprovision','running',$2,now() FROM tenants t WHERE t.id=$3")
        .bind(operation_id).bind(customer_id).bind(tenant_id).execute(&mut **tx).await?;
    sqlx::query("INSERT INTO operation_events(operation_id,message) VALUES($1,'Deprovisioning started; data is retained.')")
        .bind(operation_id).execute(&mut **tx).await?;
    Ok(())
}

/// A deprovisioning step that failed is retried this often, this many times,
/// before the operation is left failed for an operator.
const DEPROVISION_RETRY_SECONDS: i64 = 60;
const DEPROVISION_MAX_ATTEMPTS: i32 = 10;

async fn deprovisioning_reconciler(state: Shared) {
    loop {
        if let Ok(ids) = sqlx::query_scalar::<_, Uuid>("SELECT id FROM tenants WHERE lifecycle_state IN ('ready','failed') AND deletion_scheduled_for <= now() ORDER BY deletion_scheduled_for LIMIT 10")
            .fetch_all(&state.db).await
        {
            for id in ids {
                if let Ok(mut tx) = state.db.begin().await {
                    // Re-checked under the row lock, so a cancellation that
                    // landed after the scan wins.
                    let due = sqlx::query_scalar::<_, bool>("SELECT true FROM tenants WHERE id=$1 AND lifecycle_state IN ('ready','failed') AND deletion_scheduled_for <= now() FOR UPDATE")
                        .bind(id).fetch_optional(&mut *tx).await.ok().flatten().is_some();
                    if due && begin_deprovisioning(&mut tx, id, None).await.is_ok() {
                        let _ = tx.commit().await;
                    }
                }
            }
        }
        if let Ok(ids) = sqlx::query_scalar::<_, Uuid>("SELECT id FROM tenants WHERE lifecycle_state='deprovisioning' ORDER BY updated_at LIMIT 10")
            .fetch_all(&state.db).await
        {
            for id in ids {
                deprovision_tenant(&state, id).await;
            }
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}

async fn deprovision_tenant(state: &Shared, tenant_id: Uuid) {
    for step in ["unrender", "reload", "offline"] {
        let row = sqlx::query_as::<_, (String, i32, bool)>(&format!(
            "SELECT status,attempts,updated_at < now()-interval '{DEPROVISION_RETRY_SECONDS} seconds' FROM deprovisioning_steps WHERE tenant_id=$1 AND step=$2"
        ))
        .bind(tenant_id)
        .bind(step)
        .fetch_optional(&state.db)
        .await;
        let Ok(Some((status, attempts, stale))) = row else {
            return;
        };
        match status.as_str() {
            "succeeded" => continue,
            "failed" if !stale || attempts >= DEPROVISION_MAX_ATTEMPTS => return,
            _ => {}
        }
        if sqlx::query("UPDATE deprovisioning_steps SET status='running',attempts=attempts+1,error=NULL,started_at=now(),finished_at=NULL,updated_at=now() WHERE tenant_id=$1 AND step=$2")
            .bind(tenant_id).bind(step).execute(&state.db).await.is_err()
        {
            return;
        }
        let result = match step {
            "unrender" => unrender_tenant(state, tenant_id).await,
            "reload" => reload_provisioned_runtime(state, tenant_id).await,
            "offline" => verify_tenant_offline(state, tenant_id).await,
            _ => unreachable!(),
        };
        let operation = sqlx::query_scalar::<_, Uuid>("SELECT id FROM operations WHERE tenant_id=$1 AND kind='tenant_deprovision' AND status='running' ORDER BY created_at DESC LIMIT 1")
            .bind(tenant_id).fetch_optional(&state.db).await.ok().flatten();
        if result.is_err() {
            let exhausted = attempts + 1 >= DEPROVISION_MAX_ATTEMPTS;
            let message = if exhausted {
                format!("{step} failed; an operator needs to take the instance offline.")
            } else {
                format!("{step} failed; retrying.")
            };
            let _ = sqlx::query("UPDATE deprovisioning_steps SET status='failed',error=$1,finished_at=now(),updated_at=now() WHERE tenant_id=$2 AND step=$3")
                .bind(&message).bind(tenant_id).bind(step).execute(&state.db).await;
            if let Some(operation) = operation {
                let _ =
                    sqlx::query("INSERT INTO operation_events(operation_id,message) VALUES($1,$2)")
                        .bind(operation)
                        .bind(&message)
                        .execute(&state.db)
                        .await;
                if exhausted {
                    mark_operation_failed(&state.db, operation, &message).await;
                }
            }
            return;
        }
        let _ = sqlx::query("UPDATE deprovisioning_steps SET status='succeeded',finished_at=now(),updated_at=now() WHERE tenant_id=$1 AND step=$2")
            .bind(tenant_id).bind(step).execute(&state.db).await;
        if let Some(operation) = operation {
            let _ = sqlx::query("INSERT INTO operation_events(operation_id,message) VALUES($1,$2)")
                .bind(operation)
                .bind(format!("{step} completed."))
                .execute(&state.db)
                .await;
        }
    }
    let _ = sqlx::query("UPDATE tenants SET lifecycle_state='retained',updated_at=now() WHERE id=$1 AND lifecycle_state='deprovisioning'")
        .bind(tenant_id).execute(&state.db).await;
    let _ = sqlx::query("UPDATE operations SET status='succeeded',finished_at=now() WHERE id=(SELECT id FROM operations WHERE tenant_id=$1 AND kind='tenant_deprovision' AND status='running' ORDER BY created_at DESC LIMIT 1)")
        .bind(tenant_id).execute(&state.db).await;
}

/// Remove the tenant's runtime configuration so the runtime stops serving it on
/// its next reload. The configuration itself stays in PostgreSQL.
async fn unrender_tenant(state: &Shared, tenant_id: Uuid) -> Result<()> {
    let (domain, aliases, directory, runtime_host) = sqlx::query_as::<_, (String, Vec<String>, String, String)>(
        "SELECT t.domain,t.aliases,r.tenant_directory,i.address FROM tenants t JOIN runtimes r ON r.id=t.runtime_id JOIN instances i ON i.id=r.instance_id WHERE t.id=$1",
    )
    .bind(tenant_id)
    .fetch_one(&state.db)
    .await?;
    let configured_host = std::env::var("CONTROL_RUNTIME_HOST").unwrap_or_default();
    anyhow::ensure!(
        !configured_host.is_empty() && configured_host == runtime_host,
        "runtime is managed elsewhere"
    );
    let directory = PathBuf::from(directory);
    for host in std::iter::once(&domain).chain(aliases.iter()) {
        anyhow::ensure!(is_safe_domain(host), "invalid tenant domain");
        let file_name = tenant_file_name(host);
        anyhow::ensure!(
            is_safe_tenant_file_name(&file_name),
            "invalid tenant file name"
        );
        match tokio::fs::remove_file(directory.join(file_name)).await {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

/// Confirm the runtime no longer answers for the tenant's domain.
async fn verify_tenant_offline(state: &Shared, tenant_id: Uuid) -> Result<()> {
    let (domain, bind_address) = sqlx::query_as::<_, (String, String)>("SELECT t.domain,r.bind_address FROM tenants t JOIN runtimes r ON r.id=t.runtime_id WHERE t.id=$1")
        .bind(tenant_id).fetch_one(&state.db).await?;
    let url = format!("http://{bind_address}/api/v2/instance");
    for _ in 0..30 {
        // A runtime left with a single tenant answers every host with that
        // tenant, so only a response naming this domain means it is served.
        let serving = match state.client.get(&url).header("Host", &domain).send().await {
            Ok(response) if response.status().is_success() => response
                .json::<serde_json::Value>()
                .await
                .ok()
                .and_then(|body| body.get("domain")?.as_str().map(str::to_owned))
                .is_none_or(|served| served.eq_ignore_ascii_case(&domain)),
            Ok(_) => false,
            // An unreachable runtime is not serving it either, but that is not
            // evidence the reload took; ask again.
            Err(_) => true,
        };
        if !serving {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    anyhow::bail!("tenant is still served")
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
    .fetch_optional(&state.db)
    .await?;
    // Tenants without an owner, such as operator-created ones, have no owner
    // step to run.
    let Some(status) = status else {
        return Ok(false);
    };
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
    for step in [
        "database", "migrate", "render", "reload", "health", "routing", "owner",
    ] {
        let ready = match step {
            "routing" => tenant_routing_ready(&state.db, tenant_id).await,
            "owner" => tenant_identity_settled(&state.db, tenant_id).await,
            _ => Ok(true),
        };
        match ready {
            Ok(true) => {}
            Ok(false) => return,
            Err(_) => {
                fail_provisioning(&state.db, tenant_id, step).await;
                return;
            }
        }
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
            "routing" => Ok(()),
            "owner" => create_tenant_owner(state, tenant_id).await,
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

async fn tenant_routing_ready(db: &PgPool, tenant_id: Uuid) -> Result<bool> {
    let statuses = sqlx::query_scalar::<_, String>(
        "SELECT status FROM tenant_domains WHERE tenant_id=$1 AND kind='provided'",
    )
    .bind(tenant_id)
    .fetch_all(db)
    .await?;
    anyhow::ensure!(!statuses.is_empty(), "tenant has no provided hostname");
    anyhow::ensure!(
        !statuses.iter().any(|status| status == "failed"),
        "provided hostname routing failed"
    );
    Ok(statuses.iter().all(|status| status == "active"))
}

/// Whether the owner can be created: a custom domain requested with the
/// instance becomes its ActivityPub identity only while no local account
/// exists, so the owner waits until that domain connects or fails. Tenants with
/// no owner left to create never wait.
async fn tenant_identity_settled(db: &PgPool, tenant_id: Uuid) -> Result<bool> {
    Ok(sqlx::query_scalar::<_, bool>(
        "SELECT NOT EXISTS(SELECT 1 FROM provisioning_steps WHERE tenant_id=$1 AND step='owner' AND status<>'succeeded') OR NOT EXISTS(SELECT 1 FROM tenant_domains WHERE tenant_id=$1 AND kind='custom' AND status='pending')",
    )
    .bind(tenant_id)
    .fetch_one(db)
    .await?)
}

/// Create the customer's Owner account, as `tootctl accounts create --role
/// Owner --confirmed --approve` would. Its random password is discarded: the
/// customer area issues one on request, so no copy is ever kept.
async fn create_tenant_owner(state: &Shared, tenant_id: Uuid) -> Result<()> {
    let (username, email, database_url) = sqlx::query_as::<_, (String, String, String)>(
        "SELECT t.owner_username,c.email,tc.database_url FROM tenants t JOIN customers c ON c.id=t.customer_id JOIN tenant_configuration tc ON tc.tenant_id=t.id WHERE t.id=$1 AND t.owner_username IS NOT NULL",
    )
    .bind(tenant_id)
    .fetch_one(&state.db)
    .await?;

    // A retry after the command succeeded but before the step was recorded
    // finds the owner already there. Sign-ups are closed until then, so the
    // username can only belong to someone else if something is badly wrong.
    let tenant_db = PgPoolOptions::new()
        .max_connections(1)
        .connect(&database_url)
        .await?;
    let existing = sqlx::query_scalar::<_, String>(
        "SELECT u.email FROM public.users u JOIN public.accounts a ON a.id=u.account_id WHERE lower(a.username)=lower($1) AND a.domain IS NULL",
    )
    .bind(&username)
    .fetch_optional(&tenant_db)
    .await;
    tenant_db.close().await;
    if let Some(existing) = existing? {
        anyhow::ensure!(
            existing.eq_ignore_ascii_case(&email),
            "owner username belongs to another account"
        );
        return Ok(());
    }

    tenant_account_command(
        state,
        tenant_id,
        &[
            "accounts",
            "create",
            &username,
            &format!("--email={email}"),
            "--confirmed",
            "--approve",
            "--role=Owner",
        ],
    )
    .await
    .map(drop)
}

/// Run an `eunha accounts` command that prints a password against one tenant,
/// with the runtime's current release, and return that password.
///
/// Like every other runtime command, its output is never persisted or logged.
async fn tenant_account_command(state: &Shared, tenant_id: Uuid, args: &[&str]) -> Result<String> {
    let (domain, tenant_directory, runtime_host) = sqlx::query_as::<_, (String, String, String)>(
        "SELECT t.domain,r.tenant_directory,i.address FROM tenants t JOIN runtimes r ON r.id=t.runtime_id JOIN instances i ON i.id=r.instance_id WHERE t.id=$1",
    )
    .bind(tenant_id)
    .fetch_one(&state.db)
    .await?;
    let configured_host = std::env::var("CONTROL_RUNTIME_HOST").unwrap_or_default();
    anyhow::ensure!(
        !configured_host.is_empty() && configured_host == runtime_host,
        "runtime is managed elsewhere"
    );
    anyhow::ensure!(is_safe_domain(&domain), "invalid tenant domain");
    let runtime_root = PathBuf::from(&tenant_directory)
        .parent()
        .context("runtime directory has no parent")?
        .to_path_buf();
    let output = tokio::time::timeout(
        Duration::from_secs(60),
        tokio::process::Command::new(runtime_root.join("current/eunha"))
            .arg("--tenants")
            .arg(&tenant_directory)
            .args(args)
            .arg(format!("--instance={domain}"))
            .current_dir(runtime_root.join("current"))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .context("account command timed out")??;
    anyhow::ensure!(output.status.success(), "account command failed");
    printed_password(&String::from_utf8_lossy(&output.stdout))
        .context("account command printed no password")
}

/// The password `tootctl`-style account commands print after `OK`.
fn printed_password(stdout: &str) -> Option<String> {
    stdout
        .lines()
        .find_map(|line| line.trim().strip_prefix("New password: "))
        .filter(|password| password.len() >= 16 && password.bytes().all(|b| b.is_ascii_hexdigit()))
        .map(str::to_owned)
}

fn is_valid_owner_username(username: &str) -> bool {
    (1..=30).contains(&username.len())
        && username
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

async fn customer_reset_owner_password(
    State(s): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let (customer_id, _) = customer(&headers, &s.db).await?;
    let owner = sqlx::query_as::<_, (Option<String>, bool, Uuid)>(
        "SELECT t.owner_username,t.lifecycle_state='ready' AND EXISTS(SELECT 1 FROM provisioning_steps p WHERE p.tenant_id=t.id AND p.step='owner' AND p.status='succeeded'),t.runtime_id FROM tenants t WHERE t.id=$1 AND t.customer_id=$2",
    )
    .bind(id)
    .bind(customer_id)
    .fetch_optional(&s.db)
    .await?
    .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "Instance not found".into()))?;
    let (Some(username), true, runtime_id) = owner else {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "The admin account is not ready yet.".into(),
        ));
    };

    let operation_id = Uuid::new_v4();
    sqlx::query("INSERT INTO operations(id,tenant_id,runtime_id,kind,status,requested_by_customer,detail,started_at) VALUES($1,$2,$3,'owner_password_reset','running',$4,jsonb_build_object('username',$5::text),now())")
        .bind(operation_id).bind(id).bind(runtime_id).bind(customer_id).bind(&username).execute(&s.db).await?;
    let password = match tenant_account_command(
        &s,
        id,
        &["accounts", "modify", &username, "--reset-password"],
    )
    .await
    {
        Ok(password) => password,
        Err(_) => {
            let message = "The admin password could not be reset.";
            mark_operation_failed(&s.db, operation_id, message).await;
            return Err(ApiError(StatusCode::BAD_GATEWAY, message.into()));
        }
    };
    sqlx::query("UPDATE tenants SET owner_password_issued_at=now(),updated_at=now() WHERE id=$1")
        .bind(id)
        .execute(&s.db)
        .await?;
    sqlx::query("UPDATE operations SET status='succeeded',finished_at=now() WHERE id=$1")
        .bind(operation_id)
        .execute(&s.db)
        .await?;
    sqlx::query("INSERT INTO operation_events(operation_id,message) VALUES($1,'Admin password reset; every session was signed out.')")
        .bind(operation_id)
        .execute(&s.db)
        .await?;
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        Json(serde_json::json!({ "username": username, "password": password })),
    )
        .into_response())
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
    let (service_label, router_enabled, tenant_directory) = sqlx::query_as::<_, (String, bool, String)>(
        "SELECT r.service_label,r.router_enabled,r.tenant_directory FROM tenants t JOIN runtimes r ON r.id=t.runtime_id WHERE t.id=$1",
    )
    .bind(tenant_id)
    .fetch_one(&state.db)
    .await?;
    let service_label =
        active_runtime_label(&service_label, router_enabled, &tenant_directory).await?;
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

async fn active_runtime_label(
    service_label: &str,
    router_enabled: bool,
    tenant_directory: &str,
) -> Result<String> {
    anyhow::ensure!(
        is_safe_launchd_label(service_label),
        "invalid service label"
    );
    if !router_enabled {
        return Ok(service_label.to_owned());
    }
    let state = PathBuf::from(tenant_directory)
        .parent()
        .context("runtime directory has no parent")?
        .join("router/active-slot");
    let slot = tokio::fs::read_to_string(state).await?;
    let slot = slot.trim();
    anyhow::ensure!(
        matches!(slot, "blue" | "green"),
        "invalid active runtime slot"
    );
    Ok(format!("{service_label}.{slot}"))
}

async fn healthcheck_provisioned_tenant(state: &Shared, tenant_id: Uuid) -> Result<()> {
    let (domain, bind_address) = sqlx::query_as::<_, (String, String)>("SELECT t.domain,r.bind_address FROM tenants t JOIN runtimes r ON r.id=t.runtime_id WHERE t.id=$1")
        .bind(tenant_id).fetch_one(&state.db).await?;
    healthcheck_url(state, &bind_address, &domain).await
}

async fn healthcheck_host(state: &Shared, tenant_id: Uuid, domain: &str) -> Result<()> {
    let bind_address = sqlx::query_scalar::<_, String>(
        "SELECT r.bind_address FROM tenants t JOIN runtimes r ON r.id=t.runtime_id WHERE t.id=$1",
    )
    .bind(tenant_id)
    .fetch_one(&state.db)
    .await?;
    healthcheck_url(state, &bind_address, domain).await
}

async fn healthcheck_url(state: &Shared, bind_address: &str, domain: &str) -> Result<()> {
    let url = format!("http://{bind_address}/api/v2/instance");
    for _ in 0..30 {
        if let Ok(response) = state.client.get(&url).header("Host", domain).send().await
            && response.status().is_success()
            && response
                .json::<serde_json::Value>()
                .await
                .ok()
                .and_then(|body| body.get("domain")?.as_str().map(str::to_owned))
                .is_some_and(|served_domain| served_domain.eq_ignore_ascii_case(domain))
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
    Ok(Json(sqlx::query_as("SELECT r.id,r.name,i.name instance_name,i.address instance_address,r.bind_address,r.service_label,r.tenant_directory,r.origin_hostname,r.router_enabled,r.blue_bind_address,r.green_bind_address,(SELECT count(*) FROM tenants WHERE runtime_id=r.id) tenant_count,r.auto_deploy_enabled,r.deployed_ref,o.id release_operation_id,o.status release_status,e.message release_message FROM runtimes r JOIN instances i ON i.id=r.instance_id LEFT JOIN LATERAL (SELECT id,status FROM operations WHERE runtime_id=r.id AND kind='runtime_release' ORDER BY created_at DESC LIMIT 1) o ON true LEFT JOIN LATERAL (SELECT string_agg(message,E'\\n' ORDER BY id) message FROM operation_events WHERE operation_id=o.id) e ON true ORDER BY i.name,r.name").fetch_all(&s.db).await?))
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
    let row = sqlx::query_as::<_, (String, String, String, String, String, bool, String, String, String)>(
        "SELECT r.name,i.address,r.tenant_directory,r.service_label,r.bind_address,r.router_enabled,r.blue_bind_address,r.green_bind_address,(SELECT domain FROM tenants WHERE runtime_id=r.id AND lifecycle_state='ready' ORDER BY created_at LIMIT 1) FROM runtimes r JOIN instances i ON i.id=r.instance_id WHERE r.id=$1",
    )
    .bind(runtime_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "Runtime not found.".into()))?;
    if !is_safe_runtime_name(&row.0)
        || !is_safe_remote_host(&row.1)
        || !is_safe_absolute_path(FsPath::new(&row.2))
        || !is_safe_launchd_label(&row.3)
        || !is_safe_bind_address(&row.4)
        || !is_safe_bind_address(&row.6)
        || !is_safe_bind_address(&row.7)
        || !is_safe_domain(&row.8)
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
    runtime: (
        String,
        String,
        String,
        String,
        String,
        bool,
        String,
        String,
        String,
    ),
) {
    let (
        _,
        host,
        tenant_directory,
        service_label,
        bind_address,
        router_enabled,
        blue,
        green,
        health_host,
    ) = runtime;
    let heartbeat = tokio::spawn(release_heartbeat(state.db.clone(), operation_id));
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
                deployment_id,
                &host,
                &tenant_directory,
                &service_label,
                &bind_address,
                router_enabled,
                &blue,
                &green,
                &health_host,
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
                &format!("Release failed: {message}"),
            )
            .await;
            let _ = sqlx::query("UPDATE runtime_deployments SET status='failed',error=$1,finished_at=now() WHERE id=$2").bind(&message).bind(deployment_id).execute(&state.db).await;
            let _ = fail_release(&state.db, operation_id, &message).await;
        }
    }
    heartbeat.abort();
}

/// How often a running release proves its process is still working on it, and
/// how long a silence means that process is gone.
const RELEASE_HEARTBEAT_SECONDS: u64 = 15;
const RELEASE_ABANDONED_AFTER: &str = "2 minutes";

async fn release_heartbeat(db: PgPool, operation_id: Uuid) {
    loop {
        let _ = sqlx::query(
            "UPDATE operations SET heartbeat_at=now() WHERE id=$1 AND status='running'",
        )
        .bind(operation_id)
        .execute(&db)
        .await;
        tokio::time::sleep(Duration::from_secs(RELEASE_HEARTBEAT_SECONDS)).await;
    }
}

/// Fail releases whose process stopped working on them, which otherwise hold
/// the fleet's release lock forever. What to tell the operator depends on how
/// far the release got: before transfer the runtime was never touched.
async fn fail_abandoned_releases(db: &PgPool) {
    let abandoned = sqlx::query_as::<_, (Uuid, Option<Uuid>, Option<String>)>(&format!(
        "SELECT o.id,d.id,d.status FROM operations o LEFT JOIN runtime_deployments d ON d.operation_id=o.id WHERE o.kind='runtime_release' AND o.status IN ('pending','running') AND COALESCE(o.heartbeat_at,o.started_at,o.created_at) < now()-interval '{RELEASE_ABANDONED_AFTER}'"
    ))
    .fetch_all(db)
    .await;
    let Ok(abandoned) = abandoned else {
        return;
    };
    for (operation_id, deployment_id, stage) in abandoned {
        let message = match stage.as_deref() {
            None | Some("queued" | "building") => {
                "Release was interrupted while building, most likely by a control-plane restart. The runtime was not changed; retrying is safe."
            }
            Some(_) => {
                "Release was interrupted during rollout, most likely by a control-plane restart. Check the runtime's active slot and health before retrying."
            }
        };
        let failed = sqlx::query(
            "UPDATE operations SET status='failed',error=$1,finished_at=now() WHERE id=$2 AND status IN ('pending','running')",
        )
        .bind(message)
        .bind(operation_id)
        .execute(db)
        .await
        .map(|result| result.rows_affected() > 0)
        .unwrap_or(false);
        if !failed {
            continue;
        }
        release_event(db, operation_id, &format!("Release failed: {message}")).await;
        if let Some(deployment_id) = deployment_id {
            let _ = sqlx::query("UPDATE runtime_deployments SET status='failed',error=$1,finished_at=now() WHERE id=$2 AND status NOT IN ('healthy','rolled_back','failed')")
                .bind(message).bind(deployment_id).execute(db).await;
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
        "Git fetch",
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
    release_event(
        db,
        operation_id,
        &format!("Resolved origin/main to release {}.", &reference[..12]),
    )
    .await;
    let artifact = root.join("releases").join(&reference);
    if artifact.join("eunha").is_file()
        && artifact.join("eunha-runtime-router").is_file()
        && artifact.join("frontend/dist/index.html").is_file()
    {
        tokio::fs::copy(
            root.join("ops/eunha-runtime-router"),
            artifact.join("eunha-runtime-router"),
        )
        .await
        .map_err(|_| "Could not refresh the runtime router helper.".to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            tokio::fs::set_permissions(
                artifact.join("eunha-runtime-router"),
                std::fs::Permissions::from_mode(0o755),
            )
            .await
            .map_err(|_| "Could not secure the runtime router helper.".to_string())?;
        }
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
        "Git checkout",
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
        "Frontend dependency installation",
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
        "Frontend build",
        "mise",
        &["exec", "--", "pnpm", "--dir", "frontend", "run", "build"],
        Some(&source),
    )
    .await?;
    command_status(
        "Eunha release build",
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
    tokio::fs::copy(
        root.join("ops/eunha-runtime-router"),
        staging.join("eunha-runtime-router"),
    )
    .await
    .map_err(|_| "Could not stage the runtime router helper.".to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(
            staging.join("eunha-runtime-router"),
            std::fs::Permissions::from_mode(0o755),
        )
        .await
        .map_err(|_| "Could not secure the runtime router helper.".to_string())?;
    }
    command_status(
        "Frontend artifact staging",
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

async fn command_status(
    label: &str,
    program: &str,
    args: &[&str],
    cwd: Option<&FsPath>,
) -> Result<(), String> {
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
        Ok(status) => Err(match status.code() {
            Some(code) => format!("{label} exited with status {code}."),
            None => format!("{label} was terminated by a signal."),
        }),
        Err(_) => Err(format!("{label} could not be started.")),
    }
}

async fn async_deploy_release(
    db: &PgPool,
    operation_id: Uuid,
    deployment_id: Uuid,
    host: &str,
    tenant_directory: &str,
    service_label: &str,
    bind_address: &str,
    router_enabled: bool,
    blue_bind_address: &str,
    green_bind_address: &str,
    health_host: &str,
    release: (String, PathBuf),
) -> Result<String, String> {
    let (reference, artifact) = release;
    let tenant = FsPath::new(tenant_directory);
    let runtime_root = tenant
        .parent()
        .ok_or_else(|| "Runtime release target is invalid.".to_string())?;
    let releases = runtime_root.join("releases");
    let staging = releases.join(format!(".{reference}.staging"));
    let stale_staging = releases.join(format!(".{reference}.stale-{}", Uuid::new_v4()));
    let superseded_release = releases.join(format!(".{reference}.superseded-{}", Uuid::new_v4()));
    let release_path = releases.join(&reference);
    let artifact_text = artifact.to_string_lossy().to_string();
    let remote_staging = shell_quote(&staging.to_string_lossy());
    let local_runtime = matches!(host, "localhost" | "127.0.0.1" | "::1");
    release_event(db, operation_id, "Preparing the runtime release directory.").await;
    if local_runtime {
        tokio::fs::create_dir_all(&releases)
            .await
            .map_err(|_| "Could not prepare the runtime release directory.".to_string())?;
        if tokio::fs::try_exists(&staging).await.unwrap_or(false) {
            tokio::fs::rename(&staging, &stale_staging)
                .await
                .map_err(|_| "Could not isolate stale release staging.".to_string())?;
        }
    } else {
        command_status(
            "Runtime directory preparation",
            "ssh",
            &[
                host,
                &format!(
                    "mkdir -p {}; if [ -e {} ]; then mv {} {}; fi",
                    shell_quote(&releases.to_string_lossy()),
                    shell_quote(&staging.to_string_lossy()),
                    shell_quote(&staging.to_string_lossy()),
                    shell_quote(&stale_staging.to_string_lossy()),
                ),
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
            "Local release transfer",
            "cp",
            &["-R", &artifact_text, &staging.to_string_lossy()],
            None,
        )
        .await
        .map_err(|_| "Could not transfer the runtime release bundle.".to_string())?;
    } else {
        command_status(
            "Remote release transfer",
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
    release_event(db, operation_id, "Release bundle transfer completed.").await;
    let _ = sqlx::query("UPDATE runtime_deployments SET status='migrating' WHERE id=$1")
        .bind(deployment_id)
        .execute(db)
        .await;
    release_event(db, operation_id, "Applying tenant database migrations.").await;
    let migration = format!(
        "set -eu; {}/eunha --tenants {} migrate",
        remote_staging,
        shell_quote(tenant_directory),
    );
    run_runtime_command(host, local_runtime, "Tenant database migration", &migration).await?;
    release_event(db, operation_id, "Tenant database migrations completed.").await;

    release_event(db, operation_id, "Publishing the immutable release bundle.").await;
    let publish = format!(
        "set -eu; rel={}; stage={}; superseded={}; if [ -e \"$rel\" ]; then mv \"$rel\" \"$superseded\"; fi; mv \"$stage\" \"$rel\"",
        shell_quote(&release_path.to_string_lossy()),
        remote_staging,
        shell_quote(&superseded_release.to_string_lossy()),
    );
    run_runtime_command(host, local_runtime, "Release bundle publication", &publish).await?;
    release_event(db, operation_id, "Immutable release bundle published.").await;

    let _ = sqlx::query("UPDATE runtime_deployments SET status='switching' WHERE id=$1")
        .bind(deployment_id)
        .execute(db)
        .await;
    let activation = if router_enabled {
        release_event(
            db,
            operation_id,
            "Starting the inactive slot, checking health, and switching the local router.",
        )
        .await;
        format!(
            "\"$rel/eunha-runtime-router\" switch \"$root\" {} {} {} {} {} \"$rel\" {}; ln -sfn \"$rel\" .current.new; mv -h .current.new current",
            shell_quote(tenant_directory),
            shell_quote(service_label),
            shell_quote(bind_address),
            shell_quote(blue_bind_address),
            shell_quote(green_bind_address),
            shell_quote(health_host),
        )
    } else {
        release_event(
            db,
            operation_id,
            "Restarting the runtime and checking health; rollback is armed.",
        )
        .await;
        format!(
            "previous=$(readlink current 2>/dev/null || true); ln -sfn \"$rel\" .current.new; mv -h .current.new current; launchctl kickstart -k gui/$(id -u)/{}; for i in $(seq 1 45); do curl -sf -m 5 -o /dev/null -H 'Host: {}' http://{}/api/v2/instance && exit 0; sleep 1; done; if [ -n \"$previous\" ]; then ln -sfn \"$previous\" .current.new; mv -h .current.new current; launchctl kickstart -k gui/$(id -u)/{}; fi; exit 1",
            service_label, health_host, bind_address, service_label
        )
    };
    let script = format!(
        "set -eu; rel={}; root={}; cd \"$root\"; {}",
        shell_quote(&release_path.to_string_lossy()),
        shell_quote(&runtime_root.to_string_lossy()),
        activation,
    );
    let switch_result = run_runtime_command(
        host,
        local_runtime,
        "Runtime switch and health check",
        &script,
    )
    .await;
    if let Err(message) = switch_result {
        let detail = if router_enabled {
            describe_router_failure(&message)
        } else {
            format!("{message} The previous release was restored when available.")
        };
        release_event(db, operation_id, &detail).await;
        return Err(detail);
    }
    release_event(
        db,
        operation_id,
        "Runtime switch and post-switch health check completed.",
    )
    .await;
    Ok(reference)
}

async fn run_runtime_command(
    host: &str,
    local_runtime: bool,
    label: &str,
    script: &str,
) -> Result<(), String> {
    if local_runtime {
        command_status(label, "/bin/sh", &["-c", script], None).await
    } else {
        command_status(label, "ssh", &[host, script], None).await
    }
}

fn describe_router_failure(message: &str) -> String {
    let phase = if message.contains("status 71") {
        "router prerequisites"
    } else if message.contains("status 72") {
        "inactive-slot startup or health check"
    } else if message.contains("status 73") {
        "HAProxy configuration or reload"
    } else if message.contains("status 74") {
        "post-switch health check and rollback"
    } else if message.contains("status 75") {
        "old-slot drain or shutdown"
    } else {
        "runtime switching"
    };
    format!(
        "Runtime switch failed during {phase}. {message} The previously active slot was retained or restored when possible."
    )
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\\"'\\\"'"))
}

async fn release_recovery_reconciler(state: Shared) {
    loop {
        fail_abandoned_releases(&state.db).await;
        tokio::time::sleep(Duration::from_secs(30)).await;
    }
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

fn is_safe_bind_address(address: &str) -> bool {
    address.parse::<std::net::SocketAddr>().is_ok()
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
    let row = sqlx::query_as::<_, (SqlJson<StoredTenant>, SqlJson<StoredConfig>, String, String, String, String, String, bool, i64)>(
        "SELECT to_jsonb(t),to_jsonb(c),b.bucket_name,i.address,r.tenant_directory,r.service_label,r.bind_address,r.router_enabled,t.config_revision FROM tenants t JOIN tenant_configuration c ON c.tenant_id=t.id JOIN runtimes r ON r.id=t.runtime_id JOIN instances i ON i.id=r.instance_id JOIN r2_buckets b ON b.tenant_id=t.id WHERE t.id=$1",
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
        router_enabled,
        revision,
    ) = row;
    if matches!(
        tenant.lifecycle_state.as_str(),
        "deprovisioning" | "retained" | "purged"
    ) {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "A deprovisioned tenant is not rendered; restore it first.".into(),
        ));
    }
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
    let service_label = active_runtime_label(&service_label, router_enabled, &tenant_directory)
        .await
        .map_err(ApiError::from)?;

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
    use super::{
        DomainDnsTargetError, REDACTED, domain_reconcile_error_message, is_valid_owner_username,
        printed_password, redact_connection_url, tenant_redis_key_prefix,
    };
    use uuid::Uuid;

    #[test]
    fn account_commands_print_their_password_after_ok() {
        let stdout = "2026-09-13T10:00:00Z  INFO eunha: connected\nOK\nNew password: 0123456789abcdef0123456789abcdef\n";
        assert_eq!(
            printed_password(stdout).as_deref(),
            Some("0123456789abcdef0123456789abcdef")
        );
        assert_eq!(printed_password("OK\n"), None);
        assert_eq!(printed_password("New password: \n"), None);
        assert_eq!(
            printed_password("New password: not hex at all, sorry\n"),
            None
        );
    }

    #[test]
    fn owner_usernames_follow_mastodons_local_username_rules() {
        assert!(is_valid_owner_username("gardener"));
        assert!(is_valid_owner_username("Garden_Keeper_2"));
        assert!(is_valid_owner_username(&"a".repeat(30)));
        assert!(!is_valid_owner_username(""));
        assert!(!is_valid_owner_username(&"a".repeat(31)));
        assert!(!is_valid_owner_username("garden-keeper"));
        assert!(!is_valid_owner_username("garden.keeper"));
        assert!(!is_valid_owner_username("--help"));
    }

    #[test]
    fn generated_redis_prefix_uses_eunhas_safe_alphabet() {
        let id = Uuid::parse_str("9f81d608-3288-4224-9ef3-cfd4e33acf0e").unwrap();
        let prefix = tenant_redis_key_prefix(id);
        assert_eq!(prefix, "t_9f81d608328842249ef3cfd4e33acf0e_");
        assert!(
            prefix
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        );
    }

    #[test]
    fn explains_a_cloudflare_cname_mismatch_without_exposing_provider_details() {
        let error = anyhow::Error::new(DomainDnsTargetError {
            cname_target: "customers.eunha.space".into(),
        });
        assert_eq!(
            domain_reconcile_error_message(&error),
            "Cloudflare cannot validate this hostname because its DNS does not point to customers.eunha.space. Update the CNAME record; existing routing remains unchanged."
        );
    }

    #[test]
    fn keeps_unclassified_reconciliation_failures_generic() {
        assert_eq!(
            domain_reconcile_error_message(&anyhow::anyhow!("request included sensitive context")),
            "Hostname routing could not be reconciled; existing routing remains unchanged."
        );
    }

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
