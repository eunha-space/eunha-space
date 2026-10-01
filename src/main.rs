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
    collections::HashMap,
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

mod auth;
mod domain_change;
mod health;
#[cfg(test)]
mod integration_tests;
mod temporal;
mod usage;

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
    health: Arc<health::Health>,
    usage: Arc<tokio::sync::Mutex<usage::Cache>>,
    db: PgPool,
    client: reqwest::Client,
    provision_database_url: String,
    /// Where tenants reach PostgreSQL, when that is not where the control plane
    /// provisions it. Roles and databases are created directly; tenant traffic
    /// goes through the pooler. Unset means both go to the same place.
    tenant_pooler_address: Option<String>,
    /// Client connections the pooler accepts in total, rendered so a runtime can
    /// size its tenants' pools against the limit that actually applies.
    tenant_pooler_client_slots: Option<u64>,
    cloudflare: Option<CloudflareConfig>,
    provided_domain_suffixes: Vec<String>,
    customer_email: Option<CustomerEmailConfig>,
    temporal_ui: Option<TemporalUiConfig>,
    /// One reload at a time per runtime: a second launchd signal that arrives
    /// while the first is still settling fails, and both would be re-reading
    /// the same directory anyway.
    runtime_reloads: Arc<tokio::sync::Mutex<HashMap<Uuid, Arc<tokio::sync::Mutex<()>>>>>,
}
#[derive(Clone)]
struct CustomerEmailConfig {
    api_key: String,
    from: String,
    public_url: String,
}
/// The operator-reachable Temporal Web UI, from `TEMPORAL_UI_URL`. The UI is
/// never published to the Internet, so an unset variable just leaves the admin
/// console without lifecycle links rather than guessing an address.
#[derive(Clone)]
struct TemporalUiConfig {
    base: url::Url,
    namespace: String,
}

impl TemporalUiConfig {
    /// A workflow ID alone is enough: the UI redirects to that execution's
    /// latest run, so the link survives the provision workflow's
    /// continue-as-new. It carries no workflow input, result, or history.
    fn workflow_url(&self, workflow_id: &str) -> Option<String> {
        let mut url = self.base.clone();
        url.path_segments_mut().ok()?.pop_if_empty().extend([
            "namespaces",
            &self.namespace,
            "workflows",
            workflow_id,
        ]);
        Some(url.into())
    }
}

#[derive(Clone)]
struct CloudflareConfig {
    zone_id: String,
    api_token: String,
    cname_target: String,
    /// `https://api.cloudflare.com/client/v4` unless `CLOUDFLARE_API_BASE`
    /// points at a stand-in, which is how the hostname lifecycle is tested.
    api_base: String,
}

impl CloudflareConfig {
    fn custom_hostnames_url(&self) -> String {
        format!(
            "{}/zones/{}/custom_hostnames",
            self.api_base.trim_end_matches('/'),
            self.zone_id
        )
    }
}
type Shared = Arc<AppState>;

#[derive(Debug)]
struct ApiError(StatusCode, String);
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut response = (self.0, Json(serde_json::json!({ "error": self.1 }))).into_response();
        response.headers_mut().insert(
            header::CACHE_CONTROL,
            header::HeaderValue::from_static("no-store"),
        );
        if self.0 == StatusCode::TOO_MANY_REQUESTS {
            response
                .headers_mut()
                .insert(header::RETRY_AFTER, header::HeaderValue::from_static("900"));
        }
        response
    }
}
impl<E: std::fmt::Display> From<E> for ApiError {
    fn from(value: E) -> Self {
        // Arbitrary driver/provider errors can contain credentials or query details.
        // Log a type and correlation ID only; never format the source error.
        let id = Uuid::new_v4();
        tracing::error!(error_id = %id, error_type = std::any::type_name::<E>(), "internal request failure");
        drop(value);
        Self(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Request could not be completed. Reference: {id}"),
        )
    }
}

