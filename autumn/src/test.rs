//! First-party integration-testing utilities for Autumn applications.
//!
//! This module brings Autumn's testing story to parity with frameworks like
//! Spring Boot's `@SpringBootTest` + `MockMvc` and Django's `TestCase` +
//! `Client`. Import it in your integration tests:
//!
//! ```rust,ignore
//! use autumn_web::test::{TestApp, TestClient};
//! ```
//!
//! # Quick start
//!
//! ```rust,no_run
//! use autumn_web::prelude::*;
//! use autumn_web::test::TestApp;
//!
//! #[get("/hello")]
//! async fn hello() -> &'static str { "hi" }
//!
//! #[tokio::test]
//! async fn hello_returns_200() {
//!     let client = TestApp::new()
//!         .routes(routes![hello])
//!         .build();
//!
//!     client.get("/hello").send().await
//!         .assert_status(200)
//!         .assert_body_contains("hi");
//! }
//! ```
//!
//! # What's included
//!
//! | Type | Spring Boot equivalent | Purpose |
//! |------|----------------------|---------|
//! | [`TestApp`] | `@SpringBootTest` | Boot a fully-configured app for testing |
//! | [`TestClient`] | `MockMvc` / `WebTestClient` | Fluent HTTP request builder |
//! | [`TestResponse`] | `MvcResult` | Response with assertion helpers |
//! | `TestDb` | `@DataJpaTest` | Shared Postgres testcontainer with pool |
//!
//! # Database testing
//!
//! For tests that need a real database, use `TestDb` to share a single
//! Postgres container across your test suite (rather than one per test):
//!
//! ```rust,ignore
//! use autumn_web::test::{TestApp, TestDb};
//!
//! #[tokio::test]
//! async fn creates_user_in_db() {
//!     let db = TestDb::shared().await;
//!     let client = TestApp::new()
//!         .routes(routes![create_user, get_user])
//!         .with_db(db.pool())
//!         .build();
//!
//!     client.post("/users")
//!         .json(&serde_json::json!({"name": "Alice"}))
//!         .send().await
//!         .assert_status(201);
//! }
//! ```

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use tower::ServiceExt;

use crate::config::AutumnConfig;
use crate::route::Route;

use crate::state::AppState;

#[cfg(feature = "db")]
use diesel_async::AsyncPgConnection;
#[cfg(feature = "db")]
use diesel_async::pooled_connection::deadpool::Pool;

// ── TestApp ────────────────────────────────────────────────────

/// Builder for constructing a fully-configured Autumn application in tests.
///
/// Analogous to Spring Boot's `@SpringBootTest` -- it wires up routes,
/// middleware, config, and optionally a database pool, then produces a
/// [`TestClient`] ready to fire requests.
///
/// # Examples
///
/// ```rust,no_run
/// use autumn_web::prelude::*;
/// use autumn_web::test::TestApp;
///
/// #[get("/ping")]
/// async fn ping() -> &'static str { "pong" }
///
/// #[tokio::test]
/// async fn ping_works() {
///     let client = TestApp::new()
///         .routes(routes![ping])
///         .build();
///
///     client.get("/ping").send().await.assert_ok();
/// }
/// ```
pub struct TestApp {
    routes: Vec<Route>,
    merge_routers: Vec<axum::Router<crate::state::AppState>>,
    nest_routers: Vec<(String, axum::Router<crate::state::AppState>)>,
    custom_layers: Vec<crate::app::CustomLayerRegistration>,
    config: AutumnConfig,
    #[cfg(feature = "openapi")]
    openapi: Option<crate::openapi::OpenApiConfig>,
    #[cfg(feature = "db")]
    pool: Option<Pool<AsyncPgConnection>>,
    /// Deferred policy / scope registrations applied during
    /// [`TestApp::build`].
    policy_registrations: Vec<TestPolicyRegistration>,
    /// Override for [`AppState::forbidden_response`]. Defaults to
    /// the value derived from
    /// [`SecurityConfig::forbidden_response`](crate::security::SecurityConfig::forbidden_response).
    forbidden_response_override: Option<crate::authorization::ForbiddenResponse>,
}

