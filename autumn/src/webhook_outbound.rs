#![allow(
    clippy::significant_drop_tightening,
    clippy::missing_panics_doc,
    clippy::missing_errors_doc
)]
//! Outbound signed webhook delivery with retries, DLQ, and subscription management.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, RwLock};

use crate::http_client::Client;
use crate::{AppState, AutumnError, AutumnResult};

/// The status of a webhook subscription.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WebhookSubscriptionStatus {
    Active,
    Disabled,
    Failed,
}

impl WebhookSubscriptionStatus {
    /// Return the lower-case status label.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Disabled => "disabled",
            Self::Failed => "failed",
        }
    }
}

impl std::fmt::Display for WebhookSubscriptionStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A registered webhook subscription targeting a consumer endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookSubscription {
    pub id: String,
    pub target_url: String,
    pub event_topics: Vec<String>,
    pub secret: String,
    pub status: WebhookSubscriptionStatus,
    pub consecutive_failures: u32,
}

/// A structured log of an outbound webhook delivery attempt.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookDeliveryLog {
    pub id: String,
    pub subscription_id: String,
    pub topic: String,
    pub payload: String,
    pub request_headers: HashMap<String, String>,
    pub response_status: Option<u16>,
    pub response_body: Option<String>,
    pub elapsed_ms: u64,
    pub attempt: u32,
    pub max_attempts: u32,
    pub is_dlq: bool,
    pub last_error: Option<String>,
    pub timestamp: DateTime<Utc>,
}

/// Pluggable handler interface for outbound webhook subscriptions and delivery logs.
pub trait OutboundWebhookHandler: Send + Sync + 'static {
    /// Retrieve active subscriptions registered for a specific event topic.
    fn get_subscriptions(
        &self,
        topic: &str,
    ) -> Pin<Box<dyn Future<Output = AutumnResult<Vec<WebhookSubscription>>> + Send>>;

    /// Log a webhook delivery attempt and handle failure counters/statuses.
    fn log_delivery(
        &self,
        log: WebhookDeliveryLog,
    ) -> Pin<Box<dyn Future<Output = AutumnResult<()>> + Send>>;

    /// Optional: List only permanently failed delivery attempts archived in the Dead Letter Queue.
    fn get_dlq_logs(
        &self,
    ) -> Pin<Box<dyn Future<Output = AutumnResult<Vec<WebhookDeliveryLog>>> + Send>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    /// Optional: Get a specific delivery log by ID.
    fn get_delivery_log(
        &self,
        _id: &str,
    ) -> Pin<Box<dyn Future<Output = AutumnResult<Option<WebhookDeliveryLog>>> + Send>> {
        Box::pin(async { Ok(None) })
    }

    /// Optional: Reset consecutive failures for a subscription.
    fn reset_subscription_failures(
        &self,
        _id: &str,
    ) -> Pin<Box<dyn Future<Output = AutumnResult<()>> + Send>> {
        Box::pin(async { Ok(()) })
    }
}

/// Legacy alias for backward compatibility.
pub use OutboundWebhookHandler as OutboundWebhookStore;

/// Bounded, thread-safe, process-local in-memory implementation of the outbound webhook handler.
#[derive(Debug, Default)]
pub struct InMemoryOutboundWebhookHandler {
    subscriptions: RwLock<HashMap<String, WebhookSubscription>>,
    logs: RwLock<HashMap<String, WebhookDeliveryLog>>,
}

/// Legacy alias for backward compatibility.
pub type InMemoryOutboundWebhookStore = InMemoryOutboundWebhookHandler;

impl InMemoryOutboundWebhookHandler {
    /// Create a new, empty in-memory handler.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Helper to register a subscription in memory for testing/dev.
    #[allow(clippy::unused_async)]
    pub async fn create_subscription(
        &self,
        sub: WebhookSubscription,
    ) -> AutumnResult<WebhookSubscription> {
        let mut subs = self
            .subscriptions
            .write()
            .expect("subscriptions write lock poisoned");
        subs.insert(sub.id.clone(), sub.clone());
        Ok(sub)
    }