/// A tenant as the console lists it.
///
/// Two endpoints build this: the operator's inventory, which selects all of it,
/// and a customer's instance list, which selects only what is theirs to see —
/// no durable-workflow metadata, no step the fleet is on. The fields the
/// narrower one leaves out are marked `default`, because sqlx reads a column
/// that is not in the query as an error rather than as `None`, and an `Option`
/// field does not change that. Leaving one unmarked breaks customer sign-in.
#[cfg(test)]
mod error_tests {
    use super::*;
    #[tokio::test]
    async fn arbitrary_errors_never_disclose_their_contents() {
        let error = ApiError::from(anyhow::anyhow!(
            "postgresql://tenant:secret@example/db token=private"
        ));
        let response = error.into_response();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let body = String::from_utf8(body.to_vec()).unwrap();
        assert!(!body.contains("secret"));
        assert!(!body.contains("private"));
        assert!(!body.contains("postgresql"));
        assert!(body.contains("Reference:"));
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
    purge_scheduled_for: Option<OffsetDateTime>,
    /// A custom domain released because it never validated; the tenant's own
    /// `domain` is then a placeholder.
    abandoned_custom_domain: Option<String>,
    /// Whether this tenant is an existing instance that was taken over. Its
    /// database, media and accounts predate the platform, which is what makes
    /// purge refuse to destroy parts of it.
    #[sqlx(default)]
    imported: bool,
    #[sqlx(default)]
    temporal_workflow_id: Option<String>,
    /// Where an operator can watch that execution, when a Temporal UI is
    /// configured. Derived, never selected from the database.
    #[sqlx(default)]
    temporal_workflow_url: Option<String>,
    #[sqlx(default)]
    temporal_workflow_kind: Option<String>,
    #[sqlx(default)]
    temporal_dispatch_attempts: Option<i32>,
    #[sqlx(default)]
    lifecycle_step: Option<String>,
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
    instance_id: Uuid,
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
    /// Proof of the old password, required unless the session came from an
    /// email link or the account has no password yet.
    #[serde(default)]
    current_password: Option<String>,
}
#[derive(Deserialize)]
struct VerificationQuery {
    token: String,
}
#[derive(Deserialize)]
struct CustomerTenantRequest {
    /// The `*.eunha.space` label, when the customer is not bringing a domain.
    #[serde(default)]
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

/// An existing Mastodon instance taken over rather than created.
///
/// Everything creation generates, an import has to be given instead, because
/// the instance's data already depends on it: the ActiveRecord secrets its
/// accounts' signing keys are encrypted with, the VAPID keypair its subscribers
/// already hold, and the storage its attachments are already in. Generating
/// fresh ones would produce an instance that cannot sign, cannot push, and
/// cannot show a picture.
#[derive(Deserialize)]
struct TenantImport {
    /// The instance's own domain, which it keeps. Its accounts' ActivityPub
    /// identity is on it and remote servers know them there, so an import is
    /// never also a move.
    domain: String,
    title: String,
    runtime_id: Uuid,
    /// A custom-format `pg_dump` already staged on the runtime host.
    dump_path: String,
    /// The instance's `public/system` tree, staged on the runtime host beside
    /// the dump, to be uploaded into the tenant's storage. Either this or
    /// `media_storage`: the instance's media either moves here or stays where
    /// it is, and it cannot do both.
    media_path: Option<String>,
    contact_email: Option<String>,
    /// The account in the dump that speaks for the instance. Recorded, never
    /// created: an imported instance arrives with its owner.
    owner_username: Option<String>,
    /// Mastodon's `ACTIVE_RECORD_ENCRYPTION_PRIMARY_KEY` and
    /// `..._KEY_DERIVATION_SALT`, as the source instance had them.
    active_record_encryption_primary_key: String,
    active_record_encryption_key_derivation_salt: String,
    /// The source's VAPID keypair. Both or neither: without it every Web Push
    /// subscription the instance has handed out stops working, so a generated
    /// one is a deliberate choice rather than a default.
    vapid_private_key: Option<String>,
    vapid_public_key: Option<String>,
    /// Where the instance's media already is, for an instance whose storage
    /// goes on serving it rather than being copied here. Nothing provisions it
    /// and the purge refuses to delete from it.
    media_storage: Option<ImportedMediaStorage>,
    #[serde(default)]
    registrations_open: bool,
    #[serde(default)]
    approval_required: bool,
}

/// The object storage an imported instance already keeps its media in. It is
/// the customer's, not ours: nothing provisions it and the purge refuses to
/// delete from it.
#[derive(Deserialize)]
struct ImportedMediaStorage {
    bucket: String,
    region: String,
    endpoint: Option<String>,
    access_key_id: String,
    secret_access_key: String,
    base_url: String,
    #[serde(default)]
    key_prefix: String,
}

#[derive(Deserialize)]
struct PurgeConfirmation {
    confirm_domain: String,
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
    /// Why the last check did not connect it: `dns` when its DNS does not point
    /// at the custom-hostname target yet, `retrying` for anything else.
    check_problem: Option<String>,
    #[serde(with = "time::serde::rfc3339::option")]
    checked_at: Option<OffsetDateTime>,
    /// When setup stops waiting for a pending custom domain.
    #[serde(with = "time::serde::rfc3339::option")]
    setup_deadline: Option<OffsetDateTime>,
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
    /// The owner signs in to Eunha with this email, which was the customer's
    /// when the account was created.
    owner_email: Option<String>,
    owner_ready: bool,
    #[serde(with = "time::serde::rfc3339::option")]
    owner_password_issued_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    purge_scheduled_for: Option<OffsetDateTime>,
    /// A custom domain chosen at creation that never connected.
    abandoned_custom_domain: Option<String>,
}

#[derive(Serialize)]
struct CustomerTenantDetail {
    tenant: CustomerTenantInfo,
    domains: Vec<CustomerDomainSetup>,
    provisioning_steps: Vec<CustomerProvisioningStep>,
    /// The instance's latest domain change, running or finished.
    domain_change: Option<domain_change::DomainChangeSummary>,
}

#[derive(Deserialize, Serialize)]
struct TenantToml {
    database_url: String,
    /// The same database through the pooler. Only the runtime's pool uses it;
    /// `database_url` stays direct so `eunha migrate` keeps its advisory lock.
    #[serde(default)]
    pooled_database_url: Option<String>,
    /// What the pooler accepts in total, so the runtime checks its tenants'
    /// pools against that rather than PostgreSQL's `max_connections`.
    #[serde(default)]
    pooled_client_slots: Option<u64>,
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
    /// Domains the instance had before, which its actors name in
    /// `alsoKnownAs` for a Move from them to be honoured. Served on nothing.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    previous_domains: Vec<String>,
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
    #[serde(default)]
    previous_domains: Vec<String>,
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
impl AppState {
    /// The pooler tenants should dial, if one is configured, with what it
    /// accepts. Absent means tenants talk to PostgreSQL directly.
    fn tenant_pooler(&self) -> Option<(&str, Option<u64>)> {
        self.tenant_pooler_address
            .as_deref()
            .map(|address| (address, self.tenant_pooler_client_slots))
    }
}

fn typed_toml(
    tenant: StoredTenant,
    config: StoredConfig,
    bucket: String,
    bind_address: String,
    pooler: Option<(&str, Option<u64>)>,
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
    let pooled_database_url = match pooler {
        Some((address, _)) => Some(pooled_database_url(&config.database_url, address)?),
        None => None,
    };
    Ok(TenantToml {
        database_url: config.database_url,
        pooled_database_url,
        pooled_client_slots: pooler.and_then(|(_, slots)| slots),
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
            previous_domains: tenant.previous_domains,
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

/// `TEMPORAL_UI_URL` is an operator convenience, not a dependency: anything
/// that is not an absolute `http`/`https` address is refused with a warning so
/// the console never renders a link that cannot be trusted or resolved.
fn temporal_ui_config() -> Option<TemporalUiConfig> {
    let configured = std::env::var("TEMPORAL_UI_URL").ok()?;
    let base = match url::Url::parse(configured.trim()) {
        Ok(base) if matches!(base.scheme(), "http" | "https") && base.has_host() => base,
        _ => {
            tracing::warn!(
                "TEMPORAL_UI_URL is not an absolute http(s) address; tenant lifecycle links are disabled"
            );
            return None;
        }
    };
    Some(TemporalUiConfig {
        base,
        namespace: std::env::var("TEMPORAL_NAMESPACE").unwrap_or_else(|_| "default".into()),
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
        .acquire_timeout(Duration::from_secs(5))
        .connect(&database_url)
        .await?;
    sqlx::migrate!().run(&db).await?;

    match args.command {
        Some(Command::SetOperator { username }) => return set_operator(&db, &username).await,
        None => {}
    }

    let state = Arc::new(AppState {
        health: Arc::new(health::Health::default()),
        usage: Arc::new(tokio::sync::Mutex::new(usage::Cache::default())),
        db,
        provision_database_url: std::env::var("CONTROL_PROVISION_DATABASE_URL")
            .unwrap_or(database_url),
        tenant_pooler_address: std::env::var("CONTROL_TENANT_POOLER_ADDRESS")
            .ok()
            .filter(|address| !address.trim().is_empty()),
        tenant_pooler_client_slots: std::env::var("CONTROL_TENANT_POOLER_CLIENT_SLOTS")
            .ok()
            .and_then(|slots| slots.trim().parse().ok()),
        cloudflare: match (
            std::env::var("CLOUDFLARE_ZONE_ID"),
            std::env::var("CLOUDFLARE_API_TOKEN"),
        ) {
            (Ok(zone_id), Ok(api_token)) => Some(CloudflareConfig {
                zone_id,
                api_token,
                cname_target: std::env::var("CLOUDFLARE_SAAS_CNAME_TARGET")
                    .unwrap_or_else(|_| "customers.eunha.space".into()),
                api_base: std::env::var("CLOUDFLARE_API_BASE")
                    .unwrap_or_else(|_| "https://api.cloudflare.com/client/v4".into()),
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
        temporal_ui: temporal_ui_config(),
        client: reqwest::Client::builder()
            .timeout(Duration::from_secs(4))
            .build()?,
        runtime_reloads: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
    });
    if rename_legacy_tenant_files(&state).await.is_err() {
        tracing::error!("could not rename tenant configuration files");
    }
    health::supervise(state.clone(), "usage::sampler", usage::sampler);
    health::supervise(state.clone(), "health_reconciler", health_reconciler);
    health::supervise(
        state.clone(),
        "release_recovery_reconciler",
        release_recovery_reconciler,
    );
    health::supervise(
        state.clone(),
        "automatic_release_reconciler",
        automatic_release_reconciler,
    );
    if std::env::var_os("TEMPORAL_ADDRESS").is_some() {
        let worker_state = state.clone();
        if let Err(error) = std::thread::Builder::new()
            .name("temporal-worker".into())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build();
                match runtime {
                    // Nothing runs a workflow while the worker is down, and it goes
                    // down whenever Temporal does; it is started again rather
                    // than left stopped until the control plane restarts.
                    Ok(runtime) => runtime.block_on(async move {
                        loop {
                            if !health::worker_attempt(
                                worker_state.health.clone(),
                                temporal::run_worker(worker_state.clone()),
                            )
                            .await
                            {
                                tracing::error!("Temporal worker stopped; restarting");
                            }
                            tokio::time::sleep(Duration::from_secs(10)).await;
                        }
                    }),
                    Err(error) => {
                        tracing::error!(error = %error, "could not start Temporal runtime")
                    }
                }
            })
        {
            tracing::error!(error = %error, "could not spawn Temporal worker thread");
        }
        health::supervise(
            state.clone(),
            "temporal::provisioning_dispatcher",
            temporal::provisioning_dispatcher,
        );
        health::supervise(
            state.clone(),
            "temporal::lifecycle_dispatcher",
            temporal::lifecycle_dispatcher,
        );
        health::supervise(
            state.clone(),
            "temporal::domain_change_dispatcher",
            temporal::domain_change_dispatcher,
        );
        health::supervise(
            state.clone(),
            "temporal::reconcile_starter",
            temporal::reconcile_starter,
        );
    } else {
        tracing::warn!("Temporal is not configured; tenant lifecycle work will remain queued");
    }
    health::supervise(state.clone(), "domain_reconciler", domain_reconciler);
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
            post(customer_delete_instance),
        )
        .route(
            "/customer/tenants/{id}/purge",
            post(customer_purge_instance),
        )
        .route("/customer/tenants/{id}/restore", post(customer_restore))
        .route(
            "/customer/tenants/{id}/domain-change",
            post(domain_change::customer_start),
        )
        .route(
            "/customer/tenants/{id}/domain-change/cancel",
            post(domain_change::customer_cancel),
        )
        .route("/customer/dns-check", get(customer_dns_check))
        .route("/tenants", get(tenants).post(create_tenant))
        .route("/usage", get(usage::usage))
        .route("/usage/history", get(usage::aggregate_history))
        .route("/tenants/{id}/usage/history", get(usage::history))
        .route("/tenants/import", post(import_tenant))
        .route("/tenants/{id}/check", post(check_tenant))
        .route("/tenants/{id}/retry", post(retry_provisioning))
        .route("/tenants/{id}/deprovision", post(deprovision_tenant))
        .route("/tenants/{id}/restore", post(restore_tenant))
        .route("/tenants/{id}/purge", post(purge_tenant))
        .route("/tenants/{id}/domains", get(tenant_domains))
        .route(
            "/tenants/{id}/domain-change",
            get(domain_change::operator_latest).post(domain_change::operator_start),
        )
        .route(
            "/tenants/{id}/domain-change/cancel",
            post(domain_change::operator_cancel),
        )
        .route(
            "/tenants/{id}/domain-change/retry",
            post(domain_change::operator_retry),
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
        .route("/operations", get(operations))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth::request_budget,
        ));
    // The console owns /admin/<section>; each of those paths has to serve the
    // shell with a 200 so a deep link or a refresh lands on the right section.
    // ServeDir::not_found_service would serve the shell but force a 404, and a
    // blanket fallback would answer for genuinely missing assets too, so the
    // console routes are explicit and everything else still 404s.
    let frontend_directory = PathBuf::from(
        std::env::var("CONTROL_FRONTEND_DIRECTORY").unwrap_or_else(|_| "frontend/dist".into()),
    );
    let index = ServeFile::new(frontend_directory.join("index.html"));
    let app = Router::new()
        .nest("/api", api)
        .route("/health/live", get(health::live))
        .route("/health/ready", get(health::ready))
        .route_service("/", index.clone())
        .route_service("/admin", index.clone())
        .route_service("/admin/{*section}", index.clone())
        .route_service("/signup", index.clone())
        .route_service("/verify", index.clone())
        .route_service("/account", index)
        .route_service(
            "/instances/{id}",
            ServeFile::new(frontend_directory.join("index.html")),
        )
        .fallback_service(ServeDir::new(frontend_directory))
        .layer(TraceLayer::new_for_http().make_span_with(|request: &axum::http::Request<_>| {
            tracing::info_span!("http_request", method = %request.method(), path = %request.uri().path())
        }))
        .with_state(state.clone());
    let bind = std::env::var("BIND_ADDRESS").unwrap_or_else(|_| "127.0.0.1:60001".into());
    let listener = tokio::net::TcpListener::bind(&bind).await?;
    tracing::info!(%bind, "eunha space control plane listening");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(health::shutdown(state))
    .await?;
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
    auth::limit(&s.db, "operator-login", &body.username, 10).await?;
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
    if !auth::verify(body.password.clone(), hash).await? {
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

/// Whether this session was established by following an email link, and so
/// already carries the proof a password reset needs.
async fn customer_email_link_session(headers: &HeaderMap, db: &PgPool) -> Result<bool, ApiError> {
    let Some(token) = customer_token(headers) else {
        return Ok(false);
    };
    Ok(sqlx::query_scalar::<_, bool>(
        "SELECT from_email_link FROM customer_sessions WHERE token_hash=$1 AND expires_at>now()",
    )
    .bind(token_hash(token))
    .fetch_optional(db)
    .await?
    .unwrap_or(false))
}

async fn customer_register(
    State(s): State<Shared>,
    Json(mut body): Json<CustomerCredentials>,
) -> Result<StatusCode, ApiError> {
    body.email = body.email.trim().to_ascii_lowercase();
    auth::limit(&s.db, "customer-auth", &body.email, 10).await?;
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
    sqlx::query("INSERT INTO customer_sessions(token_hash,customer_id,expires_at,from_email_link) VALUES($1,$2,now()+interval '30 days',true)").bind(token_hash(&token)).bind(customer_id).execute(&s.db).await?;
    Ok(([(header::SET_COOKIE,format!("eunha_space_customer_session={token}; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age=2592000"))],StatusCode::NO_CONTENT).into_response())
}

async fn customer_login(
    State(s): State<Shared>,
    Json(mut body): Json<CustomerCredentials>,
) -> Result<Response, ApiError> {
    body.email = body.email.trim().to_ascii_lowercase();
    auth::limit(&s.db, "customer-auth", &body.email, 10).await?;
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
    if !auth::verify(body.password.clone().unwrap_or_default(), hash).await? {
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
    auth::limit(&s.db, "password-change", &id.to_string(), 10).await?;
    if body.password.len() < 14 || body.password.len() > 1024 {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "Use a password with at least 14 characters.".into(),
        ));
    }
    let existing =
        sqlx::query_scalar::<_, Option<String>>("SELECT password_hash FROM customers WHERE id=$1")
            .bind(id)
            .fetch_one(&s.db)
            .await?;
    // A forgotten password is replaced from an email-link session, which is
    // itself the proof of ownership; anywhere else, prove the old one.
    let proved_by_email = customer_email_link_session(&headers, &s.db).await?;
    if let (Some(existing), false) = (existing, proved_by_email) {
        if !auth::verify(body.current_password.clone().unwrap_or_default(), existing).await? {
            return Err(ApiError(
                StatusCode::FORBIDDEN,
                "That current password is not right.".into(),
            ));
        }
    }
    let hash = auth::hash(body.password.clone()).await?;
    sqlx::query("UPDATE customers SET password_hash=$1 WHERE id=$2")
        .bind(hash)
        .bind(id)
        .execute(&s.db)
        .await?;
    // A new password ends every other session: whoever reset it keeps theirs,
    // and anyone signed in with the old one is turned out.
    sqlx::query("DELETE FROM customer_sessions WHERE customer_id=$1 AND token_hash<>$2")
        .bind(id)
        .bind(token_hash(customer_token(&headers).unwrap_or_default()))
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
    let (has_password, instance_limit) = sqlx::query_as::<_, (bool, Option<i64>)>(
        "SELECT password_hash IS NOT NULL,instance_limit FROM customers WHERE id=$1",
    )
    .bind(id)
    .fetch_one(&s.db)
    .await?;
    let requires_current_password =
        has_password && !customer_email_link_session(&headers, &s.db).await?;
    Ok(Json(
        serde_json::json!({"email":email,"instance_limit":instance_limit,"has_password":has_password,"requires_current_password":requires_current_password}),
    ))
}
async fn customer_tenants(
    State(s): State<Shared>,
    headers: HeaderMap,
) -> Result<Json<Vec<TenantSummary>>, ApiError> {
    let (id, _) = customer(&headers, &s.db).await?;
    Ok(Json(sqlx::query_as("SELECT t.id,t.domain,t.title,t.lifecycle_state,t.desired_state,CASE WHEN t.desired_state='running' AND (t.observed_at IS NULL OR t.observed_at < now()-interval '10 minutes') THEN 'unknown' ELSE t.observed_state END observed_state,t.config_revision,t.rendered_revision,r.name runtime_name,r.bind_address,t.updated_at,t.purge_scheduled_for,t.abandoned_custom_domain FROM tenants t JOIN runtimes r ON r.id=t.runtime_id WHERE t.customer_id=$1 AND t.lifecycle_state NOT IN ('purging','purged') ORDER BY t.created_at").bind(id).fetch_all(&s.db).await?))
}

async fn customer_tenant_detail(
    State(s): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<CustomerTenantDetail>, ApiError> {
    let (customer_id, _) = customer(&headers, &s.db).await?;
    let tenant = sqlx::query_as("SELECT t.id,t.domain,t.title,t.lifecycle_state,CASE WHEN t.desired_state='running' AND (t.observed_at IS NULL OR t.observed_at < now()-interval '10 minutes') THEN 'unknown' ELSE t.observed_state END observed_state,t.updated_at,t.owner_username,t.owner_email,EXISTS(SELECT 1 FROM provisioning_steps p WHERE p.tenant_id=t.id AND p.step='owner' AND p.status='succeeded') owner_ready,t.owner_password_issued_at,t.purge_scheduled_for,t.abandoned_custom_domain FROM tenants t JOIN customers c ON c.id=t.customer_id WHERE t.id=$1 AND t.customer_id=$2 AND t.lifecycle_state NOT IN ('purging','purged')")
        .bind(id)
        .bind(customer_id)
        .fetch_optional(&s.db)
        .await?
        .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "Instance not found".into()))?;
    let domains = sqlx::query_as(&format!("SELECT hostname,kind,status,is_primary,cname_target,ownership_name,ownership_value,ssl_status,CASE WHEN status<>'pending' OR error IS NULL THEN NULL WHEN error LIKE '%DNS does not point to%' THEN 'dns' ELSE 'retrying' END check_problem,CASE WHEN attempts>0 THEN updated_at END checked_at,CASE WHEN kind='custom' AND status='pending' AND NOT EXISTS(SELECT 1 FROM provisioning_steps p WHERE p.tenant_id=tenant_domains.tenant_id AND p.step='database' AND p.status='succeeded') THEN created_at+interval '{CUSTOM_DOMAIN_SETUP_WINDOW}' END setup_deadline FROM tenant_domains WHERE tenant_id=$1 AND kind IN ('provided','custom') ORDER BY is_primary DESC,created_at"))
        .bind(id)
        .fetch_all(&s.db)
        .await?;
    let provisioning_steps = sqlx::query_as("SELECT step,status,updated_at FROM provisioning_steps WHERE tenant_id=$1 ORDER BY array_position(ARRAY['database','migrate','import','render','media','reload','health','routing','owner'],step)")
        .bind(id)
        .fetch_all(&s.db)
        .await?;
    let domain_change = domain_change::latest(&s.db, id).await?;
    Ok(Json(CustomerTenantDetail {
        tenant,
        domains,
        provisioning_steps,
        domain_change,
    }))
}
async fn tenants(
    State(s): State<Shared>,
    headers: HeaderMap,
) -> Result<Json<Vec<TenantSummary>>, ApiError> {
    operator(&headers, &s.db).await?;
    let mut tenants: Vec<TenantSummary> = sqlx::query_as("SELECT t.id,t.domain,t.title,t.lifecycle_state,t.desired_state,CASE WHEN t.desired_state='running' AND (t.observed_at IS NULL OR t.observed_at < now()-interval '10 minutes') THEN 'unknown' ELSE t.observed_state END observed_state,t.config_revision,t.rendered_revision,r.name runtime_name,r.bind_address,t.updated_at,t.purge_scheduled_for,t.abandoned_custom_domain,EXISTS(SELECT 1 FROM tenant_imports m WHERE m.tenant_id=t.id) imported,d.workflow_id temporal_workflow_id,d.workflow_kind temporal_workflow_kind,d.attempts temporal_dispatch_attempts,CASE WHEN t.lifecycle_state='provisioning' THEN (SELECT step FROM provisioning_steps WHERE tenant_id=t.id AND status IN ('running','pending') ORDER BY CASE status WHEN 'running' THEN 0 ELSE 1 END,array_position(ARRAY['database','migrate','import','render','media','reload','health','routing','owner'],step) LIMIT 1) WHEN t.lifecycle_state IN ('deprovisioning','retained','purging') THEN (SELECT step FROM deprovisioning_steps WHERE tenant_id=t.id AND status IN ('running','pending') ORDER BY CASE status WHEN 'running' THEN 0 ELSE 1 END,step LIMIT 1) END lifecycle_step FROM tenants t JOIN runtimes r ON r.id=t.runtime_id LEFT JOIN LATERAL (SELECT workflow_id,workflow_kind,attempts FROM temporal_dispatches WHERE tenant_id=t.id ORDER BY created_at DESC LIMIT 1) d ON true ORDER BY t.domain")
        .fetch_all(&s.db)
        .await?;
    if let Some(ui) = &s.temporal_ui {
        for tenant in &mut tenants {
            tenant.temporal_workflow_url = tenant
                .temporal_workflow_id
                .as_deref()
                .and_then(|workflow_id| ui.workflow_url(workflow_id));
        }
    }
    Ok(Json(tenants))
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

/// The same database reached through the pooler, which is what a tenant's pool
/// uses. `eunha migrate` keeps the direct URL: sqlx holds a session-scoped
/// advisory lock across a migration run, and a transaction pooler would hand
/// the unlock to a different server connection than the lock.
fn pooled_database_url(direct_url: &str, pooler_address: &str) -> Result<String> {
    let mut url = url::Url::parse(direct_url).context("invalid tenant database URL")?;
    let (host, port) = pooler_address
        .rsplit_once(':')
        .context("tenant pooler address needs host:port")?;
    let port: u16 = port.parse().context("invalid tenant pooler port")?;
    url.set_host(Some(host))
        .map_err(|_| anyhow::anyhow!("invalid tenant pooler host"))?;
    url.set_port(Some(port))
        .map_err(|_| anyhow::anyhow!("invalid tenant pooler port"))?;
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
    body.title = body.title.trim().to_owned();
    body.contact_email = body
        .contact_email
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());
    let owner_email = owner_username
        .as_ref()
        .and_then(|_| body.contact_email.clone());
    if body.title.is_empty() || body.title.len() > 200 {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "Enter a valid domain and title.".into(),
        ));
    }
    let custom_domain = custom_domain
        .map(|value| value.trim().trim_end_matches('.').to_ascii_lowercase())
        .filter(|value| !value.is_empty());
    // An instance lives either at a customer's own domain or at a provided
    // `*.eunha.space` hostname, never both: its accounts are created under one
    // domain, and a second hostname would only ever be an alias to it.
    let (domain, provided_domains) = if let Some(custom_domain) = &custom_domain {
        if !is_valid_domain(custom_domain)
            || s.provided_domain_suffixes.iter().any(|suffix| {
                custom_domain == suffix || custom_domain.ends_with(&format!(".{suffix}"))
            })
        {
            return Err(ApiError(
                StatusCode::BAD_REQUEST,
                "Enter a valid externally owned custom domain.".into(),
            ));
        }
        (custom_domain.clone(), Vec::new())
    } else {
        let suffix = s.provided_domain_suffixes.first().ok_or_else(|| {
            ApiError(
                StatusCode::SERVICE_UNAVAILABLE,
                "No provided domain suffix is configured.".into(),
            )
        })?;
        if body.subdomain.is_empty()
            || body.subdomain.len() > 63
            || body.subdomain.starts_with('-')
            || body.subdomain.ends_with('-')
            || !body
                .subdomain
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
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
        let provided: Vec<String> = s
            .provided_domain_suffixes
            .iter()
            .map(|suffix| format!("{}.{}", body.subdomain, suffix))
            .collect();
        (format!("{}.{}", body.subdomain, suffix), provided)
    };
    let aliases: Vec<String> = provided_domains.iter().skip(1).cloned().collect();

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

    let mut tx = s.db.begin().await?;
    if let Some(customer_id) = customer_id {
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1::text,0))")
            .bind(customer_id)
            .execute(&mut *tx)
            .await?;
        let instance_limit = customer_instance_limit(&mut tx, customer_id).await?;
        let instance_count = customer_instance_count(&mut tx, customer_id).await?;
        if instance_limit.is_some_and(|limit| instance_count >= limit) {
            return Err(ApiError(
                StatusCode::CONFLICT,
                "You have reached your instance limit.".into(),
            ));
        }
    }
    sqlx::query("INSERT INTO tenants(id,domain,aliases,title,runtime_id,desired_state,observed_state,lifecycle_state,contact_email,registrations_open,approval_required,vapid_private_key,vapid_public_key,customer_id,owner_username,owner_email) VALUES($1,$2,$3,$4,$5,'running','unknown','provisioning',$6,$7,$8,$9,$10,$11,$12,$13)")
        .bind(tenant_id).bind(&domain).bind(&aliases).bind(&body.title).bind(body.runtime_id)
        .bind(&body.contact_email).bind(body.registrations_open).bind(body.approval_required)
        .bind(vapid_private).bind(vapid_public).bind(customer_id).bind(&owner_username).bind(owner_email).execute(&mut *tx).await
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
        sqlx::query("INSERT INTO tenant_domains(id,tenant_id,hostname,kind,status,is_primary,cname_target) VALUES($1,$2,$3,'custom','pending',true,$4)")
            .bind(Uuid::new_v4()).bind(tenant_id).bind(hostname).bind(&cname_target).execute(&mut *tx).await
            .map_err(|error| if error.as_database_error().and_then(|e| e.constraint()) == Some("tenant_domains_hostname_key") { ApiError(StatusCode::CONFLICT, "That domain is already managed.".into()) } else { ApiError::from(error) })?;
    }
    sqlx::query("INSERT INTO operations(id,tenant_id,runtime_id,kind,status,requested_by,requested_by_customer,detail,started_at) VALUES($1,$2,$3,'tenant_provision','running',$4,$5,jsonb_build_object('domain',$6),now())")
        .bind(operation_id).bind(tenant_id).bind(body.runtime_id).bind(operator_id).bind(customer_id).bind(&domain).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO provisioning_steps(tenant_id,step) SELECT $1,unnest(ARRAY['database','migrate','render','reload','health','routing'] || CASE WHEN $2 THEN ARRAY['owner'] ELSE ARRAY[]::text[] END)")
        .bind(tenant_id).bind(owner_username.is_some()).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO operation_events(operation_id,message) VALUES($1,'Tenant reserved; provisioning queued.')")
        .bind(operation_id).execute(&mut *tx).await?;
    enqueue_temporal_dispatch(&mut tx, tenant_id, "provision").await?;
    tx.commit().await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(serde_json::json!({"id": tenant_id, "operation_id": operation_id})),
    ))
}

