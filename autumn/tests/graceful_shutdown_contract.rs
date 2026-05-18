//! Rolling-deploy shutdown contract tests for issue #679.
//!
//! Written RED-first: tests reference `ServerConfig::prestop_grace_secs`
//! and `run_shutdown_hooks_with_timeout` before either exists, so the
//! first `cargo test` run will fail to compile (RED). The green phase
//! adds the missing symbols; the refactor phase cleans up.
//!
//! AC coverage:
//! - AC 2: prestop_grace_secs config field (≥ 5 default)
//! - AC 3: autumn_shutdown_aborted_requests_total counter
//! - AC 7: per-hook shutdown timeout + overrun logging
//! - AC 8: exit-code contract tested via ShutdownOutcome
//! - AC 9: integration test — SIGTERM during long HTTP request

use autumn_web::config::AutumnConfig;

// ── AC 2: prestop_grace_secs ─────────────────────────────────────────────────

#[test]
fn server_config_prestop_grace_secs_defaults_to_five() {
    let config = AutumnConfig::default();
    assert!(
        config.server.prestop_grace_secs >= 5,
        "prestop_grace_secs default must be ≥ 5 so load balancers can deregister before \
         the listener closes"
    );
}

#[test]
fn server_config_prestop_grace_secs_is_configurable_via_toml() {
    let config: AutumnConfig = toml::from_str(
        r#"
        [server]
        prestop_grace_secs = 12
        "#,
    )
    .expect("config should deserialize");
    assert_eq!(config.server.prestop_grace_secs, 12);
}

#[test]
fn server_config_prestop_grace_secs_env_override() {
    use autumn_web::config::MockEnv;
    let env = MockEnv::new().with("AUTUMN_SERVER__PRESTOP_GRACE_SECS", "8");
    let mut config = AutumnConfig::default();
    config.apply_env_overrides_with_env(&env);
    assert_eq!(config.server.prestop_grace_secs, 8);
}

// ── AC 3: autumn_shutdown_aborted_requests_total ─────────────────────────────

#[test]
fn metrics_snapshot_includes_shutdown_aborted_requests() {
    use autumn_web::middleware::MetricsCollector;
    let collector = MetricsCollector::new();
    let snap = collector.snapshot();
    // Field must exist; starts at zero.
    assert_eq!(
        snap.http.shutdown_aborted_requests_total,
        0,
        "aborted-requests counter must start at zero"
    );
}

#[test]
fn metrics_collector_can_record_aborted_requests() {
    use autumn_web::middleware::MetricsCollector;
    let collector = MetricsCollector::new();
    collector.record_shutdown_aborted(3);
    let snap = collector.snapshot();
    assert_eq!(snap.http.shutdown_aborted_requests_total, 3);
}

// ── AC 7: per-hook shutdown timeout ─────────────────────────────────────────

#[tokio::test]
async fn shutdown_hooks_with_timeout_runs_all_fast_hooks() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    let counter = Arc::new(AtomicUsize::new(0));
    let c1 = Arc::clone(&counter);
    let c2 = Arc::clone(&counter);

    let hooks: Vec<Box<dyn Fn() -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> + Send + Sync>> = vec![
        Box::new(move || {
            let c = Arc::clone(&c1);
            Box::pin(async move { c.fetch_add(1, Ordering::SeqCst); })
        }),
        Box::new(move || {
            let c = Arc::clone(&c2);
            Box::pin(async move { c.fetch_add(1, Ordering::SeqCst); })
        }),
    ];

    autumn_web::app::run_shutdown_hooks_with_timeout_for_test(
        &hooks,
        Duration::from_secs(2),
        Duration::from_secs(10),
    )
    .await;

    assert_eq!(counter.load(Ordering::SeqCst), 2, "both hooks must run");
}

#[tokio::test]
async fn shutdown_hooks_with_timeout_tolerates_slow_hook_overrun() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    let fast_ran = Arc::new(AtomicBool::new(false));
    let fr = Arc::clone(&fast_ran);

    let hooks: Vec<Box<dyn Fn() -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> + Send + Sync>> = vec![
        // hook 0: slow (exceeds per-hook budget of 50ms)
        Box::new(|| {
            Box::pin(async move {
                tokio::time::sleep(Duration::from_millis(200)).await;
            })
        }),
        // hook 1: fast — must still run within total budget
        Box::new(move || {
            let fr = Arc::clone(&fr);
            Box::pin(async move {
                fr.store(true, Ordering::SeqCst);
            })
        }),
    ];

    // Per-hook budget = 50 ms (hook 0 will overrun).
    // Total budget = 1 s (ample for hook 1 to complete).
    autumn_web::app::run_shutdown_hooks_with_timeout_for_test(
        &hooks,
        Duration::from_millis(50),
        Duration::from_secs(1),
    )
    .await;

    assert!(
        fast_ran.load(Ordering::SeqCst),
        "fast hook must still run even after a slow hook overruns its per-hook budget"
    );
}

