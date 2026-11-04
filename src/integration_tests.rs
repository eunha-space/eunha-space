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
        billing: None,
        provided_domain_suffixes: vec![],
        customer_email: None,
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
    let legacy: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM information_schema.columns WHERE table_name='tenants' AND column_name='title')").fetch_one(pool).await.unwrap();
    let statement = if legacy {
        "INSERT INTO tenants(id,domain,title,runtime_id,desired_state,observed_state) VALUES($1,$2,'Test',$3,'running','unknown')"
    } else {
        "INSERT INTO tenants(id,domain,runtime_id,desired_state,observed_state) VALUES($1,$2,$3,'running','unknown')"
    };
    sqlx::query(statement)
        .bind(id)
        .bind(format!("{}.example", id.simple()))
        .bind(runtime)
        .execute(pool)
        .await
        .unwrap();
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
    jobs::schedule_due_purge(&state, id).await.unwrap();
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
            jobs::schedule_due_purge(&state, id)
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
        let error = sqlx::query("INSERT INTO tenants(id,domain,runtime_id,desired_state,observed_state) VALUES($1,$2,$3,'running','unknown')")
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
    sqlx::query("INSERT INTO tenants(id,domain,runtime_id,desired_state,observed_state) VALUES($1,$2,$3,'running','unknown')")
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

#[sqlx::test(migrations = false)]
#[ignore = "requires disposable PostgreSQL; use mise run test-integration"]
async fn site_settings_columns_are_removed(pool: PgPool) {
    for migration in sqlx::migrate!("./migrations")
        .iter()
        .filter(|m| m.version < 52)
    {
        sqlx::raw_sql(&migration.sql).execute(&pool).await.unwrap();
    }
    let (runtime, _) = fixture(&pool).await;
    let id = tenant(&pool, runtime).await;
    sqlx::raw_sql(include_str!(
        "../migrations/052_remove_site_configuration.sql"
    ))
    .execute(&pool)
    .await
    .unwrap();
    sqlx::raw_sql(include_str!("../migrations/053_remove_icon_url.sql"))
        .execute(&pool)
        .await
        .unwrap();
    let columns: i64 = sqlx::query_scalar("SELECT count(*) FROM information_schema.columns WHERE table_name IN ('tenants','tenant_configuration') AND column_name IN ('title','description','short_description','contact_email','registrations_open','approval_required','privacy_policy','terms_of_service','icon_url')")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(columns, 0);
    finish_provisioning(&state(pool.clone()), id).await;
}

async fn queue_job(pool: &PgPool, tenant_id: Uuid, kind: &str) {
    let mut tx = pool.begin().await.unwrap();
    jobs::enqueue(&mut tx, tenant_id, kind).await.unwrap();
    tx.commit().await.unwrap();
}

async fn job_finished(pool: &PgPool, id: Uuid) -> bool {
    sqlx::query_scalar("SELECT finished_at IS NOT NULL FROM lifecycle_jobs WHERE id=$1")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[sqlx::test(migrations = "./migrations")]
#[ignore = "requires disposable PostgreSQL; use mise run test-integration"]
async fn a_job_is_claimed_once_until_its_lease_runs_out(pool: PgPool) {
    let (runtime, _) = fixture(&pool).await;
    let id = tenant(&pool, runtime).await;
    queue_job(&pool, id, "provision").await;
    queue_job(&pool, id, "provision").await;
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM lifecycle_jobs WHERE tenant_id=$1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );
    let claims = futures::future::join_all((0..8).map(|_| jobs::claim(&pool))).await;
    assert_eq!(
        claims
            .into_iter()
            .filter(|claim| claim.as_ref().unwrap().is_some())
            .count(),
        1
    );
    assert!(jobs::claim(&pool).await.unwrap().is_none());
    // The control plane that held it died.
    sqlx::query("UPDATE lifecycle_jobs SET lease_until=now()-interval '1 second'")
        .execute(&pool)
        .await
        .unwrap();
    assert!(jobs::claim(&pool).await.unwrap().is_some());
}

