//! # Interceptors
//!
//! Interceptors provide a way to hook into and modify the behavior of internal Autumn
//! systems. They act as middleware for out-of-band operations that aren't strictly
//! tied to a single HTTP request, such as sending emails, enqueuing background jobs,
//! checking out database connections, or making outbound HTTP requests.
//!
//! Interceptors are generally registered during application setup and wrap the underlying
//! operations. This is useful for adding telemetrics, observability, context propagation,
//! or modifying payloads dynamically.

#[cfg(feature = "oauth2")]
use std::sync::Arc;

#[cfg(feature = "mail")]
/// Intercepts outgoing email messages before they are delivered.
///
/// This trait allows you to inject logic immediately before an email is sent to the configured
/// mail transport. You can use this to add logging, track email volume metrics, or even cancel
/// the email delivery by refusing to call `next`.
///
/// # Examples
///
/// ```rust
/// # use autumn_web::interceptor::MailInterceptor;
/// # use autumn_web::mail::{Mail, MailError};
/// # use std::future::Future;
/// # use std::pin::Pin;
/// #
/// pub struct LoggingMailInterceptor;
///
/// impl MailInterceptor for LoggingMailInterceptor {
///     fn intercept<'a>(
///         &'a self,
///         mail: &'a Mail,
///         next: Pin<Box<dyn Future<Output = Result<(), MailError>> + Send + 'a>>,
///     ) -> Pin<Box<dyn Future<Output = Result<(), MailError>> + Send + 'a>> {
///         Box::pin(async move {
///             println!("Sending email to: {}", mail.to);
///             let result = next.await;
///             if result.is_ok() {
///                 println!("Email successfully sent!");
///             }
///             result
///         })
///     }
/// }
/// ```
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

/// Intercepts background job execution and enqueueing.
///
/// This trait allows you to inject logic before and after background jobs are enqueued or executed.
/// You can use this to add observability, trace context, or dynamically modify payloads.
///
/// # Examples
///
/// ```rust
/// # use autumn_web::interceptor::JobInterceptor;
/// # use autumn_web::AutumnResult;
/// # use serde_json::Value;
/// # use std::future::Future;
/// # use std::pin::Pin;
/// #
/// pub struct LoggingJobInterceptor;
///
/// impl JobInterceptor for LoggingJobInterceptor {
///     fn intercept_enqueue<'a>(
///         &'a self,
///         name: &'a str,
///         payload: &'a Value,
///         next: Pin<Box<dyn Future<Output = AutumnResult<()>> + Send + 'a>>,
///     ) -> Pin<Box<dyn Future<Output = AutumnResult<()>> + Send + 'a>> {
///         Box::pin(async move {
///             println!("Enqueueing job: {} with payload {:?}", name, payload);
///             next.await
///         })
///     }
///
///     fn intercept_execute<'a>(
///         &'a self,
///         name: &'a str,
///         payload: &'a Value,
///         next: Pin<Box<dyn Future<Output = AutumnResult<()>> + Send + 'a>>,
///     ) -> Pin<Box<dyn Future<Output = AutumnResult<()>> + Send + 'a>> {
///         Box::pin(async move {
///             println!("Executing job: {}", name);
///             let result = next.await;
///             if result.is_ok() {
///                 println!("Job executed successfully!");
///             }
///             result
///         })
///     }
/// }
/// ```
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

/// Contextual information provided when checking out a database connection.
#[derive(Debug, Clone)]
pub struct DbCheckoutContext {
    /// The name of the connection pool being checked out from.
    pub pool_name: String,
}

#[cfg(feature = "db")]
/// Intercepts database connection checkouts from the connection pool.
///
/// This trait allows you to hook into the process of acquiring a database connection.
/// You can use this to execute setup queries (like configuring row-level security parameters)
/// or setting tenant contexts immediately after a connection is leased from the pool.
///
/// # Examples
///
/// ```rust
/// # use autumn_web::interceptor::{DbCheckoutContext, DbConnectionInterceptor};
/// # use autumn_web::db::PooledConnection;
/// # use autumn_web::AutumnError;
/// # use std::future::Future;
/// # use std::pin::Pin;
/// #
/// pub struct SetupConnectionInterceptor;
///
/// impl DbConnectionInterceptor for SetupConnectionInterceptor {
///     fn intercept_checkout<'a>(
///         &'a self,
///         ctx: DbCheckoutContext,
///         next: Pin<Box<dyn Future<Output = Result<PooledConnection, AutumnError>> + Send + 'a>>,
///     ) -> Pin<Box<dyn Future<Output = Result<PooledConnection, AutumnError>> + Send + 'a>> {
///         Box::pin(async move {
///             println!("Checking out connection from pool: {}", ctx.pool_name);
///             let conn = next.await?;
///             // Execute custom setup on `conn` here...
///             Ok(conn)
///         })
///     }
/// }
/// ```
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
/// Intercepts WebSocket channel message publication.
///
/// This trait allows you to inject logic before a message is published to a WebSocket
/// channel. You can use this to filter messages, log them, or validate payloads
/// before they reach subscribers.
///
/// # Examples
///
/// ```rust
/// # use autumn_web::interceptor::ChannelsInterceptor;
/// # use autumn_web::channels::{ChannelMessage, ChannelPublishError};
/// #
/// pub struct LoggingChannelsInterceptor;
///
/// impl ChannelsInterceptor for LoggingChannelsInterceptor {
///     fn intercept_publish(
///         &self,
///         topic: &str,
///         msg: &ChannelMessage,
///         next: &dyn Fn(&str, &ChannelMessage) -> Result<usize, ChannelPublishError>,
///     ) -> Result<usize, ChannelPublishError> {
///         println!("Publishing message to topic {}: {:?}", topic, msg);
///         let result = next(topic, msg);
///         if result.is_ok() {
///             println!("Message successfully published!");
///         }
///         result
///     }
/// }
/// ```
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
/// The future type returned by HTTP interceptors.
pub type HttpInterceptorFuture<'a> = std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<reqwest::Response, reqwest::Error>> + Send + 'a>,
>;

#[cfg(feature = "oauth2")]
/// Intercepts outbound HTTP requests.
///
/// This trait allows you to observe, modify, or block HTTP requests made by the application
/// using `reqwest`. This is especially useful for adding authentication headers automatically,
/// logging outbound request metrics, or mocking external APIs during testing.
///
/// # Examples
///
/// ```rust
/// # use autumn_web::interceptor::{HttpInterceptor, HttpInterceptorFuture};
/// # use reqwest::{Request, Response};
/// #
/// pub struct LoggingHttpInterceptor;
///
/// impl HttpInterceptor for LoggingHttpInterceptor {
///     fn intercept<'a>(
///         &'a self,
///         req: Request,
///         next: &'a dyn Fn(Request) -> HttpInterceptorFuture<'a>,
///     ) -> HttpInterceptorFuture<'a> {
///         Box::pin(async move {
///             println!("Sending request to: {}", req.url());
///             let response = next(req).await?;
///             println!("Received response status: {}", response.status());
///             Ok(response)
///         })
///     }
/// }
/// ```
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