/// Take over an existing Mastodon instance.
///
/// Creation mints a tenant: a domain nobody has used, an empty database, fresh
/// secrets, and an Owner account made at the end. An import inherits one, and
/// almost every difference follows from that. The domain is the customer's own
/// and already has accounts on it, so it is never a provided hostname and never
/// renamed. The secrets are the source instance's, because its data is
/// encrypted with them. There is no owner step, because the owner exists.
///
/// The one difference that is not about the data is when it happens. A created
/// instance provisions nothing until its domain validates, so an abandoned one
/// leaves nothing behind; an imported instance must be restored, served and
/// checked *before* its domain moves, because moving its domain is the cutover
/// and there is no going back from pointing live traffic at an empty server.
async fn import_tenant(
    State(s): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<TenantImport>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let (operator_id, _) = operator(&headers, &s.db).await?;
    let bad = |message: &str| ApiError(StatusCode::BAD_REQUEST, message.into());

    let domain = body
        .domain
        .trim()
        .trim_end_matches('.')
        .to_ascii_lowercase();
    if !is_valid_domain(&domain)
        || s.provided_domain_suffixes
            .iter()
            .any(|suffix| domain == *suffix || domain.ends_with(&format!(".{suffix}")))
    {
        return Err(bad(
            "An imported instance keeps its own domain, which cannot be a platform one.",
        ));
    }
    let title = body.title.trim().to_owned();
    if title.is_empty() || title.len() > 200 {
        return Err(bad("Enter a title."));
    }
    let dump_path = body.dump_path.trim().to_owned();
    if !is_safe_absolute_path(FsPath::new(&dump_path))
        || !dump_path
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'/'))
    {
        return Err(bad(
            "Give the dump as an absolute path on the runtime host.",
        ));
    }
    let owner_username = body
        .owner_username
        .as_deref()
        .map(str::trim)
        .filter(|username| !username.is_empty())
        .map(str::to_owned);
    if owner_username
        .as_deref()
        .is_some_and(|username| !is_valid_owner_username(username))
    {
        return Err(bad(
            "Use up to 30 letters, numbers, and underscores for the admin username.",
        ));
    }
    let encryption_primary = body.active_record_encryption_primary_key.trim().to_owned();
    let encryption_salt = body
        .active_record_encryption_key_derivation_salt
        .trim()
        .to_owned();
    if encryption_primary.is_empty() || encryption_salt.is_empty() {
        return Err(bad(
            "An imported instance needs the ActiveRecord encryption secrets its accounts' signing keys are encrypted with.",
        ));
    }
    // Generating a keypair here would be silent: the instance would serve, and
    // only its subscribers would find out, one missed notification at a time.
    let (vapid_private, vapid_public) = match (body.vapid_private_key, body.vapid_public_key) {
        (Some(private), Some(public))
            if !private.trim().is_empty() && !public.trim().is_empty() =>
        {
            (private.trim().to_owned(), public.trim().to_owned())
        }
        (None, None) => generate_vapid_keypair().map_err(|_| {
            ApiError(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Could not generate tenant keys.".into(),
            )
        })?,
        _ => {
            return Err(bad(
                "Give both halves of the instance's VAPID keypair, or neither.",
            ));
        }
    };
    let media_path = body
        .media_path
        .as_deref()
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(str::to_owned);
    if let Some(path) = &media_path
        && (!is_safe_absolute_path(FsPath::new(path))
            || !path.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'/')
            }))
    {
        return Err(bad(
            "Give the instance's media as an absolute path on the runtime host.",
        ));
    }
    // An instance's media either moves here or stays where it is. Asking for
    // both would leave two answers to where a file lives and one of them wrong.
    if media_path.is_some() && body.media_storage.is_some() {
        return Err(bad(
            "Either upload the instance's media or point at the storage it already uses, not both.",
        ));
    }
    if let Some(media) = &body.media_storage
        && (media.bucket.trim().is_empty()
            || media.region.trim().is_empty()
            || media.access_key_id.trim().is_empty()
            || media.secret_access_key.trim().is_empty()
            || media.base_url.trim().is_empty())
    {
        return Err(bad(
            "Give the bucket, region, credentials and base URL of the instance's existing media storage.",
        ));
    }

    let origin_hostname =
        sqlx::query_scalar::<_, String>("SELECT origin_hostname FROM runtimes WHERE id=$1")
            .bind(body.runtime_id)
            .fetch_optional(&s.db)
            .await?
            .ok_or_else(|| bad("Select a valid runtime."))?;
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
    // Redis, mail and worker settings are the runtime's, not the instance's:
    // they describe where it is hosted rather than who it is.
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
    let redis_prefix = tenant_redis_key_prefix(tenant_id);
    let brings_own_media = body.media_storage.is_some();
    let media = body.media_storage;
    let contact_email = body
        .contact_email
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());

    let mut tx = s.db.begin().await?;
    sqlx::query("INSERT INTO tenants(id,domain,title,runtime_id,desired_state,observed_state,lifecycle_state,contact_email,registrations_open,approval_required,vapid_private_key,vapid_public_key,owner_username) VALUES($1,$2,$3,$4,'running','unknown','provisioning',$5,$6,$7,$8,$9,$10)")
        .bind(tenant_id).bind(&domain).bind(&title).bind(body.runtime_id)
        .bind(&contact_email).bind(body.registrations_open).bind(body.approval_required)
        .bind(vapid_private).bind(vapid_public).bind(&owner_username).execute(&mut *tx).await
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
        CASE WHEN $6 THEN $7 ELSE media_storage_region END,
        CASE WHEN $6 THEN $8 ELSE media_storage_endpoint END,
        CASE WHEN $6 THEN $9 ELSE media_storage_access_key_id END,
        CASE WHEN $6 THEN $10 ELSE media_storage_secret_access_key END,
        CASE WHEN $6 THEN $11 ELSE media_storage_base_url END,
        CASE WHEN $6 THEN $12 ELSE $13 END,
        delivery_workers,delivery_batch,delivery_concurrency,inbox_workers,inbox_batch,inbox_concurrency,queue_idle_poll_seconds,process_delivery_concurrency,
        max_concurrent_requests,process_max_tenants,process_database_connections FROM tenant_configuration WHERE tenant_id=$14"#)
        .bind(tenant_id).bind(database_url).bind(redis_prefix).bind(&encryption_primary).bind(&encryption_salt)
        .bind(brings_own_media)
        .bind(media.as_ref().map(|m| m.region.trim()))
        .bind(media.as_ref().and_then(|m| m.endpoint.as_deref().map(str::trim)).filter(|value| !value.is_empty()))
        .bind(media.as_ref().map(|m| m.access_key_id.trim()))
        .bind(media.as_ref().map(|m| m.secret_access_key.trim()))
        .bind(media.as_ref().map(|m| m.base_url.trim().trim_end_matches('/')))
        .bind(media.as_ref().map(|m| m.key_prefix.trim().trim_matches('/')).unwrap_or_default())
        .bind(format!("t/{tenant_id}"))
        .bind(template)
        .execute(&mut *tx).await
        .map_err(|error| if error.as_database_error().and_then(|e| e.constraint()) == Some("tenant_configuration_media_storage_key_prefix_safe") { bad("That media key prefix is not a usable storage path.") } else { ApiError::from(error) })?;
    sqlx::query("INSERT INTO r2_buckets(tenant_id,bucket_name,lifecycle_state) SELECT $1,CASE WHEN $2 THEN $3 ELSE bucket_name END,'provisioning' FROM r2_buckets WHERE tenant_id=$4")
        .bind(tenant_id).bind(brings_own_media).bind(media.as_ref().map(|m| m.bucket.trim())).bind(template)
        .execute(&mut *tx).await
        .map_err(|error| if error.as_database_error().and_then(|e| e.constraint()) == Some("r2_buckets_bucket_name_safe") { bad("That is not a usable bucket name.") } else { ApiError::from(error) })?;
    sqlx::query("INSERT INTO tenant_domains(id,tenant_id,hostname,kind,status,is_primary,cname_target) VALUES($1,$2,$3,'custom','pending',true,$4)")
        .bind(Uuid::new_v4()).bind(tenant_id).bind(&domain).bind(&cname_target).execute(&mut *tx).await
        .map_err(|error| if error.as_database_error().and_then(|e| e.constraint()) == Some("tenant_domains_hostname_key") { ApiError(StatusCode::CONFLICT, "That domain is already managed.".into()) } else { ApiError::from(error) })?;
    sqlx::query("INSERT INTO tenant_imports(tenant_id,dump_path,media_path) VALUES($1,$2,$3)")
        .bind(tenant_id)
        .bind(&dump_path)
        .bind(&media_path)
        .execute(&mut *tx)
        .await
        .map_err(
            |error| match error.as_database_error().and_then(|e| e.constraint()) {
                Some("tenant_imports_dump_path_absolute") => {
                    bad("Give the dump as an absolute path on the runtime host.")
                }
                Some("tenant_imports_media_path_absolute") => {
                    bad("Give the instance's media as an absolute path on the runtime host.")
                }
                _ => ApiError::from(error),
            },
        )?;
    // The same kind as a created tenant's: these are provisioning steps, and
    // every failure, event and retry path finds the operation by that name. The
    // detail is what says the instance came from somewhere.
    sqlx::query("INSERT INTO operations(id,tenant_id,runtime_id,kind,status,requested_by,detail,started_at) VALUES($1,$2,$3,'tenant_provision','running',$4,jsonb_build_object('domain',$5,'imported',true),now())")
        .bind(operation_id).bind(tenant_id).bind(body.runtime_id).bind(operator_id).bind(&domain).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO provisioning_steps(tenant_id,step) SELECT $1,unnest(ARRAY['database','migrate','import','render'] || CASE WHEN $2 THEN ARRAY['media'] ELSE ARRAY[]::text[] END || ARRAY['reload','health','routing'])")
        .bind(tenant_id).bind(media_path.is_some()).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO operation_events(operation_id,message) VALUES($1,$2)")
        .bind(operation_id)
        .bind(format!(
            "Tenant reserved for {domain}; the instance is set up before its domain moves."
        ))
        .execute(&mut *tx)
        .await?;
    enqueue_temporal_dispatch(&mut tx, tenant_id, "provision").await?;
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
    // A provided hostname that failed would fail routing again; check it anew.
    sqlx::query("UPDATE tenant_domains SET status='pending',attempts=0,error=NULL,next_check_at=NULL,updated_at=now()-interval '1 minute' WHERE tenant_id=$1 AND kind='provided' AND status='failed'")
        .bind(id).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO operations(id,tenant_id,runtime_id,kind,status,requested_by,started_at) SELECT $1,t.id,t.runtime_id,'tenant_provision','running',$2,now() FROM tenants t WHERE t.id=$3")
        .bind(operation_id).bind(operator_id).bind(id).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO operation_events(operation_id,message) VALUES($1,'Provisioning retry queued.')")
        .bind(operation_id).execute(&mut *tx).await?;
    enqueue_temporal_dispatch(&mut tx, id, "provision").await?;
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

/// How long setup keeps trying to connect a custom domain. Custom domains can
/// only be chosen when an instance is created, and the domain is what the
/// instance was asked to be, so when the window closes the domain is released
/// and the unfinished instance is deleted rather than quietly finished elsewhere.
const CUSTOM_DOMAIN_SETUP_WINDOW: &str = "7 days";

async fn domain_reconciler(state: Shared) {
    loop {
        expire_custom_domains(&state).await;
        if let Ok(ids) = sqlx::query_scalar::<_, Uuid>("SELECT d.id FROM tenant_domains d JOIN tenants t ON t.id=d.tenant_id WHERE d.status='removing' AND d.kind='custom' AND t.lifecycle_state IN ('provisioning','ready','failed','deprovisioning','retained') ORDER BY d.updated_at LIMIT 10")
            .fetch_all(&state.db).await
        {
            for id in ids {
                let _ = drop_custom_domain(&state, id).await;
            }
        }
        // A provided hostname a cancelled domain change asked for: never
        // primary, never an identity, so it is simply taken off Cloudflare.
        if let Ok(ids) = sqlx::query_scalar::<_, Uuid>("SELECT id FROM tenant_domains WHERE status='removing' AND kind='provided' AND NOT is_primary ORDER BY updated_at LIMIT 10")
            .fetch_all(&state.db).await
        {
            for id in ids {
                let _ = delete_domain(&state, id).await;
            }
        }
        if let Ok(ids) = sqlx::query_scalar::<_, Uuid>("SELECT d.id FROM tenant_domains d JOIN tenants t ON t.id=d.tenant_id JOIN runtimes r ON r.id=t.runtime_id WHERE d.kind IN ('provided','custom') AND t.lifecycle_state IN ('provisioning','ready','failed') AND ((d.status='pending' AND d.updated_at < now()-interval '15 seconds' AND (d.next_check_at IS NULL OR d.next_check_at <= now())) OR (d.status<>'removing' AND d.routed_origin_hostname IS DISTINCT FROM r.origin_hostname)) ORDER BY d.created_at LIMIT 10")
            .fetch_all(&state.db).await {
            for id in ids {
                if let Err(error) = reconcile_domain(&state, id).await {
                    let message = domain_reconcile_error_message(&error);
                    if error.downcast_ref::<DomainTerminalError>().is_some() {
                        // Cloudflare has given up on a provided hostname, which
                        // provisioning must surface. A custom domain keeps being
                        // retried until its setup window closes.
                        let _ = sqlx::query("UPDATE tenant_domains SET status='failed',error=$1,updated_at=now() WHERE id=$2 AND kind<>'custom'")
                            .bind(&message).bind(id).execute(&state.db).await;
                        record_domain_error(&state.db, id, &message).await;
                    } else {
                        record_domain_error(&state.db, id, &message).await;
                    }
                }
            }
        }
        state.health.tick("domains");
        tokio::time::sleep(Duration::from_secs(15)).await;
    }
}

/// Release custom domains whose setup window closed without connecting, and
/// delete the instances still waiting for them. The deletion is the ordinary
/// one: the instance goes offline and is retained, freeing the customer's slot,
/// and only an operator's purge destroys its data.
///
/// The window exists so that an instance which was never set up leaves nothing
/// behind. An imported one was set up on its first day — its data is restored,
/// rendered and served — and is only waiting for an operator and a customer to
/// agree on a cutover, so the window does not apply to it.
async fn expire_custom_domains(state: &Shared) {
    let Ok(expired) = sqlx::query_as::<_, (Uuid, Uuid, String)>(&format!(
        "SELECT d.id,d.tenant_id,d.hostname FROM tenant_domains d JOIN tenants t ON t.id=d.tenant_id WHERE d.kind='custom' AND d.status='pending' AND t.lifecycle_state='provisioning' AND NOT EXISTS(SELECT 1 FROM tenant_imports m WHERE m.tenant_id=t.id) AND d.created_at < now()-interval '{CUSTOM_DOMAIN_SETUP_WINDOW}' ORDER BY d.created_at LIMIT 10"
    ))
    .fetch_all(&state.db)
    .await
    else {
        return;
    };
    for (domain_id, tenant_id, hostname) in expired {
        let Ok(mut tx) = state.db.begin().await else {
            continue;
        };
        let expired = async {
            let lifecycle = sqlx::query_scalar::<_, String>(
                "SELECT lifecycle_state FROM tenants WHERE id=$1 FOR UPDATE",
            )
            .bind(tenant_id)
            .fetch_one(&mut *tx)
            .await?;
            let released = sqlx::query(&format!("UPDATE tenant_domains SET status='removing',error='It was not connected within {CUSTOM_DOMAIN_SETUP_WINDOW}.',updated_at=now() WHERE id=$1 AND status='pending'"))
                .bind(domain_id)
                .execute(&mut *tx)
                .await?
                .rows_affected();
            if released == 1 && lifecycle == "provisioning" {
                let message = format!("Setup stopped: {hostname} was not connected within {CUSTOM_DOMAIN_SETUP_WINDOW}, so the instance is being deleted.");
                sqlx::query("INSERT INTO operation_events(operation_id,message) SELECT id,$2 FROM operations WHERE tenant_id=$1 AND kind='tenant_provision' AND status='running'")
                    .bind(tenant_id).bind(&message).execute(&mut *tx).await?;
                sqlx::query("UPDATE operations SET status='failed',error=$2,finished_at=now() WHERE tenant_id=$1 AND kind='tenant_provision' AND status='running'")
                    .bind(tenant_id).bind(&message).execute(&mut *tx).await?;
                begin_deprovisioning(&mut tx, tenant_id, None, None).await?;
            }
            Ok::<_, sqlx::Error>(())
        }
        .await;
        if expired.is_ok() {
            let _ = tx.commit().await;
        }
    }
}

/// Cloudflare has given up on a hostname or its certificate; checking again
/// will not change that on its own.
#[derive(Debug)]
struct DomainTerminalError(&'static str);

impl std::fmt::Display for DomainTerminalError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.0)
    }
}

impl std::error::Error for DomainTerminalError {}