    /// Helper to retrieve logged deliveries for testing/dev.
    #[allow(clippy::unused_async)]
    pub async fn get_delivery_logs(&self) -> AutumnResult<Vec<WebhookDeliveryLog>> {
        let logs = self.logs.read().expect("logs read lock poisoned");
        let mut list: Vec<WebhookDeliveryLog> = logs.values().cloned().collect();
        list.sort_by_key(|l| l.timestamp);
        list.reverse();
        Ok(list)
    }

    /// Helper to fetch a single subscription.
    #[allow(clippy::unused_async)]
    pub async fn get_subscription(&self, id: &str) -> AutumnResult<Option<WebhookSubscription>> {
        let subs = self
            .subscriptions
            .read()
            .expect("subscriptions read lock poisoned");
        Ok(subs.get(id).cloned())
    }
}

impl OutboundWebhookHandler for InMemoryOutboundWebhookHandler {
    fn get_subscriptions(
        &self,
        topic: &str,
    ) -> Pin<Box<dyn Future<Output = AutumnResult<Vec<WebhookSubscription>>> + Send>> {
        let subs = self
            .subscriptions
            .read()
            .expect("subscriptions read lock poisoned");
        let topic = topic.to_owned();
        let list: Vec<WebhookSubscription> = subs
            .values()
            .filter(|sub| {
                sub.event_topics.iter().any(|t| t == &topic)
                    && sub.status == WebhookSubscriptionStatus::Active
            })
            .cloned()
            .collect();
        Box::pin(async move { Ok(list) })
    }

    fn log_delivery(
        &self,
        log: WebhookDeliveryLog,
    ) -> Pin<Box<dyn Future<Output = AutumnResult<()>> + Send>> {
        let mut logs = self.logs.write().expect("logs write lock poisoned");
        logs.insert(log.id.clone(), log.clone());

        // Manage subscription consecutive failures and auto-disabling state
        let mut subs = self
            .subscriptions
            .write()
            .expect("subscriptions write lock poisoned");
        if let Some(sub) = subs.get_mut(&log.subscription_id) {
            if let Some(status) = log.response_status {
                if (200..300).contains(&status) {
                    sub.consecutive_failures = 0;
                } else {
                    sub.consecutive_failures = sub.consecutive_failures.saturating_add(1);
                    if sub.consecutive_failures >= 50 {
                        sub.status = WebhookSubscriptionStatus::Failed;
                        tracing::warn!(subscription_id = %sub.id, "Webhook subscription auto-disabled due to 50 consecutive failures");
                    }
                }
            } else if log.last_error.is_some() {
                sub.consecutive_failures = sub.consecutive_failures.saturating_add(1);
                if sub.consecutive_failures >= 50 {
                    sub.status = WebhookSubscriptionStatus::Failed;
                    tracing::warn!(subscription_id = %sub.id, "Webhook subscription auto-disabled due to 50 consecutive failures");
                }
            }
        }

        Box::pin(async move { Ok(()) })
    }

    fn get_dlq_logs(
        &self,
    ) -> Pin<Box<dyn Future<Output = AutumnResult<Vec<WebhookDeliveryLog>>> + Send>> {
        let list = {
            let logs = self.logs.read().expect("logs read lock poisoned");
            let mut list: Vec<WebhookDeliveryLog> =
                logs.values().filter(|l| l.is_dlq).cloned().collect();
            list.sort_by_key(|l| l.timestamp);
            list.reverse();
            list
        };
        Box::pin(async move { Ok(list) })
    }

    fn get_delivery_log(
        &self,
        id: &str,
    ) -> Pin<Box<dyn Future<Output = AutumnResult<Option<WebhookDeliveryLog>>> + Send>> {
        let log = self.logs.read().expect("logs read lock poisoned").get(id).cloned();
        Box::pin(async move { Ok(log) })
    }