type TestPolicyRegistration =
    Box<dyn FnOnce(&crate::authorization::PolicyRegistry) + Send>;

impl TestApp {
    /// Create a new test app builder with default configuration.
    #[must_use]
    pub fn new() -> Self {
        let mut config = AutumnConfig::default();
        config.profile = Some("test".into());
        // Disable CSRF for tests by default (like Spring Security's test support)
        config.security.csrf.enabled = false;

        Self {
            routes: Vec::new(),
            merge_routers: Vec::new(),
            nest_routers: Vec::new(),
            custom_layers: Vec::new(),
            config,
            #[cfg(feature = "openapi")]
            openapi: None,
            #[cfg(feature = "db")]
            pool: None,
            policy_registrations: Vec::new(),
            forbidden_response_override: None,
        }
    }

    /// Register a [`Policy`](crate::authorization::Policy) for
    /// resource type `R`. Mirrors
    /// [`AppBuilder::policy`](crate::app::AppBuilder::policy).
    #[must_use]
    pub fn policy<R, P>(mut self, policy: P) -> Self
    where
        R: Send + Sync + 'static,
        P: crate::authorization::Policy<R>,
    {
        self.policy_registrations.push(Box::new(move |registry| {
            registry.register_policy::<R, _>(policy);
        }));
        self
    }

    /// Register a [`Scope`](crate::authorization::Scope) for resource
    /// type `R`. Mirrors
    /// [`AppBuilder::scope`](crate::app::AppBuilder::scope).
    #[must_use]
    pub fn scope<R, S>(mut self, scope: S) -> Self
    where
        R: Send + Sync + 'static,
        S: crate::authorization::Scope<R>,
    {
        self.policy_registrations.push(Box::new(move |registry| {
            registry.register_scope::<R, _>(scope);
        }));
        self
    }

    /// Override the deny-response shape used by `#[authorize]` and
    /// `#[repository(policy = ...)]` handlers. Useful for
    /// round-tripping the `403`-vs-`404` decision in tests.
    #[must_use]
    pub const fn forbidden_response(
        mut self,
        value: crate::authorization::ForbiddenResponse,
    ) -> Self {
        self.forbidden_response_override = Some(value);
        self
    }

    /// Enable `OpenAPI` spec generation for the test app.
    ///
    /// Mirrors [`crate::app::AppBuilder::openapi`] so integration tests
    /// can exercise the `/v3/api-docs` and `/swagger-ui` endpoints.
    ///
    /// Gated behind the `openapi` Cargo feature.
    #[cfg(feature = "openapi")]
    #[must_use]
    pub fn openapi(mut self, config: crate::openapi::OpenApiConfig) -> Self {
        self.openapi = Some(config);
        self
    }

    /// Merge a router into the internal application state.
    ///
    /// This is useful when testing modular route definitions without building
    /// the full application.
    #[must_use]
    pub fn merge(mut self, router: axum::Router<crate::state::AppState>) -> Self {
        self.merge_routers.push(router);
        self
    }

    /// Nest a router under a specific path prefix for testing.
    ///
    /// This is useful for testing sub-applications or API versions.
    #[must_use]
    pub fn nest(mut self, path: &str, router: axum::Router<crate::state::AppState>) -> Self {
        self.nest_routers.push((path.to_owned(), router));
        self
    }

    /// Apply a custom [`tower::Layer`] to the entire test application.
    ///
    /// Mirrors [`crate::app::AppBuilder::layer`] so tests can exercise the
    /// exact middleware wiring that `AppBuilder::run()` produces.
    #[must_use]
    pub fn layer<L: crate::app::IntoAppLayer>(mut self, layer: L) -> Self {
        self.custom_layers
            .push(crate::app::CustomLayerRegistration {
                type_id: std::any::TypeId::of::<L>(),
                apply: Box::new(move |router| layer.apply_to(router)),
            });
        self
    }