#[sqlx::test(migrations = "./migrations")]
#[ignore = "requires disposable PostgreSQL; use mise run test-integration"]
async fn a_request_during_a_run_is_not_lost_to_its_ending(pool: PgPool) {
    let (runtime, _) = fixture(&pool).await;
    let id = tenant(&pool, runtime).await;
    let state = state(pool.clone());
    queue_job(&pool, id, "provision").await;
    let job = jobs::claim(&pool).await.unwrap().unwrap();
    // A restore arrives while the run is concluding from the state before it.
    queue_job(&pool, id, "provision").await;
    jobs::settle(&pool, &job, &jobs::Next::Done).await.unwrap();
    assert!(!job_finished(&pool, job.id).await);
    let again = jobs::claim(&pool).await.unwrap().unwrap();
    assert_eq!(again.id, job.id);
    jobs::settle(&pool, &again, &jobs::Next::Done)
        .await
        .unwrap();
    assert!(job_finished(&pool, job.id).await);
    // A wake is such a request too, but a finished job has nothing to wake.
    jobs::wake_provisioning(&state, id).await;
    assert!(jobs::claim(&pool).await.unwrap().is_none());
}

#[sqlx::test(migrations = "./migrations")]
#[ignore = "requires disposable PostgreSQL; use mise run test-integration"]
async fn a_waiting_job_waits_until_woken_and_failures_back_off(pool: PgPool) {
    let (runtime, _) = fixture(&pool).await;
    let id = tenant(&pool, runtime).await;
    let state = state(pool.clone());
    queue_job(&pool, id, "provision").await;
    let job = jobs::claim(&pool).await.unwrap().unwrap();
    let wait = jobs::Next::After(Duration::from_secs(30));
    jobs::settle(&pool, &job, &wait).await.unwrap();
    assert!(jobs::claim(&pool).await.unwrap().is_none());
    jobs::wake_provisioning(&state, id).await;
    let job = jobs::claim(&pool).await.unwrap().unwrap();
    let retry = jobs::Next::Retry("failed; retrying.".into(), Duration::from_secs(30));
    jobs::settle(&pool, &job, &retry).await.unwrap();
    assert!(jobs::claim(&pool).await.unwrap().is_none());
    let row = || {
        sqlx::query_as::<_, (i32, Option<String>)>(
            "SELECT failures,last_error FROM lifecycle_jobs WHERE id=$1",
        )
        .bind(job.id)
        .fetch_one(&pool)
    };
    assert_eq!(row().await.unwrap(), (1, Some("failed; retrying.".into())));
    // Waiting again is not failing again.
    sqlx::query("UPDATE lifecycle_jobs SET run_after=now()")
        .execute(&pool)
        .await
        .unwrap();
    let job = jobs::claim(&pool).await.unwrap().unwrap();
    jobs::settle(&pool, &job, &wait).await.unwrap();
    assert_eq!(row().await.unwrap(), (0, None));
}

#[sqlx::test(migrations = "./migrations")]
#[ignore = "requires disposable PostgreSQL; use mise run test-integration"]
async fn provisioning_waits_for_a_custom_domain_and_stops_for_a_changed_tenant(pool: PgPool) {
    let (runtime, _) = fixture(&pool).await;
    let id = tenant(&pool, runtime).await;
    let state = state(pool.clone());
    sqlx::query("UPDATE tenants SET lifecycle_state='provisioning' WHERE id=$1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO provisioning_steps(tenant_id,step) VALUES($1,'database')")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO tenant_domains(id,tenant_id,hostname,kind,status,is_primary) VALUES($1,$2,'waiting.example','custom','pending',true)")
        .bind(Uuid::new_v4())
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    queue_job(&pool, id, "provision").await;
    let job = jobs::claim(&pool).await.unwrap().unwrap();
    jobs::run(&state, &job).await;
    // The domain has not validated, so nothing about the tenant exists yet.
    assert!(!job_finished(&pool, job.id).await);
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT status FROM provisioning_steps WHERE tenant_id=$1 AND step='database'"
        )
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap(),
        "pending"
    );
    assert!(jobs::claim(&pool).await.unwrap().is_none());
    // Taken offline while waiting: the job has nothing left to do.
    sqlx::query("UPDATE tenants SET lifecycle_state='deprovisioning' WHERE id=$1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    jobs::wake_provisioning(&state, id).await;
    let job = jobs::claim(&pool).await.unwrap().unwrap();
    jobs::run(&state, &job).await;
    assert!(job_finished(&pool, job.id).await);
}