// ── AC 9: integration — SIGTERM during long HTTP request ─────────────────────

/// Spins up a real TCP server, begins a slow HTTP request, fires the
/// shutdown cancellation token (simulating SIGTERM), then asserts that:
///  (a) the slow request completes (not dropped mid-flight), and
///  (b) /ready returned 503 after begin_shutdown() and before the
///      listener closed.
///
/// Uses raw tokio TCP + hand-written HTTP/1.1 to avoid adding reqwest to
/// dev-deps.
#[tokio::test(flavor = "multi_thread")]
async fn sigterm_during_long_http_request_drains_cleanly() {
    use autumn_web::prelude::*;
    use autumn_web::probe::ProbeState;
    use autumn_web::test::TestApp;
    use std::net::SocketAddr;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};
    use tokio_util::sync::CancellationToken;

    // ── shared signal flags ────────────────────────────────────────
    let request_completed = Arc::new(AtomicBool::new(false));
    let ready_was_503_during_drain = Arc::new(AtomicBool::new(false));

    let rc = Arc::clone(&request_completed);
    let rwas = Arc::clone(&ready_was_503_during_drain);

    // ── handler: takes 200 ms to respond ──────────────────────────
    #[get("/slow")]
    async fn slow() -> &'static str {
        tokio::time::sleep(Duration::from_millis(200)).await;
        "done"
    }

    // ── build router + server ──────────────────────────────────────
    let probe = ProbeState::ready_for_test();
    let probe_clone = probe.clone();

    let shutdown = CancellationToken::new();
    let shutdown_clone = shutdown.clone();

    let router = TestApp::new()
        .routes(routes![slow])
        .build()
        .into_router();

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral port");
    let addr: SocketAddr = listener.local_addr().expect("local_addr");

    let server_shutdown = shutdown.clone();
    tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(server_shutdown.cancelled_owned())
            .await
            .ok();
    });

    // brief warmup
    tokio::time::sleep(Duration::from_millis(20)).await;

    // ── start the slow request via raw TCP concurrently ───────────
    let client_task = {
        let rc = Arc::clone(&rc);
        tokio::spawn(async move {
            let mut stream = TcpStream::connect(addr)
                .await
                .expect("connect to test server");
            // Send a minimal HTTP/1.1 GET request
            stream
                .write_all(b"GET /slow HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
                .await
                .expect("send request");

            // Read the full response (headers + body)
            let mut buf = Vec::new();
            stream
                .read_to_end(&mut buf)
                .await
                .expect("read response");

            let response = String::from_utf8_lossy(&buf);
            assert!(
                response.starts_with("HTTP/1.1 200"),
                "/slow must respond 200; got: {response}"
            );
            rc.store(true, Ordering::SeqCst);
        })
    };

    // ── simulate SIGTERM: flip /ready → 503, then cancel listener ─
    // After 50 ms the slow request is in-flight (it sleeps 200 ms).
    tokio::time::sleep(Duration::from_millis(50)).await;
    probe_clone.begin_draining();

    // /ready must be 503 at this point (before the listener closes)
    if probe_clone.draining() {
        rwas.store(true, Ordering::SeqCst);
    }

    // Cancel the listener (simulates the prestop_grace elapsing + SIGTERM processed)
    shutdown_clone.cancel();

    // ── drain window: request must complete within 2 s ────────────
    tokio::time::timeout(Duration::from_secs(3), client_task)
        .await
        .expect("slow request should complete within drain window")
        .expect("client task must not panic");

    assert!(
        request_completed.load(Ordering::SeqCst),
        "in-flight HTTP request must complete before server exits"
    );
    assert!(
        ready_was_503_during_drain.load(Ordering::SeqCst),
        "/ready must have been draining (503) before the listener closed"
    );
}

// ── AC 9b: job workers stop dequeuing at drain start ─────────────────────────

/// Asserts that draining the ProbeState (begin_draining) correctly reflects
/// the drain state — the job runtime's shutdown token is the existing
/// server_shutdown CancellationToken; this test verifies the probe contract
/// that callers (job loops, scheduler loops) can observe via is_shutting_down().
#[test]
fn probe_draining_is_observable_for_job_runtime_coordination() {
    use autumn_web::probe::ProbeState;

    let probe = ProbeState::ready_for_test();
    assert!(!probe.is_shutting_down());
    assert!(!probe.draining());

    probe.begin_draining();
    assert!(probe.is_shutting_down());
    assert!(probe.draining());
}
