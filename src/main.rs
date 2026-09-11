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
use std::{path::PathBuf, sync::Arc, time::Duration};
use time::OffsetDateTime;
use tower_http::{
    services::{ServeDir, ServeFile},
    trace::TraceLayer,
};
use uuid::Uuid;

#[derive(Parser)]
struct Args {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Create or replace an operator password from EUNHA_SPACE_ADMIN_PASSWORD.
    SetOperator { username: String },
    /// Import an existing tenant configuration without changing its runtime.
    Import {
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        runtime_name: String,
        #[arg(long)]
        host: String,
        #[arg(long)]
        tenant_directory: PathBuf,
        #[arg(long)]
        service_label: String,
        #[arg(long)]
        binary_path: PathBuf,
    },
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
    tenant_file_name: String,
    runtime_name: String,
    bind_address: String,
    #[serde(with = "time::serde::rfc3339")]
    updated_at: OffsetDateTime,
}

#[derive(Serialize, sqlx::FromRow)]
struct OperationSummary {
    id: Uuid,
    domain: Option<String>,
    kind: String,
    status: String,
    error: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
}

#[derive(Deserialize)]
struct Login {
    username: String,
    password: String,
}

#[derive(Deserialize, Serialize)]
struct TenantToml {
    database_url: String,
    #[serde(default)] database_pool: DatabasePoolToml,
    redis_url: String,
    #[serde(default)] redis_coordination_url: Option<String>,
    #[serde(default)] redis_key_prefix: String,
    #[serde(default = "default_true")] redis_process_metrics: bool,
    bind_address: String,
    media_storage: MediaStorageToml,
    #[serde(default)] smtp: Option<SmtpToml>,
    resend: ResendToml,
    instance: InstanceToml,
    #[serde(default)] active_record_encryption: Option<EncryptionToml>,
    #[serde(default)] software_update_url: Option<String>,
    #[serde(default)] allowed_private_networks: Vec<String>,
    #[serde(default)] sign_integrity_proofs: bool,
    #[serde(default)] workers: WorkersToml,
    #[serde(default)] limits: LimitsToml,
}
fn default_true() -> bool { true }
#[derive(Deserialize, Serialize)] struct DatabasePoolToml { #[serde(default = "d20")] max_connections:i32, #[serde(default)] min_connections:i32, #[serde(default = "d30")] acquire_timeout_seconds:i64, #[serde(default = "d600")] idle_timeout_seconds:i64 }
fn d20()->i32{20} fn d30()->i64{30} fn d600()->i64{600}
impl Default for DatabasePoolToml { fn default()->Self{Self{max_connections:20,min_connections:0,acquire_timeout_seconds:30,idle_timeout_seconds:600}} }
#[derive(Deserialize, Serialize)] struct MediaStorageToml { #[serde(skip_serializing_if="Option::is_none")] bucket:Option<String>, region:String, endpoint:Option<String>, access_key_id:String, secret_access_key:String, base_url:String }
#[derive(Deserialize, Serialize)] struct ResendToml { api_key:String, from:String }
#[derive(Deserialize, Serialize)] struct SmtpToml { host:String, port:i32, username:String, password:String, from:String }
#[derive(Deserialize, Serialize)] struct EncryptionToml { primary_key:String, key_derivation_salt:String }
#[derive(Deserialize, Serialize)] struct InstanceToml { domain:String,title:String,#[serde(default)]description:String,#[serde(default)]short_description:String,contact_email:Option<String>,#[serde(default="default_true")]registrations_open:bool,#[serde(default)]approval_required:bool,vapid_private_key:String,vapid_public_key:String,icon_url:Option<String>,#[serde(default)]privacy_policy:String,#[serde(default)]terms_of_service:String }
#[derive(Deserialize, Serialize)] struct WorkersToml { #[serde(default="one")]delivery_workers:i32,#[serde(default="d50")]delivery_batch:i64,#[serde(default="d16")]delivery_concurrency:i32,#[serde(default="one")]inbox_workers:i32,#[serde(default="d20i")]inbox_batch:i64,#[serde(default="d4")]inbox_concurrency:i32,#[serde(default="d30")]queue_idle_poll_seconds:i64,#[serde(default="d256")]process_delivery_concurrency:i32 }
fn one()->i32{1} fn d50()->i64{50} fn d16()->i32{16} fn d20i()->i64{20} fn d4()->i32{4} fn d256()->i32{256}
impl Default for WorkersToml{fn default()->Self{Self{delivery_workers:1,delivery_batch:50,delivery_concurrency:16,inbox_workers:1,inbox_batch:20,inbox_concurrency:4,queue_idle_poll_seconds:30,process_delivery_concurrency:256}}}
#[derive(Deserialize, Serialize, Default)] struct LimitsToml { max_concurrent_requests:Option<i32>,process_max_tenants:Option<i32>,process_database_connections:Option<i64> }

