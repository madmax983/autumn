use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};

use autumn_web::auth::{
    API_TOKEN_MIGRATIONS, ApiTokenStore, DbApiTokenStore, RequireApiToken, issue_api_token,
    revoke_api_token,
};
use autumn_web::prelude::*;
use autumn_web::reexports::axum::{Router, routing};

// ── Deferred store ────────────────────────────────────────────────────────────
//
// `RequireApiToken` takes the store at construction time, but the DB pool is
// only available after the app has started. `DeferredStore` bridges the gap:
// it is created early, wired into the middleware, then populated with a
// `DbApiTokenStore` in the `on_startup` hook.

#[derive(Clone)]
struct DeferredStore(Arc<OnceLock<Arc<dyn ApiTokenStore>>>);

impl DeferredStore {
    fn new() -> Self {
        Self(Arc::new(OnceLock::new()))
    }

    fn init(&self, store: Arc<dyn ApiTokenStore>) {
        let _ = self.0.set(store);
    }

    fn inner(&self) -> Arc<dyn ApiTokenStore> {
        Arc::clone(
            self.0
                .get()
                .expect("DeferredStore used before on_startup ran"),
        )
    }
}

impl ApiTokenStore for DeferredStore {
    fn issue<'a>(
        &'a self,
        principal_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = AutumnResult<String>> + Send + 'a>> {
        let store = self.inner();
        Box::pin(async move { store.issue(principal_id).await })
    }

    fn verify<'a>(
        &'a self,
        raw_token: &'a str,
    ) -> Pin<Box<dyn Future<Output = AutumnResult<Option<String>>> + Send + 'a>> {
        let store = self.inner();
        Box::pin(async move { store.verify(raw_token).await })
    }

    fn revoke<'a>(
        &'a self,
        raw_token: &'a str,
    ) -> Pin<Box<dyn Future<Output = AutumnResult<()>> + Send + 'a>> {
        let store = self.inner();
        Box::pin(async move { store.revoke(raw_token).await })
    }
}

// ── Handlers ─────────────────────────────────────────────────────────────────

/// Issue a new API token for `principal_id`.
///
/// Returns the raw bearer token as plain text. It is shown only once — store
/// it securely.
///
/// In production this endpoint must itself be secured (e.g. by an admin role
/// check). It is left open here for demonstration clarity.
#[post("/tokens/:principal_id")]
async fn issue(
    State(state): State<AppState>,
    Path(principal_id): Path<String>,
) -> AutumnResult<String> {
    let pool = state
        .pool()
        .expect("database not configured — set database.url in autumn.toml")
        .clone();
    let store = DbApiTokenStore::new(pool);
    let raw_token = issue_api_token(&store, &principal_id).await?;
    Ok(raw_token)
}

/// Return the authenticated principal ID.
///
/// Requires `Authorization: Bearer <token>` header.
#[get("/me")]
async fn whoami(ApiToken(principal_id): ApiToken) -> String {
    format!("authenticated as {principal_id}\n")
}

/// Revoke the presented bearer token.
///
/// After revocation, the token is rejected with `401 Unauthorized`.
#[delete("/tokens/current")]
async fn revoke_current(
    State(state): State<AppState>,
    ApiToken(raw_token): ApiToken,
) -> AutumnResult<StatusCode> {
    let pool = state
        .pool()
        .expect("database not configured")
        .clone();
    let store = DbApiTokenStore::new(pool);
    revoke_api_token(&store, &raw_token).await?;
    Ok(StatusCode::NO_CONTENT)
}

// ── App entry point ───────────────────────────────────────────────────────────

#[autumn_web::main]
async fn main() {
    let deferred = DeferredStore::new();
    let deferred_for_startup = deferred.clone();

    // Protected sub-router: every route here requires a valid Bearer token.
    let protected = Router::new()
        .route("/me", routing::get(whoami))
        .route("/tokens/current", routing::delete(revoke_current))
        .layer(RequireApiToken::new(Arc::new(deferred.clone())));

    autumn_web::app()
        // Creates the `api_tokens` table on first run.
        .migrations(API_TOKEN_MIGRATIONS)
        // Public: issue a token (admin only in real apps).
        .routes(routes![issue])
        // Protected: require a valid Bearer token on these routes.
        .merge(protected)
        // Initialise the deferred store once the DB pool is ready.
        .on_startup(move |state| {
            let deferred = deferred_for_startup.clone();
            async move {
                let pool = state
                    .pool()
                    .expect("DB pool required for api-tokens example")
                    .clone();
                deferred.init(Arc::new(DbApiTokenStore::new(pool)));
                Ok(())
            }
        })
        .run()
        .await;
}
