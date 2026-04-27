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
//! use autumn_admin_plugin::AdminPlugin;
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
pub use traits::{AdminAction, AdminField, AdminFieldKind, AdminModel};

use std::borrow::Cow;
use std::sync::Arc;

use autumn_web::app::AppBuilder;
use autumn_web::plugin::Plugin;

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

        app.nest(&prefix, router)
    }
}