/// Remove a custom domain that did not connect: its Cloudflare hostname, then
/// its record, which frees the hostname. A primary domain is only released from
/// an instance that was never set up, since no account ever lived under it.
async fn drop_custom_domain(state: &Shared, id: Uuid) -> Result<()> {
    let Some((tenant_id, hostname, cloudflare_id, reason)) =
        sqlx::query_as::<_, (Uuid, String, Option<String>, Option<String>)>(
            "SELECT d.tenant_id,d.hostname,d.cloudflare_id,d.error FROM tenant_domains d WHERE d.id=$1 AND d.kind='custom' AND d.status='removing' AND (NOT d.is_primary OR NOT EXISTS(SELECT 1 FROM provisioning_steps p WHERE p.tenant_id=d.tenant_id AND p.step='database' AND p.status='succeeded'))",
        )
        .bind(id)
        .fetch_optional(&state.db)
        .await?
    else {
        return Ok(());
    };
    if let Some(cloudflare_id) = cloudflare_id {
        let cloudflare = state
            .cloudflare
            .as_ref()
            .context("Cloudflare is not configured")?;
        anyhow::ensure!(
            cloudflare_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-'),
            "invalid Cloudflare hostname id"
        );
        let response = state
            .client
            .delete(format!(
                "{}/{cloudflare_id}",
                cloudflare.custom_hostnames_url()
            ))
            .bearer_auth(&cloudflare.api_token)
            .send()
            .await?;
        anyhow::ensure!(
            response.status().is_success() || response.status() == StatusCode::NOT_FOUND,
            "Cloudflare refused to delete a hostname"
        );
    }
    let reason = reason.unwrap_or_else(|| "The custom domain was removed.".into());
    let mut tx = state.db.begin().await?;
    let deleted = sqlx::query("DELETE FROM tenant_domains WHERE id=$1 AND status='removing'")
        .bind(id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    if deleted == 0 {
        return Ok(());
    }
    // A released primary domain must not stay reserved on the tenant row, which
    // otherwise holds its domain forever: the tenant takes a placeholder that
    // can never be a real hostname, and remembers the domain for restoring.
    // Only the domain an instance was created under is remembered as
    // abandoned; one a domain change asked for and did not get is just gone.
    sqlx::query("UPDATE tenants SET abandoned_custom_domain=CASE WHEN domain=$1 THEN $1 ELSE abandoned_custom_domain END,aliases=array_remove(aliases,$1),domain=CASE WHEN domain=$1 THEN id::text||'.invalid' ELSE domain END WHERE id=$2")
        .bind(&hostname).bind(tenant_id).execute(&mut *tx).await?;
    if let Some(operation_id) = sqlx::query_scalar::<_, Uuid>("SELECT id FROM operations WHERE tenant_id=$1 AND kind='tenant_provision' AND status='running' ORDER BY created_at DESC LIMIT 1")
        .bind(tenant_id).fetch_optional(&mut *tx).await?
    {
        sqlx::query("INSERT INTO operation_events(operation_id,message) VALUES($1,$2)")
            .bind(operation_id)
            .bind(format!("Custom domain {hostname} dropped: {reason} Setup continues on the provided hostname."))
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Take a hostname off Cloudflare, if it is there, and delete its record,
/// which frees it.
async fn delete_domain(state: &Shared, id: Uuid) -> Result<()> {
    let Some(cloudflare_id) = sqlx::query_scalar::<_, Option<String>>(
        "SELECT cloudflare_id FROM tenant_domains WHERE id=$1",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await?
    else {
        return Ok(());
    };
    if let Some(cloudflare_id) = cloudflare_id {
        let cloudflare = state
            .cloudflare
            .as_ref()
            .context("Cloudflare is not configured")?;
        anyhow::ensure!(
            cloudflare_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-'),
            "invalid Cloudflare hostname id"
        );
        let response = state
            .client
            .delete(format!(
                "{}/{cloudflare_id}",
                cloudflare.custom_hostnames_url()
            ))
            .bearer_auth(&cloudflare.api_token)
            .send()
            .await?;
        anyhow::ensure!(
            response.status().is_success() || response.status() == StatusCode::NOT_FOUND,
            "Cloudflare refused to delete a hostname"
        );
    }
    sqlx::query("DELETE FROM tenant_domains WHERE id=$1")
        .bind(id)
        .execute(&state.db)
        .await?;
    Ok(())
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
    if let Some(error) = error.downcast_ref::<DomainTerminalError>() {
        return error.0.to_string();
    }
    if let Some(error) = error.downcast_ref::<DomainDnsTargetError>() {
        return format!(
            "Cloudflare cannot validate this hostname because its DNS does not point to {}. Update the CNAME record; existing routing remains unchanged.",
            error.cname_target
        );
    }
    "Hostname routing could not be reconciled; existing routing remains unchanged.".into()
}

/// Record a failed check. A provided hostname of a served tenant fails after
/// five, so a broken provisioning surfaces quickly. A custom domain instead
/// waits for its owner's DNS, checking less often as time passes, until its
/// setup window closes; and nothing about a tenant that is still waiting for its
/// custom domain counts toward failure, since that wait can last days.
async fn record_domain_error(db: &PgPool, id: Uuid, message: &str) {
    let failed = sqlx::query_scalar::<_, bool>("UPDATE tenant_domains d SET attempts=CASE WHEN w.unserved AND d.kind<>'custom' THEN d.attempts ELSE d.attempts+1 END,status=CASE WHEN d.kind<>'custom' AND d.status='pending' AND NOT w.unserved AND d.attempts+1 >= 5 THEN 'failed' ELSE d.status END,next_check_at=CASE WHEN d.kind='custom' THEN now()+make_interval(secs => LEAST(900, 30*power(2, LEAST(d.attempts, 5)))) WHEN w.unserved THEN now()+CASE WHEN now()-d.created_at < interval '10 minutes' THEN interval '30 seconds' WHEN now()-d.created_at < interval '1 hour' THEN interval '2 minutes' ELSE interval '15 minutes' END ELSE NULL END,error=$1,updated_at=now() FROM (SELECT EXISTS(SELECT 1 FROM provisioning_steps p JOIN tenant_domains x ON x.tenant_id=p.tenant_id WHERE x.id=$2 AND p.step='health' AND p.status<>'succeeded') AS unserved) w WHERE d.id=$2 RETURNING d.status='failed'")
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
    let endpoint = cloudflare.custom_hostnames_url();
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
    let terminal = if matches!(
        cf_status,
        "blocked" | "deleted" | "pending_blocked" | "test_blocked" | "test_failed"
    ) {
        Some("Cloudflare will not serve this hostname.")
    } else if ssl_status.ends_with("_timed_out")
        || matches!(ssl_status, "expired" | "inactive" | "deleted")
    {
        Some("Cloudflare could not issue a certificate for this hostname.")
    } else {
        None
    };
    if let Some(terminal) = terminal {
        // A custom domain's owner may yet fix what stopped validation, such as a
        // CAA record, so ask Cloudflare to validate it again on each check.
        if kind == "custom" {
            let _ = state
                .client
                .patch(format!("{endpoint}/{cf_id}"))
                .bearer_auth(&cloudflare.api_token)
                .json(&serde_json::json!({
                    "custom_origin_server": origin_hostname,
                    "ssl": {"method": "http", "type": "dv", "settings": {"min_tls_version": "1.2"}}
                }))
                .send()
                .await;
        }
        return Err(DomainTerminalError(terminal).into());
    }
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
        schedule_next_domain_check(&state.db, id).await?;
        return Ok(());
    }
    // Provisioning has not served the tenant yet, which for an instance with a
    // custom domain means waiting for that domain. A hostname cannot be health
    // checked before then; check again later instead of counting a failure.
    let unserved = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM provisioning_steps WHERE tenant_id=$1 AND step='health' AND status<>'succeeded')",
    )
    .bind(tenant_id)
    .fetch_one(&state.db)
    .await?;

    let (primary_id, primary_kind) = sqlx::query_as::<_, (Uuid, String)>(
        "SELECT id,kind FROM tenant_domains WHERE tenant_id=$1 AND is_primary",
    )
    .bind(tenant_id)
    .fetch_one(&state.db)
    .await?;
    // An imported tenant can adopt its existing primary hostname into
    // Cloudflare for SaaS. Its ActivityPub identity and runtime configuration
    // are already correct, so activation only completes the durable operation.
    if unserved && primary_id == id && kind == "custom" {
        // The domain has validated; setup can now begin under it and checks the
        // tenant's health itself.
        finish_domain(state, id).await?;
        return Ok(());
    }
    if unserved && kind == "provided" {
        schedule_next_domain_check(&state.db, id).await?;
        return Ok(());
    }
    if primary_id == id {
        healthcheck_provisioned_tenant(state, tenant_id).await?;
        finish_domain(state, id).await?;
        return Ok(());
    }
    if kind == "provided" {
        healthcheck_host(state, tenant_id, &hostname).await?;
        finish_domain(state, id).await?;
        return Ok(());
    }
    let has_identity = if primary_kind == "custom" {
        true
    } else if unserved {
        // Not even its database has been used yet.
        false
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
        finish_domain(state, id).await?;
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
    if unserved {
        // Provisioning renders, starts and checks the tenant under this domain
        // from the beginning; there is nothing running to move over.
        finish_domain(state, id).await?;
        return Ok(());
    }
    render_provisioned_tenant(state, tenant_id).await?;
    reload_provisioned_runtime(state, tenant_id).await?;
    healthcheck_provisioned_tenant(state, tenant_id).await?;
    finish_domain(state, id).await?;
    Ok(())
}

/// Check a hostname that is still settling less often the longer it has been:
/// every 30 seconds for its first ten minutes, every two minutes for its first
/// hour, then every fifteen minutes.
async fn schedule_next_domain_check(db: &PgPool, id: Uuid) -> Result<()> {
    sqlx::query("UPDATE tenant_domains SET next_check_at=now()+CASE WHEN now()-created_at < interval '10 minutes' THEN interval '30 seconds' WHEN now()-created_at < interval '1 hour' THEN interval '2 minutes' ELSE interval '15 minutes' END WHERE id=$1")
        .bind(id)
        .execute(db)
        .await?;
    Ok(())
}

async fn finish_domain(state: &Shared, id: Uuid) -> Result<()> {
    let tenant_id = sqlx::query_scalar::<_, Uuid>(
        "UPDATE tenant_domains SET status='active',error=NULL,updated_at=now() WHERE id=$1 RETURNING tenant_id",
    )
    .bind(id)
    .fetch_one(&state.db)
    .await?;
    sqlx::query("UPDATE operations SET status='succeeded',finished_at=now() WHERE id=(SELECT o.id FROM operations o JOIN tenant_domains d ON d.tenant_id=o.tenant_id WHERE d.id=$1 AND o.kind='custom_domain_attach' AND o.status='running' ORDER BY o.created_at DESC LIMIT 1)")
        .bind(id).execute(&state.db).await?;
    temporal::wake_provisioning(state, tenant_id).await;
    Ok(())
}

/// How long a deleted instance's data is kept, so it can be restored, before it
/// is purged.
const RETENTION_BEFORE_PURGE: &str = "7 days";

#[derive(Deserialize)]
struct DnsCheckQuery {
    hostname: String,
}

/// Whether a hostname's CNAME already points at the custom-hostname target, so
/// the create form can warn before setup starts waiting. Advisory only: a
/// flattened apex record resolves to addresses rather than a CNAME, and
/// Cloudflare still validates it.
async fn customer_dns_check(
    State(s): State<Shared>,
    headers: HeaderMap,
    Query(query): Query<DnsCheckQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    customer(&headers, &s.db).await?;
    let hostname = query
        .hostname
        .trim()
        .trim_end_matches('.')
        .to_ascii_lowercase();
    if !is_valid_domain(&hostname) {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "Enter a valid domain.".into(),
        ));
    }
    let target = s
        .cloudflare
        .as_ref()
        .map(|cloudflare| cloudflare.cname_target.clone())
        .ok_or_else(|| {
            ApiError(
                StatusCode::SERVICE_UNAVAILABLE,
                "Cloudflare for SaaS is not configured.".into(),
            )
        })?;
    let answer = s
        .client
        .get("https://cloudflare-dns.com/dns-query")
        .query(&[("name", hostname.as_str()), ("type", "CNAME")])
        .header(header::ACCEPT, "application/dns-json")
        .send()
        .await
        .ok()
        .filter(|response| response.status().is_success());
    let points_to_target = match answer {
        Some(response) => response
            .json::<serde_json::Value>()
            .await
            .ok()
            .map(|body| cname_points_to(&body, &target)),
        None => None,
    };
    Ok(Json(
        serde_json::json!({ "points_to_target": points_to_target, "target": target }),
    ))
}

/// Whether a DNS-over-HTTPS JSON answer holds a record naming `target`.
fn cname_points_to(body: &serde_json::Value, target: &str) -> bool {
    body.get("Answer")
        .and_then(|answers| answers.as_array())
        .is_some_and(|answers| {
            answers.iter().any(|answer| {
                answer
                    .get("data")
                    .and_then(|data| data.as_str())
                    .is_some_and(|data| data.trim_end_matches('.').eq_ignore_ascii_case(target))
            })
        })
}

/// Instances counting against a customer's limit. A deprovisioned instance
/// frees its slot while its data is retained.
async fn customer_instance_count(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    customer_id: Uuid,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM tenants WHERE customer_id=$1 AND lifecycle_state NOT IN ('deprovisioning','retained','purging','purged')",
    )
    .bind(customer_id)
    .fetch_one(&mut **tx)
    .await
}

/// A null limit grants the customer unlimited instances.
async fn customer_instance_limit(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    customer_id: Uuid,
) -> Result<Option<i64>, sqlx::Error> {
    sqlx::query_scalar("SELECT instance_limit FROM customers WHERE id=$1")
        .bind(customer_id)
        .fetch_one(&mut **tx)
        .await
}

/// Delete an instance: take it offline at once, keeping its data until
/// `RETENTION_BEFORE_PURGE` has passed, when it is purged unless restored first.
async fn customer_delete_instance(
    State(s): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let (customer_id, _) = customer(&headers, &s.db).await?;
    let mut tx = s.db.begin().await?;
    let deletable = sqlx::query_scalar::<_, bool>("SELECT true FROM tenants WHERE id=$1 AND customer_id=$2 AND lifecycle_state IN ('provisioning','ready','failed') FOR UPDATE")
        .bind(id).bind(customer_id).fetch_optional(&mut *tx).await?.is_some();
    if !deletable {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "This instance is already deleted.".into(),
        ));
    }
    begin_deprovisioning(&mut tx, id, None, Some(customer_id)).await?;
    tx.commit().await?;
    temporal::wake_provisioning(&s, id).await;
    Ok(StatusCode::ACCEPTED)
}

/// Purge a deleted instance now instead of when its retention ends.
async fn customer_purge_instance(
    State(s): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(body): Json<PurgeConfirmation>,
) -> Result<StatusCode, ApiError> {
    let (customer_id, _) = customer(&headers, &s.db).await?;
    let mut tx = s.db.begin().await?;
    let address = sqlx::query_scalar::<_, String>("SELECT COALESCE(abandoned_custom_domain,domain) FROM tenants WHERE id=$1 AND customer_id=$2 AND lifecycle_state='retained' FOR UPDATE")
        .bind(id).bind(customer_id).fetch_optional(&mut *tx).await?
        .ok_or_else(|| ApiError(StatusCode::CONFLICT, "Only a deleted instance can be deleted permanently.".into()))?;
    if !body.confirm_domain.trim().eq_ignore_ascii_case(&address) {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "Type the instance's address to confirm.".into(),
        ));
    }
    start_purge(
        &mut tx,
        id,
        None,
        Some(customer_id),
        true,
        "Permanent deletion confirmed by the customer; the instance's data is being destroyed.",
    )
    .await?;
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
    let instance_limit = customer_instance_limit(&mut tx, customer_id).await?;
    let instance_count = customer_instance_count(&mut tx, customer_id).await?;
    if instance_limit.is_some_and(|limit| instance_count >= limit) {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "Delete another instance before restoring this one.".into(),
        ));
    }
    queue_restore(&s, &mut tx, id, None, Some(customer_id)).await?;
    tx.commit().await?;
    Ok(StatusCode::ACCEPTED)
}

/// Restore a retained tenant on an operator's behalf. Unlike a customer's own
/// restore it does not check the customer's instance limit: an operator can
/// grant the exception.
async fn restore_tenant(
    State(s): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let (operator_id, _) = operator(&headers, &s.db).await?;
    let mut tx = s.db.begin().await?;
    let retained = sqlx::query_scalar::<_, bool>(
        "SELECT true FROM tenants WHERE id=$1 AND lifecycle_state='retained' FOR UPDATE",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?
    .is_some();
    if !retained {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "Only a retained tenant can be restored.".into(),
        ));
    }
    queue_restore(&s, &mut tx, id, Some(operator_id), None).await?;
    tx.commit().await?;
    Ok(StatusCode::ACCEPTED)
}

/// Take a tenant offline on an operator's behalf while retaining all tenant
/// data for the normal restoration window.
async fn deprovision_tenant(
    State(s): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let (operator_id, _) = operator(&headers, &s.db).await?;
    let mut tx = s.db.begin().await?;
    let deletable = sqlx::query_scalar::<_, bool>(
        "SELECT true FROM tenants WHERE id=$1 AND lifecycle_state IN ('provisioning','ready','failed') FOR UPDATE",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?
    .is_some();
    if !deletable {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "This tenant is already deleted or being deleted.".into(),
        ));
    }
    begin_deprovisioning(&mut tx, id, Some(operator_id), None).await?;
    tx.commit().await?;
    temporal::wake_provisioning(&s, id).await;
    Ok(StatusCode::ACCEPTED)
}

/// Queue a retained tenant to be served again. Its database, owner and routing
/// survive, so it only needs its schema brought up to the runtime's release and
/// its configuration served.
async fn queue_restore(
    s: &Shared,
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: Uuid,
    operator_id: Option<Uuid>,
    customer_id: Option<Uuid>,
) -> Result<(), ApiError> {
    // An instance deleted because its custom domain never validated was never
    // set up. Restoring it tries that domain again, with a new window, rather
    // than setting it up under its provided hostname.
    let retried = sqlx::query_scalar::<_, String>(
        "SELECT abandoned_custom_domain FROM tenants t WHERE id=$1 AND abandoned_custom_domain IS NOT NULL AND NOT EXISTS(SELECT 1 FROM provisioning_steps p WHERE p.tenant_id=t.id AND p.step='owner' AND p.status='succeeded')",
    )
    .bind(tenant_id)
    .fetch_optional(&mut **tx)
    .await?;
    if let Some(hostname) = retried {
        let cname_target = s
            .cloudflare
            .as_ref()
            .map(|cloudflare| cloudflare.cname_target.clone())
            .ok_or_else(|| {
                ApiError(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "Cloudflare for SaaS is not configured.".into(),
                )
            })?;
        let taken = |error: sqlx::Error| {
            if matches!(
                error.as_database_error().and_then(|e| e.constraint()),
                Some("tenant_domains_hostname_key" | "tenants_domain_key")
            ) {
                ApiError(
                    StatusCode::CONFLICT,
                    "Its custom domain is now used by another instance, so it cannot be restored."
                        .into(),
                )
            } else {
                ApiError::from(error)
            }
        };
        sqlx::query("UPDATE tenants SET domain=$1,abandoned_custom_domain=NULL WHERE id=$2")
            .bind(&hostname)
            .bind(tenant_id)
            .execute(&mut **tx)
            .await
            .map_err(taken)?;
        sqlx::query("INSERT INTO tenant_domains(id,tenant_id,hostname,kind,status,is_primary,cname_target) VALUES($1,$2,$3,'custom','pending',true,$4)")
            .bind(Uuid::new_v4()).bind(tenant_id).bind(&hostname).bind(&cname_target)
            .execute(&mut **tx).await.map_err(taken)?;
    }
    sqlx::query("UPDATE tenants SET lifecycle_state='provisioning',desired_state='running',observed_state='unknown',observed_at=NULL,observation_error=NULL,purge_scheduled_for=NULL,updated_at=now() WHERE id=$1")
        .bind(tenant_id).execute(&mut **tx).await?;
    sqlx::query("INSERT INTO provisioning_steps(tenant_id,step) SELECT $1,unnest(ARRAY['migrate','render','reload','health']) ON CONFLICT (tenant_id,step) DO UPDATE SET status='pending',error=NULL,started_at=NULL,finished_at=NULL,updated_at=now()")
        .bind(tenant_id).execute(&mut **tx).await?;
    sqlx::query("DELETE FROM deprovisioning_steps WHERE tenant_id=$1")
        .bind(tenant_id)
        .execute(&mut **tx)
        .await?;
    let operation_id = Uuid::new_v4();
    sqlx::query("INSERT INTO operations(id,tenant_id,runtime_id,kind,status,requested_by,requested_by_customer,detail,started_at) SELECT $1,t.id,t.runtime_id,'tenant_provision','running',$2,$3,jsonb_build_object('mode','restore'),now() FROM tenants t WHERE t.id=$4")
        .bind(operation_id).bind(operator_id).bind(customer_id).bind(tenant_id).execute(&mut **tx).await?;
    sqlx::query("INSERT INTO operation_events(operation_id,message) VALUES($1,'Restore queued; retained data is served again once the instance is healthy.')")
        .bind(operation_id).execute(&mut **tx).await?;
    enqueue_temporal_dispatch(tx, tenant_id, "provision").await?;
    Ok(())
}

