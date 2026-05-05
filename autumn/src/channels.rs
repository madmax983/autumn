//! Named broadcast channel registry for real-time messaging.
//!
//! [`Channels`] provides a lightweight pub-sub primitive with a local
//! in-process backend by default and an optional Redis pub/sub backend for
//! multi-replica fan-out.
//!
//! # Examples
//!
//! ```rust
//! use autumn_web::channels::Channels;
//!
//! let channels = Channels::new(32);
//! let tx = channels.sender("lobby");
//! let mut rx = channels.subscribe("lobby");
//!
//! tx.send("hello").ok();
//! # // In async context: let msg = rx.recv().await.expect("should receive");
//! ```

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::{Arc, Mutex};

use serde::Serialize;
use thiserror::Error;
use tokio::sync::broadcast;

/// A registry of named broadcast channels.
#[derive(Clone)]
pub struct Channels {
    backend: Arc<dyn ChannelsBackend>,
}

/// Backend abstraction for channel fan-out.
pub trait ChannelsBackend: Send + Sync + 'static {
    /// Publish one message to a topic.
    ///
    /// # Errors
    ///
    /// Returns [`ChannelPublishError`] if the backend cannot accept the
    /// publish request.
    fn publish(&self, topic: &str, msg: ChannelMessage) -> Result<usize, ChannelPublishError>;

    /// Ensure a local topic exists and return a keepalive sender handle.
    fn ensure_topic(&self, topic: &str) -> Arc<broadcast::Sender<ChannelMessage>>;

    /// Subscribe to future messages on a topic.
    fn subscribe(&self, topic: &str) -> Subscriber;

    /// Return the number of topics known to this backend.
    fn channel_count(&self) -> usize;

    /// Remove idle local topic registries when supported.
    fn gc(&self);

    /// Return per-topic subscriber and delivery metrics.
    fn snapshot(&self) -> HashMap<String, ChannelStats>;
}

/// Local in-process [`tokio::sync::broadcast`] channel backend.
#[derive(Clone)]
pub struct LocalChannelsBackend {
    inner: Arc<LocalChannelsInner>,
}

struct LocalChannelsInner {
    capacity: usize,
    registry: Mutex<HashMap<String, Arc<broadcast::Sender<ChannelMessage>>>>,
    metrics: Arc<ChannelMetrics>,
}

/// A message sent through a broadcast channel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChannelMessage(pub String);

impl From<String> for ChannelMessage {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for ChannelMessage {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}

impl ChannelMessage {
    /// Get the message content as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consume the message, returning the inner `String`.
    #[must_use]
    pub fn into_string(self) -> String {
        self.0
    }
}

impl std::fmt::Display for ChannelMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Per-topic channel metrics exposed by `/actuator/channels`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct ChannelStats {
    /// Current active subscriber count.
    pub subscriber_count: usize,
    /// Successful local publish attempts for this topic over this process lifetime.
    pub lifetime_publish_count: u64,
    /// Messages dropped because no local receiver accepted them.
    pub dropped_count: u64,
    /// Messages skipped by slow subscribers.
    pub lagged_count: u64,
}

#[derive(Default)]
struct ChannelMetrics {
    counters: Mutex<HashMap<String, ChannelMetricCounters>>,
}

#[derive(Clone, Default)]
struct ChannelMetricCounters {
    lifetime_publish_count: u64,
    dropped_count: u64,
    lagged_count: u64,
}

impl ChannelMetrics {
    fn ensure_topic(&self, topic: &str) {
        let mut counters = self.counters.lock().expect("channel metrics lock poisoned");
        counters.entry(topic.to_owned()).or_default();
    }

    fn record_publish(&self, topic: &str) {
        let mut counters = self.counters.lock().expect("channel metrics lock poisoned");
        let stats = counters.entry(topic.to_owned()).or_default();
        stats.lifetime_publish_count = stats.lifetime_publish_count.saturating_add(1);
    }

    fn record_dropped(&self, topic: &str, count: u64) {
        let mut counters = self.counters.lock().expect("channel metrics lock poisoned");
        let stats = counters.entry(topic.to_owned()).or_default();
        stats.dropped_count = stats.dropped_count.saturating_add(count);
    }

    fn record_lagged(&self, topic: &str, count: u64) {
        let mut counters = self.counters.lock().expect("channel metrics lock poisoned");
        let stats = counters.entry(topic.to_owned()).or_default();
        stats.lagged_count = stats.lagged_count.saturating_add(count);
    }

