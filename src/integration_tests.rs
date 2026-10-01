//! Run explicitly with `mise run test-integration` against an isolated PostgreSQL cluster.
use super::*;

#[sqlx::test(migrations = "./migrations")]
#[ignore = "requires disposable PostgreSQL; use mise run test-integration"]
async fn service_history_is_private_scoped_persisted_and_reset_aware(pool: PgPool) {
    let (runtime, headers) = fixture(&pool).await;
    let id = tenant(&pool, runtime).await;
    let retained = tenant(&pool, runtime).await;
    sqlx::query("UPDATE tenants SET lifecycle_state='ready' WHERE id=$1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE tenants SET lifecycle_state='retained' WHERE id=$1")
        .bind(retained)
        .execute(&pool)
        .await
        .unwrap();
    let now = (OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000) as i64;
    sqlx::query("INSERT INTO tenant_service_samples(tenant_id,sampled_at,epoch,requests,errors,duration_sum,duration_count,pool) VALUES($1,$2,'blue:1',10,1,1,10,2),($1,$3,'blue:1',30,3,5,30,3),($1,$4,'green:2',2,0,0.1,2,2)")
        .bind(id).bind(now-30000).bind(now-20000).bind(now-10000).execute(&pool).await.unwrap();
    let shared = state(pool.clone());
    let Json(history) = service_usage::history(
        State(shared.clone()),
        headers.clone(),
        Query(service_usage::Scope::default()),
    )
    .await
    .unwrap();
    let value = serde_json::to_value(history).unwrap();
    assert_eq!(value["eligible_tenants"], 1);
    assert_eq!(value["samples"][1]["request_rate"], 2.0);
    assert_eq!(value["samples"][1]["latency_ms"], 200.0);
    assert!(value["samples"][2]["request_rate"].is_null());
    assert!(!value.to_string().contains("epoch"));
    let Json(empty) = service_usage::history(
        State(shared.clone()),
        headers.clone(),
        Query(service_usage::Scope {
            tenant_id: Some(retained),
            ..Default::default()
        }),
    )
    .await
    .unwrap();
    assert_eq!(serde_json::to_value(empty).unwrap()["eligible_tenants"], 0);
    let Json(empty) = service_usage::history(
        State(shared.clone()),
        headers.clone(),
        Query(service_usage::Scope {
            instance_id: Some(Uuid::new_v4()),
            ..Default::default()
        }),
    )
    .await
    .unwrap();
    assert!(
        serde_json::to_value(empty).unwrap()["samples"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        service_usage::history(
            State(shared.clone()),
            HeaderMap::new(),
            Query(service_usage::Scope::default())
        )
        .await
        .unwrap_err()
        .0,
        StatusCode::UNAUTHORIZED
    );
    // A runtime with no running slot still persists a null reading, never zero.
    service_usage::collect(&shared).await.unwrap();
    let missing = sqlx::query_as::<_, service_usage::Sample>(
        "SELECT * FROM tenant_service_samples WHERE tenant_id=$1 ORDER BY sampled_at DESC LIMIT 1",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(missing.requests.is_none());
    assert!(missing.pool.is_none());
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM tenant_service_samples WHERE tenant_id=$1"
        )
        .bind(retained)
        .fetch_one(&pool)
        .await
        .unwrap(),
        0
    );
}

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

#[sqlx::test(migrations = "./migrations")]
#[ignore = "requires disposable PostgreSQL; use mise run test-integration"]
async fn provisioning_audit_rejects_secret_error_payloads(pool: PgPool) {
    let (runtime, _) = fixture(&pool).await;
    let id = tenant(&pool, runtime).await;
    sqlx::query("UPDATE tenants SET lifecycle_state='provisioning' WHERE id=$1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO provisioning_steps(tenant_id,step,status) VALUES($1,'database','running')",
    )
    .bind(id)
    .execute(&pool)
    .await
    .unwrap();
    let operation = Uuid::new_v4();
    sqlx::query("INSERT INTO operations(id,tenant_id,kind,status) VALUES($1,$2,'tenant_provision','running')")
        .bind(operation).bind(id).execute(&pool).await.unwrap();
    fail_provisioning(
        &pool,
        id,
        "database",
        Some(
            "postgresql://tenant:private-password@host/database; failing row includes private-key",
        ),
    )
    .await;
    let errors: (String, String, String) = sqlx::query_as("SELECT p.error,t.observation_error,o.error FROM provisioning_steps p JOIN tenants t ON t.id=p.tenant_id JOIN operations o ON o.tenant_id=t.id WHERE t.id=$1")
        .bind(id).fetch_one(&pool).await.unwrap();
    assert_eq!(
        errors,
        (
            "database failed; retry is safe.".into(),
            "database failed; retry is safe.".into(),
            "database failed; retry is safe.".into()
        )
    );
    assert_eq!(
        safe_connection_budget_reason(
            "this server is full: its instances would open 100 database connections and it accepts 97"
        ),
        connection_budget_error(100, 97)
    );
    assert!(safe_connection_budget_reason("this server is full: its instances would open 100 database connections and it accepts 97 private-password").is_none());
}

