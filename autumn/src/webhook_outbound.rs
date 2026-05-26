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

/// Pluggable storage backend for outbound webhook subscriptions and delivery logs.
pub trait OutboundWebhookStore: Send + Sync + 'static {
    /// Create a new subscription.
    fn create_subscription(
        &self,
        sub: WebhookSubscription,
    ) -> Pin<Box<dyn Future<Output = AutumnResult<WebhookSubscription>> + Send>>;

    /// Get a subscription by ID.
    fn get_subscription(
        &self,
        id: &str,
    ) -> Pin<Box<dyn Future<Output = AutumnResult<Option<WebhookSubscription>>> + Send>>;

    /// List all registered subscriptions.
    fn list_subscriptions(
        &self,
    ) -> Pin<Box<dyn Future<Output = AutumnResult<Vec<WebhookSubscription>>> + Send>>;

    /// List all active subscriptions registered for a specific event topic.
    fn list_subscriptions_for_topic(
        &self,
        topic: &str,
    ) -> Pin<Box<dyn Future<Output = AutumnResult<Vec<WebhookSubscription>>> + Send>>;

    /// Update a subscription's status.
    fn update_subscription_status(
        &self,
        id: &str,
        status: WebhookSubscriptionStatus,
    ) -> Pin<Box<dyn Future<Output = AutumnResult<()>> + Send>>;

    /// Increment the consecutive failure counter for a subscription and return the new count.
    fn increment_subscription_failures(
        &self,
        id: &str,
    ) -> Pin<Box<dyn Future<Output = AutumnResult<u32>> + Send>>;

    /// Reset the consecutive failure counter for a subscription back to 0.
    fn reset_subscription_failures(
        &self,
        id: &str,
    ) -> Pin<Box<dyn Future<Output = AutumnResult<()>> + Send>>;

    /// Log a webhook delivery attempt.
    fn log_delivery(
        &self,
        log: WebhookDeliveryLog,
    ) -> Pin<Box<dyn Future<Output = AutumnResult<()>> + Send>>;

    /// List all logged delivery attempts.
    fn get_delivery_logs(
        &self,
    ) -> Pin<Box<dyn Future<Output = AutumnResult<Vec<WebhookDeliveryLog>>> + Send>>;

    /// List only permanently failed delivery attempts archived in the Dead Letter Queue.
    fn get_dlq_logs(
        &self,
    ) -> Pin<Box<dyn Future<Output = AutumnResult<Vec<WebhookDeliveryLog>>> + Send>>;

    /// Get a specific delivery log by ID.
    fn get_delivery_log(
        &self,
        id: &str,
    ) -> Pin<Box<dyn Future<Output = AutumnResult<Option<WebhookDeliveryLog>>> + Send>>;
}

/// Bounded, thread-safe, process-local in-memory implementation of the outbound webhook store.
#[derive(Debug, Default)]
pub struct InMemoryOutboundWebhookStore {
    subscriptions: RwLock<HashMap<String, WebhookSubscription>>,
    logs: RwLock<HashMap<String, WebhookDeliveryLog>>,
}

impl InMemoryOutboundWebhookStore {
    /// Create a new, empty in-memory store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl OutboundWebhookStore for InMemoryOutboundWebhookStore {
    fn create_subscription(
        &self,
        sub: WebhookSubscription,
    ) -> Pin<Box<dyn Future<Output = AutumnResult<WebhookSubscription>> + Send>> {
        let mut subs = self
            .subscriptions
            .write()
            .expect("subscriptions write lock poisoned");
        subs.insert(sub.id.clone(), sub.clone());
        Box::pin(async move { Ok(sub) })
    }

    fn get_subscription(
        &self,
        id: &str,
    ) -> Pin<Box<dyn Future<Output = AutumnResult<Option<WebhookSubscription>>> + Send>> {
        let subs = self
            .subscriptions
            .read()
            .expect("subscriptions read lock poisoned");
        let sub = subs.get(id).cloned();
        Box::pin(async move { Ok(sub) })
    }

    fn list_subscriptions(
        &self,
    ) -> Pin<Box<dyn Future<Output = AutumnResult<Vec<WebhookSubscription>>> + Send>> {
        let subs = self
            .subscriptions
            .read()
            .expect("subscriptions read lock poisoned");
        let list: Vec<WebhookSubscription> = subs.values().cloned().collect();
        Box::pin(async move { Ok(list) })
    }

