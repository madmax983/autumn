//! Cross-tenant idempotency replay (Warden 2026-09-02).
//!
//! Composes two documented framework features — `[tenancy] enabled = true`
//! (`docs/guide/tenant-cells.md`) and `AppBuilder::idempotent()`
//! (`docs/guide/idempotency.md`) — and asserts that a cached mutation recorded
//! for one tenant is never replayed to a request that resolved to a *different*
//! tenant.
//!
//! The idempotency storage key namespaces by method, request target and the
//! cookie-backed session id. A framework-resolved tenant is none of those, so
//! before the fix two requests that differ *only* by tenant shared one cache
//! slot: the second tenant received the first tenant's stored response body and
//! its own handler — and therefore every `tenant_scoped` repository filter
//! inside it — never ran.
//!
//! The router already fails closed for this exact hazard when the tenant is
//! resolved by an app-supplied `AppBuilder::layer` (see
//! `idempotency_middleware::test_app_wide_generated_route_fails_closed_for_opaque_tenant_scope`).
//! These tests hold the framework's own tenancy middleware to the same bar.

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use autumn_web::config::AutumnConfig;
use autumn_web::idempotency::{IdempotencyLayer, IdempotencyStore, MemoryIdempotencyStore};
use autumn_web::session::{
    MemoryStore as SessionMemoryStore, Session, SessionConfig, SessionLayer, SessionStore,
    SessionStoreError,
};
use autumn_web::tenancy::tenancy_middleware;
use autumn_web::test::TestApp;
use autumn_web::{AppState, post, public, routes};
use axum::body::Body;
use tower::ServiceExt as _;

/// Sentinel-bearing mutation: the body names the tenant the request resolved
/// to, so a replay across tenants is visible in the response text alone.
#[post("/orders")]
#[public]
async fn create_order(tenant: autumn_web::tenancy::Tenant) -> String {
    format!("order-for-{}-sentinel", tenant.0)
}

fn tenancy_config(source: &str) -> AutumnConfig {
    let mut config = AutumnConfig::default();
    config.tenancy.enabled = true;
    source.clone_into(&mut config.tenancy.source);
    "x-tenant-id".clone_into(&mut config.tenancy.header_name);
    config.tenancy.base_domain = Some("example.test".to_owned());
    // Subdomain tenancy resolves the tenant from `Host`, so both tenant hosts
    // have to clear the trusted-host policy before tenancy ever runs.
    config.security.trusted_hosts.hosts = vec![
        "tenant-a.example.test".to_owned(),
        "tenant-b.example.test".to_owned(),
    ];
    config
}

/// Header-sourced tenancy: tenant B must never be served tenant A's stored
/// response, however the two requests happen to agree on `Idempotency-Key`.
#[tokio::test]
async fn header_tenancy_does_not_replay_across_tenants() {
    let app = TestApp::new()
        .config(tenancy_config("header"))
        .idempotent()
        .routes(routes![create_order])
        .build();

    let first = app
        .post("/orders")
        .header("x-tenant-id", "tenant-a")
        .header("idempotency-key", "shared-key")
        .body("{}")
        .send()
        .await;
    first.assert_ok();
    assert_eq!(
        first.text(),
        "order-for-tenant-a-sentinel",
        "tenant A's own mutation runs normally"
    );

    let second = app
        .post("/orders")
        .header("x-tenant-id", "tenant-b")
        .header("idempotency-key", "shared-key")
        .body("{}")
        .send()
        .await;

    assert!(
        !second.text().contains("tenant-a"),
        "tenant B received tenant A's cached response body: {:?} (status {})",
        second.text(),
        second.status
    );
    assert_ne!(
        second.header("x-idempotent-replayed"),
        Some("true"),
        "tenant B's request must not be answered from tenant A's cache slot"
    );
}

