//! Interceptors for framework-internal actions.
//!
//! Interceptors provide a way to wrap and observe core framework operations, similar to
//! how middleware wraps HTTP requests. They are incredibly useful for instrumenting
//! performance tracing, altering behavior conditionally, or integrating third-party observability
//! tools into Autumn's underlying subsystems without altering the framework source code.
//!
//! Available interceptors depending on configured features:
//! - [`JobInterceptor`]: Wraps background job enqueue and execution.
//! - `DbConnectionInterceptor`: Wraps database pool checkouts.
//! - `MailInterceptor`: Wraps email delivery attempts.
//! - `ChannelsInterceptor`: Wraps WebSocket topic publications.
//! - `HttpInterceptor`: Wraps outbound OAuth2/OIDC HTTP requests.
//!
//! # Examples
//!
//! To implement an interceptor, implement its specific trait and return a Boxed Future:
//!
//! ```rust,ignore
//! use autumn_web::interceptor::JobInterceptor;
//! use autumn_web::AutumnResult;
//! use std::pin::Pin;
//! use std::future::Future;
//!
//! pub struct MyMetricsInterceptor;
//!
//! impl JobInterceptor for MyMetricsInterceptor {
//!     fn intercept_enqueue<'a>(
//!         &'a self,
//!         name: &'a str,
//!         payload: &'a serde_json::Value,
//!         next: Pin<Box<dyn Future<Output = AutumnResult<()>> + Send + 'a>>,
//!     ) -> Pin<Box<dyn Future<Output = AutumnResult<()>> + Send + 'a>> {
//!         Box::pin(async move {
//!             println!("Enqueuing job: {}", name);
//!             let result = next.await;
//!             println!("Job enqueued: {}", name);
//!             result
//!         })
//!     }
//!
//!     fn intercept_execute<'a>(
//!         &'a self,
//!         name: &'a str,
//!         payload: &'a serde_json::Value,
//!         next: Pin<Box<dyn Future<Output = AutumnResult<()>> + Send + 'a>>,
//!     ) -> Pin<Box<dyn Future<Output = AutumnResult<()>> + Send + 'a>> {
//!         Box::pin(async move {
//!             next.await
//!         })
//!     }
//! }
//! ```
#[cfg(feature = "oauth2")]
use std::sync::Arc;

#[cfg(feature = "mail")]
pub trait MailInterceptor: Send + Sync + 'static {
    fn intercept<'a>(
        &'a self,
        mail: &'a crate::mail::Mail,
        next: std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<(), crate::mail::MailError>> + Send + 'a>,
        >,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<(), crate::mail::MailError>> + Send + 'a>,
    >;
}

/// Middleware trait for intercepting background job enqueuing and execution.
///
/// This allows observing job workflows, emitting metrics, or short-circuiting job
/// dispatch without altering application business logic.
///
/// # Examples
///
/// Check the module level docs for an implementation example.
pub trait JobInterceptor: Send + Sync + 'static {
    fn intercept_enqueue<'a>(
        &'a self,
        name: &'a str,
        payload: &'a serde_json::Value,
        next: std::pin::Pin<
            Box<dyn std::future::Future<Output = crate::AutumnResult<()>> + Send + 'a>,
        >,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = crate::AutumnResult<()>> + Send + 'a>>;

    fn intercept_execute<'a>(
        &'a self,
        name: &'a str,
        payload: &'a serde_json::Value,
        next: std::pin::Pin<
            Box<dyn std::future::Future<Output = crate::AutumnResult<()>> + Send + 'a>,
        >,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = crate::AutumnResult<()>> + Send + 'a>>;
}

#[derive(Debug, Clone)]
pub struct DbCheckoutContext {
    pub pool_name: String,
}

#[cfg(feature = "db")]
pub trait DbConnectionInterceptor: Send + Sync + 'static {
    fn intercept_checkout<'a>(
        &'a self,
        ctx: DbCheckoutContext,
        next: std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<crate::db::PooledConnection, crate::AutumnError>,
                    > + Send
                    + 'a,
            >,
        >,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<crate::db::PooledConnection, crate::AutumnError>,
                > + Send
                + 'a,
        >,
    >;
}

#[cfg(feature = "ws")]
pub trait ChannelsInterceptor: Send + Sync + 'static {
    /// Intercepts a channel message publication.
    ///
    /// # Errors
    ///
    /// Returns a [`ChannelPublishError`](crate::channels::ChannelPublishError) if publication fails.
    fn intercept_publish(
        &self,
        topic: &str,
        msg: &crate::channels::ChannelMessage,
        next: &dyn Fn(
            &str,
            &crate::channels::ChannelMessage,
        ) -> Result<usize, crate::channels::ChannelPublishError>,
    ) -> Result<usize, crate::channels::ChannelPublishError>;
}
#[cfg(feature = "oauth2")]
pub type HttpInterceptorFuture<'a> = std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<reqwest::Response, reqwest::Error>> + Send + 'a>,
>;

#[cfg(feature = "oauth2")]
pub trait HttpInterceptor: Send + Sync + 'static {
    fn intercept<'a>(
        &'a self,
        req: reqwest::Request,
        next: &'a dyn Fn(reqwest::Request) -> HttpInterceptorFuture<'a>,
    ) -> HttpInterceptorFuture<'a>;
}

#[cfg(feature = "oauth2")]
tokio::task_local! {
    pub static ACTIVE_HTTP_INTERCEPTORS: Vec<Arc<dyn HttpInterceptor>>;
}