#[sqlx::test(migrations = "./migrations")]
#[ignore = "requires disposable PostgreSQL; use mise run test-integration"]
async fn provisioning_does_not_wait_for_a_provided_hostname_to_go_active(pool: PgPool) {
    let (runtime, _) = fixture(&pool).await;
    let id = tenant(&pool, runtime).await;
    let state = state(pool.clone());
    sqlx::query("UPDATE tenants SET lifecycle_state='provisioning' WHERE id=$1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO provisioning_steps(tenant_id,step) VALUES($1,'database')")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    // It only goes active once the tenant is served, so waiting for it here
    // would wait forever.
    sqlx::query("INSERT INTO tenant_domains(id,tenant_id,hostname,kind,status,is_primary) VALUES($1,$2,'new.eunha.example','provided','pending',true)")
        .bind(Uuid::new_v4())
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    queue_job(&pool, id, "provision").await;
    let job = jobs::claim(&pool).await.unwrap().unwrap();
    jobs::run(&state, &job).await;
    // This test has no server to create a database on, so reaching that failure
    // is what shows the job did not sit waiting.
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT status FROM provisioning_steps WHERE tenant_id=$1 AND step='database'"
        )
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap(),
        "failed"
    );
    assert!(job_finished(&pool, job.id).await);
}

#[sqlx::test(migrations = "./migrations")]
#[ignore = "requires disposable PostgreSQL; use mise run test-integration"]
async fn the_sweep_purges_only_retained_tenants_past_their_date_and_only_once(pool: PgPool) {
    let (runtime, _) = fixture(&pool).await;
    let due = tenant(&pool, runtime).await;
    let waiting = tenant(&pool, runtime).await;
    let serving = tenant(&pool, runtime).await;
    sqlx::query("UPDATE tenants SET lifecycle_state='retained',purge_scheduled_for=now()-interval '1 minute' WHERE id=$1").bind(due).execute(&pool).await.unwrap();
    sqlx::query("UPDATE tenants SET lifecycle_state='retained',purge_scheduled_for=now()+interval '1 day' WHERE id=$1").bind(waiting).execute(&pool).await.unwrap();
    sqlx::query("UPDATE tenants SET lifecycle_state='ready',purge_scheduled_for=now()-interval '1 minute' WHERE id=$1").bind(serving).execute(&pool).await.unwrap();
    let state = state(pool.clone());
    jobs::sweep_due_purges(&state).await.unwrap();
    jobs::sweep_due_purges(&state).await.unwrap();
    let states = sqlx::query_as::<_, (Uuid, String)>("SELECT id,lifecycle_state FROM tenants")
        .fetch_all(&pool)
        .await
        .unwrap();
    for (id, lifecycle) in states {
        let expected = if id == due {
            "purging"
        } else if id == waiting {
            "retained"
        } else {
            "ready"
        };
        assert_eq!(lifecycle, expected);
    }
    assert_eq!(
        sqlx::query_as::<_, (Uuid, String)>("SELECT tenant_id,kind FROM lifecycle_jobs")
            .fetch_all(&pool)
            .await
            .unwrap(),
        vec![(due, "purge".to_owned())]
    );
}