    fn reset_subscription_failures(
        &self,
        id: &str,
    ) -> Pin<Box<dyn Future<Output = AutumnResult<()>> + Send>> {
        {
            let mut subs = self
                .subscriptions
                .write()
                .expect("subscriptions write lock poisoned");
            if let Some(sub) = subs.get_mut(id) {
                sub.consecutive_failures = 0;
            }
        }
        Box::pin(async move { Ok(()) })
    }
}

/// A runtime delegation callback type to bridge core autumn to autumn-harvest dynamically.
pub type WebhookDelegate = Arc<
    dyn Fn(
            &AppState,
            WebhookSubscription,
            WebhookDeliveryLog,
        ) -> Pin<Box<dyn Future<Output = AutumnResult<()>> + Send>>
        + Send
        + Sync,
>;

/// `AppState` extension for the runtime delegation hook.
#[derive(Clone)]
pub struct WebhookDelegateExt(pub WebhookDelegate);

/// The runtime manager for outbound webhooks.
#[derive(Clone)]
pub struct WebhookOutboundManager {
    handler: Arc<dyn OutboundWebhookHandler>,
    client: Client,
    initial_backoff_ms: u64,
}

impl WebhookOutboundManager {
    /// Create a new webhook manager with a handler.
    pub fn new(handler: Arc<dyn OutboundWebhookHandler>) -> Self {
        Self {
            handler,
            client: Client::new(),
            initial_backoff_ms: 1000,
        }
    }

    /// Set a custom initial backoff for retries.
    #[must_use]
    pub const fn with_initial_backoff_ms(mut self, ms: u64) -> Self {
        self.initial_backoff_ms = ms;
        self
    }

    /// Access the underlying webhook handler (compatibility/actuator support).
    #[must_use]
    pub fn store(&self) -> &Arc<dyn OutboundWebhookHandler> {
        &self.handler
    }

    /// Access the underlying http client.
    #[must_use]
    pub const fn client(&self) -> &Client {
        &self.client
    }

    /// Dispatch a signed webhook payload to all subscriptions interested in `topic`.
    ///
    /// # Errors
    ///
    /// Returns [`AutumnError`] if payload serialization or queueing fails.
    pub async fn dispatch<T: Serialize + Sync>(
        &self,
        state: &AppState,
        topic: &str,
        payload: &T,
    ) -> AutumnResult<()> {
        let serialized = serde_json::to_string(payload).map_err(|e| {
            AutumnError::internal_server_error_msg(format!("failed to serialize payload: {e}"))
        })?;

        let subs = self.handler.get_subscriptions(topic).await?;
        for sub in subs {
            if sub.status == WebhookSubscriptionStatus::Disabled {
                continue;
            }

            let log_id = uuid::Uuid::new_v4().to_string();
            let log = WebhookDeliveryLog {
                id: log_id.clone(),
                subscription_id: sub.id.clone(),
                topic: topic.to_owned(),
                payload: serialized.clone(),
                request_headers: HashMap::new(),
                response_status: None,
                response_body: None,
                elapsed_ms: 0,
                attempt: 1,
                max_attempts: 5,
                is_dlq: false,
                last_error: None,
                timestamp: Utc::now(),
            };

            // Register the initial attempt in local storage
            self.handler.log_delivery(log.clone()).await?;

            // If a delegate extension is registered, run it (delegates to Harvest workflow)
            if let Some(delegate_ext) = state.extension::<WebhookDelegateExt>() {
                tracing::info!(subscription_id = %sub.id, "WebhookOutboundManager::dispatch: delegating webhook delivery via runtime hook");
                (delegate_ext.0)(state, sub, log).await?;
            } else {
                // Fallback: enqueue a standard background job
                let job_payload = serde_json::json!({
                    "log_id": log.id,
                });

                tracing::debug!(subscription_id = %sub.id, "WebhookOutboundManager::dispatch: enqueuing fallback webhook delivery job");
                if let Some(job_client) = crate::job::global_job_client() {
                    job_client
                        .enqueue("autumn_webhook_delivery", job_payload)
                        .await?;
                } else {
                    return Err(AutumnError::internal_server_error_msg(
                        "Global job client is unavailable; fallback webhook delivery job not enqueued",
                    ));
                }
            }
        }

        Ok(())
    }
}