/// Subdomain-sourced tenancy: the request target is byte-identical across
/// tenants (only the `Host` differs), so the storage key must carry the
/// resolved tenant or the two tenants share one slot.
#[tokio::test]
async fn subdomain_tenancy_does_not_replay_across_tenants() {
    let app = TestApp::new()
        .config(tenancy_config("subdomain"))
        .idempotent()
        .routes(routes![create_order])
        .build();

    let first = app
        .post("/orders")
        .header("host", "tenant-a.example.test")
        .header("idempotency-key", "shared-key")
        .body("{}")
        .send()
        .await;
    first.assert_ok();
    assert_eq!(first.text(), "order-for-tenant-a-sentinel");

    let second = app
        .post("/orders")
        .header("host", "tenant-b.example.test")
        .header("idempotency-key", "shared-key")
        .body("{}")
        .send()
        .await;

    assert!(
        !second.text().contains("tenant-a"),
        "tenant B received tenant A's cached response body: {:?} (status {})",
        second.text(),
        second.status
    );
}

/// The fix must not break the feature it protects: the *same* tenant retrying
/// the *same* key still gets the stored response back rather than re-executing
/// the mutation.
#[tokio::test]
async fn same_tenant_retry_still_replays() {
    let app = TestApp::new()
        .config(tenancy_config("header"))
        .idempotent()
        .routes(routes![create_order])
        .build();

    let first = app
        .post("/orders")
        .header("x-tenant-id", "tenant-a")
        .header("idempotency-key", "retry-key")
        .body("{}")
        .send()
        .await;
    first.assert_ok();

    let retry = app
        .post("/orders")
        .header("x-tenant-id", "tenant-a")
        .header("idempotency-key", "retry-key")
        .body("{}")
        .send()
        .await;
    retry.assert_ok();
    assert_eq!(retry.text(), "order-for-tenant-a-sentinel");
    assert_eq!(
        retry.header("x-idempotent-replayed"),
        Some("true"),
        "a same-tenant retry must still be served from the idempotency cache"
    );
}

static SWITCH_HANDLER_CALLS: AtomicUsize = AtomicUsize::new(0);

/// Exempt from tenancy (`public_paths`) so a session with no tenant yet can
/// reach it: establishes `tenant_id = "org-a"` for the rest of the test.
#[post("/login")]
#[public]
async fn establish_session_tenant(session: Session) -> &'static str {
    session.insert("tenant_id", "org-a").await;
    "ok"
}

/// Mimics an organization-switch handler (`examples/teams`'s
/// `switch_organization`): mutates the *same* session's tenancy key without
/// rotating the session id, so the request that resolved "org-a" leaves a
/// session now holding "org-b".
#[post("/switch-org")]
#[public]
async fn switch_org(session: Session) -> String {
    let calls = SWITCH_HANDLER_CALLS.fetch_add(1, Ordering::SeqCst) + 1;
    session.insert("tenant_id", "org-b").await;
    format!("switched-{calls}")
}

/// Session-sourced tenancy: the tenant captured before the handler ran is
/// stale the instant the handler itself changes the session's tenancy key.
/// A retry presents the *same* session (no id rotation), which now resolves
/// "org-b" — its deferred idempotency alias must be keyed by that finalized
/// tenant, not the "org-a" the request started as, or the retry misses the
/// cache and re-runs the switch a second time.
#[tokio::test]
async fn session_tenancy_alias_uses_finalized_tenant_after_switch() {
    let mut config = tenancy_config("session");
    config.tenancy.public_paths = vec!["/login".to_owned()];
    let app = TestApp::new()
        .config(config)
        .idempotent()
        .routes(routes![establish_session_tenant, switch_org])
        .build();

    let login = app.post("/login").body("{}").send().await;
    login.assert_ok();

    let first = app
        .post("/switch-org")
        .header("idempotency-key", "switch-key")
        .body("{}")
        .send()
        .await;
    first.assert_ok();
    assert_eq!(first.text(), "switched-1");

    let retry = app
        .post("/switch-org")
        .header("idempotency-key", "switch-key")
        .body("{}")
        .send()
        .await;
    retry.assert_ok();
    assert_eq!(
        retry.text(),
        "switched-1",
        "a retry after an org switch must replay the cached response instead of re-running \
         the handler under the tenant it switched *to*"
    );
    assert_eq!(
        retry.header("x-idempotent-replayed"),
        Some("true"),
        "the retry must be served from the idempotency cache"
    );
}