#[derive(Deserialize)]
struct StoredConfig { database_url:String, database_pool_max_connections:i32, database_pool_min_connections:i32, database_pool_acquire_timeout_seconds:i64, database_pool_idle_timeout_seconds:i64, redis_url:String, redis_coordination_url:Option<String>, redis_key_prefix:String, redis_process_metrics:bool, resend_api_key:String, resend_from:String, smtp_host:Option<String>,smtp_port:Option<i32>,smtp_username:Option<String>,smtp_password:Option<String>,smtp_from:Option<String>,active_record_encryption_primary_key:Option<String>,active_record_encryption_key_derivation_salt:Option<String>, media_storage_region:String, media_storage_endpoint:Option<String>, media_storage_access_key_id:String, media_storage_secret_access_key:String, media_storage_base_url:String, software_update_url:Option<String>, allowed_private_networks:Vec<String>, sign_integrity_proofs:bool, delivery_workers:i32,delivery_batch:i64,delivery_concurrency:i32,inbox_workers:i32,inbox_batch:i64,inbox_concurrency:i32,queue_idle_poll_seconds:i64,process_delivery_concurrency:i32,max_concurrent_requests:Option<i32>,process_max_tenants:Option<i32>,process_database_connections:Option<i64> }
#[derive(Deserialize)]
struct StoredTenant { domain:String,title:String,description:String,short_description:String,contact_email:Option<String>,registrations_open:bool,approval_required:bool,vapid_private_key:Option<String>,vapid_public_key:Option<String>,icon_url:Option<String>,privacy_policy:String,terms_of_service:String }
fn required(value: Option<String>, name: &str) -> Result<String> { value.with_context(|| format!("typed configuration is missing {name}")) }
fn typed_toml(tenant: StoredTenant, config: StoredConfig, bucket: String, bind_address: String) -> Result<TenantToml> { let smtp=match(config.smtp_host,config.smtp_port,config.smtp_username,config.smtp_password,config.smtp_from){(None,None,None,None,None)=>None,(h,p,u,pw,f)=>Some(SmtpToml{host:required(h,"smtp_host")?,port:p.context("typed configuration is missing smtp_port")?,username:required(u,"smtp_username")?,password:required(pw,"smtp_password")?,from:required(f,"smtp_from")?})};let encryption=match(config.active_record_encryption_primary_key,config.active_record_encryption_key_derivation_salt){(None,None)=>None,(p,s)=>Some(EncryptionToml{primary_key:required(p,"active_record_encryption_primary_key")?,key_derivation_salt:required(s,"active_record_encryption_key_derivation_salt")?})}; Ok(TenantToml{database_url:config.database_url,database_pool:DatabasePoolToml{max_connections:config.database_pool_max_connections,min_connections:config.database_pool_min_connections,acquire_timeout_seconds:config.database_pool_acquire_timeout_seconds,idle_timeout_seconds:config.database_pool_idle_timeout_seconds},redis_url:config.redis_url,redis_coordination_url:config.redis_coordination_url,redis_key_prefix:config.redis_key_prefix,redis_process_metrics:config.redis_process_metrics,bind_address,media_storage:MediaStorageToml{bucket:Some(bucket),region:config.media_storage_region,endpoint:config.media_storage_endpoint,access_key_id:config.media_storage_access_key_id,secret_access_key:config.media_storage_secret_access_key,base_url:config.media_storage_base_url},smtp,resend:ResendToml{api_key:config.resend_api_key,from:config.resend_from},instance:InstanceToml{domain:tenant.domain,title:tenant.title,description:tenant.description,short_description:tenant.short_description,contact_email:tenant.contact_email,registrations_open:tenant.registrations_open,approval_required:tenant.approval_required,vapid_private_key:required(tenant.vapid_private_key,"vapid_private_key")?,vapid_public_key:required(tenant.vapid_public_key,"vapid_public_key")?,icon_url:tenant.icon_url,privacy_policy:tenant.privacy_policy,terms_of_service:tenant.terms_of_service},active_record_encryption:encryption,software_update_url:config.software_update_url,allowed_private_networks:config.allowed_private_networks,sign_integrity_proofs:config.sign_integrity_proofs,workers:WorkersToml{delivery_workers:config.delivery_workers,delivery_batch:config.delivery_batch,delivery_concurrency:config.delivery_concurrency,inbox_workers:config.inbox_workers,inbox_batch:config.inbox_batch,inbox_concurrency:config.inbox_concurrency,queue_idle_poll_seconds:config.queue_idle_poll_seconds,process_delivery_concurrency:config.process_delivery_concurrency},limits:LimitsToml{max_concurrent_requests:config.max_concurrent_requests,process_max_tenants:config.process_max_tenants,process_database_connections:config.process_database_connections}}) }

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
        Some(Command::Import {
            config,
            runtime_name,
            host,
            tenant_directory,
            service_label,
            binary_path,
        }) => {
            return import(
                &db,
                config,
                runtime_name,
                host,
                tenant_directory,
                service_label,
                binary_path,
            )
            .await;
        }
        None => {}
    }

    let state = Arc::new(AppState {
        db,
        client: reqwest::Client::builder()
            .timeout(Duration::from_secs(4))
            .build()?,
    });
    tokio::spawn(health_reconciler(state.clone()));
    let api = Router::new()
        .route("/session", post(login).delete(logout))
        .route("/session/me", get(me))
        .route("/tenants", get(tenants))
        .route("/tenants/{id}/check", post(check_tenant))
        .route("/tenants/{id}/render", post(render_tenant))
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

