use std::task::{Context, Poll};
use std::future::Future;
use std::pin::Pin;

use axum::http::Request;
use axum::response::Response;
use tower::{Layer, Service};

use crate::flash::Flash;
use crate::session::Session;

/// Middleware that automatically injects unconsumed flash messages as `HX-Trigger` headers
/// for HTMX requests.
#[derive(Clone)]
pub struct FlashInjectionLayer;

impl<S> Layer<S> for FlashInjectionLayer {
    type Service = FlashInjectionService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        FlashInjectionService { inner }
    }
}

#[derive(Clone)]
pub struct FlashInjectionService<S> {
    inner: S,
}

impl<S, ReqBody> Service<Request<ReqBody>> for FlashInjectionService<S>
where
    S: Service<Request<ReqBody>, Response = Response> + Send + 'static,
    S::Future: Send + 'static,
    ReqBody: Send + 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send + 'static>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request<ReqBody>) -> Self::Future {
        // Only trigger the injection if it's an HTMX request.
        let is_htmx = req
            .headers()
            .get("hx-request")
            .is_some_and(|v| v == "true");

        let session_opt = req.extensions().get::<Session>().cloned();

        let future = self.inner.call(req);

        Box::pin(async move {
            let mut response = future.await?;

            if is_htmx {
                if let Some(session) = session_opt {
                    let flash = Flash::new(session);
                    let messages = flash.consume().await;

                    if !messages.is_empty() {
                        // Merge with existing HX-Trigger if present, otherwise create a new JSON payload.
                        let mut payload = serde_json::json!({});

                        if let Some(existing_header) = response.headers().get("hx-trigger") {
                            if let Ok(existing_str) = existing_header.to_str() {
                                if let Ok(serde_json::Value::Object(map)) = serde_json::from_str(existing_str) {
                                    for (k, v) in map {
                                        payload[k] = v;
                                    }
                                } else {
                                    // Handle cases where the existing trigger might just be a string.
                                    payload[existing_str] = serde_json::Value::String("".to_string());
                                }
                            }
                        }

                        payload["flash"] = serde_json::json!(messages);

                        if let Ok(v) = http::header::HeaderValue::from_str(&payload.to_string()) {
                            response.headers_mut()
                                .insert(http::header::HeaderName::from_static("hx-trigger"), v);
                        }
                    }
                }
            }

            Ok(response)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use axum::response::IntoResponse;
    use std::collections::HashMap;
    use tower::Service;

    // A simple mock service to test the middleware functionality.
    #[derive(Clone)]
    struct MockService {
        with_existing: bool,
    }

    impl Service<Request<Body>> for MockService {
        type Response = axum::response::Response;
        type Error = std::convert::Infallible;
        type Future = std::future::Ready<Result<Self::Response, Self::Error>>;

        fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }

        fn call(&mut self, _req: Request<Body>) -> Self::Future {
            let mut response = "ok".into_response();
            if self.with_existing {
                response.headers_mut().insert(
                    http::header::HeaderName::from_static("hx-trigger"),
                    http::header::HeaderValue::from_static(r#"{"customEvent": "triggered"}"#)
                );
            }
            std::future::ready(Ok(response))
        }
    }

    #[tokio::test]
    async fn injects_hx_trigger_for_htmx_request() {
        let session = Session::new_for_test("test".to_owned(), HashMap::new());
        let flash = Flash::new(session.clone());
        flash.success("Hello world").await;

        let mut service = FlashInjectionLayer.layer(MockService { with_existing: false });

        let mut req = Request::builder()
            .uri("/")
            .header("hx-request", "true")
            .body(Body::empty())
            .unwrap();
        req.extensions_mut().insert(session.clone());

        let response = service.call(req).await.unwrap();

        let trigger = response.headers().get("hx-trigger").expect("header missing");
        let val = trigger.to_str().unwrap();
        assert!(val.contains("Hello world"));
        assert!(val.contains("success"));

        // Ensure flash was consumed
        assert!(flash.peek().await.is_empty());
    }

    #[tokio::test]
    async fn merges_with_existing_hx_trigger() {
        let session = Session::new_for_test("test".to_owned(), HashMap::new());
        let flash = Flash::new(session.clone());
        flash.success("Hello world").await;

        let mut service = FlashInjectionLayer.layer(MockService { with_existing: true });

        let mut req = Request::builder()
            .uri("/")
            .header("hx-request", "true")
            .body(Body::empty())
            .unwrap();
        req.extensions_mut().insert(session.clone());

        let response = service.call(req).await.unwrap();

        let trigger = response.headers().get("hx-trigger").expect("header missing");
        let val = trigger.to_str().unwrap();

        // Ensure both original and new exist
        assert!(val.contains("customEvent"));
        assert!(val.contains("Hello world"));
    }

    #[tokio::test]
    async fn does_not_inject_for_normal_request() {
        let session = Session::new_for_test("test".to_owned(), HashMap::new());
        let flash = Flash::new(session.clone());
        flash.success("Hello world").await;

        let mut service = FlashInjectionLayer.layer(MockService { with_existing: false });

        // Missing hx-request: true
        let mut req = Request::builder()
            .uri("/")
            .body(Body::empty())
            .unwrap();
        req.extensions_mut().insert(session.clone());

        let response = service.call(req).await.unwrap();

        assert!(response.headers().get("hx-trigger").is_none());

        // Ensure flash was NOT consumed
        assert_eq!(flash.peek().await.len(), 1);
    }
}