/// Asynchronous background job that delivers a webhook payload (legacy fallback).
#[must_use]
#[allow(clippy::redundant_closure_for_method_calls, clippy::too_many_lines)]
pub fn deliver_webhook_job(
    state: AppState,
    payload: serde_json::Value,
) -> Pin<Box<dyn Future<Output = AutumnResult<()>> + Send + 'static>> {
    Box::pin(async move {
        let manager = state.extension::<WebhookOutboundManager>().ok_or_else(|| {
            AutumnError::internal_server_error_msg("WebhookOutboundManager not found in extensions")
        })?;

        // Support both self-contained payload structure and legacy log_id lookup (for replays)
        let (sub, mut log) = if let Some(sub_val) = payload.get("subscription") {
            let sub: WebhookSubscription =
                serde_json::from_value(sub_val.clone()).map_err(|e| {
                    AutumnError::bad_request_msg(format!("failed to parse subscription: {e}"))
                })?;
            let log: WebhookDeliveryLog = serde_json::from_value(
                payload
                    .get("log")
                    .cloned()
                    .ok_or_else(|| AutumnError::bad_request_msg("missing log in job payload"))?,
            )
            .map_err(|e| AutumnError::bad_request_msg(format!("failed to parse log: {e}")))?;
            (sub, log)
        } else {
            let log_id = payload
                .get("log_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| AutumnError::bad_request_msg("missing log_id in job payload"))?;

            tracing::debug!(log_id = %log_id, "deliver_webhook_job: starting webhook delivery via log lookup");

            let log_opt = manager.store().get_delivery_log(log_id).await?;
            let mut log = log_opt.ok_or_else(|| {
                AutumnError::not_found_msg(format!("delivery log {log_id} not found"))
            })?;

            // If this log has already been attempted (i.e. is running a retry from the job runner),
            // increment the attempt counter and write the pre-send log.
            if log.response_status.is_some() || log.last_error.is_some() {
                log.attempt = log.attempt.saturating_add(1);
                log.response_status = None;
                log.response_body = None;
                log.last_error = None;
                manager.store().log_delivery(log.clone()).await?;
            }

            // Load latest subscription state to respect emergency rotations/disable
            let subs = manager.store().get_subscriptions(&log.topic).await?;
            let sub = subs
                .into_iter()
                .find(|s| s.id == log.subscription_id)
                .ok_or_else(|| {
                    AutumnError::not_found_msg(format!(
                        "subscription {} not found",
                        log.subscription_id
                    ))
                })?;
            (sub, log)
        };

        if sub.status == WebhookSubscriptionStatus::Disabled {
            tracing::info!(subscription_id = %sub.id, "Webhook subscription is disabled; skipping delivery");
            return Ok(());
        }

        // Stripe-style payload signing: t=<timestamp>,v1=<signature>
        let timestamp = Utc::now().timestamp();
        let signing_payload = format!("{timestamp}.{}", log.payload);
        let signature = crate::security::config::hmac_sha256_hex(
            sub.secret.as_bytes(),
            signing_payload.as_bytes(),
        );
        let signature_header = format!("t={timestamp},v1={signature}");

        let mut request_headers = HashMap::new();
        request_headers.insert("Content-Type".to_owned(), "application/json".to_owned());
        request_headers.insert("Autumn-Signature".to_owned(), signature_header.clone());

        let start = std::time::Instant::now();
        let req = manager
            .client
            .named(&sub.target_url)
            .post(&sub.target_url)
            .header("Content-Type", "application/json")
            .header("Autumn-Signature", signature_header)
            .text_body(log.payload.clone());

        let response = req.send().await;
        let elapsed = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);

        tracing::debug!(
            log_id = %log.id,
            status = ?response.as_ref().map(|r| r.status()),
            "deliver_webhook_job: webhook HTTP request finished"
        );

        log.elapsed_ms = elapsed;
        log.timestamp = Utc::now();
        log.request_headers = request_headers;

        match response {
            Ok(res) => {
                let status = res.status();
                log.response_status = Some(status.as_u16());
                let is_success = res.is_success();
                let body_str = res.text();
                log.response_body = Some(body_str);

                if is_success {
                    log.last_error = None;
                    manager.store().log_delivery(log).await?;
                    manager.store().reset_subscription_failures(&sub.id).await?;
                    Ok(())
                } else {
                    let status_err = format!("server returned status: {status}");
                    log.last_error = Some(status_err.clone());
                    manager.store().log_delivery(log.clone()).await?;
                    handle_delivery_failure(&manager, &sub, log, status_err).await
                }
            }
            Err(e) => {
                let error_str = e.to_string();
                log.last_error = Some(error_str.clone());
                manager.store().log_delivery(log.clone()).await?;
                handle_delivery_failure(&manager, &sub, log, error_str).await
            }
        }
    })
}

