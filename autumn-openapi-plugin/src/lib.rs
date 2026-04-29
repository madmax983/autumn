//! # autumn-openapi-plugin
//!
//! Exposes a Swagger UI and API explorer out of the box for autumn-web applications.
//!
//! By simply registering this plugin, you get a fully interactive UI that consumes
//! the generated `OpenAPI` spec provided by Autumn's `#[api_doc]` macros.

use autumn_web::app::AppBuilder;
use autumn_web::openapi::OpenApiConfig;
use autumn_web::plugin::Plugin;
use std::borrow::Cow;

pub struct OpenApiPlugin {
    title: String,
    version: String,
    path: String,
}

impl OpenApiPlugin {
    pub fn new(title: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            version: version.into(),
            path: "/docs".to_owned(),
        }
    }

    #[must_use]
    pub fn path(mut self, path: impl Into<String>) -> Self {
        self.path = path.into();
        self
    }
}

impl Plugin for OpenApiPlugin {
    fn name(&self) -> Cow<'static, str> {
        Cow::Borrowed("autumn-openapi-plugin")
    }

    fn build(self, app: AppBuilder) -> AppBuilder {
        let config = OpenApiConfig::new(self.title, self.version).swagger_ui_path(Some(self.path));

        app.openapi(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_openapi_plugin() {
        let plugin = OpenApiPlugin::new("My API", "v1.0").path("/swagger");
        assert_eq!(plugin.title, "My API");
        assert_eq!(plugin.version, "v1.0");
        assert_eq!(plugin.path, "/swagger");
    }
}
