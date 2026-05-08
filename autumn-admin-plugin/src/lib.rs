//! # autumn-admin-plugin
//!
//! Out-of-the-box admin panel plugin for autumn-web applications.
//!
//! Provides auto-generated CRUD views, search, filtering, and audit trails
//! for any model registered via the [`AdminPlugin`] builder. The UI is
//! server-rendered with Maud + HTMX — no JS build step required.
//!
//! # Quick start
//!
//! ```rust,ignore
//! use autumn_admin_plugin::{prelude::*, AdminPlugin};
//!
//! autumn_web::app()
//!     .plugin(
//!         AdminPlugin::new()
//!             .register(ProjectAdmin::default())
//!             .register(TicketAdmin::default()),
//!     )
//!     .routes(routes![...])
//!     .run()
//!     .await;
//! ```
//!
//! # Security
//!
//! The plugin requires the `"admin"` role in the session by default. Override
//! with [`AdminPlugin::require_role`] (pass `None` to disable; not recommended
//! for production).
//!
//! # Naming convention
//!
//! First-party plugin: `autumn-<name>-plugin`.

mod auth;
mod registry;
mod routes;
mod templates;
mod traits;

pub use registry::AdminRegistry;
pub use traits::{
    AdminAction, AdminError, AdminField, AdminFieldKind, AdminFuture, AdminModel, ListParams,
    ListResult, SortDirection,
};

/// Common downstream imports for implementing admin models.
pub mod prelude {
    pub use crate::{
        AdminError, AdminField, AdminFieldKind, AdminFuture, AdminModel, ListParams, ListResult,
        SortDirection,
    };
}

use std::borrow::Cow;
use std::sync::Arc;

use autumn_web::app::AppBuilder;
use autumn_web::plugin::Plugin;
use autumn_web::route_listing::RouteInfo;

/// The admin panel plugin.
///
/// Register models via `.register()` and the plugin will mount a full admin
/// UI under the configured prefix (default: `/admin`).
pub struct AdminPlugin {
    registry: AdminRegistry,
    prefix: String,
    actuator_prefix: String,
    auth_session_key: String,
    require_role: Option<String>,
}

impl AdminPlugin {
    /// Create a new admin plugin with default settings.
    ///
    /// Mounts at `/admin` and requires the `"admin"` role in the session.
    /// Links to the actuator UI under `/actuator`. Reads the user
    /// identifier from session key `"user_id"` (Autumn's default
    /// `auth.session_key`).
    #[must_use]
    pub fn new() -> Self {
        Self {
            registry: AdminRegistry::new(),
            prefix: "/admin".to_owned(),
            actuator_prefix: "/actuator".to_owned(),
            auth_session_key: "user_id".to_owned(),
            require_role: Some("admin".to_owned()),
        }
    }

    /// Override the URL prefix (default: `/admin`).
    #[must_use]
    pub fn prefix(mut self, prefix: impl Into<String>) -> Self {
        self.prefix = prefix.into();
        self
    }

    /// Override the actuator mount prefix that dashboard links/polling target
    /// (default: `/actuator`). Must match `config.actuator.prefix` from your
    /// autumn config — the plugin cannot read it automatically because config
    /// is loaded after `Plugin::build` runs.
    #[must_use]
    pub fn actuator_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.actuator_prefix = prefix.into();
        self
    }

    /// Override the session key the role middleware reads to detect an
    /// authenticated user. Default: `"user_id"`, matching Autumn's default
    /// `auth.session_key`. Must match whatever your application populates
    /// after login — e.g. set this to `"uid"` if you configured
    /// `auth.session_key = "uid"`.
    ///
    /// The plugin can't read `config.auth.session_key` automatically
    /// because config is loaded after `Plugin::build` runs.
    #[must_use]
    pub fn auth_session_key(mut self, key: impl Into<String>) -> Self {
        self.auth_session_key = key.into();
        self
    }

    /// Set the required session role for accessing the admin panel.
    ///
    /// Pass `None` to disable role checks entirely. Authentication
    /// (a populated `user_id` session key) is always required when a role
    /// is set.
    #[must_use]
    pub fn require_role(mut self, role: impl Into<Option<String>>) -> Self {
        self.require_role = role.into();
        self
    }

    /// Register a model for admin management.
    ///
    /// The model must implement [`AdminModel`], which provides field metadata,
    /// CRUD operations, and display configuration.
    #[must_use]
    pub fn register<M: AdminModel>(mut self, model: M) -> Self {
        self.registry.register(model);
        self
    }
}