    /// Construct a [`TestClient`] directly from an `axum::Router`.
    ///
    /// Useful for bypassing `TestApp` builder if you just want to write requests
    /// against a standard axum Router.
    #[must_use]
    pub const fn from_router(router: axum::Router) -> TestClient {
        TestClient { router }
    }

    /// Register a collection of routes to be built into the `TestApp`.
    #[must_use]
    pub fn routes(mut self, routes: Vec<Route>) -> Self {
        self.routes.extend(routes);
        self
    }

    /// Override the default test configuration.
    #[must_use]
    pub fn config(mut self, config: AutumnConfig) -> Self {
        self.config = config;
        self
    }

    /// Set the active profile (default is `"test"`).
    #[must_use]
    pub fn profile(mut self, profile: &str) -> Self {
        self.config.profile = Some(profile.to_owned());
        self
    }

    /// Attach a database connection pool to the test app.
    #[cfg(feature = "db")]
    #[must_use]
    pub fn with_db(mut self, pool: Pool<AsyncPgConnection>) -> Self {
        self.pool = Some(pool);
        self
    }

    /// Build the application and return a [`TestClient`] ready for requests.
    ///
    /// This constructs the full Axum router with all middleware applied,
    /// identical to what `AppBuilder::run()` produces -- without binding
    /// a TCP listener.
    #[must_use]
    pub fn build(self) -> TestClient {
        let state = AppState {
            extensions: std::sync::Arc::new(std::sync::RwLock::new(
                std::collections::HashMap::new(),
            )),
            #[cfg(feature = "db")]
            pool: self.pool,
            profile: self.config.profile.clone(),
            started_at: std::time::Instant::now(),
            health_detailed: self.config.health.detailed,
            probes: crate::probe::ProbeState::ready_for_test(),
            metrics: crate::middleware::MetricsCollector::new(),
            log_levels: crate::actuator::LogLevels::new(&self.config.log.level),
            task_registry: crate::actuator::TaskRegistry::new(),
            config_props: crate::actuator::ConfigProperties::default(),
            #[cfg(feature = "ws")]
            channels: crate::channels::Channels::new(32),
            #[cfg(feature = "ws")]
            shutdown: tokio_util::sync::CancellationToken::new(),
            policy_registry: crate::authorization::PolicyRegistry::default(),
            forbidden_response: self
                .forbidden_response_override
                .unwrap_or(self.config.security.forbidden_response),
            auth_session_key: self.config.auth.session_key.clone(),
        };

        for register in self.policy_registrations {
            register(state.policy_registry());
        }

        let router = crate::router::try_build_router_inner(
            self.routes,
            &self.config,
            state,
            crate::router::RouterContext {
                exception_filters: Vec::new(),
                scoped_groups: Vec::new(),
                merge_routers: self.merge_routers,
                nest_routers: self.nest_routers,
                custom_layers: self.custom_layers,
                error_page_renderer: None,
                session_store: None,
                #[cfg(feature = "openapi")]
                openapi: self.openapi,
            },
        )
        .expect("failed to build test router");
        TestClient { router }
    }
}

impl Default for TestApp {
    fn default() -> Self {
        Self::new()
    }
}

// ── TestClient ─────────────────────────────────────────────────

/// Fluent HTTP client for integration tests.
///
/// Analogous to Spring Boot's `MockMvc` or Django's `Client`.
/// Fires requests through the full Axum middleware pipeline using
/// `tower::ServiceExt::oneshot()` -- no TCP listener required.
///
/// Created by [`TestApp::build()`].
///
/// # Examples
///
/// ```rust,ignore
/// let client = TestApp::new().routes(routes![handler]).build();
///
/// // GET request
/// client.get("/path").send().await.assert_ok();
///
/// // POST with JSON body
/// client.post("/items")
///     .json(&serde_json::json!({"name": "foo"}))
///     .send().await
///     .assert_status(201);
///
/// // PUT with header
/// client.put("/items/1")
///     .header("authorization", "Bearer token")
///     .json(&serde_json::json!({"name": "bar"}))
///     .send().await
///     .assert_ok();
/// ```
pub struct TestClient {
    router: axum::Router,
}