async fn import(
    db: &PgPool,
    config_path: PathBuf,
    runtime_name: String,
    host: String,
    tenant_directory: PathBuf,
    service_label: String,
    binary_path: PathBuf,
) -> Result<()> {
    let raw = tokio::fs::read_to_string(&config_path).await?;
    let mut parsed: toml::Value = toml::from_str(&raw).context("tenant config is not valid TOML")?;
    let instance = parsed
        .get("instance")
        .and_then(toml::Value::as_table)
        .context("missing [instance]")?;
    let domain = instance
        .get("domain")
        .and_then(toml::Value::as_str)
        .context("missing instance.domain")?
        .to_owned();
    let title = instance
        .get("title")
        .and_then(toml::Value::as_str)
        .unwrap_or(&domain)
        .to_owned();
    let bind = parsed
        .get("bind_address")
        .and_then(toml::Value::as_str)
        .context("missing bind_address")?
        .to_owned();
    let bucket_name = take_r2_bucket(&mut parsed)?;
    let normalized_config = toml::to_string(&parsed).context("serializing normalized tenant config")?;
    let runtime_id = Uuid::new_v4();
    let mut tx = db.begin().await?;
    let runtime_id: Uuid = sqlx::query_scalar("INSERT INTO runtimes(id,name,host,tenant_directory,service_label,binary_path,bind_address) VALUES($1,$2,$3,$4,$5,$6,$7) ON CONFLICT(name) DO UPDATE SET host=excluded.host,tenant_directory=excluded.tenant_directory,service_label=excluded.service_label,binary_path=excluded.binary_path,bind_address=excluded.bind_address RETURNING id")
        .bind(runtime_id).bind(&runtime_name).bind(&host).bind(tenant_directory.to_string_lossy().as_ref()).bind(&service_label).bind(binary_path.to_string_lossy().as_ref()).bind(bind).fetch_one(&mut *tx).await?;
    let tenant_id = Uuid::new_v4();
    let tenant_file_name = format!("{}.toml", domain.replace('.', "-"));
    let imported_from = config_path.to_string_lossy().to_string();
    let id: Uuid = sqlx::query_scalar("INSERT INTO tenants(id,domain,title,runtime_id,desired_state,observed_state,config_toml,tenant_file_name,imported_from) VALUES($1,$2,$3,$4,'running','unknown',$5,$6,$7) ON CONFLICT(domain) DO UPDATE SET title=excluded.title,runtime_id=excluded.runtime_id,config_toml=excluded.config_toml,tenant_file_name=excluded.tenant_file_name,config_revision=tenants.config_revision+1,updated_at=now() RETURNING id")
        .bind(tenant_id).bind(&domain).bind(title).bind(runtime_id).bind(normalized_config).bind(tenant_file_name).bind(imported_from).fetch_one(&mut *tx).await?;
    sqlx::query("INSERT INTO r2_buckets(tenant_id,bucket_name) VALUES($1,$2) ON CONFLICT(tenant_id) DO UPDATE SET bucket_name=excluded.bucket_name,lifecycle_state='ready',updated_at=now()")
        .bind(id).bind(bucket_name).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO operations(id,tenant_id,kind,status,detail,finished_at) VALUES($1,$2,'import','succeeded',jsonb_build_object('source',$3),now())")
        .bind(Uuid::new_v4()).bind(id).bind(config_path.to_string_lossy().as_ref()).execute(&mut *tx).await?;
    tx.commit().await?;
    println!("Imported {domain} as {id}; no runtime files were changed.");
    Ok(())
}