    fn list_subscriptions_for_topic(
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

    fn update_subscription_status(
        &self,
        id: &str,
        status: WebhookSubscriptionStatus,
    ) -> Pin<Box<dyn Future<Output = AutumnResult<()>> + Send>> {
        let mut subs = self
            .subscriptions
            .write()
            .expect("subscriptions write lock poisoned");
        let id = id.to_owned();
        if let Some(sub) = subs.get_mut(&id) {
            sub.status = status;
        }
        Box::pin(async move { Ok(()) })
    }

    fn increment_subscription_failures(
        &self,
        id: &str,
    ) -> Pin<Box<dyn Future<Output = AutumnResult<u32>> + Send>> {
        let mut subs = self
            .subscriptions
            .write()
            .expect("subscriptions write lock poisoned");
        let id = id.to_owned();
        let count = if let Some(sub) = subs.get_mut(&id) {
            sub.consecutive_failures = sub.consecutive_failures.saturating_add(1);
            sub.consecutive_failures
        } else {
            0
        };
        Box::pin(async move { Ok(count) })
    }

    fn reset_subscription_failures(
        &self,
        id: &str,
    ) -> Pin<Box<dyn Future<Output = AutumnResult<()>> + Send>> {
        let mut subs = self
            .subscriptions
            .write()
            .expect("subscriptions write lock poisoned");
        let id = id.to_owned();
        if let Some(sub) = subs.get_mut(&id) {
            sub.consecutive_failures = 0;
        }
        Box::pin(async move { Ok(()) })
    }

    fn log_delivery(
        &self,
        log: WebhookDeliveryLog,
    ) -> Pin<Box<dyn Future<Output = AutumnResult<()>> + Send>> {
        let mut logs = self.logs.write().expect("logs write lock poisoned");
        logs.insert(log.id.clone(), log);
        Box::pin(async move { Ok(()) })
    }

    fn get_delivery_logs(
        &self,
    ) -> Pin<Box<dyn Future<Output = AutumnResult<Vec<WebhookDeliveryLog>>> + Send>> {
        let logs = self.logs.read().expect("logs read lock poisoned");
        let mut list: Vec<WebhookDeliveryLog> = logs.values().cloned().collect();
        list.sort_by_key(|l| l.timestamp);
        list.reverse();
        Box::pin(async move { Ok(list) })
    }

    fn get_dlq_logs(
        &self,
    ) -> Pin<Box<dyn Future<Output = AutumnResult<Vec<WebhookDeliveryLog>>> + Send>> {
        let logs = self.logs.read().expect("logs read lock poisoned");
        let mut list: Vec<WebhookDeliveryLog> =
            logs.values().filter(|l| l.is_dlq).cloned().collect();
        list.sort_by_key(|l| l.timestamp);
        list.reverse();
        Box::pin(async move { Ok(list) })
    }

    fn get_delivery_log(
        &self,
        id: &str,
    ) -> Pin<Box<dyn Future<Output = AutumnResult<Option<WebhookDeliveryLog>>> + Send>> {
        let logs = self.logs.read().expect("logs read lock poisoned");
        let log = logs.get(id).cloned();
        Box::pin(async move { Ok(log) })
    }
}

/// The runtime manager for outbound webhooks.
#[derive(Clone)]
pub struct WebhookOutboundManager {
    store: Arc<dyn OutboundWebhookStore>,
    client: Client,
    initial_backoff_ms: u64,
}

impl WebhookOutboundManager {
    /// Create a new webhook manager with a store.
    pub fn new(store: Arc<dyn OutboundWebhookStore>) -> Self {
        Self {
            store,
            client: Client::new(),
            initial_backoff_ms: 1000,
        }
    }

    /// Set a custom initial backoff for retries.
    pub fn with_initial_backoff_ms(mut self, ms: u64) -> Self {
        self.initial_backoff_ms = ms;
        self
    }

    /// Access the underlying webhook store.
    pub fn store(&self) -> &Arc<dyn OutboundWebhookStore> {
        &self.store
    }

    /// Dispatch a signed webhook payload to all subscriptions interested in `topic`.
    ///
    /// # Errors
    ///
    /// Returns [`AutumnError`] if payload serialization or queueing fails.
    pub async fn dispatch<T: Serialize>(
        &self,
        _state: &AppState,
        topic: &str,
        payload: &T,
    ) -> AutumnResult<()> {
        let serialized = serde_json::to_string(payload).map_err(|e| {
            AutumnError::internal_server_error_msg(format!("failed to serialize payload: {e}"))
        })?;

        let subs = self.store.list_subscriptions_for_topic(topic).await?;
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

            self.store.log_delivery(log).await?;

            let job_payload = serde_json::json!({
                "log_id": log_id,
            });

            tracing::debug!(subscription_id = %sub.id, "WebhookOutboundManager::dispatch: enqueuing webhook delivery job");
            if let Some(job_client) = crate::job::global_job_client() {
                job_client
                    .enqueue("autumn_webhook_delivery", job_payload)
                    .await?;
            } else {
                tracing::warn!(
                    "Global job client is unavailable; webhook delivery job not enqueued"
                );
            }
        }

        Ok(())
    }
}