async fn handle_delivery_failure(
    manager: &WebhookOutboundManager,
    sub: &WebhookSubscription,
    mut log: WebhookDeliveryLog,
    error_msg: String,
) -> AutumnResult<()> {
    if log.attempt < log.max_attempts {
        // Return an error to signal the background job runner to retry this job
        Err(AutumnError::internal_server_error_msg(format!(
            "delivery attempt {} failed, scheduled retry: {error_msg}",
            log.attempt
        )))
    } else {
        log.is_dlq = true;
        manager.store().log_delivery(log).await?;
        // Return Ok(()) to mark the permanently failed job as complete and send to DLQ
        tracing::warn!(subscription_id = %sub.id, "Webhook delivery failed permanently; sent to DLQ: {}", error_msg);
        Ok(())
    }
}

/// `AppBuilder` plugin for outbound signed webhook delivery infrastructure.
pub struct OutboundWebhookPlugin {
    store: Arc<dyn OutboundWebhookHandler>,
    initial_backoff_ms: u64,
}

impl OutboundWebhookPlugin {
    /// Create a new outbound webhook plugin using the specified store.
    #[must_use]
    pub fn new(store: Arc<dyn OutboundWebhookHandler>) -> Self {
        Self {
            store,
            initial_backoff_ms: 1000,
        }
    }

    /// Override the initial backoff retry delay.
    #[must_use]
    pub const fn with_initial_backoff_ms(mut self, ms: u64) -> Self {
        self.initial_backoff_ms = ms;
        self
    }
}

impl crate::plugin::Plugin for OutboundWebhookPlugin {
    fn build(self, app: crate::app::AppBuilder) -> crate::app::AppBuilder {
        let store = self.store;
        let initial_backoff_ms = self.initial_backoff_ms;

        app.on_startup(move |state| {
            let mut manager = WebhookOutboundManager::new(store.clone())
                .with_initial_backoff_ms(initial_backoff_ms);
            if let Some(ext) = state.extension::<crate::http_client::HttpMockRegistryExt>() {
                manager.client = manager.client.with_mock(ext.0.clone());
            }
            state.insert_extension(manager);
            async move { Ok(()) }
        })
        .jobs(vec![crate::job::JobInfo {
            name: "autumn_webhook_delivery".to_string(),
            max_attempts: 10, // Retries are handled durably via the background job engine
            initial_backoff_ms,
            handler: deliver_webhook_job,
        }])
    }
}