impl Default for AdminPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl Plugin for AdminPlugin {
    fn name(&self) -> Cow<'static, str> {
        Cow::Borrowed("autumn-admin-plugin")
    }

    fn build(self, app: AppBuilder) -> AppBuilder {
        let Self {
            registry,
            prefix,
            actuator_prefix,
            auth_session_key,
            require_role,
        } = self;
        let registry = Arc::new(registry);
        let router = routes::admin_router(
            Arc::clone(&registry),
            &prefix,
            actuator_prefix.clone(),
            auth_session_key.clone(),
            require_role.clone(),
        );

        tracing::info!(
            prefix = %prefix,
            actuator_prefix = %actuator_prefix,
            auth_session_key = %auth_session_key,
            models = registry.model_count(),
            role = require_role.as_deref().unwrap_or("<none>"),
            "🍂 Autumn Admin mounted"
        );

        // Declare routes for `autumn routes` listing. The underlying Axum router
        // is added via nest() which is opaque to route enumeration, so we
        // explicitly register route metadata here.
        let declared = admin_route_infos(&prefix);

        app.nest(&prefix, router).declare_plugin_routes(declared)
    }
}

/// Generate the route metadata list for this plugin's mounted routes.
///
/// Kept in sync with `routes::admin_router` — update here when routes are
/// added or removed from the admin router.
pub(crate) fn admin_route_infos(prefix: &str) -> Vec<RouteInfo> {
    [
        ("GET", format!("{prefix}")),
        ("GET", format!("{prefix}/{{slug}}")),
        ("POST", format!("{prefix}/{{slug}}")),
        ("GET", format!("{prefix}/{{slug}}/new")),
        ("GET", format!("{prefix}/{{slug}}/{{id}}")),
        ("POST", format!("{prefix}/{{slug}}/{{id}}")),
        ("DELETE", format!("{prefix}/{{slug}}/{{id}}")),
        ("GET", format!("{prefix}/{{slug}}/{{id}}/edit")),
        ("POST", format!("{prefix}/{{slug}}/actions")),
        ("GET", format!("{prefix}/static/admin.{{hash}}.js")),
    ]
    .into_iter()
    .map(|(method, path)| RouteInfo {
        method: method.to_owned(),
        path,
        handler: format!("admin::{}", method.to_lowercase()),
        source: autumn_web::route_listing::RouteSource::User, // overwritten by declare_plugin_routes
        middleware: vec![],
    })
    .collect()
}

// ── Conformance reference tests ────────────────────────────────────────────
//
// These tests are the reference example for the Autumn plugin conformance
// workflow documented in docs/plugins.md.  They use
// `autumn_web::plugin_conformance` to verify that the admin plugin's declared
// routes satisfy all conformance checks before publication.
//
// See docs/plugins.md § "Plugin conformance and publishing checklist" for the
// equivalent `autumn plugin-check` CLI invocation.

#[cfg(test)]
mod conformance_tests {
    use autumn_web::plugin_conformance::{run_conformance, ConformanceConfig};
    use autumn_web::route_listing::{RouteInfo, RouteSource};

    const PLUGIN_NAME: &str = "autumn-admin-plugin";

    /// Build the routes that `AdminPlugin` contributes under `prefix`,
    /// attributed to the plugin. Reuses `admin_route_infos` from the outer
    /// module and overrides the source to `Plugin(PLUGIN_NAME)`.
    fn admin_routes(prefix: &str) -> Vec<RouteInfo> {
        super::admin_route_infos(prefix)
            .into_iter()
            .map(|mut r| {
                r.source = RouteSource::Plugin(PLUGIN_NAME.to_owned());
                r
            })
            .collect()
    }

    #[test]
    fn admin_plugin_routes_are_attributed_to_plugin_name() {
        let routes = admin_routes("/admin");
        let result = autumn_web::plugin_conformance::check_route_attribution(PLUGIN_NAME, &routes);
        assert_eq!(
            result.status,
            autumn_web::plugin_conformance::CheckStatus::Pass,
            "route attribution failed: {}",
            result.message
        );
    }

    #[test]
    fn admin_plugin_routes_live_under_admin_prefix() {
        let routes = admin_routes("/admin");
        let result = autumn_web::plugin_conformance::check_route_prefix(
            PLUGIN_NAME,
            "/admin",
            &[],
            &routes,
        );
        assert_eq!(
            result.status,
            autumn_web::plugin_conformance::CheckStatus::Pass,
            "prefix check failed: {}\n{:?}",
            result.message,
            result.diagnostics
        );
    }