#[sqlx::test(migrations = "./migrations")]
#[ignore = "requires disposable PostgreSQL; use mise run test-integration"]
async fn interrupted_provisioning_step_resumes_and_completed_step_is_skipped(pool: PgPool) {
    let (runtime, _) = fixture(&pool).await;
    let id = tenant(&pool, runtime).await;
    sqlx::query("INSERT INTO provisioning_steps(tenant_id,step,status,attempts) VALUES($1,'health','running',1)")
        .bind(id).execute(&pool).await.unwrap();
    sqlx::query("UPDATE tenants SET lifecycle_state='provisioning' WHERE id=$1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    let requests = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let request_count = requests.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    sqlx::query("UPDATE runtimes SET bind_address=$1 WHERE id=$2")
        .bind(listener.local_addr().unwrap().to_string())
        .bind(runtime)
        .execute(&pool)
        .await
        .unwrap();
    let domain = format!("{}.example", id.simple());
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new().route(
                "/api/v2/instance",
                get(move || {
                    let domain = domain.clone();
                    let requests = request_count.clone();
                    async move {
                        requests.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        Json(serde_json::json!({"domain":domain}))
                    }
                }),
            ),
        )
        .await
        .unwrap();
    });
    let restarted = state(pool.clone());
    assert!(matches!(
        provision_tenant_step(&restarted, id, "health").await,
        ProvisionStepOutcome::Completed
    ));
    assert!(matches!(
        provision_tenant_step(&state(pool.clone()), id, "health").await,
        ProvisionStepOutcome::Completed
    ));
    assert_eq!(requests.load(std::sync::atomic::Ordering::Relaxed), 1);
    server.abort();
    let attempts: i32 = sqlx::query_scalar(
        "SELECT attempts FROM provisioning_steps WHERE tenant_id=$1 AND step='health'",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(attempts, 2);
}