#[sqlx::test(migrations = "./migrations")]
#[ignore = "requires disposable PostgreSQL; use mise run test-integration"]
async fn one_reconciliation_pass_is_in_flight_per_runtime(pool: PgPool) {
    let (runtime, _) = fixture(&pool).await;
    let claims =
        futures::future::join_all((0..8).map(|_| jobs::claim_reconcile(&pool, runtime))).await;
    assert_eq!(
        claims
            .into_iter()
            .filter(|claim| *claim.as_ref().unwrap())
            .count(),
        1
    );
}

#[sqlx::test(migrations = false)]
#[ignore = "requires disposable PostgreSQL; use mise run test-integration"]
async fn the_migration_adopts_temporals_work_and_spares_overdue_tenants(pool: PgPool) {
    for migration in sqlx::migrate!("./migrations")
        .iter()
        .filter(|m| m.version < 54)
    {
        sqlx::raw_sql(&migration.sql).execute(&pool).await.unwrap();
    }
    let (runtime, _) = fixture(&pool).await;
    let mut tenants = HashMap::new();
    for lifecycle in [
        "provisioning",
        "deprovisioning",
        "purging",
        "retained",
        "ready",
    ] {
        let id = tenant(&pool, runtime).await;
        sqlx::query("UPDATE tenants SET lifecycle_state=$1,purge_scheduled_for=now()-interval '1 day' WHERE id=$2")
            .bind(lifecycle)
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
        tenants.insert(lifecycle, id);
    }
    let later = tenant(&pool, runtime).await;
    sqlx::query("UPDATE tenants SET lifecycle_state='retained',purge_scheduled_for=now()+interval '1 day' WHERE id=$1").bind(later).execute(&pool).await.unwrap();
    let moving = tenants["ready"];
    let operation = Uuid::new_v4();
    sqlx::query("INSERT INTO operations(id,tenant_id,runtime_id,kind,status,started_at) VALUES($1,$2,$3,'tenant_domain_change','running',now())").bind(operation).bind(moving).bind(runtime).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO domain_changes(id,tenant_id,operation_id,from_domain,to_domain) VALUES($1,$2,$3,'a.example','b.example')").bind(Uuid::new_v4()).bind(moving).bind(operation).execute(&pool).await.unwrap();
    sqlx::raw_sql(include_str!("../migrations/054_lifecycle_jobs.sql"))
        .execute(&pool)
        .await
        .unwrap();
    let mut adopted =
        sqlx::query_as::<_, (Uuid, String)>("SELECT tenant_id,kind FROM lifecycle_jobs")
            .fetch_all(&pool)
            .await
            .unwrap();
    adopted.sort();
    let mut expected = vec![
        (tenants["provisioning"], "provision".to_owned()),
        (tenants["deprovisioning"], "deprovision".to_owned()),
        (tenants["purging"], "purge".to_owned()),
        (moving, "domain_change".to_owned()),
    ];
    expected.sort();
    assert_eq!(adopted, expected);
    // Nothing is purged for a date that passed while Temporal was in charge,
    // and a date still ahead is kept.
    let state = state(pool.clone());
    jobs::sweep_due_purges(&state).await.unwrap();
    for id in [tenants["retained"], later] {
        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT lifecycle_state FROM tenants WHERE id=$1")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap(),
            "retained"
        );
    }
    assert!(
        sqlx::query_scalar::<_, bool>(
            "SELECT purge_scheduled_for > now()+interval '6 days' FROM tenants WHERE id=$1"
        )
        .bind(tenants["retained"])
        .fetch_one(&pool)
        .await
        .unwrap()
    );
    assert!(
        sqlx::query_scalar::<_, bool>(
            "SELECT purge_scheduled_for < now()+interval '2 days' FROM tenants WHERE id=$1"
        )
        .bind(later)
        .fetch_one(&pool)
        .await
        .unwrap()
    );
}

