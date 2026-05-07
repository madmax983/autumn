//! System information plugin.

use std::env;
use std::sync::OnceLock;
use std::thread;

use axum::Json;
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
pub struct SystemInfo {
    pub os: String,
    pub arch: String,
    pub available_parallelism: usize,
}

pub(crate) async fn system_info_handler() -> Json<SystemInfo> {
    static INFO: OnceLock<SystemInfo> = OnceLock::new();
    Json(
        INFO.get_or_init(|| SystemInfo {
            os: env::consts::OS.to_owned(),
            arch: env::consts::ARCH.to_owned(),
            available_parallelism: thread::available_parallelism()
                .map(std::num::NonZero::get)
                .unwrap_or(1),
        })
        .clone(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    #[tokio::test]
    async fn test_system_info_handler() {
        let app =
            axum::Router::new().route("/actuator/system", axum::routing::get(system_info_handler));
        let req = Request::builder()
            .uri("/actuator/system")
            .body(Body::empty())
            .unwrap();
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