#[sqlx::test(migrations = "./migrations")]
#[ignore = "requires disposable PostgreSQL; use mise run test-integration"]
async fn fleet_history_groups_hosts_without_double_counting_or_hiding_gaps(pool: PgPool) {
    let (runtime, headers) = fixture(&pool).await;
    let host = sqlx::query_scalar::<_, Uuid>("SELECT instance_id FROM runtimes WHERE id=$1")
        .bind(runtime)
        .fetch_one(&pool)
        .await
        .unwrap();
    let other_host = Uuid::new_v4();
    let other_runtime = Uuid::new_v4();
    sqlx::query("INSERT INTO instances(id,name,address) VALUES($1,'other-host','127.0.0.2')")
        .bind(other_host)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO runtimes(id,name,instance_id,tenant_directory,service_label,binary_path,bind_address,blue_bind_address,green_bind_address,origin_hostname) VALUES($1,'other-runtime',$2,'/tmp/other','other.runtime','/tmp/other/eunha','127.0.0.1:60002','127.0.0.1:61002','127.0.0.1:61003','other.origin.example')")
        .bind(other_runtime).bind(other_host).execute(&pool).await.unwrap();
    let sibling_runtime = Uuid::new_v4();
    sqlx::query("INSERT INTO runtimes(id,name,instance_id,tenant_directory,service_label,binary_path,bind_address,blue_bind_address,green_bind_address,origin_hostname) VALUES($1,'sibling-runtime',$2,'/tmp/sibling','sibling.runtime','/tmp/sibling/eunha','127.0.0.1:60004','127.0.0.1:61004','127.0.0.1:61005','sibling.origin.example')")
        .bind(sibling_runtime).bind(host).execute(&pool).await.unwrap();
    let ids = [
        tenant(&pool, runtime).await,
        tenant(&pool, sibling_runtime).await,
        tenant(&pool, other_runtime).await,
        tenant(&pool, runtime).await,
    ];
    for id in ids {
        sqlx::query("INSERT INTO tenant_configuration(tenant_id,database_url,redis_url,resend_api_key,resend_from,media_storage_region,media_storage_access_key_id,media_storage_secret_access_key,media_storage_base_url) VALUES($1,'postgresql://private-secret@localhost/test','redis://localhost','private-key','test@example','auto','private-id','private-media-secret','https://media.example')")
            .bind(id).execute(&pool).await.unwrap();
    }
    sqlx::query("UPDATE tenants SET lifecycle_state='provisioning' WHERE id=$1")
        .bind(ids[3])
        .execute(&pool)
        .await
        .unwrap();
    let now = (OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000) as i64;
    let bucket = (now / 10000 - 4) * 10000;
    for (index, id) in ids.into_iter().enumerate() {
        let multiplier = 1_i64 << index;
        for (offset, transactions) in [(-9000, multiplier * 10), (1000, multiplier * 20)] {
            sqlx::query("INSERT INTO tenant_usage_samples(tenant_id,sampled_at,database_bytes,connections,active_connections,transactions,stats_reset) VALUES($1,$2,$3,$4,1,$5,'reset-a')")
                .bind(id).bind(bucket+offset).bind(multiplier*100+10).bind(multiplier).bind(transactions).execute(&pool).await.unwrap();
        }
    }
    // Two samples in one bucket contribute only the latest reading.
    sqlx::query("INSERT INTO tenant_usage_samples(tenant_id,sampled_at,database_bytes,connections,active_connections,transactions,stats_reset) VALUES($1,$2,120,2,1,21,'reset-a')")
        .bind(ids[0]).bind(bucket+1500).execute(&pool).await.unwrap();
    // A failed probe must not turn partial fleet totals into apparently complete values.
    for (index, id) in ids[..3].iter().enumerate() {
        sqlx::query("INSERT INTO tenant_usage_samples(tenant_id,sampled_at,database_bytes,connections,active_connections,transactions,stats_reset) VALUES($1,$2,$3,$4,$4,$5,'reset-a')")
            .bind(id).bind(bucket+11000).bind((index>0).then_some(100_i64)).bind((index>0).then_some(1_i64)).bind((index>0).then_some(100_i64)).execute(&pool).await.unwrap();
    }
    let app = state(pool);
    let Json(fleet) = usage::aggregate_history(
        State(app.clone()),
        headers.clone(),
        Query(usage::AggregateScope::default()),
    )
    .await
    .unwrap();
    let value = serde_json::to_value(&fleet).unwrap();
    assert_eq!(value["eligible_tenants"], 3);
    let complete = value["samples"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["sampled_at"] == bucket)
        .unwrap();
    assert_eq!(complete["database_bytes"], 740);
    assert_eq!(complete["connections"], 8);
    assert_eq!(complete["rate"], 8.0);
    let partial = value["samples"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["sampled_at"] == bucket + 10000)
        .unwrap();
    assert_eq!(partial["reporting_tenants"], 2);
    assert!(partial["database_bytes"].is_null());
    assert!(partial["rate"].is_null());
    let Json(instance) = usage::aggregate_history(
        State(app.clone()),
        headers.clone(),
        Query(usage::AggregateScope {
            instance_id: Some(host),
        }),
    )
    .await
    .unwrap();
    let instance = serde_json::to_value(instance).unwrap();
    assert_eq!(instance["eligible_tenants"], 2);
    let point = instance["samples"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["sampled_at"] == bucket)
        .unwrap();
    assert_eq!(point["database_bytes"], 330);
    assert_eq!(point["rate"], 4.0);
    let serialized = value.to_string();
    assert!(!serialized.contains("private"));
    assert!(!serialized.contains("postgresql"));
    let Json(empty) = usage::aggregate_history(
        State(app.clone()),
        headers,
        Query(usage::AggregateScope {
            instance_id: Some(Uuid::new_v4()),
        }),
    )
    .await
    .unwrap();
    assert_eq!(serde_json::to_value(empty).unwrap()["eligible_tenants"], 0);
    assert_eq!(
        usage::aggregate_history(
            State(app),
            HeaderMap::new(),
            Query(usage::AggregateScope::default())
        )
        .await
        .unwrap_err()
        .0,
        StatusCode::UNAUTHORIZED
    );
}