/// What the TossPayments stand-in answers the next charge with.
#[derive(Clone, Copy)]
enum TossScript {
    Approve,
    Decline,
    /// Fails without recording anything.
    Outage,
    /// Approves, then the answer is lost.
    LostApproval,
}

#[derive(Clone, Default)]
struct TossStandIn {
    script: Arc<std::sync::Mutex<std::collections::VecDeque<TossScript>>>,
    /// The idempotency key and order id of every charge, in order.
    charges: Arc<std::sync::Mutex<Vec<(String, String)>>>,
    /// What TossPayments recorded, by order id.
    orders: Arc<std::sync::Mutex<HashMap<String, serde_json::Value>>>,
}

impl TossStandIn {
    fn answer(&self, answers: &[TossScript]) {
        self.script.lock().unwrap().extend(answers);
    }
    fn charges(&self) -> Vec<(String, String)> {
        self.charges.lock().unwrap().clone()
    }
}

async fn toss_stand_in() -> (TossStandIn, String) {
    let toss = TossStandIn::default();
    let issue = |Json(body): Json<serde_json::Value>| async move {
        Json(serde_json::json!({
            "mId": "tvivarepublica",
            "customerKey": body["customerKey"],
            "billingKey": "bk_never_shown",
            "cardCompany": "현대",
            "cardNumber": "43301234****123*",
        }))
    };
    let charge = |State(toss): State<TossStandIn>,
                  headers: HeaderMap,
                  Json(body): Json<serde_json::Value>| async move {
        let order_id = body["orderId"].as_str().unwrap().to_owned();
        let key = headers
            .get("Idempotency-Key")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        assert_eq!(
            headers.get(header::AUTHORIZATION).unwrap(),
            "Basic dGVzdF9za19zZWNyZXQ6"
        );
        toss.charges.lock().unwrap().push((key, order_id.clone()));
        let next = toss
            .script
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(TossScript::Approve);
        let approved = serde_json::json!({
            "paymentKey": format!("pk_{order_id}"),
            "orderId": order_id,
            "status": "DONE",
            "totalAmount": body["amount"],
            "receipt": {"url": "https://receipt.example/r"},
        });
        match next {
            TossScript::Approve => {
                toss.orders
                    .lock()
                    .unwrap()
                    .insert(order_id, approved.clone());
                (StatusCode::OK, Json(approved))
            }
            TossScript::LostApproval => {
                toss.orders.lock().unwrap().insert(order_id, approved);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(
                        serde_json::json!({"code":"FAILED_INTERNAL_SYSTEM_PROCESSING","message":"x"}),
                    ),
                )
            }
            TossScript::Decline => (
                StatusCode::FORBIDDEN,
                Json(
                    serde_json::json!({"code":"REJECT_CARD_COMPANY","message":"잔액이 부족합니다."}),
                ),
            ),
            TossScript::Outage => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"code":"FAILED_INTERNAL_SYSTEM_PROCESSING","message":"x"})),
            ),
        }
    };
    let lookup = |State(toss): State<TossStandIn>, Path(order_id): Path<String>| async move {
        match toss.orders.lock().unwrap().get(&order_id) {
            Some(payment) => (StatusCode::OK, Json(payment.clone())),
            None => (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"code":"NOT_FOUND_PAYMENT","message":"없음"})),
            ),
        }
    };
    let app = Router::new()
        .route("/v1/billing/authorizations/issue", post(issue))
        .route("/v1/billing/{billing_key}", post(charge))
        .route("/v1/payments/orders/{order_id}", get(lookup))
        .with_state(toss.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (toss, base)
}

fn billed_state(pool: PgPool, toss_base: &str) -> Shared {
    let mut state = Arc::into_inner(state(pool)).unwrap();
    state.billing = Some(Arc::new(
        billing::Config::new(
            "test_ck".into(),
            "test_sk_secret".into(),
            toss_base.into(),
            9900,
            "eunha space".into(),
        )
        .unwrap(),
    ));
    Arc::new(state)
}

