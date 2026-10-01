//! Run explicitly with `mise run test-integration` against an isolated PostgreSQL cluster.
use super::*;

#[sqlx::test(migrations = "./migrations")]
#[ignore = "requires disposable PostgreSQL; use mise run test-integration"]
async fn auth_budgets_are_atomic_and_reset(pool: PgPool) {
    let attempts =
        futures::future::join_all((0..20).map(|_| auth::limit(&pool, "test", "identity", 5))).await;
    assert_eq!(attempts.iter().filter(|result| result.is_ok()).count(), 5);
    assert!(
        attempts
            .iter()
            .filter_map(|result| result.as_ref().err())
            .all(|error| error.0 == StatusCode::TOO_MANY_REQUESTS)
    );
    assert!(
        auth::limit(&pool, "test", "other-identity", 5)
            .await
            .is_ok()
    );
    sqlx::query("UPDATE auth_rate_limits SET window_started_at=now()-interval '16 minutes'")
        .execute(&pool)
        .await
        .unwrap();
    assert!(auth::limit(&pool, "test", "identity", 5).await.is_ok());
    assert_eq!(
        sqlx::query_scalar::<_, i32>("SELECT attempts FROM auth_rate_limits WHERE key_hash=$1")
            .bind(token_hash("test:identity"))
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );
}

#[sqlx::test(migrations = "./migrations")]
#[ignore = "requires disposable PostgreSQL; use mise run test-integration"]
async fn failed_transactions_leave_no_partial_operation(pool: PgPool) {
    let mut tx = pool.begin().await.unwrap();
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO operations(id,kind,status) VALUES($1,'test','pending')")
        .bind(id)
        .execute(&mut *tx)
        .await
        .unwrap();
    // A disconnect before commit must not make a partial operation visible.
    drop(tx);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM operations WHERE id=$1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
}

fn state(pool: PgPool) -> Shared {
    Arc::new(AppState {
        db: pool,
        health: Arc::new(health::Health::default()),
        usage: Arc::new(tokio::sync::Mutex::new(usage::Cache::default())),
        client: reqwest::Client::new(),
        provision_database_url: String::new(),
        tenant_pooler_address: None,
        tenant_pooler_client_slots: None,
        cloudflare: None,
        provided_domain_suffixes: vec![],
        customer_email: None,
        temporal_ui: None,
        runtime_reloads: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
    })
}

async fn fixture(pool: &PgPool) -> (Uuid, HeaderMap) {
    let instance = Uuid::new_v4();
    let runtime = Uuid::new_v4();
    let operator_id = Uuid::new_v4();
    sqlx::query("INSERT INTO instances(id,name,address) VALUES($1,'test','127.0.0.1')")
        .bind(instance)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO runtimes(id,name,instance_id,tenant_directory,service_label,binary_path,bind_address,blue_bind_address,green_bind_address,origin_hostname) VALUES($1,'test',$2,'/tmp/test','test.runtime','/tmp/test/eunha','127.0.0.1:60000','127.0.0.1:61000','127.0.0.1:61001','test.origin.example')").bind(runtime).bind(instance).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO operators(id,username,password_hash) VALUES($1,'test','unused')")
        .bind(operator_id)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO sessions(token_hash,operator_id,expires_at) VALUES($1,$2,now()+interval '1 hour')").bind(token_hash("test-session")).bind(operator_id).execute(pool).await.unwrap();
    let mut headers = HeaderMap::new();
    headers.insert(
        header::COOKIE,
        "eunha_space_session=test-session".parse().unwrap(),
    );
    (runtime, headers)
}

async fn tenant(pool: &PgPool, runtime: Uuid) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO tenants(id,domain,title,runtime_id,desired_state,observed_state) VALUES($1,$2,'Test',$3,'running','unknown')").bind(id).bind(format!("{}.example", id.simple())).bind(runtime).execute(pool).await.unwrap();
    id
}

#[sqlx::test(migrations = "./migrations")]
#[ignore = "requires disposable PostgreSQL; use mise run test-integration"]
async fn deletion_retains_data_and_restore_cancels_purge(pool: PgPool) {
    let (runtime, _) = fixture(&pool).await;
    let id = tenant(&pool, runtime).await;
    let mut tx = pool.begin().await.unwrap();
    begin_deprovisioning(&mut tx, id, None, None).await.unwrap();
    tx.commit().await.unwrap();
    let (lifecycle, seconds) = sqlx::query_as::<_, (String, i64)>("SELECT lifecycle_state,extract(epoch FROM purge_scheduled_for-now())::bigint FROM tenants WHERE id=$1").bind(id).fetch_one(&pool).await.unwrap();
    assert_eq!(lifecycle, "deprovisioning");
    assert!((604_790..=604_800).contains(&seconds));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM purge_steps WHERE tenant_id=$1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
    sqlx::query("UPDATE tenants SET lifecycle_state='retained' WHERE id=$1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    let state = state(pool.clone());
    let mut tx = pool.begin().await.unwrap();
    queue_restore(&state, &mut tx, id, None, None)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        sqlx::query_as::<_, (String, bool)>(
            "SELECT lifecycle_state,purge_scheduled_for IS NULL FROM tenants WHERE id=$1"
        )
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap(),
        ("provisioning".into(), true)
    );
    temporal::schedule_due_purge(&state, id).await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM purge_steps WHERE tenant_id=$1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
}