/// Asynchronous background job that delivers a webhook payload.
pub fn deliver_webhook_job(
    state: AppState,
    payload: serde_json::Value,
) -> Pin<Box<dyn Future<Output = AutumnResult<()>> + Send + 'static>> {
    Box::pin(async move {
        let log_id = payload
            .get("log_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AutumnError::bad_request_msg("missing log_id in job payload"))?;

        tracing::debug!(log_id = %log_id, "deliver_webhook_job: starting webhook delivery");

        let manager = state.extension::<WebhookOutboundManager>().ok_or_else(|| {
            AutumnError::internal_server_error_msg("WebhookOutboundManager not found in extensions")
        })?;

        let log_opt = manager.store.get_delivery_log(log_id).await?;
        let mut log = match log_opt {
            Some(l) => l,
            None => {
                return Err(AutumnError::not_found_msg(format!(
                    "delivery log {log_id} not found"
                )));
            }
        };

        let sub_opt = manager.store.get_subscription(&log.subscription_id).await?;
        let sub = match sub_opt {
            Some(s) => s,
            None => {
                return Err(AutumnError::not_found_msg(format!(
                    "subscription {} not found",
                    log.subscription_id
                )));
            }
        };

        if sub.status == WebhookSubscriptionStatus::Disabled {
            tracing::info!(subscription_id = %sub.id, "Webhook subscription is disabled; skipping delivery");
            return Ok(());
        }

        // Stripe-style payload signing: t=<timestamp>,v1=<signature>
        let timestamp = Utc::now().timestamp();
        let signing_payload = format!("{}.{}", timestamp, log.payload);
        let signature = crate::security::config::hmac_sha256_hex(
            sub.secret.as_bytes(),
            signing_payload.as_bytes(),
        );
        let signature_header = format!("t={},v1={}", timestamp, signature);

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
        let elapsed = start.elapsed().as_millis() as u64;

        tracing::debug!(
            log_id = %log_id,
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
                let body_str = res.text().to_owned();
                log.response_body = Some(body_str);

                if is_success {
                    log.last_error = None;
                    manager.store.log_delivery(log).await?;
                    manager.store.reset_subscription_failures(&sub.id).await?;
                    Ok(())
                } else {
                    let status_err = format!("server returned status: {}", status);
                    log.last_error = Some(status_err.clone());
                    manager.store.log_delivery(log.clone()).await?;
                    handle_delivery_failure(&manager, &sub, log, status_err).await
                }
            }
            Err(e) => {
                let error_str = e.to_string();
                log.last_error = Some(error_str.clone());
                manager.store.log_delivery(log.clone()).await?;
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
    let consecutive_failures = manager
        .store
        .increment_subscription_failures(&sub.id)
        .await?;
    if consecutive_failures >= 50 {
        manager
            .store
            .update_subscription_status(&sub.id, WebhookSubscriptionStatus::Failed)
            .await?;
        tracing::warn!(subscription_id = %sub.id, "Webhook subscription auto-disabled due to 50 consecutive failures");
    }

    if log.attempt < log.max_attempts {
        let attempt = log.attempt + 1;
        let base_delay = manager.initial_backoff_ms;
        let multiplier = 2u64.pow(attempt - 1);
        let mut buf = [0u8; 8];
        let jitter = if getrandom::getrandom(&mut buf).is_ok() {
            let val = u64::from_ne_bytes(buf);
            (val % 1000) as f64 / 100.0
        } else {
            5.0
        };
        let delay_ms = base_delay * multiplier + (jitter as u64);

        let log_id = log.id.clone();
        log.attempt = attempt;
        manager.store.log_delivery(log).await?;

        let job_payload = serde_json::json!({
            "log_id": log_id,
        });

        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
            if let Some(job_client) = crate::job::global_job_client() {
                let _ = job_client
                    .enqueue("autumn_webhook_delivery", job_payload)
                    .await;
            }
        });

        Err(AutumnError::internal_server_error_msg(format!(
            "delivery attempt failed, scheduled retry: {error_msg}"
        )))
    } else {
        log.is_dlq = true;
        manager.store.log_delivery(log).await?;
        Err(AutumnError::internal_server_error_msg(format!(
            "delivery failed permanently, sent to DLQ: {error_msg}"
        )))
    }
}

/// AppBuilder plugin for outbound signed webhook delivery infrastructure.
pub struct OutboundWebhookPlugin {
    store: Arc<dyn OutboundWebhookStore>,
    initial_backoff_ms: u64,
}

impl OutboundWebhookPlugin {
    /// Create a new outbound webhook plugin using the specified store.
    #[must_use]
    pub fn new(store: Arc<dyn OutboundWebhookStore>) -> Self {
        Self {
            store,
            initial_backoff_ms: 1000,
        }
    }

    /// Override the initial backoff retry delay.
    #[must_use]
    pub fn with_initial_backoff_ms(mut self, ms: u64) -> Self {
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
            max_attempts: 1, // Retries are handled durably via handle_delivery_failure
            initial_backoff_ms: 1,
            handler: deliver_webhook_job,
        }])
    }
}