async fn billed_customer(pool: &PgPool) -> (Uuid, HeaderMap) {
    let user_id = Uuid::new_v4();
    sqlx::query("INSERT INTO users(id,email,password_hash,email_verified_at) VALUES($1,'customer@example.com','unused',now())")
        .bind(user_id).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO user_sessions(token_hash,user_id,expires_at) VALUES($1,$2,now()+interval '1 hour')").bind(token_hash("customer-session")).bind(user_id).execute(pool).await.unwrap();
    let mut headers = HeaderMap::new();
    headers.insert(
        header::COOKIE,
        "eunha_space_customer_session=customer-session"
            .parse()
            .unwrap(),
    );
    (user_id, headers)
}

async fn subscribe(state: &Shared, headers: &HeaderMap) -> serde_json::Value {
    let Json(checkout) = billing::checkout(State(state.clone()), headers.clone())
        .await
        .unwrap();
    let checkout = serde_json::to_value(checkout).unwrap();
    assert_eq!(checkout["client_key"], "test_ck");
    let customer_key: Uuid = serde_json::from_value(checkout["customer_key"].clone()).unwrap();
    let Json(view) = billing::authorize(
        State(state.clone()),
        headers.clone(),
        Json(
            serde_json::from_value(
                serde_json::json!({"auth_key":"auth","customer_key":customer_key}),
            )
            .unwrap(),
        ),
    )
    .await
    .unwrap();
    serde_json::to_value(view).unwrap()
}