/// Start taking a tenant offline. Nothing is destroyed yet: the database, media,
/// configuration and hostnames stay until the purge scheduled for
/// `RETENTION_BEFORE_PURGE` later, which a restore cancels.
async fn begin_deprovisioning(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: Uuid,
    operator_id: Option<Uuid>,
    customer_id: Option<Uuid>,
) -> Result<(), sqlx::Error> {
    sqlx::query(&format!("UPDATE tenants SET lifecycle_state='deprovisioning',desired_state='deprovisioning',observed_state='disabled',observed_at=now(),observation_error=NULL,purge_scheduled_for=now()+interval '{RETENTION_BEFORE_PURGE}',updated_at=now() WHERE id=$1"))
        .bind(tenant_id).execute(&mut **tx).await?;
    sqlx::query("INSERT INTO deprovisioning_steps(tenant_id,step) SELECT $1,unnest(ARRAY['unrender','reload','offline']) ON CONFLICT (tenant_id,step) DO UPDATE SET status='pending',attempts=0,error=NULL,started_at=NULL,finished_at=NULL,updated_at=now()")
        .bind(tenant_id).execute(&mut **tx).await?;
    let operation_id = Uuid::new_v4();
    sqlx::query("INSERT INTO operations(id,tenant_id,runtime_id,kind,status,requested_by,requested_by_customer,started_at) SELECT $1,t.id,t.runtime_id,'tenant_deprovision','running',$2,$3,now() FROM tenants t WHERE t.id=$4")
        .bind(operation_id).bind(operator_id).bind(customer_id).bind(tenant_id).execute(&mut **tx).await?;
    sqlx::query("INSERT INTO operation_events(operation_id,message) VALUES($1,'Deprovisioning started; data is retained.')")
        .bind(operation_id).execute(&mut **tx).await?;
    enqueue_temporal_dispatch(tx, tenant_id, "deprovision").await?;
    Ok(())
}

async fn enqueue_temporal_dispatch(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: Uuid,
    kind: &str,
) -> Result<(), sqlx::Error> {
    let workflow_id = format!("tenant-{kind}-v3-{tenant_id}-{}", Uuid::new_v4());
    sqlx::query("INSERT INTO temporal_dispatches(id,tenant_id,workflow_kind,workflow_id) VALUES($1,$2,$3,$4) ON CONFLICT (workflow_id) DO NOTHING")
        .bind(Uuid::new_v4()).bind(tenant_id).bind(kind).bind(workflow_id)
        .execute(&mut **tx).await?;
    Ok(())
}

/// A deprovisioning or purge step that failed is retried this often, this many
/// times, before the operation is left failed for an operator.
const LIFECYCLE_RETRY_SECONDS: i64 = 60;
const LIFECYCLE_MAX_ATTEMPTS: i32 = 10;

/// A durable sequence that moves a tenant from one lifecycle state to the next.
struct LifecycleFlow {
    steps_table: &'static str,
    operation_kind: &'static str,
    running_state: &'static str,
    done_state: &'static str,
    steps: &'static [&'static str],
    /// What an exhausted step asks of an operator.
    exhausted: &'static str,
}

const DEPROVISION: LifecycleFlow = LifecycleFlow {
    steps_table: "deprovisioning_steps",
    operation_kind: "tenant_deprovision",
    running_state: "deprovisioning",
    done_state: "retained",
    steps: &["unrender", "reload", "offline"],
    exhausted: "an operator needs to take the instance offline",
};

const PURGE: LifecycleFlow = LifecycleFlow {
    steps_table: "purge_steps",
    operation_kind: "tenant_purge",
    running_state: "purging",
    done_state: "purged",
    steps: &[
        "offline",
        "cloudflare",
        "database",
        "media",
        "redis",
        "configuration",
    ],
    exhausted: "an operator needs to finish removing the tenant's data",
};

/// A lifecycle step failed and is to be tried again: the activity fails with
/// it, and Temporal's retry runs the flow again, which resumes at that step
/// once `LIFECYCLE_RETRY_SECONDS` have passed.
#[derive(Debug)]
pub(crate) struct LifecycleRetry(String);

impl std::fmt::Display for LifecycleRetry {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for LifecycleRetry {}

/// Run `flow` for `tenant_id` from its first unfinished step. A step that
/// fails is retried, `LIFECYCLE_RETRY_SECONDS` apart, until it has been tried
/// `LIFECYCLE_MAX_ATTEMPTS` times; then the operation is failed for an
/// operator and this returns, so that the workflow does not retry forever.
async fn run_lifecycle_flow(
    state: &Shared,
    tenant_id: Uuid,
    flow: &LifecycleFlow,
) -> Result<(), LifecycleRetry> {
    let table = flow.steps_table;
    for &step in flow.steps {
        let row = sqlx::query_as::<_, (String, i32, bool)>(&format!(
            "SELECT status,attempts,updated_at < now()-interval '{LIFECYCLE_RETRY_SECONDS} seconds' FROM {table} WHERE tenant_id=$1 AND step=$2"
        ))
        .bind(tenant_id)
        .bind(step)
        .fetch_optional(&state.db)
        .await;
        let (status, attempts, stale) = match row {
            Ok(Some(row)) => row,
            Ok(None) => return Ok(()),
            Err(_) => {
                return Err(LifecycleRetry(
                    "Lifecycle state unavailable; retrying.".into(),
                ));
            }
        };
        match status.as_str() {
            "succeeded" => continue,
            "failed" if attempts >= LIFECYCLE_MAX_ATTEMPTS => return Ok(()),
            "failed" if !stale => {
                return Err(LifecycleRetry(format!("{step} is waiting to be retried")));
            }
            _ => {}
        }
        if let Err(_error) = sqlx::query(&format!("UPDATE {table} SET status='running',attempts=attempts+1,error=NULL,started_at=now(),finished_at=NULL,updated_at=now() WHERE tenant_id=$1 AND step=$2"))
            .bind(tenant_id).bind(step).execute(&state.db).await
        {
            return Err(LifecycleRetry("Lifecycle state update failed; retrying.".into()));
        }
        let result = lifecycle_step(state, tenant_id, step).await;
        let operation = sqlx::query_scalar::<_, Uuid>("SELECT id FROM operations WHERE tenant_id=$1 AND kind=$2 AND status='running' ORDER BY created_at DESC LIMIT 1")
            .bind(tenant_id).bind(flow.operation_kind).fetch_optional(&state.db).await.ok().flatten();
        let note = match result {
            Ok(note) => note,
            Err(_) => {
                let exhausted = attempts + 1 >= LIFECYCLE_MAX_ATTEMPTS;
                let message = if exhausted {
                    format!("{step} failed; {}.", flow.exhausted)
                } else {
                    format!("{step} failed; retrying.")
                };
                let _ = sqlx::query(&format!("UPDATE {table} SET status='failed',error=$1,finished_at=now(),updated_at=now() WHERE tenant_id=$2 AND step=$3"))
                    .bind(&message).bind(tenant_id).bind(step).execute(&state.db).await;
                if let Some(operation) = operation {
                    release_event(&state.db, operation, &message).await;
                    if exhausted {
                        mark_operation_failed(&state.db, operation, &message).await;
                    }
                }
                return if exhausted {
                    Ok(())
                } else {
                    Err(LifecycleRetry(message))
                };
            }
        };
        let _ = sqlx::query(&format!("UPDATE {table} SET status='succeeded',finished_at=now(),updated_at=now() WHERE tenant_id=$1 AND step=$2"))
            .bind(tenant_id).bind(step).execute(&state.db).await;
        if let Some(operation) = operation {
            release_event(
                &state.db,
                operation,
                &note.unwrap_or_else(|| format!("{step} completed.")),
            )
            .await;
        }
    }
    let finished = if flow.done_state == "purged" {
        "UPDATE tenants SET lifecycle_state=$1,purged_at=now(),updated_at=now() WHERE id=$2 AND lifecycle_state=$3"
    } else {
        "UPDATE tenants SET lifecycle_state=$1,updated_at=now() WHERE id=$2 AND lifecycle_state=$3"
    };
    let _ = sqlx::query(finished)
        .bind(flow.done_state)
        .bind(tenant_id)
        .bind(flow.running_state)
        .execute(&state.db)
        .await;
    let _ = sqlx::query("UPDATE operations SET status='succeeded',finished_at=now() WHERE id=(SELECT id FROM operations WHERE tenant_id=$1 AND kind=$2 AND status='running' ORDER BY created_at DESC LIMIT 1)")
        .bind(tenant_id).bind(flow.operation_kind).execute(&state.db).await;
    Ok(())
}

/// Run one step of a lifecycle flow. A step that deliberately left something in
/// place says why, and that note replaces its "completed" event.
async fn lifecycle_step(state: &Shared, tenant_id: Uuid, step: &str) -> Result<Option<String>> {
    match step {
        "unrender" => unrender_tenant(state, tenant_id).await.map(|()| None),
        // Purging a tenant that is still served takes it offline in one step,
        // so a retry removes a configuration that an in-flight provisioning or
        // domain step rendered again after the first attempt.
        "offline" if purging(state, tenant_id).await? => {
            unrender_tenant(state, tenant_id).await?;
            reload_runtime(state, tenant_id, false).await?;
            verify_tenant_offline(state, tenant_id).await.map(|()| None)
        }
        // The tenant's file is gone, so the runtime answering its hostname
        // with 421 afterwards is success, not a rejected reload.
        "reload" => reload_runtime(state, tenant_id, false).await.map(|()| None),
        "offline" => verify_tenant_offline(state, tenant_id).await.map(|()| None),
        "cloudflare" => purge_cloudflare_hostnames(state, tenant_id).await,
        "database" => purge_tenant_database(state, tenant_id).await,
        "media" => purge_tenant_media(state, tenant_id).await,
        "redis" => purge_tenant_redis(state, tenant_id).await,
        "configuration" => purge_tenant_configuration(state, tenant_id).await,
        _ => anyhow::bail!("unknown lifecycle step"),
    }
}

async fn purging(state: &Shared, tenant_id: Uuid) -> Result<bool> {
    Ok(
        sqlx::query_scalar::<_, String>("SELECT lifecycle_state FROM tenants WHERE id=$1")
            .bind(tenant_id)
            .fetch_one(&state.db)
            .await?
            == "purging",
    )
}

async fn purge_tenant(
    State(s): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(body): Json<PurgeConfirmation>,
) -> Result<StatusCode, ApiError> {
    let (operator_id, _) = operator(&headers, &s.db).await?;
    let mut tx = s.db.begin().await?;
    // A tenant already on its way offline finishes that first; anything else,
    // served or not, can be purged.
    // A tenant whose custom domain was released is confirmed by that domain; its
    // own `domain` is only a placeholder.
    let (domain, retained) = sqlx::query_as::<_, (String, bool)>(
        "SELECT COALESCE(abandoned_custom_domain,domain),lifecycle_state='retained' FROM tenants WHERE id=$1 AND lifecycle_state IN ('provisioning','ready','failed','retained') FOR UPDATE",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| {
        ApiError(
            StatusCode::CONFLICT,
            "This tenant is already being deprovisioned or purged.".into(),
        )
    })?;
    if !body.confirm_domain.trim().eq_ignore_ascii_case(&domain) {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "Type the tenant's domain to confirm the purge.".into(),
        ));
    }
    start_purge(
        &mut tx,
        id,
        Some(operator_id),
        None,
        retained,
        "Purge confirmed; the tenant's data is being destroyed.",
    )
    .await?;
    tx.commit().await?;
    Ok(StatusCode::ACCEPTED)
}

/// Begin purging a tenant. A retained tenant is already offline, so its
/// `offline` step is recorded as done.
async fn start_purge(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: Uuid,
    operator_id: Option<Uuid>,
    customer_id: Option<Uuid>,
    retained: bool,
    reason: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE tenants SET lifecycle_state='purging',desired_state='deprovisioning',observed_state='disabled',observed_at=now(),observation_error=NULL,purge_scheduled_for=NULL,updated_at=now() WHERE id=$1")
        .bind(tenant_id)
        .execute(&mut **tx)
        .await?;
    sqlx::query("INSERT INTO purge_steps(tenant_id,step) SELECT $1,unnest(ARRAY['offline','cloudflare','database','media','redis','configuration']) ON CONFLICT (tenant_id,step) DO UPDATE SET status='pending',attempts=0,error=NULL,started_at=NULL,finished_at=NULL,updated_at=now()")
        .bind(tenant_id).execute(&mut **tx).await?;
    if retained {
        sqlx::query("UPDATE purge_steps SET status='succeeded',finished_at=now() WHERE tenant_id=$1 AND step='offline'")
            .bind(tenant_id).execute(&mut **tx).await?;
    }
    let operation_id = Uuid::new_v4();
    sqlx::query("INSERT INTO operations(id,tenant_id,runtime_id,kind,status,requested_by,requested_by_customer,detail,started_at) SELECT $1,t.id,t.runtime_id,'tenant_purge','running',$2,$3,jsonb_build_object('domain',COALESCE(t.abandoned_custom_domain,t.domain)),now() FROM tenants t WHERE t.id=$4")
        .bind(operation_id).bind(operator_id).bind(customer_id).bind(tenant_id).execute(&mut **tx).await?;
    sqlx::query("INSERT INTO operation_events(operation_id,message) VALUES($1,$2)")
        .bind(operation_id)
        .bind(reason)
        .execute(&mut **tx)
        .await?;
    enqueue_temporal_dispatch(tx, tenant_id, "purge").await?;
    Ok(())
}

/// Remove the tenant's Cloudflare for SaaS hostnames, then its domain records,
/// which frees a customer's own custom domain for use elsewhere. The tenant's
/// primary domain stays reserved on the tenant row.
async fn purge_cloudflare_hostnames(state: &Shared, tenant_id: Uuid) -> Result<Option<String>> {
    let registered = sqlx::query_scalar::<_, String>(
        "SELECT cloudflare_id FROM tenant_domains WHERE tenant_id=$1 AND cloudflare_id IS NOT NULL",
    )
    .bind(tenant_id)
    .fetch_all(&state.db)
    .await?;
    if !registered.is_empty() {
        let cloudflare = state
            .cloudflare
            .as_ref()
            .context("Cloudflare is not configured")?;
        for cloudflare_id in registered {
            anyhow::ensure!(
                cloudflare_id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-'),
                "invalid Cloudflare hostname id"
            );
            let response = state
                .client
                .delete(format!(
                    "{}/{cloudflare_id}",
                    cloudflare.custom_hostnames_url()
                ))
                .bearer_auth(&cloudflare.api_token)
                .send()
                .await?;
            anyhow::ensure!(
                response.status().is_success() || response.status() == StatusCode::NOT_FOUND,
                "Cloudflare refused to delete a hostname"
            );
            sqlx::query("UPDATE tenant_domains SET cloudflare_id=NULL,updated_at=now() WHERE tenant_id=$1 AND cloudflare_id=$2")
                .bind(tenant_id).bind(&cloudflare_id).execute(&state.db).await?;
        }
    }
    sqlx::query("DELETE FROM tenant_domains WHERE tenant_id=$1")
        .bind(tenant_id)
        .execute(&state.db)
        .await?;
    Ok(None)
}

/// Drop the tenant's database and role — only if they are the ones provisioning
/// created for exactly this tenant. An imported tenant's database is left alone.
async fn purge_tenant_database(state: &Shared, tenant_id: Uuid) -> Result<Option<String>> {
    let Some(database_url) = sqlx::query_scalar::<_, String>(
        "SELECT database_url FROM tenant_configuration WHERE tenant_id=$1",
    )
    .bind(tenant_id)
    .fetch_optional(&state.db)
    .await?
    else {
        return Ok(Some("database has no configuration left to purge.".into()));
    };
    let Some((role, database)) = provisioned_database_names(tenant_id, &database_url) else {
        return Ok(Some(
            "database was not created by provisioning for this tenant; it was left in place."
                .into(),
        ));
    };
    let mut admin_url = url::Url::parse(&state.provision_database_url)?;
    admin_url.set_path("/postgres");
    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect(admin_url.as_str())
        .await?;
    // Both names are derived from the tenant id above, never from stored text.
    let dropped = async {
        // Dropping a database takes the privileges of its owner. A superuser has
        // them; an account with CREATEROLE can take them by granting itself the
        // tenant's role, which PostgreSQL 16 gave it ADMIN OPTION on.
        let can_drop_as_superuser = sqlx::query_scalar::<_, bool>(
            "SELECT rolsuper FROM pg_roles WHERE rolname=current_user",
        )
        .fetch_one(&admin)
        .await?;
        let role_exists =
            sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM pg_roles WHERE rolname=$1)")
                .bind(&role)
                .fetch_one(&admin)
                .await?;
        if !can_drop_as_superuser && role_exists {
            sqlx::query(&format!("GRANT {role} TO CURRENT_USER"))
                .execute(&admin)
                .await?;
        }
        sqlx::query(&format!("DROP DATABASE IF EXISTS {database} WITH (FORCE)"))
            .execute(&admin)
            .await?;
        sqlx::query(&format!("DROP ROLE IF EXISTS {role}"))
            .execute(&admin)
            .await
    }
    .await;
    admin.close().await;
    dropped?;
    Ok(None)
}

/// The role and database provisioning created for this tenant, if `database_url`
/// names exactly those. Names are derived from the tenant id, never taken from
/// the stored URL, so they are safe to put in DDL.
fn provisioned_database_names(tenant_id: Uuid, database_url: &str) -> Option<(String, String)> {
    let compact = tenant_id.simple().to_string();
    let (role, database) = (format!("eunha_r_{compact}"), format!("eunha_t_{compact}"));
    let url = url::Url::parse(database_url).ok()?;
    (url.username() == role && url.path().trim_start_matches('/') == database)
        .then_some((role, database))
}

