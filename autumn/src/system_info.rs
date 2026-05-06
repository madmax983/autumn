//! System information plugin.

use std::borrow::Cow;
use std::env;
use std::thread;

use axum::Json;
use axum::routing::get;
use serde::{Deserialize, Serialize};

use crate::app::AppBuilder;
use crate::plugin::Plugin;

#[derive(Serialize, Deserialize)]
pub struct SystemInfo {
    pub os: String,
    pub arch: String,
    pub available_parallelism: usize,
}

pub struct SystemInfoPlugin {
    path: String,
}

impl SystemInfoPlugin {
    #[must_use]
    pub fn new() -> Self {
        Self {
            path: "/actuator/system".to_owned(),
        }
    }

    #[must_use]
    pub fn path(mut self, path: impl Into<String>) -> Self {
        self.path = path.into();
        self
    }
}

impl Default for SystemInfoPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl Plugin for SystemInfoPlugin {
    fn name(&self) -> Cow<'static, str> {
        Cow::Borrowed("autumn-system-info-plugin")
    }

    fn build(self, app: AppBuilder) -> AppBuilder {
        let router = axum::Router::new().route("/", get(system_info_handler));
        app.nest(&self.path, router)
    }
}

async fn system_info_handler() -> Json<SystemInfo> {
    Json(SystemInfo {
        os: env::consts::OS.to_owned(),
        arch: env::consts::ARCH.to_owned(),
        available_parallelism: thread::available_parallelism()
            .map(std::num::NonZero::get)
            .unwrap_or(1),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    #[tokio::test]
    async fn test_system_info_handler() {
        let app = axum::Router::new().route("/", axum::routing::get(system_info_handler));
        let req = Request::builder().uri("/").body(Body::empty()).unwrap();
        let res = app.oneshot(req).await.unwrap();
        assert_eq!(res.status(), 200);

        // Ensure it returns valid json
        let body_bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        let info: SystemInfo = serde_json::from_slice(&body_bytes).unwrap();
        assert!(!info.os.is_empty());
    }
}