fn take_r2_bucket(config: &mut toml::Value) -> Result<String> {
    let media_storage = config
        .get_mut("media_storage")
        .and_then(toml::Value::as_table_mut)
        .context("missing [media_storage]")?;
    media_storage
        .remove("bucket")
        .and_then(|value| value.as_str().map(str::to_owned))
        .context("missing media_storage.bucket")
}

/* async fn normalize_r2_buckets(db: &PgPool) -> Result<()> {
    let tenants = sqlx::query_as::<_, (Uuid, String)>(
        "SELECT t.id,t.config_toml FROM tenants t LEFT JOIN r2_buckets b ON b.tenant_id=t.id WHERE b.tenant_id IS NULL",
    )
    .fetch_all(db)
    .await?;
    for (tenant_id, config_toml) in tenants {
        let mut config: toml::Value = toml::from_str(&config_toml)
            .with_context(|| format!("tenant {tenant_id} has invalid stored TOML"))?;
        let bucket_name = take_r2_bucket(&mut config)
            .with_context(|| format!("tenant {tenant_id} is missing an R2 bucket"))?;
        let normalized_config = toml::to_string(&config)?;
        let mut tx = db.begin().await?;
        sqlx::query("INSERT INTO r2_buckets(tenant_id,bucket_name) VALUES($1,$2)")
            .bind(tenant_id)
            .bind(bucket_name)
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE tenants SET config_toml=$1,config_revision=config_revision+1,updated_at=now() WHERE id=$2")
            .bind(normalized_config)
            .bind(tenant_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
    }
    Ok(())
}

async fn normalize_typed_configuration(db: &PgPool) -> Result<()> {
    let rows = sqlx::query_as::<_, (Uuid, String)>("SELECT t.id,t.config_toml FROM tenants t LEFT JOIN tenant_configuration c ON c.tenant_id=t.id WHERE c.tenant_id IS NULL").fetch_all(db).await?;
    for (id, raw) in rows {
        let config: TenantToml = toml::from_str(&raw).with_context(|| format!("tenant {id} has invalid configuration"))?;
        let mut tx = db.begin().await?;
        sqlx::query("INSERT INTO tenant_configuration(tenant_id,database_url,redis_url,resend_api_key,resend_from,media_storage_region,media_storage_endpoint,media_storage_access_key_id,media_storage_secret_access_key,media_storage_base_url,redis_coordination_url,redis_key_prefix,redis_process_metrics,software_update_url,allowed_private_networks,sign_integrity_proofs,database_pool_max_connections,database_pool_min_connections,database_pool_acquire_timeout_seconds,database_pool_idle_timeout_seconds,delivery_workers,delivery_batch,delivery_concurrency,inbox_workers,inbox_batch,inbox_concurrency,queue_idle_poll_seconds,process_delivery_concurrency,max_concurrent_requests,process_max_tenants,process_database_connections,active_record_encryption_primary_key,active_record_encryption_key_derivation_salt,smtp_host,smtp_port,smtp_username,smtp_password,smtp_from) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,$21,$22,$23,$24,$25,$26,$27,$28,$29,$30,$31,$32,$33,$34,$35,$36,$37,$38)")
            .bind(id).bind(&config.database_url).bind(&config.redis_url).bind(&config.resend.api_key).bind(&config.resend.from).bind(&config.media_storage.region).bind(&config.media_storage.endpoint).bind(&config.media_storage.access_key_id).bind(&config.media_storage.secret_access_key).bind(&config.media_storage.base_url)
            .bind(&config.redis_coordination_url).bind(&config.redis_key_prefix).bind(config.redis_process_metrics).bind(&config.software_update_url).bind(&config.allowed_private_networks).bind(config.sign_integrity_proofs)
            .bind(config.database_pool.max_connections).bind(config.database_pool.min_connections).bind(config.database_pool.acquire_timeout_seconds).bind(config.database_pool.idle_timeout_seconds)
            .bind(config.workers.delivery_workers).bind(config.workers.delivery_batch).bind(config.workers.delivery_concurrency).bind(config.workers.inbox_workers).bind(config.workers.inbox_batch).bind(config.workers.inbox_concurrency).bind(config.workers.queue_idle_poll_seconds).bind(config.workers.process_delivery_concurrency)
            .bind(config.limits.max_concurrent_requests).bind(config.limits.process_max_tenants).bind(config.limits.process_database_connections)
            .bind(config.active_record_encryption.as_ref().map(|x| &x.primary_key)).bind(config.active_record_encryption.as_ref().map(|x| &x.key_derivation_salt)).bind(config.smtp.as_ref().map(|x| &x.host)).bind(config.smtp.as_ref().map(|x| x.port)).bind(config.smtp.as_ref().map(|x| &x.username)).bind(config.smtp.as_ref().map(|x| &x.password)).bind(config.smtp.as_ref().map(|x| &x.from)).execute(&mut *tx).await?;
        sqlx::query("UPDATE tenants SET description=$1,short_description=$2,contact_email=$3,registrations_open=$4,approval_required=$5,vapid_private_key=$6,vapid_public_key=$7,icon_url=$8,privacy_policy=$9,terms_of_service=$10,config_revision=config_revision+1,updated_at=now() WHERE id=$11")
            .bind(&config.instance.description).bind(&config.instance.short_description).bind(&config.instance.contact_email).bind(config.instance.registrations_open).bind(config.instance.approval_required).bind(&config.instance.vapid_private_key).bind(&config.instance.vapid_public_key).bind(&config.instance.icon_url).bind(&config.instance.privacy_policy).bind(&config.instance.terms_of_service).bind(id).execute(&mut *tx).await?;
        tx.commit().await?;
    }
    Ok(())
} */

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
    Ok(Json(sqlx::query_as("SELECT t.id,t.domain,t.title,t.desired_state,CASE WHEN t.observed_at IS NULL OR t.observed_at < now()-interval '90 seconds' THEN 'unknown' ELSE t.observed_state END observed_state,t.config_revision,t.rendered_revision,t.tenant_file_name,r.name runtime_name,r.bind_address,t.updated_at FROM tenants t JOIN runtimes r ON r.id=t.runtime_id ORDER BY t.domain").fetch_all(&s.db).await?))
}
async fn operations(
    State(s): State<Shared>,
    headers: HeaderMap,
) -> Result<Json<Vec<OperationSummary>>, ApiError> {
    operator(&headers, &s.db).await?;
    Ok(Json(sqlx::query_as("SELECT o.id,t.domain,o.kind,o.status,o.error,o.created_at FROM operations o LEFT JOIN tenants t ON t.id=o.tenant_id ORDER BY o.created_at DESC LIMIT 50").fetch_all(&s.db).await?))
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
    let observed = if healthy { "healthy" } else { "unhealthy" };
    sqlx::query("UPDATE tenants SET observed_state=$1,observed_at=now(),observation_error=NULL,updated_at=now() WHERE id=$2")
        .bind(observed)
        .bind(id)
        .execute(&s.db)
        .await?;
    sqlx::query("UPDATE operations SET status='succeeded',detail=jsonb_build_object('healthy',$1),finished_at=now() WHERE id=$2").bind(healthy).bind(op).execute(&s.db).await?;
    Ok(Json(serde_json::json!({"healthy": healthy})))
}

