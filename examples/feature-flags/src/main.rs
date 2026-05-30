//! Feature-flags example.
//!
//! Demonstrates handler gating, fragment gating, and percent rollout.
//!
//! Run:
//!   cargo run -p feature-flags
//!
//! Then try:
//!   curl http://localhost:3000/dashboard         # feature off
//!   curl http://localhost:3000/new-feature       # 404 — flag disabled
//!   curl http://localhost:3000/api/flags         # JSON flag list
//!
//! The example pre-enables `dark_mode` for everyone, `beta_inbox` for user:42
//! only, and sets `new_dashboard` to a 50% rollout.

use autumn_web::feature_flags::{FeatureFlagService, FlagStore as _, Flags, InMemoryFlagStore};
use autumn_web::prelude::*;

// ── Handlers ──────────────────────────────────────────────────────────────────

/// Fragment gating: render different sections based on individual flags.
#[get("/dashboard")]
async fn dashboard(flags: Flags) -> String {
    let mut parts = vec!["=== Dashboard ===".to_owned()];

    // Global gate: dark_mode is enabled for everyone.
    if flags.enabled("dark_mode") {
        parts.push("  [dark mode ON]".to_owned());
    }

    // Actor allowlist: beta_inbox is only enabled for user:42.
    if flags.enabled("beta_inbox") {
        parts.push("  [beta inbox visible]".to_owned());
    } else {
        parts.push("  [classic inbox]".to_owned());
    }

    parts.join("\n")
}

/// Handler gating via the Flags extractor.
#[get("/new-feature")]
async fn new_feature(flags: Flags) -> Result<&'static str, AutumnError> {
    if !flags.enabled("new_dashboard") {
        return Err(AutumnError::not_found_msg(
            "new_dashboard flag is disabled for you",
        ));
    }
    Ok("Welcome to the new feature!")
}

/// Expose the full flag list as JSON for inspection.
#[get("/api/flags")]
async fn list_flags(
    axum::extract::State(state): axum::extract::State<AppState>,
) -> Result<Json<serde_json::Value>, AutumnError> {
    let svc = state
        .extension::<FeatureFlagService>()
        .ok_or_else(|| AutumnError::internal_server_error_msg("flag service not installed"))?;

    let flags = svc
        .list()
        .map_err(|e| AutumnError::internal_server_error_msg(e.to_string()))?;

    Ok(Json(serde_json::json!({
        "flags": flags.iter().map(|f| serde_json::json!({
            "key": f.key,
            "enabled": f.enabled,
            "rollout_pct": f.rollout_pct,
            "actor_allowlist": f.actor_allowlist,
        })).collect::<Vec<_>>(),
    })))
}

// ── App setup ─────────────────────────────────────────────────────────────────

#[autumn_web::main]
async fn main() {
    let store = InMemoryFlagStore::new();

    // Global gate: everyone sees dark mode.
    store.enable("dark_mode", Some("setup")).unwrap();

    // Actor allowlist: only user:42 sees the beta inbox.
    store.allow_actor("beta_inbox", "user:42", Some("setup")).unwrap();

    // Percent rollout: 50% of actors see the new dashboard.
    store.set_rollout("new_dashboard", 50, Some("setup")).unwrap();

    autumn_web::app()
        .with_flag_store(store)
        .routes(routes![dashboard, new_feature, list_flags])
        .run()
        .await;
}