impl TestClient {
    /// Unwrap the underlying [`axum::Router`] out of the [`TestClient`].
    pub fn into_router(self) -> axum::Router {
        self.router
    }

    /// Start building a GET request.
    #[must_use]
    pub fn get(&self, uri: &str) -> RequestBuilder {
        RequestBuilder::new(self.router.clone(), Method::GET, uri)
    }

    /// Start building a POST request.
    #[must_use]
    pub fn post(&self, uri: &str) -> RequestBuilder {
        RequestBuilder::new(self.router.clone(), Method::POST, uri)
    }

    /// Start building a PUT request.
    #[must_use]
    pub fn put(&self, uri: &str) -> RequestBuilder {
        RequestBuilder::new(self.router.clone(), Method::PUT, uri)
    }

    /// Start building a DELETE request.
    #[must_use]
    pub fn delete(&self, uri: &str) -> RequestBuilder {
        RequestBuilder::new(self.router.clone(), Method::DELETE, uri)
    }

    /// Start building a PATCH request.
    #[must_use]
    pub fn patch(&self, uri: &str) -> RequestBuilder {
        RequestBuilder::new(self.router.clone(), Method::PATCH, uri)
    }
}

// ── RequestBuilder ─────────────────────────────────────────────

/// Fluent builder for composing an HTTP request in tests.
///
/// Created by [`TestClient::get()`], [`TestClient::post()`], etc.
/// Call [`.send()`](Self::send) to fire the request and get a
/// [`TestResponse`].
pub struct RequestBuilder {
    router: axum::Router,
    method: Method,
    uri: String,
    headers: Vec<(String, String)>,
    body: Body,
}

impl RequestBuilder {
    fn new(router: axum::Router, method: Method, uri: &str) -> Self {
        Self {
            router,
            method,
            uri: uri.to_owned(),
            headers: Vec::new(),
            body: Body::empty(),
        }
    }

    /// Add a header to the request.
    #[must_use]
    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_owned(), value.to_owned()));
        self
    }

    /// Set the request body to a JSON-serialized value.
    ///
    /// Automatically sets `Content-Type: application/json`.
    #[must_use]
    pub fn json(mut self, value: &serde_json::Value) -> Self {
        self.headers
            .push(("content-type".to_owned(), "application/json".to_owned()));
        self.body = Body::from(serde_json::to_vec(value).expect("failed to serialize JSON body"));
        self
    }

    /// Set the request body to URL-encoded form data.
    ///
    /// Automatically sets `Content-Type: application/x-www-form-urlencoded`.
    #[must_use]
    pub fn form(mut self, body: &str) -> Self {
        self.headers.push((
            "content-type".to_owned(),
            "application/x-www-form-urlencoded".to_owned(),
        ));
        self.body = Body::from(body.to_owned());
        self
    }

    /// Set a raw string body.
    #[must_use]
    pub fn body(mut self, body: impl Into<Body>) -> Self {
        self.body = body.into();
        self
    }

    /// Fire the request through the full middleware pipeline and return
    /// a [`TestResponse`].
    pub async fn send(self) -> TestResponse {
        let mut builder = Request::builder().method(self.method).uri(&self.uri);

        for (name, value) in &self.headers {
            builder = builder.header(name.as_str(), value.as_str());
        }

        let request = builder.body(self.body).expect("failed to build request");

        let response = self.router.oneshot(request).await.expect("request failed");

        let status = response.status();
        let headers: Vec<(String, String)> = response
            .headers()
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_owned()))
            .collect();
        let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("failed to read response body");

        TestResponse {
            status,
            headers,
            body: body_bytes.to_vec(),
        }
    }
}