async fn health_reconciler(state: Shared) {
    loop {
        let rows = sqlx::query_as::<_, (Uuid, String, String)>(
            "SELECT t.id,t.domain,r.bind_address FROM tenants t JOIN runtimes r ON r.id=t.runtime_id WHERE t.desired_state='running'",
        )
        .fetch_all(&state.db)
        .await;
        if let Ok(rows) = rows {
            for (id, domain, bind_address) in rows {
                let result = state.client.get(format!("http://{bind_address}/api/v2/instance")).header("Host", domain).send().await;
                let (observed, error) = match result {
                    Ok(response) if response.status().is_success() => ("healthy", None),
                    Ok(response) => ("unhealthy", Some(format!("Health endpoint returned {}", response.status()))),
                    Err(_) => ("unhealthy", Some("Health endpoint was unreachable.".to_string())),
                };
                let _ = sqlx::query("UPDATE tenants SET observed_state=$1,observed_at=now(),observation_error=$2,updated_at=now() WHERE id=$3")
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

fn is_safe_tenant_file_name(file_name: &str) -> bool {
    file_name.ends_with(".toml")
        && file_name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-'))
}

async fn mark_operation_failed(db: &PgPool, id: Uuid, message: &str) {
    let _ = sqlx::query("UPDATE operations SET status='failed',error=$1,finished_at=now() WHERE id=$2")
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
    let row = sqlx::query_as::<_, (SqlJson<StoredTenant>, SqlJson<StoredConfig>, String, String, String, String, String, String, i64)>(
        "SELECT to_jsonb(t),to_jsonb(c),b.bucket_name,r.host,r.tenant_directory,r.service_label,r.bind_address,t.tenant_file_name,t.config_revision FROM tenants t JOIN tenant_configuration c ON c.tenant_id=t.id JOIN runtimes r ON r.id=t.runtime_id JOIN r2_buckets b ON b.tenant_id=t.id WHERE t.id=$1",
    )
    .bind(id)
    .fetch_optional(&s.db)
    .await?
    .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "Tenant not found".into()))?;
    let (SqlJson(tenant), SqlJson(config), bucket_name, runtime_host, tenant_directory, service_label, bind_address, tenant_file_name, revision) = row;
    let domain = tenant.domain.clone();
    let op = Uuid::new_v4();
    sqlx::query("INSERT INTO operations(id,tenant_id,kind,status,requested_by,started_at) VALUES($1,$2,'render','running',$3,now())")
        .bind(op).bind(id).bind(operator_id).execute(&s.db).await?;

    let configured_host = std::env::var("CONTROL_RUNTIME_HOST").unwrap_or_default();
    if configured_host.is_empty() || configured_host != runtime_host {
        mark_operation_failed(&s.db, op, "This control plane is not the configured runtime host.").await;
        return Err(ApiError(StatusCode::CONFLICT, "This runtime is managed by a different control-plane host.".into()));
    }
    if !is_safe_domain(&domain) || !is_safe_launchd_label(&service_label) || !is_safe_tenant_file_name(&tenant_file_name) {
        mark_operation_failed(&s.db, op, "The stored runtime target is invalid.").await;
        return Err(ApiError(StatusCode::CONFLICT, "The stored runtime target is invalid.".into()));
    }
    let typed = typed_toml(tenant, config, bucket_name, bind_address.clone()).map_err(ApiError::from)?;
    let rendered_config = toml::to_string_pretty(&typed).map_err(ApiError::from)?;
    toml::from_str::<TenantToml>(&rendered_config).map_err(ApiError::from)?;

    let directory = PathBuf::from(tenant_directory);
    if tokio::fs::metadata(&directory).await.map(|meta| !meta.is_dir()).unwrap_or(true) {
        mark_operation_failed(&s.db, op, "The tenant directory is unavailable.").await;
        return Err(ApiError(StatusCode::CONFLICT, "The tenant directory is unavailable.".into()));
    }
    let target = directory.join(&tenant_file_name);
    let temporary = directory.join(format!(".{tenant_file_name}.{}", Uuid::new_v4()));
    if tokio::fs::write(&temporary, rendered_config.as_bytes()).await.is_err() {
        mark_operation_failed(&s.db, op, "The tenant configuration could not be written.").await;
        return Err(ApiError(StatusCode::INTERNAL_SERVER_ERROR, "Could not render the tenant configuration.".into()));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if tokio::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o600)).await.is_err() {
            let _ = tokio::fs::remove_file(&temporary).await;
            mark_operation_failed(&s.db, op, "The tenant configuration permissions could not be set.").await;
            return Err(ApiError(StatusCode::INTERNAL_SERVER_ERROR, "Could not secure the rendered configuration.".into()));
        }
    }
    if tokio::fs::rename(&temporary, &target).await.is_err() {
        let _ = tokio::fs::remove_file(&temporary).await;
        mark_operation_failed(&s.db, op, "The tenant configuration could not be activated.").await;
        return Err(ApiError(StatusCode::INTERNAL_SERVER_ERROR, "Could not activate the rendered configuration.".into()));
    }
    let uid = match tokio::process::Command::new("id").arg("-u").output().await {
        Ok(output) if output.status.success() => String::from_utf8_lossy(&output.stdout).trim().to_owned(),
        _ => {
            mark_operation_failed(&s.db, op, "The service account could not be identified after rendering.").await;
            return Err(ApiError(StatusCode::INTERNAL_SERVER_ERROR, "Configuration rendered, but the service could not be restarted.".into()));
        }
    };
    let launch_target = format!("gui/{uid}/{service_label}");
    if !matches!(tokio::process::Command::new("launchctl").args(["kickstart", "-k", &launch_target]).output().await, Ok(output) if output.status.success()) {
        mark_operation_failed(&s.db, op, "Configuration rendered, but launchd could not restart the instance.").await;
        return Err(ApiError(StatusCode::INTERNAL_SERVER_ERROR, "Configuration rendered, but the instance could not be restarted.".into()));
    }
    let health_url = format!("http://{bind_address}/api/v2/instance");
    let mut healthy = false;
    for _ in 0..20 {
        if s.client.get(&health_url).header("Host", &domain).send().await.map(|response| response.status().is_success()).unwrap_or(false) {
            healthy = true;
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    if !healthy {
        sqlx::query("UPDATE tenants SET observed_state='unhealthy',observed_at=now(),observation_error='Tenant did not become healthy after restart.',updated_at=now() WHERE id=$1")
            .bind(id).execute(&s.db).await?;
        mark_operation_failed(&s.db, op, "Configuration rendered and restarted, but the tenant did not become healthy.").await;
        return Err(ApiError(StatusCode::INTERNAL_SERVER_ERROR, "Configuration rendered, but the tenant did not become healthy after restart.".into()));
    }
    let checksum = format!("{:x}", Sha256::digest(rendered_config.as_bytes()));
    sqlx::query("UPDATE tenants SET rendered_revision=$1,rendered_checksum=$2,observed_state='healthy',observed_at=now(),observation_error=NULL,updated_at=now() WHERE id=$3")
        .bind(revision).bind(checksum).bind(id).execute(&s.db).await?;
    sqlx::query("UPDATE operations SET status='succeeded',detail=jsonb_build_object('revision',$1),finished_at=now() WHERE id=$2")
        .bind(revision).bind(op).execute(&s.db).await?;
    Ok(Json(serde_json::json!({"rendered": true, "revision": revision})))
}