async fn subscription_status(pool: &PgPool, user_id: Uuid) -> String {
    sqlx::query_scalar("SELECT status FROM subscriptions WHERE user_id=$1")
        .bind(user_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn make_due(pool: &PgPool, user_id: Uuid) {
    sqlx::query("UPDATE subscriptions SET next_attempt_at=now() WHERE user_id=$1")
        .bind(user_id)
        .execute(pool)
        .await
        .unwrap();
}

#[sqlx::test(migrations = "./migrations")]
#[ignore = "requires disposable PostgreSQL; use mise run test-integration"]
async fn a_charge_with_no_answer_is_asked_again_under_the_same_key(pool: PgPool) {
    let (toss, base) = toss_stand_in().await;
    let state = billed_state(pool.clone(), &base);
    let (user_id, headers) = billed_customer(&pool).await;
    let mut tx = pool.begin().await.unwrap();
    assert_eq!(
        billing::require_subscription(&state, &mut tx, user_id)
            .await
            .unwrap_err()
            .0,
        StatusCode::PAYMENT_REQUIRED
    );
    tx.rollback().await.unwrap();

    // A redirect carrying somebody else's customer key registers nothing.
    let _ = billing::checkout(State(state.clone()), headers.clone())
        .await
        .unwrap();
    let foreign = billing::authorize(
        State(state.clone()),
        headers.clone(),
        Json(
            serde_json::from_value(
                serde_json::json!({"auth_key":"auth","customer_key":Uuid::new_v4()}),
            )
            .unwrap(),
        ),
    )
    .await;
    assert_eq!(foreign.err().unwrap().0, StatusCode::BAD_REQUEST);

    toss.answer(&[TossScript::Outage]);
    let view = subscribe(&state, &headers).await;
    assert_eq!(view["subscription"]["status"], "incomplete");
    assert_eq!(view["payments"][0]["status"], "pending");
    assert_eq!(view["subscription"]["card_number"], "43301234****123*");
    assert!(!view.to_string().contains("bk_never_shown"));
    let first_order = toss.charges()[0].1.clone();
    assert_eq!(toss.charges()[0].0, first_order);

    // Asked again soon after, the order is looked up rather than sent again
    // under a key that would only replay the failure.
    make_due(&pool, user_id).await;
    assert!(billing::charge_due(&state, None).await.unwrap());
    assert_eq!(toss.charges().len(), 1);
    assert_eq!(subscription_status(&pool, user_id).await, "incomplete");

    // Long enough after it was sent, an order TossPayments never recorded is
    // given up for a new one, which is charged.
    sqlx::query("UPDATE subscription_payments SET charge_attempted_at=now()-interval '1 hour' WHERE order_id=$1")
        .bind(&first_order)
        .execute(&pool)
        .await
        .unwrap();
    make_due(&pool, user_id).await;
    assert!(billing::charge_due(&state, None).await.unwrap());
    let charges = toss.charges();
    assert_eq!(charges.len(), 2);
    assert_ne!(charges[1].1, first_order);
    assert_eq!(charges[1].0, charges[1].1);
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT status FROM subscription_payments WHERE order_id=$1"
        )
        .bind(&first_order)
        .fetch_one(&pool)
        .await
        .unwrap(),
        "expired"
    );
    assert_eq!(subscription_status(&pool, user_id).await, "active");
    let (paid, months) = sqlx::query_as::<_, (i64, bool)>(
        "SELECT count(*) FILTER (WHERE p.status='paid'),bool_and(s.current_period_end=s.anchor_at+interval '1 month' AND s.next_attempt_at=s.current_period_end) FROM subscription_payments p JOIN subscriptions s ON s.user_id=p.user_id WHERE p.user_id=$1",
    )
    .bind(user_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!((paid, months), (1, true));
    let mut tx = pool.begin().await.unwrap();
    billing::require_subscription(&state, &mut tx, user_id)
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    // Nothing is due until the period ends.
    assert!(!billing::charge_due(&state, None).await.unwrap());
    let audit: String = sqlx::query_scalar("SELECT string_agg(detail::text||coalesce(error,''),' ') FROM operations WHERE requested_by=$1")
        .bind(user_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(!audit.contains("bk_never_shown"));
}

#[sqlx::test(migrations = "./migrations")]
#[ignore = "requires disposable PostgreSQL; use mise run test-integration"]
async fn declined_renewals_suspend_instances_without_a_purge_until_paid_again(pool: PgPool) {
    let (toss, base) = toss_stand_in().await;
    let state = billed_state(pool.clone(), &base);
    let (runtime, _) = fixture(&pool).await;
    let (user_id, headers) = billed_customer(&pool).await;
    assert_eq!(
        subscribe(&state, &headers).await["subscription"]["status"],
        "active"
    );
    let id = tenant(&pool, runtime).await;
    sqlx::query("UPDATE tenants SET user_id=$2,lifecycle_state='ready' WHERE id=$1")
        .bind(id)
        .bind(user_id)
        .execute(&pool)
        .await
        .unwrap();

    toss.answer(&[TossScript::Decline; 4]);
    for attempt in 1..=3 {
        make_due(&pool, user_id).await;
        assert!(billing::charge_due(&state, None).await.unwrap());
        assert_eq!(subscription_status(&pool, user_id).await, "past_due");
        let (failed, last_error): (i32, String) =
            sqlx::query_as("SELECT failed_attempts,last_error FROM subscriptions WHERE user_id=$1")
                .bind(user_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            (failed, last_error.as_str()),
            (attempt, "잔액이 부족합니다.")
        );
        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT lifecycle_state FROM tenants WHERE id=$1")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap(),
            "ready"
        );
    }
    make_due(&pool, user_id).await;
    assert!(billing::charge_due(&state, None).await.unwrap());
    assert_eq!(subscription_status(&pool, user_id).await, "lapsed");
    // Every declined attempt was its own order.
    let orders: std::collections::HashSet<_> =
        toss.charges().into_iter().map(|(_, order)| order).collect();
    assert_eq!(orders.len(), 5);
    let (lifecycle, purge_unscheduled, suspended) = sqlx::query_as::<_, (String, bool, bool)>(
        "SELECT lifecycle_state,purge_scheduled_for IS NULL,billing_suspended_at IS NOT NULL FROM tenants WHERE id=$1",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        (lifecycle.as_str(), purge_unscheduled, suspended),
        ("deprovisioning", true, true)
    );
    // Retained with no date, the sweep never purges it.
    sqlx::query("UPDATE tenants SET lifecycle_state='retained' WHERE id=$1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    jobs::schedule_due_purge(&state, id).await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT lifecycle_state FROM tenants WHERE id=$1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap(),
        "retained"
    );
    // A restore needs a subscription first.
    assert_eq!(
        customer_restore(State(state.clone()), headers.clone(), Path(id))
            .await
            .unwrap_err()
            .0,
        StatusCode::PAYMENT_REQUIRED
    );

    // Paying again starts a new period today and serves the instance again.
    let view = subscribe(&state, &headers).await;
    assert_eq!(view["subscription"]["status"], "active");
    let (lifecycle, suspended) = sqlx::query_as::<_, (String, bool)>(
        "SELECT lifecycle_state,billing_suspended_at IS NOT NULL FROM tenants WHERE id=$1",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!((lifecycle.as_str(), suspended), ("provisioning", false));
    assert!(
        sqlx::query_scalar::<_, bool>(
            "SELECT periods_paid=1 AND anchor_at>now()-interval '1 minute' FROM subscriptions WHERE user_id=$1"
        )
        .bind(user_id)
        .fetch_one(&pool)
        .await
        .unwrap()
    );
}

#[sqlx::test(migrations = "./migrations")]
#[ignore = "requires disposable PostgreSQL; use mise run test-integration"]
async fn a_cancelled_subscription_ends_with_its_period(pool: PgPool) {
    let (toss, base) = toss_stand_in().await;
    let state = billed_state(pool.clone(), &base);
    let (user_id, headers) = billed_customer(&pool).await;
    subscribe(&state, &headers).await;
    let Json(view) = billing::cancel(State(state.clone()), headers.clone())
        .await
        .unwrap();
    let view = serde_json::to_value(view).unwrap();
    assert_eq!(view["subscription"]["status"], "active");
    assert_eq!(view["subscription"]["cancel_at_period_end"], true);
    let _ = billing::resume(State(state.clone()), headers.clone())
        .await
        .unwrap();
    let _ = billing::cancel(State(state.clone()), headers.clone())
        .await
        .unwrap();
    assert!(!billing::charge_due(&state, None).await.unwrap());
    make_due(&pool, user_id).await;
    assert!(billing::charge_due(&state, None).await.unwrap());
    assert_eq!(subscription_status(&pool, user_id).await, "canceled");
    assert_eq!(toss.charges().len(), 1);
    assert_eq!(
        billing::resume(State(state.clone()), headers.clone())
            .await
            .err()
            .unwrap()
            .0,
        StatusCode::CONFLICT
    );
}

#[sqlx::test(migrations = "./migrations")]
#[ignore = "requires disposable PostgreSQL; use mise run test-integration"]
async fn an_approval_whose_answer_was_lost_is_found_and_not_charged_again(pool: PgPool) {
    let (toss, base) = toss_stand_in().await;
    let state = billed_state(pool.clone(), &base);
    let (user_id, headers) = billed_customer(&pool).await;
    toss.answer(&[TossScript::LostApproval]);
    let view = subscribe(&state, &headers).await;
    assert_eq!(view["subscription"]["status"], "incomplete");
    make_due(&pool, user_id).await;
    assert!(billing::charge_due(&state, None).await.unwrap());
    assert_eq!(toss.charges().len(), 1);
    assert_eq!(subscription_status(&pool, user_id).await, "active");
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM subscription_payments WHERE user_id=$1 AND status='paid'"
        )
        .bind(user_id)
        .fetch_one(&pool)
        .await
        .unwrap(),
        1
    );
}