// ── TestResponse ───────────────────────────────────────────────

/// HTTP response from a test request with fluent assertion helpers.
///
/// All assertion methods return `&Self` for chaining:
///
/// ```rust,ignore
/// client.get("/users/1").send().await
///     .assert_ok()
///     .assert_header("content-type", "application/json")
///     .assert_body_contains("Alice");
/// ```
pub struct TestResponse {
    /// HTTP status code.
    pub status: StatusCode,
    /// Response headers as `(name, value)` pairs.
    pub headers: Vec<(String, String)>,
    /// Raw response body bytes.
    pub body: Vec<u8>,
}

impl TestResponse {
    /// Get the response body as a UTF-8 string.
    ///
    /// # Panics
    ///
    /// Panics if the body is not valid UTF-8.
    #[must_use]
    pub fn text(&self) -> String {
        String::from_utf8(self.body.clone()).expect("response body is not valid UTF-8")
    }

    /// Deserialize the response body as JSON.
    ///
    /// # Panics
    ///
    /// Panics if the body is not valid JSON or cannot be deserialized
    /// into `T`.
    #[must_use]
    pub fn json<T: serde::de::DeserializeOwned>(&self) -> T {
        serde_json::from_slice(&self.body).expect("failed to parse response body as JSON")
    }

    /// Get the value of a response header.
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        let name_lower = name.to_lowercase();
        self.headers
            .iter()
            .find(|(k, _)| k.to_lowercase() == name_lower)
            .map(|(_, v)| v.as_str())
    }

    // ── Assertion helpers ──────────────────────────────────────

    /// Assert the response status is 200 OK.
    #[track_caller]
    pub fn assert_ok(&self) -> &Self {
        assert_eq!(
            self.status,
            StatusCode::OK,
            "expected 200 OK, got {}.\nBody: {}",
            self.status,
            String::from_utf8_lossy(&self.body)
        );
        self
    }

    /// Assert the response status matches the given code.
    #[track_caller]
    pub fn assert_status(&self, expected: u16) -> &Self {
        assert_eq!(
            self.status.as_u16(),
            expected,
            "expected status {expected}, got {}.\nBody: {}",
            self.status,
            String::from_utf8_lossy(&self.body)
        );
        self
    }

    /// Assert the response status indicates a successful request (2xx).
    #[track_caller]
    pub fn assert_success(&self) -> &Self {
        assert!(
            self.status.is_success(),
            "expected 2xx success, got {}.\nBody: {}",
            self.status,
            String::from_utf8_lossy(&self.body)
        );
        self
    }

    /// Assert a response header exists and equals the expected value.
    #[track_caller]
    pub fn assert_header(&self, name: &str, expected: &str) -> &Self {
        let value = self
            .header(name)
            .unwrap_or_else(|| panic!("expected header `{name}` to be present"));
        assert_eq!(
            value, expected,
            "header `{name}`: expected `{expected}`, got `{value}`"
        );
        self
    }

    /// Assert a response header exists and contains the expected substring.
    #[track_caller]
    pub fn assert_header_contains(&self, name: &str, substring: &str) -> &Self {
        let value = self
            .header(name)
            .unwrap_or_else(|| panic!("expected header `{name}` to be present"));
        assert!(
            value.contains(substring),
            "header `{name}`: expected `{value}` to contain `{substring}`"
        );
        self
    }

    /// Assert the response body contains the given substring.
    #[track_caller]
    pub fn assert_body_contains(&self, substring: &str) -> &Self {
        let body = self.text();
        assert!(
            body.contains(substring),
            "expected body to contain `{substring}`.\nBody: {body}"
        );
        self
    }

    /// Assert the response body exactly equals the given string.
    #[track_caller]
    pub fn assert_body_eq(&self, expected: &str) -> &Self {
        let body = self.text();
        assert_eq!(body, expected, "body mismatch");
        self
    }

    /// Assert the response body deserializes to JSON matching the predicate.
    #[track_caller]
    pub fn assert_json<T, F>(&self, predicate: F) -> &Self
    where
        T: serde::de::DeserializeOwned,
        F: FnOnce(&T),
    {
        let value: T = self.json();
        predicate(&value);
        self
    }

    /// Assert the response body is empty.
    #[track_caller]
    pub fn assert_body_empty(&self) -> &Self {
        assert!(
            self.body.is_empty(),
            "expected empty body, got {} bytes: {}",
            self.body.len(),
            String::from_utf8_lossy(&self.body)
        );
        self
    }
}

