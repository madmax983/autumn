//! Framework-wide interception points for cross-cutting concerns.
//!
//! Interceptors allow you to wrap core framework operations (like sending email,
//! executing background jobs, or checking out database connections) with custom logic.
//! They are typically used for observability (logging, metrics, tracing), fault injection,
//! or modifying payloads before they reach their destination.
//!
//! # Architecture
//!
//! Interceptors follow a middleware pattern. Each interceptor receives the input arguments
//! and a `next` continuation future (or closure). The interceptor must call `next` to proceed
//! with the operation, but it can inspect or modify the inputs beforehand, or inspect the
//! result afterwards.

#[cfg(feature = "oauth2")]
use std::sync::Arc;

#[cfg(feature = "mail")]
/// Intercepts outgoing transactional emails.
///
/// Use this trait to log email activity, add default headers, or block emails
/// from being sent during local development.
///
/// # Example
///
/// ```rust,no_run
/// struct LoggingMailInterceptor;
///
/// impl autumn_web::interceptor::MailInterceptor for LoggingMailInterceptor {
///     fn intercept<'a>(
///         &'a self,
///         mail: &'a autumn_web::mail::Mail,
///         next: std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), autumn_web::mail::MailError>> + Send + 'a>>,
///     ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), autumn_web::mail::MailError>> + Send + 'a>> {
///         Box::pin(async move {
///             println!("Sending email to: {:?}", mail.to);
///             next.await
///         })
///     }
/// }
/// ```
pub trait MailInterceptor: Send + Sync + 'static {
    /// Intercept an outgoing email before it is dispatched to the backend.
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

/// Intercepts background job enqueueing and execution.
///
/// Job interceptors are useful for propagating distributed tracing spans
/// across the queue boundary, or for recording job execution metrics.
pub trait JobInterceptor: Send + Sync + 'static {
    /// Intercept a job right before it is pushed into the queue.
    fn intercept_enqueue<'a>(
        &'a self,
        name: &'a str,
        payload: &'a serde_json::Value,
        next: std::pin::Pin<
            Box<dyn std::future::Future<Output = crate::AutumnResult<()>> + Send + 'a>,
        >,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = crate::AutumnResult<()>> + Send + 'a>>;

    /// Intercept a job right before it is executed by a worker.
    fn intercept_execute<'a>(
        &'a self,
        name: &'a str,
        payload: &'a serde_json::Value,
        next: std::pin::Pin<
            Box<dyn std::future::Future<Output = crate::AutumnResult<()>> + Send + 'a>,
        >,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = crate::AutumnResult<()>> + Send + 'a>>;
}

/// Context provided when a database connection is requested from the pool.
#[derive(Debug, Clone)]
pub struct DbCheckoutContext {
    /// The name of the database pool being accessed (e.g. "primary", "replica").
    pub pool_name: String,
}

#[cfg(feature = "db")]
/// Intercepts database connection checkouts from the connection pool.
///
/// This is typically used to inject tenant-specific `SET LOCAL` statements
/// when a connection is checked out for a request in a multi-tenant environment,
/// ensuring the connection is properly initialized for the current context.
pub trait DbConnectionInterceptor: Send + Sync + 'static {
    /// Intercept the acquisition of a connection from the pool.
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
/// Intercepts pub/sub messages sent via the real-time channels system.
///
/// Useful for augmenting messages with global state, enforcing authorization,
/// or dropping messages entirely before they reach connected clients.
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
/// Future type returned by an `HttpInterceptor`.
pub type HttpInterceptorFuture<'a> = std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<reqwest::Response, reqwest::Error>> + Send + 'a>,
>;

#[cfg(feature = "oauth2")]
/// Intercepts outbound HTTP requests made by the framework.
///
/// Often used to inject authentication headers, implement custom retry logic,
/// or record outgoing request latency.
pub trait HttpInterceptor: Send + Sync + 'static {
    /// Intercept an outbound HTTP request before it is sent.
    fn intercept<'a>(
        &'a self,
        req: reqwest::Request,
        next: &'a dyn Fn(reqwest::Request) -> HttpInterceptorFuture<'a>,
    ) -> HttpInterceptorFuture<'a>;
}

#[cfg(feature = "oauth2")]
tokio::task_local! {
    /// Task-local storage for active HTTP interceptors.
    ///
    /// Used internally by the framework to propagate interceptors across
    /// asynchronous boundaries when executing outbound HTTP calls.
    pub static ACTIVE_HTTP_INTERCEPTORS: Vec<Arc<dyn HttpInterceptor>>;
}