/// Delete every media object under the tenant's own `t/<tenant id>/` prefix.
/// Media stored anywhere else, such as an imported tenant's dedicated bucket, is
/// left alone.
async fn purge_tenant_media(state: &Shared, tenant_id: Uuid) -> Result<Option<String>> {
    use rusty_s3::{
        Bucket, Credentials, S3Action, UrlStyle,
        actions::{DeleteObjects, DeleteObjectsResponse, ListObjectsV2, ObjectIdentifier},
    };
    let Some((bucket_name, endpoint, region, access_key_id, secret_access_key, key_prefix)) =
        sqlx::query_as::<_, (String, Option<String>, String, String, String, String)>(
            "SELECT b.bucket_name,c.media_storage_endpoint,c.media_storage_region,c.media_storage_access_key_id,c.media_storage_secret_access_key,c.media_storage_key_prefix FROM tenant_configuration c JOIN r2_buckets b ON b.tenant_id=c.tenant_id WHERE c.tenant_id=$1",
        )
        .bind(tenant_id)
        .fetch_optional(&state.db)
        .await?
    else {
        return Ok(Some("media has no configuration left to purge.".into()));
    };
    let prefix = format!("t/{tenant_id}");
    if key_prefix.trim_matches('/') != prefix {
        return Ok(Some(
            "media is not under this tenant's own storage prefix; it was left in place.".into(),
        ));
    }
    let endpoint = url::Url::parse(&endpoint.context("media storage has no endpoint")?)?;
    let bucket = Bucket::new(endpoint, UrlStyle::Path, bucket_name, region)?;
    let credentials = Credentials::new(access_key_id, secret_access_key);
    let objects_prefix = format!("{prefix}/");
    let expires = Duration::from_secs(300);
    let mut deleted = 0usize;
    let mut previous_first: Option<String> = None;
    loop {
        let mut list = ListObjectsV2::new(&bucket, Some(&credentials));
        list.with_prefix(objects_prefix.as_str());
        list.with_max_keys(1000);
        let listing = state
            .client
            .get(list.sign(expires))
            .timeout(Duration::from_secs(60))
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;
        let keys: Vec<ObjectIdentifier> = ListObjectsV2::parse_response(&listing)?
            .contents
            .into_iter()
            .map(|object| object.key)
            // The listing is already prefixed; this only refuses to act on
            // anything a misbehaving endpoint returns outside it.
            .filter(|key| key.starts_with(&objects_prefix))
            .map(ObjectIdentifier::new)
            .collect();
        let Some(first) = keys.first().map(|object| object.key.clone()) else {
            break;
        };
        anyhow::ensure!(
            previous_first.as_deref() != Some(first.as_str()),
            "media objects are not being deleted"
        );
        previous_first = Some(first);
        let mut delete = DeleteObjects::new(&bucket, Some(&credentials), keys.iter());
        delete.set_quiet(true);
        let url = delete.sign(expires);
        let (body, content_md5) = delete.body_with_md5();
        let response = state
            .client
            .post(url)
            .header("Content-MD5", content_md5)
            .body(body)
            .timeout(Duration::from_secs(60))
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;
        anyhow::ensure!(
            DeleteObjectsResponse::parse(&response)?.errors.is_empty(),
            "some media objects could not be deleted"
        );
        deleted += keys.len();
    }
    Ok(Some(format!("media completed; {deleted} objects deleted.")))
}

/// Delete the tenant's Redis keys under its own prefix. They are cache and
/// coordination state under a prefix nothing else uses, so a Redis that refuses
/// is noted rather than blocking the purge.
async fn purge_tenant_redis(state: &Shared, tenant_id: Uuid) -> Result<Option<String>> {
    let Some((redis_url, coordination_url, key_prefix)) =
        sqlx::query_as::<_, (String, Option<String>, String)>(
            "SELECT redis_url,redis_coordination_url,redis_key_prefix FROM tenant_configuration WHERE tenant_id=$1",
        )
        .bind(tenant_id)
        .fetch_optional(&state.db)
        .await?
    else {
        return Ok(Some("redis has no configuration left to purge.".into()));
    };
    if key_prefix != tenant_redis_key_prefix(tenant_id) {
        return Ok(Some(
            "redis keys are not under this tenant's own prefix; they were left in place.".into(),
        ));
    }
    let mut urls = vec![redis_url];
    urls.extend(coordination_url.filter(|url| !urls.contains(url)));
    let pattern = format!("{key_prefix}:*");
    let mut deleted = 0usize;
    for url in urls {
        match delete_redis_keys(&url, &pattern).await {
            Ok(count) => deleted += count,
            Err(error) => {
                // The error names the command that failed, never the URL.
                return Ok(Some(format!(
                    "redis keys could not all be removed ({:?}); they can be cleared by hand with the pattern {pattern}.",
                    error.kind()
                )));
            }
        }
    }
    Ok(Some(format!("redis completed; {deleted} keys deleted.")))
}

async fn delete_redis_keys(url: &str, pattern: &str) -> redis::RedisResult<usize> {
    let client = redis::Client::open(url)?;
    let mut connection = client.get_multiplexed_async_connection().await?;
    let mut cursor: u64 = 0;
    let mut deleted = 0usize;
    loop {
        let (next, keys): (u64, Vec<String>) = redis::cmd("SCAN")
            .arg(cursor)
            .arg("MATCH")
            .arg(pattern)
            .arg("COUNT")
            .arg(1000)
            .query_async(&mut connection)
            .await?;
        if !keys.is_empty() {
            let removed: usize = redis::cmd("DEL")
                .arg(&keys)
                .query_async(&mut connection)
                .await?;
            deleted += removed;
        }
        if next == 0 {
            return Ok(deleted);
        }
        cursor = next;
    }
}