// ── TestDb ─────────────────────────────────────────────────────

/// Shared Postgres testcontainer for database integration tests.
///
/// Rather than spinning up a new container per test (slow!), `TestDb`
/// provides a shared container that all tests in a binary can reuse.
/// This mirrors Spring Boot's `@Testcontainers` with `@Container` +
/// `static` pattern.
///
/// Requires the `test-support` feature (and `db`):
///
/// ```toml
/// [dev-dependencies]
/// autumn-web = { path = "..", features = ["test-support"] }
/// ```
///
/// # Examples
///
/// ```rust,ignore
/// use autumn_web::test::{TestApp, TestDb};
///
/// #[tokio::test]
/// #[ignore = "requires Docker"]
/// async fn db_test() {
///     let db = TestDb::shared().await;
///     let client = TestApp::new()
///         .routes(routes![my_handler])
///         .with_db(db.pool())
///         .build();
///
///     // Run migrations or seed data via db.pool()
///     client.get("/data").send().await.assert_ok();
/// }
/// ```
#[cfg(all(feature = "db", feature = "test-support"))]
pub struct TestDb {
    _container: testcontainers::ContainerAsync<testcontainers_modules::postgres::Postgres>,
    pool: Pool<AsyncPgConnection>,
    url: String,
}

#[cfg(all(feature = "db", feature = "test-support"))]
impl TestDb {
    /// Start a new Postgres testcontainer and create a connection pool.
    ///
    /// For most test suites, prefer [`TestDb::shared()`] to reuse a
    /// single container across all tests.
    pub async fn new() -> Self {
        use diesel_async::pooled_connection::AsyncDieselConnectionManager;
        use testcontainers::runners::AsyncRunner;
        use testcontainers_modules::postgres::Postgres;

        let container = Postgres::default()
            .start()
            .await
            .expect("failed to start Postgres testcontainer (is Docker running?)");

        let host = container
            .get_host()
            .await
            .expect("failed to build test router");
        let port = container
            .get_host_port_ipv4(5432)
            .await
            .expect("failed to build test router");
        let url = format!("postgres://postgres:postgres@{host}:{port}/postgres");

        let manager = AsyncDieselConnectionManager::<AsyncPgConnection>::new(&url);
        let pool = Pool::builder(manager)
            .max_size(5)
            .build()
            .expect("failed to build connection pool");

        Self {
            _container: container,
            pool,
            url,
        }
    }

    /// Get a shared `TestDb` instance, starting the container on first use.
    ///
    /// Uses a process-global `OnceLock` so the container is started only
    /// once per test binary, regardless of how many tests call this method.
    /// This dramatically speeds up test suites with multiple DB tests.
    ///
    /// The container is automatically cleaned up when the process exits.
    pub async fn shared() -> &'static Self {
        use std::sync::OnceLock;
        use tokio::sync::OnceCell;