    fn topics(&self) -> HashSet<String> {
        self.counters
            .lock()
            .expect("channel metrics lock poisoned")
            .keys()
            .cloned()
            .collect()
    }

    fn snapshot_for(&self, topic: &str, subscriber_count: usize) -> ChannelStats {
        let counters = self.counters.lock().expect("channel metrics lock poisoned");
        let stats = counters.get(topic).cloned().unwrap_or_default();
        ChannelStats {
            subscriber_count,
            lifetime_publish_count: stats.lifetime_publish_count,
            dropped_count: stats.dropped_count,
            lagged_count: stats.lagged_count,
        }
    }
}

/// Error returned when a channel backend cannot accept a publish request.
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum ChannelPublishError {
    /// The backend has shut down and can no longer accept publish requests.
    #[error("channel backend is closed")]
    BackendClosed,
}

/// Error returned by the htmx/raw broadcast facade.
#[derive(Debug, Error)]
pub enum BroadcastError {
    /// Raw byte payloads must be UTF-8 because htmx SSE and WebSocket text
    /// transports consume text frames.
    #[error("broadcast payload is not valid UTF-8: {0}")]
    InvalidUtf8(#[from] std::string::FromUtf8Error),

    /// The selected channel backend rejected the publish request.
    #[error(transparent)]
    Publish(#[from] ChannelPublishError),
}

/// Raw broadcast payload accepted by [`Broadcast::publish`].
pub enum BroadcastPayload {
    /// Text payload.
    Text(String),
    /// Byte payload, decoded as UTF-8 before publishing.
    Bytes(Vec<u8>),
}

impl From<&str> for BroadcastPayload {
    fn from(value: &str) -> Self {
        Self::Text(value.to_owned())
    }
}

impl From<String> for BroadcastPayload {
    fn from(value: String) -> Self {
        Self::Text(value)
    }
}

impl From<&String> for BroadcastPayload {
    fn from(value: &String) -> Self {
        Self::Text(value.clone())
    }
}

impl From<Vec<u8>> for BroadcastPayload {
    fn from(value: Vec<u8>) -> Self {
        Self::Bytes(value)
    }
}

impl From<&[u8]> for BroadcastPayload {
    fn from(value: &[u8]) -> Self {
        Self::Bytes(value.to_vec())
    }
}

impl<const N: usize> From<&[u8; N]> for BroadcastPayload {
    fn from(value: &[u8; N]) -> Self {
        Self::Bytes(value.to_vec())
    }
}

/// Productive publishing facade for htmx-oriented applications.
#[derive(Clone)]
pub struct Broadcast {
    channels: Channels,
}

impl Broadcast {
    /// Create a broadcast facade from a channel registry.
    #[must_use]
    pub fn new(channels: Channels) -> Self {
        Self { channels }
    }

    /// Publish a raw UTF-8 payload to a topic.
    ///
    /// ```
    /// use autumn_web::channels::Channels;
    ///
    /// let channels = Channels::new(16);
    /// channels
    ///     .broadcast()
    ///     .publish("feed", b"raw fragment".as_slice())
    ///     .expect("raw publish should succeed");
    /// ```
    ///
    /// # Errors
    ///
    /// Returns [`BroadcastError::InvalidUtf8`] for invalid byte payloads or
    /// [`BroadcastError::Publish`] when the backend rejects the publish.
    pub fn publish(
        &self,
        topic: &str,
        payload: impl Into<BroadcastPayload>,
    ) -> Result<usize, BroadcastError> {
        let message = match payload.into() {
            BroadcastPayload::Text(text) => ChannelMessage(text),
            BroadcastPayload::Bytes(bytes) => ChannelMessage(String::from_utf8(bytes)?),
        };
        Ok(self.channels.publish(topic, message)?)
    }

    /// Publish a Maud fragment wrapped in an htmx out-of-band envelope.
    ///
    /// ```
    /// use autumn_web::channels::Channels;
    /// use maud::html;
    ///
    /// let channels = Channels::new(16);
    /// channels
    ///     .broadcast()
    ///     .publish_html("feed", html! { div id="notice" { "Saved" } })
    ///     .expect("html publish should succeed");
    /// ```
    ///
    /// # Errors
    ///
    /// Returns [`BroadcastError::Publish`] when the selected backend rejects
    /// the publish request.
    #[cfg(feature = "maud")]
    pub fn publish_html(
        &self,
        topic: &str,
        fragment: maud::Markup,
    ) -> Result<usize, BroadcastError> {
        self.publish(topic, htmx_oob_envelope(fragment))
    }
}

#[cfg(feature = "maud")]
fn htmx_oob_envelope(fragment: maud::Markup) -> String {
    maud::html! {
        template hx-swap-oob="true" {
            (fragment)
        }
    }
    .into_string()
}

/// A sender handle for a broadcast channel.
#[derive(Clone)]
pub struct Sender {
    topic: String,
    backend: Arc<dyn ChannelsBackend>,
    _keepalive: Arc<broadcast::Sender<ChannelMessage>>,
}

impl Sender {
    /// Broadcast a message to all current subscribers of this channel.
    ///
    /// Publishing to a topic with no subscribers is not fatal; the backend
    /// records a drop metric and returns `Ok(0)`.
    ///
    /// # Errors
    ///
    /// Returns [`ChannelPublishError`] if the backend is closed.
    pub fn send(&self, msg: impl Into<ChannelMessage>) -> Result<usize, ChannelPublishError> {
        self.backend.publish(&self.topic, msg.into())
    }

    /// Returns the current number of active subscribers.
    #[must_use]
    pub fn receiver_count(&self) -> usize {
        self._keepalive.receiver_count()
    }
}

/// A subscriber handle for a broadcast channel.
pub struct Subscriber {
    topic: String,
    inner: broadcast::Receiver<ChannelMessage>,
    metrics: Arc<ChannelMetrics>,
}

impl Subscriber {
    /// Receive the next message from the channel.
    ///
    /// # Errors
    ///
    /// Returns `RecvError::Closed` if all senders have been dropped, or
    /// `RecvError::Lagged(n)` if messages were skipped.
    pub async fn recv(&mut self) -> Result<ChannelMessage, broadcast::error::RecvError> {
        match self.inner.recv().await {
            Err(broadcast::error::RecvError::Lagged(count)) => {
                self.metrics.record_lagged(&self.topic, count);
                Err(broadcast::error::RecvError::Lagged(count))
            }
            result => result,
        }
    }

    /// Try to receive a message without waiting.
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`broadcast::Receiver::try_recv`].
    pub fn try_recv(&mut self) -> Result<ChannelMessage, broadcast::error::TryRecvError> {
        match self.inner.try_recv() {
            Err(broadcast::error::TryRecvError::Lagged(count)) => {
                self.metrics.record_lagged(&self.topic, count);
                Err(broadcast::error::TryRecvError::Lagged(count))
            }
            result => result,
        }
    }

    /// Convert this subscriber into a stream of channel messages.
    #[cfg(feature = "ws")]
    pub fn into_stream(self) -> impl tokio_stream::Stream<Item = ChannelMessage> {
        use tokio_stream::StreamExt;
        let topic = self.topic;
        let metrics = self.metrics;
        tokio_stream::wrappers::BroadcastStream::new(self.inner).filter_map(move |result| {
            if let Err(tokio_stream::wrappers::errors::BroadcastStreamRecvError::Lagged(count)) =
                &result
            {
                metrics.record_lagged(&topic, *count);
            }
            result.ok()
        })
    }
}

impl LocalChannelsBackend {
    /// Create a local backend with the given per-topic buffer capacity.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            inner: Arc::new(LocalChannelsInner {
                capacity: capacity.clamp(1, 16_384),
                registry: Mutex::new(HashMap::new()),
                metrics: Arc::new(ChannelMetrics::default()),
            }),
        }
    }

    fn get_or_create_sender(&self, topic: &str) -> Arc<broadcast::Sender<ChannelMessage>> {
        let mut registry = self.inner.registry.lock().expect("channels lock poisoned");

        #[allow(clippy::option_if_let_else)]
        if let Some(tx) = registry.get(topic) {
            Arc::clone(tx)
        } else {
            let tx = Arc::new(broadcast::channel(self.inner.capacity).0);
            registry.insert(topic.to_owned(), Arc::clone(&tx));
            tx
        }
    }

    fn publish_local(&self, topic: &str, msg: ChannelMessage) -> usize {
        self.inner.metrics.record_publish(topic);
        self.send_without_publish_metric(topic, msg)
    }

    fn send_without_publish_metric(&self, topic: &str, msg: ChannelMessage) -> usize {
        let tx = self.get_or_create_sender(topic);
        match tx.send(msg) {
            Ok(count) => count,
            Err(_error) => {
                self.inner.metrics.record_dropped(topic, 1);
                0
            }
        }
    }
}