static LOGIN_HANDLER_CALLS: AtomicUsize = AtomicUsize::new(0);

/// An idempotent login on a `public_paths` route: rotates the session id
/// *and* writes the tenancy session key, mirroring `examples/teams`'s
/// `establish_session` (`session.rotate_id()` then `session.insert(tenant
/// key, ...)`).
#[post("/idempotent-login")]
#[public]
async fn idempotent_login(session: Session) -> String {
    let calls = LOGIN_HANDLER_CALLS.fetch_add(1, Ordering::SeqCst) + 1;
    session.rotate_id().await;
    session.insert("tenant_id", "org-a").await;
    format!("logged-in-{calls}")
}

/// A `public_paths` route is exempt from tenancy resolution unconditionally —
/// that is a property of the *path*, not of session content, so a retry to it
/// resolves no tenant however the handler mutates the session. The primary
/// key this request commits under is therefore tenantless; the finalized
/// tenant the handler just wrote to the (rotated) session must NOT be forced
/// into the alias, or the alias becomes unreachable by any retry to this
/// always-exempt path — the exact "reserve a tenant-shaped alias nothing can
/// ever match" version of the underlying bug.
#[tokio::test]
async fn session_tenancy_public_path_alias_stays_tenantless() {
    let mut config = tenancy_config("session");
    config.tenancy.public_paths = vec!["/idempotent-login".to_owned()];
    let app = TestApp::new()
        .config(config)
        .idempotent()
        .routes(routes![idempotent_login])
        .build();

    let first = app
        .post("/idempotent-login")
        .header("idempotency-key", "login-key")
        .body("{}")
        .send()
        .await;
    first.assert_ok();
    assert_eq!(first.text(), "logged-in-1");
    assert!(
        first.header("set-cookie").is_some(),
        "the rotated session id must reach the client via Set-Cookie"
    );

    let retry = app
        .post("/idempotent-login")
        .header("idempotency-key", "login-key")
        .body("{}")
        .send()
        .await;
    retry.assert_ok();
    assert_eq!(
        retry.text(),
        "logged-in-1",
        "a retry to an always-exempt public path must replay the cached login, not re-run it"
    );
}

static MANUAL_COMPOSITION_HANDLER_CALLS: AtomicUsize = AtomicUsize::new(0);

async fn manual_composition_switch_handler(session: Session) -> String {
    let calls = MANUAL_COMPOSITION_HANDLER_CALLS.fetch_add(1, Ordering::SeqCst) + 1;
    session.insert("tenant_id", "org-b").await;
    format!("switched-{calls}")
}

