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
        polar: None,
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
    let error = queue_runtime_release(state(pool.clone()), runtime, None, "test", None, None)
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
    let error = queue_runtime_release(state(pool.clone()), runtime, None, "test", None, None)
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
    assert_eq!(response.headers()[header::LOCATION], "/signin");
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
    // The sessions still sign in once the schema has caught up with today's.
    for migration in migrations.iter().filter(|m| m.version > 48) {
        sqlx::raw_sql(&migration.sql).execute(&pool).await.unwrap();
    }
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
    /// Refuses the control plane's own secret key.
    BadKey,
}

#[derive(Clone, Default)]
struct TossStandIn {
    script: Arc<std::sync::Mutex<std::collections::VecDeque<TossScript>>>,
    /// The idempotency key and order id of every charge, in order.
    charges: Arc<std::sync::Mutex<Vec<(String, String)>>>,
    /// What TossPayments recorded, by order id.
    orders: Arc<std::sync::Mutex<HashMap<String, serde_json::Value>>>,
    /// What the transactions API lists.
    transactions: Arc<std::sync::Mutex<Vec<serde_json::Value>>>,
    /// Whether the next cancel is refused on its merits.
    refuse_cancel: Arc<std::sync::atomic::AtomicBool>,
    /// Billing keys deleted, in order.
    deleted_keys: Arc<std::sync::Mutex<Vec<String>>>,
}

impl TossStandIn {
    fn answer(&self, answers: &[TossScript]) {
        self.script.lock().unwrap().extend(answers);
    }
    fn lists_transaction(&self, order_id: &str, status: &str, amount: i64) {
        let mut transactions = self.transactions.lock().unwrap();
        let key = format!("tx_{}", transactions.len());
        transactions.push(serde_json::json!({"transactionKey":key,"orderId":order_id,"status":status,"amount":amount}));
    }
    fn charges(&self) -> Vec<(String, String)> {
        self.charges.lock().unwrap().clone()
    }
    /// Give part of an order back as the TossPayments dashboard would.
    fn cancel_part_in_dashboard(&self, order_id: &str, refunded: i64) {
        let mut orders = self.orders.lock().unwrap();
        let order = orders.get_mut(order_id).unwrap();
        let balance = order["totalAmount"].as_i64().unwrap() - refunded;
        order["status"] = "PARTIAL_CANCELED".into();
        order["balanceAmount"] = balance.into();
    }
    /// Cancel an order as the TossPayments dashboard would.
    fn cancel_in_dashboard(&self, order_id: &str) {
        let mut orders = self.orders.lock().unwrap();
        let order = orders.get_mut(order_id).unwrap();
        order["status"] = "CANCELED".into();
        order["balanceAmount"] = 0.into();
    }
}

async fn toss_stand_in() -> (TossStandIn, String) {
    let toss = TossStandIn::default();
    let issue = |Json(body): Json<serde_json::Value>| async move {
        Json(serde_json::json!({
            "mId": "tvivarepublica",
            "customerKey": body["customerKey"],
            "billingKey": "bk_never_shown",
            // API version 2024-06-01's shape: an issuer code, not a name.
            "card": {"issuerCode": "61", "number": "43301234****123*"},
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
            TossScript::BadKey => (
                StatusCode::UNAUTHORIZED,
                Json(
                    serde_json::json!({"code":"UNAUTHORIZED_KEY","message":"인증되지 않은 시크릿 키 혹은 클라이언트 키 입니다."}),
                ),
            ),
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
    let cancel = |State(toss): State<TossStandIn>, Path(payment_key): Path<String>| async move {
        if toss
            .refuse_cancel
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            return (
                StatusCode::FORBIDDEN,
                Json(
                    serde_json::json!({"code":"NOT_CANCELABLE_PAYMENT","message":"취소 할 수 없는 결제 입니다."}),
                ),
            );
        }
        let mut orders = toss.orders.lock().unwrap();
        let Some(order) = orders
            .values_mut()
            .find(|order| order["paymentKey"] == payment_key.as_str())
        else {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"code":"NOT_FOUND_PAYMENT","message":"없음"})),
            );
        };
        order["status"] = "CANCELED".into();
        order["balanceAmount"] = 0.into();
        (StatusCode::OK, Json(order.clone()))
    };
    let transactions =
        |State(toss): State<TossStandIn>,
         axum::extract::RawQuery(query): axum::extract::RawQuery| async move {
            let query = query.unwrap();
            assert!(query.contains("startDate=2026-11-04T00:00:00"), "{query}");
            assert!(query.contains("endDate=2026-11-04T23:59:59"), "{query}");
            Json(toss.transactions.lock().unwrap().clone())
        };
    let delete_key = |State(toss): State<TossStandIn>, Path(billing_key): Path<String>| async move {
        toss.deleted_keys.lock().unwrap().push(billing_key);
        StatusCode::OK
    };
    let app = Router::new()
        .route("/v1/billing/authorizations/issue", post(issue))
        .route("/v1/billing/{billing_key}", post(charge).delete(delete_key))
        .route("/v1/payments/{payment_key}/cancel", post(cancel))
        .route("/v1/payments/orders/{order_id}", get(lookup))
        .route("/v1/transactions", get(transactions))
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
            "eunha space".into(),
            Some("tvivarepublica".into()),
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

/// The plan the billing tests subscribe to, unless they choose another.
async fn offer_a_plan(pool: &PgPool) {
    sqlx::query("INSERT INTO plans(name_en,name_ko,amount) SELECT 'Standard','기본',9900 WHERE NOT EXISTS(SELECT 1 FROM plans WHERE amount=9900)")
        .execute(pool)
        .await
        .unwrap();
}