impl ChannelsBackend for LocalChannelsBackend {
    fn publish(&self, topic: &str, msg: ChannelMessage) -> Result<usize, ChannelPublishError> {
        Ok(self.publish_local(topic, msg))
    }

    fn ensure_topic(&self, topic: &str) -> Arc<broadcast::Sender<ChannelMessage>> {
        self.inner.metrics.ensure_topic(topic);
        self.get_or_create_sender(topic)
    }

    fn subscribe(&self, topic: &str) -> Subscriber {
        let tx = self.ensure_topic(topic);
        Subscriber {
            topic: topic.to_owned(),
            inner: tx.subscribe(),
            metrics: Arc::clone(&self.inner.metrics),
        }
    }

    fn channel_count(&self) -> usize {
        let registry = self.inner.registry.lock().expect("channels lock poisoned");
        registry.len()
    }

    fn gc(&self) {
        let mut registry = self.inner.registry.lock().expect("channels lock poisoned");
        registry.retain(|_, tx| tx.receiver_count() > 0 || Arc::strong_count(tx) > 1);
    }

    fn snapshot(&self) -> HashMap<String, ChannelStats> {
        let registry = self.inner.registry.lock().expect("channels lock poisoned");
        let mut topics = self.inner.metrics.topics();
        topics.extend(registry.keys().cloned());

        topics
            .into_iter()
            .map(|topic| {
                let subscriber_count = registry
                    .get(&topic)
                    .map_or(0, |sender| sender.receiver_count());
                let stats = self.inner.metrics.snapshot_for(&topic, subscriber_count);
                (topic, stats)
            })
            .collect()
    }
}

