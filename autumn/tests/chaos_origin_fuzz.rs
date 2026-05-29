use autumn_web::middleware::MethodOverrideLayer;
use axum::{body::Body, http::Request, routing::post, Router};
use tower::{ServiceExt, Layer};
use proptest::prelude::*;

proptest! {
    #[test]
    fn does_not_crash_on_any_header_combination(
        ref origin in "\\PC*",
        ref xfh in "\\PC*",
        ref xfp in "\\PC*",
        ref host in "\\PC*"
    ) {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let _guard = rt.enter();

        let router = Router::new()
            .route("/items/1", post(|| async { "post-ok" }))
            .route("/items/1", axum::routing::delete(|| async { "deleted" }));

        let app = MethodOverrideLayer::new().layer(router);

        let mut builder = Request::builder()
            .method("POST")
            .uri("/items/1")
            .header("content-type", "application/x-www-form-urlencoded")
            .header("sec-fetch-site", "same-origin");

        if let Ok(val) = axum::http::HeaderValue::from_str(origin) {
            builder = builder.header("origin", val);
        }
        if let Ok(val) = axum::http::HeaderValue::from_str(xfh) {
            builder = builder.header("x-forwarded-host", val.clone());
            builder = builder.header("x-forwarded-host", val);
        }
        if let Ok(val) = axum::http::HeaderValue::from_str(xfp) {
            builder = builder.header("x-forwarded-proto", val);
        }
        if let Ok(val) = axum::http::HeaderValue::from_str(host) {
            builder = builder.header("host", val);
        }

        let request = builder.body(Body::from("_method=DELETE")).unwrap();

        let _ = rt.block_on(app.oneshot(request));
    }
}