/// `SessionLayer`, [`tenancy_middleware`] and [`IdempotencyLayer`] are all
/// public building blocks an app can compose directly as plain tower layers
/// instead of going through `AppBuilder` — exactly the pattern
/// `idempotency_middleware.rs`'s own raw-router tests already use. That
/// composition needs `with_tenancy_session_key` set explicitly (`AppBuilder`
/// sets it automatically); this pins that it actually works when set, so the
/// setter being `pub` isn't a dead knob nothing can reach.
#[tokio::test]
async fn manually_composed_stack_uses_finalized_tenant_after_switch() {
    MANUAL_COMPOSITION_HANDLER_CALLS.store(0, Ordering::SeqCst);

    let mut config = AutumnConfig::default();
    config.tenancy.enabled = true;
    "session".clone_into(&mut config.tenancy.source);

    let state = AppState::for_test();
    state.insert_extension(config);

    let session_store = SessionMemoryStore::new();
    let mut seed = std::collections::HashMap::new();
    seed.insert("tenant_id".to_owned(), "org-a".to_owned());
    session_store
        .save("manual-session", seed)
        .await
        .expect("seeding the session store must succeed");

    let idempotency_store: Arc<dyn IdempotencyStore> = Arc::new(MemoryIdempotencyStore::new(
        std::time::Duration::from_secs(60),
    ));

    let app = axum::Router::new()
        .route(
            "/switch-org",
            axum::routing::post(manual_composition_switch_handler),
        )
        .layer(IdempotencyLayer::new(idempotency_store))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            tenancy_middleware,
        ))
        .layer(
            SessionLayer::new(session_store, SessionConfig::default())
                .with_tenancy_session_key(Some(Arc::from("tenant_id"))),
        );

    let request = || {
        axum::http::Request::builder()
            .method(axum::http::Method::POST)
            .uri("/switch-org")
            .header("idempotency-key", "manual-switch-key")
            .header("cookie", "autumn.sid=manual-session")
            .body(Body::empty())
            .expect("request must build")
    };

    let first = app.clone().oneshot(request()).await.unwrap();
    assert_eq!(first.status(), axum::http::StatusCode::OK);
    let body = axum::body::to_bytes(first.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(&body[..], b"switched-1");

    let retry = app.oneshot(request()).await.unwrap();
    assert_eq!(retry.status(), axum::http::StatusCode::OK);
    let retry_body = axum::body::to_bytes(retry.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(
        &retry_body[..],
        b"switched-1",
        "a retry after an org switch must replay through a manually composed SessionLayer + \
         tenancy_middleware + IdempotencyLayer stack too, not just AppBuilder's"
    );
}

static RACE_HANDLER_CALLS: AtomicUsize = AtomicUsize::new(0);

async fn race_switch_handler(session: Session) -> String {
    let calls = RACE_HANDLER_CALLS.fetch_add(1, Ordering::SeqCst) + 1;
    session.insert("tenant_id", "org-b").await;
    format!("switched-{calls}")
}

/// [`SessionStore`] decorator that parks request 1's dirty-session `save`
/// *after* the inner save has landed but before the response is finalized.
/// At the park point the switched session is visible (the new tenant resolves)
/// while the idempotency commit hasn't run yet — exactly the window issue
/// #2455 describes. Fully async: no worker thread is ever blocked.
#[derive(Clone)]
struct GatedSessionStore {
    inner: SessionMemoryStore,
    entered_tx: Arc<Mutex<Option<tokio::sync::mpsc::UnboundedSender<()>>>>,
    release: Arc<tokio::sync::Notify>,
    gate_armed: Arc<AtomicBool>,
}

impl SessionStore for GatedSessionStore {
    fn load(
        &self,
        id: &str,
    ) -> impl Future<
        Output = Result<Option<std::collections::HashMap<String, String>>, SessionStoreError>,
    > + Send {
        self.inner.load(id)
    }

    fn save(
        &self,
        id: &str,
        data: std::collections::HashMap<String, String>,
    ) -> impl Future<Output = Result<(), SessionStoreError>> + Send {
        async move {
            let result = self.inner.save(id, data).await;
            if self.gate_armed.swap(false, Ordering::SeqCst) {
                if let Some(tx) = self.entered_tx.lock().unwrap().take() {
                    let _ = tx.send(());
                }
                // Park here: the save landed (new tenant visible) but the
                // Set-Cookie / alias registration / commit haven't run.
                self.release.notified().await;
            }
            result
        }
    }

    fn destroy(&self, id: &str) -> impl Future<Output = Result<(), SessionStoreError>> + Send {
        self.inner.destroy(id)
    }
}

/// Issue #2455: the deferred commit's session-alias key had no in-flight lock
/// between the session `save` (which makes the finalized tenant visible) and
/// the commit writes. A concurrent second request presenting the same
/// unrotated session cookie resolves the new tenant, computes the alias key,
/// finds no record and no lock, and re-runs the handler.
///
/// The session layer now reserves the alias lock *before* persisting the
/// session, so the concurrent request must observe a 409 in-flight conflict
/// instead of a second execution.
#[tokio::test]
async fn concurrent_retry_during_commit_does_not_rerun_handler_after_org_switch() {
    RACE_HANDLER_CALLS.store(0, Ordering::SeqCst);

    let mut config = AutumnConfig::default();
    config.tenancy.enabled = true;
    "session".clone_into(&mut config.tenancy.source);

    let state = AppState::for_test();
    state.insert_extension(config);

    // Seed the inner store directly so the seed write doesn't trip the gate.
    let inner_session_store = SessionMemoryStore::new();
    let mut seed = std::collections::HashMap::new();
    seed.insert("tenant_id".to_owned(), "org-a".to_owned());
    inner_session_store
        .save("race-session", seed)
        .await
        .expect("seeding the session store must succeed");

    let (entered_tx, mut entered_rx) = tokio::sync::mpsc::unbounded_channel();
    let release = Arc::new(tokio::sync::Notify::new());
    let session_store = GatedSessionStore {
        inner: inner_session_store,
        entered_tx: Arc::new(Mutex::new(Some(entered_tx))),
        release: release.clone(),
        gate_armed: Arc::new(AtomicBool::new(true)),
    };

    let idempotency_store: Arc<dyn IdempotencyStore> =
        Arc::new(MemoryIdempotencyStore::new(Duration::from_secs(60)));

    let app = axum::Router::new()
        .route("/switch-org", axum::routing::post(race_switch_handler))
        .layer(IdempotencyLayer::new(idempotency_store))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            tenancy_middleware,
        ))
        .layer(
            SessionLayer::new(session_store, SessionConfig::default())
                .with_tenancy_session_key(Some(Arc::from("tenant_id"))),
        );

    let request = || {
        axum::http::Request::builder()
            .method(axum::http::Method::POST)
            .uri("/switch-org")
            .header("idempotency-key", "race-switch-key")
            .header("cookie", "autumn.sid=race-session")
            .body(Body::empty())
            .expect("request must build")
    };

    // Request 1 runs the handler (switching the session tenant to "org-b")
    // and parks after its session save lands, before the response is
    // finalized. The alias in-flight lock is held from before the save.
    let first = tokio::spawn(app.clone().oneshot(request()));
    let entered = tokio::time::timeout(Duration::from_secs(10), entered_rx.recv()).await;
    assert!(
        entered.is_ok(),
        "request 1 must persist its session (handler calls so far: {})",
        RACE_HANDLER_CALLS.load(Ordering::SeqCst)
    );

    // Request 2 presents the same session (now resolving "org-b") and the
    // same idempotency key while request 1 is parked: the alias key it
    // computes is locked, so it must take 409 — not re-run the handler.
    let second = app.clone().oneshot(request()).await.unwrap();
    assert_eq!(
        second.status(),
        axum::http::StatusCode::CONFLICT,
        "a concurrent retry that lands on the in-flight alias key must get 409, \
         not a second handler execution"
    );

    // Release request 1; it finishes normally.
    release.notify_one();
    let first = first.await.expect("request 1 must complete").unwrap();
    assert_eq!(first.status(), axum::http::StatusCode::OK);
    let body = axum::body::to_bytes(first.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(&body[..], b"switched-1");

    assert_eq!(
        RACE_HANDLER_CALLS.load(Ordering::SeqCst),
        1,
        "the handler must run exactly once across the racing requests"
    );

    // A retry after the commit finished replays through the alias key.
    let retry = app.oneshot(request()).await.unwrap();
    assert_eq!(retry.status(), axum::http::StatusCode::OK);
    assert_eq!(
        retry
            .headers()
            .get("x-idempotent-replayed")
            .and_then(|v| v.to_str().ok()),
        Some("true"),
        "a post-commit retry under the switched tenant must replay the alias record"
    );
    let retry_body = axum::body::to_bytes(retry.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(&retry_body[..], b"switched-1");
}