#[cfg(feature = "redis")]
#[derive(Clone)]
struct RedisChannelsBackend {
    local: LocalChannelsBackend,
    publisher: tokio::sync::mpsc::UnboundedSender<RedisPublishCommand>,
    origin_id: String,
    key_prefix: String,
}

#[cfg(feature = "redis")]
struct RedisPublishCommand {
    redis_channel: String,
    envelope: RedisEnvelope,
}

#[cfg(feature = "redis")]
#[derive(serde::Deserialize, serde::Serialize)]
struct RedisEnvelope {
    origin: String,
    topic: String,
    payload: String,
}

/// Channel backend configuration error.
#[derive(Debug, Error)]
pub enum ChannelBackendConfigError {
    /// `channels.backend = "redis"` needs `channels.redis.url`.
    #[error("channels.redis.url is required when channels.backend = \"redis\"")]
    MissingRedisUrl,
    /// Redis URL failed validation by the Redis client.
    #[error("invalid channels.redis.url: {0}")]
    InvalidRedisUrl(String),
    /// The `redis` cargo feature is required for the Redis backend.
    #[error("channels.backend = \"redis\" requires the redis cargo feature")]
    RedisFeatureDisabled,
}

#[cfg(feature = "redis")]
impl RedisChannelsBackend {
    fn from_config(
        config: &crate::config::ChannelConfig,
        shutdown: tokio_util::sync::CancellationToken,
    ) -> Result<Self, ChannelBackendConfigError> {
        let url = config
            .redis
            .url
            .clone()
            .filter(|url| !url.trim().is_empty())
            .ok_or(ChannelBackendConfigError::MissingRedisUrl)?;
        let client = redis::Client::open(url)
            .map_err(|error| ChannelBackendConfigError::InvalidRedisUrl(error.to_string()))?;
        let local = LocalChannelsBackend::new(config.capacity);
        let (publisher, receiver) = tokio::sync::mpsc::unbounded_channel();
        let origin_id = uuid::Uuid::new_v4().to_string();
        let backend = Self {
            local: local.clone(),
            publisher,
            origin_id: origin_id.clone(),
            key_prefix: config.redis.key_prefix.clone(),
        };
        spawn_redis_publisher(client.clone(), receiver, shutdown.clone());
        spawn_redis_listener(
            client,
            local,
            origin_id,
            config.redis.key_prefix.clone(),
            shutdown,
        );
        Ok(backend)
    }

    fn redis_channel(&self, topic: &str) -> String {
        redis_channel_name(&self.key_prefix, topic)
    }
}

#[cfg(feature = "redis")]
fn redis_channel_name(prefix: &str, topic: &str) -> String {
    format!("{prefix}:{topic}")
}

