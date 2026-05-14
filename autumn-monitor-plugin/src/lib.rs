use autumn_web::app::AppBuilder;
use autumn_web::plugin::Plugin;
use autumn_web::reexports::axum::{Router, response::IntoResponse, routing};
use maud::{DOCTYPE, html};
use std::borrow::Cow;

/// A plugin that adds an interactive HTML dashboard at `/actuator/monitor`
/// to view metrics from the existing `/actuator/metrics` JSON endpoint via HTMX.
pub struct MonitorPlugin {
    prefix: String,
}

impl MonitorPlugin {
    /// Create a new monitor plugin.
    #[must_use]
    pub fn new() -> Self {
        Self {
            prefix: "/actuator".to_owned(),
        }
    }

    /// Set a custom prefix instead of `/actuator`.
    #[must_use]
    pub fn prefix(mut self, prefix: impl Into<String>) -> Self {
        self.prefix = prefix.into();
        self
    }
}

impl Default for MonitorPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl Plugin for MonitorPlugin {
    fn name(&self) -> Cow<'static, str> {
        Cow::Borrowed("autumn_monitor_plugin::MonitorPlugin")
    }

    fn build(self, app: AppBuilder) -> AppBuilder {
        let monitor_route = format!("{}/monitor", self.prefix.trim_end_matches('/'));

        let router = Router::new().route("/", routing::get(monitor_dashboard));

        app.nest(&monitor_route, router)
    }
}

async fn monitor_dashboard() -> impl IntoResponse {
    let markup = html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { "Autumn Monitor" }
                // Fallback basic styling
                style {
                    "body { font-family: system-ui, sans-serif; background: #f9fafb; color: #111827; margin: 0; padding: 2rem; }"
                    "h1 { font-size: 1.5rem; font-weight: 600; margin-bottom: 1.5rem; }"
                    ".grid { display: grid; grid-template-columns: repeat(auto-fit, minmax(300px, 1fr)); gap: 1.5rem; }"
                    ".card { background: white; padding: 1.5rem; border-radius: 0.5rem; box-shadow: 0 1px 3px rgba(0,0,0,0.1); }"
                    ".card h2 { font-size: 1.125rem; font-weight: 500; margin-top: 0; margin-bottom: 1rem; border-bottom: 1px solid #e5e7eb; padding-bottom: 0.5rem; }"
                    "pre { white-space: pre-wrap; font-size: 0.875rem; background: #f3f4f6; padding: 1rem; border-radius: 0.25rem; }"
                }
                script src="/static/js/htmx.min.js" {}
            }
            body {
                h1 { "🍂 Autumn Monitor" }
                div class="grid" {
                    div class="card" {
                        h2 { "Live Metrics" }
                        // Fetch the JSON from /actuator/metrics and just dump it for now.
                        // Real implementation would parse it and show pretty charts.
                        div hx-get="/actuator/metrics" hx-trigger="load, every 2s" {
                            "Loading metrics..."
                        }
                    }
                    div class="card" {
                        h2 { "Tasks" }
                        div hx-get="/actuator/tasks" hx-trigger="load, every 5s" {
                            "Loading tasks..."
                        }
                    }
                }
            }
        }
    };

    (
        [(
            autumn_web::reexports::http::header::CONTENT_TYPE,
            "text/html; charset=utf-8",
        )],
        markup.into_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use autumn_web::reexports::axum::{Router, routing};
    use autumn_web::test::TestApp;

    // Use a direct router approach because plugins directly integrate into the `AppBuilder` lifecycle
    // during boot rather than simple route mounts that TestApp handles via `routes()`.
    // In actual framework usages, one might boot a real app.

    #[tokio::test]
    async fn ui_renders_html() {
        let router = Router::new().route("/monitor", routing::get(monitor_dashboard));
        let test_app = TestApp::new().merge(router).build();

        let response = test_app.get("/monitor").send().await;
        response.assert_status(200);

        let html = response.text();
        assert!(html.contains("Autumn Monitor"));
        assert!(html.contains("hx-get=\"/actuator/metrics\""));
    }
}
