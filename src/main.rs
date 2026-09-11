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
use sqlx::{PgPool, postgres::PgPoolOptions};
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
    updated_at: OffsetDateTime,
}

#[derive(Serialize, sqlx::FromRow)]
struct OperationSummary {
    id: Uuid,
    domain: Option<String>,
    kind: String,
    status: String,
    error: Option<String>,
    created_at: OffsetDateTime,
}

#[derive(Deserialize)]
struct Login {
    username: String,
    password: String,
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
    let parsed: toml::Value = toml::from_str(&raw).context("tenant config is not valid TOML")?;
    let instance = parsed
        .get("instance")
        .and_then(toml::Value::as_table)
        .context("missing [instance]")?;
    let domain = instance
        .get("domain")
        .and_then(toml::Value::as_str)
        .context("missing instance.domain")?;
    let title = instance
        .get("title")
        .and_then(toml::Value::as_str)
        .unwrap_or(domain);
    let bind = parsed
        .get("bind_address")
        .and_then(toml::Value::as_str)
        .context("missing bind_address")?;
    let runtime_id = Uuid::new_v4();
    let mut tx = db.begin().await?;
    let runtime_id: Uuid = sqlx::query_scalar("INSERT INTO runtimes(id,name,host,tenant_directory,service_label,binary_path,bind_address) VALUES($1,$2,$3,$4,$5,$6,$7) ON CONFLICT(name) DO UPDATE SET host=excluded.host,tenant_directory=excluded.tenant_directory,service_label=excluded.service_label,binary_path=excluded.binary_path,bind_address=excluded.bind_address RETURNING id")
        .bind(runtime_id).bind(&runtime_name).bind(&host).bind(tenant_directory.to_string_lossy().as_ref()).bind(&service_label).bind(binary_path.to_string_lossy().as_ref()).bind(bind).fetch_one(&mut *tx).await?;
    let tenant_id = Uuid::new_v4();
    let tenant_file_name = format!("{}.toml", domain.replace('.', "-"));
    let imported_from = config_path.to_string_lossy().to_string();
    let id: Uuid = sqlx::query_scalar("INSERT INTO tenants(id,domain,title,runtime_id,desired_state,observed_state,config_toml,tenant_file_name,imported_from) VALUES($1,$2,$3,$4,'running','unknown',$5,$6,$7) ON CONFLICT(domain) DO UPDATE SET title=excluded.title,runtime_id=excluded.runtime_id,config_toml=excluded.config_toml,tenant_file_name=excluded.tenant_file_name,config_revision=tenants.config_revision+1,updated_at=now() RETURNING id")
        .bind(tenant_id).bind(domain).bind(title).bind(runtime_id).bind(raw).bind(tenant_file_name).bind(imported_from).fetch_one(&mut *tx).await?;
    sqlx::query("INSERT INTO operations(id,tenant_id,kind,status,detail,finished_at) VALUES($1,$2,'import','succeeded',jsonb_build_object('source',$3),now())")
        .bind(Uuid::new_v4()).bind(id).bind(config_path.to_string_lossy().as_ref()).execute(&mut *tx).await?;
    tx.commit().await?;
    println!("Imported {domain} as {id}; no runtime files were changed.");
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
    Ok(Json(sqlx::query_as("SELECT t.id,t.domain,t.title,t.desired_state,t.observed_state,t.config_revision,t.rendered_revision,t.tenant_file_name,r.name runtime_name,r.bind_address,t.updated_at FROM tenants t JOIN runtimes r ON r.id=t.runtime_id ORDER BY t.domain").fetch_all(&s.db).await?))
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
    sqlx::query("UPDATE tenants SET observed_state=$1,updated_at=now() WHERE id=$2")
        .bind(observed)
        .bind(id)
        .execute(&s.db)
        .await?;
    sqlx::query("UPDATE operations SET status='succeeded',detail=jsonb_build_object('healthy',$1),finished_at=now() WHERE id=$2").bind(healthy).bind(op).execute(&s.db).await?;
    Ok(Json(serde_json::json!({"healthy": healthy})))
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
    let row = sqlx::query_as::<_, (String, String, String, String, String, String, i64)>(
        "SELECT t.domain,t.config_toml,r.host,r.tenant_directory,r.service_label,t.tenant_file_name,t.config_revision FROM tenants t JOIN runtimes r ON r.id=t.runtime_id WHERE t.id=$1",
    )
    .bind(id)
    .fetch_optional(&s.db)
    .await?
    .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "Tenant not found".into()))?;
    let (domain, config_toml, runtime_host, tenant_directory, service_label, tenant_file_name, revision) = row;
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
    let parsed: toml::Value = match toml::from_str(&config_toml) {
        Ok(value) => value,
        Err(_) => {
            mark_operation_failed(&s.db, op, "The stored tenant configuration is invalid TOML.").await;
            return Err(ApiError(StatusCode::CONFLICT, "The stored tenant configuration is invalid.".into()));
        }
    };
    let config_domain = parsed.get("instance").and_then(toml::Value::as_table).and_then(|instance| instance.get("domain")).and_then(toml::Value::as_str);
    if config_domain != Some(&domain) {
        mark_operation_failed(&s.db, op, "The stored configuration domain does not match the tenant.").await;
        return Err(ApiError(StatusCode::CONFLICT, "The stored configuration does not match this tenant.".into()));
    }

    let directory = PathBuf::from(tenant_directory);
    if tokio::fs::metadata(&directory).await.map(|meta| !meta.is_dir()).unwrap_or(true) {
        mark_operation_failed(&s.db, op, "The tenant directory is unavailable.").await;
        return Err(ApiError(StatusCode::CONFLICT, "The tenant directory is unavailable.".into()));
    }
    let target = directory.join(&tenant_file_name);
    let temporary = directory.join(format!(".{tenant_file_name}.{}", Uuid::new_v4()));
    if tokio::fs::write(&temporary, config_toml.as_bytes()).await.is_err() {
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
    let checksum = format!("{:x}", Sha256::digest(config_toml.as_bytes()));
    sqlx::query("UPDATE tenants SET rendered_revision=$1,rendered_checksum=$2,observed_state='unknown',updated_at=now() WHERE id=$3")
        .bind(revision).bind(checksum).bind(id).execute(&s.db).await?;
    sqlx::query("UPDATE operations SET status='succeeded',detail=jsonb_build_object('revision',$1),finished_at=now() WHERE id=$2")
        .bind(revision).bind(op).execute(&s.db).await?;
    Ok(Json(serde_json::json!({"rendered": true, "revision": revision})))
}