async fn subscribe(state: &Shared, headers: &HeaderMap) -> serde_json::Value {
    offer_a_plan(&state.db).await;
    let Json(checkout) = billing::checkout(State(state.clone()), headers.clone(), None)
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
    sqlx::query("UPDATE subscriptions SET next_attempt_at=now()-interval '1 day' WHERE user_id=$1")
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
    offer_a_plan(&state.db).await;
    let _ = billing::checkout(State(state.clone()), headers.clone(), None)
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
    assert_eq!(view["subscription"]["card_company"], "현대");
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

async fn mismatches(pool: &PgPool) -> Vec<(String, String)> {
    sqlx::query_as("SELECT order_id,kind FROM toss_mismatches WHERE resolved_at IS NULL ORDER BY order_id,kind")
        .fetch_all(pool)
        .await
        .unwrap()
}

/// Set the day a payment was settled on to the one the checks below compare.
async fn settled_on_the_checked_day(pool: &PgPool, order_id: &str) {
    sqlx::query(
        "UPDATE subscription_payments SET settled_at='2026-11-04 12:00:00+09' WHERE order_id=$1",
    )
    .bind(order_id)
    .execute(pool)
    .await
    .unwrap();
}

#[sqlx::test(migrations = "./migrations")]
#[ignore = "requires disposable PostgreSQL; use mise run test-integration"]
async fn a_day_that_matches_tosspayments_raises_nothing(pool: PgPool) {
    let (toss, base) = toss_stand_in().await;
    let state = billed_state(pool.clone(), &base);
    let (_, headers) = billed_customer(&pool).await;
    let view = subscribe(&state, &headers).await;
    let order_id = view["payments"][0]["order_id"].as_str().unwrap().to_owned();
    settled_on_the_checked_day(&pool, &order_id).await;
    toss.lists_transaction(&order_id, "DONE", 9900);
    assert_eq!(
        billing_check::check_day(&state, "2026-11-04")
            .await
            .unwrap(),
        1
    );
    assert!(mismatches(&pool).await.is_empty());
}

#[sqlx::test(migrations = "./migrations")]
#[ignore = "requires disposable PostgreSQL; use mise run test-integration"]
async fn money_with_no_matching_record_is_raised_once_for_an_operator(pool: PgPool) {
    let (toss, base) = toss_stand_in().await;
    let state = billed_state(pool.clone(), &base);
    let (user_id, headers) = billed_customer(&pool).await;
    let view = subscribe(&state, &headers).await;
    let order_id = view["payments"][0]["order_id"].as_str().unwrap().to_owned();
    // Closed here as never recorded, and approved after all.
    sqlx::query("UPDATE subscription_payments SET status='expired' WHERE order_id=$1")
        .bind(&order_id)
        .execute(&pool)
        .await
        .unwrap();
    toss.lists_transaction(&order_id, "DONE", 9900);
    toss.lists_transaction("2026-11-04-0000-0001", "DONE", 5900);
    toss.lists_transaction("2026-11-04-0000-0002", "READY", 5900);
    for _ in 0..2 {
        assert_eq!(
            billing_check::check_day(&state, "2026-11-04")
                .await
                .unwrap(),
            2
        );
    }
    assert_eq!(
        mismatches(&pool).await,
        vec![
            (
                "2026-11-04-0000-0001".to_owned(),
                "unknown_order".to_owned()
            ),
            (order_id.clone(), "approved_not_recorded".to_owned()),
        ]
    );
    // Reported, not acted on: an operator decides.
    assert_eq!(subscription_status(&pool, user_id).await, "active");
    let id: Uuid = sqlx::query_scalar("SELECT id FROM toss_mismatches WHERE kind='unknown_order'")
        .fetch_one(&pool)
        .await
        .unwrap();
    let (_, admin) = fixture(&pool).await;
    billing_admin::resolve_mismatch(State(state.clone()), admin, Path(id))
        .await
        .unwrap();
    billing_check::check_day(&state, "2026-11-04")
        .await
        .unwrap();
    assert_eq!(mismatches(&pool).await.len(), 1);

    // Cancelled in full since, nothing is owed.
    sqlx::query("DELETE FROM toss_mismatches")
        .execute(&pool)
        .await
        .unwrap();
    toss.cancel_in_dashboard(&order_id);
    toss.transactions.lock().unwrap().remove(1);
    billing_check::check_day(&state, "2026-11-04")
        .await
        .unwrap();
    assert!(mismatches(&pool).await.is_empty());
}

#[sqlx::test(migrations = "./migrations")]
#[ignore = "requires disposable PostgreSQL; use mise run test-integration"]
async fn a_cancel_whose_webhook_never_came_is_recorded(pool: PgPool) {
    let (toss, base) = toss_stand_in().await;
    let state = billed_state(pool.clone(), &base);
    let (_, headers) = billed_customer(&pool).await;
    let view = subscribe(&state, &headers).await;
    let order_id = view["payments"][0]["order_id"].as_str().unwrap().to_owned();
    toss.cancel_part_in_dashboard(&order_id, 1000);
    toss.lists_transaction(&order_id, "PARTIAL_CANCELED", 1000);
    billing_check::check_day(&state, "2026-11-04")
        .await
        .unwrap();
    let refunded: i64 =
        sqlx::query_scalar("SELECT refunded_amount FROM subscription_payments WHERE order_id=$1")
            .bind(&order_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(refunded, 1000);
    assert!(mismatches(&pool).await.is_empty());
}

#[sqlx::test(migrations = "./migrations")]
#[ignore = "requires disposable PostgreSQL; use mise run test-integration"]
async fn a_payment_tosspayments_has_no_record_of_is_raised(pool: PgPool) {
    let (toss, base) = toss_stand_in().await;
    let state = billed_state(pool.clone(), &base);
    let (_, headers) = billed_customer(&pool).await;
    let view = subscribe(&state, &headers).await;
    let order_id = view["payments"][0]["order_id"].as_str().unwrap().to_owned();
    settled_on_the_checked_day(&pool, &order_id).await;
    toss.orders.lock().unwrap().remove(&order_id);
    billing_check::check_day(&state, "2026-11-04")
        .await
        .unwrap();
    assert_eq!(
        mismatches(&pool).await,
        vec![(order_id, "not_at_toss".to_owned())]
    );
}

#[sqlx::test(migrations = "./migrations")]
#[ignore = "requires disposable PostgreSQL; use mise run test-integration"]
async fn a_webhook_is_kept_with_its_headers_and_without_secrets(pool: PgPool) {
    let (_toss, base) = toss_stand_in().await;
    let state = billed_state(pool.clone(), &base);
    let mut headers = HeaderMap::new();
    headers.insert(
        "tosspayments-webhook-transmission-id",
        "whtrans_1".parse().unwrap(),
    );
    headers.insert("cookie", "private".parse().unwrap());
    headers.insert("authorization", "private".parse().unwrap());
    let body = axum::body::Bytes::from(
        serde_json::json!({"eventType":"BILLING_DELETED","data":{"billingKey":"key-that-charges-a-card"}}).to_string(),
    );
    assert_eq!(
        billing_sync::webhook(State(state.clone()), headers.clone(), body).await,
        StatusCode::OK
    );
    let (event, signed, kept, payload, status): (
        Option<String>,
        bool,
        serde_json::Value,
        Option<String>,
        i32,
    ) = sqlx::query_as(
        "SELECT event_type,signed,headers,payload,http_status FROM toss_webhook_deliveries",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(event.as_deref(), Some("BILLING_DELETED"));
    assert!(!signed);
    assert_eq!(status, 200);
    assert_eq!(
        kept,
        serde_json::json!({"tosspayments-webhook-transmission-id":"whtrans_1"})
    );
    assert!(!payload.unwrap().contains("key-that-charges-a-card"));

    headers.insert("tosspayments-webhook-signature", "v1:abc".parse().unwrap());
    billing_sync::webhook(
        State(state.clone()),
        headers,
        axum::body::Bytes::from_static(b"not json"),
    )
    .await;
    let (signed, payload): (bool, Option<String>) = sqlx::query_as(
        "SELECT signed,payload FROM toss_webhook_deliveries ORDER BY received_at DESC LIMIT 1",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(signed);
    assert!(payload.is_none());
}

#[sqlx::test(migrations = "./migrations")]
#[ignore = "requires disposable PostgreSQL; use mise run test-integration"]
async fn renewals_are_charged_at_nine_in_korea_and_nothing_else_waits(pool: PgPool) {
    let (toss, base) = toss_stand_in().await;
    let state = billed_state(pool.clone(), &base);
    let (user_id, headers) = billed_customer(&pool).await;
    subscribe(&state, &headers).await;
    let charges = toss.charges().len();
    const LATEST_NINE: &str = "((date_trunc('day',now() AT TIME ZONE 'Asia/Seoul'-interval '9 hours')+interval '9 hours') AT TIME ZONE 'Asia/Seoul')";
    let due_at = |at: String| {
        let pool = pool.clone();
        async move {
            sqlx::query(&format!(
                "UPDATE subscriptions SET next_attempt_at={at} WHERE user_id=$1"
            ))
            .bind(user_id)
            .execute(&pool)
            .await
            .unwrap();
        }
    };
    // Due since the last 09:00: it waits for the next. One second in, so this
    // also holds right after 09:00, when that time has yet to come.
    due_at(format!("{LATEST_NINE}+interval '1 second'")).await;
    assert!(!billing::charge_due(&state, None).await.unwrap());
    assert_eq!(toss.charges().len(), charges);
    // Due by the last 09:00: it is charged.
    due_at(LATEST_NINE.to_owned()).await;
    assert!(billing::charge_due(&state, None).await.unwrap());
    assert_eq!(toss.charges().len(), charges + 1);

    // A decline comes back at 09:00, however late in the day it was.
    toss.answer(&[TossScript::Decline]);
    make_due(&pool, user_id).await;
    assert!(billing::charge_due(&state, None).await.unwrap());
    let (hour, minute, status): (f64, f64, String) = sqlx::query_as(
        "SELECT extract(hour FROM next_attempt_at AT TIME ZONE 'Asia/Seoul')::float8,extract(minute FROM next_attempt_at AT TIME ZONE 'Asia/Seoul')::float8,status FROM subscriptions WHERE user_id=$1",
    )
    .bind(user_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!((hour, minute, status.as_str()), (9.0, 0.0, "past_due"));

    // An operator's retry doesn't wait.
    due_at("now()".to_owned()).await;
    assert!(billing::charge_due(&state, Some(user_id)).await.unwrap());
    assert_eq!(subscription_status(&pool, user_id).await, "active");

    // A cancelled subscription ends with its period, not at the next 09:00.
    sqlx::query(
        "UPDATE subscriptions SET cancel_at_period_end=true,next_attempt_at=now() WHERE user_id=$1",
    )
    .bind(user_id)
    .execute(&pool)
    .await
    .unwrap();
    assert!(billing::charge_due(&state, None).await.unwrap());
    assert_eq!(subscription_status(&pool, user_id).await, "canceled");
}

#[sqlx::test(migrations = "./migrations")]
#[ignore = "requires disposable PostgreSQL; use mise run test-integration"]
async fn a_charge_canceled_before_its_approval_was_recorded_is_not_paid(pool: PgPool) {
    let (toss, base) = toss_stand_in().await;
    let state = billed_state(pool.clone(), &base);
    let (user_id, headers) = billed_customer(&pool).await;
    toss.answer(&[TossScript::LostApproval]);
    let view = subscribe(&state, &headers).await;
    let order_id = view["payments"][0]["order_id"].as_str().unwrap().to_owned();

    // The event brings the renewer's look forward, which settles the order.
    toss.cancel_in_dashboard(&order_id);
    let event = axum::body::Bytes::from(
        serde_json::json!({"eventType":"PAYMENT_STATUS_CHANGED","data":{"orderId":order_id,"status":"CANCELED"}})
            .to_string(),
    );
    assert_eq!(
        billing_sync::webhook(State(state.clone()), HeaderMap::new(), event).await,
        StatusCode::OK
    );
    assert!(billing::charge_due(&state, None).await.unwrap());
    assert_eq!(toss.charges().len(), 1);
    assert_eq!(subscription_status(&pool, user_id).await, "incomplete");
    let (status, code): (String, Option<String>) =
        sqlx::query_as("SELECT status,failure_code FROM subscription_payments WHERE order_id=$1")
            .bind(&order_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(status, "failed");
    assert_eq!(code.as_deref(), Some("CANCELED"));
}

#[sqlx::test(migrations = "./migrations")]
#[ignore = "requires disposable PostgreSQL; use mise run test-integration"]
async fn a_charge_partly_canceled_before_its_approval_was_recorded_is_paid(pool: PgPool) {
    let (toss, base) = toss_stand_in().await;
    let state = billed_state(pool.clone(), &base);
    let (user_id, headers) = billed_customer(&pool).await;
    toss.answer(&[TossScript::LostApproval]);
    let view = subscribe(&state, &headers).await;
    let order_id = view["payments"][0]["order_id"].as_str().unwrap().to_owned();

    toss.cancel_part_in_dashboard(&order_id, 1000);
    make_due(&pool, user_id).await;
    assert!(billing::charge_due(&state, None).await.unwrap());
    assert_eq!(toss.charges().len(), 1);
    assert_eq!(subscription_status(&pool, user_id).await, "active");
    let (status, refunded): (String, i64) = sqlx::query_as(
        "SELECT status,refunded_amount FROM subscription_payments WHERE order_id=$1",
    )
    .bind(&order_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(status, "paid");
    assert_eq!(refunded, 1000);
}

#[sqlx::test(migrations = "./migrations")]
#[ignore = "requires disposable PostgreSQL; use mise run test-integration"]
async fn a_key_from_another_mid_is_not_charged_or_counted(pool: PgPool) {
    let (toss, base) = toss_stand_in().await;
    let state = billed_state(pool.clone(), &base);
    let (user_id, headers) = billed_customer(&pool).await;
    assert_eq!(
        subscribe(&state, &headers).await["subscription"]["status"],
        "active"
    );
    assert_eq!(
        sqlx::query_scalar::<_, Option<String>>(
            "SELECT toss_mid FROM subscriptions WHERE user_id=$1"
        )
        .bind(user_id)
        .fetch_one(&pool)
        .await
        .unwrap()
        .as_deref(),
        Some("tvivarepublica")
    );
    // Issued under the test MID before the switch to live keys.
    sqlx::query("UPDATE subscriptions SET toss_mid='tosstest' WHERE user_id=$1")
        .bind(user_id)
        .execute(&pool)
        .await
        .unwrap();
    make_due(&pool, user_id).await;
    assert!(billing::charge_due(&state, None).await.unwrap());
    assert_eq!(toss.charges().len(), 1);
    let (status, failed, later): (String, i32, bool) = sqlx::query_as(
        "SELECT status,failed_attempts,next_attempt_at>now()+interval '50 minutes' FROM subscriptions WHERE user_id=$1",
    )
    .bind(user_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!((status.as_str(), failed, later), ("active", 0, true));
    assert!(!billing::charge_due(&state, None).await.unwrap());
}

#[sqlx::test(migrations = "./migrations")]
#[ignore = "requires disposable PostgreSQL; use mise run test-integration"]
async fn a_bad_secret_key_never_counts_against_the_card(pool: PgPool) {
    let (toss, base) = toss_stand_in().await;
    let state = billed_state(pool.clone(), &base);
    let (runtime, _) = fixture(&pool).await;
    let (user_id, headers) = billed_customer(&pool).await;
    subscribe(&state, &headers).await;
    let id = tenant(&pool, runtime).await;
    sqlx::query("UPDATE tenants SET user_id=$2,lifecycle_state='ready' WHERE id=$1")
        .bind(id)
        .bind(user_id)
        .execute(&pool)
        .await
        .unwrap();
    // More renewals than it takes declines to end a subscription.
    toss.answer(&[TossScript::BadKey; 6]);
    for _ in 0..6 {
        sqlx::query("UPDATE subscription_payments SET charge_attempted_at=now()-interval '1 hour' WHERE user_id=$1 AND status='pending'")
            .bind(user_id)
            .execute(&pool)
            .await
            .unwrap();
        make_due(&pool, user_id).await;
        assert!(billing::charge_due(&state, None).await.unwrap());
    }
    let (status, failed): (String, i32) =
        sqlx::query_as("SELECT status,failed_attempts FROM subscriptions WHERE user_id=$1")
            .bind(user_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!((status.as_str(), failed), ("active", 0));
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT lifecycle_state FROM tenants WHERE id=$1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap(),
        "ready"
    );
}

async fn ready_instance(pool: &PgPool, user_id: Uuid) -> Uuid {
    let (runtime, _) = fixture(pool).await;
    let id = tenant(pool, runtime).await;
    sqlx::query("UPDATE tenants SET user_id=$2,lifecycle_state='ready' WHERE id=$1")
        .bind(id)
        .bind(user_id)
        .execute(pool)
        .await
        .unwrap();
    id
}

async fn refund(
    state: &Shared,
    headers: &HeaderMap,
    order_id: &str,
) -> Result<serde_json::Value, StatusCode> {
    billing_sync::request_refund(
        State(state.clone()),
        headers.clone(),
        Path(order_id.to_owned()),
    )
    .await
    .map(|Json(view)| serde_json::to_value(view).unwrap())
    .map_err(|error| error.0)
}

async fn lifecycle(pool: &PgPool, id: Uuid) -> (String, bool) {
    sqlx::query_as("SELECT lifecycle_state,purge_scheduled_for IS NULL FROM tenants WHERE id=$1")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[sqlx::test(migrations = "./migrations")]
#[ignore = "requires disposable PostgreSQL; use mise run test-integration"]
async fn a_customers_refund_ends_the_subscription_at_once_and_its_key_is_deleted(pool: PgPool) {
    let (toss, base) = toss_stand_in().await;
    let state = billed_state(pool.clone(), &base);
    let (user_id, headers) = billed_customer(&pool).await;
    let view = subscribe(&state, &headers).await;
    let id = ready_instance(&pool, user_id).await;
    assert_eq!(view["payments"][0]["refundable"], true);
    let order_id = view["payments"][0]["order_id"].as_str().unwrap().to_owned();

    let view = refund(&state, &headers, &order_id).await.unwrap();
    assert_eq!(view["payments"][0]["status"], "refunded");
    assert_eq!(view["payments"][0]["refunded_amount"], 9900);
    // Offline now, with its data, and nothing scheduled to destroy it.
    assert_eq!(view["subscription"]["status"], "canceled");
    assert_eq!(view["subscription"]["has_card"], false);
    assert_eq!(lifecycle(&pool, id).await, ("deprovisioning".into(), true));
    // Not charged again.
    assert!(!billing::charge_due(&state, None).await.unwrap());
    assert_eq!(toss.charges().len(), 1);
    // Asking twice gives nothing back twice.
    assert_eq!(
        refund(&state, &headers, &order_id).await.unwrap_err(),
        StatusCode::CONFLICT
    );

    // The key nothing uses any more is deleted at TossPayments, then erased.
    assert!(billing_sync::delete_retired_key(&state).await.unwrap());
    assert_eq!(*toss.deleted_keys.lock().unwrap(), vec!["bk_never_shown"]);
    assert_eq!(
        sqlx::query_as::<_, (bool, bool)>(
            "SELECT billing_key IS NULL,deleted_at IS NOT NULL FROM retired_billing_keys WHERE user_id=$1"
        )
        .bind(user_id)
        .fetch_one(&pool)
        .await
        .unwrap(),
        (true, true)
    );
    assert!(!billing_sync::delete_retired_key(&state).await.unwrap());
}

/// Paid for two periods, the second of them being served.
async fn renewed(pool: &PgPool, state: &Shared, user_id: Uuid) -> Vec<String> {
    make_due(pool, user_id).await;
    assert!(billing::charge_due(state, None).await.unwrap());
    sqlx::query_scalar(
        "SELECT order_id FROM subscription_payments WHERE user_id=$1 ORDER BY period",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await
    .unwrap()
}

#[sqlx::test(migrations = "./migrations")]
#[ignore = "requires disposable PostgreSQL; use mise run test-integration"]
async fn a_customer_refunding_an_earlier_period_ends_the_subscription_too(pool: PgPool) {
    let (toss, base) = toss_stand_in().await;
    let state = billed_state(pool.clone(), &base);
    let (user_id, headers) = billed_customer(&pool).await;
    subscribe(&state, &headers).await;
    let id = ready_instance(&pool, user_id).await;
    let orders = renewed(&pool, &state, user_id).await;

    let view = refund(&state, &headers, &orders[0]).await.unwrap();
    assert_eq!(view["subscription"]["status"], "canceled");
    assert_eq!(view["subscription"]["has_card"], false);
    assert_eq!(lifecycle(&pool, id).await.0, "deprovisioning");
    // The other payment can still be given back, and ends nothing more.
    let view = refund(&state, &headers, &orders[1]).await.unwrap();
    assert!(
        view["payments"]
            .as_array()
            .unwrap()
            .iter()
            .all(|payment| payment["status"] == "refunded")
    );
    assert_eq!(toss.charges().len(), 2);
}

#[sqlx::test(migrations = "./migrations")]
#[ignore = "requires disposable PostgreSQL; use mise run test-integration"]
async fn a_partial_refund_leaves_the_subscription_alone_until_it_is_whole(pool: PgPool) {
    let (toss, base) = toss_stand_in().await;
    let state = billed_state(pool.clone(), &base);
    let (user_id, headers) = billed_customer(&pool).await;
    let view = subscribe(&state, &headers).await;
    let order_id = view["payments"][0]["order_id"].as_str().unwrap().to_owned();
    let event = axum::body::Bytes::from(
        serde_json::json!({"eventType":"PAYMENT_STATUS_CHANGED","data":{"orderId":order_id,"status":"PARTIAL_CANCELED"}})
            .to_string(),
    );

    toss.cancel_part_in_dashboard(&order_id, 1000);
    assert_eq!(
        billing_sync::webhook(State(state.clone()), HeaderMap::new(), event.clone()).await,
        StatusCode::OK
    );
    let Json(view) = billing::get(State(state.clone()), headers.clone())
        .await
        .unwrap();
    let view = serde_json::to_value(view).unwrap();
    assert_eq!(view["payments"][0]["refunded_amount"], 1000);
    assert_eq!(view["subscription"]["status"], "active");
    assert_eq!(view["subscription"]["cancel_at_period_end"], false);
    assert_eq!(view["subscription"]["has_card"], true);
    let email = emails(&pool, user_id)
        .await
        .into_iter()
        .find(|email| email.0 == "refund")
        .unwrap();
    assert!(!email.2.contains("cancelled"));

    // It still renews.
    make_due(&pool, user_id).await;
    assert!(billing::charge_due(&state, None).await.unwrap());
    assert_eq!(toss.charges().len(), 2);

    // Giving back the rest of it, in the dashboard, changes nothing either.
    toss.cancel_in_dashboard(&order_id);
    assert_eq!(
        billing_sync::webhook(State(state.clone()), HeaderMap::new(), event).await,
        StatusCode::OK
    );
    let Json(view) = billing::get(State(state.clone()), headers.clone())
        .await
        .unwrap();
    let view = serde_json::to_value(view).unwrap();
    assert_eq!(view["subscription"]["cancel_at_period_end"], false);
    assert_eq!(view["subscription"]["has_card"], true);
}

#[sqlx::test(migrations = "./migrations")]
#[ignore = "requires disposable PostgreSQL; use mise run test-integration"]
async fn subscribing_after_a_refund_starts_a_new_period(pool: PgPool) {
    let (toss, base) = toss_stand_in().await;
    let state = billed_state(pool.clone(), &base);
    let (user_id, headers) = billed_customer(&pool).await;
    let view = subscribe(&state, &headers).await;
    let order_id = view["payments"][0]["order_id"].as_str().unwrap().to_owned();
    refund(&state, &headers, &order_id).await.unwrap();
    // The refunded period cannot simply be picked up again.
    assert_eq!(
        billing::resume(State(state.clone()), headers.clone())
            .await
            .err()
            .unwrap()
            .0,
        StatusCode::CONFLICT
    );

    let view = subscribe(&state, &headers).await;
    assert_eq!(view["subscription"]["status"], "active");
    assert_eq!(view["subscription"]["cancel_at_period_end"], false);
    assert_eq!(view["subscription"]["has_card"], true);
    assert_eq!(toss.charges().len(), 2);
    let (days, periods): (i32, i32) = sqlx::query_as(
        "SELECT ceil(extract(epoch FROM current_period_end-now())/86400)::int,periods_paid FROM subscriptions WHERE user_id=$1",
    )
    .bind(user_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!((28..=31).contains(&days), "{days}");
    assert_eq!(periods, 1);
}

#[sqlx::test(migrations = "./migrations")]
#[ignore = "requires disposable PostgreSQL; use mise run test-integration"]
async fn a_refund_tosspayments_refuses_is_withdrawn_and_renewals_resume(pool: PgPool) {
    let (toss, base) = toss_stand_in().await;
    let state = billed_state(pool.clone(), &base);
    let (user_id, headers) = billed_customer(&pool).await;
    let view = subscribe(&state, &headers).await;
    let order_id = view["payments"][0]["order_id"].as_str().unwrap().to_owned();
    toss.refuse_cancel
        .store(true, std::sync::atomic::Ordering::SeqCst);

    let view = refund(&state, &headers, &order_id).await.unwrap();
    assert_eq!(view["payments"][0]["status"], "paid");
    assert_eq!(view["payments"][0]["refund_pending"], false);
    assert_eq!(
        view["payments"][0]["refund_error"],
        "취소 할 수 없는 결제 입니다."
    );
    assert_eq!(view["payments"][0]["refundable"], true);
    assert_eq!(view["subscription"]["status"], "active");

    // Nothing holds the subscription back: it renews when due.
    make_due(&pool, user_id).await;
    assert!(billing::charge_due(&state, None).await.unwrap());
    assert_eq!(toss.charges().len(), 2);

    // And it can be asked for again.
    let view = refund(&state, &headers, &order_id).await.unwrap();
    assert_eq!(
        view["payments"]
            .as_array()
            .unwrap()
            .iter()
            .find(|payment| payment["order_id"] == order_id.as_str())
            .unwrap()["status"],
        "refunded"
    );
}

#[sqlx::test(migrations = "./migrations")]
#[ignore = "requires disposable PostgreSQL; use mise run test-integration"]
async fn a_payment_is_refundable_for_seven_days(pool: PgPool) {
    let (_toss, base) = toss_stand_in().await;
    let state = billed_state(pool.clone(), &base);
    let (_, headers) = billed_customer(&pool).await;
    let view = subscribe(&state, &headers).await;
    let order_id = view["payments"][0]["order_id"].as_str().unwrap().to_owned();
    sqlx::query(
        "UPDATE subscription_payments SET settled_at=now()-interval '8 days' WHERE order_id=$1",
    )
    .bind(&order_id)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        refund(&state, &headers, &order_id).await.unwrap_err(),
        StatusCode::CONFLICT
    );
    // Nobody else's payment is refundable through this account.
    let (_, other) = {
        let user_id = Uuid::new_v4();
        sqlx::query("INSERT INTO users(id,email,password_hash,email_verified_at) VALUES($1,'other@example.com','unused',now())")
            .bind(user_id).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO user_sessions(token_hash,user_id,expires_at) VALUES($1,$2,now()+interval '1 hour')").bind(token_hash("other-session")).bind(user_id).execute(&pool).await.unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            "eunha_space_customer_session=other-session"
                .parse()
                .unwrap(),
        );
        (user_id, headers)
    };
    assert_eq!(
        refund(&state, &other, &order_id).await.unwrap_err(),
        StatusCode::NOT_FOUND
    );
}

#[sqlx::test(migrations = "./migrations")]
#[ignore = "requires disposable PostgreSQL; use mise run test-integration"]
async fn a_refund_made_in_the_dashboard_is_learned_from_the_webhook(pool: PgPool) {
    let (toss, base) = toss_stand_in().await;
    let state = billed_state(pool.clone(), &base);
    let (user_id, headers) = billed_customer(&pool).await;
    let view = subscribe(&state, &headers).await;
    let id = ready_instance(&pool, user_id).await;
    let order_id = view["payments"][0]["order_id"].as_str().unwrap().to_owned();

    // The event alone changes nothing: it is only a reason to look.
    let event = |order_id: &str| {
        axum::body::Bytes::from(
            serde_json::json!({"eventType":"PAYMENT_STATUS_CHANGED","data":{"orderId":order_id,"status":"CANCELED"}})
                .to_string(),
        )
    };
    assert_eq!(
        billing_sync::webhook(State(state.clone()), HeaderMap::new(), event(&order_id)).await,
        StatusCode::OK
    );
    assert_eq!(subscription_status(&pool, user_id).await, "active");

    toss.cancel_in_dashboard(&order_id);
    assert_eq!(
        billing_sync::webhook(State(state.clone()), HeaderMap::new(), event(&order_id)).await,
        StatusCode::OK
    );
    // A refund made there is an operator's, and leaves everything running.
    assert_eq!(subscription_status(&pool, user_id).await, "active");
    assert_eq!(lifecycle(&pool, id).await.0, "ready");
    let renews: bool = sqlx::query_scalar(
        "SELECT billing_key IS NOT NULL AND NOT cancel_at_period_end FROM subscriptions WHERE user_id=$1",
    )
    .bind(user_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(renews);
    // Delivered again, it changes nothing more.
    assert_eq!(
        billing_sync::webhook(State(state.clone()), HeaderMap::new(), event(&order_id)).await,
        StatusCode::OK
    );
    assert_eq!(
        billing_sync::webhook(
            State(state.clone()),
            HeaderMap::new(),
            event("eunha-unknown")
        )
        .await,
        StatusCode::OK
    );
}

#[sqlx::test(migrations = "./migrations")]
#[ignore = "requires disposable PostgreSQL; use mise run test-integration"]
async fn a_card_removed_at_tosspayments_ends_with_the_period_unless_replaced(pool: PgPool) {
    let (_toss, base) = toss_stand_in().await;
    let state = billed_state(pool.clone(), &base);
    let (user_id, headers) = billed_customer(&pool).await;
    subscribe(&state, &headers).await;
    let deleted = axum::body::Bytes::from(
        serde_json::json!({"eventType":"BILLING_DELETED","createdAt":"2026-11-04T00:00:00","data":{"billingKey":"bk_never_shown","reason":"x"}})
            .to_string(),
    );
    assert_eq!(
        billing_sync::webhook(State(state.clone()), HeaderMap::new(), deleted).await,
        StatusCode::OK
    );
    let (status, has_key, ending): (String, bool, bool) = sqlx::query_as(
        "SELECT status,billing_key IS NOT NULL,cancel_at_period_end FROM subscriptions WHERE user_id=$1",
    )
    .bind(user_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!((status.as_str(), has_key, ending), ("active", false, true));

    // A new card keeps it going.
    let view = subscribe(&state, &headers).await;
    assert_eq!(view["subscription"]["cancel_at_period_end"], false);
    assert_eq!(view["subscription"]["has_card"], true);
}

#[sqlx::test(migrations = "./migrations")]
#[ignore = "requires disposable PostgreSQL; use mise run test-integration"]
async fn operators_see_billing_and_customers_do_not(pool: PgPool) {
    let (toss, base) = toss_stand_in().await;
    let state = billed_state(pool.clone(), &base);
    let (runtime, admin) = fixture(&pool).await;
    let (user_id, headers) = billed_customer(&pool).await;
    subscribe(&state, &headers).await;
    // A customer from before billing, running an instance unbilled.
    let grandfathered = Uuid::new_v4();
    sqlx::query("INSERT INTO users(id,email,password_hash,email_verified_at) VALUES($1,'early@example.com','unused',now())")
        .bind(grandfathered).execute(&pool).await.unwrap();
    let id = tenant(&pool, runtime).await;
    sqlx::query("UPDATE tenants SET user_id=$2,lifecycle_state='ready' WHERE id=$1")
        .bind(id)
        .bind(grandfathered)
        .execute(&pool)
        .await
        .unwrap();

    assert_eq!(
        billing_admin::overview(State(state.clone()), headers.clone())
            .await
            .err()
            .unwrap()
            .0,
        StatusCode::FORBIDDEN
    );
    let Json(overview) = billing_admin::overview(State(state.clone()), admin.clone())
        .await
        .unwrap();
    let overview = serde_json::to_value(overview).unwrap();
    assert!(!overview.to_string().contains("bk_never_shown"));
    assert_eq!(overview["summary"]["active"], 1);
    assert_eq!(overview["summary"]["monthly_revenue"], 9900);
    assert_eq!(overview["summary"]["unbilled"], 1);
    let accounts = overview["accounts"].as_array().unwrap();
    assert_eq!(accounts.len(), 2);
    let early = accounts
        .iter()
        .find(|account| account["email"] == "early@example.com")
        .unwrap();
    assert!(early["status"].is_null());
    assert_eq!(early["instances"], 1);

    // An operator refunds outside the customer's seven days.
    let Json(detail) = billing_admin::account(State(state.clone()), admin.clone(), Path(user_id))
        .await
        .unwrap();
    let detail = serde_json::to_value(detail).unwrap();
    let order_id = detail["payments"][0]["order_id"]
        .as_str()
        .unwrap()
        .to_owned();
    sqlx::query(
        "UPDATE subscription_payments SET settled_at=now()-interval '30 days' WHERE order_id=$1",
    )
    .bind(&order_id)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        billing_admin::refund(State(state.clone()), admin.clone(), Path(order_id.clone()))
            .await
            .unwrap(),
        StatusCode::NO_CONTENT
    );
    while billing_sync::refund_due(&state, None).await.unwrap() {}
    // The money goes back; the subscription goes on as it was.
    let (status, has_card, ending, refunded): (String, bool, bool, String) = sqlx::query_as(
        "SELECT s.status,s.billing_key IS NOT NULL,s.cancel_at_period_end,p.status FROM subscriptions s JOIN subscription_payments p ON p.user_id=s.user_id WHERE p.order_id=$1",
    )
    .bind(&order_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        (status.as_str(), has_card, ending, refunded.as_str()),
        ("active", true, false, "refunded")
    );
    assert_eq!(
        sqlx::query_scalar::<_, Option<Uuid>>(
            "SELECT refund_requested_by FROM subscription_payments WHERE order_id=$1"
        )
        .bind(&order_id)
        .fetch_one(&pool)
        .await
        .unwrap()
        .is_some(),
        true
    );
    drop(toss);
}

#[sqlx::test(migrations = "./migrations")]
#[ignore = "requires disposable PostgreSQL; use mise run test-integration"]
async fn an_exempt_account_runs_instances_and_keeps_them(pool: PgPool) {
    let (toss, base) = toss_stand_in().await;
    let state = billed_state(pool.clone(), &base);
    let (runtime, admin) = fixture(&pool).await;
    let (user_id, headers) = billed_customer(&pool).await;
    subscribe(&state, &headers).await;
    let id = tenant(&pool, runtime).await;
    sqlx::query("UPDATE tenants SET user_id=$2,lifecycle_state='ready' WHERE id=$1")
        .bind(id)
        .bind(user_id)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        billing_admin::exempt(
            State(state.clone()),
            admin.clone(),
            Path(user_id),
            Json(serde_json::from_value(serde_json::json!({"exempt": true})).unwrap()),
        )
        .await
        .unwrap(),
        StatusCode::NO_CONTENT
    );
    // Its subscription lapsing takes nothing offline.
    toss.answer(&[TossScript::Decline; 4]);
    for _ in 0..4 {
        make_due(&pool, user_id).await;
        assert!(billing::charge_due(&state, None).await.unwrap());
    }
    assert_eq!(subscription_status(&pool, user_id).await, "lapsed");
    assert_eq!(lifecycle(&pool, id).await.0, "ready");
    let mut tx = pool.begin().await.unwrap();
    billing::require_subscription(&state, &mut tx, user_id)
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    // An operator's retry needs a card on file, which an ended subscription
    // no longer has.
    assert_eq!(
        billing_admin::retry(State(state.clone()), admin.clone(), Path(user_id))
            .await
            .unwrap_err()
            .0,
        StatusCode::CONFLICT
    );
}

#[sqlx::test(migrations = "./migrations")]
#[ignore = "requires disposable PostgreSQL; use mise run test-integration"]
async fn an_operator_charges_a_declined_renewal_again(pool: PgPool) {
    let (toss, base) = toss_stand_in().await;
    let state = billed_state(pool.clone(), &base);
    let (_, admin) = fixture(&pool).await;
    let (user_id, headers) = billed_customer(&pool).await;
    subscribe(&state, &headers).await;
    toss.answer(&[TossScript::Decline]);
    make_due(&pool, user_id).await;
    assert!(billing::charge_due(&state, None).await.unwrap());
    assert_eq!(subscription_status(&pool, user_id).await, "past_due");
    assert_eq!(
        billing_admin::retry(State(state.clone()), admin.clone(), Path(user_id))
            .await
            .unwrap(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(subscription_status(&pool, user_id).await, "active");
    assert_eq!(toss.charges().len(), 3);
}

async fn emails(pool: &PgPool, user_id: Uuid) -> Vec<(String, String, String)> {
    sqlx::query_as(
        "SELECT kind,subject,body FROM billing_emails WHERE user_id=$1 ORDER BY created_at",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await
    .unwrap()
}

#[sqlx::test(migrations = "./migrations")]
#[ignore = "requires disposable PostgreSQL; use mise run test-integration"]
async fn every_paid_charge_gets_one_receipt(pool: PgPool) {
    let (toss, base) = toss_stand_in().await;
    let state = billed_state(pool.clone(), &base);
    let (user_id, headers) = billed_customer(&pool).await;
    let view = subscribe(&state, &headers).await;
    let order_id = view["payments"][0]["order_id"].as_str().unwrap().to_owned();
    let sent = emails(&pool, user_id).await;
    assert_eq!(sent.len(), 1);
    let (kind, subject, body) = &sent[0];
    assert_eq!(kind, "receipt");
    assert_eq!(subject, "Your eunha space receipt: ₩9,900");
    assert!(body.contains("We charged ₩9,900 to your 현대 card ending 123*"));
    assert!(body.contains(&order_id));
    assert!(body.contains("Receipt: https://receipt.example/r"));
    assert!(!body.contains("bk_never_shown"));

    // A renewal gets its own receipt; a declined one gets none.
    toss.answer(&[TossScript::Decline, TossScript::Approve]);
    make_due(&pool, user_id).await;
    billing::charge_due(&state, None).await.unwrap();
    assert_eq!(emails(&pool, user_id).await.len(), 1);
    make_due(&pool, user_id).await;
    billing::charge_due(&state, None).await.unwrap();
    let kinds: Vec<String> = emails(&pool, user_id)
        .await
        .into_iter()
        .map(|e| e.0)
        .collect();
    assert_eq!(kinds, vec!["receipt", "receipt"]);

    // Without control-plane SMTP nothing is sent, and nothing is lost.
    assert!(!billing_mail::send_due(&state).await.unwrap());
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM billing_emails WHERE sent_at IS NULL AND abandoned_at IS NULL"
        )
        .fetch_one(&pool)
        .await
        .unwrap(),
        2
    );
}

#[sqlx::test(migrations = "./migrations")]
#[ignore = "requires disposable PostgreSQL; use mise run test-integration"]
async fn a_renewal_is_announced_once_a_week_ahead(pool: PgPool) {
    let (_toss, base) = toss_stand_in().await;
    let state = billed_state(pool.clone(), &base);
    let (user_id, headers) = billed_customer(&pool).await;
    subscribe(&state, &headers).await;
    let set_end = |interval: &'static str| {
        let pool = pool.clone();
        async move {
            sqlx::query(&format!("UPDATE subscriptions SET current_period_end=now()+interval '{interval}',next_attempt_at=now()+interval '{interval}' WHERE user_id=$1"))
                .bind(user_id)
                .execute(&pool)
                .await
                .unwrap();
        }
    };
    // A month away: too early.
    assert_eq!(
        billing_mail::schedule_renewal_notices(&state)
            .await
            .unwrap(),
        0
    );
    // Five days away: announced, once.
    set_end("5 days").await;
    assert_eq!(
        billing_mail::schedule_renewal_notices(&state)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        billing_mail::schedule_renewal_notices(&state)
            .await
            .unwrap(),
        0
    );
    let notice = emails(&pool, user_id)
        .await
        .into_iter()
        .find(|email| email.0 == "renewal_notice")
        .unwrap();
    assert!(
        notice
            .1
            .starts_with("Your eunha space subscription renews on ")
    );
    assert!(
        notice
            .2
            .contains("we will charge ₩9,900 to your 현대 card ending 123*")
    );

    // A subscription that will not renew is not announced.
    set_end("6 days").await;
    sqlx::query("UPDATE subscriptions SET cancel_at_period_end=true WHERE user_id=$1")
        .bind(user_id)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        billing_mail::schedule_renewal_notices(&state)
            .await
            .unwrap(),
        0
    );
    // Nor is one renewing within the day.
    sqlx::query("UPDATE subscriptions SET cancel_at_period_end=false WHERE user_id=$1")
        .bind(user_id)
        .execute(&pool)
        .await
        .unwrap();
    set_end("12 hours").await;
    assert_eq!(
        billing_mail::schedule_renewal_notices(&state)
            .await
            .unwrap(),
        0
    );
}

#[sqlx::test(migrations = "./migrations")]
#[ignore = "requires disposable PostgreSQL; use mise run test-integration"]
async fn a_refund_is_confirmed_by_email(pool: PgPool) {
    let (_toss, base) = toss_stand_in().await;
    let state = billed_state(pool.clone(), &base);
    let (user_id, headers) = billed_customer(&pool).await;
    let view = subscribe(&state, &headers).await;
    let order_id = view["payments"][0]["order_id"].as_str().unwrap().to_owned();
    refund(&state, &headers, &order_id).await.unwrap();
    let refund = emails(&pool, user_id)
        .await
        .into_iter()
        .find(|email| email.0 == "refund")
        .unwrap();
    assert_eq!(refund.1, "Your eunha space refund: ₩9,900");
    assert!(refund.2.contains("Your subscription is cancelled"));
    // This customer has no instance to take offline.
    assert!(!refund.2.contains("offline"));
}

fn with_language(mut headers: HeaderMap, accept: &str) -> HeaderMap {
    headers.insert(header::ACCEPT_LANGUAGE, accept.parse().unwrap());
    headers
}

#[sqlx::test(migrations = "./migrations")]
#[ignore = "requires disposable PostgreSQL; use mise run test-integration"]
async fn the_language_follows_the_browser_until_the_customer_picks_one(pool: PgPool) {
    let state = state(pool.clone());
    let (user_id, headers) = billed_customer(&pool).await;
    let language = |headers: HeaderMap| {
        let state = state.clone();
        async move {
            let Json(view) = locale::get(State(state), headers).await.unwrap();
            serde_json::to_value(view).unwrap()
        }
    };
    // Signed out: what the browser asks for.
    assert_eq!(
        language(with_language(HeaderMap::new(), "ko-KR,ko;q=0.9,en;q=0.8")).await,
        serde_json::json!({"locale": "ko", "saved": false})
    );
    assert_eq!(
        language(HeaderMap::new()).await,
        serde_json::json!({"locale": "en", "saved": false})
    );
    // An account from before languages were kept takes its browser's.
    let Json(me) = customer_me(
        State(state.clone()),
        with_language(headers.clone(), "ko;q=0.9, en;q=0.5"),
    )
    .await
    .unwrap();
    assert_eq!(me["locale"], "ko");
    assert_eq!(
        language(with_language(headers.clone(), "en")).await,
        serde_json::json!({"locale": "ko", "saved": true})
    );
    // The customer's choice wins over the browser from then on.
    assert_eq!(
        locale::set(
            State(state.clone()),
            headers.clone(),
            Json(serde_json::from_value(serde_json::json!({"locale": "en"})).unwrap()),
        )
        .await
        .unwrap(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        language(with_language(headers.clone(), "ko")).await,
        serde_json::json!({"locale": "en", "saved": true})
    );
    assert_eq!(
        locale::set(
            State(state.clone()),
            headers.clone(),
            Json(serde_json::from_value(serde_json::json!({"locale": "fr"})).unwrap()),
        )
        .await
        .unwrap_err()
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        locale::set(
            State(state.clone()),
            HeaderMap::new(),
            Json(serde_json::from_value(serde_json::json!({"locale": "ko"})).unwrap()),
        )
        .await
        .unwrap_err()
        .0,
        StatusCode::UNAUTHORIZED
    );
    let _ = user_id;
}

#[sqlx::test(migrations = "./migrations")]
#[ignore = "requires disposable PostgreSQL; use mise run test-integration"]
async fn a_korean_customer_gets_billing_email_in_korean(pool: PgPool) {
    let (_toss, base) = toss_stand_in().await;
    let state = billed_state(pool.clone(), &base);
    let (user_id, headers) = billed_customer(&pool).await;
    sqlx::query("UPDATE users SET locale='ko' WHERE id=$1")
        .bind(user_id)
        .execute(&pool)
        .await
        .unwrap();
    let view = subscribe(&state, &headers).await;
    let order_id = view["payments"][0]["order_id"].as_str().unwrap().to_owned();
    let (language, subject, body): (String, String, String) = sqlx::query_as(
        "SELECT locale,subject,body FROM billing_emails WHERE user_id=$1 AND kind='receipt'",
    )
    .bind(user_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(language, "ko");
    assert_eq!(subject, "은하공간 결제 영수증: ₩9,900");
    assert!(body.starts_with("은하공간을 이용해 주셔서 감사합니다."));
    assert!(body.contains("₩9,900을 현대 카드(끝자리 123*)로 결제했습니다."));
    assert!(body.contains(&format!("주문 번호: {order_id}")));

    refund(&state, &headers, &order_id).await.unwrap();
    let subject: String =
        sqlx::query_scalar("SELECT subject FROM billing_emails WHERE user_id=$1 AND kind='refund'")
            .bind(user_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(subject, "은하공간 환불 안내: ₩9,900");
}

async fn plan(pool: &PgPool, amount: i64, instance_limit: Option<i64>) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO plans(name_en,name_ko,amount,instance_limit) VALUES($1,$1,$2,$3) RETURNING id",
    )
    .bind(format!("plan {amount}"))
    .bind(amount)
    .bind(instance_limit)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn change_plan(
    state: &Shared,
    headers: &HeaderMap,
    plan_id: Uuid,
) -> Result<serde_json::Value, StatusCode> {
    billing::change_plan(
        State(state.clone()),
        headers.clone(),
        Json(serde_json::from_value(serde_json::json!({ "plan_id": plan_id })).unwrap()),
    )
    .await
    .map(|Json(view)| serde_json::to_value(view).unwrap())
    .map_err(|error| error.0)
}

/// The subscription's plan, amount, and the plan its next renewal moves to.
async fn on_plan(pool: &PgPool, user_id: Uuid) -> (Option<Uuid>, i64, Option<Uuid>) {
    sqlx::query_as("SELECT plan_id,amount,pending_plan_id FROM subscriptions WHERE user_id=$1")
        .bind(user_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[test]
fn an_upgrade_costs_the_difference_for_what_is_left_of_the_period() {
    let start = OffsetDateTime::from_unix_timestamp(1_000_000_000).unwrap();
    let end = start + time::Duration::days(30);
    let charge = |now| billing::upgrade_charge(19900, 9900, start, end, now);
    assert_eq!(charge(start), 10000);
    assert_eq!(charge(start + time::Duration::days(15)), 5000);
    // Too little to put through a card, or nothing at all.
    assert_eq!(charge(end - time::Duration::hours(1)), 0);
    assert_eq!(charge(end + time::Duration::days(1)), 0);
}

#[sqlx::test(migrations = "./migrations")]
#[ignore = "requires disposable PostgreSQL; use mise run test-integration"]
async fn an_upgrade_is_charged_now_and_keeps_the_renewal_date(pool: PgPool) {
    let (toss, base) = toss_stand_in().await;
    let state = billed_state(pool.clone(), &base);
    let (user_id, headers) = billed_customer(&pool).await;
    subscribe(&state, &headers).await;
    let (first_plan, ..) = on_plan(&pool, user_id).await;
    assert!(first_plan.is_some());
    let big = plan(&pool, 19900, None).await;
    let period_end = |pool: PgPool| async move {
        sqlx::query_scalar::<_, OffsetDateTime>(
            "SELECT current_period_end FROM subscriptions WHERE user_id=$1",
        )
        .bind(user_id)
        .fetch_one(&pool)
        .await
        .unwrap()
    };
    let renews_on = period_end(pool.clone()).await;

    let view = change_plan(&state, &headers, big).await.unwrap();
    assert_eq!(on_plan(&pool, user_id).await, (Some(big), 19900, None));
    assert_eq!(view["payments"][0]["kind"], "upgrade");
    assert_eq!(view["payments"][0]["status"], "paid");
    let charged = view["payments"][0]["amount"].as_i64().unwrap();
    assert!((9990..=10000).contains(&charged), "charged {charged}");
    assert_eq!(toss.charges().len(), 2);
    assert_eq!(period_end(pool.clone()).await, renews_on);
    assert_eq!(
        view["subscription"]["next_attempt_at"],
        view["subscription"]["current_period_end"]
    );

    // The next renewal is at the new price.
    make_due(&pool, user_id).await;
    assert!(billing::charge_due(&state, None).await.unwrap());
    let amount: i64 = sqlx::query_scalar("SELECT amount FROM subscription_payments WHERE user_id=$1 AND kind='renewal' ORDER BY requested_at DESC LIMIT 1")
        .bind(user_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(amount, 19900);
}

#[sqlx::test(migrations = "./migrations")]
#[ignore = "requires disposable PostgreSQL; use mise run test-integration"]
async fn a_declined_upgrade_changes_nothing(pool: PgPool) {
    let (toss, base) = toss_stand_in().await;
    let state = billed_state(pool.clone(), &base);
    let (user_id, headers) = billed_customer(&pool).await;
    subscribe(&state, &headers).await;
    let before = on_plan(&pool, user_id).await;
    let big = plan(&pool, 19900, None).await;

    toss.answer(&[TossScript::Decline]);
    assert_eq!(
        change_plan(&state, &headers, big).await.unwrap_err(),
        StatusCode::PAYMENT_REQUIRED
    );
    assert_eq!(on_plan(&pool, user_id).await, before);
    assert_eq!(subscription_status(&pool, user_id).await, "active");
    let (failed, waits): (i32, bool) = sqlx::query_as(
        "SELECT failed_attempts,next_attempt_at=current_period_end FROM subscriptions WHERE user_id=$1",
    )
    .bind(user_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!((failed, waits), (0, true));
    assert!(!billing::charge_due(&state, None).await.unwrap());
}

#[sqlx::test(migrations = "./migrations")]
#[ignore = "requires disposable PostgreSQL; use mise run test-integration"]
async fn a_downgrade_waits_for_the_renewal_and_can_be_taken_back(pool: PgPool) {
    let (toss, base) = toss_stand_in().await;
    let state = billed_state(pool.clone(), &base);
    let (user_id, headers) = billed_customer(&pool).await;
    subscribe(&state, &headers).await;
    let (first_plan, ..) = on_plan(&pool, user_id).await;
    let small = plan(&pool, 4900, Some(1)).await;

    change_plan(&state, &headers, small).await.unwrap();
    assert_eq!(
        on_plan(&pool, user_id).await,
        (first_plan, 9900, Some(small))
    );
    assert_eq!(toss.charges().len(), 1);
    change_plan(&state, &headers, first_plan.unwrap())
        .await
        .unwrap();
    assert_eq!(on_plan(&pool, user_id).await, (first_plan, 9900, None));

    change_plan(&state, &headers, small).await.unwrap();
    make_due(&pool, user_id).await;
    assert!(billing::charge_due(&state, None).await.unwrap());
    assert_eq!(on_plan(&pool, user_id).await, (Some(small), 4900, None));
    let amount: i64 = sqlx::query_scalar("SELECT amount FROM subscription_payments WHERE user_id=$1 ORDER BY requested_at DESC LIMIT 1")
        .bind(user_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(amount, 4900);
}

#[sqlx::test(migrations = "./migrations")]
#[ignore = "requires disposable PostgreSQL; use mise run test-integration"]
async fn visitors_see_the_plans_on_offer_only_when_billing_is_on(pool: PgPool) {
    plan(&pool, 19900, Some(5)).await;
    plan(&pool, 9900, None).await;
    let retired = plan(&pool, 4900, Some(1)).await;
    sqlx::query("UPDATE plans SET active=false WHERE id=$1")
        .bind(retired)
        .execute(&pool)
        .await
        .unwrap();

    let Json(plans) = billing::public_plans(State(state(pool.clone())))
        .await
        .unwrap();
    assert_eq!(serde_json::to_value(plans).unwrap(), serde_json::json!([]));

    let (_toss, base) = toss_stand_in().await;
    sqlx::query("UPDATE plans SET user_limit=50,media_gb=10,database_gb=5,monthly_activities=100000 WHERE amount=9900")
        .execute(&pool)
        .await
        .unwrap();
    let Json(plans) = billing::public_plans(State(billed_state(pool, &base)))
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(plans).unwrap(),
        serde_json::json!([
            {"name_en":"plan 9900","name_ko":"plan 9900","amount":9900,"instance_limit":null,"user_limit":50,"media_gb":10,"database_gb":5,"monthly_activities":100000,"polar_amount":null,"polar_currency":null},
            {"name_en":"plan 19900","name_ko":"plan 19900","amount":19900,"instance_limit":5,"user_limit":null,"media_gb":null,"database_gb":null,"monthly_activities":null,"polar_amount":null,"polar_currency":null},
        ])
    );
}

#[sqlx::test(migrations = "./migrations")]
#[ignore = "requires disposable PostgreSQL; use mise run test-integration"]
async fn a_plan_limits_instances_and_a_downgrade_must_fit(pool: PgPool) {
    let (_toss, base) = toss_stand_in().await;
    let state = billed_state(pool.clone(), &base);
    let (user_id, headers) = billed_customer(&pool).await;
    subscribe(&state, &headers).await;
    ready_instance(&pool, user_id).await;
    let small = plan(&pool, 4900, Some(0)).await;
    assert_eq!(
        change_plan(&state, &headers, small).await.unwrap_err(),
        StatusCode::CONFLICT
    );

    let limit = |pool: PgPool| async move {
        let mut tx = pool.begin().await.unwrap();
        customer_instance_limit(&mut tx, user_id).await.unwrap()
    };
    // The account's own limit, while the plan sets none.
    assert_eq!(limit(pool.clone()).await, Some(2));
    let big = plan(&pool, 19900, Some(10)).await;
    change_plan(&state, &headers, big).await.unwrap();
    assert_eq!(limit(pool.clone()).await, Some(10));
}

#[sqlx::test(migrations = "./migrations")]
#[ignore = "requires disposable PostgreSQL; use mise run test-integration"]
async fn a_customer_refunding_an_upgrade_ends_the_subscription_and_an_operator_does_not(
    pool: PgPool,
) {
    let (_toss, base) = toss_stand_in().await;
    let state = billed_state(pool.clone(), &base);
    let (runtime, admin) = fixture(&pool).await;
    let (user_id, headers) = billed_customer(&pool).await;
    subscribe(&state, &headers).await;
    let id = tenant(&pool, runtime).await;
    sqlx::query("UPDATE tenants SET user_id=$2,lifecycle_state='ready' WHERE id=$1")
        .bind(id)
        .bind(user_id)
        .execute(&pool)
        .await
        .unwrap();
    let big = plan(&pool, 19900, None).await;
    let view = change_plan(&state, &headers, big).await.unwrap();
    let upgrade = view["payments"][0]["order_id"].as_str().unwrap().to_owned();

    // An operator gives it back: the plan and the instance stay as they are.
    billing_admin::refund(State(state.clone()), admin, Path(upgrade.clone()))
        .await
        .unwrap();
    while billing_sync::refund_due(&state, None).await.unwrap() {}
    assert_eq!(on_plan(&pool, user_id).await, (Some(big), 19900, None));
    assert_eq!(subscription_status(&pool, user_id).await, "active");
    assert_eq!(lifecycle(&pool, id).await.0, "ready");

    // The customer's own refund of another upgrade ends it all.
    let bigger = plan(&pool, 29900, None).await;
    let view = change_plan(&state, &headers, bigger).await.unwrap();
    let upgrade = view["payments"][0]["order_id"].as_str().unwrap().to_owned();
    let view = refund(&state, &headers, &upgrade).await.unwrap();
    assert_eq!(view["subscription"]["status"], "canceled");
    assert_eq!(lifecycle(&pool, id).await.0, "deprovisioning");
}

#[sqlx::test(migrations = "./migrations")]
#[ignore = "requires disposable PostgreSQL; use mise run test-integration"]
async fn refunding_the_period_also_refunds_its_upgrade(pool: PgPool) {
    let (_toss, base) = toss_stand_in().await;
    let state = billed_state(pool.clone(), &base);
    let (user_id, headers) = billed_customer(&pool).await;
    let view = subscribe(&state, &headers).await;
    let first = view["payments"][0]["order_id"].as_str().unwrap().to_owned();
    let big = plan(&pool, 19900, None).await;
    change_plan(&state, &headers, big).await.unwrap();

    let view = refund(&state, &headers, &first).await.unwrap();
    assert_eq!(view["subscription"]["status"], "canceled");
    while billing_sync::refund_due(&state, None).await.unwrap() {}
    let unrefunded: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM subscription_payments WHERE user_id=$1 AND status<>'refunded'",
    )
    .bind(user_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(unrefunded, 0);
}

#[sqlx::test(migrations = "./migrations")]
#[ignore = "requires disposable PostgreSQL; use mise run test-integration"]
async fn checkout_subscribes_to_the_plan_chosen(pool: PgPool) {
    let (_toss, base) = toss_stand_in().await;
    let state = billed_state(pool.clone(), &base);
    let (user_id, headers) = billed_customer(&pool).await;
    assert_eq!(
        billing::checkout(State(state.clone()), headers.clone(), None)
            .await
            .err()
            .unwrap()
            .0,
        StatusCode::CONFLICT
    );

    let chosen = plan(&pool, 19900, None).await;
    let Json(checkout) = billing::checkout(
        State(state.clone()),
        headers.clone(),
        Some(Json(
            serde_json::from_value(serde_json::json!({ "plan_id": chosen })).unwrap(),
        )),
    )
    .await
    .unwrap();
    let checkout = serde_json::to_value(checkout).unwrap();
    let Json(view) = billing::authorize(
        State(state.clone()),
        headers.clone(),
        Json(
            serde_json::from_value(
                serde_json::json!({"auth_key":"auth","customer_key":checkout["customer_key"]}),
            )
            .unwrap(),
        ),
    )
    .await
    .unwrap();
    let view = serde_json::to_value(view).unwrap();
    assert_eq!(view["payments"][0]["amount"], 19900);
    assert_eq!(on_plan(&pool, user_id).await, (Some(chosen), 19900, None));
}

#[sqlx::test(migrations = "./migrations")]
#[ignore = "requires disposable PostgreSQL; use mise run test-integration"]
async fn the_migration_puts_running_subscriptions_on_a_plan_at_their_price(pool: PgPool) {
    let (user_id, _) = billed_customer(&pool).await;
    sqlx::query(
        "INSERT INTO subscriptions(user_id,customer_key,amount,status) VALUES($1,$2,5900,'active')",
    )
    .bind(user_id)
    .bind(Uuid::new_v4())
    .execute(&pool)
    .await
    .unwrap();
    let migration = sqlx::migrate!("./migrations")
        .iter()
        .find(|m| m.version == 65)
        .unwrap()
        .sql
        .to_string();
    sqlx::raw_sql(&migration).execute(&pool).await.unwrap();
    let (plan_id, amount, _) = on_plan(&pool, user_id).await;
    assert_eq!(amount, 5900);
    let plan_amount: i64 = sqlx::query_scalar("SELECT amount FROM plans WHERE id=$1 AND active")
        .bind(plan_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(plan_amount, 5900);
}

#[sqlx::test(migrations = "./migrations")]
#[ignore = "requires disposable PostgreSQL; use mise run test-integration"]
async fn an_administrator_signs_in_as_a_customer_and_comes_back(pool: PgPool) {
    let (_, admin) = fixture(&pool).await;
    let (customer_id, _) = billed_customer(&pool).await;
    let shared = state(pool.clone());
    // Set-Cookie values, by cookie name.
    fn cookies(response: &Response) -> HashMap<String, String> {
        response
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .map(|value| {
                let value = value.to_str().unwrap();
                let (pair, _) = value.split_once(';').unwrap();
                let (name, token) = pair.split_once('=').unwrap();
                (name.to_owned(), token.to_owned())
            })
            .collect()
    }
    // The console lists every account, apart from billing.
    let Json(users) = users::list(State(shared.clone()), admin.clone())
        .await
        .unwrap();
    let users = serde_json::to_value(users).unwrap();
    assert_eq!(users.as_array().unwrap().len(), 2);
    assert!(users.to_string().contains("customer@example.com"));
    let started = masquerade::start(State(shared.clone()), admin.clone(), Path(customer_id))
        .await
        .unwrap();
    let set = cookies(&started);
    assert_eq!(set["eunha_space_admin_session"], "test-session");
    let mut masked = HeaderMap::new();
    masked.insert(
        header::COOKIE,
        format!(
            "eunha_space_customer_session={}; eunha_space_admin_session=test-session",
            set["eunha_space_customer_session"]
        )
        .parse()
        .unwrap(),
    );
    // The customer's pages show the customer, and who is looking.
    let Json(me) = customer_me(State(shared.clone()), masked.clone())
        .await
        .unwrap();
    assert_eq!(me["email"], "customer@example.com");
    assert_eq!(me["masquerading_admin"], "admin@example.com");
    // The console is out of reach, and so is masquerading further.
    assert_eq!(
        admin_user(&masked, &pool).await.unwrap_err().0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        users::list(State(shared.clone()), masked.clone())
            .await
            .unwrap_err()
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        masquerade::start(State(shared.clone()), masked.clone(), Path(customer_id))
            .await
            .unwrap_err()
            .0,
        StatusCode::FORBIDDEN
    );
    // Another administrator cannot be signed in as.
    let other_admin = Uuid::new_v4();
    sqlx::query("INSERT INTO users(id,email,password_hash,email_verified_at,is_admin) VALUES($1,'other-admin@example.com','unused',now(),true)")
        .bind(other_admin).execute(&pool).await.unwrap();
    assert_eq!(
        masquerade::start(State(shared.clone()), admin.clone(), Path(other_admin))
            .await
            .unwrap_err()
            .0,
        StatusCode::FORBIDDEN
    );
    // The password and billing are the customer's alone.
    let app = Router::new()
        .route("/customer/password", post(customer_set_password))
        .route("/customer/billing/cancel", post(billing::cancel))
        .route_layer(axum::middleware::from_fn_with_state(
            shared.clone(),
            masquerade::guard,
        ))
        .with_state(shared.clone());
    for path in ["/customer/password", "/customer/billing/cancel"] {
        let mut request = axum::http::Request::post(path)
            .body(axum::body::Body::from(
                r#"{"password":"a sufficiently long password"}"#,
            ))
            .unwrap();
        *request.headers_mut() = masked.clone();
        request
            .headers_mut()
            .insert(header::CONTENT_TYPE, "application/json".parse().unwrap());
        let response = tower::ServiceExt::oneshot(app.clone(), request)
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{path}");
    }
    // The language the administrator reads in is not chosen for the customer.
    locale::set(
        State(shared.clone()),
        masked.clone(),
        Json(serde_json::from_value(serde_json::json!({"locale":"ko"})).unwrap()),
    )
    .await
    .unwrap();
    assert!(
        sqlx::query_scalar::<_, Option<String>>("SELECT locale FROM users WHERE id=$1")
            .bind(customer_id)
            .fetch_one(&pool)
            .await
            .unwrap()
            != Some("ko".into())
    );
    assert_eq!(
        sqlx::query_scalar::<_, serde_json::Value>(
            "SELECT detail FROM operations WHERE kind='masquerade'"
        )
        .fetch_one(&pool)
        .await
        .unwrap()["email"],
        "customer@example.com"
    );
    // Signing out returns to the administrator's own session.
    let ended = customer_logout(State(shared.clone()), masked.clone())
        .await
        .unwrap();
    let set = cookies(&ended);
    assert_eq!(set["eunha_space_customer_session"], "test-session");
    assert_eq!(set["eunha_space_admin_session"], "");
    assert_eq!(
        customer(&masked, &pool).await.unwrap_err().0,
        StatusCode::UNAUTHORIZED
    );
    assert!(admin_user(&admin, &pool).await.is_ok());
    // A masquerade ends with its administrator's role.
    let started = masquerade::start(State(shared.clone()), admin.clone(), Path(customer_id))
        .await
        .unwrap();
    let mut masked = HeaderMap::new();
    masked.insert(
        header::COOKIE,
        format!(
            "eunha_space_customer_session={}",
            cookies(&started)["eunha_space_customer_session"]
        )
        .parse()
        .unwrap(),
    );
    sqlx::query("UPDATE users SET is_admin=false WHERE email='admin@example.com'")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        customer(&masked, &pool).await.unwrap_err().0,
        StatusCode::UNAUTHORIZED
    );
}

/// A second runtime on the fixture's instance, in `channel`.
async fn second_runtime(pool: &PgPool, first: Uuid, name: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO runtimes(id,name,instance_id,tenant_directory,service_label,binary_path,bind_address,blue_bind_address,green_bind_address,origin_hostname,channel_id) SELECT $1,$2,instance_id,'/tmp/'||$2,$2||'.runtime','/tmp/eunha','127.0.0.1:'||(60100+(SELECT count(*) FROM runtimes)),'127.0.0.1:'||(61100+2*(SELECT count(*) FROM runtimes)),'127.0.0.1:'||(61101+2*(SELECT count(*) FROM runtimes)),$2||'.origin.example',channel_id FROM runtimes WHERE id=$3")
        .bind(id).bind(name).bind(first).execute(pool).await.unwrap();
    id
}

async fn channel_id(pool: &PgPool, name: &str) -> Uuid {
    sqlx::query_scalar("SELECT id FROM release_channels WHERE name=$1")
        .bind(name)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn ready_tenant(pool: &PgPool, runtime: Uuid) -> Uuid {
    let id = tenant(pool, runtime).await;
    sqlx::query("UPDATE tenants SET lifecycle_state='ready' WHERE id=$1")
        .bind(id)
        .execute(pool)
        .await
        .unwrap();
    id
}

/// Record where `head` stands against `base`, as an answer from eunha's
/// commit graph would be kept, so that no test asks GitHub.
async fn ordered(pool: &PgPool, base: &str, head: &str, status: &str) {
    sqlx::query("INSERT INTO eunha_release_order(base,head,status) VALUES($1,$2,$3)")
        .bind(base)
        .bind(head)
        .bind(status)
        .execute(pool)
        .await
        .unwrap();
}

async fn deployed(pool: &PgPool, runtime: Uuid, sha: &str) {
    sqlx::query("INSERT INTO eunha_releases(id,git_sha,artifact_path,build_status) VALUES($1,$2,'/tmp','ready') ON CONFLICT DO NOTHING")
        .bind(Uuid::new_v4()).bind(sha).execute(pool).await.unwrap();
    sqlx::query("UPDATE runtimes SET deployed_ref=$2 WHERE id=$1")
        .bind(runtime)
        .bind(sha)
        .execute(pool)
        .await
        .unwrap();
}

#[sqlx::test(migrations = "./migrations")]
#[ignore = "requires disposable PostgreSQL; use mise run test-integration"]
async fn existing_runtimes_land_in_stable_and_a_rollout_starts_with_canary(pool: PgPool) {
    let (stable, _) = fixture(&pool).await;
    // The fixture inserts after the migration; put it where the migration
    // puts every runtime it finds.
    sqlx::query(
        "UPDATE runtimes SET channel_id=(SELECT id FROM release_channels WHERE name='stable')",
    )
    .execute(&pool)
    .await
    .unwrap();
    let canary = second_runtime(&pool, stable, "canary-a").await;
    sqlx::query("UPDATE runtimes SET auto_deploy_enabled=true")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE runtimes SET channel_id=$2 WHERE id=$1")
        .bind(canary)
        .bind(channel_id(&pool, "canary").await)
        .execute(&pool)
        .await
        .unwrap();
    ready_tenant(&pool, stable).await;
    ready_tenant(&pool, canary).await;

    let first = "a".repeat(40);
    assert!(rollout::begin(&pool, &first).await.unwrap());
    assert!(!rollout::begin(&pool, &first).await.unwrap());
    let active = rollout::active(&pool).await.unwrap().unwrap();
    let channels = rollout::channels(&pool, &active).await.unwrap();
    assert_eq!(
        channels.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
        ["canary", "stable"]
    );
    assert_eq!(
        rollout::plan(&channels, false, OffsetDateTime::now_utc()),
        rollout::Action::Release {
            channel_id: channels[0].id,
            runtime_id: canary
        }
    );

    // A pause is a freeze: the next build waits paused too.
    sqlx::query("UPDATE release_rollouts SET status='paused' WHERE id=$1")
        .bind(active.id)
        .execute(&pool)
        .await
        .unwrap();
    let second = "b".repeat(40);
    assert!(rollout::begin(&pool, &second).await.unwrap());
    let statuses: Vec<(String, String)> =
        sqlx::query_as("SELECT git_sha,status FROM release_rollouts ORDER BY created_at")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(
        statuses,
        [(first, "superseded".into()), (second, "paused".into())]
    );
}

#[sqlx::test(migrations = "./migrations")]
#[ignore = "requires disposable PostgreSQL; use mise run test-integration"]
async fn a_move_is_admitted_only_where_it_is_safe_and_holds_its_runtimes(pool: PgPool) {
    let (from, _) = fixture(&pool).await;
    let to = second_runtime(&pool, from, "next").await;
    let shared = state(pool.clone());
    let moving = ready_tenant(&pool, from).await;

    // The last tenant cannot leave: eunha refuses an empty tenants directory.
    deployed(&pool, from, &"a".repeat(40)).await;
    deployed(&pool, to, &"a".repeat(40)).await;
    let refused = tenant_move::begin(&shared, moving, to, None, "127.0.0.1")
        .await
        .unwrap_err();
    assert_eq!(refused.0, StatusCode::CONFLICT);
    assert!(refused.1.contains("last tenant"), "{}", refused.1);
    ready_tenant(&pool, from).await;

    // Only runtimes this control plane manages.
    let refused = tenant_move::begin(&shared, moving, to, None, "")
        .await
        .unwrap_err();
    assert!(
        refused.1.contains("instance this control plane manages"),
        "{}",
        refused.1
    );

    // Not to an older release than the one that migrated its database.
    deployed(&pool, to, &"0".repeat(40)).await;
    let refused = tenant_move::begin(&shared, moving, to, None, "127.0.0.1")
        .await
        .unwrap_err();
    assert!(refused.1.contains("is not known"), "{}", refused.1);
    ordered(&pool, &"0".repeat(40), &"a".repeat(40), "ahead").await;
    let refused = tenant_move::begin(&shared, moving, to, None, "127.0.0.1")
        .await
        .unwrap_err();
    assert!(refused.1.contains("older release"), "{}", refused.1);
    deployed(&pool, to, &"1".repeat(40)).await;
    ordered(&pool, &"a".repeat(40), &"1".repeat(40), "diverged").await;
    let refused = tenant_move::begin(&shared, moving, to, None, "127.0.0.1")
        .await
        .unwrap_err();
    assert!(refused.1.contains("different histories"), "{}", refused.1);
    deployed(&pool, to, &"a".repeat(40)).await;

    // An existing domain is routed by infrastructure, which a move cannot change.
    sqlx::query("INSERT INTO tenant_domains(id,tenant_id,hostname,kind,status,is_primary,cname_target) VALUES($1,$2,$3,'legacy','active',true,'x')")
        .bind(Uuid::new_v4()).bind(moving).bind(format!("legacy-{}.example", moving.simple())).execute(&pool).await.unwrap();
    let refused = tenant_move::begin(&shared, moving, to, None, "127.0.0.1")
        .await
        .unwrap_err();
    assert!(refused.1.contains("existing domain"), "{}", refused.1);
    sqlx::query("DELETE FROM tenant_domains WHERE tenant_id=$1")
        .bind(moving)
        .execute(&pool)
        .await
        .unwrap();

    let (move_id, _) = tenant_move::begin(&shared, moving, to, None, "127.0.0.1")
        .await
        .unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT kind FROM lifecycle_jobs WHERE tenant_id=$1 AND finished_at IS NULL"
        )
        .bind(moving)
        .fetch_one(&pool)
        .await
        .unwrap(),
        "runtime_move"
    );
    // Once at a time.
    let refused = tenant_move::begin(&shared, moving, to, None, "127.0.0.1")
        .await
        .unwrap_err();
    assert_eq!(refused.0, StatusCode::CONFLICT);
    // Neither runtime is released while it is open.
    ready_tenant(&pool, to).await;
    for runtime in [from, to] {
        let refused = queue_runtime_release(shared.clone(), runtime, None, "manual", None, None)
            .await
            .unwrap_err();
        assert_eq!(refused.0, StatusCode::CONFLICT);
        assert!(refused.1.contains("moving"), "{}", refused.1);
    }
    // A move that failed after the switch stays open; one finished does not.
    sqlx::query("UPDATE tenant_moves SET status='failed' WHERE id=$1")
        .bind(move_id)
        .execute(&pool)
        .await
        .unwrap();
    let mut tx = pool.begin().await.unwrap();
    assert!(tenant_move::holds_runtime(&mut tx, from).await.unwrap());
    tx.rollback().await.unwrap();
    sqlx::query("UPDATE tenant_moves SET finished_at=now() WHERE id=$1")
        .bind(move_id)
        .execute(&pool)
        .await
        .unwrap();
    let mut tx = pool.begin().await.unwrap();
    assert!(!tenant_move::holds_runtime(&mut tx, from).await.unwrap());
    tx.rollback().await.unwrap();
}

#[sqlx::test(migrations = "./migrations")]
#[ignore = "requires disposable PostgreSQL; use mise run test-integration"]
async fn channels_keep_their_order_and_only_empty_ones_go(pool: PgPool) {
    let (runtime, headers) = fixture(&pool).await;
    let shared = state(pool.clone());
    let (status, Json(created)) = rollout::create_channel(
        State(shared.clone()),
        headers.clone(),
        Json(
            serde_json::from_value(serde_json::json!({"name": "Early", "soak_minutes": 30}))
                .unwrap(),
        ),
    )
    .await
    .unwrap();
    assert_eq!(status, StatusCode::CREATED);
    let early: Uuid = serde_json::from_value(created["id"].clone()).unwrap();
    let order = |pool: PgPool| async move {
        sqlx::query_scalar::<_, String>("SELECT name FROM release_channels ORDER BY position")
            .fetch_all(&pool)
            .await
            .unwrap()
    };
    assert_eq!(order(pool.clone()).await, ["canary", "stable", "early"]);
    rollout::reorder_channel(
        State(shared.clone()),
        headers.clone(),
        Path(early),
        Json(serde_json::from_value(serde_json::json!({"direction": "earlier"})).unwrap()),
    )
    .await
    .unwrap();
    assert_eq!(order(pool.clone()).await, ["canary", "early", "stable"]);

    rollout::assign_runtime(
        State(shared.clone()),
        headers.clone(),
        Path(runtime),
        Json(serde_json::from_value(serde_json::json!({"channel_id": early})).unwrap()),
    )
    .await
    .unwrap();
    let refused = rollout::delete_channel(State(shared.clone()), headers.clone(), Path(early))
        .await
        .unwrap_err();
    assert_eq!(refused.0, StatusCode::CONFLICT);
    let duplicate = rollout::create_channel(
        State(shared.clone()),
        headers.clone(),
        Json(
            serde_json::from_value(serde_json::json!({"name": "early", "soak_minutes": 0}))
                .unwrap(),
        ),
    )
    .await
    .unwrap_err();
    assert_eq!(duplicate.0, StatusCode::CONFLICT);
    rollout::assign_runtime(
        State(shared.clone()),
        headers.clone(),
        Path(runtime),
        Json(serde_json::from_value(serde_json::json!({"channel_id": null})).unwrap()),
    )
    .await
    .unwrap();
    rollout::delete_channel(State(shared), headers, Path(early))
        .await
        .unwrap();
    assert_eq!(order(pool).await, ["canary", "stable"]);
}

#[sqlx::test(migrations = "./migrations")]
#[ignore = "requires disposable PostgreSQL; use mise run test-integration"]
async fn a_rollout_never_takes_a_runtime_back_to_an_older_build(pool: PgPool) {
    let (runtime, _) = fixture(&pool).await;
    sqlx::query("UPDATE runtimes SET channel_id=(SELECT id FROM release_channels WHERE name='stable'),auto_deploy_enabled=true")
        .execute(&pool).await.unwrap();
    ready_tenant(&pool, runtime).await;
    let rolling = "a".repeat(40);
    deployed(&pool, runtime, &rolling).await;
    rollout::begin(&pool, &rolling).await.unwrap();
    // An operator deploys a newer build by hand.
    let newer = "c".repeat(40);
    deployed(&pool, runtime, &newer).await;
    let active = rollout::active(&pool).await.unwrap().unwrap();
    // Until the graph has said which is newer, the runtime is held.
    let channels = rollout::channels(&pool, &active).await.unwrap();
    assert!(!channels[1].runtimes[0].on_release);
    let held = channels[1].runtimes[0].held.clone().unwrap();
    assert!(held.contains("is not known"), "{held}");
    ordered(&pool, &rolling, &newer, "ahead").await;
    let channels = rollout::channels(&pool, &active).await.unwrap();
    assert!(channels[1].runtimes[0].on_release);
    // An older one is still taken forward.
    let older = "0".repeat(40);
    deployed(&pool, runtime, &older).await;
    ordered(&pool, &older, &rolling, "ahead").await;
    let channels = rollout::channels(&pool, &active).await.unwrap();
    assert!(!channels[1].runtimes[0].on_release);
    assert_eq!(channels[1].runtimes[0].held, None);
}

/// A tenant being set up at `domain`, connected, with handles on `handle`: a
/// `.invalid` domain resolves nowhere, so its WebFinger never redirects.
async fn handle_domain_tenant(pool: &PgPool, runtime: Uuid, domain: &str, handle: &str) -> Uuid {
    let id = tenant(pool, runtime).await;
    sqlx::query(
        "UPDATE tenants SET lifecycle_state='provisioning',domain=$2,handle_domain=$3 WHERE id=$1",
    )
    .bind(id)
    .bind(domain)
    .bind(handle)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO provisioning_steps(tenant_id,step) VALUES($1,'database')")
        .bind(id)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO tenant_domains(id,tenant_id,hostname,kind,status,is_primary) VALUES($1,$2,$3,'custom','active',true)")
        .bind(Uuid::new_v4())
        .bind(id)
        .bind(domain)
        .execute(pool)
        .await
        .unwrap();
    id
}

#[sqlx::test(migrations = "./migrations")]
#[ignore = "requires disposable PostgreSQL; use mise run test-integration"]
async fn provisioning_waits_for_the_handle_domain_to_redirect(pool: PgPool) {
    let (runtime, _) = fixture(&pool).await;
    let id = handle_domain_tenant(&pool, runtime, "custom.example", "handle.invalid").await;
    let state = state(pool.clone());
    let step = || async {
        sqlx::query_scalar::<_, String>(
            "SELECT status FROM provisioning_steps WHERE tenant_id=$1 AND step='database'",
        )
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap()
    };
    queue_job(&pool, id, "provision").await;
    let job = jobs::claim(&pool).await.unwrap().unwrap();
    jobs::run(&state, &job).await;
    // Its web domain connected, but `@user@handle.invalid` would not find it.
    assert!(!job_finished(&pool, job.id).await);
    assert_eq!(step().await, "pending");
    assert!(
        sqlx::query_scalar::<_, Option<OffsetDateTime>>(
            "SELECT handle_domain_verified_at FROM tenants WHERE id=$1"
        )
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap()
        .is_none()
    );
    // Once seen redirecting, setup goes on: this test has no server to make
    // a database on, so the step failing shows it was tried.
    sqlx::query("UPDATE tenants SET handle_domain_verified_at=now() WHERE id=$1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    jobs::wake_provisioning(&state, id).await;
    let job = jobs::claim(&pool).await.unwrap().unwrap();
    jobs::run(&state, &job).await;
    assert_eq!(step().await, "failed");
}

#[sqlx::test(migrations = "./migrations")]
#[ignore = "requires disposable PostgreSQL; use mise run test-integration"]
async fn a_handle_domain_that_never_redirects_expires_with_its_instance(pool: PgPool) {
    let (runtime, _) = fixture(&pool).await;
    let waiting = handle_domain_tenant(&pool, runtime, "waiting.example", "waiting.invalid").await;
    let expired = handle_domain_tenant(&pool, runtime, "expired.example", "expired.invalid").await;
    // Verified, it is being set up, however long ago it was asked for.
    let verified =
        handle_domain_tenant(&pool, runtime, "verified.example", "verified.invalid").await;
    sqlx::query("UPDATE tenants SET handle_domain_verified_at=now() WHERE id=$1")
        .bind(verified)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE tenant_domains SET created_at=now()-interval '8 days' WHERE tenant_id=ANY($1)",
    )
    .bind(vec![expired, verified])
    .execute(&pool)
    .await
    .unwrap();
    expire_custom_domains(&state(pool.clone())).await;
    let states = sqlx::query_as::<_, (Uuid, String, String)>(
        "SELECT t.id,t.lifecycle_state,d.status FROM tenants t JOIN tenant_domains d ON d.tenant_id=t.id",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(states.len(), 3);
    for (id, lifecycle, domain) in states {
        let expected = if id == expired {
            ("deprovisioning", "removing")
        } else {
            ("provisioning", "active")
        };
        assert_eq!((lifecycle.as_str(), domain.as_str()), expected, "{id}");
    }
    let _ = waiting;
}

#[sqlx::test(migrations = "./migrations")]
#[ignore = "requires disposable PostgreSQL; use mise run test-integration"]
async fn a_handle_domain_needs_a_custom_domain_of_its_own(pool: PgPool) {
    let (runtime, _) = fixture(&pool).await;
    let taken = tenant(&pool, runtime).await;
    sqlx::query("UPDATE tenants SET domain='taken.example',handle_domain='taken-handle.example' WHERE id=$1")
        .bind(taken)
        .execute(&pool)
        .await
        .unwrap();
    let mut state = state(pool.clone());
    Arc::get_mut(&mut state).unwrap().provided_domain_suffixes = vec!["eunha.example".into()];
    let check = |handle: &'static str, custom: Option<&'static str>| {
        let state = state.clone();
        async move {
            validate_handle_domain(&state, handle, custom, false)
                .await
                .map_err(|error| error.0)
        }
    };
    assert_eq!(
        check(" Example.COM. ", Some("social.example.com")).await,
        Ok(Some("example.com".into()))
    );
    assert_eq!(check("  ", None).await, Ok(None));
    // On a provided hostname the ids stay on eunha.space when the customer
    // leaves, so the handles could never follow them.
    assert_eq!(
        check("example.com", None).await,
        Err(StatusCode::BAD_REQUEST)
    );
    for handle in [
        "social.example.com",
        "a.eunha.example",
        "eunha.example",
        "not a domain",
    ] {
        assert_eq!(
            check(handle, Some("social.example.com")).await,
            Err(StatusCode::BAD_REQUEST),
            "{handle}"
        );
    }
    for handle in ["taken.example", "taken-handle.example"] {
        assert_eq!(
            check(handle, Some("social.example.com")).await,
            Err(StatusCode::CONFLICT),
            "{handle}"
        );
    }
}

#[sqlx::test(migrations = "./migrations")]
#[ignore = "requires disposable PostgreSQL; use mise run test-integration"]
async fn only_an_operator_puts_handles_on_a_provided_suffix(pool: PgPool) {
    let _ = fixture(&pool).await;
    let mut state = state(pool.clone());
    Arc::get_mut(&mut state).unwrap().provided_domain_suffixes =
        vec!["eunha.example".into(), "eunha.example.net".into()];
    let check = |handle: &'static str, custom: Option<&'static str>, operator: bool| {
        let state = state.clone();
        async move {
            validate_handle_domain(&state, handle, custom, operator)
                .await
                .map_err(|error| error.0)
        }
    };
    // An official instance: @nyx@eunha.example at social.eunha.example.
    assert_eq!(
        check("Eunha.Example.", None, true).await,
        Ok(Some("eunha.example".into()))
    );
    assert_eq!(
        check("eunha.example.net", None, true).await,
        Ok(Some("eunha.example.net".into()))
    );
    // A customer never may, and only the suffix itself is an operator's.
    assert_eq!(
        check("eunha.example", None, false).await,
        Err(StatusCode::BAD_REQUEST)
    );
    for handle in ["a.eunha.example", "example.com"] {
        assert_eq!(
            check(handle, None, true).await,
            Err(StatusCode::BAD_REQUEST),
            "{handle}"
        );
    }
    // Ids on a custom web domain would leave with it; handles on the suffix
    // could not.
    assert_eq!(
        check("eunha.example", Some("social.example.com"), true).await,
        Err(StatusCode::BAD_REQUEST)
    );
}

#[sqlx::test(migrations = "./migrations")]
#[ignore = "requires disposable PostgreSQL; use mise run test-integration"]
async fn the_suffix_sends_webfinger_on_to_the_instance_with_its_handles(pool: PgPool) {
    let (runtime, _) = fixture(&pool).await;
    let id = tenant(&pool, runtime).await;
    sqlx::query("UPDATE tenants SET domain='social.eunha.example',handle_domain='eunha.example' WHERE id=$1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    let mut state = state(pool.clone());
    Arc::get_mut(&mut state).unwrap().provided_domain_suffixes =
        vec!["eunha.example".into(), "eunha.example.net".into()];
    let ask = |host: &'static str| {
        let state = state.clone();
        async move {
            let mut headers = HeaderMap::new();
            headers.insert(axum::http::header::HOST, host.parse().unwrap());
            handle_domain_webfinger(
                State(state),
                headers,
                "/.well-known/webfinger?resource=acct:nyx@eunha.example"
                    .parse()
                    .unwrap(),
            )
            .await
            .unwrap()
        }
    };
    let response = ask("eunha.example:443").await;
    assert_eq!(response.status(), StatusCode::MOVED_PERMANENTLY);
    let location = response.headers()[axum::http::header::LOCATION]
        .to_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        location,
        "https://social.eunha.example/.well-known/webfinger?resource=acct:nyx@eunha.example"
    );
    // The redirect is the one setup waits for.
    assert!(webfinger_redirect_reaches(
        &location,
        "social.eunha.example"
    ));
    // A suffix no instance has its handles on, and a host that is no suffix.
    for host in ["eunha.example.net", "elsewhere.example"] {
        assert_eq!(ask(host).await.status(), StatusCode::NOT_FOUND, "{host}");
    }
    sqlx::query("UPDATE tenants SET lifecycle_state='purged' WHERE id=$1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(ask("eunha.example").await.status(), StatusCode::NOT_FOUND);
}

// --- Polar -------------------------------------------------------------------

const POLAR_SECRET: &str = "whsec_MfKQ9r8GKYqrTwjUPD8ILPZIo2LaLaSw";

/// Polar's API as far as billing uses it: what it holds, and what it was asked.
#[derive(Clone, Default)]
struct PolarStandIn {
    subscriptions: Arc<std::sync::Mutex<HashMap<String, serde_json::Value>>>,
    orders: Arc<std::sync::Mutex<HashMap<String, serde_json::Value>>>,
    refunds: Arc<std::sync::Mutex<Vec<serde_json::Value>>>,
    checkouts: Arc<std::sync::Mutex<Vec<serde_json::Value>>>,
    /// Every PATCH and DELETE of a subscription, in order.
    changes: Arc<std::sync::Mutex<Vec<(String, serde_json::Value)>>>,
    /// Products' prices, where they differ from $6.99 ($14.99 for the large
    /// one).
    prices: Arc<std::sync::Mutex<HashMap<String, i64>>>,
}

fn rfc3339(at: time::OffsetDateTime) -> String {
    at.format(&time::format_description::well_known::Rfc3339)
        .unwrap()
}

impl PolarStandIn {
    fn holds_subscription(
        &self,
        id: &str,
        user_id: Uuid,
        status: &str,
        product: &str,
        amount: i64,
    ) {
        let now = time::OffsetDateTime::now_utc();
        self.subscriptions.lock().unwrap().insert(
            id.into(),
            serde_json::json!({
                "id": id,
                "status": status,
                "amount": amount,
                "currency": "usd",
                "current_period_start": rfc3339(now),
                "current_period_end": rfc3339(now + time::Duration::days(30)),
                "cancel_at_period_end": false,
                "ended_at": null,
                "past_due_at": null,
                "product_id": product,
                "customer": {"id": "cus_1", "external_id": user_id.to_string()},
                "pending_update": null,
            }),
        );
    }
    fn holds_order(&self, id: &str, user_id: Uuid, subscription: &str, total: i64, tax: i64) {
        self.orders.lock().unwrap().insert(
            id.into(),
            serde_json::json!({
                "id": id,
                "status": "paid",
                "total_amount": total,
                "net_amount": total - tax,
                "tax_amount": tax,
                "refunded_amount": 0,
                "refunded_tax_amount": 0,
                "refundable_amount": total - tax,
                "currency": "usd",
                "billing_reason": "subscription_create",
                "created_at": rfc3339(time::OffsetDateTime::now_utc()),
                "customer": {"id": "cus_1", "external_id": user_id.to_string()},
                "product_id": "prod_standard",
                "subscription_id": subscription,
            }),
        );
    }
    fn set(&self, id: &str, field: &str, value: serde_json::Value) {
        self.subscriptions.lock().unwrap().get_mut(id).unwrap()[field] = value;
    }
    fn changes(&self) -> Vec<(String, serde_json::Value)> {
        self.changes.lock().unwrap().clone()
    }
}

/// Polar names the version it answered in; every request must pin one.
fn polar_answer(
    headers: &HeaderMap,
    status: StatusCode,
    body: serde_json::Value,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    assert_eq!(headers.get("Polar-Version").unwrap(), "2026-10");
    assert_eq!(
        headers.get(header::AUTHORIZATION).unwrap(),
        "Bearer polar_oat_test"
    );
    (status, [("Polar-Version", "2026-10")], Json(body)).into_response()
}

async fn polar_stand_in() -> (PolarStandIn, String) {
    use axum::routing::{get, post};
    let polar = PolarStandIn::default();
    let missing = || serde_json::json!({"detail": "Not found"});
    let app = axum::Router::new()
        .route(
            "/v1/checkouts/",
            post(|State(polar): State<PolarStandIn>, headers: HeaderMap, Json(body): Json<serde_json::Value>| async move {
                polar.checkouts.lock().unwrap().push(body);
                polar_answer(&headers, StatusCode::CREATED, serde_json::json!({"id": "chk_1", "url": "https://polar.example/checkout/chk_1"}))
            }),
        )
        .route(
            "/v1/customer-sessions/",
            post(|headers: HeaderMap, Json(body): Json<serde_json::Value>| async move {
                assert!(body["external_customer_id"].is_string());
                polar_answer(&headers, StatusCode::CREATED, serde_json::json!({"customer_portal_url": "https://polar.example/portal"}))
            }),
        )
        .route(
            "/v1/products/{id}",
            get(|State(polar): State<PolarStandIn>, headers: HeaderMap, Path(id): Path<String>| async move {
                let price = polar.prices.lock().unwrap().get(&id).copied()
                    .unwrap_or(if id == "prod_large" { 1499 } else { 699 });
                polar_answer(&headers, StatusCode::OK, serde_json::json!({
                    "id": id,
                    "is_recurring": true,
                    "is_archived": false,
                    "recurring_interval": "month",
                    "prices": [{"amount_type": "fixed", "is_archived": false, "price_currency": "usd", "price_amount": price}],
                }))
            }),
        )
        .route(
            "/v1/subscriptions/",
            get(|State(polar): State<PolarStandIn>, headers: HeaderMap, Query(query): Query<HashMap<String, String>>| async move {
                let items: Vec<_> = polar.subscriptions.lock().unwrap().values()
                    .filter(|s| s["customer"]["external_id"].as_str() == query.get("external_customer_id").map(String::as_str))
                    .cloned().collect();
                polar_answer(&headers, StatusCode::OK, serde_json::json!({"items": items, "pagination": {}}))
            }),
        )
        .route(
            "/v1/subscriptions/{id}",
            get(move |State(polar): State<PolarStandIn>, headers: HeaderMap, Path(id): Path<String>| async move {
                match polar.subscriptions.lock().unwrap().get(&id) {
                    Some(s) => polar_answer(&headers, StatusCode::OK, s.clone()),
                    None => polar_answer(&headers, StatusCode::NOT_FOUND, missing()),
                }
            })
            .patch(|State(polar): State<PolarStandIn>, headers: HeaderMap, Path(id): Path<String>, Json(body): Json<serde_json::Value>| async move {
                polar.changes.lock().unwrap().push((id.clone(), body.clone()));
                let mut subscriptions = polar.subscriptions.lock().unwrap();
                let s = subscriptions.get_mut(&id).unwrap();
                if let Some(cancel) = body.get("cancel_at_period_end") {
                    s["cancel_at_period_end"] = cancel.clone();
                }
                if body.get("pending_update").is_some() {
                    s["pending_update"] = serde_json::Value::Null;
                }
                if let Some(product) = body.get("product_id") {
                    if body["proration_behavior"] == "next_period" {
                        s["pending_update"] = serde_json::json!({"product_id": product});
                    } else {
                        s["product_id"] = product.clone();
                        s["amount"] = if product == "prod_large" { 1499 } else { 699 }.into();
                        s["pending_update"] = serde_json::Value::Null;
                    }
                }
                polar_answer(&headers, StatusCode::OK, s.clone())
            })
            .delete(|State(polar): State<PolarStandIn>, headers: HeaderMap, Path(id): Path<String>| async move {
                polar.changes.lock().unwrap().push((id.clone(), serde_json::json!("revoke")));
                let mut subscriptions = polar.subscriptions.lock().unwrap();
                let s = subscriptions.get_mut(&id).unwrap();
                s["status"] = "canceled".into();
                s["ended_at"] = rfc3339(time::OffsetDateTime::now_utc()).into();
                polar_answer(&headers, StatusCode::OK, s.clone())
            }),
        )
        .route(
            "/v1/orders/",
            get(|State(polar): State<PolarStandIn>, headers: HeaderMap, Query(query): Query<HashMap<String, String>>| async move {
                let items: Vec<_> = polar.orders.lock().unwrap().values()
                    .filter(|o| o["customer"]["external_id"].as_str() == query.get("external_customer_id").map(String::as_str))
                    .cloned().collect();
                polar_answer(&headers, StatusCode::OK, serde_json::json!({"items": items, "pagination": {}}))
            }),
        )
        .route(
            "/v1/orders/{id}",
            get(move |State(polar): State<PolarStandIn>, headers: HeaderMap, Path(id): Path<String>| async move {
                match polar.orders.lock().unwrap().get(&id) {
                    Some(o) => polar_answer(&headers, StatusCode::OK, o.clone()),
                    None => polar_answer(&headers, StatusCode::NOT_FOUND, missing()),
                }
            }),
        )
        .route(
            "/v1/refunds/",
            get(|State(polar): State<PolarStandIn>, headers: HeaderMap, Query(query): Query<HashMap<String, String>>| async move {
                let items: Vec<_> = polar.refunds.lock().unwrap().iter()
                    .filter(|r| r["order_id"].as_str() == query.get("order_id").map(String::as_str))
                    .cloned().collect();
                polar_answer(&headers, StatusCode::OK, serde_json::json!({"items": items, "pagination": {}}))
            })
            .post(|State(polar): State<PolarStandIn>, headers: HeaderMap, Json(body): Json<serde_json::Value>| async move {
                let order_id = body["order_id"].as_str().unwrap().to_owned();
                let mut orders = polar.orders.lock().unwrap();
                let order = orders.get_mut(&order_id).unwrap();
                // The net amount is refunded with its tax.
                assert_eq!(body["amount"], order["refundable_amount"]);
                assert_eq!(body["revoke_benefits"], false);
                order["refunded_amount"] = body["amount"].clone();
                order["refunded_tax_amount"] = order["tax_amount"].clone();
                order["refundable_amount"] = 0.into();
                order["status"] = "refunded".into();
                let refund = serde_json::json!({"id": "re_1", "order_id": order_id, "status": "succeeded"});
                polar.refunds.lock().unwrap().push(refund.clone());
                polar_answer(&headers, StatusCode::CREATED, refund)
            }),
        )
        .with_state(polar.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (polar, base)
}

fn polar_state(pool: PgPool, polar_base: &str, toss_base: Option<&str>) -> Shared {
    let mut state = match toss_base {
        Some(toss_base) => Arc::into_inner(billed_state(pool, toss_base)).unwrap(),
        None => Arc::into_inner(state(pool)).unwrap(),
    };
    state.polar = Some(Arc::new(
        billing_polar::Config::new(
            "polar_oat_test".into(),
            POLAR_SECRET.into(),
            polar_base.into(),
            "https://eunha.space".into(),
            "usd".into(),
        )
        .unwrap(),
    ));
    Arc::new(state)
}

/// The plans the Polar tests sell: a standard one and a dearer one.
async fn offer_polar_plans(pool: &PgPool) -> (Uuid, Uuid) {
    let standard = sqlx::query_scalar("INSERT INTO plans(name_en,name_ko,amount,polar_product_id,polar_amount,polar_currency) VALUES('Standard','기본',9900,'prod_standard',699,'USD') RETURNING id")
        .fetch_one(pool).await.unwrap();
    let large = sqlx::query_scalar("INSERT INTO plans(name_en,name_ko,amount,polar_product_id,polar_amount,polar_currency) VALUES('Large','대형',19900,'prod_large',1499,'USD') RETURNING id")
        .fetch_one(pool).await.unwrap();
    (standard, large)
}

/// Deliver a webhook as Polar would, signed or not.
async fn polar_webhook(
    state: &Shared,
    kind: &str,
    data: serde_json::Value,
    signed: bool,
) -> StatusCode {
    let body =
        serde_json::to_vec(&serde_json::json!({"type": kind, "timestamp": "", "data": data}))
            .unwrap();
    let id = format!("msg_{}", Uuid::new_v4().simple());
    let timestamp = time::OffsetDateTime::now_utc().unix_timestamp().to_string();
    let signature = if signed {
        billing_polar::sign(POLAR_SECRET, &id, &timestamp, &body)
    } else {
        billing_polar::sign("whsec_c29tZW9uZSBlbHNl", &id, &timestamp, &body)
    };
    let mut headers = HeaderMap::new();
    headers.insert("webhook-id", id.parse().unwrap());
    headers.insert("webhook-timestamp", timestamp.parse().unwrap());
    headers.insert("webhook-signature", signature.parse().unwrap());
    billing_polar::webhook(State(state.clone()), headers, body.into()).await
}

async fn polar_view(state: &Shared, headers: &HeaderMap) -> serde_json::Value {
    let Json(view) = billing::get(State(state.clone()), headers.clone())
        .await
        .unwrap();
    serde_json::to_value(view).unwrap()
}

#[sqlx::test(migrations = "./migrations")]
#[ignore = "requires disposable PostgreSQL; use mise run test-integration"]
async fn a_polar_checkout_names_the_customer_plan_and_currency(pool: PgPool) {
    let (polar, base) = polar_stand_in().await;
    let state = polar_state(pool.clone(), &base, None);
    let (user_id, mut headers) = billed_customer(&pool).await;
    let (_, large) = offer_polar_plans(&pool).await;
    headers.insert("cf-connecting-ip", "203.0.113.9".parse().unwrap());
    let Json(page) = billing_polar::checkout(
        State(state.clone()),
        headers.clone(),
        Some(Json(
            serde_json::from_value(serde_json::json!({"plan_id": large})).unwrap(),
        )),
    )
    .await
    .unwrap();
    assert_eq!(
        serde_json::to_value(page).unwrap()["url"],
        "https://polar.example/checkout/chk_1"
    );
    let sent = polar.checkouts.lock().unwrap()[0].clone();
    assert_eq!(sent["products"], serde_json::json!(["prod_large"]));
    assert_eq!(sent["external_customer_id"], user_id.to_string());
    assert_eq!(sent["customer_email"], "customer@example.com");
    assert_eq!(sent["currency"], "usd");
    assert_eq!(sent["customer_ip_address"], "203.0.113.9");
    assert_eq!(
        sent["success_url"],
        "https://eunha.space/account/billing?polar_checkout={CHECKOUT_ID}"
    );
    // Billing counts as on with Polar alone, and Korean cards are not offered.
    let view = polar_view(&state, &headers).await;
    assert_eq!(
        (
            view["enabled"].clone(),
            view["toss"].clone(),
            view["polar"].clone()
        ),
        (true.into(), false.into(), true.into())
    );
    assert_eq!(view["polar_currency"], "USD");
    assert_eq!(view["plans"][1]["polar_amount"], 1499);
}

#[sqlx::test(migrations = "./migrations")]
#[ignore = "requires disposable PostgreSQL; use mise run test-integration"]
async fn a_signed_polar_webhook_starts_the_subscription_and_serves_suspended_instances(
    pool: PgPool,
) {
    let (polar, base) = polar_stand_in().await;
    let state = polar_state(pool.clone(), &base, None);
    let (user_id, headers) = billed_customer(&pool).await;
    let (standard, _) = offer_polar_plans(&pool).await;
    let id = ready_instance(&pool, user_id).await;
    sqlx::query(
        "UPDATE tenants SET lifecycle_state='retained',billing_suspended_at=now() WHERE id=$1",
    )
    .bind(id)
    .execute(&pool)
    .await
    .unwrap();
    polar.holds_subscription("sub_1", user_id, "active", "prod_standard", 699);

    // An unsigned delivery is kept, refused and not acted on, whatever it says.
    let forged = serde_json::json!({"id": "sub_1"});
    assert_eq!(
        polar_webhook(&state, "subscription.active", forged.clone(), false).await,
        StatusCode::UNAUTHORIZED
    );
    assert!(
        sqlx::query_scalar::<_, bool>("SELECT true FROM subscriptions WHERE user_id=$1")
            .bind(user_id)
            .fetch_optional(&pool)
            .await
            .unwrap()
            .is_none()
    );
    // What a signed one names is read from Polar: the event's own copy is not
    // trusted, so a stale one cannot undo a newer state.
    let stale = serde_json::json!({"id": "sub_1", "status": "canceled"});
    assert_eq!(
        polar_webhook(&state, "subscription.active", stale, true).await,
        StatusCode::ACCEPTED
    );
    let kept = sqlx::query_as::<_, (bool, i32)>(
        "SELECT verified,http_status FROM polar_webhook_deliveries ORDER BY received_at",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(kept, vec![(false, 401), (true, 202)]);

    let view = polar_view(&state, &headers).await;
    let subscription = &view["subscription"];
    assert_eq!(subscription["status"], "active");
    assert_eq!(subscription["provider"], "polar");
    assert_eq!(subscription["currency"], "USD");
    assert_eq!(subscription["amount"], 699);
    assert_eq!(subscription["plan_id"], standard.to_string());
    assert!(
        billing::require_subscription(&state, &mut pool.begin().await.unwrap(), user_id)
            .await
            .is_ok()
    );
    assert_eq!(lifecycle(&pool, id).await.0, "provisioning");
    // Delivered again, it changes nothing.
    assert_eq!(
        polar_webhook(&state, "subscription.updated", forged, true).await,
        StatusCode::ACCEPTED
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM operations WHERE kind='subscription_start'"
        )
        .fetch_one(&pool)
        .await
        .unwrap(),
        1
    );
}

#[sqlx::test(migrations = "./migrations")]
#[ignore = "requires disposable PostgreSQL; use mise run test-integration"]
async fn a_polar_subscription_cancels_resumes_changes_plans_and_ends_with_polar(pool: PgPool) {
    let (polar, base) = polar_stand_in().await;
    let state = polar_state(pool.clone(), &base, None);
    let (user_id, headers) = billed_customer(&pool).await;
    let (standard, large) = offer_polar_plans(&pool).await;
    polar.holds_subscription("sub_1", user_id, "active", "prod_standard", 699);
    let _ = billing_polar::refresh(State(state.clone()), headers.clone())
        .await
        .unwrap();
    let id = ready_instance(&pool, user_id).await;

    let Json(view) = billing::cancel(State(state.clone()), headers.clone())
        .await
        .unwrap();
    let view = serde_json::to_value(view).unwrap();
    assert_eq!(view["subscription"]["status"], "active");
    assert_eq!(view["subscription"]["cancel_at_period_end"], true);
    let Json(view) = billing::resume(State(state.clone()), headers.clone())
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(view).unwrap()["subscription"]["cancel_at_period_end"],
        false
    );

    // A dearer plan applies now, and Polar charges the difference.
    let choose =
        |plan: Uuid| Json(serde_json::from_value(serde_json::json!({"plan_id": plan})).unwrap());
    let Json(view) = billing::change_plan(State(state.clone()), headers.clone(), choose(large))
        .await
        .unwrap();
    let view = serde_json::to_value(view).unwrap();
    assert_eq!(view["subscription"]["plan_id"], large.to_string());
    assert_eq!(view["subscription"]["amount"], 1499);
    // A cheaper one waits for the renewal, and keeping the current plan takes
    // that back.
    let Json(view) = billing::change_plan(State(state.clone()), headers.clone(), choose(standard))
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(view).unwrap()["subscription"]["pending_plan_id"],
        standard.to_string()
    );
    let Json(view) = billing::change_plan(State(state.clone()), headers.clone(), choose(large))
        .await
        .unwrap();
    assert!(serde_json::to_value(view).unwrap()["subscription"]["pending_plan_id"].is_null());
    let changes: Vec<serde_json::Value> =
        polar.changes().into_iter().map(|(_, body)| body).collect();
    assert_eq!(
        changes,
        vec![
            serde_json::json!({"cancel_at_period_end": true}),
            serde_json::json!({"cancel_at_period_end": false}),
            serde_json::json!({"product_id": "prod_large", "proration_behavior": "invoice"}),
            serde_json::json!({"product_id": "prod_standard", "proration_behavior": "next_period"}),
            serde_json::json!({"pending_update": null}),
        ]
    );

    // Polar retries a declined renewal; the instances keep running meanwhile.
    polar.set("sub_1", "status", "past_due".into());
    polar_webhook(
        &state,
        "subscription.past_due",
        serde_json::json!({"id": "sub_1"}),
        true,
    )
    .await;
    assert_eq!(subscription_status(&pool, user_id).await, "past_due");
    assert_eq!(lifecycle(&pool, id).await.0, "ready");
    // When it gives up and revokes, the subscription lapses here, found by the
    // periodic look even with no webhook.
    polar.set("sub_1", "status", "canceled".into());
    polar.set(
        "sub_1",
        "ended_at",
        rfc3339(time::OffsetDateTime::now_utc()).into(),
    );
    sqlx::query(
        "UPDATE subscriptions SET polar_synced_at=now()-interval '2 hours' WHERE user_id=$1",
    )
    .bind(user_id)
    .execute(&pool)
    .await
    .unwrap();
    assert!(billing_polar::sync_due(&state).await.unwrap());
    assert!(!billing_polar::sync_due(&state).await.unwrap());
    assert_eq!(subscription_status(&pool, user_id).await, "lapsed");
    assert_eq!(lifecycle(&pool, id).await, ("deprovisioning".into(), true));
}

#[sqlx::test(migrations = "./migrations")]
#[ignore = "requires disposable PostgreSQL; use mise run test-integration"]
async fn a_polar_payment_is_recorded_and_a_customers_refund_revokes_at_polar(pool: PgPool) {
    let (polar, base) = polar_stand_in().await;
    let state = polar_state(pool.clone(), &base, None);
    let (user_id, headers) = billed_customer(&pool).await;
    offer_polar_plans(&pool).await;
    polar.holds_subscription("sub_1", user_id, "active", "prod_standard", 699);
    polar.holds_order("ord_1", user_id, "sub_1", 769, 70);
    assert_eq!(
        polar_webhook(
            &state,
            "subscription.created",
            serde_json::json!({"id": "sub_1"}),
            true
        )
        .await,
        StatusCode::ACCEPTED
    );
    assert_eq!(
        polar_webhook(
            &state,
            "order.paid",
            serde_json::json!({"id": "ord_1"}),
            true
        )
        .await,
        StatusCode::ACCEPTED
    );
    // Again: recorded once.
    polar_webhook(
        &state,
        "order.updated",
        serde_json::json!({"id": "ord_1"}),
        true,
    )
    .await;
    let id = ready_instance(&pool, user_id).await;

    let view = polar_view(&state, &headers).await;
    assert_eq!(view["payments"].as_array().unwrap().len(), 1);
    let payment = &view["payments"][0];
    assert_eq!(
        (
            payment["order_id"].clone(),
            payment["amount"].clone(),
            payment["currency"].clone(),
            payment["provider"].clone()
        ),
        ("ord_1".into(), 769.into(), "USD".into(), "polar".into())
    );
    assert_eq!(payment["refundable"], true);
    // Polar sends its own receipts.
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM billing_emails WHERE kind='receipt'")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );

    let view = refund(&state, &headers, "ord_1").await.unwrap();
    assert_eq!(view["payments"][0]["status"], "refunded");
    assert_eq!(view["payments"][0]["refunded_amount"], 769);
    assert_eq!(view["subscription"]["status"], "canceled");
    assert_eq!(lifecycle(&pool, id).await, ("deprovisioning".into(), true));
    // Polar is told to stop billing it, once.
    assert_eq!(
        polar.changes(),
        vec![("sub_1".to_owned(), serde_json::json!("revoke"))]
    );
    assert_eq!(polar.refunds.lock().unwrap().len(), 1);
    assert!(
        !sqlx::query_scalar::<_, bool>("SELECT polar_live FROM subscriptions WHERE user_id=$1")
            .bind(user_id)
            .fetch_one(&pool)
            .await
            .unwrap()
    );
    // The revoked subscription's own webhook changes nothing more.
    polar_webhook(
        &state,
        "subscription.revoked",
        serde_json::json!({"id": "sub_1"}),
        true,
    )
    .await;
    assert_eq!(subscription_status(&pool, user_id).await, "canceled");
    assert_eq!(polar.changes().len(), 1);
}

#[sqlx::test(migrations = "./migrations")]
#[ignore = "requires disposable PostgreSQL; use mise run test-integration"]
async fn tosspayments_and_polar_never_bill_the_same_customer(pool: PgPool) {
    let (toss, toss_base) = toss_stand_in().await;
    let (polar, polar_base) = polar_stand_in().await;
    let state = polar_state(pool.clone(), &polar_base, Some(&toss_base));
    offer_polar_plans(&pool).await;

    // A Korean card's subscription: no Polar checkout on top of it.
    let (_, korean) = billed_customer(&pool).await;
    assert_eq!(
        subscribe(&state, &korean).await["subscription"]["provider"],
        "toss"
    );
    assert_eq!(
        billing_polar::checkout(State(state.clone()), korean.clone(), None)
            .await
            .err()
            .unwrap()
            .0,
        StatusCode::CONFLICT
    );

    // An international card's: no TossPayments card on top of it, and the
    // renewer never charges it.
    let user_id = Uuid::new_v4();
    sqlx::query("INSERT INTO users(id,email,password_hash,email_verified_at) VALUES($1,'abroad@example.com','unused',now())")
        .bind(user_id).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO user_sessions(token_hash,user_id,expires_at) VALUES($1,$2,now()+interval '1 hour')").bind(token_hash("abroad-session")).bind(user_id).execute(&pool).await.unwrap();
    let mut abroad = HeaderMap::new();
    abroad.insert(
        header::COOKIE,
        "eunha_space_customer_session=abroad-session"
            .parse()
            .unwrap(),
    );
    polar.holds_subscription("sub_1", user_id, "active", "prod_standard", 699);
    let _ = billing_polar::refresh(State(state.clone()), abroad.clone())
        .await
        .unwrap();
    assert_eq!(
        billing::checkout(State(state.clone()), abroad.clone(), None)
            .await
            .err()
            .unwrap()
            .0,
        StatusCode::CONFLICT
    );
    let charges = toss.charges().len();
    sqlx::query("UPDATE subscriptions SET next_attempt_at=now()-interval '1 day',current_period_end=now()-interval '1 day' WHERE user_id=$1")
        .bind(user_id).execute(&pool).await.unwrap();
    assert!(!billing::charge_due(&state, Some(user_id)).await.unwrap());
    assert_eq!(toss.charges().len(), charges);
    assert_eq!(subscription_status(&pool, user_id).await, "active");
}

#[sqlx::test(migrations = "./migrations")]
#[ignore = "requires disposable PostgreSQL; use mise run test-integration"]
async fn operators_edit_shown_limits_and_delete_only_unused_plans(pool: PgPool) {
    let (_, headers) = fixture(&pool).await;
    let state = state(pool.clone());
    let used = plan(&pool, 9900, None).await;
    let unused = plan(&pool, 19900, None).await;
    sqlx::query("UPDATE plans SET user_limit=50,media_gb=10 WHERE id=$1")
        .bind(used)
        .execute(&pool)
        .await
        .unwrap();
    let update = |body: serde_json::Value| Json(serde_json::from_value(body).unwrap());

    // Sent as null, a shown limit is cleared; left out, it stays.
    let Json(edited) = billing_admin::update_plan(
        State(state.clone()),
        headers.clone(),
        Path(used),
        update(serde_json::json!({"name_en":"Personal","name_ko":"개인","instance_limit":1,"active":true,"user_limit":3,"media_gb":null})),
    )
    .await
    .unwrap();
    let edited = serde_json::to_value(edited).unwrap();
    assert_eq!(
        (
            edited["name_ko"].clone(),
            edited["instance_limit"].clone(),
            edited["user_limit"].clone(),
            edited["media_gb"].clone(),
            edited["amount"].clone()
        ),
        (
            "개인".into(),
            1.into(),
            3.into(),
            serde_json::Value::Null,
            9900.into()
        )
    );
    let Json(again) = billing_admin::update_plan(
        State(state.clone()),
        headers.clone(),
        Path(used),
        update(serde_json::json!({"name_en":"Personal","name_ko":"개인","instance_limit":1,"active":true})),
    )
    .await
    .unwrap();
    assert_eq!(serde_json::to_value(again).unwrap()["user_limit"], 3);

    // A plan a subscription ever used is retired, not deleted.
    let (user_id, _) = billed_customer(&pool).await;
    sqlx::query("INSERT INTO subscriptions(user_id,customer_key,amount,status,plan_id) VALUES($1,$2,9900,'canceled',$3)")
        .bind(user_id)
        .bind(Uuid::new_v4())
        .bind(used)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        billing_admin::delete_plan(State(state.clone()), headers.clone(), Path(used))
            .await
            .unwrap_err()
            .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        billing_admin::delete_plan(State(state.clone()), headers.clone(), Path(unused))
            .await
            .unwrap(),
        StatusCode::NO_CONTENT
    );
    let left: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM plans")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(left, vec![used]);
}

#[sqlx::test(migrations = "./migrations")]
#[ignore = "requires disposable PostgreSQL; use mise run test-integration"]
async fn a_polar_price_change_reaches_new_customers_and_spares_subscribers(pool: PgPool) {
    let (polar, base) = polar_stand_in().await;
    let state = polar_state(pool.clone(), &base, None);
    let (_, admin) = fixture(&pool).await;
    let (user_id, headers) = billed_customer(&pool).await;
    let (standard, large) = offer_polar_plans(&pool).await;
    polar.holds_subscription("sub_1", user_id, "active", "prod_standard", 699);
    let _ = billing_polar::refresh(State(state.clone()), headers.clone())
        .await
        .unwrap();

    // Polar raises the price; its webhook names the product.
    polar
        .prices
        .lock()
        .unwrap()
        .insert("prod_standard".into(), 799);
    assert_eq!(
        polar_webhook(
            &state,
            "product.updated",
            serde_json::json!({"id": "prod_standard"}),
            true
        )
        .await,
        StatusCode::ACCEPTED
    );
    let view = polar_view(&state, &headers).await;
    assert_eq!(view["plans"][0]["polar_amount"], 799);
    // The subscriber keeps the price they signed up at, and the next renewal
    // reading Polar's subscription changes nothing about it.
    assert_eq!(view["subscription"]["amount"], 699);
    sqlx::query(
        "UPDATE subscriptions SET polar_synced_at=now()-interval '2 hours' WHERE user_id=$1",
    )
    .bind(user_id)
    .execute(&pool)
    .await
    .unwrap();
    assert!(billing_polar::sync_due(&state).await.unwrap());
    assert_eq!(
        polar_view(&state, &headers).await["subscription"]["amount"],
        699
    );

    // An operator can read it again without waiting for the webhook.
    polar
        .prices
        .lock()
        .unwrap()
        .insert("prod_large".into(), 1599);
    let Json(plan) = billing_admin::refresh_polar(State(state.clone()), admin.clone(), Path(large))
        .await
        .unwrap();
    assert_eq!(serde_json::to_value(plan).unwrap()["polar_amount"], 1599);

    // A plan with a running Polar subscription keeps its product; one without
    // moves to another, as from the sandbox to live products, at its price.
    let link = |product: &str| {
        Json(serde_json::from_value(serde_json::json!({"product_id": product})).unwrap())
    };
    assert_eq!(
        billing_admin::link_polar(
            State(state.clone()),
            admin.clone(),
            Path(standard),
            link("prod_live_standard")
        )
        .await
        .err()
        .unwrap()
        .0,
        StatusCode::CONFLICT
    );
    polar
        .prices
        .lock()
        .unwrap()
        .insert("prod_live_large".into(), 1499);
    let Json(plan) = billing_admin::link_polar(
        State(state.clone()),
        admin.clone(),
        Path(large),
        link("prod_live_large"),
    )
    .await
    .unwrap();
    let plan = serde_json::to_value(plan).unwrap();
    assert_eq!(
        (
            plan["polar_product_id"].clone(),
            plan["polar_amount"].clone()
        ),
        ("prod_live_large".into(), 1499.into())
    );
}
