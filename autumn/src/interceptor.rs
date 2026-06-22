//! Pluggable interceptors for cross-cutting observability and mutation.
//!
//! Interceptors provide a hook into core framework pipelines—such as `OAuth2` flows,
//! Mail delivery, or Background Jobs—allowing applications to transparently
//! observe, mutate, or block operations before they occur.
//!
//! For example, a `MailInterceptor` could log outgoing emails, rewrite test domains,
//! or deny delivery based on external rate-limiting. A `JobInterceptor` could
//! inject distributed tracing contexts or increment enqueue metrics.
//!
//! # Note
//! These are distinct from HTTP Middleware (`tower::Layer`), which operates exclusively
//! on the incoming HTTP request path.

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

    /// Returns whether this interceptor enables transactional test isolation mode.
    fn is_transactional_test(&self) -> bool {
        false
    }
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
