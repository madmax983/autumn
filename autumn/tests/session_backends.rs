use autumn_web::config::{AutumnConfig, ConfigError, MockEnv};
#[cfg(not(feature = "redis"))]
use autumn_web::session::SessionBackendConfigError;
use autumn_web::session::{SessionBackend, SessionBackendPlan, SessionConfig};

#[test]
fn session_backends_config_selects_backend_from_env() {
    let mut config = AutumnConfig::default();
    config.apply_env_overrides_with_env(
        &MockEnv::new()
            .with("AUTUMN_SESSION__BACKEND", "redis")
            .with("AUTUMN_SESSION__REDIS__URL", "redis://127.0.0.1:6379")
            .with("AUTUMN_SESSION__REDIS__KEY_PREFIX", "autumn:test:sessions"),
    );

    assert_eq!(config.session.backend, SessionBackend::Redis);
    assert_eq!(
        config.session.redis.url.as_deref(),
        Some("redis://127.0.0.1:6379")
    );
    assert_eq!(config.session.redis.key_prefix, "autumn:test:sessions");

    #[cfg(feature = "redis")]
    assert_eq!(
        config.session.backend_plan(None).unwrap(),
        SessionBackendPlan::Redis {
            url: "redis://127.0.0.1:6379".to_owned(),
            key_prefix: "autumn:test:sessions".to_owned(),
        }
    );

    #[cfg(not(feature = "redis"))]
    assert_eq!(
        config.session.backend_plan(None).unwrap_err(),
        SessionBackendConfigError::RedisFeatureDisabled
    );
}

#[test]
fn session_backends_memory_requires_explicit_prod_acknowledgement() {
    let unacknowledged = SessionConfig::default().backend_plan(Some("prod")).unwrap();
    assert_eq!(
        unacknowledged,
        SessionBackendPlan::Memory {
            warn_in_production: true
        }
    );

    let mut acknowledged = SessionConfig::default();
    acknowledged.allow_memory_in_production = true;
    assert_eq!(
        acknowledged.backend_plan(Some("prod")).unwrap(),
        SessionBackendPlan::Memory {
            warn_in_production: false
        }
    );
}

#[test]
fn session_backends_env_overrides_include_http_only_and_path() {
    let mut config = AutumnConfig::default();
    config.apply_env_overrides_with_env(
        &MockEnv::new()
            .with("AUTUMN_SESSION__HTTP_ONLY", "false")
            .with("AUTUMN_SESSION__PATH", "/app"),
    );

    assert!(!config.session.http_only);
    assert_eq!(config.session.path, "/app");
}

#[test]
fn session_backends_load_from_validates_redis_config() {
    let temp_dir = tempfile::tempdir().unwrap();
    let config_path = temp_dir.path().join("autumn.toml");
    std::fs::write(&config_path, "[session]\nbackend = \"redis\"\n").unwrap();

    let error = AutumnConfig::load_from(&config_path).unwrap_err();
    assert!(matches!(error, ConfigError::Validation(_)));
    assert!(error.to_string().contains("session.redis.url"));
}

#[cfg(feature = "redis")]
use autumn_web::AppState;
#[cfg(feature = "redis")]
use autumn_web::route::Route;
#[cfg(feature = "redis")]
use autumn_web::router;
#[cfg(feature = "redis")]
use autumn_web::session::Session;
#[cfg(feature = "redis")]
use axum::body::Body;
#[cfg(feature = "redis")]
use axum::http::header::{COOKIE, SET_COOKIE};
#[cfg(feature = "redis")]
use axum::http::{Method, Request};
#[cfg(feature = "redis")]
use axum::routing::get;
#[cfg(feature = "redis")]
use tower::ServiceExt;

#[cfg(feature = "redis")]
fn redis_test_config(url: String) -> AutumnConfig {
    let mut config = AutumnConfig::default();
    config.session.backend = SessionBackend::Redis;
    config.session.redis.url = Some(url);
    config.session.redis.key_prefix = format!("autumn:test:{}", uuid::Uuid::new_v4());
    config
}

#[cfg(feature = "redis")]
#[tokio::test]
async fn session_backends_redis_preserves_sessions_across_app_instances() {
    use testcontainers_modules::redis::{REDIS_PORT, Redis};
    use testcontainers_modules::testcontainers::runners::AsyncRunner;

    async fn write_handler(session: Session) -> &'static str {
        session.insert("user", "alice").await;
        "saved"
    }

    async fn read_handler(session: Session) -> String {
        session.get("user").await.unwrap_or_default()
    }

    let redis = Redis::default().start().await.unwrap();
    let host = redis.get_host().await.unwrap();
    let port = redis.get_host_port_ipv4(REDIS_PORT).await.unwrap();
    let config = redis_test_config(format!("redis://{host}:{port}"));

    let app1 = router::build_router(
        vec![
            Route {
                method: Method::GET,
                path: "/write",
                handler: get(write_handler),
                name: "/write",
            },
            Route {
                method: Method::GET,
                path: "/read",
                handler: get(read_handler),
                name: "/read",
            },
        ],
        &config,
        AppState::for_test(),
    );
    let app2 = router::build_router(
        vec![
            Route {
                method: Method::GET,
                path: "/write",
                handler: get(write_handler),
                name: "/write",
            },
            Route {
                method: Method::GET,
                path: "/read",
                handler: get(read_handler),
                name: "/read",
            },
        ],
        &config,
        AppState::for_test(),
    );

    let response = app1
        .oneshot(
            Request::builder()
                .uri("/write")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let cookie = response
        .headers()
        .get(SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();

    let response = app2
        .oneshot(
            Request::builder()
                .uri("/read")
                .header(COOKIE, cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(std::str::from_utf8(&body).unwrap(), "alice");
}