#[cfg(feature = "redis")]
fn redis_channel_pattern(prefix: &str) -> String {
    format!("{prefix}:*")
}

#[cfg(feature = "redis")]
fn spawn_redis_publisher(
    client: redis::Client,
    mut receiver: tokio::sync::mpsc::UnboundedReceiver<RedisPublishCommand>,
    shutdown: tokio_util::sync::CancellationToken,
) {
    tokio::spawn(async move {
        use redis::AsyncCommands as _;
        use redis::aio::{ConnectionManager, ConnectionManagerConfig};

        let mut connection =
            match ConnectionManager::new_lazy_with_config(client, ConnectionManagerConfig::new()) {
                Ok(connection) => connection,
                Err(error) => {
                    tracing::warn!(error = %error, "failed to create Redis channels publisher");
                    return;
                }
            };

        loop {
            tokio::select! {
                () = shutdown.cancelled() => break,
                Some(command) = receiver.recv() => {
                    let Ok(payload) = serde_json::to_string(&command.envelope) else {
                        tracing::warn!("failed to serialize Redis channel envelope");
                        continue;
                    };
                    if let Err(error) = connection
                        .publish::<_, _, usize>(&command.redis_channel, payload)
                        .await
                    {
                        tracing::warn!(error = %error, channel = %command.redis_channel, "Redis channel publish failed");
                    }
                }
                else => break,
            }
        }
    });
}

#[cfg(feature = "redis")]
fn spawn_redis_listener(
    client: redis::Client,
    local: LocalChannelsBackend,
    origin_id: String,
    key_prefix: String,
    shutdown: tokio_util::sync::CancellationToken,
) {
    tokio::spawn(async move {
        use futures::StreamExt as _;

        let pattern = redis_channel_pattern(&key_prefix);
        loop {
            if shutdown.is_cancelled() {
                break;
            }

            let mut pubsub = match client.get_async_pubsub().await {
                Ok(pubsub) => pubsub,
                Err(error) => {
                    tracing::warn!(error = %error, "failed to connect Redis channels listener");
                    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                    continue;
                }
            };

            if let Err(error) = pubsub.psubscribe(&pattern).await {
                tracing::warn!(error = %error, pattern = %pattern, "failed to subscribe Redis channels listener");
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                continue;
            }

            let mut stream = pubsub.on_message();
            loop {
                tokio::select! {
                    () = shutdown.cancelled() => return,
                    message = stream.next() => {
                        let Some(message) = message else {
                            break;
                        };
                        let payload: String = match message.get_payload() {
                            Ok(payload) => payload,
                            Err(error) => {
                                tracing::warn!(error = %error, "failed to decode Redis channel payload");
                                continue;
                            }
                        };
                        let envelope: RedisEnvelope = match serde_json::from_str(&payload) {
                            Ok(envelope) => envelope,
                            Err(error) => {
                                tracing::warn!(error = %error, "failed to parse Redis channel envelope");
                                continue;
                            }
                        };
                        if envelope.origin == origin_id {
                            continue;
                        }
                        local.send_without_publish_metric(
                            &envelope.topic,
                            ChannelMessage(envelope.payload),
                        );
                    }
                }
            }
        }
    });
}

#[cfg(feature = "redis")]
impl ChannelsBackend for RedisChannelsBackend {
    fn publish(&self, topic: &str, msg: ChannelMessage) -> Result<usize, ChannelPublishError> {
        let local_count = self.local.publish_local(topic, msg.clone());
        let command = RedisPublishCommand {
            redis_channel: self.redis_channel(topic),
            envelope: RedisEnvelope {
                origin: self.origin_id.clone(),
                topic: topic.to_owned(),
                payload: msg.into_string(),
            },
        };
        self.publisher
            .send(command)
            .map_err(|_| ChannelPublishError::BackendClosed)?;
        Ok(local_count)
    }

    fn ensure_topic(&self, topic: &str) -> Arc<broadcast::Sender<ChannelMessage>> {
        self.local.ensure_topic(topic)
    }

    fn subscribe(&self, topic: &str) -> Subscriber {
        self.local.subscribe(topic)
    }

    fn channel_count(&self) -> usize {
        self.local.channel_count()
    }

    fn gc(&self) {
        self.local.gc();
    }

    fn snapshot(&self) -> HashMap<String, ChannelStats> {
        self.local.snapshot()
    }
}