        // Two-phase init: OnceLock for the OnceCell, OnceCell for the async init.
        static CELL: OnceLock<OnceCell<TestDb>> = OnceLock::new();
        let once = CELL.get_or_init(OnceCell::new);
        once.get_or_init(Self::new).await
    }

    /// Get the database connection pool.
    #[must_use]
    pub fn pool(&self) -> Pool<AsyncPgConnection> {
        self.pool.clone()
    }

    /// Get the Postgres connection URL.
    #[must_use]
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Execute raw SQL against the test database.
    ///
    /// Useful for creating tables, seeding data, or running migrations
    /// in tests.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// let db = TestDb::shared().await;
    /// db.execute_sql("CREATE TABLE IF NOT EXISTS users (id SERIAL PRIMARY KEY, name TEXT NOT NULL)")
    ///     .await;
    /// ```
    pub async fn execute_sql(&self, sql: &str) {
        use diesel_async::RunQueryDsl;
        let mut conn = self.pool.get().await.expect("failed to get connection");
        diesel::sql_query(sql)
            .execute(&mut *conn)
            .await
            .unwrap_or_else(|e| panic!("SQL execution failed: {e}\nSQL: {sql}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_routes() -> Vec<Route> {
        use axum::routing;

        async fn hello() -> &'static str {
            "hello"
        }

        async fn echo_json(
            axum::Json(value): axum::Json<serde_json::Value>,
        ) -> axum::Json<serde_json::Value> {
            axum::Json(value)
        }

        async fn status_201() -> (StatusCode, &'static str) {
            (StatusCode::CREATED, "created")
        }

        vec![
            Route {
                method: Method::GET,
                path: "/hello",
                handler: routing::get(hello),
                name: "hello",
                api_doc: crate::openapi::ApiDoc {
                    method: "GET",
                    path: "/hello",
                    operation_id: "hello",
                    success_status: 200,
                    ..Default::default()
                },
                repository: None,
            },
            Route {
                method: Method::POST,
                path: "/echo",
                handler: routing::post(echo_json),
                name: "echo",
                api_doc: crate::openapi::ApiDoc {
                    method: "POST",
                    path: "/echo",
                    operation_id: "echo",
                    success_status: 200,
                    ..Default::default()
                },
                repository: None,
            },
            Route {
                method: Method::POST,
                path: "/create",
                handler: routing::post(status_201),
                name: "create",
                api_doc: crate::openapi::ApiDoc {
                    method: "POST",
                    path: "/create",
                    operation_id: "create",
                    success_status: 201,
                    ..Default::default()
                },
                repository: None,
            },
        ]
    }

    #[tokio::test]
    async fn test_app_get_request() {
        let client = TestApp::new().routes(test_routes()).build();
        client.get("/hello").send().await.assert_ok();
    }

    #[tokio::test]
    async fn test_app_post_json() {
        let client = TestApp::new().routes(test_routes()).build();

        client
            .post("/echo")
            .json(&serde_json::json!({"key": "value"}))
            .send()
            .await
            .assert_ok()
            .assert_body_contains("key");
    }

    #[tokio::test]
    async fn test_response_assert_status() {
        let client = TestApp::new().routes(test_routes()).build();

        client
            .post("/create")
            .send()
            .await
            .assert_status(201)
            .assert_body_eq("created");
    }

    #[tokio::test]
    async fn test_response_assert_success() {
        let client = TestApp::new().routes(test_routes()).build();
        client.get("/hello").send().await.assert_success();
    }

    #[tokio::test]
    async fn test_not_found() {
        let client = TestApp::new().routes(test_routes()).build();
        client.get("/nonexistent").send().await.assert_status(404);
    }

    #[tokio::test]
    async fn test_response_json_deserialization() {
        let client = TestApp::new().routes(test_routes()).build();

        let resp = client
            .post("/echo")
            .json(&serde_json::json!({"count": 42}))
            .send()
            .await;

        resp.assert_ok().assert_json::<serde_json::Value, _>(|v| {
            assert_eq!(v["count"], 42);
        });
    }

    #[tokio::test]
    async fn test_custom_header() {
        let client = TestApp::new().routes(test_routes()).build();

        let resp = client
            .get("/hello")
            .header("x-custom", "test-value")
            .send()
            .await;
        resp.assert_ok();
    }

    #[tokio::test]
    async fn test_client_default() {
        let _app = TestApp::default();
    }
}
