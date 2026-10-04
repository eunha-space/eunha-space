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
    sqlx::query("INSERT INTO instances(id,name,address) VALUES($1,'test','127.0.0.1')")
        .bind(instance)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO runtimes(id,name,instance_id,tenant_directory,service_label,binary_path,bind_address,blue_bind_address,green_bind_address,origin_hostname) VALUES($1,'test',$2,'/tmp/test','test.runtime','/tmp/test/eunha','127.0.0.1:60000','127.0.0.1:61000','127.0.0.1:61001','test.origin.example')").bind(runtime).bind(instance).execute(pool).await.unwrap();
    let user_id = Uuid::new_v4();
    sqlx::query("INSERT INTO users(id,email,password_hash,email_verified_at,is_admin) VALUES($1,'admin@example.com','unused',now(),true)")
        .bind(user_id).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO user_sessions(token_hash,user_id,expires_at) VALUES($1,$2,now()+interval '1 hour')").bind(token_hash("test-session")).bind(user_id).execute(pool).await.unwrap();
    let mut headers = HeaderMap::new();
    headers.insert(
        header::COOKIE,
        "eunha_space_customer_session=test-session".parse().unwrap(),
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
    let Json(history) = usage::history(
        State(state.clone()),
        headers,
        Path(id),
        Query(usage_range::Range::default()),
    )
    .await
    .unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(
        serde_json::to_value(&history[0]).unwrap()["database_bytes"],
        serde_json::Value::Null
    );
    assert_eq!(
        usage::history(
            State(state),
            HeaderMap::new(),
            Path(id),
            Query(usage_range::Range::default())
        )
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
    sqlx::query("INSERT INTO tenant_configuration(tenant_id,database_url,redis_url,smtp_host,smtp_port,smtp_username,smtp_password,smtp_from,media_storage_region,media_storage_access_key_id,media_storage_secret_access_key,media_storage_base_url) VALUES($1,'postgresql://tenant:private-secret@127.0.0.1:1/missing','redis://127.0.0.1','smtp.example.com',465,'smtp-user','private-key','test@example','auto','private-id','private-media-secret','https://media.example')")
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
    let Json(history) = usage::history(
        State(state),
        headers,
        Path(id),
        Query(usage_range::Range::default()),
    )
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
        sqlx::query("INSERT INTO tenant_configuration(tenant_id,database_url,redis_url,smtp_host,smtp_port,smtp_username,smtp_password,smtp_from,media_storage_region,media_storage_access_key_id,media_storage_secret_access_key,media_storage_base_url) VALUES($1,'postgresql://private-secret@localhost/test','redis://localhost','smtp.example.com',465,'smtp-user','private-key','test@example','auto','private-id','private-media-secret','https://media.example')")
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
    assert_eq!(partial["database_bytes"], 200);
    assert_eq!(partial["connections"], 2);
    assert_eq!(partial["active_connections"], 2);
    assert_eq!(partial["rate_reporting_tenants"], 2);
    assert!(partial["rate"].as_f64().unwrap() > 0.0);
    let Json(instance) = usage::aggregate_history(
        State(app.clone()),
        headers.clone(),
        Query(usage::AggregateScope {
            instance_id: Some(host),
            range: usage_range::Range::default(),
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
            ..Default::default()
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

#[sqlx::test(migrations = "./migrations")]
#[ignore = "requires disposable PostgreSQL; use mise run test-integration"]
async fn usage_ranges_include_old_history_and_compute_rates_before_rollup(pool: PgPool) {
    let (runtime, headers) = fixture(&pool).await;
    let id = tenant(&pool, runtime).await;
    sqlx::query("UPDATE tenants SET lifecycle_state='ready' WHERE id=$1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    let now = (OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000) as i64;
    let at = (now / 3600000 - 48) * 3600000;
    for (offset, counter) in [(10000, 10), (20000, 30), (30000, 50)] {
        sqlx::query("INSERT INTO tenant_usage_samples(tenant_id,sampled_at,database_bytes,connections,active_connections,transactions,stats_reset) VALUES($1,$2,1024,2,1,$3,'reset')")
            .bind(id).bind(at+offset).bind(counter as i64).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO tenant_service_samples(tenant_id,sampled_at,epoch,requests,errors,duration_sum,duration_count,pool,pool_idle,in_flight,aborted,rejected) VALUES($1,$2,'blue:1',$3,0,$3*0.1,$3,2,1,0,0,0)")
            .bind(id).bind(at+offset).bind(counter as f64).execute(&pool).await.unwrap();
    }
    let s = state(pool);
    let range = usage_range::Range {
        minutes: Some(10080),
    };
    let Json(short) = usage::history(
        State(s.clone()),
        headers.clone(),
        Path(id),
        Query(Default::default()),
    )
    .await
    .unwrap();
    assert!(short.is_empty());
    let Json(long) = usage::history(State(s.clone()), headers.clone(), Path(id), Query(range))
        .await
        .unwrap();
    assert!(!long.is_empty());
    assert!(long.len() <= 361);
    // The first reading lacks a predecessor: the coarse bucket is a gap,
    // rather than incorrectly calculating across a multi-minute interval.
    assert!(serde_json::to_value(&long).unwrap()[0]["rate"].is_null());
    let Json(service) = service_usage::history(
        State(s.clone()),
        headers.clone(),
        Query(service_usage::Scope {
            tenant_id: Some(id),
            range,
            ..Default::default()
        }),
    )
    .await
    .unwrap();
    assert!(
        !serde_json::to_value(service).unwrap()["samples"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        usage::history(
            State(s),
            headers,
            Path(id),
            Query(usage_range::Range {
                minutes: Some(40321)
            })
        )
        .await
        .unwrap_err()
        .0,
        StatusCode::BAD_REQUEST
    );
}

#[sqlx::test(migrations = "./migrations")]
#[ignore = "requires disposable PostgreSQL; use mise run test-integration"]
async fn smtp_is_owner_scoped_custom_only_and_write_only(pool: PgPool) {
    let (runtime, _) = fixture(&pool).await;
    let id = tenant(&pool, runtime).await;
    let owner = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users(id,email,email_verified_at) VALUES($1,'smtp-owner@example.com',now())",
    )
    .bind(owner)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO user_sessions(token_hash,user_id,expires_at) VALUES($1,$2,now()+interval '1 hour')").bind(token_hash("smtp-session")).bind(owner).execute(&pool).await.unwrap();
    let mut headers = HeaderMap::new();
    headers.insert(
        header::COOKIE,
        "eunha_space_customer_session=smtp-session".parse().unwrap(),
    );
    sqlx::query("UPDATE tenants SET user_id=$2,lifecycle_state='ready' WHERE id=$1")
        .bind(id)
        .bind(owner)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO tenant_configuration(tenant_id,database_url,redis_url,smtp_host,smtp_port,smtp_username,smtp_password,smtp_from,media_storage_region,media_storage_access_key_id,media_storage_secret_access_key,media_storage_base_url) VALUES($1,'postgresql://localhost/test','redis://localhost','smtp.example.com',465,'smtp-user','smtp-secret','mail@example.com','auto','media-key','media-secret','https://media.example.com')").bind(id).execute(&pool).await.unwrap();
    let shared = state(pool.clone());
    assert_eq!(
        smtp::get(State(shared.clone()), headers.clone(), Path(id))
            .await
            .unwrap_err()
            .0,
        StatusCode::CONFLICT
    );
    sqlx::query("INSERT INTO tenant_domains(id,tenant_id,hostname,kind,status,is_primary) VALUES($1,$2,'custom.example.com','custom','active',true)").bind(Uuid::new_v4()).bind(id).execute(&pool).await.unwrap();
    let settings = serde_json::from_value(serde_json::json!({"host":"smtp.example.com","port":587,"username":"private-login","password":"private-password","from":"mail@custom.example.com"})).unwrap();
    smtp::save(
        State(shared.clone()),
        headers.clone(),
        Path(id),
        Json(settings),
    )
    .await
    .unwrap();
    let Json(status) = smtp::get(State(shared.clone()), headers.clone(), Path(id))
        .await
        .unwrap();
    assert_eq!(status["configured"], true);
    assert!(!status.to_string().contains("private-"));
    assert!(status.get("username").is_none());
    assert!(status.get("password").is_none());
    let (password, revision) = sqlx::query_as::<_, (String,i64)>("SELECT c.smtp_password,t.config_revision FROM tenant_configuration c JOIN tenants t ON t.id=c.tenant_id WHERE t.id=$1").bind(id).fetch_one(&pool).await.unwrap();
    assert_eq!(password, "private-password");
    assert_eq!(revision, 2);
    let detail = sqlx::query_scalar::<_, serde_json::Value>(
        "SELECT detail FROM operations WHERE tenant_id=$1 AND kind='smtp_update'",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(detail, serde_json::json!({}));
    let other = tenant(&pool, runtime).await;
    assert_eq!(
        smtp::get(State(shared.clone()), headers, Path(other))
            .await
            .unwrap_err()
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        smtp::get(State(shared), HeaderMap::new(), Path(id))
            .await
            .unwrap_err()
            .0,
        StatusCode::UNAUTHORIZED
    );
}

#[sqlx::test(migrations = false)]
#[ignore = "requires disposable PostgreSQL; use mise run test-integration"]
async fn smtp_migration_preserves_owner_settings_and_converts_legacy_mail(pool: PgPool) {
    for migration in sqlx::migrate!("./migrations")
        .iter()
        .filter(|m| m.version < 49)
    {
        sqlx::raw_sql(&migration.sql).execute(&pool).await.unwrap();
    }
    let (runtime, _) = fixture(&pool).await;
    let legacy = tenant(&pool, runtime).await;
    let custom = tenant(&pool, runtime).await;
    for id in [legacy, custom] {
        sqlx::query("INSERT INTO tenant_configuration(tenant_id,database_url,redis_url,resend_api_key,resend_from,media_storage_region,media_storage_access_key_id,media_storage_secret_access_key,media_storage_base_url) VALUES($1,'postgresql://localhost/test','redis://localhost','legacy-private-key','mail@example.com','auto','media-key','media-secret','https://media.example.com')").bind(id).execute(&pool).await.unwrap();
    }
    sqlx::query("UPDATE tenant_configuration SET smtp_host='smtp.owner.example',smtp_port=587,smtp_username='owner-login',smtp_password='owner-private-password',smtp_from='mail@owner.example' WHERE tenant_id=$1").bind(custom).execute(&pool).await.unwrap();
    sqlx::raw_sql(include_str!("../migrations/045_smtp_only.sql"))
        .execute(&pool)
        .await
        .unwrap();
    let (host, port, password) = sqlx::query_as::<_, (String, i32, String)>(
        "SELECT smtp_host,smtp_port,smtp_password FROM tenant_configuration WHERE tenant_id=$1",
    )
    .bind(legacy)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        (host.as_str(), port, password.as_str()),
        ("smtp.resend.com", 465, "legacy-private-key")
    );
    let (host, password) = sqlx::query_as::<_, (String, String)>(
        "SELECT smtp_host,smtp_password FROM tenant_configuration WHERE tenant_id=$1",
    )
    .bind(custom)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        (host.as_str(), password.as_str()),
        ("smtp.owner.example", "owner-private-password")
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT config_revision FROM tenants WHERE id=$1")
            .bind(legacy)
            .fetch_one(&pool)
            .await
            .unwrap(),
        2
    );
    sqlx::raw_sql(include_str!("../migrations/049_drop_resend.sql"))
        .execute(&pool)
        .await
        .unwrap();
    let columns: i64 = sqlx::query_scalar("SELECT count(*) FROM information_schema.columns WHERE table_schema='public' AND table_name='tenant_configuration' AND column_name IN ('resend_api_key','resend_from')")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(columns, 0);
    for (id, expected_host, expected_password) in [
        (legacy, "smtp.resend.com", "legacy-private-key"),
        (custom, "smtp.owner.example", "owner-private-password"),
    ] {
        let row: (String, String) = sqlx::query_as(
            "SELECT smtp_host,smtp_password FROM tenant_configuration WHERE tenant_id=$1",
        )
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row, (expected_host.into(), expected_password.into()));
    }
}
#[sqlx::test(migrations = "./migrations")]
#[ignore = "requires disposable PostgreSQL; use mise run test-integration"]
async fn admin_middleware_uses_user_sessions_and_checks_current_role(pool: PgPool) {
    let (_, headers) = fixture(&pool).await;
    let shared = state(pool.clone());
    let app = Router::new()
        .route("/api/admin", get(|| async { StatusCode::NO_CONTENT }))
        .route_layer(axum::middleware::from_fn_with_state(
            shared.clone(),
            auth::require_admin,
        ))
        .merge(
            Router::new()
                .route("/admin", get(|| async { StatusCode::NO_CONTENT }))
                .route(
                    "/admin/{*section}",
                    get(|| async { StatusCode::NO_CONTENT }),
                )
                .route_layer(axum::middleware::from_fn_with_state(
                    shared.clone(),
                    auth::require_admin_page,
                )),
        )
        .with_state(shared.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    assert_eq!(
        client
            .get(format!("{base}/api/admin"))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let response = client
        .get(format!("{base}/admin/tenants"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(response.headers()[header::LOCATION], "/");
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    let cookie = headers[header::COOKIE].to_str().unwrap();
    for path in ["/api/admin", "/admin", "/admin/tenants"] {
        assert_eq!(
            client
                .get(format!("{base}{path}"))
                .header(header::COOKIE, cookie)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::NO_CONTENT
        );
    }
    // Both authorization paths reject expiry and unverified accounts.
    sqlx::query("UPDATE user_sessions SET expires_at=now()-interval '1 second'")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        client
            .get(format!("{base}/api/admin"))
            .header(header::COOKIE, cookie)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    sqlx::query("UPDATE user_sessions SET expires_at=now()+interval '1 hour'")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE users SET email_verified_at=NULL")
        .execute(&pool)
        .await
        .unwrap();
    assert!(admin_user(&headers, &pool).await.is_err());
    sqlx::query("UPDATE users SET email_verified_at=now(),is_admin=false")
        .execute(&pool)
        .await
        .unwrap();
    assert!(customer(&headers, &pool).await.is_ok());
    for path in ["/api/admin", "/admin", "/admin/tenants"] {
        assert_eq!(
            client
                .get(format!("{base}{path}"))
                .header(header::COOKIE, cookie)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
    }
    let Json(me) = customer_me(State(shared.clone()), headers.clone())
        .await
        .unwrap();
    assert_eq!(me["is_admin"], false);
    customer_logout(State(shared), headers.clone())
        .await
        .unwrap();
    assert!(customer(&headers, &pool).await.is_err());
    server.abort();
}

#[sqlx::test(migrations = false)]
#[ignore = "requires disposable PostgreSQL; use mise run test-integration"]
async fn users_migration_preserves_sessions_roles_and_historical_actors(pool: PgPool) {
    let migrations = sqlx::migrate!("./migrations");
    for migration in migrations.iter().filter(|m| m.version < 47) {
        sqlx::raw_sql(&migration.sql).execute(&pool).await.unwrap();
    }
    let admin_id = Uuid::new_v4();
    let operator_id = Uuid::new_v4();
    let legacy_id = Uuid::new_v4();
    let customer_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO operators(id,username) VALUES($1,'admin@example.com'),($2,'old-admin')",
    )
    .bind(operator_id)
    .bind(legacy_id)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO customers(id,email,password_hash,email_verified_at,operator_id) VALUES($1,'admin@example.com','preserved',now(),$2),($3,'customer@example.com',NULL,now(),NULL)")
        .bind(admin_id).bind(operator_id).bind(customer_id).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO customer_sessions(token_hash,customer_id,expires_at,from_email_link) VALUES($1,$2,now()+interval '1 hour',true)")
        .bind(token_hash("migration-session")).bind(admin_id).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO customer_email_verifications(token_hash,customer_id,expires_at) VALUES($1,$2,now()+interval '1 hour')")
        .bind(token_hash("migration-link")).bind(customer_id).execute(&pool).await.unwrap();
    let admin_op = Uuid::new_v4();
    let legacy_op = Uuid::new_v4();
    let customer_op = Uuid::new_v4();
    sqlx::query("INSERT INTO operations(id,kind,status,requested_by,requested_by_customer) VALUES($1,'health_check','succeeded',$2,NULL),($3,'health_check','succeeded',$4,NULL),($5,'health_check','succeeded',NULL,$6)")
        .bind(admin_op).bind(operator_id).bind(legacy_op).bind(legacy_id).bind(customer_op).bind(customer_id).execute(&pool).await.unwrap();
    sqlx::raw_sql(include_str!("../migrations/047_users.sql"))
        .execute(&pool)
        .await
        .unwrap();
    sqlx::raw_sql(include_str!("../migrations/048_drop_legacy_username.sql"))
        .execute(&pool)
        .await
        .unwrap();
    for (operation_id, expected_actor) in [
        (admin_op, admin_id),
        (legacy_op, legacy_id),
        (customer_op, customer_id),
    ] {
        let actor: Uuid = sqlx::query_scalar("SELECT requested_by FROM operations WHERE id=$1")
            .bind(operation_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(actor, expected_actor);
    }
    let (email, hash, is_admin): (String, String, bool) =
        sqlx::query_as("SELECT email,password_hash,is_admin FROM users WHERE id=$1")
            .bind(admin_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(email, "admin@example.com");
    assert_eq!(hash, "preserved");
    assert!(is_admin);
    let legacy: (Option<String>, Option<String>, bool) =
        sqlx::query_as("SELECT email,password_hash,is_admin FROM users WHERE id=$1")
            .bind(legacy_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(legacy, (None, None, false));
    let legacy_columns: i64 = sqlx::query_scalar("SELECT count(*) FROM information_schema.columns WHERE table_schema='public' AND table_name='users' AND column_name='legacy_username'")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(legacy_columns, 0);
    let link_owner: Uuid =
        sqlx::query_scalar("SELECT user_id FROM user_email_verifications WHERE token_hash=$1")
            .bind(token_hash("migration-link"))
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(link_owner, customer_id);
    let mut headers = HeaderMap::new();
    headers.insert(
        header::COOKIE,
        "eunha_space_customer_session=migration-session"
            .parse()
            .unwrap(),
    );
    assert_eq!(admin_user(&headers, &pool).await.unwrap().0, admin_id);
    assert!(customer_email_link_session(&headers, &pool).await.unwrap());
    let old_tables: i64 = sqlx::query_scalar("SELECT count(*) FROM information_schema.tables WHERE table_schema='public' AND table_name IN ('operators','customers','customer_sessions','customer_email_verifications')")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(old_tables, 0);
    assert!(
        sqlx::query("UPDATE users SET is_admin=true WHERE id=$1")
            .bind(legacy_id)
            .execute(&pool)
            .await
            .is_err()
    );
}

#[sqlx::test(migrations = "./migrations")]
#[ignore = "requires disposable PostgreSQL; use mise run test-integration"]
async fn domains_are_reusable_only_after_purge_completes(pool: PgPool) {
    let (runtime, _) = fixture(&pool).await;
    let old = tenant(&pool, runtime).await;
    let domain: String = sqlx::query_scalar("SELECT domain FROM tenants WHERE id=$1")
        .bind(old)
        .fetch_one(&pool)
        .await
        .unwrap();
    let replacement = Uuid::new_v4();
    for lifecycle in ["ready", "retained", "purging"] {
        sqlx::query("UPDATE tenants SET lifecycle_state=$2,desired_state='disabled' WHERE id=$1")
            .bind(old)
            .bind(lifecycle)
            .execute(&pool)
            .await
            .unwrap();
        let error = sqlx::query("INSERT INTO tenants(id,domain,title,runtime_id,desired_state,observed_state) VALUES($1,$2,'Replacement',$3,'running','unknown')")
            .bind(replacement).bind(&domain).bind(runtime).execute(&pool).await.unwrap_err();
        assert_eq!(
            error.as_database_error().unwrap().constraint(),
            Some("tenants_domain_key")
        );
    }
    sqlx::query("UPDATE tenants SET lifecycle_state='purged' WHERE id=$1")
        .bind(old)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO tenants(id,domain,title,runtime_id,desired_state,observed_state) VALUES($1,$2,'Replacement',$3,'running','unknown')")
        .bind(replacement).bind(&domain).bind(runtime).execute(&pool).await.unwrap();
    let preserved: String = sqlx::query_scalar("SELECT domain FROM tenants WHERE id=$1")
        .bind(old)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(preserved, domain);
    assert!(
        sqlx::query("UPDATE tenants SET lifecycle_state='ready' WHERE id=$1")
            .bind(old)
            .execute(&pool)
            .await
            .is_err()
    );
}

#[sqlx::test(migrations = "./migrations")]
#[ignore = "requires disposable PostgreSQL; use mise run test-integration"]
async fn mail_defaults_are_isolated_by_primary_domain(pool: PgPool) {
    let (runtime, _) = fixture(&pool).await;
    let id = tenant(&pool, runtime).await;
    sqlx::query("INSERT INTO tenant_configuration(tenant_id,database_url,redis_url,media_storage_region,media_storage_access_key_id,media_storage_secret_access_key,media_storage_base_url,smtp_host,smtp_port,smtp_username,smtp_password,smtp_from) VALUES($1,'postgresql://localhost/test','redis://localhost','auto','key','secret','https://media.example','smtp.owner.example',587,'owner','owner-password','mail@owner.example')")
        .bind(id).execute(&pool).await.unwrap();
    let mut shared = state(pool.clone());
    Arc::get_mut(&mut shared).unwrap().customer_email = Some(CustomerEmailConfig {
        transport: lettre::AsyncSmtpTransport::<lettre::Tokio1Executor>::relay("smtp.example.com").unwrap().build(),
        from: "noreply@eunha.space".parse().unwrap(),
        public_url: "https://eunha.space".into(),
        tenant_smtp: serde_json::from_value(serde_json::json!({"host":"smtp.shared.example","port":465,"username":"shared","password":"shared-password","from":"noreply@eunha.space"})).unwrap(),
    });
    let mut tx = pool.begin().await.unwrap();
    smtp::set_domain_defaults(&shared, &mut tx, id, "new.eunha.space")
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let row: (String, String, String) = sqlx::query_as(
        "SELECT smtp_host,smtp_password,smtp_from FROM tenant_configuration WHERE tenant_id=$1",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        row,
        (
            "smtp.shared.example".into(),
            "shared-password".into(),
            "noreply@eunha.space".into()
        )
    );
    let mut tx = pool.begin().await.unwrap();
    smtp::change_domain_defaults(&shared, &mut tx, id, "new.eunha.space", "custom.example")
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let unset: bool = sqlx::query_scalar("SELECT smtp_host IS NULL AND smtp_port IS NULL AND smtp_username IS NULL AND smtp_password IS NULL AND smtp_from IS NULL FROM tenant_configuration WHERE tenant_id=$1").bind(id).fetch_one(&pool).await.unwrap();
    assert!(unset);
    sqlx::query("UPDATE tenant_configuration SET smtp_host='smtp.owner.example',smtp_port=587,smtp_username='owner',smtp_password='owner-password',smtp_from='mail@owner.example' WHERE tenant_id=$1").bind(id).execute(&pool).await.unwrap();
    let mut tx = pool.begin().await.unwrap();
    smtp::change_domain_defaults(&shared, &mut tx, id, "custom.example", "other.example")
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let retained: String =
        sqlx::query_scalar("SELECT smtp_password FROM tenant_configuration WHERE tenant_id=$1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(retained, "owner-password");
    let mut tx = pool.begin().await.unwrap();
    smtp::set_domain_defaults(&shared, &mut tx, id, "eunha.space.attacker.example")
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let empty: bool =
        sqlx::query_scalar("SELECT smtp_host IS NULL FROM tenant_configuration WHERE tenant_id=$1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(empty);
}