impl Channels {
    /// Create a new local channel registry with the given buffer capacity.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self::with_backend(LocalChannelsBackend::new(capacity))
    }

    /// Create a registry from any backend implementation.
    #[must_use]
    pub fn with_backend(backend: impl ChannelsBackend) -> Self {
        Self {
            backend: Arc::new(backend),
        }
    }

    /// Create a registry from a shared backend implementation.
    #[must_use]
    pub fn with_shared_backend(backend: Arc<dyn ChannelsBackend>) -> Self {
        Self { backend }
    }

    /// Create a channel registry from resolved framework config.
    ///
    /// # Errors
    ///
    /// Returns [`ChannelBackendConfigError`] when a Redis backend is requested
    /// without usable Redis configuration or without the `redis` feature.
    pub fn from_config(
        config: &crate::config::ChannelConfig,
        shutdown: tokio_util::sync::CancellationToken,
    ) -> Result<Self, ChannelBackendConfigError> {
        match config.backend {
            crate::config::ChannelBackend::InProcess => Ok(Self::new(config.capacity)),
            crate::config::ChannelBackend::Redis => Self::redis_from_config(config, shutdown),
        }
    }

    #[cfg(feature = "redis")]
    fn redis_from_config(
        config: &crate::config::ChannelConfig,
        shutdown: tokio_util::sync::CancellationToken,
    ) -> Result<Self, ChannelBackendConfigError> {
        Ok(Self::with_backend(RedisChannelsBackend::from_config(
            config, shutdown,
        )?))
    }

    #[cfg(not(feature = "redis"))]
    fn redis_from_config(
        _config: &crate::config::ChannelConfig,
        _shutdown: tokio_util::sync::CancellationToken,
    ) -> Result<Self, ChannelBackendConfigError> {
        Err(ChannelBackendConfigError::RedisFeatureDisabled)
    }

    /// Return a htmx-friendly broadcast facade.
    #[must_use]
    pub fn broadcast(&self) -> Broadcast {
        Broadcast::new(self.clone())
    }

    /// Publish a raw channel message through the selected backend.
    ///
    /// # Errors
    ///
    /// Returns [`ChannelPublishError`] if the backend is closed.
    pub fn publish(
        &self,
        topic: &str,
        msg: impl Into<ChannelMessage>,
    ) -> Result<usize, ChannelPublishError> {
        self.backend.publish(topic, msg.into())
    }

    /// Get or create a sender for the named channel.
    #[must_use]
    pub fn sender(&self, name: &str) -> Sender {
        let keepalive = self.backend.ensure_topic(name);
        Sender {
            topic: name.to_owned(),
            backend: Arc::clone(&self.backend),
            _keepalive: keepalive,
        }
    }

    /// Subscribe to the named channel.
    #[must_use]
    pub fn subscribe(&self, name: &str) -> Subscriber {
        self.backend.subscribe(name)
    }

    /// Authorize a channel subscription before allocating the subscriber.
    ///
    /// The hook receives the requested topic name. If it returns an error,
    /// no subscriber is created and the error is returned unchanged.
    ///
    /// ```rust,no_run
    /// use autumn_web::channels::Channels;
    ///
    /// # async fn example(channels: Channels) -> autumn_web::AutumnResult<()> {
    /// let mut rx = channels
    ///     .subscribe_authorized("private-feed", |topic| async move {
    ///         if topic == "private-feed" {
    ///             Ok(())
    ///         } else {
    ///             Err(autumn_web::AutumnError::forbidden_msg("not your feed"))
    ///         }
    ///     })
    ///     .await?;
    /// # let _ = &mut rx;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// Returns the error produced by the authorization hook.
    pub async fn subscribe_authorized<E, Fut>(
        &self,
        name: &str,
        authorize: impl FnOnce(String) -> Fut,
    ) -> Result<Subscriber, E>
    where
        Fut: Future<Output = Result<(), E>>,
    {
        authorize(name.to_owned()).await?;
        Ok(self.subscribe(name))
    }

    /// Returns the number of active topics in the registry.
    #[must_use]
    pub fn channel_count(&self) -> usize {
        self.backend.channel_count()
    }

    /// Remove channels with no active senders or receivers.
    pub fn gc(&self) {
        self.backend.gc();
    }

    /// Get a snapshot of all active channels and their metrics.
    #[must_use]
    pub fn snapshot(&self) -> HashMap<String, ChannelStats> {
        self.backend.snapshot()
    }

    /// Creates an SSE response stream for a channel.
    #[cfg(feature = "ws")]
    pub fn sse_stream(
        &self,
        name: &str,
    ) -> axum::response::sse::Sse<
        impl tokio_stream::Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>>
        + use<>,
    > {
        crate::sse::from_subscriber(self.subscribe(name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_channels() {
        let channels = Channels::new(16);
        assert_eq!(channels.channel_count(), 0);
    }

    #[test]
    fn sender_creates_channel_lazily() {
        let channels = Channels::new(16);
        let _tx = channels.sender("test");
        assert_eq!(channels.channel_count(), 1);
    }

    #[test]
    fn subscribe_creates_channel_lazily() {
        let channels = Channels::new(16);
        let _rx = channels.subscribe("test");
        assert_eq!(channels.channel_count(), 1);
    }

    #[tokio::test]
    async fn send_and_receive() -> Result<(), broadcast::error::RecvError> {
        let channels = Channels::new(16);
        let tx = channels.sender("chat");
        let mut rx = channels.subscribe("chat");

        tx.send("hello").expect("should send");
        let msg = rx.recv().await?;
        assert_eq!(msg.as_str(), "hello");
        Ok(())
    }

    #[tokio::test]
    async fn multiple_subscribers() -> Result<(), broadcast::error::RecvError> {
        let channels = Channels::new(16);
        let tx = channels.sender("chat");
        let mut rx1 = channels.subscribe("chat");
        let mut rx2 = channels.subscribe("chat");

        tx.send("broadcast").expect("should send");

        let msg1 = rx1.recv().await?;
        let msg2 = rx2.recv().await?;
        assert_eq!(msg1.as_str(), "broadcast");
        assert_eq!(msg2.as_str(), "broadcast");
        Ok(())
    }

    #[test]
    fn sender_receiver_count() {
        let channels = Channels::new(16);
        let tx = channels.sender("chat");
        assert_eq!(tx.receiver_count(), 0);

        let _rx1 = channels.subscribe("chat");
        assert_eq!(tx.receiver_count(), 1);

        let _rx2 = channels.subscribe("chat");
        assert_eq!(tx.receiver_count(), 2);
    }

    #[test]
    fn channel_message_conversions() {
        let msg: ChannelMessage = "hello".into();
        assert_eq!(msg.as_str(), "hello");
        assert_eq!(msg.to_string(), "hello");

        let msg2: ChannelMessage = String::from("world").into();
        assert_eq!(msg2.into_string(), "world");
    }

    #[test]
    #[allow(clippy::redundant_clone)]
    fn channels_is_clone() {
        let channels = Channels::new(16);
        let _cloned = channels.clone();
    }

    #[test]
    fn snapshot_returns_counts() {
        let channels = Channels::new(16);
        let _tx = channels.sender("empty");

        let _tx2 = channels.sender("one");
        let _rx_one = channels.subscribe("one");

        let _tx3 = channels.sender("two");
        let _rx_two_1 = channels.subscribe("two");
        let _rx_two_2 = channels.subscribe("two");

        let snap = channels.snapshot();
        assert_eq!(
            snap.get("empty").map(|stats| stats.subscriber_count),
            Some(0)
        );
        assert_eq!(snap.get("one").map(|stats| stats.subscriber_count), Some(1));
        assert_eq!(snap.get("two").map(|stats| stats.subscriber_count), Some(2));
        assert_eq!(snap.len(), 3);
    }

    #[cfg(all(feature = "ws", feature = "maud"))]
    #[tokio::test]
    async fn broadcast_publish_html_wraps_fragment_in_hx_swap_oob_envelope()
    -> Result<(), broadcast::error::RecvError> {
        let channels = Channels::new(16);
        let broadcast = Broadcast::new(channels.clone());
        let mut rx = channels.subscribe("feed");

        let sent = broadcast
            .publish_html(
                "feed",
                maud::html! {
                    li id="item-1" { "one" }
                },
            )
            .expect("html publish should succeed");

        assert_eq!(sent, 1);
        let msg = rx.recv().await?;
        assert!(msg.as_str().contains("hx-swap-oob"));
        assert!(msg.as_str().contains("<li id=\"item-1\">one</li>"));
        Ok(())
    }

    #[cfg(feature = "ws")]
    #[tokio::test]
    async fn broadcast_publish_raw_bytes_delivers_text_payload()
    -> Result<(), broadcast::error::RecvError> {
        let channels = Channels::new(16);
        let broadcast = Broadcast::new(channels.clone());
        let mut rx = channels.subscribe("raw");

        let sent = broadcast
            .publish("raw", b"hello".as_slice())
            .expect("raw publish should succeed");

        assert_eq!(sent, 1);
        assert_eq!(rx.recv().await?.as_str(), "hello");
        Ok(())
    }

    #[cfg(feature = "ws")]
    #[test]
    fn broadcast_publish_rejects_invalid_utf8_bytes() {
        let channels = Channels::new(16);
        let broadcast = Broadcast::new(channels);

        let error = broadcast
            .publish("raw", vec![0xff, 0xfe])
            .expect_err("invalid UTF-8 should be rejected before publishing");

        assert!(matches!(error, BroadcastError::InvalidUtf8(_)));
    }

    #[cfg(feature = "ws")]
    #[tokio::test]
    async fn snapshot_returns_channel_metrics() -> Result<(), broadcast::error::RecvError> {
        let channels = Channels::new(16);
        let broadcast = Broadcast::new(channels.clone());
        let mut rx = channels.subscribe("metrics");

        broadcast
            .publish("metrics", "one")
            .expect("publish should succeed");
        let _ = rx.recv().await?;

        let snap = channels.snapshot();
        let stats = snap.get("metrics").expect("topic should be tracked");
        assert_eq!(stats.subscriber_count, 1);
        assert_eq!(stats.lifetime_publish_count, 1);
        assert_eq!(stats.dropped_count, 0);
        assert_eq!(stats.lagged_count, 0);
        Ok(())
    }

    #[cfg(feature = "ws")]
    #[tokio::test]
    async fn app_state_broadcast_uses_state_channels() -> Result<(), broadcast::error::RecvError> {
        let state = crate::AppState::for_test();
        let mut rx = state.channels().subscribe("state-topic");

        state
            .broadcast()
            .publish("state-topic", "from-state")
            .expect("publish should succeed");

        assert_eq!(rx.recv().await?.as_str(), "from-state");
        Ok(())
    }

    #[cfg(feature = "ws")]
    #[tokio::test]
    async fn subscribe_authorized_rejects_before_creating_subscriber() {
        let channels = Channels::new(16);

        let result: Result<Subscriber, &'static str> = channels
            .subscribe_authorized("private", |topic| async move {
                assert_eq!(topic, "private");
                Err("denied")
            })
            .await;

        assert!(matches!(result, Err("denied")));
        assert!(!channels.snapshot().contains_key("private"));
    }

    #[cfg(feature = "ws")]
    #[tokio::test]
    async fn subscribe_authorized_allows_after_hook_passes()
    -> Result<(), broadcast::error::RecvError> {
        let channels = Channels::new(16);
        let mut rx = channels
            .subscribe_authorized("private", |topic| async move {
                assert_eq!(topic, "private");
                Ok::<(), std::convert::Infallible>(())
            })
            .await
            .expect("authorization should pass");

        channels
            .broadcast()
            .publish("private", "secret")
            .expect("publish should succeed");

        assert_eq!(rx.recv().await?.as_str(), "secret");
        Ok(())
    }

    #[test]
    fn gc_removes_dead_channels() {
        let channels = Channels::new(16);
        let _tx = channels.sender("alive");
        {
            let _tx = channels.sender("dead");
        }
        assert_eq!(channels.channel_count(), 2);
        channels.gc();
        assert_eq!(channels.channel_count(), 1);
    }

    #[cfg(feature = "ws")]
    #[tokio::test]
    async fn subscriber_into_stream() {
        use tokio_stream::StreamExt;
        let channels = Channels::new(16);
        let tx = channels.sender("test_stream");
        let rx = channels.subscribe("test_stream");

        tx.send("message 1").unwrap();
        tx.send("message 2").unwrap();

        let mut stream = rx.into_stream();
        let msg1 = stream.next().await.unwrap();
        assert_eq!(msg1.as_str(), "message 1");

        let msg2 = stream.next().await.unwrap();
        assert_eq!(msg2.as_str(), "message 2");
    }

    #[cfg(feature = "ws")]
    #[tokio::test]
    async fn channels_sse_stream() {
        let channels = Channels::new(16);
        let tx = channels.sender("test_sse");

        let sse = channels.sse_stream("test_sse");

        tx.send("sse message").unwrap();
        let _stream = sse;
    }
}
