//! Readiness reflects dependencies and actual background-loop progress.
use super::*;
use futures::FutureExt;
use std::{
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};

#[derive(Default)]
pub(crate) struct Health {
    progress: Mutex<HashMap<&'static str, Instant>>,
    pub worker: AtomicBool,
    draining: AtomicBool,
}
impl Health {
    pub fn tick(&self, job: &'static str) {
        self.progress
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(job, Instant::now());
    }
    fn fresh(&self, job: &str, limit: Duration) -> bool {
        self.progress
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(job)
            .is_some_and(|at| at.elapsed() < limit)
    }
}

pub(crate) async fn live() -> StatusCode {
    StatusCode::OK
}

pub(crate) async fn ready(State(s): State<Shared>) -> Response {
    let database = matches!(
        tokio::time::timeout(
            Duration::from_secs(2),
            sqlx::query_scalar::<_, i32>("SELECT 1").fetch_one(&s.db)
        )
        .await,
        Ok(Ok(1))
    );
    let temporal = if std::env::var_os("TEMPORAL_ADDRESS").is_some() {
        s.health.worker.load(Ordering::Relaxed) && crate::temporal::available().await
    } else {
        false
    };
    let background = [
        "domains",
        "release-recovery",
        "provision-dispatch",
        "lifecycle-dispatch",
        "domain-dispatch",
        "reconcile-starter",
    ]
    .iter()
    .all(|job| s.health.fresh(job, Duration::from_secs(120)));
    let accepting = !s.health.draining.load(Ordering::Relaxed);
    let ok = database && temporal && background && accepting;
    (if ok { StatusCode::OK } else { StatusCode::SERVICE_UNAVAILABLE }, Json(serde_json::json!({"ready":ok,"database":database,"temporal":temporal,"background":background,"accepting":accepting,"release":std::env::var("CONTROL_RELEASE_ID").unwrap_or_else(|_| "development".into())}))).into_response()
}

pub(crate) async fn shutdown(s: Shared) {
    #[cfg(unix)]
    {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut terminate) => {
                tokio::select! { _ = terminate.recv() => {}, _ = tokio::signal::ctrl_c() => {} }
            }
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
    s.health.draining.store(true, Ordering::Relaxed);
    tracing::info!("draining HTTP requests for shutdown");
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn readiness_requires_job_progress() {
        let health = Health::default();
        assert!(!health.fresh("domains", Duration::from_secs(120)));
        health.tick("domains");
        assert!(health.fresh("domains", Duration::from_secs(120)));
        assert!(!health.fresh("domains", Duration::ZERO));
    }
}

/// Restart crashed background controllers; readiness detects loops that stall.
pub(crate) fn supervise<F, Fut>(state: Shared, job: &'static str, run: F)
where
    F: Fn(Shared) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    tokio::spawn(async move {
        loop {
            let _ = std::panic::AssertUnwindSafe(run(state.clone()))
                .catch_unwind()
                .await;
            tracing::error!(job, "background controller stopped; restarting");
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    });
}