    #[test]
    fn admin_plugin_has_no_route_collisions_in_isolation() {
        let routes = admin_routes("/admin");
        let (result, _) = autumn_web::plugin_conformance::check_collisions(&routes);
        assert_eq!(
            result.status,
            autumn_web::plugin_conformance::CheckStatus::Pass,
            "unexpected collision: {}\n{:?}",
            result.message,
            result.diagnostics
        );
    }

    #[test]
    fn admin_plugin_sensitive_surfaces_declared_with_role_requirement() {
        let routes = admin_routes("/admin");
        let declared = vec![autumn_web::plugin_conformance::SensitiveRoute {
            path_pattern: "/admin".to_owned(),
            auth_mechanism: "Role: admin required via AdminPlugin::require_role \
                             (default) or AdminPlugin::require_role(None) to disable"
                .to_owned(),
        }];
        let result = autumn_web::plugin_conformance::check_sensitive_surfaces(
            PLUGIN_NAME,
            &routes,
            &declared,
        );
        assert_eq!(
            result.status,
            autumn_web::plugin_conformance::CheckStatus::Pass,
            "sensitive-surfaces check failed: {}",
            result.message
        );
    }

    #[test]
    fn admin_plugin_sensitive_surfaces_fail_without_declaration() {
        let routes = admin_routes("/admin");
        let result =
            autumn_web::plugin_conformance::check_sensitive_surfaces(PLUGIN_NAME, &routes, &[]);
        assert_eq!(
            result.status,
            autumn_web::plugin_conformance::CheckStatus::Fail,
            "expected FAIL when sensitive routes are undeclared"
        );
    }

    #[test]
    fn admin_plugin_passes_full_conformance_with_config() {
        let routes = admin_routes("/admin");
        let config = ConformanceConfig::new(PLUGIN_NAME)
            .prefix("/admin")
            .sensitive_route(
                "/admin",
                "Role: admin required via AdminPlugin::require_role",
            );
        let report = run_conformance(&config, &routes);
        assert!(
            report.passed(),
            "AdminPlugin conformance failed:\n{}",
            report.to_text_report()
        );
    }

    #[test]
    fn admin_plugin_collision_with_host_route_detected() {
        let mut routes = admin_routes("/admin");
        // Simulate a host app that accidentally defines GET /admin
        routes.push(RouteInfo {
            method: "GET".to_owned(),
            path: "/admin".to_owned(),
            handler: "host::admin_redirect".to_owned(),
            source: RouteSource::User,
            middleware: vec![],
        });
        let (result, diagnostics) = autumn_web::plugin_conformance::check_collisions(&routes);
        assert_eq!(
            result.status,
            autumn_web::plugin_conformance::CheckStatus::Fail,
            "expected collision to be detected"
        );
        let diag = &diagnostics[0];
        assert_eq!(diag.method, "GET");
        assert_eq!(diag.path, "/admin");
        let sources: Vec<&str> = diag.contributors.iter().map(|c| c.source.as_str()).collect();
        assert!(
            sources.contains(&"user"),
            "missing user contributor: {sources:?}"
        );
        assert!(
            sources.contains(&"plugin:autumn-admin-plugin"),
            "missing plugin contributor: {sources:?}"
        );
    }

    #[test]
    fn admin_plugin_custom_prefix_passes_conformance() {
        let routes = admin_routes("/backend");
        let config = ConformanceConfig::new(PLUGIN_NAME)
            .prefix("/backend")
            .sensitive_route(
                "/backend",
                "Role: admin required via AdminPlugin::require_role",
            );
        let report = run_conformance(&config, &routes);
        assert!(
            report.passed(),
            "AdminPlugin with custom prefix failed conformance:\n{}",
            report.to_text_report()
        );
    }

    #[test]
    fn admin_plugin_double_registration_detected() {
        // Simulate registering the admin plugin twice — its routes appear twice.
        let mut routes = admin_routes("/admin");
        routes.extend(admin_routes("/admin"));
        let result =
            autumn_web::plugin_conformance::check_duplicate_registration(PLUGIN_NAME, &routes);
        assert_eq!(
            result.status,
            autumn_web::plugin_conformance::CheckStatus::Fail,
            "expected duplicate-registration FAIL when plugin installed twice"
        );
    }

    #[test]
    fn admin_plugin_single_registration_passes_duplicate_check() {
        let routes = admin_routes("/admin");
        let result =
            autumn_web::plugin_conformance::check_duplicate_registration(PLUGIN_NAME, &routes);
        assert_eq!(
            result.status,
            autumn_web::plugin_conformance::CheckStatus::Pass,
            "single registration should pass: {}",
            result.message
        );
    }
}