/// Discard the tenant's configuration and remaining secrets. The tenant row
/// stays as a purged record that keeps its domain reserved.
async fn purge_tenant_configuration(state: &Shared, tenant_id: Uuid) -> Result<Option<String>> {
    let mut tx = state.db.begin().await?;
    sqlx::query("DELETE FROM tenant_configuration WHERE tenant_id=$1")
        .bind(tenant_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "UPDATE r2_buckets SET lifecycle_state='deleted',updated_at=now() WHERE tenant_id=$1",
    )
    .bind(tenant_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query("UPDATE tenants SET vapid_private_key=NULL,vapid_public_key=NULL,updated_at=now() WHERE id=$1")
        .bind(tenant_id)
        .execute(&mut *tx)
        .await?;
    // A purged tenant keeps its domain reserved because remote servers remember
    // the accounts that lived there. One that was never set up had no accounts,
    // so its domain goes back to whoever controls it.
    sqlx::query("UPDATE tenants SET domain=id::text||'.invalid',aliases='{}' WHERE id=$1 AND domain<>id::text||'.invalid' AND NOT EXISTS(SELECT 1 FROM provisioning_steps p WHERE p.tenant_id=tenants.id AND p.step='database' AND p.status='succeeded') AND EXISTS(SELECT 1 FROM provisioning_steps p WHERE p.tenant_id=tenants.id)")
        .bind(tenant_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(None)
}

/// Remove the tenant's runtime configuration so the runtime stops serving it on
/// its next reload. The configuration itself stays in PostgreSQL.
async fn unrender_tenant(state: &Shared, tenant_id: Uuid) -> Result<()> {
    let (directory, runtime_host) = sqlx::query_as::<_, (String, String)>(
        "SELECT r.tenant_directory,i.address FROM tenants t JOIN runtimes r ON r.id=t.runtime_id JOIN instances i ON i.id=r.instance_id WHERE t.id=$1",
    )
    .bind(tenant_id)
    .fetch_one(&state.db)
    .await?;
    let configured_host = std::env::var("CONTROL_RUNTIME_HOST").unwrap_or_default();
    anyhow::ensure!(
        !configured_host.is_empty() && configured_host == runtime_host,
        "runtime is managed elsewhere"
    );
    match tokio::fs::remove_file(PathBuf::from(directory).join(tenant_file_name(tenant_id))).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

/// Rename files still named after their tenant's domain to the tenant's id.
/// Renaming within a directory is atomic, so the runtime never sees two files
/// for one domain, and the contents are unchanged, so a reload leaves the
/// tenant running. Only this host's runtimes are touched.
async fn rename_legacy_tenant_files(state: &Shared) -> Result<()> {
    let configured_host = std::env::var("CONTROL_RUNTIME_HOST").unwrap_or_default();
    if configured_host.is_empty() {
        return Ok(());
    }
    let tenants = sqlx::query_as::<_, (Uuid, String, String)>(
        "SELECT t.id,t.domain,r.tenant_directory FROM tenants t JOIN runtimes r ON r.id=t.runtime_id JOIN instances i ON i.id=r.instance_id WHERE i.address=$1 AND t.lifecycle_state<>'purged'",
    )
    .bind(&configured_host)
    .fetch_all(&state.db)
    .await?;
    for (tenant_id, domain, directory) in tenants {
        if !is_safe_domain(&domain) {
            continue;
        }
        let directory = PathBuf::from(directory);
        let legacy = directory.join(legacy_tenant_file_name(&domain));
        let current = directory.join(tenant_file_name(tenant_id));
        if !tokio::fs::try_exists(&legacy).await? {
            continue;
        }
        if tokio::fs::try_exists(&current).await? {
            // Rendered under both names; the id-named file is the newer one.
            tokio::fs::remove_file(&legacy).await?;
        } else {
            tokio::fs::rename(&legacy, &current).await?;
        }
        tracing::info!(%tenant_id, %domain, "renamed tenant configuration file to its tenant id");
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

/// The runtime rejects its whole tenants directory when the pools its tenants
/// declare exceed what Postgres will accept, so a tenant that does not fit is
/// turned away here rather than after its database exists.
fn connection_budget_error(declared: i64, accepted: i64) -> Option<String> {
    (declared > accepted).then(|| {
        format!(
            "this server is full: its instances would open {declared} database connections and it accepts {accepted}"
        )
    })
}

async fn runtime_connection_budget(db: &PgPool, tenant_id: Uuid) -> Result<()> {
    // Tenants being deleted no longer have configuration on the runtime, so
    // they are not counted; a failed one still does.
    let (declared, accepted) = sqlx::query_as::<_, (i64, i64)>(
        "SELECT (SELECT COALESCE(SUM(c.database_pool_max_connections),0) FROM tenants peer JOIN tenant_configuration c ON c.tenant_id=peer.id WHERE peer.runtime_id=t.runtime_id AND peer.lifecycle_state NOT IN ('deprovisioning','retained','purging','purged')),current_setting('max_connections')::bigint-current_setting('superuser_reserved_connections')::bigint FROM tenants t WHERE t.id=$1",
    )
    .bind(tenant_id)
    .fetch_one(db)
    .await?;
    match connection_budget_error(declared, accepted) {
        Some(message) => anyhow::bail!(message),
        None => Ok(()),
    }
}

// Only reconstructed numeric capacity diagnostics may cross the audit boundary.
fn safe_connection_budget_reason(reason: &str) -> Option<String> {
    let rest = reason.strip_prefix("this server is full: its instances would open ")?;
    let (declared, accepted) = rest.split_once(" database connections and it accepts ")?;
    let declared: i64 = declared.parse().ok()?;
    let accepted: i64 = accepted.parse().ok()?;
    let safe = connection_budget_error(declared, accepted)?;
    (safe == reason).then_some(safe)
}

async fn fail_provisioning(db: &PgPool, tenant_id: Uuid, step: &str, reason: Option<&str>) {
    let safe_reason = reason.and_then(safe_connection_budget_reason);
    let message = match safe_reason {
        Some(reason) => format!("{step} failed: {reason}; retry is safe."),
        None => format!("{step} failed; retry is safe."),
    };
    let _ = sqlx::query("UPDATE provisioning_steps SET status='failed',error=$1,finished_at=now(),updated_at=now() WHERE tenant_id=$2 AND step=$3")
        .bind(&message).bind(tenant_id).bind(step).execute(db).await;
    let _ = sqlx::query("UPDATE tenants SET lifecycle_state='failed',observed_state='unhealthy',observation_error=$1,observed_at=now(),updated_at=now() WHERE id=$2 AND lifecycle_state='provisioning'")
        .bind(&message).bind(tenant_id).execute(db).await;
    let _ = sqlx::query("UPDATE operations SET status='failed',error=$1,finished_at=now() WHERE id=(SELECT id FROM operations WHERE tenant_id=$2 AND kind='tenant_provision' AND status='running' ORDER BY created_at DESC LIMIT 1)")
        .bind(&message).bind(tenant_id).execute(db).await;
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub(crate) enum ProvisionStepOutcome {
    Completed,
    Wait,
    Halted,
    Failed,
}

pub(crate) async fn provision_tenant_step(
    state: &Shared,
    tenant_id: Uuid,
    step: &str,
) -> ProvisionStepOutcome {
    // A purge can start while provisioning is under way; stop between steps
    // rather than render a tenant that is being destroyed.
    match sqlx::query_scalar::<_, String>("SELECT lifecycle_state FROM tenants WHERE id=$1")
        .bind(tenant_id)
        .fetch_one(&state.db)
        .await
    {
        Ok(lifecycle) if lifecycle == "provisioning" => {}
        _ => return ProvisionStepOutcome::Halted,
    }
    let ready = match step {
        "database" => match runtime_connection_budget(&state.db, tenant_id).await {
            Ok(()) => custom_domain_validated(&state.db, tenant_id).await,
            Err(error) => Err(error),
        },
        "routing" => tenant_routing_ready(&state.db, tenant_id).await,
        _ => Ok(true),
    };
    match ready {
        Ok(true) => {}
        Ok(false) => return ProvisionStepOutcome::Wait,
        Err(error) => {
            fail_provisioning(&state.db, tenant_id, step, Some(&error.to_string())).await;
            return ProvisionStepOutcome::Failed;
        }
    }
    let should_run = match provisioning_step(state, tenant_id, step).await {
        Ok(value) => value,
        Err(_) => return ProvisionStepOutcome::Halted,
    };
    if !should_run {
        return ProvisionStepOutcome::Completed;
    }
    let result = match step {
        "database" => create_tenant_database(state, tenant_id).await,
        "migrate" => migrate_tenant_database(state, tenant_id).await,
        "import" => import_tenant_database(state, tenant_id).await,
        "media" => upload_imported_media(state, tenant_id).await,
        "render" => render_provisioned_tenant(state, tenant_id)
            .await
            .map(|_| ()),
        "reload" => reload_provisioned_runtime(state, tenant_id).await,
        "health" => healthcheck_provisioned_tenant(state, tenant_id).await,
        "routing" => Ok(()),
        "owner" => create_tenant_owner(state, tenant_id).await,
        _ => unreachable!(),
    };
    if let Err(error) = result {
        fail_provisioning(&state.db, tenant_id, step, Some(&error.to_string())).await;
        return ProvisionStepOutcome::Failed;
    }
    if finish_provisioning_step(&state.db, tenant_id, step)
        .await
        .is_err()
    {
        return ProvisionStepOutcome::Halted;
    }
    ProvisionStepOutcome::Completed
}

pub(crate) async fn finish_provisioning(state: &Shared, tenant_id: Uuid) {
    let _ = sqlx::query("UPDATE tenants SET lifecycle_state='ready',observed_state='healthy',observed_at=now(),observation_error=NULL,updated_at=now() WHERE id=$1 AND lifecycle_state='provisioning'")
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
        "SELECT status FROM tenant_domains WHERE tenant_id=$1 AND kind IN ('provided','custom')",
    )
    .bind(tenant_id)
    .fetch_all(db)
    .await?;
    anyhow::ensure!(!statuses.is_empty(), "tenant has no managed hostname");
    anyhow::ensure!(
        !statuses.iter().any(|status| status == "failed"),
        "hostname routing failed"
    );
    Ok(statuses.iter().all(|status| status == "active"))
}

/// Whether an instance created with a custom domain may be set up. Nothing is
/// provisioned — no database, no runtime file, no owner — until its domain has
/// validated, so an instance whose domain never validates leaves nothing behind.
///
/// An imported instance is the exception, and for the same reason. Its domain
/// is already serving somewhere else, so waiting for it would mean waiting for
/// a cutover onto a server with no data on it. It is set up first and cut over
/// afterwards; what it leaves behind if its domain never moves is a database
/// full of the customer's own instance, which is not litter.
async fn custom_domain_validated(db: &PgPool, tenant_id: Uuid) -> Result<bool> {
    Ok(sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM tenant_imports WHERE tenant_id=$1) OR NOT EXISTS(SELECT 1 FROM tenant_domains WHERE tenant_id=$1 AND kind='custom' AND is_primary AND status<>'active')",
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
        "SELECT t.owner_username,t.owner_email,tc.database_url FROM tenants t JOIN tenant_configuration tc ON tc.tenant_id=t.id WHERE t.id=$1 AND t.owner_username IS NOT NULL AND t.owner_email IS NOT NULL",
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
    let owner = sqlx::query_as::<_, (Option<String>, Option<String>, bool, Uuid)>(
        "SELECT t.owner_username,t.owner_email,t.lifecycle_state='ready' AND EXISTS(SELECT 1 FROM provisioning_steps p WHERE p.tenant_id=t.id AND p.step='owner' AND p.status='succeeded'),t.runtime_id FROM tenants t WHERE t.id=$1 AND t.customer_id=$2",
    )
    .bind(id)
    .bind(customer_id)
    .fetch_optional(&s.db)
    .await?
    .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "Instance not found".into()))?;
    let (Some(username), Some(email), true, runtime_id) = owner else {
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
        Json(serde_json::json!({ "username": username, "email": email, "password": password })),
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
    // Eunha keeps its own tables in the `eunha` schema and finds them through
    // `search_path`. Setting that on the role rather than on each connection is
    // what makes a pooled connection safe: behind a transaction pooler a server
    // connection is handed to clients that never ran `SET search_path`, and one
    // opened after them never ran it at all.
    sqlx::query(&format!(
        "ALTER ROLE {role} SET search_path TO eunha, public"
    ))
    .execute(&admin)
    .await?;
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

/// Restore the instance's Mastodon dump into the database the previous steps
/// created and migrated, with the runtime's own `eunha import-mastodon`.
///
/// The runtime's binary does the work for the same reason `migrate` uses it: it
/// is the thing that knows which Mastodon release this fleet builds, and the
/// release that matters is the one the tenant will be served by rather than
/// whatever the control plane happened to be built against. It also refuses
/// what should be refused — a dump from another release, a database that is not
/// empty, data belonging to a different instance — so this step's job is to run
/// it in the right place, as the right role, and keep what it reported.
///
/// It connects as the provisioning role rather than the tenant's, because a
/// data-only restore loads through the schema's foreign keys and only a
/// superuser may disable those.
async fn import_tenant_database(state: &Shared, tenant_id: Uuid) -> Result<()> {
    let (domain, dump_path, database_url, tenant_directory, runtime_host) =
        sqlx::query_as::<_, (String, String, String, String, String)>(
            "SELECT t.domain,m.dump_path,c.database_url,r.tenant_directory,i.address FROM tenants t JOIN tenant_imports m ON m.tenant_id=t.id JOIN tenant_configuration c ON c.tenant_id=t.id JOIN runtimes r ON r.id=t.runtime_id JOIN instances i ON i.id=r.instance_id WHERE t.id=$1",
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
    let dump = PathBuf::from(&dump_path);
    anyhow::ensure!(is_safe_absolute_path(&dump), "invalid dump path");
    anyhow::ensure!(
        tokio::fs::try_exists(&dump).await.unwrap_or(false),
        "there is no dump at {dump_path} on this runtime"
    );

    // The tenant's own database, reached by the role that provisioned it. Only
    // the path is taken from the tenant's URL; its credentials are not the ones
    // that may disable a trigger.
    let tenant_database = url::Url::parse(&database_url)?;
    let mut admin_url = url::Url::parse(&state.provision_database_url)?;
    admin_url.set_path(tenant_database.path());

    let runtime_root = PathBuf::from(tenant_directory)
        .parent()
        .context("runtime directory has no parent")?
        .to_path_buf();
    // However long it takes: a restore is one transaction over the whole of
    // somebody's server, and killing it halfway is worth nothing extra.
    let output = tokio::process::Command::new(runtime_root.join("current/eunha"))
        .arg("import-mastodon")
        .arg(&dump)
        .arg("--domain")
        .arg(&domain)
        .env("DATABASE_URL", admin_url.as_str())
        .current_dir(runtime_root.join("current"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await?;
    anyhow::ensure!(
        output.status.success(),
        "{}",
        import_failure(&String::from_utf8_lossy(&output.stderr))
    );
    record_import_evidence(state, tenant_id, &String::from_utf8_lossy(&output.stdout)).await
}

/// Upload the instance's media into the storage its rendered configuration
/// names, with the runtime's own `eunha import-media`.
///
/// Mastodon stores a file's name rather than its address, so an instance whose
/// media stayed behind serves rows that point at nothing. The runtime's binary
/// reads the tenant file the render just wrote, which is the only place the
/// tenant's bucket and its key prefix are stated together — and the prefix is
/// the whole of what keeps one instance's objects apart from its neighbours' in
/// a bucket they share.
///
/// A retry asks for each object before sending it. The first attempt does not:
/// over a whole media library that would be a request per file for nothing.
async fn upload_imported_media(state: &Shared, tenant_id: Uuid) -> Result<()> {
    let (domain, media_path, tenant_directory, runtime_host, attempts) =
        sqlx::query_as::<_, (String, String, String, String, i32)>(
            "SELECT t.domain,m.media_path,r.tenant_directory,i.address,p.attempts FROM tenants t JOIN tenant_imports m ON m.tenant_id=t.id JOIN runtimes r ON r.id=t.runtime_id JOIN instances i ON i.id=r.instance_id JOIN provisioning_steps p ON p.tenant_id=t.id AND p.step='media' WHERE t.id=$1 AND m.media_path IS NOT NULL",
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
    let media = PathBuf::from(&media_path);
    anyhow::ensure!(is_safe_absolute_path(&media), "invalid media path");
    anyhow::ensure!(
        tokio::fs::metadata(&media)
            .await
            .map(|entry| entry.is_dir())
            .unwrap_or(false),
        "there is no media directory at {media_path} on this runtime"
    );

    let runtime_root = PathBuf::from(&tenant_directory)
        .parent()
        .context("runtime directory has no parent")?
        .to_path_buf();
    let mut command = tokio::process::Command::new(runtime_root.join("current/eunha"));
    command
        .arg("--tenants")
        .arg(&tenant_directory)
        .arg("import-media")
        .arg(&media)
        .arg(format!("--instance={domain}"));
    // The instance's own files only. The rest of a Mastodon media directory is
    // its cache of what other servers published, which is fetched again when it
    // is missing and is usually most of the bytes.
    if attempts > 1 {
        command.arg("--skip-existing");
    }
    let output = command
        .current_dir(runtime_root.join("current"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await?;
    anyhow::ensure!(
        output.status.success(),
        "{}",
        import_failure(&String::from_utf8_lossy(&output.stderr))
    );
    record_import_evidence(state, tenant_id, &String::from_utf8_lossy(&output.stdout)).await
}

/// Keep what an importer reported, and say it where an operator is looking.
///
/// The row is the record; the operation event is how it is read at the time it
/// matters, beside the step that produced it. Merged rather than replaced,
/// because the restore's evidence and the media's are both what the import is
/// answerable for.
async fn record_import_evidence(state: &Shared, tenant_id: Uuid, stdout: &str) -> Result<()> {
    let report = imported_instance_report(stdout);
    sqlx::query("UPDATE tenant_imports SET report=COALESCE(report,'{}'::jsonb)||$1::jsonb,updated_at=now() WHERE tenant_id=$2")
        .bind(SqlJson(&report))
        .bind(tenant_id)
        .execute(&state.db)
        .await?;
    let Some(fields) = report.as_object().filter(|fields| !fields.is_empty()) else {
        return Ok(());
    };
    let summary = fields
        .iter()
        .map(|(key, value)| format!("{key} {}", value.to_string().trim_matches('"')))
        .collect::<Vec<_>>()
        .join(", ");
    sqlx::query("INSERT INTO operation_events(operation_id,message) SELECT id,$2 FROM operations WHERE tenant_id=$1 AND kind='tenant_provision' AND status='running' ORDER BY created_at DESC LIMIT 1")
        .bind(tenant_id)
        .bind(format!("Imported: {summary}."))
        .execute(&state.db)
        .await?;
    Ok(())
}

/// The report an importer prints after `OK`: from the restore, the domain it
/// proved the data belongs to and the row counts that say the instance arrived
/// whole; from the media upload, the file counts and the storage prefix they
/// went under.
///
/// Only counts, a domain and a storage prefix are kept. Command output is not a
/// place for a tenant's secrets to accumulate, and the way to guarantee that is
/// to decide here what a report may contain rather than to trust what was
/// printed.
fn imported_instance_report(stdout: &str) -> serde_json::Value {
    let mut report = serde_json::Map::new();
    for line in stdout
        .lines()
        .skip_while(|line| line.trim() != "OK")
        .skip(1)
    {
        let Some((key, value)) = line.split_once(": ") else {
            continue;
        };
        let (key, value) = (key.trim(), value.trim());
        if key.is_empty() || key.len() > 40 {
            continue;
        }
        let value = match value.parse::<i64>() {
            Ok(count) => serde_json::Value::from(count),
            Err(_) if is_valid_domain(value) || is_safe_storage_prefix(value) => {
                serde_json::Value::from(value)
            }
            Err(_) => continue,
        };
        report.insert(key.to_owned(), value);
    }
    serde_json::Value::Object(report)
}

/// Why the importer refused, said as an operator can act on it.
///
/// Its own refusals name domains, row counts and schema versions. A connection
/// string can still reach its output through a driver's error, so anything
/// URL-shaped is redacted before this becomes a step's error and an operation's.
fn import_failure(stderr: &str) -> String {
    let message = stderr
        .split_whitespace()
        .map(|word| match word.contains("://") {
            true => redact_connection_url(word),
            false => word.to_owned(),
        })
        .collect::<Vec<_>>()
        .join(" ");
    let message = message.trim_start_matches("Error: ").trim();
    match message.is_empty() {
        true => "the importer refused the dump and said nothing".to_owned(),
        false => message.chars().take(1000).collect(),
    }
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
    let rendered = toml::to_string_pretty(&typed_toml(
        tenant,
        config,
        bucket,
        bind_address,
        state.tenant_pooler(),
    )?)?;
    toml::from_str::<TenantToml>(&rendered)?;
    let directory = PathBuf::from(directory);
    anyhow::ensure!(
        tokio::fs::metadata(&directory).await?.is_dir(),
        "tenant directory unavailable"
    );
    let file_name = tenant_file_name(tenant_id);
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

/// The lock that serialises reloads of one runtime, shared by provisioning and
/// by an operator's render.
async fn runtime_reload_lock(state: &Shared, runtime_id: Uuid) -> Arc<tokio::sync::Mutex<()>> {
    let mut locks = state.runtime_reloads.lock().await;
    locks.entry(runtime_id).or_default().clone()
}

/// Ask the runtime process behind an already-resolved launchd label to re-read
/// its tenants directory. It applies what changed in place, so the tenants it
/// already serves keep serving.
async fn signal_runtime_reload(service_label: &str) -> Result<()> {
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

/// Whether the runtime is dispatching a hostname at all. A runtime that
/// rejects its tenants directory goes on serving what it had and answers 421
/// for everything else, so a delivered signal is not yet a reload. Any other
/// answer means the hostname is dispatched; whether it is well is the health
/// step's business.
async fn runtime_dispatches_host(state: &Shared, bind_address: &str, domain: &str) -> bool {
    let url = format!("http://{bind_address}/api/v2/instance");
    for _ in 0..15 {
        if let Ok(response) = state.client.get(&url).header("Host", domain).send().await
            && response.status().as_u16() != 421
        {
            return true;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    false
}

async fn reload_provisioned_runtime(state: &Shared, tenant_id: Uuid) -> Result<()> {
    reload_runtime(state, tenant_id, true).await
}

/// Make the runtime re-read its tenants directory, after `tenant_id`'s file
/// was written or removed. With `serves`, wait until the runtime serves the
/// tenant, which is how a rejected reload shows; a tenant taken off the
/// runtime is not served afterwards by design, and whether it is gone is the
/// `offline` step's to check.
async fn reload_runtime(state: &Shared, tenant_id: Uuid, serves: bool) -> Result<()> {
    let (runtime_id, service_label, router_enabled, tenant_directory, bind_address, domain) = sqlx::query_as::<_, (Uuid, String, bool, String, String, String)>(
        "SELECT r.id,r.service_label,r.router_enabled,r.tenant_directory,r.bind_address,t.domain FROM tenants t JOIN runtimes r ON r.id=t.runtime_id WHERE t.id=$1",
    )
    .bind(tenant_id)
    .fetch_one(&state.db)
    .await?;
    let service_label =
        active_runtime_label(&service_label, router_enabled, &tenant_directory).await?;
    let lock = runtime_reload_lock(state, runtime_id).await;
    let _guard = lock.lock().await;
    signal_runtime_reload(&service_label).await?;
    if serves {
        anyhow::ensure!(
            runtime_dispatches_host(state, &bind_address, &domain).await,
            "the runtime did not adopt this instance; its log says why it rejected the reload"
        );
    }
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
    healthcheck_url(state, &bind_address, &domain, &domain).await
}

/// Check that `host`, which may be an alias, reaches this tenant. An alias is
/// answered by the tenant under its primary domain, which is what it reports.
async fn healthcheck_host(state: &Shared, tenant_id: Uuid, host: &str) -> Result<()> {
    let (domain, bind_address) = sqlx::query_as::<_, (String, String)>(
        "SELECT t.domain,r.bind_address FROM tenants t JOIN runtimes r ON r.id=t.runtime_id WHERE t.id=$1",
    )
    .bind(tenant_id)
    .fetch_one(&state.db)
    .await?;
    healthcheck_url(state, &bind_address, host, &domain).await
}

async fn healthcheck_url(
    state: &Shared,
    bind_address: &str,
    host: &str,
    domain: &str,
) -> Result<()> {
    let url = format!("http://{bind_address}/api/v2/instance");
    // A misdirect is a different fault from an unwell tenant: the runtime is
    // answering, it just has no configuration for this hostname.
    let mut misdirected = false;
    for _ in 0..30 {
        if let Ok(response) = state.client.get(&url).header("Host", host).send().await {
            if response.status().as_u16() == 421 {
                misdirected = true;
            } else if response.status().is_success()
                && response
                    .json::<serde_json::Value>()
                    .await
                    .ok()
                    .and_then(|body| body.get("domain")?.as_str().map(str::to_owned))
                    .is_some_and(|served_domain| served_domain.eq_ignore_ascii_case(domain))
            {
                return Ok(());
            }
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    anyhow::ensure!(
        !misdirected,
        "the runtime is not serving this hostname, so it never adopted the instance's configuration"
    );
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
    config.pooled_database_url = config
        .pooled_database_url
        .as_deref()
        .map(redact_connection_url);
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
    let mut typed = typed_toml(tenant, config, bucket_name, bind_address, s.tenant_pooler())
        .map_err(ApiError::from)?;
    redact_tenant_toml(&mut typed);
    let rendered = toml::to_string_pretty(&typed).map_err(ApiError::from)?;
    Ok(Json(TenantConfiguration {
        file_name: tenant_file_name(id),
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
    Ok(Json(sqlx::query_as("SELECT r.id,r.instance_id,r.name,i.name instance_name,i.address instance_address,r.bind_address,r.service_label,r.tenant_directory,r.origin_hostname,r.router_enabled,r.blue_bind_address,r.green_bind_address,(SELECT count(*) FROM tenants WHERE runtime_id=r.id) tenant_count,r.auto_deploy_enabled,r.deployed_ref,o.id release_operation_id,o.status release_status,e.message release_message FROM runtimes r JOIN instances i ON i.id=r.instance_id LEFT JOIN LATERAL (SELECT id,status FROM operations WHERE runtime_id=r.id AND kind='runtime_release' ORDER BY created_at DESC LIMIT 1) o ON true LEFT JOIN LATERAL (SELECT string_agg(message,E'\\n' ORDER BY id) message FROM operation_events WHERE operation_id=o.id) e ON true ORDER BY i.name,r.name").fetch_all(&s.db).await?))
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
    let mut tx = state.db.begin().await?;
    // Serialize deployment admission with the maintenance lease transition.
    sqlx::query("SELECT pg_advisory_xact_lock(60001001)")
        .execute(&mut *tx)
        .await?;
    if sqlx::query_scalar::<_, bool>("SELECT COALESCE(deploy_until > now(),false) FROM control_plane_maintenance WHERE singleton").fetch_one(&mut *tx).await? {
        return Err(ApiError(StatusCode::SERVICE_UNAVAILABLE, "Control plane deployment is in progress. Try again shortly.".into()));
    }
    let operation_id = Uuid::new_v4();
    let inserted: Option<Uuid> = sqlx::query_scalar("INSERT INTO operations(id,runtime_id,kind,status,requested_by,detail,started_at) VALUES($1,$2,'runtime_release','running',$3,jsonb_build_object('trigger',$4),now()) ON CONFLICT DO NOTHING RETURNING id")
        .bind(operation_id).bind(runtime_id).bind(requested_by).bind(trigger).fetch_optional(&mut *tx).await?;
    let Some(operation_id) = inserted else {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "Another runtime release is already in progress.".into(),
        ));
    };
    let deployment_id = Uuid::new_v4();
    sqlx::query("INSERT INTO runtime_deployments(id,runtime_id,operation_id,trigger,status,started_at) VALUES($1,$2,$3,$4,'queued',now())")
        .bind(deployment_id).bind(runtime_id).bind(operation_id).bind(trigger).execute(&mut *tx).await?;
    tx.commit().await?;
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
    let root = match std::env::var("CONTROL_PROJECT_ROOT") {
        Ok(root) => PathBuf::from(root),
        Err(_) => exe
            .parent()
            .and_then(FsPath::parent)
            .and_then(FsPath::parent)
            .ok_or_else(|| "Control-plane release directory is unavailable.".to_string())?
            .to_path_buf(),
    };
    let source = root
        .parent()
        .ok_or_else(|| "Central Eunha source directory is unavailable.".to_string())?
        .join("eunha");
    if !source.join("Cargo.toml").is_file() {
        return Err("Central Eunha source checkout is unavailable.".into());
    }
    Ok((root, source))
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
        state.health.tick("release-recovery");
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
    let served = sqlx::query_scalar::<_, bool>(
        "SELECT lifecycle_state NOT IN ('deprovisioning','retained','purging','purged') FROM tenants WHERE id=$1",
    )
    .bind(id)
    .fetch_one(&s.db)
    .await?;
    if !served {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "A deprovisioned tenant is not served, so there is nothing to check.".into(),
        ));
    }
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
        let _ = sqlx::query(
            "DELETE FROM auth_rate_limits WHERE window_started_at < now()-interval '1 day'",
        )
        .execute(&state.db)
        .await;
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
                    // The cheap endpoint: one trivial query, where
                    // `/api/v2/instance` counts distinct domains and posting
                    // accounts and takes a pool connection per statistic. This
                    // runs against every tenant forever, so it has to be the
                    // cheapest thing that still proves the database answers.
                    .get(format!("http://{bind_address}/api/eunha/v1/health"))
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
        // A tenant's pool releases a connection once it has been unused for
        // `database_pool.idle_timeout_seconds`, so a sweep faster than that
        // keeps every tenant holding connections it is not using. Keep this
        // above that timeout, and `TENANT_HEALTH_STALE_AFTER` above this.
        tokio::time::sleep(HEALTH_SWEEP_INTERVAL).await;
    }
}

/// How long between health sweeps over every running tenant.
const HEALTH_SWEEP_INTERVAL: Duration = Duration::from_secs(300);

/// How many consecutive failed reconciliations retire a tenant from the loop.
/// A render that cannot succeed — an invalid stored domain, a runtime
/// directory that is gone — would otherwise be retried on every pass forever,
/// and the fault needs an operator either way.
const RECONCILE_GIVE_UP_AFTER: i32 = 5;

/// The runtimes this control plane is allowed to reconcile: the ones on the
/// host it was configured to manage. A runtime elsewhere has its own control
/// plane, and `render_provisioned_tenant` refuses it anyway.
pub(crate) async fn local_runtimes(state: &Shared) -> Result<Vec<Uuid>> {
    let configured_host = std::env::var("CONTROL_RUNTIME_HOST").unwrap_or_default();
    if configured_host.is_empty() {
        return Ok(Vec::new());
    }
    Ok(sqlx::query_scalar::<_, Uuid>(
        "SELECT r.id FROM runtimes r JOIN instances i ON i.id=r.instance_id WHERE i.address=$1 ORDER BY r.name",
    )
    .bind(configured_host)
    .fetch_all(&state.db)
    .await?)
}

/// One reconciliation pass over a runtime: render every tenant whose stored
/// configuration is newer than the file on disk, reload the runtime once for
/// all of them, and check that each one came back.
///
/// Rendering is per tenant; reloading is not. A reload makes the runtime
/// re-read its whole tenants directory and apply what changed, so a fleet that
/// drifted together costs one signal rather than one per tenant. Migration 037
/// changed every tenant's pool size at once, which is the case this has to
/// survive.
///
/// Returns how many tenants were brought back to their stored configuration.
pub(crate) async fn reconcile_runtime_drift(state: &Shared, runtime_id: Uuid) -> Result<usize> {
    let configured_host = std::env::var("CONTROL_RUNTIME_HOST").unwrap_or_default();
    if configured_host.is_empty() {
        return Ok(0);
    }
    // Only a tenant that has finished provisioning and is meant to be running.
    // One still provisioning has a workflow of its own rendering and reloading
    // it, and a retained or purging one was taken off disk deliberately.
    let drifted = sqlx::query_as::<_, (Uuid, i64)>(
        "SELECT t.id,t.config_revision FROM tenants t JOIN runtimes r ON r.id=t.runtime_id JOIN instances i ON i.id=r.instance_id WHERE t.runtime_id=$1 AND i.address=$2 AND t.lifecycle_state='ready' AND t.desired_state='running' AND t.reconcile_failures<$3 AND (t.rendered_revision IS NULL OR t.rendered_revision<t.config_revision) ORDER BY t.created_at",
    )
    .bind(runtime_id)
    .bind(&configured_host)
    .bind(RECONCILE_GIVE_UP_AFTER)
    .fetch_all(&state.db)
    .await?;
    if drifted.is_empty() {
        return Ok(0);
    }

    // A tenant that cannot be rendered must not hold back the reload for the
    // others on the runtime, so a failure is recorded and the pass carries on
    // with whatever did render.
    let mut rendered = Vec::new();
    for (tenant_id, revision) in drifted {
        let operation = start_reconcile_operation(&state.db, tenant_id).await;
        match render_provisioned_tenant(state, tenant_id).await {
            Ok(()) => rendered.push((tenant_id, revision, operation)),
            Err(_) => {
                record_reconcile_failure(
                    &state.db,
                    tenant_id,
                    operation,
                    "The tenant configuration could not be rendered.",
                )
                .await
            }
        }
    }
    let Some(&(first, _, _)) = rendered.first() else {
        return Ok(0);
    };

    if reload_provisioned_runtime(state, first).await.is_err() {
        let message = "Configuration rendered, but the runtime could not be reloaded.";
        for (tenant_id, _, operation) in rendered {
            // The file is ahead of what the runtime is serving, and a render
            // that recorded its revision leaves no drift for the next pass to
            // find. Give the revision up so this tenant is tried again.
            let _ = sqlx::query(
                "UPDATE tenants SET rendered_revision=NULL,updated_at=now() WHERE id=$1",
            )
            .bind(tenant_id)
            .execute(&state.db)
            .await;
            record_reconcile_failure(&state.db, tenant_id, operation, &message).await;
        }
        return Ok(0);
    }

    // The reload succeeded, so every file on disk is what the runtime read. A
    // tenant that is unwell past that point has a fault a re-render will not
    // fix, and the health sweep owns saying so; the revision stays recorded.
    let mut converged = 0;
    for (tenant_id, revision, operation) in rendered {
        match healthcheck_provisioned_tenant(state, tenant_id).await {
            Ok(()) => {
                finish_reconcile_operation(&state.db, tenant_id, operation, revision).await;
                converged += 1;
            }
            Err(_) => {
                record_reconcile_failure(
                    &state.db,
                    tenant_id,
                    operation,
                    "Configuration rendered and reloaded, but the tenant did not become healthy.",
                )
                .await
            }
        }
    }
    Ok(converged)
}

async fn start_reconcile_operation(db: &PgPool, tenant_id: Uuid) -> Uuid {
    let id = Uuid::new_v4();
    // No `requested_by`: nobody asked for this one, which is what tells it
    // apart from an operator's render in the operations log.
    let _ = sqlx::query("INSERT INTO operations(id,tenant_id,kind,status,started_at) VALUES($1,$2,'reconcile','running',now())")
        .bind(id)
        .bind(tenant_id)
        .execute(db)
        .await;
    id
}

async fn record_reconcile_failure(db: &PgPool, tenant_id: Uuid, operation: Uuid, message: &str) {
    mark_operation_failed(db, operation, message).await;
    let _ = sqlx::query("UPDATE tenants SET reconcile_failures=reconcile_failures+1,reconcile_error=$1,updated_at=now() WHERE id=$2")
        .bind(message)
        .bind(tenant_id)
        .execute(db)
        .await;
}

async fn finish_reconcile_operation(db: &PgPool, tenant_id: Uuid, operation: Uuid, revision: i64) {
    let _ = sqlx::query("UPDATE operations SET status='succeeded',detail=jsonb_build_object('revision',$1),finished_at=now() WHERE id=$2")
        .bind(revision)
        .bind(operation)
        .execute(db)
        .await;
    let _ = sqlx::query("UPDATE tenants SET reconcile_failures=0,reconcile_error=NULL WHERE id=$1 AND reconcile_failures>0")
        .bind(tenant_id)
        .execute(db)
        .await;
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

/// A media storage namespace, as `tenant_configuration` constrains it: the
/// tenant's own `t/<tenant id>`, or empty for a bucket it does not share.
fn is_safe_storage_prefix(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('/')
        && !value.ends_with('/')
        && value.split('/').all(|segment| {
            !segment.is_empty()
                && segment
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        })
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

/// A tenant's runtime configuration file, named by its id. Eunha loads every
/// `*.toml` in the directory and identifies a tenant by the domain inside, so
/// the name only has to be unique and stable, which a domain-derived one is
/// not: hyphens are legal in domains, and domains change.
fn tenant_file_name(tenant_id: Uuid) -> String {
    format!("{tenant_id}.toml")
}

/// The name a tenant's file had before files were named by tenant id.
fn legacy_tenant_file_name(domain: &str) -> String {
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
    let row = sqlx::query_as::<_, (SqlJson<StoredTenant>, SqlJson<StoredConfig>, String, String, String, String, String, bool, i64, Uuid)>(
        "SELECT to_jsonb(t),to_jsonb(c),b.bucket_name,i.address,r.tenant_directory,r.service_label,r.bind_address,r.router_enabled,t.config_revision,r.id FROM tenants t JOIN tenant_configuration c ON c.tenant_id=t.id JOIN runtimes r ON r.id=t.runtime_id JOIN instances i ON i.id=r.instance_id JOIN r2_buckets b ON b.tenant_id=t.id WHERE t.id=$1",
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
        runtime_id,
    ) = row;
    if matches!(
        tenant.lifecycle_state.as_str(),
        "deprovisioning" | "retained" | "purging" | "purged"
    ) {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "A deprovisioned tenant is not rendered; restore it first.".into(),
        ));
    }
    // Before the first provisioning step there is no tenant database to point
    // configuration at, and a runtime handed such a tenant loads a hostname it
    // cannot serve. For a tenant bringing its own domain that step waits on
    // the domain, which is what makes this read as a domain check. A tenant
    // already serving is past all of this, steps or no steps: the ones that
    // predate provisioning have none to show.
    if !sqlx::query_scalar::<_, bool>("SELECT t.lifecycle_state='ready' OR EXISTS(SELECT 1 FROM provisioning_steps p WHERE p.tenant_id=t.id AND p.step='database' AND p.status='succeeded') FROM tenants t WHERE t.id=$1")
        .bind(id)
        .fetch_one(&s.db)
        .await?
    {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "This instance has not finished its first setup step yet; wait for its domain to connect.".into(),
        ));
    }
    let domain = tenant.domain.clone();
    let tenant_file_name = tenant_file_name(id);
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
    let typed = typed_toml(
        tenant,
        config,
        bucket_name,
        bind_address.clone(),
        s.tenant_pooler(),
    )
    .map_err(ApiError::from)?;
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
    // A reload applies this tenant's change in place, leaving every other
    // tenant on the runtime serving; the lock keeps a second render from
    // signalling while this one is still settling.
    let lock = runtime_reload_lock(&s, runtime_id).await;
    let _guard = lock.lock().await;
    if signal_runtime_reload(&service_label).await.is_err() {
        mark_operation_failed(
            &s.db,
            op,
            "Configuration rendered, but the runtime could not be reloaded.",
        )
        .await;
        return Err(ApiError(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Configuration rendered, but the runtime could not be reloaded.".into(),
        ));
    }
    if !runtime_dispatches_host(&s, &bind_address, &domain).await {
        sqlx::query("UPDATE tenants SET observed_state='unhealthy',observed_at=now(),observation_error='The runtime did not adopt this configuration.',updated_at=now() WHERE id=$1")
            .bind(id).execute(&s.db).await?;
        mark_operation_failed(
            &s.db,
            op,
            "Configuration rendered, but the runtime did not adopt it; its log says why it rejected the reload.",
        )
        .await;
        return Err(ApiError(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Configuration rendered, but the runtime did not adopt it.".into(),
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
            "Configuration rendered and reloaded, but the tenant did not become healthy.",
        )
        .await;
        return Err(ApiError(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Configuration rendered, but the tenant did not become healthy after the reload."
                .into(),
        ));
    }
    let checksum = format!("{:x}", Sha256::digest(rendered_config.as_bytes()));
    // An operator's render is also how a tenant the reconciler retired after
    // repeated failures rejoins the loop.
    sqlx::query("UPDATE tenants SET rendered_revision=$1,rendered_checksum=$2,observed_state='healthy',observed_at=now(),observation_error=NULL,reconcile_failures=0,reconcile_error=NULL,updated_at=now() WHERE id=$3")
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
        DomainDnsTargetError, REDACTED, connection_budget_error, domain_reconcile_error_message,
        is_valid_owner_username, printed_password, redact_connection_url, tenant_redis_key_prefix,
    };
    use uuid::Uuid;

    #[test]
    fn connection_budget_allows_what_postgres_accepts() {
        assert_eq!(connection_budget_error(80, 97), None);
        // The edge is inclusive: a runtime may declare exactly what is left.
        assert_eq!(connection_budget_error(97, 97), None);
        assert_eq!(connection_budget_error(0, 0), None);
    }

    #[test]
    fn connection_budget_names_both_numbers_when_full() {
        let message = connection_budget_error(100, 97).expect("100 does not fit in 97");
        assert!(message.contains("100"), "{message}");
        assert!(message.contains("97"), "{message}");
    }

    #[test]
    fn temporal_links_stay_inside_the_configured_ui() {
        let ui = |base: &str| super::TemporalUiConfig {
            base: url::Url::parse(base).unwrap(),
            namespace: "eunha-space".into(),
        };
        assert_eq!(
            ui("http://127.0.0.1:8080/").workflow_url("tenant-provision-v3-abc"),
            Some(
                "http://127.0.0.1:8080/namespaces/eunha-space/workflows/tenant-provision-v3-abc"
                    .into()
            )
        );
        // A UI mounted under a path keeps it, with or without a trailing slash.
        assert_eq!(
            ui("https://ops.example.com/temporal").workflow_url("w"),
            Some("https://ops.example.com/temporal/namespaces/eunha-space/workflows/w".into())
        );
        // The workflow ID is one path segment, so a stored ID can never walk
        // out of the namespace or smuggle a query string into the link.
        assert_eq!(
            ui("http://127.0.0.1:8080").workflow_url("../../evil?x=1"),
            Some(
                "http://127.0.0.1:8080/namespaces/eunha-space/workflows/..%2F..%2Fevil%3Fx=1"
                    .into()
            )
        );
    }

    #[test]
    fn tenant_files_are_named_by_tenant_id() {
        let tenant = Uuid::parse_str("44444444-4444-4444-4444-444444444444").unwrap();
        assert_eq!(
            super::tenant_file_name(tenant),
            "44444444-4444-4444-4444-444444444444.toml"
        );
        assert!(super::is_safe_tenant_file_name(&super::tenant_file_name(
            tenant
        )));
        // The collision that naming by domain allowed.
        assert_eq!(
            super::legacy_tenant_file_name("foo-bar.example.com"),
            super::legacy_tenant_file_name("foo.bar-example.com")
        );
    }

    #[test]
    fn dns_check_looks_for_the_custom_hostname_target() {
        let target = "customers.eunha.space";
        let pointing = serde_json::json!({"Status": 0, "Answer": [
            {"name": "social.example.com.", "type": 5, "data": "Customers.Eunha.Space."}
        ]});
        let elsewhere = serde_json::json!({"Status": 0, "Answer": [
            {"name": "social.example.com.", "type": 5, "data": "example.github.io."}
        ]});
        let missing = serde_json::json!({"Status": 3});
        assert!(super::cname_points_to(&pointing, target));
        assert!(!super::cname_points_to(&elsewhere, target));
        assert!(!super::cname_points_to(&missing, target));
    }

    #[test]
    fn the_pooled_url_keeps_everything_but_where_it_dials() {
        let direct = "postgres://eunha_r_abc:s3cret@localhost:5432/eunha_t_abc";
        let pooled = super::pooled_database_url(direct, "127.0.0.1:6432").unwrap();
        assert_eq!(
            pooled,
            "postgres://eunha_r_abc:s3cret@127.0.0.1:6432/eunha_t_abc"
        );
        // It carries the same password, so operator-facing output has to hide
        // it exactly as the direct URL's is hidden.
        let shown = super::redact_connection_url(&pooled);
        assert!(!shown.contains("s3cret"), "{shown}");
    }

    #[test]
    fn tenant_url_points_at_the_pooler_while_names_stay_intact() {
        let tenant = Uuid::parse_str("66666666-6666-6666-6666-666666666666").unwrap();
        let compact = tenant.simple().to_string();
        let direct = super::tenant_database_url(
            "postgres://admin:pw@db.internal:5432/postgres",
            tenant,
            "pw",
        )
        .unwrap();
        let pooled = super::pooled_database_url(&direct, "127.0.0.1:6432").unwrap();
        assert!(pooled.contains("@127.0.0.1:6432/"), "{pooled}");
        // Purge still recognises its own role and database through the pooler.
        assert_eq!(
            super::provisioned_database_names(tenant, &pooled),
            Some((format!("eunha_r_{compact}"), format!("eunha_t_{compact}")))
        );
    }

    /// An import's evidence comes from a command's output, which is the kind of
    /// place a connection string ends up by accident. Deciding here what a
    /// report may hold is what keeps that from being stored.
    #[test]
    fn an_import_report_keeps_counts_a_domain_and_a_prefix_and_nothing_else() {
        let report = super::imported_instance_report(
            "some progress nobody asked for\n\
             OK\n\
             domain: seoul.earth\n\
             users: 12\n\
             local accounts: 12\n\
             known accounts: 340219\n\
             statuses: 88214\n\
             key prefix: t/44444444-4444-4444-4444-444444444444\n\
             database_url: postgres://eunha_r_x:hunter2@db.internal/eunha_t_x\n\
             note: something chatty\n",
        );
        assert_eq!(report["domain"], "seoul.earth");
        assert_eq!(report["users"], 12);
        assert_eq!(report["known accounts"], 340219);
        assert_eq!(
            report["key prefix"],
            "t/44444444-4444-4444-4444-444444444444"
        );
        // Neither the URL nor the free-form line survives the allowlist.
        assert!(report.get("database_url").is_none());
        assert!(report.get("note").is_none());
        // Anything printed before the importer said OK is progress, not result.
        assert!(report.as_object().is_some_and(|fields| fields.len() == 6));
    }

    #[test]
    fn a_report_from_a_command_that_never_succeeded_is_empty() {
        assert_eq!(
            super::imported_instance_report("connecting...\nit went wrong\n"),
            serde_json::json!({})
        );
    }

    #[test]
    fn an_import_failure_keeps_its_reason_and_loses_its_credentials() {
        let failure = super::import_failure(
            "Error: could not connect to postgres://eunha_r_x:hunter2@db.internal/eunha_t_x\n",
        );
        assert!(failure.starts_with("could not connect to postgres://eunha_r_x:"));
        assert!(!failure.contains("hunter2"));
        assert_eq!(
            super::import_failure("the dump holds other.example, not seoul.earth"),
            "the dump holds other.example, not seoul.earth"
        );
        assert_eq!(
            super::import_failure("   \n"),
            "the importer refused the dump and said nothing"
        );
    }

    #[test]
    fn a_storage_prefix_is_a_tenants_own_namespace_or_nothing() {
        assert!(super::is_safe_storage_prefix(
            "t/44444444-4444-4444-4444-444444444444"
        ));
        assert!(super::is_safe_storage_prefix("media"));
        // The shapes `tenant_configuration` refuses, which are also the shapes
        // that would silently address a different object.
        assert!(!super::is_safe_storage_prefix(""));
        assert!(!super::is_safe_storage_prefix("/t/one"));
        assert!(!super::is_safe_storage_prefix("t/one/"));
        assert!(!super::is_safe_storage_prefix("t//one"));
        assert!(!super::is_safe_storage_prefix("t/../one"));
    }

    #[test]
    fn purge_drops_only_the_database_provisioning_created() {
        let tenant = Uuid::parse_str("44444444-4444-4444-4444-444444444444").unwrap();
        let other = Uuid::parse_str("55555555-5555-5555-5555-555555555555").unwrap();
        let provisioned = super::tenant_database_url(
            "postgres://admin:pw@db.internal:5432/postgres",
            tenant,
            "pw",
        )
        .unwrap();
        eprintln!("{provisioned}");
        let compact = tenant.simple().to_string();
        assert_eq!(
            super::provisioned_database_names(tenant, &provisioned),
            Some((format!("eunha_r_{compact}"), format!("eunha_t_{compact}")))
        );
        assert_eq!(super::provisioned_database_names(other, &provisioned), None);
        assert_eq!(
            super::provisioned_database_names(tenant, "postgres://legacy:pw@localhost/eunha"),
            None
        );
        assert_eq!(super::provisioned_database_names(tenant, "not a url"), None);
    }

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
