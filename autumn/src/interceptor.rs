//! Extensibility hooks for intercepting internal framework behaviors.
//!
//! Interceptors allow you to run custom code before or after core operations
//! in Autumn. They act as "middleware" for internal components like background
//! jobs, outgoing mail, database checkouts, and HTTP clients.
//!
//! # Available Interceptors
//!
//! - [`JobInterceptor`]: Wraps the `enqueue` and `execute` phases of background jobs.
//! - `MailInterceptor` (requires `mail` feature): Wraps the sending of transactional emails.
//! - `DbConnectionInterceptor` (requires `db` feature): Wraps the checkout of database connections from the pool.
//! - `HttpInterceptor` (requires `oauth2` feature): Wraps outgoing HTTP requests (e.g., during `OAuth2` flows).
//! - `ChannelsInterceptor` (requires `ws` feature): Intercepts channel message publications.
//!
//! # Examples
//!
//! To implement an interceptor, define a struct that implements the desired trait,
//! and register it during application setup.
//!
//! ```rust,ignore
//! use autumn_web::prelude::*;
//! use autumn_web::interceptor::JobInterceptor;
//! use std::pin::Pin;
//!
//! #[derive(Clone)]
//! struct LoggingJobInterceptor;
//!
//! impl JobInterceptor for LoggingJobInterceptor {
//!     fn intercept_enqueue<'a>(
//!         &'a self,
//!         name: &'a str,
//!         payload: &'a serde_json::Value,
//!         next: Pin<Box<dyn std::future::Future<Output = AutumnResult<()>> + Send + 'a>>,
//!     ) -> Pin<Box<dyn std::future::Future<Output = AutumnResult<()>> + Send + 'a>> {
//!         Box::pin(async move {
//!             tracing::info!("Enqueuing job: {}", name);
//!             next.await
//!         })
//!     }
//!
//!     fn intercept_execute<'a>(
//!         &'a self,
//!         name: &'a str,
//!         payload: &'a serde_json::Value,
//!         next: Pin<Box<dyn std::future::Future<Output = AutumnResult<()>> + Send + 'a>>,
//!     ) -> Pin<Box<dyn std::future::Future<Output = AutumnResult<()>> + Send + 'a>> {
//!         Box::pin(async move {
//!             tracing::info!("Executing job: {}", name);
//!             let result = next.await;
//!             tracing::info!("Job {} finished", name);
//!             result
//!         })
//!     }
//! }
//!
//! #[autumn_web::main]
//! async fn main() {
//!     autumn_web::app()
//!         .intercept_job(LoggingJobInterceptor)
//!         .run()
//!         .await;
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