#[sqlx::test(migrations = "./migrations")]
#[ignore = "requires disposable PostgreSQL; use mise run test-integration"]
async fn restore_and_due_purge_serialize(pool: PgPool) {
    let (runtime, headers) = fixture(&pool).await;
    let state = state(pool.clone());
    for _ in 0..16 {
        let id = tenant(&pool, runtime).await;
        sqlx::query("UPDATE tenants SET lifecycle_state='retained',purge_scheduled_for=now()-interval '1 minute' WHERE id=$1").bind(id).execute(&pool).await.unwrap();
        let (restored, purged) = tokio::join!(
            restore_tenant(State(state.clone()), headers.clone(), Path(id)),
            temporal::schedule_due_purge(&state, id)
        );
        purged.unwrap();
        let lifecycle =
            sqlx::query_scalar::<_, String>("SELECT lifecycle_state FROM tenants WHERE id=$1")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap();
        match lifecycle.as_str() {
            "provisioning" => {
                assert!(restored.is_ok());
                assert_eq!(
                    sqlx::query_scalar::<_, i64>(
                        "SELECT count(*) FROM purge_steps WHERE tenant_id=$1"
                    )
                    .bind(id)
                    .fetch_one(&pool)
                    .await
                    .unwrap(),
                    0
                );
            }
            "purging" => assert_eq!(restored.unwrap_err().0, StatusCode::CONFLICT),
            other => panic!("unexpected lifecycle: {other}"),
        }
    }
}

#[sqlx::test(migrations = "./migrations")]
#[ignore = "requires disposable PostgreSQL; use mise run test-integration"]
async fn usage_history_is_persisted_bounded_and_authenticated(pool: PgPool) {
    let (runtime, headers) = fixture(&pool).await;
    let id = tenant(&pool, runtime).await;
    let now = (OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000) as i64;
    sqlx::query("INSERT INTO tenant_usage_samples(tenant_id,sampled_at,database_bytes,transactions) VALUES($1,$2,1024,10),($1,$3,NULL,NULL)").bind(id).bind(now-700_000).bind(now).execute(&pool).await.unwrap();
    let state = state(pool);
    let Json(history) = usage::history(State(state.clone()), headers, Path(id))
        .await
        .unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(
        serde_json::to_value(&history[0]).unwrap()["database_bytes"],
        serde_json::Value::Null
    );
    assert_eq!(
        usage::history(State(state), HeaderMap::new(), Path(id))
            .await
            .unwrap_err()
            .0,
        StatusCode::UNAUTHORIZED
    );
}

#[sqlx::test(migrations = "./migrations")]
#[ignore = "requires disposable PostgreSQL; use mise run test-integration"]
async fn failed_metric_probe_persists_nulls_without_credentials(pool: PgPool) {
    let (runtime, headers) = fixture(&pool).await;
    let id = tenant(&pool, runtime).await;
    sqlx::query("INSERT INTO tenant_configuration(tenant_id,database_url,redis_url,resend_api_key,resend_from,media_storage_region,media_storage_access_key_id,media_storage_secret_access_key,media_storage_base_url) VALUES($1,'postgresql://tenant:private-secret@127.0.0.1:1/missing','redis://127.0.0.1','private-key','test@example','auto','private-id','private-media-secret','https://media.example')")
        .bind(id).execute(&pool).await.unwrap();
    let state = state(pool.clone());
    usage::collect(&state).await.unwrap();
    let Json(rows) = usage::usage(State(state.clone()), headers.clone())
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    let serialized = serde_json::to_string(&rows).unwrap();
    assert!(serialized.contains("\"database_bytes\":null"));
    assert!(!serialized.contains("private"));
    assert!(!serialized.contains("postgresql"));
    let Json(history) = usage::history(State(state), headers, Path(id))
        .await
        .unwrap();
    assert_eq!(history.len(), 1);
}

#[sqlx::test(migrations = "./migrations")]
#[ignore = "requires disposable PostgreSQL; use mise run test-integration"]
async fn deployment_lease_prevents_runtime_release_admission(pool: PgPool) {
    let (runtime, _) = fixture(&pool).await;
    tenant(&pool, runtime).await;
    sqlx::query("UPDATE control_plane_maintenance SET deploy_until=now()+interval '30 minutes' WHERE singleton").execute(&pool).await.unwrap();
    let error = queue_runtime_release(state(pool.clone()), runtime, None, "test")
        .await
        .unwrap_err();
    assert_eq!(error.0, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM operations WHERE kind='runtime_release'"
        )
        .fetch_one(&pool)
        .await
        .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM runtime_deployments")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
}

#[sqlx::test(migrations = "./migrations")]
#[ignore = "requires disposable PostgreSQL; use mise run test-integration"]
async fn readiness_fails_when_database_is_unavailable(pool: PgPool) {
    pool.close().await;
    let response = health::ready(State(state(pool))).await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .unwrap();
    let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(value["database"], false);
    assert_eq!(value["ready"], false);
}

#[sqlx::test(migrations = "./migrations")]
#[ignore = "requires disposable PostgreSQL; use mise run test-integration"]
async fn failed_release_admission_leaves_no_orphan_operation(pool: PgPool) {
    let (runtime, _) = fixture(&pool).await;
    tenant(&pool, runtime).await;
    sqlx::raw_sql("CREATE FUNCTION fail_deployment() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'simulated write failure'; END $$; CREATE TRIGGER fail_deployment BEFORE INSERT ON runtime_deployments FOR EACH ROW EXECUTE FUNCTION fail_deployment();")
        .execute(&pool).await.unwrap();
    let error = queue_runtime_release(state(pool.clone()), runtime, None, "test")
        .await
        .unwrap_err();
    assert_eq!(error.0, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM operations WHERE kind='runtime_release'"
        )
        .fetch_one(&pool)
        .await
        .unwrap(),
        0
    );
}
